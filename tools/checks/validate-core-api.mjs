import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exitCode = 1;
}

function read(relativePath) {
  // Normalize CRLF to LF so the contract regexes work the same on Windows
  // (where git autocrlf=true checks files out with \r\n endings) and POSIX.
  return readFileSync(path.join(root, relativePath), "utf8").replace(/\r\n/g, "\n");
}

const openApi = read("contracts/core-api/openapi.yaml");
const routesRs = read("crates/core/src/api/routes.rs");
const apiModRs = read("crates/core/src/api/mod.rs");
const dbModRs = read("crates/core/src/db/mod.rs");
const tasksRs = read("crates/core/src/tasks.rs");
const dbMigrationsRs = read("crates/core/src/db/migrations.rs");
const mainRs = read("crates/core/src/main.rs");
const migrationSql = read("crates/core/migrations/0001_initial.sql");
const thumbnailMigrationSql = read("crates/core/migrations/0004_thumbnail_state.sql");
const thumbnailSourceMigrationSql = read("crates/core/migrations/0005_thumbnail_source_fingerprint.sql");
const thumbnailTaskAttemptMigrationSql = read("crates/core/migrations/0006_thumbnail_task_attempt_fingerprint.sql");
const taskStatusContractMigrationSql = read("crates/core/migrations/0007_task_status_contract.sql");
const taskAttemptGenerationMigrationSql = read("crates/core/migrations/0008_task_attempt_generation.sql");
const previewServedByMigrationSql = read("crates/core/migrations/0013_preview_served_by.sql");
const thumbnailsRs = read("crates/core/src/thumbnails/mod.rs");
const pluginsRs = read("crates/core/src/plugins/mod.rs");
const fsopsRs = read("crates/core/src/fsops/mod.rs");
const coreCargo = read("crates/core/Cargo.toml");
const pluginManifest = read("contracts/plugins/manifest.schema.json");

const openApiPaths = new Set(
  [...openApi.matchAll(/^  (\/[^:\n]+):/gm)].map((match) => `/api${match[1]}`)
);

const rustPathsBlock = routesRs.match(/PHASE1_API_PATHS:\s*&\[&str\]\s*=\s*&\[(?<body>[\s\S]*?)\];/);
if (!rustPathsBlock?.groups?.body) {
  fail("PHASE1_API_PATHS constant is missing in routes.rs");
}

const rustContractPaths = new Set(
  [...(rustPathsBlock?.groups?.body ?? "").matchAll(/"([^"]+)"/g)].map((match) => match[1])
);

for (const openApiPath of openApiPaths) {
  if (!rustContractPaths.has(openApiPath)) {
    fail(`OpenAPI path missing from PHASE1_API_PATHS: ${openApiPath}`);
  }
}
for (const rustPath of rustContractPaths) {
  if (!openApiPaths.has(rustPath)) {
    fail(`PHASE1_API_PATHS path missing from OpenAPI: ${rustPath}`);
  }
}

const axumRoutes = new Set(
  [...routesRs.matchAll(/\.route\(\s*"([^"]+)"/g)].map((match) => match[1])
);

const expectedAxumRoutes = [
  "/api/health",
  "/api/roots",
  "/api/roots/:root_id",
  "/api/folders/:folder_id/children",
  "/api/folders/:folder_id/ancestors",
  "/api/media",
  "/api/media/:file_id",
  "/api/media/:file_id/thumbnail",
  "/api/media/:file_id/preview",
  "/api/media/:file_id/metadata",
  "/api/media/:file_id/tags",
  "/api/media/:file_id/tags/:tag_id",
  "/api/tags",
  "/api/tags/:tag_id",
  "/api/search",
  "/api/tasks",
  "/api/tasks/scan",
  "/api/tasks/:task_id/cancel",
  "/api/tasks/:task_id/retry",
  "/api/file-ops/rename",
  "/api/file-ops/move",
  "/api/file-ops/delete",
  "/api/plugins",
  "/api/plugins/discover",
  "/api/plugins/:plugin_id",
  "/api/plugins/:plugin_id/enable",
  "/api/plugins/:plugin_id/disable"
];

for (const route of expectedAxumRoutes) {
  if (!axumRoutes.has(route)) {
    fail(`Axum router missing route: ${route}`);
  }
}

for (const routeGroup of [
  "Health",
  "Roots",
  "Folders",
  "Media",
  "Thumbnails",
  "Preview",
  "Tasks",
  "FileOps",
  "Plugins"
]) {
  if (!routesRs.includes(`RouteGroup::${routeGroup}`)) {
    fail(`RouteGroup missing ${routeGroup}`);
  }
}

if (!apiModRs.includes('pub const API_PREFIX: &str = "/api"')) {
  fail('API_PREFIX must remain "/api"');
}
if (!apiModRs.includes("pub fn router(database: Database) -> Router")) {
  fail("api::router() is missing");
}
if (!apiModRs.includes("pub struct ApiConfig")) {
  fail("api::ApiConfig is missing");
}
if (!apiModRs.includes("router_with_config")) {
  fail("api::router_with_config() is missing");
}
if (!dbMigrationsRs.includes('include_str!("../../migrations/0001_initial.sql")')) {
  fail("db migration include path changed or missing");
}
if (!dbMigrationsRs.includes('include_str!("../../migrations/0002_task_progress.sql")')) {
  fail("db task progress migration include path changed or missing");
}
if (!dbMigrationsRs.includes('include_str!("../../migrations/0003_browsing_indexes.sql")')) {
  fail("db browsing indexes migration include path changed or missing");
}
if (!dbMigrationsRs.includes('include_str!("../../migrations/0004_thumbnail_state.sql")')) {
  fail("db thumbnail state migration include path changed or missing");
}
if (!dbMigrationsRs.includes('include_str!("../../migrations/0005_thumbnail_source_fingerprint.sql")')) {
  fail("db thumbnail source fingerprint migration include path changed or missing");
}
if (!dbMigrationsRs.includes('include_str!("../../migrations/0006_thumbnail_task_attempt_fingerprint.sql")')) {
  fail("db thumbnail task attempt fingerprint migration include path changed or missing");
}
if (!dbMigrationsRs.includes('include_str!("../../migrations/0007_task_status_contract.sql")')) {
  fail("db task status contract migration include path changed or missing");
}
if (!dbMigrationsRs.includes('include_str!("../../migrations/0008_task_attempt_generation.sql")')) {
  fail("db task attempt generation migration include path changed or missing");
}
if (!dbMigrationsRs.includes('include_str!("../../migrations/0010_media_fts_contentless_delete.sql")')) {
  fail("db media_fts contentless_delete migration include path changed or missing");
}
if (!dbMigrationsRs.includes('include_str!("../../migrations/0011_plugins_extended.sql")')) {
  fail("db plugins extended migration include path changed or missing");
}
if (!dbMigrationsRs.includes('include_str!("../../migrations/0013_preview_served_by.sql")')) {
  fail("db preview served_by migration include path changed or missing");
}
if (!dbModRs.includes("pub fn apply_migrations")) {
  fail("Database::apply_migrations is missing");
}
if (!mainRs.includes("database.apply_migrations()?")) {
  fail("main.rs must apply migrations before serving");
}
if (!mainRs.includes("axum::serve")) {
  fail("main.rs must start the Axum server");
}

if (!openApi.includes("name: X-Megle-Session")) {
  fail("OpenAPI must keep X-Megle-Session header security");
}
if (!mainRs.includes("MEGLE_SESSION_TOKEN")) {
  fail("main.rs must read MEGLE_SESSION_TOKEN");
}
if (!mainRs.includes("MEGLE_ALLOWED_ORIGIN")) {
  fail("main.rs must read MEGLE_ALLOWED_ORIGIN for dev CORS");
}
if (!mainRs.includes("MEGLE_SERVE_WEB")) {
  fail("main.rs must read MEGLE_SERVE_WEB to opt into static UI serving");
}
if (!mainRs.includes("MEGLE_WEB_DIR")) {
  fail("main.rs must read MEGLE_WEB_DIR to locate the built web UI");
}
if (!mainRs.includes("MEGLE_BASIC_AUTH")) {
  fail("main.rs must read MEGLE_BASIC_AUTH to enable HTTP Basic auth");
}
for (const value of ["pub web_dir", "pub basic_auth", "BasicAuthCredentials", "ServeDir", "ServeFile"]) {
  if (!apiModRs.includes(value)) {
    fail(`Core API static-serve / basic-auth wiring missing ${value}`);
  }
}
for (const value of ["X-Megle-Session", "tower_http::cors", "CorsLayer", "allow_headers"]) {
  if (!apiModRs.includes(value)) {
    fail(`Core API auth/CORS wiring missing ${value}`);
  }
}
if (apiModRs.includes("AllowOrigin::any") || apiModRs.includes("Any")) {
  fail("Core API CORS must not use wildcard origins");
}

for (const value of ["mtime_desc", "mtime_asc", "name_asc", "name_desc"]) {
  if (!openApi.includes(value) || !routesRs.includes(value)) {
    fail(`media sort value is not aligned: ${value}`);
  }
}

for (const value of ["image", "video", "other"]) {
  if (!openApi.includes(value) || !routesRs.includes(value)) {
    fail(`media kind value is not aligned: ${value}`);
  }
}

for (const value of ["grid_320"]) {
  if (!openApi.includes(value) || !routesRs.includes(value) || !thumbnailsRs.includes(value)) {
    fail(`thumbnail target value is not aligned: ${value}`);
  }
}
for (const value of ["pending", "queued", "ready", "failed", "skipped_small"]) {
  if (!openApi.includes(value) || !routesRs.includes(value) || !thumbnailsRs.includes(value)) {
    fail(`thumbnail status value is not aligned: ${value}`);
  }
}
for (const value of [
  "image/webp",
  "shortSidePx",
  "outputFormat",
  "ThumbnailResponse",
  "previewPlaceholder",
  "previewPlaceholderFormat",
  "target",
  "width",
  "height",
  "byteSize",
  "servedBy",
  "db_blob"
]) {
  if (!openApi.includes(value)) {
    fail(`OpenAPI thumbnail contract missing ${value}`);
  }
}
if (/cacheKey:\n/.test(openApi) || /cache_key:\s*String/.test(routesRs)) {
  fail("thumbnail public API must not expose disk cache keys");
}
for (const value of ["short_side_px", "output_format", "skipped_small", "image/webp"]) {
  if (!migrationSql.includes(value) || !thumbnailMigrationSql.includes(value)) {
    fail(`thumbnail schema migration missing ${value}`);
  }
}
for (const value of [
  "source_fingerprint",
  "state IN ('ready', 'skipped_small')"
]) {
  if (!thumbnailSourceMigrationSql.includes(value)) {
    fail(`thumbnail source fingerprint migration missing ${value}`);
  }
}
for (const value of ["thumbnail_source_fingerprint", "thumbnail_task_attempt_fingerprint"]) {
  if (!thumbnailTaskAttemptMigrationSql.includes(value)) {
    fail(`thumbnail task attempt fingerprint migration missing ${value}`);
  }
}
for (const value of ["served_by", "db_blob", "version, name, applied_at", "preview_served_by"]) {
  if (!previewServedByMigrationSql.includes(value)) {
    fail(`preview served_by migration missing ${value}`);
  }
}
for (const value of ["served_by", "db_blob"]) {
  if (!dbModRs.includes(value)) {
    fail(`thumbnail served_by ledger support missing ${value}`);
  }
}
if (/cache_key:\s*Some\(cache_key/.test(tasksRs)) {
  fail("thumbnail tasks must not persist runtime disk cache keys for DB blob thumbnails");
}
for (const value of [
  "DROP TABLE IF EXISTS thumbs_new",
  "BEGIN IMMEDIATE",
  "COMMIT",
  "pragma_foreign_key_check",
  "grid_320_explicit"
]) {
  if (!thumbnailMigrationSql.includes(value)) {
    fail(`thumbnail state migration hardening missing ${value}`);
  }
}

for (const value of [
  "AcceptedRootResponse",
  "ScanSummary",
  "RootRecord",
  "FolderRecord",
  "MediaRecord",
  "ErrorResponse",
  "rootFolderId",
  "lastScanAt",
  "foldersSeen",
  "mediaFilesSeen",
  "skippedFiles",
  "nextCursor"
]) {
  if (!openApi.includes(value)) {
    fail(`OpenAPI response schema missing ${value}`);
  }
}

for (const value of ["root_folder_id", "last_scan_at"]) {
  if (!dbModRs.includes(value)) {
    fail(`DB root response field missing ${value}`);
  }
}

for (const value of [
  "MediaRecord",
  "FolderRecord",
  "TaskRecord",
  "create_root_scan_task",
  "list_tasks",
  "get_task",
  "mark_task_running",
  "mark_task_succeeded",
  "mark_task_failed",
  "folder_exists",
  "mark_root_scanned"
]) {
  if (!dbModRs.includes(value)) {
    fail(`DB API missing ${value}`);
  }
}

if (!routesRs.includes("StatusCode::NOT_FOUND")) {
  fail("routes must expose NOT_FOUND for missing folder/media");
}

for (const value of ["decoder", "metadata", "action", "import-provider"]) {
  if (!pluginManifest.includes(value) || !pluginsRs.includes(value)) {
    fail(`plugin capability value is not aligned: ${value}`);
  }
}

for (const value of ["rename", "move", "delete_recycle", "delete_permanent"]) {
  if (!fsopsRs.includes(value)) {
    fail(`file operation value is missing: ${value}`);
  }
}

if (!dbModRs.includes('pub const WAL_MODE: &str = "WAL"')) {
  fail('WAL_MODE must remain "WAL"');
}
if (!migrationSql.includes("PRAGMA journal_mode = WAL")) {
  fail("initial migration must enable WAL journal mode");
}

function functionBody(name) {
  const match = routesRs.match(new RegExp(`async fn ${name}[^]*?\\n}\\n`, "m"));
  return match?.[0] ?? "";
}

function operationBlock(pathName, methodName) {
  const pathIndex = openApi.indexOf(`  ${pathName}:`);
  if (pathIndex === -1) {
    fail(`OpenAPI path missing ${pathName}`);
    return "";
  }
  const nextPathIndex = openApi.slice(pathIndex + 1).search(/\n  \/[^:\n]+:/);
  const pathBlock =
    nextPathIndex === -1
      ? openApi.slice(pathIndex)
      : openApi.slice(pathIndex, pathIndex + 1 + nextPathIndex);
  const methodIndex = pathBlock.indexOf(`    ${methodName}:`);
  if (methodIndex === -1) {
    fail(`OpenAPI operation missing ${methodName.toUpperCase()} ${pathName}`);
    return "";
  }
  const nextMethodIndex = pathBlock.slice(methodIndex + 1).search(/\n    [a-z]+:/);
  return nextMethodIndex === -1
    ? pathBlock.slice(methodIndex)
    : pathBlock.slice(methodIndex, methodIndex + 1 + nextMethodIndex);
}

for (const name of [
  "remove_root",
  "get_thumbnail",
  "enqueue_scan"
]) {
  if (!functionBody(name).includes("StatusCode::ACCEPTED")) {
    fail(`${name} must return StatusCode::ACCEPTED while it is queued/placeholder work`);
  }
}

const addRootBody = functionBody("add_root");
if (addRootBody.includes("scan_root")) {
  fail("POST /api/roots must enqueue root scans instead of scanning inline");
}
for (const value of ["create_root_scan_task", "enqueue_task", "task_id: Some", "scan: None"]) {
  if (!addRootBody.includes(value)) {
    fail(`POST /api/roots queued response missing ${value}`);
  }
}
const addRootOperation = operationBlock("/roots", "post");
if (!addRootOperation.includes('"400"') || !addRootOperation.includes("ErrorResponse")) {
  fail("OpenAPI POST /roots must document bad-root-path 400 ErrorResponse");
}

for (const value of [
  "TaskRecord",
  "TaskListResponse",
  "ScanTaskRequest",
  "TaskKind",
  "TaskStatus",
  "cancelled",
  "cancelTask",
  "retryTask",
  "itemsSeen",
  "itemsTotal",
  "foldersSeen",
  "mediaFilesSeen",
  "skippedFiles",
  "$ref: \"#/components/schemas/ScanTaskRequest\"",
  "$ref: \"#/components/schemas/TaskListResponse\""
]) {
  if (!openApi.includes(value)) {
    fail(`OpenAPI task contract missing ${value}`);
  }
}
for (const value of [
  "CHECK(status IN ('pending', 'running', 'succeeded', 'failed', 'cancelled'))",
  "CHECK(kind IN ('root_scan', 'thumbnail'))",
  "task_status_contract"
]) {
  if (!taskStatusContractMigrationSql.includes(value)) {
    fail(`task status contract migration missing ${value}`);
  }
}
for (const value of ["attempt_generation", "task_attempt_generation"]) {
  if (!taskAttemptGenerationMigrationSql.includes(value)) {
    fail(`task attempt generation migration missing ${value}`);
  }
}
if (openApi.includes("scans inline")) {
  fail("OpenAPI must not describe root creation as inline scanning");
}

const enqueueScanBody = functionBody("enqueue_scan");
for (const value of ["Json(payload): Json<ScanTaskRequest>", "get_root", "create_root_scan_task", "enqueue_task"]) {
  if (!enqueueScanBody.includes(value)) {
    fail(`POST /api/tasks/scan implementation missing ${value}`);
  }
}
const enqueueScanOperation = operationBlock("/tasks/scan", "post");
if (!enqueueScanOperation.includes('"409"') || !enqueueScanOperation.includes("ErrorResponse")) {
  fail("OpenAPI POST /tasks/scan must document disabled-root 409 ErrorResponse");
}
const cancelTaskOperation = operationBlock("/tasks/{taskId}/cancel", "post");
for (const value of ['operationId: cancelTask', '"200"', '"404"', '"409"', "ErrorResponse"]) {
  if (!cancelTaskOperation.includes(value)) {
    fail(`OpenAPI POST /tasks/{taskId}/cancel missing ${value}`);
  }
}
const retryTaskOperation = operationBlock("/tasks/{taskId}/retry", "post");
for (const value of ['operationId: retryTask', '"202"', '"404"', '"409"', "ErrorResponse"]) {
  if (!retryTaskOperation.includes(value)) {
    fail(`OpenAPI POST /tasks/{taskId}/retry missing ${value}`);
  }
}
const mediaOperation = operationBlock("/media", "get");
if (!mediaOperation.includes('"400"') || !mediaOperation.includes("ErrorResponse")) {
  fail("OpenAPI GET /media must document invalid query/cursor 400 ErrorResponse");
}
const thumbnailOperation = operationBlock("/media/{fileId}/thumbnail", "get");
for (const value of [
  "ThumbnailResponse",
  "target",
  '"200"',
  '"202"',
  '"404"',
  "ErrorResponse"
]) {
  if (!thumbnailOperation.includes(value)) {
    fail(`OpenAPI GET /media/{fileId}/thumbnail missing ${value}`);
  }
}
const folderChildrenOperation = operationBlock("/folders/{folderId}/children", "get");
if (!folderChildrenOperation.includes('"400"') || !folderChildrenOperation.includes("ErrorResponse")) {
  fail("OpenAPI GET /folders/{folderId}/children must document invalid cursor 400 ErrorResponse");
}
const folderAncestorsOperation = operationBlock("/folders/{folderId}/ancestors", "get");
for (const value of ["operationId: listFolderAncestors", '"200"', '"404"', "FolderListResponse", "ErrorResponse"]) {
  if (!folderAncestorsOperation.includes(value)) {
    fail(`OpenAPI GET /folders/{folderId}/ancestors missing ${value}`);
  }
}
if (!functionBody("list_tasks").includes("database.list_tasks")) {
  fail("GET /api/tasks must return persisted task rows");
}
if (
  !routesRs.includes("tokio::task::spawn_blocking") ||
  !routesRs.includes("run_read_database") ||
  !routesRs.includes("run_shared_database")
) {
  fail("Core database route helpers must offload synchronous SQLite work with spawn_blocking");
}
for (const name of [
  "list_roots",
  "list_folder_children",
  "list_folder_ancestors",
  "list_media",
  "get_media",
  "get_thumbnail_blob",
  "get_preview",
  "list_tasks",
  "search_media"
]) {
  if (!functionBody(name).includes("run_read_database")) {
    fail(`${name} route must use the read database spawn_blocking helper`);
  }
}
for (const name of ["get_thumbnail", "enqueue_interactive_folder_scan", "sync_thumbnail_priority_scope"]) {
  const body = functionBody(name);
  // All writes go through the single writer actor. A route may submit writes
  // via the `run_shared_database`/`run_dedicated_or_shared_database` helpers
  // (which now route to the writer) or call `state.writer.write` directly.
  // get_thumbnail reads on the read connection first, then submits only the
  // pending-path enqueue through `writer.write`.
  if (
    !body.includes("run_shared_database") &&
    !body.includes("run_dedicated_or_shared_database") &&
    !/\.writer\s*\.\s*write/.test(body)
  ) {
    fail(`${name} route must submit writes through the single DbWriter (writer.write or a writer-backed helper)`);
  }
}
for (const name of ["get_thumbnail_cache_stats"]) {
  if (!functionBody(name).includes("run_read_database")) {
    fail(`${name} route must use the read database spawn_blocking helper`);
  }
  // The cache-stats aggregate scans every file in scope and holds a WAL read
  // snapshot while it runs. It is polled on an interval, and an aborted poll does
  // not cancel the spawn_blocking query, so without single-flight + a TTL cache
  // concurrent polls pile up and pin the WAL checkpoint (the WAL then grows to
  // gigabytes and the whole DB crawls). Keep the single-flight cache in place.
  if (!functionBody(name).includes("THUMBNAIL_CACHE_STATS_CACHE")) {
    fail(
      `${name} must single-flight + TTL-cache the aggregate (THUMBNAIL_CACHE_STATS_CACHE) so polled requests cannot pile up and pin the WAL`
    );
  }
}
for (const name of ["enqueue_thumbnail_cache", "clear_thumbnail_cache"]) {
  const body = functionBody(name);
  // Either offload helper is acceptable (both use spawn_blocking).
  // enqueue_thumbnail_cache uses the dedicated-connection variant so its bulk
  // 256-4096-row task insert can't head-of-line block the per-scroll
  // priority-scope write on the shared write mutex.
  if (!body.includes("run_shared_database") && !body.includes("run_dedicated_or_shared_database")) {
    fail(`${name} route must offload shared database writes from the async handler`);
  }
}
for (const [name, action] of [
  ["cancel_task", "cancel_task"],
  ["retry_task", "retry_task"]
]) {
  if (!new RegExp(`\\.${action}\\(`).test(functionBody(name))) {
    fail(`${name} route must call database.${action}`);
  }
}
const thumbnailBody = functionBody("get_thumbnail");
for (const value of ["Json<ThumbnailResponse>", "get_thumbnail", "target", "StatusCode::OK", "StatusCode::ACCEPTED"]) {
  if (!thumbnailBody.includes(value)) {
    fail(`GET /api/media/{fileId}/thumbnail implementation missing ${value}`);
  }
}
const thumbnailBlobBody = functionBody("get_thumbnail_blob");
for (const value of ["get_thumb_blob", "target", "db_blob"]) {
  if (!thumbnailBlobBody.includes(value)) {
    fail(`GET /api/media/{fileId}/thumbnail/blob implementation missing ${value}`);
  }
}
const thumbnailBlobOperation = operationBlock("/media/{fileId}/thumbnail/blob", "get");
for (const value of ["target", "image/webp", "x-megle-served-by", "db_blob"]) {
  if (!thumbnailBlobOperation.includes(value)) {
    fail(`OpenAPI GET /media/{fileId}/thumbnail/blob missing ${value}`);
  }
}
const thumbnailCacheStatsOperation = operationBlock("/thumbnails/cache/stats", "get");
for (const value of [
  "operationId: getThumbnailCacheStats",
  "rootId",
  "folderId",
  "includeDescendants",
  "ThumbnailCacheStatsResponse"
]) {
  if (!thumbnailCacheStatsOperation.includes(value)) {
    fail(`OpenAPI GET /thumbnails/cache/stats missing ${value}`);
  }
}
const thumbnailCacheTaskOperation = operationBlock("/tasks/thumbnail-cache", "post");
for (const value of ["operationId: enqueueThumbnailCache", "ThumbnailCacheTaskRequest", "ThumbnailCacheEnqueueResponse"]) {
  if (!thumbnailCacheTaskOperation.includes(value)) {
    fail(`OpenAPI POST /tasks/thumbnail-cache missing ${value}`);
  }
}
if (!openApi.includes("fileIds:")) {
  fail("OpenAPI thumbnail cache task schema missing fileIds");
}
for (const value of ["retryFailedAndStale", "staleOrMissing", "missingOnly"]) {
  if (!openApi.includes(value)) {
    fail(`OpenAPI thumbnail cache refresh mode schema missing ${value}`);
  }
}
const thumbnailCacheClearOperation = operationBlock("/thumbnails/cache/clear", "post");
for (const value of [
  "operationId: clearThumbnailCache",
  "ThumbnailCacheClearResponse"
]) {
  if (!thumbnailCacheClearOperation.includes(value)) {
    fail(`OpenAPI POST /thumbnails/cache/clear missing ${value}`);
  }
}
const previewOperation = operationBlock("/media/{fileId}/preview", "get");
for (const value of [
  "operationId: getPreview",
  '"200"',
  "application/octet-stream",
  '"404"',
  "ErrorResponse"
]) {
  if (!previewOperation.includes(value)) {
    fail(`OpenAPI GET /media/{fileId}/preview original media contract missing ${value}`);
  }
}
if (previewOperation.includes('"202"')) {
  fail("OpenAPI GET /media/{fileId}/preview must describe original bytes, not queued placeholder work");
}
const previewBody = functionBody("get_preview");
for (const value of [
  "resolve_file_source_path",
  "tokio::fs::File::open",
  "Body::from_stream",
  "CONTENT_TYPE",
  "preview_content_type"
]) {
  if (!previewBody.includes(value)) {
    fail(`GET /api/media/{fileId}/preview implementation must stream original media bytes: missing ${value}`);
  }
}
if (previewBody.includes("StatusCode::ACCEPTED")) {
  fail("GET /api/media/{fileId}/preview must not be a 202 Accepted placeholder");
}
for (const value of [
  '"/api/thumbnails/cache/stats"',
  '"/api/thumbnails/cache/clear"',
  '"/api/tasks/thumbnail-cache"'
]) {
  if (!routesRs.includes(value)) {
    fail(`PHASE1_API_PATHS must include ${value}`);
  }
}
const thumbnailCacheStatsBody = functionBody("get_thumbnail_cache_stats");
for (const value of [
  "thumbnail_cache_scope(query)",
  "get_thumbnail_cache_stats",
  "thumbnail_cache_stats_response"
]) {
  if (!thumbnailCacheStatsBody.includes(value)) {
    fail(`GET /api/thumbnails/cache/stats implementation missing ${value}`);
  }
}
const thumbnailCacheEnqueueBody = functionBody("enqueue_thumbnail_cache");
for (const value of [
  "ThumbnailCacheRefreshMode::from_wire_value",
  "file_ids",
  "enqueue_thumbnail_cache_tasks",
  "enqueue_thumbnail_cache_tasks_for_file_ids",
  "enqueue_thumbnail_task_wakeup",
  "THUMBNAIL_BULK_PRIORITY",
  "thumbnail_cache_enqueue_response"
]) {
  if (!thumbnailCacheEnqueueBody.includes(value)) {
    fail(`POST /api/tasks/thumbnail-cache implementation missing ${value}`);
  }
}
const thumbnailCacheClearBody = functionBody("clear_thumbnail_cache");
for (const value of ["clear_thumbnail_cache", "thumbnail_cache_clear_response"]) {
  if (!thumbnailCacheClearBody.includes(value)) {
    fail(`POST /api/thumbnails/cache/clear implementation missing ${value}`);
  }
}
for (const value of [
  "offset:",
  "Zero-based media row offset for viewport-windowed browsing",
  "totalCount"
]) {
  if (!openApi.includes(value)) {
    fail(`OpenAPI media browsing viewport-window contract missing ${value}`);
  }
}
for (const value of [
  "query.offset",
  "use_offset",
  "offset_clause",
  "is_first_media_page",
  "list_media_page_reports_total_count_on_first_page_only"
]) {
  if (!dbModRs.includes(value)) {
    fail(`Core media browsing viewport-window implementation missing ${value}`);
  }
}
for (const value of ["?2 IS NULL OR files.root_id", "?3 IS NULL OR files.folder_id", "?4 IS NULL OR media.kind"]) {
  if (dbModRs.includes(value)) {
    fail(`media browsing query must build dynamic predicates instead of optional OR predicate ${value}`);
  }
}
for (const value of ["next_cursor", "decode_media_cursor", "encode_media_cursor", "disable_root"]) {
  if (!dbModRs.includes(value)) {
    fail(`Core browsing hardening missing ${value}`);
  }
}
for (const value of ["canonicalize", "StatusCode::BAD_REQUEST", "StatusCode::CONFLICT", "parse_media_sort", "parse_media_kind"]) {
  if (!routesRs.includes(value)) {
    fail(`Core root validation/removal routes missing ${value}`);
  }
}
for (const value of ["cancel_root_scan_tasks", "root_enabled", "fail_pending_root_scan_tasks_for_disabled_roots"]) {
  if (!dbModRs.includes(value) && !tasksRs.includes(value)) {
    fail(`Core scan cancellation hardening missing ${value}`);
  }
}
if (tasksRs.includes("Arc<Mutex<Database>>")) {
  fail("background scan worker must not hold the API Arc<Mutex<Database>> during scans");
}
if (!tasksRs.includes("start_worker(worker_database: Database, writer: DbWriter)")) {
  fail("background scan worker must own a separate read/scheduling Database handle and receive the single DbWriter");
}
if (!/mark_task_failed_for_attempt\(\s*task_id,\s*attempt_generation,\s*&error\.to_string\(\),\s*\)/.test(routesRs)) {
  fail("enqueue failures must mark the created task failed through an attempt guard before returning an API error");
}
for (const value of [
  "WHERE id = ?2 AND status = 'pending'",
  "WHERE id = ?2 AND status = 'running'",
  "attempt_generation = ?3",
  "task_attempt_is_current",
  "update_task_scan_progress",
  "WHERE id = ?3 AND status IN ('pending', 'running')",
  "WHERE id = ?2 AND status IN ('pending', 'running')",
  "WHERE id = ?2 AND status IN ('failed', 'cancelled')",
  "ensure_one_task_attempt_updated",
  "ensure_one_task_updated"
]) {
  if (!dbModRs.includes(value)) {
    fail(`DB task status transition guard missing ${value}`);
  }
}

// Phase 5 metadata + search contract alignment.
for (const fragment of [
  "  /tags:",
  "  /tags/{tagId}:",
  "  /media/{fileId}/metadata:",
  "  /media/{fileId}/tags:",
  "  /media/{fileId}/tags/{tagId}:",
  "  /search:"
]) {
  if (!openApi.includes(fragment)) {
    fail(`OpenAPI must declare ${fragment}`);
  }
}

for (const value of [
  "TagRecord",
  "TagListResponse",
  "CreateTagRequest",
  "DeleteTagResponse",
  "UserMetadataRecord",
  "UserMetadataUpdate",
  "FileTagsResponse",
  "AddFileTagRequest",
  "SetFileTagsRequest"
]) {
  if (!openApi.includes(`#/components/schemas/${value}`)) {
    fail(`OpenAPI must reference component schema ${value}`);
  }
  if (!openApi.includes(`    ${value}:`)) {
    fail(`OpenAPI must define schema ${value}`);
  }
}

const searchOperation = operationBlock("/search", "get");
for (const value of [
  "operationId: searchMedia",
  "rating_desc",
  "rating_asc",
  "mtime_desc",
  "name_asc",
  "minRating",
  "tagId",
  "MediaListResponse"
]) {
  if (!searchOperation.includes(value)) {
    fail(`OpenAPI GET /search missing ${value}`);
  }
}

const mediaSchemaBlock = openApi.match(/\n    MediaRecord:[\s\S]*?(?=\n    [A-Z][A-Za-z]+:)/);
if (!mediaSchemaBlock) {
  fail("OpenAPI must define MediaRecord schema block");
} else {
  for (const value of ["rating", "favorite", "note", "tagIds"]) {
    if (!mediaSchemaBlock[0].includes(value)) {
      fail(`OpenAPI MediaRecord must include ${value}`);
    }
  }
}

for (const value of [
  '"/api/tags"',
  '"/api/tags/{tagId}"',
  '"/api/media/{fileId}/metadata"',
  '"/api/media/{fileId}/tags"',
  '"/api/media/{fileId}/tags/{tagId}"',
  '"/api/search"'
]) {
  if (!routesRs.includes(value)) {
    fail(`PHASE1_API_PATHS must include ${value}`);
  }
}

for (const value of ["SEARCH_SORT_VALUES", "rating_desc", "rating_asc"]) {
  if (!routesRs.includes(value)) {
    fail(`routes.rs must declare ${value}`);
  }
}

for (const value of [
  "list_tags",
  "create_tag",
  "delete_tag",
  "get_user_metadata",
  "upsert_user_metadata_partial",
  "set_file_tags",
  "add_file_tag",
  "remove_file_tag",
  "sync_media_fts_for_file",
  "search_media_page",
  "TagRecord",
  "UserMetadataRecord",
  "UserMetadataPatch",
  "SearchQuery"
]) {
  if (!dbModRs.includes(value)) {
    fail(`db/mod.rs must expose ${value}`);
  }
}

// Phase 6 file operations contract alignment.
for (const fragment of [
  "  /file-ops/rename:",
  "  /file-ops/move:",
  "  /file-ops/delete:",
  "  /file-ops:"
]) {
  if (!openApi.includes(fragment)) {
    fail(`OpenAPI must declare ${fragment}`);
  }
}

for (const value of [
  "FileOperationRecord",
  "FileOperationKind",
  "FileOperationStatus",
  "FileOperationsResponse",
  "FileOperationListResponse",
  "RenameRequest",
  "MoveRequest",
  "DeleteRequest"
]) {
  if (!openApi.includes(`#/components/schemas/${value}`)) {
    fail(`OpenAPI must reference component schema ${value}`);
  }
  if (!openApi.includes(`    ${value}:`)) {
    fail(`OpenAPI must define schema ${value}`);
  }
}

for (const value of ["rename", "move", "delete_recycle", "delete_permanent"]) {
  if (!openApi.includes(value)) {
    fail(`OpenAPI FileOperationKind enum missing ${value}`);
  }
}

for (const value of [
  '"/api/file-ops/rename"',
  '"/api/file-ops/move"',
  '"/api/file-ops/delete"',
  '"/api/file-ops"'
]) {
  if (!routesRs.includes(value)) {
    fail(`PHASE1_API_PATHS must include ${value}`);
  }
}

for (const value of [
  "fsops::rename",
  "fsops::move_items",
  "fsops::delete",
  "fsops::list_recent",
  "list_file_operations",
  "map_fsops_error",
  "FileOperationRecord",
  "FileOperationsResponse"
]) {
  if (!routesRs.includes(value)) {
    fail(`routes.rs must wire ${value}`);
  }
}

for (const value of [
  "BEGIN IMMEDIATE",
  "TransactionBehavior::Immediate",
  "FileOperationRecord",
  "FsOpsError",
  "FsOpsErrorCode",
  "cross_root",
  "trash::delete",
  "remove_file",
  "remove_dir_all",
  "validate_name",
  "is_windows_reserved",
  "list_recent",
  "rename(",
  "move_items(",
  "delete("
]) {
  if (!fsopsRs.includes(value)) {
    fail(`fsops/mod.rs must include ${value}`);
  }
}

if (!coreCargo.includes("trash =")) {
  fail("crates/core/Cargo.toml must depend on trash");
}

if (!process.exitCode) {
  console.log("PASS: core api contract alignment");
}
