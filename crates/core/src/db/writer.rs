use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::db::Database;

/// A boxed write job: run against the single write connection, reply with the
/// boxed result over the oneshot. The closure returns `Box<dyn Any + Send>` so
/// jobs of different result types share one channel; `write` downcasts.
type BoxedResult = anyhow::Result<Box<dyn Any + Send>>;
type WriteJob = Box<dyn FnOnce(&mut Database) -> BoxedResult + Send>;
type AuthoritativeCommitHook = Arc<dyn Fn() + Send + Sync>;

/// Durability promised by a writer turn. Regenerable cache/index work remains
/// on SQLite WAL `NORMAL`; user-authored and synchronization truth must commit
/// under `FULL` so an acknowledged mutation survives a power loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteDurability {
    Derived,
    Authoritative,
}

struct QueuedJob {
    job: WriteJob,
    durability: WriteDurability,
    notify_authoritative_commit: bool,
    reply: oneshot::Sender<BoxedResult>,
}

enum WriterMessage {
    Job(QueuedJob),
    Shutdown(oneshot::Sender<()>),
}

/// Handle to the single-writer actor. Cheap to clone; all clones feed the same
/// actor thread. Every SQLite write in Core goes through here so the one WAL
/// writer lock is never contended — `database is locked` becomes impossible and
/// the checkpoint (run by the actor between job batches) always gets its window.
#[derive(Clone)]
pub struct DbWriter {
    inner: Arc<DbWriterInner>,
    notify_authoritative_commit: bool,
    write_admission: Option<Arc<dyn Fn() -> anyhow::Result<()> + Send + Sync>>,
}

struct DbWriterInner {
    sender: mpsc::UnboundedSender<WriterMessage>,
    /// Process-stable identity for this actor (shared across clones), used for
    /// per-Core-instance bookkeeping like wakeup de-duplication.
    instance_id: usize,
    notification_flush_signal: Arc<NotificationFlushSignal>,
    closed: AtomicBool,
    join_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    scheduler_join_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
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
        Self::spawn_with_authoritative_commit_hook(database, None)
    }

    /// Start the actor with a callback invoked after every successful
    /// authoritative commit. Active Library runtimes use this to wake the Host
    /// sync coordinator for API, watcher, task, and filesystem mutations
    /// without coupling those producers to the scheduler.
    pub fn spawn_with_authoritative_commit_hook(
        database: Database,
        authoritative_commit_hook: Option<AuthoritativeCommitHook>,
    ) -> DbWriter {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT_INSTANCE_ID: AtomicUsize = AtomicUsize::new(1);
        let notification_flush_signal = database.notification_flush_signal();
        let (sender, receiver) = mpsc::unbounded_channel::<WriterMessage>();
        let join_handle = std::thread::Builder::new()
            .name("mangoo-db-writer".to_string())
            .spawn(move || run_writer_actor(database, receiver, authoritative_commit_hook))
            .expect("spawn db writer thread");
        let writer = DbWriter {
            inner: Arc::new(DbWriterInner {
                sender,
                instance_id: NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed),
                notification_flush_signal: Arc::clone(&notification_flush_signal),
                closed: AtomicBool::new(false),
                join_handle: Mutex::new(Some(join_handle)),
                scheduler_join_handle: Mutex::new(None),
            }),
            notify_authoritative_commit: true,
            write_admission: None,
        };
        let scheduler_writer = Arc::downgrade(&writer.inner);
        let scheduler_join_handle = std::thread::Builder::new()
            .name("mangoo-notification-flush".to_string())
            .spawn(move || {
                run_notification_flush_scheduler(scheduler_writer, notification_flush_signal)
            })
            .expect("spawn notification flush scheduler");
        *writer
            .inner
            .scheduler_join_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(scheduler_join_handle);
        writer
    }

    /// A process-stable identity for this writer (and thus this Core instance).
    /// Clones share it; distinct actors differ.
    pub fn instance_id(&self) -> usize {
        self.inner.instance_id
    }

    /// Clone this writer for a coordinator-owned turn. Such work still uses
    /// authoritative durability, but must not be mistaken for a new local
    /// mutation and wake the coordinator recursively.
    pub(crate) fn without_authoritative_commit_wake(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            notify_authoritative_commit: false,
            write_admission: self.write_admission.clone(),
        }
    }

    /// Scope a handle to owned background work. The queue retains admission
    /// ownership even when its awaiting future is dropped.
    pub(crate) fn with_write_admission(
        &self,
        admission: Arc<dyn Fn() -> anyhow::Result<()> + Send + Sync>,
    ) -> Self {
        let mut writer = self.clone();
        let previous = writer.write_admission.take();
        writer.write_admission = Some(Arc::new(move || {
            if let Some(previous) = &previous {
                previous()?;
            }
            admission()
        }));
        writer
    }

    /// Stop accepting writes, drain all writes queued before this call, flush,
    /// and join the actor thread. Safe to call repeatedly from runtime teardown.
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        if !self.inner.closed.swap(true, Ordering::AcqRel) {
            self.inner.notification_flush_signal.notify();
            let (reply, reply_rx) = oneshot::channel();
            self.inner
                .sender
                .send(WriterMessage::Shutdown(reply))
                .map_err(|_| anyhow::anyhow!("database writer is unavailable during shutdown"))?;
            let _ = reply_rx.await;
        }
        self.inner.notification_flush_signal.notify();
        let join = self
            .inner
            .join_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(join) = join {
            tokio::task::spawn_blocking(move || join.join())
                .await?
                .map_err(|_| anyhow::anyhow!("database writer thread panicked"))?;
        }
        let scheduler_join = self
            .inner
            .scheduler_join_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(join) = scheduler_join {
            tokio::task::spawn_blocking(move || join.join())
                .await?
                .map_err(|_| anyhow::anyhow!("notification flush scheduler thread panicked"))?;
        }
        Ok(())
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
        self.enqueue(op, WriteDurability::Derived, false, reply)?;
        let boxed = reply_rx
            .await
            .map_err(|_| anyhow::anyhow!("database writer dropped the job"))??;
        downcast(boxed)
    }

    /// Submit a user/sync truth mutation. The writer changes the connection's
    /// durability before the closure starts and restores `NORMAL` afterwards.
    pub async fn write_authoritative<T, F>(&self, op: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static,
    {
        let (reply, reply_rx) = oneshot::channel();
        self.enqueue(
            op,
            WriteDurability::Authoritative,
            self.notify_authoritative_commit,
            reply,
        )?;
        let boxed = reply_rx
            .await
            .map_err(|_| anyhow::anyhow!("database writer dropped the job"))??;
        downcast(boxed)
    }

    /// Submit an authoritative synchronization turn without re-triggering the
    /// local-commit wake hook. Remote apply still gets FULL durability, while
    /// avoiding a scheduler self-wake loop after every successful pull turn.
    pub(crate) async fn write_authoritative_without_wake<T, F>(&self, op: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static,
    {
        let (reply, reply_rx) = oneshot::channel();
        self.enqueue(op, WriteDurability::Authoritative, false, reply)?;
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
        self.enqueue(op, WriteDurability::Derived, false, reply)?;
        let boxed = reply_rx
            .blocking_recv()
            .map_err(|_| anyhow::anyhow!("database writer dropped the job"))??;
        downcast(boxed)
    }

    pub fn write_authoritative_blocking<T, F>(&self, op: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static,
    {
        let (reply, reply_rx) = oneshot::channel();
        self.enqueue(
            op,
            WriteDurability::Authoritative,
            self.notify_authoritative_commit,
            reply,
        )?;
        let boxed = reply_rx
            .blocking_recv()
            .map_err(|_| anyhow::anyhow!("database writer dropped the job"))??;
        downcast(boxed)
    }

    fn enqueue<T, F>(
        &self,
        op: F,
        durability: WriteDurability,
        notify_authoritative_commit: bool,
        reply: oneshot::Sender<BoxedResult>,
    ) -> anyhow::Result<()>
    where
        T: Send + 'static,
        F: FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static,
    {
        let admission = self.write_admission.clone();
        let job: WriteJob = Box::new(move |database| {
            if let Some(admission) = &admission {
                admission()?;
            }
            op(database).map(|value| Box::new(value) as Box<dyn Any + Send>)
        });
        if self.inner.closed.load(Ordering::Acquire) {
            anyhow::bail!("database writer is closed");
        }
        self.inner
            .sender
            .send(WriterMessage::Job(QueuedJob {
                job,
                durability,
                notify_authoritative_commit,
                reply,
            }))
            .map_err(|_| anyhow::anyhow!("database writer is unavailable"))
    }
}

fn downcast<T: 'static>(boxed: Box<dyn Any + Send>) -> anyhow::Result<T> {
    boxed
        .downcast::<T>()
        .map(|value| *value)
        .map_err(|_| anyhow::anyhow!("database writer returned an unexpected type"))
}

fn run_writer_actor(
    mut database: Database,
    mut receiver: mpsc::UnboundedReceiver<WriterMessage>,
    authoritative_commit_hook: Option<AuthoritativeCommitHook>,
) {
    crate::debug_log!("writer actor started");
    let mut current_durability = WriteDurability::Derived;
    while let Some(message) = receiver.blocking_recv() {
        let first = match message {
            WriterMessage::Job(job) => job,
            WriterMessage::Shutdown(reply) => {
                let _ = reply.send(());
                break;
            }
        };
        crate::debug_log!("writer running a job");
        run_one(
            &mut database,
            first,
            &mut current_durability,
            authoritative_commit_hook.as_ref(),
        );
        let mut shutdown_reply = None;
        while let Ok(next) = receiver.try_recv() {
            match next {
                WriterMessage::Job(job) => run_one(
                    &mut database,
                    job,
                    &mut current_durability,
                    authoritative_commit_hook.as_ref(),
                ),
                WriterMessage::Shutdown(reply) => {
                    shutdown_reply = Some(reply);
                    break;
                }
            }
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
        if let Some(reply) = shutdown_reply {
            let _ = reply.send(());
            break;
        }
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
        if inner.closed.load(Ordering::Acquire) {
            break;
        }
        let handle = DbWriter {
            inner,
            notify_authoritative_commit: false,
            write_admission: None,
        };
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
        if inner.closed.load(Ordering::Acquire) {
            break;
        }
        let handle = DbWriter {
            inner,
            notify_authoritative_commit: false,
            write_admission: None,
        };
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

fn run_one(
    database: &mut Database,
    queued: QueuedJob,
    current_durability: &mut WriteDurability,
    authoritative_commit_hook: Option<&AuthoritativeCommitHook>,
) {
    let started = std::time::Instant::now();
    let result = if *current_durability == queued.durability {
        (queued.job)(database)
    } else {
        database
            .set_write_durability(queued.durability)
            .and_then(|_| {
                *current_durability = queued.durability;
                (queued.job)(database)
            })
    };
    crate::debug_log!(
        "writer job closure returned in {}ms (ok={})",
        started.elapsed().as_millis(),
        result.is_ok()
    );
    if queued.durability == WriteDurability::Authoritative
        && queued.notify_authoritative_commit
        && result.is_ok()
    {
        if let Some(hook) = authoritative_commit_hook {
            hook();
        }
    }
    // Ignore send failure: the caller may have been aborted/dropped.
    let _ = queued.reply.send(result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::notification_outbox::{NotificationOperation, PublishMode};
    use crate::db::Database;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn authoritative_turn_runs_full_and_restores_normal() {
        let database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        let writer = DbWriter::spawn(database);
        let observed = writer
            .write_authoritative(|database| database.write_durability_for_test())
            .await
            .expect("authoritative turn");
        assert_eq!(observed, 2);
        let restored = writer
            .write(|database| database.write_durability_for_test())
            .await
            .expect("derived turn");
        assert_eq!(restored, 1);
        writer.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn scheduler_authoritative_turn_does_not_retrigger_local_commit_wake() {
        let database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        let wakes = Arc::new(AtomicUsize::new(0));
        let observed_wakes = Arc::clone(&wakes);
        let writer = DbWriter::spawn_with_authoritative_commit_hook(
            database,
            Some(Arc::new(move || {
                observed_wakes.fetch_add(1, Ordering::SeqCst);
            })),
        );

        writer
            .write_authoritative_without_wake(|database| database.write_durability_for_test())
            .await
            .expect("scheduler authoritative turn");
        assert_eq!(wakes.load(Ordering::SeqCst), 0);

        writer
            .without_authoritative_commit_wake()
            .write_authoritative(|database| database.write_durability_for_test())
            .await
            .expect("scheduler-owned helper turn");
        assert_eq!(wakes.load(Ordering::SeqCst), 0);

        writer
            .write_authoritative(|database| database.write_durability_for_test())
            .await
            .expect("local authoritative turn");
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        writer.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn authoritative_failure_still_restores_normal() {
        let database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        let writer = DbWriter::spawn(database);
        let failure = writer
            .write_authoritative::<(), _>(|database| {
                assert_eq!(database.write_durability_for_test()?, 2);
                anyhow::bail!("failpoint")
            })
            .await;
        assert!(failure.is_err());
        assert_eq!(
            writer
                .write(|database| database.write_durability_for_test())
                .await
                .unwrap(),
            1
        );
        writer.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn captured_user_mutation_commits_event_and_outbox_together() {
        let database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        database
            .add_root(crate::db::NewRoot {
                path: "C:\\managed".into(),
                display_name: "Managed".into(),
            })
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        database.enable_fake_sync_capture_for_test().unwrap();
        let writer = DbWriter::spawn(database);
        writer
            .write_authoritative(|database| {
                database
                    .create_tag("captured", None)
                    .map_err(anyhow::Error::from)?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            writer
                .write(|database| database.sync_capture_counts_for_test())
                .await
                .unwrap(),
            (2, 2, 3)
        );
        writer.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn publication_intent_does_not_consume_actor_sequence_until_manifest_bound() {
        let database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: "C:\\managed-content".into(),
                display_name: "Managed".into(),
            })
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        database
            .execute_batch_for_test(&format!(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,status) VALUES ({root_id},NULL,'','root','active');\
                 INSERT INTO files(root_id,folder_id,name,ext,size,mtime,status) VALUES ({root_id},last_insert_rowid(),'a.jpg','jpg',12,1,'active');"
            ))
            .unwrap();
        database.enable_fake_sync_capture_for_test().unwrap();
        let writer = DbWriter::spawn(database);
        let intent = writer
            .write_authoritative(|database| {
                database.create_sync_publication_intent(1, "C:\\managed-content\\a.jpg", 12)
            })
            .await
            .unwrap();
        assert_eq!(
            writer
                .write(|database| database.sync_capture_counts_for_test())
                .await
                .unwrap(),
            (0, 0, 1)
        );
        writer
            .write_authoritative(move |database| database.bind_fake_sync_manifest(intent, [7; 32]))
            .await
            .unwrap();
        assert_eq!(
            writer
                .write(|database| database.sync_capture_counts_for_test())
                .await
                .unwrap(),
            (1, 1, 2)
        );
        writer.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn real_manifest_binding_requires_matching_durable_receipts() {
        let database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: "C:\\managed-content".into(),
                display_name: "Managed".into(),
            })
            .unwrap();
        let library = uuid::Uuid::now_v7();
        let binding = uuid::Uuid::now_v7();
        database
            .initialize_sync_identity(library, uuid::Uuid::now_v7(), uuid::Uuid::now_v7())
            .unwrap();
        database
            .begin_sync_capture(binding, uuid::Uuid::now_v7())
            .unwrap();
        database
            .execute_batch_for_test(&format!(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,status) VALUES ({root_id},NULL,'','root','active');\
                 INSERT INTO files(root_id,folder_id,name,ext,size,mtime,status) VALUES ({root_id},last_insert_rowid(),'a.jpg','jpg',12,1,'active');"
            ))
            .unwrap();
        let writer = DbWriter::spawn(database);
        let intent = writer
            .write_authoritative(|database| {
                database.create_sync_publication_intent(1, "C:\\managed-content\\a.jpg", 12)
            })
            .await
            .unwrap();
        let manifest_hash = [7; 32];
        writer
            .write_authoritative(move |database| {
                let connection = database.connection_for_fsops();
                connection.execute(
                    "UPDATE media_publication_jobs SET phase='canonicalizing',transport_generation=1 WHERE intent_uuid=?1",
                    [intent.as_bytes().as_slice()],
                )?;
                connection.execute(
                    "INSERT INTO media_manifests(manifest_hash,content_hash,byte_length,manifest_pack_hash,pack_offset,record_length,state,created_at,updated_at) VALUES (?1,?2,12,?3,0,12,'sealed',1,1)",
                    rusqlite::params![manifest_hash.as_slice(),[8_u8;32].as_slice(),[9_u8;32].as_slice()],
                )?;
                connection.execute(
                    "INSERT INTO media_content_manifest_links(content_revision_uuid,manifest_hash,linked_at) SELECT content_revision_uuid,?1,1 FROM sync_publication_intents WHERE intent_uuid=?2",
                    rusqlite::params![manifest_hash.as_slice(),intent.as_bytes().as_slice()],
                )?;
                connection.execute(
                    "INSERT INTO media_publication_objects(intent_uuid,ordinal,object_kind,object_hash,object_key,byte_length,local_relpath,state,updated_at) VALUES (?1,0,'manifest-pack',?2,'content/manifests/test.mpack',1,'manifests/test.mpack','sealed',1)",
                    rusqlite::params![intent.as_bytes().as_slice(),[9_u8;32].as_slice()],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(writer
            .write_authoritative(move |database| {
                database.bind_sync_manifest(intent, manifest_hash, 1)
            })
            .await
            .is_err());
        writer
            .write_authoritative(move |database| {
                database.connection_for_fsops().execute(
                    "UPDATE media_publication_objects SET state='durable',receipt_generation=1 WHERE intent_uuid=?1",
                    [intent.as_bytes().as_slice()],
                )?;
                database.bind_sync_manifest(intent, manifest_hash, 1)
            })
            .await
            .unwrap();
        assert_eq!(
            writer
                .write(|database| database.sync_capture_counts_for_test())
                .await
                .unwrap(),
            (1, 1, 2)
        );
        writer.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn remote_apply_is_bounded_idempotent_and_never_echoes() {
        use crate::sync::test_model::event;
        use crate::sync::types::{EventUuid, Operation};
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        database
            .add_root(crate::db::NewRoot {
                path: "C:\\remote".into(),
                display_name: "Remote".into(),
            })
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        let first = event(
            1,
            10,
            Operation::Delete {
                token: EventUuid::new(),
            },
        );
        assert_eq!(
            database.apply_remote_sync_events(&[first.clone()]).unwrap(),
            1
        );
        assert_eq!(database.apply_remote_sync_events(&[first]).unwrap(), 0);
        let counts: (i64, i64) = database
            .connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sync_events),(SELECT COUNT(*) FROM sync_outbox)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (1, 0));
    }

    #[test]
    fn remote_scalar_projection_materializes_the_lww_winner_not_arrival_order() {
        use crate::sync::test_model::event;
        use crate::sync::types::{DeviceUuid, Operation};

        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let root_id = database
            .add_root(crate::db::NewRoot {
                path: "C:\\remote-lww".into(),
                display_name: "Remote LWW".into(),
            })
            .unwrap();
        let folder_id = database
            .upsert_folder(crate::db::FolderUpsert {
                root_id,
                parent_id: None,
                name: "Remote LWW".into(),
                path_hash: "remote-lww-root".into(),
                mtime: None,
            })
            .unwrap();
        let file_id = database
            .upsert_file(crate::db::FileUpsert {
                root_id,
                folder_id,
                name: "image.jpg".into(),
                ext: ".jpg".into(),
                size: 10,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        let asset_uuid: Vec<u8> = database
            .connection
            .query_row(
                "SELECT asset_uuid FROM files WHERE id=?1",
                [file_id],
                |row| row.get(0),
            )
            .unwrap();
        let entity = u128::from_be_bytes(asset_uuid.try_into().unwrap());

        let mut winner = event(
            1,
            entity,
            Operation::SetField {
                field: "rating".into(),
                value: 5.into(),
            },
        );
        winner.hlc.wall_ms = 20;
        winner = crate::sync::codec::seal_event(winner).unwrap();
        database.apply_remote_sync_events(&[winner]).unwrap();

        let mut loser = event(
            1,
            entity,
            Operation::SetField {
                field: "rating".into(),
                value: 1.into(),
            },
        );
        loser.actor.device_uuid = DeviceUuid::new();
        loser.event_uuid = crate::sync::types::EventUuid::new();
        loser.hlc.wall_ms = 10;
        loser = crate::sync::codec::seal_event(loser).unwrap();
        database.apply_remote_sync_events(&[loser]).unwrap();

        let rating: i64 = database
            .connection
            .query_row(
                "SELECT rating FROM user_metadata WHERE file_id=?1",
                [file_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rating, 5);
    }

    #[tokio::test]
    async fn unknown_remote_schema_sets_compatibility_lock_without_advancing() {
        use crate::sync::test_model::event;
        use crate::sync::types::{EventUuid, Operation};
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        database
            .add_root(crate::db::NewRoot {
                path: "C:\\future".into(),
                display_name: "Future".into(),
            })
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        let mut future = event(
            1,
            10,
            Operation::Delete {
                token: EventUuid::new(),
            },
        );
        future.wire_version = 2;
        future = crate::sync::codec::seal_event(future).unwrap();
        assert!(database.apply_remote_sync_events(&[future]).is_err());
        let state: String = database
            .connection
            .query_row(
                "SELECT compatibility_state FROM sync_local_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "compatibility-lock");
        let count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM sync_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert!(database.create_tag("blocked", None).is_err());
        let tags: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM tags", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tags, 0);
    }

    #[test]
    fn unbind_archives_unpublished_suffix_and_never_reuses_actor_epoch() {
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        database
            .add_root(crate::db::NewRoot {
                path: "C:\\unbind".into(),
                display_name: "Unbind".into(),
            })
            .unwrap();
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .unwrap();
        let binding = uuid::Uuid::now_v7();
        let actor = uuid::Uuid::now_v7();
        database.begin_sync_capture(binding, actor).unwrap();
        database.create_tag("pending", None).unwrap();
        database.unbind_sync_capture(0).unwrap();
        let state: String = database
            .connection
            .query_row("SELECT state FROM sync_outbox", [], |row| row.get(0))
            .unwrap();
        assert_eq!(state, "archived");
        let epoch_state: String = database
            .connection
            .query_row(
                "SELECT state FROM sync_actor_epochs WHERE actor_epoch=?1",
                [actor.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(epoch_state, "closed");
        let active: Option<Vec<u8>> = database
            .connection
            .query_row(
                "SELECT active_actor_epoch FROM sync_local_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(active.is_none());
    }

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
            library_uuid: None,
            root_uuid: None,
            managed_root_action: None,
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
            "mangoo-notification-wake-{}-{}",
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
        let dir = std::env::temp_dir().join(format!("mangoo-writer-repro-{}", std::process::id()));
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
    async fn shutdown_joins_writer_and_notification_scheduler_threads() {
        let database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        let writer = DbWriter::spawn(database);

        writer.shutdown().await.expect("shutdown writer runtime");

        assert!(writer
            .inner
            .join_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
        assert!(writer
            .inner
            .scheduler_join_handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none());
        assert!(writer.write(|_| Ok(())).await.is_err());
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
                path: format!("D:/mangoo-wal-bound-{}", std::process::id()),
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
        let dir = std::env::temp_dir().join(format!("mangoo-wal-bound-{}", std::process::id()));
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

    #[tokio::test]
    async fn plugin_lifecycle_queued_writer_keeps_ownership_and_rejects_late_mutation() {
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        let writer = DbWriter::spawn(database);
        let providers = crate::sync::provider::ProviderManager::default();
        providers.open_plugin("provider", 1).unwrap();
        let work = providers.begin_work("provider").unwrap();
        let scoped = writer.with_write_admission(Arc::new(move || work.check_current()));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocking_writer = writer.clone();
        let blocker = tokio::spawn(async move {
            blocking_writer
                .write(move |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
        });
        entered_rx.await.unwrap();
        let mutated = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mutation = mutated.clone();
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        scoped
            .enqueue(
                move |_| {
                    mutation.store(true, Ordering::Release);
                    Ok(())
                },
                WriteDurability::Authoritative,
                false,
                reply,
            )
            .unwrap();
        drop(scoped);
        drop(reply_rx);
        providers.close_plugin("provider", 2);
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(20),
            providers.join_plugin_work("provider")
        )
        .await
        .is_err());
        release_tx.send(()).unwrap();
        blocker.await.unwrap().unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            providers.join_plugin_work("provider"),
        )
        .await
        .unwrap();
        assert!(!mutated.load(Ordering::Acquire));
        // Local edits through the ordinary handle remain available.
        writer.write(|_| Ok(())).await.unwrap();
        writer.shutdown().await.unwrap();
    }
}
