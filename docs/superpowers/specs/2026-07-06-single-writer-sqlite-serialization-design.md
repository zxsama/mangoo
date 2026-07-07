# Single-Writer Serialization for SQLite Under Full-Library Thumbnail Generation

**Date:** 2026-07-06
**Status:** Approved, ready for implementation plan

## Problem

Switching through many top-level folders (measured: 20 folders) enqueues a large
burst of thumbnail tasks (~16k pending observed). Under that load the app
degrades badly:

- **"Core API request failed with status 500"** in the UI — traced to
  `GET /media/{id}/thumbnail` (and other write routes) failing with SQLite
  **"database is locked"** (`SQLITE_BUSY` past the 5s `busy_timeout`).
- **Folder switches load slowly or never** (measured 15/20 slow, up to 6s;
  `media`/`folders` reads themselves reaching ~6s).
- **Thumbnail generation stalls**: 0 tasks running while 16k pending, ~756 tasks
  failed with "database is locked".
- **WAL grows unbounded** (measured 750MB–1.5GB) and the periodic checkpoint
  returns `busy=1` — it can never reclaim.

### Root cause (confirmed by live debugging)

SQLite has exactly one WAL writer lock. Today the write path uses **many writer
connections concurrently**:

- The API shared mutex connection (`AppState.database: Arc<Mutex<Database>>`).
- Up to 12 blob-commit worker connections (10 foreground + 2 background), each
  `reopen()`-ing its own connection per task on `spawn_blocking`.
- The scan worker connection.
- A dedicated WAL-checkpoint thread connection.

Under the burst, ~14 writer connections plus the checkpoint contend for the
single writer lock. The checkpoint gets stuck holding the checkpoint lock while
blocked on a reader's WAL snapshot; writers time out (`database is locked`);
failed transactions and continued attempts keep the WAL growing; the checkpoint
can never win because writes never pause. It is a positive-feedback death
spiral: contention → locked writes → stalled generation + growing WAL → slower
everything → more contention.

Prior incremental fixes (write-gate yields, dedicated connections, checkpoint
barriers, TTL-cached stats) reduced but never eliminated this, because they all
leave **multiple writer connections** in play.

## Goal

Make SQLite writes non-contended by construction so `database is locked` cannot
occur, the WAL stays bounded, and folder switching / thumbnail generation stay
responsive under the 20-folder burst. Success criteria (verified live against the
652k-file library):

- Switching 20 top-level folders: **0 requests return 500**, no folder "never"
  loads, per-switch load stays sub-second in the common case.
- WAL stays bounded (hundreds of MB at most, reclaimed between bursts), never
  grows to GB.
- Thumbnail generation keeps draining (tasks keep succeeding; not stuck at 0
  running with a large pending backlog).

## Architecture

Introduce a single **writer actor**: one dedicated thread that owns the sole
write connection. All writes in the system are submitted to it as jobs and
awaited. Because exactly one connection ever writes, the WAL writer lock is
never contended (`database is locked` becomes impossible), and the actor runs
the WAL checkpoint between jobs so the checkpoint always gets an exclusive
window and keeps the WAL bounded.

Reads are unchanged: they keep using the separate per-blocking-thread WAL read
connections (`READ_DATABASE` thread-local via `run_read_database`), which never
block on the writer and stay fast.

```
Reads  ──► per-thread WAL read connections (unchanged, always fast)

Writes ──► DbWriter handle ──MPSC(job)──► Writer actor thread
              (await oneshot reply)          owns THE ONE write connection
                                             runs each job, then between
                                             job batches runs WAL checkpoint
```

## Components

### 1. Writer actor — `crates/core/src/db/writer.rs` (new)

- Owns one `Database` (write) connection on a dedicated `std::thread`.
- Job = a boxed `FnOnce(&mut Database) -> anyhow::Result<Box<dyn Any + Send>>`
  paired with a `tokio::sync::oneshot::Sender` for the typed reply. (Internally
  boxed for type erasure; the public API is generic and downcasts.)
- Public handle `DbWriter` (cheaply clonable; holds the MPSC `Sender`):
  - `async fn write<T: Send + 'static>(&self, op: impl FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T>`
    — submits the job, awaits the oneshot, returns the value or the error.
  - A job that returns `Err` replies with that error; it never poisons the
    connection or kills the actor (the next job runs normally).
- Actor loop: receive one job (block), then **drain** any immediately-available
  queued jobs (`try_recv`) running each in turn; when the queue is momentarily
  empty, run one checkpoint pass, then block for the next job. This batches
  writes and interleaves checkpoints without a separate thread.
- Checkpoint policy (moved here from the deleted checkpoint thread): if WAL size
  > `WAL_TRUNCATE_THRESHOLD_BYTES` run `wal_checkpoint(TRUNCATE)`, else
  `wal_checkpoint(PASSIVE)`. Since the actor is the only writer, TRUNCATE
  reliably resets the WAL (only reader snapshots can still hold frames; that is
  acceptable and self-heals as reads roll over).
- Shutdown: dropping the last `DbWriter` closes the channel; the actor finishes
  in-flight jobs and exits. A test hook allows constructing an actor over an
  in-memory/temp DB.

### 2. AppState + wiring — `crates/core/src/api/mod.rs`, `main.rs`

- Replace `pub database: Arc<Mutex<Database>>` with `pub writer: DbWriter` for
  the write path. Read routes continue to open their own read connections from
  `read_database_path` (unchanged).
- `main.rs` opens the DB, applies migrations, constructs the `DbWriter` from the
  write connection, and passes the handle into `AppState`, the task worker, the
  watcher, and the scan worker.
- In-memory/test databases: the writer actor works over the single in-memory
  connection just the same (no `read_database_path`, so reads fall back to the
  writer — acceptable for tests, matching today's fallback).

### 3. Write route migration — `crates/core/src/api/routes.rs`

- Every `run_shared_database`, `run_dedicated_or_shared_database`, and direct
  `state.database.lock()` **write** becomes `state.writer.write(|db| { ... })`.
- `run_shared_database` / `run_dedicated_or_shared_database` helpers are removed
  (replaced by the writer). Read helpers (`run_read_database`,
  `read_with_cached_connection`) stay.
- `get_thumbnail` becomes **read-only**: it returns the current thumbnail state
  (and blob if ready) from a read connection and does NOT enqueue. Generation is
  driven entirely by the batched `priority-scope` sync (one write per scroll).
  The response contract is unchanged (`200` ready / `202` pending). The write
  gate `begin_interactive_write` usage in `get_thumbnail` is removed.
- Keep the `is_database_locked_error` soft-pending fallback as defense-in-depth
  (it should now be unreachable, but harmless).

### 4. Blob commit + task worker — `crates/core/src/tasks.rs`

- Decode/resize stays parallel on `spawn_blocking` (10 foreground + 2 background
  concurrency unchanged) — CPU work is not serialized.
- The final commit (`publish_thumbnail_and_succeed_task…`), task state
  transitions, enqueue/dequeue, and startup recovery writes are submitted to the
  `DbWriter` instead of running on per-task `reopen()` connections.
- Remove per-task `reopen()` write connections and the dedicated
  `start_wal_checkpoint_thread` (checkpointing now lives in the actor).
- The scheduler still reads task rows via a read connection; only writes go
  through the actor.

### 5. Scan + watcher — `crates/core/src/scan/mod.rs`, `crates/core/src/watch/mod.rs`

- Scan batch commits (`commit_scan_batch_for_task_attempt`) and watcher upserts
  are submitted through the `DbWriter`.
- The `write_gate` module (`begin_interactive_write` / `yield_to_pending_interactive_writes`
  / `begin_checkpoint` / `wait_for_pending_checkpoint`) is **deleted** — with a
  single writer there is no cross-writer contention to coordinate, so the gate
  and its call sites are removed. (This also removes the checkpoint-barrier code
  added in prior sessions.)

### 6. Client — `apps/web/src/core/useLibraryData.ts`, `mediaResources.ts`

- Stop issuing a per-tile `get_thumbnail` state write for generation. Continue to
  read thumbnail state for rendering, but rely on the batched `priority-scope`
  sync to drive generation. (Exact client change scoped during planning; the
  server no longer enqueues on `get_thumbnail` regardless, so this is an
  optimization to cut redundant requests, not a correctness requirement.)

## Data flow (folder switch, after)

1. UI switches folder → reads folders/media via read connections (fast, never
   blocked).
2. UI posts one `priority-scope` sync for the visible/ahead set → one write job
   to the actor → enqueues the needed thumbnail tasks in a single transaction.
3. Worker decodes visible thumbnails in parallel; each commit is one write job to
   the actor. No two writes ever contend.
4. Actor runs a checkpoint between job batches; WAL stays bounded.
5. UI polls `get_thumbnail` (read-only) / blob until ready.

## Error handling

- Writer jobs that error return the error to the caller; the actor connection is
  never poisoned and the actor never dies on a job error.
- `database is locked` is impossible with one writer; the existing soft-pending
  mapping stays as defense-in-depth.
- If the writer actor thread panics (should not happen), `write()` returns an
  error rather than hanging (the oneshot sender is dropped → caller sees a
  "writer unavailable" error). Callers already surface errors as HTTP 500 for
  genuine faults, which is correct for an actual writer failure.

## Testing

- **Unit (`db/writer.rs`):** concurrent `write()` calls are serialized and each
  gets its correct typed result; a job returning `Err` is delivered to that
  caller and the next job still succeeds; a value round-trips; checkpoint runs
  without a separate thread.
- **Migrated integration tests:** existing route/scan/thumbnail tests that used
  the shared mutex now go through the writer; behavior (status codes, queued
  task ids, recovery) is unchanged.
- **Structural checks:** `validate-core-api.mjs` markers that pin
  `run_shared_database` / `run_dedicated_or_shared_database` on specific routes
  are updated to the writer; the `write_gate` references in checks (if any) are
  removed.
- **Live verification (the acceptance gate):** the 20-top-level-folder switch
  test (`tools/dev` CDP harness) must show **0 500s, 0 never-loads, WAL bounded,
  generation draining**. Re-run the durable regression suite (10/10) with the
  settings window open (stats polling) to confirm no regression.

## Non-goals

- No change to the read path, thumbnail decode/resize, layout, or UI design.
- No change to the viewport-priority scheduling model (selected/visible/ahead
  buckets) beyond routing its writes through the actor.
- Not introducing a general job/queue framework — this is one focused writer
  actor for the single SQLite write connection.
