use rusqlite::{params, OptionalExtension, Transaction};
use uuid::Uuid;

use super::capture::{capture_catalog_mutation, entity_uuid, MutationOrigin};
use super::types::{ActorEpochUuid, ActorId, DeviceUuid, EventUuid, Operation, StreamUuid};

pub const MAX_BULK_ITEMS_PER_TURN: usize = 100;

pub fn begin_subtree_delete(transaction: &Transaction<'_>, root: &[u8]) -> anyhow::Result<Uuid> {
    let next_generation: i64 = transaction.query_row(
        "SELECT next_local_mutation_generation FROM sync_local_state WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let generation = next_generation.saturating_sub(1);
    transaction.execute(
        "UPDATE sync_local_state SET next_local_mutation_generation=?1,updated_at=?2 WHERE singleton=1",
        params![next_generation + 1, now_ms()],
    )?;
    let mut statement = transaction.prepare("SELECT device_uuid,stream_uuid,actor_epoch,applied_through_seq FROM sync_actor_frontiers ORDER BY device_uuid,stream_uuid,actor_epoch")?;
    let rows: Vec<(Vec<u8>, Vec<u8>, Vec<u8>, i64)> = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    let frontier = super::codec::canonical_bytes(&rows)?;
    let operation = Uuid::now_v7();
    let now = now_ms();
    transaction.execute("INSERT INTO sync_bulk_operations(bulk_operation_uuid,operation_kind,root_entity_uuid,observed_local_mutation_generation,observed_frontier,state,item_count,created_at,updated_at) VALUES (?1,'delete-subtree',?2,?3,?4,'pending',0,?5,?5)", params![operation.as_bytes().as_slice(),root,generation,frontier,now])?;
    Ok(operation)
}

/// Expands a frozen subtree snapshot in a bounded, restartable keyset turn.
pub fn build_subtree_delete_turn(
    transaction: &Transaction<'_>,
    operation: Uuid,
    limit: usize,
) -> anyhow::Result<usize> {
    let limit = limit.clamp(1, MAX_BULK_ITEMS_PER_TURN);
    let (root,boundary,frontier_bytes,cursor): (Vec<u8>,i64,Vec<u8>,Option<Vec<u8>>) = transaction.query_row("SELECT root_entity_uuid,observed_local_mutation_generation,observed_frontier,cursor_entity_uuid FROM sync_bulk_operations WHERE bulk_operation_uuid=?1 AND state IN ('pending','building')", [operation.as_bytes().as_slice()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let frontier_rows: Vec<(Vec<u8>, Vec<u8>, Vec<u8>, i64)> =
        serde_json::from_slice(&frontier_bytes)?;
    let frontier = frontier_rows
        .into_iter()
        .map(|(d, s, a, q)| {
            Ok((
                ActorId {
                    device_uuid: DeviceUuid(Uuid::from_slice(&d)?),
                    stream_uuid: StreamUuid(Uuid::from_slice(&s)?),
                    actor_epoch: ActorEpochUuid(Uuid::from_slice(&a)?),
                },
                q,
            ))
        })
        .collect::<anyhow::Result<std::collections::BTreeMap<_, _>>>()?;
    // Rebuild the location register from revisions that were visible at both
    // boundaries. Event-backed revisions must be covered by the frozen actor
    // frontier; local/eventless revisions must precede the generation fence.
    let mut state = super::reducer::ProtocolState::default();
    let mut statement = transaction.prepare("SELECT r.local_mutation_generation,e.payload,e.device_uuid,e.stream_uuid,e.actor_epoch,e.actor_seq FROM sync_location_revisions r LEFT JOIN sync_events e ON e.event_uuid=r.event_uuid ORDER BY r.rowid")?;
    for row in statement.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<Vec<u8>>>(1)?,
            r.get::<_, Option<Vec<u8>>>(2)?,
            r.get::<_, Option<Vec<u8>>>(3)?,
            r.get::<_, Option<Vec<u8>>>(4)?,
            r.get::<_, Option<i64>>(5)?,
        ))
    })? {
        let (generation, payload, device, stream, epoch, seq) = row?;
        if generation > boundary {
            continue;
        }
        let Some(payload) = payload else {
            continue;
        };
        let actor = ActorId {
            device_uuid: DeviceUuid(Uuid::from_slice(
                &device.ok_or_else(|| anyhow::anyhow!("location event missing device"))?,
            )?),
            stream_uuid: StreamUuid(Uuid::from_slice(
                &stream.ok_or_else(|| anyhow::anyhow!("location event missing stream"))?,
            )?),
            actor_epoch: ActorEpochUuid(Uuid::from_slice(
                &epoch.ok_or_else(|| anyhow::anyhow!("location event missing epoch"))?,
            )?),
        };
        if seq.is_none_or(|seq| frontier.get(&actor).copied().unwrap_or(0) < seq) {
            continue;
        }
        state.apply(&super::codec::decode_event(&payload)?);
    }
    // Include local bootstrap/eventless revisions as deterministic zero-clock
    // locations; they are immutable base truth at the generation fence.
    let mut local=transaction.prepare("SELECT revision_uuid,entity_uuid,parent_uuid,name FROM sync_location_revisions WHERE event_uuid IS NULL AND local_mutation_generation<=?1 ORDER BY local_mutation_generation,revision_uuid")?;
    for row in local.query_map([boundary], |r| {
        Ok((
            r.get::<_, Vec<u8>>(0)?,
            r.get::<_, Vec<u8>>(1)?,
            r.get::<_, Option<Vec<u8>>>(2)?,
            r.get::<_, String>(3)?,
        ))
    })? {
        let (revision, entity, parent, name) = row?;
        let entity = super::types::EntityUuid(Uuid::from_slice(&entity)?);
        let truth = state.entities.entry(entity).or_default();
        let revision = super::types::RevisionUuid(Uuid::from_slice(&revision)?);
        truth.locations.insert(
            revision,
            super::reducer::LocationRevision {
                revision_uuid: revision,
                parent_uuid: parent
                    .map(|v| Uuid::from_slice(&v).map(super::types::EntityUuid))
                    .transpose()?,
                name,
                version: super::types::Version {
                    hlc: Default::default(),
                    device_uuid: DeviceUuid(Uuid::nil()),
                    actor_epoch: ActorEpochUuid(Uuid::nil()),
                    actor_seq: 0,
                    event_uuid: EventUuid(Uuid::from_bytes(*revision.0.as_bytes())),
                },
            },
        );
        truth.location_heads.insert(revision);
    }
    let root_uuid = super::types::EntityUuid(Uuid::from_slice(&root)?);
    let locations = super::reducer::legalize_locations(&state, root_uuid);
    let mut all = std::collections::BTreeSet::from([root_uuid]);
    loop {
        let before = all.len();
        for (entity, location) in &locations {
            if location.parent_uuid.is_some_and(|p| all.contains(&p)) {
                all.insert(*entity);
            }
        }
        if all.len() == before {
            break;
        }
    }
    let cursor_uuid = cursor
        .as_deref()
        .map(Uuid::from_slice)
        .transpose()?
        .map(super::types::EntityUuid);
    let entities = all
        .into_iter()
        .filter(|entity| cursor_uuid.is_none_or(|cursor| *entity > cursor))
        .take(limit)
        .map(|entity| entity.0.as_bytes().to_vec())
        .collect::<Vec<_>>();
    for entity in &entities {
        let mut events=transaction.prepare("SELECT device_uuid,stream_uuid,actor_epoch,actor_seq FROM sync_events WHERE entity_uuid=?1")?;
        let published = events
            .query_map([entity], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?
            .filter_map(Result::ok)
            .any(|(d, s, a, q)| {
                let actor = ActorId {
                    device_uuid: DeviceUuid(Uuid::from_slice(&d).unwrap_or(Uuid::nil())),
                    stream_uuid: StreamUuid(Uuid::from_slice(&s).unwrap_or(Uuid::nil())),
                    actor_epoch: ActorEpochUuid(Uuid::from_slice(&a).unwrap_or(Uuid::nil())),
                };
                frontier
                    .get(&actor)
                    .is_some_and(|through| q >= 0 && q <= *through)
            });
        let action = if published {
            "publish-delete"
        } else {
            "archive-local"
        };
        transaction.execute("INSERT OR IGNORE INTO sync_bulk_items(bulk_operation_uuid,entity_uuid,action,state) VALUES (?1,?2,?3,'pending')", params![operation.as_bytes().as_slice(),entity,action])?;
    }
    let next_cursor = entities.last().cloned();
    let done = entities.len() < limit;
    transaction.execute("UPDATE sync_bulk_operations SET state=?1,cursor_entity_uuid=?2,item_count=(SELECT COUNT(*) FROM sync_bulk_items WHERE bulk_operation_uuid=?3),updated_at=?4 WHERE bulk_operation_uuid=?3", params![if done {"publishing"} else {"building"},next_cursor,operation.as_bytes().as_slice(),now_ms()])?;
    super::fault::hit("bulk.after-page-cursor");
    Ok(entities.len())
}

pub fn publish_subtree_delete_turn(
    transaction: &Transaction<'_>,
    operation: Uuid,
    limit: usize,
) -> anyhow::Result<usize> {
    let mut statement = transaction.prepare("SELECT entity_uuid,action FROM sync_bulk_items WHERE bulk_operation_uuid=?1 AND state='pending' ORDER BY entity_uuid LIMIT ?2")?;
    let items: Vec<(Vec<u8>, String)> = statement
        .query_map(
            params![
                operation.as_bytes().as_slice(),
                limit.clamp(1, MAX_BULK_ITEMS_PER_TURN) as i64
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .collect::<Result<_, _>>()?;
    for (entity, action) in &items {
        if action == "publish-delete" {
            let result = capture_catalog_mutation(
                transaction,
                MutationOrigin::LocalApi,
                entity_uuid(entity)?,
                Operation::Delete {
                    token: EventUuid::new(),
                },
            )?;
            transaction.execute("UPDATE sync_bulk_items SET state='complete',event_uuid=?1 WHERE bulk_operation_uuid=?2 AND entity_uuid=?3", params![result.event_uuid,operation.as_bytes().as_slice(),entity])?;
        } else {
            transaction.execute("UPDATE sync_publication_intents SET state='cancelled',updated_at=?1 WHERE asset_uuid=?2 AND state NOT IN ('canonicalized','cancelled')", params![now_ms(),entity])?;
            transaction.execute("UPDATE sync_bulk_items SET state='complete' WHERE bulk_operation_uuid=?1 AND entity_uuid=?2", params![operation.as_bytes().as_slice(),entity])?;
        }
    }
    let pending: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM sync_bulk_items WHERE bulk_operation_uuid=?1 AND state='pending'",
        [operation.as_bytes().as_slice()],
        |r| r.get(0),
    )?;
    if pending == 0 {
        transaction.execute("UPDATE sync_bulk_operations SET state='complete',updated_at=?1 WHERE bulk_operation_uuid=?2", params![now_ms(),operation.as_bytes().as_slice()])?;
    }
    Ok(items.len())
}

pub fn operation_state(
    transaction: &Transaction<'_>,
    operation: Uuid,
) -> anyhow::Result<Option<String>> {
    Ok(transaction
        .query_row(
            "SELECT state FROM sync_bulk_operations WHERE bulk_operation_uuid=?1",
            [operation.as_bytes().as_slice()],
            |r| r.get(0),
        )
        .optional()?)
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

    #[test]
    fn persistent_scope_builder_is_bounded_and_restartable() {
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        database
            .add_root(NewRoot {
                path: "D:/Managed".into(),
                display_name: "Managed".into(),
            })
            .unwrap();
        let library = Uuid::now_v7();
        database
            .initialize_sync_identity(library, Uuid::now_v7(), Uuid::now_v7())
            .unwrap();
        let transaction = database
            .connection_for_fsops()
            .unchecked_transaction()
            .unwrap();
        let root = Uuid::now_v7();
        transaction.execute("INSERT INTO sync_location_revisions(revision_uuid,entity_uuid,parent_uuid,name,local_mutation_generation,origin,created_at) VALUES (?1,?2,NULL,'root',1,'bootstrap',1)",params![Uuid::now_v7().as_bytes().as_slice(),root.as_bytes().as_slice()]).unwrap();
        for index in 0..205 {
            transaction.execute("INSERT INTO sync_location_revisions(revision_uuid,entity_uuid,parent_uuid,name,local_mutation_generation,origin,created_at) VALUES (?1,?2,?3,?4,1,'bootstrap',1)",params![Uuid::now_v7().as_bytes().as_slice(),Uuid::now_v7().as_bytes().as_slice(),root.as_bytes().as_slice(),format!("item-{index}")]).unwrap();
        }
        let operation = begin_subtree_delete(&transaction, root.as_bytes()).unwrap();
        assert_eq!(
            build_subtree_delete_turn(&transaction, operation, 100).unwrap(),
            100
        );
        assert_eq!(
            build_subtree_delete_turn(&transaction, operation, 100).unwrap(),
            100
        );
        assert_eq!(
            build_subtree_delete_turn(&transaction, operation, 100).unwrap(),
            6
        );
        let count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM sync_bulk_items WHERE bulk_operation_uuid=?1",
                [operation.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 206);
        assert_eq!(
            operation_state(&transaction, operation).unwrap().as_deref(),
            Some("publishing")
        );
    }

    #[test]
    fn subtree_membership_excludes_post_frontier_remote_move_in_and_next_local_generation() {
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        database
            .add_root(NewRoot {
                path: "D:/Managed".into(),
                display_name: "Managed".into(),
            })
            .unwrap();
        database
            .initialize_sync_identity(Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7())
            .unwrap();
        database.enable_fake_sync_capture_for_test().unwrap();
        let transaction = database
            .connection_for_fsops()
            .unchecked_transaction()
            .unwrap();
        let root = Uuid::now_v7();
        transaction.execute("INSERT INTO sync_location_revisions(revision_uuid,entity_uuid,parent_uuid,name,local_mutation_generation,origin,created_at) VALUES (?1,?2,NULL,'root',1,'bootstrap',1)",params![Uuid::now_v7().as_bytes().as_slice(),root.as_bytes().as_slice()]).unwrap();
        let operation = begin_subtree_delete(&transaction, root.as_bytes()).unwrap();
        let next_local = Uuid::now_v7();
        transaction.execute("INSERT INTO sync_location_revisions(revision_uuid,entity_uuid,parent_uuid,name,local_mutation_generation,origin,created_at) VALUES (?1,?2,?3,'later-local',2,'external_fs',2)",params![Uuid::now_v7().as_bytes().as_slice(),next_local.as_bytes().as_slice(),root.as_bytes().as_slice()]).unwrap();
        let remote = crate::sync::test_model::event(
            1,
            900,
            Operation::SetLocation {
                revision_uuid: crate::sync::types::RevisionUuid::new(),
                parent_uuid: Some(crate::sync::types::EntityUuid(root)),
                name: "later-remote".into(),
                supersedes: Default::default(),
            },
        );
        let payload = crate::sync::codec::encode_event(&remote).unwrap();
        transaction.execute("INSERT INTO sync_events(event_uuid,library_uuid,entity_uuid,stream_uuid,stream_kind,device_uuid,actor_epoch,binding_epoch,actor_seq,local_mutation_generation,hlc_wall_ms,hlc_logical,wire_version,envelope_version,reducer_version,key_version,operation,apply_state,integrity_hash,created_at,payload) VALUES (?1,?2,?3,?4,'catalog',?5,?6,?7,1,1,?8,?9,1,1,1,0,'set-location','applied',?10,1,?11)",params![remote.event_uuid.0.as_bytes().as_slice(),remote.library_uuid.0.as_bytes().as_slice(),remote.entity_uuid.0.as_bytes().as_slice(),remote.stream_uuid.0.as_bytes().as_slice(),remote.actor.device_uuid.0.as_bytes().as_slice(),remote.actor.actor_epoch.0.as_bytes().as_slice(),remote.binding_epoch.0.as_bytes().as_slice(),remote.hlc.wall_ms as i64,remote.hlc.logical as i64,remote.integrity_hash.as_slice(),payload]).unwrap();
        if let Operation::SetLocation {
            revision_uuid,
            parent_uuid,
            name,
            ..
        } = &remote.operation
        {
            transaction.execute("INSERT INTO sync_location_revisions(revision_uuid,entity_uuid,event_uuid,parent_uuid,name,local_mutation_generation,origin,created_at) VALUES (?1,?2,?3,?4,?5,1,'remote_apply',1)",params![revision_uuid.0.as_bytes().as_slice(),remote.entity_uuid.0.as_bytes().as_slice(),remote.event_uuid.0.as_bytes().as_slice(),parent_uuid.map(|v|v.0.as_bytes().to_vec()),name]).unwrap();
        }
        build_subtree_delete_turn(&transaction, operation, 100).unwrap();
        let count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM sync_bulk_items WHERE bulk_operation_uuid=?1",
                [operation.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "only the observed root belongs to the frozen scope"
        );
    }
}
