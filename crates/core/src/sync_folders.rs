use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelocationEvidence {
    pub source_path: String,
    pub source_canonical_key: String,
    pub source_filesystem_identity: String,
    pub source_digest: String,
    #[serde(default)]
    pub source_bytes: u64,
    pub target_path: String,
    pub target_canonical_key: String,
    pub target_parent_identity: String,
    pub same_volume: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelocationTransferProgress {
    pub staging_path: Option<String>,
    pub verification_digest: Option<String>,
    pub transferred_bytes: u64,
    pub checkpoint: RelocationTransferCheckpoint,
    pub complete: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelocationTransferCheckpoint {
    #[serde(default)]
    pub completed_entries: Vec<RelocationCompletedEntry>,
    pub current_file: Option<RelocationFileCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelocationCompletedEntry {
    pub relative_path: String,
    pub length: u64,
    pub source_modified_nanos: u128,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelocationFileCursor {
    pub relative_path: String,
    pub offset: u64,
    pub length: u64,
    pub source_modified_nanos: u128,
}

const RELOCATION_COPY_BATCH_ENTRIES: usize = 16;
const RELOCATION_COPY_BATCH_BYTES: u64 = 64 * 1024 * 1024;

pub fn validate_relocation_paths(
    source: &Path,
    target: &Path,
    placement_base: &Path,
    other_bound_paths: &[PathBuf],
    expected_marker: &SyncRootMarker,
) -> anyhow::Result<RelocationEvidence> {
    let source = normalize_absolute(source).map_err(|_| anyhow::anyhow!("invalid-path"))?;
    let target = normalize_absolute(target).map_err(|_| anyhow::anyhow!("invalid-path"))?;
    validate_platform_path(&target)?;
    if canonical_path_key(&source) == canonical_path_key(&target) {
        bail!("invalid-path");
    }
    if read_root_marker(&source)? != *expected_marker {
        bail!("marker-mismatch");
    }
    if has_existing_reparse_component(&source) || has_existing_reparse_component(&target) {
        bail!("reparse-point");
    }
    if canonical_path_key(&target) == canonical_path_key(placement_base)
        || paths_overlap(&source, &target)
        || other_bound_paths
            .iter()
            .any(|path| paths_overlap(&target, path))
    {
        bail!("overlaps-active-root");
    }
    if known_cloud_roots()
        .iter()
        .any(|path| paths_overlap(&target, path))
        || is_platform_sync_root(&target)
    {
        bail!("cloud-sync-root");
    }
    if target.exists() {
        bail!("destination-exists");
    }
    let target_parent = target
        .parent()
        .filter(|path| path.is_dir())
        .context("path-unavailable")?;
    if !probe_writable(target_parent) {
        bail!("path-not-writable");
    }
    let source_filesystem_identity = local_file_identity(&source).context("source-missing")?;
    let target_parent_identity = local_file_identity(target_parent).context("path-unavailable")?;
    let same_volume = same_volume_identity(&source_filesystem_identity, &target_parent_identity);
    // Same-volume rename is guarded by open directory identities and no-replace
    // semantics. Avoid walking and hashing the entire tree on this O(1) path.
    let (source_digest, source_bytes) = if same_volume {
        (format!("identity:{source_filesystem_identity}"), 0)
    } else {
        (
            directory_evidence_digest(&source)?,
            directory_evidence_size(&source)?,
        )
    };
    if !same_volume && free_space_bytes(target_parent).context("path-unavailable")? < source_bytes {
        bail!("insufficient-space");
    }
    Ok(RelocationEvidence {
        source_path: source.to_string_lossy().into_owned(),
        source_canonical_key: canonical_path_key(&source),
        source_filesystem_identity,
        source_digest,
        source_bytes,
        target_path: target.to_string_lossy().into_owned(),
        target_canonical_key: canonical_path_key(&target),
        target_parent_identity: target_parent_identity.clone(),
        same_volume,
    })
}

pub fn capture_relocation_retry_evidence(
    source: &Path,
    target: &Path,
    expected_marker: &SyncRootMarker,
) -> anyhow::Result<RelocationEvidence> {
    let source = normalize_absolute(source).map_err(|_| anyhow::anyhow!("invalid-path"))?;
    let target = normalize_absolute(target).map_err(|_| anyhow::anyhow!("invalid-path"))?;
    validate_platform_path(&target)?;
    if read_root_marker(&source)? != *expected_marker {
        bail!("marker-mismatch");
    }
    let target_parent = target
        .parent()
        .filter(|path| path.is_dir())
        .context("path-unavailable")?;
    let source_filesystem_identity = local_file_identity(&source).context("source-missing")?;
    let target_parent_identity = local_file_identity(target_parent).context("path-unavailable")?;
    let same_volume = same_volume_identity(&source_filesystem_identity, &target_parent_identity);
    let (source_digest, source_bytes) = if same_volume {
        (format!("identity:{source_filesystem_identity}"), 0)
    } else {
        (
            directory_evidence_digest(&source)?,
            directory_evidence_size(&source)?,
        )
    };
    Ok(RelocationEvidence {
        source_path: source.to_string_lossy().into_owned(),
        source_canonical_key: canonical_path_key(&source),
        source_filesystem_identity,
        source_digest,
        source_bytes,
        target_path: target.to_string_lossy().into_owned(),
        target_canonical_key: canonical_path_key(&target),
        target_parent_identity,
        same_volume,
    })
}

fn same_volume_identity(source_identity: &str, target_parent_identity: &str) -> bool {
    fn volume_key(identity: &str) -> Option<(&str, &str)> {
        let mut components = identity.split(':');
        Some((components.next()?, components.next()?))
    }
    volume_key(source_identity) == volume_key(target_parent_identity)
}

pub fn transfer_relocation_tree(
    evidence: &RelocationEvidence,
    expected_marker: &SyncRootMarker,
    operation_uuid: &str,
    checkpoint: RelocationTransferCheckpoint,
) -> anyhow::Result<RelocationTransferProgress> {
    let source = Path::new(&evidence.source_path);
    let target = Path::new(&evidence.target_path);
    if target.exists() {
        if let Ok(marker) = read_root_marker(target) {
            let same_root = marker.library_uuid == expected_marker.library_uuid
                && marker.root_uuid == expected_marker.root_uuid
                && marker.root_folder_uuid == expected_marker.root_folder_uuid
                && marker.local_binding_uuid == expected_marker.local_binding_uuid
                && matches!(
                    marker.binding_generation,
                    generation if generation == expected_marker.binding_generation
                        || generation == expected_marker.binding_generation.saturating_add(1)
                );
            let target_identity_matches = !evidence.same_volume
                || local_file_identity(target).ok().as_deref()
                    == Some(evidence.source_filesystem_identity.as_str());
            let target_digest = if evidence.same_volume && target_identity_matches {
                evidence.source_digest.clone()
            } else {
                directory_evidence_digest(target)?
            };
            if same_root && target_identity_matches && target_digest == evidence.source_digest {
                return Ok(RelocationTransferProgress {
                    staging_path: None,
                    verification_digest: Some(target_digest),
                    transferred_bytes: evidence.source_bytes,
                    checkpoint,
                    complete: true,
                });
            }
        }
        bail!("destination-exists");
    }
    if !source.is_dir()
        || local_file_identity(source).ok().as_deref() != Some(&evidence.source_filesystem_identity)
    {
        bail!("source-missing");
    }
    if read_root_marker(source)? != *expected_marker {
        bail!("marker-mismatch");
    }
    let target_parent = target.parent().context("path-unavailable")?;
    if local_file_identity(target_parent).ok().as_deref() != Some(&evidence.target_parent_identity)
    {
        bail!("path-changed");
    }
    if evidence.same_volume {
        crate::fsops::rename_verified_directory_no_replace(
            source,
            target,
            &evidence.source_filesystem_identity,
            &evidence.target_parent_identity,
        )
        .context("transfer-failed")?;
        return Ok(RelocationTransferProgress {
            staging_path: None,
            verification_digest: Some(evidence.source_digest.clone()),
            transferred_bytes: 0,
            checkpoint,
            complete: true,
        });
    }
    let staging = target.with_file_name(format!(
        ".{}.megle-relocation-{}",
        target
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("root"),
        operation_uuid
    ));
    if staging.exists() {
        match read_root_marker(&staging) {
            Ok(marker) if marker == *expected_marker => {}
            Err(_) if fs::read_dir(&staging)?.next().is_none() => {
                write_root_marker(&staging, expected_marker).context("transfer-failed")?;
            }
            _ => bail!("target-conflict"),
        }
    } else {
        fs::create_dir(&staging).context("transfer-failed")?;
        write_root_marker(&staging, expected_marker).context("transfer-failed")?;
    }
    let staging_filesystem_identity = local_file_identity(&staging).context("transfer-failed")?;
    let copy = copy_directory_resumable_bounded(source, &staging, checkpoint)?;
    if !copy.complete {
        return Ok(RelocationTransferProgress {
            staging_path: Some(staging.to_string_lossy().into_owned()),
            verification_digest: None,
            transferred_bytes: copy.transferred_bytes,
            checkpoint: copy.checkpoint,
            complete: false,
        });
    }
    let source_digest = directory_evidence_digest(source)?;
    let digest = directory_evidence_digest(&staging)?;
    if source_digest != evidence.source_digest {
        bail!("source-changed");
    }
    if digest != evidence.source_digest {
        // The staging tree is operation-owned and its marker was validated
        // above. A crash may durably persist the checkpoint while losing or
        // corrupting a directory entry. Reset that private tree so retry makes
        // progress instead of looping forever on the stale checkpoint.
        crate::library_path::safe_delete_tree_expected(&staging, &staging_filesystem_identity)
            .context("transfer-failed")?;
        fs::create_dir(&staging).context("transfer-failed")?;
        write_root_marker(&staging, expected_marker).context("transfer-failed")?;
        SafeDirectory::open(target_parent)
            .context("transfer-failed")?
            .sync_all()
            .context("transfer-failed")?;
        return Ok(RelocationTransferProgress {
            staging_path: Some(staging.to_string_lossy().into_owned()),
            verification_digest: None,
            transferred_bytes: 0,
            checkpoint: RelocationTransferCheckpoint::default(),
            complete: false,
        });
    }
    SafeDirectory::open(&staging)
        .context("transfer-failed")?
        .sync_all()
        .context("transfer-failed")?;
    crate::fsops::rename_verified_directory_no_replace(
        &staging,
        target,
        &staging_filesystem_identity,
        &evidence.target_parent_identity,
    )
    .context("transfer-failed")?;
    SafeDirectory::open(target_parent)
        .context("transfer-failed")?
        .sync_all()
        .context("transfer-failed")?;
    Ok(RelocationTransferProgress {
        staging_path: Some(staging.to_string_lossy().into_owned()),
        verification_digest: Some(digest),
        transferred_bytes: evidence.source_bytes,
        checkpoint: copy.checkpoint,
        complete: true,
    })
}

pub fn remove_relocation_source_if_unchanged(
    source: &Path,
    expected_identity: &str,
    expected_digest: &str,
    expected_marker: &SyncRootMarker,
) -> anyhow::Result<()> {
    if !source.exists() {
        return Ok(());
    }
    if !source.is_dir()
        || local_file_identity(source).ok().as_deref() != Some(expected_identity)
        || read_root_marker(source).ok().as_ref() != Some(expected_marker)
        || directory_evidence_digest(source).ok().as_deref() != Some(expected_digest)
    {
        bail!("cleanup-source-changed");
    }
    crate::library_path::safe_delete_tree_expected(source, expected_identity)
        .context("cleanup-failed")
}

struct CopyBatchProgress {
    transferred_bytes: u64,
    checkpoint: RelocationTransferCheckpoint,
    complete: bool,
}

fn copy_directory_resumable_bounded(
    source: &Path,
    target: &Path,
    mut checkpoint: RelocationTransferCheckpoint,
) -> anyhow::Result<CopyBatchProgress> {
    let mut entries = Vec::new();
    collect_copy_entries(source, source, &mut entries)?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut transferred_bytes = 0_u64;
    let completed = checkpoint
        .completed_entries
        .iter()
        .map(|entry| (entry.relative_path.clone(), entry.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut copied_entries = 0_usize;
    let mut copied_bytes = 0_u64;
    for (relative, source_path, metadata) in entries {
        let target_path = target.join(&relative);
        if has_existing_reparse_component(&target_path) {
            bail!("reparse-point");
        }
        if metadata.is_dir() {
            fs::create_dir_all(&target_path)?;
            continue;
        }
        let relative = relative.to_string_lossy().replace('\\', "/");
        let source_modified_nanos = modified_nanos(&metadata);
        if let Some(entry) = completed.get(&relative) {
            if entry.length != metadata.len()
                || entry.source_modified_nanos != source_modified_nanos
            {
                bail!("verification-failed");
            }
            if target_path
                .metadata()
                .ok()
                .is_some_and(|target_metadata| target_metadata.len() == entry.length)
            {
                transferred_bytes = transferred_bytes.saturating_add(metadata.len());
                continue;
            }
            checkpoint
                .completed_entries
                .retain(|candidate| candidate.relative_path != relative);
            if checkpoint
                .current_file
                .as_ref()
                .is_some_and(|cursor| cursor.relative_path == relative)
            {
                checkpoint.current_file = None;
            }
        }
        if copied_entries >= RELOCATION_COPY_BATCH_ENTRIES
            || copied_bytes >= RELOCATION_COPY_BATCH_BYTES
        {
            return Ok(CopyBatchProgress {
                transferred_bytes,
                checkpoint,
                complete: false,
            });
        }
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut offset = checkpoint
            .current_file
            .as_ref()
            .filter(|cursor| {
                cursor.relative_path == relative
                    && cursor.length == metadata.len()
                    && cursor.source_modified_nanos == source_modified_nanos
            })
            .map_or(0, |cursor| cursor.offset);
        if target_path.metadata().ok().map(|value| value.len()) != Some(offset) {
            offset = 0;
        }
        let remaining_budget = RELOCATION_COPY_BATCH_BYTES.saturating_sub(copied_bytes);
        let chunk_length = metadata.len().saturating_sub(offset).min(remaining_budget);
        let mut input = fs::File::open(&source_path).context("transfer-failed")?;
        input.seek(SeekFrom::Start(offset))?;
        let mut output = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(offset == 0)
            .open(&target_path)
            .context("transfer-failed")?;
        output.seek(SeekFrom::Start(offset))?;
        let copied = std::io::copy(&mut input.take(chunk_length), &mut output)?;
        output.sync_all()?;
        if copied != chunk_length {
            bail!("source-changed");
        }
        offset = offset.saturating_add(copied);
        copied_bytes = copied_bytes.saturating_add(copied);
        transferred_bytes = transferred_bytes.saturating_add(offset);
        if offset < metadata.len() {
            checkpoint.current_file = Some(RelocationFileCursor {
                relative_path: relative,
                offset,
                length: metadata.len(),
                source_modified_nanos,
            });
            return Ok(CopyBatchProgress {
                transferred_bytes,
                checkpoint,
                complete: false,
            });
        }
        let source_hash = file_content_digest(&source_path)?;
        if source_hash != file_content_digest(&target_path)? {
            bail!("verification-failed");
        }
        checkpoint.completed_entries.push(RelocationCompletedEntry {
            relative_path: relative,
            length: metadata.len(),
            source_modified_nanos,
            sha256: source_hash,
        });
        checkpoint.current_file = None;
        copied_entries += 1;
    }
    Ok(CopyBatchProgress {
        transferred_bytes,
        checkpoint,
        complete: true,
    })
}

fn modified_nanos(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |value| value.as_nanos())
}

fn collect_copy_entries(
    base: &Path,
    current: &Path,
    entries: &mut Vec<(PathBuf, PathBuf, fs::Metadata)>,
) -> anyhow::Result<()> {
    for entry in fs::read_dir(current).context("transfer-failed")? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || metadata_is_reparse(&metadata) {
            bail!("reparse-point");
        }
        let relative = entry.path().strip_prefix(base)?.to_path_buf();
        if metadata.is_dir() {
            entries.push((relative, entry.path(), metadata));
            collect_copy_entries(base, &entry.path(), entries)?;
        } else {
            entries.push((relative, entry.path(), metadata));
        }
    }
    Ok(())
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
    validate_platform_path(&normalized)?;
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

#[cfg(not(windows))]
fn validate_platform_path(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn validate_platform_path(path: &Path) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Component;

    for component in path.components() {
        let Component::Normal(component) = component else {
            continue;
        };
        let value = component.to_string_lossy();
        if value.is_empty()
            || value.ends_with(['.', ' '])
            || value
                .chars()
                .any(|character| character < ' ' || r#"<>:\"/\\|?*"#.contains(character))
            || is_windows_device_component(&value)
            || value.encode_utf16().count() > 255
        {
            bail!("invalid-path");
        }
    }
    let utf16_length = path.as_os_str().encode_wide().count();
    if utf16_length >= 32_767 || (utf16_length >= 260 && !windows_long_paths_enabled()) {
        bail!("invalid-path");
    }
    Ok(())
}

#[cfg(windows)]
fn is_windows_device_component(value: &str) -> bool {
    let stem = value
        .trim_end_matches(['.', ' '])
        .split('.')
        .next()
        .unwrap_or(value)
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')
                || matches!(suffix, "¹" | "²" | "³")
        })
}

#[cfg(windows)]
fn windows_long_paths_enabled() -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD,
    };

    let key_name = std::ffi::OsStr::new(r"SYSTEM\CurrentControlSet\Control\FileSystem")
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let value_name = std::ffi::OsStr::new("LongPathsEnabled")
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut key = std::ptr::null_mut();
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, key_name.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
        return false;
    }
    let mut value = 0_u32;
    let mut value_type = 0_u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let result = unsafe {
        RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    unsafe { RegCloseKey(key) };
    result == 0 && value_type == REG_DWORD && value != 0
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

pub fn directory_evidence_size(path: &Path) -> anyhow::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut entries = Vec::new();
    collect_directory_evidence(path, path, &mut entries, 0)?;
    entries.into_iter().try_fold(0_u64, |total, entry| {
        let size = entry
            .split('\0')
            .nth(2)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        total
            .checked_add(size)
            .ok_or_else(|| anyhow::anyhow!("directory-inventory-too-large"))
    })
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
        let kind = if metadata.is_dir() { "d" } else { "f" };
        let content_digest = if metadata.is_file() {
            file_content_digest(&entry.path())?
        } else {
            String::new()
        };
        output.push(format!(
            "{}\0{}\0{}\0{}",
            relative,
            kind,
            metadata.len(),
            content_digest
        ));
        if metadata.is_dir() {
            collect_directory_evidence(base, &entry.path(), output, depth + 1)?;
        }
    }
    Ok(())
}

fn file_content_digest(path: &Path) -> anyhow::Result<String> {
    let mut file = fs::File::open(path).context("path-unavailable")?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).context("path-unavailable")?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(&digest.finalize()))
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
    fn relocation_preflight_rejects_overlap_and_preserves_source_identity() {
        let base = std::env::temp_dir().join(format!("megle-relocation-{}", Uuid::now_v7()));
        let source = base.join("Photos");
        let target = base.join("Moved");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested").join("one.txt"), b"one").unwrap();
        let expected = marker(1);
        write_root_marker(&source, &expected).unwrap();
        let evidence = validate_relocation_paths(&source, &target, &base, &[], &expected).unwrap();
        assert_eq!(
            evidence.source_filesystem_identity,
            local_file_identity(&source).unwrap()
        );
        let overlap = validate_relocation_paths(
            &source,
            &source.join("nested").join("Moved"),
            &base,
            &[],
            &expected,
        )
        .unwrap_err();
        assert!(overlap.to_string().contains("overlaps-active-root"));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn same_volume_relocation_moves_one_tree_without_changing_marker_identity_fields() {
        let base = std::env::temp_dir().join(format!("megle-relocation-{}", Uuid::now_v7()));
        let source = base.join("Photos");
        let target = base.join("Renamed");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested").join("one.txt"), b"one").unwrap();
        let expected = marker(1);
        write_root_marker(&source, &expected).unwrap();
        let evidence = validate_relocation_paths(&source, &target, &base, &[], &expected).unwrap();
        assert!(evidence.same_volume);
        let transfer = transfer_relocation_tree(
            &evidence,
            &expected,
            &Uuid::now_v7().to_string(),
            RelocationTransferCheckpoint::default(),
        )
        .unwrap();
        assert!(!source.exists());
        assert!(target.join("nested").join("one.txt").is_file());
        assert_eq!(
            transfer.verification_digest.as_deref(),
            Some(evidence.source_digest.as_str())
        );
        assert_eq!(read_root_marker(&target).unwrap(), expected);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn same_volume_resume_rejects_a_replaced_target_with_a_copied_marker() {
        let base = std::env::temp_dir().join(format!("megle-relocation-{}", Uuid::now_v7()));
        let source = base.join("Photos");
        let target = base.join("Renamed");
        let displaced = base.join("Displaced");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("one.txt"), b"one").unwrap();
        let expected = marker(1);
        write_root_marker(&source, &expected).unwrap();
        let evidence = validate_relocation_paths(&source, &target, &base, &[], &expected).unwrap();
        assert!(evidence.same_volume);

        fs::rename(&source, &target).unwrap();
        fs::rename(&target, &displaced).unwrap();
        fs::create_dir(&target).unwrap();
        write_root_marker(&target, &expected).unwrap();

        let error = transfer_relocation_tree(
            &evidence,
            &expected,
            &Uuid::now_v7().to_string(),
            RelocationTransferCheckpoint::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("destination-exists"));
        assert!(target.is_dir());
        assert!(displaced.join("one.txt").is_file());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relocation_rejects_source_content_changed_after_preflight() {
        let base = std::env::temp_dir().join(format!("megle-relocation-{}", Uuid::now_v7()));
        let source = base.join("Photos");
        let target = base.join("Renamed");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("one.txt"), b"good").unwrap();
        let expected = marker(1);
        write_root_marker(&source, &expected).unwrap();
        let evidence = validate_relocation_paths(&source, &target, &base, &[], &expected).unwrap();

        fs::write(source.join("one.txt"), b"evil").unwrap();
        if evidence.same_volume {
            let transfer = transfer_relocation_tree(
                &evidence,
                &expected,
                &Uuid::now_v7().to_string(),
                RelocationTransferCheckpoint::default(),
            )
            .unwrap();
            assert!(transfer.complete);
            assert!(target.is_dir());
        } else {
            let error = transfer_relocation_tree(
                &evidence,
                &expected,
                &Uuid::now_v7().to_string(),
                RelocationTransferCheckpoint::default(),
            )
            .unwrap_err();
            assert!(error.to_string().contains("verification-failed"));
            assert!(source.is_dir());
            assert!(!target.exists());
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relocation_cleanup_preserves_source_with_unexpected_content() {
        let base = std::env::temp_dir().join(format!("megle-cleanup-{}", Uuid::now_v7()));
        let source = base.join("Photos");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("one.txt"), b"one").unwrap();
        let expected = marker(1);
        write_root_marker(&source, &expected).unwrap();
        let identity = local_file_identity(&source).unwrap();
        let digest = directory_evidence_digest(&source).unwrap();

        fs::write(source.join("unexpected.txt"), b"keep me").unwrap();
        let error = remove_relocation_source_if_unchanged(&source, &identity, &digest, &expected)
            .unwrap_err();

        assert!(error.to_string().contains("cleanup-source-changed"));
        assert!(source.join("one.txt").is_file());
        assert!(source.join("unexpected.txt").is_file());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn cross_volume_resume_claims_an_empty_operation_staging_directory() {
        let base = std::env::temp_dir().join(format!("megle-relocation-{}", Uuid::now_v7()));
        let source = base.join("Photos");
        let target = base.join("Moved");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("one.txt"), b"one").unwrap();
        let expected = marker(1);
        write_root_marker(&source, &expected).unwrap();
        let mut evidence =
            validate_relocation_paths(&source, &target, &base, &[], &expected).unwrap();
        evidence.same_volume = false;
        evidence.source_digest = directory_evidence_digest(&source).unwrap();
        evidence.source_bytes = directory_evidence_size(&source).unwrap();
        let operation_uuid = Uuid::now_v7().to_string();
        let staging = target.with_file_name(format!(
            ".{}.megle-relocation-{operation_uuid}",
            target.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir(&staging).unwrap();

        let transfer = transfer_relocation_tree(
            &evidence,
            &expected,
            &operation_uuid,
            RelocationTransferCheckpoint::default(),
        )
        .unwrap();

        assert_eq!(
            transfer.staging_path.as_deref(),
            Some(staging.to_string_lossy().as_ref())
        );
        assert_eq!(
            transfer.verification_digest.as_deref(),
            Some(evidence.source_digest.as_str())
        );
        assert!(source.is_dir());
        assert!(target.join("one.txt").is_file());
        assert_eq!(read_root_marker(&target).unwrap(), expected);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn directory_evidence_detects_same_size_content_changes() {
        let base = std::env::temp_dir().join(format!("megle-evidence-{}", Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let file = base.join("one.txt");
        fs::write(&file, b"good").unwrap();
        let before = directory_evidence_digest(&base).unwrap();
        fs::write(&file, b"evil").unwrap();
        let after = directory_evidence_digest(&base).unwrap();
        assert_ne!(before, after);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn resumable_copy_repairs_same_size_corruption() {
        let base = std::env::temp_dir().join(format!("megle-copy-resume-{}", Uuid::now_v7()));
        let source = base.join("source");
        let target = base.join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("one.txt"), b"good").unwrap();
        fs::write(target.join("one.txt"), b"evil").unwrap();

        let progress = copy_directory_resumable_bounded(
            &source,
            &target,
            RelocationTransferCheckpoint::default(),
        )
        .unwrap();

        assert!(progress.complete);
        assert_eq!(fs::read(target.join("one.txt")).unwrap(), b"good");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn resumable_copy_is_bounded_and_advances_on_retry() {
        let base = std::env::temp_dir().join(format!("megle-copy-bounded-{}", Uuid::now_v7()));
        let source = base.join("source");
        let target = base.join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        for index in 0..(RELOCATION_COPY_BATCH_ENTRIES + 3) {
            fs::write(
                source.join(format!("{index:03}.txt")),
                format!("file-{index}"),
            )
            .unwrap();
        }

        let first = copy_directory_resumable_bounded(
            &source,
            &target,
            RelocationTransferCheckpoint::default(),
        )
        .unwrap();
        assert!(!first.complete);
        assert_eq!(
            first.checkpoint.completed_entries.len(),
            RELOCATION_COPY_BATCH_ENTRIES
        );
        let second = copy_directory_resumable_bounded(&source, &target, first.checkpoint).unwrap();
        assert!(second.complete);
        assert_eq!(
            fs::read_dir(&target).unwrap().count(),
            RELOCATION_COPY_BATCH_ENTRIES + 3
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn resumable_copy_recopies_a_completed_entry_lost_after_checkpoint() {
        let base = std::env::temp_dir().join(format!("megle-copy-lost-{}", Uuid::now_v7()));
        let source = base.join("source");
        let target = base.join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(source.join("one.txt"), b"one").unwrap();
        let first = copy_directory_resumable_bounded(
            &source,
            &target,
            RelocationTransferCheckpoint::default(),
        )
        .unwrap();
        assert!(first.complete);
        fs::remove_file(target.join("one.txt")).unwrap();

        let repaired =
            copy_directory_resumable_bounded(&source, &target, first.checkpoint).unwrap();

        assert!(repaired.complete);
        assert_eq!(fs::read(target.join("one.txt")).unwrap(), b"one");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relocation_resets_corrupt_operation_staging_instead_of_retry_looping() {
        let base = std::env::temp_dir().join(format!("megle-copy-reset-{}", Uuid::now_v7()));
        let source = base.join("source");
        let target = base.join("target");
        fs::create_dir_all(&source).unwrap();
        let expected = marker(1);
        write_root_marker(&source, &expected).unwrap();
        for index in 0..(RELOCATION_COPY_BATCH_ENTRIES + 3) {
            fs::write(source.join(format!("{index:03}.txt")), b"good").unwrap();
        }
        let mut evidence =
            validate_relocation_paths(&source, &target, &base, &[], &expected).unwrap();
        evidence.same_volume = false;
        evidence.source_digest = directory_evidence_digest(&source).unwrap();
        evidence.source_bytes = directory_evidence_size(&source).unwrap();
        let operation_uuid = Uuid::now_v7().to_string();
        let first = transfer_relocation_tree(
            &evidence,
            &expected,
            &operation_uuid,
            RelocationTransferCheckpoint::default(),
        )
        .unwrap();
        assert!(!first.complete);
        let staging = PathBuf::from(first.staging_path.unwrap());
        let completed = first
            .checkpoint
            .completed_entries
            .iter()
            .find(|entry| entry.relative_path != ROOT_MARKER_FILE_NAME)
            .unwrap()
            .relative_path
            .clone();
        fs::write(staging.join(&completed), b"evil").unwrap();

        let reset =
            transfer_relocation_tree(&evidence, &expected, &operation_uuid, first.checkpoint)
                .unwrap();

        assert!(!reset.complete);
        assert!(reset.checkpoint.completed_entries.is_empty());
        assert!(reset.checkpoint.current_file.is_none());
        assert_eq!(read_root_marker(&staging).unwrap(), expected);
        assert_eq!(fs::read_dir(&staging).unwrap().count(), 1);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn resumable_copy_persists_a_large_file_offset() {
        let base = std::env::temp_dir().join(format!("megle-copy-cursor-{}", Uuid::now_v7()));
        let source = base.join("source");
        let target = base.join("target");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&target).unwrap();
        let large = source.join("large.bin");
        let file = fs::File::create(&large).unwrap();
        file.set_len(RELOCATION_COPY_BATCH_BYTES + 4096).unwrap();

        let first = copy_directory_resumable_bounded(
            &source,
            &target,
            RelocationTransferCheckpoint::default(),
        )
        .unwrap();
        assert!(!first.complete);
        assert_eq!(
            first.checkpoint.current_file.as_ref().unwrap().offset,
            RELOCATION_COPY_BATCH_BYTES
        );
        let second = copy_directory_resumable_bounded(&source, &target, first.checkpoint).unwrap();
        assert!(second.complete);
        assert_eq!(
            target.join("large.bin").metadata().unwrap().len(),
            RELOCATION_COPY_BATCH_BYTES + 4096
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_reject_device_names_trailing_space_and_unsupported_length() {
        let base = std::env::temp_dir();
        assert!(validate_platform_path(&base.join("CON.txt")).is_err());
        assert!(validate_platform_path(&base.join("CONIN$.txt")).is_err());
        assert!(validate_platform_path(&base.join("COM¹.txt")).is_err());
        assert!(validate_platform_path(&base.join("folder. ")).is_err());
        let long = base.join("x".repeat(32_768));
        assert!(validate_platform_path(&long).is_err());
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
