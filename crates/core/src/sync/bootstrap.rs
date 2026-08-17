use serde::{Deserialize, Serialize};

use super::checkpoint::StreamVector;
use super::types::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FrozenSubtreeScope {
    pub root_uuid: EntityUuid,
    pub observed_frontier: StreamVector,
    pub observed_local_mutation_generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubtreeBuildCursor {
    pub scope: FrozenSubtreeScope,
    pub last_entity_uuid: Option<EntityUuid>,
    pub items_emitted: u64,
    pub rolling_hash: [u8; 32],
    pub complete: bool,
}

impl SubtreeBuildCursor {
    pub fn new(scope: FrozenSubtreeScope) -> Self {
        Self {
            scope,
            last_entity_uuid: None,
            items_emitted: 0,
            rolling_hash: [0; 32],
            complete: false,
        }
    }

    pub fn advance(&mut self, entity_uuid: EntityUuid, canonical_item: &[u8]) {
        update_cursor_hash(
            &mut self.rolling_hash,
            &mut self.last_entity_uuid,
            &mut self.items_emitted,
            entity_uuid,
            canonical_item,
        );
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BootstrapBoundary {
    pub binding_epoch: BindingEpochUuid,
    pub actor_epoch: ActorEpochUuid,
    pub capture_generation: u64,
    pub catalog_vector: StreamVector,
    pub host_vector: StreamVector,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BootstrapCursor {
    pub boundary: BootstrapBoundary,
    pub last_entity_uuid: Option<EntityUuid>,
    pub items_emitted: u64,
    pub rolling_hash: [u8; 32],
    pub complete: bool,
}

impl BootstrapCursor {
    pub fn new(boundary: BootstrapBoundary) -> Self {
        Self {
            boundary,
            last_entity_uuid: None,
            items_emitted: 0,
            rolling_hash: [0; 32],
            complete: false,
        }
    }
    pub fn advance(&mut self, entity_uuid: EntityUuid, canonical_item: &[u8]) {
        update_cursor_hash(
            &mut self.rolling_hash,
            &mut self.last_entity_uuid,
            &mut self.items_emitted,
            entity_uuid,
            canonical_item,
        );
    }
    pub fn finish(&mut self) {
        self.complete = true;
    }
}

fn update_cursor_hash(
    rolling_hash: &mut [u8; 32],
    last_entity_uuid: &mut Option<EntityUuid>,
    items_emitted: &mut u64,
    entity_uuid: EntityUuid,
    canonical_item: &[u8],
) {
    let mut material = Vec::with_capacity(32 + 16 + canonical_item.len());
    material.extend_from_slice(rolling_hash);
    material.extend_from_slice(entity_uuid.as_uuid().as_bytes());
    material.extend_from_slice(canonical_item);
    *rolling_hash = super::codec::sha256(&material);
    *last_entity_uuid = Some(entity_uuid);
    *items_emitted += 1;
}
