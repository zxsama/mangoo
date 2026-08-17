use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::Request;
use axum::http::{header, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::db::{Database, DbWriter, ThumbnailCacheRefreshMode, ThumbnailCacheScope};
use crate::host_registry::HostRegistry;
use crate::library_manager::{ActiveSnapshot, LibraryManager, LibraryRuntimeResources};
use crate::roots::reachability::{refresh_root_reachability, RootReachability};

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
    // Runtime-owned equivalent: notification_outbox_wake: Arc<tokio::sync::Notify>
    active: Arc<std::sync::RwLock<ActiveSnapshot>>,
    request_gate: Arc<tokio::sync::RwLock<()>>,
    pub library_manager: Option<Arc<LibraryManager>>,
    #[cfg(test)]
    pub(crate) notification_outbox_test_probe: Option<Arc<NotificationOutboxTestProbe>>,
}

impl AppState {
    #[cfg(test)]
    pub(crate) fn new_with_task_queue(
        database: Database,
        task_queue: crate::tasks::TaskSender,
    ) -> Self {
        let runtime = LibraryRuntimeResources::from_database_with_task_queue(database, task_queue);
        Self {
            active: Arc::new(std::sync::RwLock::new(ActiveSnapshot {
                library_uuid: None,
                context_revision: 0,
                runtime_generation: runtime.runtime_generation,
                runtime: Some(runtime),
            })),
            request_gate: Arc::new(tokio::sync::RwLock::new(())),
            library_manager: None,
            notification_outbox_test_probe: None,
        }
    }

    pub fn new(database: Database) -> Self {
        // LibraryRuntimeResources obtains database.notification_outbox_wake()
        let runtime = LibraryRuntimeResources::from_database(None, 1, database, None, None)
            .expect("construct library runtime");
        Self {
            active: Arc::new(std::sync::RwLock::new(ActiveSnapshot {
                library_uuid: None,
                context_revision: 0,
                runtime_generation: runtime.runtime_generation,
                runtime: Some(runtime),
            })),
            request_gate: Arc::new(tokio::sync::RwLock::new(())),
            library_manager: None,
            #[cfg(test)]
            notification_outbox_test_probe: None,
        }
    }

    #[cfg(test)]
    pub fn new_with_worker(database: Database, worker_database: Database) -> Self {
        drop(worker_database);
        Self::new(database)
    }

    pub async fn new_host(registry_path: PathBuf, libraries_dir: PathBuf) -> anyhow::Result<Self> {
        let registry = Arc::new(HostRegistry::open(registry_path)?);
        let active = Arc::new(std::sync::RwLock::new(ActiveSnapshot {
            library_uuid: None,
            context_revision: 0,
            runtime_generation: 0,
            runtime: None,
        }));
        let request_gate = Arc::new(tokio::sync::RwLock::new(()));
        let manager = Arc::new(LibraryManager::new(
            registry,
            libraries_dir,
            Arc::clone(&active),
            Arc::clone(&request_gate),
        ));
        let state = Self {
            active,
            request_gate,
            library_manager: Some(Arc::clone(&manager)),
            #[cfg(test)]
            notification_outbox_test_probe: None,
        };
        manager.restore_active().await?;
        manager.start_sync_coordinator();
        Ok(state)
    }

    pub fn resources(&self) -> anyhow::Result<Arc<LibraryRuntimeResources>> {
        self.active
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .runtime
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no-active-library"))
    }

    pub fn writer(&self) -> anyhow::Result<DbWriter> {
        Ok(self.resources()?.writer.clone())
    }

    pub fn task_queue(&self) -> anyhow::Result<crate::tasks::TaskSender> {
        Ok(self.resources()?.task_queue.clone())
    }

    pub fn notification_outbox_wake(&self) -> anyhow::Result<Arc<tokio::sync::Notify>> {
        Ok(Arc::clone(&self.resources()?.notification_outbox_wake))
    }

    pub fn notification_outbox_read_gate(&self) -> anyhow::Result<Arc<tokio::sync::Semaphore>> {
        Ok(Arc::clone(&self.resources()?.notification_outbox_read_gate))
    }

    pub fn preview_decode_gate(&self) -> anyhow::Result<Arc<tokio::sync::Semaphore>> {
        Ok(Arc::clone(&self.resources()?.preview_decode_gate))
    }

    pub fn fsops_database(&self) -> anyhow::Result<Option<Arc<std::sync::Mutex<Database>>>> {
        Ok(self.resources()?.fsops_database.clone())
    }

    pub fn plugins_dir(&self) -> anyhow::Result<Option<PathBuf>> {
        Ok(self.resources()?.plugins_dir.clone())
    }

    pub fn root_reachability(&self) -> anyhow::Result<RootReachability> {
        Ok(self.resources()?.root_reachability.clone())
    }

    pub fn request_gate(&self) -> Arc<tokio::sync::RwLock<()>> {
        Arc::clone(&self.request_gate)
    }

    pub fn open_read_database(&self) -> anyhow::Result<Option<Database>> {
        let resources = self.resources()?;
        let Some(path) = &resources.read_database_path else {
            return Ok(None);
        };
        Ok(Some(Database::open(path)?))
    }

    /// Whether a dedicated read connection can be opened (the database is
    /// file-backed). In-memory/test databases return `false`, and read
    /// handlers fall back to the shared connection.
    pub fn has_read_database(&self) -> bool {
        self.resources()
            .is_ok_and(|resources| resources.read_database_path.is_some())
    }
}

#[allow(dead_code)]
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
    let state = AppState::new(database);
    router_with_state(state, config)
}

pub fn router_with_state(state: AppState, config: ApiConfig) -> Router {
    // Debug self-test: submit a trivial write through the writer actor right at
    // startup (independent of any client) and log the outcome, so a deadlocked
    // actor is diagnosable without the renderer. Gated by the master debug flag.
    if crate::debug::enabled() {
        if let Ok(writer) = state.writer() {
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
    }
    start_runtime_background_tasks(&state);
    let api_router = routes::router(state.clone())
        .layer(middleware::from_fn_with_state(
            state,
            attach_library_context,
        ))
        .layer(middleware::from_fn_with_state(
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

fn start_runtime_background_tasks(state: &AppState) {
    let Ok(runtime) = state.resources() else {
        return;
    };
    if !runtime.begin_background_ownership() {
        return;
    }
    let token = runtime.cancellation_token();
    let probe_state = state.clone();
    runtime.own_background_task(tokio::spawn(run_root_reachability_probe(
        probe_state,
        token,
    )));
    let token = runtime.cancellation_token();
    let sweep_state = state.clone();
    runtime.own_background_task(tokio::spawn(run_transient_retry_loop(sweep_state, token)));
}

/// Starts the background reachability probe: refreshes `state.root_reachability`
/// immediately (so a cold start has real values as soon as possible), then again
/// every 30s for as long as the process runs. NAS shares flap on transient network
/// blips; without this loop a root probed unreachable once would stay stuck that
/// way until the next explicit action, which is what let ~19k thumbnails go
/// permanently `failed` during a brief outage instead of just skipping a cycle.
async fn run_root_reachability_probe(
    probe_state: AppState,
    cancellation: tokio_util::sync::CancellationToken,
) {
    loop {
        if let Ok(reach) = probe_state.root_reachability() {
            match enabled_root_paths(probe_state.clone()).await {
                Ok(roots) => refresh_root_reachability(&reach, &roots),
                Err(error) => {
                    tracing::warn!(%error, "root reachability probe: failed to list roots");
                }
            }
        }
        tokio::select! {
            _ = cancellation.cancelled() => break,
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
        }
    }
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
async fn run_transient_retry_loop(
    sweep_state: AppState,
    cancellation: tokio_util::sync::CancellationToken,
) {
    loop {
        if cancellation.is_cancelled() {
            break;
        }
        let result = sweep_transient_retries(&sweep_state).await;
        match result {
            Ok(enqueued_count) if enqueued_count > 0 => {
                tracing::info!(
                    enqueued_count,
                    "transient retry sweep re-enqueued failed thumbnails"
                );
            }
            Ok(_) => {}
            Err(error) if !cancellation.is_cancelled() => {
                tracing::warn!(%error, "transient retry sweep failed; will retry next tick");
            }
            Err(_) => break,
        }
        tokio::select! {
            _ = cancellation.cancelled() => break,
            _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}
        }
    }
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
    if state.resources().is_err() {
        return Ok(0);
    }
    if !state.has_read_database() {
        let reach = state.root_reachability()?;
        return state
            .writer()?
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
        .root_reachability()?
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
            .writer()?
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
            .writer()
            .expect("active library writer")
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
            HeaderName::from_static("x-megle-context-revision"),
        ])
        // The renderer and Core are on different loopback PORTS, so every
        // browser request is cross-origin and only CORS-safelisted response
        // headers are readable from JS. Without this the preview client always
        // saw `x-megle-preview-tier` as null and fell back to "original",
        // silently disabling the background sharpening upgrade — a >4096px
        // image would stay at tier resolution no matter how far you zoomed.
        .expose_headers([
            HeaderName::from_static("x-megle-preview-tier"),
            HeaderName::from_static("x-megle-library-id"),
            HeaderName::from_static("x-megle-context-revision"),
        ])
}

async fn attach_library_context(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    start_runtime_background_tasks(&state);
    let is_library_control = request.uri().path().starts_with("/api/libraries");
    let is_mutation = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    // Hold the read side of the global request gate before validating the
    // revision. Otherwise a request can validate A, wait behind an A -> B
    // activation, and then mutate B with A's revision.
    let gate = state.request_gate();
    let request_guard = if is_library_control {
        None
    } else {
        Some(gate.read_owned().await)
    };
    if request_guard.is_some() && is_mutation {
        if let Some(manager) = state.library_manager.as_ref() {
            let snapshot = manager.active_snapshot();
            let expected = request
                .headers()
                .get("x-megle-context-revision")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            if expected != Some(snapshot.context_revision) {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "Active Library context changed; refresh and retry",
                        "code": "stale_library_context"
                    })),
                )
                    .into_response();
            }
        }
    }
    if !is_library_control && request.uri().path() != "/api/health" && state.resources().is_err() {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "No Library is active on this device",
                "code": "no_active_library"
            })),
        )
            .into_response();
    }
    let _request_guard = request_guard;
    let mut response = next.run(request).await;
    if let Some(manager) = state.library_manager.as_ref() {
        let snapshot = manager.active_snapshot();
        if let Some(library_uuid) = snapshot.library_uuid {
            if let Ok(value) = HeaderValue::from_str(&library_uuid) {
                response.headers_mut().insert("x-megle-library-id", value);
            }
        }
        if let Ok(value) = HeaderValue::from_str(&snapshot.context_revision.to_string()) {
            response
                .headers_mut()
                .insert("x-megle-context-revision", value);
        }
    }
    response
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
