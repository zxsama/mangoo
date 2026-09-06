use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Component, Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicationPhase {
    PendingHash,
    Chunking,
    PacksSealed,
    UploadingData,
    DataDurable,
    UploadingManifest,
    ManifestDurable,
    Canonicalizing,
    Canonicalized,
    EventPublished,
    Failed,
    Cancelled,
    Archived,
}

impl PublicationPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingHash => "pending-hash",
            Self::Chunking => "chunking",
            Self::PacksSealed => "packs-sealed",
            Self::UploadingData => "uploading-data",
            Self::DataDurable => "data-durable",
            Self::UploadingManifest => "uploading-manifest",
            Self::ManifestDurable => "manifest-durable",
            Self::Canonicalizing => "canonicalizing",
            Self::Canonicalized => "canonicalized",
            Self::EventPublished => "event-published",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Archived => "archived",
        }
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        Ok(match value {
            "pending-hash" => Self::PendingHash,
            "chunking" => Self::Chunking,
            "packs-sealed" => Self::PacksSealed,
            "uploading-data" => Self::UploadingData,
            "data-durable" => Self::DataDurable,
            "uploading-manifest" => Self::UploadingManifest,
            "manifest-durable" => Self::ManifestDurable,
            "canonicalizing" => Self::Canonicalizing,
            "canonicalized" => Self::Canonicalized,
            "event-published" => Self::EventPublished,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "archived" => Self::Archived,
            _ => anyhow::bail!("unknown media publication phase"),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicationJob {
    pub intent_uuid: uuid::Uuid,
    pub asset_uuid: uuid::Uuid,
    pub content_revision_uuid: uuid::Uuid,
    pub source_path: String,
    pub expected_source_length: u64,
    pub expected_source_mtime_ms: Option<i64>,
    pub expected_content_hash: Option<[u8; 32]>,
    pub phase: PublicationPhase,
    pub binding_epoch: Option<uuid::Uuid>,
    pub transport_generation: u64,
    pub root_uuid: Option<uuid::Uuid>,
    pub local_binding_uuid: Option<uuid::Uuid>,
    pub binding_generation: Option<u64>,
    pub upload_object_key: Option<String>,
    pub upload_id: Option<String>,
    pub upload_offset: u64,
    pub attempt_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicationObject {
    pub ordinal: u32,
    pub object_kind: String,
    pub object_hash: [u8; 32],
    pub object_key: String,
    pub byte_length: u64,
    pub local_relpath: String,
    pub state: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterializationJob {
    pub content_revision_uuid: uuid::Uuid,
    pub asset_uuid: uuid::Uuid,
    pub destination_relpath: String,
    pub expected_length: u64,
    pub priority: u8,
    pub binding_epoch: Option<uuid::Uuid>,
    pub transport_generation: u64,
    pub root_uuid: Option<uuid::Uuid>,
    pub local_binding_uuid: Option<uuid::Uuid>,
    pub binding_generation: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimedMaterializationJob {
    pub content_revision_uuid: uuid::Uuid,
    pub asset_uuid: uuid::Uuid,
    pub destination_relpath: String,
    pub managed_root: String,
    pub expected_length: u64,
    pub source_mtime_ms: Option<i64>,
    pub manifest_hash: [u8; 32],
    pub content_hash: [u8; 32],
    pub manifest_pack_hash: [u8; 32],
    pub manifest_offset: u64,
    pub manifest_length: u64,
    pub binding_epoch: Option<uuid::Uuid>,
    pub transport_generation: u64,
    pub root_uuid: Option<uuid::Uuid>,
    pub local_binding_uuid: Option<uuid::Uuid>,
    pub binding_generation: Option<u64>,
}

pub fn ensure_publication_jobs(
    connection: &Connection,
    binding_epoch: uuid::Uuid,
    transport_generation: u64,
    now: i64,
) -> anyhow::Result<usize> {
    Ok(connection.execute(
        "INSERT INTO media_publication_jobs(intent_uuid,phase,binding_epoch,transport_generation,created_at,updated_at) SELECT intent_uuid,CASE state WHEN 'failed' THEN 'failed' ELSE 'pending-hash' END,binding_epoch,?1,?2,?2 FROM sync_publication_intents WHERE state IN ('pending-hash','pending-manifest','ready','failed') AND binding_epoch=?3 ON CONFLICT(intent_uuid) DO UPDATE SET transport_generation=excluded.transport_generation,updated_at=excluded.updated_at WHERE media_publication_jobs.binding_epoch=excluded.binding_epoch AND media_publication_jobs.phase IN ('pending-hash','failed') AND media_publication_jobs.owner_token IS NULL",
        params![transport_generation,now,binding_epoch.as_bytes().as_slice()],
    )?)
}

pub fn bind_pending_publication_jobs(
    connection: &Connection,
    binding_epoch: uuid::Uuid,
    transport_generation: u64,
    now: i64,
) -> anyhow::Result<usize> {
    connection.execute(
        "UPDATE sync_publication_intents SET binding_epoch=?1,updated_at=?2 WHERE binding_epoch IS NULL AND state IN ('pending-hash','pending-manifest','ready','failed')",
        params![binding_epoch.as_bytes().as_slice(),now],
    )?;
    let changed = connection.execute(
        "UPDATE media_publication_jobs SET binding_epoch=?1,transport_generation=?2,updated_at=?3 WHERE binding_epoch IS NULL AND phase IN ('pending-hash','failed') AND owner_token IS NULL",
        params![binding_epoch.as_bytes().as_slice(),transport_generation,now],
    )?;
    Ok(changed)
}

pub fn claim_publication_job(
    connection: &Connection,
    owner_token: &str,
    binding_epoch: uuid::Uuid,
    transport_generation: u64,
    now: i64,
    lease_ms: i64,
) -> anyhow::Result<Option<PublicationJob>> {
    let transaction = connection.unchecked_transaction()?;
    let row: Option<(Vec<u8>, Vec<u8>, Vec<u8>, String, i64, Option<i64>, Option<String>, String, Option<Vec<u8>>, i64, Option<String>, Option<String>, i64, i64, Option<Vec<u8>>, Option<Vec<u8>>, Option<i64>)> = transaction.query_row(
        "SELECT j.intent_uuid,i.asset_uuid,i.content_revision_uuid,i.source_path,r.byte_size,r.source_mtime_ms,json_extract(j.source_identity_json,'$.expectedContentSha256'),j.phase,j.binding_epoch,j.transport_generation,j.upload_object_key,j.upload_id,j.upload_offset,j.attempt_count,j.root_uuid,j.local_binding_uuid,j.binding_generation FROM media_publication_jobs j JOIN sync_publication_intents i ON i.intent_uuid=j.intent_uuid JOIN asset_content_revisions r ON r.content_revision_uuid=i.content_revision_uuid WHERE j.binding_epoch=?1 AND j.transport_generation=?2 AND j.phase NOT IN ('canonicalized','event-published','cancelled','archived') AND COALESCE(j.next_attempt_at,0)<=?3 AND (j.owner_token IS NULL OR j.lease_expires_at<?3) AND (j.root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=j.root_uuid AND b.local_binding_uuid=j.local_binding_uuid AND b.binding_generation=j.binding_generation AND b.state='active')) ORDER BY j.priority DESC,j.updated_at,j.intent_uuid LIMIT 1",
        params![binding_epoch.as_bytes().as_slice(),transport_generation,now],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?,row.get(10)?,row.get(11)?,row.get(12)?,row.get(13)?,row.get(14)?,row.get(15)?,row.get(16)?)),
    ).optional()?;
    let Some((
        intent,
        asset,
        revision,
        source_path,
        expected_source_length,
        expected_source_mtime_ms,
        expected_content_hash,
        phase,
        binding,
        generation,
        object_key,
        upload_id,
        upload_offset,
        attempts,
        root_uuid,
        local_binding_uuid,
        binding_generation,
    )) = row
    else {
        return Ok(None);
    };
    let changed = transaction.execute(
        "UPDATE media_publication_jobs SET owner_token=?1,lease_expires_at=?2,attempt_count=attempt_count+1,updated_at=?3 WHERE intent_uuid=?4 AND (owner_token IS NULL OR lease_expires_at<?3) AND (root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=media_publication_jobs.root_uuid AND b.local_binding_uuid=media_publication_jobs.local_binding_uuid AND b.binding_generation=media_publication_jobs.binding_generation AND b.state='active'))",
        params![owner_token, now.saturating_add(lease_ms), now, intent],
    )?;
    if changed != 1 {
        transaction.rollback()?;
        return Ok(None);
    }
    transaction.commit()?;
    Ok(Some(PublicationJob {
        intent_uuid: uuid::Uuid::from_slice(&intent)?,
        asset_uuid: uuid::Uuid::from_slice(&asset)?,
        content_revision_uuid: uuid::Uuid::from_slice(&revision)?,
        source_path,
        expected_source_length: u64::try_from(expected_source_length)?,
        expected_source_mtime_ms,
        expected_content_hash: expected_content_hash
            .as_deref()
            .map(super::parse_hash)
            .transpose()?,
        phase: PublicationPhase::parse(&phase)?,
        binding_epoch: binding.as_deref().map(uuid::Uuid::from_slice).transpose()?,
        transport_generation: generation as u64,
        root_uuid: root_uuid
            .as_deref()
            .map(uuid::Uuid::from_slice)
            .transpose()?,
        local_binding_uuid: local_binding_uuid
            .as_deref()
            .map(uuid::Uuid::from_slice)
            .transpose()?,
        binding_generation: binding_generation.map(u64::try_from).transpose()?,
        upload_object_key: object_key,
        upload_id,
        upload_offset: upload_offset as u64,
        attempt_count: attempts as u32 + 1,
    }))
}

pub fn set_publication_phase(
    connection: &Connection,
    intent_uuid: uuid::Uuid,
    owner_token: &str,
    phase: PublicationPhase,
    now: i64,
) -> anyhow::Result<()> {
    let changed = connection.execute(
        "UPDATE media_publication_jobs SET phase=?1,error_code=NULL,next_attempt_at=NULL,updated_at=?2 WHERE intent_uuid=?3 AND owner_token=?4 AND (root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=media_publication_jobs.root_uuid AND b.local_binding_uuid=media_publication_jobs.local_binding_uuid AND b.binding_generation=media_publication_jobs.binding_generation AND b.state='active'))",
        params![phase.as_str(),now,intent_uuid.as_bytes().as_slice(),owner_token],
    )?;
    if changed != 1 {
        anyhow::bail!("publication job lease is stale");
    }
    Ok(())
}

pub fn release_publication_job(
    connection: &Connection,
    intent_uuid: uuid::Uuid,
    owner_token: &str,
) -> anyhow::Result<()> {
    connection.execute(
        "UPDATE media_publication_jobs SET owner_token=NULL,lease_expires_at=NULL WHERE intent_uuid=?1 AND owner_token=?2",
        params![intent_uuid.as_bytes().as_slice(),owner_token],
    )?;
    Ok(())
}

pub fn fail_publication_job(
    connection: &Connection,
    intent_uuid: uuid::Uuid,
    owner_token: &str,
    error_code: &str,
    next_attempt_at: i64,
    now: i64,
) -> anyhow::Result<()> {
    if error_code.len() > 128 {
        anyhow::bail!("media error code exceeds limit");
    }
    let changed = connection.execute(
        "UPDATE media_publication_jobs SET phase='failed',owner_token=NULL,lease_expires_at=NULL,error_code=?1,next_attempt_at=?2,updated_at=?3 WHERE intent_uuid=?4 AND owner_token=?5 AND (root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=media_publication_jobs.root_uuid AND b.local_binding_uuid=media_publication_jobs.local_binding_uuid AND b.binding_generation=media_publication_jobs.binding_generation AND b.state='active'))",
        params![error_code,next_attempt_at,now,intent_uuid.as_bytes().as_slice(),owner_token],
    )?;
    if changed == 0 {
        connection.execute(
            "UPDATE media_publication_jobs SET owner_token=NULL,lease_expires_at=NULL WHERE intent_uuid=?1 AND owner_token=?2",
            params![intent_uuid.as_bytes().as_slice(), owner_token],
        )?;
    }
    Ok(())
}

pub fn register_publication_object(
    connection: &Connection,
    intent_uuid: uuid::Uuid,
    object: &PublicationObject,
    now: i64,
) -> anyhow::Result<()> {
    validate_relative(&object.local_relpath)?;
    connection.execute(
        "INSERT INTO media_publication_objects(intent_uuid,ordinal,object_kind,object_hash,object_key,byte_length,local_relpath,state,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(intent_uuid,object_kind,ordinal) DO UPDATE SET object_hash=excluded.object_hash,object_key=excluded.object_key,byte_length=excluded.byte_length,local_relpath=excluded.local_relpath,state=excluded.state,updated_at=excluded.updated_at",
        params![intent_uuid.as_bytes().as_slice(),object.ordinal,object.object_kind,object.object_hash.as_slice(),object.object_key,object.byte_length,object.local_relpath,object.state,now],
    )?;
    Ok(())
}

pub fn publication_objects(
    connection: &Connection,
    intent_uuid: uuid::Uuid,
) -> anyhow::Result<Vec<PublicationObject>> {
    let mut statement = connection.prepare("SELECT ordinal,object_kind,object_hash,object_key,byte_length,local_relpath,state FROM media_publication_objects WHERE intent_uuid=?1 ORDER BY CASE object_kind WHEN 'data-pack' THEN 0 WHEN 'manifest-pack' THEN 1 ELSE 2 END,ordinal")?;
    let values = statement
        .query_map([intent_uuid.as_bytes().as_slice()], |row| {
            let hash: Vec<u8> = row.get(2)?;
            let hash: [u8; 32] = hash.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok(PublicationObject {
                ordinal: row.get::<_, i64>(0)? as u32,
                object_kind: row.get(1)?,
                object_hash: hash,
                object_key: row.get(3)?,
                byte_length: row.get::<_, i64>(4)? as u64,
                local_relpath: row.get(5)?,
                state: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(values)
}

pub fn publication_graph_ready(
    connection: &Connection,
    intent_uuid: uuid::Uuid,
) -> anyhow::Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM media_publication_objects WHERE intent_uuid=?1) AND EXISTS(SELECT 1 FROM sync_publication_intents i JOIN media_content_manifest_links l ON l.content_revision_uuid=i.content_revision_uuid JOIN media_manifests m ON m.manifest_hash=l.manifest_hash WHERE i.intent_uuid=?1 AND m.manifest_pack_hash IS NOT NULL AND m.pack_offset IS NOT NULL AND m.record_length IS NOT NULL)",
            [intent_uuid.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

pub fn register_remote_receipt(
    connection: &Connection,
    intent_uuid: uuid::Uuid,
    owner_token: &str,
    object: &PublicationObject,
    binding_epoch: Option<uuid::Uuid>,
    generation: u64,
    now: i64,
) -> anyhow::Result<()> {
    let binding = binding_epoch.map(|value| value.as_bytes().to_vec());
    let transaction = connection.unchecked_transaction()?;
    let fence_matches: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM media_publication_jobs j JOIN sync_local_state s ON s.singleton=1 WHERE j.intent_uuid=?1 AND j.owner_token=?2 AND j.binding_epoch=?3 AND j.transport_generation=?4 AND s.capture_enabled=1 AND s.active_binding_epoch=?3 AND (j.root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=j.root_uuid AND b.local_binding_uuid=j.local_binding_uuid AND b.binding_generation=j.binding_generation AND b.state='active')))",
        params![intent_uuid.as_bytes().as_slice(),owner_token,binding,generation],
        |row| row.get(0),
    )?;
    if !fence_matches {
        anyhow::bail!("media publication receipt fence is stale");
    }
    let conflict: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT object_hash FROM media_remote_objects WHERE object_key=?1 AND object_hash<>?2",
            params![object.object_key, object.object_hash.as_slice()],
            |row| row.get(0),
        )
        .optional()?;
    if conflict.is_some() {
        anyhow::bail!("same media object key has different hash");
    }
    transaction.execute("INSERT INTO media_remote_objects(object_hash,object_kind,object_key,byte_length,binding_epoch,receipt_generation,state,updated_at) VALUES (?1,?2,?3,?4,?5,?6,'durable',?7) ON CONFLICT(object_hash) DO UPDATE SET state='durable',receipt_generation=excluded.receipt_generation,updated_at=excluded.updated_at",params![object.object_hash.as_slice(),object.object_kind,object.object_key,object.byte_length,binding,generation,now])?;
    transaction.execute("UPDATE media_publication_objects SET state='durable',receipt_generation=?1,updated_at=?2 WHERE object_key=?3 AND object_hash=?4",params![generation,now,object.object_key,object.object_hash.as_slice()])?;
    transaction.commit()?;
    Ok(())
}

pub fn persist_sealed_graph(
    connection: &Connection,
    intent_uuid: uuid::Uuid,
    owner_token: &str,
    binding_epoch: uuid::Uuid,
    content_revision_uuid: uuid::Uuid,
    source_mtime_ms: Option<i64>,
    source_file_identity: Option<&str>,
    content_hash: [u8; 32],
    content_length: u64,
    chunks: &[crate::sync::media::format::ManifestChunkV1],
    objects: &[PublicationObject],
    manifest_hash: [u8; 32],
    manifest_pack_hash: [u8; 32],
    manifest_offset: u64,
    manifest_length: u64,
    transport_generation: u64,
    now: i64,
) -> anyhow::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    let owns_current_attempt = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM media_publication_jobs j JOIN sync_local_state s ON s.singleton=1 WHERE j.intent_uuid=?1 AND j.owner_token=?2 AND j.binding_epoch=?3 AND j.transport_generation=?4 AND j.phase='chunking' AND s.capture_enabled=1 AND s.active_binding_epoch=j.binding_epoch AND (j.root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=j.root_uuid AND b.local_binding_uuid=j.local_binding_uuid AND b.binding_generation=j.binding_generation AND b.state='active')))",
        params![intent_uuid.as_bytes().as_slice(),owner_token,binding_epoch.as_bytes().as_slice(),transport_generation],
        |row| row.get::<_, bool>(0),
    )?;
    if !owns_current_attempt {
        anyhow::bail!("publication sealed graph lease is stale");
    }
    for object in objects {
        validate_relative(&object.local_relpath)?;
        transaction.execute("INSERT INTO media_packs(pack_hash,object_key,byte_length,local_relpath,state,created_at,updated_at) VALUES (?1,?2,?3,?4,'sealed',?5,?5) ON CONFLICT(pack_hash) DO UPDATE SET local_relpath=COALESCE(media_packs.local_relpath,excluded.local_relpath),state=CASE WHEN media_packs.state IN ('durable','remote') THEN media_packs.state ELSE 'sealed' END,updated_at=excluded.updated_at",params![object.object_hash.as_slice(),object.object_key,object.byte_length,object.local_relpath,now])?;
        transaction.execute("INSERT INTO media_publication_objects(intent_uuid,ordinal,object_kind,object_hash,object_key,byte_length,local_relpath,state,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'sealed',?8) ON CONFLICT(intent_uuid,object_kind,ordinal) DO UPDATE SET object_hash=excluded.object_hash,object_key=excluded.object_key,byte_length=excluded.byte_length,local_relpath=excluded.local_relpath,state=CASE WHEN media_publication_objects.object_hash=excluded.object_hash AND media_publication_objects.object_key=excluded.object_key AND media_publication_objects.byte_length=excluded.byte_length AND media_publication_objects.state='durable' THEN 'durable' ELSE 'sealed' END,receipt_generation=CASE WHEN media_publication_objects.object_hash=excluded.object_hash AND media_publication_objects.object_key=excluded.object_key AND media_publication_objects.byte_length=excluded.byte_length AND media_publication_objects.state='durable' THEN media_publication_objects.receipt_generation ELSE NULL END,updated_at=excluded.updated_at",params![intent_uuid.as_bytes().as_slice(),object.ordinal,object.object_kind,object.object_hash.as_slice(),object.object_key,object.byte_length,object.local_relpath,now])?;
        transaction.execute("UPDATE media_publication_objects SET state='durable',receipt_generation=?1,updated_at=?2 WHERE intent_uuid=?3 AND object_kind=?4 AND ordinal=?5 AND EXISTS (SELECT 1 FROM media_remote_objects r WHERE r.object_hash=media_publication_objects.object_hash AND r.object_key=media_publication_objects.object_key AND r.byte_length=media_publication_objects.byte_length AND r.receipt_generation=?1 AND r.state IN ('durable','remote'))",params![transport_generation,now,intent_uuid.as_bytes().as_slice(),object.object_kind,object.ordinal])?;
    }
    for chunk in chunks {
        transaction.execute("INSERT INTO media_chunks(chunk_hash,byte_length,pack_hash,pack_offset,state,created_at,updated_at) VALUES (?1,?2,?3,?4,'sealed',?5,?5) ON CONFLICT(chunk_hash) DO UPDATE SET byte_length=excluded.byte_length,pack_hash=COALESCE(media_chunks.pack_hash,excluded.pack_hash),pack_offset=COALESCE(media_chunks.pack_offset,excluded.pack_offset),updated_at=excluded.updated_at",params![super::parse_hash(&chunk.chunk_sha256)?.as_slice(),chunk.byte_length,super::parse_hash(&chunk.pack_sha256)?.as_slice(),chunk.pack_offset,now])?;
    }
    transaction.execute("INSERT INTO media_manifests(manifest_hash,content_hash,byte_length,manifest_pack_hash,pack_offset,record_length,state,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,'sealed',?7,?7) ON CONFLICT(manifest_hash) DO UPDATE SET manifest_pack_hash=excluded.manifest_pack_hash,pack_offset=excluded.pack_offset,record_length=excluded.record_length,updated_at=excluded.updated_at",params![manifest_hash.as_slice(),content_hash.as_slice(),content_length,manifest_pack_hash.as_slice(),manifest_offset,manifest_length,now])?;
    transaction.execute("INSERT INTO media_content_manifest_links(content_revision_uuid,manifest_hash,linked_at) VALUES (?1,?2,?3) ON CONFLICT(content_revision_uuid) DO UPDATE SET manifest_hash=excluded.manifest_hash,linked_at=excluded.linked_at",params![content_revision_uuid.as_bytes().as_slice(),manifest_hash.as_slice(),now])?;
    transaction.execute("UPDATE asset_content_revisions SET manifest_hash=?1,byte_size=?2,source_mtime_ms=?3 WHERE content_revision_uuid=?4",params![manifest_hash.as_slice(),content_length,source_mtime_ms,content_revision_uuid.as_bytes().as_slice()])?;
    let changed = transaction.execute("UPDATE media_publication_jobs SET phase='packs-sealed',source_identity_json=json_object('byteLength',?1,'sourceMtimeMs',?2,'fileIdentity',?3),updated_at=?4 WHERE intent_uuid=?5 AND owner_token=?6 AND binding_epoch=?7 AND transport_generation=?8 AND phase='chunking' AND (root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=media_publication_jobs.root_uuid AND b.local_binding_uuid=media_publication_jobs.local_binding_uuid AND b.binding_generation=media_publication_jobs.binding_generation AND b.state='active'))",params![content_length,source_mtime_ms,source_file_identity,now,intent_uuid.as_bytes().as_slice(),owner_token,binding_epoch.as_bytes().as_slice(),transport_generation])?;
    if changed != 1 {
        anyhow::bail!("publication sealed graph lease is stale");
    }
    transaction.commit()?;
    Ok(())
}

pub fn all_publication_objects_durable(
    connection: &Connection,
    intent_uuid: uuid::Uuid,
    generation: u64,
) -> anyhow::Result<bool> {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM media_publication_objects WHERE intent_uuid=?1) AND NOT EXISTS(SELECT 1 FROM media_publication_objects WHERE intent_uuid=?1 AND (state<>'durable' OR receipt_generation<>?2))",params![intent_uuid.as_bytes().as_slice(),generation],|row|row.get(0)).map_err(Into::into)
}

pub fn enqueue_materialization(
    connection: &Connection,
    job: &MaterializationJob,
    now: i64,
) -> anyhow::Result<()> {
    validate_relative(&job.destination_relpath)?;
    let binding = job.binding_epoch.map(|value| value.as_bytes().to_vec());
    connection.execute("INSERT OR IGNORE INTO media_materialization_jobs(content_revision_uuid,asset_uuid,destination_relpath,phase,expected_length,downloaded_bytes,priority,binding_epoch,transport_generation,attempt_count,created_at,updated_at) VALUES (?1,?2,?3,'pending',?4,0,?5,?6,?7,0,?8,?8)",params![job.content_revision_uuid.as_bytes().as_slice(),job.asset_uuid.as_bytes().as_slice(),job.destination_relpath,job.expected_length,job.priority,binding,job.transport_generation,now])?;
    Ok(())
}

pub fn enqueue_missing_materializations(
    connection: &Connection,
    binding_epoch: uuid::Uuid,
    transport_generation: u64,
    now: i64,
) -> anyhow::Result<usize> {
    Ok(connection.execute(
        "WITH RECURSIVE folder_paths(id,root_id,relpath) AS (SELECT id,root_id,'' FROM folders WHERE parent_id IS NULL UNION ALL SELECT f.id,f.root_id,CASE WHEN p.relpath='' THEN f.name ELSE p.relpath||'/'||f.name END FROM folders f JOIN folder_paths p ON f.parent_id=p.id), candidates AS (SELECT r.content_revision_uuid,r.asset_uuid,CASE WHEN h.is_display_winner=1 THEN f.name ELSE f.name||'.megle-conflict-'||substr(lower(hex(r.content_revision_uuid)),1,12) END AS materialized_name,p.relpath,COALESCE(r.byte_size,0) AS byte_size,CASE WHEN root.root_folder_uuid IS NULL THEN NULL ELSE root.root_uuid END AS root_uuid,binding.local_binding_uuid,binding.binding_generation FROM asset_content_revisions r JOIN asset_content_heads h ON h.content_revision_uuid=r.content_revision_uuid JOIN files f ON f.asset_uuid=r.asset_uuid JOIN folder_paths p ON p.id=f.folder_id JOIN roots root ON root.id=f.root_id LEFT JOIN sync_root_local_bindings binding ON binding.root_id=root.id JOIN media_content_manifest_links l ON l.content_revision_uuid=r.content_revision_uuid JOIN media_manifests m ON m.manifest_hash=l.manifest_hash WHERE h.is_display_winner=1 AND r.manifest_hash IS NOT NULL AND f.status<>'deleted' AND root.enabled=1 AND root.path<>'sync-bootstrap' AND (root.root_folder_uuid IS NULL OR binding.state IN ('materializing','active','reconcile-required')) AND m.manifest_pack_hash IS NOT NULL AND m.pack_offset IS NOT NULL AND m.record_length IS NOT NULL AND NOT EXISTS (SELECT 1 FROM sync_tombstone_tokens t WHERE t.entity_uuid=r.asset_uuid AND NOT EXISTS (SELECT 1 FROM sync_tombstone_removals x WHERE x.observed_token_uuid=t.token_uuid))) INSERT INTO media_materialization_jobs(content_revision_uuid,asset_uuid,destination_relpath,phase,expected_length,downloaded_bytes,priority,binding_epoch,transport_generation,attempt_count,created_at,updated_at,root_uuid,local_binding_uuid,binding_generation) SELECT content_revision_uuid,asset_uuid,CASE WHEN relpath='' THEN materialized_name ELSE relpath||'/'||materialized_name END,'pending',byte_size,0,0,?1,?2,0,?3,?3,root_uuid,local_binding_uuid,binding_generation FROM candidates WHERE true ON CONFLICT(content_revision_uuid) DO UPDATE SET destination_relpath=excluded.destination_relpath,phase=CASE WHEN media_materialization_jobs.destination_relpath<>excluded.destination_relpath OR media_materialization_jobs.binding_epoch<>excluded.binding_epoch OR media_materialization_jobs.transport_generation<>excluded.transport_generation THEN 'pending' ELSE media_materialization_jobs.phase END,downloaded_bytes=CASE WHEN media_materialization_jobs.destination_relpath<>excluded.destination_relpath THEN 0 ELSE media_materialization_jobs.downloaded_bytes END,binding_epoch=excluded.binding_epoch,transport_generation=excluded.transport_generation,owner_token=CASE WHEN media_materialization_jobs.destination_relpath<>excluded.destination_relpath THEN NULL ELSE media_materialization_jobs.owner_token END,lease_expires_at=CASE WHEN media_materialization_jobs.destination_relpath<>excluded.destination_relpath THEN NULL ELSE media_materialization_jobs.lease_expires_at END,updated_at=excluded.updated_at",
        params![binding_epoch.as_bytes().as_slice(),transport_generation,now],
    )?)
}

pub fn claim_materialization_job(
    connection: &Connection,
    owner_token: &str,
    binding_epoch: uuid::Uuid,
    transport_generation: u64,
    now: i64,
    lease_ms: i64,
) -> anyhow::Result<Option<ClaimedMaterializationJob>> {
    type Row = (
        Vec<u8>,
        Vec<u8>,
        String,
        String,
        i64,
        Option<i64>,
        Vec<u8>,
        Vec<u8>,
        Vec<u8>,
        i64,
        i64,
        Option<Vec<u8>>,
        i64,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<i64>,
    );
    let transaction = connection.unchecked_transaction()?;
    let row: Option<Row> = transaction.query_row(
        "SELECT j.content_revision_uuid,j.asset_uuid,j.destination_relpath,root.path,j.expected_length,r.source_mtime_ms,l.manifest_hash,m.content_hash,m.manifest_pack_hash,m.pack_offset,m.record_length,j.binding_epoch,j.transport_generation,j.root_uuid,j.local_binding_uuid,j.binding_generation FROM media_materialization_jobs j JOIN asset_content_revisions r ON r.content_revision_uuid=j.content_revision_uuid JOIN media_content_manifest_links l ON l.content_revision_uuid=j.content_revision_uuid JOIN media_manifests m ON m.manifest_hash=l.manifest_hash JOIN files f ON f.asset_uuid=j.asset_uuid JOIN roots root ON root.id=f.root_id WHERE j.binding_epoch=?1 AND j.transport_generation=?2 AND j.phase NOT IN ('complete','cancelled') AND COALESCE(j.next_attempt_at,0)<=?3 AND (j.owner_token IS NULL OR j.lease_expires_at<?3) AND root.path<>'sync-bootstrap' AND (j.root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=j.root_uuid AND b.local_binding_uuid=j.local_binding_uuid AND b.binding_generation=j.binding_generation AND b.state IN ('materializing','active','reconcile-required'))) ORDER BY j.priority DESC,j.updated_at,j.content_revision_uuid LIMIT 1",
        params![binding_epoch.as_bytes().as_slice(),transport_generation,now],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?,row.get(10)?,row.get(11)?,row.get(12)?,row.get(13)?,row.get(14)?,row.get(15)?)),
    ).optional()?;
    let Some((
        revision,
        asset,
        destination,
        managed_root,
        expected_length,
        source_mtime_ms,
        manifest_hash,
        content_hash,
        manifest_pack_hash,
        manifest_offset,
        manifest_length,
        binding,
        generation,
        root_uuid,
        local_binding_uuid,
        binding_generation,
    )) = row
    else {
        return Ok(None);
    };
    let changed = transaction.execute(
        "UPDATE media_materialization_jobs SET owner_token=?1,lease_expires_at=?2,attempt_count=attempt_count+1,phase='fetching-manifest',error_code=NULL,updated_at=?3 WHERE content_revision_uuid=?4 AND (owner_token IS NULL OR lease_expires_at<?3) AND (root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=media_materialization_jobs.root_uuid AND b.local_binding_uuid=media_materialization_jobs.local_binding_uuid AND b.binding_generation=media_materialization_jobs.binding_generation AND b.state IN ('materializing','active','reconcile-required')))",
        params![owner_token,now.saturating_add(lease_ms),now,revision],
    )?;
    if changed != 1 {
        transaction.rollback()?;
        return Ok(None);
    }
    transaction.commit()?;
    Ok(Some(ClaimedMaterializationJob {
        content_revision_uuid: uuid::Uuid::from_slice(&revision)?,
        asset_uuid: uuid::Uuid::from_slice(&asset)?,
        destination_relpath: destination,
        managed_root,
        expected_length: u64::try_from(expected_length)?,
        source_mtime_ms,
        manifest_hash: manifest_hash
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid manifest hash"))?,
        content_hash: content_hash
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid content hash"))?,
        manifest_pack_hash: manifest_pack_hash
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid manifest pack hash"))?,
        manifest_offset: u64::try_from(manifest_offset)?,
        manifest_length: u64::try_from(manifest_length)?,
        binding_epoch: binding.as_deref().map(uuid::Uuid::from_slice).transpose()?,
        transport_generation: u64::try_from(generation)?,
        root_uuid: root_uuid
            .as_deref()
            .map(uuid::Uuid::from_slice)
            .transpose()?,
        local_binding_uuid: local_binding_uuid
            .as_deref()
            .map(uuid::Uuid::from_slice)
            .transpose()?,
        binding_generation: binding_generation.map(u64::try_from).transpose()?,
    }))
}

pub fn set_materialization_phase(
    connection: &Connection,
    revision: uuid::Uuid,
    owner_token: &str,
    phase: &str,
    downloaded_bytes: u64,
    now: i64,
) -> anyhow::Result<()> {
    if !matches!(
        phase,
        "fetching-manifest" | "downloading" | "assembling" | "publishing"
    ) {
        anyhow::bail!("invalid active materialization phase");
    }
    let changed = connection.execute(
        "UPDATE media_materialization_jobs SET phase=?1,downloaded_bytes=MIN(expected_length,?2),updated_at=?3 WHERE content_revision_uuid=?4 AND owner_token=?5 AND (root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=media_materialization_jobs.root_uuid AND b.local_binding_uuid=media_materialization_jobs.local_binding_uuid AND b.binding_generation=media_materialization_jobs.binding_generation AND b.state IN ('materializing','active','reconcile-required')))",
        params![phase,downloaded_bytes,now,revision.as_bytes().as_slice(),owner_token],
    )?;
    if changed != 1 {
        anyhow::bail!("materialization job lease is stale");
    }
    Ok(())
}

pub fn fail_materialization_job(
    connection: &Connection,
    revision: uuid::Uuid,
    owner_token: &str,
    phase: &str,
    error_code: &str,
    next_attempt_at: i64,
    now: i64,
) -> anyhow::Result<()> {
    if !matches!(
        phase,
        "remote-content-missing" | "corrupt" | "path-conflict" | "disk-full" | "locked" | "failed"
    ) || error_code.len() > 128
    {
        anyhow::bail!("invalid materialization failure state");
    }
    let changed = connection.execute(
        "UPDATE media_materialization_jobs SET phase=?1,owner_token=NULL,lease_expires_at=NULL,error_code=?2,next_attempt_at=?3,updated_at=?4 WHERE content_revision_uuid=?5 AND owner_token=?6 AND (root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=media_materialization_jobs.root_uuid AND b.local_binding_uuid=media_materialization_jobs.local_binding_uuid AND b.binding_generation=media_materialization_jobs.binding_generation AND b.state IN ('materializing','active','reconcile-required')))",
        params![phase,error_code,next_attempt_at,now,revision.as_bytes().as_slice(),owner_token],
    )?;
    if changed == 0 {
        release_materialization_job(connection, revision, owner_token)?;
    }
    Ok(())
}

pub fn release_materialization_job(
    connection: &Connection,
    revision: uuid::Uuid,
    owner_token: &str,
) -> anyhow::Result<()> {
    connection.execute(
        "UPDATE media_materialization_jobs SET phase='pending',owner_token=NULL,lease_expires_at=NULL,error_code=NULL,next_attempt_at=NULL WHERE content_revision_uuid=?1 AND owner_token=?2",
        params![revision.as_bytes().as_slice(), owner_token],
    )?;
    Ok(())
}

pub fn complete_materialization_job(
    connection: &Connection,
    revision: uuid::Uuid,
    asset_uuid: uuid::Uuid,
    owner_token: &str,
    binding_epoch: uuid::Uuid,
    transport_generation: u64,
    source_mtime_ms: Option<i64>,
    expected_length: u64,
    volume_identity: Vec<u8>,
    file_identity: Vec<u8>,
    file_key: Option<String>,
    local_path: String,
    content_hash: [u8; 32],
    now: i64,
) -> anyhow::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    let changed = transaction.execute(
        "UPDATE media_materialization_jobs SET phase='complete',downloaded_bytes=expected_length,owner_token=NULL,lease_expires_at=NULL,error_code=NULL,next_attempt_at=NULL,updated_at=?1 WHERE content_revision_uuid=?2 AND owner_token=?3 AND binding_epoch=?4 AND transport_generation=?5 AND EXISTS (SELECT 1 FROM sync_local_state s WHERE s.singleton=1 AND s.active_binding_epoch=?4) AND (root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=media_materialization_jobs.root_uuid AND b.local_binding_uuid=media_materialization_jobs.local_binding_uuid AND b.binding_generation=media_materialization_jobs.binding_generation AND b.state IN ('materializing','active','reconcile-required')))",
        params![now,revision.as_bytes().as_slice(),owner_token,binding_epoch.as_bytes().as_slice(),transport_generation],
    )?;
    if changed != 1 {
        anyhow::bail!("materialization job lease is stale");
    }
    transaction.execute(
        "UPDATE files SET status='active',size=?1,mtime=COALESCE(?2,mtime),file_key=?3 WHERE asset_uuid=?4",
        params![
            expected_length,
            source_mtime_ms.map(|value| value / 1000),
            file_key,
            asset_uuid.as_bytes().as_slice()
        ],
    )?;
    crate::sync::identity::record_identity(
        &transaction,
        &crate::sync::capture::LocalFileIdentity {
            asset_uuid,
            volume_identity,
            file_identity,
            content_fingerprint: Some(content_hash.to_vec()),
        },
        &local_path,
    )?;
    transaction.execute(
        "UPDATE media SET metadata_status='pending' WHERE file_id IN (SELECT id FROM files WHERE asset_uuid=?1)",
        [asset_uuid.as_bytes().as_slice()],
    )?;
    transaction.commit()?;
    Ok(())
}

pub fn record_verified_range(
    connection: &Connection,
    pack_hash: [u8; 32],
    start: u64,
    end: u64,
    range_hash: [u8; 32],
    relative: &str,
    now: i64,
) -> anyhow::Result<()> {
    validate_relative(relative)?;
    if end <= start {
        anyhow::bail!("verified range is empty");
    }
    connection.execute("INSERT INTO media_verified_ranges(pack_hash,range_start,range_end,range_hash,local_relpath,verified_at) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(pack_hash,range_start,range_end) DO UPDATE SET range_hash=excluded.range_hash,local_relpath=excluded.local_relpath,verified_at=excluded.verified_at",params![pack_hash.as_slice(),start,end,range_hash.as_slice(),relative,now])?;
    Ok(())
}

pub fn verified_range_relpath(
    connection: &Connection,
    pack_hash: [u8; 32],
    start: u64,
    end: u64,
) -> anyhow::Result<Option<(String, [u8; 32])>> {
    let value: Option<(String, Vec<u8>)> = connection
        .query_row(
            "SELECT local_relpath,range_hash FROM media_verified_ranges WHERE pack_hash=?1 AND range_start=?2 AND range_end=?3",
            params![pack_hash.as_slice(),start,end],
            |row| Ok((row.get(0)?,row.get(1)?)),
        )
        .optional()?;
    value
        .map(|(path, hash)| {
            validate_relative(&path)?;
            Ok((
                path,
                hash.try_into()
                    .map_err(|_| anyhow::anyhow!("invalid verified range hash"))?,
            ))
        })
        .transpose()
}

pub fn validate_relative(value: &str) -> anyhow::Result<()> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 1024
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        anyhow::bail!("managed media path must be normalized and relative");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_is_leased_and_reclaimed_after_expiry() {
        let database = crate::db::Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        database
            .add_root(crate::db::NewRoot {
                path: "D:/Media".into(),
                display_name: "Media".into(),
            })
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        let binding = uuid::Uuid::now_v7();
        database
            .begin_sync_capture(binding, uuid::Uuid::now_v7())
            .unwrap();
        let file = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id: 1,
                parent_id: None,
                name: String::new(),
                path_hash: "r".into(),
                mtime: None,
            })
            .unwrap();
        let id = database
            .upsert_file(crate::db::FileUpsert {
                root_id: 1,
                folder_id: file,
                name: "a.jpg".into(),
                ext: "jpg".into(),
                size: 3,
                mtime: 1,
                ctime: None,
                file_key: Some("k".into()),
            })
            .unwrap();
        database
            .create_sync_publication_intent(id, "D:/Media/a.jpg", 3)
            .unwrap();
        let connection = database.connection_for_fsops();
        ensure_publication_jobs(connection, binding, 1, 10).unwrap();
        assert!(claim_publication_job(connection, "a", binding, 1, 10, 10)
            .unwrap()
            .is_some());
        assert!(claim_publication_job(connection, "b", binding, 1, 15, 10)
            .unwrap()
            .is_none());
        assert!(claim_publication_job(connection, "b", binding, 1, 21, 10)
            .unwrap()
            .is_some());
    }

    #[test]
    fn remote_receipt_rejects_stale_binding_after_unbind() {
        let database = crate::db::Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        database
            .add_root(crate::db::NewRoot {
                path: "D:/Media".into(),
                display_name: "Media".into(),
            })
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        let binding = uuid::Uuid::now_v7();
        database
            .begin_sync_capture(binding, uuid::Uuid::now_v7())
            .unwrap();
        let folder = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id: 1,
                parent_id: None,
                name: String::new(),
                path_hash: "r".into(),
                mtime: None,
            })
            .unwrap();
        let file = database
            .upsert_file(crate::db::FileUpsert {
                root_id: 1,
                folder_id: folder,
                name: "a.jpg".into(),
                ext: "jpg".into(),
                size: 3,
                mtime: 1,
                ctime: None,
                file_key: Some("k".into()),
            })
            .unwrap();
        let intent = database
            .create_sync_publication_intent(file, "D:/Media/a.jpg", 3)
            .unwrap();
        let connection = database.connection_for_fsops();
        ensure_publication_jobs(connection, binding, 1, 10).unwrap();
        claim_publication_job(connection, "owner", binding, 1, 10, 1_000)
            .unwrap()
            .unwrap();
        set_publication_phase(connection, intent, "owner", PublicationPhase::Chunking, 10).unwrap();
        let object = PublicationObject {
            ordinal: 0,
            object_kind: "data-pack".into(),
            object_hash: [7; 32],
            object_key: "content/data/07/example.pack".into(),
            byte_length: 3,
            local_relpath: "data/example.pack".into(),
            state: "sealed".into(),
        };
        register_publication_object(connection, intent, &object, 10).unwrap();
        let content_revision: Vec<u8> = connection
            .query_row(
                "SELECT content_revision_uuid FROM sync_publication_intents WHERE intent_uuid=?1",
                [intent.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let content_revision = uuid::Uuid::from_slice(&content_revision).unwrap();
        database.unbind_sync_capture(0).unwrap();
        assert!(persist_sealed_graph(
            connection,
            intent,
            "owner",
            binding,
            content_revision,
            None,
            None,
            [8; 32],
            3,
            &[],
            &[],
            [9; 32],
            [10; 32],
            0,
            3,
            1,
            11,
        )
        .is_err());
        assert!(register_remote_receipt(
            connection,
            intent,
            "owner",
            &object,
            Some(binding),
            1,
            11,
        )
        .is_err());
        let receipt_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM media_remote_objects", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(receipt_count, 0);
    }
}
