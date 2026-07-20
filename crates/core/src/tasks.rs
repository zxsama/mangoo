use tokio::sync::{mpsc, Semaphore};

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use crate::db::{
    Database, DbWriter, TaskScanProgress, ThumbBlobRecord, ThumbnailStateUpsert, PURGE_BATCH_SIZE,
    ROOT_SCAN_TASK_PRIORITY, THUMBNAIL_SELECTED_PRIORITY,
};
use crate::scan::{ScanOptions, TaskAttemptGuard};
use crate::thumbnails::{
    generate_image_thumbnail_bytes_with_checkpoint, generate_video_thumbnail_bytes,
    ThumbnailDecision, ThumbnailPolicy, GENERATED_FORMAT, GRID_320_PROFILE,
};

const TASK_PROGRESS_FLUSH_INTERVAL_ITEMS: i64 = 100;
// Minimum wall-clock gap between real cancellation/attempt/yield checks during a
// scan. The scan hot path calls the cancellation callback multiple times per
// file; without throttling that runs 4 DB queries per call (cancel + attempt +
// two pending-task probes), dominating scan throughput on large NAS libraries.
// The checks' results barely change between files, so we do the real work at
// most once per this interval — cancel/yield still respond within ~200ms (well
// under any user-perceivable delay), and an attempt superseded between checks
// only lets the stale attempt write a few hundred more idempotent, scan-
// generation-stamped upserts before it's caught (reconciled away on completion).
const SCAN_CANCELLATION_CHECK_MIN_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(200);
const THUMBNAIL_SOURCE_CHANGED_ERROR: &str = "thumbnail source changed while processing";
const FFMPEG_NOT_AVAILABLE_ERROR: &str = "ffmpeg not available";
const THUMBNAIL_SELECTED_WORKER_CONCURRENCY: usize = 1;
// Foreground = the viewport thumbnails the user is actively waiting for
// (visible/ahead priority). Keep this generous so opening a folder fills its
// tiles quickly. Decode/resize is CPU work that runs in parallel; the tiny blob
// commit is serialized by the single writer actor, so more foreground workers
// just clear the viewport faster without any writer contention.
const THUMBNAIL_FOREGROUND_WORKER_CONCURRENCY: usize = 10;
// Background/bulk: kept low so a "generate all" storm does not swamp CPU.
const THUMBNAIL_BACKGROUND_WORKER_CONCURRENCY: usize = 2;
const IDLE_PENDING_TASK_DRAIN_INTERVAL_MS: u64 = 250;

pub type TaskSender = mpsc::Sender<i64>;

struct ThumbnailWorkerPermits {
    selected: Arc<Semaphore>,
    foreground: Arc<Semaphore>,
    background: Arc<Semaphore>,
}

#[derive(Debug)]
struct CooperativeRootScanYield {
    task_id: i64,
}

impl std::fmt::Display for CooperativeRootScanYield {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "root_scan task {} yielded to pending foreground work",
            self.task_id
        )
    }
}

impl std::error::Error for CooperativeRootScanYield {}

#[derive(Debug)]
struct CooperativeInteractiveFolderScanYield {
    task_id: i64,
}

impl std::fmt::Display for CooperativeInteractiveFolderScanYield {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "interactive_folder_scan task {} yielded to pending foreground work",
            self.task_id
        )
    }
}

impl std::error::Error for CooperativeInteractiveFolderScanYield {}

#[derive(Debug)]
struct CooperativeThumbnailYield {
    task_id: i64,
}

impl std::fmt::Display for CooperativeThumbnailYield {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "thumbnail task {} yielded to pending higher-priority work",
            self.task_id
        )
    }
}

impl std::error::Error for CooperativeThumbnailYield {}

#[cfg(test)]
type WorkerTaskHook = Arc<dyn Fn(&str, i64, Option<&Path>) + Send + Sync>;

#[cfg(test)]
static WORKER_TASK_HOOK: std::sync::LazyLock<std::sync::Mutex<Option<WorkerTaskHook>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

#[cfg(test)]
static TEST_HOOK_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

#[cfg(test)]
struct WorkerTaskHookGuard {
    _test_lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for WorkerTaskHookGuard {
    fn drop(&mut self) {
        *WORKER_TASK_HOOK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

#[cfg(test)]
fn set_worker_task_hook_for_test(hook: WorkerTaskHook) -> WorkerTaskHookGuard {
    let test_lock = TEST_HOOK_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *WORKER_TASK_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(hook);
    WorkerTaskHookGuard {
        _test_lock: test_lock,
    }
}

#[cfg(test)]
fn invoke_worker_task_hook(kind: &str, task_id: i64, database_path: Option<&Path>) {
    let hook = WORKER_TASK_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if let Some(hook) = hook {
        hook(kind, task_id, database_path);
    }
}

#[cfg(test)]
type ThumbnailProcessingCheckpointHook = Box<dyn Fn(i64, Option<&Path>) + Send>;

#[cfg(test)]
static THUMBNAIL_PROCESSING_CHECKPOINT_HOOK: std::sync::LazyLock<
    std::sync::Mutex<Option<ThumbnailProcessingCheckpointHook>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

#[cfg(test)]
struct ThumbnailProcessingCheckpointHookGuard {
    _test_lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for ThumbnailProcessingCheckpointHookGuard {
    fn drop(&mut self) {
        *THUMBNAIL_PROCESSING_CHECKPOINT_HOOK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

#[cfg(test)]
fn set_thumbnail_processing_checkpoint_hook_for_test(
    hook: ThumbnailProcessingCheckpointHook,
) -> ThumbnailProcessingCheckpointHookGuard {
    let test_lock = TEST_HOOK_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *THUMBNAIL_PROCESSING_CHECKPOINT_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(hook);
    ThumbnailProcessingCheckpointHookGuard {
        _test_lock: test_lock,
    }
}

#[cfg(test)]
fn invoke_thumbnail_processing_checkpoint_hook(task_id: i64, database_path: Option<&Path>) {
    let guard = THUMBNAIL_PROCESSING_CHECKPOINT_HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(hook) = guard.as_ref() {
        hook(task_id, database_path);
    }
}

#[cfg(not(test))]
fn invoke_thumbnail_processing_checkpoint_hook(_task_id: i64, _database_path: Option<&Path>) {}

#[cfg(not(test))]
fn invoke_worker_task_hook(_kind: &str, _task_id: i64, _database_path: Option<&Path>) {}

pub fn start_worker(worker_database: Database, writer: DbWriter) -> TaskSender {
    start_worker_with_ffmpeg(
        worker_database,
        writer,
        crate::thumbnails::ffmpeg_available(),
    )
}

/// Variant that lets callers (and tests) decide whether the worker should
/// attempt video poster generation. Used by Core startup to detect ffmpeg
/// once and forward the result without re-spawning per task.
pub fn start_worker_with_ffmpeg(
    worker_database: Database,
    writer: DbWriter,
    ffmpeg_available: bool,
) -> TaskSender {
    if !ffmpeg_available {
        tracing::warn!(
            "ffmpeg binary not found on PATH; video thumbnail tasks will fail with \"{FFMPEG_NOT_AVAILABLE_ERROR}\""
        );
    }
    let mut worker_database = worker_database;
    // Startup recovery runs once, before any task dispatch and before the writer
    // actor handles concurrent work, so it is safe to run directly on the worker
    // connection.
    if let Err(error) = crate::fsops::recover_pending_file_operations(&worker_database) {
        tracing::error!(
            %error,
            "startup file-operation journal recovery failed; scheduling bounded in-process repair"
        );
        crate::fsops::schedule_pending_file_operation_repair(&worker_database);
    }
    if let Err(error) = worker_database.recover_interrupted_notification_operations() {
        tracing::error!(
            %error,
            "startup notification recovery failed; Core will continue with observable pending state"
        );
    }
    worker_database
        .reset_running_root_scan_tasks_for_recovery()
        .expect("reset running root_scan tasks for startup recovery");
    worker_database
        .fail_pending_root_scan_tasks_for_disabled_roots()
        .expect("fail disabled root_scan tasks for startup recovery");
    worker_database
        .reset_running_thumbnail_tasks_for_recovery()
        .expect("reset running thumbnail tasks for startup recovery");
    worker_database
        .reset_running_root_purge_tasks_for_recovery()
        .expect("reset running root_purge tasks for startup recovery");
    worker_database
        .ensure_pending_delete_purge_tasks()
        .expect("re-enqueue root_purge for pending-delete roots at startup recovery");
    // WAL checkpoints are run by the single writer actor between job batches
    // (see DbWriter), so no separate checkpoint thread/connection is needed.
    //
    // Thumbnail blob commits are routed through `writer` (see
    // try_spawn_thumbnail_task): decode/resize stays parallel on the
    // spawn_blocking thread, only the tiny publish write is serialized by the
    // single writer actor, which checkpoints the WAL between batches.
    let (sender, mut receiver) = mpsc::channel(128);
    let (task_completion_sender, mut task_completion_receiver) = mpsc::unbounded_channel();
    let thumbnail_permits = ThumbnailWorkerPermits {
        selected: Arc::new(Semaphore::new(THUMBNAIL_SELECTED_WORKER_CONCURRENCY)),
        foreground: Arc::new(Semaphore::new(THUMBNAIL_FOREGROUND_WORKER_CONCURRENCY)),
        background: Arc::new(Semaphore::new(THUMBNAIL_BACKGROUND_WORKER_CONCURRENCY)),
    };
    let non_thumbnail_permit = Arc::new(Semaphore::new(1));
    tokio::spawn(async move {
        let mut in_flight_thumbnail_task_ids = HashSet::new();
        let mut in_flight_non_thumbnail_task_ids = HashSet::new();
        schedule_pending_worker_tasks_with_concurrency(
            &mut worker_database,
            &writer,
            ffmpeg_available,
            &thumbnail_permits,
            &non_thumbnail_permit,
            &task_completion_sender,
            &mut in_flight_thumbnail_task_ids,
            &mut in_flight_non_thumbnail_task_ids,
        );
        let mut idle_drain_interval = tokio::time::interval(std::time::Duration::from_millis(
            IDLE_PENDING_TASK_DRAIN_INTERVAL_MS,
        ));
        idle_drain_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut receiver_closed = false;
        loop {
            if receiver_closed
                && in_flight_thumbnail_task_ids.is_empty()
                && in_flight_non_thumbnail_task_ids.is_empty()
            {
                break;
            }

            tokio::select! {
                maybe_completed_task_id = task_completion_receiver.recv() => {
                    if let Some(completed_task_id) = maybe_completed_task_id {
                        in_flight_thumbnail_task_ids.remove(&completed_task_id);
                        in_flight_non_thumbnail_task_ids.remove(&completed_task_id);
                        schedule_pending_worker_tasks_with_concurrency(
                            &mut worker_database,
                            &writer,
                            ffmpeg_available,
                            &thumbnail_permits,
                            &non_thumbnail_permit,
                            &task_completion_sender,
                            &mut in_flight_thumbnail_task_ids,
                            &mut in_flight_non_thumbnail_task_ids,
                        );
                    }
                }
                maybe_task_id = receiver.recv(), if !receiver_closed => {
                    match maybe_task_id {
                        Some(task_id) => {
                            dispatch_worker_task_with_concurrency(
                                &mut worker_database,
                                &writer,
                                task_id,
                                ffmpeg_available,
                                &thumbnail_permits,
                                &non_thumbnail_permit,
                                &task_completion_sender,
                                &mut in_flight_thumbnail_task_ids,
                                &mut in_flight_non_thumbnail_task_ids,
                            );
                            schedule_pending_worker_tasks_with_concurrency(
                                &mut worker_database,
                                &writer,
                                ffmpeg_available,
                                &thumbnail_permits,
                                &non_thumbnail_permit,
                                &task_completion_sender,
                                &mut in_flight_thumbnail_task_ids,
                                &mut in_flight_non_thumbnail_task_ids,
                            );
                        }
                        None => {
                            receiver_closed = true;
                        }
                    }
                }
                _ = idle_drain_interval.tick() => {
                    schedule_pending_worker_tasks_with_concurrency(
                        &mut worker_database,
                        &writer,
                        ffmpeg_available,
                        &thumbnail_permits,
                        &non_thumbnail_permit,
                        &task_completion_sender,
                        &mut in_flight_thumbnail_task_ids,
                        &mut in_flight_non_thumbnail_task_ids,
                    );
                }
            }
        }
    });
    sender
}

#[cfg(test)]
fn replay_startup_recovery_task_ids_with_after_each(
    database: &mut Database,
    task_ids: &[i64],
    ffmpeg_available: bool,
    mut after_each: impl FnMut(&mut Database, i64),
) {
    for &task_id in task_ids {
        run_task(database, None, task_id, ffmpeg_available);
        after_each(database, task_id);
        drain_pending_persisted_tasks(database, ffmpeg_available);
    }
}

fn pending_drain_task_ids(database: &Database) -> anyhow::Result<Vec<i64>> {
    let mut task_ids = Vec::new();
    task_ids.extend(database.list_pending_foreground_thumbnail_task_ids()?);
    task_ids.extend(database.list_pending_interactive_folder_scan_task_ids()?);
    task_ids.extend(database.list_pending_root_scan_task_ids()?);
    task_ids.extend(database.list_pending_root_purge_task_ids()?);
    task_ids.extend(database.list_pending_background_thumbnail_task_ids()?);
    Ok(task_ids)
}

/// Run any pending root_scan and thumbnail tasks persisted in the database
/// that haven't already been observed via the in-memory channel. This covers
/// the case where the watcher persisted a pending task but `try_send` failed
/// (channel full or closed).
#[cfg(test)]
fn drain_pending_persisted_tasks(database: &mut Database, ffmpeg_available: bool) {
    let pending_task_ids = match pending_drain_task_ids(database) {
        Ok(ids) => ids,
        Err(error) => {
            tracing::warn!(%error, "failed to list pending tasks for drain");
            return;
        }
    };
    for task_id in pending_task_ids {
        run_task(database, None, task_id, ffmpeg_available);
    }
}

fn reconcile_in_flight_thumbnail_task_ids(
    database: &Database,
    in_flight_thumbnail_task_ids: &mut HashSet<i64>,
) {
    in_flight_thumbnail_task_ids.retain(|task_id| {
        database
            .get_task(*task_id)
            .ok()
            .flatten()
            .is_some_and(|task| task.kind == "thumbnail" && task.status == "running")
    });
}

fn reconcile_in_flight_non_thumbnail_task_ids(
    database: &Database,
    in_flight_non_thumbnail_task_ids: &mut HashSet<i64>,
) {
    in_flight_non_thumbnail_task_ids.retain(|task_id| {
        database
            .get_task(*task_id)
            .ok()
            .flatten()
            .is_some_and(|task| task.kind != "thumbnail" && task.status == "running")
    });
}

fn schedule_pending_worker_tasks_with_concurrency(
    database: &mut Database,
    writer: &DbWriter,
    ffmpeg_available: bool,
    thumbnail_permits: &ThumbnailWorkerPermits,
    non_thumbnail_permit: &Arc<Semaphore>,
    task_completion_sender: &mpsc::UnboundedSender<i64>,
    in_flight_thumbnail_task_ids: &mut HashSet<i64>,
    in_flight_non_thumbnail_task_ids: &mut HashSet<i64>,
) {
    reconcile_in_flight_thumbnail_task_ids(database, in_flight_thumbnail_task_ids);
    reconcile_in_flight_non_thumbnail_task_ids(database, in_flight_non_thumbnail_task_ids);
    let pending_task_ids = match pending_drain_task_ids(database) {
        Ok(ids) => ids,
        Err(error) => {
            tracing::warn!(%error, "failed to list pending tasks for worker drain");
            return;
        }
    };
    for task_id in pending_task_ids {
        dispatch_worker_task_with_concurrency(
            database,
            writer,
            task_id,
            ffmpeg_available,
            thumbnail_permits,
            non_thumbnail_permit,
            task_completion_sender,
            in_flight_thumbnail_task_ids,
            in_flight_non_thumbnail_task_ids,
        );
    }
}

#[cfg(test)]
fn schedule_pending_tasks_with_thumbnail_concurrency(
    database: &mut Database,
    writer: &DbWriter,
    ffmpeg_available: bool,
    thumbnail_permits: &ThumbnailWorkerPermits,
    thumbnail_completion_sender: &mpsc::UnboundedSender<i64>,
    in_flight_thumbnail_task_ids: &mut HashSet<i64>,
) {
    reconcile_in_flight_thumbnail_task_ids(database, in_flight_thumbnail_task_ids);
    let pending_task_ids = match pending_drain_task_ids(database) {
        Ok(ids) => ids,
        Err(error) => {
            tracing::warn!(%error, "failed to list pending tasks for concurrent drain");
            return;
        }
    };
    for task_id in pending_task_ids {
        dispatch_task_with_thumbnail_concurrency(
            database,
            writer,
            task_id,
            ffmpeg_available,
            thumbnail_permits,
            thumbnail_completion_sender,
            in_flight_thumbnail_task_ids,
        );
    }
}

#[cfg(test)]
fn dispatch_task_with_thumbnail_concurrency(
    database: &mut Database,
    writer: &DbWriter,
    task_id: i64,
    ffmpeg_available: bool,
    thumbnail_permits: &ThumbnailWorkerPermits,
    thumbnail_completion_sender: &mpsc::UnboundedSender<i64>,
    in_flight_thumbnail_task_ids: &mut HashSet<i64>,
) {
    match try_spawn_thumbnail_task(
        database,
        writer,
        task_id,
        ffmpeg_available,
        thumbnail_permits,
        thumbnail_completion_sender,
        in_flight_thumbnail_task_ids,
    ) {
        ThumbnailSpawnResult::Spawned | ThumbnailSpawnResult::Deferred => return,
        ThumbnailSpawnResult::NotThumbnail => {}
    }
    run_task(database, None, task_id, ffmpeg_available);
}

fn dispatch_worker_task_with_concurrency(
    database: &mut Database,
    writer: &DbWriter,
    task_id: i64,
    ffmpeg_available: bool,
    thumbnail_permits: &ThumbnailWorkerPermits,
    non_thumbnail_permit: &Arc<Semaphore>,
    task_completion_sender: &mpsc::UnboundedSender<i64>,
    in_flight_thumbnail_task_ids: &mut HashSet<i64>,
    in_flight_non_thumbnail_task_ids: &mut HashSet<i64>,
) {
    match try_spawn_thumbnail_task(
        database,
        writer,
        task_id,
        ffmpeg_available,
        thumbnail_permits,
        task_completion_sender,
        in_flight_thumbnail_task_ids,
    ) {
        ThumbnailSpawnResult::Spawned | ThumbnailSpawnResult::Deferred => return,
        ThumbnailSpawnResult::NotThumbnail => {}
    }
    match try_spawn_non_thumbnail_task(
        database,
        writer,
        task_id,
        ffmpeg_available,
        non_thumbnail_permit,
        task_completion_sender,
        in_flight_non_thumbnail_task_ids,
    ) {
        ThumbnailSpawnResult::Spawned | ThumbnailSpawnResult::Deferred => return,
        ThumbnailSpawnResult::NotThumbnail => {}
    }
    run_task(database, None, task_id, ffmpeg_available);
}

enum ThumbnailSpawnResult {
    /// This dispatcher does not own the task (wrong kind, missing, or not
    /// pending) — caller should try the next dispatcher / fall through.
    NotThumbnail,
    Spawned,
    /// Confirmed to be this dispatcher's kind of task, but it cannot run right
    /// now (permit exhausted, already in flight, or the per-task `reopen()`
    /// failed). The task is left `pending` and will be retried on the next
    /// scheduler tick — callers must `return` on this, never fall through to
    /// an inline run.
    Deferred,
}

fn try_spawn_thumbnail_task(
    database: &Database,
    writer: &DbWriter,
    task_id: i64,
    ffmpeg_available: bool,
    thumbnail_permits: &ThumbnailWorkerPermits,
    thumbnail_completion_sender: &mpsc::UnboundedSender<i64>,
    in_flight_thumbnail_task_ids: &mut HashSet<i64>,
) -> ThumbnailSpawnResult {
    if in_flight_thumbnail_task_ids.contains(&task_id) {
        return ThumbnailSpawnResult::Deferred;
    }

    let Ok(Some(task)) = database.get_task(task_id) else {
        return ThumbnailSpawnResult::NotThumbnail;
    };
    if task.kind != "thumbnail" || task.status != "pending" {
        return ThumbnailSpawnResult::NotThumbnail;
    }

    let semaphore = if task.priority >= THUMBNAIL_SELECTED_PRIORITY {
        thumbnail_permits.selected.clone()
    } else if task.priority > ROOT_SCAN_TASK_PRIORITY {
        thumbnail_permits.foreground.clone()
    } else {
        thumbnail_permits.background.clone()
    };
    let Ok(thumbnail_permit) = semaphore.try_acquire_owned() else {
        return ThumbnailSpawnResult::Deferred;
    };
    let Ok(Some(mut thumbnail_database)) = database.reopen() else {
        // This IS a genuine pending thumbnail task (kind/status already
        // confirmed above) — a reopen failure here must not fall through to
        // an inline run on the shared worker connection (that would bypass
        // the single writer actor and reintroduce unbounded WAL growth).
        // Leave it pending; it is retried on the next scheduler tick.
        crate::debug_log!(
            "thumbnail dispatch: reopen() failed for task {}, leaving pending for retry",
            task_id
        );
        return ThumbnailSpawnResult::Deferred;
    };

    in_flight_thumbnail_task_ids.insert(task_id);
    let thumbnail_completion_sender = thumbnail_completion_sender.clone();
    // Decode/resize runs on this blocking thread; only the tiny blob publish is
    // routed onto the single writer actor (which checkpoints the WAL), so the
    // per-task worker connection never grows an unbounded WAL.
    let writer = writer.clone();
    tokio::task::spawn_blocking(move || {
        let _thumbnail_permit = thumbnail_permit;
        run_task(
            &mut thumbnail_database,
            Some(&writer),
            task_id,
            ffmpeg_available,
        );
        let _ = thumbnail_completion_sender.send(task_id);
    });
    ThumbnailSpawnResult::Spawned
}

fn try_spawn_non_thumbnail_task(
    database: &Database,
    writer: &DbWriter,
    task_id: i64,
    ffmpeg_available: bool,
    non_thumbnail_permit: &Arc<Semaphore>,
    task_completion_sender: &mpsc::UnboundedSender<i64>,
    in_flight_non_thumbnail_task_ids: &mut HashSet<i64>,
) -> ThumbnailSpawnResult {
    if in_flight_non_thumbnail_task_ids.contains(&task_id) {
        return ThumbnailSpawnResult::Deferred;
    }

    let Ok(Some(task)) = database.get_task(task_id) else {
        return ThumbnailSpawnResult::NotThumbnail;
    };
    if task.kind == "thumbnail" || task.status != "pending" {
        return ThumbnailSpawnResult::NotThumbnail;
    }

    let Ok(non_thumbnail_permit) = non_thumbnail_permit.clone().try_acquire_owned() else {
        return ThumbnailSpawnResult::Deferred;
    };
    let Ok(Some(mut task_database)) = database.reopen() else {
        // Same reasoning as try_spawn_thumbnail_task: this is a confirmed
        // pending non-thumbnail (root_scan / interactive_folder_scan) task,
        // so a reopen failure must never fall through to an inline run.
        crate::debug_log!(
            "non-thumbnail dispatch: reopen() failed for task {}, leaving pending for retry",
            task_id
        );
        return ThumbnailSpawnResult::Deferred;
    };

    in_flight_non_thumbnail_task_ids.insert(task_id);
    let task_completion_sender = task_completion_sender.clone();
    // root_scan / interactive_folder_scan batch commits are routed onto the
    // single writer actor (see root_scan_options/interactive_scan_options),
    // which checkpoints the WAL between batches, so the per-task worker
    // connection never grows an unbounded WAL.
    let writer = writer.clone();
    tokio::task::spawn_blocking(move || {
        let _non_thumbnail_permit = non_thumbnail_permit;
        run_task(&mut task_database, Some(&writer), task_id, ffmpeg_available);
        let _ = task_completion_sender.send(task_id);
    });
    ThumbnailSpawnResult::Spawned
}

fn run_task(
    database: &mut Database,
    writer: Option<&DbWriter>,
    task_id: i64,
    ffmpeg_available: bool,
) {
    let attempt_generation = match database.current_task_attempt_generation(task_id) {
        Ok(attempt_generation) => Some(attempt_generation),
        Err(error) => {
            tracing::error!(
                task_id,
                %error,
                "failed to read task attempt before handling worker error"
            );
            None
        }
    };
    if let Err(error) = run_task_with_database(database, writer, task_id, ffmpeg_available) {
        if error.downcast_ref::<CooperativeRootScanYield>().is_some() {
            return;
        }
        if error
            .downcast_ref::<CooperativeInteractiveFolderScanYield>()
            .is_some()
        {
            return;
        }
        if error.downcast_ref::<CooperativeThumbnailYield>().is_some() {
            return;
        }
        if let Some(attempt_generation) = attempt_generation {
            handle_task_error_for_attempt(database, task_id, attempt_generation, error);
        } else {
            handle_task_error(database, task_id, error);
        }
    }
}

fn handle_task_error(database: &mut Database, task_id: i64, error: anyhow::Error) {
    match database.current_task_attempt_generation(task_id) {
        Ok(attempt_generation) => {
            handle_task_error_for_attempt(database, task_id, attempt_generation, error);
        }
        Err(database_error) => {
            tracing::error!(
                task_id,
                error = %error,
                %database_error,
                "failed to resolve task attempt while handling worker error"
            );
        }
    }
}

fn handle_task_error_for_attempt(
    database: &mut Database,
    task_id: i64,
    attempt_generation: i64,
    error: anyhow::Error,
) {
    match database.task_attempt_is_current(task_id, attempt_generation) {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(
                task_id,
                attempt_generation,
                "discarding worker error for stale task attempt"
            );
            return;
        }
        Err(database_error) => {
            tracing::error!(
                task_id,
                attempt_generation,
                %database_error,
                "failed to verify task attempt while handling worker error"
            );
            return;
        }
    }
    match database.task_is_cancelled(task_id) {
        Ok(true) => return,
        Ok(false) => {}
        Err(database_error) => {
            tracing::error!(
                task_id,
                attempt_generation,
                %database_error,
                "failed to check task cancellation while handling worker error"
            );
            return;
        }
    }
    let error_message = error.to_string();
    match database.get_task(task_id) {
        Ok(Some(task)) if task.kind == "thumbnail" => {
            if let Some(file_id) = task.file_id {
                let auxiliary_result = if error_message.contains(THUMBNAIL_SOURCE_CHANGED_ERROR) {
                    database.reset_thumbnail_after_stale_source_for_task_attempt(
                        file_id,
                        GRID_320_PROFILE,
                        &error_message,
                        task_id,
                        attempt_generation,
                    )
                } else {
                    let failure_kind =
                        crate::thumbnails::classify_thumbnail_failure(&error_message).as_str();
                    database.publish_thumbnail_failure_for_attempted_source_for_task_attempt(
                        file_id,
                        GRID_320_PROFILE,
                        task.thumbnail_source_fingerprint.as_deref(),
                        &error_message,
                        failure_kind,
                        task_id,
                        attempt_generation,
                    )
                };
                if let Err(database_error) = auxiliary_result {
                    tracing::warn!(
                        task_id,
                        attempt_generation,
                        %database_error,
                        "failed to update thumbnail failure state while handling worker error"
                    );
                }
            }
        }
        Ok(Some(_)) => {}
        Ok(None) => {
            tracing::error!(
                task_id,
                attempt_generation,
                "task disappeared while handling worker error"
            );
        }
        Err(database_error) => {
            tracing::error!(
                task_id,
                attempt_generation,
                %database_error,
                "failed to load task while handling worker error"
            );
        }
    }
    if let Err(database_error) =
        database.mark_task_failed_for_attempt(task_id, attempt_generation, &error_message)
    {
        tracing::error!(
            task_id,
            attempt_generation,
            error = %error_message,
            %database_error,
            recovery = "startup reset of running tasks",
            "failed to persist worker task failure; task will remain recoverable by startup"
        );
    }
}

fn yield_thumbnail_if_higher_priority_pending(
    database: &Database,
    task_id: i64,
    attempt_generation: i64,
) -> anyhow::Result<()> {
    let current_priority = database
        .get_task(task_id)?
        .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))?
        .priority;
    if database.has_pending_thumbnail_task_higher_priority(current_priority)? {
        database.yield_running_thumbnail_task_to_pending(
            task_id,
            attempt_generation,
            current_priority,
        )?;
        return Err(CooperativeThumbnailYield { task_id }.into());
    }
    Ok(())
}

fn run_thumbnail_processing_checkpoint(
    database: &Database,
    task_id: i64,
    attempt_generation: i64,
) -> anyhow::Result<()> {
    invoke_thumbnail_processing_checkpoint_hook(task_id, database.path());
    database.ensure_task_not_cancelled(task_id)?;
    if !database.task_attempt_is_current(task_id, attempt_generation)? {
        return Err(anyhow::anyhow!(
            "task {task_id} attempt superseded during thumbnail processing"
        ));
    }
    yield_thumbnail_if_higher_priority_pending(database, task_id, attempt_generation)
}

fn run_task_with_database(
    database: &mut Database,
    writer: Option<&DbWriter>,
    task_id: i64,
    ffmpeg_available: bool,
) -> anyhow::Result<()> {
    let task = database
        .get_task(task_id)?
        .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))?;
    if task.status == "cancelled" {
        return Ok(());
    }
    if task.status != "pending" {
        // Another worker path (channel send vs scheduler drain) already
        // moved this task out of `pending`. Skipping silently here is safe:
        // whichever path actually claimed the task is responsible for its
        // terminal status, and reporting "not pending" as a failure here
        // would clobber that path's outcome.
        return Ok(());
    }
    let attempt_generation = task.attempt_generation;
    match task.kind.as_str() {
        "root_scan" => {
            database.mark_task_running_for_attempt(task_id, attempt_generation)?;
            invoke_worker_task_hook("root_scan", task_id, database.path());
            let root_id = task
                .root_id
                .ok_or_else(|| anyhow::anyhow!("root_scan task missing root id: {task_id}"))?;
            let root = database
                .get_root(root_id)?
                .ok_or_else(|| anyhow::anyhow!("root not found: {root_id}"))?;
            if !root.enabled {
                return Err(anyhow::anyhow!("root is disabled: {root_id}"));
            }
            let progress_database = std::cell::RefCell::new(database.reopen()?);
            let mut latest_progress = TaskScanProgress {
                items_seen: 0,
                items_total: None,
                folders_seen: 0,
                media_files_seen: 0,
                skipped_files: 0,
            };
            let mut last_flushed_items = -TASK_PROGRESS_FLUSH_INTERVAL_ITEMS;
            let mut progress_callback = |progress: TaskScanProgress| {
                latest_progress = progress;
                if progress.items_seen - last_flushed_items >= TASK_PROGRESS_FLUSH_INTERVAL_ITEMS {
                    if let Some(progress_database) = progress_database.borrow_mut().as_mut() {
                        if progress_database
                            .update_task_scan_progress_for_attempt(
                                task_id,
                                attempt_generation,
                                progress,
                            )
                            .is_ok()
                        {
                            last_flushed_items = progress.items_seen;
                        }
                    }
                }
            };
            let mut last_cancellation_check: Option<std::time::Instant> = None;
            let mut cancellation_callback = || {
                // Time-throttle the real (multi-query) check: do it at most once
                // per SCAN_CANCELLATION_CHECK_MIN_INTERVAL. The hot path calls
                // this per-file; running four DB queries every file was the
                // dominant scan cost. Skipping is safe — cancel/yield still land
                // within the interval.
                let now = std::time::Instant::now();
                if let Some(last) = last_cancellation_check {
                    if now.duration_since(last) < SCAN_CANCELLATION_CHECK_MIN_INTERVAL {
                        return Ok(());
                    }
                }
                last_cancellation_check = Some(now);
                if let Some(progress_database) = progress_database.borrow_mut().as_mut() {
                    progress_database.ensure_task_not_cancelled(task_id)?;
                    if !progress_database.task_attempt_is_current(task_id, attempt_generation)? {
                        return Err(anyhow::anyhow!(
                            "task {task_id} attempt superseded while scanning"
                        ));
                    }
                    if progress_database.has_pending_interactive_folder_scan_task(root_id)? {
                        progress_database
                            .yield_running_root_scan_task_to_pending(task_id, attempt_generation)?;
                        return Err(CooperativeRootScanYield { task_id }.into());
                    }
                    if task.priority <= ROOT_SCAN_TASK_PRIORITY
                        && progress_database
                            .has_pending_foreground_thumbnail_task_for_root(root_id)?
                    {
                        progress_database
                            .yield_running_root_scan_task_to_pending_after_foreground_thumbnail(
                                task_id,
                                attempt_generation,
                            )?;
                        return Err(CooperativeRootScanYield { task_id }.into());
                    }
                }
                Ok(())
            };
            let summary = crate::scan::scan_root_with_options(
                database,
                &root,
                root_scan_options(
                    &mut progress_callback,
                    &mut cancellation_callback,
                    TaskAttemptGuard {
                        task_id,
                        attempt_generation,
                    },
                    writer,
                ),
            )?;
            drop(progress_callback);
            database.ensure_task_not_cancelled(task_id)?;
            if !database.task_attempt_is_current(task_id, attempt_generation)? {
                return Err(anyhow::anyhow!(
                    "task {task_id} attempt superseded before completion"
                ));
            }
            latest_progress = TaskScanProgress {
                items_seen: latest_progress.items_seen,
                items_total: None,
                folders_seen: summary.folders_seen as i64,
                media_files_seen: summary.media_files_seen as i64,
                skipped_files: summary.skipped_files as i64,
            };
            database.update_task_scan_progress_for_attempt(
                task_id,
                attempt_generation,
                latest_progress,
            )?;
            database.mark_task_succeeded_for_attempt(task_id, attempt_generation)?;
            Ok(())
        }
        "interactive_folder_scan" => {
            database.mark_task_running_for_attempt(task_id, attempt_generation)?;
            invoke_worker_task_hook("interactive_folder_scan", task_id, database.path());
            let root_id = task.root_id.ok_or_else(|| {
                anyhow::anyhow!("interactive_folder_scan task missing root id: {task_id}")
            })?;
            let folder_id = task.folder_id.ok_or_else(|| {
                anyhow::anyhow!("interactive_folder_scan task missing folder id: {task_id}")
            })?;
            let root = database
                .get_root(root_id)?
                .ok_or_else(|| anyhow::anyhow!("root not found: {root_id}"))?;
            if !root.enabled {
                return Err(anyhow::anyhow!("root is disabled: {root_id}"));
            }
            let folder_path = database
                .resolve_folder_source_path(folder_id)?
                .ok_or_else(|| anyhow::anyhow!("folder not found: {folder_id}"))?;
            let progress_database = std::cell::RefCell::new(database.reopen()?);
            let mut latest_progress = TaskScanProgress {
                items_seen: 0,
                items_total: None,
                folders_seen: 0,
                media_files_seen: 0,
                skipped_files: 0,
            };
            let mut last_flushed_items = -TASK_PROGRESS_FLUSH_INTERVAL_ITEMS;
            let mut progress_callback = |progress: TaskScanProgress| {
                latest_progress = progress;
                if progress.items_seen - last_flushed_items >= TASK_PROGRESS_FLUSH_INTERVAL_ITEMS {
                    if let Some(progress_database) = progress_database.borrow_mut().as_mut() {
                        if progress_database
                            .update_task_scan_progress_for_attempt(
                                task_id,
                                attempt_generation,
                                progress,
                            )
                            .is_ok()
                        {
                            last_flushed_items = progress.items_seen;
                        }
                    }
                }
            };
            let mut last_cancellation_check: Option<std::time::Instant> = None;
            let mut cancellation_callback = || {
                // Same per-file cancellation-check throttle as the root scan
                // (see SCAN_CANCELLATION_CHECK_MIN_INTERVAL).
                let now = std::time::Instant::now();
                if let Some(last) = last_cancellation_check {
                    if now.duration_since(last) < SCAN_CANCELLATION_CHECK_MIN_INTERVAL {
                        return Ok(());
                    }
                }
                last_cancellation_check = Some(now);
                if let Some(progress_database) = progress_database.borrow_mut().as_mut() {
                    progress_database.ensure_task_not_cancelled(task_id)?;
                    if !progress_database.task_attempt_is_current(task_id, attempt_generation)? {
                        return Err(anyhow::anyhow!(
                            "task {task_id} attempt superseded while scanning"
                        ));
                    }
                    if progress_database.has_pending_foreground_thumbnail_task()? {
                        progress_database.yield_running_interactive_folder_scan_task_to_pending(
                            task_id,
                            attempt_generation,
                        )?;
                        return Err(CooperativeInteractiveFolderScanYield { task_id }.into());
                    }
                }
                Ok(())
            };
            let summary = crate::scan::scan_folder_with_options(
                database,
                &root,
                folder_id,
                &folder_path,
                interactive_scan_options(
                    &mut progress_callback,
                    &mut cancellation_callback,
                    TaskAttemptGuard {
                        task_id,
                        attempt_generation,
                    },
                    writer,
                ),
            )?;
            drop(progress_callback);
            database.ensure_task_not_cancelled(task_id)?;
            if !database.task_attempt_is_current(task_id, attempt_generation)? {
                return Err(anyhow::anyhow!(
                    "task {task_id} attempt superseded before completion"
                ));
            }
            latest_progress = TaskScanProgress {
                items_seen: latest_progress.items_seen,
                items_total: None,
                folders_seen: summary.folders_seen as i64,
                media_files_seen: summary.media_files_seen as i64,
                skipped_files: summary.skipped_files as i64,
            };
            database.update_task_scan_progress_for_attempt(
                task_id,
                attempt_generation,
                latest_progress,
            )?;
            database.mark_task_succeeded_for_attempt(task_id, attempt_generation)?;
            Ok(())
        }
        "thumbnail" => {
            let cache_root = database.default_thumbnail_cache_dir();
            run_thumbnail_task_with_cache_for_attempt(
                database,
                writer,
                task_id,
                attempt_generation,
                &cache_root,
                ffmpeg_available,
            )
        }
        "root_purge" => {
            database.mark_task_running_for_attempt(task_id, attempt_generation)?;
            invoke_worker_task_hook("root_purge", task_id, database.path());
            let root_id = task
                .root_id
                .ok_or_else(|| anyhow::anyhow!("root_purge task missing root id: {task_id}"))?;
            loop {
                database.ensure_task_not_cancelled(task_id)?;
                // One bounded batch = one short writer job (or inline when there is no
                // writer, i.e. in-memory tests).
                let outcome = match writer {
                    Some(writer) => writer.write_blocking(move |db| {
                        db.purge_root_next_batch_for_task(
                            root_id,
                            task_id,
                            attempt_generation,
                            PURGE_BATCH_SIZE,
                        )
                    })?,
                    None => database.purge_root_next_batch_for_task(
                        root_id,
                        task_id,
                        attempt_generation,
                        PURGE_BATCH_SIZE,
                    )?,
                };
                if outcome.done {
                    // The final batch deleted this task's own row + the roots row, so
                    // there is nothing left to mark succeeded or to update progress on.
                    break;
                }
            }
            Ok(())
        }
        other => Err(anyhow::anyhow!("unsupported task kind: {other}")),
    }
}

fn root_scan_options<'a>(
    progress_callback: &'a mut dyn FnMut(TaskScanProgress),
    cancellation_callback: &'a mut dyn FnMut() -> anyhow::Result<()>,
    task_attempt_guard: TaskAttemptGuard,
    writer: Option<&DbWriter>,
) -> ScanOptions<'a> {
    ScanOptions {
        progress_callback: Some(progress_callback),
        cancellation_callback: Some(cancellation_callback),
        task_attempt_guard: Some(task_attempt_guard),
        writer: writer.cloned(),
        ..ScanOptions::background()
    }
}

fn interactive_scan_options<'a>(
    progress_callback: &'a mut dyn FnMut(TaskScanProgress),
    cancellation_callback: &'a mut dyn FnMut() -> anyhow::Result<()>,
    task_attempt_guard: TaskAttemptGuard,
    writer: Option<&DbWriter>,
) -> ScanOptions<'a> {
    ScanOptions {
        progress_callback: Some(progress_callback),
        cancellation_callback: Some(cancellation_callback),
        task_attempt_guard: Some(task_attempt_guard),
        writer: writer.cloned(),
        ..ScanOptions::interactive()
    }
}

#[cfg(test)]
fn run_thumbnail_task_with_cache(
    database: &mut Database,
    writer: Option<&DbWriter>,
    task_id: i64,
    cache_root: &Path,
    ffmpeg_available: bool,
) -> anyhow::Result<()> {
    let attempt_generation = database.current_task_attempt_generation(task_id)?;
    run_thumbnail_task_with_cache_for_attempt(
        database,
        writer,
        task_id,
        attempt_generation,
        cache_root,
        ffmpeg_available,
    )
}

fn run_thumbnail_task_with_cache_for_attempt(
    database: &mut Database,
    writer: Option<&DbWriter>,
    task_id: i64,
    attempt_generation: i64,
    cache_root: &Path,
    ffmpeg_available: bool,
) -> anyhow::Result<()> {
    run_thumbnail_task_with_cache_and_before_publish_for_attempt(
        database,
        writer,
        task_id,
        attempt_generation,
        cache_root,
        ffmpeg_available,
        |_| {},
    )
}

#[cfg(test)]
fn run_thumbnail_task_with_cache_and_before_publish(
    database: &mut Database,
    writer: Option<&DbWriter>,
    task_id: i64,
    cache_root: &Path,
    ffmpeg_available: bool,
    before_publish: impl FnOnce(&mut Database),
) -> anyhow::Result<()> {
    let attempt_generation = database.current_task_attempt_generation(task_id)?;
    run_thumbnail_task_with_cache_and_before_publish_for_attempt(
        database,
        writer,
        task_id,
        attempt_generation,
        cache_root,
        ffmpeg_available,
        before_publish,
    )
}

fn run_thumbnail_task_with_cache_and_before_publish_for_attempt(
    database: &mut Database,
    writer: Option<&DbWriter>,
    task_id: i64,
    attempt_generation: i64,
    _cache_root: &Path,
    ffmpeg_available: bool,
    before_publish: impl FnOnce(&mut Database),
) -> anyhow::Result<()> {
    let task = database
        .get_task(task_id)?
        .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))?;
    if task.status == "cancelled" {
        return Ok(());
    }
    if task.status != "pending" && task.status != "running" {
        return Err(anyhow::anyhow!(
            "task {task_id} is not pending or running; current status is {}",
            task.status
        ));
    }
    let file_id = task
        .file_id
        .ok_or_else(|| anyhow::anyhow!("thumbnail task missing file id: {task_id}"))?;
    let source = database
        .get_thumbnail_source(file_id)?
        .ok_or_else(|| anyhow::anyhow!("media item not found: {file_id}"))?;
    let policy = ThumbnailPolicy::grid_320();
    let source_fingerprint = source.source_fingerprint(GRID_320_PROFILE);
    if task.status == "pending" {
        database.mark_thumbnail_task_running_for_attempt(
            task_id,
            attempt_generation,
            &source_fingerprint,
        )?;
    }
    invoke_worker_task_hook("thumbnail", task_id, database.path());
    database.ensure_task_not_cancelled(task_id)?;
    if !database.task_attempt_is_current(task_id, attempt_generation)? {
        return Err(anyhow::anyhow!(
            "task {task_id} attempt superseded before thumbnail processing"
        ));
    }
    yield_thumbnail_if_higher_priority_pending(database, task_id, attempt_generation)?;

    let dimensions = if source.metadata_status.as_deref() == Some("ready") {
        (source.width, source.height)
    } else {
        (None, None)
    };

    if policy.initial_state(source.media_kind.as_deref(), dimensions.0, dimensions.1)
        == ThumbnailDecision::SkippedSmall
    {
        before_publish(database);
        database.ensure_task_not_cancelled(task_id)?;
        if !database.task_attempt_is_current(task_id, attempt_generation)? {
            return Err(anyhow::anyhow!(
                "task {task_id} attempt superseded before thumbnail publish"
            ));
        }
        let published = database
            .upsert_thumbnail_state_if_source_fingerprint_and_task_attempt_current(
                ThumbnailStateUpsert {
                    file_id,
                    profile: GRID_320_PROFILE.to_string(),
                    state: "skipped_small".to_string(),
                    cache_key: None,
                    width: None,
                    height: None,
                    byte_size: None,
                    error: None,
                    source_fingerprint: Some(source_fingerprint.clone()),
                },
                &source_fingerprint,
                task_id,
                attempt_generation,
            )?;
        if !published {
            return Err(anyhow::anyhow!(
                "{THUMBNAIL_SOURCE_CHANGED_ERROR}: {file_id}"
            ));
        }
        database.mark_task_succeeded_for_attempt(task_id, attempt_generation)?;
        return Ok(());
    }

    let source_path = database
        .resolve_file_source_path(file_id)?
        .ok_or_else(|| anyhow::anyhow!("source path not found for file {file_id}"))?;
    let media_kind = source.media_kind.as_deref();
    let generated = match media_kind {
        Some("image") => generate_image_thumbnail_bytes_with_checkpoint(&source_path, || {
            run_thumbnail_processing_checkpoint(database, task_id, attempt_generation)
        })?,
        Some("video") => {
            if !ffmpeg_available {
                return Err(anyhow::anyhow!(FFMPEG_NOT_AVAILABLE_ERROR));
            }
            generate_video_thumbnail_bytes(&source_path)?
        }
        Some(other) => {
            return Err(anyhow::anyhow!(
                "thumbnail decode failed: unsupported media kind \"{other}\""
            ));
        }
        None => {
            return Err(anyhow::anyhow!(
                "thumbnail decode failed: media kind not classified"
            ));
        }
    };
    before_publish(database);
    database.ensure_task_not_cancelled(task_id)?;
    if !database.task_attempt_is_current(task_id, attempt_generation)? {
        return Err(anyhow::anyhow!(
            "task {task_id} attempt superseded before thumbnail publish"
        ));
    }
    let now = unix_timestamp();
    // Captured from the decode this task already ran; persisted to media in the
    // publish transaction below so the layout engine gets real aspect ratios.
    let source_dimensions = Some((generated.source_width, generated.source_height));
    // Copy the scalar outputs before `generated.data` is moved into the blob so
    // the moved-into-writer closure and the inline path can both build the state
    // row without borrowing `generated` after the move.
    let generated_width = generated.width;
    let generated_height = generated.height;
    let generated_byte_size = generated.byte_size;
    // Blob commits run on the single writer connection (submitted via the
    // DbWriter). Decode/resize already happened above on this blocking thread;
    // only the commit below is serialized by the writer, so the per-task worker
    // connection never grows an unbounded WAL and can never hit
    // "database is locked".
    let blob = ThumbBlobRecord {
        file_id,
        profile: GRID_320_PROFILE.to_string(),
        data: generated.data,
        width: generated_width,
        height: generated_height,
        byte_size: generated_byte_size,
        output_format: GENERATED_FORMAT.to_string(),
        created_at: now,
        updated_at: now,
    };
    let state = ThumbnailStateUpsert {
        file_id,
        profile: GRID_320_PROFILE.to_string(),
        state: "ready".to_string(),
        cache_key: None,
        width: Some(generated_width),
        height: Some(generated_height),
        byte_size: Some(generated_byte_size),
        error: None,
        source_fingerprint: Some(source_fingerprint.clone()),
    };
    let published = match writer {
        Some(writer) => {
            // Production path: route the write onto the single writer actor so it
            // is serialized and the WAL is checkpointed between batches.
            crate::debug_log!(
                "thumbnail publish -> writer: task {} file {} blob {} bytes",
                task_id,
                file_id,
                blob.byte_size
            );
            let fingerprint = source_fingerprint.clone();
            writer.write_blocking(move |db| {
                db.publish_thumbnail_and_succeed_task_if_source_fingerprint_and_task_attempt_current(
                    state,
                    blob,
                    source_dimensions,
                    &fingerprint,
                    task_id,
                    attempt_generation,
                )
            })?
        }
        None => {
            // No writer (in-memory tests): publish inline on the per-task
            // connection exactly as before.
            database
                .publish_thumbnail_and_succeed_task_if_source_fingerprint_and_task_attempt_current(
                    state,
                    blob,
                    source_dimensions,
                    &source_fingerprint,
                    task_id,
                    attempt_generation,
                )?
        }
    };
    if !published {
        return Err(anyhow::anyhow!(
            "{THUMBNAIL_SOURCE_CHANGED_ERROR}: {file_id}"
        ));
    }
    Ok(())
}

fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::db::{
        FileUpsert, FolderUpsert, NewRoot, ROOT_SCAN_FOREGROUND_FAIRNESS_CONSUMED_PRIORITY,
    };
    use tokio::sync::Notify;
    use tokio::time::timeout;

    #[test]
    fn root_scan_options_reuses_background_profile_and_overrides_worker_hooks() {
        let mut progress_callback = |_progress: TaskScanProgress| {};
        let mut cancellation_callback = || Ok(());
        let guard = TaskAttemptGuard {
            task_id: 7,
            attempt_generation: 9,
        };

        let options = root_scan_options(
            &mut progress_callback,
            &mut cancellation_callback,
            guard,
            None,
        );

        assert_eq!(
            options.write_batch_size,
            ScanOptions::background().write_batch_size
        );
        assert!(options.progress_callback.is_some());
        assert!(options.cancellation_callback.is_some());
        assert_eq!(options.task_attempt_guard, Some(guard));
        assert!(options.writer.is_none());
    }

    #[test]
    fn pending_drain_task_ids_prioritize_foreground_thumbnail_before_interactive_and_background_work(
    ) {
        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Drain Order".to_string(),
                display_name: "Drain Order".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "drain-order-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let selected_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "selected".to_string(),
                path_hash: "drain-order-selected".to_string(),
                mtime: Some(2),
            })
            .expect("insert selected folder");

        let background_file_id = insert_pending_thumbnail_file(
            &database,
            root_id,
            root_folder_id,
            "background.jpg",
            "drain-order-background",
        );
        let ahead_file_id = insert_pending_thumbnail_file(
            &database,
            root_id,
            root_folder_id,
            "ahead.jpg",
            "drain-order-ahead",
        );
        let visible_file_id = insert_pending_thumbnail_file(
            &database,
            root_id,
            root_folder_id,
            "visible.jpg",
            "drain-order-visible",
        );
        let selected_file_id = insert_pending_thumbnail_file(
            &database,
            root_id,
            selected_folder_id,
            "selected.jpg",
            "drain-order-selected",
        );

        let background_task_id = database
            .request_thumbnail_task(
                background_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Background,
            )
            .expect("request background thumbnail")
            .task_id
            .expect("background task id");
        let ahead_task_id = database
            .request_thumbnail_task(
                ahead_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Ahead,
            )
            .expect("request ahead thumbnail")
            .task_id
            .expect("ahead task id");
        let visible_task_id = database
            .request_thumbnail_task(
                visible_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request visible thumbnail")
            .task_id
            .expect("visible task id");
        let selected_task_id = database
            .request_thumbnail_task(
                selected_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Selected,
            )
            .expect("request selected thumbnail")
            .task_id
            .expect("selected task id");
        let interactive_task_id = database
            .create_interactive_folder_scan_task(selected_folder_id)
            .expect("create interactive task");
        let root_task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");

        assert_eq!(
            pending_drain_task_ids(&database).expect("list drain task ids"),
            vec![
                selected_task_id,
                visible_task_id,
                ahead_task_id,
                interactive_task_id,
                root_task_id,
                background_task_id,
            ]
        );
    }

    #[test]
    fn thumbnail_task_writes_real_webp_and_marks_task_succeeded() {
        let temp_root = unique_temp_dir("media");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("image.jpg"), 800, 400);
        let cache_root = unique_temp_dir("cache");
        fs::create_dir_all(&cache_root).expect("create cache root");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "image.jpg");
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail task");

        let task_id = request.task_id.expect("thumbnail task id");
        run_thumbnail_task_with_cache(&mut database, None, task_id, &cache_root, true)
            .expect("run thumbnail task");

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "succeeded");
        assert_eq!(task.error, None);

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "ready");
        assert_eq!(thumbnail.output_format, crate::thumbnails::GENERATED_FORMAT);
        assert_eq!(thumbnail.cache_key, None);
        assert_eq!(thumbnail.served_by.as_deref(), Some("db_blob"));
        // Source is 800x400 (landscape, short side 400). After resizing the
        // short side to 320 the long side becomes 640. Real WebP, not
        // placeholder.
        assert_eq!(thumbnail.width, Some(640));
        assert_eq!(thumbnail.height, Some(320));
        assert!(thumbnail.byte_size.expect("byte size") > 0);

        let blob = database
            .get_thumb_blob(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("read thumb blob")
            .expect("thumb blob exists");
        assert_eq!(&blob.data[0..4], b"RIFF");
        assert_eq!(&blob.data[8..12], b"WEBP");
        assert!(fs::read_dir(&cache_root)
            .expect("read cache root")
            .next()
            .is_none());

        fs::remove_dir_all(temp_root).expect("cleanup media root");
        fs::remove_dir_all(cache_root).expect("cleanup cache root");
    }

    #[test]
    fn thumbnail_task_persists_source_dimensions_to_media() {
        // The scan ingest path leaves media dimensions `pending`; the thumbnail
        // task must persist the real source dimensions captured during the
        // decode it already performs, so justified-rows / masonry layouts get
        // true aspect ratios without a second decode pass.
        let temp_root = unique_temp_dir("media");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("portrait.jpg"), 600, 900);
        let cache_root = unique_temp_dir("cache");
        fs::create_dir_all(&cache_root).expect("create cache root");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "portrait.jpg");

        // Precondition: scan leaves dimensions unpopulated and metadata pending.
        let before = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists");
        assert_eq!(before.width, None);
        assert_eq!(before.height, None);
        assert_eq!(before.metadata_status.as_deref(), Some("pending"));

        let task_id = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail task")
            .task_id
            .expect("thumbnail task id");
        run_thumbnail_task_with_cache(&mut database, None, task_id, &cache_root, true)
            .expect("run thumbnail task");

        let after = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists");
        assert_eq!(after.width, Some(600));
        assert_eq!(after.height, Some(900));
        assert_eq!(after.metadata_status.as_deref(), Some("ready"));

        fs::remove_dir_all(temp_root).expect("cleanup media root");
        fs::remove_dir_all(cache_root).expect("cleanup cache root");
    }

    #[test]
    fn thumbnail_task_persists_grid_320_bytes_in_thumb_blobs() {
        let temp_root = unique_temp_dir("thumb_blob");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("image.jpg"), 800, 400);

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "image.jpg");
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");

        run_task(&mut database, None, request.task_id.expect("task id"), true);

        let blob = database
            .get_thumb_blob(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("read thumb blob")
            .expect("thumb blob exists");
        assert!(blob.data.starts_with(b"RIFF"));
        assert_eq!(&blob.data[8..12], b"WEBP");
        assert_eq!(blob.output_format, crate::thumbnails::GENERATED_FORMAT);
        assert_eq!(blob.width, 640);
        assert_eq!(blob.height, 320);
        assert_eq!(blob.byte_size, blob.data.len() as i64);

        fs::remove_dir_all(temp_root).expect("cleanup media root");
    }

    #[test]
    fn thumbnail_task_for_video_without_ffmpeg_marks_failed_with_clear_error() {
        let temp_root = unique_temp_dir("media");
        fs::create_dir_all(&temp_root).expect("create media root");
        // 1-byte placeholder masquerading as a video. We never invoke ffmpeg
        // because the worker is configured with ffmpeg_available = false.
        fs::write(temp_root.join("clip.mp4"), [0u8]).expect("write fake video");
        let cache_root = unique_temp_dir("cache");
        fs::create_dir_all(&cache_root).expect("create cache root");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file_with_kind(&database, &temp_root, "clip.mp4", "video");
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail task");
        let task_id = request.task_id.expect("thumbnail task id");

        run_task(&mut database, None, task_id, false);

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "failed");
        assert_eq!(task.error.as_deref(), Some(FFMPEG_NOT_AVAILABLE_ERROR));

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "failed");
        assert_eq!(thumbnail.error.as_deref(), Some(FFMPEG_NOT_AVAILABLE_ERROR));
        // No cache files should have been written.
        assert!(fs::read_dir(&cache_root)
            .expect("read cache root")
            .next()
            .is_none());

        fs::remove_dir_all(temp_root).expect("cleanup media root");
        fs::remove_dir_all(cache_root).expect("cleanup cache root");
    }

    #[test]
    fn startup_recovery_resets_stale_running_thumbnail_tasks() {
        let temp_root = unique_temp_dir("media");
        fs::create_dir_all(&temp_root).expect("create media root");
        fs::write(temp_root.join("image.jpg"), b"not a real image").expect("write media file");

        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "image.jpg");
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail task");
        let task_id = request.task_id.expect("task id");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark running");

        let reset = database
            .reset_running_thumbnail_tasks_for_recovery()
            .expect("reset thumbnail tasks");
        assert_eq!(reset, 1);
        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "pending");
        let pending = database
            .list_pending_thumbnail_task_ids()
            .expect("list pending thumbnails");
        assert_eq!(pending, vec![task_id]);

        fs::remove_dir_all(temp_root).expect("cleanup media root");
    }

    #[test]
    fn cancelled_thumbnail_tasks_are_not_recovered_or_run() {
        let temp_root = unique_temp_dir("media");
        fs::create_dir_all(&temp_root).expect("create media root");
        fs::write(temp_root.join("image.jpg"), b"not a real image").expect("write media file");
        let cache_root = unique_temp_dir("cache");
        fs::create_dir_all(&cache_root).expect("create cache root");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "image.jpg");
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail task");
        let task_id = request.task_id.expect("task id");
        database
            .cancel_task(task_id)
            .expect("cancel thumbnail task");

        assert_eq!(
            database
                .reset_running_thumbnail_tasks_for_recovery()
                .expect("recover thumbnails"),
            0
        );
        assert!(database
            .list_pending_thumbnail_task_ids()
            .expect("list pending thumbnails")
            .is_empty());

        run_task(&mut database, None, task_id, true);

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "cancelled");
        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_ne!(thumbnail.state, "ready");
        assert!(fs::read_dir(&cache_root)
            .expect("read cache root")
            .next()
            .is_none());

        fs::remove_dir_all(temp_root).expect("cleanup media root");
        fs::remove_dir_all(cache_root).expect("cleanup cache root");
    }

    #[test]
    fn thumbnail_task_does_not_publish_ready_when_source_changes_before_completion() {
        let temp_root = unique_temp_dir("media");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("image.jpg"), 400, 400);
        let cache_root = unique_temp_dir("cache");
        fs::create_dir_all(&cache_root).expect("create cache root");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "image.jpg");
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail task");
        let task_id = request.task_id.expect("thumbnail task id");

        let error = run_thumbnail_task_with_cache_and_before_publish(
            &mut database,
            None,
            task_id,
            &cache_root,
            true,
            |database: &mut Database| {
                let source = database
                    .get_thumbnail_source(file_id)
                    .expect("get source")
                    .expect("source exists");
                database
                    .upsert_file(FileUpsert {
                        root_id: source.root_id,
                        folder_id: source.folder_id,
                        name: source.name,
                        ext: ".jpg".to_string(),
                        size: source.size + 1,
                        mtime: source.mtime + 1,
                        ctime: None,
                        file_key: Some("worker-identity-changed".to_string()),
                    })
                    .expect("change source identity");
            },
        )
        .expect_err("stale thumbnail publish should fail");

        assert!(error.to_string().contains("source changed"));
        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_ne!(thumbnail.state, "ready");
        assert_eq!(thumbnail.cache_key, None);
        let cache_entries = fs::read_dir(&cache_root)
            .expect("read cache root")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect cache entries");
        assert_eq!(cache_entries.len(), 0);
        assert!(database
            .get_thumb_blob(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("read thumb blob")
            .is_none());

        fs::remove_dir_all(temp_root).expect("cleanup media root");
        fs::remove_dir_all(cache_root).expect("cleanup cache root");
    }

    #[test]
    fn stale_thumbnail_error_handler_leaves_current_source_regeneratable() {
        let temp_root = unique_temp_dir("media");
        fs::create_dir_all(&temp_root).expect("create media root");
        fs::write(temp_root.join("image.jpg"), b"not a real image").expect("write media file");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "image.jpg");
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail task");
        let task_id = request.task_id.expect("thumbnail task id");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark running");

        handle_task_error(
            &mut database,
            task_id,
            anyhow::anyhow!("thumbnail source changed while processing: {file_id}"),
        );

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "failed");
        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "pending");
        assert_eq!(thumbnail.cache_key, None);

        let next_request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail again");
        assert_eq!(next_request.thumbnail.state, "queued");
        assert!(next_request.queued);

        fs::remove_dir_all(temp_root).expect("cleanup media root");
    }

    #[test]
    fn thumbnail_error_handler_records_failed_with_attempt_fingerprint_when_source_matches() {
        let temp_root = unique_temp_dir("media");
        fs::create_dir_all(&temp_root).expect("create media root");
        fs::write(temp_root.join("image.jpg"), b"not a real image").expect("write media file");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "image.jpg");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail task");
        let task_id = request.task_id.expect("thumbnail task id");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark running");

        handle_task_error(&mut database, task_id, anyhow::anyhow!("decode failed"));

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "failed");
        assert_eq!(thumbnail.error.as_deref(), Some("decode failed"));
        assert_eq!(
            thumbnail.source_fingerprint.as_deref(),
            Some(source_fingerprint.as_str())
        );

        fs::remove_dir_all(temp_root).expect("cleanup media root");
    }

    #[test]
    fn thumbnail_error_handler_resets_generic_failure_when_source_changed() {
        let temp_root = unique_temp_dir("media");
        fs::create_dir_all(&temp_root).expect("create media root");
        fs::write(temp_root.join("image.jpg"), b"not a real image").expect("write media file");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "image.jpg");
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail task");
        let task_id = request.task_id.expect("thumbnail task id");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark running");
        let source = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists");
        database
            .upsert_file(FileUpsert {
                root_id: source.root_id,
                folder_id: source.folder_id,
                name: source.name,
                ext: ".jpg".to_string(),
                size: source.size + 1,
                mtime: source.mtime + 1,
                ctime: None,
                file_key: Some("worker-identity-changed".to_string()),
            })
            .expect("change source identity");

        handle_task_error(&mut database, task_id, anyhow::anyhow!("decode failed"));

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "pending");
        assert_eq!(thumbnail.cache_key, None);
        assert_eq!(thumbnail.source_fingerprint, None);

        let next_request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail again");
        assert_eq!(next_request.thumbnail.state, "queued");
        assert!(next_request.queued);

        fs::remove_dir_all(temp_root).expect("cleanup media root");
    }

    #[test]
    fn coalesced_running_thumbnail_failure_does_not_fail_new_source() {
        let temp_root = unique_temp_dir("media");
        fs::create_dir_all(&temp_root).expect("create media root");
        fs::write(temp_root.join("image.jpg"), b"not a real image").expect("write media file");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "image.jpg");
        let first = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail task");
        let task_id = first.task_id.expect("thumbnail task id");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark running");

        let old_source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        let source = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists");
        database
            .upsert_file(FileUpsert {
                root_id: source.root_id,
                folder_id: source.folder_id,
                name: source.name,
                ext: ".jpg".to_string(),
                size: source.size + 1,
                mtime: source.mtime + 1,
                ctime: None,
                file_key: Some("coalesced-current-source".to_string()),
            })
            .expect("change source identity");

        let coalesced = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail after source change");
        assert_eq!(coalesced.task_id, Some(task_id));
        assert!(!coalesced.queued);
        assert_ne!(
            coalesced.thumbnail.source_fingerprint.as_deref(),
            Some(old_source_fingerprint.as_str())
        );
        let coalesced_task = database
            .get_task(task_id)
            .expect("get coalesced task")
            .expect("coalesced task exists");
        assert_eq!(
            coalesced_task.thumbnail_source_fingerprint.as_deref(),
            Some(old_source_fingerprint.as_str())
        );

        handle_task_error(&mut database, task_id, anyhow::anyhow!("decode failed"));

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "pending");
        assert_eq!(thumbnail.cache_key, None);
        assert_eq!(thumbnail.source_fingerprint, None);

        let next_request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail again");
        assert_eq!(next_request.thumbnail.state, "queued");
        assert!(next_request.queued);

        fs::remove_dir_all(temp_root).expect("cleanup media root");
    }

    #[test]
    fn old_thumbnail_attempt_failure_does_not_mutate_retried_task_or_thumbnail() {
        let temp_root = unique_temp_dir("media");
        fs::create_dir_all(&temp_root).expect("create media root");
        fs::write(temp_root.join("image.jpg"), b"not a real image").expect("write media file");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "image.jpg");
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail task");
        let task_id = request.task_id.expect("thumbnail task id");
        let old_attempt = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;
        database
            .mark_thumbnail_task_running_for_attempt(task_id, old_attempt, "old-source")
            .expect("mark old thumbnail attempt running");
        database.cancel_task(task_id).expect("cancel task");
        let retried = database.retry_task(task_id).expect("retry task");
        assert_eq!(retried.status, "pending");
        assert!(retried.attempt_generation > old_attempt);

        handle_task_error_for_attempt(
            &mut database,
            task_id,
            old_attempt,
            anyhow::anyhow!("late decode failure"),
        );

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "pending");
        assert_eq!(task.error, None);
        assert_eq!(task.attempt_generation, retried.attempt_generation);

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "queued");
        assert_eq!(thumbnail.error, None);

        fs::remove_dir_all(temp_root).expect("cleanup media root");
    }

    #[test]
    fn worker_drain_picks_up_persisted_pending_root_scan_task_after_send_failed() {
        // Regression for Blocker 3: when the watcher persists a pending
        // root_scan task but the in-memory channel is Full or Closed, the
        // task must not be stranded. The worker drains pending persisted
        // tasks in scheduler order, so the next drain tick picks it up and
        // runs it.
        let temp_root = unique_temp_dir("root");
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("seed.jpg"), b"seed").expect("seed image");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "drain-test".to_string(),
            })
            .expect("add root");

        // Simulate the watcher persisting a pending root_scan task while
        // its `try_send` to the worker channel fails (channel is full or
        // closed). We model that by creating the bounded channel,
        // exhausting it with a sentinel id, and persisting the task in DB
        // without delivering it to the channel.
        let (sender, mut receiver) = mpsc::channel::<i64>(1);
        sender
            .try_send(987654)
            .expect("fill channel with sentinel id");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("persist pending root scan task");
        // The watcher would call `task_queue.try_send(task_id)` here — and
        // it would fail because the channel is full. The task remains in
        // DB as pending.
        assert!(sender.try_send(task_id).is_err());

        // Sanity: the task is in the persisted pending list.
        let pending = database
            .list_pending_root_scan_task_ids()
            .expect("list pending root scans");
        assert_eq!(pending, vec![task_id]);

        // Drop the channel so the worker can never receive task_id from it.
        drop(sender);
        // Drain the sentinel from the receiver to mirror the worker's
        // channel-fed behavior, but task_id was never sent.
        assert_eq!(receiver.try_recv().expect("sentinel id"), 987654);

        // Run the drain helper: it must pick up the persisted pending task
        // and execute it, producing a terminal task status.
        drain_pending_persisted_tasks(&mut database, true);

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert!(
            matches!(task.status.as_str(), "succeeded" | "failed"),
            "drained task should reach a terminal state, got {}",
            task.status
        );
        assert!(database
            .list_pending_root_scan_task_ids()
            .expect("list pending root scans after drain")
            .is_empty());

        fs::remove_dir_all(&temp_root).expect("cleanup temp root");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn worker_eventually_picks_up_persisted_pending_thumbnail_task_when_wakeup_is_missed() {
        let temp_root = unique_temp_dir("missed_thumbnail_wakeup_root");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("orphan.jpg"), 800, 400);

        let db_dir = unique_temp_dir("missed_thumbnail_wakeup_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("missed-thumbnail-wakeup.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let writer = crate::db::DbWriter::spawn(database.reopen().unwrap().unwrap());
        let _sender = start_worker_with_ffmpeg(database, writer, true);

        let request_database = Database::open(&db_path).expect("open request database");
        request_database
            .apply_migrations()
            .expect("apply request database migrations");
        let file_id = seed_media_file(&request_database, &temp_root, "orphan.jpg");
        let request = request_database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Background,
            )
            .expect("create pending thumbnail task");
        let task_id = request.task_id.expect("task id");

        let polling_database = Database::open(&db_path).expect("open polling database");
        polling_database
            .apply_migrations()
            .expect("apply polling database migrations");
        wait_for_task_statuses(
            &polling_database,
            &[(task_id, "succeeded")],
            Duration::from_secs(5),
        );
        let thumbnail = polling_database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "ready");

        fs::remove_dir_all(&temp_root).expect("cleanup temp root");
        let _ = fs::remove_dir_all(&db_dir);
    }

    #[test]
    fn interactive_folder_scan_task_ingests_only_selected_folder_media() {
        let temp_root = unique_temp_dir("interactive_root");
        let selected_dir = temp_root.join("selected");
        fs::create_dir_all(&selected_dir).expect("create selected dir");
        fs::write(temp_root.join("outside.jpg"), b"outside").expect("write outside image");
        fs::write(selected_dir.join("inside.jpg"), b"inside").expect("write inside image");

        let db_dir = unique_temp_dir("interactive_yield_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("interactive-yield.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Interactive Worker".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "interactive-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "selected".to_string(),
                path_hash: "interactive-selected".to_string(),
                mtime: Some(2),
            })
            .expect("insert selected folder");
        let task_id = database
            .create_interactive_folder_scan_task(folder_id)
            .expect("create interactive folder scan task");

        run_task(&mut database, None, task_id, true);

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.kind, "interactive_folder_scan");
        assert_eq!(task.status, "succeeded");
        assert_eq!(task.root_id, Some(root_id));
        assert_eq!(task.folder_id, Some(folder_id));

        let selected_media = database
            .list_media_page(crate::db::MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list selected media");
        assert_eq!(selected_media.items.len(), 1);
        assert_eq!(selected_media.items[0].name, "inside.jpg");

        let root_media = database
            .list_media_page(crate::db::MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list root media");
        assert_eq!(root_media.items.len(), 1);
        assert_eq!(root_media.items[0].name, "inside.jpg");

        fs::remove_dir_all(&temp_root).expect("cleanup temp root");
    }

    #[test]
    fn interactive_folder_scan_pending_causes_root_scan_task_to_yield_back_to_pending() {
        let temp_root = unique_temp_dir("interactive_yield_root");
        let selected_dir = temp_root.join("selected");
        fs::create_dir_all(&selected_dir).expect("create selected dir");
        fs::write(temp_root.join("outside.jpg"), b"outside").expect("write outside image");
        fs::write(selected_dir.join("inside.jpg"), b"inside").expect("write inside image");

        let db_dir = unique_temp_dir("interactive_yield_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("interactive-yield.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Interactive Yield".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .ensure_folder_chain_for_path(root_id, &temp_root, &selected_dir)
            .expect("seed folder chain");
        let root_task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        let old_attempt = database
            .get_task(root_task_id)
            .expect("get root task")
            .expect("root task exists")
            .attempt_generation;
        let interactive_task_id = database
            .create_interactive_folder_scan_task(folder_id)
            .expect("create interactive folder scan task");
        assert!(database
            .has_pending_interactive_folder_scan_task(root_id)
            .expect("main connection sees pending interactive task"));
        assert!(database
            .reopen()
            .expect("reopen database")
            .expect("file-backed database")
            .has_pending_interactive_folder_scan_task(root_id)
            .expect("reopened connection sees pending interactive task"));

        run_task(&mut database, None, root_task_id, true);

        let root_task = database
            .get_task(root_task_id)
            .expect("get root task")
            .expect("root task exists");
        assert_eq!(root_task.status, "pending");
        assert!(root_task.attempt_generation > old_attempt);
        assert_eq!(root_task.error, None);

        let interactive_task = database
            .get_task(interactive_task_id)
            .expect("get interactive task")
            .expect("interactive task exists");
        assert_eq!(interactive_task.status, "pending");

        assert_eq!(
            database
                .list_pending_interactive_folder_scan_task_ids()
                .expect("list pending interactive tasks"),
            vec![interactive_task_id]
        );
        assert_eq!(
            database
                .list_pending_root_scan_task_ids()
                .expect("list pending root tasks"),
            vec![root_task_id]
        );
        assert_eq!(
            database
                .get_root(root_id)
                .expect("get root")
                .expect("root exists")
                .last_scan_at,
            None
        );

        fs::remove_dir_all(&temp_root).expect("cleanup temp root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn interactive_folder_scan_runs_before_yielded_root_scan_retries() {
        let temp_root = unique_temp_dir("interactive_before_root");
        let selected_dir = temp_root.join("selected");
        fs::create_dir_all(&selected_dir).expect("create selected dir");
        fs::write(temp_root.join("outside.jpg"), b"outside").expect("write outside image");
        fs::write(selected_dir.join("inside.jpg"), b"inside").expect("write inside image");

        let db_dir = unique_temp_dir("interactive_before_root_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("interactive-before-root.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Interactive First".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .ensure_folder_chain_for_path(root_id, &temp_root, &selected_dir)
            .expect("seed folder chain");
        let root_task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        let interactive_task_id = database
            .create_interactive_folder_scan_task(folder_id)
            .expect("create interactive folder scan task");
        assert!(database
            .has_pending_interactive_folder_scan_task(root_id)
            .expect("main connection sees pending interactive task"));

        run_task(&mut database, None, root_task_id, true);

        let root_task = database
            .get_task(root_task_id)
            .expect("get root task")
            .expect("root task exists");
        assert_eq!(root_task.status, "pending");

        for task_id in database
            .list_pending_interactive_folder_scan_task_ids()
            .expect("list pending interactive tasks")
        {
            run_task(&mut database, None, task_id, true);
        }

        let selected_media = database
            .list_media_page(crate::db::MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list selected media after interactive scan");
        assert_eq!(selected_media.items.len(), 1);
        assert_eq!(selected_media.items[0].name, "inside.jpg");

        let root_task = database
            .get_task(root_task_id)
            .expect("get root task after interactive scan")
            .expect("root task exists after interactive scan");
        assert_eq!(root_task.status, "pending");

        run_task(&mut database, None, root_task_id, true);

        let final_root_media = database
            .list_media_page(crate::db::MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list root media after root retry");
        let names: Vec<&str> = final_root_media
            .items
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(names, vec!["inside.jpg", "outside.jpg"]);

        let interactive_task = database
            .get_task(interactive_task_id)
            .expect("get interactive task")
            .expect("interactive task exists");
        assert_eq!(interactive_task.status, "succeeded");

        fs::remove_dir_all(&temp_root).expect("cleanup temp root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn interactive_folder_scan_yields_to_pending_foreground_thumbnail() {
        let temp_root = unique_temp_dir("interactive_foreground_yield_root");
        let selected_dir = temp_root.join("selected");
        fs::create_dir_all(&selected_dir).expect("create selected dir");
        fs::write(selected_dir.join("inside.jpg"), b"inside").expect("write inside image");

        let db_dir = unique_temp_dir("interactive_foreground_yield_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("interactive-foreground-yield.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Interactive Foreground Yield".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .ensure_folder_chain_for_path(root_id, &temp_root, &selected_dir)
            .expect("seed folder chain");
        let interactive_task_id = database
            .create_interactive_folder_scan_task(folder_id)
            .expect("create interactive task");
        let old_attempt = database
            .get_task(interactive_task_id)
            .expect("get interactive task")
            .expect("interactive task exists")
            .attempt_generation;

        let foreground_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "inside.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 6,
                mtime: 1,
                ctime: None,
                file_key: Some("interactive-foreground".to_string()),
            })
            .expect("insert foreground file");
        database
            .upsert_media_kind(foreground_file_id, "image")
            .expect("insert foreground media kind");
        let foreground_task_id = database
            .request_thumbnail_task(
                foreground_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Selected,
            )
            .expect("request selected thumbnail")
            .task_id
            .expect("foreground thumbnail task id");

        run_task(&mut database, None, interactive_task_id, true);

        let interactive_task = database
            .get_task(interactive_task_id)
            .expect("get interactive task after run")
            .expect("interactive task exists after run");
        assert_eq!(interactive_task.status, "pending");
        assert!(interactive_task.attempt_generation > old_attempt);
        assert_eq!(interactive_task.error, None);

        let foreground_task = database
            .get_task(foreground_task_id)
            .expect("get foreground task")
            .expect("foreground task exists");
        assert_eq!(foreground_task.status, "pending");

        fs::remove_dir_all(&temp_root).expect("cleanup temp root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn background_revisit_keeps_interactive_folder_rows_queryable_after_root_yield() {
        let temp_root = unique_temp_dir("interactive_published_rows");
        let selected_dir = temp_root.join("selected");
        fs::create_dir_all(&selected_dir).expect("create selected dir");
        fs::write(temp_root.join("outside.jpg"), b"outside").expect("write outside image");
        fs::write(selected_dir.join("inside.jpg"), b"inside").expect("write inside image");

        let db_dir = unique_temp_dir("interactive_published_rows_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("interactive-published-rows.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Interactive Published Rows".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .ensure_folder_chain_for_path(root_id, &temp_root, &selected_dir)
            .expect("seed folder chain");

        let initial_interactive_task_id = database
            .create_interactive_folder_scan_task(folder_id)
            .expect("create initial interactive task");
        run_task(&mut database, None, initial_interactive_task_id, true);

        let initially_published = database
            .list_media_page(crate::db::MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list initially published interactive media");
        assert_eq!(initially_published.items.len(), 1);
        assert_eq!(initially_published.items[0].name, "inside.jpg");

        let root_task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        let old_attempt = database
            .get_task(root_task_id)
            .expect("get root task")
            .expect("root task exists")
            .attempt_generation;
        let follow_up_interactive_task_id = database
            .create_interactive_folder_scan_task(folder_id)
            .expect("create follow-up interactive task");
        assert!(database
            .has_pending_interactive_folder_scan_task(root_id)
            .expect("main connection sees pending interactive task"));

        run_task(&mut database, None, root_task_id, true);

        let root_task = database
            .get_task(root_task_id)
            .expect("get root task after yield")
            .expect("root task exists after yield");
        assert_eq!(root_task.status, "pending");
        assert!(root_task.attempt_generation > old_attempt);
        assert_eq!(root_task.error, None);

        let after_yield = database
            .list_media_page(crate::db::MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list interactive media after root yield");
        assert_eq!(after_yield.items.len(), 1);
        assert_eq!(after_yield.items[0].name, "inside.jpg");

        drain_pending_persisted_tasks(&mut database, true);

        let interactive_task = database
            .get_task(follow_up_interactive_task_id)
            .expect("get follow-up interactive task")
            .expect("follow-up interactive task exists");
        assert_eq!(interactive_task.status, "succeeded");

        let after_revisit = database
            .list_media_page(crate::db::MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list interactive media after revisit drain");
        assert_eq!(after_revisit.items.len(), 1);
        assert_eq!(after_revisit.items[0].name, "inside.jpg");

        fs::remove_dir_all(&temp_root).expect("cleanup temp root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn foreground_thumbnail_pending_causes_root_scan_task_to_yield_back_to_pending() {
        let temp_root = unique_temp_dir("foreground_thumbnail_yield_root");
        fs::create_dir_all(&temp_root).expect("create root");
        fs::write(temp_root.join("outside.jpg"), b"outside").expect("write outside image");

        let db_dir = unique_temp_dir("foreground_thumbnail_yield_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("foreground-thumbnail-yield.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Foreground Thumbnail Yield".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .ensure_folder_chain_for_path(root_id, &temp_root, &temp_root)
            .expect("seed root folder");
        let root_task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        let old_attempt = database
            .get_task(root_task_id)
            .expect("get root task")
            .expect("root task exists")
            .attempt_generation;

        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id: root_folder_id,
                name: "foreground.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 123,
                mtime: 321,
                ctime: None,
                file_key: Some("foreground-thumbnail".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");
        let thumb_request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Selected,
            )
            .expect("request thumbnail");
        let thumbnail_task_id = thumb_request.task_id.expect("thumbnail task id");

        run_task(&mut database, None, root_task_id, true);

        let root_task = database
            .get_task(root_task_id)
            .expect("get root task")
            .expect("root task exists");
        assert_eq!(root_task.status, "pending");
        assert!(root_task.attempt_generation > old_attempt);
        assert_eq!(root_task.error, None);

        let thumbnail_task = database
            .get_task(thumbnail_task_id)
            .expect("get thumbnail task")
            .expect("thumbnail task exists");
        assert_eq!(thumbnail_task.status, "pending");

        fs::remove_dir_all(&temp_root).expect("cleanup root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn running_background_thumbnail_yields_to_pending_selected_thumbnail_before_generation() {
        let temp_root = unique_temp_dir("thumbnail_priority_pre_generation_yield_root");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("background.jpg"), 800, 400);
        write_test_image(&temp_root.join("selected.jpg"), 800, 400);
        let cache_root = unique_temp_dir("thumbnail_priority_pre_generation_yield_cache");
        fs::create_dir_all(&cache_root).expect("create cache root");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Thumbnail Pre Generation Yield".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "thumbnail-pre-generation-yield-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let background_file_id = insert_pending_thumbnail_file(
            &database,
            root_id,
            folder_id,
            "background.jpg",
            "thumbnail-pre-generation-yield-background",
        );
        let selected_file_id = insert_pending_thumbnail_file(
            &database,
            root_id,
            folder_id,
            "selected.jpg",
            "thumbnail-pre-generation-yield-selected",
        );

        let background_task_id = database
            .request_thumbnail_task(
                background_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Background,
            )
            .expect("request background thumbnail")
            .task_id
            .expect("background task id");
        let old_attempt = database
            .get_task(background_task_id)
            .expect("get background task")
            .expect("background task exists")
            .attempt_generation;
        let selected_task_id = database
            .request_thumbnail_task(
                selected_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Selected,
            )
            .expect("request selected thumbnail before background starts")
            .task_id
            .expect("selected task id");

        let error = run_thumbnail_task_with_cache(
            &mut database,
            None,
            background_task_id,
            &cache_root,
            true,
        )
        .expect_err("background thumbnail should yield before generation");

        assert!(
            error.to_string().contains("yield"),
            "expected cooperative yield error, got {error}"
        );

        let background_task = database
            .get_task(background_task_id)
            .expect("get background task after yield")
            .expect("background task exists after yield");
        assert_eq!(background_task.status, "pending");
        assert!(background_task.attempt_generation > old_attempt);
        assert_eq!(background_task.error, None);

        let selected_task = database
            .get_task(selected_task_id)
            .expect("get selected task")
            .expect("selected task exists");
        assert_eq!(selected_task.status, "pending");
        assert_eq!(
            selected_task.priority,
            crate::db::THUMBNAIL_SELECTED_PRIORITY
        );

        let background_thumbnail = database
            .get_thumbnail(background_file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get background thumbnail")
            .expect("background thumbnail exists");
        assert_ne!(background_thumbnail.state, "ready");

        fs::remove_dir_all(&temp_root).expect("cleanup media root");
        fs::remove_dir_all(&cache_root).expect("cleanup cache root");
    }

    #[test]
    fn running_background_thumbnail_publishes_generated_bytes_when_selected_arrives_before_publish()
    {
        let temp_root = unique_temp_dir("thumbnail_priority_yield_root");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("background.jpg"), 800, 400);
        write_test_image(&temp_root.join("selected.jpg"), 800, 400);
        let cache_root = unique_temp_dir("thumbnail_priority_yield_cache");
        fs::create_dir_all(&cache_root).expect("create cache root");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Thumbnail Yield".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "thumbnail-yield-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let background_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "background.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 10,
                mtime: 1,
                ctime: None,
                file_key: Some("thumbnail-yield-background".to_string()),
            })
            .expect("insert background file");
        let selected_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "selected.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 11,
                mtime: 2,
                ctime: None,
                file_key: Some("thumbnail-yield-selected".to_string()),
            })
            .expect("insert selected file");
        database
            .upsert_media_kind(background_file_id, "image")
            .expect("insert background media kind");
        database
            .upsert_media_kind(selected_file_id, "image")
            .expect("insert selected media kind");

        let background_task_id = database
            .request_thumbnail_task(
                background_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Background,
            )
            .expect("request background thumbnail")
            .task_id
            .expect("background task id");
        run_thumbnail_task_with_cache_and_before_publish(
            &mut database,
            None,
            background_task_id,
            &cache_root,
            true,
            |database| {
                database
                    .request_thumbnail_task(
                        selected_file_id,
                        crate::thumbnails::GRID_320_PROFILE,
                        crate::db::ThumbnailTaskPriority::Selected,
                    )
                    .expect("request selected thumbnail while background is running");
            },
        )
        .expect("background thumbnail should publish after generating bytes");

        let background_task = database
            .get_task(background_task_id)
            .expect("get background task after publish")
            .expect("background task exists after publish");
        assert_eq!(background_task.status, "succeeded");
        assert_eq!(background_task.error, None);

        let selected_task = database
            .list_tasks()
            .expect("list tasks")
            .into_iter()
            .find(|task| task.kind == "thumbnail" && task.file_id == Some(selected_file_id))
            .expect("selected thumbnail task exists");
        assert_eq!(selected_task.status, "pending");
        assert_eq!(
            selected_task.priority,
            crate::db::THUMBNAIL_SELECTED_PRIORITY
        );

        let background_thumbnail = database
            .get_thumbnail(background_file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get background thumbnail")
            .expect("background thumbnail exists");
        assert_eq!(background_thumbnail.state, "ready");
        assert_eq!(background_thumbnail.served_by.as_deref(), Some("db_blob"));
        assert!(database
            .get_thumb_blob(background_file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("read background thumb blob")
            .is_some());

        fs::remove_dir_all(&temp_root).expect("cleanup media root");
        fs::remove_dir_all(&cache_root).expect("cleanup cache root");
    }

    #[test]
    fn running_bulk_thumbnail_publishes_generated_bytes_when_selected_arrives_before_publish() {
        let temp_root = unique_temp_dir("thumbnail_bulk_priority_yield_root");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("bulk.jpg"), 800, 400);
        write_test_image(&temp_root.join("selected.jpg"), 800, 400);
        let cache_root = unique_temp_dir("thumbnail_bulk_priority_yield_cache");
        fs::create_dir_all(&cache_root).expect("create cache root");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Thumbnail Bulk Yield".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "thumbnail-bulk-yield-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let bulk_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "bulk.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 10,
                mtime: 1,
                ctime: None,
                file_key: Some("thumbnail-yield-bulk".to_string()),
            })
            .expect("insert bulk file");
        let selected_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "selected.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 11,
                mtime: 2,
                ctime: None,
                file_key: Some("thumbnail-yield-selected".to_string()),
            })
            .expect("insert selected file");
        database
            .upsert_media_kind(bulk_file_id, "image")
            .expect("insert bulk media kind");
        database
            .upsert_media_kind(selected_file_id, "image")
            .expect("insert selected media kind");

        let bulk_task_id = database
            .request_thumbnail_task(
                bulk_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Bulk,
            )
            .expect("request bulk thumbnail")
            .task_id
            .expect("bulk task id");
        run_thumbnail_task_with_cache_and_before_publish(
            &mut database,
            None,
            bulk_task_id,
            &cache_root,
            true,
            |database| {
                database
                    .request_thumbnail_task(
                        selected_file_id,
                        crate::thumbnails::GRID_320_PROFILE,
                        crate::db::ThumbnailTaskPriority::Selected,
                    )
                    .expect("request selected thumbnail while bulk is running");
            },
        )
        .expect("bulk thumbnail should publish after generating bytes");

        let bulk_task = database
            .get_task(bulk_task_id)
            .expect("get bulk task after publish")
            .expect("bulk task exists after publish");
        assert_eq!(bulk_task.status, "succeeded");
        assert_eq!(bulk_task.priority, crate::db::THUMBNAIL_BULK_PRIORITY);
        assert_eq!(bulk_task.error, None);

        let selected_task = database
            .list_tasks()
            .expect("list tasks")
            .into_iter()
            .find(|task| task.kind == "thumbnail" && task.file_id == Some(selected_file_id))
            .expect("selected thumbnail task exists");
        assert_eq!(selected_task.status, "pending");
        assert_eq!(
            selected_task.priority,
            crate::db::THUMBNAIL_SELECTED_PRIORITY
        );

        let bulk_thumbnail = database
            .get_thumbnail(bulk_file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get bulk thumbnail")
            .expect("bulk thumbnail exists");
        assert_eq!(bulk_thumbnail.state, "ready");
        assert_eq!(bulk_thumbnail.served_by.as_deref(), Some("db_blob"));
        assert!(database
            .get_thumb_blob(bulk_file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("read bulk thumb blob")
            .is_some());

        fs::remove_dir_all(&temp_root).expect("cleanup media root");
        fs::remove_dir_all(&cache_root).expect("cleanup cache root");
    }

    #[test]
    fn running_background_thumbnail_for_same_file_publishes_when_selected_arrives_before_publish() {
        let temp_root = unique_temp_dir("thumbnail_same_file_priority_yield_root");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("same-file.jpg"), 800, 400);
        let cache_root = unique_temp_dir("thumbnail_same_file_priority_yield_cache");
        fs::create_dir_all(&cache_root).expect("create cache root");

        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "same-file.jpg");

        let background = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Background,
            )
            .expect("request background thumbnail");
        let background_task_id = background.task_id.expect("background task id");
        database
            .mark_task_running_current_attempt_for_test(background_task_id)
            .expect("mark background task running");

        run_thumbnail_task_with_cache_and_before_publish(
            &mut database,
            None,
            background_task_id,
            &cache_root,
            true,
            |database| {
                let selected = database
                    .request_thumbnail_task(
                        file_id,
                        crate::thumbnails::GRID_320_PROFILE,
                        crate::db::ThumbnailTaskPriority::Selected,
                    )
                    .expect("request selected thumbnail while same file is running");
                assert!(selected.queued);
                assert_ne!(selected.task_id, Some(background_task_id));
            },
        )
        .expect("background thumbnail should publish after generating bytes");

        let background_task = database
            .get_task(background_task_id)
            .expect("get background task after publish")
            .expect("background task exists after publish");
        assert_eq!(background_task.status, "succeeded");
        assert_eq!(
            background_task.priority,
            crate::db::THUMBNAIL_BACKGROUND_PRIORITY
        );

        let selected_task = database
            .list_tasks()
            .expect("list tasks")
            .into_iter()
            .find(|task| {
                task.kind == "thumbnail"
                    && task.file_id == Some(file_id)
                    && task.id != background_task_id
                    && task.priority == crate::db::THUMBNAIL_SELECTED_PRIORITY
            })
            .expect("selected thumbnail task exists");
        assert_eq!(selected_task.status, "pending");
        assert_eq!(
            database
                .list_pending_foreground_thumbnail_task_ids()
                .expect("list pending foreground thumbnails"),
            vec![selected_task.id]
        );

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "ready");
        assert_eq!(thumbnail.served_by.as_deref(), Some("db_blob"));
        assert!(database
            .get_thumb_blob(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("read same-file thumb blob")
            .is_some());

        fs::remove_dir_all(&temp_root).expect("cleanup media root");
        fs::remove_dir_all(&cache_root).expect("cleanup cache root");
    }

    #[test]
    fn running_visible_thumbnail_yields_mid_processing_after_scope_demotion() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let temp_root = unique_temp_dir("thumbnail_mid_processing_scope_demotion_root");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("old-visible.jpg"), 800, 400);
        write_test_image(&temp_root.join("new-visible.jpg"), 800, 400);
        let cache_root = unique_temp_dir("thumbnail_mid_processing_scope_demotion_cache");
        fs::create_dir_all(&cache_root).expect("create cache root");

        let db_dir = unique_temp_dir("thumbnail_mid_processing_scope_demotion_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("thumbnail-mid-processing-scope-demotion.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Scope Demotion".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "thumbnail-mid-processing-scope-demotion-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let old_visible_file_id = insert_pending_thumbnail_file(
            &database,
            root_id,
            folder_id,
            "old-visible.jpg",
            "thumbnail-scope-demotion-old",
        );
        let new_visible_file_id = insert_pending_thumbnail_file(
            &database,
            root_id,
            folder_id,
            "new-visible.jpg",
            "thumbnail-scope-demotion-new",
        );

        let old_visible_task_id = database
            .request_thumbnail_task(
                old_visible_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request old visible thumbnail")
            .task_id
            .expect("old visible task id");
        let old_attempt = database
            .get_task(old_visible_task_id)
            .expect("get old visible task")
            .expect("old visible task exists")
            .attempt_generation;
        let hook_database = database
            .reopen()
            .expect("reopen db")
            .expect("reopened database");
        let injected = Arc::new(AtomicBool::new(false));
        let hook_injected = Arc::clone(&injected);
        let hook_db_path = db_path.clone();
        let _hook_guard = set_thumbnail_processing_checkpoint_hook_for_test(Box::new(
            move |task_id, database_path| {
                if database_path != Some(hook_db_path.as_path())
                    || task_id != old_visible_task_id
                    || hook_injected.swap(true, Ordering::SeqCst)
                {
                    return;
                }
                hook_database
                    .request_thumbnail_task(
                        new_visible_file_id,
                        crate::thumbnails::GRID_320_PROFILE,
                        crate::db::ThumbnailTaskPriority::Visible,
                    )
                    .expect("request new visible thumbnail during checkpoint");
                hook_database
                    .sync_thumbnail_priority_scope(root_id, &[], &[new_visible_file_id], &[])
                    .expect("sync scope to demote old visible task");
            },
        ));

        let error = run_thumbnail_task_with_cache(
            &mut database,
            None,
            old_visible_task_id,
            &cache_root,
            true,
        )
        .expect_err("old visible thumbnail should yield after scope demotion");

        assert!(
            error.to_string().contains("yield"),
            "expected cooperative yield error, got {error}"
        );

        let old_visible_task = database
            .get_task(old_visible_task_id)
            .expect("get old visible task after yield")
            .expect("old visible task exists after yield");
        assert_eq!(old_visible_task.status, "pending");
        assert_eq!(
            old_visible_task.priority,
            crate::db::THUMBNAIL_BACKGROUND_PRIORITY
        );
        assert!(old_visible_task.attempt_generation > old_attempt);

        let new_visible_task = database
            .list_tasks()
            .expect("list tasks")
            .into_iter()
            .find(|task| {
                task.kind == "thumbnail"
                    && task.file_id == Some(new_visible_file_id)
                    && task.priority == crate::db::THUMBNAIL_VISIBLE_PRIORITY
            })
            .expect("new visible task exists");
        assert_eq!(new_visible_task.status, "pending");

        fs::remove_dir_all(&temp_root).expect("cleanup media root");
        fs::remove_dir_all(&cache_root).expect("cleanup cache root");
        let _ = fs::remove_dir_all(&db_dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn thumbnail_worker_runs_multiple_thumbnail_tasks_concurrently() {
        let temp_root = unique_temp_dir("thumbnail_worker_concurrency_root");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("first.jpg"), 800, 400);
        write_test_image(&temp_root.join("second.jpg"), 800, 400);

        let db_dir = unique_temp_dir("thumbnail_worker_concurrency_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("thumbnail-worker-concurrency.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Thumbnail Worker Concurrency".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "thumbnail-worker-concurrency-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");

        let first_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "first.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 10,
                mtime: 1,
                ctime: None,
                file_key: Some("thumbnail-worker-first".to_string()),
            })
            .expect("insert first file");
        let second_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "second.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 11,
                mtime: 2,
                ctime: None,
                file_key: Some("thumbnail-worker-second".to_string()),
            })
            .expect("insert second file");
        database
            .upsert_media_kind(first_file_id, "image")
            .expect("insert first media kind");
        database
            .upsert_media_kind(second_file_id, "image")
            .expect("insert second media kind");

        let first_task_id = database
            .request_thumbnail_task(
                first_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request first thumbnail")
            .task_id
            .expect("first task id");
        let second_task_id = database
            .request_thumbnail_task(
                second_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request second thumbnail")
            .task_id
            .expect("second task id");

        let started_count = Arc::new(AtomicUsize::new(0));
        let release_workers = Arc::new(AtomicBool::new(false));
        let started_notify = Arc::new(Notify::new());
        let watched_task_ids = Arc::new([first_task_id, second_task_id]);
        let hook_started_count = Arc::clone(&started_count);
        let hook_release_workers = Arc::clone(&release_workers);
        let hook_started_notify = Arc::clone(&started_notify);
        let hook_task_ids = Arc::clone(&watched_task_ids);
        let hook_db_path = db_path.clone();
        let _hook_guard =
            set_worker_task_hook_for_test(Arc::new(move |kind, task_id, database_path| {
                if database_path != Some(hook_db_path.as_path())
                    || kind != "thumbnail"
                    || !hook_task_ids.contains(&task_id)
                {
                    return;
                }
                let previous = hook_started_count.fetch_add(1, Ordering::SeqCst);
                if previous < 2 {
                    hook_started_notify.notify_waiters();
                }
                while !hook_release_workers.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }));

        let thumbnail_permits = ThumbnailWorkerPermits {
            selected: Arc::new(Semaphore::new(THUMBNAIL_SELECTED_WORKER_CONCURRENCY)),
            foreground: Arc::new(Semaphore::new(THUMBNAIL_FOREGROUND_WORKER_CONCURRENCY)),
            background: Arc::new(Semaphore::new(THUMBNAIL_BACKGROUND_WORKER_CONCURRENCY)),
        };
        let (completion_sender, mut completion_receiver) = mpsc::unbounded_channel();
        let mut in_flight_thumbnail_task_ids = HashSet::new();
        let mut database = database;
        let writer = crate::db::DbWriter::spawn(database.reopen().unwrap().unwrap());
        dispatch_task_with_thumbnail_concurrency(
            &mut database,
            &writer,
            first_task_id,
            true,
            &thumbnail_permits,
            &completion_sender,
            &mut in_flight_thumbnail_task_ids,
        );
        dispatch_task_with_thumbnail_concurrency(
            &mut database,
            &writer,
            second_task_id,
            true,
            &thumbnail_permits,
            &completion_sender,
            &mut in_flight_thumbnail_task_ids,
        );

        timeout(Duration::from_secs(5), async {
            while started_count.load(Ordering::SeqCst) < 2 {
                started_notify.notified().await;
            }
        })
        .await
        .expect("both thumbnail workers should start concurrently");
        release_workers.store(true, Ordering::SeqCst);
        timeout(Duration::from_secs(5), async {
            let mut completed = HashSet::new();
            while completed.len() < 2 {
                let task_id = completion_receiver
                    .recv()
                    .await
                    .expect("thumbnail worker completion");
                completed.insert(task_id);
            }
        })
        .await
        .expect("both thumbnail workers should complete");

        fs::remove_dir_all(&temp_root).expect("cleanup media root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn foreground_thumbnail_worker_starts_while_background_worker_is_blocked() {
        let temp_root = unique_temp_dir("thumbnail_worker_foreground_priority_root");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("background.jpg"), 800, 400);
        write_test_image(&temp_root.join("foreground.jpg"), 800, 400);

        let db_dir = unique_temp_dir("thumbnail_worker_foreground_priority_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("thumbnail-worker-foreground-priority.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Thumbnail Worker Foreground Priority".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "thumbnail-worker-foreground-priority-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");

        let background_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "background.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 10,
                mtime: 1,
                ctime: None,
                file_key: Some("thumbnail-worker-background".to_string()),
            })
            .expect("insert background file");
        let foreground_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "foreground.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 11,
                mtime: 2,
                ctime: None,
                file_key: Some("thumbnail-worker-foreground".to_string()),
            })
            .expect("insert foreground file");
        database
            .upsert_media_kind(background_file_id, "image")
            .expect("insert background media kind");
        database
            .upsert_media_kind(foreground_file_id, "image")
            .expect("insert foreground media kind");

        let background_task_id = database
            .request_thumbnail_task(
                background_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Background,
            )
            .expect("request background thumbnail")
            .task_id
            .expect("background task id");
        let foreground_task_id = database
            .request_thumbnail_task(
                foreground_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request foreground thumbnail")
            .task_id
            .expect("foreground task id");

        let background_started = Arc::new(AtomicBool::new(false));
        let foreground_started = Arc::new(AtomicBool::new(false));
        let release_background = Arc::new(AtomicBool::new(false));
        let foreground_started_notify = Arc::new(Notify::new());
        let background_task_id_copy = background_task_id;
        let foreground_task_id_copy = foreground_task_id;
        let hook_background_started = Arc::clone(&background_started);
        let hook_foreground_started = Arc::clone(&foreground_started);
        let hook_release_background = Arc::clone(&release_background);
        let hook_foreground_started_notify = Arc::clone(&foreground_started_notify);
        let hook_db_path = db_path.clone();
        let _hook_guard =
            set_worker_task_hook_for_test(Arc::new(move |kind, task_id, database_path| {
                if database_path != Some(hook_db_path.as_path()) || kind != "thumbnail" {
                    return;
                }
                if task_id == background_task_id_copy {
                    hook_background_started.store(true, Ordering::SeqCst);
                    while !hook_release_background.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                } else if task_id == foreground_task_id_copy {
                    hook_foreground_started.store(true, Ordering::SeqCst);
                    hook_foreground_started_notify.notify_waiters();
                }
            }));

        let thumbnail_permits = ThumbnailWorkerPermits {
            selected: Arc::new(Semaphore::new(THUMBNAIL_SELECTED_WORKER_CONCURRENCY)),
            foreground: Arc::new(Semaphore::new(THUMBNAIL_FOREGROUND_WORKER_CONCURRENCY)),
            background: Arc::new(Semaphore::new(THUMBNAIL_BACKGROUND_WORKER_CONCURRENCY)),
        };
        let (completion_sender, mut completion_receiver) = mpsc::unbounded_channel();
        let mut in_flight_thumbnail_task_ids = HashSet::new();
        let mut database = database;
        let writer = crate::db::DbWriter::spawn(database.reopen().unwrap().unwrap());
        dispatch_task_with_thumbnail_concurrency(
            &mut database,
            &writer,
            background_task_id,
            true,
            &thumbnail_permits,
            &completion_sender,
            &mut in_flight_thumbnail_task_ids,
        );
        dispatch_task_with_thumbnail_concurrency(
            &mut database,
            &writer,
            foreground_task_id,
            true,
            &thumbnail_permits,
            &completion_sender,
            &mut in_flight_thumbnail_task_ids,
        );

        timeout(Duration::from_secs(5), async {
            while !foreground_started.load(Ordering::SeqCst) {
                foreground_started_notify.notified().await;
            }
        })
        .await
        .expect("foreground thumbnail worker should start while background worker is blocked");
        assert!(
            background_started.load(Ordering::SeqCst),
            "background worker should have started and remained blocked"
        );

        release_background.store(true, Ordering::SeqCst);
        timeout(Duration::from_secs(5), async {
            let mut completed = HashSet::new();
            while completed.len() < 2 {
                let task_id = completion_receiver
                    .recv()
                    .await
                    .expect("thumbnail worker completion");
                completed.insert(task_id);
            }
        })
        .await
        .expect("background and foreground thumbnail workers should both complete");

        fs::remove_dir_all(&temp_root).expect("cleanup media root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn schedule_pending_tasks_clears_stale_in_flight_thumbnail_ids() {
        let temp_root = unique_temp_dir("thumbnail_stale_in_flight_root");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("pending.jpg"), 800, 400);

        let db_dir = unique_temp_dir("thumbnail_stale_in_flight_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("thumbnail-stale-in-flight.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let file_id = seed_media_file(&database, &temp_root, "pending.jpg");
        let task_id = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Background,
            )
            .expect("request pending thumbnail")
            .task_id
            .expect("task id");

        let thumbnail_permits = ThumbnailWorkerPermits {
            selected: Arc::new(Semaphore::new(THUMBNAIL_SELECTED_WORKER_CONCURRENCY)),
            foreground: Arc::new(Semaphore::new(THUMBNAIL_FOREGROUND_WORKER_CONCURRENCY)),
            background: Arc::new(Semaphore::new(THUMBNAIL_BACKGROUND_WORKER_CONCURRENCY)),
        };
        let (completion_sender, mut completion_receiver) = mpsc::unbounded_channel();
        let mut in_flight_thumbnail_task_ids = HashSet::new();
        in_flight_thumbnail_task_ids.insert(task_id);
        let mut database = database;
        let writer = crate::db::DbWriter::spawn(database.reopen().unwrap().unwrap());

        schedule_pending_tasks_with_thumbnail_concurrency(
            &mut database,
            &writer,
            true,
            &thumbnail_permits,
            &completion_sender,
            &mut in_flight_thumbnail_task_ids,
        );

        timeout(Duration::from_secs(5), async {
            completion_receiver
                .recv()
                .await
                .expect("background thumbnail completion")
        })
        .await
        .expect("stale in-flight id should not prevent pending thumbnail from starting");

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "succeeded");

        fs::remove_dir_all(&temp_root).expect("cleanup media root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn startup_recovery_thumbnail_backlog_does_not_block_new_selected_thumbnail() {
        let temp_root = unique_temp_dir("thumbnail_startup_recovery_priority_root");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("background.jpg"), 800, 400);
        write_test_image(&temp_root.join("selected.jpg"), 800, 400);

        let db_dir = unique_temp_dir("thumbnail_startup_recovery_priority_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("thumbnail-startup-recovery-priority.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Thumbnail Startup Recovery Priority".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "thumbnail-startup-recovery-priority-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");

        let background_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "background.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 10,
                mtime: 1,
                ctime: None,
                file_key: Some("thumbnail-startup-background".to_string()),
            })
            .expect("insert background file");
        let selected_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "selected.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 11,
                mtime: 2,
                ctime: None,
                file_key: Some("thumbnail-startup-selected".to_string()),
            })
            .expect("insert selected file");
        database
            .upsert_media_kind(background_file_id, "image")
            .expect("insert background media kind");
        database
            .upsert_media_kind(selected_file_id, "image")
            .expect("insert selected media kind");

        let background_task_id = database
            .request_thumbnail_task(
                background_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Background,
            )
            .expect("request startup background thumbnail")
            .task_id
            .expect("background task id");

        let background_started = Arc::new(AtomicBool::new(false));
        let selected_started = Arc::new(AtomicBool::new(false));
        let release_background = Arc::new(AtomicBool::new(false));
        let selected_started_notify = Arc::new(Notify::new());
        let hook_background_started = Arc::clone(&background_started);
        let hook_selected_started = Arc::clone(&selected_started);
        let hook_release_background = Arc::clone(&release_background);
        let hook_selected_started_notify = Arc::clone(&selected_started_notify);
        let wait_selected_started_notify = Arc::clone(&selected_started_notify);
        let hook_db_path = db_path.clone();
        let _hook_guard =
            set_worker_task_hook_for_test(Arc::new(move |kind, task_id, database_path| {
                if database_path != Some(hook_db_path.as_path()) || kind != "thumbnail" {
                    return;
                }
                if task_id == background_task_id {
                    hook_background_started.store(true, Ordering::SeqCst);
                    while !hook_release_background.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                } else {
                    hook_selected_started.store(true, Ordering::SeqCst);
                    hook_selected_started_notify.notify_waiters();
                }
            }));

        let writer = crate::db::DbWriter::spawn(database.reopen().unwrap().unwrap());
        let sender = start_worker_with_ffmpeg(database, writer, true);
        let request_database = Database::open(&db_path).expect("open request database");
        request_database
            .apply_migrations()
            .expect("apply request database migrations");
        let selected_task_id = request_database
            .request_thumbnail_task(
                selected_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Selected,
            )
            .expect("request selected thumbnail")
            .task_id
            .expect("selected task id");
        sender
            .send(selected_task_id)
            .await
            .expect("send selected task id");

        timeout(Duration::from_secs(5), async {
            while !selected_started.load(Ordering::SeqCst) {
                wait_selected_started_notify.notified().await;
            }
        })
        .await
        .expect("selected thumbnail should start while startup backlog background task is blocked");

        release_background.store(true, Ordering::SeqCst);

        fs::remove_dir_all(&temp_root).expect("cleanup media root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn selected_thumbnail_worker_starts_while_visible_workers_are_blocked() {
        let temp_root = unique_temp_dir("thumbnail_worker_selected_priority_root");
        fs::create_dir_all(&temp_root).expect("create media root");
        for index in 0..THUMBNAIL_FOREGROUND_WORKER_CONCURRENCY {
            write_test_image(&temp_root.join(format!("visible-{index}.jpg")), 800, 400);
        }
        write_test_image(&temp_root.join("selected.jpg"), 800, 400);

        let db_dir = unique_temp_dir("thumbnail_worker_selected_priority_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("thumbnail-worker-selected-priority.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Thumbnail Worker Selected Priority".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "thumbnail-worker-selected-priority-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");

        let mut visible_task_ids = Vec::new();
        for index in 0..THUMBNAIL_FOREGROUND_WORKER_CONCURRENCY {
            let file_id = database
                .upsert_file(FileUpsert {
                    root_id,
                    folder_id,
                    name: format!("visible-{index}.jpg"),
                    ext: ".jpg".to_string(),
                    size: 10 + index as i64,
                    mtime: 1 + index as i64,
                    ctime: None,
                    file_key: Some(format!("thumbnail-worker-visible-{index}")),
                })
                .expect("insert visible file");
            database
                .upsert_media_kind(file_id, "image")
                .expect("insert visible media kind");
            let task_id = database
                .request_thumbnail_task(
                    file_id,
                    crate::thumbnails::GRID_320_PROFILE,
                    crate::db::ThumbnailTaskPriority::Visible,
                )
                .expect("request visible thumbnail")
                .task_id
                .expect("visible task id");
            visible_task_ids.push(task_id);
        }

        let selected_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "selected.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 100,
                ctime: None,
                file_key: Some("thumbnail-worker-selected".to_string()),
            })
            .expect("insert selected file");
        database
            .upsert_media_kind(selected_file_id, "image")
            .expect("insert selected media kind");
        let selected_task_id = database
            .request_thumbnail_task(
                selected_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Selected,
            )
            .expect("request selected thumbnail")
            .task_id
            .expect("selected task id");

        let release_visible = Arc::new(AtomicBool::new(false));
        let selected_started = Arc::new(AtomicBool::new(false));
        let selected_started_notify = Arc::new(Notify::new());
        let watched_visible_task_ids = Arc::new(visible_task_ids.clone());
        let hook_release_visible = Arc::clone(&release_visible);
        let hook_selected_started = Arc::clone(&selected_started);
        let hook_selected_started_notify = Arc::clone(&selected_started_notify);
        let hook_visible_task_ids = Arc::clone(&watched_visible_task_ids);
        let hook_db_path = db_path.clone();
        let _hook_guard =
            set_worker_task_hook_for_test(Arc::new(move |kind, task_id, database_path| {
                if database_path != Some(hook_db_path.as_path()) || kind != "thumbnail" {
                    return;
                }
                if hook_visible_task_ids.contains(&task_id) {
                    while !hook_release_visible.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                } else if task_id == selected_task_id {
                    hook_selected_started.store(true, Ordering::SeqCst);
                    hook_selected_started_notify.notify_waiters();
                }
            }));

        let thumbnail_permits = ThumbnailWorkerPermits {
            selected: Arc::new(Semaphore::new(THUMBNAIL_SELECTED_WORKER_CONCURRENCY)),
            foreground: Arc::new(Semaphore::new(THUMBNAIL_FOREGROUND_WORKER_CONCURRENCY)),
            background: Arc::new(Semaphore::new(THUMBNAIL_BACKGROUND_WORKER_CONCURRENCY)),
        };
        let (completion_sender, mut completion_receiver) = mpsc::unbounded_channel();
        let mut in_flight_thumbnail_task_ids = HashSet::new();
        let mut database = database;
        let writer = crate::db::DbWriter::spawn(database.reopen().unwrap().unwrap());
        for task_id in &visible_task_ids {
            dispatch_task_with_thumbnail_concurrency(
                &mut database,
                &writer,
                *task_id,
                true,
                &thumbnail_permits,
                &completion_sender,
                &mut in_flight_thumbnail_task_ids,
            );
        }
        dispatch_task_with_thumbnail_concurrency(
            &mut database,
            &writer,
            selected_task_id,
            true,
            &thumbnail_permits,
            &completion_sender,
            &mut in_flight_thumbnail_task_ids,
        );

        timeout(Duration::from_secs(5), async {
            while !selected_started.load(Ordering::SeqCst) {
                selected_started_notify.notified().await;
            }
        })
        .await
        .expect("selected thumbnail worker should start while visible workers are blocked");

        release_visible.store(true, Ordering::SeqCst);
        timeout(Duration::from_secs(5), async {
            let mut completed = HashSet::new();
            while completed.len() < visible_task_ids.len() + 1 {
                let task_id = completion_receiver
                    .recv()
                    .await
                    .expect("thumbnail worker completion");
                completed.insert(task_id);
            }
        })
        .await
        .expect("visible and selected thumbnail workers should all complete");

        fs::remove_dir_all(&temp_root).expect("cleanup media root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn scan_tasks_remain_serial_while_thumbnail_workers_are_concurrent() {
        let first_root_dir = unique_temp_dir("serial_scan_first_root");
        let second_root_dir = unique_temp_dir("serial_scan_second_root");
        fs::create_dir_all(&first_root_dir).expect("create first root");
        fs::create_dir_all(&second_root_dir).expect("create second root");
        fs::write(first_root_dir.join("first.jpg"), b"first").expect("write first seed");
        fs::write(second_root_dir.join("second.jpg"), b"second").expect("write second seed");

        let db_dir = unique_temp_dir("serial_scan_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("serial-scan.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");

        let first_root_id = database
            .add_root(NewRoot {
                path: first_root_dir.to_string_lossy().to_string(),
                display_name: "First Scan Root".to_string(),
            })
            .expect("add first root");
        database
            .ensure_folder_chain_for_path(first_root_id, &first_root_dir, &first_root_dir)
            .expect("seed first root folder");
        let second_root_id = database
            .add_root(NewRoot {
                path: second_root_dir.to_string_lossy().to_string(),
                display_name: "Second Scan Root".to_string(),
            })
            .expect("add second root");
        database
            .ensure_folder_chain_for_path(second_root_id, &second_root_dir, &second_root_dir)
            .expect("seed second root folder");

        let first_task_id = database
            .create_root_scan_task(first_root_id)
            .expect("create first root scan task");
        let second_task_id = database
            .create_root_scan_task(second_root_id)
            .expect("create second root scan task");

        let started_count = Arc::new(AtomicUsize::new(0));
        let release_first_scan = Arc::new(AtomicBool::new(false));
        let first_scan_started_notify = Arc::new(Notify::new());
        let any_scan_started_notify = Arc::new(Notify::new());
        let watched_task_ids = Arc::new([first_task_id, second_task_id]);
        let hook_started_count = Arc::clone(&started_count);
        let hook_release_first_scan = Arc::clone(&release_first_scan);
        let hook_first_scan_started_notify = Arc::clone(&first_scan_started_notify);
        let hook_any_scan_started_notify = Arc::clone(&any_scan_started_notify);
        let hook_task_ids = Arc::clone(&watched_task_ids);
        let hook_db_path = db_path.clone();
        let _hook_guard =
            set_worker_task_hook_for_test(Arc::new(move |kind, task_id, database_path| {
                if database_path != Some(hook_db_path.as_path())
                    || kind != "root_scan"
                    || !hook_task_ids.contains(&task_id)
                {
                    return;
                }
                let previous = hook_started_count.fetch_add(1, Ordering::SeqCst);
                if previous == 0 {
                    hook_first_scan_started_notify.notify_waiters();
                }
                hook_any_scan_started_notify.notify_waiters();
                if task_id == first_task_id {
                    while !hook_release_first_scan.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            }));

        let writer = crate::db::DbWriter::spawn(database.reopen().unwrap().unwrap());
        let sender = start_worker_with_ffmpeg(database, writer, true);
        sender
            .send(first_task_id)
            .await
            .expect("send first root scan");
        timeout(Duration::from_secs(5), first_scan_started_notify.notified())
            .await
            .expect("first scan should start");
        sender
            .send(second_task_id)
            .await
            .expect("send second root scan");
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(
            started_count.load(Ordering::SeqCst),
            1,
            "second scan should not start while first scan is blocked"
        );

        release_first_scan.store(true, Ordering::SeqCst);
        timeout(Duration::from_secs(5), async {
            while started_count.load(Ordering::SeqCst) < 2 {
                any_scan_started_notify.notified().await;
            }
        })
        .await
        .expect("second scan should start after first releases");

        let observer = Database::open(&db_path).expect("reopen observer db");
        wait_for_task_statuses(
            &observer,
            &[(first_task_id, "succeeded"), (second_task_id, "succeeded")],
            Duration::from_secs(5),
        );

        fs::remove_dir_all(&first_root_dir).expect("cleanup first root");
        fs::remove_dir_all(&second_root_dir).expect("cleanup second root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn background_thumbnail_wakeup_starts_while_root_scan_is_running() {
        let temp_root = unique_temp_dir("background_wakeup_during_root_scan_root");
        fs::create_dir_all(&temp_root).expect("create media root");
        write_test_image(&temp_root.join("background.jpg"), 800, 400);

        let db_dir = unique_temp_dir("background_wakeup_during_root_scan_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("background-wakeup-during-root-scan.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Background Wake During Root Scan".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "background-wakeup-during-root-scan-folder".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let file_id = insert_pending_thumbnail_file(
            &database,
            root_id,
            folder_id,
            "background.jpg",
            "background-wakeup-during-root-scan-file",
        );
        let root_scan_task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");

        let root_scan_started = Arc::new(AtomicBool::new(false));
        let thumbnail_started = Arc::new(AtomicBool::new(false));
        let release_root_scan = Arc::new(AtomicBool::new(false));
        let root_scan_started_notify = Arc::new(Notify::new());
        let thumbnail_started_notify = Arc::new(Notify::new());
        let hook_root_scan_started = Arc::clone(&root_scan_started);
        let hook_thumbnail_started = Arc::clone(&thumbnail_started);
        let hook_release_root_scan = Arc::clone(&release_root_scan);
        let hook_root_scan_started_notify = Arc::clone(&root_scan_started_notify);
        let hook_thumbnail_started_notify = Arc::clone(&thumbnail_started_notify);
        let wait_thumbnail_started_notify = Arc::clone(&thumbnail_started_notify);
        let hook_db_path = db_path.clone();
        let _hook_guard =
            set_worker_task_hook_for_test(Arc::new(move |kind, task_id, database_path| {
                if database_path != Some(hook_db_path.as_path()) {
                    return;
                }
                if kind == "root_scan" && task_id == root_scan_task_id {
                    hook_root_scan_started.store(true, Ordering::SeqCst);
                    hook_root_scan_started_notify.notify_waiters();
                    while !hook_release_root_scan.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                } else if kind == "thumbnail" {
                    hook_thumbnail_started.store(true, Ordering::SeqCst);
                    hook_thumbnail_started_notify.notify_waiters();
                }
            }));

        let writer = crate::db::DbWriter::spawn(database.reopen().unwrap().unwrap());
        let sender = start_worker_with_ffmpeg(database, writer, true);
        sender
            .send(root_scan_task_id)
            .await
            .expect("send root scan task");
        timeout(Duration::from_secs(5), async {
            while !root_scan_started.load(Ordering::SeqCst) {
                root_scan_started_notify.notified().await;
            }
        })
        .await
        .expect("root scan should start");

        let request_database = Database::open(&db_path).expect("open request database");
        request_database
            .apply_migrations()
            .expect("apply request database migrations");
        let thumbnail_task_id = request_database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Background,
            )
            .expect("request background thumbnail")
            .task_id
            .expect("thumbnail task id");
        sender
            .send(thumbnail_task_id)
            .await
            .expect("send background thumbnail wakeup");

        let thumbnail_started_result = timeout(Duration::from_secs(1), async {
            while !thumbnail_started.load(Ordering::SeqCst) {
                wait_thumbnail_started_notify.notified().await;
            }
        })
        .await;
        release_root_scan.store(true, Ordering::SeqCst);
        thumbnail_started_result
            .expect("background thumbnail should start while root scan is still running");

        drop(sender);
        fs::remove_dir_all(&temp_root).expect("cleanup temp root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn root_scan_yields_to_foreground_thumbnail_at_most_once_before_resuming_progress() {
        let temp_root = unique_temp_dir("foreground_thumbnail_fairness_root");
        fs::create_dir_all(&temp_root).expect("create root");
        fs::write(temp_root.join("outside-a.jpg"), b"outside-a").expect("write first image");
        fs::write(temp_root.join("outside-b.jpg"), b"outside-b").expect("write second image");

        let db_dir = unique_temp_dir("foreground_thumbnail_fairness_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("foreground-thumbnail-fairness.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Foreground Thumbnail Fairness".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .ensure_folder_chain_for_path(root_id, &temp_root, &temp_root)
            .expect("seed root folder");
        let root_task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        let old_attempt = database
            .get_task(root_task_id)
            .expect("get root task")
            .expect("root task exists")
            .attempt_generation;

        for name in ["foreground-a.jpg", "foreground-b.jpg"] {
            let file_id = database
                .upsert_file(crate::db::FileUpsert {
                    root_id,
                    folder_id: root_folder_id,
                    name: name.to_string(),
                    ext: ".jpg".to_string(),
                    size: 123,
                    mtime: 321,
                    ctime: None,
                    file_key: Some(format!("{name}-key")),
                })
                .expect("insert file");
            database
                .upsert_media_kind(file_id, "image")
                .expect("insert media kind");
        }

        let first_foreground_file_id = database
            .list_media_page(crate::db::MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(root_folder_id),
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list seeded media")
            .items
            .iter()
            .find(|item| item.name == "foreground-a.jpg")
            .expect("foreground-a media")
            .id;
        let second_foreground_file_id = database
            .list_media_page(crate::db::MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(root_folder_id),
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list seeded media again")
            .items
            .iter()
            .find(|item| item.name == "foreground-b.jpg")
            .expect("foreground-b media")
            .id;

        database
            .request_thumbnail_task(
                first_foreground_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Selected,
            )
            .expect("request first foreground thumbnail");

        run_task(&mut database, None, root_task_id, true);

        let yielded_task = database
            .get_task(root_task_id)
            .expect("get yielded root task")
            .expect("yielded root task exists");
        assert_eq!(yielded_task.status, "pending");
        assert_eq!(yielded_task.attempt_generation, old_attempt + 1);
        assert_eq!(
            yielded_task.priority,
            ROOT_SCAN_FOREGROUND_FAIRNESS_CONSUMED_PRIORITY
        );

        database
            .request_thumbnail_task(
                second_foreground_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Selected,
            )
            .expect("request second foreground thumbnail");

        run_task(&mut database, None, root_task_id, true);

        let resumed_task = database
            .get_task(root_task_id)
            .expect("get resumed root task")
            .expect("resumed root task exists");
        assert_eq!(resumed_task.status, "succeeded");
        assert_eq!(resumed_task.attempt_generation, old_attempt + 1);
        assert_eq!(
            resumed_task.priority,
            ROOT_SCAN_FOREGROUND_FAIRNESS_CONSUMED_PRIORITY
        );

        fs::remove_dir_all(&temp_root).expect("cleanup root");
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn startup_recovery_re_drains_pending_interactive_tasks_between_replayed_root_tasks() {
        let temp_root = unique_temp_dir("startup_recovery_redrain");
        let selected_dir = temp_root.join("selected");
        fs::create_dir_all(&selected_dir).expect("create selected dir");
        fs::write(temp_root.join("outside.jpg"), b"outside").expect("write outside image");
        fs::write(selected_dir.join("inside.jpg"), b"inside").expect("write inside image");

        let db_dir = unique_temp_dir("startup_recovery_redrain_db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("startup-recovery-redrain.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: "Startup Recovery".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .ensure_folder_chain_for_path(root_id, &temp_root, &selected_dir)
            .expect("seed folder chain");
        let first_root_task_id = database
            .create_root_scan_task(root_id)
            .expect("create first root recovery task");
        let second_root_task_id = database
            .create_root_scan_task(root_id)
            .expect("create second root recovery task");
        let mut inserted_interactive_task_id = None;

        replay_startup_recovery_task_ids_with_after_each(
            &mut database,
            &[first_root_task_id, second_root_task_id],
            true,
            |database, task_id| {
                if task_id == first_root_task_id {
                    let interactive_task_id = database
                        .create_interactive_folder_scan_task(folder_id)
                        .expect("insert interactive task during recovery");
                    inserted_interactive_task_id = Some(interactive_task_id);
                }
            },
        );

        let interactive_task_id = inserted_interactive_task_id.expect("interactive task id");
        let interactive_task = database
            .get_task(interactive_task_id)
            .expect("get interactive task")
            .expect("interactive task exists");
        assert_eq!(interactive_task.status, "succeeded");

        let second_root_task = database
            .get_task(second_root_task_id)
            .expect("get second root task")
            .expect("second root task exists");
        assert_eq!(second_root_task.status, "succeeded");

        assert!(database
            .list_pending_interactive_folder_scan_task_ids()
            .expect("list pending interactive tasks")
            .is_empty());
        assert!(database
            .list_pending_root_scan_task_ids()
            .expect("list pending root tasks")
            .is_empty());

        fs::remove_dir_all(&temp_root).expect("cleanup temp root");
        let _ = fs::remove_dir_all(db_dir);
    }

    struct TestThumbCtx {
        task_id: i64,
        file_id: i64,
    }

    /// Seed a root + folder + one real on-disk image + a pending grid_320
    /// thumbnail task, returning the ids the writer-publish test needs.
    fn seed_one_pending_thumbnail_task(database: &Database, media_root: &Path) -> TestThumbCtx {
        let file_id = seed_media_file(database, media_root, "writer-publish.jpg");
        let task_id = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail task")
            .task_id
            .expect("thumbnail task id");
        TestThumbCtx { task_id, file_id }
    }

    /// Drive the real thumbnail leaf with the writer path (`Some(writer)`), so
    /// the blob commit is routed through the single writer actor exactly as
    /// production does. Runs on a blocking thread (as the real worker does),
    /// because `write_blocking` cannot block a runtime worker thread. Returns
    /// the owned worker connection so the caller can read it back.
    async fn publish_thumbnail_for_test(
        mut worker_db: Database,
        writer: DbWriter,
        task_id: i64,
    ) -> (Database, anyhow::Result<()>) {
        tokio::task::spawn_blocking(move || {
            let cache_root = unique_temp_dir("writer-publish-cache");
            let result = run_thumbnail_task_with_cache(
                &mut worker_db,
                Some(&writer),
                task_id,
                &cache_root,
                true,
            );
            (worker_db, result)
        })
        .await
        .expect("join publish blocking task")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn thumbnail_publish_routes_through_writer_and_bounds_wal() {
        use crate::db::{Database, DbWriter};
        let media_root = unique_temp_dir("thumb-writer-media");
        fs::create_dir_all(&media_root).expect("create media root");
        write_test_image(&media_root.join("writer-publish.jpg"), 800, 400);

        let dir = unique_temp_dir("thumb-writer");
        fs::create_dir_all(&dir).expect("create db dir");
        let db_path = dir.join("thumb.sqlite");
        let seed = Database::open(&db_path).expect("open");
        seed.apply_migrations().expect("migrate");
        // Seed a root + folder + one image file + a pending thumbnail task.
        let ctx = seed_one_pending_thumbnail_task(&seed, &media_root);

        let worker_db = seed.reopen().expect("reopen").expect("worker conn");
        let writer = DbWriter::spawn(seed);

        // Publish through the writer (the code path under test).
        let (worker_db, result) = publish_thumbnail_for_test(worker_db, writer, ctx.task_id).await;
        result.expect("publish via writer");

        // Blob landed.
        let blob = worker_db
            .get_thumb_blob(ctx.file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("read blob");
        assert!(blob.is_some(), "blob must be retrievable after publish");

        // The task completed in the same publish transaction on the writer.
        let task = worker_db
            .get_task(ctx.task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "succeeded");

        // The worker connection's WAL is bounded (writer checkpointed it).
        assert!(
            worker_db.wal_size_bytes() < 64 * 1024 * 1024,
            "WAL must stay under the 64MiB journal_size_limit, got {}",
            worker_db.wal_size_bytes()
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&media_root);
    }

    fn seed_media_file(database: &Database, root_path: &Path, name: &str) -> i64 {
        seed_media_file_with_kind(database, root_path, name, "image")
    }

    fn insert_pending_thumbnail_file(
        database: &Database,
        root_id: i64,
        folder_id: i64,
        name: &str,
        file_key: &str,
    ) -> i64 {
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: name.to_string(),
                ext: format!(
                    ".{}",
                    Path::new(name)
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("")
                ),
                size: 16,
                mtime: 2,
                ctime: None,
                file_key: Some(file_key.to_string()),
            })
            .expect("insert pending thumbnail file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert pending thumbnail media kind");
        file_id
    }

    fn seed_media_file_with_kind(
        database: &Database,
        root_path: &Path,
        name: &str,
        kind: &str,
    ) -> i64 {
        let root_id = database
            .add_root(NewRoot {
                path: root_path.to_string_lossy().to_string(),
                display_name: "Thumbnail Worker".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root-hash".to_string(),
                mtime: Some(1),
            })
            .expect("insert folder");
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: name.to_string(),
                ext: format!(
                    ".{}",
                    Path::new(name)
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("")
                ),
                size: 16,
                mtime: 2,
                ctime: None,
                file_key: Some("worker-identity".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, kind)
            .expect("insert media kind");
        file_id
    }

    fn write_test_image(path: &Path, width: u32, height: u32) {
        let buffer = image::ImageBuffer::from_fn(width, height, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, 0])
        });
        image::DynamicImage::ImageRgb8(buffer)
            .save(path)
            .expect("write test image");
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

        std::env::temp_dir().join(format!(
            "megle_thumbnail_task_test_{}_{}_{}_{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    fn wait_for_task_statuses(
        database: &Database,
        expected_statuses: &[(i64, &str)],
        timeout: Duration,
    ) {
        let deadline = Instant::now() + timeout;
        loop {
            let mut all_matched = true;
            for (task_id, expected_status) in expected_statuses {
                let status = database
                    .get_task(*task_id)
                    .expect("get task during polling")
                    .expect("task exists during polling")
                    .status;
                if status != *expected_status {
                    all_matched = false;
                    break;
                }
            }
            if all_matched {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for task statuses {:?}",
                expected_statuses
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn dispatch_worker_task_never_falls_through_to_inline_run_when_reopen_fails_for_thumbnail_task()
    {
        // Regression: `try_spawn_thumbnail_task` used to return
        // `NotThumbnail` when `database.reopen()` failed even for a
        // CONFIRMED pending thumbnail task, which made
        // `dispatch_worker_task_with_concurrency` fall through to
        // `run_task(database, None, ...)` -- an inline run on the shared
        // worker connection that bypasses the single writer actor (the
        // exact pre-fix WAL-growth bug). `Database::open_in_memory()` has no
        // `path`, so `reopen()` deterministically returns `Ok(None)`,
        // reproducing the failure without needing to fake a filesystem
        // error.
        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Reopen Failure".to_string(),
                display_name: "Reopen Failure".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "reopen-failure-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let file_id = insert_pending_thumbnail_file(
            &database,
            root_id,
            root_folder_id,
            "reopen-failure.jpg",
            "reopen-failure-file",
        );
        let task_id = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                crate::db::ThumbnailTaskPriority::Background,
            )
            .expect("create pending thumbnail task")
            .task_id
            .expect("task id");

        // Separate writer connection over a real file path so `DbWriter::spawn`
        // can open it. The writer is never exercised on the Deferred path
        // this test targets, but dispatch requires a live handle.
        let writer_db_dir = unique_temp_dir("reopen_failure_writer_db");
        fs::create_dir_all(&writer_db_dir).expect("create writer db dir");
        let writer_database =
            Database::open(writer_db_dir.join("writer.sqlite")).expect("open writer database");
        writer_database
            .apply_migrations()
            .expect("apply writer migrations");
        let writer = crate::db::DbWriter::spawn(writer_database);

        let thumbnail_permits = ThumbnailWorkerPermits {
            selected: Arc::new(Semaphore::new(THUMBNAIL_SELECTED_WORKER_CONCURRENCY)),
            foreground: Arc::new(Semaphore::new(THUMBNAIL_FOREGROUND_WORKER_CONCURRENCY)),
            background: Arc::new(Semaphore::new(THUMBNAIL_BACKGROUND_WORKER_CONCURRENCY)),
        };
        let (completion_sender, _completion_receiver) = mpsc::unbounded_channel();
        let mut in_flight_thumbnail_task_ids = HashSet::new();

        let result = try_spawn_thumbnail_task(
            &database,
            &writer,
            task_id,
            true,
            &thumbnail_permits,
            &completion_sender,
            &mut in_flight_thumbnail_task_ids,
        );

        assert!(
            matches!(result, ThumbnailSpawnResult::Deferred),
            "a confirmed pending thumbnail task with a failing reopen() must defer, not fall through to an inline run"
        );
        assert!(
            !in_flight_thumbnail_task_ids.contains(&task_id),
            "a deferred task must not be tracked as in-flight"
        );

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(
            task.status, "pending",
            "task must remain pending, not be run inline on the shared worker connection"
        );

        let _ = fs::remove_dir_all(&writer_db_dir);
    }

    #[test]
    fn dispatch_worker_task_never_falls_through_to_inline_run_when_reopen_fails_for_root_scan_task()
    {
        // Same regression as above but for the non-thumbnail dispatcher
        // path (root_scan / interactive_folder_scan tasks).
        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Reopen Failure Scan".to_string(),
                display_name: "Reopen Failure Scan".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("persist pending root scan task");

        let writer_db_dir = unique_temp_dir("reopen_failure_scan_writer_db");
        fs::create_dir_all(&writer_db_dir).expect("create writer db dir");
        let writer_database =
            Database::open(writer_db_dir.join("writer.sqlite")).expect("open writer database");
        writer_database
            .apply_migrations()
            .expect("apply writer migrations");
        let writer = crate::db::DbWriter::spawn(writer_database);

        let non_thumbnail_permit = Arc::new(Semaphore::new(1));
        let (completion_sender, _completion_receiver) = mpsc::unbounded_channel();
        let mut in_flight_non_thumbnail_task_ids = HashSet::new();

        let result = try_spawn_non_thumbnail_task(
            &database,
            &writer,
            task_id,
            true,
            &non_thumbnail_permit,
            &completion_sender,
            &mut in_flight_non_thumbnail_task_ids,
        );

        assert!(
            matches!(result, ThumbnailSpawnResult::Deferred),
            "a confirmed pending root_scan task with a failing reopen() must defer, not fall through to an inline run"
        );
        assert!(
            !in_flight_non_thumbnail_task_ids.contains(&task_id),
            "a deferred task must not be tracked as in-flight"
        );

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(
            task.status, "pending",
            "task must remain pending, not be run inline on the shared worker connection"
        );

        let _ = fs::remove_dir_all(&writer_db_dir);
    }

    #[test]
    fn root_purge_task_drains_root_and_completes() {
        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: "D:/purge".to_string(),
                display_name: "purge".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root-purge-task-folder".to_string(),
                mtime: Some(1),
            })
            .expect("folder");
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "a.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 1,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .expect("file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("media kind");

        let task_id = match database.mark_root_for_purge(root_id).expect("mark") {
            crate::db::MarkRootForPurge::Marked { task_id, .. } => task_id,
            other => panic!("expected Marked, got {other:?}"),
        };

        // No writer (in-memory): purge runs inline. Drive the task through the worker.
        run_task(&mut database, None, task_id, true);

        // Root + all its rows are gone; the task self-deleted.
        assert!(
            database.get_root(root_id).expect("get root").is_none(),
            "root purged"
        );
        assert!(
            database.get_media(file_id).expect("get media").is_none(),
            "files purged"
        );
        assert!(
            database.get_task(task_id).expect("get task").is_none(),
            "root_purge task self-deleted"
        );
        let events = database
            .list_notification_outbox(0, 200)
            .expect("read root purge notifications");
        let running = events
            .iter()
            .find(|event| {
                event.operation.task_id == Some(task_id) && event.operation.status == "running"
            })
            .expect("root purge running event");
        assert!(running.operation.total_value.is_some_and(|total| total > 0));
        let terminal = events
            .iter()
            .find(|event| {
                event.operation.task_id == Some(task_id) && event.operation.status == "succeeded"
            })
            .expect("root purge terminal event survives self-delete");
        assert_eq!(
            terminal.operation.current_value,
            terminal.operation.total_value.expect("terminal total")
        );
    }

    #[test]
    fn root_purge_worker_rolls_back_failed_batch_and_retries_same_operation() {
        let db_dir = unique_temp_dir("purge-progress-trigger");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: "D:/purge-progress-trigger".to_string(),
                display_name: "purge-progress-trigger".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "purge-progress-trigger-folder".to_string(),
                mtime: Some(1),
            })
            .expect("insert folder");
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "purge.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 1,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");
        let task_id = match database.mark_root_for_purge(root_id).expect("mark") {
            crate::db::MarkRootForPurge::Marked { task_id, .. } => task_id,
            other => panic!("expected Marked, got {other:?}"),
        };
        database
            .execute_batch_for_test(
                "CREATE TRIGGER fail_root_purge_progress
                 BEFORE UPDATE OF items_seen ON tasks
                 WHEN NEW.kind = 'root_purge'
                 BEGIN SELECT RAISE(ABORT, 'injected progress failure'); END;",
            )
            .expect("install progress failure trigger");
        let writer = DbWriter::spawn(Database::open(&db_path).expect("open writer database"));

        run_task(&mut database, Some(&writer), task_id, true);

        let task = database
            .get_task(task_id)
            .expect("read failed task")
            .expect("failed task remains for retry");
        assert_eq!(task.status, "failed");
        assert_eq!(task.items_seen, 0);
        let total = task.items_total.expect("determinate purge total");
        assert!(database
            .file_row_exists_for_test(file_id)
            .expect("failed purge batch must leave file row intact"));
        let events = database
            .list_notification_outbox(0, 200)
            .expect("read failed operation");
        let terminal = events
            .iter()
            .find(|event| {
                event.operation.task_id == Some(task_id) && event.operation.status == "failed"
            })
            .expect("failed terminal operation");
        assert_eq!(terminal.operation.current_value, 0);
        assert!(events.iter().all(|event| {
            event.operation.task_id != Some(task_id) || event.operation.status != "succeeded"
        }));
        assert!(database
            .roots_pending_delete_ids()
            .expect("read pending-delete roots")
            .contains(&root_id));

        database
            .execute_batch_for_test("DROP TRIGGER fail_root_purge_progress;")
            .expect("remove progress failure trigger");
        let retried = database.retry_task(task_id).expect("retry failed purge");
        assert_eq!(retried.id, task_id);
        assert_eq!(retried.items_seen, 0);
        assert_eq!(retried.items_total, Some(total));

        run_task(&mut database, Some(&writer), task_id, true);
        drop(writer);

        assert!(database
            .get_task(task_id)
            .expect("read completed task")
            .is_none());
        assert!(!database
            .roots_pending_delete_ids()
            .expect("read pending-delete roots after retry")
            .contains(&root_id));
        assert!(!database
            .file_row_exists_for_test(file_id)
            .expect("completed purge deletes file row"));

        let events = database
            .list_notification_outbox(0, 200)
            .expect("read retried operation");
        let operation_id = format!("task:{task_id}");
        let purge_events = events
            .iter()
            .filter(|event| event.operation.operation_id == operation_id)
            .collect::<Vec<_>>();
        let task_operation_ids = events
            .iter()
            .filter(|event| event.operation.task_id == Some(task_id))
            .map(|event| event.operation.operation_id.clone())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            task_operation_ids,
            std::collections::HashSet::from([operation_id.clone()])
        );
        assert!(purge_events
            .windows(2)
            .all(|pair| { pair[0].operation.current_value <= pair[1].operation.current_value }));
        assert_eq!(
            purge_events
                .iter()
                .map(|event| event.operation.operation_id.as_str())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            1
        );
        let terminal = purge_events
            .iter()
            .find(|event| event.operation.status == "succeeded")
            .expect("retried purge terminal");
        assert_eq!(terminal.operation.current_value, total);
        assert_eq!(terminal.operation.total_value, Some(total));
        let heads = database
            .notification_heads_for_test()
            .expect("read operation heads");
        assert_eq!(
            heads
                .iter()
                .filter(|operation| operation.operation_id == operation_id)
                .count(),
            1
        );
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn task_error_terminal_notification_failure_is_recoverable_and_logged() {
        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: "D:/terminal-notification-failure".to_string(),
                display_name: "terminal-notification-failure".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task_with_origin(root_id, crate::db::NotificationOrigin::User)
            .expect("create root scan task");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark task running");
        database
            .execute_batch_for_test(
                "CREATE TRIGGER fail_task_terminal_notification
                 BEFORE INSERT ON notification_outbox
                 WHEN json_extract(NEW.payload_json, '$.status') = 'failed'
                 BEGIN SELECT RAISE(ABORT, 'injected terminal notification failure'); END;",
            )
            .expect("install terminal notification failure trigger");

        handle_task_error(&mut database, task_id, anyhow::anyhow!("worker failure"));

        let task = database
            .get_task(task_id)
            .expect("read task after failed terminal publish")
            .expect("task remains for recovery");
        assert_eq!(task.status, "running");
        assert!(database
            .notification_heads_for_test()
            .expect("read operation head")
            .into_iter()
            .find(|operation| operation.task_id == Some(task_id))
            .is_some_and(|operation| operation.status == "running"));
        assert!(database
            .list_notification_outbox(0, 100)
            .expect("read failed terminal events")
            .into_iter()
            .all(|event| event.operation.task_id != Some(task_id)
                || event.operation.status != "failed"));

        database
            .execute_batch_for_test("DROP TRIGGER fail_task_terminal_notification;")
            .expect("remove terminal notification failure trigger");
        assert_eq!(
            database
                .reset_running_root_scan_tasks_for_recovery()
                .expect("recover failed task"),
            1
        );
        assert_eq!(
            database
                .get_task(task_id)
                .expect("read recovered task")
                .expect("recovered task")
                .status,
            "pending"
        );
    }

    #[test]
    fn worker_drain_picks_up_persisted_pending_root_purge_task_after_crash_before_send() {
        // Regression for Critical 1: `mark_root_for_purge` persists a pending
        // `root_purge` task, but if the process crashes (or the enqueue send
        // is otherwise never delivered) before that task id reaches the
        // in-memory worker channel, the ONLY other path that can ever run it
        // is the startup/idle drain (`pending_drain_task_ids`). Before this
        // fix that function did not list `root_purge` tasks at all, so a
        // pending-delete root would be stuck forever: hidden from
        // list_roots/get_root, its files never deleted, and any future
        // add_root of the same path permanently 409ing.
        let mut database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: "D:/purge-drain".to_string(),
                display_name: "purge-drain".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root-purge-drain-folder".to_string(),
                mtime: Some(1),
            })
            .expect("folder");
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "a.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 1,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .expect("file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("media kind");

        // Mark the root for purge exactly like the remove_root route does.
        // This persists a pending `root_purge` task but — unlike the live
        // route path — we deliberately never deliver the task id to a worker
        // channel, modeling a crash (or a dropped/failed send) between the DB
        // commit and the enqueue.
        let task_id = match database
            .mark_root_for_purge(root_id)
            .expect("mark for purge")
        {
            crate::db::MarkRootForPurge::Marked { task_id, .. } => task_id,
            other => panic!("expected Marked, got {other:?}"),
        };

        // Sanity: the root is hidden from get_root (pending-delete), its row
        // is still present pre-drain (only the background purge deletes it),
        // and the purge task is sitting pending in the DB, never having
        // touched a channel.
        assert!(
            database.get_root(root_id).expect("get root").is_none(),
            "pending-delete root must already be hidden from get_root"
        );
        assert_eq!(
            database
                .roots_pending_delete_ids()
                .expect("list pending-delete roots"),
            vec![root_id],
            "root row still present pre-drain, marked pending-delete"
        );
        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.kind, "root_purge");
        assert_eq!(task.status, "pending");

        // Drive the SAME scheduler/idle-drain path startup recovery relies on
        // (not `run_task` directly): it must discover the persisted pending
        // root_purge task and run it to completion.
        drain_pending_persisted_tasks(&mut database, true);

        assert!(
            database.get_root(root_id).expect("get root").is_none(),
            "root_purge task must be discovered and run by the drain, deleting the root"
        );
        assert!(
            database.get_media(file_id).expect("get media").is_none(),
            "root_purge task must delete the root's files"
        );
        assert!(
            database.get_task(task_id).expect("get task").is_none(),
            "root_purge task self-deletes on completion"
        );
    }
}
