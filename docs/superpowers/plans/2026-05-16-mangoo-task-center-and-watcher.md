# Mangoo Task Center And Watcher Implementation Plan

> **Status (2026-05-17):** Complete. Watcher, scan reconciliation, and Task Center UI shipped. See `docs/superpowers/handovers/2026-05-17-phase4b-phase4c-handoff.md` for the resolved blockers and final state.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the backend watcher that keeps the SQLite index current for external filesystem changes under enabled roots, with incremental updates for normal edits and bounded rescan recovery for overflow.

**Architecture:** Keep the existing SQLite task model and root scan pipeline as the source of truth. Add a focused watcher module in `crates/core/src/watch/mod.rs` that listens to enabled roots, classifies filesystem events into incremental upserts or subtree invalidations, and falls back to a queued root rescan when the event stream is too noisy or unreliable.

**Tech Stack:** Rust, Tokio, SQLite via rusqlite, notify 8.2, existing Mangoo scan/task/database code.

---

## File Structure

**Create:**

- `D:/Mangoo/crates/core/src/watch/mod.rs`

**Modify:**

- `D:/Mangoo/crates/core/Cargo.toml`
- `D:/Mangoo/crates/core/src/api/mod.rs`
- `D:/Mangoo/crates/core/src/db/mod.rs`
- `D:/Mangoo/crates/core/src/main.rs`

## Task 1: Add watcher scaffolding and failing tests

**Files:**

- Create: `D:/Mangoo/crates/core/src/watch/mod.rs`
- Modify: `D:/Mangoo/crates/core/Cargo.toml`
- Modify: `D:/Mangoo/crates/core/src/db/mod.rs`

- [ ] **Step 1: Add the watcher dependency and the minimal DB helpers it needs**

Update `D:/Mangoo/crates/core/Cargo.toml` so the core crate can build the watcher.

```toml
[dependencies]
anyhow.workspace = true
axum.workspace = true
notify = "8.2.0"
rusqlite.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
tokio.workspace = true
tower.workspace = true
tower-http.workspace = true
tracing.workspace = true
uuid.workspace = true
walkdir.workspace = true
```

Add the smallest DB helpers the watcher will call from `D:/Mangoo/crates/core/src/db/mod.rs`.

```rust
pub(crate) fn list_enabled_root_records(&self) -> anyhow::Result<Vec<RootRecord>>;
pub(crate) fn find_root_by_path(&self, path: &Path) -> anyhow::Result<Option<RootRecord>>;
pub(crate) fn list_active_files_in_folder(&self, folder_id: i64) -> anyhow::Result<Vec<FileRecord>>;
pub(crate) fn mark_file_missing_by_id(&self, file_id: i64) -> anyhow::Result<()>;
pub(crate) fn mark_folder_subtree_missing_by_id(&self, folder_id: i64) -> anyhow::Result<()>;
pub(crate) fn ensure_folder_chain_for_path(
    &self,
    root_id: i64,
    root_path: &Path,
    folder_path: &Path,
) -> anyhow::Result<i64>;
```

- [ ] **Step 2: Write the failing watcher tests**

Create `D:/Mangoo/crates/core/src/watch/mod.rs` with tests that describe the required behavior before any implementation lands.

```rust
#[tokio::test]
async fn added_media_under_watched_root_becomes_visible_without_restart() {
    // start watcher
    // create root, add it to DB, write image.jpg under the root
    // wait for /api/media?rootId=... to report the file
    // assert the task queue did not require a manual rescan
}

#[tokio::test]
async fn deleted_media_disappears_after_watcher_processes_remove_event() {
    // seed image.jpg, confirm it is visible
    // delete it from disk
    // wait for media listing to become empty
}

#[tokio::test]
async fn rename_or_move_within_root_keeps_one_index_row() {
    // seed old.jpg, confirm visible
    // rename it to new.jpg in the same watched root
    // assert old name is gone and new name exists exactly once
}

#[tokio::test]
async fn non_media_creates_are_ignored() {
    // write notes.txt under the watched root
    // assert media listing stays empty
}

#[tokio::test]
async fn disabled_roots_are_ignored_by_watcher_refresh_and_events() {
    // create a root, disable it, then write image.jpg beneath it
    // assert no incremental row appears and no rescan task is queued
}

#[tokio::test]
async fn overflow_falls_back_to_a_bounded_rescan_or_equivalent_safe_recovery() {
    // inject an overflow notification into the watcher event processor
    // assert the root gets queued for a root_scan task exactly once
}
```

- [ ] **Step 3: Verify the new tests fail for the right reason**

Run:

```bash
cargo test -p mangoo-core watch:: -- --nocapture
```

Expected:

- compilation fails because `watch` is not implemented yet, or
- the new tests fail because the watcher does not start or does not update the index.

## Task 2: Implement incremental path reconciliation and missing-state recovery

**Files:**

- Create: `D:/Mangoo/crates/core/src/watch/mod.rs`
- Modify: `D:/Mangoo/crates/core/src/db/mod.rs`

- [ ] **Step 1: Add the directory/file reconciliation helpers**

Implement the watcher module around a narrow internal API:

```rust
pub fn start_watcher(database: Database, task_queue: TaskSender) -> WatcherHandle;

fn handle_event(state: &WatcherState, event: notify::Event) -> anyhow::Result<()>;
fn handle_create(state: &WatcherState, path: &Path) -> anyhow::Result<()>;
fn handle_modify(state: &WatcherState, path: &Path) -> anyhow::Result<()>;
fn handle_remove(state: &WatcherState, path: &Path) -> anyhow::Result<()>;
fn reconcile_directory(state: &WatcherState, folder_path: &Path) -> anyhow::Result<()>;
fn reconcile_subtree(state: &WatcherState, folder_path: &Path) -> anyhow::Result<()>;
fn mark_subtree_missing(state: &WatcherState, folder_path: &Path) -> anyhow::Result<()>;
```

Use `walkdir` only for subtree rescans, and keep ordinary file edits on the parent-directory path.

```rust
match event.kind {
    notify::EventKind::Create(_) => handle_create(&state, path)?,
    notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
        // treat rename-from, rename-to, and paired rename events as remove + create
    }
    notify::EventKind::Modify(_) => handle_modify(&state, path)?,
    notify::EventKind::Remove(_) => handle_remove(&state, path)?,
    _ => {}
}
```

- [ ] **Step 2: Reconcile folders and files with the existing scan semantics**

Use the same extension/media-kind rules as `D:/Mangoo/crates/core/src/scan/mod.rs`, but only for the affected path or subtree.

```rust
if let Some(kind) = media_kind(path) {
    let folder_id = database.ensure_folder_chain_for_path(root.id, &root_path, parent_dir)?;
    database.upsert_file(FileUpsert {
        root_id: root.id,
        folder_id,
        name: file_name(path),
        ext: extension(path),
        size: metadata.len() as i64,
        mtime: metadata_time(Some(&metadata), TimeField::Modified).unwrap_or(0),
        ctime: metadata_time(Some(&metadata), TimeField::Created),
        file_key: None,
    })?;
    database.upsert_media_kind(file_id, kind)?;
}
```

For deletions and directory removals, keep the rows but mark them missing so they disappear from the active queries immediately.

```rust
database.mark_file_missing_by_id(file_id)?;
database.mark_folder_subtree_missing_by_id(folder_id)?;
```

- [ ] **Step 3: Verify the watcher tests turn green**

Run:

```bash
cargo test -p mangoo-core watch:: -- --nocapture
```

Expected:

- the added-media, deletion, rename/move, non-media, disabled-root, and overflow tests all pass.

## Task 3: Wire watcher startup into app startup and add overflow recovery

**Files:**

- Modify: `D:/Mangoo/crates/core/src/api/mod.rs`
- Modify: `D:/Mangoo/crates/core/src/main.rs`

- [ ] **Step 1: Start the watcher alongside the worker**

Have `D:/Mangoo/crates/core/src/api/mod.rs` spawn the watcher from `AppState::new` and `AppState::new_with_worker` when the database can be reopened.

```rust
if let Some(watcher_database) = database.reopen().expect("reopen watcher database") {
    let watcher_queue = task_queue.clone();
    let _watcher = crate::watch::start_watcher(watcher_database, watcher_queue);
}
```

- [ ] **Step 2: Fall back to a queued rescan when the watcher overflows**

Inside `D:/Mangoo/crates/core/src/watch/mod.rs`, treat notify overflow and unrecoverable root mismatches as a signal to enqueue a single `root_scan` task for the affected root, instead of trying to replay a huge burst of events.

```rust
let task_id = database.create_root_scan_task(root_id)?;
task_queue.blocking_send(task_id)?;
```

- [ ] **Step 3: Run the backend checks**

Run:

```bash
cargo test -p mangoo-core watch::
cargo test -p mangoo-core api::routes::
```

Expected:

- watcher tests pass,
- existing API/task tests still pass,
- the core API contract remains unchanged for callers.
