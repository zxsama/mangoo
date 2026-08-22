use super::{
    data_object_key,
    format::{encode_manifest_pack, ContentManifestV1, ManifestChunkV1},
    hex_hash, manifest_object_key, DATA_PACK_MAX_BYTES, DATA_PACK_TARGET_BYTES,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SealedObject {
    pub hash: [u8; 32],
    pub object_key: String,
    pub byte_length: u64,
    pub relative_path: PathBuf,
}

#[derive(Debug)]
pub struct DataPackBuilder {
    payload: Vec<u8>,
    entries: Vec<crate::sync::media::format::PackIndexEntry>,
}

impl DataPackBuilder {
    pub fn new() -> Self {
        Self {
            payload: Vec::with_capacity(DATA_PACK_TARGET_BYTES as usize),
            entries: Vec::new(),
        }
    }

    pub fn push(&mut self, hash: [u8; 32], bytes: Vec<u8>) -> anyhow::Result<bool> {
        if bytes.is_empty() || bytes.len() as u64 > super::CDC_MAX_BYTES {
            anyhow::bail!("chunk exceeds pack limits");
        }
        if !self.payload.is_empty()
            && self.payload.len() as u64 + bytes.len() as u64 > DATA_PACK_MAX_BYTES
        {
            return Ok(false);
        }
        if sha2::Sha256::digest(&bytes).as_slice() != hash {
            anyhow::bail!("data pack chunk identity mismatch");
        }
        if self
            .entries
            .iter()
            .any(|entry| entry.hash == hex_hash(&hash))
        {
            anyhow::bail!("duplicate chunk identity in data pack");
        }
        let offset = self.payload.len() as u64;
        self.payload.extend_from_slice(&bytes);
        self.entries
            .push(crate::sync::media::format::PackIndexEntry {
                hash: hex_hash(&hash),
                offset,
                length: bytes.len() as u64,
            });
        Ok(true)
    }

    pub fn should_seal(&self) -> bool {
        self.payload.len() as u64 >= DATA_PACK_TARGET_BYTES
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn seal(self, root: &Path) -> anyhow::Result<(SealedObject, Vec<ManifestChunkV1>)> {
        let entries = self.entries;
        let bytes = crate::sync::media::format::finalize_data_pack(self.payload, &entries)?;
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let object_key = data_object_key(&hash);
        let relative_path = PathBuf::from("data").join(format!("{}.pack", hex_hash(&hash)));
        durable_publish(root, &relative_path, &bytes, hash)?;
        let chunks = entries
            .into_iter()
            .enumerate()
            .map(|(ordinal, entry)| ManifestChunkV1 {
                ordinal: ordinal as u32,
                chunk_sha256: entry.hash,
                byte_length: entry.length,
                pack_sha256: hex_hash(&hash),
                pack_offset: entry.offset,
            })
            .collect();
        Ok((
            SealedObject {
                hash,
                object_key,
                byte_length: bytes.len() as u64,
                relative_path,
            },
            chunks,
        ))
    }
}

pub fn seal_manifest_pack(
    root: &Path,
    manifests: &[ContentManifestV1],
) -> anyhow::Result<(SealedObject, Vec<([u8; 32], u64, u64)>)> {
    let records = manifests
        .iter()
        .map(|manifest| {
            let bytes = manifest.canonical_bytes()?;
            let hash: [u8; 32] = Sha256::digest(&bytes).into();
            Ok((hash, bytes))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let bytes = encode_manifest_pack(&records)?;
    let hash: [u8; 32] = Sha256::digest(&bytes).into();
    let object_key = manifest_object_key(&hash);
    let relative_path = PathBuf::from("manifests").join(format!("{}.mpack", hex_hash(&hash)));
    durable_publish(root, &relative_path, &bytes, hash)?;
    let decoded = crate::sync::media::format::decode_manifest_pack(&bytes)?;
    let mut locations = Vec::new();
    for (record_hash, _) in decoded {
        let needle = records
            .iter()
            .find(|(hash, _)| hex_hash(hash) == record_hash)
            .unwrap();
        let pattern = &needle.1;
        let offset = bytes
            .windows(pattern.len())
            .position(|candidate| candidate == pattern)
            .ok_or_else(|| anyhow::anyhow!("sealed manifest record missing"))?
            as u64;
        locations.push((needle.0, offset, pattern.len() as u64));
    }
    Ok((
        SealedObject {
            hash,
            object_key,
            byte_length: bytes.len() as u64,
            relative_path,
        },
        locations,
    ))
}

pub fn durable_publish(
    root: &Path,
    relative: &Path,
    bytes: &[u8],
    expected_hash: [u8; 32],
) -> anyhow::Result<PathBuf> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        anyhow::bail!("managed object path must be relative");
    }
    let final_path = root.join(relative);
    let parent = final_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("managed object path has no parent"))?;
    fs::create_dir_all(parent)?;
    if final_path.exists() {
        verify_file(&final_path, expected_hash, bytes.len() as u64)?;
        return Ok(final_path);
    }
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        final_path.file_name().unwrap().to_string_lossy(),
        uuid::Uuid::now_v7()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    match fs::hard_link(&temporary, &final_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            verify_file(&final_path, expected_hash, bytes.len() as u64)?
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
    }
    fs::remove_file(&temporary)?;
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(final_path)
}

pub fn verify_file(
    path: &Path,
    expected_hash: [u8; 32],
    expected_length: u64,
) -> anyhow::Result<()> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() != expected_length {
        anyhow::bail!("sealed object length mismatch");
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
    if hash.finalize().as_slice() != expected_hash {
        anyhow::bail!("sealed object hash mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_is_idempotent_and_never_replaces_conflict() {
        let root = std::env::temp_dir().join(format!("megle-media-pack-{}", uuid::Uuid::now_v7()));
        let bytes = b"abc";
        let hash: [u8; 32] = Sha256::digest(bytes).into();
        let path = durable_publish(&root, Path::new("data/object"), bytes, hash).unwrap();
        durable_publish(&root, Path::new("data/object"), bytes, hash).unwrap();
        std::fs::write(&path, b"wrong").unwrap();
        assert!(durable_publish(&root, Path::new("data/object"), bytes, hash).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
