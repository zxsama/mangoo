use std::any::Any;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::db::Database;

/// A boxed write job: run against the single write connection, reply with the
/// boxed result over the oneshot. The closure returns `Box<dyn Any + Send>` so
/// jobs of different result types share one channel; `write` downcasts.
type BoxedResult = anyhow::Result<Box<dyn Any + Send>>;
type WriteJob = Box<dyn FnOnce(&mut Database) -> BoxedResult + Send>;

struct QueuedJob {
    job: WriteJob,
    reply: oneshot::Sender<BoxedResult>,
}

/// Handle to the single-writer actor. Cheap to clone; all clones feed the same
/// actor thread. Every SQLite write in Core goes through here so the one WAL
/// writer lock is never contended — `database is locked` becomes impossible and
/// the checkpoint (run by the actor between job batches) always gets its window.
#[derive(Clone)]
pub struct DbWriter {
    inner: Arc<DbWriterInner>,
}

struct DbWriterInner {
    sender: mpsc::UnboundedSender<QueuedJob>,
    /// Process-stable identity for this actor (shared across clones), used for
    /// per-Core-instance bookkeeping like wakeup de-duplication.
    instance_id: usize,
    notification_flush_signal: Arc<NotificationFlushSignal>,
}

impl Drop for DbWriterInner {
    fn drop(&mut self) {
        self.notification_flush_signal.notify();
    }
}

pub(super) struct NotificationFlushSignal {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl NotificationFlushSignal {
    pub(super) fn new() -> Self {
        Self {
            generation: Mutex::new(0),
            changed: Condvar::new(),
        }
    }

    pub(super) fn notify(&self) {
        let mut generation = self
            .generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *generation = generation.wrapping_add(1);
        self.changed.notify_one();
    }

    fn generation(&self) -> u64 {
        *self
            .generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn wait_for_change(&self, observed: u64, timeout: Option<Duration>) -> bool {
        let generation = self
            .generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *generation != observed {
            return true;
        }
        match timeout {
            Some(timeout) => {
                let (generation, _) = self
                    .changed
                    .wait_timeout_while(generation, timeout, |value| *value == observed)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *generation != observed
            }
            None => {
                let generation = self
                    .changed
                    .wait_while(generation, |value| *value == observed)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *generation != observed
            }
        }
    }
}

impl DbWriter {
    /// Start the actor thread over the given write connection and return a handle.
    pub fn spawn(database: Database) -> DbWriter {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT_INSTANCE_ID: AtomicUsize = AtomicUsize::new(1);
        let notification_flush_signal = database.notification_flush_signal();
        let (sender, receiver) = mpsc::unbounded_channel::<QueuedJob>();
        std::thread::Builder::new()
            .name("megle-db-writer".to_string())
            .spawn(move || run_writer_actor(database, receiver))
            .expect("spawn db writer thread");
        let writer = DbWriter {
            inner: Arc::new(DbWriterInner {
                sender,
                instance_id: NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed),
                notification_flush_signal: Arc::clone(&notification_flush_signal),
            }),
        };
        let scheduler_writer = Arc::downgrade(&writer.inner);
        std::thread::Builder::new()
            .name("megle-notification-flush".to_string())
            .spawn(move || {
                run_notification_flush_scheduler(scheduler_writer, notification_flush_signal)
            })
            .expect("spawn notification flush scheduler");
        writer
    }

    /// A process-stable identity for this writer (and thus this Core instance).
    /// Clones share it; distinct actors differ.
    pub fn instance_id(&self) -> usize {
        self.inner.instance_id
    }

    /// Submit a write job and await its typed result. The closure runs on the
    /// actor thread against the sole write connection. A job that returns `Err`
    /// delivers that error here without poisoning the connection or the actor.
    pub async fn write<T, F>(&self, op: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static,
    {
        let (reply, reply_rx) = oneshot::channel();
        self.enqueue(op, reply)?;
        let boxed = reply_rx
            .await
            .map_err(|_| anyhow::anyhow!("database writer dropped the job"))??;
        downcast(boxed)
    }

    /// Blocking variant for non-async callers (the watcher runs on a plain
    /// `std::thread`). Same serialization + error guarantees as `write`.
    pub fn write_blocking<T, F>(&self, op: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static,
    {
        let (reply, reply_rx) = oneshot::channel();
        self.enqueue(op, reply)?;
        let boxed = reply_rx
            .blocking_recv()
            .map_err(|_| anyhow::anyhow!("database writer dropped the job"))??;
        downcast(boxed)
    }

    fn enqueue<T, F>(&self, op: F, reply: oneshot::Sender<BoxedResult>) -> anyhow::Result<()>
    where
        T: Send + 'static,
        F: FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static,
    {
        let job: WriteJob = Box::new(move |database| {
            op(database).map(|value| Box::new(value) as Box<dyn Any + Send>)
        });
        self.inner
            .sender
            .send(QueuedJob { job, reply })
            .map_err(|_| anyhow::anyhow!("database writer is unavailable"))
    }
}

fn downcast<T: 'static>(boxed: Box<dyn Any + Send>) -> anyhow::Result<T> {
    boxed
        .downcast::<T>()
        .map(|value| *value)
        .map_err(|_| anyhow::anyhow!("database writer returned an unexpected type"))
}

fn run_writer_actor(mut database: Database, mut receiver: mpsc::UnboundedReceiver<QueuedJob>) {
    crate::debug_log!("writer actor started");
    while let Some(first) = receiver.blocking_recv() {
        crate::debug_log!("writer running a job");
        run_one(&mut database, first);
        while let Ok(next) = receiver.try_recv() {
            run_one(&mut database, next);
        }
        let checkpoint_start = std::time::Instant::now();
        if let Err(error) = database.run_wal_checkpoint_pass() {
            tracing::warn!(%error, "writer actor checkpoint pass failed");
        }
        let elapsed = checkpoint_start.elapsed();
        crate::debug_log!(
            "writer checkpoint pass done in {}ms, wal now {} bytes",
            elapsed.as_millis(),
            database.wal_size_bytes()
        );
    }
    if let Err(error) = database.flush_dirty_notification_heads() {
        tracing::warn!(%error, "writer shutdown notification flush failed");
    }
    crate::debug_log!("writer actor exited (channel closed)");
}

fn run_notification_flush_scheduler(
    writer: Weak<DbWriterInner>,
    signal: Arc<NotificationFlushSignal>,
) {
    loop {
        let observed = signal.generation();
        let Some(inner) = writer.upgrade() else {
            break;
        };
        let handle = DbWriter { inner };
        let next_due =
            handle.write_blocking(|database| database.next_dirty_notification_flush_at());
        drop(handle);
        let next_due = match next_due {
            Ok(next_due) => next_due,
            Err(error) => {
                tracing::warn!(%error, "notification trailing-flush scheduling query failed");
                if !signal.wait_for_change(observed, None) {
                    continue;
                }
                continue;
            }
        };
        let Some(next_due) = next_due else {
            signal.wait_for_change(observed, None);
            continue;
        };
        let now = crate::db::unix_timestamp_millis();
        if next_due > now {
            let timeout = Duration::from_millis((next_due - now) as u64);
            if signal.wait_for_change(observed, Some(timeout)) {
                continue;
            }
        }
        let Some(inner) = writer.upgrade() else {
            break;
        };
        let handle = DbWriter { inner };
        let flush_at = crate::db::unix_timestamp_millis();
        let flush_result =
            handle.write_blocking(move |database| database.flush_due_notification_heads(flush_at));
        drop(handle);
        if let Err(error) = flush_result {
            tracing::warn!(%error, "notification trailing flush failed");
            let failed_generation = signal.generation();
            let Some(liveness) = writer.upgrade() else {
                break;
            };
            drop(liveness);
            signal.wait_for_change(failed_generation, None);
        }
    }
}

fn run_one(database: &mut Database, queued: QueuedJob) {
    let started = std::time::Instant::now();
    let result = (queued.job)(database);
    crate::debug_log!(
        "writer job closure returned in {}ms (ok={})",
        started.elapsed().as_millis(),
        result.is_ok()
    );
    // Ignore send failure: the caller may have been aborted/dropped.
    let _ = queued.reply.send(result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::notification_outbox::{NotificationOperation, PublishMode};
    use crate::db::Database;

    fn notification_operation(operation_id: &str, updated_at: i64) -> NotificationOperation {
        NotificationOperation {
            operation_id: operation_id.to_string(),
            revision: 0,
            kind: "root_scan".to_string(),
            category: "task".to_string(),
            origin: "user".to_string(),
            presentation: "important".to_string(),
            status: "running".to_string(),
            phase: Some("working".to_string()),
            title: "Scan".to_string(),
            summary: None,
            task_id: Some(1),
            root_id: Some(1),
            folder_id: None,
            file_id: None,
            current_value: 0,
            total_value: Some(100),
            unit: Some("items".to_string()),
            error_code: None,
            error_detail: None,
            cancellable: true,
            retryable: false,
            attempt: 1,
            occurrence_count: 1,
            started_at: updated_at,
            updated_at,
            completed_at: None,
        }
    }

    async fn publish_notification(
        writer: &DbWriter,
        mut operation: NotificationOperation,
        mode: PublishMode,
    ) -> NotificationOperation {
        writer
            .write(move |database| {
                database.publish_notification_operation(&mut operation, mode)?;
                Ok(operation)
            })
            .await
            .expect("publish notification")
    }

    async fn wait_for_notification_revision(
        writer: &DbWriter,
        revision: i64,
    ) -> Vec<crate::db::notification_outbox::NotificationOutboxEvent> {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let events = writer
                    .write(|database| database.list_notification_outbox(0, 200))
                    .await
                    .expect("list notification outbox");
                if events
                    .last()
                    .is_some_and(|event| event.operation.revision >= revision)
                {
                    return events;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("notification trailing flush must complete")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notification_progress_scheduler_flushes_the_latest_burst_value() {
        let mut database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        database.set_notification_progress_interval_for_test(500);
        let writer = DbWriter::spawn(database);
        let mut operation =
            notification_operation("task:trailing-burst", crate::db::unix_timestamp_millis());
        operation = publish_notification(&writer, operation, PublishMode::Phase).await;
        for value in 1..=20 {
            operation.current_value = value;
            operation.updated_at = crate::db::unix_timestamp_millis();
            operation = publish_notification(&writer, operation, PublishMode::Progress).await;
        }

        let events = wait_for_notification_revision(&writer, operation.revision).await;
        assert_eq!(
            events.len(),
            2,
            "the burst should produce one trailing event"
        );
        assert_eq!(events.last().unwrap().operation.current_value, 20);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notification_progress_scheduler_flushes_continuous_updates_each_window() {
        let mut database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        database.set_notification_progress_interval_for_test(30);
        let writer = DbWriter::spawn(database);
        let mut operation =
            notification_operation("task:continuous", crate::db::unix_timestamp_millis());
        operation = publish_notification(&writer, operation, PublishMode::Phase).await;
        for value in 1..=9 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            operation.current_value = value;
            operation.updated_at = crate::db::unix_timestamp_millis();
            operation = publish_notification(&writer, operation, PublishMode::Progress).await;
        }

        let events = wait_for_notification_revision(&writer, operation.revision).await;
        assert!(
            events.len() >= 3,
            "continuous progress should flush once per window"
        );
        assert_eq!(events.last().unwrap().operation.current_value, 9);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_notification_cancels_the_pending_progress_flush() {
        let mut database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        database.set_notification_progress_interval_for_test(80);
        let writer = DbWriter::spawn(database);
        let mut operation =
            notification_operation("task:terminal-cancels", crate::db::unix_timestamp_millis());
        operation = publish_notification(&writer, operation, PublishMode::Phase).await;
        operation.current_value = 1;
        operation.updated_at = crate::db::unix_timestamp_millis();
        operation = publish_notification(&writer, operation, PublishMode::Progress).await;
        operation.status = "succeeded".to_string();
        operation.completed_at = Some(crate::db::unix_timestamp_millis());
        operation.updated_at = operation.completed_at.unwrap();
        operation = publish_notification(&writer, operation, PublishMode::Terminal).await;

        tokio::time::sleep(std::time::Duration::from_millis(160)).await;
        let events = writer
            .write(|database| database.list_notification_outbox(0, 200))
            .await
            .expect("list notification outbox");
        assert_eq!(
            events
                .iter()
                .map(|event| event.operation.revision)
                .collect::<Vec<_>>(),
            vec![1, operation.revision]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reopened_notification_producer_cannot_lose_the_scheduler_wake() {
        let dir = std::env::temp_dir().join(format!(
            "megle-notification-wake-{}-{}",
            std::process::id(),
            crate::db::unix_timestamp_millis()
        ));
        std::fs::create_dir_all(&dir).expect("create notification test dir");
        let db_path = dir.join("wake.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        database.set_notification_progress_interval_for_test(40);
        let producer = database
            .reopen()
            .expect("reopen producer")
            .expect("file database");
        let writer = DbWriter::spawn(database);
        let mut operation =
            notification_operation("task:reopened-producer", crate::db::unix_timestamp_millis());
        producer
            .publish_notification_operation(&mut operation, PublishMode::Phase)
            .expect("publish phase from reopened connection");
        operation.current_value = 7;
        operation.updated_at = crate::db::unix_timestamp_millis();
        producer
            .publish_notification_operation(&mut operation, PublishMode::Progress)
            .expect("publish dirty progress from reopened connection");

        let events = wait_for_notification_revision(&writer, operation.revision).await;
        assert_eq!(events.last().unwrap().operation.current_value, 7);
        drop(producer);
        drop(writer);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn write_completes_over_a_file_db_with_a_concurrent_writer_connection() {
        // Reproduces Core's real setup: a FILE-backed db where the writer actor
        // owns one connection while another connection (like the worker) also
        // writes. A write submitted to the actor must complete, not hang.
        let dir = std::env::temp_dir().join(format!("megle-writer-repro-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("repro.sqlite");
        let _ = std::fs::remove_file(&db_path);
        let database = Database::open(&db_path).expect("open file db");
        database.apply_migrations().expect("apply migrations");
        // Seed a root+folder so create_interactive_folder_scan_task-style writes
        // have something to reference is unnecessary; a trivial write suffices.
        let second = database.reopen().expect("reopen").expect("second conn");
        let writer = DbWriter::spawn(database);

        // Second connection does a quick write (holds the write lock briefly),
        // mirroring a worker blob commit racing the actor.
        second
            .create_tag("concurrent", None)
            .expect("second connection write");

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            writer.write(|db| Ok(db.list_tags()?.len())),
        )
        .await
        .expect("writer.write must not hang over a file db");
        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn write_returns_typed_result() {
        let database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        let writer = DbWriter::spawn(database);
        let value = writer
            .write(|_db| Ok(41 + 1))
            .await
            .expect("write should succeed");
        assert_eq!(value, 42);
    }

    #[tokio::test]
    async fn write_error_is_delivered_and_actor_survives() {
        let database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        let writer = DbWriter::spawn(database);
        let err = writer
            .write(|_db| -> anyhow::Result<i32> { Err(anyhow::anyhow!("boom")) })
            .await
            .expect_err("write should return the job error");
        assert!(err.to_string().contains("boom"));
        // Actor still alive: a subsequent write succeeds.
        let ok = writer.write(|_db| Ok(7)).await.expect("next write ok");
        assert_eq!(ok, 7);
    }

    #[tokio::test]
    async fn concurrent_writes_are_serialized() {
        let database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        let writer = DbWriter::spawn(database);
        let mut handles = Vec::new();
        for i in 0..25 {
            let w = writer.clone();
            handles.push(tokio::spawn(async move { w.write(move |_db| Ok(i)).await }));
        }
        let mut results = Vec::new();
        for h in handles {
            results.push(h.await.unwrap().unwrap());
        }
        results.sort_unstable();
        assert_eq!(results, (0..25).collect::<Vec<_>>());
    }

    /// Insert a root + a root folder + `n` file rows and return their file ids,
    /// so blob writes below have a real FK target. Mirrors
    /// `db::tests::seed_media_page_fixture`.
    fn seed_files_for_blobs(database: &Database, n: usize) -> Vec<i64> {
        use crate::db::{FileUpsert, FolderUpsert, NewRoot};
        let root_id = database
            .add_root(NewRoot {
                path: format!("D:/megle-wal-bound-{}", std::process::id()),
                display_name: "wal-bound-root".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root-hash".to_string(),
                mtime: Some(0),
            })
            .expect("insert root folder");
        (0..n)
            .map(|i| {
                database
                    .upsert_file(FileUpsert {
                        root_id,
                        folder_id,
                        name: format!("file-{i}.jpg"),
                        ext: ".jpg".to_string(),
                        size: 4096,
                        mtime: i as i64,
                        ctime: None,
                        file_key: Some(format!("wal-bound-file-{i}")),
                    })
                    .expect("insert file")
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sustained_blob_writes_through_writer_keep_wal_bounded() {
        use crate::db::Database;
        let dir = std::env::temp_dir().join(format!("megle-wal-bound-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("bound.sqlite");
        let _ = std::fs::remove_file(&db_path);
        let seed = Database::open(&db_path).expect("open");
        seed.apply_migrations().expect("migrate");
        // Seed enough files to attach blobs to (satisfy FK if any).
        //
        // NOTE: N=400 (~25MB) as suggested in the task brief does not clear the
        // 128MiB bound even with checkpointing fully disabled (writes are
        // sequential/awaited one at a time, and 25MB alone can't exceed 128MB).
        // Bumped to 3000 (~192MB of raw blob payload) so the no-checkpoint case
        // clearly overshoots the bound while the checkpointed case still drains
        // back near zero after each job.
        let file_ids = seed_files_for_blobs(&seed, 3000);
        let probe = seed.reopen().expect("reopen").expect("probe conn");
        let writer = DbWriter::spawn(seed);

        // Write ~3000 64KB blobs through the writer (≈192MB of data). Without
        // checkpointing this pushes the WAL well past the 128MiB bound; the
        // writer checkpoints between batches so it stays bounded instead.
        let payload = vec![0u8; 64 * 1024];
        for (i, file_id) in file_ids.into_iter().enumerate() {
            let data = payload.clone();
            writer
                .write(move |db| {
                    db.upsert_thumb_blob(crate::db::ThumbBlobRecord {
                        file_id,
                        profile: "grid_320".to_string(),
                        data,
                        width: 320,
                        height: 240,
                        byte_size: (64 * 1024) as i64,
                        output_format: "image/webp".to_string(),
                        created_at: i as i64,
                        updated_at: i as i64,
                    })
                })
                .await
                .expect("blob write");
        }
        // Give the actor a moment to drain + checkpoint the final batch.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let wal = probe.wal_size_bytes();
        assert!(
            wal < 128 * 1024 * 1024,
            "WAL must stay bounded well under repeated-blob growth, got {} bytes",
            wal
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
