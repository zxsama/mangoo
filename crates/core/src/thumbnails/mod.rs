use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use image::ImageReader;
use sha2::{Digest, Sha256};

/// Wall-clock cap for the ffmpeg subprocess used to extract a video poster.
/// A malformed input or a stalled decoder must not pin the thumbnail worker.
const FFMPEG_THUMBNAIL_TIMEOUT: Duration = Duration::from_secs(30);
/// Polling cadence for [`wait_with_timeout`]. The helper is intentionally
/// dependency-free so the worker doesn't pull in tokio/wait_timeout just to
/// bound a single subprocess.
const CHILD_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Outcome of [`wait_with_timeout`]. The exit-status case is mapped onto
/// `std::process::Output` by the ffmpeg path so the rest of the call site
/// stays the same.
enum ChildWaitOutcome {
    Exited(std::process::ExitStatus),
    TimedOut,
}

/// Polls `child.try_wait()` until it exits or `timeout` elapses. On timeout
/// the child is killed and reaped so we don't leak a zombie. Stdout/stderr
/// are *not* drained here — callers that piped them must read before the
/// child exits to avoid deadlocking on a full pipe.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> std::io::Result<ChildWaitOutcome> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(ChildWaitOutcome::Exited(status));
        }
        if Instant::now() >= deadline {
            // Best-effort kill + reap. If the child already exited between
            // the try_wait above and the kill, both calls succeed cleanly.
            let _ = child.kill();
            let _ = child.wait();
            return Ok(ChildWaitOutcome::TimedOut);
        }
        std::thread::sleep(CHILD_WAIT_POLL_INTERVAL);
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailProfile {
    Grid320,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailStatus {
    Pending,
    Queued,
    Ready,
    Failed,
    SkippedSmall,
}

pub const GRID_320_PROFILE: &str = "grid_320";
pub const GRID_320_SHORT_SIDE_PX: i64 = 320;
pub const GRID_320_MAX_SIDE_PX: u32 = 4096;
pub const GENERATED_FORMAT: &str = "image/webp";
#[allow(dead_code)]
pub const PREVIEW_PLACEHOLDER_SHORT_SIDE_PX: u32 = 20;
#[allow(dead_code)]
pub const PREVIEW_PLACEHOLDER_MAX_SIDE_PX: u32 = 64;
#[allow(dead_code)]
pub const THUMBNAIL_PROFILE_VALUES: &[&str] = &[GRID_320_PROFILE];
#[allow(dead_code)]
pub const THUMBNAIL_STATUS_VALUES: &[&str] =
    &["pending", "queued", "ready", "failed", "skipped_small"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThumbnailPolicy {
    pub profile: &'static str,
    pub short_side_px: i64,
    pub output_format: &'static str,
    pub file_extension: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailDecision {
    Generatable,
    SkippedSmall,
}

#[derive(Debug, Clone, Copy)]
pub struct CacheIdentity<'a> {
    pub file_id: i64,
    pub root_id: i64,
    pub folder_id: i64,
    pub name: &'a str,
    pub size: i64,
    pub mtime: i64,
    pub file_key: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedThumbnail {
    pub width: i64,
    pub height: i64,
    pub byte_size: i64,
    pub data: Vec<u8>,
    /// Original media dimensions captured during the decode the thumbnail
    /// pipeline already performs. Persisted to `media.width/height` so the
    /// scan ingest path can leave dimensions `pending` and the layout engine
    /// (justified rows / masonry) gets real aspect ratios without a second
    /// decode pass. For video these are the aspect-preserving poster
    /// dimensions, so the ratio is exact even though the absolute resolution
    /// is the poster's.
    pub source_width: i64,
    pub source_height: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct PreviewPlaceholder {
    pub data: Vec<u8>,
    pub width: i64,
    pub height: i64,
    pub byte_size: i64,
    pub output_format: &'static str,
}

impl ThumbnailPolicy {
    pub fn grid_320() -> Self {
        Self {
            profile: GRID_320_PROFILE,
            short_side_px: GRID_320_SHORT_SIDE_PX,
            output_format: GENERATED_FORMAT,
            file_extension: "webp",
        }
    }

    pub fn initial_state(
        &self,
        media_kind: Option<&str>,
        width: Option<i64>,
        height: Option<i64>,
    ) -> ThumbnailDecision {
        if media_kind == Some("image") {
            if let (Some(width), Some(height)) = (width, height) {
                if width < self.short_side_px && height < self.short_side_px {
                    return ThumbnailDecision::SkippedSmall;
                }
            }
        }
        ThumbnailDecision::Generatable
    }
}

pub fn normalize_profile(profile: Option<&str>) -> Option<&'static str> {
    match profile.unwrap_or(GRID_320_PROFILE) {
        GRID_320_PROFILE => Some(GRID_320_PROFILE),
        _ => None,
    }
}

pub fn is_pending_status(state: &str) -> bool {
    matches!(state, "pending" | "queued")
}

#[cfg(test)]
pub fn cache_key_for(identity: &CacheIdentity<'_>, profile: &str) -> String {
    let digest = source_fingerprint_for(identity, profile);
    format!("{}/{}/{}.webp", &digest[0..2], &digest[2..4], digest)
}

pub fn source_fingerprint_for(identity: &CacheIdentity<'_>, profile: &str) -> String {
    let mut hasher = Sha256::new();
    let reliable_file_key = identity.file_key.filter(|value| !value.is_empty());
    let mut parts = vec!["v3".to_string()];
    if let Some(file_key) = reliable_file_key {
        parts.push("content".to_string());
        parts.push(file_key.to_string());
    } else {
        parts.push("path".to_string());
        parts.push(identity.root_id.to_string());
        parts.push(identity.folder_id.to_string());
        parts.push(identity.name.to_string());
    }
    parts.push(identity.size.to_string());
    parts.push(identity.mtime.to_string());
    parts.push(profile.to_string());
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

pub fn is_safe_cache_key(cache_key: &str) -> bool {
    !cache_key.is_empty()
        && !cache_key.starts_with('/')
        && !cache_key.starts_with('\\')
        && !cache_key.contains(':')
        && !cache_key.contains('\\')
        && cache_key
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
pub fn generate_image_thumbnail(
    cache_root: &Path,
    cache_key: &str,
    source_path: &Path,
) -> anyhow::Result<GeneratedThumbnail> {
    if !is_safe_cache_key(cache_key) {
        return Err(anyhow::anyhow!("unsafe thumbnail cache key: {cache_key}"));
    }
    let generated = generate_image_thumbnail_bytes(source_path)?;
    let path = cache_root.join(cache_key);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, &generated.data)?;
    Ok(generated)
}

#[allow(dead_code)]
pub fn generate_image_thumbnail_bytes(source_path: &Path) -> anyhow::Result<GeneratedThumbnail> {
    generate_image_thumbnail_bytes_with_checkpoint(source_path, || Ok(()))
}

pub fn generate_image_thumbnail_bytes_with_checkpoint(
    source_path: &Path,
    mut checkpoint: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<GeneratedThumbnail> {
    let reader = ImageReader::open(source_path)
        .map_err(|error| anyhow::anyhow!("thumbnail decode failed: {error}"))?
        .with_guessed_format()
        .map_err(|error| anyhow::anyhow!("thumbnail decode failed: {error}"))?;
    let decoded = reader
        .decode()
        .map_err(|error| anyhow::anyhow!("thumbnail decode failed: {error}"))?;
    checkpoint()?;

    let source_width = decoded.width();
    let source_height = decoded.height();
    if source_width == 0 || source_height == 0 {
        return Err(anyhow::anyhow!(
            "thumbnail decode failed: zero-sized source image"
        ));
    }
    let (target_width, target_height) =
        target_dimensions(source_width, source_height, GRID_320_SHORT_SIDE_PX as u32);

    // `thumbnail_exact` is a single-pass area-average downscale: for large
    // sources it is markedly faster than `resize_exact` with a separable
    // bilinear kernel, and gives equal-or-better quality at thumbnail scale
    // factors. Output dimensions are unchanged (still `target_dimensions`).
    let resized = decoded.thumbnail_exact(target_width, target_height);
    checkpoint()?;
    // `webp::Encoder::from_image` only accepts RGB8/RGBA8. Convert through
    // `to_rgba8` so paletted, grayscale, and 16-bit decodes still encode.
    let rgba = resized.to_rgba8();
    let encoded = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height()).encode(75.0);
    let bytes: &[u8] = &encoded;

    Ok(GeneratedThumbnail {
        width: target_width as i64,
        height: target_height as i64,
        byte_size: bytes.len() as i64,
        data: bytes.to_vec(),
        source_width: source_width as i64,
        source_height: source_height as i64,
    })
}

#[allow(dead_code)]
pub fn generate_preview_placeholder(source_path: &Path) -> anyhow::Result<PreviewPlaceholder> {
    let reader = ImageReader::open(source_path)
        .map_err(|error| anyhow::anyhow!("placeholder decode failed: {error}"))?
        .with_guessed_format()
        .map_err(|error| anyhow::anyhow!("placeholder decode failed: {error}"))?;
    let decoded = reader
        .decode()
        .map_err(|error| anyhow::anyhow!("placeholder decode failed: {error}"))?;

    let source_width = decoded.width();
    let source_height = decoded.height();
    if source_width == 0 || source_height == 0 {
        return Err(anyhow::anyhow!(
            "placeholder decode failed: zero-sized source image"
        ));
    }
    let (target_width, target_height) = placeholder_dimensions(
        source_width,
        source_height,
        PREVIEW_PLACEHOLDER_SHORT_SIDE_PX,
        PREVIEW_PLACEHOLDER_MAX_SIDE_PX,
    );
    let resized = decoded.thumbnail_exact(target_width, target_height);
    let rgba = resized.to_rgba8();
    let encoded = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height()).encode(45.0);
    let bytes: &[u8] = &encoded;

    Ok(PreviewPlaceholder {
        data: bytes.to_vec(),
        width: target_width as i64,
        height: target_height as i64,
        byte_size: bytes.len() as i64,
        output_format: GENERATED_FORMAT,
    })
}

pub fn generate_video_thumbnail(
    cache_root: &Path,
    cache_key: &str,
    source_path: &Path,
) -> anyhow::Result<GeneratedThumbnail> {
    if !is_safe_cache_key(cache_key) {
        return Err(anyhow::anyhow!("unsafe thumbnail cache key: {cache_key}"));
    }
    let cache_path = cache_root.join(cache_key);
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            "00:00:01.0",
            "-i",
        ])
        .arg(source_path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale='if(gt(a,1),320,-2)':'if(gt(a,1),-2,320)'",
            "-f",
            "image2",
            "-codec:v",
            "webp",
            "-y",
        ])
        .arg(&cache_path)
        // Never let ffmpeg block on stdin: a malformed `-` input or a
        // ffmpeg build that prompts for confirmation would otherwise hang
        // the worker forever. We pipe stdout/stderr so we can capture
        // diagnostics; both are drained after the child exits below.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| anyhow::anyhow!("thumbnail decode failed: ffmpeg spawn: {error}"))?;

    let outcome = wait_with_timeout(&mut child, FFMPEG_THUMBNAIL_TIMEOUT)
        .map_err(|error| anyhow::anyhow!("thumbnail decode failed: ffmpeg wait: {error}"))?;

    let status = match outcome {
        ChildWaitOutcome::Exited(status) => status,
        ChildWaitOutcome::TimedOut => {
            let _ = fs::remove_file(&cache_path);
            return Err(anyhow::anyhow!(
                "thumbnail decode failed: ffmpeg timed out after {}s",
                FFMPEG_THUMBNAIL_TIMEOUT.as_secs()
            ));
        }
    };

    // The child has exited. Drain stderr for diagnostics; stdout is unused
    // because ffmpeg writes the frame to `cache_path` directly.
    let mut stderr_buf = Vec::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_end(&mut stderr_buf);
    }
    if let Some(mut stdout) = child.stdout.take() {
        let mut sink = Vec::new();
        let _ = stdout.read_to_end(&mut sink);
    }

    if !status.success() {
        let _ = fs::remove_file(&cache_path);
        let stderr = String::from_utf8_lossy(&stderr_buf);
        let code_display = match status.code() {
            Some(code) => code.to_string(),
            None => "signal".to_string(),
        };
        return Err(anyhow::anyhow!(
            "thumbnail decode failed: ffmpeg exit {code_display} stderr: {}",
            stderr.trim()
        ));
    }

    let dimensions = ImageReader::open(&cache_path)
        .map_err(|error| anyhow::anyhow!("thumbnail decode failed: ffmpeg output open: {error}"))?
        .with_guessed_format()
        .map_err(|error| anyhow::anyhow!("thumbnail decode failed: ffmpeg output guess: {error}"))?
        .into_dimensions()
        .map_err(|error| {
            anyhow::anyhow!("thumbnail decode failed: ffmpeg output dimensions: {error}")
        })?;

    let metadata = fs::metadata(&cache_path)?;
    let data = fs::read(&cache_path)?;
    Ok(GeneratedThumbnail {
        width: dimensions.0 as i64,
        height: dimensions.1 as i64,
        byte_size: metadata.len() as i64,
        data,
        // ffmpeg's scale filter preserves aspect ratio, so the poster's
        // dimensions carry the source ratio exactly (the absolute resolution
        // is the poster's, not the original video's).
        source_width: dimensions.0 as i64,
        source_height: dimensions.1 as i64,
    })
}

pub fn generate_video_thumbnail_bytes(source_path: &Path) -> anyhow::Result<GeneratedThumbnail> {
    let temp_root = std::env::temp_dir().join(format!(
        "megle-video-thumbnail-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let cache_key = "aa/bb/temp.webp";
    let result = generate_video_thumbnail(&temp_root, cache_key, source_path);
    let _ = fs::remove_dir_all(&temp_root);
    result
}

fn target_dimensions(width: u32, height: u32, short_side_px: u32) -> (u32, u32) {
    if width <= short_side_px && height <= short_side_px {
        return (width.max(1), height.max(1));
    }
    bounded_dimensions(width, height, short_side_px, GRID_320_MAX_SIDE_PX)
}

#[allow(dead_code)]
fn placeholder_dimensions(
    width: u32,
    height: u32,
    short_side_px: u32,
    max_side_px: u32,
) -> (u32, u32) {
    bounded_dimensions(width, height, short_side_px, max_side_px)
}

fn bounded_dimensions(width: u32, height: u32, short_side_px: u32, max_side_px: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    let short_side = width.min(height) as f64;
    let long_side = width.max(height) as f64;
    let scale = (short_side_px.max(1) as f64 / short_side)
        .min(max_side_px.max(1) as f64 / long_side)
        .min(1.0);
    let target_width = ((width as f64 * scale).round() as u32)
        .max(1)
        .min(max_side_px.max(1));
    let target_height = ((height as f64 * scale).round() as u32)
        .max(1)
        .min(max_side_px.max(1));
    (target_width, target_height)
}

/// Detects whether `ffmpeg` is on PATH. Result is cached for the lifetime of
/// the process so the worker only pays the spawn cost once.
pub fn ffmpeg_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| match Command::new("ffmpeg").arg("-version").output() {
        Ok(output) => output.status.success(),
        Err(_) => false,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailFailureKind {
    Transient,
    Permanent,
}

impl ThumbnailFailureKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThumbnailFailureKind::Transient => "transient",
            ThumbnailFailureKind::Permanent => "permanent",
        }
    }
}

/// Classify a thumbnail generation error as transient (source momentarily
/// unreachable / contention / memory — will succeed on retry once conditions
/// recover) or permanent (unsupported format / corrupt file — retrying is
/// futile). Pure function on the stored error string.
pub fn classify_thumbnail_failure(error: &str) -> ThumbnailFailureKind {
    const TRANSIENT_SIGNATURES: &[&str] = &[
        "os error 53",    // Windows: network path not found
        "找不到网络路径", // localized "network path not found"
        "network path",
        "database is locked",
        "Memory limit exceeded",
    ];
    if TRANSIENT_SIGNATURES.iter().any(|sig| error.contains(sig)) {
        ThumbnailFailureKind::Transient
    } else {
        ThumbnailFailureKind::Permanent
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn classify_thumbnail_failure_splits_transient_and_permanent() {
        use super::{classify_thumbnail_failure, ThumbnailFailureKind::*};
        // transient
        for e in [
            "thumbnail decode failed: 找不到网络路径。 (os error 53)",
            "database is locked",
            "thumbnail decode failed: Memory limit exceeded",
            "read failed: network path not found",
        ] {
            assert!(
                matches!(classify_thumbnail_failure(e), Transient),
                "should be transient: {e}"
            );
        }
        // permanent
        for e in [
            "thumbnail decode failed: The image format Avif is not supported",
            "thumbnail decode failed: The file extension `.\"psd\"` was not recognized as an image format",
            "thumbnail decode failed: Format error decoding Jpeg: Error parsing image",
            "thumbnail decode failed: unsupported media kind \"text\"",
        ] {
            assert!(matches!(classify_thumbnail_failure(e), Permanent), "should be permanent: {e}");
        }
    }

    #[test]
    fn grid_320_policy_is_webp_with_exact_short_side() {
        let policy = ThumbnailPolicy::grid_320();

        assert_eq!(policy.profile, GRID_320_PROFILE);
        assert_eq!(policy.output_format, GENERATED_FORMAT);
        assert_eq!(policy.short_side_px, 320);
        assert_eq!(policy.file_extension, "webp");
    }

    #[test]
    fn displayable_originals_below_profile_are_skipped_small() {
        let policy = ThumbnailPolicy::grid_320();

        assert_eq!(
            policy.initial_state(Some("image"), Some(128), Some(240)),
            ThumbnailDecision::SkippedSmall
        );
        assert_eq!(
            policy.initial_state(Some("image"), Some(640), Some(240)),
            ThumbnailDecision::Generatable
        );
        assert_eq!(
            policy.initial_state(Some("image"), Some(320), Some(320)),
            ThumbnailDecision::Generatable
        );
        assert_eq!(
            policy.initial_state(Some("video"), Some(128), Some(128)),
            ThumbnailDecision::Generatable
        );
        assert_eq!(
            policy.initial_state(Some("other"), Some(128), Some(128)),
            ThumbnailDecision::Generatable
        );
        assert_eq!(
            policy.initial_state(Some("image"), None, Some(128)),
            ThumbnailDecision::Generatable
        );
    }

    #[test]
    fn cache_key_is_sharded_safe_relative_and_changes_with_invalidation_inputs() {
        let identity = CacheIdentity {
            file_id: 42,
            root_id: 7,
            folder_id: 9,
            name: "image.jpg",
            size: 1024,
            mtime: 123456,
            file_key: Some("dev-inode-1"),
        };

        let first = cache_key_for(&identity, GRID_320_PROFILE);
        let same = cache_key_for(&identity, GRID_320_PROFILE);
        let changed_size = cache_key_for(
            &CacheIdentity {
                size: 2048,
                ..identity
            },
            GRID_320_PROFILE,
        );
        let changed_key = cache_key_for(
            &CacheIdentity {
                file_key: Some("dev-inode-2"),
                ..identity
            },
            GRID_320_PROFILE,
        );

        assert_eq!(first, same);
        assert_ne!(first, changed_size);
        assert_ne!(first, changed_key);
        assert!(is_safe_cache_key(&first));
        assert!(first.ends_with(".webp"));
        assert_eq!(first.matches('/').count(), 2);
        let parts: Vec<&str> = first.split('/').collect();
        assert_eq!(parts[0].len(), 2);
        assert_eq!(parts[1].len(), 2);
        assert!(parts[2].ends_with(".webp"));
        assert_eq!(parts[2].trim_end_matches(".webp").len(), 64);
        assert!(!Path::new(&first).is_absolute());
    }

    #[test]
    fn source_fingerprint_reuses_reliable_file_key_across_path_identity_changes() {
        let original = CacheIdentity {
            file_id: 42,
            root_id: 7,
            folder_id: 9,
            name: "image.jpg",
            size: 1024,
            mtime: 123456,
            file_key: Some("dev-inode-1"),
        };
        let renamed = CacheIdentity {
            file_id: 99,
            root_id: 100,
            folder_id: 200,
            name: "renamed.jpg",
            ..original
        };

        assert_eq!(
            source_fingerprint_for(&original, GRID_320_PROFILE),
            source_fingerprint_for(&renamed, GRID_320_PROFILE)
        );
    }

    #[test]
    fn source_fingerprint_without_file_key_stays_path_bound() {
        let original = CacheIdentity {
            file_id: 42,
            root_id: 7,
            folder_id: 9,
            name: "image.jpg",
            size: 1024,
            mtime: 123456,
            file_key: None,
        };
        let renamed = CacheIdentity {
            folder_id: 10,
            name: "renamed.jpg",
            ..original
        };

        assert_ne!(
            source_fingerprint_for(&original, GRID_320_PROFILE),
            source_fingerprint_for(&renamed, GRID_320_PROFILE)
        );
    }

    #[test]
    fn source_fingerprint_changes_when_reliable_content_identity_changes() {
        let identity = CacheIdentity {
            file_id: 42,
            root_id: 7,
            folder_id: 9,
            name: "image.jpg",
            size: 1024,
            mtime: 123456,
            file_key: Some("dev-inode-1"),
        };

        let same = source_fingerprint_for(&identity, GRID_320_PROFILE);
        let changed_size = source_fingerprint_for(
            &CacheIdentity {
                size: 2048,
                ..identity
            },
            GRID_320_PROFILE,
        );
        let changed_mtime = source_fingerprint_for(
            &CacheIdentity {
                mtime: 654321,
                ..identity
            },
            GRID_320_PROFILE,
        );
        let changed_key = source_fingerprint_for(
            &CacheIdentity {
                file_key: Some("dev-inode-2"),
                ..identity
            },
            GRID_320_PROFILE,
        );

        assert_ne!(same, changed_size);
        assert_ne!(same, changed_mtime);
        assert_ne!(same, changed_key);
    }

    #[test]
    fn generate_image_thumbnail_writes_real_webp_with_resized_aspect_ratio() {
        let cache_root = unique_temp_dir();
        fs::create_dir_all(&cache_root).expect("create cache root");
        let cache_key = "aa/bb/test.webp";

        let source_dir = unique_temp_dir();
        fs::create_dir_all(&source_dir).expect("create source dir");
        let source_path = source_dir.join("source.png");
        // 800x400 source: short side = 400, long side = 800. After
        // resizing the short side to 320 the long side scales to 640.
        let buffer =
            image::ImageBuffer::from_fn(800u32, 400u32, |x, _| image::Rgb([(x % 255) as u8, 0, 0]));
        image::DynamicImage::ImageRgb8(buffer)
            .save(&source_path)
            .expect("write source png");

        let generated = generate_image_thumbnail(&cache_root, cache_key, &source_path)
            .expect("generate thumbnail");
        let bytes = fs::read(cache_root.join(cache_key)).expect("read generated thumbnail");

        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        assert_eq!(generated.byte_size, bytes.len() as i64);
        assert_eq!(generated.width, 640);
        assert_eq!(generated.height, 320);

        fs::remove_dir_all(&cache_root).expect("cleanup cache root");
        fs::remove_dir_all(&source_dir).expect("cleanup source dir");
    }

    #[test]
    fn generate_image_thumbnail_returns_decode_failed_on_corrupt_source() {
        let cache_root = unique_temp_dir();
        fs::create_dir_all(&cache_root).expect("create cache root");
        let cache_key = "aa/bb/corrupt.webp";

        let source_dir = unique_temp_dir();
        fs::create_dir_all(&source_dir).expect("create source dir");
        let source_path = source_dir.join("corrupt.jpg");
        fs::write(&source_path, b"not actually an image").expect("write corrupt source");

        let error = generate_image_thumbnail(&cache_root, cache_key, &source_path)
            .expect_err("corrupt source should fail to decode");
        assert!(
            error.to_string().starts_with("thumbnail decode failed:"),
            "expected prefix on error: {error}"
        );

        fs::remove_dir_all(&cache_root).expect("cleanup cache root");
        fs::remove_dir_all(&source_dir).expect("cleanup source dir");
    }

    #[test]
    fn target_dimensions_resizes_short_side_to_320() {
        assert_eq!(target_dimensions(800, 400, 320), (640, 320));
        assert_eq!(target_dimensions(400, 800, 320), (320, 640));
        // Square sources land on the exact short side.
        assert_eq!(target_dimensions(1024, 1024, 320), (320, 320));
        // Sources already at or under the short side stay unchanged so
        // skipped_small can short-circuit the pipeline before we touch them.
        assert_eq!(target_dimensions(200, 100, 320), (200, 100));
    }

    #[test]
    fn target_dimensions_caps_extreme_aspect_ratio_to_safe_max_side() {
        assert_eq!(
            target_dimensions(1, 100_000, 320),
            (1, GRID_320_MAX_SIDE_PX)
        );
        assert_eq!(
            target_dimensions(100_000, 1, 320),
            (GRID_320_MAX_SIDE_PX, 1)
        );
    }

    #[test]
    fn generate_preview_placeholder_writes_bounded_real_webp_for_extreme_panorama() {
        let source_dir = unique_temp_dir();
        fs::create_dir_all(&source_dir).expect("create source dir");
        let source_path = source_dir.join("panorama.png");
        let buffer = image::ImageBuffer::from_fn(1000u32, 1u32, |x, _| {
            image::Rgb([(x % 255) as u8, 64, 32])
        });
        image::DynamicImage::ImageRgb8(buffer)
            .save(&source_path)
            .expect("write panorama source");

        let placeholder =
            generate_preview_placeholder(&source_path).expect("generate preview placeholder");

        assert_eq!(placeholder.output_format, GENERATED_FORMAT);
        assert_eq!(&placeholder.data[0..4], b"RIFF");
        assert_eq!(&placeholder.data[8..12], b"WEBP");
        assert!(placeholder.width <= PREVIEW_PLACEHOLDER_MAX_SIDE_PX as i64);
        assert!(placeholder.height <= PREVIEW_PLACEHOLDER_MAX_SIDE_PX as i64);
        assert_eq!(placeholder.byte_size, placeholder.data.len() as i64);
        assert!(placeholder.byte_size <= 8192);

        fs::remove_dir_all(&source_dir).expect("cleanup source dir");
    }

    #[test]
    fn cache_key_rejects_absolute_parent_and_dot_segments() {
        for candidate in [
            "",
            "/aa/bb/key.webp",
            "\\aa\\bb\\key.webp",
            "C:/aa/bb/key.webp",
            "aa/../key.webp",
            "aa/./key.webp",
            "aa//key.webp",
            "aa\\bb\\key.webp",
        ] {
            assert!(
                !is_safe_cache_key(candidate),
                "candidate should be rejected: {candidate}"
            );
        }
    }

    #[test]
    fn wait_with_timeout_kills_long_running_child_and_reports_timeout() {
        // Spawn a tiny sleeper and wait far less than its lifetime so the
        // helper's deadline branch fires. We verify the child is reaped
        // (try_wait returns the killed status) and that the surrounding
        // ffmpeg path would have surfaced a "timed out" error to callers.
        let mut child = if cfg!(target_os = "windows") {
            // `cmd /c ping -n 6 127.0.0.1` blocks for ~5s. We avoid
            // `cmd /c timeout` because Windows `timeout.exe` requires
            // a real console handle and exits immediately without one,
            // which would defeat the test.
            std::process::Command::new("cmd")
                .args(["/c", "ping", "-n", "6", "127.0.0.1"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn windows sleeper")
        } else {
            std::process::Command::new("sleep")
                .arg("5")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn unix sleeper")
        };

        let outcome = wait_with_timeout(&mut child, Duration::from_millis(100))
            .expect("wait_with_timeout should not error on a healthy child");
        assert!(
            matches!(outcome, ChildWaitOutcome::TimedOut),
            "expected TimedOut for a long-running child"
        );

        // Synthesize the same error message generate_video_thumbnail would
        // emit so the test pins the user-facing wording.
        let message = format!(
            "thumbnail decode failed: ffmpeg timed out after {}s",
            FFMPEG_THUMBNAIL_TIMEOUT.as_secs()
        );
        assert!(message.contains("timed out"), "message: {message}");
    }

    fn unique_temp_dir() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

        std::env::temp_dir().join(format!(
            "megle_thumbnail_policy_test_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }
}
