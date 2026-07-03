use std::path::{Path, PathBuf};
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

use crate::db::Database;
use crate::tasks::{start_worker, TaskSender};
use crate::watch::{start_watcher, WatcherHandle};

pub mod routes;

#[allow(dead_code)]
pub const API_PREFIX: &str = "/api";
pub const SESSION_HEADER: &str = "X-Megle-Session";

#[derive(Clone)]
pub struct AppState {
    pub database: Arc<Mutex<Database>>,
    pub task_queue: TaskSender,
    read_database_path: Option<PathBuf>,
    /// Resolved at startup so route handlers (notably
    /// `POST /api/plugins/discover`) agree with `main.rs` on where to look
    /// for plugin manifests. `None` when the API is built without a
    /// configured directory; the handler falls back to the same default
    /// rules as startup.
    pub plugins_dir: Option<PathBuf>,
    _watcher: Option<Arc<WatcherHandle>>,
}

impl AppState {
    pub fn new(database: Database) -> Self {
        if let Some(worker_database) = database.reopen().expect("reopen worker database") {
            return Self::new_with_worker(database, worker_database);
        }

        let (task_queue, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        Self {
            database: Arc::new(Mutex::new(database)),
            task_queue,
            read_database_path: None,
            plugins_dir: None,
            _watcher: None,
        }
    }

    pub fn new_with_worker(database: Database, worker_database: Database) -> Self {
        let read_database_path = database.path().map(Path::to_path_buf);
        let watcher_database = database.reopen().expect("reopen watcher database");
        let database = Arc::new(Mutex::new(database));
        let task_queue = start_worker(worker_database);
        let watcher = watcher_database
            .map(|watcher_database| Arc::new(start_watcher(watcher_database, task_queue.clone())));
        Self {
            database,
            task_queue,
            read_database_path,
            plugins_dir: None,
            _watcher: watcher,
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
