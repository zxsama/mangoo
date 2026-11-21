//! Generic plugin data service: `/api/plugin-data/{pluginId}/*`.
//!
//! A Host-engine plugin that requests (and is granted) the `library-data`
//! permission keeps its own records in the active Library Catalog
//! (`db::plugin_records`). The records synchronize with that Library through the
//! ordinary Catalog stream, so a plugin never owns storage, sync transport or
//! schema of its own.
//!
//! - Every route answers `403 plugin_data_disabled` unless `pluginId` is an
//!   enabled, granted, trusted Host-engine plugin that requests
//!   `library-data`. Disabling the plugin turns its data surface off; the data
//!   itself stays in the Library (and keeps synchronizing) so re-enabling the
//!   plugin, on this or another device, finds it intact.
//! - Records are scoped to the plugin: one plugin can never read or write
//!   another plugin's records.
//! - `changes` is a bounded long-poll over the plugin's change counter. It
//!   deliberately does not hold the Library request gate (see
//!   `require_active_library_context`), re-resolves the active Library on every
//!   wake and returns as soon as the Library or the revision differs, so a held
//!   poll never delays a Library switch.

use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::routes::{run_authoritative_database, run_read_database, CoreError};
use super::AppState;
use crate::db::plugin_records::{
    plugin_record_wake, PluginRecord, PluginRecordError, PluginRecordOperation, PluginRecordPage,
    DEFAULT_LIST_LIMIT, MAX_BATCH_OPERATIONS, MAX_LIST_LIMIT,
};

pub const LIBRARY_DATA_PERMISSION: &str = "library-data";
const MAX_WAIT_MS: u64 = 30_000;
/// Remote applies run on the sync coordinator's connection; their wake is a
/// best effort, so a waiter re-reads the committed revision at least this often.
const RECHECK_MS: u64 = 1_000;

pub(super) fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/plugin-data/:plugin_id/records", get(list_records))
        .route(
            "/api/plugin-data/:plugin_id/records/:record_id",
            get(get_record),
        )
        .route("/api/plugin-data/:plugin_id/batch", post(apply_batch))
        .route("/api/plugin-data/:plugin_id/changes", get(wait_for_changes))
        .route(
            "/api/plugin-data/:plugin_id/shared/records",
            get(list_shared_records),
        )
        .route(
            "/api/plugin-data/:plugin_id/shared/records/:record_id",
            get(get_shared_record),
        )
        .route(
            "/api/plugin-data/:plugin_id/shared/batch",
            post(apply_shared_batch),
        )
        .with_state(state)
}

/// Paths whose handler needs no Library request gate (see module docs):
/// the change poll and the cross-Library shared scope.
pub(super) fn is_ungated_path(path: &str) -> bool {
    path.starts_with("/api/plugin-data/")
        && (path.ends_with("/changes") || path.contains("/shared/"))
}

pub(super) fn require_plugin(state: &AppState, plugin_id: &str) -> Result<(), CoreError> {
    let enabled = state
        .library_manager
        .as_ref()
        .map(|manager| {
            manager
                .host_registry()
                .host_plugin_permission_enabled(plugin_id, LIBRARY_DATA_PERMISSION)
        })
        .transpose()?
        .unwrap_or(false);
    if enabled {
        Ok(())
    } else {
        Err(CoreError::coded(
            StatusCode::FORBIDDEN,
            "plugin_data_disabled",
            format!("Plugin {plugin_id} is not enabled for Library data"),
        ))
    }
}

fn map_record_error(error: PluginRecordError) -> CoreError {
    match error {
        PluginRecordError::Invalid(message) => {
            CoreError::coded(StatusCode::BAD_REQUEST, "invalid_plugin_record", message)
        }
        PluginRecordError::NotFound(message) => {
            CoreError::coded(StatusCode::NOT_FOUND, "plugin_record_not_found", message)
        }
        PluginRecordError::Conflict(message) => {
            let code = if message == "record-deleted" {
                "plugin_record_deleted"
            } else {
                "plugin_record_conflict"
            };
            CoreError::coded(StatusCode::CONFLICT, code, message)
        }
        PluginRecordError::Other(error) => CoreError::from(error),
    }
}

// This built-in exposes validated, version-checked documents and owned
// attachments. Its internal Catalog records are a private transport format.
fn require_generic_records(state: &AppState, plugin_id: &str) -> Result<(), CoreError> {
    require_plugin(state, plugin_id)?;
    if plugin_id == crate::db::notebook::PLUGIN {
        return Err(CoreError::coded(
            StatusCode::FORBIDDEN,
            "notebook_api_required",
            "Use the notebook document and attachment API".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    collection: String,
    cursor: Option<String>,
    limit: Option<i64>,
    include_deleted: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginRecordPageResponse {
    library_id: Option<String>,
    #[serde(flatten)]
    page: PluginRecordPage,
}

fn active_library_id(state: &AppState) -> Option<String> {
    state
        .library_manager
        .as_ref()
        .and_then(|manager| manager.active_snapshot().library_uuid)
}

async fn list_records(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    query: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<PluginRecordPageResponse>, CoreError> {
    let Query(query) = query.map_err(|error| {
        CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_plugin_record",
            error.body_text(),
        )
    })?;
    require_generic_records(&state, &plugin_id)?;
    let limit = query.limit.unwrap_or(DEFAULT_LIST_LIMIT);
    if !(1..=MAX_LIST_LIMIT).contains(&limit) {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_plugin_record",
            format!("limit must be between 1 and {MAX_LIST_LIMIT}"),
        ));
    }
    let library_id = active_library_id(&state);
    let page = run_read_database(state, move |database| {
        database
            .list_plugin_records(
                &plugin_id,
                &query.collection,
                query.cursor.as_deref(),
                limit,
                query.include_deleted.unwrap_or(false),
            )
            .map_err(map_record_error)
    })
    .await?;
    Ok(Json(PluginRecordPageResponse { library_id, page }))
}

async fn get_record(
    State(state): State<AppState>,
    Path((plugin_id, record_id)): Path<(String, String)>,
) -> Result<Json<PluginRecord>, CoreError> {
    require_generic_records(&state, &plugin_id)?;
    let record = run_read_database(state, move |database| {
        database
            .get_plugin_record(&plugin_id, &record_id)
            .map_err(map_record_error)
    })
    .await?;
    Ok(Json(record))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BatchRequest {
    operations: Vec<PluginRecordOperation>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchResponse {
    library_id: Option<String>,
    revision: i64,
    records: Vec<PluginRecord>,
}

async fn apply_batch(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    payload: Result<Json<BatchRequest>, JsonRejection>,
) -> Result<Json<BatchResponse>, CoreError> {
    let Json(payload) = payload.map_err(|error| {
        CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_plugin_record",
            error.body_text(),
        )
    })?;
    require_generic_records(&state, &plugin_id)?;
    if payload.operations.is_empty() || payload.operations.len() > MAX_BATCH_OPERATIONS {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_plugin_record",
            format!("a batch holds 1 to {MAX_BATCH_OPERATIONS} operations"),
        ));
    }
    let library_id = active_library_id(&state);
    let (revision, records) = run_authoritative_database(state, move |database| {
        database
            .apply_plugin_record_batch(&plugin_id, payload.operations)
            .map_err(map_record_error)
    })
    .await?;
    Ok(Json(BatchResponse {
        library_id,
        revision,
        records,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChangesQuery {
    after_revision: Option<i64>,
    after_shared_revision: Option<i64>,
    library_id: Option<String>,
    wait_ms: Option<u64>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ChangesResponse {
    library_id: Option<String>,
    revision: i64,
    shared_revision: i64,
    changed: bool,
}

async fn read_revision(
    state: &AppState,
    plugin_id: &str,
) -> Result<(Option<String>, i64), CoreError> {
    let library_id = active_library_id(state);
    if state.resources().is_err() {
        return Ok((None, 0));
    }
    let plugin_id = plugin_id.to_owned();
    let revision = run_read_database(state.clone(), move |database| {
        database
            .plugin_record_revision(&plugin_id)
            .map_err(CoreError::from)
    })
    .await?;
    Ok((library_id, revision))
}

async fn wait_for_changes(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    query: Result<Query<ChangesQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<ChangesResponse>, CoreError> {
    let Query(query) = query.map_err(|error| {
        CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_plugin_record",
            error.body_text(),
        )
    })?;
    require_plugin(&state, &plugin_id)?;
    let wait_ms = query.wait_ms.unwrap_or(0);
    if wait_ms > MAX_WAIT_MS {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_plugin_record",
            format!("waitMs must be between 0 and {MAX_WAIT_MS}"),
        ));
    }
    let after = query.after_revision.unwrap_or(-1);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        // Register before reading so a commit between the read and the wait
        // is never missed (`notify_waiters` keeps no permit).
        let notified = plugin_record_wake().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let (library_id, revision) = read_revision(&state, &plugin_id).await?;
        let shared_revision = shared_revision(&state, &plugin_id)?;
        let changed = revision != after
            || library_id != query.library_id
            || shared_revision != query.after_shared_revision.unwrap_or(-1);
        let now = tokio::time::Instant::now();
        if changed || now >= deadline {
            return Ok(Json(ChangesResponse {
                library_id,
                revision,
                shared_revision,
                changed,
            }));
        }
        let slice = (deadline - now).min(Duration::from_millis(RECHECK_MS));
        let _ = tokio::time::timeout(slice, notified.as_mut()).await;
        // Fail fast if the plugin was disabled while waiting.
        require_plugin(&state, &plugin_id)?;
    }
}

// ------------------------------------------------------------ shared scope

fn registry(state: &AppState) -> Result<&crate::host_registry::HostRegistry, CoreError> {
    state
        .library_manager
        .as_ref()
        .map(|manager| manager.host_registry().as_ref())
        .ok_or_else(|| {
            CoreError::coded(
                StatusCode::FORBIDDEN,
                "plugin_data_disabled",
                "Shared plugin data needs the Host registry".into(),
            )
        })
}

fn shared_revision(state: &AppState, plugin_id: &str) -> Result<i64, CoreError> {
    match state.library_manager.as_ref() {
        Some(manager) => Ok(manager.host_registry().shared_plugin_revision(plugin_id)?),
        None => Ok(0),
    }
}

/// One reconcile at a time per process. The work is idempotent; this only
/// avoids duplicate mirror writes when several windows read at once.
fn reconcile_gate() -> &'static tokio::sync::Mutex<()> {
    static GATE: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn as_anyhow(error: CoreError) -> anyhow::Error {
    anyhow::anyhow!("{}", error.message())
}

/// Merge the shared store and the active Library's mirror collections in
/// both directions; a no-op when neither changed since the last pass.
pub(super) async fn reconcile_shared(state: &AppState, plugin_id: &str) -> anyhow::Result<()> {
    use crate::plugin_shared::plan_reconcile;
    let Some(manager) = state.library_manager.as_ref() else {
        return Ok(());
    };
    let Some(library_id) = active_library_id(state) else {
        return Ok(());
    };
    if state.resources().is_err() {
        return Ok(());
    }
    let _guard = reconcile_gate().lock().await;
    let registry = manager.host_registry();
    let plugin = plugin_id.to_owned();
    let library_revision = run_read_database(state.clone(), {
        let plugin = plugin.clone();
        move |database| {
            database
                .plugin_record_revision(&plugin)
                .map_err(CoreError::from)
        }
    })
    .await
    .map_err(as_anyhow)?;
    let host_revision = registry.shared_plugin_revision(plugin_id)?;
    if registry.shared_mirror_state(plugin_id, &library_id)?
        == Some((library_revision, host_revision))
    {
        return Ok(());
    }
    let mirrors = run_read_database(state.clone(), {
        let plugin = plugin.clone();
        move |database| {
            database
                .list_plugin_mirror_records(&plugin)
                .map_err(CoreError::from)
        }
    })
    .await
    .map_err(as_anyhow)?;
    let host = registry.shared_plugin_records(plugin_id, None)?;
    let known: std::collections::BTreeSet<_> = host.iter().map(|row| row.id).collect();
    let mut foreign = std::collections::BTreeSet::new();
    for (id, _, _) in &mirrors {
        if !known.contains(id) && registry.shared_plugin_record_owner(id)?.is_some() {
            foreign.insert(*id);
        }
    }
    let (host_updates, library_updates) = plan_reconcile(host, mirrors, |id| foreign.contains(id));
    if !host_updates.is_empty() {
        registry.upsert_shared_plugin_records(plugin_id, &host_updates)?;
        plugin_record_wake().notify_waiters();
    }
    let library_revision = if library_updates.is_empty() {
        library_revision
    } else {
        run_authoritative_database(state.clone(), move |database| {
            database
                .apply_plugin_mirror_puts(&plugin, library_updates)
                .map_err(map_record_error)
        })
        .await
        .map_err(as_anyhow)?
    };
    registry.set_shared_mirror_state(
        plugin_id,
        &library_id,
        library_revision,
        registry.shared_plugin_revision(plugin_id)?,
    )?;
    Ok(())
}

async fn reconcile_or_log(state: &AppState, plugin_id: &str) {
    if let Err(error) = reconcile_shared(state, plugin_id).await {
        // The shared store stays authoritative locally; the next pass retries.
        tracing::warn!(%error, plugin_id, "shared plugin data reconcile failed");
    }
}

fn operation_id(operation: &PluginRecordOperation) -> &str {
    match operation {
        PluginRecordOperation::Put { id, .. }
        | PluginRecordOperation::Delete { id }
        | PluginRecordOperation::Restore { id } => id,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SharedListQuery {
    collection: String,
    include_deleted: Option<bool>,
}

async fn list_shared_records(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    query: Result<Query<SharedListQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<PluginRecordPageResponse>, CoreError> {
    let Query(query) = query.map_err(|error| {
        CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_plugin_record",
            error.body_text(),
        )
    })?;
    require_plugin(&state, &plugin_id)?;
    if !crate::plugin_shared::valid_shared_collection(&query.collection) {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_plugin_record",
            format!("invalid shared collection: {}", query.collection),
        ));
    }
    reconcile_or_log(&state, &plugin_id).await;
    let registry = registry(&state)?;
    let revision = registry.shared_plugin_revision(&plugin_id)?;
    let items = registry
        .shared_plugin_records(&plugin_id, Some(&query.collection))?
        .iter()
        .map(|row| row.to_record())
        .filter(|record| query.include_deleted.unwrap_or(false) || !record.deleted)
        .collect();
    Ok(Json(PluginRecordPageResponse {
        library_id: active_library_id(&state),
        page: PluginRecordPage {
            items,
            next_cursor: None,
            revision,
        },
    }))
}

async fn get_shared_record(
    State(state): State<AppState>,
    Path((plugin_id, record_id)): Path<(String, String)>,
) -> Result<Json<PluginRecord>, CoreError> {
    require_plugin(&state, &plugin_id)?;
    let id = crate::db::plugin_records::parse_record_id(&record_id).map_err(map_record_error)?;
    let registry = registry(&state)?;
    if registry.shared_plugin_record_owner(&id)?.is_none() {
        // Perhaps synchronized into the active Library but not pulled yet.
        reconcile_or_log(&state, &plugin_id).await;
    }
    registry
        .shared_plugin_records(&plugin_id, None)?
        .into_iter()
        .find(|row| row.id == id)
        .map(|row| Json(row.to_record()))
        .ok_or_else(|| {
            CoreError::coded(
                StatusCode::NOT_FOUND,
                "plugin_record_not_found",
                format!("record not found: {id}"),
            )
        })
}

async fn apply_shared_batch(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    payload: Result<Json<BatchRequest>, JsonRejection>,
) -> Result<Json<BatchResponse>, CoreError> {
    let Json(payload) = payload.map_err(|error| {
        CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_plugin_record",
            error.body_text(),
        )
    })?;
    require_plugin(&state, &plugin_id)?;
    if payload.operations.is_empty() || payload.operations.len() > MAX_BATCH_OPERATIONS {
        return Err(CoreError::coded(
            StatusCode::BAD_REQUEST,
            "invalid_plugin_record",
            format!("a batch holds 1 to {MAX_BATCH_OPERATIONS} operations"),
        ));
    }
    // Pull first so the edit lands on top of the newest synchronized values.
    reconcile_or_log(&state, &plugin_id).await;
    let registry = registry(&state)?;
    let existing: std::collections::BTreeMap<_, _> = registry
        .shared_plugin_records(&plugin_id, None)?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    let ids: Vec<String> = payload
        .operations
        .iter()
        .map(|operation| operation_id(operation).to_owned())
        .collect();
    for id in &ids {
        if let Ok(id) = crate::db::plugin_records::parse_record_id(id) {
            if !existing.contains_key(&id) && registry.shared_plugin_record_owner(&id)?.is_some() {
                return Err(CoreError::coded(
                    StatusCode::CONFLICT,
                    "plugin_record_conflict",
                    "record belongs to another plugin".into(),
                ));
            }
        }
    }
    let device = registry.device_uuid()?.to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0);
    let changed =
        crate::plugin_shared::apply_shared_operations(&existing, payload.operations, now, &device)
            .map_err(map_record_error)?;
    let mut revision = registry.shared_plugin_revision(&plugin_id)?;
    if !changed.is_empty() {
        revision = registry.upsert_shared_plugin_records(&plugin_id, &changed)?;
        plugin_record_wake().notify_waiters();
        // Push into the active Library's mirror at once.
        reconcile_or_log(&state, &plugin_id).await;
    }
    let rows: std::collections::BTreeMap<_, _> = registry
        .shared_plugin_records(&plugin_id, None)?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    let records = ids
        .iter()
        .filter_map(|id| uuid::Uuid::parse_str(id).ok())
        .filter_map(|id| rows.get(&id).map(|row| row.to_record()))
        .collect();
    Ok(Json(BatchResponse {
        library_id: active_library_id(&state),
        revision,
        records,
    }))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    use crate::api::{router_with_config, ApiConfig};
    use crate::db::Database;

    fn database() -> Database {
        let database = Database::open_in_memory().expect("open");
        database.apply_migrations().expect("migrate");
        database
    }

    /// Without an enabled, granted Host plugin requesting `library-data`,
    /// every plugin-data route is off and nothing is read or written.
    #[tokio::test]
    async fn plugin_data_routes_are_forbidden_unless_the_plugin_is_enabled() {
        let app = router_with_config(database(), ApiConfig::default());
        for (method, uri, body) in [
            (Method::GET, "/api/plugin-data/com.mangoo.todo-notes/records?collection=items", ""),
            (Method::GET, "/api/plugin-data/com.mangoo.todo-notes/changes?waitMs=0", ""),
            (
                Method::GET,
                "/api/plugin-data/com.mangoo.todo-notes/records/0192f3c5-6e1a-7a3e-8a4b-2f6d1c9e7b10",
                "",
            ),
            (
                Method::POST,
                "/api/plugin-data/com.mangoo.todo-notes/batch",
                r#"{"operations":[{"op":"put","id":"0192f3c5-6e1a-7a3e-8a4b-2f6d1c9e7b10","collection":"items","fields":{"title":"x"}}]}"#,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("content-type", "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {}", String::from_utf8_lossy(&bytes));
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(json["code"], "plugin_data_disabled");
        }
    }

    #[test]
    fn only_the_change_poll_skips_the_request_gate() {
        assert!(super::is_ungated_path(
            "/api/plugin-data/com.mangoo.todo-notes/changes"
        ));
        assert!(!super::is_ungated_path(
            "/api/plugin-data/com.mangoo.todo-notes/batch"
        ));
        assert!(!super::is_ungated_path(
            "/api/plugin-data/com.mangoo.todo-notes/records"
        ));
    }
}
