use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::codec::{canonical_bytes, sha256, CodecError, REDUCER_VERSION};
use super::reducer::ProtocolState;
use super::types::*;

pub type StreamVector = BTreeMap<ActorId, u64>;

// v5 owns multiple portable root descriptors and records root ownership for
// browse projection rows while excluding device-local path hashes.
pub const CHECKPOINT_SCHEMA_VERSION: u16 = 5;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SqlValueSnapshot {
    Null,
    Integer(i64),
    Real(String),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TableSnapshot {
    pub table: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<SqlValueSnapshot>>,
}

/// SQL truth that cannot be regenerated solely from canonical events.  This is
/// deliberately table-shaped so schema additions fail closed until the
/// checkpoint schema is advanced and the allow-list in persistence is updated.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CatalogRecoverySnapshot {
    pub tables: Vec<TableSnapshot>,
    pub portable_complete: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaIndexShardRef {
    pub kind: String,
    pub object_key: String,
    pub sha256: String,
    pub byte_length: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostMetadataSnapshot {
    pub library_uuid: String,
    pub name: String,
    pub icon: Option<String>,
    pub binding_state: String,
    #[serde(default)]
    pub events: Vec<EventEnvelope>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CatalogProjectionSnapshot {
    #[serde(default)]
    pub roots: Vec<RootProjectionSnapshot>,
    pub folders: Vec<FolderSnapshot>,
    pub files: Vec<FileSnapshot>,
    pub tags: Vec<TagSnapshot>,
    pub metadata: Vec<MetadataSnapshot>,
    pub memberships: Vec<(EntityUuid, EntityUuid)>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RootProjectionSnapshot {
    pub root_uuid: EntityUuid,
    pub root_folder_uuid: EntityUuid,
    pub display_name: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FolderSnapshot {
    pub uuid: EntityUuid,
    pub root_uuid: EntityUuid,
    pub parent_uuid: Option<EntityUuid>,
    pub name: String,
    pub mtime: Option<i64>,
    pub status: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub uuid: EntityUuid,
    pub root_uuid: EntityUuid,
    pub folder_uuid: EntityUuid,
    pub name: String,
    pub ext: String,
    pub size: i64,
    pub mtime: i64,
    pub ctime: Option<i64>,
    pub status: String,
    pub media_kind: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TagSnapshot {
    pub uuid: EntityUuid,
    pub name: String,
    pub color: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MetadataSnapshot {
    pub asset_uuid: EntityUuid,
    pub rating: Option<i64>,
    pub favorite: bool,
    pub note: Option<String>,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub checkpoint_schema_version: u16,
    pub checkpoint_uuid: CheckpointUuid,
    pub library_uuid: LibraryUuid,
    pub reducer_version: u16,
    #[serde(with = "stream_vector_serde")]
    pub catalog_vector: StreamVector,
    #[serde(with = "stream_vector_serde")]
    pub host_vector: StreamVector,
    pub host_metadata: HostMetadataSnapshot,
    pub projection: CatalogProjectionSnapshot,
    pub state: ProtocolState,
    pub events: Vec<EventEnvelope>,
    pub recovery: CatalogRecoverySnapshot,
    #[serde(default)]
    pub media_index_shards: Vec<MediaIndexShardRef>,
    pub digest: [u8; 32],
}

mod stream_vector_serde {
    use super::{ActorId, StreamVector};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        value: &StreamVector,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .iter()
            .map(|(actor, seq)| (actor, *seq))
            .collect::<Vec<(&ActorId, u64)>>()
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<StreamVector, D::Error> {
        Ok(Vec::<(ActorId, u64)>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

impl Checkpoint {
    pub fn build(
        checkpoint_uuid: CheckpointUuid,
        library_uuid: LibraryUuid,
        catalog_vector: StreamVector,
        host_vector: StreamVector,
        host_metadata: HostMetadataSnapshot,
        projection: CatalogProjectionSnapshot,
        state: ProtocolState,
        events: Vec<EventEnvelope>,
        recovery: CatalogRecoverySnapshot,
        media_index_shards: Vec<MediaIndexShardRef>,
    ) -> Result<Self, CodecError> {
        let mut checkpoint = Self {
            checkpoint_schema_version: CHECKPOINT_SCHEMA_VERSION,
            checkpoint_uuid,
            library_uuid,
            reducer_version: REDUCER_VERSION,
            catalog_vector,
            host_vector,
            host_metadata,
            projection,
            state,
            events,
            recovery,
            media_index_shards,
            digest: [0; 32],
        };
        checkpoint.digest = checkpoint_digest(&checkpoint)?;
        Ok(checkpoint)
    }
    pub fn verify(&self) -> Result<(), CodecError> {
        if self.checkpoint_schema_version != CHECKPOINT_SCHEMA_VERSION
            || self.reducer_version != REDUCER_VERSION
            || self.host_metadata.library_uuid != self.library_uuid.to_string()
        {
            return Err(CodecError::IntegrityMismatch);
        }
        for event in self.events.iter().chain(self.host_metadata.events.iter()) {
            if event.library_uuid != self.library_uuid {
                return Err(CodecError::IntegrityMismatch);
            }
            let vector = if event.stream_kind == StreamKind::HostMetadata {
                &self.host_vector
            } else {
                &self.catalog_vector
            };
            if vector.get(&event.actor).copied().unwrap_or(0) < event.actor_seq {
                return Err(CodecError::IntegrityMismatch);
            }
        }
        let mut shard_keys = std::collections::BTreeSet::new();
        for shard in &self.media_index_shards {
            if !matches!(shard.kind.as_str(), "chunk" | "manifest")
                || shard.byte_length == 0
                || shard.byte_length > super::media::MAX_INDEX_SHARD_BYTES as u64
                || super::media::parse_hash(&shard.sha256).is_err()
                || !shard.object_key.starts_with(if shard.kind == "chunk" {
                    "indexes/chunks/"
                } else {
                    "indexes/manifests/"
                })
                || !shard_keys.insert(shard.object_key.as_str())
            {
                return Err(CodecError::IntegrityMismatch);
            }
        }
        if checkpoint_digest(self)? != self.digest {
            return Err(CodecError::IntegrityMismatch);
        }
        Ok(())
    }
}

fn checkpoint_digest(checkpoint: &Checkpoint) -> Result<[u8; 32], CodecError> {
    let mut material = checkpoint.clone();
    material.digest = [0; 32];
    Ok(sha256(&canonical_bytes(&material)?))
}

pub fn state_digest(state: &ProtocolState) -> Result<[u8; 32], CodecError> {
    Ok(sha256(&canonical_bytes(state)?))
}

pub fn vector_covers(left: &StreamVector, right: &StreamVector) -> bool {
    right
        .iter()
        .all(|(actor, seq)| left.get(actor).copied().unwrap_or(0) >= *seq)
}
pub fn checkpoint_covers(left: &Checkpoint, right: &Checkpoint) -> bool {
    vector_covers(&left.catalog_vector, &right.catalog_vector)
        && vector_covers(&left.host_vector, &right.host_vector)
}

pub fn maximal_checkpoints(checkpoints: &[Checkpoint]) -> Vec<&Checkpoint> {
    checkpoints
        .iter()
        .filter(|candidate| {
            !checkpoints.iter().any(|other| {
                other.checkpoint_uuid != candidate.checkpoint_uuid
                    && checkpoint_covers(other, candidate)
                    && !checkpoint_covers(candidate, other)
            })
        })
        .collect()
}
