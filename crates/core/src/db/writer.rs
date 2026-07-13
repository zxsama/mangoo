use std::any::Any;

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
    sender: mpsc::UnboundedSender<QueuedJob>,
    /// Process-stable identity for this actor (shared across clones), used for
    /// per-Core-instance bookkeeping like wakeup de-duplication.
    instance_id: usize,
}

impl DbWriter {
    /// Start the actor thread over the given write connection and return a handle.
    pub fn spawn(database: Database) -> DbWriter {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT_INSTANCE_ID: AtomicUsize = AtomicUsize::new(1);
        let (sender, receiver) = mpsc::unbounded_channel::<QueuedJob>();
        std::thread::Builder::new()
            .name("megle-db-writer".to_string())
            .spawn(move || run_writer_actor(database, receiver))
            .expect("spawn db writer thread");
        DbWriter {
            sender,
            instance_id: NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// A process-stable identity for this writer (and thus this Core instance).
    /// Clones share it; distinct actors differ.
    pub fn instance_id(&self) -> usize {
        self.instance_id
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
    #[allow(dead_code)] // Reserved for the watcher/worker write migration.
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
        self.sender
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
    crate::debug_log!("writer actor exited (channel closed)");
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
    use crate::db::Database;

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
