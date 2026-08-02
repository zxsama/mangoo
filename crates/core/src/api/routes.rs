use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use axum_extra::extract::Query as ExtraQuery;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::Path as FilePath;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::api::AppState;
use crate::db::notification_outbox::NotificationOutboxEvent;
use crate::db::{
    Database, FolderRecord, MarkRootForPurge, MediaPageQuery, MediaRecord, NewRoot,
    NotificationOrigin, PluginRecord, PreviewBlobRecord, RootRecord, SearchQuery, TagError,
    TagRecord, TaskRecord, ThumbBlobRecord, ThumbnailCacheClearResult, ThumbnailCacheEnqueueResult,
    ThumbnailCacheRefreshMode, ThumbnailCacheScope, ThumbnailCacheSnapshot, ThumbnailCacheStats,
    ThumbnailFailureRecord, ThumbnailRecord, ThumbnailScopeLeaverPolicy, ThumbnailTaskPriority,
    UserMetadataPatch, UserMetadataRecord, THUMBNAIL_BULK_PRIORITY,
};
use crate::fsops::{
    self, DeleteRequest, FileOperationRecord, FsOpsError, FsOpsErrorCode, MoveRequest,
    RenameRequest,
};
use crate::plugins;
use crate::scan::ScanSummary;
use crate::thumbnails::{
    generate_preview_webp, is_pending_status, normalize_profile, preview_profile_max_px,
    select_preview_profile, PREVIEW_NATIVE_MAX_PX, PREVIEW_NATIVE_PROFILE,
};

const THUMBNAIL_TASK_WAKE_DEDUPE_WINDOW: Duration = Duration::from_millis(750);
const INVALID_NOTIFICATION_OUTBOX_QUERY: &str = "invalid notification outbox query";
const INVALID_NOTIFICATION_OUTBOX_ACKNOWLEDGEMENT: &str =
    "invalid notification outbox acknowledgement";
const MAX_SAFE_NOTIFICATION_SEQUENCE: i64 = 9_007_199_254_740_991;
static RECENT_THUMBNAIL_TASK_WAKEUPS: LazyLock<Mutex<HashMap<(usize, i64, i64), Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[allow(dead_code)]
pub const MEDIA_SORT_VALUES: &[&str] = &["mtime_desc", "mtime_asc", "name_asc", "name_desc"];
#[allow(dead_code)]
pub const SEARCH_SORT_VALUES: &[&str] = &[
    "mtime_desc",
    "mtime_asc",
    "name_asc",
    "name_desc",
    "rating_desc",
    "rating_asc",
];
#[allow(dead_code)]
pub const MEDIA_KIND_VALUES: &[&str] = &["image", "video", "other"];

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteGroup {
    Health,
    Roots,
    Folders,
    Media,
    Thumbnails,
    Preview,
    Tasks,
    FileOps,
    Plugins,
}

#[allow(dead_code)]
pub const PHASE1_ROUTE_GROUPS: &[RouteGroup] = &[
    RouteGroup::Health,
    RouteGroup::Roots,
    RouteGroup::Folders,
    RouteGroup::Media,
    RouteGroup::Thumbnails,
    RouteGroup::Preview,
    RouteGroup::Tasks,
    RouteGroup::FileOps,
    RouteGroup::Plugins,
];

#[allow(dead_code)]
pub const PHASE1_API_PATHS: &[&str] = &[
    "/api/health",
    "/api/roots",
    "/api/roots/{rootId}",
    "/api/folders/{folderId}/children",
    "/api/folders/{folderId}/ancestors",
    "/api/media",
    "/api/media/{fileId}",
    "/api/media/{fileId}/position",
    "/api/media/{fileId}/thumbnail",
    "/api/media/{fileId}/thumbnail/blob",
    "/api/thumbnails/cache/stats",
    "/api/thumbnails/cache/failures",
    "/api/thumbnails/cache/clear",
    "/api/thumbnails/cache/cancel-bulk",
    "/api/media/{fileId}/preview",
    "/api/media/{fileId}/metadata",
    "/api/media/{fileId}/tags",
    "/api/media/{fileId}/tags/{tagId}",
    "/api/tags",
    "/api/tags/{tagId}",
    "/api/search",
    "/api/tasks",
    "/api/tasks/scan",
    "/api/tasks/interactive-folder-scan",
    "/api/tasks/thumbnail-cache",
    "/api/tasks/thumbnail-priority-scope",
    "/api/tasks/{taskId}/cancel",
    "/api/tasks/{taskId}/retry",
    "/api/file-ops/rename",
    "/api/file-ops/move",
    "/api/file-ops/delete",
    "/api/file-ops",
    "/api/notifications/outbox",
    "/api/notifications/outbox/ack",
    "/api/plugins",
    "/api/plugins/discover",
    "/api/plugins/{pluginId}",
    "/api/plugins/{pluginId}/enable",
    "/api/plugins/{pluginId}/disable",
];

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListResponse<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_count: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcceptedResponse {
    accepted: bool,
    task_id: Option<i64>,
    root_id: Option<i64>,
    scan: Option<ScanSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginListResponse {
    items: Vec<PluginRecord>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginDiscoveryError {
    manifest_path: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginDiscoveryResponse {
    discovered: u64,
    errors: Vec<PluginDiscoveryError>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeletePluginResponse {
    deleted: bool,
}

#[derive(Debug, Deserialize)]
struct AddRootRequest {
    path: String,
    #[serde(rename = "displayName", alias = "display_name")]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScanTaskRequest {
    #[serde(rename = "rootId")]
    root_id: i64,
}

#[derive(Debug, Deserialize)]
struct InteractiveFolderScanTaskRequest {
    #[serde(rename = "folderId")]
    folder_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailPriorityScopeSyncRequest {
    root_id: i64,
    #[serde(default)]
    selected_file_ids: Vec<i64>,
    #[serde(default)]
    visible_file_ids: Vec<i64>,
    #[serde(default)]
    ahead_file_ids: Vec<i64>,
    /// Files to keep warm (never cancel) even when out of scope — the client's
    /// recency grace window so scrolling back does not lose/regenerate a tile.
    #[serde(default)]
    retained_file_ids: Vec<i64>,
    /// Current generation mode. Absent ⇒ `allPersist` (never cancels), which
    /// preserves the pre-change behavior for any caller that omits it.
    #[serde(default)]
    generation_mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListFolderChildrenQuery {
    limit: Option<i64>,
    cursor: Option<String>,
    #[serde(rename = "includeDescendants")]
    include_descendants: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ListMediaQuery {
    #[serde(rename = "rootId")]
    root_id: Option<i64>,
    #[serde(rename = "folderId")]
    folder_id: Option<i64>,
    #[serde(rename = "includeDescendants")]
    include_descendants: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
    cursor: Option<String>,
    sort: Option<String>,
    kind: Option<String>,
}

#[derive(Debug, Serialize)]
struct MediaPositionResponse {
    index: i64,
}

#[derive(Debug, Deserialize)]
struct ThumbnailQuery {
    priority: Option<String>,
    target: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PreviewQuery {
    /// Cache-busting fingerprint; opaque to the server.
    #[allow(dead_code)]
    v: Option<String>,
    /// Longest side of the preview viewport, in CSS pixels. Omitted by clients
    /// that want the original bytes verbatim.
    #[serde(rename = "viewportLongPx")]
    viewport_long_px: Option<u32>,
}

/// What `get_preview` decided to serve instead of the raw original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreviewPlan {
    profile: &'static str,
    max_px: u32,
}

/// Pure preview-source resolution. `None` means "stream the original file
/// bytes", which is the zero-cost majority path and the exact behaviour that
/// shipped before this pipeline existed.
fn resolve_preview_plan(
    media_kind: Option<&str>,
    ext: &str,
    source_max_px: u32,
    viewport_long_px: u32,
) -> Option<PreviewPlan> {
    // Video always streams. Detecting "Chromium cannot play this HEVC/WMV" needs
    // codec probing and a transcode pipeline, both explicitly later work; this
    // early return is that plug point.
    if media_kind != Some("image") {
        return None;
    }
    if let Some(profile) = select_preview_profile(source_max_px, viewport_long_px) {
        match preview_profile_max_px(profile) {
            Some(max_px) => return Some(PreviewPlan { profile, max_px }),
            // Unreachable: `select_preview_profile` only ever returns a member of
            // `PREVIEW_PROFILE_VALUES`, and `preview_profile_max_px` maps every
            // one of them. Written as an explicit arm rather than `?` because `?`
            // returns `None` from the WHOLE function and thereby skips the
            // browser-renderable check below — i.e. it would silently stream raw
            // bytes for a format Chromium cannot paint. Falling through keeps
            // that check reachable, and the debug line makes the miss findable.
            None => crate::debug_log!("preview profile has no max_px mapping: {profile}"),
        }
    }
    // The transcode branch requires a REPORTED viewport. A client that sends no
    // `viewportLongPx` is asking for the pre-pipeline behaviour — the original
    // bytes, verbatim — and that request must keep succeeding. Transcoding here
    // instead would route every `.raw` (no decoder claims it at all) and every
    // `.heic`/`.heif`/`.psd` (ffmpeg-only, and ffmpeg may not be on PATH) into a
    // decode that fails, turning a 200 into a 500 for a URL that used to work.
    if viewport_long_px > 0 && !crate::decode::formats::is_browser_renderable_image(ext) {
        // Small enough that no tier applies, but the browser still cannot paint
        // it — transcode at (bounded) native resolution.
        return Some(PreviewPlan {
            profile: PREVIEW_NATIVE_PROFILE,
            max_px: PREVIEW_NATIVE_MAX_PX,
        });
    }
    None
}

#[derive(Debug, Deserialize)]
struct ThumbnailCacheScopeQuery {
    #[serde(rename = "rootId")]
    root_id: Option<i64>,
    #[serde(rename = "folderId")]
    folder_id: Option<i64>,
    #[serde(rename = "includeDescendants")]
    include_descendants: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailCacheTaskRequestBody {
    root_id: Option<i64>,
    folder_id: Option<i64>,
    include_descendants: Option<bool>,
    #[serde(rename = "fileIds")]
    file_ids: Option<Vec<i64>>,
    refresh_mode: String,
    limit: Option<usize>,
    /// Keyset cursor for the whole-library "generate all" pass: scan one window
    /// of files past this id and return `nextCursor`. Omit for the
    /// folder-scoped / explicit-id enqueues.
    #[serde(rename = "afterFileId")]
    after_file_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailCacheStatsResponse {
    cached_count: i64,
    missing_count: i64,
    stale_count: i64,
    failed_count: i64,
    pending_candidate_count: i64,
    total_blob_bytes: i64,
    active_bulk_task_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailCacheSnapshotResponse {
    #[serde(flatten)]
    stats: ThumbnailCacheStatsResponse,
    failure_details: Vec<ThumbnailFailureRecord>,
    failure_details_truncated: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailFailureQuery {
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailFailureListResponse {
    items: Vec<ThumbnailFailureRecord>,
    truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailCacheEnqueueResponse {
    accepted_count: usize,
    cached_count: i64,
    missing_count: i64,
    stale_count: i64,
    failed_count: i64,
    pending_candidate_count: i64,
    total_blob_bytes: i64,
    active_bulk_task_count: i64,
    /// Keyset cursor to resume the whole-library "generate all" pass from, or
    /// `null` once the scan reaches the end of the scope. Always `null` for the
    /// folder-scoped / explicit-id enqueues.
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailCacheClearResponse {
    cleared: bool,
    deleted_blob_count: usize,
    reset_thumbnail_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailCacheCancelBulkResponse {
    cancelled_count: usize,
}

#[derive(Debug, Deserialize)]
struct CreateTagRequestBody {
    name: String,
    #[serde(default)]
    color: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AddFileTagRequestBody {
    #[serde(rename = "tagId")]
    tag_id: i64,
}

#[derive(Debug, Deserialize)]
struct SetFileTagsRequestBody {
    #[serde(rename = "tagIds")]
    tag_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
struct SearchMediaQuery {
    q: Option<String>,
    #[serde(rename = "rootId")]
    root_id: Option<i64>,
    #[serde(rename = "folderId")]
    folder_id: Option<i64>,
    #[serde(rename = "includeDescendants")]
    include_descendants: Option<bool>,
    kind: Option<String>,
    #[serde(rename = "minRating")]
    min_rating: Option<i64>,
    favorite: Option<bool>,
    #[serde(default, rename = "tagId")]
    tag_id: Vec<i64>,
    sort: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    cursor: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteTagResponse {
    deleted: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileTagsResponse {
    file_id: i64,
    tag_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameRequestBody {
    #[serde(default)]
    file_id: Option<i64>,
    #[serde(default)]
    folder_id: Option<i64>,
    new_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveRequestBody {
    #[serde(default)]
    file_ids: Option<Vec<i64>>,
    #[serde(default)]
    folder_ids: Option<Vec<i64>>,
    target_folder_id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteRequestBody {
    #[serde(default)]
    file_ids: Option<Vec<i64>>,
    #[serde(default)]
    folder_ids: Option<Vec<i64>>,
    #[serde(default)]
    permanent: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileOperationsResponse {
    operations: Vec<FileOperationRecord>,
}

#[derive(Debug, Deserialize)]
struct ListFileOperationsQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListNotificationOutboxQuery {
    after_sequence: Option<i64>,
    limit: Option<i64>,
    wait_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationOutboxResponse {
    items: Vec<NotificationOutboxEvent>,
    max_sequence: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotificationOutboxAckRequest {
    max_sequence: i64,
}

#[derive(Debug, Serialize)]
struct NotificationOutboxAckResponse {
    acknowledged: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct ErrorResponse {
    error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailAsset {
    width: i64,
    height: i64,
    byte_size: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThumbnailResponse {
    file_id: i64,
    target: String,
    state: String,
    short_side_px: i64,
    output_format: String,
    width: Option<i64>,
    height: Option<i64>,
    byte_size: Option<i64>,
    served_by: Option<String>,
    asset: Option<ThumbnailAsset>,
    error: Option<String>,
    updated_at: Option<i64>,
}

struct CoreError {
    status: StatusCode,
    message: String,
    code: Option<String>,
}

impl From<anyhow::Error> for CoreError {
    fn from(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
            code: None,
        }
    }
}

impl IntoResponse for CoreError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
                code: self.code,
            }),
        )
            .into_response()
    }
}

type ApiResult<T> = Result<T, CoreError>;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(get_health))
        .route("/api/roots", get(list_roots).post(add_root))
        .route("/api/roots/:root_id", axum::routing::delete(remove_root))
        .route(
            "/api/folders/:folder_id/children",
            get(list_folder_children),
        )
        .route(
            "/api/folders/:folder_id/ancestors",
            get(list_folder_ancestors),
        )
        .route("/api/media", get(list_media))
        .route("/api/media/:file_id", get(get_media))
        .route("/api/media/:file_id/position", get(get_media_position))
        .route("/api/media/:file_id/thumbnail", get(get_thumbnail))
        .route(
            "/api/media/:file_id/thumbnail/blob",
            get(get_thumbnail_blob),
        )
        .route(
            "/api/thumbnails/cache/stats",
            get(get_thumbnail_cache_stats),
        )
        .route(
            "/api/thumbnails/cache/failures",
            get(list_thumbnail_cache_failures),
        )
        .route("/api/thumbnails/cache/clear", post(clear_thumbnail_cache))
        .route(
            "/api/thumbnails/cache/cancel-bulk",
            post(cancel_bulk_thumbnail_cache),
        )
        .route("/api/media/:file_id/preview", get(get_preview))
        .route(
            "/api/media/:file_id/metadata",
            get(get_user_metadata).put(update_user_metadata),
        )
        .route(
            "/api/media/:file_id/tags",
            put(set_file_tags).post(add_file_tag),
        )
        .route(
            "/api/media/:file_id/tags/:tag_id",
            axum::routing::delete(remove_file_tag),
        )
        .route("/api/tags", get(list_tags).post(create_tag))
        .route("/api/tags/:tag_id", axum::routing::delete(delete_tag))
        .route("/api/search", get(search_media))
        .route("/api/tasks", get(list_tasks))
        .route("/api/tasks/scan", post(enqueue_scan))
        .route(
            "/api/tasks/interactive-folder-scan",
            post(enqueue_interactive_folder_scan),
        )
        .route("/api/tasks/thumbnail-cache", post(enqueue_thumbnail_cache))
        .route(
            "/api/tasks/thumbnail-priority-scope",
            post(sync_thumbnail_priority_scope),
        )
        .route("/api/tasks/:task_id/cancel", post(cancel_task))
        .route("/api/tasks/:task_id/retry", post(retry_task))
        .route("/api/file-ops/rename", post(rename_file))
        .route("/api/file-ops/move", post(move_files))
        .route("/api/file-ops/delete", post(delete_files))
        .route("/api/file-ops", get(list_file_operations))
        .route("/api/notifications/outbox", get(list_notification_outbox))
        .route(
            "/api/notifications/outbox/ack",
            post(acknowledge_notification_outbox),
        )
        .route("/api/plugins", get(list_plugins))
        .route("/api/plugins/discover", post(discover_plugins))
        .route(
            "/api/plugins/:plugin_id",
            get(get_plugin).delete(delete_plugin),
        )
        .route("/api/plugins/:plugin_id/enable", post(enable_plugin))
        .route("/api/plugins/:plugin_id/disable", post(disable_plugin))
        .with_state(state)
}

thread_local! {
    /// Per-blocking-thread cached read connection. Read handlers run on
    /// Tokio's blocking pool, whose threads are reused across requests, so
    /// caching one connection per thread amortizes the SQLite open + PRAGMA +
    /// UDF-registration cost that was previously paid on *every* tile/page
    /// fetch. WAL autocommit reads always observe the latest committed
    /// snapshot, so a reused connection never serves stale data.
    static READ_DATABASE: RefCell<Option<Database>> = const { RefCell::new(None) };
}

fn read_with_cached_connection<T>(
    state: &AppState,
    operation: impl FnOnce(&Database) -> ApiResult<T>,
) -> ApiResult<T> {
    // Callers must only reach this with a file-backed read database. The
    // in-memory/test fallback is handled in `run_read_database` (it has no
    // separate read connection, so it routes through the single writer).
    READ_DATABASE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = state.open_read_database()?;
        }
        match slot.as_ref() {
            Some(database) => operation(database),
            None => Err(CoreError::from(anyhow::anyhow!(
                "read database connection unavailable"
            ))),
        }
    })
}

async fn run_read_database<T, F>(state: AppState, operation: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Database) -> ApiResult<T> + Send + 'static,
{
    if !state.has_read_database() {
        // In-memory / test database: no separate read connection exists, so run
        // the read on the single writer connection (the only connection there is).
        let inner = state
            .writer
            .write(move |database: &mut Database| Ok(operation(database)))
            .await;
        return match inner {
            Ok(api_result) => api_result,
            Err(error) => Err(CoreError::from(error)),
        };
    }
    tokio::task::spawn_blocking(move || read_with_cached_connection(&state, operation))
        .await
        .map_err(|error| CoreError::from(anyhow::anyhow!("database task failed: {error}")))?
}

async fn run_shared_database<T, F>(state: AppState, operation: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Database) -> ApiResult<T> + Send + 'static,
{
    // Every write goes through the single writer actor: exactly one connection
    // ever writes, so the WAL writer lock is never contended (no "database is
    // locked") and the actor checkpoints between jobs to keep the WAL bounded.
    // The closure returns `ApiResult<T>`; we carry it through the writer's
    // `anyhow::Result` as an inner `Ok(ApiResult<T>)` so a `CoreError` (with its
    // HTTP status) survives the boundary intact.
    let inner = state
        .writer
        .write(move |database: &mut Database| Ok(operation(database)))
        .await;
    match inner {
        Ok(api_result) => api_result,
        Err(error) => Err(CoreError::from(error)),
    }
}

/// Historical alias kept so route bodies read the same; identical to
/// `run_shared_database` now that all writes flow through the single writer.
async fn run_dedicated_or_shared_database<T, F>(state: AppState, operation: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Database) -> ApiResult<T> + Send + 'static,
{
    run_shared_database(state, operation).await
}

async fn get_health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "megle-core",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn list_roots(State(state): State<AppState>) -> ApiResult<Json<ListResponse<RootRecord>>> {
    let reach = state.root_reachability.clone();
    let mut items = run_read_database(state, |database| Ok(database.list_roots()?)).await?;
    if let Ok(map) = reach.lock() {
        for item in items.iter_mut() {
            item.reachable = *map.get(&item.id).unwrap_or(&true);
        }
    }
    Ok(Json(ListResponse {
        items,
        next_cursor: None,
        total_count: None,
    }))
}

async fn add_root(
    State(state): State<AppState>,
    Json(payload): Json<AddRootRequest>,
) -> ApiResult<(StatusCode, Json<AcceptedResponse>)> {
    let canonical_path = fs::canonicalize(&payload.path).map_err(|error| {
        CoreError::bad_request(format!(
            "root path does not exist or cannot be opened: {error}"
        ))
    })?;
    if !canonical_path.is_dir() {
        return Err(CoreError::bad_request(format!(
            "root path is not a directory: {}",
            canonical_path.display()
        )));
    }
    let canonical_path = canonical_path.to_string_lossy().into_owned();
    let display_name = payload
        .display_name
        .unwrap_or_else(|| fallback_display_name(&canonical_path));
    let (root_id, task_id, attempt_generation) =
        run_shared_database(state.clone(), move |database| {
            if database
                .root_pending_delete_by_path(&canonical_path)
                .map_err(CoreError::from)?
            {
                return Err(CoreError::conflict(format!(
                    "root path is being removed; try again shortly: {canonical_path}"
                )));
            }
            let root_id = database.add_root(NewRoot {
                path: canonical_path,
                display_name,
            })?;
            let task_id =
                database.create_root_scan_task_with_origin(root_id, NotificationOrigin::User)?;
            let attempt_generation = database.current_task_attempt_generation(task_id)?;
            Ok((root_id, task_id, attempt_generation))
        })
        .await?;
    enqueue_task(&state, task_id, attempt_generation).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(AcceptedResponse {
            accepted: true,
            task_id: Some(task_id),
            root_id: Some(root_id),
            scan: None,
        }),
    ))
}

async fn remove_root(
    State(state): State<AppState>,
    Path(root_id): Path<i64>,
) -> ApiResult<(StatusCode, Json<AcceptedResponse>)> {
    let outcome = run_shared_database(state.clone(), move |database| {
        database
            .mark_root_for_purge_with_origin(root_id, NotificationOrigin::User)
            .map_err(CoreError::from)
    })
    .await?;
    match outcome {
        MarkRootForPurge::NotFound => {
            Err(CoreError::not_found(format!("root not found: {root_id}")))
        }
        MarkRootForPurge::AlreadyPending => Ok((StatusCode::ACCEPTED, Json(accepted()))),
        MarkRootForPurge::Marked {
            task_id,
            attempt_generation,
        } => {
            enqueue_task(&state, task_id, attempt_generation).await?;
            Ok((StatusCode::ACCEPTED, Json(accepted())))
        }
    }
}

async fn list_folder_children(
    State(state): State<AppState>,
    Path(folder_id): Path<i64>,
    Query(query): Query<ListFolderChildrenQuery>,
) -> ApiResult<Json<ListResponse<FolderRecord>>> {
    let limit = query.limit.unwrap_or(200);
    let cursor = query.cursor;
    let include_descendants = query.include_descendants.unwrap_or(false);
    let page = run_read_database(state, move |database| {
        if !database.folder_exists(folder_id)? {
            return Err(CoreError::not_found(format!(
                "folder not found: {folder_id}"
            )));
        }
        let page = database
            .list_folder_children_page(folder_id, limit, cursor, include_descendants)
            .map_err(map_cursor_error)?;
        Ok(page)
    })
    .await?;
    Ok(Json(ListResponse {
        items: page.items,
        next_cursor: page.next_cursor,
        total_count: page.total_count,
    }))
}

async fn list_folder_ancestors(
    State(state): State<AppState>,
    Path(folder_id): Path<i64>,
) -> ApiResult<Json<ListResponse<FolderRecord>>> {
    let ancestors = run_read_database(state, move |database| {
        Ok(database.list_folder_ancestors(folder_id)?)
    })
    .await?
    .ok_or_else(|| CoreError::not_found(format!("folder not found: {folder_id}")))?;
    Ok(Json(ListResponse {
        items: ancestors,
        next_cursor: None,
        total_count: None,
    }))
}

async fn list_media(
    State(state): State<AppState>,
    Query(query): Query<ListMediaQuery>,
) -> ApiResult<Json<ListResponse<MediaRecord>>> {
    let sort = parse_media_sort(query.sort.as_deref())?.to_string();
    let kind = parse_media_kind(query.kind.as_deref())?.map(str::to_string);
    let media_query = MediaPageQuery {
        root_id: query.root_id,
        folder_id: query.folder_id,
        include_descendants: query.include_descendants.unwrap_or(false),
        limit: query.limit.unwrap_or(200),
        offset: query.offset,
        cursor: query.cursor,
        sort,
        kind,
    };
    let page = run_read_database(state, move |database| {
        database
            .list_media_page(media_query)
            .map_err(map_cursor_error)
    })
    .await?;
    Ok(Json(ListResponse {
        items: page.items,
        next_cursor: page.next_cursor,
        total_count: page.total_count,
    }))
}

async fn get_media(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
) -> ApiResult<Json<MediaRecord>> {
    let item = run_read_database(state, move |database| {
        database
            .get_media(file_id)?
            .ok_or_else(|| CoreError::not_found(format!("media item not found: {file_id}")))
    })
    .await?;
    Ok(Json(item))
}

async fn get_media_position(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Query(query): Query<ListMediaQuery>,
) -> ApiResult<Json<MediaPositionResponse>> {
    let sort = parse_media_sort(query.sort.as_deref())?.to_string();
    let media_query = MediaPageQuery {
        root_id: query.root_id,
        folder_id: query.folder_id,
        include_descendants: query.include_descendants.unwrap_or(false),
        limit: 1,
        offset: None,
        cursor: None,
        sort,
        kind: None,
    };
    let index = run_read_database(state, move |database| {
        database
            .media_position(file_id, &media_query)
            .map_err(CoreError::from)
    })
    .await?
    .ok_or_else(|| CoreError::not_found(format!("media item not found in scope: {file_id}")))?;
    Ok(Json(MediaPositionResponse { index }))
}

async fn get_thumbnail(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Query(query): Query<ThumbnailQuery>,
) -> ApiResult<(StatusCode, Json<ThumbnailResponse>)> {
    let target = normalize_profile(query.target.as_deref())
        .ok_or_else(|| CoreError::bad_request("unsupported thumbnail target".to_string()))?;
    let priority = ThumbnailTaskPriority::from_query_value(query.priority.as_deref())
        .ok_or_else(|| CoreError::bad_request("unsupported thumbnail priority".to_string()))?;
    let requested_task_priority = priority.task_priority();

    // Fast path: read the current state on the dedicated read connection. If the
    // thumbnail is already ready, return it without any write. This keeps the
    // common (cached) case a pure read that never touches the writer.
    let read_snapshot = run_read_database(state.clone(), move |database| {
        let thumbnail = database
            .get_thumbnail(file_id, target)?
            .ok_or_else(|| CoreError::not_found(format!("media item not found: {file_id}")))?;
        let blob = if thumbnail.state.as_str() == "ready" {
            database.get_thumb_blob(file_id, target)?
        } else {
            None
        };
        Ok((thumbnail, blob))
    })
    .await?;
    let (thumbnail, blob) = read_snapshot;
    if !is_pending_status(&thumbnail.state) {
        return Ok((StatusCode::OK, Json(thumbnail_response(thumbnail, blob))));
    }

    // Pending: request/promote generation through the SINGLE writer actor. Every
    // write goes through it, so this never contends for the WAL writer and can
    // never return "database is locked" — the folder-switch 500 storm is gone by
    // construction (the old code ran this on a separate connection that fought
    // ~14 other writers for the lock). The enqueue is idempotent; an
    // already-pending task is returned so its worker gets a (deduped) wakeup.
    let (thumbnail, queued_task) = state
        .writer
        .write(move |database| {
            let thumbnail = database
                .get_thumbnail(file_id, target)?
                .ok_or_else(|| anyhow::anyhow!("media item not found: {file_id}"))?;
            if !is_pending_status(&thumbnail.state) {
                return Ok((thumbnail, None));
            }
            let request = database.request_thumbnail_task(file_id, target, priority)?;
            let wake_task_id = if request.should_enqueue {
                request.task_id
            } else {
                None
            };
            let queued_task = wake_task_id
                .map(|task_id| {
                    database
                        .current_task_attempt_generation(task_id)
                        .map(|attempt| (task_id, attempt, requested_task_priority))
                })
                .transpose()?;
            Ok((request.thumbnail, queued_task))
        })
        .await?;
    if let Some((task_id, attempt_generation, task_priority)) = queued_task {
        // Always dedupe get_thumbnail-driven wakeups over the short window:
        // repeated polls of the same pending tile (every re-render / re-scroll)
        // must not spam the worker queue. The first poll wakes; repeats within
        // the window are no-ops. Fresh vs. re-wake need no distinction — a
        // brand-new task's key isn't in the window yet, so it still wakes.
        enqueue_thumbnail_task_wakeup(&state, task_id, attempt_generation, task_priority, true)
            .await?;
    }
    let status = if is_pending_status(&thumbnail.state) {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(thumbnail_response(thumbnail, blob))))
}

async fn get_thumbnail_blob(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Query(query): Query<ThumbnailQuery>,
) -> ApiResult<axum::response::Response> {
    use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
    use axum::response::Response;
    let target = normalize_profile(query.target.as_deref())
        .ok_or_else(|| CoreError::bad_request("unsupported thumbnail target".to_string()))?;
    // A `thumb_blobs` row exists only when the thumbnail is ready and the file
    // is active: every state-reset / root-disable / delete path drops the blob
    // in the same transaction, and the FK cascades on hard delete. So a present
    // blob is always serveable — fetch it directly and skip the redundant
    // get_thumbnail readiness probe (which cost two extra joined queries per
    // tile on the hottest read path).
    let blob = run_read_database(state, move |database| {
        let blob = database
            .get_thumb_blob(file_id, target)?
            .ok_or_else(|| CoreError::not_found("thumbnail blob missing".to_string()))?;
        Ok(blob)
    })
    .await?;
    Response::builder()
        .header(CONTENT_TYPE, blob.output_format)
        .header(CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header("x-megle-served-by", "db_blob")
        .body(axum::body::Body::from(blob.data))
        .map_err(|err| CoreError::bad_request(format!("failed to build thumbnail response: {err}")))
}

#[derive(Default)]
struct ThumbnailCacheStatsSlot {
    computed_at: Option<Instant>,
    value: Option<ThumbnailCacheSnapshotResponse>,
}

type ThumbnailCacheStatsKey = (Option<i64>, Option<i64>, bool);

// Per-scope single-flight + TTL cache for the cache-stats aggregate. That query
// scans every file in scope (652k on the test library, ~1.5s cold) and holds a
// WAL read snapshot the whole time it runs. The UI polls it every ~2.5s while a
// bulk generation runs, and an aborted HTTP poll does NOT cancel the underlying
// spawn_blocking query — so slow polls piled up (measured 8 concurrent at ~11s
// each), each pinning the WAL checkpoint at its start frame. The checkpoint then
// reclaimed nothing, the WAL grew to gigabytes, and every read/write crawled
// ("folder switching got slow after idle"). Single-flight serializes same-scope
// requests so they can never pile up, and the short TTL serves a recent result
// so the aggregate runs at most once per window, leaving the checkpoint gaps to
// reclaim the WAL.
static THUMBNAIL_CACHE_STATS_CACHE: LazyLock<
    Mutex<HashMap<ThumbnailCacheStatsKey, Arc<tokio::sync::Mutex<ThumbnailCacheStatsSlot>>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));
const THUMBNAIL_CACHE_STATS_TTL: Duration = Duration::from_millis(15000);

async fn get_thumbnail_cache_stats(
    State(state): State<AppState>,
    Query(query): Query<ThumbnailCacheScopeQuery>,
) -> ApiResult<Json<ThumbnailCacheSnapshotResponse>> {
    let scope = thumbnail_cache_scope(query);
    let key = (scope.root_id, scope.folder_id, scope.include_descendants);
    let slot = {
        let mut cache = THUMBNAIL_CACHE_STATS_CACHE
            .lock()
            .expect("thumbnail cache stats cache mutex poisoned");
        cache
            .entry(key)
            .or_insert_with(|| {
                Arc::new(tokio::sync::Mutex::new(ThumbnailCacheStatsSlot::default()))
            })
            .clone()
    };
    // Single-flight per scope: concurrent polls block here until the in-flight
    // computation finishes, then observe the fresh cached value instead of
    // launching their own (WAL-pinning) query.
    let mut guard = slot.lock().await;
    if let (Some(computed_at), Some(value)) = (guard.computed_at, guard.value.as_ref()) {
        if computed_at.elapsed() < THUMBNAIL_CACHE_STATS_TTL {
            return Ok(Json(value.clone()));
        }
    }
    let snapshot = run_read_database(state, move |database| {
        database
            .get_thumbnail_cache_snapshot(scope, crate::thumbnails::GRID_320_PROFILE, 100)
            .map(thumbnail_cache_snapshot_response)
            .map_err(CoreError::from)
    })
    .await?;
    guard.computed_at = Some(Instant::now());
    guard.value = Some(snapshot.clone());
    Ok(Json(snapshot))
}

async fn list_thumbnail_cache_failures(
    State(state): State<AppState>,
    Query(query): Query<ThumbnailFailureQuery>,
) -> ApiResult<Json<ThumbnailFailureListResponse>> {
    let limit = query.limit.unwrap_or(100);
    if !(1..=200).contains(&limit) {
        return Err(CoreError::bad_request(
            "thumbnail failure limit must be between 1 and 200".to_string(),
        ));
    }
    run_read_database(state, move |database| {
        let mut items = database
            .list_thumbnail_failures(crate::thumbnails::GRID_320_PROFILE, limit + 1)
            .map_err(CoreError::from)?;
        let truncated = items.len() > limit;
        items.truncate(limit);
        Ok(Json(ThumbnailFailureListResponse { items, truncated }))
    })
    .await
}

async fn enqueue_thumbnail_cache(
    State(state): State<AppState>,
    Json(body): Json<ThumbnailCacheTaskRequestBody>,
) -> ApiResult<(StatusCode, Json<ThumbnailCacheEnqueueResponse>)> {
    let refresh_mode = ThumbnailCacheRefreshMode::from_wire_value(body.refresh_mode.as_str())
        .ok_or_else(|| {
            CoreError::bad_request(format!(
                "unsupported thumbnail cache refresh mode: {}",
                body.refresh_mode
            ))
        })?;
    let scope = ThumbnailCacheScope {
        root_id: body.root_id,
        folder_id: body.folder_id,
        include_descendants: body.include_descendants.unwrap_or(false),
    };
    // Manual "retry failed/stale" (Settings -> retry button) is the one enqueue
    // path a user can trigger directly against a root the background
    // reachability probe already knows is offline (NAS unplugged, share
    // unmounted). Without this gate the button would reset every
    // transient-failed row in scope back to `queued`, the worker would
    // immediately fail them again against the unreachable path, and the
    // frontend's poll loop (`enqueueThumbnailCacheLoop`) would keep re-issuing
    // the same request since something was "accepted" each round — hammering a
    // known-offline root exactly like `sweep_transient_retries`
    // (`api/mod.rs`) is designed to avoid for the background sweep. Short-
    // circuiting to a zero-accepted response lets the frontend loop's existing
    // `acceptedCount === 0` termination stop it without resetting any rows or
    // waking a worker. Only gates `RetryFailedAndStale` scoped to one root:
    // `MissingOnly`/`StaleOrMissing` enqueues (including the whole-library
    // "generate all" paginated pass) never reset already-failed rows, so they
    // stay ungated; a root-less scope (no root selected) also stays ungated,
    // matching `sweep_transient_retries`' "absent id defaults reachable"
    // convention.
    if refresh_mode == ThumbnailCacheRefreshMode::RetryFailedAndStale {
        if let Some(root_id) = scope.root_id {
            let is_reachable = state
                .root_reachability
                .lock()
                .map(|guard| *guard.get(&root_id).unwrap_or(&true))
                .unwrap_or(true);
            if !is_reachable {
                let response = thumbnail_cache_enqueue_response(ThumbnailCacheEnqueueResult {
                    accepted_count: 0,
                    task_id_to_wake: None,
                    stats: ThumbnailCacheStats::default(),
                    next_cursor: None,
                });
                return Ok((StatusCode::ACCEPTED, Json(response)));
            }
        }
    }
    let file_ids = body.file_ids.unwrap_or_default();
    let limit = body.limit.unwrap_or(256).clamp(1, 4096);
    // Keyset cursor for the whole-library "generate all" pass. When present,
    // the enqueue scans one bounded window of `limit` files past this id and
    // returns `nextCursor`; the client advances it batch by batch. This keeps
    // each writer job short (bounded by the window) so it interleaves with
    // interactive writes, and makes the whole-library scan O(files) total
    // instead of re-scanning from the start on every batch. Absent for the
    // folder-scoped/explicit-id paths, which stay on the whole-scope enqueue.
    let after_file_id = body.after_file_id;
    // Run on a dedicated connection, NOT the shared write mutex. This bulk enqueue
    // inserts up to `limit` (256-4096) task rows in one transaction; on the shared
    // mutex that long hold blocked the per-scroll priority-scope write queued right
    // behind it, and since both are client-"interactive" the two pinned the web
    // client's interactive request slots and viewport prioritization stalled on a
    // cold library. A dedicated connection lets it proceed without head-of-line
    // blocking the smaller interactive writes (serialized only by the WAL writer,
    // which the scan and background blob workers yield via the interactive gate).
    let (response, wakeup) = run_dedicated_or_shared_database(state.clone(), move |database| {
        let result = if file_ids.is_empty() {
            database
                .enqueue_thumbnail_cache_tasks(scope, refresh_mode, limit, after_file_id)
                .map_err(CoreError::from)?
        } else {
            database
                .enqueue_thumbnail_cache_tasks_for_file_ids(&file_ids, refresh_mode, limit)
                .map_err(CoreError::from)?
        };
        let wakeup = result
            .task_id_to_wake
            .map(|task_id| {
                database
                    .current_task_attempt_generation(task_id)
                    .map(|attempt_generation| (task_id, attempt_generation))
            })
            .transpose()
            .map_err(CoreError::from)?;
        Ok((thumbnail_cache_enqueue_response(result), wakeup))
    })
    .await?;
    if let Some((task_id, attempt_generation)) = wakeup {
        enqueue_thumbnail_task_wakeup(
            &state,
            task_id,
            attempt_generation,
            THUMBNAIL_BULK_PRIORITY,
            false,
        )
        .await?;
    }
    Ok((StatusCode::ACCEPTED, Json(response)))
}

async fn clear_thumbnail_cache(
    State(state): State<AppState>,
) -> ApiResult<Json<ThumbnailCacheClearResponse>> {
    let response = run_shared_database(state, move |database| {
        database
            .clear_thumbnail_cache()
            .map(thumbnail_cache_clear_response)
            .map(Json)
            .map_err(CoreError::from)
    })
    .await?;
    Ok(response)
}

async fn cancel_bulk_thumbnail_cache(
    State(state): State<AppState>,
) -> ApiResult<Json<ThumbnailCacheCancelBulkResponse>> {
    let response = run_shared_database(state, move |database| {
        database
            .cancel_pending_bulk_thumbnail_tasks()
            .map(|cancelled_count| Json(ThumbnailCacheCancelBulkResponse { cancelled_count }))
            .map_err(CoreError::from)
    })
    .await?;
    Ok(response)
}

async fn get_preview(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    query: Result<Query<PreviewQuery>, QueryRejection>,
) -> ApiResult<Response> {
    // Map the extractor's rejection ourselves. Axum's built-in rejection body
    // is PLAIN TEXT, but the OpenAPI contract declares every 400 on this route
    // as an `ErrorResponse` JSON object — the same shape every other route
    // returns. `list_notification_outbox` uses this pattern for the same reason.
    let Query(query) = query.map_err(|_| {
        CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "viewportLongPx must be a positive integer".to_string(),
        )
    })?;
    use axum::body::Body;
    use axum::http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE};
    use tokio_util::io::ReaderStream;

    let (source_path, media_kind, source_max_px) =
        run_read_database(state.clone(), move |database| {
            let media = database
                .get_media(file_id)?
                .ok_or_else(|| CoreError::not_found(format!("media item not found: {file_id}")))?;
            let source_path = database.resolve_file_source_path(file_id)?.ok_or_else(|| {
                CoreError::not_found(format!("media source not found: {file_id}"))
            })?;
            // `width`/`height` stay NULL until the thumbnail pass fills them in;
            // 0 then means "unknown", which resolves to "stream the original".
            let source_max_px = media
                .width
                .unwrap_or(0)
                .max(media.height.unwrap_or(0))
                .clamp(0, u32::MAX as i64) as u32;
            Ok((source_path, media.kind, source_max_px))
        })
        .await?;

    let file = tokio::fs::File::open(&source_path)
        .await
        .map_err(|err| CoreError::not_found(format!("media source file missing: {err}")))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|err| CoreError::not_found(format!("media source metadata missing: {err}")))?;
    if !metadata.is_file() {
        return Err(CoreError::not_found(format!(
            "media source is not a file: {}",
            source_path.display()
        )));
    }

    let ext = crate::decode::formats::lowercase_extension(&source_path);
    // Clamp rather than reject. `select_preview_profile` already saturates to
    // the largest tier, so an absurd value is harmless — but clamping keeps the
    // accepted input space finite and documented instead of "anything up to
    // u32::MAX happens to work".
    let viewport_long_px = query
        .viewport_long_px
        .unwrap_or(0)
        .min(MAX_VIEWPORT_LONG_PX);
    if let Some(plan) = resolve_preview_plan(
        media_kind.as_deref(),
        ext.as_str(),
        source_max_px,
        viewport_long_px,
    ) {
        // `Ok(None)` means the representation could not be produced (no decoder
        // for the format, ffmpeg missing, NAS blip, locked file). That is a
        // degradation, not a failure: fall through to the raw-stream branch
        // below, which is exactly what this route served before the preview
        // pipeline existed. Only a decode PANIC propagates as an error.
        if let Some(response) =
            serve_cached_preview_webp(state, file_id, plan, &source_path, &metadata).await?
        {
            return Ok(response);
        }
    }

    let body = Body::from_stream(ReaderStream::new(file));
    let mut response = Response::builder()
        .header(
            CONTENT_TYPE,
            preview_content_type(&source_path, media_kind.as_deref()),
        )
        .header(CACHE_CONTROL, "private, max-age=0, must-revalidate")
        .header(PREVIEW_TIER_HEADER, "original");
    response = response.header(CONTENT_LENGTH, metadata.len().to_string());
    response
        .body(body)
        .map_err(|err| CoreError::bad_request(format!("failed to build preview response: {err}")))
}

/// Tells the client WHICH representation it received, so it knows whether a
/// background full-resolution upgrade is still worth requesting.
const PREVIEW_TIER_HEADER: &str = "x-megle-preview-tier";

/// Upper bound on a reported preview viewport. Far beyond any real display,
/// and values above it are clamped rather than rejected.
const MAX_VIEWPORT_LONG_PX: u32 = 16384;

/// Serve one cached-or-generated WebP preview representation.
///
/// `Ok(None)` is the DEGRADE signal: the plan was valid but the representation
/// could not be produced, and the caller must fall back to streaming the
/// original bytes. Only a decode panic (a bug) becomes an `Err`.
async fn serve_cached_preview_webp(
    state: AppState,
    file_id: i64,
    plan: PreviewPlan,
    source_path: &FilePath,
    metadata: &std::fs::Metadata,
) -> ApiResult<Option<Response>> {
    use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};

    let source_size = metadata.len() as i64;
    let source_mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);

    let cached = run_read_database(state.clone(), move |database| {
        Ok(database.get_preview_blob(file_id, plan.profile)?)
    })
    .await?
    .filter(|blob| blob.source_size == source_size && blob.source_mtime == source_mtime);

    let blob = match cached {
        Some(blob) => blob,
        None => {
            // Decode on the blocking pool, NEVER inside the writer closure: the
            // writer owns the process's single write connection, and a
            // multi-hundred-millisecond decode there would stall every write.
            let generation_path = source_path.to_path_buf();
            // Bound concurrent decodes. Without this, one bare `spawn_blocking`
            // per request stacked without limit on the shared blocking pool —
            // each holding a full decoded image and delaying every read route
            // using that pool. Acquired BEFORE the spawn so waiters queue on a
            // cheap future rather than on a thread.
            let _decode_permit = state
                .preview_decode_gate
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| CoreError::internal("preview decode gate closed".to_string()))?;
            let outcome = tokio::task::spawn_blocking(move || {
                generate_preview_webp(&generation_path, plan.max_px)
            })
            .await
            // A panic is a bug in the decode path, not a runtime degradation.
            .map_err(|err| CoreError::internal(format!("preview generation panicked: {err}")))?;
            let generated = match outcome {
                Ok(generated) => generated,
                Err(error) => {
                    // Degrade instead of failing. Every reason this fires — no
                    // decoder for the format, ffmpeg not on PATH, an unreachable
                    // NAS, a locked file, OOM — leaves the ORIGINAL bytes
                    // perfectly servable, and those are what this route returned
                    // before the preview pipeline existed. A 500 here would blank
                    // out a picture that used to display fine.
                    crate::debug_log!(
                        "preview tier generation failed (file {file_id}, profile {}): {error:#}",
                        plan.profile
                    );
                    return Ok(None);
                }
            };

            let now = crate::db::unix_timestamp();
            let record = PreviewBlobRecord {
                file_id,
                profile: plan.profile.to_string(),
                data: generated.data,
                width: generated.width,
                height: generated.height,
                byte_size: generated.byte_size,
                output_format: generated.output_format.to_string(),
                source_size,
                source_mtime,
                created_at: now,
                updated_at: now,
            };
            let stored = record.clone();
            // Persisting is a CACHE write, so its failure must never cost the
            // user the picture we already decoded. Disk full, SQLITE_BUSY past
            // the timeout (fsops writes on its own connection and genuinely
            // contends), an FK violation because the file was removed during
            // the decode, or a writer shut down by a library switch — every one
            // of those leaves this WebP perfectly serveable. Propagating turned
            // them into a 500 and a blank frame, which is the same hole the
            // decode path already closes by degrading.
            if let Err(error) = run_shared_database(state, move |database| {
                database.upsert_preview_blob(stored)?;
                Ok(())
            })
            .await
            {
                crate::debug_log!(
                    "preview tier cache write failed (file {file_id}, profile {}): {}",
                    plan.profile,
                    error.message
                );
            }
            record
        }
    };

    let response = Response::builder()
        .header(CONTENT_TYPE, blob.output_format)
        // Same policy as the original-bytes branch: the frontend owns caching,
        // and a tier can legitimately change when the source file changes.
        .header(CACHE_CONTROL, "private, max-age=0, must-revalidate")
        .header(PREVIEW_TIER_HEADER, plan.profile)
        .body(axum::body::Body::from(blob.data))
        .map_err(|err| {
            CoreError::bad_request(format!("failed to build preview response: {err}"))
        })?;
    Ok(Some(response))
}

async fn list_tasks(State(state): State<AppState>) -> ApiResult<Json<ListResponse<TaskRecord>>> {
    let items = run_read_database(state, |database| Ok(database.list_tasks()?)).await?;
    Ok(Json(ListResponse {
        items,
        next_cursor: None,
        total_count: None,
    }))
}

async fn enqueue_scan(
    State(state): State<AppState>,
    Json(payload): Json<ScanTaskRequest>,
) -> ApiResult<(StatusCode, Json<AcceptedResponse>)> {
    let root_id = payload.root_id;
    let (task_id, attempt_generation) = run_shared_database(state.clone(), move |database| {
        let root = database
            .get_root(root_id)?
            .ok_or_else(|| CoreError::not_found(format!("root not found: {}", root_id)))?;
        if !root.enabled {
            return Err(CoreError::conflict(format!(
                "root is disabled: {}",
                root_id
            )));
        }
        let task_id =
            database.create_root_scan_task_with_origin(root_id, NotificationOrigin::User)?;
        let attempt_generation = database.current_task_attempt_generation(task_id)?;
        Ok((task_id, attempt_generation))
    })
    .await?;
    enqueue_task(&state, task_id, attempt_generation).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(AcceptedResponse {
            accepted: true,
            task_id: Some(task_id),
            root_id: Some(root_id),
            scan: None,
        }),
    ))
}

async fn enqueue_interactive_folder_scan(
    State(state): State<AppState>,
    Json(payload): Json<InteractiveFolderScanTaskRequest>,
) -> ApiResult<(StatusCode, Json<AcceptedResponse>)> {
    let folder_id = payload.folder_id;
    let (task_id, attempt_generation, root_id) =
        run_shared_database(state.clone(), move |database| {
            if !database.folder_exists(folder_id)? {
                return Err(CoreError::not_found(format!(
                    "folder not found: {}",
                    folder_id
                )));
            }
            let task_id = database.create_interactive_folder_scan_task_with_origin(
                folder_id,
                NotificationOrigin::User,
            )?;
            let task = database
                .get_task(task_id)?
                .ok_or_else(|| CoreError::not_found(format!("task not found: {task_id}")))?;
            let attempt_generation = database.current_task_attempt_generation(task_id)?;
            let root_id = task
                .root_id
                .ok_or_else(|| CoreError::conflict(format!("task missing root id: {task_id}")))?;
            Ok((task_id, attempt_generation, root_id))
        })
        .await?;
    enqueue_task(&state, task_id, attempt_generation).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(AcceptedResponse {
            accepted: true,
            task_id: Some(task_id),
            root_id: Some(root_id),
            scan: None,
        }),
    ))
}

async fn sync_thumbnail_priority_scope(
    State(state): State<AppState>,
    Json(payload): Json<ThumbnailPriorityScopeSyncRequest>,
) -> ApiResult<(StatusCode, Json<AcceptedResponse>)> {
    let root_id = payload.root_id;
    let selected_file_ids = payload.selected_file_ids;
    let visible_file_ids = payload.visible_file_ids;
    let ahead_file_ids = payload.ahead_file_ids;
    let retained_file_ids = payload.retained_file_ids;
    // Map the wire mode to the leaver policy. Absent / unknown ⇒ Demote (the
    // allPersist behavior), which never cancels — the safe default.
    let leaver_policy = match payload.generation_mode.as_deref() {
        Some("renderedOnly") | Some("cacheOnly") => ThumbnailScopeLeaverPolicy::Cancel,
        _ => ThumbnailScopeLeaverPolicy::DemoteToBulk,
    };
    // Per-scroll prioritization write. It goes through the single writer actor
    // (like every write), so it is serialized with — never blocked by — other
    // writes; the actor checkpoints between jobs.
    let promoted_tasks = run_shared_database(state.clone(), move |database| {
        if database.get_root(root_id)?.is_none() {
            return Err(CoreError::not_found(format!("root not found: {}", root_id)));
        }
        let result = database.sync_thumbnail_priority_scope_with_promotions(
            root_id,
            &selected_file_ids,
            &visible_file_ids,
            &ahead_file_ids,
            &retained_file_ids,
            leaver_policy,
        )?;
        Ok(result
            .promoted_pending_task_ids
            .into_iter()
            .map(|task_id| {
                database
                    .current_task_attempt_generation(task_id)
                    .map(|attempt| (task_id, attempt))
            })
            .collect::<anyhow::Result<Vec<_>>>()?)
    })
    .await?;
    for (task_id, attempt_generation) in promoted_tasks {
        enqueue_task(&state, task_id, attempt_generation).await?;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(AcceptedResponse {
            accepted: true,
            task_id: None,
            root_id: Some(root_id),
            scan: None,
        }),
    ))
}

async fn cancel_task(
    State(state): State<AppState>,
    Path(task_id): Path<i64>,
) -> ApiResult<(StatusCode, Json<AcceptedResponse>)> {
    let task = run_shared_database(state, move |database| {
        database.cancel_task(task_id).map_err(map_task_action_error)
    })
    .await?;
    Ok((
        StatusCode::OK,
        Json(AcceptedResponse {
            accepted: true,
            task_id: Some(task.id),
            root_id: task.root_id,
            scan: None,
        }),
    ))
}

async fn retry_task(
    State(state): State<AppState>,
    Path(task_id): Path<i64>,
) -> ApiResult<(StatusCode, Json<AcceptedResponse>)> {
    let task = run_shared_database(state.clone(), move |database| {
        database.retry_task(task_id).map_err(map_task_action_error)
    })
    .await?;
    enqueue_task(&state, task.id, task.attempt_generation).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(AcceptedResponse {
            accepted: true,
            task_id: Some(task.id),
            root_id: task.root_id,
            scan: None,
        }),
    ))
}

async fn rename_file(
    State(state): State<AppState>,
    Json(payload): Json<RenameRequestBody>,
) -> ApiResult<Json<FileOperationRecord>> {
    let database = fsops_database(&state)?.clone();
    let writer = state.writer.clone();
    let record = tokio::task::spawn_blocking(move || {
        fsops::rename_with_writer(
            &database,
            &writer,
            RenameRequest {
                file_id: payload.file_id,
                folder_id: payload.folder_id,
                new_name: payload.new_name,
            },
        )
    })
    .await
    .map_err(|error| CoreError::from(anyhow::anyhow!("file rename worker failed: {error}")))?
    .map_err(map_fsops_error)?;
    Ok(Json(record))
}

async fn move_files(
    State(state): State<AppState>,
    Json(payload): Json<MoveRequestBody>,
) -> ApiResult<Json<FileOperationsResponse>> {
    let database = fsops_database(&state)?.clone();
    let writer = state.writer.clone();
    let operations = tokio::task::spawn_blocking(move || {
        fsops::move_items_with_writer(
            &database,
            &writer,
            MoveRequest {
                file_ids: payload.file_ids.unwrap_or_default(),
                folder_ids: payload.folder_ids.unwrap_or_default(),
                target_folder_id: payload.target_folder_id,
            },
        )
    })
    .await
    .map_err(|error| CoreError::from(anyhow::anyhow!("file move worker failed: {error}")))?
    .map_err(map_fsops_error)?;
    Ok(Json(FileOperationsResponse { operations }))
}

async fn delete_files(
    State(state): State<AppState>,
    Json(payload): Json<DeleteRequestBody>,
) -> ApiResult<Json<FileOperationsResponse>> {
    let database = fsops_database(&state)?.clone();
    let writer = state.writer.clone();
    let operations = tokio::task::spawn_blocking(move || {
        fsops::delete_with_writer(
            &database,
            &writer,
            DeleteRequest {
                file_ids: payload.file_ids.unwrap_or_default(),
                folder_ids: payload.folder_ids.unwrap_or_default(),
                permanent: payload.permanent.unwrap_or(false),
            },
        )
    })
    .await
    .map_err(|error| CoreError::from(anyhow::anyhow!("file delete worker failed: {error}")))?
    .map_err(map_fsops_error)?;
    Ok(Json(FileOperationsResponse { operations }))
}

async fn list_file_operations(
    State(state): State<AppState>,
    Query(query): Query<ListFileOperationsQuery>,
) -> ApiResult<Json<ListResponse<FileOperationRecord>>> {
    let limit = query.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(CoreError::bad_request(format!(
            "limit must be between 1 and 200: {limit}"
        )));
    }
    let page = fsops::list_recent(fsops_database(&state)?, limit, query.cursor.as_deref())
        .map_err(map_fsops_error)?;
    Ok(Json(ListResponse {
        items: page.items,
        next_cursor: page.next_cursor,
        total_count: page.total_count,
    }))
}

async fn list_notification_outbox(
    State(state): State<AppState>,
    query: Result<Query<ListNotificationOutboxQuery>, QueryRejection>,
) -> ApiResult<Json<NotificationOutboxResponse>> {
    let Query(query) =
        query.map_err(|_| invalid_notification_request(INVALID_NOTIFICATION_OUTBOX_QUERY))?;
    let after_sequence = query.after_sequence.unwrap_or(0);
    let limit = query.limit.unwrap_or(100);
    let wait_ms = query.wait_ms.unwrap_or(0);
    if after_sequence < 0 {
        return Err(CoreError::bad_request(
            "afterSequence must be greater than or equal to 0".to_string(),
        ));
    }
    if after_sequence > MAX_SAFE_NOTIFICATION_SEQUENCE {
        return Err(CoreError::bad_request(format!(
            "afterSequence must be less than or equal to {MAX_SAFE_NOTIFICATION_SEQUENCE}"
        )));
    }
    if !(1..=200).contains(&limit) {
        return Err(CoreError::bad_request(
            "limit must be between 1 and 200".to_string(),
        ));
    }
    if !(0..=30_000).contains(&wait_ms) {
        return Err(CoreError::bad_request(
            "waitMs must be between 0 and 30000".to_string(),
        ));
    }

    let items = read_notification_outbox(&state, after_sequence, limit).await?;
    if !items.is_empty() || wait_ms == 0 {
        return Ok(Json(notification_outbox_response(items, after_sequence)?));
    }

    // `notify_waiters` does not retain a permit. Register first, then repeat the
    // read to close the commit window between the initial empty query and this
    // waiter becoming visible to producers.
    let notified = state.notification_outbox_wake.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    let items = read_notification_outbox(&state, after_sequence, limit).await?;
    if !items.is_empty() {
        return Ok(Json(notification_outbox_response(items, after_sequence)?));
    }

    #[cfg(test)]
    let waiter_activity = state
        .notification_outbox_test_probe
        .as_ref()
        .map(|probe| probe.begin_waiter());
    let _ = tokio::time::timeout(Duration::from_millis(wait_ms as u64), notified.as_mut()).await;
    #[cfg(test)]
    drop(waiter_activity);
    let items = read_notification_outbox(&state, after_sequence, limit).await?;
    Ok(Json(notification_outbox_response(items, after_sequence)?))
}

async fn read_notification_outbox(
    state: &AppState,
    after_sequence: i64,
    limit: i64,
) -> ApiResult<Vec<NotificationOutboxEvent>> {
    let read_permit = Arc::clone(&state.notification_outbox_read_gate)
        .acquire_owned()
        .await
        .map_err(|_| {
            CoreError::from(anyhow::anyhow!(
                "notification outbox read gate is unavailable"
            ))
        })?;
    #[cfg(test)]
    let read_activity = if let Some(probe) = &state.notification_outbox_test_probe {
        let activity = probe.begin_read();
        probe.wait_for_read_release().await;
        Some(activity)
    } else {
        None
    };
    run_read_database(state.clone(), move |database| {
        let _read_permit = read_permit;
        #[cfg(test)]
        let _read_activity = read_activity;
        Ok(database.list_notification_outbox(after_sequence, limit)?)
    })
    .await
}

fn notification_outbox_response(
    items: Vec<NotificationOutboxEvent>,
    after_sequence: i64,
) -> ApiResult<NotificationOutboxResponse> {
    if items
        .iter()
        .any(|event| !(0..=MAX_SAFE_NOTIFICATION_SEQUENCE).contains(&event.sequence))
    {
        return Err(CoreError::bad_request(format!(
            "notification outbox sequence must be between 0 and {MAX_SAFE_NOTIFICATION_SEQUENCE}"
        )));
    }
    let max_sequence = items
        .last()
        .map(|event| event.sequence)
        .unwrap_or(after_sequence);
    Ok(NotificationOutboxResponse {
        items,
        max_sequence,
    })
}

async fn acknowledge_notification_outbox(
    State(state): State<AppState>,
    body: Result<Json<NotificationOutboxAckRequest>, JsonRejection>,
) -> ApiResult<Json<NotificationOutboxAckResponse>> {
    let Json(body) = body
        .map_err(|_| invalid_notification_request(INVALID_NOTIFICATION_OUTBOX_ACKNOWLEDGEMENT))?;
    if body.max_sequence < 0 {
        return Err(CoreError::bad_request(
            "maxSequence must be greater than or equal to 0".to_string(),
        ));
    }
    if body.max_sequence > MAX_SAFE_NOTIFICATION_SEQUENCE {
        return Err(CoreError::bad_request(format!(
            "maxSequence must be less than or equal to {MAX_SAFE_NOTIFICATION_SEQUENCE}"
        )));
    }
    let max_sequence = body.max_sequence;
    run_shared_database(state, move |database| {
        database.acknowledge_notification_outbox(max_sequence)?;
        Ok(())
    })
    .await?;
    Ok(Json(NotificationOutboxAckResponse { acknowledged: true }))
}

fn invalid_notification_request(message: &'static str) -> CoreError {
    CoreError::coded(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        message.to_string(),
    )
}

async fn list_plugins(State(state): State<AppState>) -> ApiResult<Json<PluginListResponse>> {
    // Read on the dedicated WAL read connection so it never blocks behind a
    // scan-time write holding the shared mutex (see get_user_metadata).
    let items = run_read_database(state, |database| Ok(database.list_plugins()?)).await?;
    Ok(Json(PluginListResponse { items }))
}

async fn discover_plugins(
    State(state): State<AppState>,
) -> ApiResult<(StatusCode, Json<PluginDiscoveryResponse>)> {
    let plugins_dir = resolve_plugins_dir(&state);
    // The locked variant performs the disk walk before acquiring the
    // mutex, then reacquires per upsert. This keeps other API endpoints
    // responsive while a large `plugins/` directory is enumerated.
    let report = plugins::discover_and_persist_locked(fsops_database(&state)?, &plugins_dir)?;
    let errors = report
        .errors
        .into_iter()
        .map(|entry| PluginDiscoveryError {
            manifest_path: entry.manifest_path.to_string_lossy().into_owned(),
            message: entry.message,
        })
        .collect();
    Ok((
        StatusCode::ACCEPTED,
        Json(PluginDiscoveryResponse {
            discovered: report.discovered as u64,
            errors,
        }),
    ))
}

async fn get_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<Json<PluginRecord>> {
    let plugin = run_read_database(state, move |database| {
        database
            .get_plugin(&plugin_id)?
            .ok_or_else(|| not_found_plugin(&plugin_id))
    })
    .await?;
    Ok(Json(plugin))
}

async fn enable_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<Json<PluginRecord>> {
    let updated = run_shared_database(state, move |database| {
        let existing = database
            .get_plugin(&plugin_id)?
            .ok_or_else(|| not_found_plugin(&plugin_id))?;
        if existing.status == "invalid" {
            return Err(CoreError::coded(
                StatusCode::CONFLICT,
                "manifest_invalid",
                format!("plugin manifest is invalid and cannot be enabled: {plugin_id}"),
            ));
        }
        database
            .set_plugin_enabled(&plugin_id, true)?
            .ok_or_else(|| not_found_plugin(&plugin_id))
    })
    .await?;
    Ok(Json(updated))
}

async fn disable_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<Json<PluginRecord>> {
    let updated = run_shared_database(state, move |database| {
        database
            .set_plugin_enabled(&plugin_id, false)?
            .ok_or_else(|| not_found_plugin(&plugin_id))
    })
    .await?;
    Ok(Json(updated))
}

async fn delete_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<Json<DeletePluginResponse>> {
    run_shared_database(state, move |database| {
        if database.delete_plugin(&plugin_id)? {
            Ok(())
        } else {
            Err(not_found_plugin(&plugin_id))
        }
    })
    .await?;
    Ok(Json(DeletePluginResponse { deleted: true }))
}

fn not_found_plugin(plugin_id: &str) -> CoreError {
    CoreError::coded(
        StatusCode::NOT_FOUND,
        "plugin_not_found",
        format!("plugin not found: {plugin_id}"),
    )
}

/// The dedicated write connection for the infrequent file-operations and
/// plugin-discovery paths (which take a `&Mutex<Database>`). Absent only for
/// in-memory/test databases; those tests don't exercise fsops.
fn fsops_database(state: &AppState) -> ApiResult<&std::sync::Arc<std::sync::Mutex<Database>>> {
    state.fsops_database.as_ref().ok_or_else(|| {
        CoreError::from(anyhow::anyhow!(
            "file operations are unavailable without a file-backed database"
        ))
    })
}

fn resolve_plugins_dir(state: &AppState) -> std::path::PathBuf {
    // Prefer the directory the startup code already resolved against the
    // active database path. Falling back to the env var or `./plugins`
    // would diverge from `main.rs` and was the root cause of Batch G HIGH 1.
    if let Some(configured) = state.plugins_dir.as_ref() {
        return configured.clone();
    }
    // Defensive fallback for callers that built `AppState` without going
    // through `router_with_config` (test helpers, the deprecated `router`
    // alias). Match the same env-var precedence as
    // `plugins::resolve_plugins_dir` so behavior stays predictable.
    if let Some(value) = std::env::var("MEGLE_PLUGINS_DIR")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return std::path::PathBuf::from(value);
    }
    std::path::PathBuf::from("./plugins")
}

async fn list_tags(State(state): State<AppState>) -> ApiResult<Json<ListResponse<TagRecord>>> {
    // Read on the dedicated WAL read connection so it never blocks behind a
    // scan-time write holding the shared mutex (see get_user_metadata).
    let items = run_read_database(state, |database| Ok(database.list_tags()?)).await?;
    Ok(Json(ListResponse {
        items,
        next_cursor: None,
        total_count: None,
    }))
}

async fn create_tag(
    State(state): State<AppState>,
    Json(payload): Json<CreateTagRequestBody>,
) -> ApiResult<(StatusCode, Json<TagRecord>)> {
    let tag = run_shared_database(state, move |database| {
        let color = payload.color.as_deref();
        database
            .create_tag(payload.name.as_str(), color)
            .map_err(map_tag_error)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(tag)))
}

async fn delete_tag(
    State(state): State<AppState>,
    Path(tag_id): Path<i64>,
) -> ApiResult<Json<DeleteTagResponse>> {
    run_shared_database(state, move |database| {
        let deleted = database.delete_tag(tag_id)?;
        if !deleted {
            return Err(CoreError::not_found(format!("tag not found: {tag_id}")));
        }
        Ok(())
    })
    .await?;
    Ok(Json(DeleteTagResponse { deleted: true }))
}

async fn get_user_metadata(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
) -> ApiResult<Json<UserMetadataRecord>> {
    // Read via a dedicated WAL read connection (not the shared write mutex). WAL
    // readers never block on the writer, so an inspector metadata fetch stays
    // fast during a scan. The old `state.database.lock()` here hung for seconds
    // under scroll+scan load (the per-scroll priority-scope write holds the
    // mutex while it waits on the scan's SQLite writer), and because the client
    // caps metadata requests as "interactive", two hung fetches pinned both
    // interactive slots and the grid stopped re-prioritizing.
    let record = run_read_database(state, move |database| {
        database
            .get_user_metadata(file_id)?
            .ok_or_else(|| CoreError::not_found(format!("media item not found: {file_id}")))
    })
    .await?;
    Ok(Json(record))
}

async fn update_user_metadata(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Json(payload): Json<serde_json::Value>,
) -> ApiResult<Json<UserMetadataRecord>> {
    let patch = parse_user_metadata_patch(&payload)?;
    let record = run_shared_database(state, move |database| {
        database
            .upsert_user_metadata_partial(file_id, patch)
            .map_err(map_metadata_error)
    })
    .await?;
    Ok(Json(record))
}

async fn set_file_tags(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Json(payload): Json<SetFileTagsRequestBody>,
) -> ApiResult<Json<FileTagsResponse>> {
    let tag_ids = run_shared_database(state, move |database| {
        database
            .set_file_tags(file_id, &payload.tag_ids)
            .map_err(map_tag_error)
    })
    .await?;
    Ok(Json(FileTagsResponse { file_id, tag_ids }))
}

async fn add_file_tag(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Json(payload): Json<AddFileTagRequestBody>,
) -> ApiResult<Json<FileTagsResponse>> {
    let tag_ids = run_shared_database(state, move |database| {
        database
            .add_file_tag(file_id, payload.tag_id)
            .map_err(map_tag_error)
    })
    .await?;
    Ok(Json(FileTagsResponse { file_id, tag_ids }))
}

async fn remove_file_tag(
    State(state): State<AppState>,
    Path((file_id, tag_id)): Path<(i64, i64)>,
) -> ApiResult<Json<FileTagsResponse>> {
    let tag_ids = run_shared_database(state, move |database| {
        database
            .remove_file_tag(file_id, tag_id)
            .map_err(map_tag_error)
    })
    .await?;
    Ok(Json(FileTagsResponse { file_id, tag_ids }))
}

async fn search_media(
    State(state): State<AppState>,
    ExtraQuery(query): ExtraQuery<SearchMediaQuery>,
) -> ApiResult<Json<ListResponse<MediaRecord>>> {
    let sort = parse_search_sort(query.sort.as_deref())?.to_string();
    let kind = parse_media_kind(query.kind.as_deref())?.map(str::to_string);
    if let Some(min_rating) = query.min_rating {
        if !(1..=5).contains(&min_rating) {
            return Err(CoreError::bad_request(format!(
                "minRating must be between 1 and 5: {min_rating}"
            )));
        }
    }
    for tag_id in &query.tag_id {
        if *tag_id <= 0 {
            return Err(CoreError::bad_request(format!(
                "tagId must be positive: {tag_id}"
            )));
        }
    }
    let limit = query.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err(CoreError::bad_request(format!(
            "limit must be between 1 and 500: {limit}"
        )));
    }
    let search_query = SearchQuery {
        q: query.q,
        root_id: query.root_id,
        folder_id: query.folder_id,
        include_descendants: query.include_descendants.unwrap_or(false),
        kind,
        min_rating: query.min_rating,
        favorite: query.favorite,
        tag_ids: query.tag_id,
        sort,
        limit,
        offset: query.offset,
        cursor: query.cursor,
    };
    let page = run_read_database(state, move |database| {
        database
            .search_media_page(search_query)
            .map_err(map_cursor_error)
    })
    .await?;
    Ok(Json(ListResponse {
        items: page.items,
        next_cursor: page.next_cursor,
        total_count: page.total_count,
    }))
}

#[allow(dead_code)]
fn empty_list<T>() -> ListResponse<T> {
    ListResponse {
        items: Vec::new(),
        next_cursor: None,
        total_count: None,
    }
}

fn accepted() -> AcceptedResponse {
    AcceptedResponse {
        accepted: true,
        task_id: None,
        root_id: None,
        scan: None,
    }
}

fn thumbnail_response(
    thumbnail: ThumbnailRecord,
    blob: Option<ThumbBlobRecord>,
) -> ThumbnailResponse {
    let ready_blob = if thumbnail.state.as_str() == "ready" {
        blob.as_ref()
    } else {
        None
    };
    let width = ready_blob.map(|blob| blob.width).or_else(|| {
        (thumbnail.state.as_str() != "ready")
            .then_some(thumbnail.width)
            .flatten()
    });
    let height = ready_blob.map(|blob| blob.height).or_else(|| {
        (thumbnail.state.as_str() != "ready")
            .then_some(thumbnail.height)
            .flatten()
    });
    let byte_size = ready_blob.map(|blob| blob.byte_size).or_else(|| {
        (thumbnail.state.as_str() != "ready")
            .then_some(thumbnail.byte_size)
            .flatten()
    });
    let output_format = ready_blob
        .map(|blob| blob.output_format.clone())
        .unwrap_or_else(|| thumbnail.output_format.clone());
    let served_by = if ready_blob.is_some() {
        thumbnail
            .served_by
            .clone()
            .or_else(|| Some("db_blob".to_string()))
    } else {
        None
    };
    let asset = ready_blob.map(|blob| ThumbnailAsset {
        width: blob.width,
        height: blob.height,
        byte_size: blob.byte_size,
    });
    ThumbnailResponse {
        file_id: thumbnail.file_id,
        target: thumbnail.profile,
        state: thumbnail.state,
        short_side_px: thumbnail.short_side_px,
        output_format,
        width,
        height,
        byte_size,
        served_by,
        asset,
        error: thumbnail.error,
        updated_at: thumbnail.updated_at,
    }
}

fn thumbnail_cache_scope(query: ThumbnailCacheScopeQuery) -> ThumbnailCacheScope {
    ThumbnailCacheScope {
        root_id: query.root_id,
        folder_id: query.folder_id,
        include_descendants: query.include_descendants.unwrap_or(false),
    }
}

fn thumbnail_cache_stats_response(stats: ThumbnailCacheStats) -> ThumbnailCacheStatsResponse {
    ThumbnailCacheStatsResponse {
        cached_count: stats.cached_count,
        missing_count: stats.missing_count,
        stale_count: stats.stale_count,
        failed_count: stats.failed_count,
        pending_candidate_count: stats.pending_candidate_count,
        total_blob_bytes: stats.total_blob_bytes,
        active_bulk_task_count: stats.active_bulk_task_count,
    }
}

fn thumbnail_cache_snapshot_response(
    snapshot: ThumbnailCacheSnapshot,
) -> ThumbnailCacheSnapshotResponse {
    ThumbnailCacheSnapshotResponse {
        stats: thumbnail_cache_stats_response(snapshot.stats),
        failure_details: snapshot.failure_details,
        failure_details_truncated: snapshot.failure_details_truncated,
    }
}

fn thumbnail_cache_enqueue_response(
    result: ThumbnailCacheEnqueueResult,
) -> ThumbnailCacheEnqueueResponse {
    ThumbnailCacheEnqueueResponse {
        accepted_count: result.accepted_count,
        cached_count: result.stats.cached_count,
        missing_count: result.stats.missing_count,
        stale_count: result.stats.stale_count,
        failed_count: result.stats.failed_count,
        pending_candidate_count: result.stats.pending_candidate_count,
        total_blob_bytes: result.stats.total_blob_bytes,
        active_bulk_task_count: result.stats.active_bulk_task_count,
        next_cursor: result.next_cursor,
    }
}

fn thumbnail_cache_clear_response(
    result: ThumbnailCacheClearResult,
) -> ThumbnailCacheClearResponse {
    ThumbnailCacheClearResponse {
        cleared: result.deleted_blob_count != 0 || result.reset_thumbnail_count != 0,
        deleted_blob_count: result.deleted_blob_count,
        reset_thumbnail_count: result.reset_thumbnail_count,
    }
}

fn parse_media_sort(value: Option<&str>) -> ApiResult<&'static str> {
    let value = value.unwrap_or("mtime_desc");
    MEDIA_SORT_VALUES
        .iter()
        .copied()
        .find(|candidate| *candidate == value)
        .ok_or_else(|| CoreError::bad_request(format!("unsupported media sort: {value}")))
}

fn parse_search_sort(value: Option<&str>) -> ApiResult<&'static str> {
    let value = value.unwrap_or("mtime_desc");
    SEARCH_SORT_VALUES
        .iter()
        .copied()
        .find(|candidate| *candidate == value)
        .ok_or_else(|| CoreError::bad_request(format!("unsupported search sort: {value}")))
}

fn parse_media_kind(value: Option<&str>) -> ApiResult<Option<&'static str>> {
    let Some(value) = value else {
        return Ok(None);
    };
    MEDIA_KIND_VALUES
        .iter()
        .copied()
        .find(|candidate| *candidate == value)
        .map(Some)
        .ok_or_else(|| CoreError::bad_request(format!("unsupported media kind: {value}")))
}

fn map_cursor_error(error: anyhow::Error) -> CoreError {
    let message = error.to_string();
    if message.contains("invalid media cursor")
        || message.contains("invalid folder cursor")
        || message.contains("invalid cursor hex")
    {
        return CoreError::bad_request(message);
    }
    error.into()
}

async fn enqueue_thumbnail_task_wakeup(
    state: &AppState,
    task_id: i64,
    attempt_generation: i64,
    task_priority: i64,
    dedupe_recent: bool,
) -> ApiResult<()> {
    let wakeup_key = thumbnail_task_wakeup_key(state, task_id, attempt_generation, task_priority);
    if dedupe_recent && !claim_thumbnail_task_wakeup(wakeup_key) {
        return Ok(());
    }

    let result = enqueue_task(state, task_id, attempt_generation).await;
    match result {
        Ok(()) => {
            remember_thumbnail_task_wakeup(wakeup_key);
            Ok(())
        }
        Err(error) => {
            forget_thumbnail_task_wakeup(wakeup_key);
            Err(error)
        }
    }
}

fn thumbnail_task_wakeup_key(
    state: &AppState,
    task_id: i64,
    _attempt_generation: i64,
    task_priority: i64,
) -> (usize, i64, i64) {
    (state.writer.instance_id(), task_id, task_priority)
}

fn claim_thumbnail_task_wakeup(key: (usize, i64, i64)) -> bool {
    let now = Instant::now();
    let mut wakeups = RECENT_THUMBNAIL_TASK_WAKEUPS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    wakeups.retain(|_, last_wakeup| {
        now.duration_since(*last_wakeup) <= THUMBNAIL_TASK_WAKE_DEDUPE_WINDOW
    });
    if wakeups.contains_key(&key) {
        return false;
    }
    wakeups.insert(key, now);
    true
}

fn remember_thumbnail_task_wakeup(key: (usize, i64, i64)) {
    let now = Instant::now();
    let mut wakeups = RECENT_THUMBNAIL_TASK_WAKEUPS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    wakeups.insert(key, now);
}

fn forget_thumbnail_task_wakeup(key: (usize, i64, i64)) {
    let mut wakeups = RECENT_THUMBNAIL_TASK_WAKEUPS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    wakeups.remove(&key);
}

fn preview_content_type(path: &FilePath, media_kind: Option<&str>) -> &'static str {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("avif") => "image/avif",
        Some("heic" | "heif") => "image/heic",
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("avi") => "video/x-msvideo",
        _ if media_kind == Some("image") => "application/octet-stream",
        _ if media_kind == Some("video") => "application/octet-stream",
        _ => "application/octet-stream",
    }
}

fn map_task_action_error(error: anyhow::Error) -> CoreError {
    let message = error.to_string();
    if message.contains("task not found") {
        return CoreError::not_found(message);
    }
    if message.contains("not cancellable")
        || message.contains("not retryable")
        || message.contains("root is disabled")
        || message.contains("root not found")
        || message.contains("folder not found")
        || message.contains("media item not found")
        || message.contains("missing root id")
        || message.contains("missing folder id")
        || message.contains("missing file id")
    {
        return CoreError::conflict(message);
    }
    error.into()
}

fn map_tag_error(error: TagError) -> CoreError {
    match error {
        TagError::Duplicate => CoreError::conflict("tag name already exists".to_string()),
        TagError::InvalidName => {
            CoreError::bad_request("tag name must be 1..=64 characters".to_string())
        }
        TagError::InvalidColor => {
            CoreError::bad_request("tag color must be #rrggbb hex when provided".to_string())
        }
        TagError::UnknownTagId(tag_id) => CoreError::not_found(format!("tag not found: {tag_id}")),
        TagError::Other(error) => {
            let message = error.to_string();
            if message.contains("media item not found") {
                CoreError::not_found(message)
            } else {
                error.into()
            }
        }
    }
}

fn map_metadata_error(error: anyhow::Error) -> CoreError {
    let message = error.to_string();
    if message.contains("media item not found") {
        return CoreError::not_found(message);
    }
    error.into()
}

fn map_fsops_error(error: FsOpsError) -> CoreError {
    let status = match error.code {
        FsOpsErrorCode::InvalidName | FsOpsErrorCode::InvalidRequest => StatusCode::BAD_REQUEST,
        FsOpsErrorCode::NotFound => StatusCode::NOT_FOUND,
        FsOpsErrorCode::NameConflict
        | FsOpsErrorCode::CrossRoot
        | FsOpsErrorCode::OutsideRoot
        | FsOpsErrorCode::SymlinkRefused => StatusCode::CONFLICT,
        FsOpsErrorCode::FsError => StatusCode::INTERNAL_SERVER_ERROR,
        FsOpsErrorCode::Interrupted => StatusCode::INTERNAL_SERVER_ERROR,
    };
    CoreError::coded(status, error.code.as_str(), error.message)
}

fn parse_user_metadata_patch(value: &serde_json::Value) -> ApiResult<UserMetadataPatch> {
    let object = value.as_object().ok_or_else(|| {
        CoreError::bad_request("metadata update body must be an object".to_string())
    })?;
    let mut patch = UserMetadataPatch::default();

    if let Some(rating_value) = object.get("rating") {
        let rating = if rating_value.is_null() {
            None
        } else {
            let rating = rating_value.as_i64().ok_or_else(|| {
                CoreError::bad_request("rating must be an integer or null".to_string())
            })?;
            if !(0..=5).contains(&rating) {
                return Err(CoreError::bad_request(format!(
                    "rating must be between 0 and 5: {rating}"
                )));
            }
            Some(rating)
        };
        patch.rating = Some(rating);
    }

    if let Some(favorite_value) = object.get("favorite") {
        let favorite = favorite_value
            .as_bool()
            .ok_or_else(|| CoreError::bad_request("favorite must be a boolean".to_string()))?;
        patch.favorite = Some(favorite);
    }

    if let Some(note_value) = object.get("note") {
        let note = if note_value.is_null() {
            None
        } else {
            let note = note_value
                .as_str()
                .ok_or_else(|| CoreError::bad_request("note must be a string or null".to_string()))?
                .trim()
                .to_string();
            if note.chars().count() > 2048 {
                return Err(CoreError::bad_request(
                    "note must be 2048 characters or fewer".to_string(),
                ));
            }
            if note.is_empty() {
                None
            } else {
                Some(note)
            }
        };
        patch.note = Some(note);
    }

    Ok(patch)
}

async fn enqueue_task(state: &AppState, task_id: i64, attempt_generation: i64) -> ApiResult<()> {
    match state.task_queue.try_send(task_id) {
        Ok(()) => Ok(()),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            // The task row is already durable. If the worker channel is full,
            // returning immediately keeps foreground browsing responsive, but
            // still queue a best-effort async send so the task is not reliant
            // on later incidental drain ticks to start.
            let state = state.clone();
            tokio::spawn(async move {
                if state.task_queue.send(task_id).await.is_err() {
                    let error = anyhow::anyhow!("background task queue is closed");
                    let _ = run_shared_database(state, move |database| {
                        database
                            .mark_task_failed_for_attempt(
                                task_id,
                                attempt_generation,
                                &error.to_string(),
                            )
                            .map_err(CoreError::from)
                    })
                    .await;
                }
            });
            Ok(())
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            let error = anyhow::anyhow!("background task queue is closed");
            let error_message = error.to_string();
            run_shared_database(state.clone(), move |database| {
                database
                    .mark_task_failed_for_attempt(task_id, attempt_generation, &error_message)
                    .map_err(CoreError::from)
            })
            .await?;
            Err(error.into())
        }
    }
}

impl CoreError {
    fn bad_request(message: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message,
            code: None,
        }
    }

    fn conflict(message: String) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message,
            code: None,
        }
    }

    fn not_found(message: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message,
            code: None,
        }
    }

    fn internal(message: String) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message,
            code: None,
        }
    }

    fn coded(status: StatusCode, code: &str, message: String) -> Self {
        Self {
            status,
            message,
            code: Some(code.to_string()),
        }
    }
}

fn fallback_display_name(path: &str) -> String {
    path.trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request, StatusCode};
    use axum::Router;
    use base64::Engine as _;
    use tower::ServiceExt;

    use super::{
        fallback_display_name, notification_outbox_response, resolve_preview_plan, router,
        ErrorResponse, PreviewBlobRecord, PREVIEW_NATIVE_MAX_PX, PREVIEW_NATIVE_PROFILE,
    };
    use crate::api::{
        router_with_config, ApiConfig, AppState, BasicAuthCredentials, NotificationOutboxTestProbe,
        NOTIFICATION_OUTBOX_READ_CONCURRENCY,
    };
    use crate::db::notification_outbox::{
        NotificationOperation, NotificationOutboxEvent, PublishMode,
    };
    use crate::db::{Database, NewRoot, ThumbnailTaskPriority};
    use crate::thumbnails::{PREVIEW_2048_PROFILE, PREVIEW_3072_PROFILE};

    #[test]
    fn fallback_display_name_uses_last_path_component() {
        assert_eq!(fallback_display_name("D:\\Media\\Photos\\"), "Photos");
        assert_eq!(fallback_display_name("D:/Media/Videos"), "Videos");
        assert_eq!(fallback_display_name("D:\\"), "D:");
    }

    #[tokio::test]
    async fn roots_post_queues_scan_and_media_routes_read_after_worker_finishes() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(temp_root.join("photos")).expect("create photos dir");
        fs::write(temp_root.join("photos").join("image.jpg"), b"fake jpg").expect("write image");
        fs::write(temp_root.join("clip.mp4"), b"fake mp4").expect("write video");
        fs::write(temp_root.join("notes.txt"), b"not media").expect("write notes");

        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let (database, worker_database) = test_database_pair(db_dir.join("megle.sqlite"));
        let state = AppState::new_with_worker(database, worker_database);
        let app = router(state.clone());

        let add_root_body = serde_json::json!({
            "path": temp_root.to_string_lossy(),
            "display_name": "Integration Root"
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/roots")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(add_root_body.to_string()))
                    .expect("build request"),
            )
            .await
            .expect("post root");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = response_json(response).await;
        assert_eq!(body["accepted"], true);
        let task_id = body["taskId"].as_i64().expect("task id");
        assert!(body["scan"].is_null());
        let root_id = body["rootId"].as_i64().expect("root id");

        wait_for_task_status(&app, task_id, "succeeded").await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/roots")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list roots");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["items"].as_array().expect("items").len(), 1);
        assert_eq!(body["items"][0]["displayName"], "Integration Root");
        assert_eq!(body["items"][0]["mediaCount"], 2);
        assert!(body["items"][0]["lastScanAt"].as_i64().is_some());
        let root_folder_id = body["items"][0]["rootFolderId"]
            .as_i64()
            .expect("root folder id");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/folders/{root_folder_id}/children"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list folder children");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["items"].as_array().expect("items").len(), 1);
        assert_eq!(body["items"][0]["name"], "photos");
        assert_eq!(body["items"][0]["mediaCount"], 1);
        let photos_folder_id = body["items"][0]["id"].as_i64().expect("photos folder id");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/folders/{photos_folder_id}/ancestors"))
                    .body(Body::empty())
                    .expect("build folder ancestors request"),
            )
            .await
            .expect("list folder ancestors");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(
            body["items"]
                .as_array()
                .expect("ancestor items")
                .iter()
                .map(|item| item["name"].as_str().expect("folder name"))
                .collect::<Vec<_>>(),
            vec!["", "photos"]
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/folders/999999/children")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list missing folder children");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/folders/999999/ancestors")
                    .body(Body::empty())
                    .expect("build missing folder ancestors request"),
            )
            .await
            .expect("missing folder ancestors");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/media?rootId={root_id}&sort=name_asc"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list media");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let items = body["items"].as_array().expect("media items");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "clip.mp4");
        assert_eq!(items[1]["name"], "image.jpg");
        let first_id = items[0]["id"].as_i64().expect("media id");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/media/{first_id}"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("get media");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["id"], first_id);
        assert_eq!(body["name"], "clip.mp4");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/media/999999")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("get missing media");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let _ = fs::remove_dir_all(temp_root);
        drop(app);
        drop(state);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn list_roots_overlays_reachable_from_cache_default_true_when_unprobed() {
        let database = test_database();
        let real_path = std::env::temp_dir().to_string_lossy().to_string();
        let root_id = database
            .add_root(NewRoot {
                path: real_path.clone(),
                display_name: "Reachability Root".to_string(),
            })
            .expect("add root");

        let state = AppState::new(database);
        let app = router(state.clone());

        // No probe has run yet: the cache has no entry for this root id, so it
        // must default to reachable rather than reading as offline.
        let body = get_roots_json(&app).await;
        let item = find_root_json(&body, root_id);
        assert_eq!(item["reachable"], true);

        // A fresh probe of the real (existing) path reports reachable.
        crate::roots::reachability::refresh_root_reachability(
            &state.root_reachability,
            &[(root_id, real_path)],
        );
        let body = get_roots_json(&app).await;
        let item = find_root_json(&body, root_id);
        assert_eq!(item["reachable"], true);

        // Force the cache to report unreachable (simulating an offline probe)
        // and confirm `list_roots` overlays it onto the DB-default `true`.
        {
            let mut guard = state
                .root_reachability
                .lock()
                .expect("lock reachability cache");
            guard.insert(root_id, false);
        }
        let body = get_roots_json(&app).await;
        let item = find_root_json(&body, root_id);
        assert_eq!(item["reachable"], false);
    }

    async fn get_roots_json(app: &axum::Router) -> serde_json::Value {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/roots")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list roots");
        assert_eq!(response.status(), StatusCode::OK);
        response_json(response).await
    }

    fn find_root_json(body: &serde_json::Value, root_id: i64) -> serde_json::Value {
        body["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|item| item["id"].as_i64() == Some(root_id))
            .cloned()
            .expect("seeded root present in list_roots response")
    }

    #[tokio::test]
    async fn thumbnail_route_queues_missing_grid_320_row() {
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let (root_id, file_ids) = seed_media_files_in_one_root(&database, &["a.jpg"]);
        let file_id = file_ids[0];
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let state = AppState {
            writer: crate::db::DbWriter::spawn(database),
            notification_outbox_wake: Arc::new(tokio::sync::Notify::new()),
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::PREVIEW_DECODE_CONCURRENCY,
            )),
            notification_outbox_test_probe: None,
            fsops_database: None,
            task_queue: sender,
            plugins_dir: None,
            read_database_path: None,
            _watcher: None,
            root_reachability: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = router(state.clone());

        // GET /thumbnail on a pending row requests generation through the single
        // writer actor (no lock contention). It returns 202 + the queued state.
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/media/{file_id}/thumbnail?target=grid_320"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("get thumbnail state");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = response_json(response).await;
        assert_eq!(body["fileId"], file_id);
        assert_eq!(body["target"], "grid_320");
        assert_eq!(body["state"], "queued");
        assert_eq!(body["shortSidePx"], 320);
        assert_eq!(body["outputFormat"], "image/webp");
        assert!(body["asset"].is_null());
        let task_id = receiver.try_recv().expect("queued thumbnail task id");
        let _ = root_id;

        let tasks = state
            .writer
            .write(|database| Ok(database.list_tasks()?))
            .await
            .expect("list tasks");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, task_id);
        assert_eq!(tasks[0].kind, "thumbnail");
        assert_eq!(tasks[0].file_id, Some(file_id));

        drop(state);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn preview_route_streams_original_media_bytes() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create preview root");
        let original_bytes = b"not a generated thumbnail";
        let file_name = "original.jpg";
        fs::write(temp_root.join(file_name), original_bytes).expect("write original file");

        let database = test_database();
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "Preview Root".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "preview-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: file_name.to_string(),
                ext: ".jpg".to_string(),
                size: original_bytes.len() as i64,
                mtime: 2,
                ctime: None,
                file_key: None,
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");
        let app = router(AppState::new(database));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/media/{file_id}/preview"))
                    .body(Body::empty())
                    .expect("build preview request"),
            )
            .await
            .expect("get original preview");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/jpeg")
        );
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read original preview body");
        assert_eq!(&bytes[..], original_bytes);

        let missing = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/media/999999/preview")
                    .body(Body::empty())
                    .expect("build missing preview request"),
            )
            .await
            .expect("get missing original preview");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let _ = fs::remove_dir_all(temp_root);
    }

    /// Router + real on-disk source image + a second connection to inspect what
    /// the route persisted. File-backed on purpose: the preview cache lives in
    /// SQLite, so an in-memory database could not be observed independently of
    /// the writer actor.
    struct PreviewHarness {
        app: Router,
        inspector: Database,
        file_id: i64,
        temp_root: PathBuf,
        db_dir: PathBuf,
    }

    impl PreviewHarness {
        fn database(&self) -> &Database {
            &self.inspector
        }

        async fn get(&self, path: &str) -> axum::response::Response {
            self.app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(path)
                        .body(Body::empty())
                        .expect("build preview request"),
                )
                .await
                .expect("preview request")
        }
    }

    impl Drop for PreviewHarness {
        fn drop(&mut self) {
            // Best effort, same as the surrounding tests: the media directory
            // always goes, the sqlite file may still be held by the writer
            // thread and is left for the OS temp sweep.
            let _ = fs::remove_dir_all(&self.temp_root);
            let _ = fs::remove_dir_all(&self.db_dir);
        }
    }

    /// Bytes for the one fixture NO decoder can read. `.raw` is indexed as an
    /// image (the user still expects to see it in the grid) but is claimed by
    /// neither `ImageCrateDecoder` nor `FfmpegDecoder`, so every generation
    /// attempt fails with `NO_DECODER_MARKER`. That is the degradation path the
    /// route has to survive without turning a working 200 into a 500.
    const UNDECODABLE_SOURCE_BYTES: &[u8] =
        b"megle-test-raw-sensor-payload-no-decoder-handles-this";

    /// A JPEG header with no scan data behind it: the file OPENS fine (so the
    /// route reaches the tier path) and then fails to decode, which is what a
    /// truncated download, a half-written file, or a mid-read NAS drop looks
    /// like to the decoder.
    const TRUNCATED_JPEG_BYTES: &[u8] =
        b"\xff\xd8\xff\xe0\x00\x10JFIF\x00 truncated before any scan data";

    async fn preview_harness() -> PreviewHarness {
        preview_harness_with_image("original.jpg", 640, 480).await
    }

    async fn preview_harness_with_large_image() -> PreviewHarness {
        preview_harness_with_image("large.jpg", 5000, 500).await
    }

    async fn preview_harness_with_image(name: &str, width: u32, height: u32) -> PreviewHarness {
        preview_harness_with_source(name, width, height, |path| {
            image::DynamicImage::new_rgb8(width, height)
                .save(path)
                .expect("write preview source image");
        })
        .await
    }

    async fn preview_harness_with_undecodable_source() -> PreviewHarness {
        preview_harness_with_source("camera.raw", 640, 480, |path| {
            fs::write(path, UNDECODABLE_SOURCE_BYTES).expect("write undecodable preview source");
        })
        .await
    }

    /// Indexed as a 16384px JPEG — so the resolver plans a real downscale tier
    /// for a format the browser CAN paint — while the bytes on disk are
    /// undecodable.
    async fn preview_harness_with_undecodable_large_image() -> PreviewHarness {
        preview_harness_with_source("truncated.jpg", 16384, 16384, |path| {
            fs::write(path, TRUNCATED_JPEG_BYTES).expect("write truncated preview source");
        })
        .await
    }

    async fn preview_harness_with_source(
        name: &str,
        width: u32,
        height: u32,
        write_source: impl FnOnce(&std::path::Path),
    ) -> PreviewHarness {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create preview root");
        let source_path = temp_root.join(name);
        write_source(&source_path);
        let size = fs::metadata(&source_path)
            .expect("stat preview source image")
            .len() as i64;

        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let (database, worker_database) = test_database_pair(db_path.clone());
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "Preview Harness Root".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: format!("preview-harness-root-{name}"),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: name.to_string(),
                ext: crate::decode::formats::lowercase_extension(&source_path),
                size,
                mtime: 2,
                ctime: None,
                file_key: None,
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");
        // `source_max_px` is read from the indexed dimensions, exactly as it is
        // in production once the thumbnail pass has filled them in.
        database
            .update_media_dimensions(file_id, width as i64, height as i64)
            .expect("record media dimensions");

        let inspector = Database::open(&db_path).expect("open inspector database");
        let app = router(AppState::new_with_worker(database, worker_database));
        PreviewHarness {
            app,
            inspector,
            file_id,
            temp_root,
            db_dir,
        }
    }

    #[test]
    fn preview_plan_streams_the_original_for_ordinary_browser_images() {
        // The overwhelming majority path: a 3000px JPEG needs no work at all.
        assert!(resolve_preview_plan(Some("image"), ".jpg", 3000, 1920).is_none());
        assert!(resolve_preview_plan(Some("image"), ".png", 4096, 3840).is_none());
        // And a client that reports no viewport gets today's behaviour exactly.
        assert!(resolve_preview_plan(Some("image"), ".jpg", 16384, 0).is_none());
    }

    #[test]
    fn preview_plan_tiers_large_browser_images() {
        let plan = resolve_preview_plan(Some("image"), ".jpg", 16384, 1920)
            .expect("large image should be tiered");
        assert_eq!(plan.profile, PREVIEW_3072_PROFILE);
        assert_eq!(plan.max_px, 3072);
    }

    #[test]
    fn preview_plan_transcodes_unrenderable_images_even_when_small() {
        let plan = resolve_preview_plan(Some("image"), ".heic", 2000, 1920)
            .expect("heic is never browser renderable");
        assert_eq!(plan.profile, PREVIEW_NATIVE_PROFILE);
        assert_eq!(plan.max_px, PREVIEW_NATIVE_MAX_PX);
    }

    #[test]
    fn preview_plan_prefers_the_tier_over_native_for_large_unrenderable_images() {
        let plan = resolve_preview_plan(Some("image"), ".heic", 12000, 1366)
            .expect("large heic should be tiered, not transcoded at native size");
        assert_eq!(plan.profile, PREVIEW_2048_PROFILE);
        assert_eq!(plan.max_px, 2048);
    }

    #[test]
    fn preview_plan_never_touches_video() {
        // Unplayable-video transcode is deliberately out of scope; this is the
        // plug point where that branch will go.
        assert!(resolve_preview_plan(Some("video"), ".mkv", 4000, 1920).is_none());
        assert!(resolve_preview_plan(Some("video"), ".mp4", 16384, 1920).is_none());
        assert!(resolve_preview_plan(None, ".bin", 16384, 1920).is_none());
    }

    #[test]
    fn preview_plan_never_transcodes_without_a_reported_viewport() {
        // The Critical safety property of this pipeline: a client that sends no
        // `viewportLongPx` gets byte-identical behaviour to the pre-pipeline
        // route. `select_preview_profile` already returns `None` for viewport 0,
        // so without an explicit gate control simply falls through into the
        // transcode branch — and every one of these extensions then 500s.
        // `.raw` is claimed by no decoder at all; `.heic`/`.heif`/`.psd` are
        // ffmpeg-only and fail whenever ffmpeg is not on PATH.
        assert!(resolve_preview_plan(Some("image"), ".heic", 2000, 0).is_none());
        assert!(resolve_preview_plan(Some("image"), ".raw", 2000, 0).is_none());
        assert!(resolve_preview_plan(Some("image"), ".heif", 12000, 0).is_none());
        assert!(resolve_preview_plan(Some("image"), ".psd", 640, 0).is_none());
        assert!(resolve_preview_plan(Some("image"), ".tif", 16384, 0).is_none());
        assert!(resolve_preview_plan(Some("image"), ".tiff", 640, 0).is_none());
    }

    #[tokio::test]
    async fn preview_route_streams_original_bytes_when_no_viewport_is_reported() {
        let harness = preview_harness().await;

        let response = harness
            .get(&format!("/api/media/{}/preview", harness.file_id))
            .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-megle-preview-tier")
                .and_then(|value| value.to_str().ok()),
            Some("original")
        );
    }

    #[tokio::test]
    async fn preview_route_still_accepts_the_cache_busting_v_parameter_alone() {
        // `getPreviewBlob` — the untouched "give me the original" client call —
        // sends `?v=<fingerprint>` and nothing else. The newly typed query
        // extractor must keep that exact request on the raw-stream path rather
        // than rejecting it as a malformed query.
        let harness = preview_harness().await;

        let response = harness
            .get(&format!("/api/media/{}/preview?v=42", harness.file_id))
            .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-megle-preview-tier")
                .and_then(|value| value.to_str().ok()),
            Some("original")
        );

        // An unknown query key must stay ignored. `serde` ignores unknown fields
        // by default and every future cache-buster relies on it; pinned here so
        // adding `deny_unknown_fields` later cannot silently 400 the app's own
        // preview requests.
        let unknown_key = harness
            .get(&format!(
                "/api/media/{}/preview?v=42&somethingNew=abc",
                harness.file_id
            ))
            .await;
        assert_eq!(unknown_key.status(), StatusCode::OK);

        // A MALFORMED viewport, by contrast, is a client bug and is reported as
        // one rather than silently coerced. This is the single input-space
        // difference the typed query extractor introduced, so lock it in.
        for malformed in ["abc", "-1", "1.5"] {
            let rejected = harness
                .get(&format!(
                    "/api/media/{}/preview?viewportLongPx={malformed}",
                    harness.file_id
                ))
                .await;
            assert_eq!(
                rejected.status(),
                StatusCode::BAD_REQUEST,
                "viewportLongPx={malformed} must be rejected"
            );
            // ...and rejected as the ErrorResponse JSON the contract declares,
            // not axum's default plain-text rejection body. Asserting only the
            // STATUS is what let that drift ship unnoticed.
            assert_eq!(
                rejected
                    .headers()
                    .get(axum::http::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .map(|value| value.starts_with("application/json")),
                Some(true),
                "viewportLongPx={malformed} rejection must be JSON"
            );
            let body = to_bytes(rejected.into_body(), usize::MAX)
                .await
                .expect("rejection body");
            let parsed: serde_json::Value =
                serde_json::from_slice(&body).expect("rejection body must parse as JSON");
            assert!(
                parsed.get("error").is_some() || parsed.get("message").is_some(),
                "rejection must carry an ErrorResponse shape, got {parsed}"
            );
        }

        // An absurd viewport is CLAMPED, not rejected: select_preview_profile
        // saturates to the largest tier anyway, and a client bug should cost a
        // slightly-wrong tier, never a broken picture.
        let clamped = harness
            .get(&format!(
                "/api/media/{}/preview?viewportLongPx=999999999",
                harness.file_id
            ))
            .await;
        assert_eq!(clamped.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn preview_route_falls_back_to_original_bytes_when_generation_fails() {
        // A `.raw` is indexed as an image and is not browser-renderable, so with
        // a reported viewport the resolver legitimately plans a native
        // transcode — but no decoder claims `.raw`, so generation always fails.
        // The route must DEGRADE to the original bytes instead of 500ing; the
        // same degradation covers a NAS blip or a locked file on an ordinary
        // large JPEG that genuinely earned a tier.
        let harness = preview_harness_with_undecodable_source().await;

        let response = harness
            .get(&format!(
                "/api/media/{}/preview?viewportLongPx=1366",
                harness.file_id
            ))
            .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-megle-preview-tier")
                .and_then(|value| value.to_str().ok()),
            Some("original")
        );
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read degraded preview body");
        assert_eq!(&bytes[..], UNDECODABLE_SOURCE_BYTES);

        // A failed generation must not poison the tier cache with a row the next
        // request would happily serve.
        assert!(harness
            .database()
            .get_preview_blob(harness.file_id, PREVIEW_NATIVE_PROFILE)
            .expect("query")
            .is_none());
    }

    #[tokio::test]
    async fn preview_route_falls_back_to_original_bytes_when_a_tier_cannot_be_generated() {
        // The other half of the degradation, on a path the viewport gate does
        // NOT cover: an ordinary BROWSER-RENDERABLE JPEG, large enough to
        // legitimately earn a downscale tier, whose generation then fails
        // (truncated file, NAS blip, file lock, OOM). It displayed perfectly by
        // streaming the original before this pipeline existed, and it must keep
        // doing so instead of turning into a 500.
        let harness = preview_harness_with_undecodable_large_image().await;

        let response = harness
            .get(&format!(
                "/api/media/{}/preview?viewportLongPx=1366",
                harness.file_id
            ))
            .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-megle-preview-tier")
                .and_then(|value| value.to_str().ok()),
            Some("original")
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/jpeg")
        );
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read degraded preview body");
        assert_eq!(&bytes[..], TRUNCATED_JPEG_BYTES);

        for profile in [PREVIEW_2048_PROFILE, PREVIEW_3072_PROFILE] {
            assert!(
                harness
                    .database()
                    .get_preview_blob(harness.file_id, profile)
                    .expect("query")
                    .is_none(),
                "a failed generation must not cache {profile}"
            );
        }
    }

    #[tokio::test]
    async fn preview_route_streams_an_undecodable_source_without_a_viewport() {
        // The same undecodable file with NO viewport never even plans a
        // transcode, so it takes the untouched raw-stream path.
        let harness = preview_harness_with_undecodable_source().await;

        let response = harness
            .get(&format!("/api/media/{}/preview", harness.file_id))
            .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-megle-preview-tier")
                .and_then(|value| value.to_str().ok()),
            Some("original")
        );
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read original preview body");
        assert_eq!(&bytes[..], UNDECODABLE_SOURCE_BYTES);
    }

    #[tokio::test]
    async fn preview_route_generates_caches_and_reuses_a_tier() {
        let harness = preview_harness_with_large_image().await;
        let path = format!("/api/media/{}/preview?viewportLongPx=1366", harness.file_id);

        let first = harness.get(&path).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(
            first
                .headers()
                .get("x-megle-preview-tier")
                .and_then(|value| value.to_str().ok()),
            Some(PREVIEW_2048_PROFILE)
        );
        assert_eq!(
            first
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/webp")
        );

        // The generated tier is now persisted, and the second request is a
        // pure cache read (no decode).
        let cached = harness
            .database()
            .get_preview_blob(harness.file_id, PREVIEW_2048_PROFILE)
            .expect("query")
            .expect("tier cached");
        assert!(cached.byte_size > 0);
        assert_eq!(cached.width, 2048);

        // Prove "no decode" rather than asserting it in a comment. `updated_at`
        // has whole-second granularity, so a plain before/after comparison would
        // still pass if the tier were regenerated inside the same second. Stamp
        // a sentinel `unix_timestamp()` can never produce and require it to
        // survive: `get_preview_blob` is a plain SELECT that never touches
        // `updated_at`, so only a regenerating upsert can clear it.
        const CACHE_READ_SENTINEL: i64 = 1;
        harness
            .database()
            .upsert_preview_blob(PreviewBlobRecord {
                updated_at: CACHE_READ_SENTINEL,
                ..cached.clone()
            })
            .expect("stamp the cache-read sentinel");

        let second = harness.get(&path).await;
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(
            second
                .headers()
                .get("x-megle-preview-tier")
                .and_then(|value| value.to_str().ok()),
            Some(PREVIEW_2048_PROFILE)
        );
        assert_eq!(
            harness
                .database()
                .get_preview_blob(harness.file_id, PREVIEW_2048_PROFILE)
                .expect("query")
                .expect("tier still cached")
                .updated_at,
            CACHE_READ_SENTINEL,
            "the second request must be a pure cache read, not a regeneration"
        );
    }

    #[tokio::test]
    async fn preview_route_regenerates_a_tier_whose_source_changed() {
        let harness = preview_harness_with_large_image().await;
        let path = format!("/api/media/{}/preview?viewportLongPx=1366", harness.file_id);
        harness.get(&path).await;

        // Poison the cached tier's staleness key, as a real edit would.
        harness
            .database()
            .upsert_preview_blob(PreviewBlobRecord {
                source_size: 1,
                source_mtime: 1,
                ..harness
                    .database()
                    .get_preview_blob(harness.file_id, PREVIEW_2048_PROFILE)
                    .expect("query")
                    .expect("row")
            })
            .expect("poison");

        let response = harness.get(&path).await;
        assert_eq!(response.status(), StatusCode::OK);

        let refreshed = harness
            .database()
            .get_preview_blob(harness.file_id, PREVIEW_2048_PROFILE)
            .expect("query")
            .expect("row");
        assert_ne!(refreshed.source_size, 1, "stale tier must be regenerated");
    }

    #[tokio::test]
    async fn preview_route_transcodes_a_browser_unrenderable_image_at_native_size() {
        // A small TIFF: no tier applies, but Chromium cannot paint it, so the
        // route must transcode. This is also the only test that writes the
        // `preview_native` profile through the route, which pins the profile
        // constants against migration 0025's
        // `CHECK(profile IN ('preview_2048','preview_3072','preview_native'))`.
        let harness = preview_harness_with_image("scanned.tif", 640, 480).await;
        let path = format!("/api/media/{}/preview?viewportLongPx=1366", harness.file_id);

        let response = harness.get(&path).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-megle-preview-tier")
                .and_then(|value| value.to_str().ok()),
            Some(PREVIEW_NATIVE_PROFILE)
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/webp")
        );

        let cached = harness
            .database()
            .get_preview_blob(harness.file_id, PREVIEW_NATIVE_PROFILE)
            .expect("query")
            .expect("native transcode cached");
        assert_eq!(cached.output_format, "image/webp");
        // Never upscaled: the point is format conversion, not resizing.
        assert_eq!(cached.width, 640);
        assert_eq!(cached.height, 480);
    }

    #[tokio::test]
    async fn thumbnail_route_wakes_existing_pending_thumbnail_task() {
        let database = test_database();
        let file_id = seed_media_file(&database);
        let queued_task_id = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("create pending visible thumbnail task")
            .task_id
            .expect("pending task id");
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let state = AppState {
            writer: crate::db::DbWriter::spawn(database),
            notification_outbox_wake: Arc::new(tokio::sync::Notify::new()),
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::PREVIEW_DECODE_CONCURRENCY,
            )),
            notification_outbox_test_probe: None,
            fsops_database: None,
            task_queue: sender,
            plugins_dir: None,
            read_database_path: None,
            _watcher: None,
            root_reachability: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = router(state.clone());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/media/{file_id}/thumbnail?target=grid_320&priority=visible"
                    ))
                    .body(Body::empty())
                    .expect("build wake request"),
            )
            .await
            .expect("wake existing pending thumbnail task");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            receiver
                .try_recv()
                .expect("existing pending task should be woken"),
            queued_task_id
        );

        let repeated = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/media/{file_id}/thumbnail?target=grid_320&priority=visible"
                    ))
                    .body(Body::empty())
                    .expect("build repeated wake request"),
            )
            .await
            .expect("repeat wake existing pending thumbnail task");
        assert_eq!(repeated.status(), StatusCode::ACCEPTED);
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn thumbnail_priority_scope_route_reenqueues_promoted_pending_tasks() {
        let database = test_database();
        let (root_id, file_ids) =
            seed_media_files_in_one_root(&database, &["selected.jpg", "visible.jpg"]);
        let selected_file_id = file_ids[0];
        let visible_file_id = file_ids[1];
        let selected_task_id = database
            .request_thumbnail_task(
                selected_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Background,
            )
            .expect("queue selected file as background")
            .task_id
            .expect("selected task id");
        let visible_task_id = database
            .request_thumbnail_task(
                visible_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Background,
            )
            .expect("queue visible file as background")
            .task_id
            .expect("visible task id");
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let state = AppState {
            writer: crate::db::DbWriter::spawn(database),
            notification_outbox_wake: Arc::new(tokio::sync::Notify::new()),
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::PREVIEW_DECODE_CONCURRENCY,
            )),
            notification_outbox_test_probe: None,
            fsops_database: None,
            task_queue: sender,
            plugins_dir: None,
            read_database_path: None,
            _watcher: None,
            root_reachability: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = router(state.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/thumbnail-priority-scope")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "rootId": root_id,
                            "selectedFileIds": [selected_file_id],
                            "visibleFileIds": [visible_file_id],
                            "aheadFileIds": []
                        })
                        .to_string(),
                    ))
                    .expect("build sync request"),
            )
            .await
            .expect("sync priority scope");
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let mut enqueued = vec![
            receiver
                .try_recv()
                .expect("selected promotion should wake task queue"),
            receiver
                .try_recv()
                .expect("visible promotion should wake task queue"),
        ];
        enqueued.sort();
        let mut expected = vec![selected_task_id, visible_task_id];
        expected.sort();
        assert_eq!(enqueued, expected);
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn thumbnail_route_distinguishes_ready_and_skipped_small_states() {
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let ready_file_id = seed_media_file(&database);
        let skipped_file_id = seed_media_file_named(&database, "icon.png");
        let ready_source_fingerprint = database
            .get_thumbnail_source(ready_file_id)
            .expect("get ready source")
            .expect("ready source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        let skipped_source_fingerprint = database
            .get_thumbnail_source(skipped_file_id)
            .expect("get skipped source")
            .expect("skipped source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        drop(database);

        let conn = rusqlite::Connection::open(&db_path).expect("open seed connection");
        conn.execute(
            "UPDATE media SET width = 640, height = 480, metadata_status = 'ready' WHERE file_id = ?1",
            [ready_file_id],
        )
        .expect("set ready media dimensions");
        conn.execute(
            "UPDATE media SET width = 128, height = 128, metadata_status = 'ready' WHERE file_id = ?1",
            [skipped_file_id],
        )
        .expect("set skipped-small media dimensions");
        conn.execute(
            r#"
            INSERT INTO thumbs(
                file_id, profile, state, cache_key, width, height, byte_size,
                source_fingerprint, updated_at
            )
            VALUES (?1, 'grid_320', 'ready', 'aa/bb/key.webp', 427, 320, 4096, ?2, 10)
            "#,
            (ready_file_id, ready_source_fingerprint),
        )
        .expect("insert ready thumbnail");
        conn.execute(
            r#"
            INSERT INTO thumb_blobs(
                file_id, profile, data, width, height, byte_size, output_format, created_at, updated_at
            )
            VALUES (?1, 'grid_320', x'524946460400000057454250', 427, 320, 4096, 'image/webp', 10, 10)
            "#,
            [ready_file_id],
        )
        .expect("insert ready thumbnail blob");
        conn.execute(
            r#"
            INSERT INTO thumbs(file_id, profile, state, source_fingerprint, updated_at)
            VALUES (?1, 'grid_320', 'skipped_small', ?2, 11)
            "#,
            (skipped_file_id, skipped_source_fingerprint),
        )
        .expect("insert skipped thumbnail");
        drop(conn);

        let database = Database::open(&db_path).expect("reopen database");
        database.apply_migrations().expect("apply migrations");
        let app = router(AppState::new(database));

        let ready_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/media/{ready_file_id}/thumbnail?target=grid_320"
                    ))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("get ready thumbnail state");
        assert_eq!(ready_response.status(), StatusCode::OK);
        let ready = response_json(ready_response).await;
        assert_eq!(ready["state"], "ready");
        assert_eq!(ready["asset"]["width"], 427);
        assert_eq!(ready["asset"]["height"], 320);
        assert_eq!(ready["asset"]["byteSize"], 4096);
        assert!(ready["asset"].get("cacheKey").is_none());
        assert_eq!(ready["servedBy"], "db_blob");

        let skipped_response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/media/{skipped_file_id}/thumbnail?target=grid_320"
                    ))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("get skipped thumbnail state");
        assert_eq!(skipped_response.status(), StatusCode::OK);
        let skipped = response_json(skipped_response).await;
        assert_eq!(skipped["state"], "skipped_small");
        assert_eq!(skipped["target"], "grid_320");
        assert!(skipped["asset"].is_null());

        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn thumbnail_blob_route_reads_from_thumb_blobs() {
        let database = test_database();
        let file_id = seed_media_file(&database);
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(crate::db::ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "ready".to_string(),
                cache_key: None,
                width: Some(2),
                height: Some(1),
                byte_size: Some(12),
                error: None,
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("mark thumbnail ready");
        let blob_bytes = b"RIFF\x04\0\0\0WEBP".to_vec();
        database
            .upsert_thumb_blob(crate::db::ThumbBlobRecord {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                data: blob_bytes.clone(),
                width: 2,
                height: 1,
                byte_size: blob_bytes.len() as i64,
                output_format: crate::thumbnails::GENERATED_FORMAT.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .expect("seed thumb blob");
        let app = router(AppState::new(database));

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/media/{file_id}/thumbnail/blob?target=grid_320"
                    ))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("get thumbnail blob");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/webp"
        );
        assert_eq!(
            response.headers().get("x-megle-served-by").unwrap(),
            "db_blob"
        );
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read blob body");
        assert_eq!(&bytes[..], blob_bytes.as_slice());
    }

    #[tokio::test]
    async fn thumbnail_cache_stats_route_reports_scope_counts() {
        let database = test_database();
        let (root_id, folder_id) = seed_thumbnail_cache_scope_fixture(&database, "cache-stats");
        seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "cached.jpg",
            "ready",
            true,
            false,
        );
        seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "missing.jpg",
            "absent",
            false,
            false,
        );
        seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "stale.jpg",
            "ready",
            true,
            true,
        );
        seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "failed.jpg",
            "failed",
            false,
            false,
        );
        let app = router(AppState::new(database));

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/thumbnails/cache/stats?rootId={root_id}&folderId={folder_id}&includeDescendants=false"
                    ))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("get thumbnail cache stats");

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["cachedCount"], 1);
        assert_eq!(body["missingCount"], 1);
        assert_eq!(body["staleCount"], 1);
        assert_eq!(body["failedCount"], 1);
        assert_eq!(body["failureDetails"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            body["failureDetails"][0]["relativePath"],
            "Pictures cache-stats\\failed.jpg"
        );
    }

    #[tokio::test]
    async fn thumbnail_cache_enqueue_route_queues_bulk_candidates() {
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let (database, inspector) = test_database_pair(db_path);
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-enqueue-route");
        let missing_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "missing.jpg",
            "absent",
            false,
            false,
        );
        let stale_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "stale.jpg",
            "ready",
            true,
            true,
        );
        let app = router(AppState::new(database));

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/thumbnail-cache")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        "{{\"rootId\":{root_id},\"folderId\":{folder_id},\"includeDescendants\":false,\"refreshMode\":\"staleOrMissing\",\"limit\":2}}"
                    )))
                    .expect("build request"),
            )
            .await
            .expect("enqueue thumbnail cache");

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = response_json(response).await;
        assert_eq!(body["acceptedCount"], 2);

        let bulk_file_ids: Vec<i64> = inspector
            .list_tasks()
            .expect("list tasks")
            .into_iter()
            .filter(|task| {
                task.kind == "thumbnail" && task.priority == crate::db::THUMBNAIL_BULK_PRIORITY
            })
            .filter_map(|task| task.file_id)
            .collect();
        assert!(bulk_file_ids.contains(&missing_file_id));
        assert!(bulk_file_ids.contains(&stale_file_id));
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn thumbnail_cache_enqueue_route_respects_include_descendants() {
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let (database, inspector) = test_database_pair(db_path);
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-enqueue-descendants");
        let child_folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: Some(folder_id),
                name: "child".to_string(),
                path_hash: "cache-enqueue-descendants-child".to_string(),
                mtime: Some(11),
            })
            .expect("insert child folder");
        let child_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            child_folder_id,
            "descendant-missing.jpg",
            "absent",
            false,
            false,
        );
        let app = router(AppState::new(database));

        let direct_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/thumbnail-cache")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        "{{\"rootId\":{root_id},\"folderId\":{folder_id},\"includeDescendants\":false,\"refreshMode\":\"staleOrMissing\",\"limit\":4}}"
                    )))
                    .expect("build direct request"),
            )
            .await
            .expect("enqueue direct thumbnail cache");
        assert_eq!(direct_response.status(), StatusCode::ACCEPTED);
        let direct_body = response_json(direct_response).await;
        assert_eq!(direct_body["acceptedCount"], 0);

        let recursive_response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/thumbnail-cache")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        "{{\"rootId\":{root_id},\"folderId\":{folder_id},\"includeDescendants\":true,\"refreshMode\":\"staleOrMissing\",\"limit\":4}}"
                    )))
                    .expect("build recursive request"),
            )
            .await
            .expect("enqueue recursive thumbnail cache");
        assert_eq!(recursive_response.status(), StatusCode::ACCEPTED);
        let recursive_body = response_json(recursive_response).await;
        assert_eq!(recursive_body["acceptedCount"], 1);

        let recursive_task = inspector
            .list_tasks()
            .expect("list recursive tasks")
            .into_iter()
            .find(|task| {
                task.file_id == Some(child_file_id)
                    && task.priority == crate::db::THUMBNAIL_BULK_PRIORITY
            });
        assert!(recursive_task.is_some());
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn thumbnail_cache_enqueue_route_retries_failed_only_when_requested() {
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let (database, inspector) = test_database_pair(db_path);
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-enqueue-retry");
        let failed_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "failed.jpg",
            "failed",
            false,
            false,
        );
        // Task 5: automatic `retryFailedAndStale` only treats `failed` as a
        // candidate when the row is classified `transient`.
        database
            .set_thumbnail_failure_kind_for_test(
                failed_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                "transient",
            )
            .expect("classify seeded failure as transient");
        let app = router(AppState::new(database));

        let missing_only_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/thumbnail-cache")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        "{{\"rootId\":{root_id},\"folderId\":{folder_id},\"includeDescendants\":false,\"refreshMode\":\"staleOrMissing\",\"limit\":4}}"
                    )))
                    .expect("build stale request"),
            )
            .await
            .expect("enqueue stale thumbnail cache");
        assert_eq!(missing_only_response.status(), StatusCode::ACCEPTED);
        let missing_only_body = response_json(missing_only_response).await;
        assert_eq!(missing_only_body["acceptedCount"], 0);

        let retry_response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/thumbnail-cache")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        "{{\"rootId\":{root_id},\"folderId\":{folder_id},\"includeDescendants\":false,\"refreshMode\":\"retryFailedAndStale\",\"limit\":4}}"
                    )))
                    .expect("build retry request"),
            )
            .await
            .expect("enqueue retry thumbnail cache");
        assert_eq!(retry_response.status(), StatusCode::ACCEPTED);
        let retry_body = response_json(retry_response).await;
        assert_eq!(retry_body["acceptedCount"], 1);

        let retry_task = inspector
            .list_tasks()
            .expect("list retry tasks")
            .into_iter()
            .find(|task| {
                task.file_id == Some(failed_file_id)
                    && task.priority == crate::db::THUMBNAIL_BULK_PRIORITY
            });
        assert!(retry_task.is_some());
        let _ = fs::remove_dir_all(db_dir);
    }

    // Gap A: manual "retry failed/stale" must not hammer a root the
    // reachability probe already knows is offline. Covers both sides of the
    // gate added to `enqueue_thumbnail_cache` (routes.rs) — unreachable short-
    // circuits to a zero-accepted response without touching the row, reachable
    // (or an unprobed/miss root) falls through to the existing enqueue.
    #[tokio::test]
    async fn thumbnail_cache_enqueue_route_skips_retry_on_unreachable_root() {
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let (database, inspector) = test_database_pair(db_path);
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-enqueue-unreachable");
        let failed_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "failed.jpg",
            "failed",
            false,
            false,
        );
        database
            .set_thumbnail_failure_kind_for_test(
                failed_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                "transient",
            )
            .expect("classify seeded failure as transient");
        let state = AppState::new(database);
        state
            .root_reachability
            .lock()
            .expect("lock reachability cache")
            .insert(root_id, false);
        let app = router(state.clone());

        // Case 1: root marked unreachable. The handler must short-circuit to
        // an accepted-count-0 response and leave the failed row untouched.
        let unreachable_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/thumbnail-cache")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        "{{\"rootId\":{root_id},\"folderId\":{folder_id},\"includeDescendants\":false,\"refreshMode\":\"retryFailedAndStale\",\"limit\":4}}"
                    )))
                    .expect("build retry request against unreachable root"),
            )
            .await
            .expect("enqueue retry thumbnail cache against unreachable root");
        assert_eq!(unreachable_response.status(), StatusCode::ACCEPTED);
        let unreachable_body = response_json(unreachable_response).await;
        assert_eq!(unreachable_body["acceptedCount"], 0);
        assert_eq!(unreachable_body.get("nextCursor"), None);

        let untouched_thumb = inspector
            .get_thumbnail(failed_file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("read thumb after unreachable retry attempt")
            .expect("thumb row still exists");
        assert_eq!(untouched_thumb.state, "failed");

        let no_retry_task = inspector
            .list_tasks()
            .expect("list tasks after unreachable retry attempt")
            .into_iter()
            .find(|task| task.file_id == Some(failed_file_id));
        assert!(
            no_retry_task.is_none(),
            "unreachable root must not enqueue a retry task"
        );

        // Case 2: root becomes reachable again (mirrors the reachability
        // probe's next 30s tick clearing a transient NAS blip). The same
        // request now falls through to the existing enqueue behavior.
        state
            .root_reachability
            .lock()
            .expect("lock reachability cache")
            .insert(root_id, true);
        let reachable_response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/thumbnail-cache")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        "{{\"rootId\":{root_id},\"folderId\":{folder_id},\"includeDescendants\":false,\"refreshMode\":\"retryFailedAndStale\",\"limit\":4}}"
                    )))
                    .expect("build retry request against reachable root"),
            )
            .await
            .expect("enqueue retry thumbnail cache against reachable root");
        assert_eq!(reachable_response.status(), StatusCode::ACCEPTED);
        let reachable_body = response_json(reachable_response).await;
        assert_eq!(reachable_body["acceptedCount"], 1);

        let retry_task = inspector
            .list_tasks()
            .expect("list tasks after reachable retry attempt")
            .into_iter()
            .find(|task| {
                task.file_id == Some(failed_file_id)
                    && task.priority == crate::db::THUMBNAIL_BULK_PRIORITY
            });
        assert!(retry_task.is_some());
        let _ = fs::remove_dir_all(db_dir);
    }

    // Only `RetryFailedAndStale` is gated by root reachability; a plain
    // `staleOrMissing` request (e.g. the whole-library "generate all" pass,
    // which also carries a concrete `rootId`) must keep enqueuing even while
    // the root is marked unreachable — it never resets an already-failed row,
    // so there is nothing for the gate to protect against there.
    #[tokio::test]
    async fn thumbnail_cache_enqueue_route_stale_mode_ignores_root_reachability_gate() {
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let (database, inspector) = test_database_pair(db_path);
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-enqueue-stale-unreachable");
        let missing_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "missing.jpg",
            "absent",
            false,
            false,
        );
        let state = AppState::new(database);
        state
            .root_reachability
            .lock()
            .expect("lock reachability cache")
            .insert(root_id, false);
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/thumbnail-cache")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        "{{\"rootId\":{root_id},\"folderId\":{folder_id},\"includeDescendants\":false,\"refreshMode\":\"staleOrMissing\",\"limit\":4}}"
                    )))
                    .expect("build stale request against unreachable root"),
            )
            .await
            .expect("enqueue stale thumbnail cache against unreachable root");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = response_json(response).await;
        assert_eq!(body["acceptedCount"], 1);

        let queued_task = inspector
            .list_tasks()
            .expect("list tasks after stale enqueue on unreachable root")
            .into_iter()
            .find(|task| task.file_id == Some(missing_file_id));
        assert!(queued_task.is_some());
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn thumbnail_cache_enqueue_route_rejects_invalid_refresh_mode() {
        let database = test_database();
        let app = router(AppState::new(database));

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/thumbnail-cache")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{\"refreshMode\":\"bogus\"}"))
                    .expect("build invalid request"),
            )
            .await
            .expect("enqueue invalid thumbnail cache");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn thumbnail_cache_enqueue_route_accepts_explicit_file_ids() {
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let (database, inspector) = test_database_pair(db_path);
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-explicit-route");
        let missing_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "missing.jpg",
            "absent",
            false,
            false,
        );
        let app = router(AppState::new(database));

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/thumbnail-cache")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        "{{\"fileIds\":[{missing_file_id}],\"refreshMode\":\"staleOrMissing\",\"limit\":16}}"
                    )))
                    .expect("build explicit file id request"),
            )
            .await
            .expect("enqueue explicit file ids");

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = response_json(response).await;
        assert_eq!(body["acceptedCount"], 1);
        let queued_task = inspector
            .list_tasks()
            .expect("list explicit route tasks")
            .into_iter()
            .find(|task| {
                task.kind == "thumbnail"
                    && task.file_id == Some(missing_file_id)
                    && task.priority == crate::db::THUMBNAIL_BULK_PRIORITY
            });
        assert!(queued_task.is_some());
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn thumbnail_cache_clear_route_clears_blobs_and_resets_states() {
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let (database, inspector) = test_database_pair(db_path);
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-clear-route");
        let file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "clear.jpg",
            "ready",
            true,
            false,
        );
        let app = router(AppState::new(database));

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/thumbnails/cache/clear")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("clear thumbnail cache");

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["cleared"], true);
        assert!(inspector
            .get_thumb_blob(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumb blob")
            .is_none());
        let thumbnail = inspector
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "pending");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn thumbnail_route_reports_ready_blob_metadata_without_cache_key() {
        let database = test_database();
        let file_id = seed_media_file(&database);
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(crate::db::ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "ready".to_string(),
                cache_key: None,
                width: Some(1),
                height: Some(1),
                byte_size: Some(1),
                error: None,
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("mark thumbnail ready");
        let blob_bytes = b"RIFF\x08\0\0\0WEBPblob".to_vec();
        database
            .upsert_thumb_blob(crate::db::ThumbBlobRecord {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                data: blob_bytes,
                width: 427,
                height: 320,
                byte_size: 4096,
                output_format: crate::thumbnails::GENERATED_FORMAT.to_string(),
                created_at: 1,
                updated_at: 2,
            })
            .expect("seed thumb blob");
        let app = router(AppState::new(database));

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/media/{file_id}/thumbnail?target=grid_320"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("get thumbnail state");

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["state"], "ready");
        assert_eq!(body["target"], "grid_320");
        assert_eq!(body["width"], 427);
        assert_eq!(body["height"], 320);
        assert_eq!(body["byteSize"], 4096);
        assert_eq!(body["servedBy"], "db_blob");
        assert_eq!(body["asset"]["width"], 427);
        assert_eq!(body["asset"]["height"], 320);
        assert_eq!(body["asset"]["byteSize"], 4096);
        assert!(body["asset"].get("cacheKey").is_none());
    }

    #[tokio::test]
    async fn thumbnail_route_handles_queued_failed_bad_profile_and_missing_media() {
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let queued_file_id = seed_media_file(&database);
        let failed_current_file_id = seed_media_file_named(&database, "failed-current.jpg");
        let failed_current_source_fingerprint = database
            .get_thumbnail_source(failed_current_file_id)
            .expect("get failed source")
            .expect("failed source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        drop(database);

        let conn = rusqlite::Connection::open(&db_path).expect("open seed connection");
        conn.execute(
            r#"
            INSERT INTO thumbs(file_id, profile, state, cache_key, width, height, byte_size, updated_at)
            VALUES (?1, 'grid_320', 'queued', 'queued/stale.webp', 320, 240, 1024, 20)
            "#,
            [queued_file_id],
        )
        .expect("insert queued thumbnail");
        conn.execute(
            r#"
            INSERT INTO thumbs(
                file_id, profile, state, cache_key, width, height, byte_size,
                error, source_fingerprint, updated_at
            )
            VALUES (?1, 'grid_320', 'failed', 'failed/stale.webp', 320, 240, 1024, 'decode failed', ?2, 21)
            "#,
            (failed_current_file_id, failed_current_source_fingerprint),
        )
        .expect("insert failed thumbnail");
        drop(conn);

        let database = Database::open(&db_path).expect("reopen database");
        database.apply_migrations().expect("apply migrations");
        let app = router(AppState::new(database));

        let queued_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/media/{queued_file_id}/thumbnail?target=grid_320"
                    ))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("get queued thumbnail state");
        assert_eq!(queued_response.status(), StatusCode::ACCEPTED);
        let queued = response_json(queued_response).await;
        assert_eq!(queued["state"], "queued");
        assert!(queued["asset"].is_null());

        let failed_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/media/{failed_current_file_id}/thumbnail?target=grid_320"
                    ))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("get failed thumbnail state");
        assert_eq!(failed_response.status(), StatusCode::OK);
        let failed = response_json(failed_response).await;
        assert_eq!(failed["state"], "failed");
        assert_eq!(failed["error"], "decode failed");
        assert!(failed["asset"].is_null());

        let bad_profile = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/media/{queued_file_id}/thumbnail?target=grid"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("get thumbnail with bad profile");
        assert_eq!(bad_profile.status(), StatusCode::BAD_REQUEST);

        let missing = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/media/999999/thumbnail?target=grid_320")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("get missing thumbnail");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn thumbnail_route_requeues_failed_thumbnail_without_current_source_fingerprint() {
        let database = test_database();
        let null_failed_file_id = seed_media_file_named(&database, "failed-null.jpg");
        let stale_failed_file_id = seed_media_file_named(&database, "failed-stale.jpg");
        database
            .upsert_thumbnail_state(crate::db::ThumbnailStateUpsert {
                file_id: null_failed_file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "failed".to_string(),
                cache_key: None,
                width: None,
                height: None,
                byte_size: None,
                error: Some("decode failed".to_string()),
                source_fingerprint: None,
            })
            .expect("insert null fingerprint failed thumbnail");
        database
            .upsert_thumbnail_state(crate::db::ThumbnailStateUpsert {
                file_id: stale_failed_file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "failed".to_string(),
                cache_key: None,
                width: None,
                height: None,
                byte_size: None,
                error: Some("decode failed".to_string()),
                source_fingerprint: Some(
                    "stale-source-fingerprint-that-must-not-match".to_string(),
                ),
            })
            .expect("insert stale fingerprint failed thumbnail");
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let state = AppState {
            writer: crate::db::DbWriter::spawn(database),
            notification_outbox_wake: Arc::new(tokio::sync::Notify::new()),
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::PREVIEW_DECODE_CONCURRENCY,
            )),
            notification_outbox_test_probe: None,
            fsops_database: None,
            task_queue: sender,
            plugins_dir: None,
            read_database_path: None,
            _watcher: None,
            root_reachability: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = router(state);

        for file_id in [null_failed_file_id, stale_failed_file_id] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(format!("/api/media/{file_id}/thumbnail?target=grid_320"))
                        .body(Body::empty())
                        .expect("build request"),
                )
                .await
                .expect("get failed thumbnail state");
            assert_eq!(response.status(), StatusCode::ACCEPTED);
            let body = response_json(response).await;
            assert_eq!(body["state"], "queued");
            assert!(body["asset"].is_null());
            receiver.try_recv().expect("queued thumbnail task id");
        }
    }

    #[tokio::test]
    async fn tasks_routes_list_queued_tasks_and_enqueue_scan_for_existing_root() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let (database, worker_database) = test_database_pair(db_dir.join("megle.sqlite"));
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "Existing Root".to_string(),
            })
            .expect("add root");
        let state = AppState::new_with_worker(database, worker_database);
        let app = router(state);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/scan")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "rootId": root_id }).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("enqueue scan");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = response_json(response).await;
        assert_eq!(body["accepted"], true);
        let task_id = body["taskId"].as_i64().expect("task id");
        assert_eq!(body["rootId"], root_id);
        assert!(body["scan"].is_null());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/tasks")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list tasks");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let tasks = body["items"].as_array().expect("task items");
        assert!(tasks.iter().any(|task| {
            task["id"] == task_id
                && task["kind"] == "root_scan"
                && task["rootId"] == root_id
                && task["fileId"].is_null()
        }));

        wait_for_task_status(&app, task_id, "succeeded").await;
        let notification_database =
            Database::open(db_dir.join("megle.sqlite")).expect("open notification database");
        let operation = notification_database
            .notification_heads_for_test()
            .expect("read route notification")
            .into_iter()
            .find(|operation| operation.task_id == Some(task_id))
            .expect("scan route notification");
        assert_eq!(operation.origin, "user");
        drop(notification_database);
        let _ = fs::remove_dir_all(temp_root);
        drop(app);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enqueue_scan_route_does_not_wait_for_full_worker_channel() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        let database = test_database();
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "Queued Root".to_string(),
            })
            .expect("add root");
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        sender.try_send(-1).expect("fill worker queue");
        let state = AppState {
            writer: crate::db::DbWriter::spawn(database),
            notification_outbox_wake: Arc::new(tokio::sync::Notify::new()),
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::PREVIEW_DECODE_CONCURRENCY,
            )),
            notification_outbox_test_probe: None,
            fsops_database: None,
            task_queue: sender,
            plugins_dir: None,
            read_database_path: None,
            _watcher: None,
            root_reachability: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = router(state);

        let response = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            app.oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/scan")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "rootId": root_id }).to_string(),
                    ))
                    .expect("build request"),
            ),
        )
        .await;
        drop(receiver);
        let response = response
            .expect("POST /api/tasks/scan should persist the task without waiting for queue space")
            .expect("enqueue scan response");
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[tokio::test]
    async fn tasks_routes_enqueue_interactive_folder_scan_for_existing_folder() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(temp_root.join("selected")).expect("create selected dir");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let (database, worker_database) = test_database_pair(db_dir.join("megle.sqlite"));
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "Interactive Root".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "interactive-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "selected".to_string(),
                path_hash: "interactive-selected".to_string(),
                mtime: Some(2),
            })
            .expect("insert selected folder");
        let state = AppState::new_with_worker(database, worker_database);
        let app = router(state);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/interactive-folder-scan")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "folderId": folder_id }).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("enqueue interactive folder scan");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = response_json(response).await;
        assert_eq!(body["accepted"], true);
        let task_id = body["taskId"].as_i64().expect("task id");
        assert_eq!(body["rootId"], root_id);
        assert!(body["scan"].is_null());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/tasks")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list tasks");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let tasks = body["items"].as_array().expect("task items");
        assert!(tasks.iter().any(|task| {
            task["id"] == task_id
                && task["kind"] == "interactive_folder_scan"
                && task["rootId"] == root_id
                && task["folderId"] == folder_id
                && task["fileId"].is_null()
        }));

        let _ = fs::remove_dir_all(temp_root);
        drop(app);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn worker_startup_processes_persisted_pending_root_scan_task() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("image.jpg"), b"fake jpg").expect("write image");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");

        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "Recovered Pending Root".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        drop(database);

        let database = Database::open(&db_path).expect("reopen database");
        let worker_database = Database::open(&db_path).expect("reopen worker database");
        let state = AppState::new_with_worker(database, worker_database);
        let app = router(state);

        wait_for_task_status(&app, task_id, "succeeded").await;
        wait_for_media_count(&app, root_id, 1).await;

        drop(app);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn root_scan_task_lists_final_progress_counters_after_worker_completes() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(temp_root.join("photos")).expect("create photos dir");
        fs::write(temp_root.join("photos").join("image.jpg"), b"fake jpg").expect("write image");
        fs::write(temp_root.join("clip.mp4"), b"fake mp4").expect("write video");
        fs::write(temp_root.join("notes.txt"), b"not media").expect("write skipped file");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let (database, worker_database) = test_database_pair(db_dir.join("megle.sqlite"));
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "Progress Root".to_string(),
            })
            .expect("add root");
        let state = AppState::new_with_worker(database, worker_database);
        let app = router(state);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/scan")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "rootId": root_id }).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("enqueue scan");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let task_id = response_json(response).await["taskId"]
            .as_i64()
            .expect("task id");

        let task = wait_for_task_status(&app, task_id, "succeeded").await;
        assert_eq!(task["itemsSeen"], 4);
        assert!(task["itemsTotal"].is_null());
        assert_eq!(task["foldersSeen"], 2);
        assert_eq!(task["mediaFilesSeen"], 2);
        assert_eq!(task["skippedFiles"], 1);

        drop(app);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        fs::remove_dir_all(temp_root).expect("cleanup temp root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn worker_startup_resets_and_processes_stale_running_root_scan_task() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("image.jpg"), b"fake jpg").expect("write image");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");

        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "Recovered Running Root".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark task running");
        drop(database);

        let database = Database::open(&db_path).expect("reopen database");
        let worker_database = Database::open(&db_path).expect("reopen worker database");
        let state = AppState::new_with_worker(database, worker_database);
        let app = router(state);

        wait_for_task_status(&app, task_id, "succeeded").await;
        wait_for_media_count(&app, root_id, 1).await;

        drop(app);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        fs::remove_dir_all(temp_root).expect("cleanup temp root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn tasks_scan_returns_not_found_for_missing_root() {
        let app = router(AppState::new(test_database()));

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/scan")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "rootId": 999999 }).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("enqueue missing scan");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response_json(response).await;
        assert!(body["error"]
            .as_str()
            .expect("error")
            .contains("root not found"));
    }

    #[tokio::test]
    async fn media_and_folder_routes_return_bad_request_for_invalid_cursors() {
        let database = test_database();
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root-hash".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let app = router(AppState::new(database));

        for uri in [
            "/api/media?cursor=not-a-cursor".to_string(),
            "/api/media?cursor=v1:media:mtime_desc:not-an-id:123".to_string(),
            "/api/media?cursor=v1:media:mtime_desc:1:not-a-mtime".to_string(),
            "/api/media?sort=name_asc&cursor=v1:media:name_asc:1:c3".to_string(),
            format!("/api/folders/{root_folder_id}/children?cursor=not-a-cursor"),
            format!("/api/folders/{root_folder_id}/children?cursor=v1:folder:not-an-id:616263"),
            format!("/api/folders/{root_folder_id}/children?cursor=v1:folder:1:c3"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(uri)
                        .body(Body::empty())
                        .expect("build request"),
                )
                .await
                .expect("request with invalid cursor");
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn media_route_returns_bad_request_for_unknown_sort_or_kind() {
        let app = router(AppState::new(test_database()));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/media?sort=size_asc")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list media with bad sort");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/media?kind=document")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list media with bad kind");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn roots_post_rejects_nonexistent_path_without_creating_task() {
        let state = AppState::new(test_database());
        let app = router(state.clone());
        let missing_path = unique_temp_dir().join("missing");

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/roots")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "path": missing_path }).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("post missing root");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let (roots, tasks) = state
            .writer
            .write(|database| Ok((database.list_roots(), database.list_tasks())))
            .await
            .expect("read state for assertion");
        assert!(roots.expect("list roots").is_empty());
        assert!(tasks.expect("list tasks").is_empty());
    }

    #[tokio::test]
    async fn remove_root_marks_pending_delete_and_enqueues_purge() {
        let database = test_database();
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: "D:/purge-route".to_string(),
                display_name: "Purge".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "h".to_string(),
                mtime: Some(1),
            })
            .expect("folder");
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: "a.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 1,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .expect("file");
        database.upsert_media_kind(file_id, "image").expect("media");
        // No worker: build an AppState whose task_queue channel is never
        // drained (the receiver is held but never `.recv()`'d), so the
        // enqueued root_purge task is created and accepted but never run.
        // Mirrors `enqueue_scan_route_does_not_wait_for_full_worker_channel`'s
        // manual AppState construction above. `AppState::new` can't be used
        // here: for an in-memory database it drops the receiver immediately,
        // which closes the channel and makes enqueue fail hard.
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let state = AppState {
            writer: crate::db::DbWriter::spawn(database),
            notification_outbox_wake: Arc::new(tokio::sync::Notify::new()),
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::PREVIEW_DECODE_CONCURRENCY,
            )),
            notification_outbox_test_probe: None,
            fsops_database: None,
            task_queue: sender,
            plugins_dir: None,
            read_database_path: None,
            _watcher: None,
            root_reachability: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = router(state);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/roots/{root_id}"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("delete root");
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        // Root hidden from GET /api/roots immediately.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/roots")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list roots");
        let body = response_json(response).await;
        assert!(
            body["items"].as_array().expect("items").is_empty(),
            "root hidden"
        );

        // A root_purge task was enqueued; files are still present (async purge
        // was never run — there is no worker in this test's AppState).
        let tasks = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/api/tasks")
                        .body(Body::empty())
                        .expect("build request"),
                )
                .await
                .expect("list tasks"),
        )
        .await;
        let has_purge = tasks["items"]
            .as_array()
            .expect("task items")
            .iter()
            .any(|task| task["kind"] == "root_purge");
        assert!(has_purge, "root_purge task enqueued");

        // DELETE of a missing root -> 404.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/roots/999999")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("delete missing");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn add_root_rejects_path_pending_delete_with_conflict() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        let database = test_database();
        // No worker: build an AppState whose task_queue channel is never
        // drained, mirroring `remove_root_marks_pending_delete_and_enqueues_purge`
        // above, so the enqueued tasks are created and accepted but never run
        // and the removed root stays pending-delete for the duration of the test.
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let state = AppState {
            writer: crate::db::DbWriter::spawn(database),
            notification_outbox_wake: Arc::new(tokio::sync::Notify::new()),
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::PREVIEW_DECODE_CONCURRENCY,
            )),
            notification_outbox_test_probe: None,
            fsops_database: None,
            task_queue: sender,
            plugins_dir: None,
            read_database_path: None,
            _watcher: None,
            root_reachability: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = router(state);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/roots")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "path": temp_root }).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("post root");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = response_json(response).await;
        let root_id = body["rootId"].as_i64().expect("rootId");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/roots/{root_id}"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("delete root");
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        // Re-adding the same path while the root is mid-purge -> 409, not a
        // resurrected root.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/roots")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "path": temp_root }).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("post root again while pending-delete");
        assert_eq!(response.status(), StatusCode::CONFLICT);

        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[tokio::test]
    async fn remove_root_hides_root_media_and_rejects_future_scans() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("image.jpg"), b"fake jpg").expect("write image");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let (database, worker_database) = test_database_pair(db_dir.join("megle.sqlite"));
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "Remove Root".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        let state = AppState::new_with_worker(database, worker_database);
        let app = router(state);
        wait_for_task_status(&app, task_id, "succeeded").await;
        wait_for_media_count(&app, root_id, 1).await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/roots/{root_id}"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("delete root");
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/roots")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list roots");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert!(body["items"].as_array().expect("root items").is_empty());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/media?rootId={root_id}&sort=name_asc"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list media");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert!(body["items"].as_array().expect("media items").is_empty());

        // `DELETE /api/roots/{id}` now hard-deletes: it marks the root
        // pending-delete and enqueues a `root_purge` task (Task 7) that a real
        // worker (this test uses `AppState::new_with_worker`) drains in the
        // background. Find that task and wait for it to actually finish
        // before asserting the scan-enqueue status below, otherwise this
        // assertion races the purge worker. The task self-deletes on
        // completion instead of reaching a terminal status (see tasks.rs:
        // "the final batch deleted this task's own row + the roots row"), so
        // wait for its row to disappear rather than for a "succeeded" status
        // it will never reach.
        let tasks_after_delete = response_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/api/tasks")
                        .body(Body::empty())
                        .expect("build request"),
                )
                .await
                .expect("list tasks after delete"),
        )
        .await;
        let purge_task_id = tasks_after_delete["items"]
            .as_array()
            .expect("task items")
            .iter()
            .find(|task| task["kind"] == "root_purge" && task["rootId"] == root_id)
            .and_then(|task| task["id"].as_i64())
            .expect("root_purge task enqueued for removed root");
        wait_for_task_absent(&app, purge_task_id).await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/scan")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "rootId": root_id }).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("enqueue scan for removed root");
        // Once the purge worker finishes, the `roots` row itself is gone (not
        // merely disabled), so `enqueue_scan`'s `get_root` lookup fails and
        // the route now returns 404 NOT_FOUND instead of the old soft-delete
        // behavior's 409 CONFLICT (root exists but disabled).
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        drop(app);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        fs::remove_dir_all(temp_root).expect("cleanup temp root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn disabled_pending_root_scan_recovered_at_startup_fails_without_scanning() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("image.jpg"), b"fake jpg").expect("write image");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");

        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "Disabled Pending Root".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        database.disable_root(root_id).expect("disable root");
        drop(database);

        let database = Database::open(&db_path).expect("reopen database");
        let worker_database = Database::open(&db_path).expect("reopen worker database");
        let state = AppState::new_with_worker(database, worker_database);
        let app = router(state.clone());

        let task = wait_for_task_status(&app, task_id, "failed").await;
        assert!(task["error"]
            .as_str()
            .expect("task error")
            .contains("disabled"));

        let file_count: i64 = rusqlite::Connection::open(&db_path)
            .expect("open verification database")
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .expect("count files");
        assert_eq!(file_count, 0);

        drop(app);
        drop(state);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        fs::remove_dir_all(temp_root).expect("cleanup temp root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn enqueue_failure_marks_created_task_failed() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        let database = test_database();
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "Existing Root".to_string(),
            })
            .expect("add root");
        let (_sender, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        let state = AppState {
            writer: crate::db::DbWriter::spawn(database),
            notification_outbox_wake: Arc::new(tokio::sync::Notify::new()),
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::PREVIEW_DECODE_CONCURRENCY,
            )),
            notification_outbox_test_probe: None,
            fsops_database: None,
            task_queue: _sender,
            plugins_dir: None,
            read_database_path: None,
            _watcher: None,
            root_reachability: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = router(state.clone());

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/scan")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "rootId": root_id }).to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("enqueue scan");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let tasks = state
            .writer
            .write(|database| Ok(database.list_tasks()?))
            .await
            .expect("list tasks");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, "failed");
        assert!(tasks[0]
            .error
            .as_deref()
            .expect("task error")
            .contains("background task queue is closed"));
        fs::remove_dir_all(temp_root).expect("cleanup temp root");
    }

    #[tokio::test]
    async fn task_cancel_and_retry_routes_update_task_and_enqueue_retry() {
        let database = test_database();
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: "D:/Pictures/Action Root".to_string(),
                display_name: "Action Root".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create task");
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let state = AppState {
            writer: crate::db::DbWriter::spawn(database),
            notification_outbox_wake: Arc::new(tokio::sync::Notify::new()),
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::PREVIEW_DECODE_CONCURRENCY,
            )),
            notification_outbox_test_probe: None,
            fsops_database: None,
            task_queue: sender,
            plugins_dir: None,
            read_database_path: None,
            _watcher: None,
            root_reachability: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = router(state.clone());

        let cancel_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{task_id}/cancel"))
                    .body(Body::empty())
                    .expect("build cancel request"),
            )
            .await
            .expect("cancel task");
        assert_eq!(cancel_response.status(), StatusCode::OK);
        let cancel_body = response_json(cancel_response).await;
        assert_eq!(cancel_body["accepted"], true);
        assert_eq!(cancel_body["taskId"], task_id);
        assert!(receiver.try_recv().is_err());

        let retry_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{task_id}/retry"))
                    .body(Body::empty())
                    .expect("build retry request"),
            )
            .await
            .expect("retry task");
        assert_eq!(retry_response.status(), StatusCode::ACCEPTED);
        let retry_body = response_json(retry_response).await;
        assert_eq!(retry_body["accepted"], true);
        assert_eq!(retry_body["taskId"], task_id);
        assert_eq!(retry_body["rootId"], root_id);
        assert_eq!(receiver.try_recv().expect("retry enqueued task"), task_id);

        let task = state
            .writer
            .write(move |database| {
                database
                    .get_task(task_id)?
                    .ok_or_else(|| anyhow::anyhow!("task exists"))
            })
            .await
            .expect("get task");
        assert_eq!(task.status, "pending");
    }

    #[tokio::test]
    async fn task_action_routes_return_not_found_and_conflict_for_invalid_actions() {
        let database = test_database();
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: "D:/Pictures/Finished Root".to_string(),
                display_name: "Finished Root".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create task");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark running");
        database
            .mark_task_succeeded_current_attempt_for_test(task_id)
            .expect("mark succeeded");
        let state = AppState::new(database);
        let app = router(state);

        let missing_cancel = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tasks/999999/cancel")
                    .body(Body::empty())
                    .expect("build missing cancel request"),
            )
            .await
            .expect("cancel missing task");
        assert_eq!(missing_cancel.status(), StatusCode::NOT_FOUND);

        let retry_finished = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{task_id}/retry"))
                    .body(Body::empty())
                    .expect("build retry finished request"),
            )
            .await
            .expect("retry finished task");
        assert_eq!(retry_finished.status(), StatusCode::CONFLICT);

        let cancel_finished = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/tasks/{task_id}/cancel"))
                    .body(Body::empty())
                    .expect("build cancel finished request"),
            )
            .await
            .expect("cancel finished task");
        assert_eq!(cancel_finished.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn configured_session_token_is_required_for_api_routes() {
        let app = router_with_config(
            test_database(),
            ApiConfig {
                session_token: Some("test-session".to_string()),
                allowed_origin: None,
                ..ApiConfig::default()
            },
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/health")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("missing token request");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/health")
                    .header("x-megle-session", "wrong-session")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("wrong token request");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/health")
                    .header("x-megle-session", "test-session")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("correct token request");
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// Empty-string env vars (e.g. `${MEGLE_SESSION_TOKEN:-}` from
    /// `compose.yaml`) used to be threaded into `ApiConfig.session_token` as
    /// `Some("")`, which made the auth middleware compare every incoming
    /// request to the empty string and reject `/api/health` with 401. Startup
    /// in `main.rs` now filters empty strings out of the optional credential
    /// vars so they collapse to `None`. This test documents the resulting
    /// behavior: with an unconfigured token the API (and `/api/health`) is
    /// reachable without any session header.
    #[tokio::test]
    async fn empty_string_session_token_does_not_reject_health() {
        // What `main.rs` produces from `MEGLE_SESSION_TOKEN=""`: filtered to
        // `None`, identical to the var being unset.
        let session_token = Some(String::new()).filter(|s: &String| !s.is_empty());
        assert!(
            session_token.is_none(),
            "empty string must collapse to None"
        );

        let app = router_with_config(
            test_database(),
            ApiConfig {
                session_token,
                ..ApiConfig::default()
            },
        );

        // No `X-Megle-Session` header at all should now succeed.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/health")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("missing header request");
        assert_eq!(response.status(), StatusCode::OK);

        // An empty header value should also succeed since no token is
        // configured.
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/health")
                    .header("x-megle-session", "")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("empty header request");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn configured_cors_allows_only_the_exact_development_origin() {
        let app = router_with_config(
            test_database(),
            ApiConfig {
                session_token: Some("test-session".to_string()),
                allowed_origin: Some("http://127.0.0.1:5173".parse().expect("origin")),
                ..ApiConfig::default()
            },
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/media/1/metadata")
                    .header(header::ORIGIN, "http://127.0.0.1:5173")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PUT")
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        "content-type,x-megle-session",
                    )
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("cors preflight");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&"http://127.0.0.1:5173".parse().expect("origin header"))
        );
        let allow_headers = response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .and_then(|value| value.to_str().ok())
            .expect("allow headers")
            .to_ascii_lowercase();
        assert!(allow_headers.contains("content-type"));
        assert!(allow_headers.contains("x-megle-session"));
        let allow_methods = response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_METHODS)
            .and_then(|value| value.to_str().ok())
            .expect("allow methods")
            .to_ascii_uppercase();
        assert!(allow_methods.contains("PUT"));
        assert_ne!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&"*".parse().expect("wildcard header"))
        );

        // `Access-Control-Expose-Headers` rides on the ACTUAL response, not the
        // preflight, so this needs a real request.
        //
        // The renderer and Core sit on different loopback PORTS, so every
        // browser request is cross-origin and JS can read only CORS-safelisted
        // response headers unless they are explicitly exposed. Without this the
        // preview client reads `x-megle-preview-tier` as null, falls back to
        // "original", and silently never requests the sharpening upgrade — a
        // >4096px image stays at tier resolution however far you zoom. Caught
        // only by driving the real app; no unit test saw it.
        let actual = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/health")
                    .header(header::ORIGIN, "http://127.0.0.1:5173")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("cors actual request");
        let expose_headers = actual
            .headers()
            .get(header::ACCESS_CONTROL_EXPOSE_HEADERS)
            .and_then(|value| value.to_str().ok())
            .expect("expose headers")
            .to_ascii_lowercase();
        assert!(
            expose_headers.contains("x-megle-preview-tier"),
            "preview tier header must be readable cross-origin, got: {expose_headers}"
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/health")
                    .header(header::ORIGIN, "http://127.0.0.1:5174")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "x-megle-session")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("disallowed cors preflight");
        assert_ne!(
            response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&"http://127.0.0.1:5174".parse().expect("origin header"))
        );
    }

    #[tokio::test]
    async fn create_tag_returns_201_then_409_on_duplicate_and_400_on_empty() {
        let app = router(AppState::new(test_database()));

        let create = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tags")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "name": "Vacation", "color": "#aabbcc" }).to_string(),
                    ))
                    .expect("build create request"),
            )
            .await
            .expect("create tag");
        assert_eq!(create.status(), StatusCode::CREATED);
        let body = response_json(create).await;
        assert_eq!(body["name"], "Vacation");
        assert_eq!(body["color"], "#aabbcc");

        let duplicate = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tags")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "name": "Vacation" }).to_string(),
                    ))
                    .expect("build duplicate request"),
            )
            .await
            .expect("duplicate tag");
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);

        let invalid = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tags")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::json!({ "name": "   " }).to_string()))
                    .expect("build invalid request"),
            )
            .await
            .expect("invalid tag");
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

        let bad_color = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/tags")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "name": "Other", "color": "blue" }).to_string(),
                    ))
                    .expect("build bad color request"),
            )
            .await
            .expect("invalid color");
        assert_eq!(bad_color.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn update_user_metadata_validates_rating_and_returns_404_for_missing_file() {
        let database = test_database();
        let file_id = seed_media_file(&database);
        let app = router(AppState::new(database));

        let valid = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/media/{file_id}/metadata"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "rating": 4,
                            "favorite": true,
                            "note": "trip notes"
                        })
                        .to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("update metadata");
        assert_eq!(valid.status(), StatusCode::OK);
        let body = response_json(valid).await;
        assert_eq!(body["fileId"], file_id);
        assert_eq!(body["rating"], 4);
        assert_eq!(body["favorite"], true);
        assert_eq!(body["note"], "trip notes");

        let invalid = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/media/{file_id}/metadata"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::json!({ "rating": 6 }).to_string()))
                    .expect("build invalid request"),
            )
            .await
            .expect("invalid rating");
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

        let missing = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/media/999999/metadata")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "favorite": true }).to_string(),
                    ))
                    .expect("build missing request"),
            )
            .await
            .expect("missing file");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn add_file_tag_returns_404_for_unknown_tag_and_200_for_existing() {
        let database = test_database();
        let file_id = seed_media_file(&database);
        let tag = database.create_tag("trip", None).expect("create tag");
        let app = router(AppState::new(database));

        let unknown = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/media/{file_id}/tags"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "tagId": 99999 }).to_string(),
                    ))
                    .expect("build unknown tag request"),
            )
            .await
            .expect("unknown tag");
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

        let added = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/media/{file_id}/tags"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "tagId": tag.id }).to_string(),
                    ))
                    .expect("build add tag request"),
            )
            .await
            .expect("add tag");
        assert_eq!(added.status(), StatusCode::OK);
        let body = response_json(added).await;
        assert_eq!(body["fileId"], file_id);
        assert_eq!(body["tagIds"], serde_json::json!([tag.id]));
    }

    #[tokio::test]
    async fn search_route_filters_by_kind_and_query_and_respects_rating_desc_null_last() {
        let database = test_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Search Route Root".to_string(),
                display_name: "Search Route".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "search-route".to_string(),
                mtime: Some(1),
            })
            .expect("seed folder");

        let mut file_ids = Vec::new();
        for (name, ext, mtime, kind) in [
            ("foo image.jpg", ".jpg", 100, "image"),
            ("bar image.jpg", ".jpg", 200, "image"),
            ("foo clip.mp4", ".mp4", 300, "video"),
        ] {
            let id = database
                .upsert_file(crate::db::FileUpsert {
                    root_id,
                    folder_id,
                    name: name.to_string(),
                    ext: ext.to_string(),
                    size: 100,
                    mtime,
                    ctime: None,
                    file_key: None,
                })
                .expect("insert file");
            database
                .upsert_media_kind(id, kind)
                .expect("upsert media kind");
            file_ids.push(id);
        }
        // Set ratings so rating_desc has a deterministic ordering.
        database
            .upsert_user_metadata_partial(
                file_ids[1],
                crate::db::UserMetadataPatch {
                    rating: Some(Some(5)),
                    favorite: None,
                    note: None,
                },
            )
            .expect("rate bar image");
        database
            .upsert_user_metadata_partial(
                file_ids[0],
                crate::db::UserMetadataPatch {
                    rating: Some(Some(2)),
                    favorite: None,
                    note: None,
                },
            )
            .expect("rate foo image");
        let app = router(AppState::new(database));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/search?q=foo&kind=image&rootId={root_id}"))
                    .body(Body::empty())
                    .expect("build search request"),
            )
            .await
            .expect("search request");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let items = body["items"].as_array().expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], file_ids[0]);
        assert_eq!(items[0]["name"], "foo image.jpg");

        let rated = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/search?rootId={root_id}&sort=rating_desc"))
                    .body(Body::empty())
                    .expect("build rating request"),
            )
            .await
            .expect("rating search request");
        assert_eq!(rated.status(), StatusCode::OK);
        let body = response_json(rated).await;
        let items = body["items"].as_array().expect("rated items");
        assert_eq!(items.len(), 3);
        // First two are rated 5, then 2.
        assert_eq!(items[0]["id"], file_ids[1]);
        assert_eq!(items[1]["id"], file_ids[0]);
        // The unrated video comes last.
        assert_eq!(items[2]["id"], file_ids[2]);
        assert!(items[2]["rating"].is_null());
    }

    #[tokio::test]
    async fn search_with_multiple_tag_ids_filters_by_and() {
        // The search route must collect repeated `tagId` query parameters
        // into a Vec and AND them together. Stock `axum::extract::Query`
        // collapses duplicate keys, so we use `axum_extra`'s Query for
        // this handler. This regression test exercises that wiring with
        // two `tagId` values.
        let database = test_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Search Tags Root".to_string(),
                display_name: "Search Tags".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "tags-route".to_string(),
                mtime: Some(1),
            })
            .expect("seed folder");

        let mut file_ids = Vec::new();
        for (name, mtime) in [("alpha.jpg", 100), ("beta.jpg", 200), ("gamma.jpg", 300)] {
            let id = database
                .upsert_file(crate::db::FileUpsert {
                    root_id,
                    folder_id,
                    name: name.to_string(),
                    ext: ".jpg".to_string(),
                    size: 100,
                    mtime,
                    ctime: None,
                    file_key: None,
                })
                .expect("insert file");
            database
                .upsert_media_kind(id, "image")
                .expect("upsert media kind");
            file_ids.push(id);
        }
        let red = database.create_tag("red", None).expect("red");
        let blue = database.create_tag("blue", None).expect("blue");
        // alpha: red+blue, beta: red only, gamma: blue only.
        database
            .set_file_tags(file_ids[0], &[red.id, blue.id])
            .expect("alpha tags");
        database
            .set_file_tags(file_ids[1], &[red.id])
            .expect("beta tags");
        database
            .set_file_tags(file_ids[2], &[blue.id])
            .expect("gamma tags");

        let app = router(AppState::new(database));
        let uri = format!(
            "/api/search?rootId={root_id}&tagId={}&tagId={}",
            red.id, blue.id
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("search");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let items = body["items"].as_array().expect("items");
        assert_eq!(
            items.len(),
            1,
            "expected only alpha to satisfy red AND blue, got {items:?}"
        );
        assert_eq!(items[0]["id"], file_ids[0]);
        assert_eq!(items[0]["name"], "alpha.jpg");
    }

    #[tokio::test]
    async fn file_ops_rename_route_returns_record_and_404_400_409() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("alpha.jpg"), b"a").expect("write alpha");
        fs::write(temp_root.join("beta.jpg"), b"b").expect("write beta");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let database = Database::open(db_dir.join("megle.sqlite")).expect("open db");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "ops".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "ops-root".to_string(),
                mtime: Some(1),
            })
            .expect("upsert folder");
        let alpha_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: "alpha.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 1,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .expect("upsert alpha");
        let _beta_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: "beta.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 1,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .expect("upsert beta");
        let app = router(AppState::new(database));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/file-ops/rename")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"fileId": alpha_id, "newName": "alpha2.jpg"})
                            .to_string(),
                    ))
                    .expect("build request"),
            )
            .await
            .expect("rename");
        let status = response.status();
        let body = response_json(response).await;
        assert_eq!(status, StatusCode::OK, "rename response: {body}");
        assert_eq!(body["operation"], "rename");
        assert_eq!(body["status"], "succeeded");

        let conflict = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/file-ops/rename")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"fileId": alpha_id, "newName": "beta.jpg"}).to_string(),
                    ))
                    .expect("build conflict request"),
            )
            .await
            .expect("rename conflict");
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        let body = response_json(conflict).await;
        assert_eq!(body["code"], "name_conflict");

        let invalid = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/file-ops/rename")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"fileId": alpha_id, "newName": "with/slash.jpg"})
                            .to_string(),
                    ))
                    .expect("build invalid request"),
            )
            .await
            .expect("rename invalid");
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/file-ops/rename")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"fileId": 999_999, "newName": "x.jpg"}).to_string(),
                    ))
                    .expect("build missing request"),
            )
            .await
            .expect("rename missing");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        drop(app);
        let _ = fs::remove_dir_all(&temp_root);
        let _ = fs::remove_dir_all(&db_dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocked_file_operation_requests_do_not_block_tokio_workers() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("source.jpg"), b"source").expect("write source");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let database = Database::open(db_dir.join("megle.sqlite")).expect("open db");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "async ops".to_owned(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "async-ops-root".to_owned(),
                mtime: Some(1),
            })
            .expect("upsert folder");
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: "source.jpg".to_owned(),
                ext: ".jpg".to_owned(),
                size: 6,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .expect("upsert file");
        let state = AppState::new(database);
        let fsops_database = state
            .fsops_database
            .as_ref()
            .expect("file-backed fsops database")
            .clone();
        let (locked_tx, locked_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let lock_holder = std::thread::spawn(move || {
            let _guard = fsops_database.lock().expect("hold fsops lock");
            locked_tx.send(()).expect("announce held lock");
            release_rx.recv().expect("release held lock");
        });
        locked_rx.recv().expect("wait for held lock");
        let app = router(state);
        let mut requests = Vec::new();
        for index in 0..4 {
            let app = app.clone();
            requests.push(tokio::spawn(async move {
                app.oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/file-ops/rename")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::json!({
                                "fileId": file_id,
                                "newName": format!("target-{index}.jpg")
                            })
                            .to_string(),
                        ))
                        .expect("build blocked request"),
                )
                .await
            }));
        }
        let watchdog = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(600));
            let _ = release_tx.send(());
        });

        let heartbeat_started = std::time::Instant::now();
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        assert!(
            heartbeat_started.elapsed() < std::time::Duration::from_millis(250),
            "file-operation handlers blocked every Tokio worker"
        );

        for request in requests {
            request
                .await
                .expect("request task")
                .expect("route response");
        }
        watchdog.join().expect("watchdog thread");
        lock_holder.join().expect("lock holder thread");
        drop(app);
        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test]
    async fn file_ops_move_route_returns_operations_and_cross_root_409() {
        let temp_root = unique_temp_dir();
        let other_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::create_dir_all(temp_root.join("dst")).expect("create dst");
        fs::create_dir_all(&other_root).expect("create other root dir");
        fs::write(temp_root.join("m.jpg"), b"m").expect("write m");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let database = Database::open(db_dir.join("megle.sqlite")).expect("open db");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "ops".to_string(),
            })
            .expect("add root");
        let other_root_id = database
            .add_root(NewRoot {
                path: other_root.to_string_lossy().into_owned(),
                display_name: "other".to_string(),
            })
            .expect("add other root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "ops-root".to_string(),
                mtime: Some(1),
            })
            .expect("upsert root folder");
        let dst_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: Some(folder_id),
                name: "dst".to_string(),
                path_hash: "dst-folder".to_string(),
                mtime: Some(1),
            })
            .expect("upsert dst");
        let other_root_folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id: other_root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "other-root".to_string(),
                mtime: Some(1),
            })
            .expect("upsert other folder");
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: "m.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 1,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .expect("upsert file");
        let app = router(AppState::new(database));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/file-ops/move")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "fileIds": [file_id],
                            "targetFolderId": dst_id
                        })
                        .to_string(),
                    ))
                    .expect("build move request"),
            )
            .await
            .expect("move");
        let status = response.status();
        let body = response_json(response).await;
        assert_eq!(status, StatusCode::OK, "move response: {body}");
        let ops = body["operations"].as_array().expect("operations");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0]["operation"], "move");
        assert_eq!(ops[0]["status"], "succeeded");

        // Now try cross-root: move from same source to other root's folder.
        // Reseed a file at temp_root.
        fs::write(temp_root.join("x.jpg"), b"x").expect("write x");
        let cross_root_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/file-ops/move")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "fileIds": [file_id],
                            "targetFolderId": other_root_folder_id
                        })
                        .to_string(),
                    ))
                    .expect("build cross-root request"),
            )
            .await
            .expect("cross root");
        assert_eq!(cross_root_response.status(), StatusCode::CONFLICT);
        let body = response_json(cross_root_response).await;
        assert_eq!(body["code"], "cross_root");

        drop(app);
        let _ = fs::remove_dir_all(&temp_root);
        let _ = fs::remove_dir_all(&other_root);
        let _ = fs::remove_dir_all(&db_dir);
    }

    #[tokio::test]
    async fn file_ops_delete_route_supports_recycle_and_permanent() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("p.jpg"), b"p").expect("write p");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let database = Database::open(db_dir.join("megle.sqlite")).expect("open db");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "ops".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "ops-root".to_string(),
                mtime: Some(1),
            })
            .expect("upsert folder");
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: "p.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 1,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .expect("upsert file");
        let app = router(AppState::new(database));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/file-ops/delete")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "fileIds": [file_id],
                            "permanent": true
                        })
                        .to_string(),
                    ))
                    .expect("build delete request"),
            )
            .await
            .expect("delete");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let ops = body["operations"].as_array().expect("operations");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0]["operation"], "delete_permanent");
        assert!(!temp_root.join("p.jpg").exists());

        let empty = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/file-ops/delete")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "permanent": true }).to_string(),
                    ))
                    .expect("build empty request"),
            )
            .await
            .expect("delete empty");
        assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

        drop(app);
        let _ = fs::remove_dir_all(&temp_root);
        let _ = fs::remove_dir_all(&db_dir);
    }

    #[tokio::test]
    async fn file_ops_list_returns_keyset_page() {
        let temp_root = unique_temp_dir();
        fs::create_dir_all(&temp_root).expect("create root dir");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let database = Database::open(db_dir.join("megle.sqlite")).expect("open db");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "ops".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "ops-root".to_string(),
                mtime: Some(1),
            })
            .expect("upsert folder");
        let mut file_ids = Vec::new();
        for index in 0..3 {
            let name = format!("f{index}.jpg");
            fs::write(temp_root.join(&name), b"x").expect("write file");
            let file_id = database
                .upsert_file(crate::db::FileUpsert {
                    root_id,
                    folder_id,
                    name: name.clone(),
                    ext: ".jpg".to_string(),
                    size: 1,
                    mtime: 1,
                    ctime: None,
                    file_key: None,
                })
                .expect("upsert file");
            file_ids.push(file_id);
        }
        // Drive renames through the route for genuine logs.
        let app = router(AppState::new(database));
        for index in 0..3 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/file-ops/rename")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::json!({
                                "fileId": file_ids[index],
                                "newName": format!("renamed{index}.jpg")
                            })
                            .to_string(),
                        ))
                        .expect("build request"),
                )
                .await
                .expect("rename");
            let status = response.status();
            let body = response_json(response).await;
            assert_eq!(status, StatusCode::OK, "rename {index}: {body}");
        }

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/file-ops?limit=2")
                    .body(Body::empty())
                    .expect("build list request"),
            )
            .await
            .expect("list");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let items = body["items"].as_array().expect("items");
        assert_eq!(items.len(), 2);
        assert!(items[0]["id"].as_i64().unwrap() > items[1]["id"].as_i64().unwrap());
        assert!(items
            .iter()
            .all(|item| item["operationGroupId"].is_string()));
        assert!(body["nextCursor"].is_string());

        drop(app);
        let _ = fs::remove_dir_all(&temp_root);
        let _ = fs::remove_dir_all(&db_dir);
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read response body");
        serde_json::from_slice(&bytes).expect("parse json response")
    }

    async fn wait_for_task_status(
        app: &axum::Router,
        task_id: i64,
        expected_status: &str,
    ) -> serde_json::Value {
        let mut last_status = None;
        for _ in 0..250 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/api/tasks")
                        .body(Body::empty())
                        .expect("build request"),
                )
                .await
                .expect("list tasks");
            assert_eq!(response.status(), StatusCode::OK);
            let body = response_json(response).await;
            let task = body["items"]
                .as_array()
                .expect("task items")
                .iter()
                .find(|task| task["id"] == task_id)
                .cloned();
            let status = task
                .as_ref()
                .and_then(|task| task["status"].as_str())
                .map(str::to_string);
            if status.as_deref() == Some(expected_status) {
                return task.expect("task exists");
            }
            last_status = status;
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!(
            "task {task_id} did not reach status {expected_status}; last status was {:?}",
            last_status
        );
    }

    /// Polls `GET /api/tasks` until `task_id` is no longer present. Used for
    /// tasks that self-delete on completion (e.g. `root_purge`) instead of
    /// reaching a terminal status, so `wait_for_task_status` cannot observe
    /// them finishing.
    async fn wait_for_task_absent(app: &axum::Router, task_id: i64) {
        for _ in 0..250 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/api/tasks")
                        .body(Body::empty())
                        .expect("build request"),
                )
                .await
                .expect("list tasks");
            assert_eq!(response.status(), StatusCode::OK);
            let body = response_json(response).await;
            let present = body["items"]
                .as_array()
                .expect("task items")
                .iter()
                .any(|task| task["id"] == task_id);
            if !present {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("task {task_id} did not disappear (root_purge did not complete in time)");
    }

    async fn wait_for_media_count(app: &axum::Router, root_id: i64, expected_count: usize) {
        let mut last_count = None;
        for _ in 0..250 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(format!("/api/media?rootId={root_id}&sort=name_asc"))
                        .body(Body::empty())
                        .expect("build request"),
                )
                .await
                .expect("list media");
            assert_eq!(response.status(), StatusCode::OK);
            let body = response_json(response).await;
            let count = body["items"].as_array().expect("media items").len();
            if count == expected_count {
                return;
            }
            last_count = Some(count);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!(
            "root {root_id} did not reach media count {expected_count}; last count was {:?}",
            last_count
        );
    }

    fn test_database() -> Database {
        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        database
    }

    fn test_database_pair(path: PathBuf) -> (Database, Database) {
        let database = Database::open(&path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let worker_database = Database::open(&path).expect("open worker database");
        (database, worker_database)
    }

    fn seed_media_file(database: &Database) -> i64 {
        seed_media_file_named(database, "image.jpg")
    }

    fn seed_media_file_named(database: &Database, name: &str) -> i64 {
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: format!("D:/Pictures/{name}"),
                display_name: format!("Pictures {name}"),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: format!("root-hash-{name}"),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: name.to_string(),
                ext: ".jpg".to_string(),
                size: 1000,
                mtime: 2,
                ctime: None,
                file_key: None,
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");
        file_id
    }

    fn seed_media_files_in_one_root(database: &Database, names: &[&str]) -> (i64, Vec<i64>) {
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: "D:/Pictures/scope-priority".to_string(),
                display_name: "Scope Priority".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "scope-priority-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let mut file_ids = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let file_id = database
                .upsert_file(crate::db::FileUpsert {
                    root_id,
                    folder_id,
                    name: (*name).to_string(),
                    ext: ".jpg".to_string(),
                    size: 1000 + index as i64,
                    mtime: 2 + index as i64,
                    ctime: None,
                    file_key: Some(format!("scope-priority-{name}")),
                })
                .expect("insert file");
            database
                .upsert_media_kind(file_id, "image")
                .expect("insert media kind");
            file_ids.push(file_id);
        }
        (root_id, file_ids)
    }

    fn seed_thumbnail_cache_scope_fixture(database: &Database, label: &str) -> (i64, i64) {
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: format!("D:/Pictures/{label}"),
                display_name: format!("Pictures {label}"),
            })
            .expect("add cache scope root");
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: format!("{label}-root-hash"),
                mtime: Some(10),
            })
            .expect("insert cache scope folder");
        (root_id, folder_id)
    }

    fn seed_thumbnail_cache_candidate(
        database: &Database,
        root_id: i64,
        folder_id: i64,
        name: &str,
        thumbnail_state: &str,
        with_blob: bool,
        stale_fingerprint: bool,
    ) -> i64 {
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: name.to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: Some(format!("cache-{name}")),
            })
            .expect("insert cache candidate file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert cache candidate media");
        let current_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get cache candidate source")
            .expect("cache candidate source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        if thumbnail_state != "absent" {
            database
                .upsert_thumbnail_state(crate::db::ThumbnailStateUpsert {
                    file_id,
                    profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                    state: thumbnail_state.to_string(),
                    cache_key: None,
                    width: if thumbnail_state == "ready" {
                        Some(320)
                    } else {
                        None
                    },
                    height: if thumbnail_state == "ready" {
                        Some(160)
                    } else {
                        None
                    },
                    byte_size: if thumbnail_state == "ready" {
                        Some(42)
                    } else {
                        None
                    },
                    error: if thumbnail_state == "failed" {
                        Some("decode failed".to_string())
                    } else {
                        None
                    },
                    source_fingerprint: if thumbnail_state == "pending" {
                        None
                    } else if stale_fingerprint {
                        Some(format!("stale-{name}"))
                    } else {
                        Some(current_fingerprint)
                    },
                })
                .expect("seed cache candidate state");
        }
        if with_blob {
            database
                .upsert_thumb_blob(crate::db::ThumbBlobRecord {
                    file_id,
                    profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                    data: b"RIFF\x04\0\0\0WEBP".to_vec(),
                    width: 320,
                    height: 160,
                    byte_size: 12,
                    output_format: crate::thumbnails::GENERATED_FORMAT.to_string(),
                    created_at: 1,
                    updated_at: 1,
                })
                .expect("seed cache candidate blob");
        }
        file_id
    }

    fn unique_temp_dir() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

        std::env::temp_dir().join(format!(
            "megle_api_test_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    fn test_notification_operation(operation_id: &str, updated_at: i64) -> NotificationOperation {
        NotificationOperation {
            operation_id: operation_id.to_string(),
            revision: 0,
            kind: "route_test".to_string(),
            category: "error".to_string(),
            origin: "system".to_string(),
            presentation: "important".to_string(),
            status: "running".to_string(),
            phase: Some("testing".to_string()),
            title: "Route test notification".to_string(),
            summary: None,
            task_id: None,
            root_id: None,
            folder_id: None,
            file_id: None,
            current_value: 0,
            total_value: None,
            unit: None,
            error_code: None,
            error_detail: None,
            cancellable: false,
            retryable: false,
            attempt: 1,
            occurrence_count: 1,
            started_at: updated_at,
            updated_at,
            completed_at: None,
        }
    }

    async fn publish_test_notification(
        writer: crate::db::DbWriter,
        operation_id: &'static str,
        updated_at: i64,
    ) {
        writer
            .write(move |database| {
                let mut operation = test_notification_operation(operation_id, updated_at);
                database.publish_notification_operation(&mut operation, PublishMode::Phase)?;
                Ok(())
            })
            .await
            .expect("publish route test notification");
    }

    #[tokio::test]
    async fn notification_outbox_route_returns_immediately_replays_and_acknowledges_idempotently() {
        let database = test_database();
        let mut operation = test_notification_operation("route:immediate", 1_000);
        operation.status = "succeeded".to_string();
        operation.phase = None;
        operation.completed_at = Some(operation.updated_at);
        database
            .publish_notification_operation(&mut operation, PublishMode::Terminal)
            .expect("publish immediate notification");
        let app = router(AppState::new(database));

        let started = std::time::Instant::now();
        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/notifications/outbox?afterSequence=0&limit=1&waitMs=1000")
                    .body(Body::empty())
                    .expect("build immediate outbox request"),
            )
            .await
            .expect("request immediate outbox");
        assert_eq!(first.status(), StatusCode::OK);
        assert!(started.elapsed() < std::time::Duration::from_millis(250));
        let first_body = response_json(first).await;
        assert_eq!(first_body["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            first_body["items"][0]["operation"]["operationId"],
            "route:immediate"
        );
        let sequence = first_body["items"][0]["sequence"]
            .as_i64()
            .expect("outbox sequence");
        assert_eq!(first_body["maxSequence"], sequence);

        let replay = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/notifications/outbox?afterSequence=0&limit=1&waitMs=0")
                    .body(Body::empty())
                    .expect("build replay request"),
            )
            .await
            .expect("request replay");
        let replay_body = response_json(replay).await;
        assert_eq!(replay_body["items"][0]["sequence"], sequence);

        for _ in 0..2 {
            let ack = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/notifications/outbox/ack")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(format!(r#"{{"maxSequence":{sequence}}}"#)))
                        .expect("build acknowledgement request"),
                )
                .await
                .expect("acknowledge outbox");
            assert_eq!(ack.status(), StatusCode::OK);
            assert_eq!(response_json(ack).await["acknowledged"], true);
        }

        let empty = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/notifications/outbox?afterSequence={sequence}&limit=200&waitMs=0"
                    ))
                    .body(Body::empty())
                    .expect("build empty outbox request"),
            )
            .await
            .expect("request acknowledged outbox");
        let empty_body = response_json(empty).await;
        assert!(empty_body["items"].as_array().expect("items").is_empty());
        assert_eq!(empty_body["maxSequence"], sequence);
    }

    #[tokio::test]
    async fn notification_outbox_route_waits_once_then_returns_empty_without_polling() {
        let app = router(AppState::new(test_database()));
        let started = std::time::Instant::now();
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/notifications/outbox?afterSequence=17&limit=100&waitMs=60")
                    .body(Body::empty())
                    .expect("build timeout outbox request"),
            )
            .await
            .expect("request timeout outbox");
        let elapsed = started.elapsed();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            elapsed >= std::time::Duration::from_millis(45),
            "elapsed {elapsed:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "elapsed {elapsed:?}"
        );
        let body = response_json(response).await;
        assert!(body["items"].as_array().expect("items").is_empty());
        assert_eq!(body["maxSequence"], 17);
    }

    #[tokio::test]
    async fn notification_outbox_route_flushes_dirty_head_before_serving_requests() {
        let database = test_database();
        let mut operation = test_notification_operation("route:dirty", 1_000);
        operation.category = "task".to_string();
        database
            .publish_notification_operation(&mut operation, PublishMode::Phase)
            .expect("publish initial dirty fixture phase");
        let first_sequence = database
            .list_notification_outbox(0, 100)
            .expect("read initial dirty fixture event")[0]
            .sequence;
        operation.current_value = 1;
        operation.updated_at = 1_500;
        assert!(!database
            .publish_notification_operation(&mut operation, PublishMode::Progress)
            .expect("persist coalesced dirty progress"));

        let app = router(AppState::new(database));
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/notifications/outbox?afterSequence={first_sequence}&limit=100&waitMs=0"
                    ))
                    .body(Body::empty())
                    .expect("build dirty flush request"),
            )
            .await
            .expect("request flushed dirty event");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(body["items"][0]["operation"]["currentValue"], 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn notification_outbox_route_bounds_one_hundred_waiters_and_wakes_every_page() {
        const WAITER_COUNT: usize = 100;
        let mut state = AppState::new(test_database());
        let writer = state.writer.clone();
        let read_gate = Arc::clone(&state.notification_outbox_read_gate);
        let (probe, mut read_started, mut waiter_registered) =
            NotificationOutboxTestProbe::new(true);
        state.notification_outbox_test_probe = Some(Arc::clone(&probe));
        let app = router(state);
        let mut requests = Vec::new();
        for _ in 0..WAITER_COUNT {
            let app = app.clone();
            requests.push(tokio::spawn(async move {
                app.oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/api/notifications/outbox?afterSequence=0&limit=1&waitMs=5000")
                        .body(Body::empty())
                        .expect("build concurrent outbox request"),
                )
                .await
                .expect("request concurrent outbox")
            }));
        }

        for _ in 0..NOTIFICATION_OUTBOX_READ_CONCURRENCY {
            tokio::time::timeout(std::time::Duration::from_secs(1), read_started.recv())
                .await
                .expect("bounded read must start")
                .expect("read probe remains open");
        }
        assert!(
            matches!(
                read_started.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "no read beyond the shared gate may start while all permits are held"
        );
        assert_eq!(probe.active_reads(), NOTIFICATION_OUTBOX_READ_CONCURRENCY);
        assert_eq!(
            probe.max_active_reads(),
            NOTIFICATION_OUTBOX_READ_CONCURRENCY
        );
        probe.release_reads(WAITER_COUNT * 3);
        for _ in 0..WAITER_COUNT {
            tokio::time::timeout(std::time::Duration::from_secs(2), waiter_registered.recv())
                .await
                .expect("every request must register after an empty second query")
                .expect("waiter probe remains open");
        }
        publish_test_notification(writer, "route:broadcast", 2_000).await;

        for request in requests {
            let response = tokio::time::timeout(std::time::Duration::from_secs(2), request)
                .await
                .expect("all long-poll waiters must wake for the same commit")
                .expect("join concurrent outbox request");
            assert_eq!(response.status(), StatusCode::OK);
            let body = response_json(response).await;
            assert_eq!(body["items"].as_array().map(Vec::len), Some(1));
            assert_eq!(
                body["items"][0]["operation"]["operationId"],
                "route:broadcast"
            );
            assert_eq!(body["maxSequence"], body["items"][0]["sequence"]);
        }
        assert!(probe.max_active_reads() <= NOTIFICATION_OUTBOX_READ_CONCURRENCY);
        assert_eq!(probe.active_reads(), 0);
        assert_eq!(probe.active_waiters(), 0);
        assert_eq!(
            read_gate.available_permits(),
            NOTIFICATION_OUTBOX_READ_CONCURRENCY
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn notification_outbox_route_reopened_database_wake_closes_registration_race() {
        let directory = unique_temp_dir();
        fs::create_dir_all(&directory).expect("create outbox route directory");
        let database =
            Database::open(directory.join("notifications.sqlite")).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let producer = database
            .reopen()
            .expect("reopen notification producer")
            .expect("file database can reopen");
        let mut state = AppState::new(database);
        let (probe, _read_started, mut waiter_registered) = NotificationOutboxTestProbe::new(false);
        state.notification_outbox_test_probe = Some(probe);
        let app = router(state);
        let request = tokio::spawn(async move {
            app.oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/notifications/outbox?afterSequence=0&limit=100&waitMs=1000")
                    .body(Body::empty())
                    .expect("build reopened wake request"),
            )
            .await
            .expect("request reopened wake outbox")
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter_registered.recv())
            .await
            .expect("request must register after an empty second query")
            .expect("waiter probe remains open");
        let mut operation = test_notification_operation("route:reopened", 3_000);
        producer
            .publish_notification_operation(&mut operation, PublishMode::Phase)
            .expect("publish through reopened database");

        let response = tokio::time::timeout(std::time::Duration::from_millis(500), request)
            .await
            .expect("reopened shared wake must release route")
            .expect("join reopened wake request");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["items"][0]["operation"]["operationId"],
            "route:reopened"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn notification_outbox_route_cancellation_releases_read_gate_permit() {
        let mut state = AppState::new(test_database());
        let read_gate = Arc::clone(&state.notification_outbox_read_gate);
        let (probe, mut read_started, _waiter_registered) = NotificationOutboxTestProbe::new(true);
        state.notification_outbox_test_probe = Some(Arc::clone(&probe));
        let app = router(state);
        let request = tokio::spawn(async move {
            app.oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/notifications/outbox?afterSequence=0&limit=1&waitMs=5000")
                    .body(Body::empty())
                    .expect("build cancellable read request"),
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), read_started.recv())
            .await
            .expect("read must acquire one gate permit")
            .expect("read probe remains open");
        assert_eq!(
            read_gate.available_permits(),
            NOTIFICATION_OUTBOX_READ_CONCURRENCY - 1
        );

        request.abort();
        assert!(request
            .await
            .expect_err("request must be cancelled")
            .is_cancelled());
        assert_eq!(probe.active_reads(), 0);
        assert_eq!(
            read_gate.available_permits(),
            NOTIFICATION_OUTBOX_READ_CONCURRENCY
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn notification_outbox_route_cancellation_unregisters_waiter_without_holding_read_gate() {
        let mut state = AppState::new(test_database());
        let read_gate = Arc::clone(&state.notification_outbox_read_gate);
        let (probe, _read_started, mut waiter_registered) = NotificationOutboxTestProbe::new(false);
        state.notification_outbox_test_probe = Some(Arc::clone(&probe));
        let app = router(state);
        let request = tokio::spawn(async move {
            app.oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/notifications/outbox?afterSequence=0&limit=1&waitMs=30000")
                    .body(Body::empty())
                    .expect("build cancellable waiter request"),
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter_registered.recv())
            .await
            .expect("request must register its waiter")
            .expect("waiter probe remains open");
        assert_eq!(probe.active_waiters(), 1);
        assert_eq!(
            read_gate.available_permits(),
            NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            "waiting must not retain a read permit"
        );

        request.abort();
        assert!(request
            .await
            .expect_err("request must be cancelled")
            .is_cancelled());
        assert_eq!(probe.active_waiters(), 0);
        assert_eq!(
            read_gate.available_permits(),
            NOTIFICATION_OUTBOX_READ_CONCURRENCY
        );
    }

    #[tokio::test]
    async fn notification_outbox_route_validates_query_and_ack_bounds() {
        const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
        let database = test_database();
        for index in 0..3 {
            let mut operation =
                test_notification_operation(&format!("route:limit:{index}"), 4_000 + index);
            operation.status = "succeeded".to_string();
            operation.phase = None;
            operation.completed_at = Some(operation.updated_at);
            database
                .publish_notification_operation(&mut operation, PublishMode::Terminal)
                .expect("publish limit fixture");
        }
        let app = router(AppState::new(database));
        let limited = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/notifications/outbox?afterSequence=0&limit=2&waitMs=0")
                    .body(Body::empty())
                    .expect("build limited outbox request"),
            )
            .await
            .expect("request limited outbox page");
        assert_eq!(limited.status(), StatusCode::OK);
        assert_eq!(
            response_json(limited).await["items"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
        let max_safe = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/notifications/outbox?afterSequence={MAX_SAFE_INTEGER}&waitMs=0"
                    ))
                    .body(Body::empty())
                    .expect("build max-safe outbox request"),
            )
            .await
            .expect("request max-safe outbox page");
        assert_eq!(max_safe.status(), StatusCode::OK);
        assert_eq!(
            response_json(max_safe).await["maxSequence"],
            MAX_SAFE_INTEGER
        );
        for uri in [
            "/api/notifications/outbox?afterSequence=-1",
            "/api/notifications/outbox?afterSequence=9007199254740992",
            "/api/notifications/outbox?limit=0",
            "/api/notifications/outbox?limit=201",
            "/api/notifications/outbox?waitMs=-1",
            "/api/notifications/outbox?waitMs=30001",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(uri)
                        .body(Body::empty())
                        .expect("build invalid outbox request"),
                )
                .await
                .expect("request invalid outbox query");
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "uri {uri}");
        }

        let max_safe_ack = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/notifications/outbox/ack")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        r#"{{"maxSequence":{MAX_SAFE_INTEGER}}}"#
                    )))
                    .expect("build max-safe ack request"),
            )
            .await
            .expect("request max-safe outbox ack");
        assert_eq!(max_safe_ack.status(), StatusCode::OK);

        let invalid_ack = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/notifications/outbox/ack")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"maxSequence":-1}"#))
                    .expect("build invalid ack request"),
            )
            .await
            .expect("request invalid outbox ack");
        assert_eq!(invalid_ack.status(), StatusCode::BAD_REQUEST);

        let unsafe_ack = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/notifications/outbox/ack")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"maxSequence":9007199254740992}"#))
                    .expect("build unsafe ack request"),
            )
            .await
            .expect("request unsafe outbox ack");
        assert_eq!(unsafe_ack.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn notification_outbox_response_rejects_sequence_above_javascript_safe_integer() {
        let result = notification_outbox_response(
            vec![NotificationOutboxEvent {
                sequence: 9_007_199_254_740_992,
                operation: test_notification_operation("route:unsafe-sequence", 5_000),
            }],
            0,
        );
        assert!(result.is_err());
        assert_eq!(
            result.expect_err("unsafe sequence must fail").status,
            StatusCode::BAD_REQUEST
        );
    }

    async fn assert_notification_request_error(
        response: axum::response::Response,
        case: &str,
        expected_message: &str,
    ) {
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "case {case}");
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(
            content_type.starts_with("application/json"),
            "case {case} returned content type {content_type:?}"
        );
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read notification request error body");
        let body: ErrorResponse = serde_json::from_slice(&bytes)
            .expect("notification request error must deserialize as ErrorResponse");
        assert!(
            body.code
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            "case {case} returned empty error code: {body:?}"
        );
        assert!(
            !body.error.trim().is_empty(),
            "case {case} returned empty error message: {body:?}"
        );
        assert_eq!(body.error, expected_message, "case {case}");
        for parser_detail in ["expected i64", "line", "column", "deserialize"] {
            assert!(
                !body.error.to_ascii_lowercase().contains(parser_detail),
                "case {case} leaked parser detail {parser_detail:?}: {body:?}"
            );
        }
    }

    #[tokio::test]
    async fn notification_outbox_route_maps_query_rejections_to_json_error_response() {
        let app = router(AppState::new(test_database()));
        for (case, uri) in [
            (
                "afterSequence is not an integer",
                "/api/notifications/outbox?afterSequence=abc",
            ),
            (
                "limit is not an integer",
                "/api/notifications/outbox?limit=many",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(uri)
                        .body(Body::empty())
                        .expect("build malformed notification query"),
                )
                .await
                .expect("request malformed notification query");
            assert_notification_request_error(response, case, "invalid notification outbox query")
                .await;
        }
    }

    #[tokio::test]
    async fn notification_outbox_route_maps_json_rejections_to_json_error_response() {
        let app = router(AppState::new(test_database()));
        for (case, body, content_type) in [
            ("missing maxSequence", r#"{}"#, true),
            (
                "maxSequence has the wrong type",
                r#"{"maxSequence":"bad"}"#,
                true,
            ),
            ("invalid JSON", r#"{"maxSequence":0"#, true),
            ("missing Content-Type", r#"{"maxSequence":0}"#, false),
        ] {
            let mut request = Request::builder()
                .method(Method::POST)
                .uri("/api/notifications/outbox/ack");
            if content_type {
                request = request.header(header::CONTENT_TYPE, "application/json");
            }
            let response = app
                .clone()
                .oneshot(
                    request
                        .body(Body::from(body))
                        .expect("build malformed notification acknowledgement"),
                )
                .await
                .expect("request malformed notification acknowledgement");
            assert_notification_request_error(
                response,
                case,
                "invalid notification outbox acknowledgement",
            )
            .await;
        }
    }

    #[tokio::test]
    async fn notification_outbox_route_acknowledgement_requires_session_authentication() {
        let app = router_with_config(
            test_database(),
            ApiConfig {
                session_token: Some("notification-session".to_string()),
                ..ApiConfig::default()
            },
        );
        let request = || {
            Request::builder()
                .method(Method::POST)
                .uri("/api/notifications/outbox/ack")
                .header(header::CONTENT_TYPE, "application/json")
        };
        let unauthorized = app
            .clone()
            .oneshot(
                request()
                    .body(Body::from(r#"{"maxSequence":0}"#))
                    .expect("build unauthenticated ack"),
            )
            .await
            .expect("request unauthenticated ack");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let authorized = app
            .oneshot(
                request()
                    .header("x-megle-session", "notification-session")
                    .body(Body::from(r#"{"maxSequence":0}"#))
                    .expect("build authenticated ack"),
            )
            .await
            .expect("request authenticated ack");
        assert_eq!(authorized.status(), StatusCode::OK);
        assert_eq!(response_json(authorized).await["acknowledged"], true);
    }

    #[tokio::test]
    async fn plugin_routes_basic_lifecycle() {
        let database = test_database();
        let state = AppState::new(database);
        let app = router(state);

        // GET /api/plugins -> empty list (no rows persisted).
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/plugins")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("list plugins");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert!(body["items"].as_array().expect("items").is_empty());

        // POST /api/plugins/{id}/enable -> 404 with code plugin_not_found.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/plugins/com.example.missing/enable")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("enable missing plugin");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response_json(response).await;
        assert_eq!(body["code"], "plugin_not_found");

        // DELETE /api/plugins/{id} -> 404 with code plugin_not_found.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/plugins/com.example.missing")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("delete missing plugin");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response_json(response).await;
        assert_eq!(body["code"], "plugin_not_found");
    }

    #[tokio::test]
    async fn plugin_discover_route_returns_zero_for_missing_directory() {
        // Confirm the discover handler reports zero/zero when the configured
        // plugins folder does not exist on disk. We exercise the discovery
        // function directly to avoid mutating process-wide environment
        // variables, which would race with tests that share the runtime.
        let database = test_database();
        let plugins_dir = unique_temp_dir();
        let report =
            crate::plugins::discover_and_persist(&database, &plugins_dir).expect("discover");
        assert_eq!(report.discovered, 0);
        assert!(report.errors.is_empty());
    }

    #[tokio::test]
    async fn serve_web_disabled_does_not_intercept_api_routes() {
        // With `web_dir` unset (the desktop default), the API router still
        // owns the namespace and unknown paths return 404 instead of being
        // rewritten to `index.html`.
        let app = router_with_config(test_database(), ApiConfig::default());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/health")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("health request");
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("root request");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn serve_web_enabled_falls_back_to_index_html_for_unknown_paths() {
        // Stand up a tiny `dist/` so ServeDir + ServeFile fallback can route
        // an unknown SPA path back to `index.html`.
        let web_dir = unique_temp_dir();
        fs::create_dir_all(&web_dir).expect("create web dir");
        fs::write(
            web_dir.join("index.html"),
            b"<!doctype html><title>spa</title>",
        )
        .expect("write index.html");
        fs::create_dir_all(web_dir.join("assets")).expect("create assets dir");
        fs::write(
            web_dir.join("assets").join("app.abc123.js"),
            b"console.log('hashed asset');",
        )
        .expect("write hashed asset");

        let app = router_with_config(
            test_database(),
            ApiConfig {
                web_dir: Some(web_dir.clone()),
                ..ApiConfig::default()
            },
        );

        // API routes still 404 for unknown API paths instead of falling back
        // to index.html.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/does-not-exist")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("unknown api path");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // SPA route falls back to index.html.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/library/some/deep/route")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("spa fallback");
        assert_eq!(response.status(), StatusCode::OK);
        let cache = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(cache, "no-cache");
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read fallback body");
        let body = String::from_utf8_lossy(&bytes).into_owned();
        assert!(
            body.contains("<title>spa</title>"),
            "expected index.html fallback body, got: {body}"
        );

        // Hashed assets get the long-lived immutable cache header.
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/assets/app.abc123.js")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("hashed asset");
        assert_eq!(response.status(), StatusCode::OK);
        let cache = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(cache, "public, max-age=31536000, immutable");

        let _ = fs::remove_dir_all(web_dir);
    }

    #[tokio::test]
    async fn basic_auth_protects_routes_except_health() {
        let credentials = BasicAuthCredentials::parse("alice:s3cret").expect("parse credentials");
        let app = router_with_config(
            test_database(),
            ApiConfig {
                basic_auth: Some(credentials),
                ..ApiConfig::default()
            },
        );

        // No credentials => 401 with WWW-Authenticate.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/roots")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("missing creds");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let challenge = response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            challenge.contains("Basic"),
            "expected Basic challenge, got: {challenge}"
        );

        // Correct credentials => API request goes through.
        let token = base64::engine::general_purpose::STANDARD.encode(b"alice:s3cret");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/roots")
                    .header(header::AUTHORIZATION, format!("Basic {token}"))
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("good creds");
        assert_eq!(response.status(), StatusCode::OK);

        // /api/health is always reachable without credentials so Docker
        // healthchecks keep working.
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/health")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("health no creds");
        assert_eq!(response.status(), StatusCode::OK);
    }
}
