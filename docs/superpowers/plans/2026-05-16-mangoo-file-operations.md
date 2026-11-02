# Mangoo File Operations Plan

> **For agentic workers:** Implement task-by-task. Steps use `- [ ]` checkboxes.

**Goal:** Real rename, move, and recycle-bin delete for files and folders, with conflict handling, operation logs, and UI recovery flows. Mangoo maps real directories — these operations are real.

**Architecture:** Add a Rust `fsops` module that performs validated filesystem operations + atomic DB updates inside a single transaction; expose via `/file-ops/*` endpoints; persist every operation to `file_operations`; surface in UI with confirmation dialogs and a recent-operations panel.

**Tech Stack:** Rust + rusqlite + std::fs + `trash` crate for recycle bin (Windows). React + TanStack Query.

**Scope cuts (deferred):**
- Cross-volume copy+verify+delete fallback (out — Phase 7+; same-volume rename only for now, error 409 if cross-volume).
- USN journal integration (out — release hardening).
- Bulk operation batching with progress (basic per-file feedback only; bulk progress is out).
- Undo (out — operation log only; undo is post-Phase 10).

---

## File Structure

**Modify (backend):**

- `D:/Mangoo/contracts/core-api/openapi.yaml`
- `D:/Mangoo/packages/core-client/src/generated-contract.ts`
- `D:/Mangoo/packages/core-client/src/client.ts`
- `D:/Mangoo/crates/core/Cargo.toml` (add `trash = "5"`)
- `D:/Mangoo/crates/core/src/fsops/mod.rs` (currently nearly empty)
- `D:/Mangoo/crates/core/src/db/mod.rs` (helpers for file_operations log + atomic rename/move/delete)
- `D:/Mangoo/crates/core/src/api/mod.rs`, `routes.rs`
- `D:/Mangoo/tools/checks/validate-core-api.mjs`

**Create (backend):**

- `D:/Mangoo/crates/core/migrations/0011_file_operations_columns.sql` only if `file_operations` table needs new columns (it currently has `id, operation, source_path, target_path, status, created_at, finished_at, error` — likely sufficient).

**Modify (frontend):**

- `D:/Mangoo/apps/web/src/core/useLibraryData.ts` (mutations + recent-ops query)
- `D:/Mangoo/apps/web/src/app/App.tsx` (context menus, confirmation dialogs, recent-ops drawer)
- `D:/Mangoo/apps/web/src/styles.css`

**Create (frontend):**

- `D:/Mangoo/apps/web/src/features/file-ops/RenameDialog.tsx`
- `D:/Mangoo/apps/web/src/features/file-ops/MoveDialog.tsx`
- `D:/Mangoo/apps/web/src/features/file-ops/DeleteConfirm.tsx`
- `D:/Mangoo/apps/web/src/features/file-ops/RecentOpsPanel.tsx`

---

## Contract surface (frozen)

- `POST /file-ops/rename` body `{ fileId?: number, folderId?: number, newName: string }` → `200 FileOperationRecord` / `400` (illegal name, contains `/\\`, length 0 or >255) / `404` / `409` (target exists).
- `POST /file-ops/move` body `{ fileIds?: number[], folderIds?: number[], targetFolderId: number }` → `200 { operations: FileOperationRecord[] }` / `400` / `404` / `409` (cross-volume — server detects, returns code `cross_volume`; or target conflict).
- `POST /file-ops/delete` body `{ fileIds?: number[], folderIds?: number[], permanent?: boolean }` (default false → recycle bin) → `200 { operations: FileOperationRecord[] }` / `400` (permanent without explicit confirmation flag missing — frontend always sends explicit) / `404`.
- `GET /file-ops` query `?limit=50&cursor=<id>` → `Page<FileOperationRecord>`.

DTOs:
```ts
export type FileOperationKind = "rename" | "move" | "delete_recycle" | "delete_permanent";
export type FileOperationStatus = "succeeded" | "failed";
export interface FileOperationRecord {
  id: number;
  operation: FileOperationKind;
  sourcePath: string;
  targetPath: string | null;
  status: FileOperationStatus;
  createdAt: number;
  finishedAt: number | null;
  error: string | null;
}
```

Operations are synchronous in the request: the handler does the FS work + DB update + log row inside a single transactional flow, then returns the log entry. No background task for now.

---

## Task 1 — Backend `fsops` module

- [ ] Add `trash = "5"` to `crates/core/Cargo.toml`. Add `path-clean` if needed (or keep using `Path::canonicalize`).
- [ ] Build `fsops::rename_file(db, file_id, new_name)`:
  - Validate `new_name` (non-empty, no path separators, no traversal, ≤255 chars).
  - Compute target path = `parent_dir / new_name`.
  - 409 if target file exists on disk OR `(folder_id, new_name)` already in DB as active.
  - `BEGIN IMMEDIATE`, `std::fs::rename(source, target)`, update `files.name` row, INSERT into `file_operations`, COMMIT. On FS error, rollback DB and return 500/400 as appropriate.
- [ ] `fsops::rename_folder(db, folder_id, new_name)`: same shape; updates `folders.name`. Children paths are derived from folder chain — no row updates needed under DB-derived paths, but watcher will pick up rename events; ensure idempotency.
- [ ] `fsops::move_files(db, ids, target_folder_id)`:
  - Reject if target folder is in a different `root_id` (different volume risk) — return `cross_volume` for now.
  - Reject if any source/target name collision in DB or FS.
  - For each file: `std::fs::rename(src, dst)`, update `files.folder_id`, log.
  - Best-effort transactional: collect failures and continue; final response carries per-item results.
- [ ] `fsops::delete(db, file_ids, folder_ids, permanent)`:
  - Default to recycle bin via `trash::delete`. Set operation kind `delete_recycle`.
  - If `permanent`: actual `std::fs::remove_file` / `remove_dir_all` with extra guard: must be inside an enabled root.
  - Mark DB rows `status='deleted'` or remove rows; pick the lowest-blast-radius option (status flag is safer — keep history).
- [ ] `fsops::list_recent_operations(db, limit, cursor)` keyset paginated by `id DESC`.
- [ ] Tests: rename success, rename collision (409), rename invalid name (400), move success, move cross-volume (409), recycle delete (mock trash via feature flag or skip integration on CI when no recycle bin), permanent delete success, log entries written for each.

## Task 2 — API routes

- [ ] Add OpenAPI paths and DTOs.
- [ ] Add Rust handlers, wire under existing session middleware.
- [ ] Update `validate-core-api.mjs` to require new path fragments and DTO names, plus `FileOperationKind` enum values.
- [ ] Add `packages/core-client/src/generated-contract.ts` types and method stubs in `client.ts`.
- [ ] Integration tests at the route layer: 200/400/404/409 for each.

## Task 3 — Frontend

- [ ] Mutations in `useLibraryData.ts`: `renameFile`, `renameFolder`, `moveItems`, `deleteItems` (recycle | permanent), `recentOps` query.
- [ ] `RenameDialog`: input + live validation (illegal chars, target conflict from a quick lookup).
- [ ] `MoveDialog`: folder tree picker (reuse existing folder tree component).
- [ ] `DeleteConfirm`: two paths — recycle bin (single click) and permanent (requires checkbox + matching item count display).
- [ ] `RecentOpsPanel`: list of last 50 operations with timestamps, source→target, status pill, error tooltip on failures.
- [ ] Context menu on grid items + folder tree: Rename / Move / Delete-to-recycle / Delete-permanent (hold Shift to reveal permanent).
- [ ] Optimistic UI with rollback. After success, refresh affected folder + recent-ops.
- [ ] No raw fetch, no node:fs.

## Task 4 — Verification + commit

- [ ] `npm test`, all green.
- [ ] Update `CLAUDE.md` snippet, master plan checkboxes.
- [ ] Commit `feat: add real file operations (phase 6)`.

---

## Acceptance

- User right-clicks an image → Rename → enters new name → file is renamed on disk, in grid, and Recent Ops shows the row.
- User selects 5 files → Move → picks a folder under the same root → succeeds; cross-volume attempt returns clean 409.
- User deletes a folder → confirms recycle bin → folder appears in Windows Recycle Bin; DB rows marked deleted; UI shows them gone.
- Permanent delete requires explicit checkbox + item count match; succeeds.
- Watcher reconciles any leftover state automatically.
