use super::format::ContentManifestV1;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::fs;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaterializationResult {
    Published(PathBuf),
    AdoptedIdentical(PathBuf),
    PathConflict(PathBuf),
}

pub fn windows_safe_component(name: &str, stable_suffix: &str) -> String {
    let mut value = name
        .chars()
        .map(|character| {
            if matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            ) || character.is_control()
            {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    value = value.trim_end_matches([' ', '.']).to_owned();
    if value.is_empty() {
        value.push('_');
    }
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if reserved {
        value.insert(0, '_');
    }
    if value.encode_utf16().count() > 255 {
        let mut units = 0;
        let mut shortened = value
            .chars()
            .take_while(|character| {
                units += character.len_utf16();
                units <= 242
            })
            .collect::<String>();
        shortened.push('~');
        shortened.push_str(&stable_suffix.chars().take(12).collect::<String>());
        value = shortened;
    }
    value
}

pub fn windows_safe_relative_path(path: &Path, stable_suffix: &str) -> anyhow::Result<PathBuf> {
    inventory_path(path)?;
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(value) => safe.push(windows_safe_component(
                &value.to_string_lossy(),
                stable_suffix,
            )),
            std::path::Component::CurDir => {}
            _ => anyhow::bail!("materialization path contains invalid component"),
        }
    }
    inventory_path(&safe)?;
    Ok(safe)
}

pub fn materialize_from_chunks(
    managed_root: &Path,
    destination_relative: &Path,
    job_id: uuid::Uuid,
    manifest: &ContentManifestV1,
    chunks: &BTreeMap<[u8; 32], Vec<u8>>,
    source_mtime_ms: Option<i64>,
) -> anyhow::Result<MaterializationResult> {
    inventory_path(destination_relative)?;
    manifest.validate()?;
    let mut owned_staging = create_unregistered_staging(managed_root, job_id)?;
    crate::sync::file_policy::require_no_links(
        managed_root,
        &managed_root.join(destination_relative),
    )?;
    let mut output = owned_staging.payload.take().expect("owned staging file");
    let mut full = Sha256::new();
    let mut written = 0u64;
    for chunk in &manifest.chunks {
        let hash = super::parse_hash(&chunk.chunk_sha256)?;
        let bytes = chunks
            .get(&hash)
            .ok_or_else(|| anyhow::anyhow!("verified chunk is missing"))?;
        if bytes.len() as u64 != chunk.byte_length || Sha256::digest(bytes).as_slice() != hash {
            anyhow::bail!("materialization chunk failed verification");
        }
        output.write_all(bytes)?;
        full.update(bytes);
        written = written
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("materialized length overflow"))?;
    }
    if written != manifest.byte_length
        || full.finalize().as_slice() != super::parse_hash(&manifest.content_sha256)?
    {
        anyhow::bail!("materialized file failed final verification");
    }
    output.sync_all()?;
    finish_materialization(
        managed_root,
        destination_relative,
        &owned_staging,
        &output,
        manifest,
        source_mtime_ms,
    )
}

pub fn materialize_from_verified_ranges(
    managed_root: &Path,
    destination_relative: &Path,
    job_id: uuid::Uuid,
    manifest: &ContentManifestV1,
    chunks: &BTreeMap<[u8; 32], super::recovery::VerifiedChunkSource>,
    source_mtime_ms: Option<i64>,
) -> anyhow::Result<MaterializationResult> {
    materialize_from_verified_ranges_with_pre_publish(
        managed_root,
        destination_relative,
        job_id,
        manifest,
        chunks,
        source_mtime_ms,
        || Ok(()),
    )
}

pub fn materialize_from_verified_ranges_with_pre_publish(
    managed_root: &Path,
    destination_relative: &Path,
    job_id: uuid::Uuid,
    manifest: &ContentManifestV1,
    chunks: &BTreeMap<[u8; 32], super::recovery::VerifiedChunkSource>,
    source_mtime_ms: Option<i64>,
    pre_publish: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<MaterializationResult> {
    materialize_from_verified_ranges_with_checks(
        managed_root,
        destination_relative,
        job_id,
        manifest,
        chunks,
        source_mtime_ms,
        None,
        || Ok(()),
        pre_publish,
    )
}

pub(crate) struct OwnedMaterializationStaging {
    pub directory: PathBuf,
    pub identity: String,
    pub guard: Option<crate::library_path::MarkerDirectoryGuard>,
    pub payload_identity: Option<String>,
    pub payload: Option<File>,
}

impl Drop for OwnedMaterializationStaging {
    fn drop(&mut self) {
        drop(self.payload.take());
        if self.guard.as_ref().is_some_and(|guard| {
            guard.identity() == self.identity && guard.verify_unchanged().is_ok()
        }) {
            if let Some(expected) = self.payload_identity.as_deref() {
                let _ = self
                    .guard
                    .as_ref()
                    .unwrap()
                    .delete_file_child_expected(std::ffi::OsStr::new("payload.partial"), expected);
            }
            drop(self.guard.take());
            // Never remove user additions or a replaced directory recursively.
            let _ = crate::library_path::safe_remove_empty_directory_expected(
                &self.directory,
                &self.identity,
            );
        }
    }
}

fn create_unregistered_staging(
    root: &Path,
    job_id: uuid::Uuid,
) -> anyhow::Result<OwnedMaterializationStaging> {
    let leaf = format!(
        ".mangoo-work-{}-{}",
        job_id.simple(),
        uuid::Uuid::now_v7().simple()
    );
    let parent = crate::library_path::MarkerDirectoryGuard::open(root)?;
    let guard = parent.create_directory_child(std::ffi::OsStr::new(&leaf))?;
    let payload = guard.open_temporary(std::ffi::OsStr::new("payload.partial"), true)?;
    let payload_identity = Some(guard.file_identity(&payload)?);
    Ok(OwnedMaterializationStaging {
        directory: root.join(leaf),
        identity: guard.identity().to_owned(),
        guard: Some(guard),
        payload_identity,
        payload: Some(payload),
    })
}

pub fn materialize_from_verified_ranges_with_checks(
    managed_root: &Path,
    destination_relative: &Path,
    job_id: uuid::Uuid,
    manifest: &ContentManifestV1,
    chunks: &BTreeMap<[u8; 32], super::recovery::VerifiedChunkSource>,
    source_mtime_ms: Option<i64>,
    owned_staging: Option<OwnedMaterializationStaging>,
    mut check_progress: impl FnMut() -> anyhow::Result<()>,
    pre_publish: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<MaterializationResult> {
    #[cfg(test)]
    if managed_root.join(".mangoo-test-disk-full").exists() {
        return Err(std::io::Error::from(std::io::ErrorKind::StorageFull).into());
    }
    inventory_path(destination_relative)?;
    manifest.validate()?;
    check_progress()?;
    let _destination_guards = crate::sync::file_policy::pin_destination_parents(
        managed_root,
        &managed_root.join(destination_relative),
    )?;
    let mut owned_staging = match owned_staging {
        Some(staging) => staging,
        None => create_unregistered_staging(managed_root, job_id)?,
    };
    crate::sync::file_policy::require_no_links(managed_root, &owned_staging.directory)?;
    crate::sync::file_policy::require_no_links(
        managed_root,
        &managed_root.join(destination_relative),
    )?;
    let _staging_guard = owned_staging.guard.as_ref().expect("owned staging guard");
    _staging_guard.verify_unchanged()?;
    let mut output = owned_staging.payload.take().expect("owned staging file");
    let mut full = Sha256::new();
    let mut written = 0u64;
    let mut buffer = vec![0u8; 1024 * 1024];
    for chunk in &manifest.chunks {
        let hash = super::parse_hash(&chunk.chunk_sha256)?;
        let source = chunks
            .get(&hash)
            .ok_or_else(|| anyhow::anyhow!("verified chunk is missing"))?;
        if source.length != chunk.byte_length {
            anyhow::bail!("materialization chunk length mismatch");
        }
        let mut input = File::open(&source.path)?;
        input.seek(SeekFrom::Start(source.offset))?;
        let mut remaining = source.length;
        let mut chunk_hash = Sha256::new();
        while remaining > 0 {
            check_progress()?;
            let requested = usize::try_from(remaining.min(buffer.len() as u64))?;
            let count = input.read(&mut buffer[..requested])?;
            if count == 0 {
                anyhow::bail!("verified range ended before chunk boundary");
            }
            output.write_all(&buffer[..count])?;
            chunk_hash.update(&buffer[..count]);
            full.update(&buffer[..count]);
            remaining -= count as u64;
            written = written
                .checked_add(count as u64)
                .ok_or_else(|| anyhow::anyhow!("materialized length overflow"))?;
        }
        if chunk_hash.finalize().as_slice() != hash {
            anyhow::bail!("materialization chunk failed verification");
        }
    }
    if written != manifest.byte_length
        || full.finalize().as_slice() != super::parse_hash(&manifest.content_sha256)?
    {
        anyhow::bail!("materialized file failed final verification");
    }
    output.sync_all()?;
    crate::sync::fault::hit("media.after-materialization-sync");
    check_progress()?;
    pre_publish()?;
    finish_materialization(
        managed_root,
        destination_relative,
        &owned_staging,
        &output,
        manifest,
        source_mtime_ms,
    )
}

#[cfg(test)]
fn publish_without_replacement(staging: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
        // Rust's filesystem APIs accept long paths, but the raw Win32 call
        // needs the extended-length prefix. Canonicalize only existing paths;
        // the final target must remain absent and is never opened/replaced.
        let source = fs::canonicalize(staging)?;
        let target_parent = target.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "materialization target has no parent",
            )
        })?;
        let target_leaf = target.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "materialization target has no leaf",
            )
        })?;
        let target = fs::canonicalize(target_parent)?.join(target_leaf);
        let source = source
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let target = target
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::hard_link(staging, target)
    }
}

fn finish_materialization(
    managed_root: &Path,
    destination_relative: &Path,
    staging: &OwnedMaterializationStaging,
    output: &File,
    manifest: &ContentManifestV1,
    source_mtime_ms: Option<i64>,
) -> anyhow::Result<MaterializationResult> {
    let target = managed_root.join(destination_relative);
    let _parents = crate::sync::file_policy::pin_destination_parents(managed_root, &target)?;
    crate::sync::file_policy::require_no_links(managed_root, &target)?;
    let source_guard = staging.guard.as_ref().expect("owned staging guard");
    source_guard.verify_unchanged()?;
    anyhow::ensure!(
        staging.payload_identity.as_deref() == Some(source_guard.file_identity(output)?.as_str()),
        "materialization payload identity changed"
    );
    if let Some(value) = source_mtime_ms.and_then(|value| u64::try_from(value).ok()) {
        let timestamp = filetime::FileTime::from_unix_time(
            (value / 1000) as i64,
            ((value % 1000) * 1_000_000) as u32,
        );
        let _ = filetime::set_file_handle_times(output, None, Some(timestamp));
    }
    if target.exists() {
        return Ok(
            if verified_matches(
                &target,
                manifest.byte_length,
                super::parse_hash(&manifest.content_sha256)?,
            )? {
                MaterializationResult::AdoptedIdentical(target)
            } else {
                MaterializationResult::PathConflict(target)
            },
        );
    }
    let parent =
        crate::library_path::MarkerDirectoryGuard::open(target.parent().expect("target parent"))?;
    match source_guard.publish_to(
        output,
        std::ffi::OsStr::new("payload.partial"),
        &parent,
        target.file_name().expect("target leaf"),
    ) {
        Ok(()) => Ok(MaterializationResult::Published(target)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(
            if verified_matches(
                &target,
                manifest.byte_length,
                super::parse_hash(&manifest.content_sha256)?,
            )? {
                MaterializationResult::AdoptedIdentical(target)
            } else {
                MaterializationResult::PathConflict(target)
            },
        ),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn verified_matches(
    path: &Path,
    expected_length: u64,
    expected_hash: [u8; 32],
) -> anyhow::Result<bool> {
    let (mut file, _guard) = crate::sync::file_policy::open_source(path)?;
    if file.metadata()?.len() != expected_length {
        return Ok(false);
    }
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().as_slice() == expected_hash)
}

fn inventory_path(path: &Path) -> anyhow::Result<()> {
    let value = path.to_string_lossy();
    super::inventory::validate_relative(&value)?;
    if path.components().count() > 128 {
        anyhow::bail!("materialization path depth exceeds limit");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::media::format::{ChunkingProfile, ManifestChunkV1, MediaHint};

    fn manifest() -> (ContentManifestV1, BTreeMap<[u8; 32], Vec<u8>>) {
        let bytes = b"hello".to_vec();
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let manifest = ContentManifestV1 {
            format_version: 1,
            byte_length: 5,
            content_sha256: crate::sync::media::hex_hash(&hash),
            media_hint: MediaHint {
                sniffed_kind: None,
                mime: None,
            },
            chunking: ChunkingProfile::default(),
            chunks: vec![ManifestChunkV1 {
                ordinal: 0,
                chunk_sha256: crate::sync::media::hex_hash(&hash),
                byte_length: 5,
                pack_sha256: crate::sync::media::hex_hash(&[1; 32]),
                pack_offset: 0,
            }],
        };
        (manifest, BTreeMap::from([(hash, bytes)]))
    }

    #[test]
    fn owned_staging_cleanup_preserves_replacement_and_user_addition() {
        let root =
            std::env::temp_dir().join(format!("mangoo-staging-cleanup-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join(".mangoo-staging"), b"user staging name").unwrap();
        let mut staging = create_unregistered_staging(&root, uuid::Uuid::now_v7()).unwrap();
        let directory = staging.directory.clone();
        let partial = directory.join("payload.partial");
        drop(staging.payload.take());
        fs::rename(&partial, directory.join("original.partial")).unwrap();
        fs::write(&partial, b"replacement").unwrap();
        fs::write(directory.join("user.bin"), b"user addition").unwrap();
        drop(staging);
        assert_eq!(fs::read(&partial).unwrap(), b"replacement");
        assert_eq!(
            fs::read(directory.join("user.bin")).unwrap(),
            b"user addition"
        );
        assert_eq!(
            fs::read(root.join(".mangoo-staging")).unwrap(),
            b"user staging name"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reserved_names_and_conflicts_are_safe() {
        assert_eq!(windows_safe_component("CON.jpg", "abcd"), "_CON.jpg");
        assert_eq!(windows_safe_component("bad. ", "abcd"), "bad");
        let root =
            std::env::temp_dir().join(format!("mangoo-materialize-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let (manifest, chunks) = manifest();
        let first = materialize_from_chunks(
            &root,
            Path::new("file.jpg"),
            uuid::Uuid::now_v7(),
            &manifest,
            &chunks,
            None,
        )
        .unwrap();
        assert!(matches!(first, MaterializationResult::Published(_)));
        let second = materialize_from_chunks(
            &root,
            Path::new("file.jpg"),
            uuid::Uuid::now_v7(),
            &manifest,
            &chunks,
            None,
        )
        .unwrap();
        assert!(matches!(second, MaterializationResult::AdoptedIdentical(_)));
        std::fs::write(root.join("file.jpg"), b"other").unwrap();
        let third = materialize_from_chunks(
            &root,
            Path::new("file.jpg"),
            uuid::Uuid::now_v7(),
            &manifest,
            &chunks,
            None,
        )
        .unwrap();
        assert!(matches!(third, MaterializationResult::PathConflict(_)));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn verified_ranges_stream_into_materialization() {
        let root =
            std::env::temp_dir().join(format!("mangoo-range-materialize-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let (manifest, chunks) = manifest();
        let hash = *chunks.keys().next().unwrap();
        let range_path = root.join("verified.range");
        std::fs::write(&range_path, b"xxhellozz").unwrap();
        let sources = BTreeMap::from([(
            hash,
            super::super::recovery::VerifiedChunkSource {
                path: range_path,
                offset: 2,
                length: 5,
            },
        )]);
        let result = materialize_from_verified_ranges(
            &root,
            Path::new("streamed.jpg"),
            uuid::Uuid::now_v7(),
            &manifest,
            &sources,
            None,
        )
        .unwrap();
        assert!(matches!(result, MaterializationResult::Published(_)));
        assert_eq!(std::fs::read(root.join("streamed.jpg")).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stale_root_fence_removes_staging_without_publishing() {
        let root = std::env::temp_dir().join(format!(
            "mangoo-stale-range-materialize-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (manifest, chunks) = manifest();
        let hash = *chunks.keys().next().unwrap();
        let range_path = root.join("verified.range");
        std::fs::write(&range_path, b"xxhellozz").unwrap();
        let sources = BTreeMap::from([(
            hash,
            super::super::recovery::VerifiedChunkSource {
                path: range_path,
                offset: 2,
                length: 5,
            },
        )]);
        let job_id = uuid::Uuid::now_v7();
        let result = materialize_from_verified_ranges_with_pre_publish(
            &root,
            Path::new("streamed.jpg"),
            job_id,
            &manifest,
            &sources,
            None,
            || anyhow::bail!("stale root binding generation"),
        );
        assert!(result.is_err());
        assert!(!root.join("streamed.jpg").exists());
        assert!(fs::read_dir(&root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".mangoo-work-")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn all_file_cancelled_assembly_cleans_staging_and_no_replace_preserves_target() {
        let root = std::env::temp_dir().join(format!(
            "mangoo-materialize-cancel-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&root).unwrap();
        let (manifest, chunks) = manifest();
        let hash = *chunks.keys().next().unwrap();
        let range = root.join("input.range");
        fs::write(&range, b"hello").unwrap();
        let sources = BTreeMap::from([(
            hash,
            super::super::recovery::VerifiedChunkSource {
                path: range,
                offset: 0,
                length: 5,
            },
        )]);
        let job = uuid::Uuid::now_v7();
        let mut checks = 0;
        let result = materialize_from_verified_ranges_with_checks(
            &root,
            Path::new("out.bin"),
            job,
            &manifest,
            &sources,
            None,
            None,
            || {
                checks += 1;
                anyhow::ensure!(checks < 2, "cancelled");
                Ok(())
            },
            || Ok(()),
        );
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(!root.join("out.bin").exists());
        assert!(fs::read_dir(&root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".mangoo-work-")));
        let staged = root.join("staged");
        let target = root.join("target");
        fs::write(&staged, b"new").unwrap();
        fs::write(&target, b"existing").unwrap();
        assert!(publish_without_replacement(&staged, &target).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"existing");
        assert_eq!(fs::read(&staged).unwrap(), b"new");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_and_colliding_windows_names_map_deterministically() {
        assert_eq!(windows_safe_component("bad?.jpg", "first"), "bad_.jpg");
        assert_eq!(windows_safe_component("bad*.jpg", "second"), "bad_.jpg");
        assert_eq!(windows_safe_component("con.txt", "first"), "_con.txt");
        // Legal components are kept verbatim; only components beyond the OS
        // limit need deterministic legalization.
        let legal = "x".repeat(220);
        assert_eq!(windows_safe_component(&legal, "abcdef0123456789"), legal);
        let long = "x".repeat(300);
        let first = windows_safe_component(&long, "abcdef0123456789");
        let second = windows_safe_component(&long, "abcdef0123456789");
        assert_eq!(first, second);
        assert_eq!(first.encode_utf16().count(), 255);
        assert!(first.ends_with("~abcdef012345"));
    }
}
