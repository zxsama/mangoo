use crate::{
    db::{writer::DbWriter, SyncRootBindingToken},
    sync::provider::{SyncProvider, TransportContext},
};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use std::{collections::BTreeMap, path::Path};
use tokio::{sync::Semaphore, task::JoinSet};

const DEFAULT_TEMP_DISK_BUDGET: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MediaBackpressureMetrics {
    pub queued_items: usize,
    pub queued_bytes: u64,
    pub reserved_temp_bytes: u64,
    pub queue_rejections: u64,
    pub disk_rejections: u64,
    pub foreground_yields: u64,
}

#[derive(Default)]
struct MediaWorkCounters {
    queued_items: AtomicUsize,
    queued_bytes: AtomicU64,
    reserved_temp_bytes: AtomicU64,
    queue_rejections: AtomicU64,
    disk_rejections: AtomicU64,
    foreground_yields: AtomicU64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum MediaPriority {
    Background = 0,
    ActiveLibrary = 1,
    Visible = 2,
    UserRequested = 3,
}

#[derive(Clone)]
pub struct MediaWorkLimiter {
    slots: Arc<Semaphore>,
    max_queued_items: usize,
    max_queued_bytes: u64,
    max_temp_bytes: u64,
    cancelled: Arc<AtomicBool>,
    counters: Arc<MediaWorkCounters>,
}

impl MediaWorkLimiter {
    pub fn new(
        concurrency: usize,
        max_queued_items: usize,
        max_queued_bytes: u64,
    ) -> anyhow::Result<Self> {
        Self::new_with_temp_budget(
            concurrency,
            max_queued_items,
            max_queued_bytes,
            DEFAULT_TEMP_DISK_BUDGET,
        )
    }

    pub fn new_with_temp_budget(
        concurrency: usize,
        max_queued_items: usize,
        max_queued_bytes: u64,
        max_temp_bytes: u64,
    ) -> anyhow::Result<Self> {
        if concurrency == 0 || max_queued_items == 0 || max_queued_bytes == 0 || max_temp_bytes == 0
        {
            anyhow::bail!("media work limits must be nonzero");
        }
        Ok(Self {
            slots: Arc::new(Semaphore::new(concurrency)),
            max_queued_items,
            max_queued_bytes,
            max_temp_bytes,
            cancelled: Arc::new(AtomicBool::new(false)),
            counters: Arc::new(MediaWorkCounters::default()),
        })
    }
    pub fn cancellation(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.slots.close();
    }
    pub fn validate_batch(&self, items: usize, bytes: u64) -> anyhow::Result<()> {
        if items > self.max_queued_items || bytes > self.max_queued_bytes {
            self.counters
                .queue_rejections
                .fetch_add(1, Ordering::AcqRel);
            anyhow::bail!("media work batch exceeds bounded queue budget");
        }
        Ok(())
    }
    pub fn metrics(&self) -> MediaBackpressureMetrics {
        MediaBackpressureMetrics {
            queued_items: self.counters.queued_items.load(Ordering::Acquire),
            queued_bytes: self.counters.queued_bytes.load(Ordering::Acquire),
            reserved_temp_bytes: self.counters.reserved_temp_bytes.load(Ordering::Acquire),
            queue_rejections: self.counters.queue_rejections.load(Ordering::Acquire),
            disk_rejections: self.counters.disk_rejections.load(Ordering::Acquire),
            foreground_yields: self.counters.foreground_yields.load(Ordering::Acquire),
        }
    }
    pub fn record_foreground_yield(&self) {
        self.counters
            .foreground_yields
            .fetch_add(1, Ordering::AcqRel);
    }
    pub fn reserve_temporary_disk(
        &self,
        path: &Path,
        bytes: u64,
    ) -> anyhow::Result<MediaTempReservation> {
        let previous = self
            .counters
            .reserved_temp_bytes
            .fetch_add(bytes, Ordering::AcqRel);
        let available = free_space_bytes(path).unwrap_or(0);
        if previous.saturating_add(bytes) > self.max_temp_bytes
            || available
                < previous
                    .saturating_add(bytes)
                    .saturating_add(64 * 1024 * 1024)
        {
            self.counters
                .reserved_temp_bytes
                .fetch_sub(bytes, Ordering::AcqRel);
            self.counters.disk_rejections.fetch_add(1, Ordering::AcqRel);
            tracing::warn!(
                bytes,
                available,
                "media temporary-disk reservation rejected"
            );
            anyhow::bail!("media temporary-disk reservation exceeds bounded budget");
        }
        Ok(MediaTempReservation {
            bytes,
            counters: Arc::clone(&self.counters),
        })
    }

    /// The worker budget bounds the active chunk/pack window. Durable spool or
    /// destination payload is capacity-checked against the actual filesystem,
    /// independently of that window, so a single file can exceed 16 GiB.
    fn reserve_content_work(
        &self,
        path: &Path,
        durable_bytes: u64,
        window_bytes: u64,
    ) -> anyhow::Result<MediaTempReservation> {
        require_publication_capacity(free_space_bytes(path)?, durable_bytes, window_bytes)?;
        self.reserve_temporary_disk(path, window_bytes)
    }
    pub async fn run_bounded<T, F>(
        &self,
        items: usize,
        bytes: u64,
        operation: F,
    ) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    {
        self.validate_batch(items, bytes)?;
        let previous_items = self
            .counters
            .queued_items
            .fetch_add(items, Ordering::AcqRel);
        let previous_bytes = self
            .counters
            .queued_bytes
            .fetch_add(bytes, Ordering::AcqRel);
        if previous_items.saturating_add(items) > self.max_queued_items
            || previous_bytes.saturating_add(bytes) > self.max_queued_bytes
        {
            self.counters
                .queued_items
                .fetch_sub(items, Ordering::AcqRel);
            self.counters
                .queued_bytes
                .fetch_sub(bytes, Ordering::AcqRel);
            self.counters
                .queue_rejections
                .fetch_add(1, Ordering::AcqRel);
            tracing::warn!(items, bytes, "media work queue backpressure rejected work");
            anyhow::bail!("media work queue is at its bounded capacity");
        }
        let queued = MediaQueueReservation {
            items,
            bytes,
            counters: Arc::clone(&self.counters),
        };
        let permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow::anyhow!("media work cancelled"))?;
        if self.cancelled.load(Ordering::Acquire) {
            drop(queued);
            anyhow::bail!("media work cancelled");
        }
        let result = tokio::task::spawn_blocking(operation).await?;
        drop(permit);
        drop(queued);
        result
    }
    pub async fn run<T, F>(&self, operation: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    {
        self.run_bounded(1, 1, operation).await
    }
}

struct MediaQueueReservation {
    items: usize,
    bytes: u64,
    counters: Arc<MediaWorkCounters>,
}
impl Drop for MediaQueueReservation {
    fn drop(&mut self) {
        self.counters
            .queued_items
            .fetch_sub(self.items, Ordering::AcqRel);
        self.counters
            .queued_bytes
            .fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
pub struct MediaTempReservation {
    bytes: u64,
    counters: Arc<MediaWorkCounters>,
}
impl Drop for MediaTempReservation {
    fn drop(&mut self) {
        self.counters
            .reserved_temp_bytes
            .fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[derive(Clone)]
pub struct MediaWorkerSet {
    limiter: MediaWorkLimiter,
    tasks: Arc<tokio::sync::Mutex<JoinSet<()>>>,
}

pub struct MediaRegistrationFence {
    _tasks: tokio::sync::OwnedMutexGuard<JoinSet<()>>,
}

impl MediaWorkerSet {
    pub fn new(limiter: MediaWorkLimiter) -> Self {
        Self {
            limiter,
            tasks: Arc::new(tokio::sync::Mutex::new(JoinSet::new())),
        }
    }
    pub fn limiter(&self) -> MediaWorkLimiter {
        self.limiter.clone()
    }
    pub fn metrics(&self) -> MediaBackpressureMetrics {
        self.limiter.metrics()
    }
    pub fn cancel(&self) {
        self.limiter.cancel();
    }
    pub async fn run<F, T>(&self, future: F) -> anyhow::Result<T>
    where
        F: std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        // Serialize the cancellation check with task registration. Otherwise
        // shutdown can observe an empty JoinSet, return, and let a turn that
        // passed the earlier check spawn after the binding was fenced.
        let mut tasks = self.tasks.lock().await;
        if self.limiter.cancelled.load(Ordering::Acquire) {
            anyhow::bail!("media worker set cancelled");
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tasks.spawn(async move {
            let _ = sender.send(future.await);
        });
        drop(tasks);
        let result = receiver
            .await
            .map_err(|_| anyhow::anyhow!("media worker task cancelled"))?;
        let mut tasks = self.tasks.lock().await;
        while tasks.try_join_next().is_some() {}
        result
    }
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.cancel();
        self.drain().await
    }

    pub async fn drain(&self) -> anyhow::Result<()> {
        let mut tasks = self.tasks.lock().await;
        while let Some(result) = tasks.join_next().await {
            result?;
        }
        Ok(())
    }

    pub async fn registration_fence(&self) -> anyhow::Result<MediaRegistrationFence> {
        let mut tasks = Arc::clone(&self.tasks).lock_owned().await;
        while let Some(result) = tasks.join_next().await {
            result?;
        }
        Ok(MediaRegistrationFence { _tasks: tasks })
    }
}

#[derive(Clone)]
pub struct MediaPublicationWorker {
    writer: DbWriter,
    provider: Arc<dyn SyncProvider>,
    object_root: PathBuf,
    context: TransportContext,
    limiter: MediaWorkLimiter,
}

#[derive(Clone)]
pub struct MediaRestoreWorker {
    writer: DbWriter,
    provider: Arc<dyn SyncProvider>,
    cache_root: PathBuf,
    context: TransportContext,
    limiter: MediaWorkLimiter,
}

impl MediaRestoreWorker {
    pub fn new(
        writer: DbWriter,
        provider: Arc<dyn SyncProvider>,
        cache_root: PathBuf,
        context: TransportContext,
        limiter: MediaWorkLimiter,
    ) -> Self {
        Self {
            writer,
            provider,
            cache_root,
            context,
            limiter,
        }
    }

    pub async fn run_once(&self) -> anyhow::Result<bool> {
        let owner = uuid::Uuid::now_v7().to_string();
        let now = unix_timestamp_millis();
        let binding_epoch = uuid::Uuid::parse_str(&self.context.binding_epoch)?;
        let generation = self.context.transport_generation;
        let owner_for_claim = owner.clone();
        let job = self
            .writer
            .write_authoritative(move |database| {
                crate::sync::media::inventory::enqueue_missing_materializations(
                    database.connection_for_fsops(),
                    binding_epoch,
                    generation,
                    now,
                )?;
                super::inventory::claim_materialization_job(
                    database.connection_for_fsops(),
                    &owner_for_claim,
                    binding_epoch,
                    generation,
                    now,
                    60_000,
                )
            })
            .await?;
        let Some(job) = job else {
            return Ok(false);
        };
        let root_binding_guard = root_binding_token(
            job.root_uuid,
            job.local_binding_uuid,
            job.binding_generation,
        )?;
        if let Some(token) = root_binding_guard {
            let validation = self
                .writer
                .write(move |database| {
                    database.require_sync_root_binding_token_for_materialization(&token)
                })
                .await;
            if let Err(error) = validation {
                if error.to_string().contains("stale root binding generation") {
                    let owner = owner.clone();
                    let revision = job.content_revision_uuid;
                    self.writer
                        .write_authoritative(move |database| {
                            super::inventory::release_materialization_job(
                                database.connection_for_fsops(),
                                revision,
                                &owner,
                            )
                        })
                        .await?;
                    return Ok(true);
                }
                return Err(error);
            }
        }
        let foreground_pending = self
            .writer
            .write(|database| database.has_pending_foreground_thumbnail_task())
            .await?;
        if foreground_pending {
            self.limiter.record_foreground_yield();
            self.fail(
                &owner,
                job.content_revision_uuid,
                "pending",
                "foreground_pressure",
                now + 250,
            )
            .await?;
            return Ok(true);
        }
        let revision = job.content_revision_uuid;
        if job.binding_epoch != Some(binding_epoch) || job.transport_generation != generation {
            self.fail(&owner, revision, "failed", "stale_generation", i64::MAX)
                .await?;
            return Ok(true);
        }
        let suffix = job.asset_uuid.simple().to_string();
        let destination = match super::materialize::windows_safe_relative_path(
            Path::new(&job.destination_relpath),
            &suffix,
        ) {
            Ok(path) => path,
            Err(_) => {
                self.fail(
                    &owner,
                    job.content_revision_uuid,
                    "path-conflict",
                    "unsupported_destination",
                    now + 30_000,
                )
                .await?;
                return Ok(true);
            }
        };
        let managed_root = PathBuf::from(&job.managed_root);
        let target = managed_root.join(&destination);
        let policy_database = self.writer.write(|database| database.reopen()).await?;
        let owned_target = policy_database
            .as_ref()
            .map(|database| database.is_owned_collection_path(&managed_root, &target))
            .transpose();
        let owned_target = match owned_target {
            Ok(owned) => owned.unwrap_or(false),
            Err(_) => {
                self.fail(
                    &owner,
                    revision,
                    "failed",
                    "ownership_pending",
                    now + 30_000,
                )
                .await?;
                return Ok(true);
            }
        };
        if crate::sync::file_policy::require_no_links(&managed_root, &target).is_err()
            || owned_target
        {
            self.fail(
                &owner,
                revision,
                "path-conflict",
                "unsupported_destination",
                now + 30_000,
            )
            .await?;
            return Ok(true);
        }
        if job.content_hash != [0; 32]
            && target.exists()
            && super::materialize::verified_matches(&target, job.expected_length, job.content_hash)?
        {
            let identity = materialized_identity(&managed_root, &target);
            let owner = owner.clone();
            let asset = job.asset_uuid;
            let source_mtime_ms = job.source_mtime_ms;
            let expected = job.expected_length;
            let content_hash = job.content_hash;
            self.writer
                .write_authoritative(move |database| {
                    super::inventory::complete_materialization_job(
                        database.connection_for_fsops(),
                        revision,
                        asset,
                        &owner,
                        binding_epoch,
                        generation,
                        source_mtime_ms,
                        expected,
                        identity.0,
                        identity.1,
                        identity.2,
                        identity.3,
                        content_hash,
                        unix_timestamp_millis(),
                    )
                })
                .await?;
            return Ok(true);
        }
        let manifest = match super::recovery::fetch_manifest(
            self.provider.as_ref(),
            &self.context,
            job.manifest_hash,
            job.manifest_pack_hash,
            job.manifest_offset,
            job.manifest_length,
        )
        .await
        {
            Ok(manifest) => manifest,
            Err(error) => {
                let message = error.to_string();
                let (phase, code) = if message.contains("not found") {
                    ("remote-content-missing", "manifest_missing")
                } else if message.contains("hash") || message.contains("invalid") {
                    ("corrupt", "manifest_corrupt")
                } else {
                    ("failed", "manifest_fetch_failed")
                };
                self.fail(&owner, revision, phase, code, now + 10_000)
                    .await?;
                return Ok(true);
            }
        };
        if manifest.hash()? != job.manifest_hash || manifest.byte_length != job.expected_length {
            self.fail(
                &owner,
                revision,
                "corrupt",
                "manifest_identity_mismatch",
                i64::MAX,
            )
            .await?;
            return Ok(true);
        }
        if job.content_hash != [0; 32]
            && super::parse_hash(&manifest.content_sha256)? != job.content_hash
        {
            self.fail(
                &owner,
                revision,
                "corrupt",
                "content_identity_mismatch",
                i64::MAX,
            )
            .await?;
            return Ok(true);
        }
        if target.exists()
            && super::materialize::verified_matches(
                &target,
                manifest.byte_length,
                super::parse_hash(&manifest.content_sha256)?,
            )?
        {
            let identity = materialized_identity(&managed_root, &target);
            let owner = owner.clone();
            let asset = job.asset_uuid;
            let source_mtime_ms = job.source_mtime_ms;
            let expected = job.expected_length;
            self.writer
                .write_authoritative(move |database| {
                    super::inventory::complete_materialization_job(
                        database.connection_for_fsops(),
                        revision,
                        asset,
                        &owner,
                        binding_epoch,
                        generation,
                        source_mtime_ms,
                        expected,
                        identity.0,
                        identity.1,
                        identity.2,
                        identity.3,
                        super::parse_hash(&manifest.content_sha256)?,
                        unix_timestamp_millis(),
                    )
                })
                .await?;
            return Ok(true);
        }
        let ranges = super::recovery::plan_ranges(&manifest)?;
        let mut additional_cache = 0u64;
        for range in &ranges {
            let cached = self
                .writer
                .write({
                    let range = range.clone();
                    move |db| {
                        super::inventory::verified_range_relpath(
                            db.connection_for_fsops(),
                            range.pack_hash,
                            range.start,
                            range.end,
                        )
                    }
                })
                .await?;
            let required = range.end.saturating_sub(range.start);
            let existing = cached
                .and_then(|(relative, _)| {
                    std::fs::symlink_metadata(self.cache_root.join(relative)).ok()
                })
                .filter(|m| m.is_file() && !crate::sync::file_policy::is_link(m))
                .map_or(0, |m| m.len().min(required));
            additional_cache = additional_cache.saturating_add(required.saturating_sub(existing));
        }
        let reservation = (|| -> anyhow::Result<MediaTempReservation> {
            let window = super::MAX_RANGE_BYTES.saturating_add(64 * 1024 * 1024);
            require_restore_capacity(
                free_space_bytes(&self.cache_root)?,
                free_space_bytes(&managed_root)?,
                additional_cache,
                job.expected_length,
                window,
                same_storage_volume(&self.cache_root, &managed_root)?,
            )?;
            self.limiter
                .reserve_temporary_disk(&self.cache_root, window)
        })();
        let _temp_reservation = match reservation {
            Ok(value) => value,
            Err(_) => {
                self.fail(&owner, revision, "disk-full", "disk_capacity", now + 30_000)
                    .await?;
                return Ok(true);
            }
        };
        self.phase(&owner, revision, "downloading", 0).await?;
        let mut verified_chunks = BTreeMap::new();
        let mut downloaded = 0u64;
        for range in ranges {
            let cached = self
                .writer
                .write({
                    let range = range.clone();
                    move |database| {
                        super::inventory::verified_range_relpath(
                            database.connection_for_fsops(),
                            range.pack_hash,
                            range.start,
                            range.end,
                        )
                    }
                })
                .await?;
            let mut range_chunks = None;
            if let Some((relative, hash)) = cached {
                let path = self.cache_root.join(relative);
                match super::recovery::read_verified_chunks(&path, &range, Some(hash)) {
                    Ok(chunks) => range_chunks = Some(chunks),
                    Err(_) => {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
            if range_chunks.is_none() {
                let path = match super::recovery::download_and_verify_range(
                    self.provider.as_ref(),
                    &self.context,
                    &range,
                    &self.cache_root,
                )
                .await
                {
                    Ok(path) => path,
                    Err(error) => {
                        let message = error.to_string();
                        let (phase, code) = if message.contains("not found") {
                            ("remote-content-missing", "data_pack_missing")
                        } else if message.contains("hash") || message.contains("invalid") {
                            ("corrupt", "data_range_corrupt")
                        } else {
                            ("failed", "range_fetch_failed")
                        };
                        self.fail(&owner, revision, phase, code, now + 10_000)
                            .await?;
                        return Ok(true);
                    }
                };
                let range_for_persist = range.clone();
                let path_for_persist = path.clone();
                self.writer
                    .write_authoritative(move |database| {
                        super::recovery::persist_verified_range(
                            database,
                            &range_for_persist,
                            &path_for_persist,
                            unix_timestamp_millis(),
                        )
                    })
                    .await?;
                crate::sync::fault::add_and_hit_after_bytes(
                    "media.after-range-receipt",
                    range.end.saturating_sub(range.start),
                );
                range_chunks = Some(super::recovery::read_verified_chunks(&path, &range, None)?);
            }
            for (hash, source) in range_chunks.expect("verified range chunks") {
                downloaded = downloaded.saturating_add(source.length);
                verified_chunks.insert(hash, source);
            }
            self.phase(&owner, revision, "downloading", downloaded)
                .await?;
        }
        self.phase(&owner, revision, "assembling", downloaded)
            .await?;
        let content_hash = super::parse_hash(&manifest.content_sha256)?;
        let manifest_for_materialization = manifest;
        let cancelled = self.limiter.cancellation();
        let source_mtime_ms = job.source_mtime_ms;
        let managed_root_for_materialization = managed_root.clone();
        let root_binding_database = self.writer.write(|database| database.reopen()).await?;
        if root_binding_guard.is_some() && root_binding_database.is_none() {
            anyhow::bail!("managed root materialization requires a reopenable Catalog");
        }
        self.phase(&owner, revision, "publishing", downloaded)
            .await?;
        let staging_writer = self.writer.clone();
        let staging_owner = owner.clone();
        let result = self
            .limiter
            .run_bounded(1, super::MAX_RANGE_BYTES, move || {
                let staging = prepare_owned_staging(
                    &staging_writer,
                    &managed_root_for_materialization,
                    revision,
                    &staging_owner,
                )?;
                super::materialize::materialize_from_verified_ranges_with_checks(
                    &managed_root_for_materialization,
                    &destination,
                    revision,
                    &manifest_for_materialization,
                    &verified_chunks,
                    source_mtime_ms,
                    Some(staging),
                    || {
                        anyhow::ensure!(!cancelled.load(Ordering::Acquire), "media work cancelled");
                        Ok(())
                    },
                    move || {
                        if let (Some(database), Some(token)) =
                            (root_binding_database.as_ref(), root_binding_guard)
                        {
                            database.require_sync_root_binding_token_for_materialization(&token)?;
                        }
                        Ok(())
                    },
                )
            })
            .await;
        match result {
            Ok(super::materialize::MaterializationResult::Published(path))
            | Ok(super::materialize::MaterializationResult::AdoptedIdentical(path)) => {
                let identity = materialized_identity(&managed_root, &path);
                let owner = owner.clone();
                let asset = job.asset_uuid;
                let expected = job.expected_length;
                self.writer
                    .write_authoritative(move |database| {
                        super::inventory::complete_materialization_job(
                            database.connection_for_fsops(),
                            revision,
                            asset,
                            &owner,
                            binding_epoch,
                            generation,
                            source_mtime_ms,
                            expected,
                            identity.0,
                            identity.1,
                            identity.2,
                            identity.3,
                            content_hash,
                            unix_timestamp_millis(),
                        )
                    })
                    .await?;
            }
            Ok(super::materialize::MaterializationResult::PathConflict(_)) => {
                self.fail(&owner, revision, "path-conflict", "target_exists", i64::MAX)
                    .await?;
            }
            Err(error) => {
                if error.to_string().contains("stale root binding generation") {
                    let owner = owner.clone();
                    self.writer
                        .write_authoritative(move |database| {
                            super::inventory::release_materialization_job(
                                database.connection_for_fsops(),
                                revision,
                                &owner,
                            )
                        })
                        .await?;
                    return Ok(true);
                }
                let (phase, code) = match error.downcast_ref::<std::io::Error>().map(|e| e.kind()) {
                    Some(std::io::ErrorKind::PermissionDenied) => ("locked", "publish_denied"),
                    Some(std::io::ErrorKind::StorageFull) => ("disk-full", "disk_full"),
                    _ => ("failed", "materialization_failed"),
                };
                self.fail(&owner, revision, phase, code, now + 10_000)
                    .await?;
            }
        }
        Ok(true)
    }

    async fn phase(
        &self,
        owner: &str,
        revision: uuid::Uuid,
        phase: &'static str,
        downloaded: u64,
    ) -> anyhow::Result<()> {
        let owner = owner.to_owned();
        self.writer
            .write_authoritative(move |database| {
                super::inventory::set_materialization_phase(
                    database.connection_for_fsops(),
                    revision,
                    &owner,
                    phase,
                    downloaded,
                    unix_timestamp_millis(),
                )
            })
            .await
    }

    async fn fail(
        &self,
        owner: &str,
        revision: uuid::Uuid,
        phase: &'static str,
        code: &'static str,
        next_attempt_at: i64,
    ) -> anyhow::Result<()> {
        let owner = owner.to_owned();
        self.writer
            .write_authoritative(move |database| {
                super::inventory::fail_materialization_job(
                    database.connection_for_fsops(),
                    revision,
                    &owner,
                    phase,
                    code,
                    next_attempt_at,
                    unix_timestamp_millis(),
                )
            })
            .await
    }
}

impl MediaPublicationWorker {
    pub fn new(
        writer: DbWriter,
        provider: Arc<dyn SyncProvider>,
        object_root: PathBuf,
        context: TransportContext,
        limiter: MediaWorkLimiter,
    ) -> Self {
        Self {
            writer,
            provider,
            object_root,
            context,
            limiter,
        }
    }

    /// Runs at most one durable publication turn. Claims and commits happen in
    /// short writer turns; source hashing and Provider I/O happen after the
    /// writer closure has returned.
    pub async fn run_once(&self) -> anyhow::Result<bool> {
        let owner = uuid::Uuid::now_v7().to_string();
        let now = unix_timestamp_millis();
        let owner_for_claim = owner.clone();
        let binding_epoch = uuid::Uuid::parse_str(&self.context.binding_epoch)?;
        let transport_generation = self.context.transport_generation;
        let job = self
            .writer
            .write_authoritative(move |database| {
                database.seed_pending_file_publications()?;
                super::inventory::bind_pending_publication_jobs(
                    database.connection_for_fsops(),
                    binding_epoch,
                    transport_generation,
                    now,
                )?;
                crate::sync::media::inventory::ensure_publication_jobs(
                    database.connection_for_fsops(),
                    binding_epoch,
                    transport_generation,
                    now,
                )?;
                crate::sync::media::inventory::claim_publication_job(
                    database.connection_for_fsops(),
                    &owner_for_claim,
                    binding_epoch,
                    transport_generation,
                    now,
                    60_000,
                )
            })
            .await?;
        let Some(job) = job else {
            return Ok(false);
        };
        let root_binding_guard = root_binding_token(
            job.root_uuid,
            job.local_binding_uuid,
            job.binding_generation,
        )?;
        let mut job = job;
        if root_binding_guard.is_some() {
            let asset_uuid = job.asset_uuid;
            let current_source = self
                .writer
                .write(move |database| {
                    let token = root_binding_guard.expect("checked managed root token");
                    database.require_sync_root_binding_token_for_publication(&token)?;
                    database
                        .resolve_asset_source_path_with_root_token(asset_uuid)?
                        .ok_or_else(|| anyhow::anyhow!("publication source is no longer available"))
                })
                .await;
            let (current_source_path, current_root_binding_guard) = match current_source {
                Ok(source) => source,
                Err(error)
                    if error.to_string().contains("stale root binding generation")
                        || error
                            .to_string()
                            .contains("publication source is no longer available") =>
                {
                    let owner = owner.clone();
                    let intent = job.intent_uuid;
                    self.writer
                        .write_authoritative(move |database| {
                            super::inventory::release_publication_job(
                                database.connection_for_fsops(),
                                intent,
                                &owner,
                            )
                        })
                        .await?;
                    return Ok(true);
                }
                Err(error) => return Err(error),
            };
            if current_root_binding_guard != root_binding_guard {
                let owner = owner.clone();
                let intent = job.intent_uuid;
                self.writer
                    .write_authoritative(move |database| {
                        super::inventory::release_publication_job(
                            database.connection_for_fsops(),
                            intent,
                            &owner,
                        )
                    })
                    .await?;
                return Ok(true);
            }
            job.source_path = current_source_path.to_string_lossy().into_owned();
        }
        let asset = job.asset_uuid;
        let source_root = self
            .writer
            .write(move |db| {
                Ok(db.connection_for_fsops().query_row(
                    "SELECT r.path FROM roots r JOIN files f ON f.root_id=r.id
                     WHERE f.asset_uuid=?1",
                    [asset.as_bytes().as_slice()],
                    |row| row.get::<_, String>(0),
                )?)
            })
            .await?;
        let policy_database = self.writer.write(|database| database.reopen()).await?;
        let owned_source = policy_database
            .as_ref()
            .map(|database| {
                database
                    .is_owned_collection_path(Path::new(&source_root), Path::new(&job.source_path))
            })
            .transpose();
        if !matches!(owned_source, Ok(None | Some(false))) {
            let (error_code, retry_at) = if owned_source.is_err() {
                ("ownership_pending", now + 30_000)
            } else {
                ("unsupported_source", i64::MAX)
            };
            let intent = job.intent_uuid;
            let owner = owner.clone();
            self.writer
                .write_authoritative(move |db| {
                    super::inventory::fail_publication_job(
                        db.connection_for_fsops(),
                        intent,
                        &owner,
                        error_code,
                        retry_at,
                        now,
                    )
                })
                .await?;
            return Ok(true);
        }
        let foreground_pending = self
            .writer
            .write(|database| database.has_pending_foreground_thumbnail_task())
            .await?;
        if foreground_pending {
            self.limiter.record_foreground_yield();
            let intent = job.intent_uuid;
            let owner = owner.clone();
            self.writer
                .write_authoritative(move |database| {
                    super::inventory::fail_publication_job(
                        database.connection_for_fsops(),
                        intent,
                        &owner,
                        "foreground_pressure",
                        unix_timestamp_millis() + 250,
                        unix_timestamp_millis(),
                    )
                })
                .await?;
            return Ok(true);
        }
        if job
            .binding_epoch
            .as_ref()
            .is_some_and(|epoch| epoch.to_string() != self.context.binding_epoch)
            || job.transport_generation != self.context.transport_generation
        {
            let intent = job.intent_uuid;
            let owner = owner.clone();
            self.writer
                .write_authoritative(move |database| {
                    super::inventory::fail_publication_job(
                        database.connection_for_fsops(),
                        intent,
                        &owner,
                        "stale_generation",
                        i64::MAX,
                        unix_timestamp_millis(),
                    )
                })
                .await?;
            return Ok(true);
        }
        let intent = job.intent_uuid;
        let sealed_graph_exists = self
            .writer
            .write(move |database| {
                super::inventory::publication_graph_ready(database.connection_for_fsops(), intent)
            })
            .await?;
        if !sealed_graph_exists {
            let reservation = self.limiter.reserve_content_work(
                &self.object_root,
                job.expected_source_length
                    .saturating_add(job.expected_source_length / 100),
                super::DATA_PACK_MAX_BYTES.saturating_add(64 * 1024 * 1024),
            );
            let _temp_reservation = match reservation {
                Ok(value) => value,
                Err(_) => {
                    let intent = job.intent_uuid;
                    let owner = owner.clone();
                    self.writer
                        .write_authoritative(move |database| {
                            super::inventory::fail_publication_job(
                                database.connection_for_fsops(),
                                intent,
                                &owner,
                                "disk_capacity",
                                now + 30_000,
                                now,
                            )
                        })
                        .await?;
                    return Ok(true);
                }
            };
            let owner_for_phase = owner.clone();
            self.writer
                .write_authoritative(move |database| {
                    super::inventory::set_publication_phase(
                        database.connection_for_fsops(),
                        intent,
                        &owner_for_phase,
                        super::inventory::PublicationPhase::Chunking,
                        unix_timestamp_millis(),
                    )
                })
                .await?;
            let object_root = self.object_root.clone();
            let cancelled = self.limiter.cancellation();
            let job_for_build = job.clone();
            let root_binding_database = self.writer.write(|database| database.reopen()).await?;
            if root_binding_guard.is_some() && root_binding_database.is_none() {
                anyhow::bail!("managed root publication requires a reopenable Catalog");
            }
            let publication = self
                .limiter
                .run_bounded(1, super::DATA_PACK_MAX_BYTES, move || {
                    if let (Some(database), Some(token)) =
                        (root_binding_database.as_ref(), root_binding_guard)
                    {
                        database.require_sync_root_binding_token_for_publication(&token)?;
                    }
                    super::publication::build_publication(
                        &job_for_build,
                        &object_root,
                        super::format::MediaHint {
                            sniffed_kind: None,
                            mime: None,
                        },
                        &cancelled,
                    )
                })
                .await;
            let publication = match publication {
                Ok(value) => value,
                Err(error) => {
                    let code = if error.to_string().contains("changed") {
                        "source_changed"
                    } else {
                        "build_failed"
                    };
                    let owner = owner.clone();
                    self.writer
                        .write_authoritative(move |database| {
                            super::inventory::fail_publication_job(
                                database.connection_for_fsops(),
                                intent,
                                &owner,
                                code,
                                unix_timestamp_millis() + 5_000,
                                unix_timestamp_millis(),
                            )
                        })
                        .await?;
                    return Ok(true);
                }
            };
            let persisted = publication;
            let job_for_persist = job.clone();
            let owner_for_persist = owner.clone();
            self.writer
                .write_authoritative(move |database| {
                    super::publication::persist_publication(
                        database,
                        &job_for_persist,
                        &owner_for_persist,
                        &persisted,
                        unix_timestamp_millis(),
                    )
                })
                .await?;
        }
        // The object list is durable now; network I/O occurs without the writer.
        let result = super::publication::upload_sealed_objects(
            &self.writer,
            self.provider.as_ref(),
            &self.context,
            &self.object_root,
            &job,
            &owner,
            unix_timestamp_millis(),
        )
        .await;
        match result {
            Ok(()) => {
                let owner = owner.clone();
                self.writer
                    .write_authoritative(move |database| {
                        super::inventory::release_publication_job(
                            database.connection_for_fsops(),
                            intent,
                            &owner,
                        )
                    })
                    .await?;
            }
            Err(error) => {
                let owner = owner.clone();
                self.writer
                    .write_authoritative(move |database| {
                        super::inventory::fail_publication_job(
                            database.connection_for_fsops(),
                            intent,
                            &owner,
                            "provider_or_canonicalization_failed",
                            unix_timestamp_millis() + 10_000,
                            unix_timestamp_millis(),
                        )
                    })
                    .await?;
                return Err(error);
            }
        }
        Ok(true)
    }
}

fn root_binding_token(
    root_uuid: Option<uuid::Uuid>,
    local_binding_uuid: Option<uuid::Uuid>,
    binding_generation: Option<u64>,
) -> anyhow::Result<Option<SyncRootBindingToken>> {
    match (root_uuid, local_binding_uuid, binding_generation) {
        (Some(root_uuid), Some(local_binding_uuid), Some(binding_generation)) => {
            Ok(Some(SyncRootBindingToken {
                root_uuid: *root_uuid.as_bytes(),
                local_binding_uuid: *local_binding_uuid.as_bytes(),
                binding_generation,
            }))
        }
        (None, None, None) => Ok(None),
        _ => anyhow::bail!("media job root binding token is partial"),
    }
}

fn unix_timestamp_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn existing_storage_directory(path: &Path) -> std::io::Result<&Path> {
    path.ancestors()
        .find(|candidate| candidate.is_dir())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "storage directory unavailable",
            )
        })
}

fn same_storage_volume(left: &Path, right: &Path) -> std::io::Result<bool> {
    let left = crate::library_path::local_file_identity(existing_storage_directory(left)?)?;
    let right = crate::library_path::local_file_identity(existing_storage_directory(right)?)?;
    Ok(left.split(':').take(2).eq(right.split(':').take(2)))
}

fn require_publication_capacity(
    available: u64,
    durable_bytes: u64,
    window_bytes: u64,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        available
            >= durable_bytes
                .saturating_add(window_bytes)
                .saturating_add(64 * 1024 * 1024),
        "content storage capacity exhausted"
    );
    Ok(())
}

fn require_restore_capacity(
    cache_free: u64,
    target_free: u64,
    cache_bytes: u64,
    target_bytes: u64,
    window: u64,
    same_volume: bool,
) -> anyhow::Result<()> {
    let cache_required = cache_bytes
        .saturating_add(window)
        .saturating_add(64 * 1024 * 1024);
    let target_required = target_bytes.saturating_add(64 * 1024 * 1024);
    if same_volume {
        anyhow::ensure!(
            cache_free.min(target_free) >= cache_required.saturating_add(target_bytes),
            "shared cache and destination capacity exhausted"
        );
    } else {
        anyhow::ensure!(
            cache_free >= cache_required,
            "range cache capacity exhausted"
        );
        anyhow::ensure!(
            target_free >= target_required,
            "destination capacity exhausted"
        );
    }
    Ok(())
}

#[cfg(windows)]
fn free_space_bytes(path: &Path) -> std::io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let existing = existing_storage_directory(path)?;
    let wide: Vec<u16> = existing.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0_u64;
    if unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(available)
    }
}

#[cfg(unix)]
fn free_space_bytes(path: &Path) -> std::io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let existing = existing_storage_directory(path)?;
    let name = CString::new(existing.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(name.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stats = unsafe { stats.assume_init() };
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

#[cfg(not(any(windows, unix)))]
fn free_space_bytes(_path: &Path) -> std::io::Result<u64> {
    Ok(u64::MAX)
}

fn prepare_owned_staging(
    writer: &DbWriter,
    root: &Path,
    revision: uuid::Uuid,
    owner: &str,
) -> anyhow::Result<super::materialize::OwnedMaterializationStaging> {
    use rusqlite::OptionalExtension;
    let owner = owner.to_owned();
    let previous = writer.write_blocking(move |db| {
        Ok(db.connection_for_fsops().query_row(
            "SELECT staging_relpath,staging_directory_identity,staging_file_identity FROM media_materialization_jobs WHERE content_revision_uuid=?1 AND staging_relpath IS NOT NULL",
            [revision.as_bytes().as_slice()], |row| Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,Option<String>>(2)?))).optional()?)
    })?;
    if let Some((relative, expected, file_identity)) = previous {
        let partial = root.join(&relative);
        let directory = partial
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid staging path"))?;
        // Legacy shared staging names have no durable ownership proof. Preserve
        // them and choose a new private directory instead of deleting/adopting.
        let owned_layout = relative.starts_with(&format!(".mangoo-work-{}-", revision.simple()))
            && partial.file_name() == Some(std::ffi::OsStr::new("payload.partial"))
            && directory.parent() == Some(root);
        if owned_layout && directory.exists() {
            let expected = expected.ok_or_else(|| {
                anyhow::anyhow!("materialization staging ownership pending; retry required")
            })?;
            let guard = crate::library_path::MarkerDirectoryGuard::open(directory)?;
            anyhow::ensure!(
                guard.identity() == expected,
                "materialization staging identity changed"
            );
            // The exact payload path belongs to the identity-bound operation
            // directory; never clean any other entries or recursively delete it.
            let file_identity = file_identity
                .ok_or_else(|| anyhow::anyhow!("materialization payload ownership pending"))?;
            match guard
                .delete_file_child_expected(std::ffi::OsStr::new("payload.partial"), &file_identity)
            {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            drop(guard);
            crate::library_path::safe_remove_empty_directory_expected(directory, &expected)?;
        }
    }
    // A new random path never claims an earlier user directory. Persist the
    // plan before creation so simultaneous observation retries pending evidence.
    let relative = format!(
        ".mangoo-work-{}-{}/payload.partial",
        revision.simple(),
        uuid::Uuid::now_v7().simple()
    );
    let planned = relative.clone();
    let planned_owner = owner.clone();
    writer.write_authoritative_blocking(move |db| {
        let changed = db.connection_for_fsops().execute(
            "UPDATE media_materialization_jobs SET staging_relpath=?1,staging_directory_identity=NULL,staging_file_identity=NULL WHERE content_revision_uuid=?2 AND owner_token=?3",
            rusqlite::params![planned,revision.as_bytes().as_slice(),planned_owner])?;
        anyhow::ensure!(changed == 1, "materialization staging claim expired");
        Ok(())
    })?;
    let directory = root
        .join(&relative)
        .parent()
        .expect("staging parent")
        .to_path_buf();
    let root_guard = crate::library_path::MarkerDirectoryGuard::open(root)?;
    let guard = match root_guard
        .create_directory_child(directory.file_name().expect("staging leaf"))
    {
        Ok(guard) => guard,
        Err(error) => {
            // Only remove our pending DB intent; an occupied path is not ours.
            writer.write_authoritative_blocking(move |db| {
            db.connection_for_fsops().execute("UPDATE media_materialization_jobs SET staging_relpath=NULL WHERE content_revision_uuid=?1 AND owner_token=?2 AND staging_relpath=?3 AND staging_directory_identity IS NULL", rusqlite::params![revision.as_bytes().as_slice(),owner,relative])?;
            Ok(())
        })?;
            return Err(error.into());
        }
    };
    root_guard.verify_unchanged()?;
    let identity = guard.identity().to_owned();
    let payload = guard.open_temporary(std::ffi::OsStr::new("payload.partial"), true)?;
    let payload_identity = guard.file_identity(&payload)?;
    let owned_file_identity = payload_identity.clone();
    let owned_identity = identity.clone();
    writer.write_authoritative_blocking(move |db| {
        let changed = db.connection_for_fsops().execute(
            "UPDATE media_materialization_jobs SET staging_directory_identity=?1,staging_file_identity=?5 WHERE content_revision_uuid=?2 AND owner_token=?3 AND staging_relpath=?4 AND staging_directory_identity IS NULL",
            rusqlite::params![owned_identity,revision.as_bytes().as_slice(),owner,relative,owned_file_identity])?;
        anyhow::ensure!(changed == 1, "materialization staging claim expired");
        Ok(())
    })?;
    Ok(super::materialize::OwnedMaterializationStaging {
        directory,
        identity,
        guard: Some(guard),
        payload_identity: Some(payload_identity),
        payload: Some(payload),
    })
}

fn materialized_identity(
    managed_root: &Path,
    target: &Path,
) -> (Vec<u8>, Vec<u8>, Option<String>, String) {
    let volume_identity = crate::library_path::local_file_identity(managed_root)
        .unwrap_or_else(|_| managed_root.to_string_lossy().into_owned())
        .into_bytes();
    let local_path = target.to_string_lossy().into_owned();
    let file_key = crate::library_path::local_file_identity(target).ok();
    let file_identity = file_key
        .as_deref()
        .unwrap_or(&local_path)
        .as_bytes()
        .to_vec();
    (volume_identity, file_identity, file_key, local_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use std::{
        fs::File,
        io::{Read, Write},
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    const PHYSICAL_TRANSFER_BUFFER_BYTES: usize = 1024 * 1024;
    const PHYSICAL_TRANSFER_RANGE_BYTES: u64 = 16 * 1024 * 1024;

    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PhysicalTransferState {
        namespace: String,
        binding_epoch: String,
        file_id: i64,
        source_sha256: String,
    }

    fn physical_transfer_root() -> PathBuf {
        std::env::var_os("MANGOO_SYNC_LARGE_TRANSFER_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!(
                    "mangoo-physical-transfer-{}",
                    uuid::Uuid::now_v7().simple()
                ))
            })
    }

    fn physical_transfer_bytes() -> u64 {
        std::env::var("MANGOO_SYNC_LARGE_TRANSFER_BYTES")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(10 * 1024 * 1024 * 1024)
    }

    fn load_physical_state(root: &Path) -> PhysicalTransferState {
        std::fs::read(root.join("transfer-state.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save_physical_state(root: &Path, state: &PhysicalTransferState) {
        let path = root.join("transfer-state.json");
        let temporary = root.join("transfer-state.json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(state).unwrap()).unwrap();
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        std::fs::rename(temporary, path).unwrap();
    }

    fn deterministic_fill(buffer: &mut [u8], absolute_offset: u64) {
        for (index, value) in buffer.iter_mut().enumerate() {
            let position = absolute_offset + index as u64;
            *value = position
                .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                .rotate_left((position & 31) as u32) as u8;
        }
    }

    fn generate_physical_source(path: &Path, byte_length: u64) {
        if path
            .metadata()
            .is_ok_and(|metadata| metadata.len() == byte_length)
        {
            return;
        }
        let mut file = File::create(path).unwrap();
        let mut buffer = vec![0_u8; PHYSICAL_TRANSFER_BUFFER_BYTES];
        let mut offset = 0_u64;
        while offset < byte_length {
            let count = usize::try_from((byte_length - offset).min(buffer.len() as u64)).unwrap();
            deterministic_fill(&mut buffer[..count], offset);
            file.write_all(&buffer[..count]).unwrap();
            offset += count as u64;
        }
        file.sync_all().unwrap();
    }

    fn hash_physical_file(path: &Path) -> String {
        let mut file = File::open(path).unwrap();
        let mut hash = Sha256::new();
        let mut buffer = vec![0_u8; PHYSICAL_TRANSFER_BUFFER_BYTES];
        loop {
            let count = file.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        super::super::hex_hash(&hash.finalize().into())
    }

    fn physical_platform() -> crate::plugins::manifest::Platform {
        if cfg!(windows) {
            crate::plugins::manifest::Platform::WindowsX64
        } else {
            crate::plugins::manifest::Platform::LinuxX64
        }
    }

    async fn activate_physical_provider(
        root: &Path,
    ) -> crate::plugins::runtime::supervisor::ActivatedProvider {
        const PLUGIN_ID: &str = "com.mangoo.fake-sync";
        let bundled_root = std::env::var_os("MANGOO_SYNC_BUNDLED_PLUGINS_DIR")
            .map(PathBuf::from)
            .expect("MANGOO_SYNC_BUNDLED_PLUGINS_DIR must identify the packaged Provider root");
        let plugin_root = bundled_root.join(PLUGIN_ID);
        let manifest = crate::plugins::parse_manifest(&plugin_root.join("plugin.json")).unwrap();
        crate::plugins::runtime::supervisor::activate_and_register(
            &crate::sync::provider::ProviderManager::default(),
            &manifest,
            "physical-transfer-account",
            manifest.requested_permissions.clone(),
            &plugin_root,
            physical_platform(),
            crate::plugins::runtime::trust::embedded_trust_registry(),
            root.join("provider-runtime"),
            None,
            std::collections::BTreeMap::new(),
        )
        .await
        .unwrap()
    }

    fn initialize_physical_transfer(root: &Path, byte_length: u64) -> PhysicalTransferState {
        let existing = load_physical_state(root);
        if !existing.namespace.is_empty() {
            return existing;
        }
        std::fs::create_dir_all(root.join("managed")).unwrap();
        let source = root.join("managed").join("source.bin");
        generate_physical_source(&source, byte_length);
        let source_sha256 = hash_physical_file(&source);
        let database = crate::db::Database::open(root.join("catalog.sqlite")).unwrap();
        database.apply_migrations().unwrap();
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: root.join("managed").to_string_lossy().into_owned(),
                display_name: "Physical transfer".into(),
            })
            .unwrap();
        let binding_epoch = uuid::Uuid::now_v7();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        database
            .begin_sync_capture(binding_epoch, uuid::Uuid::now_v7())
            .unwrap();
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root".into(),
                mtime: None,
            })
            .unwrap();
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: "source.bin".into(),
                ext: "bin".into(),
                size: i64::try_from(byte_length).unwrap(),
                mtime: 1,
                ctime: None,
                file_key: Some("physical-source".into()),
            })
            .unwrap();
        database
            .create_sync_publication_intent(
                file_id,
                &source.to_string_lossy(),
                i64::try_from(byte_length).unwrap(),
            )
            .unwrap();
        let state = PhysicalTransferState {
            namespace: uuid::Uuid::now_v7().to_string(),
            binding_epoch: binding_epoch.to_string(),
            file_id,
            source_sha256,
        };
        save_physical_state(root, &state);
        state
    }

    async fn run_physical_upload_child(root: &Path, byte_length: u64) {
        let state = initialize_physical_transfer(root, byte_length);
        let database = crate::db::Database::open(root.join("catalog.sqlite")).unwrap();
        database.apply_migrations().unwrap();
        database.connection_for_fsops().execute(
            "UPDATE media_publication_jobs SET owner_token=NULL,lease_expires_at=0,next_attempt_at=0",
            [],
        ).unwrap();
        let writer = DbWriter::spawn(database);
        let activated = activate_physical_provider(root).await;
        let limiter = MediaWorkLimiter::new_with_temp_budget(
            1,
            8,
            byte_length.saturating_add(super::super::DATA_PACK_MAX_BYTES),
            byte_length
                .saturating_add(super::super::DATA_PACK_MAX_BYTES)
                .saturating_add(64 * 1024 * 1024),
        )
        .unwrap();
        let worker = MediaPublicationWorker::new(
            writer.clone(),
            activated.provider.clone(),
            root.join("sealed-objects"),
            TransportContext {
                namespace: state.namespace,
                binding_epoch: state.binding_epoch,
                transport_generation: 1,
            },
            limiter,
        );
        assert!(worker.run_once().await.unwrap());
        activated.shutdown().await.unwrap();
        writer.shutdown().await.unwrap();
    }

    async fn run_physical_download_child(root: &Path, byte_length: u64) {
        let state = load_physical_state(root);
        assert_eq!(physical_transfer_bytes(), byte_length);
        let database = crate::db::Database::open(root.join("catalog.sqlite")).unwrap();
        database.apply_migrations().unwrap();
        database.connection_for_fsops().execute(
            "UPDATE media_materialization_jobs SET owner_token=NULL,lease_expires_at=0,next_attempt_at=0 WHERE phase<>'complete'",
            [],
        ).unwrap();
        let writer = DbWriter::spawn(database);
        let activated = activate_physical_provider(root).await;
        let limiter = MediaWorkLimiter::new_with_temp_budget(
            1,
            8,
            byte_length.saturating_add(super::super::MAX_RANGE_BYTES),
            byte_length
                .saturating_add(super::super::MAX_RANGE_BYTES)
                .saturating_add(64 * 1024 * 1024),
        )
        .unwrap();
        let worker = MediaRestoreWorker::new(
            writer.clone(),
            activated.provider.clone(),
            root.join("range-cache"),
            TransportContext {
                namespace: state.namespace,
                binding_epoch: state.binding_epoch,
                transport_generation: 1,
            },
            limiter,
        );
        assert!(worker.run_once().await.unwrap());
        activated.shutdown().await.unwrap();
        writer.shutdown().await.unwrap();
    }

    fn spawn_physical_transfer_child(
        root: &Path,
        byte_length: u64,
        phase: &str,
        stop: bool,
    ) -> std::process::Child {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--ignored")
            .arg("--exact")
            .arg("sync::media::worker::tests::physical_large_transfer_child")
            .arg("--nocapture")
            .env("MANGOO_SYNC_LARGE_TRANSFER_ROOT", root)
            .env("MANGOO_SYNC_LARGE_TRANSFER_BYTES", byte_length.to_string())
            .env("MANGOO_SYNC_LARGE_TRANSFER_PHASE", phase)
            .env(
                "MANGOO_SYNC_FAULT_COUNTER",
                root.join(format!("{phase}-durable-bytes")),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        if let Some(bundled) = std::env::var_os("MANGOO_SYNC_BUNDLED_PLUGINS_DIR") {
            command.env("MANGOO_SYNC_BUNDLED_PLUGINS_DIR", bundled);
        }
        if stop {
            command
                .env(
                    "MANGOO_SYNC_FAULTPOINT",
                    if phase == "upload" {
                        "media.after-upload-cursor"
                    } else {
                        "media.after-range-receipt"
                    },
                )
                .env(
                    "MANGOO_SYNC_FAULT_AFTER_BYTES",
                    (byte_length / 2).to_string(),
                )
                .env(
                    "MANGOO_SYNC_FAULT_MARKER",
                    root.join(format!("{phase}-stop.marker")),
                );
        }
        command.spawn().unwrap()
    }

    #[cfg(windows)]
    fn process_peak_rss_bytes(process_id: u32) -> u64 {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, process_id);
            if handle.is_null() {
                return 0;
            }
            let mut counters = PROCESS_MEMORY_COUNTERS::default();
            counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            let ok = K32GetProcessMemoryInfo(handle, &mut counters, counters.cb);
            CloseHandle(handle);
            if ok == 0 {
                0
            } else {
                counters.PeakWorkingSetSize as u64
            }
        }
    }

    #[cfg(not(windows))]
    fn process_peak_rss_bytes(process_id: u32) -> u64 {
        std::fs::read_to_string(format!("/proc/{process_id}/status"))
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find(|line| line.starts_with("VmHWM:"))
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .unwrap_or(0)
            .saturating_mul(1024)
    }

    fn wait_for_physical_marker(path: &Path, child: &mut std::process::Child) -> u64 {
        let deadline = Instant::now() + Duration::from_secs(15 * 60);
        let mut peak_rss = 0;
        while Instant::now() < deadline {
            if path.exists() {
                return peak_rss.max(process_peak_rss_bytes(child.id()));
            }
            peak_rss = peak_rss.max(process_peak_rss_bytes(child.id()));
            if let Some(status) = child.try_wait().unwrap() {
                panic!("physical transfer child exited before durable marker: {status}");
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("physical transfer child did not reach durable marker");
    }

    fn wait_for_physical_completion(
        mut child: std::process::Child,
    ) -> (std::process::ExitStatus, u64) {
        let deadline = Instant::now() + Duration::from_secs(60 * 60);
        let mut peak_rss = 0;
        loop {
            peak_rss = peak_rss.max(process_peak_rss_bytes(child.id()));
            if let Some(status) = child.try_wait().unwrap() {
                return (status, peak_rss);
            }
            assert!(
                Instant::now() < deadline,
                "physical transfer child timed out"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "child entry point for the physical transfer kill/resume probe"]
    async fn physical_large_transfer_child() {
        let root = physical_transfer_root();
        let byte_length = physical_transfer_bytes();
        match std::env::var("MANGOO_SYNC_LARGE_TRANSFER_PHASE").as_deref() {
            Ok("upload") => run_physical_upload_child(&root, byte_length).await,
            Ok("download") => run_physical_download_child(&root, byte_length).await,
            phase => panic!("invalid physical transfer phase: {phase:?}"),
        }
    }

    #[test]
    #[ignore = "physical 10 GiB acceptance probe; invoked explicitly by the release benchmark"]
    fn physical_large_transfer_resume_probe() {
        let root = physical_transfer_root();
        let byte_length = physical_transfer_bytes();
        let state = initialize_physical_transfer(&root, byte_length);

        let mut upload = spawn_physical_transfer_child(&root, byte_length, "upload", true);
        let mut peak_rss = wait_for_physical_marker(&root.join("upload-stop.marker"), &mut upload);
        upload.kill().unwrap();
        let killed_upload = upload.wait().unwrap();
        assert!(!killed_upload.success());
        let upload_after_kill = std::fs::read_to_string(root.join("upload-durable-bytes"))
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(upload_after_kill >= byte_length / 2 && upload_after_kill < byte_length);
        let (status, upload_resume_peak) = wait_for_physical_completion(
            spawn_physical_transfer_child(&root, byte_length, "upload", false),
        );
        peak_rss = peak_rss.max(upload_resume_peak);
        assert!(status.success());

        let source = root.join("managed").join("source.bin");
        assert_eq!(hash_physical_file(&source), state.source_sha256);
        std::fs::remove_file(&source).unwrap();
        // Upload has committed every immutable object to the Provider. Drop the
        // local publication cache before restore so the acceptance probe does
        // not require five simultaneous copies of the 10 GiB corpus.
        std::fs::remove_dir_all(root.join("sealed-objects")).unwrap();
        let database = crate::db::Database::open(root.join("catalog.sqlite")).unwrap();
        database
            .connection_for_fsops()
            .execute(
                "UPDATE files SET status='missing' WHERE id=?1",
                [state.file_id],
            )
            .unwrap();

        let mut download = spawn_physical_transfer_child(&root, byte_length, "download", true);
        peak_rss = peak_rss.max(wait_for_physical_marker(
            &root.join("download-stop.marker"),
            &mut download,
        ));
        download.kill().unwrap();
        let killed_download = download.wait().unwrap();
        assert!(!killed_download.success());
        let download_after_kill = std::fs::read_to_string(root.join("download-durable-bytes"))
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(download_after_kill >= byte_length / 2 && download_after_kill < byte_length);
        let (status, download_resume_peak) = wait_for_physical_completion(
            spawn_physical_transfer_child(&root, byte_length, "download", false),
        );
        peak_rss = peak_rss.max(download_resume_peak);
        assert!(status.success());

        let restored_digest = hash_physical_file(&source);
        assert_eq!(restored_digest, state.source_sha256);
        let database = crate::db::Database::open(root.join("catalog.sqlite")).unwrap();
        let (sealed_bytes, verified_bytes, verified_ranges): (i64, i64, i64) = database
            .connection_for_fsops()
            .query_row(
                "SELECT (SELECT COALESCE(SUM(byte_length),0) FROM media_publication_objects),(SELECT COALESCE(SUM(range_end-range_start),0) FROM media_verified_ranges),(SELECT COUNT(*) FROM media_verified_ranges)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let counters: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                root.join("provider-runtime")
                    .join("provider-state")
                    .join("counters.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let upload_wire_bytes = counters["uploadWireBytes"].as_u64().unwrap();
        let data_range_wire_bytes = counters["dataRangeWireBytes"].as_u64().unwrap();
        let peak_provider_buffer = counters["peakBufferedBytes"].as_u64().unwrap();
        assert_eq!(upload_wire_bytes, u64::try_from(sealed_bytes).unwrap());
        assert_eq!(
            data_range_wire_bytes,
            u64::try_from(verified_bytes).unwrap()
        );
        assert!(peak_provider_buffer <= PHYSICAL_TRANSFER_RANGE_BYTES);
        assert!(
            peak_rss < 1024 * 1024 * 1024,
            "worker peak RSS exceeded 1 GiB"
        );
        println!(
            "SYNC_LARGE_TRANSFER_JSON={}",
            serde_json::json!({
                "byteLength": byte_length,
                "rangeBytes": PHYSICAL_TRANSFER_RANGE_BYTES,
                "bufferBytes": PHYSICAL_TRANSFER_BUFFER_BYTES,
                "transport": "production-media-workers-over-framed-provider-rpc",
                "providerRpc": true,
                "uploadOffsetAfterKill": upload_after_kill,
                "downloadOffsetAfterKill": download_after_kill,
                "uploadWireBytes": upload_wire_bytes,
                "sealedObjectBytes": sealed_bytes,
                "dataRangeWireBytes": data_range_wire_bytes,
                "verifiedRangeBytes": verified_bytes,
                "verifiedRanges": verified_ranges,
                "peakWorkerRssBytes": peak_rss,
                "peakProviderBufferBytes": peak_provider_buffer,
                "sourceSha256": state.source_sha256,
                "restoredSha256": restored_digest,
                "runDirectory": root,
            })
        );
    }

    #[tokio::test]
    async fn bounded_limiter_cancels_and_joins() {
        let limiter = MediaWorkLimiter::new(2, 10, 1024).unwrap();
        assert!(limiter.validate_batch(11, 1).is_err());
        assert!(limiter.run_bounded(11, 1, || Ok(())).await.is_err());
        assert_eq!(limiter.metrics().queue_rejections, 2);
        assert_eq!(limiter.run(|| Ok(7)).await.unwrap(), 7);
        let root = std::env::temp_dir();
        let reservation = limiter.reserve_temporary_disk(&root, 1024).unwrap();
        assert_eq!(limiter.metrics().reserved_temp_bytes, 1024);
        drop(reservation);
        assert_eq!(limiter.metrics().reserved_temp_bytes, 0);
        let tiny_disk = MediaWorkLimiter::new_with_temp_budget(1, 1, 1, 1).unwrap();
        assert!(tiny_disk.reserve_temporary_disk(&root, 2).is_err());
        assert_eq!(tiny_disk.metrics().disk_rejections, 1);
        let workers = MediaWorkerSet::new(limiter);
        let running = workers.clone();
        let cancellation = running.limiter().cancellation();
        let task = tokio::spawn(async move {
            running
                .run(async move {
                    while !cancellation.load(Ordering::Acquire) {
                        tokio::task::yield_now().await;
                    }
                    Ok(())
                })
                .await
        });
        tokio::task::yield_now().await;
        workers.shutdown().await.unwrap();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn cancellation_cannot_race_task_registration() {
        let workers = MediaWorkerSet::new(MediaWorkLimiter::new(1, 1, 1).unwrap());
        let registration_guard = workers.tasks.lock().await;
        let late = workers.clone();
        let task = tokio::spawn(async move { late.run(async { Ok(()) }).await });
        tokio::task::yield_now().await;
        workers.cancel();
        drop(registration_guard);
        assert!(task.await.unwrap().is_err());
        workers.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn draining_registered_turns_does_not_cancel_the_shared_worker_set() {
        let workers = MediaWorkerSet::new(MediaWorkLimiter::new(1, 1, 1).unwrap());
        let running = workers.clone();
        let (release, released) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            running
                .run(async move {
                    let _ = released.await;
                    Ok(7)
                })
                .await
        });
        tokio::task::yield_now().await;
        release.send(()).unwrap();
        workers.drain().await.unwrap();
        assert_eq!(task.await.unwrap().unwrap(), 7);
        assert_eq!(workers.run(async { Ok(9) }).await.unwrap(), 9);
    }

    #[tokio::test]
    async fn registration_fence_drains_old_turns_and_blocks_new_turns() {
        let workers = MediaWorkerSet::new(MediaWorkLimiter::new(1, 1, 1).unwrap());
        let running = workers.clone();
        let (release, released) = tokio::sync::oneshot::channel();
        let old = tokio::spawn(async move {
            running
                .run(async move {
                    let _ = released.await;
                    Ok(())
                })
                .await
        });
        tokio::task::yield_now().await;
        release.send(()).unwrap();
        let fence = workers.registration_fence().await.unwrap();
        assert!(!old.is_finished());
        let late_workers = workers.clone();
        let late = tokio::spawn(async move { late_workers.run(async { Ok(11) }).await });
        tokio::task::yield_now().await;
        assert!(!late.is_finished());
        drop(fence);
        old.await.unwrap().unwrap();
        assert_eq!(late.await.unwrap().unwrap(), 11);
    }

    #[test]
    fn ten_gib_resume_plan_is_bounded_and_skips_verified_ranges() {
        let pack_hash_bytes = crate::sync::codec::sha256(b"pack");
        let pack_hash = super::super::hex_hash(&pack_hash_bytes);
        let chunk_size = super::super::CDC_MAX_BYTES;
        let chunk_count = (10 * 1024 * 1024 * 1024_u64) / chunk_size;
        let chunks = (0..chunk_count)
            .map(|ordinal| crate::sync::media::format::ManifestChunkV1 {
                ordinal: ordinal as u32,
                chunk_sha256: super::super::hex_hash(&crate::sync::codec::sha256(
                    &ordinal.to_le_bytes(),
                )),
                byte_length: chunk_size,
                pack_sha256: pack_hash.clone(),
                pack_offset: ordinal * chunk_size,
            })
            .collect();
        let manifest = crate::sync::media::format::ContentManifestV1 {
            format_version: 1,
            byte_length: 10 * 1024 * 1024 * 1024_u64,
            content_sha256: super::super::hex_hash(&crate::sync::codec::sha256(b"logical-10g")),
            media_hint: crate::sync::media::format::MediaHint {
                sniffed_kind: Some("video".into()),
                mime: Some("video/mp4".into()),
            },
            chunking: crate::sync::media::format::ChunkingProfile::default(),
            chunks,
        };
        let ranges = crate::sync::media::recovery::plan_ranges(&manifest).unwrap();
        assert_eq!(ranges.len(), chunk_count as usize);
        assert!(ranges
            .iter()
            .all(|range| range.end - range.start <= super::super::MAX_RANGE_BYTES));

        let database = crate::db::Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let connection = database.connection_for_fsops();
        let already_verified = ranges.len() / 2;
        for (index, range) in ranges.iter().take(already_verified).enumerate() {
            crate::sync::media::inventory::record_verified_range(
                connection,
                pack_hash_bytes,
                range.start,
                range.end,
                crate::sync::codec::sha256(&index.to_le_bytes()),
                &format!("ranges/{index}.range"),
                1,
            )
            .unwrap();
        }
        let pending = ranges
            .iter()
            .filter(|range| {
                crate::sync::media::inventory::verified_range_relpath(
                    connection,
                    pack_hash_bytes,
                    range.start,
                    range.end,
                )
                .unwrap()
                .is_none()
            })
            .count();
        assert_eq!(pending, ranges.len() - already_verified);
    }

    #[test]
    #[ignore = "scale acceptance probe; invoked explicitly by the media verification gate"]
    fn media_inventory_scale_probe() {
        let database = crate::db::Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let connection = database.connection_for_fsops();
        connection
            .execute_batch(
                "PRAGMA foreign_keys=OFF;
             WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000)
             INSERT INTO media_publication_jobs(intent_uuid,phase,created_at,updated_at)
             SELECT randomblob(16),'pending-hash',x,x FROM n;
             WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000)
             INSERT INTO media_chunks(chunk_hash,byte_length,state,created_at,updated_at)
             SELECT randomblob(32),8388608,'pending',x,x FROM n;",
            )
            .unwrap();
        let jobs: i64 = connection
            .query_row("SELECT COUNT(*) FROM media_publication_jobs", [], |r| {
                r.get(0)
            })
            .unwrap();
        let chunks: i64 = connection
            .query_row("SELECT COUNT(*) FROM media_chunks", [], |r| r.get(0))
            .unwrap();
        assert_eq!((jobs, chunks), (100_000, 1_000_000));
        let publication_plan = connection.prepare("EXPLAIN QUERY PLAN SELECT intent_uuid FROM media_publication_jobs WHERE phase NOT IN ('canonicalized','event-published','cancelled','archived') AND COALESCE(next_attempt_at,0)<=0 ORDER BY priority DESC,next_attempt_at,updated_at,intent_uuid LIMIT 1").unwrap().query_map([], |row| row.get::<_, String>(3)).unwrap().collect::<Result<Vec<_>,_>>().unwrap().join(" ");
        assert!(
            publication_plan.contains("idx_media_publication_work"),
            "{publication_plan}"
        );
    }

    #[tokio::test]
    async fn all_file_scan_publication_checkpoint_and_incremental_restore_without_echo() {
        use crate::sync::{capture, persistence};
        let temp = std::env::temp_dir().join(format!(
            "mangoo-all-file-roundtrip-{}",
            uuid::Uuid::now_v7()
        ));
        let source = temp.join("source");
        let target = temp.join("target");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        let long_name = format!("{}.bin", "x".repeat(246));
        let long_folder = "d".repeat(250);
        std::fs::create_dir_all(source.join(&long_folder)).unwrap();
        let long_path = format!("{long_folder}/{long_name}");
        let unicode_path = "nested/e\u{301}.txt";
        let corpus = [
            ("empty", Vec::new()),
            ("notes.bin", b"generic bytes".to_vec()),
            ("nested/a..b.jpg", b"corrupt but classified media".to_vec()),
            (long_path.as_str(), b"long path".to_vec()),
            (unicode_path, b"decomposed name".to_vec()),
            (".mangoo-staging/user.bin", b"user staging content".to_vec()),
        ];
        std::fs::create_dir_all(source.join(".mangoo-staging")).unwrap();
        std::fs::create_dir_all(target.join(".mangoo-staging")).unwrap();
        std::fs::write(
            target.join(".mangoo-staging/user.bin"),
            b"user staging content",
        )
        .unwrap();
        for (name, bytes) in &corpus {
            std::fs::write(source.join(name), bytes).unwrap();
        }
        let library = uuid::Uuid::now_v7();
        let library_root = uuid::Uuid::now_v7();
        let root_uuid = uuid::Uuid::now_v7();
        let root_folder = uuid::Uuid::now_v7();
        let binding = uuid::Uuid::now_v7();
        let mut db = crate::db::Database::open(temp.join("source.sqlite")).unwrap();
        db.apply_migrations().unwrap();
        db.ensure_selected_sync_root(
            library,
            root_uuid,
            root_folder,
            library_root,
            &source.to_string_lossy(),
            "Files",
            "Files",
            [7; 32],
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            1,
            "active",
            1,
            false,
        )
        .unwrap();
        db.initialize_sync_identity(library, library_root, uuid::Uuid::now_v7())
            .unwrap();
        db.begin_sync_capture(binding, uuid::Uuid::now_v7())
            .unwrap();
        let root = db.list_roots().unwrap().remove(0);
        crate::scan::scan_root(&mut db, &root).unwrap();
        capture::capture_enrolled_root_identities(db.connection_for_fsops()).unwrap();
        let writer = DbWriter::spawn(db);
        let provider = Arc::new(crate::sync::provider::InMemorySyncProvider::default());
        let context = TransportContext {
            namespace: library.to_string(),
            binding_epoch: binding.to_string(),
            transport_generation: 1,
        };
        let publication = MediaPublicationWorker::new(
            writer.clone(),
            provider.clone(),
            temp.join("objects"),
            context.clone(),
            MediaWorkLimiter::new(1, 8, 128 * 1024 * 1024).unwrap(),
        );
        for _ in 0..10 {
            if !publication.run_once().await.unwrap() {
                break;
            }
        }
        crate::sync::coordinator::publish_test_catalog_segments(
            &writer,
            provider.as_ref(),
            &context,
            temp.join("sync-spool"),
        )
        .await
        .unwrap();
        let checkpoint_dir = temp.join("sync-spool/checkpoints");
        let directory = checkpoint_dir.clone();
        let checkpoint = writer
            .write_authoritative(move |db| {
                let fence = persistence::capture_checkpoint_fence(
                    db.connection_for_fsops(),
                    Default::default(),
                    crate::sync::checkpoint::HostMetadataSnapshot {
                        library_uuid: library.to_string(),
                        name: "Files".into(),
                        icon: None,
                        binding_state: "synchronized".into(),
                        events: Vec::new(),
                    },
                )?;
                persistence::build_checkpoint(db.connection_for_fsops(), &fence, &directory)?;
                persistence::load_and_validate_checkpoint(
                    db.connection_for_fsops(),
                    fence.checkpoint_uuid,
                )
            })
            .await
            .unwrap();
        assert_eq!(checkpoint.projection.files.len(), 6);
        assert_eq!(
            checkpoint
                .projection
                .files
                .iter()
                .filter(|f| f.media_kind.is_some())
                .count(),
            1
        );
        let receiver_directory = temp.join("receiver");
        std::fs::create_dir_all(receiver_directory.join("sync-spool/indexes")).unwrap();
        for shard in &checkpoint.media_index_shards {
            let bytes = std::fs::read(
                temp.join("sync-spool/indexes")
                    .join(format!("{}.idx", shard.sha256)),
            )
            .unwrap();
            provider
                .object_put_immutable(
                    &context,
                    &shard.object_key,
                    &bytes,
                    crate::sync::media::parse_hash(&shard.sha256).unwrap(),
                )
                .await
                .unwrap();
            let downloaded = provider
                .object_get(&context, &shard.object_key, shard.byte_length)
                .await
                .unwrap();
            crate::sync::provider::validate_download(&downloaded, shard.byte_length).unwrap();
            std::fs::copy(
                &downloaded.path,
                receiver_directory
                    .join("sync-spool/indexes")
                    .join(format!("{}.idx", shard.sha256)),
            )
            .unwrap();
        }
        let receiver_path = receiver_directory.join("receiver.sqlite");
        persistence::materialize_staging_catalog_with_roots(
            &checkpoint,
            &receiver_path,
            library_root,
            uuid::Uuid::now_v7(),
            &[(root_uuid, target.clone())],
        )
        .unwrap();
        let receiver = crate::db::Database::open(&receiver_path).unwrap();
        receiver.apply_migrations().unwrap();
        receiver
            .ensure_selected_sync_root(
                library,
                root_uuid,
                root_folder,
                library_root,
                &target.to_string_lossy(),
                "Files",
                "Files",
                [7; 32],
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                1,
                "active",
                1,
                false,
            )
            .unwrap();
        receiver
            .begin_restored_sync_capture(binding, uuid::Uuid::now_v7())
            .unwrap();
        let marker =
            crate::sync::file_policy::test_root_marker(receiver.connection_for_fsops(), &target);
        std::fs::write(
            target.join(".mangoo-sync-root"),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        let receiver_writer = DbWriter::spawn(receiver);
        let restore = MediaRestoreWorker::new(
            receiver_writer.clone(),
            provider.clone(),
            temp.join("fresh-range-cache"),
            context.clone(),
            MediaWorkLimiter::new(1, 8, 128 * 1024 * 1024).unwrap(),
        );
        for _ in 0..10 {
            if !restore.run_once().await.unwrap() {
                break;
            }
        }
        for (name, bytes) in &corpus {
            assert_eq!(
                std::fs::read(target.join(name))
                    .unwrap_or_else(|error| panic!("restore corpus {name}: {error}")),
                *bytes,
                "{name}"
            );
        }
        // Queue paths are derived from the current Catalog projection each turn.
        // Exercise an unsupported remote destination through that projection,
        // so the normal enqueue step cannot erase the intended fault fixture.
        receiver_writer
            .write_authoritative(|db| {
                db.connection_for_fsops().execute(
                    "UPDATE files SET name='.mangoo-sync-root' WHERE name='notes.bin'",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(restore.run_once().await.unwrap());
        receiver_writer.write(|db| {
            let row:(String,Option<String>,bool)=db.connection_for_fsops().query_row("SELECT phase,error_code,owner_token IS NULL FROM media_materialization_jobs WHERE asset_uuid=(SELECT asset_uuid FROM files WHERE name='.mangoo-sync-root')",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            assert_eq!(row,("path-conflict".into(),Some("unsupported_destination".into()),true));
            Ok(())
        }).await.unwrap();
        receiver_writer.write_authoritative(|db| {
            db.connection_for_fsops().execute("UPDATE files SET name='notes.bin' WHERE name='.mangoo-sync-root'",[])?;
            db.connection_for_fsops().execute("UPDATE media_materialization_jobs SET next_attempt_at=NULL WHERE asset_uuid=(SELECT asset_uuid FROM files WHERE name='notes.bin')",[])?;
            Ok(())
        }).await.unwrap();
        let tiny_limiter =
            MediaWorkLimiter::new_with_temp_budget(1, 8, 128 * 1024 * 1024, 1).unwrap();
        let adopt = MediaRestoreWorker::new(
            receiver_writer.clone(),
            provider.clone(),
            temp.join("unused-cache"),
            context.clone(),
            tiny_limiter.clone(),
        );
        assert!(adopt.run_once().await.unwrap());
        assert_eq!(tiny_limiter.metrics().disk_rejections, 0);
        // Receiver remains alive while a new unsupported file is observed and
        // published; replaying its canonical events must create its local row.
        std::fs::write(source.join("later.hlsl"), b"new generic file").unwrap();
        let mut source_db = crate::db::Database::open(temp.join("source.sqlite")).unwrap();
        crate::scan::scan_root(&mut source_db, &root).unwrap();
        drop(source_db);
        writer
            .write_authoritative(|db| {
                capture::capture_enrolled_root_identities(db.connection_for_fsops())
            })
            .await
            .unwrap();
        for _ in 0..6 {
            if !publication.run_once().await.unwrap() {
                break;
            }
        }
        let events=writer.write(|db| {
            let mut query=db.connection_for_fsops().prepare("SELECT payload FROM sync_events ORDER BY device_uuid,stream_uuid,actor_epoch,actor_seq")?;
            let events=query.query_map([],|r|r.get::<_,Vec<u8>>(0))?.map(|r|crate::sync::codec::decode_event(&r?).map_err(anyhow::Error::from)).collect::<anyhow::Result<Vec<_>>>();
            events
        }).await.unwrap();
        receiver_writer
            .write_authoritative(move |db| {
                for chunk in events.chunks(100) {
                    db.apply_remote_sync_events(chunk)?;
                }
                Ok(())
            })
            .await
            .unwrap();
        for _ in 0..6 {
            if !restore.run_once().await.unwrap() {
                break;
            }
        }
        assert_eq!(
            std::fs::read(target.join("later.hlsl")).unwrap(),
            b"new generic file"
        );
        let mut receiver_db = crate::db::Database::open(&receiver_path).unwrap();
        let receiver_root = receiver_db.list_roots().unwrap().remove(0);
        crate::scan::scan_root(&mut receiver_db, &receiver_root).unwrap();
        let counts:(i64,i64,i64)=receiver_db.connection_for_fsops().query_row("SELECT (SELECT COUNT(*) FROM sync_publication_intents),(SELECT COUNT(*) FROM files),(SELECT COUNT(*) FROM media_materialization_jobs WHERE phase='complete')",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(counts, (0, 7, 7));
        let presentation:(i64,i64,i64)=receiver_db.connection_for_fsops().query_row("SELECT (SELECT COUNT(*) FROM media),(SELECT COUNT(*) FROM media_fts),(SELECT COUNT(*) FROM files WHERE browse_visible=0)",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
        assert_eq!(presentation,(1,7,6),"all restored files must remain searchable while only recognized media carries media presentation");
        assert!(std::fs::read_dir(&target).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".mangoo-work-")));
        drop(receiver_db);
        receiver_writer.shutdown().await.unwrap();
        writer.shutdown().await.unwrap();
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[tokio::test]
    async fn publication_worker_uploads_dependencies_before_releasing_event() {
        let root = std::env::temp_dir().join(format!(
            "mangoo-media-publication-{}",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.jpg");
        let source_bytes = vec![0x5a_u8; 5 * 1024 * 1024];
        let source_length = source_bytes.len() as i64;
        std::fs::write(&source, &source_bytes).unwrap();
        let database = crate::db::Database::open(root.join("catalog.sqlite")).unwrap();
        database.apply_migrations().unwrap();
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: root.to_string_lossy().into_owned(),
                display_name: "Media".into(),
            })
            .unwrap();
        let binding = uuid::Uuid::now_v7();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        database
            .begin_sync_capture(binding, uuid::Uuid::now_v7())
            .unwrap();
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root".into(),
                mtime: None,
            })
            .unwrap();
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: "source.jpg".into(),
                ext: "jpg".into(),
                size: source_length,
                mtime: 1,
                ctime: None,
                file_key: Some("source".into()),
            })
            .unwrap();
        database
            .create_sync_publication_intent(file_id, &source.to_string_lossy(), source_length)
            .unwrap();
        let writer = DbWriter::spawn(database);
        let provider = Arc::new(crate::sync::provider::InMemorySyncProvider::default());
        let preseal_owner = "preseal".to_owned();
        let preseal_owner_for_claim = preseal_owner.clone();
        let job = writer
            .write_authoritative(move |database| {
                crate::sync::media::inventory::ensure_publication_jobs(
                    database.connection_for_fsops(),
                    binding,
                    1,
                    unix_timestamp_millis(),
                )?;
                crate::sync::media::inventory::claim_publication_job(
                    database.connection_for_fsops(),
                    &preseal_owner_for_claim,
                    binding,
                    1,
                    unix_timestamp_millis(),
                    60_000,
                )?
                .ok_or_else(|| anyhow::anyhow!("publication job was not claimable"))
            })
            .await
            .unwrap();
        writer
            .write_authoritative({
                let preseal_owner = preseal_owner.clone();
                move |database| {
                    crate::sync::media::inventory::set_publication_phase(
                        database.connection_for_fsops(),
                        job.intent_uuid,
                        &preseal_owner,
                        crate::sync::media::inventory::PublicationPhase::Chunking,
                        unix_timestamp_millis(),
                    )
                }
            })
            .await
            .unwrap();
        let sealed = crate::sync::media::publication::build_publication(
            &job,
            &root.join("media-objects"),
            crate::sync::media::format::MediaHint {
                sniffed_kind: None,
                mime: None,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        writer
            .write_authoritative({
                let job = job.clone();
                let preseal_owner = preseal_owner.clone();
                move |database| {
                    crate::sync::media::publication::persist_publication(
                        database,
                        &job,
                        &preseal_owner,
                        &sealed,
                        unix_timestamp_millis(),
                    )
                }
            })
            .await
            .unwrap();
        writer
            .write_authoritative(move |database| {
                crate::sync::media::inventory::release_publication_job(
                    database.connection_for_fsops(),
                    job.intent_uuid,
                    &preseal_owner,
                )
            })
            .await
            .unwrap();
        let held_source = root.join("source-held-after-seal.jpg");
        std::fs::rename(&source, &held_source).unwrap();
        let worker = MediaPublicationWorker::new(
            writer.clone(),
            provider.clone(),
            root.join("media-objects"),
            TransportContext {
                namespace: uuid::Uuid::now_v7().to_string(),
                binding_epoch: binding.to_string(),
                transport_generation: 1,
            },
            MediaWorkLimiter::new(1, 8, 128 * 1024 * 1024).unwrap(),
        );
        assert!(worker.run_once().await.unwrap());
        std::fs::rename(&held_source, &source).unwrap();
        let counts = writer
            .write(|database| {
                database.connection_for_fsops().query_row(
                    "SELECT (SELECT COUNT(*) FROM sync_events),(SELECT COUNT(*) FROM sync_outbox),(SELECT COUNT(*) FROM media_publication_objects WHERE state='durable'),(SELECT COUNT(*) FROM media_publication_jobs WHERE phase='canonicalized')",
                    [],
                    |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?)),
                ).map_err(Into::into)
            })
            .await
            .unwrap();
        assert_eq!(counts, (1, 1, 2, 1));
        let active_restore_jobs = writer
            .write_authoritative(move |database| {
                crate::sync::media::inventory::enqueue_missing_materializations(
                    database.connection_for_fsops(),
                    binding,
                    1,
                    unix_timestamp_millis(),
                )?;
                let count = database.connection_for_fsops().query_row(
                    "SELECT COUNT(*) FROM media_materialization_jobs",
                    [],
                    |row| row.get::<_, i64>(0),
                )?;
                database
                    .connection_for_fsops()
                    .execute("DELETE FROM media_materialization_jobs", [])?;
                Ok(count)
            })
            .await
            .unwrap();
        assert_eq!(
            active_restore_jobs, 1,
            "portable checkpoint heads must enqueue even when projection status is active"
        );
        let restore_existing = MediaRestoreWorker::new(
            writer.clone(),
            provider.clone(),
            root.join("media-restore-cache"),
            TransportContext {
                namespace: uuid::Uuid::now_v7().to_string(),
                binding_epoch: binding.to_string(),
                transport_generation: 1,
            },
            MediaWorkLimiter::new(1, 8, 128 * 1024 * 1024).unwrap(),
        );
        assert!(restore_existing.run_once().await.unwrap());
        let existing_state = writer
            .write(|database| {
                database
                    .connection_for_fsops()
                    .query_row(
                        "SELECT phase,downloaded_bytes FROM media_materialization_jobs",
                        [],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .map_err(Into::into)
            })
            .await
            .unwrap();
        assert_eq!(existing_state, ("complete".into(), source_length));
        writer
            .write_authoritative(move |database| {
                database
                    .connection_for_fsops()
                    .execute("DELETE FROM media_materialization_jobs", [])?;
                Ok(())
            })
            .await
            .unwrap();
        std::fs::remove_file(&source).unwrap();
        writer
            .write_authoritative(move |database| {
                database
                    .connection_for_fsops()
                    .execute("UPDATE files SET status='missing' WHERE id=?1", [file_id])?;
                Ok(())
            })
            .await
            .unwrap();
        let restore = MediaRestoreWorker::new(
            writer.clone(),
            provider,
            root.join("media-restore-cache"),
            TransportContext {
                namespace: uuid::Uuid::now_v7().to_string(),
                binding_epoch: binding.to_string(),
                transport_generation: 1,
            },
            MediaWorkLimiter::new(1, 8, 128 * 1024 * 1024).unwrap(),
        );
        assert!(restore.run_once().await.unwrap());
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        let restore_state = writer
            .write(|database| {
                database
                    .connection_for_fsops()
                    .query_row(
                        "SELECT phase,downloaded_bytes FROM media_materialization_jobs",
                        [],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .map_err(Into::into)
            })
            .await
            .unwrap();
        assert_eq!(restore_state, ("complete".into(), source_length));
        let (stored_identity, stored_mtime) = writer
            .write(move |database| {
                database.connection_for_fsops().query_row(
                    "SELECT (SELECT COUNT(*) FROM sync_local_file_identities WHERE asset_uuid=f.asset_uuid AND content_fingerprint IS NOT NULL),f.mtime FROM files f WHERE f.id=?1",
                    [file_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                ).map_err(Into::into)
            })
            .await
            .unwrap();
        assert_eq!(
            stored_identity, 1,
            "materialization must register local identity"
        );
        let volume_identity = crate::library_path::local_file_identity(&root)
            .unwrap_or_else(|_| root.to_string_lossy().into_owned())
            .into_bytes();
        let file_identity = crate::library_path::local_file_identity(&source)
            .unwrap()
            .into_bytes();
        let content_fingerprint =
            crate::sync::identity::stable_content_fingerprint(&source).unwrap();
        let source_for_scan = source.clone();
        writer
            .write_authoritative(move |database| {
                database.commit_scan_batch(crate::db::ScanWriteBatch {
                    folders: Vec::new(),
                    files: vec![crate::db::ScanFileUpsert {
                        file: crate::db::FileUpsert {
                            root_id,
                            folder_id,
                            name: "source.jpg".into(),
                            ext: "jpg".into(),
                            size: source_length,
                            mtime: stored_mtime,
                            ctime: None,
                            file_key: Some(String::from_utf8_lossy(&file_identity).into_owned()),
                        },
                        media_kind: Some("image".into()),
                        external_identity: Some(crate::db::ExternalFileIdentity {
                            source_stamp: None,
                            rename_candidates: Default::default(),
                            volume_identity,
                            file_identity,
                            content_fingerprint: Some(content_fingerprint),
                            path: source_for_scan,
                        }),
                    }],
                    scan_generation: None,
                })?;
                Ok(())
            })
            .await
            .unwrap();
        let publication_count = writer
            .write(|database| {
                database
                    .connection_for_fsops()
                    .query_row("SELECT COUNT(*) FROM sync_publication_intents", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(Into::into)
            })
            .await
            .unwrap();
        assert_eq!(
            publication_count, 1,
            "restored content must not echo-publish"
        );
        writer.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn publication_worker_rejects_source_changed_after_capture() {
        let root = std::env::temp_dir().join(format!(
            "mangoo-media-source-race-{}",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.jpg");
        std::fs::write(&source, b"before").unwrap();
        let database = crate::db::Database::open(root.join("catalog.sqlite")).unwrap();
        database.apply_migrations().unwrap();
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: root.to_string_lossy().into_owned(),
                display_name: "Media".into(),
            })
            .unwrap();
        let binding = uuid::Uuid::now_v7();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        database
            .begin_sync_capture(binding, uuid::Uuid::now_v7())
            .unwrap();
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root".into(),
                mtime: None,
            })
            .unwrap();
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: "source.jpg".into(),
                ext: "jpg".into(),
                size: 6,
                mtime: 1,
                ctime: None,
                file_key: Some("source".into()),
            })
            .unwrap();
        database
            .create_sync_publication_intent(file_id, &source.to_string_lossy(), 6)
            .unwrap();
        let expected_hash = crate::sync::codec::sha256(b"before");
        database
            .connection_for_fsops()
            .execute(
                "UPDATE media_publication_jobs SET source_identity_json=json_object('expectedContentSha256',?1)",
                [super::super::hex_hash(&expected_hash)],
            )
            .unwrap();
        std::fs::write(&source, b"after!").unwrap();
        let writer = DbWriter::spawn(database);
        let worker = MediaPublicationWorker::new(
            writer.clone(),
            Arc::new(crate::sync::provider::InMemorySyncProvider::default()),
            root.join("media-objects"),
            TransportContext {
                namespace: uuid::Uuid::now_v7().to_string(),
                binding_epoch: binding.to_string(),
                transport_generation: 1,
            },
            MediaWorkLimiter::new(1, 8, 128 * 1024 * 1024).unwrap(),
        );
        assert!(worker.run_once().await.unwrap());
        let state = writer
            .write(|database| {
                database.connection_for_fsops().query_row(
                    "SELECT j.phase,j.error_code,(SELECT COUNT(*) FROM sync_events) FROM media_publication_jobs j",
                    [],
                    |row| Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?,row.get::<_,i64>(2)?)),
                ).map_err(Into::into)
            })
            .await
            .unwrap();
        assert_eq!(state, ("failed".into(), Some("source_changed".into()), 0));
        writer.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn materialization_disk_full_is_durable_and_never_publishes_target() {
        let root = std::env::temp_dir().join(format!(
            "mangoo-media-disk-full-{}",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let managed = root.join("managed");
        std::fs::create_dir_all(&managed).unwrap();
        let source = managed.join("restored.bin");
        let source_bytes = vec![0x4d_u8; 2 * 1024 * 1024 + 7];
        std::fs::write(&source, &source_bytes).unwrap();
        let database = crate::db::Database::open(root.join("catalog.sqlite")).unwrap();
        database.apply_migrations().unwrap();
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: managed.to_string_lossy().into_owned(),
                display_name: "Media".into(),
            })
            .unwrap();
        let binding = uuid::Uuid::now_v7();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        database
            .begin_sync_capture(binding, uuid::Uuid::now_v7())
            .unwrap();
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root".into(),
                mtime: None,
            })
            .unwrap();
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: "restored.bin".into(),
                ext: "bin".into(),
                size: source_bytes.len() as i64,
                mtime: 1,
                ctime: None,
                file_key: Some("disk-full".into()),
            })
            .unwrap();
        database
            .create_sync_publication_intent(
                file_id,
                &source.to_string_lossy(),
                source_bytes.len() as i64,
            )
            .unwrap();
        database
            .connection_for_fsops()
            .execute(
                "UPDATE asset_content_revisions SET source_mtime_ms=?1 WHERE content_revision_uuid=(SELECT content_revision_uuid FROM sync_publication_intents ORDER BY created_at DESC LIMIT 1)",
                [std::fs::metadata(&source)
                    .unwrap()
                    .modified()
                    .unwrap()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64],
            )
            .unwrap();
        let writer = DbWriter::spawn(database);
        let provider = Arc::new(crate::sync::provider::InMemorySyncProvider::default());
        let transport = TransportContext {
            namespace: uuid::Uuid::now_v7().to_string(),
            binding_epoch: binding.to_string(),
            transport_generation: 1,
        };
        let publication = MediaPublicationWorker::new(
            writer.clone(),
            provider.clone(),
            root.join("objects"),
            transport.clone(),
            MediaWorkLimiter::new(1, 8, 128 * 1024 * 1024).unwrap(),
        );
        for _ in 0..32 {
            if !publication.run_once().await.unwrap() {
                break;
            }
        }
        let published = writer
            .write(|database| {
                database.connection_for_fsops().query_row(
                    "SELECT phase||':'||COALESCE(error_code,''),(SELECT COUNT(*) FROM media_content_manifest_links),(SELECT COUNT(*) FROM media_manifests WHERE manifest_pack_hash IS NOT NULL)
                     FROM media_publication_jobs",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
                ).map_err(Into::into)
            })
            .await
            .unwrap();
        assert_eq!(published, ("canonicalized:".into(), 1, 1));
        std::fs::remove_file(&source).unwrap();
        let enqueued = writer
            .write_authoritative(move |database| {
                database
                    .connection_for_fsops()
                    .execute("UPDATE files SET status='missing' WHERE id=?1", [file_id])?;
                database
                    .connection_for_fsops()
                    .execute("DELETE FROM media_materialization_jobs", [])?;
                let enqueued = crate::sync::media::inventory::enqueue_missing_materializations(
                    database.connection_for_fsops(),
                    binding,
                    1,
                    unix_timestamp_millis(),
                )?;
                Ok(enqueued)
            })
            .await
            .unwrap();
        assert_eq!(
            enqueued, 1,
            "published media graph must enqueue a restore job"
        );
        let restore = MediaRestoreWorker::new(
            writer.clone(),
            provider,
            root.join("range-cache"),
            transport,
            MediaWorkLimiter::new(1, 8, 64 * 1024 * 1024).unwrap(),
        );
        std::fs::write(managed.join(".mangoo-test-disk-full"), b"fault injection").unwrap();
        let outcome = restore.run_once().await;
        assert!(outcome.unwrap());
        let state = writer
            .write(|database| {
                database
                    .connection_for_fsops()
                    .query_row(
                        "SELECT phase,error_code FROM media_materialization_jobs",
                        [],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .map_err(Into::into)
            })
            .await
            .unwrap();
        assert_eq!(state, ("disk-full".into(), "disk_full".into()));
        assert!(!root.join("managed").join("restored.bin").exists());
        writer.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn owned_staging_restart_preserves_legacy_pending_and_replaced_payloads() {
        let root =
            std::env::temp_dir().join(format!("mangoo-staging-restart-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&root).unwrap();
        let managed = root.join("managed");
        std::fs::create_dir(&managed).unwrap();
        std::fs::create_dir(managed.join(".mangoo-staging")).unwrap();
        let legacy = managed.join(".mangoo-staging/legacy.partial");
        std::fs::write(&legacy, b"legacy user bytes").unwrap();
        let db = crate::db::Database::open(root.join("catalog.sqlite")).unwrap();
        db.apply_migrations().unwrap();
        let revision = uuid::Uuid::now_v7();
        let asset = uuid::Uuid::now_v7();
        db.connection_for_fsops().execute("INSERT INTO asset_content_revisions(content_revision_uuid,asset_uuid,byte_size,local_mutation_generation,created_at) VALUES (?1,?2,0,1,1)",rusqlite::params![revision.as_bytes().as_slice(),asset.as_bytes().as_slice()]).unwrap();
        db.connection_for_fsops().execute("INSERT INTO media_materialization_jobs(content_revision_uuid,asset_uuid,destination_relpath,phase,expected_length,staging_relpath,owner_token,created_at,updated_at) VALUES (?1,?2,'data.bin','assembling',0,'.mangoo-staging/legacy.partial','owner',1,1)",rusqlite::params![revision.as_bytes().as_slice(),asset.as_bytes().as_slice()]).unwrap();
        let writer = DbWriter::spawn(db);
        let mut staging = prepare_owned_staging(&writer, &managed, revision, "owner").unwrap();
        assert_eq!(std::fs::read(&legacy).unwrap(), b"legacy user bytes");
        let directory = staging.directory.clone();
        let partial = directory.join("payload.partial");
        drop(staging.payload.take());
        std::fs::rename(&partial, directory.join("original.partial")).unwrap();
        std::fs::write(&partial, b"replacement").unwrap();
        drop(staging);
        assert!(prepare_owned_staging(&writer, &managed, revision, "owner").is_err());
        assert_eq!(std::fs::read(&partial).unwrap(), b"replacement");
        writer.write_authoritative_blocking(|db| {
            db.connection_for_fsops().execute("UPDATE media_materialization_jobs SET staging_directory_identity=NULL,staging_file_identity=NULL",[])?;
            Ok(())
        }).unwrap();
        assert!(prepare_owned_staging(&writer, &managed, revision, "owner")
            .err()
            .unwrap()
            .to_string()
            .contains("pending"));
        assert_eq!(std::fs::read(&partial).unwrap(), b"replacement");
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(writer.shutdown())
            .unwrap();
        let reopened = crate::db::Database::open(root.join("catalog.sqlite")).unwrap();
        let pending:(String,bool)=reopened.connection_for_fsops().query_row("SELECT staging_relpath,staging_directory_identity IS NULL FROM media_materialization_jobs",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert!(pending.1);
        assert!(managed.join(pending.0).exists());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn all_file_capacity_failure_releases_claim_and_allows_another_root() {
        let temporary =
            std::env::temp_dir().join(format!("mangoo-capacity-fairness-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&temporary).unwrap();
        let database = crate::db::Database::open(temporary.join("catalog.sqlite")).unwrap();
        database.apply_migrations().unwrap();
        let binding = uuid::Uuid::now_v7();
        let mut publications = Vec::new();
        for (name, size) in [("large", i64::MAX / 2), ("small", 5)] {
            let root_path = temporary.join(name);
            std::fs::create_dir_all(&root_path).unwrap();
            let source = root_path.join("data.bin");
            if name == "small" {
                std::fs::write(&source, b"bytes").unwrap();
            }
            // Capacity admission consumes captured metadata before opening the
            // source. The large fixture needs no huge allocation to prove a
            // rejected captured payload releases its claim for another root.
            let root = database
                .add_root(crate::db::NewRoot {
                    path: root_path.to_string_lossy().into_owned(),
                    display_name: name.into(),
                })
                .unwrap();
            let folder = database
                .upsert_folder(crate::db::FolderUpsert {
                    root_id: root,
                    parent_id: None,
                    name: String::new(),
                    path_hash: name.into(),
                    mtime: None,
                })
                .unwrap();
            let file = database
                .upsert_file(crate::db::FileUpsert {
                    root_id: root,
                    folder_id: folder,
                    name: "data.bin".into(),
                    ext: ".bin".into(),
                    size,
                    mtime: 0,
                    ctime: None,
                    file_key: None,
                })
                .unwrap();
            publications.push((file, source, size));
        }
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        database
            .begin_sync_capture(binding, uuid::Uuid::now_v7())
            .unwrap();
        let intents = publications
            .into_iter()
            .map(|(file, source, size)| {
                database
                    .create_sync_publication_intent(file, &source.to_string_lossy(), size)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        database
            .connection_for_fsops()
            .execute(
                "UPDATE media_publication_jobs SET priority=3 WHERE intent_uuid=?1",
                [intents[0].as_bytes().as_slice()],
            )
            .unwrap();
        let writer = DbWriter::spawn(database);
        let limiter = MediaWorkLimiter::new(1, 8, 128 * 1024 * 1024).unwrap();
        let worker = MediaPublicationWorker::new(
            writer.clone(),
            Arc::new(crate::sync::provider::InMemorySyncProvider::default()),
            temporary.join("objects"),
            TransportContext {
                namespace: uuid::Uuid::now_v7().to_string(),
                binding_epoch: binding.to_string(),
                transport_generation: 1,
            },
            limiter.clone(),
        );
        assert!(worker.run_once().await.unwrap());
        let large = intents[0];
        writer
            .write(move |db| {
                let state: (String, Option<String>, bool, bool) =
                    db.connection_for_fsops().query_row(
                        "SELECT phase,error_code,owner_token IS NULL,next_attempt_at>updated_at
                         FROM media_publication_jobs WHERE intent_uuid=?1",
                        [large.as_bytes().as_slice()],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )?;
                assert_eq!(
                    state,
                    ("failed".into(), Some("disk_capacity".into()), true, true)
                );
                Ok(())
            })
            .await
            .unwrap();
        assert!(worker.run_once().await.unwrap());
        let small = intents[1];
        writer
            .write(move |db| {
                let state: String = db.connection_for_fsops().query_row(
                    "SELECT phase FROM media_publication_jobs WHERE intent_uuid=?1",
                    [small.as_bytes().as_slice()],
                    |row| row.get(0),
                )?;
                assert_eq!(state, "canonicalized");
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(limiter.metrics().reserved_temp_bytes, 0);
        writer.shutdown().await.unwrap();
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn all_file_capacity_accounts_for_shared_and_distinct_volumes() {
        let gib = 1024u64 * 1024 * 1024;
        let large = 20 * gib;
        assert!(require_restore_capacity(50 * gib, 50 * gib, large, large, gib, true).is_ok());
        assert!(require_restore_capacity(25 * gib, 25 * gib, large, large, gib, true).is_err());
        assert!(require_restore_capacity(25 * gib, 25 * gib, large, large, gib, false).is_ok());
        assert!(require_restore_capacity(50 * gib, 19 * gib, large, large, gib, false).is_err());
        // A resumed cache needs only its remaining bytes; the destination still
        // needs the full payload even on a different volume.
        assert!(require_restore_capacity(2 * gib, 25 * gib, 0, large, gib, false).is_ok());
        let root = std::env::temp_dir();
        let limiter = MediaWorkLimiter::new_with_temp_budget(
            1,
            8,
            128 * 1024 * 1024,
            DEFAULT_TEMP_DISK_BUDGET,
        )
        .unwrap();
        assert!(require_publication_capacity(25 * gib, large, 128 * 1024 * 1024).is_ok());
        assert!(require_publication_capacity(19 * gib, large, 128 * 1024 * 1024).is_err());
        let reservation = limiter
            .reserve_temporary_disk(&root, 128 * 1024 * 1024)
            .unwrap();
        assert_eq!(limiter.metrics().reserved_temp_bytes, 128 * 1024 * 1024);
        drop(reservation);
        assert_eq!(limiter.metrics().reserved_temp_bytes, 0);
    }

    #[test]
    fn materialization_temp_disk_exhaustion_is_fail_closed() {
        let root = std::env::temp_dir().join(format!(
            "mangoo-media-temp-full-{}",
            uuid::Uuid::now_v7().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let limiter = MediaWorkLimiter::new_with_temp_budget(1, 8, 64 * 1024 * 1024, 1024).unwrap();
        let error = match limiter.reserve_temporary_disk(&root, 2048) {
            Ok(_) => panic!("reservation larger than the durable budget must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("temporary-disk reservation"));
        assert_eq!(limiter.metrics().disk_rejections, 1);
        assert!(!root.join("restored.bin").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
