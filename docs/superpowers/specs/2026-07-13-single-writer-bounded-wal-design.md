# Route all non-API writes through the single DbWriter (bounded WAL)

- **Date:** 2026-07-13
- **Status:** Approved design, ready for implementation planning
- **Area:** Rust Core (`crates/core`) — DB write path, thumbnail/scan/watch workers

## Problem

A full thumbnail-generation run left the app unable to open. Root cause (confirmed
by systematic debugging, not assumed):

- `configure_file_database_wal` sets `wal_autocheckpoint = 0` (SQLite's inline
  autocheckpoint is deliberately disabled to keep checkpoints off the interactive
  write path) and `journal_size_limit = 64 MiB` (only truncates *after* a
  checkpoint).
- The **only** code that checkpoints is the `DbWriter` actor, which runs
  `run_wal_checkpoint_pass()` (PASSIVE) after draining each job batch
  (`db/writer.rs:113-116`).
- But the thumbnail worker, scan worker, and watcher each write on their **own
  `reopen()`'d connection** and never route through `DbWriter`, so they never
  checkpoint. `tasks.rs` even discards the handle: `let _writer = writer;` with a
  NOTE that routing worker commits through the writer "is the remaining step of
  the single-writer migration."
- During a background full-generation run the actor is idle (no jobs), so nothing
  checkpoints. 107,538 thumbnail blobs (~2.9 GB WebP) committed on the worker
  connection grew the WAL to **19 GB**.
- On restart, `Database::open` must replay that 19 GB WAL. Recovery exceeded the
  desktop's 12 s health-check timeout, so the desktop killed Core
  (`[core] exited code=1`) and the app "wouldn't open."

Evidence: main DB file opens immutably in 0.01 s (healthy); all Core startup
steps pass and `/api/health` returns 200 once running; a single clean Core run
regrows the WAL to ~197 MB, confirming the leak is live in normal operation, not
just full generation.

Immediate recovery already performed: opening the DB once checkpointed the 19 GB
WAL away; Core now reaches healthy in ~2 s. This spec is the durable root-cause
fix.

## Goal

Make unbounded WAL growth **structurally impossible**: every non-API write goes
through the single `DbWriter` actor, which is the one place that checkpoints. One
WAL writer connection → PASSIVE checkpoint after each batch → `journal_size_limit`
truncates → the WAL stays bounded during generation and browsing alike.

## Non-goals (explicitly deferred)

- **Startup self-heal** (bounded checkpoint-on-open when an oversized WAL is
  detected) and **relaxing the desktop 12 s health-check timeout** — a separate
  later effort, per the user. Not in this spec.
- **fsops write migration.** fsops deliberately keeps its own
  `Arc<Mutex<Database>>` connection (documented at `api/mod.rs:30-35`): file ops
  are slow and would block the actor, and they are low-volume/user-triggered, not
  a WAL-growth source. Out of scope.
- No changes to SQLite PRAGMA tuning (`wal_autocheckpoint=0`,
  `journal_size_limit=64MiB`, NORMAL sync) — the tuning is correct; the bug is
  that writes bypassed the checkpointer.

## Architecture

**Principle:** decode/resize (CPU-bound, 50–500 ms per image) stays parallel on
`spawn_blocking` threads. Only the **write** — the publish transaction, the scan
batch commit, the watcher upsert — moves onto the single `DbWriter`. Writes are
fast SQLite commits, so serializing them through one actor is cheap and yields the
checkpoint window for free.

**Concurrency safety:** the `DbWriter` actor runs on a dedicated `std::thread`
(`db/writer.rs:36-39`, named `mangoo-db-writer`), NOT tokio's blocking pool, so it
can never be starved by the `spawn_blocking` threads that park on it via
`write_blocking()` → `blocking_recv()`. N blocking threads → 1 actor is safe; no
deadlock.

**Plumbing already exists:** `AppState` holds `writer: DbWriter` and threads it to
`start_worker(worker_database, writer.clone())` and
`start_watcher(watcher_database, task_sender, writer.clone())`. The migration is
incomplete only in the *bodies* — the handle is received and dropped/ignored. No
signature or wiring changes needed; use the handle that is already passed in.

**Reads stay on the per-task/worker/watcher connections.** Only writes route to
the actor. This preserves the read/write connection separation that keeps reads
off the write mutex.

## Write paths to migrate

### 1. Thumbnail worker (`crates/core/src/tasks.rs`) — the 19 GB culprit

- Decode/resize on the `spawn_blocking` thread: unchanged.
- The publish at `tasks.rs:1155`,
  `thumbnail_database.publish_thumbnail_and_succeed_task_if_source_fingerprint_and_task_attempt_current(blob, state, source_dimensions, task_id, attempt_generation, ...)`,
  moves into `writer.write_blocking(move |db| db.publish_thumbnail_and_succeed_...(...))`.
- The blob `Vec<u8>` moves into the closure once (no copy).
- Reads/guards before publish (`resolve_file_source_path`,
  `ensure_task_not_cancelled`, `task_attempt_is_current`) stay on the per-task
  connection. The publish SQL re-checks `status='running' AND attempt_generation=?`
  inside the transaction, so a superseded/cancelled attempt still commits zero
  rows regardless of the slight timing shift onto the actor thread.
- Replace `let _writer = writer;` in `start_worker` with threading the handle into
  the per-task dispatch so the `spawn_blocking` closure can call it.

### 2. Scan worker (`crates/core/src/scan/mod.rs`)

- Traversal + reads stay on the worker connection.
- Each `commit_scan_batch_with_optional_task_guard(database, batch, &options)`
  routes through `writer.write_blocking(move |db| commit_scan_batch...(db, batch, ...))`.
- Scan already batches (`write_batch_size` by priority), so the actor receives
  coarse batch commits, not per-file writes.

### 3. Watcher (`crates/core/src/watch/mod.rs`)

- Per-event `upsert_file` / `upsert_media_kind` (`watch/mod.rs:274,284`) route
  through `writer.write_blocking(...)`.
- The watcher runs on a plain `std::thread`, so `write_blocking` (already present,
  currently `#[allow(dead_code)]` "reserved for the watcher/worker write
  migration") is the correct entry point — this completes that migration and the
  attribute is removed.

## Error handling

- `write_blocking` returns `anyhow::Result<T>` — the same type the direct call
  returns — so every call site's existing error handling (task → `failed`, scan
  retry, watcher warn-and-continue) is unchanged.
- A failing job delivers its error to the caller without poisoning the actor or
  connection (`write_error_is_delivered_and_actor_survives` test proves this).
- Attempt/cancellation correctness is enforced in SQL (transaction re-checks
  status + attempt_generation), not by thread timing — unaffected by the move.
- Actor unavailable (shutdown/drop): `write_blocking` returns
  `Err("database writer is unavailable")`; the task is marked failed and retried on
  next startup. No hang, no crash.
- Checkpoint failure stays non-fatal (`writer.rs:114-116` warns and continues).

## Debug logging (gated by the existing global debug flag, default off)

Reuse the existing `crate::debug_log!` macro (no new framework). Add at key
branches so a future full-generation run is observable:

- Worker: before/after the publish write submission, with `task_id` + blob byte
  size.
- Writer actor: WAL file size after each checkpoint pass, extending the existing
  "checkpoint slow" log at `writer.rs:117-120`, so the WAL can be watched staying
  bounded.
- Scan: batch commit routed through the writer (folder/file counts).
- Watcher: upsert routed through the writer (path).

## Testing

- **Bounded-WAL regression test (core proof):** file-backed DB; spawn the writer
  plus a worker-style second connection; publish N thumbnails through the writer;
  assert the `-wal` file stays under a bound (≈ `journal_size_limit` + margin)
  rather than growing per-blob. This test FAILS on today's code (WAL grows
  unbounded) and passes after the fix.
- **Publish-through-writer test:** a thumbnail publish routed through the writer
  completes and the blob is retrievable via the existing blob read path.
- **Scan + watcher:** existing tests (which already construct a `DbWriter`) must
  stay green with writes routed through the actor.
- Full `npm test` gate (structure, contracts, schema, typecheck, `cargo fmt
  --check`, `cargo test --workspace`) before done.

## Rollout / risk

- Behavior-preserving refactor of the write path; no schema, contract, or API
  changes.
- Main risk is serialization latency if a slow write blocks the actor — mitigated
  because only fast commits (not decode/resize) run on the actor, and scan already
  batches. fsops (the one genuinely slow writer) stays off the actor by design.
- Reversible per-path: each of the three migrations is independent, so a
  regression in one path can be reverted without touching the others.
