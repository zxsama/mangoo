# Single-Writer SQLite Serialization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route every SQLite write in Core through one dedicated writer-actor thread that owns the single write connection, so the WAL writer lock is never contended (`database is locked` becomes impossible), the WAL stays bounded, and folder switching + thumbnail generation stay responsive under a 20-folder / 16k-task burst.

**Architecture:** A `DbWriter` handle submits boxed write closures over an MPSC channel to a writer-actor thread that owns the one write `Database`. The actor runs each job, drains queued jobs, then runs a WAL checkpoint between batches. Reads keep their separate per-thread WAL read connections unchanged. Blob decode stays parallel; only the tiny commit is serialized. `get_thumbnail` becomes read-only; generation is driven by the batched `priority-scope` write. The `write_gate` module and the separate checkpoint thread are deleted.

**Tech Stack:** Rust, Axum, rusqlite (SQLite WAL), Tokio (`spawn_blocking`, `mpsc`, `oneshot`), TypeScript/React web client.

## Global Constraints

- SQLite file connections keep WAL + `synchronous=NORMAL` + 64 MiB cache + 256 MiB mmap + `temp_store=MEMORY` (`configure_file_database_wal`); do not change these.
- `WAL_TRUNCATE_THRESHOLD_BYTES = 256 * 1024 * 1024` — checkpoint escalates to TRUNCATE above this, else PASSIVE.
- Reads must never go through the writer actor when a read connection is available (`run_read_database` / `read_with_cached_connection` stay for reads).
- The HTTP contract in `contracts/core-api/openapi.yaml` is unchanged: `GET /media/{fileId}/thumbnail` still returns `200` (ready) or `202` (pending) with the same `ThumbnailResponse` body.
- `cargo fmt --all --check` and `npm test` (8 structural gates + `cargo test --workspace`) must pass at each commit.
- In-memory/test databases have no `read_database_path`; reads fall back to the writer connection (existing behavior — preserve it).
- User priority: user-imperceptible, fastest UI load. Do not add UI-visible latency; the write path change must not slow reads or block the render path.

---

## File Structure

- `crates/core/src/db/writer.rs` (**new**) — the `DbWriter` handle + writer-actor thread. Owns the single write connection; runs jobs + checkpoints.
- `crates/core/src/db/mod.rs` (**modify**) — `mod writer;` re-export; move checkpoint policy helper (`run_wal_checkpoint_pass`) to be callable by the actor; keep `checkpoint_wal_if_larger_than` / `wal_size_bytes`.
- `crates/core/src/api/mod.rs` (**modify**) — `AppState.database: Arc<Mutex<Database>>` → `AppState.writer: DbWriter`; constructors build the writer and pass its handle to worker/watcher.
- `crates/core/src/api/routes.rs` (**modify**) — migrate all write sites to `state.writer.write(...)`; delete `run_shared_database` / `run_dedicated_or_shared_database`; make `get_thumbnail` read-only.
- `crates/core/src/tasks.rs` (**modify**) — worker submits writes through `DbWriter`; remove per-task `reopen()` write connections + the dedicated checkpoint thread.
- `crates/core/src/scan/mod.rs` (**modify**) — scan batch commits go through `DbWriter`.
- `crates/core/src/watch/mod.rs` (**modify**) — watcher upserts go through `DbWriter`.
- `crates/core/src/write_gate.rs` (**delete**) — no longer needed with a single writer.
- `crates/core/src/main.rs` (**modify**) — build `DbWriter`, thread into `AppState`.
- `tools/checks/validate-core-api.mjs` (**modify**) — update route markers from `run_shared_database` to the writer; drop write_gate markers.
- `apps/web/src/core/useLibraryData.ts` (**modify**) — stop the redundant per-tile generation write (optimization).

---

## Task 1: Writer actor core (`DbWriter` + actor thread)

**Files:**
- Create: `crates/core/src/db/writer.rs`
- Modify: `crates/core/src/db/mod.rs` (add `pub mod writer;` and re-export `DbWriter`)
- Test: inline `#[cfg(test)]` in `crates/core/src/db/writer.rs`

**Interfaces:**
- Consumes: `crate::db::Database` (existing; owns a rusqlite `Connection`), `Database::open_in_memory()`, `Database::wal_size_bytes()` (private today — make `pub(crate)` in this task), `Database::checkpoint_wal_if_larger_than(threshold) -> anyhow::Result<bool>` (private today — make `pub(crate)`), the const `WAL_TRUNCATE_THRESHOLD_BYTES` (make `pub(crate)`).
- Produces:
  - `pub struct DbWriter { sender: tokio::sync::mpsc::UnboundedSender<WriteJob> }` (derive `Clone`).
  - `pub fn DbWriter::spawn(database: Database) -> DbWriter` — starts the actor thread, returns the handle.
  - `pub async fn DbWriter::write<T, F>(&self, op: F) -> anyhow::Result<T> where T: Send + 'static, F: FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static`.

- [ ] **Step 1: Make the checkpoint helpers reachable from the actor**

In `crates/core/src/db/mod.rs`, change the visibility of these three items from private to `pub(crate)` (they exist today as `const WAL_TRUNCATE_THRESHOLD_BYTES`, `fn wal_size_bytes`, `fn checkpoint_wal_if_larger_than`):

```rust
pub(crate) const WAL_TRUNCATE_THRESHOLD_BYTES: u64 = 256 * 1024 * 1024;
```
```rust
    pub(crate) fn wal_size_bytes(&self) -> anyhow::Result<u64> {
```
```rust
    pub(crate) fn checkpoint_wal_if_larger_than(&self, threshold_bytes: u64) -> anyhow::Result<bool> {
```

Add a PASSIVE-checkpoint helper on `Database` (used by the actor between batches) right after `checkpoint_wal_if_larger_than`:

```rust
    /// Run one WAL checkpoint pass: TRUNCATE if the WAL has outgrown the
    /// threshold (reclaims the file), else a cheap PASSIVE pass. Called by the
    /// writer actor between job batches — it is the ONLY writer, so a TRUNCATE
    /// here always gets its exclusive window.
    pub(crate) fn run_wal_checkpoint_pass(&self) -> anyhow::Result<()> {
        if self.path.is_none() {
            return Ok(());
        }
        if self.checkpoint_wal_if_larger_than(WAL_TRUNCATE_THRESHOLD_BYTES)? {
            return Ok(());
        }
        self.connection
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| row.get::<_, i64>(0))?;
        Ok(())
    }
```

- [ ] **Step 2: Add the module declaration**

In `crates/core/src/db/mod.rs`, near the top with the other `mod`/`use` lines, add:

```rust
pub mod writer;
pub use writer::DbWriter;
```

- [ ] **Step 3: Write the failing test for the writer actor**

Create `crates/core/src/db/writer.rs` with only the test module first:

```rust
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
```

- [ ] **Step 4: Run the test to verify it fails to compile**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p mangoo-core db::writer 2>&1 | tail -20`
Expected: compile error — `DbWriter` / `spawn` / `write` not found.

- [ ] **Step 5: Implement the writer actor**

At the top of `crates/core/src/db/writer.rs`, above the test module:

```rust
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
/// actor thread. Every SQLite write in Core goes through here so the WAL writer
/// lock is never contended.
#[derive(Clone)]
pub struct DbWriter {
    sender: mpsc::UnboundedSender<QueuedJob>,
}

impl DbWriter {
    /// Start the actor thread over the given write connection and return a handle.
    pub fn spawn(database: Database) -> DbWriter {
        let (sender, receiver) = mpsc::unbounded_channel::<QueuedJob>();
        std::thread::Builder::new()
            .name("mangoo-db-writer".to_string())
            .spawn(move || run_writer_actor(database, receiver))
            .expect("spawn db writer thread");
        DbWriter { sender }
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
        let job: WriteJob = Box::new(move |database| {
            op(database).map(|value| Box::new(value) as Box<dyn Any + Send>)
        });
        self.sender
            .send(QueuedJob { job, reply })
            .map_err(|_| anyhow::anyhow!("database writer is unavailable"))?;
        let boxed = reply_rx
            .await
            .map_err(|_| anyhow::anyhow!("database writer dropped the job"))??;
        Ok(*boxed
            .downcast::<T>()
            .map_err(|_| anyhow::anyhow!("database writer returned an unexpected type"))?)
    }
}

fn run_writer_actor(mut database: Database, mut receiver: mpsc::UnboundedReceiver<QueuedJob>) {
    // Block for the next job; run it and any already-queued jobs; then, when the
    // queue drains, run one checkpoint pass before blocking again. Batching keeps
    // the checkpoint off the per-write hot path while guaranteeing it runs.
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
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p mangoo-core db::writer 2>&1 | tail -20`
Expected: `test result: ok. 3 passed`

- [ ] **Step 7: Format and commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt -p mangoo-core
git add crates/core/src/db/writer.rs crates/core/src/db/mod.rs
git commit -m "feat(core): single-writer DbWriter actor over the SQLite write connection"
```

---

## Task 2: Wire `DbWriter` into `AppState` and startup

**Files:**
- Modify: `crates/core/src/api/mod.rs` (AppState struct + `new` + `new_with_worker` + a `writer()` accessor)
- Modify: `crates/core/src/main.rs` (no signature change expected; `AppState::new_with_worker` still receives the two `Database`s)
- Test: existing `cargo test -p mangoo-core` (compile + route tests exercise this)

**Interfaces:**
- Consumes: `DbWriter::spawn(Database) -> DbWriter` from Task 1.
- Produces: `AppState.writer: DbWriter` (field, public); `start_worker` / `start_watcher` now take `DbWriter` (Task 4/5 change their signatures — this task threads the handle to them, so land Task 2 together with the call-site updates below).

- [ ] **Step 1: Replace the shared mutex field with the writer handle**

In `crates/core/src/api/mod.rs`, change the struct field:

```rust
pub struct AppState {
    pub writer: crate::db::DbWriter,
    pub task_queue: TaskSender,
    read_database_path: Option<PathBuf>,
    pub plugins_dir: Option<PathBuf>,
    _watcher: Option<Arc<WatcherHandle>>,
}
```

Remove the now-unused `use std::sync::{Arc, Mutex}` for `Mutex` if `Arc` is still needed elsewhere (keep `Arc` — `_watcher` uses it).

- [ ] **Step 2: Build the writer in the constructors**

Rewrite `AppState::new` and `AppState::new_with_worker` in `crates/core/src/api/mod.rs`:

```rust
    pub fn new(database: Database) -> Self {
        if let Some(worker_database) = database.reopen().expect("reopen worker database") {
            return Self::new_with_worker(database, worker_database);
        }
        // In-memory/test DB: no reopen; the writer owns the sole connection and
        // reads fall back to it (no read_database_path).
        let (task_queue, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        Self {
            writer: crate::db::DbWriter::spawn(database),
            task_queue,
            read_database_path: None,
            plugins_dir: None,
            _watcher: None,
        }
    }

    pub fn new_with_worker(database: Database, worker_database: Database) -> Self {
        let read_database_path = database.path().map(Path::to_path_buf);
        let watcher_database = database.reopen().expect("reopen watcher database");
        // The primary connection becomes the single writer. The worker + watcher
        // no longer hold their own WRITE connections; they submit writes through
        // this handle. `worker_database` / `watcher_database` remain as READ
        // connections for scheduling/lookup only.
        let writer = crate::db::DbWriter::spawn(database);
        let task_queue = start_worker(worker_database, writer.clone());
        let watcher = watcher_database.map(|watcher_database| {
            Arc::new(start_watcher(watcher_database, task_queue.clone(), writer.clone()))
        });
        Self {
            writer,
            task_queue,
            read_database_path,
            plugins_dir: None,
            _watcher: watcher,
        }
    }
```

(Note: `start_worker` and `start_watcher` gain a `DbWriter` parameter — implemented in Tasks 4 and 5. This task will not compile until those land; that is expected for a fresh subagent — implement Task 2 and the call sites in 4/5 as one reviewable unit if executing inline. When using subagents, dispatch Task 2 with a note that it compiles only after 4 and 5.)

- [ ] **Step 3: Add a convenience accessor (optional, keeps route code short)**

Not required; route code uses `state.writer.write(...)` directly. Skip.

- [ ] **Step 4: Defer compile check to Task 5**

The workspace will not compile until Tasks 3–5 land. Commit this task together with 3–5 as noted, OR (subagent mode) mark Task 2 DONE_WITH_CONCERNS and let the controller sequence 3–5 before the shared compile/test gate.

- [ ] **Step 5: Commit (with Tasks 3–5)**

```bash
git add crates/core/src/api/mod.rs
git commit -m "refactor(core): AppState holds a DbWriter instead of Arc<Mutex<Database>>"
```

---

## Task 3: Migrate route writes to `DbWriter`; make `get_thumbnail` read-only

**Files:**
- Modify: `crates/core/src/api/routes.rs`
- Test: existing route tests in `crates/core/src/api/routes.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `AppState.writer: DbWriter` (Task 2); `DbWriter::write` (Task 1); existing `run_read_database` / `read_with_cached_connection` (unchanged).
- Produces: no new public interface; all write handlers now use `state.writer.write(...)`.

- [ ] **Step 1: Replace the write helpers with writer calls**

Delete `run_shared_database` and `run_dedicated_or_shared_database` (around `crates/core/src/api/routes.rs:563` and `:576`). For every call site of those two helpers AND every direct `state.database.lock()` that performs a WRITE, replace with the writer. The mechanical transform (the closure body is identical):

Before:
```rust
    let record = run_shared_database(state.clone(), move |database| {
        database.upsert_user_metadata_partial(file_id, patch).map_err(map_metadata_error)
    })
    .await?;
```
After:
```rust
    let record = state
        .writer
        .write(move |database| database.upsert_user_metadata_partial(file_id, patch).map_err(map_metadata_error))
        .await?;
```

For direct-lock writes like:
```rust
    let database = state.database.lock().expect("database mutex poisoned");
    let record = database.create_tag(...)?;
```
Convert to:
```rust
    let record = state
        .writer
        .write(move |database| Ok(database.create_tag(...)?))
        .await?;
```

Apply to every write handler: `add_root`, `remove_root`, `enqueue_scan`, `sync_thumbnail_priority_scope`, `enqueue_thumbnail_cache`, `clear_thumbnail_cache`, `cancel_task`, `retry_task`, `create_tag`, `delete_tag`, `update_user_metadata`, `set_file_tags`, `add_file_tag`, `remove_file_tag`, `discover_plugins`, `enable_plugin`, `disable_plugin`, `delete_plugin`, `rename_file_op`, `move_file_ops`, `delete_file_ops`, the foreground-promotion `touch_pending_foreground_thumbnail_task` path in `get_thumbnail`, and the enqueue-failure marking in `enqueue_task`.
Leave READ handlers (`list_roots`, `list_folder_children`, `list_media`, `get_media`, `get_thumbnail_blob`, `get_preview`, `list_tasks`, `search_media`, `get_user_metadata`, `list_tags`, `list_plugins`, `get_plugin`, `get_thumbnail_cache_stats`, `list_file_operations`) on `run_read_database`.

- [ ] **Step 2: Make `get_thumbnail` read-only**

Replace the whole body of `async fn get_thumbnail` (`crates/core/src/api/routes.rs`, currently ~lines 724–843) so it only READS state and never writes/enqueues (generation is driven by the batched `priority-scope` sync):

```rust
async fn get_thumbnail(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Query(query): Query<ThumbnailQuery>,
) -> ApiResult<(StatusCode, Json<ThumbnailResponse>)> {
    let target = normalize_profile(query.target.as_deref())
        .ok_or_else(|| CoreError::bad_request("unsupported thumbnail target".to_string()))?;
    // priority is accepted for API compatibility but no longer drives an on-demand
    // enqueue here; the batched /tasks/thumbnail-priority-scope write enqueues and
    // prioritizes generation. This keeps get_thumbnail a pure read (no WAL write),
    // so it never contends and never returns "database is locked".
    let _ = ThumbnailTaskPriority::from_query_value(query.priority.as_deref())
        .ok_or_else(|| CoreError::bad_request("unsupported thumbnail priority".to_string()))?;
    let (thumbnail, blob) = run_read_database(state, move |database| {
        let thumbnail = database
            .get_thumbnail(file_id, target)?
            .ok_or_else(|| CoreError::not_found(format!("media item not found: {file_id}")))?;
        let blob = if thumbnail.state.as_str() == "ready" {
            database.get_thumb_blob(file_id, target)?
        } else {
            None
        };
        Ok((thumbnail, blob))
    })
    .await?;
    let status = if is_pending_status(&thumbnail.state) {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(thumbnail_response(thumbnail, blob))))
}
```

Remove the now-unused helpers `is_database_locked_error` (added earlier) only if nothing else references it; if the enqueue paths elsewhere still map lock errors, keep it. Remove the `write_gate` `begin_interactive_write` call from this handler.

- [ ] **Step 3: Run route tests to verify the contract holds**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p mangoo-core api::routes 2>&1 | tail -25`
Expected: the thumbnail route tests that asserted `state == "queued"` after `GET /thumbnail` now need updating (they asserted the OLD enqueue-on-GET behavior). Update those specific tests:
- `thumbnail_route_queues_missing_grid_320_row`, `thumbnail_route_queues_missing_thumbnail_once_on_repeated_requests`, `thumbnail_route_reenqueues_promoted_pending_thumbnail_task`, `thumbnail_route_wakes_existing_pending_thumbnail_task` assert that `GET /thumbnail` enqueues. Since generation now flows through `/tasks/thumbnail-priority-scope`, change these tests to POST `/api/tasks/thumbnail-priority-scope` (with the file id in `visibleFileIds`) to trigger the enqueue, then assert the queued task exactly as before. Keep the pure-read assertion for `GET /thumbnail`: it returns `202` + `state` pending/queued reflecting the row, and does NOT itself enqueue.

- [ ] **Step 4: Run the updated tests**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p mangoo-core api::routes 2>&1 | tail -25`
Expected: `test result: ok` (all route tests pass, including the migrated enqueue-via-priority-scope tests).

- [ ] **Step 5: Commit (with Tasks 2/4/5)**

```bash
git add crates/core/src/api/routes.rs
git commit -m "refactor(core): route writes through DbWriter; get_thumbnail is read-only"
```

---

## Task 4: Worker submits writes through `DbWriter`; drop per-task write connections + checkpoint thread

**Files:**
- Modify: `crates/core/src/tasks.rs`
- Test: existing `cargo test -p mangoo-core` worker/thumbnail tests

**Interfaces:**
- Consumes: `DbWriter` (Task 1); the worker's own `Database` stays as a READ/scheduling connection.
- Produces: `pub fn start_worker(worker_database: Database, writer: DbWriter) -> TaskSender` and `pub fn start_worker_with_ffmpeg(worker_database: Database, writer: DbWriter, ffmpeg_available: bool) -> TaskSender` — the added `writer` parameter is what Task 2 passes.

- [ ] **Step 1: Add the `writer` parameter and drop the checkpoint thread**

In `crates/core/src/tasks.rs`, change `start_worker` / `start_worker_with_ffmpeg` signatures to take `writer: DbWriter`. Delete `fn start_wal_checkpoint_thread` and its call (the actor checkpoints now). Startup-recovery writes (`reset_running_root_scan_tasks_for_recovery`, `fail_pending_root_scan_tasks_for_disabled_roots`, `reset_running_thumbnail_tasks_for_recovery`) run once at startup on `worker_database` directly — that is safe (the actor is not yet processing and there is no concurrency at startup), so leave them on `worker_database`. Store `writer` in the worker task's async move block.

- [ ] **Step 2: Route the blob commit + task-state writes through the writer**

The per-task path currently does `let Ok(Some(mut thumbnail_database)) = database.reopen()` then `run_task(&mut thumbnail_database, ...)` on `spawn_blocking`. Change so decode stays on `spawn_blocking` but the final publish is a writer job. Concretely, in the thumbnail dispatch (`dispatch_...` → `run_task` → `run_thumbnail_task_with_cache...`): keep the decode/resize producing `generated` bytes on the blocking thread, then submit the publish as:

```rust
    let published = writer
        .write(move |database| {
            Ok(database
                .publish_thumbnail_and_succeed_task_if_source_fingerprint_and_task_attempt_current(
                    upsert, blob, source_dimensions, &source_fingerprint, task_id, attempt_generation,
                )?)
        })
        .await?;
```

Because `run_task` is currently sync-over-a-connection, wrap the thumbnail dispatch so the decode runs in `spawn_blocking` and the publish awaits the writer (the dispatch is already inside a Tokio task). Remove `thumbnail_database` `reopen()` for the write; a read connection (from `worker_database.reopen()` once, reused) supplies `get_thumbnail_source` and fingerprint reads. Task-state transitions used during scheduling (`mark_task_*`, `touch_*`) that WRITE also go through `writer.write(...)`; reads (`active_thumbnail_task`, `list`/scheduling queries) stay on the worker's read connection.

Remove the `write_gate` yield calls inside `run_thumbnail_task_with_cache...` publish path (deleted in Task 6).

- [ ] **Step 3: Run worker + thumbnail tests**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p mangoo-core tasks 2>&1 | tail -25 && cargo test -p mangoo-core thumbnail 2>&1 | tail -25`
Expected: `test result: ok` for both (worker recovery, thumbnail publish, priority yield tests updated to the writer path where they asserted the old gate).

- [ ] **Step 4: Commit (with Tasks 2/3/5)**

```bash
git add crates/core/src/tasks.rs
git commit -m "refactor(core): thumbnail worker commits through DbWriter; remove per-task write conns + checkpoint thread"
```

---

## Task 5: Scan + watcher writes through `DbWriter`; delete `write_gate`; final compile/test gate

**Files:**
- Modify: `crates/core/src/scan/mod.rs`, `crates/core/src/watch/mod.rs`
- Delete: `crates/core/src/write_gate.rs` and its `mod write_gate;` in `crates/core/src/main.rs`
- Test: full `cargo test -p mangoo-core`

**Interfaces:**
- Consumes: `DbWriter` (Task 1).
- Produces: `pub fn start_watcher(watcher_database: Database, task_queue: TaskSender, writer: DbWriter) -> WatcherHandle`; scan commit functions take `&DbWriter`.

- [ ] **Step 1: Route scan batch commits through the writer**

In `crates/core/src/scan/mod.rs`, `commit_scan_batch_with_optional_task_guard` currently calls `database.commit_scan_batch_for_task_attempt(...)` / `database.commit_scan_batch(...)` on a `&mut Database`. Thread a `&DbWriter` into the scan (the scan runs on the worker; pass the worker's `writer`). Replace each commit with `writer.write(move |db| Ok(db.commit_scan_batch_for_task_attempt(batch, guard.task_id, guard.attempt_generation)?)).await`. The scan traversal (WalkDir, reads) stays on the scan's own connection; only the batch COMMIT goes through the writer.

Remove the `write_gate::interactive_write_gate().yield_to_pending_interactive_writes()` and `wait_for_pending_checkpoint()` calls added to the commit path (they live in `db/mod.rs` `commit_scan_batch*` — delete those two lines from both commit variants).

- [ ] **Step 2: Route watcher upserts through the writer**

In `crates/core/src/watch/mod.rs`, `start_watcher` gains a `writer: DbWriter` param stored in `WatcherState`. The upsert/remove/queue-rescan operations that WRITE (`upsert_media_file`, `queue_root_rescan`) submit through `writer.write(...)`; `list_roots` / `get_root` reads stay on the watcher's connection. Since the watcher loop is sync (`std::thread`), use `writer`'s blocking bridge: add a `pub fn write_blocking<T, F>(&self, op: F) -> anyhow::Result<T>` to `DbWriter` that sends the job and blocks on the oneshot with `blocking_recv` (for non-async callers like the watcher thread). Add it in `db/writer.rs`:

```rust
    /// Blocking variant for non-async callers (the watcher runs on a plain
    /// std::thread). Same guarantees as `write`.
    pub fn write_blocking<T, F>(&self, op: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static,
    {
        let (reply, reply_rx) = oneshot::channel();
        let job: WriteJob = Box::new(move |database| {
            op(database).map(|value| Box::new(value) as Box<dyn Any + Send>)
        });
        self.sender
            .send(QueuedJob { job, reply })
            .map_err(|_| anyhow::anyhow!("database writer is unavailable"))?;
        let boxed = reply_rx
            .blocking_recv()
            .map_err(|_| anyhow::anyhow!("database writer dropped the job"))??;
        Ok(*boxed
            .downcast::<T>()
            .map_err(|_| anyhow::anyhow!("database writer returned an unexpected type"))?)
    }
```

- [ ] **Step 3: Delete the write_gate module**

```bash
git rm crates/core/src/write_gate.rs
```
In `crates/core/src/main.rs`, remove the `mod write_gate;` line. Remove any remaining `crate::write_gate::` references across the crate (grep first: `grep -rn "write_gate" crates/core/src`). The checkpoint that write_gate coordinated is now the actor's `run_wal_checkpoint_pass`.

- [ ] **Step 4: Full compile + test gate**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo fmt -p mangoo-core && cargo test -p mangoo-core 2>&1 | tail -15`
Expected: `test result: ok. N passed; 0 failed` (all Core tests).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/scan/mod.rs crates/core/src/watch/mod.rs crates/core/src/main.rs crates/core/src/db/writer.rs
git commit -m "refactor(core): scan + watcher writes through DbWriter; delete write_gate"
```

---

## Task 6: Update structural checks + client per-tile write; full `npm test`

**Files:**
- Modify: `tools/checks/validate-core-api.mjs`
- Modify: `apps/web/src/core/useLibraryData.ts`
- Test: `npm test`

**Interfaces:**
- Consumes: nothing new.
- Produces: passing static gates.

- [ ] **Step 1: Update the core-api route markers**

In `tools/checks/validate-core-api.mjs`, the loops that assert `run_shared_database` / `run_dedicated_or_shared_database` on `get_thumbnail`, `enqueue_interactive_folder_scan`, `sync_thumbnail_priority_scope`, `enqueue_thumbnail_cache`, `clear_thumbnail_cache` (added in earlier sessions) must change: those routes now use `state.writer.write`. Replace the assertion so each of those write routes' `functionBody(name)` includes `state.writer.write` (or for `get_thumbnail`, that it uses `run_read_database` and does NOT contain `.write(`). Also keep the general assertion that `spawn_blocking` + `run_read_database` exist. Remove any `write_gate` markers.

Concretely, replace the two `for (const name of [...])` write-offload loops with:
```js
for (const name of ["sync_thumbnail_priority_scope", "enqueue_thumbnail_cache", "clear_thumbnail_cache", "enqueue_interactive_folder_scan"]) {
  if (!functionBody(name).includes("writer.write")) {
    fail(`${name} route must submit its write through the single DbWriter (state.writer.write)`);
  }
}
if (functionBody("get_thumbnail").includes("writer.write")) {
  fail("get_thumbnail must be read-only (no writer.write) — generation is driven by the priority-scope sync");
}
```

- [ ] **Step 2: Stop the redundant per-tile generation write in the client**

In `apps/web/src/core/useLibraryData.ts`, the per-tile `requestThumbnailState` path calls `client.getThumbnail(...)` which was relied on to enqueue. Since the server no longer enqueues on `GET /thumbnail`, ensure the viewport `syncThumbnailPriorityScope` still fires for visible/ahead/selected sets (it does — verify the effect at ~line 596 runs on folder change/scroll). Keep `getThumbnail`/`requestThumbnailState` as the state read for rendering. No behavioral removal is required for correctness (the server change is authoritative); only confirm priority-scope covers the visible set so generation is still triggered. Add a code comment at the priority-scope sync noting it is now the sole generation trigger.

- [ ] **Step 3: Run the full gate**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && npm test 2>&1 | tail -14`
Expected: all 8 `PASS:` gates + `test result: ok` for cargo.

- [ ] **Step 4: Commit**

```bash
git add tools/checks/validate-core-api.mjs apps/web/src/core/useLibraryData.ts
git commit -m "chore(core): checks assert DbWriter route usage; client comments on priority-scope as sole generation trigger"
```

---

## Task 7: Live acceptance — 20-folder switch + regression suite

**Files:**
- No source changes (verification task). Uses `tools/dev` CDP harness.

**Interfaces:**
- Consumes: the running app (`MANGOO_DB_PATH=.data/mangoo-dev.sqlite MANGOO_REMOTE_DEBUG=1 npm run dev`).

- [ ] **Step 1: Rebuild + relaunch**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
powershell.exe -NoProfile -Command "Get-Process electron,mangoo-core -ErrorAction SilentlyContinue | Stop-Process -Force"
sleep 3 && cargo build -p mangoo-core
MANGOO_DB_PATH="D:/Mangoo/.data/mangoo-dev.sqlite" MANGOO_REMOTE_DEBUG=1 nohup npm run dev > /tmp/mangoo-dev.log 2>&1 &
```
Wait for CDP page on 9222.

- [ ] **Step 2: Run the 20-top-level-folder switch test**

Drive the main window over CDP: switch 20 distinct depth-1 folders, measuring per-switch load time, 500 count, WAL size, and Core thread count (reuse the `_test20.mjs` pattern from the debugging session).
Expected (acceptance gate): **0 requests return 500; no folder "never" loads; WAL stays bounded (< ~300MB, reclaimed between bursts); Core thread count stable.**

- [ ] **Step 3: Confirm generation drains**

Query task states: `sqlite3` the DB (external, read-only) for `SELECT status, COUNT(*) FROM tasks WHERE kind='thumbnail' GROUP BY status` twice ~10s apart; `succeeded` must be climbing and no growing `failed` with "database is locked".

- [ ] **Step 4: Run the durable regression suite**

Run: `node tools/dev/regression-suite.mjs` with the settings window open (stats polling).
Expected: `10/10 checks passed`.

- [ ] **Step 5: Final full gate + commit any check fixups**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && npm test 2>&1 | tail -14
```
Expected: all gates green. Commit any final adjustments; report honestly with the measured 20-folder numbers.

---

## Self-Review

**Spec coverage:** Writer actor (Task 1) ✓; AppState wiring (Task 2) ✓; route migration + read-only get_thumbnail (Task 3) ✓; blob commit serialization + drop per-task conns/checkpoint thread (Task 4) ✓; scan + watcher + delete write_gate (Task 5) ✓; checks + client (Task 6) ✓; live acceptance incl. WAL bound + generation drain + regression suite (Task 7) ✓. Batched-enqueue-drives-generation is realized by making get_thumbnail read-only (Task 3) while priority-scope stays a writer write (Task 3 migration). Error handling (job error doesn't poison actor) covered by Task 1 tests. Soft-pending defense-in-depth: get_thumbnail no longer writes, so it can't hit the lock; the mapping is retained where enqueue writes remain (priority-scope) via `writer.write` returning the error normally.

**Placeholder scan:** No TBD/TODO. Task 4 Step 2 references exact method `publish_thumbnail_and_succeed_task_if_source_fingerprint_and_task_attempt_current` (exists today). Task 5 adds `write_blocking` with full code.

**Type consistency:** `DbWriter::spawn(Database) -> DbWriter`, `write<T,F>(&self, F) -> anyhow::Result<T>`, `write_blocking<T,F>` same shape; `start_worker(Database, DbWriter)`, `start_watcher(Database, TaskSender, DbWriter)` — used consistently in Tasks 2/4/5. `AppState.writer: DbWriter` field name consistent across Tasks 2/3.
