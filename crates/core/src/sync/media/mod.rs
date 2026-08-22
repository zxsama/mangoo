//! Content-addressed original-media storage.
//!
//! This module owns deterministic chunk/pack formats and durable job state.
//! Provider implementations only move the resulting opaque immutable objects.

pub mod chunker;
pub mod format;
pub mod inventory;
pub mod materialize;
pub mod packer;
pub mod publication;
pub mod recovery;
pub mod worker;

pub const MEDIA_FORMAT_VERSION: u16 = 1;
pub const SMALL_FILE_THRESHOLD: u64 = 2 * 1024 * 1024;
pub const CDC_MIN_BYTES: u64 = 2 * 1024 * 1024;
pub const CDC_TARGET_BYTES: u64 = 8 * 1024 * 1024;
pub const CDC_MAX_BYTES: u64 = 16 * 1024 * 1024;
pub const DATA_PACK_TARGET_BYTES: u64 = 64 * 1024 * 1024;
pub const DATA_PACK_MIN_BYTES: u64 = 32 * 1024 * 1024;
pub const DATA_PACK_MAX_BYTES: u64 = 96 * 1024 * 1024;
pub const MANIFEST_PACK_TARGET_BYTES: u64 = 4 * 1024 * 1024;
pub const MANIFEST_PACK_MAX_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_CHUNKS_PER_MANIFEST: usize = 1_000_000;
pub const MAX_INDEX_SHARD_ENTRIES: usize = 4_096;
pub const MAX_INDEX_SHARD_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RANGE_BYTES: u64 = 16 * 1024 * 1024;

pub fn hex_hash(hash: &[u8; 32]) -> String {
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn parse_hash(value: &str) -> anyhow::Result<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("invalid sha256 identity");
    }
    let mut hash = [0u8; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(hash)
}

pub fn data_object_key(hash: &[u8; 32]) -> String {
    let hash = hex_hash(hash);
    format!("content/data/{}/{}.pack", &hash[..2], hash)
}

pub fn manifest_object_key(hash: &[u8; 32]) -> String {
    let hash = hex_hash(hash);
    format!("content/manifests/{hash}.mpack")
}
