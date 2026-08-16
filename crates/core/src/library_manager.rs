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
        })
    }

    pub fn open(
        library_uuid: Option<String>,
        runtime_generation: u64,
        database_path: &Path,
        plugins_dir: Option<PathBuf>,
    ) -> anyhow::Result<Arc<Self>> {
        let database = Database::open(database_path)?;
        database.apply_migrations()?;
        Self::from_database(library_uuid, runtime_generation, database, plugins_dir)
    }

    pub fn from_database(
        library_uuid: Option<String>,
        runtime_generation: u64,
        database: Database,
        plugins_dir: Option<PathBuf>,
    ) -> anyhow::Result<Arc<Self>> {
        let notification_outbox_wake = database.notification_outbox_wake();
        let read_database_path = database.path().map(Path::to_path_buf);
        if let Err(error) = database.flush_dirty_notification_heads() {
            tracing::error!(%error, "startup notification dirty-head flush failed");
        }
        let worker_database = database.reopen()?;
        let watcher_database = database.reopen()?;
        let fsops_database = database.reopen()?.map(|db| Arc::new(Mutex::new(db)));
        let writer = DbWriter::spawn(database);
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
}

impl LibraryManager {
    pub fn new(
        registry: Arc<HostRegistry>,
        libraries_dir: PathBuf,
        active: Arc<RwLock<ActiveSnapshot>>,
        request_gate: Arc<tokio::sync::RwLock<()>>,
    ) -> Self {
        Self {
            registry,
            active,
            request_gate,
            operation: AsyncMutex::new(()),
            runtime_generation: AtomicU64::new(0),
            libraries_dir,
        }
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
        let _operation = self.operation.lock().await;
        let validation = self.preflight(&name, &managed_root)?;
        if !validation.ok {
            anyhow::bail!("{}", validation.reason.as_str());
        }
        if let Some(token) = preflight_token.as_deref() {
            verify_preflight_token(&validation.normalized_root, token)
                .map_err(|reason| anyhow::anyhow!(reason.as_str()))?;
        }
        let library_uuid = Uuid::now_v7().to_string();
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
        let catalog_path = local_staging_dir.join("catalog.sqlite");
        {
            let catalog = Database::open(&catalog_path)?;
            catalog.apply_migrations()?;
            catalog.add_root(NewRoot {
                path: descriptor.managed_root.to_string_lossy().into_owned(),
                display_name: name.clone(),
            })?;
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

    fn ensure_context_revision(&self, expected_revision: u64) -> anyhow::Result<()> {
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
        LibraryRuntimeResources::open(
            Some(descriptor.library_uuid.clone()),
            generation,
            &descriptor.local_data_dir.join("catalog.sqlite"),
            Some(descriptor.local_data_dir.join("plugins")),
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

    fn test_manager(base: &Path) -> LibraryManager {
        let registry = Arc::new(HostRegistry::open(&base.join("host.sqlite")).unwrap());
        LibraryManager::new(
            registry,
            base.join("libraries"),
            Arc::new(RwLock::new(ActiveSnapshot {
                library_uuid: None,
                context_revision: 0,
                runtime_generation: 0,
                runtime: None,
            })),
            Arc::new(tokio::sync::RwLock::new(())),
        )
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
}
