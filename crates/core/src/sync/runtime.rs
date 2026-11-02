use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::db::{Database, DbWriter};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RootExecutionToken {
    pub library_uuid: String,
    pub root_uuid: String,
    pub local_binding_uuid: String,
    pub binding_generation: u64,
}

#[derive(Clone, Default)]
pub struct RootRuntimeRegistry {
    inner: Arc<Mutex<HashMap<String, RootExecutionToken>>>,
}

impl RootRuntimeRegistry {
    pub fn replace_all(&self, tokens: impl IntoIterator<Item = RootExecutionToken>) {
        let mut current = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        current.clear();
        current.extend(
            tokens
                .into_iter()
                .map(|token| (token.root_uuid.clone(), token)),
        );
    }

    pub fn install(&self, token: RootExecutionToken) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(token.root_uuid.clone(), token);
    }

    pub fn invalidate(&self, root_uuid: &str) -> Option<RootExecutionToken> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(root_uuid)
    }

    pub fn require_current(&self, token: &RootExecutionToken) -> anyhow::Result<()> {
        let current = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if current.get(&token.root_uuid) != Some(token) {
            anyhow::bail!("stale-root-binding");
        }
        Ok(())
    }
}

/// Per-Library fence shared by active promotion, dormant turns and unbind.
/// Owning the permit is the proof that exactly one writer runtime may exist.
#[derive(Clone, Default)]
pub struct RuntimeLeases {
    inner: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
}

impl RuntimeLeases {
    pub async fn acquire(&self, library_uuid: &str) -> anyhow::Result<RuntimeLease> {
        let semaphore = {
            let mut leases = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            Arc::clone(
                leases
                    .entry(library_uuid.to_owned())
                    .or_insert_with(|| Arc::new(Semaphore::new(1))),
            )
        };
        let permit = semaphore
            .acquire_owned()
            .await
            .context("runtime lease closed")?;
        Ok(RuntimeLease {
            library_uuid: library_uuid.to_owned(),
            _permit: permit,
        })
    }

    pub fn try_acquire(&self, library_uuid: &str) -> anyhow::Result<RuntimeLease> {
        let semaphore = {
            let mut leases = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            Arc::clone(
                leases
                    .entry(library_uuid.to_owned())
                    .or_insert_with(|| Arc::new(Semaphore::new(1))),
            )
        };
        let permit = semaphore
            .try_acquire_owned()
            .context("Library runtime already owns the writer lease")?;
        Ok(RuntimeLease {
            library_uuid: library_uuid.to_owned(),
            _permit: permit,
        })
    }
}

pub struct RuntimeLease {
    pub library_uuid: String,
    _permit: OwnedSemaphorePermit,
}

/// Minimal inactive-Library runtime. It owns no watcher, thumbnail worker or
/// long-lived read connection: each scheduled turn opens one small SQLite
/// connection, starts the sole writer, performs bounded work, then drains and
/// joins it before releasing the shared active/dormant/unbind fence.
#[derive(Clone)]
pub struct DormantSyncRuntime {
    pub(crate) library_uuid: String,
    pub(crate) catalog_path: std::path::PathBuf,
    leases: RuntimeLeases,
}

impl DormantSyncRuntime {
    pub fn new(
        library_uuid: impl Into<String>,
        catalog_path: impl Into<std::path::PathBuf>,
        leases: RuntimeLeases,
    ) -> Self {
        Self {
            library_uuid: library_uuid.into(),
            catalog_path: catalog_path.into(),
            leases,
        }
    }

    pub async fn run_turn<T, F>(&self, turn: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static,
    {
        let _lease = self.leases.acquire(&self.library_uuid).await?;
        let database =
            Database::open_existing(&self.catalog_path).context("open dormant catalog")?;
        database
            .apply_migrations()
            .context("migrate dormant catalog")?;
        database
            .connection_for_fsops()
            .execute_batch("PRAGMA cache_size=-4096; PRAGMA mmap_size=16777216;")
            .context("configure dormant catalog resource limits")?;
        let writer = DbWriter::spawn(database);
        let result = writer.write_authoritative(turn).await;
        let shutdown = writer.shutdown().await;
        match (result, shutdown) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error).context("shutdown dormant catalog writer"),
        }
    }

    /// Active promotion and unbind use this exact fence. The returned lease
    /// must live for the entire promoted runtime or cutover transaction.
    pub async fn acquire_exclusive(&self) -> anyhow::Result<RuntimeLease> {
        self.leases.acquire(&self.library_uuid).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncWork {
    pub library_uuid: String,
    pub event_budget: usize,
    pub byte_budget: usize,
}

/// Fair fake coordinator used by the protocol foundation and later replaced by
/// a provider-backed scheduler. It queues at most one bounded turn per Library.
#[derive(Default)]
pub struct FakeCoordinator {
    queue: VecDeque<SyncWork>,
}

impl FakeCoordinator {
    pub fn schedule(&mut self, library_uuid: impl Into<String>) {
        let library_uuid = library_uuid.into();
        if self
            .queue
            .iter()
            .any(|work| work.library_uuid == library_uuid)
        {
            return;
        }
        self.queue.push_back(SyncWork {
            library_uuid,
            event_budget: super::inbox::MAX_APPLY_EVENTS,
            byte_budget: super::inbox::MAX_APPLY_DECODED_BYTES,
        });
    }

    pub fn next(&mut self) -> Option<SyncWork> {
        self.queue.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lease_fences_two_runtimes_for_the_same_library() {
        let leases = RuntimeLeases::default();
        let first = leases.acquire("library").await.unwrap();
        let blocked = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            leases.acquire("library"),
        )
        .await;
        assert!(blocked.is_err());
        drop(first);
        assert!(leases.acquire("library").await.is_ok());
    }

    #[tokio::test]
    async fn dormant_turn_opens_bounded_writer_then_releases_for_promotion() {
        let directory =
            std::env::temp_dir().join(format!("mangoo-dormant-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("catalog.sqlite");
        drop(Database::open(&path).unwrap());
        let leases = RuntimeLeases::default();
        let runtime = DormantSyncRuntime::new("library", &path, leases.clone());
        let (synchronous, cache_kib, mmap_bytes) = runtime
            .run_turn(|database| {
                let connection = database.connection_for_fsops();
                Ok((
                    database.write_durability_for_test()?,
                    connection
                        .pragma_query_value(None, "cache_size", |row| row.get::<_, i64>(0))?,
                    connection.pragma_query_value(None, "mmap_size", |row| row.get::<_, i64>(0))?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(synchronous, 2);
        assert_eq!(cache_kib, -4096);
        assert_eq!(mmap_bytes, 16 * 1024 * 1024);
        let _promotion = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            runtime.acquire_exclusive(),
        )
        .await
        .unwrap()
        .unwrap();
        drop(_promotion);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn dormant_turn_and_unbind_share_the_same_fence() {
        let directory =
            std::env::temp_dir().join(format!("mangoo-dormant-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&directory).unwrap();
        drop(Database::open(directory.join("catalog.sqlite")).unwrap());
        let runtime = DormantSyncRuntime::new(
            "library",
            directory.join("catalog.sqlite"),
            RuntimeLeases::default(),
        );
        let unbind = runtime.acquire_exclusive().await.unwrap();
        let blocked = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            runtime.run_turn(|_| Ok(())),
        )
        .await;
        assert!(blocked.is_err());
        drop(unbind);
        runtime.run_turn(|_| Ok(())).await.unwrap();
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn fake_coordinator_is_fair_and_deduplicates_turns() {
        let mut coordinator = FakeCoordinator::default();
        for index in 0..8 {
            coordinator.schedule(format!("library-{index}"));
        }
        coordinator.schedule("library-0");
        let libraries = std::iter::from_fn(|| coordinator.next())
            .map(|work| work.library_uuid)
            .collect::<Vec<_>>();
        assert_eq!(libraries.len(), 8);
        assert_eq!(libraries[0], "library-0");
        assert_eq!(libraries[7], "library-7");
    }

    #[test]
    fn root_runtime_registry_fences_only_the_changed_root() {
        let registry = RootRuntimeRegistry::default();
        let first = RootExecutionToken {
            library_uuid: "library".into(),
            root_uuid: "root-a".into(),
            local_binding_uuid: "binding-a".into(),
            binding_generation: 1,
        };
        let second = RootExecutionToken {
            library_uuid: "library".into(),
            root_uuid: "root-b".into(),
            local_binding_uuid: "binding-b".into(),
            binding_generation: 1,
        };
        registry.replace_all([first.clone(), second.clone()]);
        registry.install(RootExecutionToken {
            binding_generation: 2,
            ..first.clone()
        });
        assert!(registry.require_current(&first).is_err());
        assert!(registry.require_current(&second).is_ok());
    }
}
