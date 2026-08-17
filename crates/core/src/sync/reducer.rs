use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::types::*;

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
pub struct ContentRevision {
    pub revision_uuid: RevisionUuid,
    pub manifest_id: String,
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
    pub display_name: String,
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
        groups
            .entry((loc.parent_uuid, loc.name.to_lowercase()))
            .or_default()
            .push(*id);
    }
    let mut output = BTreeMap::new();
    for ((parent, _), mut ids) in groups {
        ids.sort();
        for (index, id) in ids.into_iter().enumerate() {
            let loc = &selected[&id];
            let display_name = if index == 0 {
                loc.name.clone()
            } else {
                format!("{} ({})", loc.name, &id.to_string()[..8])
            };
            output.insert(
                id,
                LegalLocation {
                    parent_uuid: parent,
                    display_name,
                },
            );
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
