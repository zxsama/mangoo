use crate::db::notebook::tree::{FolderInput, NotebookFolder, NotebookTree, DUPLICATE_FOLDER_NAME};
use axum::{
    body::Body,
    extract::{rejection::JsonRejection, Path, Query, Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use http_body_util::BodyExt;
use serde::{de::DeserializeOwned, Deserialize};
use std::io::{BufReader, Seek};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use super::routes::{
    run_authoritative_database, run_read_database, run_shared_database, CoreError,
};
use super::AppState;
use crate::db::{
    notebook::{NoteAttachment, NoteInput, NoteSummary, NotebookNote, PLUGIN},
    plugin_records::PluginRecordError,
};
use crate::host_registry::LibraryKind;

pub(super) fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/notebooks/:library_id/tree", get(get_tree))
        .route("/api/notebooks/:library_id/folders", post(create_folder))
        .route(
            "/api/notebooks/:library_id/folders/:notebook_folder_id",
            axum::routing::put(update_folder).delete(delete_folder),
        )
        .route(
            "/api/notebooks/:library_id/notes/:note_id/folder",
            axum::routing::put(move_note),
        )
        .route(
            "/api/notebooks/:library_id/notes",
            get(list_notes).post(create_note),
        )
        .route(
            "/api/notebooks/:library_id/notes/:note_id",
            get(get_note).put(update_note).delete(delete_note),
        )
        .route(
            "/api/notebooks/:library_id/notes/:note_id/attachments",
            post(upload_attachment),
        )
        .route(
            "/api/notebooks/:library_id/notes/:note_id/attachments/:attachment_id",
            get(get_attachment),
        )
        .with_state(state)
}

/// The global Library read lease spans every notebook request. The UUID in
/// the URL also fences loads and writes already queued in an old editor.
fn require_notebook(state: &AppState, id: &str) -> Result<(), CoreError> {
    super::plugin_data::require_plugin(state, PLUGIN)?;
    if let Some(manager) = &state.library_manager {
        if manager.active_snapshot().library_uuid.as_deref() == Some(id)
            && manager
                .host_registry()
                .library(id)?
                .is_some_and(|lib| lib.kind == LibraryKind::Notes)
        {
            return Ok(());
        }
    }
    Err(CoreError::coded(
        StatusCode::CONFLICT,
        "stale_library_context",
        "This notebook is not active".into(),
    ))
}

fn error(err: PluginRecordError) -> CoreError {
    match err {
        PluginRecordError::Invalid(message) => {
            CoreError::coded(StatusCode::BAD_REQUEST, "invalid_note", message)
        }
        PluginRecordError::NotFound(message) => {
            CoreError::coded(StatusCode::NOT_FOUND, "note_not_found", message)
        }
        PluginRecordError::Conflict(message) if message == "note_content_pending" => {
            CoreError::coded(
                StatusCode::CONFLICT,
                "note_content_pending",
                "Content is still synchronizing".into(),
            )
        }
        PluginRecordError::Conflict(message) if message == DUPLICATE_FOLDER_NAME => {
            CoreError::coded(
                StatusCode::CONFLICT,
                DUPLICATE_FOLDER_NAME,
                "此目录中已有同名文件夹，请使用其他名称或选择其他位置。".into(),
            )
        }
        PluginRecordError::Conflict(message) => {
            CoreError::coded(StatusCode::CONFLICT, "note_revision_conflict", message)
        }
        PluginRecordError::Other(error) => CoreError::from(error),
    }
}

#[derive(Deserialize)]
struct SearchParams {
    query: Option<String>,
}

async fn list_notes(
    State(state): State<AppState>,
    Path(library): Path<String>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<NoteSummary>>, CoreError> {
    require_notebook(&state, &library)?;
    if let Some(query) = params.query.filter(|value| !value.trim().is_empty()) {
        return run_shared_database(state, move |db| {
            db.search_notes(&query).map(Json).map_err(error)
        })
        .await;
    }
    run_read_database(state, |db| db.list_notes().map(Json).map_err(error)).await
}

// An anonymous temporary file is removed even if the upload disconnects.
// No SQLite transaction or writer lease is held while the sender is streaming.
async fn spool_body(mut body: Body) -> Result<std::fs::File, CoreError> {
    let file = tempfile::tempfile().map_err(anyhow::Error::from)?;
    let mut file = tokio::io::BufWriter::new(tokio::fs::File::from_std(file));
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|err| {
            CoreError::coded(
                StatusCode::BAD_REQUEST,
                "incomplete_upload",
                err.to_string(),
            )
        })?;
        if let Ok(bytes) = frame.into_data() {
            file.write_all(&bytes).await.map_err(anyhow::Error::from)?;
        }
    }
    file.flush().await.map_err(anyhow::Error::from)?;
    let mut file = file.into_inner();
    file.rewind().await.map_err(anyhow::Error::from)?;
    Ok(file.into_std().await)
}

async fn note_json<T: DeserializeOwned + Send + 'static>(request: Request) -> Result<T, CoreError> {
    use axum::extract::FromRequest;
    // Small requests use Axum's normal fast path. Larger and unknown-length
    // bodies are spooled and decoded on a blocking thread, without buffering
    // both the encoded request and its decoded source in the Core heap.
    use axum::body::HttpBody;
    if request
        .body()
        .size_hint()
        .upper()
        .is_some_and(|n| n <= 256 * 1024)
    {
        return Json::<T>::from_request(request, &())
            .await
            .map(|Json(value)| value)
            .map_err(invalid_json);
    }
    let json = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|mime| {
            mime.trim().eq_ignore_ascii_case("application/json") || mime.trim().ends_with("+json")
        });
    if !json {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_note",
            "Expected JSON request".into(),
        ));
    }
    let file = spool_body(request.into_body()).await?;
    tokio::task::spawn_blocking(move || {
        serde_json::from_reader(BufReader::new(file))
            .map_err(|e| CoreError::coded(StatusCode::BAD_REQUEST, "invalid_note", e.to_string()))
    })
    .await
    .map_err(anyhow::Error::from)?
}

fn note_response(mut note: NotebookNote) -> Result<Response, CoreError> {
    use axum::body::Bytes;
    use futures_util::{stream, StreamExt};
    if note.markdown.len() <= 256 * 1024 {
        return Ok(Json(note).into_response());
    }
    // Stream JSON escapes in UTF-8-aligned blocks after the transaction. A
    // committed save never needs additional disk space to deliver its reply.
    let source = std::mem::take(&mut note.markdown);
    let mut metadata = serde_json::to_value(note).map_err(anyhow::Error::from)?;
    metadata
        .as_object_mut()
        .expect("note serializes as an object")
        .remove("markdown");
    let mut prefix = serde_json::to_vec(&metadata).map_err(anyhow::Error::from)?;
    prefix.pop();
    prefix.extend_from_slice(br#","markdown":""#);
    let content = stream::unfold((source, 0usize), |(source, start)| async move {
        if start == source.len() {
            return None;
        }
        let mut end = (start + 64 * 1024).min(source.len());
        while !source.is_char_boundary(end) {
            end -= 1;
        }
        let json =
            Bytes::from(serde_json::to_vec(&source[start..end]).expect("valid UTF-8 string"));
        Some((json.slice(1..json.len() - 1), (source, end)))
    });
    let stream = stream::iter([Bytes::from(prefix)])
        .chain(content)
        .chain(stream::iter([Bytes::from_static(br#""}"#)]))
        .map(Ok::<_, std::convert::Infallible>);
    Ok((
        [(header::CONTENT_TYPE, "application/json")],
        Body::from_stream(stream),
    )
        .into_response())
}

async fn get_note(
    State(state): State<AppState>,
    Path((library, id)): Path<(String, String)>,
) -> Result<Response, CoreError> {
    require_notebook(&state, &library)?;
    run_read_database(state, move |db| {
        note_response(db.notebook_note(&id).map_err(error)?)
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRequest {
    folder_id: Option<Uuid>,
    #[serde(flatten)]
    note: NoteInput,
}

async fn create_note(
    State(state): State<AppState>,
    Path(library): Path<String>,
    request: Request,
) -> Result<(StatusCode, Response), CoreError> {
    require_notebook(&state, &library)?;
    let input: CreateRequest = note_json(request).await?;
    let note = run_authoritative_database(state, move |db| {
        note_response(
            db.create_note_in_folder(input.note, input.folder_id)
                .map_err(error)?,
        )
    })
    .await?;
    Ok((StatusCode::CREATED, note))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateRequest {
    expected_revision: i64,
    #[serde(flatten)]
    note: NoteInput,
}

fn invalid_json(rejection: JsonRejection) -> CoreError {
    let status = if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        StatusCode::PAYLOAD_TOO_LARGE
    } else {
        StatusCode::BAD_REQUEST
    };
    CoreError::coded(status, "invalid_note", rejection.body_text())
}

async fn update_note(
    State(state): State<AppState>,
    Path((library, id)): Path<(String, String)>,
    request: Request,
) -> Result<Response, CoreError> {
    require_notebook(&state, &library)?;
    let input: UpdateRequest = note_json(request).await?;
    run_authoritative_database(state, move |db| {
        note_response(
            db.save_note(Some((&id, input.expected_revision)), input.note)
                .map_err(error)?,
        )
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteQuery {
    expected_revision: i64,
}

async fn get_tree(
    State(state): State<AppState>,
    Path(library): Path<String>,
) -> Result<Json<NotebookTree>, CoreError> {
    require_notebook(&state, &library)?;
    run_read_database(state, |db| db.notebook_tree().map(Json).map_err(error)).await
}

async fn create_folder(
    State(state): State<AppState>,
    Path(library): Path<String>,
    input: Result<Json<FolderInput>, JsonRejection>,
) -> Result<(StatusCode, Json<NotebookFolder>), CoreError> {
    require_notebook(&state, &library)?;
    let Json(input) = input.map_err(invalid_json)?;
    let folder = run_authoritative_database(state, move |db| {
        db.save_notebook_folder(None, input).map_err(error)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(folder)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateFolder {
    expected_revision: i64,
    #[serde(flatten)]
    folder: FolderInput,
}

async fn update_folder(
    State(state): State<AppState>,
    Path((library, id)): Path<(String, Uuid)>,
    input: Result<Json<UpdateFolder>, JsonRejection>,
) -> Result<Json<NotebookFolder>, CoreError> {
    require_notebook(&state, &library)?;
    let Json(input) = input.map_err(invalid_json)?;
    run_authoritative_database(state, move |db| {
        db.save_notebook_folder(Some((id, input.expected_revision)), input.folder)
            .map(Json)
            .map_err(error)
    })
    .await
}

async fn delete_folder(
    State(state): State<AppState>,
    Path((library, id)): Path<(String, Uuid)>,
    Query(query): Query<DeleteQuery>,
) -> Result<StatusCode, CoreError> {
    require_notebook(&state, &library)?;
    run_authoritative_database(state, move |db| {
        db.delete_notebook_folder(id, query.expected_revision)
            .map_err(error)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveNote {
    #[serde(deserialize_with = "Option::deserialize")]
    parent_id: Option<Uuid>,
    #[serde(deserialize_with = "Option::deserialize")]
    expected_parent_id: Option<Uuid>,
}

async fn move_note(
    State(state): State<AppState>,
    Path((library, id)): Path<(String, Uuid)>,
    input: Result<Json<MoveNote>, JsonRejection>,
) -> Result<StatusCode, CoreError> {
    require_notebook(&state, &library)?;
    let Json(input) = input.map_err(invalid_json)?;
    run_authoritative_database(state, move |db| {
        db.move_notebook_note(id, input.expected_parent_id, input.parent_id)
            .map_err(error)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_note(
    State(state): State<AppState>,
    Path((library, id)): Path<(String, String)>,
    Query(query): Query<DeleteQuery>,
) -> Result<StatusCode, CoreError> {
    require_notebook(&state, &library)?;
    run_authoritative_database(state, move |db| {
        db.delete_note(&id, query.expected_revision).map_err(error)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileQuery {
    name: String,
    #[serde(default)]
    mime_type: String,
}

async fn upload_attachment(
    State(state): State<AppState>,
    Path((library, id)): Path<(String, String)>,
    Query(query): Query<FileQuery>,
    body: Body,
) -> Result<(StatusCode, Json<NoteAttachment>), CoreError> {
    require_notebook(&state, &library)?;
    let file = spool_body(body).await?;
    let attachment = run_authoritative_database(state, move |db| {
        db.collect_note_attachment(&id, query.name, query.mime_type, file)
            .map_err(error)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(attachment)))
}

async fn get_attachment(
    State(state): State<AppState>,
    Path((library, note, id)): Path<(String, String, String)>,
) -> Result<Response, CoreError> {
    require_notebook(&state, &library)?;
    let (mime, size, file) = run_read_database(state, move |db| {
        // Verify all chunks before sending headers. Release the SQLite read
        // snapshot before streaming to the client so slow readers cannot pin WAL.
        let mut file = tempfile::tempfile().map_err(anyhow::Error::from)?;
        let (mime, size) = db
            .write_note_attachment(&note, &id, &mut file)
            .map_err(error)?;
        file.rewind().map_err(anyhow::Error::from)?;
        Ok((mime, size.to_string(), file))
    })
    .await?;
    Ok((
        [
            (header::CONTENT_TYPE, mime.as_str()),
            (header::CONTENT_LENGTH, size.as_str()),
            (header::CONTENT_DISPOSITION, "attachment"),
            (
                header::CONTENT_SECURITY_POLICY,
                "sandbox; default-src 'none'",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(ReaderStream::new(tokio::fs::File::from_std(file))),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn notebook_document_api_requires_enabled_host_plugin() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.apply_migrations().unwrap();
        let app = crate::api::router_with_config(db, crate::api::ApiConfig::default());
        let response = app
            .oneshot(
                Request::get("/api/notebooks/a/notes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "plugin_data_disabled");
    }
}
