use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::db::{Database, DbWriter, NewRoot};
use crate::host_registry::{CreationIntent, HostRegistry, LibraryDescriptor};
use crate::library_path::{
    atomic_write_marker_guarded, identity_token, known_cloud_roots, marker_identity, preflight,
    read_marker, safe_delete_tree_expected, safe_rename_directory, safe_rename_directory_expected,
    verify_preflight_token, LibraryMarker, MarkerState, PreflightResult, SafeDirectory,
    MARKER_FILE_NAME,
};
use crate::roots::reachability::RootReachability;
use crate::sync::runtime::{RuntimeLease, RuntimeLeases};
use crate::tasks::{start_managed_worker, TaskSender, TaskWorkerHandle};
use crate::watch::{start_watcher, WatcherHandle};

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
    pub plugins_dir: Option<PathBuf>,
    pub root_reachability: RootReachability,
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
            plugins_dir: None,
            root_reachability: Arc::new(Mutex::new(std::collections::HashMap::new())),
            worker: None,
            watcher: Mutex::new(None),
            cancellation: CancellationToken::new(),
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
        let notification_outbox_wake = database.notification_outbox_wake();
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
            plugins_dir,
            root_reachability: Arc::new(Mutex::new(std::collections::HashMap::new())),
            worker,
            watcher: Mutex::new(watcher),
            cancellation: CancellationToken::new(),
            background_tasks: Mutex::new(Vec::new()),
            background_started: AtomicBool::new(false),
            runtime_lease: Mutex::new(runtime_lease),
        }))
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.cancellation.cancel();
        let handles = self
            .background_tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain(..)
            .collect::<Vec<_>>();
        for handle in handles {
            handle
                .await
                .map_err(|error| anyhow::anyhow!("runtime background task failed: {error}"))?;
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
    registry: Arc<HostRegistry>,
    active: Arc<RwLock<ActiveSnapshot>>,
    request_gate: Arc<tokio::sync::RwLock<()>>,
    operation: AsyncMutex<()>,
    runtime_generation: AtomicU64,
    libraries_dir: PathBuf,
    bundled_plugins_dir: PathBuf,
    secret_store: Option<Arc<dyn crate::plugins::secret_store::SecretStore>>,
    runtime_leases: RuntimeLeases,
    sync_coordinator: crate::sync::coordinator::SyncCoordinator,
    sync_scheduler: Mutex<Option<crate::sync::coordinator::SyncSchedulerHandle>>,
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
        completed: tokio::sync::oneshot::Sender<()>,
    },
}

impl LibraryManager {
    pub fn host_registry(&self) -> &Arc<HostRegistry> {
        &self.registry
    }
    pub fn provider_manager(&self) -> crate::sync::provider::ProviderManager {
        self.sync_coordinator.provider_manager()
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
        let commands = self
            .plugin_runtime_scheduler
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|scheduler| scheduler.commands.clone());
        let Some(commands) = commands else {
            return Ok(());
        };
        let (completed, joined) = tokio::sync::oneshot::channel();
        commands
            .send(PluginRuntimeCommand::Stop {
                plugin_id: plugin_id.to_owned(),
                completed,
            })
            .await
            .map_err(|_| anyhow::anyhow!("plugin runtime scheduler is unavailable"))?;
        joined
            .await
            .map_err(|_| anyhow::anyhow!("plugin runtime stop acknowledgement was dropped"))
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
        Self {
            registry,
            active,
            request_gate,
            operation: AsyncMutex::new(()),
            runtime_generation: AtomicU64::new(0),
            libraries_dir,
            bundled_plugins_dir,
            secret_store,
            runtime_leases,
            sync_coordinator,
            sync_scheduler: Mutex::new(None),
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
            let mut running: HashMap<
                String,
                (String, CancellationToken, tokio::task::JoinHandle<()>),
            > = HashMap::new();
            let mut blocked: HashMap<String, String> = HashMap::new();
            loop {
                let Some(manager) = weak.upgrade() else {
                    break;
                };
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
                        let _ = handle.await;
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
                        cancel.cancel();
                        let _ = handle.await;
                    }
                }
                for launch in launches {
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
                    let child_cancel = task_cancel.child_token();
                    let supervisor_cancel = child_cancel.clone();
                    let install_root = launch.install_root.clone();
                    let runtime_root = std::env::var_os("MEGLE_PLUGIN_STATE_DIR")
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
                        if let Err(error) =
                            crate::plugins::runtime::supervisor::supervise_until_cancelled(
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
                                        let onedrive_client_id = registry.effective_onedrive_client_id()?;
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
                                            onedrive_client_id,
                                        )
                                        .await
                                    }
                                },
                            )
                            .await
                        {
                            tracing::error!(plugin_id = %task_plugin_id, %error, "plugin supervisor stopped with error");
                        }
                    });
                    running.insert(plugin_id, (fingerprint, child_cancel, handle));
                }
                tokio::select! {
                    _ = task_cancel.cancelled() => break,
                    command = command_rx.recv() => {
                        let Some(PluginRuntimeCommand::Stop { plugin_id, completed }) = command else {
                            break;
                        };
                        if let Some((_, cancel, handle)) = running.remove(&plugin_id) {
                            cancel.cancel();
                            let _ = handle.await;
                        } else if let Ok(Some(account)) = manager.registry.plugin_account_secret(&plugin_id) {
                            let _ = manager.provider_manager().unregister(&plugin_id, &account.account_uuid);
                        }
                        let _ = completed.send(());
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                }
            }
            for (_, (_, cancel, handle)) in running {
                cancel.cancel();
                let _ = handle.await;
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
    }

    #[allow(dead_code)]
    pub async fn shutdown_sync_coordinator(&self) -> anyhow::Result<()> {
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
        self.recover_journals()?;
        self.registry.recover_activation_journal()?;
        self.registry.recover_sync_orchestration_journals()?;
        let snapshot = self.registry.snapshot()?;
        if let Some(library_uuid) = snapshot.active_library_uuid {
            let descriptor = self
                .registry
                .library(&library_uuid)?
                .ok_or_else(|| anyhow::anyhow!("active library is missing from registry"))?;
            let runtime = self.prepare_runtime(&descriptor)?;
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
        self.recover_sync_runtime_journals().await
    }

    async fn recover_sync_runtime_journals(&self) -> anyhow::Result<()> {
        for recovery in self.registry.pending_sync_binding_recovery()? {
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
                if descriptor.local_data_dir.join("catalog.sqlite").is_file() {
                    self.registry
                        .mark_restore_catalog_ready(&restore.library_uuid)?;
                } else if restore.state == "failed" {
                    let _ = self.cancel_restore(&restore.restore_uuid).await;
                } else {
                    let _ = self.registry.wake_binding(&restore.library_uuid);
                }
            } else if restore.state == "failed" {
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
            let marker = read_marker(&recovery.resolved_root).ok();
            let committed = marker.as_ref().is_some_and(|marker| {
                marker.state == MarkerState::Committed
                    && marker.library_uuid == recovery.library_uuid
                    && marker.root_uuid == recovery.root_uuid
                    && marker.creation_txn_uuid == recovery.txn_uuid
            });
            if committed {
                let final_dir = self.libraries_dir.join(&recovery.library_uuid);
                if !final_dir.exists() && recovery.staging_dir.exists() {
                    fs::create_dir_all(&self.libraries_dir)?;
                    safe_rename_directory(&recovery.staging_dir, &final_dir)?;
                }
                let marker_identity =
                    marker_identity(&recovery.resolved_root.join(MARKER_FILE_NAME))?;
                self.registry.commit_creation(
                    &recovery.txn_uuid,
                    &LibraryDescriptor {
                        library_uuid: recovery.library_uuid,
                        root_uuid: recovery.root_uuid,
                        name: recovery.name,
                        icon: recovery.icon,
                        color: recovery.color,
                        local_data_dir: final_dir,
                        managed_root: recovery.resolved_root,
                        lifecycle_state: "ready".to_string(),
                        binding_state: "local-only".to_string(),
                        availability: "available".to_string(),
                        marker_identity,
                    },
                )?;
            } else {
                if marker.as_ref().is_some_and(|marker| {
                    marker.state == MarkerState::Pending
                        && marker.library_uuid == recovery.library_uuid
                        && marker.root_uuid == recovery.root_uuid
                        && marker.creation_txn_uuid == recovery.txn_uuid
                }) {
                    let root_guard = SafeDirectory::open(&recovery.resolved_root)?;
                    if recovery.root_identity.as_deref() != Some(root_guard.identity()) {
                        anyhow::bail!("creation recovery root identity changed");
                    }
                    root_guard.delete_file_child(std::ffi::OsStr::new(MARKER_FILE_NAME))?;
                }
                if recovery.staging_dir.exists() {
                    safe_delete_tree_expected(
                        &recovery.staging_dir,
                        recovery.staging_identity.as_deref().ok_or_else(|| {
                            anyhow::anyhow!("creation recovery staging identity missing")
                        })?,
                    )?;
                }
                if recovery.created_root
                    && recovery.resolved_root.is_dir()
                    && fs::read_dir(&recovery.resolved_root)?.next().is_none()
                {
                    safe_delete_tree_expected(
                        &recovery.resolved_root,
                        recovery.root_identity.as_deref().ok_or_else(|| {
                            anyhow::anyhow!("creation recovery root identity missing")
                        })?,
                    )?;
                }
                self.registry
                    .mark_creation_rolled_back(&recovery.txn_uuid)?;
            }
        }
        for recovery in self.registry.pending_removal_recovery()? {
            if recovery.phase == "requested" {
                // The rename can commit before the following journal phase
                // update. Inspect the recorded quarantine identities instead
                // of assuming `requested` means no filesystem work happened.
                if let Some(local) = recovery.local_quarantine.as_ref() {
                    if local.exists() && !recovery.local_data_dir.exists() {
                        safe_rename_directory_expected(
                            local,
                            &recovery.local_data_dir,
                            recovery
                                .local_quarantine_identity
                                .as_deref()
                                .ok_or_else(|| {
                                    anyhow::anyhow!("local quarantine identity missing")
                                })?,
                        )?;
                    }
                }
                if let Some(root) = recovery.root_quarantine.as_ref() {
                    if root.exists() && !recovery.managed_root.exists() {
                        safe_rename_directory_expected(
                            root,
                            &recovery.managed_root,
                            recovery
                                .root_quarantine_identity
                                .as_deref()
                                .ok_or_else(|| {
                                    anyhow::anyhow!("root quarantine identity missing")
                                })?,
                        )?;
                    }
                }
                self.registry
                    .rollback_removal(&recovery.txn_uuid, &recovery.library_uuid)?;
                continue;
            }
            if recovery.phase == "root-quarantined" {
                if let Some(local) = recovery.local_quarantine.as_ref() {
                    if local.exists() && !recovery.local_data_dir.exists() {
                        safe_rename_directory_expected(
                            local,
                            &recovery.local_data_dir,
                            recovery
                                .local_quarantine_identity
                                .as_deref()
                                .ok_or_else(|| {
                                    anyhow::anyhow!("local quarantine identity missing")
                                })?,
                        )?;
                    }
                }
                if let Some(root) = recovery.root_quarantine.as_ref() {
                    if root.exists() && !recovery.managed_root.exists() {
                        safe_rename_directory_expected(
                            root,
                            &recovery.managed_root,
                            recovery
                                .root_quarantine_identity
                                .as_deref()
                                .ok_or_else(|| {
                                    anyhow::anyhow!("root quarantine identity missing")
                                })?,
                        )?;
                    }
                }
                self.registry
                    .rollback_removal(&recovery.txn_uuid, &recovery.library_uuid)?;
                continue;
            }
            if recovery.phase == "committed" || recovery.phase == "quarantined" {
                let root_ok = recovery.root_quarantine.as_ref().is_some_and(|root| {
                    !root.exists()
                        || (SafeDirectory::open(root).is_ok_and(|guard| {
                            recovery.root_quarantine_identity.as_deref() == Some(guard.identity())
                        }) && marker_identity(&root.join(MARKER_FILE_NAME))
                            .is_ok_and(|identity| identity == recovery.marker_identity))
                });
                let local_ok = recovery.local_quarantine.as_ref().is_some_and(|local| {
                    !local.exists()
                        || (SafeDirectory::open(local).is_ok_and(|guard| {
                            recovery.local_quarantine_identity.as_deref() == Some(guard.identity())
                        }) && local.join("catalog.sqlite").is_file())
                });
                if root_ok && local_ok {
                    if recovery.phase == "quarantined" {
                        let active_library = self.registry.snapshot()?.active_library_uuid;
                        if active_library.as_deref() == Some(recovery.library_uuid.as_str()) {
                            if let (Some(root), Some(local)) = (
                                recovery.root_quarantine.as_ref(),
                                recovery.local_quarantine.as_ref(),
                            ) {
                                safe_rename_directory_expected(
                                    local,
                                    &recovery.local_data_dir,
                                    recovery.local_quarantine_identity.as_deref().ok_or_else(
                                        || anyhow::anyhow!("local quarantine identity missing"),
                                    )?,
                                )?;
                                safe_rename_directory_expected(
                                    root,
                                    &recovery.managed_root,
                                    recovery.root_quarantine_identity.as_deref().ok_or_else(
                                        || anyhow::anyhow!("root quarantine identity missing"),
                                    )?,
                                )?;
                                self.registry
                                    .rollback_removal(&recovery.txn_uuid, &recovery.library_uuid)?;
                            }
                            continue;
                        }
                        self.registry
                            .commit_removal(&recovery.txn_uuid, &recovery.library_uuid)?;
                    }
                    if let Some(root) = recovery.root_quarantine {
                        if root.exists() {
                            safe_delete_tree_expected(
                                &root,
                                recovery
                                    .root_quarantine_identity
                                    .as_deref()
                                    .ok_or_else(|| {
                                        anyhow::anyhow!("root quarantine identity missing")
                                    })?,
                            )?;
                        }
                    }
                    if let Some(local) = recovery.local_quarantine {
                        if local.exists() {
                            safe_delete_tree_expected(
                                &local,
                                recovery
                                    .local_quarantine_identity
                                    .as_deref()
                                    .ok_or_else(|| {
                                        anyhow::anyhow!("local quarantine identity missing")
                                    })?,
                            )?;
                        }
                    }
                    self.registry.mark_removal_complete(&recovery.txn_uuid)?;
                }
            }
        }
        Ok(())
    }

    pub fn preflight(&self, name: &str, managed_root: &Path) -> anyhow::Result<PreflightResult> {
        let roots = self
            .registry
            .snapshot()?
            .libraries
            .into_iter()
            .map(|library| library.managed_root)
            .collect::<Vec<_>>();
        Ok(preflight(name, managed_root, &roots, &known_cloud_roots()))
    }

    pub async fn create_library(
        &self,
        name: String,
        icon: Option<String>,
        color: Option<String>,
        managed_root: PathBuf,
        preflight_token: Option<String>,
        activate: bool,
    ) -> anyhow::Result<(LibraryDescriptor, bool, Option<String>)> {
        self.create_library_internal(
            name,
            icon,
            color,
            managed_root,
            preflight_token,
            activate,
            None,
            true,
        )
        .await
    }

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
                None,
                false,
                Some(library_uuid),
                false,
            )
            .await?;
        self.registry
            .begin_restore_binding(&descriptor.library_uuid, account_uuid)?;
        self.sync_coordinator.wake();
        Ok(descriptor)
    }

    async fn create_library_internal(
        &self,
        name: String,
        icon: Option<String>,
        color: Option<String>,
        managed_root: PathBuf,
        preflight_token: Option<String>,
        activate: bool,
        requested_library_uuid: Option<String>,
        initialize_catalog: bool,
    ) -> anyhow::Result<(LibraryDescriptor, bool, Option<String>)> {
        let _operation = self.operation.lock().await;
        let validation = self.preflight(&name, &managed_root)?;
        if !validation.ok {
            anyhow::bail!("{}", validation.reason.as_str());
        }
        if let Some(token) = preflight_token.as_deref() {
            verify_preflight_token(&validation.normalized_root, token)
                .map_err(|reason| anyhow::anyhow!(reason.as_str()))?;
        }
        let library_uuid = requested_library_uuid.unwrap_or_else(|| Uuid::now_v7().to_string());
        Uuid::parse_str(&library_uuid).map_err(|_| anyhow::anyhow!("invalid-library-uuid"))?;
        if self.registry.library(&library_uuid)?.is_some() {
            anyhow::bail!("library-already-present");
        }
        let root_uuid = Uuid::now_v7().to_string();
        let txn_uuid = Uuid::now_v7().to_string();
        let local_staging_dir = self
            .libraries_dir
            .join(format!("{library_uuid}.staging.{txn_uuid}"));
        let local_data_dir = self.libraries_dir.join(&library_uuid);
        let mut descriptor = LibraryDescriptor {
            library_uuid: library_uuid.clone(),
            root_uuid: root_uuid.clone(),
            name: name.clone(),
            icon,
            color,
            local_data_dir: local_data_dir.clone(),
            managed_root: validation.normalized_root.clone(),
            lifecycle_state: "ready".to_string(),
            binding_state: "local-only".to_string(),
            availability: "available".to_string(),
            marker_identity: String::new(),
        };
        self.registry.begin_creation(&CreationIntent {
            txn_uuid: txn_uuid.clone(),
            descriptor: descriptor.clone(),
            requested_root: managed_root,
            staging_dir: local_staging_dir.clone(),
        })?;

        let created_root = !descriptor.managed_root.exists();
        self.registry.update_creation_phase(
            &txn_uuid,
            "root-reserved",
            created_root,
            false,
            None,
            None,
        )?;
        let created_root_identity = if created_root {
            let parent = descriptor
                .managed_root
                .parent()
                .ok_or_else(|| anyhow::anyhow!("invalid-path"))?;
            let leaf = descriptor
                .managed_root
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("invalid-path"))?;
            let parent_guard =
                SafeDirectory::open(parent).map_err(|_| anyhow::anyhow!("path-changed"))?;
            parent_guard
                .create_directory(leaf)
                .map_err(|_| anyhow::anyhow!("path-changed"))?;
            parent_guard
                .verify_unchanged()
                .map_err(|_| anyhow::anyhow!("path-changed"))?;
            Some(
                parent_guard
                    .child_identity(leaf, true)
                    .map_err(|_| anyhow::anyhow!("path-changed"))?,
            )
        } else {
            None
        };
        let root_guard = SafeDirectory::open(&descriptor.managed_root)
            .map_err(|_| anyhow::anyhow!("path-changed"))?;
        if created_root_identity
            .as_deref()
            .is_some_and(|identity| root_guard.identity() != identity)
        {
            self.registry.update_creation_phase(
                &txn_uuid,
                "error",
                created_root,
                false,
                None,
                Some("path-changed"),
            )?;
            anyhow::bail!("path-changed");
        }
        self.registry
            .update_creation_identities(&txn_uuid, Some(root_guard.identity()), None)?;
        let effective_token = identity_token(&descriptor.managed_root)?;
        if !created_root && validation.token.as_deref() != Some(effective_token.as_str()) {
            self.registry.update_creation_phase(
                &txn_uuid,
                "error",
                created_root,
                false,
                None,
                Some("path-changed"),
            )?;
            anyhow::bail!("path-changed");
        }
        let pending_marker = LibraryMarker {
            format_version: 1,
            library_uuid: library_uuid.clone(),
            root_uuid: root_uuid.clone(),
            state: MarkerState::Pending,
            creation_txn_uuid: txn_uuid.clone(),
        };
        let pending_identity = atomic_write_marker_guarded(&root_guard, &pending_marker)?;
        self.registry.update_creation_phase(
            &txn_uuid,
            "marker-pending",
            created_root,
            true,
            Some(&pending_identity),
            None,
        )?;
        fs::create_dir_all(&local_staging_dir)?;
        let staging_guard = SafeDirectory::open(&local_staging_dir)?;
        self.registry.update_creation_identities(
            &txn_uuid,
            None,
            Some(staging_guard.identity()),
        )?;
        if initialize_catalog {
            let catalog_path = local_staging_dir.join("catalog.sqlite");
            let catalog = Database::open(&catalog_path)?;
            catalog.apply_migrations()?;
            catalog.add_root(NewRoot {
                path: descriptor.managed_root.to_string_lossy().into_owned(),
                display_name: name.clone(),
            })?;
            catalog.initialize_sync_identity(
                Uuid::parse_str(&library_uuid)?,
                Uuid::parse_str(&root_uuid)?,
                self.registry.device_uuid()?,
            )?;
        }
        self.registry.update_creation_phase(
            &txn_uuid,
            "prepared",
            created_root,
            true,
            Some(&pending_identity),
            None,
        )?;
        let committed_marker = LibraryMarker {
            state: MarkerState::Committed,
            ..pending_marker
        };
        descriptor.marker_identity = atomic_write_marker_guarded(&root_guard, &committed_marker)?;
        root_guard
            .verify_unchanged()
            .map_err(|_| anyhow::anyhow!("path-changed"))?;
        self.registry.update_creation_phase(
            &txn_uuid,
            "marker-committed",
            created_root,
            true,
            Some(&descriptor.marker_identity),
            None,
        )?;
        fs::create_dir_all(&self.libraries_dir)?;
        fs::rename(&local_staging_dir, &local_data_dir)?;
        self.registry.commit_creation(&txn_uuid, &descriptor)?;

        let mut activated = false;
        let mut activation_error = None;
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
        self.activate_locked(&descriptor).await
    }

    async fn activate_locked(&self, descriptor: &LibraryDescriptor) -> anyhow::Result<u64> {
        if descriptor.lifecycle_state != "ready" || descriptor.availability != "available" {
            anyhow::bail!("library-unavailable");
        }
        let _gate = self.request_gate.write().await;
        let previous_snapshot = self.active_snapshot();
        *self.active.write().unwrap_or_else(|p| p.into_inner()) = ActiveSnapshot {
            library_uuid: previous_snapshot.library_uuid.clone(),
            context_revision: previous_snapshot.context_revision,
            runtime_generation: previous_snapshot.runtime_generation,
            runtime: None,
        };
        if let Some(previous) = &previous_snapshot.runtime {
            previous.shutdown().await.map_err(|error| {
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
                candidate.shutdown().await.map_err(|shutdown_error| {
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
            candidate.shutdown().await.map_err(|shutdown_error| {
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
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        if descriptor.binding_state != "local-only" {
            anyhow::bail!("library-is-synchronized");
        }
        verify_descriptor_identity(&descriptor)?;
        let txn_uuid = Uuid::now_v7().to_string();
        self.registry.begin_removal(&descriptor, &txn_uuid)?;
        let _gate = self.request_gate.write().await;
        let previous = self.active_snapshot();
        let was_active = previous.library_uuid.as_deref() == Some(library_uuid);
        if was_active {
            *self.active.write().unwrap_or_else(|p| p.into_inner()) = ActiveSnapshot {
                library_uuid: previous.library_uuid.clone(),
                context_revision: previous.context_revision,
                runtime_generation: previous.runtime_generation,
                runtime: None,
            };
            if let Some(active) = &previous.runtime {
                if let Err(error) = active.shutdown().await {
                    self.registry.rollback_removal(&txn_uuid, library_uuid)?;
                    return Err(error);
                }
            }
        }
        verify_descriptor_identity(&descriptor)?;
        let root_quarantine = descriptor
            .managed_root
            .with_file_name(format!(".megle-quarantine-{txn_uuid}"));
        let local_quarantine = descriptor
            .local_data_dir
            .with_file_name(format!(".megle-quarantine-{txn_uuid}"));
        let root_quarantine_identity = SafeDirectory::open(&descriptor.managed_root)?
            .identity()
            .to_string();
        let local_quarantine_identity = SafeDirectory::open(&descriptor.local_data_dir)?
            .identity()
            .to_string();
        self.registry.update_removal_partial(
            &txn_uuid,
            "requested",
            Some(&root_quarantine),
            Some(&local_quarantine),
            Some(&root_quarantine_identity),
            Some(&local_quarantine_identity),
        )?;
        if let Err(error) = safe_rename_directory_expected(
            &descriptor.managed_root,
            &root_quarantine,
            &root_quarantine_identity,
        ) {
            self.registry.rollback_removal(&txn_uuid, library_uuid)?;
            if was_active {
                self.restore_previous_snapshot(&previous)?;
            }
            return Err(error.into());
        }
        self.registry.update_removal_partial(
            &txn_uuid,
            "root-quarantined",
            Some(&root_quarantine),
            Some(&local_quarantine),
            Some(&root_quarantine_identity),
            None,
        )?;
        if let Err(error) = safe_rename_directory_expected(
            &descriptor.local_data_dir,
            &local_quarantine,
            &local_quarantine_identity,
        ) {
            if safe_rename_directory(&root_quarantine, &descriptor.managed_root).is_ok() {
                self.registry.rollback_removal(&txn_uuid, library_uuid)?;
                if was_active {
                    self.restore_previous_snapshot(&previous)?;
                }
            }
            return Err(error.into());
        }
        self.registry.update_removal_quarantine(
            &txn_uuid,
            &root_quarantine,
            &local_quarantine,
            &root_quarantine_identity,
            &local_quarantine_identity,
        )?;
        if was_active {
            let activation_txn = Uuid::now_v7().to_string();
            let revision = match self.registry.begin_activation(&activation_txn, None) {
                Ok(revision) => revision,
                Err(error) => {
                    safe_rename_directory(&local_quarantine, &descriptor.local_data_dir)?;
                    safe_rename_directory(&root_quarantine, &descriptor.managed_root)?;
                    self.registry.rollback_removal(&txn_uuid, library_uuid)?;
                    self.restore_previous_snapshot(&previous)?;
                    return Err(error);
                }
            };
            if let Err(error) = self
                .registry
                .commit_activation(&activation_txn, None, revision)
            {
                self.registry
                    .rollback_activation(&activation_txn, "remove-activation-failed")
                    .ok();
                safe_rename_directory(&local_quarantine, &descriptor.local_data_dir)?;
                safe_rename_directory(&root_quarantine, &descriptor.managed_root)?;
                self.registry.rollback_removal(&txn_uuid, library_uuid)?;
                self.restore_previous_snapshot(&previous)?;
                return Err(error);
            }
            *self.active.write().unwrap_or_else(|p| p.into_inner()) = ActiveSnapshot {
                library_uuid: None,
                context_revision: revision,
                runtime_generation: previous.runtime_generation,
                runtime: None,
            };
        }
        self.registry.commit_removal(&txn_uuid, library_uuid)?;
        let expected_marker_identity = descriptor.marker_identity.clone();
        let registry = Arc::clone(&self.registry);
        let cleanup_txn_uuid = txn_uuid.clone();
        tokio::task::spawn_blocking(move || {
            let root_marker = root_quarantine.join(MARKER_FILE_NAME);
            let root_identity_matches = marker_identity(&root_marker)
                .is_ok_and(|actual| actual == expected_marker_identity);
            let local_catalog_exists = local_quarantine.join("catalog.sqlite").is_file();
            if root_identity_matches && local_catalog_exists {
                let root_result =
                    safe_delete_tree_expected(&root_quarantine, &root_quarantine_identity);
                let local_result =
                    safe_delete_tree_expected(&local_quarantine, &local_quarantine_identity);
                if root_result.is_ok()
                    && local_result.is_ok()
                    && !root_quarantine.exists()
                    && !local_quarantine.exists()
                {
                    if let Err(error) = registry.mark_removal_complete(&cleanup_txn_uuid) {
                        tracing::error!(%error, "failed to mark Library quarantine cleanup complete");
                    }
                } else {
                    tracing::error!(
                        root_error = ?root_result.err(),
                        local_error = ?local_result.err(),
                        "Library quarantine cleanup remains pending"
                    );
                }
            } else {
                tracing::error!(
                    root_identity_matches,
                    local_catalog_exists,
                    "refused to delete Library quarantine after identity verification failed"
                );
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
            .ok_or_else(|| anyhow::anyhow!("restore-not-found"))?;
        if plan.catalog_ready {
            anyhow::bail!("restore-action-not-allowed");
        }
        let descriptor = self
            .registry
            .library(&plan.library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        verify_descriptor_identity(&descriptor)?;
        if descriptor.local_data_dir.join("catalog.sqlite").exists() {
            anyhow::bail!("restore-action-not-allowed");
        }
        if let Some(transport) = self
            .registry
            .binding_transport(&plan.library_uuid, "library")?
        {
            let next_generation = self
                .registry
                .begin_unbind(&plan.library_uuid, transport.binding_epoch)?;
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
        }
        let root_identity = SafeDirectory::open(&descriptor.managed_root)?
            .identity()
            .to_string();
        let local_identity = SafeDirectory::open(&descriptor.local_data_dir)?
            .identity()
            .to_string();
        let cleanup_uuid = Uuid::now_v7();
        let root_quarantine = descriptor
            .managed_root
            .with_file_name(format!(".megle-restore-cancel-{cleanup_uuid}"));
        let local_quarantine = descriptor
            .local_data_dir
            .with_file_name(format!(".megle-restore-cancel-{cleanup_uuid}"));
        safe_rename_directory_expected(
            &descriptor.local_data_dir,
            &local_quarantine,
            &local_identity,
        )?;
        if let Err(error) = safe_rename_directory_expected(
            &descriptor.managed_root,
            &root_quarantine,
            &root_identity,
        ) {
            safe_rename_directory(&local_quarantine, &descriptor.local_data_dir)?;
            return Err(error.into());
        }
        let cancelled = match self
            .registry
            .commit_cancelled_restore_cleanup(restore_uuid, &plan.library_uuid)
        {
            Ok(plan) => plan,
            Err(error) => {
                safe_rename_directory(&root_quarantine, &descriptor.managed_root)?;
                safe_rename_directory(&local_quarantine, &descriptor.local_data_dir)?;
                return Err(error);
            }
        };
        safe_delete_tree_expected(&local_quarantine, &local_identity)?;
        safe_delete_tree_expected(&root_quarantine, &root_identity)?;
        Ok(cancelled)
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
        self.registry.set_binding_state(library_uuid, "active")?;
        self.sync_coordinator
            .resume_media_binding(transport.binding_epoch)
            .await;
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
        let database = Database::open(descriptor.local_data_dir.join("catalog.sqlite"))?;
        database.apply_migrations()?;
        database.initialize_sync_identity(
            Uuid::parse_str(&descriptor.library_uuid)?,
            Uuid::parse_str(&descriptor.root_uuid)?,
            self.registry.device_uuid()?,
        )?;
        let registry = Arc::clone(&self.registry);
        let coordinator = self.sync_coordinator.clone();
        let wake_library_uuid = descriptor.library_uuid.clone();
        LibraryRuntimeResources::from_database_with_commit_hook(
            Some(descriptor.library_uuid.clone()),
            generation,
            database,
            Some(descriptor.local_data_dir.join("plugins")),
            Some(self.runtime_leases.try_acquire(&descriptor.library_uuid)?),
            Some(Arc::new(move || {
                let _ = registry.wake_binding(&wake_library_uuid);
                coordinator.wake();
            })),
        )
    }
}

fn verify_descriptor_identity(descriptor: &LibraryDescriptor) -> anyhow::Result<()> {
    let marker = read_marker(&descriptor.managed_root)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_PLUGIN_ID: &str = "com.megle.fake-sync";
    const FAKE_ACCOUNT_ID: &str = "fake-account";

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
            None,
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
    fn startup_recovery_rolls_back_an_uncommitted_creation() {
        let base = std::env::temp_dir().join(format!("megle-create-rollback-{}", Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let manager = test_manager(&base);
        let intent = creation_intent(&base, "rollback");
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

        assert!(!intent.staging_dir.exists());
        assert!(manager
            .registry
            .pending_creation_recovery()
            .unwrap()
            .is_empty());
        assert!(manager
            .registry
            .library(&intent.descriptor.library_uuid)
            .unwrap()
            .is_none());
        drop(manager);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn startup_recovery_finishes_committed_creation_and_quarantined_removal() {
        let base = std::env::temp_dir().join(format!("megle-journal-recovery-{}", Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let manager = test_manager(&base);
        let intent = creation_intent(&base, "finish");
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
        fs::write(
            intent.descriptor.managed_root.join(MARKER_FILE_NAME),
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();
        manager
            .registry
            .update_creation_phase(&intent.txn_uuid, "marker-committed", true, true, None, None)
            .unwrap();

        manager.recover_journals().unwrap();
        let descriptor = manager
            .registry
            .library(&intent.descriptor.library_uuid)
            .unwrap()
            .expect("creation committed");
        assert!(descriptor.local_data_dir.join("catalog.sqlite").is_file());

        let removal_txn = "removal-finish";
        manager
            .registry
            .begin_removal(&descriptor, removal_txn)
            .unwrap();
        let root_quarantine = base.join("root-quarantine");
        let local_quarantine = base.join("local-quarantine");
        fs::rename(&descriptor.managed_root, &root_quarantine).unwrap();
        fs::rename(&descriptor.local_data_dir, &local_quarantine).unwrap();
        let root_identity = SafeDirectory::open(&root_quarantine)
            .unwrap()
            .identity()
            .to_string();
        let local_identity = SafeDirectory::open(&local_quarantine)
            .unwrap()
            .identity()
            .to_string();
        manager
            .registry
            .update_removal_quarantine(
                removal_txn,
                &root_quarantine,
                &local_quarantine,
                &root_identity,
                &local_identity,
            )
            .unwrap();

        manager.recover_journals().unwrap();
        assert!(manager
            .registry
            .library(&descriptor.library_uuid)
            .unwrap()
            .is_none());
        assert!(!root_quarantine.exists());
        assert!(!local_quarantine.exists());
        assert!(manager
            .registry
            .pending_removal_recovery()
            .unwrap()
            .is_empty());
        drop(manager);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn startup_recovery_rolls_back_requested_and_partial_removal_states() {
        let base = std::env::temp_dir().join(format!("megle-removal-matrix-{}", Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let manager = test_manager(&base);

        let first = creation_intent(&base, "requested");
        manager.registry.begin_creation(&first).unwrap();
        fs::create_dir_all(&first.descriptor.managed_root).unwrap();
        fs::create_dir_all(&first.descriptor.local_data_dir).unwrap();
        fs::write(
            first.descriptor.local_data_dir.join("catalog.sqlite"),
            b"catalog",
        )
        .unwrap();
        let marker = LibraryMarker {
            format_version: 1,
            library_uuid: first.descriptor.library_uuid.clone(),
            root_uuid: first.descriptor.root_uuid.clone(),
            state: MarkerState::Committed,
            creation_txn_uuid: first.txn_uuid.clone(),
        };
        fs::write(
            first.descriptor.managed_root.join(MARKER_FILE_NAME),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        let mut descriptor = first.descriptor.clone();
        descriptor.marker_identity =
            marker_identity(&descriptor.managed_root.join(MARKER_FILE_NAME)).unwrap();
        manager
            .registry
            .commit_creation(&first.txn_uuid, &descriptor)
            .unwrap();
        manager
            .registry
            .begin_removal(&descriptor, "requested-removal")
            .unwrap();
        manager.recover_journals().unwrap();
        assert_eq!(
            manager
                .registry
                .library(&descriptor.library_uuid)
                .unwrap()
                .unwrap()
                .lifecycle_state,
            "ready"
        );

        manager
            .registry
            .begin_removal(&descriptor, "partial-removal")
            .unwrap();
        let root_quarantine = descriptor
            .managed_root
            .with_file_name("partial-root-quarantine");
        let local_quarantine = descriptor
            .local_data_dir
            .with_file_name("partial-local-quarantine");
        safe_rename_directory(&descriptor.managed_root, &root_quarantine).unwrap();
        safe_rename_directory(&descriptor.local_data_dir, &local_quarantine).unwrap();
        manager
            .registry
            .update_removal_partial(
                "partial-removal",
                "root-quarantined",
                Some(&root_quarantine),
                Some(&local_quarantine),
                Some(SafeDirectory::open(&root_quarantine).unwrap().identity()),
                Some(SafeDirectory::open(&local_quarantine).unwrap().identity()),
            )
            .unwrap();
        manager.recover_journals().unwrap();
        assert!(descriptor.managed_root.exists());
        assert!(descriptor.local_data_dir.exists());
        assert!(!root_quarantine.exists());
        assert!(!local_quarantine.exists());
        assert_eq!(
            manager
                .registry
                .library(&descriptor.library_uuid)
                .unwrap()
                .unwrap()
                .lifecycle_state,
            "ready"
        );
        drop(manager);
        let _ = fs::remove_dir_all(base);
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

    #[tokio::test]
    async fn duplicate_create_for_same_root_converges_to_one_library() {
        let base = std::env::temp_dir().join(format!("megle-double-create-{}", Uuid::now_v7()));
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
        let base = std::env::temp_dir().join(format!("megle-clean-bootstrap-{}", Uuid::now_v7()));
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
        assert_eq!(migration, 29);
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
        let phase = std::env::var("MEGLE_DOCKER_PERSISTENCE_PHASE")
            .expect("MEGLE_DOCKER_PERSISTENCE_PHASE must be create or verify");
        let base = PathBuf::from(
            std::env::var("MEGLE_DOCKER_PERSISTENCE_ROOT")
                .expect("MEGLE_DOCKER_PERSISTENCE_ROOT must be mounted from a Docker volume"),
        );
        let bundled_root = PathBuf::from(
            std::env::var("MEGLE_SYNC_BUNDLED_PLUGINS_DIR")
                .expect("MEGLE_SYNC_BUNDLED_PLUGINS_DIR must identify packaged Providers"),
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
            let marker = b"megle-docker-volume-sync-state".to_vec();
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
        let bundled_root = std::env::var_os("MEGLE_SYNC_BUNDLED_PLUGINS_DIR")
            .map(PathBuf::from)
            .expect("MEGLE_SYNC_BUNDLED_PLUGINS_DIR must identify the packaged Provider root");
        let base = std::env::temp_dir().join(format!("megle-real-two-device-{}", Uuid::now_v7()));
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
                ext: "png".into(),
                size: 16,
                mtime: 1,
                ctime: None,
                file_key: Some("seed-file".into()),
            })
            .unwrap();
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
        let library_b = manager_b
            .create_restore_library(
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
        let library_c = manager_c
            .create_restore_library(
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
            std::env::temp_dir().join(format!("megle-activation-rollback-{}", Uuid::now_v7()));
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
        let base = std::env::temp_dir().join(format!("megle-control-revision-{}", Uuid::now_v7()));
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
        let base = std::env::temp_dir().join(format!("megle-sync-enable-{}", Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let manager = test_manager(&base);
        manager
            .registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: "com.megle.fake",
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
                "com.megle.fake",
                Some("Fake"),
                "authenticated",
            )
            .unwrap();
        manager
            .registry
            .set_plugin_intent("com.megle.fake", true, true)
            .unwrap();
        manager
            .registry
            .record_plugin_runtime_ready(
                "com.megle.fake",
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
                    "com.megle.fake",
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
                "com.megle.fake",
                "fake-account",
                Arc::new(crate::sync::provider::InMemorySyncProvider::default()),
            )
            .unwrap();
        manager
            .enable_library_sync_with_revision(
                &library.library_uuid,
                0,
                "com.megle.fake",
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
