use super::format::ContentManifestV1;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
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
    if value.encode_utf16().count() > 180 {
        let mut shortened = value.chars().take(150).collect::<String>();
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
    let staging_directory = managed_root.join(".megle-staging");
    fs::create_dir_all(&staging_directory)?;
    let staging = staging_directory.join(format!("{}.partial", job_id.simple()));
    if staging.exists() {
        fs::remove_file(&staging)?;
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
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
    drop(output);
    if let Some(value) = source_mtime_ms.and_then(|value| u64::try_from(value).ok()) {
        let timestamp = filetime::FileTime::from_unix_time(
            (value / 1000) as i64,
            ((value % 1000) * 1_000_000) as u32,
        );
        let _ = filetime::set_file_mtime(&staging, timestamp);
    }
    let target = managed_root.join(destination_relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if target.exists() {
        if verified_matches(
            &target,
            manifest.byte_length,
            super::parse_hash(&manifest.content_sha256)?,
        )? {
            fs::remove_file(&staging)?;
            return Ok(MaterializationResult::AdoptedIdentical(target));
        }
        return Ok(MaterializationResult::PathConflict(target));
    }
    #[cfg(windows)]
    let publish = fs::rename(&staging, &target);
    #[cfg(not(windows))]
    let publish = fs::hard_link(&staging, &target);
    match publish {
        Ok(()) => {
            #[cfg(not(windows))]
            fs::remove_file(&staging)?;
            if let Some(parent) = target.parent().and_then(|path| File::open(path).ok()) {
                let _ = parent.sync_all();
            }
            Ok(MaterializationResult::Published(target))
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if verified_matches(
                &target,
                manifest.byte_length,
                super::parse_hash(&manifest.content_sha256)?,
            )? {
                fs::remove_file(&staging)?;
                Ok(MaterializationResult::AdoptedIdentical(target))
            } else {
                Ok(MaterializationResult::PathConflict(target))
            }
        }
        Err(error) => Err(error.into()),
    }
}

pub fn materialize_from_verified_ranges(
    managed_root: &Path,
    destination_relative: &Path,
    job_id: uuid::Uuid,
    manifest: &ContentManifestV1,
    chunks: &BTreeMap<[u8; 32], super::recovery::VerifiedChunkSource>,
    source_mtime_ms: Option<i64>,
) -> anyhow::Result<MaterializationResult> {
    inventory_path(destination_relative)?;
    manifest.validate()?;
    let staging_directory = managed_root.join(".megle-staging");
    fs::create_dir_all(&staging_directory)?;
    let staging = staging_directory.join(format!("{}.partial", job_id.simple()));
    if staging.exists() {
        fs::remove_file(&staging)?;
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)?;
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
    drop(output);
    crate::sync::fault::hit("media.after-materialization-sync");
    finish_materialization(
        managed_root,
        destination_relative,
        &staging,
        manifest,
        source_mtime_ms,
    )
}

fn finish_materialization(
    managed_root: &Path,
    destination_relative: &Path,
    staging: &Path,
    manifest: &ContentManifestV1,
    source_mtime_ms: Option<i64>,
) -> anyhow::Result<MaterializationResult> {
    if let Some(value) = source_mtime_ms.and_then(|value| u64::try_from(value).ok()) {
        let timestamp = filetime::FileTime::from_unix_time(
            (value / 1000) as i64,
            ((value % 1000) * 1_000_000) as u32,
        );
        let _ = filetime::set_file_mtime(staging, timestamp);
    }
    let target = managed_root.join(destination_relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if target.exists() {
        if verified_matches(
            &target,
            manifest.byte_length,
            super::parse_hash(&manifest.content_sha256)?,
        )? {
            fs::remove_file(staging)?;
            return Ok(MaterializationResult::AdoptedIdentical(target));
        }
        return Ok(MaterializationResult::PathConflict(target));
    }
    #[cfg(windows)]
    let publish = fs::rename(staging, &target);
    #[cfg(not(windows))]
    let publish = fs::hard_link(staging, &target);
    match publish {
        Ok(()) => {
            #[cfg(not(windows))]
            fs::remove_file(staging)?;
            if let Some(parent) = target.parent().and_then(|path| File::open(path).ok()) {
                let _ = parent.sync_all();
            }
            Ok(MaterializationResult::Published(target))
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if verified_matches(
                &target,
                manifest.byte_length,
                super::parse_hash(&manifest.content_sha256)?,
            )? {
                fs::remove_file(staging)?;
                Ok(MaterializationResult::AdoptedIdentical(target))
            } else {
                Ok(MaterializationResult::PathConflict(target))
            }
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn verified_matches(
    path: &Path,
    expected_length: u64,
    expected_hash: [u8; 32],
) -> anyhow::Result<bool> {
    let mut file = File::open(path)?;
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
    fn reserved_names_and_conflicts_are_safe() {
        assert_eq!(windows_safe_component("CON.jpg", "abcd"), "_CON.jpg");
        assert_eq!(windows_safe_component("bad. ", "abcd"), "bad");
        let root = std::env::temp_dir().join(format!("megle-materialize-{}", uuid::Uuid::now_v7()));
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
            std::env::temp_dir().join(format!("megle-range-materialize-{}", uuid::Uuid::now_v7()));
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
    fn invalid_and_colliding_windows_names_map_deterministically() {
        assert_eq!(windows_safe_component("bad?.jpg", "first"), "bad_.jpg");
        assert_eq!(windows_safe_component("bad*.jpg", "second"), "bad_.jpg");
        assert_eq!(windows_safe_component("con.txt", "first"), "_con.txt");
        let long = "x".repeat(220);
        let first = windows_safe_component(&long, "abcdef0123456789");
        let second = windows_safe_component(&long, "abcdef0123456789");
        assert_eq!(first, second);
        assert!(first.encode_utf16().count() <= 163);
    }
}
