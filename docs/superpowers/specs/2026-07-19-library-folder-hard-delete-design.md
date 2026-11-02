# Design: Hard-delete a folder (root) from the library

Date: 2026-07-19
Status: Approved (brainstorming) — pending implementation plan

## 1. Context & problem

"从库中删除文件夹" removes a top-level mapped folder (a `roots` row) from the
active library. The current flow is a **soft delete** (`DELETE /api/roots/{id}`
→ `disable_root`):

- `roots.enabled = 0` (row kept, hidden from `list_roots WHERE enabled = 1`)
- `files` / `folders` index rows **kept**
- `thumbs` + `thumb_blobs` for the root's files **deleted** (inside the same
  transaction, on the single writer)
- pending/running scan tasks marked `failed`
- No confirmation dialog; the frontend `loadLibrary({rootId:null})` full-reloads

Problems this design fixes:

1. **Soft delete is not what the product wants.** Removal should be a real,
   complete removal (decided: hard delete only — no user-visible disabled state).
2. **Orphan rows accumulate.** Soft-deleted roots leave `files`/`folders` rows
   forever; the DB only grows.
3. **Freeze hazard.** A synchronous multi-table delete of a large root (e.g. the
   652k-file NAS root) on the single `DbWriter` would block every other write
   for minutes — the same class of freeze already fixed for `add_root`
   (`crates/core/src/db/mod.rs` re-enable path) and the thumbnail enqueue.

## 2. Goals & non-goals

**Goals**
- Removal is a **hard delete**: purge all of the root's rows from the DB.
- **Never freeze the single writer**, even for a 600k-file root → background,
  bounded-batch purge; the root disappears from the UI immediately.
- **Progress visible** in the Task Center.
- **Durable recovery**: a purge interrupted by app close / Core restart resumes
  and completes on the next start.
- **No cross-subsystem races**: nothing scans, enqueues thumbnails for, probes,
  or lists a root that is being deleted.
- **Two-step confirmation** naming the permanently-lost data types.

**Non-goals**
- **Disk files are never touched.** Mangoo maps folders; it does not own them.
  "Remove from library" ≠ "delete files" (that is the separate recycle-bin
  fsops flow). Not in scope.
- No undo/restore of a hard delete.
- `file_operations` audit history is **kept** (its refs go NULL); "彻底移除"
  covers media index + user metadata + thumbnails + tasks, not the audit log.

## 3. State model & lifecycle

New migration **`0019_root_pending_delete.sql`**:

```sql
ALTER TABLE roots ADD COLUMN pending_delete_at INTEGER;  -- NULL = normal; non-NULL = being purged
CREATE INDEX IF NOT EXISTS idx_roots_pending_delete
  ON roots(id) WHERE pending_delete_at IS NOT NULL;       -- partial: cheap startup recovery scan
```

A root now has three states:

- **active** — `pending_delete_at IS NULL` (and `enabled = 1`): shown normally.
- **deleting** — `pending_delete_at` set: vanishes from the sidebar/lists
  immediately; the background worker is draining its rows.
- **gone** — all rows deleted, including the `roots` row itself.

`pending_delete_at` is the **authoritative source of truth** for "this root is
being removed" (see recovery, §5). The legacy `enabled = 0` soft-delete state is
**retired** — removal is always a hard delete.

`remove_root` becomes an **O(1) small transaction** (`DELETE /api/roots/{id}`):

1. `UPDATE roots SET pending_delete_at = <now> WHERE id = ? AND pending_delete_at IS NULL`
2. cancel the root's pending/running scan tasks (reuse
   `cancel_root_scan_tasks_in_transaction`)
3. create one `root_purge` task (new task kind)
4. commit → `202 Accepted`; the sidebar drops the folder at once.

If the root does not exist → 404. If it is already `deleting` → idempotent 202
(no new purge task needed; recovery/worker already owns it).

## 4. What gets purged, order, and batching

`foreign_keys = ON` for file-backed connections
(`configure_file_database_wal`), so `ON DELETE CASCADE` fires. But a single
`DELETE FROM roots` would cascade hundreds of thousands of rows in one statement
— exactly the freeze we avoid. The purge deletes **explicitly, leaf-to-root, in
bounded batches** (default `PURGE_BATCH_SIZE = 5000`, tuned so one batch is a
~50ms writer job).

| Step | Operation | Notes |
|---|---|---|
| 1 | delete the root's **tasks** (batched) | `thumbnail` tasks by `file_id IN (files of root)`, `root_scan`/`interactive_folder_scan` by `root_id`. Done **first**, while `file_id` still resolves — otherwise `ON DELETE SET NULL` turns them into `file_id = NULL` orphans. |
| 2 | delete **files** (batched) | `DELETE FROM files WHERE id IN (SELECT id FROM files WHERE root_id = ? LIMIT N)`; each batch cascades that batch's `media` / `user_metadata` / `file_tags` / `thumbs` / `thumb_blobs`. Loop until none remain. |
| 3 | delete **folders** (batched) | after files are gone; folder self-ref `parent_id` cascade makes id-batched deletion safe. |
| 4 | delete the **roots** row | final O(1) statement; `pending_delete_at` record disappears → root is `gone`. |

Details:
- **No `DELETE ... LIMIT`** (rusqlite's bundled SQLite lacks
  `SQLITE_ENABLE_UPDATE_DELETE_LIMIT`); always batch via
  `WHERE id IN (SELECT ... LIMIT N)`.
- **`file_operations`** (`root_id`/`file_id` are `ON DELETE SET NULL`) is
  **kept** as audit history; its refs go NULL when files/roots are deleted.
- **Idempotent**: every step is keyed by `root_id` (or a derived subquery), so
  re-running after an interruption simply continues clearing whatever remains.

## 5. Purge worker, writer batching, durable recovery

New `root_purge` task handler (a new branch in the existing background task
worker; implementation module e.g. `crates/core/src/purge/mod.rs` or a `tasks.rs`
branch), reusing the existing worker + single `DbWriter`:

- Each batch is **one `writer.write_blocking(...)` short job** (≤ `N` rows).
  Between batches the loop yields, so the single writer interleaves add_root,
  priority-scope, thumbnail publishes, etc. — **never a long freeze**.
- After each batch, update task progress (`items_seen += rows_deleted`); the Task
  Center shows "正在清理 X / 已删 N 行".
- The `DbWriter` runs its normal PASSIVE checkpoint after draining each batch →
  **WAL stays bounded** (unlike a synchronous mega-delete).

**Durable recovery (interrupted purge):**
- `pending_delete_at` is persistent and **authoritative**. Startup recovery
  (same place stale `root_scan` tasks are reset) adds one step: scan roots
  `WHERE pending_delete_at IS NOT NULL` (via the partial index) and **re-enqueue
  a `root_purge`** for each.
- Because deletion is idempotent, an interruption at any batch just resumes.
- Only the final step (deleting the `roots` row) clears `pending_delete_at`, so
  a root is never "half-deleted and forgotten".

**Failure handling:** a failed batch (e.g. transient `database is locked`) marks
the `root_purge` task failed, but `pending_delete_at` stays set → next startup
recovery retries. No half-deleted stuck state.

## 6. Race handling (no writes into a deleting root)

Once a root is `deleting`, every path that would write to or surface it must
skip it. Single source of truth: `pending_delete_at IS NOT NULL`.

| Subsystem | Change |
|---|---|
| `list_roots` / `get_root` | add `AND pending_delete_at IS NULL` |
| scan worker | a picked-up `root_scan` for a deleting root is dropped; `commit_scan_batch`'s `ensure_root_enabled` check is extended to "enabled AND not deleting" so a scan can't write back into a purging root |
| thumbnail transient-retry sweep | skip deleting roots when enumerating roots |
| browse enqueue / priority-scope | scope hitting a deleting root is a no-op (the root is already gone from the UI; server-side guard as backstop) |
| reachability probe | skip deleting roots |
| watcher | drop filesystem events for a deleting root (no upsert) |

**Core invariant:** after entering `deleting`, **no new row is ever written into
the root**; the purge worker is the only writer still touching it, monotonically
draining to zero. This eliminates "delete vs. thumbnail-ingest / scan-writeback"
contention.

**Re-add of the same path mid-purge:** `add_root` matches an existing row by the
`path` UNIQUE key. If the matched root has `pending_delete_at` set → **reject**
with a clear error (409, e.g. "该文件夹正在移除中,请稍候再添加"). After the purge
deletes the `roots` row, adding the path again is a fresh `INSERT`. This avoids
resurrecting a root that is being deleted.

**Legacy soft-delete retirement:** with `enabled = 0` no longer produced by
removal, `disable_root` and `cleanup_inactive_thumbnail_artifacts_in_transaction`
(its old callers) are removed or repointed at the new flow. The add_root
re-enable branch (`existing && !enabled`) becomes effectively unreachable and is
simplified. The implementation plan enumerates every affected call site to avoid
dead code or dual semantics. (Note: `cleanup_inactive_thumbnail_artifacts_*` may
still be referenced by other flows — the plan verifies before removing.)

## 7. UI & confirmation

- The `LibrarySwitcher` root remove button opens a **two-step confirmation
  dialog** (replacing today's immediate delete).
- **Static warning naming the permanently-lost data types** — no counts, no
  impact pre-query, no conditional variants:

  > **从库移除「X」?**
  > 该文件夹的索引和缩略图将被删除(重新添加可重建)。
  > ⚠️ **评分、标签、备注将永久丢失,无法恢复。**
  > 磁盘上的原文件不受影响。
  > [取消] [移除]

- On confirm → `removeRoot` → the root is **optimistically dropped from the
  sidebar immediately**; cleanup progress appears in the Task Center. The
  frontend removes just that entry from the roots list rather than
  `loadLibrary()` full-reloading (avoids the flash).

## 8. Testing strategy (TDD — failing test first)

**Rust / Core**
- `remove_root` is O(1): sets `pending_delete_at` + creates `root_purge` +
  cancels scan tasks, and **deletes no `files` rows** (assert file count
  unchanged, root gone from `list_roots`).
- Purge correctness: seed a root with rows across
  files/folders/media/user_metadata/file_tags/thumbs/thumb_blobs, run purge to
  completion → all those tables have zero rows for the root, `roots` row gone;
  `file_operations` kept with NULL refs.
- Bounded batches: seed more than one batch (batch=2, seed 5) → multiple batches,
  each ≤ batch size.
- Idempotent / recovery: interrupt mid-purge, run again → fully drained
  (simulates restart resume).
- Recovery enqueue: a root with `pending_delete_at` set gets a `root_purge`
  re-enqueued on startup recovery.
- Race skipping: a deleting root is absent from `list_roots`; scan / sweep /
  thumbnail enqueue are no-ops for it; `add_root` on the same path returns 409.
- Contract/schema: migration `0019` added to `tools/checks/validate_sqlite_schema.py`
  (column + partial index); new task kind `root_purge` added to the contract.

**Frontend / Web** (extract pure logic, unit-tested per this repo's pattern)
- Confirmation dialog show/cancel/confirm branches.
- After `removeRoot`, the root is dropped from the roots list (no full reload).

**Real-app verification** (debug mode)
- Remove the 652k-file root → root vanishes immediately, writer never freezes
  (no long jobs), WAL bounded, Task Center progress climbs, `roots` row gone at
  the end; kill + restart mid-purge → recovery resumes and finishes.

## 9. Edge cases / open items

- Removing a root mid-scan: step 1 cancels its scan tasks; the scan worker's
  deleting-root guard prevents further writeback (§6).
- Removing the last root / active-library implications: `remove_root` only acts
  on the active library's roots; multi-library switching is unaffected.
- `PURGE_BATCH_SIZE` exact value tuned during implementation to keep a batch's
  writer job ≈ 50ms on the `thumb_blobs` (BLOB) deletes.
