//! Cross-Library ("shared") plugin records.
//!
//! The Library scope (`db::plugin_records`) belongs to one Library. The shared
//! scope belongs to the Host: it is visible from every Library and even with
//! none open. It still synchronizes, without a transport of its own:
//!
//! - every field value is stored wrapped as `{"v": value, "t": edit-ms,
//!   "d": device}` and merged last-writer-wins by `(t, d)` — a pure, commutative,
//!   idempotent merge ([`merge_fields`]);
//! - whenever a Library is active, Core reconciles the shared store with that
//!   Library's reserved mirror collections (`mirror--<collection>`), which are
//!   ordinary Library plugin records and therefore travel with that Library's
//!   sync. On another device the mirror merges back into its Host store and
//!   from there into each of its Libraries, so the shared scope converges
//!   through any Library either device synchronizes;
//! - deletion is the merged field `deleted`, never a tombstone, so a delete
//!   and a later edit on different mirrors resolve by time like any field.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::db::plugin_records::{
    parse_record_id, valid_collection, valid_field_name, PluginRecord, PluginRecordError,
    PluginRecordOperation, MAX_FIELDS_PER_RECORD, MAX_FIELD_VALUE_BYTES, MAX_RECORD_BYTES,
};

/// Library collections reserved for shared-scope mirrors.
pub const MIRROR_PREFIX: &str = "mirror--";
/// Reserved shared field carrying the merged deletion state.
pub const DELETED_FIELD: &str = "deleted";
/// Shared collections leave room for the mirror prefix (32 characters).
pub const MAX_SHARED_COLLECTION_LEN: usize = 24;

pub type WrappedFields = Map<String, Value>;

pub fn mirror_collection(collection: &str) -> String {
    format!("{MIRROR_PREFIX}{collection}")
}

pub fn shared_collection_of(mirror: &str) -> Option<&str> {
    mirror
        .strip_prefix(MIRROR_PREFIX)
        .filter(|rest| valid_shared_collection(rest))
}

pub fn is_mirror_collection(collection: &str) -> bool {
    collection.starts_with(MIRROR_PREFIX)
}

pub fn valid_shared_collection(collection: &str) -> bool {
    collection.len() <= MAX_SHARED_COLLECTION_LEN
        && valid_collection(collection)
        && !is_mirror_collection(collection)
}

pub fn wrap(value: Value, t: i64, d: &str) -> Value {
    serde_json::json!({ "v": value, "t": t, "d": d })
}

/// `(t, d)` of a well-formed wrapped value.
fn stamp(value: &Value) -> Option<(i64, &str)> {
    let object = value.as_object()?;
    if !object.contains_key("v") {
        return None;
    }
    Some((object.get("t")?.as_i64()?, object.get("d")?.as_str()?))
}

/// Whether `a` wins over `b` (strictly later stamp; malformed never wins).
fn wins(a: &Value, b: &Value) -> bool {
    match (stamp(a), stamp(b)) {
        (Some(left), Some(right)) => left > right,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Field-wise last-writer-wins union. Commutative, associative, idempotent.
pub fn merge_fields(a: &WrappedFields, b: &WrappedFields) -> WrappedFields {
    let mut merged = WrappedFields::new();
    let names: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    for name in names {
        if name != DELETED_FIELD && !valid_field_name(name) {
            continue;
        }
        let winner = match (a.get(name), b.get(name)) {
            (Some(left), Some(right)) => {
                if wins(right, left) {
                    right
                } else {
                    left
                }
            }
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => continue,
        };
        if stamp(winner).is_some() {
            merged.insert(name.clone(), winner.clone());
        }
    }
    merged
}

/// Visible fields (non-null, without the deletion marker) and deletion state.
pub fn unwrap_fields(fields: &WrappedFields) -> (Map<String, Value>, bool) {
    let mut visible = Map::new();
    let mut deleted = false;
    for (name, wrapped) in fields {
        let Some(value) = wrapped.get("v") else {
            continue;
        };
        if name == DELETED_FIELD {
            deleted = value.as_bool().unwrap_or(false);
        } else if !value.is_null() {
            visible.insert(name.clone(), value.clone());
        }
    }
    (visible, deleted)
}

/// Latest edit time of a record (its `updatedAt`).
pub fn latest_stamp(fields: &WrappedFields) -> i64 {
    fields
        .values()
        .filter_map(stamp)
        .map(|(t, _)| t)
        .max()
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct SharedRecordRow {
    pub id: uuid::Uuid,
    pub collection: String,
    pub fields: WrappedFields,
    pub revision: i64,
    pub created_at: i64,
}

impl SharedRecordRow {
    pub fn to_record(&self) -> PluginRecord {
        let (fields, deleted) = unwrap_fields(&self.fields);
        PluginRecord {
            id: self.id.to_string(),
            collection: self.collection.clone(),
            fields,
            deleted,
            revision: self.revision,
            created_at: self.created_at,
            updated_at: latest_stamp(&self.fields).max(self.created_at),
        }
    }
}

fn validate_visible(fields: &Map<String, Value>) -> Result<(), PluginRecordError> {
    if fields.len() > MAX_FIELDS_PER_RECORD {
        return Err(PluginRecordError::Invalid(format!(
            "a record holds at most {MAX_FIELDS_PER_RECORD} fields"
        )));
    }
    let mut total = 0usize;
    for (name, value) in fields {
        let size = serde_json::to_vec(value)
            .map_err(|error| PluginRecordError::Other(error.into()))?
            .len();
        if size > MAX_FIELD_VALUE_BYTES {
            return Err(PluginRecordError::Invalid(format!(
                "field {name} exceeds {MAX_FIELD_VALUE_BYTES} bytes"
            )));
        }
        total += size + name.len();
    }
    if total > MAX_RECORD_BYTES {
        return Err(PluginRecordError::Invalid(format!(
            "record exceeds {MAX_RECORD_BYTES} bytes"
        )));
    }
    Ok(())
}

/// Apply put / delete / restore operations to the current shared rows
/// (`existing`, by id) and return every row that changed, in operation order.
/// `now` / `device` stamp every written field.
pub fn apply_shared_operations(
    existing: &BTreeMap<uuid::Uuid, SharedRecordRow>,
    operations: Vec<PluginRecordOperation>,
    now: i64,
    device: &str,
) -> Result<Vec<SharedRecordRow>, PluginRecordError> {
    let mut working = existing.clone();
    let mut order: Vec<uuid::Uuid> = Vec::new();
    for operation in operations {
        let (id, collection, patch): (uuid::Uuid, Option<String>, Vec<(String, Value)>) =
            match operation {
                PluginRecordOperation::Put {
                    id,
                    collection,
                    fields,
                    unset,
                } => {
                    let id = parse_record_id(&id)?;
                    if !valid_shared_collection(&collection) {
                        return Err(PluginRecordError::Invalid(format!(
                            "invalid shared collection: {collection}"
                        )));
                    }
                    let mut patch = Vec::new();
                    for (name, value) in fields {
                        if name == DELETED_FIELD || !valid_field_name(&name) {
                            return Err(PluginRecordError::Invalid(format!(
                                "invalid field name: {name}"
                            )));
                        }
                        patch.push((name, value));
                    }
                    for name in unset {
                        if name == DELETED_FIELD || !valid_field_name(&name) {
                            return Err(PluginRecordError::Invalid(format!(
                                "invalid field name: {name}"
                            )));
                        }
                        patch.push((name, Value::Null));
                    }
                    (id, Some(collection), patch)
                }
                PluginRecordOperation::Delete { id } => (
                    parse_record_id(&id)?,
                    None,
                    vec![(DELETED_FIELD.to_owned(), Value::Bool(true))],
                ),
                PluginRecordOperation::Restore { id } => (
                    parse_record_id(&id)?,
                    None,
                    vec![(DELETED_FIELD.to_owned(), Value::Bool(false))],
                ),
            };
        let row = match (working.get(&id), &collection) {
            (Some(row), Some(collection)) if &row.collection != collection => {
                return Err(PluginRecordError::Conflict(
                    "record belongs to another collection".into(),
                ))
            }
            (Some(row), _) => row.clone(),
            (None, Some(collection)) => SharedRecordRow {
                id,
                collection: collection.clone(),
                fields: WrappedFields::new(),
                revision: 0,
                created_at: now,
            },
            (None, None)
                if patch
                    .iter()
                    .any(|(name, value)| name == DELETED_FIELD && value == &Value::Bool(false)) =>
            {
                // Restoring an unknown record is a no-op ("restore then put").
                continue;
            }
            (None, None) => {
                return Err(PluginRecordError::NotFound(format!(
                    "record not found: {id}"
                )))
            }
        };
        let (_, deleted) = unwrap_fields(&row.fields);
        if deleted && collection.is_some() {
            return Err(PluginRecordError::Conflict("record-deleted".into()));
        }
        let mut next = row.clone();
        let mut changed = false;
        for (name, value) in patch {
            let current = next.fields.get(&name).and_then(|wrapped| wrapped.get("v"));
            let same = match current {
                Some(current) => current == &value,
                None => value.is_null(),
            };
            if !same {
                next.fields.insert(name, wrap(value, now, device));
                changed = true;
            }
        }
        if !changed {
            continue;
        }
        validate_visible(&unwrap_fields(&next.fields).0)?;
        working.insert(id, next);
        if !order.contains(&id) {
            order.push(id);
        }
    }
    Ok(order
        .into_iter()
        .filter_map(|id| working.remove(&id))
        .collect())
}

/// Mirror fields (Library side) arrive through sync: keep only well-formed
/// wrapped values so a malformed remote record can never poison the store.
pub fn sanitize_wrapped(fields: &Map<String, Value>) -> WrappedFields {
    fields
        .iter()
        .filter(|(name, value)| {
            (name.as_str() == DELETED_FIELD || valid_field_name(name)) && stamp(value).is_some()
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// A Library mirror record: `(id, mirror collection, wrapped fields)`.
pub type MirrorRecord = (uuid::Uuid, String, Map<String, Value>);

/// Plan one reconcile pass between the Host's shared rows and one Library's
/// mirror records. Returns the Host rows to rewrite and the mirror fields to
/// write (only fields the mirror lacks or holds older). `foreign` names ids
/// owned by another plugin, which are never adopted.
pub fn plan_reconcile(
    host: Vec<SharedRecordRow>,
    mirrors: Vec<MirrorRecord>,
    foreign: impl Fn(&uuid::Uuid) -> bool,
) -> (Vec<SharedRecordRow>, Vec<MirrorRecord>) {
    let mut host: BTreeMap<uuid::Uuid, SharedRecordRow> =
        host.into_iter().map(|row| (row.id, row)).collect();
    let mut host_updates = Vec::new();
    let mut library_updates = Vec::new();
    for (id, mirror, fields) in mirrors {
        let Some(collection) = shared_collection_of(&mirror) else {
            continue;
        };
        let remote = sanitize_wrapped(&fields);
        match host.remove(&id) {
            Some(row) if row.collection != collection => {}
            Some(row) => {
                let merged = merge_fields(&row.fields, &remote);
                let missing: Map<String, Value> = merged
                    .iter()
                    .filter(|(name, value)| remote.get(*name) != Some(*value))
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect();
                if !missing.is_empty() {
                    library_updates.push((id, mirror.clone(), missing));
                }
                if merged != row.fields {
                    host_updates.push(SharedRecordRow {
                        fields: merged,
                        ..row
                    });
                }
            }
            None if foreign(&id) => {}
            None => {
                let created_at = latest_stamp(&remote);
                host_updates.push(SharedRecordRow {
                    id,
                    collection: collection.to_owned(),
                    fields: remote,
                    revision: 0,
                    created_at,
                });
            }
        }
    }
    // Shared records this Library has never carried.
    for (id, row) in host {
        library_updates.push((id, mirror_collection(&row.collection), row.fields));
    }
    (host_updates, library_updates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(entries: &[(&str, Value, i64, &str)]) -> WrappedFields {
        entries
            .iter()
            .map(|(name, value, t, d)| ((*name).to_owned(), wrap(value.clone(), *t, d)))
            .collect()
    }

    #[test]
    fn merge_is_field_wise_last_writer_wins_and_order_independent() {
        let a = fields(&[
            ("title", "a".into(), 10, "dev-a"),
            ("done", false.into(), 30, "dev-a"),
        ]);
        let b = fields(&[
            ("title", "b".into(), 20, "dev-b"),
            ("done", true.into(), 5, "dev-b"),
        ]);
        let merged = merge_fields(&a, &b);
        assert_eq!(merged, merge_fields(&b, &a), "commutative");
        assert_eq!(merge_fields(&merged, &a), merged, "idempotent");
        let (visible, deleted) = unwrap_fields(&merged);
        assert_eq!(visible["title"], "b");
        assert_eq!(visible["done"], false);
        assert!(!deleted);
        // Equal times break ties by device, deterministically.
        let x = fields(&[("title", "x".into(), 7, "dev-a")]);
        let y = fields(&[("title", "y".into(), 7, "dev-b")]);
        assert_eq!(unwrap_fields(&merge_fields(&x, &y)).0["title"], "y");
        assert_eq!(unwrap_fields(&merge_fields(&y, &x)).0["title"], "y");
    }

    #[test]
    fn delete_and_restore_are_merged_fields() {
        let base = fields(&[("title", "t".into(), 1, "d")]);
        let deleted = merge_fields(&base, &fields(&[(DELETED_FIELD, true.into(), 5, "d")]));
        assert!(unwrap_fields(&deleted).1);
        let restored = merge_fields(&deleted, &fields(&[(DELETED_FIELD, false.into(), 9, "e")]));
        assert!(!unwrap_fields(&restored).1);
        // A stale delete does not undo the later restore.
        assert!(!unwrap_fields(&merge_fields(&restored, &deleted)).1);
    }

    #[test]
    fn operations_stamp_changes_and_enforce_rules() {
        let id = uuid::Uuid::now_v7();
        let put = |fields: Value| PluginRecordOperation::Put {
            id: id.to_string(),
            collection: "items".into(),
            fields: fields.as_object().cloned().unwrap(),
            unset: Vec::new(),
        };
        let rows = apply_shared_operations(
            &BTreeMap::new(),
            vec![put(serde_json::json!({"title": "a"}))],
            100,
            "dev",
        )
        .unwrap();
        assert_eq!(rows[0].fields["title"]["t"], 100);
        let existing: BTreeMap<_, _> = rows.into_iter().map(|row| (row.id, row)).collect();
        // Unchanged values are not re-stamped (no spurious sync traffic).
        assert!(apply_shared_operations(
            &existing,
            vec![put(serde_json::json!({"title": "a"}))],
            200,
            "dev"
        )
        .unwrap()
        .is_empty());
        let deleted = apply_shared_operations(
            &existing,
            vec![PluginRecordOperation::Delete { id: id.to_string() }],
            300,
            "dev",
        )
        .unwrap();
        let existing: BTreeMap<_, _> = deleted.into_iter().map(|row| (row.id, row)).collect();
        assert!(matches!(
            apply_shared_operations(
                &existing,
                vec![put(serde_json::json!({"title": "b"}))],
                400,
                "dev"
            ),
            Err(PluginRecordError::Conflict(_))
        ));
        assert!(matches!(
            apply_shared_operations(
                &BTreeMap::new(),
                vec![put(serde_json::json!({"deleted": true}))],
                1,
                "dev"
            ),
            Err(PluginRecordError::Invalid(_))
        ));
        assert!(!valid_shared_collection("mirror--items"));
        assert_eq!(shared_collection_of("mirror--items"), Some("items"));
        assert!(sanitize_wrapped(
            &serde_json::json!({"a": 1, "b": {"v": 1, "t": 1, "d": "x"}})
                .as_object()
                .unwrap()
                .clone()
        )
        .contains_key("b"));
    }

    /// Two devices, two Libraries. Device A edits in Library L1, device B
    /// (sharing only L1 through sync) edits another field, A later opens L2:
    /// every Host store and every mirror converges to the same values.
    #[test]
    fn shared_records_converge_through_any_library_mirror() {
        type Store = BTreeMap<uuid::Uuid, SharedRecordRow>;
        type Mirror = BTreeMap<uuid::Uuid, (String, WrappedFields)>;
        fn reconcile(host: &mut Store, mirror: &mut Mirror) {
            let rows = host.values().cloned().collect();
            let records = mirror
                .iter()
                .map(|(id, (c, f))| (*id, c.clone(), f.clone()))
                .collect();
            let (host_updates, library_updates) = plan_reconcile(rows, records, |_| false);
            for row in host_updates {
                host.insert(row.id, row);
            }
            for (id, collection, fields) in library_updates {
                let entry = mirror
                    .entry(id)
                    .or_insert((collection, WrappedFields::new()));
                for (name, value) in fields {
                    entry.1.insert(name, value);
                }
            }
        }
        /// Library sync between devices: per-field LWW (stamps stand in for HLC).
        fn sync(a: &mut Mirror, b: &mut Mirror) {
            let ids: BTreeSet<_> = a.keys().chain(b.keys()).copied().collect();
            for id in ids {
                let collection = a.get(&id).or(b.get(&id)).unwrap().0.clone();
                let left = a.get(&id).map(|e| e.1.clone()).unwrap_or_default();
                let right = b.get(&id).map(|e| e.1.clone()).unwrap_or_default();
                let merged = merge_fields(&left, &right);
                a.insert(id, (collection.clone(), merged.clone()));
                b.insert(id, (collection, merged));
            }
        }
        let id = uuid::Uuid::now_v7();
        let put = |fields: Value| PluginRecordOperation::Put {
            id: id.to_string(),
            collection: "items".into(),
            fields: fields.as_object().cloned().unwrap(),
            unset: Vec::new(),
        };
        let (mut host_a, mut host_b) = (Store::new(), Store::new());
        let (mut a_l1, mut a_l2, mut b_l1) = (Mirror::new(), Mirror::new(), Mirror::new());
        for row in apply_shared_operations(
            &host_a,
            vec![put(serde_json::json!({"title": "shared", "done": false}))],
            10,
            "A",
        )
        .unwrap()
        {
            host_a.insert(row.id, row);
        }
        reconcile(&mut host_a, &mut a_l1);
        sync(&mut a_l1, &mut b_l1);
        reconcile(&mut host_b, &mut b_l1);
        assert_eq!(unwrap_fields(&host_b[&id].fields).0["title"], "shared");
        for row in apply_shared_operations(
            &host_b,
            vec![put(serde_json::json!({"done": true}))],
            20,
            "B",
        )
        .unwrap()
        {
            host_b.insert(row.id, row);
        }
        for row in apply_shared_operations(
            &host_a,
            vec![put(serde_json::json!({"title": "renamed"}))],
            25,
            "A",
        )
        .unwrap()
        {
            host_a.insert(row.id, row);
        }
        reconcile(&mut host_b, &mut b_l1);
        reconcile(&mut host_a, &mut a_l1);
        sync(&mut a_l1, &mut b_l1);
        reconcile(&mut host_a, &mut a_l1);
        reconcile(&mut host_b, &mut b_l1);
        // Device A switches to another Library: the shared record follows.
        reconcile(&mut host_a, &mut a_l2);
        for fields in [
            &host_a[&id].fields,
            &host_b[&id].fields,
            &a_l1[&id].1,
            &a_l2[&id].1,
            &b_l1[&id].1,
        ] {
            let (visible, deleted) = unwrap_fields(fields);
            assert_eq!(visible["title"], "renamed");
            assert_eq!(visible["done"], true);
            assert!(!deleted);
        }
        // A converged pass plans nothing.
        let records = a_l2
            .iter()
            .map(|(i, (c, f))| (*i, c.clone(), f.clone()))
            .collect();
        let (h, l) = plan_reconcile(host_a.values().cloned().collect(), records, |_| false);
        assert!(h.is_empty() && l.is_empty());
    }
}
