use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    io::Read,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
};

pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = ProviderResult<T>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderErrorCode {
    InvalidArgument,
    Unauthenticated,
    PermissionDenied,
    NotFound,
    PreconditionFailed,
    Throttled,
    CursorReset,
    Unavailable,
    DeadlineExceeded,
    Cancelled,
    ProtocolError,
    Internal,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub code: ProviderErrorCode,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
    pub detail: String,
}
impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.detail)
    }
}
impl std::error::Error for ProviderError {}
pub type ProviderResult<T> = Result<T, ProviderError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDescription {
    pub plugin_id: String,
    pub account_uuid: String,
    pub capabilities: Vec<String>,
    pub max_object_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealth {
    pub ready: bool,
    pub detail: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthBegin {
    pub authorization_url: String,
    pub flow_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    pub authenticated: bool,
    pub account_display_name: Option<String>,
    pub stable_account_identity: Option<String>,
    pub account_kind: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaScope {
    Root,
    Library {
        namespace: String,
        binding_epoch: String,
        transport_generation: u64,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMetadata {
    pub key: String,
    pub size: u64,
    pub sha256: [u8; 32],
    pub etag: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaPage {
    pub objects: Vec<ObjectMetadata>,
    pub page_token: Option<String>,
    pub completed_cursor: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportContext {
    pub namespace: String,
    pub binding_epoch: String,
    pub transport_generation: u64,
}
#[derive(Debug)]
pub struct DownloadedObject {
    pub metadata: ObjectMetadata,
    pub path: PathBuf,
}
#[derive(Debug)]
pub struct DownloadedRange {
    pub metadata: ObjectMetadata,
    pub requested_start: u64,
    pub requested_end: u64,
    pub response_start: u64,
    pub response_end: u64,
    pub whole_object: bool,
    pub path: PathBuf,
}
impl Drop for DownloadedRange {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
impl Drop for DownloadedObject {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutImmutableResult {
    Created { etag: Option<String> },
    AlreadyExistsMatching { etag: Option<String> },
    HashConflict { remote_sha256: [u8; 32] },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadSession {
    pub upload_id: String,
    pub committed_bytes: u64,
}

pub trait SyncProvider: Send + Sync {
    fn describe(&self) -> ProviderFuture<'_, ProviderDescription>;
    fn health(&self) -> ProviderFuture<'_, ProviderHealth>;
    fn diagnostics(&self) -> ProviderFuture<'_, serde_json::Value>;
    fn auth_begin(&self, callback_url: Option<String>) -> ProviderFuture<'_, AuthBegin>;
    fn auth_complete<'a>(
        &'a self,
        flow_id: &'a str,
        fields: BTreeMap<String, String>,
    ) -> ProviderFuture<'a, AuthStatus>;
    fn auth_status(&self) -> ProviderFuture<'_, AuthStatus>;
    fn auth_sign_out(&self) -> ProviderFuture<'_, ()>;
    fn delta_list<'a>(
        &'a self,
        scope: DeltaScope,
        cursor: Option<&'a str>,
        page_token: Option<&'a str>,
        limit: usize,
    ) -> ProviderFuture<'a, DeltaPage>;
    fn object_head<'a>(
        &'a self,
        context: &'a TransportContext,
        key: &'a str,
    ) -> ProviderFuture<'a, Option<ObjectMetadata>>;
    fn object_get<'a>(
        &'a self,
        context: &'a TransportContext,
        key: &'a str,
        max_bytes: u64,
    ) -> ProviderFuture<'a, DownloadedObject>;
    fn object_get_range<'a>(
        &'a self,
        context: &'a TransportContext,
        key: &'a str,
        start: u64,
        end: u64,
        max_bytes: u64,
    ) -> ProviderFuture<'a, DownloadedRange>;
    fn object_put_immutable<'a>(
        &'a self,
        context: &'a TransportContext,
        key: &'a str,
        bytes: &'a [u8],
        sha256: [u8; 32],
    ) -> ProviderFuture<'a, PutImmutableResult>;
    fn upload_begin<'a>(
        &'a self,
        context: &'a TransportContext,
        key: &'a str,
        size: u64,
        sha256: [u8; 32],
    ) -> ProviderFuture<'a, UploadSession>;
    fn upload_write<'a>(
        &'a self,
        context: &'a TransportContext,
        upload_id: &'a str,
        offset: u64,
        bytes: &'a [u8],
    ) -> ProviderFuture<'a, u64>;
    fn upload_commit<'a>(
        &'a self,
        context: &'a TransportContext,
        upload_id: &'a str,
    ) -> ProviderFuture<'a, PutImmutableResult>;
    fn upload_abort<'a>(
        &'a self,
        context: &'a TransportContext,
        upload_id: &'a str,
    ) -> ProviderFuture<'a, ()>;
    fn fence_transport<'a>(
        &'a self,
        _binding_epoch: &'a str,
        _before_generation: u64,
    ) -> ProviderFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }
    fn shutdown(&self) -> ProviderFuture<'_, ()>;
}

#[derive(Clone, Default)]
pub struct ProviderManager {
    providers: Arc<Mutex<HashMap<(String, String), Arc<dyn SyncProvider>>>>,
}
impl ProviderManager {
    pub async fn fence_binding(
        &self,
        _plugin_id: &str,
        _account_uuid: &str,
        _binding_epoch: uuid::Uuid,
        _transport_generation: u64,
    ) -> anyhow::Result<()> {
        if let Some(provider) = self.resolve(_plugin_id, _account_uuid)? {
            provider
                .fence_transport(&_binding_epoch.to_string(), _transport_generation)
                .await
                .map_err(anyhow::Error::new)?;
        }
        Ok(())
    }
    pub fn register(
        &self,
        plugin_id: impl Into<String>,
        account_uuid: impl Into<String>,
        provider: Arc<dyn SyncProvider>,
    ) -> anyhow::Result<()> {
        self.providers
            .lock()
            .map_err(|e| anyhow::anyhow!("provider manager poisoned: {e}"))?
            .insert((plugin_id.into(), account_uuid.into()), provider);
        Ok(())
    }
    pub fn resolve(
        &self,
        plugin_id: &str,
        account_uuid: &str,
    ) -> anyhow::Result<Option<Arc<dyn SyncProvider>>> {
        Ok(self
            .providers
            .lock()
            .map_err(|e| anyhow::anyhow!("provider manager poisoned: {e}"))?
            .get(&(plugin_id.to_owned(), account_uuid.to_owned()))
            .cloned())
    }
    pub fn unregister(
        &self,
        plugin_id: &str,
        account_uuid: &str,
    ) -> anyhow::Result<Option<Arc<dyn SyncProvider>>> {
        Ok(self
            .providers
            .lock()
            .map_err(|e| anyhow::anyhow!("provider manager poisoned: {e}"))?
            .remove(&(plugin_id.to_owned(), account_uuid.to_owned())))
    }
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        let providers = self
            .providers
            .lock()
            .map_err(|e| anyhow::anyhow!("provider manager poisoned: {e}"))?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for provider in providers {
            provider.shutdown().await.map_err(anyhow::Error::new)?;
        }
        Ok(())
    }
}

pub fn validate_object(
    key: &str,
    declared: &ObjectMetadata,
    bytes: &[u8],
    max_bytes: u64,
) -> ProviderResult<()> {
    validate_object_key(key)?;
    if bytes.len() as u64 != declared.size || declared.size > max_bytes {
        return Err(error(
            ProviderErrorCode::ProtocolError,
            false,
            "object size mismatch or limit exceeded",
        ));
    }
    let actual: [u8; 32] = Sha256::digest(bytes).into();
    if actual != declared.sha256 {
        return Err(error(
            ProviderErrorCode::ProtocolError,
            false,
            "object hash mismatch",
        ));
    }
    Ok(())
}

pub fn validate_download(object: &DownloadedObject, max_bytes: u64) -> ProviderResult<()> {
    validate_object_key(&object.metadata.key)?;
    let (size, actual) = file_size_and_sha256(&object.path)?;
    if size != object.metadata.size || size > max_bytes {
        return Err(error(
            ProviderErrorCode::ProtocolError,
            false,
            "object size mismatch or limit exceeded",
        ));
    }
    if actual != object.metadata.sha256 {
        return Err(error(
            ProviderErrorCode::ProtocolError,
            false,
            "object hash mismatch",
        ));
    }
    Ok(())
}

fn file_size_and_sha256(path: &Path) -> ProviderResult<(u64, [u8; 32])> {
    let mut file = std::fs::File::open(path).map_err(storage_error)?;
    let size = file.metadata().map_err(storage_error)?.len();
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(storage_error)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok((size, hasher.finalize().into()))
}

fn validate_object_key(key: &str) -> ProviderResult<()> {
    if key.is_empty()
        || key.len() > 1024
        || key.starts_with('/')
        || key
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.contains('\\'))
    {
        return Err(error(
            ProviderErrorCode::InvalidArgument,
            false,
            "invalid immutable object key",
        ));
    }
    Ok(())
}

pub fn intake_verified_object(
    object: &DownloadedObject,
    spool_dir: &Path,
    max_bytes: u64,
) -> ProviderResult<std::path::PathBuf> {
    validate_download(object, max_bytes)?;
    std::fs::create_dir_all(spool_dir).map_err(storage_error)?;
    let final_path = spool_dir.join(hex(&object.metadata.sha256));
    if final_path.exists() {
        let (_, existing_hash) = file_size_and_sha256(&final_path)?;
        if existing_hash == object.metadata.sha256 {
            return Ok(final_path);
        }
        return Err(error(
            ProviderErrorCode::PreconditionFailed,
            false,
            "durable object path hash conflict",
        ));
    }
    let temp = spool_dir.join(format!(
        ".{}.{}.tmp",
        hex(&object.metadata.sha256),
        uuid::Uuid::now_v7().simple()
    ));
    {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .map_err(storage_error)?;
        let mut source = std::fs::File::open(&object.path).map_err(storage_error)?;
        std::io::copy(&mut source, &mut file).map_err(storage_error)?;
        file.sync_all().map_err(storage_error)?;
    }
    std::fs::rename(&temp, &final_path).map_err(storage_error)?;
    if let Ok(dir) = std::fs::File::open(spool_dir) {
        let _ = dir.sync_all();
    }
    Ok(final_path)
}
fn error(code: ProviderErrorCode, retryable: bool, detail: impl Into<String>) -> ProviderError {
    ProviderError {
        code,
        retryable,
        retry_after_ms: None,
        detail: detail.into(),
    }
}
fn storage_error(e: std::io::Error) -> ProviderError {
    error(ProviderErrorCode::Internal, true, e.to_string())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// Benchmark fixture compatibility only. Production coordination uses
// `SyncProvider`; this tiny source is not exported in non-test builds.
#[cfg(test)]
pub trait SegmentProvider {
    fn pending_segments(&self) -> anyhow::Result<Vec<Vec<u8>>>;
}
#[cfg(test)]
pub fn intake_provider_segments(
    provider: &dyn SegmentProvider,
    spool_dir: &Path,
) -> anyhow::Result<usize> {
    let mut accepted = 0;
    for bytes in provider.pending_segments()? {
        super::spool::intake_segment(spool_dir, &bytes, None, |_| Ok(()))?;
        accepted += 1;
    }
    Ok(accepted)
}

#[derive(Default)]
pub struct InMemorySyncProvider {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    uploads: Mutex<HashMap<String, (String, u64, [u8; 32], Vec<u8>)>>,
    authenticated: Mutex<bool>,
}
impl SyncProvider for InMemorySyncProvider {
    fn describe(&self) -> ProviderFuture<'_, ProviderDescription> {
        Box::pin(async {
            Ok(ProviderDescription {
                plugin_id: "com.megle.fake".into(),
                account_uuid: "fake-account".into(),
                capabilities: vec!["sync-provider.v1".into()],
                max_object_bytes: 64 * 1024 * 1024,
            })
        })
    }
    fn object_get_range<'a>(
        &'a self,
        _: &'a TransportContext,
        key: &'a str,
        start: u64,
        end: u64,
        max_bytes: u64,
    ) -> ProviderFuture<'a, DownloadedRange> {
        Box::pin(async move {
            if end <= start || end - start > max_bytes {
                return Err(error(
                    ProviderErrorCode::InvalidArgument,
                    false,
                    "invalid object range",
                ));
            }
            let objects = self
                .objects
                .lock()
                .map_err(|e| error(ProviderErrorCode::Internal, true, e.to_string()))?;
            let bytes = objects
                .get(key)
                .ok_or_else(|| error(ProviderErrorCode::NotFound, false, "object not found"))?;
            if end > bytes.len() as u64 {
                return Err(error(
                    ProviderErrorCode::InvalidArgument,
                    false,
                    "object range exceeds size",
                ));
            }
            let range = bytes[start as usize..end as usize].to_vec();
            let path = std::env::temp_dir().join(format!(
                "megle-memory-provider-{}-{}.range",
                std::process::id(),
                uuid::Uuid::now_v7().simple()
            ));
            std::fs::write(&path, &range).map_err(storage_error)?;
            Ok(DownloadedRange {
                metadata: ObjectMetadata {
                    key: key.into(),
                    size: bytes.len() as u64,
                    sha256: Sha256::digest(bytes).into(),
                    etag: Some(hex(&Sha256::digest(bytes))),
                },
                requested_start: start,
                requested_end: end,
                response_start: start,
                response_end: end,
                whole_object: false,
                path,
            })
        })
    }
    fn health(&self) -> ProviderFuture<'_, ProviderHealth> {
        Box::pin(async {
            Ok(ProviderHealth {
                ready: true,
                detail: None,
            })
        })
    }
    fn diagnostics(&self) -> ProviderFuture<'_, serde_json::Value> {
        Box::pin(async move {
            Ok(
                serde_json::json!({"objects":self.objects.lock().map_err(|e|error(ProviderErrorCode::Internal,true,e.to_string()))?.len()}),
            )
        })
    }
    fn auth_begin(&self, _: Option<String>) -> ProviderFuture<'_, AuthBegin> {
        Box::pin(async {
            Ok(AuthBegin {
                authorization_url: "https://example.invalid/authorize".into(),
                flow_id: uuid::Uuid::now_v7().to_string(),
            })
        })
    }
    fn auth_complete<'a>(
        &'a self,
        _: &'a str,
        _: BTreeMap<String, String>,
    ) -> ProviderFuture<'a, AuthStatus> {
        Box::pin(async move {
            *self
                .authenticated
                .lock()
                .map_err(|e| error(ProviderErrorCode::Internal, true, e.to_string()))? = true;
            Ok(AuthStatus {
                authenticated: true,
                account_display_name: Some("Fake account".into()),
                stable_account_identity: None,
                account_kind: None,
            })
        })
    }
    fn auth_status(&self) -> ProviderFuture<'_, AuthStatus> {
        Box::pin(async move {
            Ok(AuthStatus {
                authenticated: *self
                    .authenticated
                    .lock()
                    .map_err(|e| error(ProviderErrorCode::Internal, true, e.to_string()))?,
                account_display_name: Some("Fake account".into()),
                stable_account_identity: None,
                account_kind: None,
            })
        })
    }
    fn auth_sign_out(&self) -> ProviderFuture<'_, ()> {
        Box::pin(async move {
            *self
                .authenticated
                .lock()
                .map_err(|e| error(ProviderErrorCode::Internal, true, e.to_string()))? = false;
            Ok(())
        })
    }
    fn delta_list<'a>(
        &'a self,
        _: DeltaScope,
        cursor: Option<&'a str>,
        page_token: Option<&'a str>,
        limit: usize,
    ) -> ProviderFuture<'a, DeltaPage> {
        Box::pin(async move {
            let objects = self
                .objects
                .lock()
                .map_err(|e| error(ProviderErrorCode::Internal, true, e.to_string()))?;
            let start = page_token.and_then(|v| v.parse().ok()).unwrap_or(0);
            let limit = limit.clamp(1, 100);
            let items = objects
                .iter()
                .skip(start)
                .take(limit)
                .map(|(key, bytes)| ObjectMetadata {
                    key: key.clone(),
                    size: bytes.len() as u64,
                    sha256: Sha256::digest(bytes).into(),
                    etag: Some(hex(&Sha256::digest(bytes))),
                })
                .collect::<Vec<_>>();
            let next = start + items.len();
            Ok(DeltaPage {
                objects: items,
                page_token: (next < objects.len()).then(|| next.to_string()),
                completed_cursor: (next >= objects.len())
                    .then(|| format!("v{}", objects.len()))
                    .or_else(|| cursor.map(str::to_owned)),
            })
        })
    }
    fn object_head<'a>(
        &'a self,
        _: &'a TransportContext,
        key: &'a str,
    ) -> ProviderFuture<'a, Option<ObjectMetadata>> {
        Box::pin(async move {
            let objects = self
                .objects
                .lock()
                .map_err(|e| error(ProviderErrorCode::Internal, true, e.to_string()))?;
            Ok(objects.get(key).map(|b| ObjectMetadata {
                key: key.into(),
                size: b.len() as u64,
                sha256: Sha256::digest(b).into(),
                etag: Some(hex(&Sha256::digest(b))),
            }))
        })
    }
    fn object_get<'a>(
        &'a self,
        _: &'a TransportContext,
        key: &'a str,
        max_bytes: u64,
    ) -> ProviderFuture<'a, DownloadedObject> {
        Box::pin(async move {
            let objects = self
                .objects
                .lock()
                .map_err(|e| error(ProviderErrorCode::Internal, true, e.to_string()))?;
            let bytes = objects
                .get(key)
                .ok_or_else(|| error(ProviderErrorCode::NotFound, false, "object not found"))?
                .clone();
            if bytes.len() as u64 > max_bytes {
                return Err(error(
                    ProviderErrorCode::ProtocolError,
                    false,
                    "object exceeds caller limit",
                ));
            }
            let path = std::env::temp_dir().join(format!(
                "megle-memory-provider-{}-{}.object",
                std::process::id(),
                uuid::Uuid::now_v7().simple()
            ));
            {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&path)
                    .map_err(storage_error)?;
                file.write_all(&bytes).map_err(storage_error)?;
                file.sync_all().map_err(storage_error)?;
            }
            Ok(DownloadedObject {
                metadata: ObjectMetadata {
                    key: key.into(),
                    size: bytes.len() as u64,
                    sha256: Sha256::digest(&bytes).into(),
                    etag: Some(hex(&Sha256::digest(&bytes))),
                },
                path,
            })
        })
    }
    fn object_put_immutable<'a>(
        &'a self,
        _: &'a TransportContext,
        key: &'a str,
        bytes: &'a [u8],
        sha256: [u8; 32],
    ) -> ProviderFuture<'a, PutImmutableResult> {
        Box::pin(async move {
            if Sha256::digest(bytes).as_slice() != sha256 {
                return Err(error(
                    ProviderErrorCode::InvalidArgument,
                    false,
                    "caller hash mismatch",
                ));
            }
            let mut objects = self
                .objects
                .lock()
                .map_err(|e| error(ProviderErrorCode::Internal, true, e.to_string()))?;
            if let Some(existing) = objects.get(key) {
                let remote: [u8; 32] = Sha256::digest(existing).into();
                return Ok(if remote == sha256 {
                    PutImmutableResult::AlreadyExistsMatching {
                        etag: Some(hex(&remote)),
                    }
                } else {
                    PutImmutableResult::HashConflict {
                        remote_sha256: remote,
                    }
                });
            }
            objects.insert(key.into(), bytes.to_vec());
            Ok(PutImmutableResult::Created {
                etag: Some(hex(&sha256)),
            })
        })
    }
    fn upload_begin<'a>(
        &'a self,
        _: &'a TransportContext,
        key: &'a str,
        size: u64,
        sha256: [u8; 32],
    ) -> ProviderFuture<'a, UploadSession> {
        Box::pin(async move {
            let id = uuid::Uuid::now_v7().to_string();
            self.uploads
                .lock()
                .map_err(|e| error(ProviderErrorCode::Internal, true, e.to_string()))?
                .insert(id.clone(), (key.into(), size, sha256, Vec::new()));
            Ok(UploadSession {
                upload_id: id,
                committed_bytes: 0,
            })
        })
    }
    fn upload_write<'a>(
        &'a self,
        _: &'a TransportContext,
        id: &'a str,
        offset: u64,
        bytes: &'a [u8],
    ) -> ProviderFuture<'a, u64> {
        Box::pin(async move {
            let mut uploads = self
                .uploads
                .lock()
                .map_err(|e| error(ProviderErrorCode::Internal, true, e.to_string()))?;
            let upload = uploads
                .get_mut(id)
                .ok_or_else(|| error(ProviderErrorCode::NotFound, false, "upload not found"))?;
            if upload.3.len() as u64 != offset || offset + bytes.len() as u64 > upload.1 {
                return Err(error(
                    ProviderErrorCode::PreconditionFailed,
                    false,
                    "upload offset or size mismatch",
                ));
            }
            upload.3.extend_from_slice(bytes);
            Ok(upload.3.len() as u64)
        })
    }
    fn upload_commit<'a>(
        &'a self,
        context: &'a TransportContext,
        id: &'a str,
    ) -> ProviderFuture<'a, PutImmutableResult> {
        Box::pin(async move {
            let (key, size, hash, bytes) = self
                .uploads
                .lock()
                .map_err(|e| error(ProviderErrorCode::Internal, true, e.to_string()))?
                .remove(id)
                .ok_or_else(|| error(ProviderErrorCode::NotFound, false, "upload not found"))?;
            if bytes.len() as u64 != size || Sha256::digest(&bytes).as_slice() != hash {
                return Err(error(
                    ProviderErrorCode::PreconditionFailed,
                    false,
                    "upload incomplete or corrupt",
                ));
            }
            self.object_put_immutable(context, &key, &bytes, hash).await
        })
    }
    fn upload_abort<'a>(&'a self, _: &'a TransportContext, id: &'a str) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            self.uploads
                .lock()
                .map_err(|e| error(ProviderErrorCode::Internal, true, e.to_string()))?
                .remove(id);
            Ok(())
        })
    }
    fn shutdown(&self) -> ProviderFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn conformance(provider: &dyn SyncProvider) {
        let bytes = b"immutable";
        let hash = Sha256::digest(bytes).into();
        let context = TransportContext {
            namespace: uuid::Uuid::now_v7().to_string(),
            binding_epoch: uuid::Uuid::now_v7().to_string(),
            transport_generation: 1,
        };
        assert!(matches!(
            provider
                .object_put_immutable(&context, "segments/a", bytes, hash)
                .await
                .unwrap(),
            PutImmutableResult::Created { .. }
        ));
        assert!(matches!(
            provider
                .object_put_immutable(&context, "segments/a", bytes, hash)
                .await
                .unwrap(),
            PutImmutableResult::AlreadyExistsMatching { .. }
        ));
        let read = provider
            .object_get(&context, "segments/a", 1024)
            .await
            .unwrap();
        validate_download(&read, 1024).unwrap();
        let page = provider
            .delta_list(DeltaScope::Root, None, None, 10)
            .await
            .unwrap();
        assert_eq!(page.objects.len(), 1);
        let up = provider
            .upload_begin(&context, "checkpoints/b", 4, Sha256::digest(b"next").into())
            .await
            .unwrap();
        assert_eq!(
            provider
                .upload_write(&context, &up.upload_id, 0, b"ne")
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            provider
                .upload_write(&context, &up.upload_id, 2, b"xt")
                .await
                .unwrap(),
            4
        );
        assert!(matches!(
            provider
                .upload_commit(&context, &up.upload_id)
                .await
                .unwrap(),
            PutImmutableResult::Created { .. }
        ));
    }
    #[tokio::test]
    async fn memory_provider_conforms() {
        conformance(&InMemorySyncProvider::default()).await
    }

    #[tokio::test]
    async fn expired_partial_upload_restarts_without_committing_truncated_bytes() {
        let provider = InMemorySyncProvider::default();
        let context = TransportContext {
            namespace: uuid::Uuid::now_v7().to_string(),
            binding_epoch: uuid::Uuid::now_v7().to_string(),
            transport_generation: 1,
        };
        let bytes = b"complete immutable object";
        let hash: [u8; 32] = Sha256::digest(bytes).into();
        let expired = provider
            .upload_begin(
                &context,
                "content/data/00/session.pack",
                bytes.len() as u64,
                hash,
            )
            .await
            .unwrap();
        provider
            .upload_write(&context, &expired.upload_id, 0, &bytes[..8])
            .await
            .unwrap();
        provider
            .upload_abort(&context, &expired.upload_id)
            .await
            .unwrap();
        assert!(provider
            .upload_write(&context, &expired.upload_id, 8, &bytes[8..])
            .await
            .is_err());
        assert!(provider
            .object_head(&context, "content/data/00/session.pack")
            .await
            .unwrap()
            .is_none());
        let resumed = provider
            .upload_begin(
                &context,
                "content/data/00/session.pack",
                bytes.len() as u64,
                hash,
            )
            .await
            .unwrap();
        provider
            .upload_write(&context, &resumed.upload_id, 0, bytes)
            .await
            .unwrap();
        assert!(matches!(
            provider
                .upload_commit(&context, &resumed.upload_id)
                .await
                .unwrap(),
            PutImmutableResult::Created { .. }
        ));
    }

    #[tokio::test]
    #[ignore = "invoked by tools/bench/plugin-runtime-bench.mjs"]
    async fn plugin_runtime_eight_library_probe() {
        let manager = ProviderManager::default();
        let provider: Arc<dyn SyncProvider> = Arc::new(InMemorySyncProvider::default());
        manager
            .register("com.megle.fake", "account", Arc::clone(&provider))
            .unwrap();
        let started = std::time::Instant::now();
        let mut turns = tokio::task::JoinSet::new();
        for index in 0..8 {
            let provider = manager
                .resolve("com.megle.fake", "account")
                .unwrap()
                .unwrap();
            turns.spawn(async move {
                let bytes = format!("library-{index}").into_bytes();
                let hash = Sha256::digest(&bytes).into();
                let context = TransportContext {
                    namespace: uuid::Uuid::now_v7().to_string(),
                    binding_epoch: uuid::Uuid::now_v7().to_string(),
                    transport_generation: 1,
                };
                provider
                    .object_put_immutable(
                        &context,
                        &format!("segments/library-{index}"),
                        &bytes,
                        hash,
                    )
                    .await
                    .unwrap();
                provider
                    .delta_list(
                        DeltaScope::Library {
                            namespace: format!("library-{index}"),
                            binding_epoch: uuid::Uuid::now_v7().to_string(),
                            transport_generation: 1,
                        },
                        None,
                        None,
                        100,
                    )
                    .await
                    .unwrap();
            });
        }
        while let Some(result) = turns.join_next().await {
            result.unwrap();
        }
        let elapsed = started.elapsed();
        let diagnostics = provider.diagnostics().await.unwrap();
        assert_eq!(diagnostics["objects"], 8);
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "eight Library provider probe took {elapsed:?}"
        );
        println!(
            "PLUGIN_RUNTIME_BENCH {}",
            serde_json::json!({"libraries":8,"processes":1,"elapsedMs":elapsed.as_secs_f64()*1000.0,"bounded":true})
        );
    }
    #[test]
    fn rejects_bad_object_keys_and_hash() {
        let data = b"x";
        let meta = ObjectMetadata {
            key: "../db".into(),
            size: 1,
            sha256: Sha256::digest(data).into(),
            etag: None,
        };
        assert!(validate_object("../db", &meta, data, 2).is_err());
    }
}
