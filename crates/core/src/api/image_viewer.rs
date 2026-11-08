//! Host services for the `image-viewer.v1` Host-engine capability.
//!
//! The standalone image viewer opens files the user picks in the operating
//! system, which are usually NOT inside any indexed Library. These routes are
//! therefore path-scoped rather than `fileId`-scoped, and are deliberately
//! narrow:
//!
//! - every route is unavailable (`403 image_viewer_disabled`) unless an
//!   enabled, granted, trusted Host-engine plugin currently provides
//!   `image-viewer.v1` — disabling the plugin turns the surface off;
//! - only absolute, `..`-free paths to regular files whose extension the decode
//!   stack (built-in registry or a running decoder plugin) supports are read;
//! - nothing is written: no Library rows, no sidecars, no `preview_blobs`.
//!
//! Format support is exactly the project's: `decode/formats.rs` plus decoder
//! plugin declarations. Transcoding reuses `thumbnails::generate_preview_webp`
//! and the existing tier policy (`resolve_preview_plan`), so a picture renders
//! the same way in the viewer as in the Library preview. Generated WebP bytes
//! live in a small bounded in-memory LRU so prev/next and the filmstrip do not
//! redo a HEIC/RAW decode, and decode work runs on `spawn_blocking` behind its
//! own semaphores (never on a Library database connection).

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio_util::io::ReaderStream;
use tokio_util::sync::CancellationToken;

use super::routes::{preview_content_type, resolve_preview_plan, CoreError};
use super::AppState;
use crate::decode::formats;

type ViewerResult<T> = Result<T, CoreError>;

const MAX_PATH_BYTES: usize = 4096;
const MAX_FOLDER_ENTRIES: usize = 50_000;
const TIER_HEADER: &str = "x-mangoo-preview-tier";
const SOURCE_WIDTH_HEADER: &str = "x-mangoo-source-width";
const SOURCE_HEIGHT_HEADER: &str = "x-mangoo-source-height";
/// `true` when the served bytes are a multi-frame animation (GIF, APNG,
/// animated WebP / AVIF), so the client keeps every frame.
const ANIMATED_HEADER: &str = "x-mangoo-animated";
const DEFAULT_THUMBNAIL_PX: u32 = 192;
const MIN_THUMBNAIL_PX: u32 = 48;
const MAX_THUMBNAIL_PX: u32 = 512;
/// Upper bound requested from a decoder plugin; matches the Library preview's
/// decoder tier so plugin formats look identical in both surfaces.
const DECODER_PREVIEW_MAX_PX: u32 = 3072;
const MAX_VIEWPORT_LONG_PX: u32 = 16_384;
const IMAGE_CACHE_BYTES: usize = 384 * 1024 * 1024;
const THUMBNAIL_CACHE_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/image-viewer/formats", get(get_formats))
        .route("/api/image-viewer/folder", get(get_folder))
        .route("/api/image-viewer/image", get(get_image))
        .route("/api/image-viewer/thumbnail", get(get_thumbnail))
        .route("/api/image-viewer/metadata", get(get_metadata))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Capability gate and path policy
// ---------------------------------------------------------------------------

fn require_capability(state: &AppState) -> ViewerResult<()> {
    let enabled = state
        .library_manager
        .as_ref()
        .map(|manager| {
            manager
                .host_registry()
                .host_capability_enabled(crate::plugins::IMAGE_VIEWER_V1_CAPABILITY)
        })
        .transpose()?
        .unwrap_or(false);
    if enabled {
        Ok(())
    } else {
        Err(CoreError::coded(
            StatusCode::FORBIDDEN,
            "image_viewer_disabled",
            "The image viewer plugin is not enabled".into(),
        ))
    }
}

fn plugin_decoder_extensions(state: &AppState) -> Vec<String> {
    let Some(manager) = state.library_manager.as_ref() else {
        return Vec::new();
    };
    let mut extensions: Vec<String> = manager
        .decoder_manager()
        .snapshot()
        .capabilities
        .iter()
        .filter(|capability| {
            capability
                .declaration
                .purposes
                .contains(&crate::plugins::decoder::DecoderPurpose::Preview)
        })
        .flat_map(|capability| capability.declaration.extensions.iter())
        .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
        .filter(|extension| !formats::IMAGE_EXTENSIONS.contains(&extension.as_str()))
        .collect();
    extensions.sort();
    extensions.dedup();
    extensions
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    /// Decoded by Core's built-in registry (`decode/`).
    Builtin,
    /// Only a running decoder plugin declares this extension.
    Plugin,
}

fn source_kind(ext: &str, plugin_extensions: &[String]) -> Option<SourceKind> {
    if formats::IMAGE_EXTENSIONS.contains(&ext) {
        Some(SourceKind::Builtin)
    } else if plugin_extensions.iter().any(|value| value == ext) {
        Some(SourceKind::Plugin)
    } else {
        None
    }
}

fn invalid_path(message: &str) -> CoreError {
    CoreError::coded(
        StatusCode::BAD_REQUEST,
        "invalid_image_path",
        message.to_owned(),
    )
}

/// Absolute, normalized (no `.`/`..`), bounded and NUL-free. Canonicalization
/// is intentionally avoided: on Windows it rewrites to `\\?\` verbatim paths
/// the UI would then echo back, and symlinked folders are legitimate here.
fn parse_absolute_path(raw: &str) -> ViewerResult<PathBuf> {
    if raw.is_empty() || raw.len() > MAX_PATH_BYTES || raw.contains('\0') {
        return Err(invalid_path("path must be a bounded absolute path"));
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid_path("path must be an absolute normalized path"));
    }
    Ok(path)
}

struct ResolvedImage {
    path: PathBuf,
    ext: String,
    kind: SourceKind,
    size: u64,
    modified_ms: i64,
    created_ms: Option<i64>,
}

fn millis(time: std::io::Result<std::time::SystemTime>) -> Option<i64> {
    time.ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

async fn resolve_image(state: &AppState, raw: &str) -> ViewerResult<ResolvedImage> {
    let path = parse_absolute_path(raw)?;
    let ext = formats::lowercase_extension(&path);
    let plugin_extensions = plugin_decoder_extensions(state);
    let kind = source_kind(&ext, &plugin_extensions).ok_or_else(|| {
        CoreError::coded(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_image_format",
            format!("{ext} is not a supported image format"),
        )
    })?;
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|_| CoreError::not_found("image file is unavailable".into()))?;
    if !metadata.is_file() {
        return Err(CoreError::not_found("image path is not a file".into()));
    }
    Ok(ResolvedImage {
        path,
        ext,
        kind,
        size: metadata.len(),
        modified_ms: millis(metadata.modified()).unwrap_or(0),
        created_ms: millis(metadata.created()),
    })
}

// ---------------------------------------------------------------------------
// Bounded in-memory cache of generated WebP bytes
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    path: PathBuf,
    size: u64,
    modified_ms: i64,
    variant: String,
}

#[derive(Clone)]
struct CachedImage {
    data: Arc<Vec<u8>>,
    tier: &'static str,
    source_width: Option<u32>,
    source_height: Option<u32>,
}

struct ByteLru {
    budget: usize,
    used: usize,
    clock: u64,
    entries: HashMap<CacheKey, (CachedImage, u64)>,
}

impl ByteLru {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            used: 0,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    fn get(&mut self, key: &CacheKey) -> Option<CachedImage> {
        self.clock += 1;
        let clock = self.clock;
        self.entries.get_mut(key).map(|(value, used)| {
            *used = clock;
            value.clone()
        })
    }

    fn insert(&mut self, key: CacheKey, value: CachedImage) {
        let bytes = value.data.len();
        if bytes > self.budget / 4 {
            return;
        }
        self.clock += 1;
        if let Some((previous, _)) = self.entries.insert(key, (value, self.clock)) {
            self.used -= previous.data.len();
        }
        self.used += bytes;
        while self.used > self.budget {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some((removed, _)) = self.entries.remove(&oldest) {
                self.used -= removed.data.len();
            }
        }
    }
}

struct ViewerRuntime {
    images: Mutex<ByteLru>,
    thumbnails: Mutex<ByteLru>,
    /// Full-view decodes: the visible image and its prefetched neighbours.
    image_decodes: Arc<Semaphore>,
    /// Filmstrip thumbnails never starve the image the user is looking at.
    thumbnail_decodes: Arc<Semaphore>,
}

fn runtime() -> &'static ViewerRuntime {
    static RUNTIME: OnceLock<ViewerRuntime> = OnceLock::new();
    RUNTIME.get_or_init(|| ViewerRuntime {
        images: Mutex::new(ByteLru::new(IMAGE_CACHE_BYTES)),
        thumbnails: Mutex::new(ByteLru::new(THUMBNAIL_CACHE_BYTES)),
        image_decodes: Arc::new(Semaphore::new(2)),
        thumbnail_decodes: Arc::new(Semaphore::new(2)),
    })
}

fn image_response(content_type: &str, body: Body, cached: &CachedImage) -> Response {
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(content_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(TIER_HEADER, HeaderValue::from_static(cached.tier));
    if let Some(width) = cached.source_width {
        headers.insert(SOURCE_WIDTH_HEADER, HeaderValue::from(width));
    }
    if let Some(height) = cached.source_height {
        headers.insert(SOURCE_HEIGHT_HEADER, HeaderValue::from(height));
    }
    response
}

fn webp_response(cached: CachedImage) -> Response {
    let body = Body::from(cached.data.as_ref().clone());
    image_response(crate::thumbnails::GENERATED_FORMAT, body, &cached)
}

fn decode_failed(error: impl std::fmt::Display) -> CoreError {
    CoreError::coded(
        StatusCode::UNPROCESSABLE_ENTITY,
        "image_decode_failed",
        error.to_string(),
    )
}

/// Whether the file header declares an animation. Bounded read (64 KiB);
/// never decodes pixels. GIF counts image descriptors beyond the first.
fn sniff_animated(path: &Path, ext: &str) -> bool {
    use std::io::Read;
    if !formats::is_animated_capable_image(ext) && ext != ".png" {
        return false;
    }
    let mut head = Vec::new();
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    if file.take(64 * 1024).read_to_end(&mut head).is_err() {
        return false;
    }
    match ext {
        ".png" | ".apng" => png_has_actl(&head),
        ".webp" => {
            head.len() > 20
                && &head[0..4] == b"RIFF"
                && &head[8..12] == b"WEBP"
                && &head[12..16] == b"VP8X"
                && head[20] & 0x02 != 0
        }
        ".avif" => {
            let size = head
                .get(0..4)
                .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
                .unwrap_or(0);
            head.get(4..8) == Some(b"ftyp")
                && head
                    .get(8..size.min(head.len()))
                    .is_some_and(|brands| brands.chunks(4).any(|brand| brand == b"avis"))
        }
        ".gif" => head.starts_with(b"GIF"),
        _ => false,
    }
}

fn png_has_actl(head: &[u8]) -> bool {
    if head.get(1..4) != Some(b"PNG") {
        return false;
    }
    let mut offset = 8usize;
    while offset + 8 <= head.len() {
        let length = u32::from_be_bytes([
            head[offset],
            head[offset + 1],
            head[offset + 2],
            head[offset + 3],
        ]) as usize;
        let kind = &head[offset + 4..offset + 8];
        if kind == b"acTL" {
            return true;
        }
        if kind == b"IDAT" {
            return false;
        }
        offset = match offset.checked_add(12 + length) {
            Some(next) => next,
            None => return false,
        };
    }
    false
}

/// Display dimensions (EXIF orientation applied) without decoding pixels.
fn probe_dimensions(path: &Path, ext: &str) -> Option<(u32, u32)> {
    let (mut width, mut height) = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
        .or_else(|| psd_dimensions(path, ext))
        .or_else(|| {
            crate::decode::exif::read_exif_summary(path, ext)
                .and_then(|exif| Some((exif.pixel_width?, exif.pixel_height?)))
        })?;
    if crate::decode::exif::read_exif_summary(path, ext)
        .and_then(|exif| exif.orientation)
        .is_some_and(|orientation| orientation >= 5)
    {
        std::mem::swap(&mut width, &mut height);
    }
    (width > 0 && height > 0).then_some((width, height))
}

fn psd_dimensions(path: &Path, ext: &str) -> Option<(u32, u32)> {
    if ext != ".psd" {
        return None;
    }
    use std::io::Read;
    let mut header = [0u8; 26];
    std::fs::File::open(path)
        .ok()?
        .read_exact(&mut header)
        .ok()?;
    if &header[0..4] != b"8BPS" {
        return None;
    }
    let height = u32::from_be_bytes(header[14..18].try_into().ok()?);
    let width = u32::from_be_bytes(header[18..22].try_into().ok()?);
    Some((width, height))
}

/// Builtin-registry WebP transcode bounded to `max_px` on the long side,
/// served from / stored into `cache`.
async fn builtin_webp(
    image: &ResolvedImage,
    max_px: u32,
    tier: &'static str,
    variant: String,
    thumbnail: bool,
) -> ViewerResult<CachedImage> {
    let services = runtime();
    let key = CacheKey {
        path: image.path.clone(),
        size: image.size,
        modified_ms: image.modified_ms,
        variant,
    };
    let cache = if thumbnail {
        &services.thumbnails
    } else {
        &services.images
    };
    if let Some(hit) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return Ok(hit);
    }
    let gate = if thumbnail {
        services.thumbnail_decodes.clone()
    } else {
        services.image_decodes.clone()
    };
    let _permit = gate
        .acquire_owned()
        .await
        .map_err(|_| CoreError::coded(StatusCode::SERVICE_UNAVAILABLE, "busy", "busy".into()))?;
    // A concurrent request for the same key may have finished while waiting.
    if let Some(hit) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return Ok(hit);
    }
    let path = image.path.clone();
    let ext = image.ext.clone();
    let generated = tokio::task::spawn_blocking(move || {
        let dimensions = probe_dimensions(&path, &ext);
        crate::thumbnails::generate_preview_webp(&path, max_px).map(|tier| (tier, dimensions))
    })
    .await
    .map_err(|error| decode_failed(error))?
    .map_err(decode_failed)?;
    let (generated, dimensions) = generated;
    let value = CachedImage {
        data: Arc::new(generated.data),
        tier,
        source_width: dimensions.map(|(width, _)| width),
        source_height: dimensions.map(|(_, height)| height),
    };
    cache
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, value.clone());
    Ok(value)
}

async fn plugin_webp(
    state: &AppState,
    image: &ResolvedImage,
    max_px: u32,
    thumbnail: bool,
) -> ViewerResult<CachedImage> {
    let services = runtime();
    let key = CacheKey {
        path: image.path.clone(),
        size: image.size,
        modified_ms: image.modified_ms,
        variant: format!("decoder:{max_px}"),
    };
    let cache = if thumbnail {
        &services.thumbnails
    } else {
        &services.images
    };
    if let Some(hit) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return Ok(hit);
    }
    let manager = state
        .library_manager
        .as_ref()
        .ok_or_else(|| decode_failed("decoder plugins require Host mode"))?
        .decoder_manager();
    let open_path = image.path.clone();
    let file = tokio::task::spawn_blocking(move || std::fs::File::open(open_path))
        .await
        .map_err(decode_failed)?
        .map_err(|_| CoreError::not_found("image file is unavailable".into()))?;
    let raster = manager
        .decode_owned(
            file,
            &image.ext,
            crate::plugins::decoder::DecoderPurpose::Preview,
            max_px,
            CancellationToken::new(),
        )
        .await
        .map_err(|failure| decode_failed(failure.error))?;
    let (width, height) = (raster.width, raster.height);
    let encoded = tokio::task::spawn_blocking(move || raster.into_webp())
        .await
        .map_err(decode_failed)?
        .map_err(decode_failed)?;
    let value = CachedImage {
        data: Arc::new(encoded.data),
        tier: "decoder",
        source_width: Some(width),
        source_height: Some(height),
    };
    cache
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, value.clone());
    Ok(value)
}

async fn stream_original(image: &ResolvedImage) -> ViewerResult<Response> {
    let file = tokio::fs::File::open(&image.path)
        .await
        .map_err(|_| CoreError::not_found("image file is unavailable".into()))?;
    let path = image.path.clone();
    let ext = image.ext.clone();
    let dimensions = tokio::task::spawn_blocking(move || probe_dimensions(&path, &ext))
        .await
        .ok()
        .flatten();
    let cached = CachedImage {
        data: Arc::new(Vec::new()),
        tier: "original",
        source_width: dimensions.map(|(width, _)| width),
        source_height: dimensions.map(|(_, height)| height),
    };
    let mut response = image_response(
        preview_content_type(&image.path, Some("image")),
        Body::from_stream(ReaderStream::new(file)),
        &cached,
    );
    if let Ok(length) = HeaderValue::from_str(&image.size.to_string()) {
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, length);
    }
    Ok(response)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageViewerFormats {
    image_extensions: Vec<String>,
    browser_renderable_extensions: Vec<String>,
    animated_extensions: Vec<String>,
    plugin_extensions: Vec<String>,
}

async fn get_formats(State(state): State<AppState>) -> ViewerResult<Json<ImageViewerFormats>> {
    require_capability(&state)?;
    let builtin: Vec<String> = formats::IMAGE_EXTENSIONS
        .iter()
        .map(|value| (*value).to_owned())
        .collect();
    let plugin_extensions = plugin_decoder_extensions(&state);
    let mut image_extensions = builtin.clone();
    image_extensions.extend(plugin_extensions.iter().cloned());
    image_extensions.sort();
    Ok(Json(ImageViewerFormats {
        image_extensions,
        browser_renderable_extensions: builtin
            .iter()
            .filter(|ext| formats::is_browser_renderable_image(ext))
            .cloned()
            .collect(),
        animated_extensions: builtin
            .iter()
            .filter(|ext| formats::is_animated_capable_image(ext))
            .cloned()
            .collect(),
        plugin_extensions,
    }))
}

#[derive(Debug, Deserialize)]
struct PathQuery {
    path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageViewerFolderEntry {
    path: String,
    name: String,
    ext: String,
    size_bytes: u64,
    modified_at_ms: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageViewerFolder {
    directory: String,
    entries: Vec<ImageViewerFolderEntry>,
    truncated: bool,
}

/// Lists the supported images directly inside the folder of `path` (a file
/// or a folder). Non-recursive; ordering is the client's sort preference.
async fn get_folder(
    State(state): State<AppState>,
    Query(query): Query<PathQuery>,
) -> ViewerResult<Json<ImageViewerFolder>> {
    require_capability(&state)?;
    let requested = parse_absolute_path(&query.path)?;
    let plugin_extensions = plugin_decoder_extensions(&state);
    let listing = tokio::task::spawn_blocking(move || -> ViewerResult<ImageViewerFolder> {
        let metadata = std::fs::metadata(&requested)
            .map_err(|_| CoreError::not_found("folder is unavailable".into()))?;
        let directory = if metadata.is_dir() {
            requested
        } else {
            requested
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| invalid_path("file has no parent folder"))?
        };
        let read = std::fs::read_dir(&directory)
            .map_err(|_| CoreError::not_found("folder is unavailable".into()))?;
        let mut entries = Vec::new();
        let mut truncated = false;
        for item in read.flatten() {
            let path = item.path();
            let ext = formats::lowercase_extension(&path);
            if source_kind(&ext, &plugin_extensions).is_none() {
                continue;
            }
            let Ok(metadata) = item.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let (Some(path_text), Some(name)) =
                (path.to_str(), item.file_name().to_str().map(str::to_owned))
            else {
                continue;
            };
            if entries.len() >= MAX_FOLDER_ENTRIES {
                truncated = true;
                break;
            }
            entries.push(ImageViewerFolderEntry {
                path: path_text.to_owned(),
                name,
                ext,
                size_bytes: metadata.len(),
                modified_at_ms: millis(metadata.modified()).unwrap_or(0),
            });
        }
        Ok(ImageViewerFolder {
            directory: directory.to_string_lossy().into_owned(),
            entries,
            truncated,
        })
    })
    .await
    .map_err(|error| {
        CoreError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            "folder_failed",
            error.to_string(),
        )
    })??;
    Ok(Json(listing))
}

#[derive(Debug, Deserialize)]
struct ImageQuery {
    path: String,
    #[serde(rename = "viewportLongPx")]
    viewport_long_px: Option<u32>,
    #[serde(rename = "full")]
    full: Option<bool>,
}

/// The viewer's image source. Small browser-paintable images stream verbatim;
/// oversized ones get the Library preview tier for the viewport (and the
/// client upgrades to the original when zoomed past fit with `full=true`);
/// formats Chromium cannot paint are always transcoded to WebP.
async fn get_image(
    State(state): State<AppState>,
    Query(query): Query<ImageQuery>,
) -> ViewerResult<Response> {
    require_capability(&state)?;
    let image = resolve_image(&state, &query.path).await?;
    if image.kind == SourceKind::Plugin {
        return Ok(webp_response(
            plugin_webp(&state, &image, DECODER_PREVIEW_MAX_PX, false).await?,
        ));
    }
    let renderable = formats::is_browser_renderable_image(&image.ext);
    let full = query.full.unwrap_or(false);
    let sniff_path = image.path.clone();
    let sniff_ext = image.ext.clone();
    let animated = renderable
        && tokio::task::spawn_blocking(move || sniff_animated(&sniff_path, &sniff_ext))
            .await
            .unwrap_or(false);
    if animated {
        // A tier would flatten the animation to its first frame.
        let mut response = stream_original(&image).await?;
        response
            .headers_mut()
            .insert(ANIMATED_HEADER, HeaderValue::from_static("true"));
        return Ok(response);
    }
    if renderable && full {
        return stream_original(&image).await;
    }
    let viewport = query
        .viewport_long_px
        .unwrap_or(0)
        .min(MAX_VIEWPORT_LONG_PX);
    let probe_path = image.path.clone();
    let probe_ext = image.ext.clone();
    let source_max_px =
        tokio::task::spawn_blocking(move || probe_dimensions(&probe_path, &probe_ext))
            .await
            .ok()
            .flatten()
            .map(|(width, height)| width.max(height))
            .unwrap_or(0);
    // The viewer always wants a paintable picture, even if the client did not
    // report a viewport: give the native transcode a nominal one.
    let plan_viewport = if viewport == 0 && !renderable {
        1
    } else {
        viewport
    };
    let plan = resolve_preview_plan(Some("image"), &image.ext, source_max_px, plan_viewport);
    let Some(plan) = plan else {
        return stream_original(&image).await;
    };
    match builtin_webp(
        &image,
        plan.max_px,
        plan.profile,
        format!("{}:{}", plan.profile, plan.max_px),
        false,
    )
    .await
    {
        Ok(cached) => Ok(webp_response(cached)),
        // Degrade rather than fail, exactly like the Library preview: a tier
        // that cannot be produced must not blank a picture Chromium can paint.
        Err(_) if renderable => stream_original(&image).await,
        Err(error) => Err(error),
    }
}

#[derive(Debug, Deserialize)]
struct ThumbnailQuery {
    path: String,
    size: Option<u32>,
}

async fn get_thumbnail(
    State(state): State<AppState>,
    Query(query): Query<ThumbnailQuery>,
) -> ViewerResult<Response> {
    require_capability(&state)?;
    let image = resolve_image(&state, &query.path).await?;
    let size = query
        .size
        .unwrap_or(DEFAULT_THUMBNAIL_PX)
        .clamp(MIN_THUMBNAIL_PX, MAX_THUMBNAIL_PX);
    let cached = match image.kind {
        SourceKind::Plugin => plugin_webp(&state, &image, size, true).await?,
        SourceKind::Builtin => {
            builtin_webp(&image, size, "thumbnail", format!("thumb:{size}"), true).await?
        }
    };
    let mut response = webp_response(cached);
    // Thumbnails are keyed by path + size + mtime on the client: safe to keep.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=300"),
    );
    Ok(response)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageViewerMetadata {
    path: String,
    name: String,
    directory: String,
    ext: String,
    size_bytes: u64,
    modified_at_ms: i64,
    created_at_ms: Option<i64>,
    width: Option<u32>,
    height: Option<u32>,
    exif: Option<crate::decode::exif::ExifSummary>,
}

async fn get_metadata(
    State(state): State<AppState>,
    Query(query): Query<PathQuery>,
) -> ViewerResult<Json<ImageViewerMetadata>> {
    require_capability(&state)?;
    let image = resolve_image(&state, &query.path).await?;
    let path = image.path.clone();
    let ext = image.ext.clone();
    let (dimensions, exif) = tokio::task::spawn_blocking(move || {
        (
            probe_dimensions(&path, &ext),
            crate::decode::exif::read_exif_summary(&path, &ext),
        )
    })
    .await
    .map_err(decode_failed)?;
    Ok(Json(ImageViewerMetadata {
        name: image
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        directory: image
            .path
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: image.path.to_string_lossy().into_owned(),
        ext: image.ext,
        size_bytes: image.size,
        modified_at_ms: image.modified_ms,
        created_at_ms: image.created_ms,
        width: dimensions.map(|(width, _)| width),
        height: dimensions.map(|(_, height)| height),
        exif,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_animation_sniff_requires_actl_before_idat() {
        let chunk = |kind: &[u8], data: &[u8]| {
            let mut out = (data.len() as u32).to_be_bytes().to_vec();
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&[0, 0, 0, 0]);
            out
        };
        const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut still = SIGNATURE.to_vec();
        still.extend(chunk(b"IHDR", &[0; 13]));
        still.extend(chunk(b"IDAT", &[0; 4]));
        assert!(!png_has_actl(&still));
        let mut animated = SIGNATURE.to_vec();
        animated.extend(chunk(b"IHDR", &[0; 13]));
        animated.extend(chunk(b"acTL", &[0; 8]));
        animated.extend(chunk(b"IDAT", &[0; 4]));
        assert!(png_has_actl(&animated));
        assert!(!png_has_actl(b"GIF89a"));
        // A corrupt length never panics.
        let mut corrupt = SIGNATURE.to_vec();
        corrupt.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, b'I', b'H', b'D', b'R']);
        assert!(!png_has_actl(&corrupt));
    }

    #[test]
    fn path_policy_rejects_relative_traversal_and_nul() {
        assert!(parse_absolute_path("").is_err());
        assert!(parse_absolute_path("relative/image.png").is_err());
        assert!(parse_absolute_path("bad\0.png").is_err());
        #[cfg(windows)]
        {
            assert!(parse_absolute_path(r"C:\photos\..\secret.png").is_err());
            assert!(parse_absolute_path(r"C:\photos\a.png").is_ok());
            assert!(parse_absolute_path(r"\\nas\share\a.png").is_ok());
        }
        #[cfg(unix)]
        {
            assert!(parse_absolute_path("/photos/../secret.png").is_err());
            assert!(parse_absolute_path("/photos/a.png").is_ok());
        }
    }

    #[test]
    fn source_kind_follows_project_formats_and_plugin_declarations() {
        let plugin = vec![".ppm".to_owned()];
        assert!(matches!(
            source_kind(".heic", &plugin),
            Some(SourceKind::Builtin)
        ));
        assert!(matches!(
            source_kind(".ppm", &plugin),
            Some(SourceKind::Plugin)
        ));
        assert!(source_kind(".ppm", &[]).is_none());
        assert!(source_kind(".mp4", &plugin).is_none());
        assert!(source_kind(".exe", &plugin).is_none());
    }

    #[test]
    fn byte_lru_evicts_least_recently_used_within_budget() {
        let entry = |bytes: usize| CachedImage {
            data: Arc::new(vec![0; bytes]),
            tier: "original",
            source_width: None,
            source_height: None,
        };
        let key = |name: &str| CacheKey {
            path: PathBuf::from(name),
            size: 1,
            modified_ms: 1,
            variant: String::new(),
        };
        let mut lru = ByteLru::new(100);
        lru.insert(key("a"), entry(20));
        lru.insert(key("b"), entry(20));
        assert!(lru.get(&key("a")).is_some());
        for name in ["c", "d", "e", "f"] {
            lru.insert(key(name), entry(20));
        }
        assert!(lru.used <= 100);
        assert!(lru.get(&key("a")).is_some(), "recently used entry survives");
        assert!(lru.get(&key("b")).is_none(), "oldest entry is evicted");
        // An entry larger than a quarter of the budget is never cached.
        lru.insert(key("huge"), entry(60));
        assert!(lru.get(&key("huge")).is_none());
    }
}
