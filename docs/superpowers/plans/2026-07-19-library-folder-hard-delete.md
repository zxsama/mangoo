# Hard-delete a folder (root) from the library — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the soft-delete "remove folder from library" with a real hard delete that purges all of the root's DB rows in bounded background batches — never freezing the single writer, resumable across restarts, race-safe against scans/thumbnails, behind a two-step confirmation.

**Architecture:** `remove_root` becomes an O(1) transaction that marks the root `pending_delete_at` (authoritative state), cancels its scan tasks, and enqueues a new `root_purge` task. A background purge handler drains the root leaf-to-root (`tasks → files (cascade) → folders → roots`) via a stateless, resumable `purge_root_next_batch`, each batch a short `DbWriter` job. Startup recovery re-enqueues a purge for any `pending_delete_at` root. Every subsystem that reads/writes roots skips `pending_delete_at` roots.

**Tech Stack:** Rust + rusqlite (SQLite WAL, `foreign_keys = ON`), Axum, single-writer `DbWriter` actor, in-process task worker; React + TypeScript UI; node `--test` for web unit tests.

## Global Constraints

- Migrations are gated in `Database::apply_migrations` (`crates/core/src/db/mod.rs`) via `migration_applied(N)`, declared in `crates/core/src/db/migrations.rs`, the SQL file self-registers with `INSERT OR IGNORE INTO schema_migrations(version, name, applied_at) VALUES (N, '<name>', unixepoch());`, and is asserted in `tools/checks/validate_sqlite_schema.py` (`MIGRATIONS` + `REQUIRED_TABLES`/`REQUIRED_INDEXES`).
- `foreign_keys = ON` for file-backed connections; deleting `files` cascades `media`/`user_metadata`/`file_tags`/`thumbs`/`thumb_blobs`; `tasks`/`file_operations` are `ON DELETE SET NULL`.
- All non-API writes and API `run_shared_database` writes go through the single `DbWriter` (`writer.write` / `write_blocking`). Each writer job must stay short (bounded batch).
- Disk files are NEVER touched. `file_operations` audit rows are kept (refs go NULL).
- New task kind wire value: `root_purge`. Update BOTH `contracts/core-api/openapi.yaml` (`TaskKind` enum) and `packages/core-client/src/generated-contract.ts` (`TaskKind` union); `npm run check:core-api` and `check:core-client` must pass.
- `PURGE_BATCH_SIZE = 5000` (Rust `const`), tuned so one batch's writer job stays short on `thumb_blobs` (BLOB) deletes.
- Rust: `cargo fmt` clean, `cargo test -p mangoo-core` green. Web: `npm --workspace @mangoo/web run typecheck`, `npm --workspace @mangoo/web run test`, `npm run check:web` green.
- Commit with explicit paths (never `git add -A`). Branch `perf/million-thumbnail-hotloop` (already off master).
- Spec: `docs/superpowers/specs/2026-07-19-library-folder-hard-delete-design.md`.

---

## File Structure

- `crates/core/migrations/0019_root_pending_delete.sql` — new: column + partial index + self-registration.
- `crates/core/src/db/migrations.rs` — modify: declare `ROOT_PENDING_DELETE_MIGRATION`.
- `crates/core/src/db/mod.rs` — modify: apply_migrations gate; `mark_root_for_purge`, `purge_root_next_batch`, `roots_pending_delete_ids`, `create_root_purge_task`; add `AND pending_delete_at IS NULL` to `list_roots`/`get_root`; extend `ensure_root_enabled_in_transaction`; retire `disable_root`.
- `crates/core/src/tasks.rs` — modify: `root_purge` dispatch branch; startup recovery re-enqueue.
- `crates/core/src/api/routes.rs` — modify: `remove_root` rewrite; `add_root` 409 on pending-delete path.
- `crates/core/src/api/mod.rs` — modify: sweep skips pending-delete roots (already enumerates via read conn).
- `crates/core/src/roots/reachability.rs` — modify: probe skips pending-delete roots.
- `crates/core/src/watch/mod.rs` — modify: drop events for pending-delete roots.
- `contracts/core-api/openapi.yaml`, `packages/core-client/src/generated-contract.ts` — modify: `TaskKind` += `root_purge`.
- `tools/checks/validate_sqlite_schema.py` — modify: register `0019` + `idx_roots_pending_delete`.
- `apps/web/src/features/library/LibrarySwitcher.tsx` — modify: two-step confirm dialog.
- `apps/web/src/core/useLibraryData.ts` — modify: `removeRoot` optimistic drop (no full reload).

---

## Task 1: Migration 0019 — `roots.pending_delete_at` + partial index

**Files:**
- Create: `crates/core/migrations/0019_root_pending_delete.sql`
- Modify: `crates/core/src/db/migrations.rs`
- Modify: `crates/core/src/db/mod.rs` (apply_migrations gate, `migration_applied(19)`)
- Modify: `tools/checks/validate_sqlite_schema.py`
- Test: `crates/core/src/db/mod.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: column `roots.pending_delete_at INTEGER` (nullable); index `idx_roots_pending_delete`.

- [ ] **Step 1: Write the migration SQL**

Create `crates/core/migrations/0019_root_pending_delete.sql`:
```sql
-- Hard-delete a folder (root): a non-NULL `pending_delete_at` marks a root that
-- is being purged in the background. It is the authoritative "being removed"
-- state (replaces the enabled=0 soft delete) and drives startup recovery.
ALTER TABLE roots ADD COLUMN pending_delete_at INTEGER;

-- Partial index so startup recovery scans only the (usually zero) roots being
-- purged, not the whole roots table.
CREATE INDEX IF NOT EXISTS idx_roots_pending_delete
  ON roots(id) WHERE pending_delete_at IS NOT NULL;

INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
VALUES (19, 'root_pending_delete', unixepoch());
```

- [ ] **Step 2: Declare the migration constant**

In `crates/core/src/db/migrations.rs`, after the `THUMB_FAILURE_KIND_MIGRATION` declaration add:
```rust
pub const ROOT_PENDING_DELETE_MIGRATION: &str =
    include_str!("../../migrations/0019_root_pending_delete.sql");
```

- [ ] **Step 3: Write the failing test (schema gate)**

In `crates/core/src/db/mod.rs` tests module add:
```rust
#[test]
fn migration_0019_adds_root_pending_delete_column_and_index() {
    let database = migrated_database();
    let columns: Vec<String> = {
        let mut stmt = database
            .connection
            .prepare("PRAGMA table_info(roots)")
            .expect("inspect roots");
        stmt.query_map([], |row| row.get::<_, String>(1))
            .expect("query columns")
            .collect::<rusqlite::Result<Vec<String>>>()
            .expect("collect columns")
    };
    assert!(columns.iter().any(|c| c == "pending_delete_at"));
    let has_index: bool = database
        .connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_roots_pending_delete'",
            [],
            |_| Ok(true),
        )
        .optional()
        .expect("query index")
        .unwrap_or(false);
    assert!(has_index, "partial index must exist");
}
```

- [ ] **Step 4: Run test to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test -p mangoo-core migration_0019_adds_root_pending_delete -- --nocapture`
Expected: FAIL — column `pending_delete_at` not present (gate not wired yet).

- [ ] **Step 5: Wire the apply_migrations gate**

In `crates/core/src/db/mod.rs::apply_migrations`, immediately after the `migration_applied(18)` block (the `THUMB_FAILURE_KIND_MIGRATION` transaction) and before `self.backfill_media_fts_if_empty()?;`, add:
```rust
if !self.migration_applied(19)? {
    self.connection
        .execute_batch(migrations::ROOT_PENDING_DELETE_MIGRATION)?;
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p mangoo-core migration_0019_adds_root_pending_delete`
Expected: PASS.

- [ ] **Step 7: Register in the schema check**

In `tools/checks/validate_sqlite_schema.py`: add to `MIGRATIONS` (after the `0018` entry):
```python
    ROOT / "crates" / "core" / "migrations" / "0019_root_pending_delete.sql",
```
and add to `REQUIRED_INDEXES`:
```python
    "idx_roots_pending_delete",
```

- [ ] **Step 8: Run the schema check + fmt**

Run: `npm run check:schema && cargo fmt -p mangoo-core -- --check`
Expected: PASS (schema check applies all migrations to a temp DB and finds the index).

- [ ] **Step 9: Commit**

```bash
git add crates/core/migrations/0019_root_pending_delete.sql crates/core/src/db/migrations.rs crates/core/src/db/mod.rs tools/checks/validate_sqlite_schema.py
git commit -m "feat(core): migration 0019 roots.pending_delete_at + partial index"
```

---

## Task 2: DB — `mark_root_for_purge` (O(1) start) + `create_root_purge_task`

**Files:**
- Modify: `crates/core/src/db/mod.rs`
- Test: `crates/core/src/db/mod.rs` tests

**Interfaces:**
- Produces:
  ```rust
  pub enum MarkRootForPurge {
      NotFound,
      AlreadyPending,
      Marked { task_id: i64, attempt_generation: i64 },
  }
  pub fn mark_root_for_purge(&self, root_id: i64) -> anyhow::Result<MarkRootForPurge>;
  pub fn roots_pending_delete_ids(&self) -> anyhow::Result<Vec<i64>>;
  ```
- Consumes: existing `cancel_root_scan_tasks_in_transaction`, `current_task_attempt_generation`, `ROOT_SCAN_TASK_PRIORITY` pattern for task insert, `unix_timestamp()`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn mark_root_for_purge_is_o1_and_idempotent() {
    let database = migrated_database();
    let (root_id, folder_id) = seed_media_page_fixture(&database); // 4 files
    // Also seed a pending root_scan task to prove it gets cancelled.
    let scan_task = database.create_root_scan_task(root_id).expect("scan task");

    let files_before: i64 = database
        .connection
        .query_row("SELECT COUNT(*) FROM files WHERE root_id=?1", [root_id], |r| r.get(0))
        .expect("count files");
    assert_eq!(files_before, 4);

    let first = database.mark_root_for_purge(root_id).expect("mark");
    let (task_id, _gen) = match first {
        MarkRootForPurge::Marked { task_id, attempt_generation } => (task_id, attempt_generation),
        other => panic!("expected Marked, got {other:?}"),
    };

    // O(1): no files deleted by marking.
    let files_after: i64 = database
        .connection
        .query_row("SELECT COUNT(*) FROM files WHERE root_id=?1", [root_id], |r| r.get(0))
        .expect("count files");
    assert_eq!(files_after, 4, "marking must not delete files");

    // pending_delete_at is set; a root_purge task exists.
    let pending: Option<i64> = database
        .connection
        .query_row("SELECT pending_delete_at FROM roots WHERE id=?1", [root_id], |r| r.get(0))
        .expect("read pending");
    assert!(pending.is_some());
    let kind: String = database
        .connection
        .query_row("SELECT kind FROM tasks WHERE id=?1", [task_id], |r| r.get(0))
        .expect("task kind");
    assert_eq!(kind, "root_purge");
    // The pre-existing scan task was cancelled.
    let scan_status: String = database
        .connection
        .query_row("SELECT status FROM tasks WHERE id=?1", [scan_task], |r| r.get(0))
        .expect("scan status");
    assert_eq!(scan_status, "failed");

    // Idempotent: a second mark makes no new task.
    assert!(matches!(
        database.mark_root_for_purge(root_id).expect("second mark"),
        MarkRootForPurge::AlreadyPending
    ));

    // Unknown root → NotFound.
    assert!(matches!(
        database.mark_root_for_purge(999_999).expect("missing"),
        MarkRootForPurge::NotFound
    ));

    let ids = database.roots_pending_delete_ids().expect("pending ids");
    assert_eq!(ids, vec![root_id]);
    let _ = folder_id;
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mangoo-core mark_root_for_purge_is_o1_and_idempotent`
Expected: FAIL — `MarkRootForPurge`/`mark_root_for_purge` not defined.

- [ ] **Step 3: Implement**

Add near `create_root_scan_task` in `crates/core/src/db/mod.rs`:
```rust
#[derive(Debug)]
pub enum MarkRootForPurge {
    NotFound,
    AlreadyPending,
    Marked { task_id: i64, attempt_generation: i64 },
}

impl Database {
    /// Begin a hard delete of `root_id`: mark it `pending_delete_at`, cancel its
    /// scan tasks, and enqueue one `root_purge` task — all in one small
    /// transaction. Deletes NO file/folder rows (the background purge does).
    pub fn mark_root_for_purge(&self, root_id: i64) -> anyhow::Result<MarkRootForPurge> {
        let now = unix_timestamp();
        let transaction = self.connection.unchecked_transaction()?;
        let existing: Option<Option<i64>> = transaction
            .query_row(
                "SELECT pending_delete_at FROM roots WHERE id = ?1",
                [root_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?;
        let Some(pending_delete_at) = existing else {
            return Ok(MarkRootForPurge::NotFound);
        };
        if pending_delete_at.is_some() {
            return Ok(MarkRootForPurge::AlreadyPending);
        }
        transaction.execute(
            "UPDATE roots SET pending_delete_at = ?1, enabled = 0 WHERE id = ?2",
            (now, root_id),
        )?;
        cancel_root_scan_tasks_in_transaction(&transaction, root_id)?;
        transaction.execute(
            r#"
            INSERT INTO tasks(kind, priority, status, root_id, file_id, created_at, updated_at, error)
            VALUES ('root_purge', ?1, 'pending', ?2, NULL, ?3, ?3, NULL)
            "#,
            (ROOT_SCAN_TASK_PRIORITY, root_id, now),
        )?;
        let task_id = transaction.last_insert_rowid();
        let attempt_generation: i64 = transaction
            .query_row(
                "SELECT attempt_generation FROM tasks WHERE id = ?1",
                [task_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        transaction.commit()?;
        Ok(MarkRootForPurge::Marked { task_id, attempt_generation })
    }

    /// All roots currently mid-purge (used by startup recovery). Uses the
    /// partial index `idx_roots_pending_delete`.
    pub fn roots_pending_delete_ids(&self) -> anyhow::Result<Vec<i64>> {
        let mut stmt = self
            .connection
            .prepare("SELECT id FROM roots WHERE pending_delete_at IS NOT NULL ORDER BY id ASC")?;
        let ids = stmt
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }
}
```
Note: `tasks` uses `folder_id` only when that column exists; `create_root_scan_task` branches on `table_has_column("tasks","folder_id")`. `folder_id` has existed since migration 14, so the plain insert above (no `folder_id`) is valid because `folder_id` is nullable — verify `INSERT` omitting `folder_id` succeeds (it is nullable). If the migration-14-absent path must be supported, mirror `create_root_scan_task`'s `table_has_column` branch. **Confirm during implementation** by reading `create_root_scan_task`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p mangoo-core mark_root_for_purge_is_o1_and_idempotent`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/db/mod.rs
git commit -m "feat(core): mark_root_for_purge (O(1) start) + roots_pending_delete_ids"
```

---

## Task 3: DB — `purge_root_next_batch` (stateless, bounded, resumable)

**Files:**
- Modify: `crates/core/src/db/mod.rs`
- Test: `crates/core/src/db/mod.rs` tests

**Interfaces:**
- Produces:
  ```rust
  pub const PURGE_BATCH_SIZE: usize = 5000;
  pub struct PurgeBatchOutcome { pub rows_deleted: usize, pub done: bool }
  pub fn purge_root_next_batch(&self, root_id: i64, batch_size: usize) -> anyhow::Result<PurgeBatchOutcome>;
  ```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn purge_root_next_batch_drains_leaf_to_root_in_bounded_batches() {
    let database = migrated_database();
    let (root_id, folder_id) = seed_media_page_fixture(&database); // 4 files + media
    // Seed unrecoverable + regenerable child rows to prove full cascade.
    let a_file: i64 = database
        .connection
        .query_row("SELECT id FROM files WHERE root_id=?1 ORDER BY id LIMIT 1", [root_id], |r| r.get(0))
        .expect("a file");
    database.set_rating(a_file, Some(5)).ok(); // user_metadata (if signature differs, use the real metadata setter)
    // a thumbnail task referencing a file (must be purged, not orphaned).
    database
        .connection
        .execute(
            "INSERT INTO tasks(kind, priority, status, file_id, created_at, updated_at) VALUES ('thumbnail', -10, 'pending', ?1, 1, 1)",
            [a_file],
        )
        .expect("seed thumbnail task");
    database.mark_root_for_purge(root_id).expect("mark");

    // Drain with a tiny batch to force multiple batches.
    let mut batches = 0;
    loop {
        let outcome = database.purge_root_next_batch(root_id, 2).expect("batch");
        assert!(outcome.rows_deleted <= /* tasks batch can delete up to */ 2 + 4, "batch bounded");
        batches += 1;
        if outcome.done { break; }
        assert!(batches < 100, "must terminate");
    }
    assert!(batches >= 2, "small batch size forces multiple batches");

    // Everything for the root is gone.
    for (table, col) in [("files","root_id"),("folders","root_id")] {
        let n: i64 = database.connection
            .query_row(&format!("SELECT COUNT(*) FROM {table} WHERE {col}=?1"), [root_id], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 0, "{table} not drained");
    }
    let root_row: i64 = database.connection
        .query_row("SELECT COUNT(*) FROM roots WHERE id=?1", [root_id], |r| r.get(0))
        .expect("count roots");
    assert_eq!(root_row, 0, "roots row removed");
    // No orphan thumbnail task left with file_id set into the purged root.
    let orphan_tasks: i64 = database.connection
        .query_row("SELECT COUNT(*) FROM tasks WHERE kind='thumbnail' AND file_id IS NOT NULL", [], |r| r.get(0))
        .expect("count tasks");
    assert_eq!(orphan_tasks, 0);
    let _ = folder_id;
}

#[test]
fn purge_root_next_batch_is_resumable_after_interruption() {
    let database = migrated_database();
    let (root_id, _folder_id) = seed_media_page_fixture(&database);
    database.mark_root_for_purge(root_id).expect("mark");
    // One batch only (simulate crash), then resume.
    let first = database.purge_root_next_batch(root_id, 2).expect("first");
    assert!(!first.done);
    loop {
        if database.purge_root_next_batch(root_id, 2).expect("resume").done { break; }
    }
    let n: i64 = database.connection
        .query_row("SELECT COUNT(*) FROM roots WHERE id=?1", [root_id], |r| r.get(0))
        .expect("count");
    assert_eq!(n, 0);
}
```
Note: `set_rating` is illustrative — during implementation use the actual user_metadata setter (find it via `grep -n "fn set_rating\|user_metadata" crates/core/src/db/mod.rs`); if none is convenient, insert directly into `user_metadata`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mangoo-core purge_root_next_batch`
Expected: FAIL — `purge_root_next_batch` not defined.

- [ ] **Step 3: Implement the stateless batch machine**

Add to `crates/core/src/db/mod.rs`:
```rust
pub const PURGE_BATCH_SIZE: usize = 5000;

#[derive(Debug, Clone, Copy)]
pub struct PurgeBatchOutcome {
    pub rows_deleted: usize,
    pub done: bool,
}

impl Database {
    /// Delete ONE bounded batch of a root's rows, leaf-to-root, resumable.
    /// Phases (first non-empty runs this call): scan tasks -> thumbnail tasks
    /// (by file) -> files (cascade to media/user_metadata/file_tags/thumbs/
    /// thumb_blobs) -> folders -> the roots row. Idempotent: keyed by root_id,
    /// safe to re-run after an interruption. `file_operations` is left (its refs
    /// go NULL) as audit history.
    pub fn purge_root_next_batch(
        &self,
        root_id: i64,
        batch_size: usize,
    ) -> anyhow::Result<PurgeBatchOutcome> {
        let batch = batch_size.max(1) as i64;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;

        // Phase 1: this root's scan/interactive tasks. Deliberately EXCLUDES
        // 'root_purge' so the worker's own active task survives for progress
        // updates; it is swept in phase 5 with the roots row.
        let n = transaction.execute(
            r#"
            DELETE FROM tasks
            WHERE id IN (
                SELECT id FROM tasks
                WHERE root_id = ?1 AND kind IN ('root_scan','interactive_folder_scan')
                LIMIT ?2
            )
            "#,
            (root_id, batch),
        )?;
        if n > 0 {
            transaction.commit()?;
            return Ok(PurgeBatchOutcome { rows_deleted: n, done: false });
        }

        // Phase 2: thumbnail tasks referencing this root's files.
        let n = transaction.execute(
            r#"
            DELETE FROM tasks
            WHERE id IN (
                SELECT t.id FROM tasks t
                JOIN files f ON f.id = t.file_id
                WHERE f.root_id = ?1 AND t.kind = 'thumbnail'
                LIMIT ?2
            )
            "#,
            (root_id, batch),
        )?;
        if n > 0 {
            transaction.commit()?;
            return Ok(PurgeBatchOutcome { rows_deleted: n, done: false });
        }

        // Phase 3: files (cascade removes media/user_metadata/file_tags/thumbs/thumb_blobs).
        let n = transaction.execute(
            "DELETE FROM files WHERE id IN (SELECT id FROM files WHERE root_id = ?1 LIMIT ?2)",
            (root_id, batch),
        )?;
        if n > 0 {
            transaction.commit()?;
            return Ok(PurgeBatchOutcome { rows_deleted: n, done: false });
        }

        // Phase 4: folders.
        let n = transaction.execute(
            "DELETE FROM folders WHERE id IN (SELECT id FROM folders WHERE root_id = ?1 LIMIT ?2)",
            (root_id, batch),
        )?;
        if n > 0 {
            transaction.commit()?;
            return Ok(PurgeBatchOutcome { rows_deleted: n, done: false });
        }

        // Phase 5: the root's own remaining tasks (the active root_purge) + the roots row.
        let mut final_rows = 0usize;
        final_rows += transaction.execute("DELETE FROM tasks WHERE root_id = ?1", [root_id])? ;
        final_rows += transaction.execute("DELETE FROM roots WHERE id = ?1", [root_id])?;
        transaction.commit()?;
        Ok(PurgeBatchOutcome { rows_deleted: final_rows, done: true })
    }
}
```
Implementation note: phase 1 deletes `root_purge` tasks too, which would delete the *active* purge task early. To avoid the worker losing its own task mid-run, in phase 1 restrict to `kind IN ('root_scan','interactive_folder_scan')` and delete the `root_purge` task(s) only in phase 5 (with the roots row). Update the phase-1 SQL kind list to exclude `root_purge` before finishing this step; the phase-5 `DELETE FROM tasks WHERE root_id` sweeps the purge task last. (The test asserts the end state, so either is correct end-to-end, but excluding it keeps the worker's task alive for progress updates.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p mangoo-core purge_root_next_batch`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/db/mod.rs
git commit -m "feat(core): purge_root_next_batch — stateless bounded leaf-to-root delete"
```

---

## Task 4: Contract — add `root_purge` task kind

**Files:**
- Modify: `contracts/core-api/openapi.yaml:1127`
- Modify: `packages/core-client/src/generated-contract.ts:102`

**Interfaces:**
- Produces: `TaskKind` includes `root_purge` on both sides.

- [ ] **Step 1: Update OpenAPI**

`contracts/core-api/openapi.yaml`, `TaskKind` enum:
```yaml
      enum: [root_scan, interactive_folder_scan, thumbnail, root_purge]
```

- [ ] **Step 2: Update the generated contract**

`packages/core-client/src/generated-contract.ts`:
```typescript
export type TaskKind = "root_scan" | "interactive_folder_scan" | "thumbnail" | "root_purge";
```

- [ ] **Step 3: Run contract checks**

Run: `npm run check:core-api && npm run check:core-client`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add contracts/core-api/openapi.yaml packages/core-client/src/generated-contract.ts
git commit -m "feat(contract): add root_purge task kind"
```

---

## Task 5: Worker — `root_purge` dispatch branch (loops batches, updates progress)

**Files:**
- Modify: `crates/core/src/tasks.rs` (the `match task.kind.as_str()` at ~line 799)
- Test: `crates/core/src/tasks.rs` tests

**Interfaces:**
- Consumes: `Database::purge_root_next_batch`, `PURGE_BATCH_SIZE`, the writer (`writer.write_blocking`), the task-progress update path used by `root_scan` (find via `grep -n "emit_progress\|update_task_progress\|mark_task_succeeded" crates/core/src/tasks.rs`).
- Produces: task kind `root_purge` handled; task marked `succeeded` when `done`.

- [ ] **Step 1: Write the failing test**

Add a test that runs the worker's purge handler against a seeded root and asserts the root is gone and the task is `succeeded`. Use the existing worker-task test harness (find via `grep -n "fn run_worker_task\|dispatch_worker_task\|process_task" crates/core/src/tasks.rs` and mirror an existing `root_scan` worker test's setup):
```rust
#[test]
fn root_purge_task_drains_root_and_succeeds() {
    // Mirror the existing root_scan worker test setup: build a Database + writer,
    // seed a small root, mark_root_for_purge, then drive the root_purge task
    // through the same entry point the worker uses.
    // Assert: roots row gone; task status 'succeeded'.
}
```
(Replace the body with the concrete harness calls used by the neighboring `root_scan` test — read that test first and copy its Database/writer construction.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mangoo-core root_purge_task_drains_root_and_succeeds`
Expected: FAIL — `unsupported task kind: root_purge`.

- [ ] **Step 3: Implement the dispatch branch**

In `crates/core/src/tasks.rs`, in the `match task.kind.as_str()` block, add a `"root_purge"` arm alongside `"root_scan"`:
```rust
"root_purge" => {
    invoke_worker_task_hook("root_purge", task_id, database.path());
    let root_id = task
        .root_id
        .ok_or_else(|| anyhow::anyhow!("root_purge task {task_id} has no root_id"))?;
    let mut total_deleted: i64 = 0;
    loop {
        // Each batch is one short writer job (via the same writer path the
        // scan worker uses). If no writer (in-memory tests), run inline.
        let outcome = match writer {
            Some(writer) => writer
                .write_blocking(move |db| db.purge_root_next_batch(root_id, PURGE_BATCH_SIZE))?,
            None => database.purge_root_next_batch(root_id, PURGE_BATCH_SIZE)?,
        };
        total_deleted += outcome.rows_deleted as i64;
        // Progress: reuse the scan progress path (items_seen) so the Task
        // Center shows rows cleared. Use the same helper root_scan uses.
        emit_task_items_seen(database, task_id, total_deleted)?; // adapt to the real helper name
        if outcome.done {
            break;
        }
        // Yield so interactive writes interleave between batches.
        check_scan_cancelled(&mut options)?; // reuse the cooperative-yield helper if present, else a short tokio yield
    }
    Ok(())
}
```
Adapt the exact writer handle, progress-update helper, and yield mechanism to match the neighboring `root_scan` arm (read it first). The purge task's own row is deleted by the final batch (phase 5), so mark-succeeded must tolerate the task row already gone — if the worker's mark-succeeded errors on a missing row, guard it for `root_purge` (the successful drain IS success).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p mangoo-core root_purge_task_drains_root_and_succeeds`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/tasks.rs
git commit -m "feat(core): root_purge worker branch (batched drain + progress)"
```

---

## Task 6: Startup recovery — re-enqueue `root_purge` for pending-delete roots

**Files:**
- Modify: `crates/core/src/tasks.rs` (startup recovery near `reset_running_root_scan_tasks_for_recovery`, ~line 225-236)
- Test: `crates/core/src/tasks.rs` or `crates/core/src/db/mod.rs` tests

**Interfaces:**
- Consumes: `Database::roots_pending_delete_ids`, the existing task-enqueue path used at startup.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn startup_recovery_reenqueues_purge_for_pending_delete_roots() {
    let database = migrated_database();
    let (root_id, _f) = seed_media_page_fixture(&database);
    database.mark_root_for_purge(root_id).expect("mark");
    // Simulate a crash that lost the task row but kept pending_delete_at:
    database.connection.execute("DELETE FROM tasks WHERE root_id=?1 AND kind='root_purge'", [root_id]).expect("drop purge task");

    // Run the recovery routine (call the function that lists pending-delete
    // roots and creates purge tasks — factor it into a testable fn).
    let created = database.ensure_pending_delete_purge_tasks().expect("recover");
    assert_eq!(created, 1);
    let n: i64 = database.connection
        .query_row("SELECT COUNT(*) FROM tasks WHERE root_id=?1 AND kind='root_purge' AND status='pending'", [root_id], |r| r.get(0))
        .expect("count");
    assert_eq!(n, 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mangoo-core startup_recovery_reenqueues_purge`
Expected: FAIL — `ensure_pending_delete_purge_tasks` not defined.

- [ ] **Step 3: Implement `ensure_pending_delete_purge_tasks` + wire startup**

Add to `crates/core/src/db/mod.rs`:
```rust
impl Database {
    /// Ensure every `pending_delete_at` root has a pending `root_purge` task.
    /// Called at startup recovery so an interrupted purge resumes. Returns the
    /// number of purge tasks created.
    pub fn ensure_pending_delete_purge_tasks(&self) -> anyhow::Result<usize> {
        let now = unix_timestamp();
        let mut created = 0;
        for root_id in self.roots_pending_delete_ids()? {
            let has_task: bool = self
                .connection
                .query_row(
                    "SELECT 1 FROM tasks WHERE root_id=?1 AND kind='root_purge' AND status IN ('pending','running') LIMIT 1",
                    [root_id],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            if !has_task {
                self.connection.execute(
                    r#"INSERT INTO tasks(kind, priority, status, root_id, file_id, created_at, updated_at, error)
                       VALUES ('root_purge', ?1, 'pending', ?2, NULL, ?3, ?3, NULL)"#,
                    (ROOT_SCAN_TASK_PRIORITY, root_id, now),
                )?;
                created += 1;
            }
        }
        Ok(created)
    }
}
```
Then in `crates/core/src/tasks.rs` startup recovery (right after the existing `reset_running_root_scan_tasks_for_recovery()` / `reset_running_thumbnail_tasks_for_recovery()` calls, ~line 229-236), add:
```rust
database
    .ensure_pending_delete_purge_tasks()
    .expect("re-enqueue root_purge for pending-delete roots at startup recovery");
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p mangoo-core startup_recovery_reenqueues_purge`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/db/mod.rs crates/core/src/tasks.rs
git commit -m "feat(core): resume root_purge for pending-delete roots on startup"
```

---

## Task 7: `remove_root` route rewrite (mark + enqueue)

**Files:**
- Modify: `crates/core/src/api/routes.rs` (`remove_root`, ~line 701) + the `enqueue_task` call already used by `add_root`
- Test: `crates/core/src/api/routes.rs` tests (mirror an existing route test) or Rust integration test

**Interfaces:**
- Consumes: `run_shared_database`, `Database::mark_root_for_purge`, `enqueue_task`.
- Produces: `DELETE /api/roots/{id}` → 404 if missing, else 202; sets `pending_delete_at` + enqueues purge.

- [ ] **Step 1: Write the failing test**

Mirror the nearest existing route test (find via `grep -n "remove_root\|async fn add_root\|#\[tokio::test\]" crates/core/src/api/routes.rs`). Assert: after `DELETE /api/roots/{id}` on a root with files, the root is absent from `GET /api/roots`, its files rows still exist immediately (purge is async), and a `root_purge` task exists.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mangoo-core <remove_root_test_name>`
Expected: FAIL (current `remove_root` calls `disable_root`, no purge task).

- [ ] **Step 3: Rewrite `remove_root`**

```rust
async fn remove_root(
    State(state): State<AppState>,
    Path(root_id): Path<i64>,
) -> ApiResult<(StatusCode, Json<AcceptedResponse>)> {
    let outcome = run_shared_database(state.clone(), move |database| {
        database.mark_root_for_purge(root_id).map_err(CoreError::from)
    })
    .await?;
    match outcome {
        MarkRootForPurge::NotFound => {
            Err(CoreError::not_found(format!("root not found: {root_id}")))
        }
        MarkRootForPurge::AlreadyPending => Ok((StatusCode::ACCEPTED, Json(accepted()))),
        MarkRootForPurge::Marked { task_id, attempt_generation } => {
            enqueue_task(&state, task_id, attempt_generation).await?;
            Ok((StatusCode::ACCEPTED, Json(accepted())))
        }
    }
}
```
Add `use crate::db::MarkRootForPurge;` (or the correct path) at the top of `routes.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p mangoo-core <remove_root_test_name>`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/api/routes.rs
git commit -m "feat(core): remove_root marks pending-delete + enqueues root_purge"
```

---

## Task 8: Race guard — `list_roots` / `get_root` hide pending-delete roots

**Files:**
- Modify: `crates/core/src/db/mod.rs` (`list_roots` ~1050, `get_root` ~1079)
- Test: `crates/core/src/db/mod.rs` tests

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn list_roots_and_get_root_hide_pending_delete_roots() {
    let database = migrated_database();
    let (root_id, _f) = seed_media_page_fixture(&database);
    database.mark_root_for_purge(root_id).expect("mark");
    assert!(database.list_roots().expect("list").is_empty());
    assert!(database.get_root(root_id).expect("get").is_none());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p mangoo-core list_roots_and_get_root_hide_pending_delete_roots`
Expected: FAIL (root still listed — `mark_root_for_purge` sets `enabled=0`, so `WHERE enabled=1` already hides it; this test may PASS already). If it PASSES, keep the test as a guard and skip step 3. If not, apply step 3.

- [ ] **Step 3: Add explicit guard (defensive, independent of enabled)**

In both `list_roots` and `get_root` SQL, change the WHERE to include:
```sql
WHERE roots.enabled = 1 AND roots.pending_delete_at IS NULL
```
(for `get_root`: add `AND roots.pending_delete_at IS NULL` to its `WHERE roots.id = ?1`). This makes the guard explicit even if a future change stops flipping `enabled`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p mangoo-core list_roots_and_get_root_hide_pending_delete_roots`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/db/mod.rs
git commit -m "feat(core): hide pending-delete roots from list_roots/get_root"
```

---

## Task 9: Race guard — scans / sweep / reachability / watcher skip pending-delete roots

**Files:**
- Modify: `crates/core/src/db/mod.rs` (`ensure_root_enabled_in_transaction`)
- Modify: `crates/core/src/api/mod.rs` (`sweep_transient_retries` root enumeration — filter)
- Modify: `crates/core/src/roots/reachability.rs` (probe enumeration)
- Modify: `crates/core/src/watch/mod.rs` (drop events)
- Test: `crates/core/src/db/mod.rs` tests (for `ensure_root_enabled`)

**Interfaces:**
- Consumes: `pending_delete_at IS NOT NULL` predicate.

- [ ] **Step 1: Write the failing test (scan writeback guard)**

```rust
#[test]
fn commit_scan_batch_refuses_pending_delete_root() {
    let mut database = migrated_database();
    let root_id = database.add_root(NewRoot{ path:"D:/X".into(), display_name:"X".into() }).expect("add");
    database.mark_root_for_purge(root_id).expect("mark");
    let result = database.commit_scan_batch(ScanWriteBatch{
        folders: vec![FolderUpsert{ root_id, parent_id:None, name:String::new(), path_hash:"h".into(), mtime:Some(1) }],
        files: Vec::new(),
        scan_generation: None,
    });
    assert!(result.is_err(), "must refuse to write into a pending-delete root");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p mangoo-core commit_scan_batch_refuses_pending_delete_root`
Expected: FAIL (write currently succeeds; `mark_root_for_purge` sets enabled=0 so `ensure_root_enabled` may already reject — if it PASSES, keep as guard).

- [ ] **Step 3: Extend `ensure_root_enabled_in_transaction`**

Find it (`grep -n "fn ensure_root_enabled_in_transaction" crates/core/src/db/mod.rs`) and change its check to reject when `pending_delete_at IS NOT NULL` as well as `enabled = 0`:
```rust
let ok: bool = transaction
    .query_row(
        "SELECT 1 FROM roots WHERE id=?1 AND enabled=1 AND pending_delete_at IS NULL",
        [root_id],
        |_| Ok(true),
    )
    .optional()?
    .unwrap_or(false);
if !ok {
    anyhow::bail!("root {root_id} is disabled or being removed");
}
```

- [ ] **Step 4: Filter sweep / reachability / watcher**

- `crates/core/src/api/mod.rs::sweep_transient_retries`: the read-conn `list_roots()` already excludes pending-delete after Task 8; if it enumerates a different query, add `AND pending_delete_at IS NULL`. Verify by reading the enumeration.
- `crates/core/src/roots/reachability.rs`: where it lists roots to probe, exclude `pending_delete_at IS NOT NULL` (read the enumeration; add the predicate).
- `crates/core/src/watch/mod.rs`: before the writer upserts for an event, look up the file/root; if the root is pending-delete, drop the event. Simplest: the writer upsert path already calls `ensure_root_enabled` (now rejecting pending-delete) — verify the watcher swallows that error rather than crashing (it already `let _ = state.writer.write_blocking(...)` per §current code). Confirm and, if needed, add an explicit skip.

- [ ] **Step 5: Run tests + build**

Run: `cargo test -p mangoo-core commit_scan_batch_refuses_pending_delete_root && cargo build -p mangoo-core`
Expected: PASS + builds.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/db/mod.rs crates/core/src/api/mod.rs crates/core/src/roots/reachability.rs crates/core/src/watch/mod.rs
git commit -m "feat(core): scans/sweep/reachability/watcher skip pending-delete roots"
```

---

## Task 10: `add_root` returns 409 for a pending-delete path; retire soft-delete

**Files:**
- Modify: `crates/core/src/db/mod.rs` (`add_root` — detect pending-delete existing row)
- Modify: `crates/core/src/api/routes.rs` (`add_root` maps the signal to 409)
- Modify: `crates/core/src/db/mod.rs` (remove now-unused `disable_root`; keep `cleanup_inactive_thumbnail_artifacts_in_transaction` — still used by `reconcile_root_scan_completion`)
- Test: `crates/core/src/db/mod.rs` tests

**Interfaces:**
- Produces: adding a path whose root is `pending_delete_at` → a distinct error the route renders as 409.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn add_root_rejects_path_being_purged() {
    let database = migrated_database();
    let root_id = database.add_root(NewRoot{ path:"D:/Y".into(), display_name:"Y".into() }).expect("add");
    database.mark_root_for_purge(root_id).expect("mark");
    let err = database.add_root(NewRoot{ path:"D:/Y".into(), display_name:"Y".into() });
    assert!(err.is_err(), "must reject re-adding a path mid-purge");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p mangoo-core add_root_rejects_path_being_purged`
Expected: FAIL — `add_root` currently re-enables the row.

- [ ] **Step 3: Implement**

In `add_root`, after the `SELECT id, enabled FROM roots WHERE path=?1` also read `pending_delete_at`; if it is non-NULL, return a typed error, e.g. `anyhow::bail!("root path is being removed: {}", root.path)`. In `routes.rs::add_root`, map that error to `CoreError::conflict` (409) — add a `conflict` constructor to `CoreError` if absent (mirror `not_found`/`bad_request`), and detect the message (or thread a typed error). Simplest robust approach: give `add_root` a typed result enum like `mark_root_for_purge` (e.g. return `Err` with a `CoreError::conflict` directly if the DB layer can't). During implementation, prefer a small enum `AddRootOutcome { Added(i64), BeingRemoved }` to avoid string matching.

Also: since removal no longer produces `enabled=0`, the `if !enabled { ... }` re-enable branch in `add_root` (already stripped of its DELETE by the earlier fix) is now only reachable in legacy DBs; leave it (harmless) but remove the now-unused `disable_root` fn and its tests. Verify `disable_root` has no remaining callers (`grep -rn "disable_root" crates/core/src`), and that `cleanup_inactive_thumbnail_artifacts_in_transaction` is still called by `reconcile_root_scan_completion` (keep it).

- [ ] **Step 4: Run tests + grep for dead callers**

Run: `cargo test -p mangoo-core add_root_rejects_path_being_purged && grep -rn "disable_root" crates/core/src`
Expected: test PASS; `disable_root` has no non-test callers (remove it + its tests).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/db/mod.rs crates/core/src/api/routes.rs
git commit -m "feat(core): reject add_root of a path mid-purge (409); retire disable_root"
```

---

## Task 11: Frontend — two-step confirm dialog + optimistic removeRoot

**Files:**
- Modify: `apps/web/src/features/library/LibrarySwitcher.tsx`
- Modify: `apps/web/src/core/useLibraryData.ts` (`removeRoot` ~2507)
- Create: `apps/web/src/core/removeRootConfirm.ts` (pure copy strings — testable)
- Test: `apps/web/tests/removeRootConfirm.test.mts`

**Interfaces:**
- Produces: confirm dialog before `removeRoot`; sidebar drops the root immediately (no `loadLibrary` full reload).

- [ ] **Step 1: Write the failing test (pure confirm copy)**

Create `apps/web/tests/removeRootConfirm.test.mts`:
```typescript
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { REMOVE_ROOT_CONFIRM } from "../src/core/removeRootConfirm.ts";

describe("REMOVE_ROOT_CONFIRM", () => {
  it("names the permanently-lost data types and reassures about disk files", () => {
    assert.match(REMOVE_ROOT_CONFIRM.body, /评分/);
    assert.match(REMOVE_ROOT_CONFIRM.body, /标签/);
    assert.match(REMOVE_ROOT_CONFIRM.body, /备注/);
    assert.match(REMOVE_ROOT_CONFIRM.body, /永久丢失|无法恢复/);
    assert.match(REMOVE_ROOT_CONFIRM.body, /磁盘.*不受影响|原文件不受影响/);
    assert.equal(REMOVE_ROOT_CONFIRM.confirmLabel, "移除");
    assert.equal(REMOVE_ROOT_CONFIRM.cancelLabel, "取消");
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `node --experimental-strip-types --test apps/web/tests/removeRootConfirm.test.mts`
Expected: FAIL — module missing.

- [ ] **Step 3: Implement the copy module**

Create `apps/web/src/core/removeRootConfirm.ts`:
```typescript
// Static confirmation copy for hard-removing a folder (root) from the library.
// Only names the UNRECOVERABLE data (ratings/tags/notes); index + thumbnails are
// rebuildable by re-adding, and disk files are never touched.
export const REMOVE_ROOT_CONFIRM = {
  title: (name: string) => `从库移除「${name}」?`,
  body:
    "该文件夹的索引和缩略图将被删除(重新添加可重建)。\n" +
    "⚠️ 评分、标签、备注将永久丢失,无法恢复。\n" +
    "磁盘上的原文件不受影响。",
  confirmLabel: "移除",
  cancelLabel: "取消"
} as const;
```

- [ ] **Step 4: Run to verify it passes**

Run: `node --experimental-strip-types --test apps/web/tests/removeRootConfirm.test.mts`
Expected: PASS.

- [ ] **Step 5: Wire the dialog + optimistic drop**

In `LibrarySwitcher.tsx::onRemoveRoot`, instead of calling `library.removeRoot` directly, open a confirm dialog using `REMOVE_ROOT_CONFIRM` (a small local `useState` for the pending root id + a modal in the render, matching the popup's existing styling). On confirm → `await library.removeRoot(rootId)` → close.

In `useLibraryData.ts::removeRoot`, replace the `await loadLibrary({rootId:null, folderId:null})` full reload with an optimistic drop + lightweight refresh:
```typescript
const removeRoot = useCallback(
  async (rootId: number) => {
    setError(null);
    // Optimistic: drop the root from the sidebar immediately.
    setRoots((current) => current.filter((r) => r.id !== rootId));
    if (selectedRootIdRef.current === rootId) {
      selectRoot(null); // or the app's "clear selection" path
    }
    try {
      await client.removeRoot(rootId);
      // Purge runs in the background; refresh the lightweight roots list so any
      // server-side correction (e.g. AlreadyPending) reconciles.
      await refreshRootsList();
    } catch (cause) {
      setError(errorMessage(cause));
      await refreshRootsList(); // resync on failure
    }
  },
  [client, refreshRootsList]
);
```
Adapt `selectRoot(null)` to the actual selection-clear function in the hook.

- [ ] **Step 6: Typecheck + web tests + boundary check**

Run: `npm --workspace @mangoo/web run typecheck && npm --workspace @mangoo/web run test && npm run check:web`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add apps/web/src/features/library/LibrarySwitcher.tsx apps/web/src/core/useLibraryData.ts apps/web/src/core/removeRootConfirm.ts apps/web/tests/removeRootConfirm.test.mts
git commit -m "feat(web): two-step confirm + optimistic removeRoot"
```

---

## Task 12: Full verification + real-app run

**Files:** none (verification only)

- [ ] **Step 1: Full test gate**

Run: `npm test`
Expected: structure, contracts, schema, web, typecheck, and `cargo test --workspace` all PASS.

- [ ] **Step 2: Real-app debug run**

Run: `MANGOO_DEBUG=1 MANGOO_REMOTE_DEBUG=1 npm run dev` (logs to a file). In the app, remove the 652k-file root.
Observe / assert:
- Root disappears from the sidebar immediately; confirm dialog showed the data-loss warning.
- Core log: `writer running a job` / `writer job closure returned in <Nms>` for purge batches — **no long job**, WAL stays bounded (`wal now <=~64MB`).
- DB (separate sqlite connection): `SELECT COUNT(*) FROM files WHERE root_id=<id>` decreases to 0; `SELECT COUNT(*) FROM roots WHERE id=<id>` becomes 0; Task Center shows a `root_purge` task with climbing progress then done.
- **Recovery:** kill the app mid-purge, restart → the root_purge resumes (files count keeps dropping) and finishes; the roots row is gone.
- Adding the same path mid-purge → 409 / "正在移除中" message; after purge completes, adding it works as a fresh scan.

- [ ] **Step 3: Commit any verification-driven fixes**

If verification surfaces a defect, return to the owning task, fix with a failing-test-first cycle, and commit.

---

## Notes for the implementer

- Read the neighboring `root_scan` worker arm and its tests BEFORE Task 5 — the writer handle name, the progress helper, and the cooperative-yield helper must match exactly.
- `mark_root_for_purge` sets `enabled = 0` in addition to `pending_delete_at`; this makes the existing `WHERE enabled = 1` guards hide the root even before Task 8's explicit predicate lands. Task 8's explicit `pending_delete_at IS NULL` is defensive and future-proofs against dropping the `enabled` flip.
- Purge is idempotent and keyed by `root_id`; a failed batch leaves `pending_delete_at` set so startup recovery retries — never a half-deleted stuck state.
- Do NOT touch disk files anywhere in this plan. `file_operations` audit rows are intentionally kept (refs go NULL).
