use std::collections::BTreeSet;

use rusqlite::{params, OptionalExtension, Transaction};
use uuid::Uuid;

use super::codec::{encode_event, seal_event, ENVELOPE_VERSION, REDUCER_VERSION, WIRE_VERSION};
use super::types::{
    ActorEpochUuid, ActorId, BindingEpochUuid, DeviceUuid, EntityUuid, EventEnvelope, EventUuid,
    Hlc, LibraryUuid, Operation, StreamKind, StreamUuid,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationOrigin {
    LocalApi,
    ExternalFs,
    Bootstrap,
    RemoteApply,
    Recovery,
    Materialize,
}
impl MutationOrigin {
    pub fn may_publish(self) -> bool {
        matches!(self, Self::LocalApi | Self::ExternalFs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureResult {
    pub local_mutation_generation: i64,
    pub event_uuid: Option<Vec<u8>>,
    pub actor_seq: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFileIdentity {
    pub asset_uuid: Uuid,
    pub volume_identity: Vec<u8>,
    pub file_identity: Vec<u8>,
    pub content_fingerprint: Option<Vec<u8>>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalIdentityMatch {
    Exact(Uuid),
    UniqueFingerprint(Uuid),
    NewAsset,
    Ambiguous(Vec<Uuid>),
}

pub fn match_external_identity(
    observed_volume: &[u8],
    observed_file: &[u8],
    observed_fingerprint: Option<&[u8]>,
    missing_candidates: &[LocalFileIdentity],
) -> ExternalIdentityMatch {
    if let Some(identity) = missing_candidates.iter().find(|identity| {
        identity.volume_identity == observed_volume && identity.file_identity == observed_file
    }) {
        return ExternalIdentityMatch::Exact(identity.asset_uuid);
    }
    let Some(fingerprint) = observed_fingerprint else {
        return ExternalIdentityMatch::NewAsset;
    };
    let matches = missing_candidates
        .iter()
        .filter(|identity| identity.content_fingerprint.as_deref() == Some(fingerprint))
        .map(|identity| identity.asset_uuid)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => ExternalIdentityMatch::NewAsset,
        [asset_uuid] => ExternalIdentityMatch::UniqueFingerprint(*asset_uuid),
        _ => ExternalIdentityMatch::Ambiguous(matches),
    }
}

/// Captures a mutation through the canonical codec and the same truth
/// materializer used by remote apply and checkpoint replay.
pub fn capture_catalog_mutation(
    transaction: &Transaction<'_>,
    origin: MutationOrigin,
    entity_uuid: EntityUuid,
    operation: Operation,
) -> anyhow::Result<CaptureResult> {
    let state: Option<(i64, i64, i64, i64, String)> = transaction.query_row(
        "SELECT next_local_mutation_generation,capture_enabled,hlc_wall_ms,hlc_logical,compatibility_state FROM sync_local_state WHERE singleton=1", [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    ).optional()?;
    let Some((generation, capture_enabled, old_wall, old_logical, compatibility_state)) = state
    else {
        anyhow::bail!("sync local state is missing");
    };
    if !origin.may_publish() {
        return Ok(CaptureResult {
            local_mutation_generation: generation,
            event_uuid: None,
            actor_seq: None,
        });
    }
    if compatibility_state != "compatible" {
        anyhow::bail!("synchronization compatibility lock is active");
    }
    let next_generation = generation
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("local mutation generation exhausted"))?;
    transaction.execute("UPDATE sync_local_state SET next_local_mutation_generation=?1,updated_at=?2 WHERE singleton=1", params![next_generation, unix_timestamp_millis()])?;
    if capture_enabled == 0 {
        return Ok(CaptureResult {
            local_mutation_generation: generation,
            event_uuid: None,
            actor_seq: None,
        });
    }

    let (library, device, stream, active_actor_epoch, active_binding_epoch, library_actor_seq): (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, i64) = transaction.query_row(
        "SELECT library_uuid,device_uuid,catalog_stream_uuid,active_actor_epoch,active_binding_epoch,next_actor_seq FROM sync_local_state WHERE singleton=1", [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
    )?;
    let root_capture = entity_root_capture_context(transaction, entity_uuid.0)?;
    let (actor_epoch, binding_epoch, actor_seq, root_scoped) = root_capture
        .as_ref()
        .map(|(_, actor, binding, sequence)| (actor.clone(), binding.clone(), *sequence, true))
        .unwrap_or((
            active_actor_epoch,
            active_binding_epoch,
            library_actor_seq,
            false,
        ));
    let event_uuid = EventUuid::new();
    let hlc = Hlc {
        wall_ms: old_wall.max(0) as u64,
        logical: old_logical.max(0) as u32,
    }
    .tick(unix_timestamp_millis().max(0) as u64);
    let stream_uuid = StreamUuid(Uuid::from_slice(&stream)?);
    let event = seal_event(EventEnvelope {
        wire_version: WIRE_VERSION,
        envelope_version: ENVELOPE_VERSION,
        reducer_version: REDUCER_VERSION,
        library_uuid: LibraryUuid(Uuid::from_slice(&library)?),
        event_uuid,
        entity_uuid,
        stream_uuid,
        stream_kind: StreamKind::Catalog,
        actor: ActorId {
            device_uuid: DeviceUuid(Uuid::from_slice(&device)?),
            stream_uuid,
            actor_epoch: ActorEpochUuid(Uuid::from_slice(&actor_epoch)?),
        },
        actor_seq: u64::try_from(actor_seq)?,
        binding_epoch: BindingEpochUuid(Uuid::from_slice(&binding_epoch)?),
        hlc,
        operation,
        base_events: BTreeSet::new(),
        key_version: 0,
        integrity_hash: [0; 32],
    })?;
    persist_local_event(transaction, &event, generation)?;
    super::fault::hit("capture.after-ledger-truth-frontier");
    // A stopped synchronized root remains fully editable, but its fork-local
    // events must never leak into the sealed Provider queue. Root ownership is
    // resolved from stable entity UUIDs inside the same Catalog transaction.
    if root_capture.as_ref().map(|value| value.0.as_str()) != Some("local-only-fork") {
        transaction.execute("INSERT INTO sync_outbox(event_uuid,binding_epoch,state,attempt_count,created_at) VALUES (?1,?2,'pending',0,?3)", params![event_uuid.0.as_bytes().as_slice(), binding_epoch, unix_timestamp_millis()])?;
        super::fault::hit("capture.after-outbox");
    }
    let next_actor_seq = actor_seq
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("actor sequence exhausted"))?;
    transaction.execute(
        "UPDATE sync_local_state SET next_actor_seq=CASE WHEN ?1=0 THEN ?2 ELSE next_actor_seq END,hlc_wall_ms=?3,hlc_logical=?4,updated_at=?5 WHERE singleton=1",
        params![root_scoped as i64,next_actor_seq,hlc.wall_ms as i64,hlc.logical as i64,unix_timestamp_millis()],
    )?;
    if root_scoped {
        transaction.execute(
            "UPDATE sync_actor_epochs SET next_actor_seq=?1 WHERE actor_epoch=?2",
            params![next_actor_seq, actor_epoch],
        )?;
    }
    Ok(CaptureResult {
        local_mutation_generation: generation,
        event_uuid: Some(event_uuid.0.as_bytes().to_vec()),
        actor_seq: Some(actor_seq),
    })
}

fn entity_root_binding_state(
    transaction: &Transaction<'_>,
    entity_uuid: Uuid,
) -> anyhow::Result<Option<String>> {
    transaction
        .query_row(
            "SELECT binding.state FROM sync_root_local_bindings binding JOIN roots root ON root.id=binding.root_id WHERE root.id=COALESCE((SELECT root_id FROM files WHERE asset_uuid=?1),(SELECT root_id FROM folders WHERE folder_uuid=?1),(SELECT id FROM roots WHERE root_folder_uuid=?1))",
            [entity_uuid.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
}

fn entity_root_capture_context(
    transaction: &Transaction<'_>,
    entity_uuid: Uuid,
) -> anyhow::Result<Option<(String, Vec<u8>, Vec<u8>, i64)>> {
    transaction
        .query_row(
            "SELECT binding.state,COALESCE(binding.root_actor_epoch,binding.local_binding_uuid),COALESCE(binding.root_capture_epoch,binding.local_binding_uuid),COALESCE(actor.next_actor_seq,1) FROM sync_root_local_bindings binding JOIN roots root ON root.id=binding.root_id LEFT JOIN sync_actor_epochs actor ON actor.actor_epoch=COALESCE(binding.root_actor_epoch,binding.local_binding_uuid) WHERE root.id=COALESCE((SELECT root_id FROM files WHERE asset_uuid=?1),(SELECT root_id FROM folders WHERE folder_uuid=?1),(SELECT id FROM roots WHERE root_folder_uuid=?1))",
            [entity_uuid.as_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(Into::into)
}

fn persist_local_event(
    transaction: &Transaction<'_>,
    event: &EventEnvelope,
    generation: i64,
) -> anyhow::Result<()> {
    let bytes = encode_event(event)?;
    let now = unix_timestamp_millis();
    transaction.execute(
        "INSERT INTO sync_events(event_uuid,library_uuid,entity_uuid,stream_uuid,stream_kind,device_uuid,actor_epoch,binding_epoch,actor_seq,local_mutation_generation,hlc_wall_ms,hlc_logical,wire_version,envelope_version,reducer_version,key_version,operation,apply_state,integrity_hash,created_at,applied_at,payload) VALUES (?1,?2,?3,?4,'catalog',?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,'local',?17,?18,?18,?19)",
        params![event.event_uuid.0.as_bytes().as_slice(),event.library_uuid.0.as_bytes().as_slice(),event.entity_uuid.0.as_bytes().as_slice(),event.stream_uuid.0.as_bytes().as_slice(),event.actor.device_uuid.0.as_bytes().as_slice(),event.actor.actor_epoch.0.as_bytes().as_slice(),event.binding_epoch.0.as_bytes().as_slice(),event.actor_seq as i64,generation,event.hlc.wall_ms as i64,event.hlc.logical as i64,event.wire_version,event.envelope_version,event.reducer_version,event.key_version,super::inbox::operation_name(&event.operation),event.integrity_hash.as_slice(),now,bytes],
    )?;
    super::inbox::materialize_truth(transaction, event, false)?;
    super::inbox::upsert_frontier_state(
        transaction,
        &event.actor,
        event.actor_seq as i64,
        "ready",
    )?;
    Ok(())
}

pub fn capture_catalog_mutation_if_initialized(
    transaction: &Transaction<'_>,
    origin: MutationOrigin,
    entity_uuid: EntityUuid,
    operation: Operation,
) -> anyhow::Result<Option<CaptureResult>> {
    let exists: i64 = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_local_state WHERE singleton=1)",
        [],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Ok(None);
    }
    capture_catalog_mutation(transaction, origin, entity_uuid, operation).map(Some)
}

pub fn entity_uuid(bytes: &[u8]) -> anyhow::Result<EntityUuid> {
    Ok(EntityUuid(Uuid::from_slice(bytes)?))
}

pub fn observed_tag_adds(
    transaction: &Transaction<'_>,
    owner: EntityUuid,
    member: EntityUuid,
) -> anyhow::Result<BTreeSet<EventUuid>> {
    let mut statement = transaction.prepare("SELECT a.add_event_uuid FROM sync_set_adds a WHERE a.owner_uuid=?1 AND a.member_uuid=?2 AND NOT EXISTS (SELECT 1 FROM sync_set_removals r WHERE r.observed_add_event_uuid=a.add_event_uuid)")?;
    let rows = statement.query_map(
        params![
            owner.0.as_bytes().as_slice(),
            member.0.as_bytes().as_slice()
        ],
        |row| row.get::<_, Vec<u8>>(0),
    )?;
    let mut result = BTreeSet::new();
    for row in rows {
        result.insert(EventUuid(Uuid::from_slice(&row?)?));
    }
    Ok(result)
}

fn unix_timestamp_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_local_and_verified_external_origins_publish() {
        assert!(MutationOrigin::LocalApi.may_publish());
        assert!(MutationOrigin::ExternalFs.may_publish());
        assert!(!MutationOrigin::RemoteApply.may_publish());
        assert!(!MutationOrigin::Bootstrap.may_publish());
        assert!(!MutationOrigin::Recovery.may_publish());
        assert!(!MutationOrigin::Materialize.may_publish());
    }
    #[test]
    fn external_rename_requires_exact_identity_or_unique_fingerprint() {
        let first = Uuid::now_v7();
        let second = Uuid::now_v7();
        let candidates = vec![
            LocalFileIdentity {
                asset_uuid: first,
                volume_identity: b"volume".to_vec(),
                file_identity: b"file".to_vec(),
                content_fingerprint: Some(b"same".to_vec()),
            },
            LocalFileIdentity {
                asset_uuid: second,
                volume_identity: b"other-volume".to_vec(),
                file_identity: b"other-file".to_vec(),
                content_fingerprint: Some(b"same".to_vec()),
            },
        ];
        assert_eq!(
            match_external_identity(b"volume", b"file", None, &candidates),
            ExternalIdentityMatch::Exact(first)
        );
        assert!(matches!(
            match_external_identity(b"unknown", b"unknown", Some(b"same"), &candidates),
            ExternalIdentityMatch::Ambiguous(_)
        ));
        assert_eq!(
            match_external_identity(b"unknown", b"unknown", Some(b"same"), &candidates[..1]),
            ExternalIdentityMatch::UniqueFingerprint(first)
        );
    }
}
