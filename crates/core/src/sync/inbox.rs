use std::collections::{BTreeMap, HashMap};

use anyhow::Context;
use rusqlite::{params, OptionalExtension, Transaction};

use super::segment::{verify_segment, SealedSegment};
use super::types::ActorId;

pub const MAX_APPLY_EVENTS: usize = 100;
pub const MAX_APPLY_DECODED_BYTES: usize = 1024 * 1024;

pub fn apply_remote_batch(
    transaction: &Transaction<'_>,
    events: &[super::types::EventEnvelope],
) -> anyhow::Result<usize> {
    if events.len() > MAX_APPLY_EVENTS {
        anyhow::bail!("remote apply batch exceeds event limit");
    }
    let mut decoded_bytes = 0usize;
    let mut applied = 0usize;
    for event in events {
        if event.wire_version > 1 || event.envelope_version > 1 || event.reducer_version > 1 {
            transaction.execute(
                "UPDATE sync_local_state SET compatibility_state='compatibility-lock' WHERE singleton=1",
                [],
            )?;
            anyhow::bail!("unknown synchronization schema");
        }
        let bytes = super::codec::encode_event(event)?;
        decoded_bytes = decoded_bytes.saturating_add(bytes.len());
        if decoded_bytes > MAX_APPLY_DECODED_BYTES {
            anyhow::bail!("remote apply batch exceeds decoded byte limit");
        }
        super::codec::verify_event(event)?;
        let actor = &event.actor;
        let frontier: Option<i64> = transaction
            .query_row(
                "SELECT applied_through_seq FROM sync_actor_frontiers WHERE device_uuid=?1 AND stream_uuid=?2 AND actor_epoch=?3",
                params![actor.device_uuid.0.as_bytes().as_slice(),actor.stream_uuid.0.as_bytes().as_slice(),actor.actor_epoch.0.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()?;
        let expected = frontier.unwrap_or(0) + 1;
        if event.actor_seq as i64 > expected {
            upsert_frontier_state(transaction, actor, frontier.unwrap_or(0), "gap")?;
            anyhow::bail!("actor gap: expected {expected}, got {}", event.actor_seq);
        }
        let existing: Option<(Vec<u8>, Vec<u8>)> = transaction
            .query_row(
                "SELECT event_uuid,integrity_hash FROM sync_events WHERE device_uuid=?1 AND stream_uuid=?2 AND actor_epoch=?3 AND actor_seq=?4",
                params![actor.device_uuid.0.as_bytes().as_slice(),actor.stream_uuid.0.as_bytes().as_slice(),actor.actor_epoch.0.as_bytes().as_slice(),event.actor_seq as i64],
                |row| Ok((row.get(0)?,row.get(1)?)),
            )
            .optional()?;
        if let Some((event_uuid, hash)) = existing {
            if event_uuid == event.event_uuid.0.as_bytes()
                && hash == event.integrity_hash.as_slice()
            {
                continue;
            }
            upsert_frontier_state(transaction, actor, frontier.unwrap_or(0), "forked")?;
            anyhow::bail!("actor fork at sequence {}", event.actor_seq);
        }
        if (event.actor_seq as i64) < expected {
            anyhow::bail!("frontier advanced without matching ledger event");
        }
        transaction.execute(
            "INSERT INTO sync_events(event_uuid,library_uuid,entity_uuid,stream_uuid,stream_kind,device_uuid,actor_epoch,binding_epoch,actor_seq,hlc_wall_ms,hlc_logical,wire_version,envelope_version,reducer_version,key_version,operation,apply_state,integrity_hash,created_at,applied_at,payload) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,'applied',?17,?18,?18,?19)",
            params![event.event_uuid.0.as_bytes().as_slice(),event.library_uuid.0.as_bytes().as_slice(),event.entity_uuid.0.as_bytes().as_slice(),event.stream_uuid.0.as_bytes().as_slice(),stream_kind(event.stream_kind),actor.device_uuid.0.as_bytes().as_slice(),actor.actor_epoch.0.as_bytes().as_slice(),event.binding_epoch.0.as_bytes().as_slice(),event.actor_seq as i64,event.hlc.wall_ms as i64,event.hlc.logical as i64,event.wire_version,event.envelope_version,event.reducer_version,event.key_version,operation_name(&event.operation),event.integrity_hash.as_slice(),unix_timestamp_millis(),bytes],
        )?;
        super::fault::hit("remote.after-ledger");
        materialize_truth(transaction, event)?;
        super::fault::hit("remote.after-truth-projection");
        upsert_frontier_state(transaction, actor, event.actor_seq as i64, "ready")?;
        super::fault::hit("remote.after-frontier");
        merge_received_hlc(transaction, event.hlc)?;
        applied += 1;
    }
    Ok(applied)
}

pub(crate) fn materialize_truth(
    transaction: &Transaction<'_>,
    event: &super::types::EventEnvelope,
) -> anyhow::Result<()> {
    use super::types::Operation;
    let entity = event.entity_uuid.0.as_bytes().as_slice();
    let event_uuid = event.event_uuid.0.as_bytes().as_slice();
    match &event.operation {
        Operation::SetField { field, value } => {
            transaction.execute(
                "INSERT INTO sync_field_versions(entity_uuid,field_name,event_uuid,is_winner,value) VALUES (?1,?2,?3,0,?4)",
                params![entity,field,event_uuid,serde_json::to_vec(value)?],
            )?;
            transaction.execute(
                "UPDATE sync_field_versions SET is_winner=0 WHERE entity_uuid=?1 AND field_name=?2",
                params![entity, field],
            )?;
            transaction.execute(
                "UPDATE sync_field_versions SET is_winner=1 WHERE entity_uuid=?1 AND field_name=?2 AND event_uuid=(SELECT v.event_uuid FROM sync_field_versions v JOIN sync_events e ON e.event_uuid=v.event_uuid WHERE v.entity_uuid=?1 AND v.field_name=?2 ORDER BY e.hlc_wall_ms DESC,e.hlc_logical DESC,e.device_uuid DESC,e.actor_epoch DESC,e.actor_seq DESC,e.event_uuid DESC LIMIT 1)",
                params![entity,field],
            )?;
            // Delivery order is arbitrary. Materialize the value selected by
            // the total-order query above, not necessarily the just-arrived
            // value (which may be an older loser).
            let winning_value: Vec<u8> = transaction.query_row(
                "SELECT value FROM sync_field_versions WHERE entity_uuid=?1 AND field_name=?2 AND is_winner=1",
                params![entity, field],
                |row| row.get(0),
            )?;
            let winning_value: serde_json::Value = serde_json::from_slice(&winning_value)?;
            materialize_scalar_projection(transaction, entity, field, &winning_value)?;
        }
        Operation::SetAdd { member_uuid, .. } => {
            transaction.execute(
                "INSERT OR IGNORE INTO sync_set_adds(owner_uuid,member_uuid,add_event_uuid) VALUES (?1,?2,?3)",
                params![entity,member_uuid.0.as_bytes().as_slice(),event_uuid],
            )?;
            materialize_tag_projection(transaction, entity)?;
        }
        Operation::SetRemove { observed_adds, .. } => {
            for observed in observed_adds {
                transaction.execute(
                    "INSERT OR IGNORE INTO sync_set_removals(remove_event_uuid,observed_add_event_uuid) VALUES (?1,?2)",
                    params![event_uuid,observed.0.as_bytes().as_slice()],
                )?;
            }
            materialize_tag_projection(transaction, entity)?;
        }
        Operation::SetLocation {
            revision_uuid,
            parent_uuid,
            name,
            supersedes,
        } => {
            transaction.execute(
                "INSERT OR IGNORE INTO sync_location_revisions(revision_uuid,entity_uuid,event_uuid,parent_uuid,name,local_mutation_generation,origin,created_at) VALUES (?1,?2,?3,?4,?5,1,'remote_apply',?6)",
                params![revision_uuid.0.as_bytes().as_slice(),entity,event_uuid,parent_uuid.map(|uuid| uuid.0.as_bytes().to_vec()),name,unix_timestamp_millis()],
            )?;
            for observed in supersedes {
                transaction.execute(
                    "INSERT OR IGNORE INTO sync_location_supersedes(revision_uuid,observed_revision_uuid) VALUES (?1,?2)",
                    params![revision_uuid.0.as_bytes().as_slice(),observed.0.as_bytes().as_slice()],
                )?;
            }
            rebuild_location_heads(transaction, entity)?;
        }
        Operation::Delete { token } => {
            transaction.execute(
                "INSERT OR IGNORE INTO sync_tombstone_tokens(token_uuid,entity_uuid,event_uuid,local_mutation_generation,created_at) VALUES (?1,?2,?3,1,?4)",
                params![token.0.as_bytes().as_slice(),entity,event_uuid,unix_timestamp_millis()],
            )?;
        }
        Operation::Restore { observed_tokens } => {
            for observed in observed_tokens {
                transaction.execute(
                    "INSERT OR IGNORE INTO sync_tombstone_removals(remove_event_uuid,observed_token_uuid) VALUES (?1,?2)",
                    params![event_uuid,observed.0.as_bytes().as_slice()],
                )?;
            }
        }
        Operation::SetContent {
            revision_uuid,
            manifest_id,
            supersedes,
        } => {
            let manifest_hash = super::codec::sha256(manifest_id.as_bytes());
            transaction.execute(
                "INSERT OR IGNORE INTO asset_content_revisions(content_revision_uuid,asset_uuid,event_uuid,manifest_hash,local_mutation_generation,created_at) VALUES (?1,?2,?3,?4,1,?5)",
                params![revision_uuid.0.as_bytes().as_slice(),entity,event_uuid,manifest_hash.as_slice(),unix_timestamp_millis()],
            )?;
            for observed in supersedes {
                transaction.execute(
                    "INSERT OR IGNORE INTO asset_content_supersedes(content_revision_uuid,observed_content_revision_uuid) VALUES (?1,?2)",
                    params![revision_uuid.0.as_bytes().as_slice(),observed.0.as_bytes().as_slice()],
                )?;
            }
            transaction.execute(
                "DELETE FROM asset_content_heads WHERE asset_uuid=?1",
                [entity],
            )?;
            transaction.execute(
                "INSERT INTO asset_content_heads(asset_uuid,content_revision_uuid,is_display_winner) SELECT r.asset_uuid,r.content_revision_uuid,CASE WHEN r.content_revision_uuid=(SELECT rr.content_revision_uuid FROM asset_content_revisions rr JOIN sync_events e ON e.event_uuid=rr.event_uuid WHERE rr.asset_uuid=?1 AND NOT EXISTS (SELECT 1 FROM asset_content_supersedes s WHERE s.observed_content_revision_uuid=rr.content_revision_uuid) ORDER BY e.hlc_wall_ms DESC,e.hlc_logical DESC,e.device_uuid DESC,e.actor_epoch DESC,e.actor_seq DESC,e.event_uuid DESC LIMIT 1) THEN 1 ELSE 0 END FROM asset_content_revisions r WHERE r.asset_uuid=?1 AND NOT EXISTS (SELECT 1 FROM asset_content_supersedes s WHERE s.observed_content_revision_uuid=r.content_revision_uuid)",
                [entity],
            )?;
        }
    }
    Ok(())
}

fn materialize_scalar_projection(
    transaction: &Transaction<'_>,
    entity: &[u8],
    field: &str,
    value: &serde_json::Value,
) -> anyhow::Result<()> {
    let file_id: Option<i64> = transaction
        .query_row(
            "SELECT id FROM files WHERE asset_uuid=?1",
            [entity],
            |row| row.get(0),
        )
        .optional()?;
    if file_id.is_none() {
        let tag_id: Option<i64> = transaction
            .query_row("SELECT id FROM tags WHERE tag_uuid=?1", [entity], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(tag_id) = tag_id {
            match field {
                "name" => {
                    if let Some(value) = value.as_str() {
                        transaction.execute(
                            "UPDATE tags SET name=?1 WHERE id=?2",
                            params![value, tag_id],
                        )?;
                    }
                }
                "color" => {
                    transaction.execute(
                        "UPDATE tags SET color=?1 WHERE id=?2",
                        params![value.as_str(), tag_id],
                    )?;
                }
                _ => {}
            }
        }
        return Ok(());
    }
    let file_id = file_id.expect("checked above");
    let now = unix_timestamp_millis() / 1000;
    transaction.execute(
        "INSERT OR IGNORE INTO user_metadata(file_id,rating,favorite,note,updated_at) VALUES (?1,NULL,0,NULL,?2)",
        params![file_id,now],
    )?;
    match field {
        "rating" => {
            transaction.execute(
                "UPDATE user_metadata SET rating=?1,updated_at=?2 WHERE file_id=?3",
                params![value.as_i64(), now, file_id],
            )?;
        }
        "favorite" => {
            transaction.execute(
                "UPDATE user_metadata SET favorite=?1,updated_at=?2 WHERE file_id=?3",
                params![value.as_bool().unwrap_or(false) as i64, now, file_id],
            )?;
        }
        "note" => {
            transaction.execute(
                "UPDATE user_metadata SET note=?1,updated_at=?2 WHERE file_id=?3",
                params![value.as_str(), now, file_id],
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn materialize_tag_projection(transaction: &Transaction<'_>, entity: &[u8]) -> anyhow::Result<()> {
    let file_id: Option<i64> = transaction
        .query_row(
            "SELECT id FROM files WHERE asset_uuid=?1",
            [entity],
            |row| row.get(0),
        )
        .optional()?;
    let Some(file_id) = file_id else {
        return Ok(());
    };
    transaction.execute("DELETE FROM file_tags WHERE file_id=?1", [file_id])?;
    transaction.execute(
        "INSERT OR IGNORE INTO file_tags(file_id,tag_id) SELECT ?1,t.id FROM sync_set_adds a JOIN tags t ON t.tag_uuid=a.member_uuid WHERE a.owner_uuid=?2 AND NOT EXISTS (SELECT 1 FROM sync_set_removals r WHERE r.observed_add_event_uuid=a.add_event_uuid)",
        params![file_id,entity],
    )?;
    Ok(())
}

fn rebuild_location_heads(transaction: &Transaction<'_>, entity: &[u8]) -> anyhow::Result<()> {
    transaction.execute(
        "DELETE FROM sync_location_heads WHERE entity_uuid=?1",
        [entity],
    )?;
    transaction.execute(
        "INSERT INTO sync_location_heads(entity_uuid,revision_uuid,is_display_winner) SELECT r.entity_uuid,r.revision_uuid,CASE WHEN r.revision_uuid=(SELECT rr.revision_uuid FROM sync_location_revisions rr JOIN sync_events e ON e.event_uuid=rr.event_uuid WHERE rr.entity_uuid=?1 AND NOT EXISTS (SELECT 1 FROM sync_location_supersedes s WHERE s.observed_revision_uuid=rr.revision_uuid) ORDER BY e.hlc_wall_ms DESC,e.hlc_logical DESC,e.device_uuid DESC,e.actor_epoch DESC,e.actor_seq DESC,e.event_uuid DESC LIMIT 1) THEN 1 ELSE 0 END FROM sync_location_revisions r WHERE r.entity_uuid=?1 AND NOT EXISTS (SELECT 1 FROM sync_location_supersedes s WHERE s.observed_revision_uuid=r.revision_uuid)",
        [entity],
    )?;
    Ok(())
}

pub(crate) fn upsert_frontier_state(
    transaction: &Transaction<'_>,
    actor: &super::types::ActorId,
    through: i64,
    state: &str,
) -> anyhow::Result<()> {
    transaction.execute(
        "INSERT INTO sync_actor_frontiers(device_uuid,stream_uuid,actor_epoch,applied_through_seq,expected_next_seq,state,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(device_uuid,stream_uuid,actor_epoch) DO UPDATE SET applied_through_seq=excluded.applied_through_seq,expected_next_seq=excluded.expected_next_seq,state=excluded.state,updated_at=excluded.updated_at",
        params![actor.device_uuid.0.as_bytes().as_slice(),actor.stream_uuid.0.as_bytes().as_slice(),actor.actor_epoch.0.as_bytes().as_slice(),through,through + 1,state,unix_timestamp_millis()],
    )?;
    Ok(())
}

pub(crate) fn operation_name(operation: &super::types::Operation) -> &'static str {
    use super::types::Operation;
    match operation {
        Operation::SetField { .. } => "set-field",
        Operation::SetAdd { .. } => "set-add",
        Operation::SetRemove { .. } => "set-remove",
        Operation::SetLocation { .. } => "set-location",
        Operation::Delete { .. } => "delete",
        Operation::Restore { .. } => "restore",
        Operation::SetContent { .. } => "set-content",
    }
}

pub(crate) fn stream_kind(kind: super::types::StreamKind) -> &'static str {
    match kind {
        super::types::StreamKind::Catalog => "catalog",
        super::types::StreamKind::HostMetadata => "host-metadata",
    }
}

fn merge_received_hlc(
    transaction: &Transaction<'_>,
    remote: super::types::Hlc,
) -> anyhow::Result<()> {
    let local: Option<(i64, i64)> = transaction
        .query_row(
            "SELECT hlc_wall_ms,hlc_logical FROM sync_local_state WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((wall, logical)) = local else {
        return Ok(());
    };
    let merged = super::types::Hlc {
        wall_ms: wall.max(0) as u64,
        logical: logical.max(0) as u32,
    }
    .merge(remote, unix_timestamp_millis().max(0) as u64);
    transaction.execute(
        "UPDATE sync_local_state SET hlc_wall_ms=?1,hlc_logical=?2,updated_at=?3 WHERE singleton=1",
        params![
            merged.wall_ms as i64,
            merged.logical as i64,
            unix_timestamp_millis()
        ],
    )?;
    Ok(())
}

fn unix_timestamp_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActorBlock {
    Gap {
        expected: u64,
        available: u64,
    },
    Fork {
        sequence: u64,
    },
    Corrupt(String),
    Compatibility {
        wire_version: u16,
        reducer_version: u16,
    },
}

/// Provider-neutral durable-intake model. The real provider writes verified
/// bytes to sync-spool before calling this boundary; this structure owns actor
/// ordering and bounded apply selection only.
#[derive(Default)]
pub struct Inbox {
    frontiers: HashMap<ActorId, u64>,
    segments: HashMap<ActorId, BTreeMap<u64, SealedSegment>>,
    blocks: HashMap<ActorId, ActorBlock>,
}

impl Inbox {
    pub fn intake(&mut self, segment: SealedSegment) -> anyhow::Result<bool> {
        verify_segment(&segment, segment.header.predecessor_hash)
            .context("segment verification failed")?;
        let actor = segment.header.actor.clone();
        let by_first = self.segments.entry(actor.clone()).or_default();
        if let Some(existing) = by_first.get(&segment.header.first_seq) {
            if existing.header.payload_hash == segment.header.payload_hash {
                return Ok(false);
            }
            self.blocks.insert(
                actor,
                ActorBlock::Fork {
                    sequence: segment.header.first_seq,
                },
            );
            anyhow::bail!("actor fork at sequence {}", segment.header.first_seq);
        }
        by_first.insert(segment.header.first_seq, segment);
        Ok(true)
    }

    pub fn next_batch(&mut self, actor: &ActorId) -> Option<Vec<super::types::EventEnvelope>> {
        if self.blocks.contains_key(actor) {
            return None;
        }
        let expected = self.frontiers.get(actor).copied().unwrap_or(0) + 1;
        let segments = self.segments.get(actor)?;
        let Some(segment) = segments.get(&expected) else {
            if let Some((&available, _)) = segments.range(expected..).next() {
                self.blocks.insert(
                    actor.clone(),
                    ActorBlock::Gap {
                        expected,
                        available,
                    },
                );
            }
            return None;
        };
        let mut bytes = 0usize;
        let mut result = Vec::new();
        for event in &segment.events {
            let encoded = serde_json::to_vec(event).ok()?;
            if !result.is_empty()
                && (result.len() == MAX_APPLY_EVENTS
                    || bytes.saturating_add(encoded.len()) > MAX_APPLY_DECODED_BYTES)
            {
                break;
            }
            if encoded.len() > MAX_APPLY_DECODED_BYTES {
                self.blocks.insert(
                    actor.clone(),
                    ActorBlock::Corrupt("single event exceeds decoded limit".into()),
                );
                return None;
            }
            bytes += encoded.len();
            result.push(event.clone());
        }
        Some(result)
    }

    pub fn commit_applied(&mut self, actor: &ActorId, last_seq: u64) -> anyhow::Result<()> {
        let expected = self.frontiers.get(actor).copied().unwrap_or(0) + 1;
        if last_seq < expected {
            return Ok(());
        }
        let segment = self
            .segments
            .get(actor)
            .and_then(|segments| segments.get(&expected))
            .ok_or_else(|| anyhow::anyhow!("frontier cannot advance across a gap"))?;
        if last_seq > segment.header.last_seq {
            anyhow::bail!("frontier cannot advance beyond verified segment");
        }
        self.frontiers.insert(actor.clone(), last_seq);
        self.blocks.remove(actor);
        Ok(())
    }

    pub fn block(&self, actor: &ActorId) -> Option<&ActorBlock> {
        self.blocks.get(actor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::segment::seal_segment;
    use crate::sync::test_model::event;
    use crate::sync::types::{EventUuid, Operation};

    #[test]
    fn gap_blocks_only_the_affected_actor_and_exact_duplicates_are_noop() {
        let a1 = event(
            1,
            10,
            Operation::Delete {
                token: EventUuid::new(),
            },
        );
        let a2 = event(
            2,
            10,
            Operation::Delete {
                token: EventUuid::new(),
            },
        );
        let mut b1 = event(
            1,
            20,
            Operation::Delete {
                token: EventUuid::new(),
            },
        );
        b1.actor.device_uuid = crate::sync::types::DeviceUuid::new();
        b1 = crate::sync::codec::seal_event(b1).unwrap();
        let mut inbox = Inbox::default();
        let a_actor = a1.actor.clone();
        let b_actor = b1.actor.clone();
        let gap = seal_segment(vec![a2], None).unwrap();
        assert!(inbox.intake(gap).unwrap());
        assert!(inbox.next_batch(&a_actor).is_none());
        assert!(matches!(
            inbox.block(&a_actor),
            Some(ActorBlock::Gap { .. })
        ));
        let b = seal_segment(vec![b1], None).unwrap();
        assert!(inbox.intake(b.clone()).unwrap());
        assert!(!inbox.intake(b).unwrap());
        assert_eq!(inbox.next_batch(&b_actor).unwrap().len(), 1);
        let a = seal_segment(vec![a1], None).unwrap();
        assert!(inbox.intake(a).unwrap());
        inbox.blocks.remove(&a_actor);
        assert_eq!(inbox.next_batch(&a_actor).unwrap()[0].actor_seq, 1);
    }
}
