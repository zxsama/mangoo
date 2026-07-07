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
    // Block for the next job; run it and any already-queued jobs; then, when the
    // queue momentarily drains, run one checkpoint pass before blocking again.
    // Batching keeps the checkpoint off the per-write hot path while guaranteeing
    // it runs — and since this is the only writer, its TRUNCATE always wins.
    while let Some(first) = receiver.blocking_recv() {
        run_one(&mut database, first);
        while let Ok(next) = receiver.try_recv() {
            run_one(&mut database, next);
        }
        if let Err(error) = database.run_wal_checkpoint_pass() {
            tracing::warn!(%error, "writer actor checkpoint pass failed");
        }
    }
}

fn run_one(database: &mut Database, queued: QueuedJob) {
    let result = (queued.job)(database);
    // Ignore send failure: the caller may have been aborted/dropped.
    let _ = queued.reply.send(result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

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
}
