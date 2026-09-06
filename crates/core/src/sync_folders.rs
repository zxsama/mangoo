use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::library_path::{
    free_space_bytes, has_existing_reparse_component, identity_token, is_platform_sync_root,
    known_cloud_roots, local_file_identity, normalize_absolute, paths_overlap, probe_writable,
    SafeDirectory,
};

pub const ROOT_MARKER_FILE_NAME: &str = ".megle-sync-root";
pub const PLAN_TTL_MILLIS: i64 = 15 * 60 * 1_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncRootMarker {
    pub format_version: u32,
    pub library_uuid: String,
    pub root_uuid: String,
    pub root_folder_uuid: String,
    pub local_binding_uuid: String,
    pub binding_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedRootPath {
    pub normalized_path: String,
    pub canonical_path_key: String,
    pub placement_parent: String,
    pub placement_parent_key: String,
    pub identity_token: String,
    pub filesystem_identity: Option<String>,
    pub content_evidence_digest: String,
    pub available_bytes: Option<u64>,
    #[serde(default)]
    pub path_existed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootPathMode {
    EnrollExisting,
    MaterializeRemote,
    RebindExisting,
}

pub fn request_hash(value: &serde_json::Value) -> anyhow::Result<String> {
    Ok(hex(&Sha256::digest(serde_json::to_vec(value)?)))
}

pub fn materialization_fingerprint(name: &str) -> String {
    hex(&Sha256::digest(name.to_lowercase().as_bytes()))
}

pub fn canonical_path_key(path: &Path) -> String {
    path.components()
        .collect::<PathBuf>()
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

pub fn default_materialized_name(
    requested_name: &str,
    root_uuid: Uuid,
    occupied_names: &BTreeSet<String>,
) -> String {
    let base = crate::sync::reducer::canonicalize_materialized_component(requested_name, None);
    if !occupied_names.contains(&crate::sync::reducer::materialized_name_compare_key(&base)) {
        return base;
    }
    let short = root_uuid.simple().to_string()[..8].to_owned();
    crate::sync::reducer::canonicalize_materialized_component(requested_name, Some(&short))
}

pub fn validate_root_path(
    candidate: &Path,
    mode: RootPathMode,
    placement_base: &Path,
    bound_paths: &[PathBuf],
) -> anyhow::Result<ValidatedRootPath> {
    let normalized = normalize_absolute(candidate).map_err(|_| anyhow::anyhow!("invalid-path"))?;
    if canonical_path_key(&normalized) == canonical_path_key(placement_base) {
        bail!("placement-base-selected");
    }
    if has_existing_reparse_component(&normalized) {
        bail!("reparse-point");
    }
    if known_cloud_roots()
        .iter()
        .any(|cloud| paths_overlap(&normalized, cloud))
        || is_platform_sync_root(&normalized)
    {
        bail!("cloud-sync-root");
    }
    if bound_paths
        .iter()
        .any(|existing| paths_overlap(&normalized, existing))
    {
        bail!("overlaps-active-root");
    }
    let existing = normalized.exists();
    if matches!(
        mode,
        RootPathMode::EnrollExisting | RootPathMode::RebindExisting
    ) && !normalized.is_dir()
    {
        bail!("path-unavailable");
    }
    if mode == RootPathMode::MaterializeRemote && existing {
        let has_user_content = fs::read_dir(&normalized)
            .context("path-unavailable")?
            .filter_map(Result::ok)
            .any(|entry| entry.file_name() != ROOT_MARKER_FILE_NAME);
        if has_user_content {
            bail!("directory-not-empty");
        }
    }
    if mode == RootPathMode::EnrollExisting && path_contains_foreign_marker(&normalized)? {
        bail!("marker-mismatch");
    }
    let parent = if existing {
        normalized.parent().unwrap_or(&normalized).to_path_buf()
    } else {
        normalized
            .parent()
            .filter(|parent| parent.is_dir())
            .context("path-unavailable")?
            .to_path_buf()
    };
    if !probe_writable(&normalized) {
        bail!("path-not-writable");
    }
    let token = identity_token(&normalized).context("path-unavailable")?;
    let filesystem_identity = existing
        .then(|| local_file_identity(&normalized).ok())
        .flatten();
    Ok(ValidatedRootPath {
        normalized_path: normalized.to_string_lossy().into_owned(),
        canonical_path_key: canonical_path_key(&normalized),
        placement_parent: parent.to_string_lossy().into_owned(),
        placement_parent_key: canonical_path_key(&parent),
        identity_token: token,
        filesystem_identity,
        content_evidence_digest: directory_evidence_digest(&normalized)?,
        available_bytes: free_space_bytes(&normalized).ok(),
        path_existed: existing,
    })
}

fn path_contains_foreign_marker(path: &Path) -> anyhow::Result<bool> {
    if !path.join(ROOT_MARKER_FILE_NAME).exists() {
        return Ok(false);
    }
    read_root_marker(path)?;
    Ok(true)
}

pub fn directory_evidence_digest(path: &Path) -> anyhow::Result<String> {
    if !path.exists() {
        return Ok(hex(&Sha256::digest(b"")));
    }
    let mut entries = Vec::new();
    collect_directory_evidence(path, path, &mut entries, 0)?;
    entries.sort();
    Ok(hex(&Sha256::digest(entries.join("\n").as_bytes())))
}

pub fn preflight_evidence_still_matches(
    planned: &ValidatedRootPath,
    current: &ValidatedRootPath,
    expected_marker: Option<&SyncRootMarker>,
) -> bool {
    if planned.normalized_path != current.normalized_path
        || planned.canonical_path_key != current.canonical_path_key
        || planned.placement_parent_key != current.placement_parent_key
        || planned.content_evidence_digest != current.content_evidence_digest
    {
        return false;
    }
    if planned.path_existed == current.path_existed {
        return planned.identity_token == current.identity_token
            && planned.filesystem_identity == current.filesystem_identity;
    }
    !planned.path_existed
        && current.path_existed
        && expected_marker.is_some_and(|marker| {
            read_root_marker(Path::new(&current.normalized_path))
                .ok()
                .as_ref()
                == Some(marker)
        })
}

fn collect_directory_evidence(
    base: &Path,
    current: &Path,
    output: &mut Vec<String>,
    depth: usize,
) -> anyhow::Result<()> {
    if depth > 64 || output.len() >= 100_000 {
        bail!("directory-inventory-too-large");
    }
    for entry in fs::read_dir(current).context("path-unavailable")? {
        let entry = entry?;
        if entry.file_name() == ROOT_MARKER_FILE_NAME {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || metadata_is_reparse(&metadata) {
            bail!("reparse-point");
        }
        let relative = entry
            .path()
            .strip_prefix(base)?
            .to_string_lossy()
            .replace('\\', "/");
        output.push(format!(
            "{}\0{}\0{}",
            relative,
            if metadata.is_dir() { "d" } else { "f" },
            metadata.len()
        ));
        if metadata.is_dir() {
            collect_directory_evidence(base, &entry.path(), output, depth + 1)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse(_metadata: &fs::Metadata) -> bool {
    false
}

pub fn read_root_marker(path: &Path) -> anyhow::Result<SyncRootMarker> {
    let bytes = fs::read(path.join(ROOT_MARKER_FILE_NAME)).context("marker-mismatch")?;
    serde_json::from_slice(&bytes).context("marker-mismatch")
}

pub fn write_root_marker(path: &Path, marker: &SyncRootMarker) -> anyhow::Result<()> {
    fs::create_dir_all(path)?;
    let marker_path = path.join(ROOT_MARKER_FILE_NAME);
    if marker_path.exists() {
        let existing = read_root_marker(path)?;
        if existing == *marker {
            return Ok(());
        }
        bail!("marker-mismatch");
    }
    let directory = SafeDirectory::open(path).map_err(|_| anyhow::anyhow!("path-changed"))?;
    let temporary_name = format!("{ROOT_MARKER_FILE_NAME}.{}.tmp", Uuid::now_v7());
    let temporary = std::ffi::OsStr::new(&temporary_name);
    let mut file = directory.create_file(temporary)?;
    file.write_all(&serde_json::to_vec_pretty(marker)?)?;
    file.sync_all()?;
    drop(file);
    directory.rename_child(
        temporary,
        std::ffi::OsStr::new(ROOT_MARKER_FILE_NAME),
        false,
    )?;
    directory
        .verify_unchanged()
        .map_err(|_| anyhow::anyhow!("path-changed"))?;
    directory.sync_all()?;
    Ok(())
}

pub fn replace_root_marker(path: &Path, marker: &SyncRootMarker) -> anyhow::Result<()> {
    let existing = read_root_marker(path)?;
    if existing.library_uuid != marker.library_uuid
        || existing.root_uuid != marker.root_uuid
        || existing.root_folder_uuid != marker.root_folder_uuid
    {
        bail!("marker-mismatch");
    }
    let directory = SafeDirectory::open(path).map_err(|_| anyhow::anyhow!("path-changed"))?;
    let temporary_name = format!("{ROOT_MARKER_FILE_NAME}.{}.tmp", Uuid::now_v7());
    let temporary = std::ffi::OsStr::new(&temporary_name);
    let mut file = directory.create_file(temporary)?;
    file.write_all(&serde_json::to_vec_pretty(marker)?)?;
    file.sync_all()?;
    drop(file);
    directory.rename_child(temporary, std::ffi::OsStr::new(ROOT_MARKER_FILE_NAME), true)?;
    directory
        .verify_unchanged()
        .map_err(|_| anyhow::anyhow!("path-changed"))?;
    directory.sync_all()?;
    Ok(())
}

pub fn replace_root_marker_for_publish(
    path: &Path,
    source_root_uuid: &str,
    marker: &SyncRootMarker,
) -> anyhow::Result<()> {
    let existing = read_root_marker(path)?;
    if existing == *marker {
        return Ok(());
    }
    if existing.library_uuid != marker.library_uuid || existing.root_uuid != source_root_uuid {
        bail!("marker-mismatch");
    }
    let directory = SafeDirectory::open(path).map_err(|_| anyhow::anyhow!("path-changed"))?;
    let temporary_name = format!("{ROOT_MARKER_FILE_NAME}.{}.tmp", Uuid::now_v7());
    let temporary = std::ffi::OsStr::new(&temporary_name);
    let mut file = directory.create_file(temporary)?;
    file.write_all(&serde_json::to_vec_pretty(marker)?)?;
    file.sync_all()?;
    drop(file);
    directory.rename_child(temporary, std::ffi::OsStr::new(ROOT_MARKER_FILE_NAME), true)?;
    directory
        .verify_unchanged()
        .map_err(|_| anyhow::anyhow!("path-changed"))?;
    directory.sync_all()?;
    Ok(())
}

pub fn remove_root_marker_if_matches(
    path: &Path,
    marker: &SyncRootMarker,
    remove_empty_directory: bool,
) -> anyhow::Result<()> {
    if read_root_marker(path).as_ref().ok() != Some(marker) {
        return Ok(());
    }
    let directory = SafeDirectory::open(path).map_err(|_| anyhow::anyhow!("path-changed"))?;
    directory.delete_file_child(std::ffi::OsStr::new(ROOT_MARKER_FILE_NAME))?;
    directory.sync_all()?;
    if remove_empty_directory {
        let _ = fs::remove_dir(path);
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RootSnapshot {
    pub root_uuid: String,
    pub frontier: serde_json::Value,
    pub entities: BTreeMap<String, BTreeMap<String, String>>,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RootStopWork {
    pub pending_work: u64,
    pub published_work: u64,
    pub receipt_keys: Vec<String>,
}

impl RootSnapshot {
    pub fn from_entities(
        root_uuid: String,
        frontier: serde_json::Value,
        entities: BTreeMap<String, BTreeMap<String, String>>,
    ) -> anyhow::Result<Self> {
        let digest = hex(&Sha256::digest(serde_json::to_vec(&(
            &root_uuid, &frontier, &entities,
        ))?));
        Ok(Self {
            root_uuid,
            frontier,
            entities,
            digest,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotDifference {
    pub entity_uuid: String,
    pub field_name: String,
    pub classification: String,
    pub base_digest: Option<String>,
    pub local_digest: Option<String>,
    pub remote_digest: Option<String>,
    pub suggested_action: Option<String>,
    pub conflict_uuid: Option<String>,
}

pub fn compare_snapshots(
    base: &RootSnapshot,
    local: &RootSnapshot,
    remote: &RootSnapshot,
) -> Vec<SnapshotDifference> {
    let mut entities = BTreeSet::new();
    entities.extend(base.entities.keys().cloned());
    entities.extend(local.entities.keys().cloned());
    entities.extend(remote.entities.keys().cloned());
    let mut output = Vec::new();
    for entity in entities {
        let base_values = base.entities.get(&entity);
        let local_values = local.entities.get(&entity);
        let remote_values = remote.entities.get(&entity);
        let local_deleted = base_values
            .and_then(|values| values.get("existence"))
            .map(String::as_str)
            != Some("deleted")
            && local_values
                .and_then(|values| values.get("existence"))
                .map(String::as_str)
                == Some("deleted");
        let remote_deleted = base_values
            .and_then(|values| values.get("existence"))
            .map(String::as_str)
            != Some("deleted")
            && remote_values
                .and_then(|values| values.get("existence"))
                .map(String::as_str)
                == Some("deleted");
        let remote_changed_while_local_deleted = local_deleted
            && remote_values.is_some_and(|values| {
                values.iter().any(|(field, value)| {
                    field != "existence"
                        && base_values.and_then(|base| base.get(field)) != Some(value)
                })
            });
        let local_changed_while_remote_deleted = remote_deleted
            && local_values.is_some_and(|values| {
                values.iter().any(|(field, value)| {
                    field != "existence"
                        && base_values.and_then(|base| base.get(field)) != Some(value)
                })
            });
        let mut fields = BTreeSet::new();
        for snapshot in [base, local, remote] {
            if let Some(values) = snapshot.entities.get(&entity) {
                fields.extend(values.keys().cloned());
            }
        }
        for field in fields {
            let b = base.entities.get(&entity).and_then(|v| v.get(&field));
            let l = local.entities.get(&entity).and_then(|v| v.get(&field));
            let r = remote.entities.get(&entity).and_then(|v| v.get(&field));
            let delete_edit_conflict = (local_deleted && r != b && field != "existence")
                || (remote_deleted && l != b && field != "existence")
                || (field == "existence"
                    && (remote_changed_while_local_deleted || local_changed_while_remote_deleted));
            let (classification, suggested_action, conflict_uuid) = if delete_edit_conflict {
                (
                    "conflict",
                    None,
                    Some(deterministic_difference_uuid(&entity, &field, b, l, r).to_string()),
                )
            } else if l == b && r == b {
                ("unchanged", None, None)
            } else if l == r {
                ("same-change", None, None)
            } else if r == b {
                (
                    if field == "existence" && l.is_some_and(|value| value == "deleted") {
                        "deleted-local"
                    } else {
                        "local-only"
                    },
                    Some("keep-local"),
                    None,
                )
            } else if l == b {
                (
                    if field == "existence" && r.is_some_and(|value| value == "deleted") {
                        "deleted-remote"
                    } else {
                        "remote-only"
                    },
                    Some("keep-remote"),
                    None,
                )
            } else {
                (
                    "conflict",
                    None,
                    Some(deterministic_difference_uuid(&entity, &field, b, l, r).to_string()),
                )
            };
            output.push(SnapshotDifference {
                entity_uuid: entity.clone(),
                field_name: field,
                classification: classification.to_owned(),
                base_digest: b.map(|value| hex(&Sha256::digest(value.as_bytes()))),
                local_digest: l.map(|value| hex(&Sha256::digest(value.as_bytes()))),
                remote_digest: r.map(|value| hex(&Sha256::digest(value.as_bytes()))),
                suggested_action: suggested_action.map(str::to_owned),
                conflict_uuid,
            });
        }
    }
    output
}

fn deterministic_difference_uuid(
    entity: &str,
    field: &str,
    base: Option<&String>,
    local: Option<&String>,
    remote: Option<&String>,
) -> Uuid {
    let bytes = serde_json::to_vec(&(entity, field, base, local, remote)).expect("serializable");
    let mut uuid_bytes: [u8; 16] = Sha256::digest(bytes)[..16].try_into().expect("uuid bytes");
    uuid_bytes[6] = (uuid_bytes[6] & 0x0f) | 0x50;
    uuid_bytes[8] = (uuid_bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(uuid_bytes)
}

pub fn snapshot_from_checkpoint(
    checkpoint: &crate::sync::checkpoint::Checkpoint,
    root_uuid: Uuid,
) -> anyhow::Result<RootSnapshot> {
    checkpoint.verify()?;
    snapshot_from_protocol_state(
        &checkpoint.state,
        root_uuid,
        serde_json::to_value(&checkpoint.catalog_vector)?,
    )
}

pub fn snapshot_from_protocol_state(
    state: &crate::sync::reducer::ProtocolState,
    root_uuid: Uuid,
    frontier: serde_json::Value,
) -> anyhow::Result<RootSnapshot> {
    use crate::sync::types::EntityUuid;

    let descriptor = state
        .roots
        .get(&EntityUuid(root_uuid))
        .context("root-not-found")?;
    let root_folder_uuid = descriptor.root_folder_uuid;
    let mut entities = BTreeMap::new();
    for (entity_uuid, truth) in &state.entities {
        if !entity_belongs_to_root(state, *entity_uuid, root_folder_uuid) {
            continue;
        }
        let mut fields = BTreeMap::new();
        fields.insert(
            "existence".to_owned(),
            if truth.is_deleted() {
                "deleted"
            } else {
                "live"
            }
            .to_owned(),
        );
        if let Some(location) = truth.winning_location() {
            fields.insert(
                "location".to_owned(),
                serde_json::to_string(&serde_json::json!({
                    "revisionUuid": location.revision_uuid.to_string(),
                    "parentUuid": location.parent_uuid.map(|value| value.to_string()),
                    "name": location.name,
                }))?,
            );
        }
        if let Some(content) = truth.winning_content() {
            fields.insert(
                "content".to_owned(),
                serde_json::to_string(&serde_json::json!({
                    "revisionUuid": content.revision_uuid.to_string(),
                    "manifestHash": normalized_manifest_hash(&content.manifest_id),
                    "manifestId": content.manifest_id,
                    "manifestPackId": content.manifest_pack_id,
                    "manifestOffset": content.manifest_offset,
                    "manifestLength": content.manifest_length,
                    "byteLength": content.byte_length,
                    "sourceMtimeMs": content.source_mtime_ms,
                }))?,
            );
        }
        for (field, value) in &truth.fields {
            fields.insert(
                format!("field:{field}"),
                serde_json::to_string(&value.value)?,
            );
        }
        let mut set_names = BTreeSet::new();
        set_names.extend(truth.set_adds.keys().cloned());
        set_names.extend(truth.set_removes.keys().cloned());
        for set in set_names {
            let members = truth
                .live_set_members(&set)
                .into_iter()
                .map(|value| value.to_string())
                .collect::<BTreeSet<_>>();
            fields.insert(format!("set:{set}"), serde_json::to_string(&members)?);
        }
        entities.insert(entity_uuid.to_string(), fields);
    }
    let root_fields = entities.entry(root_folder_uuid.to_string()).or_default();
    root_fields.insert(
        "root:requested-name".to_owned(),
        descriptor.requested_name.clone(),
    );
    root_fields.insert(
        "root:materialized-name".to_owned(),
        descriptor.materialized_name.clone(),
    );
    root_fields.insert("root:lifecycle".to_owned(), descriptor.lifecycle.clone());
    root_fields.insert(
        "root:logical-revision".to_owned(),
        descriptor.logical_revision.to_string(),
    );
    root_fields.insert(
        "root:materialization-fingerprint".to_owned(),
        hex(&descriptor.materialization_fingerprint),
    );
    root_fields.insert(
        "root:winning-location-revision".to_owned(),
        descriptor
            .winning_location_revision_uuid
            .map(|value| value.to_string())
            .unwrap_or_default(),
    );
    RootSnapshot::from_entities(root_uuid.to_string(), frontier, entities)
}

fn entity_belongs_to_root(
    state: &crate::sync::reducer::ProtocolState,
    entity_uuid: crate::sync::types::EntityUuid,
    root_folder_uuid: crate::sync::types::EntityUuid,
) -> bool {
    if entity_uuid == root_folder_uuid {
        return true;
    }
    let mut cursor = entity_uuid;
    let mut visited = BTreeSet::new();
    while visited.insert(cursor) {
        let Some(parent) = state
            .entities
            .get(&cursor)
            .and_then(|truth| truth.winning_location())
            .and_then(|location| location.parent_uuid)
        else {
            return false;
        };
        if parent == root_folder_uuid {
            return true;
        }
        cursor = parent;
    }
    false
}

fn normalized_manifest_hash(manifest_id: &str) -> String {
    crate::sync::media::parse_hash(manifest_id)
        .map(|value| hex(&value))
        .unwrap_or_else(|_| hex(&Sha256::digest(manifest_id.as_bytes())))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(generation: u64) -> SyncRootMarker {
        SyncRootMarker {
            format_version: 1,
            library_uuid: Uuid::from_u128(1).to_string(),
            root_uuid: Uuid::from_u128(2).to_string(),
            root_folder_uuid: Uuid::from_u128(3).to_string(),
            local_binding_uuid: Uuid::from_u128(generation as u128 + 10).to_string(),
            binding_generation: generation,
        }
    }

    #[test]
    fn same_name_collision_uses_stable_short_root_uuid() {
        let root = Uuid::parse_str("12345678-0000-7000-8000-000000000001").unwrap();
        let occupied = BTreeSet::from(["照片".to_owned()]);
        assert_eq!(
            default_materialized_name("照片", root, &occupied),
            "照片 (12345678)"
        );
    }

    #[test]
    fn comparison_classifies_three_way_fields() {
        let make = |value: &str| {
            RootSnapshot::from_entities(
                "root".to_owned(),
                serde_json::json!({}),
                BTreeMap::from([(
                    "entity".to_owned(),
                    BTreeMap::from([("content".to_owned(), value.to_owned())]),
                )]),
            )
            .unwrap()
        };
        assert_eq!(
            compare_snapshots(&make("b"), &make("l"), &make("b"))[0].classification,
            "local-only"
        );
        assert_eq!(
            compare_snapshots(&make("b"), &make("l"), &make("r"))[0].classification,
            "conflict"
        );
        assert_eq!(
            compare_snapshots(&make("b"), &make("l"), &make("r"))[0].conflict_uuid,
            compare_snapshots(&make("b"), &make("l"), &make("r"))[0].conflict_uuid
        );
    }

    #[test]
    fn marker_replacement_is_atomic_and_changes_generation() {
        let directory = std::env::temp_dir().join(format!("megle-root-marker-{}", Uuid::now_v7()));
        fs::create_dir_all(&directory).unwrap();
        write_root_marker(&directory, &marker(1)).unwrap();
        replace_root_marker(&directory, &marker(2)).unwrap();
        assert_eq!(read_root_marker(&directory).unwrap(), marker(2));
        fs::remove_file(directory.join(ROOT_MARKER_FILE_NAME)).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn materialization_preflight_accepts_only_its_own_marker_on_retry() {
        let base = std::env::temp_dir().join(format!("megle-root-preflight-{}", Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let target = base.join("Photos");
        let planned =
            validate_root_path(&target, RootPathMode::MaterializeRemote, &base, &[]).unwrap();
        let expected = marker(1);
        write_root_marker(&target, &expected).unwrap();
        let current =
            validate_root_path(&target, RootPathMode::MaterializeRemote, &base, &[]).unwrap();
        assert!(preflight_evidence_still_matches(
            &planned,
            &current,
            Some(&expected)
        ));
        assert!(!preflight_evidence_still_matches(
            &planned,
            &current,
            Some(&marker(2))
        ));
        fs::remove_file(target.join(ROOT_MARKER_FILE_NAME)).unwrap();
        fs::remove_dir(target).unwrap();
        fs::remove_dir(base).unwrap();
    }

    #[test]
    fn enrollment_refuses_a_directory_already_claimed_by_a_sync_marker() {
        let base = std::env::temp_dir().join(format!("megle-root-enroll-{}", Uuid::now_v7()));
        let target = base.join("Photos");
        fs::create_dir_all(&target).unwrap();
        write_root_marker(&target, &marker(1)).unwrap();
        let error =
            validate_root_path(&target, RootPathMode::EnrollExisting, &base, &[]).unwrap_err();
        assert!(error.to_string().contains("marker-mismatch"));
        fs::remove_file(target.join(ROOT_MARKER_FILE_NAME)).unwrap();
        fs::remove_dir(target).unwrap();
        fs::remove_dir(base).unwrap();
    }

    #[test]
    fn canonical_snapshot_uses_protocol_content_identity_and_root_scope() {
        use crate::sync::reducer::{ContentRevision, EntityTruth, LocationRevision, SyncRootTruth};
        use crate::sync::types::{
            ActorEpochUuid, DeviceUuid, EntityUuid, EventUuid, Hlc, RevisionUuid, Version,
        };

        let root_uuid = EntityUuid(Uuid::from_u128(201));
        let root_folder_uuid = EntityUuid(Uuid::from_u128(202));
        let asset_uuid = EntityUuid(Uuid::from_u128(203));
        let other_asset_uuid = EntityUuid(Uuid::from_u128(204));
        let version = Version {
            hlc: Hlc {
                wall_ms: 10,
                logical: 0,
            },
            device_uuid: DeviceUuid(Uuid::from_u128(205)),
            actor_epoch: ActorEpochUuid(Uuid::from_u128(206)),
            actor_seq: 1,
            event_uuid: EventUuid(Uuid::from_u128(207)),
        };
        let location_revision = RevisionUuid(Uuid::from_u128(208));
        let content_revision = RevisionUuid(Uuid::from_u128(209));
        let mut state = crate::sync::reducer::ProtocolState::default();
        state.roots.insert(
            root_uuid,
            SyncRootTruth {
                root_uuid,
                root_folder_uuid,
                library_root_uuid: EntityUuid(Uuid::from_u128(200)),
                creation_event_uuid: None,
                winning_location_revision_uuid: None,
                requested_name: "Photos".into(),
                materialized_name: "Photos".into(),
                materialization_fingerprint: [1; 32],
                lifecycle: "active".into(),
                logical_revision: 1,
            },
        );
        state
            .entities
            .insert(root_folder_uuid, EntityTruth::default());
        let mut asset = EntityTruth::default();
        asset.locations.insert(
            location_revision,
            LocationRevision {
                revision_uuid: location_revision,
                parent_uuid: Some(root_folder_uuid),
                name: "photo.jpg".into(),
                version: version.clone(),
            },
        );
        asset.location_heads.insert(location_revision);
        asset.contents.insert(
            content_revision,
            ContentRevision {
                revision_uuid: content_revision,
                manifest_id: "same-content".into(),
                manifest_pack_id: None,
                manifest_offset: None,
                manifest_length: None,
                byte_length: None,
                source_mtime_ms: None,
                version,
            },
        );
        asset.content_heads.insert(content_revision);
        state.entities.insert(asset_uuid, asset);
        state
            .entities
            .insert(other_asset_uuid, EntityTruth::default());

        let snapshot =
            snapshot_from_protocol_state(&state, root_uuid.0, serde_json::json!({})).unwrap();
        assert!(snapshot.entities.contains_key(&asset_uuid.to_string()));
        assert!(!snapshot
            .entities
            .contains_key(&other_asset_uuid.to_string()));
        assert!(snapshot.entities[&asset_uuid.to_string()]["content"]
            .contains(&content_revision.to_string()));
        assert_eq!(
            snapshot.entities[&root_folder_uuid.to_string()]["root:requested-name"],
            "Photos"
        );
    }

    #[test]
    fn delete_versus_edit_is_a_conflict() {
        let snapshot = |existence: &str, note: &str| {
            RootSnapshot::from_entities(
                "root".into(),
                serde_json::json!({}),
                BTreeMap::from([(
                    "entity".into(),
                    BTreeMap::from([
                        ("existence".into(), existence.into()),
                        ("field:note".into(), note.into()),
                    ]),
                )]),
            )
            .unwrap()
        };
        let differences = compare_snapshots(
            &snapshot("live", "base"),
            &snapshot("deleted", "base"),
            &snapshot("live", "remote edit"),
        );
        assert!(differences
            .iter()
            .filter(|item| matches!(item.field_name.as_str(), "existence" | "field:note"))
            .all(|item| item.classification == "conflict"));
    }
}
