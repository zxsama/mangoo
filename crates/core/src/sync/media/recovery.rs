use super::{format::ContentManifestV1, inventory, MAX_RANGE_BYTES};
use crate::sync::provider::{DownloadedRange, SyncProvider, TransportContext};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedRange {
    pub pack_hash: [u8; 32],
    pub object_key: String,
    pub start: u64,
    pub end: u64,
    pub chunks: Vec<(u32, [u8; 32], u64, u64)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedChunkSource {
    pub path: PathBuf,
    pub offset: u64,
    pub length: u64,
}

pub fn plan_ranges(manifest: &ContentManifestV1) -> anyhow::Result<Vec<PlannedRange>> {
    manifest.validate()?;
    let mut by_pack: BTreeMap<[u8; 32], Vec<(u32, [u8; 32], u64, u64)>> = BTreeMap::new();
    for chunk in &manifest.chunks {
        by_pack
            .entry(super::parse_hash(&chunk.pack_sha256)?)
            .or_default()
            .push((
                chunk.ordinal,
                super::parse_hash(&chunk.chunk_sha256)?,
                chunk.pack_offset,
                chunk.byte_length,
            ));
    }
    let mut ranges = Vec::new();
    for (pack_hash, mut chunks) in by_pack {
        chunks.sort_by_key(|chunk| chunk.2);
        let mut current: Option<PlannedRange> = None;
        for chunk in chunks {
            let end = chunk
                .2
                .checked_add(chunk.3)
                .ok_or_else(|| anyhow::anyhow!("chunk range overflows"))?;
            if let Some(range) = &mut current {
                if chunk.2 >= range.end
                    && chunk.2 <= range.end.saturating_add(64 * 1024)
                    && end - range.start <= MAX_RANGE_BYTES
                {
                    range.end = range.end.max(end);
                    range.chunks.push(chunk);
                    continue;
                }
                ranges.push(current.take().unwrap());
            }
            current = Some(PlannedRange {
                pack_hash,
                object_key: super::data_object_key(&pack_hash),
                start: chunk.2,
                end,
                chunks: vec![chunk],
            });
        }
        if let Some(range) = current {
            ranges.push(range);
        }
    }
    Ok(ranges)
}

pub async fn download_and_verify_range(
    provider: &dyn SyncProvider,
    context: &TransportContext,
    range: &PlannedRange,
    cache_root: &Path,
) -> anyhow::Result<PathBuf> {
    if range.end <= range.start || range.end - range.start > MAX_RANGE_BYTES {
        anyhow::bail!("planned media range exceeds hard cap");
    }
    let download = provider
        .object_get_range(
            context,
            &range.object_key,
            range.start,
            range.end,
            super::DATA_PACK_MAX_BYTES,
        )
        .await
        .map_err(anyhow::Error::new)?;
    let selected = read_validated_response(
        &download,
        &range.object_key,
        range.pack_hash,
        range.start,
        range.end,
        super::DATA_PACK_MAX_BYTES,
    )?;
    validate_downloaded_chunks(&selected, range)?;
    fs::create_dir_all(cache_root.join("ranges"))?;
    let relative = PathBuf::from("ranges").join(format!(
        "{}-{}-{}.range",
        super::hex_hash(&range.pack_hash),
        range.start,
        range.end
    ));
    let final_path = cache_root.join(&relative);
    let hash: [u8; 32] = Sha256::digest(&selected).into();
    super::packer::durable_publish(cache_root, &relative, &selected, hash)?;
    Ok(final_path)
}

pub async fn fetch_manifest(
    provider: &dyn SyncProvider,
    context: &TransportContext,
    manifest_hash: [u8; 32],
    manifest_pack_hash: [u8; 32],
    offset: u64,
    length: u64,
) -> anyhow::Result<ContentManifestV1> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| anyhow::anyhow!("manifest range overflows"))?;
    if length == 0 || length > super::MANIFEST_PACK_MAX_BYTES {
        anyhow::bail!("manifest record exceeds hard cap");
    }
    let object_key = super::manifest_object_key(&manifest_pack_hash);
    let download = provider
        .object_get_range(
            context,
            &object_key,
            offset,
            end,
            super::MANIFEST_PACK_MAX_BYTES,
        )
        .await
        .map_err(anyhow::Error::new)?;
    let selected = read_validated_response(
        &download,
        &object_key,
        manifest_pack_hash,
        offset,
        end,
        super::MANIFEST_PACK_MAX_BYTES,
    )?;
    if Sha256::digest(&selected).as_slice() != manifest_hash {
        anyhow::bail!("downloaded manifest hash mismatch");
    }
    ContentManifestV1::decode(&selected)
}

pub fn read_verified_chunks(
    path: &Path,
    range: &PlannedRange,
    expected_range_hash: Option<[u8; 32]>,
) -> anyhow::Result<BTreeMap<[u8; 32], VerifiedChunkSource>> {
    let bytes = fs::read(path)?;
    if bytes.len() as u64 != range.end - range.start {
        anyhow::bail!("verified range length mismatch");
    }
    if expected_range_hash.is_some_and(|hash| Sha256::digest(&bytes).as_slice() != hash) {
        anyhow::bail!("verified range cache hash mismatch");
    }
    let mut chunks = BTreeMap::new();
    for (_, hash, offset, length) in &range.chunks {
        let start = usize::try_from(
            offset
                .checked_sub(range.start)
                .ok_or_else(|| anyhow::anyhow!("chunk starts before verified range"))?,
        )?;
        let end = start
            .checked_add(usize::try_from(*length)?)
            .ok_or_else(|| anyhow::anyhow!("verified range chunk overflows"))?;
        if end > bytes.len() || Sha256::digest(&bytes[start..end]).as_slice() != hash {
            anyhow::bail!("verified range chunk hash mismatch");
        }
        chunks.insert(
            *hash,
            VerifiedChunkSource {
                path: path.to_path_buf(),
                offset: start as u64,
                length: *length,
            },
        );
    }
    Ok(chunks)
}

fn read_validated_response(
    download: &DownloadedRange,
    expected_key: &str,
    expected_object_hash: [u8; 32],
    requested_start: u64,
    requested_end: u64,
    max_response_bytes: u64,
) -> anyhow::Result<Vec<u8>> {
    if requested_end <= requested_start
        || requested_end - requested_start > max_response_bytes
        || download.requested_start != requested_start
        || download.requested_end != requested_end
    {
        anyhow::bail!("provider range request echo mismatch");
    }
    if download.metadata.key != expected_key || download.metadata.sha256 != expected_object_hash {
        anyhow::bail!("provider range object identity mismatch");
    }
    let file_length = fs::metadata(&download.path)?.len();
    let (selection_start, response_length, whole_hash) = if download.whole_object {
        if download.response_start != 0
            || download.response_end != download.metadata.size
            || file_length != download.metadata.size
            || download.metadata.size > max_response_bytes
            || requested_end > download.metadata.size
        {
            anyhow::bail!("provider whole-object range fallback is invalid");
        }
        (
            requested_start,
            download.metadata.size,
            Some(expected_object_hash),
        )
    } else {
        let requested_length = requested_end - requested_start;
        if download.response_start != requested_start
            || download.response_end != requested_end
            || file_length != requested_length
        {
            anyhow::bail!("provider returned wrong range");
        }
        (0, requested_length, None)
    };
    let selection_length = requested_end - requested_start;
    let selection_end = selection_start
        .checked_add(selection_length)
        .ok_or_else(|| anyhow::anyhow!("provider range selection overflows"))?;
    let mut file = File::open(&download.path)?;
    let mut selected = Vec::with_capacity(usize::try_from(selection_length)?);
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut position = 0_u64;
    let mut hasher = whole_hash.map(|_| Sha256::new());
    while position < response_length {
        let remaining = usize::try_from((response_length - position).min(buffer.len() as u64))?;
        let read = file.read(&mut buffer[..remaining])?;
        if read == 0 {
            anyhow::bail!("provider range response is truncated");
        }
        if let Some(hasher) = &mut hasher {
            hasher.update(&buffer[..read]);
        }
        let block_end = position + read as u64;
        let overlap_start = position.max(selection_start);
        let overlap_end = block_end.min(selection_end);
        if overlap_start < overlap_end {
            let local_start = usize::try_from(overlap_start - position)?;
            let local_end = usize::try_from(overlap_end - position)?;
            selected.extend_from_slice(&buffer[local_start..local_end]);
        }
        position = block_end;
    }
    if hasher.is_some_and(|hasher| hasher.finalize().as_slice() != expected_object_hash) {
        anyhow::bail!("provider whole-object range fallback hash mismatch");
    }
    if selected.len() as u64 != selection_length {
        anyhow::bail!("provider range selection length mismatch");
    }
    Ok(selected)
}

fn validate_downloaded_chunks(selected: &[u8], range: &PlannedRange) -> anyhow::Result<()> {
    for (_, hash, offset, length) in &range.chunks {
        let local_start = usize::try_from(
            offset
                .checked_sub(range.start)
                .ok_or_else(|| anyhow::anyhow!("chunk starts before verified range"))?,
        )?;
        let local_end = local_start
            .checked_add(*length as usize)
            .ok_or_else(|| anyhow::anyhow!("range slice overflows"))?;
        if local_end > selected.len()
            || Sha256::digest(&selected[local_start..local_end]).as_slice() != hash
        {
            anyhow::bail!("downloaded chunk hash mismatch");
        }
    }
    Ok(())
}

pub fn persist_verified_range(
    database: &crate::db::Database,
    range: &PlannedRange,
    path: &Path,
    now: i64,
) -> anyhow::Result<()> {
    let bytes = fs::read(path)?;
    let hash: [u8; 32] = Sha256::digest(&bytes).into();
    let relative = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("range cache path has no file name"))?
        .to_string_lossy();
    inventory::record_verified_range(
        database.connection_for_fsops(),
        range.pack_hash,
        range.start,
        range.end,
        hash,
        &format!("ranges/{relative}"),
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::media::format::{ChunkingProfile, ManifestChunkV1, MediaHint};

    #[test]
    fn adjacent_chunks_coalesce_but_ranges_stay_bounded() {
        let pack = [1; 32];
        let manifest = ContentManifestV1 {
            format_version: 1,
            byte_length: 6,
            content_sha256: super::super::hex_hash(&[2; 32]),
            media_hint: MediaHint {
                sniffed_kind: None,
                mime: None,
            },
            chunking: ChunkingProfile::default(),
            chunks: vec![
                ManifestChunkV1 {
                    ordinal: 0,
                    chunk_sha256: super::super::hex_hash(&[3; 32]),
                    byte_length: 3,
                    pack_sha256: super::super::hex_hash(&pack),
                    pack_offset: 0,
                },
                ManifestChunkV1 {
                    ordinal: 1,
                    chunk_sha256: super::super::hex_hash(&[4; 32]),
                    byte_length: 3,
                    pack_sha256: super::super::hex_hash(&pack),
                    pack_offset: 3,
                },
            ],
        };
        let ranges = plan_ranges(&manifest).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!((ranges[0].start, ranges[0].end), (0, 6));
    }

    #[test]
    fn whole_object_fallback_is_stream_verified_and_sliced() {
        let root = std::env::temp_dir().join(format!(
            "megle-media-range-response-{}",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("object.pack");
        let bytes = b"prefix-requested-suffix";
        std::fs::write(&path, bytes).unwrap();
        let object_hash: [u8; 32] = Sha256::digest(bytes).into();
        let download = DownloadedRange {
            metadata: crate::sync::provider::ObjectMetadata {
                key: "objects/data/test".into(),
                size: bytes.len() as u64,
                sha256: object_hash,
                etag: None,
            },
            requested_start: 7,
            requested_end: 16,
            response_start: 0,
            response_end: bytes.len() as u64,
            whole_object: true,
            path,
        };
        let selected =
            read_validated_response(&download, "objects/data/test", object_hash, 7, 16, 1024)
                .unwrap();
        assert_eq!(selected, b"requested");
        let _ = std::fs::remove_dir_all(root);
    }
}
