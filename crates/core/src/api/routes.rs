use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{OriginalUri, Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use axum_extra::extract::Query as ExtraQuery;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path as FilePath;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::api::AppState;
use crate::db::notification_outbox::NotificationOutboxEvent;
use crate::db::{
    Database, DirectorySortMutationResult, DirectorySortRule, DirectorySortState, DirectoryTarget,
    FolderChildrenPage, FolderRecord, MarkRootForPurge, MediaPageQuery, MediaRecord, NewRoot,
    NotificationOrigin, PreviewBlobRecord, PreviewFailureRecord, RootRecord, SearchQuery, TagError,
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
    classify_thumbnail_failure, generate_preview_webp, is_pending_status, normalize_profile,
    preview_profile_max_px, select_preview_profile, ThumbnailFailureKind, PREVIEW_NATIVE_MAX_PX,
    PREVIEW_NATIVE_PROFILE,
};

const THUMBNAIL_TASK_WAKE_DEDUPE_WINDOW: Duration = Duration::from_millis(750);
const INVALID_NOTIFICATION_OUTBOX_QUERY: &str = "invalid notification outbox query";
const INVALID_NOTIFICATION_OUTBOX_ACKNOWLEDGEMENT: &str =
    "invalid notification outbox acknowledgement";
const MAX_SAFE_NOTIFICATION_SEQUENCE: i64 = 9_007_199_254_740_991;
const PREVIEW_FAILURE_CACHE_TTL_SECONDS: i64 = 5 * 60;
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
    Libraries,
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
    RouteGroup::Libraries,
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
    "/api/libraries",
    "/api/libraries/active",
    "/api/libraries/preflight",
    "/api/libraries/open",
    "/api/libraries/{libraryUuid}/registration",
    "/api/libraries/{libraryUuid}/activate",
    "/api/libraries/{libraryUuid}",
    "/api/libraries/{libraryUuid}/local-copy",
    "/api/libraries/{libraryUuid}/sync-binding",
    "/api/sync/status",
    "/api/sync/wake",
    "/api/sync/remote-libraries",
    "/api/sync/remote-libraries/{accountUuid}/{libraryUuid}/roots",
    "/api/sync/restores/preflight",
    "/api/sync/restores",
    "/api/sync/restores/{restoreUuid}",
    "/api/sync/restores/{restoreUuid}/pause",
    "/api/sync/restores/{restoreUuid}/resume",
    "/api/sync/restores/{restoreUuid}/cancel",
    "/api/libraries/{libraryUuid}/sync/preflight",
    "/api/libraries/{libraryUuid}/sync/enable",
    "/api/libraries/{libraryUuid}/sync/pause",
    "/api/libraries/{libraryUuid}/sync/resume",
    "/api/libraries/{libraryUuid}/sync/sync-now",
    "/api/libraries/{libraryUuid}/sync/retry",
    "/api/libraries/{libraryUuid}/sync/unbind-plan",
    "/api/libraries/{libraryUuid}/sync/unbind-commit",
    "/api/libraries/{libraryUuid}/sync/reconnect-preflight",
    "/api/libraries/{libraryUuid}/sync/reconnect-commit",
    "/api/libraries/{libraryUuid}/sync/conflicts",
    "/api/libraries/{libraryUuid}/sync/conflicts/{conflictUuid}/resolve",
    "/api/libraries/{libraryUuid}/sync-folders",
    "/api/libraries/{libraryUuid}/sync-folders/settings",
    "/api/libraries/{libraryUuid}/sync-folders/enrollment/preflight",
    "/api/libraries/{libraryUuid}/sync-folders/enrollment/commit",
    "/api/libraries/{libraryUuid}/sync-folders/selection/preflight",
    "/api/libraries/{libraryUuid}/sync-folders/selection/commit",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/rebind/preflight",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/rebind/commit",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/commands/{command}",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/stop/preflight",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/stop/commit",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/reenable/preflight",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/reenable/commit",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/reenable/comparisons/{planUuid}",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/reenable/comparisons/{planUuid}/items",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/reenable/comparisons/{planUuid}/items/{itemOrdinal}/resolve",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/rename/preflight",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/rename/commit",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/relocations/preflight",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/relocations",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/relocations/{operationUuid}",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/operations",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/operations/{operationUuid}",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/relocations/{operationUuid}/{action}",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/conflicts",
    "/api/libraries/{libraryUuid}/sync-folders/{rootUuid}/conflicts/{conflictUuid}/resolve",
    "/api/libraries/{libraryUuid}/sync/diagnostics",
    "/api/libraries/{libraryUuid}/sync/diagnostics/export",
    "/api/roots",
    "/api/roots/{rootId}/appearance",
    "/api/roots/{rootId}/sort",
    "/api/folders/{folderId}/sort",
    "/api/roots/{rootId}",
    "/api/folders/{folderId}/appearance",
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
    "/api/media/{fileId}/playback-sessions",
    "/api/playback-sessions/{sessionId}",
    "/api/playback-sessions/{sessionId}/renew",
    "/api/playback/{capability}",
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
    "/api/plugins/{pluginId}/views/{viewId}",
    "/api/plugins/{pluginId}/enable",
    "/api/plugins/{pluginId}/disable",
    "/api/plugins/{pluginId}/grants",
    "/api/plugins/{pluginId}/retry",
    "/api/plugins/{pluginId}/versions/select",
    "/api/plugins/{pluginId}/versions/rollback",
    "/api/plugins/{pluginId}/auth/begin",
    "/api/plugins/{pluginId}/auth/complete",
    "/api/plugins/{pluginId}/auth/cancel",
    "/api/plugins/{pluginId}/auth/sign-out",
    "/api/plugins/{pluginId}/external-link",
    "/api/plugins/{pluginId}/configuration",
    "/api/plugins/{pluginId}/configuration/plan",
    "/api/plugins/{pluginId}/health",
    "/api/plugins/{pluginId}/diagnostics",
    "/api/plugin-auth/callback/{flowId}",
    "/api/image-viewer/formats",
    "/api/image-viewer/folder",
    "/api/image-viewer/image",
    "/api/image-viewer/thumbnail",
    "/api/image-viewer/metadata",
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
    items: Vec<crate::host_registry::HostPluginProjection>,
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
struct PluginCommandResponse {
    accepted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginGrantRequest {
    permissions: Vec<serde_json::Value>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginVersionRequest {
    version: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginAuthCompleteRequest {
    flow_id: String,
    response_uri: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginAuthCancelRequest {
    flow_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginConfigurationPlanRequest {
    values: std::collections::BTreeMap<String, Option<String>>,
    expected_revision: u64,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginConfigurationCommitRequest {
    plan_uuid: String,
    expected_revision: u64,
    confirmed: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginAuthBeginResponse {
    authorization_url: String,
    flow_id: String,
    expires_at: i64,
    completion_mode: String,
    redirect_uri: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginHealthResponse {
    state: String,
    last_healthy_at: Option<i64>,
    last_error_code: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginDiagnosticsResponse {
    generated_at: i64,
    entries: Vec<serde_json::Value>,
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
pub(crate) struct PreviewPlan {
    pub(crate) profile: &'static str,
    pub(crate) max_px: u32,
}

/// Pure preview-source resolution. `None` means "stream the original file
/// bytes", which is the zero-cost majority path and the exact behaviour that
/// shipped before this pipeline existed.
pub(crate) fn resolve_preview_plan(
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
    // instead would route every non-browser format into a decode that can fail
    // for corrupt data or an unavailable external decoder, turning a 200 into a
    // 500 for a URL that used to work.
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

pub(super) struct CoreError {
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
        .route("/api/libraries", get(list_libraries).post(create_library))
        .route("/api/libraries/active", get(get_active_library))
        .route("/api/libraries/preflight", post(preflight_library))
        .route("/api/libraries/open", post(open_library))
        .route("/api/libraries/:library_uuid/registration", axum::routing::delete(forget_library))
        .route(
            "/api/libraries/:library_uuid/activate",
            post(activate_library),
        )
        .route(
            "/api/libraries/:library_uuid",
            axum::routing::patch(update_library),
        )
        .route(
            "/api/libraries/:library_uuid/local-copy",
            axum::routing::delete(remove_local_library),
        )
        .route(
            "/api/libraries/:library_uuid/sync-binding",
            post(enable_library_sync).delete(unbind_library_sync),
        )
        .route("/api/sync/status", get(get_sync_status))
        .route("/api/sync/wake", post(wake_sync))
        .route("/api/sync/remote-libraries", get(list_remote_libraries))
        .route(
            "/api/sync/remote-libraries/:account_uuid/:library_uuid/roots",
            get(list_remote_library_roots),
        )
        .route("/api/sync/restores/preflight", post(preflight_restore))
        .route("/api/sync/restores", post(start_restore))
        .route("/api/sync/restores/:restore_uuid", get(get_restore))
        .route(
            "/api/sync/restores/:restore_uuid/pause",
            post(pause_restore),
        )
        .route(
            "/api/sync/restores/:restore_uuid/resume",
            post(resume_restore),
        )
        .route(
            "/api/sync/restores/:restore_uuid/cancel",
            post(cancel_restore),
        )
        .route(
            "/api/libraries/:library_uuid/sync/preflight",
            post(sync_preflight),
        )
        .route(
            "/api/libraries/:library_uuid/sync/enable",
            post(start_sync_enable),
        )
        .route(
            "/api/libraries/:library_uuid/sync/pause",
            post(pause_library_sync),
        )
        .route(
            "/api/libraries/:library_uuid/sync/resume",
            post(resume_library_sync),
        )
        .route(
            "/api/libraries/:library_uuid/sync/sync-now",
            post(sync_library_now),
        )
        .route(
            "/api/libraries/:library_uuid/sync/retry",
            post(retry_library_sync),
        )
        .route(
            "/api/libraries/:library_uuid/sync/unbind-plan",
            post(create_unbind_plan),
        )
        .route(
            "/api/libraries/:library_uuid/sync/unbind-commit",
            post(commit_unbind_plan),
        )
        .route(
            "/api/libraries/:library_uuid/sync/reconnect-preflight",
            post(reconnect_preflight),
        )
        .route(
            "/api/libraries/:library_uuid/sync/reconnect-commit",
            post(reconnect_commit),
        )
        .route(
            "/api/libraries/:library_uuid/sync/conflicts",
            get(list_sync_conflicts),
        )
        .route(
            "/api/libraries/:library_uuid/sync/conflicts/:conflict_uuid/resolve",
            post(resolve_sync_conflict),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders",
            get(sync_folder_contract_read),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/settings",
            get(sync_folder_contract_read).patch(sync_folder_contract_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/enrollment/preflight",
            post(sync_folder_contract_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/enrollment/commit",
            post(sync_folder_contract_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/selection/preflight",
            post(sync_folder_contract_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/selection/commit",
            post(sync_folder_contract_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid",
            get(sync_folder_contract_root_read),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/rebind/preflight",
            post(sync_folder_contract_root_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/rebind/commit",
            post(sync_folder_contract_root_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/commands/:command",
            post(sync_folder_contract_root_action),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/stop/preflight",
            post(sync_folder_contract_root_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/stop/commit",
            post(sync_folder_contract_root_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/reenable/preflight",
            post(sync_folder_contract_root_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/reenable/commit",
            post(sync_folder_contract_root_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/reenable/comparisons/:plan_uuid",
            get(sync_folder_contract_plan_read),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/reenable/comparisons/:plan_uuid/items",
            get(sync_folder_contract_plan_read),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/reenable/comparisons/:plan_uuid/items/:item_ordinal/resolve",
            post(sync_folder_contract_comparison_item_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/rename/preflight",
            post(sync_folder_contract_root_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/rename/commit",
            post(sync_folder_contract_root_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/relocations/preflight",
            post(sync_folder_contract_root_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/relocations",
            post(sync_folder_contract_root_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/relocations/:operation_uuid",
            get(sync_folder_contract_operation_read),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/operations",
            get(sync_folder_contract_operations_read),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/operations/:operation_uuid",
            get(sync_folder_contract_operation_read),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/relocations/:operation_uuid/:action",
            post(sync_folder_contract_operation_action),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/conflicts",
            get(sync_folder_contract_root_read),
        )
        .route(
            "/api/libraries/:library_uuid/sync-folders/:root_uuid/conflicts/:conflict_uuid/resolve",
            post(sync_folder_contract_conflict_write),
        )
        .route(
            "/api/libraries/:library_uuid/sync/diagnostics",
            get(sync_diagnostics),
        )
        .route(
            "/api/libraries/:library_uuid/sync/diagnostics/export",
            post(export_sync_diagnostics),
        )
        .route("/api/roots", get(list_roots).post(add_root))
        .route("/api/roots/:root_id", axum::routing::delete(remove_root))
        .route("/api/roots/:root_id/appearance", axum::routing::patch(update_root_appearance))
        .route("/api/roots/:root_id/sort", get(get_root_sort).patch(update_root_sort).delete(reset_root_sort))
        .route("/api/folders/:folder_id/sort", get(get_folder_sort).patch(update_folder_sort).delete(reset_folder_sort))
        .route(
            "/api/folders/:folder_id/appearance",
            axum::routing::patch(update_folder_appearance),
        )
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
        .route("/api/plugins/:plugin_id/views/:view_id", get(get_plugin_view_document))
        .route("/api/plugins/:plugin_id/disable", post(disable_plugin))
        .route("/api/plugins/:plugin_id/grants", put(update_plugin_grants))
        .route("/api/plugins/:plugin_id/retry", post(retry_plugin_runtime))
        .route(
            "/api/plugins/:plugin_id/versions/select",
            post(select_plugin_version),
        )
        .route(
            "/api/plugins/:plugin_id/versions/rollback",
            post(rollback_plugin_version),
        )
        .route(
            "/api/plugins/:plugin_id/auth/begin",
            post(begin_plugin_auth),
        )
        .route(
            "/api/plugins/:plugin_id/auth/complete",
            post(complete_plugin_auth),
        )
        .route(
            "/api/plugins/:plugin_id/auth/cancel",
            post(cancel_plugin_auth),
        )
        .route(
            "/api/plugins/:plugin_id/auth/sign-out",
            post(sign_out_plugin),
        )
        .route("/api/plugins/:plugin_id/external-link", post(resolve_plugin_external_link))
        .route(
            "/api/plugins/:plugin_id/configuration",
            get(get_plugin_configuration).put(commit_plugin_configuration),
        )
        .route(
            "/api/plugins/:plugin_id/configuration/plan",
            post(plan_plugin_configuration),
        )
        .route("/api/plugins/:plugin_id/health", get(plugin_health))
        .route(
            "/api/plugins/:plugin_id/diagnostics",
            get(plugin_diagnostics),
        )
        .route(
            "/api/plugin-auth/callback/:flow_id",
            get(complete_plugin_auth_callback),
        )
        .with_state(state)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LibraryPreflightRequest {
    name: String,
    database_path: String,
    content_roots: Vec<crate::library_path::LibraryContentRootInput>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryPreflightResponse {
    ok: bool,
    reason: String,
    path_role: Option<&'static str>,
    root_index: Option<usize>,
    normalized_database_path: Option<String>,
    normalized_content_roots: Option<Vec<crate::library_path::LibraryContentRootInput>>,
    preflight_token: Option<String>,
    available_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateLibraryRequest {
    name: String,
    database_path: String,
    content_roots: Vec<crate::library_path::LibraryContentRootInput>,
    icon: Option<String>,
    color: Option<String>,
    initial_mode: String,
    preflight_token: Option<String>,
    activate: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenLibraryRequest {
    database_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibrarySummaryResponse {
    library_uuid: String,
    root_uuid: String,
    name: String,
    icon: Option<String>,
    color: Option<String>,
    database_path: String,
    active: bool,
    availability: String,
    lifecycle_state: String,
    binding_state: String,
    context_revision: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryListResponse {
    items: Vec<LibrarySummaryResponse>,
    context_revision: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActiveLibraryResponse {
    library: Option<LibrarySummaryResponse>,
    context_revision: u64,
    runtime_generation: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateLibraryResponse {
    library: LibrarySummaryResponse,
    activated: bool,
    activation_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateLibraryRequest {
    name: Option<String>,
    icon: Option<Option<String>>,
    color: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EnableLibrarySyncRequest {
    provider_id: String,
    account_identity: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncCommandRequest {
    operation_uuid: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncPreflightRequest {
    provider_id: String,
    account_uuid: String,
    remote_library_uuid: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncPreflightResponse {
    ok: bool,
    reason: String,
    preflight_token: Option<String>,
    remote_library_uuid: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncEnableRequest {
    operation_uuid: String,
    provider_id: String,
    account_uuid: String,
    preflight_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UnbindCommitRequest {
    operation_uuid: String,
    plan_uuid: String,
    choice: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReconnectPreflightRequest {
    account_uuid: String,
    remote_library_uuid: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReconnectCommitRequest {
    operation_uuid: String,
    plan_uuid: String,
    choice: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreStartRequest {
    operation_uuid: String,
    preflight_token: String,
    items: Vec<RestoreStartItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestorePreflightRequest {
    items: Vec<RestoreStartItem>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum RestoreStartItem {
    Legacy(RestoreLegacyItem),
    RootAware(RestoreRootAwareItem),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreLegacyItem {
    #[serde(default)]
    restore_mode: Option<RestoreLegacyMode>,
    account_uuid: String,
    library_uuid: String,
    name: String,
    placement_base: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
enum RestoreLegacyMode {
    #[serde(rename = "legacy-library")]
    LegacyLibrary,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreRootAwareItem {
    restore_mode: RestoreRootAwareMode,
    account_uuid: String,
    library_uuid: String,
    placement_base: String,
    inventory_fence: String,
    roots: Vec<RestoreRootChoice>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
enum RestoreRootAwareMode {
    #[serde(rename = "root-aware")]
    RootAware,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreRootChoice {
    root_uuid: String,
    expected_logical_revision: u64,
    expected_materialization_fingerprint: String,
    selected: bool,
    #[serde(default)]
    target_path: Option<String>,
}

fn restore_operation_request(items: &[RestoreStartItem]) -> serde_json::Value {
    let mut items = items.to_vec();
    items.sort_by(|left, right| restore_item_identity(left).cmp(&restore_item_identity(right)));
    for item in &mut items {
        if let RestoreStartItem::RootAware(item) = item {
            item.roots
                .sort_by(|left, right| left.root_uuid.cmp(&right.root_uuid));
        }
    }
    serde_json::json!({ "items": items })
}

fn restore_item_identity(item: &RestoreStartItem) -> (&str, &str) {
    match item {
        RestoreStartItem::Legacy(item) => (&item.account_uuid, &item.library_uuid),
        RestoreStartItem::RootAware(item) => (&item.account_uuid, &item.library_uuid),
    }
}

/// Reuse the actual endpoint DTOs when extracting sandbox target identities.
/// The request body is retained for the normal handler's complete validation.
pub(super) fn plugin_ui_body_targets(
    route: &str,
    bytes: &[u8],
) -> Result<super::plugin_ui_admission::BodyTargets, serde_json::Error> {
    use super::plugin_ui_admission::BodyTargets;
    let mut targets = BodyTargets::default();
    match route {
        "/api/libraries/:library_uuid/sync/preflight" => {
            let body: SyncPreflightRequest = serde_json::from_slice(bytes)?;
            targets.plugin_id = Some(body.provider_id);
            targets.account_uuid = Some(body.account_uuid);
        }
        "/api/libraries/:library_uuid/sync/enable" => {
            let body: SyncEnableRequest = serde_json::from_slice(bytes)?;
            targets.plugin_id = Some(body.provider_id);
            targets.account_uuid = Some(body.account_uuid);
            targets.plan = Some(("enable", body.preflight_token));
        }
        "/api/libraries/:library_uuid/sync/reconnect-preflight" => {
            let body: ReconnectPreflightRequest = serde_json::from_slice(bytes)?;
            targets.account_uuid = Some(body.account_uuid);
        }
        "/api/libraries/:library_uuid/sync/reconnect-commit" => {
            let body: ReconnectCommitRequest = serde_json::from_slice(bytes)?;
            targets.plan = Some(("reconnect", body.plan_uuid));
        }
        "/api/libraries/:library_uuid/sync/unbind-commit" => {
            let body: UnbindCommitRequest = serde_json::from_slice(bytes)?;
            targets.plan = Some(("unbind", body.plan_uuid));
        }
        "/api/sync/restores/preflight" | "/api/sync/restores" => {
            let items = if route.ends_with("/preflight") {
                serde_json::from_slice::<RestorePreflightRequest>(bytes)?.items
            } else {
                serde_json::from_slice::<RestoreStartRequest>(bytes)?.items
            };
            targets.restore_items = items
                .iter()
                .map(|item| {
                    let (account, library) = restore_item_identity(item);
                    (account.to_owned(), library.to_owned())
                })
                .collect();
        }
        _ => (),
    }
    Ok(targets)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RestoreListResponse {
    items: Vec<crate::host_registry::SyncRestorePlan>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteRootListResponse {
    items: Vec<crate::host_registry::RemoteLibraryRootSummary>,
    next_cursor: Option<String>,
    inventory_fence: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConflictQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RootOperationQuery {
    limit: Option<usize>,
    cursor: Option<String>,
    state: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteRootQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncFolderComparisonItemsQuery {
    cursor: Option<u64>,
    limit: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncConflictRecord {
    conflict_uuid: String,
    entity_uuid: Option<String>,
    conflict_kind: String,
    state: String,
    details: Option<serde_json::Value>,
    created_at: i64,
    resolved_at: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncConflictListResponse {
    items: Vec<SyncConflictRecord>,
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConflictResolutionRequest {
    operation_uuid: String,
    resolution: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderSettingsMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    default_placement_base: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SyncFolderPathMode {
    EnrollExisting,
    MaterializeRemote,
    Relocate,
    RebindExisting,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderPathPreflightMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: Option<u64>,
    root_uuid: Option<String>,
    mode: SyncFolderPathMode,
    path: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderEnrollmentMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    root_uuid: String,
    root_folder_uuid: String,
    preflight_token: String,
    path: String,
    requested_name: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderSelectionItemMutation {
    root_uuid: String,
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: Option<u64>,
    selected: bool,
    path: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderSelectionPreflightMutation {
    items: Vec<SyncFolderSelectionItemMutation>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderSelectionCommitMutation {
    preflight_token: String,
    items: Vec<SyncFolderSelectionItemMutation>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderRebindPreflightMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: Option<u64>,
    path: String,
    marker_identity: String,
    prior_cutover_snapshot_uuid: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderRebindCommitMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: Option<u64>,
    plan_uuid: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderOperationFenceMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderStopCommitMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: u64,
    plan_uuid: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SyncFolderComparisonMode {
    MergeLocalFork,
    PublishAsNew,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderReenableMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: u64,
    plan_uuid: String,
    mode: SyncFolderComparisonMode,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderRenameMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: Option<u64>,
    requested_name: String,
    expected_location_heads: Vec<String>,
    supersedes: Vec<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderRenameCommitMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: Option<u64>,
    requested_name: String,
    expected_location_heads: Vec<String>,
    supersedes: Vec<String>,
    plan_uuid: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SyncFolderRelocationMode {
    ExplicitMove,
    AutomaticRename,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderRelocationPreflightMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: u64,
    mode: SyncFolderRelocationMode,
    target_path: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderRelocationCommitMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: u64,
    preflight_token: String,
    mode: SyncFolderRelocationMode,
    target_path: String,
    desired_materialization_fingerprint: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SyncFolderComparisonResolution {
    KeepLocal,
    KeepRemote,
    KeepBoth,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderComparisonResolutionMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: u64,
    resolution: SyncFolderComparisonResolution,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncFolderConflictResolutionMutation {
    operation_uuid: String,
    expected_operation_revision: u64,
    expected_binding_generation: Option<u64>,
    expected_location_heads: Vec<String>,
    supersedes: Vec<String>,
    requested_name: Option<String>,
}

fn require_context_revision(headers: &HeaderMap) -> ApiResult<u64> {
    headers
        .get("x-mangoo-context-revision")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            CoreError::coded(
                StatusCode::CONFLICT,
                "stale_library_context",
                "Missing active Library context revision".to_string(),
            )
        })
}

fn library_manager(state: &AppState) -> ApiResult<Arc<crate::library_manager::LibraryManager>> {
    state.library_manager.clone().ok_or_else(|| {
        CoreError::coded(
            StatusCode::SERVICE_UNAVAILABLE,
            "host_mode_required",
            "Library management is unavailable outside Core Host mode".to_string(),
        )
    })
}

fn sync_folder_library_manager(
    state: &AppState,
) -> ApiResult<Arc<crate::library_manager::LibraryManager>> {
    state.library_manager.clone().ok_or_else(|| {
        CoreError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal-error",
            "Synchronized-folder management requires Core Host mode".to_string(),
        )
    })
}

fn validate_contract_uuid(value: &str, field: &str) -> ApiResult<()> {
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| {
        CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid-input",
            format!("{field} must be a canonical UUID"),
        )
    })?;
    if parsed.to_string() != value {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid-input",
            format!("{field} must be a lowercase canonical UUID"),
        ));
    }
    Ok(())
}

fn deserialize_sync_folder_body<T: for<'de> Deserialize<'de>>(
    body: &serde_json::Value,
) -> ApiResult<T> {
    serde_json::from_value(body.clone()).map_err(|error| {
        CoreError::coded(StatusCode::BAD_REQUEST, "invalid-input", error.to_string())
    })
}

fn validate_optional_contract_uuid_field(body: &serde_json::Value, field: &str) -> ApiResult<()> {
    if let Some(value) = body.get(field).filter(|value| !value.is_null()) {
        let value = value.as_str().ok_or_else(|| {
            CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid-input",
                format!("{field} must be a UUID string"),
            )
        })?;
        validate_contract_uuid(value, field)?;
    }
    Ok(())
}

fn validate_required_contract_uuid_field(body: &serde_json::Value, field: &str) -> ApiResult<()> {
    let value = body
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid-input",
                format!("{field} is required"),
            )
        })?;
    validate_contract_uuid(value, field)
}

fn validate_sync_folder_write_shape(path: &str, body: &serde_json::Value) -> ApiResult<()> {
    if path.ends_with("/settings") {
        let _: SyncFolderSettingsMutation = deserialize_sync_folder_body(body)?;
    } else if path.ends_with("/enrollment/preflight") {
        let _: SyncFolderPathPreflightMutation = deserialize_sync_folder_body(body)?;
        validate_optional_contract_uuid_field(body, "rootUuid")?;
    } else if path.ends_with("/enrollment/commit") {
        let request: SyncFolderEnrollmentMutation = deserialize_sync_folder_body(body)?;
        if request.requested_name.is_empty() {
            return Err(CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid-input",
                "requestedName must not be empty".to_string(),
            ));
        }
        for field in ["rootUuid", "rootFolderUuid", "preflightToken"] {
            validate_required_contract_uuid_field(body, field)?;
        }
    } else if path.ends_with("/selection/preflight") {
        let _: SyncFolderSelectionPreflightMutation = deserialize_sync_folder_body(body)?;
    } else if path.ends_with("/selection/commit") {
        let _: SyncFolderSelectionCommitMutation = deserialize_sync_folder_body(body)?;
        validate_required_contract_uuid_field(body, "preflightToken")?;
    } else {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid-input",
            "Unknown synchronized-folder mutation".to_string(),
        ));
    }
    Ok(())
}

fn validate_sync_folder_root_write_shape(path: &str, body: &serde_json::Value) -> ApiResult<()> {
    if path.ends_with("/rebind/preflight") {
        let _: SyncFolderRebindPreflightMutation = deserialize_sync_folder_body(body)?;
        validate_required_contract_uuid_field(body, "priorCutoverSnapshotUuid")?;
    } else if path.ends_with("/rebind/commit") {
        let _: SyncFolderRebindCommitMutation = deserialize_sync_folder_body(body)?;
        validate_required_contract_uuid_field(body, "planUuid")?;
    } else if path.ends_with("/reenable/preflight") {
        let _: SyncFolderOperationFenceMutation = deserialize_sync_folder_body(body)?;
    } else if path.ends_with("/stop/preflight") {
        let _: SyncFolderOperationFenceMutation = deserialize_sync_folder_body(body)?;
    } else if path.ends_with("/stop/commit") {
        let _: SyncFolderStopCommitMutation = deserialize_sync_folder_body(body)?;
        validate_required_contract_uuid_field(body, "planUuid")?;
    } else if path.ends_with("/reenable/commit") {
        let _: SyncFolderReenableMutation = deserialize_sync_folder_body(body)?;
        validate_required_contract_uuid_field(body, "planUuid")?;
    } else if path.ends_with("/rename/preflight") {
        let request: SyncFolderRenameMutation = deserialize_sync_folder_body(body)?;
        if request.requested_name.is_empty() {
            return Err(CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid-input",
                "requestedName must not be empty".to_string(),
            ));
        }
    } else if path.ends_with("/rename/commit") {
        let request: SyncFolderRenameCommitMutation = deserialize_sync_folder_body(body)?;
        if request.requested_name.is_empty() {
            return Err(CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid-input",
                "requestedName must not be empty".to_string(),
            ));
        }
        validate_required_contract_uuid_field(body, "planUuid")?;
    } else if path.ends_with("/relocations/preflight") {
        let _: SyncFolderRelocationPreflightMutation = deserialize_sync_folder_body(body)?;
    } else if path.ends_with("/relocations") {
        let request: SyncFolderRelocationCommitMutation = deserialize_sync_folder_body(body)?;
        validate_required_contract_uuid_field(body, "preflightToken")?;
        if request.desired_materialization_fingerprint.len() != 64 {
            return Err(CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid-input",
                "desiredMaterializationFingerprint must contain 64 characters".to_string(),
            ));
        }
    } else {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid-input",
            "Unknown synchronized-folder root mutation".to_string(),
        ));
    }
    Ok(())
}

fn validate_sync_folder_mutation_body(
    body: &serde_json::Value,
    location_heads_required: bool,
) -> ApiResult<()> {
    let object = body.as_object().ok_or_else(|| {
        CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid-input",
            "Request body must be an object".to_string(),
        )
    })?;
    let operation_uuid = object
        .get("operationUuid")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid-input",
                "operationUuid is required".to_string(),
            )
        })?;
    validate_contract_uuid(operation_uuid, "operationUuid")?;
    if object
        .get("expectedOperationRevision")
        .and_then(serde_json::Value::as_u64)
        .is_none()
    {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid-input",
            "expectedOperationRevision is required".to_string(),
        ));
    }
    if object.contains_key("expectedBindingGeneration")
        && !matches!(
            object.get("expectedBindingGeneration"),
            Some(serde_json::Value::Null)
        )
        && !object
            .get("expectedBindingGeneration")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|value| value >= 1)
    {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid-input",
            "expectedBindingGeneration must be a positive integer or null".to_string(),
        ));
    }
    if location_heads_required {
        let mut revision_sets = Vec::with_capacity(2);
        for field in ["expectedLocationHeads", "supersedes"] {
            let values = object
                .get(field)
                .and_then(serde_json::Value::as_array)
                .filter(|values| !values.is_empty())
                .ok_or_else(|| {
                    CoreError::coded(
                        StatusCode::BAD_REQUEST,
                        "invalid-input",
                        format!("{field} must contain at least one live location revision"),
                    )
                })?;
            for value in values {
                let value = value.as_str().ok_or_else(|| {
                    CoreError::coded(
                        StatusCode::BAD_REQUEST,
                        "invalid-input",
                        format!("{field} must contain UUID strings"),
                    )
                })?;
                validate_contract_uuid(value, field)?;
            }
            let revisions = values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<HashSet<_>>();
            if revisions.len() != values.len() {
                return Err(CoreError::coded(
                    StatusCode::BAD_REQUEST,
                    "invalid-input",
                    format!("{field} must not contain duplicate revisions"),
                ));
            }
            revision_sets.push(revisions);
        }
        if revision_sets[0] != revision_sets[1] {
            return Err(CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid-input",
                "supersedes must match the expected live location heads".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_sync_folder_batch_body(body: &serde_json::Value) -> ApiResult<()> {
    let items = body
        .get("items")
        .and_then(serde_json::Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| {
            CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid-input",
                "items must contain at least one synchronized folder".to_string(),
            )
        })?;
    for item in items {
        validate_sync_folder_mutation_body(item, false)?;
        let root_uuid = item
            .get("rootUuid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                CoreError::coded(
                    StatusCode::BAD_REQUEST,
                    "invalid-input",
                    "rootUuid is required for every selection item".to_string(),
                )
            })?;
        validate_contract_uuid(root_uuid, "rootUuid")?;
    }
    Ok(())
}

fn require_sync_folder_binding_generation(
    body: &serde_json::Value,
    operation: &str,
) -> ApiResult<()> {
    if body
        .get("expectedBindingGeneration")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|value| value >= 1)
    {
        return Ok(());
    }
    Err(CoreError::coded(
        StatusCode::BAD_REQUEST,
        "invalid-input",
        format!("expectedBindingGeneration is required for {operation}"),
    ))
}

fn root_operation_json(operation: &crate::host_registry::RootOperationRecord) -> serde_json::Value {
    serde_json::json!({
        "operationUuid":operation.operation_uuid,
        "libraryUuid":operation.library_uuid,
        "rootUuid":operation.root_uuid,
        "kind":operation.kind,
        "state":operation.state,
        "phase":operation.phase,
        "operationRevision":operation.operation_revision,
        "cutoverCommitted":operation.cutover_committed,
        "errorCode":operation.error_code,
        "transferMode":serde_json::Value::Null,
        "bytesDone":serde_json::Value::Null,
        "bytesTotal":serde_json::Value::Null,
        "itemsDone":serde_json::Value::Null,
        "itemsTotal":serde_json::Value::Null,
        "availableBytes":serde_json::Value::Null,
        "canCancel":false,
        "cancellationBoundary":serde_json::Value::Null,
        "recoveryState":serde_json::Value::Null,
        "createdAt":operation.created_at,
        "updatedAt":operation.updated_at,
    })
}

fn root_operation_detail_json(
    registry: &crate::host_registry::HostRegistry,
    operation: &crate::host_registry::RootOperationRecord,
) -> anyhow::Result<serde_json::Value> {
    let relocation = registry.root_relocation_operation(&operation.operation_uuid)?;
    let result = registry.root_operation_result(&operation.operation_uuid)?;
    let transfer_progress = result
        .as_ref()
        .and_then(|value| value.get("transferProgress"));
    let evidence = result.as_ref();
    let mut value = root_operation_json(operation);
    value["transferMode"] = serde_json::json!(relocation.as_ref().map(|item| item.mode.as_str()));
    value["bytesDone"] = serde_json::json!(relocation.as_ref().map(|item| item.transferred_bytes));
    value["bytesTotal"] = serde_json::json!(evidence
        .and_then(|item| item.get("sourceBytes"))
        .and_then(serde_json::Value::as_u64));
    value["itemsDone"] = serde_json::json!(transfer_progress
        .and_then(|item| item.get("completedEntries"))
        .and_then(serde_json::Value::as_array)
        .map(|items| items.len()));
    value["itemsTotal"] = serde_json::json!(transfer_progress
        .and_then(|item| item.get("totalEntries"))
        .and_then(serde_json::Value::as_u64));
    value["availableBytes"] = serde_json::json!(evidence
        .and_then(|item| item.get("availableBytes"))
        .and_then(serde_json::Value::as_u64));
    value["canCancel"] = serde_json::json!(
        relocation.is_some()
            && matches!(operation.state.as_str(), "pending" | "running" | "waiting")
            && !operation.cutover_committed
    );
    value["cancellationBoundary"] = serde_json::json!(if operation.kind == "relocate"
        || operation.kind == "automatic-rename"
    {
        Some(if operation.cutover_committed {
            "after-cutover"
        } else {
            "before-cutover"
        })
    } else {
        None
    });
    value["recoveryState"] =
        serde_json::json!(relocation.as_ref().map(|item| item.cleanup_state.as_str()));
    Ok(value)
}

fn sync_folder_allowed_actions(
    descriptor: &crate::host_registry::RemoteSyncRootRecord,
    binding: Option<&crate::host_registry::LocalRootBindingRecord>,
) -> Vec<&'static str> {
    match binding.map(|value| value.state.as_str()) {
        None if descriptor.lifecycle == "active" => vec!["select", "rebind-existing"],
        Some("active") => vec!["pause", "stop-sync", "relocate", "rename"],
        Some("paused") => vec!["resume", "stop-sync", "relocate", "rename"],
        Some("local-only-fork") => vec!["re-enable", "publish-as-new"],
        Some("reconcile-required") => vec!["retry"],
        Some("local-rename-required") => vec!["retry", "relocate"],
        Some("materializing") => vec!["retry"],
        Some("unavailable") => vec!["rebind-existing", "retry"],
        Some("error") => vec!["retry"],
        _ => Vec::new(),
    }
}

async fn sync_folder_record_json(
    manager: &crate::library_manager::LibraryManager,
    descriptor: &crate::host_registry::RemoteSyncRootRecord,
    settings: &crate::host_registry::SyncFolderSettingsRecord,
) -> anyhow::Result<serde_json::Value> {
    let binding = manager
        .registry()
        .local_root_binding(&descriptor.library_uuid, &descriptor.root_uuid)?;
    let proposal = FilePath::new(&settings.default_placement_base)
        .join(&descriptor.materialized_name)
        .to_string_lossy()
        .into_owned();
    let mut allowed_actions = sync_folder_allowed_actions(descriptor, binding.as_ref());
    let rebind_evidence = manager
        .registry()
        .root_rebind_evidence(&descriptor.library_uuid, &descriptor.root_uuid)?;
    if rebind_evidence.is_none() {
        allowed_actions.retain(|action| *action != "rebind-existing");
    }
    let location_heads = manager
        .sync_folder_location_heads(
            &descriptor.library_uuid,
            uuid::Uuid::parse_str(&descriptor.root_uuid)?,
        )
        .await?;
    let latest_operations = manager.registry().root_operations(
        &descriptor.library_uuid,
        &descriptor.root_uuid,
        None,
        1,
        "all",
    )?;
    let active_operations = manager.registry().root_operations(
        &descriptor.library_uuid,
        &descriptor.root_uuid,
        None,
        1,
        "active",
    )?;
    let latest_operation = latest_operations
        .first()
        .map(|operation| root_operation_detail_json(manager.registry(), operation))
        .transpose()?;
    let active_operation = active_operations
        .first()
        .map(|operation| root_operation_detail_json(manager.registry(), operation))
        .transpose()?;
    Ok(serde_json::json!({
        "descriptor":descriptor,
        "binding":binding,
        "allowedActions":allowed_actions,
        "selectedByDefault":binding.is_none() && descriptor.lifecycle=="active",
        "proposedLocalPath":proposal,
        "locationHeads":location_heads,
        "rebindEvidence":rebind_evidence,
        "lastSyncedAt":manager.registry().library_sync_last_success_at(&descriptor.library_uuid)?,
        "activeOperation":active_operation,
        "latestOperation":latest_operation,
    }))
}

fn sync_folder_error(error: anyhow::Error) -> CoreError {
    let message = error.to_string();
    let mapped = [
        ("stale-library-context", "stale_library_context"),
        ("stale-operation-revision", "stale-operation-revision"),
        ("stale-remote-root-revision", "stale-operation-revision"),
        ("stale-binding-generation", "stale-binding-generation"),
        ("stale-location-heads", "stale-operation-revision"),
        ("idempotency-key-reused", "idempotency-key-reused"),
        ("comparison-required", "comparison-required"),
        ("comparison-plan-stale", "comparison-expired"),
        ("stale-plan", "comparison-expired"),
        ("operation-in-progress", "operation-in-progress"),
        ("root-operation-not-found", "operation-not-found"),
        ("comparison-plan-not-found", "operation-not-found"),
        ("comparison-item-not-found", "operation-not-found"),
        ("local-root-binding-not-found", "root-not-found"),
        ("root-not-found", "root-not-found"),
        ("library-not-found", "library-not-found"),
        ("overlaps-active-root", "path-overlap"),
        ("root-overlap", "path-overlap"),
        ("placement-base-selected", "path-overlap"),
        ("path-changed", "path-occupied"),
        ("marker-mismatch", "path-occupied"),
        ("directory-not-empty", "path-occupied"),
        ("destination-exists", "path-occupied"),
        ("target-conflict", "path-occupied"),
        ("source-missing", "invalid-lifecycle-transition"),
        ("cutover-committed", "cutover-committed"),
        ("insufficient-space", "path-occupied"),
        ("root-not-active", "invalid-lifecycle-transition"),
        ("root-already-selected", "invalid-lifecycle-transition"),
        ("invalid-root-state", "invalid-lifecycle-transition"),
        (
            "comparison-resolution-not-required",
            "invalid-lifecycle-transition",
        ),
        ("invalid-path", "invalid-path"),
        ("path-unavailable", "invalid-path"),
        ("path-not-writable", "invalid-path"),
        ("cloud-sync-root", "invalid-path"),
        ("reparse-point", "invalid-path"),
        ("directory-inventory-too-large", "invalid-path"),
        ("duplicate-root", "invalid-input"),
        ("duplicate-operation", "invalid-input"),
    ];
    let Some((internal_code, response_code)) = mapped
        .iter()
        .copied()
        .find(|(internal_code, _)| message.contains(internal_code))
    else {
        tracing::error!(%error, "synchronized-folder orchestration failed");
        return CoreError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal-error",
            "Synchronized-folder operation failed".to_owned(),
        );
    };
    let status = if matches!(
        response_code,
        "library-not-found" | "root-not-found" | "operation-not-found"
    ) {
        StatusCode::NOT_FOUND
    } else if matches!(response_code, "invalid-input" | "invalid-path") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::CONFLICT
    };
    CoreError::coded(status, response_code, internal_code.replace('-', " "))
}

fn preflight_mode_name(
    mode: &SyncFolderPathMode,
) -> (&'static str, crate::sync_folders::RootPathMode) {
    match mode {
        SyncFolderPathMode::EnrollExisting => (
            "enroll-existing",
            crate::sync_folders::RootPathMode::EnrollExisting,
        ),
        SyncFolderPathMode::MaterializeRemote => (
            "materialize-remote",
            crate::sync_folders::RootPathMode::MaterializeRemote,
        ),
        SyncFolderPathMode::RebindExisting => (
            "rebind-existing",
            crate::sync_folders::RootPathMode::RebindExisting,
        ),
        SyncFolderPathMode::Relocate => (
            "relocate",
            crate::sync_folders::RootPathMode::MaterializeRemote,
        ),
    }
}

fn validated_path_result(
    root_uuid: Option<&str>,
    path: &crate::sync_folders::ValidatedRootPath,
    token: &str,
    expires_at: i64,
    materialized_name: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({"rootUuid":root_uuid,"ok":true,"reason":"ok","normalizedPath":path.normalized_path,"proposedMaterializedName":materialized_name,"preflightToken":token,"expiresAt":expires_at,"availableBytes":path.available_bytes,"pathEvidence":path})
}

fn sync_folder_v2_gate(state: &AppState, library_uuid: &str) -> ApiResult<()> {
    validate_contract_uuid(library_uuid, "libraryUuid")?;
    let manager = sync_folder_library_manager(state)?;
    if manager.registry().library(library_uuid)?.is_none() {
        return Err(CoreError::coded(
            StatusCode::NOT_FOUND,
            "library-not-found",
            "Library not found".to_string(),
        ));
    }
    if !manager.registry().sync_folder_v2_enabled() {
        return Err(CoreError::coded(
            StatusCode::CONFLICT,
            "migration-required",
            "Synchronized-folder v2 is disabled until its runtime dependencies are installed"
                .to_string(),
        ));
    }
    Ok(())
}

async fn sync_folder_contract_read(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    OriginalUri(uri): OriginalUri,
) -> ApiResult<Response> {
    sync_folder_v2_gate(&state, &library_uuid)?;
    let manager = sync_folder_library_manager(&state)?;
    let settings = manager
        .registry()
        .sync_folder_settings(&library_uuid)
        .map_err(sync_folder_error)?;
    if uri.path().ends_with("/settings") {
        return Ok(Json(settings).into_response());
    }
    let roots = manager
        .registry()
        .remote_sync_roots(&library_uuid)
        .map_err(sync_folder_error)?;
    let mut items = Vec::with_capacity(roots.len());
    for root in &roots {
        items.push(
            sync_folder_record_json(&manager, root, &settings)
                .await
                .map_err(sync_folder_error)?,
        );
    }
    Ok(
        Json(serde_json::json!({"revision":settings.operation_revision,"items":items}))
            .into_response(),
    )
}

async fn sync_folder_contract_write(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> ApiResult<Response> {
    let Json(body) = body.map_err(|error| {
        CoreError::coded(StatusCode::BAD_REQUEST, "invalid-input", error.body_text())
    })?;
    validate_sync_folder_write_shape(uri.path(), &body)?;
    if uri.path().contains("/selection/") {
        validate_sync_folder_batch_body(&body)?;
    } else {
        validate_sync_folder_mutation_body(&body, false)?;
    }
    if uri.path().ends_with("/settings") || uri.path().ends_with("/commit") {
        let expected = require_context_revision(&headers)?;
        sync_folder_library_manager(&state)?
            .ensure_context_revision(expected)
            .map_err(map_library_error)?;
    }
    sync_folder_v2_gate(&state, &library_uuid)?;
    let manager = sync_folder_library_manager(&state)?;
    if uri.path().ends_with("/settings") {
        let request: SyncFolderSettingsMutation = deserialize_sync_folder_body(&body)?;
        let settings = manager
            .registry()
            .update_sync_folder_settings(
                &library_uuid,
                request.expected_operation_revision,
                FilePath::new(&request.default_placement_base),
            )
            .map_err(sync_folder_error)?;
        return Ok(Json(settings).into_response());
    }
    let settings = manager
        .registry()
        .sync_folder_settings(&library_uuid)
        .map_err(sync_folder_error)?;
    if uri.path().ends_with("/enrollment/preflight") {
        let _admission = manager.path_admission().await;
        let request: SyncFolderPathPreflightMutation = deserialize_sync_folder_body(&body)?;
        let (mode_name, mode) = preflight_mode_name(&request.mode);
        if mode != crate::sync_folders::RootPathMode::EnrollExisting {
            return Err(CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid-input",
                "Enrollment preflight requires enroll-existing mode".to_owned(),
            ));
        }
        let token = request.operation_uuid.clone();
        let request_hash = crate::sync_folders::request_hash(&body).map_err(sync_folder_error)?;
        if let Ok((_root, stored_mode, stored_hash, _request, result, expires_at, _)) = manager
            .registry()
            .root_path_preflight(&token, &library_uuid)
        {
            if stored_mode != mode_name || stored_hash != request_hash {
                return Err(sync_folder_error(anyhow::anyhow!("idempotency-key-reused")));
            }
            let operation_started = manager
                .registry()
                .root_operation(&token)
                .map_err(sync_folder_error)?
                .is_some_and(|operation| {
                    operation.kind == "enroll"
                        && matches!(operation.state.as_str(), "pending" | "running" | "waiting")
                });
            if expires_at < crate::host_registry::unix_timestamp_millis_for_runtime()
                && !operation_started
            {
                return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
            }
            let mut result = result;
            if operation_started
                && expires_at < crate::host_registry::unix_timestamp_millis_for_runtime()
            {
                let refreshed_expires_at =
                    crate::host_registry::unix_timestamp_millis_for_runtime()
                        + crate::sync_folders::PLAN_TTL_MILLIS;
                manager
                    .registry()
                    .refresh_started_root_path_preflight(
                        &token,
                        &library_uuid,
                        refreshed_expires_at,
                    )
                    .map_err(sync_folder_error)?;
                result["expiresAt"] = serde_json::json!(refreshed_expires_at);
            }
            return Ok(Json(result).into_response());
        }
        let bindings = manager
            .registry()
            .local_root_bindings(&library_uuid)
            .map_err(sync_folder_error)?;
        let candidate_key = crate::sync_folders::canonical_path_key(FilePath::new(&request.path));
        let path_binding = manager
            .registry()
            .local_root_binding_for_canonical_path(&candidate_key)
            .map_err(sync_folder_error)?;
        if path_binding
            .as_ref()
            .is_some_and(|binding| binding.library_uuid != library_uuid)
        {
            return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
        }
        let matching_binding = path_binding.as_ref();
        let recovery_marker = if matching_binding.is_none()
            && FilePath::new(&request.path)
                .join(crate::sync_folders::ROOT_MARKER_FILE_NAME)
                .is_file()
        {
            Some(
                crate::sync_folders::read_root_marker(FilePath::new(&request.path))
                    .map_err(sync_folder_error)?,
            )
        } else {
            None
        };
        let paths = bindings
            .iter()
            .filter(|binding| {
                matching_binding.is_none_or(|matching| {
                    binding.local_binding_uuid != matching.local_binding_uuid
                })
            })
            .map(|value| std::path::PathBuf::from(&value.local_path))
            .collect::<Vec<_>>();
        manager
            .admit_content_path(FilePath::new(&request.path), true)
            .map_err(sync_folder_error)?;
        let validated = crate::sync_folders::validate_root_path(
            FilePath::new(&request.path),
            if matching_binding.is_some() || recovery_marker.is_some() {
                crate::sync_folders::RootPathMode::RebindExisting
            } else {
                mode
            },
            FilePath::new(&settings.default_placement_base),
            &paths,
        )
        .map_err(sync_folder_error)?;
        let identity_bindings = validated
            .filesystem_identity
            .as_deref()
            .map(|identity| {
                manager
                    .registry()
                    .local_root_bindings_for_filesystem_identity(identity)
            })
            .transpose()
            .map_err(sync_folder_error)?
            .unwrap_or_default();
        if identity_bindings.iter().any(|identity_binding| {
            matching_binding.is_none_or(|binding| {
                binding.local_binding_uuid != identity_binding.local_binding_uuid
            })
        }) {
            return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
        }
        if manager
            .other_library_sync_root_identity_for_path(
                &library_uuid,
                validated.normalized_path.clone(),
            )
            .await
            .map_err(sync_folder_error)?
            .is_some()
        {
            return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
        }
        let existing_identity = manager
            .sync_root_identity_for_path(&library_uuid, validated.normalized_path.clone())
            .await
            .map_err(sync_folder_error)?;
        if let Some(requested_root_uuid) = request.root_uuid.as_deref() {
            let discovered_root_uuid = existing_identity
                .as_ref()
                .map(|(root_uuid, _)| root_uuid.to_string())
                .or_else(|| {
                    recovery_marker
                        .as_ref()
                        .map(|marker| marker.root_uuid.clone())
                });
            if discovered_root_uuid
                .as_deref()
                .is_some_and(|root_uuid| root_uuid != requested_root_uuid)
            {
                return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
            }
        }
        let root_uuid = existing_identity
            .as_ref()
            .map(|(root_uuid, _)| root_uuid.to_string())
            .or_else(|| {
                recovery_marker
                    .as_ref()
                    .map(|marker| marker.root_uuid.clone())
            })
            .or(request.root_uuid)
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        if let Some(binding) = matching_binding {
            if binding.root_uuid != root_uuid {
                return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
            }
            let marker =
                crate::sync_folders::read_root_marker(FilePath::new(&validated.normalized_path))
                    .map_err(sync_folder_error)?;
            if marker.library_uuid != library_uuid
                || marker.root_uuid != root_uuid
                || marker.local_binding_uuid != binding.local_binding_uuid
                || marker.binding_generation != binding.binding_generation
            {
                return Err(sync_folder_error(anyhow::anyhow!("marker-mismatch")));
            }
        }
        if let Some(operation) = manager
            .registry()
            .latest_root_operation_of_kind(&library_uuid, &root_uuid, "enroll")
            .map_err(sync_folder_error)?
            .filter(|operation| {
                matches!(operation.state.as_str(), "pending" | "running" | "waiting")
            })
        {
            let stored_request: SyncFolderEnrollmentMutation =
                serde_json::from_str(&operation.request_json)
                    .map_err(|error| sync_folder_error(error.into()))?;
            if crate::sync_folders::canonical_path_key(FilePath::new(&stored_request.path))
                != validated.canonical_path_key
            {
                return Err(sync_folder_error(anyhow::anyhow!("operation-in-progress")));
            }
            let (_, stored_mode, _, _, mut result, expires_at, _) = manager
                .registry()
                .root_path_preflight(&stored_request.preflight_token, &library_uuid)
                .map_err(sync_folder_error)?;
            if stored_mode != mode_name {
                return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
            }
            if let Some(marker) = recovery_marker.as_ref() {
                let expected_root_folder_uuid = existing_identity
                    .as_ref()
                    .map(|(_, root_folder_uuid)| root_folder_uuid.to_string())
                    .unwrap_or_else(|| stored_request.root_folder_uuid.clone());
                if marker.library_uuid != library_uuid
                    || marker.root_uuid != operation.root_uuid
                    || marker.root_folder_uuid != expected_root_folder_uuid
                    || marker.local_binding_uuid
                        != result
                            .get("bindingUuid")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                    || marker.binding_generation != 1
                {
                    return Err(sync_folder_error(anyhow::anyhow!("marker-mismatch")));
                }
            }
            if expires_at < crate::host_registry::unix_timestamp_millis_for_runtime() {
                let refreshed_expires_at =
                    crate::host_registry::unix_timestamp_millis_for_runtime()
                        + crate::sync_folders::PLAN_TTL_MILLIS;
                manager
                    .registry()
                    .refresh_started_root_path_preflight(
                        &stored_request.preflight_token,
                        &library_uuid,
                        refreshed_expires_at,
                    )
                    .map_err(sync_folder_error)?;
                result["expiresAt"] = serde_json::json!(refreshed_expires_at);
            }
            return Ok(Json(result).into_response());
        }
        if recovery_marker.is_some() {
            return Err(sync_folder_error(anyhow::anyhow!("marker-mismatch")));
        }
        let occupied = manager
            .registry()
            .remote_sync_roots(&library_uuid)
            .map_err(sync_folder_error)?
            .into_iter()
            .filter(|item| item.root_uuid != root_uuid)
            .map(|item| {
                crate::sync::reducer::materialized_name_compare_key(&item.materialized_name)
            })
            .collect::<std::collections::BTreeSet<_>>();
        let requested_name = FilePath::new(&validated.normalized_path)
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("invalid-path")))?;
        let materialized_name = crate::sync_folders::default_materialized_name(
            requested_name,
            uuid::Uuid::parse_str(&root_uuid).map_err(|error| sync_folder_error(error.into()))?,
            &occupied,
        );
        let expires_at = crate::host_registry::unix_timestamp_millis_for_runtime()
            + crate::sync_folders::PLAN_TTL_MILLIS;
        let mut result = validated_path_result(
            Some(&root_uuid),
            &validated,
            &token,
            expires_at,
            Some(&materialized_name),
        );
        result["bindingUuid"] = serde_json::json!(matching_binding
            .map(|binding| binding.local_binding_uuid.clone())
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string()));
        manager
            .registry()
            .store_root_path_preflight(
                &token,
                &library_uuid,
                Some(&root_uuid),
                mode_name,
                &request_hash,
                &body,
                &result,
                expires_at,
            )
            .map_err(sync_folder_error)?;
        return Ok(Json(result).into_response());
    }
    if uri.path().ends_with("/enrollment/commit") {
        let _admission = manager.path_admission().await;
        manager
            .ensure_context_revision(require_context_revision(&headers)?)
            .map_err(map_library_error)?;
        let submitted_request: SyncFolderEnrollmentMutation = deserialize_sync_folder_body(&body)?;
        let submitted_preflight_token = submitted_request.preflight_token.clone();
        let exact_operation = manager
            .registry()
            .root_operation(&submitted_request.operation_uuid)
            .map_err(sync_folder_error)?;
        let existing_binding = manager
            .registry()
            .local_root_binding(&library_uuid, &submitted_request.root_uuid)
            .map_err(sync_folder_error)?;
        if let Some(operation) = exact_operation
            .as_ref()
            .filter(|operation| operation.state == "completed")
        {
            let request_hash =
                crate::sync_folders::request_hash(&body).map_err(sync_folder_error)?;
            if operation.library_uuid != library_uuid
                || operation.root_uuid != submitted_request.root_uuid
                || operation.kind != "enroll"
                || operation.request_hash != request_hash
            {
                return Err(sync_folder_error(anyhow::anyhow!("idempotency-key-reused")));
            }
            return Ok((StatusCode::ACCEPTED, Json(root_operation_json(operation))).into_response());
        }
        let coordinated_operation = if exact_operation.is_none() {
            manager
                .registry()
                .latest_root_operation_of_kind(
                    &library_uuid,
                    &submitted_request.root_uuid,
                    "enroll",
                )
                .map_err(sync_folder_error)?
                .filter(|operation| {
                    matches!(operation.state.as_str(), "pending" | "running" | "waiting")
                        || (operation.state == "completed"
                            && existing_binding.as_ref().is_some_and(|binding| {
                                binding.state == "local-rename-required"
                                    || binding.applied_materialized_name.is_none()
                                    || binding.applied_materialization_fingerprint.is_none()
                            }))
                })
        } else {
            None
        };
        let existing_operation = exact_operation.or(coordinated_operation);
        let (request, operation_body) = if let Some(operation) = existing_operation.as_ref() {
            let stored_request: SyncFolderEnrollmentMutation =
                serde_json::from_str(&operation.request_json)
                    .map_err(|error| sync_folder_error(error.into()))?;
            let same_request = operation.operation_uuid == submitted_request.operation_uuid;
            if same_request {
                let request_hash =
                    crate::sync_folders::request_hash(&body).map_err(sync_folder_error)?;
                if operation.library_uuid != library_uuid
                    || operation.root_uuid != submitted_request.root_uuid
                    || operation.kind != "enroll"
                    || operation.request_hash != request_hash
                {
                    return Err(sync_folder_error(anyhow::anyhow!("idempotency-key-reused")));
                }
            } else if operation.library_uuid != library_uuid
                || operation.root_uuid != submitted_request.root_uuid
                || crate::sync_folders::canonical_path_key(FilePath::new(&stored_request.path))
                    != crate::sync_folders::canonical_path_key(FilePath::new(
                        &submitted_request.path,
                    ))
                || stored_request.requested_name != submitted_request.requested_name
            {
                return Err(sync_folder_error(anyhow::anyhow!("operation-in-progress")));
            }
            let stored_body = serde_json::from_str(&operation.request_json)
                .map_err(|error| sync_folder_error(error.into()))?;
            (stored_request, stored_body)
        } else {
            (submitted_request, body)
        };
        if let Some(operation) = existing_operation.as_ref() {
            if operation.library_uuid != library_uuid
                || operation.root_uuid != request.root_uuid
                || operation.kind != "enroll"
            {
                return Err(sync_folder_error(anyhow::anyhow!("idempotency-key-reused")));
            }
        }
        let resuming_unfinished_operation = existing_operation.as_ref().is_some_and(|operation| {
            matches!(operation.state.as_str(), "pending" | "running" | "waiting")
        });
        let validation_preflight_token = if existing_operation
            .as_ref()
            .is_some_and(|operation| operation.state == "completed")
        {
            &submitted_preflight_token
        } else {
            &request.preflight_token
        };
        let (planned_root, mode, _hash, planned, result, expires_at, _) = manager
            .registry()
            .root_path_preflight(validation_preflight_token, &library_uuid)
            .map_err(sync_folder_error)?;
        if mode != "enroll-existing"
            || planned_root.as_deref() != Some(&request.root_uuid)
            || (expires_at < crate::host_registry::unix_timestamp_millis_for_runtime()
                && !resuming_unfinished_operation)
        {
            return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
        }
        let planned_request: SyncFolderPathPreflightMutation =
            serde_json::from_value(planned).map_err(|error| sync_folder_error(error.into()))?;
        if crate::sync_folders::canonical_path_key(FilePath::new(&planned_request.path))
            != crate::sync_folders::canonical_path_key(FilePath::new(&request.path))
        {
            return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
        }
        let evidence: crate::sync_folders::ValidatedRootPath = serde_json::from_value(
            result
                .get("pathEvidence")
                .cloned()
                .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?,
        )
        .map_err(|error| sync_folder_error(error.into()))?;
        let identity_bindings = evidence
            .filesystem_identity
            .as_deref()
            .map(|identity| {
                manager
                    .registry()
                    .local_root_bindings_for_filesystem_identity(identity)
            })
            .transpose()
            .map_err(sync_folder_error)?
            .unwrap_or_default();
        let evidence_path_key =
            crate::sync_folders::canonical_path_key(FilePath::new(&evidence.normalized_path));
        if identity_bindings.iter().any(|identity_binding| {
            identity_binding.library_uuid != library_uuid
                || identity_binding.root_uuid != request.root_uuid
                || identity_binding.canonical_path_key != evidence_path_key
        }) {
            return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
        }
        if manager
            .other_library_sync_root_identity_for_path(
                &library_uuid,
                evidence.normalized_path.clone(),
            )
            .await
            .map_err(sync_folder_error)?
            .is_some()
        {
            return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
        }
        let bindings = manager
            .registry()
            .local_root_bindings(&library_uuid)
            .map_err(sync_folder_error)?;
        let request_path_key =
            crate::sync_folders::canonical_path_key(FilePath::new(&request.path));
        let path_binding = manager
            .registry()
            .local_root_binding_for_canonical_path(&request_path_key)
            .map_err(sync_folder_error)?;
        if path_binding.as_ref().is_some_and(|binding| {
            binding.library_uuid != library_uuid || binding.root_uuid != request.root_uuid
        }) {
            return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
        }
        let matching_binding = path_binding.as_ref();
        let existing_identity = manager
            .sync_root_identity_for_path(&library_uuid, evidence.normalized_path.clone())
            .await
            .map_err(sync_folder_error)?;
        let root_uuid = uuid::Uuid::parse_str(&request.root_uuid)
            .map_err(|error| sync_folder_error(error.into()))?;
        let catalog_identity = existing_identity;
        let root_folder_uuid =
            if let Some((existing_root_uuid, existing_root_folder_uuid)) = catalog_identity {
                if existing_root_uuid != root_uuid {
                    return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
                }
                existing_root_folder_uuid
            } else if matching_binding.is_some() {
                let remote = manager
                    .registry()
                    .remote_sync_root(&library_uuid, &request.root_uuid)
                    .map_err(sync_folder_error)?
                    .ok_or_else(|| sync_folder_error(anyhow::anyhow!("path-changed")))?;
                uuid::Uuid::parse_str(&remote.root_folder_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?
            } else {
                uuid::Uuid::parse_str(&request.root_folder_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?
            };
        let root_folder_uuid_string = root_folder_uuid.to_string();
        let local_binding_uuid = result
            .get("bindingUuid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?
            .parse::<uuid::Uuid>()
            .map_err(|error| sync_folder_error(error.into()))?;
        if matching_binding
            .is_some_and(|binding| binding.local_binding_uuid != local_binding_uuid.to_string())
        {
            return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
        }
        let marker = crate::sync_folders::SyncRootMarker {
            format_version: 1,
            library_uuid: library_uuid.clone(),
            root_uuid: request.root_uuid.clone(),
            root_folder_uuid: root_folder_uuid_string.clone(),
            local_binding_uuid: local_binding_uuid.to_string(),
            binding_generation: 1,
        };
        // Publication can finish before the Host binding transaction. Only the
        // unfinished durable operation's exact marker may cross that boundary;
        // the guarded publisher verifies it again before any binding is committed.
        let recovering_published_marker = resuming_unfinished_operation
            && matching_binding.is_none()
            && FilePath::new(&request.path)
                .join(crate::sync_folders::ROOT_MARKER_FILE_NAME)
                .exists();
        if recovering_published_marker
            && crate::sync_folders::read_root_marker(FilePath::new(&request.path))
                .map_err(sync_folder_error)?
                != marker
        {
            return Err(sync_folder_error(anyhow::anyhow!("marker-mismatch")));
        }
        let replaying_owned_browse_root = resuming_unfinished_operation
            && request_path_key == evidence.canonical_path_key
            && (matching_binding.is_some()
                || existing_identity
                    .as_ref()
                    .is_some_and(|(existing_root_uuid, _)| *existing_root_uuid == root_uuid));
        manager
            .admit_content_path(FilePath::new(&request.path), true)
            .map_err(sync_folder_error)?;
        let current = crate::sync_folders::validate_root_path(
            FilePath::new(&request.path),
            if matching_binding.is_some() || recovering_published_marker {
                crate::sync_folders::RootPathMode::RebindExisting
            } else {
                crate::sync_folders::RootPathMode::EnrollExisting
            },
            FilePath::new(&settings.default_placement_base),
            &bindings
                .iter()
                .filter(|binding| {
                    matching_binding.is_none_or(|matching| {
                        binding.local_binding_uuid != matching.local_binding_uuid
                    })
                })
                .map(|value| std::path::PathBuf::from(&value.local_path))
                .collect::<Vec<_>>(),
        )
        .map_err(sync_folder_error)?;
        if current.identity_token != evidence.identity_token
            || current.canonical_path_key != evidence.canonical_path_key
            || (!replaying_owned_browse_root
                && current.content_evidence_digest != evidence.content_evidence_digest)
        {
            return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
        }
        let occupied = manager
            .registry()
            .remote_sync_roots(&library_uuid)
            .map_err(sync_folder_error)?
            .into_iter()
            .filter(|item| item.root_uuid != request.root_uuid)
            .map(|item| {
                crate::sync::reducer::materialized_name_compare_key(&item.materialized_name)
            })
            .collect::<std::collections::BTreeSet<_>>();
        let materialized = crate::sync_folders::default_materialized_name(
            &request.requested_name,
            root_uuid,
            &occupied,
        );
        let fingerprint = crate::sync_folders::materialization_fingerprint(&materialized);
        manager
            .registry()
            .upsert_remote_sync_root(
                &library_uuid,
                &request.root_uuid,
                &root_folder_uuid_string,
                &request.requested_name,
                &materialized,
                &fingerprint,
                0,
                "active",
                None,
                None,
            )
            .map_err(sync_folder_error)?;
        let request_hash =
            crate::sync_folders::request_hash(&operation_body).map_err(sync_folder_error)?;
        let (_operation, _) = manager
            .registry()
            .begin_root_operation_once(
                &request.operation_uuid,
                &library_uuid,
                &request.root_uuid,
                "enroll",
                "preflighting",
                &request_hash,
                &operation_body,
                request.expected_operation_revision,
                None,
            )
            .map_err(sync_folder_error)?;
        let plan =
            serde_json::json!({"preflightToken":request.preflight_token,"pathEvidence":current});
        manager
            .registry()
            .set_root_operation_plan(
                &request.operation_uuid,
                &request.preflight_token,
                &plan,
                "capturing-seed",
            )
            .map_err(sync_folder_error)?;
        let marker_operation_uuid = uuid::Uuid::parse_str(&request.operation_uuid)
            .map_err(|error| sync_folder_error(error.into()))?;
        let operation_temporary_marker = crate::sync_folders::root_marker_temporary_path(
            FilePath::new(&current.normalized_path),
            marker_operation_uuid,
        );
        if operation_temporary_marker.exists()
            && (!resuming_unfinished_operation
                || catalog_identity != Some((root_uuid, root_folder_uuid)))
        {
            return Err(sync_folder_error(anyhow::anyhow!("marker-mismatch")));
        }
        let marker_preexisted = FilePath::new(&current.normalized_path)
            .join(crate::sync_folders::ROOT_MARKER_FILE_NAME)
            .exists();
        crate::sync_folders::write_root_marker_for_operation(
            FilePath::new(&current.normalized_path),
            &marker,
            marker_operation_uuid,
            current
                .filesystem_identity
                .as_deref()
                .ok_or_else(|| sync_folder_error(anyhow::anyhow!("path-changed")))?,
        )
        .map_err(sync_folder_error)?;
        let marker_identity = match crate::library_path::local_file_identity(
            &FilePath::new(&current.normalized_path)
                .join(crate::sync_folders::ROOT_MARKER_FILE_NAME),
        ) {
            Ok(identity) => identity,
            Err(error) => {
                if !marker_preexisted {
                    let _ = crate::sync_folders::remove_root_marker_if_matches(
                        FilePath::new(&current.normalized_path),
                        &marker,
                        false,
                    );
                }
                return Err(sync_folder_error(error.into()));
            }
        };
        let binding = crate::host_registry::NewLocalRootBinding {
            library_uuid: library_uuid.clone(),
            root_uuid: request.root_uuid.clone(),
            local_binding_uuid: local_binding_uuid.to_string(),
            local_path: current.normalized_path.clone(),
            canonical_path_key: current.canonical_path_key.clone(),
            placement_parent: current.placement_parent.clone(),
            placement_parent_key: current.placement_parent_key.clone(),
            marker_identity,
            filesystem_identity: current.filesystem_identity.clone(),
            state: "active".to_owned(),
            binding_generation: 1,
            root_capture_epoch: matching_binding
                .and_then(|binding| binding.root_capture_epoch.clone())
                .or_else(|| Some(uuid::Uuid::now_v7().to_string())),
            root_actor_epoch: matching_binding
                .and_then(|binding| binding.root_actor_epoch.clone())
                .or_else(|| Some(uuid::Uuid::now_v7().to_string())),
            desired_materialized_name: materialized.clone(),
            desired_materialization_fingerprint: fingerprint.clone(),
            applied_materialized_name: Some(materialized.clone()),
            applied_materialization_fingerprint: Some(fingerprint.clone()),
            operation_revision: matching_binding
                .map(|binding| {
                    if binding.state == "local-rename-required"
                        || binding.applied_materialization_fingerprint.as_deref()
                            != Some(fingerprint.as_str())
                    {
                        binding.operation_revision.saturating_add(1)
                    } else {
                        binding.operation_revision
                    }
                })
                .unwrap_or(request.expected_operation_revision + 1),
            verification_json: plan,
        };
        let stored = match manager.registry().commit_local_root_binding(
            &request.operation_uuid,
            &request.preflight_token,
            &binding,
        ) {
            Ok(stored) => stored,
            Err(error) => {
                if !marker_preexisted {
                    let _ = crate::sync_folders::remove_root_marker_if_matches(
                        FilePath::new(&current.normalized_path),
                        &marker,
                        false,
                    );
                }
                return Err(sync_folder_error(error));
            }
        };
        manager
            .ensure_selected_sync_root(
                &library_uuid,
                root_uuid,
                root_folder_uuid,
                current.normalized_path.clone(),
                request.requested_name,
                materialized,
                crate::sync::media::parse_hash(&fingerprint).map_err(sync_folder_error)?,
                local_binding_uuid,
                uuid::Uuid::parse_str(
                    stored
                        .root_capture_epoch
                        .as_deref()
                        .unwrap_or(&stored.local_binding_uuid),
                )
                .map_err(|error| sync_folder_error(error.into()))?,
                uuid::Uuid::parse_str(
                    stored
                        .root_actor_epoch
                        .as_deref()
                        .unwrap_or(&stored.local_binding_uuid),
                )
                .map_err(|error| sync_folder_error(error.into()))?,
                stored.binding_generation,
                stored.state,
                stored.operation_revision,
                true,
            )
            .await
            .map_err(sync_folder_error)?;
        let completed = manager
            .registry()
            .complete_root_enrollment(
                &request.operation_uuid,
                &request.preflight_token,
                &request.preflight_token,
            )
            .map_err(sync_folder_error)?;
        if submitted_preflight_token != request.preflight_token {
            let _ = manager
                .registry()
                .consume_root_path_preflight(&submitted_preflight_token, &library_uuid);
        }
        manager.wake_sync_folder_workers();
        return Ok((StatusCode::ACCEPTED, Json(root_operation_json(&completed))).into_response());
    }
    if uri.path().ends_with("/selection/preflight") {
        let _admission = manager.path_admission().await;
        let request: SyncFolderSelectionPreflightMutation = deserialize_sync_folder_body(&body)?;
        let token = request.items[0].operation_uuid.clone();
        let request_hash = crate::sync_folders::request_hash(&body).map_err(sync_folder_error)?;
        if let Ok((_root, stored_mode, stored_hash, _request, result, expires_at, _)) = manager
            .registry()
            .root_path_preflight(&token, &library_uuid)
        {
            if stored_mode != "materialize-remote" || stored_hash != request_hash {
                return Err(sync_folder_error(anyhow::anyhow!("idempotency-key-reused")));
            }
            if expires_at < crate::host_registry::unix_timestamp_millis_for_runtime() {
                return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
            }
            return Ok(Json(result).into_response());
        }
        let existing = manager
            .registry()
            .local_root_bindings(&library_uuid)
            .map_err(sync_folder_error)?;
        let existing_paths = existing
            .iter()
            .map(|value| std::path::PathBuf::from(&value.local_path))
            .collect::<Vec<_>>();
        let mut proposed = Vec::new();
        let mut selected_keys = HashSet::new();
        let mut selected_paths: Vec<std::path::PathBuf> = Vec::new();
        let mut root_uuids = HashSet::new();
        let mut operation_uuids = HashSet::new();
        for item in &request.items {
            if !root_uuids.insert(item.root_uuid.clone()) {
                return Err(sync_folder_error(anyhow::anyhow!("duplicate-root")));
            }
            if !operation_uuids.insert(item.operation_uuid.clone()) {
                return Err(sync_folder_error(anyhow::anyhow!("duplicate-operation")));
            }
            let descriptor = manager
                .registry()
                .remote_sync_root(&library_uuid, &item.root_uuid)
                .map_err(sync_folder_error)?
                .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-not-found")))?;
            if descriptor.operation_revision != item.expected_operation_revision {
                return Err(sync_folder_error(anyhow::anyhow!(
                    "stale-operation-revision"
                )));
            }
            if !item.selected {
                proposed.push(serde_json::json!({"rootUuid":item.root_uuid,"ok":true,"reason":"ok","normalizedPath":item.path,"proposedMaterializedName":descriptor.materialized_name,"preflightToken":null,"expiresAt":null}));
                continue;
            }
            if FilePath::new(&item.path)
                .join(crate::sync_folders::ROOT_MARKER_FILE_NAME)
                .exists()
            {
                return Err(sync_folder_error(anyhow::anyhow!("marker-mismatch")));
            }
            if descriptor.lifecycle != "active" {
                return Err(sync_folder_error(anyhow::anyhow!("root-not-active")));
            }
            let existing_binding = manager
                .registry()
                .local_root_binding(&library_uuid, &item.root_uuid)
                .map_err(sync_folder_error)?;
            if existing_binding
                .as_ref()
                .is_some_and(|binding| binding.state == "local-only-fork")
            {
                return Err(sync_folder_error(anyhow::anyhow!("comparison-required")));
            }
            if existing_binding.is_some() {
                return Err(sync_folder_error(anyhow::anyhow!("root-already-selected")));
            }
            if manager
                .registry()
                .root_has_cutover_lineage(&library_uuid, &item.root_uuid)
                .map_err(sync_folder_error)?
            {
                return Err(sync_folder_error(anyhow::anyhow!("comparison-required")));
            }
            manager
                .admit_content_path(FilePath::new(&item.path), true)
                .map_err(sync_folder_error)?;
            let validated = crate::sync_folders::validate_root_path(
                FilePath::new(&item.path),
                crate::sync_folders::RootPathMode::MaterializeRemote,
                FilePath::new(&settings.default_placement_base),
                &existing_paths,
            )
            .map_err(sync_folder_error)?;
            if !selected_keys.insert(validated.canonical_path_key.clone()) {
                return Err(sync_folder_error(anyhow::anyhow!("destination-exists")));
            }
            let validated_path = std::path::PathBuf::from(&validated.normalized_path);
            if selected_paths
                .iter()
                .any(|path| crate::library_path::paths_overlap(&validated_path, path))
            {
                return Err(sync_folder_error(anyhow::anyhow!("root-overlap")));
            }
            if descriptor
                .logical_bytes
                .zip(validated.available_bytes)
                .is_some_and(|(required, available)| required > available)
            {
                return Err(sync_folder_error(anyhow::anyhow!("insufficient-space")));
            }
            selected_paths.push(validated_path);
            proposed.push(serde_json::json!({"rootUuid":item.root_uuid,"ok":true,"reason":"ok","normalizedPath":validated.normalized_path,"proposedMaterializedName":descriptor.materialized_name,"preflightToken":null,"expiresAt":null,"pathEvidence":validated,"remoteRevision":descriptor.operation_revision,"bindingUuid":uuid::Uuid::now_v7().to_string()}));
        }
        let expires_at = crate::host_registry::unix_timestamp_millis_for_runtime()
            + crate::sync_folders::PLAN_TTL_MILLIS;
        let result =
            serde_json::json!({"preflightToken":token,"expiresAt":expires_at,"items":proposed});
        manager
            .registry()
            .store_root_path_preflight(
                &token,
                &library_uuid,
                None,
                "materialize-remote",
                &request_hash,
                &body,
                &result,
                expires_at,
            )
            .map_err(sync_folder_error)?;
        return Ok(Json(result).into_response());
    }
    if uri.path().ends_with("/selection/commit") {
        let _admission = manager.path_admission().await;
        manager
            .ensure_context_revision(require_context_revision(&headers)?)
            .map_err(map_library_error)?;
        let request: SyncFolderSelectionCommitMutation = deserialize_sync_folder_body(&body)?;
        let (_root, mode, _hash, planned, plan_result, expires_at, _) = manager
            .registry()
            .root_path_preflight(&request.preflight_token, &library_uuid)
            .map_err(sync_folder_error)?;
        if mode != "materialize-remote"
            || expires_at < crate::host_registry::unix_timestamp_millis_for_runtime()
        {
            return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
        }
        let planned_request: SyncFolderSelectionPreflightMutation =
            serde_json::from_value(planned).map_err(|error| sync_folder_error(error.into()))?;
        if serde_json::to_value(&planned_request.items).ok()
            != serde_json::to_value(&request.items).ok()
        {
            return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
        }
        for item in &request.items {
            if item.selected {
                continue;
            }
            let descriptor = manager
                .registry()
                .remote_sync_root(&library_uuid, &item.root_uuid)
                .map_err(sync_folder_error)?
                .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-not-found")))?;
            let already_acknowledged = descriptor.inventory_state == "unselected"
                && descriptor.operation_revision
                    == item.expected_operation_revision.saturating_add(1);
            if descriptor.operation_revision != item.expected_operation_revision
                && !already_acknowledged
            {
                return Err(sync_folder_error(anyhow::anyhow!(
                    "stale-operation-revision"
                )));
            }
        }
        let selected_items = request
            .items
            .iter()
            .filter(|item| item.selected)
            .collect::<Vec<_>>();
        let completed_operations = selected_items
            .iter()
            .map(|item| manager.registry().root_operation(&item.operation_uuid))
            .collect::<anyhow::Result<Vec<_>>>()
            .map_err(sync_folder_error)?;
        if !selected_items.is_empty()
            && completed_operations.iter().all(|operation| {
                operation
                    .as_ref()
                    .is_some_and(|value| value.state == "completed")
            })
        {
            for (item, operation) in selected_items.iter().zip(&completed_operations) {
                let operation = operation
                    .as_ref()
                    .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?;
                let item_value =
                    serde_json::to_value(item).map_err(|error| sync_folder_error(error.into()))?;
                if operation.library_uuid != library_uuid
                    || operation.root_uuid != item.root_uuid
                    || operation.kind != "select"
                    || operation.request_hash
                        != crate::sync_folders::request_hash(&item_value)
                            .map_err(sync_folder_error)?
                {
                    return Err(sync_folder_error(anyhow::anyhow!("idempotency-key-reused")));
                }
                let descriptor = manager
                    .registry()
                    .remote_sync_root(&library_uuid, &item.root_uuid)
                    .map_err(sync_folder_error)?
                    .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-not-found")))?;
                let binding = manager
                    .registry()
                    .local_root_binding(&library_uuid, &item.root_uuid)
                    .map_err(sync_folder_error)?
                    .ok_or_else(|| {
                        sync_folder_error(anyhow::anyhow!("local-root-binding-not-found"))
                    })?;
                manager
                    .ensure_selected_sync_root(
                        &library_uuid,
                        uuid::Uuid::parse_str(&item.root_uuid)
                            .map_err(|error| sync_folder_error(error.into()))?,
                        uuid::Uuid::parse_str(&descriptor.root_folder_uuid)
                            .map_err(|error| sync_folder_error(error.into()))?,
                        binding.local_path,
                        descriptor.requested_name,
                        descriptor.materialized_name,
                        crate::sync::media::parse_hash(&descriptor.materialization_fingerprint)
                            .map_err(sync_folder_error)?,
                        uuid::Uuid::parse_str(&binding.local_binding_uuid)
                            .map_err(|error| sync_folder_error(error.into()))?,
                        uuid::Uuid::parse_str(
                            binding
                                .root_capture_epoch
                                .as_deref()
                                .unwrap_or(&binding.local_binding_uuid),
                        )
                        .map_err(|error| sync_folder_error(error.into()))?,
                        uuid::Uuid::parse_str(
                            binding
                                .root_actor_epoch
                                .as_deref()
                                .unwrap_or(&binding.local_binding_uuid),
                        )
                        .map_err(|error| sync_folder_error(error.into()))?,
                        binding.binding_generation,
                        binding.state,
                        binding.operation_revision,
                        false,
                    )
                    .await
                    .map_err(sync_folder_error)?;
            }
            let completed = completed_operations
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            return Ok((StatusCode::ACCEPTED,Json(serde_json::json!({"items":completed.iter().map(root_operation_json).collect::<Vec<_>>() }))).into_response());
        }
        if completed_operations.iter().any(Option::is_some) {
            return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
        }
        let result_items = plan_result
            .get("items")
            .and_then(|value| value.as_array())
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?;
        let mut operations = Vec::new();
        let mut evidences = Vec::new();
        for item in request.items.iter().filter(|item| item.selected) {
            let descriptor = manager
                .registry()
                .remote_sync_root(&library_uuid, &item.root_uuid)
                .map_err(sync_folder_error)?
                .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-not-found")))?;
            let result = result_items
                .iter()
                .find(|value| {
                    value.get("rootUuid").and_then(|value| value.as_str()) == Some(&item.root_uuid)
                })
                .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?;
            let evidence: crate::sync_folders::ValidatedRootPath = serde_json::from_value(
                result
                    .get("pathEvidence")
                    .cloned()
                    .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?,
            )
            .map_err(|error| sync_folder_error(error.into()))?;
            let binding_uuid = result
                .get("bindingUuid")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?
                .parse::<uuid::Uuid>()
                .map_err(|error| sync_folder_error(error.into()))?;
            manager
                .admit_content_path(FilePath::new(&item.path), true)
                .map_err(sync_folder_error)?;
            let current = crate::sync_folders::validate_root_path(
                FilePath::new(&item.path),
                crate::sync_folders::RootPathMode::MaterializeRemote,
                FilePath::new(&settings.default_placement_base),
                &manager
                    .registry()
                    .local_root_bindings(&library_uuid)
                    .map_err(sync_folder_error)?
                    .iter()
                    .map(|value| std::path::PathBuf::from(&value.local_path))
                    .collect::<Vec<_>>(),
            )
            .map_err(sync_folder_error)?;
            let expected_marker = crate::sync_folders::SyncRootMarker {
                format_version: 1,
                library_uuid: library_uuid.clone(),
                root_uuid: item.root_uuid.clone(),
                root_folder_uuid: descriptor.root_folder_uuid.clone(),
                local_binding_uuid: binding_uuid.to_string(),
                binding_generation: 1,
            };
            if !crate::sync_folders::preflight_evidence_still_matches(
                &evidence,
                &current,
                Some(&expected_marker),
            ) || descriptor.operation_revision != item.expected_operation_revision
            {
                return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
            }
            let request_value =
                serde_json::to_value(item).map_err(|error| sync_folder_error(error.into()))?;
            let request_hash =
                crate::sync_folders::request_hash(&request_value).map_err(sync_folder_error)?;
            evidences.push((
                item,
                descriptor,
                current,
                request_value,
                request_hash,
                binding_uuid,
            ));
        }
        for (item, _, current, request_value, request_hash, _) in &evidences {
            operations.push(crate::host_registry::PlannedRootOperation{operation_uuid:item.operation_uuid.clone(),root_uuid:item.root_uuid.clone(),kind:"select".to_owned(),phase:"preflighting".to_owned(),request_hash:request_hash.clone(),request:request_value.clone(),expected_operation_revision:item.expected_operation_revision,expected_binding_generation:item.expected_binding_generation,result:serde_json::json!({"preflightToken":request.preflight_token,"pathEvidence":current})});
        }
        manager
            .registry()
            .begin_root_operations_aggregate(&library_uuid, &request.preflight_token, &operations)
            .map_err(sync_folder_error)?;
        let mut owned = Vec::new();
        let mut staged_markers: Vec<(
            std::path::PathBuf,
            crate::sync_folders::SyncRootMarker,
            bool,
        )> = Vec::new();
        for (item, descriptor, current, _, _, binding_uuid) in &evidences {
            let marker = crate::sync_folders::SyncRootMarker {
                format_version: 1,
                library_uuid: library_uuid.clone(),
                root_uuid: item.root_uuid.clone(),
                root_folder_uuid: descriptor.root_folder_uuid.clone(),
                local_binding_uuid: binding_uuid.to_string(),
                binding_generation: 1,
            };
            let marker_preexisted = FilePath::new(&current.normalized_path)
                .join(crate::sync_folders::ROOT_MARKER_FILE_NAME)
                .is_file();
            if let Err(error) = crate::sync_folders::write_root_marker(
                FilePath::new(&current.normalized_path),
                &marker,
            ) {
                for (path, marker, remove_empty_directory) in staged_markers.iter().rev() {
                    let _ = crate::sync_folders::remove_root_marker_if_matches(
                        path,
                        marker,
                        *remove_empty_directory,
                    );
                }
                return Err(sync_folder_error(error));
            }
            staged_markers.push((
                std::path::PathBuf::from(&current.normalized_path),
                marker,
                !current.path_existed && !marker_preexisted,
            ));
            let marker_identity = match crate::library_path::local_file_identity(
                &FilePath::new(&current.normalized_path)
                    .join(crate::sync_folders::ROOT_MARKER_FILE_NAME),
            ) {
                Ok(identity) => identity,
                Err(error) => {
                    for (path, marker, remove_empty_directory) in staged_markers.iter().rev() {
                        let _ = crate::sync_folders::remove_root_marker_if_matches(
                            path,
                            marker,
                            *remove_empty_directory,
                        );
                    }
                    return Err(sync_folder_error(error.into()));
                }
            };
            owned.push((item, descriptor, current, *binding_uuid, marker_identity));
        }
        let mut bindings = Vec::new();
        for (item, descriptor, current, binding_uuid, marker_identity) in &owned {
            bindings.push(crate::host_registry::NewLocalRootBinding{library_uuid:library_uuid.clone(),root_uuid:item.root_uuid.clone(),local_binding_uuid:binding_uuid.to_string(),local_path:current.normalized_path.clone(),canonical_path_key:current.canonical_path_key.clone(),placement_parent:current.placement_parent.clone(),placement_parent_key:current.placement_parent_key.clone(),marker_identity:marker_identity.clone(),filesystem_identity:current.filesystem_identity.clone(),state:"materializing".to_owned(),binding_generation:1,root_capture_epoch:Some(uuid::Uuid::now_v7().to_string()),root_actor_epoch:Some(uuid::Uuid::now_v7().to_string()),desired_materialized_name:descriptor.materialized_name.clone(),desired_materialization_fingerprint:descriptor.materialization_fingerprint.clone(),applied_materialized_name:None,applied_materialization_fingerprint:None,operation_revision:item.expected_operation_revision+1,verification_json:serde_json::json!({"preflightToken":request.preflight_token,"pathEvidence":current})});
        }
        let deselections = request
            .items
            .iter()
            .filter(|item| !item.selected)
            .map(|item| (item.root_uuid.clone(), item.expected_operation_revision))
            .collect::<Vec<_>>();
        let completed = match manager.registry().complete_root_operations_aggregate(
            &library_uuid,
            &request.preflight_token,
            &bindings,
            &deselections,
        ) {
            Ok(completed) => completed,
            Err(error) => {
                for (path, marker, remove_empty_directory) in staged_markers.iter().rev() {
                    let _ = crate::sync_folders::remove_root_marker_if_matches(
                        path,
                        marker,
                        *remove_empty_directory,
                    );
                }
                return Err(sync_folder_error(error));
            }
        };
        for (_, descriptor, current, binding_uuid, _) in &owned {
            let stored_binding = manager
                .registry()
                .local_root_binding(&library_uuid, &descriptor.root_uuid)
                .map_err(sync_folder_error)?
                .ok_or_else(|| {
                    sync_folder_error(anyhow::anyhow!("local-root-binding-not-found"))
                })?;
            manager
                .ensure_selected_sync_root(
                    &library_uuid,
                    uuid::Uuid::parse_str(&descriptor.root_uuid)
                        .map_err(|error| sync_folder_error(error.into()))?,
                    uuid::Uuid::parse_str(&descriptor.root_folder_uuid)
                        .map_err(|error| sync_folder_error(error.into()))?,
                    current.normalized_path.clone(),
                    descriptor.requested_name.clone(),
                    descriptor.materialized_name.clone(),
                    crate::sync::media::parse_hash(&descriptor.materialization_fingerprint)
                        .map_err(sync_folder_error)?,
                    *binding_uuid,
                    uuid::Uuid::parse_str(
                        stored_binding
                            .root_capture_epoch
                            .as_deref()
                            .unwrap_or(&stored_binding.local_binding_uuid),
                    )
                    .map_err(|error| sync_folder_error(error.into()))?,
                    uuid::Uuid::parse_str(
                        stored_binding
                            .root_actor_epoch
                            .as_deref()
                            .unwrap_or(&stored_binding.local_binding_uuid),
                    )
                    .map_err(|error| sync_folder_error(error.into()))?,
                    1,
                    "materializing".to_owned(),
                    descriptor.operation_revision + 1,
                    false,
                )
                .await
                .map_err(sync_folder_error)?;
        }
        manager
            .registry()
            .consume_root_path_preflight(&request.preflight_token, &library_uuid)
            .map_err(sync_folder_error)?;
        return Ok((StatusCode::ACCEPTED,Json(serde_json::json!({"items":completed.iter().map(root_operation_json).collect::<Vec<_>>() }))).into_response());
    }
    Err(CoreError::coded(
        StatusCode::NOT_IMPLEMENTED,
        "not-implemented",
        "Synchronized-folder operation is owned by another task".to_owned(),
    ))
}

async fn sync_folder_contract_root_read(
    State(state): State<AppState>,
    Path((library_uuid, root_uuid)): Path<(String, String)>,
) -> ApiResult<Response> {
    validate_contract_uuid(&root_uuid, "rootUuid")?;
    sync_folder_v2_gate(&state, &library_uuid)?;
    let manager = sync_folder_library_manager(&state)?;
    if let Some(root) = manager
        .registry()
        .remote_sync_root(&library_uuid, &root_uuid)
        .map_err(sync_folder_error)?
    {
        let settings = manager
            .registry()
            .sync_folder_settings(&library_uuid)
            .map_err(sync_folder_error)?;
        return Ok(Json(
            sync_folder_record_json(&manager, &root, &settings)
                .await
                .map_err(sync_folder_error)?,
        )
        .into_response());
    }
    Err(CoreError::coded(
        StatusCode::NOT_FOUND,
        "root-not-found",
        "Synchronized folder not found".to_owned(),
    ))
}

async fn sync_folder_contract_root_write(
    State(state): State<AppState>,
    Path((library_uuid, root_uuid)): Path<(String, String)>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> ApiResult<Response> {
    validate_contract_uuid(&root_uuid, "rootUuid")?;
    let Json(body) = body.map_err(|error| {
        CoreError::coded(StatusCode::BAD_REQUEST, "invalid-input", error.body_text())
    })?;
    validate_sync_folder_root_write_shape(uri.path(), &body)?;
    validate_sync_folder_mutation_body(&body, uri.path().contains("/rename/"))?;
    if uri.path().ends_with("/reenable/preflight") || uri.path().ends_with("/stop/preflight") {
        require_sync_folder_binding_generation(&body, "re-enable comparison")?;
    }
    if uri.path().ends_with("/commit") || uri.path().ends_with("/relocations") {
        let expected = require_context_revision(&headers)?;
        sync_folder_library_manager(&state)?
            .ensure_context_revision(expected)
            .map_err(map_library_error)?;
    }
    sync_folder_v2_gate(&state, &library_uuid)?;
    let manager = sync_folder_library_manager(&state)?;
    if uri.path().ends_with("/rename/preflight") {
        let request: SyncFolderRenameMutation = deserialize_sync_folder_body(&body)?;
        let descriptor = manager
            .registry()
            .remote_sync_root(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-not-found")))?;
        if descriptor.operation_revision != request.expected_operation_revision {
            return Err(sync_folder_error(anyhow::anyhow!(
                "stale-operation-revision"
            )));
        }
        if let Some(expected_generation) = request.expected_binding_generation {
            let binding = manager
                .registry()
                .local_root_binding(&library_uuid, &root_uuid)
                .map_err(sync_folder_error)?
                .ok_or_else(|| {
                    sync_folder_error(anyhow::anyhow!("local-root-binding-not-found"))
                })?;
            if binding.binding_generation != expected_generation {
                return Err(sync_folder_error(anyhow::anyhow!(
                    "stale-binding-generation"
                )));
            }
        }
        let expected_heads = request
            .expected_location_heads
            .iter()
            .map(|value| uuid::Uuid::parse_str(value))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .map_err(|error| sync_folder_error(error.into()))?;
        let supersedes = request
            .supersedes
            .iter()
            .map(|value| uuid::Uuid::parse_str(value))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .map_err(|error| sync_folder_error(error.into()))?;
        if expected_heads != supersedes {
            return Err(sync_folder_error(anyhow::anyhow!("stale-location-heads")));
        }
        let root_folder_uuid = uuid::Uuid::parse_str(&descriptor.root_folder_uuid)
            .map_err(|error| sync_folder_error(error.into()))?;
        let live_heads = manager
            .write_sync_folder_catalog(&library_uuid, move |database| {
                database.sync_location_heads(root_folder_uuid)
            })
            .await
            .map_err(sync_folder_error)?;
        if live_heads != expected_heads {
            return Err(sync_folder_error(anyhow::anyhow!("stale-location-heads")));
        }
        let occupied = manager
            .registry()
            .remote_sync_roots(&library_uuid)
            .map_err(sync_folder_error)?
            .into_iter()
            .filter(|candidate| candidate.root_uuid != root_uuid && candidate.lifecycle == "active")
            .map(|candidate| {
                crate::sync::reducer::materialized_name_compare_key(&candidate.materialized_name)
            })
            .collect::<std::collections::BTreeSet<_>>();
        let materialized = crate::sync_folders::default_materialized_name(
            &request.requested_name,
            uuid::Uuid::parse_str(&root_uuid).map_err(|error| sync_folder_error(error.into()))?,
            &occupied,
        );
        let plan_uuid = request.operation_uuid.clone();
        let expires_at = crate::host_registry::unix_timestamp_millis_for_runtime()
            + crate::sync_folders::PLAN_TTL_MILLIS;
        let plan = serde_json::json!({
            "planUuid": plan_uuid,
            "requestedName": request.requested_name,
            "materializedName": materialized,
            "materializationFingerprint": crate::sync_folders::materialization_fingerprint(&materialized),
            "expectedOperationRevision": request.expected_operation_revision,
            "expectedBindingGeneration": request.expected_binding_generation,
            "expectedLocationHeads": request.expected_location_heads,
            "supersedes": request.supersedes,
            "expiresAt": expires_at,
        });
        let request_hash = crate::sync_folders::request_hash(&body).map_err(sync_folder_error)?;
        manager
            .registry()
            .begin_root_operation_once(
                &request.operation_uuid,
                &library_uuid,
                &root_uuid,
                "logical-rename",
                "planned",
                &request_hash,
                &body,
                request.expected_operation_revision,
                request.expected_binding_generation,
            )
            .map_err(sync_folder_error)?;
        manager
            .registry()
            .set_root_operation_plan(
                &request.operation_uuid,
                &plan_uuid,
                &plan,
                "validating-heads",
            )
            .map_err(sync_folder_error)?;
        return Ok(Json(plan).into_response());
    }
    if uri.path().ends_with("/rename/commit") {
        let request: SyncFolderRenameCommitMutation = deserialize_sync_folder_body(&body)?;
        let plan = manager
            .registry()
            .root_operation_result(&request.operation_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?;
        if plan.get("planUuid").and_then(serde_json::Value::as_str)
            != Some(request.plan_uuid.as_str())
            || plan
                .get("expiresAt")
                .and_then(serde_json::Value::as_i64)
                .is_none_or(|expires| {
                    expires < crate::host_registry::unix_timestamp_millis_for_runtime()
                })
            || plan
                .get("requestedName")
                .and_then(serde_json::Value::as_str)
                != Some(request.requested_name.as_str())
            || plan.get("expectedOperationRevision")
                != Some(&serde_json::json!(request.expected_operation_revision))
            || plan.get("expectedBindingGeneration")
                != Some(&serde_json::json!(request.expected_binding_generation))
            || plan.get("expectedLocationHeads")
                != Some(&serde_json::json!(request.expected_location_heads))
            || plan.get("supersedes") != Some(&serde_json::json!(request.supersedes))
        {
            return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
        }
        let descriptor = manager
            .registry()
            .remote_sync_root(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-not-found")))?;
        let expected_heads = request
            .expected_location_heads
            .iter()
            .map(|value| uuid::Uuid::parse_str(value))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .map_err(|error| sync_folder_error(error.into()))?;
        let supersedes = request
            .supersedes
            .iter()
            .map(|value| uuid::Uuid::parse_str(value))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .map_err(|error| sync_folder_error(error.into()))?;
        if expected_heads != supersedes {
            return Err(sync_folder_error(anyhow::anyhow!("stale-location-heads")));
        }
        let binding = manager
            .registry()
            .local_root_binding(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?;
        match (request.expected_binding_generation, binding.as_ref()) {
            (Some(expected), Some(binding)) if binding.binding_generation == expected => {}
            (Some(_), _) => {
                return Err(sync_folder_error(anyhow::anyhow!(
                    "stale-binding-generation"
                )))
            }
            (None, _) => {}
        }
        if descriptor.operation_revision != request.expected_operation_revision {
            return Err(sync_folder_error(anyhow::anyhow!(
                "stale-operation-revision"
            )));
        }
        let operation_uuid = uuid::Uuid::parse_str(&request.operation_uuid)
            .map_err(|error| sync_folder_error(error.into()))?;
        let root_folder_uuid = uuid::Uuid::parse_str(&descriptor.root_folder_uuid)
            .map_err(|error| sync_folder_error(error.into()))?;
        let requested_name = request.requested_name.clone();
        manager
            .write_sync_folder_catalog(&library_uuid, move |database| {
                database.commit_location_rename_once(
                    operation_uuid,
                    root_folder_uuid,
                    &requested_name,
                    &expected_heads,
                    &supersedes,
                )
            })
            .await
            .map_err(sync_folder_error)?;
        let operation = manager
            .registry()
            .complete_logical_root_rename(&request.operation_uuid)
            .map_err(sync_folder_error)?;
        manager
            .refresh_sync_folder_inventory(&library_uuid)
            .await
            .map_err(sync_folder_error)?;
        return Ok((StatusCode::ACCEPTED, Json(root_operation_json(&operation))).into_response());
    }
    if uri.path().ends_with("/relocations/preflight") {
        let _admission = manager.path_admission().await;
        let request: SyncFolderRelocationPreflightMutation = deserialize_sync_folder_body(&body)?;
        manager
            .admit_content_path(FilePath::new(&request.target_path), true)
            .map_err(sync_folder_error)?;
        let binding = manager
            .registry()
            .local_root_binding(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("local-root-binding-not-found")))?;
        if binding.operation_revision != request.expected_operation_revision {
            return Err(sync_folder_error(anyhow::anyhow!(
                "stale-operation-revision"
            )));
        }
        if binding.binding_generation != request.expected_binding_generation {
            return Err(sync_folder_error(anyhow::anyhow!(
                "stale-binding-generation"
            )));
        }
        if !matches!(
            binding.state.as_str(),
            "active" | "paused" | "local-rename-required"
        ) {
            return Err(sync_folder_error(anyhow::anyhow!("invalid-root-state")));
        }
        let descriptor = manager
            .registry()
            .remote_sync_root(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-not-found")))?;
        let marker = crate::sync_folders::SyncRootMarker {
            format_version: 1,
            library_uuid: library_uuid.clone(),
            root_uuid: root_uuid.clone(),
            root_folder_uuid: descriptor.root_folder_uuid.clone(),
            local_binding_uuid: binding.local_binding_uuid.clone(),
            binding_generation: binding.binding_generation,
        };
        let settings = manager
            .registry()
            .sync_folder_settings(&library_uuid)
            .map_err(sync_folder_error)?;
        let other_paths = manager
            .registry()
            .local_root_bindings(&library_uuid)
            .map_err(sync_folder_error)?
            .into_iter()
            .filter(|candidate| candidate.root_uuid != root_uuid)
            .map(|candidate| std::path::PathBuf::from(candidate.local_path))
            .collect::<Vec<_>>();
        let source_path = std::path::PathBuf::from(&binding.local_path);
        let target_path = std::path::PathBuf::from(&request.target_path);
        let placement_base = std::path::PathBuf::from(&settings.default_placement_base);
        let evidence = tokio::task::spawn_blocking(move || {
            crate::sync_folders::validate_relocation_paths(
                &source_path,
                &target_path,
                &placement_base,
                &other_paths,
                &marker,
            )
        })
        .await
        .map_err(|error| sync_folder_error(error.into()))?
        .map_err(sync_folder_error)?;
        if matches!(request.mode, SyncFolderRelocationMode::AutomaticRename)
            && (FilePath::new(&evidence.target_path)
                .file_name()
                .and_then(|name| name.to_str())
                != Some(descriptor.materialized_name.as_str())
                || binding.desired_materialization_fingerprint
                    != descriptor.materialization_fingerprint)
        {
            return Err(sync_folder_error(anyhow::anyhow!(
                "stale-operation-revision"
            )));
        }
        let token = request.operation_uuid.clone();
        let expires_at = crate::host_registry::unix_timestamp_millis_for_runtime()
            + crate::sync_folders::PLAN_TTL_MILLIS;
        let result = serde_json::json!({
            "rootUuid": root_uuid,
            "ok": true,
            "reason": "ok",
            "normalizedPath": evidence.target_path,
            "proposedMaterializedName": FilePath::new(&evidence.target_path).file_name().and_then(|name| name.to_str()),
            "preflightToken": token,
            "expiresAt": expires_at,
            "availableBytes": crate::library_path::free_space_bytes(FilePath::new(&evidence.target_path)).ok(),
            "pathEvidence": evidence,
        });
        let mode = match request.mode {
            SyncFolderRelocationMode::ExplicitMove => "relocate-explicit",
            SyncFolderRelocationMode::AutomaticRename => "relocate-automatic",
        };
        manager
            .registry()
            .store_root_path_preflight(
                &request.operation_uuid,
                &library_uuid,
                Some(&root_uuid),
                mode,
                &crate::sync_folders::request_hash(&body).map_err(sync_folder_error)?,
                &body,
                &result,
                expires_at,
            )
            .map_err(sync_folder_error)?;
        return Ok(Json(result).into_response());
    }
    if uri.path().ends_with("/relocations") {
        let admission = manager.path_admission().await;
        manager
            .ensure_context_revision(require_context_revision(&headers)?)
            .map_err(map_library_error)?;
        let request: SyncFolderRelocationCommitMutation = deserialize_sync_folder_body(&body)?;
        manager
            .admit_content_path(FilePath::new(&request.target_path), true)
            .map_err(sync_folder_error)?;
        let (stored_root, stored_mode, _, stored_request, result, expires_at, _) = manager
            .registry()
            .root_path_preflight(&request.preflight_token, &library_uuid)
            .map_err(sync_folder_error)?;
        let expected_mode = match request.mode {
            SyncFolderRelocationMode::ExplicitMove => "relocate-explicit",
            SyncFolderRelocationMode::AutomaticRename => "relocate-automatic",
        };
        if stored_root.as_deref() != Some(root_uuid.as_str())
            || stored_mode != expected_mode
            || expires_at < crate::host_registry::unix_timestamp_millis_for_runtime()
            || stored_request.get("targetPath") != Some(&serde_json::json!(request.target_path))
            || stored_request.get("operationUuid")
                != Some(&serde_json::json!(request.operation_uuid))
            || stored_request.get("expectedOperationRevision")
                != Some(&serde_json::json!(request.expected_operation_revision))
            || stored_request.get("expectedBindingGeneration")
                != Some(&serde_json::json!(request.expected_binding_generation))
        {
            return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
        }
        let evidence: crate::sync_folders::RelocationEvidence = serde_json::from_value(
            result
                .get("pathEvidence")
                .cloned()
                .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?,
        )
        .map_err(|error| sync_folder_error(error.into()))?;
        if evidence.target_canonical_key
            != crate::sync_folders::canonical_path_key(FilePath::new(&request.target_path))
        {
            return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
        }
        let descriptor = manager
            .registry()
            .remote_sync_root(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-not-found")))?;
        if request.desired_materialization_fingerprint != descriptor.materialization_fingerprint {
            return Err(sync_folder_error(anyhow::anyhow!(
                "stale-operation-revision"
            )));
        }
        let binding = manager
            .registry()
            .local_root_binding(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("local-root-binding-not-found")))?;
        let existing_relocation = manager
            .registry()
            .root_relocation_operation(&request.operation_uuid)
            .map_err(sync_folder_error)?;
        let marker = crate::sync_folders::SyncRootMarker {
            format_version: 1,
            library_uuid: library_uuid.clone(),
            root_uuid: root_uuid.clone(),
            root_folder_uuid: descriptor.root_folder_uuid,
            local_binding_uuid: binding.local_binding_uuid.clone(),
            binding_generation: request.expected_binding_generation,
        };
        if existing_relocation.is_none() {
            let settings = manager
                .registry()
                .sync_folder_settings(&library_uuid)
                .map_err(sync_folder_error)?;
            let other_paths = manager
                .registry()
                .local_root_bindings(&library_uuid)
                .map_err(sync_folder_error)?
                .into_iter()
                .filter(|candidate| candidate.root_uuid != root_uuid)
                .map(|candidate| std::path::PathBuf::from(candidate.local_path))
                .collect::<Vec<_>>();
            let source_path = std::path::PathBuf::from(&binding.local_path);
            let target_path = std::path::PathBuf::from(&request.target_path);
            let placement_base = std::path::PathBuf::from(&settings.default_placement_base);
            let marker_for_preflight = marker.clone();
            let current_evidence = tokio::task::spawn_blocking(move || {
                crate::sync_folders::validate_relocation_paths(
                    &source_path,
                    &target_path,
                    &placement_base,
                    &other_paths,
                    &marker_for_preflight,
                )
            })
            .await
            .map_err(|error| sync_folder_error(error.into()))?
            .map_err(sync_folder_error)?;
            if current_evidence != evidence {
                return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
            }
        }
        let mode = match request.mode {
            SyncFolderRelocationMode::ExplicitMove => "explicit-move",
            SyncFolderRelocationMode::AutomaticRename => "automatic-rename",
        };
        let mut journal_request = body.clone();
        journal_request["pathEvidence"] =
            serde_json::to_value(&evidence).map_err(|error| sync_folder_error(error.into()))?;
        journal_request["localBindingUuid"] = serde_json::json!(binding.local_binding_uuid);
        let resume_state = existing_relocation
            .as_ref()
            .map(|existing| {
                crate::host_registry::relocation_resume_state(&existing.operation.request_json)
            })
            .unwrap_or_else(|| {
                if binding.state == "paused" {
                    "paused"
                } else {
                    "active"
                }
            });
        journal_request["resumeState"] = serde_json::json!(resume_state);
        let (relocation, created) = manager
            .registry()
            .begin_root_relocation(
                &request.operation_uuid,
                &library_uuid,
                &root_uuid,
                mode,
                &crate::sync_folders::request_hash(&journal_request).map_err(sync_folder_error)?,
                &journal_request,
                request.expected_operation_revision,
                request.expected_binding_generation,
                &request.desired_materialization_fingerprint,
                &evidence,
            )
            .map_err(sync_folder_error)?;
        if created {
            manager
                .registry()
                .consume_root_path_preflight(&request.preflight_token, &library_uuid)
                .map_err(sync_folder_error)?;
        }
        drop(admission);
        let operation = manager
            .execute_root_relocation(&relocation.operation.operation_uuid, marker)
            .await
            .map_err(sync_folder_error)?;
        return Ok((StatusCode::ACCEPTED, Json(root_operation_json(&operation))).into_response());
    }
    if uri.path().ends_with("/stop/preflight") {
        let request: SyncFolderOperationFenceMutation = deserialize_sync_folder_body(&body)?;
        let snapshot = manager
            .sync_folder_snapshot(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
            )
            .await
            .map_err(sync_folder_error)?;
        let work = manager
            .sync_folder_stop_work(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
            )
            .await
            .map_err(sync_folder_error)?;
        let plan = manager
            .registry()
            .create_stop_sync_plan(
                &request.operation_uuid,
                &library_uuid,
                &root_uuid,
                &crate::sync_folders::request_hash(&body).map_err(sync_folder_error)?,
                &body,
                request.expected_operation_revision,
                request.expected_binding_generation,
                &snapshot,
                &work,
            )
            .map_err(sync_folder_error)?;
        return Ok(Json(plan).into_response());
    }
    if uri.path().ends_with("/stop/commit") {
        let request: SyncFolderStopCommitMutation = deserialize_sync_folder_body(&body)?;
        let binding = manager
            .registry()
            .local_root_binding(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("local-root-binding-not-found")))?;
        let snapshot = manager
            .sync_folder_snapshot(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
            )
            .await
            .map_err(sync_folder_error)?;
        let work = manager
            .sync_folder_stop_work(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
            )
            .await
            .map_err(sync_folder_error)?;
        let planned_body = serde_json::json!({"operationUuid":request.operation_uuid,"expectedOperationRevision":request.expected_operation_revision,"expectedBindingGeneration":request.expected_binding_generation});
        let operation = manager
            .registry()
            .commit_stop_sync(
                &request.operation_uuid,
                &request.plan_uuid,
                &library_uuid,
                &root_uuid,
                &crate::sync_folders::request_hash(&planned_body).map_err(sync_folder_error)?,
                &planned_body,
                request.expected_operation_revision,
                request.expected_binding_generation,
                &snapshot,
                &work,
            )
            .map_err(sync_folder_error)?;
        let stopped = manager
            .registry()
            .local_root_binding(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("local-root-binding-not-found")))?;
        manager
            .fence_sync_root_local_only(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
                uuid::Uuid::parse_str(&binding.local_binding_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
                request.expected_binding_generation,
                request.expected_binding_generation + 1,
                uuid::Uuid::parse_str(
                    stopped
                        .root_capture_epoch
                        .as_deref()
                        .unwrap_or(&stopped.local_binding_uuid),
                )
                .map_err(|error| sync_folder_error(error.into()))?,
                uuid::Uuid::parse_str(
                    stopped
                        .root_actor_epoch
                        .as_deref()
                        .unwrap_or(&stopped.local_binding_uuid),
                )
                .map_err(|error| sync_folder_error(error.into()))?,
                binding.operation_revision + 1,
            )
            .await
            .map_err(sync_folder_error)?;
        manager.wake_sync_folder_workers();
        return Ok((StatusCode::ACCEPTED, Json(root_operation_json(&operation))).into_response());
    }
    if uri.path().ends_with("/rebind/preflight") {
        let _admission = manager.path_admission().await;
        let request: SyncFolderRebindPreflightMutation = deserialize_sync_folder_body(&body)?;
        let existing_binding = manager
            .registry()
            .local_root_binding(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?;
        if existing_binding
            .as_ref()
            .is_some_and(|binding| binding.state == "local-only-fork")
            || manager
                .registry()
                .root_has_cutover_lineage(&library_uuid, &root_uuid)
                .map_err(sync_folder_error)?
        {
            return Err(sync_folder_error(anyhow::anyhow!("comparison-required")));
        }
        if existing_binding.as_ref().is_some_and(|binding| {
            !matches!(
                binding.state.as_str(),
                "materializing" | "unavailable" | "error"
            )
        }) {
            return Err(sync_folder_error(anyhow::anyhow!("invalid-root-state")));
        }
        let settings = manager
            .registry()
            .sync_folder_settings(&library_uuid)
            .map_err(sync_folder_error)?;
        let existing = manager
            .registry()
            .local_root_bindings(&library_uuid)
            .map_err(sync_folder_error)?
            .into_iter()
            .filter(|binding| binding.root_uuid != root_uuid)
            .map(|binding| std::path::PathBuf::from(binding.local_path))
            .collect::<Vec<_>>();
        manager
            .admit_content_path(FilePath::new(&request.path), true)
            .map_err(sync_folder_error)?;
        let validated = crate::sync_folders::validate_root_path(
            FilePath::new(&request.path),
            crate::sync_folders::RootPathMode::RebindExisting,
            FilePath::new(&settings.default_placement_base),
            &existing,
        )
        .map_err(sync_folder_error)?;
        let marker =
            crate::sync_folders::read_root_marker(FilePath::new(&validated.normalized_path))
                .map_err(sync_folder_error)?;
        if marker.library_uuid != library_uuid
            || marker.root_uuid != root_uuid
            || existing_binding.as_ref().is_some_and(|binding| {
                marker.local_binding_uuid != binding.local_binding_uuid
                    || marker.binding_generation != binding.binding_generation
            })
            || request.marker_identity
                != crate::library_path::local_file_identity(
                    &FilePath::new(&validated.normalized_path)
                        .join(crate::sync_folders::ROOT_MARKER_FILE_NAME),
                )
                .map_err(|error| sync_folder_error(error.into()))?
        {
            return Err(sync_folder_error(anyhow::anyhow!("marker-mismatch")));
        }
        if request.prior_cutover_snapshot_uuid != "00000000-0000-0000-0000-000000000000" {
            let _ = manager
                .registry()
                .cutover_base_snapshot(&library_uuid, &root_uuid)
                .map_err(|_| sync_folder_error(anyhow::anyhow!("comparison-required")))?;
            return Err(sync_folder_error(anyhow::anyhow!("comparison-required")));
        }
        let descriptor = manager
            .registry()
            .remote_sync_root(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-not-found")))?;
        let plan_uuid = request.operation_uuid.clone();
        let expires_at = crate::host_registry::unix_timestamp_millis_for_runtime()
            + crate::sync_folders::PLAN_TTL_MILLIS;
        let next_generation = existing_binding
            .as_ref()
            .map(|binding| binding.binding_generation)
            .unwrap_or(marker.binding_generation)
            .saturating_add(1);
        let plan = serde_json::json!({"planUuid":plan_uuid,"rootUuid":root_uuid,"path":validated.normalized_path,"markerMatches":true,"markerIdentity":request.marker_identity,"contentEvidenceDigest":validated.content_evidence_digest,"pathEvidence":validated,"remoteRevision":descriptor.operation_revision,"remoteMaterializationFingerprint":descriptor.materialization_fingerprint,"nextBindingUuid":uuid::Uuid::now_v7().to_string(),"nextBindingGeneration":next_generation,"expiresAt":expires_at});
        let request_hash = crate::sync_folders::request_hash(&body).map_err(sync_folder_error)?;
        manager
            .registry()
            .begin_root_operation_once(
                &request.operation_uuid,
                &library_uuid,
                &root_uuid,
                "rebind",
                "preflighting",
                &request_hash,
                &body,
                request.expected_operation_revision,
                request.expected_binding_generation,
            )
            .map_err(sync_folder_error)?;
        manager
            .registry()
            .set_root_operation_plan(
                &request.operation_uuid,
                &plan_uuid,
                &plan,
                "verifying-existing",
            )
            .map_err(sync_folder_error)?;
        return Ok(Json(plan).into_response());
    }
    if uri.path().ends_with("/rebind/commit") {
        let _admission = manager.path_admission().await;
        manager
            .ensure_context_revision(require_context_revision(&headers)?)
            .map_err(map_library_error)?;
        let request: SyncFolderRebindCommitMutation = deserialize_sync_folder_body(&body)?;
        if let Some(operation) = manager
            .registry()
            .root_operation(&request.operation_uuid)
            .map_err(sync_folder_error)?
        {
            if operation.library_uuid != library_uuid
                || operation.root_uuid != root_uuid
                || operation.kind != "rebind"
            {
                return Err(sync_folder_error(anyhow::anyhow!("idempotency-key-reused")));
            }
        }
        let plan = manager
            .registry()
            .root_operation_result(&request.operation_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?;
        if plan.get("planUuid").and_then(|value| value.as_str()) != Some(&request.plan_uuid)
            || plan
                .get("expiresAt")
                .and_then(|value| value.as_i64())
                .is_none_or(|value| {
                    value < crate::host_registry::unix_timestamp_millis_for_runtime()
                })
        {
            return Err(sync_folder_error(anyhow::anyhow!("stale-plan")));
        }
        let descriptor = manager
            .registry()
            .remote_sync_root(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-not-found")))?;
        let evidence: crate::sync_folders::ValidatedRootPath = serde_json::from_value(
            plan.get("pathEvidence")
                .cloned()
                .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?,
        )
        .map_err(|error| sync_folder_error(error.into()))?;
        manager
            .admit_content_path(FilePath::new(&evidence.normalized_path), false)
            .map_err(sync_folder_error)?;
        let marker =
            crate::sync_folders::read_root_marker(FilePath::new(&evidence.normalized_path))
                .map_err(sync_folder_error)?;
        let current_marker_identity = crate::library_path::local_file_identity(
            &FilePath::new(&evidence.normalized_path)
                .join(crate::sync_folders::ROOT_MARKER_FILE_NAME),
        )
        .map_err(|error| sync_folder_error(error.into()))?;
        let binding_uuid = plan
            .get("nextBindingUuid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?
            .parse::<uuid::Uuid>()
            .map_err(|error| sync_folder_error(error.into()))?;
        let generation = plan
            .get("nextBindingGeneration")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("stale-plan")))?;
        let marker_already_advanced = marker.binding_generation == generation
            && marker.local_binding_uuid == binding_uuid.to_string();
        if marker.library_uuid != library_uuid
            || marker.root_uuid != root_uuid
            || (!marker_already_advanced
                && plan
                    .get("markerIdentity")
                    .and_then(serde_json::Value::as_str)
                    != Some(current_marker_identity.as_str()))
            || plan
                .get("remoteRevision")
                .and_then(serde_json::Value::as_u64)
                != Some(descriptor.operation_revision)
            || plan
                .get("remoteMaterializationFingerprint")
                .and_then(serde_json::Value::as_str)
                != Some(descriptor.materialization_fingerprint.as_str())
            || crate::sync_folders::directory_evidence_digest(FilePath::new(
                &evidence.normalized_path,
            ))
            .map_err(sync_folder_error)?
                != evidence.content_evidence_digest
        {
            return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
        }
        if marker.binding_generation != generation.saturating_sub(1)
            && (marker.binding_generation != generation
                || marker.local_binding_uuid != binding_uuid.to_string())
        {
            return Err(sync_folder_error(anyhow::anyhow!("path-changed")));
        }
        let original_marker = marker.clone();
        let marker_advanced = marker.binding_generation != generation;
        if marker_advanced {
            let next_marker = crate::sync_folders::SyncRootMarker {
                local_binding_uuid: binding_uuid.to_string(),
                binding_generation: generation,
                ..marker
            };
            crate::sync_folders::replace_root_marker(
                FilePath::new(&evidence.normalized_path),
                &next_marker,
            )
            .map_err(sync_folder_error)?;
        }
        let marker_identity = match crate::library_path::local_file_identity(
            &FilePath::new(&evidence.normalized_path)
                .join(crate::sync_folders::ROOT_MARKER_FILE_NAME),
        ) {
            Ok(identity) => identity,
            Err(error) => {
                if marker_advanced {
                    let _ = crate::sync_folders::replace_root_marker(
                        FilePath::new(&evidence.normalized_path),
                        &original_marker,
                    );
                }
                return Err(sync_folder_error(error.into()));
            }
        };
        let binding = crate::host_registry::NewLocalRootBinding {
            library_uuid: library_uuid.clone(),
            root_uuid: root_uuid.clone(),
            local_binding_uuid: binding_uuid.to_string(),
            local_path: evidence.normalized_path.clone(),
            canonical_path_key: evidence.canonical_path_key.clone(),
            placement_parent: evidence.placement_parent.clone(),
            placement_parent_key: evidence.placement_parent_key.clone(),
            marker_identity,
            filesystem_identity: evidence.filesystem_identity.clone(),
            state: "materializing".to_owned(),
            binding_generation: generation,
            root_capture_epoch: Some(uuid::Uuid::now_v7().to_string()),
            root_actor_epoch: Some(uuid::Uuid::now_v7().to_string()),
            desired_materialized_name: descriptor.materialized_name.clone(),
            desired_materialization_fingerprint: descriptor.materialization_fingerprint.clone(),
            applied_materialized_name: None,
            applied_materialization_fingerprint: None,
            operation_revision: request.expected_operation_revision + 1,
            verification_json: plan,
        };
        let stored = match manager.registry().commit_local_root_binding(
            &request.operation_uuid,
            &request.plan_uuid,
            &binding,
        ) {
            Ok(stored) => stored,
            Err(error) => {
                if marker_advanced {
                    let _ = crate::sync_folders::replace_root_marker(
                        FilePath::new(&evidence.normalized_path),
                        &original_marker,
                    );
                }
                return Err(sync_folder_error(error));
            }
        };
        manager
            .ensure_selected_sync_root(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
                uuid::Uuid::parse_str(&descriptor.root_folder_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
                evidence.normalized_path,
                descriptor.requested_name,
                descriptor.materialized_name,
                crate::sync::media::parse_hash(&descriptor.materialization_fingerprint)
                    .map_err(sync_folder_error)?,
                binding_uuid,
                uuid::Uuid::parse_str(
                    stored
                        .root_capture_epoch
                        .as_deref()
                        .unwrap_or(&stored.local_binding_uuid),
                )
                .map_err(|error| sync_folder_error(error.into()))?,
                uuid::Uuid::parse_str(
                    stored
                        .root_actor_epoch
                        .as_deref()
                        .unwrap_or(&stored.local_binding_uuid),
                )
                .map_err(|error| sync_folder_error(error.into()))?,
                stored.binding_generation,
                stored.state,
                stored.operation_revision,
                false,
            )
            .await
            .map_err(sync_folder_error)?;
        manager.wake_sync_folder_workers();
        let operation = manager
            .registry()
            .root_operation(&request.operation_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-operation-not-found")))?;
        return Ok((StatusCode::ACCEPTED, Json(root_operation_json(&operation))).into_response());
    }
    if uri.path().ends_with("/reenable/preflight") {
        let request: SyncFolderOperationFenceMutation = deserialize_sync_folder_body(&body)?;
        let (cutover_uuid, base, sealed_generation) = manager
            .registry()
            .cutover_base_snapshot(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?;
        if request.expected_binding_generation <= sealed_generation {
            return Err(sync_folder_error(anyhow::anyhow!(
                "stale-binding-generation"
            )));
        }
        let local = manager
            .sync_folder_snapshot(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
            )
            .await
            .map_err(sync_folder_error)?;
        let (remote, _) = manager
            .current_remote_root_snapshot(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
            )
            .await
            .map_err(sync_folder_error)?;
        let differences = crate::sync_folders::compare_snapshots(&base, &local, &remote);
        let generations = manager
            .registry()
            .sync_transport_generations(&library_uuid)
            .map_err(sync_folder_error)?;
        let plan = manager
            .registry()
            .create_comparison_plan(
                &request.operation_uuid,
                &library_uuid,
                &root_uuid,
                &crate::sync_folders::request_hash(&body).map_err(sync_folder_error)?,
                &body,
                request.expected_operation_revision,
                request.expected_binding_generation,
                &cutover_uuid,
                &base,
                &local,
                &remote,
                &differences,
                generations.0,
                generations.1,
            )
            .map_err(sync_folder_error)?;
        return Ok(Json(plan).into_response());
    }
    if uri.path().ends_with("/reenable/commit") {
        let request: SyncFolderReenableMutation = deserialize_sync_folder_body(&body)?;
        if let Some(publish) = manager
            .registry()
            .publish_as_new(&request.operation_uuid)
            .map_err(sync_folder_error)?
        {
            let completed = manager
                .recover_publish_as_new(&request.operation_uuid)
                .await
                .map_err(sync_folder_error)?;
            let mut response = root_operation_json(&completed);
            response["newRootUuid"] = serde_json::json!(publish.new_root_uuid);
            response["newRootFolderUuid"] = serde_json::json!(publish.new_root_folder_uuid);
            response["enrollmentOperationUuid"] =
                serde_json::json!(publish.enrollment_operation_uuid);
            return Ok((StatusCode::ACCEPTED, Json(response)).into_response());
        }
        let _plan = manager
            .registry()
            .root_comparison_plan(&library_uuid, &root_uuid, &request.plan_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("comparison-plan-not-found")))?;
        let local = manager
            .sync_folder_snapshot(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
            )
            .await
            .map_err(sync_folder_error)?;
        let (remote, _) = manager
            .current_remote_root_snapshot(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
            )
            .await
            .map_err(sync_folder_error)?;
        let generations = manager
            .registry()
            .sync_transport_generations(&library_uuid)
            .map_err(sync_folder_error)?;
        let mode = match request.mode {
            SyncFolderComparisonMode::MergeLocalFork => "merge-local-fork",
            SyncFolderComparisonMode::PublishAsNew => "publish-as-new",
        };
        if mode == "publish-as-new" {
            let (operation, publish) = manager
                .registry()
                .commit_publish_as_new(
                    &request.operation_uuid,
                    &library_uuid,
                    &root_uuid,
                    &request.plan_uuid,
                    request.expected_operation_revision,
                    request.expected_binding_generation,
                    &local.digest,
                    &remote.digest,
                    generations.0,
                    generations.1,
                )
                .map_err(sync_folder_error)?;
            let completed = manager
                .recover_publish_as_new(&request.operation_uuid)
                .await
                .map_err(sync_folder_error)?;
            let mut response = root_operation_json(&completed);
            response["newRootUuid"] = serde_json::json!(publish.new_root_uuid);
            response["newRootFolderUuid"] = serde_json::json!(publish.new_root_folder_uuid);
            response["enrollmentOperationUuid"] =
                serde_json::json!(publish.enrollment_operation_uuid);
            let _ = operation;
            return Ok((StatusCode::ACCEPTED, Json(response)).into_response());
        }
        let (operation, binding) = manager
            .registry()
            .commit_reenable(
                &request.operation_uuid,
                &library_uuid,
                &root_uuid,
                &request.plan_uuid,
                request.expected_operation_revision,
                request.expected_binding_generation,
                &local.digest,
                &remote.digest,
                generations.0,
                generations.1,
                mode,
            )
            .map_err(sync_folder_error)?;
        let descriptor = manager
            .registry()
            .remote_sync_root(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-not-found")))?;
        let marker = crate::sync_folders::read_root_marker(FilePath::new(&binding.local_path))
            .map_err(sync_folder_error)?;
        let binding_uuid = uuid::Uuid::parse_str(&binding.local_binding_uuid)
            .map_err(|error| sync_folder_error(error.into()))?;
        crate::sync_folders::replace_root_marker(
            FilePath::new(&binding.local_path),
            &crate::sync_folders::SyncRootMarker {
                local_binding_uuid: binding.local_binding_uuid.clone(),
                binding_generation: binding.binding_generation,
                ..marker
            },
        )
        .map_err(sync_folder_error)?;
        let marker_identity = crate::library_path::local_file_identity(
            &FilePath::new(&binding.local_path).join(crate::sync_folders::ROOT_MARKER_FILE_NAME),
        )
        .map_err(|error| sync_folder_error(error.into()))?;
        manager
            .registry()
            .update_local_root_marker_identity(
                &library_uuid,
                &root_uuid,
                &binding.local_binding_uuid,
                binding.binding_generation,
                &marker_identity,
            )
            .map_err(sync_folder_error)?;
        manager
            .ensure_selected_sync_root(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
                uuid::Uuid::parse_str(&descriptor.root_folder_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
                binding.local_path.clone(),
                descriptor.requested_name,
                descriptor.materialized_name,
                crate::sync::media::parse_hash(&descriptor.materialization_fingerprint)
                    .map_err(sync_folder_error)?,
                binding_uuid,
                uuid::Uuid::parse_str(
                    binding
                        .root_capture_epoch
                        .as_deref()
                        .unwrap_or(&binding.local_binding_uuid),
                )
                .map_err(|error| sync_folder_error(error.into()))?,
                uuid::Uuid::parse_str(
                    binding
                        .root_actor_epoch
                        .as_deref()
                        .unwrap_or(&binding.local_binding_uuid),
                )
                .map_err(|error| sync_folder_error(error.into()))?,
                binding.binding_generation,
                binding.state,
                binding.operation_revision,
                false,
            )
            .await
            .map_err(sync_folder_error)?;
        let completed = manager
            .recover_root_reconciliation(&library_uuid, &root_uuid)
            .await
            .map_err(sync_folder_error)?
            .unwrap_or(operation);
        return Ok((StatusCode::ACCEPTED, Json(root_operation_json(&completed))).into_response());
    }
    Err(CoreError::coded(
        StatusCode::NOT_IMPLEMENTED,
        "not-implemented",
        "Synchronized-folder operation is owned by another task".to_owned(),
    ))
}

async fn sync_folder_contract_root_action(
    State(state): State<AppState>,
    Path((library_uuid, root_uuid, command)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> ApiResult<Response> {
    if !matches!(command.as_str(), "pause" | "resume" | "stop-sync") {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid-input",
            "Unknown synchronized-folder command".to_string(),
        ));
    }
    validate_contract_uuid(&root_uuid, "rootUuid")?;
    let Json(body) = body.map_err(|error| {
        CoreError::coded(StatusCode::BAD_REQUEST, "invalid-input", error.body_text())
    })?;
    let _: SyncFolderOperationFenceMutation = deserialize_sync_folder_body(&body)?;
    validate_sync_folder_mutation_body(&body, false)?;
    require_sync_folder_binding_generation(&body, "synchronized-folder commands")?;
    let expected = require_context_revision(&headers)?;
    sync_folder_library_manager(&state)?
        .ensure_context_revision(expected)
        .map_err(map_library_error)?;
    sync_folder_v2_gate(&state, &library_uuid)?;
    let manager = sync_folder_library_manager(&state)?;
    let request: SyncFolderOperationFenceMutation = deserialize_sync_folder_body(&body)?;
    let request_hash = crate::sync_folders::request_hash(&body).map_err(sync_folder_error)?;
    let operation = if command == "stop-sync" {
        let binding = manager
            .registry()
            .local_root_binding(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("local-root-binding-not-found")))?;
        let snapshot = manager
            .sync_folder_snapshot(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
            )
            .await
            .map_err(sync_folder_error)?;
        let work = manager
            .sync_folder_stop_work(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
            )
            .await
            .map_err(sync_folder_error)?;
        let plan = manager
            .registry()
            .create_stop_sync_plan(
                &request.operation_uuid,
                &library_uuid,
                &root_uuid,
                &request_hash,
                &body,
                request.expected_operation_revision,
                request.expected_binding_generation,
                &snapshot,
                &work,
            )
            .map_err(sync_folder_error)?;
        let operation = manager
            .registry()
            .commit_stop_sync(
                &request.operation_uuid,
                &plan.plan_uuid,
                &library_uuid,
                &root_uuid,
                &request_hash,
                &body,
                request.expected_operation_revision,
                request.expected_binding_generation,
                &snapshot,
                &work,
            )
            .map_err(sync_folder_error)?;
        let stopped = manager
            .registry()
            .local_root_binding(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("local-root-binding-not-found")))?;
        manager
            .fence_sync_root_local_only(
                &library_uuid,
                uuid::Uuid::parse_str(&root_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
                uuid::Uuid::parse_str(&binding.local_binding_uuid)
                    .map_err(|error| sync_folder_error(error.into()))?,
                request.expected_binding_generation,
                request.expected_binding_generation + 1,
                uuid::Uuid::parse_str(
                    stopped
                        .root_capture_epoch
                        .as_deref()
                        .unwrap_or(&stopped.local_binding_uuid),
                )
                .map_err(|error| sync_folder_error(error.into()))?,
                uuid::Uuid::parse_str(
                    stopped
                        .root_actor_epoch
                        .as_deref()
                        .unwrap_or(&stopped.local_binding_uuid),
                )
                .map_err(|error| sync_folder_error(error.into()))?,
                binding.operation_revision + 1,
            )
            .await
            .map_err(sync_folder_error)?;
        let updated = manager
            .registry()
            .local_root_binding(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("local-root-binding-not-found")))?;
        if FilePath::new(&updated.local_path)
            .join(crate::sync_folders::ROOT_MARKER_FILE_NAME)
            .is_file()
        {
            let marker = crate::sync_folders::read_root_marker(FilePath::new(&updated.local_path))
                .map_err(sync_folder_error)?;
            crate::sync_folders::replace_root_marker(
                FilePath::new(&updated.local_path),
                &crate::sync_folders::SyncRootMarker {
                    binding_generation: updated.binding_generation,
                    ..marker
                },
            )
            .map_err(sync_folder_error)?;
            let marker_identity = crate::library_path::local_file_identity(
                &FilePath::new(&updated.local_path)
                    .join(crate::sync_folders::ROOT_MARKER_FILE_NAME),
            )
            .map_err(|error| sync_folder_error(error.into()))?;
            manager
                .registry()
                .update_local_root_marker_identity(
                    &library_uuid,
                    &root_uuid,
                    &updated.local_binding_uuid,
                    updated.binding_generation,
                    &marker_identity,
                )
                .map_err(sync_folder_error)?;
        }
        operation
    } else {
        let operation = manager
            .registry()
            .set_root_binding_state(
                &request.operation_uuid,
                &library_uuid,
                &root_uuid,
                &command,
                &request_hash,
                &body,
                request.expected_operation_revision,
                request.expected_binding_generation,
            )
            .map_err(sync_folder_error)?;
        let binding = manager
            .registry()
            .local_root_binding(&library_uuid, &root_uuid)
            .map_err(sync_folder_error)?
            .ok_or_else(|| sync_folder_error(anyhow::anyhow!("local-root-binding-not-found")))?;
        let root_uuid_parsed =
            uuid::Uuid::parse_str(&root_uuid).map_err(|error| sync_folder_error(error.into()))?;
        manager
            .write_sync_folder_catalog(&library_uuid, move |database| {
                let mut projected = database
                    .sync_root_local_binding(root_uuid_parsed.as_bytes())?
                    .ok_or_else(|| anyhow::anyhow!("local-root-binding-not-found"))?;
                projected.state = binding.state;
                projected.operation_revision = binding.operation_revision;
                projected.updated_at = crate::host_registry::unix_timestamp_millis_for_runtime();
                database.upsert_sync_root_local_binding(&projected)
            })
            .await
            .map_err(sync_folder_error)?;
        operation
    };
    Ok((StatusCode::ACCEPTED, Json(root_operation_json(&operation))).into_response())
}

async fn sync_folder_contract_operation_read(
    State(state): State<AppState>,
    Path((library_uuid, root_uuid, operation_uuid)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    validate_contract_uuid(&root_uuid, "rootUuid")?;
    validate_contract_uuid(&operation_uuid, "operationUuid")?;
    sync_folder_v2_gate(&state, &library_uuid)?;
    let manager = sync_folder_library_manager(&state)?;
    let operation = manager
        .registry()
        .root_operation(&operation_uuid)
        .map_err(sync_folder_error)?
        .filter(|operation| {
            operation.library_uuid == library_uuid && operation.root_uuid == root_uuid
        })
        .ok_or_else(|| {
            CoreError::coded(
                StatusCode::NOT_FOUND,
                "operation-not-found",
                "Root operation not found".to_owned(),
            )
        })?;
    Ok(
        Json(
            root_operation_detail_json(manager.registry(), &operation)
                .map_err(sync_folder_error)?,
        )
        .into_response(),
    )
}

async fn sync_folder_contract_plan_read(
    State(state): State<AppState>,
    Path((library_uuid, root_uuid, plan_uuid)): Path<(String, String, String)>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<SyncFolderComparisonItemsQuery>,
) -> ApiResult<Response> {
    validate_contract_uuid(&root_uuid, "rootUuid")?;
    validate_contract_uuid(&plan_uuid, "planUuid")?;
    sync_folder_v2_gate(&state, &library_uuid)?;
    let manager = sync_folder_library_manager(&state)?;
    if uri.path().ends_with("/items") {
        let cursor = query.cursor.unwrap_or(0);
        let limit = query.limit.unwrap_or(200).clamp(1, 500);
        let items = manager
            .registry()
            .root_comparison_items(&library_uuid, &root_uuid, &plan_uuid, cursor, limit)
            .map_err(sync_folder_error)?;
        let next = (items.len() == limit as usize)
            .then(|| items.last().map(|item| item.item_ordinal.saturating_add(1)))
            .flatten();
        return Ok(Json(serde_json::json!({"items":items,"nextCursor":next})).into_response());
    }
    let plan = manager
        .registry()
        .root_comparison_plan(&library_uuid, &root_uuid, &plan_uuid)
        .map_err(sync_folder_error)?
        .ok_or_else(|| {
            CoreError::coded(
                StatusCode::NOT_FOUND,
                "operation-not-found",
                "Comparison plan not found".to_owned(),
            )
        })?;
    Ok(Json(plan).into_response())
}

async fn sync_folder_contract_comparison_item_write(
    State(state): State<AppState>,
    Path((library_uuid, root_uuid, plan_uuid, item_ordinal)): Path<(String, String, String, u64)>,
    headers: HeaderMap,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> ApiResult<Response> {
    validate_contract_uuid(&root_uuid, "rootUuid")?;
    validate_contract_uuid(&plan_uuid, "planUuid")?;
    let Json(body) = body.map_err(|error| {
        CoreError::coded(StatusCode::BAD_REQUEST, "invalid-input", error.body_text())
    })?;
    let _: SyncFolderComparisonResolutionMutation = deserialize_sync_folder_body(&body)?;
    validate_sync_folder_mutation_body(&body, false)?;
    require_sync_folder_binding_generation(&body, "comparison resolution")?;
    let expected = require_context_revision(&headers)?;
    sync_folder_library_manager(&state)?
        .ensure_context_revision(expected)
        .map_err(map_library_error)?;
    sync_folder_v2_gate(&state, &library_uuid)?;
    let request: SyncFolderComparisonResolutionMutation = deserialize_sync_folder_body(&body)?;
    let resolution = match request.resolution {
        SyncFolderComparisonResolution::KeepLocal => "keep-local",
        SyncFolderComparisonResolution::KeepRemote => "keep-remote",
        SyncFolderComparisonResolution::KeepBoth => "keep-both",
    };
    let item = sync_folder_library_manager(&state)?
        .registry()
        .resolve_comparison_item(
            &library_uuid,
            &root_uuid,
            &plan_uuid,
            item_ordinal,
            &request.operation_uuid,
            resolution,
            request.expected_operation_revision,
            request.expected_binding_generation,
        )
        .map_err(sync_folder_error)?;
    Ok(Json(item).into_response())
}

async fn sync_folder_contract_operation_action(
    State(state): State<AppState>,
    Path((library_uuid, root_uuid, operation_uuid, action)): Path<(String, String, String, String)>,
    headers: HeaderMap,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> ApiResult<Response> {
    if !matches!(action.as_str(), "cancel" | "retry") {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid-input",
            "Unknown synchronized-folder operation action".to_string(),
        ));
    }
    validate_contract_uuid(&root_uuid, "rootUuid")?;
    validate_contract_uuid(&operation_uuid, "operationUuid")?;
    let Json(body) = body.map_err(|error| {
        CoreError::coded(StatusCode::BAD_REQUEST, "invalid-input", error.body_text())
    })?;
    let request: SyncFolderOperationFenceMutation = deserialize_sync_folder_body(&body)?;
    validate_sync_folder_mutation_body(&body, false)?;
    require_sync_folder_binding_generation(&body, "relocation commands")?;
    if body
        .get("operationUuid")
        .and_then(serde_json::Value::as_str)
        != Some(operation_uuid.as_str())
    {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid-input",
            "operationUuid must match the operation path".to_string(),
        ));
    }
    let expected = require_context_revision(&headers)?;
    sync_folder_library_manager(&state)?
        .ensure_context_revision(expected)
        .map_err(map_library_error)?;
    sync_folder_v2_gate(&state, &library_uuid)?;
    let manager = sync_folder_library_manager(&state)?;
    manager
        .registry()
        .root_relocation_operation(&operation_uuid)
        .map_err(sync_folder_error)?
        .filter(|operation| {
            operation.operation.library_uuid == library_uuid
                && operation.operation.root_uuid == root_uuid
        })
        .ok_or_else(|| sync_folder_error(anyhow::anyhow!("root-operation-not-found")))?;
    let operation = if action == "cancel" {
        manager
            .cancel_root_relocation(
                &operation_uuid,
                request.expected_operation_revision,
                request.expected_binding_generation,
            )
            .await
            .map_err(sync_folder_error)?
    } else {
        manager
            .retry_root_relocation(
                &operation_uuid,
                request.expected_operation_revision,
                request.expected_binding_generation,
            )
            .await
            .map_err(sync_folder_error)?
    };
    Ok((StatusCode::ACCEPTED, Json(root_operation_json(&operation))).into_response())
}

async fn sync_folder_contract_conflict_write(
    State(state): State<AppState>,
    Path((library_uuid, root_uuid, conflict_uuid)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Result<Json<serde_json::Value>, JsonRejection>,
) -> ApiResult<Response> {
    validate_contract_uuid(&root_uuid, "rootUuid")?;
    validate_contract_uuid(&conflict_uuid, "conflictUuid")?;
    let Json(body) = body.map_err(|error| {
        CoreError::coded(StatusCode::BAD_REQUEST, "invalid-input", error.body_text())
    })?;
    let request: SyncFolderConflictResolutionMutation = deserialize_sync_folder_body(&body)?;
    if request.requested_name.as_deref() == Some("") {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid-input",
            "requestedName must not be empty".to_string(),
        ));
    }
    validate_sync_folder_mutation_body(&body, true)?;
    let expected = require_context_revision(&headers)?;
    sync_folder_library_manager(&state)?
        .ensure_context_revision(expected)
        .map_err(map_library_error)?;
    sync_folder_v2_gate(&state, &library_uuid)?;
    Err(CoreError::coded(
        StatusCode::CONFLICT,
        "migration-required",
        "Root conflict resolution is not installed".to_owned(),
    ))
}

fn library_summary(
    descriptor: crate::host_registry::LibraryDescriptor,
    active_library_uuid: Option<&str>,
    context_revision: u64,
) -> LibrarySummaryResponse {
    LibrarySummaryResponse {
        active: active_library_uuid == Some(descriptor.library_uuid.as_str()),
        library_uuid: descriptor.library_uuid,
        root_uuid: descriptor.root_uuid,
        name: descriptor.name,
        icon: descriptor.icon,
        color: descriptor.color,
        database_path: descriptor.managed_root.to_string_lossy().into_owned(),
        availability: descriptor.availability,
        lifecycle_state: descriptor.lifecycle_state,
        binding_state: descriptor.binding_state,
        context_revision,
    }
}

async fn list_libraries(State(state): State<AppState>) -> ApiResult<Json<LibraryListResponse>> {
    let manager = library_manager(&state)?;
    let _gate = state.request_gate().read_owned().await;
    let snapshot = manager.registry().listed_snapshot()?;
    let active_snapshot = manager.active_snapshot();
    let active = active_snapshot.library_uuid.clone();
    Ok(Json(LibraryListResponse {
        context_revision: active_snapshot.context_revision,
        items: snapshot
            .libraries
            .into_iter()
            .map(|descriptor| {
                library_summary(
                    descriptor,
                    active.as_deref(),
                    active_snapshot.context_revision,
                )
            })
            .collect(),
    }))
}

async fn get_active_library(
    State(state): State<AppState>,
) -> ApiResult<Json<ActiveLibraryResponse>> {
    let manager = library_manager(&state)?;
    let _gate = state.request_gate().read_owned().await;
    let snapshot = manager.active_snapshot();
    let library = snapshot
        .library_uuid
        .as_deref()
        .and_then(|uuid| manager.registry().library(uuid).transpose())
        .transpose()?
        .map(|descriptor| {
            library_summary(
                descriptor,
                snapshot.library_uuid.as_deref(),
                snapshot.context_revision,
            )
        });
    Ok(Json(ActiveLibraryResponse {
        library,
        context_revision: snapshot.context_revision,
        runtime_generation: snapshot.runtime_generation,
    }))
}

async fn preflight_library(
    State(state): State<AppState>,
    Json(request): Json<LibraryPreflightRequest>,
) -> ApiResult<Json<LibraryPreflightResponse>> {
    let manager = library_manager(&state)?;
    let _admission = manager.path_admission().await;
    let result = manager.preflight_plan(
        &request.name,
        FilePath::new(&request.database_path),
        &request.content_roots,
    )?;
    Ok(Json(LibraryPreflightResponse {
        ok: result.ok,
        reason: result.reason.as_str().to_string(),
        path_role: result.path_role,
        root_index: result.root_index,
        normalized_database_path: result
            .normalized_database_path
            .map(|path| path.to_string_lossy().into_owned()),
        normalized_content_roots: result.normalized_content_roots,
        preflight_token: result.token,
        available_bytes: result.available_bytes,
    }))
}

async fn create_library(
    State(state): State<AppState>,
    Json(request): Json<CreateLibraryRequest>,
) -> ApiResult<(StatusCode, Json<CreateLibraryResponse>)> {
    if request.initial_mode != "local-only" {
        return Err(CoreError::bad_request(
            "initialMode must be local-only".to_string(),
        ));
    }
    let manager = library_manager(&state)?;
    let (descriptor, activated, activation_error) = manager
        .create_library_with_content(
            request.name,
            request.icon,
            request.color,
            std::path::PathBuf::from(request.database_path),
            request.content_roots,
            request.preflight_token,
            request.activate.unwrap_or(true),
        )
        .await
        .map_err(map_library_error)?;
    let snapshot = manager.registry().snapshot()?;
    Ok((
        StatusCode::CREATED,
        Json(CreateLibraryResponse {
            library: library_summary(
                descriptor,
                snapshot.active_library_uuid.as_deref(),
                snapshot.context_revision,
            ),
            activated,
            activation_error,
        }),
    ))
}

async fn open_library(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<OpenLibraryRequest>,
) -> ApiResult<Json<CreateLibraryResponse>> {
    let expected_revision = require_context_revision(&headers)?;
    let manager = library_manager(&state)?;
    // Keep the owned operation alive if its requesting window closes, including
    // the blocking registration turn and runtime shutdown/publication.
    let owner = Arc::clone(&manager);
    let (descriptor, activated, activation_error) = tokio::spawn(async move {
        owner
            .open_library_with_revision(request.database_path.into(), expected_revision)
            .await
    })
    .await
    .map_err(|error| CoreError::internal(error.to_string()))?
    .map_err(map_library_error)?;
    let _gate = state.request_gate().read_owned().await;
    let snapshot = manager.active_snapshot();
    Ok(Json(CreateLibraryResponse {
        library: library_summary(
            descriptor,
            snapshot.library_uuid.as_deref(),
            snapshot.context_revision,
        ),
        activated,
        activation_error,
    }))
}

async fn forget_library(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<ActiveLibraryResponse>> {
    let expected_revision = require_context_revision(&headers)?;
    let owner = library_manager(&state)?;
    tokio::spawn(async move {
        owner
            .forget_library_with_revision(&library_uuid, expected_revision)
            .await
    })
    .await
    .map_err(|error| CoreError::internal(error.to_string()))?
    .map_err(map_library_error)?;
    get_active_library(State(state)).await
}

async fn activate_library(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<ActiveLibraryResponse>> {
    let expected_revision = require_context_revision(&headers)?;
    let owner = library_manager(&state)?;
    // Runtime retirement owns background joins. Keep the accepted operation
    // alive when the requester disconnects, just as open and forget do.
    tokio::spawn(async move {
        owner
            .activate_with_revision(&library_uuid, expected_revision)
            .await
    })
    .await
    .map_err(|error| CoreError::internal(error.to_string()))?
    .map_err(map_library_error)?;
    get_active_library(State(state)).await
}

async fn update_library(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateLibraryRequest>,
) -> ApiResult<Json<LibrarySummaryResponse>> {
    let expected_revision = require_context_revision(&headers)?;
    if request
        .name
        .as_deref()
        .is_some_and(|name| name.trim().is_empty())
    {
        return Err(CoreError::bad_request(
            "Library name cannot be empty".to_string(),
        ));
    }
    let manager = library_manager(&state)?;
    let descriptor = manager
        .update_library_metadata_with_revision(
            &library_uuid,
            expected_revision,
            request.name.as_deref(),
            request.icon.as_ref().map(|value| value.as_deref()),
            request.color.as_ref().map(|value| value.as_deref()),
        )
        .await
        .map_err(map_library_error)?
        .ok_or_else(|| CoreError::not_found("library-not-found".to_string()))?;
    let snapshot = manager.registry().snapshot()?;
    Ok(Json(library_summary(
        descriptor,
        snapshot.active_library_uuid.as_deref(),
        snapshot.context_revision,
    )))
}

async fn remove_local_library(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let expected_revision = require_context_revision(&headers)?;
    library_manager(&state)?
        .remove_local_library_with_revision(&library_uuid, expected_revision)
        .await
        .map_err(map_library_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn enable_library_sync(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
    Json(request): Json<EnableLibrarySyncRequest>,
) -> ApiResult<Json<LibrarySummaryResponse>> {
    let expected_revision = require_context_revision(&headers)?;
    let manager = library_manager(&state)?;
    manager
        .enable_library_sync_with_revision(
            &library_uuid,
            expected_revision,
            &request.provider_id,
            &request.account_identity,
        )
        .await
        .map_err(map_library_error)?;
    let snapshot = manager.active_snapshot();
    let descriptor = manager
        .registry()
        .library(&library_uuid)?
        .ok_or_else(|| CoreError::not_found("library-not-found".to_string()))?;
    Ok(Json(library_summary(
        descriptor,
        snapshot.library_uuid.as_deref(),
        snapshot.context_revision,
    )))
}

async fn unbind_library_sync(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    let expected_revision = require_context_revision(&headers)?;
    library_manager(&state)?
        .unbind_library_sync_with_revision(&library_uuid, expected_revision)
        .await
        .map_err(map_library_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_sync_status(
    State(state): State<AppState>,
) -> ApiResult<Json<crate::host_registry::SyncStatusSnapshot>> {
    Ok(Json(
        library_manager(&state)?.registry().sync_status_snapshot()?,
    ))
}

async fn wake_sync(State(state): State<AppState>) -> StatusCode {
    // The renderer can regain focus while Core is still finishing Host bootstrap.
    // Treat that early wake as an idempotent no-op instead of surfacing a 409 to
    // the desktop shell; the normal startup tick will pick up the same work.
    if let Some(manager) = state.library_manager.as_ref() {
        manager.wake_sync_for_foreground_or_network();
    }
    StatusCode::NO_CONTENT
}

async fn list_remote_libraries(
    State(state): State<AppState>,
) -> ApiResult<Json<ListResponse<crate::host_registry::RemoteLibrarySummary>>> {
    Ok(Json(ListResponse {
        items: library_manager(&state)?
            .registry()
            .list_remote_libraries()?,
        next_cursor: None,
        total_count: None,
    }))
}

async fn list_remote_library_roots(
    State(state): State<AppState>,
    Path((account_uuid, library_uuid)): Path<(String, String)>,
    Query(query): Query<RemoteRootQuery>,
) -> ApiResult<Json<RemoteRootListResponse>> {
    validate_contract_uuid(&library_uuid, "libraryUuid")?;
    if let Some(cursor) = query.cursor.as_deref() {
        validate_contract_uuid(cursor, "cursor")?;
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let inventory = library_manager(&state)?
        .registry()
        .remote_library_root_inventory(
            &account_uuid,
            &library_uuid,
            query.cursor.as_deref(),
            limit + 1,
        )?;
    let mut items = inventory.items;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|item| item.root_uuid.clone())
    } else {
        None
    };
    Ok(Json(RemoteRootListResponse {
        items,
        next_cursor,
        inventory_fence: inventory.inventory_fence,
    }))
}

async fn sync_folder_contract_operations_read(
    State(state): State<AppState>,
    Path((library_uuid, root_uuid)): Path<(String, String)>,
    Query(query): Query<RootOperationQuery>,
) -> ApiResult<Response> {
    validate_contract_uuid(&root_uuid, "rootUuid")?;
    if let Some(cursor) = query.cursor.as_deref() {
        validate_contract_uuid(cursor, "cursor")?;
    }
    sync_folder_v2_gate(&state, &library_uuid)?;
    let state_filter = query.state.as_deref().unwrap_or("all");
    if !matches!(state_filter, "all" | "active" | "terminal") {
        return Err(CoreError::bad_request(
            "invalid operation state filter".to_owned(),
        ));
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let manager = sync_folder_library_manager(&state)?;
    let mut operations = manager
        .registry()
        .root_operations(
            &library_uuid,
            &root_uuid,
            query.cursor.as_deref(),
            limit + 1,
            state_filter,
        )
        .map_err(sync_folder_error)?;
    let next_cursor = if operations.len() > limit {
        operations.truncate(limit);
        operations.last().map(|item| item.operation_uuid.clone())
    } else {
        None
    };
    let items = operations
        .iter()
        .map(|operation| root_operation_detail_json(manager.registry(), operation))
        .collect::<anyhow::Result<Vec<_>>>()
        .map_err(sync_folder_error)?;
    Ok(Json(serde_json::json!({"items":items,"nextCursor":next_cursor})).into_response())
}

async fn sync_preflight(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    Json(request): Json<SyncPreflightRequest>,
) -> ApiResult<Json<SyncPreflightResponse>> {
    let manager = library_manager(&state)?;
    if manager.registry().library(&library_uuid)?.is_none() {
        return Err(CoreError::not_found("library-not-found".into()));
    }
    manager
        .registry()
        .require_ready_sync_provider(&request.provider_id, &request.account_uuid)
        .map_err(map_library_error)?;
    let token = manager.registry().create_sync_preflight_token(
        &library_uuid,
        &request.provider_id,
        &request.account_uuid,
        request.remote_library_uuid.as_deref(),
    )?;
    Ok(Json(SyncPreflightResponse {
        ok: true,
        reason: "ok".into(),
        preflight_token: Some(token),
        remote_library_uuid: request.remote_library_uuid,
    }))
}

async fn start_sync_enable(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SyncEnableRequest>,
) -> ApiResult<Json<crate::host_registry::SyncOperation>> {
    let expected = require_context_revision(&headers)?;
    let manager = library_manager(&state)?;
    manager
        .ensure_context_revision(expected)
        .map_err(map_library_error)?;
    let (operation, inserted) = manager.registry().begin_sync_operation_once(
        &request.operation_uuid,
        Some(&library_uuid),
        "enable",
        "capturing",
        &serde_json::json!({"providerId":request.provider_id,"accountUuid":request.account_uuid,"preflightToken":request.preflight_token}),
    )?;
    if !inserted {
        return Ok(Json(operation));
    }
    let _ = manager
        .publish_sync_operation_notification(&operation)
        .await;
    if let Err(error) = manager.registry().consume_sync_preflight_token(
        &request.preflight_token,
        &library_uuid,
        &request.provider_id,
        &request.account_uuid,
    ) {
        let code = error.to_string().replace('-', "_");
        if let Ok(finished) = manager.registry().finish_sync_operation(
            &operation.operation_uuid,
            "failed",
            "preflight",
            Some(&code),
        ) {
            let _ = manager.publish_sync_operation_notification(&finished).await;
        }
        return Err(map_library_error(error));
    }
    match manager
        .enable_library_sync_with_revision(
            &library_uuid,
            expected,
            &request.provider_id,
            &request.account_uuid,
        )
        .await
    {
        Ok(()) => {
            let finished = manager.registry().finish_sync_operation(
                &operation.operation_uuid,
                "waiting",
                "uploading",
                None,
            )?;
            let _ = manager.publish_sync_operation_notification(&finished).await;
            Ok(Json(finished))
        }
        Err(error) => {
            let code = error.to_string().replace('-', "_");
            if let Ok(finished) = manager.registry().finish_sync_operation(
                &operation.operation_uuid,
                "failed",
                "retry-wait",
                Some(&code),
            ) {
                let _ = manager.publish_sync_operation_notification(&finished).await;
            }
            Err(map_library_error(error))
        }
    }
}

async fn run_sync_command(
    state: AppState,
    library_uuid: String,
    headers: HeaderMap,
    request: SyncCommandRequest,
    action: &'static str,
) -> ApiResult<Json<crate::host_registry::SyncOperation>> {
    let expected = require_context_revision(&headers)?;
    let manager = library_manager(&state)?;
    let (operation, inserted) = manager
        .registry()
        .begin_sync_operation_once(
            &request.operation_uuid,
            Some(&library_uuid),
            action,
            "active",
            &serde_json::json!({}),
        )
        .map_err(map_library_error)?;
    if !inserted {
        return Ok(Json(operation));
    }
    let _ = manager
        .publish_sync_operation_notification(&operation)
        .await;
    let result = match action {
        "pause" => {
            manager
                .pause_library_sync_with_revision(&library_uuid, expected)
                .await
        }
        "resume" => {
            manager
                .resume_library_sync_with_revision(&library_uuid, expected)
                .await
        }
        "retry" => {
            manager
                .retry_library_sync_with_revision(&library_uuid, expected)
                .await
        }
        _ => {
            manager
                .wake_library_sync_with_revision(&library_uuid, expected)
                .await
        }
    };
    match result {
        Ok(()) => {
            let finished = manager.registry().finish_sync_operation(
                &operation.operation_uuid,
                "complete",
                "complete",
                None,
            )?;
            let _ = manager.publish_sync_operation_notification(&finished).await;
            Ok(Json(finished))
        }
        Err(error) => {
            let code = error.to_string().replace('-', "_");
            if let Ok(finished) = manager.registry().finish_sync_operation(
                &operation.operation_uuid,
                "failed",
                "retry-wait",
                Some(&code),
            ) {
                let _ = manager.publish_sync_operation_notification(&finished).await;
            }
            Err(map_library_error(error))
        }
    }
}

async fn pause_library_sync(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SyncCommandRequest>,
) -> ApiResult<Json<crate::host_registry::SyncOperation>> {
    run_sync_command(state, library_uuid, headers, request, "pause").await
}
async fn resume_library_sync(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SyncCommandRequest>,
) -> ApiResult<Json<crate::host_registry::SyncOperation>> {
    run_sync_command(state, library_uuid, headers, request, "resume").await
}
async fn sync_library_now(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SyncCommandRequest>,
) -> ApiResult<Json<crate::host_registry::SyncOperation>> {
    run_sync_command(state, library_uuid, headers, request, "sync-now").await
}
async fn retry_library_sync(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
    Json(request): Json<SyncCommandRequest>,
) -> ApiResult<Json<crate::host_registry::SyncOperation>> {
    run_sync_command(state, library_uuid, headers, request, "retry").await
}

async fn create_unbind_plan(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
) -> ApiResult<Json<crate::host_registry::SyncUnbindPlan>> {
    Ok(Json(
        library_manager(&state)?
            .registry()
            .create_unbind_plan(&library_uuid)?,
    ))
}

async fn commit_unbind_plan(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UnbindCommitRequest>,
) -> ApiResult<Json<crate::host_registry::SyncOperation>> {
    if !matches!(
        request.choice.as_str(),
        "drain-before-unbind" | "keep-local-fork"
    ) {
        return Err(CoreError::bad_request("invalid unbind choice".into()));
    }
    let expected = require_context_revision(&headers)?;
    let manager = library_manager(&state)?;
    let (operation, inserted) = manager.registry().begin_sync_operation_once(
        &request.operation_uuid,
        Some(&library_uuid),
        "unbind",
        "reconcile",
        &serde_json::json!({"planUuid":request.plan_uuid,"choice":request.choice}),
    )?;
    if !inserted {
        return Ok(Json(operation));
    }
    let _ = manager
        .publish_sync_operation_notification(&operation)
        .await;
    manager
        .registry()
        .attach_unbind_operation(&request.plan_uuid, &operation.operation_uuid)?;
    let completed = match manager
        .commit_unbind_plan_with_revision(
            &library_uuid,
            expected,
            &request.plan_uuid,
            &request.choice,
        )
        .await
    {
        Ok(completed) => completed,
        Err(error) => {
            let code = error.to_string().replace('-', "_");
            if let Ok(finished) = manager.registry().finish_sync_operation(
                &operation.operation_uuid,
                "failed",
                "retry-wait",
                Some(&code),
            ) {
                let _ = manager.publish_sync_operation_notification(&finished).await;
            }
            return Err(map_library_error(error));
        }
    };
    let finished = manager.registry().finish_sync_operation(
        &operation.operation_uuid,
        if completed { "complete" } else { "waiting" },
        if completed { "complete" } else { "retry-wait" },
        if completed {
            None
        } else {
            Some("remote_visibility_unknown")
        },
    )?;
    let _ = manager.publish_sync_operation_notification(&finished).await;
    Ok(Json(finished))
}

async fn reconnect_preflight(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    Json(request): Json<ReconnectPreflightRequest>,
) -> ApiResult<Json<crate::host_registry::SyncReconnectPlan>> {
    Ok(Json(
        library_manager(&state)?
            .create_reconnect_plan(
                &library_uuid,
                &request.account_uuid,
                &request.remote_library_uuid,
            )
            .await?,
    ))
}

async fn reconnect_commit(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ReconnectCommitRequest>,
) -> ApiResult<Json<crate::host_registry::SyncOperation>> {
    if !matches!(
        request.choice.as_str(),
        "merge-local-fork" | "publish-new-remote"
    ) {
        return Err(CoreError::bad_request("invalid reconnect choice".into()));
    }
    let expected = require_context_revision(&headers)?;
    let manager = library_manager(&state)?;
    let (operation, inserted) = manager.registry().begin_sync_operation_once(
        &request.operation_uuid,
        Some(&library_uuid),
        "reconnect",
        "capturing",
        &serde_json::json!({"planUuid":request.plan_uuid,"choice":request.choice}),
    )?;
    if !inserted {
        return Ok(Json(operation));
    }
    let _ = manager
        .publish_sync_operation_notification(&operation)
        .await;
    if let Err(error) = manager
        .commit_reconnect_plan_with_revision(
            &library_uuid,
            expected,
            &request.plan_uuid,
            &request.choice,
        )
        .await
    {
        let code = error.to_string().replace('-', "_");
        if let Ok(finished) = manager.registry().finish_sync_operation(
            &operation.operation_uuid,
            "failed",
            "retry-wait",
            Some(&code),
        ) {
            let _ = manager.publish_sync_operation_notification(&finished).await;
        }
        return Err(map_library_error(error));
    }
    let finished = manager.registry().finish_sync_operation(
        &operation.operation_uuid,
        "waiting",
        "uploading",
        None,
    )?;
    let _ = manager.publish_sync_operation_notification(&finished).await;
    Ok(Json(finished))
}

fn invalid_restore_preflight(
    total_items: usize,
    root_results: Vec<crate::host_registry::SyncRestoreRootResult>,
    volume_results: Vec<crate::host_registry::SyncRestoreVolumeResult>,
) -> crate::host_registry::SyncRestorePreflight {
    crate::host_registry::SyncRestorePreflight {
        preflight_token: None,
        expires_at: 0,
        total_items: total_items as u64,
        total_bytes: volume_results.iter().map(|item| item.required_bytes).sum(),
        ok: false,
        root_results,
        volume_results,
    }
}

fn volume_key(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    if let Some(rest) = text.strip_prefix("\\\\") {
        let mut parts = rest.split('\\');
        return format!(
            "unc://{}/{}",
            parts.next().unwrap_or_default().to_lowercase(),
            parts.next().unwrap_or_default().to_lowercase()
        );
    }
    path.components()
        .next()
        .map(|value| value.as_os_str().to_string_lossy().to_lowercase())
        .unwrap_or_else(|| text.to_lowercase())
}

fn validate_restore_batch_paths(
    databases: &[std::path::PathBuf],
    content: &[std::path::PathBuf],
) -> Result<(), &'static str> {
    if databases.iter().any(|database| {
        content
            .iter()
            .any(|root| crate::library_path::paths_overlap(database, root))
    }) {
        return Err("database-content-overlap");
    }
    if databases.iter().enumerate().any(|(index, path)| {
        databases[..index]
            .iter()
            .any(|previous| crate::library_path::paths_overlap(previous, path))
    }) {
        return Err("nested-library-root");
    }
    if content.iter().enumerate().any(|(index, path)| {
        content[..index]
            .iter()
            .any(|previous| crate::library_path::paths_overlap(previous, path))
    }) {
        return Err("content-roots-overlap");
    }
    Ok(())
}

fn validate_restore_items(
    manager: &crate::library_manager::LibraryManager,
    items: &[RestoreStartItem],
    persist: bool,
) -> ApiResult<crate::host_registry::SyncRestorePreflight> {
    if items.is_empty() {
        return Err(CoreError::bad_request(
            "restore items cannot be empty".into(),
        ));
    }
    let root_aware = items
        .iter()
        .any(|item| matches!(item, RestoreStartItem::RootAware(_)));
    if root_aware
        && items
            .iter()
            .any(|item| matches!(item, RestoreStartItem::Legacy(_)))
    {
        return Err(CoreError::bad_request("mixed-restore-modes".into()));
    }
    for item in items {
        validate_contract_uuid(restore_item_identity(item).1, "libraryUuid")?;
    }
    manager
        .registry()
        .ensure_restore_libraries_absent(items.iter().map(|item| restore_item_identity(item).1))
        .map_err(map_library_error)?;
    let mut normalized_roots = Vec::with_capacity(items.len());
    let mut batch_databases = Vec::new();
    let mut batch_content = Vec::new();
    let mut selected_restore_paths = Vec::<std::path::PathBuf>::new();
    let remote_inventory = manager.registry().list_remote_libraries()?;
    let mut required_by_volume = std::collections::BTreeMap::<String, u64>::new();
    let mut available_by_volume = std::collections::BTreeMap::<String, u64>::new();
    let mut root_results = Vec::new();
    let mut submitted_libraries = std::collections::BTreeSet::new();
    for item in items {
        if let RestoreStartItem::RootAware(item) = item {
            validate_contract_uuid(&item.library_uuid, "libraryUuid")?;
            if item.inventory_fence.len() != 64
                || !item
                    .inventory_fence
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(CoreError::bad_request("invalid-inventory-fence".into()));
            }
            for root in &item.roots {
                validate_contract_uuid(&root.root_uuid, "rootUuid")?;
                if root.expected_materialization_fingerprint.len() != 64
                    || !root
                        .expected_materialization_fingerprint
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(CoreError::bad_request(
                        "invalid-materialization-fingerprint".into(),
                    ));
                }
            }
        }
        let (account_uuid, library_uuid, name, managed_root) = match item {
            RestoreStartItem::Legacy(item) => (
                &item.account_uuid,
                &item.library_uuid,
                &item.name,
                &item.placement_base,
            ),
            RestoreStartItem::RootAware(item) => {
                let Some(remote) = remote_inventory.iter().find(|remote| {
                    remote.account_uuid == item.account_uuid
                        && remote.library_uuid == item.library_uuid
                        && remote.compatible
                }) else {
                    return Ok(invalid_restore_preflight(items.len(), vec![], vec![]));
                };
                let inventory = manager
                    .registry()
                    .remote_library_root_inventory(
                        &item.account_uuid,
                        &item.library_uuid,
                        None,
                        usize::MAX,
                    )
                    .map_err(map_library_error)?;
                let mut reasons = std::collections::BTreeMap::<String, String>::new();
                if inventory.inventory_fence != item.inventory_fence {
                    reasons.insert(String::new(), "stale-inventory".into());
                }
                let expected = inventory
                    .items
                    .iter()
                    .map(|root| root.root_uuid.as_str())
                    .collect::<std::collections::BTreeSet<_>>();
                let submitted = item
                    .roots
                    .iter()
                    .map(|root| root.root_uuid.as_str())
                    .collect::<std::collections::BTreeSet<_>>();
                if submitted.len() != item.roots.len() {
                    reasons.insert(String::new(), "duplicate-root".into());
                }
                if expected != submitted {
                    reasons.insert(String::new(), "incomplete-root-inventory".into());
                }
                for missing in inventory
                    .items
                    .iter()
                    .filter(|root| !submitted.contains(root.root_uuid.as_str()))
                {
                    root_results.push(crate::host_registry::SyncRestoreRootResult {
                        library_uuid: item.library_uuid.clone(),
                        root_uuid: missing.root_uuid.clone(),
                        selected: false,
                        ok: false,
                        reason: Some("missing-root".into()),
                        normalized_target_path: None,
                        required_bytes: missing.logical_bytes.unwrap_or(0),
                        available_bytes: None,
                        volume_key: None,
                    });
                }
                for choice in &item.roots {
                    let fact = inventory
                        .items
                        .iter()
                        .find(|root| root.root_uuid == choice.root_uuid);
                    let mut reason = reasons.get("").cloned();
                    if fact.is_none() {
                        reason = Some("foreign-root".into());
                    }
                    if let Some(fact) = fact {
                        if fact.logical_revision != choice.expected_logical_revision
                            || fact.materialization_fingerprint
                                != choice.expected_materialization_fingerprint
                        {
                            reason = Some("stale-root-facts".into());
                        }
                    }
                    if choice.selected != choice.target_path.is_some() {
                        reason = Some("target-selection-mismatch".into());
                    }
                    let mut normalized_target_path = None;
                    let mut available_bytes = None;
                    let mut root_volume = None;
                    if reason.is_none() && choice.selected {
                        match crate::sync_folders::validate_root_path(
                            FilePath::new(choice.target_path.as_deref().unwrap_or_default()),
                            crate::sync_folders::RootPathMode::MaterializeRemote,
                            FilePath::new(&item.placement_base),
                            &selected_restore_paths,
                        ) {
                            Ok(path) => {
                                root_volume =
                                    Some(volume_key(FilePath::new(&path.normalized_path)));
                                available_bytes = path.available_bytes;
                                selected_restore_paths
                                    .push(FilePath::new(&path.normalized_path).to_path_buf());
                                normalized_target_path = Some(path.normalized_path);
                            }
                            Err(error) => reason = Some(error.to_string()),
                        }
                    }
                    let required_bytes = fact.and_then(|root| root.logical_bytes).unwrap_or(0);
                    if let Some(volume) = root_volume.as_ref() {
                        *required_by_volume.entry(volume.clone()).or_default() = required_by_volume
                            .get(volume)
                            .copied()
                            .unwrap_or(0)
                            .saturating_add(required_bytes);
                        if let Some(available) = available_bytes {
                            available_by_volume
                                .entry(volume.clone())
                                .and_modify(|v| *v = (*v).min(available))
                                .or_insert(available);
                        }
                    }
                    root_results.push(crate::host_registry::SyncRestoreRootResult {
                        library_uuid: item.library_uuid.clone(),
                        root_uuid: choice.root_uuid.clone(),
                        selected: choice.selected,
                        ok: reason.is_none(),
                        reason,
                        normalized_target_path,
                        required_bytes,
                        available_bytes,
                        volume_key: root_volume,
                    });
                }
                (
                    &item.account_uuid,
                    &item.library_uuid,
                    &remote.name,
                    &item.placement_base,
                )
            }
        };
        if !submitted_libraries.insert((account_uuid.clone(), library_uuid.clone())) {
            return Err(CoreError::bad_request("duplicate-restore-library".into()));
        }
        let database_path = FilePath::new(managed_root).join("Database");
        let content_roots = match item {
            RestoreStartItem::Legacy(_) => vec![crate::library_path::LibraryContentRootInput {
                path: FilePath::new(managed_root)
                    .join("Files")
                    .to_string_lossy()
                    .into_owned(),
                create_if_missing: true,
            }],
            RestoreStartItem::RootAware(item) => item
                .roots
                .iter()
                .filter_map(|choice| choice.target_path.as_ref().filter(|_| choice.selected))
                .map(|path| crate::library_path::LibraryContentRootInput {
                    path: path.clone(),
                    create_if_missing: true,
                })
                .collect(),
        };
        let plan = manager.preflight_restore_plan(name, &database_path, &content_roots)?;
        if let (Some(database), Some(roots)) = (
            &plan.normalized_database_path,
            &plan.normalized_content_roots,
        ) {
            batch_databases.push(database.clone());
            batch_content.extend(
                roots
                    .iter()
                    .map(|root| std::path::PathBuf::from(&root.path)),
            );
        }
        let check = crate::library_path::PreflightResult {
            ok: plan.ok,
            reason: plan.reason,
            normalized_root: plan.normalized_database_path.unwrap_or(database_path),
            available_bytes: plan.available_bytes,
        };
        if !check.ok {
            if root_aware {
                root_results.push(crate::host_registry::SyncRestoreRootResult {
                    library_uuid: library_uuid.clone(),
                    root_uuid: String::new(),
                    selected: false,
                    ok: false,
                    reason: Some(check.reason.as_str().into()),
                    normalized_target_path: Some(
                        check.normalized_root.to_string_lossy().into_owned(),
                    ),
                    required_bytes: 0,
                    available_bytes: check.available_bytes,
                    volume_key: Some(volume_key(&check.normalized_root)),
                });
                continue;
            }
            return Err(CoreError::conflict(check.reason.as_str().into()));
        }
        let comparable = check
            .normalized_root
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase();
        let overlaps_restore_path = selected_restore_paths.iter().any(|existing| {
            let existing = existing
                .to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_lowercase();
            comparable == existing
                || comparable.starts_with(&format!("{existing}\\"))
                || existing.starts_with(&format!("{comparable}\\"))
        });
        if overlaps_restore_path
            || normalized_roots.iter().any(|existing: &String| {
                comparable == *existing
                    || comparable.starts_with(&format!("{existing}\\"))
                    || existing.starts_with(&format!("{comparable}\\"))
            })
        {
            if root_aware {
                root_results.push(crate::host_registry::SyncRestoreRootResult {
                    library_uuid: library_uuid.clone(),
                    root_uuid: String::new(),
                    selected: false,
                    ok: false,
                    reason: Some("managed-root-overlap".into()),
                    normalized_target_path: Some(
                        check.normalized_root.to_string_lossy().into_owned(),
                    ),
                    required_bytes: 0,
                    available_bytes: check.available_bytes,
                    volume_key: Some(volume_key(&check.normalized_root)),
                });
                continue;
            }
            return Err(CoreError::conflict("managed-root-overlap".into()));
        }
        let Some(remote) = remote_inventory.iter().find(|remote| {
            remote.account_uuid == *account_uuid
                && remote.library_uuid == *library_uuid
                && remote.compatible
        }) else {
            return Err(CoreError::conflict("remote-library-not-restorable".into()));
        };
        if !root_aware {
            let volume = volume_key(&check.normalized_root);
            let required = remote
                .logical_bytes
                .unwrap_or(remote.descriptor_size)
                .saturating_add(256 * 1024 * 1024);
            *required_by_volume.entry(volume.clone()).or_default() = required_by_volume
                .get(&volume)
                .copied()
                .unwrap_or(0)
                .saturating_add(required);
            if let Some(available) = check.available_bytes {
                available_by_volume
                    .entry(volume)
                    .and_modify(|value| *value = (*value).min(available))
                    .or_insert(available);
            }
        }
        normalized_roots.push(comparable);
    }
    if let Err(reason) = validate_restore_batch_paths(&batch_databases, &batch_content) {
        if !root_aware {
            return Err(CoreError::conflict(reason.to_owned()));
        }
        root_results.push(crate::host_registry::SyncRestoreRootResult {
            library_uuid: String::new(),
            root_uuid: String::new(),
            selected: false,
            ok: false,
            reason: Some(reason.to_owned()),
            normalized_target_path: None,
            required_bytes: 0,
            available_bytes: None,
            volume_key: None,
        });
    }
    if root_aware {
        for required in required_by_volume.values_mut() {
            *required = required.saturating_add(256 * 1024 * 1024);
        }
    }
    let volume_results = required_by_volume
        .iter()
        .map(|(volume, required)| {
            let available = available_by_volume.get(volume).copied();
            crate::host_registry::SyncRestoreVolumeResult {
                volume_key: volume.clone(),
                required_bytes: *required,
                available_bytes: available,
                ok: available.is_none_or(|value| *required <= value),
            }
        })
        .collect::<Vec<_>>();
    root_results.sort_by(|left, right| {
        (&left.library_uuid, &left.root_uuid).cmp(&(&right.library_uuid, &right.root_uuid))
    });
    if root_aware
        && (root_results.iter().any(|result| !result.ok)
            || volume_results.iter().any(|result| !result.ok))
    {
        return Ok(invalid_restore_preflight(
            items.len(),
            root_results,
            volume_results,
        ));
    }
    if !root_aware && volume_results.iter().any(|result| !result.ok) {
        return Err(CoreError::conflict(
            "aggregate-disk-space-insufficient".into(),
        ));
    }
    let request = restore_operation_request(items);
    let mut result = if persist {
        manager.registry().create_restore_preflight(
            &request,
            items.len() as u64,
            required_by_volume.values().copied().sum(),
            &root_results,
            &volume_results,
        )?
    } else {
        crate::host_registry::SyncRestorePreflight {
            preflight_token: None,
            expires_at: 0,
            total_items: items.len() as u64,
            total_bytes: required_by_volume.values().copied().sum(),
            ok: true,
            root_results: Vec::new(),
            volume_results: Vec::new(),
        }
    };
    result.root_results = root_results;
    result.volume_results = volume_results;
    Ok(result)
}

async fn preflight_restore(
    State(state): State<AppState>,
    Json(request): Json<RestorePreflightRequest>,
) -> ApiResult<Json<crate::host_registry::SyncRestorePreflight>> {
    let manager = library_manager(&state)?;
    let _admission = manager.path_admission().await;
    Ok(Json(validate_restore_items(
        &manager,
        &request.items,
        true,
    )?))
}

async fn start_restore(
    State(state): State<AppState>,
    Json(request): Json<RestoreStartRequest>,
) -> ApiResult<Json<RestoreListResponse>> {
    validate_contract_uuid(&request.operation_uuid, "operationUuid")?;
    validate_contract_uuid(&request.preflight_token, "preflightToken")?;
    let manager = library_manager(&state)?;
    let admission = manager.path_admission().await;
    let request_value = restore_operation_request(&request.items);
    if let Some(plans) = manager
        .registry()
        .replay_restore_operation(
            &request.preflight_token,
            &request.operation_uuid,
            &request_value,
        )
        .map_err(map_library_error)?
    {
        return Ok(Json(RestoreListResponse { items: plans }));
    }
    let validation = validate_restore_items(&manager, &request.items, false)?;
    if !validation.ok {
        return Err(CoreError::conflict("stale-inventory".into()));
    }
    let mut plan_inputs = Vec::with_capacity(request.items.len());
    for item in &request.items {
        let (account_uuid, library_uuid, managed_root, restore_mode, inventory_fence, root_choices) =
            match item {
                RestoreStartItem::Legacy(item) => (
                    &item.account_uuid,
                    &item.library_uuid,
                    &item.placement_base,
                    "legacy-library",
                    None,
                    None,
                ),
                RestoreStartItem::RootAware(item) => (
                    &item.account_uuid,
                    &item.library_uuid,
                    &item.placement_base,
                    "root-aware",
                    Some(item.inventory_fence.as_str()),
                    Some(item.roots.as_slice()),
                ),
            };
        let mut roots = Vec::new();
        if let Some(choices) = root_choices {
            let inventory = manager
                .registry()
                .remote_library_root_inventory(account_uuid, library_uuid, None, usize::MAX)
                .map_err(map_library_error)?;
            for choice in choices {
                let fact = inventory
                    .items
                    .iter()
                    .find(|root| root.root_uuid == choice.root_uuid)
                    .ok_or_else(|| CoreError::conflict("stale-inventory".into()))?;
                let selected_binding = if choice.selected {
                    let target = choice.target_path.as_deref().ok_or_else(|| {
                        CoreError::bad_request("target-selection-mismatch".into())
                    })?;
                    let normalized = crate::sync_folders::validate_root_path(
                        FilePath::new(target),
                        crate::sync_folders::RootPathMode::MaterializeRemote,
                        FilePath::new(managed_root),
                        &[],
                    )
                    .map_err(sync_folder_error)?;
                    Some(crate::host_registry::NewLocalRootBinding {
                        library_uuid: library_uuid.clone(),
                        root_uuid: fact.root_uuid.clone(),
                        local_binding_uuid: uuid::Uuid::now_v7().to_string(),
                        local_path: normalized.normalized_path.clone(),
                        canonical_path_key: normalized.canonical_path_key.clone(),
                        placement_parent: normalized.placement_parent.clone(),
                        placement_parent_key: normalized.placement_parent_key.clone(),
                        marker_identity: String::new(),
                        filesystem_identity: normalized.filesystem_identity.clone(),
                        state: "materializing".into(),
                        binding_generation: 1,
                        root_capture_epoch: Some(uuid::Uuid::now_v7().to_string()),
                        root_actor_epoch: Some(uuid::Uuid::now_v7().to_string()),
                        desired_materialized_name: fact.materialized_name.clone(),
                        desired_materialization_fingerprint: fact
                            .materialization_fingerprint
                            .clone(),
                        applied_materialized_name: None,
                        applied_materialization_fingerprint: None,
                        operation_revision: 0,
                        verification_json: serde_json::json!({"inventoryFence":inventory_fence}),
                    })
                } else {
                    None
                };
                roots.push(crate::host_registry::RestoreRootPlanInput {
                    root_uuid: fact.root_uuid.clone(),
                    root_folder_uuid: fact.root_folder_uuid.clone(),
                    requested_name: fact.requested_name.clone(),
                    materialized_name: fact.materialized_name.clone(),
                    materialization_fingerprint: fact.materialization_fingerprint.clone(),
                    logical_revision: fact.logical_revision,
                    logical_bytes: fact.logical_bytes,
                    selected_binding,
                });
            }
        }
        plan_inputs.push(crate::host_registry::RestorePlanInput {
            account_uuid: account_uuid.clone(),
            library_uuid: library_uuid.clone(),
            managed_root: crate::library_path::inspect_directory_path(
                FilePath::new(managed_root),
                true,
            )
            .map_err(|reason| map_library_error(anyhow::anyhow!(reason.as_str())))?,
            restore_mode: restore_mode.to_owned(),
            inventory_fence: inventory_fence.map(ToOwned::to_owned),
            roots,
        });
    }
    let (inserted, plans) = manager
        .registry()
        .claim_restore_and_record_plans(
            &request.preflight_token,
            &request.operation_uuid,
            &request_value,
            &plan_inputs,
        )
        .map_err(map_library_error)?;
    if !inserted {
        return Ok(Json(RestoreListResponse { items: plans }));
    }
    drop(admission);
    let mut completed_plans: Vec<crate::host_registry::SyncRestorePlan> =
        Vec::with_capacity(plans.len());
    for (plan, input) in plans.into_iter().zip(plan_inputs.iter_mut()) {
        if let Err(error) = manager
            .create_planned_restore_library(
                &plan.restore_uuid,
                input.library_uuid.clone(),
                plan.name.clone(),
                input.managed_root.clone(),
                &input.account_uuid,
            )
            .await
        {
            let _ = manager
                .registry()
                .fail_restore_plan(&plan.restore_uuid, &error.to_string());
            for created in completed_plans.iter().rev() {
                let _ = manager.discard_restore_plan(&created.restore_uuid).await;
            }
            let _ = manager.discard_restore_plan(&plan.restore_uuid).await;
            let _ = manager.registry().fail_restore_operation_before_catalog(
                &request.operation_uuid,
                "multi-restore-create-failed",
            );
            let _ = manager.registry().finish_sync_operation(
                &request.operation_uuid,
                "failed",
                "compensated",
                Some("multi-restore-create-failed"),
            );
            let _ = manager
                .registry()
                .fail_restore_preflight(&request.preflight_token, &request.operation_uuid);
            return Err(map_library_error(error));
        }
        manager
            .registry()
            .advance_restore_phase(&input.library_uuid, "library-shell-created")?;
        if input.restore_mode == "root-aware" {
            if let Err(error) = manager.registry().import_restore_root_inventory(
                &plan.restore_uuid,
                &input.library_uuid,
                &input.roots,
            ) {
                for created in completed_plans.iter().rev() {
                    let _ = manager.discard_restore_plan(&created.restore_uuid).await;
                }
                let _ = manager.discard_restore_plan(&plan.restore_uuid).await;
                let _ = manager.registry().fail_restore_operation_before_catalog(
                    &request.operation_uuid,
                    "restore-inventory-import-failed",
                );
                let _ = manager.registry().finish_sync_operation(
                    &request.operation_uuid,
                    "failed",
                    "compensated",
                    Some("restore-inventory-import-failed"),
                );
                let _ = manager
                    .registry()
                    .fail_restore_preflight(&request.preflight_token, &request.operation_uuid);
                return Err(map_library_error(error));
            }
            let mut staged_markers: Vec<(std::path::PathBuf, crate::sync_folders::SyncRootMarker)> =
                Vec::new();
            for root in &mut input.roots {
                if let Some(binding) = root.selected_binding.as_mut() {
                    let marker = crate::sync_folders::SyncRootMarker {
                        format_version: 1,
                        library_uuid: input.library_uuid.clone(),
                        root_uuid: root.root_uuid.clone(),
                        root_folder_uuid: root.root_folder_uuid.clone(),
                        local_binding_uuid: binding.local_binding_uuid.clone(),
                        binding_generation: binding.binding_generation,
                    };
                    if let Err(error) = crate::sync_folders::write_root_marker(
                        FilePath::new(&binding.local_path),
                        &marker,
                    ) {
                        for (path, owned_marker) in staged_markers.iter().rev() {
                            let _ = crate::sync_folders::remove_root_marker_if_matches(
                                path,
                                owned_marker,
                                false,
                            );
                        }
                        for created in completed_plans.iter().rev() {
                            let _ = manager.discard_restore_plan(&created.restore_uuid).await;
                        }
                        let _ = manager.discard_restore_plan(&plan.restore_uuid).await;
                        let _ = manager.registry().fail_restore_operation_before_catalog(
                            &request.operation_uuid,
                            "restore-marker-write-failed",
                        );
                        let _ = manager.registry().finish_sync_operation(
                            &request.operation_uuid,
                            "failed",
                            "compensated",
                            Some("restore-marker-write-failed"),
                        );
                        let _ = manager.registry().fail_restore_preflight(
                            &request.preflight_token,
                            &request.operation_uuid,
                        );
                        return Err(sync_folder_error(error));
                    }
                    staged_markers.push((FilePath::new(&binding.local_path).to_path_buf(), marker));
                    let marker_identity = match crate::library_path::local_file_identity(
                        &FilePath::new(&binding.local_path)
                            .join(crate::sync_folders::ROOT_MARKER_FILE_NAME),
                    ) {
                        Ok(identity) => identity,
                        Err(error) => {
                            for (path, owned_marker) in staged_markers.iter().rev() {
                                let _ = crate::sync_folders::remove_root_marker_if_matches(
                                    path,
                                    owned_marker,
                                    false,
                                );
                            }
                            for created in completed_plans.iter().rev() {
                                let _ = manager.discard_restore_plan(&created.restore_uuid).await;
                            }
                            let _ = manager.discard_restore_plan(&plan.restore_uuid).await;
                            let _ = manager.registry().fail_restore_operation_before_catalog(
                                &request.operation_uuid,
                                "restore-marker-identity-failed",
                            );
                            let _ = manager.registry().finish_sync_operation(
                                &request.operation_uuid,
                                "failed",
                                "compensated",
                                Some("restore-marker-identity-failed"),
                            );
                            let _ = manager.registry().fail_restore_preflight(
                                &request.preflight_token,
                                &request.operation_uuid,
                            );
                            return Err(map_library_error(error.into()));
                        }
                    };
                    binding.marker_identity = marker_identity;
                    binding.verification_json["restoreUuid"] =
                        serde_json::Value::String(plan.restore_uuid.clone());
                }
            }
            manager
                .registry()
                .advance_restore_phase(&input.library_uuid, "markers-staged")?;
            if let Err(error) = manager.registry().stage_restore_root_bindings(
                &plan.restore_uuid,
                &input.library_uuid,
                &input.roots,
            ) {
                for (path, owned_marker) in staged_markers.iter().rev() {
                    let _ = crate::sync_folders::remove_root_marker_if_matches(
                        path,
                        owned_marker,
                        false,
                    );
                }
                for created in completed_plans.iter().rev() {
                    let _ = manager.discard_restore_plan(&created.restore_uuid).await;
                }
                let _ = manager.discard_restore_plan(&plan.restore_uuid).await;
                let _ = manager.registry().fail_restore_operation_before_catalog(
                    &request.operation_uuid,
                    "restore-root-staging-failed",
                );
                let _ = manager.registry().finish_sync_operation(
                    &request.operation_uuid,
                    "failed",
                    "compensated",
                    Some("restore-root-staging-failed"),
                );
                let _ = manager
                    .registry()
                    .fail_restore_preflight(&request.preflight_token, &request.operation_uuid);
                return Err(map_library_error(error));
            }
            manager
                .registry()
                .advance_restore_phase(&input.library_uuid, "host-bindings-staged")?;
        }
        let _ = manager.publish_restore_notification(&plan).await;
        completed_plans.push(plan);
    }
    manager.registry().finish_sync_operation(
        &request.operation_uuid,
        "waiting",
        "workers-published",
        None,
    )?;
    manager
        .registry()
        .complete_restore_preflight(&request.preflight_token, &request.operation_uuid)?;
    manager.wake_sync_folder_workers();
    for plan in &completed_plans {
        manager
            .registry()
            .advance_restore_phase(&plan.library_uuid, "workers-published")?;
    }
    Ok(Json(RestoreListResponse {
        items: completed_plans,
    }))
}
async fn get_restore(
    State(state): State<AppState>,
    Path(restore_uuid): Path<String>,
) -> ApiResult<Json<crate::host_registry::SyncRestorePlan>> {
    Ok(Json(
        library_manager(&state)?
            .registry()
            .restore_plan(&restore_uuid)?
            .ok_or_else(|| CoreError::not_found("restore-not-found".into()))?,
    ))
}
async fn pause_restore(
    State(state): State<AppState>,
    Path(restore_uuid): Path<String>,
) -> ApiResult<Json<crate::host_registry::SyncRestorePlan>> {
    let manager = library_manager(&state)?;
    let plan = manager.pause_restore(&restore_uuid).await?;
    let _ = manager.publish_restore_notification(&plan).await;
    Ok(Json(plan))
}
async fn resume_restore(
    State(state): State<AppState>,
    Path(restore_uuid): Path<String>,
) -> ApiResult<Json<crate::host_registry::SyncRestorePlan>> {
    let manager = library_manager(&state)?;
    let plan = manager.resume_restore(&restore_uuid).await?;
    let _ = manager.publish_restore_notification(&plan).await;
    Ok(Json(plan))
}
async fn cancel_restore(
    State(state): State<AppState>,
    Path(restore_uuid): Path<String>,
) -> ApiResult<Json<crate::host_registry::SyncRestorePlan>> {
    let manager = library_manager(&state)?;
    let plan = manager.cancel_restore(&restore_uuid).await?;
    // The local Catalog no longer exists after pre-catalog cancellation, so
    // publish the terminal notification before cleanup is impossible. The
    // durable Host operation remains the recovery/audit record.
    Ok(Json(plan))
}

async fn list_sync_conflicts(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
    Query(query): Query<ConflictQuery>,
) -> ApiResult<Json<SyncConflictListResponse>> {
    let descriptor = library_manager(&state)?
        .registry()
        .library(&library_uuid)?
        .ok_or_else(|| CoreError::not_found("library-not-found".into()))?;
    let database = Database::open(descriptor.local_data_dir.join("catalog.sqlite"))?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let after = query
        .cursor
        .as_deref()
        .and_then(|v| uuid::Uuid::parse_str(v).ok());
    let mut statement=database.connection_for_fsops().prepare("SELECT conflict_uuid,entity_uuid,conflict_kind,state,details,created_at,resolved_at FROM sync_conflicts WHERE (?1 IS NULL OR conflict_uuid>?1) ORDER BY conflict_uuid LIMIT ?2").map_err(anyhow::Error::new)?;
    let items = statement
        .query_map(
            params![after.map(|v| v.as_bytes().to_vec()), (limit + 1) as i64],
            |row| {
                let id: Vec<u8> = row.get(0)?;
                Ok(SyncConflictRecord {
                    conflict_uuid: uuid::Uuid::from_slice(&id)
                        .map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Blob,
                                Box::new(e),
                            )
                        })?
                        .to_string(),
                    entity_uuid: row
                        .get::<_, Option<Vec<u8>>>(1)?
                        .and_then(|value| uuid::Uuid::from_slice(&value).ok())
                        .map(|value| value.to_string()),
                    conflict_kind: row.get(2)?,
                    state: row.get(3)?,
                    details: row
                        .get::<_, Option<Vec<u8>>>(4)?
                        .and_then(|value| serde_json::from_slice::<serde_json::Value>(&value).ok())
                        .map(redact_sync_conflict_details),
                    created_at: row.get(5)?,
                    resolved_at: row.get(6)?,
                })
            },
        )
        .map_err(anyhow::Error::new)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(anyhow::Error::new)?;
    let next_cursor = (items.len() > limit).then(|| items[limit - 1].conflict_uuid.clone());
    Ok(Json(SyncConflictListResponse {
        items: items.into_iter().take(limit).collect(),
        next_cursor,
    }))
}

fn redact_sync_conflict_details(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .into_iter()
                .filter_map(|(key, value)| {
                    let normalized = key.to_ascii_lowercase();
                    if normalized.contains("path")
                        || normalized.contains("name")
                        || normalized.contains("token")
                        || normalized.contains("secret")
                    {
                        None
                    } else {
                        Some((key, redact_sync_conflict_details(value)))
                    }
                })
                .collect(),
        ),
        serde_json::Value::Array(values) => serde_json::Value::Array(
            values
                .into_iter()
                .map(redact_sync_conflict_details)
                .collect(),
        ),
        other => other,
    }
}

async fn resolve_sync_conflict(
    State(state): State<AppState>,
    Path((library_uuid, conflict_uuid)): Path<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<ConflictResolutionRequest>,
) -> ApiResult<Json<crate::host_registry::SyncOperation>> {
    let expected = require_context_revision(&headers)?;
    let manager = library_manager(&state)?;
    manager
        .ensure_context_revision(expected)
        .map_err(map_library_error)?;
    if !matches!(request.resolution.as_str(), "resolve" | "ignore") {
        return Err(CoreError::bad_request("invalid conflict resolution".into()));
    }
    let (operation, inserted) = manager.registry().begin_sync_operation_once(
        &request.operation_uuid,
        Some(&library_uuid),
        "retry",
        "complete",
        &serde_json::json!({"conflictUuid":conflict_uuid,"resolution":request.resolution}),
    )?;
    if !inserted {
        return Ok(Json(operation));
    }
    let _ = manager
        .publish_sync_operation_notification(&operation)
        .await;
    let id = uuid::Uuid::parse_str(&conflict_uuid)
        .map_err(|_| CoreError::bad_request("invalid conflict uuid".into()))?;
    if let Err(error) = manager
        .resolve_sync_conflict_with_revision(&library_uuid, expected, id, &request.resolution)
        .await
    {
        let code = error.to_string().replace('-', "_");
        if let Ok(finished) = manager.registry().finish_sync_operation(
            &operation.operation_uuid,
            "failed",
            "retry-wait",
            Some(&code),
        ) {
            let _ = manager.publish_sync_operation_notification(&finished).await;
        }
        return Err(map_library_error(error));
    }
    let finished = manager.registry().finish_sync_operation(
        &operation.operation_uuid,
        "complete",
        "complete",
        None,
    )?;
    let _ = manager.publish_sync_operation_notification(&finished).await;
    Ok(Json(finished))
}

async fn sync_diagnostics(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(
        library_manager(&state)?
            .registry()
            .redacted_sync_diagnostics(&library_uuid)?,
    ))
}
async fn export_sync_diagnostics(
    State(state): State<AppState>,
    Path(library_uuid): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    sync_diagnostics(State(state), Path(library_uuid)).await
}

fn map_library_error(error: anyhow::Error) -> CoreError {
    let message = error.to_string();
    let status = if message.contains("not-found") {
        StatusCode::NOT_FOUND
    } else if message.contains("invalid") || message.contains("not-empty") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::CONFLICT
    };
    let code = message.replace('-', "_");
    CoreError::coded(status, &code, message)
}

fn read_with_cached_connection<T>(
    state: &AppState,
    resources: &crate::library_manager::LibraryRuntimeResources,
    operation: impl FnOnce(&Database) -> ApiResult<T>,
) -> ApiResult<T> {
    // Callers must only reach this with a file-backed read database. The
    // in-memory/test fallback is handled in `run_read_database` (it has no
    // separate read connection, so it routes through the single writer).
    // Blocking threads outlive Library activation. Cache connections on the
    // captured runtime instead, with one exclusive lease per concurrent read.
    // The cache lock is never held while opening SQLite or running the query.
    let mut lease = resources.read_database_cache.checkout()?;
    if lease.database.is_none() {
        lease.database = state.open_runtime_read_database(resources)?;
    }
    match lease.database.as_ref() {
        Some(database) => operation(database),
        None => Err(CoreError::from(anyhow::anyhow!(
            "read database connection unavailable"
        ))),
    }
}

pub(super) async fn run_read_database<T, F>(state: AppState, operation: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Database) -> ApiResult<T> + Send + 'static,
{
    // Capture while the request still owns admission. A dropped HTTP waiter
    // releases its gate even when its blocking job has not started yet.
    let resources = state.resources()?;
    if resources.read_database_path.is_none() {
        // In-memory / test database: no separate read connection exists, so run
        // the read on the single writer connection (the only connection there is).
        let inner = resources
            .writer
            .write(move |database: &mut Database| Ok(operation(database)))
            .await;
        return match inner {
            Ok(api_result) => api_result,
            Err(error) => Err(CoreError::from(error)),
        };
    }
    // Checkout stays on the blocking thread: queued jobs must not hold leases
    // that retirement could wait for ahead of those jobs in the same pool.
    tokio::task::spawn_blocking(move || read_with_cached_connection(&state, &resources, operation))
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
        .writer()
        .expect("active library writer")
        .write(move |database: &mut Database| Ok(operation(database)))
        .await;
    match inner {
        Ok(api_result) => api_result,
        Err(error) => Err(CoreError::from(error)),
    }
}

/// Run an unrecoverable user-authored mutation under SQLite `FULL` durability.
/// Sync capture, when enabled for the Library, executes inside the same closure
/// and therefore the same transaction as the browse projection mutation.
async fn run_authoritative_database<T, F>(state: AppState, operation: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Database) -> ApiResult<T> + Send + 'static,
{
    let inner = state
        .writer()
        .expect("active library writer")
        .write_authoritative(move |database: &mut Database| Ok(operation(database)))
        .await;
    match inner {
        Ok(api_result) => {
            if api_result.is_ok() {
                if let Some(manager) = state.library_manager.as_ref() {
                    manager.wake_sync_after_local_mutation();
                }
            }
            api_result
        }
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
        service: "mangoo-core",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn list_roots(State(state): State<AppState>) -> ApiResult<Json<ListResponse<RootRecord>>> {
    let reach = state
        .root_reachability()
        .expect("active library reachability");
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
    let canonical_path = if let Some(manager) = state.library_manager.as_ref() {
        manager
            .admit_content_path(FilePath::new(&payload.path), false)
            .map_err(map_library_error)?
    } else {
        crate::library_path::inspect_directory_path(FilePath::new(&payload.path), false).map_err(
            |reason| {
                CoreError::bad_request(format!(
                    "root path does not exist or cannot be opened: {}",
                    reason.as_str()
                ))
            },
        )?
    };
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateFolderAppearanceRequest {
    icon: String,
    color: String,
}

fn validate_directory_appearance(input: &UpdateFolderAppearanceRequest) -> ApiResult<()> {
    let valid_icon = !input.icon.is_empty()
        && input.icon.len() <= 64
        && input
            .icon
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
    let valid_color = matches!(input.color.len(), 4 | 7)
        && input.color.starts_with('#')
        && input.color.as_bytes()[1..]
            .iter()
            .all(u8::is_ascii_hexdigit);
    if !valid_icon || !valid_color {
        return Err(CoreError::bad_request(
            "invalid folder appearance".to_string(),
        ));
    }
    Ok(())
}

async fn update_root_appearance(
    State(state): State<AppState>,
    Path(root_id): Path<i64>,
    body: Result<Json<UpdateFolderAppearanceRequest>, JsonRejection>,
) -> ApiResult<Json<RootRecord>> {
    let Json(input) = body.map_err(|_| CoreError::bad_request("invalid root appearance".into()))?;
    validate_directory_appearance(&input)?;
    let mut root = state
        .writer()
        .expect("active library writer")
        .write_authoritative(move |database| {
            database.update_root_appearance(root_id, &input.icon, &input.color)
        })
        .await?
        .ok_or_else(|| CoreError::not_found(format!("root not found: {root_id}")))?;
    let reach = state
        .root_reachability()
        .expect("active library reachability");
    if let Ok(map) = reach.lock() {
        root.reachable = *map.get(&root.id).unwrap_or(&true);
    }
    Ok(Json(root))
}

async fn read_directory_sort(
    state: AppState,
    target: DirectoryTarget,
) -> ApiResult<Json<DirectorySortState>> {
    let sort = run_read_database(state, move |database| {
        database
            .read_directory_sort_state(target)?
            .ok_or_else(|| CoreError::not_found("directory not found".into()))
    })
    .await?;
    Ok(Json(sort))
}

async fn write_directory_sort(
    state: AppState,
    target: DirectoryTarget,
    rule: Option<DirectorySortRule>,
) -> ApiResult<Json<DirectorySortMutationResult>> {
    let result = state
        .writer()
        .expect("active library writer")
        .write_authoritative(move |database| database.update_directory_sort(target, rule))
        .await?
        .ok_or_else(|| CoreError::not_found("directory not found".into()))?;
    Ok(Json(result))
}

async fn get_root_sort(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<DirectorySortState>> {
    read_directory_sort(state, DirectoryTarget::Root(id)).await
}

async fn update_root_sort(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Result<Json<DirectorySortRule>, JsonRejection>,
) -> ApiResult<Json<DirectorySortMutationResult>> {
    let Json(rule) =
        body.map_err(|_| CoreError::bad_request("invalid directory sort rule".into()))?;
    write_directory_sort(state, DirectoryTarget::Root(id), Some(rule)).await
}

async fn reset_root_sort(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<DirectorySortMutationResult>> {
    write_directory_sort(state, DirectoryTarget::Root(id), None).await
}

async fn get_folder_sort(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<DirectorySortState>> {
    read_directory_sort(state, DirectoryTarget::Folder(id)).await
}

async fn update_folder_sort(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Result<Json<DirectorySortRule>, JsonRejection>,
) -> ApiResult<Json<DirectorySortMutationResult>> {
    let Json(rule) =
        body.map_err(|_| CoreError::bad_request("invalid directory sort rule".into()))?;
    write_directory_sort(state, DirectoryTarget::Folder(id), Some(rule)).await
}

async fn reset_folder_sort(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<DirectorySortMutationResult>> {
    write_directory_sort(state, DirectoryTarget::Folder(id), None).await
}

async fn update_folder_appearance(
    State(state): State<AppState>,
    Path(folder_id): Path<i64>,
    body: Result<Json<UpdateFolderAppearanceRequest>, JsonRejection>,
) -> ApiResult<Json<FolderRecord>> {
    let Json(input) =
        body.map_err(|_| CoreError::bad_request("invalid folder appearance".to_string()))?;
    validate_directory_appearance(&input)?;
    // Local appearance is durable user metadata, without a sync outbox mutation.
    let folder = state
        .writer()
        .expect("active library writer")
        .write_authoritative(move |database| {
            database.update_folder_appearance(folder_id, &input.icon, &input.color)
        })
        .await?
        .ok_or_else(|| CoreError::not_found(format!("ordinary folder not found: {folder_id}")))?;
    Ok(Json(folder))
}

async fn list_folder_children(
    State(state): State<AppState>,
    Path(folder_id): Path<i64>,
    Query(query): Query<ListFolderChildrenQuery>,
) -> ApiResult<Json<FolderChildrenPage>> {
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
    Ok(Json(page))
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
        kind: parse_media_kind(query.kind.as_deref())?.map(str::to_string),
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
    let resources = state.resources().expect("active library resources");
    let (thumbnail, queued_task) = resources
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
        if let Some(source) = database.get_thumbnail_source(file_id)? {
            if !matches!(source.media_kind.as_deref(), Some("image" | "video"))
                && !database
                    .get_thumbnail(file_id, target)?
                    .is_some_and(|thumbnail| thumbnail.state == "ready")
            {
                return Err(CoreError::not_found("thumbnail blob is stale".to_string()));
            }
        }
        let blob = database
            .get_thumb_blob(file_id, target)?
            .ok_or_else(|| CoreError::not_found("thumbnail blob missing".to_string()))?;
        Ok(blob)
    })
    .await?;
    Response::builder()
        .header(CONTENT_TYPE, blob.output_format)
        .header(CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header("x-mangoo-served-by", "db_blob")
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
                .root_reachability()
                .expect("active library reachability")
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

    // Generic files may expose only a verified static result. Never stream
    // arbitrary original bytes into the image/video preview surface.
    let static_thumbnail = run_read_database(state.clone(), move |database| {
        let media = database
            .get_media(file_id)?
            .ok_or_else(|| CoreError::not_found(format!("file not found: {file_id}")))?;
        Ok(media.presentation.preview_kind == "static-thumbnail")
    })
    .await?;
    if static_thumbnail {
        if let Some(response) = serve_decoder_preview(state.clone(), file_id).await? {
            return Ok(response);
        }
        return get_thumbnail_blob(
            State(state),
            Path(file_id),
            Query(ThumbnailQuery {
                target: Some(crate::thumbnails::GRID_320_PROFILE.to_string()),
                priority: None,
            }),
        )
        .await;
    }

    let (source_path, source_root_token, media_kind, source_max_px) =
        run_read_database(state.clone(), move |database| {
            let media = database
                .get_media(file_id)?
                .ok_or_else(|| CoreError::not_found(format!("media item not found: {file_id}")))?;
            let (source_path, source_root_token) = database
                .resolve_file_source_path_with_root_token(file_id)?
                .ok_or_else(|| {
                    CoreError::not_found(format!("media source not found: {file_id}"))
                })?;
            // `width`/`height` stay NULL until the thumbnail pass fills them in;
            // 0 then means "unknown", which resolves to "stream the original".
            let source_max_px = media
                .width
                .unwrap_or(0)
                .max(media.height.unwrap_or(0))
                .clamp(0, u32::MAX as i64) as u32;
            Ok((source_path, source_root_token, media.kind, source_max_px))
        })
        .await?;

    let token_for_open = source_root_token;
    run_read_database(state.clone(), move |database| {
        if let Some(token) = token_for_open.as_ref() {
            database.require_sync_root_binding_token(token)?;
        }
        Ok(())
    })
    .await?;
    if let Some(token) = source_root_token.as_ref() {
        state
            .require_current_root_token(token)
            .map_err(CoreError::from)?;
    }
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
    if let Some(token) = source_root_token.as_ref() {
        state
            .require_current_root_token(token)
            .map_err(CoreError::from)?;
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
        if let Some(response) = serve_cached_preview_webp(
            state,
            file_id,
            plan,
            &source_path,
            &metadata,
            token_for_open,
        )
        .await?
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
const PREVIEW_TIER_HEADER: &str = "x-mangoo-preview-tier";

/// Upper bound on a reported preview viewport. Far beyond any real display,
/// and values above it are clamped rather than rejected.
const MAX_VIEWPORT_LONG_PX: u32 = 16384;

async fn serve_decoder_preview(state: AppState, file_id: i64) -> ApiResult<Option<Response>> {
    use crate::plugins::decoder::DecoderPurpose;
    use crate::thumbnails::{GENERATED_FORMAT, PREVIEW_3072_PROFILE};
    let (source, path, token, context, cached, failure) =
        run_read_database(state.clone(), move |db| {
            let source = db
                .get_thumbnail_source(file_id)?
                .ok_or_else(|| CoreError::not_found("file missing".into()))?;
            let (path, token) = db
                .resolve_file_source_path_with_root_token(file_id)?
                .ok_or_else(|| CoreError::not_found("source missing".into()))?;
            let context = db.decoder_context();
            let extension = crate::decode::formats::lowercase_extension(&path);
            let snapshot = context
                .as_ref()
                .map(|(manager, _)| manager.snapshot())
                .unwrap_or_default();
            let valid = db.decoder_cache_matches_candidates(
                file_id,
                PREVIEW_3072_PROFILE,
                &source.source_fingerprint(PREVIEW_3072_PROFILE),
                &snapshot,
                &extension,
                DecoderPurpose::Preview,
                false,
            )?;
            let cached = if valid {
                db.get_preview_blob(file_id, PREVIEW_3072_PROFILE)?
            } else {
                None
            };
            let failure = db.decoder_cache_matches_candidates(
                file_id,
                PREVIEW_3072_PROFILE,
                &source.source_fingerprint(PREVIEW_3072_PROFILE),
                &snapshot,
                &extension,
                DecoderPurpose::Preview,
                true,
            )? && db
                .get_preview_failure(file_id, PREVIEW_3072_PROFILE)?
                .is_some();
            Ok((source, path, token, context, cached, failure))
        })
        .await?;
    let Some((manager, library_cancel)) = context else {
        return Ok(None);
    };
    let cancellation = library_cancel.child_token();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    if cancellation.is_cancelled() {
        return Ok(None);
    }
    let record = if let Some(cached) = cached {
        cached
    } else {
        if failure {
            return Ok(None);
        }
        let extension = crate::decode::formats::lowercase_extension(&path);
        if let Some(token) = token.as_ref() {
            state
                .require_current_root_token(token)
                .map_err(CoreError::from)?;
        }
        let fingerprint = source.source_fingerprint(PREVIEW_3072_PROFILE);
        let open_path = path.clone();
        let (file, directory_guard, stamp) =
            tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                let (file, guard) = crate::sync::file_policy::open_source(&open_path)?;
                let stamp = crate::sync::identity::source_stamp(&open_path)?;
                Ok((file, guard, stamp))
            })
            .await
            .map_err(|error| CoreError::internal(error.to_string()))?
            .map_err(|_| CoreError::not_found("decoder source unavailable".into()))?;
        let source_pin = file
            .try_clone()
            .map_err(|error| CoreError::from(anyhow::Error::from(error)))?;
        let result = manager
            .decode_owned(
                file,
                &extension,
                DecoderPurpose::Preview,
                3072,
                cancellation.clone(),
            )
            .await;
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        if let Some(token) = token.as_ref() {
            state
                .require_current_root_token(token)
                .map_err(CoreError::from)?;
        }
        let raster = match result {
            Ok(raster) => raster,
            Err(failed) => {
                let error = failed.error;
                if matches!(
                    error,
                    crate::plugins::decoder::DecoderError::Busy
                        | crate::plugins::decoder::DecoderError::Cancelled
                ) {
                    return Ok(None);
                }
                let Some(key) = failed.cache_key else {
                    return Ok(None);
                };
                let Some(failure_guard) = failed.publication_guard else {
                    return Ok(None);
                };
                let now = crate::db::unix_timestamp();
                let failure = PreviewFailureRecord {
                    file_id,
                    profile: PREVIEW_3072_PROFILE.into(),
                    failure_kind: "permanent".into(),
                    error: error.to_string(),
                    source_size: source.size,
                    source_mtime: source.mtime,
                    created_at: now,
                    updated_at: now,
                };
                let _ = run_shared_database(state.clone(), move |db| {
                    failure_guard
                        .publish(|| -> ApiResult<()> {
                            if cancellation.is_cancelled() {
                                return Ok(());
                            }
                            db.publish_decoder_preview_failure(
                                failure,
                                token.as_ref(),
                                Some((&key, &fingerprint)),
                            )?;
                            Ok(())
                        })
                        .map_err(|error| CoreError::from(anyhow::Error::from(error)))?
                })
                .await;
                return Ok(None);
            }
        };
        let generated = tokio::task::spawn_blocking(move || raster.into_webp())
            .await
            .map_err(|error| CoreError::internal(error.to_string()))?
            .map_err(|error| CoreError::from(anyhow::Error::from(error)))?;
        let current_stamp =
            tokio::task::spawn_blocking(move || crate::sync::identity::source_stamp(&path))
                .await
                .map_err(|error| CoreError::internal(error.to_string()))?
                .map_err(|_| CoreError::not_found("decoder source changed".into()))?;
        if current_stamp != stamp || cancellation.is_cancelled() {
            return Ok(None);
        }
        let now = crate::db::unix_timestamp();
        let record = PreviewBlobRecord {
            file_id,
            profile: PREVIEW_3072_PROFILE.into(),
            byte_size: generated.data.len() as i64,
            data: generated.data,
            width: generated.width,
            height: generated.height,
            output_format: GENERATED_FORMAT.into(),
            source_size: source.size,
            source_mtime: source.mtime,
            created_at: now,
            updated_at: now,
        };
        let stored = record.clone();
        let key = generated.identity.cache_key();
        run_shared_database(state.clone(), move |db| {
            let (_source_pin, _directory_guard) = (source_pin, directory_guard);
            generated
                .publication_guard
                .publish(|| -> ApiResult<()> {
                    if cancellation.is_cancelled() {
                        return Err(CoreError::not_found("decoder library stopped".into()));
                    }
                    db.publish_decoder_preview_blob(
                        stored,
                        token.as_ref(),
                        Some((&key, &fingerprint)),
                    )?;
                    Ok(())
                })
                .map_err(|error| CoreError::from(anyhow::Error::from(error)))?
        })
        .await?;
        record
    };
    Response::builder()
        .header("content-type", record.output_format)
        .header("cache-control", "private, max-age=0, must-revalidate")
        .header(PREVIEW_TIER_HEADER, PREVIEW_3072_PROFILE)
        .body(axum::body::Body::from(record.data))
        .map(Some)
        .map_err(|error| CoreError::internal(error.to_string()))
}

fn preview_failure_is_fresh(
    failure: &PreviewFailureRecord,
    source_size: i64,
    source_mtime: i64,
    now: i64,
) -> bool {
    failure.failure_kind == "permanent"
        && failure.source_size == source_size
        && failure.source_mtime == source_mtime
        && now.saturating_sub(failure.updated_at) <= PREVIEW_FAILURE_CACHE_TTL_SECONDS
}

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
    source_root_token: Option<crate::db::SyncRootBindingToken>,
) -> ApiResult<Option<Response>> {
    use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};

    let source_size = metadata.len() as i64;
    if let Some(token) = source_root_token.as_ref() {
        state
            .require_current_root_token(token)
            .map_err(CoreError::from)?;
    }
    let source_mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);

    let (cached, cached_failure) = run_read_database(state.clone(), move |database| {
        Ok((
            database.get_preview_blob(file_id, plan.profile)?,
            database.get_preview_failure(file_id, plan.profile)?,
        ))
    })
    .await?;
    let cached =
        cached.filter(|blob| blob.source_size == source_size && blob.source_mtime == source_mtime);
    let now = crate::db::unix_timestamp();
    // A permanent classification suppresses hot-loop retries, while the TTL
    // lets decoder installation and environmental recovery take effect without
    // requiring a source-file mutation or a manual cache clear.
    let cached_failure = cached_failure
        .filter(|failure| preview_failure_is_fresh(failure, source_size, source_mtime, now));

    // A successful representation wins defensively if a damaged or manually
    // edited database contains both rows. Normal writes keep the tables
    // mutually exclusive, but serving known-good bytes is the safest recovery.
    if cached.is_none() {
        if let Some(failure) = cached_failure {
            crate::debug_log!(
                "preview tier negative-cache hit (file {file_id}, profile {}): {}",
                plan.profile,
                failure.error
            );
            return Ok(None);
        }
    }

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
                .preview_decode_gate()
                .expect("active library preview gate")
                .acquire_owned()
                .await
                .map_err(|_| CoreError::internal("preview decode gate closed".to_string()))?;
            let outcome = tokio::task::spawn_blocking(move || {
                generate_preview_webp(&generation_path, plan.max_px)
            })
            .await
            // A panic is a bug in the decode path, not a runtime degradation.
            .map_err(|err| CoreError::internal(format!("preview generation panicked: {err}")))?;
            if let Some(token) = source_root_token.as_ref() {
                state
                    .require_current_root_token(token)
                    .map_err(CoreError::from)?;
            }
            let generated = match outcome {
                Ok(generated) => generated,
                Err(error) => {
                    // Degrade instead of failing. Every reason this fires — no
                    // decoder for the format, ffmpeg not on PATH, an unreachable
                    // NAS, a locked file, OOM — leaves the ORIGINAL bytes
                    // perfectly servable, and those are what this route returned
                    // before the preview pipeline existed. A 500 here would blank
                    // out a picture that used to display fine.
                    let error_text = format!("{error:#}");
                    crate::debug_log!(
                        "preview tier generation failed (file {file_id}, profile {}): {error:#}",
                        plan.profile
                    );
                    if matches!(
                        classify_thumbnail_failure(&error_text),
                        ThumbnailFailureKind::Permanent
                    ) {
                        let now = crate::db::unix_timestamp();
                        let failure = PreviewFailureRecord {
                            file_id,
                            profile: plan.profile.to_string(),
                            failure_kind: "permanent".to_string(),
                            error: error_text,
                            source_size,
                            source_mtime,
                            created_at: now,
                            updated_at: now,
                        };
                        if let Err(cache_error) =
                            run_shared_database(state.clone(), move |database| {
                                database.upsert_preview_failure_for_root_token(
                                    failure,
                                    source_root_token.as_ref(),
                                )?;
                                Ok(())
                            })
                            .await
                        {
                            crate::debug_log!(
                                "preview failure cache write failed (file {file_id}, profile {}): {}",
                                plan.profile,
                                cache_error.message
                            );
                        }
                    }
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
                database.upsert_preview_blob_for_root_token(stored, source_root_token.as_ref())?;
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
    let writer = state.writer().expect("active library writer");
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
    let writer = state.writer().expect("active library writer");
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
    let writer = state.writer().expect("active library writer");
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
    let database = fsops_database(&state)?;
    let page = fsops::list_recent(database.as_ref(), limit, query.cursor.as_deref())
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
    let notification_outbox_wake = state
        .notification_outbox_wake()
        .expect("active library outbox wake");
    let notified = notification_outbox_wake.notified();
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
    let read_permit = state
        .notification_outbox_read_gate()
        .expect("active library outbox gate")
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
    let items = host_registry(&state)?.list_host_plugins()?;
    Ok(Json(PluginListResponse { items }))
}

async fn discover_plugins(
    State(state): State<AppState>,
) -> ApiResult<(StatusCode, Json<PluginDiscoveryResponse>)> {
    let plugins_dir = resolve_plugins_dir(&state);
    let registry = Arc::clone(host_registry(&state)?);
    let (registered, errors) =
        tokio::task::spawn_blocking(move || plugins::reconcile_bundled(&registry, &plugins_dir))
            .await
            .map_err(|e| CoreError::from(anyhow::anyhow!("plugin discovery task failed: {e}")))?;
    let errors = errors
        .into_iter()
        .map(|entry| PluginDiscoveryError {
            manifest_path: entry.manifest_path.to_string_lossy().into_owned(),
            message: entry.message,
        })
        .collect();
    Ok((
        StatusCode::ACCEPTED,
        Json(PluginDiscoveryResponse {
            discovered: registered as u64,
            errors,
        }),
    ))
}

async fn get_plugin_view_document(
    State(state): State<AppState>,
    Path((plugin_id, view_id)): Path<(String, String)>,
) -> ApiResult<Json<plugins::ui::HostPluginViewDocument>> {
    let bundled_root = resolve_plugins_dir(&state);
    let registry = Arc::clone(host_registry(&state)?);
    let document = tokio::task::spawn_blocking(move || {
        registry
            .host_plugin(&plugin_id)?
            .ok_or_else(|| not_found_plugin(&plugin_id))?;
        if !plugins::runtime::process::is_bundled_root(&bundled_root) {
            return Err(CoreError::coded(
                StatusCode::CONFLICT,
                "plugin-ui-untrusted",
                "Plugin UI is unavailable".into(),
            ));
        }
        let platform = if cfg!(windows) {
            plugins::manifest::Platform::WindowsX64
        } else {
            plugins::manifest::Platform::LinuxX64
        };
        plugins::ui::load_view_document(
            &registry,
            &bundled_root,
            &plugin_id,
            &view_id,
            platform,
            plugins::runtime::trust::embedded_trust_registry(),
        )
        .map_err(|error| {
            let code = error.to_string();
            match code.as_str() {
                "plugin-view-not-found" => CoreError::coded(
                    StatusCode::NOT_FOUND,
                    &code,
                    "Plugin view was not found".into(),
                ),
                "plugin-ui-unavailable" | "plugin-ui-untrusted" | "plugin-ui-changed" => {
                    CoreError::coded(
                        StatusCode::CONFLICT,
                        &code,
                        "Plugin UI is unavailable".into(),
                    )
                }
                _ => CoreError::from(error),
            }
        })
    })
    .await
    .map_err(|_| CoreError::from(anyhow::anyhow!("plugin UI read task failed")))??;
    Ok(Json(document))
}

async fn get_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<Json<crate::host_registry::HostPluginProjection>> {
    let plugin = host_registry(&state)?
        .host_plugin(&plugin_id)?
        .ok_or_else(|| not_found_plugin(&plugin_id))?;
    Ok(Json(plugin))
}

async fn enable_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<Json<crate::host_registry::HostPluginProjection>> {
    let manager = library_manager(&state)?;
    let _lifecycle = manager.lock_plugin_lifecycle(&plugin_id).await;
    let registry = manager.host_registry();
    let current = registry
        .host_plugin(&plugin_id)?
        .ok_or_else(|| not_found_plugin(&plugin_id))?;
    if current.runtime_state == "stopping" {
        return Err(CoreError::coded(
            StatusCode::CONFLICT,
            "plugin_stopping",
            "Provider stop has not completed".into(),
        ));
    }
    if current.enabled_intent {
        return Ok(Json(current));
    }
    registry
        .set_plugin_intent(&plugin_id, true, state.secret_store.is_some())
        .map_err(|_| not_found_plugin(&plugin_id))?;
    let updated = registry
        .host_plugin(&plugin_id)?
        .ok_or_else(|| not_found_plugin(&plugin_id))?;
    Ok(Json(updated))
}

async fn disable_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<Json<crate::host_registry::HostPluginProjection>> {
    let manager = library_manager(&state)?;
    let _lifecycle = manager.lock_plugin_lifecycle(&plugin_id).await;
    let _begin_guard = state.plugin_auth_begin_gate.lock().await;
    let registry = manager.host_registry();
    registry
        .set_plugin_intent(&plugin_id, false, state.secret_store.is_some())
        .map_err(|_| not_found_plugin(&plugin_id))?;
    manager
        .provider_manager()
        .close_plugin(&plugin_id, registry.plugin_lifecycle_epoch(&plugin_id)?);
    cancel_owned_auth_worker(&state, &plugin_id, None, true).await?;
    manager.finish_disable_plugin(&plugin_id).await?;
    let updated = registry
        .host_plugin(&plugin_id)?
        .ok_or_else(|| not_found_plugin(&plugin_id))?;
    Ok(Json(updated))
}

async fn finish_plugin_restart(state: &AppState, plugin_id: &str) -> ApiResult<()> {
    let manager = library_manager(state)?;
    manager.provider_manager().close_plugin(
        plugin_id,
        manager.host_registry().plugin_lifecycle_epoch(plugin_id)?,
    );
    let _begin_guard = state.plugin_auth_begin_gate.lock().await;
    cancel_owned_auth_worker(state, plugin_id, None, true).await?;
    manager.finish_plugin_restart(plugin_id).await?;
    Ok(())
}

async fn delete_plugin(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> ApiResult<Json<PluginCommandResponse>> {
    host_registry(&state)?
        .unregister_plugin(&plugin_id)
        .map_err(|e| {
            if e.to_string().contains("in-use") {
                CoreError::coded(StatusCode::CONFLICT, "plugin_in_use", e.to_string())
            } else {
                not_found_plugin(&plugin_id)
            }
        })?;
    Ok(Json(PluginCommandResponse { accepted: true }))
}

fn host_registry(state: &AppState) -> ApiResult<&Arc<crate::host_registry::HostRegistry>> {
    state
        .library_manager
        .as_ref()
        .map(|m| m.host_registry())
        .ok_or_else(|| {
            CoreError::from(anyhow::anyhow!(
                "Host plugin control is unavailable outside Host mode"
            ))
        })
}

async fn update_plugin_grants(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PluginGrantRequest>,
) -> ApiResult<Json<crate::host_registry::HostPluginProjection>> {
    let lifecycle_manager = library_manager(&state)?;
    let _lifecycle = lifecycle_manager.lock_plugin_lifecycle(&id).await;
    let manager = library_manager(&state)?;
    let registry = manager.host_registry();
    let json = serde_json::to_string(&body.permissions).map_err(anyhow::Error::from)?;
    let fences = registry
        .set_plugin_grants(&id, &json, state.secret_store.is_some())
        .map_err(|error| match error.to_string().as_str() {
            "permission-not-requested" => CoreError::coded(
                StatusCode::BAD_REQUEST,
                "permission_not_requested",
                error.to_string(),
            ),
            _ => not_found_plugin(&id),
        })?;
    manager
        .provider_manager()
        .close_plugin(&id, registry.plugin_lifecycle_epoch(&id)?);
    manager.fence_plugin_bindings(&id, fences).await?;
    finish_plugin_restart(&state, &id).await?;
    Ok(Json(
        registry
            .host_plugin(&id)?
            .ok_or_else(|| not_found_plugin(&id))?,
    ))
}
async fn retry_plugin_runtime(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<(StatusCode, Json<PluginCommandResponse>)> {
    let lifecycle_manager = library_manager(&state)?;
    let _lifecycle = lifecycle_manager.lock_plugin_lifecycle(&id).await;
    host_registry(&state)?
        .retry_plugin_runtime(&id, state.secret_store.is_some())
        .map_err(|error| match error.to_string().as_str() {
            "plugin-disabled" | "plugin-permission-required" | "plugin-configuration-required" => {
                CoreError::coded(
                    StatusCode::CONFLICT,
                    "plugin_not_retryable",
                    error.to_string(),
                )
            }
            _ => not_found_plugin(&id),
        })?;
    finish_plugin_restart(&state, &id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(PluginCommandResponse { accepted: true }),
    ))
}
async fn select_plugin_version(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PluginVersionRequest>,
) -> ApiResult<(StatusCode, Json<PluginCommandResponse>)> {
    let lifecycle_manager = library_manager(&state)?;
    let _lifecycle = lifecycle_manager.lock_plugin_lifecycle(&id).await;
    let manager = library_manager(&state)?;
    let fences = manager
        .host_registry()
        .select_plugin_version(&id, &body.version, state.secret_store.is_some())
        .map_err(|error| match error.to_string().as_str() {
            "plugin-version-switch-in-progress" => CoreError::coded(
                StatusCode::CONFLICT,
                "plugin_version_switch_in_progress",
                error.to_string(),
            ),
            _ => CoreError::coded(
                StatusCode::NOT_FOUND,
                "plugin_version_not_found",
                error.to_string(),
            ),
        })?;
    manager
        .provider_manager()
        .close_plugin(&id, manager.host_registry().plugin_lifecycle_epoch(&id)?);
    manager.fence_plugin_bindings(&id, fences).await?;
    finish_plugin_restart(&state, &id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(PluginCommandResponse { accepted: true }),
    ))
}
async fn rollback_plugin_version(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<(StatusCode, Json<PluginCommandResponse>)> {
    let lifecycle_manager = library_manager(&state)?;
    let _lifecycle = lifecycle_manager.lock_plugin_lifecycle(&id).await;
    let manager = library_manager(&state)?;
    let fences = manager
        .host_registry()
        .rollback_plugin_version(&id)
        .map_err(|error| match error.to_string().as_str() {
            "plugin-not-found" => not_found_plugin(&id),
            _ => CoreError::coded(
                StatusCode::CONFLICT,
                "known_good_unavailable",
                error.to_string(),
            ),
        })?;
    manager
        .provider_manager()
        .close_plugin(&id, manager.host_registry().plugin_lifecycle_epoch(&id)?);
    manager.fence_plugin_bindings(&id, fences).await?;
    finish_plugin_restart(&state, &id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(PluginCommandResponse { accepted: true }),
    ))
}
fn plugin_provider(
    state: &AppState,
    id: &str,
) -> ApiResult<std::sync::Arc<dyn crate::sync::provider::SyncProvider>> {
    let plugin = host_registry(state)?
        .host_plugin(id)?
        .ok_or_else(|| not_found_plugin(id))?;
    let account_uuid = plugin.account_uuid.ok_or_else(|| {
        CoreError::coded(
            StatusCode::CONFLICT,
            "provider_account_required",
            format!("provider account is not configured: {id}"),
        )
    })?;
    host_registry(state)?
        .require_runtime_provider(id, &account_uuid)
        .map_err(|_| {
            CoreError::coded(
                StatusCode::CONFLICT,
                "provider_not_ready",
                format!("provider runtime is not ready: {id}"),
            )
        })?;
    library_manager(state)?
        .provider_manager()
        .resolve(id, &account_uuid)?
        .ok_or_else(|| {
            CoreError::coded(
                StatusCode::CONFLICT,
                "provider_not_ready",
                format!("provider runtime is not ready: {id}"),
            )
        })
}

fn map_provider_error(error: crate::sync::provider::ProviderError) -> CoreError {
    use crate::sync::provider::ProviderErrorCode;
    let status = match error.code {
        ProviderErrorCode::InvalidArgument | ProviderErrorCode::ProtocolError => {
            StatusCode::BAD_REQUEST
        }
        ProviderErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
        ProviderErrorCode::PermissionDenied => StatusCode::FORBIDDEN,
        ProviderErrorCode::NotFound => StatusCode::NOT_FOUND,
        ProviderErrorCode::PreconditionFailed | ProviderErrorCode::CursorReset => {
            StatusCode::CONFLICT
        }
        ProviderErrorCode::Throttled => StatusCode::TOO_MANY_REQUESTS,
        ProviderErrorCode::Unavailable
        | ProviderErrorCode::DeadlineExceeded
        | ProviderErrorCode::Cancelled
        | ProviderErrorCode::Internal => StatusCode::SERVICE_UNAVAILABLE,
    };
    CoreError::coded(status, "provider_error", error.to_string())
}

async fn begin_plugin_auth(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<(StatusCode, Json<PluginAuthBeginResponse>)> {
    let manager = library_manager(&state)?;
    let _lifecycle = manager.lock_plugin_lifecycle(&id).await;
    let _begin_guard = state.plugin_auth_begin_gate.lock().await;
    plugin_provider(&state, &id)?;
    cancel_owned_auth_worker(&state, &id, None, true).await?;
    let plugin = host_registry(&state)?
        .host_plugin(&id)?
        .ok_or_else(|| not_found_plugin(&id))?;
    let account_uuid = plugin.account_uuid.ok_or_else(|| {
        CoreError::coded(
            StatusCode::CONFLICT,
            "provider_account_required",
            format!("provider account is not configured: {id}"),
        )
    })?;
    let supervisor_session = host_registry(&state)?
        .plugin_runtime_session(&id)?
        .ok_or_else(|| {
            CoreError::coded(
                StatusCode::CONFLICT,
                "provider_not_ready",
                "Provider runtime session is unavailable".into(),
            )
        })?;
    let declaration = host_registry(&state)?.plugin_contributions(&id)?;
    let auth_declaration = declaration.authentication.as_ref().ok_or_else(|| {
        CoreError::coded(
            StatusCode::CONFLICT,
            "plugin_auth_unavailable",
            "Provider has no authentication declaration".into(),
        )
    })?;
    let public_url = auth_declaration
        .modes
        .iter()
        .any(|mode| mode == "host-callback")
        .then(|| {
            std::env::var("MANGOO_PUBLIC_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .flatten()
        .map(|value| crate::plugins::auth_flow::PublicUrl::parse(&value, cfg!(debug_assertions)))
        .transpose()?;
    let host_flow_id = if public_url.is_some() {
        Some(
            state
                .plugin_auth_flows
                .create(crate::plugins::auth_flow::AuthFlowBinding {
                    plugin_id: id.clone(),
                    account_uuid,
                    supervisor_session: supervisor_session.clone(),
                    provider_flow_id: None,
                })
                .map_err(|error| {
                    CoreError::coded(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "oauth_flow_capacity",
                        error.to_string(),
                    )
                })?,
        )
    } else {
        None
    };
    let callback_url = match (public_url, host_flow_id.as_deref()) {
        (Some(value), Some(flow_id)) => Some(value.callback_url(flow_id)?.to_string()),
        _ => None,
    };
    let provider = plugin_provider(&state, &id)?;
    let auth = match provider.auth_begin(callback_url.clone()).await {
        Ok(auth) => auth,
        Err(error) => {
            if let Some(flow_id) = host_flow_id.as_deref() {
                state.plugin_auth_flows.cancel_host_flow(flow_id);
            }
            return Err(map_provider_error(error));
        }
    };
    let callback_matches = match callback_url.as_deref() {
        Some(expected) => {
            auth.completion_mode == "host-callback"
                && auth.redirect_uri.as_deref() == Some(expected)
        }
        None => auth.completion_mode != "host-callback",
    };
    if !callback_matches {
        let _ = provider.auth_cancel(&auth.flow_id).await;
        if let Some(flow_id) = host_flow_id.as_deref() {
            state.plugin_auth_flows.cancel_host_flow(flow_id);
        }
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_provider_auth_flow",
            "Provider callback does not match the negotiated flow".into(),
        ));
    }
    let provider_flow_id = auth.flow_id.clone();
    let response_flow_id = if let Some(flow_id) = host_flow_id {
        if state
            .plugin_auth_flows
            .attach_provider_flow(&flow_id, auth.flow_id.clone())
            .is_err()
        {
            state.plugin_auth_flows.cancel_host_flow(&flow_id);
            let _ = provider.auth_cancel(&auth.flow_id).await;
            return Err(CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid_provider_auth_flow",
                "Provider flow identifier is invalid".into(),
            ));
        }
        flow_id
    } else {
        provider_flow_id.clone()
    };
    if !host_registry(&state)?.begin_plugin_auth_flow(
        &id,
        &response_flow_id,
        &supervisor_session,
    )? {
        state.plugin_auth_flows.cancel_host_flow(&response_flow_id);
        let _ = plugin_provider(&state, &id)?
            .auth_cancel(&provider_flow_id)
            .await;
        return Err(CoreError::coded(
            StatusCode::CONFLICT,
            "oauth_session_changed",
            "Provider runtime restarted during authorization".into(),
        ));
    }
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
        + 10 * 60 * 1000;
    if state
        .plugin_auth_flows
        .register_pending(
            &id,
            &response_flow_id,
            &supervisor_session,
            host_registry(&state)?.plugin_lifecycle_epoch(&id)?,
            &auth,
            auth_declaration,
            expires_at,
        )
        .is_err()
    {
        cancel_owned_auth_worker(&state, &id, Some(&response_flow_id), true).await?;
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_provider_auth_flow",
            "Provider authorization declaration is invalid".into(),
        ));
    }
    if auth.completion_mode == "loopback" {
        spawn_loopback_auth_completion(
            state.clone(),
            id.clone(),
            provider_flow_id,
            supervisor_session,
        )
        .await;
    }
    Ok((
        StatusCode::CREATED,
        Json(PluginAuthBeginResponse {
            authorization_url: auth.authorization_url,
            flow_id: response_flow_id,
            expires_at,
            completion_mode: auth.completion_mode.clone(),
            redirect_uri: auth.redirect_uri,
        }),
    ))
}

async fn apply_plugin_auth_result(
    state: &AppState,
    plugin_id: &str,
    flow_id: &str,
    supervisor_session: &str,
    auth: crate::sync::provider::AuthStatus,
) -> ApiResult<()> {
    let Some(fences) = host_registry(state)?.apply_plugin_auth_result_for_flow(
        plugin_id,
        flow_id,
        supervisor_session,
        if auth.authenticated {
            "authenticated"
        } else {
            "authorization-required"
        },
        auth.account_display_name.as_deref(),
        auth.account_kind.as_deref(),
        auth.stable_account_identity.as_deref(),
    )?
    else {
        return Err(CoreError::coded(
            StatusCode::CONFLICT,
            "oauth_flow_cancelled",
            "OAuth flow was cancelled or the Provider runtime changed".into(),
        ));
    };
    if auth.authenticated {
        library_manager(state)?
            .fence_plugin_bindings(plugin_id, fences)
            .await?;
    }
    Ok(())
}

async fn spawn_loopback_auth_completion(
    state: AppState,
    plugin_id: String,
    flow_id: String,
    supervisor_session: String,
) {
    let cancellation = tokio_util::sync::CancellationToken::new();
    let worker_cancel = cancellation.clone();
    let worker_state = state.clone();
    let worker_plugin_id = plugin_id.clone();
    let worker_flow_id = flow_id.clone();
    let worker_session = supervisor_session.clone();
    let task = tokio::spawn(async move {
        let started = tokio::time::Instant::now();
        let deadline = started + std::time::Duration::from_secs(10 * 60);
        let mut delay = std::time::Duration::from_millis(500);
        loop {
            if worker_cancel.is_cancelled()
                || !host_registry(&worker_state)
                    .and_then(|registry| {
                        registry
                            .plugin_auth_flow_is_current(
                                &worker_plugin_id,
                                &worker_flow_id,
                                &worker_session,
                            )
                            .map_err(CoreError::from)
                    })
                    .unwrap_or(false)
            {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                if let Ok(provider) = plugin_provider(&worker_state, &worker_plugin_id) {
                    let _ = provider.auth_cancel(&worker_flow_id).await;
                }
                if let Ok(registry) = host_registry(&worker_state) {
                    let _ = registry.update_plugin_account_auth_for_session(
                        &worker_plugin_id,
                        &worker_session,
                        "expired",
                        None,
                        None,
                    );
                    let _ = registry.finish_plugin_auth_flow(
                        &worker_plugin_id,
                        &worker_flow_id,
                        &worker_session,
                    );
                }
                return;
            }
            let provider = match plugin_provider(&worker_state, &worker_plugin_id) {
                Ok(provider) => provider,
                Err(_) => {
                    if let Ok(registry) = host_registry(&worker_state) {
                        let _ = registry.cancel_plugin_auth_flow(
                            &worker_plugin_id,
                            &worker_flow_id,
                            &worker_session,
                        );
                    }
                    return;
                }
            };
            if !host_registry(&worker_state)
                .and_then(|registry| {
                    registry
                        .claim_plugin_auth_completion(
                            &worker_plugin_id,
                            &worker_flow_id,
                            &worker_session,
                        )
                        .map_err(CoreError::from)
                })
                .unwrap_or(false)
            {
                return;
            }
            match provider.auth_poll(&worker_flow_id).await {
                Ok(Some(auth)) => {
                    let _ = apply_plugin_auth_result(
                        &worker_state,
                        &worker_plugin_id,
                        &worker_flow_id,
                        &worker_session,
                        auth,
                    )
                    .await;
                    if let Ok(registry) = host_registry(&worker_state) {
                        let _ = registry.finish_plugin_auth_flow(
                            &worker_plugin_id,
                            &worker_flow_id,
                            &worker_session,
                        );
                    }
                    return;
                }
                Ok(None) => {
                    if let Ok(registry) = host_registry(&worker_state) {
                        let _ = registry.release_plugin_auth_completion(
                            &worker_plugin_id,
                            &worker_flow_id,
                            &worker_session,
                        );
                    }
                }
                Err(error) if error.retryable => {
                    if let Ok(registry) = host_registry(&worker_state) {
                        let _ = registry.release_plugin_auth_completion(
                            &worker_plugin_id,
                            &worker_flow_id,
                            &worker_session,
                        );
                    }
                }
                Err(_) => {
                    if let Ok(registry) = host_registry(&worker_state) {
                        let _ = registry.update_plugin_account_auth_for_session(
                            &worker_plugin_id,
                            &worker_session,
                            "error",
                            None,
                            None,
                        );
                        let _ = registry.finish_plugin_auth_flow(
                            &worker_plugin_id,
                            &worker_flow_id,
                            &worker_session,
                        );
                    }
                    return;
                }
            }
            tokio::select! {
                _ = worker_cancel.cancelled() => return,
                _ = tokio::time::sleep(delay) => {}
            }
            delay = (delay * 2).min(std::time::Duration::from_secs(2));
        }
    });
    state.plugin_auth_workers.lock().await.insert(
        plugin_id,
        crate::api::PluginAuthWorker {
            flow_id,
            cancellation,
            task,
        },
    );
}

async fn cancel_owned_auth_worker(
    state: &AppState,
    plugin_id: &str,
    requested_flow_id: Option<&str>,
    restore_prior_state: bool,
) -> ApiResult<bool> {
    state
        .plugin_auth_flows
        .cancel_pending(plugin_id, requested_flow_id);
    let ownership = host_registry(state)?.plugin_auth_flow_ownership(plugin_id)?;
    let matches_requested = ownership
        .as_ref()
        .is_some_and(|(flow_id, _)| requested_flow_id.is_none_or(|requested| requested == flow_id));
    let claimed = if let Some((flow_id, supervisor_session)) =
        ownership.as_ref().filter(|_| matches_requested)
    {
        host_registry(state)?.claim_plugin_auth_cancellation(
            plugin_id,
            flow_id,
            supervisor_session,
        )?
    } else {
        false
    };
    let worker = {
        let mut workers = state.plugin_auth_workers.lock().await;
        if workers
            .get(plugin_id)
            .is_some_and(|worker| requested_flow_id.is_none_or(|flow_id| worker.flow_id == flow_id))
        {
            workers.remove(plugin_id)
        } else {
            None
        }
    };
    if let Some(worker) = worker {
        worker.cancellation.cancel();
        let _ = worker.task.await;
    }
    let Some((flow_id, supervisor_session)) = ownership.filter(|_| matches_requested) else {
        return Ok(false);
    };
    let provider_flow_id = state
        .plugin_auth_flows
        .cancel_host_flow(&flow_id)
        .unwrap_or_else(|| flow_id.clone());
    if let Ok(provider) = plugin_provider(state, plugin_id) {
        let _ = provider.auth_cancel(&provider_flow_id).await;
    }
    if restore_prior_state {
        host_registry(state)?.cancel_plugin_auth_flow(plugin_id, &flow_id, &supervisor_session)?;
    } else {
        host_registry(state)?.discard_plugin_auth_flow(plugin_id, &flow_id, &supervisor_session)?;
    }
    Ok(claimed)
}

async fn complete_plugin_auth_callback(
    State(state): State<AppState>,
    Path(flow_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> ApiResult<Html<&'static str>> {
    let entries = crate::plugins::auth_flow::parse_callback_fields(raw_query.as_deref()).map_err(
        |error| {
            CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid_oauth_callback",
                error.to_string(),
            )
        },
    )?;
    let binding = state
        .plugin_auth_flows
        .consume_bound(&flow_id, &entries)
        .map_err(|error| {
            CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid_oauth_callback",
                error.to_string(),
            )
        })?;
    let provider_flow_id = binding.provider_flow_id.ok_or_else(|| {
        CoreError::coded(
            StatusCode::CONFLICT,
            "oauth_flow_invalid",
            "OAuth provider flow is unavailable".into(),
        )
    })?;
    let registry = host_registry(&state)?;
    if registry
        .plugin_runtime_session(&binding.plugin_id)?
        .as_deref()
        != Some(binding.supervisor_session.as_str())
    {
        return Err(CoreError::coded(
            StatusCode::CONFLICT,
            "oauth_session_changed",
            "Provider runtime restarted during authorization".into(),
        ));
    }
    let pending = state
        .plugin_auth_flows
        .pending(
            &binding.plugin_id,
            &flow_id,
            &binding.supervisor_session,
            registry.plugin_lifecycle_epoch(&binding.plugin_id)?,
        )
        .map_err(|_| {
            CoreError::coded(
                StatusCode::CONFLICT,
                "oauth_flow_invalid",
                "Authorization flow is unavailable".into(),
            )
        })?;
    if !pending.is_host_callback()
        || !registry.claim_plugin_auth_completion(
            &binding.plugin_id,
            &flow_id,
            &binding.supervisor_session,
        )?
    {
        return Err(CoreError::coded(
            StatusCode::CONFLICT,
            "oauth_flow_invalid",
            "Authorization flow is unavailable".into(),
        ));
    }
    let auth = match plugin_provider(&state, &binding.plugin_id)?
        .auth_complete(&provider_flow_id, entries.into_iter().collect())
        .await
    {
        Ok(auth) => auth,
        Err(error) => {
            cancel_owned_auth_worker(&state, &binding.plugin_id, Some(&flow_id), true).await?;
            return Err(map_provider_error(error));
        }
    };
    let Some(fences) = registry.apply_plugin_auth_result_for_flow(
        &binding.plugin_id,
        &flow_id,
        &binding.supervisor_session,
        if auth.authenticated {
            "authenticated"
        } else {
            "authorization-required"
        },
        auth.account_display_name.as_deref(),
        auth.account_kind.as_deref(),
        auth.stable_account_identity.as_deref(),
    )?
    else {
        return Err(CoreError::coded(
            StatusCode::CONFLICT,
            "oauth_flow_cancelled",
            "Authorization flow is no longer current".into(),
        ));
    };
    if auth.authenticated {
        library_manager(&state)?
            .fence_plugin_bindings(&binding.plugin_id, fences)
            .await?;
    }
    registry.finish_plugin_auth_flow(&binding.plugin_id, &flow_id, &binding.supervisor_session)?;
    state
        .plugin_auth_flows
        .cancel_pending(&binding.plugin_id, Some(&flow_id));
    Ok(Html(
        "<!doctype html><meta charset=utf-8><title>Mangoo</title>Authorization completed. You may close this window.",
    ))
}
async fn complete_plugin_auth(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PluginAuthCompleteRequest>,
) -> ApiResult<(StatusCode, Json<PluginCommandResponse>)> {
    let ownership = host_registry(&state)?
        .plugin_auth_flow_ownership(&id)?
        .filter(|(flow_id, _)| flow_id == &body.flow_id)
        .ok_or_else(|| {
            CoreError::coded(
                StatusCode::CONFLICT,
                "oauth_flow_invalid",
                "OAuth flow is unavailable or no longer current".into(),
            )
        })?;
    let pending = state
        .plugin_auth_flows
        .pending(
            &id,
            &body.flow_id,
            &ownership.1,
            host_registry(&state)?.plugin_lifecycle_epoch(&id)?,
        )
        .map_err(|_| {
            CoreError::coded(
                StatusCode::CONFLICT,
                "oauth_flow_invalid",
                "Authorization flow is unavailable".into(),
            )
        })?;
    let auth_fields = pending
        .parse_manual_response(&body.response_uri)
        .map_err(|_| {
            CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid_oauth_response_uri",
                "The authorization response URI is invalid".into(),
            )
        })?;
    let supervisor_session = ownership.1;
    if host_registry(&state)?
        .plugin_runtime_session(&id)?
        .as_deref()
        != Some(supervisor_session.as_str())
    {
        return Err(CoreError::coded(
            StatusCode::CONFLICT,
            "oauth_session_changed",
            "Provider runtime restarted during authorization".into(),
        ));
    }
    if !host_registry(&state)?.claim_plugin_auth_completion(
        &id,
        &body.flow_id,
        &supervisor_session,
    )? {
        return Err(CoreError::coded(
            StatusCode::CONFLICT,
            "oauth_flow_cancelled",
            "OAuth flow was cancelled before completion".into(),
        ));
    }
    let provider = match plugin_provider(&state, &id) {
        Ok(provider) => provider,
        Err(error) => {
            let _ = host_registry(&state)?.release_plugin_auth_completion(
                &id,
                &body.flow_id,
                &supervisor_session,
            );
            return Err(error);
        }
    };
    let auth = match provider
        .auth_complete(&body.flow_id, auth_fields.into_iter().collect())
        .await
    {
        Ok(auth) => auth,
        Err(error) => {
            let _ = host_registry(&state)?.release_plugin_auth_completion(
                &id,
                &body.flow_id,
                &supervisor_session,
            );
            return Err(map_provider_error(error));
        }
    };
    let Some(fences) = host_registry(&state)?.apply_plugin_auth_result_for_flow(
        &id,
        &body.flow_id,
        &supervisor_session,
        if auth.authenticated {
            "authenticated"
        } else {
            "authorization-required"
        },
        auth.account_display_name.as_deref(),
        auth.account_kind.as_deref(),
        auth.stable_account_identity.as_deref(),
    )?
    else {
        return Err(CoreError::coded(
            StatusCode::CONFLICT,
            "oauth_flow_cancelled",
            "OAuth flow was cancelled or the Provider runtime changed".into(),
        ));
    };
    if auth.authenticated {
        library_manager(&state)?
            .fence_plugin_bindings(&id, fences)
            .await?;
    }
    host_registry(&state)?.finish_plugin_auth_flow(&id, &body.flow_id, &supervisor_session)?;
    state
        .plugin_auth_flows
        .cancel_pending(&id, Some(&body.flow_id));
    Ok((
        StatusCode::ACCEPTED,
        Json(PluginCommandResponse { accepted: true }),
    ))
}
async fn cancel_plugin_auth(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PluginAuthCancelRequest>,
) -> ApiResult<(StatusCode, Json<PluginCommandResponse>)> {
    let _begin_guard = state.plugin_auth_begin_gate.lock().await;
    cancel_owned_auth_worker(&state, &id, Some(&body.flow_id), true).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(PluginCommandResponse { accepted: true }),
    ))
}
async fn sign_out_plugin(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<(StatusCode, Json<PluginCommandResponse>)> {
    let lifecycle_manager = library_manager(&state)?;
    let _lifecycle = lifecycle_manager.lock_plugin_lifecycle(&id).await;
    let _begin_guard = state.plugin_auth_begin_gate.lock().await;
    let registry = Arc::clone(host_registry(&state)?);
    let account = registry
        .plugin_account_secret(&id)?
        .ok_or_else(|| not_found_plugin(&id))?;
    let prior_flow = registry.plugin_auth_flow_ownership(&id)?;
    let fences = registry
        .sign_out_plugin_account(&id, account.generation)?
        .ok_or_else(|| {
            CoreError::coded(
                StatusCode::CONFLICT,
                "plugin_account_changed",
                "Provider account changed during sign-out".into(),
            )
        })?;
    lifecycle_manager
        .provider_manager()
        .close_plugin(&id, registry.plugin_lifecycle_epoch(&id)?);
    if let Some((flow_id, _)) = prior_flow {
        state.plugin_auth_flows.cancel_host_flow(&flow_id);
    }
    library_manager(&state)?
        .fence_plugin_bindings(&id, fences)
        .await?;
    cancel_owned_auth_worker(&state, &id, None, false).await?;
    library_manager(&state)?.stop_plugin_runtime(&id).await?;
    lifecycle_manager
        .provider_manager()
        .join_plugin_work(&id)
        .await;
    registry.finish_plugin_runtime_restart(&id)?;
    crate::api::reconcile_credential_retirements_once(&state)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(PluginCommandResponse { accepted: true }),
    ))
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginExternalLinkRequest {
    kind: String,
    id: String,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginExternalLink {
    kind: String,
    id: String,
    url: String,
    expires_at: Option<i64>,
}

async fn resolve_plugin_external_link(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    Json(body): Json<PluginExternalLinkRequest>,
) -> ApiResult<Json<PluginExternalLink>> {
    let registry = host_registry(&state)?;
    registry
        .host_plugin(&plugin_id)?
        .ok_or_else(|| not_found_plugin(&plugin_id))?;
    if body.id.is_empty() || body.id.len() > 160 {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_external_link",
            "Invalid link identifier".into(),
        ));
    }
    let (url, expires_at) = match body.kind.as_str() {
        "authorization" => {
            plugin_provider(&state, &plugin_id)?;
            let ownership = registry
                .plugin_auth_flow_ownership(&plugin_id)?
                .filter(|(id, _)| id == &body.id)
                .ok_or_else(|| {
                    CoreError::coded(
                        StatusCode::CONFLICT,
                        "oauth_flow_invalid",
                        "Authorization flow is unavailable".into(),
                    )
                })?;
            if registry.plugin_runtime_session(&plugin_id)?.as_deref() != Some(ownership.1.as_str())
                || !registry.plugin_auth_flow_is_current(&plugin_id, &body.id, &ownership.1)?
            {
                return Err(CoreError::coded(
                    StatusCode::CONFLICT,
                    "oauth_flow_invalid",
                    "Authorization flow is unavailable".into(),
                ));
            }
            let pending = state
                .plugin_auth_flows
                .pending(
                    &plugin_id,
                    &body.id,
                    &ownership.1,
                    registry.plugin_lifecycle_epoch(&plugin_id)?,
                )
                .map_err(|_| {
                    CoreError::coded(
                        StatusCode::CONFLICT,
                        "oauth_flow_invalid",
                        "Authorization flow is unavailable".into(),
                    )
                })?;
            (pending.authorization_url, Some(pending.expires_at))
        }
        "help" => {
            let declaration = registry.plugin_contributions(&plugin_id)?;
            let link = declaration
                .presentation
                .as_ref()
                .and_then(|p| p.help_links.iter().find(|l| l.id == body.id))
                .ok_or_else(|| {
                    CoreError::coded(
                        StatusCode::NOT_FOUND,
                        "plugin_link_not_found",
                        "Provider help link is unavailable".into(),
                    )
                })?;
            crate::plugins::contributions::safe_external_url(&link.url, false)?;
            (link.url.clone(), None)
        }
        _ => {
            return Err(CoreError::coded(
                StatusCode::BAD_REQUEST,
                "invalid_external_link",
                "Invalid link kind".into(),
            ))
        }
    };
    Ok(Json(PluginExternalLink {
        kind: body.kind,
        id: body.id,
        url,
        expires_at,
    }))
}

async fn get_plugin_configuration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::plugin_configuration::PluginConfiguration>> {
    host_registry(&state)?
        .host_plugin(&id)?
        .ok_or_else(|| not_found_plugin(&id))?;
    Ok(Json(host_registry(&state)?.plugin_configuration(&id)?))
}
async fn plan_plugin_configuration(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PluginConfigurationPlanRequest>,
) -> ApiResult<Json<crate::plugin_configuration::PluginConfigurationPlan>> {
    host_registry(&state)?
        .host_plugin(&id)?
        .ok_or_else(|| not_found_plugin(&id))?;
    host_registry(&state)?
        .plan_plugin_configuration(&id, &body.values, body.expected_revision)
        .map(Json)
        .map_err(|error| {
            CoreError::coded(
                StatusCode::CONFLICT,
                "invalid_plugin_configuration",
                error.to_string(),
            )
        })
}
async fn commit_plugin_configuration(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PluginConfigurationCommitRequest>,
) -> ApiResult<Json<crate::plugin_configuration::PluginConfiguration>> {
    let manager = library_manager(&state)?;
    let _lifecycle = manager.lock_plugin_lifecycle(&id).await;
    let _begin_guard = state.plugin_auth_begin_gate.lock().await;
    let registry = host_registry(&state)?;
    registry
        .host_plugin(&id)?
        .ok_or_else(|| not_found_plugin(&id))?;
    let prior_flow = registry.plugin_auth_flow_ownership(&id)?;
    let commit = registry
        .commit_plugin_configuration(&id, &body.plan_uuid, body.expected_revision, body.confirmed)
        .map_err(|error| {
            CoreError::coded(
                StatusCode::CONFLICT,
                "invalid_plugin_configuration",
                error.to_string(),
            )
        })?;
    if commit.effective_changed {
        manager
            .provider_manager()
            .close_plugin(&id, registry.plugin_lifecycle_epoch(&id)?);
        if let Some((flow_id, _)) = prior_flow {
            state.plugin_auth_flows.cancel_host_flow(&flow_id);
        }
        manager.fence_plugin_bindings(&id, commit.fences).await?;
        cancel_owned_auth_worker(&state, &id, None, false).await?;
        manager.stop_plugin_runtime(&id).await?;
        manager.provider_manager().join_plugin_work(&id).await;
        registry.finish_plugin_configuration_stop(&id)?;
        crate::api::reconcile_credential_retirements_once(&state)?;
    }
    Ok(Json(registry.plugin_configuration(&id)?))
}
async fn plugin_health(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PluginHealthResponse>> {
    if let Ok(provider) = plugin_provider(&state, &id) {
        let health = provider.health().await.map_err(map_provider_error)?;
        return Ok(Json(PluginHealthResponse {
            state: if health.ready { "ready" } else { "degraded" }.to_string(),
            last_healthy_at: None,
            last_error_code: health.detail,
        }));
    }
    let p = host_registry(&state)?
        .host_plugin(&id)?
        .ok_or_else(|| not_found_plugin(&id))?;
    Ok(Json(PluginHealthResponse {
        state: p.runtime_state,
        last_healthy_at: p.last_healthy_at,
        last_error_code: p.last_error_code,
    }))
}
async fn plugin_diagnostics(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PluginDiagnosticsResponse>> {
    if let Ok(provider) = plugin_provider(&state, &id) {
        let entry = provider.diagnostics().await.map_err(map_provider_error)?;
        return Ok(Json(PluginDiagnosticsResponse {
            generated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            entries: vec![entry],
        }));
    }
    let p = host_registry(&state)?
        .host_plugin(&id)?
        .ok_or_else(|| not_found_plugin(&id))?;
    let entry = serde_json::json!({"runtimeState":p.runtime_state,"lastErrorCode":p.last_error_code,"lastErrorDetail":p.last_error_detail});
    Ok(Json(PluginDiagnosticsResponse {
        generated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
        entries: vec![entry],
    }))
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
fn fsops_database(state: &AppState) -> ApiResult<std::sync::Arc<std::sync::Mutex<Database>>> {
    state
        .fsops_database()
        .map_err(CoreError::from)?
        .ok_or_else(|| {
            CoreError::from(anyhow::anyhow!(
                "file operations are unavailable without a file-backed database"
            ))
        })
}

fn resolve_plugins_dir(state: &AppState) -> std::path::PathBuf {
    // Prefer the directory the startup code already resolved against the
    // active database path. Falling back to the env var or `./plugins`
    // would diverge from `main.rs` and was the root cause of Batch G HIGH 1.
    if let Ok(Some(configured)) = state.plugins_dir() {
        return configured;
    }
    // Defensive fallback for callers that built `AppState` without going
    // through `router_with_config` (test helpers, the deprecated `router`
    // alias). Match the same env-var precedence as
    // `plugins::resolve_plugins_dir` so behavior stays predictable.
    if let Some(value) = std::env::var("MANGOO_PLUGINS_DIR")
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
    let tag = run_authoritative_database(state, move |database| {
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
    run_authoritative_database(state, move |database| {
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
    let record = run_authoritative_database(state, move |database| {
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
    let tag_ids = run_authoritative_database(state, move |database| {
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
    let tag_ids = run_authoritative_database(state, move |database| {
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
    let tag_ids = run_authoritative_database(state, move |database| {
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
    if message.contains("invalid folder cursor: parent or sort revision changed") {
        return CoreError::coded(StatusCode::BAD_REQUEST, "stale_folder_cursor", message);
    }
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
    (
        state.writer().expect("active library writer").instance_id(),
        task_id,
        task_priority,
    )
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

pub(super) fn preview_content_type(path: &FilePath, media_kind: Option<&str>) -> &'static str {
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
    match state
        .task_queue()
        .expect("active library task queue")
        .try_send(task_id)
    {
        Ok(()) => Ok(()),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            // The task row is already durable. If the worker channel is full,
            // returning immediately keeps foreground browsing responsive, but
            // still queue a best-effort async send so the task is not reliant
            // on later incidental drain ticks to start.
            let state = state.clone();
            tokio::spawn(async move {
                if state
                    .task_queue()
                    .expect("active library task queue")
                    .send(task_id)
                    .await
                    .is_err()
                {
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

    pub(super) fn not_found(message: String) -> Self {
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

    pub(super) fn coded(status: StatusCode, code: &str, message: String) -> Self {
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
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request, StatusCode};
    use axum::Router;
    use base64::Engine as _;
    use tower::ServiceExt;

    use super::{
        fallback_display_name, notification_outbox_response, resolve_preview_plan,
        restore_operation_request, router, ErrorResponse, PreviewBlobRecord,
        RestorePreflightRequest, RestoreRootAwareItem, RestoreRootAwareMode, RestoreRootChoice,
        RestoreStartItem, PREVIEW_NATIVE_MAX_PX, PREVIEW_NATIVE_PROFILE,
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
    fn sync_folder_marker_write_failures_are_redacted_as_internal_errors() {
        let error = super::sync_folder_error(anyhow::anyhow!(
            "marker-write-failed: publish marker: sensitive path"
        ));
        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.code.as_deref(), Some("internal-error"));
        assert_eq!(error.message, "Synchronized-folder operation failed");
    }

    #[test]
    fn restore_contract_keeps_legacy_compatibility_and_rejects_unknown_modes() {
        let legacy: RestorePreflightRequest = serde_json::from_value(serde_json::json!({
            "items": [{
                "accountUuid": "account",
                "libraryUuid": "00000000-0000-7000-8000-000000000101",
                "name": "Legacy",
                "placementBase": "D:/Legacy"
            }]
        }))
        .expect("legacy restore request");
        assert!(matches!(legacy.items[0], RestoreStartItem::Legacy(_)));
        assert!(
            serde_json::from_value::<RestorePreflightRequest>(serde_json::json!({
                "items": [{
                    "restoreMode": "future-mode",
                    "accountUuid": "account",
                    "libraryUuid": "00000000-0000-7000-8000-000000000101",
                    "name": "Legacy",
                    "placementBase": "D:/Legacy"
                }]
            }))
            .is_err()
        );
    }

    #[test]
    fn restore_request_hash_input_is_order_independent() {
        let item = |library: &str, root: &str| {
            RestoreStartItem::RootAware(RestoreRootAwareItem {
                restore_mode: RestoreRootAwareMode::RootAware,
                account_uuid: "account".into(),
                library_uuid: library.into(),
                placement_base: format!("D:/{library}"),
                inventory_fence: "a".repeat(64),
                roots: vec![RestoreRootChoice {
                    root_uuid: root.into(),
                    expected_logical_revision: 1,
                    expected_materialization_fingerprint: "b".repeat(64),
                    selected: false,
                    target_path: None,
                }],
            })
        };
        let first = vec![item("library-b", "root-b"), item("library-a", "root-a")];
        let second = vec![item("library-a", "root-a"), item("library-b", "root-b")];
        assert_eq!(
            restore_operation_request(&first),
            restore_operation_request(&second)
        );
    }

    #[test]
    fn fallback_display_name_uses_last_path_component() {
        assert_eq!(fallback_display_name("D:\\Media\\Photos\\"), "Photos");
        assert_eq!(fallback_display_name("D:/Media/Videos"), "Videos");
        assert_eq!(fallback_display_name("D:\\"), "D:");
    }

    mod restore_admission {
        use super::*;
        use crate::host_registry::{
            DiscoveredRemoteLibrary, DiscoveredRemoteRoot, RestorePlanInput, RestoreRootPlanInput,
        };
        use crate::library_manager::{ActiveSnapshot, LibraryManager};
        use crate::library_path::MARKER_FILE_NAME;
        use std::collections::BTreeMap;
        use std::path::Path;
        use std::sync::RwLock;

        const ACCOUNT: &str = "restore-fixture-account";

        #[test]
        fn library_reads_follow_activation_on_a_reused_blocking_thread() {
            // One blocking thread makes a stale thread-local connection
            // deterministic instead of depending on pool scheduling.
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .max_blocking_threads(1)
                .build()
                .unwrap()
                .block_on(async {
                    let (base, manager, app) = fixture();
                    let mut libraries = Vec::new();
                    for name in ["First", "Second"] {
                        libraries.push(
                            manager
                                .create_library(
                                    name.into(),
                                    None,
                                    None,
                                    base.join(name).join("Database"),
                                    None,
                                    false,
                                )
                                .await
                                .unwrap()
                                .0,
                        );
                    }
                    for index in [0, 1, 0, 1] {
                        let library = &libraries[index];
                        let previous = manager.active_snapshot();
                        let revision = previous.context_revision;
                        let activated = app
                            .clone()
                            .oneshot(
                                Request::builder()
                                    .method(Method::POST)
                                    .uri(format!(
                                        "/api/libraries/{}/activate",
                                        library.library_uuid
                                    ))
                                    .header("x-mangoo-context-revision", revision.to_string())
                                    .body(Body::empty())
                                    .unwrap(),
                            )
                            .await
                            .unwrap();
                        assert_eq!(activated.status(), StatusCode::OK);
                        if let Some(retired) = previous.runtime {
                            assert!(retired.read_database_cache.checkout().is_err());
                            // In particular, SQLite handles must be closed on
                            // Windows even while this old runtime Arc survives.
                            let catalog = retired.read_database_path.as_ref().unwrap();
                            let moved = catalog.with_extension("retired-check");
                            fs::rename(catalog, &moved).unwrap();
                            fs::rename(moved, catalog).unwrap();
                        }
                        let expected_path = fs::canonicalize(
                            base.join(if index == 0 { "First" } else { "Second" })
                                .join("Database-Files"),
                        )
                        .unwrap();
                        for _ in 0..2 {
                            let response = app
                                .clone()
                                .oneshot(
                                    Request::builder()
                                        .uri("/api/roots")
                                        .body(Body::empty())
                                        .unwrap(),
                                )
                                .await
                                .unwrap();
                            assert_eq!(response.status(), StatusCode::OK);
                            assert_eq!(
                                response.headers()["x-mangoo-library-id"],
                                library.library_uuid
                            );
                            assert_eq!(
                                response.headers()["x-mangoo-context-revision"],
                                (revision + 1).to_string()
                            );
                            let body = response_json(response).await;
                            assert_eq!(body["items"].as_array().unwrap().len(), 1);
                            assert_eq!(
                                Path::new(body["items"][0]["path"].as_str().unwrap()),
                                expected_path
                            );
                        }
                    }
                    manager
                        .active_snapshot()
                        .runtime
                        .unwrap()
                        .shutdown()
                        .await
                        .unwrap();
                });
        }

        #[test]
        fn cancelled_queued_read_cannot_adopt_a_new_library_runtime() {
            use crate::api::routes::run_read_database;
            use std::future::Future;
            use std::task::Poll;

            let reads = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .max_blocking_threads(1)
                .build()
                .unwrap();
            // Activation needs its own blocking executor so the delayed read
            // can stay queued until the old runtime is completely retired.
            let activation = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(1)
                .build()
                .unwrap();
            reads.block_on(async {
                let (base, manager, app, state) = fixture_with_state();
                manager
                    .create_library(
                        "First".into(),
                        None,
                        None,
                        base.join("First/Database"),
                        None,
                        true,
                    )
                    .await
                    .unwrap();
                let (second, _, _) = manager
                    .create_library(
                        "Second".into(),
                        None,
                        None,
                        base.join("Second/Database"),
                        None,
                        false,
                    )
                    .await
                    .unwrap();
                let retired = state.resources().unwrap();
                let revision = manager.active_snapshot().context_revision;
                // Do not strand unrelated startup scan jobs behind the read
                // barrier: real runtime retirement must join those jobs too.
                tokio::time::timeout(std::time::Duration::from_secs(60), async {
                    loop {
                        let tasks = retired
                            .writer
                            .write(|database| database.list_tasks())
                            .await
                            .unwrap();
                        if tasks
                            .iter()
                            .all(|task| !matches!(task.status.as_str(), "pending" | "running"))
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                let (started, ready) = tokio::sync::oneshot::channel();
                let (release, released) = std::sync::mpsc::channel();
                let blocker = tokio::task::spawn_blocking(move || {
                    started.send(()).unwrap();
                    released.recv().unwrap();
                });
                ready.await.unwrap();

                let gate = state.request_gate().read_owned().await;
                let (observed, observation) = tokio::sync::oneshot::channel();
                let mut read = Box::pin(run_read_database(state, move |database| {
                    let roots = database.list_roots()?;
                    observed
                        .send(roots.into_iter().map(|root| root.path).collect::<Vec<_>>())
                        .unwrap();
                    Ok(())
                }));
                std::future::poll_fn(|cx| {
                    assert!(read.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
                // Match a disconnected HTTP request: its waiter and admission
                // guard disappear, but spawn_blocking work remains submitted.
                drop(read);
                drop(gate);
                let switched = activation
                    .spawn(async move {
                        app.oneshot(
                            Request::builder()
                                .method(Method::POST)
                                .uri(format!("/api/libraries/{}/activate", second.library_uuid))
                                .header("x-mangoo-context-revision", revision.to_string())
                                .body(Body::empty())
                                .unwrap(),
                        )
                        .await
                        .unwrap()
                    })
                    .await
                    .unwrap();
                assert_eq!(switched.status(), StatusCode::OK);
                assert!(retired.read_database_cache.checkout().is_err());
                release.send(()).unwrap();
                blocker.await.unwrap();
                let queried = tokio::time::timeout(std::time::Duration::from_secs(10), observation)
                    .await
                    .unwrap();
                assert!(
                    queried.is_err(),
                    "retired request queried a new Library: {queried:?}"
                );
                manager
                    .active_snapshot()
                    .runtime
                    .unwrap()
                    .shutdown()
                    .await
                    .unwrap();
            });
        }

        #[tokio::test]
        async fn separated_storage_create_add_root_and_activate_serialize_without_deadlock() {
            use std::future::Future;
            use std::task::Poll;

            let (base, manager, app) = fixture();
            let (first, _, _) = manager
                .create_library(
                    "First".into(),
                    None,
                    None,
                    base.join("First/Database"),
                    None,
                    true,
                )
                .await
                .unwrap();
            let (second, _, _) = manager
                .create_library(
                    "Second".into(),
                    None,
                    None,
                    base.join("Second/Database"),
                    None,
                    false,
                )
                .await
                .unwrap();
            let revision = manager.active_snapshot().context_revision;
            let extra = base.join("additional-root");
            fs::create_dir(&extra).unwrap();
            let request = Request::builder()
                .method(Method::POST)
                .uri("/api/roots")
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-mangoo-context-revision", revision.to_string())
                .body(Body::from(
                    serde_json::json!({"path":extra,"displayName":"Extra"}).to_string(),
                ))
                .unwrap();
            let admission = manager.path_admission().await;
            let activated = manager.activate_with_revision(&second.library_uuid, revision);
            let added = app.clone().oneshot(request);
            let created = manager.create_library(
                "Third".into(),
                None,
                None,
                base.join("Third/Database"),
                None,
                false,
            );
            tokio::pin!(activated, added, created);
            // Queue activation first while holding operation ownership. A root
            // request that takes the read gate before operation ownership would
            // now block activation's write gate, deterministically forming the
            // lock cycle instead of depending on join!'s polling order.
            std::future::poll_fn(|cx| {
                assert!(activated.as_mut().poll(cx).is_pending());
                assert!(added.as_mut().poll(cx).is_pending());
                assert!(created.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            drop(admission);
            // This bounds a deadlock, not filesystem/runtime-start performance:
            // full-suite contention must not consume a short timing assertion.
            let (created, added, activated) =
                tokio::time::timeout(std::time::Duration::from_secs(60), async {
                    tokio::join!(created, added, activated)
                })
                .await
                .expect("operation mutex must precede the ordinary-root request gate");
            assert!(created.is_ok());
            let status = added.unwrap().status();
            assert_eq!(status, StatusCode::CONFLICT);
            assert!(activated.is_ok());
            assert_eq!(manager.registry().snapshot().unwrap().libraries.len(), 3);
            for library in [&first, &second] {
                let catalog =
                    Database::open_existing(library.local_data_dir.join("catalog.sqlite")).unwrap();
                assert!(catalog.list_roots().unwrap().iter().all(|root| {
                    !crate::library_path::paths_overlap(
                        Path::new(&root.path),
                        &library.local_data_dir,
                    )
                }));
            }
            manager
                .active_snapshot()
                .runtime
                .unwrap()
                .shutdown()
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn separated_storage_restore_batch_checks_both_path_roles_in_both_orders() {
            for root_aware in [false, true] {
                for reverse in [false, true] {
                    let (base, manager, _app) = fixture();
                    let first = uuid::Uuid::now_v7().to_string();
                    let second = uuid::Uuid::now_v7().to_string();
                    discover(&manager, &first);
                    discover(&manager, &second);
                    let mut items = vec![
                        item(&manager, &base, &first, root_aware),
                        item(&manager, &base, &second, root_aware),
                    ];
                    let first_base = PathBuf::from(items[0]["placementBase"].as_str().unwrap());
                    if root_aware {
                        items[1]["roots"][0]["targetPath"] =
                            serde_json::json!(first_base.join("Database/nested"));
                    } else {
                        items[1]["placementBase"] = serde_json::json!(first_base.join("Files"));
                    }
                    if reverse {
                        items.reverse();
                    }
                    let parsed: RestorePreflightRequest =
                        serde_json::from_value(serde_json::json!({"items":items})).unwrap();
                    let result =
                        super::super::validate_restore_items(&manager, &parsed.items, false);
                    match result {
                        Err(error) => {
                            assert!(error.message.contains("overlap"), "{}", error.message)
                        }
                        Ok(result) => {
                            assert!(result
                                .root_results
                                .iter()
                                .any(|root| root.reason.as_deref()
                                    == Some("database-content-overlap")))
                        }
                    }
                    assert!(!first_base.exists());
                }
            }
        }

        fn fixture() -> (PathBuf, Arc<LibraryManager>, Router) {
            let (base, manager, app, _) = fixture_with_state();
            (base, manager, app)
        }

        fn fixture_with_state() -> (PathBuf, Arc<LibraryManager>, Router, AppState) {
            let base = unique_temp_dir();
            fs::create_dir_all(&base).unwrap();
            let registry = Arc::new(
                crate::host_registry::HostRegistry::open(base.join("host.sqlite")).unwrap(),
            );
            registry
                .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                    plugin_id: "restore-fixture",
                    version: "1.0.0",
                    manifest_json: r#"{"manifestVersion":2}"#,
                    entry_relative_path: "bin/provider",
                    entry_sha256: &[7; 32],
                    install_root: &base,
                    trust_policy_id: "fixture",
                    requested_permissions_json: "[]",
                })
                .unwrap();
            registry
                .upsert_plugin_account(ACCOUNT, "restore-fixture", Some("Fixture"), "authenticated")
                .unwrap();
            registry
                .set_plugin_intent("restore-fixture", true, true)
                .unwrap();
            registry
                .record_plugin_runtime_ready(
                    "restore-fixture",
                    "restore-fixture-session",
                    &["sync-provider.v1".into()],
                )
                .unwrap();
            let active = Arc::new(RwLock::new(ActiveSnapshot {
                library_uuid: None,
                context_revision: 0,
                runtime_generation: 0,
                runtime: None,
            }));
            let request_gate = Arc::new(tokio::sync::RwLock::new(()));
            let manager = Arc::new(LibraryManager::new(
                registry,
                base.join("libraries"),
                base.join("plugins"),
                None,
                active.clone(),
                request_gate.clone(),
            ));
            let state = AppState {
                playback_sessions: Arc::new(Default::default()),
                active,
                request_gate,
                plugin_ui_gate: Arc::new(tokio::sync::RwLock::new(())),
                library_manager: Some(manager.clone()),
                plugin_auth_flows: Arc::new(Default::default()),
                plugin_auth_workers: Arc::new(Default::default()),
                plugin_auth_begin_gate: Arc::new(Default::default()),
                secret_store: None,
                notification_outbox_test_probe: None,
            };
            let app = router(state.clone()).layer(axum::middleware::from_fn_with_state(
                state.clone(),
                crate::api::attach_library_context,
            ));
            (base, manager, app, state)
        }

        fn discover(manager: &LibraryManager, library: &str) {
            let transport = manager
                .registry()
                .provider_root_transports()
                .unwrap()
                .remove(0);
            manager
                .registry()
                .commit_provider_root_cursor(
                    ACCOUNT,
                    transport.account_generation,
                    &transport.runtime_session_id,
                    None,
                    &[DiscoveredRemoteLibrary {
                        metadata: crate::sync::provider::ObjectMetadata {
                            key: format!("{library}/library-descriptor"),
                            size: 42,
                            sha256: [9; 32],
                            etag: None,
                        },
                        name: Some("Remote fixture".into()),
                        icon: None,
                        media_count: Some(0),
                        logical_bytes: Some(0),
                        last_synced_at: None,
                        compatible: true,
                        error_code: None,
                        evidence: None,
                        history: BTreeMap::new(),
                        roots: vec![DiscoveredRemoteRoot {
                            root_uuid: uuid::Uuid::now_v7().to_string(),
                            root_folder_uuid: uuid::Uuid::now_v7().to_string(),
                            requested_name: "Files".into(),
                            materialized_name: "Files".into(),
                            materialization_fingerprint: "a".repeat(64),
                            logical_revision: 1,
                            logical_bytes: Some(0),
                        }],
                    }],
                )
                .unwrap();
        }

        fn item(
            manager: &LibraryManager,
            base: &Path,
            library: &str,
            root_aware: bool,
        ) -> serde_json::Value {
            if root_aware {
                let inventory = manager
                    .registry()
                    .remote_library_root_inventory(ACCOUNT, library, None, usize::MAX)
                    .unwrap();
                let root = &inventory.items[0];
                serde_json::json!({ "restoreMode":"root-aware", "accountUuid":ACCOUNT, "libraryUuid":library,
                    "placementBase":base.join(format!("shell-{library}")), "inventoryFence":inventory.inventory_fence,
                    "roots":[{"rootUuid":root.root_uuid,"expectedLogicalRevision":root.logical_revision,
                        "expectedMaterializationFingerprint":root.materialization_fingerprint,"selected":true,"targetPath":base.join(format!("files-{library}"))}] })
            } else {
                serde_json::json!({"accountUuid":ACCOUNT,"libraryUuid":library,"name":"Remote fixture","placementBase":base.join(format!("shell-{library}"))})
            }
        }

        async fn post(
            app: &Router,
            path: &str,
            body: serde_json::Value,
        ) -> (StatusCode, serde_json::Value) {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(path)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            (status, response_json(response).await)
        }

        fn request(items: &[serde_json::Value]) -> serde_json::Value {
            let parsed: RestorePreflightRequest =
                serde_json::from_value(serde_json::json!({"items":items})).unwrap();
            restore_operation_request(&parsed.items)
        }

        fn plan_inputs(
            manager: &LibraryManager,
            items: &[serde_json::Value],
        ) -> Vec<RestorePlanInput> {
            items
                .iter()
                .map(|item| {
                    let library = item["libraryUuid"].as_str().unwrap();
                    let root_aware = item["restoreMode"] == "root-aware";
                    let roots = if root_aware {
                        manager
                            .registry()
                            .remote_library_root_inventory(ACCOUNT, library, None, usize::MAX)
                            .unwrap()
                            .items
                            .into_iter()
                            .map(|root| RestoreRootPlanInput {
                                root_uuid: root.root_uuid,
                                root_folder_uuid: root.root_folder_uuid,
                                requested_name: root.requested_name,
                                materialized_name: root.materialized_name,
                                materialization_fingerprint: root.materialization_fingerprint,
                                logical_revision: root.logical_revision,
                                logical_bytes: root.logical_bytes,
                                selected_binding: None,
                            })
                            .collect()
                    } else {
                        vec![]
                    };
                    RestorePlanInput {
                        account_uuid: ACCOUNT.into(),
                        library_uuid: library.into(),
                        managed_root: PathBuf::from(
                            item[if root_aware {
                                "placementBase"
                            } else {
                                "placementBase"
                            }]
                            .as_str()
                            .unwrap(),
                        ),
                        restore_mode: if root_aware {
                            "root-aware"
                        } else {
                            "legacy-library"
                        }
                        .into(),
                        inventory_fence: item["inventoryFence"].as_str().map(str::to_owned),
                        roots,
                    }
                })
                .collect()
        }

        // Read-only logical DB and byte-level filesystem snapshots. Host WAL bytes
        // themselves are excluded; every table affected by admission is included.
        fn snapshot(
            base: &Path,
        ) -> (
            Vec<(String, Vec<Vec<String>>)>,
            BTreeMap<PathBuf, Option<Vec<u8>>>,
        ) {
            let db = rusqlite::Connection::open_with_flags(
                base.join("host.sqlite"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let tables = [
                "libraries",
                "host_meta",
                "host_sync_local_state",
                "host_sync_actor_epochs",
                "sync_restore_preflights",
                "sync_orchestration_operations",
                "sync_restore_plans",
                "sync_restore_root_plans",
                "library_creation_journal",
                "library_sync_bindings",
                "remote_sync_roots",
                "local_root_bindings",
                "library_local_settings",
                "provider_remote_libraries",
                "provider_remote_sync_roots",
            ];
            let rows = tables
                .into_iter()
                .map(|table| {
                    let mut statement = db
                        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                        .unwrap();
                    let columns = statement.column_count();
                    let values = statement
                        .query_map([], |row| {
                            (0..columns)
                                .map(|i| {
                                    row.get::<_, rusqlite::types::Value>(i)
                                        .map(|value| format!("{value:?}"))
                                })
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .unwrap()
                        .collect::<Result<Vec<_>, _>>()
                        .unwrap();
                    (table.to_owned(), values)
                })
                .collect();
            fn visit(base: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
                for entry in fs::read_dir(path).unwrap() {
                    let entry = entry.unwrap();
                    if path == base
                        && entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with("host.sqlite")
                    {
                        continue;
                    }
                    let path = entry.path();
                    let relative = path.strip_prefix(base).unwrap().to_path_buf();
                    if path.is_dir() {
                        result.insert(relative, None);
                        visit(base, &path, result);
                    } else {
                        result.insert(relative, Some(fs::read(&path).unwrap()));
                    }
                }
            }
            let mut files = BTreeMap::new();
            visit(base, base, &mut files);
            (rows, files)
        }

        fn count(base: &Path, table: &str) -> i64 {
            rusqlite::Connection::open_with_flags(
                base.join("host.sqlite"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
        }

        #[tokio::test]
        async fn duplicate_preflight_rejects_entire_aggregate_before_path_validation() {
            for root_aware in [false, true] {
                let (base, manager, app) = fixture();
                let existing = manager
                    .create_library(
                        "Existing".into(),
                        None,
                        None,
                        base.join("original"),
                        None,
                        false,
                    )
                    .await
                    .unwrap()
                    .0;
                fs::write(existing.managed_root.join("keep.txt"), b"original bytes").unwrap();
                let other = uuid::Uuid::now_v7().to_string();
                discover(&manager, &existing.library_uuid);
                discover(&manager, &other);
                let mut first = item(&manager, &base, &other, root_aware);
                first[if root_aware {
                    "placementBase"
                } else {
                    "placementBase"
                }] = "invalid-relative-path".into();
                let duplicate = item(&manager, &base, &existing.library_uuid, root_aware);
                let before = snapshot(&base);
                for items in [vec![duplicate.clone()], vec![first, duplicate]] {
                    let (status, body) = post(
                        &app,
                        "/api/sync/restores/preflight",
                        serde_json::json!({"items":items}),
                    )
                    .await;
                    assert_eq!(status, StatusCode::CONFLICT, "{body}");
                    assert_eq!(body["code"], "library_already_present");
                    assert_eq!(snapshot(&base), before);
                }
                assert_eq!(count(&base, "sync_restore_preflights"), 0);
                assert_eq!(count(&base, "sync_restore_plans"), 0);
                drop(app);
                drop(manager);
                fs::remove_dir_all(base).unwrap();
            }
        }

        #[tokio::test]
        async fn registration_after_preflight_rejects_start_and_atomic_claim_without_mutation() {
            for root_aware in [false, true] {
                let (base, manager, app) = fixture();
                let ids = [
                    uuid::Uuid::now_v7().to_string(),
                    uuid::Uuid::now_v7().to_string(),
                ];
                for id in &ids {
                    discover(&manager, id);
                }
                let items = ids
                    .iter()
                    .map(|id| item(&manager, &base, id, root_aware))
                    .collect::<Vec<_>>();
                let (status, preflight) = post(
                    &app,
                    "/api/sync/restores/preflight",
                    serde_json::json!({"items":items}),
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{preflight}");
                assert_eq!(preflight["ok"], true, "{preflight}");
                let token = preflight["preflightToken"].as_str().unwrap();
                let operation = uuid::Uuid::now_v7().to_string();
                let existing = manager
                    .create_restore_library(
                        ids[1].clone(),
                        "Concurrent".into(),
                        base.join("concurrent"),
                        ACCOUNT,
                    )
                    .await
                    .unwrap();
                fs::write(
                    existing.managed_root.join("keep.txt"),
                    b"concurrent content",
                )
                .unwrap();
                let before = snapshot(&base);
                let (status,body) = post(&app,"/api/sync/restores",serde_json::json!({"operationUuid":operation,"preflightToken":token,"items":items})).await;
                assert_eq!(status, StatusCode::CONFLICT, "{body}");
                assert_eq!(body["code"], "library_already_present");
                assert_eq!(snapshot(&base), before);
                let error = manager
                    .registry()
                    .claim_restore_and_record_plans(
                        token,
                        &operation,
                        &request(&items),
                        &plan_inputs(&manager, &items),
                    )
                    .unwrap_err();
                assert_eq!(error.to_string(), "library-already-present");
                assert_eq!(snapshot(&base), before);
                assert_eq!(count(&base, "sync_restore_preflights"), 1);
                assert_eq!(count(&base, "sync_orchestration_operations"), 0);
                assert_eq!(count(&base, "sync_restore_root_plans"), 0);
                assert!(!base.join(format!("shell-{}", ids[0])).exists());
                drop(app);
                drop(manager);
                fs::remove_dir_all(base).unwrap();
            }
        }

        #[tokio::test]
        async fn exact_success_replay_precedes_admission_and_mismatch_preserves_all_state() {
            for root_aware in [false, true] {
                let (base, manager, app) = fixture();
                let id = uuid::Uuid::now_v7().to_string();
                discover(&manager, &id);
                let items = vec![item(&manager, &base, &id, root_aware)];
                let (_, preflight) = post(
                    &app,
                    "/api/sync/restores/preflight",
                    serde_json::json!({"items":items}),
                )
                .await;
                assert_eq!(preflight["ok"], true, "{preflight}");
                let token = preflight["preflightToken"].as_str().unwrap();
                let operation = uuid::Uuid::now_v7().to_string();
                let start = serde_json::json!({"operationUuid":operation,"preflightToken":token,"items":items});
                let (status, result) = post(&app, "/api/sync/restores", start.clone()).await;
                assert_eq!(status, StatusCode::OK, "{result}");
                let restore = result["items"][0]["restoreUuid"].as_str().unwrap();
                let descriptor = manager.registry().library(&id).unwrap().unwrap();
                assert!(!descriptor.local_data_dir.join("catalog.sqlite").exists());
                assert_eq!(
                    crate::library_path::read_marker(&descriptor.managed_root)
                        .unwrap()
                        .creation_txn_uuid,
                    restore
                );
                if root_aware {
                    assert_eq!(count(&base, "local_root_bindings"), 1);
                }
                let before = snapshot(&base);
                let (status, replay) = post(&app, "/api/sync/restores", start.clone()).await;
                assert_eq!(status, StatusCode::OK, "{replay}");
                assert_eq!(replay["items"][0]["restoreUuid"], restore);
                assert_eq!(snapshot(&base), before);
                let (inserted, plans) = manager
                    .registry()
                    .claim_restore_and_record_plans(
                        token,
                        &operation,
                        &request(&items),
                        &plan_inputs(&manager, &items),
                    )
                    .unwrap();
                assert!(!inserted);
                assert_eq!(plans[0].restore_uuid, restore);
                assert_eq!(snapshot(&base), before);
                let mut changed = start;
                changed["items"][0][if root_aware {
                    "placementBase"
                } else {
                    "placementBase"
                }] = base.join("different").to_string_lossy().into_owned().into();
                let (status, error) = post(&app, "/api/sync/restores", changed).await;
                assert_eq!(status, StatusCode::CONFLICT, "{error}");
                assert_eq!(error["code"], "idempotency_key_reused");
                assert_eq!(snapshot(&base), before);
                let error = manager
                    .registry()
                    .claim_restore_and_record_plans(
                        token,
                        &operation,
                        &serde_json::json!({"different":true}),
                        &[],
                    )
                    .unwrap_err();
                assert_eq!(error.to_string(), "idempotency-key-reused");
                assert_eq!(snapshot(&base), before);
                manager.discard_restore_plan(restore).await.unwrap();
                assert!(manager.registry().library(&id).unwrap().is_none());
                assert!(!descriptor.managed_root.exists());
                assert!(!descriptor.local_data_dir.exists());
                assert_eq!(count(&base, "local_root_bindings"), 0);
                drop(app);
                drop(manager);
                fs::remove_dir_all(base).unwrap();
            }
        }

        #[tokio::test]
        async fn compensation_rejects_unowned_library_even_when_uuid_and_path_match() {
            for with_catalog in [false, true] {
                let (base, manager, app) = fixture();
                let id = uuid::Uuid::now_v7().to_string();
                discover(&manager, &id);
                let items = vec![item(&manager, &base, &id, false)];
                let (_, preflight) = post(
                    &app,
                    "/api/sync/restores/preflight",
                    serde_json::json!({"items":items}),
                )
                .await;
                let inputs = plan_inputs(&manager, &items);
                let (_, plans) = manager
                    .registry()
                    .claim_restore_and_record_plans(
                        preflight["preflightToken"].as_str().unwrap(),
                        &uuid::Uuid::now_v7().to_string(),
                        &request(&items),
                        &inputs,
                    )
                    .unwrap();
                let restore = &plans[0].restore_uuid;
                let mut existing = crate::host_registry::LibraryDescriptor {
                    library_uuid: id.clone(),
                    root_uuid: uuid::Uuid::now_v7().to_string(),
                    name: "Existing".into(),
                    icon: None,
                    color: None,
                    managed_root: PathBuf::from(&plans[0].database_path),
                    local_data_dir: PathBuf::from(&plans[0].database_path),
                    lifecycle_state: "ready".into(),
                    binding_state: "local-only".into(),
                    availability: "available".into(),
                    marker_identity: String::new(),
                };
                let other_txn = uuid::Uuid::now_v7().to_string();
                manager
                    .registry()
                    .begin_creation(&crate::host_registry::CreationIntent {
                        txn_uuid: other_txn.clone(),
                        descriptor: existing.clone(),
                        requested_root: existing.managed_root.clone(),
                        staging_dir: existing.managed_root.join("staging"),
                    })
                    .unwrap();
                fs::create_dir_all(&existing.managed_root).unwrap();
                fs::write(
                    existing.managed_root.join(MARKER_FILE_NAME),
                    serde_json::to_vec(&crate::library_path::LibraryMarker {
                        format_version: 2,
                        library_uuid: id.clone(),
                        root_uuid: existing.root_uuid.clone(),
                        state: crate::library_path::MarkerState::Committed,
                        creation_txn_uuid: other_txn.clone(),
                    })
                    .unwrap(),
                )
                .unwrap();
                existing.marker_identity = crate::library_path::marker_identity(
                    &existing.managed_root.join(MARKER_FILE_NAME),
                )
                .unwrap();
                manager
                    .registry()
                    .commit_creation(&other_txn, &existing)
                    .unwrap();
                if with_catalog {
                    let db =
                        Database::open(existing.local_data_dir.join("catalog.sqlite")).unwrap();
                    db.apply_migrations().unwrap();
                }
                fs::write(existing.managed_root.join("keep.txt"), b"existing content").unwrap();
                let before = snapshot(&base);
                assert_eq!(
                    manager
                        .create_planned_restore_library(
                            restore,
                            id.clone(),
                            "Remote fixture".into(),
                            inputs[0].managed_root.clone(),
                            ACCOUNT,
                        )
                        .await
                        .unwrap_err()
                        .to_string(),
                    "library-already-present"
                );
                assert_eq!(snapshot(&base), before);
                assert_eq!(
                    manager
                        .discard_restore_plan(restore)
                        .await
                        .unwrap_err()
                        .to_string(),
                    "restore-ownership-mismatch"
                );
                assert_eq!(snapshot(&base), before);
                assert_eq!(
                    manager
                        .registry()
                        .commit_cancelled_restore_cleanup(restore, &id)
                        .unwrap_err()
                        .to_string(),
                    "restore-ownership-mismatch"
                );
                assert_eq!(snapshot(&base), before);
                let mut conflicting = existing.clone();
                conflicting.root_uuid = uuid::Uuid::now_v7().to_string();
                assert_eq!(
                    manager
                        .registry()
                        .commit_creation(restore, &conflicting)
                        .unwrap_err()
                        .to_string(),
                    "library-already-present"
                );
                assert_eq!(snapshot(&base), before);
                assert!(existing.managed_root.join(MARKER_FILE_NAME).is_file());
                drop(app);
                drop(manager);
                fs::remove_dir_all(base).unwrap();
            }
        }

        #[tokio::test]
        async fn owned_compensation_preserves_preexisting_empty_destination() {
            let (base, manager, app) = fixture();
            let id = uuid::Uuid::now_v7().to_string();
            discover(&manager, &id);
            let items = vec![item(&manager, &base, &id, false)];
            let destination = PathBuf::from(items[0]["placementBase"].as_str().unwrap());
            fs::create_dir(&destination).unwrap();
            let identity = crate::library_path::SafeDirectory::open(&destination)
                .unwrap()
                .identity()
                .to_owned();
            let (_, preflight) = post(
                &app,
                "/api/sync/restores/preflight",
                serde_json::json!({"items":items}),
            )
            .await;
            let (status,result) = post(&app,"/api/sync/restores",serde_json::json!({"operationUuid":uuid::Uuid::now_v7().to_string(),"preflightToken":preflight["preflightToken"],"items":items})).await;
            assert_eq!(status, StatusCode::OK, "{result}");
            manager
                .discard_restore_plan(result["items"][0]["restoreUuid"].as_str().unwrap())
                .await
                .unwrap();
            assert!(destination.is_dir());
            assert!(fs::read_dir(&destination).unwrap().next().is_none());
            assert_eq!(
                crate::library_path::SafeDirectory::open(&destination)
                    .unwrap()
                    .identity(),
                identity
            );
            assert_eq!(count(&base, "libraries"), 0);
            assert_eq!(count(&base, "library_sync_bindings"), 0);
            drop(app);
            drop(manager);
            fs::remove_dir_all(base).unwrap();
        }

        #[tokio::test]
        async fn uncreated_plan_compensation_cannot_touch_its_proposed_path() {
            let (base, manager, app) = fixture();
            let id = uuid::Uuid::now_v7().to_string();
            discover(&manager, &id);
            let items = vec![item(&manager, &base, &id, false)];
            let (_, preflight) = post(
                &app,
                "/api/sync/restores/preflight",
                serde_json::json!({"items":items}),
            )
            .await;
            let inputs = plan_inputs(&manager, &items);
            let (_, plans) = manager
                .registry()
                .claim_restore_and_record_plans(
                    preflight["preflightToken"].as_str().unwrap(),
                    &uuid::Uuid::now_v7().to_string(),
                    &request(&items),
                    &inputs,
                )
                .unwrap();
            fs::create_dir(&inputs[0].managed_root).unwrap();
            fs::write(
                inputs[0].managed_root.join("keep.txt"),
                b"arrived after claim",
            )
            .unwrap();
            let before_files = snapshot(&base).1;
            let cancelled = manager
                .discard_restore_plan(&plans[0].restore_uuid)
                .await
                .unwrap();
            assert_eq!(cancelled.state, "cancelled");
            assert_eq!(snapshot(&base).1, before_files);
            assert_eq!(count(&base, "libraries"), 0);
            assert_eq!(count(&base, "library_creation_journal"), 0);
            assert_eq!(count(&base, "library_sync_bindings"), 0);
            assert_eq!(count(&base, "local_root_bindings"), 0);
            drop(app);
            drop(manager);
            fs::remove_dir_all(base).unwrap();
        }

        #[tokio::test]
        async fn owned_shell_refuses_cleanup_after_catalog_or_unexpected_local_content() {
            for catalog in [false, true] {
                let (base, manager, app) = fixture();
                let id = uuid::Uuid::now_v7().to_string();
                discover(&manager, &id);
                let items = vec![item(&manager, &base, &id, true)];
                let (_, preflight) = post(
                    &app,
                    "/api/sync/restores/preflight",
                    serde_json::json!({"items":items}),
                )
                .await;
                let (status,result) = post(&app,"/api/sync/restores",serde_json::json!({"operationUuid":uuid::Uuid::now_v7().to_string(),"preflightToken":preflight["preflightToken"],"items":items})).await;
                assert_eq!(status, StatusCode::OK, "{result}");
                let descriptor = manager.registry().library(&id).unwrap().unwrap();
                if catalog {
                    let db =
                        Database::open(descriptor.local_data_dir.join("catalog.sqlite")).unwrap();
                    db.apply_migrations().unwrap();
                } else {
                    fs::write(
                        descriptor.local_data_dir.join("keep.txt"),
                        b"unowned local content",
                    )
                    .unwrap();
                }
                let before = snapshot(&base);
                let error = manager
                    .discard_restore_plan(result["items"][0]["restoreUuid"].as_str().unwrap())
                    .await
                    .unwrap_err();
                assert_eq!(error.to_string(), "restore-action-not-allowed");
                assert_eq!(snapshot(&base), before);
                drop(app);
                drop(manager);
                fs::remove_dir_all(base).unwrap();
            }
        }

        #[tokio::test]
        async fn unsupported_restore_creation_journal_preserves_resources_without_new_cleanup_rights(
        ) {
            use crate::host_registry::{CreationIntent, LibraryDescriptor};
            use crate::library_path::{
                atomic_write_marker_guarded, LibraryMarker, MarkerState, SafeDirectory,
            };
            for committed in [false, true] {
                let (base, manager, app) = fixture();
                fs::write(base.join("keep.txt"), b"unrelated local content").unwrap();
                let id = uuid::Uuid::now_v7().to_string();
                discover(&manager, &id);
                let items = vec![item(&manager, &base, &id, false)];
                let (_, preflight) = post(
                    &app,
                    "/api/sync/restores/preflight",
                    serde_json::json!({"items":items}),
                )
                .await;
                let inputs = plan_inputs(&manager, &items);
                let (_, plans) = manager
                    .registry()
                    .claim_restore_and_record_plans(
                        preflight["preflightToken"].as_str().unwrap(),
                        &uuid::Uuid::now_v7().to_string(),
                        &request(&items),
                        &inputs,
                    )
                    .unwrap();
                let restore = &plans[0].restore_uuid;
                let root = uuid::Uuid::now_v7().to_string();
                let local = base.join("libraries").join(&id);
                let staging = base
                    .join("libraries")
                    .join(format!("{id}.staging.{restore}"));
                let intent = CreationIntent {
                    txn_uuid: restore.clone(),
                    requested_root: PathBuf::from(&plans[0].database_path),
                    staging_dir: staging.clone(),
                    descriptor: LibraryDescriptor {
                        library_uuid: id.clone(),
                        root_uuid: root.clone(),
                        name: "Remote fixture".into(),
                        icon: None,
                        color: None,
                        local_data_dir: local.clone(),
                        managed_root: PathBuf::from(&plans[0].database_path),
                        lifecycle_state: "ready".into(),
                        binding_state: "local-only".into(),
                        availability: "available".into(),
                        marker_identity: String::new(),
                    },
                };
                manager
                    .registry()
                    .begin_restore_creation(&intent, ACCOUNT)
                    .unwrap();
                fs::create_dir_all(&intent.descriptor.managed_root).unwrap();
                fs::create_dir_all(&staging).unwrap();
                let root_guard = SafeDirectory::open(&intent.descriptor.managed_root).unwrap();
                let staging_guard = SafeDirectory::open(&staging).unwrap();
                manager
                    .registry()
                    .update_creation_identities(
                        restore,
                        Some(root_guard.identity()),
                        Some(staging_guard.identity()),
                    )
                    .unwrap();
                let marker = LibraryMarker {
                    format_version: 1,
                    library_uuid: id.clone(),
                    root_uuid: root,
                    state: if committed {
                        MarkerState::Committed
                    } else {
                        MarkerState::Pending
                    },
                    creation_txn_uuid: restore.clone(),
                };
                let marker_identity = atomic_write_marker_guarded(&root_guard, &marker).unwrap();
                let marker_bytes =
                    fs::read(intent.descriptor.managed_root.join(MARKER_FILE_NAME)).unwrap();
                manager
                    .registry()
                    .update_creation_phase(
                        restore,
                        if committed {
                            "marker-committed"
                        } else {
                            "marker-pending"
                        },
                        true,
                        true,
                        Some(&marker_identity),
                        None,
                    )
                    .unwrap();
                drop(root_guard);
                drop(staging_guard);
                manager.restore_active().await.unwrap();
                assert!(manager.registry().library(&id).unwrap().is_none());
                assert!(intent.descriptor.managed_root.exists());
                assert_eq!(
                    fs::read(intent.descriptor.managed_root.join(MARKER_FILE_NAME)).unwrap(),
                    marker_bytes
                );
                assert!(staging.exists());
                assert!(!local.exists());
                assert_eq!(
                    fs::read(base.join("keep.txt")).unwrap(),
                    b"unrelated local content"
                );
                assert_eq!(
                    manager
                        .registry()
                        .restore_plan(restore)
                        .unwrap()
                        .unwrap()
                        .state,
                    "cancelled"
                );
                assert_eq!(
                    manager
                        .registry()
                        .pending_creation_recovery()
                        .unwrap()
                        .len(),
                    1
                );
                assert_eq!(count(&base, "library_sync_bindings"), 0);
                drop(app);
                drop(manager);
                fs::remove_dir_all(base).unwrap();
            }
        }
    }

    #[test]
    fn non_relocation_root_operation_is_not_advertised_as_cancellable() {
        let path = unique_temp_dir().join("host.sqlite");
        let registry = crate::host_registry::HostRegistry::open(&path).unwrap();
        let operation = crate::host_registry::RootOperationRecord {
            operation_uuid: "00000000-0000-7000-8000-000000000001".into(),
            library_uuid: "00000000-0000-7000-8000-000000000002".into(),
            root_uuid: "00000000-0000-7000-8000-000000000003".into(),
            kind: "re-enable".into(),
            state: "running".into(),
            phase: "reconciling".into(),
            request_hash: "a".repeat(64),
            request_json: "{}".into(),
            expected_operation_revision: 0,
            expected_binding_generation: Some(1),
            operation_revision: 1,
            cutover_committed: false,
            error_code: None,
            created_at: 1,
            updated_at: 2,
        };

        let json = super::root_operation_detail_json(&registry, &operation).unwrap();
        assert_eq!(json["canCancel"], false);
        assert!(json["cancellationBoundary"].is_null());
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
        let (database, worker_database) = test_database_pair(db_dir.join("mangoo.sqlite"));
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
        assert_eq!(body["items"][0]["fileCount"], 3);
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
        assert_eq!(body["items"][0]["fileCount"], 1);
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
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["name"], "clip.mp4");
        assert_eq!(items[1]["name"], "image.jpg");
        assert_eq!(items[2]["name"], "notes.txt");
        assert!(items[2]["kind"].is_null());
        assert_eq!(items[2]["presentation"]["thumbnail"], "unavailable");
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
            &state
                .root_reachability()
                .expect("active library reachability"),
            &[(root_id, real_path)],
        );
        let body = get_roots_json(&app).await;
        let item = find_root_json(&body, root_id);
        assert_eq!(item["reachable"], true);

        // Force the cache to report unreachable (simulating an offline probe)
        // and confirm `list_roots` overlays it onto the DB-default `true`.
        {
            let reachability = state
                .root_reachability()
                .expect("active reachability cache");
            let mut guard = reachability.lock().expect("lock reachability cache");
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
    async fn all_file_generic_http_is_visible_and_thumbnail_unavailable_without_jobs() {
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let (_, ids) = seed_media_files_in_one_root(&database, &["ordinary.bin"]);
        let file_id = ids[0];
        database
            .connection_for_fsops()
            .execute("DELETE FROM media WHERE file_id=?1", [file_id])
            .unwrap();
        database
            .connection_for_fsops()
            .execute("UPDATE files SET ext='.bin' WHERE id=?1", [file_id])
            .unwrap();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let state = AppState::new_with_task_queue(database, sender);
        let app = router(state.clone());
        for (path, expected) in [
            (format!("/api/media/{file_id}"), StatusCode::OK),
            (format!("/api/media/{file_id}/thumbnail"), StatusCode::OK),
            (
                format!("/api/media/{file_id}/preview"),
                StatusCode::NOT_FOUND,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{path}");
            let body = response_json(response).await;
            if path.ends_with("/thumbnail") {
                assert_eq!(body["state"], "unavailable");
            } else if expected == StatusCode::OK {
                assert!(body["kind"].is_null());
                assert_eq!(body["presentation"]["previewKind"], "static-thumbnail");
            }
        }
        assert!(receiver.try_recv().is_err());
        let tasks = state
            .writer()
            .unwrap()
            .write(|db| db.list_tasks())
            .await
            .unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn thumbnail_route_queues_missing_grid_320_row() {
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("mangoo.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let (root_id, file_ids) = seed_media_files_in_one_root(&database, &["a.jpg"]);
        let file_id = file_ids[0];
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let state = AppState::new_with_task_queue(database, sender);
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
            .writer()
            .expect("active library writer")
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

    /// Bytes for a corrupt camera RAW fixture. Rawler claims the extension but
    /// rejects the payload. That is the degradation path the route has to
    /// survive without turning a working original-byte 200 into a 500.
    const UNDECODABLE_SOURCE_BYTES: &[u8] =
        b"mangoo-test-raw-sensor-payload-no-decoder-handles-this";

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
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("mangoo.sqlite");
        let (mut database, worker_database) = test_database_pair(db_path.clone());
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().into_owned(),
                display_name: "Preview Harness Root".to_string(),
            })
            .expect("add root");
        // Initialize actual source identity and complete collection before
        // seeding decoded dimensions. An automatic upgrade scan would rightly
        // invalidate dimensions attached to the old synthetic mtime/key.
        let root = database.get_root(root_id).unwrap().unwrap();
        let scan = crate::scan::scan_root(&mut database, &root).expect("scan preview source");
        assert_eq!(scan.files_seen, 1);
        assert_eq!(scan.media_files_seen, 1);
        let file_id = database
            .connection_for_fsops()
            .query_row(
                "SELECT id FROM files WHERE root_id=?1 AND name=?2 AND status='active'",
                (root_id, name),
                |row| row.get::<_, i64>(0),
            )
            .expect("scanned source file");
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
        // transcode branch — and corrupt files or a missing ffmpeg can then
        // fail generation. The no-viewport request must remain byte-identical.
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
                .get("x-mangoo-preview-tier")
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
                .get("x-mangoo-preview-tier")
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
        // transcode. Rawler claims it, then rejects this intentionally corrupt
        // fixture as a permanent decode failure.
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
                .get("x-mangoo-preview-tier")
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

        let mut failure = harness
            .database()
            .get_preview_failure(harness.file_id, PREVIEW_NATIVE_PROFILE)
            .expect("query failure")
            .expect("permanent decode failure is cached");
        assert_eq!(failure.failure_kind, "permanent");
        assert_eq!(failure.source_size, UNDECODABLE_SOURCE_BYTES.len() as i64);

        // A fresh failure suppresses immediate repeated decoder work.
        failure.error = "fresh preview failure sentinel".to_string();
        failure.updated_at = crate::db::unix_timestamp();
        harness
            .database()
            .upsert_preview_failure(failure)
            .expect("stamp fresh failure sentinel");
        let cached_response = harness
            .get(&format!(
                "/api/media/{}/preview?viewportLongPx=1366",
                harness.file_id
            ))
            .await;
        assert_eq!(cached_response.status(), StatusCode::OK);
        assert_eq!(
            harness
                .database()
                .get_preview_failure(harness.file_id, PREVIEW_NATIVE_PROFILE)
                .expect("query cached failure")
                .expect("failure remains")
                .error,
            "fresh preview failure sentinel"
        );

        // A stale failure must not poison an unchanged source forever. Retrying
        // this still-corrupt fixture overwrites the sentinel with the real
        // decoder failure.
        let mut stale_failure = harness
            .database()
            .get_preview_failure(harness.file_id, PREVIEW_NATIVE_PROFILE)
            .expect("query fresh failure")
            .expect("fresh failure remains");
        stale_failure.error = "stale preview failure sentinel".to_string();
        stale_failure.updated_at = crate::db::unix_timestamp() - 301;
        harness
            .database()
            .upsert_preview_failure(stale_failure)
            .expect("stamp stale failure sentinel");
        let stale_response = harness
            .get(&format!(
                "/api/media/{}/preview?viewportLongPx=1366",
                harness.file_id
            ))
            .await;
        assert_eq!(stale_response.status(), StatusCode::OK);
        let refreshed_after_expiry = harness
            .database()
            .get_preview_failure(harness.file_id, PREVIEW_NATIVE_PROFILE)
            .expect("query refreshed failure")
            .expect("retry still records the permanent decode failure");
        assert_ne!(
            refreshed_after_expiry.error, "stale preview failure sentinel",
            "expired negative-cache rows must be retried"
        );

        // A source fingerprint change invalidates the negative row and retries
        // generation. This fixture is still corrupt, so the retry stores a new
        // permanent failure and degrades to the new original bytes.
        let changed_source = b"changed but still undecodable camera raw";
        fs::write(harness.temp_root.join("camera.raw"), changed_source)
            .expect("replace undecodable source");
        let changed_response = harness
            .get(&format!(
                "/api/media/{}/preview?viewportLongPx=1366",
                harness.file_id
            ))
            .await;
        assert_eq!(changed_response.status(), StatusCode::OK);
        let changed_bytes = to_bytes(changed_response.into_body(), usize::MAX)
            .await
            .expect("read changed degraded preview body");
        assert_eq!(&changed_bytes[..], changed_source);
        let changed_failure = harness
            .database()
            .get_preview_failure(harness.file_id, PREVIEW_NATIVE_PROFILE)
            .expect("query refreshed failure")
            .expect("refreshed failure remains cached");
        assert_eq!(changed_failure.source_size, changed_source.len() as i64);
        assert!(changed_failure.updated_at >= refreshed_after_expiry.updated_at);
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
                .get("x-mangoo-preview-tier")
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
                .get("x-mangoo-preview-tier")
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
                .get("x-mangoo-preview-tier")
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
                .get("x-mangoo-preview-tier")
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
        assert_eq!(
            response.headers()[super::PREVIEW_TIER_HEADER],
            PREVIEW_2048_PROFILE
        );
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
                .get("x-mangoo-preview-tier")
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
        let state = AppState::new_with_task_queue(database, sender);
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
        let state = AppState::new_with_task_queue(database, sender);
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
        let db_path = db_dir.join("mangoo.sqlite");
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
            response.headers().get("x-mangoo-served-by").unwrap(),
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
        let db_path = db_dir.join("mangoo.sqlite");
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
        let db_path = db_dir.join("mangoo.sqlite");
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
        let db_path = db_dir.join("mangoo.sqlite");
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
        let db_path = db_dir.join("mangoo.sqlite");
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
            .root_reachability()
            .expect("active reachability cache")
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
            .root_reachability()
            .expect("active reachability cache")
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
        let db_path = db_dir.join("mangoo.sqlite");
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
            .root_reachability()
            .expect("active reachability cache")
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
        let db_path = db_dir.join("mangoo.sqlite");
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
        let db_path = db_dir.join("mangoo.sqlite");
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
        let db_path = db_dir.join("mangoo.sqlite");
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
        let state = AppState::new_with_task_queue(database, sender);
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
        let (database, worker_database) = test_database_pair(db_dir.join("mangoo.sqlite"));
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
            Database::open(db_dir.join("mangoo.sqlite")).expect("open notification database");
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
        let state = AppState::new_with_task_queue(database, sender);
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
        let (database, worker_database) = test_database_pair(db_dir.join("mangoo.sqlite"));
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
        let db_path = db_dir.join("mangoo.sqlite");

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
        fs::write(temp_root.join("notes.txt"), b"not media").expect("write ordinary file");
        let db_dir = unique_temp_dir();
        fs::create_dir_all(&db_dir).expect("create db dir");
        let (database, worker_database) = test_database_pair(db_dir.join("mangoo.sqlite"));
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
        assert_eq!(task["filesSeen"], 3);
        assert_eq!(task["skippedFiles"], 0);

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
        let db_path = db_dir.join("mangoo.sqlite");

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
            .writer()
            .expect("active library writer")
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
        let state = AppState::new_with_task_queue(database, sender);
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
        let state = AppState::new_with_task_queue(database, sender);
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
        let (database, worker_database) = test_database_pair(db_dir.join("mangoo.sqlite"));
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
            .and_then(|task| task["id"].as_i64());
        // A fast worker may finish and self-delete the purge task before this
        // diagnostic list call. Presence means wait; absence already proves
        // the terminal state this test needs.
        if let Some(purge_task_id) = purge_task_id {
            wait_for_task_absent(&app, purge_task_id).await;
        }

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
        let db_path = db_dir.join("mangoo.sqlite");

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
        let state = AppState::new_with_task_queue(database, _sender);
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
            .writer()
            .expect("active library writer")
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
        let state = AppState::new_with_task_queue(database, sender);
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
            .writer()
            .expect("active library writer")
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
                    .header("x-mangoo-session", "wrong-session")
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
                    .header("x-mangoo-session", "test-session")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("correct token request");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn sync_wake_is_host_scoped_before_any_library_is_active() {
        let app = router(AppState::new(test_database()));
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/sync/wake?reason=foreground")
                    .body(Body::empty())
                    .expect("build sync wake request"),
            )
            .await
            .expect("wake sync before Library activation");

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn directory_preferences_routes_validate_context_offline_roots_and_stale_cursors() {
        let base = tempfile::tempdir().unwrap();
        let state = AppState::new_host(
            base.path().join("host.sqlite"),
            base.path().join("libraries"),
        )
        .await
        .unwrap();
        let manager = state.library_manager.clone().unwrap();
        // Provision the later switch target before any active Library workers.
        // This test exercises context fencing, not concurrent Library creation.
        let second = manager
            .create_library(
                "Second".into(),
                None,
                None,
                base.path().join("second"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;
        let first = manager
            .create_library(
                "First".into(),
                None,
                None,
                base.path().join("first"),
                None,
                true,
            )
            .await
            .unwrap()
            .0;
        let offline = base.path().join("offline");
        let root_path = offline.to_string_lossy().to_string();
        let sorting_root = base.path().join("sorting");
        // Populate before registering the root: any scan must observe the same
        // children as this fixture, independent of background worker timing.
        for name in ["a", "b", "c"] {
            fs::create_dir_all(sorting_root.join(name)).unwrap();
        }
        let sorting_root_path = sorting_root.to_string_lossy().to_string();
        let (root_id, sort_root_id, sentinel, child) = state
            .writer()
            .unwrap()
            .write(move |db| {
                let root_id = db.add_root(NewRoot {
                    path: root_path,
                    display_name: "Offline".into(),
                })?;
                let other = db.add_root(NewRoot {
                    path: sorting_root_path,
                    display_name: "Sorting fixture".into(),
                })?;
                let sentinel = db.upsert_folder(crate::db::FolderUpsert {
                    root_id: other,
                    parent_id: None,
                    name: String::new(),
                    path_hash: "sort-root".into(),
                    mtime: None,
                })?;
                let mut child = 0;
                for name in ["a", "b", "c"] {
                    child = db.upsert_folder(crate::db::FolderUpsert {
                        root_id: other,
                        parent_id: Some(sentinel),
                        name: name.into(),
                        path_hash: format!("sort-{name}"),
                        mtime: None,
                    })?;
                }
                Ok((root_id, other, sentinel, child))
            })
            .await
            .unwrap();
        // Reconcile the actual filesystem through the same writer path used by
        // background scans before asserting that paging has multiple children.
        let catalog_path = first.local_data_dir.join("catalog.sqlite");
        let scan_writer = state.writer().unwrap();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut worker_database = Database::open(&catalog_path)?;
            let root = worker_database.get_root(sort_root_id)?.unwrap();
            crate::scan::scan_root_with_options(
                &mut worker_database,
                &root,
                crate::scan::ScanOptions {
                    writer: Some(scan_writer),
                    ..crate::scan::ScanOptions::background()
                },
            )?;
            let folders = worker_database.list_folder_children(sentinel)?;
            assert_eq!(
                folders
                    .iter()
                    .map(|folder| folder.name.as_str())
                    .collect::<Vec<_>>(),
                ["a", "b", "c"],
                "real scan must retain the physical paging fixture"
            );
            assert_eq!(
                folders[2].id, child,
                "scan must preserve the fixture identity"
            );
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();
        state
            .root_reachability()
            .unwrap()
            .lock()
            .unwrap()
            .insert(root_id, false);
        let revision = manager.active_snapshot().context_revision;
        let app = router(state.clone()).layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::api::attach_library_context,
        ));
        let request = |method: Method, path: String, rev: Option<u64>, body: serde_json::Value| {
            let mut builder = Request::builder()
                .method(method)
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(rev) = rev {
                builder = builder.header("x-mangoo-context-revision", rev.to_string());
            }
            builder.body(Body::from(body.to_string())).unwrap()
        };
        let appearance = serde_json::json!({"icon":"archive","color":"#a1B2c3"});
        let descending = serde_json::json!({"order":"name_desc","recursive":true});
        for path in [
            format!("/api/roots/{root_id}/appearance"),
            format!("/api/roots/{root_id}/sort"),
            format!("/api/folders/{child}/sort"),
        ] {
            for rev in [None, Some(revision - 1)] {
                let response = app
                    .clone()
                    .oneshot(request(
                        Method::PATCH,
                        path.clone(),
                        rev,
                        descending.clone(),
                    ))
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::CONFLICT);
            }
        }
        let saved = app
            .clone()
            .oneshot(request(
                Method::PATCH,
                format!("/api/roots/{root_id}/appearance"),
                Some(revision),
                appearance.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(saved.status(), StatusCode::OK);
        let saved = response_json(saved).await;
        assert_eq!(saved["rootFolderId"], serde_json::Value::Null);
        assert_eq!(saved["reachable"], false);
        assert_eq!(saved["icon"], "archive");
        assert!(!offline.exists());
        for invalid in [
            serde_json::json!({"icon":"archive"}),
            serde_json::json!({"icon":null,"color":"#fff"}),
            serde_json::json!({"icon":"archive","color":"red"}),
        ] {
            let response = app
                .clone()
                .oneshot(request(
                    Method::PATCH,
                    format!("/api/roots/{root_id}/appearance"),
                    Some(revision),
                    invalid,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        for invalid in [
            serde_json::json!({"order":"mtime_desc","recursive":true}),
            serde_json::json!({"order":"name_desc"}),
            serde_json::json!({"order":"name_desc","recursive":true,"extra":1}),
        ] {
            let response = app
                .clone()
                .oneshot(request(
                    Method::PATCH,
                    format!("/api/roots/{root_id}/sort"),
                    Some(revision),
                    invalid,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        let changed = app
            .clone()
            .oneshot(request(
                Method::PATCH,
                format!("/api/roots/{root_id}/sort"),
                Some(revision),
                descending.clone(),
            ))
            .await
            .unwrap();
        let changed_status = changed.status();
        let changed = response_json(changed).await;
        assert_eq!(changed_status, StatusCode::OK, "sort response: {changed}");
        assert_eq!(changed["state"]["folderId"], serde_json::Value::Null);
        assert_eq!(changed["invalidateDescendants"], true);
        let reset = app
            .clone()
            .oneshot(request(
                Method::DELETE,
                format!("/api/roots/{root_id}/sort"),
                Some(revision),
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(reset.status(), StatusCode::OK);
        assert_eq!(
            response_json(reset).await["state"]["effectiveOrder"],
            "name_asc"
        );
        let sentinel_write = app
            .clone()
            .oneshot(request(
                Method::PATCH,
                format!("/api/folders/{sentinel}/sort"),
                Some(revision),
                descending.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(sentinel_write.status(), StatusCode::NOT_FOUND);
        let page = app
            .clone()
            .oneshot(request(
                Method::GET,
                format!("/api/folders/{sentinel}/children?limit=1"),
                None,
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        let page = response_json(page).await;
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["name"], "a");
        assert_eq!(page["sortState"]["effectiveOrder"], "name_asc");
        let cursor = page["nextCursor"].as_str().unwrap();
        let invalid = app
            .clone()
            .oneshot(request(
                Method::GET,
                format!("/api/folders/{child}/children?cursor={cursor}"),
                None,
                serde_json::Value::Null,
            ))
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response_json(invalid).await["code"], "stale_folder_cursor");
        manager.activate(&second.library_uuid).await.unwrap();
        let stale = app
            .oneshot(request(
                Method::PATCH,
                format!("/api/roots/{root_id}/sort"),
                Some(revision),
                descending,
            ))
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        manager.shutdown_sync_coordinator().await.unwrap();
        if let Some(runtime) = manager.active_snapshot().runtime {
            runtime.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn folder_appearance_route_validates_pair_and_pins_library_context() {
        let base = tempfile::tempdir().unwrap();
        let state = AppState::new_host(
            base.path().join("host.sqlite"),
            base.path().join("libraries"),
        )
        .await
        .unwrap();
        let manager = state.library_manager.clone().unwrap();
        let first = manager
            .create_library(
                "First".into(),
                None,
                None,
                base.path().join("first"),
                None,
                true,
            )
            .await
            .unwrap()
            .0;
        let content_path = state
            .writer()
            .unwrap()
            .write(|database| Ok(PathBuf::from(&database.list_roots()?[0].path)))
            .await
            .unwrap();
        fs::create_dir_all(content_path.join("child")).unwrap();
        let (sentinel, child) = state
            .writer()
            .unwrap()
            .write(|database| {
                let root = database.list_roots()?.remove(0);
                let sentinel = database.upsert_folder(crate::db::FolderUpsert {
                    root_id: root.id,
                    parent_id: None,
                    name: String::new(),
                    path_hash: "root".into(),
                    mtime: None,
                })?;
                let child = database.upsert_folder(crate::db::FolderUpsert {
                    root_id: root.id,
                    parent_id: Some(sentinel),
                    name: "child".into(),
                    path_hash: "child".into(),
                    mtime: None,
                })?;
                Ok((sentinel, child))
            })
            .await
            .unwrap();
        let revision = manager.active_snapshot().context_revision;
        let app = router(state.clone()).layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::api::attach_library_context,
        ));
        let request = |id: i64, rev: Option<u64>, body: serde_json::Value| {
            let mut builder = Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/folders/{id}/appearance"));
            if let Some(rev) = rev {
                builder = builder.header("x-mangoo-context-revision", rev.to_string());
            }
            builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let pair = serde_json::json!({"icon":"future-icon","color":"#a1B2c3"});
        for rev in [None, Some(revision - 1)] {
            assert_eq!(
                app.clone()
                    .oneshot(request(child, rev, pair.clone()))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::CONFLICT
            );
        }
        for invalid in [
            serde_json::json!({"icon":"camera"}),
            serde_json::json!({"icon":null,"color":"#fff"}),
            serde_json::json!({"icon":"bad key","color":"#fff"}),
            serde_json::json!({"icon":"x".repeat(65),"color":"#fff"}),
            serde_json::json!({"icon":"camera","color":"red"}),
            serde_json::json!({"icon":"camera","color":"#ffff"}),
            serde_json::json!({"icon":"camera","color":"#fff","extra":true}),
        ] {
            assert_eq!(
                app.clone()
                    .oneshot(request(child, Some(revision), invalid))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        for id in [sentinel, i64::MAX] {
            assert_eq!(
                app.clone()
                    .oneshot(request(id, Some(revision), pair.clone()))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
        let saved = app
            .clone()
            .oneshot(request(child, Some(revision), pair.clone()))
            .await
            .unwrap();
        assert_eq!(saved.status(), StatusCode::OK);
        let saved = response_json(saved).await;
        assert_eq!(saved["icon"], "future-icon");
        assert_eq!(saved["color"], "#a1B2c3");
        assert_eq!(fs::read_dir(content_path.join("child")).unwrap().count(), 0);
        let second = manager
            .create_library(
                "Second".into(),
                None,
                None,
                base.path().join("second"),
                None,
                true,
            )
            .await
            .unwrap()
            .0;
        assert_ne!(first.library_uuid, second.library_uuid);
        assert_eq!(
            app.clone()
                .oneshot(request(child, Some(revision), pair.clone()))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        manager.activate(&first.library_uuid).await.unwrap();
        let read = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/folders/{child}/ancestors"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);
        let body = response_json(read).await;
        let folder = body["items"].as_array().unwrap().last().unwrap();
        assert_eq!(folder["icon"], "future-icon");
        assert_eq!(folder["color"], "#a1B2c3");
        let reset = app
            .oneshot(request(
                child,
                Some(manager.active_snapshot().context_revision),
                serde_json::json!({"icon":"folder","color":"#ffffff"}),
            ))
            .await
            .unwrap();
        assert_eq!(reset.status(), StatusCode::OK);
        assert_eq!(response_json(reset).await["icon"], "folder");
        manager.shutdown_sync_coordinator().await.unwrap();
        if let Some(runtime) = manager.active_snapshot().runtime {
            runtime.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn safe_library_routes_use_revision_headers_and_non_destructive_registration_path() {
        let base = tempfile::tempdir().unwrap();
        let state = AppState::new_host(
            base.path().join("host.sqlite"),
            base.path().join("libraries"),
        )
        .await
        .unwrap();
        let manager = state.library_manager.clone().unwrap();
        let library = manager
            .create_library(
                "Retained".into(),
                None,
                None,
                base.path().join("managed"),
                None,
                true,
            )
            .await
            .unwrap()
            .0;
        let revision = manager.active_snapshot().context_revision;
        fs::write(library.managed_root.join("sentinel"), b"preserved").unwrap();
        let app = router(state.clone());
        let uri = format!("/api/libraries/{}/registration", library.library_uuid);
        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(&uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::CONFLICT);
        let stale = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(&uri)
                    .header("x-mangoo-context-revision", (revision - 1).to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        let forgotten = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(&uri)
                    .header("x-mangoo-context-revision", revision.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forgotten.status(), StatusCode::OK);
        let result: serde_json::Value =
            serde_json::from_slice(&to_bytes(forgotten.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert!(result["library"].is_null());
        assert_eq!(result["contextRevision"], revision + 1);
        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/libraries")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let list: serde_json::Value =
            serde_json::from_slice(&to_bytes(listed.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(list["items"], serde_json::json!([]));
        let opened = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/libraries/open")
                    .header("x-mangoo-context-revision", (revision + 1).to_string())
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"databasePath": library.managed_root}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(opened.status(), StatusCode::OK);
        let result: serde_json::Value =
            serde_json::from_slice(&to_bytes(opened.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(result["activated"], true);
        assert_eq!(result["library"]["libraryUuid"], library.library_uuid);
        assert_eq!(
            fs::read(library.managed_root.join("sentinel")).unwrap(),
            b"preserved"
        );
        assert!(library.local_data_dir.join("catalog.sqlite").is_file());
        // A closing requester cannot abandon shutdown handles. Host shutdown
        // waits for that detached operation before retiring its own workers.
        let runtime = manager.active_snapshot().runtime.unwrap();
        let cancellation = runtime.cancellation_token();
        let release = Arc::new(tokio::sync::Notify::new());
        let release_worker = Arc::clone(&release);
        let worker_cancellation = cancellation.clone();
        runtime.own_background_task(tokio::spawn(async move {
            worker_cancellation.cancelled().await;
            release_worker.notified().await;
        }));
        let request = Request::builder()
            .method(Method::DELETE)
            .uri(&uri)
            .header(
                "x-mangoo-context-revision",
                manager.active_snapshot().context_revision.to_string(),
            )
            .body(Body::empty())
            .unwrap();
        let requester = tokio::spawn(app.oneshot(request));
        tokio::time::timeout(std::time::Duration::from_secs(5), cancellation.cancelled())
            .await
            .unwrap();
        requester.abort();
        assert!(requester.await.unwrap_err().is_cancelled());
        let closing_manager = Arc::clone(&manager);
        let shutdown =
            tokio::spawn(async move { closing_manager.shutdown_sync_coordinator().await });
        tokio::task::yield_now().await;
        assert!(!shutdown.is_finished());
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(5), shutdown)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(manager.active_snapshot().library_uuid.is_none());
        assert!(!manager
            .registry()
            .is_library_listed(&library.library_uuid)
            .unwrap());
        assert!(runtime.cancellation_token().is_cancelled());
        assert_eq!(
            fs::read(library.managed_root.join("sentinel")).unwrap(),
            b"preserved"
        );
        assert_eq!(
            manager
                .open_library_with_revision(
                    library.managed_root,
                    manager.active_snapshot().context_revision
                )
                .await
                .unwrap_err()
                .to_string(),
            "host-shutting-down"
        );
    }

    #[tokio::test]
    async fn sync_folder_v2_contract_rejects_invalid_uuid_and_requires_rename_preflight() {
        let base = unique_temp_dir();
        std::fs::create_dir_all(&base).unwrap();
        let state = AppState::new_host(base.join("host.sqlite"), base.join("libraries"))
            .await
            .unwrap();
        let manager = state.library_manager.clone().unwrap();
        let created = manager
            .create_library(
                "Contract gate".to_string(),
                None,
                None,
                base.join("managed"),
                None,
                true,
            )
            .await
            .unwrap();
        let library_uuid = created.0.library_uuid;
        let context_revision = manager.active_snapshot().context_revision;
        let app = router(state);

        let invalid = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/libraries/not-a-uuid/sync-folders")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/libraries/{library_uuid}/sync-folders"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let body = axum::body::to_bytes(listed.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["items"].is_array());

        let stale_rename_heads = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/00000000-0000-7000-8000-000000000010/rename/commit"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-mangoo-context-revision", context_revision.to_string())
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": "00000000-0000-7000-8000-000000000011",
                            "expectedOperationRevision": 0,
                            "requestedName": "Photos",
                            "expectedLocationHeads": [],
                            "supersedes": []
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale_rename_heads.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(stale_rename_heads.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "invalid-input");

        let valid_rename_fence = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/00000000-0000-7000-8000-000000000010/rename/commit"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-mangoo-context-revision", context_revision.to_string())
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": "00000000-0000-7000-8000-000000000011",
                            "expectedOperationRevision": 0,
                            "requestedName": "Photos",
                            "expectedLocationHeads": ["00000000-0000-7000-8000-000000000012"],
                            "supersedes": ["00000000-0000-7000-8000-000000000012"],
                            "planUuid": "00000000-0000-7000-8000-000000000013"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(valid_rename_fence.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(valid_rename_fence.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "comparison-expired");

        let stale_context = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/00000000-0000-7000-8000-000000000010/commands/stop-sync"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-mangoo-context-revision", (context_revision + 1).to_string())
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": "00000000-0000-7000-8000-000000000014",
                            "expectedOperationRevision": 0,
                            "expectedBindingGeneration": 1
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale_context.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(stale_context.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "stale_library_context");

        let missing_binding_generation = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/00000000-0000-7000-8000-000000000010/commands/stop-sync"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-mangoo-context-revision", context_revision.to_string())
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": "00000000-0000-7000-8000-000000000015",
                            "expectedOperationRevision": 0
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_binding_generation.status(), StatusCode::BAD_REQUEST);

        let reenable_without_generation = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/00000000-0000-7000-8000-000000000010/reenable/preflight"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": "00000000-0000-7000-8000-000000000016",
                            "expectedOperationRevision": 0
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            reenable_without_generation.status(),
            StatusCode::BAD_REQUEST
        );

        let invalid_relocation_mode = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/00000000-0000-7000-8000-000000000010/relocations/preflight"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": "00000000-0000-7000-8000-000000000016",
                            "expectedOperationRevision": 0,
                            "expectedBindingGeneration": 1,
                            "mode": "copy",
                            "targetPath": "D:/Library/Photos"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_relocation_mode.status(), StatusCode::BAD_REQUEST);

        let unexpected_field = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/00000000-0000-7000-8000-000000000010/reenable/preflight"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": "00000000-0000-7000-8000-000000000016",
                            "expectedOperationRevision": 0,
                            "expectedBindingGeneration": 1,
                            "unexpected": true
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unexpected_field.status(), StatusCode::BAD_REQUEST);

        let missing = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/libraries/00000000-0000-7000-8000-000000000099/sync-folders")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn sync_folder_reads_do_not_schedule_work_and_refresh_deduplicates_placement() {
        let base = unique_temp_dir();
        fs::create_dir_all(&base).unwrap();
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        executor.block_on(async {
            let state = AppState::new_host(base.join("host.sqlite"), base.join("libraries"))
                .await
                .unwrap();
            let manager = state.library_manager.clone().unwrap();
            let library = manager
                .create_library(
                    "Read-only inventory".into(),
                    None,
                    None,
                    base.join("managed"),
                    None,
                    true,
                )
                .await
                .unwrap()
                .0;
            // Build the fixture before Host shutdown closes Library lifecycle
            // admission; the scheduler stays stopped for the read assertions.
            manager.shutdown_sync_coordinator().await.unwrap();
            let library_uuid = &library.library_uuid;
            let registry = manager.registry();
            registry
                .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                    plugin_id: "com.mangoo.read-fixture",
                    version: "1.0.0",
                    manifest_json: "{}",
                    entry_relative_path: "fake-provider",
                    entry_sha256: &[0; 32],
                    install_root: &base,
                    trust_policy_id: "test",
                    requested_permissions_json: "[]",
                })
                .unwrap();
            registry
                .upsert_plugin_account(
                    "read-fixture-account",
                    "com.mangoo.read-fixture",
                    None,
                    "authenticated",
                )
                .unwrap();
            registry
                .set_plugin_intent("com.mangoo.read-fixture", true, true)
                .unwrap();
            registry
                .record_plugin_runtime_ready(
                    "com.mangoo.read-fixture",
                    "test-session",
                    &["sync-provider.v1".into()],
                )
                .unwrap();
            registry
                .begin_sync_binding(
                    library_uuid,
                    "com.mangoo.read-fixture",
                    "read-fixture-account",
                )
                .unwrap();
            let root_uuid = uuid::Uuid::now_v7();
            let descriptor = crate::db::SyncRootDescriptorRecord {
                root_uuid: *root_uuid.as_bytes(),
                root_folder_uuid: *uuid::Uuid::now_v7().as_bytes(),
                library_root_uuid: *uuid::Uuid::parse_str(&library.root_uuid)
                    .unwrap()
                    .as_bytes(),
                creation_event_uuid: None,
                winning_location_revision_uuid: None,
                requested_name: "Photos".into(),
                materialized_name: "Photos".into(),
                materialization_fingerprint: [1; 32],
                lifecycle: "active".into(),
                logical_revision: 1,
                updated_at: 1,
            };
            let root_text = root_uuid.to_string();
            let runtime = state.resources().unwrap();
            let catalog = state.open_read_database().unwrap().unwrap();
            let app = router(state.clone());
            // Collection now starts for ordinary managed roots too. Observe its
            // completion before attributing any later work to inventory reads.
            for task in catalog.list_tasks().unwrap() {
                assert_eq!(task.kind, "root_scan");
                wait_for_task_status(&app, task.id, "succeeded").await;
            }
            let startup_notifications = catalog.list_notification_outbox(0, 100).unwrap();
            let sync_status = || {
                let mut status =
                    serde_json::to_value(registry.sync_status_snapshot().unwrap()).unwrap();
                status.as_object_mut().unwrap().remove("generatedAt");
                status
            };
            for revision in [1, 2] {
                let mut descriptor = descriptor.clone();
                descriptor.logical_revision = revision;
                descriptor.materialization_fingerprint = [revision as u8; 32];
                runtime
                    .writer
                    .write(move |database| database.upsert_sync_root_descriptor(&descriptor))
                    .await
                    .unwrap();
                // A real authoritative-commit hook would reset this future poll
                // and increment the Host status revision, even with no scheduler.
                registry
                    .record_sync_turn_success(library_uuid, true)
                    .unwrap();
                let before = sync_status();
                assert!(before["libraries"][0]["pollAfter"].as_i64().unwrap() > 0);
                let inventory = registry.remote_sync_roots(library_uuid).unwrap();
                let notifications = catalog.list_notification_outbox(0, 100).unwrap();
                let mut suffixes = vec![String::new(), "/settings".into()];
                if revision > 1 {
                    suffixes.push(format!("/{root_uuid}"));
                }
                for _ in 0..2 {
                    for suffix in &suffixes {
                        let response = app
                            .clone()
                            .oneshot(
                                Request::builder()
                                    .uri(format!(
                                        "/api/libraries/{library_uuid}/sync-folders{suffix}"
                                    ))
                                    .body(Body::empty())
                                    .unwrap(),
                            )
                            .await
                            .unwrap();
                        assert_eq!(response.status(), StatusCode::OK);
                        let response = response_json(response).await;
                        if suffix.is_empty() {
                            assert_eq!(
                                response["items"].as_array().unwrap().len(),
                                inventory.len()
                            );
                        }
                    }
                }
                assert_eq!(
                    sync_status(),
                    before,
                    "GET must not invoke the active writer's authoritative-commit wake"
                );
                assert_eq!(registry.remote_sync_roots(library_uuid).unwrap(), inventory);
                assert!(registry
                    .local_root_bindings(library_uuid)
                    .unwrap()
                    .is_empty());
                assert!(registry
                    .root_operations(library_uuid, &root_text, None, 100, "all")
                    .unwrap()
                    .is_empty());
                assert_eq!(
                    catalog.list_notification_outbox(0, 100).unwrap(),
                    notifications
                );
                assert_eq!(
                    manager
                        .refresh_sync_folder_inventory(library_uuid)
                        .await
                        .unwrap(),
                    vec![root_text.clone()]
                );
                let published = catalog.list_notification_outbox(0, 100).unwrap();
                assert_eq!(
                    published.len(),
                    startup_notifications.len() + revision as usize
                );
                assert_eq!(
                    &published[..startup_notifications.len()],
                    startup_notifications.as_slice()
                );
                let notification = &published.last().unwrap().operation;
                assert_eq!(
                    notification.library_uuid.as_deref(),
                    Some(library_uuid.as_str())
                );
                assert_eq!(notification.root_uuid.as_deref(), Some(root_text.as_str()));
                assert_eq!(
                    notification.operation_id,
                    format!("sync-folder-placement:{library_uuid}:{root_uuid}:{revision}")
                );
                assert!(manager
                    .refresh_sync_folder_inventory(library_uuid)
                    .await
                    .unwrap()
                    .is_empty());
                assert_eq!(catalog.list_notification_outbox(0, 100).unwrap(), published);
            }
            drop(catalog);
            runtime.shutdown().await.unwrap();
            drop(app);
            drop(state);
            drop(manager);
            drop(runtime);
        });
        // Read handlers cache SQLite connections on blocking-pool threads.
        // Join those threads before removing the fixture files on Windows.
        drop(executor);
        fs::remove_dir_all(base).unwrap();
    }

    enum EnrollmentRecoveryBoundary {
        TemporaryMarker,
        PublishedMarker,
        HostBinding,
    }

    #[tokio::test]
    async fn enrollment_replays_a_catalog_owned_existing_root_after_marker_publish_failure() {
        verify_catalog_owned_enrollment_recovery(EnrollmentRecoveryBoundary::TemporaryMarker).await;
    }

    #[tokio::test]
    async fn enrollment_recovers_a_published_marker_before_host_binding_commit() {
        verify_catalog_owned_enrollment_recovery(EnrollmentRecoveryBoundary::PublishedMarker).await;
    }

    #[tokio::test]
    async fn enrollment_activation_recovers_host_binding_before_catalog_projection() {
        verify_catalog_owned_enrollment_recovery(EnrollmentRecoveryBoundary::HostBinding).await;
    }

    async fn verify_catalog_owned_enrollment_recovery(boundary: EnrollmentRecoveryBoundary) {
        let base = unique_temp_dir();
        let existing_root = base.join("existing-root");
        fs::create_dir_all(&existing_root).expect("create existing browse root");
        fs::write(existing_root.join("photo.jpg"), b"existing media").expect("seed existing media");
        let host_path = base.join("host.sqlite");
        let state = AppState::new_host(host_path.clone(), base.join("libraries"))
            .await
            .expect("create Host state");
        let manager = state.library_manager.clone().expect("library manager");
        let created = manager
            .create_library(
                "Enrollment replay".to_string(),
                None,
                None,
                base.join("managed"),
                None,
                true,
            )
            .await
            .expect("create library");
        let library_uuid = created.0.library_uuid;
        let mut context_revision = manager.active_snapshot().context_revision;
        let app = router(state.clone());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/roots")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "path": existing_root.to_string_lossy(),
                            "displayName": "Existing Root"
                        })
                        .to_string(),
                    ))
                    .expect("build add-root request"),
            )
            .await
            .expect("add browse root");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let task_id = response_json(response).await["taskId"]
            .as_i64()
            .expect("scan task id");
        wait_for_task_status(&app, task_id, "succeeded").await;

        // This fixture repeatedly changes marker identities to exercise API
        // validation. Mismatched markers are ordinary user files and the live
        // watcher legitimately pins them while fingerprinting. Join it before
        // fault injection; later activation starts a fresh runtime normally.
        manager
            .active_snapshot()
            .runtime
            .unwrap()
            .stop_watcher_for_marker_fixture()
            .await
            .unwrap();

        let foreign_library_uuid = manager
            .create_library(
                "Foreign catalog owner".to_string(),
                None,
                None,
                base.join("foreign-managed"),
                None,
                false,
            )
            .await
            .expect("create foreign library")
            .0
            .library_uuid;
        let catalog_only_root = base.join("catalog-only-root");
        fs::create_dir_all(&catalog_only_root).expect("create Catalog-only root");
        let foreign_root_id = manager
            .write_sync_folder_catalog(&foreign_library_uuid, {
                let catalog_only_root = catalog_only_root.clone();
                move |database| {
                    let root_id = database.add_root(crate::db::NewRoot {
                        path: fs::canonicalize(&catalog_only_root)?
                            .to_string_lossy()
                            .into_owned(),
                        display_name: "Catalog-only Root".to_string(),
                    })?;
                    database.upsert_folder(crate::db::FolderUpsert {
                        root_id,
                        parent_id: None,
                        name: String::new(),
                        path_hash: "foreign-root-sentinel".to_string(),
                        mtime: Some(1),
                    })?;
                    Ok(root_id)
                }
            })
            .await
            .expect("seed foreign Catalog ownership");
        assert!(foreign_root_id > 0);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/enrollment/preflight"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": uuid::Uuid::now_v7().to_string(),
                            "expectedOperationRevision": 0,
                            "rootUuid": null,
                            "mode": "enroll-existing",
                            "path": catalog_only_root.to_string_lossy()
                        })
                        .to_string(),
                    ))
                    .expect("build cross-library Catalog preflight request"),
            )
            .await
            .expect("reject Catalog-owned path from another Library");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = response_json(response).await;
        assert_eq!(response["code"], "path-occupied", "{response}");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/enrollment/preflight"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": uuid::Uuid::now_v7().to_string(),
                            "expectedOperationRevision": 0,
                            "rootUuid": uuid::Uuid::now_v7().to_string(),
                            "mode": "enroll-existing",
                            "path": existing_root.to_string_lossy()
                        })
                        .to_string(),
                    ))
                    .expect("build mismatched root preflight request"),
            )
            .await
            .expect("reject mismatched existing root identity");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = response_json(response).await;
        assert_eq!(response["code"], "path-occupied", "{response}");

        let original_operation_uuid = uuid::Uuid::now_v7().to_string();
        let preflight_request = serde_json::json!({
            "operationUuid": original_operation_uuid,
            "expectedOperationRevision": 0,
            "rootUuid": null,
            "mode": "enroll-existing",
            "path": existing_root.to_string_lossy()
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/enrollment/preflight"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(preflight_request.to_string()))
                    .expect("build enrollment preflight request"),
            )
            .await
            .expect("preflight enrollment");
        assert_eq!(response.status(), StatusCode::OK);
        let preflight = response_json(response).await;
        let root_uuid = preflight["rootUuid"].as_str().expect("root uuid");
        let binding_uuid = preflight["bindingUuid"].as_str().expect("binding uuid");
        let normalized_path = preflight["normalizedPath"]
            .as_str()
            .expect("normalized path");
        let (_, root_folder_uuid) = manager
            .sync_root_identity_for_path(&library_uuid, normalized_path.to_owned())
            .await
            .expect("read existing root identity")
            .expect("existing root identity");
        let requested_name = existing_root
            .file_name()
            .and_then(|name| name.to_str())
            .expect("root name");
        #[cfg(windows)]
        let equivalent_commit_path = {
            let mut path = normalized_path.to_owned();
            path.replace_range(..1, &path[..1].to_ascii_lowercase());
            path
        };
        #[cfg(not(windows))]
        let equivalent_commit_path = normalized_path.to_owned();
        let commit_body = serde_json::json!({
            "operationUuid": original_operation_uuid,
            "expectedOperationRevision": 0,
            "rootUuid": root_uuid,
            "rootFolderUuid": uuid::Uuid::now_v7().to_string(),
            "preflightToken": original_operation_uuid,
            "path": equivalent_commit_path,
            "requestedName": requested_name
        });
        let materialized = crate::sync_folders::default_materialized_name(
            requested_name,
            uuid::Uuid::parse_str(root_uuid).expect("valid root uuid"),
            &std::collections::BTreeSet::new(),
        );
        let fingerprint = crate::sync_folders::materialization_fingerprint(&materialized);
        manager
            .registry()
            .upsert_remote_sync_root(
                &library_uuid,
                root_uuid,
                &root_folder_uuid.to_string(),
                requested_name,
                &materialized,
                &fingerprint,
                0,
                "active",
                None,
                None,
            )
            .expect("stage remote root");
        let request_hash = crate::sync_folders::request_hash(&commit_body).unwrap();
        manager
            .registry()
            .begin_root_operation_once(
                &original_operation_uuid,
                &library_uuid,
                root_uuid,
                "enroll",
                "preflighting",
                &request_hash,
                &commit_body,
                0,
                None,
            )
            .expect("begin enrollment");
        let evidence: crate::sync_folders::ValidatedRootPath =
            serde_json::from_value(preflight["pathEvidence"].clone()).expect("path evidence");
        let plan = serde_json::json!({
            "preflightToken": original_operation_uuid,
            "pathEvidence": evidence
        });
        manager
            .registry()
            .set_root_operation_plan(
                &original_operation_uuid,
                &original_operation_uuid,
                &plan,
                "capturing-seed",
            )
            .expect("set enrollment plan");
        fs::write(
            existing_root.join("arrived-after-preflight.jpg"),
            b"late media",
        )
        .expect("add media while enrollment waits");
        let marker = crate::sync_folders::SyncRootMarker {
            format_version: 1,
            library_uuid: library_uuid.clone(),
            root_uuid: root_uuid.to_owned(),
            root_folder_uuid: root_folder_uuid.to_string(),
            local_binding_uuid: binding_uuid.to_owned(),
            binding_generation: 1,
        };
        let unrelated_temporary_marker = existing_root.join(format!(
            "{}.{}.tmp",
            crate::sync_folders::ROOT_MARKER_FILE_NAME,
            uuid::Uuid::now_v7()
        ));
        fs::write(
            &unrelated_temporary_marker,
            serde_json::to_vec_pretty(&marker).expect("serialize unrelated marker"),
        )
        .expect("leave unrelated temporary marker");
        let interrupted_temporary_marker = existing_root.join(format!(
            "{}.{}.tmp",
            crate::sync_folders::ROOT_MARKER_FILE_NAME,
            original_operation_uuid
        ));
        let mismatched_marker = crate::sync_folders::SyncRootMarker {
            root_folder_uuid: uuid::Uuid::now_v7().to_string(),
            ..marker.clone()
        };
        fs::write(
            &interrupted_temporary_marker,
            serde_json::to_vec_pretty(&mismatched_marker)
                .expect("serialize mismatched operation marker"),
        )
        .expect("leave mismatched interrupted temporary marker");
        let interrupted_revision = manager
            .registry()
            .root_operation(&original_operation_uuid)
            .unwrap()
            .expect("interrupted enrollment operation")
            .operation_revision;
        rusqlite::Connection::open(&host_path)
            .expect("open Host database")
            .execute(
                "UPDATE root_path_preflights SET expires_at=0 WHERE preflight_token=?1",
                [&original_operation_uuid],
            )
            .expect("expire interrupted preflight");

        let replacement_operation_uuid = uuid::Uuid::now_v7().to_string();
        let replacement_preflight = serde_json::json!({
            "operationUuid": replacement_operation_uuid,
            "expectedOperationRevision": 0,
            "rootUuid": null,
            "mode": "enroll-existing",
            "path": existing_root.to_string_lossy()
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/enrollment/preflight"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(replacement_preflight.to_string()))
                    .expect("build replacement preflight request"),
            )
            .await
            .expect("resume staged preflight");
        assert_eq!(response.status(), StatusCode::OK);
        let resumed_preflight = response_json(response).await;
        assert_eq!(resumed_preflight["preflightToken"], original_operation_uuid);
        assert_eq!(resumed_preflight["bindingUuid"], binding_uuid);
        assert_eq!(
            manager
                .registry()
                .root_operation(&original_operation_uuid)
                .unwrap()
                .expect("unchanged interrupted enrollment operation")
                .operation_revision,
            interrupted_revision
        );
        let replacement_commit = serde_json::json!({
            "operationUuid": replacement_operation_uuid,
            "expectedOperationRevision": 0,
            "rootUuid": root_uuid,
            "rootFolderUuid": uuid::Uuid::now_v7().to_string(),
            "preflightToken": resumed_preflight["preflightToken"],
            "path": resumed_preflight["normalizedPath"],
            "requestedName": requested_name
        });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/enrollment/commit"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-mangoo-context-revision", context_revision.to_string())
                    .body(Body::from(replacement_commit.to_string()))
                    .expect("build replacement enrollment commit"),
            )
            .await
            .expect("reject mismatched staged marker");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = response_json(response).await;
        assert_eq!(response["code"], "path-occupied", "{response}");
        assert!(!existing_root
            .join(crate::sync_folders::ROOT_MARKER_FILE_NAME)
            .exists());
        assert!(manager
            .registry()
            .local_root_binding(&library_uuid, root_uuid)
            .unwrap()
            .is_none());
        fs::write(
            &interrupted_temporary_marker,
            serde_json::to_vec_pretty(&marker).expect("serialize operation marker"),
        )
        .expect("repair interrupted temporary marker");

        if matches!(
            boundary,
            EnrollmentRecoveryBoundary::PublishedMarker | EnrollmentRecoveryBoundary::HostBinding
        ) {
            crate::sync_folders::write_root_marker_for_operation(
                &existing_root,
                &marker,
                uuid::Uuid::parse_str(&original_operation_uuid).unwrap(),
                evidence.filesystem_identity.as_deref().unwrap(),
            )
            .expect("publish marker before the simulated Host commit interruption");
            assert!(manager
                .registry()
                .local_root_binding(&library_uuid, root_uuid)
                .unwrap()
                .is_none());
            let final_path = existing_root.join(crate::sync_folders::ROOT_MARKER_FILE_NAME);
            let final_identity = crate::library_path::local_file_identity(&final_path).unwrap();
            for field in [
                "libraryUuid",
                "rootUuid",
                "rootFolderUuid",
                "localBindingUuid",
                "bindingGeneration",
            ] {
                let mut foreign_marker = serde_json::to_value(&marker).unwrap();
                foreign_marker[field] = if field == "bindingGeneration" {
                    serde_json::json!(2)
                } else {
                    serde_json::json!(uuid::Uuid::now_v7().to_string())
                };
                let foreign_bytes = serde_json::to_vec_pretty(&foreign_marker).unwrap();
                fs::write(&final_path, &foreign_bytes).unwrap();
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(Method::POST)
                            .uri(format!(
                                "/api/libraries/{library_uuid}/sync-folders/enrollment/commit"
                            ))
                            .header(header::CONTENT_TYPE, "application/json")
                            .header("x-mangoo-context-revision", context_revision.to_string())
                            .body(Body::from(replacement_commit.to_string()))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::CONFLICT, "{field}");
                assert!(manager
                    .registry()
                    .local_root_binding(&library_uuid, root_uuid)
                    .unwrap()
                    .is_none());
                assert_eq!(fs::read(&final_path).unwrap(), foreign_bytes, "{field}");
            }
            fs::write(&final_path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();
            assert_eq!(
                crate::library_path::local_file_identity(&final_path).unwrap(),
                final_identity
            );
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!(
                            "/api/libraries/{library_uuid}/sync-folders/enrollment/preflight"
                        ))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(replacement_preflight.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let resumed = response_json(response).await;
            assert_eq!(resumed["preflightToken"], original_operation_uuid);
            assert_eq!(resumed["bindingUuid"], binding_uuid);
        }

        if matches!(boundary, EnrollmentRecoveryBoundary::HostBinding) {
            let projected_root_uuid = uuid::Uuid::parse_str(root_uuid).unwrap();
            manager
                .write_sync_folder_catalog(&library_uuid, move |database| {
                    assert!(database
                        .sync_root_descriptor(projected_root_uuid.as_bytes())?
                        .is_none());
                    assert!(database
                        .sync_root_local_binding(projected_root_uuid.as_bytes())?
                        .is_none());
                    Ok(())
                })
                .await
                .unwrap();
            manager
                .registry()
                .commit_local_root_binding(
                    &original_operation_uuid,
                    &original_operation_uuid,
                    &crate::host_registry::NewLocalRootBinding {
                        library_uuid: library_uuid.clone(),
                        root_uuid: root_uuid.to_owned(),
                        local_binding_uuid: binding_uuid.to_owned(),
                        local_path: equivalent_commit_path.clone(),
                        canonical_path_key: evidence.canonical_path_key.clone(),
                        placement_parent: evidence.placement_parent.clone(),
                        placement_parent_key: evidence.placement_parent_key.clone(),
                        marker_identity: crate::library_path::local_file_identity(
                            &existing_root.join(crate::sync_folders::ROOT_MARKER_FILE_NAME),
                        )
                        .unwrap(),
                        filesystem_identity: evidence.filesystem_identity.clone(),
                        state: "active".into(),
                        binding_generation: 1,
                        root_capture_epoch: Some(uuid::Uuid::now_v7().to_string()),
                        root_actor_epoch: Some(uuid::Uuid::now_v7().to_string()),
                        desired_materialized_name: materialized.clone(),
                        desired_materialization_fingerprint: fingerprint.clone(),
                        applied_materialized_name: Some(materialized.clone()),
                        applied_materialization_fingerprint: Some(fingerprint.clone()),
                        operation_revision: 1,
                        verification_json: plan.clone(),
                    },
                )
                .expect("commit Host binding before the simulated Catalog interruption");
            manager.activate(&foreign_library_uuid).await.unwrap();
            manager
                .activate(&library_uuid)
                .await
                .expect("activation must finish the inherited Catalog root projection");
            context_revision = manager.active_snapshot().context_revision;
            manager
                .write_sync_folder_catalog(&library_uuid, move |database| {
                    let descriptor = database
                        .sync_root_descriptor(projected_root_uuid.as_bytes())?
                        .expect("recovered descriptor");
                    assert_eq!(descriptor.root_folder_uuid, *root_folder_uuid.as_bytes());
                    assert!(database
                        .sync_root_local_binding(projected_root_uuid.as_bytes())?
                        .is_some());
                    Ok(())
                })
                .await
                .unwrap();
        }

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/enrollment/commit"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-mangoo-context-revision", context_revision.to_string())
                    .body(Body::from(replacement_commit.to_string()))
                    .expect("build corrected replacement enrollment commit"),
            )
            .await
            .expect("complete staged enrollment with matching marker");
        let response_status = response.status();
        let completed = response_json(response).await;
        assert_eq!(response_status, StatusCode::ACCEPTED, "{completed}");
        assert_eq!(completed["operationUuid"], original_operation_uuid);
        assert_eq!(completed["state"], "completed");
        assert_eq!(
            crate::sync_folders::read_root_marker(&existing_root).unwrap(),
            marker
        );
        assert!(!interrupted_temporary_marker.exists());
        assert!(
            unrelated_temporary_marker.exists(),
            "recovery must not adopt or clean an unrelated operation's marker"
        );

        let binding = manager
            .registry()
            .local_root_binding(&library_uuid, root_uuid)
            .unwrap()
            .expect("read completed binding");
        assert_eq!(binding.state, "active");
        assert_eq!(
            binding.applied_materialized_name.as_deref(),
            Some(materialized.as_str())
        );
        assert_eq!(
            binding.applied_materialization_fingerprint.as_deref(),
            Some(fingerprint.as_str())
        );
        assert!(manager
            .registry()
            .pending_automatic_root_renames(&library_uuid)
            .unwrap()
            .is_empty());
        let tasks = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/tasks")
                    .body(Body::empty())
                    .expect("build task list request"),
            )
            .await
            .expect("list tasks after enrollment");
        assert_eq!(tasks.status(), StatusCode::OK);
        let tasks = response_json(tasks).await;
        assert!(
            tasks["items"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .filter(|task| task["kind"] == "root_scan")
                    .count()
                    >= 2
                    && items.iter().any(|task| {
                        task["kind"] == "root_scan"
                            && matches!(
                                task["status"].as_str(),
                                Some("pending" | "running" | "succeeded")
                            )
                    })
            }),
            "{tasks}"
        );
        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/libraries/{library_uuid}/sync-folders"))
                    .body(Body::empty())
                    .expect("build synchronized-root list request"),
            )
            .await
            .expect("list synchronized roots");
        assert_eq!(listed.status(), StatusCode::OK);
        let listed = response_json(listed).await;
        assert!(
            listed["items"].as_array().is_some_and(|items| {
                items.iter().any(|item| {
                    item["descriptor"]["rootUuid"] == root_uuid
                        && item["binding"]["state"] == "active"
                })
            }),
            "{listed}"
        );

        #[cfg(windows)]
        let foreign_path = {
            let direct_text = existing_root.to_string_lossy();
            let drive = direct_text.chars().next().expect("drive letter");
            assert_eq!(direct_text.as_bytes().get(1), Some(&b':'));
            format!(
                r"\\localhost\{}$\{}",
                drive,
                direct_text[3..].replace('/', r"\")
            )
        };
        #[cfg(not(windows))]
        let foreign_path = existing_root.to_string_lossy().into_owned();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{foreign_library_uuid}/sync-folders/enrollment/preflight"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": uuid::Uuid::now_v7().to_string(),
                            "expectedOperationRevision": 0,
                            "rootUuid": null,
                            "mode": "enroll-existing",
                            "path": foreign_path
                        })
                        .to_string(),
                    ))
                    .expect("build cross-library preflight request"),
            )
            .await
            .expect("reject path bound to another Library");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = response_json(response).await;
        assert_eq!(response["code"], "path-occupied", "{response}");

        let changed_operation_uuid = uuid::Uuid::now_v7().to_string();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/enrollment/preflight"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": changed_operation_uuid,
                            "expectedOperationRevision": 0,
                            "rootUuid": null,
                            "mode": "enroll-existing",
                            "path": existing_root.to_string_lossy()
                        })
                        .to_string(),
                    ))
                    .expect("build completed enrollment replay preflight"),
            )
            .await
            .expect("preflight completed enrollment replay");
        assert_eq!(response.status(), StatusCode::OK);
        let changed_preflight = response_json(response).await;
        fs::write(
            existing_root.join("arrived-after-completed-preflight.jpg"),
            b"new content",
        )
        .expect("change content after completed enrollment preflight");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/enrollment/commit"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-mangoo-context-revision", context_revision.to_string())
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": changed_operation_uuid,
                            "expectedOperationRevision": 0,
                            "rootUuid": root_uuid,
                            "rootFolderUuid": uuid::Uuid::now_v7().to_string(),
                            "preflightToken": changed_preflight["preflightToken"],
                            "path": changed_preflight["normalizedPath"],
                            "requestedName": requested_name
                        })
                        .to_string(),
                    ))
                    .expect("build changed completed enrollment commit"),
            )
            .await
            .expect("reject changed completed enrollment replay");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = response_json(response).await;
        assert_eq!(response["code"], "path-occupied", "{response}");

        rusqlite::Connection::open(&host_path)
            .expect("reopen Host database")
            .execute(
                "UPDATE local_root_bindings SET state='local-rename-required',applied_materialized_name=NULL,applied_materialized_name_key=NULL,applied_materialization_fingerprint=NULL,last_error_code='local-rename-required',operation_revision=operation_revision+1 WHERE library_uuid=?1 AND root_uuid=?2",
                rusqlite::params![library_uuid,root_uuid],
            )
            .expect("simulate legacy automatic rename failure");
        let repair_operation_uuid = uuid::Uuid::now_v7().to_string();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/enrollment/preflight"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": repair_operation_uuid,
                            "expectedOperationRevision": 0,
                            "rootUuid": null,
                            "mode": "enroll-existing",
                            "path": existing_root.to_string_lossy()
                        })
                        .to_string(),
                    ))
                    .expect("build legacy repair preflight"),
            )
            .await
            .expect("preflight legacy repair");
        assert_eq!(response.status(), StatusCode::OK);
        let repair_preflight = response_json(response).await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/libraries/{library_uuid}/sync-folders/enrollment/commit"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-mangoo-context-revision", context_revision.to_string())
                    .body(Body::from(
                        serde_json::json!({
                            "operationUuid": repair_operation_uuid,
                            "expectedOperationRevision": 0,
                            "rootUuid": root_uuid,
                            "rootFolderUuid": uuid::Uuid::now_v7().to_string(),
                            "preflightToken": repair_preflight["preflightToken"],
                            "path": repair_preflight["normalizedPath"],
                            "requestedName": requested_name
                        })
                        .to_string(),
                    ))
                    .expect("build legacy repair commit"),
            )
            .await
            .expect("repair completed legacy enrollment");
        let response_status = response.status();
        let repaired = response_json(response).await;
        assert_eq!(response_status, StatusCode::ACCEPTED, "{repaired}");
        assert_eq!(repaired["operationUuid"], original_operation_uuid);
        let repaired_binding = manager
            .registry()
            .local_root_binding(&library_uuid, root_uuid)
            .unwrap()
            .expect("read repaired binding");
        assert_eq!(repaired_binding.state, "active");
        assert_eq!(
            repaired_binding
                .applied_materialization_fingerprint
                .as_deref(),
            Some(fingerprint.as_str())
        );

        drop(app);
        drop(state);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        fs::remove_file(unrelated_temporary_marker).expect("remove unrelated temporary marker");
        let _ = fs::remove_dir_all(base);
    }

    /// Empty-string env vars (e.g. `${MANGOO_SESSION_TOKEN:-}` from
    /// `compose.yaml`) used to be threaded into `ApiConfig.session_token` as
    /// `Some("")`, which made the auth middleware compare every incoming
    /// request to the empty string and reject `/api/health` with 401. Startup
    /// in `main.rs` now filters empty strings out of the optional credential
    /// vars so they collapse to `None`. This test documents the resulting
    /// behavior: with an unconfigured token the API (and `/api/health`) is
    /// reachable without any session header.
    #[tokio::test]
    async fn empty_string_session_token_does_not_reject_health() {
        // What `main.rs` produces from `MANGOO_SESSION_TOKEN=""`: filtered to
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

        // No `X-Mangoo-Session` header at all should now succeed.
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
                    .header("x-mangoo-session", "")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("empty header request");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn configured_cors_allows_library_patch_preflight_without_widening_access() {
        let origin = "http://127.0.0.1:5173";
        let app = router_with_config(
            test_database(),
            ApiConfig {
                session_token: Some("test-session".to_string()),
                allowed_origin: Some(origin.parse().expect("origin")),
                ..ApiConfig::default()
            },
        );
        let path = "/api/libraries/00000000-0000-4000-8000-000000000001";
        for (request_origin, method) in [
            (origin, "PATCH"),
            ("http://127.0.0.1:5174", "PATCH"),
            (origin, "TRACE"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::OPTIONS)
                        .uri(path)
                        .header(header::ORIGIN, request_origin)
                        .header(header::ACCESS_CONTROL_REQUEST_METHOD, method)
                        .header(
                            header::ACCESS_CONTROL_REQUEST_HEADERS,
                            "content-type,x-mangoo-session,x-mangoo-context-revision",
                        )
                        .body(Body::empty())
                        .expect("build library preflight"),
                )
                .await
                .expect("library preflight");
            assert_eq!(response.status(), StatusCode::OK);
            // CORS denial is represented by a mismatched origin or an absent
            // allowed method, not necessarily an HTTP error status.
            assert_eq!(
                response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
                origin
            );
            let methods = response.headers()[header::ACCESS_CONTROL_ALLOW_METHODS]
                .to_str()
                .expect("allow methods")
                .split(',')
                .map(str::trim)
                .collect::<Vec<_>>();
            assert!(methods.contains(&"PATCH"));
            assert!(!methods.contains(&"TRACE"));
            assert!(!methods.contains(&"*"));
            let browser_permitted = request_origin == origin && methods.contains(&method);
            assert_eq!(
                browser_permitted,
                request_origin == origin && method == "PATCH"
            );
            let headers = response.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS]
                .to_str()
                .expect("allow headers")
                .split(',')
                .map(str::trim)
                .collect::<Vec<_>>();
            for required in [
                "content-type",
                "x-mangoo-session",
                "x-mangoo-context-revision",
            ] {
                assert!(headers.contains(&required));
            }
            assert!(!headers.contains(&"*"));
        }

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(path)
                    .header(header::ORIGIN, origin)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"Renamed"}"#))
                    .expect("build unauthenticated patch"),
            )
            .await
            .expect("unauthenticated patch");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
                        "content-type,x-mangoo-session,x-mangoo-context-revision",
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
        assert!(allow_headers.contains("x-mangoo-session"));
        assert!(allow_headers.contains("x-mangoo-context-revision"));
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
        // preview client reads `x-mangoo-preview-tier` as null, falls back to
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
            expose_headers.contains("x-mangoo-preview-tier"),
            "preview tier header must be readable cross-origin, got: {expose_headers}"
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/health")
                    .header(header::ORIGIN, "http://127.0.0.1:5174")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "x-mangoo-session")
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
        let database = Database::open(db_dir.join("mangoo.sqlite")).expect("open db");
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
        let database = Database::open(db_dir.join("mangoo.sqlite")).expect("open db");
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
            .fsops_database()
            .expect("read fsops database")
            .expect("file-backed fsops database");
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
        let database = Database::open(db_dir.join("mangoo.sqlite")).expect("open db");
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
        let database = Database::open(db_dir.join("mangoo.sqlite")).expect("open db");
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
        let database = Database::open(db_dir.join("mangoo.sqlite")).expect("open db");
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
            "mangoo_api_test_{}_{}_{}",
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
            library_uuid: None,
            root_uuid: None,
            managed_root_action: None,
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
        let writer = state.writer().expect("active library writer");
        let read_gate = state
            .notification_outbox_read_gate()
            .expect("active library outbox gate");
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
        let read_gate = state
            .notification_outbox_read_gate()
            .expect("active library outbox gate");
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
        let read_gate = state
            .notification_outbox_read_gate()
            .expect("active library outbox gate");
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
                    .header("x-mangoo-session", "notification-session")
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
        let base = unique_temp_dir();
        std::fs::create_dir_all(&base).unwrap();
        let state = AppState::new_host(base.join("host.sqlite"), base.join("libraries"))
            .await
            .unwrap();
        let app = router(state);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/plugins/com.example.missing/views/setup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response_json(response).await["code"], "plugin_not_found");

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
        let _ = std::fs::remove_dir_all(base);

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
    async fn generic_host_callback_claims_current_flow_and_rejects_cancel_epoch_replay() {
        let base = unique_temp_dir();
        std::fs::create_dir_all(&base).unwrap();
        let state = AppState::new_host(base.join("host.sqlite"), base.join("libraries"))
            .await
            .unwrap();
        let manager = state.library_manager.clone().unwrap();
        let registry = manager.host_registry();
        let id = "com.mangoo.callback-fixture";
        registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: id,
                version: "1.0.0",
                manifest_json: include_str!(
                    "../../../../bundled-plugins/com.mangoo.fake-sync/plugin.template.json"
                ),
                entry_relative_path: "fixture",
                entry_sha256: &[0; 32],
                install_root: &base,
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();
        registry
            .upsert_plugin_account("callback-account", id, None, "signed-out")
            .unwrap();
        registry.set_plugin_intent(id, true, true).unwrap();
        registry
            .record_plugin_runtime_ready(id, "callback-session", &["sync-provider.v1".into()])
            .unwrap();
        manager
            .provider_manager()
            .register(
                id,
                "callback-account",
                Arc::new(crate::sync::provider::InMemorySyncProvider::default()),
            )
            .unwrap();
        let app = router(state.clone());
        for scenario in ["cancel", "epoch", "success"] {
            let host_flow = state
                .plugin_auth_flows
                .create(crate::plugins::auth_flow::AuthFlowBinding {
                    plugin_id: id.into(),
                    account_uuid: "callback-account".into(),
                    supervisor_session: "callback-session".into(),
                    provider_flow_id: None,
                })
                .unwrap();
            let callback =
                format!("https://core.example.test/api/plugin-auth/callback/{host_flow}");
            let auth = super::plugin_provider(&state, id)
                .map_err(|e| e.message)
                .unwrap()
                .auth_begin(Some(callback))
                .await
                .unwrap();
            state
                .plugin_auth_flows
                .attach_provider_flow(&host_flow, auth.flow_id.clone())
                .unwrap();
            assert!(registry
                .begin_plugin_auth_flow(id, &host_flow, "callback-session")
                .unwrap());
            let epoch = registry.plugin_lifecycle_epoch(id).unwrap();
            state
                .plugin_auth_flows
                .register_pending(
                    id,
                    &host_flow,
                    "callback-session",
                    epoch,
                    &auth,
                    &registry
                        .plugin_contributions(id)
                        .unwrap()
                        .authentication
                        .unwrap(),
                    crate::host_registry::unix_timestamp_millis() + 60_000,
                )
                .unwrap();
            match scenario {
                "cancel" => {
                    assert!(
                        super::cancel_owned_auth_worker(&state, id, Some(&host_flow), true)
                            .await
                            .map_err(|e| e.message)
                            .unwrap()
                    );
                }
                "epoch" => {
                    registry.connection.lock().unwrap().execute("UPDATE plugin_installations SET lifecycle_epoch=lifecycle_epoch+1 WHERE plugin_id=?1",[id]).unwrap();
                }
                _ => {}
            }
            let path = format!("/api/plugin-auth/callback/{host_flow}?code=ok&state=test");
            let response = app
                .clone()
                .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                match scenario {
                    "cancel" => StatusCode::BAD_REQUEST,
                    "epoch" => StatusCode::CONFLICT,
                    _ => StatusCode::OK,
                },
                "{scenario}"
            );
            let replay = app
                .clone()
                .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
            if scenario == "epoch" {
                super::cancel_owned_auth_worker(&state, id, Some(&host_flow), true)
                    .await
                    .map_err(|e| e.message)
                    .unwrap();
            } else {
                assert!(registry.plugin_auth_flow_ownership(id).unwrap().is_none());
            }
            if scenario != "success" {
                assert_eq!(
                    registry
                        .host_plugin(id)
                        .unwrap()
                        .unwrap()
                        .auth_state
                        .as_deref(),
                    Some("signed-out")
                );
            }
        }
        assert_eq!(
            registry
                .host_plugin(id)
                .unwrap()
                .unwrap()
                .auth_state
                .as_deref(),
            Some("authenticated")
        );
        manager.shutdown_sync_coordinator().await.unwrap();
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn generic_provider_real_child_configuration_auth_links_and_restart() {
        use crate::library_manager::{ActiveSnapshot, LibraryManager};
        use std::{sync::RwLock, time::Duration};
        let base = unique_temp_dir();
        std::fs::create_dir_all(&base).unwrap();
        let bundled = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("bundled-plugins")
            .canonicalize()
            .unwrap();
        let id = "com.mangoo.fake-sync";
        let account = "generic-account";
        let manifest =
            crate::plugins::parse_manifest(&bundled.join(id).join("plugin.json")).unwrap();
        let platform = if cfg!(windows) {
            crate::plugins::manifest::Platform::WindowsX64
        } else {
            crate::plugins::manifest::Platform::LinuxX64
        };
        let entry = manifest.entry_for(platform).unwrap();
        let hash = crate::plugins::manifest::sha256_file(&bundled.join(id).join(entry)).unwrap();
        let registry =
            Arc::new(crate::host_registry::HostRegistry::open(base.join("host.sqlite")).unwrap());
        let permissions = serde_json::to_string(&manifest.requested_permissions).unwrap();
        registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: id,
                version: &manifest.version,
                manifest_json: &serde_json::to_string(&manifest).unwrap(),
                entry_relative_path: entry,
                entry_sha256: &hash,
                install_root: &bundled.join(id),
                trust_policy_id: "embedded-v1",
                requested_permissions_json: &permissions,
            })
            .unwrap();
        registry.set_plugin_grants(id, &permissions, true).unwrap();
        registry
            .upsert_plugin_account(account, id, Some("Fixture account"), "signed-out")
            .unwrap();
        registry.set_plugin_intent(id, true, true).unwrap();
        let active = Arc::new(RwLock::new(ActiveSnapshot {
            library_uuid: None,
            context_revision: 0,
            runtime_generation: 0,
            runtime: None,
        }));
        let request_gate = Arc::new(tokio::sync::RwLock::new(()));
        let secret_store: Option<Arc<dyn crate::plugins::secret_store::SecretStore>> = Some(
            Arc::new(crate::plugins::secret_store::MemorySecretStore::default()),
        );
        let manager = Arc::new(LibraryManager::new(
            registry.clone(),
            base.join("libraries"),
            bundled,
            secret_store.clone(),
            active.clone(),
            request_gate.clone(),
        ));
        let state = AppState {
            playback_sessions: Arc::new(Default::default()),
            active,
            request_gate,
            plugin_ui_gate: Arc::new(tokio::sync::RwLock::new(())),
            library_manager: Some(manager.clone()),
            plugin_auth_flows: Arc::new(Default::default()),
            plugin_auth_workers: Arc::new(Default::default()),
            plugin_auth_begin_gate: Arc::new(Default::default()),
            secret_store,
            notification_outbox_test_probe: None,
        };
        let app = router(state);
        manager.start_plugin_runtime_supervisors();
        async fn session(
            registry: &crate::host_registry::HostRegistry,
            id: &str,
            old: Option<&str>,
        ) -> String {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Some(value) = registry.plugin_runtime_session(id).unwrap() {
                        if old != Some(value.as_str()) {
                            return value;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap()
        }
        let original = session(&registry, id, None).await;
        async fn json_request(
            app: &Router,
            path: &str,
            method: Method,
            body: serde_json::Value,
        ) -> axum::response::Response {
            app.clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap()
        }
        let configuration = response_json(
            json_request(
                &app,
                "/api/plugins/com.mangoo.fake-sync/configuration",
                Method::GET,
                serde_json::json!({}),
            )
            .await,
        )
        .await;
        assert_eq!(configuration["values"]["workspace"], "fixture");
        let plan=response_json(json_request(&app,"/api/plugins/com.mangoo.fake-sync/configuration/plan",Method::POST,serde_json::json!({"values":{"workspace":"next"},"expectedRevision":configuration["revision"]})).await).await;
        let response=json_request(&app,"/api/plugins/com.mangoo.fake-sync/configuration",Method::PUT,serde_json::json!({"planUuid":plan["planUuid"],"expectedRevision":plan["expectedRevision"],"confirmed":true})).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["values"]["workspace"], "next");
        let restarted = session(&registry, id, Some(&original)).await;
        let response = json_request(
            &app,
            "/api/plugins/com.mangoo.fake-sync/auth/begin",
            Method::POST,
            serde_json::json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let flow = response_json(response).await;
        let response = json_request(
            &app,
            "/api/plugins/com.mangoo.fake-sync/external-link",
            Method::POST,
            serde_json::json!({"kind":"authorization","id":flow["flowId"]}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let link = response_json(response).await;
        assert_eq!(link["url"], flow["authorizationUrl"]);
        assert!(link["url"]
            .as_str()
            .unwrap()
            .starts_with("https://auth.example.test/"));
        let response=json_request(&app,"/api/plugins/com.mangoo.fake-sync/external-link",Method::POST,serde_json::json!({"kind":"authorization","id":flow["flowId"],"url":"https://arbitrary.example.test/"})).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let response=json_request(&app,"/api/plugins/com.mangoo.fake-sync/auth/complete",Method::POST,serde_json::json!({"flowId":flow["flowId"],"responseUri":"https://wrong.example.test/return?code=ok&state=s"})).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response=json_request(&app,"/api/plugins/com.mangoo.fake-sync/auth/complete",Method::POST,serde_json::json!({"flowId":flow["flowId"],"responseUri":"https://auth.example.test/return?code=ok&state=fixture-state&session_hint=discarded"})).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let response = json_request(
            &app,
            "/api/plugins/com.mangoo.fake-sync/external-link",
            Method::POST,
            serde_json::json!({"kind":"authorization","id":flow["flowId"]}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = json_request(
            &app,
            "/api/plugins/com.mangoo.fake-sync/auth/sign-out",
            Method::POST,
            serde_json::json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let signedout = session(&registry, id, Some(&restarted)).await;
        assert_ne!(signedout, restarted);
        let response = json_request(
            &app,
            "/api/plugins/com.mangoo.fake-sync/auth/begin",
            Method::POST,
            serde_json::json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let pending = response_json(response).await;
        let response = json_request(
            &app,
            "/api/plugins/com.mangoo.fake-sync/disable",
            Method::POST,
            serde_json::json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = json_request(
            &app,
            "/api/plugins/com.mangoo.fake-sync/external-link",
            Method::POST,
            serde_json::json!({"kind":"authorization","id":pending["flowId"]}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = json_request(
            &app,
            "/api/plugins/com.mangoo.fake-sync/external-link",
            Method::POST,
            serde_json::json!({"kind":"help","id":"guide"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json(response).await["url"],
            "https://docs.example.test/setup"
        );
        assert!(!registry.host_plugin(id).unwrap().unwrap().enabled_intent);
        manager.shutdown_sync_coordinator().await.unwrap();
        drop(manager);
        drop(registry);
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plugin_lifecycle_real_child_disable_joins_discovery_and_rejects_aba() {
        use crate::library_manager::{ActiveSnapshot, LibraryManager};
        use std::sync::RwLock;
        use std::time::Duration;
        let base = unique_temp_dir();
        std::fs::create_dir_all(&base).unwrap();
        let bundled = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("bundled-plugins")
            .canonicalize()
            .unwrap();
        let id = "com.mangoo.fake-sync";
        let account = "lifecycle-account";
        let manifest =
            crate::plugins::parse_manifest(&bundled.join(id).join("plugin.json")).unwrap();
        let platform = if cfg!(windows) {
            crate::plugins::manifest::Platform::WindowsX64
        } else {
            crate::plugins::manifest::Platform::LinuxX64
        };
        let entry = manifest.entry_for(platform).unwrap();
        let hash = crate::plugins::manifest::sha256_file(&bundled.join(id).join(entry)).unwrap();
        let registry =
            Arc::new(crate::host_registry::HostRegistry::open(base.join("host.sqlite")).unwrap());
        let permissions = serde_json::to_string(&manifest.requested_permissions).unwrap();
        registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: id,
                version: &manifest.version,
                manifest_json: &serde_json::to_string(&manifest).unwrap(),
                entry_relative_path: entry,
                entry_sha256: &hash,
                install_root: &bundled.join(id),
                trust_policy_id: "embedded-v1",
                requested_permissions_json: &permissions,
            })
            .unwrap();
        registry.set_plugin_grants(id, &permissions, true).unwrap();
        registry
            .upsert_plugin_account(account, id, Some("Retained account"), "authenticated")
            .unwrap();
        registry.set_plugin_intent(id, true, true).unwrap();
        let active = Arc::new(RwLock::new(ActiveSnapshot {
            library_uuid: None,
            context_revision: 0,
            runtime_generation: 0,
            runtime: None,
        }));
        let request_gate = Arc::new(tokio::sync::RwLock::new(()));
        let secret_store: Option<Arc<dyn crate::plugins::secret_store::SecretStore>> = Some(
            Arc::new(crate::plugins::secret_store::MemorySecretStore::default()),
        );
        let manager = Arc::new(LibraryManager::new(
            registry.clone(),
            base.join("libraries"),
            bundled,
            secret_store.clone(),
            active.clone(),
            request_gate.clone(),
        ));
        let state = AppState {
            playback_sessions: Arc::new(Default::default()),
            active,
            request_gate,
            plugin_ui_gate: Arc::new(tokio::sync::RwLock::new(())),
            library_manager: Some(manager.clone()),
            plugin_auth_flows: Arc::new(Default::default()),
            plugin_auth_workers: Arc::new(Default::default()),
            plugin_auth_begin_gate: Arc::new(Default::default()),
            secret_store,
            notification_outbox_test_probe: None,
        };
        let app = router(state);
        manager.start_plugin_runtime_supervisors();
        let old_session = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(session) = registry.plugin_runtime_session(id).unwrap() {
                    break session;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let old_fingerprint = registry.plugin_runtime_launches().unwrap()[0].fingerprint();
        let old_provider = manager
            .provider_manager()
            .resolve(id, account)
            .unwrap()
            .unwrap();
        let provider_state = base
            .join("plugin-state")
            .join(id)
            .join(account)
            .join("provider-state");
        std::fs::write(
            provider_state.join("lifecycle-barrier-method"),
            "delta.list",
        )
        .unwrap();
        let coordinator = crate::sync::coordinator::SyncCoordinator::with_providers(
            registry.clone(),
            Default::default(),
            manager.provider_manager(),
        );
        let discovery =
            tokio::spawn(async move { coordinator.run_root_discovery_for_test().await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !provider_state.join("lifecycle-barrier-entered").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        // A queued local completion is part of the same Provider stop barrier.
        #[cfg(windows)]
        let child_handle = {
            use std::os::windows::io::{FromRawHandle, OwnedHandle};
            let pid: u32 =
                std::fs::read_to_string(provider_state.join("lifecycle-barrier-entered"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
            let handle = unsafe {
                windows_sys::Win32::System::Threading::OpenProcess(
                    windows_sys::Win32::System::Threading::PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                )
            };
            assert!(!handle.is_null());
            unsafe { OwnedHandle::from_raw_handle(handle) }
        };
        let queued = manager.provider_manager().begin_work(id).unwrap();
        let disable_app = app.clone();
        let disable = tokio::spawn(async move {
            disable_app
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/plugins/com.mangoo.fake-sync/disable")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while registry.host_plugin(id).unwrap().unwrap().runtime_state != "stopping" {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!disable.is_finished());
        assert!(queued.check_current().is_err());
        assert!(old_provider.health().await.is_err());
        drop(queued);
        let response = tokio::time::timeout(Duration::from_secs(10), disable)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["runtimeState"], "disabled");
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            assert_eq!(
                unsafe {
                    windows_sys::Win32::System::Threading::WaitForSingleObject(
                        child_handle.as_raw_handle(),
                        0,
                    )
                },
                windows_sys::Win32::Foundation::WAIT_OBJECT_0,
                "disabled response must wait for the original process to exit"
            );
        }
        assert!(discovery.await.unwrap().is_err());
        assert!(manager
            .provider_manager()
            .resolve(id, account)
            .unwrap()
            .is_none());
        assert_eq!(
            registry
                .host_plugin(id)
                .unwrap()
                .unwrap()
                .auth_state
                .as_deref(),
            Some("authenticated")
        );
        assert!(!registry
            .plugin_runtime_launch_is_current(id, &old_fingerprint)
            .unwrap());
        std::fs::remove_file(provider_state.join("lifecycle-barrier-method")).unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/plugins/com.mangoo.fake-sync/enable")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let new_session = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(session) = registry.plugin_runtime_session(id).unwrap() {
                    break session;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_ne!(old_session, new_session);
        assert!(old_provider.health().await.is_err());
        let before_retry = manager
            .provider_manager()
            .resolve(id, account)
            .unwrap()
            .unwrap();
        let retry_epoch = registry.plugin_lifecycle_epoch(id).unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/plugins/com.mangoo.fake-sync/retry")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(registry.plugin_lifecycle_epoch(id).unwrap() > retry_epoch + 1);
        assert!(before_retry.health().await.is_err());
        let restarted_session = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(session) = registry.plugin_runtime_session(id).unwrap() {
                    if session != new_session {
                        break session;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_ne!(restarted_session, new_session);
        assert!(manager
            .provider_manager()
            .resolve(id, account)
            .unwrap()
            .unwrap()
            .health()
            .await
            .is_ok());
        let (library, _, _) = manager
            .create_library(
                "Local usable".into(),
                None,
                None,
                base.join("managed"),
                None,
                false,
            )
            .await
            .unwrap();
        registry
            .begin_sync_binding(&library.library_uuid, id, account)
            .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/plugins/com.mangoo.fake-sync/disable")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let status_revision = registry.sync_status_snapshot().unwrap().revision;
        let context_revision = registry.snapshot().unwrap().context_revision;
        for command in ["resume", "retry", "sync-now"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!(
                            "/api/libraries/{}/sync/{command}",
                            library.library_uuid
                        ))
                        .header("X-Mangoo-Context-Revision", context_revision.to_string())
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::json!({"operationUuid":uuid::Uuid::now_v7().to_string()})
                                .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT, "{command}");
            assert_eq!(response_json(response).await["code"], "provider_not_ready");
        }
        assert_eq!(
            registry.sync_status_snapshot().unwrap().revision,
            status_revision
        );
        let browse = app
            .oneshot(
                Request::builder()
                    .uri("/api/libraries")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(browse.status(), StatusCode::OK);
        manager.shutdown_sync_coordinator().await.unwrap();
        drop(manager);
        drop(registry);
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn sync_retry_route_recovers_runnable_phase_and_preserves_command_fences() {
        let base = unique_temp_dir();
        std::fs::create_dir_all(&base).unwrap();
        let state = AppState::new_host(base.join("host.sqlite"), base.join("libraries"))
            .await
            .unwrap();
        let manager = state.library_manager.clone().unwrap();
        // Library creation precedes shutdown's lifecycle admission fence.
        // No Provider is registered yet, so fixture setup cannot schedule sync.
        let created = manager
            .create_library(
                "Retry Library".into(),
                None,
                None,
                base.join("managed"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;
        // Drive actual coordinator turns explicitly so assertions cannot race
        // the scheduler or the test-only Provider supervisor.
        manager.shutdown_sync_coordinator().await.unwrap();
        let registry = manager.registry();
        registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: "com.mangoo.fake",
                version: "1.0.0",
                manifest_json: "{}",
                entry_relative_path: "fake-provider",
                entry_sha256: &[0; 32],
                install_root: &base,
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();
        registry
            .upsert_plugin_account(
                "fake-account",
                "com.mangoo.fake",
                Some("Test"),
                "authenticated",
            )
            .unwrap();
        registry
            .set_plugin_intent("com.mangoo.fake", true, true)
            .unwrap();
        registry
            .record_plugin_runtime_ready(
                "com.mangoo.fake",
                "test-session",
                &["sync-provider.v1".into()],
            )
            .unwrap();
        manager
            .provider_manager()
            .register(
                "com.mangoo.fake",
                "fake-account",
                std::sync::Arc::new(crate::sync::provider::InMemorySyncProvider::default()),
            )
            .unwrap();
        let library_uuid = &created.library_uuid;
        let revision = manager.active_snapshot().context_revision;
        registry
            .begin_sync_binding(library_uuid, "com.mangoo.fake", "fake-account")
            .unwrap();
        registry
            .record_sync_turn_error(library_uuid, "test_failure")
            .unwrap();
        let original = registry
            .binding_transport(library_uuid, "library")
            .unwrap()
            .unwrap();
        let app = router(state);
        let request = |operation: &str, context: u64| {
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/libraries/{library_uuid}/sync/retry"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-mangoo-context-revision", context.to_string())
                .body(Body::from(
                    serde_json::json!({"operationUuid":operation}).to_string(),
                ))
                .unwrap()
        };
        let operation = uuid::Uuid::now_v7().to_string();
        let stale = app
            .clone()
            .oneshot(request(&uuid::Uuid::now_v7().to_string(), revision + 1))
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        assert_eq!(response_json(stale).await["code"], "stale_library_context");
        assert_eq!(
            registry
                .binding_transport(library_uuid, "library")
                .unwrap()
                .unwrap()
                .state,
            "error"
        );
        assert_eq!(
            registry.sync_status_snapshot().unwrap().libraries[0]
                .last_error_code
                .as_deref(),
            Some("test_failure")
        );

        let response = app
            .clone()
            .oneshot(request(&operation, revision))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response_json(response).await["state"], "complete");
        let recovered = registry
            .binding_transport(library_uuid, "library")
            .unwrap()
            .unwrap();
        assert_eq!(recovered.state, "preparing");
        assert_eq!(recovered.binding_epoch, original.binding_epoch);
        assert_eq!(recovered.actor_epoch, original.actor_epoch);
        assert_eq!(
            recovered.transport_generation,
            original.transport_generation
        );
        assert!(registry
            .pending_sync_capture_generation(library_uuid)
            .unwrap()
            .is_some());
        let coordinator = crate::sync::coordinator::SyncCoordinator::with_providers(
            std::sync::Arc::clone(registry),
            crate::sync::runtime::RuntimeLeases::default(),
            manager.provider_manager(),
        );
        let catalog_path = created.local_data_dir.join("catalog.sqlite");
        coordinator
            .run_library_turn(library_uuid, &catalog_path)
            .await
            .unwrap();
        assert_eq!(
            registry
                .binding_transport(library_uuid, "library")
                .unwrap()
                .unwrap()
                .state,
            "seed_ready"
        );
        coordinator
            .run_library_turn(library_uuid, &catalog_path)
            .await
            .unwrap();
        let published = registry
            .binding_transport(library_uuid, "library")
            .unwrap()
            .unwrap();
        assert_eq!(published.state, "active");
        assert!(
            published.cursor.is_some(),
            "the real Provider transport turn must run after retry"
        );

        registry
            .record_sync_turn_error(library_uuid, "second_failure")
            .unwrap();
        let repeated = app
            .clone()
            .oneshot(request(&operation, revision))
            .await
            .unwrap();
        assert_eq!(repeated.status(), StatusCode::OK);
        assert_eq!(
            registry
                .binding_transport(library_uuid, "library")
                .unwrap()
                .unwrap()
                .state,
            "error",
            "replayed completed commands must have no new effect"
        );
        let second_operation = uuid::Uuid::now_v7().to_string();
        let second = app
            .clone()
            .oneshot(request(&second_operation, revision))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let recovered = registry
            .binding_transport(library_uuid, "library")
            .unwrap()
            .unwrap();
        assert_eq!(recovered.state, "active");
        assert_eq!(recovered.binding_epoch, published.binding_epoch);
        assert_eq!(
            recovered.transport_generation,
            published.transport_generation
        );
        assert_eq!(
            recovered.latest_checkpoint_uuid,
            published.latest_checkpoint_uuid
        );
        assert_eq!(recovered.cursor, published.cursor);
        assert!(registry.sync_status_snapshot().unwrap().libraries[0]
            .last_error_code
            .is_none());
        coordinator
            .run_library_turn(library_uuid, &catalog_path)
            .await
            .unwrap();

        registry
            .record_sync_turn_error(library_uuid, "authorization_failure")
            .unwrap();
        registry
            .update_plugin_account_auth("com.mangoo.fake", "expired", None)
            .unwrap();
        let unauthorized = app
            .clone()
            .oneshot(request(&uuid::Uuid::now_v7().to_string(), revision))
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::CONFLICT);
        assert_eq!(
            registry
                .binding_transport(library_uuid, "library")
                .unwrap()
                .unwrap()
                .state,
            "error"
        );
        registry
            .update_plugin_account_auth("com.mangoo.fake", "authenticated", None)
            .unwrap();
        registry.pause_binding(library_uuid).unwrap();
        let paused = registry
            .binding_transport(library_uuid, "library")
            .unwrap()
            .unwrap();
        let repeated = app
            .clone()
            .oneshot(request(&second_operation, revision))
            .await
            .unwrap();
        assert_eq!(repeated.status(), StatusCode::OK);
        let rejected = app
            .oneshot(request(&uuid::Uuid::now_v7().to_string(), revision))
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::CONFLICT);
        let after = registry
            .binding_transport(library_uuid, "library")
            .unwrap()
            .unwrap();
        assert_eq!(after.state, "paused");
        assert_eq!(after.transport_generation, paused.transport_generation);
        coordinator.shutdown_media_workers().await.unwrap();
        manager.provider_manager().shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn plugin_provider_routes_delegate_to_registered_runtime() {
        let base = unique_temp_dir();
        std::fs::create_dir_all(&base).unwrap();
        let state = AppState::new_host(base.join("host.sqlite"), base.join("libraries"))
            .await
            .unwrap();
        let manager = state.library_manager.clone().unwrap();
        manager
            .host_registry()
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: "com.mangoo.fake",
                version: "1.0.0",
                manifest_json: include_str!(
                    "../../../../bundled-plugins/com.mangoo.fake-sync/plugin.template.json"
                ),
                entry_relative_path: "fake-provider",
                entry_sha256: &[0; 32],
                install_root: &base,
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();
        manager
            .host_registry()
            .upsert_plugin_account(
                "fake-account",
                "com.mangoo.fake",
                Some("Fake account"),
                "authenticated",
            )
            .unwrap();
        manager
            .host_registry()
            .set_plugin_intent("com.mangoo.fake", true, true)
            .unwrap();
        manager
            .host_registry()
            .record_plugin_runtime_ready(
                "com.mangoo.fake",
                "test-session",
                &["sync-provider.v1".into()],
            )
            .unwrap();
        manager
            .provider_manager()
            .register(
                "com.mangoo.fake",
                "fake-account",
                std::sync::Arc::new(crate::sync::provider::InMemorySyncProvider::default()),
            )
            .unwrap();
        let created = manager
            .create_library(
                "Synced Library".into(),
                None,
                None,
                base.join("managed"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;
        // This is the first authentication of this fixture Provider. Complete it
        // before binding: an existing binding with no known remote identity must
        // instead take the conservative scope-change fence.
        assert!(manager
            .host_registry()
            .binding_transport(&created.library_uuid, "library")
            .unwrap()
            .is_none());
        let app = router(state);

        let begin = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/plugins/com.mangoo.fake/auth/begin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(begin.status(), StatusCode::CREATED);
        let flow_id = response_json(begin).await["flowId"]
            .as_str()
            .unwrap()
            .to_string();

        let complete = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/plugins/com.mangoo.fake/auth/complete")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        "{{\"flowId\":\"{flow_id}\",\"responseUri\":\"https://auth.example.test/return?code=ok&state=test\"}}"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(complete.status(), StatusCode::ACCEPTED);

        let (binding_epoch, _) = manager
            .host_registry()
            .begin_sync_binding(&created.library_uuid, "com.mangoo.fake", "fake-account")
            .unwrap();
        assert_eq!(
            manager
                .host_registry()
                .binding_transport(&created.library_uuid, "library")
                .unwrap()
                .unwrap()
                .transport_generation,
            1
        );

        for endpoint in ["health", "diagnostics"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(format!("/api/plugins/com.mangoo.fake/{endpoint}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        manager
            .provider_manager()
            .unregister("com.mangoo.fake", "fake-account")
            .unwrap();

        let sign_out = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/plugins/com.mangoo.fake/auth/sign-out")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(sign_out.status(), StatusCode::ACCEPTED);
        let account = manager
            .host_registry()
            .plugin_account_secret("com.mangoo.fake")
            .unwrap()
            .unwrap();
        assert!(account.secret_ref.is_none());
        assert_eq!(account.generation, 2);
        let binding = manager
            .host_registry()
            .binding_transport(&created.library_uuid, "library")
            .unwrap()
            .unwrap();
        assert_eq!(binding.binding_epoch, binding_epoch);
        assert_eq!(binding.transport_generation, 2);
        assert_eq!(
            manager
                .host_registry()
                .sync_status_snapshot()
                .unwrap()
                .libraries
                .into_iter()
                .find(|status| status.library_uuid == created.library_uuid)
                .unwrap()
                .lifecycle,
            "paused"
        );
        manager.provider_manager().shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn ordinary_active_library_mutations_reject_missing_and_stale_revision() {
        let base = unique_temp_dir();
        std::fs::create_dir_all(&base).unwrap();
        let state = AppState::new_host(base.join("host.sqlite"), base.join("libraries"))
            .await
            .unwrap();
        let manager = state.library_manager.clone().unwrap();
        let created = manager
            .create_library(
                "Revision fence".to_string(),
                None,
                None,
                base.join("managed"),
                None,
                true,
            )
            .await
            .unwrap();
        assert!(created.1);
        let revision = manager.active_snapshot().context_revision;
        let app = crate::api::router_with_state(state, crate::api::ApiConfig::default());
        let request = |revision_header: Option<u64>| {
            let mut builder = Request::builder()
                .method(Method::POST)
                .uri("/api/tags")
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(revision) = revision_header {
                builder = builder.header("x-mangoo-context-revision", revision.to_string());
            }
            builder
                .body(Body::from(r#"{"name":"fenced","color":null}"#))
                .unwrap()
        };
        for stale in [None, Some(revision.saturating_sub(1))] {
            let response = app.clone().oneshot(request(stale)).await.unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            assert_eq!(
                response_json(response).await["code"],
                "stale_library_context"
            );
        }
        let response = app.clone().oneshot(request(Some(revision))).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        if let Some(runtime) = manager.active_snapshot().runtime {
            runtime.shutdown().await.unwrap();
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn plugin_discover_route_returns_zero_for_missing_directory() {
        // Confirm the discover handler reports zero/zero when the configured
        // plugins folder does not exist on disk. We exercise the discovery
        // function directly to avoid mutating process-wide environment
        // variables, which would race with tests that share the runtime.
        let plugins_dir = unique_temp_dir();
        let (records, errors) = crate::plugins::discover(&plugins_dir);
        assert!(records.is_empty());
        assert!(errors.is_empty());
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
        let csp = response
            .headers()
            .get("content-security-policy")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("connect-src 'self'"));
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
        let csp = response
            .headers()
            .get("content-security-policy")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(csp.contains("default-src 'self'"));

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
