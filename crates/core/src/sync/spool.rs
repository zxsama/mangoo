use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};

use super::segment::{verify_segment, SealedSegment};

/// Metadata handed to the authoritative inbox registration transaction only
/// after the object is durable at `spool_path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableInboxSegment {
    pub segment_uuid: [u8; 16],
    pub stream_uuid: [u8; 16],
    pub device_uuid: [u8; 16],
    pub actor_epoch: [u8; 16],
    pub segment_hash: [u8; 32],
    pub payload_hash: [u8; 32],
    pub spool_path: PathBuf,
    pub first_seq: u64,
    pub last_seq: u64,
    pub event_count: u32,
}

/// Provider-neutral durable intake boundary. Downloaded bytes are decoded and
/// fully verified before they are made visible. The database registrar runs
/// after the rename, so a crash can leave only an unregistered durable object;
/// startup reconciliation may safely register it on the next pass.
pub fn intake_segment<F>(
    spool_dir: &Path,
    encoded: &[u8],
    expected_predecessor: Option<[u8; 32]>,
    register: F,
) -> anyhow::Result<DurableInboxSegment>
where
    F: FnOnce(&DurableInboxSegment) -> anyhow::Result<()>,
{
    let segment: SealedSegment = serde_json::from_slice(encoded).context("decode sync segment")?;
    verify_segment(&segment, expected_predecessor).context("verify sync segment")?;
    let canonical = serde_json::to_vec(&segment).context("encode verified sync segment")?;
    if canonical != encoded {
        anyhow::bail!("sync segment is not in canonical JSON encoding");
    }

    fs::create_dir_all(spool_dir).context("create sync spool directory")?;
    let hash_hex = hex(&segment.segment_hash);
    let final_path = spool_dir.join(format!("{hash_hex}.segment.json"));
    let mut segment_uuid = [0_u8; 16];
    segment_uuid.copy_from_slice(&segment.segment_hash[..16]);
    let durable = DurableInboxSegment {
        segment_uuid,
        stream_uuid: *segment.header.actor.stream_uuid.0.as_bytes(),
        device_uuid: *segment.header.actor.device_uuid.0.as_bytes(),
        actor_epoch: *segment.header.actor.actor_epoch.0.as_bytes(),
        segment_hash: segment.segment_hash,
        payload_hash: segment.header.payload_hash,
        spool_path: final_path.clone(),
        first_seq: segment.header.first_seq,
        last_seq: segment.header.last_seq,
        event_count: segment.header.count,
    };

    if final_path.exists() {
        let existing = fs::read(&final_path).context("read duplicate spool segment")?;
        if existing != encoded {
            anyhow::bail!("spool hash collision or corrupt existing segment");
        }
        register(&durable)?;
        return Ok(durable);
    }

    let temp_path = spool_dir.join(format!(".{hash_hex}.{}.tmp", uuid::Uuid::new_v4()));
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .context("create spool temporary file")?;
        file.write_all(encoded)
            .context("write spool temporary file")?;
        file.flush().context("flush spool temporary file")?;
        file.sync_all().context("fsync spool temporary file")?;
        drop(file);
        match fs::rename(&temp_path, &final_path) {
            Ok(()) => {}
            Err(error) if final_path.exists() => {
                let existing = fs::read(&final_path).context("read raced spool segment")?;
                if existing != encoded {
                    return Err(error).context("publish spool segment");
                }
                let _ = fs::remove_file(&temp_path);
            }
            Err(error) => return Err(error).context("publish spool segment"),
        }
        sync_directory(spool_dir).context("fsync sync spool directory")?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result?;
    super::fault::hit("spool.after-durable-publish");
    register(&durable)?;
    Ok(durable)
}

pub fn register_durable_segment(
    connection: &Connection,
    segment: &DurableInboxSegment,
) -> anyhow::Result<bool> {
    let existing: Option<(Vec<u8>, String)> = connection
        .query_row(
            "SELECT payload_hash,spool_path FROM sync_inbox_segments WHERE segment_uuid=?1",
            [segment.segment_uuid.as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((hash, path)) = existing {
        if hash != segment.payload_hash || path != segment.spool_path.to_string_lossy() {
            anyhow::bail!("inbox segment identity collision");
        }
        return Ok(false);
    }
    connection.execute("INSERT INTO sync_inbox_segments(segment_uuid,stream_uuid,device_uuid,actor_epoch,first_seq,last_seq,event_count,payload_hash,spool_path,state,received_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'durable',?10)", params![segment.segment_uuid.as_slice(),segment.stream_uuid.as_slice(),segment.device_uuid.as_slice(),segment.actor_epoch.as_slice(),segment.first_seq as i64,segment.last_seq as i64,segment.event_count as i64,segment.payload_hash.as_slice(),segment.spool_path.to_string_lossy(),now_ms()])?;
    super::fault::hit("spool.after-registration");
    Ok(true)
}

/// Applies at most one bounded writer turn from the durable spool. Re-entering
/// after a crash is safe because ledger insertion/frontier update is atomic and
/// exact duplicate events are idempotent.
pub fn apply_next_durable_segment(connection: &Connection) -> anyhow::Result<usize> {
    // Providers may return Delta pages in arbitrary object-key order. Pick a
    // segment that is contiguous with (or entirely behind) the actor's durable
    // frontier instead of letting an early successor block its predecessor.
    let row: Option<(Vec<u8>,String)> = connection.query_row("SELECT s.segment_uuid,s.spool_path FROM sync_inbox_segments s WHERE s.state IN ('durable','ready','applying') AND s.first_seq<=COALESCE((SELECT f.applied_through_seq FROM sync_actor_frontiers f WHERE f.device_uuid=s.device_uuid AND f.stream_uuid=s.stream_uuid AND f.actor_epoch=s.actor_epoch),0)+1 ORDER BY s.received_at,s.first_seq,s.segment_uuid LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((segment_uuid, path)) = row else {
        return Ok(0);
    };
    let bytes = fs::read(&path).context("read durable inbox segment")?;
    let segment: SealedSegment =
        serde_json::from_slice(&bytes).context("decode durable inbox segment")?;
    verify_segment(&segment, segment.header.predecessor_hash)?;
    let frontier: i64=connection.query_row("SELECT COALESCE((SELECT applied_through_seq FROM sync_actor_frontiers WHERE device_uuid=?1 AND stream_uuid=?2 AND actor_epoch=?3),0)",params![segment.header.actor.device_uuid.0.as_bytes().as_slice(),segment.header.actor.stream_uuid.0.as_bytes().as_slice(),segment.header.actor.actor_epoch.0.as_bytes().as_slice()],|r|r.get(0))?;
    let mut decoded = 0usize;
    let mut batch = Vec::new();
    for event in segment
        .events
        .iter()
        .filter(|e| e.actor_seq > frontier as u64)
    {
        let size = super::codec::encode_event(event)?.len();
        if !batch.is_empty()
            && (batch.len() == super::inbox::MAX_APPLY_EVENTS
                || decoded + size > super::inbox::MAX_APPLY_DECODED_BYTES)
        {
            break;
        }
        decoded += size;
        batch.push(event.clone());
    }
    if batch.is_empty() {
        connection.execute(
            "UPDATE sync_inbox_segments SET state='applied',applied_at=?1 WHERE segment_uuid=?2",
            params![now_ms(), segment_uuid],
        )?;
        return Ok(0);
    }
    let transaction = connection.unchecked_transaction()?;
    transaction.execute(
        "UPDATE sync_inbox_segments SET state='applying' WHERE segment_uuid=?1",
        [&segment_uuid],
    )?;
    let applied = match super::inbox::apply_remote_batch(&transaction, &batch) {
        Ok(applied) => applied,
        Err(error)
            if error.to_string().contains("unknown synchronization schema")
                || error.to_string().contains("actor gap")
                || error.to_string().contains("actor fork") =>
        {
            // The blocking diagnostic/frontier state is authoritative. Keep it
            // without advancing the inbox segment so a later repair can retry.
            transaction.commit()?;
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    if batch.last().map(|e| e.actor_seq) == Some(segment.header.last_seq) {
        transaction.execute(
            "UPDATE sync_inbox_segments SET state='applied',applied_at=?1 WHERE segment_uuid=?2",
            params![now_ms(), segment_uuid],
        )?;
    } else {
        transaction.execute(
            "UPDATE sync_inbox_segments SET state='ready' WHERE segment_uuid=?1",
            [&segment_uuid],
        )?;
    }
    transaction.commit()?;
    Ok(applied)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(path)?.sync_all()
    }
    #[cfg(windows)]
    {
        // Windows does not allow opening directories through std::fs::File.
        // The file itself has already been FlushFileBuffers'd; MoveFileEx-style
        // rename is atomic on the same volume. Recovery scans the content-hash
        // namespace for an object whose DB registration was interrupted.
        let _ = path;
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(value, "{byte:02x}");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::segment::seal_segment;
    use crate::sync::test_model::event;
    use crate::sync::types::Operation;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fixture() -> (PathBuf, Vec<u8>, SealedSegment) {
        let directory = std::env::temp_dir().join(format!("mangoo-spool-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&directory).unwrap();
        let segment = seal_segment(
            vec![event(
                1,
                10,
                Operation::SetField {
                    field: "rating".into(),
                    value: 5.into(),
                },
            )],
            None,
        )
        .unwrap();
        let bytes = serde_json::to_vec(&segment).unwrap();
        (directory, bytes, segment)
    }

    #[test]
    fn intake_publishes_before_registration_and_duplicate_is_idempotent() {
        let (directory, bytes, segment) = fixture();
        let registrations = AtomicUsize::new(0);
        let first = intake_segment(&directory, &bytes, None, |record| {
            assert!(record.spool_path.exists());
            registrations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        let second = intake_segment(&directory, &bytes, None, |_| {
            registrations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.segment_hash, segment.segment_hash);
        assert_eq!(registrations.load(Ordering::SeqCst), 2);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn corrupt_or_wrong_predecessor_never_reaches_disk_or_database() {
        let (directory, mut bytes, _) = fixture();
        let last = bytes.len() - 2;
        bytes[last] ^= 1;
        let called = AtomicUsize::new(0);
        assert!(intake_segment(&directory, &bytes, None, |_| {
            called.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .is_err());
        assert_eq!(called.load(Ordering::SeqCst), 0);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);

        let (_, valid, _) = fixture();
        assert!(intake_segment(&directory, &valid, Some([7; 32]), |_| Ok(())).is_err());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn registration_failure_leaves_recoverable_object_without_temp_file() {
        let (directory, bytes, _) = fixture();
        assert!(intake_segment(&directory, &bytes, None, |_| anyhow::bail!("crash")).is_err());
        let paths = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].extension().and_then(|v| v.to_str()), Some("json"));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn reordered_segments_wait_for_their_predecessor_and_then_converge() {
        let directory =
            std::env::temp_dir().join(format!("mangoo-spool-reordered-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&directory).unwrap();
        let first = seal_segment(
            vec![event(
                1,
                10,
                Operation::SetField {
                    field: "rating".into(),
                    value: 1.into(),
                },
            )],
            None,
        )
        .unwrap();
        let second = seal_segment(
            vec![event(
                2,
                10,
                Operation::SetField {
                    field: "rating".into(),
                    value: 2.into(),
                },
            )],
            Some(first.segment_hash),
        )
        .unwrap();
        let database = crate::db::Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let connection = database.connection_for_fsops();
        for segment in [&second, &first, &second] {
            let bytes = serde_json::to_vec(segment).unwrap();
            intake_segment(
                &directory,
                &bytes,
                segment.header.predecessor_hash,
                |record| register_durable_segment(connection, record).map(|_| ()),
            )
            .unwrap();
        }
        assert_eq!(apply_next_durable_segment(connection).unwrap(), 1);
        assert_eq!(apply_next_durable_segment(connection).unwrap(), 1);
        assert_eq!(apply_next_durable_segment(connection).unwrap(), 0);
        let (frontier, rating, applied): (i64, i64, i64) = connection
            .query_row(
                "SELECT (SELECT MAX(applied_through_seq) FROM sync_actor_frontiers),(SELECT CAST(json_extract(value,'$') AS INTEGER) FROM sync_field_versions WHERE is_winner=1),(SELECT COUNT(*) FROM sync_inbox_segments WHERE state='applied')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((frontier, rating, applied), (2, 2, 2));
        let _ = fs::remove_dir_all(directory);
    }
}
