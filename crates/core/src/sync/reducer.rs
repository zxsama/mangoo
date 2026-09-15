use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use super::codec::{canonical_bytes, sha256};
use super::types::*;

pub const MATERIALIZED_NAME_VERSION: u16 = 1;
pub const ROOT_IDENTITY_FIELD: &str = "sync-root-identity-v1";
const MAX_COMPONENT_UTF16_UNITS: usize = 240;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Versioned<T> {
    pub value: T,
    pub version: Version,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocationRevision {
    pub revision_uuid: RevisionUuid,
    pub parent_uuid: Option<EntityUuid>,
    pub name: String,
    pub version: Version,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyncRootTruth {
    pub root_uuid: EntityUuid,
    pub root_folder_uuid: EntityUuid,
    pub library_root_uuid: EntityUuid,
    pub creation_event_uuid: Option<EventUuid>,
    pub winning_location_revision_uuid: Option<RevisionUuid>,
    pub requested_name: String,
    pub materialized_name: String,
    pub materialization_fingerprint: [u8; 32],
    pub lifecycle: String,
    pub logical_revision: u64,
}

pub fn decode_root_identity(
    entity: EntityUuid,
    value: &serde_json::Value,
) -> anyhow::Result<SyncRootTruth> {
    let root: SyncRootTruth = serde_json::from_value(value.clone())?;
    if root.root_folder_uuid != entity
        || root.root_uuid == root.root_folder_uuid
        || root.creation_event_uuid.is_some()
        || root.winning_location_revision_uuid.is_some()
        || !matches!(root.lifecycle.as_str(), "active" | "tombstoned")
        || root.materialized_name.is_empty()
        || root.requested_name.is_empty()
    {
        anyhow::bail!("invalid synchronized root identity event");
    }
    Ok(root)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContentRevision {
    pub revision_uuid: RevisionUuid,
    pub manifest_id: String,
    #[serde(default)]
    pub manifest_pack_id: Option<String>,
    #[serde(default)]
    pub manifest_offset: Option<u64>,
    #[serde(default)]
    pub manifest_length: Option<u64>,
    #[serde(default)]
    pub byte_length: Option<u64>,
    #[serde(default)]
    pub source_mtime_ms: Option<i64>,
    pub version: Version,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EntityTruth {
    pub fields: BTreeMap<String, Versioned<serde_json::Value>>,
    /// Losing scalar versions in deterministic Version order.  This is a
    /// vector rather than a JSON object keyed by `Version`: JSON object keys
    /// must be strings and serde_json cannot encode a structured Version key.
    pub field_history: BTreeMap<String, Vec<Versioned<serde_json::Value>>>,
    pub set_adds: BTreeMap<String, BTreeMap<EntityUuid, BTreeSet<EventUuid>>>,
    pub set_removes: BTreeMap<String, BTreeMap<EntityUuid, BTreeSet<EventUuid>>>,
    pub locations: BTreeMap<RevisionUuid, LocationRevision>,
    pub location_heads: BTreeSet<RevisionUuid>,
    pub location_superseded: BTreeSet<RevisionUuid>,
    pub delete_tokens: BTreeSet<EventUuid>,
    pub restored_tokens: BTreeSet<EventUuid>,
    pub contents: BTreeMap<RevisionUuid, ContentRevision>,
    pub content_heads: BTreeSet<RevisionUuid>,
    pub content_superseded: BTreeSet<RevisionUuid>,
}

impl EntityTruth {
    pub fn is_deleted(&self) -> bool {
        self.delete_tokens
            .iter()
            .any(|t| !self.restored_tokens.contains(t))
    }
    pub fn live_set_members(&self, set: &str) -> BTreeSet<EntityUuid> {
        let removed = self.set_removes.get(set);
        self.set_adds
            .get(set)
            .into_iter()
            .flat_map(|members| members.iter())
            .filter(|(member, dots)| {
                let removed_dots = removed.and_then(|members| members.get(member));
                dots.iter()
                    .any(|dot| removed_dots.is_none_or(|removed| !removed.contains(dot)))
            })
            .map(|(member, _)| *member)
            .collect()
    }
    pub fn winning_location(&self) -> Option<&LocationRevision> {
        self.location_heads
            .iter()
            .filter_map(|id| self.locations.get(id))
            .max_by_key(|r| &r.version)
    }
    pub fn winning_content(&self) -> Option<&ContentRevision> {
        self.content_heads
            .iter()
            .filter_map(|id| self.contents.get(id))
            .max_by_key(|r| &r.version)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProtocolState {
    pub entities: BTreeMap<EntityUuid, EntityTruth>,
    pub applied_events: BTreeSet<EventUuid>,
    #[serde(default)]
    pub roots: BTreeMap<EntityUuid, SyncRootTruth>,
}

impl ProtocolState {
    pub fn apply(&mut self, event: &EventEnvelope) -> bool {
        if !self.applied_events.insert(event.event_uuid) {
            return false;
        }
        let version = event.version();
        let entity = self.entities.entry(event.entity_uuid).or_default();
        match &event.operation {
            Operation::SetField { field, value } => {
                if field == ROOT_IDENTITY_FIELD {
                    if let Ok(mut root) = decode_root_identity(event.entity_uuid, value) {
                        root.creation_event_uuid = Some(event.event_uuid);
                        root.logical_revision = root.logical_revision.saturating_add(1);
                        self.roots.entry(root.root_uuid).or_insert(root);
                    }
                }
                let incoming = Versioned {
                    value: value.clone(),
                    version,
                };
                match entity.fields.get(field) {
                    Some(current) if current.version >= incoming.version => {
                        insert_field_history(
                            entity.field_history.entry(field.clone()).or_default(),
                            incoming,
                        );
                    }
                    _ => {
                        if let Some(old) = entity.fields.insert(field.clone(), incoming) {
                            insert_field_history(
                                entity.field_history.entry(field.clone()).or_default(),
                                old,
                            );
                        }
                    }
                }
            }
            Operation::SetAdd { set, member_uuid } => {
                entity
                    .set_adds
                    .entry(set.clone())
                    .or_default()
                    .entry(*member_uuid)
                    .or_default()
                    .insert(event.event_uuid);
            }
            Operation::SetRemove {
                set,
                member_uuid,
                observed_adds,
            } => {
                entity
                    .set_removes
                    .entry(set.clone())
                    .or_default()
                    .entry(*member_uuid)
                    .or_default()
                    .extend(observed_adds);
            }
            Operation::SetLocation {
                revision_uuid,
                parent_uuid,
                name,
                supersedes,
            } => {
                entity.location_superseded.extend(supersedes);
                entity.location_heads = entity
                    .location_heads
                    .difference(&entity.location_superseded)
                    .copied()
                    .collect();
                entity
                    .locations
                    .entry(*revision_uuid)
                    .or_insert_with(|| LocationRevision {
                        revision_uuid: *revision_uuid,
                        parent_uuid: *parent_uuid,
                        name: name.clone(),
                        version,
                    });
                if !entity.location_superseded.contains(revision_uuid) {
                    entity.location_heads.insert(*revision_uuid);
                }
            }
            Operation::Delete { token } => {
                entity.delete_tokens.insert(*token);
            }
            Operation::Restore { observed_tokens } => {
                entity.restored_tokens.extend(observed_tokens);
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
                entity.content_superseded.extend(supersedes);
                entity.content_heads = entity
                    .content_heads
                    .difference(&entity.content_superseded)
                    .copied()
                    .collect();
                entity
                    .contents
                    .entry(*revision_uuid)
                    .or_insert_with(|| ContentRevision {
                        revision_uuid: *revision_uuid,
                        manifest_id: manifest_id.clone(),
                        manifest_pack_id: manifest_pack_id.clone(),
                        manifest_offset: *manifest_offset,
                        manifest_length: *manifest_length,
                        byte_length: *byte_length,
                        source_mtime_ms: *source_mtime_ms,
                        version,
                    });
                if !entity.content_superseded.contains(revision_uuid) {
                    entity.content_heads.insert(*revision_uuid);
                }
            }
        }
        true
    }
}

fn insert_field_history(
    history: &mut Vec<Versioned<serde_json::Value>>,
    value: Versioned<serde_json::Value>,
) {
    match history.binary_search_by(|existing| existing.version.cmp(&value.version)) {
        Ok(_) => {}
        Err(position) => history.insert(position, value),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegalLocation {
    pub parent_uuid: Option<EntityUuid>,
    pub requested_name: String,
    pub display_name: String,
    pub compare_key: String,
    pub materialization_fingerprint: [u8; 32],
}

fn is_windows_reserved_component(value: &str) -> bool {
    let stem = value.split('.').next().unwrap_or_default();
    let upper = stem.to_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
}

fn truncate_utf16(value: &str, limit: usize) -> String {
    let mut units = 0usize;
    value
        .chars()
        .take_while(|character| {
            let next = units + character.len_utf16();
            if next > limit {
                false
            } else {
                units = next;
                true
            }
        })
        .collect()
}

/// Portable component legalization. It deliberately uses no platform locale
/// or filesystem API, so replay on Windows and Linux produces identical names.
pub fn canonicalize_materialized_component(requested: &str, suffix: Option<&str>) -> String {
    let normalized: String = requested.nfc().collect();
    let mut value: String = normalized
        .chars()
        .map(|character| {
            if character <= '\u{1f}'
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    while value.ends_with([' ', '.']) {
        value.pop();
    }
    if value.is_empty() || value == "." || value == ".." {
        value = "untitled".to_owned();
    }
    if is_windows_reserved_component(&value) {
        value.insert(0, '_');
    }
    let suffix = suffix
        .map(|value| format!(" ({value})"))
        .unwrap_or_default();
    let suffix_units = suffix.encode_utf16().count();
    let base_budget = MAX_COMPONENT_UTF16_UNITS
        .saturating_sub(suffix_units)
        .max(1);
    let mut base = truncate_utf16(&value, base_budget);
    while base.ends_with([' ', '.']) {
        base.pop();
    }
    if base.is_empty() {
        base = "_".to_owned();
    }
    base.push_str(&suffix);
    base
}

pub fn materialized_name_compare_key(value: &str) -> String {
    value.nfc().flat_map(char::to_lowercase).collect()
}

fn suffix_candidates(entity_uuid: EntityUuid) -> Vec<String> {
    let simple = entity_uuid.0.simple().to_string();
    [8_usize, 12, 16, 24, 32]
        .into_iter()
        .map(|length| simple[..length].to_owned())
        .collect()
}

pub fn legalize_locations(
    state: &ProtocolState,
    logical_root: EntityUuid,
) -> BTreeMap<EntityUuid, LegalLocation> {
    let mut selected: BTreeMap<_, _> = state
        .entities
        .iter()
        .filter(|(_, e)| !e.is_deleted())
        .filter_map(|(id, e)| e.winning_location().map(|l| (*id, l.clone())))
        .collect();
    let known_entities: BTreeSet<_> = selected.keys().copied().collect();
    for (entity_uuid, location) in &mut selected {
        if location.parent_uuid == Some(*entity_uuid)
            || location
                .parent_uuid
                .is_some_and(|p| !known_entities.contains(&p))
        {
            location.parent_uuid = Some(logical_root);
        }
    }
    // Break each functional-graph cycle at the entity with the smallest stable UUID.
    let keys: Vec<_> = selected.keys().copied().collect();
    for start in keys {
        let mut path = Vec::new();
        let mut positions = HashMap::new();
        let mut cursor = Some(start);
        while let Some(id) = cursor {
            if let Some(pos) = positions.get(&id).copied() {
                if let Some(break_id) = path[pos..].iter().min().copied() {
                    selected.get_mut(&break_id).unwrap().parent_uuid = Some(logical_root);
                }
                break;
            }
            positions.insert(id, path.len());
            path.push(id);
            cursor = selected
                .get(&id)
                .and_then(|l| l.parent_uuid)
                .filter(|p| *p != logical_root);
        }
    }
    let mut groups: HashMap<(Option<EntityUuid>, String), Vec<EntityUuid>> = HashMap::new();
    for (id, loc) in &selected {
        let canonical = canonicalize_materialized_component(&loc.name, None);
        groups
            .entry((loc.parent_uuid, materialized_name_compare_key(&canonical)))
            .or_default()
            .push(*id);
    }
    let mut output = BTreeMap::new();
    let mut groups = groups.into_iter().collect::<Vec<_>>();
    groups.sort_by(|left, right| left.0.cmp(&right.0));
    let mut used_by_parent: BTreeMap<Option<EntityUuid>, BTreeSet<String>> = BTreeMap::new();
    for ((parent, _), mut ids) in groups {
        ids.sort();
        for (index, id) in ids.into_iter().enumerate() {
            let loc = &selected[&id];
            let used = used_by_parent.entry(parent).or_default();
            let mut display_name = canonicalize_materialized_component(&loc.name, None);
            if index != 0 || used.contains(&materialized_name_compare_key(&display_name)) {
                display_name = suffix_candidates(id)
                    .into_iter()
                    .map(|suffix| canonicalize_materialized_component(&loc.name, Some(&suffix)))
                    .find(|candidate| !used.contains(&materialized_name_compare_key(candidate)))
                    .unwrap_or_else(|| {
                        canonicalize_materialized_component(
                            &loc.name,
                            Some(&id.0.hyphenated().to_string()),
                        )
                    });
            }
            let compare_key = materialized_name_compare_key(&display_name);
            used.insert(compare_key.clone());
            output.insert(
                id,
                LegalLocation {
                    parent_uuid: parent,
                    requested_name: loc.name.clone(),
                    display_name,
                    compare_key,
                    materialization_fingerprint: [0; 32],
                },
            );
        }
    }
    let by_parent = output
        .iter()
        .map(|(entity, location)| (location.parent_uuid, *entity))
        .collect::<Vec<_>>();
    for (parent, _) in by_parent {
        let siblings = output
            .iter()
            .filter(|(_, location)| location.parent_uuid == parent)
            .map(|(entity, location)| {
                let winning = selected[entity].revision_uuid;
                (*entity, winning, location.display_name.clone())
            })
            .collect::<Vec<_>>();
        let fingerprint = sha256(
            &canonical_bytes(&(MATERIALIZED_NAME_VERSION, parent, siblings))
                .expect("materialized-name fingerprint serialization"),
        );
        for location in output
            .values_mut()
            .filter(|location| location.parent_uuid == parent)
        {
            location.materialization_fingerprint = fingerprint;
        }
    }
    output
}

pub fn active_delete_tokens(entity: &EntityTruth) -> HashSet<EventUuid> {
    entity
        .delete_tokens
        .difference(&entity.restored_tokens)
        .copied()
        .collect()
}
