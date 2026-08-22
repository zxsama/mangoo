use super::{
    chunker::stable_chunk_file,
    format::{ChunkingProfile, ContentManifestV1, MediaHint},
    inventory::{self, PublicationJob, PublicationObject, PublicationPhase},
    packer::{seal_manifest_pack, DataPackBuilder, SealedObject},
};
use crate::{
    db::{writer::DbWriter, Database},
    sync::provider::{
        ObjectMetadata, ProviderError, ProviderErrorCode, PutImmutableResult, SyncProvider,
        TransportContext, UploadSession,
    },
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::AtomicBool,
};

const UPLOAD_FRAGMENT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub struct SealedPublication {
    pub manifest: ContentManifestV1,
    pub manifest_hash: [u8; 32],
    pub manifest_pack_hash: [u8; 32],
    pub manifest_offset: u64,
    pub manifest_length: u64,
    pub objects: Vec<PublicationObject>,
    pub source_mtime_ms: Option<i64>,
    pub source_file_identity: Option<String>,
}

pub fn build_publication(
    job: &PublicationJob,
    object_root: &Path,
    media_hint: MediaHint,
    cancelled: &AtomicBool,
) -> anyhow::Result<SealedPublication> {
    let mut builder = DataPackBuilder::new();
    let mut data_objects = Vec::<SealedObject>::new();
    let mut manifest_chunks = Vec::new();
    let source = stable_chunk_file(
        Path::new(&job.source_path),
        cancelled,
        |descriptor, bytes| {
            if !builder.push(descriptor.sha256, bytes.to_vec())? {
                let sealed =
                    std::mem::replace(&mut builder, DataPackBuilder::new()).seal(object_root)?;
                crate::sync::fault::hit("media.after-data-seal");
                let base = manifest_chunks.len() as u32;
                manifest_chunks.extend(sealed.1.into_iter().enumerate().map(
                    |(index, mut chunk)| {
                        chunk.ordinal = base + index as u32;
                        chunk
                    },
                ));
                data_objects.push(sealed.0);
                if !builder.push(descriptor.sha256, bytes.to_vec())? {
                    anyhow::bail!("chunk cannot fit empty data pack");
                }
            }
            if builder.should_seal() {
                let sealed =
                    std::mem::replace(&mut builder, DataPackBuilder::new()).seal(object_root)?;
                crate::sync::fault::hit("media.after-data-seal");
                let base = manifest_chunks.len() as u32;
                manifest_chunks.extend(sealed.1.into_iter().enumerate().map(
                    |(index, mut chunk)| {
                        chunk.ordinal = base + index as u32;
                        chunk
                    },
                ));
                data_objects.push(sealed.0);
            }
            Ok(())
        },
    )?;
    if source.identity.byte_length != job.expected_source_length
        // `FileUpsert::mtime` is repository-wide seconds precision, whereas
        // the stable reader observes the filesystem timestamp in milliseconds.
        // Compare at the captured precision; the captured SHA still catches
        // same-second rewrites.
        || !source_mtime_matches_capture(
            job.expected_source_mtime_ms,
            source.identity.source_mtime_ms,
        )
        || job
            .expected_content_hash
            .is_some_and(|expected| source.content_sha256 != expected)
    {
        anyhow::bail!("media source changed since capture");
    }
    if !builder.is_empty() {
        let sealed = builder.seal(object_root)?;
        crate::sync::fault::hit("media.after-data-seal");
        let base = manifest_chunks.len() as u32;
        manifest_chunks.extend(sealed.1.into_iter().enumerate().map(|(index, mut chunk)| {
            chunk.ordinal = base + index as u32;
            chunk
        }));
        data_objects.push(sealed.0);
    }
    let manifest = ContentManifestV1 {
        format_version: super::MEDIA_FORMAT_VERSION,
        byte_length: source.identity.byte_length,
        content_sha256: super::hex_hash(&source.content_sha256),
        media_hint,
        chunking: ChunkingProfile::default(),
        chunks: manifest_chunks,
    };
    let manifest_bytes = manifest.canonical_bytes()?;
    let manifest_hash: [u8; 32] = Sha256::digest(&manifest_bytes).into();
    let (manifest_pack, locations) =
        seal_manifest_pack(object_root, std::slice::from_ref(&manifest))?;
    crate::sync::fault::hit("media.after-manifest-seal");
    let (_, manifest_offset, manifest_length) = locations[0];
    let mut objects = data_objects
        .into_iter()
        .enumerate()
        .map(|(ordinal, object)| PublicationObject {
            ordinal: ordinal as u32,
            object_kind: "data-pack".into(),
            object_hash: object.hash,
            object_key: object.object_key,
            byte_length: object.byte_length,
            local_relpath: object.relative_path.to_string_lossy().replace('\\', "/"),
            state: "sealed".into(),
        })
        .collect::<Vec<_>>();
    objects.push(PublicationObject {
        ordinal: 0,
        object_kind: "manifest-pack".into(),
        object_hash: manifest_pack.hash,
        object_key: manifest_pack.object_key,
        byte_length: manifest_pack.byte_length,
        local_relpath: manifest_pack
            .relative_path
            .to_string_lossy()
            .replace('\\', "/"),
        state: "sealed".into(),
    });
    Ok(SealedPublication {
        manifest,
        manifest_hash,
        manifest_pack_hash: manifest_pack.hash,
        manifest_offset,
        manifest_length,
        objects,
        source_mtime_ms: source.identity.source_mtime_ms,
        source_file_identity: source.identity.file_identity,
    })
}

fn source_mtime_matches_capture(expected_ms: Option<i64>, actual_ms: Option<i64>) -> bool {
    expected_ms
        .is_none_or(|expected| actual_ms.map(|actual| actual / 1000) == Some(expected / 1000))
}

pub fn persist_publication(
    database: &Database,
    job: &PublicationJob,
    owner_token: &str,
    publication: &SealedPublication,
    now: i64,
) -> anyhow::Result<()> {
    let binding_epoch = job
        .binding_epoch
        .ok_or_else(|| anyhow::anyhow!("publication job has no binding epoch"))?;
    inventory::persist_sealed_graph(
        database.connection_for_fsops(),
        job.intent_uuid,
        owner_token,
        binding_epoch,
        job.content_revision_uuid,
        publication.source_mtime_ms,
        publication.source_file_identity.as_deref(),
        super::parse_hash(&publication.manifest.content_sha256)?,
        publication.manifest.byte_length,
        &publication.manifest.chunks,
        &publication.objects,
        publication.manifest_hash,
        publication.manifest_pack_hash,
        publication.manifest_offset,
        publication.manifest_length,
        job.transport_generation,
        now,
    )
}

pub async fn upload_sealed_objects(
    writer: &DbWriter,
    provider: &dyn SyncProvider,
    context: &TransportContext,
    object_root: &Path,
    job: &PublicationJob,
    owner_token: &str,
    now: i64,
) -> anyhow::Result<()> {
    let intent = job.intent_uuid;
    let objects = writer
        .write(move |database| {
            inventory::publication_objects(database.connection_for_fsops(), intent)
        })
        .await?;
    for object in objects {
        if object.state == "durable" {
            continue;
        }
        if let Some(remote) = provider.object_head(context, &object.object_key).await? {
            validate_remote_object(&remote, &object)?;
            let object_for_receipt = object.clone();
            let binding = job.binding_epoch;
            let generation = context.transport_generation;
            let owner = owner_token.to_owned();
            writer
                .write_authoritative(move |database| {
                    inventory::register_remote_receipt(
                        database.connection_for_fsops(),
                        intent,
                        &owner,
                        &object_for_receipt,
                        binding,
                        generation,
                        now,
                    )
                })
                .await?;
            if job.upload_object_key.as_deref() == Some(object.object_key.as_str()) {
                clear_upload_cursor(
                    writer,
                    intent,
                    owner_token,
                    context.transport_generation,
                    now,
                )
                .await?;
            }
            continue;
        }
        let path = object_root.join(&object.local_relpath);
        super::packer::verify_file(&path, object.object_hash, object.byte_length)?;
        let phase = if object.object_kind == "data-pack" {
            PublicationPhase::UploadingData
        } else {
            PublicationPhase::UploadingManifest
        };
        let owner = owner_token.to_owned();
        writer
            .write_authoritative(move |database| {
                inventory::set_publication_phase(
                    database.connection_for_fsops(),
                    intent,
                    &owner,
                    phase,
                    now,
                )
            })
            .await?;
        let result = if object.byte_length <= UPLOAD_FRAGMENT_BYTES as u64 {
            let bytes = std::fs::read(&path)?;
            reconcile_put_result(
                provider,
                context,
                &object,
                provider
                    .object_put_immutable(context, &object.object_key, &bytes, object.object_hash)
                    .await,
            )
            .await?
        } else {
            let persisted_session =
                if job.upload_object_key.as_deref() == Some(object.object_key.as_str()) {
                    job.upload_id.as_ref().map(|upload_id| UploadSession {
                        upload_id: upload_id.clone(),
                        committed_bytes: job.upload_offset,
                    })
                } else {
                    None
                };
            let mut session = match persisted_session.clone() {
                Some(session) => session,
                None => {
                    provider
                        .upload_begin(
                            context,
                            &object.object_key,
                            object.byte_length,
                            object.object_hash,
                        )
                        .await?
                }
            };
            let mut may_restart_stale_session = persisted_session.is_some();
            'session: loop {
                if session.committed_bytes > object.byte_length {
                    anyhow::bail!("provider upload cursor exceeds sealed object");
                }
                persist_upload_cursor(
                    writer,
                    intent,
                    owner_token,
                    &object.object_key,
                    &session.upload_id,
                    session.committed_bytes,
                    context.transport_generation,
                    now,
                )
                .await?;
                let mut file = File::open(&path)?;
                file.seek(SeekFrom::Start(session.committed_bytes))?;
                let mut offset = usize::try_from(session.committed_bytes)?;
                let object_length = usize::try_from(object.byte_length)?;
                let mut buffer = vec![0_u8; UPLOAD_FRAGMENT_BYTES];
                while offset < object_length {
                    let requested = (object_length - offset).min(buffer.len());
                    file.read_exact(&mut buffer[..requested])?;
                    let end = offset + requested;
                    let committed = match provider
                        .upload_write(
                            context,
                            &session.upload_id,
                            offset as u64,
                            &buffer[..requested],
                        )
                        .await
                    {
                        Ok(committed) => committed,
                        Err(error)
                            if may_restart_stale_session
                                && matches!(
                                    error.code,
                                    ProviderErrorCode::NotFound
                                        | ProviderErrorCode::PreconditionFailed
                                ) =>
                        {
                            let _ = provider.upload_abort(context, &session.upload_id).await;
                            clear_upload_cursor(
                                writer,
                                intent,
                                owner_token,
                                context.transport_generation,
                                now,
                            )
                            .await?;
                            session = provider
                                .upload_begin(
                                    context,
                                    &object.object_key,
                                    object.byte_length,
                                    object.object_hash,
                                )
                                .await?;
                            may_restart_stale_session = false;
                            continue 'session;
                        }
                        Err(error) => {
                            break 'session reconcile_put_error(provider, context, &object, error)
                                .await?
                        }
                    };
                    if committed != end as u64 {
                        anyhow::bail!("provider upload cursor did not advance exactly");
                    }
                    persist_upload_cursor(
                        writer,
                        intent,
                        owner_token,
                        &object.object_key,
                        &session.upload_id,
                        committed,
                        context.transport_generation,
                        now,
                    )
                    .await?;
                    offset = end;
                    session.committed_bytes = committed;
                }
                match provider.upload_commit(context, &session.upload_id).await {
                    Ok(result) => break 'session result,
                    Err(error)
                        if may_restart_stale_session
                            && matches!(
                                error.code,
                                ProviderErrorCode::NotFound | ProviderErrorCode::PreconditionFailed
                            ) =>
                    {
                        let _ = provider.upload_abort(context, &session.upload_id).await;
                        clear_upload_cursor(
                            writer,
                            intent,
                            owner_token,
                            context.transport_generation,
                            now,
                        )
                        .await?;
                        session = provider
                            .upload_begin(
                                context,
                                &object.object_key,
                                object.byte_length,
                                object.object_hash,
                            )
                            .await?;
                        may_restart_stale_session = false;
                    }
                    Err(error) => {
                        break 'session reconcile_put_error(provider, context, &object, error)
                            .await?
                    }
                }
            }
        };
        match result {
            PutImmutableResult::Created { .. }
            | PutImmutableResult::AlreadyExistsMatching { .. } => {
                let remote = provider
                    .object_head(context, &object.object_key)
                    .await?
                    .ok_or_else(|| {
                        anyhow::anyhow!("provider did not expose committed immutable object")
                    })?;
                validate_remote_object(&remote, &object)?;
                let object_for_receipt = object.clone();
                let binding = job.binding_epoch;
                let generation = context.transport_generation;
                let owner = owner_token.to_owned();
                writer
                    .write_authoritative(move |database| {
                        inventory::register_remote_receipt(
                            database.connection_for_fsops(),
                            intent,
                            &owner,
                            &object_for_receipt,
                            binding,
                            generation,
                            now,
                        )
                    })
                    .await?;
            }
            PutImmutableResult::HashConflict { .. } => {
                anyhow::bail!("provider immutable object hash conflict")
            }
        }
        clear_upload_cursor(
            writer,
            intent,
            owner_token,
            context.transport_generation,
            now,
        )
        .await?;
        let completed_phase = if object.object_kind == "data-pack" {
            PublicationPhase::DataDurable
        } else {
            PublicationPhase::ManifestDurable
        };
        let owner = owner_token.to_owned();
        writer
            .write_authoritative(move |database| {
                inventory::set_publication_phase(
                    database.connection_for_fsops(),
                    intent,
                    &owner,
                    completed_phase,
                    now,
                )
            })
            .await?;
        crate::sync::fault::hit(if object.object_kind == "data-pack" {
            "media.after-data-receipt"
        } else {
            "media.after-manifest-receipt"
        });
    }
    let generation = context.transport_generation;
    if !writer
        .write(move |database| {
            inventory::all_publication_objects_durable(
                database.connection_for_fsops(),
                intent,
                generation,
            )
        })
        .await?
    {
        anyhow::bail!("media publication dependencies are not durable");
    }
    let owner = owner_token.to_owned();
    writer
        .write_authoritative(move |database| {
            inventory::set_publication_phase(
                database.connection_for_fsops(),
                intent,
                &owner,
                PublicationPhase::Canonicalizing,
                now,
            )
        })
        .await?;
    let manifest_hash = writer
        .write(move |database| publication_manifest_hash(database, intent))
        .await?;
    let manifest_hash = super::parse_hash(&manifest_hash)?;
    writer
        .write_authoritative(move |database| {
            database.bind_sync_manifest(intent, manifest_hash, generation)
        })
        .await?;
    let owner = owner_token.to_owned();
    writer
        .write_authoritative(move |database| {
            inventory::set_publication_phase(
                database.connection_for_fsops(),
                intent,
                &owner,
                PublicationPhase::Canonicalized,
                now,
            )
        })
        .await?;
    Ok(())
}

fn validate_remote_object(
    remote: &ObjectMetadata,
    object: &PublicationObject,
) -> anyhow::Result<()> {
    if remote.key != object.object_key
        || remote.size != object.byte_length
        || remote.sha256 != object.object_hash
    {
        anyhow::bail!("provider immutable object metadata conflict");
    }
    Ok(())
}

async fn reconcile_put_result(
    provider: &dyn SyncProvider,
    context: &TransportContext,
    object: &PublicationObject,
    result: Result<PutImmutableResult, ProviderError>,
) -> anyhow::Result<PutImmutableResult> {
    match result {
        Ok(result) => Ok(result),
        Err(error) => reconcile_put_error(provider, context, object, error).await,
    }
}

async fn reconcile_put_error(
    provider: &dyn SyncProvider,
    context: &TransportContext,
    object: &PublicationObject,
    error: ProviderError,
) -> anyhow::Result<PutImmutableResult> {
    if let Ok(Some(remote)) = provider.object_head(context, &object.object_key).await {
        validate_remote_object(&remote, object)?;
        return Ok(PutImmutableResult::AlreadyExistsMatching { etag: remote.etag });
    }
    Err(error.into())
}

async fn persist_upload_cursor(
    writer: &DbWriter,
    intent: uuid::Uuid,
    owner_token: &str,
    object_key: &str,
    upload_id: &str,
    offset: u64,
    context_generation: u64,
    now: i64,
) -> anyhow::Result<()> {
    let object_key = object_key.to_owned();
    let upload_id = upload_id.to_owned();
    let owner = owner_token.to_owned();
    writer
        .write_authoritative(move |database| {
            let changed = database.connection_for_fsops().execute(
                "UPDATE media_publication_jobs SET upload_object_key=?1,upload_id=?2,upload_offset=?3,updated_at=?4 WHERE intent_uuid=?5 AND owner_token=?6 AND binding_epoch=(SELECT active_binding_epoch FROM sync_local_state WHERE singleton=1 AND capture_enabled=1) AND transport_generation=?7",
                rusqlite::params![object_key,upload_id,offset,now,intent.as_bytes().as_slice(),owner,context_generation],
            )?;
            if changed != 1 {
                anyhow::bail!("publication upload cursor lease is stale");
            }
            Ok(())
        })
        .await
}

async fn clear_upload_cursor(
    writer: &DbWriter,
    intent: uuid::Uuid,
    owner_token: &str,
    context_generation: u64,
    now: i64,
) -> anyhow::Result<()> {
    let owner = owner_token.to_owned();
    writer
        .write_authoritative(move |database| {
            let changed = database.connection_for_fsops().execute(
                "UPDATE media_publication_jobs SET upload_object_key=NULL,upload_id=NULL,upload_offset=0,updated_at=?1 WHERE intent_uuid=?2 AND owner_token=?3 AND binding_epoch=(SELECT active_binding_epoch FROM sync_local_state WHERE singleton=1 AND capture_enabled=1) AND transport_generation=?4",
                rusqlite::params![now,intent.as_bytes().as_slice(),owner,context_generation],
            )?;
            if changed != 1 {
                anyhow::bail!("publication upload cursor lease is stale");
            }
            Ok(())
        })
        .await
}

fn publication_manifest_hash(
    database: &Database,
    intent_uuid: uuid::Uuid,
) -> anyhow::Result<String> {
    database.connection_for_fsops().query_row("SELECT lower(hex(l.manifest_hash)) FROM media_content_manifest_links l JOIN sync_publication_intents i ON i.content_revision_uuid=l.content_revision_uuid WHERE i.intent_uuid=?1",[intent_uuid.as_bytes().as_slice()],|row|row.get(0)).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::source_mtime_matches_capture;

    #[test]
    fn source_mtime_comparison_honors_scanner_precision() {
        assert!(source_mtime_matches_capture(
            Some(1_725_000_123_000),
            Some(1_725_000_123_987),
        ));
        assert!(!source_mtime_matches_capture(
            Some(1_725_000_123_000),
            Some(1_725_000_124_000),
        ));
        assert!(source_mtime_matches_capture(None, Some(42)));
    }
}
