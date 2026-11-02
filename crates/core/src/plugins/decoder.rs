//! Local, accountless file decoder ownership. No Provider calls are made here.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub use super::manifest::DecoderPurpose;
use super::manifest::{DecoderDeclaration, ManifestRecord};
use super::runtime::decoder_process::DecoderProcess;
use super::runtime::rpc::RpcError;

pub const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_OUTPUT_PIXELS: u64 = 10_000_000;
pub const MAX_OUTPUT_BYTES: u64 = MAX_OUTPUT_PIXELS * 4;
pub const MAX_SOURCE_PIXELS: u64 = 16_000_000;
pub const MAX_HEADER_BYTES: u64 = 64 * 1024;
pub const CHUNK_BYTES: usize = 256 * 1024;
pub const MAX_TASK_TEMP_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_GLOBAL_TEMP_BYTES: u64 = 768 * 1024 * 1024;
pub const MAX_CONCURRENT_JOBS: usize = 2;

impl DecoderPurpose {
    pub fn maximum_edge(self) -> u32 {
        match self {
            Self::Thumbnail => 1024,
            Self::Preview => 3072,
        }
    }
    pub fn timeout(self) -> Duration {
        Duration::from_secs(match self {
            Self::Thumbnail => 15,
            Self::Preview => 30,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecoderIdentity {
    pub plugin_id: String,
    pub version: String,
    pub implementation_version: String,
    pub artifact_sha256: String,
}

impl DecoderIdentity {
    pub fn cache_key(&self) -> String {
        format!(
            "file-decoder.v1:{}:{}:{}:{}",
            self.plugin_id, self.version, self.implementation_version, self.artifact_sha256
        )
    }
}

#[derive(Debug, Clone)]
pub struct DecoderCapability {
    pub identity: DecoderIdentity,
    pub declaration: DecoderDeclaration,
    pub lifecycle_epoch: i64,
}

impl DecoderCapability {
    pub fn failure_cache_key(&self) -> String {
        format!(
            "{}:failure-epoch:{}",
            self.identity.cache_key(),
            self.lifecycle_epoch
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct DecoderSnapshot {
    pub generation: u64,
    pub capabilities: Vec<DecoderCapability>,
}

impl DecoderSnapshot {
    pub fn candidates(&self, extension: &str, purpose: DecoderPurpose) -> Vec<&DecoderCapability> {
        self.capabilities
            .iter()
            .filter(|capability| {
                capability.declaration.purposes.contains(&purpose)
                    && capability
                        .declaration
                        .extensions
                        .iter()
                        .any(|value| value.eq_ignore_ascii_case(extension.trim_start_matches('.')))
            })
            .collect()
    }

    pub fn failure_key(
        &self,
        capability: &DecoderCapability,
        extension: &str,
        purpose: DecoderPurpose,
    ) -> String {
        let candidates = self.candidates(extension, purpose);
        let key = capability.failure_cache_key();
        if candidates.len() <= 1 {
            return key;
        }
        let mut hash = Sha256::new();
        for candidate in candidates {
            hash.update(candidate.failure_cache_key());
            hash.update([0]);
        }
        format!("{key}:candidates:{:x}", hash.finalize())
    }

    pub fn cache_key_matches(
        &self,
        key: &str,
        extension: &str,
        purpose: DecoderPurpose,
        failed: bool,
    ) -> bool {
        let candidates = self.candidates(extension, purpose);
        candidates.is_empty()
            || candidates.into_iter().any(|candidate| {
                key == if failed {
                    self.failure_key(candidate, extension, purpose)
                } else {
                    candidate.identity.cache_key()
                }
            })
    }

    pub fn matching(&self, extension: &str, purpose: DecoderPurpose) -> Option<&DecoderCapability> {
        self.capabilities.iter().find(|capability| {
            capability.declaration.purposes.contains(&purpose)
                && capability
                    .declaration
                    .extensions
                    .iter()
                    .any(|value| value.eq_ignore_ascii_case(extension.trim_start_matches('.')))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderError {
    Unavailable,
    Busy,
    Unsupported,
    InvalidData,
    ResourceLimit,
    SourceChanged,
    Cancelled,
    Timeout,
    Protocol,
    Process,
    Io,
}

impl std::fmt::Display for DecoderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "file decoder {self:?}")
    }
}
impl std::error::Error for DecoderError {}

pub struct DecoderFailure {
    pub error: DecoderError,
    pub publication_guard: Option<DecoderPublicationGuard>,
    pub cache_key: Option<String>,
}
impl From<DecoderError> for DecoderFailure {
    fn from(error: DecoderError) -> Self {
        Self {
            error,
            publication_guard: None,
            cache_key: None,
        }
    }
}

struct CancelDispatchedJob {
    process: Arc<DecoderProcess>,
    dispatched: Arc<AtomicBool>,
    deadline: Instant,
    armed: bool,
}
impl Drop for CancelDispatchedJob {
    fn drop(&mut self) {
        if self.armed && self.dispatched.load(Ordering::Acquire) && Instant::now() < self.deadline {
            self.process.mark_job_cancelled();
        }
    }
}
impl From<RpcError> for DecoderError {
    fn from(error: RpcError) -> Self {
        match error {
            RpcError::DeadlineExceeded | RpcError::AdmissionDeadlineExceeded => Self::Timeout,
            RpcError::Protocol => Self::Protocol,
            RpcError::Closed => Self::Process,
            RpcError::Remote { code, .. } => match code.as_str() {
                "unsupported" => Self::Unsupported,
                "invalid_data" => Self::InvalidData,
                "resource_limit" => Self::ResourceLimit,
                _ => Self::Process,
            },
        }
    }
}

struct Entry {
    capability: DecoderCapability,
    process: Arc<DecoderProcess>,
    cancelled: CancellationToken,
    operation: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    generation: u64,
    entries: BTreeMap<String, Arc<Entry>>,
    active: BTreeMap<(String, i64), usize>,
    closed_epochs: BTreeMap<String, i64>,
}
struct Inner {
    state: Mutex<State>,
    changed: Notify,
    jobs: Arc<Semaphore>,
    queue: Arc<Semaphore>,
    temporary_directory: PathBuf,
}

#[derive(Clone)]
pub struct DecoderManager(Arc<Inner>);

/// Hold until the task's atomic source/attempt/cache transaction completes.
/// `publish` arbitrates with close under the same lock; checking an epoch and
/// dropping it before the transaction is deliberately not exposed as a commit API.
pub struct DecoderPublicationGuard {
    manager: DecoderManager,
    entry: Arc<Entry>,
}
impl DecoderPublicationGuard {
    pub(crate) fn fork(&self) -> Self {
        let mut state = self
            .manager
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        *state
            .active
            .entry((
                self.entry.capability.identity.plugin_id.clone(),
                self.entry.capability.lifecycle_epoch,
            ))
            .or_default() += 1;
        Self {
            manager: self.manager.clone(),
            entry: self.entry.clone(),
        }
    }
    pub fn publish<T>(&self, publish: impl FnOnce() -> T) -> Result<T, DecoderError> {
        let state = self
            .manager
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let identity = &self.entry.capability.identity.plugin_id;
        if self.entry.cancelled.is_cancelled()
            || !state
                .entries
                .get(identity)
                .is_some_and(|entry| Arc::ptr_eq(entry, &self.entry))
        {
            return Err(DecoderError::Cancelled);
        }
        Ok(publish())
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.entry.cancelled.clone()
    }
}
impl Drop for DecoderPublicationGuard {
    fn drop(&mut self) {
        let mut state = self
            .manager
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let key = (
            self.entry.capability.identity.plugin_id.clone(),
            self.entry.capability.lifecycle_epoch,
        );
        if let Some(count) = state.active.get_mut(&key) {
            *count -= 1;
            if *count == 0 {
                state.active.remove(&key);
            }
        }
        self.manager.0.changed.notify_waiters();
    }
}

pub struct DecoderRaster {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub identity: DecoderIdentity,
    pub publication_guard: DecoderPublicationGuard,
    budget: Arc<tokio::sync::OwnedSemaphorePermit>,
}

pub struct DecoderWebp {
    pub data: Vec<u8>,
    pub width: i64,
    pub height: i64,
    pub identity: DecoderIdentity,
    pub publication_guard: DecoderPublicationGuard,
    _budget: Arc<tokio::sync::OwnedSemaphorePermit>,
}

impl DecoderRaster {
    /// Runs on a blocking worker while retaining both lifecycle and memory quota.
    pub fn into_webp(self) -> Result<DecoderWebp, DecoderError> {
        self.publication_guard.publish(|| ())?;
        let encoded = webp::Encoder::from_rgba(&self.rgba, self.width, self.height).encode(80.0);
        Ok(DecoderWebp {
            data: encoded.to_vec(),
            width: i64::from(self.width),
            height: i64::from(self.height),
            identity: self.identity,
            publication_guard: self.publication_guard,
            _budget: self.budget,
        })
    }
}

impl DecoderManager {
    pub fn new(temporary_directory: PathBuf) -> Self {
        // Two immutable input copies (Core/plugin) and one output, per active
        // operation. Anonymous OS files release even after an abnormal exit.
        debug_assert!(MAX_INPUT_BYTES * 2 + MAX_OUTPUT_BYTES <= MAX_TASK_TEMP_BYTES);
        debug_assert!(MAX_TASK_TEMP_BYTES * MAX_CONCURRENT_JOBS as u64 <= MAX_GLOBAL_TEMP_BYTES);
        Self(Arc::new(Inner {
            state: Mutex::new(State::default()),
            changed: Notify::new(),
            jobs: Arc::new(Semaphore::new(MAX_CONCURRENT_JOBS)),
            queue: Arc::new(Semaphore::new(32)),
            temporary_directory,
        }))
    }

    pub fn snapshot(&self) -> DecoderSnapshot {
        let state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        DecoderSnapshot {
            generation: state.generation,
            capabilities: state
                .entries
                .values()
                .filter(|entry| !entry.cancelled.is_cancelled() && !entry.process.is_closed())
                .map(|entry| entry.capability.clone())
                .collect(),
        }
    }
    #[cfg(test)]
    pub fn matching(&self, extension: &str, purpose: DecoderPurpose) -> Option<DecoderCapability> {
        self.snapshot().matching(extension, purpose).cloned()
    }

    pub fn publication_guard(
        &self,
        capability: &DecoderCapability,
    ) -> Result<DecoderPublicationGuard, DecoderError> {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        let entry = state
            .entries
            .get(&capability.identity.plugin_id)
            .filter(|entry| {
                entry.capability.lifecycle_epoch == capability.lifecycle_epoch
                    && entry.capability.identity == capability.identity
                    && !entry.cancelled.is_cancelled()
            })
            .cloned()
            .ok_or(DecoderError::Unavailable)?;
        *state
            .active
            .entry((
                entry.capability.identity.plugin_id.clone(),
                entry.capability.lifecycle_epoch,
            ))
            .or_default() += 1;
        Ok(DecoderPublicationGuard {
            manager: self.clone(),
            entry,
        })
    }

    pub fn register(
        &self,
        manifest: &ManifestRecord,
        lifecycle_epoch: i64,
        process: Arc<DecoderProcess>,
    ) -> anyhow::Result<()> {
        let declaration = manifest
            .rpc
            .decoder
            .clone()
            .ok_or_else(|| anyhow::anyhow!("missing decoder declaration"))?;
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        anyhow::ensure!(
            !state.entries.contains_key(&manifest.id)
                && !state
                    .closed_epochs
                    .get(&manifest.id)
                    .is_some_and(|epoch| *epoch >= lifecycle_epoch),
            "decoder epoch is closed"
        );
        let capability = DecoderCapability {
            lifecycle_epoch,
            declaration: declaration.clone(),
            identity: DecoderIdentity {
                plugin_id: manifest.id.clone(),
                version: manifest.version.clone(),
                implementation_version: declaration.implementation_version,
                artifact_sha256: manifest.integrity.sha256.clone(),
            },
        };
        state.entries.insert(
            manifest.id.clone(),
            Arc::new(Entry {
                capability,
                process,
                cancelled: CancellationToken::new(),
                operation: tokio::sync::Mutex::new(()),
            }),
        );
        state.generation = state.generation.saturating_add(1);
        Ok(())
    }

    /// Synchronous admission fence. Call at the same lifecycle boundary as the
    /// Provider fence, then join before acknowledging a completed disable.
    pub fn close_plugin(&self, plugin_id: &str, epoch: i64) {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        let previous = state
            .closed_epochs
            .entry(plugin_id.to_owned())
            .or_insert(epoch);
        *previous = (*previous).max(epoch);
        if let Some(entry) = state.entries.get(plugin_id) {
            entry.cancelled.cancel();
            entry.process.close_admission();
        }
        state.generation = state.generation.saturating_add(1);
    }

    pub async fn stop_plugin(&self, plugin_id: &str, epoch: i64) -> anyhow::Result<()> {
        self.close_plugin(plugin_id, epoch);
        self.join_stopped_plugin(plugin_id).await
    }

    pub fn retired_after_job_cancellation(&self, plugin_id: &str) -> bool {
        self.0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entries
            .get(plugin_id)
            .is_some_and(|entry| entry.process.was_job_cancelled())
    }

    #[cfg(test)]
    pub(crate) fn job_dispatched_for_test(&self, plugin_id: &str) -> bool {
        self.0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entries
            .get(plugin_id)
            .is_some_and(|entry| entry.process.job_dispatched_for_test())
    }

    pub async fn stop_expected_cancelled_process(
        &self,
        plugin_id: &str,
        epoch: i64,
    ) -> anyhow::Result<()> {
        {
            let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(entry) = state.entries.get(plugin_id) {
                anyhow::ensure!(
                    entry.capability.lifecycle_epoch == epoch && entry.process.was_job_cancelled(),
                    "decoder retirement is not an expected job cancellation"
                );
                entry.cancelled.cancel();
                entry.process.close_admission();
            }
            state.generation = state.generation.saturating_add(1);
        }
        // A job cancellation revokes this process owner, not the enabled
        // lifecycle epoch. A concurrent explicit disable still fences register.
        self.join_stopped_plugin(plugin_id).await
    }

    async fn join_stopped_plugin(&self, plugin_id: &str) -> anyhow::Result<()> {
        let entry = self
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entries
            .get(plugin_id)
            .cloned();
        if let Some(entry) = &entry {
            entry.process.shutdown().await?;
        }
        loop {
            let changed = self.0.changed.notified();
            let joined = {
                let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
                if !state.active.keys().any(|(id, _)| id == plugin_id) {
                    state.entries.remove(plugin_id);
                    true
                } else {
                    false
                }
            };
            if joined {
                break;
            }
            changed.await;
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        let entries: Vec<_> = self
            .0
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entries
            .values()
            .map(|entry| {
                (
                    entry.capability.identity.plugin_id.clone(),
                    entry.capability.lifecycle_epoch,
                )
            })
            .collect();
        for (id, epoch) in entries {
            self.stop_plugin(&id, epoch).await?;
        }
        Ok(())
    }

    /// Caller supplies an already-authorized open source and retains its root,
    /// Library, file revision and attempt checks through `publication_guard`.
    #[cfg(test)]
    pub async fn decode(
        &self,
        source: File,
        extension: &str,
        purpose: DecoderPurpose,
        target_max_px: u32,
        cancellation: CancellationToken,
    ) -> Result<DecoderRaster, DecoderError> {
        self.decode_owned(source, extension, purpose, target_max_px, cancellation)
            .await
            .map_err(|failure| failure.error)
    }

    pub async fn decode_owned(
        &self,
        mut source: File,
        extension: &str,
        purpose: DecoderPurpose,
        target_max_px: u32,
        cancellation: CancellationToken,
    ) -> Result<DecoderRaster, DecoderFailure> {
        let deadline = Instant::now() + purpose.timeout();
        let _queued = self
            .0
            .queue
            .clone()
            .try_acquire_owned()
            .map_err(|_| DecoderError::Busy)?;
        let entries: Vec<_> = {
            let state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
            state
                .entries
                .values()
                .filter(|entry| {
                    !entry.cancelled.is_cancelled()
                        && !entry.process.is_closed()
                        && entry.capability.declaration.purposes.contains(&purpose)
                        && entry.capability.declaration.extensions.iter().any(|value| {
                            value.eq_ignore_ascii_case(extension.trim_start_matches('.'))
                        })
                })
                .cloned()
                .collect()
        };
        let snapshot_capabilities = DecoderSnapshot {
            generation: 0,
            capabilities: entries
                .iter()
                .map(|entry| entry.capability.clone())
                .collect(),
        };
        let count = entries.len();
        for (index, entry) in entries.into_iter().enumerate() {
            let guard = self.publication_guard(&entry.capability)?;
            let cancel = entry.cancelled.child_token();
            // Authorized source remains pinned by the caller. Each fallback
            // rewinds it; all candidates share the original absolute deadline.
            source
                .seek(SeekFrom::Start(0))
                .map_err(|_| DecoderError::Io)?;
            let candidate_source = source.try_clone().map_err(|_| DecoderError::Io)?;
            let dispatched = Arc::new(AtomicBool::new(false));
            let result = {
                let operation = async {
                    let _plugin_operation = entry.operation.lock().await;
                    let global = Arc::new(
                        self.0
                            .jobs
                            .clone()
                            .acquire_owned()
                            .await
                            .map_err(|_| DecoderError::Cancelled)?,
                    );
                    let snapshot = snapshot(
                        candidate_source,
                        &self.0.temporary_directory,
                        &cancel,
                        deadline,
                        guard.fork(),
                        global.clone(),
                    )
                    .await?;
                    dispatched.store(true, Ordering::Release);
                    let rgba = entry
                        .process
                        .decode(
                            snapshot.file,
                            snapshot.size,
                            snapshot.sha256,
                            purpose,
                            target_max_px.min(purpose.maximum_edge()).max(1),
                            deadline,
                            guard.fork(),
                            global.clone(),
                        )
                        .await?;
                    Ok((rgba, global))
                };
                tokio::pin!(operation);
                // Declared after the inner future so caller-drop marks the
                // cause before OperationGuard retires its process session.
                let mut dropped = CancelDispatchedJob {
                    process: entry.process.clone(),
                    dispatched: dispatched.clone(),
                    deadline,
                    armed: true,
                };
                let result = tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        if dispatched.load(Ordering::Acquire) { entry.process.mark_job_cancelled(); }
                        Err(DecoderError::Cancelled)
                    },
                    _ = entry.cancelled.cancelled() => {
                        if dispatched.load(Ordering::Acquire) { entry.process.mark_job_cancelled(); }
                        Err(DecoderError::Cancelled)
                    },
                    result = tokio::time::timeout_at(deadline, &mut operation) => result.unwrap_or(Err(DecoderError::Timeout)),
                };
                dropped.armed = false;
                result
            };
            cancel.cancel();
            match result {
                Ok(((rgba, width, height), budget)) => {
                    guard.publish(|| ())?;
                    return Ok(DecoderRaster {
                        rgba,
                        width,
                        height,
                        identity: entry.capability.identity.clone(),
                        publication_guard: guard,
                        budget,
                    });
                }
                Err(error) => {
                    if entry.process.is_closed() {
                        entry
                            .process
                            .shutdown()
                            .await
                            .map_err(|_| DecoderError::Process)?;
                    }
                    if error == DecoderError::Unsupported && index + 1 < count {
                        continue;
                    }
                    return Err(DecoderFailure {
                        error,
                        cache_key: Some(snapshot_capabilities.failure_key(
                            &entry.capability,
                            extension,
                            purpose,
                        )),
                        publication_guard: Some(guard),
                    });
                }
            }
        }
        Err(DecoderError::Unavailable.into())
    }
}

struct Snapshot {
    file: File,
    size: u64,
    sha256: [u8; 32],
}
async fn snapshot(
    mut source: File,
    directory: &Path,
    cancel: &CancellationToken,
    deadline: Instant,
    ownership: DecoderPublicationGuard,
    budget: Arc<tokio::sync::OwnedSemaphorePermit>,
) -> Result<Snapshot, DecoderError> {
    let directory = directory.to_owned();
    let cancellation = cancel.clone();
    // This worker keeps the OS file handles until its final cancellation check.
    // A caller-drop guard cancels it; no task path survives worker completion.
    let worker_cancel = cancellation.clone().drop_guard();
    let result = tokio::task::spawn_blocking(move || {
        let _ownership = ownership;
        let _budget = budget;
        std::fs::create_dir_all(&directory).map_err(|_| DecoderError::Io)?;
        let before = source.metadata().map_err(|_| DecoderError::Io)?;
        if !before.is_file() || before.len() > MAX_INPUT_BYTES {
            return Err(DecoderError::ResourceLimit);
        }
        let mut target = tempfile::tempfile_in(directory).map_err(|_| DecoderError::Io)?;
        let mut hash = Sha256::new();
        let mut bytes = [0u8; CHUNK_BYTES];
        let mut size = 0u64;
        loop {
            if cancellation.is_cancelled() {
                return Err(DecoderError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(DecoderError::Timeout);
            }
            let count = source.read(&mut bytes).map_err(|_| DecoderError::Io)?;
            if count == 0 {
                break;
            }
            size += count as u64;
            if size > MAX_INPUT_BYTES {
                return Err(DecoderError::ResourceLimit);
            }
            hash.update(&bytes[..count]);
            target
                .write_all(&bytes[..count])
                .map_err(|_| DecoderError::Io)?;
        }
        let after = source.metadata().map_err(|_| DecoderError::Io)?;
        if before.len() != size
            || after.len() != size
            || before.modified().ok() != after.modified().ok()
        {
            return Err(DecoderError::SourceChanged);
        }
        target
            .seek(SeekFrom::Start(0))
            .map_err(|_| DecoderError::Io)?;
        Ok(Snapshot {
            file: target,
            size,
            sha256: hash.finalize().into(),
        })
    })
    .await
    .map_err(|_| DecoderError::Io)?;
    worker_cancel.disarm();
    result
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::plugins::runtime::trust::embedded_trust_registry;

    pub(crate) async fn real_decoder() -> (tempfile::TempDir, DecoderManager, ManifestRecord) {
        let root = tempfile::tempdir().unwrap();
        let executable = std::env::current_exe().unwrap();
        let profile = executable.parent().unwrap().parent().unwrap();
        let bundle = profile
            .join("bundled-plugins")
            .join("com.mangoo.ppm-decoder");
        let manifest = crate::plugins::manifest::parse_manifest(&bundle.join("plugin.json"))
            .expect("run node tools/build/build-bundled-plugins.mjs before decoder process tests");
        let manager = DecoderManager::new(root.path().join("snapshot"));
        activate_real_decoder(root.path(), &manager, &manifest, 1).await;
        (root, manager, manifest)
    }

    pub(crate) async fn activate_real_decoder(
        root: &Path,
        manager: &DecoderManager,
        manifest: &ManifestRecord,
        epoch: i64,
    ) {
        let executable = std::env::current_exe().unwrap();
        let profile = executable.parent().unwrap().parent().unwrap();
        let bundle = profile
            .join("bundled-plugins")
            .join("com.mangoo.ppm-decoder");
        let platform = if cfg!(windows) {
            crate::plugins::manifest::Platform::WindowsX64
        } else {
            crate::plugins::manifest::Platform::LinuxX64
        };
        let process = DecoderProcess::activate(
            &manifest,
            &bundle,
            platform,
            embedded_trust_registry(),
            root.join("runtime"),
            manifest.requested_permissions.clone(),
        )
        .await
        .unwrap();
        manager.register(&manifest, epoch, process).unwrap();
    }

    fn source(root: &Path, bytes: &[u8]) -> File {
        let path = root.join("source.ppm");
        std::fs::write(&path, bytes).unwrap();
        File::open(path).unwrap()
    }

    #[tokio::test]
    async fn decoder_real_process_pixels_large_input_and_publication_fence() {
        let (root, manager, manifest) = real_decoder().await;
        let first = manager
            .decode(
                source(root.path(), b"P3\n2 1\n255\n255 0 0 0 255 0\n"),
                "ppm",
                DecoderPurpose::Thumbnail,
                1024,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(first.rgba, [255, 0, 0, 255, 0, 255, 0, 255]);
        first.publication_guard.publish(|| ()).unwrap();
        drop(first);
        let invalid = manager
            .decode(
                source(root.path(), b"P3\n1 1\n15\n1 2 3"),
                "ppm",
                DecoderPurpose::Thumbnail,
                1024,
                CancellationToken::new(),
            )
            .await;
        assert!(matches!(invalid, Err(DecoderError::Unsupported)));
        assert!(manager.matching("ppm", DecoderPurpose::Thumbnail).is_some());
        let mut bytes = b"P6\n2048 1024\n255\n".to_vec();
        let header = bytes.len();
        bytes.resize(header + 2048 * 1024 * 3, 0);
        for pixel in bytes[header..].chunks_exact_mut(3) {
            pixel[2] = 255;
        }
        let raster = manager
            .decode(
                source(root.path(), &bytes),
                "ppm",
                DecoderPurpose::Preview,
                1024,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!((raster.width, raster.height), (1024, 512));
        assert!(raster
            .rgba
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 255, 255]));
        let stop_manager = manager.clone();
        let id = manifest.id.clone();
        let stop = tokio::spawn(async move { stop_manager.stop_plugin(&id, 2).await });
        tokio::time::timeout(
            Duration::from_secs(3),
            raster.publication_guard.cancellation().cancelled(),
        )
        .await
        .unwrap();
        assert!(raster
            .publication_guard
            .publish(|| panic!("late publication"))
            .is_err());
        assert!(!stop.is_finished());
        drop(raster);
        tokio::time::timeout(Duration::from_secs(3), stop)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(manager.snapshot().capabilities.is_empty());
        assert_eq!(
            std::fs::read_dir(root.path().join("runtime"))
                .unwrap()
                .count(),
            0
        );
        assert_eq!(
            std::fs::read_dir(root.path().join("snapshot"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn decoder_host_enable_never_creates_sync_account() {
        let root = tempfile::tempdir().unwrap();
        let registry =
            crate::host_registry::HostRegistry::open(root.path().join("host.sqlite")).unwrap();
        let manifest =
            include_str!("../../../../contracts/plugins/fixtures/valid-file-decoder-v2.json");
        let parsed: ManifestRecord = serde_json::from_str(manifest).unwrap();
        registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: &parsed.id,
                version: &parsed.version,
                manifest_json: manifest,
                entry_relative_path: "decoder",
                entry_sha256: &[0; 32],
                install_root: root.path(),
                trust_policy_id: "embedded-v1",
                requested_permissions_json: "[]",
            })
            .unwrap();
        registry.set_plugin_intent(&parsed.id, true, false).unwrap();
        let launches = registry.decoder_runtime_launches().unwrap();
        assert_eq!(launches.len(), 1);
        assert!(registry.plugin_runtime_launches().unwrap().is_empty());
        assert!(registry
            .host_plugin(&parsed.id)
            .unwrap()
            .unwrap()
            .account_uuid
            .is_none());
        registry
            .set_plugin_intent(&parsed.id, false, false)
            .unwrap();
        assert!(registry.decoder_runtime_launches().unwrap().is_empty());
        assert!(!registry
            .record_decoder_runtime_state(&parsed.id, launches[0].lifecycle_epoch, "ready")
            .unwrap());
    }

    #[tokio::test]
    async fn decoder_memory_quota_outlives_rpc_and_cancelled_queue_never_starts() {
        let (root, manager, manifest) = real_decoder().await;
        let first = manager
            .decode(
                source(root.path(), b"P3\n1 1\n255\n255 0 0\n"),
                "ppm",
                DecoderPurpose::Thumbnail,
                320,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(manager.0.jobs.available_permits(), MAX_CONCURRENT_JOBS - 1);
        let encoded = first.into_webp().unwrap();
        assert_eq!(manager.0.jobs.available_permits(), MAX_CONCURRENT_JOBS - 1);
        let held = manager.0.jobs.clone().acquire_owned().await.unwrap();
        let cancel = CancellationToken::new();
        let waiting_manager = manager.clone();
        let input = source(root.path(), b"P3\n1 1\n255\n0 255 0\n");
        let token = cancel.clone();
        let waiting = tokio::spawn(async move {
            waiting_manager
                .decode(input, "ppm", DecoderPurpose::Thumbnail, 320, token)
                .await
        });
        tokio::task::yield_now().await;
        cancel.cancel();
        assert!(matches!(
            waiting.await.unwrap(),
            Err(DecoderError::Cancelled)
        ));
        assert!(manager.matching("ppm", DecoderPurpose::Thumbnail).is_some());
        drop(held);
        drop(encoded);
        assert_eq!(manager.0.jobs.available_permits(), MAX_CONCURRENT_JOBS);
        manager.stop_plugin(&manifest.id, 2).await.unwrap();
    }

    #[tokio::test]
    async fn decoder_oversized_snapshot_falls_back_without_retiring_process() {
        let (root, manager, manifest) = real_decoder().await;
        let path = root.path().join("oversized.ppm");
        File::create(&path)
            .unwrap()
            .set_len(MAX_INPUT_BYTES + 1)
            .unwrap();
        let result = manager
            .decode(
                File::open(&path).unwrap(),
                "ppm",
                DecoderPurpose::Thumbnail,
                1024,
                CancellationToken::new(),
            )
            .await;
        assert!(matches!(result, Err(DecoderError::ResourceLimit)));
        assert_eq!(
            std::fs::read_dir(root.path().join("snapshot"))
                .unwrap()
                .count(),
            0
        );
        let valid = manager
            .decode(
                source(root.path(), b"P3\n1 1\n255\n0 0 255\n"),
                "ppm",
                DecoderPurpose::Thumbnail,
                320,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(valid.rgba, [0, 0, 255, 255]);
        drop(valid);
        manager.stop_plugin(&manifest.id, 2).await.unwrap();
    }
}
