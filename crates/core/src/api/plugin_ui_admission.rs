//! API ordering for sandbox-delegated requests and plugin lifecycle mutations.
//! Engine generation fences still govern background work and durable effects.
use std::{collections::HashMap, sync::Arc};

use axum::extract::{FromRequestParts, MatchedPath, Path, Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::AppState;
use crate::host_registry::HostRegistry;
use crate::plugins::manifest::{HostUiOperation, ManifestRecord, Permission};

const UI_ID_HEADER: &str = "x-mangoo-plugin-ui-id";
const UI_EPOCH_HEADER: &str = "x-mangoo-plugin-ui-epoch";

#[derive(Debug, PartialEq, Eq)]
struct UiSession {
    plugin_id: String,
    lifecycle_epoch: i64,
}

fn session(headers: &HeaderMap) -> Result<Option<UiSession>, ()> {
    if !headers.contains_key(UI_ID_HEADER) && !headers.contains_key(UI_EPOCH_HEADER) {
        return Ok(None);
    }
    if headers.get_all(UI_ID_HEADER).iter().count() != 1
        || headers.get_all(UI_EPOCH_HEADER).iter().count() != 1
    {
        return Err(());
    }
    let id = headers
        .get(UI_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(())?;
    let epoch = headers
        .get(UI_EPOCH_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(())?;
    let parts: Vec<_> = id.split('.').collect();
    if id.len() > 255
        || parts.len() < 2
        || parts.iter().any(|part| part.is_empty())
        || !parts[0]
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        || parts[1..].iter().any(|part| {
            !part.as_bytes()[0].is_ascii_lowercase() && !part.as_bytes()[0].is_ascii_digit()
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        || epoch.is_empty()
        || !epoch.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(());
    }
    let lifecycle_epoch = epoch.parse::<i64>().map_err(|_| ())?;
    if !(1..=9_007_199_254_740_991).contains(&lifecycle_epoch) {
        return Err(());
    }
    Ok(Some(UiSession {
        plugin_id: id.to_owned(),
        lifecycle_epoch,
    }))
}

fn lifecycle_write(method: &Method, route: &str) -> bool {
    // Auth completion callbacks may mutate identity despite being GET routes.
    route.starts_with("/api/plugin-auth/callback/")
        || ((route == "/api/plugins"
            || route.starts_with("/api/plugins/")
            || route == "/api/libraries"
            || route.starts_with("/api/libraries/")
            || route == "/api/sync"
            || route.starts_with("/api/sync/"))
            && !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS))
}

#[derive(Default)]
pub(super) struct BodyTargets {
    pub plugin_id: Option<String>,
    pub account_uuid: Option<String>,
    pub plan: Option<(&'static str, String)>,
    pub restore_items: Vec<(String, String)>,
}

fn target_changed() -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "code": "plugin-ui-target-changed", "error": "Plugin UI target ownership changed"
        })),
    )
        .into_response()
}

fn body_has_targets(route: &str) -> bool {
    matches!(
        route,
        "/api/libraries/:library_uuid/sync/preflight"
            | "/api/libraries/:library_uuid/sync/enable"
            | "/api/libraries/:library_uuid/sync/reconnect-preflight"
            | "/api/libraries/:library_uuid/sync/reconnect-commit"
            | "/api/libraries/:library_uuid/sync/unbind-commit"
            | "/api/sync/restores/preflight"
            | "/api/sync/restores"
    )
}

fn local_setup_allowed(route: &str) -> bool {
    matches!(
        route,
        "/api/libraries/:library_uuid/sync/preflight"
            | "/api/libraries/:library_uuid/sync/enable"
            | "/api/libraries/:library_uuid/sync/reconnect-preflight"
            | "/api/libraries/:library_uuid/sync/reconnect-commit"
            | "/api/libraries/:library_uuid/sync-folders/enrollment/preflight"
            | "/api/libraries/:library_uuid/sync-folders/enrollment/commit"
            | "/api/roots"
    )
}

fn target_owned(
    registry: &HostRegistry,
    expected: &UiSession,
    method: &Method,
    route: &str,
    paths: &HashMap<String, String>,
    body: &BodyTargets,
    active_library: Option<&str>,
) -> anyhow::Result<bool> {
    let Some(plugin) = registry.host_plugin(&expected.plugin_id)? else {
        return Ok(false);
    };
    let account = plugin.account_uuid.as_deref();
    let account_owned = |id: &str| -> anyhow::Result<bool> {
        Ok(account == Some(id)
            && registry.plugin_id_for_account(id)?.as_deref() == Some(&expected.plugin_id))
    };
    let library_owned = |id: &str, allow_local: bool, allow_absent: bool| -> anyhow::Result<bool> {
        match registry.plugin_ui_library_owner(id)? {
            Some((Some(owner), Some(owner_account), _)) => {
                Ok(owner == expected.plugin_id && account_owned(&owner_account)?)
            }
            Some((None, None, state)) => Ok(allow_local && state == "local-only"),
            None => Ok(allow_absent),
            _ => Ok(false),
        }
    };
    if paths
        .get("plugin_id")
        .is_some_and(|id| id != &expected.plugin_id)
        || body
            .plugin_id
            .as_ref()
            .is_some_and(|id| id != &expected.plugin_id)
    {
        return Ok(false);
    }
    if let Some(id) = paths.get("account_uuid").or(body.account_uuid.as_ref()) {
        if !account_owned(id)? {
            return Ok(false);
        }
    }
    if let Some(id) = paths.get("library_uuid") {
        // Remote inventory is scoped by its account, and may not have a local copy.
        let remote = route == "/api/sync/remote-libraries/:account_uuid/:library_uuid/roots";
        if !library_owned(id, local_setup_allowed(route) || remote, remote)? {
            return Ok(false);
        }
    }
    if route == "/api/roots" {
        let Some(library) = active_library else {
            return Ok(false);
        };
        if method != Method::GET || !library_owned(library, true, false)? {
            return Ok(false);
        }
    }
    if let Some(id) = paths.get("restore_uuid") {
        let Some(restore) = registry.restore_plan(id)? else {
            return Ok(false);
        };
        if !account_owned(&restore.account_uuid)?
            || !library_owned(&restore.library_uuid, false, true)?
        {
            return Ok(false);
        }
    }
    for (account_id, library_id) in &body.restore_items {
        if !account_owned(account_id)? || !library_owned(library_id, true, true)? {
            return Ok(false);
        }
    }
    if let Some((kind, id)) = &body.plan {
        let Some((library, plugin_id, account_id)) = registry.plugin_ui_plan_owner(kind, id)?
        else {
            return Ok(false);
        };
        if paths.get("library_uuid") != Some(&library)
            || plugin_id != expected.plugin_id
            || !account_owned(&account_id)?
        {
            return Ok(false);
        }
    }
    // These global mutation routes do not have a plugin-scoped target.
    if matches!(
        route,
        "/api/sync/wake"
            | "/api/libraries/:library_uuid/sync-binding"
            | "/api/libraries/open"
            | "/api/libraries/:library_uuid/registration"
    ) {
        return Ok(false);
    }
    if route == "/api/libraries" && method != Method::GET {
        return Ok(false);
    }
    Ok(true)
}

fn unavailable() -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "code": "plugin-ui-session-changed", "error": "Plugin UI session is no longer available"
        })),
    )
        .into_response()
}

fn current_session(registry: &HostRegistry, expected: &UiSession) -> anyhow::Result<bool> {
    let Some(source) = registry.plugin_ui_source(&expected.plugin_id)? else {
        return Ok(false);
    };
    if source.lifecycle_epoch != expected.lifecycle_epoch {
        return Ok(false);
    }
    let manifest: ManifestRecord = serde_json::from_str(&source.manifest_json)?;
    // The initial view document performs embedded manifest/document trust
    // verification. Dispatch keeps that selected declaration and its grants.
    Ok(manifest.id == expected.plugin_id
        && manifest.version == source.version
        && manifest.requested_permissions == source.requested_permissions
        && manifest
            .contributions
            .as_ref()
            .is_some_and(|c| c.ui.is_some())
        && source.granted_permissions.iter().any(|permission| {
            matches!(permission,
            Permission::HostUi { operations } if operations.contains(&HostUiOperation::CoreApi))
        }))
}

pub(super) async fn admit_plugin_ui(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let expected = match session(request.headers()) {
        Ok(value) => value,
        Err(()) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "code": "invalid-plugin-ui-session", "error": "Both plugin UI identity headers are required"
        }))).into_response(),
    };
    // MatchedPath is supplied by the router and cannot be confused by encoded
    // path segments. The raw path is only a fallback for unmapped requests.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str())
        .unwrap_or_else(|| request.uri().path())
        .to_owned();
    let is_write = lifecycle_write(request.method(), &route);
    let method = request.method().clone();
    let is_view = route.starts_with("/api/plugins/") && route.contains("/views/");
    let gate = Arc::clone(&state.plugin_ui_gate);
    // These are disjoint; a self-mutation acquires the write lease directly.
    let write_lease = if is_write {
        Some(Arc::clone(&gate).write_owned().await)
    } else {
        None
    };
    let read_lease = if !is_write && (expected.is_some() || is_view) {
        Some(gate.read_owned().await)
    } else {
        None
    };
    if let Some(expected) = expected {
        let Some(manager) = state.library_manager.as_ref() else {
            return unavailable();
        };
        let registry = Arc::clone(manager.host_registry());
        let active_library = manager.active_snapshot().library_uuid;
        let (mut parts, body) = request.into_parts();
        let paths = if route.contains(':') {
            match Path::<HashMap<String, String>>::from_request_parts(&mut parts, &()).await {
                Ok(Path(paths)) => paths,
                Err(rejection) => return rejection.into_response(),
            }
        } else {
            HashMap::new()
        };
        let (targets, body) =
            if body_has_targets(&route) {
                let bytes = match axum::body::to_bytes(body, 2 * 1024 * 1024).await {
                    Ok(bytes) => bytes,
                    Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
                };
                let targets = match super::routes::plugin_ui_body_targets(&route, &bytes) {
                Ok(targets) => targets,
                Err(_) => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                    "code":"invalid-plugin-ui-target", "error":"Invalid plugin UI request body"
                }))).into_response(),
            };
                (targets, axum::body::Body::from(bytes))
            } else {
                (BodyTargets::default(), body)
            };
        request = Request::from_parts(parts, body);
        match tokio::task::spawn_blocking(move || -> anyhow::Result<(bool, bool)> {
            if !current_session(&registry, &expected)? {
                return Ok((false, false));
            }
            Ok((
                true,
                target_owned(
                    &registry,
                    &expected,
                    &method,
                    &route,
                    &paths,
                    &targets,
                    active_library.as_deref(),
                )?,
            ))
        })
        .await
        {
            Ok(Ok((true, true))) => (),
            Ok(Ok((false, _))) => return unavailable(),
            Ok(Ok((true, false))) => return target_changed(),
            _ => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "code": "plugin-ui-admission-failed", "error": "Plugin UI admission failed"
                    })),
                )
                    .into_response()
            }
        }
    }
    if write_lease.is_none() && read_lease.is_none() {
        return next.run(request).await;
    }
    // A disconnected iframe may cancel the response future while a handler's
    // blocking operation is still running. Keep ownership of the lease in the
    // actual handler task, not the caller's wait. Existing handler/engine
    // deadlines bound operations; this task adds neither retries nor workers.
    match tokio::spawn(async move {
        let _leases = (write_lease, read_lease);
        next.run(request).await
    })
    .await
    {
        Ok(response) => response,
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "code": "plugin-ui-handler-failed", "error": "Plugin UI request failed"
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::get, routing::post, Router};
    use std::future::{poll_fn, Future};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Poll;
    use tokio::sync::{Notify, Semaphore};
    use tower::ServiceExt;

    const ID: &str = "com.example.ui";
    const OTHER_ID: &str = "com.example.other";
    const ACCOUNT: &str = "ui-account";
    const OTHER_ACCOUNT: &str = "other-account";
    const LIBRARY: &str = "00000000-0000-7000-8000-000000000001";

    struct Fixture {
        state: AppState,
        _cleanup: Cleanup,
    }
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl Fixture {
        fn new() -> Self {
            let base =
                std::env::temp_dir().join(format!("mangoo-ui-admission-{}", uuid::Uuid::now_v7()));
            let registry = Arc::new(HostRegistry::open(base.join("host.sqlite")).unwrap());
            let mut manifest: serde_json::Value = serde_json::from_str(include_str!(
                "../../../../contracts/plugins/fixtures/valid-ui-provider-v2.json"
            ))
            .unwrap();
            manifest["contributions"]["configuration"] = serde_json::json!([{
                "id":"workspace", "label":"Workspace", "description":"", "format":"text", "required":false,
                "environment":null, "defaultValue":null, "choices":[], "hidden":false, "debugOnly":false
            }]);
            let permissions = serde_json::to_string(&manifest["requestedPermissions"]).unwrap();
            registry
                .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                    plugin_id: ID,
                    version: "1.0.0",
                    manifest_json: &manifest.to_string(),
                    entry_relative_path: "bin/provider.exe",
                    entry_sha256: &[1; 32],
                    install_root: &base.join("plugins").join(ID),
                    trust_policy_id: "embedded-v1",
                    requested_permissions_json: &permissions,
                })
                .unwrap();
            registry
                .upsert_plugin_account(ACCOUNT, ID, None, "signed-out")
                .unwrap();
            registry.set_plugin_grants(ID, &permissions, true).unwrap();
            registry.finish_plugin_runtime_restart(ID).unwrap();
            registry.set_plugin_intent(ID, true, true).unwrap();
            let active = Arc::new(std::sync::RwLock::new(
                crate::library_manager::ActiveSnapshot {
                    library_uuid: None,
                    context_revision: 0,
                    runtime_generation: 0,
                    runtime: None,
                },
            ));
            let request_gate = Arc::new(tokio::sync::RwLock::new(()));
            let secret_store: Option<Arc<dyn crate::plugins::secret_store::SecretStore>> = Some(
                Arc::new(crate::plugins::secret_store::MemorySecretStore::default()),
            );
            let manager = Arc::new(crate::library_manager::LibraryManager::new(
                registry,
                base.join("libraries"),
                base.join("plugins"),
                secret_store.clone(),
                active.clone(),
                request_gate.clone(),
            ));
            let state = AppState {
                playback_sessions: Arc::new(Default::default()),
                active,
                request_gate,
                plugin_ui_gate: Arc::new(tokio::sync::RwLock::new(())),
                library_manager: Some(manager),
                plugin_auth_flows: Arc::new(Default::default()),
                plugin_auth_workers: Arc::new(Default::default()),
                plugin_auth_begin_gate: Arc::new(Default::default()),
                secret_store,
                notification_outbox_test_probe: None,
            };
            Self {
                state,
                _cleanup: Cleanup(base),
            }
        }
        fn registry(&self) -> &Arc<HostRegistry> {
            self.state.library_manager.as_ref().unwrap().host_registry()
        }
        fn epoch(&self) -> i64 {
            self.registry().plugin_lifecycle_epoch(ID).unwrap()
        }
        fn app(&self) -> Router {
            crate::api::routes::router(self.state.clone()).layer(
                axum::middleware::from_fn_with_state(self.state.clone(), admit_plugin_ui),
            )
        }

        fn local_library(&self) {
            let registry = self.registry();
            let manifest =
                include_str!("../../../../contracts/plugins/fixtures/valid-ui-provider-v2.json")
                    .replace(ID, OTHER_ID);
            let permissions = serde_json::from_str::<serde_json::Value>(&manifest).unwrap()
                ["requestedPermissions"]
                .to_string();
            registry
                .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                    plugin_id: OTHER_ID,
                    version: "1.0.0",
                    manifest_json: &manifest,
                    entry_relative_path: "bin/provider.exe",
                    entry_sha256: &[1; 32],
                    install_root: &self._cleanup.0.join("plugins").join(OTHER_ID),
                    trust_policy_id: "embedded-v1",
                    requested_permissions_json: &permissions,
                })
                .unwrap();
            registry
                .upsert_plugin_account(OTHER_ACCOUNT, OTHER_ID, None, "signed-out")
                .unwrap();
            registry
                .set_plugin_grants(OTHER_ID, &permissions, true)
                .unwrap();
            registry.finish_plugin_runtime_restart(OTHER_ID).unwrap();
            for (plugin, account) in [(ID, ACCOUNT), (OTHER_ID, OTHER_ACCOUNT)] {
                registry
                    .upsert_plugin_account(account, plugin, None, "authenticated")
                    .unwrap();
                registry.set_plugin_intent(plugin, true, true).unwrap();
                registry
                    .record_plugin_runtime_ready(
                        plugin,
                        "fixture-session",
                        &["sync-provider.v1".into()],
                    )
                    .unwrap();
            }
            // Initial local registry fixture only; binding transitions below use
            // the production registry APIs and real middleware/command routes.
            let db = rusqlite::Connection::open(self._cleanup.0.join("host.sqlite")).unwrap();
            db.execute("INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES (?1,'root','Fixture','local','root','ready','local-only','available',1,'marker',1,1)", [LIBRARY]).unwrap();
        }

        fn rebind_app(&self) -> Router {
            let registry = self.registry().clone();
            crate::api::routes::router(self.state.clone())
                .route(
                    "/api/libraries/:library_uuid/fixture-rebind",
                    post(move |Path(library): Path<String>| {
                        let registry = registry.clone();
                        async move {
                            tokio::task::spawn_blocking(move || {
                                if let Some(binding) =
                                    registry.binding_transport(&library, "catalog").unwrap()
                                {
                                    registry
                                        .abort_sync_binding(&library, binding.binding_epoch)
                                        .unwrap();
                                }
                                registry
                                    .begin_sync_binding(&library, OTHER_ID, OTHER_ACCOUNT)
                                    .unwrap();
                            })
                            .await
                            .unwrap();
                            StatusCode::OK
                        }
                    }),
                )
                .layer(axum::middleware::from_fn_with_state(
                    self.state.clone(),
                    admit_plugin_ui,
                ))
        }
    }

    fn request(method: Method, path: &str, epoch: Option<i64>, body: &str) -> Request {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(epoch) = epoch {
            builder = builder
                .header(UI_ID_HEADER, ID)
                .header(UI_EPOCH_HEADER, epoch.to_string());
        }
        builder.body(Body::from(body.to_owned())).unwrap()
    }

    #[tokio::test]
    async fn pending_disable_writer_fences_queued_old_ui_reader_and_disable_enable_aba() {
        let fixture = Fixture::new();
        let epoch = fixture.epoch();
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let handler = {
            let calls = calls.clone();
            let entered = entered.clone();
            let release = release.clone();
            move || {
                let calls = calls.clone();
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        entered.notify_one();
                        let _permit = release.acquire().await.unwrap();
                    }
                    StatusCode::OK
                }
            }
        };
        let app = crate::api::routes::router(fixture.state.clone())
            .route("/api/ui-probe", get(handler))
            .layer(axum::middleware::from_fn_with_state(
                fixture.state.clone(),
                admit_plugin_ui,
            ));
        let first = tokio::spawn(app.clone().oneshot(request(
            Method::GET,
            "/api/ui-probe",
            Some(epoch),
            "",
        )));
        entered.notified().await;
        let mut disable = Box::pin(app.clone().oneshot(request(
            Method::POST,
            &format!("/api/plugins/{ID}/disable"),
            None,
            "",
        )));
        // Poll once to register the writer behind the active read lease.
        poll_fn(|cx| {
            assert!(disable.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        let mut queued =
            Box::pin(
                app.clone()
                    .oneshot(request(Method::GET, "/api/ui-probe", Some(epoch), "")),
            );
        poll_fn(|cx| {
            assert!(queued.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.add_permits(1);
        assert_eq!(first.await.unwrap().unwrap().status(), StatusCode::OK);
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(3), disable)
                .await
                .unwrap()
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(queued.await.unwrap().status(), StatusCode::CONFLICT);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    &format!("/api/plugins/{ID}/enable"),
                    None,
                    ""
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert!(fixture.epoch() > epoch);
        assert_eq!(
            app.clone()
                .oneshot(request(Method::GET, "/api/ui-probe", Some(epoch), ""))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            app.oneshot(request(
                Method::GET,
                "/api/ui-probe",
                Some(fixture.epoch()),
                ""
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn ui_self_configuration_and_signout_take_one_write_lease_without_upgrade_deadlock() {
        let fixture = Fixture::new();
        let app = fixture.app();
        let epoch = fixture.epoch();
        let plan = fixture
            .registry()
            .plan_plugin_configuration(
                ID,
                &std::collections::BTreeMap::from([("workspace".into(), Some("second".into()))]),
                1,
            )
            .unwrap();
        let body =
            serde_json::json!({"planUuid":plan.plan_uuid,"expectedRevision":1,"confirmed":true})
                .to_string();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            app.clone().oneshot(request(
                Method::PUT,
                &format!("/api/plugins/{ID}/configuration"),
                Some(epoch),
                &body,
            )),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(fixture.epoch() > epoch);
        let before_signout = fixture.epoch();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            app.clone().oneshot(request(
                Method::POST,
                &format!("/api/plugins/{ID}/auth/sign-out"),
                Some(before_signout),
                "",
            )),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(fixture.epoch() > before_signout);
        assert_eq!(
            app.oneshot(request(
                Method::GET,
                &format!("/api/plugins/{ID}"),
                Some(before_signout),
                ""
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::CONFLICT
        );
    }

    #[tokio::test]
    async fn ui_admission_does_not_bypass_active_library_revision_fence() {
        let fixture = Fixture::new();
        let app = Router::new()
            .route("/api/ui-probe", post(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                fixture.state.clone(),
                crate::api::attach_library_context,
            ))
            .layer(axum::middleware::from_fn_with_state(
                fixture.state.clone(),
                admit_plugin_ui,
            ));
        for revision in [None, Some("1")] {
            let mut request = request(Method::POST, "/api/ui-probe", Some(fixture.epoch()), "");
            if let Some(revision) = revision {
                request
                    .headers_mut()
                    .insert("x-mangoo-context-revision", revision.parse().unwrap());
            }
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["code"], "stale_library_context");
        }
    }

    #[tokio::test]
    async fn canceled_ui_response_keeps_admission_until_handler_completion() {
        let fixture = Fixture::new();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let handler = {
            let entered = entered.clone();
            let release = release.clone();
            move || {
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    entered.notify_one();
                    let _permit = release.acquire().await.unwrap();
                    StatusCode::OK
                }
            }
        };
        let app = crate::api::routes::router(fixture.state.clone())
            .route("/api/ui-probe", get(handler))
            .layer(axum::middleware::from_fn_with_state(
                fixture.state.clone(),
                admit_plugin_ui,
            ));
        let ui_waiter = tokio::spawn(app.clone().oneshot(request(
            Method::GET,
            "/api/ui-probe",
            Some(fixture.epoch()),
            "",
        )));
        entered.notified().await;
        ui_waiter.abort();
        assert!(ui_waiter.await.unwrap_err().is_cancelled());
        let mut disable = Box::pin(app.oneshot(request(
            Method::POST,
            &format!("/api/plugins/{ID}/disable"),
            None,
            "",
        )));
        poll_fn(|cx| {
            assert!(disable.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert!(fixture.registry().plugin_ui_source(ID).unwrap().is_some());
        release.add_permits(1);
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(3), disable)
                .await
                .unwrap()
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert!(fixture.registry().plugin_ui_source(ID).unwrap().is_none());
    }

    async fn assert_target_changed(response: Response) {
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "plugin-ui-target-changed");
    }

    #[tokio::test]
    async fn non_ui_rebind_writer_precedes_and_rejects_old_ui_pause_with_same_epoch() {
        let fixture = Fixture::new();
        fixture.local_library();
        fixture
            .registry()
            .begin_sync_binding(LIBRARY, ID, ACCOUNT)
            .unwrap();
        let epoch = fixture.epoch();
        let app = fixture.rebind_app();
        let barrier = fixture.state.plugin_ui_gate.clone().read_owned().await;
        let mut rebind = Box::pin(app.clone().oneshot(request(
            Method::POST,
            &format!("/api/libraries/{LIBRARY}/fixture-rebind"),
            None,
            "",
        )));
        poll_fn(|cx| {
            assert!(rebind.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        let mut pause_request = request(
            Method::POST,
            &format!("/api/libraries/{LIBRARY}/sync/pause"),
            Some(epoch),
            r#"{"operationUuid":"must-not-pause-other"}"#,
        );
        pause_request
            .headers_mut()
            .insert("x-mangoo-context-revision", "0".parse().unwrap());
        let mut pause = Box::pin(app.oneshot(pause_request));
        poll_fn(|cx| {
            assert!(pause.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(barrier);
        assert_eq!(rebind.await.unwrap().status(), StatusCode::OK);
        assert_target_changed(pause.await.unwrap()).await;
        assert_eq!(fixture.epoch(), epoch);
        let binding = fixture
            .registry()
            .binding_transport(LIBRARY, "catalog")
            .unwrap()
            .unwrap();
        assert_eq!(binding.plugin_id, OTHER_ID);
        assert_eq!(binding.state, "preparing");
    }

    #[tokio::test]
    async fn local_preflight_then_rebind_rejects_enable_commit_and_foreign_plan() {
        let fixture = Fixture::new();
        fixture.local_library();
        let epoch = fixture.epoch();
        let app = fixture.rebind_app();
        let preflight = app.clone().oneshot(request(Method::POST,
            &format!("/api/libraries/{LIBRARY}/sync/preflight"), Some(epoch),
            &serde_json::json!({"providerId":ID,"accountUuid":ACCOUNT,"remoteLibraryUuid":null}).to_string())).await.unwrap();
        assert_eq!(preflight.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(preflight.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let token = body["preflightToken"].as_str().unwrap();
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    &format!("/api/libraries/{LIBRARY}/fixture-rebind"),
                    None,
                    ""
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let commit = app.clone().oneshot(request(Method::POST,
            &format!("/api/libraries/{LIBRARY}/sync/enable"), Some(epoch),
            &serde_json::json!({"operationUuid":"must-not-enable-other","providerId":ID,"accountUuid":ACCOUNT,"preflightToken":token}).to_string())).await.unwrap();
        assert_target_changed(commit).await;
        assert_eq!(fixture.epoch(), epoch);
        // The denied request never consumed the original preflight token.
        fixture
            .registry()
            .consume_sync_preflight_token(token, LIBRARY, ID, ACCOUNT)
            .unwrap();

        let binding = fixture
            .registry()
            .binding_transport(LIBRARY, "catalog")
            .unwrap()
            .unwrap();
        fixture
            .registry()
            .abort_sync_binding(LIBRARY, binding.binding_epoch)
            .unwrap();
        let foreign_token = fixture
            .registry()
            .create_sync_preflight_token(LIBRARY, OTHER_ID, OTHER_ACCOUNT, None)
            .unwrap();
        let commit = app.oneshot(request(Method::POST,
            &format!("/api/libraries/{LIBRARY}/sync/enable"), Some(epoch),
            &serde_json::json!({"operationUuid":"must-not-use-foreign-plan","providerId":ID,"accountUuid":ACCOUNT,"preflightToken":foreign_token}).to_string())).await.unwrap();
        assert_target_changed(commit).await;
    }

    #[tokio::test]
    async fn target_rules_reject_absent_foreign_and_encoded_targets_but_allow_local_setup() {
        let fixture = Fixture::new();
        fixture.local_library();
        let epoch = fixture.epoch();
        let app = fixture.app();
        for library in [LIBRARY, "absent"] {
            assert_target_changed(
                app.clone()
                    .oneshot(request(
                        Method::POST,
                        &format!("/api/libraries/{library}/sync/pause"),
                        Some(epoch),
                        r#"{"operationUuid":"not-owned"}"#,
                    ))
                    .await
                    .unwrap(),
            )
            .await;
        }
        assert_target_changed(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    "/api/libraries/absent/sync/preflight",
                    Some(epoch),
                    &serde_json::json!({"providerId":ID,"accountUuid":ACCOUNT}).to_string(),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_target_changed(
            app.clone()
                .oneshot(request(
                    Method::GET,
                    "/api/plugins/com%2Eexample%2Eother",
                    Some(epoch),
                    "",
                ))
                .await
                .unwrap(),
        )
        .await;
        fixture
            .registry()
            .begin_sync_binding(LIBRARY, OTHER_ID, OTHER_ACCOUNT)
            .unwrap();
        assert_target_changed(app.clone().oneshot(request(Method::POST,
            "/api/sync/restores/preflight", Some(epoch),
            &serde_json::json!({"items":[{"accountUuid":ACCOUNT,"libraryUuid":LIBRARY,"name":"Fixture","placementBase":"target"}]}).to_string())).await.unwrap()).await;
        assert_target_changed(app.oneshot(request(Method::POST,
            "/api/sync/restores/preflight", Some(epoch),
            &serde_json::json!({"items":[{"accountUuid":OTHER_ACCOUNT,"libraryUuid":"absent","name":"Fixture","placementBase":"target"}]}).to_string())).await.unwrap()).await;
    }

    #[tokio::test]
    async fn local_and_absent_exceptions_are_explicit_and_restore_plans_remain_owned() {
        let fixture = Fixture::new();
        fixture.local_library();
        let expected = UiSession {
            plugin_id: ID.into(),
            lifecycle_epoch: fixture.epoch(),
        };
        let paths = HashMap::from([("library_uuid".into(), LIBRARY.into())]);
        for route in [
            "/api/libraries/:library_uuid/sync/preflight",
            "/api/libraries/:library_uuid/sync/enable",
            "/api/libraries/:library_uuid/sync/reconnect-preflight",
            "/api/libraries/:library_uuid/sync-folders/enrollment/preflight",
            "/api/libraries/:library_uuid/sync-folders/enrollment/commit",
        ] {
            assert!(target_owned(
                fixture.registry(),
                &expected,
                &Method::POST,
                route,
                &paths,
                &BodyTargets::default(),
                None
            )
            .unwrap());
            let absent = HashMap::from([("library_uuid".into(), "absent".into())]);
            assert!(!target_owned(
                fixture.registry(),
                &expected,
                &Method::POST,
                route,
                &absent,
                &BodyTargets::default(),
                None
            )
            .unwrap());
        }
        let mut restore = BodyTargets {
            restore_items: vec![(ACCOUNT.into(), "new-library".into())],
            ..Default::default()
        };
        assert!(target_owned(
            fixture.registry(),
            &expected,
            &Method::POST,
            "/api/sync/restores/preflight",
            &HashMap::new(),
            &restore,
            None
        )
        .unwrap());
        restore.restore_items[0].0 = OTHER_ACCOUNT.into();
        assert!(!target_owned(
            fixture.registry(),
            &expected,
            &Method::POST,
            "/api/sync/restores/preflight",
            &HashMap::new(),
            &restore,
            None
        )
        .unwrap());

        let db = rusqlite::Connection::open(fixture._cleanup.0.join("host.sqlite")).unwrap();
        db.execute("INSERT INTO sync_reconnect_plans(plan_uuid,library_uuid,account_uuid,remote_library_uuid,state,created_at,updated_at) VALUES ('foreign-plan',?1,?2,'remote','planned',1,1)", rusqlite::params![LIBRARY,OTHER_ACCOUNT]).unwrap();
        let foreign_plan = fixture.app().oneshot(request(Method::POST,
            &format!("/api/libraries/{LIBRARY}/sync/reconnect-commit"), Some(fixture.epoch()),
            r#"{"operationUuid":"reconnect-other","planUuid":"foreign-plan","choice":"publish-new-remote"}"#)).await.unwrap();
        assert_target_changed(foreign_plan).await;
        db.execute("INSERT INTO sync_orchestration_operations(operation_uuid,kind,request_json,state,phase,created_at,updated_at) VALUES ('restore-operation','restore','{}','running','preparing',1,1)", []).unwrap();
        db.execute("INSERT INTO sync_restore_plans(restore_uuid,operation_uuid,account_uuid,library_uuid,name,managed_root,descriptor_sha256,state,phase,created_at,updated_at) VALUES ('foreign-restore','restore-operation',?1,'new-library','Restore','target',zeroblob(32),'planned','preparing',1,1)", [OTHER_ACCOUNT]).unwrap();
        assert_target_changed(
            fixture
                .app()
                .oneshot(request(
                    Method::GET,
                    "/api/sync/restores/foreign-restore",
                    Some(fixture.epoch()),
                    "",
                ))
                .await
                .unwrap(),
        )
        .await;
    }

    #[test]
    fn ui_headers_and_lifecycle_route_classification_are_closed() {
        let mut headers = HeaderMap::new();
        assert_eq!(session(&headers).unwrap(), None);
        headers.insert(UI_ID_HEADER, "com.example.ui".parse().unwrap());
        assert!(session(&headers).is_err());
        for value in ["0", "-1", "+1", "9007199254740992", "1,2"] {
            headers.insert(UI_EPOCH_HEADER, value.parse().unwrap());
            assert!(session(&headers).is_err());
        }
        headers.insert(UI_EPOCH_HEADER, "7".parse().unwrap());
        assert_eq!(session(&headers).unwrap().unwrap().lifecycle_epoch, 7);
        headers.append(UI_EPOCH_HEADER, "7".parse().unwrap());
        assert!(session(&headers).is_err());
        assert!(lifecycle_write(&Method::POST, "/api/plugins/discover"));
        assert!(lifecycle_write(
            &Method::PUT,
            "/api/plugins/:id/configuration"
        ));
        assert!(lifecycle_write(
            &Method::GET,
            "/api/plugin-auth/callback/:flow"
        ));
        assert!(!lifecycle_write(&Method::GET, "/api/plugins/:id"));
        assert!(!lifecycle_write(&Method::POST, "/api/media"));
    }
}
