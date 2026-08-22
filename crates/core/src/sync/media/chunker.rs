use super::{CDC_MAX_BYTES, CDC_MIN_BYTES, CDC_TARGET_BYTES, SMALL_FILE_THRESHOLD};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
    time::UNIX_EPOCH,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkDescriptor {
    pub ordinal: u32,
    pub sha256: [u8; 32],
    pub byte_length: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StableSourceIdentity {
    pub path: PathBuf,
    pub file_identity: Option<String>,
    pub byte_length: u64,
    pub source_mtime_ms: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkedSource {
    pub identity: StableSourceIdentity,
    pub content_sha256: [u8; 32],
    pub chunks: Vec<ChunkDescriptor>,
}

pub fn chunk_reader<R, F>(
    mut reader: R,
    expected_length: u64,
    cancelled: &AtomicBool,
    mut on_chunk: F,
) -> anyhow::Result<([u8; 32], Vec<ChunkDescriptor>)>
where
    R: Read,
    F: FnMut(&ChunkDescriptor, &[u8]) -> anyhow::Result<()>,
{
    let mut full = Sha256::new();
    let mut chunks = Vec::new();
    if expected_length == 0 {
        return Ok((full.finalize().into(), chunks));
    }
    let mut current = Vec::with_capacity(CDC_TARGET_BYTES as usize);
    let mut buffer = vec![0u8; 256 * 1024];
    let mut rolling = 0u64;
    const WINDOW: usize = 63;
    let mut window = [0u8; WINDOW];
    let mut window_index = 0usize;
    let mut window_filled = 0usize;
    let strict_mask = (CDC_TARGET_BYTES * 2).next_power_of_two() - 1;
    let loose_mask = (CDC_TARGET_BYTES / 2).next_power_of_two() - 1;
    let force_single = expected_length <= SMALL_FILE_THRESHOLD;
    let mut observed = 0u64;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            anyhow::bail!("media chunking cancelled");
        }
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        full.update(&buffer[..count]);
        observed = observed
            .checked_add(count as u64)
            .ok_or_else(|| anyhow::anyhow!("source length overflow"))?;
        for byte in &buffer[..count] {
            current.push(*byte);
            rolling = rolling.rotate_left(1);
            if window_filled == WINDOW {
                rolling ^= gear()[window[window_index] as usize].rotate_left(WINDOW as u32);
            } else {
                window_filled += 1;
            }
            rolling ^= gear()[*byte as usize];
            window[window_index] = *byte;
            window_index = (window_index + 1) % WINDOW;
            let length = current.len() as u64;
            let mask = if length < CDC_TARGET_BYTES {
                strict_mask
            } else {
                loose_mask
            };
            let boundary = !force_single
                && length >= CDC_MIN_BYTES
                && ((rolling & mask) == 0 || length >= CDC_MAX_BYTES);
            if boundary {
                emit_chunk(&mut chunks, &mut current, &mut on_chunk)?;
            }
        }
    }
    if observed != expected_length {
        anyhow::bail!("source length changed during read");
    }
    if !current.is_empty() {
        emit_chunk(&mut chunks, &mut current, &mut on_chunk)?;
    }
    Ok((full.finalize().into(), chunks))
}

pub fn stable_chunk_file<F>(
    path: &Path,
    cancelled: &AtomicBool,
    on_chunk: F,
) -> anyhow::Result<ChunkedSource>
where
    F: FnMut(&ChunkDescriptor, &[u8]) -> anyhow::Result<()>,
{
    let file = File::open(path)?;
    let before = file.metadata()?;
    if !before.is_file() {
        anyhow::bail!("media source is not a regular file");
    }
    let before_identity = crate::library_path::local_file_identity(path).ok();
    let mtime = before
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|value| i64::try_from(value.as_millis()).ok());
    let (content_sha256, chunks) = chunk_reader(
        BufReader::with_capacity(1024 * 1024, &file),
        before.len(),
        cancelled,
        on_chunk,
    )?;
    let after_handle = file.metadata()?;
    let after_path = std::fs::metadata(path)?;
    let after_identity = crate::library_path::local_file_identity(path).ok();
    if before.len() != after_handle.len()
        || before.len() != after_path.len()
        || before.modified().ok() != after_handle.modified().ok()
        || before.modified().ok() != after_path.modified().ok()
        || before_identity != after_identity
    {
        anyhow::bail!("media source changed during stable read");
    }
    Ok(ChunkedSource {
        identity: StableSourceIdentity {
            path: path.to_path_buf(),
            file_identity: before_identity,
            byte_length: before.len(),
            source_mtime_ms: mtime,
        },
        content_sha256,
        chunks,
    })
}

fn emit_chunk<F>(
    chunks: &mut Vec<ChunkDescriptor>,
    current: &mut Vec<u8>,
    on_chunk: &mut F,
) -> anyhow::Result<()>
where
    F: FnMut(&ChunkDescriptor, &[u8]) -> anyhow::Result<()>,
{
    let descriptor = ChunkDescriptor {
        ordinal: u32::try_from(chunks.len())?,
        sha256: Sha256::digest(current.as_slice()).into(),
        byte_length: current.len() as u64,
    };
    on_chunk(&descriptor, current)?;
    chunks.push(descriptor);
    current.clear();
    Ok(())
}

fn gear() -> &'static [u64; 256] {
    static TABLE: OnceLock<[u64; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut values = [0u64; 256];
        for (index, value) in values.iter_mut().enumerate() {
            let mut x = (index as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            *value = x ^ (x >> 31);
        }
        values
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdc_is_deterministic_and_local_insert_reuses_chunks() {
        let mut bytes = vec![0u8; 40 * 1024 * 1024];
        for (block, chunk) in bytes.chunks_mut(32).enumerate() {
            let digest = Sha256::digest((block as u64).to_le_bytes());
            chunk.copy_from_slice(&digest[..chunk.len()]);
        }
        let cancelled = AtomicBool::new(false);
        let (_, first) = chunk_reader(bytes.as_slice(), bytes.len() as u64, &cancelled, |_, _| {
            Ok(())
        })
        .unwrap();
        let mut edited = Vec::with_capacity(bytes.len() + 17);
        edited.extend_from_slice(&bytes[..3 * 1024 * 1024]);
        edited.extend_from_slice(b"localized-insert");
        edited.extend_from_slice(&bytes[3 * 1024 * 1024..]);
        let (_, second) = chunk_reader(
            edited.as_slice(),
            edited.len() as u64,
            &cancelled,
            |_, _| Ok(()),
        )
        .unwrap();
        let first_hashes = first
            .iter()
            .map(|chunk| chunk.sha256)
            .collect::<std::collections::BTreeSet<_>>();
        let reused = second
            .iter()
            .filter(|chunk| first_hashes.contains(&chunk.sha256))
            .map(|chunk| chunk.byte_length)
            .sum::<u64>();
        assert!(reused > 0, "first={first:?} second={second:?}");
        let (_, repeated) =
            chunk_reader(bytes.as_slice(), bytes.len() as u64, &cancelled, |_, _| {
                Ok(())
            })
            .unwrap();
        assert_eq!(first, repeated);
    }

    #[test]
    fn small_and_empty_files_follow_wire_contract() {
        let cancelled = AtomicBool::new(false);
        let (empty_hash, empty) = chunk_reader(&b""[..], 0, &cancelled, |_, _| Ok(())).unwrap();
        assert!(empty.is_empty());
        assert_eq!(empty_hash, Sha256::digest(b"").as_slice());
        let (_, small) = chunk_reader(&b"small"[..], 5, &cancelled, |_, _| Ok(())).unwrap();
        assert_eq!(small.len(), 1);
    }
}
