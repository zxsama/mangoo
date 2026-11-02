use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use anyhow::Context;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::db::{Database, DbWriter, NewRoot, SyncRootLocalBindingRecord};
use crate::host_registry::{
    CreationIntent, HostRegistry, LibraryDescriptor, LibraryOpenIntent, OwnedCreationArtifact,
};
#[cfg(test)]
use crate::library_path::{known_cloud_roots, preflight, PreflightResult};
use crate::library_path::{
    marker_identity, read_marker, safe_delete_tree_expected, safe_remove_empty_directory_expected,
    safe_rename_directory_expected, ExistingLibraryRoot, LibraryContentRootInput, LibraryMarker,
    LibraryPlanPreflight, MarkerDirectoryGuard, MarkerState, SafeDirectory, MARKER_FILE_NAME,
    STORAGE_FORMAT_VERSION,
};
use crate::roots::reachability::RootReachability;
use crate::sync::runtime::{RootRuntimeRegistry, RuntimeLease, RuntimeLeases};
use crate::tasks::{start_managed_worker, TaskSender, TaskWorkerHandle};
use crate::watch::{start_watcher, WatcherHandle};

const MAX_CACHED_READ_DATABASES: usize = 8;

#[derive(Default)]
pub(crate) struct ReadDatabaseCache {
    state: Mutex<ReadDatabaseCacheState>,
    drained: Condvar,
}

#[derive(Default)]
struct ReadDatabaseCacheState {
    idle: Vec<Database>,
    active: usize,
    closed: bool,
}

pub(crate) struct ReadDatabaseLease {
    pub database: Option<Database>,
    cache: Arc<ReadDatabaseCache>,
}

impl ReadDatabaseCache {
    pub(crate) fn checkout(self: &Arc<Self>) -> anyhow::Result<ReadDatabaseLease> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        anyhow::ensure!(!state.closed, "library read database cache is closed");
        state.active += 1;
        Ok(ReadDatabaseLease {
            database: state.idle.pop(),
            cache: self.clone(),
        })
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.closed = true;
        state.idle.clear();
        while state.active != 0 {
            state = self.drained.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }
}

impl Drop for ReadDatabaseLease {
    fn drop(&mut self) {
        let mut state = self.cache.state.lock().unwrap_or_else(|p| p.into_inner());
        if !state.closed && state.idle.len() < MAX_CACHED_READ_DATABASES {
            if let Some(database) = self.database.take() {
                state.idle.push(database);
            }
        }
        // Close before publishing completion, including when opening or a read
        // failed. Shutdown must not release the Catalog lease before SQLite.
        drop(self.database.take());
        state.active -= 1;
        if state.active == 0 {
            self.cache.drained.notify_all();
        }
    }
}

pub struct LibraryRuntimeResources {
    #[allow(dead_code)]
    pub library_uuid: Option<String>,
    pub runtime_generation: u64,
    pub writer: DbWriter,
    pub notification_outbox_wake: Arc<tokio::sync::Notify>,
    pub notification_outbox_read_gate: Arc<tokio::sync::Semaphore>,
    pub preview_decode_gate: Arc<tokio::sync::Semaphore>,
    pub fsops_database: Option<Arc<Mutex<Database>>>,
    pub task_queue: TaskSender,
    pub read_database_path: Option<PathBuf>,
    pub(crate) read_database_cache: Arc<ReadDatabaseCache>,
    pub plugins_dir: Option<PathBuf>,
    pub root_reachability: RootReachability,
    pub root_runtime_registry: RootRuntimeRegistry,
    worker: Option<TaskWorkerHandle>,
    watcher: Mutex<Option<WatcherHandle>>,
    cancellation: CancellationToken,
    background_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    background_started: AtomicBool,
    runtime_lease: Mutex<Option<RuntimeLease>>,
}

#[derive(Clone)]
pub struct ActiveSnapshot {
    pub library_uuid: Option<String>,
    pub context_revision: u64,
    pub runtime_generation: u64,
    pub runtime: Option<Arc<LibraryRuntimeResources>>,
}

impl LibraryRuntimeResources {
    #[cfg(test)]
    pub fn from_database_with_task_queue(database: Database, task_queue: TaskSender) -> Arc<Self> {
        let cancellation = database
            .decoder_context()
            .map(|(_, cancellation)| cancellation)
            .unwrap_or_default();
        let notification_outbox_wake = database.notification_outbox_wake();
        Arc::new(Self {
            library_uuid: None,
            runtime_generation: 1,
            writer: DbWriter::spawn(database),
            notification_outbox_wake,
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::PREVIEW_DECODE_CONCURRENCY,
            )),
            fsops_database: None,
            task_queue,
            read_database_path: None,
            read_database_cache: Arc::new(ReadDatabaseCache::default()),
            plugins_dir: None,
            root_reachability: Arc::new(Mutex::new(std::collections::HashMap::new())),
            root_runtime_registry: RootRuntimeRegistry::default(),
            worker: None,
            watcher: Mutex::new(None),
            cancellation,
            background_tasks: Mutex::new(Vec::new()),
            background_started: AtomicBool::new(false),
            runtime_lease: Mutex::new(None),
        })
    }

    pub fn from_database(
        library_uuid: Option<String>,
        runtime_generation: u64,
        database: Database,
        plugins_dir: Option<PathBuf>,
        runtime_lease: Option<RuntimeLease>,
    ) -> anyhow::Result<Arc<Self>> {
        Self::from_database_with_commit_hook(
            library_uuid,
            runtime_generation,
            database,
            plugins_dir,
            runtime_lease,
            None,
        )
    }

    fn from_database_with_commit_hook(
        library_uuid: Option<String>,
        runtime_generation: u64,
        database: Database,
        plugins_dir: Option<PathBuf>,
        runtime_lease: Option<RuntimeLease>,
        authoritative_commit_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> anyhow::Result<Arc<Self>> {
        let cancellation = database
            .decoder_context()
            .map(|(_, cancellation)| cancellation)
            .unwrap_or_default();
        let notification_outbox_wake = database.notification_outbox_wake();
        database.ensure_file_collection_upgrade_scans()?;
        let read_database_path = database.path().map(Path::to_path_buf);
        if let Err(error) = database.flush_dirty_notification_heads() {
            tracing::error!(%error, "startup notification dirty-head flush failed");
        }
        let worker_database = database.reopen()?;
        let watcher_database = database.reopen()?;
        let fsops_database = database.reopen()?.map(|db| Arc::new(Mutex::new(db)));
        let writer =
            DbWriter::spawn_with_authoritative_commit_hook(database, authoritative_commit_hook);
        let worker = worker_database.map(|database| start_managed_worker(database, writer.clone()));
        let task_queue = worker
            .as_ref()
            .map(TaskWorkerHandle::sender)
            .unwrap_or_else(|| {
                let (sender, receiver) = tokio::sync::mpsc::channel(1);
                drop(receiver);
                sender
            });
        let watcher = watcher_database
            .map(|database| start_watcher(database, task_queue.clone(), writer.clone()));
        Ok(Arc::new(Self {
            library_uuid,
            runtime_generation,
            writer,
            notification_outbox_wake,
            notification_outbox_read_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::NOTIFICATION_OUTBOX_READ_CONCURRENCY,
            )),
            preview_decode_gate: Arc::new(tokio::sync::Semaphore::new(
                crate::api::PREVIEW_DECODE_CONCURRENCY,
            )),
            fsops_database,
            task_queue,
            read_database_path,
            read_database_cache: Arc::new(ReadDatabaseCache::default()),
            plugins_dir,
            root_reachability: Arc::new(Mutex::new(std::collections::HashMap::new())),
            root_runtime_registry: RootRuntimeRegistry::default(),
            worker,
            watcher: Mutex::new(watcher),
            cancellation,
            background_tasks: Mutex::new(Vec::new()),
            background_started: AtomicBool::new(false),
            runtime_lease: Mutex::new(runtime_lease),
        }))
    }

    #[cfg(test)]
    pub(crate) async fn stop_watcher_for_marker_fixture(&self) -> anyhow::Result<()> {
        let watcher = self
            .watcher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(mut watcher) = watcher {
            tokio::task::spawn_blocking(move || watcher.shutdown()).await??;
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.cancellation.cancel();
        // Requests may have detached blocking reads after an HTTP disconnect.
        // Fence checkout and drain their connections before retiring this
        // runtime, even if callers still retain an Arc to its resources.
        let read_database_cache = self.read_database_cache.clone();
        tokio::task::spawn_blocking(move || read_database_cache.close()).await?;
        let handles = self
            .background_tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain(..)
            .collect::<Vec<_>>();
        let mut failure = None;
        for handle in handles {
            if let Err(error) = handle.await {
                failure.get_or_insert_with(|| {
                    anyhow::anyhow!("runtime background task failed: {error}")
                });
            }
        }
        // Even a failed task must not detach the remaining owned handles.
        if let Some(error) = failure {
            return Err(error);
        }
        let watcher = self
            .watcher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(mut watcher) = watcher {
            tokio::task::spawn_blocking(move || watcher.shutdown()).await??;
        }
        if let Some(worker) = &self.worker {
            worker.shutdown().await?;
        }
        self.writer.shutdown().await?;
        if let Some(database) = &self.fsops_database {
            database
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .retire_file_connection()?;
        }
        self.runtime_lease
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        Ok(())
    }

    pub fn own_background_task(&self, handle: tokio::task::JoinHandle<()>) {
        self.background_tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(handle);
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub fn begin_background_ownership(&self) -> bool {
        !self.background_started.swap(true, Ordering::AcqRel)
    }
}

pub struct LibraryManager {
    storage_snapshot: crate::sync::file_policy::StorageSnapshotSource,
    decoder_manager: crate::plugins::decoder::DecoderManager,
    registry: Arc<HostRegistry>,
    active: Arc<RwLock<ActiveSnapshot>>,
    request_gate: Arc<tokio::sync::RwLock<()>>,
    operation: AsyncMutex<()>,
    root_relocation_operations: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    plugin_lifecycle_operations: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    runtime_generation: AtomicU64,
    pending_runtime_stop: Mutex<Option<Arc<LibraryRuntimeResources>>>,
    library_operations_closed: AtomicBool,
    libraries_dir: PathBuf,
    bundled_plugins_dir: PathBuf,
    secret_store: Option<Arc<dyn crate::plugins::secret_store::SecretStore>>,
    runtime_leases: RuntimeLeases,
    sync_coordinator: crate::sync::coordinator::SyncCoordinator,
    sync_scheduler: Mutex<Option<crate::sync::coordinator::SyncSchedulerHandle>>,
    root_relocation_scheduler: Mutex<Option<(CancellationToken, tokio::task::JoinHandle<()>)>>,
    plugin_runtime_scheduler: Mutex<Option<PluginRuntimeSchedulerHandle>>,
}

struct PluginRuntimeSchedulerHandle {
    cancellation: CancellationToken,
    commands: tokio::sync::mpsc::Sender<PluginRuntimeCommand>,
    task: tokio::task::JoinHandle<()>,
}

enum PluginRuntimeCommand {
    Stop {
        plugin_id: String,
        completed: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
}

type PluginSupervisorTask = (
    String,
    CancellationToken,
    tokio::task::JoinHandle<anyhow::Result<()>>,
);

async fn stop_plugin_supervisor(
    plugin_id: &str,
    running: &mut HashMap<String, PluginSupervisorTask>,
    stop_errors: &mut HashMap<String, String>,
    registry: &HostRegistry,
    providers: &crate::sync::provider::ProviderManager,
) -> Result<(), String> {
    let result = if let Some((_, cancel, handle)) = running.remove(plugin_id) {
        cancel.cancel();
        match handle.await {
            Ok(result) => result.map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        }
    } else if let Some(error) = stop_errors.get(plugin_id) {
        Err(error.clone())
    } else {
        if let Ok(Some(account)) = registry.plugin_account_secret(plugin_id) {
            let _ = providers.unregister(plugin_id, &account.account_uuid);
        }
        Ok(())
    };
    if let Err(error) = &result {
        stop_errors.insert(plugin_id.to_owned(), error.clone());
    }
    result
}

impl LibraryManager {
    fn root_relocation_operation_lock(&self, root_uuid: &str) -> Arc<AsyncMutex<()>> {
        Arc::clone(
            self.root_relocation_operations
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .entry(root_uuid.to_owned())
                .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
        )
    }

    pub fn host_registry(&self) -> &Arc<HostRegistry> {
        &self.registry
    }
    pub fn provider_manager(&self) -> crate::sync::provider::ProviderManager {
        self.sync_coordinator.provider_manager()
    }

    pub fn decoder_manager(&self) -> crate::plugins::decoder::DecoderManager {
        self.decoder_manager.clone()
    }

    pub fn reconcile_credential_retirements_once(
        &self,
        running_plugins: Option<&std::collections::HashSet<String>>,
    ) -> anyhow::Result<usize> {
        let Some(store) = self.secret_store.as_ref() else {
            return Ok(0);
        };
        crate::api::reconcile_credential_retirements(
            self.host_registry(),
            store,
            Some(&self.provider_manager()),
            running_plugins,
        )
    }

    pub async fn stop_plugin_runtime(&self, plugin_id: &str) -> anyhow::Result<()> {
        let epoch = self.registry.plugin_lifecycle_epoch(plugin_id)?;
        self.decoder_manager.close_plugin(plugin_id, epoch);
        let commands = self
            .plugin_runtime_scheduler
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|scheduler| scheduler.commands.clone());
        let Some(commands) = commands else {
            return self.decoder_manager.stop_plugin(plugin_id, epoch).await;
        };
        let (completed, joined) = tokio::sync::oneshot::channel();
        commands
            .send(PluginRuntimeCommand::Stop {
                plugin_id: plugin_id.to_owned(),
                completed,
            })
            .await
            .map_err(|_| anyhow::anyhow!("plugin runtime scheduler is unavailable"))?;
        let result = joined
            .await
            .map_err(|_| anyhow::anyhow!("plugin runtime stop acknowledgement was dropped"))?
            .map_err(anyhow::Error::msg);
        self.decoder_manager.stop_plugin(plugin_id, epoch).await?;
        result
    }

    pub async fn lock_plugin_lifecycle(&self, plugin_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let gate = self
            .plugin_lifecycle_operations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(plugin_id.to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        gate.lock_owned().await
    }

    /// Caller holds the plugin lifecycle lock, including across auth cancellation.
    pub async fn finish_disable_plugin(&self, plugin_id: &str) -> anyhow::Result<()> {
        let providers = self.provider_manager();
        providers.close_plugin(plugin_id, self.registry.plugin_lifecycle_epoch(plugin_id)?);
        self.stop_plugin_runtime(plugin_id).await?;
        let bindings = self.registry.plugin_binding_fences(plugin_id)?;
        self.fence_plugin_bindings(plugin_id, bindings).await?;
        providers.join_plugin_work(plugin_id).await;
        self.registry.finish_plugin_stop(plugin_id)?;
        Ok(())
    }

    /// Caller holds the lifecycle lock and has joined the account auth worker.
    pub async fn finish_plugin_restart(&self, plugin_id: &str) -> anyhow::Result<()> {
        let providers = self.provider_manager();
        providers.close_plugin(plugin_id, self.registry.plugin_lifecycle_epoch(plugin_id)?);
        self.registry.mark_plugin_stop_pending(plugin_id)?;
        self.stop_plugin_runtime(plugin_id).await?;
        self.fence_plugin_bindings(plugin_id, self.registry.plugin_binding_fences(plugin_id)?)
            .await?;
        providers.join_plugin_work(plugin_id).await;
        self.registry.finish_plugin_runtime_restart(plugin_id)?;
        Ok(())
    }

    pub fn wake_sync_after_local_mutation(&self) {
        if let Some(library_uuid) = self.active_snapshot().library_uuid {
            let _ = self.registry.wake_binding(&library_uuid);
        }
        self.sync_coordinator.wake();
    }

    pub fn wake_sync_for_foreground_or_network(&self) {
        let _ = self.registry.wake_all_sync_bindings();
        self.sync_coordinator.wake();
    }

    pub(crate) async fn write_sync_folder_catalog<T, F>(
        &self,
        library_uuid: &str,
        operation: F,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static,
    {
        let descriptor = self
            .registry
            .library(library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        verify_descriptor_identity(&descriptor)?;
        let active = self.active_snapshot();
        if active.library_uuid.as_deref() == Some(library_uuid) {
            let runtime = active
                .runtime
                .ok_or_else(|| anyhow::anyhow!("library-runtime-unavailable"))?;
            runtime.writer.write_authoritative(operation).await
        } else {
            let runtime = crate::sync::runtime::DormantSyncRuntime::new(
                library_uuid,
                descriptor.local_data_dir.join("catalog.sqlite"),
                self.runtime_leases.clone(),
            );
            runtime.run_turn(operation).await
        }
    }

    pub async fn sync_folder_snapshot(
        &self,
        library_uuid: &str,
        root_uuid: Uuid,
    ) -> anyhow::Result<crate::sync_folders::RootSnapshot> {
        self.write_sync_folder_catalog(library_uuid, move |database| {
            database.canonical_root_snapshot(root_uuid)
        })
        .await
    }

    pub async fn sync_folder_location_heads(
        &self,
        library_uuid: &str,
        root_uuid: Uuid,
    ) -> anyhow::Result<Vec<String>> {
        let read_heads = move |database: &mut Database| {
            Ok(database
                .sync_location_heads(root_uuid)?
                .into_iter()
                .map(|head| head.to_string())
                .collect())
        };
        let active = self.active_snapshot();
        if active.library_uuid.as_deref() == Some(library_uuid) {
            let runtime = active
                .runtime
                .ok_or_else(|| anyhow::anyhow!("library-runtime-unavailable"))?;
            // GET projection reads must not trigger the local-mutation wake.
            runtime.writer.write(read_heads).await
        } else {
            self.write_sync_folder_catalog(library_uuid, read_heads)
                .await
        }
    }

    pub async fn sync_folder_stop_work(
        &self,
        library_uuid: &str,
        root_uuid: Uuid,
    ) -> anyhow::Result<crate::sync_folders::RootStopWork> {
        self.write_sync_folder_catalog(library_uuid, move |database| {
            database.root_stop_work(root_uuid)
        })
        .await
    }

    pub async fn refresh_sync_folder_inventory(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<Vec<String>> {
        let descriptors = self
            .write_sync_folder_catalog(library_uuid, |database| database.sync_root_descriptors())
            .await?;
        let mut actionable = Vec::new();
        for descriptor in descriptors {
            let root_uuid = Uuid::from_bytes(descriptor.root_uuid).to_string();
            let checkpoint_uuid = None;
            if self.registry.upsert_remote_sync_root(
                library_uuid,
                &root_uuid,
                &Uuid::from_bytes(descriptor.root_folder_uuid).to_string(),
                &descriptor.requested_name,
                &descriptor.materialized_name,
                &crate::sync::media::hex_hash(&descriptor.materialization_fingerprint),
                descriptor.logical_revision,
                &descriptor.lifecycle,
                None,
                checkpoint_uuid,
            )? {
                if let Some(root) = self.registry.remote_sync_root(library_uuid, &root_uuid)? {
                    self.publish_sync_folder_placement_required(
                        library_uuid,
                        &root_uuid,
                        root.inventory_revision,
                        &root.requested_name,
                    )
                    .await?;
                }
                actionable.push(root_uuid);
            }
        }
        for (binding, descriptor) in self.registry.pending_automatic_root_renames(library_uuid)? {
            let registry = self.registry.clone();
            let binding_for_reconcile = binding.clone();
            let descriptor_for_reconcile = descriptor.clone();
            match tokio::task::spawn_blocking(move || {
                registry.reconcile_same_name_root_materialization(
                    &binding_for_reconcile,
                    &descriptor_for_reconcile,
                )
            })
            .await
            .context("root materialization reconciliation task failed")?
            {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) if error.to_string() == "operation-in-progress" => continue,
                Err(error) => return Err(error),
            }
            let target = Path::new(&binding.placement_parent).join(&descriptor.materialized_name);
            let operation_uuid = Uuid::now_v7().to_string();
            let resume_state = if binding.state == "paused" {
                "paused"
            } else {
                "active"
            };
            let mut request = serde_json::json!({
                "operationUuid": operation_uuid,
                "expectedOperationRevision": binding.operation_revision,
                "expectedBindingGeneration": binding.binding_generation,
                "mode": "automatic-rename",
                "targetPath": target.to_string_lossy(),
                "desiredMaterializationFingerprint": descriptor.materialization_fingerprint,
                "localBindingUuid": binding.local_binding_uuid,
                "resumeState": resume_state,
            });
            let marker = crate::sync_folders::SyncRootMarker {
                format_version: 1,
                library_uuid: library_uuid.to_owned(),
                root_uuid: descriptor.root_uuid.clone(),
                root_folder_uuid: descriptor.root_folder_uuid.clone(),
                local_binding_uuid: binding.local_binding_uuid.clone(),
                binding_generation: binding.binding_generation,
            };
            let other_paths = self
                .registry
                .local_root_bindings(library_uuid)?
                .into_iter()
                .filter(|candidate| candidate.root_uuid != descriptor.root_uuid)
                .map(|candidate| PathBuf::from(candidate.local_path))
                .collect::<Vec<_>>();
            let source = PathBuf::from(&binding.local_path);
            let placement_base = PathBuf::from(
                self.registry
                    .sync_folder_settings(library_uuid)?
                    .default_placement_base,
            );
            let target_for_preflight = target.clone();
            let marker_for_preflight = marker.clone();
            let evidence = match tokio::task::spawn_blocking(move || {
                crate::sync_folders::validate_relocation_paths(
                    &source,
                    &target_for_preflight,
                    &placement_base,
                    &other_paths,
                    &marker_for_preflight,
                )
            })
            .await
            .context("automatic rename preflight task failed")?
            {
                Ok(evidence) => evidence,
                Err(error) => {
                    tracing::warn!(root_uuid=%descriptor.root_uuid,%error,"automatic synchronized-folder rename preflight failed");
                    let retry_source = PathBuf::from(&binding.local_path);
                    let retry_target = target.clone();
                    let retry_marker = marker.clone();
                    if let Ok(Ok(retry_evidence)) = tokio::task::spawn_blocking(move || {
                        crate::sync_folders::capture_relocation_retry_evidence(
                            &retry_source,
                            &retry_target,
                            &retry_marker,
                        )
                    })
                    .await
                    {
                        request["pathEvidence"] = serde_json::to_value(retry_evidence)?;
                    }
                    let request_hash = crate::sync_folders::request_hash(&request)?;
                    self.registry
                        .record_automatic_root_rename_preflight_failure(
                            request["operationUuid"].as_str().expect("operation UUID"),
                            library_uuid,
                            &descriptor.root_uuid,
                            &request_hash,
                            &request,
                            binding.operation_revision,
                            binding.binding_generation,
                            &descriptor.materialization_fingerprint,
                            &target,
                        )?;
                    continue;
                }
            };
            let evidence_for_request = evidence.clone();
            request["targetPath"] = serde_json::json!(evidence.target_path);
            request["pathEvidence"] = serde_json::to_value(evidence_for_request)?;
            let request_hash = crate::sync_folders::request_hash(&request)?;
            let admission = self.operation.lock().await;
            self.admit_content_path(&target, true)?;
            let reservation = self.registry.begin_root_relocation(
                request["operationUuid"].as_str().expect("operation UUID"),
                library_uuid,
                &descriptor.root_uuid,
                "automatic-rename",
                &request_hash,
                &request,
                binding.operation_revision,
                binding.binding_generation,
                &descriptor.materialization_fingerprint,
                &evidence,
            );
            drop(admission);
            match reservation {
                Ok((relocation, _)) => {
                    let _ = self
                        .execute_root_relocation(&relocation.operation.operation_uuid, marker)
                        .await;
                }
                Err(error) if error.to_string().contains("operation-in-progress") => {}
                Err(error) => return Err(error),
            }
        }
        // Also repairs a previous Host commit interrupted before Catalog
        // projection. Workers continue to enforce the Catalog token fence.
        let coordinator = self.sync_coordinator.clone();
        let library_for_projection = library_uuid.to_owned();
        self.write_sync_folder_catalog(library_uuid, move |database| {
            coordinator.project_root_binding_tokens(&library_for_projection, database)
        })
        .await?;
        Ok(actionable)
    }

    pub async fn publish_sync_folder_placement_required(
        &self,
        library_uuid: &str,
        root_uuid: &str,
        revision: u64,
        logical_name: &str,
    ) -> anyhow::Result<()> {
        let now = crate::host_registry::unix_timestamp_millis_for_runtime();
        let mut notification = crate::db::notification_outbox::managed_root_placement_notification(
            library_uuid,
            root_uuid,
            revision,
            logical_name,
            now,
        );
        self.write_sync_folder_catalog(library_uuid, move |database| {
            database
                .publish_notification_operation(
                    &mut notification,
                    crate::db::notification_outbox::PublishMode::Phase,
                )
                .map(|_| ())
        })
        .await
    }

    pub async fn current_remote_root_snapshot(
        &self,
        library_uuid: &str,
        root_uuid: Uuid,
    ) -> anyhow::Result<(crate::sync_folders::RootSnapshot, Option<String>)> {
        self.sync_coordinator
            .fetch_remote_root_snapshot(library_uuid, root_uuid)
            .await
    }

    pub fn wake_sync_folder_workers(&self) {
        self.sync_coordinator.wake();
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn execute_root_relocation(
        &self,
        operation_uuid: &str,
        expected_marker: crate::sync_folders::SyncRootMarker,
    ) -> anyhow::Result<crate::host_registry::RootOperationRecord> {
        let relocation = self
            .registry
            .root_relocation_operation(operation_uuid)?
            .context("root-relocation-not-found")?;
        let root_lock = self.root_relocation_operation_lock(&relocation.operation.root_uuid);
        let _operation = root_lock.lock().await;
        self.execute_root_relocation_locked(operation_uuid, expected_marker)
            .await
    }

    async fn execute_root_relocation_locked(
        &self,
        operation_uuid: &str,
        expected_marker: crate::sync_folders::SyncRootMarker,
    ) -> anyhow::Result<crate::host_registry::RootOperationRecord> {
        let mut relocation = self
            .registry
            .root_relocation_operation(operation_uuid)?
            .ok_or_else(|| anyhow::anyhow!("root-relocation-not-found"))?;
        if relocation.operation.state == "completed" {
            return Ok(relocation.operation);
        }
        if !matches!(
            relocation.operation.state.as_str(),
            "pending" | "running" | "waiting"
        ) {
            anyhow::bail!("invalid-root-state");
        }
        let library_uuid = relocation.operation.library_uuid.clone();
        let root_uuid = Uuid::parse_str(&relocation.operation.root_uuid)?;
        let binding = self
            .registry
            .local_root_binding(&library_uuid, &relocation.operation.root_uuid)?
            .ok_or_else(|| anyhow::anyhow!("local-root-binding-not-found"))?;
        let local_binding_uuid = Uuid::parse_str(&binding.local_binding_uuid)?;
        let old_generation = relocation
            .operation
            .expected_binding_generation
            .context("stale-binding-generation")?;
        let generation = old_generation.saturating_add(1);
        let automatic_rename = relocation.operation.kind == "automatic-rename";
        if binding.local_binding_uuid != expected_marker.local_binding_uuid
            || binding.binding_generation != generation
        {
            anyhow::bail!("stale-binding-generation");
        }
        if !matches!(binding.state.as_str(), "relocating" | "reconcile-required") {
            anyhow::bail!("invalid-root-state");
        }
        let resume_state =
            crate::host_registry::relocation_resume_state(&relocation.operation.request_json);

        let result: anyhow::Result<crate::host_registry::RootOperationRecord> = async {
            if automatic_rename
                && !relocation.operation.cutover_committed
                && matches!(relocation.operation.phase.as_str(), "fenced" | "quiescing")
                && binding.desired_materialization_fingerprint
                    != relocation.desired_materialization_fingerprint
            {
                anyhow::bail!("stale-operation-revision");
            }
            if matches!(relocation.operation.phase.as_str(), "fenced" | "quiescing") {
                // Drain turns registered before the Catalog generation fence and
                // prevent a new turn from registering until that fence is durable.
                // This preserves unrelated-root workers without allowing a turn
                // that already opened the old root to outlive the fence.
                let media_registration_fence =
                    match self.registry.binding_transport(&library_uuid, "library")? {
                        Some(transport) => Some(
                            self.sync_coordinator
                                .fence_media_binding_registration(transport.binding_epoch)
                                .await?,
                        ),
                        None => None,
                    };
                let operation_revision = binding.operation_revision;
                self.write_sync_folder_catalog(&library_uuid, move |database| {
                    database.fence_sync_root_relocation(
                        root_uuid,
                        local_binding_uuid,
                        old_generation,
                        generation,
                        operation_revision,
                    )
                })
                .await?;
                let active = self.active_snapshot();
                if active.library_uuid.as_deref() == Some(library_uuid.as_str()) {
                    if let Some(runtime) = active.runtime {
                        runtime
                            .root_runtime_registry
                            .invalidate(&root_uuid.to_string());
                        if let Some(watcher) = runtime
                            .watcher
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .as_ref()
                        {
                            watcher.quiesce_path(Path::new(&relocation.source_path))?;
                        }
                    }
                }
                drop(media_registration_fence);
                relocation = self.registry.advance_root_relocation(
                    operation_uuid,
                    relocation.operation.operation_revision,
                    "transferring",
                    None,
                    None,
                    None,
                    0,
                    0,
                    false,
                )?;
            }

            if matches!(
                relocation.operation.phase.as_str(),
                "transferring" | "verifying"
            ) {
                let current_binding = self
                    .registry
                    .local_root_binding(&library_uuid, &relocation.operation.root_uuid)?
                    .context("local-root-binding-not-found")?;
                if automatic_rename
                    && relocation.operation.phase == "transferring"
                    && current_binding.desired_materialization_fingerprint
                        != relocation.desired_materialization_fingerprint
                    && !Path::new(&relocation.target_path).exists()
                {
                    anyhow::bail!("stale-operation-revision");
                }
                let evidence: crate::sync_folders::RelocationEvidence =
                    serde_json::from_str(&relocation.operation.request_json)
                        .ok()
                        .and_then(|value: serde_json::Value| value.get("pathEvidence").cloned())
                        .map(serde_json::from_value)
                        .transpose()?
                        .ok_or_else(|| anyhow::anyhow!("stale-plan"))?;
                let transfer_marker = expected_marker.clone();
                let transfer_operation_uuid = operation_uuid.to_owned();
                let transfer_evidence = evidence.clone();
                let transfer_checkpoint = self
                    .registry
                    .root_relocation_transfer_checkpoint(operation_uuid)?;
                let transfer = tokio::task::spawn_blocking(move || {
                    crate::sync_folders::transfer_relocation_tree(
                        &transfer_evidence,
                        &transfer_marker,
                        &transfer_operation_uuid,
                        transfer_checkpoint,
                    )
                })
                .await
                .context("relocation transfer task failed")??;
                if !transfer.complete {
                    let staging_path = transfer
                        .staging_path
                        .as_deref()
                        .context("transfer-failed")?;
                    relocation = self.registry.record_root_relocation_transfer_progress(
                        operation_uuid,
                        relocation.operation.operation_revision,
                        staging_path,
                        transfer.transferred_bytes,
                        &transfer.checkpoint,
                    )?;
                    return Ok(relocation.operation);
                }
                let target = Path::new(&evidence.target_path);
                let target_identity = crate::library_path::local_file_identity(target)?;
                let bytes = evidence.source_bytes;
                let verification_digest = transfer
                    .verification_digest
                    .as_deref()
                    .context("verification-failed")?;
                relocation = self.registry.advance_root_relocation(
                    operation_uuid,
                    relocation.operation.operation_revision,
                    "catalog-cutover",
                    transfer.staging_path.as_deref(),
                    Some(&target_identity),
                    Some(verification_digest),
                    bytes,
                    bytes,
                    false,
                )?;
            }

            if relocation.operation.phase == "catalog-cutover" {
                let _admission = self.operation.lock().await;
                self.admit_content_path(Path::new(&relocation.target_path), false)?;
                let target_path = relocation.target_path.clone();
                let desired_name = self
                    .registry
                    .local_root_binding(&library_uuid, &relocation.operation.root_uuid)?
                    .context("local-root-binding-not-found")?
                    .desired_materialized_name;
                let revision = relocation.operation.operation_revision.saturating_add(1);
                let cutover = self
                    .write_sync_folder_catalog(&library_uuid, move |database| {
                        database.cutover_sync_root_relocation(
                            root_uuid,
                            local_binding_uuid,
                            old_generation,
                            generation,
                            Path::new(&target_path),
                            &desired_name,
                            revision,
                        )
                    })
                    .await?;
                let active = self.active_snapshot();
                if active.library_uuid.as_deref() == Some(library_uuid.as_str()) {
                    if let Some(runtime) = active.runtime {
                        let _ = runtime.task_queue.send(cutover.scan_task_id).await;
                    }
                }
                relocation = self.registry.advance_root_relocation(
                    operation_uuid,
                    relocation.operation.operation_revision,
                    "host-cutover",
                    None,
                    None,
                    None,
                    0,
                    0,
                    true,
                )?;
            }

            if relocation.operation.phase == "host-cutover" {
                let _admission = self.operation.lock().await;
                self.admit_content_path(Path::new(&relocation.target_path), false)?;
                let target = Path::new(&relocation.target_path);
                let marker = crate::sync_folders::SyncRootMarker {
                    binding_generation: generation,
                    ..expected_marker.clone()
                };
                crate::sync_folders::replace_root_marker(target, &marker)?;
                let marker_identity = crate::library_path::local_file_identity(
                    &target.join(crate::sync_folders::ROOT_MARKER_FILE_NAME),
                )?;
                let filesystem_identity = crate::library_path::local_file_identity(target)?;
                let placement_parent = target.parent().context("path-unavailable")?;
                let descriptor = self
                    .registry
                    .remote_sync_root(&library_uuid, &relocation.operation.root_uuid)?
                    .context("root-not-found")?;
                relocation = self.registry.commit_root_relocation_host_cutover(
                    operation_uuid,
                    relocation.operation.operation_revision,
                    &binding.local_binding_uuid,
                    generation,
                    &relocation.target_path,
                    &relocation.target_canonical_key,
                    &placement_parent.to_string_lossy(),
                    &crate::sync_folders::canonical_path_key(placement_parent),
                    &marker_identity,
                    &filesystem_identity,
                    &descriptor.materialized_name,
                    &relocation.desired_materialization_fingerprint,
                )?;
                let active = self.active_snapshot();
                if active.library_uuid.as_deref() == Some(library_uuid.as_str()) {
                    if let Some(runtime) = active.runtime {
                        runtime.root_runtime_registry.install(
                            crate::sync::runtime::RootExecutionToken {
                                library_uuid: library_uuid.clone(),
                                root_uuid: relocation.operation.root_uuid.clone(),
                                local_binding_uuid: binding.local_binding_uuid.clone(),
                                binding_generation: generation,
                            },
                        );
                        if let Some(watcher) = runtime
                            .watcher
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .as_ref()
                        {
                            watcher.refresh()?;
                        }
                    }
                }
                if let Some(transport) =
                    self.registry.binding_transport(&library_uuid, "library")?
                {
                    self.sync_coordinator
                        .resume_media_binding(transport.binding_epoch)
                        .await;
                }
            }

            if relocation.operation.phase == "reconciling" {
                let status = self
                    .write_sync_folder_catalog(&library_uuid, move |database| {
                        database.sync_root_relocation_reconciliation_status(
                            root_uuid,
                            local_binding_uuid,
                            generation,
                        )
                    })
                    .await?;
                if !relocation_reconciliation_succeeded(status.as_deref())? {
                    return Ok(relocation.operation);
                }
                relocation = self.registry.advance_root_relocation(
                    operation_uuid,
                    relocation.operation.operation_revision,
                    "cleanup",
                    None,
                    None,
                    None,
                    0,
                    0,
                    true,
                )?;
            }

            if relocation.operation.phase == "cleanup" {
                let source = Path::new(&relocation.source_path);
                let target = Path::new(&relocation.target_path);
                let target_identity = relocation
                    .target_filesystem_identity
                    .as_deref()
                    .context("verification-failed")?;
                if crate::library_path::local_file_identity(target)
                    .ok()
                    .as_deref()
                    != Some(target_identity)
                {
                    anyhow::bail!("verification-failed");
                }
                let target_marker = crate::sync_folders::read_root_marker(target)?;
                let expected_target_marker = crate::sync_folders::SyncRootMarker {
                    binding_generation: generation,
                    ..expected_marker.clone()
                };
                if target_marker != expected_target_marker {
                    anyhow::bail!("marker-mismatch");
                }
                if source.exists() && source != target {
                    let expected_identity = relocation
                        .source_filesystem_identity
                        .as_deref()
                        .context("source-missing")?;
                    let evidence: crate::sync_folders::RelocationEvidence =
                        serde_json::from_str(&relocation.operation.request_json)
                            .ok()
                            .and_then(|value: serde_json::Value| value.get("pathEvidence").cloned())
                            .map(serde_json::from_value)
                            .transpose()?
                            .context("stale-plan")?;
                    let cleanup_source = source.to_path_buf();
                    let cleanup_identity = expected_identity.to_owned();
                    let cleanup_digest = evidence.source_digest;
                    let cleanup_marker = expected_marker.clone();
                    let cleanup = tokio::task::spawn_blocking(move || {
                        crate::sync_folders::remove_relocation_source_if_unchanged(
                            &cleanup_source,
                            &cleanup_identity,
                            &cleanup_digest,
                            &cleanup_marker,
                        )
                    })
                    .await
                    .context("relocation cleanup task failed")?;
                    if let Err(error) = cleanup {
                        if error.to_string().contains("cleanup-source-changed") {
                            return self.registry.fail_root_relocation_cleanup(
                                operation_uuid,
                                relocation.operation.operation_revision,
                                generation,
                                "cleanup-source-changed",
                            );
                        }
                        return Err(error);
                    }
                }
                let revision = relocation.operation.operation_revision.saturating_add(1);
                self.write_sync_folder_catalog(&library_uuid, move |database| {
                    database.complete_sync_root_relocation_projection(
                        root_uuid,
                        local_binding_uuid,
                        generation,
                        revision,
                        resume_state,
                    )
                })
                .await?;
                let operation = self.registry.complete_root_relocation(
                    operation_uuid,
                    relocation.operation.operation_revision,
                    generation,
                )?;
                self.refresh_active_root_runtime(&library_uuid)?;
                return Ok(operation);
            }
            Ok(relocation.operation)
        }
        .await;
        match result {
            Ok(operation) => Ok(operation),
            Err(error) => {
                let message = error.to_string();
                let phase = if automatic_rename {
                    "local-rename-required"
                } else if message.contains("source-missing") {
                    "source-missing"
                } else if message.contains("target-conflict")
                    || message.contains("destination-exists")
                {
                    "target-conflict"
                } else if message.contains("verification") {
                    "verification-failed"
                } else {
                    "transfer-failed"
                };
                let failure_revision = self
                    .registry
                    .root_operation(operation_uuid)
                    .ok()
                    .flatten()
                    .map(|operation| operation.operation_revision)
                    .unwrap_or_default();
                let _ = self.registry.fail_root_relocation(
                    operation_uuid,
                    failure_revision,
                    phase,
                    phase,
                );
                let cutover_committed = self
                    .registry
                    .root_operation(operation_uuid)
                    .ok()
                    .flatten()
                    .is_some_and(|operation| operation.cutover_committed);
                let target_path = self
                    .registry
                    .root_relocation_operation(operation_uuid)
                    .ok()
                    .flatten()
                    .map(|operation| operation.target_path);
                let catalog_cutover_committed = if let Some(target_path) = target_path {
                    match self
                        .write_sync_folder_catalog(&library_uuid, move |database| {
                            database.sync_root_relocation_catalog_cutover_is_current(
                                root_uuid,
                                local_binding_uuid,
                                generation,
                                Path::new(&target_path),
                            )
                        })
                        .await
                    {
                        Ok(committed) => committed,
                        Err(inspect_error) => {
                            tracing::warn!(
                                operation_uuid = %operation_uuid,
                                error = %inspect_error,
                                "could not inspect relocation catalog cutover; preserving fenced state"
                            );
                            let _ = self.refresh_active_root_runtime(&library_uuid);
                            return Err(error);
                        }
                    }
                } else {
                    false
                };
                if catalog_cutover_committed && !cutover_committed {
                    if let Ok(Some(operation)) = self.registry.root_operation(operation_uuid) {
                        if operation.state == "failed" {
                            let _ = self.registry.repair_root_relocation_after_catalog_cutover(
                                operation_uuid,
                                operation.operation_revision,
                            );
                        }
                    }
                    let _ = self.refresh_active_root_runtime(&library_uuid);
                    return Err(error);
                }
                let source_still_authoritative = self
                    .registry
                    .root_relocation_operation(operation_uuid)
                    .ok()
                    .flatten()
                    .is_some_and(|operation| {
                        let source = Path::new(&operation.source_path);
                        source.is_dir()
                            && operation.source_filesystem_identity.as_deref()
                                == crate::library_path::local_file_identity(source)
                                    .ok()
                                    .as_deref()
                    });
                if !cutover_committed && source_still_authoritative {
                    let revision = binding.operation_revision.saturating_add(2);
                    let rollback_state = if automatic_rename {
                        "local-rename-required"
                    } else {
                        resume_state
                    };
                    let repaired = self
                        .write_sync_folder_catalog(&library_uuid, move |database| {
                            database.abort_sync_root_relocation_projection(
                                root_uuid,
                                local_binding_uuid,
                                generation,
                                revision,
                                rollback_state,
                            )
                        })
                        .await
                        .is_ok();
                    if repaired {
                        let _ = self
                            .registry
                            .mark_root_relocation_projection_repaired(operation_uuid);
                    }
                    let _ = self.refresh_active_root_runtime(&library_uuid);
                }
                Err(error)
            }
        }
    }

    pub async fn cancel_root_relocation(
        &self,
        operation_uuid: &str,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
    ) -> anyhow::Result<crate::host_registry::RootOperationRecord> {
        let relocation = self
            .registry
            .root_relocation_operation(operation_uuid)?
            .context("root-relocation-not-found")?;
        let root_lock = self.root_relocation_operation_lock(&relocation.operation.root_uuid);
        let _operation = root_lock.lock().await;
        if relocation.operation.cutover_committed {
            anyhow::bail!("cutover-committed");
        }
        let binding = self
            .registry
            .local_root_binding(
                &relocation.operation.library_uuid,
                &relocation.operation.root_uuid,
            )?
            .context("local-root-binding-not-found")?;
        if binding.binding_generation != expected_binding_generation {
            anyhow::bail!("stale-binding-generation");
        }
        if relocation.operation.state == "cancelled"
            && relocation.operation.operation_revision
                == expected_operation_revision.saturating_add(1)
        {
            let root_uuid = Uuid::parse_str(&relocation.operation.root_uuid)?;
            let local_binding_uuid = Uuid::parse_str(&binding.local_binding_uuid)?;
            let resume_state =
                crate::host_registry::relocation_resume_state(&relocation.operation.request_json);
            let revision = binding.operation_revision;
            self.write_sync_folder_catalog(&relocation.operation.library_uuid, move |database| {
                database.abort_sync_root_relocation_projection(
                    root_uuid,
                    local_binding_uuid,
                    expected_binding_generation,
                    revision,
                    resume_state,
                )
            })
            .await?;
            self.registry
                .mark_root_relocation_projection_repaired(operation_uuid)?;
            self.refresh_active_root_runtime(&relocation.operation.library_uuid)?;
            return Ok(relocation.operation);
        }
        let source = Path::new(&relocation.source_path);
        if Path::new(&relocation.target_path).exists() {
            anyhow::bail!("cutover-committed");
        }
        if !source.is_dir()
            || relocation.source_filesystem_identity.as_deref()
                != crate::library_path::local_file_identity(source)
                    .ok()
                    .as_deref()
        {
            anyhow::bail!("source-missing");
        }
        let descriptor = self
            .registry
            .remote_sync_root(
                &relocation.operation.library_uuid,
                &relocation.operation.root_uuid,
            )?
            .context("root-not-found")?;
        let expected_marker = crate::sync_folders::SyncRootMarker {
            format_version: 1,
            library_uuid: relocation.operation.library_uuid.clone(),
            root_uuid: relocation.operation.root_uuid.clone(),
            root_folder_uuid: descriptor.root_folder_uuid,
            local_binding_uuid: binding.local_binding_uuid.clone(),
            binding_generation: relocation
                .operation
                .expected_binding_generation
                .context("stale-binding-generation")?,
        };
        if crate::sync_folders::read_root_marker(source)? != expected_marker {
            anyhow::bail!("marker-mismatch");
        }
        let operation = self.registry.cancel_root_relocation(
            operation_uuid,
            expected_operation_revision,
            expected_binding_generation,
        )?;
        let root_uuid = Uuid::parse_str(&operation.root_uuid)?;
        let local_binding_uuid = Uuid::parse_str(&binding.local_binding_uuid)?;
        let resume_state = crate::host_registry::relocation_resume_state(&operation.request_json);
        let revision = binding.operation_revision.saturating_add(1);
        self.write_sync_folder_catalog(&operation.library_uuid, move |database| {
            database.abort_sync_root_relocation_projection(
                root_uuid,
                local_binding_uuid,
                expected_binding_generation,
                revision,
                resume_state,
            )
        })
        .await?;
        self.registry
            .mark_root_relocation_projection_repaired(operation_uuid)?;
        self.refresh_active_root_runtime(&operation.library_uuid)?;
        Ok(operation)
    }

    pub async fn retry_root_relocation(
        &self,
        operation_uuid: &str,
        expected_operation_revision: u64,
        expected_binding_generation: u64,
    ) -> anyhow::Result<crate::host_registry::RootOperationRecord> {
        let relocation = self
            .registry
            .root_relocation_operation(operation_uuid)?
            .context("root-relocation-not-found")?;
        let root_lock = self.root_relocation_operation_lock(&relocation.operation.root_uuid);
        let _operation = root_lock.lock().await;
        let binding = self
            .registry
            .local_root_binding(
                &relocation.operation.library_uuid,
                &relocation.operation.root_uuid,
            )?
            .context("local-root-binding-not-found")?;
        if binding.binding_generation != expected_binding_generation {
            anyhow::bail!("stale-binding-generation");
        }
        if relocation.operation.state == "completed" {
            return Ok(relocation.operation);
        }
        if relocation.operation.kind == "automatic-rename"
            && !relocation.operation.cutover_committed
            && binding.desired_materialization_fingerprint
                != relocation.desired_materialization_fingerprint
        {
            anyhow::bail!("stale-operation-revision");
        }
        let descriptor = self
            .registry
            .remote_sync_root(
                &relocation.operation.library_uuid,
                &relocation.operation.root_uuid,
            )?
            .context("root-not-found")?;
        let marker = crate::sync_folders::SyncRootMarker {
            format_version: 1,
            library_uuid: relocation.operation.library_uuid.clone(),
            root_uuid: relocation.operation.root_uuid.clone(),
            root_folder_uuid: descriptor.root_folder_uuid,
            local_binding_uuid: binding.local_binding_uuid.clone(),
            binding_generation: relocation
                .operation
                .expected_binding_generation
                .context("stale-binding-generation")?,
        };
        let source_path = PathBuf::from(&relocation.source_path);
        let target_path = PathBuf::from(&relocation.target_path);
        if !relocation.operation.cutover_committed && source_path.is_dir() && !target_path.exists()
        {
            let placement_base = PathBuf::from(
                self.registry
                    .sync_folder_settings(&relocation.operation.library_uuid)?
                    .default_placement_base,
            );
            let other_paths = self
                .registry
                .local_root_bindings(&relocation.operation.library_uuid)?
                .into_iter()
                .filter(|candidate| candidate.root_uuid != relocation.operation.root_uuid)
                .map(|candidate| PathBuf::from(candidate.local_path))
                .collect::<Vec<_>>();
            let marker_for_preflight = marker.clone();
            tokio::task::spawn_blocking(move || {
                crate::sync_folders::validate_relocation_paths(
                    &source_path,
                    &target_path,
                    &placement_base,
                    &other_paths,
                    &marker_for_preflight,
                )
            })
            .await
            .context("root relocation retry preflight task failed")??;
        }
        let authoritative_path = if relocation.operation.cutover_committed {
            Path::new(&relocation.target_path)
        } else if Path::new(&relocation.source_path).is_dir() {
            Path::new(&relocation.source_path)
        } else {
            Path::new(&relocation.target_path)
        };
        let actual_marker = crate::sync_folders::read_root_marker(authoritative_path)?;
        if actual_marker.library_uuid != marker.library_uuid
            || actual_marker.root_uuid != marker.root_uuid
            || actual_marker.root_folder_uuid != marker.root_folder_uuid
            || actual_marker.local_binding_uuid != marker.local_binding_uuid
            || !matches!(
                actual_marker.binding_generation,
                value if value == marker.binding_generation
                    || value == marker.binding_generation.saturating_add(1)
            )
        {
            anyhow::bail!("marker-mismatch");
        }
        self.registry.retry_root_relocation(
            operation_uuid,
            expected_operation_revision,
            expected_binding_generation,
        )?;
        self.execute_root_relocation_locked(operation_uuid, marker)
            .await
    }

    pub fn refresh_active_root_runtime(&self, library_uuid: &str) -> anyhow::Result<()> {
        let active = self.active_snapshot();
        if active.library_uuid.as_deref() != Some(library_uuid) {
            return Ok(());
        }
        if let Some(runtime) = active.runtime {
            runtime
                .root_runtime_registry
                .replace_all(self.registry.local_root_binding_tokens(library_uuid)?);
            if let Some(watcher) = runtime
                .watcher
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
            {
                watcher.refresh()?;
            }
        }
        Ok(())
    }

    pub async fn resume_pending_root_relocations(&self) -> anyhow::Result<usize> {
        let operations = self.registry.pending_root_relocations()?;
        let mut resumed = 0;
        for relocation in operations {
            let Some(library) = self.registry.library(&relocation.operation.library_uuid)? else {
                continue;
            };
            if let Err(error) = verify_descriptor_identity(&library) {
                self.registry
                    .isolate_unavailable_library(&library.library_uuid)?;
                tracing::warn!(%error, library_uuid = %library.library_uuid, "unsupported relocation preserved");
                continue;
            }
            let binding = self
                .registry
                .local_root_binding(
                    &relocation.operation.library_uuid,
                    &relocation.operation.root_uuid,
                )?
                .context("local-root-binding-not-found")?;
            let descriptor = self
                .registry
                .remote_sync_root(
                    &relocation.operation.library_uuid,
                    &relocation.operation.root_uuid,
                )?
                .context("root-not-found")?;
            let marker = crate::sync_folders::SyncRootMarker {
                format_version: 1,
                library_uuid: relocation.operation.library_uuid.clone(),
                root_uuid: relocation.operation.root_uuid.clone(),
                root_folder_uuid: descriptor.root_folder_uuid,
                local_binding_uuid: binding.local_binding_uuid.clone(),
                binding_generation: relocation
                    .operation
                    .expected_binding_generation
                    .unwrap_or(binding.binding_generation),
            };
            if matches!(relocation.operation.state.as_str(), "cancelled" | "failed") {
                let source = Path::new(&relocation.source_path);
                let root_uuid = Uuid::parse_str(&relocation.operation.root_uuid)?;
                let local_binding_uuid = Uuid::parse_str(&binding.local_binding_uuid)?;
                let generation = binding.binding_generation;
                let catalog_binding = self
                    .write_sync_folder_catalog(
                        &relocation.operation.library_uuid,
                        move |database| database.sync_root_local_binding(root_uuid.as_bytes()),
                    )
                    .await?;
                if relocation.operation.state == "failed"
                    && catalog_binding.as_ref().is_some_and(|candidate| {
                        candidate.local_binding_uuid == *local_binding_uuid.as_bytes()
                            && candidate.binding_generation == generation
                            && candidate.state == "reconcile-required"
                    })
                {
                    let repaired = if relocation.operation.cutover_committed {
                        self.registry.retry_root_relocation(
                            &relocation.operation.operation_uuid,
                            relocation.operation.operation_revision,
                            generation,
                        )?
                    } else {
                        self.registry.repair_root_relocation_after_catalog_cutover(
                            &relocation.operation.operation_uuid,
                            relocation.operation.operation_revision,
                        )?
                    };
                    let _ = self
                        .execute_root_relocation(&repaired.operation.operation_uuid, marker)
                        .await;
                    resumed += 1;
                    continue;
                }
                let source_is_authoritative = source.is_dir()
                    && relocation.source_filesystem_identity.as_deref()
                        == crate::library_path::local_file_identity(source)
                            .ok()
                            .as_deref();
                if relocation.operation.state == "failed" && !source_is_authoritative {
                    if catalog_binding.as_ref().is_some_and(|candidate| {
                        candidate.local_binding_uuid == *local_binding_uuid.as_bytes()
                            && candidate.binding_generation == generation
                            && candidate.state == "relocating"
                    }) {
                        let retried = self.registry.retry_root_relocation(
                            &relocation.operation.operation_uuid,
                            relocation.operation.operation_revision,
                            generation,
                        )?;
                        let _ = self
                            .execute_root_relocation(&retried.operation.operation_uuid, marker)
                            .await;
                        resumed += 1;
                    }
                    continue;
                }
                let revision = binding.operation_revision;
                let resume_state = if relocation.operation.kind == "automatic-rename" {
                    "local-rename-required"
                } else {
                    crate::host_registry::relocation_resume_state(
                        &relocation.operation.request_json,
                    )
                };
                self.write_sync_folder_catalog(
                    &relocation.operation.library_uuid,
                    move |database| {
                        database.abort_sync_root_relocation_projection(
                            root_uuid,
                            local_binding_uuid,
                            generation,
                            revision,
                            resume_state,
                        )
                    },
                )
                .await?;
                self.registry.mark_root_relocation_projection_repaired(
                    &relocation.operation.operation_uuid,
                )?;
                resumed += 1;
                continue;
            }
            let _ = self
                .execute_root_relocation(&relocation.operation.operation_uuid, marker)
                .await;
            resumed += 1;
        }
        Ok(resumed)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn ensure_selected_sync_root(
        &self,
        library_uuid: &str,
        root_uuid: Uuid,
        root_folder_uuid: Uuid,
        path: String,
        requested_name: String,
        materialized_name: String,
        materialization_fingerprint: [u8; 32],
        local_binding_uuid: Uuid,
        root_capture_epoch: Uuid,
        root_actor_epoch: Uuid,
        binding_generation: u64,
        state: String,
        operation_revision: u64,
        enqueue_scan: bool,
    ) -> anyhow::Result<Option<i64>> {
        let descriptor = self
            .registry
            .library(library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        let library_uuid_parsed = Uuid::parse_str(library_uuid)?;
        let library_root_uuid = Uuid::parse_str(&descriptor.root_uuid)?;
        let task = self
            .write_sync_folder_catalog(library_uuid, move |database| {
                database.ensure_selected_sync_root(
                    library_uuid_parsed,
                    root_uuid,
                    root_folder_uuid,
                    library_root_uuid,
                    &path,
                    &requested_name,
                    &materialized_name,
                    materialization_fingerprint,
                    local_binding_uuid,
                    root_capture_epoch,
                    root_actor_epoch,
                    binding_generation,
                    &state,
                    operation_revision,
                    enqueue_scan,
                )
            })
            .await?;
        self.refresh_active_root_runtime(library_uuid)?;
        if let Some(task_id) = task {
            let active = self.active_snapshot();
            if active.library_uuid.as_deref() == Some(library_uuid) {
                if let Some(runtime) = active.runtime {
                    let _ = runtime.task_queue.send(task_id).await;
                }
            }
        }
        Ok(task)
    }

    pub async fn sync_root_identity_for_path(
        &self,
        library_uuid: &str,
        path: String,
    ) -> anyhow::Result<Option<(Uuid, Uuid)>> {
        self.write_sync_folder_catalog(library_uuid, move |database| {
            database.sync_root_identity_for_path(&path)
        })
        .await
    }

    pub async fn other_library_sync_root_identity_for_path(
        &self,
        library_uuid: &str,
        path: String,
    ) -> anyhow::Result<Option<(String, Uuid, Uuid)>> {
        for descriptor in self.registry.snapshot()?.libraries {
            if descriptor.library_uuid == library_uuid {
                continue;
            }
            if let Some((root_uuid, root_folder_uuid)) = self
                .sync_root_identity_for_path(&descriptor.library_uuid, path.clone())
                .await?
            {
                return Ok(Some((descriptor.library_uuid, root_uuid, root_folder_uuid)));
            }
        }
        Ok(None)
    }

    pub async fn fence_sync_root_local_only(
        &self,
        library_uuid: &str,
        root_uuid: Uuid,
        local_binding_uuid: Uuid,
        old_generation: u64,
        new_generation: u64,
        root_capture_epoch: Uuid,
        root_actor_epoch: Uuid,
        operation_revision: u64,
    ) -> anyhow::Result<()> {
        self.write_sync_folder_catalog(library_uuid, move |database| {
            database.fence_root_as_local_only(
                root_uuid,
                local_binding_uuid,
                old_generation,
                new_generation,
                root_capture_epoch,
                root_actor_epoch,
                operation_revision,
            )
        })
        .await?;
        let active = self.active_snapshot();
        if active.library_uuid.as_deref() == Some(library_uuid) {
            if let Some(runtime) = active.runtime {
                runtime
                    .root_runtime_registry
                    .replace_all(self.registry.local_root_binding_tokens(library_uuid)?);
            }
        }
        Ok(())
    }

    pub async fn recover_root_reconciliation(
        &self,
        library_uuid: &str,
        root_uuid: &str,
    ) -> anyhow::Result<Option<crate::host_registry::RootOperationRecord>> {
        let work = self
            .registry
            .pending_root_reconciliation_work(library_uuid, root_uuid)?;
        if work.is_empty() {
            let binding = self.registry.local_root_binding(library_uuid, root_uuid)?;
            if binding
                .as_ref()
                .is_none_or(|binding| binding.state != "reconcile-required")
            {
                return Ok(None);
            }
        }
        for item in &work {
            let plan_uuid = Uuid::parse_str(&item.plan_uuid)?;
            let item_ordinal = item.item_ordinal;
            let root_uuid_value = Uuid::parse_str(root_uuid)?;
            let binding_uuid = Uuid::parse_str(&item.local_binding_uuid)?;
            let entity_uuid = Uuid::parse_str(&item.entity_uuid)?;
            let field = item.field_name.clone();
            let resolution = item.resolution.clone();
            let selected = item.selected_value.clone();
            let selected_entity = item.selected_entity.clone();
            let generation = item.binding_generation;
            let result = self
                .write_sync_folder_catalog(library_uuid, move |database| {
                    database.apply_root_reconciliation_item(
                        plan_uuid,
                        item_ordinal,
                        root_uuid_value,
                        binding_uuid,
                        generation,
                        entity_uuid,
                        &field,
                        &resolution,
                        selected.as_deref(),
                        selected_entity.as_deref(),
                    )
                })
                .await;
            match result {
                Ok(()) => self
                    .registry
                    .complete_root_reconciliation_item(&item.plan_uuid, item.item_ordinal)?,
                Err(error) => {
                    self.registry.fail_root_reconciliation_item(
                        &item.plan_uuid,
                        item.item_ordinal,
                        "catalog-apply-failed",
                    )?;
                    return Err(error);
                }
            }
        }
        let plan_uuid = work
            .first()
            .map(|item| item.plan_uuid.clone())
            .or_else(|| {
                self.registry
                    .pending_reenable_plan_uuid(library_uuid, root_uuid)
                    .ok()
                    .flatten()
            })
            .context("comparison-plan-not-found")?;
        let binding = self
            .registry
            .local_root_binding(library_uuid, root_uuid)?
            .context("local-root-binding-not-found")?;
        let root_uuid_value = Uuid::parse_str(root_uuid)?;
        let binding_uuid = Uuid::parse_str(&binding.local_binding_uuid)?;
        let revision = binding.operation_revision.saturating_add(1);
        self.write_sync_folder_catalog(library_uuid, move |database| {
            database.complete_root_reconciliation_projection(
                root_uuid_value,
                binding_uuid,
                binding.binding_generation,
                revision,
            )
        })
        .await?;
        let operation =
            self.registry
                .complete_root_reconciliation(library_uuid, root_uuid, &plan_uuid)?;
        self.sync_coordinator.wake();
        Ok(Some(operation))
    }

    pub async fn recover_publish_as_new(
        &self,
        operation_uuid: &str,
    ) -> anyhow::Result<crate::host_registry::RootOperationRecord> {
        let publish = self
            .registry
            .publish_as_new(operation_uuid)?
            .context("publish-as-new-not-found")?;
        let descriptor = self
            .registry
            .remote_sync_root(&publish.library_uuid, &publish.new_root_uuid)?
            .context("root-not-found")?;
        let binding = self
            .registry
            .local_root_binding(&publish.library_uuid, &publish.new_root_uuid)?
            .context("local-root-binding-not-found")?;
        let operation = Uuid::parse_str(&publish.operation_uuid)?;
        let library = Uuid::parse_str(&publish.library_uuid)?;
        let source_root = Uuid::parse_str(&publish.source_root_uuid)?;
        let new_root = Uuid::parse_str(&publish.new_root_uuid)?;
        let new_root_folder = Uuid::parse_str(&publish.new_root_folder_uuid)?;
        let local_binding = Uuid::parse_str(&publish.new_local_binding_uuid)?;
        let capture_epoch = Uuid::parse_str(&publish.root_capture_epoch)?;
        let actor_epoch = Uuid::parse_str(&publish.root_actor_epoch)?;
        let requested_name = descriptor.requested_name.clone();
        let materialized_name = descriptor.materialized_name.clone();
        let fingerprint = crate::sync::media::parse_hash(&descriptor.materialization_fingerprint)?;
        self.write_sync_folder_catalog(&publish.library_uuid, move |database| {
            database.publish_root_as_new(
                operation,
                library,
                source_root,
                new_root,
                new_root_folder,
                &requested_name,
                &materialized_name,
                fingerprint,
                local_binding,
                capture_epoch,
                actor_epoch,
                binding.operation_revision,
            )
        })
        .await?;
        let marker = crate::sync_folders::SyncRootMarker {
            format_version: 1,
            library_uuid: publish.library_uuid.clone(),
            root_uuid: publish.new_root_uuid.clone(),
            root_folder_uuid: publish.new_root_folder_uuid.clone(),
            local_binding_uuid: publish.new_local_binding_uuid.clone(),
            binding_generation: binding.binding_generation,
        };
        crate::sync_folders::replace_root_marker_for_publish(
            Path::new(&binding.local_path),
            &publish.source_root_uuid,
            &marker,
        )?;
        let marker_identity = crate::library_path::local_file_identity(
            &Path::new(&binding.local_path).join(crate::sync_folders::ROOT_MARKER_FILE_NAME),
        )?;
        self.registry.update_local_root_marker_identity(
            &publish.library_uuid,
            &publish.new_root_uuid,
            &publish.new_local_binding_uuid,
            binding.binding_generation,
            &marker_identity,
        )?;
        let completed = self.registry.complete_publish_as_new(operation_uuid)?;
        let active = self.active_snapshot();
        if active.library_uuid.as_deref() == Some(&publish.library_uuid) {
            if let Some(runtime) = active.runtime {
                runtime.root_runtime_registry.replace_all(
                    self.registry
                        .local_root_binding_tokens(&publish.library_uuid)?,
                );
            }
        }
        self.sync_coordinator.wake();
        Ok(completed)
    }

    pub async fn publish_sync_operation_notification(
        &self,
        operation: &crate::host_registry::SyncOperation,
    ) -> anyhow::Result<()> {
        let Some(library_uuid) = operation.library_uuid.as_deref() else {
            return Ok(());
        };
        let descriptor = self
            .registry
            .library(library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        let terminal = matches!(
            operation.state.as_str(),
            "complete" | "failed" | "cancelled"
        );
        let status = match operation.state.as_str() {
            "complete" => "succeeded",
            "failed" => "failed",
            "cancelled" => "cancelled",
            "pending" | "waiting" => "pending",
            _ => "running",
        };
        let mut notification = crate::db::notification_outbox::NotificationOperation {
            operation_id: format!("sync:{}", operation.operation_uuid),
            revision: 0,
            kind: operation.kind.clone(),
            category: "sync".to_owned(),
            origin: "user".to_owned(),
            presentation: "important".to_owned(),
            status: status.to_owned(),
            phase: Some(operation.phase.clone()),
            title: format!("{} · 资源库同步", descriptor.name),
            summary: operation
                .error_code
                .clone()
                .or_else(|| Some(operation.phase.clone())),
            task_id: None,
            root_id: None,
            library_uuid: operation.library_uuid.clone(),
            root_uuid: None,
            managed_root_action: None,
            folder_id: None,
            file_id: None,
            current_value: 0,
            total_value: None,
            unit: None,
            error_code: operation.error_code.clone(),
            error_detail: None,
            cancellable: operation.kind == "restore" && !terminal,
            retryable: operation.state == "failed",
            attempt: 1,
            occurrence_count: 1,
            started_at: operation.updated_at,
            updated_at: operation.updated_at,
            completed_at: terminal.then_some(operation.updated_at),
        };
        let mode = if terminal {
            crate::db::notification_outbox::PublishMode::Terminal
        } else {
            crate::db::notification_outbox::PublishMode::Phase
        };
        let active = self.active_snapshot();
        if active.library_uuid.as_deref() == Some(library_uuid) {
            let runtime = active
                .runtime
                .ok_or_else(|| anyhow::anyhow!("library-runtime-unavailable"))?;
            runtime
                .writer
                .write_authoritative(move |database| {
                    database
                        .publish_notification_operation(&mut notification, mode)
                        .map(|_| ())
                })
                .await
        } else {
            let database = Database::open(descriptor.local_data_dir.join("catalog.sqlite"))?;
            database.apply_migrations()?;
            let writer = DbWriter::spawn(database);
            let result = writer
                .write_authoritative(move |database| {
                    database
                        .publish_notification_operation(&mut notification, mode)
                        .map(|_| ())
                })
                .await;
            let shutdown = writer.shutdown().await;
            result?;
            shutdown
        }
    }

    pub async fn publish_restore_notification(
        &self,
        plan: &crate::host_registry::SyncRestorePlan,
    ) -> anyhow::Result<()> {
        let descriptor = self
            .registry
            .library(&plan.library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        let catalog_path = descriptor.local_data_dir.join("catalog.sqlite");
        if !catalog_path.is_file() {
            return Ok(());
        }
        let terminal = matches!(plan.state.as_str(), "complete" | "failed" | "cancelled");
        let status = match plan.state.as_str() {
            "complete" => "succeeded",
            "failed" => "failed",
            "cancelled" => "cancelled",
            "paused" => "pending",
            _ => "running",
        };
        let mut notification = crate::db::notification_outbox::NotificationOperation {
            operation_id: format!("sync:{}:{}", plan.operation_uuid, plan.library_uuid),
            revision: 0,
            kind: "restore".to_owned(),
            category: "sync".to_owned(),
            origin: "user".to_owned(),
            presentation: "important".to_owned(),
            status: status.to_owned(),
            phase: Some(plan.phase.clone()),
            title: format!("{} · 资源库恢复", descriptor.name),
            summary: plan.error_code.clone().or_else(|| Some(plan.phase.clone())),
            task_id: None,
            root_id: None,
            library_uuid: Some(plan.library_uuid.clone()),
            root_uuid: None,
            managed_root_action: None,
            folder_id: None,
            file_id: None,
            current_value: i64::try_from(plan.media_items_done).unwrap_or(i64::MAX),
            total_value: plan
                .media_items_total
                .map(|value| i64::try_from(value).unwrap_or(i64::MAX)),
            unit: Some("items".to_owned()),
            error_code: plan.error_code.clone(),
            error_detail: None,
            cancellable: !terminal && !plan.catalog_ready,
            retryable: plan.state == "failed",
            attempt: 1,
            occurrence_count: 1,
            started_at: plan.updated_at,
            updated_at: plan.updated_at,
            completed_at: terminal.then_some(plan.updated_at),
        };
        let mode = if terminal {
            crate::db::notification_outbox::PublishMode::Terminal
        } else {
            crate::db::notification_outbox::PublishMode::Phase
        };
        let database = Database::open(catalog_path)?;
        database.apply_migrations()?;
        let writer = DbWriter::spawn(database);
        let result = writer
            .write_authoritative(move |database| {
                database
                    .publish_notification_operation(&mut notification, mode)
                    .map(|_| ())
            })
            .await;
        let shutdown = writer.shutdown().await;
        result?;
        shutdown
    }

    pub async fn create_reconnect_plan(
        &self,
        library_uuid: &str,
        account_uuid: &str,
        remote_library_uuid: &str,
    ) -> anyhow::Result<crate::host_registry::SyncReconnectPlan> {
        let descriptor = self
            .registry
            .library(library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        let database = Database::open(descriptor.local_data_dir.join("catalog.sqlite"))?;
        database.apply_migrations()?;
        let connection = database.connection_for_fsops();
        let next_generation: i64 = connection.query_row(
            "SELECT next_local_mutation_generation FROM sync_local_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let closed_generation: i64 = connection.query_row(
            "SELECT COALESCE(MAX(e.local_mutation_generation),0) FROM sync_events e JOIN sync_actor_epochs a ON a.actor_epoch=e.actor_epoch WHERE a.state IN ('closed','archived','forked')",
            [],
            |row| row.get(0),
        )?;
        let local_divergence_count = next_generation
            .saturating_sub(closed_generation)
            .saturating_sub(1) as u64;
        self.registry.ensure_sync_provider_ready(account_uuid)?;
        let plugin_id = self
            .registry
            .plugin_id_for_account(account_uuid)?
            .ok_or_else(|| anyhow::anyhow!("provider-account-not-found"))?;
        let work = self.provider_manager().begin_work(&plugin_id)?;
        let provider = self
            .provider_manager()
            .resolve(&plugin_id, account_uuid)?
            .ok_or_else(|| anyhow::anyhow!("provider-not-ready"))?;
        let (origin_binding_epoch, origin_transport_generation) = self
            .registry
            .reconnect_origin_transport(library_uuid, account_uuid, remote_library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("reconnect-origin-not-found"))?;
        let context = crate::sync::provider::TransportContext {
            namespace: remote_library_uuid.to_owned(),
            binding_epoch: origin_binding_epoch.to_string(),
            transport_generation: origin_transport_generation,
        };
        let mut remote_object_count = 0u64;
        let mut page_token = None;
        let mut remote_exists = false;
        loop {
            let page = provider
                .delta_list(
                    crate::sync::provider::DeltaScope::Library {
                        namespace: remote_library_uuid.to_owned(),
                        binding_epoch: origin_binding_epoch.to_string(),
                        transport_generation: origin_transport_generation,
                    },
                    None,
                    page_token.as_deref(),
                    200,
                )
                .await
                .map_err(anyhow::Error::new)?;
            remote_object_count = remote_object_count.saturating_add(page.objects.len() as u64);
            remote_exists |= page
                .objects
                .iter()
                .any(|object| object.key == "library-descriptor");
            page_token = page.page_token;
            if page_token.is_none() {
                break;
            }
            if remote_object_count > 1_000_000 {
                anyhow::bail!("remote-preview-object-limit");
            }
        }
        let mut remote_compatible = false;
        let mut remote_descriptor_sha256 = None;
        let mut remote_binding_epoch = None;
        if remote_exists {
            let object = provider
                .object_get(&context, "library-descriptor", 1024 * 1024)
                .await
                .map_err(anyhow::Error::new)?;
            crate::sync::provider::validate_download(&object, 1024 * 1024)
                .map_err(anyhow::Error::new)?;
            let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&object.path)?)?;
            remote_compatible = value.get("version").and_then(serde_json::Value::as_u64) == Some(1)
                && value.get("libraryUuid").and_then(serde_json::Value::as_str)
                    == Some(remote_library_uuid);
            remote_binding_epoch = value
                .get("bindingEpoch")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            remote_descriptor_sha256 = Some(object.metadata.sha256);
        }
        work.check_current()?;
        self.registry.create_reconnect_plan(
            library_uuid,
            account_uuid,
            remote_library_uuid,
            local_divergence_count,
            crate::host_registry::ReconnectPreviewFacts {
                remote_exists,
                remote_compatible,
                remote_object_count,
                remote_descriptor_sha256,
                remote_binding_epoch,
            },
        )
    }

    pub async fn fence_plugin_bindings(
        &self,
        plugin_id: &str,
        fences: Vec<crate::host_registry::BindingFence>,
    ) -> anyhow::Result<()> {
        let providers = self.provider_manager();
        for fence in fences {
            providers
                .fence_binding(
                    plugin_id,
                    &fence.account_uuid,
                    fence.binding_epoch,
                    fence.next_transport_generation,
                )
                .await?;
            self.sync_coordinator
                .cancel_media_binding(fence.binding_epoch)
                .await?;
        }
        Ok(())
    }
    pub fn new(
        registry: Arc<HostRegistry>,
        libraries_dir: PathBuf,
        bundled_plugins_dir: PathBuf,
        secret_store: Option<Arc<dyn crate::plugins::secret_store::SecretStore>>,
        active: Arc<RwLock<ActiveSnapshot>>,
        request_gate: Arc<tokio::sync::RwLock<()>>,
    ) -> Self {
        let runtime_leases = RuntimeLeases::default();
        let sync_coordinator = crate::sync::coordinator::SyncCoordinator::new(
            Arc::clone(&registry),
            runtime_leases.clone(),
        );
        let decoder_manager = crate::plugins::decoder::DecoderManager::new(
            libraries_dir
                .parent()
                .unwrap_or(&libraries_dir)
                .join("decoder-temp"),
        );
        Self {
            decoder_manager,
            storage_snapshot: Default::default(),
            registry,
            active,
            request_gate,
            operation: AsyncMutex::new(()),
            root_relocation_operations: Mutex::new(HashMap::new()),
            plugin_lifecycle_operations: Mutex::new(HashMap::new()),
            runtime_generation: AtomicU64::new(0),
            pending_runtime_stop: Mutex::new(None),
            library_operations_closed: AtomicBool::new(false),
            libraries_dir,
            bundled_plugins_dir,
            secret_store,
            runtime_leases,
            sync_coordinator,
            sync_scheduler: Mutex::new(None),
            root_relocation_scheduler: Mutex::new(None),
            plugin_runtime_scheduler: Mutex::new(None),
        }
    }

    pub fn bundled_plugins_dir(&self) -> &Path {
        &self.bundled_plugins_dir
    }

    pub fn start_plugin_runtime_supervisors(self: &Arc<Self>) {
        let cancellation = CancellationToken::new();
        let task_cancel = cancellation.clone();
        let (commands, mut command_rx) = tokio::sync::mpsc::channel(8);
        let weak = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            use std::collections::HashMap;
            let mut running: HashMap<String, PluginSupervisorTask> = HashMap::new();
            let mut stop_errors = HashMap::<String, String>::new();
            let mut blocked: HashMap<String, String> = HashMap::new();
            let mut decoder_running = HashMap::<String, (String, i64)>::new();
            let mut decoder_blocked = HashMap::<String, String>::new();
            loop {
                let Some(manager) = weak.upgrade() else {
                    break;
                };
                // Decoders are local capabilities and never create an account,
                // register a SyncProvider, or participate in sync reconciliation.
                let decoder_launches = manager
                    .registry
                    .decoder_runtime_launches()
                    .unwrap_or_default();
                let desired_decoders = decoder_launches
                    .iter()
                    .map(|launch| (launch.plugin_id.clone(), launch.fingerprint()))
                    .collect::<HashMap<_, _>>();
                decoder_blocked
                    .retain(|id, fingerprint| desired_decoders.get(id) == Some(fingerprint));
                let available = manager.decoder_manager.snapshot();
                let stale_decoders = decoder_running
                    .iter()
                    .filter(|(id, (fingerprint, _))| {
                        desired_decoders.get(*id) != Some(fingerprint)
                            || !available
                                .capabilities
                                .iter()
                                .any(|capability| &capability.identity.plugin_id == *id)
                    })
                    .map(|(id, (_, epoch))| (id.clone(), *epoch))
                    .collect::<Vec<_>>();
                for (id, epoch) in stale_decoders {
                    let mut expected_cancellation = false;
                    if let Some((fingerprint, _)) = decoder_running.remove(&id) {
                        if desired_decoders.get(&id) == Some(&fingerprint) {
                            expected_cancellation =
                                manager.decoder_manager.retired_after_job_cancellation(&id);
                            if expected_cancellation {
                                let _ = manager
                                    .registry
                                    .record_decoder_runtime_state(&id, epoch, "starting");
                            } else {
                                decoder_blocked.insert(id.clone(), fingerprint);
                                let _ = manager.registry.record_decoder_runtime_state(
                                    &id,
                                    epoch,
                                    "crashed-backoff",
                                );
                            }
                        }
                    }
                    let stopped = if expected_cancellation {
                        manager
                            .decoder_manager
                            .stop_expected_cancelled_process(&id, epoch)
                            .await
                    } else {
                        manager.decoder_manager.stop_plugin(&id, epoch).await
                    };
                    if let Err(error) = stopped {
                        stop_errors.insert(id, error.to_string());
                    }
                }
                for launch in decoder_launches {
                    let fingerprint = launch.fingerprint();
                    if decoder_running.contains_key(&launch.plugin_id)
                        || stop_errors.contains_key(&launch.plugin_id)
                        || decoder_blocked.get(&launch.plugin_id) == Some(&fingerprint)
                    {
                        continue;
                    }
                    let result = async {
                        let manifest: crate::plugins::manifest::ManifestRecord =
                            serde_json::from_str(&launch.manifest_json)?;
                        let platform = if cfg!(windows) {
                            crate::plugins::manifest::Platform::WindowsX64
                        } else {
                            crate::plugins::manifest::Platform::LinuxX64
                        };
                        let runtime_dir = manager
                            .libraries_dir
                            .parent()
                            .unwrap_or(&manager.libraries_dir)
                            .join("decoder-temp")
                            .join(&launch.plugin_id);
                        let process =
                            crate::plugins::runtime::decoder_process::DecoderProcess::activate(
                                &manifest,
                                &launch.install_root,
                                platform,
                                crate::plugins::runtime::trust::embedded_trust_registry(),
                                runtime_dir,
                                launch.granted_permissions.clone(),
                            )
                            .await?;
                        if let Err(error) = manager.decoder_manager.register(
                            &manifest,
                            launch.lifecycle_epoch,
                            process.clone(),
                        ) {
                            process.shutdown().await?;
                            return Err(error);
                        }
                        if !manager.registry.record_decoder_runtime_state(
                            &launch.plugin_id,
                            launch.lifecycle_epoch,
                            "ready",
                        )? {
                            manager
                                .decoder_manager
                                .stop_plugin(&launch.plugin_id, launch.lifecycle_epoch)
                                .await?;
                            anyhow::bail!("decoder launch became stale");
                        }
                        Ok::<(), anyhow::Error>(())
                    }
                    .await;
                    match result {
                        Ok(()) => {
                            decoder_running
                                .insert(launch.plugin_id, (fingerprint, launch.lifecycle_epoch));
                        }
                        Err(error) => {
                            let _ = manager.registry.record_decoder_runtime_state(
                                &launch.plugin_id,
                                launch.lifecycle_epoch,
                                "incompatible",
                            );
                            tracing::warn!(plugin_id = %launch.plugin_id, %error, "decoder activation failed");
                            decoder_blocked.insert(launch.plugin_id, fingerprint);
                        }
                    }
                }
                let running_plugins = running.keys().cloned().collect();
                if let Err(error) =
                    manager.reconcile_credential_retirements_once(Some(&running_plugins))
                {
                    tracing::warn!(%error, "credential retirement reconciliation failed");
                }
                let launches = match manager.registry.plugin_runtime_launches() {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::error!(%error, "failed to reconcile plugin runtimes");
                        Vec::new()
                    }
                };
                let desired = launches
                    .iter()
                    .map(|v| (v.plugin_id.clone(), v.fingerprint()))
                    .collect::<HashMap<_, _>>();
                blocked.retain(|id, fingerprint| {
                    desired
                        .get(id)
                        .is_some_and(|desired| desired == fingerprint)
                });
                let finished = running
                    .iter()
                    .filter(|(_, (_, _, handle))| handle.is_finished())
                    .map(|(id, _)| id.clone())
                    .collect::<Vec<_>>();
                for id in finished {
                    if let Some((_, _, handle)) = running.remove(&id) {
                        match handle.await {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => {
                                stop_errors.insert(id, error.to_string());
                            }
                            Err(error) => {
                                stop_errors.insert(id, error.to_string());
                            }
                        }
                    }
                }
                let stale = running
                    .keys()
                    .filter(|id| {
                        desired.get(*id).is_none_or(|fingerprint| {
                            running.get(*id).is_some_and(|v| &v.0 != fingerprint)
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                for id in stale {
                    if let Some((_, cancel, handle)) = running.remove(&id) {
                        // Preserve the retired slot's epoch. The new persisted
                        // launch must be newer, and cannot enter until all old
                        // provider work (including queued writes) has drained.
                        manager.provider_manager().close_plugin(&id, 0);
                        cancel.cancel();
                        match handle.await {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => {
                                stop_errors.insert(id.clone(), error.to_string());
                            }
                            Err(error) => {
                                stop_errors.insert(id.clone(), error.to_string());
                            }
                        }
                        manager.provider_manager().join_plugin_work(&id).await;
                    }
                }
                for launch in launches {
                    if stop_errors.contains_key(&launch.plugin_id) {
                        continue;
                    }
                    if let Ok(epochs) = manager
                        .registry
                        .resume_plugin_suspended_bindings(&launch.plugin_id)
                    {
                        for epoch in epochs {
                            manager.sync_coordinator.resume_media_binding(epoch).await;
                            manager.sync_coordinator.wake();
                        }
                    }
                    if running.contains_key(&launch.plugin_id) {
                        continue;
                    }
                    let fingerprint = launch.fingerprint();
                    if blocked.get(&launch.plugin_id) == Some(&fingerprint) {
                        continue;
                    }
                    let Ok(manifest) = serde_json::from_str::<
                        crate::plugins::manifest::ManifestRecord,
                    >(&launch.manifest_json) else {
                        let _ = manager.registry.record_plugin_runtime_failure(
                            &launch.plugin_id,
                            &fingerprint,
                            "incompatible",
                            "manifest-invalid",
                            "stored bundled manifest is invalid",
                            None,
                        );
                        blocked.insert(launch.plugin_id.clone(), fingerprint);
                        continue;
                    };
                    let platform = if cfg!(windows) {
                        crate::plugins::manifest::Platform::WindowsX64
                    } else {
                        crate::plugins::manifest::Platform::LinuxX64
                    };
                    let trusted = crate::plugins::runtime::trust::embedded_trust_registry()
                        .iter()
                        .any(|entry| {
                            entry.plugin_id == launch.plugin_id
                                && entry.version == launch.version
                                && entry.platform == platform
                                && entry.binary_sha256 == launch.entry_sha256
                        });
                    if !trusted {
                        let _ = manager.registry.record_plugin_runtime_failure(
                            &launch.plugin_id,
                            &fingerprint,
                            "incompatible",
                            "not-in-embedded-trust-registry",
                            "selected Provider is not present in the build-time trust registry",
                            None,
                        );
                        blocked.insert(launch.plugin_id.clone(), fingerprint);
                        continue;
                    }
                    let plugin_id = launch.plugin_id.clone();
                    let account_uuid = launch.account_uuid.clone();
                    let registry = Arc::clone(&manager.registry);
                    let providers = manager.provider_manager();
                    if providers
                        .open_plugin(&plugin_id, launch.lifecycle_epoch)
                        .is_err()
                    {
                        continue;
                    }
                    let child_cancel = task_cancel.child_token();
                    let supervisor_cancel = child_cancel.clone();
                    let install_root = launch.install_root.clone();
                    let runtime_root = std::env::var_os("MANGOO_PLUGIN_STATE_DIR")
                        .filter(|value| !value.is_empty())
                        .map(PathBuf::from)
                        .unwrap_or_else(|| {
                            manager
                                .libraries_dir
                                .parent()
                                .unwrap_or(&manager.libraries_dir)
                                .join("plugin-state")
                        });
                    let runtime_dir = runtime_root.join(&plugin_id).join(&account_uuid);
                    let task_manifest = manifest.clone();
                    let task_plugin_id = plugin_id.clone();
                    let task_account_uuid = account_uuid.clone();
                    let task_granted_permissions = launch.granted_permissions.clone();
                    let task_fingerprint = fingerprint.clone();
                    let task_providers = providers.clone();
                    let activation_registry = Arc::clone(&registry);
                    let secret_store = manager.secret_store.clone();
                    let handle = tokio::spawn(async move {
                        let supervisor_fingerprint = task_fingerprint.clone();
                        let activation_fingerprint = task_fingerprint;
                        let activation_providers = task_providers.clone();
                        let activation_manifest = task_manifest.clone();
                        let activation_account = task_account_uuid.clone();
                        let activation_root = install_root.clone();
                        let activation_runtime = runtime_dir.clone();
                        let result = crate::plugins::runtime::supervisor::supervise_until_cancelled(
                                Arc::clone(&registry),
                                task_providers,
                                task_plugin_id.clone(),
                                task_account_uuid,
                                supervisor_fingerprint,
                                supervisor_cancel,
                                Default::default(),
                                Default::default(),
                                move || {
                                    let providers = activation_providers.clone();
                                    let manifest = activation_manifest.clone();
                                    let account = activation_account.clone();
                                    let granted_permissions = task_granted_permissions.clone();
                                    let root = activation_root.clone();
                                    let runtime = activation_runtime.clone();
                                    let secret_store = secret_store.clone();
                                    let registry = Arc::clone(&activation_registry);
                                    let launch_fingerprint = activation_fingerprint.clone();
                                    async move {
                                        let configuration = registry.plugin_startup_configuration(&manifest.id)?;
                                        let reverse = secret_store.map(|store| Arc::new(crate::plugins::runtime::secret_broker::SecretBroker::new(store, registry, manifest.id.clone(), account.clone(), launch_fingerprint)) as Arc<dyn crate::plugins::runtime::rpc::ReverseRpcHandler>);
                                        crate::plugins::runtime::supervisor::activate_and_register(
                                            &providers,
                                            &manifest,
                                            &account,
                                            granted_permissions,
                                            &root,
                                            platform,
                                            crate::plugins::runtime::trust::embedded_trust_registry(
                                            ),
                                            runtime,
                                            reverse,
                                            configuration,
                                        )
                                        .await
                                    }
                                },
                            )
                            .await;
                        if let Err(error) = &result {
                            tracing::error!(plugin_id = %task_plugin_id, %error, "plugin supervisor stopped with error");
                        }
                        result
                    });
                    running.insert(plugin_id, (fingerprint, child_cancel, handle));
                }
                tokio::select! {
                    _ = task_cancel.cancelled() => break,
                    command = command_rx.recv() => {
                        let Some(PluginRuntimeCommand::Stop { plugin_id, completed }) = command else {
                            break;
                        };
                        let result = stop_plugin_supervisor(&plugin_id, &mut running, &mut stop_errors, &manager.registry, &manager.provider_manager()).await;
                        let result = match manager.decoder_manager.stop_plugin(&plugin_id, manager.registry.plugin_lifecycle_epoch(&plugin_id).unwrap_or(0)).await {
                            Ok(()) => result,
                            Err(error) => Err(error.to_string()),
                        };
                        decoder_running.remove(&plugin_id);
                        let _ = completed.send(result);
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                }
            }
            for (_, (_, cancel, handle)) in running {
                cancel.cancel();
                let _ = handle.await;
            }
            if let Some(manager) = weak.upgrade() {
                if let Err(error) = manager.decoder_manager.shutdown().await {
                    tracing::error!(%error, "decoder runtime shutdown failed");
                }
            }
        });
        *self
            .plugin_runtime_scheduler
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(PluginRuntimeSchedulerHandle {
            cancellation,
            commands,
            task,
        });
    }

    pub fn start_sync_coordinator(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let scheduler = self.sync_coordinator.spawn_scheduler(
            crate::sync::coordinator::PRODUCTION_SYNC_POLL_INTERVAL,
            crate::sync::coordinator::PRODUCTION_SYNC_CONCURRENCY,
            move || {
                let manager = weak.upgrade()?;
                let snapshot = manager.active_snapshot();
                let library_uuid = snapshot.library_uuid?;
                let runtime = snapshot.runtime?;
                let path = runtime.read_database_path.clone()?;
                Some((library_uuid, path, runtime.writer.clone()))
            },
        );
        *self
            .sync_scheduler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(scheduler);

        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let weak = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = task_cancellation.cancelled() => break,
                    _ = interval.tick() => {
                        let Some(manager) = weak.upgrade() else { break; };
                        if let Err(error) = manager.resume_pending_root_relocations().await {
                            tracing::warn!(%error, "root relocation recovery turn failed");
                        }
                        match manager.registry.sync_bound_libraries() {
                            Ok(libraries) => {
                                for library in libraries {
                                    if let Err(error) = manager
                                        .refresh_sync_folder_inventory(&library.library_uuid)
                                        .await
                                    {
                                        tracing::warn!(
                                            library_uuid = %library.library_uuid,
                                            %error,
                                            "synchronized-folder materialization turn failed"
                                        );
                                    }
                                }
                            }
                            Err(error) => tracing::warn!(%error, "could not list synchronized libraries for materialization"),
                        }
                    }
                }
            }
        });
        *self
            .root_relocation_scheduler
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some((cancellation, task));
    }

    #[allow(dead_code)]
    pub async fn shutdown_sync_coordinator(&self) -> anyhow::Result<()> {
        // New HTTP lifecycle tasks outlive a cancelled requester. Close their
        // admission before draining the shared operation lock so none can publish
        // a runtime while Host-owned workers are being stopped.
        self.library_operations_closed
            .store(true, Ordering::Release);
        let _operation = self.operation.lock().await;
        self.finish_pending_runtime_stop().await?;
        drop(_operation);
        let relocation_scheduler = self
            .root_relocation_scheduler
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some((cancellation, task)) = relocation_scheduler {
            cancellation.cancel();
            task.await
                .map_err(|error| anyhow::anyhow!("root relocation scheduler failed: {error}"))?;
        }
        let plugin_scheduler = self
            .plugin_runtime_scheduler
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(plugin_scheduler) = plugin_scheduler {
            plugin_scheduler.cancellation.cancel();
            plugin_scheduler
                .task
                .await
                .map_err(|error| anyhow::anyhow!("plugin runtime scheduler failed: {error}"))?;
        }
        let scheduler = self
            .sync_scheduler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let scheduler_join = if let Some(scheduler) = scheduler {
            scheduler.cancel();
            self.sync_coordinator.cancel_all_media_workers().await;
            scheduler.join().await
        } else {
            self.sync_coordinator.cancel_all_media_workers().await;
            Ok(())
        };
        let media_shutdown = self.sync_coordinator.shutdown_media_workers().await;
        self.decoder_manager.shutdown().await?;
        let provider_shutdown = self.sync_coordinator.provider_manager().shutdown().await;
        scheduler_join?;
        media_shutdown?;
        provider_shutdown
    }

    pub fn registry(&self) -> &Arc<HostRegistry> {
        &self.registry
    }

    pub fn active_snapshot(&self) -> ActiveSnapshot {
        self.active
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn runtime_generation(&self) -> u64 {
        self.runtime_generation.load(Ordering::Acquire)
    }

    pub async fn restore_active(&self) -> anyhow::Result<()> {
        self.refresh_storage_snapshot()?;
        self.recover_journals()?;
        // Opening owns only a fresh Host-local catalog. Recovery never writes
        // or deletes anything in the selected managed root.
        let registration_storage = LibraryRegistrationStorage {
            registry: Arc::clone(&self.registry),
            libraries_dir: self.libraries_dir.clone(),
        };
        for intent in self.registry.library_open_intents()? {
            if let Err(error) = registration_storage.finish_library_open(intent) {
                tracing::warn!(%error, "existing Library registration remains pending");
            }
        }
        self.registry.recover_activation_journal()?;
        self.registry.recover_sync_orchestration_journals()?;
        // Resolve ambiguous source/target journal states before prepare_runtime
        // starts ordinary root workers and publishes their execution tokens.
        let _ = self.resume_pending_root_relocations().await?;
        let snapshot = self.registry.snapshot()?;
        if let Some(library_uuid) = snapshot.active_library_uuid {
            let descriptor = self
                .registry
                .library(&library_uuid)?
                .ok_or_else(|| anyhow::anyhow!("active library is missing from registry"))?;
            let runtime = match self.prepare_runtime(&descriptor) {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::warn!(%error, %library_uuid, "Library unavailable; continuing Host startup");
                    self.registry.isolate_unavailable_library(&library_uuid)?;
                    let snapshot = self.registry.snapshot()?;
                    *self.active.write().unwrap_or_else(|p| p.into_inner()) = ActiveSnapshot {
                        library_uuid: None,
                        context_revision: snapshot.context_revision,
                        runtime_generation: self.runtime_generation(),
                        runtime: None,
                    };
                    return Ok(());
                }
            };
            *self.active.write().unwrap_or_else(|p| p.into_inner()) = ActiveSnapshot {
                library_uuid: Some(library_uuid),
                context_revision: snapshot.context_revision,
                runtime_generation: runtime.runtime_generation,
                runtime: Some(runtime),
            };
        } else {
            *self.active.write().unwrap_or_else(|p| p.into_inner()) = ActiveSnapshot {
                library_uuid: None,
                context_revision: snapshot.context_revision,
                runtime_generation: self.runtime_generation(),
                runtime: None,
            };
        }
        self.recover_sync_runtime_journals().await?;
        let _ = self.resume_pending_root_relocations().await?;
        Ok(())
    }

    async fn recover_sync_runtime_journals(&self) -> anyhow::Result<()> {
        for recovery in self.registry.pending_sync_binding_recovery()? {
            let Some(library) = self.registry.library(&recovery.library_uuid)? else {
                continue;
            };
            if verify_descriptor_identity(&library).is_err() {
                self.registry
                    .isolate_unavailable_library(&library.library_uuid)?;
                continue;
            }
            let revision = self.registry.snapshot()?.context_revision;
            match recovery.state.as_str() {
                "unbinding" => {
                    if self
                        .registry
                        .binding_transport(&recovery.library_uuid, "library")?
                        .is_some()
                    {
                        self.unbind_library_sync_with_revision(&recovery.library_uuid, revision)
                            .await?;
                    }
                    if let (Some(plan_uuid), Some(choice)) =
                        (recovery.plan_uuid.as_deref(), recovery.choice.as_deref())
                    {
                        self.registry
                            .advance_unbind_plan(plan_uuid, choice, "reconciling")?;
                    }
                }
                "preparing" => {
                    let descriptor = self
                        .registry
                        .library(&recovery.library_uuid)?
                        .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
                    let transport = self
                        .registry
                        .binding_transport(&recovery.library_uuid, "library")?
                        .ok_or_else(|| anyhow::anyhow!("sync-binding-not-found"))?;
                    let active = self.active_snapshot();
                    let generation =
                        if active.library_uuid.as_deref() == Some(recovery.library_uuid.as_str()) {
                            active
                                .runtime
                                .ok_or_else(|| anyhow::anyhow!("library-runtime-unavailable"))?
                                .writer
                                .write_authoritative(move |database| {
                                    database.begin_sync_capture(
                                        transport.binding_epoch,
                                        transport.actor_epoch,
                                    )
                                })
                                .await?
                        } else {
                            let runtime = crate::sync::runtime::DormantSyncRuntime::new(
                                &recovery.library_uuid,
                                descriptor.local_data_dir.join("catalog.sqlite"),
                                self.runtime_leases.clone(),
                            );
                            runtime
                                .run_turn(move |database| {
                                    database.begin_sync_capture(
                                        transport.binding_epoch,
                                        transport.actor_epoch,
                                    )
                                })
                                .await?
                        };
                    self.registry.set_sync_capture_generation(
                        &recovery.library_uuid,
                        transport.binding_epoch,
                        generation,
                    )?;
                }
                _ => {}
            }
        }
        let _ = self.sync_coordinator.reconcile_pending_unbinds().await?;
        for restore in self.registry.pending_restore_recovery()? {
            let descriptor = self.registry.library(&restore.library_uuid)?;
            if restore.catalog_ready {
                if descriptor
                    .as_ref()
                    .is_some_and(|value| value.local_data_dir.join("catalog.sqlite").is_file())
                {
                    if restore.state != "paused" {
                        let _ = self.registry.wake_binding(&restore.library_uuid);
                    }
                    let _ = self.publish_restore_notification(&restore).await;
                    continue;
                }
                self.registry
                    .fail_restore_plan(&restore.restore_uuid, "catalog-missing-after-restart")?;
                continue;
            }
            if let Some(descriptor) = descriptor {
                if self
                    .registry
                    .ensure_restore_cleanup_owned(&restore.restore_uuid, &restore.library_uuid)
                    .is_err()
                {
                    self.registry
                        .fail_restore_plan(&restore.restore_uuid, "restore-ownership-mismatch")?;
                    continue;
                }
                if descriptor.local_data_dir.join("catalog.sqlite").is_file() {
                    self.registry
                        .mark_restore_catalog_ready(&restore.library_uuid)?;
                } else if restore.state == "failed" {
                    let _ = self.discard_restore_plan(&restore.restore_uuid).await;
                } else {
                    let _ = self.registry.wake_binding(&restore.library_uuid);
                }
            } else {
                let _ = self
                    .registry
                    .commit_cancelled_restore_cleanup(&restore.restore_uuid, &restore.library_uuid);
            }
        }
        self.sync_coordinator.wake();
        Ok(())
    }

    fn recover_journals(&self) -> anyhow::Result<()> {
        for recovery in self.registry.pending_creation_recovery()? {
            if recovery.layout_version != STORAGE_FORMAT_VERSION {
                tracing::warn!(txn_uuid = %recovery.txn_uuid, "unsupported Library creation journal preserved");
                continue;
            }
            if let Err(error) = self.recover_creation(&recovery.txn_uuid) {
                tracing::warn!(%error, txn_uuid = %recovery.txn_uuid, "Library creation recovery remains pending");
            }
        }
        for recovery in self.registry.pending_removal_recovery()? {
            if recovery.layout_version != STORAGE_FORMAT_VERSION
                || recovery.managed_root != recovery.local_data_dir
                || recovery.local_quarantine.is_some()
            {
                tracing::warn!(txn_uuid = %recovery.txn_uuid, "unsupported Library removal journal preserved");
                continue;
            }
            let result = (|| -> anyhow::Result<()> {
                let Some(quarantine) = &recovery.root_quarantine else {
                    return self
                        .registry
                        .rollback_removal(&recovery.txn_uuid, &recovery.library_uuid);
                };
                let identity = recovery
                    .root_quarantine_identity
                    .as_deref()
                    .context("quarantine identity missing")?;
                if recovery.phase == "committed" {
                    if quarantine.exists() {
                        if marker_identity(&quarantine.join(MARKER_FILE_NAME))?
                            != recovery.marker_identity
                        {
                            anyhow::bail!("marker-identity-mismatch");
                        }
                        safe_delete_tree_expected(quarantine, identity)?;
                    }
                    self.registry.mark_removal_complete(&recovery.txn_uuid)?;
                } else {
                    if quarantine.exists() {
                        safe_rename_directory_expected(
                            quarantine,
                            &recovery.managed_root,
                            identity,
                        )?;
                    }
                    self.registry
                        .rollback_removal(&recovery.txn_uuid, &recovery.library_uuid)?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                tracing::warn!(%error, "Library storage removal recovery remains pending");
            }
        }
        Ok(())
    }

    pub async fn path_admission(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.operation.lock().await
    }

    fn refresh_storage_snapshot(&self) -> anyhow::Result<()> {
        let mut storage: Vec<PathBuf> = self
            .registry
            .snapshot()?
            .libraries
            .into_iter()
            .flat_map(|library| [library.managed_root, library.local_data_dir])
            .collect();
        storage.extend(self.registry.pending_storage_paths(None)?.0);
        self.storage_snapshot.publish(storage);
        Ok(())
    }

    pub fn preflight_plan(
        &self,
        name: &str,
        database: &Path,
        roots: &[LibraryContentRootInput],
    ) -> anyhow::Result<LibraryPlanPreflight> {
        self.preflight_plan_internal(name, database, roots, false, None)
    }

    pub fn preflight_restore_plan(
        &self,
        name: &str,
        database: &Path,
        roots: &[LibraryContentRootInput],
    ) -> anyhow::Result<LibraryPlanPreflight> {
        self.preflight_plan_internal(name, database, roots, true, None)
    }

    fn admission_paths(
        &self,
        skip_restore: Option<&str>,
    ) -> anyhow::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
        // Snapshot the Host first; never hold its connection while opening a Catalog.
        let descriptors = self.registry.snapshot()?.libraries;
        let (mut storage, mut content) = self.registry.pending_storage_paths(skip_restore)?;
        for descriptor in descriptors {
            let path = crate::library_path::inspect_directory_path(&descriptor.managed_root, true)
                .unwrap_or_else(|_| descriptor.managed_root.clone());
            storage.push(path);
            if descriptor.local_data_dir != descriptor.managed_root {
                storage.push(
                    crate::library_path::inspect_directory_path(&descriptor.local_data_dir, true)
                        .unwrap_or_else(|_| descriptor.local_data_dir.clone()),
                );
            }
            let marker = read_marker(&descriptor.managed_root).ok();
            if (descriptor.local_data_dir == descriptor.managed_root
                || marker
                    .as_ref()
                    .is_some_and(|marker| marker.format_version == STORAGE_FORMAT_VERSION))
                && !self
                    .registry
                    .is_pending_restore_library(&descriptor.library_uuid)?
            {
                content.extend(
                    Database::inspect_existing_catalog(
                        &descriptor.local_data_dir.join("catalog.sqlite"),
                        Uuid::parse_str(&descriptor.library_uuid)?,
                        self.registry.device_uuid()?,
                    )
                    .with_context(|| {
                        format!(
                            "library-path-admission-unavailable: {}",
                            descriptor.library_uuid
                        )
                    })?,
                );
            }
        }
        for path in &mut content {
            *path = crate::library_path::inspect_directory_path(path, true).map_err(|reason| {
                anyhow::anyhow!("library-path-admission-unavailable: {}", reason.as_str())
            })?;
        }
        Ok((storage, content))
    }

    fn preflight_plan_internal(
        &self,
        name: &str,
        database: &Path,
        roots: &[LibraryContentRootInput],
        allow_empty: bool,
        skip_restore: Option<&str>,
    ) -> anyhow::Result<LibraryPlanPreflight> {
        let (storage, content) = self.admission_paths(skip_restore)?;
        Ok(crate::library_path::preflight_plan(
            name,
            database,
            roots,
            &storage,
            &content,
            allow_empty,
        ))
    }

    pub fn admit_content_path(&self, path: &Path, create: bool) -> anyhow::Result<PathBuf> {
        let path = crate::library_path::inspect_directory_path(path, create)
            .map_err(|reason| anyhow::anyhow!(reason.as_str()))?;
        let (storage, _) = self.admission_paths(None)?;
        if storage
            .iter()
            .any(|database| crate::library_path::paths_overlap(database, &path))
        {
            anyhow::bail!("database-content-overlap");
        }
        Ok(path)
    }

    fn create_owned_directory_chain(
        &self,
        txn: &str,
        target: &Path,
        role: &str,
        owned: &mut Vec<OwnedCreationArtifact>,
    ) -> anyhow::Result<()> {
        let ancestor = crate::library_path::nearest_existing_directory(target)?;
        let mut guard = MarkerDirectoryGuard::open(ancestor)?;
        let mut path = ancestor.to_path_buf();
        for component in target.strip_prefix(ancestor)?.components() {
            let child = guard.create_directory_child(component.as_os_str())?;
            path.push(component);
            owned.push(OwnedCreationArtifact {
                path: path.clone(),
                identity: child.identity().to_owned(),
                directory: true,
                role: role.to_owned(),
            });
            self.registry.record_creation_artifacts(txn, owned)?;
            guard = child;
        }
        guard.verify_unchanged()?;
        Ok(())
    }

    fn cleanup_creation_staging(
        &self,
        staging: &Path,
        artifacts: &[OwnedCreationArtifact],
    ) -> anyhow::Result<()> {
        cleanup_owned_creation_artifacts(
            &artifacts
                .iter()
                .filter(|artifact| artifact.path.starts_with(staging))
                .cloned()
                .collect::<Vec<_>>(),
        )
    }

    fn recover_creation(&self, txn: &str) -> anyhow::Result<()> {
        let Some(recovery) = self
            .registry
            .pending_creation_recovery()?
            .into_iter()
            .find(|item| item.txn_uuid == txn)
        else {
            return Ok(());
        };
        if recovery.layout_version != STORAGE_FORMAT_VERSION {
            return Ok(());
        }
        let final_path = recovery
            .final_storage_path
            .as_ref()
            .context("creation-storage-path-missing")?;
        let owned: Vec<OwnedCreationArtifact> =
            serde_json::from_str(&recovery.owned_artifacts_json)?;
        if final_path != &recovery.resolved_root {
            anyhow::bail!("creation-storage-path-changed");
        }
        if let Some(identity) = &recovery.root_identity {
            if final_path.exists()
                && MarkerDirectoryGuard::open_readonly(final_path)?.identity() != identity
            {
                anyhow::bail!("path-changed");
            }
        }
        let marker = read_marker(final_path).ok();
        if recovery.phase != "restore-cancelling"
            && marker.as_ref().is_some_and(|marker| {
                marker.format_version == STORAGE_FORMAT_VERSION
                    && marker.state == MarkerState::Committed
                    && marker.library_uuid == recovery.library_uuid
                    && marker.root_uuid == recovery.root_uuid
                    && marker.creation_txn_uuid == txn
            })
        {
            let marker_id = marker_identity(&final_path.join(MARKER_FILE_NAME))?;
            if !owned.iter().any(|artifact| {
                artifact.path == final_path.join(MARKER_FILE_NAME) && artifact.identity == marker_id
            }) {
                anyhow::bail!("marker-identity-mismatch");
            }
            if !self
                .registry
                .is_pending_restore_library(&recovery.library_uuid)?
            {
                let catalog_id =
                    crate::library_path::local_file_identity(&final_path.join("catalog.sqlite"))?;
                if !owned.iter().any(|artifact| {
                    artifact.path == final_path.join("catalog.sqlite")
                        && artifact.identity == catalog_id
                }) {
                    anyhow::bail!("catalog-identity-mismatch");
                }
                let actual = Database::inspect_existing_catalog(
                    &final_path.join("catalog.sqlite"),
                    Uuid::parse_str(&recovery.library_uuid)?,
                    self.registry.device_uuid()?,
                )?;
                let expected: Vec<LibraryContentRootInput> =
                    serde_json::from_str(&recovery.content_roots_json)?;
                if actual
                    != expected
                        .iter()
                        .map(|root| PathBuf::from(&root.path))
                        .collect::<Vec<_>>()
                {
                    anyhow::bail!("creation-root-set-changed");
                }
            }
            let descriptor = LibraryDescriptor {
                library_uuid: recovery.library_uuid,
                root_uuid: recovery.root_uuid,
                name: recovery.name,
                icon: recovery.icon,
                color: recovery.color,
                local_data_dir: final_path.clone(),
                managed_root: final_path.clone(),
                lifecycle_state: "ready".into(),
                binding_state: "local-only".into(),
                availability: "available".into(),
                marker_identity: marker_identity(&final_path.join(MARKER_FILE_NAME))?,
            };
            self.cleanup_creation_staging(&recovery.staging_dir, &owned)?;
            self.registry.commit_creation(txn, &descriptor)?;
        } else {
            cleanup_owned_creation_artifacts(&owned)?;
            if recovery.phase == "restore-cancelling" {
                self.registry
                    .commit_cancelled_restore_cleanup(txn, &recovery.library_uuid)?;
            }
            self.registry.mark_creation_rolled_back(txn)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn preflight(&self, name: &str, managed_root: &Path) -> anyhow::Result<PreflightResult> {
        let mut roots = self
            .registry
            .snapshot()?
            .libraries
            .into_iter()
            .map(|library| library.managed_root)
            .collect::<Vec<_>>();
        roots.extend(
            self.registry
                .library_open_intents()?
                .into_iter()
                .map(|intent| intent.descriptor.managed_root),
        );
        Ok(preflight(name, managed_root, &roots, &known_cloud_roots()))
    }

    pub async fn open_library_with_revision(
        &self,
        managed_root: PathBuf,
        expected_revision: u64,
    ) -> anyhow::Result<(LibraryDescriptor, bool, Option<String>)> {
        let _operation = self.operation.lock().await;
        self.ensure_library_operations_open()?;
        self.ensure_context_revision(expected_revision)?;
        let candidate = ExistingLibraryRoot::open(&managed_root)?;
        let candidate_content = Database::inspect_existing_catalog(
            &candidate.path.join("catalog.sqlite"),
            Uuid::parse_str(&candidate.marker.library_uuid)?,
            self.registry.device_uuid()?,
        )?;
        let (known_storage, known_content) = self.admission_paths(None)?;
        if known_content
            .iter()
            .any(|root| crate::library_path::paths_overlap(&candidate.path, root))
            || candidate_content.iter().any(|root| {
                known_storage
                    .iter()
                    .any(|storage| crate::library_path::paths_overlap(root, storage))
            })
        {
            anyhow::bail!("database-content-overlap");
        }
        drop(candidate);
        let storage = LibraryRegistrationStorage {
            registry: Arc::clone(&self.registry),
            libraries_dir: self.libraries_dir.clone(),
        };
        let (descriptor, _root_guard) =
            tokio::task::spawn_blocking(move || storage.open(managed_root)).await??;
        // Reopening the already active identity does not churn its runtime or
        // revision. A failed activation leaves a valid, visible registration.
        let active = self.active_snapshot();
        if active.library_uuid.as_deref() == Some(&descriptor.library_uuid)
            && active.runtime.is_some()
        {
            return Ok((descriptor, true, None));
        }
        let result = self.activate_locked(&descriptor).await;
        match result {
            Ok(_) => Ok((descriptor, true, None)),
            Err(error) => Ok((descriptor, false, Some(error.to_string()))),
        }
    }

    pub async fn forget_library_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
    ) -> anyhow::Result<()> {
        let _operation = self.operation.lock().await;
        self.ensure_library_operations_open()?;
        self.ensure_context_revision(expected_revision)?;
        let descriptor = self
            .registry
            .library(library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        if descriptor.lifecycle_state != "ready" {
            anyhow::bail!("library-unavailable");
        }
        let _gate = self.request_gate.write().await;
        let previous = self.active_snapshot();
        let was_active = previous.library_uuid.as_deref() == Some(library_uuid);
        if was_active {
            // Publish no usable runtime while stopping. A shutdown failure stays
            // fail-closed and leaves persistent visibility/active identity intact.
            let mut stopping = previous.clone();
            stopping.runtime = None;
            *self.active.write().unwrap_or_else(|p| p.into_inner()) = stopping;
            if let Some(runtime) = &previous.runtime {
                *self
                    .pending_runtime_stop
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Some(Arc::clone(runtime));
            }
            self.finish_pending_runtime_stop().await?;
        }
        let revision = match self
            .registry
            .forget_library(library_uuid, expected_revision)
        {
            Ok(revision) => revision,
            Err(error) => {
                if was_active {
                    self.restore_previous_snapshot(&previous)?;
                }
                return Err(error);
            }
        };
        if was_active {
            *self.active.write().unwrap_or_else(|p| p.into_inner()) = ActiveSnapshot {
                library_uuid: None,
                context_revision: revision,
                runtime_generation: previous.runtime_generation,
                runtime: None,
            };
        }
        // Host-owned synchronization is independent of browsing/list visibility.
        // Do not pause bindings, revoke accounts, or delete their catalog/cache.
        Ok(())
    }

    async fn finish_pending_runtime_stop(&self) -> anyhow::Result<()> {
        let pending = self
            .pending_runtime_stop
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(runtime) = pending {
            runtime
                .shutdown()
                .await
                .map_err(|error| anyhow::anyhow!("library-stop-failed: {error}"))?;
            self.pending_runtime_stop
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
        }
        Ok(())
    }

    fn ensure_library_operations_open(&self) -> anyhow::Result<()> {
        if self.library_operations_closed.load(Ordering::Acquire) {
            anyhow::bail!("host-shutting-down");
        }
        Ok(())
    }

    async fn stop_owned_runtime(
        &self,
        runtime: &Arc<LibraryRuntimeResources>,
    ) -> anyhow::Result<()> {
        *self
            .pending_runtime_stop
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(Arc::clone(runtime));
        self.finish_pending_runtime_stop().await
    }

    #[cfg(test)]
    pub async fn create_library(
        &self,
        name: String,
        icon: Option<String>,
        color: Option<String>,
        managed_root: PathBuf,
        preflight_token: Option<String>,
        activate: bool,
    ) -> anyhow::Result<(LibraryDescriptor, bool, Option<String>)> {
        let content = managed_root.with_file_name(format!(
            "{}-Files",
            managed_root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
        self.create_library_with_content(
            name,
            icon,
            color,
            managed_root,
            vec![LibraryContentRootInput {
                path: content.to_string_lossy().into_owned(),
                create_if_missing: true,
            }],
            preflight_token,
            activate,
        )
        .await
    }

    pub async fn create_library_with_content(
        &self,
        name: String,
        icon: Option<String>,
        color: Option<String>,
        database_path: PathBuf,
        content_roots: Vec<LibraryContentRootInput>,
        preflight_token: Option<String>,
        activate: bool,
    ) -> anyhow::Result<(LibraryDescriptor, bool, Option<String>)> {
        self.create_library_internal(
            name,
            icon,
            color,
            database_path,
            content_roots,
            preflight_token,
            activate,
            None,
            true,
            None,
        )
        .await
    }

    #[cfg(test)]
    pub async fn create_restore_library(
        &self,
        library_uuid: String,
        name: String,
        managed_root: PathBuf,
        account_uuid: &str,
    ) -> anyhow::Result<LibraryDescriptor> {
        self.registry.ensure_sync_provider_ready(account_uuid)?;
        let (descriptor, _, _) = self
            .create_library_internal(
                name,
                None,
                None,
                managed_root,
                Vec::new(),
                None,
                false,
                Some(library_uuid),
                false,
                None,
            )
            .await?;
        self.registry
            .begin_restore_binding(&descriptor.library_uuid, account_uuid)?;
        Ok(descriptor)
    }

    #[cfg(test)]
    async fn create_planned_restore_fixture(
        &self,
        library_uuid: String,
        name: String,
        managed_root: PathBuf,
        account_uuid: &str,
    ) -> anyhow::Result<LibraryDescriptor> {
        self.registry.ensure_sync_provider_ready(account_uuid)?;
        // Keep the integration helper on the durable restore-plan and owned
        // shell path; discovered descriptor evidence is pinned by plan creation.
        // HTTP preflight/path validation is covered separately by route tests.
        let operation_uuid = Uuid::now_v7().to_string();
        self.registry
            .ensure_restore_libraries_absent([library_uuid.as_str()])?;
        let request = serde_json::json!({
            "accountUuid": account_uuid,
            "libraryUuid": library_uuid,
            "name": name,
            "managedRoot": managed_root,
        });
        let preflight = self
            .registry
            .create_restore_preflight(&request, 1, 0, &[], &[])?;
        let (_, plans) = self.registry.claim_restore_and_record_plans(
            preflight.preflight_token.as_deref().unwrap(),
            &operation_uuid,
            &request,
            &[crate::host_registry::RestorePlanInput {
                account_uuid: account_uuid.to_owned(),
                library_uuid: library_uuid.clone(),
                managed_root: managed_root.clone(),
                restore_mode: "legacy-library".into(),
                inventory_fence: None,
                roots: Vec::new(),
            }],
        )?;
        self.create_planned_restore_library(
            &plans[0].restore_uuid,
            library_uuid,
            name,
            managed_root,
            account_uuid,
        )
        .await
    }

    pub async fn create_planned_restore_library(
        &self,
        restore_uuid: &str,
        library_uuid: String,
        name: String,
        managed_root: PathBuf,
        account_uuid: &str,
    ) -> anyhow::Result<LibraryDescriptor> {
        self.registry.ensure_sync_provider_ready(account_uuid)?;
        let plan = self
            .registry
            .pending_restore_recovery()?
            .into_iter()
            .find(|plan| plan.restore_uuid == restore_uuid)
            .context("restore-plan-not-found")?;
        if crate::library_path::inspect_directory_path(Path::new(&plan.placement_base), true)
            .map_err(|reason| anyhow::anyhow!(reason.as_str()))?
            != crate::library_path::inspect_directory_path(&managed_root, true)
                .map_err(|reason| anyhow::anyhow!(reason.as_str()))?
        {
            anyhow::bail!("restore-ownership-mismatch");
        }
        let (descriptor, _, _) = self
            .create_library_internal(
                name,
                None,
                None,
                PathBuf::from(&plan.database_path),
                plan.content_paths
                    .into_iter()
                    .map(|path| LibraryContentRootInput {
                        path,
                        create_if_missing: true,
                    })
                    .collect(),
                None,
                false,
                Some(library_uuid),
                false,
                Some((restore_uuid, account_uuid)),
            )
            .await?;
        self.registry
            .begin_restore_binding(&descriptor.library_uuid, account_uuid)?;
        Ok(descriptor)
    }

    async fn create_library_internal(
        &self,
        name: String,
        icon: Option<String>,
        color: Option<String>,
        database_path: PathBuf,
        content_roots: Vec<LibraryContentRootInput>,
        preflight_token: Option<String>,
        activate: bool,
        requested_library_uuid: Option<String>,
        initialize_catalog: bool,
        restore_owner: Option<(&str, &str)>,
    ) -> anyhow::Result<(LibraryDescriptor, bool, Option<String>)> {
        let _operation = self.operation.lock().await;
        self.ensure_library_operations_open()?;
        let library_uuid = requested_library_uuid.unwrap_or_else(|| Uuid::now_v7().to_string());
        Uuid::parse_str(&library_uuid)?;
        self.registry
            .ensure_restore_libraries_absent([library_uuid.as_str()])?;
        let validation = self.preflight_plan_internal(
            &name,
            &database_path,
            &content_roots,
            !initialize_catalog,
            restore_owner.map(|(txn, _)| txn),
        )?;
        if !validation.ok {
            anyhow::bail!("{}", validation.reason.as_str());
        }
        if preflight_token
            .as_ref()
            .is_some_and(|token| Some(token) != validation.token.as_ref())
        {
            anyhow::bail!("path-changed");
        }
        let database_path = validation
            .normalized_database_path
            .context("invalid-path")?;
        let content_roots = validation
            .normalized_content_roots
            .context("invalid-path")?;
        let anchors = std::iter::once(database_path.as_path())
            .chain(content_roots.iter().map(|root| Path::new(&root.path)))
            .map(|path| {
                let ancestor = crate::library_path::nearest_existing_directory(path)?.to_path_buf();
                let identity = MarkerDirectoryGuard::open_readonly(&ancestor)?
                    .identity()
                    .to_owned();
                Ok((ancestor, identity))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let root_uuid = Uuid::now_v7().to_string();
        let txn_uuid = restore_owner
            .map(|(txn, _)| txn.to_owned())
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        let staging = database_path.join(format!(".mangoo-create-{txn_uuid}"));
        let mut descriptor = LibraryDescriptor {
            library_uuid: library_uuid.clone(),
            root_uuid: root_uuid.clone(),
            name: crate::library_path::safe_library_name(&name)
                .map_err(|reason| anyhow::anyhow!(reason.as_str()))?,
            icon,
            color,
            local_data_dir: database_path.clone(),
            managed_root: database_path.clone(),
            lifecycle_state: "ready".into(),
            binding_state: "local-only".into(),
            availability: "available".into(),
            marker_identity: String::new(),
        };
        let intent = CreationIntent {
            txn_uuid: txn_uuid.clone(),
            descriptor: descriptor.clone(),
            requested_root: database_path.clone(),
            staging_dir: staging.clone(),
        };
        if let Some((_, account)) = restore_owner {
            self.registry.begin_restore_creation(&intent, account)?;
        } else {
            self.registry.begin_creation(&intent)?;
        }
        self.registry
            .record_creation_layout(&txn_uuid, &database_path, &content_roots)?;
        self.refresh_storage_snapshot()?;
        let mut owned = Vec::new();
        let preparation = (|| -> anyhow::Result<()> {
            self.create_owned_directory_chain(&txn_uuid, &database_path, "database", &mut owned)?;
            for root in &content_roots {
                if root.create_if_missing {
                    self.create_owned_directory_chain(
                        &txn_uuid,
                        Path::new(&root.path),
                        "content-root",
                        &mut owned,
                    )?;
                }
            }
            for (path, identity) in &anchors {
                if MarkerDirectoryGuard::open_readonly(path)?.identity() != identity {
                    anyhow::bail!("path-changed");
                }
            }
            let content_guards = content_roots
                .iter()
                .map(|root| MarkerDirectoryGuard::open_readonly(Path::new(&root.path)))
                .collect::<Result<Vec<_>, _>>()?;
            let root_guard = SafeDirectory::open(&database_path)?;
            self.registry.update_creation_identities(
                &txn_uuid,
                Some(root_guard.identity()),
                None,
            )?;
            let pending = LibraryMarker {
                format_version: STORAGE_FORMAT_VERSION,
                library_uuid: library_uuid.clone(),
                root_uuid: root_uuid.clone(),
                state: MarkerState::Pending,
                creation_txn_uuid: txn_uuid.clone(),
            };
            let identity = crate::library_path::atomic_write_marker_with_ownership(
                &root_guard,
                &pending,
                |temporary, identity| {
                    for path in [
                        database_path.join(temporary),
                        database_path.join(MARKER_FILE_NAME),
                    ] {
                        owned.push(OwnedCreationArtifact {
                            path,
                            identity: identity.to_owned(),
                            directory: false,
                            role: "marker".into(),
                        });
                    }
                    self.registry.record_creation_artifacts(&txn_uuid, &owned)
                },
            )?;
            self.registry.record_creation_artifacts(&txn_uuid, &owned)?;
            self.registry.update_creation_phase(
                &txn_uuid,
                "marker-pending",
                false,
                true,
                Some(&identity),
                None,
            )?;
            drop(root_guard);
            if initialize_catalog {
                self.create_owned_directory_chain(&txn_uuid, &staging, "staging", &mut owned)?;
                let staging_guard = MarkerDirectoryGuard::open(&staging)?;
                self.registry.update_creation_identities(
                    &txn_uuid,
                    None,
                    Some(staging_guard.identity()),
                )?;
                // CREATE_NEW pins the Catalog identity before SQLite can write it.
                let file = staging_guard.create_catalog()?;
                let catalog_identity = staging_guard.file_identity(&file)?;
                owned.push(OwnedCreationArtifact {
                    path: staging.join("catalog.sqlite"),
                    identity: catalog_identity.clone(),
                    directory: false,
                    role: "catalog".into(),
                });
                self.registry.record_creation_artifacts(&txn_uuid, &owned)?;
                {
                    let catalog = Database::open_existing(staging.join("catalog.sqlite"))?;
                    catalog.apply_migrations()?;
                    if initialize_catalog {
                        for root in &content_roots {
                            let path = crate::library_path::inspect_directory_path(
                                Path::new(&root.path),
                                false,
                            )
                            .map_err(|reason| anyhow::anyhow!(reason.as_str()))?;
                            if path != PathBuf::from(&root.path) {
                                anyhow::bail!("path-changed");
                            }
                            let root_id = catalog.add_root(NewRoot {
                                path: root.path.clone(),
                                display_name: path
                                    .file_name()
                                    .and_then(|leaf| leaf.to_str())
                                    .unwrap_or(&name)
                                    .to_owned(),
                            })?;
                            catalog.create_root_scan_task_with_origin(
                                root_id,
                                crate::db::NotificationOrigin::User,
                            )?;
                        }
                    }
                    catalog.initialize_sync_identity(
                        Uuid::parse_str(&library_uuid)?,
                        Uuid::parse_str(&root_uuid)?,
                        self.registry.device_uuid()?,
                    )?;
                }
                drop(file);
                staging_guard.verify_unchanged()?;
                // Closing the sole SQLite connection checkpoints/deletes its sidecars.
                // A residual name is not ownership proof: preserve it and retry.
                if ["catalog.sqlite-wal", "catalog.sqlite-shm"]
                    .iter()
                    .any(|leaf| staging.join(leaf).exists())
                {
                    anyhow::bail!("creation-staging-unowned-artifact");
                }
                self.registry.record_creation_artifacts(&txn_uuid, &owned)?;
                self.registry.update_creation_phase(
                    &txn_uuid,
                    "prepared",
                    false,
                    true,
                    Some(&identity),
                    None,
                )?;
                let destination = MarkerDirectoryGuard::open(&database_path)?;
                let catalog = staging_guard.open_temporary("catalog.sqlite".as_ref(), false)?;
                if staging_guard.file_identity(&catalog)? != catalog_identity {
                    anyhow::bail!("path-changed");
                }
                // Record both possible locations before no-replace publication.
                owned.push(OwnedCreationArtifact {
                    path: database_path.join("catalog.sqlite"),
                    identity: catalog_identity,
                    directory: false,
                    role: "catalog".into(),
                });
                self.registry.record_creation_artifacts(&txn_uuid, &owned)?;
                staging_guard.publish_to(
                    &catalog,
                    "catalog.sqlite".as_ref(),
                    &destination,
                    "catalog.sqlite".as_ref(),
                )?;
                drop(catalog);
                drop(destination);
                drop(staging_guard);
                Database::inspect_existing_catalog(
                    &database_path.join("catalog.sqlite"),
                    Uuid::parse_str(&library_uuid)?,
                    self.registry.device_uuid()?,
                )?;
            }
            for guard in &content_guards {
                guard.verify_unchanged()?;
            }
            for (path, identity) in &anchors {
                if MarkerDirectoryGuard::open_readonly(path)?.identity() != identity {
                    anyhow::bail!("path-changed");
                }
            }
            let root_guard = SafeDirectory::open(&database_path)?;
            if root_guard.child_identity(MARKER_FILE_NAME.as_ref(), false)? != identity {
                anyhow::bail!("path-changed");
            }
            descriptor.marker_identity = crate::library_path::atomic_write_marker_with_ownership(
                &root_guard,
                &LibraryMarker {
                    state: MarkerState::Committed,
                    ..pending
                },
                |temporary, identity| {
                    for path in [
                        database_path.join(temporary),
                        database_path.join(MARKER_FILE_NAME),
                    ] {
                        owned.push(OwnedCreationArtifact {
                            path,
                            identity: identity.to_owned(),
                            directory: false,
                            role: "marker".into(),
                        });
                    }
                    self.registry.record_creation_artifacts(&txn_uuid, &owned)
                },
            )?;
            self.registry.record_creation_artifacts(&txn_uuid, &owned)?;
            self.registry.update_creation_phase(
                &txn_uuid,
                "marker-committed",
                false,
                true,
                Some(&descriptor.marker_identity),
                None,
            )?;
            drop(root_guard);
            drop(content_guards);
            self.cleanup_creation_staging(&staging, &owned)?;
            self.registry.commit_creation(&txn_uuid, &descriptor)?;
            Ok(())
        })();
        if let Err(error) = preparation {
            // A committed marker is forward-only. Compensation uses the same
            // persisted identity evidence as restart recovery.
            if let Err(recovery_error) = self.recover_creation(&txn_uuid) {
                tracing::warn!(%recovery_error, %txn_uuid, "Library creation recovery remains pending");
            }
            return Err(error);
        }
        let (mut activated, mut activation_error) = (false, None);
        if activate {
            match self.activate_locked(&descriptor).await {
                Ok(_) => activated = true,
                Err(error) => activation_error = Some(error.to_string()),
            }
        }
        Ok((descriptor, activated, activation_error))
    }

    #[cfg(test)]
    pub async fn activate(&self, library_uuid: &str) -> anyhow::Result<u64> {
        let _operation = self.operation.lock().await;
        self.activate_by_uuid_locked(library_uuid).await
    }

    pub async fn activate_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
    ) -> anyhow::Result<u64> {
        let _operation = self.operation.lock().await;
        self.ensure_context_revision(expected_revision)?;
        self.activate_by_uuid_locked(library_uuid).await
    }

    async fn activate_by_uuid_locked(&self, library_uuid: &str) -> anyhow::Result<u64> {
        let descriptor = self
            .registry
            .library(library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        if !self.registry.is_library_listed(library_uuid)? {
            anyhow::bail!("library-not-listed");
        }
        self.activate_locked(&descriptor).await
    }

    async fn activate_locked(&self, descriptor: &LibraryDescriptor) -> anyhow::Result<u64> {
        self.ensure_library_operations_open()?;
        if descriptor.lifecycle_state != "ready" || descriptor.availability != "available" {
            anyhow::bail!("library-unavailable");
        }
        let _gate = self.request_gate.write().await;
        self.finish_pending_runtime_stop().await?;
        let previous_snapshot = self.active_snapshot();
        *self.active.write().unwrap_or_else(|p| p.into_inner()) = ActiveSnapshot {
            library_uuid: previous_snapshot.library_uuid.clone(),
            context_revision: previous_snapshot.context_revision,
            runtime_generation: previous_snapshot.runtime_generation,
            runtime: None,
        };
        if let Some(previous) = &previous_snapshot.runtime {
            self.stop_owned_runtime(previous).await.map_err(|error| {
                anyhow::anyhow!(
                    "failed to stop previous Library runtime; Host remains fail-closed: {error}"
                )
            })?;
        }
        let candidate = match self.prepare_runtime(descriptor) {
            Ok(candidate) => candidate,
            Err(error) => {
                self.restore_previous_snapshot(&previous_snapshot)?;
                return Err(error);
            }
        };
        let txn_uuid = Uuid::now_v7().to_string();
        let revision = match self
            .registry
            .begin_activation(&txn_uuid, Some(&descriptor.library_uuid))
        {
            Ok(revision) => revision,
            Err(error) => {
                self.stop_owned_runtime(&candidate).await.map_err(|shutdown_error| {
                    anyhow::anyhow!(
                        "activation prepare failed ({error}); candidate shutdown also failed; Host remains fail-closed: {shutdown_error}"
                    )
                })?;
                self.restore_previous_snapshot(&previous_snapshot)?;
                return Err(error);
            }
        };
        if let Err(error) =
            self.registry
                .commit_activation(&txn_uuid, Some(&descriptor.library_uuid), revision)
        {
            self.registry
                .rollback_activation(&txn_uuid, "commit-failed")
                .ok();
            self.stop_owned_runtime(&candidate).await.map_err(|shutdown_error| {
                anyhow::anyhow!(
                    "activation commit failed ({error}); candidate shutdown also failed; Host remains fail-closed: {shutdown_error}"
                )
            })?;
            self.restore_previous_snapshot(&previous_snapshot)?;
            return Err(error);
        }
        *self.active.write().unwrap_or_else(|p| p.into_inner()) = ActiveSnapshot {
            library_uuid: Some(descriptor.library_uuid.clone()),
            context_revision: revision,
            runtime_generation: candidate.runtime_generation,
            runtime: Some(candidate),
        };
        Ok(revision)
    }

    fn restore_previous_snapshot(&self, previous: &ActiveSnapshot) -> anyhow::Result<()> {
        let runtime = match previous.library_uuid.as_deref() {
            Some(uuid) => self
                .registry
                .library(uuid)?
                .map(|descriptor| self.prepare_runtime(&descriptor))
                .transpose()?,
            None => None,
        };
        let generation = runtime
            .as_ref()
            .map_or(previous.runtime_generation, |value| {
                value.runtime_generation
            });
        *self.active.write().unwrap_or_else(|p| p.into_inner()) = ActiveSnapshot {
            library_uuid: previous.library_uuid.clone(),
            context_revision: previous.context_revision,
            runtime_generation: generation,
            runtime,
        };
        Ok(())
    }

    pub async fn remove_local_library_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
    ) -> anyhow::Result<()> {
        let _operation = self.operation.lock().await;
        self.ensure_context_revision(expected_revision)?;
        self.remove_local_library_locked(library_uuid).await
    }

    async fn remove_local_library_locked(&self, library_uuid: &str) -> anyhow::Result<()> {
        let descriptor = self
            .registry
            .library(library_uuid)?
            .context("library-not-found")?;
        if descriptor.binding_state != "local-only" {
            anyhow::bail!("library-is-synchronized");
        }
        verify_descriptor_identity(&descriptor)?;
        if descriptor.local_data_dir != descriptor.managed_root {
            anyhow::bail!("unsupported-library-marker");
        }
        let txn = Uuid::now_v7().to_string();
        self.registry.begin_removal(&descriptor, &txn)?;
        let _gate = self.request_gate.write().await;
        let previous = self.active_snapshot();
        let was_active = previous.library_uuid.as_deref() == Some(library_uuid);
        if was_active {
            *self.active.write().unwrap_or_else(|p| p.into_inner()) = ActiveSnapshot {
                runtime: None,
                ..previous.clone()
            };
            if let Some(runtime) = &previous.runtime {
                if let Err(error) = self.stop_owned_runtime(runtime).await {
                    self.registry.rollback_removal(&txn, library_uuid)?;
                    return Err(error);
                }
            } else if let Err(error) = self.finish_pending_runtime_stop().await {
                self.registry.rollback_removal(&txn, library_uuid)?;
                return Err(error);
            }
        }
        let quarantine = descriptor
            .managed_root
            .with_file_name(format!(".mangoo-quarantine-{txn}"));
        let identity = match (|| -> anyhow::Result<String> {
            verify_descriptor_identity(&descriptor)?;
            let identity = SafeDirectory::open(&descriptor.managed_root)
                .context("remove storage pin")?
                .identity()
                .to_owned();
            self.registry.update_removal_partial(
                &txn,
                "requested",
                Some(&quarantine),
                None,
                Some(&identity),
                None,
            )?;
            Ok(identity)
        })() {
            Ok(identity) => identity,
            Err(error) => {
                self.registry.rollback_removal(&txn, library_uuid)?;
                if was_active {
                    self.restore_previous_snapshot(&previous)?;
                }
                return Err(error);
            }
        };
        if let Err(error) =
            safe_rename_directory_expected(&descriptor.managed_root, &quarantine, &identity)
        {
            self.registry.rollback_removal(&txn, library_uuid)?;
            if was_active {
                self.restore_previous_snapshot(&previous)?;
            }
            return Err(anyhow::Error::new(error).context("remove storage quarantine rename"));
        }
        let transition = (|| -> anyhow::Result<()> {
            self.registry.update_removal_partial(
                &txn,
                "quarantined",
                Some(&quarantine),
                None,
                Some(&identity),
                None,
            )?;
            if was_active {
                let activation = Uuid::now_v7().to_string();
                let revision = self.registry.begin_activation(&activation, None)?;
                if let Err(error) = self.registry.commit_activation(&activation, None, revision) {
                    self.registry
                        .rollback_activation(&activation, "remove-activation-failed")?;
                    return Err(error);
                }
                *self.active.write().unwrap_or_else(|p| p.into_inner()) = ActiveSnapshot {
                    library_uuid: None,
                    context_revision: revision,
                    runtime_generation: previous.runtime_generation,
                    runtime: None,
                };
            }
            self.registry.commit_removal(&txn, library_uuid)?;
            Ok(())
        })();
        if let Err(error) = transition {
            safe_rename_directory_expected(&quarantine, &descriptor.managed_root, &identity)?;
            self.registry.rollback_removal(&txn, library_uuid)?;
            if was_active {
                self.restore_previous_snapshot(&previous)?;
            }
            return Err(error);
        }
        let registry = Arc::clone(&self.registry);
        let expected_marker = descriptor.marker_identity;
        tokio::task::spawn_blocking(move || {
            let result = (|| -> anyhow::Result<()> {
                if marker_identity(&quarantine.join(MARKER_FILE_NAME))? != expected_marker {
                    anyhow::bail!("marker-identity-mismatch");
                }
                safe_delete_tree_expected(&quarantine, &identity)?;
                registry.mark_removal_complete(&txn)
            })();
            if let Err(error) = result {
                tracing::warn!(%error, "Library storage quarantine cleanup remains pending");
            }
        });
        Ok(())
    }

    pub async fn update_library_metadata_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
        name: Option<&str>,
        icon: Option<Option<&str>>,
        color: Option<Option<&str>>,
    ) -> anyhow::Result<Option<LibraryDescriptor>> {
        let _operation = self.operation.lock().await;
        self.ensure_context_revision(expected_revision)?;
        self.registry
            .update_library_metadata(library_uuid, name, icon, color)
    }

    /// Provider-neutral production lifecycle entry point. Provider/account are
    /// identities only; network descriptor publication remains in the provider child.
    pub async fn enable_library_sync_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
        provider_id: &str,
        account_identity: &str,
    ) -> anyhow::Result<()> {
        let _operation = self.operation.lock().await;
        let _lifecycle = self.lock_plugin_lifecycle(provider_id).await;
        self.ensure_context_revision(expected_revision)?;
        let descriptor = self
            .registry
            .library(library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        if descriptor.binding_state != "local-only" {
            anyhow::bail!("library-already-synchronized");
        }
        self.registry
            .require_ready_sync_provider(provider_id, account_identity)?;
        if self
            .sync_coordinator
            .provider_manager()
            .resolve(provider_id, account_identity)?
            .is_none()
        {
            anyhow::bail!("provider-not-ready");
        }
        let (binding_epoch, actor_epoch) =
            self.registry
                .begin_sync_binding(library_uuid, provider_id, account_identity)?;
        let active = self.active_snapshot();
        let result: anyhow::Result<i64> = if active.library_uuid.as_deref() == Some(library_uuid) {
            let runtime = active
                .runtime
                .ok_or_else(|| anyhow::anyhow!("library-runtime-unavailable"))?;
            runtime
                .writer
                .write_authoritative(move |database| {
                    database.begin_sync_capture(binding_epoch, actor_epoch)
                })
                .await
        } else {
            let runtime = crate::sync::runtime::DormantSyncRuntime::new(
                library_uuid,
                descriptor.local_data_dir.join("catalog.sqlite"),
                self.runtime_leases.clone(),
            );
            runtime
                .run_turn(move |database| database.begin_sync_capture(binding_epoch, actor_epoch))
                .await
        };
        if let Err(error) = result {
            self.registry
                .abort_sync_binding(library_uuid, binding_epoch)?;
            return Err(error);
        }
        // The Catalog transaction returns the exact immutable generation
        // boundary. Persisting it in Host makes restart resume the same seed
        // rather than silently capturing a later projection.
        self.registry.set_sync_capture_generation(
            library_uuid,
            binding_epoch,
            result.expect("handled error above"),
        )?;
        self.sync_coordinator.wake();
        Ok(())
    }

    /// Stops local transport/capture without deleting the remote Library. The
    /// shared runtime lease/DbWriter is the cutover fence: all earlier turns
    /// finish first and late work sees capture disabled/old binding archived.
    pub async fn unbind_library_sync_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
    ) -> anyhow::Result<()> {
        let _operation = self.operation.lock().await;
        self.ensure_context_revision(expected_revision)?;
        let descriptor = self
            .registry
            .library(library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        if descriptor.binding_state == "local-only" {
            return Ok(());
        }
        let transport = self
            .registry
            .binding_transport(library_uuid, "library")?
            .ok_or_else(|| anyhow::anyhow!("sync-binding-not-found"))?;
        let next_generation = self
            .registry
            .begin_unbind(library_uuid, transport.binding_epoch)?;
        self.sync_coordinator
            .provider_manager()
            .fence_binding(
                &transport.plugin_id,
                &transport.account_uuid,
                transport.binding_epoch,
                next_generation as u64,
            )
            .await?;
        self.sync_coordinator
            .cancel_media_binding(transport.binding_epoch)
            .await?;
        let published = self.registry.sync_published_frontier(library_uuid)?;
        let active = self.active_snapshot();
        if active.library_uuid.as_deref() == Some(library_uuid) {
            let runtime = active
                .runtime
                .ok_or_else(|| anyhow::anyhow!("library-runtime-unavailable"))?;
            runtime
                .writer
                .write_authoritative(move |database| database.unbind_sync_capture(published))
                .await?;
        } else {
            let runtime = crate::sync::runtime::DormantSyncRuntime::new(
                library_uuid,
                descriptor.local_data_dir.join("catalog.sqlite"),
                self.runtime_leases.clone(),
            );
            runtime
                .run_turn(move |database| database.unbind_sync_capture(published))
                .await?;
        }
        self.registry.commit_local_unbind(library_uuid)?;
        Ok(())
    }

    pub async fn pause_library_sync_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
    ) -> anyhow::Result<()> {
        let _operation = self.operation.lock().await;
        self.ensure_context_revision(expected_revision)?;
        let transport = self
            .registry
            .binding_transport(library_uuid, "library")?
            .ok_or_else(|| anyhow::anyhow!("sync-binding-not-found"))?;
        let next_generation = self.registry.pause_binding(library_uuid)?;
        self.sync_coordinator
            .provider_manager()
            .fence_binding(
                &transport.plugin_id,
                &transport.account_uuid,
                transport.binding_epoch,
                next_generation as u64,
            )
            .await?;
        self.sync_coordinator
            .cancel_media_binding(transport.binding_epoch)
            .await?;
        Ok(())
    }

    pub async fn pause_restore(
        &self,
        restore_uuid: &str,
    ) -> anyhow::Result<crate::host_registry::SyncRestorePlan> {
        let plan = self
            .registry
            .restore_plan(restore_uuid)?
            .ok_or_else(|| anyhow::anyhow!("restore-not-found"))?;
        let revision = self.registry.snapshot()?.context_revision;
        self.pause_library_sync_with_revision(&plan.library_uuid, revision)
            .await?;
        self.registry.set_restore_state(restore_uuid, "paused")
    }

    pub async fn resume_restore(
        &self,
        restore_uuid: &str,
    ) -> anyhow::Result<crate::host_registry::SyncRestorePlan> {
        let plan = self
            .registry
            .restore_plan(restore_uuid)?
            .ok_or_else(|| anyhow::anyhow!("restore-not-found"))?;
        let revision = self.registry.snapshot()?.context_revision;
        self.resume_library_sync_with_revision(&plan.library_uuid, revision)
            .await?;
        self.registry.set_restore_state(restore_uuid, "restoring")
    }

    pub async fn cancel_restore(
        &self,
        restore_uuid: &str,
    ) -> anyhow::Result<crate::host_registry::SyncRestorePlan> {
        let _operation = self.operation.lock().await;
        let plan = self
            .registry
            .restore_plan(restore_uuid)?
            .context("restore-not-found")?;
        if plan.catalog_ready || plan.database_path.is_empty() {
            anyhow::bail!("restore-action-not-allowed");
        }
        self.registry
            .ensure_restore_cleanup_owned(restore_uuid, &plan.library_uuid)?;
        let descriptor = self
            .registry
            .library(&plan.library_uuid)?
            .context("library-not-found")?;
        verify_descriptor_identity(&descriptor)?;
        if read_marker(&descriptor.managed_root)?.creation_txn_uuid != restore_uuid {
            anyhow::bail!("restore-ownership-mismatch");
        }
        if fs::read_dir(&descriptor.local_data_dir)?.any(|entry| {
            entry.is_err() || entry.is_ok_and(|entry| entry.file_name() != MARKER_FILE_NAME)
        }) {
            anyhow::bail!("restore-action-not-allowed");
        }
        if let Some(transport) = self
            .registry
            .binding_transport(&plan.library_uuid, "library")?
        {
            let next = self
                .registry
                .begin_unbind(&plan.library_uuid, transport.binding_epoch)?;
            self.sync_coordinator
                .provider_manager()
                .fence_binding(
                    &transport.plugin_id,
                    &transport.account_uuid,
                    transport.binding_epoch,
                    next as u64,
                )
                .await?;
            self.sync_coordinator
                .cancel_media_binding(transport.binding_epoch)
                .await?;
        }
        for root in self.registry.restore_root_cleanup_entries(restore_uuid)? {
            let marker = crate::sync_folders::SyncRootMarker {
                format_version: 1,
                library_uuid: plan.library_uuid.clone(),
                root_uuid: root.root_uuid,
                root_folder_uuid: root.root_folder_uuid,
                local_binding_uuid: root.local_binding_uuid,
                binding_generation: root.binding_generation,
            };
            crate::sync_folders::remove_root_marker_if_matches(&root.target_path, &marker, false)?;
        }
        self.registry.update_creation_phase(
            restore_uuid,
            "restore-cancelling",
            false,
            true,
            None,
            None,
        )?;
        self.recover_creation(restore_uuid)?;
        self.registry
            .restore_plan(restore_uuid)?
            .context("restore-not-found")
    }

    pub async fn discard_restore_plan(
        &self,
        restore_uuid: &str,
    ) -> anyhow::Result<crate::host_registry::SyncRestorePlan> {
        let plan = self
            .registry
            .restore_plan(restore_uuid)?
            .ok_or_else(|| anyhow::anyhow!("restore-not-found"))?;
        if self.registry.library(&plan.library_uuid)?.is_some() {
            self.cancel_restore(restore_uuid).await
        } else {
            self.registry
                .commit_cancelled_restore_cleanup(restore_uuid, &plan.library_uuid)
        }
    }

    pub async fn resume_library_sync_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
    ) -> anyhow::Result<()> {
        let _operation = self.operation.lock().await;
        self.ensure_context_revision(expected_revision)?;
        let transport = self
            .registry
            .binding_transport(library_uuid, "library")?
            .ok_or_else(|| anyhow::anyhow!("sync-binding-not-found"))?;
        let _lifecycle = self.lock_plugin_lifecycle(&transport.plugin_id).await;
        self.registry
            .require_ready_sync_provider(&transport.plugin_id, &transport.account_uuid)?;
        let _work = self.provider_manager().begin_work(&transport.plugin_id)?;
        self.registry.resume_binding(library_uuid, &transport)?;
        self.sync_coordinator
            .resume_media_binding(transport.binding_epoch)
            .await;
        self.sync_coordinator.wake();
        Ok(())
    }

    pub async fn retry_library_sync_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
    ) -> anyhow::Result<()> {
        let _operation = self.operation.lock().await;
        self.ensure_context_revision(expected_revision)?;
        let transport = self
            .registry
            .binding_transport(library_uuid, "library")?
            .ok_or_else(|| anyhow::anyhow!("sync-binding-not-found"))?;
        let _lifecycle = self.lock_plugin_lifecycle(&transport.plugin_id).await;
        self.registry
            .require_ready_sync_provider(&transport.plugin_id, &transport.account_uuid)?;
        if self
            .sync_coordinator
            .provider_manager()
            .resolve(&transport.plugin_id, &transport.account_uuid)?
            .is_none()
        {
            anyhow::bail!("provider-not-ready");
        }
        let needs_capture =
            self.registry
                .retry_binding(library_uuid, expected_revision, &transport)?;
        if needs_capture {
            let result: anyhow::Result<()> = async {
                let active = self.active_snapshot();
                let generation = if active.library_uuid.as_deref() == Some(library_uuid) {
                    active
                        .runtime
                        .ok_or_else(|| anyhow::anyhow!("library-runtime-unavailable"))?
                        .writer
                        .write_authoritative(move |database| {
                            database
                                .begin_sync_capture(transport.binding_epoch, transport.actor_epoch)
                        })
                        .await?
                } else {
                    let descriptor = self
                        .registry
                        .library(library_uuid)?
                        .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
                    crate::sync::runtime::DormantSyncRuntime::new(
                        library_uuid,
                        descriptor.local_data_dir.join("catalog.sqlite"),
                        self.runtime_leases.clone(),
                    )
                    .run_turn(move |database| {
                        database.begin_sync_capture(transport.binding_epoch, transport.actor_epoch)
                    })
                    .await?
                };
                self.registry.set_sync_capture_generation(
                    library_uuid,
                    transport.binding_epoch,
                    generation,
                )
            }
            .await;
            if let Err(error) = result {
                self.registry
                    .record_sync_turn_error(library_uuid, "sync-capture-recovery-failed")?;
                return Err(error);
            }
        }
        self.sync_coordinator.wake();
        Ok(())
    }

    pub async fn wake_library_sync_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
    ) -> anyhow::Result<()> {
        let _operation = self.operation.lock().await;
        self.ensure_context_revision(expected_revision)?;
        let transport = self
            .registry
            .binding_transport(library_uuid, "library")?
            .ok_or_else(|| anyhow::anyhow!("sync-binding-not-found"))?;
        let _lifecycle = self.lock_plugin_lifecycle(&transport.plugin_id).await;
        self.registry
            .require_ready_sync_provider(&transport.plugin_id, &transport.account_uuid)?;
        self.registry.wake_binding(library_uuid)?;
        self.sync_coordinator.wake();
        Ok(())
    }

    pub async fn resolve_sync_conflict_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
        conflict_uuid: Uuid,
        resolution: &str,
    ) -> anyhow::Result<u64> {
        let _operation = self.operation.lock().await;
        self.ensure_context_revision(expected_revision)?;
        let descriptor = self
            .registry
            .library(library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        let state = match resolution {
            "resolve" => "resolved",
            "ignore" => "ignored",
            _ => anyhow::bail!("invalid-conflict-resolution"),
        };
        let conflict_bytes = conflict_uuid.as_bytes().to_vec();
        let state = state.to_owned();
        let now = crate::host_registry::unix_timestamp_millis_for_runtime();
        let update = move |database: &mut Database| -> anyhow::Result<u64> {
            if database.connection_for_fsops().execute(
                "UPDATE sync_conflicts SET state=?2,resolved_at=?3 WHERE conflict_uuid=?1 AND state='open'",
                rusqlite::params![conflict_bytes, state, now],
            )? == 0
            {
                anyhow::bail!("sync-conflict-not-found");
            }
            let remaining: i64 = database.connection_for_fsops().query_row(
                "SELECT COUNT(*) FROM sync_conflicts WHERE state='open'",
                [],
                |row| row.get(0),
            )?;
            Ok(remaining.max(0) as u64)
        };
        let active = self.active_snapshot();
        let remaining = if active.library_uuid.as_deref() == Some(library_uuid) {
            let runtime = active
                .runtime
                .ok_or_else(|| anyhow::anyhow!("library-runtime-unavailable"))?;
            runtime.writer.write_authoritative(update).await?
        } else {
            let runtime = crate::sync::runtime::DormantSyncRuntime::new(
                library_uuid,
                descriptor.local_data_dir.join("catalog.sqlite"),
                self.runtime_leases.clone(),
            );
            runtime.run_turn(update).await?
        };
        self.registry
            .update_library_conflict_count(library_uuid, remaining)?;
        Ok(remaining)
    }

    pub async fn commit_unbind_plan_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
        plan_uuid: &str,
        choice: &str,
    ) -> anyhow::Result<bool> {
        self.registry
            .validate_unbind_plan(library_uuid, plan_uuid)?;
        if choice == "drain-before-unbind" {
            let pending = self.registry.pending_sync_upload_count(library_uuid)?;
            if pending != 0 {
                self.sync_coordinator.wake();
                anyhow::bail!("sync-unbind-drain-pending:{pending}");
            }
        }
        self.registry
            .advance_unbind_plan(plan_uuid, choice, "cutover")?;
        self.unbind_library_sync_with_revision(library_uuid, expected_revision)
            .await?;
        self.registry
            .advance_unbind_plan(plan_uuid, choice, "reconciling")?;
        self.sync_coordinator.reconcile_pending_unbinds().await?;
        Ok(self.registry.unbind_plan_complete(plan_uuid)?)
    }

    pub async fn commit_reconnect_plan_with_revision(
        &self,
        library_uuid: &str,
        expected_revision: u64,
        plan_uuid: &str,
        choice: &str,
    ) -> anyhow::Result<()> {
        let _operation = self.operation.lock().await;
        self.ensure_context_revision(expected_revision)?;
        let descriptor = self
            .registry
            .library(library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        let committed = self.registry.commit_reconnect_plan(plan_uuid, choice)?;
        if committed != library_uuid {
            anyhow::bail!("reconnect-plan-library-mismatch");
        }
        let transport = self
            .registry
            .binding_transport(library_uuid, "library")?
            .ok_or_else(|| anyhow::anyhow!("sync-binding-not-found"))?;
        let active = self.active_snapshot();
        let capture = if active.library_uuid.as_deref() == Some(library_uuid) {
            let runtime = active
                .runtime
                .ok_or_else(|| anyhow::anyhow!("library-runtime-unavailable"))?;
            runtime
                .writer
                .write_authoritative(move |database| {
                    database.begin_sync_capture(transport.binding_epoch, transport.actor_epoch)
                })
                .await
        } else {
            let runtime = crate::sync::runtime::DormantSyncRuntime::new(
                library_uuid,
                descriptor.local_data_dir.join("catalog.sqlite"),
                self.runtime_leases.clone(),
            );
            runtime
                .run_turn(move |database| {
                    database.begin_sync_capture(transport.binding_epoch, transport.actor_epoch)
                })
                .await
        };
        match capture {
            Ok(generation) => self.registry.set_sync_capture_generation(
                library_uuid,
                transport.binding_epoch,
                generation,
            )?,
            Err(error) => {
                self.registry.rollback_reconnect_plan(
                    plan_uuid,
                    library_uuid,
                    transport.binding_epoch,
                )?;
                return Err(error);
            }
        }
        self.sync_coordinator.wake();
        Ok(())
    }

    pub(crate) fn ensure_context_revision(&self, expected_revision: u64) -> anyhow::Result<()> {
        let actual = self.active_snapshot().context_revision;
        if expected_revision != actual {
            anyhow::bail!("stale-library-context");
        }
        Ok(())
    }

    fn prepare_runtime(
        &self,
        descriptor: &LibraryDescriptor,
    ) -> anyhow::Result<Arc<LibraryRuntimeResources>> {
        verify_descriptor_identity(descriptor)?;
        let generation = self.runtime_generation.fetch_add(1, Ordering::AcqRel) + 1;
        let mut database = Database::open_verified(
            &descriptor.local_data_dir.join("catalog.sqlite"),
            Uuid::parse_str(&descriptor.library_uuid)?,
            self.registry.device_uuid()?,
        )?;
        self.refresh_storage_snapshot()?;
        database.attach_storage_snapshot(self.storage_snapshot.clone());
        database.attach_decoder_context(self.decoder_manager.clone(), CancellationToken::new());
        database.apply_migrations()?;
        database.initialize_sync_identity(
            Uuid::parse_str(&descriptor.library_uuid)?,
            Uuid::parse_str(&descriptor.root_uuid)?,
            self.registry.device_uuid()?,
        )?;
        for publish in self
            .registry
            .pending_publish_as_new(&descriptor.library_uuid)?
        {
            let remote = self
                .registry
                .remote_sync_root(&descriptor.library_uuid, &publish.new_root_uuid)?
                .context("root-not-found")?;
            let binding = self
                .registry
                .local_root_binding(&descriptor.library_uuid, &publish.new_root_uuid)?
                .context("local-root-binding-not-found")?;
            database.publish_root_as_new(
                Uuid::parse_str(&publish.operation_uuid)?,
                Uuid::parse_str(&publish.library_uuid)?,
                Uuid::parse_str(&publish.source_root_uuid)?,
                Uuid::parse_str(&publish.new_root_uuid)?,
                Uuid::parse_str(&publish.new_root_folder_uuid)?,
                &remote.requested_name,
                &remote.materialized_name,
                crate::sync::media::parse_hash(&remote.materialization_fingerprint)?,
                Uuid::parse_str(&publish.new_local_binding_uuid)?,
                Uuid::parse_str(&publish.root_capture_epoch)?,
                Uuid::parse_str(&publish.root_actor_epoch)?,
                binding.operation_revision,
            )?;
            crate::sync_folders::replace_root_marker_for_publish(
                Path::new(&binding.local_path),
                &publish.source_root_uuid,
                &crate::sync_folders::SyncRootMarker {
                    format_version: 1,
                    library_uuid: publish.library_uuid.clone(),
                    root_uuid: publish.new_root_uuid.clone(),
                    root_folder_uuid: publish.new_root_folder_uuid.clone(),
                    local_binding_uuid: publish.new_local_binding_uuid.clone(),
                    binding_generation: binding.binding_generation,
                },
            )?;
            self.registry
                .complete_publish_as_new(&publish.operation_uuid)?;
        }
        for host_binding in self
            .registry
            .local_root_bindings(&descriptor.library_uuid)?
        {
            let root_uuid = Uuid::parse_str(&host_binding.root_uuid)?;
            let remote = self
                .registry
                .remote_sync_root(&descriptor.library_uuid, &host_binding.root_uuid)?;
            if !database.sync_root_projection_ready(root_uuid.as_bytes())? {
                let remote = remote
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("root-not-found"))?;
                database.ensure_selected_sync_root(
                    Uuid::parse_str(&descriptor.library_uuid)?,
                    root_uuid,
                    Uuid::parse_str(&remote.root_folder_uuid)?,
                    Uuid::parse_str(&descriptor.root_uuid)?,
                    &host_binding.local_path,
                    &remote.requested_name,
                    &remote.materialized_name,
                    crate::sync::media::parse_hash(&remote.materialization_fingerprint)?,
                    Uuid::parse_str(&host_binding.local_binding_uuid)?,
                    Uuid::parse_str(
                        host_binding
                            .root_capture_epoch
                            .as_deref()
                            .unwrap_or(&host_binding.local_binding_uuid),
                    )?,
                    Uuid::parse_str(
                        host_binding
                            .root_actor_epoch
                            .as_deref()
                            .unwrap_or(&host_binding.local_binding_uuid),
                    )?,
                    host_binding.binding_generation,
                    &host_binding.state,
                    host_binding.operation_revision,
                    false,
                )?;
            }
            let root_id = database
                .sync_root_id(root_uuid.as_bytes())?
                .ok_or_else(|| anyhow::anyhow!("root-not-found"))?;
            let projected = SyncRootLocalBindingRecord {
                root_uuid: *root_uuid.as_bytes(),
                library_uuid: *Uuid::parse_str(&descriptor.library_uuid)?.as_bytes(),
                local_binding_uuid: *Uuid::parse_str(&host_binding.local_binding_uuid)?.as_bytes(),
                binding_generation: host_binding.binding_generation,
                root_capture_epoch: *Uuid::parse_str(
                    host_binding
                        .root_capture_epoch
                        .as_deref()
                        .unwrap_or(&host_binding.local_binding_uuid),
                )?
                .as_bytes(),
                root_actor_epoch: *Uuid::parse_str(
                    host_binding
                        .root_actor_epoch
                        .as_deref()
                        .unwrap_or(&host_binding.local_binding_uuid),
                )?
                .as_bytes(),
                state: host_binding.state.clone(),
                root_id,
                operation_revision: host_binding.operation_revision,
                updated_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as i64)
                    .unwrap_or(0),
            };
            let current = database.sync_root_local_binding(root_uuid.as_bytes())?;
            if host_binding.state == "local-only-fork"
                && current.as_ref().is_some_and(|current| {
                    current.local_binding_uuid == projected.local_binding_uuid
                        && current.binding_generation < projected.binding_generation
                })
            {
                let current = current.expect("checked above");
                database.fence_root_as_local_only(
                    root_uuid,
                    Uuid::from_bytes(current.local_binding_uuid),
                    current.binding_generation,
                    projected.binding_generation,
                    Uuid::from_bytes(projected.root_capture_epoch),
                    Uuid::from_bytes(projected.root_actor_epoch),
                    projected.operation_revision,
                )?;
            } else {
                database.upsert_sync_root_local_binding(&projected)?;
            }

            if let Some(remote) = remote {
                let local_path = Path::new(&host_binding.local_path);
                if local_path
                    .join(crate::sync_folders::ROOT_MARKER_FILE_NAME)
                    .is_file()
                {
                    if let Ok(marker) = crate::sync_folders::read_root_marker(local_path) {
                        if marker.library_uuid == descriptor.library_uuid
                            && marker.root_uuid == host_binding.root_uuid
                            && marker.root_folder_uuid == remote.root_folder_uuid
                            && (marker.local_binding_uuid != host_binding.local_binding_uuid
                                || marker.binding_generation != host_binding.binding_generation)
                        {
                            crate::sync_folders::replace_root_marker(
                                local_path,
                                &crate::sync_folders::SyncRootMarker {
                                    local_binding_uuid: host_binding.local_binding_uuid.clone(),
                                    binding_generation: host_binding.binding_generation,
                                    ..marker
                                },
                            )?;
                        }
                    }
                }
            }
        }
        let registry = Arc::clone(&self.registry);
        let coordinator = self.sync_coordinator.clone();
        let wake_library_uuid = descriptor.library_uuid.clone();
        let runtime = LibraryRuntimeResources::from_database_with_commit_hook(
            Some(descriptor.library_uuid.clone()),
            generation,
            database,
            Some(descriptor.local_data_dir.join("plugins")),
            Some(self.runtime_leases.try_acquire(&descriptor.library_uuid)?),
            Some(Arc::new(move || {
                let _ = registry.wake_binding(&wake_library_uuid);
                coordinator.wake();
            })),
        )?;
        runtime.root_runtime_registry.replace_all(
            self.registry
                .local_root_binding_tokens(&descriptor.library_uuid)?,
        );
        Ok(runtime)
    }
}

#[cfg(test)]
fn finish_restore_container_cleanup(
    descriptor: &LibraryDescriptor,
    local_quarantine: &Path,
    root_quarantine: &Path,
    local_identity: &str,
    root_identity: &str,
    created_root: bool,
) -> anyhow::Result<()> {
    // Empty checks before the asynchronous drain cannot grant ownership of
    // content added afterwards. Never recursively delete these containers.
    if safe_remove_empty_directory_expected(local_quarantine, local_identity).is_err() {
        safe_rename_directory_expected(
            local_quarantine,
            &descriptor.local_data_dir,
            local_identity,
        )?;
    }
    let marker_result = (|| -> anyhow::Result<()> {
        let root_guard = SafeDirectory::open(root_quarantine)?;
        if root_guard.identity() != root_identity {
            anyhow::bail!("path-changed");
        }
        root_guard.delete_file_child_expected(
            std::ffi::OsStr::new(MARKER_FILE_NAME),
            &descriptor.marker_identity,
        )?;
        Ok(())
    })();
    if let Err(error) = marker_result {
        safe_rename_directory_expected(root_quarantine, &descriptor.managed_root, root_identity)?;
        return Err(error);
    }
    if !created_root
        || safe_remove_empty_directory_expected(root_quarantine, root_identity).is_err()
    {
        safe_rename_directory_expected(root_quarantine, &descriptor.managed_root, root_identity)?;
    }
    Ok(())
}

// Host-local registration I/O is isolated from Tokio lifecycle coordination.
// This owner never modifies the selected root and never uses removal recovery.
struct LibraryRegistrationStorage {
    registry: Arc<HostRegistry>,
    libraries_dir: PathBuf,
}

impl LibraryRegistrationStorage {
    fn open(
        &self,
        database_path: PathBuf,
    ) -> anyhow::Result<(LibraryDescriptor, ExistingLibraryRoot)> {
        let normalized = crate::library_path::inspect_directory_path(&database_path, false)
            .map_err(|reason| anyhow::anyhow!(reason.as_str()))?;
        let mut root = ExistingLibraryRoot::open(&normalized)?;
        Database::inspect_existing_catalog(
            &root.path.join("catalog.sqlite"),
            Uuid::parse_str(&root.marker.library_uuid)?,
            self.registry.device_uuid()?,
        )?;
        let descriptor = if let Some(existing) = self.registry.library(&root.marker.library_uuid)? {
            if existing.root_uuid != root.marker.root_uuid
                || existing.marker_identity != root.marker_identity
                || !same_library_path(&existing.managed_root, &root.path)
                || existing.local_data_dir != existing.managed_root
            {
                anyhow::bail!("library-identity-conflict");
            }
            if existing.lifecycle_state != "ready" {
                anyhow::bail!("library-unavailable");
            }
            root.verify_unchanged()?;
            self.registry.show_library(&existing.library_uuid)?;
            existing
        } else {
            self.ensure_open_root_unclaimed(&root)?;
            let pending = self
                .registry
                .library_open_intents()?
                .into_iter()
                .find(|intent| intent.descriptor.library_uuid == root.marker.library_uuid);
            let intent = if let Some(intent) = pending {
                intent
            } else {
                let intent = LibraryOpenIntent {
                    descriptor: LibraryDescriptor {
                        library_uuid: root.marker.library_uuid.clone(),
                        root_uuid: root.marker.root_uuid.clone(),
                        name: root
                            .path
                            .parent()
                            .and_then(Path::file_name)
                            .and_then(|name| name.to_str())
                            .unwrap_or("Library")
                            .to_owned(),
                        icon: None,
                        color: None,
                        local_data_dir: root.path.clone(),
                        managed_root: root.path.clone(),
                        lifecycle_state: "ready".into(),
                        binding_state: "local-only".into(),
                        availability: "available".into(),
                        marker_identity: root.marker_identity.clone(),
                    },
                    root_identity: root.root_identity().to_owned(),
                    local_identity: None,
                    catalog_identity: None,
                };
                self.registry.begin_library_open(&intent)?;
                intent
            };
            self.finish_library_open(intent)?
        };
        root.verify_unchanged()?;
        Ok((descriptor, root))
    }

    fn ensure_open_root_unclaimed(&self, root: &ExistingLibraryRoot) -> anyhow::Result<()> {
        if crate::library_path::paths_overlap(&root.path, &self.libraries_dir) {
            anyhow::bail!("nested-library-root");
        }
        for existing in self.registry.snapshot()?.libraries {
            if existing.root_uuid == root.marker.root_uuid
                || existing.library_uuid == root.marker.library_uuid
            {
                anyhow::bail!("library-identity-conflict");
            }
            if crate::library_path::paths_overlap(&root.path, &existing.managed_root) {
                anyhow::bail!("nested-library-root");
            }
        }
        for intent in self.registry.library_open_intents()? {
            if intent.descriptor.library_uuid != root.marker.library_uuid
                && (intent.descriptor.root_uuid == root.marker.root_uuid
                    || crate::library_path::paths_overlap(
                        &root.path,
                        &intent.descriptor.managed_root,
                    ))
            {
                anyhow::bail!("library-open-conflict");
            }
        }
        Ok(())
    }

    fn finish_library_open(&self, intent: LibraryOpenIntent) -> anyhow::Result<LibraryDescriptor> {
        let descriptor = &intent.descriptor;
        let mut root = ExistingLibraryRoot::open(&descriptor.managed_root)?;
        if root.marker.library_uuid != descriptor.library_uuid
            || root.marker.root_uuid != descriptor.root_uuid
            || root.marker_identity != descriptor.marker_identity
            || root.root_identity() != intent.root_identity
            || descriptor.local_data_dir != descriptor.managed_root
        {
            anyhow::bail!("library-identity-conflict");
        }
        self.ensure_open_root_unclaimed(&root)?;
        Database::inspect_existing_catalog(
            &descriptor.local_data_dir.join("catalog.sqlite"),
            Uuid::parse_str(&descriptor.library_uuid)?,
            self.registry.device_uuid()?,
        )?;
        let directory = MarkerDirectoryGuard::open_readonly(&descriptor.local_data_dir)?;
        let catalog = directory.open_catalog()?;
        let identity = directory.file_identity(&catalog)?;
        if let Some(recorded) = intent.catalog_identity {
            if recorded != identity {
                anyhow::bail!("library-local-data-conflict");
            }
        } else {
            if intent.local_identity.is_none() {
                self.registry
                    .own_library_open_directory(&descriptor.library_uuid, directory.identity())?;
            }
            self.registry
                .own_library_open_catalog(&descriptor.library_uuid, &identity)?;
        }
        root.verify_unchanged()?;
        self.registry.commit_library_open(descriptor)?;
        Ok(intent.descriptor)
    }
}

fn cleanup_owned_creation_artifacts(artifacts: &[OwnedCreationArtifact]) -> anyhow::Result<()> {
    let mut visited = std::collections::HashSet::new();
    for artifact in artifacts.iter().rev() {
        if !visited.insert(&artifact.path) || !artifact.path.try_exists()? {
            continue;
        }
        let actual = crate::library_path::local_file_identity(&artifact.path)?;
        let owned = artifacts
            .iter()
            .find(|candidate| candidate.path == artifact.path && candidate.identity == actual)
            .context("path-changed")?;
        cleanup_owned_creation_artifact(owned)?;
    }
    Ok(())
}

fn cleanup_owned_creation_artifact(artifact: &OwnedCreationArtifact) -> anyhow::Result<()> {
    if !artifact.path.try_exists()? {
        return Ok(());
    }
    if artifact.directory {
        let guard = MarkerDirectoryGuard::open_readonly(&artifact.path)?;
        if guard.identity() != artifact.identity {
            anyhow::bail!("path-changed");
        }
        if fs::read_dir(&artifact.path)?.next().is_some() {
            return Ok(());
        }
        drop(guard);
        safe_remove_empty_directory_expected(&artifact.path, &artifact.identity)?;
    } else {
        let parent = MarkerDirectoryGuard::open(artifact.path.parent().context("invalid-path")?)?;
        parent.delete_file_child_expected(
            artifact.path.file_name().context("invalid-path")?,
            &artifact.identity,
        )?;
    }
    Ok(())
}

fn same_library_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

pub(crate) fn verify_descriptor_identity(descriptor: &LibraryDescriptor) -> anyhow::Result<()> {
    let marker = read_marker(&descriptor.managed_root)?;
    if marker.format_version != STORAGE_FORMAT_VERSION {
        anyhow::bail!("unsupported-library-marker");
    }
    if marker.state != MarkerState::Committed
        || marker.library_uuid != descriptor.library_uuid
        || marker.root_uuid != descriptor.root_uuid
    {
        anyhow::bail!("marker-mismatch");
    }
    let actual = marker_identity(&descriptor.managed_root.join(MARKER_FILE_NAME))?;
    if actual != descriptor.marker_identity {
        anyhow::bail!("marker-identity-mismatch");
    }
    Ok(())
}

fn relocation_reconciliation_succeeded(status: Option<&str>) -> anyhow::Result<bool> {
    match status {
        Some("succeeded") => Ok(true),
        Some("failed" | "cancelled") => anyhow::bail!("reconciliation-failed"),
        Some("pending" | "running") | None => Ok(false),
        Some(_) => anyhow::bail!("invalid-reconciliation-state"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_PLUGIN_ID: &str = "com.mangoo.fake-sync";
    const FAKE_ACCOUNT_ID: &str = "fake-account";

    #[tokio::test]
    async fn decoder_supervisor_restores_capability_after_cancelled_or_dropped_job() {
        use crate::plugins::decoder::{DecoderError, DecoderPurpose};
        use std::io::{Seek, SeekFrom, Write};
        let root = tempfile::tempdir().unwrap();
        let executable = std::env::current_exe().unwrap();
        let bundled = executable
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("bundled-plugins");
        let manager = Arc::new(isolated_manager(root.path(), bundled.clone()));
        let plugin_id = "com.mangoo.ppm-decoder";
        let plugin_root = bundled.join(plugin_id);
        let manifest = crate::plugins::parse_manifest(&plugin_root.join("plugin.json")).unwrap();
        let verified = crate::plugins::runtime::trust::verify_bundled_entry(
            &manifest,
            &plugin_root,
            platform(),
            crate::plugins::runtime::trust::embedded_trust_registry(),
        )
        .unwrap();
        // Unit binaries live in deps/, outside the packaged Core discovery
        // root. Seed the verified artifact as other supervisor fixtures do;
        // the API integration exercises discovery from the packaged binary.
        manager
            .registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id,
                version: &manifest.version,
                manifest_json: &serde_json::to_string(&manifest).unwrap(),
                entry_relative_path: manifest.entry_for(platform()).unwrap(),
                entry_sha256: &verified.expected_sha256(),
                install_root: &plugin_root,
                trust_policy_id: "embedded-v1",
                requested_permissions_json: &serde_json::to_string(&manifest.requested_permissions)
                    .unwrap(),
            })
            .unwrap();
        manager
            .registry
            .set_plugin_grants(
                plugin_id,
                &serde_json::to_string(&manifest.requested_permissions).unwrap(),
                true,
            )
            .unwrap();
        manager.finish_plugin_restart(plugin_id).await.unwrap();
        manager
            .registry
            .set_plugin_intent(plugin_id, true, true)
            .unwrap();
        manager.start_plugin_runtime_supervisors();
        let decoder = manager.decoder_manager();
        let epoch = manager.registry.plugin_lifecycle_epoch(plugin_id).unwrap();
        for drop_waiter in [false, true] {
            tokio::time::timeout(std::time::Duration::from_secs(8), async {
                while decoder.matching("ppm", DecoderPurpose::Thumbnail).is_none() {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("decoder initially ready");
            let before = decoder.snapshot().generation;
            let mut source = tempfile::tempfile().unwrap();
            source.write_all(b"P6\n1 1\n255\n\x11\x53\xd3").unwrap();
            let padding = [b' '; 256 * 1024];
            for _ in 0..192 {
                source.write_all(&padding).unwrap();
            }
            source.seek(SeekFrom::Start(0)).unwrap();
            let cancellation = CancellationToken::new();
            let job_manager = decoder.clone();
            let job_cancel = cancellation.clone();
            let job = tokio::spawn(async move {
                job_manager
                    .decode(source, "ppm", DecoderPurpose::Thumbnail, 1024, job_cancel)
                    .await
            });
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !decoder.job_dispatched_for_test(plugin_id) {
                    assert!(
                        !job.is_finished(),
                        "observe actual dispatch, not queue cancellation"
                    );
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            if drop_waiter {
                job.abort();
                assert!(job.await.is_err());
            } else {
                cancellation.cancel();
                assert!(matches!(job.await.unwrap(), Err(DecoderError::Cancelled)));
            }
            tokio::time::timeout(std::time::Duration::from_secs(8), async {
                loop {
                    let snapshot = decoder.snapshot();
                    if snapshot.generation > before
                        && snapshot
                            .matching("ppm", DecoderPurpose::Thumbnail)
                            .is_some()
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("normal cancellation must restore ready without a manual toggle");
            assert_eq!(
                manager.registry.plugin_lifecycle_epoch(plugin_id).unwrap(),
                epoch
            );
            assert_eq!(
                manager
                    .registry
                    .host_plugin(plugin_id)
                    .unwrap()
                    .unwrap()
                    .runtime_state,
                "ready"
            );
            let mut source = tempfile::tempfile().unwrap();
            source.write_all(b"P3\n1 1\n255\n17 83 211\n").unwrap();
            source.seek(SeekFrom::Start(0)).unwrap();
            let decoded = decoder
                .decode(
                    source,
                    "ppm",
                    DecoderPurpose::Thumbnail,
                    1024,
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            assert_eq!(decoded.rgba, [17, 83, 211, 255]);
            drop(decoded);
        }
        manager.shutdown_sync_coordinator().await.unwrap();
        assert!(decoder.snapshot().capabilities.is_empty());
    }

    fn isolated_manager(base: &Path, bundled_plugins_dir: PathBuf) -> LibraryManager {
        let registry = Arc::new(HostRegistry::open(&base.join("host.sqlite")).unwrap());
        LibraryManager::new(
            registry,
            base.join("libraries"),
            bundled_plugins_dir,
            Some(Arc::new(
                crate::plugins::secret_store::MemorySecretStore::default(),
            )),
            Arc::new(RwLock::new(ActiveSnapshot {
                library_uuid: None,
                context_revision: 0,
                runtime_generation: 0,
                runtime: None,
            })),
            Arc::new(tokio::sync::RwLock::new(())),
        )
    }

    fn test_manager(base: &Path) -> LibraryManager {
        isolated_manager(base, base.join("bundled-plugins"))
    }

    #[tokio::test]
    async fn separated_storage_old_active_with_pending_relocation_is_isolated_without_catalog_write(
    ) {
        let (base, manager, root_marker) = same_name_root_fixture("active").await;
        let descriptor = manager
            .registry
            .library(&root_marker.library_uuid)
            .unwrap()
            .unwrap();
        let binding = manager
            .registry
            .local_root_binding(&root_marker.library_uuid, &root_marker.root_uuid)
            .unwrap()
            .unwrap();
        let operation = Uuid::now_v7().to_string();
        let request = serde_json::json!({"targetPath":binding.local_path, "resumeState":"active"});
        manager
            .registry
            .record_automatic_root_rename_preflight_failure(
                &operation,
                &root_marker.library_uuid,
                &root_marker.root_uuid,
                &crate::sync_folders::request_hash(&request).unwrap(),
                &request,
                binding.operation_revision,
                binding.binding_generation,
                &"bb".repeat(32),
                Path::new(&binding.local_path),
            )
            .unwrap();
        let host = rusqlite::Connection::open(base.join("host.sqlite")).unwrap();
        host.execute(
            "UPDATE root_relocation_operations SET cleanup_state='pending' WHERE operation_uuid=?1",
            [&operation],
        )
        .unwrap();
        host.execute(
            "UPDATE host_meta SET active_library_uuid=?1 WHERE singleton=1",
            [&root_marker.library_uuid],
        )
        .unwrap();
        assert_eq!(
            manager.registry.pending_root_relocations().unwrap().len(),
            1
        );
        let mut marker = read_marker(&descriptor.managed_root).unwrap();
        marker.format_version = 1;
        fs::write(
            descriptor.managed_root.join(MARKER_FILE_NAME),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        let catalog = fs::read(descriptor.local_data_dir.join("catalog.sqlite")).unwrap();
        drop(host);
        drop(manager);
        let manager = test_manager(&base);
        manager.restore_active().await.unwrap();
        assert!(manager.active_snapshot().library_uuid.is_none());
        assert_eq!(
            manager
                .registry
                .library(&root_marker.library_uuid)
                .unwrap()
                .unwrap()
                .availability,
            "unavailable"
        );
        assert_eq!(
            manager.registry.pending_root_relocations().unwrap().len(),
            1
        );
        assert!(manager
            .write_sync_folder_catalog(&root_marker.library_uuid, |_| -> anyhow::Result<()> {
                panic!("unsupported Catalog must not open")
            })
            .await
            .is_err());
        assert_eq!(
            fs::read(descriptor.local_data_dir.join("catalog.sqlite")).unwrap(),
            catalog
        );
        drop(manager);
        fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn separated_storage_creation_failure_phases_recover_without_touching_existing_content() {
        for phase in ["marker-pending", "prepared", "marker-committed", "created"] {
            let base = tempfile::tempdir().unwrap();
            let manager = test_manager(base.path());
            let external = base.path().join("external");
            fs::create_dir(&external).unwrap();
            fs::write(external.join("keep.txt"), b"untouched").unwrap();
            let host = rusqlite::Connection::open(base.path().join("host.sqlite")).unwrap();
            host.execute_batch(&format!("CREATE TRIGGER fail_creation_phase BEFORE UPDATE OF phase ON library_creation_journal WHEN NEW.phase='{phase}' BEGIN SELECT RAISE(ABORT,'injected-journal-failure'); END;")).unwrap();
            let database = base.path().join("new/placement/Database");
            let files = base.path().join("new/placement/Files");
            let result = manager
                .create_library_with_content(
                    "Failure".into(),
                    None,
                    None,
                    database.clone(),
                    vec![
                        LibraryContentRootInput {
                            path: files.to_string_lossy().into_owned(),
                            create_if_missing: true,
                        },
                        LibraryContentRootInput {
                            path: external.to_string_lossy().into_owned(),
                            create_if_missing: false,
                        },
                    ],
                    None,
                    false,
                )
                .await;
            assert!(result.is_err(), "fault at {phase} must be observed");
            host.execute_batch("DROP TRIGGER fail_creation_phase")
                .unwrap();
            manager.recover_journals().unwrap();
            assert_eq!(fs::read(external.join("keep.txt")).unwrap(), b"untouched");
            assert!(manager
                .registry
                .pending_creation_recovery()
                .unwrap()
                .is_empty());
            if matches!(phase, "marker-committed" | "created") {
                assert_eq!(
                    read_marker(&database).unwrap().state,
                    MarkerState::Committed
                );
                assert_eq!(manager.registry.snapshot().unwrap().libraries.len(), 1);
                let catalog = Database::open_existing(database.join("catalog.sqlite")).unwrap();
                assert_eq!(catalog.list_roots().unwrap().len(), 2);
            } else {
                assert!(!database.exists());
                assert!(!files.exists());
                assert!(!base.path().join("new").exists());
                assert!(manager.registry.snapshot().unwrap().libraries.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn separated_storage_forward_cleanup_failure_remains_recoverable() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let (library, _, _) = manager
            .create_library(
                "Forward".into(),
                None,
                None,
                base.path().join("Database"),
                None,
                false,
            )
            .await
            .unwrap();
        let marker = read_marker(&library.managed_root).unwrap();
        let staging = library
            .managed_root
            .join(format!(".mangoo-create-{}", marker.creation_txn_uuid));
        fs::create_dir(&staging).unwrap();
        let original = crate::library_path::local_file_identity(&staging).unwrap();
        let mut owned = manager
            .registry
            .creation_artifacts(&marker.creation_txn_uuid)
            .unwrap();
        owned.push(OwnedCreationArtifact {
            path: staging.clone(),
            identity: original,
            directory: true,
            role: "staging".into(),
        });
        manager
            .registry
            .record_creation_artifacts(&marker.creation_txn_uuid, &owned)
            .unwrap();
        let parked = library.managed_root.join("parked-staging");
        fs::rename(&staging, &parked).unwrap();
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("catalog.sqlite-wal"), b"concurrent-user-file").unwrap();
        let host = rusqlite::Connection::open(base.path().join("host.sqlite")).unwrap();
        host.execute(
            "DELETE FROM libraries WHERE library_uuid=?1",
            [&library.library_uuid],
        )
        .unwrap();
        host.execute(
            "UPDATE library_creation_journal SET phase='marker-committed' WHERE txn_uuid=?1",
            [&marker.creation_txn_uuid],
        )
        .unwrap();
        assert!(manager.recover_creation(&marker.creation_txn_uuid).is_err());
        assert_eq!(
            manager.registry.pending_creation_recovery().unwrap().len(),
            1
        );
        assert!(manager
            .registry
            .library(&library.library_uuid)
            .unwrap()
            .is_none());
        assert_eq!(
            fs::read(staging.join("catalog.sqlite-wal")).unwrap(),
            b"concurrent-user-file"
        );
        let preserved = library.managed_root.join("preserved-user-directory");
        fs::rename(&staging, &preserved).unwrap();
        fs::rename(&parked, &staging).unwrap();
        manager.recover_creation(&marker.creation_txn_uuid).unwrap();
        assert!(manager
            .registry
            .pending_creation_recovery()
            .unwrap()
            .is_empty());
        assert!(manager
            .registry
            .library(&library.library_uuid)
            .unwrap()
            .is_some());
        assert!(!staging.exists());
        assert_eq!(
            fs::read(preserved.join("catalog.sqlite-wal")).unwrap(),
            b"concurrent-user-file"
        );
    }

    #[tokio::test]
    async fn separated_storage_unsupported_journals_are_preserved_even_with_equal_paths() {
        for version in [1, 99] {
            let base = tempfile::tempdir().unwrap();
            let manager = test_manager(base.path());
            let (library, _, _) = manager
                .create_library(
                    "Version".into(),
                    None,
                    None,
                    base.path().join("Database"),
                    None,
                    false,
                )
                .await
                .unwrap();
            let marker = read_marker(&library.managed_root).unwrap();
            manager
                .registry
                .begin_removal(&library, "unsupported-removal")
                .unwrap();
            let host = rusqlite::Connection::open(base.path().join("host.sqlite")).unwrap();
            host.execute("UPDATE library_removal_journal SET layout_version=?1 WHERE txn_uuid='unsupported-removal'", [version]).unwrap();
            host.execute("UPDATE library_creation_journal SET phase='marker-committed',layout_version=?1 WHERE txn_uuid=?2", rusqlite::params![version, marker.creation_txn_uuid]).unwrap();
            let bytes = fs::read(library.managed_root.join("catalog.sqlite")).unwrap();
            manager.recover_journals().unwrap();
            assert_eq!(
                manager.registry.pending_removal_recovery().unwrap()[0].layout_version,
                version
            );
            assert_eq!(
                manager.registry.pending_creation_recovery().unwrap()[0].layout_version,
                version
            );
            assert_eq!(
                fs::read(library.managed_root.join("catalog.sqlite")).unwrap(),
                bytes
            );
            assert_eq!(
                manager
                    .registry
                    .library(&library.library_uuid)
                    .unwrap()
                    .unwrap()
                    .lifecycle_state,
                "removing"
            );
        }
    }

    #[tokio::test]
    async fn separated_storage_remove_stop_failure_keeps_retry_ownership() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let (library, _, _) = manager
            .create_library(
                "Stop".into(),
                None,
                None,
                base.path().join("Database"),
                None,
                true,
            )
            .await
            .unwrap();
        let files = base.path().join("Database-Files");
        fs::write(files.join("keep.txt"), b"content").unwrap();
        manager
            .active_snapshot()
            .runtime
            .unwrap()
            .own_background_task(tokio::spawn(async {
                panic!("injected owned task failure")
            }));
        let revision = manager.active_snapshot().context_revision;
        let error = manager
            .remove_local_library_with_revision(&library.library_uuid, revision)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("library-stop-failed"));
        assert!(manager.pending_runtime_stop.lock().unwrap().is_some());
        assert!(library.local_data_dir.join("catalog.sqlite").exists());
        assert!(manager
            .registry
            .library(&library.library_uuid)
            .unwrap()
            .is_some());
        manager
            .remove_local_library_with_revision(&library.library_uuid, revision)
            .await
            .unwrap();
        assert!(manager.pending_runtime_stop.lock().unwrap().is_none());
        assert!(manager.active_snapshot().library_uuid.is_none());
        assert!(!library.local_data_dir.exists());
        assert_eq!(fs::read(files.join("keep.txt")).unwrap(), b"content");
    }

    #[cfg(windows)]
    #[test]
    fn separated_storage_catalog_pin_prevents_replacement_until_sqlite_closes() {
        let base = tempfile::tempdir().unwrap();
        let path = base.path().join("catalog.sqlite");
        let directory = MarkerDirectoryGuard::open(base.path()).unwrap();
        let new_pin = directory.create_catalog().unwrap();
        let identity = directory.file_identity(&new_pin).unwrap();
        assert!(fs::rename(&path, base.path().join("replaced.sqlite")).is_err());
        let database = Database::open_existing(&path).unwrap();
        database.apply_migrations().unwrap();
        drop(new_pin);
        assert!(fs::remove_file(&path).is_err());
        assert!(fs::rename(&path, base.path().join("replaced.sqlite")).is_err());
        let reopened = database.reopen().unwrap().unwrap();
        drop(database);
        assert!(fs::remove_file(&path).is_err());
        assert_eq!(
            crate::library_path::local_file_identity(&path).unwrap(),
            identity
        );
        drop(reopened);
        fs::rename(&path, base.path().join("after-close.sqlite")).unwrap();
    }

    #[tokio::test]
    async fn separated_storage_creates_two_libraries_with_independent_populated_roots() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let external = base.path().join("external");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("user.txt"), b"keep").unwrap();
        for name in ["First", "Second"] {
            let parent = base.path().join("Documents/Mangoo/Libraries").join(name);
            let database = parent.join("Database");
            let roots = vec![
                LibraryContentRootInput {
                    path: parent.join("Files").to_string_lossy().into_owned(),
                    create_if_missing: true,
                },
                LibraryContentRootInput {
                    path: external.to_string_lossy().into_owned(),
                    create_if_missing: false,
                },
            ];
            let preview = manager.preflight_plan(name, &database, &roots).unwrap();
            assert!(preview.ok, "{:?}", preview.reason);
            assert!(!parent.exists(), "preflight must not write");
            let (library, activated, _) = manager
                .create_library_with_content(
                    name.into(),
                    None,
                    None,
                    database.clone(),
                    roots,
                    preview.token,
                    false,
                )
                .await
                .unwrap();
            assert!(!activated);
            assert_eq!(library.local_data_dir, library.managed_root);
            let marker = read_marker(&database).unwrap();
            assert_eq!(marker.format_version, 2);
            let catalog = Database::open_existing(database.join("catalog.sqlite")).unwrap();
            let paths = catalog.list_roots().unwrap();
            assert_eq!(paths.len(), 2);
            assert!(paths
                .iter()
                .all(|root| Path::new(&root.path) != library.managed_root));
            let root_ids: Vec<Vec<u8>> = catalog
                .connection_for_fsops()
                .prepare("SELECT root_uuid FROM roots ORDER BY id")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert!(
                root_ids
                    .iter()
                    .all(|id| id.as_slice()
                        != Uuid::parse_str(&library.root_uuid).unwrap().as_bytes())
            );
            assert_ne!(root_ids[0], root_ids[1]);
            assert_eq!(fs::read(external.join("user.txt")).unwrap(), b"keep");
        }
        assert_eq!(manager.registry.snapshot().unwrap().libraries.len(), 2);
    }

    #[tokio::test]
    async fn separated_storage_rejects_empty_overlap_and_stale_whole_plan_without_effects() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let database = base.path().join("Library/Database");
        assert_eq!(
            manager
                .preflight_plan("Library", &database, &[])
                .unwrap()
                .reason,
            crate::library_path::PreflightReason::ContentRootsRequired
        );
        let root = LibraryContentRootInput {
            path: base
                .path()
                .join("Library/Files")
                .to_string_lossy()
                .into_owned(),
            create_if_missing: true,
        };
        let preview = manager
            .preflight_plan("Library", &database, &[root.clone()])
            .unwrap();
        let error = manager
            .create_library_with_content(
                "Different".into(),
                None,
                None,
                database.clone(),
                vec![root.clone()],
                preview.token,
                false,
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "path-changed");
        assert!(!database.parent().unwrap().exists());
        let overlap = manager
            .preflight_plan("Library", &database, &[root.clone(), root.clone()])
            .unwrap();
        assert_eq!(
            overlap.reason,
            crate::library_path::PreflightReason::ContentRootsOverlap
        );
        assert_eq!(overlap.root_index, Some(1));
        assert!(overlap.normalized_content_roots.is_none());
        let (library, _, _) = manager
            .create_library_with_content(
                "Library".into(),
                None,
                None,
                database.clone(),
                vec![root.clone()],
                None,
                false,
            )
            .await
            .unwrap();
        for forbidden in [
            &database,
            database.parent().unwrap(),
            &database.join("subfolder"),
        ] {
            assert!(manager
                .admit_content_path(forbidden, true)
                .unwrap_err()
                .to_string()
                .contains("database-content-overlap"));
        }
        let blocked = manager
            .preflight_plan(
                "Other",
                &Path::new(&root.path).join("Database"),
                &[LibraryContentRootInput {
                    path: base.path().join("Elsewhere").to_string_lossy().into_owned(),
                    create_if_missing: true,
                }],
            )
            .unwrap();
        assert_eq!(
            blocked.reason,
            crate::library_path::PreflightReason::DatabaseContentOverlap
        );
        assert_eq!(blocked.path_role, Some("database"));
        assert!(manager
            .create_library_with_content(
                "Collision".into(),
                None,
                None,
                database.clone(),
                vec![root],
                None,
                false
            )
            .await
            .is_err());
        assert_eq!(
            read_marker(&database).unwrap().library_uuid,
            library.library_uuid
        );
    }

    #[tokio::test]
    async fn separated_storage_open_checks_existing_catalog_and_foreign_device_without_writes() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let (library, _, _) = manager
            .create_library(
                "Library".into(),
                None,
                None,
                base.path().join("Database"),
                None,
                false,
            )
            .await
            .unwrap();
        let foreign = tempfile::tempdir().unwrap();
        let other = test_manager(foreign.path());
        let error = other
            .open_library_with_revision(library.managed_root.clone(), 0)
            .await
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "library-device-identity-mismatch");
        assert!(other.registry.snapshot().unwrap().libraries.is_empty());
        fs::remove_file(library.local_data_dir.join("catalog.sqlite")).unwrap();
        assert!(manager
            .open_library_with_revision(library.managed_root.clone(), 0)
            .await
            .is_err());
        assert!(!library.local_data_dir.join("catalog.sqlite").exists());
        fs::write(
            library.local_data_dir.join("catalog.sqlite"),
            b"corrupt-user-bytes",
        )
        .unwrap();
        assert!(manager
            .open_library_with_revision(library.managed_root.clone(), 0)
            .await
            .is_err());
        assert_eq!(
            fs::read(library.local_data_dir.join("catalog.sqlite")).unwrap(),
            b"corrupt-user-bytes"
        );
    }

    #[tokio::test]
    async fn separated_storage_removal_preserves_sibling_and_external_content() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let files = base.path().join("Library/Files");
        let external = base.path().join("external");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("original.txt"), b"external").unwrap();
        let roots = vec![
            LibraryContentRootInput {
                path: files.to_string_lossy().into_owned(),
                create_if_missing: true,
            },
            LibraryContentRootInput {
                path: external.to_string_lossy().into_owned(),
                create_if_missing: false,
            },
        ];
        let (library, _, _) = manager
            .create_library_with_content(
                "Library".into(),
                None,
                None,
                base.path().join("Library/Database"),
                roots,
                None,
                false,
            )
            .await
            .unwrap();
        fs::write(files.join("original.txt"), b"sibling").unwrap();
        manager
            .remove_local_library_with_revision(&library.library_uuid, 0)
            .await
            .unwrap();
        assert!(!library.local_data_dir.exists());
        assert_eq!(fs::read(files.join("original.txt")).unwrap(), b"sibling");
        assert_eq!(
            fs::read(external.join("original.txt")).unwrap(),
            b"external"
        );
        assert!(manager.registry.snapshot().unwrap().libraries.is_empty());
    }

    #[tokio::test]
    async fn safe_library_forget_preserves_active_inactive_last_catalog_and_restart() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let first = manager
            .create_library(
                "First".into(),
                Some("folder".into()),
                Some("#123456".into()),
                base.path().join("first"),
                None,
                true,
            )
            .await
            .unwrap()
            .0;
        let second = manager
            .create_library(
                "Second".into(),
                None,
                None,
                base.path().join("second"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;
        let marker = fs::read(first.managed_root.join(MARKER_FILE_NAME)).unwrap();
        fs::write(
            first.managed_root.join("media.bin"),
            b"original media\0\xff",
        )
        .unwrap();
        fs::write(
            first.local_data_dir.join("cache-sentinel"),
            b"retained cache",
        )
        .unwrap();
        manager.active_snapshot().runtime.unwrap().writer.write(|db| {
            db.connection_for_fsops().execute_batch("CREATE TABLE retained_user_data(value TEXT); INSERT INTO retained_user_data VALUES ('original catalog');")?;
            Ok(())
        }).await.unwrap();
        let revision = manager.active_snapshot().context_revision;
        manager
            .forget_library_with_revision(&second.library_uuid, revision)
            .await
            .unwrap();
        assert_eq!(
            manager.active_snapshot().library_uuid.as_deref(),
            Some(first.library_uuid.as_str())
        );
        assert_eq!(manager.active_snapshot().context_revision, revision);
        assert_eq!(
            manager.registry.listed_snapshot().unwrap().libraries.len(),
            1
        );
        manager
            .forget_library_with_revision(&first.library_uuid, revision)
            .await
            .unwrap();
        assert!(manager.active_snapshot().library_uuid.is_none());
        assert!(manager.active_snapshot().runtime.is_none());
        assert_eq!(manager.active_snapshot().context_revision, revision + 1);
        assert!(manager
            .registry
            .listed_snapshot()
            .unwrap()
            .libraries
            .is_empty());
        assert_eq!(manager.registry.snapshot().unwrap().libraries.len(), 2);
        assert_eq!(
            fs::read(first.managed_root.join(MARKER_FILE_NAME)).unwrap(),
            marker
        );
        assert_eq!(
            fs::read(first.managed_root.join("media.bin")).unwrap(),
            b"original media\0\xff"
        );
        assert_eq!(
            fs::read(first.local_data_dir.join("cache-sentinel")).unwrap(),
            b"retained cache"
        );
        let db = Database::open(first.local_data_dir.join("catalog.sqlite")).unwrap();
        assert_eq!(
            db.connection_for_fsops()
                .query_row("SELECT value FROM retained_user_data", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "original catalog"
        );
        drop(db);
        assert_eq!(
            manager
                .activate_with_revision(&first.library_uuid, revision + 1)
                .await
                .unwrap_err()
                .to_string(),
            "library-not-listed"
        );
        manager
            .forget_library_with_revision(&first.library_uuid, revision + 1)
            .await
            .unwrap();
        assert!(
            !manager
                .preflight("Nested", &first.managed_root.join("nested"))
                .unwrap()
                .ok
        );
        drop(manager);

        let reopened = test_manager(base.path());
        reopened.restore_active().await.unwrap();
        assert!(reopened.active_snapshot().library_uuid.is_none());
        assert!(reopened
            .registry
            .listed_snapshot()
            .unwrap()
            .libraries
            .is_empty());
        let (restored, activated, error) = reopened
            .open_library_with_revision(first.managed_root.clone(), revision + 1)
            .await
            .unwrap();
        assert!(activated, "{error:?}");
        assert_eq!(restored.library_uuid, first.library_uuid);
        assert_eq!(restored.local_data_dir, first.local_data_dir);
        assert_eq!(restored.icon, first.icon);
        assert_eq!(restored.color, first.color);
        let before = reopened.active_snapshot();
        reopened
            .open_library_with_revision(first.managed_root.clone(), before.context_revision)
            .await
            .unwrap();
        assert_eq!(
            reopened.active_snapshot().runtime_generation,
            before.runtime_generation
        );
        assert_eq!(
            reopened.active_snapshot().context_revision,
            before.context_revision
        );
        assert_eq!(
            reopened.registry.listed_snapshot().unwrap().libraries.len(),
            1
        );
        assert_eq!(
            fs::read(first.managed_root.join(MARKER_FILE_NAME)).unwrap(),
            marker
        );
        assert_eq!(
            fs::read(first.local_data_dir.join("cache-sentinel")).unwrap(),
            b"retained cache"
        );
        reopened
            .active_snapshot()
            .runtime
            .unwrap()
            .writer
            .write(|db| {
                assert_eq!(
                    db.connection_for_fsops().query_row(
                        "SELECT value FROM retained_user_data",
                        [],
                        |row| row.get::<_, String>(0)
                    )?,
                    "original catalog"
                );
                Ok(())
            })
            .await
            .unwrap();
        reopened
            .active_snapshot()
            .runtime
            .unwrap()
            .shutdown()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn safe_library_foreign_host_rejects_device_identity_and_preserves_all_bytes() {
        let base = tempfile::tempdir().unwrap();
        let origin = test_manager(&base.path().join("origin"));
        let (descriptor, _, _) = origin
            .create_library(
                "Origin name".into(),
                Some("star".into()),
                Some("#123456".into()),
                base.path().join("shared"),
                None,
                false,
            )
            .await
            .unwrap();
        fs::write(descriptor.managed_root.join("media.bin"), b"media").unwrap();
        let marker = fs::read(descriptor.managed_root.join(MARKER_FILE_NAME)).unwrap();
        let catalog = fs::read(descriptor.local_data_dir.join("catalog.sqlite")).unwrap();
        for restart in [false, true] {
            let host = test_manager(&base.path().join("other-host"));
            if restart {
                host.restore_active().await.unwrap();
            }
            let error = host
                .open_library_with_revision(descriptor.managed_root.clone(), 0)
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "library-device-identity-mismatch");
            assert!(host.registry.snapshot().unwrap().libraries.is_empty());
            assert!(host.registry.library_open_intents().unwrap().is_empty());
            assert!(host.active_snapshot().library_uuid.is_none());
            assert_eq!(
                fs::read(descriptor.managed_root.join(MARKER_FILE_NAME)).unwrap(),
                marker
            );
            assert_eq!(
                fs::read(descriptor.local_data_dir.join("catalog.sqlite")).unwrap(),
                catalog
            );
            assert_eq!(
                fs::read(descriptor.managed_root.join("media.bin")).unwrap(),
                b"media"
            );
        }
    }

    #[tokio::test]
    async fn safe_library_open_rejects_invalid_conflicting_and_unowned_paths_without_writes() {
        let base = tempfile::tempdir().unwrap();
        let origin = test_manager(&base.path().join("origin"));
        let descriptor = origin
            .create_library(
                "Source".into(),
                None,
                None,
                base.path().join("source"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;
        let host = test_manager(&base.path().join("host"));
        for path in [base.path().join("missing"), base.path().join("ordinary")] {
            if path.ends_with("ordinary") {
                fs::create_dir(&path).unwrap();
                fs::write(path.join("keep"), b"untouched").unwrap();
            }
            assert!(host.open_library_with_revision(path, 0).await.is_err());
        }
        let marker_path = descriptor.managed_root.join(MARKER_FILE_NAME);
        let original = fs::read(&marker_path).unwrap();
        for kind in ["unsupported", "pending", "noncanonical", "oversized"] {
            let mut marker = read_marker(&descriptor.managed_root).unwrap();
            match kind {
                "unsupported" => marker.format_version = 99,
                "pending" => marker.state = MarkerState::Pending,
                "noncanonical" => marker.library_uuid = marker.library_uuid.replace('-', ""),
                _ => {}
            }
            let bytes = if kind == "oversized" {
                vec![b' '; 16 * 1024 + 1]
            } else {
                serde_json::to_vec(&marker).unwrap()
            };
            fs::write(&marker_path, &bytes).unwrap();
            assert!(host
                .open_library_with_revision(descriptor.managed_root.clone(), 0)
                .await
                .is_err());
            assert_eq!(fs::read(&marker_path).unwrap(), bytes);
            fs::write(&marker_path, &original).unwrap();
        }
        // Unowned local data may be valuable: no adoption, migration or deletion.
        let collision = host.libraries_dir.join(&descriptor.library_uuid);
        fs::create_dir_all(&collision).unwrap();
        fs::write(
            collision.join("catalog.sqlite"),
            b"valuable unknown catalog",
        )
        .unwrap();
        assert_eq!(
            host.open_library_with_revision(descriptor.managed_root.clone(), 0)
                .await
                .unwrap_err()
                .to_string(),
            "library-device-identity-mismatch"
        );
        assert_eq!(
            fs::read(collision.join("catalog.sqlite")).unwrap(),
            b"valuable unknown catalog"
        );
        assert!(host.registry.snapshot().unwrap().libraries.is_empty());

        let clone = base.path().join("clone");
        fs::create_dir(&clone).unwrap();
        fs::write(clone.join(MARKER_FILE_NAME), &original).unwrap();
        assert_eq!(
            origin
                .open_library_with_revision(clone, 0)
                .await
                .unwrap_err()
                .to_string(),
            "library-local-data-unavailable"
        );
        origin
            .forget_library_with_revision(&descriptor.library_uuid, 0)
            .await
            .unwrap();
        let nested = descriptor.managed_root.join("nested");
        fs::create_dir(&nested).unwrap();
        let mut marker = read_marker(&descriptor.managed_root).unwrap();
        marker.library_uuid = Uuid::now_v7().to_string();
        marker.root_uuid = Uuid::now_v7().to_string();
        fs::write(
            nested.join(MARKER_FILE_NAME),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        assert_eq!(
            origin
                .open_library_with_revision(nested, 0)
                .await
                .unwrap_err()
                .to_string(),
            "library-local-data-unavailable"
        );
        assert_eq!(fs::read(&marker_path).unwrap(), original);
    }

    #[tokio::test]
    async fn safe_library_forget_commit_failure_restores_previous_runtime() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let descriptor = manager
            .create_library(
                "Active".into(),
                None,
                None,
                base.path().join("active"),
                None,
                true,
            )
            .await
            .unwrap()
            .0;
        let before = manager.active_snapshot();
        let injection = rusqlite::Connection::open(base.path().join("host.sqlite")).unwrap();
        injection.execute_batch("CREATE TRIGGER fail_forget BEFORE INSERT ON library_list_visibility BEGIN SELECT RAISE(FAIL,'simulated persistence failure'); END;").unwrap();
        assert!(manager
            .forget_library_with_revision(&descriptor.library_uuid, before.context_revision)
            .await
            .is_err());
        let after = manager.active_snapshot();
        assert_eq!(after.library_uuid, before.library_uuid);
        assert_eq!(after.context_revision, before.context_revision);
        assert!(after.runtime.is_some());
        assert!(manager
            .registry
            .is_library_listed(&descriptor.library_uuid)
            .unwrap());
        injection.execute_batch("DROP TRIGGER fail_forget").unwrap();
        manager
            .forget_library_with_revision(&descriptor.library_uuid, before.context_revision)
            .await
            .unwrap();
        assert!(descriptor.managed_root.is_dir());
        assert!(descriptor.local_data_dir.join("catalog.sqlite").is_file());
    }

    #[tokio::test]
    async fn safe_library_open_and_forget_serialization_rechecks_context() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let descriptor = manager
            .create_library(
                "Active".into(),
                None,
                None,
                base.path().join("active"),
                None,
                true,
            )
            .await
            .unwrap()
            .0;
        let revision = manager.active_snapshot().context_revision;
        let (forgotten, switched) = tokio::join!(
            manager.forget_library_with_revision(&descriptor.library_uuid, revision),
            manager.activate_with_revision(&descriptor.library_uuid, revision)
        );
        forgotten.unwrap();
        assert_eq!(switched.unwrap_err().to_string(), "stale-library-context");
        let (one, two) = tokio::join!(
            manager.open_library_with_revision(descriptor.managed_root.clone(), revision + 1),
            manager.open_library_with_revision(descriptor.managed_root.clone(), revision + 1)
        );
        assert!(one.unwrap().1);
        assert_eq!(two.unwrap_err().to_string(), "stale-library-context");
        assert_eq!(manager.registry.snapshot().unwrap().libraries.len(), 1);
        manager
            .active_snapshot()
            .runtime
            .unwrap()
            .shutdown()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn safe_library_open_recovers_existing_registration_and_preserves_unsupported_intent() {
        for supported in [true, false] {
            let base = tempfile::tempdir().unwrap();
            let manager = test_manager(base.path());
            let (source, _, _) = manager
                .create_library(
                    "Source".into(),
                    None,
                    None,
                    base.path().join("Database"),
                    None,
                    false,
                )
                .await
                .unwrap();
            let marker = fs::read(source.managed_root.join(MARKER_FILE_NAME)).unwrap();
            let catalog = fs::read(source.local_data_dir.join("catalog.sqlite")).unwrap();
            let host = rusqlite::Connection::open(base.path().join("host.sqlite")).unwrap();
            host.execute(
                "DELETE FROM libraries WHERE library_uuid=?1",
                [&source.library_uuid],
            )
            .unwrap();
            let root = ExistingLibraryRoot::open(&source.managed_root).unwrap();
            let mut descriptor = source.clone();
            if !supported {
                descriptor.local_data_dir = base.path().join("old-host-data");
                fs::create_dir(&descriptor.local_data_dir).unwrap();
                fs::write(
                    descriptor.local_data_dir.join("catalog.sqlite"),
                    b"unowned crash-gap sentinel",
                )
                .unwrap();
            }
            manager
                .registry
                .begin_library_open(&LibraryOpenIntent {
                    descriptor: descriptor.clone(),
                    root_identity: root.root_identity().into(),
                    local_identity: None,
                    catalog_identity: None,
                })
                .unwrap();
            drop(root);
            drop(host);
            drop(manager);
            let restarted = test_manager(base.path());
            restarted.restore_active().await.unwrap();
            assert!(restarted.active_snapshot().library_uuid.is_none());
            assert_eq!(
                restarted
                    .registry
                    .listed_snapshot()
                    .unwrap()
                    .libraries
                    .len(),
                usize::from(supported)
            );
            assert_eq!(
                restarted
                    .registry
                    .library_open_intents()
                    .unwrap()
                    .is_empty(),
                supported
            );
            if supported {
                assert_eq!(
                    restarted
                        .registry
                        .library(&source.library_uuid)
                        .unwrap()
                        .unwrap()
                        .local_data_dir,
                    source.local_data_dir
                );
            } else {
                assert_eq!(
                    fs::read(descriptor.local_data_dir.join("catalog.sqlite")).unwrap(),
                    b"unowned crash-gap sentinel"
                );
            }
            assert_eq!(
                fs::read(source.managed_root.join(MARKER_FILE_NAME)).unwrap(),
                marker
            );
            assert_eq!(
                fs::read(source.local_data_dir.join("catalog.sqlite")).unwrap(),
                catalog
            );
        }
    }

    #[tokio::test]
    async fn safe_library_open_partial_activation_keeps_registration_and_previous_context() {
        let base = tempfile::tempdir().unwrap();
        let host = test_manager(&base.path().join("host"));
        let source = host
            .create_library(
                "Source".into(),
                None,
                None,
                base.path().join("source"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;
        host.forget_library_with_revision(&source.library_uuid, 0)
            .await
            .unwrap();
        let active = host
            .create_library(
                "Previous".into(),
                None,
                None,
                base.path().join("previous"),
                None,
                true,
            )
            .await
            .unwrap()
            .0;
        let revision = host.active_snapshot().context_revision;
        let lease = host
            .runtime_leases
            .try_acquire(&source.library_uuid)
            .unwrap();
        let (opened, activated, error) = host
            .open_library_with_revision(source.managed_root.clone(), revision)
            .await
            .unwrap();
        assert!(!activated);
        assert!(error.is_some());
        assert_eq!(opened.library_uuid, source.library_uuid);
        assert!(host
            .registry
            .is_library_listed(&source.library_uuid)
            .unwrap());
        assert_eq!(
            host.active_snapshot().library_uuid.as_deref(),
            Some(active.library_uuid.as_str())
        );
        assert_eq!(host.active_snapshot().context_revision, revision);
        assert!(host.active_snapshot().runtime.is_some());
        drop(lease);
        assert!(
            host.open_library_with_revision(source.managed_root, revision)
                .await
                .unwrap()
                .1
        );
        host.active_snapshot()
            .runtime
            .unwrap()
            .shutdown()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn safe_library_forget_stop_failure_stays_visible_and_retry_finishes_stop() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let descriptor = manager
            .create_library(
                "Active".into(),
                None,
                None,
                base.path().join("active"),
                None,
                true,
            )
            .await
            .unwrap()
            .0;
        manager
            .active_snapshot()
            .runtime
            .unwrap()
            .own_background_task(tokio::spawn(async {
                panic!("simulated owned task failure");
            }));
        let revision = manager.active_snapshot().context_revision;
        let error = manager
            .forget_library_with_revision(&descriptor.library_uuid, revision)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("library-stop-failed"));
        assert!(manager
            .registry
            .is_library_listed(&descriptor.library_uuid)
            .unwrap());
        assert_eq!(
            manager
                .registry
                .snapshot()
                .unwrap()
                .active_library_uuid
                .as_deref(),
            Some(descriptor.library_uuid.as_str())
        );
        assert!(manager.active_snapshot().runtime.is_none());
        assert!(manager.pending_runtime_stop.lock().unwrap().is_some());
        manager
            .forget_library_with_revision(&descriptor.library_uuid, revision)
            .await
            .unwrap();
        assert!(manager.pending_runtime_stop.lock().unwrap().is_none());
        assert!(manager.active_snapshot().library_uuid.is_none());
        assert!(descriptor.local_data_dir.join("catalog.sqlite").is_file());
    }

    #[tokio::test]
    async fn safe_library_forget_retains_sync_binding_and_background_schedule() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let library = manager
            .create_library(
                "Synced".into(),
                None,
                None,
                base.path().join("synced"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;
        manager
            .registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: "org.example.retained",
                version: "1.0.0",
                manifest_json: "{}",
                entry_relative_path: "provider",
                entry_sha256: &[0; 32],
                install_root: base.path(),
                trust_policy_id: "embedded-v1",
                requested_permissions_json: "[]",
            })
            .unwrap();
        manager
            .registry
            .upsert_plugin_account(
                "retained-account",
                "org.example.retained",
                Some("Retained"),
                "authenticated",
            )
            .unwrap();
        manager
            .registry
            .set_plugin_intent("org.example.retained", true, true)
            .unwrap();
        manager
            .registry
            .record_plugin_runtime_ready(
                "org.example.retained",
                "retained-session",
                &["sync-provider.v1".to_owned()],
            )
            .unwrap();
        manager
            .registry
            .begin_sync_binding(
                &library.library_uuid,
                "org.example.retained",
                "retained-account",
            )
            .unwrap();
        manager
            .registry
            .set_binding_state(&library.library_uuid, "active")
            .unwrap();
        let before = manager
            .registry
            .binding_transport(&library.library_uuid, "library")
            .unwrap()
            .unwrap();
        manager
            .forget_library_with_revision(&library.library_uuid, 0)
            .await
            .unwrap();
        assert!(!manager
            .registry
            .is_library_listed(&library.library_uuid)
            .unwrap());
        let after = manager
            .registry
            .binding_transport(&library.library_uuid, "library")
            .unwrap()
            .unwrap();
        assert_eq!(after.state, before.state);
        assert_eq!(after.binding_epoch, before.binding_epoch);
        assert_eq!(after.transport_generation, before.transport_generation);
        assert_eq!(after.account_uuid, before.account_uuid);
        assert!(manager
            .registry
            .sync_bound_libraries()
            .unwrap()
            .iter()
            .any(|item| item.library_uuid == library.library_uuid));
        let (opened, activated, error) = manager
            .open_library_with_revision(library.managed_root, 0)
            .await
            .unwrap();
        assert!(activated, "{error:?}");
        assert_eq!(opened.binding_state, "synchronized");
        assert_eq!(
            manager
                .registry
                .binding_transport(&library.library_uuid, "library")
                .unwrap()
                .unwrap()
                .binding_epoch,
            before.binding_epoch
        );
        manager
            .active_snapshot()
            .runtime
            .unwrap()
            .shutdown()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn safe_library_open_failed_previous_stop_retains_all_owned_work() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let previous = manager
            .create_library(
                "Previous".into(),
                None,
                None,
                base.path().join("previous"),
                None,
                true,
            )
            .await
            .unwrap()
            .0;
        let target = manager
            .create_library(
                "Target".into(),
                None,
                None,
                base.path().join("target"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;
        let runtime = manager.active_snapshot().runtime.unwrap();
        let cancelled = runtime.cancellation_token();
        let joined = Arc::new(AtomicBool::new(false));
        let finished = Arc::clone(&joined);
        runtime.own_background_task(tokio::spawn(async {
            panic!("simulated stop failure");
        }));
        runtime.own_background_task(tokio::spawn(async move {
            cancelled.cancelled().await;
            tokio::task::yield_now().await;
            finished.store(true, Ordering::Release);
        }));
        let revision = manager.active_snapshot().context_revision;
        let (_, activated, error) = manager
            .open_library_with_revision(target.managed_root.clone(), revision)
            .await
            .unwrap();
        assert!(!activated && error.is_some());
        assert!(joined.load(Ordering::Acquire));
        assert_eq!(
            manager.active_snapshot().library_uuid.as_deref(),
            Some(previous.library_uuid.as_str())
        );
        assert!(manager.active_snapshot().runtime.is_none());
        assert!(manager.pending_runtime_stop.lock().unwrap().is_some());
        assert!(
            manager
                .open_library_with_revision(target.managed_root, revision)
                .await
                .unwrap()
                .1
        );
        assert!(manager.pending_runtime_stop.lock().unwrap().is_none());
        manager
            .active_snapshot()
            .runtime
            .unwrap()
            .shutdown()
            .await
            .unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn safe_library_open_rejects_directory_junction_without_writing_target() {
        let base = tempfile::tempdir().unwrap();
        let origin = test_manager(&base.path().join("origin"));
        let source = origin
            .create_library(
                "Source".into(),
                None,
                None,
                base.path().join("source"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;
        let marker = fs::read(source.managed_root.join(MARKER_FILE_NAME)).unwrap();
        let link = base.path().join("link");
        let result = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&source.managed_root)
            .output()
            .unwrap();
        assert!(result.status.success());
        let host = test_manager(&base.path().join("host"));
        assert_eq!(
            host.open_library_with_revision(link.clone(), 0)
                .await
                .unwrap_err()
                .to_string(),
            "library-path-unavailable-or-reparse"
        );
        assert!(host.registry.snapshot().unwrap().libraries.is_empty());
        assert_eq!(
            fs::read(source.managed_root.join(MARKER_FILE_NAME)).unwrap(),
            marker
        );
        fs::remove_dir(link).unwrap();
    }

    async fn same_name_root_fixture(
        state: &str,
    ) -> (PathBuf, LibraryManager, crate::sync_folders::SyncRootMarker) {
        let base = std::env::temp_dir().join(format!("mangoo-same-name-{}", Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let manager = test_manager(&base);
        let library = manager
            .create_library(
                "Same name".into(),
                None,
                None,
                base.join("managed"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;
        let marker = crate::sync_folders::SyncRootMarker {
            format_version: 1,
            library_uuid: library.library_uuid.clone(),
            root_uuid: Uuid::now_v7().to_string(),
            root_folder_uuid: Uuid::now_v7().to_string(),
            local_binding_uuid: Uuid::now_v7().to_string(),
            binding_generation: 1,
        };
        let source = base.join("Photos");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("photo.jpg"), b"unchanged source").unwrap();
        crate::sync_folders::write_root_marker(&source, &marker).unwrap();
        let source = crate::library_path::normalize_absolute(&source).unwrap();
        let parent = source.parent().unwrap();
        manager
            .registry
            .upsert_remote_sync_root(
                &marker.library_uuid,
                &marker.root_uuid,
                &marker.root_folder_uuid,
                "Photos",
                "Photos",
                &"aa".repeat(32),
                0,
                "active",
                None,
                None,
            )
            .unwrap();
        // Isolated persisted enrollment fixture; production recovery below is
        // exercised through the same inventory turn used by the scheduler.
        let host = rusqlite::Connection::open(base.join("host.sqlite")).unwrap();
        host.execute(
            "INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,marker_identity,filesystem_identity,state,binding_generation,root_capture_epoch,root_actor_epoch,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,applied_materialized_name,applied_materialized_name_key,applied_materialization_fingerprint,operation_revision,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,?3,?3,'Photos','photos',?11,'Photos','photos',?11,1,1,1)",
            rusqlite::params![marker.library_uuid,marker.root_uuid,marker.local_binding_uuid,source.to_string_lossy(),crate::sync_folders::canonical_path_key(&source),parent.to_string_lossy(),crate::sync_folders::canonical_path_key(parent),crate::library_path::local_file_identity(&source.join(crate::sync_folders::ROOT_MARKER_FILE_NAME)).unwrap(),crate::library_path::local_file_identity(&source).unwrap(),state,"aa".repeat(32)],
        ).unwrap();
        drop(host);
        let marker_for_catalog = marker.clone();
        let state = state.to_owned();
        manager
            .write_sync_folder_catalog(&marker.library_uuid, move |database| {
                let root = Uuid::parse_str(&marker_for_catalog.root_uuid)?;
                let binding = Uuid::parse_str(&marker_for_catalog.local_binding_uuid)?;
                database.ensure_selected_sync_root(
                    Uuid::parse_str(&library.library_uuid)?,
                    root,
                    Uuid::parse_str(&marker_for_catalog.root_folder_uuid)?,
                    Uuid::parse_str(&library.root_uuid)?,
                    &source.to_string_lossy(),
                    "Photos",
                    "Photos",
                    [0xaa; 32],
                    binding,
                    binding,
                    binding,
                    1,
                    &state,
                    1,
                    false,
                )?;
                let mut descriptor = database.sync_root_descriptor(root.as_bytes())?.unwrap();
                descriptor.logical_revision += 1;
                descriptor.materialization_fingerprint = [0xbb; 32];
                database.upsert_sync_root_descriptor(&descriptor)
            })
            .await
            .unwrap();
        (base, manager, marker)
    }

    #[tokio::test]
    async fn same_name_root_refresh_adopts_fingerprint_without_relocation() {
        for state in ["active", "paused"] {
            let (base, manager, marker) = same_name_root_fixture(state).await;
            let before = manager
                .registry
                .local_root_binding(&marker.library_uuid, &marker.root_uuid)
                .unwrap()
                .unwrap();
            let source = Path::new(&before.local_path);
            let source_identity = crate::library_path::local_file_identity(source).unwrap();
            let marker_path = source.join(crate::sync_folders::ROOT_MARKER_FILE_NAME);
            let marker_bytes = fs::read(&marker_path).unwrap();
            let marker_identity = crate::library_path::local_file_identity(&marker_path).unwrap();
            manager
                .refresh_sync_folder_inventory(&marker.library_uuid)
                .await
                .unwrap();
            let after = manager
                .registry
                .local_root_binding(&marker.library_uuid, &marker.root_uuid)
                .unwrap()
                .unwrap();
            assert_eq!(after.state, state);
            assert_eq!(after.local_path, before.local_path);
            assert_eq!(after.local_binding_uuid, before.local_binding_uuid);
            assert_eq!(after.binding_generation, before.binding_generation);
            assert_eq!(after.root_capture_epoch, before.root_capture_epoch);
            assert_eq!(after.root_actor_epoch, before.root_actor_epoch);
            assert_eq!(
                after.applied_materialization_fingerprint.as_deref(),
                Some("bb".repeat(32).as_str())
            );
            assert_eq!(after.operation_revision, before.operation_revision + 1);
            assert!(manager
                .registry
                .root_operations(&marker.library_uuid, &marker.root_uuid, None, 50, "all")
                .unwrap()
                .is_empty());
            assert_eq!(
                crate::library_path::local_file_identity(source).unwrap(),
                source_identity
            );
            assert_eq!(
                crate::library_path::local_file_identity(&marker_path).unwrap(),
                marker_identity
            );
            assert_eq!(fs::read(&marker_path).unwrap(), marker_bytes);
            assert_eq!(
                fs::read(source.join("photo.jpg")).unwrap(),
                b"unchanged source"
            );
            manager
                .refresh_sync_folder_inventory(&marker.library_uuid)
                .await
                .unwrap();
            assert_eq!(
                manager
                    .registry
                    .local_root_binding(&marker.library_uuid, &marker.root_uuid)
                    .unwrap()
                    .unwrap(),
                after
            );
            drop(manager);
            fs::remove_dir_all(base).unwrap();
        }
    }

    #[tokio::test]
    async fn same_name_root_recovers_failed_rename_and_catalog_after_reopen() {
        for (resume_state, interrupted_projection) in [
            ("active", false),
            ("paused", false),
            ("active", true),
            ("paused", true),
        ] {
            let (base, manager, marker) = same_name_root_fixture(resume_state).await;
            manager
                .registry
                .upsert_remote_sync_root(
                    &marker.library_uuid,
                    &marker.root_uuid,
                    &marker.root_folder_uuid,
                    "Photos",
                    "Photos",
                    &"bb".repeat(32),
                    1,
                    "active",
                    None,
                    None,
                )
                .unwrap();
            let binding = manager
                .registry
                .local_root_binding(&marker.library_uuid, &marker.root_uuid)
                .unwrap()
                .unwrap();
            let operation_uuid = Uuid::now_v7().to_string();
            let request = serde_json::json!({
                "operationUuid":operation_uuid,"localBindingUuid":marker.local_binding_uuid,
                "resumeState":resume_state,"expectedBindingGeneration":1,
                "targetPath":binding.local_path,"desiredMaterializationFingerprint":"bb".repeat(32)
            });
            manager
                .registry
                .record_automatic_root_rename_preflight_failure(
                    &operation_uuid,
                    &marker.library_uuid,
                    &marker.root_uuid,
                    &crate::sync_folders::request_hash(&request).unwrap(),
                    &request,
                    binding.operation_revision,
                    1,
                    &"bb".repeat(32),
                    Path::new(&binding.local_path),
                )
                .unwrap();
            let coordinator = manager.sync_coordinator.clone();
            let library_uuid = marker.library_uuid.clone();
            manager
                .write_sync_folder_catalog(&marker.library_uuid, move |database| {
                    coordinator.project_root_binding_tokens(&library_uuid, database)
                })
                .await
                .unwrap();
            drop(manager);
            let manager = test_manager(&base);
            let (failed, descriptor) = manager
                .registry
                .pending_automatic_root_renames(&marker.library_uuid)
                .unwrap()
                .pop()
                .unwrap();
            // Model interruption after the atomic Host acknowledgement but
            // before Catalog projection; the next normal turn must finish it.
            if interrupted_projection {
                assert!(manager
                    .registry
                    .reconcile_same_name_root_materialization(&failed, &descriptor)
                    .unwrap());
            }
            let root = Uuid::parse_str(&marker.root_uuid).unwrap();
            manager
                .write_sync_folder_catalog(&marker.library_uuid, move |database| {
                    assert_eq!(
                        database
                            .sync_root_local_binding(root.as_bytes())?
                            .unwrap()
                            .state,
                        "local-rename-required"
                    );
                    Ok(())
                })
                .await
                .unwrap();
            drop(manager);
            let manager = test_manager(&base);
            manager
                .refresh_sync_folder_inventory(&marker.library_uuid)
                .await
                .unwrap();
            let binding = manager
                .registry
                .local_root_binding(&marker.library_uuid, &marker.root_uuid)
                .unwrap()
                .unwrap();
            assert_eq!(binding.state, resume_state);
            assert_eq!(binding.binding_generation, 1);
            assert_eq!(binding.last_error_code, None);
            assert_eq!(
                binding.applied_materialization_fingerprint,
                binding.desired_materialization_fingerprint.clone().into()
            );
            let expected = binding.clone();
            manager
                .write_sync_folder_catalog(&marker.library_uuid, move |database| {
                    let projected = database.sync_root_local_binding(root.as_bytes())?.unwrap();
                    assert_eq!(projected.state, expected.state);
                    assert_eq!(projected.binding_generation, expected.binding_generation);
                    assert_eq!(projected.operation_revision, expected.operation_revision);
                    assert_eq!(
                        projected.local_binding_uuid,
                        *Uuid::parse_str(&expected.local_binding_uuid)?.as_bytes()
                    );
                    Ok(())
                })
                .await
                .unwrap();
            manager
                .refresh_sync_folder_inventory(&marker.library_uuid)
                .await
                .unwrap();
            assert_eq!(
                manager
                    .registry
                    .local_root_binding(&marker.library_uuid, &marker.root_uuid)
                    .unwrap()
                    .unwrap(),
                binding
            );
            let operations = manager
                .registry
                .root_operations(&marker.library_uuid, &marker.root_uuid, None, 50, "all")
                .unwrap();
            assert_eq!(operations.len(), 1);
            assert_eq!(operations[0].state, "failed");
            assert_eq!(operations[0].operation_uuid, operation_uuid);
            assert_eq!(
                crate::sync_folders::read_root_marker(Path::new(&binding.local_path)).unwrap(),
                marker
            );
            drop(manager);
            fs::remove_dir_all(base).unwrap();
        }
    }

    #[tokio::test]
    async fn same_name_root_rejects_stale_identity_and_open_operation() {
        let (base, manager, marker) = same_name_root_fixture("active").await;
        manager
            .registry
            .upsert_remote_sync_root(
                &marker.library_uuid,
                &marker.root_uuid,
                &marker.root_folder_uuid,
                "Photos",
                "Photos",
                &"bb".repeat(32),
                1,
                "active",
                None,
                None,
            )
            .unwrap();
        let (binding, descriptor) = manager
            .registry
            .pending_automatic_root_renames(&marker.library_uuid)
            .unwrap()
            .pop()
            .unwrap();
        for field in ["generation", "revision", "binding", "fingerprint"] {
            let mut stale = binding.clone();
            match field {
                "generation" => stale.binding_generation += 1,
                "revision" => stale.operation_revision += 1,
                "binding" => stale.local_binding_uuid = Uuid::now_v7().to_string(),
                "fingerprint" => stale.desired_materialization_fingerprint = "cc".repeat(32),
                _ => unreachable!(),
            }
            assert!(
                manager
                    .registry
                    .reconcile_same_name_root_materialization(&stale, &descriptor)
                    .is_err(),
                "{field}"
            );
        }
        let mut stale_descriptor = descriptor.clone();
        stale_descriptor.logical_revision += 1;
        assert!(manager
            .registry
            .reconcile_same_name_root_materialization(&binding, &stale_descriptor)
            .is_err());
        for name in ["Changed", "photos"] {
            let mut renamed = descriptor.clone();
            renamed.materialized_name = name.into();
            assert!(!manager
                .registry
                .reconcile_same_name_root_materialization(&binding, &renamed)
                .unwrap());
        }
        let marker_path =
            Path::new(&binding.local_path).join(crate::sync_folders::ROOT_MARKER_FILE_NAME);
        let marker_bytes = fs::read(&marker_path).unwrap();
        let mut foreign = marker.clone();
        foreign.root_folder_uuid = Uuid::now_v7().to_string();
        fs::write(&marker_path, serde_json::to_vec(&foreign).unwrap()).unwrap();
        assert!(manager
            .registry
            .reconcile_same_name_root_materialization(&binding, &descriptor)
            .is_err());
        fs::write(&marker_path, &marker_bytes).unwrap();
        let host = rusqlite::Connection::open(base.join("host.sqlite")).unwrap();
        let old_identity: String = host
            .query_row(
                "SELECT filesystem_identity FROM local_root_bindings WHERE root_uuid=?1",
                [&marker.root_uuid],
                |row| row.get(0),
            )
            .unwrap();
        host.execute(
            "UPDATE local_root_bindings SET filesystem_identity='foreign' WHERE root_uuid=?1",
            [&marker.root_uuid],
        )
        .unwrap();
        assert!(manager
            .registry
            .reconcile_same_name_root_materialization(&binding, &descriptor)
            .is_err());
        host.execute(
            "UPDATE local_root_bindings SET filesystem_identity=?1 WHERE root_uuid=?2",
            rusqlite::params![old_identity, marker.root_uuid],
        )
        .unwrap();
        drop(host);
        manager
            .registry
            .begin_root_operation_once(
                &Uuid::now_v7().to_string(),
                &marker.library_uuid,
                &marker.root_uuid,
                "logical-rename",
                "planned",
                &"ff".repeat(32),
                &serde_json::json!({}),
                binding.operation_revision,
                Some(1),
            )
            .unwrap();
        assert_eq!(
            manager
                .registry
                .reconcile_same_name_root_materialization(&binding, &descriptor)
                .unwrap_err()
                .to_string(),
            "operation-in-progress"
        );
        assert_eq!(
            manager
                .registry
                .local_root_binding(&marker.library_uuid, &marker.root_uuid)
                .unwrap()
                .unwrap(),
            binding
        );
        assert_eq!(fs::read(&marker_path).unwrap(), marker_bytes);
        assert_eq!(
            fs::read(Path::new(&binding.local_path).join("photo.jpg")).unwrap(),
            b"unchanged source"
        );
        drop(manager);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relocation_serialization_is_scoped_per_root() {
        let base = std::env::temp_dir().join(format!("mangoo-root-locks-{}", Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let manager = test_manager(&base);
        let first = manager.root_relocation_operation_lock("root-a");
        let same = manager.root_relocation_operation_lock("root-a");
        let other = manager.root_relocation_operation_lock("root-b");
        assert!(Arc::ptr_eq(&first, &same));
        assert!(!Arc::ptr_eq(&first, &other));
        drop(first);
        drop(same);
        drop(other);
        drop(manager);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn relocation_reconciliation_waits_only_for_non_terminal_scan_states() {
        assert!(!relocation_reconciliation_succeeded(None).unwrap());
        assert!(!relocation_reconciliation_succeeded(Some("pending")).unwrap());
        assert!(!relocation_reconciliation_succeeded(Some("running")).unwrap());
        assert!(relocation_reconciliation_succeeded(Some("succeeded")).unwrap());
        assert!(relocation_reconciliation_succeeded(Some("failed")).is_err());
        assert!(relocation_reconciliation_succeeded(Some("cancelled")).is_err());
        assert!(relocation_reconciliation_succeeded(Some("unknown")).is_err());
    }

    fn platform() -> crate::plugins::manifest::Platform {
        if cfg!(windows) {
            crate::plugins::manifest::Platform::WindowsX64
        } else {
            crate::plugins::manifest::Platform::LinuxX64
        }
    }

    fn configure_fake_provider(
        manager: &LibraryManager,
        bundled_root: &Path,
    ) -> crate::plugins::manifest::ManifestRecord {
        let manifest =
            crate::plugins::parse_manifest(&bundled_root.join(FAKE_PLUGIN_ID).join("plugin.json"))
                .unwrap();
        let entry = manifest.entry_for(platform()).unwrap();
        let executable = bundled_root.join(FAKE_PLUGIN_ID).join(entry);
        assert!(executable.is_file(), "bundled fake Provider was not built");
        let hash = crate::plugins::manifest::sha256_file(&executable).unwrap();
        let permissions = serde_json::to_string(&manifest.requested_permissions).unwrap();
        manager
            .registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: &manifest.id,
                version: &manifest.version,
                manifest_json: &serde_json::to_string(&manifest).unwrap(),
                entry_relative_path: entry,
                entry_sha256: &hash,
                install_root: &bundled_root.join(FAKE_PLUGIN_ID),
                trust_policy_id: "embedded-v1",
                requested_permissions_json: &permissions,
            })
            .unwrap();
        manager
            .registry
            .set_plugin_grants(FAKE_PLUGIN_ID, &permissions, true)
            .unwrap();
        manager
            .registry
            .upsert_plugin_account(
                FAKE_ACCOUNT_ID,
                FAKE_PLUGIN_ID,
                Some("Bundled Fake Account"),
                "authenticated",
            )
            .unwrap();
        manager
            .registry
            .set_plugin_intent(FAKE_PLUGIN_ID, true, true)
            .unwrap();
        manifest
    }

    #[tokio::test]
    async fn plugin_lifecycle_failed_stop_cannot_advance_or_reopen_restart_epoch() {
        let base =
            std::env::temp_dir().join(format!("mangoo_stop_failure_{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&base).unwrap();
        let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/bundled-plugins")
            .canonicalize()
            .unwrap();
        let manager = isolated_manager(&base, bundled.clone());
        configure_fake_provider(&manager, &bundled);
        let epoch = manager
            .registry
            .plugin_lifecycle_epoch(FAKE_PLUGIN_ID)
            .unwrap();
        manager
            .provider_manager()
            .open_plugin(FAKE_PLUGIN_ID, epoch)
            .unwrap();
        let (commands, mut receiver) = tokio::sync::mpsc::channel(1);
        let registry = manager.registry.clone();
        let providers = manager.provider_manager();
        let task = tokio::spawn(async move {
            let mut running = HashMap::from([(
                FAKE_PLUGIN_ID.to_owned(),
                (
                    String::new(),
                    CancellationToken::new(),
                    tokio::spawn(async { anyhow::bail!("fixture child join failure") }),
                ),
            )]);
            let mut errors = HashMap::new();
            for _ in 0..2 {
                if let Some(PluginRuntimeCommand::Stop {
                    plugin_id,
                    completed,
                }) = receiver.recv().await
                {
                    let result = stop_plugin_supervisor(
                        &plugin_id,
                        &mut running,
                        &mut errors,
                        &registry,
                        &providers,
                    )
                    .await;
                    let _ = completed.send(result);
                }
            }
        });
        *manager.plugin_runtime_scheduler.lock().unwrap() = Some(PluginRuntimeSchedulerHandle {
            cancellation: CancellationToken::new(),
            commands,
            task,
        });
        assert!(manager.finish_plugin_restart(FAKE_PLUGIN_ID).await.is_err());
        // The failed supervisor handle has been consumed, but another stop must
        // retain that failure and must not issue a new launch epoch.
        assert!(manager.finish_plugin_restart(FAKE_PLUGIN_ID).await.is_err());
        assert_eq!(
            manager
                .registry
                .plugin_lifecycle_epoch(FAKE_PLUGIN_ID)
                .unwrap(),
            epoch
        );
        assert_eq!(
            manager
                .registry
                .host_plugin(FAKE_PLUGIN_ID)
                .unwrap()
                .unwrap()
                .runtime_state,
            "stopping"
        );
        assert!(manager
            .provider_manager()
            .open_plugin(FAKE_PLUGIN_ID, epoch)
            .is_err());
        assert!(manager
            .provider_manager()
            .begin_work(FAKE_PLUGIN_ID)
            .is_err());
        let scheduler = manager
            .plugin_runtime_scheduler
            .lock()
            .unwrap()
            .take()
            .unwrap();
        scheduler.task.await.unwrap();
        drop(manager);
        let _ = std::fs::remove_dir_all(base);
    }

    async fn activate_fake_provider(
        manager: &LibraryManager,
        manifest: &crate::plugins::manifest::ManifestRecord,
        bundled_root: &Path,
        runtime: &Path,
    ) -> crate::plugins::runtime::supervisor::ActivatedProvider {
        crate::plugins::runtime::supervisor::activate_and_register(
            &manager.provider_manager(),
            manifest,
            FAKE_ACCOUNT_ID,
            manifest.requested_permissions.clone(),
            &bundled_root.join(FAKE_PLUGIN_ID),
            platform(),
            crate::plugins::runtime::trust::embedded_trust_registry(),
            runtime.to_path_buf(),
            None,
            std::collections::BTreeMap::new(),
        )
        .await
        .unwrap()
    }

    fn creation_intent(base: &Path, suffix: &str) -> CreationIntent {
        let library_uuid = format!("library-{suffix}");
        CreationIntent {
            txn_uuid: format!("txn-{suffix}"),
            descriptor: LibraryDescriptor {
                library_uuid: library_uuid.clone(),
                root_uuid: format!("root-{suffix}"),
                name: format!("Library {suffix}"),
                icon: None,
                color: None,
                local_data_dir: base.join("libraries").join(&library_uuid),
                managed_root: base.join(format!("managed-{suffix}")),
                lifecycle_state: "ready".to_string(),
                binding_state: "local-only".to_string(),
                availability: "available".to_string(),
                marker_identity: String::new(),
            },
            requested_root: base.join(format!("managed-{suffix}")),
            staging_dir: base
                .join("libraries")
                .join(format!("{library_uuid}.staging.txn-{suffix}")),
        }
    }

    #[test]
    fn restore_cleanup_preserves_content_added_after_quarantine_and_marker_replacement() {
        for replace_marker in [false, true] {
            let base =
                std::env::temp_dir().join(format!("mangoo-restore-cleanup-{}", Uuid::now_v7()));
            fs::create_dir_all(&base).unwrap();
            let local_quarantine = base.join("local-quarantine");
            let root_quarantine = base.join("root-quarantine");
            fs::create_dir(&local_quarantine).unwrap();
            fs::create_dir(&root_quarantine).unwrap();
            let mut descriptor = creation_intent(&base, "late-content").descriptor;
            descriptor.local_data_dir = base.join("local");
            let local_identity = SafeDirectory::open(&local_quarantine)
                .unwrap()
                .identity()
                .to_owned();
            let root_identity = SafeDirectory::open(&root_quarantine)
                .unwrap()
                .identity()
                .to_owned();
            let marker = root_quarantine.join(MARKER_FILE_NAME);
            fs::write(&marker, b"owned marker").unwrap();
            descriptor.marker_identity = marker_identity(&marker).unwrap();
            // Model the previously unsafe interval after early empty checks and
            // asynchronous fencing, immediately before final container cleanup.
            fs::write(
                local_quarantine.join("user-local.bin"),
                b"new local content",
            )
            .unwrap();
            fs::write(root_quarantine.join("user-root.bin"), b"new root content").unwrap();
            if replace_marker {
                fs::rename(&marker, root_quarantine.join("original-marker")).unwrap();
                fs::write(&marker, b"replacement content").unwrap();
            }
            let result = finish_restore_container_cleanup(
                &descriptor,
                &local_quarantine,
                &root_quarantine,
                &local_identity,
                &root_identity,
                true,
            );
            assert_eq!(result.is_err(), replace_marker);
            assert_eq!(
                fs::read(descriptor.local_data_dir.join("user-local.bin")).unwrap(),
                b"new local content"
            );
            assert_eq!(
                fs::read(descriptor.managed_root.join("user-root.bin")).unwrap(),
                b"new root content"
            );
            if replace_marker {
                assert_eq!(
                    fs::read(descriptor.managed_root.join(MARKER_FILE_NAME)).unwrap(),
                    b"replacement content"
                );
            } else {
                assert!(!descriptor.managed_root.join(MARKER_FILE_NAME).exists());
            }
            assert!(!local_quarantine.exists());
            assert!(!root_quarantine.exists());
            fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn startup_recovery_preserves_unsupported_uncommitted_creation() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let intent = creation_intent(base.path(), "rollback");
        manager.registry.begin_creation(&intent).unwrap();
        fs::create_dir_all(&intent.staging_dir).unwrap();
        let staging_identity = SafeDirectory::open(&intent.staging_dir)
            .unwrap()
            .identity()
            .to_string();
        manager
            .registry
            .update_creation_identities(&intent.txn_uuid, None, Some(&staging_identity))
            .unwrap();
        fs::write(intent.staging_dir.join("partial"), b"partial").unwrap();
        manager.recover_journals().unwrap();
        assert_eq!(
            fs::read(intent.staging_dir.join("partial")).unwrap(),
            b"partial"
        );
        assert_eq!(
            manager.registry.pending_creation_recovery().unwrap().len(),
            1
        );
        assert!(manager
            .registry
            .library(&intent.descriptor.library_uuid)
            .unwrap()
            .is_none());
    }

    #[test]
    fn startup_recovery_preserves_unsupported_committed_creation() {
        let base = tempfile::tempdir().unwrap();
        let manager = test_manager(base.path());
        let intent = creation_intent(base.path(), "finish");
        manager.registry.begin_creation(&intent).unwrap();
        fs::create_dir_all(&intent.descriptor.managed_root).unwrap();
        fs::create_dir_all(&intent.staging_dir).unwrap();
        fs::write(intent.staging_dir.join("catalog.sqlite"), b"catalog").unwrap();
        let marker = LibraryMarker {
            format_version: 1,
            library_uuid: intent.descriptor.library_uuid.clone(),
            root_uuid: intent.descriptor.root_uuid.clone(),
            state: MarkerState::Committed,
            creation_txn_uuid: intent.txn_uuid.clone(),
        };
        let bytes = serde_json::to_vec(&marker).unwrap();
        fs::write(
            intent.descriptor.managed_root.join(MARKER_FILE_NAME),
            &bytes,
        )
        .unwrap();
        manager
            .registry
            .update_creation_phase(&intent.txn_uuid, "marker-committed", true, true, None, None)
            .unwrap();
        manager.recover_journals().unwrap();
        assert!(manager
            .registry
            .library(&intent.descriptor.library_uuid)
            .unwrap()
            .is_none());
        assert_eq!(
            manager.registry.pending_creation_recovery().unwrap().len(),
            1
        );
        assert_eq!(
            fs::read(intent.descriptor.managed_root.join(MARKER_FILE_NAME)).unwrap(),
            bytes
        );
        assert_eq!(
            fs::read(intent.staging_dir.join("catalog.sqlite")).unwrap(),
            b"catalog"
        );
        assert!(!intent.descriptor.local_data_dir.exists());
    }

    #[test]
    fn startup_recovery_preserves_unsupported_requested_and_partial_removal_states() {
        for quarantined in [false, true] {
            let base = tempfile::tempdir().unwrap();
            let manager = test_manager(base.path());
            let intent = creation_intent(base.path(), "old-removal");
            fs::create_dir_all(&intent.descriptor.managed_root).unwrap();
            fs::create_dir_all(&intent.descriptor.local_data_dir).unwrap();
            fs::write(
                intent.descriptor.local_data_dir.join("catalog.sqlite"),
                b"catalog",
            )
            .unwrap();
            let marker = LibraryMarker {
                format_version: 1,
                library_uuid: intent.descriptor.library_uuid.clone(),
                root_uuid: intent.descriptor.root_uuid.clone(),
                state: MarkerState::Committed,
                creation_txn_uuid: intent.txn_uuid.clone(),
            };
            let marker_bytes = serde_json::to_vec(&marker).unwrap();
            fs::write(
                intent.descriptor.managed_root.join(MARKER_FILE_NAME),
                &marker_bytes,
            )
            .unwrap();
            let mut descriptor = intent.descriptor.clone();
            descriptor.marker_identity =
                marker_identity(&descriptor.managed_root.join(MARKER_FILE_NAME)).unwrap();
            manager.registry.begin_creation(&intent).unwrap();
            manager
                .registry
                .commit_creation(&intent.txn_uuid, &descriptor)
                .unwrap();
            manager
                .registry
                .begin_removal(&descriptor, "old-removal")
                .unwrap();
            let host = rusqlite::Connection::open(base.path().join("host.sqlite")).unwrap();
            host.execute("UPDATE library_removal_journal SET layout_version=1", [])
                .unwrap();
            let (root, local) = if quarantined {
                let root = base.path().join("root-quarantine");
                let local = base.path().join("local-quarantine");
                fs::rename(&descriptor.managed_root, &root).unwrap();
                fs::rename(&descriptor.local_data_dir, &local).unwrap();
                manager
                    .registry
                    .update_removal_quarantine(
                        "old-removal",
                        &root,
                        &local,
                        SafeDirectory::open(&root).unwrap().identity(),
                        SafeDirectory::open(&local).unwrap().identity(),
                    )
                    .unwrap();
                (root, local)
            } else {
                (
                    descriptor.managed_root.clone(),
                    descriptor.local_data_dir.clone(),
                )
            };
            manager.recover_journals().unwrap();
            assert_eq!(
                manager.registry.pending_removal_recovery().unwrap().len(),
                1
            );
            assert_eq!(
                manager
                    .registry
                    .library(&descriptor.library_uuid)
                    .unwrap()
                    .unwrap()
                    .lifecycle_state,
                "removing"
            );
            assert_eq!(fs::read(root.join(MARKER_FILE_NAME)).unwrap(), marker_bytes);
            assert_eq!(fs::read(local.join("catalog.sqlite")).unwrap(), b"catalog");
            assert_eq!(descriptor.managed_root.exists(), !quarantined);
            assert_eq!(descriptor.local_data_dir.exists(), !quarantined);
        }
    }

    #[tokio::test]
    async fn cancelled_activation_route_retains_runtime_shutdown_until_retry_or_host_exit() {
        use axum::body::Body;
        use axum::http::{Method, Request, StatusCode};
        use std::time::Duration;
        use tower::ServiceExt;

        for host_exit in [false, true] {
            let base = tempfile::tempdir().unwrap();
            let state = crate::api::AppState::new_host(
                base.path().join("host.sqlite"),
                base.path().join("libraries"),
            )
            .await
            .unwrap();
            let manager = state.library_manager.clone().unwrap();
            manager
                .create_library(
                    "Previous".into(),
                    None,
                    None,
                    base.path().join("previous"),
                    None,
                    true,
                )
                .await
                .unwrap();
            let target = manager
                .create_library(
                    "Target".into(),
                    None,
                    None,
                    base.path().join("target"),
                    None,
                    false,
                )
                .await
                .unwrap()
                .0;
            let previous = manager.active_snapshot();
            let runtime = previous.runtime.unwrap();
            let catalog = runtime.read_database_path.clone().unwrap();
            let database = Database::open(&catalog).unwrap();
            let (release, blocked) = tokio::sync::oneshot::channel();
            runtime.own_background_task(tokio::spawn(async move {
                blocked.await.unwrap();
                drop(database);
            }));
            let app = crate::api::routes::router(state);
            let request = || {
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/libraries/{}/activate", target.library_uuid))
                    .header(
                        "x-mangoo-context-revision",
                        previous.context_revision.to_string(),
                    )
                    .body(Body::empty())
                    .unwrap()
            };
            let requester = tokio::spawn(app.clone().oneshot(request()));
            tokio::time::timeout(Duration::from_secs(5), async {
                // Wait until shutdown owns the drained JoinHandle, not merely
                // until it has cancelled the runtime before closing read leases.
                while !runtime.background_tasks.lock().unwrap().is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            requester.abort();
            assert!(requester.await.unwrap_err().is_cancelled());
            assert!(manager.operation.try_lock().is_err());

            let mut continuation = if host_exit {
                let manager = manager.clone();
                tokio::spawn(async move {
                    manager.shutdown_sync_coordinator().await.unwrap();
                })
            } else {
                let request = request();
                tokio::spawn(async move {
                    let response = app.oneshot(request).await.unwrap();
                    // The accepted first activation commits once; its retry
                    // subsequently observes the advanced context revision.
                    assert_eq!(response.status(), StatusCode::CONFLICT);
                })
            };
            assert!(
                tokio::time::timeout(Duration::from_millis(100), &mut continuation)
                    .await
                    .is_err()
            );
            assert!(manager.active_snapshot().runtime.is_none());
            release.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(5), continuation)
                .await
                .unwrap()
                .unwrap();
            let active = manager.active_snapshot();
            assert_eq!(
                active.library_uuid.as_deref(),
                Some(target.library_uuid.as_str())
            );
            assert_eq!(active.context_revision, previous.context_revision + 1);
            // The retired runtime Arc may survive, but its native handles may
            // not: on Windows this rename fails while the blocked task owns one.
            let moved = catalog.with_extension("retired-check");
            fs::rename(&catalog, &moved).unwrap();
            fs::rename(moved, catalog).unwrap();
            if !host_exit {
                manager.shutdown_sync_coordinator().await.unwrap();
            }
            active.runtime.unwrap().shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn runtime_shutdown_cancels_and_joins_owned_background_work() {
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        let runtime = LibraryRuntimeResources::from_database_with_task_queue(database, sender);
        let cancellation = runtime.cancellation_token();
        let finished = Arc::new(AtomicBool::new(false));
        let finished_task = Arc::clone(&finished);
        runtime.own_background_task(tokio::spawn(async move {
            cancellation.cancelled().await;
            finished_task.store(true, Ordering::Release);
        }));
        runtime.shutdown().await.unwrap();
        assert!(finished.load(Ordering::Acquire));
    }

    #[test]
    fn runtime_read_database_cache_bounds_idle_connections() {
        let cache = Arc::new(ReadDatabaseCache::default());
        let leases = (0..MAX_CACHED_READ_DATABASES + 2)
            .map(|_| {
                let mut lease = cache.checkout().unwrap();
                lease.database = Some(Database::open_in_memory().unwrap());
                lease
            })
            .collect::<Vec<_>>();
        drop(leases);
        assert_eq!(
            cache.state.lock().unwrap().idle.len(),
            MAX_CACHED_READ_DATABASES
        );
        cache.close();
        assert!(cache.state.lock().unwrap().idle.is_empty());
        assert!(cache.checkout().is_err());
    }

    #[tokio::test]
    async fn runtime_shutdown_drains_read_databases_after_errors_and_panics() {
        use std::time::Duration;

        for panic_after_read in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let catalog = directory.path().join("catalog.sqlite");
            let database = Database::open(&catalog).unwrap();
            database.apply_migrations().unwrap();
            let runtime =
                LibraryRuntimeResources::from_database(None, 1, database, None, None).unwrap();
            let mut idle = runtime.read_database_cache.checkout().unwrap();
            idle.database = Some(Database::open(&catalog).unwrap());
            drop(idle);
            let mut lease = runtime.read_database_cache.checkout().unwrap();
            assert!(
                lease.database.is_some(),
                "reuse the runtime's idle connection"
            );
            // A second independently owned connection remains cached while
            // this read is running; shutdown must drain both kinds of owner.
            let mut idle = runtime.read_database_cache.checkout().unwrap();
            idle.database = Some(Database::open(&catalog).unwrap());
            drop(idle);
            let (started, ready) = tokio::sync::oneshot::channel();
            let (release, released) = std::sync::mpsc::channel();
            let read = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let database = lease.database.as_mut().unwrap();
                database.list_roots()?;
                started.send(()).unwrap();
                released.recv().unwrap();
                if panic_after_read {
                    panic!("read lease unwind fixture");
                }
                anyhow::bail!("read lease error fixture")
            });
            ready.await.unwrap();
            let retiring = runtime.clone();
            let shutdown = tokio::spawn(async move { retiring.shutdown().await });
            tokio::time::timeout(Duration::from_secs(10), async {
                while !runtime.read_database_cache.state.lock().unwrap().closed {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(
                !shutdown.is_finished(),
                "active reads must keep retirement pending"
            );
            assert!(runtime.read_database_cache.checkout().is_err());
            release.send(()).unwrap();
            let outcome = read.await;
            if panic_after_read {
                assert!(outcome.unwrap_err().is_panic());
            } else {
                assert!(outcome.unwrap().is_err());
            }
            tokio::time::timeout(Duration::from_secs(10), shutdown)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let moved = catalog.with_extension("retired-check");
            fs::rename(&catalog, &moved).unwrap();
            fs::rename(moved, &catalog).unwrap();
        }
    }

    #[tokio::test]
    async fn duplicate_create_for_same_root_converges_to_one_library() {
        let base = std::env::temp_dir().join(format!("mangoo-double-create-{}", Uuid::now_v7()));
        let root = base.join("managed");
        fs::create_dir_all(&base).unwrap();
        let manager = test_manager(&base);
        let first = manager
            .create_library("Same".into(), None, None, root.clone(), None, false)
            .await
            .unwrap();
        let second = manager
            .create_library("Same".into(), None, None, root.clone(), None, false)
            .await;
        assert!(second.is_err());
        let snapshot = manager.registry.snapshot().unwrap();
        assert_eq!(snapshot.libraries.len(), 1);
        assert_eq!(snapshot.libraries[0].library_uuid, first.0.library_uuid);
        drop(manager);
        let _ = fs::remove_dir_all(base);
    }

    /// Release integration entry point: exercises the production managed-Library
    /// creation and reopen path from an empty Host without any legacy Catalog
    /// selector or root-adoption compatibility branch.
    #[tokio::test]
    async fn clean_managed_bootstrap_reopens_current_schema() {
        let base = std::env::temp_dir().join(format!("mangoo-clean-bootstrap-{}", Uuid::now_v7()));
        let managed_root = base.join("managed-root");
        fs::create_dir_all(&base).unwrap();

        let manager = test_manager(&base);
        let (created, activated, activation_error) = manager
            .create_library(
                "Clean bootstrap".into(),
                None,
                None,
                managed_root.clone(),
                None,
                true,
            )
            .await
            .unwrap();
        assert!(activated, "new managed Library must become active");
        assert!(activation_error.is_none());
        let initial = manager.active_snapshot();
        assert_eq!(
            initial.library_uuid.as_deref(),
            Some(created.library_uuid.as_str())
        );
        assert_eq!(manager.registry.snapshot().unwrap().libraries.len(), 1);
        let marker = read_marker(&managed_root).unwrap();
        assert_eq!(marker.state, MarkerState::Committed);
        assert_eq!(marker.library_uuid, created.library_uuid);
        assert_eq!(marker.root_uuid, created.root_uuid);
        verify_descriptor_identity(&created).unwrap();
        let catalog_path = created.local_data_dir.join("catalog.sqlite");
        let database = Database::open(&catalog_path).unwrap();
        let migration: i64 = database
            .connection_for_fsops()
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        let identity: (Vec<u8>, Vec<u8>) = database
            .connection_for_fsops()
            .query_row(
                "SELECT library_uuid,device_uuid FROM sync_local_state WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(migration, 39);
        let presentation_tables: i64 = database
            .connection_for_fsops()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('file_browse_fts_progress','decoder_presentations')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(presentation_tables, 2);
        assert_eq!(
            Uuid::from_slice(&identity.0).unwrap().to_string(),
            created.library_uuid
        );
        assert_eq!(
            Uuid::from_slice(&identity.1).unwrap(),
            manager.registry.device_uuid().unwrap()
        );
        drop(database);
        if let Some(runtime) = initial.runtime {
            runtime.shutdown().await.unwrap();
        }
        drop(manager);

        let reopened = test_manager(&base);
        reopened.restore_active().await.unwrap();
        let snapshot = reopened.active_snapshot();
        assert_eq!(
            snapshot.library_uuid.as_deref(),
            Some(created.library_uuid.as_str())
        );
        assert!(snapshot.runtime.is_some());
        let reopened_descriptor = reopened
            .registry
            .library(&created.library_uuid)
            .unwrap()
            .unwrap();
        verify_descriptor_identity(&reopened_descriptor).unwrap();
        assert_eq!(reopened.registry.snapshot().unwrap().libraries.len(), 1);
        if let Some(runtime) = snapshot.runtime {
            runtime.shutdown().await.unwrap();
        }
        drop(reopened);
        let _ = fs::remove_dir_all(base);
    }

    /// Docker release verifier invoked in two independent containers sharing
    /// only `/evidence`. The first phase persists a real managed Library and a
    /// framed fake-Provider immutable object; the second reopens the Host and
    /// proves that registry, Catalog, binding and Provider state survived the
    /// volume restart.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "invoked twice by the exact-tree Docker release harness"]
    async fn docker_volume_restart_persists_library_catalog_and_sync_state() {
        let phase = std::env::var("MANGOO_DOCKER_PERSISTENCE_PHASE")
            .expect("MANGOO_DOCKER_PERSISTENCE_PHASE must be create or verify");
        let base = PathBuf::from(
            std::env::var("MANGOO_DOCKER_PERSISTENCE_ROOT")
                .expect("MANGOO_DOCKER_PERSISTENCE_ROOT must be mounted from a Docker volume"),
        );
        let bundled_root = PathBuf::from(
            std::env::var("MANGOO_SYNC_BUNDLED_PLUGINS_DIR")
                .expect("MANGOO_SYNC_BUNDLED_PLUGINS_DIR must identify packaged Providers"),
        );
        fs::create_dir_all(&base).unwrap();
        let state_file = base.join("expected.json");
        if phase == "create" {
            assert!(
                !state_file.exists(),
                "create phase must start from an empty verifier state"
            );
            let manager = isolated_manager(&base, bundled_root.clone());
            let manifest = configure_fake_provider(&manager, &bundled_root);
            let provider = activate_fake_provider(
                &manager,
                &manifest,
                &bundled_root,
                &base.join("provider-runtime"),
            )
            .await;
            manager
                .registry
                .record_plugin_runtime_ready(
                    FAKE_PLUGIN_ID,
                    "docker-persistence-create",
                    &["sync-provider.v1".into()],
                )
                .unwrap();
            let (library, _, _) = manager
                .create_library(
                    "Docker Persistent Library".into(),
                    None,
                    None,
                    base.join("managed"),
                    None,
                    true,
                )
                .await
                .unwrap();
            manager
                .enable_library_sync_with_revision(
                    &library.library_uuid,
                    manager.active_snapshot().context_revision,
                    FAKE_PLUGIN_ID,
                    FAKE_ACCOUNT_ID,
                )
                .await
                .unwrap();
            let binding = manager
                .registry
                .binding_transport(&library.library_uuid, "library")
                .unwrap()
                .unwrap();
            let namespace = binding
                .remote_namespace
                .clone()
                .unwrap_or_else(|| library.library_uuid.clone());
            let context = crate::sync::provider::TransportContext {
                namespace: namespace.clone(),
                binding_epoch: binding.binding_epoch.to_string(),
                transport_generation: u64::try_from(binding.transport_generation).unwrap(),
            };
            let marker = b"mangoo-docker-volume-sync-state".to_vec();
            let hash = crate::sync::codec::sha256(&marker);
            provider
                .provider
                .object_put_immutable(&context, "objects/release/persistence", &marker, hash)
                .await
                .unwrap();
            fs::write(
                &state_file,
                serde_json::to_vec_pretty(&serde_json::json!({
                    "libraryUuid": library.library_uuid,
                    "remoteNamespace": namespace,
                    "bindingEpoch": binding.binding_epoch,
                    "transportGeneration": binding.transport_generation,
                    "objectSha256": crate::sync::media::hex_hash(&hash),
                }))
                .unwrap(),
            )
            .unwrap();
            if let Some(runtime) = manager.active_snapshot().runtime {
                runtime.shutdown().await.unwrap();
            }
            provider.shutdown().await.unwrap();
        } else if phase == "verify" {
            let expected: serde_json::Value =
                serde_json::from_slice(&fs::read(&state_file).unwrap()).unwrap();
            let library_uuid = expected["libraryUuid"].as_str().unwrap();
            let manager = isolated_manager(&base, bundled_root.clone());
            let manifest = configure_fake_provider(&manager, &bundled_root);
            let provider = activate_fake_provider(
                &manager,
                &manifest,
                &bundled_root,
                &base.join("provider-runtime"),
            )
            .await;
            manager
                .registry
                .record_plugin_runtime_ready(
                    FAKE_PLUGIN_ID,
                    "docker-persistence-verify",
                    &["sync-provider.v1".into()],
                )
                .unwrap();
            manager.restore_active().await.unwrap();
            let snapshot = manager.active_snapshot();
            assert_eq!(snapshot.library_uuid.as_deref(), Some(library_uuid));
            let library = manager.registry.library(library_uuid).unwrap().unwrap();
            verify_descriptor_identity(&library).unwrap();
            let catalog = Database::open(&library.local_data_dir.join("catalog.sqlite")).unwrap();
            let schema: i64 = catalog
                .connection_for_fsops()
                .query_row(
                    "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                schema, 29,
                "all production Catalog migrations must be present"
            );
            let capture: bool = catalog
                .connection_for_fsops()
                .query_row(
                    "SELECT capture_enabled FROM sync_local_state WHERE singleton=1",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(capture);
            drop(catalog);
            let context = crate::sync::provider::TransportContext {
                namespace: expected["remoteNamespace"].as_str().unwrap().to_owned(),
                binding_epoch: expected["bindingEpoch"].as_str().unwrap().to_owned(),
                transport_generation: expected["transportGeneration"].as_u64().unwrap(),
            };
            let object = provider
                .provider
                .object_head(&context, "objects/release/persistence")
                .await
                .unwrap()
                .expect("Provider object must survive the Docker volume restart");
            assert_eq!(
                crate::sync::media::hex_hash(&object.sha256),
                expected["objectSha256"].as_str().unwrap()
            );
            provider.shutdown().await.unwrap();
            if let Some(runtime) = snapshot.runtime {
                runtime.shutdown().await.unwrap();
            }
        } else {
            panic!("unsupported persistence phase {phase}");
        }
    }

    /// Release integration entry point: two isolated Hosts share only one
    /// verified bundled Provider child. Device B starts without a Catalog,
    /// restores the seed checkpoint through framed RPC, then both devices
    /// capture independent offline edits and converge through production
    /// coordinator turns. A third Host repeats the no-Catalog restore after
    /// the concurrent event tail is published.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "invoked by tools/checks/sync-integration.mjs"]
    async fn real_provider_two_device_and_third_device_restore() {
        async fn discover_fixture_library(manager: &LibraryManager, library_uuid: &str) {
            // This single-library fake Provider exposes library-scoped Delta only;
            // this adapter does not cover Root-scope discovery. Validate its real
            // framed-RPC objects with the production descriptor validator before
            // installing the same account/session-bound candidate evidence.
            let transport = manager
                .registry
                .provider_root_transports()
                .unwrap()
                .into_iter()
                .find(|transport| transport.account_uuid == FAKE_ACCOUNT_ID)
                .unwrap();
            let provider = manager
                .provider_manager()
                .resolve(FAKE_PLUGIN_ID, FAKE_ACCOUNT_ID)
                .unwrap()
                .unwrap();
            let context = crate::sync::provider::TransportContext {
                namespace: library_uuid.to_owned(),
                binding_epoch: Uuid::nil().to_string(),
                transport_generation: 1,
            };
            let mut page_token = None;
            for _ in 0..1024 {
                let requested_page = page_token.clone();
                let page = provider
                    .delta_list(
                        crate::sync::provider::DeltaScope::Library {
                            namespace: context.namespace.clone(),
                            binding_epoch: context.binding_epoch.clone(),
                            transport_generation: context.transport_generation,
                        },
                        None,
                        page_token.as_deref(),
                        100,
                    )
                    .await
                    .unwrap();
                let mut candidates = Vec::new();
                for metadata in page
                    .objects
                    .iter()
                    .filter(|metadata| crate::sync::descriptor::is_descriptor_key(&metadata.key))
                {
                    let (candidate, _, _) = crate::sync::descriptor::download_candidate(
                        provider.as_ref(),
                        &context,
                        metadata,
                    )
                    .await
                    .unwrap();
                    candidates.push(candidate);
                }
                manager
                    .registry
                    .commit_provider_root_cursor(
                        FAKE_ACCOUNT_ID,
                        transport.account_generation,
                        &transport.runtime_session_id,
                        None,
                        &candidates,
                    )
                    .unwrap();
                page_token = page.page_token;
                if page_token.is_none() {
                    assert!(manager
                        .registry
                        .list_remote_libraries()
                        .unwrap()
                        .iter()
                        .any(|remote| remote.library_uuid == library_uuid
                            && remote.account_uuid == FAKE_ACCOUNT_ID
                            && remote.compatible
                            && !remote.bound_locally));
                    return;
                }
                assert_ne!(page_token, requested_page, "non-advancing fixture Delta");
            }
            panic!("fixture discovery exceeded its page limit");
        }

        let bundled_root = std::env::var_os("MANGOO_SYNC_BUNDLED_PLUGINS_DIR")
            .map(PathBuf::from)
            .expect("MANGOO_SYNC_BUNDLED_PLUGINS_DIR must identify the packaged Provider root");
        let base = std::env::temp_dir().join(format!("mangoo-real-two-device-{}", Uuid::now_v7()));
        let device_a_root = base.join("device-a");
        let device_b_root = base.join("device-b");
        let device_c_root = base.join("device-c");
        fs::create_dir_all(&device_a_root).unwrap();
        fs::create_dir_all(&device_b_root).unwrap();
        fs::create_dir_all(&device_c_root).unwrap();

        let manager_a = isolated_manager(&device_a_root, bundled_root.clone());
        let manifest = configure_fake_provider(&manager_a, &bundled_root);
        let provider_a = activate_fake_provider(
            &manager_a,
            &manifest,
            &bundled_root,
            &base.join("provider-runtime"),
        )
        .await;
        manager_a
            .registry
            .record_plugin_runtime_ready(
                FAKE_PLUGIN_ID,
                "two-device-integration",
                &["sync-provider.v1".into()],
            )
            .unwrap();

        let (library_a, _, _) = manager_a
            .create_library(
                "Shared Library".into(),
                None,
                None,
                device_a_root.join("managed"),
                None,
                false,
            )
            .await
            .unwrap();
        let catalog_a = library_a.local_data_dir.join("catalog.sqlite");
        let database_a = Database::open(&catalog_a).unwrap();
        let root_id = database_a
            .connection_for_fsops()
            .query_row("SELECT id FROM roots LIMIT 1", [], |row| row.get(0))
            .unwrap();
        let folder_id = database_a
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: "Shared".into(),
                path_hash: "shared".into(),
                mtime: Some(1),
            })
            .unwrap();
        let file_id_a = database_a
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: "seed.png".into(),
                ext: crate::decode::formats::lowercase_extension(Path::new("seed.png")),
                size: 16,
                mtime: 1,
                ctime: None,
                file_key: Some("seed-file".into()),
            })
            .unwrap();
        // This corpus exercises media-derived state. File identity alone is
        // format-neutral; seed the same classification a normal PNG scan adds.
        database_a.upsert_media_kind(file_id_a, "image").unwrap();
        let source_presentation: (String, String, bool, i64) = database_a
            .connection_for_fsops()
            .query_row(
                "SELECT f.ext,m.kind,f.browse_visible,(SELECT COUNT(*) FROM media_fts WHERE rowid=f.id AND media_fts MATCH 'seed') FROM files f JOIN media m ON m.file_id=f.id WHERE f.id=?1",
                [file_id_a],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            source_presentation,
            (".png".into(), "image".into(), true, 1)
        );
        let source_path = PathBuf::from(
            database_a
                .connection_for_fsops()
                .query_row("SELECT path FROM roots WHERE id=?1", [root_id], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
        )
        .join("Shared")
        .join("seed.png");
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        let pixels = image::RgbaImage::from_fn(1024, 1024, |x, y| {
            image::Rgba([
                x.wrapping_mul(37).wrapping_add(y) as u8,
                y.wrapping_mul(61).wrapping_add(x) as u8,
                x.wrapping_mul(y).wrapping_add(17) as u8,
                255,
            ])
        });
        let mut source_cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut source_cursor, image::ImageFormat::Png)
            .unwrap();
        let source_bytes = source_cursor.into_inner();
        fs::write(&source_path, &source_bytes).unwrap();
        database_a
            .connection_for_fsops()
            .execute(
                "UPDATE files SET size=?1 WHERE id=?2",
                rusqlite::params![source_bytes.len() as i64, file_id_a],
            )
            .unwrap();
        database_a
            .create_sync_publication_intent(
                file_id_a,
                &source_path.to_string_lossy(),
                source_bytes.len() as i64,
            )
            .unwrap();
        drop(database_a);
        manager_a
            .enable_library_sync_with_revision(
                &library_a.library_uuid,
                0,
                FAKE_PLUGIN_ID,
                FAKE_ACCOUNT_ID,
            )
            .await
            .unwrap();
        // Media dependencies are published before the event/checkpoint that
        // references them. Drive bounded turns until the seed is remotely
        // restorable through the real framed child.
        for _ in 0..8 {
            manager_a
                .sync_coordinator
                .run_library_turn(&library_a.library_uuid, &catalog_a)
                .await
                .unwrap();
        }

        let manager_b = isolated_manager(&device_b_root, bundled_root.clone());
        configure_fake_provider(&manager_b, &bundled_root);
        manager_b
            .registry
            .record_plugin_runtime_ready(
                FAKE_PLUGIN_ID,
                "two-device-integration",
                &["sync-provider.v1".into()],
            )
            .unwrap();
        manager_b
            .provider_manager()
            .register(FAKE_PLUGIN_ID, FAKE_ACCOUNT_ID, provider_a.provider.clone())
            .unwrap();
        discover_fixture_library(&manager_b, &library_a.library_uuid).await;
        let library_b = manager_b
            .create_planned_restore_fixture(
                library_a.library_uuid.clone(),
                "Shared Library".into(),
                device_b_root.join("managed"),
                FAKE_ACCOUNT_ID,
            )
            .await
            .unwrap();
        let catalog_b = library_b.local_data_dir.join("catalog.sqlite");
        assert!(!catalog_b.exists(), "restore must begin without a Catalog");
        manager_b
            .sync_coordinator
            .run_library_turn(&library_b.library_uuid, &catalog_b)
            .await
            .unwrap();
        assert!(
            catalog_b.is_file(),
            "device B did not materialize the seed Catalog"
        );
        let transport_b = manager_b
            .registry
            .binding_transport(&library_b.library_uuid, "library")
            .unwrap()
            .unwrap();
        let database_b = Database::open(&catalog_b).unwrap();
        let generation_b = database_b
            .begin_sync_capture(transport_b.binding_epoch, transport_b.actor_epoch)
            .unwrap();
        assert!(generation_b >= 1);
        drop(database_b);

        // Both devices edit while disconnected (no coordinator turns). The
        // separate actor identities and binding epochs are retained in their
        // local event ledgers until reconnection.
        let database_a = Database::open(&catalog_a).unwrap();
        database_a
            .upsert_user_metadata_partial(
                file_id_a,
                crate::db::UserMetadataPatch {
                    rating: Some(Some(5)),
                    favorite: None,
                    note: Some(Some("device-a".into())),
                },
            )
            .unwrap();
        let tag = database_a
            .create_tag("shared-tag", Some("#4477aa"))
            .unwrap();
        database_a.add_file_tag(file_id_a, tag.id).unwrap();
        let asset_uuid = database_a
            .connection_for_fsops()
            .query_row(
                "SELECT asset_uuid FROM files WHERE id=?1",
                [file_id_a],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap();
        let asset = crate::sync::capture::entity_uuid(&asset_uuid).unwrap();
        let parent_uuid = database_a
            .connection_for_fsops()
            .query_row(
                "SELECT fo.folder_uuid FROM files f JOIN folders fo ON fo.id=f.folder_id WHERE f.id=?1",
                [file_id_a],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .map(|bytes| crate::sync::types::EntityUuid(Uuid::from_slice(&bytes).unwrap()))
            .unwrap();
        let location_supersedes = database_a
            .connection_for_fsops()
            .prepare("SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1")
            .unwrap()
            .query_map([asset_uuid.as_slice()], |row| row.get::<_, Vec<u8>>(0))
            .unwrap()
            .map(|row| crate::sync::types::RevisionUuid(Uuid::from_slice(&row.unwrap()).unwrap()))
            .collect::<std::collections::BTreeSet<_>>();
        let delete_token = crate::sync::types::EventUuid::new();
        for operation in [
            crate::sync::types::Operation::SetLocation {
                revision_uuid: crate::sync::types::RevisionUuid::new(),
                parent_uuid: Some(parent_uuid),
                name: "renamed-seed.png".into(),
                supersedes: location_supersedes,
            },
            crate::sync::types::Operation::Delete {
                token: delete_token,
            },
            crate::sync::types::Operation::Restore {
                observed_tokens: std::collections::BTreeSet::from([delete_token]),
            },
        ] {
            let transaction = database_a
                .connection_for_fsops()
                .unchecked_transaction()
                .unwrap();
            crate::sync::capture::capture_catalog_mutation_if_initialized(
                &transaction,
                crate::sync::capture::MutationOrigin::LocalApi,
                asset,
                operation,
            )
            .unwrap();
            transaction.commit().unwrap();
        }
        let database_b = Database::open(&catalog_b).unwrap();
        let file_id_b: i64 = database_b
            .connection_for_fsops()
            .query_row("SELECT id FROM files LIMIT 1", [], |row| row.get(0))
            .unwrap();
        database_b
            .upsert_user_metadata_partial(
                file_id_b,
                crate::db::UserMetadataPatch {
                    rating: Some(Some(2)),
                    favorite: Some(true),
                    note: None,
                },
            )
            .unwrap();
        drop(database_a);
        drop(database_b);

        // Publish each device, then pull/apply the other's immutable segment.
        for _ in 0..4 {
            manager_a
                .sync_coordinator
                .run_library_turn(&library_a.library_uuid, &catalog_a)
                .await
                .unwrap();
            manager_b
                .sync_coordinator
                .run_library_turn(&library_b.library_uuid, &catalog_b)
                .await
                .unwrap();
        }

        let logical_digest = |path: &Path| -> (
            Vec<(String, String, String)>,
            Vec<(String, i64, i64, String)>,
            Vec<(String, i64)>,
            Vec<(String, String)>,
        ) {
            let database = Database::open(path).unwrap();
            let connection = database.connection_for_fsops();
            let files = connection
                .prepare("SELECT lower(hex(asset_uuid)),name,status FROM files ORDER BY asset_uuid")
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let metadata = connection
                .prepare("SELECT lower(hex(f.asset_uuid)),COALESCE(u.rating,-1),u.favorite,COALESCE(u.note,'') FROM files f LEFT JOIN user_metadata u ON u.file_id=f.id ORDER BY f.asset_uuid")
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let frontier = connection
                .prepare("SELECT lower(hex(f.actor_epoch)),f.applied_through_seq FROM sync_actor_frontiers f WHERE EXISTS (SELECT 1 FROM sync_events e WHERE e.actor_epoch=f.actor_epoch AND e.hlc_wall_ms>0) ORDER BY f.actor_epoch")
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let tags = connection
                .prepare("SELECT t.name,lower(hex(f.asset_uuid)) FROM tags t JOIN file_tags ft ON ft.tag_id=t.id JOIN files f ON f.id=ft.file_id ORDER BY t.name,f.asset_uuid")
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (files, metadata, frontier, tags)
        };
        let digest_a = logical_digest(&catalog_a);
        let digest_b = logical_digest(&catalog_b);
        assert_eq!(
            digest_a, digest_b,
            "two production devices did not converge"
        );
        assert_eq!(digest_a.1[0].2, 1);
        assert_eq!(digest_a.1[0].3, "device-a");
        assert_eq!(digest_a.0[0].1, "renamed-seed.png");
        assert_eq!(digest_a.0[0].2, "active");
        assert_eq!(digest_a.3[0].0, "shared-tag");

        // Repeat delivery through both live coordinators and prove idempotence.
        for _ in 0..2 {
            manager_a
                .sync_coordinator
                .run_library_turn(&library_a.library_uuid, &catalog_a)
                .await
                .unwrap();
            manager_b
                .sync_coordinator
                .run_library_turn(&library_b.library_uuid, &catalog_b)
                .await
                .unwrap();
        }
        assert_eq!(logical_digest(&catalog_a), digest_a);
        assert_eq!(logical_digest(&catalog_b), digest_b);

        let manager_c = isolated_manager(&device_c_root, bundled_root.clone());
        configure_fake_provider(&manager_c, &bundled_root);
        manager_c
            .registry
            .record_plugin_runtime_ready(
                FAKE_PLUGIN_ID,
                "third-device-integration",
                &["sync-provider.v1".into()],
            )
            .unwrap();
        manager_c
            .provider_manager()
            .register(FAKE_PLUGIN_ID, FAKE_ACCOUNT_ID, provider_a.provider.clone())
            .unwrap();
        discover_fixture_library(&manager_c, &library_a.library_uuid).await;
        let library_c = manager_c
            .create_planned_restore_fixture(
                library_a.library_uuid.clone(),
                "Shared Library".into(),
                device_c_root.join("managed"),
                FAKE_ACCOUNT_ID,
            )
            .await
            .unwrap();
        let catalog_c = library_c.local_data_dir.join("catalog.sqlite");
        manager_c
            .sync_coordinator
            .run_library_turn(&library_c.library_uuid, &catalog_c)
            .await
            .unwrap();
        for _ in 0..12 {
            manager_c
                .sync_coordinator
                .run_library_turn(&library_c.library_uuid, &catalog_c)
                .await
                .unwrap();
        }
        assert_eq!(logical_digest(&catalog_c), digest_a);
        let database_c = Database::open(&catalog_c).unwrap();
        let restored_path = database_c
            .connection_for_fsops()
            .query_row(
                "SELECT r.path,j.destination_relpath FROM media_materialization_jobs j JOIN files f ON f.asset_uuid=j.asset_uuid JOIN roots r ON r.id=f.root_id WHERE f.asset_uuid=?1 AND j.phase='complete'",
                [asset_uuid.as_slice()],
                |row| {
                    let root = PathBuf::from(row.get::<_, String>(0)?);
                    let relpath = row.get::<_, String>(1)?;
                    Ok(root.join(relpath))
                },
            )
            .unwrap();
        assert_eq!(fs::read(&restored_path).unwrap(), source_bytes);
        assert_eq!(
            restored_path.file_name().and_then(|value| value.to_str()),
            Some("renamed-seed.png")
        );
        let local_derived_counts: (i64, i64, i64) = database_c
            .connection_for_fsops()
            .query_row(
                "SELECT (SELECT COUNT(*) FROM thumbs),(SELECT COUNT(*) FROM tasks WHERE kind='thumbnail'),(SELECT COUNT(*) FROM media_fts WHERE media_fts MATCH 'renamed')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            local_derived_counts.0, 0,
            "thumbnail blobs must not be transported"
        );
        assert_eq!(local_derived_counts.1, 0, "thumbnail tasks are local-only");
        assert_eq!(
            local_derived_counts.2, 1,
            "FTS must be regenerated from restored projection"
        );
        let generated_thumb =
            crate::thumbnails::generate_image_thumbnail_bytes(&restored_path).unwrap();
        let generated_preview =
            crate::thumbnails::generate_preview_webp(&restored_path, 2048).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let restored_file_id: i64 = database_c
            .connection_for_fsops()
            .query_row(
                "SELECT id FROM files WHERE asset_uuid=?1",
                [asset_uuid.as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        database_c
            .upsert_thumb_blob(crate::db::ThumbBlobRecord {
                file_id: restored_file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.into(),
                data: generated_thumb.data,
                width: generated_thumb.width,
                height: generated_thumb.height,
                byte_size: generated_thumb.byte_size,
                output_format: crate::thumbnails::GENERATED_FORMAT.into(),
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        database_c
            .upsert_preview_blob(crate::db::PreviewBlobRecord {
                file_id: restored_file_id,
                profile: crate::thumbnails::PREVIEW_2048_PROFILE.into(),
                data: generated_preview.data,
                width: generated_preview.width,
                height: generated_preview.height,
                byte_size: generated_preview.byte_size,
                output_format: generated_preview.output_format.into(),
                source_size: source_bytes.len() as i64,
                source_mtime: 1,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
        let regenerated: (i64, i64) = database_c
            .connection_for_fsops()
            .query_row(
                "SELECT (SELECT COUNT(*) FROM thumb_blobs WHERE file_id=?1),(SELECT COUNT(*) FROM preview_blobs WHERE file_id=?1)",
                [restored_file_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(regenerated, (1, 1), "derived media must regenerate locally");
        let provider_diagnostics = provider_a.provider.diagnostics().await.unwrap();
        let provider_state_root = base.join("provider-runtime").join("provider-state");
        let object_metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(provider_state_root.join("objects.json")).unwrap())
                .unwrap();
        assert!(object_metadata.as_object().unwrap().keys().all(|key| {
            !key.contains("thumb") && !key.contains("preview") && !key.contains("fts")
        }));
        assert_eq!(
            manager_c
                .registry
                .library(&library_c.library_uuid)
                .unwrap()
                .unwrap()
                .availability,
            "available"
        );
        println!(
            "SYNC_TWO_DEVICE_JSON={}",
            serde_json::json!({
                "providerTransport": "framed-child-rpc",
                "isolatedHosts": 3,
                "freshCatalogRestores": 2,
                "convergedFiles": digest_a.0.len(),
                "convergedFrontiers": digest_a.2.len(),
                "convergedTags": digest_a.3.len(),
                "favorite": digest_a.1[0].2,
                "note": digest_a.1[0].3,
                "renamed": digest_a.0[0].1,
                "deleteRestoreActive": digest_a.0[0].2 == "active",
                "mediaBytesRestored": source_bytes.len(),
                "localDerived": {"transportedThumbs":local_derived_counts.0,"transportedThumbnailTasks":local_derived_counts.1,"ftsMatches":local_derived_counts.2,"regeneratedThumbs":regenerated.0,"regeneratedPreviews":regenerated.1},
                "providerDiagnostics": provider_diagnostics,
                "repeatedDeliveryIdempotent": true,
            })
        );

        manager_a
            .provider_manager()
            .unregister(FAKE_PLUGIN_ID, FAKE_ACCOUNT_ID)
            .unwrap();
        manager_b
            .provider_manager()
            .unregister(FAKE_PLUGIN_ID, FAKE_ACCOUNT_ID)
            .unwrap();
        manager_c
            .provider_manager()
            .unregister(FAKE_PLUGIN_ID, FAKE_ACCOUNT_ID)
            .unwrap();
        provider_a.shutdown().await.unwrap();
        drop(database_c);
        drop(manager_a);
        drop(manager_b);
        drop(manager_c);
        let _ = fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn failed_activation_reopens_previous_runtime_without_publishing_target() {
        let base =
            std::env::temp_dir().join(format!("mangoo-activation-rollback-{}", Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let manager = test_manager(&base);
        let first = manager
            .create_library("First".into(), None, None, base.join("first"), None, false)
            .await
            .unwrap()
            .0;
        let second = manager
            .create_library(
                "Second".into(),
                None,
                None,
                base.join("second"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;
        let first_revision = manager.activate(&first.library_uuid).await.unwrap();
        fs::write(second.managed_root.join(MARKER_FILE_NAME), b"corrupt").unwrap();
        assert!(manager.activate(&second.library_uuid).await.is_err());
        let active = manager.active_snapshot();
        assert_eq!(
            active.library_uuid.as_deref(),
            Some(first.library_uuid.as_str())
        );
        assert_eq!(active.context_revision, first_revision);
        assert!(active.runtime.is_some());
        assert_eq!(
            manager
                .registry
                .snapshot()
                .unwrap()
                .active_library_uuid
                .as_deref(),
            Some(first.library_uuid.as_str())
        );
        if let Some(runtime) = active.runtime {
            runtime.shutdown().await.unwrap();
        }
        drop(manager);
        let _ = fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn queued_library_control_rechecks_revision_under_operation_lock() {
        let base = std::env::temp_dir().join(format!("mangoo-control-revision-{}", Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let manager = test_manager(&base);
        let first = manager
            .create_library("First".into(), None, None, base.join("first"), None, false)
            .await
            .unwrap()
            .0;
        let second = manager
            .create_library(
                "Second".into(),
                None,
                None,
                base.join("second"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;
        let initial_revision = manager.active_snapshot().context_revision;
        manager
            .activate_with_revision(&first.library_uuid, initial_revision)
            .await
            .unwrap();

        let stale = manager
            .activate_with_revision(&second.library_uuid, initial_revision)
            .await
            .unwrap_err();
        assert_eq!(stale.to_string(), "stale-library-context");
        assert_eq!(
            manager.active_snapshot().library_uuid.as_deref(),
            Some(first.library_uuid.as_str())
        );

        if let Some(runtime) = manager.active_snapshot().runtime {
            runtime.shutdown().await.unwrap();
        }
        drop(manager);
        let _ = fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn sync_enable_requires_a_live_ready_provider_before_creating_a_binding() {
        let base = std::env::temp_dir().join(format!("mangoo-sync-enable-{}", Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let manager = test_manager(&base);
        manager
            .registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: "com.mangoo.fake",
                version: "1.0.0",
                manifest_json: r#"{"manifestVersion":2}"#,
                entry_relative_path: "bin/provider",
                entry_sha256: &[7; 32],
                install_root: Path::new("bundled"),
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();
        manager
            .registry
            .upsert_plugin_account(
                "fake-account",
                "com.mangoo.fake",
                Some("Fake"),
                "authenticated",
            )
            .unwrap();
        manager
            .registry
            .set_plugin_intent("com.mangoo.fake", true, true)
            .unwrap();
        manager
            .registry
            .record_plugin_runtime_ready(
                "com.mangoo.fake",
                "test-session",
                &["sync-provider.v1".into()],
            )
            .unwrap();
        let library = manager
            .create_library(
                "Sync".into(),
                None,
                None,
                base.join("managed-sync"),
                None,
                false,
            )
            .await
            .unwrap()
            .0;

        assert_eq!(
            manager
                .enable_library_sync_with_revision(
                    &library.library_uuid,
                    0,
                    "com.mangoo.fake",
                    "fake-account",
                )
                .await
                .unwrap_err()
                .to_string(),
            "provider-not-ready"
        );
        assert_eq!(
            manager
                .registry
                .library(&library.library_uuid)
                .unwrap()
                .unwrap()
                .binding_state,
            "local-only"
        );

        manager
            .provider_manager()
            .register(
                "com.mangoo.fake",
                "fake-account",
                Arc::new(crate::sync::provider::InMemorySyncProvider::default()),
            )
            .unwrap();
        manager
            .enable_library_sync_with_revision(
                &library.library_uuid,
                0,
                "com.mangoo.fake",
                "fake-account",
            )
            .await
            .unwrap();
        assert_eq!(
            manager
                .registry
                .binding_transport(&library.library_uuid, "library")
                .unwrap()
                .unwrap()
                .state,
            "preparing"
        );

        drop(manager);
        let _ = fs::remove_dir_all(base);
    }
}
