use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};

use super::checkpoint::{
    CatalogProjectionSnapshot, CatalogRecoverySnapshot, Checkpoint, FileSnapshot, FolderSnapshot,
    HostMetadataSnapshot, MetadataSnapshot, RootProjectionSnapshot, SqlValueSnapshot, StreamVector,
    TableSnapshot, TagSnapshot, CHECKPOINT_SCHEMA_VERSION,
};
use super::codec::{canonical_bytes, decode_event, seal_event, sha256, REDUCER_VERSION};
use super::reducer::{legalize_locations, LocationRevision, ProtocolState};
use super::types::{
    ActorEpochUuid, ActorId, BindingEpochUuid, CheckpointUuid, DeviceUuid, EntityUuid,
    EventEnvelope, EventUuid, Hlc, LibraryUuid, Operation, RevisionUuid, StreamKind, StreamUuid,
    Version,
};

#[derive(Debug, Clone)]
pub struct CheckpointFence {
    pub checkpoint_uuid: CheckpointUuid,
    pub library_uuid: LibraryUuid,
    pub local_generation: i64,
    pub max_event_rowid: i64,
    pub catalog_vector: StreamVector,
    pub host_vector: StreamVector,
    pub host_metadata: HostMetadataSnapshot,
    snapshot_path: PathBuf,
}

/// Captures a short, transactionally-consistent fence. Building and fsync are
/// deliberately performed after this transaction so WAL readers are not held.
pub fn capture_checkpoint_fence(
    connection: &Connection,
    host_vector: StreamVector,
    host_metadata: HostMetadataSnapshot,
) -> anyhow::Result<CheckpointFence> {
    let transaction = connection.unchecked_transaction()?;
    let (library, generation): (Vec<u8>, i64) = transaction.query_row("SELECT library_uuid,next_local_mutation_generation FROM sync_local_state WHERE singleton=1", [], |r| Ok((r.get(0)?,r.get(1)?)))?;
    let max_event_rowid =
        transaction.query_row("SELECT COALESCE(MAX(rowid),0) FROM sync_events", [], |r| {
            r.get(0)
        })?;
    let mut statement = transaction.prepare("SELECT device_uuid,stream_uuid,actor_epoch,applied_through_seq FROM sync_actor_frontiers ORDER BY device_uuid,stream_uuid,actor_epoch")?;
    let mut catalog_vector = BTreeMap::new();
    for row in statement.query_map([], |r| {
        Ok((
            r.get::<_, Vec<u8>>(0)?,
            r.get::<_, Vec<u8>>(1)?,
            r.get::<_, Vec<u8>>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })? {
        let (device, stream, epoch, seq) = row?;
        catalog_vector.insert(
            ActorId {
                device_uuid: DeviceUuid(uuid::Uuid::from_slice(&device)?),
                stream_uuid: StreamUuid(uuid::Uuid::from_slice(&stream)?),
                actor_epoch: ActorEpochUuid(uuid::Uuid::from_slice(&epoch)?),
            },
            seq.max(0) as u64,
        );
    }
    drop(statement);
    let checkpoint_uuid = CheckpointUuid::new();
    transaction.execute(
        "INSERT INTO sync_checkpoints(checkpoint_uuid,schema_version,reducer_version,state,capture_local_mutation_generation,created_at) VALUES (?1,?2,?3,'building',?4,?5)",
        params![
            checkpoint_uuid.0.as_bytes().as_slice(),
            CHECKPOINT_SCHEMA_VERSION,
            REDUCER_VERSION,
            generation,
            now_ms()
        ],
    )?;
    transaction.commit()?;
    // A private SQLite backup is the immutable fence image. It is created
    // after the short fence transaction and lets the keyset builder release
    // the live WAL immediately while guaranteeing that every later query sees
    // exactly the same projection/truth/event snapshot.
    let snapshot_path =
        std::env::temp_dir().join(format!("mangoo-checkpoint-{}.sqlite", checkpoint_uuid));
    let mut snapshot = Connection::open(&snapshot_path)?;
    rusqlite::backup::Backup::new(connection, &mut snapshot)?.run_to_completion(
        64,
        std::time::Duration::from_millis(1),
        None,
    )?;
    drop(snapshot);
    Ok(CheckpointFence {
        checkpoint_uuid,
        library_uuid: LibraryUuid(uuid::Uuid::from_slice(&library)?),
        local_generation: generation,
        max_event_rowid,
        catalog_vector,
        host_vector,
        host_metadata,
        snapshot_path,
    })
}

pub fn build_checkpoint(
    connection: &Connection,
    fence: &CheckpointFence,
    spool_dir: &Path,
) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(spool_dir)?;
    let snapshot = Connection::open(&fence.snapshot_path)?;
    let mut state = ProtocolState::default();
    let mut events = Vec::new();
    // Older initial seeds contain deterministic bootstrap events that were not
    // persisted in the live ledger. Carry their exact history forward rather
    // than regenerating shorter/reassigned actor sequences after canonical edits.
    let previous: Option<Vec<u8>> = snapshot.query_row(
        "SELECT checkpoint_uuid FROM sync_checkpoints WHERE state='ready' ORDER BY rowid DESC LIMIT 1",[],|row|row.get(0),
    ).optional()?;
    if let Some(previous) = previous {
        let previous = load_and_validate_checkpoint(
            &snapshot,
            CheckpointUuid(uuid::Uuid::from_slice(&previous)?),
        )?;
        if previous.library_uuid != fence.library_uuid {
            anyhow::bail!("checkpoint lineage identity mismatch");
        }
        let bootstrap_device = DeviceUuid(deterministic_uuid(
            fence.library_uuid.0.as_bytes(),
            b"bootstrap-device",
        ));
        for event in previous
            .events
            .into_iter()
            .filter(|event| event.actor.device_uuid == bootstrap_device)
        {
            if state.apply(&event) {
                events.push(event);
            }
        }
    }
    let mut cursor = 0_i64;
    loop {
        let mut statement = snapshot.prepare("SELECT rowid,payload FROM sync_events WHERE rowid>?1 AND rowid<=?2 ORDER BY rowid LIMIT 100")?;
        let rows: Vec<(i64, Vec<u8>)> = statement
            .query_map(params![cursor, fence.max_event_rowid], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        if rows.is_empty() {
            break;
        }
        for (rowid, payload) in &rows {
            let event = decode_event(payload)?;
            if state.apply(&event) {
                events.push(event);
            }
            cursor = *rowid;
        }
    }
    let mut projection = capture_projection(&snapshot)?;
    capture_root_truth(&snapshot, &mut state)?;
    let bootstrap = bootstrap_projection_events(fence.library_uuid, &projection, &state, &events)?;
    for event in bootstrap {
        if !state.applied_events.contains(&event.event_uuid) {
            state.apply(&event);
            events.push(event);
        }
    }
    complete_portable_projection(&mut projection, &state)?;
    let mut checkpoint_vector = fence.catalog_vector.clone();
    for event in &events {
        checkpoint_vector
            .entry(event.actor.clone())
            .and_modify(|seq| *seq = (*seq).max(event.actor_seq))
            .or_insert(event.actor_seq);
    }
    let recovery = capture_recovery_snapshot(&snapshot)?;
    let index_root = spool_dir.parent().unwrap_or(spool_dir).join("indexes");
    let media_index_shards =
        capture_media_index_shards(&snapshot, fence.checkpoint_uuid, &index_root)?;
    let checkpoint = Checkpoint::build(
        fence.checkpoint_uuid,
        fence.library_uuid,
        checkpoint_vector.clone(),
        fence.host_vector.clone(),
        fence.host_metadata.clone(),
        projection,
        state,
        events,
        recovery,
        media_index_shards,
    )?;
    let payload_checkpoint = checkpoint;
    payload_checkpoint.verify()?;
    let bytes = canonical_bytes(&payload_checkpoint)?;
    let hash = sha256(&bytes);
    let final_path = spool_dir.join(format!("{}.checkpoint", fence.checkpoint_uuid));
    let temporary = spool_dir.join(format!("{}.checkpoint.tmp", fence.checkpoint_uuid));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, &final_path)?;
    if let Ok(directory) = OpenOptions::new().read(true).open(spool_dir) {
        let _ = directory.sync_all();
    }
    super::fault::hit("checkpoint.after-payload-publish");
    let transaction = connection.unchecked_transaction()?;
    transaction.execute("UPDATE sync_checkpoints SET state='ready',spool_path=?1,payload_hash=?2,state_digest=?3,ready_at=?4 WHERE checkpoint_uuid=?5 AND state='building'", params![final_path.to_string_lossy(),hash.as_slice(),payload_checkpoint.digest.as_slice(),now_ms(),fence.checkpoint_uuid.0.as_bytes().as_slice()])?;
    for (actor, seq) in &checkpoint_vector {
        transaction.execute("INSERT INTO sync_checkpoint_watermarks(checkpoint_uuid,stream_uuid,device_uuid,actor_epoch,covered_through_seq) VALUES (?1,?2,?3,?4,?5)", params![fence.checkpoint_uuid.0.as_bytes().as_slice(),actor.stream_uuid.0.as_bytes().as_slice(),actor.device_uuid.0.as_bytes().as_slice(),actor.actor_epoch.0.as_bytes().as_slice(),*seq as i64])?;
    }
    super::fault::hit("checkpoint.after-descriptor-watermarks");
    transaction.commit()?;
    drop(snapshot);
    let _ = fs::remove_file(&fence.snapshot_path);
    Ok(final_path)
}

/// A restored device may materialize no local roots. Retain a complete portable
/// browse seed by reconstructing absent rows from canonical identity/content
/// truth, while keeping observational media metadata for rows held locally.
fn complete_portable_projection(
    projection: &mut CatalogProjectionSnapshot,
    state: &ProtocolState,
) -> anyhow::Result<()> {
    // Root sentinels can precede their canonical location event. Supply only a
    // derived anchor, matching their bootstrap location, so the shared name and
    // parent legalization sees the same namespace as ordinary inbox projection.
    let mut root_ids = projection
        .roots
        .iter()
        .map(|v| v.root_uuid)
        .collect::<std::collections::HashSet<_>>();
    let mut folder_ids = projection
        .folders
        .iter()
        .map(|v| v.uuid)
        .collect::<std::collections::HashSet<_>>();
    let mut file_ids = projection
        .files
        .iter()
        .map(|v| v.uuid)
        .collect::<std::collections::HashSet<_>>();
    let mut metadata_ids = projection
        .metadata
        .iter()
        .map(|v| v.asset_uuid)
        .collect::<std::collections::HashSet<_>>();
    let mut tag_ids = projection
        .tags
        .iter()
        .map(|v| v.uuid)
        .collect::<std::collections::HashSet<_>>();
    let mut membership_ids = projection
        .memberships
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let roots_by_entity = state
        .roots
        .values()
        .flat_map(|root| [(root.root_uuid, root), (root.root_folder_uuid, root)])
        .collect::<std::collections::HashMap<_, _>>();
    let mut location_state = state.clone();
    for root in state.roots.values() {
        let entity = location_state
            .entities
            .entry(root.root_folder_uuid)
            .or_default();
        if entity.winning_location().is_none() {
            let revision_uuid = RevisionUuid(root.root_folder_uuid.0);
            entity.locations.insert(
                revision_uuid,
                LocationRevision {
                    preserve_legal_name: false,
                    revision_uuid,
                    parent_uuid: Some(root.library_root_uuid),
                    name: root.requested_name.clone(),
                    version: Version {
                        hlc: Hlc {
                            wall_ms: 0,
                            logical: 0,
                        },
                        device_uuid: DeviceUuid(uuid::Uuid::nil()),
                        actor_epoch: ActorEpochUuid(uuid::Uuid::nil()),
                        actor_seq: 0,
                        event_uuid: EventUuid(root.root_folder_uuid.0),
                    },
                },
            );
            entity.location_heads.insert(revision_uuid);
        }
    }
    let locations = state
        .roots
        .values()
        .next()
        .map(|root| legalize_locations(&location_state, root.library_root_uuid))
        .unwrap_or_default();
    for root in state
        .roots
        .values()
        .filter(|root| root.lifecycle == "active")
    {
        if root_ids.insert(root.root_uuid) {
            projection.roots.push(RootProjectionSnapshot {
                root_uuid: root.root_uuid,
                root_folder_uuid: root.root_folder_uuid,
                display_name: root.materialized_name.clone(),
            });
        }
        if folder_ids.insert(root.root_folder_uuid) {
            projection.folders.push(FolderSnapshot {
                uuid: root.root_folder_uuid,
                root_uuid: root.root_uuid,
                parent_uuid: None,
                name: String::new(),
                mtime: None,
                status: "active".into(),
            });
        }
    }
    for (uuid, entity) in &state.entities {
        if entity.is_deleted() {
            continue;
        }
        let Some(location) = locations.get(uuid) else {
            continue;
        };
        let Some(parent) = location.parent_uuid else {
            continue;
        };
        let mut cursor = parent;
        let mut seen = std::collections::BTreeSet::new();
        let root = loop {
            if let Some(root) = roots_by_entity.get(&cursor) {
                break (root.lifecycle == "active").then_some(root.root_uuid);
            }
            if !seen.insert(cursor) {
                break None;
            }
            let Some(parent) = locations
                .get(&cursor)
                .and_then(|location| location.parent_uuid)
            else {
                break None;
            };
            cursor = parent;
        };
        let Some(root_uuid) = root else { continue };
        if let Some(content) = entity.winning_content() {
            if file_ids.insert(*uuid) {
                projection.files.push(FileSnapshot {
                    uuid: *uuid,
                    root_uuid,
                    folder_uuid: parent,
                    name: location.display_name.clone(),
                    ext: crate::decode::formats::lowercase_extension(Path::new(
                        &location.display_name,
                    )),
                    size: i64::try_from(
                        content
                            .byte_length
                            .context("portable content length missing")?,
                    )?,
                    mtime: content.source_mtime_ms.unwrap_or(0),
                    ctime: None,
                    status: "active".into(),
                    media_kind: crate::decode::formats::media_kind_for_extension(
                        &crate::decode::formats::lowercase_extension(Path::new(
                            &location.display_name,
                        )),
                    )
                    .map(ToOwned::to_owned),
                });
            }
            if metadata_ids.insert(*uuid)
                && ["rating", "favorite", "note"]
                    .iter()
                    .any(|field| entity.fields.contains_key(*field))
            {
                projection.metadata.push(MetadataSnapshot {
                    asset_uuid: *uuid,
                    rating: entity
                        .fields
                        .get("rating")
                        .and_then(|field| field.value.as_i64()),
                    favorite: entity
                        .fields
                        .get("favorite")
                        .and_then(|field| field.value.as_bool())
                        .unwrap_or(false),
                    note: entity
                        .fields
                        .get("note")
                        .and_then(|field| field.value.as_str())
                        .map(ToOwned::to_owned),
                    updated_at: 0,
                });
            }
            for tag in entity.live_set_members("tags") {
                if membership_ids.insert((*uuid, tag)) {
                    projection.memberships.push((*uuid, tag));
                }
            }
        } else if entity
            .fields
            .get("entity-kind")
            .and_then(|field| field.value.as_str())
            == Some("folder")
            && folder_ids.insert(*uuid)
        {
            projection.folders.push(FolderSnapshot {
                uuid: *uuid,
                root_uuid,
                parent_uuid: Some(parent),
                name: location.display_name.clone(),
                mtime: None,
                status: "active".into(),
            });
        }
    }
    for (uuid, entity) in &state.entities {
        if !entity.is_deleted()
            && entity
                .fields
                .get("entity-kind")
                .and_then(|field| field.value.as_str())
                == Some("tag")
            && tag_ids.insert(*uuid)
        {
            projection.tags.push(TagSnapshot {
                uuid: *uuid,
                name: entity
                    .fields
                    .get("name")
                    .and_then(|field| field.value.as_str())
                    .unwrap_or_default()
                    .into(),
                color: entity
                    .fields
                    .get("color")
                    .and_then(|field| field.value.as_str())
                    .map(ToOwned::to_owned),
            });
        }
    }
    projection.roots.sort_by_key(|root| root.root_uuid);
    projection.folders.sort_by_key(|folder| folder.uuid);
    projection.files.sort_by_key(|file| file.uuid);
    projection.tags.sort_by_key(|tag| tag.uuid);
    projection
        .metadata
        .sort_by_key(|metadata| metadata.asset_uuid);
    projection.memberships.sort();
    Ok(())
}

/// Captures the same canonical protocol truth used by checkpoints without
/// writing a checkpoint file. Re-enable comparison uses this boundary so it
/// never falls back to filesystem mtimes or other projection-only metadata.
pub fn capture_protocol_state(
    connection: &Connection,
) -> anyhow::Result<(ProtocolState, StreamVector)> {
    let library = connection.query_row(
        "SELECT library_uuid FROM sync_local_state WHERE singleton=1",
        [],
        |row| row.get::<_, Vec<u8>>(0),
    )?;
    let library = LibraryUuid(uuid::Uuid::from_slice(&library)?);
    let mut state = ProtocolState::default();
    let mut events = Vec::new();
    let mut statement = connection.prepare("SELECT payload FROM sync_events ORDER BY rowid")?;
    for payload in statement.query_map([], |row| row.get::<_, Vec<u8>>(0))? {
        let payload = payload?;
        let event = decode_event(&payload)?;
        state.apply(&event);
        events.push(event);
    }
    drop(statement);
    capture_root_truth(connection, &mut state)?;
    let projection = capture_projection(connection)?;
    for event in bootstrap_projection_events(library, &projection, &state, &events)? {
        if !state.applied_events.contains(&event.event_uuid) {
            state.apply(&event);
            events.push(event);
        }
    }
    let mut vector = StreamVector::new();
    let mut statement = connection.prepare(
        "SELECT device_uuid,stream_uuid,actor_epoch,applied_through_seq FROM sync_actor_frontiers ORDER BY device_uuid,stream_uuid,actor_epoch",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })? {
        let (device, stream, epoch, sequence) = row?;
        vector.insert(
            ActorId {
                device_uuid: DeviceUuid(uuid::Uuid::from_slice(&device)?),
                stream_uuid: StreamUuid(uuid::Uuid::from_slice(&stream)?),
                actor_epoch: ActorEpochUuid(uuid::Uuid::from_slice(&epoch)?),
            },
            sequence.max(0) as u64,
        );
    }
    for event in events {
        vector
            .entry(event.actor)
            .and_modify(|sequence| *sequence = (*sequence).max(event.actor_seq))
            .or_insert(event.actor_seq);
    }
    Ok((state, vector))
}

const RECOVERY_TABLES: &[&str] = &[
    "sync_conflicts",
    "sync_bulk_operations",
    "sync_bulk_items",
    "asset_content_revisions",
    "asset_content_heads",
    "asset_content_supersedes",
    "sync_publication_intents",
];

const SYNC_PUBLICATION_INTENT_RECOVERY_COLUMNS: &[&str] = &[
    "intent_uuid",
    "asset_uuid",
    "content_revision_uuid",
    "event_uuid",
    "binding_epoch",
    "local_mutation_generation",
    "source_path",
    "state",
    "attempt_count",
    "next_attempt_at",
    "error_code",
    "created_at",
    "updated_at",
];

fn capture_recovery_snapshot(connection: &Connection) -> anyhow::Result<CatalogRecoverySnapshot> {
    use rusqlite::types::ValueRef;
    let mut tables = Vec::new();
    for table in RECOVERY_TABLES {
        let selected_columns = if *table == "sync_publication_intents" {
            SYNC_PUBLICATION_INTENT_RECOVERY_COLUMNS.join(",")
        } else {
            "*".to_owned()
        };
        let recovery_filter = if *table == "sync_publication_intents" {
            " WHERE state IN ('pending-hash','pending-manifest','ready','failed')"
        } else {
            ""
        };
        let mut statement = connection.prepare(&format!(
            "SELECT {selected_columns} FROM {table}{recovery_filter} ORDER BY rowid"
        ))?;
        let columns: Vec<String> = statement
            .column_names()
            .iter()
            .map(|v| (*v).to_owned())
            .collect();
        let count = statement.column_count();
        let mut rows = statement
            .query_map([], |row| {
                (0..count)
                    .map(|index| match row.get_ref(index)? {
                        ValueRef::Null => Ok(SqlValueSnapshot::Null),
                        ValueRef::Integer(value) => Ok(SqlValueSnapshot::Integer(value)),
                        ValueRef::Real(value) => {
                            Ok(SqlValueSnapshot::Real(value.to_bits().to_string()))
                        }
                        ValueRef::Text(value) => Ok(SqlValueSnapshot::Text(
                            String::from_utf8_lossy(value).into_owned(),
                        )),
                        ValueRef::Blob(value) => Ok(SqlValueSnapshot::Blob(value.to_vec())),
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()
            })?
            .collect::<Result<Vec<_>, _>>()?;
        // Checkpoints are portable protocol objects. Publication source paths
        // are only useful to the originating device while the intent is live;
        // retaining them after publication would leak an absolute local path
        // into every remote recovery snapshot. The fixed column list above
        // omits Catalog-local root/binding/generation execution ownership.
        if *table == "sync_publication_intents" {
            let source_path_index = columns
                .iter()
                .position(|column| column == "source_path")
                .ok_or_else(|| anyhow::anyhow!("publication snapshot is missing source_path"))?;
            for row in &mut rows {
                row[source_path_index] = SqlValueSnapshot::Text("checkpoint-redacted".to_owned());
            }
        }
        tables.push(TableSnapshot {
            table: (*table).to_owned(),
            columns,
            rows,
        });
    }
    let portable_complete: bool = connection.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM sync_file_seed_progress) AND NOT EXISTS(SELECT 1 FROM sync_publication_intents WHERE state IN ('pending-hash','pending-manifest','ready','failed')) AND NOT EXISTS(SELECT 1 FROM media_publication_jobs WHERE phase NOT IN ('event-published','cancelled','archived'))",
        [],
        |row| row.get(0),
    )?;
    Ok(CatalogRecoverySnapshot {
        tables,
        portable_complete,
    })
}

fn capture_media_index_shards(
    connection: &Connection,
    checkpoint_uuid: CheckpointUuid,
    index_root: &Path,
) -> anyhow::Result<Vec<super::checkpoint::MediaIndexShardRef>> {
    use super::media::format::{IndexShardEntryV1, IndexShardV1};
    fn persist_shard(
        index_root: &Path,
        checkpoint_uuid: CheckpointUuid,
        kind: &str,
        ordinal: usize,
        entries: &mut Vec<IndexShardEntryV1>,
        shards: &mut Vec<super::checkpoint::MediaIndexShardRef>,
    ) -> anyhow::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let shard = IndexShardV1 {
            format_version: super::media::MEDIA_FORMAT_VERSION,
            kind: kind.to_owned(),
            entries: std::mem::take(entries),
        };
        let bytes = shard.encode()?;
        let hash = sha256(&bytes);
        let plural_kind = if kind == "chunk" {
            "chunks"
        } else {
            "manifests"
        };
        let object_key = format!(
            "indexes/{plural_kind}/{checkpoint_uuid}/{ordinal}-{}.idx",
            super::media::hex_hash(&hash)
        );
        let relative = PathBuf::from(format!("{}.idx", super::media::hex_hash(&hash)));
        super::media::packer::durable_publish(index_root, &relative, &bytes, hash)?;
        shards.push(super::checkpoint::MediaIndexShardRef {
            kind: kind.to_owned(),
            object_key,
            sha256: super::media::hex_hash(&hash),
            byte_length: bytes.len() as u64,
        });
        Ok(())
    }

    let mut shards = Vec::new();
    let mut chunks = Vec::with_capacity(super::media::MAX_INDEX_SHARD_ENTRIES);
    let mut chunk_ordinal = 0usize;
    let mut statement = connection.prepare(
        "SELECT c.chunk_hash,p.object_key,p.pack_hash,p.byte_length,c.pack_offset,c.byte_length FROM media_chunks c JOIN media_packs p ON p.pack_hash=c.pack_hash WHERE c.pack_hash IS NOT NULL ORDER BY c.chunk_hash",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, u64>(3)?,
            row.get::<_, u64>(4)?,
            row.get::<_, u64>(5)?,
        ))
    })? {
        let (hash, object_key, object_hash, object_length, offset, length) = row?;
        chunks.push(IndexShardEntryV1 {
            hash: super::media::hex_hash(hash.as_slice().try_into()?),
            object_key,
            object_hash: super::media::hex_hash(object_hash.as_slice().try_into()?),
            object_length,
            offset,
            length,
        });
        if chunks.len() == super::media::MAX_INDEX_SHARD_ENTRIES {
            persist_shard(
                index_root,
                checkpoint_uuid,
                "chunk",
                chunk_ordinal,
                &mut chunks,
                &mut shards,
            )?;
            chunk_ordinal += 1;
            chunks = Vec::with_capacity(super::media::MAX_INDEX_SHARD_ENTRIES);
        }
    }
    persist_shard(
        index_root,
        checkpoint_uuid,
        "chunk",
        chunk_ordinal,
        &mut chunks,
        &mut shards,
    )?;

    let mut manifests = Vec::with_capacity(super::media::MAX_INDEX_SHARD_ENTRIES);
    let mut manifest_ordinal = 0usize;
    let mut statement = connection.prepare(
        "SELECT m.manifest_hash,p.object_key,p.pack_hash,p.byte_length,m.pack_offset,m.record_length FROM media_manifests m JOIN media_packs p ON p.pack_hash=m.manifest_pack_hash WHERE m.manifest_pack_hash IS NOT NULL ORDER BY m.manifest_hash",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, u64>(3)?,
            row.get::<_, u64>(4)?,
            row.get::<_, u64>(5)?,
        ))
    })? {
        let (hash, object_key, object_hash, object_length, offset, length) = row?;
        manifests.push(IndexShardEntryV1 {
            hash: super::media::hex_hash(hash.as_slice().try_into()?),
            object_key,
            object_hash: super::media::hex_hash(object_hash.as_slice().try_into()?),
            object_length,
            offset,
            length,
        });
        if manifests.len() == super::media::MAX_INDEX_SHARD_ENTRIES {
            persist_shard(
                index_root,
                checkpoint_uuid,
                "manifest",
                manifest_ordinal,
                &mut manifests,
                &mut shards,
            )?;
            manifest_ordinal += 1;
            manifests = Vec::with_capacity(super::media::MAX_INDEX_SHARD_ENTRIES);
        }
    }
    persist_shard(
        index_root,
        checkpoint_uuid,
        "manifest",
        manifest_ordinal,
        &mut manifests,
        &mut shards,
    )?;
    Ok(shards)
}

fn ingest_media_index_shards(
    connection: &Connection,
    shards: &[super::checkpoint::MediaIndexShardRef],
    index_root: &Path,
) -> anyhow::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    let now = now_ms();
    for reference in shards {
        let expected_hash = super::media::parse_hash(&reference.sha256)?;
        let path = index_root.join(format!("{}.idx", reference.sha256));
        let bytes = fs::read(&path)?;
        if bytes.len() as u64 != reference.byte_length || sha256(&bytes) != expected_hash {
            anyhow::bail!("checkpoint media index shard identity mismatch");
        }
        let shard = super::media::format::IndexShardV1::decode(&bytes)?;
        if shard.kind != reference.kind {
            anyhow::bail!("checkpoint media index shard kind mismatch");
        }
        for entry in shard.entries {
            let object_hash = super::media::parse_hash(&entry.object_hash)?;
            let identity_hash = super::media::parse_hash(&entry.hash)?;
            let object_kind = if shard.kind == "chunk" {
                "data-pack"
            } else {
                "manifest-pack"
            };
            transaction.execute(
                "INSERT INTO media_remote_objects(object_hash,object_kind,object_key,byte_length,receipt_generation,state,updated_at) VALUES (?1,?2,?3,?4,0,'remote',?5) ON CONFLICT(object_hash) DO UPDATE SET object_key=excluded.object_key,byte_length=excluded.byte_length,state='remote',updated_at=excluded.updated_at",
                params![object_hash.as_slice(), object_kind, entry.object_key, entry.object_length, now],
            )?;
            transaction.execute(
                "INSERT INTO media_packs(pack_hash,object_key,byte_length,state,created_at,updated_at) VALUES (?1,?2,?3,'remote',?4,?4) ON CONFLICT(pack_hash) DO UPDATE SET object_key=excluded.object_key,byte_length=excluded.byte_length,local_relpath=NULL,state='remote',updated_at=excluded.updated_at",
                params![object_hash.as_slice(), entry.object_key, entry.object_length, now],
            )?;
            if shard.kind == "chunk" {
                transaction.execute(
                    "INSERT INTO media_chunks(chunk_hash,byte_length,pack_hash,pack_offset,state,created_at,updated_at) VALUES (?1,?2,?3,?4,'remote',?5,?5) ON CONFLICT(chunk_hash) DO UPDATE SET pack_hash=excluded.pack_hash,pack_offset=excluded.pack_offset,state='remote',updated_at=excluded.updated_at WHERE media_chunks.byte_length=excluded.byte_length",
                    params![identity_hash.as_slice(), entry.length, object_hash.as_slice(), entry.offset, now],
                )?;
            } else {
                transaction.execute(
                    "UPDATE media_manifests SET manifest_pack_hash=?1,pack_offset=?2,record_length=?3,state='remote',updated_at=?4 WHERE manifest_hash=?5",
                    params![object_hash.as_slice(), entry.offset, entry.length, now, identity_hash.as_slice()],
                )?;
            }
        }
    }
    transaction.commit()?;
    Ok(())
}

fn bootstrap_projection_events(
    library: LibraryUuid,
    projection: &CatalogProjectionSnapshot,
    existing: &ProtocolState,
    events: &[EventEnvelope],
) -> anyhow::Result<Vec<EventEnvelope>> {
    let actor = ActorId {
        device_uuid: DeviceUuid(deterministic_uuid(
            library.0.as_bytes(),
            b"bootstrap-device",
        )),
        stream_uuid: StreamUuid(deterministic_uuid(
            library.0.as_bytes(),
            b"bootstrap-stream",
        )),
        actor_epoch: ActorEpochUuid(deterministic_uuid(library.0.as_bytes(), b"bootstrap-epoch")),
    };
    let binding = BindingEpochUuid(deterministic_uuid(
        library.0.as_bytes(),
        b"bootstrap-binding",
    ));
    let mut seq = events
        .iter()
        .filter(|event| event.actor == actor)
        .map(|event| event.actor_seq)
        .max()
        .unwrap_or(0);
    let mut output = Vec::new();
    let root_folders = projection
        .roots
        .iter()
        .map(|root| root.root_folder_uuid)
        .collect::<std::collections::BTreeSet<_>>();
    let mut push = |entity: EntityUuid, operation: Operation| -> anyhow::Result<()> {
        seq += 1;
        let event_uuid = EventUuid(deterministic_uuid(
            entity.0.as_bytes(),
            format!("bootstrap-{seq}").as_bytes(),
        ));
        output.push(seal_event(EventEnvelope {
            wire_version: super::codec::WIRE_VERSION,
            envelope_version: super::codec::ENVELOPE_VERSION,
            reducer_version: super::codec::REDUCER_VERSION,
            library_uuid: library,
            event_uuid,
            entity_uuid: entity,
            stream_uuid: actor.stream_uuid,
            stream_kind: StreamKind::Catalog,
            actor: actor.clone(),
            actor_seq: seq,
            binding_epoch: binding,
            hlc: Hlc {
                wall_ms: 0,
                logical: seq as u32,
            },
            operation,
            base_events: Default::default(),
            key_version: 0,
            integrity_hash: [0; 32],
        })?);
        Ok(())
    };
    for folder in &projection.folders {
        if root_folders.contains(&folder.uuid) || existing.entities.contains_key(&folder.uuid) {
            continue;
        }
        push(
            folder.uuid,
            Operation::SetField {
                field: "entity-kind".into(),
                value: serde_json::json!("folder"),
            },
        )?;
        push(
            folder.uuid,
            Operation::SetLocation {
                revision_uuid: RevisionUuid(deterministic_uuid(
                    folder.uuid.0.as_bytes(),
                    b"bootstrap-location",
                )),
                parent_uuid: folder.parent_uuid,
                name: folder.name.clone(),
                supersedes: Default::default(),
            },
        )?;
    }
    for file in &projection.files {
        if existing.entities.contains_key(&file.uuid) {
            continue;
        }
        push(
            file.uuid,
            Operation::SetField {
                field: "entity-kind".into(),
                value: serde_json::json!("asset"),
            },
        )?;
        push(
            file.uuid,
            Operation::SetLocation {
                revision_uuid: RevisionUuid(deterministic_uuid(
                    file.uuid.0.as_bytes(),
                    b"bootstrap-location",
                )),
                parent_uuid: Some(file.folder_uuid),
                name: file.name.clone(),
                supersedes: Default::default(),
            },
        )?;
        push(
            file.uuid,
            Operation::SetField {
                field: "extension".into(),
                value: serde_json::json!(file.ext),
            },
        )?;
    }
    for tag in &projection.tags {
        if existing.entities.contains_key(&tag.uuid) {
            continue;
        }
        push(
            tag.uuid,
            Operation::SetField {
                field: "entity-kind".into(),
                value: serde_json::json!("tag"),
            },
        )?;
        push(
            tag.uuid,
            Operation::SetField {
                field: "name".into(),
                value: serde_json::json!(tag.name),
            },
        )?;
        push(
            tag.uuid,
            Operation::SetField {
                field: "color".into(),
                value: serde_json::json!(tag.color),
            },
        )?;
    }
    for item in &projection.metadata {
        if existing.entities.contains_key(&item.asset_uuid) {
            continue;
        }
        if let Some(rating) = item.rating {
            push(
                item.asset_uuid,
                Operation::SetField {
                    field: "rating".into(),
                    value: serde_json::json!(rating),
                },
            )?;
        }
        push(
            item.asset_uuid,
            Operation::SetField {
                field: "favorite".into(),
                value: serde_json::json!(item.favorite),
            },
        )?;
        if let Some(note) = &item.note {
            push(
                item.asset_uuid,
                Operation::SetField {
                    field: "note".into(),
                    value: serde_json::json!(note),
                },
            )?;
        }
    }
    for (asset, tag) in &projection.memberships {
        if existing
            .entities
            .get(asset)
            .is_some_and(|truth| truth.live_set_members("tags").contains(tag))
        {
            continue;
        }
        push(
            *asset,
            Operation::SetAdd {
                set: "tags".into(),
                member_uuid: *tag,
            },
        )?;
    }
    Ok(output)
}
fn deterministic_uuid(namespace: &[u8], label: &[u8]) -> uuid::Uuid {
    let mut bytes = sha256(&[namespace, label].concat());
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes[..16].try_into().expect("sha256 UUID slice"))
}

pub fn load_and_validate_checkpoint(
    connection: &Connection,
    checkpoint_uuid: CheckpointUuid,
) -> anyhow::Result<Checkpoint> {
    let (path,expected): (String,Vec<u8>) = connection.query_row("SELECT spool_path,payload_hash FROM sync_checkpoints WHERE checkpoint_uuid=?1 AND state='ready'", [checkpoint_uuid.0.as_bytes().as_slice()], |r| Ok((r.get(0)?,r.get(1)?)))?;
    let bytes = fs::read(path)?;
    if sha256(&bytes).as_slice() != expected {
        anyhow::bail!("checkpoint payload hash mismatch");
    }
    let checkpoint: Checkpoint = serde_json::from_slice(&bytes)?;
    checkpoint.verify().context("verify checkpoint")?;
    Ok(checkpoint)
}

/// Crash recovery never advertises an unfinished descriptor. A complete temp
/// may be rebuilt from the immutable fence/events by a later coordinator turn.
pub fn recover_interrupted_checkpoints(connection: &Connection) -> anyhow::Result<usize> {
    Ok(connection.execute(
        "UPDATE sync_checkpoints SET state='invalid' WHERE state='building'",
        [],
    )?)
}

pub fn descriptor_ready(
    connection: &Connection,
    checkpoint_uuid: CheckpointUuid,
) -> anyhow::Result<bool> {
    let ready: bool = connection.query_row("SELECT state='ready' AND spool_path IS NOT NULL AND payload_hash IS NOT NULL FROM sync_checkpoints WHERE checkpoint_uuid=?1", [checkpoint_uuid.0.as_bytes().as_slice()], |r| r.get(0)).optional()?.unwrap_or(false);
    if !ready {
        return Ok(false);
    }
    Ok(load_and_validate_checkpoint(connection, checkpoint_uuid)?
        .recovery
        .portable_complete)
}

/// Replays the sealed checkpoint truth through the same typed reducer used by
/// live apply. The caller may then materialize it into a staging Catalog and
/// atomically promote only after this digest check succeeds.
pub fn rebuild_checkpoint_state(checkpoint: &Checkpoint) -> anyhow::Result<ProtocolState> {
    checkpoint.verify()?;
    Ok(checkpoint.state.clone())
}

/// New-device bootstrap builds an isolated Catalog and replays the checkpoint
/// event stream through the production inbox reducer. Promotion refuses to
/// overwrite an existing Catalog; active-library replacement remains owned by
/// LibraryManager fencing.
pub fn materialize_staging_catalog(
    checkpoint: &Checkpoint,
    staging_path: &Path,
    root_uuid: uuid::Uuid,
    device_uuid: uuid::Uuid,
    managed_root: &Path,
) -> anyhow::Result<()> {
    if checkpoint.projection.roots.len() > 1 {
        anyhow::bail!("multi-root checkpoint restore requires explicit root bindings");
    }
    let selected_root = checkpoint
        .projection
        .roots
        .first()
        .map(|root| root.root_uuid.0)
        .unwrap_or(root_uuid);
    let library_root_uuid = checkpoint
        .state
        .roots
        .get(&EntityUuid(selected_root))
        .map(|root| root.library_root_uuid.0)
        .unwrap_or(root_uuid);
    materialize_staging_catalog_internal(
        checkpoint,
        staging_path,
        library_root_uuid,
        device_uuid,
        &[(selected_root, managed_root.to_path_buf())],
        false,
    )
}

/// Restores portable truth for every synchronized root while materializing
/// browse rows only for the caller-selected device-local bindings.
pub fn materialize_staging_catalog_with_roots(
    checkpoint: &Checkpoint,
    staging_path: &Path,
    library_root_uuid: uuid::Uuid,
    device_uuid: uuid::Uuid,
    selected_roots: &[(uuid::Uuid, PathBuf)],
) -> anyhow::Result<()> {
    materialize_staging_catalog_internal(
        checkpoint,
        staging_path,
        library_root_uuid,
        device_uuid,
        selected_roots,
        true,
    )
}

fn materialize_staging_catalog_internal(
    checkpoint: &Checkpoint,
    staging_path: &Path,
    library_root_uuid: uuid::Uuid,
    device_uuid: uuid::Uuid,
    selected_roots: &[(uuid::Uuid, PathBuf)],
    link_managed_roots: bool,
) -> anyhow::Result<()> {
    checkpoint.verify()?;
    if !checkpoint.recovery.portable_complete {
        anyhow::bail!("checkpoint contains unfinished device-local work");
    }
    if staging_path.exists() {
        std::fs::remove_file(staging_path)?;
    }
    let database = crate::db::Database::open(staging_path).context("open staging database")?;
    database
        .apply_migrations()
        .context("apply staging migrations")?;
    // Replay canonical history before installing its final browse projection.
    // Historical locations may temporarily use a name occupied by another row
    // in the final snapshot, even though the final legalized names are unique.
    database.connection_for_fsops().execute("INSERT INTO sync_local_state(singleton,library_uuid,device_uuid,catalog_stream_uuid,next_actor_seq,next_local_mutation_generation,hlc_wall_ms,hlc_logical,capture_enabled,compatibility_state,updated_at) VALUES (1,?1,?2,?3,1,2,0,0,0,'compatible',?4)",params![checkpoint.library_uuid.0.as_bytes().as_slice(),device_uuid.as_bytes().as_slice(),uuid::Uuid::now_v7().as_bytes().as_slice(),now_ms()])?;
    let mut by_actor = checkpoint.events.clone();
    by_actor.sort_by(|a, b| a.actor.cmp(&b.actor).then(a.actor_seq.cmp(&b.actor_seq)));
    let replay = database.connection_for_fsops().unchecked_transaction()?;
    // A verified checkpoint contains all actors' history. An observed revision
    // can belong to a later-sorted actor; enforce those references at the end
    // of this isolated replay rather than imposing a new actor ordering rule.
    replay.execute_batch("PRAGMA defer_foreign_keys=ON;")?;
    for event in by_actor {
        super::inbox::apply_remote_batch(&replay, std::slice::from_ref(&event))
            .context("apply restored event")?;
    }
    replay
        .commit()
        .context("commit restored event references")?;
    // Replay can build transient descriptor revisions. This new staging
    // Catalog has no bindings or browse roots yet; replace only that derived
    // projection with the exact checkpoint truth after checking its identity.
    for restored in database.sync_root_descriptors()? {
        let expected = checkpoint
            .state
            .roots
            .get(&EntityUuid(uuid::Uuid::from_bytes(restored.root_uuid)))
            .context("restored root is absent from checkpoint truth")?;
        if restored.root_folder_uuid != *expected.root_folder_uuid.0.as_bytes()
            || restored.library_root_uuid != *expected.library_root_uuid.0.as_bytes()
        {
            anyhow::bail!("restored root identity differs from checkpoint truth");
        }
    }
    database
        .connection_for_fsops()
        .execute("DELETE FROM sync_root_descriptors", [])?;
    for (root_uuid, path) in selected_roots {
        let descriptor = checkpoint
            .state
            .roots
            .get(&EntityUuid(*root_uuid))
            .ok_or_else(|| anyhow::anyhow!("selected root is absent from checkpoint truth"))?;
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: path.to_string_lossy().into_owned(),
                display_name: descriptor.materialized_name.clone(),
            })
            .context("add selected root")?;
        database.connection_for_fsops().execute(
            "UPDATE roots SET root_uuid=?1 WHERE id=?2",
            params![root_uuid.as_bytes().as_slice(), root_id],
        )?;
        database.connection_for_fsops().execute(
            "INSERT INTO folders(root_id,parent_id,name,path_hash,mtime,status,folder_uuid) VALUES (?1,NULL,'','',NULL,'active',?2)",
            params![root_id, descriptor.root_folder_uuid.0.as_bytes().as_slice()],
        )?;
        let record = crate::db::SyncRootDescriptorRecord {
            root_uuid: *root_uuid.as_bytes(),
            root_folder_uuid: *descriptor.root_folder_uuid.0.as_bytes(),
            library_root_uuid: *descriptor.library_root_uuid.0.as_bytes(),
            creation_event_uuid: descriptor
                .creation_event_uuid
                .map(|value| *value.0.as_bytes()),
            winning_location_revision_uuid: descriptor
                .winning_location_revision_uuid
                .map(|value| *value.0.as_bytes()),
            requested_name: descriptor.requested_name.clone(),
            materialized_name: descriptor.materialized_name.clone(),
            materialization_fingerprint: descriptor.materialization_fingerprint,
            lifecycle: descriptor.lifecycle.clone(),
            logical_revision: descriptor.logical_revision,
            updated_at: now_ms(),
        };
        database.upsert_sync_root_descriptor(&record)?;
        if link_managed_roots {
            database.link_sync_root_descriptor(
                root_id,
                root_uuid.as_bytes(),
                descriptor.root_folder_uuid.0.as_bytes(),
            )?;
        }
    }
    for descriptor in checkpoint.state.roots.values() {
        if database
            .sync_root_descriptor(descriptor.root_uuid.0.as_bytes())?
            .is_none()
        {
            database.upsert_sync_root_descriptor(&crate::db::SyncRootDescriptorRecord {
                root_uuid: *descriptor.root_uuid.0.as_bytes(),
                root_folder_uuid: *descriptor.root_folder_uuid.0.as_bytes(),
                library_root_uuid: *descriptor.library_root_uuid.0.as_bytes(),
                creation_event_uuid: descriptor
                    .creation_event_uuid
                    .map(|value| *value.0.as_bytes()),
                winning_location_revision_uuid: descriptor
                    .winning_location_revision_uuid
                    .map(|value| *value.0.as_bytes()),
                requested_name: descriptor.requested_name.clone(),
                materialized_name: descriptor.materialized_name.clone(),
                materialization_fingerprint: descriptor.materialization_fingerprint,
                lifecycle: descriptor.lifecycle.clone(),
                logical_revision: descriptor.logical_revision,
                updated_at: now_ms(),
            })?;
        }
    }
    if !selected_roots.is_empty() || !checkpoint.state.roots.is_empty() {
        database
            .initialize_sync_identity(checkpoint.library_uuid.0, library_root_uuid, device_uuid)
            .context("initialize restored sync identity")?;
    }
    restore_projection(database.connection_for_fsops(), checkpoint, selected_roots)
        .context("restore browse projection")?;
    restore_recovery_snapshot(database.connection_for_fsops(), &checkpoint.recovery)
        .context("restore recovery snapshot")?;
    ingest_media_index_shards(
        database.connection_for_fsops(),
        &checkpoint.media_index_shards,
        &staging_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("sync-spool")
            .join("indexes"),
    )
    .context("restore media index shards")?;
    restore_catalog_frontiers(database.connection_for_fsops(), &checkpoint.catalog_vector)
        .context("restore Catalog frontiers")?;
    validate_genesis_replay(database.connection_for_fsops(), checkpoint, i64::MAX)
        .context("validate restored genesis replay")?;
    validate_browse_projection(database.connection_for_fsops(), checkpoint, selected_roots)
        .context("validate restored browse projection")?;
    database
        .connection_for_fsops()
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

fn bind_snapshot_value(value: &SqlValueSnapshot) -> rusqlite::types::Value {
    match value {
        SqlValueSnapshot::Null => rusqlite::types::Value::Null,
        SqlValueSnapshot::Integer(value) => rusqlite::types::Value::Integer(*value),
        SqlValueSnapshot::Real(bits) => {
            rusqlite::types::Value::Real(f64::from_bits(bits.parse().unwrap_or(0)))
        }
        SqlValueSnapshot::Text(value) => rusqlite::types::Value::Text(value.clone()),
        SqlValueSnapshot::Blob(value) => rusqlite::types::Value::Blob(value.clone()),
    }
}

fn restore_recovery_snapshot(
    connection: &Connection,
    recovery: &CatalogRecoverySnapshot,
) -> anyhow::Result<()> {
    if recovery.tables.len() != RECOVERY_TABLES.len() {
        anyhow::bail!("checkpoint recovery table set is incomplete");
    }
    let mut seen_tables = std::collections::BTreeSet::new();
    let transaction = connection.unchecked_transaction()?;
    for table in &recovery.tables {
        if !RECOVERY_TABLES.contains(&table.table.as_str()) {
            anyhow::bail!(
                "checkpoint contains unsupported recovery table: {}",
                table.table
            );
        }
        if !seen_tables.insert(table.table.as_str()) {
            anyhow::bail!(
                "checkpoint contains duplicate recovery table: {}",
                table.table
            );
        }
        let expected_columns = if table.table == "sync_publication_intents" {
            SYNC_PUBLICATION_INTENT_RECOVERY_COLUMNS
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>()
        } else {
            let schema_sql = format!("SELECT * FROM {} LIMIT 0", table.table);
            let schema_statement = transaction.prepare(&schema_sql)?;
            schema_statement
                .column_names()
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>()
        };
        if table.columns != expected_columns {
            anyhow::bail!(
                "checkpoint recovery columns do not match schema for {}",
                table.table
            );
        }
        if table.rows.is_empty() {
            continue;
        }
        let placeholders = (1..=table.columns.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        // Immutable content rows may already have been reconstructed by the
        // checkpoint event replay. REPLACE would delete the referenced row
        // before reinserting it and trip the event foreign key. Recovery only
        // needs to add rows that replay did not materialize.
        let insert_mode = if matches!(
            table.table.as_str(),
            "asset_content_revisions" | "asset_content_supersedes"
        ) {
            "INSERT OR IGNORE"
        } else {
            "INSERT OR REPLACE"
        };
        let sql = format!(
            "{insert_mode} INTO {} ({}) VALUES ({})",
            table.table,
            table.columns.join(","),
            placeholders
        );
        let mut statement = transaction.prepare(&sql)?;
        for (row_index, row) in table.rows.iter().enumerate() {
            if row.len() != table.columns.len() {
                anyhow::bail!("checkpoint recovery row width mismatch");
            }
            let values = row.iter().map(bind_snapshot_value).collect::<Vec<_>>();
            statement
                .execute(rusqlite::params_from_iter(values))
                .with_context(|| {
                    format!(
                        "restore checkpoint recovery row {row_index} for {}",
                        table.table
                    )
                })?;
        }
    }
    transaction.commit()?;
    Ok(())
}

fn restore_catalog_frontiers(connection: &Connection, vector: &StreamVector) -> anyhow::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    for (actor, seq) in vector {
        super::inbox::upsert_frontier_state(&transaction, actor, *seq as i64, "ready")?;
    }
    transaction.commit()?;
    Ok(())
}

fn capture_projection(connection: &Connection) -> anyhow::Result<CatalogProjectionSnapshot> {
    let roots = connection.prepare("SELECT r.root_uuid,COALESCE(r.root_folder_uuid,s.folder_uuid),r.display_name FROM roots r LEFT JOIN folders s ON s.root_id=r.id AND s.parent_id IS NULL AND s.name='' ORDER BY r.root_uuid")?
        .query_map([], |r| Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,Option<Vec<u8>>>(1)?,r.get::<_,String>(2)?)))?
        .map(|row| {
            let (root, folder, display_name) = row?;
            let root_uuid = uuid::Uuid::from_slice(&root).map_err(sql_uuid)?;
            let root_folder_uuid = folder
                .map(|value| uuid::Uuid::from_slice(&value).map_err(sql_uuid))
                .transpose()?
                .unwrap_or_else(|| deterministic_uuid(root_uuid.as_bytes(), b"root-folder"));
            Ok(RootProjectionSnapshot { root_uuid: super::types::EntityUuid(root_uuid), root_folder_uuid: super::types::EntityUuid(root_folder_uuid), display_name })
        }).collect::<Result<Vec<_>,rusqlite::Error>>()?;
    let mut folders = connection.prepare("SELECT f.folder_uuid,r.root_uuid,p.folder_uuid,f.name,f.mtime,f.status FROM folders f JOIN roots r ON r.id=f.root_id LEFT JOIN folders p ON p.id=f.parent_id ORDER BY f.folder_uuid")?
        .query_map([], |r| Ok(FolderSnapshot { uuid: super::types::EntityUuid(uuid::Uuid::from_slice(&r.get::<_,Vec<u8>>(0)?).map_err(sql_uuid)?), root_uuid: super::types::EntityUuid(uuid::Uuid::from_slice(&r.get::<_,Vec<u8>>(1)?).map_err(sql_uuid)?), parent_uuid: r.get::<_,Option<Vec<u8>>>(2)?.map(|v| uuid::Uuid::from_slice(&v).map(super::types::EntityUuid).map_err(sql_uuid)).transpose()?, name:r.get(3)?,mtime:r.get(4)?,status:r.get(5)? }))?.collect::<Result<Vec<_>,_>>()?;
    for root in &roots {
        if !folders
            .iter()
            .any(|folder| folder.uuid == root.root_folder_uuid)
        {
            folders.push(FolderSnapshot {
                uuid: root.root_folder_uuid,
                root_uuid: root.root_uuid,
                parent_uuid: None,
                name: String::new(),
                mtime: None,
                status: "active".to_owned(),
            });
        }
    }
    folders.sort_by_key(|folder| folder.uuid);
    let files = connection.prepare("SELECT f.asset_uuid,r.root_uuid,d.folder_uuid,f.name,f.ext,f.size,f.mtime,f.ctime,f.status,m.kind FROM files f JOIN roots r ON r.id=f.root_id JOIN folders d ON d.id=f.folder_id LEFT JOIN media m ON m.file_id=f.id ORDER BY f.asset_uuid")?
        .query_map([], |r| Ok(FileSnapshot { uuid: super::types::EntityUuid(uuid::Uuid::from_slice(&r.get::<_,Vec<u8>>(0)?).map_err(sql_uuid)?), root_uuid: super::types::EntityUuid(uuid::Uuid::from_slice(&r.get::<_,Vec<u8>>(1)?).map_err(sql_uuid)?), folder_uuid:super::types::EntityUuid(uuid::Uuid::from_slice(&r.get::<_,Vec<u8>>(2)?).map_err(sql_uuid)?),name:r.get(3)?,ext:r.get(4)?,size:r.get(5)?,mtime:r.get(6)?,ctime:r.get(7)?,status:r.get(8)?,media_kind:r.get(9)? }))?.collect::<Result<Vec<_>,_>>()?;
    let tags = connection
        .prepare("SELECT tag_uuid,name,color FROM tags ORDER BY tag_uuid")?
        .query_map([], |r| {
            Ok(TagSnapshot {
                uuid: super::types::EntityUuid(
                    uuid::Uuid::from_slice(&r.get::<_, Vec<u8>>(0)?).map_err(sql_uuid)?,
                ),
                name: r.get(1)?,
                color: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let metadata = connection.prepare("SELECT f.asset_uuid,u.rating,u.favorite,u.note,u.updated_at FROM user_metadata u JOIN files f ON f.id=u.file_id ORDER BY f.asset_uuid")?.query_map([], |r| Ok(MetadataSnapshot { asset_uuid:super::types::EntityUuid(uuid::Uuid::from_slice(&r.get::<_,Vec<u8>>(0)?).map_err(sql_uuid)?),rating:r.get(1)?,favorite:r.get::<_,i64>(2)?!=0,note:r.get(3)?,updated_at:r.get(4)? }))?.collect::<Result<Vec<_>,_>>()?;
    let memberships = connection.prepare("SELECT f.asset_uuid,t.tag_uuid FROM file_tags x JOIN files f ON f.id=x.file_id JOIN tags t ON t.id=x.tag_id ORDER BY f.asset_uuid,t.tag_uuid")?.query_map([], |r| Ok((super::types::EntityUuid(uuid::Uuid::from_slice(&r.get::<_,Vec<u8>>(0)?).map_err(sql_uuid)?),super::types::EntityUuid(uuid::Uuid::from_slice(&r.get::<_,Vec<u8>>(1)?).map_err(sql_uuid)?))))?.collect::<Result<Vec<_>,_>>()?;
    Ok(CatalogProjectionSnapshot {
        roots,
        folders,
        files,
        tags,
        metadata,
        memberships,
    })
}

fn capture_root_truth(connection: &Connection, state: &mut ProtocolState) -> anyhow::Result<()> {
    let mut statement = connection.prepare("SELECT root_uuid,root_folder_uuid,library_root_uuid,creation_event_uuid,winning_location_revision_uuid,requested_name,materialized_name,materialization_fingerprint,lifecycle,logical_revision FROM sync_root_descriptors ORDER BY root_uuid")?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, Option<Vec<u8>>>(3)?,
            row.get::<_, Option<Vec<u8>>>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, Vec<u8>>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, i64>(9)?,
        ))
    })? {
        let (
            root,
            folder,
            library_root,
            creation,
            winning,
            requested,
            materialized,
            fingerprint,
            lifecycle,
            revision,
        ) = row?;
        let root_uuid = EntityUuid(uuid::Uuid::from_slice(&root)?);
        state.roots.insert(
            root_uuid,
            super::reducer::SyncRootTruth {
                root_uuid,
                root_folder_uuid: EntityUuid(uuid::Uuid::from_slice(&folder)?),
                library_root_uuid: EntityUuid(uuid::Uuid::from_slice(&library_root)?),
                creation_event_uuid: creation
                    .map(|value| uuid::Uuid::from_slice(&value).map(EventUuid))
                    .transpose()?,
                winning_location_revision_uuid: winning
                    .map(|value| uuid::Uuid::from_slice(&value).map(RevisionUuid))
                    .transpose()?,
                requested_name: requested,
                materialized_name: materialized,
                materialization_fingerprint: fingerprint
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("invalid root materialization fingerprint"))?,
                lifecycle,
                logical_revision: u64::try_from(revision)?,
            },
        );
    }
    let mut legacy = connection.prepare("SELECT r.root_uuid,COALESCE(r.root_folder_uuid,s.folder_uuid),r.display_name FROM roots r LEFT JOIN folders s ON s.root_id=r.id AND s.parent_id IS NULL AND s.name='' WHERE NOT EXISTS (SELECT 1 FROM sync_root_descriptors d WHERE d.root_uuid=r.root_uuid) ORDER BY r.root_uuid")?;
    for row in legacy.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Option<Vec<u8>>>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (root, folder, requested_name) = row?;
        let root_uuid = EntityUuid(uuid::Uuid::from_slice(&root)?);
        // Before managed-root v2 the one Catalog root UUID doubled as the
        // virtual Library-root parent used by bootstrap location events.
        let library_root_uuid = root_uuid;
        let root_folder_uuid = EntityUuid(
            folder
                .map(|value| uuid::Uuid::from_slice(&value))
                .transpose()?
                .unwrap_or_else(|| deterministic_uuid(root_uuid.0.as_bytes(), b"root-folder")),
        );
        let materialized_name =
            super::reducer::canonicalize_materialized_component(&requested_name, None);
        state.roots.insert(
            root_uuid,
            super::reducer::SyncRootTruth {
                root_uuid,
                root_folder_uuid,
                library_root_uuid,
                creation_event_uuid: None,
                winning_location_revision_uuid: None,
                requested_name,
                materialized_name: materialized_name.clone(),
                materialization_fingerprint: sha256(&canonical_bytes(&(
                    super::reducer::MATERIALIZED_NAME_VERSION,
                    root_uuid,
                    root_folder_uuid,
                    materialized_name,
                ))?),
                lifecycle: "active".to_owned(),
                logical_revision: 0,
            },
        );
    }
    Ok(())
}

fn sql_uuid(error: uuid::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Blob, Box::new(error))
}

fn restore_projection(
    connection: &Connection,
    checkpoint: &Checkpoint,
    selected_roots: &[(uuid::Uuid, PathBuf)],
) -> anyhow::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    let selected = selected_roots
        .iter()
        .map(|(root_uuid, _)| *root_uuid)
        .collect::<std::collections::BTreeSet<_>>();
    let root_folders = checkpoint
        .projection
        .roots
        .iter()
        .filter(|root| selected.contains(&root.root_uuid.0))
        .map(|root| root.root_folder_uuid)
        .collect::<std::collections::BTreeSet<_>>();
    let mut pending = checkpoint
        .projection
        .folders
        .iter()
        .filter(|folder| selected.contains(&folder.root_uuid.0))
        .cloned()
        .collect::<Vec<_>>();
    while !pending.is_empty() {
        let before = pending.len();
        let mut rest = Vec::new();
        for folder in pending {
            let root_id: i64 = transaction
                .query_row(
                    "SELECT id FROM roots WHERE root_uuid=?1",
                    [folder.root_uuid.0.as_bytes().as_slice()],
                    |row| row.get(0),
                )
                .with_context(|| format!("resolve restored folder root {}", folder.root_uuid.0))?;
            if root_folders.contains(&folder.uuid) {
                transaction.execute(
                    "UPDATE folders SET mtime=?1,status=?2 WHERE root_id=?3 AND parent_id IS NULL AND name='' AND folder_uuid=?4",
                    params![folder.mtime, folder.status, root_id, folder.uuid.0.as_bytes().as_slice()],
                )?;
                continue;
            }
            let parent_id: Option<i64> = folder.parent_uuid.and_then(|u| {
                transaction
                    .query_row(
                        "SELECT id FROM folders WHERE folder_uuid=?1",
                        [u.0.as_bytes().as_slice()],
                        |r| r.get(0),
                    )
                    .optional()
                    .ok()
                    .flatten()
            });
            if folder.parent_uuid.is_some() && parent_id.is_none() {
                rest.push(folder);
                continue;
            }
            transaction.execute("INSERT INTO folders(root_id,parent_id,name,path_hash,mtime,status,folder_uuid) VALUES (?1,?2,?3,'',?4,?5,?6)",params![root_id,parent_id,folder.name,folder.mtime,folder.status,folder.uuid.0.as_bytes().as_slice()])?;
        }
        if rest.len() == before {
            anyhow::bail!("checkpoint folder graph cannot be materialized");
        }
        pending = rest;
    }
    for file in checkpoint
        .projection
        .files
        .iter()
        .filter(|file| selected.contains(&file.root_uuid.0))
    {
        let root_id: i64 = transaction
            .query_row(
                "SELECT id FROM roots WHERE root_uuid=?1",
                [file.root_uuid.0.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .with_context(|| format!("resolve restored file root {}", file.root_uuid.0))?;
        let folder_id: i64 = transaction
            .query_row(
                "SELECT id FROM folders WHERE folder_uuid=?1",
                [file.folder_uuid.0.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .with_context(|| {
                format!(
                    "resolve restored file folder {} for asset {}",
                    file.folder_uuid.0, file.uuid.0
                )
            })?;
        transaction.execute("INSERT INTO files(root_id,folder_id,name,ext,size,mtime,ctime,status,asset_uuid) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![root_id,folder_id,file.name,file.ext,file.size,file.mtime,file.ctime,file.status,file.uuid.0.as_bytes().as_slice()])?;
        if let Some(kind) = &file.media_kind {
            transaction.execute("INSERT INTO media(file_id,kind,metadata_status) SELECT id,?1,'pending' FROM files WHERE asset_uuid=?2",params![kind,file.uuid.0.as_bytes().as_slice()])?;
        }
    }
    for tag in &checkpoint.projection.tags {
        let existing: Option<(String, Option<String>)> = transaction
            .query_row(
                "SELECT name,color FROM tags WHERE tag_uuid=?1",
                [tag.uuid.0.as_bytes().as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((name, color)) = existing {
            if name != tag.name || color != tag.color {
                anyhow::bail!("checkpoint tag projection differs from replayed truth");
            }
            continue;
        }
        transaction.execute(
            "INSERT INTO tags(name,color,tag_uuid) VALUES (?1,?2,?3)",
            params![tag.name, tag.color, tag.uuid.0.as_bytes().as_slice()],
        )?;
    }
    for item in &checkpoint.projection.metadata {
        transaction.execute("INSERT INTO user_metadata(file_id,rating,favorite,note,updated_at) SELECT id,?1,?2,?3,?4 FROM files WHERE asset_uuid=?5",params![item.rating,item.favorite as i64,item.note,item.updated_at,item.asset_uuid.0.as_bytes().as_slice()])?;
    }
    for (asset, tag) in &checkpoint.projection.memberships {
        transaction.execute("INSERT INTO file_tags(file_id,tag_id) SELECT f.id,t.id FROM files f,tags t WHERE f.asset_uuid=?1 AND t.tag_uuid=?2",params![asset.0.as_bytes().as_slice(),tag.0.as_bytes().as_slice()])?;
    }
    for file in checkpoint
        .projection
        .files
        .iter()
        .filter(|file| selected.contains(&file.root_uuid.0))
    {
        let file_id = transaction.query_row(
            "SELECT id FROM files WHERE asset_uuid=?1",
            [file.uuid.0.as_bytes().as_slice()],
            |row| row.get(0),
        )?;
        crate::db::sync_media_fts_for_file_in_transaction_pub(&transaction, file_id)?;
    }
    transaction.commit()?;
    Ok(())
}

fn validate_browse_projection(
    connection: &Connection,
    checkpoint: &Checkpoint,
    selected_roots: &[(uuid::Uuid, PathBuf)],
) -> anyhow::Result<()> {
    let mut actual = capture_projection(connection)?;
    let selected = selected_roots
        .iter()
        .map(|(root_uuid, _)| *root_uuid)
        .collect::<std::collections::BTreeSet<_>>();
    let mut expected = CatalogProjectionSnapshot {
        roots: checkpoint
            .projection
            .roots
            .iter()
            .filter(|root| selected.contains(&root.root_uuid.0))
            .cloned()
            .collect(),
        folders: checkpoint
            .projection
            .folders
            .iter()
            .filter(|folder| selected.contains(&folder.root_uuid.0))
            .cloned()
            .collect(),
        files: checkpoint
            .projection
            .files
            .iter()
            .filter(|file| selected.contains(&file.root_uuid.0))
            .cloned()
            .collect(),
        tags: checkpoint.projection.tags.clone(),
        metadata: checkpoint
            .projection
            .metadata
            .iter()
            .filter(|metadata| {
                actual
                    .files
                    .iter()
                    .any(|file| file.uuid == metadata.asset_uuid)
            })
            .cloned()
            .collect(),
        memberships: checkpoint
            .projection
            .memberships
            .iter()
            .filter(|(asset, _)| actual.files.iter().any(|file| file.uuid == *asset))
            .copied()
            .collect(),
    };
    // Applying a field stamps the receiving clock; it is not portable truth.
    for metadata in actual
        .metadata
        .iter_mut()
        .chain(expected.metadata.iter_mut())
    {
        metadata.updated_at = 0;
    }
    if actual != expected {
        anyhow::bail!(
            "staging browse projection differs from checkpoint projection: expected={:?} actual={:?}",
            expected,
            actual
        );
    }
    Ok(())
}

pub fn promote_new_staging_catalog(staging_path: &Path, target_path: &Path) -> anyhow::Result<()> {
    if target_path.exists() {
        anyhow::bail!("refusing to replace an existing Catalog during bootstrap promotion");
    }
    super::fault::hit("checkpoint.before-staging-promotion");
    std::fs::rename(staging_path, target_path)?;
    if let Some(parent) = target_path.parent() {
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(())
}

pub fn validate_genesis_replay(
    connection: &Connection,
    checkpoint: &Checkpoint,
    max_rowid: i64,
) -> anyhow::Result<()> {
    let mut state = ProtocolState::default();
    let mut cursor = 0_i64;
    loop {
        let mut statement=connection.prepare("SELECT rowid,payload FROM sync_events WHERE rowid>?1 AND rowid<=?2 ORDER BY rowid LIMIT 100")?;
        let rows: Vec<(i64, Vec<u8>)> = statement
            .query_map(params![cursor, max_rowid], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        if rows.is_empty() {
            break;
        }
        for (rowid, payload) in rows {
            state.apply(&decode_event(&payload)?);
            cursor = rowid;
        }
    }
    state.roots = checkpoint.state.roots.clone();
    if super::checkpoint::state_digest(&state)?
        != super::checkpoint::state_digest(&checkpoint.state)?
    {
        anyhow::bail!("checkpoint replay digest differs from genesis replay");
    }
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, NewRoot, SyncRootDescriptorRecord, SyncRootLocalBindingRecord};

    fn database() -> Database {
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        database
            .add_root(NewRoot {
                path: "D:/Managed".into(),
                display_name: "Managed".into(),
            })
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        database
    }

    #[test]
    fn checkpoint_fence_build_is_durable_and_replay_validates() {
        let database = database();
        database.enable_fake_sync_capture_for_test().unwrap();
        database
            .create_tag("checkpointed", Some("#112233"))
            .unwrap();
        let connection = database.connection_for_fsops();
        let library: Vec<u8> = connection
            .query_row(
                "SELECT library_uuid FROM sync_local_state WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let fence = capture_checkpoint_fence(
            connection,
            StreamVector::new(),
            test_host(uuid::Uuid::from_slice(&library).unwrap()),
        )
        .unwrap();
        let directory =
            std::env::temp_dir().join(format!("mangoo-checkpoint-{}", uuid::Uuid::now_v7()));
        let path = build_checkpoint(connection, &fence, &directory).unwrap();
        assert!(path.exists());
        let restored = load_and_validate_checkpoint(connection, fence.checkpoint_uuid).unwrap();
        assert_eq!(restored.catalog_vector, fence.catalog_vector);
        assert!(descriptor_ready(connection, fence.checkpoint_uuid).unwrap());
        let staging = directory.join("staging.sqlite");
        materialize_staging_catalog(
            &restored,
            &staging,
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            &directory.join("restored-media"),
        )
        .unwrap();
        let promoted = directory.join("promoted.sqlite");
        promote_new_staging_catalog(&staging, &promoted).unwrap();
        assert!(promoted.exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn descriptor_refresh_preserves_legacy_bootstrap_causality() {
        let database = database();
        let tag = database.create_tag("before-enable", None).unwrap();
        let connection = database.connection_for_fsops();
        let library: Vec<u8> = connection
            .query_row(
                "SELECT library_uuid FROM sync_local_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let library = uuid::Uuid::from_slice(&library).unwrap();
        let directory = std::env::temp_dir().join(format!(
            "mangoo-descriptor-bootstrap-{}",
            uuid::Uuid::now_v7()
        ));
        let first =
            capture_checkpoint_fence(connection, StreamVector::new(), test_host(library)).unwrap();
        build_checkpoint(connection, &first, &directory).unwrap();
        let seed = load_and_validate_checkpoint(connection, first.checkpoint_uuid).unwrap();
        assert!(!seed.events.is_empty());
        database.enable_fake_sync_capture_for_test().unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        let tag_uuid: Vec<u8> = transaction
            .query_row("SELECT tag_uuid FROM tags WHERE id=?1", [tag.id], |row| {
                row.get(0)
            })
            .unwrap();
        super::super::capture::capture_catalog_mutation(
            &transaction,
            super::super::capture::MutationOrigin::LocalApi,
            EntityUuid(uuid::Uuid::from_slice(&tag_uuid).unwrap()),
            Operation::SetField {
                field: "name".into(),
                value: serde_json::json!("after-enable"),
            },
        )
        .unwrap();
        transaction.commit().unwrap();
        let second =
            capture_checkpoint_fence(connection, StreamVector::new(), test_host(library)).unwrap();
        build_checkpoint(connection, &second, &directory).unwrap();
        let current = load_and_validate_checkpoint(connection, second.checkpoint_uuid).unwrap();
        assert!(super::super::checkpoint::checkpoint_covers(&current, &seed));
        for event in &seed.events {
            assert!(current.events.contains(event));
        }
        let unique = current
            .events
            .iter()
            .map(|event| (&event.actor, event.actor_seq))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), current.events.len());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn checkpoint_restores_only_selected_root_and_keeps_all_root_truth() {
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let library = uuid::Uuid::from_u128(1);
        let library_root = uuid::Uuid::from_u128(2);
        let first_root = uuid::Uuid::from_u128(10);
        let first_folder = uuid::Uuid::from_u128(11);
        let second_root = uuid::Uuid::from_u128(20);
        let second_folder = uuid::Uuid::from_u128(21);
        for (root_uuid, folder_uuid, path, name, fingerprint) in [
            (first_root, first_folder, "D:/First", "First", [1; 32]),
            (second_root, second_folder, "E:/Second", "Second", [2; 32]),
        ] {
            database
                .upsert_sync_root_descriptor(&crate::db::SyncRootDescriptorRecord {
                    root_uuid: *root_uuid.as_bytes(),
                    root_folder_uuid: *folder_uuid.as_bytes(),
                    library_root_uuid: *library_root.as_bytes(),
                    creation_event_uuid: None,
                    winning_location_revision_uuid: None,
                    requested_name: name.into(),
                    materialized_name: name.into(),
                    materialization_fingerprint: fingerprint,
                    lifecycle: "active".into(),
                    logical_revision: 0,
                    updated_at: 1,
                })
                .unwrap();
            let root_id = database
                .add_root(NewRoot {
                    path: path.into(),
                    display_name: name.into(),
                })
                .unwrap();
            database
                .connection_for_fsops()
                .execute(
                    "UPDATE roots SET root_uuid=?1 WHERE id=?2",
                    params![root_uuid.as_bytes().as_slice(), root_id],
                )
                .unwrap();
            database.connection_for_fsops().execute(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,status,folder_uuid) VALUES (?1,NULL,'','root','active',?2)",
                params![root_id, folder_uuid.as_bytes().as_slice()],
            ).unwrap();
            database
                .link_sync_root_descriptor(root_id, root_uuid.as_bytes(), folder_uuid.as_bytes())
                .unwrap();
            let asset = uuid::Uuid::from_u128(root_uuid.as_u128() + 100);
            database.connection_for_fsops().execute(
                "INSERT INTO files(root_id,folder_id,name,ext,size,mtime,status,asset_uuid) SELECT ?1,id,?2,'jpg',1,1,'active',?3 FROM folders WHERE folder_uuid=?4",
                params![root_id, format!("{name}.jpg"), asset.as_bytes().as_slice(), folder_uuid.as_bytes().as_slice()],
            ).unwrap();
        }
        database
            .initialize_sync_identity(library, library_root, uuid::Uuid::now_v7())
            .unwrap();
        let fence = capture_checkpoint_fence(
            database.connection_for_fsops(),
            StreamVector::new(),
            test_host(library),
        )
        .unwrap();
        let directory =
            std::env::temp_dir().join(format!("mangoo-multi-root-{}", uuid::Uuid::now_v7()));
        let path = build_checkpoint(database.connection_for_fsops(), &fence, &directory).unwrap();
        let checkpoint: Checkpoint = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(checkpoint.state.roots.len(), 2);
        let staging = directory.join("subset.sqlite");
        materialize_staging_catalog_with_roots(
            &checkpoint,
            &staging,
            library_root,
            uuid::Uuid::now_v7(),
            &[(second_root, PathBuf::from("F:/Chosen"))],
        )
        .unwrap();
        let restored = Database::open(&staging).unwrap();
        assert_eq!(restored.list_roots().unwrap().len(), 1);
        assert_eq!(restored.list_roots().unwrap()[0].path, "F:/Chosen");
        assert_eq!(
            restored
                .connection_for_fsops()
                .query_row("SELECT COUNT(*) FROM sync_root_descriptors", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            2
        );
        assert_eq!(
            restored
                .connection_for_fsops()
                .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(restored);
        let unselected_staging = directory.join("unselected.sqlite");
        materialize_staging_catalog_with_roots(
            &checkpoint,
            &unselected_staging,
            library_root,
            uuid::Uuid::now_v7(),
            &[],
        )
        .unwrap();
        let unselected = Database::open(&unselected_staging).unwrap();
        assert_eq!(unselected.list_roots().unwrap().len(), 0);
        assert_eq!(
            unselected
                .connection_for_fsops()
                .query_row("SELECT COUNT(*) FROM sync_root_descriptors", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            2
        );
        assert_eq!(
            unselected
                .connection_for_fsops()
                .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(unselected);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn checkpoint_restore_installs_final_names_after_both_actor_history_orders() {
        let library = uuid::Uuid::from_u128(1);
        let library_root = EntityUuid(uuid::Uuid::from_u128(2));
        let roots = [10_u128, 20].map(|id| EntityUuid(uuid::Uuid::from_u128(id)));
        let sentinels = [11_u128, 21].map(|id| EntityUuid(uuid::Uuid::from_u128(id)));
        // The later moved sibling sorts before the old name owner, so the
        // final unsuffixed name is occupied during an old-order browse replay.
        let folders = [50_u128, 40].map(|id| EntityUuid(uuid::Uuid::from_u128(id)));
        let assets = [70_u128, 60].map(|id| EntityUuid(uuid::Uuid::from_u128(id)));
        let tag = EntityUuid(uuid::Uuid::from_u128(80));
        let mut original = CatalogProjectionSnapshot::default();
        let mut root_truth = BTreeMap::new();
        for index in 0..2 {
            let name = format!("Root {index}");
            root_truth.insert(
                roots[index],
                super::super::reducer::SyncRootTruth {
                    root_uuid: roots[index],
                    root_folder_uuid: sentinels[index],
                    library_root_uuid: library_root,
                    creation_event_uuid: None,
                    winning_location_revision_uuid: None,
                    requested_name: name.clone(),
                    materialized_name: name.clone(),
                    materialization_fingerprint: [index as u8; 32],
                    lifecycle: "active".into(),
                    logical_revision: 0,
                },
            );
            original.roots.push(RootProjectionSnapshot {
                root_uuid: roots[index],
                root_folder_uuid: sentinels[index],
                display_name: name,
            });
            original.folders.push(FolderSnapshot {
                uuid: sentinels[index],
                root_uuid: roots[index],
                parent_uuid: None,
                name: String::new(),
                mtime: None,
                status: "active".into(),
            });
            original.folders.push(FolderSnapshot {
                uuid: folders[index],
                root_uuid: roots[index],
                parent_uuid: Some(sentinels[index]),
                name: "Shared".into(),
                mtime: None,
                status: "active".into(),
            });
            original.files.push(FileSnapshot {
                uuid: assets[index],
                root_uuid: roots[index],
                folder_uuid: folders[index],
                name: "shared.jpg".into(),
                ext: "jpg".into(),
                size: 7,
                mtime: 1,
                ctime: None,
                status: "active".into(),
                // These are classified images, so retain media presentation
                // while exercising actor ordering and final-name collisions.
                media_kind: Some("image".into()),
            });
            original.metadata.push(MetadataSnapshot {
                asset_uuid: assets[index],
                rating: Some(4),
                favorite: true,
                note: Some("portable note".into()),
                updated_at: 0,
            });
            original.memberships.push((assets[index], tag));
        }
        original.tags.push(TagSnapshot {
            uuid: tag,
            name: "reviewed".into(),
            color: Some("#112233".into()),
        });
        let directory =
            std::env::temp_dir().join(format!("mangoo-checkpoint-order-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&directory).unwrap();
        let recovery_database = Database::open_in_memory().unwrap();
        recovery_database.apply_migrations().unwrap();
        let recovery = capture_recovery_snapshot(recovery_database.connection_for_fsops()).unwrap();
        for history_first in [true, false] {
            let mut events = bootstrap_projection_events(
                LibraryUuid(library),
                &original,
                &ProtocolState::default(),
                &[],
            )
            .unwrap();
            for event in &mut events {
                event.actor.device_uuid =
                    DeviceUuid(uuid::Uuid::from_u128(if history_first { 100 } else { 300 }));
                *event = seal_event(event.clone()).unwrap();
            }
            let actor = ActorId {
                device_uuid: DeviceUuid(uuid::Uuid::from_u128(200)),
                stream_uuid: StreamUuid(uuid::Uuid::from_u128(201)),
                actor_epoch: ActorEpochUuid(uuid::Uuid::from_u128(202)),
            };
            let mut append = |seq, entity, parent, name: &str| {
                let supersedes = events
                    .iter()
                    .filter_map(|event| match &event.operation {
                        Operation::SetLocation { revision_uuid, .. }
                            if event.entity_uuid == entity =>
                        {
                            Some(*revision_uuid)
                        }
                        _ => None,
                    })
                    .collect();
                events.push(
                    seal_event(EventEnvelope {
                        wire_version: super::super::codec::WIRE_VERSION,
                        envelope_version: super::super::codec::ENVELOPE_VERSION,
                        reducer_version: REDUCER_VERSION,
                        library_uuid: LibraryUuid(library),
                        event_uuid: EventUuid(uuid::Uuid::from_u128(1000 + seq as u128)),
                        entity_uuid: entity,
                        stream_uuid: actor.stream_uuid,
                        stream_kind: StreamKind::Catalog,
                        actor: actor.clone(),
                        actor_seq: seq,
                        binding_epoch: BindingEpochUuid(uuid::Uuid::from_u128(203)),
                        hlc: Hlc {
                            wall_ms: 10,
                            logical: seq as u32,
                        },
                        operation: Operation::SetLocation {
                            revision_uuid: RevisionUuid(uuid::Uuid::from_u128(2000 + seq as u128)),
                            parent_uuid: Some(parent),
                            name: name.into(),
                            supersedes,
                        },
                        base_events: Default::default(),
                        key_version: 0,
                        integrity_hash: [0; 32],
                    })
                    .unwrap(),
                );
            };
            append(1, sentinels[0], library_root, "Root 0");
            append(2, sentinels[1], library_root, "Root 1");
            append(3, folders[1], sentinels[0], "Shared");
            append(4, assets[1], folders[0], "shared.jpg");
            let mut state = ProtocolState::default();
            let mut vector = StreamVector::new();
            for event in &events {
                state.apply(event);
                vector
                    .entry(event.actor.clone())
                    .and_modify(|seq| *seq = (*seq).max(event.actor_seq))
                    .or_insert(event.actor_seq);
            }
            state.roots = root_truth.clone();
            let locations = legalize_locations(&state, library_root);
            let mut projection = original.clone();
            for folder in projection
                .folders
                .iter_mut()
                .filter(|folder| folders.contains(&folder.uuid))
            {
                folder.root_uuid = roots[0];
                folder.parent_uuid = locations[&folder.uuid].parent_uuid;
                folder.name = locations[&folder.uuid].display_name.clone();
            }
            for file in &mut projection.files {
                file.root_uuid = roots[0];
                file.folder_uuid = locations[&file.uuid].parent_uuid.unwrap();
                file.name = locations[&file.uuid].display_name.clone();
            }
            complete_portable_projection(&mut projection, &state).unwrap();
            let checkpoint = Checkpoint::build(
                CheckpointUuid::new(),
                LibraryUuid(library),
                vector,
                StreamVector::new(),
                test_host(library),
                projection,
                state,
                events,
                recovery.clone(),
                vec![],
            )
            .unwrap();
            for selected_count in 0..=2 {
                let selected = roots[..selected_count]
                    .iter()
                    .map(|root| (root.0, directory.join(root.to_string())))
                    .collect::<Vec<_>>();
                let path = directory.join(format!("{history_first}-{selected_count}.sqlite"));
                materialize_staging_catalog_with_roots(
                    &checkpoint,
                    &path,
                    library_root.0,
                    uuid::Uuid::now_v7(),
                    &selected,
                )
                .unwrap();
                let restored = Database::open(&path).unwrap();
                assert_eq!(restored.list_roots().unwrap().len(), selected_count);
                assert_eq!(restored.sync_root_descriptors().unwrap().len(), 2);
                let expected_files = if selected_count == 0 { 0 } else { 2 };
                let facts: (i64, i64, i64, i64) = restored.connection_for_fsops().query_row(
                    "SELECT (SELECT COUNT(DISTINCT name) FROM files),(SELECT COUNT(*) FROM file_tags),(SELECT COUNT(*) FROM user_metadata WHERE note='portable note' AND rating=4 AND favorite=1),(SELECT COUNT(*) FROM tags WHERE name='reviewed' AND color='#112233')",
                    [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
                ).unwrap();
                assert_eq!(facts, (expected_files, expected_files, expected_files, 1));
                assert_eq!(
                    restored
                        .connection_for_fsops()
                        .query_row(
                            "SELECT COUNT(*) FROM media_fts WHERE media_fts MATCH 'shared'",
                            [],
                            |row| row.get::<_, i64>(0)
                        )
                        .unwrap(),
                    expected_files
                );
                for descriptor in restored.sync_root_descriptors().unwrap() {
                    let expected =
                        &root_truth[&EntityUuid(uuid::Uuid::from_bytes(descriptor.root_uuid))];
                    assert_eq!(descriptor.logical_revision, expected.logical_revision);
                    assert_eq!(
                        descriptor.materialization_fingerprint,
                        expected.materialization_fingerprint
                    );
                }
            }
            let mut invalid = checkpoint.clone();
            let event = invalid
                .events
                .iter_mut()
                .find(|event| event.actor == actor && event.actor_seq == 3)
                .unwrap();
            let Operation::SetLocation { supersedes, .. } = &mut event.operation else {
                unreachable!()
            };
            supersedes.insert(RevisionUuid(uuid::Uuid::from_u128(9999)));
            *event = seal_event(event.clone()).unwrap();
            invalid.state = ProtocolState::default();
            for event in &invalid.events {
                invalid.state.apply(event);
            }
            invalid.state.roots = root_truth.clone();
            let invalid = Checkpoint::build(
                invalid.checkpoint_uuid,
                invalid.library_uuid,
                invalid.catalog_vector,
                invalid.host_vector,
                invalid.host_metadata,
                invalid.projection,
                invalid.state,
                invalid.events,
                invalid.recovery,
                invalid.media_index_shards,
            )
            .unwrap();
            let path = directory.join(format!("missing-reference-{history_first}.sqlite"));
            let error = materialize_staging_catalog_with_roots(
                &invalid,
                &path,
                library_root.0,
                uuid::Uuid::now_v7(),
                &[(roots[0].0, directory.join("invalid-root"))],
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains("FOREIGN KEY constraint failed"));
            let rejected = Database::open(&path).unwrap();
            assert_eq!(rejected.connection_for_fsops().query_row(
                "SELECT (SELECT COUNT(*) FROM roots)+(SELECT COUNT(*) FROM files)+(SELECT COUNT(*) FROM sync_events)",
                [], |row| row.get::<_, i64>(0),
            ).unwrap(), 0);
            assert_eq!(
                rejected
                    .connection_for_fsops()
                    .query_row("PRAGMA defer_foreign_keys", [], |row| row.get::<_, i64>(0),)
                    .unwrap(),
                0
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interrupted_checkpoint_never_passes_descriptor_gate() {
        let database = database();
        let connection = database.connection_for_fsops();
        let library: Vec<u8> = connection
            .query_row(
                "SELECT library_uuid FROM sync_local_state WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let fence = capture_checkpoint_fence(
            connection,
            StreamVector::new(),
            test_host(uuid::Uuid::from_slice(&library).unwrap()),
        )
        .unwrap();
        assert!(!descriptor_ready(connection, fence.checkpoint_uuid).unwrap());
        assert_eq!(recover_interrupted_checkpoints(connection).unwrap(), 1);
    }

    #[test]
    fn checkpoint_excludes_projection_and_truth_mutated_after_fence() {
        let database = database();
        database.enable_fake_sync_capture_for_test().unwrap();
        let tag = database.create_tag("before", None).unwrap();
        let tag_uuid: Vec<u8> = connection_for_tag(&database, tag.id);
        let connection = database.connection_for_fsops();
        let library: Vec<u8> = connection
            .query_row(
                "SELECT library_uuid FROM sync_local_state WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let fence = capture_checkpoint_fence(
            connection,
            StreamVector::new(),
            test_host(uuid::Uuid::from_slice(&library).unwrap()),
        )
        .unwrap();
        connection
            .execute("UPDATE tags SET name='after' WHERE id=?1", [tag.id])
            .unwrap();
        let directory =
            std::env::temp_dir().join(format!("mangoo-checkpoint-race-{}", uuid::Uuid::now_v7()));
        build_checkpoint(connection, &fence, &directory).unwrap();
        let checkpoint = load_and_validate_checkpoint(connection, fence.checkpoint_uuid).unwrap();
        assert_eq!(
            checkpoint
                .projection
                .tags
                .iter()
                .find(|v| v.uuid.0.as_bytes() == tag_uuid.as_slice())
                .unwrap()
                .name,
            "before"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    fn connection_for_tag(database: &Database, id: i64) -> Vec<u8> {
        database
            .connection_for_fsops()
            .query_row("SELECT tag_uuid FROM tags WHERE id=?1", [id], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn checkpoint_round_trips_unfinished_recovery_state_and_blocks_portable_gate() {
        let database = database();
        let connection = database.connection_for_fsops();
        let (root_id, root_uuid): (i64, Vec<u8>) = connection
            .query_row("SELECT id,root_uuid FROM roots LIMIT 1", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        let root_uuid = uuid::Uuid::from_slice(&root_uuid).unwrap();
        let root_folder_uuid = uuid::Uuid::now_v7();
        let binding_uuid = uuid::Uuid::now_v7();
        let library: Vec<u8> = connection
            .query_row(
                "SELECT library_uuid FROM sync_local_state WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        database
            .upsert_sync_root_descriptor(&SyncRootDescriptorRecord {
                root_uuid: *root_uuid.as_bytes(),
                root_folder_uuid: *root_folder_uuid.as_bytes(),
                library_root_uuid: *root_uuid.as_bytes(),
                creation_event_uuid: None,
                winning_location_revision_uuid: None,
                requested_name: "Managed".to_owned(),
                materialized_name: "Managed".to_owned(),
                materialization_fingerprint: [7; 32],
                lifecycle: "active".to_owned(),
                logical_revision: 1,
                updated_at: 1,
            })
            .unwrap();
        connection
            .execute(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,status,folder_uuid) VALUES (?1,NULL,'','root','active',?2)",
                params![root_id, root_folder_uuid.as_bytes().as_slice()],
            )
            .unwrap();
        let folder_id = connection.last_insert_rowid();
        database
            .link_sync_root_descriptor(root_id, root_uuid.as_bytes(), root_folder_uuid.as_bytes())
            .unwrap();
        database
            .upsert_sync_root_local_binding(&SyncRootLocalBindingRecord {
                root_uuid: *root_uuid.as_bytes(),
                library_uuid: *uuid::Uuid::from_slice(&library).unwrap().as_bytes(),
                local_binding_uuid: *binding_uuid.as_bytes(),
                binding_generation: 3,
                root_capture_epoch: [70; 16],
                root_actor_epoch: [71; 16],
                state: "active".to_owned(),
                root_id,
                operation_revision: 1,
                updated_at: 1,
            })
            .unwrap();
        let asset = uuid::Uuid::now_v7();
        connection
            .execute(
                "INSERT INTO files(root_id,folder_id,name,ext,size,mtime,asset_uuid) VALUES (?1,?2,'secret.jpg','jpg',3,1,?3)",
                params![root_id, folder_id, asset.as_bytes().as_slice()],
            )
            .unwrap();
        let revision = uuid::Uuid::now_v7();
        connection.execute("INSERT INTO asset_content_revisions(content_revision_uuid,asset_uuid,local_mutation_generation,created_at) VALUES (?1,?2,1,1)",params![revision.as_bytes().as_slice(),asset.as_bytes().as_slice()]).unwrap();
        let source_path = r"D:\Private\Originals\secret.jpg";
        connection.execute("INSERT INTO sync_publication_intents(intent_uuid,asset_uuid,content_revision_uuid,local_mutation_generation,source_path,state,attempt_count,created_at,updated_at) VALUES (?1,?2,?3,1,?4,'pending-hash',0,1,1)",params![uuid::Uuid::now_v7().as_bytes().as_slice(),asset.as_bytes().as_slice(),revision.as_bytes().as_slice(),source_path]).unwrap();
        let source_token: (Vec<u8>, Vec<u8>, i64) = connection
            .query_row(
                "SELECT root_uuid,local_binding_uuid,binding_generation FROM sync_publication_intents",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            source_token,
            (
                root_uuid.as_bytes().to_vec(),
                binding_uuid.as_bytes().to_vec(),
                3
            )
        );
        let fence = capture_checkpoint_fence(
            connection,
            StreamVector::new(),
            test_host(uuid::Uuid::from_slice(&library).unwrap()),
        )
        .unwrap();
        let directory =
            std::env::temp_dir().join(format!("mangoo-checkpoint-work-{}", uuid::Uuid::now_v7()));
        build_checkpoint(connection, &fence, &directory).unwrap();
        let checkpoint = load_and_validate_checkpoint(connection, fence.checkpoint_uuid).unwrap();
        assert!(!checkpoint.recovery.portable_complete);
        assert!(!descriptor_ready(connection, fence.checkpoint_uuid).unwrap());
        assert!(checkpoint
            .recovery
            .tables
            .iter()
            .find(|v| v.table == "sync_publication_intents")
            .is_some_and(|v| v.rows.len() == 1));
        let encoded = super::canonical_bytes(&checkpoint).unwrap();
        assert!(
            !String::from_utf8_lossy(&encoded).contains(source_path),
            "portable checkpoint must redact the originating absolute path"
        );
        let publication = checkpoint
            .recovery
            .tables
            .iter()
            .find(|table| table.table == "sync_publication_intents")
            .unwrap();
        for column in ["root_uuid", "local_binding_uuid", "binding_generation"] {
            assert!(!publication
                .columns
                .iter()
                .any(|candidate| candidate == column));
        }
        let staging = directory.join("unfinished.sqlite");
        let restore_error = materialize_staging_catalog_with_roots(
            &checkpoint,
            &staging,
            root_uuid,
            uuid::Uuid::now_v7(),
            &[(root_uuid, PathBuf::from("D:/Restored"))],
        )
        .unwrap_err();
        assert!(
            restore_error
                .to_string()
                .contains("checkpoint contains unfinished device-local work"),
            "unfinished local publication work must remain non-portable: {restore_error:#}"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn checkpoint_uses_external_bounded_media_indexes_and_restores_chunk_locations() {
        let database = database();
        let connection = database.connection_for_fsops();
        let asset = uuid::Uuid::now_v7();
        let revision = uuid::Uuid::now_v7();
        let manifest_hash = [1_u8; 32];
        let manifest_pack_hash = [2_u8; 32];
        let chunk_hash = [3_u8; 32];
        let data_pack_hash = [4_u8; 32];
        let root_id: i64 = connection
            .query_row("SELECT id FROM roots LIMIT 1", [], |row| row.get(0))
            .unwrap();
        connection
            .execute(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,mtime,status) VALUES (?1,NULL,'','root-hash',1,'active')",
                [root_id],
            )
            .unwrap();
        let folder_id = connection.last_insert_rowid();
        connection
            .execute(
                "INSERT INTO files(root_id,folder_id,asset_uuid,name,ext,size,mtime,status) VALUES (?1,?2,?3,'remote.jpg','jpg',7,1,'ready')",
                params![root_id, folder_id, asset.as_bytes().as_slice()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO asset_content_revisions(content_revision_uuid,asset_uuid,manifest_hash,byte_size,local_mutation_generation,created_at) VALUES (?1,?2,?3,7,1,1)",
                params![revision.as_bytes().as_slice(), asset.as_bytes().as_slice(), manifest_hash.as_slice()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO asset_content_heads(asset_uuid,content_revision_uuid,is_display_winner) VALUES (?1,?2,1)",
                params![asset.as_bytes().as_slice(), revision.as_bytes().as_slice()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO media_packs(pack_hash,object_key,byte_length,state,created_at,updated_at) VALUES (?1,'content/data/04.pack',64,'durable',1,1), (?2,'content/manifests/02.mpack',64,'durable',1,1)",
                params![data_pack_hash.as_slice(), manifest_pack_hash.as_slice()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO media_chunks(chunk_hash,byte_length,pack_hash,pack_offset,state,created_at,updated_at) VALUES (?1,7,?2,5,'sealed',1,1)",
                params![chunk_hash.as_slice(), data_pack_hash.as_slice()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO media_manifests(manifest_hash,content_hash,byte_length,manifest_pack_hash,pack_offset,record_length,state,created_at,updated_at) VALUES (?1,?2,7,?3,9,11,'durable',1,1)",
                params![manifest_hash.as_slice(), [5_u8; 32].as_slice(), manifest_pack_hash.as_slice()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO media_content_manifest_links(content_revision_uuid,manifest_hash,linked_at) VALUES (?1,?2,1)",
                params![revision.as_bytes().as_slice(), manifest_hash.as_slice()],
            )
            .unwrap();
        let library: Vec<u8> = connection
            .query_row(
                "SELECT library_uuid FROM sync_local_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let fence = capture_checkpoint_fence(
            connection,
            StreamVector::new(),
            test_host(uuid::Uuid::from_slice(&library).unwrap()),
        )
        .unwrap();
        let library_dir =
            std::env::temp_dir().join(format!("mangoo-media-index-{}", uuid::Uuid::now_v7()));
        let checkpoint_dir = library_dir.join("sync-spool").join("checkpoints");
        build_checkpoint(connection, &fence, &checkpoint_dir).unwrap();
        let checkpoint = load_and_validate_checkpoint(connection, fence.checkpoint_uuid).unwrap();
        assert_eq!(checkpoint.media_index_shards.len(), 2);
        assert!(checkpoint
            .media_index_shards
            .iter()
            .all(|shard| shard.byte_length <= super::super::media::MAX_INDEX_SHARD_BYTES as u64));
        let encoded = canonical_bytes(&checkpoint).unwrap();
        assert!(!String::from_utf8_lossy(&encoded).contains("content/data/04.pack"));

        let staging = library_dir.join("catalog.sqlite.sync-staging");
        let restored_database = Database::open(&staging).unwrap();
        restored_database.apply_migrations().unwrap();
        ingest_media_index_shards(
            restored_database.connection_for_fsops(),
            &checkpoint.media_index_shards,
            &library_dir.join("sync-spool").join("indexes"),
        )
        .unwrap();
        drop(restored_database);
        let restored = Connection::open(&staging).unwrap();
        let chunk_location: (Vec<u8>, i64, String) = restored
            .query_row(
                "SELECT pack_hash,pack_offset,state FROM media_chunks WHERE chunk_hash=?1",
                [chunk_hash.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            chunk_location,
            (data_pack_hash.to_vec(), 5, "remote".into())
        );
        drop(restored);
        drop(database);
        std::fs::remove_dir_all(library_dir).unwrap();
    }

    fn test_host(library_uuid: uuid::Uuid) -> HostMetadataSnapshot {
        HostMetadataSnapshot {
            library_uuid: library_uuid.to_string(),
            name: "Managed".into(),
            icon: None,
            binding_state: "synchronized".into(),
            events: Vec::new(),
        }
    }
}
