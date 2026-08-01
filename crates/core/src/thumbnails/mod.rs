use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use image::ImageReader;
use sha2::{Digest, Sha256};

pub use crate::decode::ffmpeg::ffmpeg_available;
use crate::decode::subprocess::{wait_with_timeout, ChildWaitOutcome};

/// Wall-clock cap for the ffmpeg subprocess used to extract a video poster.
/// A malformed input or a stalled decoder must not pin the thumbnail worker.
const FFMPEG_THUMBNAIL_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Adaptive downscaled preview tiers, keyed by the image's LONGEST side.
///
/// Longest-side (not short-side) because the preview's default view mode is
/// `fit-long-edge`, and because it bounds decoded memory unconditionally: a
/// longest-side-2048 tier is at most 2048*2048*4 = 17 MB no matter the aspect
/// ratio, while a short-side-2048 tier on a 6:1 panorama is 100 MB.
// No production caller until Task 7 wires the preview route.
#[allow(dead_code)]
pub const PREVIEW_2048_PROFILE: &str = "preview_2048";
#[allow(dead_code)]
pub const PREVIEW_3072_PROFILE: &str = "preview_3072";
/// Cache profile for a browser-UNRENDERABLE image small enough that no tier
/// applies (PSD/HEIC/TIFF under the threshold): the goal is format conversion,
/// not shrinking, so this is "native resolution" — bounded by
/// [`PREVIEW_NATIVE_MAX_PX`] purely as a safety rail for the case where
/// `media.width/height` are still `pending` and the source turns out to be huge.
#[allow(dead_code)]
pub const PREVIEW_NATIVE_PROFILE: &str = "preview_native";
#[allow(dead_code)]
pub const PREVIEW_NATIVE_MAX_PX: u32 = 4096;
/// The downscaled TIERS, ordered smallest-first; selection walks this list.
/// `PREVIEW_NATIVE_PROFILE` is deliberately absent — it is not a tier.
#[allow(dead_code)]
pub const PREVIEW_PROFILE_VALUES: &[&str] = &[PREVIEW_2048_PROFILE, PREVIEW_3072_PROFILE];
/// Originals whose longest side is at or below this never get a tier — the
/// original is already a reasonable preview and streaming it costs nothing.
#[allow(dead_code)]
pub const PREVIEW_TIER_MIN_SOURCE_MAX_PX: u32 = 4096;
/// Resolution headroom over the viewport (1.2x) so a modest zoom or a
/// fractional device pixel ratio does not immediately look soft.
#[allow(dead_code)]
const PREVIEW_TIER_HEADROOM_NUMERATOR: u32 = 6;
#[allow(dead_code)]
const PREVIEW_TIER_HEADROOM_DENOMINATOR: u32 = 5;
/// WebP quality for preview tiers. Higher than the 75 used for 320px grid
/// thumbs because this IS the image the user looks at.
#[allow(dead_code)]
const PREVIEW_WEBP_QUALITY: f32 = 82.0;

// No production caller until Task 7 wires the preview route.
#[allow(dead_code)]
pub fn preview_profile_max_px(profile: &str) -> Option<u32> {
    match profile {
        PREVIEW_2048_PROFILE => Some(2048),
        PREVIEW_3072_PROFILE => Some(3072),
        _ => None,
    }
}

/// Which cached tier (if any) should back a preview of an image whose longest
/// side is `source_max_px`, viewed in a window whose longest side is
/// `viewport_long_px`. `None` means "serve the original bytes" — which is both
/// the small-image case and the no-viewport-reported case, so a client that
/// sends nothing gets exactly today's behaviour.
// No production caller until Task 7 wires the preview route.
#[allow(dead_code)]
pub fn select_preview_profile(source_max_px: u32, viewport_long_px: u32) -> Option<&'static str> {
    if viewport_long_px == 0 || source_max_px <= PREVIEW_TIER_MIN_SOURCE_MAX_PX {
        return None;
    }
    let required = viewport_long_px
        .saturating_mul(PREVIEW_TIER_HEADROOM_NUMERATOR)
        .div_ceil(PREVIEW_TIER_HEADROOM_DENOMINATOR);
    let profile = PREVIEW_PROFILE_VALUES
        .iter()
        .copied()
        .find(|profile| preview_profile_max_px(profile).unwrap_or(0) >= required)
        // Beyond the largest tier, clamp to it: the frontend's background
        // original-upgrade closes the remaining sharpness gap.
        .unwrap_or(PREVIEW_3072_PROFILE);
    // A tier that is not smaller than the original would be pure loss.
    if preview_profile_max_px(profile).unwrap_or(0) >= source_max_px {
        return None;
    }
    Some(profile)
}

/// Fit `width x height` inside a `max_px` box, preserving aspect ratio and
/// never upscaling. Neither dimension is ever allowed to round to zero.
// No production caller until Task 7 wires the preview route.
#[allow(dead_code)]
pub fn preview_tier_dimensions(width: u32, height: u32, max_px: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest == 0 || max_px == 0 || longest <= max_px {
        return (width, height);
    }
    let scaled_width = ((width as u64 * max_px as u64) / longest as u64) as u32;
    let scaled_height = ((height as u64 * max_px as u64) / longest as u64) as u32;
    (scaled_width.max(1), scaled_height.max(1))
}

// No production caller until Task 7 wires the preview route.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedPreviewTier {
    pub data: Vec<u8>,
    pub width: i64,
    pub height: i64,
    pub byte_size: i64,
    pub output_format: &'static str,
}
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
    let ext = crate::decode::formats::lowercase_extension(source_path);
    // `target_max_px: None` on purpose. `source_width`/`source_height` below are
    // persisted straight into `media.width/height` and drive both the grid's
    // aspect ratios and the preview's natural frame; letting a decoder pre-scale
    // would report the scaled size as the original's. The cost lands once per
    // file, and only on formats that produce nothing at all today.
    let decoded = crate::decode::shared_registry()
        .decode(&crate::decode::DecodeRequest {
            source: crate::decode::MediaSource::Path(source_path),
            ext: ext.as_str(),
            target_max_px: None,
            purpose: crate::decode::DecodePurpose::Thumbnail,
        })
        .map_err(|error| anyhow::anyhow!("thumbnail decode failed: {error}"))?;
    let decoded = decoded
        .into_dynamic()
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

/// Decode `source_path` through the registry and re-encode it as WebP, capped
/// to `max_px` on its longest side. A cap larger than the source is a no-op —
/// this never upscales — so `PREVIEW_NATIVE_MAX_PX` doubles as "native
/// resolution, with a safety rail".
// No production caller until Task 7 wires the preview route.
#[allow(dead_code)]
pub fn generate_preview_webp(
    source_path: &Path,
    max_px: u32,
) -> anyhow::Result<GeneratedPreviewTier> {
    let ext = crate::decode::formats::lowercase_extension(source_path);
    let decoded = crate::decode::shared_registry()
        .decode(&crate::decode::DecodeRequest {
            source: crate::decode::MediaSource::Path(source_path),
            ext: ext.as_str(),
            // Passed as a hint: ffmpeg scales in-process (so a 16k HEIC never
            // materialises at full size), while `image` ignores it and the
            // resize below runs instead. Either way the output honours the cap.
            target_max_px: Some(max_px),
            purpose: crate::decode::DecodePurpose::Preview,
        })
        .map_err(|error| anyhow::anyhow!("preview decode failed: {error}"))?;
    if decoded.width == 0 || decoded.height == 0 {
        return Err(anyhow::anyhow!(
            "preview decode failed: zero-sized source image"
        ));
    }
    let (target_width, target_height) =
        preview_tier_dimensions(decoded.width, decoded.height, max_px);
    let dynamic = decoded
        .into_dynamic()
        .map_err(|error| anyhow::anyhow!("preview decode failed: {error}"))?;
    let resized = if (dynamic.width(), dynamic.height()) == (target_width, target_height) {
        dynamic
    } else {
        // Same single-pass area-average downscale the thumbnail path uses.
        dynamic.thumbnail_exact(target_width, target_height)
    };
    let rgba = resized.to_rgba8();
    let encoded = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
        .encode(PREVIEW_WEBP_QUALITY);
    let bytes: &[u8] = &encoded;
    Ok(GeneratedPreviewTier {
        data: bytes.to_vec(),
        width: rgba.width() as i64,
        height: rgba.height() as i64,
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
///
/// Errors now arrive from the decode chain, so a single string can carry every
/// link's failure ("all decoders failed for \".tif\": image_crate: ...; ffmpeg:
/// ..."). That is exactly the semantics the classifier wants: a `contains`
/// match means ANY link hit a transient condition, so the attempt is worth
/// retrying, while a chain that failed purely on format grounds — or a
/// `no decoder handles` / `decoder unavailable` result — falls through to
/// permanent and is never auto-retried.
pub fn classify_thumbnail_failure(error: &str) -> ThumbnailFailureKind {
    const TRANSIENT_SIGNATURES: &[&str] = &[
        "os error 53",    // Windows: network path not found
        "找不到网络路径", // localized "network path not found"
        "网络",           // bare "network" (zh) — broader Windows network-error catch-all
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
            "thumbnail decode failed: 网络名不再可用。 (os error 64)",
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
    fn chain_failures_stay_permanent_but_inherit_a_transient_link() {
        use super::{classify_thumbnail_failure, ThumbnailFailureKind::*};
        // Nothing in the chain claimed the format: genuinely unsupported.
        assert!(matches!(
            classify_thumbnail_failure("thumbnail decode failed: no decoder handles \".raw\""),
            Permanent
        ));
        // Every claiming decoder failed on format grounds: still permanent.
        assert!(matches!(
            classify_thumbnail_failure(
                "thumbnail decode failed: all decoders failed for \".tif\": image_crate: unsupported compression; ffmpeg: ffmpeg exit 1 stderr: Invalid data"
            ),
            Permanent
        ));
        // One link hit a network drop: the whole attempt is retryable, because
        // the aggregate preserves each link's text verbatim.
        assert!(matches!(
            classify_thumbnail_failure(
                "thumbnail decode failed: all decoders failed for \".tif\": image_crate: os error 53; ffmpeg: ffmpeg spawn: 找不到网络路径。"
            ),
            Transient
        ));
    }

    #[test]
    fn a_missing_ffmpeg_is_permanent_and_names_itself() {
        use super::{classify_thumbnail_failure, ThumbnailFailureKind::*};
        let error = "thumbnail decode failed: all decoders failed for \".heic\": ffmpeg: decoder unavailable: ffmpeg is not installed or not on PATH";
        // Permanent on purpose: an uninstalled ffmpeg will not install itself,
        // and transient would arm the 60s retry sweep against every AVIF/HEIC
        // in the library forever.
        assert!(matches!(classify_thumbnail_failure(error), Permanent));
        assert!(error.contains(crate::decode::DECODER_UNAVAILABLE_MARKER));
    }

    #[test]
    fn heif_is_routed_to_a_decoder_rather_than_reported_unsupported() {
        // Scope: this locks the DECODER half of `.heif` support only —
        // `FfmpegDecoder::SUPPORTED_EXTENSIONS` claiming it, and the lowercasing
        // in `lowercase_extension` being on the thumbnail path.
        //
        // It does NOT cover `formats::IMAGE_EXTENSIONS`: this entry point never
        // reads that table. `IMAGE_EXTENSIONS` gates *indexing*, in scan and the
        // watcher. The join of the two halves lives in
        // `scan::tests::scan_indexes_heif_as_an_image_through_the_shared_format_table`.
        let dir = std::env::temp_dir().join(format!(
            "megle_thumb_heif_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        // Uppercase on purpose: proves the lowercasing in lowercase_extension
        // is on the path too.
        let source = dir.join("photo.HEIF");
        std::fs::write(&source, b"not a real heif payload").expect("write fixture");

        let error =
            generate_image_thumbnail_bytes(&source).expect_err("junk payload cannot decode");
        let text = error.to_string();

        // The point: a decoder must have CLAIMED it. Reaching NO_DECODER_MARKER
        // would mean `.heif` fell out of the ffmpeg decoder's extension list.
        // Holds with or without ffmpeg installed: a missing binary yields
        // "ffmpeg: decoder unavailable", which also names ffmpeg and is also not
        // the no-decoder marker.
        assert!(
            !text.contains(crate::decode::NO_DECODER_MARKER),
            "heif must reach a decoder, got: {text}"
        );
        assert!(text.contains("ffmpeg"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn image_thumbnail_generation_flows_through_the_decoder_registry() {
        let dir = std::env::temp_dir().join(format!(
            "megle_thumb_registry_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("wide.png");
        let mut buffer = image::RgbaImage::new(900, 450);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgba([200, 100, 50, 255]);
        }
        image::DynamicImage::ImageRgba8(buffer)
            .save(&source)
            .expect("write fixture");

        let generated =
            generate_image_thumbnail_bytes(&source).expect("registry-backed generation");

        // Short side is exactly the profile; the true source dimensions survive
        // for media.width/height.
        assert_eq!(generated.height, GRID_320_SHORT_SIDE_PX);
        assert_eq!(generated.source_width, 900);
        assert_eq!(generated.source_height, 450);
        assert!(generated.byte_size > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unclaimed_format_reports_the_no_decoder_marker() {
        let dir = std::env::temp_dir().join(format!(
            "megle_thumb_nodecoder_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("camera.raw");
        std::fs::write(&source, b"raw sensor bytes").expect("write fixture");

        let error = generate_image_thumbnail_bytes(&source).expect_err("no decoder claims .raw");

        assert!(
            error.to_string().contains(crate::decode::NO_DECODER_MARKER),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn small_originals_never_get_a_downscaled_tier() {
        // At or below the threshold the original IS the preview.
        for source_max_px in [320, 1920, 4096] {
            assert_eq!(
                select_preview_profile(source_max_px, 2560),
                None,
                "source {source_max_px} should serve the original"
            );
        }
    }

    #[test]
    fn tier_selection_picks_the_smallest_profile_that_covers_the_viewport() {
        // 1366x768 laptop: 1366 * 1.2 = 1639 -> 2048 is enough.
        assert_eq!(
            select_preview_profile(16384, 1366),
            Some(PREVIEW_2048_PROFILE)
        );
        // 1920x1080: 1920 * 1.2 = 2304 -> needs 3072.
        assert_eq!(
            select_preview_profile(16384, 1920),
            Some(PREVIEW_3072_PROFILE)
        );
        // 4K window: clamps at the largest tier; the background original
        // upgrade closes the remaining gap.
        assert_eq!(
            select_preview_profile(16384, 3840),
            Some(PREVIEW_3072_PROFILE)
        );
    }

    #[test]
    fn tier_selection_keeps_downscaling_when_the_viewport_outruns_the_largest_tier() {
        // 5000px source on a 1366px viewport: preview_2048 covers it.
        assert_eq!(
            select_preview_profile(5000, 1366),
            Some(PREVIEW_2048_PROFILE)
        );
        // 4200px source on a 4K viewport: required is 4608, past every tier, so
        // it clamps to the largest. 3072 is still a real downscale of 4200, so
        // serving the tier is correct — the frontend's background original
        // upgrade closes the remaining sharpness gap.
        assert_eq!(
            select_preview_profile(4200, 3840),
            Some(PREVIEW_3072_PROFILE)
        );
    }

    #[test]
    fn the_no_upscale_guard_is_unreachable_while_the_threshold_exceeds_every_tier() {
        // Documents a real invariant rather than testing behaviour: every source
        // that survives the PREVIEW_TIER_MIN_SOURCE_MAX_PX early return is
        // strictly larger than the biggest tier, so `tier >= source` can never
        // hold. If someone adds a tier above the threshold, or lowers the
        // threshold, this fails and the guard's tests become worth writing.
        let largest_tier = PREVIEW_PROFILE_VALUES
            .iter()
            .filter_map(|profile| preview_profile_max_px(profile))
            .max()
            .expect("at least one tier");
        assert!(
            largest_tier <= PREVIEW_TIER_MIN_SOURCE_MAX_PX,
            "largest tier {largest_tier} exceeds the tier threshold {PREVIEW_TIER_MIN_SOURCE_MAX_PX}; \
             the no-upscale guard in select_preview_profile is now reachable and needs its own test"
        );
    }

    #[test]
    fn an_unknown_viewport_serves_the_original_unchanged() {
        // A client that sends no viewport gets today's exact behaviour.
        assert_eq!(select_preview_profile(16384, 0), None);
    }

    #[test]
    fn preview_profile_max_px_is_defined_for_every_declared_profile() {
        for profile in PREVIEW_PROFILE_VALUES {
            assert!(
                preview_profile_max_px(profile).is_some(),
                "no cap for {profile}"
            );
        }
        assert_eq!(preview_profile_max_px("grid_320"), None);
        assert_eq!(preview_profile_max_px("preview_9999"), None);
    }

    #[test]
    fn tier_dimensions_fit_the_box_and_never_upscale() {
        assert_eq!(preview_tier_dimensions(8000, 4000, 2048), (2048, 1024));
        assert_eq!(preview_tier_dimensions(4000, 8000, 2048), (1024, 2048));
        // Already inside the box: unchanged.
        assert_eq!(preview_tier_dimensions(1000, 500, 2048), (1000, 500));
        // Never collapse a dimension to zero on extreme ratios.
        assert_eq!(preview_tier_dimensions(20000, 3, 2048), (2048, 1));
    }

    #[test]
    fn generates_a_downscaled_webp_tier_from_a_large_original() {
        let dir = std::env::temp_dir().join(format!(
            "megle_preview_tier_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("huge.png");
        let mut buffer = image::RgbaImage::new(5000, 500);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgba([30, 60, 90, 255]);
        }
        image::DynamicImage::ImageRgba8(buffer)
            .save(&source)
            .expect("write fixture");

        let tier = generate_preview_webp(&source, 2048).expect("tier generation");

        assert_eq!(tier.width, 2048);
        assert_eq!(tier.height, 204);
        assert_eq!(tier.output_format, GENERATED_FORMAT);
        assert_eq!(tier.byte_size, tier.data.len() as i64);
        // RIFF....WEBP
        assert_eq!(&tier.data[0..4], b"RIFF");
        assert_eq!(&tier.data[8..12], b"WEBP");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cap_larger_than_the_source_transcodes_without_upscaling() {
        let dir = std::env::temp_dir().join(format!(
            "megle_preview_transcode_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("medium.png");
        let mut buffer = image::RgbaImage::new(640, 480);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgba([7, 7, 7, 255]);
        }
        image::DynamicImage::ImageRgba8(buffer)
            .save(&source)
            .expect("write fixture");

        let transcoded = generate_preview_webp(&source, PREVIEW_NATIVE_MAX_PX).expect("transcode");

        assert_eq!((transcoded.width, transcoded.height), (640, 480));
        assert_eq!(&transcoded.data[8..12], b"WEBP");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_generation_reports_chain_failures_verbatim() {
        let dir = std::env::temp_dir().join(format!(
            "megle_preview_fail_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("sensor.raw");
        std::fs::write(&source, b"not decodable").expect("write fixture");

        let error = generate_preview_webp(&source, 2048).expect_err("no decoder for .raw");

        assert!(
            error.to_string().contains(crate::decode::NO_DECODER_MARKER),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
