use anyhow::Context;
use rusqlite::OptionalExtension;
use sha2::Digest;
use std::path::{Path, PathBuf};

use super::runtime::{DormantSyncRuntime, RuntimeLeases};
use crate::host_registry::HostRegistry;

pub const PRODUCTION_SYNC_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
pub const PRODUCTION_SYNC_METADATA_CONCURRENCY: usize = 3;
pub const PRODUCTION_SYNC_APPLY_CONCURRENCY: usize = 1;
pub const PRODUCTION_SYNC_MEDIA_CONCURRENCY: usize = 2;
pub const PRODUCTION_SYNC_CONCURRENCY: usize = PRODUCTION_SYNC_METADATA_CONCURRENCY;

/// Joinable ownership for the production dormant-Library polling loop.
/// Cancellation aborts any turn waiting for a lease; completed turns always
/// drain their writer before returning from `shutdown`.
pub struct SyncSchedulerHandle {
    cancellation: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
    in_flight: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

struct SchedulerTurnGuard(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Drop for SchedulerTurnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

impl SyncSchedulerHandle {
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn is_idle(&self) -> bool {
        self.in_flight.load(std::sync::atomic::Ordering::Acquire) == 0
    }

    pub async fn join(self) -> anyhow::Result<()> {
        self.task
            .await
            .map_err(|error| anyhow::anyhow!("sync scheduler task failed: {error}"))
    }

    pub async fn shutdown(self) -> anyhow::Result<()> {
        self.cancel();
        self.join().await
    }
}

#[derive(Clone)]
pub struct SyncCoordinator {
    registry: std::sync::Arc<HostRegistry>,
    leases: RuntimeLeases,
    providers: super::provider::ProviderManager,
    wake: std::sync::Arc<tokio::sync::Notify>,
    metadata_budget: std::sync::Arc<tokio::sync::Semaphore>,
    apply_budget: std::sync::Arc<tokio::sync::Semaphore>,
    media_budget: std::sync::Arc<tokio::sync::Semaphore>,
    media_workers: std::sync::Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<uuid::Uuid, super::media::worker::MediaWorkerSet>,
        >,
    >,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncTurnResult {
    pub capture_pending: bool,
    pub applied_events: usize,
    pub checkpoint_built: bool,
    /// Wall time spent in the bounded authoritative ledger/truth/projection
    /// transaction. Spool enumeration and writer scheduling are reported
    /// separately by benchmarks and are not part of the transaction SLO.
    pub apply_transaction_micros: u64,
    pub seed_checkpoint: Option<(uuid::Uuid, i64)>,
}

#[derive(Debug, Clone)]
struct PublishedSegment {
    segment_uuid: [u8; 16],
    actor_epoch: uuid::Uuid,
    last_seq: u64,
    object_key: String,
}

impl SyncCoordinator {
    pub fn new(registry: std::sync::Arc<HostRegistry>, leases: RuntimeLeases) -> Self {
        Self {
            registry,
            leases,
            providers: super::provider::ProviderManager::default(),
            wake: std::sync::Arc::new(tokio::sync::Notify::new()),
            metadata_budget: std::sync::Arc::new(tokio::sync::Semaphore::new(
                PRODUCTION_SYNC_METADATA_CONCURRENCY,
            )),
            apply_budget: std::sync::Arc::new(tokio::sync::Semaphore::new(
                PRODUCTION_SYNC_APPLY_CONCURRENCY,
            )),
            media_budget: std::sync::Arc::new(tokio::sync::Semaphore::new(
                PRODUCTION_SYNC_MEDIA_CONCURRENCY,
            )),
            media_workers: Default::default(),
        }
    }

    pub fn with_providers(
        registry: std::sync::Arc<HostRegistry>,
        leases: RuntimeLeases,
        providers: super::provider::ProviderManager,
    ) -> Self {
        Self {
            registry,
            leases,
            providers,
            wake: std::sync::Arc::new(tokio::sync::Notify::new()),
            metadata_budget: std::sync::Arc::new(tokio::sync::Semaphore::new(
                PRODUCTION_SYNC_METADATA_CONCURRENCY,
            )),
            apply_budget: std::sync::Arc::new(tokio::sync::Semaphore::new(
                PRODUCTION_SYNC_APPLY_CONCURRENCY,
            )),
            media_budget: std::sync::Arc::new(tokio::sync::Semaphore::new(
                PRODUCTION_SYNC_MEDIA_CONCURRENCY,
            )),
            media_workers: Default::default(),
        }
    }

    fn begin_library_work(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<Option<super::provider::ProviderWork>> {
        let Some(descriptor) = self.registry.library(library_uuid)? else {
            return Ok(None);
        };
        if crate::library_manager::verify_descriptor_identity(&descriptor).is_err() {
            self.registry.isolate_unavailable_library(library_uuid)?;
            return Ok(None);
        }
        let Some(binding) = self.registry.binding_transport(library_uuid, "library")? else {
            return Ok(None);
        };
        if !matches!(
            binding.state.as_str(),
            "preparing" | "capturing" | "seed_ready" | "publishing" | "active" | "restoring"
        ) {
            return Ok(None);
        }
        if self
            .registry
            .require_ready_sync_provider(&binding.plugin_id, &binding.account_uuid)
            .is_err()
        {
            return Ok(None);
        }
        Ok(self.providers.begin_work(&binding.plugin_id).ok())
    }

    pub fn provider_manager(&self) -> super::provider::ProviderManager {
        self.providers.clone()
    }

    pub async fn fetch_remote_root_snapshot(
        &self,
        library_uuid: &str,
        root_uuid: uuid::Uuid,
    ) -> anyhow::Result<(crate::sync_folders::RootSnapshot, Option<String>)> {
        let _work = self
            .begin_library_work(library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("provider-not-ready"))?;
        let binding = self
            .registry
            .binding_transport(library_uuid, "library")?
            .ok_or_else(|| anyhow::anyhow!("provider-binding-unavailable"))?;
        if !matches!(
            binding.state.as_str(),
            "preparing" | "capturing" | "seed_ready" | "publishing" | "active" | "restoring"
        ) {
            anyhow::bail!("provider-binding-unavailable");
        }
        let provider = self
            .providers
            .resolve(&binding.plugin_id, &binding.account_uuid)?
            .ok_or_else(|| anyhow::anyhow!("provider-not-ready"))?;
        let namespace = binding
            .remote_namespace
            .clone()
            .unwrap_or_else(|| library_uuid.to_owned());
        let context = super::provider::TransportContext {
            namespace: namespace.clone(),
            binding_epoch: binding.binding_epoch.to_string(),
            transport_generation: binding.transport_generation as u64,
        };
        let mut page_token = None;
        let mut candidates: Vec<(
            crate::host_registry::DiscoveredRemoteLibrary,
            super::checkpoint::Checkpoint,
        )> = Vec::new();
        let mut listing_complete = false;
        for _ in 0..1024 {
            let requested_page = page_token.clone();
            let page = provider
                .delta_list(
                    super::provider::DeltaScope::Library {
                        namespace: namespace.clone(),
                        binding_epoch: binding.binding_epoch.to_string(),
                        transport_generation: binding.transport_generation as u64,
                    },
                    None,
                    page_token.as_deref(),
                    100,
                )
                .await
                .map_err(anyhow::Error::new)?;
            for metadata in page
                .objects
                .iter()
                .filter(|metadata| super::descriptor::is_descriptor_key(&metadata.key))
            {
                let (descriptor, checkpoint, _) =
                    super::descriptor::download_candidate(provider.as_ref(), &context, metadata)
                        .await?;
                let mut dominated = false;
                let mut retained = Vec::new();
                for (previous, checkpoint) in candidates.drain(..) {
                    let (covers, covered) =
                        super::descriptor::causal_relation(&descriptor, &previous)?;
                    let same_truth = descriptor.evidence.as_ref().unwrap().truth_sha256
                        == previous.evidence.as_ref().unwrap().truth_sha256;
                    if covered && (!covers || same_truth) {
                        dominated = true;
                        retained.push((previous, checkpoint));
                    } else if !covers || (covered && !same_truth) {
                        retained.push((previous, checkpoint));
                    }
                }
                candidates = retained;
                if dominated {
                    continue;
                }
                candidates.push((descriptor, checkpoint));
                if candidates.len() > 64 {
                    anyhow::bail!("descriptor causal head limit exceeded");
                }
            }
            page_token = page.page_token;
            if page_token.is_none() {
                listing_complete = true;
                break;
            }
            if page_token == requested_page {
                anyhow::bail!("provider-non-advancing-page");
            }
        }
        if !listing_complete {
            anyhow::bail!("provider-pagination-limit");
        }
        if candidates.len() > 1 {
            anyhow::bail!("descriptor-causal-conflict");
        }
        let (_, checkpoint) = candidates.pop().context("remote-checkpoint-unavailable")?;
        Ok((
            crate::sync_folders::snapshot_from_checkpoint(&checkpoint, root_uuid)?,
            Some(checkpoint.checkpoint_uuid.0.to_string()),
        ))
    }

    /// Wake the single Host scheduler after a local commit, a manual command,
    /// or an external connectivity transition. Notifications coalesce, so a
    /// burst never creates one task per Library.
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// Reconciles forward-only unbind journals against the remote namespace.
    /// A missing/failed HEAD is deliberately not treated as success: after the
    /// local cutover the journal remains retryable until the immutable remote
    /// descriptor is observably durable again, including after restart.
    pub async fn reconcile_pending_unbinds(&self) -> anyhow::Result<usize> {
        let mut completed = 0usize;
        for plan in self.registry.pending_unbind_reconciliations()? {
            let Ok(work) = self.providers.begin_work(&plan.plugin_id) else {
                continue;
            };
            let Some(provider) = self
                .providers
                .resolve(&plan.plugin_id, &plan.account_uuid)?
            else {
                self.registry.record_unbind_reconcile_failure(
                    &plan.plan_uuid,
                    "provider-not-ready",
                    15_000,
                )?;
                continue;
            };
            let context = super::provider::TransportContext {
                namespace: plan.remote_namespace.clone(),
                binding_epoch: plan.binding_epoch.to_string(),
                transport_generation: plan.transport_generation,
            };
            let visibility = async {
                let descriptor = provider
                    .object_head(&context, "library-descriptor")
                    .await
                    .map_err(anyhow::Error::new)?
                    .ok_or_else(|| anyhow::anyhow!("remote-descriptor-not-visible"))?;
                let expected = plan
                    .descriptor_sha256
                    .ok_or_else(|| anyhow::anyhow!("remote-descriptor-receipt-missing"))?;
                if descriptor.sha256 != expected {
                    anyhow::bail!("remote-descriptor-mismatch");
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            work.check_current()?;
            match visibility {
                Ok(()) => {
                    self.registry
                        .complete_unbind_plan(&plan.plan_uuid, &plan.choice)?;
                    completed += 1;
                }
                Err(error) => {
                    self.registry.record_unbind_reconcile_failure(
                        &plan.plan_uuid,
                        &normalize_sync_error_code(&error),
                        15_000,
                    )?;
                }
            }
        }
        Ok(completed)
    }

    pub async fn reconcile_pending_root_visibility(&self) -> anyhow::Result<usize> {
        let mut completed = 0usize;
        for pending in self.registry.pending_root_visibility_reconciliations()? {
            let Ok(work) = self.providers.begin_work(&pending.provider_id) else {
                continue;
            };
            let Some(provider) = self
                .providers
                .resolve(&pending.provider_id, &pending.account_uuid)?
            else {
                self.registry.retry_root_visibility_reconciliation(
                    &pending.operation_uuid,
                    "provider-not-ready",
                    15_000,
                )?;
                continue;
            };
            let context = super::provider::TransportContext {
                namespace: pending.remote_namespace.clone(),
                binding_epoch: pending.provider_binding_epoch.clone(),
                transport_generation: pending.provider_transport_generation,
            };
            let receipts = pending.receipts.as_array().cloned().unwrap_or_default();
            let mut visible = 0usize;
            let mut hidden = 0usize;
            let mut evidence = Vec::new();
            let mut retry_error: Option<String> = None;
            for receipt in receipts {
                let Some(key) = receipt.get("objectKey").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let expected = receipt
                    .get("sha256")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                match provider.object_head(&context, key).await {
                    Ok(Some(metadata)) if hex_hash(&metadata.sha256) == expected => {
                        visible += 1;
                        evidence.push(serde_json::json!({"objectKey":key,"visibility":"visible","etag":metadata.etag,"sha256":expected}));
                    }
                    Ok(None) => {
                        hidden += 1;
                        evidence.push(serde_json::json!({"objectKey":key,"visibility":"hidden","sha256":expected}));
                    }
                    Ok(Some(_)) => {
                        retry_error = Some("remote-object-mismatch".to_owned());
                        break;
                    }
                    Err(error) => {
                        retry_error = Some(normalize_sync_error_code(&anyhow::Error::new(error)));
                        break;
                    }
                }
            }
            work.check_current()?;
            if let Some(error) = retry_error {
                self.registry.retry_root_visibility_reconciliation(
                    &pending.operation_uuid,
                    &error,
                    15_000,
                )?;
            } else if visible > 0 && hidden > 0 {
                self.registry.retry_root_visibility_reconciliation(
                    &pending.operation_uuid,
                    "mixed-remote-visibility",
                    15_000,
                )?;
            } else {
                let state = if visible > 0 { "visible" } else { "hidden" };
                self.registry.finish_root_visibility_reconciliation(
                    &pending.operation_uuid,
                    state,
                    &serde_json::json!({"snapshotUuid":pending.snapshot_uuid,"objects":evidence}),
                )?;
                completed += 1;
            }
        }
        Ok(completed)
    }

    pub async fn cancel_media_binding(&self, binding_epoch: uuid::Uuid) -> anyhow::Result<()> {
        // Keep the cancelled worker set registered as an epoch tombstone. A
        // scheduler turn may already have read the old binding before unbind;
        // removing the entry would let that late turn create fresh workers for
        // the fenced epoch.
        let workers = self
            .media_workers
            .lock()
            .await
            .entry(binding_epoch)
            .or_insert_with(|| {
                super::media::worker::MediaWorkerSet::new(
                    super::media::worker::MediaWorkLimiter::new_with_temp_budget(
                        1,
                        128,
                        128 * 1024 * 1024,
                        16 * 1024 * 1024 * 1024,
                    )
                    .expect("production media limits are nonzero"),
                )
            })
            .clone();
        workers.shutdown().await
    }

    pub async fn fence_media_binding_registration(
        &self,
        binding_epoch: uuid::Uuid,
    ) -> anyhow::Result<super::media::worker::MediaRegistrationFence> {
        let workers = self
            .media_workers
            .lock()
            .await
            .entry(binding_epoch)
            .or_insert_with(|| {
                super::media::worker::MediaWorkerSet::new(
                    super::media::worker::MediaWorkLimiter::new_with_temp_budget(
                        1,
                        128,
                        128 * 1024 * 1024,
                        16 * 1024 * 1024 * 1024,
                    )
                    .expect("production media limits are nonzero"),
                )
            })
            .clone();
        workers.registration_fence().await
    }

    pub async fn resume_media_binding(&self, binding_epoch: uuid::Uuid) {
        // Pause leaves a cancelled worker set behind as an epoch tombstone so
        // an already-started turn cannot recreate work after the fence. Once
        // the explicit resume transition is serialized by LibraryManager, the
        // tombstone can be removed and the next scheduled turn creates a fresh
        // worker set for the incremented transport generation.
        self.media_workers.lock().await.remove(&binding_epoch);
    }

    pub async fn cancel_all_media_workers(&self) {
        let workers = self
            .media_workers
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for worker in workers {
            worker.cancel();
        }
    }

    pub async fn shutdown_media_workers(&self) -> anyhow::Result<()> {
        let workers = {
            let mut active = self.media_workers.lock().await;
            active
                .drain()
                .map(|(_, workers)| workers)
                .collect::<Vec<_>>()
        };
        for worker in workers {
            worker.shutdown().await?;
        }
        Ok(())
    }

    pub async fn media_backpressure_metrics(
        &self,
    ) -> Vec<(uuid::Uuid, super::media::worker::MediaBackpressureMetrics)> {
        self.media_workers
            .lock()
            .await
            .iter()
            .map(|(epoch, workers)| (*epoch, workers.metrics()))
            .collect()
    }

    pub fn spawn_scheduler<F>(
        &self,
        interval: std::time::Duration,
        concurrency: usize,
        active_runtime: F,
    ) -> SyncSchedulerHandle
    where
        F: Fn() -> Option<(String, PathBuf, crate::db::DbWriter)> + Send + Sync + 'static,
    {
        let coordinator = self.clone();
        let cancellation = tokio_util::sync::CancellationToken::new();
        let child_cancellation = cancellation.clone();
        let active_runtime = std::sync::Arc::new(active_runtime);
        let in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let task_in_flight = std::sync::Arc::clone(&in_flight);
        let task = tokio::spawn(async move {
            let metadata_concurrency = concurrency.max(1);
            let mut rotation = 0usize;
            let mut initial_turn = true;
            let _ = coordinator.registry.set_sync_scheduler_state("running");
            loop {
                if initial_turn {
                    initial_turn = false;
                } else {
                    tokio::select! {
                        _ = child_cancellation.cancelled() => break,
                        _ = coordinator.wake.notified() => {},
                        _ = tokio::time::sleep(interval) => {}
                    }
                }

                let mut libraries = match coordinator.registry.sync_bound_libraries() {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = coordinator.registry.set_sync_scheduler_state("degraded");
                        tracing::warn!(%error, "sync coordinator registry poll failed");
                        continue;
                    }
                };
                if let Err(error) = coordinator.reconcile_pending_unbinds().await {
                    tracing::warn!(%error, "unbind remote reconciliation turn failed");
                }
                if let Err(error) = coordinator.reconcile_pending_root_visibility().await {
                    tracing::warn!(%error, "root stop visibility reconciliation turn failed");
                }
                let active = active_runtime().and_then(|runtime| {
                    libraries
                        .iter()
                        .position(|library| library.library_uuid == runtime.0)
                        .map(|index| {
                            libraries.remove(index);
                            runtime
                        })
                });
                libraries.sort_by(|left, right| left.library_uuid.cmp(&right.library_uuid));
                if !libraries.is_empty() {
                    let offset = rotation % libraries.len();
                    libraries.rotate_left(offset);
                    rotation = (rotation + 1) % libraries.len();
                }

                // Reserve the first metadata/apply opportunity for the active
                // Library. Dormant work cannot consume its slot even when a
                // provider discovery request is already queued.
                if let Some((library_uuid, path, writer)) = active.as_ref() {
                    task_in_flight.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    let _guard = SchedulerTurnGuard(std::sync::Arc::clone(&task_in_flight));
                    let lifecycle = coordinator.begin_library_work(library_uuid).ok().flatten();
                    if let Some(lifecycle) = lifecycle {
                        match coordinator
                            .run_active_metadata_turn(library_uuid, path, writer)
                            .await
                        {
                            Ok(result)
                                if !result.capture_pending && lifecycle.check_current().is_ok() =>
                            {
                                let _ = coordinator
                                    .registry
                                    .record_sync_turn_success(library_uuid, true);
                            }
                            Ok(_) => {}
                            Err(error) if lifecycle.check_current().is_ok() => {
                                let code = normalize_sync_error_code(&error);
                                let _ = coordinator
                                    .registry
                                    .record_sync_turn_error(library_uuid, &code);
                                tracing::warn!(%error,%library_uuid,"active sync metadata turn failed");
                            }
                            Err(_) => {}
                        }
                        if lifecycle.check_current().is_ok() {
                            if let Err(error) =
                                coordinator.refresh_pending_metrics(library_uuid, path)
                            {
                                tracing::warn!(%error,"refresh sync pending metrics failed");
                            }
                        }
                    }
                }

                let metadata_gate =
                    std::sync::Arc::new(tokio::sync::Semaphore::new(metadata_concurrency));
                let mut metadata_turns = tokio::task::JoinSet::new();
                {
                    let coordinator = coordinator.clone();
                    let gate = std::sync::Arc::clone(&metadata_gate);
                    let cancellation = child_cancellation.clone();
                    task_in_flight.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    let guard = SchedulerTurnGuard(std::sync::Arc::clone(&task_in_flight));
                    metadata_turns.spawn(async move {
                        let _guard = guard;
                        let permit = tokio::select! {
                            _ = cancellation.cancelled() => return,
                            permit = gate.acquire_owned() => permit.ok(),
                        };
                        let Some(_permit) = permit else { return };
                        if let Err(error) = coordinator.run_root_discovery_turns().await {
                            tracing::warn!(%error, "provider root discovery turn failed");
                        }
                    });
                }
                for library in libraries.iter().cloned() {
                    let coordinator = coordinator.clone();
                    let gate = std::sync::Arc::clone(&metadata_gate);
                    let cancellation = child_cancellation.clone();
                    task_in_flight.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    let guard = SchedulerTurnGuard(std::sync::Arc::clone(&task_in_flight));
                    metadata_turns.spawn(async move {
                        let _guard = guard;
                        let permit = tokio::select! {
                            _ = cancellation.cancelled() => return,
                            permit = gate.acquire_owned() => permit.ok(),
                        };
                        let Some(_permit) = permit else { return };
                        let path = library.local_data_dir.join("catalog.sqlite");
                        let Some(lifecycle) = coordinator.begin_library_work(&library.library_uuid).ok().flatten() else { return };
                        match coordinator
                            .run_library_metadata_turn(&library.library_uuid, &path)
                            .await
                        {
                            Ok(result) if !result.capture_pending && lifecycle.check_current().is_ok() => {
                                let _ = coordinator
                                    .registry
                                    .record_sync_turn_success(&library.library_uuid, false);
                            }
                            Ok(_) => {}
                            Err(error) if lifecycle.check_current().is_ok() => {
                                let code = normalize_sync_error_code(&error);
                                let _ = coordinator
                                    .registry
                                    .record_sync_turn_error(&library.library_uuid, &code);
                                tracing::warn!(%error,library_uuid=%library.library_uuid,"dormant sync metadata turn failed");
                            }
                            Err(_) => {}
                        }
                        if lifecycle.check_current().is_ok() {
                            if let Err(error) = coordinator.refresh_pending_metrics(&library.library_uuid,&path) { tracing::warn!(%error,"refresh sync pending metrics failed"); }
                        }
                    });
                }
                while let Some(result) = metadata_turns.join_next().await {
                    if let Err(error) = result {
                        tracing::warn!(%error, "sync metadata task failed");
                    }
                }

                // Media has an independent budget and uses the same active-first
                // admission plus rotated dormant order. Metadata/apply can
                // therefore make progress under a saturated transfer queue.
                if let Some((library_uuid, path, writer)) = active.as_ref() {
                    task_in_flight.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    let _guard = SchedulerTurnGuard(std::sync::Arc::clone(&task_in_flight));
                    let lifecycle = coordinator.begin_library_work(library_uuid).ok().flatten();
                    if let Some(lifecycle) = lifecycle {
                        if let Err(error) = coordinator
                            .run_active_media_turn(library_uuid, path, writer)
                            .await
                        {
                            tracing::warn!(%error,%library_uuid,"active sync media turn failed");
                        }
                        if lifecycle.check_current().is_ok() {
                            if let Err(error) =
                                coordinator.refresh_pending_metrics(library_uuid, path)
                            {
                                tracing::warn!(%error,"refresh sync pending metrics failed");
                            }
                        }
                    }
                }
                let mut media_turns = tokio::task::JoinSet::new();
                for library in libraries {
                    let coordinator = coordinator.clone();
                    let cancellation = child_cancellation.clone();
                    task_in_flight.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    let guard = SchedulerTurnGuard(std::sync::Arc::clone(&task_in_flight));
                    media_turns.spawn(async move {
                        let _guard = guard;
                        if cancellation.is_cancelled() {
                            return;
                        }
                        let path = library.local_data_dir.join("catalog.sqlite");
                        let Some(lifecycle) = coordinator.begin_library_work(&library.library_uuid).ok().flatten() else { return };
                        if let Err(error) = coordinator
                            .run_dormant_media_turn(&library.library_uuid, &path)
                            .await
                        {
                            tracing::warn!(%error,library_uuid=%library.library_uuid,"dormant sync media turn failed");
                        }
                        if lifecycle.check_current().is_ok() {
                            if let Err(error) = coordinator.refresh_pending_metrics(&library.library_uuid,&path) { tracing::warn!(%error,"refresh sync pending metrics failed"); }
                        }
                    });
                }
                while let Some(result) = media_turns.join_next().await {
                    if let Err(error) = result {
                        tracing::warn!(%error, "sync media task failed");
                    }
                }
                let _ = coordinator.registry.set_sync_scheduler_state("running");
            }
            let _ = coordinator.registry.set_sync_scheduler_state("stopped");
        });
        SyncSchedulerHandle {
            cancellation,
            task,
            in_flight,
        }
    }

    async fn run_root_discovery_turns(&self) -> anyhow::Result<()> {
        let _metadata_permit = self
            .metadata_budget
            .acquire()
            .await
            .map_err(|_| anyhow::anyhow!("sync metadata budget closed"))?;
        for transport in self.registry.provider_root_transports()? {
            let Ok(work) = self.providers.begin_work(&transport.plugin_id) else {
                continue;
            };
            let Some(provider) = self
                .providers
                .resolve(&transport.plugin_id, &transport.account_uuid)?
            else {
                continue;
            };
            let mut page_token = transport.page_token.clone();
            let mut cursor = transport.cursor.clone();
            let mut completed_cursor = None;
            let mut pages = 0usize;
            let mut reset_attempted = false;
            loop {
                work.check_current()?;
                pages += 1;
                if pages > 1024 {
                    anyhow::bail!("provider root pagination exceeded the per-turn safety limit");
                }
                let requested_page = page_token.clone();
                let page = match provider
                    .delta_list(
                        super::provider::DeltaScope::Root,
                        cursor.as_deref(),
                        page_token.as_deref(),
                        100,
                    )
                    .await
                {
                    Ok(page) => page,
                    Err(error)
                        if error.code == super::provider::ProviderErrorCode::CursorReset
                            && !reset_attempted =>
                    {
                        reset_attempted = true;
                        cursor = None;
                        page_token = None;
                        self.registry.commit_provider_root_cursor(
                            &transport.account_uuid,
                            transport.account_generation,
                            &transport.runtime_session_id,
                            None,
                            &[],
                        )?;
                        continue;
                    }
                    Err(error) => return Err(anyhow::Error::new(error)),
                };
                let mut discovered = Vec::with_capacity(page.objects.len());
                for metadata in &page.objects {
                    let (library_uuid, key) = super::descriptor::split_root_key(&metadata.key)?;
                    let context = super::provider::TransportContext {
                        namespace: library_uuid.to_owned(),
                        binding_epoch: uuid::Uuid::nil().to_string(),
                        transport_generation: 1,
                    };
                    let relative = super::provider::ObjectMetadata {
                        key: key.to_owned(),
                        ..metadata.clone()
                    };
                    if let Some(candidate) = self.registry.verified_remote_descriptor(
                        &transport.account_uuid,
                        library_uuid,
                        &relative,
                    )? {
                        discovered.push(candidate);
                        continue;
                    }
                    let (candidate, _, _) = super::descriptor::download_candidate(
                        provider.as_ref(),
                        &context,
                        &relative,
                    )
                    .await?;
                    discovered.push(candidate);
                }
                work.check_current()?;
                page_token = page.page_token.clone();
                completed_cursor = page.completed_cursor.clone().or(completed_cursor);
                if let Some(next_page) = page_token.as_deref() {
                    if requested_page.as_deref() == Some(next_page) {
                        anyhow::bail!("provider root returned a non-advancing page token");
                    }
                    self.registry.commit_provider_root_page(
                        &transport.account_uuid,
                        transport.account_generation,
                        &transport.runtime_session_id,
                        cursor.as_deref(),
                        next_page,
                        &discovered,
                    )?;
                } else {
                    self.registry.commit_provider_root_cursor(
                        &transport.account_uuid,
                        transport.account_generation,
                        &transport.runtime_session_id,
                        completed_cursor.as_deref(),
                        &discovered,
                    )?;
                    break;
                }
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub async fn run_root_discovery_for_test(&self) -> anyhow::Result<()> {
        self.run_root_discovery_turns().await
    }

    /// Production provider-neutral turn. Provider adapters only place verified
    /// immutable segment bytes in this Library's spool; this coordinator owns
    /// registration, bounded apply, checkpoint recovery/build and writer lease.
    pub async fn run_library_turn(
        &self,
        library_uuid: &str,
        catalog_path: &Path,
    ) -> anyhow::Result<SyncTurnResult> {
        let result = self
            .run_library_metadata_turn(library_uuid, catalog_path)
            .await
            .context("run Library metadata turn")?;
        self.run_dormant_media_turn(library_uuid, catalog_path)
            .await
            .context("run dormant Library media turn")?;
        self.record_restore_progress(library_uuid, catalog_path)
            .await
            .context("record Library restore progress")?;
        Ok(result)
    }

    async fn run_library_metadata_turn(
        &self,
        library_uuid: &str,
        catalog_path: &Path,
    ) -> anyhow::Result<SyncTurnResult> {
        let Some(work) = self.begin_library_work(library_uuid)? else {
            return Ok(SyncTurnResult {
                capture_pending: true,
                ..Default::default()
            });
        };
        let _metadata_permit = self.metadata_budget.acquire().await?;
        if self.registry.sync_capture_boundary_pending(library_uuid)? {
            return Ok(SyncTurnResult {
                capture_pending: true,
                ..Default::default()
            });
        }
        let restoring_catalog = !catalog_path.exists();
        let published = self
            .provider_transport_turn(library_uuid, catalog_path)
            .await
            .context("run provider transport turn")?;
        if !catalog_path.exists() {
            self.bootstrap_missing_catalog(library_uuid, catalog_path)
                .context("bootstrap missing Catalog")?;
        }
        let runtime = DormantSyncRuntime::new(library_uuid, catalog_path, self.leases.clone());
        let root_bindings = self.catalog_root_binding_records(library_uuid)?;
        work.check_current()?;
        let (host_vector, host_metadata) = self.registry.host_checkpoint_snapshot(library_uuid)?;
        let spool_dir = catalog_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("sync-spool");
        work.check_current()?;
        self.apply_host_spool(&spool_dir)?;
        let published_for_catalog = published.clone();
        let _apply_permit = self.apply_budget.acquire().await?;
        let apply_work = work.clone();
        let result = runtime
            .run_turn(move |database| {
                apply_work.check_current()?;
                Self::project_root_binding_records(database, root_bindings)?;
                commit_published_segments(database.connection_for_fsops(), &published_for_catalog)?;
                perform_turn(database, host_vector, host_metadata, spool_dir)
            })
            .await
            .context("apply dormant Catalog turn")?;
        work.check_current()?;
        self.reconcile_remote_root_inventory(library_uuid, catalog_path)
            .context("reconcile remote synchronized-root inventory")?;
        self.finish_seed_if_ready(library_uuid, &result)
            .context("finish seed checkpoint")?;
        self.commit_host_published_frontier(library_uuid, &published)
            .context("commit Host published frontier")?;
        let reconciliation_writer =
            crate::db::DbWriter::spawn(crate::db::Database::open(catalog_path)?);
        let reconciliation = self
            .reconcile_root_work(library_uuid, &reconciliation_writer)
            .await;
        let reconciliation_shutdown = reconciliation_writer.shutdown().await;
        reconciliation.context("reconcile dormant root work")?;
        reconciliation_shutdown?;
        if restoring_catalog {
            self.registry
                .mark_restore_catalog_ready(library_uuid)
                .context("mark restored Catalog ready")?;
        }
        Ok(result)
    }

    fn bootstrap_missing_catalog(
        &self,
        library_uuid: &str,
        catalog_path: &Path,
    ) -> anyhow::Result<()> {
        let descriptor = self
            .registry
            .library(library_uuid)?
            .ok_or_else(|| anyhow::anyhow!("sync Library is missing from Host Registry"))?;
        let checkpoint_dir = catalog_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("sync-spool")
            .join("checkpoints");
        let mut candidates = Vec::new();
        if checkpoint_dir.exists() {
            for entry in std::fs::read_dir(&checkpoint_dir)? {
                let path = entry?.path();
                if path.extension().and_then(|v| v.to_str()) == Some("checkpoint") {
                    let checkpoint: super::checkpoint::Checkpoint =
                        serde_json::from_slice(&std::fs::read(&path)?)?;
                    checkpoint.verify()?;
                    candidates.push(checkpoint);
                }
            }
        }
        let portable_candidates = candidates
            .into_iter()
            .filter(|checkpoint| checkpoint.recovery.portable_complete)
            .collect::<Vec<_>>();
        let chosen = self.registry.chosen_descriptor(library_uuid)?;
        let checkpoint = if let Some((_, Some(key), Some(hash))) = chosen {
            let checkpoint = portable_candidates
                .iter()
                .find(|checkpoint| {
                    format!("checkpoints/{}.checkpoint", checkpoint.checkpoint_uuid) == key
                })
                .context("synchronized Library is awaiting its chosen descriptor checkpoint")?;
            if hex_hash(&super::codec::sha256(&super::codec::canonical_bytes(
                checkpoint,
            )?)) != hash
            {
                anyhow::bail!("chosen descriptor checkpoint hash mismatch");
            }
            checkpoint
        } else if chosen.is_some() {
            anyhow::bail!("synchronized Library is awaiting its pinned descriptor validation");
        } else {
            let maximal = super::checkpoint::maximal_checkpoints(&portable_candidates);
            if maximal.len() != 1 {
                anyhow::bail!("synchronized Library has no unambiguous portable checkpoint");
            }
            maximal[0]
        };
        let staging = catalog_path.with_extension("sqlite.sync-staging");
        let host_bindings = self.registry.local_root_bindings(library_uuid)?;
        if self.registry.is_root_aware_restore(library_uuid)?
            || checkpoint.projection.roots.len() > 1
            || !host_bindings.is_empty()
        {
            let selected_roots = host_bindings
                .iter()
                .filter(|binding| binding.state != "local-only-fork")
                .map(|binding| {
                    Ok((
                        uuid::Uuid::parse_str(&binding.root_uuid)?,
                        PathBuf::from(&binding.local_path),
                    ))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            if selected_roots.is_empty()
                && checkpoint.projection.roots.len() > 1
                && !self.registry.is_root_aware_restore(library_uuid)?
            {
                anyhow::bail!(
                    "multi-root checkpoint restore is awaiting selected local root bindings"
                );
            }
            let library_root_uuid = checkpoint
                .state
                .roots
                .values()
                .map(|root| root.library_root_uuid.0)
                .next()
                .unwrap_or(uuid::Uuid::parse_str(&descriptor.root_uuid)?);
            if checkpoint
                .state
                .roots
                .values()
                .any(|root| root.library_root_uuid.0 != library_root_uuid)
            {
                anyhow::bail!("checkpoint contains inconsistent Library-root identities");
            }
            super::persistence::materialize_staging_catalog_with_roots(
                checkpoint,
                &staging,
                library_root_uuid,
                self.registry.device_uuid()?,
                &selected_roots,
            )
            .context("materialize multi-root staging Catalog")?;
        } else {
            let content = self.registry.restore_content_paths(library_uuid)?;
            let content = content.first().context("restore-content-target-missing")?;
            super::persistence::materialize_staging_catalog(
                checkpoint,
                &staging,
                uuid::Uuid::parse_str(&descriptor.root_uuid)?,
                self.registry.device_uuid()?,
                Path::new(content),
            )
            .context("materialize staging Catalog")?;
        }
        self.registry
            .advance_restore_phase(library_uuid, "catalog-staged")
            .context("record staged restore Catalog")?;
        let transport = self
            .registry
            .binding_transport(library_uuid, "library")?
            .ok_or_else(|| anyhow::anyhow!("sync binding is missing during restore bootstrap"))?;
        let restored =
            crate::db::Database::open(&staging).context("open restored staging Catalog")?;
        restored
            .begin_restored_sync_capture(transport.binding_epoch, transport.actor_epoch)
            .context("begin restored sync capture")?;
        drop(restored);
        self.registry
            .restore_host_checkpoint_metadata(&checkpoint.host_metadata)
            .context("restore Host checkpoint metadata")?;
        super::persistence::promote_new_staging_catalog(&staging, catalog_path)
            .context("promote restored staging Catalog")?;
        self.registry
            .mark_restore_catalog_ready(library_uuid)
            .context("record restored Catalog cutover")
    }

    pub async fn run_active_turn(
        &self,
        library_uuid: &str,
        catalog_path: &Path,
        writer: &crate::db::DbWriter,
    ) -> anyhow::Result<SyncTurnResult> {
        let result = self
            .run_active_metadata_turn(library_uuid, catalog_path, writer)
            .await?;
        self.run_active_media_turn(library_uuid, catalog_path, writer)
            .await?;
        self.record_restore_progress(library_uuid, catalog_path)
            .await?;
        Ok(result)
    }

    async fn run_active_metadata_turn(
        &self,
        library_uuid: &str,
        catalog_path: &Path,
        writer: &crate::db::DbWriter,
    ) -> anyhow::Result<SyncTurnResult> {
        let Some(work) = self.begin_library_work(library_uuid)? else {
            return Ok(SyncTurnResult {
                capture_pending: true,
                ..Default::default()
            });
        };
        let _metadata_permit = self.metadata_budget.acquire().await?;
        // Enable/retry commits Host intent before its asynchronous Catalog capture.
        // This is a waiting boundary, not a failed capture or permission to seed.
        if self.registry.sync_capture_boundary_pending(library_uuid)? {
            return Ok(SyncTurnResult {
                capture_pending: true,
                ..Default::default()
            });
        }
        let published = self
            .provider_transport_turn(library_uuid, catalog_path)
            .await?;
        work.check_current()?;
        let (host_vector, host_metadata) = self.registry.host_checkpoint_snapshot(library_uuid)?;
        let root_bindings = self.catalog_root_binding_records(library_uuid)?;
        let spool_dir = catalog_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("sync-spool");
        work.check_current()?;
        self.apply_host_spool(&spool_dir)?;
        let published_for_catalog = published.clone();
        let _apply_permit = self.apply_budget.acquire().await?;
        let apply_work = work.clone();
        let result = writer
            .write_authoritative_without_wake(move |database| {
                apply_work.check_current()?;
                Self::project_root_binding_records(database, root_bindings)?;
                commit_published_segments(database.connection_for_fsops(), &published_for_catalog)?;
                perform_turn(database, host_vector, host_metadata, spool_dir)
            })
            .await?;
        work.check_current()?;
        self.reconcile_remote_root_inventory(library_uuid, catalog_path)?;
        self.finish_seed_if_ready(library_uuid, &result)?;
        self.commit_host_published_frontier(library_uuid, &published)?;
        self.reconcile_root_work(library_uuid, writer).await?;
        Ok(result)
    }

    async fn reconcile_root_work(
        &self,
        library_uuid: &str,
        writer: &crate::db::DbWriter,
    ) -> anyhow::Result<()> {
        let Some(lifecycle) = self.begin_library_work(library_uuid)? else {
            return Ok(());
        };
        let admitted = lifecycle.clone();
        let writer =
            writer.with_write_admission(std::sync::Arc::new(move || admitted.check_current()));
        let roots = self.registry.pending_root_reconciliations()?;
        for (_, root_uuid) in roots
            .into_iter()
            .filter(|(library, _)| library == library_uuid)
        {
            let work = self
                .registry
                .pending_root_reconciliation_work(library_uuid, &root_uuid)?;
            for item in &work {
                let plan_uuid = uuid::Uuid::parse_str(&item.plan_uuid)?;
                let root_uuid_value = uuid::Uuid::parse_str(&root_uuid)?;
                let binding_uuid = uuid::Uuid::parse_str(&item.local_binding_uuid)?;
                let entity_uuid = uuid::Uuid::parse_str(&item.entity_uuid)?;
                let ordinal = item.item_ordinal;
                let generation = item.binding_generation;
                let field = item.field_name.clone();
                let resolution = item.resolution.clone();
                let selected = item.selected_value.clone();
                let selected_entity = item.selected_entity.clone();
                let applied = writer
                    .write_authoritative_without_wake(move |database| {
                        database.apply_root_reconciliation_item(
                            plan_uuid,
                            ordinal,
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
                lifecycle.check_current()?;
                match applied {
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
            if self
                .registry
                .pending_root_reconciliation_work(library_uuid, &root_uuid)?
                .is_empty()
            {
                let binding = self
                    .registry
                    .local_root_binding(library_uuid, &root_uuid)?
                    .context("local-root-binding-not-found")?;
                let plan_uuid = self
                    .registry
                    .pending_reenable_plan_uuid(library_uuid, &root_uuid)?
                    .context("comparison-plan-not-found")?;
                let root_uuid_value = uuid::Uuid::parse_str(&root_uuid)?;
                let binding_uuid = uuid::Uuid::parse_str(&binding.local_binding_uuid)?;
                let generation = binding.binding_generation;
                let revision = binding.operation_revision.saturating_add(1);
                writer
                    .write_authoritative_without_wake(move |database| {
                        database.complete_root_reconciliation_projection(
                            root_uuid_value,
                            binding_uuid,
                            generation,
                            revision,
                        )
                    })
                    .await?;
                self.registry
                    .complete_root_reconciliation(library_uuid, &root_uuid, &plan_uuid)?;
            }
        }
        Ok(())
    }

    fn reconcile_remote_root_inventory(
        &self,
        library_uuid: &str,
        catalog_path: &Path,
    ) -> anyhow::Result<()> {
        let database = crate::db::Database::open(catalog_path)?;
        for descriptor in database.sync_root_descriptors()? {
            let root_uuid = uuid::Uuid::from_bytes(descriptor.root_uuid).to_string();
            let actionable = self.registry.upsert_remote_sync_root(
                library_uuid,
                &root_uuid,
                &uuid::Uuid::from_bytes(descriptor.root_folder_uuid).to_string(),
                &descriptor.requested_name,
                &descriptor.materialized_name,
                &super::media::hex_hash(&descriptor.materialization_fingerprint),
                descriptor.logical_revision,
                &descriptor.lifecycle,
                None,
                None,
            )?;
            if actionable {
                let inventory_revision = self
                    .registry
                    .remote_sync_root(library_uuid, &root_uuid)?
                    .map(|root| root.inventory_revision)
                    .unwrap_or(1);
                let now = crate::host_registry::unix_timestamp_millis_for_runtime();
                let mut notification =
                    crate::db::notification_outbox::managed_root_placement_notification(
                        library_uuid,
                        &root_uuid,
                        inventory_revision,
                        &descriptor.requested_name,
                        now,
                    );
                database.publish_notification_operation(
                    &mut notification,
                    crate::db::notification_outbox::PublishMode::Phase,
                )?;
            }
        }
        Ok(())
    }

    async fn run_dormant_media_turn(
        &self,
        library_uuid: &str,
        catalog_path: &Path,
    ) -> anyhow::Result<()> {
        let Some(_work) = self.begin_library_work(library_uuid)? else {
            return Ok(());
        };
        let _media_permit = self.media_budget.acquire().await?;
        let database = crate::db::Database::open(catalog_path)?;
        database.apply_migrations()?;
        self.project_root_binding_tokens(library_uuid, &database)?;
        let writer = crate::db::DbWriter::spawn(database);
        let result = self
            .run_media_turn(library_uuid, catalog_path, &writer)
            .await;
        let shutdown = writer.shutdown().await;
        result?;
        shutdown
    }

    pub(crate) fn project_root_binding_tokens(
        &self,
        library_uuid: &str,
        database: &crate::db::Database,
    ) -> anyhow::Result<()> {
        Self::project_root_binding_records(
            database,
            self.catalog_root_binding_records(library_uuid)?,
        )
    }

    fn catalog_root_binding_records(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<Vec<crate::db::SyncRootLocalBindingRecord>> {
        let library_uuid_bytes = *uuid::Uuid::parse_str(library_uuid)?.as_bytes();
        let mut records = Vec::new();
        for binding in self.registry.local_root_bindings(library_uuid)? {
            let root_uuid = uuid::Uuid::parse_str(&binding.root_uuid)?;
            records.push(crate::db::SyncRootLocalBindingRecord {
                root_uuid: *root_uuid.as_bytes(),
                library_uuid: library_uuid_bytes,
                local_binding_uuid: *uuid::Uuid::parse_str(&binding.local_binding_uuid)?.as_bytes(),
                binding_generation: binding.binding_generation,
                root_capture_epoch: *uuid::Uuid::parse_str(
                    binding
                        .root_capture_epoch
                        .as_deref()
                        .unwrap_or(&binding.local_binding_uuid),
                )?
                .as_bytes(),
                root_actor_epoch: *uuid::Uuid::parse_str(
                    binding
                        .root_actor_epoch
                        .as_deref()
                        .unwrap_or(&binding.local_binding_uuid),
                )?
                .as_bytes(),
                state: binding.state,
                root_id: 0,
                operation_revision: binding.operation_revision,
                updated_at: unix_timestamp_millis(),
            });
        }
        Ok(records)
    }

    fn project_root_binding_records(
        database: &crate::db::Database,
        records: Vec<crate::db::SyncRootLocalBindingRecord>,
    ) -> anyhow::Result<()> {
        for mut record in records {
            let Some(root_id) = database.sync_root_id(&record.root_uuid)? else {
                continue;
            };
            record.root_id = root_id;
            let current = database.sync_root_local_binding(&record.root_uuid)?;
            if record.state == "local-only-fork"
                && current.as_ref().is_some_and(|current| {
                    current.local_binding_uuid == record.local_binding_uuid
                        && current.binding_generation < record.binding_generation
                })
            {
                let current = current.expect("checked above");
                database.fence_root_as_local_only(
                    uuid::Uuid::from_bytes(record.root_uuid),
                    uuid::Uuid::from_bytes(current.local_binding_uuid),
                    current.binding_generation,
                    record.binding_generation,
                    uuid::Uuid::from_bytes(record.root_capture_epoch),
                    uuid::Uuid::from_bytes(record.root_actor_epoch),
                    record.operation_revision,
                )?;
            } else {
                database.upsert_sync_root_local_binding(&record)?;
            }
        }
        Ok(())
    }

    fn refresh_pending_metrics(
        &self,
        library_uuid: &str,
        catalog_path: &Path,
    ) -> anyhow::Result<()> {
        if !catalog_path.exists() {
            return Ok(());
        }
        let connection = rusqlite::Connection::open_with_flags(
            catalog_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let (metadata,metadata_bytes): (u64,u64) = connection.query_row("SELECT COUNT(*),COALESCE(SUM(length(e.payload)),0) FROM sync_outbox o JOIN sync_events e ON e.event_uuid=o.event_uuid WHERE o.state IN ('pending','sealed')",[],|row|Ok((row.get(0)?,row.get(1)?)))?;
        let (objects,object_bytes): (u64,u64) = connection.query_row("SELECT COUNT(*),COALESCE(SUM(o.byte_length),0) FROM media_publication_objects o JOIN media_publication_jobs j ON j.intent_uuid=o.intent_uuid WHERE o.state IN ('sealed','uploading','failed') AND j.phase NOT IN ('event-published','cancelled','archived')",[],|row|Ok((row.get(0)?,row.get(1)?)))?;
        let (capture,capture_bytes): (u64,u64) = connection.query_row("SELECT COUNT(*),COALESCE(SUM(MAX(0,COALESCE(r.byte_size,0))),0) FROM sync_publication_intents i JOIN asset_content_revisions r ON r.content_revision_uuid=i.content_revision_uuid WHERE i.state IN ('pending-hash','pending-manifest','ready','failed') AND NOT EXISTS(SELECT 1 FROM media_publication_objects o WHERE o.intent_uuid=i.intent_uuid)",[],|row|Ok((row.get(0)?,row.get(1)?)))?;
        let (download,download_bytes): (u64,u64) = connection.query_row("SELECT COUNT(*),COALESCE(SUM(MAX(0,expected_length-downloaded_bytes)),0) FROM media_materialization_jobs WHERE phase<>'complete'",[],|row|Ok((row.get(0)?,row.get(1)?)))?;
        let conflicts: u64 = connection.query_row(
            "SELECT COUNT(*) FROM sync_conflicts WHERE state='open'",
            [],
            |row| row.get(0),
        )?;
        self.registry.update_sync_pending_metrics(
            library_uuid,
            metadata + objects + capture,
            metadata_bytes + object_bytes + capture_bytes,
            download,
            download_bytes,
            conflicts,
        )
    }

    async fn record_restore_progress(
        &self,
        library_uuid: &str,
        catalog_path: &Path,
    ) -> anyhow::Result<()> {
        let Some(lifecycle) = self.begin_library_work(library_uuid)? else {
            return Ok(());
        };
        lifecycle.check_current()?;
        self.refresh_pending_metrics(library_uuid, catalog_path)?;
        let database = crate::db::Database::open(catalog_path)?;
        let connection = database.connection_for_fsops();
        let (total, done, bytes_total, bytes_done): (i64, i64, i64, i64) = connection
            .query_row(
                "SELECT COUNT(*),COALESCE(SUM(CASE WHEN phase='complete' THEN 1 ELSE 0 END),0),COALESCE(SUM(expected_length),0),COALESCE(SUM(downloaded_bytes),0) FROM media_materialization_jobs",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        let conflicts: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sync_conflicts WHERE state='open'",
            [],
            |row| row.get(0),
        )?;
        self.registry.update_restore_progress(
            library_uuid,
            done.max(0) as u64,
            total.max(0) as u64,
            bytes_done.max(0) as u64,
            bytes_total.max(0) as u64,
            conflicts.max(0) as u64,
        )?;
        if let Some(plan) = self.registry.restore_plan_for_library(library_uuid)? {
            let terminal = matches!(plan.state.as_str(), "complete" | "failed" | "cancelled");
            let status = match plan.state.as_str() {
                "complete" => "succeeded",
                "failed" => "failed",
                "cancelled" => "cancelled",
                "paused" => "pending",
                _ => "running",
            };
            let mut notification = crate::db::notification_outbox::NotificationOperation {
                operation_id: format!("sync:{}:{}", plan.operation_uuid, library_uuid),
                revision: 0,
                kind: "restore".to_owned(),
                category: "sync".to_owned(),
                origin: "background".to_owned(),
                presentation: "important".to_owned(),
                status: status.to_owned(),
                phase: Some(plan.phase.clone()),
                title: format!("{} · 资源库恢复", plan.name),
                summary: plan.error_code.clone().or_else(|| Some(plan.phase.clone())),
                task_id: None,
                root_id: None,
                library_uuid: Some(library_uuid.to_owned()),
                root_uuid: None,
                managed_root_action: None,
                folder_id: None,
                file_id: None,
                current_value: i64::try_from(plan.media_items_done).unwrap_or(i64::MAX),
                total_value: plan
                    .media_items_total
                    .map(|value| i64::try_from(value).unwrap_or(i64::MAX)),
                unit: Some("items".to_owned()),
                error_code: plan.error_code,
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
            let database = crate::db::Database::open(catalog_path)?;
            database.apply_migrations()?;
            let admitted = lifecycle.clone();
            let writer = crate::db::DbWriter::spawn(database).with_write_admission(
                std::sync::Arc::new(move || admitted.check_current().map_err(anyhow::Error::from)),
            );
            let publish = writer
                .write_authoritative(move |database| {
                    database
                        .publish_notification_operation(&mut notification, mode)
                        .map(|_| ())
                })
                .await;
            let shutdown = writer.shutdown().await;
            publish?;
            shutdown?;
        }
        Ok(())
    }

    async fn run_active_media_turn(
        &self,
        library_uuid: &str,
        catalog_path: &Path,
        writer: &crate::db::DbWriter,
    ) -> anyhow::Result<()> {
        let Some(_work) = self.begin_library_work(library_uuid)? else {
            return Ok(());
        };
        let _media_permit = self.media_budget.acquire().await?;
        let scheduler_writer = writer.without_authoritative_commit_wake();
        self.run_media_turn(library_uuid, catalog_path, &scheduler_writer)
            .await
    }

    async fn run_media_turn(
        &self,
        library_uuid: &str,
        catalog_path: &Path,
        writer: &crate::db::DbWriter,
    ) -> anyhow::Result<()> {
        if self.registry.sync_capture_boundary_pending(library_uuid)? {
            return Ok(());
        }
        let Some(binding) = self.registry.binding_transport(library_uuid, "library")? else {
            return Ok(());
        };
        if !matches!(
            binding.state.as_str(),
            "preparing" | "capturing" | "seed_ready" | "publishing" | "active" | "restoring"
        ) {
            return Ok(());
        }
        let Some(provider) = self
            .providers
            .resolve(&binding.plugin_id, &binding.account_uuid)?
        else {
            return Ok(());
        };
        let work = self.providers.begin_work(&binding.plugin_id)?;
        let writer = writer.with_write_admission(std::sync::Arc::new(move || work.check_current()));
        let library_data_root = catalog_path.parent().unwrap_or_else(|| Path::new("."));
        let object_root = library_data_root.join("media-objects");
        let workers = {
            let mut active = self.media_workers.lock().await;
            active
                .entry(binding.binding_epoch)
                .or_insert_with(|| {
                    super::media::worker::MediaWorkerSet::new(
                        super::media::worker::MediaWorkLimiter::new_with_temp_budget(
                            1,
                            128,
                            128 * 1024 * 1024,
                            16 * 1024 * 1024 * 1024,
                        )
                        .expect("production media limits are nonzero"),
                    )
                })
                .clone()
        };
        let limiter = workers.limiter();
        let context = super::provider::TransportContext {
            namespace: binding
                .remote_namespace
                .clone()
                .unwrap_or_else(|| library_uuid.to_owned()),
            binding_epoch: binding.binding_epoch.to_string(),
            transport_generation: binding.transport_generation as u64,
        };
        let worker = super::media::worker::MediaPublicationWorker::new(
            writer.clone(),
            provider.clone(),
            object_root.clone(),
            context.clone(),
            limiter.clone(),
        );
        let restore = super::media::worker::MediaRestoreWorker::new(
            writer.clone(),
            provider,
            library_data_root.join("media-restore-cache"),
            context,
            limiter,
        );
        workers
            .run(async move {
                worker.run_once().await?;
                restore.run_once().await?;
                Ok(())
            })
            .await
    }

    /// Performs provider I/O before any Catalog writer or runtime lease is
    /// acquired. Durable intake and cursor commits remain Host/Core-owned and
    /// are fenced by binding epoch plus transport generation.
    async fn provider_transport_turn(
        &self,
        library_uuid: &str,
        catalog_path: &Path,
    ) -> anyhow::Result<Vec<PublishedSegment>> {
        let Some(binding) = self.registry.binding_transport(library_uuid, "library")? else {
            return Ok(Vec::new());
        };
        if !matches!(
            binding.state.as_str(),
            "preparing" | "capturing" | "seed_ready" | "publishing" | "active" | "restoring"
        ) {
            return Ok(Vec::new());
        }
        let Some(provider) = self
            .providers
            .resolve(&binding.plugin_id, &binding.account_uuid)?
        else {
            return Ok(Vec::new());
        };
        let namespace = binding
            .remote_namespace
            .clone()
            .unwrap_or_else(|| library_uuid.to_owned());
        let context = super::provider::TransportContext {
            namespace: namespace.clone(),
            binding_epoch: binding.binding_epoch.to_string(),
            transport_generation: binding.transport_generation as u64,
        };
        let mut page_token = binding.page_token.clone();
        let mut cursor = binding.cursor.clone();
        let mut completed_cursor = None;
        let spool = catalog_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("sync-spool");
        if binding.state == "restoring" && !catalog_path.exists() {
            let (key, hash) = self
                .registry
                .chosen_descriptor_identity(library_uuid)?
                .context("restore descriptor identity missing")?;
            let object = provider
                .object_get(&context, &key, super::descriptor::MAX_DESCRIPTOR_BYTES)
                .await
                .map_err(anyhow::Error::new)?;
            super::provider::validate_download(&object, super::descriptor::MAX_DESCRIPTOR_BYTES)
                .map_err(anyhow::Error::new)?;
            if object.metadata.key != key || object.metadata.sha256.as_slice() != hash {
                anyhow::bail!("pinned restore descriptor identity mismatch");
            }
            let (candidate, checkpoint, bytes) = super::descriptor::download_candidate(
                provider.as_ref(),
                &context,
                &object.metadata,
            )
            .await?;
            self.registry
                .complete_restore_descriptor_pin(library_uuid, &binding, &candidate)?;
            std::fs::create_dir_all(spool.join("descriptors"))?;
            std::fs::create_dir_all(spool.join("checkpoints"))?;
            std::fs::create_dir_all(spool.join("indexes"))?;
            atomic_write(
                &spool
                    .join("descriptors")
                    .join(format!("{}.json", hex_hash(&object.metadata.sha256))),
                &bytes,
            )?;
            atomic_write(
                &spool
                    .join("checkpoints")
                    .join(format!("{}.checkpoint", checkpoint.checkpoint_uuid)),
                &super::codec::canonical_bytes(&checkpoint)?,
            )?;
            for shard in &checkpoint.media_index_shards {
                let object = provider
                    .object_get(
                        &context,
                        &shard.object_key,
                        super::media::MAX_INDEX_SHARD_BYTES as u64,
                    )
                    .await
                    .map_err(anyhow::Error::new)?;
                super::provider::validate_download(
                    &object,
                    super::media::MAX_INDEX_SHARD_BYTES as u64,
                )
                .map_err(anyhow::Error::new)?;
                if object.metadata.key != shard.object_key
                    || object.metadata.sha256 != super::media::parse_hash(&shard.sha256)?
                    || object.metadata.size != shard.byte_length
                {
                    anyhow::bail!("restore index identity mismatch");
                }
                atomic_write(
                    &spool.join("indexes").join(format!("{}.idx", shard.sha256)),
                    &std::fs::read(&object.path)?,
                )?;
            }
        }
        let mut pages = 0usize;
        let mut reset_attempted = false;
        loop {
            pages += 1;
            if pages > 1024 {
                anyhow::bail!("provider pagination exceeded the per-turn safety limit");
            }
            let requested_page = page_token.clone();
            let page = match provider
                .delta_list(
                    super::provider::DeltaScope::Library {
                        namespace: namespace.clone(),
                        binding_epoch: binding.binding_epoch.to_string(),
                        transport_generation: binding.transport_generation as u64,
                    },
                    cursor.as_deref(),
                    page_token.as_deref(),
                    100,
                )
                .await
            {
                Ok(page) => page,
                Err(error)
                    if error.code == super::provider::ProviderErrorCode::CursorReset
                        && !reset_attempted =>
                {
                    reset_attempted = true;
                    cursor = None;
                    page_token = None;
                    self.registry.commit_binding_cursor(
                        library_uuid,
                        binding.binding_epoch,
                        binding.transport_generation,
                        &namespace,
                        "library",
                        None,
                    )?;
                    continue;
                }
                Err(error) => return Err(anyhow::Error::new(error)),
            };
            for metadata in &page.objects {
                if super::descriptor::is_descriptor_key(&metadata.key) {
                    let verified = self.registry.verified_remote_descriptor(
                        &binding.account_uuid,
                        library_uuid,
                        metadata,
                    )?;
                    let (candidate, checkpoint, bytes) =
                        super::descriptor::download_candidate_reusing(
                            provider.as_ref(),
                            &context,
                            metadata,
                            verified.as_ref(),
                        )
                        .await?;
                    self.registry
                        .cache_bound_descriptor(library_uuid, &binding, &candidate)?;
                    let directory = spool.join("descriptors");
                    std::fs::create_dir_all(&directory)?;
                    atomic_write(
                        &directory.join(format!("{}.json", hex_hash(&metadata.sha256))),
                        &bytes,
                    )?;
                    let directory = spool.join("checkpoints");
                    std::fs::create_dir_all(&directory)?;
                    atomic_write(
                        &directory.join(format!("{}.checkpoint", checkpoint.checkpoint_uuid)),
                        &super::codec::canonical_bytes(&checkpoint)?,
                    )?;
                    self.registry.record_provider_receipt(
                        library_uuid,
                        binding.binding_epoch,
                        binding.transport_generation,
                        &metadata.key,
                        metadata.etag.as_deref(),
                        &metadata.sha256,
                        "durable",
                    )?;
                    continue;
                }
                if !metadata.key.starts_with("segments/")
                    && !metadata.key.starts_with("host/")
                    && !metadata.key.starts_with("checkpoints/")
                    && !metadata.key.starts_with("indexes/")
                    && metadata.key != "library-descriptor"
                {
                    continue;
                }
                let object_limit = if metadata.key.starts_with("checkpoints/") {
                    64 * 1024 * 1024
                } else if metadata.key.starts_with("indexes/") {
                    super::media::MAX_INDEX_SHARD_BYTES as u64
                } else {
                    super::inbox::MAX_APPLY_DECODED_BYTES as u64
                };
                let object = provider
                    .object_get(&context, &metadata.key, object_limit)
                    .await
                    .map_err(anyhow::Error::new)?;
                super::provider::validate_download(&object, object_limit)
                    .map_err(anyhow::Error::new)?;
                if object.metadata.key != metadata.key
                    || object.metadata.size != metadata.size
                    || object.metadata.sha256 != metadata.sha256
                {
                    anyhow::bail!(
                        "provider object metadata does not match the delta listing for {}",
                        metadata.key
                    );
                }
                let object_bytes = std::fs::read(&object.path)?;
                if metadata.key.starts_with("segments/") {
                    // Delta object order is Provider-defined and may place a successor before
                    // its predecessor. Verify the predecessor committed by the segment itself;
                    // durable inbox registration and actor-head application enforce chain order.
                    let segment: super::segment::SealedSegment =
                        serde_json::from_slice(&object_bytes)?;
                    super::spool::intake_segment(
                        &spool,
                        &object_bytes,
                        segment.header.predecessor_hash,
                        |_| Ok(()),
                    )
                    .with_context(|| format!("intake Provider segment object {}", metadata.key))?;
                } else if metadata.key.starts_with("host/") {
                    let directory = spool.join("host");
                    std::fs::create_dir_all(&directory)?;
                    let destination =
                        directory.join(format!("{}.json", hex_hash(&metadata.sha256)));
                    atomic_write(&destination, &object_bytes)?;
                } else if metadata.key.starts_with("checkpoints/") {
                    let checkpoint: super::checkpoint::Checkpoint =
                        serde_json::from_slice(&object_bytes)?;
                    checkpoint.verify()?;
                    let directory = spool.join("checkpoints");
                    std::fs::create_dir_all(&directory)?;
                    let destination = directory.join(
                        Path::new(&metadata.key)
                            .file_name()
                            .ok_or_else(|| anyhow::anyhow!("invalid checkpoint object key"))?,
                    );
                    atomic_write(&destination, &object_bytes)?;
                } else if metadata.key.starts_with("indexes/") {
                    super::media::format::IndexShardV1::decode(&object_bytes)?;
                    let directory = spool.join("indexes");
                    std::fs::create_dir_all(&directory)?;
                    let destination = directory.join(format!("{}.idx", hex_hash(&metadata.sha256)));
                    atomic_write(&destination, &object_bytes)?;
                }
                self.registry.record_provider_receipt(
                    library_uuid,
                    binding.binding_epoch,
                    binding.transport_generation,
                    &metadata.key,
                    metadata.etag.as_deref(),
                    &metadata.sha256,
                    "durable",
                )?;
            }
            page_token = page.page_token.clone();
            completed_cursor = page.completed_cursor.clone().or(completed_cursor);
            if let Some(next_page) = page_token.as_deref() {
                if requested_page.as_deref() == Some(next_page) {
                    anyhow::bail!("provider returned a non-advancing page token");
                }
                self.registry.commit_binding_page(
                    library_uuid,
                    binding.binding_epoch,
                    binding.transport_generation,
                    &namespace,
                    "library",
                    cursor.as_deref(),
                    next_page,
                )?;
            } else {
                self.registry.commit_binding_cursor(
                    library_uuid,
                    binding.binding_epoch,
                    binding.transport_generation,
                    &namespace,
                    "library",
                    completed_cursor.as_deref(),
                )?;
                break;
            }
        }
        if binding.state == "restoring" {
            return Ok(Vec::new());
        }
        self.push_protocol_objects(
            provider.as_ref(),
            library_uuid,
            &binding,
            catalog_path,
            &spool,
        )
        .await
    }

    async fn push_protocol_objects(
        &self,
        provider: &dyn super::provider::SyncProvider,
        library_uuid: &str,
        binding: &crate::host_registry::BindingTransport,
        catalog_path: &Path,
        spool: &Path,
    ) -> anyhow::Result<Vec<PublishedSegment>> {
        let context = super::provider::TransportContext {
            namespace: binding
                .remote_namespace
                .clone()
                .unwrap_or_else(|| library_uuid.to_owned()),
            binding_epoch: binding.binding_epoch.to_string(),
            transport_generation: binding.transport_generation as u64,
        };
        let outgoing = spool.join("outgoing");
        let mut published = Vec::new();
        if outgoing.exists() {
            for entry in std::fs::read_dir(&outgoing)? {
                let path = entry?.path();
                if path.extension().and_then(|v| v.to_str()) != Some("json") {
                    continue;
                }
                let bytes = std::fs::read(&path)?;
                let segment = match serde_json::from_slice::<super::segment::SealedSegment>(&bytes)
                {
                    Ok(segment) if segment.header.library_uuid.to_string() == library_uuid => {
                        segment
                    }
                    _ => continue,
                };
                super::segment::verify_segment(&segment, segment.header.predecessor_hash)
                    .with_context(|| format!("verify outgoing sync segment {}", path.display()))?;
                let hash: [u8; 32] = sha2::Sha256::digest(&bytes).into();
                let key = format!("segments/{}.json", hex_hash(&hash));
                self.publish_protocol_object(
                    provider,
                    &context,
                    library_uuid,
                    binding,
                    &key,
                    &bytes,
                )
                .await?;
                let mut segment_uuid = [0u8; 16];
                segment_uuid.copy_from_slice(&segment.segment_hash[..16]);
                published.push(PublishedSegment {
                    segment_uuid,
                    actor_epoch: segment.header.actor.actor_epoch.0,
                    last_seq: segment.header.last_seq,
                    object_key: key,
                });
            }
        }
        let seed_publication = matches!(binding.state.as_str(), "seed_ready" | "publishing");
        if seed_publication || binding.state == "active" {
            let connection = rusqlite::Connection::open_with_flags(
                catalog_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let checkpoint_uuid = if seed_publication {
                binding.latest_checkpoint_uuid
            } else {
                latest_portable_checkpoint(&connection)?.map(|value| value.0)
            };
            let Some(checkpoint_uuid) = checkpoint_uuid else {
                return Ok(published);
            };
            let (checkpoint_path, expected_hash, created_at): (String,Vec<u8>,i64) = connection.query_row(
                "SELECT spool_path,payload_hash,created_at FROM sync_checkpoints WHERE checkpoint_uuid=?1 AND state='ready'",
                [checkpoint_uuid.as_bytes().as_slice()], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
            drop(connection);
            let checkpoint = std::fs::read(&checkpoint_path)?;
            let checkpoint_hash = super::codec::sha256(&checkpoint);
            if expected_hash.as_slice() != checkpoint_hash {
                anyhow::bail!("checkpoint payload hash mismatch");
            }
            let payload: super::checkpoint::Checkpoint = serde_json::from_slice(&checkpoint)?;
            payload.verify()?;
            if !payload.recovery.portable_complete {
                return Ok(published);
            }
            let checkpoint_key = format!("checkpoints/{checkpoint_uuid}.checkpoint");
            if seed_publication {
                if let Some(metadata) = provider
                    .object_head(&context, super::descriptor::LEGACY_KEY)
                    .await
                    .map_err(anyhow::Error::new)?
                {
                    // Earlier clients could downgrade their own publication receipts
                    // to download-only receipts. Recover authority from the verified
                    // remote seed's exact bytes, never from a newly serialized seed.
                    let (_, remote_checkpoint, remote_bytes) =
                        super::descriptor::download_candidate(provider, &context, &metadata)
                            .await?;
                    let remote: super::descriptor::LibraryDescriptor =
                        serde_json::from_slice(&remote_bytes)?;
                    if remote.binding_epoch != binding.binding_epoch
                        || remote_checkpoint.checkpoint_uuid.0 != checkpoint_uuid
                        || super::media::parse_hash(&remote.seed.sha256)? != checkpoint_hash
                    {
                        anyhow::bail!("existing seed does not match the binding capture");
                    }
                    self.publish_protocol_object(
                        provider,
                        &context,
                        library_uuid,
                        binding,
                        &checkpoint_key,
                        &checkpoint,
                    )
                    .await?;
                    self.publish_protocol_object(
                        provider,
                        &context,
                        library_uuid,
                        binding,
                        super::descriptor::LEGACY_KEY,
                        &remote_bytes,
                    )
                    .await?;
                    self.registry.confirm_remote_descriptor(
                        library_uuid,
                        binding.binding_epoch,
                        binding.transport_generation,
                        &checkpoint_key,
                    )?;
                    return Ok(published);
                }
            }
            let legacy = if seed_publication {
                None
            } else {
                let object = provider
                    .object_get(
                        &context,
                        super::descriptor::LEGACY_KEY,
                        super::descriptor::MAX_DESCRIPTOR_BYTES,
                    )
                    .await
                    .map_err(anyhow::Error::new)?;
                super::provider::validate_download(
                    &object,
                    super::descriptor::MAX_DESCRIPTOR_BYTES,
                )
                .map_err(anyhow::Error::new)?;
                let verified = self.registry.verified_remote_descriptor(
                    &binding.account_uuid,
                    library_uuid,
                    &object.metadata,
                )?;
                let (candidate, seed, _) = super::descriptor::download_candidate_reusing(
                    provider,
                    &context,
                    &object.metadata,
                    verified.as_ref(),
                )
                .await?;
                self.registry
                    .cache_bound_descriptor(library_uuid, binding, &candidate)?;
                if !super::checkpoint::checkpoint_covers(&payload, &seed)
                    || !super::descriptor::history_covers(
                        &super::descriptor::verified_history(&payload)?,
                        &super::descriptor::verified_history(&seed)?,
                    )
                {
                    anyhow::bail!("descriptor snapshot does not cover seed");
                }
                Some(hex_hash(&object.metadata.sha256))
            };
            let mut dependencies = vec![checkpoint_key.clone()];
            dependencies.extend(
                payload
                    .media_index_shards
                    .iter()
                    .map(|shard| shard.object_key.clone()),
            );
            let descriptor = super::descriptor::from_checkpoint(
                &payload,
                checkpoint_hash,
                binding.binding_epoch,
                created_at,
                dependencies,
                legacy,
            )?;
            let bytes = super::codec::canonical_bytes(&descriptor)?;
            let descriptor_key = if seed_publication {
                super::descriptor::LEGACY_KEY.to_owned()
            } else {
                super::descriptor::snapshot_key(&bytes)
            };
            let descriptor_hash = super::codec::sha256(&bytes);
            if self.registry.has_published_receipt(
                library_uuid,
                binding,
                &descriptor_key,
                &descriptor_hash,
            )? {
                if seed_publication {
                    self.registry.confirm_remote_descriptor(
                        library_uuid,
                        binding.binding_epoch,
                        binding.transport_generation,
                        &checkpoint_key,
                    )?;
                }
                return Ok(published);
            }
            // Check every media dependency before advertising the checkpoint. The
            // publication worker owns uploads; this path verifies its remote receipts.
            let mut media_dependencies = std::collections::BTreeMap::new();
            for shard in &payload.media_index_shards {
                let shard_bytes =
                    std::fs::read(spool.join("indexes").join(format!("{}.idx", shard.sha256)))?;
                if shard_bytes.len() as u64 != shard.byte_length
                    || super::codec::sha256(&shard_bytes)
                        != super::media::parse_hash(&shard.sha256)?
                {
                    anyhow::bail!("checkpoint index shard identity mismatch");
                }
                for entry in super::media::format::IndexShardV1::decode(&shard_bytes)?.entries {
                    media_dependencies
                        .insert(entry.object_key, (entry.object_hash, entry.object_length));
                }
            }
            for (key, (hash, length)) in media_dependencies {
                let object = provider
                    .object_head(&context, &key)
                    .await
                    .map_err(anyhow::Error::new)?
                    .context("checkpoint media dependency unavailable")?;
                if object.key != key
                    || object.size != length
                    || object.sha256 != super::media::parse_hash(&hash)?
                {
                    anyhow::bail!("checkpoint media dependency mismatch");
                }
            }
            self.publish_protocol_object(
                provider,
                &context,
                library_uuid,
                binding,
                &checkpoint_key,
                &checkpoint,
            )
            .await?;
            for shard in &payload.media_index_shards {
                let shard_bytes =
                    std::fs::read(spool.join("indexes").join(format!("{}.idx", shard.sha256)))?;
                self.publish_protocol_object(
                    provider,
                    &context,
                    library_uuid,
                    binding,
                    &shard.object_key,
                    &shard_bytes,
                )
                .await?;
            }
            super::fault::hit("descriptor.after-dependencies");
            self.publish_protocol_object(
                provider,
                &context,
                library_uuid,
                binding,
                &descriptor_key,
                &bytes,
            )
            .await?;
            if seed_publication {
                self.registry.confirm_remote_descriptor(
                    library_uuid,
                    binding.binding_epoch,
                    binding.transport_generation,
                    &checkpoint_key,
                )?;
            }
        }
        Ok(published)
    }

    async fn publish_protocol_object(
        &self,
        provider: &dyn super::provider::SyncProvider,
        context: &super::provider::TransportContext,
        library_uuid: &str,
        binding: &crate::host_registry::BindingTransport,
        key: &str,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        let hash = super::codec::sha256(bytes);
        if self
            .registry
            .has_published_receipt(library_uuid, binding, key, &hash)?
        {
            return Ok(());
        }
        let result = provider
            .object_put_immutable(context, key, bytes, hash)
            .await
            .map_err(anyhow::Error::new)?;
        let (state, etag) = put_receipt(&result)?;
        self.registry.record_provider_receipt(
            library_uuid,
            binding.binding_epoch,
            binding.transport_generation,
            key,
            etag,
            &hash,
            state,
        )
    }

    fn commit_host_published_frontier(
        &self,
        library_uuid: &str,
        published: &[PublishedSegment],
    ) -> anyhow::Result<()> {
        let Some(frontier) = published.iter().map(|segment| segment.last_seq).max() else {
            return Ok(());
        };
        let Some(binding) = self.registry.binding_transport(library_uuid, "library")? else {
            return Ok(());
        };
        self.registry.advance_published_frontier(
            library_uuid,
            binding.binding_epoch,
            binding.transport_generation,
            frontier,
            &published
                .iter()
                .max_by_key(|segment| segment.last_seq)
                .expect("frontier exists")
                .object_key,
        )
    }

    fn finish_seed_if_ready(
        &self,
        library_uuid: &str,
        result: &SyncTurnResult,
    ) -> anyhow::Result<()> {
        if let Some((checkpoint, generation)) = result.seed_checkpoint {
            if let Some(capture_generation) = self
                .registry
                .pending_sync_capture_generation(library_uuid)?
            {
                // A later coherent checkpoint may cover edits made after the
                // original capture boundary. Those events remain in outbox and
                // replay idempotently; the descriptor continues to record the
                // original enable boundary rather than silently moving it.
                if generation < capture_generation {
                    return Ok(());
                }
                self.registry.mark_local_seed_ready(
                    library_uuid,
                    checkpoint,
                    capture_generation,
                    true,
                )?;
            }
        }
        Ok(())
    }

    fn apply_host_spool(&self, spool_dir: &Path) -> anyhow::Result<usize> {
        let directory = spool_dir.join("host");
        if !directory.exists() {
            return Ok(0);
        }
        let mut paths = std::fs::read_dir(directory)?
            .map(|entry| entry.map(|v| v.path()))
            .collect::<Result<Vec<_>, _>>()?;
        paths.sort();
        let mut applied = 0;
        for path in paths {
            if path.extension().and_then(|v| v.to_str()) != Some("json") {
                continue;
            }
            let segment: super::segment::SealedSegment =
                serde_json::from_slice(&std::fs::read(&path)?)?;
            applied += self.registry.apply_host_segment(&segment)?;
        }
        Ok(applied)
    }
}

fn normalize_sync_error_code(error: &anyhow::Error) -> String {
    let normalized = error
        .to_string()
        .to_ascii_lowercase()
        .replace([' ', '-'], "_");
    for code in [
        "authorization_required",
        "quota",
        "compatibility",
        "remote_corruption",
        "disk_full",
        "path_conflict",
        "locked",
        "rate_limited",
        "unavailable",
    ] {
        if normalized.contains(code) {
            return code.to_owned();
        }
    }
    "provider_unavailable".to_owned()
}

fn perform_turn(
    database: &mut crate::db::Database,
    host_vector: super::checkpoint::StreamVector,
    host_metadata: super::checkpoint::HostMetadataSnapshot,
    spool_dir: PathBuf,
) -> anyhow::Result<SyncTurnResult> {
    let connection = database.connection_for_fsops();
    super::capture::capture_enrolled_root_identities(connection)?;
    super::persistence::recover_interrupted_checkpoints(connection)?;
    register_unindexed_segments(connection, &spool_dir)?;
    seal_next_outbox_segment(connection, &spool_dir)?;
    let apply_started = std::time::Instant::now();
    let applied_events = super::spool::apply_next_durable_segment(connection)?;
    let apply_transaction_micros = apply_started.elapsed().as_micros() as u64;
    advance_bulk_turn(connection)?;
    let portable_checkpoint = latest_portable_checkpoint(connection)?;
    let pending: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sync_file_seed_progress) OR EXISTS(SELECT 1 FROM sync_publication_intents WHERE state IN ('pending-hash','pending-manifest','ready','failed')) OR EXISTS(SELECT 1 FROM media_publication_jobs WHERE phase NOT IN ('event-published','cancelled','archived'))",[],|row|row.get(0))?;
    let current: Option<(Vec<u8>,i64)> = connection.query_row("SELECT checkpoint_uuid,capture_local_mutation_generation FROM sync_checkpoints WHERE state='ready' ORDER BY rowid DESC LIMIT 1",[],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    let matches = if let Some((id, generation)) = &current {
        let local_generation: i64 = connection.query_row(
            "SELECT next_local_mutation_generation FROM sync_local_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let checkpoint = super::persistence::load_and_validate_checkpoint(
            connection,
            super::types::CheckpointUuid(uuid::Uuid::from_slice(id)?),
        )?;
        let mut statement = connection.prepare("SELECT device_uuid,stream_uuid,actor_epoch,applied_through_seq FROM sync_actor_frontiers")?;
        let vectors = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut covers = true;
        for (device, stream, epoch, seq) in vectors {
            let actor = super::types::ActorId {
                device_uuid: super::types::DeviceUuid(uuid::Uuid::from_slice(&device)?),
                stream_uuid: super::types::StreamUuid(uuid::Uuid::from_slice(&stream)?),
                actor_epoch: super::types::ActorEpochUuid(uuid::Uuid::from_slice(&epoch)?),
            };
            covers &= checkpoint.catalog_vector.get(&actor).copied().unwrap_or(0) == seq;
        }
        *generation == local_generation
            && covers
            && checkpoint.host_vector == host_vector
            && checkpoint.host_metadata.name == host_metadata.name
            && checkpoint.host_metadata.icon == host_metadata.icon
            && (pending || checkpoint.recovery.portable_complete)
    } else {
        false
    };
    let (checkpoint_built, seed_checkpoint) = if !matches && (!pending || current.is_none()) {
        let fence =
            super::persistence::capture_checkpoint_fence(connection, host_vector, host_metadata)?;
        super::persistence::build_checkpoint(connection, &fence, &spool_dir.join("checkpoints"))?;
        let ready = super::persistence::descriptor_ready(connection, fence.checkpoint_uuid)?;
        (
            true,
            ready.then_some((fence.checkpoint_uuid.0, fence.local_generation)),
        )
    } else {
        (false, portable_checkpoint)
    };
    Ok(SyncTurnResult {
        capture_pending: false,
        applied_events,
        checkpoint_built,
        apply_transaction_micros,
        seed_checkpoint,
    })
}

fn latest_portable_checkpoint(
    connection: &rusqlite::Connection,
) -> anyhow::Result<Option<(uuid::Uuid, i64)>> {
    let mut statement = connection.prepare(
        "SELECT checkpoint_uuid,capture_local_mutation_generation FROM sync_checkpoints WHERE state='ready' ORDER BY rowid DESC",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, generation) in rows {
        let checkpoint_uuid = super::types::CheckpointUuid(uuid::Uuid::from_slice(&id)?);
        if super::persistence::descriptor_ready(connection, checkpoint_uuid)? {
            return Ok(Some((checkpoint_uuid.0, generation)));
        }
    }
    Ok(None)
}

fn commit_published_segments(
    connection: &rusqlite::Connection,
    published: &[PublishedSegment],
) -> anyhow::Result<()> {
    if published.is_empty() {
        return Ok(());
    }
    let transaction = connection.unchecked_transaction()?;
    let now = unix_timestamp_millis();
    for segment in published {
        transaction.execute(
            "UPDATE sync_segments SET state='published' WHERE segment_uuid=?1 AND actor_epoch=?2 AND state IN ('sealed','published')",
            rusqlite::params![
                segment.segment_uuid.as_slice(),
                segment.actor_epoch.as_bytes().as_slice()
            ],
        )?;
        transaction.execute(
            "UPDATE sync_outbox SET state='published',published_at=COALESCE(published_at,?1) WHERE segment_uuid=?2 AND state IN ('sealed','published')",
            rusqlite::params![now, segment.segment_uuid.as_slice()],
        )?;
        transaction.execute(
            "UPDATE sync_actor_epochs SET published_through_seq=MAX(published_through_seq,?1) WHERE actor_epoch=?2",
            rusqlite::params![
                segment.last_seq as i64,
                segment.actor_epoch.as_bytes().as_slice()
            ],
        )?;
    }
    transaction.execute(
            "UPDATE media_publication_jobs SET phase='event-published',owner_token=NULL,lease_expires_at=NULL,updated_at=?1 WHERE phase='canonicalized' AND intent_uuid IN (SELECT i.intent_uuid FROM sync_publication_intents i JOIN sync_outbox o ON o.event_uuid=i.event_uuid WHERE o.state IN ('published','acked'))",
            [now],
        )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
pub(crate) async fn publish_test_catalog_segments(
    writer: &crate::db::DbWriter,
    provider: &dyn super::provider::SyncProvider,
    context: &super::provider::TransportContext,
    spool: PathBuf,
) -> anyhow::Result<()> {
    let segments=writer.write_authoritative(move |db| {
        while seal_next_outbox_segment(db.connection_for_fsops(),&spool)? {}
        let mut query=db.connection_for_fsops().prepare("SELECT segment_uuid,actor_epoch,last_seq,spool_path FROM sync_segments WHERE state='sealed' ORDER BY actor_epoch,first_seq")?;
        let segments=query.query_map([],|row|Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,Vec<u8>>(1)?,row.get::<_,u64>(2)?,row.get::<_,String>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(segments)
    }).await?;
    let mut published = Vec::new();
    for (id, actor, last_seq, path) in segments {
        let bytes = std::fs::read(path)?;
        let hash = super::codec::sha256(&bytes);
        let key = format!("segments/{}.json", hex_hash(&hash));
        provider
            .object_put_immutable(context, &key, &bytes, hash)
            .await?;
        published.push(PublishedSegment {
            segment_uuid: id
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid segment id"))?,
            actor_epoch: uuid::Uuid::from_slice(&actor)?,
            last_seq,
            object_key: key,
        });
    }
    writer
        .write_authoritative(move |db| {
            commit_published_segments(db.connection_for_fsops(), &published)
        })
        .await
}

fn seal_next_outbox_segment(
    connection: &rusqlite::Connection,
    spool_dir: &Path,
) -> anyhow::Result<bool> {
    let actor: Option<Vec<u8>> = connection
        .query_row(
            "SELECT e.actor_epoch FROM sync_outbox o JOIN sync_events e ON e.event_uuid=o.event_uuid JOIN sync_actor_epochs actor ON actor.actor_epoch=e.actor_epoch WHERE o.state='pending' AND actor.state IN ('active','forked') ORDER BY o.created_at,e.actor_epoch,e.actor_seq LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(actor) = actor else { return Ok(false) };
    let mut statement = connection.prepare(
        "SELECT e.payload FROM sync_outbox o JOIN sync_events e ON e.event_uuid=o.event_uuid WHERE o.state='pending' AND e.actor_epoch=?1 ORDER BY e.actor_seq LIMIT 256",
    )?;
    let events = statement
        .query_map([&actor], |row| row.get::<_, Vec<u8>>(0))?
        .map(|row| Ok(super::codec::decode_event(&row?)?))
        .collect::<anyhow::Result<Vec<_>>>()?;
    drop(statement);
    if events.is_empty() {
        return Ok(false);
    }
    let predecessor_path: Option<String> = connection
        .query_row(
            "SELECT spool_path FROM sync_segments WHERE actor_epoch=?1 AND state IN ('sealed','published','acked') ORDER BY last_seq DESC LIMIT 1",
            [&actor],
            |row| row.get(0),
        )
        .optional()?;
    let predecessor = predecessor_path
        .map(|path| -> anyhow::Result<[u8; 32]> {
            Ok(
                serde_json::from_slice::<super::segment::SealedSegment>(&std::fs::read(path)?)?
                    .segment_hash,
            )
        })
        .transpose()?;
    let segment = super::segment::seal_segment(events, predecessor)?;
    let outgoing = spool_dir.join("outgoing");
    std::fs::create_dir_all(&outgoing)?;
    let bytes = serde_json::to_vec(&segment)?;
    let path = outgoing.join(format!("{}.segment.json", hex_hash(&segment.segment_hash)));
    atomic_write(&path, &bytes)?;
    let mut segment_uuid = [0u8; 16];
    segment_uuid.copy_from_slice(&segment.segment_hash[..16]);
    let transaction = connection.unchecked_transaction()?;
    transaction.execute(
        "INSERT INTO sync_segments(segment_uuid,stream_uuid,stream_kind,device_uuid,actor_epoch,binding_epoch,first_seq,last_seq,event_count,wire_version,reducer_version,compression,decoded_bytes,predecessor_hash,payload_hash,spool_path,state,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'none',?12,?13,?14,?15,'sealed',?16)",
        rusqlite::params![
            segment_uuid.as_slice(),
            segment.header.actor.stream_uuid.0.as_bytes().as_slice(),
            match segment.header.stream_kind { super::types::StreamKind::Catalog => "catalog", super::types::StreamKind::HostMetadata => "host-metadata" },
            segment.header.actor.device_uuid.0.as_bytes().as_slice(),
            segment.header.actor.actor_epoch.0.as_bytes().as_slice(),
            segment.events[0].binding_epoch.0.as_bytes().as_slice(),
            segment.header.first_seq as i64,
            segment.header.last_seq as i64,
            segment.header.count as i64,
            segment.header.wire_version,
            segment.header.reducer_version,
            segment.header.decoded_bytes as i64,
            segment.header.predecessor_hash.map(|value| value.to_vec()),
            segment.header.payload_hash.as_slice(),
            path.to_string_lossy(),
            unix_timestamp_millis(),
        ],
    )?;
    transaction.execute(
        "UPDATE sync_outbox SET state='sealed',segment_uuid=?1 WHERE state='pending' AND event_uuid IN (SELECT event_uuid FROM sync_events WHERE actor_epoch=?2 AND actor_seq BETWEEN ?3 AND ?4)",
        rusqlite::params![segment_uuid.as_slice(),actor,segment.header.first_seq as i64,segment.header.last_seq as i64],
    )?;
    transaction.commit()?;
    Ok(true)
}

fn put_receipt(
    result: &super::provider::PutImmutableResult,
) -> anyhow::Result<(&'static str, Option<&str>)> {
    match result {
        super::provider::PutImmutableResult::Created { etag } => Ok(("created", etag.as_deref())),
        super::provider::PutImmutableResult::AlreadyExistsMatching { etag } => {
            Ok(("already-exists-matching", etag.as_deref()))
        }
        super::provider::PutImmutableResult::HashConflict { .. } => {
            anyhow::bail!("provider immutable object hash conflict")
        }
    }
}
fn hex_hash(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if path.exists() {
        return Ok(());
    }
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::now_v7().simple()));
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(temp, path)?;
    Ok(())
}

fn unix_timestamp_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn advance_bulk_turn(connection: &rusqlite::Connection) -> anyhow::Result<()> {
    use rusqlite::OptionalExtension;
    let row:Option<(Vec<u8>,String)>=connection.query_row("SELECT bulk_operation_uuid,state FROM sync_bulk_operations WHERE state IN ('pending','building','publishing') ORDER BY created_at LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((id, state)) = row else {
        return Ok(());
    };
    let operation = uuid::Uuid::from_slice(&id)?;
    let transaction = connection.unchecked_transaction()?;
    if state == "pending" || state == "building" {
        super::bulk::build_subtree_delete_turn(
            &transaction,
            operation,
            super::bulk::MAX_BULK_ITEMS_PER_TURN,
        )?;
    } else {
        super::bulk::publish_subtree_delete_turn(
            &transaction,
            operation,
            super::bulk::MAX_BULK_ITEMS_PER_TURN,
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn register_unindexed_segments(
    connection: &rusqlite::Connection,
    spool_dir: &Path,
) -> anyhow::Result<usize> {
    if !spool_dir.exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in std::fs::read_dir(spool_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        let segment: super::segment::SealedSegment = serde_json::from_slice(&bytes)?;
        super::segment::verify_segment(&segment, segment.header.predecessor_hash)
            .with_context(|| format!("verify downloaded sync segment {}", path.display()))?;
        let mut id = [0; 16];
        id.copy_from_slice(&segment.segment_hash[..16]);
        let record = super::spool::DurableInboxSegment {
            segment_uuid: id,
            stream_uuid: *segment.header.actor.stream_uuid.0.as_bytes(),
            device_uuid: *segment.header.actor.device_uuid.0.as_bytes(),
            actor_epoch: *segment.header.actor.actor_epoch.0.as_bytes(),
            segment_hash: segment.segment_hash,
            payload_hash: segment.header.payload_hash,
            spool_path: PathBuf::from(&path),
            first_seq: segment.header.first_seq,
            last_seq: segment.header.last_seq,
            event_count: segment.header.count,
        };
        if super::spool::register_durable_segment(connection, &record)? {
            count += 1
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        Database, DbWriter, FileUpsert, MediaPageQuery, NewRoot, SearchQuery, ThumbBlobRecord,
        ThumbnailStateUpsert, UserMetadataPatch,
    };
    use crate::host_registry::{CreationIntent, LibraryDescriptor, RegisterBundledPlugin};
    use crate::sync::provider::SyncProvider;
    use crate::sync::segment::seal_segment;
    use crate::sync::types::Operation;
    use rusqlite::params;
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    };

    fn register_test_provider_account(
        registry: &HostRegistry,
        plugin_id: &str,
        account_uuid: &str,
    ) {
        registry
            .register_bundled_plugin(RegisterBundledPlugin {
                plugin_id,
                version: "1.0.0",
                manifest_json: r#"{"manifestVersion":2}"#,
                entry_relative_path: "bin/provider",
                entry_sha256: &[7; 32],
                install_root: std::path::Path::new("bundled"),
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();
        registry
            .upsert_plugin_account(account_uuid, plugin_id, Some("Test"), "authenticated")
            .unwrap();
        registry.set_plugin_intent(plugin_id, true, true).unwrap();
        registry
            .record_plugin_runtime_ready(plugin_id, "test-session", &["sync-provider.v1".into()])
            .unwrap();
    }

    #[tokio::test]
    async fn descriptor_refresh_enrolled_roots_media_discovery_and_idle() {
        fn storage_marker(path: &std::path::Path, library: uuid::Uuid, root: uuid::Uuid) -> String {
            std::fs::create_dir_all(path).unwrap();
            crate::library_path::atomic_write_marker_guarded(
                &crate::library_path::SafeDirectory::open(path).unwrap(),
                &crate::library_path::LibraryMarker {
                    format_version: crate::library_path::STORAGE_FORMAT_VERSION,
                    library_uuid: library.to_string(),
                    root_uuid: root.to_string(),
                    state: crate::library_path::MarkerState::Committed,
                    creation_txn_uuid: uuid::Uuid::now_v7().to_string(),
                },
            )
            .unwrap()
        }
        let base = std::env::temp_dir().join(format!(
            "mangoo-descriptor-refresh-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let library = uuid::Uuid::now_v7();
        let library_root = uuid::Uuid::now_v7();
        let registry = Arc::new(HostRegistry::open(base.join("host.sqlite")).unwrap());
        register_test_provider_account(&registry, "fake", "account");
        registry
            .commit_creation(
                "create",
                &LibraryDescriptor {
                    library_uuid: library.to_string(),
                    root_uuid: library_root.to_string(),
                    name: "Library".into(),
                    icon: None,
                    color: None,
                    local_data_dir: base.join("catalog"),
                    managed_root: base.join("catalog"),
                    lifecycle_state: "ready".into(),
                    binding_state: "local-only".into(),
                    availability: "available".into(),
                    marker_identity: storage_marker(&base.join("catalog"), library, library_root),
                },
            )
            .unwrap();
        std::fs::create_dir_all(base.join("catalog")).unwrap();
        let path = base.join("catalog/catalog.sqlite");
        let database = Database::open(&path).unwrap();
        database.apply_migrations().unwrap();
        database
            .add_root(NewRoot {
                path: base.join("managed").to_string_lossy().into_owned(),
                display_name: "Library".into(),
            })
            .unwrap();
        database
            .initialize_sync_identity(library, library_root, registry.device_uuid().unwrap())
            .unwrap();
        // Empty managed seed before any user directory is enrolled.
        database
            .connection_for_fsops()
            .execute("DELETE FROM roots", [])
            .unwrap();
        let (binding, actor) = registry
            .begin_sync_binding(&library.to_string(), "fake", "account")
            .unwrap();
        let writer = DbWriter::spawn(database);
        let coordinator = SyncCoordinator::new(registry.clone(), RuntimeLeases::default());
        let provider = Arc::new(crate::sync::provider::InMemorySyncProvider::default());
        coordinator
            .provider_manager()
            .register("fake", "account", provider.clone())
            .unwrap();
        // Retry commits preparing before the asynchronous Catalog boundary.
        registry
            .record_sync_turn_error(&library.to_string(), "interrupted-capture")
            .unwrap();
        assert!(registry
            .retry_binding(
                &library.to_string(),
                0,
                &registry
                    .binding_transport(&library.to_string(), "library")
                    .unwrap()
                    .unwrap()
            )
            .unwrap());
        let scheduler = coordinator.spawn_scheduler(std::time::Duration::from_millis(10), 1, {
            let library = library.to_string();
            let path = path.clone();
            let writer = writer.clone();
            move || Some((library.clone(), path.clone(), writer.clone()))
        });
        for _ in 0..50 {
            let polled: bool = rusqlite::Connection::open(base.join("host.sqlite"))
                .unwrap()
                .query_row(
                    "SELECT last_success_at IS NOT NULL FROM library_sync_bindings",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            if polled {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(registry
            .sync_capture_boundary_pending(&library.to_string())
            .unwrap());
        assert_eq!(
            registry
                .binding_transport(&library.to_string(), "library")
                .unwrap()
                .unwrap()
                .state,
            "preparing"
        );
        assert_eq!(
            writer
                .write(|db| Ok(db.connection_for_fsops().query_row(
                    "SELECT COUNT(*) FROM sync_checkpoints",
                    [],
                    |row| row.get::<_, u64>(0)
                )?))
                .await
                .unwrap(),
            0
        );
        let generation = writer
            .write_authoritative(move |db| db.begin_sync_capture(binding, actor))
            .await
            .unwrap();
        registry
            .set_sync_capture_generation(&library.to_string(), binding, generation)
            .unwrap();
        coordinator.wake();
        for _ in 0..50 {
            if registry
                .binding_transport(&library.to_string(), "library")
                .unwrap()
                .unwrap()
                .latest_checkpoint_uuid
                .is_some()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            registry
                .binding_transport(&library.to_string(), "library")
                .unwrap()
                .unwrap()
                .latest_checkpoint_uuid
                .is_some(),
            "capture completion must be eligible immediately, not after a dormant poll delay"
        );
        scheduler.shutdown().await.unwrap();
        for _ in 0..1 {
            coordinator
                .run_active_turn(&library.to_string(), &path, &writer)
                .await
                .unwrap();
        }
        let context = crate::sync::provider::TransportContext {
            namespace: library.to_string(),
            binding_epoch: binding.to_string(),
            transport_generation: 1,
        };
        let legacy = provider
            .object_get(&context, "library-descriptor", 1024 * 1024)
            .await
            .unwrap();
        let mut historical: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&legacy.path).unwrap()).unwrap();
        historical["lastSyncedAt"] = serde_json::json!(1);
        let historical_bytes = serde_json::to_vec_pretty(&historical).unwrap();
        let legacy_hash = super::super::codec::sha256(&historical_bytes);
        provider.replace_fixture_object("library-descriptor", historical_bytes.clone());
        let host_fixture = rusqlite::Connection::open(base.join("host.sqlite")).unwrap();
        host_fixture.execute("UPDATE provider_object_receipts SET state='durable' WHERE object_key='library-descriptor' OR object_key LIKE 'checkpoints/%'",[]).unwrap();
        host_fixture.execute("UPDATE provider_object_receipts SET sha256=?1 WHERE object_key='library-descriptor'",[legacy_hash.as_slice()]).unwrap();
        host_fixture
            .execute("DELETE FROM provider_verified_descriptors", [])
            .unwrap();
        host_fixture
            .execute("DELETE FROM provider_remote_descriptor_heads", [])
            .unwrap();
        drop(host_fixture);
        registry
            .record_sync_turn_error(&library.to_string(), "old-build-self-poll")
            .unwrap();
        registry
            .retry_binding(
                &library.to_string(),
                0,
                &registry
                    .binding_transport(&library.to_string(), "library")
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            registry
                .binding_transport(&library.to_string(), "library")
                .unwrap()
                .unwrap()
                .state,
            "seed_ready"
        );
        coordinator
            .run_active_turn(&library.to_string(), &path, &writer)
            .await
            .unwrap();
        assert_eq!(
            registry
                .binding_transport(&library.to_string(), "library")
                .unwrap()
                .unwrap()
                .state,
            "active"
        );
        assert_eq!(
            std::fs::read(
                &provider
                    .object_get(&context, "library-descriptor", 1024 * 1024)
                    .await
                    .unwrap()
                    .path
            )
            .unwrap(),
            historical_bytes
        );
        let legacy_doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&legacy.path).unwrap()).unwrap();
        assert_eq!(legacy_doc["mediaCount"], 0);
        assert_eq!(legacy_doc["roots"].as_array().unwrap().len(), 0);
        let first_seed_checkpoint = registry
            .binding_transport(&library.to_string(), "library")
            .unwrap()
            .unwrap()
            .latest_checkpoint_uuid;
        let mut expected_bytes = 0;
        for index in 0..2 {
            let root_path = base.join(format!("source-{index}"));
            std::fs::create_dir_all(&root_path).unwrap();
            let contents = vec![index as u8 + 31; 1024 + index];
            let length = contents.len() as i64;
            expected_bytes += length;
            std::fs::create_dir_all(root_path.join("nested")).unwrap();
            let source = root_path.join("nested/media.jpg");
            std::fs::write(&source, &contents).unwrap();
            let root_uuid = uuid::Uuid::now_v7();
            let folder = uuid::Uuid::now_v7();
            writer
                .write_authoritative(move |db| {
                    db.ensure_selected_sync_root(
                        library,
                        root_uuid,
                        folder,
                        library_root,
                        &root_path.to_string_lossy(),
                        &format!("Root{index}"),
                        &format!("Root{index}"),
                        [index as u8 + 1; 32],
                        uuid::Uuid::now_v7(),
                        binding,
                        uuid::Uuid::now_v7(),
                        1,
                        "active",
                        1,
                        false,
                    )?;
                    let root_id = db.sync_root_id(root_uuid.as_bytes())?.unwrap();
                    let parent_id:i64 = db.connection_for_fsops().query_row(
                        "SELECT id FROM folders WHERE folder_uuid=?1",
                        [folder.as_bytes().as_slice()],
                        |row| row.get(0),
                    )?;
                    db.connection_for_fsops().execute("INSERT INTO folders(root_id,parent_id,name,path_hash,status,folder_uuid) VALUES (?1,?2,'nested','nested','active',?3)",params![root_id,parent_id,uuid::Uuid::now_v7().as_bytes().as_slice()])?;
                    let folder_id=db.connection_for_fsops().last_insert_rowid();
                    let file = db.upsert_file(FileUpsert {
                        root_id,
                        folder_id,
                        name: "media.jpg".into(),
                        ext: "jpg".into(),
                        size: length,
                        mtime: 1,
                        ctime: None,
                        file_key: None,
                    })?;
                    let device:Vec<u8>=db.connection_for_fsops().query_row("SELECT device_uuid FROM sync_local_state WHERE singleton=1",[],|row|row.get(0))?;
                    db.initialize_sync_identity(library,library_root,uuid::Uuid::from_slice(&device)?)?;
                    db.create_sync_publication_intent(file, &source.to_string_lossy(), length)?;
                    Ok(())
                })
                .await
                .unwrap();
        }
        // Saturate only the media budget so production metadata turns must
        // expose pre-object jobs without the convenience turn wrapper.
        registry.wake_binding(&library.to_string()).unwrap();
        let media_permits = coordinator.media_budget.acquire_many(2).await.unwrap();
        let scheduler = coordinator.spawn_scheduler(std::time::Duration::from_millis(10), 1, {
            let library = library.to_string();
            let path = path.clone();
            let writer = writer.clone();
            move || Some((library.clone(), path.clone(), writer.clone()))
        });
        for _ in 0..50 {
            if registry.sync_status_snapshot().unwrap().libraries[0].pending_upload_items >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(registry.sync_status_snapshot().unwrap().libraries[0].pending_upload_items >= 2);
        drop(media_permits);
        scheduler.shutdown().await.unwrap();
        *provider.rejected_put_prefix.lock().unwrap() = Some("descriptors/".into());
        let mut failed = false;
        for _ in 0..12 {
            if coordinator
                .run_active_turn(&library.to_string(), &path, &writer)
                .await
                .is_err()
            {
                failed = true;
                break;
            }
        }
        assert!(
            failed,
            "snapshot failure must occur after dependencies publish"
        );
        let failed_snapshot = provider
            .put_attempts
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert!(failed_snapshot.0.starts_with("descriptors/"));
        let checkpoint_count = writer
            .write(|db| {
                Ok(db.connection_for_fsops().query_row(
                    "SELECT COUNT(*) FROM sync_checkpoints",
                    [],
                    |row| row.get::<_, u64>(0),
                )?)
            })
            .await
            .unwrap();
        coordinator.shutdown_media_workers().await.unwrap();
        writer.shutdown().await.unwrap();
        drop(coordinator);
        drop(registry);
        *provider.rejected_put_prefix.lock().unwrap() = None;
        let registry = Arc::new(HostRegistry::open(base.join("host.sqlite")).unwrap());
        let coordinator = SyncCoordinator::new(registry.clone(), RuntimeLeases::default());
        coordinator
            .provider_manager()
            .register("fake", "account", provider.clone())
            .unwrap();
        let writer = DbWriter::spawn(Database::open(&path).unwrap());
        provider.put_attempts.lock().unwrap().clear();
        for _ in 0..3 {
            coordinator
                .run_active_turn(&library.to_string(), &path, &writer)
                .await
                .unwrap();
        }
        assert_eq!(
            *provider.put_attempts.lock().unwrap(),
            vec![failed_snapshot]
        );
        assert_eq!(
            checkpoint_count,
            writer
                .write(|db| Ok(db.connection_for_fsops().query_row(
                    "SELECT COUNT(*) FROM sync_checkpoints",
                    [],
                    |row| row.get::<_, u64>(0)
                )?))
                .await
                .unwrap()
        );
        coordinator.run_root_discovery_turns().await.unwrap();
        let chosen = registry
            .chosen_descriptor(&library.to_string())
            .unwrap()
            .unwrap();
        assert!(chosen.0.starts_with("descriptors/"));
        let latest = provider
            .object_get(&context, &chosen.0, 1024 * 1024)
            .await
            .unwrap();
        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&latest.path).unwrap()).unwrap();
        assert_eq!(document["roots"].as_array().unwrap().len(), 2);
        assert_eq!(document["mediaCount"], 2);
        assert_eq!(document["logicalBytes"], expected_bytes);
        let candidate = super::super::descriptor::download_candidate(
            provider.as_ref(),
            &context,
            &latest.metadata,
        )
        .await
        .unwrap()
        .0;
        let cached = registry
            .verified_remote_descriptor("account", &library.to_string(), &latest.metadata)
            .unwrap()
            .unwrap();
        let before_heads = provider
            .head_calls
            .load(std::sync::atomic::Ordering::Relaxed);
        super::super::descriptor::download_candidate_reusing(
            provider.as_ref(),
            &context,
            &latest.metadata,
            Some(&cached),
        )
        .await
        .unwrap();
        assert_eq!(
            provider
                .head_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            before_heads,
            "same verified candidate must not revalidate media bodies"
        );
        for etag in [
            None,
            Some(String::new()),
            Some("W/weak".into()),
            Some("changed".into()),
        ] {
            let mut changed = latest.metadata.clone();
            changed.etag = etag;
            assert!(registry
                .verified_remote_descriptor("account", &library.to_string(), &changed)
                .unwrap()
                .is_none());
        }
        let mut changed = latest.metadata.clone();
        changed.size += 1;
        assert!(registry
            .verified_remote_descriptor("account", &library.to_string(), &changed)
            .unwrap()
            .is_none());
        changed = latest.metadata.clone();
        changed.sha256 = [19; 32];
        assert!(registry
            .verified_remote_descriptor("account", &library.to_string(), &changed)
            .unwrap()
            .is_none());
        let host_fixture = rusqlite::Connection::open(base.join("host.sqlite")).unwrap();
        host_fixture.execute("UPDATE provider_verified_descriptors SET account_generation=account_generation+1 WHERE descriptor_key=?1",[&latest.metadata.key]).unwrap();
        assert!(registry
            .verified_remote_descriptor("account", &library.to_string(), &latest.metadata)
            .unwrap()
            .is_none());
        host_fixture.execute("UPDATE provider_verified_descriptors SET account_generation=account_generation-1 WHERE descriptor_key=?1",[&latest.metadata.key]).unwrap();
        drop(host_fixture);
        let discovered = registry.list_remote_libraries().unwrap();
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].media_count, Some(2));
        assert!(discovered[0].compatible);
        let restore_root = base.join("restore");
        std::fs::create_dir_all(&restore_root).unwrap();
        let restore_registry =
            Arc::new(HostRegistry::open(restore_root.join("host.sqlite")).unwrap());
        register_test_provider_account(&restore_registry, "fake", "account");
        restore_registry
            .commit_creation(
                "restore-create",
                &LibraryDescriptor {
                    library_uuid: library.to_string(),
                    root_uuid: library_root.to_string(),
                    name: "Library".into(),
                    icon: None,
                    color: None,
                    local_data_dir: restore_root.clone(),
                    managed_root: restore_root.clone(),
                    lifecycle_state: "ready".into(),
                    binding_state: "local-only".into(),
                    availability: "available".into(),
                    marker_identity: storage_marker(&restore_root, library, library_root),
                },
            )
            .unwrap();
        let restore_coordinator =
            SyncCoordinator::new(restore_registry.clone(), RuntimeLeases::default());
        let restore_provider = Arc::new(provider.fork_fixture());
        restore_coordinator
            .provider_manager()
            .register("fake", "account", restore_provider.clone())
            .unwrap();
        restore_coordinator
            .run_root_discovery_turns()
            .await
            .unwrap();
        restore_registry
            .begin_sync_operation_once(
                "restore",
                None,
                "restore",
                "preparing",
                &serde_json::json!({}),
            )
            .unwrap();
        restore_registry
            .start_restore_plan(crate::host_registry::StartRestoreRequest {
                operation_uuid: "restore",
                account_uuid: "account",
                library_uuid: &library.to_string(),
                managed_root: &restore_root.join("managed"),
                restore_mode: "root-aware",
                inventory_fence: None,
                request_hash: &"1".repeat(64),
            })
            .unwrap();
        restore_registry
            .begin_restore_binding(&library.to_string(), "account")
            .unwrap();
        let restore_chosen = restore_registry
            .chosen_descriptor(&library.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(restore_chosen, chosen);
        let restore_catalog = restore_root.join("catalog.sqlite");
        restore_coordinator
            .provider_transport_turn(&library.to_string(), &restore_catalog)
            .await
            .unwrap();
        restore_coordinator
            .bootstrap_missing_catalog(&library.to_string(), &restore_catalog)
            .unwrap();
        let restored = Database::open(&restore_catalog).unwrap();
        assert_eq!(restored.sync_root_descriptors().unwrap().len(), 2);
        assert_eq!(
            restored
                .connection_for_fsops()
                .query_row("SELECT COUNT(*) FROM roots", [], |row| row.get::<_, u64>(0))
                .unwrap(),
            0
        );
        drop(restored);
        let restore_writer = DbWriter::spawn(Database::open(&restore_catalog).unwrap());
        for _ in 0..3 {
            restore_coordinator
                .run_active_turn(&library.to_string(), &restore_catalog, &restore_writer)
                .await
                .unwrap();
        }
        restore_coordinator
            .run_root_discovery_turns()
            .await
            .unwrap();
        let restored_view = restore_registry.list_remote_libraries().unwrap();
        assert!(restored_view[0].compatible);
        assert_eq!(restored_view[0].media_count, Some(2));
        assert_eq!(restored_view[0].logical_bytes, Some(expected_bytes as u64));
        assert_eq!(
            restore_registry
                .binding_transport(&library.to_string(), "library")
                .unwrap()
                .unwrap()
                .state,
            "active"
        );
        restore_registry
            .update_library_metadata(&library.to_string(), Some("Renamed remotely"), None, None)
            .unwrap();
        restore_writer
            .write_authoritative(|db| {
                // This receiver has no local files. Concurrent same-name folder
                // and file requests must still produce a complete, legal seed
                // for the next device's selected-root restore.
                let assets = {
                    let mut statement = db.connection_for_fsops().prepare(
                        "SELECT h.asset_uuid,r.parent_uuid,r.name FROM asset_content_heads h JOIN sync_location_heads l ON l.entity_uuid=h.asset_uuid AND l.is_display_winner=1 JOIN sync_location_revisions r ON r.revision_uuid=l.revision_uuid ORDER BY h.asset_uuid",
                    )?;
                    let rows = statement.query_map([], |row| Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, String>(2)?,
                    )))?.collect::<Result<Vec<_>, _>>()?;
                    rows
                };
                assert_eq!(assets.len(), 2);
                let (folder_parent, folder_name): (Vec<u8>, String) = db.connection_for_fsops().query_row(
                    "SELECT r.parent_uuid,r.name FROM sync_location_heads h JOIN sync_location_revisions r ON r.revision_uuid=h.revision_uuid WHERE h.entity_uuid=?1 AND h.is_display_winner=1",
                    [assets[0].1.as_slice()], |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                let transaction = db.connection_for_fsops().unchecked_transaction()?;
                super::super::capture::capture_catalog_mutation(
                    &transaction,
                    super::super::capture::MutationOrigin::LocalApi,
                    super::super::types::EntityUuid(uuid::Uuid::from_slice(&assets[0].0)?),
                    super::super::types::Operation::SetField {
                        field: "note".into(),
                        value: serde_json::json!("portable note"),
                    },
                )?;
                for (entity, parent, name) in [
                    (&assets[1].1, &folder_parent, &folder_name),
                    (&assets[1].0, &assets[0].1, &assets[0].2),
                ] {
                    let supersedes = {
                        let mut statement = transaction.prepare("SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1")?;
                        let revisions = statement.query_map([entity.as_slice()], |row| row.get::<_, Vec<u8>>(0))?
                            .collect::<Result<Vec<_>, _>>()?;
                        revisions.into_iter().map(|revision| uuid::Uuid::from_slice(&revision).map(super::super::types::RevisionUuid)).collect::<Result<std::collections::BTreeSet<_>, _>>()?
                    };
                    super::super::capture::capture_catalog_mutation(
                        &transaction,
                        super::super::capture::MutationOrigin::LocalApi,
                        super::super::types::EntityUuid(uuid::Uuid::from_slice(entity)?),
                        super::super::types::Operation::SetLocation {
                            revision_uuid: super::super::types::RevisionUuid(uuid::Uuid::now_v7()),
                            parent_uuid: Some(super::super::types::EntityUuid(uuid::Uuid::from_slice(parent)?)),
                            name: name.clone(),
                            supersedes,
                        },
                    )?;
                }
                transaction.commit()?;
                Ok(())
            })
            .await
            .unwrap();
        for _ in 0..3 {
            restore_coordinator
                .run_active_turn(&library.to_string(), &restore_catalog, &restore_writer)
                .await
                .unwrap();
        }
        restore_coordinator
            .run_root_discovery_turns()
            .await
            .unwrap();
        let updated = restore_registry
            .chosen_descriptor(&library.to_string())
            .unwrap()
            .unwrap();
        assert_ne!(updated.0, chosen.0);
        let published = restore_provider
            .object_get(&context, &updated.0, 1024 * 1024)
            .await
            .unwrap();
        let (_, third_checkpoint, _) = super::super::descriptor::download_candidate(
            restore_provider.as_ref(),
            &context,
            &published.metadata,
        )
        .await
        .unwrap();
        assert_eq!(third_checkpoint.projection.files.len(), 2);
        let third_path = restore_root.join("third.sqlite");
        let selected = third_checkpoint
            .state
            .roots
            .values()
            .map(|root| {
                (
                    root.root_uuid.0,
                    restore_root.join(format!("third-{}", root.root_uuid)),
                )
            })
            .collect::<Vec<_>>();
        super::super::persistence::materialize_staging_catalog_with_roots(
            &third_checkpoint,
            &third_path,
            library_root,
            uuid::Uuid::now_v7(),
            &selected,
        )
        .unwrap();
        let third = Database::open(&third_path).unwrap();
        assert_eq!(
            third
                .connection_for_fsops()
                .query_row(
                    "SELECT COUNT(*) FROM user_metadata WHERE note='portable note'",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(
            third
                .connection_for_fsops()
                .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, u64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            third
                .connection_for_fsops()
                .query_row(
                    "SELECT COUNT(DISTINCT name),COUNT(DISTINCT folder_id) FROM files",
                    [],
                    |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
                )
                .unwrap(),
            (2, 1),
        );
        assert_eq!(
            third.connection_for_fsops().query_row(
                "SELECT COUNT(*),COUNT(DISTINCT name),COUNT(DISTINCT parent_id) FROM folders WHERE parent_id IS NOT NULL",
                [], |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?, row.get::<_, u64>(2)?)),
            ).unwrap(),
            (2, 2, 1),
        );
        assert_eq!(
            third
                .connection_for_fsops()
                .query_row("SELECT SUM(size) FROM files", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            expected_bytes
        );
        drop(third);
        restore_coordinator.shutdown_media_workers().await.unwrap();
        restore_writer.shutdown().await.unwrap();
        // Migration 18 initializes old confirmed plans with their exact fixed
        // key/hash but no checkpoint fields. A newer snapshot must not replace it.
        let legacy_fixture = rusqlite::Connection::open(restore_root.join("host.sqlite")).unwrap();
        legacy_fixture.execute("UPDATE sync_restore_plans SET catalog_ready=0,state='restoring',descriptor_key='library-descriptor',descriptor_sha256=?1,checkpoint_key=NULL,checkpoint_sha256=NULL",[legacy_hash.as_slice()]).unwrap();
        legacy_fixture
            .execute("UPDATE library_sync_bindings SET state='restoring'", [])
            .unwrap();
        drop(legacy_fixture);
        let legacy_catalog = restore_root.join("legacy.sqlite");
        restore_coordinator
            .provider_transport_turn(&library.to_string(), &legacy_catalog)
            .await
            .unwrap();
        let legacy_chosen = restore_registry
            .chosen_descriptor(&library.to_string())
            .unwrap()
            .unwrap();
        assert_eq!(legacy_chosen.0, "library-descriptor");
        assert_eq!(
            legacy_chosen.1,
            Some(format!(
                "checkpoints/{}.checkpoint",
                first_seed_checkpoint.unwrap()
            ))
        );
        restore_coordinator
            .bootstrap_missing_catalog(&library.to_string(), &legacy_catalog)
            .unwrap();
        let legacy_restored = Database::open(&legacy_catalog).unwrap();
        assert_eq!(legacy_restored.sync_root_descriptors().unwrap().len(), 0);
        drop(legacy_restored);
        drop(restore_coordinator);
        drop(restore_registry);
        let checkpoint_bytes = provider
            .object_get(&context, &chosen.1.clone().unwrap(), 64 * 1024 * 1024)
            .await
            .unwrap();
        let checkpoint: super::super::checkpoint::Checkpoint =
            serde_json::from_slice(&std::fs::read(&checkpoint_bytes.path).unwrap()).unwrap();
        let remote = Database::open_in_memory().unwrap();
        remote.apply_migrations().unwrap();
        let transaction = remote
            .connection_for_fsops()
            .unchecked_transaction()
            .unwrap();
        let mut events = checkpoint.events.clone();
        events.sort_by_key(|event| (event.actor.clone(), event.actor_seq));
        for events in events.chunks(100) {
            super::super::inbox::apply_remote_batch(&transaction, events).unwrap();
        }
        transaction.commit().unwrap();
        assert_eq!(remote.sync_root_descriptors().unwrap().len(), 2);
        assert!(remote
            .sync_root_descriptors()
            .unwrap()
            .iter()
            .all(|root| root.creation_event_uuid.is_some()));
        // Same immutable seed pointer remains retry authority even after refresh.
        assert_eq!(
            registry
                .binding_transport(&library.to_string(), "library")
                .unwrap()
                .unwrap()
                .latest_checkpoint_uuid,
            first_seed_checkpoint
        );
        registry
            .record_sync_turn_error(&library.to_string(), "fixture-retry")
            .unwrap();
        registry
            .retry_binding(
                &library.to_string(),
                0,
                &registry
                    .binding_transport(&library.to_string(), "library")
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(
            registry
                .binding_transport(&library.to_string(), "library")
                .unwrap()
                .unwrap()
                .state,
            "active"
        );
        assert_eq!(
            provider
                .object_head(&context, "library-descriptor")
                .await
                .unwrap()
                .unwrap()
                .sha256,
            legacy_hash
        );
        let writes_before_idle = provider.put_attempts.lock().unwrap().len();
        let before = writer
            .write(|db| {
                Ok(db.connection_for_fsops().query_row(
                    "SELECT COUNT(*) FROM sync_checkpoints",
                    [],
                    |row| row.get::<_, u64>(0),
                )?)
            })
            .await
            .unwrap();
        for _ in 0..3 {
            assert!(
                !coordinator
                    .run_active_turn(&library.to_string(), &path, &writer)
                    .await
                    .unwrap()
                    .checkpoint_built
            );
        }
        assert_eq!(
            before,
            writer
                .write(|db| Ok(db.connection_for_fsops().query_row(
                    "SELECT COUNT(*) FROM sync_checkpoints",
                    [],
                    |row| row.get::<_, u64>(0)
                )?))
                .await
                .unwrap()
        );
        assert_eq!(
            registry.sync_status_snapshot().unwrap().libraries[0].pending_upload_items,
            0
        );
        assert_eq!(
            writes_before_idle,
            provider.put_attempts.lock().unwrap().len()
        );
        let selected_root = checkpoint.state.roots.values().next().unwrap().root_uuid.0;
        coordinator
            .fetch_remote_root_snapshot(&library.to_string(), selected_root)
            .await
            .unwrap();
        let fork_provider = Arc::new(provider.fork_fixture());
        let fork_coordinator = SyncCoordinator::new(registry.clone(), RuntimeLeases::default());
        fork_coordinator
            .provider_manager()
            .register("fake", "account", fork_provider.clone())
            .unwrap();
        let mut fork_checkpoint = checkpoint.clone();
        fork_checkpoint.checkpoint_uuid = super::super::types::CheckpointUuid::new();
        let index=fork_checkpoint.events.iter().position(|event|matches!(&event.operation,super::super::types::Operation::SetField{field,..} if field=="extension")).unwrap();
        fork_checkpoint.events[index].operation = super::super::types::Operation::SetField {
            field: "extension".into(),
            value: serde_json::json!("jpeg"),
        };
        fork_checkpoint.events[index] =
            super::super::codec::seal_event(fork_checkpoint.events[index].clone()).unwrap();
        let mut replay = super::super::reducer::ProtocolState::default();
        for event in &fork_checkpoint.events {
            replay.apply(event);
        }
        replay.roots = fork_checkpoint.state.roots.clone();
        fork_checkpoint.state = replay;
        fork_checkpoint.digest = [0; 32];
        fork_checkpoint.digest = super::super::codec::sha256(
            &super::super::codec::canonical_bytes(&fork_checkpoint).unwrap(),
        );
        let fork_bytes = super::super::codec::canonical_bytes(&fork_checkpoint).unwrap();
        let fork_key = format!("checkpoints/{}.checkpoint", fork_checkpoint.checkpoint_uuid);
        fork_provider
            .object_put_immutable(
                &context,
                &fork_key,
                &fork_bytes,
                super::super::codec::sha256(&fork_bytes),
            )
            .await
            .unwrap();
        let mut dependencies = vec![fork_key];
        dependencies.extend(
            fork_checkpoint
                .media_index_shards
                .iter()
                .map(|shard| shard.object_key.clone()),
        );
        let fork_document = super::super::descriptor::from_checkpoint(
            &fork_checkpoint,
            super::super::codec::sha256(&fork_bytes),
            binding,
            1,
            dependencies,
            Some(hex_hash(&legacy_hash)),
        )
        .unwrap();
        let fork_bytes = super::super::codec::canonical_bytes(&fork_document).unwrap();
        fork_provider
            .object_put_immutable(
                &context,
                &super::super::descriptor::snapshot_key(&fork_bytes),
                &fork_bytes,
                super::super::codec::sha256(&fork_bytes),
            )
            .await
            .unwrap();
        assert!(fork_coordinator
            .fetch_remote_root_snapshot(&library.to_string(), selected_root)
            .await
            .unwrap_err()
            .to_string()
            .contains("history fork"));
        drop(fork_coordinator);
        let transport = registry
            .binding_transport(&library.to_string(), "library")
            .unwrap()
            .unwrap();
        let mut actor_fork = candidate.clone();
        actor_fork.metadata.key = format!("{library}/descriptors/{}.json", "b".repeat(64));
        *actor_fork.history.values_mut().next().unwrap() = "c".repeat(64);
        for sequence in actor_fork
            .evidence
            .as_mut()
            .unwrap()
            .catalog_vector
            .values_mut()
        {
            *sequence += 1;
        }
        assert!(registry
            .cache_bound_descriptor(&library.to_string(), &transport, &actor_fork)
            .unwrap_err()
            .to_string()
            .contains("history fork"));
        assert!(registry.list_remote_libraries().unwrap()[0].compatible);
        let mut conflict = candidate.clone();
        conflict.metadata.key = format!("{library}/descriptors/{}.json", "f".repeat(64));
        conflict.evidence.as_mut().unwrap().truth_sha256 = "e".repeat(64);
        registry
            .cache_bound_descriptor(&library.to_string(), &transport, &conflict)
            .unwrap();
        assert!(!registry.list_remote_libraries().unwrap()[0].compatible);
        let mut resolving = candidate.clone();
        resolving.metadata.key = format!("{library}/descriptors/{}.json", "a".repeat(64));
        for seq in resolving
            .evidence
            .as_mut()
            .unwrap()
            .catalog_vector
            .values_mut()
        {
            *seq += 1;
        }
        registry
            .cache_bound_descriptor(&library.to_string(), &transport, &resolving)
            .unwrap();
        assert!(registry.list_remote_libraries().unwrap()[0].compatible);
        registry
            .cache_bound_descriptor(&library.to_string(), &transport, &candidate)
            .unwrap();
        assert_eq!(
            registry
                .chosen_descriptor(&library.to_string())
                .unwrap()
                .unwrap()
                .0,
            format!("descriptors/{}.json", "a".repeat(64))
        );
        let mut concurrent = resolving.clone();
        concurrent.metadata.key = format!("{library}/descriptors/{}.json", "b".repeat(64));
        let vector = &mut concurrent.evidence.as_mut().unwrap().catalog_vector;
        let actor = vector.keys().next().unwrap().clone();
        vector.remove(&actor);
        let mut other_actor = actor;
        other_actor.actor_epoch = super::super::types::ActorEpochUuid(uuid::Uuid::now_v7());
        vector.insert(other_actor, 1);
        registry
            .cache_bound_descriptor(&library.to_string(), &transport, &concurrent)
            .unwrap();
        assert!(!registry.list_remote_libraries().unwrap()[0].compatible);
        assert!(registry
            .has_published_receipt(
                &library.to_string(),
                &transport,
                "library-descriptor",
                &legacy_hash
            )
            .unwrap());
        coordinator.shutdown_media_workers().await.unwrap();
        writer.shutdown().await.unwrap();
        drop(coordinator);
        drop(registry);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn root_binding_projection_is_available_before_remote_apply() {
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let root_uuid = uuid::Uuid::from_u128(41);
        let root_folder_uuid = uuid::Uuid::from_u128(42);
        let root_id = database
            .add_root(NewRoot {
                path: "D:/SelectedRoot".into(),
                display_name: "SelectedRoot".into(),
            })
            .unwrap();
        database
            .upsert_sync_root_descriptor(&crate::db::SyncRootDescriptorRecord {
                root_uuid: *root_uuid.as_bytes(),
                root_folder_uuid: *root_folder_uuid.as_bytes(),
                library_root_uuid: *uuid::Uuid::from_u128(40).as_bytes(),
                creation_event_uuid: None,
                winning_location_revision_uuid: None,
                requested_name: "SelectedRoot".into(),
                materialized_name: "SelectedRoot".into(),
                materialization_fingerprint: [1; 32],
                lifecycle: "active".into(),
                logical_revision: 1,
                updated_at: 1,
            })
            .unwrap();
        database
            .connection_for_fsops()
            .execute(
                "UPDATE roots SET root_uuid=?1 WHERE id=?2",
                rusqlite::params![root_uuid.as_bytes().as_slice(), root_id],
            )
            .unwrap();
        database
            .connection_for_fsops()
            .execute(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,status,folder_uuid) VALUES (?1,NULL,'','root','active',?2)",
                rusqlite::params![root_id, root_folder_uuid.as_bytes().as_slice()],
            )
            .unwrap();
        database
            .link_sync_root_descriptor(root_id, root_uuid.as_bytes(), root_folder_uuid.as_bytes())
            .unwrap();
        let local_binding_uuid = uuid::Uuid::from_u128(43);
        SyncCoordinator::project_root_binding_records(
            &database,
            vec![crate::db::SyncRootLocalBindingRecord {
                root_uuid: *root_uuid.as_bytes(),
                library_uuid: *uuid::Uuid::from_u128(40).as_bytes(),
                local_binding_uuid: *local_binding_uuid.as_bytes(),
                binding_generation: 7,
                root_capture_epoch: [50; 16],
                root_actor_epoch: [51; 16],
                state: "active".into(),
                root_id: 0,
                operation_revision: 3,
                updated_at: 1,
            }],
        )
        .unwrap();

        assert_eq!(
            database
                .sync_root_binding_token_for_root_id(root_id)
                .unwrap(),
            Some(crate::db::SyncRootBindingToken {
                root_uuid: *root_uuid.as_bytes(),
                local_binding_uuid: *local_binding_uuid.as_bytes(),
                binding_generation: 7,
            })
        );
    }

    #[tokio::test]
    async fn unbind_reconciliation_requires_observable_remote_descriptor() {
        let base = std::env::temp_dir().join(format!(
            "mangoo-unbind-reconciliation-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let registry = Arc::new(HostRegistry::open(base.join("host.sqlite")).unwrap());
        register_test_provider_account(&registry, "fake", "account");
        registry
            .commit_creation(
                "test-create",
                &LibraryDescriptor {
                    library_uuid: "library".into(),
                    root_uuid: "root".into(),
                    name: "Library".into(),
                    icon: None,
                    color: None,
                    local_data_dir: base.join("local"),
                    managed_root: base.join("root"),
                    lifecycle_state: "ready".into(),
                    binding_state: "local-only".into(),
                    availability: "available".into(),
                    marker_identity: "marker".into(),
                },
            )
            .unwrap();
        let (binding, _) = registry
            .begin_sync_binding("library", "fake", "account")
            .unwrap();
        let descriptor = br#"{"version":1,"libraryUuid":"library"}"#;
        let descriptor_hash: [u8; 32] = sha2::Sha256::digest(descriptor).into();
        registry
            .record_provider_receipt(
                "library",
                binding,
                1,
                "library-descriptor",
                None,
                &descriptor_hash,
                "created",
            )
            .unwrap();
        let plan = registry.create_unbind_plan("library").unwrap();
        registry
            .advance_unbind_plan(&plan.plan_uuid, "keep-local-fork", "cutover")
            .unwrap();
        registry.begin_unbind("library", binding).unwrap();
        registry.commit_local_unbind("library").unwrap();
        registry
            .advance_unbind_plan(&plan.plan_uuid, "keep-local-fork", "reconciling")
            .unwrap();

        let coordinator = SyncCoordinator::new(Arc::clone(&registry), RuntimeLeases::default());
        let provider = Arc::new(crate::sync::provider::InMemorySyncProvider::default());
        coordinator
            .provider_manager()
            .register("fake", "account", provider.clone())
            .unwrap();

        assert_eq!(coordinator.reconcile_pending_unbinds().await.unwrap(), 0);
        assert!(!registry.unbind_plan_complete(&plan.plan_uuid).unwrap());
        registry
            .record_unbind_reconcile_failure(&plan.plan_uuid, "retry-now", 0)
            .unwrap();
        let context = crate::sync::provider::TransportContext {
            namespace: "library".into(),
            binding_epoch: binding.to_string(),
            transport_generation: 2,
        };
        provider
            .object_put_immutable(&context, "library-descriptor", descriptor, descriptor_hash)
            .await
            .unwrap();
        assert_eq!(coordinator.reconcile_pending_unbinds().await.unwrap(), 1);
        assert!(registry.unbind_plan_complete(&plan.plan_uuid).unwrap());
        drop(coordinator);
        drop(registry);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn inactive_scheduler_is_cancellable_and_joinable() {
        let base = std::env::temp_dir().join(format!(
            "mangoo-sync-scheduler-cancel-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let registry = Arc::new(HostRegistry::open(base.join("host.sqlite")).unwrap());
        let coordinator = SyncCoordinator::new(Arc::clone(&registry), RuntimeLeases::default());
        let handle = coordinator.spawn_scheduler(std::time::Duration::from_secs(5), 2, || None);
        tokio::time::timeout(std::time::Duration::from_millis(250), handle.shutdown())
            .await
            .expect("scheduler shutdown waited for its five-second tick")
            .unwrap();
        drop(coordinator);
        drop(registry);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn cancelled_media_binding_remains_fenced_against_late_turns() {
        let base = std::env::temp_dir().join(format!(
            "mangoo-media-binding-cancel-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let registry = Arc::new(HostRegistry::open(base.join("host.sqlite")).unwrap());
        let coordinator = SyncCoordinator::new(Arc::clone(&registry), RuntimeLeases::default());
        let binding = uuid::Uuid::now_v7();
        assert!(!coordinator
            .media_workers
            .lock()
            .await
            .contains_key(&binding));
        coordinator.cancel_media_binding(binding).await.unwrap();

        let stored = coordinator
            .media_workers
            .lock()
            .await
            .get(&binding)
            .cloned()
            .expect("cancelled epoch must remain as a tombstone");
        assert!(stored.run(async { Ok(()) }).await.is_err());
        drop(stored);
        coordinator.resume_media_binding(binding).await;
        assert!(!coordinator
            .media_workers
            .lock()
            .await
            .contains_key(&binding));
        coordinator.shutdown_media_workers().await.unwrap();
        drop(coordinator);
        drop(registry);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[derive(Clone, Default)]
    struct CountingProvider {
        calls: Arc<AtomicU64>,
        objects: Arc<Mutex<Vec<Vec<u8>>>>,
    }
    impl CountingProvider {
        fn calls(&self) -> u64 {
            self.calls.load(Ordering::Relaxed)
        }
        fn push(&self, bytes: Vec<u8>) {
            self.objects.lock().unwrap().push(bytes);
        }
    }
    impl crate::sync::provider::SegmentProvider for CountingProvider {
        fn pending_segments(&self) -> anyhow::Result<Vec<Vec<u8>>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(std::mem::take(&mut *self.objects.lock().unwrap()))
        }
    }

    fn percentile(mut values: Vec<f64>, p: f64) -> f64 {
        assert!(!values.is_empty());
        values.sort_by(|a, b| a.total_cmp(b));
        values[((values.len() - 1) as f64 * p).round() as usize]
    }

    #[derive(Clone, Copy)]
    struct ProcessMetrics {
        cpu_ms: f64,
        private_bytes: u64,
    }

    #[cfg(windows)]
    fn process_metrics() -> ProcessMetrics {
        #[repr(C)]
        #[derive(Default, Clone, Copy)]
        struct FileTime {
            low: u32,
            high: u32,
        }
        #[repr(C)]
        #[derive(Default)]
        struct ProcessMemoryCountersEx {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
            private_usage: usize,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> isize;
            fn GetProcessTimes(
                process: isize,
                creation: *mut FileTime,
                exit: *mut FileTime,
                kernel: *mut FileTime,
                user: *mut FileTime,
            ) -> i32;
        }
        #[link(name = "psapi")]
        extern "system" {
            fn GetProcessMemoryInfo(
                process: isize,
                counters: *mut ProcessMemoryCountersEx,
                size: u32,
            ) -> i32;
        }
        fn ticks(value: FileTime) -> u64 {
            ((value.high as u64) << 32) | value.low as u64
        }
        unsafe {
            let process = GetCurrentProcess();
            let (mut creation, mut exit, mut kernel, mut user) = Default::default();
            assert_ne!(
                GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user),
                0
            );
            let mut memory = ProcessMemoryCountersEx::default();
            memory.cb = std::mem::size_of::<ProcessMemoryCountersEx>() as u32;
            assert_ne!(GetProcessMemoryInfo(process, &mut memory, memory.cb), 0);
            ProcessMetrics {
                cpu_ms: (ticks(kernel) + ticks(user)) as f64 / 10_000.0,
                private_bytes: memory.private_usage as u64,
            }
        }
    }

    #[cfg(not(windows))]
    fn process_metrics() -> ProcessMetrics {
        let stat = std::fs::read_to_string("/proc/self/stat").expect("read process stat");
        let fields: Vec<&str> = stat.split_whitespace().collect();
        let ticks: f64 =
            fields[13].parse::<u64>().unwrap() as f64 + fields[14].parse::<u64>().unwrap() as f64;
        let status = std::fs::read_to_string("/proc/self/status").expect("read process status");
        let private_kib = status
            .lines()
            .find_map(|line| line.strip_prefix("VmData:"))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        ProcessMetrics {
            cpu_ms: ticks * 10.0,
            private_bytes: private_kib * 1024,
        }
    }

    fn explain_details(connection: &rusqlite::Connection, sql: &str) -> Vec<String> {
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn seed_browse_fixture(
        database: &Database,
        count: usize,
        existing_scope: Option<(i64, i64)>,
    ) -> (i64, i64, i64) {
        assert!(count > 0);
        let (root_id, folder_id) = existing_scope.unwrap_or_else(|| {
            let root_id = database
                .add_root(NewRoot {
                    path: "D:/SyncBench".into(),
                    display_name: "SyncBench".into(),
                })
                .unwrap();
            let folder_id = database
                .upsert_folder(crate::db::FolderUpsert {
                    root_id,
                    parent_id: None,
                    name: "Bench".into(),
                    path_hash: "sync-bench".into(),
                    mtime: Some(1),
                })
                .unwrap();
            (root_id, folder_id)
        });
        let mut first = 0;
        if count >= 100_000 {
            let connection = database.connection_for_fsops();
            let fixture_started = std::time::Instant::now();
            let report = |name: &str, started: std::time::Instant| {
                eprintln!(
                    "SYNC_FIXTURE_STAGE {name} {:.3}s",
                    started.elapsed().as_secs_f64()
                );
            };
            // Fixture construction is disposable benchmark setup, not an
            // authoritative write. Disable journaling only while populating;
            // production WAL/FULL semantics are restored before any measured
            // capture/apply/query begins.
            connection
                .execute_batch("PRAGMA synchronous=OFF; PRAGMA journal_mode=OFF")
                .unwrap();
            let production_indexes = {
                let mut statement = connection.prepare("SELECT name,sql FROM sqlite_schema WHERE type='index' AND tbl_name IN ('files','media','user_metadata','file_tags') AND sql IS NOT NULL ORDER BY name").unwrap();
                statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            };
            for (name, _) in &production_indexes {
                connection
                    .execute_batch(&format!(
                        "DROP INDEX IF EXISTS \"{}\"",
                        name.replace('"', "\"\"")
                    ))
                    .unwrap();
            }
            report("drop-indexes", fixture_started);
            connection.execute_batch("BEGIN IMMEDIATE").unwrap();
            let started = std::time::Instant::now();
            connection.execute(
                "WITH RECURSIVE n(x) AS (SELECT 0 UNION ALL SELECT x+1 FROM n WHERE x+1<?1) INSERT INTO files(root_id,folder_id,name,ext,size,mtime,file_key,status,asset_uuid) SELECT ?2,?3,printf('bench-image-%07d.jpg',x),'.jpg',1024,x+1,printf('bench-%d',x),'active',randomblob(16) FROM n",
                params![count as i64,root_id,folder_id],
            ).unwrap();
            report("files", started);
            first = database
                .connection_for_fsops()
                .query_row(
                    "SELECT MIN(id) FROM files WHERE folder_id=?1",
                    [folder_id],
                    |r| r.get(0),
                )
                .unwrap();
            let started = std::time::Instant::now();
            connection.execute("INSERT INTO media(file_id,kind,metadata_status) SELECT id,'image','pending' FROM files WHERE folder_id=?1",[folder_id]).unwrap();
            report("media", started);
            // Metadata/tag/search projections are intentionally sparse like a
            // real library: every tenth of the one million assets has user
            // metadata and an FTS/tag entry. The production search still joins
            // those rows against the full million-row files/media projection.
            let started = std::time::Instant::now();
            connection.execute("INSERT INTO user_metadata(file_id,rating,favorite,note,updated_at) SELECT id,id%6,id%2,printf('bench note %d',id),1 FROM files WHERE folder_id=?1 AND id%10=0",[folder_id]).unwrap();
            report("metadata", started);
            connection
                .execute(
                    "INSERT INTO tags(name,color) VALUES ('benchmark-tag','#abcdef')",
                    [],
                )
                .unwrap();
            let tag_id = connection.last_insert_rowid();
            let started = std::time::Instant::now();
            connection.execute("INSERT INTO file_tags(file_id,tag_id) SELECT id,?1 FROM files WHERE folder_id=?2 AND id%10=0",params![tag_id,folder_id]).unwrap();
            report("file-tags", started);
            let started = std::time::Instant::now();
            connection.execute("INSERT INTO media_fts(rowid,name,note,tags) SELECT f.id,f.name,u.note,'benchmark-tag' FROM files f JOIN user_metadata u ON u.file_id=f.id WHERE f.folder_id=?1",[folder_id]).unwrap();
            report("fts", started);
            connection.execute_batch("COMMIT").unwrap();
            for (name, sql) in production_indexes {
                let started = std::time::Instant::now();
                connection.execute_batch(&sql).unwrap();
                report(&format!("index:{name}"), started);
            }
            connection
                .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL")
                .unwrap();
            report("fixture-total", fixture_started);
        } else {
            for index in 0..count {
                let file_id = database
                    .upsert_file(FileUpsert {
                        root_id,
                        folder_id,
                        name: format!("bench-image-{index:05}.jpg"),
                        ext: ".jpg".into(),
                        size: 1024,
                        mtime: index as i64 + 1,
                        ctime: None,
                        file_key: Some(format!("bench-{index}")),
                    })
                    .unwrap();
                database.upsert_media_kind(file_id, "image").unwrap();
                if index == 0 {
                    first = file_id;
                }
            }
            database.backfill_media_fts_if_empty().unwrap();
        }
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id: first,
                profile: crate::thumbnails::GRID_320_PROFILE.into(),
                state: "ready".into(),
                cache_key: Some("sync-bench-thumb.webp".into()),
                width: Some(320),
                height: Some(180),
                byte_size: Some(4),
                error: None,
                source_fingerprint: Some("bench".into()),
            })
            .unwrap();
        database
            .upsert_thumb_blob(ThumbBlobRecord {
                file_id: first,
                profile: crate::thumbnails::GRID_320_PROFILE.into(),
                data: vec![1, 2, 3, 4],
                width: 320,
                height: 180,
                byte_size: 4,
                output_format: "image/webp".into(),
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        (root_id, folder_id, first)
    }

    /// Invoked by tools/bench/sync-protocol-bench.mjs. This intentionally uses
    /// the production coordinator turn, DbWriter durability, browse query and
    /// dormant runtime rather than the pure reference model.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "invoked by tools/bench/sync-protocol-bench.mjs"]
    async fn sync_protocol_acceptance_probe() {
        let base = std::env::temp_dir().join(format!("mangoo-sync-bench-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&base).unwrap();
        let host = Arc::new(HostRegistry::open(base.join("host.sqlite")).unwrap());
        let leases = RuntimeLeases::default();

        // Eight runtime descriptors are intentionally retained while idle. A
        // DormantSyncRuntime owns only paths and the shared lease map; the DB,
        // writer, cache and mmap disappear at the end of each bounded turn.
        let resource_before = process_metrics();
        let mut dormant = Vec::new();
        for index in 0..8 {
            let library = uuid::Uuid::from_u128(100 + index as u128);
            let library_dir = base.join(format!("library-{index}"));
            std::fs::create_dir_all(&library_dir).unwrap();
            let path = library_dir.join("catalog.sqlite");
            let descriptor = LibraryDescriptor {
                library_uuid: library.to_string(),
                root_uuid: uuid::Uuid::from_u128(200 + index as u128).to_string(),
                name: format!("Dormant {index}"),
                icon: None,
                color: None,
                local_data_dir: library_dir.clone(),
                managed_root: library_dir.join("managed"),
                lifecycle_state: "ready".into(),
                binding_state: "local-only".into(),
                availability: "available".into(),
                marker_identity: format!("marker-{index}"),
            };
            let creation = uuid::Uuid::now_v7().to_string();
            host.begin_creation(&CreationIntent {
                txn_uuid: creation.clone(),
                descriptor: descriptor.clone(),
                requested_root: descriptor.managed_root.clone(),
                staging_dir: library_dir.join("staging"),
            })
            .unwrap();
            host.commit_creation(&creation, &descriptor).unwrap();
            host.register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: &format!("benchmark-{index}"),
                version: "1.0.0",
                manifest_json: r#"{"manifestVersion":2}"#,
                entry_relative_path: "bin/provider",
                entry_sha256: &[9; 32],
                install_root: &base,
                trust_policy_id: "benchmark",
                requested_permissions_json: "[]",
            })
            .unwrap();
            host.upsert_plugin_account(
                &format!("local-{index}"),
                &format!("benchmark-{index}"),
                Some("Benchmark"),
                "authenticated",
            )
            .unwrap();
            host.set_plugin_intent(&format!("benchmark-{index}"), true, true)
                .unwrap();
            host.record_plugin_runtime_ready(
                &format!("benchmark-{index}"),
                &format!("benchmark-session-{index}"),
                &["sync-provider.v1".into()],
            )
            .unwrap();
            let (binding, _) = host
                .begin_sync_binding(
                    &descriptor.library_uuid,
                    &format!("benchmark-{index}"),
                    &format!("local-{index}"),
                )
                .unwrap();
            host.set_sync_capture_generation(&descriptor.library_uuid, binding, 1)
                .unwrap();
            let database = Database::open(&path).unwrap();
            database.apply_migrations().unwrap();
            database
                .add_root(NewRoot {
                    path: descriptor.managed_root.to_string_lossy().into_owned(),
                    display_name: descriptor.name.clone(),
                })
                .unwrap();
            database
                .initialize_sync_identity(
                    library,
                    uuid::Uuid::from_u128(300 + index as u128),
                    uuid::Uuid::from_u128(400 + index as u128),
                )
                .unwrap();
            let mut event = crate::sync::test_model::event(
                1,
                50_000 + index as u128,
                Operation::SetField {
                    field: "rating".into(),
                    value: 1.into(),
                },
            );
            event.library_uuid = crate::sync::types::LibraryUuid(library);
            event = crate::sync::codec::seal_event(event).unwrap();
            let segment = seal_segment(vec![event], None).unwrap();
            crate::sync::spool::intake_segment(
                &library_dir.join("sync-spool"),
                &serde_json::to_vec(&segment).unwrap(),
                None,
                |_| Ok(()),
            )
            .unwrap();
            drop(database);
            dormant.push(DormantSyncRuntime::new(
                library.to_string(),
                path,
                leases.clone(),
            ));
        }
        let resource_after = process_metrics();
        let dormant_private_mib = resource_after
            .private_bytes
            .saturating_sub(resource_before.private_bytes) as f64
            / 1_048_576.0;
        let coordinator_before = process_metrics();
        // Keep the real production scheduler running throughout intake/apply
        // and the idle observation window. Its only benchmark-specific input
        // is no benchmark-specific scheduling path: the same five-second
        // cadence, fairness, leases, bounded concurrency and writer lifecycle
        // used by the Host process remain active here.
        let dormant_coordinator = SyncCoordinator::new(Arc::clone(&host), leases.clone());
        let dormant_scheduler = dormant_coordinator.spawn_scheduler(
            PRODUCTION_SYNC_POLL_INTERVAL,
            PRODUCTION_SYNC_CONCURRENCY,
            || None,
        );
        let scheduler_deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let applied = dormant
                .iter()
                .filter(|runtime| {
                    Database::open(&runtime.catalog_path)
                        .and_then(|database| {
                            database.connection_for_fsops().query_row(
                                "SELECT EXISTS(SELECT 1 FROM sync_actor_frontiers WHERE applied_through_seq>=1)",
                                [],
                                |row| row.get::<_, i64>(0),
                            ).map_err(Into::into)
                        })
                        .is_ok_and(|value| value == 1)
                })
                .count();
            if applied == 8 {
                break;
            }
            assert!(
                std::time::Instant::now() < scheduler_deadline,
                "production scheduler did not fairly service all eight Libraries"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let coordinator_after = process_metrics();
        let coordinator_cpu_ms = (coordinator_after.cpu_ms - coordinator_before.cpu_ms).max(0.0);
        let idle_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !dormant_scheduler.is_idle() {
            assert!(
                std::time::Instant::now() < idle_deadline,
                "production scheduler did not settle before idle measurement"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let idle_before = process_metrics();
        let idle_started = std::time::Instant::now();
        // Windows accounts process CPU in coarse quanta (typically 31.25 ms).
        // A sub-second sample therefore reports a single accounting tick as
        // more than 4% even when the scheduler remains dormant. Sample for
        // four seconds, still below the production five-second poll interval,
        // so the <1% idle contract is measurable without including a turn.
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        let idle_after = process_metrics();
        let idle_cpu_percent = ((idle_after.cpu_ms - idle_before.cpu_ms).max(0.0)
            / idle_started.elapsed().as_secs_f64()
            / 10.0)
            .min(100.0);
        assert!(dormant.len() == 8);
        dormant_scheduler.shutdown().await.unwrap();

        let path = base.join("production.sqlite");
        let library_uuid = uuid::Uuid::from_u128(1).to_string();
        let descriptor = LibraryDescriptor {
            library_uuid: library_uuid.clone(),
            root_uuid: uuid::Uuid::from_u128(10).to_string(),
            name: "Sync benchmark".into(),
            icon: None,
            color: None,
            local_data_dir: base.clone(),
            managed_root: base.join("managed"),
            lifecycle_state: "ready".into(),
            binding_state: "local-only".into(),
            availability: "available".into(),
            marker_identity: "sync-benchmark-marker".into(),
        };
        let creation_txn = uuid::Uuid::now_v7().to_string();
        host.begin_creation(&CreationIntent {
            txn_uuid: creation_txn.clone(),
            descriptor: descriptor.clone(),
            requested_root: descriptor.managed_root.clone(),
            staging_dir: base.join("staging"),
        })
        .unwrap();
        host.commit_creation(&creation_txn, &descriptor).unwrap();
        let activation_txn = uuid::Uuid::now_v7().to_string();
        let revision = host
            .begin_activation(&activation_txn, Some(&library_uuid))
            .unwrap();
        host.commit_activation(&activation_txn, Some(&library_uuid), revision)
            .unwrap();
        let database = Database::open(&path).unwrap();
        database.apply_migrations().unwrap();
        let asset_count = std::env::var("MANGOO_SYNC_BENCH_ASSETS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1_000_000);
        let root_id = database
            .add_root(NewRoot {
                path: "D:/SyncBench".into(),
                display_name: "SyncBench".into(),
            })
            .unwrap();
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: "Bench".into(),
                path_hash: "sync-bench".into(),
                mtime: Some(1),
            })
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::from_u128(1),
                uuid::Uuid::from_u128(2),
                uuid::Uuid::from_u128(4),
            )
            .unwrap();
        database.enable_fake_sync_capture_for_test().unwrap();

        // Build the seed through the production checkpoint pipeline before the
        // million-row performance fixture. The million fixture is for hot-path
        // query and concurrent apply measurement, not checkpoint throughput.
        let seed_writer = DbWriter::spawn(database);
        let coordinator = SyncCoordinator::new(Arc::clone(&host), leases.clone());
        assert!(
            coordinator
                .run_active_turn(&library_uuid, &path, &seed_writer)
                .await
                .unwrap()
                .checkpoint_built
        );
        seed_writer.shutdown().await.unwrap();
        let database = Database::open(&path).unwrap();
        let (_, _, thumb_file_id) =
            seed_browse_fixture(&database, asset_count, Some((root_id, folder_id)));

        // Plans must remain local projection/index reads and may not touch the
        // sync ledger. Provider calls are structurally impossible on these DB
        // APIs; count the only provider boundary exercised by this probe.
        let browse_plan = explain_details(database.connection_for_fsops(),
            "SELECT id FROM files INDEXED BY idx_files_root_status_mtime_id WHERE root_id=1 AND status='active' ORDER BY mtime DESC,id DESC LIMIT 101");
        let search_plan = explain_details(database.connection_for_fsops(),
            "SELECT files.id FROM files,media_fts WHERE media_fts.rowid=files.id AND media_fts MATCH 'bench' LIMIT 100");
        let thumb_plan = explain_details(
            database.connection_for_fsops(),
            "SELECT b.data FROM thumb_blobs b WHERE b.file_id=1 AND b.profile='grid_320'",
        );
        let joined_plan = browse_plan
            .iter()
            .chain(search_plan.iter())
            .chain(thumb_plan.iter())
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            browse_plan
                .iter()
                .any(|v| v.contains("idx_files_root_status_mtime_id")),
            "browse plan: {browse_plan:?}"
        );
        assert!(
            search_plan.iter().any(|v| v.contains("media_fts")),
            "search plan: {search_plan:?}"
        );
        assert!(
            thumb_plan
                .iter()
                .any(|v| v.contains("sqlite_autoindex_thumb_blobs") || v.contains("PRIMARY KEY")),
            "thumb plan: {thumb_plan:?}"
        );
        assert!(
            !joined_plan.iter().any(|v| v.contains("sync_")),
            "hot query joined sync tables: {joined_plan:?}"
        );

        let provider = CountingProvider::default();
        let hot_calls_before = provider.calls();
        database
            .list_media_page(MediaPageQuery {
                root_id: None,
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "mtime_desc".into(),
                kind: None,
            })
            .unwrap();
        database
            .search_media_page(SearchQuery {
                q: None,
                root_id: None,
                folder_id: None,
                include_descendants: false,
                kind: None,
                min_rating: None,
                favorite: None,
                tag_ids: vec![],
                sort: "relevance".into(),
                limit: 10,
                offset: None,
                cursor: None,
            })
            .unwrap();
        assert!(database
            .get_thumb_blob(thumb_file_id, crate::thumbnails::GRID_320_PROFILE)
            .unwrap()
            .is_some());
        let hot_provider_calls = provider.calls() - hot_calls_before;
        assert_eq!(
            hot_provider_calls, 0,
            "browse/search/thumb reached provider boundary"
        );
        let sample_hot_paths = |reader: &Database| {
            let started = std::time::Instant::now();
            reader
                .list_media_page(MediaPageQuery {
                    root_id: None,
                    folder_id: None,
                    include_descendants: false,
                    limit: 100,
                    offset: Some(1),
                    cursor: None,
                    sort: "mtime_desc".into(),
                    kind: None,
                })
                .unwrap();
            let browse_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = std::time::Instant::now();
            reader
                .search_media_page(SearchQuery {
                    q: None,
                    root_id: None,
                    folder_id: None,
                    include_descendants: false,
                    kind: None,
                    min_rating: None,
                    favorite: None,
                    tag_ids: vec![],
                    sort: "relevance".into(),
                    limit: 100,
                    offset: Some(1),
                    cursor: None,
                })
                .unwrap();
            let search_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = std::time::Instant::now();
            assert!(reader
                .get_thumb_blob(thumb_file_id, crate::thumbnails::GRID_320_PROFILE)
                .unwrap()
                .is_some());
            let thumb_ms = started.elapsed().as_secs_f64() * 1000.0;
            (browse_ms, search_ms, thumb_ms)
        };
        for _ in 0..20 {
            let _ = sample_hot_paths(&database);
        }
        let mut baseline_browse_ms = Vec::new();
        let mut baseline_search_ms = Vec::new();
        let mut baseline_thumb_ms = Vec::new();
        for _ in 0..500 {
            let sample = sample_hot_paths(&database);
            baseline_browse_ms.push(sample.0);
            baseline_search_ms.push(sample.1);
            baseline_thumb_ms.push(sample.2);
        }
        let stress_before = process_metrics();
        let stress_started = std::time::Instant::now();
        let writer = DbWriter::spawn(database);
        let mut local_capture_ms = Vec::new();
        let capture_started = std::time::Instant::now();
        const CAPTURE_BATCHES: usize = 260;
        const CAPTURE_WARMUP_BATCHES: usize = 10;
        const CAPTURE_EDITS_PER_BATCH: usize = 4;
        const CAPTURE_ITERATIONS: usize = CAPTURE_BATCHES * CAPTURE_EDITS_PER_BATCH;
        const CAPTURE_WARMUP: usize = CAPTURE_WARMUP_BATCHES * CAPTURE_EDITS_PER_BATCH;
        for batch in 0..CAPTURE_BATCHES {
            let started = std::time::Instant::now();
            writer
                .write_authoritative(move |db| {
                    for offset in 0..CAPTURE_EDITS_PER_BATCH {
                        let index = batch * CAPTURE_EDITS_PER_BATCH + offset;
                        db.upsert_user_metadata_partial(
                            thumb_file_id,
                            UserMetadataPatch {
                                rating: None,
                                favorite: Some(index % 2 != 0),
                                note: None,
                            },
                        )?;
                    }
                    Ok(())
                })
                .await
                .unwrap();
            if batch >= CAPTURE_WARMUP_BATCHES {
                let per_edit_ms =
                    started.elapsed().as_secs_f64() * 1000.0 / CAPTURE_EDITS_PER_BATCH as f64;
                local_capture_ms.extend(std::iter::repeat_n(per_edit_ms, CAPTURE_EDITS_PER_BATCH));
            }
            // Bounded writer turns preserve interactive cadence while
            // amortizing scheduler/channel overhead across a short edit burst.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let capture_wall_ms = capture_started.elapsed().as_secs_f64() * 1000.0;
        let coordinator = SyncCoordinator::new(host, leases);
        let spool = base.join("sync-spool");
        std::fs::create_dir_all(&spool).unwrap();
        let mut apply_ms = Vec::new();
        let mut max_events = 0usize;
        let mut max_decoded = 0u64;
        const TURNS: u64 = 50;
        const EVENTS_PER_TURN: u64 = 10;
        for segment_index in 0..TURNS {
            let first_seq = segment_index * EVENTS_PER_TURN + 1;
            let events = (first_seq..first_seq + EVENTS_PER_TURN)
                .map(|seq| {
                    crate::sync::test_model::event(
                        seq,
                        10_000 + seq as u128,
                        Operation::SetField {
                            field: "rating".into(),
                            value: (seq % 5).into(),
                        },
                    )
                })
                .collect::<Vec<_>>();
            let segment = seal_segment(events, None).unwrap();
            max_decoded = max_decoded.max(segment.header.decoded_bytes);
            provider.push(serde_json::to_vec(&segment).unwrap());
            crate::sync::provider::intake_provider_segments(&provider, &spool).unwrap();
            let started = std::time::Instant::now();
            let result = coordinator
                .run_active_turn(&library_uuid, &path, &writer)
                .await
                .unwrap();
            let _coordinator_turn_ms = started.elapsed().as_secs_f64() * 1000.0;
            apply_ms.push(result.apply_transaction_micros as f64 / 1000.0);
            max_events = max_events.max(result.applied_events);
            assert_eq!(result.applied_events, EVENTS_PER_TURN as usize);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        // Exercise the hard event budget once without mixing that saturation
        // sample into the ordinary-turn latency distribution.
        let first_seq = TURNS * EVENTS_PER_TURN + 1;
        let budget_events = (first_seq..first_seq + 100)
            .map(|seq| {
                crate::sync::test_model::event(
                    seq,
                    20_000 + seq as u128,
                    Operation::SetField {
                        field: "rating".into(),
                        value: (seq % 5).into(),
                    },
                )
            })
            .collect::<Vec<_>>();
        let budget_segment = seal_segment(budget_events, None).unwrap();
        max_decoded = max_decoded.max(budget_segment.header.decoded_bytes);
        provider.push(serde_json::to_vec(&budget_segment).unwrap());
        crate::sync::provider::intake_provider_segments(&provider, &spool).unwrap();
        let budget_result = coordinator
            .run_active_turn(&library_uuid, &path, &writer)
            .await
            .unwrap();
        max_events = max_events.max(budget_result.applied_events);
        assert_eq!(budget_result.applied_events, 100);
        let stress_after = process_metrics();
        let stress_cpu_percent = ((stress_after.cpu_ms - stress_before.cpu_ms).max(0.0)
            / stress_started.elapsed().as_secs_f64()
            / 10.0)
            .min(100.0);
        let stress_private_mib = stress_after.private_bytes as f64 / 1_048_576.0;
        let stressed_reader = Database::open(&path).unwrap();
        for _ in 0..20 {
            let _ = sample_hot_paths(&stressed_reader);
        }
        let mut browse_samples = Vec::new();
        let mut search_samples = Vec::new();
        let mut thumb_samples = Vec::new();
        for _ in 0..500 {
            let sample = sample_hot_paths(&stressed_reader);
            browse_samples.push(sample.0);
            search_samples.push(sample.1);
            thumb_samples.push(sample.2);
        }
        writer.shutdown().await.unwrap();

        // Reopen after every durable writer has been joined and prove the
        // ledger/frontier survived the close boundary.
        let reopened = Database::open(&path).unwrap();
        let (events, frontier): (i64, i64) = reopened.connection_for_fsops().query_row(
            "SELECT (SELECT COUNT(*) FROM sync_events),(SELECT MAX(applied_through_seq) FROM sync_actor_frontiers)", [],
            |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(
            (events, frontier),
            (CAPTURE_ITERATIONS as i64 + 600, CAPTURE_ITERATIONS as i64)
        );
        assert!(max_events <= super::super::inbox::MAX_APPLY_EVENTS);
        assert!(max_decoded <= super::super::inbox::MAX_APPLY_DECODED_BYTES as u64);

        let apply_p95 = percentile(apply_ms.clone(), 0.95);
        let apply_p99 = percentile(apply_ms, 0.99);
        let browse = browse_samples;
        let search = search_samples;
        let thumb = thumb_samples;
        assert!(
            !browse.is_empty(),
            "contention probe collected no browse samples"
        );
        let browse_p95 = percentile(browse.clone(), 0.95);
        let browse_p99 = percentile(browse.clone(), 0.99);
        let search_p95 = percentile(search.clone(), 0.95);
        let search_p99 = percentile(search, 0.99);
        let thumb_p95 = percentile(thumb.clone(), 0.95);
        let thumb_p99 = percentile(thumb, 0.99);
        let baseline_browse_p50 = percentile(baseline_browse_ms.clone(), 0.50);
        let baseline_browse_p95 = percentile(baseline_browse_ms.clone(), 0.95);
        let baseline_browse_p99 = percentile(baseline_browse_ms, 0.99);
        let baseline_search_p50 = percentile(baseline_search_ms.clone(), 0.50);
        let baseline_search_p95 = percentile(baseline_search_ms.clone(), 0.95);
        let baseline_search_p99 = percentile(baseline_search_ms, 0.99);
        let baseline_thumb_p50 = percentile(baseline_thumb_ms.clone(), 0.50);
        let baseline_thumb_p95 = percentile(baseline_thumb_ms.clone(), 0.95);
        let baseline_thumb_p99 = percentile(baseline_thumb_ms, 0.99);
        // Sub-millisecond Windows samples have quantization/scheduler jitter
        // larger than the percentage budget itself. The product contract also
        // has absolute 50/100 ms browse ceilings, so use a one-millisecond
        // measurement floor for relative comparisons without weakening those
        // absolute limits.
        let percent_regression = |baseline: f64, stress: f64| {
            let baseline = baseline.max(1.0);
            ((stress - baseline) / baseline) * 100.0
        };
        let browse_p95_regression = percent_regression(baseline_browse_p95, browse_p95);
        let browse_p99_regression = percent_regression(baseline_browse_p99, browse_p99);
        let search_p95_regression = percent_regression(baseline_search_p95, search_p95);
        let search_p99_regression = percent_regression(baseline_search_p99, search_p99);
        let thumb_p95_regression = percent_regression(baseline_thumb_p95, thumb_p95);
        let local_capture_p95 = percentile(local_capture_ms.clone(), 0.95);
        let local_capture_p99 = percentile(local_capture_ms, 0.99);
        assert_eq!(
            provider.calls(),
            TURNS + 1,
            "fake provider boundary was not exercised"
        );
        println!("SYNC_BENCH_JSON={{\"libraries\":8,\"assets\":{asset_count},\"captureWarmupSamples\":{CAPTURE_WARMUP},\"captureSamples\":{},\"captureWallMs\":{capture_wall_ms:.3},\"captureThroughputPerSecond\":{:.3},\"applyP95Ms\":{apply_p95:.3},\"applyP99Ms\":{apply_p99:.3},\"localCaptureP95Ms\":{local_capture_p95:.3},\"localCaptureP99Ms\":{local_capture_p99:.3},\"maxAppliedEvents\":{max_events},\"maxDecodedBytes\":{max_decoded},\"idleCpuPercent\":{idle_cpu_percent:.3},\"coordinatorCpuMs\":{coordinator_cpu_ms:.3},\"dormantPrivateMiB\":{dormant_private_mib:.3},\"stressCpuPercent\":{stress_cpu_percent:.3},\"stressPrivateMiB\":{stress_private_mib:.3},\"baselineBrowseP50Ms\":{baseline_browse_p50:.3},\"baselineBrowseP95Ms\":{baseline_browse_p95:.3},\"baselineBrowseP99Ms\":{baseline_browse_p99:.3},\"baselineSearchP50Ms\":{baseline_search_p50:.3},\"baselineSearchP95Ms\":{baseline_search_p95:.3},\"baselineSearchP99Ms\":{baseline_search_p99:.3},\"baselineThumbP50Ms\":{baseline_thumb_p50:.3},\"baselineThumbP95Ms\":{baseline_thumb_p95:.3},\"baselineThumbP99Ms\":{baseline_thumb_p99:.3},\"browseP95Ms\":{browse_p95:.3},\"browseP99Ms\":{browse_p99:.3},\"browseP95RegressionPercent\":{browse_p95_regression:.3},\"browseP99RegressionPercent\":{browse_p99_regression:.3},\"searchP95Ms\":{search_p95:.3},\"searchP99Ms\":{search_p99:.3},\"searchP95RegressionPercent\":{search_p95_regression:.3},\"searchP99RegressionPercent\":{search_p99_regression:.3},\"thumbP95Ms\":{thumb_p95:.3},\"thumbP99Ms\":{thumb_p99:.3},\"thumbP95RegressionPercent\":{thumb_p95_regression:.3},\"providerCalls\":{hot_provider_calls},\"providerIntakeCalls\":{},\"reopenEvents\":{events},\"reopenFrontier\":{frontier}}}", CAPTURE_ITERATIONS-CAPTURE_WARMUP, 1000.0 / (capture_wall_ms / CAPTURE_ITERATIONS as f64), provider.calls());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    #[ignore = "subprocess helper for abrupt kill probe"]
    fn sync_protocol_kill_child() {
        let Some(path) = std::env::var_os("MANGOO_SYNC_KILL_DB").map(PathBuf::from) else {
            return;
        };
        let marker = PathBuf::from(std::env::var_os("MANGOO_SYNC_KILL_MARKER").unwrap());
        let stop_at = std::env::var("MANGOO_SYNC_KILL_STOP_AT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok());
        let database = Database::open(&path).unwrap();
        database.apply_migrations().unwrap();
        if database.list_roots().unwrap().is_empty() {
            database
                .add_root(NewRoot {
                    path: "D:/KillProbe".into(),
                    display_name: "KillProbe".into(),
                })
                .unwrap();
        }
        database
            .initialize_sync_identity(
                uuid::Uuid::from_u128(1),
                uuid::Uuid::from_u128(2),
                uuid::Uuid::from_u128(4),
            )
            .unwrap();
        let current: u64 = database
            .connection_for_fsops()
            .query_row(
                "SELECT COALESCE(MAX(applied_through_seq),0) FROM sync_actor_frontiers",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
            .max(0) as u64;
        for first in ((current + 1)..=100_000).step_by(100) {
            let last = (first + 99).min(100_000);
            let events = (first..=last)
                .map(|seq| {
                    crate::sync::test_model::event(
                        seq,
                        1_000_000 + seq as u128,
                        Operation::SetField {
                            field: "rating".into(),
                            value: (seq % 5).into(),
                        },
                    )
                })
                .collect::<Vec<_>>();
            database.apply_remote_sync_events(&events).unwrap();
            if stop_at.is_some_and(|stop| last >= stop) {
                use std::io::Write;
                let mut file = std::fs::File::create(&marker).unwrap();
                file.write_all(last.to_string().as_bytes()).unwrap();
                file.sync_all().unwrap();
                loop {
                    std::thread::park_timeout(std::time::Duration::from_secs(1));
                }
            }
        }
    }

    #[test]
    #[ignore = "subprocess helper for deterministic sync fault matrix"]
    fn sync_protocol_fault_child() {
        let Some(path) = std::env::var_os("MANGOO_SYNC_FAULT_DB").map(PathBuf::from) else {
            return;
        };
        let scenario = std::env::var("MANGOO_SYNC_FAULT_SCENARIO").unwrap();
        let database = Database::open(&path).unwrap();
        database.apply_migrations().unwrap();
        let root = database
            .add_root(NewRoot {
                path: path.parent().unwrap().to_string_lossy().into_owned(),
                display_name: "Fault".into(),
            })
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::from_u128(1),
                uuid::Uuid::from_u128(2),
                uuid::Uuid::from_u128(4),
            )
            .unwrap();
        match scenario.as_str() {
            "capture" | "publication" => {
                let folder = database
                    .upsert_folder(crate::db::FolderUpsert {
                        root_id: root,
                        parent_id: None,
                        name: String::new(),
                        path_hash: "fault-folder".into(),
                        mtime: Some(1),
                    })
                    .unwrap();
                let file = database
                    .upsert_file(FileUpsert {
                        root_id: root,
                        folder_id: folder,
                        name: "fault.jpg".into(),
                        ext: ".jpg".into(),
                        size: 1,
                        mtime: 1,
                        ctime: None,
                        file_key: Some("fault".into()),
                    })
                    .unwrap();
                database.enable_fake_sync_capture_for_test().unwrap();
                if scenario == "capture" {
                    database
                        .upsert_user_metadata_partial(
                            file,
                            UserMetadataPatch {
                                rating: None,
                                favorite: Some(true),
                                note: None,
                            },
                        )
                        .unwrap();
                } else {
                    let intent = database
                        .create_sync_publication_intent(
                            file,
                            &path.parent().unwrap().join("fault.jpg").to_string_lossy(),
                            1,
                        )
                        .unwrap();
                    database.bind_fake_sync_manifest(intent, [7; 32]).unwrap();
                }
            }
            "media-publication" => {
                let source = path.parent().unwrap().join("fault-media.jpg");
                let source_bytes = vec![0x5a; 3 * 1024 * 1024];
                std::fs::write(&source, &source_bytes).unwrap();
                let source_mtime = std::fs::metadata(&source)
                    .unwrap()
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                let folder = database
                    .upsert_folder(crate::db::FolderUpsert {
                        root_id: root,
                        parent_id: None,
                        name: String::new(),
                        path_hash: "fault-media-folder".into(),
                        mtime: Some(source_mtime),
                    })
                    .unwrap();
                let file = database
                    .upsert_file(FileUpsert {
                        root_id: root,
                        folder_id: folder,
                        name: "fault-media.jpg".into(),
                        ext: ".jpg".into(),
                        size: source_bytes.len() as i64,
                        mtime: source_mtime,
                        ctime: None,
                        file_key: Some("fault-media".into()),
                    })
                    .unwrap();
                let binding = uuid::Uuid::from_u128(5);
                database
                    .begin_sync_capture(binding, uuid::Uuid::from_u128(6))
                    .unwrap();
                database
                    .create_sync_publication_intent(
                        file,
                        &source.to_string_lossy(),
                        source_bytes.len() as i64,
                    )
                    .unwrap();
                let writer = DbWriter::spawn(database);
                let worker = crate::sync::media::worker::MediaPublicationWorker::new(
                    writer.clone(),
                    Arc::new(crate::sync::provider::InMemorySyncProvider::default()),
                    path.parent().unwrap().join("media-objects"),
                    crate::sync::provider::TransportContext {
                        namespace: uuid::Uuid::now_v7().to_string(),
                        binding_epoch: binding.to_string(),
                        transport_generation: 1,
                    },
                    crate::sync::media::worker::MediaWorkLimiter::new(1, 8, 128 * 1024 * 1024)
                        .unwrap(),
                );
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async move {
                        worker.run_once().await.unwrap();
                        writer.shutdown().await.unwrap();
                    });
            }
            "media-materialization" => {
                let managed_root = path.parent().unwrap().join("managed-root");
                std::fs::create_dir_all(&managed_root).unwrap();
                let range_path = path.parent().unwrap().join("verified.range");
                let bytes = b"verified";
                std::fs::write(&range_path, bytes).unwrap();
                let chunk_hash = crate::sync::codec::sha256(bytes);
                let manifest = crate::sync::media::format::ContentManifestV1 {
                    format_version: crate::sync::media::MEDIA_FORMAT_VERSION,
                    byte_length: bytes.len() as u64,
                    content_sha256: crate::sync::media::hex_hash(&chunk_hash),
                    media_hint: crate::sync::media::format::MediaHint {
                        sniffed_kind: Some("image".into()),
                        mime: Some("image/jpeg".into()),
                    },
                    chunking: crate::sync::media::format::ChunkingProfile::default(),
                    chunks: vec![crate::sync::media::format::ManifestChunkV1 {
                        ordinal: 0,
                        chunk_sha256: crate::sync::media::hex_hash(&chunk_hash),
                        byte_length: bytes.len() as u64,
                        pack_sha256: crate::sync::media::hex_hash(&crate::sync::codec::sha256(
                            b"pack",
                        )),
                        pack_offset: 0,
                    }],
                };
                let chunks = std::collections::BTreeMap::from([(
                    chunk_hash,
                    crate::sync::media::recovery::VerifiedChunkSource {
                        path: range_path,
                        offset: 0,
                        length: bytes.len() as u64,
                    },
                )]);
                crate::sync::media::materialize::materialize_from_verified_ranges(
                    &managed_root,
                    Path::new("fault-media.jpg"),
                    uuid::Uuid::from_u128(7),
                    &manifest,
                    &chunks,
                    None,
                )
                .unwrap();
            }
            "remote" => {
                database
                    .apply_remote_sync_events(&[crate::sync::test_model::event(
                        1,
                        999,
                        Operation::SetField {
                            field: "rating".into(),
                            value: 4.into(),
                        },
                    )])
                    .unwrap();
            }
            "spool" => {
                let segment = seal_segment(
                    vec![crate::sync::test_model::event(
                        1,
                        999,
                        Operation::SetField {
                            field: "rating".into(),
                            value: 4.into(),
                        },
                    )],
                    None,
                )
                .unwrap();
                crate::sync::spool::intake_segment(
                    path.parent().unwrap().join("sync-spool").as_path(),
                    &serde_json::to_vec(&segment).unwrap(),
                    None,
                    |record| {
                        crate::sync::spool::register_durable_segment(
                            database.connection_for_fsops(),
                            record,
                        )?;
                        Ok(())
                    },
                )
                .unwrap();
            }
            "bulk" => {
                let operation = uuid::Uuid::now_v7();
                let root = uuid::Uuid::from_u128(700);
                database.connection_for_fsops().execute(
                    "INSERT INTO sync_bulk_operations(bulk_operation_uuid,operation_kind,root_entity_uuid,observed_local_mutation_generation,observed_frontier,state,item_count,created_at,updated_at) VALUES (?1,'delete-subtree',?2,1,?3,'pending',0,1,1)",
                    params![operation.as_bytes().as_slice(),root.as_bytes().as_slice(),serde_json::to_vec(&Vec::<(Vec<u8>,Vec<u8>,Vec<u8>,i64)>::new()).unwrap()],
                ).unwrap();
                let transaction = database
                    .connection_for_fsops()
                    .unchecked_transaction()
                    .unwrap();
                crate::sync::bulk::build_subtree_delete_turn(&transaction, operation, 100).unwrap();
                transaction.commit().unwrap();
            }
            "checkpoint" => {
                let host = crate::sync::checkpoint::HostMetadataSnapshot {
                    library_uuid: uuid::Uuid::from_u128(1).to_string(),
                    name: "Fault".into(),
                    icon: None,
                    binding_state: "synchronized".into(),
                    events: vec![],
                };
                let fence = crate::sync::persistence::capture_checkpoint_fence(
                    database.connection_for_fsops(),
                    Default::default(),
                    host,
                )
                .unwrap();
                crate::sync::persistence::build_checkpoint(
                    database.connection_for_fsops(),
                    &fence,
                    &path.parent().unwrap().join("checkpoints"),
                )
                .unwrap();
            }
            "promotion" => {
                let staging = path.with_extension("staging");
                std::fs::write(&staging, b"validated staging").unwrap();
                crate::sync::persistence::promote_new_staging_catalog(
                    &staging,
                    &path.with_extension("promoted"),
                )
                .unwrap();
            }
            _ => panic!("unknown sync fault scenario"),
        }
    }

    fn count_files_recursively(root: &Path) -> usize {
        std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    count_files_recursively(&entry.path())
                } else {
                    1
                }
            })
            .sum()
    }

    #[test]
    fn process_kill_fault_matrix_preserves_atomic_boundaries_and_recovery() {
        let cases = [
            ("capture", "capture.after-ledger-truth-frontier"),
            ("capture", "capture.after-outbox"),
            ("publication", "publication.after-intent"),
            ("publication", "publication.after-manifest"),
            ("publication", "publication.after-canonicalization"),
            ("media-publication", "media.after-data-seal"),
            ("media-publication", "media.after-manifest-seal"),
            ("media-publication", "media.after-data-receipt"),
            ("media-publication", "media.after-manifest-receipt"),
            ("media-materialization", "media.after-materialization-sync"),
            ("remote", "remote.after-ledger"),
            ("remote", "remote.after-truth-projection"),
            ("remote", "remote.after-frontier"),
            ("spool", "spool.after-durable-publish"),
            ("spool", "spool.after-registration"),
            ("bulk", "bulk.after-page-cursor"),
            ("checkpoint", "checkpoint.after-payload-publish"),
            ("checkpoint", "checkpoint.after-descriptor-watermarks"),
            ("promotion", "checkpoint.before-staging-promotion"),
        ];
        let exe = std::env::current_exe().unwrap();
        for (scenario, point) in cases {
            let base = std::env::temp_dir().join(format!(
                "mangoo-sync-fault-{scenario}-{}",
                uuid::Uuid::now_v7()
            ));
            std::fs::create_dir_all(&base).unwrap();
            let db_path = base.join("catalog.sqlite");
            let marker = base.join("fault.marker");
            let mut child = std::process::Command::new(&exe)
                .args([
                    "--ignored",
                    "--exact",
                    "sync::coordinator::tests::sync_protocol_fault_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("MANGOO_SYNC_FAULT_DB", &db_path)
                .env("MANGOO_SYNC_FAULT_SCENARIO", scenario)
                .env("MANGOO_SYNC_FAULTPOINT", point)
                .env("MANGOO_SYNC_FAULT_MARKER", &marker)
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !marker.exists() && std::time::Instant::now() < deadline {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "fault child exited at {point}"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(marker.exists(), "faultpoint was not reached: {point}");
            child.kill().unwrap();
            child.wait().unwrap();
            if scenario != "promotion" {
                let reopened = Database::open(&db_path).unwrap();
                reopened.apply_migrations().unwrap();
                let connection = reopened.connection_for_fsops();
                match scenario {
                    "capture" => {
                        let counts: (i64, i64, i64) = connection.query_row(
                            "SELECT (SELECT COUNT(*) FROM user_metadata),(SELECT COUNT(*) FROM sync_events),(SELECT COUNT(*) FROM sync_outbox)", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
                        assert_eq!(
                            counts,
                            (0, 0, 0),
                            "partial authoritative capture at {point}"
                        );
                    }
                    "publication" => {
                        let canonical: i64 = connection.query_row("SELECT COUNT(*) FROM sync_publication_intents WHERE state='canonicalized'",[],|row|row.get(0)).unwrap();
                        assert_eq!(canonical, 0, "partial publication at {point}");
                    }
                    "media-publication" => {
                        let (durable, canonical_events): (i64, i64) = connection
                            .query_row(
                                "SELECT (SELECT COUNT(*) FROM media_publication_objects WHERE state='durable'),(SELECT COUNT(*) FROM sync_events)",
                                [],
                                |row| Ok((row.get(0)?, row.get(1)?)),
                            )
                            .unwrap();
                        let expected_durable = match point {
                            "media.after-data-receipt" => 1,
                            "media.after-manifest-receipt" => 2,
                            _ => 0,
                        };
                        assert_eq!(durable, expected_durable, "receipt durability at {point}");
                        assert_eq!(canonical_events, 0, "media event escaped early at {point}");
                        let expected_files = match point {
                            "media.after-data-seal" => 1,
                            "media.after-manifest-seal"
                            | "media.after-data-receipt"
                            | "media.after-manifest-receipt" => 2,
                            _ => unreachable!(),
                        };
                        assert_eq!(
                            count_files_recursively(&base.join("media-objects")),
                            expected_files,
                            "sealed object publication boundary at {point}"
                        );
                    }
                    "media-materialization" => {
                        let managed_root = base.join("managed-root");
                        let staging_prefix =
                            format!(".mangoo-work-{}-", uuid::Uuid::from_u128(7).simple());
                        let staging_directories = std::fs::read_dir(&managed_root)
                            .unwrap()
                            .map(|entry| entry.unwrap())
                            .filter(|entry| {
                                entry.file_type().unwrap().is_dir()
                                    && entry
                                        .file_name()
                                        .to_string_lossy()
                                        .starts_with(&staging_prefix)
                            })
                            .map(|entry| entry.path())
                            .collect::<Vec<_>>();
                        assert_eq!(
                            staging_directories.len(),
                            1,
                            "expected one owned staging directory at {point}"
                        );
                        let staging = staging_directories[0].join("payload.partial");
                        let target = managed_root.join("fault-media.jpg");
                        assert_eq!(
                            std::fs::read(&staging).unwrap(),
                            b"verified",
                            "verified staging was not durable at {point}"
                        );
                        assert!(!target.exists(), "partial media target escaped at {point}");
                        let range_path = base.join("verified.range");
                        let bytes = b"verified";
                        let chunk_hash = crate::sync::codec::sha256(bytes);
                        let manifest = crate::sync::media::format::ContentManifestV1 {
                            format_version: crate::sync::media::MEDIA_FORMAT_VERSION,
                            byte_length: bytes.len() as u64,
                            content_sha256: crate::sync::media::hex_hash(&chunk_hash),
                            media_hint: crate::sync::media::format::MediaHint {
                                sniffed_kind: Some("image".into()),
                                mime: Some("image/jpeg".into()),
                            },
                            chunking: crate::sync::media::format::ChunkingProfile::default(),
                            chunks: vec![crate::sync::media::format::ManifestChunkV1 {
                                ordinal: 0,
                                chunk_sha256: crate::sync::media::hex_hash(&chunk_hash),
                                byte_length: bytes.len() as u64,
                                pack_sha256: crate::sync::media::hex_hash(
                                    &crate::sync::codec::sha256(b"pack"),
                                ),
                                pack_offset: 0,
                            }],
                        };
                        let chunks = std::collections::BTreeMap::from([(
                            chunk_hash,
                            crate::sync::media::recovery::VerifiedChunkSource {
                                path: range_path,
                                offset: 0,
                                length: bytes.len() as u64,
                            },
                        )]);
                        let result =
                            crate::sync::media::materialize::materialize_from_verified_ranges(
                                &managed_root,
                                Path::new("fault-media.jpg"),
                                uuid::Uuid::from_u128(7),
                                &manifest,
                                &chunks,
                                None,
                            )
                            .unwrap();
                        assert!(matches!(
                            result,
                            crate::sync::media::materialize::MaterializationResult::Published(_)
                        ));
                        assert_eq!(std::fs::read(target).unwrap(), bytes);
                    }
                    "remote" => {
                        let counts: (i64, i64) = connection.query_row("SELECT (SELECT COUNT(*) FROM sync_events),(SELECT COUNT(*) FROM sync_actor_frontiers)",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
                        assert_eq!(counts, (0, 0), "partial remote apply at {point}");
                    }
                    "spool" => {
                        let files = std::fs::read_dir(base.join("sync-spool"))
                            .unwrap()
                            .filter_map(Result::ok)
                            .filter(|entry| {
                                entry.path().extension().and_then(|v| v.to_str()) == Some("json")
                            })
                            .count();
                        assert_eq!(files, 1);
                        register_unindexed_segments(connection, &base.join("sync-spool")).unwrap();
                        register_unindexed_segments(connection, &base.join("sync-spool")).unwrap();
                        let registered: i64 = connection
                            .query_row("SELECT COUNT(*) FROM sync_inbox_segments", [], |row| {
                                row.get(0)
                            })
                            .unwrap();
                        assert_eq!(
                            registered, 1,
                            "spool recovery was not idempotent at {point}"
                        );
                    }
                    "bulk" => {
                        let counts: (i64, i64) = connection.query_row("SELECT (SELECT COUNT(*) FROM sync_bulk_items),(SELECT COUNT(*) FROM sync_bulk_operations WHERE cursor_entity_uuid IS NOT NULL)",[],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
                        assert_eq!(counts, (0, 0), "partial bulk page at {point}");
                    }
                    "checkpoint" => {
                        crate::sync::persistence::recover_interrupted_checkpoints(connection)
                            .unwrap();
                        let partial: i64 = connection.query_row("SELECT COUNT(*) FROM sync_checkpoints WHERE state='ready' AND NOT EXISTS (SELECT 1 FROM sync_checkpoint_watermarks w WHERE w.checkpoint_uuid=sync_checkpoints.checkpoint_uuid)",[],|row|row.get(0)).unwrap();
                        assert_eq!(partial, 0, "partial checkpoint descriptor at {point}");
                    }
                    _ => unreachable!(),
                }
            } else {
                assert!(db_path.with_extension("staging").exists());
                assert!(!db_path.with_extension("promoted").exists());
                crate::sync::persistence::promote_new_staging_catalog(
                    &db_path.with_extension("staging"),
                    &db_path.with_extension("promoted"),
                )
                .unwrap();
                assert!(db_path.with_extension("promoted").exists());
            }
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    #[ignore = "invoked by tools/bench/sync-protocol-bench.mjs"]
    fn sync_protocol_abrupt_kill_replay_probe() {
        let base = std::env::temp_dir().join(format!("mangoo-sync-kill-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&base).unwrap();
        let path = base.join("catalog.sqlite");
        let marker = base.join("committed.marker");
        let exe = std::env::current_exe().unwrap();
        let spawn = |stop: Option<u64>| {
            let mut command = std::process::Command::new(&exe);
            command
                .args([
                    "--ignored",
                    "--exact",
                    "sync::coordinator::tests::sync_protocol_kill_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("MANGOO_SYNC_KILL_DB", &path)
                .env("MANGOO_SYNC_KILL_MARKER", &marker);
            if let Some(stop) = stop {
                command.env("MANGOO_SYNC_KILL_STOP_AT", stop.to_string());
            }
            command.spawn().unwrap()
        };
        let mut child = spawn(Some(50_000));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while !marker.exists() && std::time::Instant::now() < deadline {
            assert!(
                child.try_wait().unwrap().is_none(),
                "kill child exited before marker"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            marker.exists(),
            "kill child did not reach committed frontier"
        );
        child.kill().unwrap();
        child.wait().unwrap();
        let reopened = Database::open(&path).unwrap();
        reopened.apply_migrations().unwrap();
        let (events, frontier): (i64,i64) = reopened.connection_for_fsops().query_row(
            "SELECT (SELECT COUNT(*) FROM sync_events),(SELECT MAX(applied_through_seq) FROM sync_actor_frontiers)", [],
            |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!(events, frontier);
        assert!((50_000..50_100).contains(&events));
        drop(reopened);
        let status = spawn(None).wait().unwrap();
        assert!(status.success());
        let final_db = Database::open(&path).unwrap();
        let (events, frontier): (i64,i64) = final_db.connection_for_fsops().query_row(
            "SELECT (SELECT COUNT(*) FROM sync_events),(SELECT MAX(applied_through_seq) FROM sync_actor_frontiers)", [],
            |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!((events, frontier), (100_000, 100_000));
        println!("SYNC_KILL_JSON={{\"killEvents\":{events},\"killFrontier\":{frontier},\"abruptKill\":1}}");
        let _ = std::fs::remove_dir_all(base);
    }
}
