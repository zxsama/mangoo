use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::http::{header, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::db::{Database, DbWriter, ThumbnailCacheRefreshMode, ThumbnailCacheScope};
use crate::roots::reachability::{refresh_root_reachability, RootReachability};
use crate::tasks::{start_worker, TaskSender};
use crate::watch::{start_watcher, WatcherHandle};

pub mod routes;

#[allow(dead_code)]
pub const API_PREFIX: &str = "/api";
pub const SESSION_HEADER: &str = "X-Megle-Session";
pub(crate) const NOTIFICATION_OUTBOX_READ_CONCURRENCY: usize = 4;
/// Concurrent preview decodes. Deliberately small: each one can hold a
/// full-resolution decoded image, and they share Tokio's blocking pool with
/// every `run_read_database` call.
pub(crate) const PREVIEW_DECODE_CONCURRENCY: usize = 3;

#[cfg(test)]
pub(crate) struct NotificationOutboxTestProbe {
    read_started: tokio::sync::mpsc::UnboundedSender<()>,
    waiter_registered: tokio::sync::mpsc::UnboundedSender<()>,
    read_release: Option<Arc<tokio::sync::Semaphore>>,
    active_reads: AtomicUsize,
    max_active_reads: AtomicUsize,
    active_waiters: AtomicUsize,
}

#[cfg(test)]
enum NotificationOutboxTestActivityKind {
    Read,
    Waiter,
}

#[cfg(test)]
pub(crate) struct NotificationOutboxTestActivity {
    probe: Arc<NotificationOutboxTestProbe>,
    kind: NotificationOutboxTestActivityKind,
}

#[cfg(test)]
impl NotificationOutboxTestProbe {
    pub(crate) fn new(
        block_reads: bool,
    ) -> (
        Arc<Self>,
        tokio::sync::mpsc::UnboundedReceiver<()>,
        tokio::sync::mpsc::UnboundedReceiver<()>,
    ) {
        let (read_started, read_started_rx) = tokio::sync::mpsc::unbounded_channel();
        let (waiter_registered, waiter_registered_rx) = tokio::sync::mpsc::unbounded_channel();
        (
            Arc::new(Self {
                read_started,
                waiter_registered,
                read_release: block_reads.then(|| Arc::new(tokio::sync::Semaphore::new(0))),
                active_reads: AtomicUsize::new(0),
                max_active_reads: AtomicUsize::new(0),
                active_waiters: AtomicUsize::new(0),
            }),
            read_started_rx,
            waiter_registered_rx,
        )
    }

    pub(crate) fn begin_read(self: &Arc<Self>) -> NotificationOutboxTestActivity {
        let active = self.active_reads.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active_reads.fetch_max(active, Ordering::SeqCst);
        let _ = self.read_started.send(());
        NotificationOutboxTestActivity {
            probe: Arc::clone(self),
            kind: NotificationOutboxTestActivityKind::Read,
        }
    }

    pub(crate) async fn wait_for_read_release(&self) {
        if let Some(release) = &self.read_release {
            if let Ok(permit) = release.acquire().await {
                permit.forget();
            }
        }
    }

    pub(crate) fn release_reads(&self, count: usize) {
        if let Some(release) = &self.read_release {
            release.add_permits(count);
        }
    }

    pub(crate) fn begin_waiter(self: &Arc<Self>) -> NotificationOutboxTestActivity {
        self.active_waiters.fetch_add(1, Ordering::SeqCst);
        let _ = self.waiter_registered.send(());
        NotificationOutboxTestActivity {
            probe: Arc::clone(self),
            kind: NotificationOutboxTestActivityKind::Waiter,
        }
    }

    pub(crate) fn active_reads(&self) -> usize {
        self.active_reads.load(Ordering::SeqCst)
    }

    pub(crate) fn max_active_reads(&self) -> usize {
        self.max_active_reads.load(Ordering::SeqCst)
    }

    pub(crate) fn active_waiters(&self) -> usize {
        self.active_waiters.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
impl Drop for NotificationOutboxTestActivity {
    fn drop(&mut self) {
        match self.kind {
            NotificationOutboxTestActivityKind::Read => {
                self.probe.active_reads.fetch_sub(1, Ordering::SeqCst);
            }
            NotificationOutboxTestActivityKind::Waiter => {
                self.probe.active_waiters.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub writer: DbWriter,
    pub(crate) notification_outbox_wake: Arc<tokio::sync::Notify>,
    /// Bounds only the short SQLite reads around long polling. Requests release
    /// their permit before awaiting `notification_outbox_wake`, so the desktop
    /// drain loop is never pinned behind idle waiters.
    pub(crate) notification_outbox_read_gate: Arc<tokio::sync::Semaphore>,
    /// Bounds concurrent preview decodes.
    ///
    /// Every thumbnail decode already runs under a per-bucket semaphore; the
    /// preview route had none, so one bare `spawn_blocking` per request could
    /// stack unboundedly on the shared blocking pool — each holding a full
    /// decoded image (hundreds of MB for a large source) and delaying every
    /// read route sharing that pool. `spawn_blocking` is also uncancellable, so
    /// a client that navigated away still pays for its decode to completion.
    pub(crate) preview_decode_gate: Arc<tokio::sync::Semaphore>,
    #[cfg(test)]
    pub(crate) notification_outbox_test_probe: Option<Arc<NotificationOutboxTestProbe>>,
    /// A reopened connection used for file-operation preflight reads and plugin
    /// discovery. File-operation journal writes are submitted as short jobs to
    /// `writer`; filesystem syscalls run outside both connections on Tokio's
    /// blocking pool. `None` for in-memory/test databases.
    pub fsops_database: Option<Arc<std::sync::Mutex<Database>>>,
    pub task_queue: TaskSender,
    read_database_path: Option<PathBuf>,
    /// Resolved at startup so route handlers (notably
    /// `POST /api/plugins/discover`) agree with `main.rs` on where to look
    /// for plugin manifests. `None` when the API is built without a
    /// configured directory; the handler falls back to the same default
    /// rules as startup.
    pub plugins_dir: Option<PathBuf>,
    _watcher: Option<Arc<WatcherHandle>>,
    /// Per-root reachability cache, refreshed by a background probe every 30s
    /// (started in `router_with_config`) and overlaid onto `RootRecord.reachable`
    /// by `routes::list_roots`. Absent root ids default to reachable (`true`) so
    /// a not-yet-probed root never reads as offline right after cold start.
    pub root_reachability: RootReachability,
}

impl AppState {
    pub fn new(database: Database) -> Self {
        if let Some(worker_database) = database.reopen().expect("reopen worker database") {
            return Self::new_with_worker(database, worker_database);
        }

        // In-memory/test DB: no reopen possible, so the writer owns the sole
        // connection and reads fall back to it (no read_database_path).
        flush_notification_outbox_at_startup(&database);
        if let Err(error) = database.recover_interrupted_notification_operations() {
            tracing::error!(
                %error,
                "startup notification recovery failed; Core will continue with observable pending state"
            );
        }
        let notification_outbox_wake = database.notification_outbox_wake();
        let (task_queue, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        Self {
            writer: DbWriter::spawn(database),
            notification_outbox_wake,
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(PREVIEW_DECODE_CONCURRENCY)),
            #[cfg(test)]
            notification_outbox_test_probe: None,
            fsops_database: None,
            task_queue,
            read_database_path: None,
            plugins_dir: None,
            _watcher: None,
            root_reachability: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn new_with_worker(database: Database, worker_database: Database) -> Self {
        flush_notification_outbox_at_startup(&database);
        let notification_outbox_wake = database.notification_outbox_wake();
        let read_database_path = database.path().map(Path::to_path_buf);
        let watcher_database = database.reopen().expect("reopen watcher database");
        // Reopened before `database` is moved into the writer. File operations
        // use it only for preflight reads; their journal writes use `writer`.
        // Plugin discovery retains its existing dedicated-connection boundary.
        let fsops_database = database
            .reopen()
            .expect("reopen fsops database")
            .map(|db| Arc::new(std::sync::Mutex::new(db)));
        // The primary connection becomes the single writer. The worker + watcher
        // keep their own connections for READ/scheduling only; every hot-path
        // write goes through this handle, so there is one WAL writer for the
        // contended path and no lock contention.
        let writer = DbWriter::spawn(database);
        let task_queue = start_worker(worker_database, writer.clone());
        let watcher = watcher_database.map(|watcher_database| {
            Arc::new(start_watcher(
                watcher_database,
                task_queue.clone(),
                writer.clone(),
            ))
        });
        Self {
            writer,
            notification_outbox_wake,
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(PREVIEW_DECODE_CONCURRENCY)),
            #[cfg(test)]
            notification_outbox_test_probe: None,
            fsops_database,
            task_queue,
            read_database_path,
            plugins_dir: None,
            _watcher: watcher,
            root_reachability: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn open_read_database(&self) -> anyhow::Result<Option<Database>> {
        let Some(path) = &self.read_database_path else {
            return Ok(None);
        };
        Ok(Some(Database::open(path)?))
    }

    /// Whether a dedicated read connection can be opened (the database is
    /// file-backed). In-memory/test databases return `false`, and read
    /// handlers fall back to the shared connection.
    pub fn has_read_database(&self) -> bool {
        self.read_database_path.is_some()
    }
}

fn flush_notification_outbox_at_startup(database: &Database) {
    if let Err(error) = database.flush_dirty_notification_heads() {
        tracing::error!(
            %error,
            "startup notification dirty-head flush failed; Core will continue with persisted head state"
        );
    }
}

#[derive(Clone, Debug, Default)]
pub struct ApiConfig {
    pub session_token: Option<String>,
    pub allowed_origin: Option<HeaderValue>,
    /// When set, the directory holding the built web UI (`apps/web/dist`).
    /// Triggered by `MEGLE_SERVE_WEB=1`. When `None`, no static UI is served
    /// and desktop dev keeps its current behavior.
    pub web_dir: Option<PathBuf>,
    /// When set, every request except `/api/health` requires HTTP Basic auth
    /// with these credentials. Format is `(user, pass)`.
    pub basic_auth: Option<BasicAuthCredentials>,
    /// Plugins directory resolved at startup. Threaded into [`AppState`] so
    /// the `POST /api/plugins/discover` handler agrees with the path
    /// `main.rs` walked at startup. `None` falls back to the same default
    /// (`<db_path.parent>/plugins`, then `./plugins`) inside the handler.
    pub plugins_dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct BasicAuthCredentials {
    pub user: String,
    pub pass: String,
}

impl BasicAuthCredentials {
    pub fn parse(raw: &str) -> Option<Self> {
        let (user, pass) = raw.split_once(':')?;
        if user.is_empty() {
            return None;
        }
        Some(Self {
            user: user.to_string(),
            pass: pass.to_string(),
        })
    }

    fn expected_header(&self) -> String {
        let token = format!("{}:{}", self.user, self.pass);
        format!("Basic {}", BASE64_STANDARD.encode(token.as_bytes()))
    }
}

#[allow(dead_code)]
pub fn router(database: Database) -> Router {
    router_with_config(database, ApiConfig::default())
}

pub fn router_with_config(database: Database, config: ApiConfig) -> Router {
    let mut state = AppState::new(database);
    state.plugins_dir = config.plugins_dir.clone();
    // Debug self-test: submit a trivial write through the writer actor right at
    // startup (independent of any client) and log the outcome, so a deadlocked
    // actor is diagnosable without the renderer. Gated by the master debug flag.
    if crate::debug::enabled() {
        let writer = state.writer.clone();
        tokio::spawn(async move {
            crate::debug_log!("writer-selftest submitting startup probe write");
            let started = std::time::Instant::now();
            let result = writer.write(|db| Ok(db.list_tags()?.len())).await;
            crate::debug_log!(
                "writer-selftest probe write returned in {}ms: {:?}",
                started.elapsed().as_millis(),
                result.as_ref().map(|n| *n).map_err(|e| e.to_string())
            );
        });
    }
    spawn_root_reachability_probe(&state);
    spawn_transient_retry_sweep(&state);
    let api_router = routes::router(state).layer(middleware::from_fn_with_state(
        config.clone(),
        require_session_token,
    ));

    let mut app = api_router;

    // Static UI serving is opt-in. When `web_dir` is set the UI is mounted
    // alongside the API router with a SPA `index.html` fallback for unknown
    // non-API paths. API requests that don't match a route still 404.
    if let Some(web_dir) = config.web_dir.as_ref() {
        let index_path = web_dir.join("index.html");
        let assets_dir = web_dir.join("assets");

        // Long-lived cache for hashed asset filenames under `assets/`.
        let assets_service = ServeDir::new(&assets_dir);
        let assets_with_cache = tower::ServiceBuilder::new()
            .layer(SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            ))
            .service(assets_service);

        // Top-level ServeDir with `index.html` fallback handles the SPA root,
        // top-level static files, and unknown client-side routes. We mark the
        // bundle as `no-cache` so the index document never goes stale; hashed
        // assets keep their immutable cache via the `/assets` mount above.
        let serve_dir = ServeDir::new(web_dir).fallback(ServeFile::new(&index_path));
        let static_with_cache = tower::ServiceBuilder::new()
            .layer(SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-cache"),
            ))
            .service(serve_dir);

        // Build the static router separately so its fallback only catches
        // non-API paths. The path filter middleware ensures `/api/...` paths
        // still 404 instead of being rewritten to index.html.
        let static_router = Router::new()
            .nest_service("/assets", assets_with_cache)
            .fallback_service(static_with_cache)
            .layer(middleware::from_fn(reject_api_paths));

        app = app.merge(static_router);
    }

    let app = match config.allowed_origin {
        Some(origin) => app.layer(cors_layer(origin)),
        None => app,
    };

    if let Some(creds) = config.basic_auth {
        app.layer(middleware::from_fn_with_state(
            BasicAuthState::new(creds),
            require_basic_auth,
        ))
    } else {
        app
    }
}

/// Starts the background reachability probe: refreshes `state.root_reachability`
/// immediately (so a cold start has real values as soon as possible), then again
/// every 30s for as long as the process runs. NAS shares flap on transient network
/// blips; without this loop a root probed unreachable once would stay stuck that
/// way until the next explicit action, which is what let ~19k thumbnails go
/// permanently `failed` during a brief outage instead of just skipping a cycle.
fn spawn_root_reachability_probe(state: &AppState) {
    let reach = state.root_reachability.clone();
    let probe_state = state.clone();
    tokio::spawn(async move {
        loop {
            match enabled_root_paths(probe_state.clone()).await {
                Ok(roots) => refresh_root_reachability(&reach, &roots),
                Err(error) => {
                    tracing::warn!(%error, "root reachability probe: failed to list roots");
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}

/// Upper bound on how many transient-failure retries this sweep enqueues per
/// root per tick. The candidate query + enqueue loop runs inside one
/// transaction on the single writer, so an unbounded batch on a root with a
/// huge backlog (e.g. the ~19k thumbnails a NAS blip left `failed`) would hold
/// the writer for a long single job; capping keeps each tick's writer hold
/// short and lets the backlog drain over several ticks instead.
const TRANSIENT_RETRY_SWEEP_PER_ROOT_LIMIT: usize = 500;

/// Starts the background transient-retry sweep: every 60s, re-enqueues
/// `failed` thumbnails classified `transient` (the `RetryFailedAndStale`
/// candidate gate, which already restricts to `failure_kind = 'transient'`)
/// for every root the reachability cache currently reports reachable, at
/// `bulk` priority so it never competes with interactive generation.
/// `spawn_root_reachability_probe` only keeps that cache fresh — nothing else
/// re-queues the backlog a transient NAS outage leaves stuck `failed`, which
/// is exactly what let ~19k thumbnails go permanently failed instead of
/// clearing once the share came back. Unreachable roots are skipped entirely
/// so an offline NAS is never hammered with retry enqueues. A failed sweep
/// tick is logged and skipped; it never crashes Core, and the next tick tries
/// again.
fn spawn_transient_retry_sweep(state: &AppState) {
    let sweep_state = state.clone();
    tokio::spawn(async move {
        loop {
            let result = sweep_transient_retries(&sweep_state).await;
            match result {
                Ok(enqueued_count) if enqueued_count > 0 => {
                    tracing::info!(
                        enqueued_count,
                        "transient retry sweep re-enqueued failed thumbnails"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "transient retry sweep failed; will retry next tick");
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
}

/// Production driver for the transient-retry sweep, invoked once per tick by
/// `spawn_transient_retry_sweep`. Enumerating roots is a read, so it uses a
/// dedicated read-only connection when the database is file-backed (same
/// pattern as `enabled_root_paths`) instead of `run_transient_retry_sweep`'s
/// write-connection `list_roots` call, which used to hold the single writer
/// for the entire multi-root enumeration. Reachability filtering happens here,
/// outside any writer closure, and each reachable root then gets its *own*
/// `writer.write(...)` job bounded to that one root's enqueue — a many-root
/// NAS library can no longer occupy the writer actor for a whole sweep tick;
/// interactive writes can interleave between per-root jobs. In-memory/test
/// databases (no separate read connection) fall back to running the whole
/// sweep as a single writer job via `run_transient_retry_sweep`, the same
/// kernel the unit tests exercise directly.
async fn sweep_transient_retries(state: &AppState) -> anyhow::Result<usize> {
    if !state.has_read_database() {
        let reach = state.root_reachability.clone();
        return state
            .writer
            .write(move |database| run_transient_retry_sweep(database, &reach))
            .await;
    }

    let read_state = state.clone();
    let root_ids = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<i64>> {
        let database = read_state
            .open_read_database()?
            .ok_or_else(|| anyhow::anyhow!("read database unexpectedly unavailable"))?;
        Ok(database
            .list_roots()?
            .into_iter()
            .map(|root| root.id)
            .collect())
    })
    .await??;

    // Snapshot the cache once up front, outside any writer job, rather than
    // holding the lock across the per-root dispatch loop below.
    let reachable_by_root_id: HashMap<i64, bool> = state
        .root_reachability
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    let mut enqueued_count = 0usize;
    for root_id in root_ids {
        let is_reachable = *reachable_by_root_id.get(&root_id).unwrap_or(&true);
        if !is_reachable {
            continue;
        }
        enqueued_count += state
            .writer
            .write(move |database| enqueue_transient_retry_for_root(database, root_id))
            .await?;
    }
    Ok(enqueued_count)
}

/// Re-enqueues one root's `failed AND failure_kind = 'transient'` thumbnails
/// via the existing `RetryFailedAndStale` bulk enqueue path, capped at
/// `TRANSIENT_RETRY_SWEEP_PER_ROOT_LIMIT`. Shared by `sweep_transient_retries`
/// (one writer job per root in production) and `run_transient_retry_sweep`
/// (the in-memory/test fallback that loops over all roots in one job).
fn enqueue_transient_retry_for_root(database: &Database, root_id: i64) -> anyhow::Result<usize> {
    let scope = ThumbnailCacheScope {
        root_id: Some(root_id),
        folder_id: None,
        include_descendants: false,
    };
    // `RetryTransientFailedOnly`, NOT `RetryFailedAndStale`: the sweep retries
    // only transient failures. `RetryFailedAndStale` also matches `missing`, so
    // it turned this 60s background sweep into a whole-library thumbnail
    // generator (500 missing/root/tick) that ignored the user's generation mode.
    let result = database.enqueue_thumbnail_cache_tasks(
        scope,
        ThumbnailCacheRefreshMode::RetryTransientFailedOnly,
        TRANSIENT_RETRY_SWEEP_PER_ROOT_LIMIT,
        None,
    )?;
    Ok(result.accepted_count)
}

/// Pure sweep logic, run once: lists every enabled root, skips any the
/// reachability cache marks unreachable (a cache miss defaults to reachable,
/// matching `routes::list_roots`'s overlay so a not-yet-probed root is never
/// treated as offline), and for each reachable root re-enqueues its `failed
/// AND failure_kind = 'transient'` thumbnails via the existing
/// `RetryFailedAndStale` bulk enqueue path. Returns the total number of
/// thumbnails accepted for retry across all roots this call.
///
/// This is the synchronous kernel the unit tests below exercise directly
/// against a single in-memory connection. In production it is only reached
/// via `sweep_transient_retries`'s in-memory/test fallback branch — the
/// file-backed path enumerates roots on a read-only connection and dispatches
/// one writer job per root instead (see `sweep_transient_retries` for why).
pub(crate) fn run_transient_retry_sweep(
    database: &Database,
    reachability: &RootReachability,
) -> anyhow::Result<usize> {
    let roots = database.list_roots()?;
    // Snapshot the cache once up front rather than holding the lock across the
    // enqueue loop below (which can run one write transaction per root).
    let reachable_by_root_id: HashMap<i64, bool> = reachability
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    let mut enqueued_count = 0usize;
    for root in roots {
        let is_reachable = *reachable_by_root_id.get(&root.id).unwrap_or(&true);
        if !is_reachable {
            continue;
        }
        enqueued_count += enqueue_transient_retry_for_root(database, root.id)?;
    }
    Ok(enqueued_count)
}

/// Reads `(id, path)` for every enabled root using a dedicated read connection
/// when one is available, so the periodic probe never contends with the single
/// write actor. In-memory/test databases have no separate read connection and
/// fall back to the writer, matching `routes::run_read_database`'s fallback.
async fn enabled_root_paths(state: AppState) -> anyhow::Result<Vec<(i64, String)>> {
    if !state.has_read_database() {
        return state
            .writer
            .write(|database| {
                Ok(database
                    .list_roots()?
                    .into_iter()
                    .map(|root| (root.id, root.path))
                    .collect())
            })
            .await;
    }
    tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<(i64, String)>> {
        let database = state
            .open_read_database()?
            .ok_or_else(|| anyhow::anyhow!("read database unexpectedly unavailable"))?;
        Ok(database
            .list_roots()?
            .into_iter()
            .map(|root| (root.id, root.path))
            .collect())
    })
    .await?
}

fn cors_layer(origin: HeaderValue) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            HeaderName::from_static("x-megle-session"),
        ])
        // The renderer and Core are on different loopback PORTS, so every
        // browser request is cross-origin and only CORS-safelisted response
        // headers are readable from JS. Without this the preview client always
        // saw `x-megle-preview-tier` as null and fell back to "original",
        // silently disabling the background sharpening upgrade — a >4096px
        // image would stay at tier resolution no matter how far you zoomed.
        .expose_headers([HeaderName::from_static("x-megle-preview-tier")])
}

async fn require_session_token(
    axum::extract::State(config): axum::extract::State<ApiConfig>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() == Method::OPTIONS {
        return next.run(request).await;
    }

    if let Some(expected) = config.session_token.as_deref() {
        let actual = request
            .headers()
            .get(SESSION_HEADER)
            .and_then(|value| value.to_str().ok());

        if actual != Some(expected) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }

    next.run(request).await
}

/// Rejects requests with a path under `/api/` so the static SPA fallback
/// never rewrites unmatched API routes to `index.html`. The API router still
/// matches its registered routes first; this filter only fires when the
/// static router would otherwise pick up the request.
async fn reject_api_paths(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if path == "/api" || path.starts_with("/api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(request).await
}

#[derive(Clone)]
struct BasicAuthState {
    expected: Arc<String>,
}

impl BasicAuthState {
    fn new(credentials: BasicAuthCredentials) -> Self {
        Self {
            expected: Arc::new(credentials.expected_header()),
        }
    }
}

async fn require_basic_auth(
    axum::extract::State(state): axum::extract::State<BasicAuthState>,
    request: Request,
    next: Next,
) -> Response {
    // `/api/health` always passes through so Docker/HTTP healthchecks keep
    // working without embedding credentials.
    if request.uri().path() == "/api/health" {
        return next.run(request).await;
    }

    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    if matches!(provided, Some(value) if value == state.expected.as_str()) {
        return next.run(request).await;
    }

    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"Megle\""),
        )
        .body(axum::body::Body::empty())
        .expect("build 401 response")
}

#[cfg(test)]
mod tests {
    use super::run_transient_retry_sweep;
    use crate::db::{Database, FileUpsert, FolderUpsert, NewRoot, ThumbnailStateUpsert};
    use crate::roots::reachability::RootReachability;
    use crate::thumbnails::GRID_320_PROFILE;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn migrated_database() -> Database {
        let database = Database::open_in_memory().expect("open in-memory database");
        database.apply_migrations().expect("apply migrations");
        database
    }

    /// Seeds one root with one folder and one `failed` thumbnail classified
    /// with `failure_kind`, mirroring the real backlog: a thumbnail that
    /// generation already attempted and marked `failed`, then Task 5's
    /// migration/classification stamped `transient` or `permanent`.
    fn seed_failed_thumbnail(
        database: &Database,
        root_id: i64,
        folder_id: i64,
        name: &str,
        file_key: &str,
        failure_kind: &str,
    ) -> i64 {
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: name.to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: Some(file_key.to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get thumbnail source")
            .expect("thumbnail source exists")
            .source_fingerprint(GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: GRID_320_PROFILE.to_string(),
                state: "failed".to_string(),
                cache_key: None,
                width: None,
                height: None,
                byte_size: None,
                error: Some("decode failed".to_string()),
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("insert failed thumbnail state");
        database
            .set_thumbnail_failure_kind_for_test(file_id, GRID_320_PROFILE, failure_kind)
            .expect("classify failure kind");
        file_id
    }

    fn thumbnail_state(database: &Database, file_id: i64) -> String {
        database
            .get_thumbnail(file_id, GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail row exists")
            .state
    }

    #[test]
    fn transient_retry_sweep_retries_reachable_root_and_skips_unreachable_root() {
        let database = migrated_database();

        let reachable_root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures/reachable-root".to_string(),
                display_name: "Reachable Root".to_string(),
            })
            .expect("add reachable root");
        let reachable_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id: reachable_root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "reachable-root-hash".to_string(),
                mtime: Some(1),
            })
            .expect("insert reachable root folder");

        let unreachable_root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures/unreachable-root".to_string(),
                display_name: "Unreachable Root".to_string(),
            })
            .expect("add unreachable root");
        let unreachable_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id: unreachable_root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "unreachable-root-hash".to_string(),
                mtime: Some(1),
            })
            .expect("insert unreachable root folder");

        let reachable_transient_file_id = seed_failed_thumbnail(
            &database,
            reachable_root_id,
            reachable_folder_id,
            "reachable-transient.jpg",
            "sweep-reachable-transient",
            "transient",
        );
        let reachable_permanent_file_id = seed_failed_thumbnail(
            &database,
            reachable_root_id,
            reachable_folder_id,
            "reachable-permanent.jpg",
            "sweep-reachable-permanent",
            "permanent",
        );
        let unreachable_transient_file_id = seed_failed_thumbnail(
            &database,
            unreachable_root_id,
            unreachable_folder_id,
            "unreachable-transient.jpg",
            "sweep-unreachable-transient",
            "transient",
        );
        let unreachable_permanent_file_id = seed_failed_thumbnail(
            &database,
            unreachable_root_id,
            unreachable_folder_id,
            "unreachable-permanent.jpg",
            "sweep-unreachable-permanent",
            "permanent",
        );

        let reachability: RootReachability = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut guard = reachability.lock().expect("lock reachability cache");
            guard.insert(reachable_root_id, true);
            guard.insert(unreachable_root_id, false);
        }

        let enqueued_count =
            run_transient_retry_sweep(&database, &reachability).expect("run transient retry sweep");

        assert_eq!(
            enqueued_count, 1,
            "only the reachable root's transient failure should be enqueued"
        );

        assert_eq!(
            thumbnail_state(&database, reachable_transient_file_id),
            "queued",
            "reachable root's transient failure must be requeued"
        );
        assert_eq!(
            thumbnail_state(&database, reachable_permanent_file_id),
            "failed",
            "reachable root's permanent failure must never be touched"
        );
        assert_eq!(
            thumbnail_state(&database, unreachable_transient_file_id),
            "failed",
            "unreachable root's transient failure must be skipped"
        );
        assert_eq!(
            thumbnail_state(&database, unreachable_permanent_file_id),
            "failed",
            "unreachable root's permanent failure must be skipped"
        );

        // A second sweep on the same DB with no reachability data (fresh cache,
        // matching cold start) still respects the transient-only gate: it must
        // not touch the permanent failures, and re-running it is idempotent for
        // the already-requeued transient one (no longer `failed`, so it is no
        // longer a candidate).
        let empty_reachability: RootReachability = Arc::new(Mutex::new(HashMap::new()));
        let second_pass_enqueued = run_transient_retry_sweep(&database, &empty_reachability)
            .expect("run second transient retry sweep");
        assert_eq!(
            second_pass_enqueued, 1,
            "a cache miss defaults to reachable, so the still-unreachable root's transient \
             failure is picked up once the cache has no entry for it"
        );
    }

    #[test]
    fn transient_retry_sweep_returns_zero_when_no_roots_exist() {
        let database = migrated_database();
        let reachability: RootReachability = Arc::new(Mutex::new(HashMap::new()));
        let enqueued_count = run_transient_retry_sweep(&database, &reachability)
            .expect("run transient retry sweep with no roots");
        assert_eq!(enqueued_count, 0);
    }

    #[test]
    fn transient_retry_sweep_does_not_enqueue_missing_thumbnails() {
        // Regression: the background sweep must retry ONLY transient failures. It
        // reused the `RetryFailedAndStale` mode, whose candidate set also matches
        // `missing` — so on a large library every 60s tick re-enqueued up to 500
        // un-generated (missing) thumbnails per reachable root, bulk-generating
        // the whole library in the background regardless of the user's generation
        // mode (e.g. `cacheOnly`). The sweep must ignore `missing`/`stale` rows
        // and touch only transient-failed ones.
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures/sweep-missing".to_string(),
                display_name: "Sweep Missing".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "sweep-missing-hash".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");

        // A media file with NO thumbnail row is "missing" — never a sweep target.
        let missing_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "missing.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 1,
                ctime: None,
                file_key: Some("sweep-missing-file".to_string()),
            })
            .expect("insert missing file");
        database
            .upsert_media_kind(missing_file_id, "image")
            .expect("insert media kind");

        // A transient-failed thumbnail is the sweep's only legitimate candidate.
        let transient_file_id = seed_failed_thumbnail(
            &database,
            root_id,
            folder_id,
            "transient.jpg",
            "sweep-transient",
            "transient",
        );

        let reachability: RootReachability = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut guard = reachability.lock().expect("lock reachability cache");
            guard.insert(root_id, true);
        }

        let enqueued =
            run_transient_retry_sweep(&database, &reachability).expect("run transient retry sweep");

        assert_eq!(
            enqueued, 1,
            "sweep must enqueue only the transient failure, not the 'missing' thumbnail"
        );
        assert_eq!(
            thumbnail_state(&database, transient_file_id),
            "queued",
            "transient failure must be requeued"
        );
    }
}
