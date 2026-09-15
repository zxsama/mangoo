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
        if event.wire_version > super::codec::WIRE_VERSION
            || event.envelope_version > super::codec::ENVELOPE_VERSION
            || event.reducer_version > super::codec::REDUCER_VERSION
        {
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
        materialize_truth(transaction, event, true)?;
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
    remote_apply: bool,
) -> anyhow::Result<()> {
    use super::types::Operation;
    let entity = event.entity_uuid.0.as_bytes().as_slice();
    let event_uuid = event.event_uuid.0.as_bytes().as_slice();
    match &event.operation {
        Operation::SetField { field, value } => {
            if field == super::reducer::ROOT_IDENTITY_FIELD {
                let root = super::reducer::decode_root_identity(event.entity_uuid, value)?;
                let existing: Option<(Vec<u8>,Vec<u8>)> = transaction.query_row("SELECT root_folder_uuid,library_root_uuid FROM sync_root_descriptors WHERE root_uuid=?1",[root.root_uuid.0.as_bytes().as_slice()],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
                if existing.as_ref().is_some_and(|(folder, parent)| {
                    folder.as_slice() != root.root_folder_uuid.0.as_bytes()
                        || parent.as_slice() != root.library_root_uuid.0.as_bytes()
                }) {
                    anyhow::bail!("synchronized root identity conflict");
                }
                transaction.execute("INSERT INTO sync_root_descriptors(root_uuid,root_folder_uuid,library_root_uuid,creation_event_uuid,winning_location_revision_uuid,requested_name,materialized_name,materialization_fingerprint,lifecycle,logical_revision,updated_at) VALUES (?1,?2,?3,?4,NULL,?5,?6,?7,?8,?9,?10) ON CONFLICT(root_uuid) DO UPDATE SET creation_event_uuid=excluded.creation_event_uuid,logical_revision=sync_root_descriptors.logical_revision+1 WHERE sync_root_descriptors.creation_event_uuid IS NULL",params![root.root_uuid.0.as_bytes().as_slice(),entity,root.library_root_uuid.0.as_bytes().as_slice(),event_uuid,root.requested_name,root.materialized_name,root.materialization_fingerprint.as_slice(),root.lifecycle,i64::try_from(root.logical_revision)?.checked_add(1).context("root revision overflow")?,unix_timestamp_millis()])?;
            }
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
            let preserve_local_fork =
                remote_apply && entity_is_in_local_only_root(transaction, entity)?;
            if !preserve_local_fork {
                materialize_scalar_projection(transaction, entity, field, &winning_value)?;
            }
            if field == "entity-kind" {
                rematerialize_location_projection(transaction, remote_apply)?;
            }
        }
        Operation::SetAdd { member_uuid, .. } => {
            transaction.execute(
                "INSERT OR IGNORE INTO sync_set_adds(owner_uuid,member_uuid,add_event_uuid) VALUES (?1,?2,?3)",
                params![entity,member_uuid.0.as_bytes().as_slice(),event_uuid],
            )?;
            if !(remote_apply && entity_is_in_local_only_root(transaction, entity)?) {
                materialize_tag_projection(transaction, entity)?;
            }
        }
        Operation::SetRemove { observed_adds, .. } => {
            for observed in observed_adds {
                transaction.execute(
                    "INSERT OR IGNORE INTO sync_set_removals(remove_event_uuid,observed_add_event_uuid) VALUES (?1,?2)",
                    params![event_uuid,observed.0.as_bytes().as_slice()],
                )?;
            }
            if !(remote_apply && entity_is_in_local_only_root(transaction, entity)?) {
                materialize_tag_projection(transaction, entity)?;
            }
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
            rematerialize_location_projection(transaction, remote_apply)?;
        }
        Operation::Delete { token } => {
            transaction.execute(
                "INSERT OR IGNORE INTO sync_tombstone_tokens(token_uuid,entity_uuid,event_uuid,local_mutation_generation,created_at) VALUES (?1,?2,?3,1,?4)",
                params![token.0.as_bytes().as_slice(),entity,event_uuid,unix_timestamp_millis()],
            )?;
            let preserve_local_fork =
                remote_apply && entity_is_in_local_only_root(transaction, entity)?;
            if !preserve_local_fork {
                transaction.execute(
                    "UPDATE files SET status='trashed' WHERE asset_uuid=?1",
                    [entity],
                )?;
                transaction.execute(
                    "UPDATE folders SET status='trashed' WHERE folder_uuid=?1 AND parent_id IS NOT NULL",
                    [entity],
                )?;
            }
            transaction.execute(
                "UPDATE sync_root_descriptors SET lifecycle='tombstoned',logical_revision=logical_revision+1,updated_at=?1 WHERE root_folder_uuid=?2",
                params![unix_timestamp_millis(), entity],
            )?;
        }
        Operation::Restore { observed_tokens } => {
            for observed in observed_tokens {
                transaction.execute(
                    "INSERT OR IGNORE INTO sync_tombstone_removals(remove_event_uuid,observed_token_uuid) VALUES (?1,?2)",
                    params![event_uuid,observed.0.as_bytes().as_slice()],
                )?;
            }
            let live_tokens: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM sync_tombstone_tokens t WHERE t.entity_uuid=?1 AND NOT EXISTS (SELECT 1 FROM sync_tombstone_removals r WHERE r.observed_token_uuid=t.token_uuid)",
                [entity],
                |row| row.get(0),
            )?;
            let preserve_local_fork =
                remote_apply && entity_is_in_local_only_root(transaction, entity)?;
            if !preserve_local_fork {
                transaction.execute(
                    "UPDATE files SET status=CASE WHEN ?1=0 THEN 'active' ELSE 'trashed' END WHERE asset_uuid=?2",
                    params![live_tokens, entity],
                )?;
                transaction.execute(
                    "UPDATE folders SET status=CASE WHEN ?1=0 THEN 'active' ELSE 'trashed' END WHERE folder_uuid=?2 AND parent_id IS NOT NULL",
                    params![live_tokens, entity],
                )?;
            }
            if live_tokens == 0 {
                transaction.execute(
                    "UPDATE sync_root_descriptors SET lifecycle='active',logical_revision=logical_revision+1,updated_at=?1 WHERE root_folder_uuid=?2",
                    params![unix_timestamp_millis(), entity],
                )?;
            }
            rematerialize_location_projection(transaction, remote_apply)?;
        }
        Operation::SetContent {
            revision_uuid,
            manifest_id,
            supersedes,
            manifest_pack_id,
            manifest_offset,
            manifest_length,
            byte_length,
            source_mtime_ms,
        } => {
            let manifest_location = match (
                manifest_pack_id.as_deref(),
                *manifest_offset,
                *manifest_length,
                *byte_length,
            ) {
                (Some(pack), Some(offset), Some(length), Some(bytes)) => Some((
                    crate::sync::media::parse_hash(manifest_id)?,
                    crate::sync::media::parse_hash(pack)?,
                    i64::try_from(offset)?,
                    i64::try_from(length)?,
                    i64::try_from(bytes)?,
                )),
                (None, None, None, None) => None,
                _ => anyhow::bail!("content manifest locator is incomplete"),
            };
            let manifest_hash = manifest_location
                .map(|value| value.0)
                .unwrap_or_else(|| super::codec::sha256(manifest_id.as_bytes()));
            let changed = transaction.execute(
                "INSERT INTO asset_content_revisions(content_revision_uuid,asset_uuid,event_uuid,manifest_hash,byte_size,local_mutation_generation,source_mtime_ms,created_at) VALUES (?1,?2,?3,?4,?5,1,?6,?7) ON CONFLICT(content_revision_uuid) DO UPDATE SET event_uuid=excluded.event_uuid,manifest_hash=excluded.manifest_hash,byte_size=excluded.byte_size,source_mtime_ms=excluded.source_mtime_ms",
                params![revision_uuid.0.as_bytes().as_slice(),entity,event_uuid,manifest_hash.as_slice(),manifest_location.map(|value|value.4),source_mtime_ms,unix_timestamp_millis()],
            )?;
            if changed != 1 {
                anyhow::bail!("content revision did not materialize");
            }
            if let Some((_, pack_hash, offset, length, bytes)) = manifest_location {
                transaction.execute(
                    "INSERT INTO media_manifests(manifest_hash,content_hash,byte_length,manifest_pack_hash,pack_offset,record_length,state,created_at,updated_at) VALUES (?1,zeroblob(32),?2,?3,?4,?5,'remote',?6,?6) ON CONFLICT(manifest_hash) DO UPDATE SET byte_length=excluded.byte_length,manifest_pack_hash=excluded.manifest_pack_hash,pack_offset=excluded.pack_offset,record_length=excluded.record_length,state='remote',updated_at=excluded.updated_at",
                    params![manifest_hash.as_slice(),bytes,pack_hash.as_slice(),offset,length,unix_timestamp_millis()],
                )?;
            } else {
                transaction.execute(
                    "INSERT OR IGNORE INTO media_manifests(manifest_hash,content_hash,byte_length,state,created_at,updated_at) VALUES (?1,zeroblob(32),0,'remote',?2,?2)",
                    params![manifest_hash.as_slice(),unix_timestamp_millis()],
                )?;
            }
            transaction.execute(
                "INSERT OR IGNORE INTO media_content_manifest_links(content_revision_uuid,manifest_hash,linked_at) VALUES (?1,?2,?3)",
                params![revision_uuid.0.as_bytes().as_slice(),manifest_hash.as_slice(),unix_timestamp_millis()],
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
        let mut tag_id: Option<i64> = transaction
            .query_row("SELECT id FROM tags WHERE tag_uuid=?1", [entity], |row| {
                row.get(0)
            })
            .optional()?;
        if tag_id.is_none() && field == "name" {
            if let Some(name) = value.as_str() {
                transaction.execute(
                    "INSERT OR IGNORE INTO tags(name,color,tag_uuid) VALUES (?1,NULL,?2)",
                    params![name, entity],
                )?;
                tag_id = transaction
                    .query_row("SELECT id FROM tags WHERE tag_uuid=?1", [entity], |row| {
                        row.get(0)
                    })
                    .optional()?;
            }
        }
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
    match field {
        "rating" => {
            transaction.execute(
                "INSERT OR IGNORE INTO user_metadata(file_id,rating,favorite,note,updated_at) VALUES (?1,NULL,0,NULL,?2)",
                params![file_id,now],
            )?;
            transaction.execute(
                "UPDATE user_metadata SET rating=?1,updated_at=?2 WHERE file_id=?3",
                params![value.as_i64(), now, file_id],
            )?;
        }
        "favorite" => {
            transaction.execute(
                "INSERT OR IGNORE INTO user_metadata(file_id,rating,favorite,note,updated_at) VALUES (?1,NULL,0,NULL,?2)",
                params![file_id,now],
            )?;
            transaction.execute(
                "UPDATE user_metadata SET favorite=?1,updated_at=?2 WHERE file_id=?3",
                params![value.as_bool().unwrap_or(false) as i64, now, file_id],
            )?;
        }
        "note" => {
            transaction.execute(
                "INSERT OR IGNORE INTO user_metadata(file_id,rating,favorite,note,updated_at) VALUES (?1,NULL,0,NULL,?2)",
                params![file_id,now],
            )?;
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

// `r` is a local location row. An exact canonical location permanently replaces
// its bootstrap placeholder, even after a later canonical rename supersedes it.
// Both immutable revisions remain; independent canonical heads never collapse.
const REDUNDANT_BOOTSTRAP_LOCATION: &str = "r.event_uuid IS NULL AND r.origin='bootstrap' AND EXISTS(SELECT 1 FROM sync_location_revisions canonical WHERE canonical.entity_uuid=r.entity_uuid AND canonical.event_uuid IS NOT NULL AND canonical.parent_uuid IS r.parent_uuid AND canonical.name=r.name)";

pub(crate) fn seed_bootstrap_location_heads(transaction: &Transaction<'_>) -> anyhow::Result<()> {
    transaction.execute(
        &format!("INSERT OR IGNORE INTO sync_location_heads(entity_uuid,revision_uuid,is_display_winner) SELECT r.entity_uuid,r.revision_uuid,1 FROM sync_location_revisions r WHERE r.origin='bootstrap' AND NOT ({REDUNDANT_BOOTSTRAP_LOCATION})"),
        [],
    )?;
    Ok(())
}

/// Repair derived conflicts left by older capture code without rewriting any
/// canonical events or bootstrap evidence. The open-conflict index bounds the
/// candidate set, and only matching shadows consume the per-turn work budget.
pub(crate) fn reconcile_bootstrap_location_conflicts(
    transaction: &Transaction<'_>,
) -> anyhow::Result<()> {
    let mut statement = transaction.prepare(&format!(
        "SELECT DISTINCT c.entity_uuid FROM sync_conflicts c WHERE c.state='open' AND c.conflict_kind='location-heads' AND EXISTS(SELECT 1 FROM sync_location_heads h JOIN sync_location_revisions r ON r.revision_uuid=h.revision_uuid WHERE h.entity_uuid=c.entity_uuid AND ({REDUNDANT_BOOTSTRAP_LOCATION})) LIMIT 100"
    ))?;
    let entities = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for entity in entities {
        rebuild_location_heads(transaction, &entity)?;
        let mut statement = transaction.prepare(
            "SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1 ORDER BY revision_uuid"
        )?;
        let heads = statement
            .query_map([&entity], |row| row.get::<_, Vec<u8>>(0))?
            .map(|row| Ok(super::types::RevisionUuid(uuid::Uuid::from_slice(&row?)?)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        // Retire this entity's obsolete derived head-set identity even when a
        // real multi-canonical conflict remains after removing the shadow.
        let conflict_uuid = deterministic_conflict_uuid(
            super::types::EntityUuid(uuid::Uuid::from_slice(&entity)?),
            &heads,
        );
        transaction.execute(
            "UPDATE sync_conflicts SET state='resolved',resolved_at=?1 WHERE entity_uuid=?2 AND conflict_kind='location-heads' AND state='open' AND conflict_uuid<>?3",
            params![unix_timestamp_millis(), entity, conflict_uuid.as_bytes().as_slice()],
        )?;
        persist_location_conflict(
            transaction,
            super::types::EntityUuid(uuid::Uuid::from_slice(&entity)?),
            &heads,
        )?;
    }
    Ok(())
}

fn rebuild_location_heads(transaction: &Transaction<'_>, entity: &[u8]) -> anyhow::Result<()> {
    transaction.execute(
        "DELETE FROM sync_location_heads WHERE entity_uuid=?1",
        [entity],
    )?;
    transaction.execute(
        &format!("INSERT INTO sync_location_heads(entity_uuid,revision_uuid,is_display_winner) SELECT r.entity_uuid,r.revision_uuid,CASE WHEN r.revision_uuid=(SELECT rr.revision_uuid FROM sync_location_revisions rr JOIN sync_events e ON e.event_uuid=rr.event_uuid WHERE rr.entity_uuid=?1 AND NOT EXISTS (SELECT 1 FROM sync_location_supersedes s WHERE s.observed_revision_uuid=rr.revision_uuid) ORDER BY e.hlc_wall_ms DESC,e.hlc_logical DESC,e.device_uuid DESC,e.actor_epoch DESC,e.actor_seq DESC,e.event_uuid DESC LIMIT 1) THEN 1 ELSE 0 END FROM sync_location_revisions r WHERE r.entity_uuid=?1 AND NOT EXISTS (SELECT 1 FROM sync_location_supersedes s WHERE s.observed_revision_uuid=r.revision_uuid) AND NOT ({REDUNDANT_BOOTSTRAP_LOCATION})"),
        [entity],
    )?;
    Ok(())
}

fn rematerialize_location_projection(
    transaction: &Transaction<'_>,
    preserve_local_only_roots: bool,
) -> anyhow::Result<()> {
    use super::reducer::{legalize_locations, LocationRevision, ProtocolState};
    use super::types::{ActorEpochUuid, DeviceUuid, EventUuid, Hlc, RevisionUuid, Version};

    let library_root: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT library_root_uuid FROM sync_root_descriptors ORDER BY library_root_uuid LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(library_root) = library_root else {
        return materialize_legacy_location_projection(transaction);
    };
    let logical_root = super::types::EntityUuid(uuid::Uuid::from_slice(&library_root)?);
    let mut state = ProtocolState::default();
    // A browse root bootstrapped before enrollment has an empty-name sentinel
    // location. Interpret only that local placeholder through its descriptor;
    // retain the anchor for children and leave immutable revisions untouched.
    let mut statement = transaction.prepare(
        "SELECT r.revision_uuid,r.entity_uuid,CASE WHEN d.root_uuid IS NOT NULL THEN d.library_root_uuid ELSE r.parent_uuid END,CASE WHEN d.root_uuid IS NOT NULL THEN d.requested_name ELSE r.name END,COALESCE(e.hlc_wall_ms,0),COALESCE(e.hlc_logical,0),COALESCE(e.device_uuid,zeroblob(16)),COALESCE(e.actor_epoch,zeroblob(16)),COALESCE(e.actor_seq,0),COALESCE(e.event_uuid,r.revision_uuid),EXISTS(SELECT 1 FROM sync_location_heads h WHERE h.entity_uuid=r.entity_uuid AND h.revision_uuid=r.revision_uuid) FROM sync_location_revisions r LEFT JOIN sync_events e ON e.event_uuid=r.event_uuid LEFT JOIN sync_root_descriptors d ON d.root_folder_uuid=r.entity_uuid AND r.event_uuid IS NULL AND r.origin='bootstrap' AND r.name='' ORDER BY r.entity_uuid,r.revision_uuid",
    )?;
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, Option<Vec<u8>>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, Vec<u8>>(6)?,
            row.get::<_, Vec<u8>>(7)?,
            row.get::<_, i64>(8)?,
            row.get::<_, Vec<u8>>(9)?,
            row.get::<_, bool>(10)?,
        ))
    })? {
        let (revision, entity, parent, name, wall, logical, device, epoch, seq, event, is_head) =
            row?;
        let entity_uuid = super::types::EntityUuid(uuid::Uuid::from_slice(&entity)?);
        let revision_uuid = RevisionUuid(uuid::Uuid::from_slice(&revision)?);
        let truth = state.entities.entry(entity_uuid).or_default();
        truth.locations.insert(
            revision_uuid,
            LocationRevision {
                revision_uuid,
                parent_uuid: parent
                    .map(|value| uuid::Uuid::from_slice(&value).map(super::types::EntityUuid))
                    .transpose()?,
                name,
                version: Version {
                    hlc: Hlc {
                        wall_ms: wall.max(0) as u64,
                        logical: logical.max(0) as u32,
                    },
                    device_uuid: DeviceUuid(uuid::Uuid::from_slice(&device)?),
                    actor_epoch: ActorEpochUuid(uuid::Uuid::from_slice(&epoch)?),
                    actor_seq: seq.max(0) as u64,
                    event_uuid: EventUuid(uuid::Uuid::from_slice(&event)?),
                },
            },
        );
        if is_head {
            truth.location_heads.insert(revision_uuid);
        }
    }
    let locations = legalize_locations(&state, logical_root);
    for (entity_uuid, location) in locations {
        let entity = entity_uuid.0.as_bytes().as_slice();
        let heads = state.entities[&entity_uuid]
            .location_heads
            .iter()
            .copied()
            .collect::<Vec<_>>();
        persist_location_conflict(transaction, entity_uuid, &heads)?;
        let root_descriptor: Option<(Vec<u8>, i64)> = transaction
            .query_row(
                "SELECT root_uuid,logical_revision FROM sync_root_descriptors WHERE root_folder_uuid=?1",
                [entity],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((root_uuid, old_revision)) = root_descriptor {
            let winning = state.entities[&entity_uuid].winning_location();
            let winning_revision = winning.map(|value| value.revision_uuid.0.as_bytes().to_vec());
            transaction.execute(
                "UPDATE sync_root_descriptors SET winning_location_revision_uuid=?1,requested_name=?2,materialized_name=?3,materialization_fingerprint=?4,logical_revision=?5,updated_at=?6 WHERE root_folder_uuid=?7 AND (NOT (winning_location_revision_uuid IS ?1) OR requested_name<>?2 OR materialized_name<>?3 OR materialization_fingerprint<>?4)",
                params![winning_revision, location.requested_name, location.display_name, location.materialization_fingerprint.as_slice(), old_revision.saturating_add(1), unix_timestamp_millis(), entity],
            )?;
            let local_only = root_is_local_only(transaction, &root_uuid)?;
            if !local_only || !preserve_local_only_roots {
                transaction.execute(
                    "UPDATE roots SET display_name=?1 WHERE root_uuid=?2",
                    params![location.display_name, root_uuid],
                )?;
            }
            continue;
        }
        let parent = location.parent_uuid.and_then(|parent_uuid| {
            transaction
                .query_row(
                    "SELECT id,root_id FROM folders WHERE folder_uuid=?1",
                    [parent_uuid.0.as_bytes().as_slice()],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .ok()
                .flatten()
        });
        let Some((parent_id, root_id)) = parent else {
            continue;
        };
        let local_only: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_root_local_bindings WHERE root_id=?1 AND state='local-only-fork')",
            [root_id],
            |row| row.get(0),
        )?;
        if local_only && preserve_local_only_roots {
            continue;
        }
        if let Some(file_id) = transaction
            .query_row(
                "SELECT id FROM files WHERE asset_uuid=?1",
                [entity],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            transaction.execute(
                "UPDATE files SET root_id=?1,folder_id=?2,name=?3 WHERE id=?4",
                params![root_id, parent_id, location.display_name, file_id],
            )?;
            crate::db::sync_media_fts_for_file_in_transaction_pub(transaction, file_id)?;
            continue;
        }
        let entity_kind: Option<Vec<u8>> = transaction
            .query_row(
                "SELECT value FROM sync_field_versions WHERE entity_uuid=?1 AND field_name='entity-kind' AND is_winner=1",
                [entity],
                |row| row.get(0),
            )
            .optional()?;
        let is_folder = entity_kind
            .as_deref()
            .and_then(|value| serde_json::from_slice::<serde_json::Value>(value).ok())
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .is_some_and(|kind| kind == "folder");
        if is_folder {
            let updated = transaction.execute(
                "UPDATE folders SET root_id=?1,parent_id=?2,name=?3,status='active' WHERE folder_uuid=?4",
                params![root_id, parent_id, location.display_name, entity],
            )?;
            if updated == 0 {
                transaction.execute(
                    "INSERT INTO folders(root_id,parent_id,name,path_hash,mtime,status,folder_uuid) VALUES (?1,?2,?3,'',NULL,'active',?4)",
                    params![root_id, parent_id, location.display_name, entity],
                )?;
            }
        }
    }
    Ok(())
}

fn root_is_local_only(transaction: &Transaction<'_>, root_uuid: &[u8]) -> anyhow::Result<bool> {
    transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_root_local_bindings WHERE root_uuid=?1 AND state='local-only-fork')",
            [root_uuid],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn entity_is_in_local_only_root(
    transaction: &Transaction<'_>,
    entity: &[u8],
) -> anyhow::Result<bool> {
    let directly_bound: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_root_local_bindings b WHERE b.state='local-only-fork' AND (b.root_id IN (SELECT root_id FROM files WHERE asset_uuid=?1) OR b.root_id IN (SELECT root_id FROM folders WHERE folder_uuid=?1) OR b.root_uuid IN (SELECT root_uuid FROM sync_root_descriptors WHERE root_folder_uuid=?1)))",
        [entity],
        |row| row.get(0),
    )?;
    if directly_bound {
        return Ok(true);
    }

    let mut cursor = entity.to_vec();
    let mut visited = std::collections::BTreeSet::new();
    for _ in 0..64 {
        if !visited.insert(cursor.clone()) {
            break;
        }
        let root_is_local_only: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_root_descriptors d JOIN sync_root_local_bindings b ON b.root_uuid=d.root_uuid WHERE d.root_folder_uuid=?1 AND b.state='local-only-fork')",
            [&cursor],
            |row| row.get(0),
        )?;
        if root_is_local_only {
            return Ok(true);
        }
        let parent: Option<Vec<u8>> = transaction
            .query_row(
                "SELECT r.parent_uuid FROM sync_location_heads h JOIN sync_location_revisions r ON r.revision_uuid=h.revision_uuid WHERE h.entity_uuid=?1 AND h.is_display_winner=1",
                [&cursor],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let Some(parent) = parent else {
            break;
        };
        cursor = parent;
    }
    Ok(false)
}

fn materialize_legacy_location_projection(transaction: &Transaction<'_>) -> anyhow::Result<()> {
    let mut statement = transaction.prepare(
        "SELECT h.entity_uuid,r.parent_uuid,r.name FROM sync_location_heads h JOIN sync_location_revisions r ON r.revision_uuid=h.revision_uuid WHERE h.is_display_winner=1",
    )?;
    let winners = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for (entity, parent, name) in winners {
        let Some(parent) = parent else { continue };
        let parent_id: Option<i64> = transaction
            .query_row(
                "SELECT id FROM folders WHERE folder_uuid=?1",
                [&parent],
                |row| row.get(0),
            )
            .optional()?;
        let Some(parent_id) = parent_id else { continue };
        if let Some(file_id) = transaction
            .query_row(
                "SELECT id FROM files WHERE asset_uuid=?1",
                [&entity],
                |row| row.get(0),
            )
            .optional()?
        {
            transaction.execute(
                "UPDATE files SET folder_id=?1,name=?2 WHERE id=?3",
                params![parent_id, name, file_id],
            )?;
            crate::db::sync_media_fts_for_file_in_transaction_pub(transaction, file_id)?;
        }
    }
    Ok(())
}

fn persist_location_conflict(
    transaction: &Transaction<'_>,
    entity_uuid: super::types::EntityUuid,
    heads: &[super::types::RevisionUuid],
) -> anyhow::Result<()> {
    let conflict_uuid = deterministic_conflict_uuid(entity_uuid, heads);
    if heads.len() > 1 {
        let details = serde_json::to_vec(&serde_json::json!({
            "locationHeads": heads.iter().map(ToString::to_string).collect::<Vec<_>>()
        }))?;
        transaction.execute(
            "INSERT INTO sync_conflicts(conflict_uuid,entity_uuid,conflict_kind,state,details,created_at) VALUES (?1,?2,'location-heads','open',?3,?4) ON CONFLICT(conflict_uuid) DO UPDATE SET state='open',details=excluded.details,resolved_at=NULL",
            params![conflict_uuid.as_bytes().as_slice(), entity_uuid.0.as_bytes().as_slice(), details, unix_timestamp_millis()],
        )?;
    } else {
        transaction.execute(
            "UPDATE sync_conflicts SET state='resolved',resolved_at=?1 WHERE entity_uuid=?2 AND conflict_kind='location-heads' AND state='open'",
            params![unix_timestamp_millis(), entity_uuid.0.as_bytes().as_slice()],
        )?;
    }
    Ok(())
}

fn deterministic_conflict_uuid(
    entity_uuid: super::types::EntityUuid,
    heads: &[super::types::RevisionUuid],
) -> uuid::Uuid {
    let mut material = Vec::with_capacity(16 + heads.len() * 16);
    material.extend_from_slice(entity_uuid.0.as_bytes());
    for head in heads {
        material.extend_from_slice(head.0.as_bytes());
    }
    let mut bytes: [u8; 16] = super::codec::sha256(&material)[..16]
        .try_into()
        .expect("uuid bytes");
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes)
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
    use crate::sync::types::{EntityUuid, EventUuid, Operation, RevisionUuid};

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

    #[test]
    fn content_event_carries_immediate_manifest_restore_location() {
        let database = crate::db::Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        database
            .add_root(crate::db::NewRoot {
                path: "D:/Remote".into(),
                display_name: "Remote".into(),
            })
            .unwrap();
        let revision = crate::sync::types::RevisionUuid(uuid::Uuid::now_v7());
        let event = event(
            1,
            10,
            Operation::SetContent {
                revision_uuid: revision,
                manifest_id: crate::sync::media::hex_hash(&[7; 32]),
                supersedes: Default::default(),
                manifest_pack_id: Some(crate::sync::media::hex_hash(&[8; 32])),
                manifest_offset: Some(12),
                manifest_length: Some(34),
                byte_length: Some(56),
                source_mtime_ms: Some(78),
            },
        );
        database
            .initialize_sync_identity(
                event.library_uuid.0,
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        let transaction = database
            .connection_for_fsops()
            .unchecked_transaction()
            .unwrap();
        apply_remote_batch(&transaction, &[event]).unwrap();
        transaction.commit().unwrap();
        let restored: (i64, i64, i64, i64, i64) = database
            .connection_for_fsops()
            .query_row(
                "SELECT r.byte_size,r.source_mtime_ms,m.byte_length,m.pack_offset,m.record_length FROM asset_content_revisions r JOIN media_content_manifest_links l ON l.content_revision_uuid=r.content_revision_uuid JOIN media_manifests m ON m.manifest_hash=l.manifest_hash WHERE r.content_revision_uuid=?1",
                [revision.0.as_bytes().as_slice()],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
            )
            .unwrap();
        assert_eq!(restored, (56, 78, 56, 12, 34));
    }

    #[test]
    fn enrolled_legacy_root_sentinels_preserve_names_during_first_capture() {
        let database = crate::db::Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let library_uuid = uuid::Uuid::from_u128(1);
        let library_root_uuid = uuid::Uuid::from_u128(999);
        let mut roots = Vec::new();
        for (index, name) in ["Photos", "Archive"].into_iter().enumerate() {
            let root_id = database
                .add_root(crate::db::NewRoot {
                    path: format!("D:/Legacy-{index}"),
                    display_name: name.into(),
                })
                .unwrap();
            let root_uuid: Vec<u8> = database
                .connection_for_fsops()
                .query_row(
                    "SELECT root_uuid FROM roots WHERE id=?1",
                    [root_id],
                    |row| row.get(0),
                )
                .unwrap();
            let root_uuid = uuid::Uuid::from_slice(&root_uuid).unwrap();
            let sentinel = uuid::Uuid::from_u128(20 + index as u128);
            let child = uuid::Uuid::from_u128(40 + index as u128);
            database.connection_for_fsops().execute(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,status,folder_uuid) VALUES (?1,NULL,'','root','active',?2)",
                params![root_id, sentinel.as_bytes().as_slice()],
            ).unwrap();
            let sentinel_id = database.connection_for_fsops().last_insert_rowid();
            database.connection_for_fsops().execute(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,status,folder_uuid) VALUES (?1,?2,'Nested','child','active',?3)",
                params![root_id, sentinel_id, child.as_bytes().as_slice()],
            ).unwrap();
            roots.push((root_id, root_uuid, sentinel, sentinel_id, child, name));
        }
        // Match an existing browse Library: its empty sentinels were bootstrapped
        // before either root acquired a managed descriptor and active binding.
        database
            .initialize_sync_identity(library_uuid, library_root_uuid, uuid::Uuid::now_v7())
            .unwrap();
        for (root_id, root_uuid, sentinel, _, _, name) in &roots {
            database
                .ensure_selected_sync_root(
                    library_uuid,
                    *root_uuid,
                    *sentinel,
                    library_root_uuid,
                    &format!("D:/Legacy-{root_id}"),
                    name,
                    name,
                    [1; 32],
                    uuid::Uuid::now_v7(),
                    uuid::Uuid::now_v7(),
                    uuid::Uuid::now_v7(),
                    1,
                    "active",
                    1,
                    false,
                )
                .unwrap();
        }
        let legacy_locations = || {
            let mut statement = database.connection_for_fsops().prepare(
                "SELECT r.revision_uuid,r.entity_uuid,r.parent_uuid,r.name,r.origin,r.event_uuid FROM sync_location_revisions r JOIN sync_root_descriptors d ON d.root_folder_uuid=r.entity_uuid WHERE r.origin='bootstrap' ORDER BY r.entity_uuid",
            ).unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<Vec<u8>>>(5)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let before = legacy_locations();
        assert_eq!(before.len(), 2);
        assert!(before.iter().all(|row| row.3.is_empty() && row.5.is_none()));
        let transaction = database
            .connection_for_fsops()
            .unchecked_transaction()
            .unwrap();
        rematerialize_location_projection(&transaction, false).unwrap();
        transaction.commit().unwrap();
        database
            .begin_sync_capture(uuid::Uuid::now_v7(), uuid::Uuid::now_v7())
            .unwrap();
        crate::sync::capture::capture_enrolled_root_identities(database.connection_for_fsops())
            .unwrap();
        assert_eq!(
            legacy_locations(),
            before,
            "projection must preserve the original bootstrap evidence"
        );
        for (root_id, root_uuid, sentinel, sentinel_id, child, name) in &roots {
            let descriptor = database
                .sync_root_descriptor(root_uuid.as_bytes())
                .unwrap()
                .unwrap();
            assert_eq!(descriptor.root_folder_uuid, *sentinel.as_bytes());
            assert_eq!(descriptor.requested_name, *name);
            assert_eq!(descriptor.materialized_name, *name);
            assert!(descriptor.creation_event_uuid.is_some());
            let sentinel_row: (String, Option<i64>) = database
                .connection_for_fsops()
                .query_row(
                    "SELECT name,parent_id FROM folders WHERE folder_uuid=?1",
                    [sentinel.as_bytes().as_slice()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(sentinel_row, (String::new(), None));
            let child_row: (String, i64, i64) = database
                .connection_for_fsops()
                .query_row(
                    "SELECT name,parent_id,root_id FROM folders WHERE folder_uuid=?1",
                    [child.as_bytes().as_slice()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(child_row, ("Nested".into(), *sentinel_id, *root_id));
        }
        let descriptors = database.sync_root_descriptors().unwrap();
        let open_conflicts: i64 = database.connection_for_fsops().query_row(
            "SELECT COUNT(*) FROM sync_conflicts WHERE conflict_kind='location-heads' AND state='open'", [], |row| row.get(0),
        ).unwrap();
        assert_eq!(
            open_conflicts, 0,
            "canonical capture must not conflict with its identical bootstrap location"
        );
        let counts = database.sync_capture_counts_for_test().unwrap();
        crate::sync::capture::capture_enrolled_root_identities(database.connection_for_fsops())
            .unwrap();
        assert_eq!(database.sync_root_descriptors().unwrap(), descriptors);
        assert_eq!(database.sync_capture_counts_for_test().unwrap(), counts);

        // A real location event must still win, and invalid canonical names must
        // not receive the compatibility interpretation reserved for bootstrap.
        let sentinel = roots[0].2;
        let revision = RevisionUuid::new();
        let transaction = database
            .connection_for_fsops()
            .unchecked_transaction()
            .unwrap();
        crate::sync::capture::capture_catalog_mutation(
            &transaction,
            crate::sync::capture::MutationOrigin::LocalApi,
            EntityUuid(sentinel),
            Operation::SetLocation {
                revision_uuid: revision,
                parent_uuid: Some(EntityUuid(library_root_uuid)),
                name: "Renamed".into(),
                supersedes: Default::default(),
            },
        )
        .unwrap();
        transaction.commit().unwrap();
        let renamed = database
            .sync_root_descriptor(roots[0].1.as_bytes())
            .unwrap()
            .unwrap();
        assert_eq!(renamed.requested_name, "Renamed");
        assert_eq!(
            renamed.winning_location_revision_uuid,
            Some(*revision.0.as_bytes())
        );
        let transaction = database
            .connection_for_fsops()
            .unchecked_transaction()
            .unwrap();
        let result = crate::sync::capture::capture_catalog_mutation(
            &transaction,
            crate::sync::capture::MutationOrigin::LocalApi,
            EntityUuid(sentinel),
            Operation::SetLocation {
                revision_uuid: RevisionUuid::new(),
                parent_uuid: Some(EntityUuid(library_root_uuid)),
                name: String::new(),
                supersedes: std::collections::BTreeSet::from([revision]),
            },
        );
        assert!(result.is_err());
        transaction.rollback().unwrap();
        assert_eq!(
            database
                .sync_root_descriptor(roots[0].1.as_bytes())
                .unwrap()
                .unwrap(),
            renamed
        );
        assert_eq!(legacy_locations(), before);
    }

    #[test]
    fn bootstrap_conflict_recovery_is_bounded_preserves_truth_and_survives_reopen() {
        let directory =
            std::env::temp_dir().join(format!("megle-bootstrap-conflict-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("catalog.sqlite");
        let library = uuid::Uuid::from_u128(1);
        let library_root = uuid::Uuid::from_u128(999);
        let device = uuid::Uuid::now_v7();
        let sentinel = uuid::Uuid::from_u128(20);
        let database = crate::db::Database::open(&path).unwrap();
        database.apply_migrations().unwrap();
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: "D:/Legacy-Conflict-Fixture".into(),
                display_name: "Photos".into(),
            })
            .unwrap();
        database.connection_for_fsops().execute(
            "INSERT INTO folders(root_id,parent_id,name,path_hash,status,folder_uuid) VALUES (?1,NULL,'','root','active',?2)",
            params![root_id, sentinel.as_bytes().as_slice()],
        ).unwrap();
        let sentinel_id = database.connection_for_fsops().last_insert_rowid();
        for index in 0..103 {
            database.connection_for_fsops().execute(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,status,folder_uuid) VALUES (?1,?2,?3,'child','active',?4)",
                params![root_id,sentinel_id,format!("Item-{index}"),uuid::Uuid::from_u128(1000+index).as_bytes().as_slice()],
            ).unwrap();
        }
        database
            .initialize_sync_identity(library, library_root, device)
            .unwrap();
        database
            .ensure_selected_sync_root(
                library,
                library_root,
                sentinel,
                library_root,
                "D:/Legacy-Conflict-Fixture",
                "Photos",
                "Photos",
                [1; 32],
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                1,
                "active",
                1,
                false,
            )
            .unwrap();
        database
            .begin_sync_capture(uuid::Uuid::now_v7(), uuid::Uuid::now_v7())
            .unwrap();
        let transaction = database
            .connection_for_fsops()
            .unchecked_transaction()
            .unwrap();
        for index in 0..103 {
            let entity = EntityUuid(uuid::Uuid::from_u128(1000 + index));
            crate::sync::capture::capture_catalog_mutation(
                &transaction,
                crate::sync::capture::MutationOrigin::LocalApi,
                entity,
                Operation::SetLocation {
                    revision_uuid: RevisionUuid::new(),
                    parent_uuid: Some(EntityUuid(sentinel)),
                    name: if index == 101 {
                        "Changed-101".into()
                    } else {
                        format!("Item-{index}")
                    },
                    supersedes: Default::default(),
                },
            )
            .unwrap();
            if index == 102 {
                crate::sync::capture::capture_catalog_mutation(
                    &transaction,
                    crate::sync::capture::MutationOrigin::LocalApi,
                    entity,
                    Operation::SetLocation {
                        revision_uuid: RevisionUuid::new(),
                        parent_uuid: Some(EntityUuid(sentinel)),
                        name: "Item-102".into(),
                        supersedes: Default::default(),
                    },
                )
                .unwrap();
            }
        }
        transaction.commit().unwrap();
        crate::sync::capture::capture_enrolled_root_identities(database.connection_for_fsops())
            .unwrap();

        // Reproduce only the old derived rows. All canonical events above were
        // captured through the production writer and remain unchanged below.
        let transaction = database
            .connection_for_fsops()
            .unchecked_transaction()
            .unwrap();
        transaction
            .execute(
                "UPDATE sync_conflicts SET state='resolved' WHERE state='open'",
                [],
            )
            .unwrap();
        transaction.execute(
            "INSERT OR IGNORE INTO sync_location_heads(entity_uuid,revision_uuid,is_display_winner) SELECT entity_uuid,revision_uuid,0 FROM sync_location_revisions WHERE origin='bootstrap' AND entity_uuid<>?1",
            [sentinel.as_bytes().as_slice()],
        ).unwrap();
        for index in 0..103 {
            let entity = EntityUuid(uuid::Uuid::from_u128(1000 + index));
            let mut statement = transaction.prepare("SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1 ORDER BY revision_uuid").unwrap();
            let heads = statement
                .query_map([entity.0.as_bytes().as_slice()], |row| {
                    row.get::<_, Vec<u8>>(0)
                })
                .unwrap()
                .map(|row| RevisionUuid(uuid::Uuid::from_slice(&row.unwrap()).unwrap()))
                .collect::<Vec<_>>();
            persist_location_conflict(&transaction, entity, &heads).unwrap();
        }
        transaction.commit().unwrap();
        let truth = |database: &crate::db::Database| {
            let connection = database.connection_for_fsops();
            let revisions: String = connection.query_row(
                "SELECT json_group_array(json_array(hex(revision_uuid),hex(entity_uuid),hex(event_uuid),hex(parent_uuid),name,origin,local_mutation_generation)) FROM (SELECT * FROM sync_location_revisions ORDER BY revision_uuid)",[],|row|row.get(0),
            ).unwrap();
            let events: String = connection.query_row(
                "SELECT json_group_array(json_array(hex(event_uuid),hex(payload),hex(integrity_hash))) FROM (SELECT * FROM sync_events ORDER BY event_uuid)",[],|row|row.get(0),
            ).unwrap();
            (
                revisions,
                events,
                database.sync_capture_counts_for_test().unwrap(),
            )
        };
        let original_truth = truth(&database);
        drop(database);
        let database = crate::db::Database::open(&path).unwrap();
        database
            .initialize_sync_identity(library, library_root, device)
            .unwrap();
        let counts = || -> (i64, i64) {
            database.connection_for_fsops().query_row(
                &format!("SELECT (SELECT count(*) FROM sync_conflicts WHERE conflict_kind='location-heads' AND state='open'),(SELECT count(*) FROM sync_location_heads h JOIN sync_location_revisions r ON r.revision_uuid=h.revision_uuid WHERE {REDUNDANT_BOOTSTRAP_LOCATION})"),
                [],|row|Ok((row.get(0)?,row.get(1)?)),
            ).unwrap()
        };
        assert_eq!(counts(), (103, 102));
        crate::sync::capture::capture_enrolled_root_identities(database.connection_for_fsops())
            .unwrap();
        assert_eq!(
            counts().1,
            2,
            "one turn repairs at most 100 shadow entities"
        );
        crate::sync::capture::capture_enrolled_root_identities(database.connection_for_fsops())
            .unwrap();
        assert_eq!(
            counts(),
            (2, 0),
            "genuine differing/bootstrap and multi-canonical conflicts remain"
        );
        assert_eq!(truth(&database), original_truth);
        for (index, expected_canonical) in [(101, 1), (102, 2)] {
            let actual:(i64,i64)=database.connection_for_fsops().query_row(
                "SELECT count(*),sum(r.event_uuid IS NOT NULL) FROM sync_location_heads h JOIN sync_location_revisions r ON r.revision_uuid=h.revision_uuid WHERE h.entity_uuid=?1",
                [uuid::Uuid::from_u128(1000+index).as_bytes().as_slice()],|row|Ok((row.get(0)?,row.get(1)?)),
            ).unwrap();
            assert_eq!(actual, (2, expected_canonical));
        }
        let entity = EntityUuid(uuid::Uuid::from_u128(1000));
        let previous: Vec<u8> = database
            .connection_for_fsops()
            .query_row(
                "SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1",
                [entity.0.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let transaction = database
            .connection_for_fsops()
            .unchecked_transaction()
            .unwrap();
        crate::sync::capture::capture_catalog_mutation(
            &transaction,
            crate::sync::capture::MutationOrigin::LocalApi,
            entity,
            Operation::SetLocation {
                revision_uuid: RevisionUuid::new(),
                parent_uuid: Some(EntityUuid(sentinel)),
                name: "Renamed-0".into(),
                supersedes: std::collections::BTreeSet::from([RevisionUuid(
                    uuid::Uuid::from_slice(&previous).unwrap(),
                )]),
            },
        )
        .unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            counts(),
            (2, 0),
            "superseding canonical history must not resurrect its bootstrap"
        );
        let renamed_truth = truth(&database);
        drop(database);
        let database = crate::db::Database::open(&path).unwrap();
        database
            .initialize_sync_identity(library, library_root, device)
            .unwrap();
        crate::sync::capture::capture_enrolled_root_identities(database.connection_for_fsops())
            .unwrap();
        let open: i64 = database
            .connection_for_fsops()
            .query_row(
                "SELECT count(*) FROM sync_conflicts WHERE state='open'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(open, 2);
        let renamed_head:(i64,i64)=database.connection_for_fsops().query_row(
            "SELECT count(*),sum(r.event_uuid IS NOT NULL) FROM sync_location_heads h JOIN sync_location_revisions r ON r.revision_uuid=h.revision_uuid WHERE h.entity_uuid=?1",
            [entity.0.as_bytes().as_slice()],|row|Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!(renamed_head, (1, 1));
        assert_eq!(truth(&database), renamed_truth);
        drop(database);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn remote_root_and_folder_locations_preserve_sentinel_and_project_folders() {
        let database = crate::db::Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let root_uuid = uuid::Uuid::from_u128(10);
        let root_folder_uuid = uuid::Uuid::from_u128(20);
        let library_root_uuid = uuid::Uuid::from_u128(999);
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: "D:/Remote".into(),
                display_name: "Photos".into(),
            })
            .unwrap();
        database
            .upsert_sync_root_descriptor(&crate::db::SyncRootDescriptorRecord {
                root_uuid: *root_uuid.as_bytes(),
                root_folder_uuid: *root_folder_uuid.as_bytes(),
                library_root_uuid: *library_root_uuid.as_bytes(),
                creation_event_uuid: None,
                winning_location_revision_uuid: None,
                requested_name: "Photos".into(),
                materialized_name: "Photos".into(),
                materialization_fingerprint: [1; 32],
                lifecycle: "active".into(),
                logical_revision: 0,
                updated_at: 1,
            })
            .unwrap();
        database.connection_for_fsops().execute(
            "INSERT INTO folders(root_id,parent_id,name,path_hash,status,folder_uuid) VALUES (?1,NULL,'','root','active',?2)",
            params![root_id, root_folder_uuid.as_bytes().as_slice()],
        ).unwrap();
        database
            .link_sync_root_descriptor(root_id, root_uuid.as_bytes(), root_folder_uuid.as_bytes())
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::from_u128(1),
                library_root_uuid,
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        let bootstrap_head: Vec<u8> = database
            .connection_for_fsops()
            .query_row(
                "SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1",
                [root_folder_uuid.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let root_rename = event(
            1,
            20,
            Operation::SetLocation {
                revision_uuid: RevisionUuid(uuid::Uuid::from_u128(30)),
                parent_uuid: Some(EntityUuid(library_root_uuid)),
                name: "Renamed".into(),
                supersedes: std::collections::BTreeSet::from([RevisionUuid(
                    uuid::Uuid::from_slice(&bootstrap_head).unwrap(),
                )]),
            },
        );
        let folder_kind = event(
            3,
            40,
            Operation::SetField {
                field: "entity-kind".into(),
                value: serde_json::json!("folder"),
            },
        );
        let folder_location = event(
            2,
            40,
            Operation::SetLocation {
                revision_uuid: RevisionUuid(uuid::Uuid::from_u128(41)),
                parent_uuid: Some(EntityUuid(root_folder_uuid)),
                name: "Nested".into(),
                supersedes: Default::default(),
            },
        );
        database
            .apply_remote_sync_events(&[root_rename, folder_location, folder_kind])
            .unwrap();
        let sentinel: String = database
            .connection_for_fsops()
            .query_row(
                "SELECT name FROM folders WHERE folder_uuid=?1",
                [root_folder_uuid.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let root_name: String = database
            .connection_for_fsops()
            .query_row(
                "SELECT display_name FROM roots WHERE root_uuid=?1",
                [root_uuid.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let nested: (String, i64) = database
            .connection_for_fsops()
            .query_row(
                "SELECT f.name,f.root_id FROM folders f WHERE f.folder_uuid=?1",
                [uuid::Uuid::from_u128(40).as_bytes().as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(sentinel, "");
        assert_eq!(root_name, "Renamed");
        assert_eq!(nested, ("Nested".into(), root_id));
    }

    #[test]
    fn remote_truth_advances_without_overwriting_a_local_only_root_projection() {
        let database = crate::db::Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let root_uuid = uuid::Uuid::from_u128(110);
        let root_folder_uuid = uuid::Uuid::from_u128(120);
        let library_root_uuid = uuid::Uuid::from_u128(999);
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: "D:/LocalFork".into(),
                display_name: "Local Photos".into(),
            })
            .unwrap();
        database
            .upsert_sync_root_descriptor(&crate::db::SyncRootDescriptorRecord {
                root_uuid: *root_uuid.as_bytes(),
                root_folder_uuid: *root_folder_uuid.as_bytes(),
                library_root_uuid: *library_root_uuid.as_bytes(),
                creation_event_uuid: None,
                winning_location_revision_uuid: None,
                requested_name: "Photos".into(),
                materialized_name: "Photos".into(),
                materialization_fingerprint: [1; 32],
                lifecycle: "active".into(),
                logical_revision: 0,
                updated_at: 1,
            })
            .unwrap();
        database
            .connection_for_fsops()
            .execute(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,status,folder_uuid) VALUES (?1,NULL,'','root','active',?2)",
                params![root_id, root_folder_uuid.as_bytes().as_slice()],
            )
            .unwrap();
        database
            .link_sync_root_descriptor(root_id, root_uuid.as_bytes(), root_folder_uuid.as_bytes())
            .unwrap();
        database
            .upsert_sync_root_local_binding(&crate::db::SyncRootLocalBindingRecord {
                root_uuid: *root_uuid.as_bytes(),
                library_uuid: *uuid::Uuid::from_u128(1).as_bytes(),
                local_binding_uuid: *uuid::Uuid::from_u128(130).as_bytes(),
                binding_generation: 2,
                root_capture_epoch: *uuid::Uuid::from_u128(131).as_bytes(),
                root_actor_epoch: *uuid::Uuid::from_u128(132).as_bytes(),
                state: "local-only-fork".into(),
                root_id,
                operation_revision: 2,
                updated_at: 1,
            })
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::from_u128(1),
                library_root_uuid,
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        let bootstrap_head: Vec<u8> = database
            .connection_for_fsops()
            .query_row(
                "SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1",
                [root_folder_uuid.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let remote_rename = event(
            1,
            120,
            Operation::SetLocation {
                revision_uuid: RevisionUuid(uuid::Uuid::from_u128(140)),
                parent_uuid: Some(EntityUuid(library_root_uuid)),
                name: "Remote Photos".into(),
                supersedes: std::collections::BTreeSet::from([RevisionUuid(
                    uuid::Uuid::from_slice(&bootstrap_head).unwrap(),
                )]),
            },
        );

        database.apply_remote_sync_events(&[remote_rename]).unwrap();

        let projected_name: String = database
            .connection_for_fsops()
            .query_row(
                "SELECT display_name FROM roots WHERE root_uuid=?1",
                [root_uuid.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let remote_truth_name: String = database
            .connection_for_fsops()
            .query_row(
                "SELECT requested_name FROM sync_root_descriptors WHERE root_uuid=?1",
                [root_uuid.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(projected_name, "Local Photos");
        assert_eq!(remote_truth_name, "Remote Photos");
    }
}
