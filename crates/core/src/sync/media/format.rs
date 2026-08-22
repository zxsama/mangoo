use super::{
    hex_hash, parse_hash, CDC_MAX_BYTES, CDC_MIN_BYTES, CDC_TARGET_BYTES, MANIFEST_PACK_MAX_BYTES,
    MAX_CHUNKS_PER_MANIFEST, MAX_MANIFEST_BYTES, MEDIA_FORMAT_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const DATA_PACK_MAGIC: &[u8; 8] = b"MGLDPK01";
const MANIFEST_PACK_MAGIC: &[u8; 8] = b"MGLMPK01";
const INDEX_SHARD_MAGIC: &[u8; 8] = b"MGLIDX01";
const TRAILER_BYTES: usize = 32 + 8 + 8;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaHint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sniffed_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkingProfile {
    pub algorithm: String,
    pub version: u16,
    pub min_bytes: u64,
    pub target_bytes: u64,
    pub max_bytes: u64,
}

impl Default for ChunkingProfile {
    fn default() -> Self {
        Self {
            algorithm: "gear-cdc".into(),
            version: 1,
            min_bytes: CDC_MIN_BYTES,
            target_bytes: CDC_TARGET_BYTES,
            max_bytes: CDC_MAX_BYTES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestChunkV1 {
    pub ordinal: u32,
    pub chunk_sha256: String,
    pub byte_length: u64,
    pub pack_sha256: String,
    pub pack_offset: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentManifestV1 {
    pub format_version: u16,
    pub byte_length: u64,
    pub content_sha256: String,
    pub media_hint: MediaHint,
    pub chunking: ChunkingProfile,
    pub chunks: Vec<ManifestChunkV1>,
}

impl ContentManifestV1 {
    pub fn canonical_bytes(&self) -> anyhow::Result<Vec<u8>> {
        self.validate()?;
        Ok(crate::sync::codec::canonical_bytes(self)?)
    }

    pub fn hash(&self) -> anyhow::Result<[u8; 32]> {
        Ok(Sha256::digest(self.canonical_bytes()?).into())
    }

    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            anyhow::bail!("manifest exceeds maximum size");
        }
        let manifest: Self = serde_json::from_slice(bytes)?;
        manifest.validate()?;
        if manifest.canonical_bytes()? != bytes {
            anyhow::bail!("manifest is not canonically encoded");
        }
        Ok(manifest)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.format_version != MEDIA_FORMAT_VERSION {
            anyhow::bail!("unsupported media manifest version");
        }
        parse_hash(&self.content_sha256)?;
        validate_hint(self.media_hint.sniffed_kind.as_deref())?;
        validate_hint(self.media_hint.mime.as_deref())?;
        if self.chunking.algorithm != "gear-cdc"
            || self.chunking.version != 1
            || self.chunking.min_bytes == 0
            || self.chunking.min_bytes > self.chunking.target_bytes
            || self.chunking.target_bytes > self.chunking.max_bytes
            || self.chunking.max_bytes > 64 * 1024 * 1024
        {
            anyhow::bail!("invalid chunking profile");
        }
        if self.chunks.len() > MAX_CHUNKS_PER_MANIFEST {
            anyhow::bail!("manifest chunk count exceeds limit");
        }
        if self.byte_length == 0 && !self.chunks.is_empty() {
            anyhow::bail!("empty content must not contain chunks");
        }
        if self.byte_length > 0 && self.chunks.is_empty() {
            anyhow::bail!("non-empty content requires chunks");
        }
        let mut total = 0u64;
        for (index, chunk) in self.chunks.iter().enumerate() {
            if chunk.ordinal as usize != index || chunk.byte_length == 0 {
                anyhow::bail!("manifest chunk order or length is invalid");
            }
            parse_hash(&chunk.chunk_sha256)?;
            parse_hash(&chunk.pack_sha256)?;
            chunk
                .pack_offset
                .checked_add(chunk.byte_length)
                .ok_or_else(|| anyhow::anyhow!("manifest chunk range overflows"))?;
            total = total
                .checked_add(chunk.byte_length)
                .ok_or_else(|| anyhow::anyhow!("manifest byte length overflows"))?;
        }
        if total != self.byte_length {
            anyhow::bail!("manifest byte length does not match chunks");
        }
        Ok(())
    }
}

fn validate_hint(value: Option<&str>) -> anyhow::Result<()> {
    if value.is_some_and(|value| value.len() > 255 || value.chars().any(char::is_control)) {
        anyhow::bail!("media hint exceeds limits");
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackIndexEntry {
    pub hash: String,
    pub offset: u64,
    pub length: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedDataPack<'a> {
    pub data: &'a [u8],
    pub entries: Vec<PackIndexEntry>,
}

pub fn encode_data_pack(chunks: &[([u8; 32], Vec<u8>)]) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut entries = Vec::with_capacity(chunks.len());
    let mut seen = BTreeSet::new();
    for (hash, chunk) in chunks {
        if chunk.is_empty() || Sha256::digest(chunk).as_slice() != hash {
            anyhow::bail!("data pack chunk identity mismatch");
        }
        if !seen.insert(*hash) {
            anyhow::bail!("duplicate chunk identity in data pack");
        }
        let offset = bytes.len() as u64;
        bytes.extend_from_slice(chunk);
        entries.push(PackIndexEntry {
            hash: hex_hash(hash),
            offset,
            length: chunk.len() as u64,
        });
    }
    append_footer(&mut bytes, &entries, DATA_PACK_MAGIC)?;
    Ok(bytes)
}

/// Finalize a pack whose payload was assembled incrementally. This avoids
/// retaining a second copy of every chunk while producing the footer.
pub(crate) fn finalize_data_pack(
    mut bytes: Vec<u8>,
    entries: &[PackIndexEntry],
) -> anyhow::Result<Vec<u8>> {
    let mut seen = BTreeSet::new();
    let mut previous_end = 0_u64;
    for entry in entries {
        parse_hash(&entry.hash)?;
        let end = entry
            .offset
            .checked_add(entry.length)
            .ok_or_else(|| anyhow::anyhow!("data pack entry overflows"))?;
        if entry.length == 0
            || entry.offset < previous_end
            || end > bytes.len() as u64
            || !seen.insert(&entry.hash)
        {
            anyhow::bail!("invalid incremental data pack entry");
        }
        previous_end = end;
    }
    if previous_end != bytes.len() as u64 {
        anyhow::bail!("incremental data pack payload is not fully indexed");
    }
    append_footer(&mut bytes, &entries.to_vec(), DATA_PACK_MAGIC)?;
    Ok(bytes)
}

pub fn decode_data_pack(bytes: &[u8]) -> anyhow::Result<DecodedDataPack<'_>> {
    let (data, entries) = decode_footer(bytes, DATA_PACK_MAGIC, MAX_CHUNKS_PER_MANIFEST)?;
    let mut previous_end = 0u64;
    let mut seen = BTreeSet::new();
    for entry in &entries {
        let hash = parse_hash(&entry.hash)?;
        let end = entry
            .offset
            .checked_add(entry.length)
            .ok_or_else(|| anyhow::anyhow!("data pack range overflows"))?;
        if entry.length == 0 || entry.offset < previous_end || end > data.len() as u64 {
            anyhow::bail!("data pack index range is invalid");
        }
        if !seen.insert(hash) {
            anyhow::bail!("duplicate data pack index hash");
        }
        if Sha256::digest(&data[entry.offset as usize..end as usize]).as_slice() != hash {
            anyhow::bail!("data pack chunk hash mismatch");
        }
        previous_end = end;
    }
    if previous_end != data.len() as u64 && !entries.is_empty() {
        anyhow::bail!("data pack contains unindexed bytes");
    }
    Ok(DecodedDataPack { data, entries })
}

pub fn encode_manifest_pack(records: &[([u8; 32], Vec<u8>)]) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut entries = Vec::with_capacity(records.len());
    let mut seen = BTreeSet::new();
    for (hash, record) in records {
        if record.len() > MAX_MANIFEST_BYTES || Sha256::digest(record).as_slice() != hash {
            anyhow::bail!("manifest record identity mismatch");
        }
        ContentManifestV1::decode(record)?;
        if !seen.insert(*hash) {
            anyhow::bail!("duplicate manifest identity");
        }
        let length = u32::try_from(record.len())?;
        bytes.extend_from_slice(&length.to_le_bytes());
        let offset = bytes.len() as u64;
        bytes.extend_from_slice(record);
        entries.push(PackIndexEntry {
            hash: hex_hash(hash),
            offset,
            length: record.len() as u64,
        });
    }
    append_footer(&mut bytes, &entries, MANIFEST_PACK_MAGIC)?;
    if bytes.len() > MANIFEST_PACK_MAX_BYTES as usize {
        anyhow::bail!("manifest pack exceeds hard cap");
    }
    Ok(bytes)
}

pub fn decode_manifest_pack(bytes: &[u8]) -> anyhow::Result<Vec<(String, ContentManifestV1)>> {
    if bytes.len() > MANIFEST_PACK_MAX_BYTES as usize {
        anyhow::bail!("manifest pack exceeds hard cap");
    }
    let (data, entries) = decode_footer(bytes, MANIFEST_PACK_MAGIC, MAX_CHUNKS_PER_MANIFEST)?;
    let mut records = Vec::with_capacity(entries.len());
    let mut previous_end = 0u64;
    for entry in entries {
        let hash = parse_hash(&entry.hash)?;
        if entry.offset < 4 || entry.offset - 4 < previous_end {
            anyhow::bail!("manifest pack entries overlap");
        }
        let prefix = usize::try_from(entry.offset - 4)?;
        let declared = u32::from_le_bytes(data[prefix..prefix + 4].try_into()?) as u64;
        let end = entry
            .offset
            .checked_add(entry.length)
            .ok_or_else(|| anyhow::anyhow!("manifest range overflows"))?;
        if declared != entry.length || end > data.len() as u64 {
            anyhow::bail!("manifest pack record range is invalid");
        }
        let record = &data[entry.offset as usize..end as usize];
        if Sha256::digest(record).as_slice() != hash {
            anyhow::bail!("manifest pack record hash mismatch");
        }
        records.push((entry.hash, ContentManifestV1::decode(record)?));
        previous_end = end;
    }
    if previous_end != data.len() as u64 && !records.is_empty() {
        anyhow::bail!("manifest pack contains trailing data");
    }
    Ok(records)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexShardV1 {
    pub format_version: u16,
    pub kind: String,
    pub entries: Vec<IndexShardEntryV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexShardEntryV1 {
    pub hash: String,
    pub object_key: String,
    pub object_hash: String,
    pub object_length: u64,
    pub offset: u64,
    pub length: u64,
}

impl IndexShardV1 {
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        self.validate()?;
        let mut bytes = crate::sync::codec::canonical_bytes(self)?;
        if bytes.len() + TRAILER_BYTES > super::MAX_INDEX_SHARD_BYTES {
            anyhow::bail!("index shard exceeds hard cap");
        }
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        bytes.extend_from_slice(&hash);
        bytes.extend_from_slice(&(bytes.len() as u64 - 32).to_le_bytes());
        bytes.extend_from_slice(INDEX_SHARD_MAGIC);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        if bytes.len() > super::MAX_INDEX_SHARD_BYTES {
            anyhow::bail!("index shard exceeds hard cap");
        }
        if bytes.len() < TRAILER_BYTES || &bytes[bytes.len() - 8..] != INDEX_SHARD_MAGIC {
            anyhow::bail!("invalid index shard trailer");
        }
        let payload_len =
            u64::from_le_bytes(bytes[bytes.len() - 16..bytes.len() - 8].try_into()?) as usize;
        if payload_len + TRAILER_BYTES != bytes.len() {
            anyhow::bail!("index shard contains trailing or truncated bytes");
        }
        let expected = &bytes[payload_len..payload_len + 32];
        if Sha256::digest(&bytes[..payload_len]).as_slice() != expected {
            anyhow::bail!("index shard checksum mismatch");
        }
        let shard: Self = serde_json::from_slice(&bytes[..payload_len])?;
        shard.validate()?;
        if crate::sync::codec::canonical_bytes(&shard)? != bytes[..payload_len] {
            anyhow::bail!("index shard is not canonical");
        }
        Ok(shard)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.format_version != MEDIA_FORMAT_VERSION
            || !matches!(self.kind.as_str(), "chunk" | "manifest")
            || self.entries.len() > super::MAX_INDEX_SHARD_ENTRIES
        {
            anyhow::bail!("invalid index shard header");
        }
        let mut keys = BTreeSet::new();
        for entry in &self.entries {
            parse_hash(&entry.hash)?;
            parse_hash(&entry.object_hash)?;
            if entry.object_key.is_empty()
                || entry.object_key.len() > 1024
                || !(entry.object_key.starts_with("content/data/")
                    || entry.object_key.starts_with("content/manifests/"))
                || entry.object_length == 0
            {
                anyhow::bail!("invalid index object locator");
            }
            entry
                .offset
                .checked_add(entry.length)
                .ok_or_else(|| anyhow::anyhow!("index range overflows"))?;
            if entry.length == 0
                || entry.offset + entry.length > entry.object_length
                || !keys.insert(&entry.hash)
            {
                anyhow::bail!("duplicate or empty index entry");
            }
        }
        Ok(())
    }
}

fn append_footer<T: Serialize>(
    bytes: &mut Vec<u8>,
    entries: &T,
    magic: &[u8; 8],
) -> anyhow::Result<()> {
    let index = crate::sync::codec::canonical_bytes(entries)?;
    let hash: [u8; 32] = Sha256::digest(&index).into();
    bytes.extend_from_slice(&index);
    bytes.extend_from_slice(&hash);
    bytes.extend_from_slice(&(index.len() as u64).to_le_bytes());
    bytes.extend_from_slice(magic);
    Ok(())
}

fn decode_footer<'a>(
    bytes: &'a [u8],
    magic: &[u8; 8],
    max_entries: usize,
) -> anyhow::Result<(&'a [u8], Vec<PackIndexEntry>)> {
    if bytes.len() < TRAILER_BYTES || &bytes[bytes.len() - 8..] != magic {
        anyhow::bail!("invalid pack trailer");
    }
    let index_len =
        u64::from_le_bytes(bytes[bytes.len() - 16..bytes.len() - 8].try_into()?) as usize;
    if index_len > MAX_MANIFEST_BYTES || index_len + TRAILER_BYTES > bytes.len() {
        anyhow::bail!("invalid pack index length");
    }
    let index_start = bytes.len() - TRAILER_BYTES - index_len;
    let expected = &bytes[index_start + index_len..index_start + index_len + 32];
    if Sha256::digest(&bytes[index_start..index_start + index_len]).as_slice() != expected {
        anyhow::bail!("pack index checksum mismatch");
    }
    let entries: Vec<PackIndexEntry> =
        serde_json::from_slice(&bytes[index_start..index_start + index_len])?;
    if entries.len() > max_entries
        || crate::sync::codec::canonical_bytes(&entries)?
            != bytes[index_start..index_start + index_len]
    {
        anyhow::bail!("pack index is not canonical or exceeds limits");
    }
    Ok((&bytes[..index_start], entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest(pack: [u8; 32], chunk: [u8; 32]) -> ContentManifestV1 {
        ContentManifestV1 {
            format_version: MEDIA_FORMAT_VERSION,
            byte_length: 3,
            content_sha256: hex_hash(&Sha256::digest(b"abc").into()),
            media_hint: MediaHint {
                sniffed_kind: Some("image".into()),
                mime: Some("image/jpeg".into()),
            },
            chunking: ChunkingProfile::default(),
            chunks: vec![ManifestChunkV1 {
                ordinal: 0,
                chunk_sha256: hex_hash(&chunk),
                byte_length: 3,
                pack_sha256: hex_hash(&pack),
                pack_offset: 0,
            }],
        }
    }

    #[test]
    fn manifest_is_content_only_and_deterministic() {
        let chunk: [u8; 32] = Sha256::digest(b"abc").into();
        let pack: [u8; 32] = Sha256::digest(b"pack").into();
        let manifest = sample_manifest(pack, chunk);
        let first = manifest.canonical_bytes().unwrap();
        assert_eq!(first, manifest.canonical_bytes().unwrap());
        assert_eq!(ContentManifestV1::decode(&first).unwrap(), manifest);
        let text = String::from_utf8(first).unwrap();
        for forbidden in [
            "asset_uuid",
            "revision_uuid",
            "source_mtime",
            "path",
            "name",
        ] {
            assert!(!text.contains(forbidden));
        }
    }

    #[test]
    fn data_pack_rejects_truncation_overlap_and_hash_mismatch() {
        let hash: [u8; 32] = Sha256::digest(b"abc").into();
        let pack = encode_data_pack(&[(hash, b"abc".to_vec())]).unwrap();
        assert_eq!(decode_data_pack(&pack).unwrap().data, b"abc");
        assert!(decode_data_pack(&pack[..pack.len() - 1]).is_err());
        let mut corrupt = pack.clone();
        corrupt[0] ^= 1;
        assert!(decode_data_pack(&corrupt).is_err());
    }

    #[test]
    fn manifest_pack_and_index_round_trip_strictly() {
        let chunk: [u8; 32] = Sha256::digest(b"abc").into();
        let pack: [u8; 32] = Sha256::digest(b"pack").into();
        let manifest = sample_manifest(pack, chunk);
        let bytes = manifest.canonical_bytes().unwrap();
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let encoded = encode_manifest_pack(&[(hash, bytes)]).unwrap();
        assert_eq!(decode_manifest_pack(&encoded).unwrap()[0].1, manifest);

        let shard = IndexShardV1 {
            format_version: 1,
            kind: "chunk".into(),
            entries: vec![IndexShardEntryV1 {
                hash: hex_hash(&chunk),
                object_key: format!("content/data/00/{}.pack", hex_hash(&pack)),
                object_hash: hex_hash(&pack),
                object_length: 4,
                offset: 1,
                length: 3,
            }],
        };
        let encoded = shard.encode().unwrap();
        assert_eq!(IndexShardV1::decode(&encoded).unwrap(), shard);
    }
}
