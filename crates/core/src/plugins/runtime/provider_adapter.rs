use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use super::rpc::{FramedSession, RpcError};
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
    fn rpc<'a>(&'a self, method: &'a str, payload: Value) -> ProviderFuture<'a, Value> {
        Box::pin(async move {
            self.session
                .call(method, payload, self.timeout, None, None)
                .await
                .map_err(map_error)
        })
    }
    fn transport_rpc<'a>(
        &'a self,
        context: &'a TransportContext,
        method: &'a str,
        payload: Value,
    ) -> ProviderFuture<'a, Value> {
        Box::pin(async move {
            self.session
                .call(
                    method,
                    payload,
                    self.timeout,
                    Some(context.binding_epoch.clone()),
                    Some(context.transport_generation),
                )
                .await
                .map_err(map_error)
        })
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
        RpcError::DeadlineExceeded => (ProviderErrorCode::DeadlineExceeded, true),
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
            })
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
                .map_err(map_error)?;
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
            let v = self
                .transport_rpc(
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
            let path = self
                .session
                .receive_file(stream_id, self.timeout)
                .await
                .map_err(map_error)?;
            let object = DownloadedObject {
                metadata: meta,
                path,
            };
            validate_download(&object, max_bytes)?;
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
            let v = self
                .transport_rpc(
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
            let path = self
                .session
                .receive_file(stream_id, self.timeout)
                .await
                .map_err(map_error)?;
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
            Ok(DownloadedRange {
                metadata: meta,
                requested_start: start,
                requested_end: end,
                response_start,
                response_end,
                whole_object,
                path,
            })
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
                .map_err(map_error)?)
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
                .map_err(map_error)?;
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
}
