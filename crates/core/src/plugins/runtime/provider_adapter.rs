use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::rpc::{BudgetClass, FramedSession, OperationGuard, RpcError};
use crate::sync::provider::*;

#[derive(Clone)]
pub struct FramedSyncProvider {
    session: FramedSession,
    timeout: Duration,
}
impl FramedSyncProvider {
    pub fn new(session: FramedSession, timeout: Duration) -> Self {
        Self { session, timeout }
    }
    fn rpc<'a>(&'a self, method: &'static str, payload: Value) -> ProviderFuture<'a, Value> {
        Box::pin(async move {
            let started = Instant::now();
            self.session
                .call(method, payload, self.timeout, None, None)
                .await
                .map_err(|error| self.call_error(method, "provider", "control", started, error))
        })
    }
    fn transport_rpc<'a>(
        &'a self,
        context: &'a TransportContext,
        method: &'static str,
        payload: Value,
    ) -> ProviderFuture<'a, Value> {
        Box::pin(async move {
            let started = Instant::now();
            self.session
                .call(
                    method,
                    payload,
                    self.timeout,
                    Some(context.binding_epoch.clone()),
                    Some(context.transport_generation),
                )
                .await
                .map_err(|error| self.call_error(method, "library", "control", started, error))
        })
    }

    fn download_rpc<'a>(
        &'a self,
        context: &'a TransportContext,
        method: &'static str,
        payload: Value,
    ) -> ProviderFuture<'a, (Value, OperationGuard)> {
        Box::pin(async move {
            let started = Instant::now();
            self.session
                .call_owned(
                    method,
                    payload,
                    self.timeout,
                    BudgetClass::Control,
                    Some(context.binding_epoch.clone()),
                    Some(context.transport_generation),
                    true,
                )
                .await
                .map_err(|error| self.call_error(method, "library", "control", started, error))
        })
    }

    // Log only fixed call labels and typed error classes, never payloads or error detail.
    fn call_error(
        &self,
        method: &'static str,
        scope: &'static str,
        stage: &'static str,
        started: Instant,
        error: RpcError,
    ) -> ProviderError {
        let (source, kind) = match &error {
            RpcError::Closed => ("local_rpc", "closed"),
            RpcError::DeadlineExceeded => ("local_rpc", "deadline_exceeded"),
            RpcError::AdmissionDeadlineExceeded => ("local_rpc", "admission_deadline_exceeded"),
            RpcError::Protocol => ("local_rpc", "protocol_error"),
            RpcError::Remote { .. } => ("remote_provider", "remote"),
        };
        let error = map_error(error);
        tracing::warn!(
            rpc_method = method,
            rpc_scope = scope,
            rpc_stage = stage,
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            timeout_ms = u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX),
            error_source = source,
            error_kind = kind,
            error_code = ?error.code,
            retryable = error.retryable,
            "provider adapter call failed"
        );
        error
    }
}
fn map_error(e: RpcError) -> ProviderError {
    if let RpcError::Remote {
        code,
        retryable,
        retry_after_ms,
        detail,
    } = e
    {
        let code = match code.as_str() {
            "invalid_argument" => ProviderErrorCode::InvalidArgument,
            "unauthenticated" => ProviderErrorCode::Unauthenticated,
            "permission_denied" => ProviderErrorCode::PermissionDenied,
            "not_found" => ProviderErrorCode::NotFound,
            "precondition_failed" => ProviderErrorCode::PreconditionFailed,
            "throttled" => ProviderErrorCode::Throttled,
            "cursor_reset" => ProviderErrorCode::CursorReset,
            "unavailable" => ProviderErrorCode::Unavailable,
            "deadline_exceeded" => ProviderErrorCode::DeadlineExceeded,
            "cancelled" => ProviderErrorCode::Cancelled,
            "protocol_error" => ProviderErrorCode::ProtocolError,
            _ => ProviderErrorCode::Internal,
        };
        return ProviderError {
            code,
            retryable,
            retry_after_ms,
            detail,
        };
    }
    let (code, retryable) = match e {
        RpcError::DeadlineExceeded | RpcError::AdmissionDeadlineExceeded => {
            (ProviderErrorCode::DeadlineExceeded, true)
        }
        RpcError::Closed => (ProviderErrorCode::Unavailable, true),
        RpcError::Protocol => (ProviderErrorCode::ProtocolError, false),
        RpcError::Remote { .. } => unreachable!(),
    };
    ProviderError {
        code,
        retryable,
        retry_after_ms: None,
        detail: format!("{e:?}"),
    }
}
fn text(v: &Value, key: &str) -> ProviderResult<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| map_error(RpcError::Protocol))
}
fn bool_field(v: &Value, key: &str) -> ProviderResult<bool> {
    v.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| map_error(RpcError::Protocol))
}
fn metadata(v: &Value) -> ProviderResult<ObjectMetadata> {
    let hash = hex32(
        v.get("sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| map_error(RpcError::Protocol))?,
    )?;
    Ok(ObjectMetadata {
        key: text(v, "key")?,
        size: v
            .get("size")
            .and_then(Value::as_u64)
            .ok_or_else(|| map_error(RpcError::Protocol))?,
        sha256: hash,
        etag: v.get("etag").and_then(Value::as_str).map(str::to_owned),
    })
}
fn hex32(v: &str) -> ProviderResult<[u8; 32]> {
    if v.len() != 64 {
        return Err(map_error(RpcError::Protocol));
    }
    let mut out = [0; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&v[i * 2..i * 2 + 2], 16)
            .map_err(|_| map_error(RpcError::Protocol))?;
    }
    Ok(out)
}
fn hex(v: &[u8]) -> String {
    v.iter().map(|b| format!("{b:02x}")).collect()
}
fn put(v: Value) -> ProviderResult<PutImmutableResult> {
    match v.get("status").and_then(Value::as_str) {
        Some("created") => Ok(PutImmutableResult::Created {
            etag: v.get("etag").and_then(Value::as_str).map(str::to_owned),
        }),
        Some("already_exists_matching") => Ok(PutImmutableResult::AlreadyExistsMatching {
            etag: v.get("etag").and_then(Value::as_str).map(str::to_owned),
        }),
        Some("hash_conflict") => Ok(PutImmutableResult::HashConflict {
            remote_sha256: hex32(
                v.get("remoteSha256")
                    .and_then(Value::as_str)
                    .ok_or_else(|| map_error(RpcError::Protocol))?,
            )?,
        }),
        _ => Err(map_error(RpcError::Protocol)),
    }
}

impl SyncProvider for FramedSyncProvider {
    fn describe(&self) -> ProviderFuture<'_, ProviderDescription> {
        Box::pin(async move {
            let v = self.rpc("describe", Value::Null).await?;
            Ok(ProviderDescription {
                plugin_id: text(&v, "pluginId")?,
                account_uuid: text(&v, "accountUuid")?,
                capabilities: v
                    .get("capabilities")
                    .and_then(Value::as_array)
                    .ok_or_else(|| map_error(RpcError::Protocol))?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                max_object_bytes: v
                    .get("maxObjectBytes")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| map_error(RpcError::Protocol))?,
            })
        })
    }
    fn health(&self) -> ProviderFuture<'_, ProviderHealth> {
        Box::pin(async move {
            let v = self.rpc("health", Value::Null).await?;
            Ok(ProviderHealth {
                ready: bool_field(&v, "ready")?,
                detail: v.get("detail").and_then(Value::as_str).map(str::to_owned),
            })
        })
    }
    fn diagnostics(&self) -> ProviderFuture<'_, Value> {
        self.rpc("diagnostics", Value::Null)
    }
    fn auth_begin(&self, callback_url: Option<String>) -> ProviderFuture<'_, AuthBegin> {
        Box::pin(async move {
            let v = self
                .rpc("auth.begin", json!({"callbackUrl":callback_url}))
                .await?;
            Ok(AuthBegin {
                authorization_url: text(&v, "authorizationUrl")?,
                flow_id: text(&v, "flowId")?,
                completion_mode: text(&v, "completionMode")?,
                redirect_uri: v
                    .get("redirectUri")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
    }
    fn auth_poll<'a>(&'a self, flow_id: &'a str) -> ProviderFuture<'a, Option<AuthStatus>> {
        Box::pin(async move {
            let value = self.rpc("auth.poll", json!({"flowId":flow_id})).await?;
            if value.get("pending").and_then(Value::as_bool) == Some(true) {
                Ok(None)
            } else {
                auth_status(value).map(Some)
            }
        })
    }
    fn auth_cancel<'a>(&'a self, flow_id: &'a str) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            self.rpc("auth.cancel", json!({"flowId":flow_id})).await?;
            Ok(())
        })
    }
    fn auth_complete<'a>(
        &'a self,
        flow_id: &'a str,
        fields: BTreeMap<String, String>,
    ) -> ProviderFuture<'a, AuthStatus> {
        Box::pin(async move {
            auth_status(
                self.rpc("auth.complete", json!({"flowId":flow_id,"fields":fields}))
                    .await?,
            )
        })
    }
    fn auth_status(&self) -> ProviderFuture<'_, AuthStatus> {
        Box::pin(async move { auth_status(self.rpc("auth.status", Value::Null).await?) })
    }
    fn auth_sign_out(&self) -> ProviderFuture<'_, ()> {
        Box::pin(async move {
            self.rpc("auth.sign_out", Value::Null).await?;
            Ok(())
        })
    }
    fn delta_list<'a>(
        &'a self,
        scope: DeltaScope,
        cursor: Option<&'a str>,
        page_token: Option<&'a str>,
        limit: usize,
    ) -> ProviderFuture<'a, DeltaPage> {
        Box::pin(async move {
            let scope_kind = match &scope {
                DeltaScope::Root => "root",
                DeltaScope::Library { .. } => "library",
            };
            let scope = match scope {
                DeltaScope::Root => (json!({"kind":"root"}), None, None),
                DeltaScope::Library {
                    namespace,
                    binding_epoch,
                    transport_generation,
                } => (
                    json!({"kind":"library","namespace":namespace,"bindingEpoch":binding_epoch,"transportGeneration":transport_generation}),
                    Some(binding_epoch),
                    Some(transport_generation),
                ),
            };
            let started = Instant::now();
            let v = self
                .session
                .call(
                    "delta.list",
                    json!({"scope":scope.0,"cursor":cursor,"pageToken":page_token,"limit":limit}),
                    self.timeout,
                    scope.1,
                    scope.2,
                )
                .await
                .map_err(|error| {
                    self.call_error("delta.list", scope_kind, "control", started, error)
                })?;
            let objects = v
                .get("objects")
                .and_then(Value::as_array)
                .ok_or_else(|| map_error(RpcError::Protocol))?
                .iter()
                .map(metadata)
                .collect::<ProviderResult<Vec<_>>>()?;
            Ok(DeltaPage {
                objects,
                page_token: v
                    .get("pageToken")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                completed_cursor: v
                    .get("completedCursor")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
    }
    fn object_head<'a>(
        &'a self,
        context: &'a TransportContext,
        key: &'a str,
    ) -> ProviderFuture<'a, Option<ObjectMetadata>> {
        Box::pin(async move {
            let v = self
                .transport_rpc(
                    context,
                    "object.head",
                    json!({"namespace":context.namespace,"key":key}),
                )
                .await?;
            if v.is_null() {
                Ok(None)
            } else {
                metadata(&v).map(Some)
            }
        })
    }
    fn object_get<'a>(
        &'a self,
        context: &'a TransportContext,
        key: &'a str,
        max_bytes: u64,
    ) -> ProviderFuture<'a, DownloadedObject> {
        Box::pin(async move {
            let (v, mut operation) = self
                .download_rpc(
                    context,
                    "object.get",
                    json!({"namespace":context.namespace,"key":key,"maxBytes":max_bytes}),
                )
                .await?;
            let meta = metadata(
                v.get("metadata")
                    .ok_or_else(|| map_error(RpcError::Protocol))?,
            )?;
            let stream_id = v
                .get("streamId")
                .and_then(Value::as_u64)
                .ok_or_else(|| map_error(RpcError::Protocol))?;
            let started = Instant::now();
            let path = self
                .session
                .receive_file_for_context(
                    stream_id,
                    operation.deadline(),
                    &context.binding_epoch,
                    context.transport_generation,
                )
                .await
                .map_err(|error| {
                    self.call_error("object.get", "library", "receive", started, error)
                })?;
            let object = DownloadedObject {
                metadata: meta,
                path,
            };
            validate_download(&object, max_bytes)?;
            operation.complete().map_err(map_error)?;
            Ok(object)
        })
    }
    fn object_get_range<'a>(
        &'a self,
        context: &'a TransportContext,
        key: &'a str,
        start: u64,
        end: u64,
        max_bytes: u64,
    ) -> ProviderFuture<'a, DownloadedRange> {
        Box::pin(async move {
            if end <= start || end - start > max_bytes {
                return Err(ProviderError {
                    code: ProviderErrorCode::InvalidArgument,
                    retryable: false,
                    retry_after_ms: None,
                    detail: "invalid object range".into(),
                });
            }
            let (v, mut operation) = self
                .download_rpc(
                    context,
                    "object.get_range",
                    json!({"namespace":context.namespace,"key":key,"start":start,"end":end,"maxBytes":max_bytes}),
                )
                .await?;
            let meta = metadata(
                v.get("metadata")
                    .ok_or_else(|| map_error(RpcError::Protocol))?,
            )?;
            let stream_id = v
                .get("streamId")
                .and_then(Value::as_u64)
                .ok_or_else(|| map_error(RpcError::Protocol))?;
            let response_start = v
                .get("responseStart")
                .and_then(Value::as_u64)
                .ok_or_else(|| map_error(RpcError::Protocol))?;
            let response_end = v
                .get("responseEnd")
                .and_then(Value::as_u64)
                .ok_or_else(|| map_error(RpcError::Protocol))?;
            let whole_object = v
                .get("wholeObject")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let started = Instant::now();
            let path = self
                .session
                .receive_file_for_context(
                    stream_id,
                    operation.deadline(),
                    &context.binding_epoch,
                    context.transport_generation,
                )
                .await
                .map_err(|error| {
                    self.call_error("object.get_range", "library", "receive", started, error)
                })?;
            let actual = std::fs::metadata(&path)
                .map_err(|error| ProviderError {
                    code: ProviderErrorCode::Internal,
                    retryable: true,
                    retry_after_ms: None,
                    detail: error.to_string(),
                })?
                .len();
            if response_end <= response_start
                || response_end - response_start != actual
                || actual > max_bytes
                || (!whole_object && (response_start != start || response_end != end))
                || (whole_object && (response_start != 0 || response_end != meta.size))
            {
                let _ = std::fs::remove_file(&path);
                return Err(map_error(RpcError::Protocol));
            }
            if whole_object {
                if let Err(error) = validate_download(
                    &DownloadedObject {
                        metadata: meta.clone(),
                        path: path.clone(),
                    },
                    max_bytes,
                ) {
                    let _ = std::fs::remove_file(&path);
                    return Err(error);
                }
            }
            let range = DownloadedRange {
                metadata: meta,
                requested_start: start,
                requested_end: end,
                response_start,
                response_end,
                whole_object,
                path,
            };
            operation.complete().map_err(map_error)?;
            Ok(range)
        })
    }
    fn object_put_immutable<'a>(
        &'a self,
        context: &'a TransportContext,
        key: &'a str,
        bytes: &'a [u8],
        sha256: [u8; 32],
    ) -> ProviderFuture<'a, PutImmutableResult> {
        Box::pin(async move {
            let started = Instant::now();
            put(self
                .session
                .call_with_bytes(
                    "object.put_immutable",
                    json!({"namespace":context.namespace,"key":key,"sha256":hex(&sha256)}),
                    bytes,
                    self.timeout,
                    Some(context.binding_epoch.clone()),
                    Some(context.transport_generation),
                )
                .await
                .map_err(|error| {
                    self.call_error("object.put_immutable", "library", "control", started, error)
                })?)
        })
    }
    fn upload_begin<'a>(
        &'a self,
        context: &'a TransportContext,
        key: &'a str,
        size: u64,
        sha256: [u8; 32],
    ) -> ProviderFuture<'a, UploadSession> {
        Box::pin(async move {
            let v = self
                .transport_rpc(
                    context,
                    "upload.begin",
                    json!({"namespace":context.namespace,"key":key,"size":size,"sha256":hex(&sha256)}),
                )
                .await?;
            Ok(UploadSession {
                upload_id: text(&v, "uploadId")?,
                committed_bytes: v
                    .get("committedBytes")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| map_error(RpcError::Protocol))?,
            })
        })
    }
    fn upload_write<'a>(
        &'a self,
        context: &'a TransportContext,
        id: &'a str,
        offset: u64,
        bytes: &'a [u8],
    ) -> ProviderFuture<'a, u64> {
        Box::pin(async move {
            let started = Instant::now();
            let v = self
                .session
                .call_with_bytes(
                    "upload.write",
                    json!({"namespace":context.namespace,"uploadId":id,"offset":offset}),
                    bytes,
                    self.timeout,
                    Some(context.binding_epoch.clone()),
                    Some(context.transport_generation),
                )
                .await
                .map_err(|error| {
                    self.call_error("upload.write", "library", "control", started, error)
                })?;
            v.get("committedBytes")
                .and_then(Value::as_u64)
                .ok_or_else(|| map_error(RpcError::Protocol))
        })
    }
    fn upload_commit<'a>(
        &'a self,
        context: &'a TransportContext,
        id: &'a str,
    ) -> ProviderFuture<'a, PutImmutableResult> {
        Box::pin(async move {
            put(self
                .transport_rpc(
                    context,
                    "upload.commit",
                    json!({"namespace":context.namespace,"uploadId":id}),
                )
                .await?)
        })
    }
    fn upload_abort<'a>(
        &'a self,
        context: &'a TransportContext,
        id: &'a str,
    ) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            self.transport_rpc(
                context,
                "upload.abort",
                json!({"namespace":context.namespace,"uploadId":id}),
            )
            .await?;
            Ok(())
        })
    }
    fn fence_transport<'a>(
        &'a self,
        binding_epoch: &'a str,
        before_generation: u64,
    ) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            self.session
                .cancel_binding(binding_epoch, before_generation)
                .await
                .map_err(map_error)
        })
    }

    fn close_admission(&self) {
        self.session.close_admission();
    }
    fn join_closed(&self) -> ProviderFuture<'_, ()> {
        Box::pin(async {
            self.session.join_closed().await;
            Ok(())
        })
    }
    fn shutdown(&self) -> ProviderFuture<'_, ()> {
        Box::pin(async move {
            self.rpc("shutdown", Value::Null).await?;
            Ok(())
        })
    }
}
fn auth_status(v: Value) -> ProviderResult<AuthStatus> {
    Ok(AuthStatus {
        authenticated: bool_field(&v, "authenticated")?,
        account_display_name: v
            .get("accountDisplayName")
            .and_then(Value::as_str)
            .map(str::to_owned),
        stable_account_identity: v.get("stableAccountIdentity").map(canonical_json),
        account_kind: v
            .get("accountKind")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn canonical_json(value: &Value) -> String {
    fn normalize(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut entries = object.iter().collect::<Vec<_>>();
                entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key.clone(), normalize(value)))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(values.iter().map(normalize).collect()),
            _ => value.clone(),
        }
    }

    normalize(value).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::frame::{encode, Frame, FrameKind, FrameLimits};
    use crate::plugins::runtime::rpc::{ControlEnvelope, ControlType};
    use crate::plugins::runtime::stream::encode_end;
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tracing::instrument::WithSubscriber;

    #[derive(Clone, Default)]
    struct DiagnosticLog(Arc<Mutex<Vec<u8>>>);

    impl Write for DiagnosticLog {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl DiagnosticLog {
        fn subscriber(&self) -> tracing::Dispatch {
            let output = self.clone();
            tracing::Dispatch::new(
                tracing_subscriber::fmt()
                    .json()
                    .without_time()
                    .with_writer(move || output.clone())
                    .finish(),
            )
        }

        fn records(&self) -> Vec<Value> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }

    fn diagnostic_provider() -> (FramedSyncProvider, tokio::io::DuplexStream) {
        let (host, child) = tokio::io::duplex(8192);
        let (reader, writer) = tokio::io::split(host);
        (
            FramedSyncProvider::new(
                FramedSession::start(reader, writer, FrameLimits::default(), 1),
                Duration::from_millis(100),
            ),
            child,
        )
    }

    async fn read_frame(child: &mut tokio::io::DuplexStream) -> Frame {
        let mut header = [0; 17];
        child.read_exact(&mut header).await.unwrap();
        let size = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        let mut payload = vec![0; size];
        child.read_exact(&mut payload).await.unwrap();
        Frame {
            kind: FrameKind::try_from(header[4]).unwrap(),
            stream_id: u64::from_le_bytes(header[5..13].try_into().unwrap()),
            sequence: u32::from_le_bytes(header[13..17].try_into().unwrap()),
            payload,
        }
    }

    async fn write_frame(child: &mut tokio::io::DuplexStream, frame: Frame) {
        child.write_all(&encode(&frame).unwrap()).await.unwrap();
    }

    async fn begin_download(child: &mut tokio::io::DuplexStream) {
        let request = read_frame(child).await;
        let mut response: ControlEnvelope = serde_json::from_slice(&request.payload).unwrap();
        assert!(matches!(
            response.method.as_str(),
            "object.get" | "object.get_range"
        ));
        response.message_type = ControlType::Response;
        response.payload = json!({
            "metadata": {"key":"object", "size":4, "sha256":hex(&Sha256::digest(b"data"))},
            "streamId":2, "responseStart":0, "responseEnd":4, "wholeObject":false
        });
        write_frame(
            child,
            Frame {
                kind: FrameKind::Control,
                stream_id: 0,
                sequence: 0,
                payload: serde_json::to_vec(&response).unwrap(),
            },
        )
        .await;
        response.message_type = ControlType::Request;
        response.request_id = uuid::Uuid::now_v7();
        response.method = "stream.open".into();
        response.binding_epoch = None;
        response.transport_generation = None;
        response.payload = json!({"streamId":2, "size":4, "sha256":hex(&Sha256::digest(b"data"))});
        write_frame(
            child,
            Frame {
                kind: FrameKind::Control,
                stream_id: 0,
                sequence: 0,
                payload: serde_json::to_vec(&response).unwrap(),
            },
        )
        .await;
        assert_eq!(read_frame(child).await.kind, FrameKind::Credit);
    }

    #[tokio::test]
    async fn download_keeps_admission_until_stream_completion_and_reuses_session() {
        for range in [false, true] {
            let (mut provider, mut child) = diagnostic_provider();
            provider.timeout = Duration::from_secs(2);
            let download_provider = provider.clone();
            let download = tokio::spawn(async move {
                let context = TransportContext {
                    namespace: "library".into(),
                    binding_epoch: "binding".into(),
                    transport_generation: 1,
                };
                if range {
                    download_provider
                        .object_get_range(&context, "object", 0, 4, 4)
                        .await
                        .map(|object| std::fs::read(&object.path).unwrap())
                } else {
                    download_provider
                        .object_get(&context, "object", 4)
                        .await
                        .map(|object| std::fs::read(&object.path).unwrap())
                }
            });
            begin_download(&mut child).await;
            let queued =
                provider
                    .session
                    .call("health", Value::Null, Duration::from_secs(1), None, None);
            tokio::pin!(queued);
            tokio::select! {
                result = &mut queued => panic!("queued call completed before stream: {result:?}"),
                frame = read_frame(&mut child) => panic!("queued call dispatched before stream: {frame:?}"),
                _ = tokio::time::sleep(Duration::from_millis(20)) => {},
            }
            write_frame(
                &mut child,
                Frame {
                    kind: FrameKind::Binary,
                    stream_id: 2,
                    sequence: 1,
                    payload: b"data".to_vec(),
                },
            )
            .await;
            write_frame(
                &mut child,
                Frame {
                    kind: FrameKind::End,
                    stream_id: 2,
                    sequence: 2,
                    payload: encode_end(4, Sha256::digest(b"data").into()),
                },
            )
            .await;
            assert_eq!(download.await.unwrap().unwrap(), b"data");
            let respond = async {
                let frame = read_frame(&mut child).await;
                let mut response: ControlEnvelope = serde_json::from_slice(&frame.payload).unwrap();
                assert_eq!(response.method, "health");
                response.message_type = ControlType::Response;
                response.payload = json!({"ok":true});
                write_frame(
                    &mut child,
                    Frame {
                        kind: FrameKind::Control,
                        stream_id: 0,
                        sequence: 0,
                        payload: serde_json::to_vec(&response).unwrap(),
                    },
                )
                .await;
            };
            let (result, ()) = tokio::join!(&mut queued, respond);
            assert_eq!(result.unwrap(), json!({"ok":true}));
            assert!(!provider.session.activity().is_retired());
            provider.session.close_admission();
            provider.session.join_closed().await;
        }
    }

    #[tokio::test]
    async fn download_cancellation_after_control_response_retires_session() {
        for fence in [false, true] {
            let (mut provider, mut child) = diagnostic_provider();
            provider.timeout = Duration::from_secs(2);
            let download_provider = provider.clone();
            let download = tokio::spawn(async move {
                let context = TransportContext {
                    namespace: "library".into(),
                    binding_epoch: "binding".into(),
                    transport_generation: 1,
                };
                download_provider.object_get(&context, "object", 4).await
            });
            begin_download(&mut child).await;
            if fence {
                provider.session.cancel_binding("binding", 2).await.unwrap();
                assert_eq!(
                    download.await.unwrap().unwrap_err().code,
                    ProviderErrorCode::Unavailable
                );
            } else {
                download.abort();
                assert!(download.await.unwrap_err().is_cancelled());
            }
            assert!(provider.session.activity().is_retired());
            tokio::time::timeout(Duration::from_secs(1), provider.session.join_closed())
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn download_receive_shares_the_control_absolute_deadline() {
        for range in [false, true] {
            let (mut provider, mut child) = diagnostic_provider();
            provider.timeout = Duration::from_millis(400);
            let context = TransportContext {
                namespace: "library".into(),
                binding_epoch: "binding".into(),
                transport_generation: 1,
            };
            let download = async {
                if range {
                    provider
                        .object_get_range(&context, "object", 0, 4, 4)
                        .await
                        .map(|_| ())
                } else {
                    provider.object_get(&context, "object", 4).await.map(|_| ())
                }
            };
            let delayed_response = async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                begin_download(&mut child).await;
            };
            let (result, ()) = tokio::time::timeout(Duration::from_millis(600), async {
                tokio::join!(download, delayed_response)
            })
            .await
            .expect("receive must use the control operation's original deadline");
            assert_eq!(
                result.unwrap_err().code,
                ProviderErrorCode::DeadlineExceeded
            );
            assert!(provider.session.activity().is_retired());
            provider.session.join_closed().await;
        }
    }

    #[tokio::test]
    async fn rejected_download_releases_admission_without_retiring_session() {
        let (provider, mut child) = diagnostic_provider();
        let context = TransportContext {
            namespace: "library".into(),
            binding_epoch: "binding".into(),
            transport_generation: 1,
        };
        let reply = async {
            let request = read_frame(&mut child).await;
            let mut response: ControlEnvelope = serde_json::from_slice(&request.payload).unwrap();
            response.message_type = ControlType::Error;
            response.payload = json!({"code":"not_found", "retryable":false});
            write_frame(
                &mut child,
                Frame {
                    kind: FrameKind::Control,
                    stream_id: 0,
                    sequence: 0,
                    payload: serde_json::to_vec(&response).unwrap(),
                },
            )
            .await;
        };
        let (result, ()) = tokio::join!(provider.object_get(&context, "object", 4), reply);
        assert_eq!(result.unwrap_err().code, ProviderErrorCode::NotFound);
        assert!(!provider.session.activity().is_retired());
        let reply = async {
            let request = read_frame(&mut child).await;
            let mut response: ControlEnvelope = serde_json::from_slice(&request.payload).unwrap();
            assert_eq!(response.method, "health");
            response.message_type = ControlType::Response;
            write_frame(
                &mut child,
                Frame {
                    kind: FrameKind::Control,
                    stream_id: 0,
                    sequence: 0,
                    payload: serde_json::to_vec(&response).unwrap(),
                },
            )
            .await;
        };
        let (result, ()) = tokio::join!(
            provider
                .session
                .call("health", Value::Null, Duration::from_secs(1), None, None),
            reply
        );
        result.unwrap();
        provider.session.close_admission();
        provider.session.join_closed().await;
    }

    #[tokio::test]
    async fn failed_control_calls_report_method_scope_and_local_deadline() {
        let log = DiagnosticLog::default();
        async {
            let context = TransportContext {
                namespace: "private-namespace".into(),
                binding_epoch: "private-binding".into(),
                transport_generation: 1,
            };
            for case in 0..6 {
                // A dispatched timeout retires its session; each diagnostic case
                // needs a fresh child to exercise its own deadline.
                let (provider, _child) = diagnostic_provider();
                let error = match case {
                    0 => provider.health().await.unwrap_err(),
                    1 => provider
                        .delta_list(DeltaScope::Root, None, None, 1)
                        .await
                        .unwrap_err(),
                    2 => provider
                        .delta_list(
                            DeltaScope::Library {
                                namespace: context.namespace.clone(),
                                binding_epoch: context.binding_epoch.clone(),
                                transport_generation: 1,
                            },
                            None,
                            None,
                            1,
                        )
                        .await
                        .unwrap_err(),
                    3 => provider
                        .object_head(&context, "private-key")
                        .await
                        .unwrap_err(),
                    4 => provider
                        .object_put_immutable(&context, "private-key", b"private-body", [0; 32])
                        .await
                        .unwrap_err(),
                    _ => provider
                        .upload_write(&context, "private-upload", 0, b"private-body")
                        .await
                        .unwrap_err(),
                };
                assert_eq!(error.code, ProviderErrorCode::DeadlineExceeded);
                assert!(error.retryable);
                assert_eq!(error.detail, "DeadlineExceeded");
            }
        }
        .with_subscriber(log.subscriber())
        .await;
        let records = log.records();
        assert_eq!(records.len(), 6);
        for (record, (method, scope)) in records.iter().zip([
            ("health", "provider"),
            ("delta.list", "root"),
            ("delta.list", "library"),
            ("object.head", "library"),
            ("object.put_immutable", "library"),
            ("upload.write", "library"),
        ]) {
            let fields = &record["fields"];
            assert_eq!(fields["rpc_method"], method);
            assert_eq!(fields["rpc_scope"], scope);
            assert_eq!(fields["rpc_stage"], "control");
            assert_eq!(fields["error_source"], "local_rpc");
            assert_eq!(fields["error_kind"], "deadline_exceeded");
            assert_eq!(fields["timeout_ms"], 100);
            assert!(fields["elapsed_ms"].as_u64().unwrap() >= 100);
            assert!(!record.to_string().contains("private-"));
        }
    }

    #[tokio::test]
    async fn failed_call_diagnostics_preserve_error_mapping_and_omit_remote_text() {
        let log = DiagnosticLog::default();
        let (provider, _child) = diagnostic_provider();
        tracing::dispatcher::with_default(&log.subscriber(), || {
            for error in [
                RpcError::Closed,
                RpcError::Protocol,
                RpcError::AdmissionDeadlineExceeded,
                RpcError::Remote {
                    code: "deadline_exceeded".into(),
                    retryable: false,
                    retry_after_ms: Some(123),
                    detail: "private-remote-detail".into(),
                },
                RpcError::Remote {
                    code: "private-unknown-code".into(),
                    retryable: true,
                    retry_after_ms: None,
                    detail: "private-remote-detail".into(),
                },
            ] {
                let expected = map_error(error.clone());
                let actual =
                    provider.call_error("health", "provider", "control", Instant::now(), error);
                assert_eq!(actual.code, expected.code);
                assert_eq!(actual.retryable, expected.retryable);
                assert_eq!(actual.retry_after_ms, expected.retry_after_ms);
                assert_eq!(actual.detail, expected.detail);
            }
        });
        let records = log.records();
        assert_eq!(records.len(), 5);
        for (record, (source, kind, code)) in records.iter().zip([
            ("local_rpc", "closed", "Unavailable"),
            ("local_rpc", "protocol_error", "ProtocolError"),
            (
                "local_rpc",
                "admission_deadline_exceeded",
                "DeadlineExceeded",
            ),
            ("remote_provider", "remote", "DeadlineExceeded"),
            ("remote_provider", "remote", "Internal"),
        ]) {
            assert_eq!(record["fields"]["error_source"], source);
            assert_eq!(record["fields"]["error_kind"], kind);
            assert_eq!(record["fields"]["error_code"], code);
            assert!(!record.to_string().contains("private-"));
        }
    }

    #[tokio::test]
    async fn download_receive_deadline_is_distinct_from_control_deadline() {
        let log = DiagnosticLog::default();
        async {
            for range in [false, true] {
                let (provider, mut child) = diagnostic_provider();
                let context = TransportContext {
                    namespace: "private-namespace".into(),
                    binding_epoch: "private-binding".into(),
                    transport_generation: 1,
                };
                let download = async {
                    if range {
                        provider
                            .object_get_range(&context, "private-key", 0, 1, 1)
                            .await
                            .map(|_| ())
                    } else {
                        provider
                            .object_get(&context, "private-key", 1)
                            .await
                            .map(|_| ())
                    }
                };
                let response_without_stream = async {
                    let mut header = [0; 17];
                    child.read_exact(&mut header).await.unwrap();
                    let size = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
                    let mut payload = vec![0; size];
                    child.read_exact(&mut payload).await.unwrap();
                    let mut response: ControlEnvelope = serde_json::from_slice(&payload).unwrap();
                    response.message_type = ControlType::Response;
                    response.payload = json!({
                        "metadata": {"key":"private-key", "size":1, "sha256":hex(&[0;32])},
                        "streamId":2, "responseStart":0, "responseEnd":1, "wholeObject":false
                    });
                    child
                        .write_all(
                            &encode(&Frame {
                                kind: FrameKind::Control,
                                stream_id: 0,
                                sequence: 0,
                                payload: serde_json::to_vec(&response).unwrap(),
                            })
                            .unwrap(),
                        )
                        .await
                        .unwrap();
                    child
                };
                // Retain the peer after its control reply so only the receive timer can fail.
                let (result, _child) = tokio::join!(download, response_without_stream);
                assert_eq!(
                    result.unwrap_err().code,
                    ProviderErrorCode::DeadlineExceeded
                );
                assert!(provider.session.activity().is_retired());
                tokio::time::timeout(Duration::from_secs(1), provider.session.join_closed())
                    .await
                    .unwrap();
            }
        }
        .with_subscriber(log.subscriber())
        .await;
        let records = log.records();
        assert_eq!(records.len(), 2);
        for (record, method) in records.iter().zip(["object.get", "object.get_range"]) {
            assert_eq!(record["fields"]["rpc_method"], method);
            assert_eq!(record["fields"]["rpc_scope"], "library");
            assert_eq!(record["fields"]["rpc_stage"], "receive");
            assert_eq!(record["fields"]["error_source"], "local_rpc");
            assert_eq!(record["fields"]["error_kind"], "deadline_exceeded");
            assert!(!record.to_string().contains("private-"));
        }
    }

    #[test]
    fn stable_account_identity_is_canonical_across_object_key_order() {
        let left = auth_status(json!({
            "authenticated": true,
            "stableAccountIdentity": {"driveId":"drive","accountKind":"business","objectHint":"object"}
        }))
        .unwrap();
        let right = auth_status(json!({
            "authenticated": true,
            "stableAccountIdentity": {"objectHint":"object","accountKind":"business","driveId":"drive"}
        }))
        .unwrap();
        assert_eq!(left.stable_account_identity, right.stable_account_identity);
    }

    #[test]
    fn whole_object_range_fallback_is_hash_verified() {
        let path = std::env::temp_dir().join(format!(
            "mangoo-provider-range-{}-{}.tmp",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"tampered").unwrap();
        file.sync_all().unwrap();
        let downloaded = DownloadedObject {
            metadata: ObjectMetadata {
                key: "media/data/object.pack".into(),
                size: 8,
                sha256: [0; 32],
                etag: None,
            },
            path: path.clone(),
        };
        assert_eq!(
            validate_download(&downloaded, 1024).unwrap_err().code,
            ProviderErrorCode::ProtocolError
        );
        let _ = std::fs::remove_file(path);
    }
}
