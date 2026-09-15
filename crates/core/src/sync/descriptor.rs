//! Core-owned immutable discovery snapshots. Provider listings carry candidates;
//! only verified checkpoint causality may advance the current remote inventory.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use super::checkpoint::{stream_vector_serde, Checkpoint, StreamVector};
use super::codec::{canonical_bytes, sha256};
use super::media::{hex_hash, parse_hash};
use super::provider::{validate_download, ObjectMetadata, SyncProvider, TransportContext};

pub const LEGACY_KEY: &str = "library-descriptor";
pub const MAX_DESCRIPTOR_BYTES: u64 = 1024 * 1024;
pub const MAX_CHECKPOINT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DescriptorEvidence {
    pub seed_sha256: String,
    pub checkpoint_key: String,
    pub checkpoint_sha256: String,
    #[serde(with = "stream_vector_serde")]
    pub catalog_vector: StreamVector,
    #[serde(with = "stream_vector_serde")]
    pub host_vector: StreamVector,
    pub truth_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeedReference {
    pub checkpoint_uuid: uuid::Uuid,
    pub object_key: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryDescriptor {
    pub version: u32,
    pub library_uuid: String,
    pub name: String,
    pub icon: Option<String>,
    pub media_count: u64,
    pub logical_bytes: u64,
    pub last_synced_at: i64,
    pub binding_epoch: uuid::Uuid,
    pub namespace: String,
    pub seed: SeedReference,
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub roots: Vec<crate::host_registry::DiscoveredRemoteRoot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<DescriptorEvidence>,
}

pub fn is_descriptor_key(key: &str) -> bool {
    key == LEGACY_KEY
        || key
            .strip_prefix("descriptors/")
            .and_then(|v| v.strip_suffix(".json"))
            .is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
            })
}

pub fn split_root_key(key: &str) -> anyhow::Result<(&str, &str)> {
    let (library, key) = key
        .split_once('/')
        .context("invalid descriptor namespace")?;
    if uuid::Uuid::parse_str(library)?.to_string() != library || !is_descriptor_key(key) {
        bail!("invalid descriptor object identity");
    }
    Ok((library, key))
}

pub fn snapshot_key(bytes: &[u8]) -> String {
    format!("descriptors/{}.json", hex_hash(&sha256(bytes)))
}

pub fn truth_hash(checkpoint: &Checkpoint) -> anyhow::Result<String> {
    let state = portable_state(checkpoint)?;
    Ok(hex_hash(&sha256(&canonical_bytes(&(
        &checkpoint.host_metadata.name,
        &checkpoint.host_metadata.icon,
        state,
    ))?)))
}

pub fn verified_history(checkpoint: &Checkpoint) -> anyhow::Result<BTreeMap<String, String>> {
    if checkpoint
        .events
        .iter()
        .any(|event| event.stream_kind != super::types::StreamKind::Catalog)
        || checkpoint
            .host_metadata
            .events
            .iter()
            .any(|event| event.stream_kind != super::types::StreamKind::HostMetadata)
    {
        bail!("descriptor checkpoint event stream mismatch");
    }
    let mut history = BTreeMap::new();
    let mut catalog_sequences = BTreeMap::<_, BTreeSet<u64>>::new();
    for event in checkpoint
        .events
        .iter()
        .chain(&checkpoint.host_metadata.events)
    {
        super::codec::verify_event(event)?;
        let key = format!(
            "{}/{}/{}/{}",
            event.actor.device_uuid,
            event.actor.stream_uuid,
            event.actor.actor_epoch,
            event.actor_seq
        );
        if history
            .insert(key, hex_hash(&event.integrity_hash))
            .is_some()
        {
            bail!("descriptor checkpoint repeats an actor sequence");
        }
        if event.stream_kind == super::types::StreamKind::Catalog {
            catalog_sequences
                .entry(event.actor.clone())
                .or_default()
                .insert(event.actor_seq);
        }
    }
    for (actor, sequence) in &checkpoint.catalog_vector {
        let seen = catalog_sequences.get(actor);
        if *sequence != 0
            && !seen.is_some_and(|seen| {
                seen.len() as u64 == *sequence
                    && seen.first() == Some(&1)
                    && seen.last() == Some(sequence)
            })
        {
            bail!("descriptor Catalog frontier has a history gap");
        }
    }
    Ok(history)
}

pub fn history_covers(left: &BTreeMap<String, String>, right: &BTreeMap<String, String>) -> bool {
    right.iter().all(|(key, hash)| left.get(key) == Some(hash))
}

pub fn causal_relation(
    left: &crate::host_registry::DiscoveredRemoteLibrary,
    right: &crate::host_registry::DiscoveredRemoteLibrary,
) -> anyhow::Result<(bool, bool)> {
    let incoming = left
        .evidence
        .as_ref()
        .context("descriptor evidence missing")?;
    let previous = right
        .evidence
        .as_ref()
        .context("descriptor evidence missing")?;
    if left.history.iter().any(|(key, hash)| {
        right
            .history
            .get(key)
            .is_some_and(|previous| previous != hash)
    }) {
        bail!("descriptor actor history fork");
    }
    let covers = incoming.covers(previous);
    let covered = previous.covers(incoming);
    if (covers && !history_covers(&left.history, &right.history))
        || (covered && !history_covers(&right.history, &left.history))
    {
        bail!("descriptor causal history incomplete");
    }
    Ok((covers, covered))
}

/// Logical truth excludes local browse selection and materialization counters.
/// Canonical root identity events provide the stable baseline; legacy roots
/// without such an event are retained only for compatibility with the seed.
fn portable_state(checkpoint: &Checkpoint) -> anyhow::Result<super::reducer::ProtocolState> {
    for field in ["name", "icon"] {
        let winner = checkpoint
            .host_metadata
            .events
            .iter()
            .filter_map(|event| match &event.operation {
                super::types::Operation::SetField {
                    field: event_field,
                    value,
                } if event_field == field => Some((event.version(), value)),
                _ => None,
            })
            .max_by_key(|(version, _)| version.clone());
        if let Some((_, value)) = winner {
            let expected = if field == "name" {
                serde_json::json!(checkpoint.host_metadata.name)
            } else {
                serde_json::json!(checkpoint.host_metadata.icon)
            };
            if value != &expected {
                bail!("descriptor Host metadata does not replay from its events");
            }
        }
    }
    let mut replay = super::reducer::ProtocolState::default();
    for event in &checkpoint.events {
        replay.apply(event);
    }
    if replay.entities != checkpoint.state.entities
        || replay.applied_events != checkpoint.state.applied_events
    {
        bail!("descriptor checkpoint state does not replay from its events");
    }
    for root in checkpoint.state.roots.values() {
        if !replay.roots.contains_key(&root.root_uuid) {
            if root.creation_event_uuid.is_some() {
                bail!("descriptor root identity history missing");
            }
            replay.roots.insert(root.root_uuid, root.clone());
        }
    }
    let library_root = replay
        .roots
        .values()
        .next()
        .map(|root| root.library_root_uuid);
    if let Some(library_root) = library_root {
        let locations = super::reducer::legalize_locations(&replay, library_root);
        for root in replay.roots.values_mut() {
            let entity = replay.entities.get(&root.root_folder_uuid);
            if let Some(location) = locations.get(&root.root_folder_uuid) {
                root.winning_location_revision_uuid = entity
                    .and_then(|entity| entity.winning_location())
                    .map(|location| location.revision_uuid);
                root.requested_name = location.requested_name.clone();
                root.materialized_name = location.display_name.clone();
                root.materialization_fingerprint = location.materialization_fingerprint;
            }
            if let Some(entity) = entity {
                root.lifecycle = if entity.is_deleted() {
                    "tombstoned"
                } else {
                    "active"
                }
                .into();
            }
            let changes = checkpoint
                .events
                .iter()
                .filter(|event| {
                    event.entity_uuid == root.root_folder_uuid
                        && matches!(
                            event.operation,
                            super::types::Operation::SetLocation { .. }
                                | super::types::Operation::Delete { .. }
                                | super::types::Operation::Restore { .. }
                        )
                })
                .count() as u64;
            root.logical_revision = root
                .logical_revision
                .checked_add(changes)
                .context("root revision overflow")?;
        }
    }
    Ok(replay)
}

pub fn from_checkpoint(
    checkpoint: &Checkpoint,
    checkpoint_hash: [u8; 32],
    binding_epoch: uuid::Uuid,
    created_at: i64,
    dependencies: Vec<String>,
    seed_sha256: Option<String>,
) -> anyhow::Result<LibraryDescriptor> {
    checkpoint.verify()?;
    if !checkpoint.recovery.portable_complete {
        bail!("descriptor checkpoint is not portable");
    }
    let state = portable_state(checkpoint)?;
    let mut root_bytes = BTreeMap::<String, u64>::new();
    let mut count = 0u64;
    let mut bytes = 0u64;
    // Browse rows are device-local materializations and may be an empty subset
    // after a root-aware restore. Inventory comes exclusively from portable
    // content/location truth; neither filesystem timestamps nor selection count.
    for (asset, entity) in &state.entities {
        if entity.is_deleted() {
            continue;
        }
        let Some(content) = entity.winning_content() else {
            continue;
        };
        let length = content
            .byte_length
            .context("portable content byte length missing")?;
        let mut cursor = *asset;
        let mut visited = BTreeSet::new();
        let root = loop {
            if let Some(root) = state
                .roots
                .values()
                .find(|root| root.root_folder_uuid == cursor || root.root_uuid == cursor)
            {
                break (root.lifecycle == "active").then_some(root.root_uuid);
            }
            if !visited.insert(cursor) {
                break None;
            }
            let Some(parent) = state
                .entities
                .get(&cursor)
                .filter(|entity| !entity.is_deleted())
                .and_then(|entity| entity.winning_location())
                .and_then(|location| location.parent_uuid)
            else {
                break None;
            };
            cursor = parent;
        };
        let Some(root) = root else { continue };
        count += 1;
        bytes = bytes
            .checked_add(length)
            .context("descriptor byte count overflow")?;
        let root_length = root_bytes.entry(root.to_string()).or_default();
        *root_length = root_length
            .checked_add(length)
            .context("root byte count overflow")?;
    }
    let roots = state
        .roots
        .values()
        .filter(|root| root.lifecycle == "active")
        .map(|root| crate::host_registry::DiscoveredRemoteRoot {
            root_uuid: root.root_uuid.to_string(),
            root_folder_uuid: root.root_folder_uuid.to_string(),
            requested_name: root.requested_name.clone(),
            materialized_name: root.materialized_name.clone(),
            materialization_fingerprint: hex_hash(&root.materialization_fingerprint),
            logical_revision: root.logical_revision,
            logical_bytes: Some(
                root_bytes
                    .get(&root.root_uuid.to_string())
                    .copied()
                    .unwrap_or(0),
            ),
        })
        .collect();
    let key = format!("checkpoints/{}.checkpoint", checkpoint.checkpoint_uuid);
    let checkpoint_sha256 = hex_hash(&checkpoint_hash);
    let snapshot = seed_sha256
        .map(|seed_sha256| -> anyhow::Result<_> {
            Ok(DescriptorEvidence {
                seed_sha256,
                checkpoint_key: key.clone(),
                checkpoint_sha256: checkpoint_sha256.clone(),
                catalog_vector: checkpoint.catalog_vector.clone(),
                host_vector: checkpoint.host_vector.clone(),
                truth_sha256: truth_hash(checkpoint)?,
            })
        })
        .transpose()?;
    Ok(LibraryDescriptor {
        version: 1,
        library_uuid: checkpoint.library_uuid.to_string(),
        name: checkpoint.host_metadata.name.clone(),
        icon: checkpoint.host_metadata.icon.clone(),
        media_count: count,
        logical_bytes: bytes,
        last_synced_at: created_at,
        binding_epoch,
        namespace: checkpoint.library_uuid.to_string(),
        seed: SeedReference {
            checkpoint_uuid: checkpoint.checkpoint_uuid.0,
            object_key: key,
            sha256: checkpoint_sha256,
        },
        dependencies,
        roots,
        snapshot,
    })
}

impl DescriptorEvidence {
    pub fn covers(&self, other: &Self) -> bool {
        self.seed_sha256 == other.seed_sha256
            && super::checkpoint::vector_covers(&self.catalog_vector, &other.catalog_vector)
            && super::checkpoint::vector_covers(&self.host_vector, &other.host_vector)
    }
}

async fn get_bytes(
    provider: &dyn SyncProvider,
    context: &TransportContext,
    key: &str,
    limit: u64,
) -> anyhow::Result<(ObjectMetadata, Vec<u8>)> {
    let object = provider
        .object_get(context, key, limit)
        .await
        .map_err(anyhow::Error::new)?;
    validate_download(&object, limit).map_err(anyhow::Error::new)?;
    if object.metadata.key != key {
        bail!("descriptor dependency key mismatch");
    }
    let bytes = std::fs::read(&object.path)?;
    Ok((object.metadata.clone(), bytes))
}

async fn checkpoint_for(
    provider: &dyn SyncProvider,
    context: &TransportContext,
    document: &LibraryDescriptor,
) -> anyhow::Result<Checkpoint> {
    if document.version != 1
        || document.library_uuid != context.namespace
        || document.namespace != context.namespace
        || document.seed.object_key
            != format!("checkpoints/{}.checkpoint", document.seed.checkpoint_uuid)
    {
        bail!("descriptor identity is incompatible");
    }
    let (metadata, bytes) = get_bytes(
        provider,
        context,
        &document.seed.object_key,
        MAX_CHECKPOINT_BYTES,
    )
    .await?;
    if metadata.sha256 != parse_hash(&document.seed.sha256)? {
        bail!("descriptor checkpoint hash mismatch");
    }
    let checkpoint: Checkpoint = serde_json::from_slice(&bytes)?;
    checkpoint.verify()?;
    verified_history(&checkpoint)?;
    portable_state(&checkpoint)?;
    let mut events = BTreeMap::new();
    for event in checkpoint
        .events
        .iter()
        .chain(checkpoint.host_metadata.events.iter())
    {
        super::codec::verify_event(event)?;
        if events
            .insert((event.actor.clone(), event.actor_seq), event.integrity_hash)
            .is_some_and(|previous| previous != event.integrity_hash)
        {
            bail!("descriptor checkpoint contains an actor fork");
        }
    }
    for (actor, sequence) in checkpoint
        .catalog_vector
        .iter()
        .chain(checkpoint.host_vector.iter())
    {
        let observed = events
            .keys()
            .filter(|(event_actor, _)| event_actor == actor)
            .map(|(_, sequence)| *sequence)
            .max()
            .unwrap_or(0);
        if observed != *sequence {
            bail!("descriptor checkpoint frontier lacks event evidence");
        }
    }
    if checkpoint.library_uuid.to_string() != context.namespace
        || checkpoint.checkpoint_uuid.0 != document.seed.checkpoint_uuid
        || !checkpoint.recovery.portable_complete
    {
        bail!("descriptor checkpoint identity is incompatible");
    }
    Ok(checkpoint)
}

pub async fn download_candidate(
    provider: &dyn SyncProvider,
    context: &TransportContext,
    listed: &ObjectMetadata,
) -> anyhow::Result<(
    crate::host_registry::DiscoveredRemoteLibrary,
    Checkpoint,
    Vec<u8>,
)> {
    download_candidate_reusing(provider, context, listed, None).await
}

pub async fn download_candidate_reusing(
    provider: &dyn SyncProvider,
    context: &TransportContext,
    listed: &ObjectMetadata,
    verified: Option<&crate::host_registry::DiscoveredRemoteLibrary>,
) -> anyhow::Result<(
    crate::host_registry::DiscoveredRemoteLibrary,
    Checkpoint,
    Vec<u8>,
)> {
    if !is_descriptor_key(&listed.key) {
        bail!("invalid descriptor object key");
    }
    let (metadata, bytes) = get_bytes(provider, context, &listed.key, MAX_DESCRIPTOR_BYTES).await?;
    if metadata.size != listed.size || metadata.sha256 != listed.sha256 {
        bail!("descriptor metadata mismatch");
    }
    if listed.key != LEGACY_KEY && snapshot_key(&bytes) != listed.key {
        bail!("descriptor key hash mismatch");
    }
    let document: LibraryDescriptor = serde_json::from_slice(&bytes)?;
    let checkpoint = checkpoint_for(provider, context, &document).await?;
    let seed_sha256 = if listed.key == LEGACY_KEY {
        if document.snapshot.is_some() {
            bail!("legacy descriptor cannot be a snapshot");
        }
        hex_hash(&metadata.sha256)
    } else {
        let snapshot = document
            .snapshot
            .as_ref()
            .context("descriptor snapshot evidence missing")?;
        let (seed_metadata, seed_bytes) =
            get_bytes(provider, context, LEGACY_KEY, MAX_DESCRIPTOR_BYTES).await?;
        if hex_hash(&seed_metadata.sha256) != snapshot.seed_sha256 {
            bail!("descriptor seed lineage mismatch");
        }
        let legacy: LibraryDescriptor = serde_json::from_slice(&seed_bytes)?;
        if legacy.snapshot.is_some() {
            bail!("descriptor seed lineage is not legacy");
        }
        let seed_checkpoint = checkpoint_for(provider, context, &legacy).await?;
        for field in ["name", "icon"] {
            if !checkpoint.host_metadata.events.iter().any(|event| matches!(&event.operation,super::types::Operation::SetField{field:event_field,..} if event_field==field)) {
                let unchanged = if field=="name" {checkpoint.host_metadata.name==seed_checkpoint.host_metadata.name} else {checkpoint.host_metadata.icon==seed_checkpoint.host_metadata.icon};
                if !unchanged { bail!("descriptor Host metadata lacks causal history"); }
            }
        }
        if !super::checkpoint::checkpoint_covers(&checkpoint, &seed_checkpoint)
            || !history_covers(
                &verified_history(&checkpoint)?,
                &verified_history(&seed_checkpoint)?,
            )
        {
            bail!("descriptor snapshot does not cover seed");
        }
        for root in checkpoint
            .state
            .roots
            .values()
            .filter(|root| root.creation_event_uuid.is_none())
        {
            if seed_checkpoint.state.roots.get(&root.root_uuid) != Some(root) {
                bail!("snapshot contains root truth without canonical history");
            }
        }
        snapshot.seed_sha256.clone()
    };
    let expected = from_checkpoint(
        &checkpoint,
        parse_hash(&document.seed.sha256)?,
        document.binding_epoch,
        document.last_synced_at,
        document.dependencies.clone(),
        Some(seed_sha256),
    )?;
    let evidence = expected
        .snapshot
        .clone()
        .context("descriptor evidence missing")?;
    let verified = verified.filter(|candidate| {
        listed
            .etag
            .as_deref()
            .is_some_and(|etag| !etag.is_empty() && !etag.starts_with("W/"))
            && candidate.metadata.key == format!("{}/{}", context.namespace, listed.key)
            && candidate.metadata.size == listed.size
            && candidate.metadata.sha256 == listed.sha256
            && candidate.metadata.etag == listed.etag
            && candidate.evidence.as_ref() == Some(&evidence)
    });
    if listed.key != LEGACY_KEY
        && (document.snapshot.as_ref() != Some(&evidence)
            || document.name != expected.name
            || document.icon != expected.icon
            || document.media_count != expected.media_count
            || document.logical_bytes != expected.logical_bytes
            || canonical_bytes(&document.roots)? != canonical_bytes(&expected.roots)?)
    {
        bail!("descriptor advertised truth differs from checkpoint");
    }
    let dependencies = document.dependencies.iter().collect::<BTreeSet<_>>();
    if dependencies.len() != document.dependencies.len()
        || !dependencies.contains(&document.seed.object_key)
    {
        bail!("descriptor dependency inventory is invalid");
    }
    if verified.is_none() {
        let mut media_objects = BTreeMap::new();
        let mut manifests = BTreeMap::new();
        let mut chunks = BTreeMap::new();
        for shard in &checkpoint.media_index_shards {
            if !dependencies.contains(&shard.object_key) {
                bail!("descriptor index dependency missing");
            }
            let (metadata, bytes) = get_bytes(
                provider,
                context,
                &shard.object_key,
                super::media::MAX_INDEX_SHARD_BYTES as u64,
            )
            .await?;
            if metadata.size != shard.byte_length || metadata.sha256 != parse_hash(&shard.sha256)? {
                bail!("descriptor index identity mismatch");
            }
            let index = super::media::format::IndexShardV1::decode(&bytes)?;
            if index.kind != shard.kind {
                bail!("descriptor index kind mismatch");
            }
            for entry in index.entries {
                if shard.kind == "manifest" {
                    if manifests
                        .insert(entry.hash.clone(), entry.clone())
                        .is_some_and(|previous| previous != entry)
                    {
                        bail!("descriptor manifest locator conflict");
                    }
                } else if chunks
                    .insert(entry.hash.clone(), entry.clone())
                    .is_some_and(|previous| previous != entry)
                {
                    bail!("descriptor chunk locator conflict");
                }
                let identity = (entry.object_hash, entry.object_length);
                if media_objects
                    .insert(entry.object_key, identity.clone())
                    .is_some_and(|previous| previous != identity)
                {
                    bail!("descriptor media locator conflict");
                }
            }
        }
        for content in checkpoint
            .state
            .entities
            .values()
            .filter(|entity| !entity.is_deleted())
            .filter_map(|entity| entity.winning_content())
        {
            let locator = manifests
                .get(&content.manifest_id)
                .context("descriptor live content manifest dependency missing")?;
            if content.manifest_pack_id.as_ref() != Some(&locator.object_hash)
                || content.manifest_offset != Some(locator.offset)
                || content.manifest_length != Some(locator.length)
            {
                bail!("descriptor content manifest locator mismatch");
            }
            if locator.object_key
                != super::media::manifest_object_key(&parse_hash(&locator.object_hash)?)
            {
                bail!("descriptor manifest object key mismatch");
            }
            let manifest = super::media::recovery::fetch_manifest(
                provider,
                context,
                parse_hash(&content.manifest_id)?,
                parse_hash(&locator.object_hash)?,
                locator.offset,
                locator.length,
            )
            .await?;
            if content.byte_length != Some(manifest.byte_length) {
                bail!("descriptor content length mismatch");
            }
            for chunk in manifest.chunks {
                let locator = chunks
                    .get(&chunk.chunk_sha256)
                    .context("descriptor live content chunk dependency missing")?;
                if locator.object_hash != chunk.pack_sha256
                    || locator.offset != chunk.pack_offset
                    || locator.length != chunk.byte_length
                {
                    bail!("descriptor content chunk locator mismatch");
                }
            }
        }
        for (key, (hash, length)) in media_objects {
            let object = provider
                .object_head(context, &key)
                .await
                .map_err(anyhow::Error::new)?
                .context("descriptor media dependency missing")?;
            if object.key != key || object.sha256 != parse_hash(&hash)? || object.size != length {
                bail!("descriptor media dependency identity mismatch");
            }
        }
        for key in &document.dependencies {
            super::provider::validate_object_key(key).map_err(anyhow::Error::new)?;
            if provider
                .object_head(context, key)
                .await
                .map_err(anyhow::Error::new)?
                .is_none()
            {
                bail!("descriptor dependency unavailable");
            }
        }
    }
    Ok((
        crate::host_registry::DiscoveredRemoteLibrary {
            metadata: ObjectMetadata {
                key: format!("{}/{}", context.namespace, listed.key),
                ..metadata
            },
            name: Some(expected.name),
            icon: expected.icon,
            media_count: Some(expected.media_count),
            logical_bytes: Some(expected.logical_bytes),
            last_synced_at: Some(expected.last_synced_at),
            compatible: true,
            error_code: None,
            roots: expected.roots,
            evidence: Some(evidence),
            history: verified_history(&checkpoint)?,
        },
        checkpoint,
        bytes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::types::*;

    fn event(library: LibraryUuid, actor: &ActorId, sequence: u64, value: &str) -> EventEnvelope {
        crate::sync::codec::seal_event(EventEnvelope {
            wire_version: crate::sync::codec::WIRE_VERSION,
            envelope_version: crate::sync::codec::ENVELOPE_VERSION,
            reducer_version: crate::sync::codec::REDUCER_VERSION,
            library_uuid: library,
            event_uuid: EventUuid::new(),
            entity_uuid: EntityUuid(library.0),
            stream_uuid: actor.stream_uuid,
            stream_kind: StreamKind::Catalog,
            actor: actor.clone(),
            actor_seq: sequence,
            binding_epoch: BindingEpochUuid(uuid::Uuid::from_u128(1)),
            hlc: Hlc {
                wall_ms: sequence,
                logical: 0,
            },
            operation: Operation::SetField {
                field: "note".into(),
                value: serde_json::json!(value),
            },
            base_events: Default::default(),
            key_version: 0,
            integrity_hash: [0; 32],
        })
        .unwrap()
    }

    fn checkpoint(library: LibraryUuid, events: Vec<EventEnvelope>) -> Checkpoint {
        let mut state = crate::sync::reducer::ProtocolState::default();
        let mut vector = StreamVector::new();
        for event in &events {
            state.apply(event);
            vector.insert(event.actor.clone(), event.actor_seq);
        }
        Checkpoint::build(
            CheckpointUuid::new(),
            library,
            vector,
            StreamVector::new(),
            crate::sync::checkpoint::HostMetadataSnapshot {
                library_uuid: library.to_string(),
                name: "Library".into(),
                icon: None,
                binding_state: "synchronized".into(),
                events: vec![],
            },
            Default::default(),
            state,
            events,
            crate::sync::checkpoint::CatalogRecoverySnapshot {
                tables: vec![],
                portable_complete: true,
            },
            vec![],
        )
        .unwrap()
    }

    async fn put(
        provider: &crate::sync::provider::InMemorySyncProvider,
        context: &TransportContext,
        checkpoint: &Checkpoint,
        seed: Option<String>,
    ) -> ObjectMetadata {
        let bytes = canonical_bytes(checkpoint).unwrap();
        let key = format!("checkpoints/{}.checkpoint", checkpoint.checkpoint_uuid);
        provider
            .object_put_immutable(context, &key, &bytes, sha256(&bytes))
            .await
            .unwrap();
        let document = from_checkpoint(
            checkpoint,
            sha256(&bytes),
            uuid::Uuid::from_u128(1),
            1,
            vec![key],
            seed.clone(),
        )
        .unwrap();
        let bytes = canonical_bytes(&document).unwrap();
        let key = if seed.is_some() {
            snapshot_key(&bytes)
        } else {
            LEGACY_KEY.into()
        };
        provider
            .object_put_immutable(context, &key, &bytes, sha256(&bytes))
            .await
            .unwrap();
        provider.object_head(context, &key).await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn descriptor_refresh_download_rejects_history_gaps_forks_and_unbacked_truth() {
        let library = LibraryUuid::new();
        let actor = ActorId {
            device_uuid: DeviceUuid::new(),
            stream_uuid: StreamUuid::new(),
            actor_epoch: ActorEpochUuid::new(),
        };
        let first = event(library, &actor, 1, "first");
        let second = event(library, &actor, 2, "second");
        let third = event(library, &actor, 3, "third");
        let context = TransportContext {
            namespace: library.to_string(),
            binding_epoch: uuid::Uuid::from_u128(1).to_string(),
            transport_generation: 1,
        };
        let provider = crate::sync::provider::InMemorySyncProvider::default();
        let seed = checkpoint(library, vec![first.clone(), second.clone()]);
        let seed_metadata = put(&provider, &context, &seed, None).await;
        download_candidate(&provider, &context, &seed_metadata)
            .await
            .unwrap();
        let seed_hash = hex_hash(&seed_metadata.sha256);
        let valid = checkpoint(library, vec![first.clone(), second.clone(), third.clone()]);
        let metadata = put(&provider, &context, &valid, Some(seed_hash.clone())).await;
        download_candidate(&provider, &context, &metadata)
            .await
            .unwrap();
        let gap = checkpoint(library, vec![second.clone(), third.clone()]);
        let metadata = put(&provider, &context, &gap, Some(seed_hash.clone())).await;
        assert!(download_candidate(&provider, &context, &metadata)
            .await
            .unwrap_err()
            .to_string()
            .contains("history gap"));
        let mut fork = first.clone();
        fork.operation = Operation::SetField {
            field: "note".into(),
            value: serde_json::json!("fork"),
        };
        let fork = crate::sync::codec::seal_event(fork).unwrap();
        let fork = checkpoint(library, vec![fork, second.clone(), third.clone()]);
        let metadata = put(&provider, &context, &fork, Some(seed_hash.clone())).await;
        assert!(download_candidate(&provider, &context, &metadata)
            .await
            .unwrap_err()
            .to_string()
            .contains("cover seed"));
        let mut host = checkpoint(library, vec![first, second, third]);
        host.host_metadata.name = "unbacked".into();
        host.digest = [0; 32];
        host.digest = sha256(&canonical_bytes(&host).unwrap());
        let metadata = put(&provider, &context, &host, Some(seed_hash.clone())).await;
        assert!(download_candidate(&provider, &context, &metadata)
            .await
            .unwrap_err()
            .to_string()
            .contains("Host metadata lacks"));
        let mut misplaced = valid.clone();
        misplaced.checkpoint_uuid = CheckpointUuid::new();
        misplaced.host_metadata.events = misplaced.events.clone();
        misplaced.events.clear();
        misplaced.state = Default::default();
        misplaced.digest = [0; 32];
        misplaced.digest = sha256(&canonical_bytes(&misplaced).unwrap());
        let metadata = put(&provider, &context, &misplaced, Some(seed_hash)).await;
        assert!(download_candidate(&provider, &context, &metadata)
            .await
            .unwrap_err()
            .to_string()
            .contains("event stream mismatch"));
    }
}
