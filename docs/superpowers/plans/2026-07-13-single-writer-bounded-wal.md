# Single-Writer Bounded-WAL Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route the thumbnail worker, scan worker, and watcher writes through the single `DbWriter` actor so the WAL is always checkpointed and can never grow unbounded.

**Architecture:** Decode/resize stays parallel on `spawn_blocking` threads; only the SQLite *write* (publish transaction, scan batch commit, watcher upsert) moves onto the one `DbWriter` connection, which checkpoints (PASSIVE) after each drained batch. Reads stay on the per-task/worker/watcher connections. fsops stays exempt by design.

**Tech Stack:** Rust, rusqlite (SQLite WAL), tokio (`spawn_blocking` + dedicated writer `std::thread`), anyhow.

## Global Constraints

- MSVC toolchain on Windows. To run cargo when not on PATH: `export PATH="$HOME/.cargo/bin:$PATH"`.
- Every task ends green on `cargo test -p megle-core` (or the specific test named). Full gate `npm test` runs once at the end (Task 6).
- `rustfmt` must pass: `cargo fmt --all --check`. Run `cargo fmt --all` before each commit.
- Debug logging uses the existing `crate::debug_log!` macro ONLY (gated by the global debug flag, default off). Do NOT add a new logging framework or `println!`/`eprintln!`.
- Do NOT change SQLite PRAGMA tuning (`wal_autocheckpoint=0`, `journal_size_limit=64MiB`, `synchronous=NORMAL`).
- Do NOT migrate fsops writes (they keep their own `Arc<Mutex<Database>>` by design — see `api/mod.rs:30-35`).
- Preserve read/write connection separation: only *writes* route to the actor; reads stay on the caller's connection.
- `DbWriter::write_blocking<T,F>(op: F) -> anyhow::Result<T>` where `F: FnOnce(&mut Database) -> anyhow::Result<T> + Send + 'static`, `T: Send + 'static` — the entry point for the non-async worker/watcher threads. It already exists in `crates/core/src/db/writer.rs` (currently `#[allow(dead_code)]`); remove that attribute when first used.

---

## File Structure

- `crates/core/src/db/writer.rs` — `DbWriter` actor. Add a WAL-size debug log after each checkpoint pass; drop `#[allow(dead_code)]` on `write_blocking` once used. Add the bounded-WAL regression test module.
- `crates/core/src/db/mod.rs` — no schema change. May expose a small `wal_size_bytes()` read helper for the checkpoint debug log and the test assertion.
- `crates/core/src/tasks.rs` — thread a `DbWriter` into the thumbnail leaf functions; route the publish through `writer.write_blocking(...)`; add `debug_log!` at publish submission.
- `crates/core/src/scan/mod.rs` — route `commit_scan_batch_with_optional_task_guard` writes through an optional `DbWriter`; add `debug_log!` at batch commit.
- `crates/core/src/watch/mod.rs` — route `upsert_media_file` writes through `state.writer.write_blocking(...)`; add `debug_log!` at upsert.

Task order: **1** (WAL-size helper + checkpoint log — foundation used by the test) → **2** (thumbnail worker, the actual bug) → **3** (scan worker) → **4** (watcher) → **5** (bounded-WAL regression test) → **6** (full gate + cleanup).

---

## Task 1: WAL-size helper + checkpoint debug log

**Files:**
- Modify: `crates/core/src/db/mod.rs` (add `wal_size_bytes`)
- Modify: `crates/core/src/db/writer.rs:113-120` (log WAL size after checkpoint)
- Test: `crates/core/src/db/mod.rs` (unit test for `wal_size_bytes`)

**Interfaces:**
- Produces: `Database::wal_size_bytes(&self) -> u64` — returns the size in bytes of the `-wal` sidecar for this database's file, or `0` for in-memory / missing WAL. Used by the checkpoint debug log (this task) and the bounded-WAL test (Task 5).

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/core/src/db/mod.rs`:

```rust
#[test]
fn wal_size_bytes_reports_zero_for_in_memory_and_reads_file_wal() {
    // In-memory DB has no WAL sidecar.
    let mem = Database::open_in_memory().expect("open in-memory");
    assert_eq!(mem.wal_size_bytes(), 0);

    // File-backed DB in WAL mode: after a write (no checkpoint), the -wal
    // sidecar exists and is non-empty.
    let dir = unique_db_temp_dir("wal-size");
    let db_path = dir.join("wal.sqlite");
    let db = Database::open(&db_path).expect("open file db");
    db.apply_migrations().expect("apply migrations");
    db.create_tag("t", None).expect("write");
    assert!(
        db.wal_size_bytes() > 0,
        "expected a non-empty -wal after a write in WAL mode"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p megle-core wal_size_bytes_reports_zero -- --nocapture`
Expected: FAIL — `no method named wal_size_bytes found for struct Database`.

- [ ] **Step 3: Implement `wal_size_bytes`**

Add as a method on `impl Database` in `crates/core/src/db/mod.rs` (near `run_wal_checkpoint_pass`):

```rust
/// Size in bytes of the `-wal` sidecar for this database's file, or 0 for
/// in-memory databases or when the WAL file is absent. Used for checkpoint
/// diagnostics and to assert the WAL stays bounded in tests.
pub fn wal_size_bytes(&self) -> u64 {
    let Some(path) = self.path.as_ref() else {
        return 0;
    };
    let mut wal = path.clone().into_os_string();
    wal.push("-wal");
    std::fs::metadata(std::path::PathBuf::from(wal))
        .map(|meta| meta.len())
        .unwrap_or(0)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p megle-core wal_size_bytes_reports_zero -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Add the WAL-size checkpoint debug log**

In `crates/core/src/db/writer.rs`, replace the checkpoint block in `run_writer_actor` (currently lines 113-120):

```rust
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
```

- [ ] **Step 6: Format, build, commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all
cargo test -p megle-core wal_size_bytes_reports_zero
git add crates/core/src/db/mod.rs crates/core/src/db/writer.rs
git commit -m "feat(core): add wal_size_bytes + log WAL size after each checkpoint pass"
```

---

## Task 2: Route thumbnail publish through DbWriter

This is the fix for the 19 GB blowup. Thread a `DbWriter` into the thumbnail leaf functions and route the single publish call through it. Decode/resize stays on the `spawn_blocking` thread.

**Files:**
- Modify: `crates/core/src/tasks.rs` — `start_worker` (stop discarding `writer`), `try_spawn_thumbnail_task` (capture `writer` into the blocking closure), the thumbnail run chain (`run_thumbnail_task_with_cache*`), and the publish site (`tasks.rs:1154-1172`).
- Test: `crates/core/src/tasks.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `DbWriter::write_blocking` (Task 0/existing), `Database::publish_thumbnail_and_succeed_task_if_source_fingerprint_and_task_attempt_current(...)` (existing, returns `anyhow::Result<bool>`).
- Produces: thumbnail publish now runs on the writer connection; the per-task connection is read-only for thumbnails.

**Context — current call chain (all `&mut Database`):**
`try_spawn_thumbnail_task` (spawns blocking thread, owns `thumbnail_database`) → `run_task` → `run_task_with_database` → (thumbnail arm) → `run_thumbnail_task_with_cache` → `..._for_attempt` → `..._and_before_publish` → `..._and_before_publish_for_attempt` → publish at line 1154.

The thumbnail arm is a *leaf* branch. We thread a `writer: &DbWriter` param down only the thumbnail leaf functions (not the scan/root arms). The blob commit closure moves `blob` + the `ThumbnailStateUpsert` + scalars into `write_blocking`.

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `crates/core/src/tasks.rs`. This asserts a thumbnail publish routed through the writer lands the blob AND leaves the worker connection's WAL bounded:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thumbnail_publish_routes_through_writer_and_bounds_wal() {
    use crate::db::{Database, DbWriter};
    let dir = crate::db::testing::unique_db_temp_dir("thumb-writer");
    let db_path = dir.join("thumb.sqlite");
    let seed = Database::open(&db_path).expect("open");
    seed.apply_migrations().expect("migrate");
    // Seed a root + folder + one image file + a pending thumbnail task.
    let ctx = seed_one_pending_thumbnail_task(&seed);

    let worker_db = seed.reopen().expect("reopen").expect("worker conn");
    let writer = DbWriter::spawn(seed);

    // Publish through the writer (the code path under test).
    publish_thumbnail_for_test(&worker_db, &writer, ctx.task_id, ctx.file_id)
        .await
        .expect("publish via writer");

    // Blob landed.
    let blob = worker_db
        .thumbnail_blob(ctx.file_id, "grid_320")
        .expect("read blob");
    assert!(blob.is_some(), "blob must be retrievable after publish");

    // The worker connection's WAL is bounded (writer checkpointed it).
    assert!(
        worker_db.wal_size_bytes() < 64 * 1024 * 1024,
        "WAL must stay under the 64MiB journal_size_limit, got {}",
        worker_db.wal_size_bytes()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
```

Note: `seed_one_pending_thumbnail_task`, `publish_thumbnail_for_test`, and `ctx` (`TestThumbCtx { task_id, file_id }`) are small test helpers you write in this same module in Step 3. `thumbnail_blob` is the existing blob read method (confirm its exact name with `grep -n "fn thumbnail_blob\|fn get_thumbnail_blob\|fn read_thumb_blob" crates/core/src/db/mod.rs` and use the real one).

- [ ] **Step 2: Run test to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p megle-core thumbnail_publish_routes_through_writer -- --nocapture`
Expected: FAIL — helpers/`publish_thumbnail_for_test` not defined.

- [ ] **Step 3: Thread `writer` into the thumbnail leaf + route the publish**

3a. In `start_worker` (`crates/core/src/tasks.rs`), remove `let _writer = writer;` and keep `writer` live; capture a clone into each spawned thumbnail task.

3b. In `try_spawn_thumbnail_task`, add a `writer: DbWriter` parameter (clone from the caller), and move it into the `spawn_blocking` closure alongside `thumbnail_database`. Update `run_task`'s thumbnail path to receive `&DbWriter`.

3c. Add a `writer: &DbWriter` parameter to the thumbnail leaf chain: `run_thumbnail_task_with_cache`, `run_thumbnail_task_with_cache_for_attempt`, `run_thumbnail_task_with_cache_and_before_publish`, `run_thumbnail_task_with_cache_and_before_publish_for_attempt`.

3d. Replace the publish call (`tasks.rs:1154-1172`) — move the write onto the writer. The reads/guards above it (`resolve_file_source_path`, `ensure_task_not_cancelled`, `task_attempt_is_current`) stay on the per-task `database`:

```rust
    crate::debug_log!(
        "thumbnail publish -> writer: task {} file {} blob {} bytes",
        task_id,
        file_id,
        blob.byte_size
    );
    let state = ThumbnailStateUpsert {
        file_id,
        profile: GRID_320_PROFILE.to_string(),
        state: "ready".to_string(),
        cache_key: None,
        width: Some(generated.width),
        height: Some(generated.height),
        byte_size: Some(generated.byte_size),
        error: None,
        source_fingerprint: Some(source_fingerprint.clone()),
    };
    let fingerprint = source_fingerprint.clone();
    let published = writer
        .write_blocking(move |db| {
            db.publish_thumbnail_and_succeed_task_if_source_fingerprint_and_task_attempt_current(
                state,
                blob,
                source_dimensions,
                &fingerprint,
                task_id,
                attempt_generation,
            )
        })?;
```

(`blob`, `state`, `fingerprint`, `source_dimensions` are moved into the closure; `generated`'s scalars are copied before the move. `write_blocking` returns the same `anyhow::Result<bool>` as before, so the surrounding `?` and `published` handling are unchanged.)

3e. Write the test helpers (`seed_one_pending_thumbnail_task`, `publish_thumbnail_for_test`, `TestThumbCtx`) in the test module. `publish_thumbnail_for_test` calls the same leaf publish path (or, if the leaf isn't easily callable in isolation, directly exercises `writer.write_blocking(|db| db.publish_thumbnail_and_succeed_...())` with a synthesized blob) so the test drives the real write route.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p megle-core thumbnail_publish_routes_through_writer -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run the whole tasks module to catch regressions**

Run: `cargo test -p megle-core --lib tasks`
Expected: PASS (existing thumbnail/scheduler tests still green).

- [ ] **Step 6: Format, commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all
git add crates/core/src/tasks.rs
git commit -m "fix(core): route thumbnail publish through single DbWriter (bounded WAL)"
```

---

## Task 3: Route scan batch commits through DbWriter

**Files:**
- Modify: `crates/core/src/scan/mod.rs` — `ScanWriteOptions` (add optional writer), `commit_scan_batch_with_optional_task_guard` (route the write), callers that build scan options.
- Modify: `crates/core/src/tasks.rs` — pass the worker's `DbWriter` into `scan_root_with_options` / `scan_folder_with_options` via the options struct.
- Test: `crates/core/src/scan/mod.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `DbWriter::write_blocking`.
- Produces: scan batch commits run on the writer connection when a writer is provided; when absent (unit tests calling `scan_root(&mut db, ...)` directly), commits fall back to the passed `&mut Database` (preserves existing test ergonomics).

**Context:** `commit_scan_batch_with_optional_task_guard(database: &mut Database, batch: ScanWriteBatch, options: &ScanWriteOptions)` at `scan/mod.rs:465`. Scan already batches by `write_batch_size`, so the actor receives coarse commits.

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `crates/core/src/scan/mod.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scan_commits_through_writer_keep_wal_bounded() {
    use crate::db::{Database, DbWriter};
    let dir = crate::db::testing::unique_db_temp_dir("scan-writer");
    let db_path = dir.join("scan.sqlite");
    let seed = Database::open(&db_path).expect("open");
    seed.apply_migrations().expect("migrate");
    let root = seed_root_with_media_dir(&seed); // helper: creates a root dir with N image files
    let worker_db = seed.reopen().expect("reopen").expect("worker conn");
    let writer = DbWriter::spawn(seed);

    // Scan routed through the writer.
    scan_root_via_writer_for_test(&worker_db, &writer, &root)
        .await
        .expect("scan via writer");

    assert!(
        worker_db.wal_size_bytes() < 64 * 1024 * 1024,
        "scan WAL must stay bounded, got {}",
        worker_db.wal_size_bytes()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
```

(`seed_root_with_media_dir` and `scan_root_via_writer_for_test` are helpers written in Step 3; the latter builds `ScanWriteOptions` with the writer attached and calls `scan_root_with_options`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p megle-core scan_commits_through_writer -- --nocapture`
Expected: FAIL — helpers / writer field on options not defined.

- [ ] **Step 3: Add optional writer to scan options + route the commit**

3a. Add an optional writer to `ScanWriteOptions` (the options struct around `scan/mod.rs:30-60`):

```rust
    /// When present, batch commits run on the single writer connection instead
    /// of the caller's `&mut Database`, so the WAL is checkpointed and stays
    /// bounded. `None` in unit tests that call `scan_root(&mut db, ...)` directly.
    pub writer: Option<crate::db::DbWriter>,
```

Initialize `writer: None` everywhere `ScanWriteOptions`/`ScanRootOptions` is constructed (e.g. `for_priority`, `root_scan_options`, `interactive_scan_options`), then set it from the worker in Task 3b.

3b. In `commit_scan_batch_with_optional_task_guard` (`scan/mod.rs:465`), route the write:

```rust
    crate::debug_log!(
        "scan batch commit ({} folders, {} files){}",
        batch.folders.len(),
        batch.files.len(),
        if options.writer.is_some() { " -> writer" } else { " (inline)" }
    );
    match options.writer.as_ref() {
        Some(writer) => {
            let guard = options.task_guard;
            writer.write_blocking(move |db| commit_scan_batch(db, batch, guard))?
        }
        None => commit_scan_batch(database, batch, options.task_guard)?,
    }
```

(Adjust `commit_scan_batch`'s exact signature/return to match the existing inner commit function — confirm with `grep -n "fn commit_scan_batch" crates/core/src/scan/mod.rs`. The `task_guard` is `Copy` (`TaskAttemptGuard`); if it isn't, clone it before the move.)

3c. In `crates/core/src/tasks.rs`, set `options.writer = Some(writer.clone())` when constructing scan options for `root_scan` and `interactive_folder_scan` arms (thread the `&DbWriter` into `run_task_with_database` for those arms, same handle used in Task 2).

3d. Write the test helpers in the scan test module.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p megle-core scan_commits_through_writer -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run the scan module tests**

Run: `cargo test -p megle-core --lib scan`
Expected: PASS (existing scan tests use `writer: None` and still commit inline).

- [ ] **Step 6: Format, commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all
git add crates/core/src/scan/mod.rs crates/core/src/tasks.rs
git commit -m "fix(core): route scan batch commits through single DbWriter"
```

---

## Task 4: Route watcher upserts through DbWriter

**Files:**
- Modify: `crates/core/src/watch/mod.rs` — `upsert_media_file` (`watch/mod.rs:258-289`) routes `upsert_file` + `upsert_media_kind` through `state.writer`.
- Test: `crates/core/src/watch/mod.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `DbWriter::write_blocking`, existing `Database::upsert_file(FileUpsert) -> anyhow::Result<i64>` and `Database::upsert_media_kind(file_id, kind)`.
- Produces: watcher file upserts run on the writer connection; `state.database` stays for reads (`list_roots`, `get_root`).

**Context:** `WatcherState` already has `writer: DbWriter` (`watch/mod.rs:75`). `upsert_media_file` currently calls `state.database.upsert_file(...)` (line 274) then `state.database.upsert_media_kind(...)` (line 284).

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `crates/core/src/watch/mod.rs` (follow the existing watcher test setup that builds a `WatcherState` with a `DbWriter`):

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn watcher_upserts_route_through_writer_and_land() {
    // Build a WatcherState with a real file DB + writer (mirror existing tests).
    let (mut state, worker_db, dir, root, path, metadata) = watcher_state_with_one_file();
    upsert_media_file(&mut state, &root, &path, &metadata).expect("upsert via writer");
    // The file row exists (written on the writer connection, read on another).
    let found = worker_db
        .find_file_by_path(&root, &path)
        .expect("query file");
    assert!(found.is_some(), "watched file must be upserted");
    let _ = std::fs::remove_dir_all(&dir);
}
```

(`watcher_state_with_one_file` is a helper mirroring the existing watcher tests; `find_file_by_path` — use the real existing lookup method, confirm name with `grep -n "fn find_file\|fn get_file_by_path\|fn file_by_path" crates/core/src/db/mod.rs`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p megle-core watcher_upserts_route_through_writer -- --nocapture`
Expected: FAIL — helper not defined (and, before Step 3, upsert still on `state.database`).

- [ ] **Step 3: Route the upsert through the writer**

In `upsert_media_file` (`crates/core/src/watch/mod.rs:258`), replace the two direct writes (lines 274-284) with a single writer job that does both in one closure (keeps them on one connection/transaction boundary):

```rust
    crate::debug_log!("watcher upsert -> writer: {}", path.display());
    let file_upsert = FileUpsert { /* same fields as today */ };
    let kind_owned = kind.to_owned();
    let file_id = state.writer.write_blocking(move |db| {
        let file_id = db.upsert_file(file_upsert)?;
        db.upsert_media_kind(file_id, &kind_owned)?;
        Ok(file_id)
    })?;
```

(Copy the exact `FileUpsert { .. }` field construction from the current lines 274-283 into `file_upsert`. Reads after this — `list_roots`, `get_root` — stay on `state.database`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p megle-core watcher_upserts_route_through_writer -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run the watch module tests**

Run: `cargo test -p megle-core --lib watch`
Expected: PASS.

- [ ] **Step 6: Format, commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all
git add crates/core/src/watch/mod.rs
git commit -m "fix(core): route watcher file upserts through single DbWriter"
```

---

## Task 5: End-to-end bounded-WAL regression test

The per-task tests above prove each path. This task adds the headline proof: a sustained burst of publishes keeps the WAL bounded, and would have caught the original blowup.

**Files:**
- Test: `crates/core/src/db/writer.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `DbWriter::spawn`, `DbWriter::write_blocking`, `Database::wal_size_bytes` (Task 1), `Database::upsert_thumb_blob` (existing).

- [ ] **Step 1: Write the test**

Add to `mod tests` in `crates/core/src/db/writer.rs`:

```rust
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
    let file_ids = seed_files_for_blobs(&seed, 400);
    let probe = seed.reopen().expect("reopen").expect("probe conn");
    let writer = DbWriter::spawn(seed);

    // Write 400 ~64KB blobs through the writer (≈25MB of data). Without
    // checkpointing this would push the WAL well past 64MiB over time; the
    // writer checkpoints between batches so it stays bounded.
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
                    output_format: "webp".to_string(),
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
```

(`seed_files_for_blobs(&Database, n) -> Vec<i64>` — small helper: insert a root+folder+n file rows, return their ids. Match `ThumbBlobRecord`'s exact fields to the struct in `db/mod.rs`. Confirm `upsert_thumb_blob` is `pub` — it is at `db/mod.rs:4396`.)

- [ ] **Step 2: Run test to verify it passes (with the fix in place)**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p megle-core sustained_blob_writes_through_writer -- --nocapture`
Expected: PASS.

- [ ] **Step 3: Sanity-check it would fail without checkpointing**

Temporarily comment out the `database.run_wal_checkpoint_pass()` call in `run_writer_actor`, re-run the test, and confirm it FAILS (WAL grows past the bound). Then restore the checkpoint call. This proves the test actually guards the behavior.

Run: `cargo test -p megle-core sustained_blob_writes_through_writer -- --nocapture`
Expected: FAIL while checkpoint is commented out; PASS after restoring.

- [ ] **Step 4: Format, commit**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all
git add crates/core/src/db/writer.rs
git commit -m "test(core): bounded-WAL regression for sustained writer blob commits"
```

---

## Task 6: Full verification gate + cleanup

**Files:**
- Modify: `crates/core/src/db/writer.rs` (remove `#[allow(dead_code)]` on `write_blocking` if still present)
- Modify: `CLAUDE.md` (update the Core Performance Model note)

- [ ] **Step 1: Remove the now-unnecessary `dead_code` allow**

In `crates/core/src/db/writer.rs`, delete the `#[allow(dead_code)] // Reserved for the watcher/worker write migration.` attribute above `write_blocking` (it is now used by Tasks 2-4).

- [ ] **Step 2: Grep for any remaining direct worker/watcher writes**

Run:
```bash
grep -n "\.upsert_file(\|\.upsert_media_kind(\|publish_thumbnail_and_succeed" crates/core/src/tasks.rs crates/core/src/watch/mod.rs
```
Expected: the only matches are inside `write_blocking(move |db| ...)` closures (i.e. running on the actor), not on a per-task/`state.database` connection. If any direct call remains, route it.

- [ ] **Step 3: Update CLAUDE.md**

In the "Core Performance Model" → "DB connections" bullet, append:

```markdown
- **All non-API writes go through the single `DbWriter` actor.** The thumbnail worker, scan worker, and watcher submit their commits via `writer.write_blocking(...)`; the actor is the one connection that checkpoints (PASSIVE, after each drained batch), so the WAL stays bounded. Only fsops writes stay off the actor (their own `Arc<Mutex<Database>>`), by design. A worker/watcher writing directly on its `reopen()`'d connection is the bug that let the WAL grow to 19 GB during full thumbnail generation — do not reintroduce it.
```

- [ ] **Step 4: Full gate**

Run:
```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all --check
npm test
```
Expected: all green (structure, contracts, schema, typecheck, `cargo test --workspace`).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/db/writer.rs CLAUDE.md
git commit -m "chore(core): finish single-writer migration (drop dead_code, document invariant)"
```

---

## Self-review notes (addressed)

- **Spec coverage:** worker publish (Task 2), scan commits (Task 3), watcher upserts (Task 4), debug logs at each branch (Tasks 1-4), bounded-WAL proof (Tasks 2/3/5), fsops exempt (untouched — verified in Task 6 grep). Deferred items (startup self-heal, timeout) correctly absent.
- **Type consistency:** `write_blocking<T,F>` signature used identically in Tasks 2-4; `wal_size_bytes` defined Task 1, consumed Tasks 2/3/5; `ThumbBlobRecord`/`ThumbnailStateUpsert`/`FileUpsert` fields flagged to copy verbatim from `db/mod.rs`.
- **Uncertain method names** (`thumbnail_blob`, `find_file_by_path`, `commit_scan_batch`) are flagged with a `grep` to confirm the real name before use, rather than guessed — the implementer resolves them against the actual source.
