use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use super::checkpoint::{
    CatalogProjectionSnapshot, CatalogRecoverySnapshot, Checkpoint, FileSnapshot, FolderSnapshot,
    HostMetadataSnapshot, MetadataSnapshot, SqlValueSnapshot, StreamVector, TableSnapshot,
    TagSnapshot,
};
use super::codec::{canonical_bytes, decode_event, seal_event, sha256};
use super::reducer::ProtocolState;
use super::types::{
    ActorEpochUuid, ActorId, BindingEpochUuid, CheckpointUuid, DeviceUuid, EntityUuid,
    EventEnvelope, EventUuid, Hlc, LibraryUuid, Operation, RevisionUuid, StreamKind, StreamUuid,
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
    transaction.execute("INSERT INTO sync_checkpoints(checkpoint_uuid,schema_version,reducer_version,state,capture_local_mutation_generation,created_at) VALUES (?1,1,1,'building',?2,?3)", params![checkpoint_uuid.0.as_bytes().as_slice(),generation,now_ms()])?;
    transaction.commit()?;
    // A private SQLite backup is the immutable fence image. It is created
    // after the short fence transaction and lets the keyset builder release
    // the live WAL immediately while guaranteeing that every later query sees
    // exactly the same projection/truth/event snapshot.
    let snapshot_path =
        std::env::temp_dir().join(format!("megle-checkpoint-{}.sqlite", checkpoint_uuid));
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
            state.apply(&event);
            events.push(event);
            cursor = *rowid;
        }
    }
    let projection = capture_projection(&snapshot)?;
    let bootstrap = bootstrap_projection_events(fence.library_uuid, &projection, &state)?;
    for event in bootstrap {
        if !state.applied_events.contains(&event.event_uuid) {
            state.apply(&event);
            events.push(event);
        }
    }
    let mut checkpoint_vector = fence.catalog_vector.clone();
    for event in &events {
        checkpoint_vector
            .entry(event.actor.clone())
            .and_modify(|seq| *seq = (*seq).max(event.actor_seq))
            .or_insert(event.actor_seq);
    }
    let recovery = capture_recovery_snapshot(&snapshot)?;
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

const RECOVERY_TABLES: &[&str] = &[
    "sync_conflicts",
    "sync_bulk_operations",
    "sync_bulk_items",
    "asset_content_revisions",
    "asset_content_heads",
    "asset_content_supersedes",
    "sync_publication_intents",
];

fn capture_recovery_snapshot(connection: &Connection) -> anyhow::Result<CatalogRecoverySnapshot> {
    use rusqlite::types::ValueRef;
    let mut tables = Vec::new();
    for table in RECOVERY_TABLES {
        let mut statement = connection.prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))?;
        let columns = statement
            .column_names()
            .iter()
            .map(|v| (*v).to_owned())
            .collect();
        let count = statement.column_count();
        let rows = statement
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
        tables.push(TableSnapshot {
            table: (*table).to_owned(),
            columns,
            rows,
        });
    }
    let portable_complete: bool = connection.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM sync_publication_intents WHERE state IN ('pending-hash','pending-manifest','ready','failed'))",
        [],
        |row| row.get(0),
    )?;
    Ok(CatalogRecoverySnapshot {
        tables,
        portable_complete,
    })
}

fn bootstrap_projection_events(
    library: LibraryUuid,
    projection: &CatalogProjectionSnapshot,
    existing: &ProtocolState,
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
    let mut seq = 0u64;
    let mut output = Vec::new();
    let mut push = |entity: EntityUuid, operation: Operation| -> anyhow::Result<()> {
        seq += 1;
        let event_uuid = EventUuid(deterministic_uuid(
            entity.0.as_bytes(),
            format!("bootstrap-{seq}").as_bytes(),
        ));
        output.push(seal_event(EventEnvelope {
            wire_version: 1,
            envelope_version: 1,
            reducer_version: 1,
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
        if existing.entities.contains_key(&folder.uuid) {
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
    checkpoint.verify()?;
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
) -> anyhow::Result<()> {
    checkpoint.verify()?;
    if staging_path.exists() {
        std::fs::remove_file(staging_path)?;
    }
    let database = crate::db::Database::open(staging_path)?;
    database.apply_migrations()?;
    database.add_root(crate::db::NewRoot {
        path: "sync-bootstrap".into(),
        display_name: checkpoint.host_metadata.name.clone(),
    })?;
    database.initialize_sync_identity(checkpoint.library_uuid.0, root_uuid, device_uuid)?;
    restore_projection(database.connection_for_fsops(), checkpoint, root_uuid)?;
    let mut by_actor = checkpoint.events.clone();
    by_actor.sort_by(|a, b| a.actor.cmp(&b.actor).then(a.actor_seq.cmp(&b.actor_seq)));
    for event in by_actor {
        database.apply_remote_sync_events(std::slice::from_ref(&event))?;
    }
    restore_recovery_snapshot(database.connection_for_fsops(), &checkpoint.recovery)?;
    restore_catalog_frontiers(database.connection_for_fsops(), &checkpoint.catalog_vector)?;
    validate_genesis_replay(database.connection_for_fsops(), checkpoint, i64::MAX)?;
    validate_browse_projection(database.connection_for_fsops(), checkpoint)?;
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
        let schema_sql = format!("SELECT * FROM {} LIMIT 0", table.table);
        let schema_statement = transaction.prepare(&schema_sql)?;
        let expected_columns = schema_statement
            .column_names()
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
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
        let sql = format!(
            "INSERT OR REPLACE INTO {} ({}) VALUES ({})",
            table.table,
            table.columns.join(","),
            placeholders
        );
        let mut statement = transaction.prepare(&sql)?;
        for row in &table.rows {
            if row.len() != table.columns.len() {
                anyhow::bail!("checkpoint recovery row width mismatch");
            }
            let values = row.iter().map(bind_snapshot_value).collect::<Vec<_>>();
            statement.execute(rusqlite::params_from_iter(values))?;
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
    let folders = connection.prepare("SELECT f.folder_uuid,p.folder_uuid,f.name,f.path_hash,f.mtime,f.status FROM folders f LEFT JOIN folders p ON p.id=f.parent_id ORDER BY f.folder_uuid")?
        .query_map([], |r| Ok(FolderSnapshot { uuid: super::types::EntityUuid(uuid::Uuid::from_slice(&r.get::<_,Vec<u8>>(0)?).map_err(sql_uuid)?), parent_uuid: r.get::<_,Option<Vec<u8>>>(1)?.map(|v| uuid::Uuid::from_slice(&v).map(super::types::EntityUuid).map_err(sql_uuid)).transpose()?, name:r.get(2)?,path_hash:r.get(3)?,mtime:r.get(4)?,status:r.get(5)? }))?.collect::<Result<Vec<_>,_>>()?;
    let files = connection.prepare("SELECT f.asset_uuid,d.folder_uuid,f.name,f.ext,f.size,f.mtime,f.ctime,f.status,m.kind FROM files f JOIN folders d ON d.id=f.folder_id LEFT JOIN media m ON m.file_id=f.id ORDER BY f.asset_uuid")?
        .query_map([], |r| Ok(FileSnapshot { uuid: super::types::EntityUuid(uuid::Uuid::from_slice(&r.get::<_,Vec<u8>>(0)?).map_err(sql_uuid)?), folder_uuid:super::types::EntityUuid(uuid::Uuid::from_slice(&r.get::<_,Vec<u8>>(1)?).map_err(sql_uuid)?),name:r.get(2)?,ext:r.get(3)?,size:r.get(4)?,mtime:r.get(5)?,ctime:r.get(6)?,status:r.get(7)?,media_kind:r.get(8)? }))?.collect::<Result<Vec<_>,_>>()?;
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
        folders,
        files,
        tags,
        metadata,
        memberships,
    })
}

fn sql_uuid(error: uuid::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Blob, Box::new(error))
}

fn restore_projection(
    connection: &Connection,
    checkpoint: &Checkpoint,
    root_uuid: uuid::Uuid,
) -> anyhow::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    let root_id: i64 = transaction.query_row(
        "SELECT id FROM roots WHERE root_uuid=?1",
        [root_uuid.as_bytes().as_slice()],
        |r| r.get(0),
    )?;
    let mut pending = checkpoint.projection.folders.clone();
    while !pending.is_empty() {
        let before = pending.len();
        let mut rest = Vec::new();
        for folder in pending {
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
            transaction.execute("INSERT INTO folders(root_id,parent_id,name,path_hash,mtime,status,folder_uuid) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![root_id,parent_id,folder.name,folder.path_hash,folder.mtime,folder.status,folder.uuid.0.as_bytes().as_slice()])?;
        }
        if rest.len() == before {
            anyhow::bail!("checkpoint folder graph cannot be materialized");
        }
        pending = rest;
    }
    for file in &checkpoint.projection.files {
        let folder_id: i64 = transaction.query_row(
            "SELECT id FROM folders WHERE folder_uuid=?1",
            [file.folder_uuid.0.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        transaction.execute("INSERT INTO files(root_id,folder_id,name,ext,size,mtime,ctime,status,asset_uuid) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![root_id,folder_id,file.name,file.ext,file.size,file.mtime,file.ctime,file.status,file.uuid.0.as_bytes().as_slice()])?;
        if let Some(kind) = &file.media_kind {
            transaction.execute("INSERT INTO media(file_id,kind,metadata_status) SELECT id,?1,'pending' FROM files WHERE asset_uuid=?2",params![kind,file.uuid.0.as_bytes().as_slice()])?;
        }
    }
    for tag in &checkpoint.projection.tags {
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
    transaction.commit()?;
    Ok(())
}

fn validate_browse_projection(
    connection: &Connection,
    checkpoint: &Checkpoint,
) -> anyhow::Result<()> {
    let actual = capture_projection(connection)?;
    if actual != checkpoint.projection {
        anyhow::bail!("staging browse projection differs from checkpoint projection");
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
    use crate::db::{Database, NewRoot};

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
            std::env::temp_dir().join(format!("megle-checkpoint-{}", uuid::Uuid::now_v7()));
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
        )
        .unwrap();
        let promoted = directory.join("promoted.sqlite");
        promote_new_staging_catalog(&staging, &promoted).unwrap();
        assert!(promoted.exists());
        let _ = std::fs::remove_dir_all(directory);
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
            std::env::temp_dir().join(format!("megle-checkpoint-race-{}", uuid::Uuid::now_v7()));
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
        let asset: Vec<u8> = connection
            .query_row("SELECT asset_uuid FROM files LIMIT 1", [], |r| r.get(0))
            .optional()
            .unwrap()
            .unwrap_or_else(|| uuid::Uuid::now_v7().as_bytes().to_vec());
        let revision = uuid::Uuid::now_v7();
        connection.execute("INSERT INTO asset_content_revisions(content_revision_uuid,asset_uuid,local_mutation_generation,created_at) VALUES (?1,?2,1,1)",params![revision.as_bytes().as_slice(),asset]).unwrap();
        connection.execute("INSERT INTO sync_publication_intents(intent_uuid,asset_uuid,content_revision_uuid,local_mutation_generation,source_path,state,attempt_count,created_at,updated_at) VALUES (?1,?2,?3,1,'local-only','pending-hash',0,1,1)",params![uuid::Uuid::now_v7().as_bytes().as_slice(),asset,revision.as_bytes().as_slice()]).unwrap();
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
            std::env::temp_dir().join(format!("megle-checkpoint-work-{}", uuid::Uuid::now_v7()));
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
        let _ = std::fs::remove_dir_all(directory);
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
