use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use megle_onedrive_provider::config::{AuthMode, ProviderConfig};
use megle_onedrive_provider::error::ProviderError;
use megle_onedrive_provider::graph::{hex, GraphClient, RemoteObject, UploadRecordV1};
use megle_onedrive_provider::namespace::parse_library_key;
use megle_onedrive_provider::oauth::{
    CredentialRecordV1, OAuthFlows, UploadSessionSecretV1, FLOW_TTL, MAX_FLOWS, OAUTH_SCOPES,
};
use megle_onedrive_provider::rpc::{ControlRequest, RpcTransport};
use megle_onedrive_provider::{MAX_OBJECT_BYTES, PLUGIN_ID, PLUGIN_VERSION, UPLOAD_FRAGMENT_BYTES};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const SECRET_NAMESPACE: &str = "microsoft-onedrive";
const CREDENTIAL_PURPOSE: &str = "oauth-v1";
type LoopbackResults = HashMap<String, (Instant, HashMap<String, String>)>;

struct ProviderState {
    config: Result<ProviderConfig, ProviderError>,
    graph: Option<GraphClient>,
    account_uuid: String,
    flows: OAuthFlows,
    loopback_results: Arc<Mutex<LoopbackResults>>,
    loopback_cancellations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    credential_ref: Option<String>,
    credential_generation: Option<u64>,
    credential: Option<CredentialRecordV1>,
}

impl ProviderState {
    fn new() -> Self {
        let config = ProviderConfig::from_env();
        let graph = config
            .as_ref()
            .ok()
            .and_then(|config| GraphClient::new(config).ok());
        Self {
            config,
            graph,
            account_uuid: String::new(),
            flows: OAuthFlows::default(),
            loopback_results: Arc::new(Mutex::new(HashMap::new())),
            loopback_cancellations: Arc::new(Mutex::new(HashMap::new())),
            credential_ref: None,
            credential_generation: None,
            credential: None,
        }
    }

    fn config(&self) -> Result<&ProviderConfig, ProviderError> {
        self.config.as_ref().map_err(Clone::clone)
    }
    fn graph(&self) -> Result<&GraphClient, ProviderError> {
        self.graph.as_ref().ok_or_else(|| {
            self.config
                .as_ref()
                .err()
                .cloned()
                .unwrap_or_else(|| ProviderError::configuration("Graph client is unavailable"))
        })
    }

    fn handle<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
        request: &ControlRequest,
    ) -> Result<Value, ProviderError> {
        match request.method.as_str() {
            "runtime.hello" => {
                self.account_uuid = text(&request.payload, "accountUuid")?;
                Ok(json!({
                    "nonce": request.payload.get("nonce"), "wireVersion": 1,
                    "pluginId": request.payload.get("pluginId"), "version": request.payload.get("version"),
                    "accountUuid": request.payload.get("accountUuid"), "capabilities": ["sync-provider.v1"],
                    "methodVersions": request.payload.get("methodVersions"), "limits": request.payload.get("limits")
                }))
            }
            "describe" => Ok(
                json!({"pluginId":PLUGIN_ID,"accountUuid":self.account_uuid,"capabilities":["sync-provider.v1"],"maxObjectBytes":MAX_OBJECT_BYTES}),
            ),
            "health" => Ok(match &self.config {
                Ok(_) => json!({"ready":true,"detail":null}),
                Err(error) => json!({"ready":false,"detail":error.detail}),
            }),
            "diagnostics" => Ok(json!({
                "implementation":"bundled-onedrive-provider", "version":PLUGIN_VERSION,
                "configured":self.config.is_ok(), "authenticated":self.credential.is_some(),
                "accountKind":self.credential.as_ref().map(|v| v.account_kind.as_str()),
                "driveHint":self.credential.as_ref().map(|v| anonymize(&v.drive_id))
            })),
            "auth.begin" => self.auth_begin(&request.payload),
            "auth.complete" => self.auth_complete(rpc, &request.payload),
            "auth.poll" => self.auth_poll(rpc, &request.payload),
            "auth.cancel" => self.auth_cancel(&request.payload),
            "auth.status" => self.auth_status(rpc),
            "auth.sign_out" => self.auth_sign_out(rpc),
            "delta.list" => {
                let scope = request.payload.get("scope").ok_or_else(|| {
                    ProviderError::new("invalid_argument", false, "missing delta scope")
                })?;
                let namespace = match scope.get("kind").and_then(Value::as_str) {
                    Some("root") => None,
                    Some("library") => Some(text(scope, "namespace")?),
                    _ => {
                        return Err(ProviderError::new(
                            "invalid_argument",
                            false,
                            "invalid delta scope",
                        ))
                    }
                };
                self.graph_call(rpc, |graph, token| {
                    graph.delta_list(
                        token,
                        namespace.as_deref(),
                        request.payload.get("cursor").and_then(Value::as_str),
                        request.payload.get("pageToken").and_then(Value::as_str),
                        request
                            .payload
                            .get("limit")
                            .and_then(Value::as_u64)
                            .unwrap_or(100) as usize,
                    )
                })
            }
            "object.head" => {
                let (namespace, key) = namespace_key(&request.payload)?;
                Ok(self
                    .graph_call(rpc, |graph, token| graph.head(token, &namespace, &key))?
                    .map(object_json)
                    .unwrap_or(Value::Null))
            }
            "object.get" | "object.get_range" => {
                let (namespace, key) = namespace_key(&request.payload)?;
                let max_bytes = request
                    .payload
                    .get("maxBytes")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        ProviderError::new("invalid_argument", false, "missing maxBytes")
                    })?;
                let range = if request.method == "object.get_range" {
                    Some((
                        required_u64(&request.payload, "start")?,
                        required_u64(&request.payload, "end")?,
                    ))
                } else {
                    None
                };
                let mut download = self.graph_call(rpc, |graph, token| {
                    graph.download(token, &namespace, &key, range, max_bytes)
                })?;
                let stream_id = rpc.send_plugin_reader(
                    download.file.as_file_mut(),
                    download.transfer_size,
                    download.transfer_sha256,
                )?;
                let mut value =
                    json!({"metadata":object_json(download.metadata),"streamId":stream_id});
                if range.is_some() {
                    value["responseStart"] = json!(download.response_start);
                    value["responseEnd"] = json!(download.response_end);
                    value["wholeObject"] = json!(download.whole_object);
                }
                Ok(value)
            }
            "object.put_immutable" => {
                let (namespace, key) = namespace_key(&request.payload)?;
                let bytes = rpc.receive_host_stream(required_u64(&request.payload, "streamId")?)?;
                let expected = text(&request.payload, "sha256")?;
                self.graph_call(rpc, |graph, token| {
                    graph.put_immutable(token, &namespace, &key, &bytes, &expected)
                })
            }
            "upload.begin" => {
                let (namespace, key) = namespace_key(&request.payload)?;
                let size = required_u64(&request.payload, "size")?;
                if size == 0 || size > MAX_OBJECT_BYTES {
                    return Err(ProviderError::new(
                        "invalid_argument",
                        false,
                        "invalid upload size",
                    ));
                }
                let sha256 = text(&request.payload, "sha256")?;
                let record = self.graph_call(rpc, |graph, token| {
                    graph.begin_upload(token, &namespace, &key, size, &sha256)
                })?;
                let purpose = upload_purpose(&namespace);
                let result = create_secret(rpc, &purpose, &record, true)?;
                let upload_id = text(&result, "secretRef")?;
                if let Err(error) = self.track_upload_secret(rpc, &upload_id, &purpose) {
                    let _ = delete_secret(rpc, &purpose, &upload_id, true);
                    return Err(error);
                }
                Ok(json!({"uploadId":upload_id,"committedBytes":0}))
            }
            "upload.write" => {
                let namespace = text(&request.payload, "namespace")?;
                validate_namespace(&namespace)?;
                let upload_id = text(&request.payload, "uploadId")?;
                let offset = required_u64(&request.payload, "offset")?;
                let bytes = rpc.receive_host_stream(required_u64(&request.payload, "streamId")?)?;
                if bytes.len() > UPLOAD_FRAGMENT_BYTES {
                    return Err(ProviderError::new(
                        "invalid_argument",
                        false,
                        "upload fragment exceeds 10 MiB",
                    ));
                }
                let (mut record, generation) = get_secret::<R, W, UploadRecordV1>(
                    rpc,
                    &upload_purpose(&namespace),
                    &upload_id,
                )?;
                validate_upload_record(&record, &namespace)?;
                if record.namespace != namespace {
                    return Err(ProviderError::new(
                        "permission_denied",
                        false,
                        "upload handle namespace mismatch",
                    ));
                }
                let mut committed = self.graph()?.upload_fragment(&mut record, offset, &bytes)?;
                if committed < record.size && offset + bytes.len() as u64 == record.size {
                    let remote = self.graph_call(rpc, |graph, token| {
                        graph.head(token, &namespace, &record.object_key)
                    })?;
                    if remote.as_ref().is_some_and(|object| {
                        object.size == record.size && object.sha256 == record.sha256
                    }) {
                        record.committed_bytes = record.size;
                        committed = record.size;
                    }
                }
                cas_secret(
                    rpc,
                    &upload_purpose(&namespace),
                    &upload_id,
                    generation,
                    &record,
                    true,
                )?;
                Ok(json!({"committedBytes":committed}))
            }
            "upload.commit" => {
                let namespace = text(&request.payload, "namespace")?;
                validate_namespace(&namespace)?;
                let upload_id = text(&request.payload, "uploadId")?;
                let (record, _) = get_secret::<R, W, UploadRecordV1>(
                    rpc,
                    &upload_purpose(&namespace),
                    &upload_id,
                )?;
                validate_upload_record(&record, &namespace)?;
                let remote = self
                    .graph_call(rpc, |graph, token| {
                        graph.head(token, &namespace, &record.object_key)
                    })?
                    .ok_or_else(|| {
                        ProviderError::new(
                            "precondition_failed",
                            true,
                            "uploaded object is not visible yet",
                        )
                    })?;
                let result = if remote.size == record.size && remote.sha256 == record.sha256 {
                    json!({"status":"created","etag":remote.etag})
                } else {
                    json!({"status":"hash_conflict","remoteSha256":remote.sha256})
                };
                let purpose = upload_purpose(&namespace);
                delete_secret(rpc, &purpose, &upload_id, true)?;
                self.untrack_upload_secret(rpc, &upload_id)?;
                Ok(result)
            }
            "upload.abort" => {
                let namespace = text(&request.payload, "namespace")?;
                validate_namespace(&namespace)?;
                let upload_id = text(&request.payload, "uploadId")?;
                if let Ok((record, _)) =
                    get_secret::<R, W, UploadRecordV1>(rpc, &upload_purpose(&namespace), &upload_id)
                {
                    if validate_upload_record(&record, &namespace).is_ok() {
                        let _ = self.graph()?.abort_upload(&record);
                    }
                }
                let purpose = upload_purpose(&namespace);
                delete_secret(rpc, &purpose, &upload_id, true)?;
                self.untrack_upload_secret(rpc, &upload_id)?;
                Ok(Value::Null)
            }
            "shutdown" => {
                self.cancel_auth_flows()?;
                Ok(Value::Null)
            }
            _ => Err(ProviderError::new(
                "invalid_argument",
                false,
                format!("unknown Provider method: {}", request.method),
            )),
        }
    }

    fn auth_begin(&mut self, payload: &Value) -> Result<Value, ProviderError> {
        let config = self.config()?.clone();
        {
            let mut results = self.loopback_results.lock().map_err(|_| {
                ProviderError::new("internal", false, "OAuth loopback state poisoned")
            })?;
            results.retain(|_, (received_at, _)| received_at.elapsed() <= FLOW_TTL);
            if results.len() >= MAX_FLOWS {
                return Err(ProviderError::new(
                    "unavailable",
                    true,
                    "too many pending OAuth callbacks",
                ));
            }
        }
        let callback = payload
            .get("callbackUrl")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let manual = config.auth_mode == AuthMode::ManualResponseUri;
        let (listener, callback) = if manual {
            (
                None,
                Some("https://login.microsoftonline.com/common/oauth2/nativeclient".to_owned()),
            )
        } else if callback.is_none() {
            let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| {
                ProviderError::new("unavailable", true, "failed to bind OAuth loopback")
            })?;
            let uri = format!(
                "http://127.0.0.1:{}/plugin-auth/callback",
                listener
                    .local_addr()
                    .map_err(|_| ProviderError::new(
                        "unavailable",
                        true,
                        "failed to inspect OAuth loopback"
                    ))?
                    .port()
            );
            (Some(listener), Some(uri))
        } else {
            (None, callback)
        };
        let (flow_id, authorization_url) = self.flows.begin(&config, callback.as_deref())?;
        if let Some(listener) = listener {
            let results = Arc::clone(&self.loopback_results);
            let cancellations = Arc::clone(&self.loopback_cancellations);
            let flow = flow_id.clone();
            let cancelled = Arc::new(AtomicBool::new(false));
            cancellations
                .lock()
                .map_err(|_| {
                    ProviderError::new("internal", false, "OAuth loopback state poisoned")
                })?
                .insert(flow.clone(), Arc::clone(&cancelled));
            std::thread::spawn(move || {
                receive_loopback(listener, results, flow.clone(), cancelled);
                if let Ok(mut guard) = cancellations.lock() {
                    guard.remove(&flow);
                }
            });
        }
        Ok(json!({
            "authorizationUrl": authorization_url,
            "flowId": flow_id,
            "completionMode": if manual { "manual-response-uri" } else { "loopback" },
            "redirectUri": callback
        }))
    }

    fn auth_poll<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
        payload: &Value,
    ) -> Result<Value, ProviderError> {
        let flow_id = text(payload, "flowId")?;
        let fields = self
            .loopback_results
            .lock()
            .map_err(|_| ProviderError::new("internal", false, "OAuth loopback state poisoned"))?
            .remove(&flow_id)
            .map(|(_, fields)| fields);
        let Some(fields) = fields else {
            return Ok(json!({"pending":true}));
        };
        self.finish_auth(rpc, &flow_id, fields)
    }

    fn auth_cancel(&mut self, payload: &Value) -> Result<Value, ProviderError> {
        let flow_id = text(payload, "flowId")?;
        self.flows.cancel(&flow_id);
        self.loopback_results
            .lock()
            .map_err(|_| ProviderError::new("internal", false, "OAuth loopback state poisoned"))?
            .remove(&flow_id);
        if let Some(cancelled) = self
            .loopback_cancellations
            .lock()
            .map_err(|_| ProviderError::new("internal", false, "OAuth loopback state poisoned"))?
            .remove(&flow_id)
        {
            cancelled.store(true, Ordering::Release);
        }
        Ok(Value::Null)
    }

    fn auth_complete<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
        payload: &Value,
    ) -> Result<Value, ProviderError> {
        let flow_id = text(payload, "flowId")?;
        if let Some(cancelled) = self
            .loopback_cancellations
            .lock()
            .map_err(|_| ProviderError::new("internal", false, "OAuth loopback state poisoned"))?
            .remove(&flow_id)
        {
            cancelled.store(true, Ordering::Release);
        }
        let mut fields = object_strings(payload.get("fields"));
        if fields.is_empty() {
            fields = self
                .loopback_results
                .lock()
                .map_err(|_| {
                    ProviderError::new("internal", false, "OAuth loopback state poisoned")
                })?
                .remove(&flow_id)
                .map(|(_, fields)| fields)
                .ok_or_else(|| {
                    ProviderError::new("unavailable", true, "OAuth callback has not arrived")
                })?;
        }
        self.finish_auth(rpc, &flow_id, fields)
    }

    fn finish_auth<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
        flow_id: &str,
        fields: HashMap<String, String>,
    ) -> Result<Value, ProviderError> {
        let (code, verifier, redirect_uri) = self.flows.consume(flow_id, &fields)?;
        let config = self.config()?.clone();
        let graph = self.graph()?.clone();
        let token = graph.exchange_code(&config, &code, &verifier, &redirect_uri)?;
        let access_token = text(&token, "access_token")?;
        let identity = graph.drive_identity(&access_token)?;
        graph.ensure_app_root(&access_token)?;
        let refresh_token = text(&token, "refresh_token")?;
        let record = CredentialRecordV1 {
            format_version: 1,
            refresh_token,
            access_token: Some(access_token),
            access_expires_at_ms: Some(
                now_ms().saturating_add(
                    token
                        .get("expires_in")
                        .and_then(Value::as_u64)
                        .unwrap_or(3600)
                        .saturating_mul(1000),
                ),
            ),
            drive_id: identity.drive_id,
            account_kind: identity.account_kind,
            tenant_hint: identity.tenant_hint,
            object_hint: identity.object_hint,
            display_hint: identity.display_hint,
            scopes: token
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or(OAUTH_SCOPES)
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            upload_sessions: self
                .credential
                .as_ref()
                .map(|credential| credential.upload_sessions.clone())
                .unwrap_or_default(),
        };
        // OAuth completion always creates a fresh record. Host swaps the
        // account reference atomically and retires the prior credential. This
        // makes cancellation reversible even when replacing an existing
        // account; an in-place CAS would destroy the prior credential bytes.
        let created = create_secret(rpc, CREDENTIAL_PURPOSE, &record, false)?;
        self.credential_ref = Some(text(&created, "secretRef")?);
        self.credential_generation = created.get("generation").and_then(Value::as_u64);
        self.credential = Some(record);
        self.auth_status_value()
    }

    fn auth_status<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
    ) -> Result<Value, ProviderError> {
        if self.credential.is_none() {
            let _ = self.load_credential(rpc);
        }
        self.auth_status_value()
    }

    fn auth_status_value(&self) -> Result<Value, ProviderError> {
        Ok(match &self.credential {
            Some(record) => {
                json!({"authenticated":true,"accountDisplayName":record.display_hint,"stableAccountIdentity":json!({"driveId":record.drive_id,"accountKind":record.account_kind,"tenantHint":record.tenant_hint,"objectHint":record.object_hint}),"accountKind":record.account_kind})
            }
            None => json!({"authenticated":false,"accountDisplayName":null}),
        })
    }

    fn auth_sign_out<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
    ) -> Result<Value, ProviderError> {
        if self.credential_ref.is_none() {
            let _ = self.load_credential(rpc);
        }
        self.retire_credential(rpc)?;
        self.cancel_auth_flows()?;
        Ok(Value::Null)
    }

    fn cancel_auth_flows(&mut self) -> Result<(), ProviderError> {
        self.flows.cancel_all();
        self.loopback_results
            .lock()
            .map_err(|_| ProviderError::new("internal", false, "OAuth loopback state poisoned"))?
            .clear();
        let mut cancellations = self
            .loopback_cancellations
            .lock()
            .map_err(|_| ProviderError::new("internal", false, "OAuth loopback state poisoned"))?;
        for cancelled in cancellations.values() {
            cancelled.store(true, Ordering::Release);
        }
        cancellations.clear();
        Ok(())
    }

    fn load_credential<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
    ) -> Result<(), ProviderError> {
        let current = rpc.reverse_call("host.secret.current", secret_scope(CREDENTIAL_PURPOSE))?;
        let Some(secret_ref) = current.get("secretRef").and_then(Value::as_str) else {
            return Err(ProviderError::new(
                "unauthenticated",
                false,
                "Microsoft authorization is required",
            ));
        };
        let (record, generation) =
            get_secret::<R, W, CredentialRecordV1>(rpc, CREDENTIAL_PURPOSE, secret_ref)?;
        if record.format_version != 1 {
            return Err(ProviderError::protocol(
                "unsupported credential record version",
            ));
        }
        self.credential_ref = Some(secret_ref.to_owned());
        self.credential_generation = Some(generation);
        self.credential = Some(record);
        Ok(())
    }

    fn persist_credential<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
    ) -> Result<(), ProviderError> {
        let record = self
            .credential
            .as_ref()
            .ok_or_else(|| ProviderError::protocol("credential record missing"))?;
        let secret_ref = self
            .credential_ref
            .clone()
            .ok_or_else(|| ProviderError::protocol("credential reference missing"))?;
        let generation = self
            .credential_generation
            .ok_or_else(|| ProviderError::protocol("credential generation missing"))?;
        let result = cas_secret(
            rpc,
            CREDENTIAL_PURPOSE,
            &secret_ref,
            generation,
            record,
            false,
        )?;
        self.credential_generation = result.get("generation").and_then(Value::as_u64);
        Ok(())
    }

    fn track_upload_secret<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
        secret_ref: &str,
        purpose: &str,
    ) -> Result<(), ProviderError> {
        if self.credential.is_none() {
            self.load_credential(rpc)?;
        }
        let record = self
            .credential
            .as_mut()
            .ok_or_else(|| ProviderError::protocol("credential record missing"))?;
        if !record
            .upload_sessions
            .iter()
            .any(|entry| entry.secret_ref == secret_ref)
        {
            record.upload_sessions.push(UploadSessionSecretV1 {
                secret_ref: secret_ref.to_owned(),
                purpose: purpose.to_owned(),
            });
        }
        self.persist_credential(rpc)
    }

    fn untrack_upload_secret<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
        secret_ref: &str,
    ) -> Result<(), ProviderError> {
        let Some(record) = self.credential.as_mut() else {
            return Ok(());
        };
        let previous = record.upload_sessions.len();
        record
            .upload_sessions
            .retain(|entry| entry.secret_ref != secret_ref);
        if record.upload_sessions.len() != previous {
            self.persist_credential(rpc)?;
        }
        Ok(())
    }

    fn access_token<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
    ) -> Result<String, ProviderError> {
        if self.credential.is_none() {
            self.load_credential(rpc)?;
        }
        let needs_refresh = self
            .credential
            .as_ref()
            .and_then(|v| v.access_expires_at_ms)
            .unwrap_or(0)
            <= now_ms().saturating_add(60_000);
        if needs_refresh {
            self.refresh_access_token(rpc)?;
        }
        self.credential
            .as_ref()
            .and_then(|v| v.access_token.clone())
            .ok_or_else(|| {
                ProviderError::new(
                    "unauthenticated",
                    false,
                    "Microsoft authorization is required",
                )
            })
    }

    fn refresh_access_token<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
    ) -> Result<String, ProviderError> {
        let config = self.config()?.clone();
        let graph = self.graph()?.clone();
        let mut record = self.credential.clone().ok_or_else(|| {
            ProviderError::new(
                "unauthenticated",
                false,
                "Microsoft authorization is required",
            )
        })?;
        let token = match graph.refresh_token(&config, &record.refresh_token) {
            Ok(token) => token,
            Err(error) if error.code == "unauthenticated" => {
                self.require_reauthorization(rpc)?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let access_token = text(&token, "access_token")?;
        let identity = match graph.drive_identity(&access_token) {
            Ok(identity) => identity,
            Err(error) if error.code == "unauthenticated" => {
                self.require_reauthorization(rpc)?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        if identity.drive_id != record.drive_id
            || identity.account_kind != record.account_kind
            || identity.tenant_hint != record.tenant_hint
            || identity.object_hint != record.object_hint
        {
            self.require_reauthorization(rpc)?;
            return Err(ProviderError::new(
                "unauthenticated",
                false,
                "Microsoft account identity changed; authorization is required",
            ));
        }
        record.access_token = Some(access_token);
        if let Some(refresh) = token.get("refresh_token").and_then(Value::as_str) {
            record.refresh_token = refresh.to_owned();
        }
        record.access_expires_at_ms = Some(
            now_ms().saturating_add(
                token
                    .get("expires_in")
                    .and_then(Value::as_u64)
                    .unwrap_or(3600)
                    .saturating_mul(1000),
            ),
        );
        let secret_ref = self
            .credential_ref
            .clone()
            .ok_or_else(|| ProviderError::protocol("credential reference missing"))?;
        let generation = self
            .credential_generation
            .ok_or_else(|| ProviderError::protocol("credential generation missing"))?;
        let result = cas_secret(
            rpc,
            CREDENTIAL_PURPOSE,
            &secret_ref,
            generation,
            &record,
            false,
        )?;
        self.credential_generation = result.get("generation").and_then(Value::as_u64);
        self.credential = Some(record);
        self.credential
            .as_ref()
            .and_then(|v| v.access_token.clone())
            .ok_or_else(|| {
                ProviderError::new(
                    "unauthenticated",
                    false,
                    "Microsoft authorization is required",
                )
            })
    }

    fn retire_credential<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
    ) -> Result<(), ProviderError> {
        if let Some(record) = self.credential.as_ref() {
            for upload in &record.upload_sessions {
                delete_secret(rpc, &upload.purpose, &upload.secret_ref, true)?;
            }
        }
        if let Some(secret_ref) = self.credential_ref.take() {
            delete_secret(rpc, CREDENTIAL_PURPOSE, &secret_ref, false)?;
        }
        self.credential = None;
        self.credential_generation = None;
        Ok(())
    }

    fn require_reauthorization<R: Read, W: Write>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
    ) -> Result<(), ProviderError> {
        self.retire_credential(rpc)?;
        rpc.reverse_call(
            "host.auth.authorization_required",
            secret_scope(CREDENTIAL_PURPOSE),
        )?;
        Ok(())
    }

    fn graph_call<R: Read, W: Write, T>(
        &mut self,
        rpc: &mut RpcTransport<R, W>,
        mut operation: impl FnMut(&GraphClient, &str) -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        let token = self.access_token(rpc)?;
        let graph = self.graph()?.clone();
        match operation(&graph, &token) {
            Err(error) if error.code == "unauthenticated" => {
                let refreshed = self.refresh_access_token(rpc)?;
                match operation(&graph, &refreshed) {
                    Err(error) if error.code == "unauthenticated" => {
                        self.require_reauthorization(rpc)?;
                        Err(error)
                    }
                    result => result,
                }
            }
            result => result,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(windows)]
    let _winsock = WindowsSockets::startup()?;
    let input = std::io::stdin().lock();
    let output = std::io::stdout().lock();
    let mut rpc = RpcTransport::new(input, output);
    let mut state = ProviderState::new();
    while let Some(request) = rpc.next_request()? {
        let shutdown = request.method == "shutdown";
        let result = state.handle(&mut rpc, &request);
        rpc.respond(&request, result)?;
        if shutdown {
            break;
        }
    }
    Ok(())
}

#[cfg(windows)]
struct WindowsSockets;

#[cfg(windows)]
impl WindowsSockets {
    fn startup() -> std::io::Result<Self> {
        use windows_sys::Win32::Networking::WinSock::{WSAStartup, WSADATA};
        let mut data = std::mem::MaybeUninit::<WSADATA>::zeroed();
        let result = unsafe { WSAStartup(0x0202, data.as_mut_ptr()) };
        if result == 0 {
            Ok(Self)
        } else {
            Err(std::io::Error::from_raw_os_error(result))
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsSockets {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Networking::WinSock::WSACleanup();
        }
    }
}

fn receive_loopback(
    listener: TcpListener,
    results: Arc<Mutex<LoopbackResults>>,
    flow_id: String,
    cancelled: Arc<AtomicBool>,
) {
    let _ = listener.set_nonblocking(true);
    let deadline = Instant::now() + FLOW_TTL;
    while Instant::now() < deadline && !cancelled.load(Ordering::Acquire) {
        let Ok((mut stream, _)) = listener.accept() else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        let mut buffer = [0u8; 8192];
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let valid = if let Ok(size) = stream.read(&mut buffer) {
            if size == buffer.len() {
                false
            } else {
                let line = String::from_utf8_lossy(&buffer[..size]);
                if let Some((method, target)) = line.lines().next().and_then(|line| {
                    let mut parts = line.split_whitespace();
                    Some((parts.next()?, parts.next()?))
                }) {
                    if let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) {
                        if method == "GET" && url.path() == "/plugin-auth/callback" {
                            let mut fields = HashMap::new();
                            let mut duplicate = false;
                            for (key, value) in url.query_pairs() {
                                if fields
                                    .insert(key.into_owned(), value.into_owned())
                                    .is_some()
                                {
                                    duplicate = true;
                                    break;
                                }
                            }
                            if !duplicate
                                && fields.len() <= 4
                                && fields.keys().all(|key| {
                                    matches!(
                                        key.as_str(),
                                        "state" | "code" | "error" | "error_description"
                                    )
                                })
                            {
                                if let Ok(mut guard) = results.lock() {
                                    guard.insert(flow_id.clone(), (Instant::now(), fields));
                                }
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        } else {
            false
        };
        let (status, body) = if valid {
            ("200 OK", "Authorization received. You can return to Megle.")
        } else {
            ("400 Bad Request", "Invalid authorization callback.")
        };
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
        let _ = stream.write_all(response.as_bytes());
        break;
    }
}

fn secret_scope(purpose: &str) -> Value {
    json!({"namespace":SECRET_NAMESPACE,"purpose":purpose})
}
fn upload_purpose(namespace: &str) -> String {
    format!("upload-session-v1.{namespace}")
}

fn create_secret<R: Read, W: Write, T: serde::Serialize>(
    rpc: &mut RpcTransport<R, W>,
    purpose: &str,
    value: &T,
    scoped: bool,
) -> Result<Value, ProviderError> {
    let mut payload = secret_scope(purpose);
    payload["value"] = json!(base64::engine::general_purpose::STANDARD.encode(
        serde_json::to_vec(value)
            .map_err(|_| ProviderError::protocol("secret serialization failed"))?
    ));
    rpc.reverse_call(
        if scoped {
            "host.secret.create_scoped"
        } else {
            "host.secret.create"
        },
        payload,
    )
}

fn get_secret<R: Read, W: Write, T: serde::de::DeserializeOwned>(
    rpc: &mut RpcTransport<R, W>,
    purpose: &str,
    secret_ref: &str,
) -> Result<(T, u64), ProviderError> {
    let mut payload = secret_scope(purpose);
    payload["secretRef"] = json!(secret_ref);
    let value = rpc.reverse_call("host.secret.get", payload)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(
            value
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| ProviderError::protocol("secret payload missing"))?,
        )
        .map_err(|_| ProviderError::protocol("secret payload is invalid"))?;
    let record = serde_json::from_slice(&bytes)
        .map_err(|_| ProviderError::protocol("secret record is invalid"))?;
    Ok((
        record,
        value
            .get("generation")
            .and_then(Value::as_u64)
            .ok_or_else(|| ProviderError::protocol("secret generation missing"))?,
    ))
}

fn cas_secret<R: Read, W: Write, T: serde::Serialize>(
    rpc: &mut RpcTransport<R, W>,
    purpose: &str,
    secret_ref: &str,
    generation: u64,
    value: &T,
    scoped: bool,
) -> Result<Value, ProviderError> {
    let mut payload = secret_scope(purpose);
    payload["secretRef"] = json!(secret_ref);
    payload["expectedGeneration"] = json!(generation);
    payload["value"] = json!(base64::engine::general_purpose::STANDARD.encode(
        serde_json::to_vec(value)
            .map_err(|_| ProviderError::protocol("secret serialization failed"))?
    ));
    rpc.reverse_call(
        if scoped {
            "host.secret.cas_scoped"
        } else {
            "host.secret.cas"
        },
        payload,
    )
}

fn delete_secret<R: Read, W: Write>(
    rpc: &mut RpcTransport<R, W>,
    purpose: &str,
    secret_ref: &str,
    scoped: bool,
) -> Result<Value, ProviderError> {
    let mut payload = secret_scope(purpose);
    payload["secretRef"] = json!(secret_ref);
    rpc.reverse_call(
        if scoped {
            "host.secret.delete_scoped"
        } else {
            "host.secret.delete"
        },
        payload,
    )
}

fn namespace_key(payload: &Value) -> Result<(String, String), ProviderError> {
    let namespace = text(payload, "namespace")?;
    let key = text(payload, "key")?;
    parse_library_key(&namespace, &key)?;
    Ok((namespace, key))
}

fn validate_namespace(namespace: &str) -> Result<(), ProviderError> {
    parse_library_key(namespace, "probe").map(|_| ())
}
fn validate_upload_record(
    record: &UploadRecordV1,
    expected_namespace: &str,
) -> Result<(), ProviderError> {
    if record.format_version != 1
        || record.namespace != expected_namespace
        || record.size == 0
        || record.size > MAX_OBJECT_BYTES
        || record.committed_bytes > record.size
        || record.sha256.len() != 64
        || !record.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || parse_library_key(&record.namespace, &record.object_key).is_err()
    {
        return Err(ProviderError::protocol("invalid upload-session record"));
    }
    Ok(())
}
fn text(value: &Value, key: &str) -> Result<String, ProviderError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::new("invalid_argument", false, format!("missing {key}")))
}
fn required_u64(value: &Value, key: &str) -> Result<u64, ProviderError> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| ProviderError::new("invalid_argument", false, format!("missing {key}")))
}
fn object_strings(value: Option<&Value>) -> HashMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}
fn object_json(value: RemoteObject) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn anonymize(value: &str) -> String {
    hex(&Sha256::digest(value.as_bytes()))[..16].to_owned()
}
