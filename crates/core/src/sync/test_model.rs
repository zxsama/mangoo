use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

use super::checkpoint::*;
use super::codec::*;
use super::reducer::*;
use super::segment::*;
use super::types::*;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
pub(crate) fn event(seq: u64, entity: u128, operation: Operation) -> EventEnvelope {
    seal_event(EventEnvelope {
        wire_version: 1,
        envelope_version: 1,
        reducer_version: 1,
        library_uuid: LibraryUuid(id(1)),
        event_uuid: EventUuid(id(1000 + seq as u128)),
        entity_uuid: EntityUuid(id(entity)),
        stream_uuid: StreamUuid(id(2)),
        stream_kind: StreamKind::Catalog,
        actor: ActorId {
            device_uuid: DeviceUuid(id(3)),
            stream_uuid: StreamUuid(id(2)),
            actor_epoch: ActorEpochUuid(id(4)),
        },
        actor_seq: seq,
        binding_epoch: BindingEpochUuid(id(5)),
        hlc: Hlc {
            wall_ms: seq,
            logical: 0,
        },
        operation,
        base_events: BTreeSet::new(),
        key_version: 0,
        integrity_hash: [0; 32],
    })
    .unwrap()
}

#[test]
fn canonical_event_round_trip_and_tamper_detection() {
    let event = event(
        1,
        10,
        Operation::SetField {
            field: "rating".into(),
            value: 5.into(),
        },
    );
    let bytes = encode_event(&event).unwrap();
    assert_eq!(decode_event(&bytes).unwrap(), event);
    let mut changed = event.clone();
    changed.actor_seq = 9;
    assert_eq!(verify_event(&changed), Err(CodecError::IntegrityMismatch));
}

#[test]
fn repeated_field_history_is_canonical_and_checkpoint_serializable() {
    let mut state = ProtocolState::default();
    for seq in [3, 1, 2] {
        state.apply(&event(
            seq,
            10,
            Operation::SetField {
                field: "note".into(),
                value: serde_json::json!(format!("v{seq}")),
            },
        ));
    }
    let history = &state.entities[&EntityUuid(id(10))].field_history["note"];
    assert_eq!(
        history
            .iter()
            .map(|v| v.version.actor_seq)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    let bytes = canonical_bytes(&state).unwrap();
    assert_eq!(
        serde_json::from_slice::<ProtocolState>(&bytes).unwrap(),
        state
    );

    let checkpoint = Checkpoint::build(
        CheckpointUuid(id(500)),
        LibraryUuid(id(1)),
        BTreeMap::from([(
            event(
                3,
                10,
                Operation::SetField {
                    field: "note".into(),
                    value: serde_json::json!("v3"),
                },
            )
            .actor,
            3,
        )]),
        BTreeMap::new(),
        HostMetadataSnapshot {
            library_uuid: id(1).to_string(),
            name: "Library".into(),
            icon: None,
            binding_state: "synchronized".into(),
            events: vec![],
        },
        CatalogProjectionSnapshot::default(),
        state,
        vec![],
        CatalogRecoverySnapshot {
            tables: vec![],
            portable_complete: true,
        },
        Vec::new(),
    )
    .unwrap();
    checkpoint.verify().unwrap();
}

#[test]
fn checkpoint_v2_is_rejected_before_payload_use() {
    let mut value = serde_json::json!({
        "checkpoint_schema_version": 2,
        "checkpoint_uuid": id(500), "library_uuid": id(1), "reducer_version": 1,
        "catalog_vector": [], "host_vector": [],
        "host_metadata": { "library_uuid": id(1).to_string(), "name": "x", "icon": null, "binding_state": "synchronized", "events": [] },
        "projection": { "folders": [], "files": [], "tags": [], "metadata": [], "memberships": [] },
        "state": { "entities": {}, "applied_events": [] }, "events": [],
        "recovery": { "tables": [], "portable_complete": true }, "digest": vec![0; 32]
    });
    let checkpoint: Checkpoint = serde_json::from_value(value.take()).unwrap();
    assert_eq!(checkpoint.verify(), Err(CodecError::IntegrityMismatch));
}

#[test]
fn hlc_never_moves_backwards() {
    let a = Hlc {
        wall_ms: 10,
        logical: 2,
    };
    assert_eq!(
        a.tick(5),
        Hlc {
            wall_ms: 10,
            logical: 3
        }
    );
    assert_eq!(
        a.merge(
            Hlc {
                wall_ms: 20,
                logical: 7
            },
            3
        ),
        Hlc {
            wall_ms: 20,
            logical: 8
        }
    );
}

#[test]
fn reducer_converges_for_permutations_and_duplicates() {
    let add = event(
        1,
        10,
        Operation::SetAdd {
            set: "tags".into(),
            member_uuid: EntityUuid(id(90)),
        },
    );
    let remove = event(
        2,
        10,
        Operation::SetRemove {
            set: "tags".into(),
            member_uuid: EntityUuid(id(90)),
            observed_adds: BTreeSet::from([add.event_uuid]),
        },
    );
    let late = event(
        3,
        10,
        Operation::SetAdd {
            set: "tags".into(),
            member_uuid: EntityUuid(id(91)),
        },
    );
    let inputs = [&add, &remove, &late, &add];
    let mut left = ProtocolState::default();
    for e in inputs {
        left.apply(e);
    }
    let mut right = ProtocolState::default();
    for e in [&late, &remove, &add, &remove] {
        right.apply(e);
    }
    assert_eq!(left, right);
    assert_eq!(
        left.entities[&EntityUuid(id(10))].live_set_members("tags"),
        BTreeSet::from([EntityUuid(id(91))])
    );
}

#[test]
fn superseding_revision_converges_when_delivered_before_its_parent() {
    let first = event(
        1,
        10,
        Operation::SetLocation {
            revision_uuid: RevisionUuid(id(20)),
            parent_uuid: None,
            name: "old".into(),
            supersedes: BTreeSet::new(),
        },
    );
    let second = event(
        2,
        10,
        Operation::SetLocation {
            revision_uuid: RevisionUuid(id(21)),
            parent_uuid: None,
            name: "new".into(),
            supersedes: BTreeSet::from([RevisionUuid(id(20))]),
        },
    );
    let mut ordered = ProtocolState::default();
    ordered.apply(&first);
    ordered.apply(&second);
    let mut reversed = ProtocolState::default();
    reversed.apply(&second);
    reversed.apply(&first);
    assert_eq!(ordered, reversed);
    assert_eq!(
        reversed.entities[&EntityUuid(id(10))]
            .winning_location()
            .unwrap()
            .name,
        "new"
    );
}

#[test]
fn concurrent_location_and_content_heads_are_retained() {
    let mut state = ProtocolState::default();
    state.apply(&event(
        1,
        10,
        Operation::SetLocation {
            revision_uuid: RevisionUuid(id(20)),
            parent_uuid: None,
            name: "a".into(),
            supersedes: BTreeSet::new(),
        },
    ));
    state.apply(&event(
        2,
        10,
        Operation::SetLocation {
            revision_uuid: RevisionUuid(id(21)),
            parent_uuid: None,
            name: "b".into(),
            supersedes: BTreeSet::new(),
        },
    ));
    state.apply(&event(
        3,
        10,
        Operation::SetContent {
            revision_uuid: RevisionUuid(id(30)),
            manifest_id: "one".into(),
            supersedes: BTreeSet::new(),
            manifest_pack_id: None,
            manifest_offset: None,
            manifest_length: None,
            byte_length: None,
            source_mtime_ms: None,
        },
    ));
    state.apply(&event(
        4,
        10,
        Operation::SetContent {
            revision_uuid: RevisionUuid(id(31)),
            manifest_id: "two".into(),
            supersedes: BTreeSet::new(),
            manifest_pack_id: None,
            manifest_offset: None,
            manifest_length: None,
            byte_length: None,
            source_mtime_ms: None,
        },
    ));
    let truth = &state.entities[&EntityUuid(id(10))];
    assert_eq!(truth.location_heads.len(), 2);
    assert_eq!(truth.content_heads.len(), 2);
    assert_eq!(truth.winning_location().unwrap().name, "b");
}

#[test]
fn restore_only_observed_delete_tokens() {
    let d1 = EventUuid(id(80));
    let d2 = EventUuid(id(81));
    let mut state = ProtocolState::default();
    state.apply(&event(1, 10, Operation::Delete { token: d1 }));
    state.apply(&event(2, 10, Operation::Delete { token: d2 }));
    state.apply(&event(
        3,
        10,
        Operation::Restore {
            observed_tokens: BTreeSet::from([d1]),
        },
    ));
    assert!(state.entities[&EntityUuid(id(10))].is_deleted());
}

#[test]
fn legalization_breaks_cycles_and_suffixes_duplicates() {
    let root = EntityUuid(id(999));
    let mut state = ProtocolState::default();
    state.apply(&event(
        1,
        10,
        Operation::SetLocation {
            revision_uuid: RevisionUuid(id(20)),
            parent_uuid: Some(EntityUuid(id(11))),
            name: "same".into(),
            supersedes: BTreeSet::new(),
        },
    ));
    state.apply(&event(
        2,
        11,
        Operation::SetLocation {
            revision_uuid: RevisionUuid(id(21)),
            parent_uuid: Some(EntityUuid(id(10))),
            name: "same".into(),
            supersedes: BTreeSet::new(),
        },
    ));
    state.apply(&event(
        3,
        12,
        Operation::SetLocation {
            revision_uuid: RevisionUuid(id(22)),
            parent_uuid: Some(root),
            name: "duplicate".into(),
            supersedes: BTreeSet::new(),
        },
    ));
    state.apply(&event(
        4,
        13,
        Operation::SetLocation {
            revision_uuid: RevisionUuid(id(23)),
            parent_uuid: Some(root),
            name: "duplicate".into(),
            supersedes: BTreeSet::new(),
        },
    ));
    let legal = legalize_locations(&state, root);
    assert_eq!(legal[&EntityUuid(id(10))].parent_uuid, Some(root));
    let names: BTreeSet<_> = [EntityUuid(id(12)), EntityUuid(id(13))]
        .into_iter()
        .map(|id| legal[&id].display_name.clone())
        .collect();
    assert_eq!(names.len(), 2);
}

#[test]
fn segment_requires_contiguous_single_actor_and_hash_chain() {
    let first = seal_segment(
        vec![
            event(
                1,
                10,
                Operation::Delete {
                    token: EventUuid(id(70)),
                },
            ),
            event(
                2,
                11,
                Operation::Delete {
                    token: EventUuid(id(71)),
                },
            ),
        ],
        None,
    )
    .unwrap();
    verify_segment(&first, None).unwrap();
    assert!(verify_segment(&first, Some([1; 32])).is_err());
    assert!(seal_segment(
        vec![
            event(
                1,
                10,
                Operation::Delete {
                    token: EventUuid(id(70))
                }
            ),
            event(
                3,
                11,
                Operation::Delete {
                    token: EventUuid(id(71))
                }
            )
        ],
        None
    )
    .is_err());
}

#[test]
fn checkpoint_selection_uses_vector_coverage_not_creation_order() {
    let actor = event(
        1,
        10,
        Operation::Delete {
            token: EventUuid(id(70)),
        },
    )
    .actor;
    let cp = |seq| {
        Checkpoint::build(
            CheckpointUuid::new(),
            LibraryUuid(id(1)),
            BTreeMap::from([(actor.clone(), seq)]),
            BTreeMap::new(),
            HostMetadataSnapshot {
                library_uuid: id(1).to_string(),
                name: "Library".into(),
                icon: None,
                binding_state: "synchronized".into(),
                events: Vec::new(),
            },
            CatalogProjectionSnapshot::default(),
            ProtocolState::default(),
            Vec::new(),
            CatalogRecoverySnapshot {
                tables: Vec::new(),
                portable_complete: true,
            },
            Vec::new(),
        )
        .unwrap()
    };
    let a = cp(2);
    let b = cp(5);
    let checkpoints = [a, b.clone()];
    let maximal = maximal_checkpoints(&checkpoints);
    assert_eq!(maximal.len(), 1);
    assert_eq!(maximal[0].checkpoint_uuid, b.checkpoint_uuid);
    b.verify().unwrap();
}

#[test]
fn bootstrap_cursor_is_stable_and_resumable() {
    let boundary = super::bootstrap::BootstrapBoundary {
        binding_epoch: BindingEpochUuid(id(1)),
        actor_epoch: ActorEpochUuid(id(2)),
        capture_generation: 3,
        catalog_vector: BTreeMap::new(),
        host_vector: BTreeMap::new(),
    };
    let mut a = super::bootstrap::BootstrapCursor::new(boundary.clone());
    a.advance(EntityUuid(id(8)), b"item");
    let resumed = a.clone();
    let mut b = super::bootstrap::BootstrapCursor::new(boundary);
    b.advance(EntityUuid(id(8)), b"item");
    assert_eq!(resumed, b);
}

#[test]
fn subtree_scope_freezes_frontier_and_local_generation() {
    let actor = event(
        1,
        10,
        Operation::Delete {
            token: EventUuid(id(70)),
        },
    )
    .actor;
    let scope = super::bootstrap::FrozenSubtreeScope {
        root_uuid: EntityUuid(id(10)),
        observed_frontier: BTreeMap::from([(actor, 17)]),
        observed_local_mutation_generation: 42,
    };
    let mut cursor = super::bootstrap::SubtreeBuildCursor::new(scope.clone());
    cursor.advance(EntityUuid(id(11)), b"delete");
    let resumed = cursor.clone();
    assert_eq!(resumed.scope, scope);
    assert_eq!(resumed.items_emitted, 1);
    assert_ne!(resumed.rolling_hash, [0; 32]);
}

#[test]
fn checkpoint_digest_detects_truth_tampering() {
    let mut checkpoint = Checkpoint::build(
        CheckpointUuid::new(),
        LibraryUuid(id(1)),
        BTreeMap::new(),
        BTreeMap::new(),
        HostMetadataSnapshot {
            library_uuid: id(1).to_string(),
            name: "Library".into(),
            icon: None,
            binding_state: "synchronized".into(),
            events: Vec::new(),
        },
        CatalogProjectionSnapshot::default(),
        ProtocolState::default(),
        Vec::new(),
        CatalogRecoverySnapshot {
            tables: Vec::new(),
            portable_complete: true,
        },
        Vec::new(),
    )
    .unwrap();
    checkpoint.state.applied_events.insert(EventUuid(id(99)));
    assert_eq!(checkpoint.verify(), Err(CodecError::IntegrityMismatch));
}

#[test]
fn checkpoint_round_trips_media_index_shards() {
    let shard_hash = crate::sync::codec::sha256(b"index shard");
    let shard = super::checkpoint::MediaIndexShardRef {
        kind: "chunk".into(),
        object_key: format!(
            "indexes/chunks/{}/0-{}.idx",
            uuid::Uuid::nil(),
            super::media::hex_hash(&shard_hash)
        ),
        sha256: super::media::hex_hash(&shard_hash),
        byte_length: 128,
    };
    let checkpoint = Checkpoint::build(
        CheckpointUuid::new(),
        LibraryUuid(id(1)),
        BTreeMap::new(),
        BTreeMap::new(),
        HostMetadataSnapshot {
            library_uuid: id(1).to_string(),
            name: "Library".into(),
            icon: None,
            binding_state: "synchronized".into(),
            events: Vec::new(),
        },
        CatalogProjectionSnapshot::default(),
        ProtocolState::default(),
        Vec::new(),
        CatalogRecoverySnapshot {
            tables: Vec::new(),
            portable_complete: true,
        },
        vec![shard.clone()],
    )
    .unwrap();
    checkpoint.verify().unwrap();
    let bytes = canonical_bytes(&checkpoint).unwrap();
    let decoded: Checkpoint = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded.media_index_shards, vec![shard]);
    decoded.verify().unwrap();
}

#[test]
fn hundred_thousand_shuffled_and_duplicated_events_converge() {
    let mut events = (1..=100_000_u64)
        .map(|seq| {
            event(
                seq,
                10,
                Operation::SetField {
                    field: "rating".into(),
                    value: (seq % 6).into(),
                },
            )
        })
        .collect::<Vec<_>>();
    let mut ordered = ProtocolState::default();
    for item in &events {
        ordered.apply(item);
    }
    // Deterministic permutation without a property-test dependency.
    events.sort_by_key(|item| item.actor_seq.wrapping_mul(6364136223846793005));
    let mut shuffled = ProtocolState::default();
    for item in &events {
        shuffled.apply(item);
        if item.actor_seq % 97 == 0 {
            shuffled.apply(item);
        }
    }
    assert_eq!(ordered, shuffled);
}

#[test]
fn hundred_thousand_subtree_items_resume_without_duplicates() {
    let scope = super::bootstrap::FrozenSubtreeScope {
        root_uuid: EntityUuid(id(10)),
        observed_frontier: BTreeMap::new(),
        observed_local_mutation_generation: 100_000,
    };
    let mut cursor = super::bootstrap::SubtreeBuildCursor::new(scope);
    for entity in 1..=100_000_u128 {
        cursor.advance(EntityUuid(id(1_000_000 + entity)), &entity.to_le_bytes());
    }
    let resumed = cursor.clone();
    assert_eq!(resumed.items_emitted, 100_000);
    assert_ne!(resumed.rolling_hash, [0; 32]);
}
