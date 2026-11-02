//! Short-lived, single-source native video reads. No ambient API authority is
//! accepted by the media route and no source bytes are cached in the renderer.
use super::routes::{preview_content_type, run_read_database, CoreError};
use super::AppState;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine;
use rand::RngCore;
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, ReadBuf};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::io::ReaderStream;
use tokio_util::sync::CancellationToken;

const MAX_GRANTS: usize = 8;
const STREAMS_PER_GRANT: usize = 2;
const CHUNK_BYTES: usize = 64 * 1024;
const RENEW_TTL: Duration = Duration::from_secs(60);
const ABSOLUTE_TTL: Duration = Duration::from_secs(600);

#[derive(Default)]
pub(crate) struct PlaybackSessions {
    grants: Mutex<HashMap<String, Arc<Grant>>>,
}

struct Grant {
    id: String,
    capability: String,
    source: Source,
    expires: Mutex<Instant>,
    absolute_expiry: Instant,
    cancel: CancellationToken,
    runtime_cancel: CancellationToken,
    streams: Arc<Semaphore>,
}

#[derive(Clone, PartialEq)]
struct CatalogSource {
    path: PathBuf,
    root: Option<crate::db::SyncRootBindingToken>,
    size: i64,
    mtime: i64,
}

struct Source {
    file_id: i64,
    library_uuid: Option<String>,
    context_revision: u64,
    runtime_generation: u64,
    catalog: CatalogSource,
    identity: String,
    stamp: Vec<u8>,
    length: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlaybackSession {
    id: String,
    src: String,
    expires_at: u64,
    renew_after_ms: u64,
}

impl Grant {
    fn live(&self) -> bool {
        !self.cancel.is_cancelled()
            && !self.runtime_cancel.is_cancelled()
            && Instant::now() < *self.expires.lock().unwrap_or_else(|p| p.into_inner())
            && Instant::now() < self.absolute_expiry
    }

    fn response(&self) -> PlaybackSession {
        let remaining = self
            .expires
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .saturating_duration_since(Instant::now());
        PlaybackSession {
            id: self.id.clone(),
            src: format!("/playback/{}", self.capability),
            expires_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
                + remaining.as_millis() as u64,
            renew_after_ms: (remaining.as_millis() as u64 / 2).clamp(1, 30_000),
        }
    }

    async fn ended(&self) {
        loop {
            let expiry = *self.expires.lock().unwrap_or_else(|p| p.into_inner());
            tokio::select! {
                _ = self.cancel.cancelled() => return,
                _ = self.runtime_cancel.cancelled() => return,
                _ = tokio::time::sleep_until(expiry.into()) => {}
            }
            if !self.live() {
                return;
            }
        }
    }
}

impl PlaybackSessions {
    fn find(&self, key: &str, capability: bool) -> Result<Arc<Grant>, CoreError> {
        let mut grants = self.grants.lock().unwrap_or_else(|p| p.into_inner());
        grants.retain(|_, grant| {
            if grant.live() {
                true
            } else {
                grant.cancel.cancel();
                false
            }
        });
        let grant = if capability {
            grants
                .values()
                .find(|grant| grant.capability == key)
                .cloned()
        } else {
            grants.get(key).cloned()
        };
        grant.ok_or_else(|| CoreError::not_found("Playback session unavailable".into()))
    }
}

fn conflict() -> CoreError {
    CoreError::coded(
        StatusCode::CONFLICT,
        "playback_source_changed",
        "Playback source changed".into(),
    )
}

pub(super) fn authenticated_router(state: AppState) -> Router {
    Router::new()
        .route("/api/media/:file_id/playback-sessions", post(create))
        .route("/api/playback-sessions/:session_id/renew", post(renew))
        .route("/api/playback-sessions/:session_id", delete(revoke))
        .with_state(state)
}

pub(super) fn stream_router(state: AppState) -> Router {
    Router::new()
        .route("/api/playback/:capability", get(stream).head(stream))
        .with_state(state)
}

async fn catalog_source(state: AppState, file_id: i64) -> Result<CatalogSource, CoreError> {
    run_read_database(state, move |db| {
        let media = db
            .get_media(file_id)?
            .ok_or_else(|| CoreError::not_found("Video unavailable".into()))?;
        if media.kind.as_deref() != Some("video")
            || media.presentation.preview_kind != "original-video"
        {
            return Err(conflict());
        }
        let (path, root) = db
            .resolve_file_source_path_with_root_token(file_id)?
            .ok_or_else(|| CoreError::not_found("Video source unavailable".into()))?;
        if let Some(token) = &root {
            db.require_sync_root_binding_token(token)?;
        }
        Ok(CatalogSource {
            path,
            root,
            size: media.size,
            mtime: media.mtime,
        })
    })
    .await
}

// Existing no-follow source primitives protect parent traversal and include
// change time in the stamp (same-size writes with restored mtime are stale).
async fn disk_identity(path: PathBuf) -> io::Result<(String, Vec<u8>, u64)> {
    tokio::task::spawn_blocking(move || {
        let (file, _guard) =
            crate::sync::file_policy::open_source(&path).map_err(io::Error::other)?;
        let length = file.metadata()?.len();
        let identity = crate::library_path::local_file_identity(&path)?;
        let stamp = crate::sync::identity::source_stamp(&path)?;
        Ok((identity, stamp, length))
    })
    .await
    .map_err(io::Error::other)?
}

async fn create(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
) -> Result<impl IntoResponse, CoreError> {
    let snapshot = state
        .active
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let catalog = catalog_source(state.clone(), file_id).await?;
    if let Some(token) = &catalog.root {
        state.require_current_root_token(token)?;
    }
    let (identity, stamp, length) = disk_identity(catalog.path.clone())
        .await
        .map_err(|_| CoreError::not_found("Video source unavailable".into()))?;
    let runtime = state.resources()?;
    let now = Instant::now();
    let mut capability = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut capability);
    let grant = Arc::new(Grant {
        id: uuid::Uuid::new_v4().to_string(),
        capability: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(capability),
        source: Source {
            file_id,
            library_uuid: snapshot.library_uuid,
            context_revision: snapshot.context_revision,
            runtime_generation: snapshot.runtime_generation,
            catalog,
            identity,
            stamp,
            length,
        },
        expires: Mutex::new(now + RENEW_TTL),
        absolute_expiry: now + ABSOLUTE_TTL,
        cancel: CancellationToken::new(),
        runtime_cancel: runtime.cancellation_token(),
        streams: Arc::new(Semaphore::new(STREAMS_PER_GRANT)),
    });
    validate(&state, &grant).await?;
    let mut grants = state
        .playback_sessions
        .grants
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    grants.retain(|_, entry| {
        if entry.live() {
            true
        } else {
            entry.cancel.cancel();
            false
        }
    });
    if grants.len() >= MAX_GRANTS {
        return Err(CoreError::coded(
            StatusCode::TOO_MANY_REQUESTS,
            "playback_limit",
            "Playback session limit reached".into(),
        ));
    }
    grants.insert(grant.id.clone(), grant.clone());
    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(grant.response()),
    ))
}

async fn validate(state: &AppState, grant: &Grant) -> Result<(), CoreError> {
    if !grant.live() {
        return Err(CoreError::not_found("Playback session unavailable".into()));
    }
    {
        let snapshot = state.active.read().unwrap_or_else(|p| p.into_inner());
        if snapshot.library_uuid != grant.source.library_uuid
            || snapshot.context_revision != grant.source.context_revision
            || snapshot.runtime_generation != grant.source.runtime_generation
        {
            return Err(conflict());
        }
    }
    if let Some(root) = &grant.source.catalog.root {
        state.require_current_root_token(root)?;
    }
    if catalog_source(state.clone(), grant.source.file_id).await? != grant.source.catalog {
        return Err(conflict());
    }
    let (identity, stamp, length) = disk_identity(grant.source.catalog.path.clone())
        .await
        .map_err(|_| conflict())?;
    if identity != grant.source.identity
        || stamp != grant.source.stamp
        || length != grant.source.length
    {
        return Err(conflict());
    }
    let snapshot = state.active.read().unwrap_or_else(|p| p.into_inner());
    if !grant.live()
        || snapshot.library_uuid != grant.source.library_uuid
        || snapshot.context_revision != grant.source.context_revision
        || snapshot.runtime_generation != grant.source.runtime_generation
    {
        return Err(conflict());
    }
    Ok(())
}

async fn renew(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, CoreError> {
    let grant = state.playback_sessions.find(&id, false)?;
    if let Err(error) = validate(&state, &grant).await {
        grant.cancel.cancel();
        return Err(error);
    }
    *grant.expires.lock().unwrap_or_else(|p| p.into_inner()) =
        (Instant::now() + RENEW_TTL).min(grant.absolute_expiry);
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(grant.response()),
    ))
}

async fn revoke(State(state): State<AppState>, Path(id): Path<String>) -> StatusCode {
    if let Some(grant) = state
        .playback_sessions
        .grants
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&id)
    {
        grant.cancel.cancel();
    }
    StatusCode::NO_CONTENT
}

#[derive(Debug, PartialEq)]
struct ByteRange {
    start: u64,
    length: u64,
    partial: bool,
}

fn parse_range(value: Option<&str>, length: u64) -> Result<ByteRange, ()> {
    let Some(value) = value else {
        return Ok(ByteRange {
            start: 0,
            length,
            partial: false,
        });
    };
    let value = value.strip_prefix("bytes=").ok_or(())?;
    let (start, end) = value.split_once('-').ok_or(())?;
    let number = |text: &str| -> Result<u64, ()> {
        if text.is_empty() || !text.bytes().all(|c| c.is_ascii_digit()) {
            return Err(());
        }
        text.parse().map_err(|_| ())
    };
    if length == 0 {
        return Err(());
    }
    if start.is_empty() {
        let suffix = number(end)?;
        if suffix == 0 {
            return Err(());
        }
        let amount = suffix.min(length);
        return Ok(ByteRange {
            start: length - amount,
            length: amount,
            partial: true,
        });
    }
    let start = number(start)?;
    if start >= length {
        return Err(());
    }
    let end = if end.is_empty() {
        length - 1
    } else {
        number(end)?.min(length - 1)
    };
    if start > end {
        return Err(());
    }
    Ok(ByteRange {
        start,
        length: end - start + 1,
        partial: true,
    })
}

async fn stream(
    State(state): State<AppState>,
    Path(capability): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, CoreError> {
    let grant = state.playback_sessions.find(&capability, true)?;
    let gate = state.request_gate();
    let _guard = gate.read_owned().await;
    if let Err(error) = validate(&state, &grant).await {
        grant.cancel.cancel();
        return Err(error);
    }
    let ranges: Vec<_> = headers.get_all(header::RANGE).iter().collect();
    let range = if ranges.len() > 1 {
        Err(())
    } else {
        match ranges.first() {
            Some(value) => value
                .to_str()
                .map_err(|_| ())
                .and_then(|value| parse_range(Some(value), grant.source.length)),
            None => parse_range(None, grant.source.length),
        }
    };
    let range = match range {
        Ok(range) => range,
        Err(()) => {
            return Ok(Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes */{}", grant.source.length),
                )
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CACHE_CONTROL, "no-store")
                .header(header::CONTENT_LENGTH, "0")
                .body(Body::empty())
                .map_err(|_| conflict())?)
        }
    };
    let mut response = Response::builder()
        .status(if range.partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(
            header::CONTENT_TYPE,
            preview_content_type(&grant.source.catalog.path, Some("video")),
        )
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store")
        .header("referrer-policy", "no-referrer")
        .header("x-content-type-options", "nosniff")
        .header(header::CONTENT_LENGTH, range.length);
    if range.partial {
        response = response.header(
            header::CONTENT_RANGE,
            format!(
                "bytes {}-{}/{}",
                range.start,
                range.start + range.length - 1,
                grant.source.length
            ),
        );
    }
    let body = if method == Method::HEAD || range.length == 0 {
        Body::empty()
    } else {
        let permit = grant.streams.clone().try_acquire_owned().map_err(|_| {
            CoreError::coded(
                StatusCode::TOO_MANY_REQUESTS,
                "playback_stream_limit",
                "Playback stream limit reached".into(),
            )
        })?;
        let path = grant.source.catalog.path.clone();
        let (file, directory) =
            tokio::task::spawn_blocking(move || crate::sync::file_policy::open_source(&path))
                .await
                .map_err(|_| conflict())?
                .map_err(|_| conflict())?;
        let mut file = tokio::fs::File::from_std(file);
        file.seek(io::SeekFrom::Start(range.start))
            .await
            .map_err(|_| conflict())?;
        Body::from_stream(ReaderStream::with_capacity(
            PlaybackReader {
                state,
                grant,
                file: Arc::new(tokio::sync::Mutex::new(file)),
                _directory: directory,
                _permit: permit,
                remaining: range.length,
                pending: None,
            },
            CHUNK_BYTES,
        ))
    };
    response.body(body).map_err(|_| conflict())
}

type PendingRead = Pin<Box<dyn Future<Output = io::Result<Vec<u8>>> + Send>>;
struct PlaybackReader {
    state: AppState,
    grant: Arc<Grant>,
    file: Arc<tokio::sync::Mutex<tokio::fs::File>>,
    _directory: crate::library_path::MarkerDirectoryGuard,
    _permit: OwnedSemaphorePermit,
    remaining: u64,
    pending: Option<PendingRead>,
}

impl AsyncRead for PlaybackReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.remaining == 0 || output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if !this.grant.live() {
            return Poll::Ready(Err(io::Error::other("Playback session ended")));
        }
        if this.pending.is_none() {
            let state = this.state.clone();
            let grant = this.grant.clone();
            let file = this.file.clone();
            let length = this
                .remaining
                .min(CHUNK_BYTES as u64)
                .min(output.remaining() as u64) as usize;
            this.pending = Some(Box::pin(async move {
                let read = async {
                    // A Library switch may run between body chunks; never hold
                    // the request gate (or a writer lock) across source I/O.
                    validate(&state, &grant)
                        .await
                        .map_err(|_| io::Error::other("Playback source changed"))?;
                    let mut bytes = vec![0; length];
                    let count = file.lock().await.read(&mut bytes).await?;
                    if count == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "Video source shortened",
                        ));
                    }
                    validate(&state, &grant)
                        .await
                        .map_err(|_| io::Error::other("Playback source changed"))?;
                    bytes.truncate(count);
                    Ok(bytes)
                };
                tokio::select! {
                    biased;
                    _ = grant.ended() => Err(io::Error::other("Playback ended")),
                    value = read => value,
                }
            }));
        }
        match this.pending.as_mut().unwrap().as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                this.pending = None;
                Poll::Ready(result.map(|bytes| {
                    this.remaining -= bytes.len() as u64;
                    output.put_slice(&bytes);
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, FileUpsert, FolderUpsert, NewRoot};
    use axum::body::HttpBody;
    use axum::http::Request;
    use std::io::{Seek, Write};
    use tower::ServiceExt;

    struct Fixture {
        state: AppState,
        app: Router,
        file_id: i64,
        path: PathBuf,
        _root: tempfile::TempDir,
    }

    impl Fixture {
        fn new(length: u64) -> Self {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("video.mp4");
            let mut file = std::fs::File::create(&path).unwrap();
            file.set_len(length).unwrap();
            file.write_all(b"start").unwrap();
            file.seek(io::SeekFrom::Start(length - 3)).unwrap();
            file.write_all(b"end").unwrap();
            drop(file);
            let db = Database::open_in_memory().unwrap();
            db.apply_migrations().unwrap();
            let root_id = db
                .add_root(NewRoot {
                    path: root.path().to_string_lossy().into_owned(),
                    display_name: "Playback".into(),
                })
                .unwrap();
            let folder_id = db
                .upsert_folder(FolderUpsert {
                    root_id,
                    parent_id: None,
                    name: String::new(),
                    path_hash: "playback".into(),
                    mtime: Some(1),
                })
                .unwrap();
            let file_id = db
                .upsert_file(FileUpsert {
                    root_id,
                    folder_id,
                    name: "video.mp4".into(),
                    ext: "mp4".into(),
                    size: length as i64,
                    mtime: 1,
                    ctime: None,
                    file_key: None,
                })
                .unwrap();
            db.upsert_media_kind(file_id, "video").unwrap();
            let state = AppState::new(db);
            let app = crate::api::router_with_state(
                state.clone(),
                crate::api::ApiConfig {
                    session_token: Some("test-session".into()),
                    ..Default::default()
                },
            );
            Self {
                state,
                app,
                file_id,
                path,
                _root: root,
            }
        }
        async fn request(
            &self,
            path: &str,
            method: &str,
            range: Option<&str>,
            auth: bool,
        ) -> Response {
            let mut request = Request::builder().uri(path).method(method);
            if let Some(range) = range {
                request = request.header(header::RANGE, range);
            }
            if auth {
                request = request.header("x-mangoo-session", "test-session");
            }
            self.app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap()
        }
        async fn grant(&self) -> serde_json::Value {
            let response = self
                .request(
                    &format!("/api/media/{}/playback-sessions", self.file_id),
                    "POST",
                    None,
                    true,
                )
                .await;
            assert_eq!(response.status(), StatusCode::CREATED);
            serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap(),
            )
            .unwrap()
        }
    }

    #[test]
    fn playback_ranges_cover_suffix_open_ended_and_invalid() {
        assert_eq!(
            parse_range(None, 0),
            Ok(ByteRange {
                start: 0,
                length: 0,
                partial: false
            })
        );
        assert_eq!(
            parse_range(Some("bytes=2-5"), 10),
            Ok(ByteRange {
                start: 2,
                length: 4,
                partial: true
            })
        );
        assert_eq!(
            parse_range(Some("bytes=2-"), 10),
            Ok(ByteRange {
                start: 2,
                length: 8,
                partial: true
            })
        );
        assert_eq!(
            parse_range(Some("bytes=-3"), 10),
            Ok(ByteRange {
                start: 7,
                length: 3,
                partial: true
            })
        );
        assert_eq!(
            parse_range(Some("bytes=-30"), 10),
            Ok(ByteRange {
                start: 0,
                length: 10,
                partial: true
            })
        );
        for bad in [
            "",
            "items=0-1",
            "bytes=",
            "bytes=-0",
            "bytes=10-",
            "bytes=7-2",
            "bytes=0-1,3-4",
            "bytes=+1-3",
            "bytes= 1-3",
            "bytes=18446744073709551616-",
        ] {
            assert_eq!(parse_range(Some(bad), 10), Err(()), "{bad}");
        }
        assert_eq!(parse_range(Some("bytes=0-"), 0), Err(()));
    }

    #[tokio::test]
    async fn playback_http_large_file_reads_only_requested_ranges_and_head() {
        let fixture = Fixture::new(1024 * 1024 * 1024);
        let response = fixture
            .request(
                &format!("/api/media/{}/playback-sessions", fixture.file_id),
                "POST",
                None,
                false,
            )
            .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let session = fixture.grant().await;
        let url = format!("/api{}", session["src"].as_str().unwrap());
        assert!(!url.contains("test-session"));
        for (range, expected) in [
            ("bytes=0-4", b"start".as_slice()),
            ("bytes=-3", b"end".as_slice()),
            ("bytes=1073741821-", b"end".as_slice()),
        ] {
            let response = fixture.request(&url, "GET", Some(range), false).await;
            assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
            assert_eq!(
                response.headers()[header::CONTENT_LENGTH],
                expected.len().to_string()
            );
            assert_eq!(response.headers()[header::ACCEPT_RANGES], "bytes");
            assert_eq!(
                axum::body::to_bytes(response.into_body(), 8)
                    .await
                    .unwrap()
                    .as_ref(),
                expected
            );
        }
        let response = fixture.request(&url, "HEAD", None, false).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "1073741824");
        assert!(axum::body::to_bytes(response.into_body(), 0)
            .await
            .unwrap()
            .is_empty());
        let response = fixture.request(&url, "HEAD", Some("bytes=-3"), false).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers()[header::CONTENT_RANGE],
            "bytes 1073741821-1073741823/1073741824"
        );
        for range in ["bytes=0-1,3-4", "bytes=1073741824-", "bytes=-0"] {
            let response = fixture.request(&url, "GET", Some(range), false).await;
            assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
            assert_eq!(
                response.headers()[header::CONTENT_RANGE],
                "bytes */1073741824"
            );
        }
        assert_eq!(
            fixture.request(&url, "POST", None, false).await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            fixture
                .request("/api/media", "GET", None, false)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn playback_body_drop_releases_slots_and_revoke_cancels_existing_body() {
        let fixture = Fixture::new(1024 * 1024);
        let session = fixture.grant().await;
        let url = format!("/api{}", session["src"].as_str().unwrap());
        let id = session["id"].as_str().unwrap();
        let first = fixture.request(&url, "GET", None, false).await;
        let second = fixture.request(&url, "GET", None, false).await;
        assert_eq!(
            fixture.request(&url, "GET", None, false).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        drop(first);
        let third = fixture.request(&url, "GET", Some("bytes=0-4"), false).await;
        assert_eq!(third.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            fixture
                .request(
                    &format!("/api/playback-sessions/{id}"),
                    "DELETE",
                    None,
                    true
                )
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        assert!(axum::body::to_bytes(second.into_body(), 1024 * 1024)
            .await
            .is_err());
        assert!(axum::body::to_bytes(third.into_body(), 10).await.is_err());
        assert_eq!(
            fixture.request(&url, "GET", None, false).await.status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn playback_revocation_after_first_chunk_stops_active_body() {
        let fixture = Fixture::new(1024 * 1024);
        let session = fixture.grant().await;
        let id = session["id"].as_str().unwrap();
        let url = format!("/api{}", session["src"].as_str().unwrap());
        let grant = fixture
            .state
            .playback_sessions
            .find(id, false)
            .unwrap_or_else(|_| panic!("new playback grant must exist"));
        let mut body = fixture.request(&url, "GET", None, false).await.into_body();
        let first = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
            .await
            .unwrap()
            .unwrap();
        let bytes = first.data_ref().unwrap();
        assert!(bytes.len() <= CHUNK_BYTES);
        assert_eq!(&bytes[..5], b"start");
        assert_eq!(grant.streams.available_permits(), STREAMS_PER_GRANT - 1);
        fixture
            .request(
                &format!("/api/playback-sessions/{id}"),
                "DELETE",
                None,
                true,
            )
            .await;
        let next = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
        assert!(next.unwrap().is_err());
        drop(body);
        assert_eq!(grant.streams.available_permits(), STREAMS_PER_GRANT);
    }

    #[tokio::test]
    async fn playback_rejects_source_replacement_context_changes_and_expiry() {
        let fixture = Fixture::new(100);
        let session = fixture.grant().await;
        let url = format!("/api{}", session["src"].as_str().unwrap());
        let old_mtime = filetime::FileTime::from_last_modification_time(
            &std::fs::metadata(&fixture.path).unwrap(),
        );
        std::fs::write(&fixture.path, [3u8; 100]).unwrap();
        filetime::set_file_mtime(&fixture.path, old_mtime).unwrap();
        assert_eq!(
            fixture.request(&url, "GET", None, false).await.status(),
            StatusCode::CONFLICT
        );
        let session = fixture.grant().await;
        let url = format!("/api{}", session["src"].as_str().unwrap());
        let body = fixture.request(&url, "GET", None, false).await.into_body();
        fixture.state.active.write().unwrap().context_revision += 1;
        assert!(axum::body::to_bytes(body, 100).await.is_err());
        assert_eq!(
            fixture.request(&url, "GET", None, false).await.status(),
            StatusCode::CONFLICT
        );
        let session = fixture.grant().await;
        let id = session["id"].as_str().unwrap();
        let grant = fixture
            .state
            .playback_sessions
            .find(id, false)
            .ok()
            .unwrap();
        *grant.expires.lock().unwrap() = Instant::now() - Duration::from_secs(1);
        assert_eq!(
            fixture
                .request(
                    &format!("/api{}", session["src"].as_str().unwrap()),
                    "GET",
                    None,
                    false
                )
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn playback_renew_and_grant_limit() {
        let fixture = Fixture::new(100);
        let first = fixture.grant().await;
        for _ in 1..MAX_GRANTS {
            fixture.grant().await;
        }
        let response = fixture
            .request(
                &format!("/api/media/{}/playback-sessions", fixture.file_id),
                "POST",
                None,
                true,
            )
            .await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let id = first["id"].as_str().unwrap();
        assert_eq!(
            fixture
                .request(
                    &format!("/api/playback-sessions/{id}/renew"),
                    "POST",
                    None,
                    true
                )
                .await
                .status(),
            StatusCode::OK
        );
        fixture
            .request(
                &format!("/api/playback-sessions/{id}"),
                "DELETE",
                None,
                true,
            )
            .await;
        fixture.grant().await;
    }

    #[tokio::test]
    async fn playback_root_removal_and_runtime_stop_invalidate_body_ownership() {
        let fixture = Fixture::new(100);
        let session = fixture.grant().await;
        let url = format!("/api{}", session["src"].as_str().unwrap());
        let response = fixture.request(&url, "GET", None, false).await;
        fixture
            .state
            .resources()
            .unwrap()
            .cancellation_token()
            .cancel();
        assert!(axum::body::to_bytes(response.into_body(), 100)
            .await
            .is_err());
        assert_eq!(
            fixture.request(&url, "GET", None, false).await.status(),
            StatusCode::NOT_FOUND
        );

        let fixture = Fixture::new(100);
        let session = fixture.grant().await;
        let url = format!("/api{}", session["src"].as_str().unwrap());
        let response = fixture.request(&url, "GET", None, false).await;
        let file_id = fixture.file_id;
        fixture
            .state
            .writer()
            .unwrap()
            .write(move |db| {
                let root_id = db.get_media(file_id)?.unwrap().root_id;
                db.mark_root_for_purge(root_id)?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(axum::body::to_bytes(response.into_body(), 100)
            .await
            .is_err());
        assert_ne!(
            fixture.request(&url, "GET", None, false).await.status(),
            StatusCode::OK
        );
    }
}
