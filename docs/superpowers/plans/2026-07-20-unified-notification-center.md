# Unified Notification Center Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the separate Tasks and Recent UI with a durable, cross-library notification center that shows aggregated task progress, important results, and current-process logs without slowing media browsing.

**Architecture:** Core stores one durable logical-operation head and an append-only Outbox in each library database. Electron Desktop drains Outbox changes into a global SQLite database owned by a worker thread, exposes typed snapshot/query subscriptions through preload, and keeps a bounded in-memory process-log ring. React renders one titlebar notification button, a compact read-only status popover, and a Settings/Plugins-style standalone notification window.

**Tech Stack:** Rust 2021, Axum 0.7, rusqlite 0.32, SQLite WAL, OpenAPI, TypeScript 5.6, Electron 43, Node 24 `node:sqlite`, worker_threads, React 18, Zustand, TanStack Virtual, Liquid Glass.

---

## Execution Constraints

- Execute in an isolated worktree created with `superpowers:using-git-worktrees`.
- The source worktree currently contains an unrelated `Cargo.toml` modification and six untracked `tools/dev` probes. Do not stage, copy, revert, or delete them.
- Follow contract-first order: OpenAPI, generated Core client types, Core implementation, Desktop bridge, then Web.
- Do not add raw Core `fetch` calls in `apps/web`; Desktop Main may call Core over authenticated HTTP.
- Do not introduce `better-sqlite3` or another native addon. Electron 43 embeds Node 24 and provides stable `node:sqlite`.
- Keep all notification SQLite work in a worker thread. Do not run `DatabaseSync` queries on Electron Main.
- Never emit an Outbox row per thumbnail item.
- Use one focused commit per task and run the task's narrow verification before committing.

## File Map

### Core and contract

- Create `crates/core/migrations/0021_notification_outbox.sql`: operation heads, Outbox, file-operation grouping, and indexes.
- Create `crates/core/src/db/notification_outbox.rs`: logical-operation DTOs plus transactional head/Outbox helpers.
- Modify `crates/core/src/db/mod.rs`: migration registration, shared Outbox wake signal, task lifecycle hooks, root-purge totals, and public Outbox methods.
- Modify `crates/core/src/db/migrations.rs`: include migration 0021.
- Modify `crates/core/src/tasks.rs`: root-purge determinate progress and terminal publication before task deletion.
- Modify `crates/core/src/fsops/mod.rs`: one group ID per rename/move/delete command and terminal group publication.
- Modify `crates/core/src/api/mod.rs`: expose the shared Outbox wake signal and recover dirty heads.
- Modify `crates/core/src/api/routes.rs`: long-poll read and acknowledgement routes.
- Modify `contracts/core-api/openapi.yaml`: Outbox schemas and endpoints.
- Modify `packages/core-client/src/generated-contract.ts`, `client.ts`, and `index.ts`: typed Outbox contract.
- Modify `packages/core-client/tests/client.test.mts`: request serialization tests.
- Modify `tools/checks/validate_sqlite_schema.py`: migration 0021 schema assertions.

### Desktop

- Create `apps/desktop/src/notification-types.ts`: Desktop-owned global notification, query, sync-health, thumbnail-summary, and log types.
- Create `apps/desktop/src/notification-store-worker.ts`: global SQLite schema and worker RPC implementation.
- Create `apps/desktop/src/notification-store.ts`: asynchronous worker facade.
- Create `apps/desktop/src/notification-service.ts`: Outbox drain loop, legacy import, retention, actions, and renderer broadcast state.
- Create `apps/desktop/src/process-log-buffer.ts`: bounded current-session log ring.
- Create `apps/desktop/src/desktop-logger.ts`: shared Main/Core/Renderer log sink.
- Create focused tests under `apps/desktop/tests/` for the store, service, and log buffer.
- Modify `apps/desktop/package.json`: suppress only Node 22's known `node:sqlite`
  experimental warning in the test runner; Electron 43 runs Node 24.
- Modify `apps/desktop/src/core-process.ts`: forward Core output into the log sink.
- Modify `apps/desktop/src/main.ts`: service lifecycle, IPC, and notification secondary-window role.
- Modify `apps/desktop/src/preload.cjs`: notification and log bridge methods/subscriptions.

### Web

- Create `apps/web/src/features/notifications/notificationModel.ts`: pure compact/history/progress selectors.
- Create `apps/web/src/features/notifications/useNotifications.ts`: snapshot and IPC subscription state.
- Create `apps/web/src/features/notifications/NotificationPopover.tsx`: compact read-only status UI.
- Create `apps/web/src/features/notifications/NotificationWindow.tsx`: standalone frame and page navigation.
- Create `apps/web/src/features/notifications/NotificationStatusView.tsx`: active work, thumbnail summary, and recent important results.
- Create `apps/web/src/features/notifications/NotificationHistoryView.tsx`: paged/filterable operation history and task actions.
- Create `apps/web/src/features/notifications/ProcessLogView.tsx`: current-session log viewer.
- Create `apps/web/src/core/notificationReporting.ts`: broad renderer-error and thumbnail-summary reporting.
- Create `apps/web/tests/notificationModel.test.mts`: pure presentation behavior.
- Modify `apps/web/src/core/desktop.ts`: typed bridge definitions and browser fallbacks.
- Modify `apps/web/src/core/useLibraryData.ts`: report broad failures, publish existing thumbnail snapshots, and remove Recent presentation state.
- Modify `apps/web/src/app/App.tsx`, `app-shell/ShellTopBar.tsx`, and `app-shell/ShellOverlayHost.tsx`: one notification popover state.
- Modify `apps/web/src/main.tsx`: notification secondary-window route.
- Modify `apps/web/src/features/library/LibraryView.tsx`: remove the broad error strip.
- Modify `apps/web/src/features/settings/settingsPanels.tsx`: reuse the thumbnail-summary mapper/publisher.
- Modify `apps/web/src/styles.css`: approved status-first visual design.
- Delete superseded `TaskPanel.tsx`, `TaskCenter.tsx`, `TaskOverlay.tsx`, and `RecentOpsPanel.tsx` after all references are gone.

### Validators and documentation

- Modify `tools/checks/validate-core-api.mjs`, `validate-web-client.mjs`, `validate-desktop-core.mjs`, `validate-ui-design.mjs`, and `validate_sqlite_schema.py`.
- Modify `.tmp/visual-check/desktop-ui-regression.mjs` for the notification popover and secondary window.
- Modify `README.md`, `CLAUDE.md`, and `docs/project-structure.md` as required by the confirmed design.

---

### Task 1: Freeze the Core Outbox API contract

**Files:**
- Modify: `contracts/core-api/openapi.yaml`
- Modify: `packages/core-client/src/generated-contract.ts`
- Modify: `packages/core-client/src/client.ts`
- Modify: `packages/core-client/src/index.ts`
- Test: `packages/core-client/tests/client.test.mts`

- [ ] **Step 1: Write failing client request tests**

Add tests that lock the cursor, wait, limit, acknowledgement body, abort signal, and session header:

```ts
test("listNotificationOutbox sends a bounded long-poll query", async () => {
  mockFetch({ items: [], maxSequence: 41 });
  const controller = new AbortController();
  await client().listNotificationOutbox(
    { afterSequence: 41, limit: 100, waitMs: 20_000 },
    { signal: controller.signal }
  );
  const url = new URL(recorded[0].url);
  assert.equal(url.pathname, "/api/notifications/outbox");
  assert.equal(url.searchParams.get("afterSequence"), "41");
  assert.equal(url.searchParams.get("limit"), "100");
  assert.equal(url.searchParams.get("waitMs"), "20000");
  assert.equal(recorded[0].signal, controller.signal);
});

test("ackNotificationOutbox posts the committed high-water sequence", async () => {
  mockFetch({ acknowledged: true });
  await client().ackNotificationOutbox({ maxSequence: 57 });
  assert.equal(recorded[0].url, `${BASE_URL}/notifications/outbox/ack`);
  assert.equal(recorded[0].method, "POST");
  assert.deepEqual(JSON.parse(recorded[0].body!), { maxSequence: 57 });
});
```

- [ ] **Step 2: Run the tests and confirm the methods are absent**

Run: `npm --workspace @mangoo/core-client run test -- --test-name-pattern="NotificationOutbox"`

Expected: FAIL because `listNotificationOutbox` and `ackNotificationOutbox` do not exist.

- [ ] **Step 3: Add OpenAPI paths and schemas**

Define `GET /notifications/outbox` and `POST /notifications/outbox/ack`. The generated TypeScript shape must be exactly:

```ts
export type NotificationOrigin = "user" | "background" | "recovery" | "system";
export type NotificationPresentation = "important" | "history_only" | "status_only";
export type NotificationOperationStatus =
  | "pending"
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled";

export interface NotificationOperationSnapshot {
  operationId: string;
  revision: number;
  kind: string;
  category: "task" | "file_operation" | "error" | "thumbnail_cache";
  origin: NotificationOrigin;
  presentation: NotificationPresentation;
  status: NotificationOperationStatus;
  phase: string | null;
  title: string;
  summary: string | null;
  taskId: number | null;
  rootId: number | null;
  folderId: number | null;
  fileId: number | null;
  currentValue: number;
  totalValue: number | null;
  unit: string | null;
  errorCode: string | null;
  errorDetail: string | null;
  cancellable: boolean;
  retryable: boolean;
  attempt: number;
  occurrenceCount: number;
  startedAt: number;
  updatedAt: number;
  completedAt: number | null;
}

export interface NotificationOutboxEvent {
  sequence: number;
  operation: NotificationOperationSnapshot;
}

export interface NotificationOutboxResponse {
  items: NotificationOutboxEvent[];
  maxSequence: number;
}

export interface NotificationOutboxAckRequest { maxSequence: number }
export interface NotificationOutboxAckResponse { acknowledged: boolean }
export interface ListNotificationOutboxParams {
  afterSequence?: number;
  limit?: number;
  waitMs?: number;
}
```

The query clamps must be documented in OpenAPI: `limit` 1..200 and `waitMs` 0..30000.

- [ ] **Step 4: Implement client methods**

Add the methods to `createCoreClient`:

```ts
listNotificationOutbox: (
  params: ListNotificationOutboxParams,
  options: CoreRequestOptions = {}
) => request<NotificationOutboxResponse>(
  `/notifications/outbox${notificationOutboxQuery(params)}`,
  {
    requestPriority: options.requestPriority ?? "metadata",
    signal: options.signal
  }
),
ackNotificationOutbox: (body: NotificationOutboxAckRequest) =>
  request<NotificationOutboxAckResponse>("/notifications/outbox/ack", {
    method: "POST",
    requestPriority: "metadata",
    body: JSON.stringify(body)
  })
```

Implement `notificationOutboxQuery` in the same style as `fileOpsQuery`, adding
only defined values through `URLSearchParams`. Export every new type from
`packages/core-client/src/index.ts`.

- [ ] **Step 5: Verify and commit**

Run:

```powershell
npm run check:core-api
npm run check:core-client
```

Expected: both PASS.

Commit:

```powershell
git add contracts/core-api/openapi.yaml packages/core-client/src packages/core-client/tests/client.test.mts
git commit -m "feat(core-api): define notification outbox contract"
```

---

### Task 2: Add the library notification schema

**Files:**
- Create: `crates/core/migrations/0021_notification_outbox.sql`
- Modify: `crates/core/src/db/migrations.rs`
- Modify: `crates/core/src/db/mod.rs`
- Modify: `tools/checks/validate_sqlite_schema.py`
- Test: `crates/core/src/db/mod.rs`

- [ ] **Step 1: Add failing schema assertions**

Extend the schema validator to require these columns and indexes:

```python
head_columns = {
    row[1]
    for row in conn.execute("PRAGMA table_info(notification_operation_heads)").fetchall()
}
required_head_columns = {
    "operation_id", "kind", "status", "presentation", "revision",
    "payload_json", "dirty", "last_outbox_at", "updated_at", "completed_at",
}
if required_head_columns - head_columns:
    fail(f"missing notification head columns: {sorted(required_head_columns - head_columns)}")

outbox_columns = {
    row[1] for row in conn.execute("PRAGMA table_info(notification_outbox)").fetchall()
}
required_outbox_columns = {
    "sequence", "operation_id", "revision", "payload_json", "created_at",
}
if required_outbox_columns - outbox_columns:
    fail(f"missing notification outbox columns: {sorted(required_outbox_columns - outbox_columns)}")

file_operation_columns = {
    row[1] for row in conn.execute("PRAGMA table_info(file_operations)").fetchall()
}
if "operation_group_id" not in file_operation_columns:
    fail("missing file_operations.operation_group_id")

index_names = {
    row[0]
    for row in conn.execute(
        "SELECT name FROM sqlite_master WHERE type='index' AND name IS NOT NULL"
    ).fetchall()
}
required_indexes = {
    "idx_notification_heads_status_updated",
    "idx_notification_outbox_operation_revision",
    "idx_file_operations_group",
}
if required_indexes - index_names:
    fail(f"missing notification indexes: {sorted(required_indexes - index_names)}")
```

- [ ] **Step 2: Run schema validation and confirm failure**

Run: `npm run check:schema`

Expected: FAIL because migration 0021 and its tables do not exist.

- [ ] **Step 3: Create migration 0021**

Use one transaction and register version 21:

```sql
BEGIN IMMEDIATE;

CREATE TABLE notification_operation_heads (
  operation_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','running','succeeded','failed','cancelled')),
  presentation TEXT NOT NULL CHECK(presentation IN ('important','history_only','status_only')),
  revision INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  dirty INTEGER NOT NULL DEFAULT 1 CHECK(dirty IN (0,1)),
  last_outbox_at INTEGER,
  updated_at INTEGER NOT NULL,
  completed_at INTEGER
);

CREATE TABLE notification_outbox (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  operation_id TEXT NOT NULL,
  revision INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  UNIQUE(operation_id, revision)
);

ALTER TABLE file_operations ADD COLUMN operation_group_id TEXT;

CREATE INDEX idx_notification_heads_status_updated
  ON notification_operation_heads(status, updated_at DESC);
CREATE INDEX idx_notification_outbox_operation_revision
  ON notification_outbox(operation_id, revision);
CREATE INDEX idx_file_operations_group
  ON file_operations(operation_group_id, id);

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (21, 'notification_outbox', unixepoch());

COMMIT;
```

- [ ] **Step 4: Register and test the migration**

Add `NOTIFICATION_OUTBOX_MIGRATION`, apply it after migration 20, and add a Rust migration test that inserts one valid head/outbox row and verifies an invalid status is rejected.

- [ ] **Step 5: Verify and commit**

Run:

```powershell
npm run check:schema
cargo test -p mangoo-core migration_0021 -- --nocapture
```

Expected: PASS.

Commit:

```powershell
git add crates/core/migrations/0021_notification_outbox.sql crates/core/src/db/migrations.rs crates/core/src/db/mod.rs tools/checks/validate_sqlite_schema.py
git commit -m "feat(core): add notification outbox schema"
```

---

### Task 3: Implement transactional operation heads and Outbox replay

**Files:**
- Create: `crates/core/src/db/notification_outbox.rs`
- Modify: `crates/core/src/db/mod.rs`
- Test: `crates/core/src/db/notification_outbox.rs`

- [ ] **Step 1: Write failing repository tests**

Cover first insert, coalesced progress, forced terminal delivery, replay, acknowledgement, dirty-head recovery, and thumbnail rejection:

```rust
#[test]
fn progress_is_coalesced_but_terminal_event_is_immediate() {
    let db = migrated_database();
    let mut op = fixture_operation("task:7", "root_scan", "running", 10);
    assert!(db.publish_notification_operation(&mut op, PublishMode::Phase).unwrap());
    op.current_value = 20;
    op.updated_at += 100;
    assert!(!db.publish_notification_operation(&mut op, PublishMode::Progress).unwrap());
    op.status = "succeeded".into();
    op.completed_at = Some(op.updated_at + 1);
    assert!(db.publish_notification_operation(&mut op, PublishMode::Terminal).unwrap());
    let page = db.list_notification_outbox(0, 100).unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page.last().unwrap().operation.status, "succeeded");
}

#[test]
fn acknowledged_events_replay_safely_and_dirty_heads_recover() {
    let db = migrated_database();
    let mut op = fixture_operation("task:9", "root_scan", "running", 1);
    assert!(db.publish_notification_operation(&mut op, PublishMode::Phase).unwrap());
    let first = db.list_notification_outbox(0, 100).unwrap();
    let replay = db.list_notification_outbox(0, 100).unwrap();
    assert_eq!(first[0].sequence, replay[0].sequence);
    assert_eq!(db.acknowledge_notification_outbox(first[0].sequence).unwrap(), 1);
    assert!(db.list_notification_outbox(0, 100).unwrap().is_empty());

    op.current_value = 2;
    op.updated_at += 100;
    assert!(!db.publish_notification_operation(&mut op, PublishMode::Progress).unwrap());
    assert_eq!(db.flush_dirty_notification_heads().unwrap(), 1);
    let recovered = db.list_notification_outbox(first[0].sequence, 100).unwrap();
    assert_eq!(recovered.len(), 1);
    assert!(recovered[0].sequence > first[0].sequence);
    assert_eq!(recovered[0].operation.current_value, 2);
}
```

- [ ] **Step 2: Run the focused test and confirm failure**

Run: `cargo test -p mangoo-core notification_outbox -- --nocapture`

Expected: FAIL because the module and methods are absent.

- [ ] **Step 3: Implement the operation model and publish policy**

Define `NotificationOperation`, `NotificationOutboxEvent`, and `PublishMode`. The public publishing rule must be:

```rust
pub enum PublishMode { Phase, Progress, Terminal }

fn should_emit(mode: PublishMode, previous_last_outbox_at: Option<i64>, now_ms: i64) -> bool {
    match mode {
        PublishMode::Phase | PublishMode::Terminal => true,
        PublishMode::Progress => previous_last_outbox_at
            .map(|last| now_ms.saturating_sub(last) >= 1_000)
            .unwrap_or(true),
    }
}
```

`publish_notification_operation_in_transaction` must increment the head revision, serialize the final revision into `payload_json`, upsert the head, and append an Outbox row only when `should_emit` is true. A suppressed progress update sets `dirty=1`. Terminal and phase emissions set `dirty=0`.

- [ ] **Step 4: Share one wake signal across reopened databases**

Change `Database` to:

```rust
pub struct Database {
    connection: Connection,
    path: Option<PathBuf>,
    notification_outbox_wake: Arc<tokio::sync::Notify>,
}
```

`open` creates the `Arc`; `reopen` clones it. Expose `notification_outbox_wake()` for `AppState`. Call `notify_one()` only after a transaction that appended an Outbox row commits.

- [ ] **Step 5: Implement read, ack, and recovery methods**

Add bounded methods:

```rust
pub fn list_notification_outbox(&self, after: i64, limit: i64)
    -> anyhow::Result<Vec<NotificationOutboxEvent>>;
pub fn acknowledge_notification_outbox(&self, max_sequence: i64)
    -> anyhow::Result<usize>;
pub fn flush_dirty_notification_heads(&self) -> anyhow::Result<usize>;
pub fn recover_interrupted_notification_operations(&self) -> anyhow::Result<usize>;
```

Recovery marks non-task heads left pending/running after a Core crash as failed with code `interrupted`, appends a forced terminal event, and leaves recoverable task heads to task startup recovery.

- [ ] **Step 6: Verify and commit**

Run: `cargo test -p mangoo-core notification_outbox -- --nocapture`

Expected: PASS.

Commit:

```powershell
git add crates/core/src/db/notification_outbox.rs crates/core/src/db/mod.rs
git commit -m "feat(core): persist logical notification operations"
```

---

### Task 4: Publish task lifecycle and determinate root-purge progress

**Files:**
- Modify: `crates/core/src/db/mod.rs`
- Modify: `crates/core/src/tasks.rs`
- Modify: `crates/core/src/api/routes.rs`
- Modify: `crates/core/src/watch/mod.rs`
- Test: `crates/core/src/db/mod.rs`
- Test: `crates/core/src/tasks.rs`

- [ ] **Step 1: Write failing task-notification tests**

Add this lifecycle test:

```rust
#[test]
fn root_scan_lifecycle_updates_one_notification_operation() {
    let db = migrated_database();
    let root_id = seed_root(&db);
    let task_id = db.create_root_scan_task_with_origin(root_id, NotificationOrigin::User).unwrap();
    db.mark_task_running_for_attempt(task_id, 0).unwrap();
    db.update_task_scan_progress_for_attempt(task_id, 0, TaskScanProgress {
        items_seen: 50, items_total: Some(100), folders_seen: 4,
        media_files_seen: 46, skipped_files: 0,
    }).unwrap();
    db.mark_task_succeeded_for_attempt(task_id, 0).unwrap();
    let heads = db.notification_heads_for_test().unwrap();
    assert_eq!(heads.len(), 1);
    assert_eq!(heads[0].operation_id, format!("task:{task_id}"));
    assert_eq!(heads[0].status, "succeeded");
}
```

Add the thumbnail exclusion test:

```rust
#[test]
fn thumbnail_tasks_never_create_notification_heads() {
    let db = migrated_database();
    let file_ids = seed_thumbnail_files(&db, 100);
    for file_id in file_ids {
        let outcome = db.request_thumbnail_task(file_id, ThumbnailTaskPriority::Background).unwrap();
        let task_id = outcome.task_id.expect("thumbnail task id");
        let generation = db.current_task_attempt_generation(task_id).unwrap();
        db.mark_thumbnail_task_running_for_attempt(task_id, generation, "fixture").unwrap();
        db.mark_task_failed_for_attempt(task_id, generation, "fixture failure").unwrap();
    }
    assert!(db.notification_heads_for_test().unwrap().is_empty());
}
```

- [ ] **Step 2: Add a failing root-purge total test**

Seed files and folders, start root purge, and assert the first running snapshot has `total_value > 0`, intermediate snapshots increase `current_value`, and the terminal Outbox event exists after both the task and root rows are deleted.

- [ ] **Step 3: Run focused tests and confirm failure**

Run:

```powershell
cargo test -p mangoo-core root_scan_lifecycle_updates_one_notification_operation -- --nocapture
cargo test -p mangoo-core root_purge_notification -- --nocapture
```

Expected: FAIL.

- [ ] **Step 4: Wire non-thumbnail task transitions**

Add `create_root_scan_task_with_origin`; keep `create_root_scan_task` as the background-origin wrapper so existing watcher/tests remain source-compatible. API-created scans use `User`; watcher scans use `Background`; startup recovery uses `Recovery`.

Create one exhaustive `task_notification_descriptor(kind)` mapping for
`root_scan`, `interactive_folder_scan`, and `root_purge`; return `None` only for
`thumbnail`. A future non-thumbnail kind must fail its notification integration
test until the mapping is extended.

Refactor pending, running, phase/progress, cancellation, retry, success, and
failure writes so the task row and `task:{task_id}` operation head update in the
same SQLite transaction. Derive capabilities from status and task kind. Notify
the Outbox waiter only after that transaction commits. Do not publish task
operations for `kind == "thumbnail"`.

- [ ] **Step 5: Make root-purge progress determinate**

Before the first purge batch, compute the bounded purge work total from the exact task/file/folder/root rows deleted by the purge phases and store it in task `items_total`. Keep `items_seen` as deleted work units. In the final `purge_root_next_batch` transaction, append the terminal succeeded Outbox snapshot before deleting the root-purge task and root row.

- [ ] **Step 6: Verify and commit**

Run:

```powershell
cargo test -p mangoo-core task_lifecycle -- --nocapture
cargo test -p mangoo-core root_purge -- --nocapture
```

Expected: PASS, including existing root-purge recovery tests.

Commit:

```powershell
git add crates/core/src/db/mod.rs crates/core/src/tasks.rs crates/core/src/api/routes.rs crates/core/src/watch/mod.rs
git commit -m "feat(core): publish aggregated task progress"
```

---

### Task 5: Group file operations by user command

**Files:**
- Modify: `crates/core/src/fsops/mod.rs`
- Modify: `crates/core/src/api/routes.rs`
- Test: `crates/core/src/fsops/mod.rs`

- [ ] **Step 1: Write failing grouping tests**

For a move request containing multiple files, assert all audit rows share one `operation_group_id`, one notification head exists, and the final snapshot reports the full count. Add a partial-failure test whose one group ends failed with completed/total counts.

```rust
assert_eq!(records.len(), 3);
assert!(records.iter().all(|row| row.operation_group_id == records[0].operation_group_id));
let head = database.notification_head(&records[0].operation_group_id).unwrap().unwrap();
assert_eq!(head.status, "succeeded");
assert_eq!(head.current_value, 3);
assert_eq!(head.total_value, Some(3));
```

- [ ] **Step 2: Run focused tests and confirm failure**

Run: `cargo test -p mangoo-core fsops::tests -- --nocapture`

Expected: FAIL on the missing group field and head.

- [ ] **Step 3: Create one group before filesystem work**

Generate a UUIDv7 group ID once in `rename`, `move_items`, and `delete`. Insert a pending user-origin operation head before processing items. Pass the group ID into every success/failure audit insert and expose it on the internal Rust `FileOperationRecord` without changing the public legacy response unless the OpenAPI schema is intentionally extended.

- [ ] **Step 4: Update one head, not one notification row per item**

After each item audit transaction, update the operation head's progress. Apply the one-second Outbox coalescing rule. At command success or failure, force one terminal Outbox event. On startup, `recover_interrupted_notification_operations` converts a non-terminal file-operation group to failed `interrupted` so a crash cannot leave it permanently running.

- [ ] **Step 5: Verify and commit**

Run:

```powershell
cargo test -p mangoo-core fsops::tests -- --nocapture
cargo test -p mangoo-core file_ops -- --nocapture
```

Expected: PASS.

Commit:

```powershell
git add crates/core/src/fsops/mod.rs crates/core/src/api/routes.rs
git commit -m "feat(core): aggregate file operation notifications"
```

---

### Task 6: Expose wake-driven Outbox routes

**Files:**
- Modify: `crates/core/src/api/mod.rs`
- Modify: `crates/core/src/api/routes.rs`
- Test: `crates/core/src/api/routes.rs`

- [ ] **Step 1: Write failing route tests**

Add tests for immediate data, timeout-empty response, wake-after-commit, replay-before-ack, and authenticated acknowledgement. The wake test should start the request, commit an operation after the waiter is registered, and assert the response returns before `waitMs` expires.

- [ ] **Step 2: Run route tests and confirm 404**

Run: `cargo test -p mangoo-core notification_outbox_route -- --nocapture`

Expected: FAIL with route not found.

- [ ] **Step 3: Add the shared wake handle to AppState**

Capture `database.notification_outbox_wake()` before moving the database into `DbWriter`:

```rust
pub struct AppState {
    // existing fields
    notification_outbox_wake: Arc<tokio::sync::Notify>,
}
```

Flush dirty heads and recover interrupted non-task heads during startup before worker dispatch.

- [ ] **Step 4: Implement bounded long-poll and ack handlers**

The read handler must query first, await `Notify` only when empty, then query once more. Clamp `waitMs` to 30 seconds and `limit` to 200. The ack handler performs one writer job and returns `{ acknowledged: true }` only after deletion commits.

```rust
let items = read_outbox(&state, query.after_sequence, limit).await?;
if !items.is_empty() || wait.is_zero() {
    return Ok(Json(outbox_response(items)));
}
let _ = tokio::time::timeout(wait, state.notification_outbox_wake.notified()).await;
Ok(Json(outbox_response(read_outbox(&state, query.after_sequence, limit).await?)))
```

- [ ] **Step 5: Verify and commit**

Run:

```powershell
cargo test -p mangoo-core notification_outbox_route -- --nocapture
npm run check:core-api
```

Expected: PASS.

Commit:

```powershell
git add crates/core/src/api/mod.rs crates/core/src/api/routes.rs
git commit -m "feat(core): stream notification outbox changes"
```

---

### Task 7: Build the async Desktop global notification store

**Files:**
- Create: `apps/desktop/src/notification-types.ts`
- Create: `apps/desktop/src/notification-store-worker.ts`
- Create: `apps/desktop/src/notification-store.ts`
- Modify: `apps/desktop/package.json`
- Test: `apps/desktop/tests/notification-store.test.mts`

- [ ] **Step 1: Write failing store tests**

Test atomic operation/cursor upsert, revision idempotency, all-library history, compact latest-three selection inputs, thumbnail summary upsert, active retention exemption, 30-day cleanup, and 2,000-completed-record cleanup.

```ts
it("commits operation revisions and cursor atomically", async () => {
  const store = await createTestNotificationStore();
  await store.applyOutboxBatch(libraryA, [event(1, "task:1", 1, "running")], 1);
  await store.applyOutboxBatch(libraryA, [event(1, "task:1", 1, "running")], 1);
  const snapshot = await store.compactSnapshot();
  assert.equal(snapshot.active.length, 1);
  assert.equal(await store.cursor(libraryA.id), 1);
  await store.close();
});
```

- [ ] **Step 2: Run the test and confirm missing modules**

Run: `npm --workspace @mangoo/desktop run test -- --test-name-pattern="notification store"`

Expected: FAIL because the store does not exist.

- [ ] **Step 3: Define Desktop-owned DTOs**

Define `DesktopNotificationOperation`, `NotificationCompactSnapshot`, `NotificationHistoryQuery`, `NotificationHistoryPage`, `ThumbnailCacheSummary`, `NotificationSyncHealth`, and `ProcessLogEntry`. Add `libraryId` and `libraryName` to the Core operation snapshot fields; do not add an unread field.

- [ ] **Step 4: Implement the worker schema**

Use `DatabaseSync` only in `notification-store-worker.ts`:

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS operations (
  library_id TEXT NOT NULL,
  operation_id TEXT NOT NULL,
  library_name TEXT NOT NULL,
  revision INTEGER NOT NULL,
  status TEXT NOT NULL,
  category TEXT NOT NULL,
  presentation TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  completed_at INTEGER,
  payload_json TEXT NOT NULL,
  PRIMARY KEY(library_id, operation_id)
);
CREATE INDEX IF NOT EXISTS idx_operations_history
  ON operations(completed_at DESC, updated_at DESC);
CREATE TABLE IF NOT EXISTS sync_cursors (
  library_id TEXT PRIMARY KEY,
  sequence INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS thumbnail_summaries (
  library_id TEXT PRIMARY KEY,
  library_name TEXT NOT NULL,
  observed_at INTEGER NOT NULL,
  payload_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS legacy_imports (
  library_id TEXT PRIMARY KEY,
  imported_at INTEGER NOT NULL
);
```

Use `BEGIN IMMEDIATE` for batch upsert plus cursor advancement. Ignore older/equal revisions except for exact replay.

- [ ] **Step 5: Implement the asynchronous worker facade**

`notification-store.ts` sends request IDs over `worker_threads`, resolves typed promises, rejects all pending calls on worker failure, and closes cleanly. Resolve the worker URL as:

```ts
new Worker(new URL("./notification-store-worker.js", import.meta.url), {
  workerData: { databasePath }
});
```

In tests, allow an injected worker URL that points at the TypeScript source under Node's strip-types runner.

Change only the desktop test command to suppress Node 22's known warning:

```json
"test": "node --disable-warning=ExperimentalWarning --experimental-strip-types --test tests/*.test.mts"
```

- [ ] **Step 6: Verify and commit**

Run:

```powershell
npm --workspace @mangoo/desktop run test -- --test-name-pattern="notification store"
npm --workspace @mangoo/desktop run typecheck
```

Expected: PASS.

Commit:

```powershell
git add apps/desktop/src/notification-types.ts apps/desktop/src/notification-store-worker.ts apps/desktop/src/notification-store.ts apps/desktop/tests/notification-store.test.mts apps/desktop/package.json
git commit -m "feat(desktop): add global notification store"
```

---

### Task 8: Implement Desktop synchronization, legacy import, and actions

**Files:**
- Create: `apps/desktop/src/notification-service.ts`
- Test: `apps/desktop/tests/notification-service.test.mts`

- [ ] **Step 1: Write failing service tests with injected fetch**

Cover attach, long-poll, atomic ack ordering, replay, abort on switch, retry backoff, legacy import, cancel/retry proxying, renderer-error deduplication, subscriber broadcast, and store-worker failure fallback.

```ts
it("acks only after the global batch commits", async () => {
  const calls: string[] = [];
  const service = createService({
    store: fakeStore({ onApply: () => calls.push("store") }),
    fetch: scriptedFetch({ onAck: () => calls.push("ack") })
  });
  await service.runOneDrainForTest(activeLibrary, coreSession);
  assert.deepEqual(calls, ["store", "ack"]);
});

it("does not ack when the global store is unavailable", async () => {
  const calls: string[] = [];
  const service = createService({
    store: fakeStore({ onApply: () => { throw new Error("store offline"); } }),
    fetch: scriptedFetch({ onAck: () => calls.push("ack") })
  });
  await service.runOneDrainForTest(activeLibrary, coreSession);
  assert.deepEqual(calls, []);
  assert.equal((await service.compactSnapshot()).syncHealth.state, "degraded");
});
```

- [ ] **Step 2: Run the tests and confirm failure**

Run: `npm --workspace @mangoo/desktop run test -- --test-name-pattern="notification service"`

Expected: FAIL because `NotificationService` is absent.

- [ ] **Step 3: Implement one abortable drain loop**

`attachLibrary` aborts the previous loop, reads the committed cursor, long-polls `/notifications/outbox`, applies a batch, then posts `/notifications/outbox/ack`. Retry transient failures with bounded backoff from 250 ms to 5 seconds. Reset backoff after one successful request. Publish sync health directly rather than recursively creating an error notification.

If the store worker fails, keep a bounded current-session in-memory map of the
latest 500 operations and thumbnail summaries, mark sync health degraded, and
do not acknowledge any newly read Outbox sequence. Restart the store worker on
backoff; after it opens successfully, replay the still-unacknowledged Core batch
into SQLite, replace the fallback snapshot with the durable snapshot, and only
then acknowledge Core. Store failure must never reject a media-browsing IPC or
stop Core.

- [ ] **Step 4: Implement idempotent legacy Recent import**

If `legacy_imports` has no row for the active library, page `/file-ops` until rows are older than 30 days. Convert each legacy row to deterministic ID `legacy-file-op:{id}`, map it to one completed operation, batch insert, then mark the library imported. A failed import leaves the marker absent and retries next activation.

- [ ] **Step 5: Implement Desktop-side commands and reports**

Add:

```ts
cancelOperation(libraryId: string, operationId: string): Promise<ActionResult>;
retryOperation(libraryId: string, operationId: string): Promise<ActionResult>;
reportRendererError(input: RendererErrorReport): Promise<void>;
publishThumbnailSummary(summary: ThumbnailCacheSummary): Promise<void>;
compactSnapshot(): Promise<NotificationCompactSnapshot>;
history(query: NotificationHistoryQuery): Promise<NotificationHistoryPage>;
subscribe(listener: (snapshot: NotificationCompactSnapshot) => void): () => void;
```

Actions reject inactive-library operations with an explicit non-destructive message because only the active library has a running Core. Broad renderer errors deduplicate to `renderer-error:{libraryId}:{source}:{code}:{target}` and increment occurrence count.

- [ ] **Step 6: Verify and commit**

Run:

```powershell
npm --workspace @mangoo/desktop run test -- --test-name-pattern="notification service"
npm --workspace @mangoo/desktop run typecheck
```

Expected: PASS.

Commit:

```powershell
git add apps/desktop/src/notification-service.ts apps/desktop/tests/notification-service.test.mts
git commit -m "feat(desktop): synchronize notification outboxes"
```

---

### Task 9: Add the bounded current-process log pipeline

**Files:**
- Create: `apps/desktop/src/process-log-buffer.ts`
- Create: `apps/desktop/src/desktop-logger.ts`
- Modify: `apps/desktop/src/core-process.ts`
- Modify: `apps/desktop/src/main.ts`
- Test: `apps/desktop/tests/process-log-buffer.test.mts`

- [ ] **Step 1: Write failing ring-buffer tests**

Test source/level filters, text search, monotonic IDs, 5,000-line cap, 5 MB cap, and subscriber delivery.

```ts
const buffer = new ProcessLogBuffer({ maxEntries: 3, maxBytes: 1024 });
buffer.append("main", "info", "one");
buffer.append("core", "warn", "two");
buffer.append("renderer", "error", "three");
buffer.append("main", "debug", "four");
assert.deepEqual(buffer.snapshot({}).map((line) => line.message), ["two", "three", "four"]);
```

- [ ] **Step 2: Run and confirm failure**

Run: `npm --workspace @mangoo/desktop run test -- --test-name-pattern="process log"`

Expected: FAIL.

- [ ] **Step 3: Implement buffer and shared logger**

The logger writes to the original console and appends a normalized line to the ring. It serializes `Error` values without throwing. Buffer bytes use UTF-8 byte length, not JavaScript string length.

- [ ] **Step 4: Capture Core and Main sources**

Change `startCoreProcess` to accept an `onLog(source, level, text)` callback. Split stdout/stderr by line, preserve partial chunks, emit stdout as `info`, stderr as `warn`, and process exit as `error` when non-zero. Replace the 11 direct Main console calls with `desktopLogger` calls.

- [ ] **Step 5: Verify and commit**

Run:

```powershell
npm --workspace @mangoo/desktop run test -- --test-name-pattern="process log"
npm --workspace @mangoo/desktop run typecheck
```

Expected: PASS.

Commit:

```powershell
git add apps/desktop/src/process-log-buffer.ts apps/desktop/src/desktop-logger.ts apps/desktop/src/core-process.ts apps/desktop/src/main.ts apps/desktop/tests/process-log-buffer.test.mts
git commit -m "feat(desktop): capture bounded process logs"
```

---

### Task 10: Wire Desktop lifecycle, IPC, preload, and notification window role

**Files:**
- Modify: `apps/desktop/src/main.ts`
- Modify: `apps/desktop/src/preload.cjs`
- Modify: `apps/web/src/core/desktop.ts`
- Modify: `apps/web/src/main.tsx`
- Modify: `tools/checks/validate-desktop-core.mjs`

- [ ] **Step 1: Add failing desktop boundary checks**

Require the `notifications` secondary-window role, open/close IPC, snapshot/history/action methods, change subscription, thumbnail/error reporting, log snapshot/subscription, and no `node:fs` import in Web.

- [ ] **Step 2: Run the desktop check and confirm failure**

Run: `npm run check:desktop`

Expected: FAIL on missing notification bridge/window markers.

- [ ] **Step 3: Add the notification secondary-window role**

Extend every closed role map and persisted-position reader with `notifications`. Add this configuration:

```ts
notifications: {
  title: "Mangoo 通知中心",
  route: "#/notifications",
  width: 1040,
  height: 720,
  minWidth: 820,
  minHeight: 560
}
```

Reuse `openSecondaryWindow`; do not add another reveal/material implementation. Add `mangoo:open-notifications-window` and `mangoo:close-notifications-window`.

- [ ] **Step 4: Own service startup and library switching in Main**

Create the store at `path.join(app.getPath("userData"), "notifications.sqlite")`. Start `NotificationService` once. After Core health succeeds, attach the active registry entry and Core session. On switch/create/open, attach the new entry after registry activation. On `before-quit`, abort sync, close the store worker, and then stop Core.

- [ ] **Step 5: Add typed IPC and preload subscriptions**

Expose a single nested bridge:

```ts
notifications: {
  getCompactSnapshot: () => ipcRenderer.invoke("mangoo:notifications-compact"),
  listHistory: (query) => ipcRenderer.invoke("mangoo:notifications-history", query),
  cancel: (libraryId, operationId) => ipcRenderer.invoke("mangoo:notifications-cancel", libraryId, operationId),
  retry: (libraryId, operationId) => ipcRenderer.invoke("mangoo:notifications-retry", libraryId, operationId),
  reportError: (input) => ipcRenderer.invoke("mangoo:notifications-report-error", input),
  publishThumbnailSummary: (input) => ipcRenderer.invoke("mangoo:notifications-thumbnail-summary", input),
  onChanged: (cb) => subscribe("mangoo:notifications-changed", cb),
  getLogs: (query) => ipcRenderer.invoke("mangoo:process-logs", query),
  onLogs: (cb) => subscribe("mangoo:process-logs-changed", cb),
  emitRendererLogs: (entries) => ipcRenderer.send("mangoo:renderer-logs", entries)
}
```

Implement `subscribe` locally in preload so each method returns a cleanup function. Validate and bound every renderer-provided array/string in Main before storing it.

- [ ] **Step 6: Route the Web entry and verify**

Add lazy `NotificationWindow` loading for hash/role `notifications` and include it in secondary-window idle preload.

Run:

```powershell
npm run check:desktop
npm --workspace @mangoo/web run typecheck
```

Expected: PASS.

Commit:

```powershell
git add apps/desktop/src/main.ts apps/desktop/src/preload.cjs apps/web/src/core/desktop.ts apps/web/src/main.tsx tools/checks/validate-desktop-core.mjs
git commit -m "feat(desktop): expose notification center window"
```

---

### Task 11: Build the Web notification state model

**Files:**
- Create: `apps/web/src/features/notifications/notificationModel.ts`
- Create: `apps/web/src/features/notifications/useNotifications.ts`
- Test: `apps/web/tests/notificationModel.test.mts`

- [ ] **Step 1: Write failing selector tests**

Cover latest-three important results, active separation, indicator priority, determinate/indeterminate progress, all-library labels, and no read state.

```ts
it("keeps active work separate and limits important results to three", () => {
  const model = buildCompactNotificationModel(snapshot({
    active: [operation("running", { currentValue: 30, totalValue: 100 })],
    recentImportant: [result(4), result(3), result(2), result(1)]
  }));
  assert.equal(model.active.length, 1);
  assert.deepEqual(model.results.map((item) => item.operationId), ["result:4", "result:3", "result:2"]);
});

it("uses failed over active for the subtle indicator", () => {
  assert.equal(notificationIndicator({ active: true, retryableFailure: true }), "failure");
  assert.equal(notificationIndicator({ active: true, retryableFailure: false }), "active");
  assert.equal(notificationIndicator({ active: false, retryableFailure: false }), "idle");
});
```

- [ ] **Step 2: Run and confirm failure**

Run: `npm --workspace @mangoo/web run test -- --test-name-pattern="notification"`

Expected: FAIL because the model is absent.

- [ ] **Step 3: Implement pure selectors**

Return explicit view models rather than formatting in JSX:

```ts
export interface ProgressViewModel {
  determinate: boolean;
  percent: number | null;
  label: string;
}

export function progressView(operation: DesktopNotificationOperation): ProgressViewModel {
  const total = operation.totalValue;
  if (total == null || total <= 0) {
    return { determinate: false, percent: null, label: operation.phase ?? "处理中" };
  }
  const percent = Math.max(0, Math.min(100, Math.round(operation.currentValue / total * 100)));
  return { determinate: true, percent, label: `${operation.currentValue.toLocaleString()} / ${total.toLocaleString()}` };
}
```

- [ ] **Step 4: Implement the subscription hook**

`useNotifications` loads one initial compact snapshot, subscribes once, applies updates through `startTransition`, and unsubscribes on unmount. Expose separate async history/log methods; do not fetch history or logs from the compact hook.

- [ ] **Step 5: Verify and commit**

Run:

```powershell
npm --workspace @mangoo/web run test -- --test-name-pattern="notification"
npm --workspace @mangoo/web run typecheck
```

Expected: PASS.

Commit:

```powershell
git add apps/web/src/features/notifications/notificationModel.ts apps/web/src/features/notifications/useNotifications.ts apps/web/tests/notificationModel.test.mts
git commit -m "feat(web): model unified notification state"
```

---

### Task 12: Replace titlebar Tasks/Recent with the compact notification popover

**Files:**
- Create: `apps/web/src/features/notifications/NotificationPopover.tsx`
- Modify: `apps/web/src/app-shell/ShellTopBar.tsx`
- Modify: `apps/web/src/app-shell/ShellOverlayHost.tsx`
- Modify: `apps/web/src/app/App.tsx`
- Modify: `apps/web/src/styles.css`
- Modify: `tools/checks/validate-ui-design.mjs`
- Modify: `tools/checks/validate-web-client.mjs`

- [ ] **Step 1: Add failing UI validators**

Require one `Bell` button with `data-titlebar-control="notifications"`, `titlebar-icon-button`, one compact root, active progress markup, thumbnail summary, latest-three result markup, and full-center button. Reject `data-titlebar-control="tasks-palette"`, `recent-operations`, and `top-action-label` in `ShellRightActions`.

- [ ] **Step 2: Run checks and confirm failure**

Run:

```powershell
npm run check:web
npm run check:ui-design
```

Expected: FAIL on missing notification UI and legacy controls.

- [ ] **Step 3: Replace compact-popover state**

Change `CompactPopover` to `"notifications" | "filter" | "sort" | null`. Remove `taskCenterOpen`, recent loading-on-open, and Tasks/Recent callbacks. `ShellRightActions` receives snapshot indicator plus open/close notification callbacks.

- [ ] **Step 4: Implement the compact component**

Render sections in this exact order: all active operations, one thumbnail summary per available library, latest three important results, and the full-center button. Unknown progress uses an indeterminate track. Do not render cancel/retry controls.

```tsx
<LiquidGlassSurface
  as="section"
  className="floating-popover notification-popover popup-surface"
  data-compact-popover-root="notifications"
  interactive
  tone="elevated"
>
  <NotificationPopoverContent model={model} onOpenCenter={onOpenCenter} />
</LiquidGlassSurface>
```

- [ ] **Step 5: Implement the subtle titlebar indicator**

Use the normal icon button, not the old pill:

```tsx
<LiquidGlassButton
  aria-label="通知"
  aria-pressed={open}
  className="titlebar-icon-button notification-toggle no-drag"
  data-compact-popover-trigger="notifications"
  data-titlebar-control="notifications"
  onClick={onToggle}
  tone="control"
>
  <Bell aria-hidden="true" size={15} />
  {indicator !== "idle" ? <span className={`notification-status-dot ${indicator}`} /> : null}
</LiquidGlassButton>
```

The dot is 5 CSS px, low opacity, and has no bright glow or count.

- [ ] **Step 6: Verify and commit**

Run:

```powershell
npm run check:web
npm run check:ui-design
```

Expected: PASS.

Commit:

```powershell
git add apps/web/src/features/notifications/NotificationPopover.tsx apps/web/src/app-shell/ShellTopBar.tsx apps/web/src/app-shell/ShellOverlayHost.tsx apps/web/src/app/App.tsx apps/web/src/styles.css tools/checks/validate-ui-design.mjs tools/checks/validate-web-client.mjs
git commit -m "feat(web): add compact notification status"
```

---

### Task 13: Build the full status-first Notification Center window

**Files:**
- Create: `apps/web/src/features/notifications/NotificationWindow.tsx`
- Create: `apps/web/src/features/notifications/NotificationStatusView.tsx`
- Create: `apps/web/src/features/notifications/NotificationHistoryView.tsx`
- Create: `apps/web/src/features/notifications/ProcessLogView.tsx`
- Modify: `apps/web/src/styles.css`
- Modify: `tools/checks/validate-ui-design.mjs`
- Modify: `.tmp/visual-check/desktop-ui-regression.mjs`

- [ ] **Step 1: Add failing structure and visual assertions**

Require `.notification-window`, three navigation items, status/history/log pages, no unread controls, dark left-pane class, right gradient class, no vertical divider border, and the shared `SecondaryWindowFrame` role `notifications`.

- [ ] **Step 2: Run checks and confirm failure**

Run:

```powershell
npm run check:web
npm run check:ui-design
```

Expected: FAIL.

- [ ] **Step 3: Implement the standalone frame and status page**

Use `useSecondaryWindowInterfaceStyle()` and `SecondaryWindowFrame`. The default page is status. Show active operations with library names and progress, thumbnail summaries, important results, and degraded sync health.

- [ ] **Step 4: Implement paged history and actions**

History filters are library, status, category, and text. Reset cursor when a filter changes. Load at most 100 rows per page. Use TanStack Virtual for the rendered row set. Expand a row for target/error/timestamps. Show cancel only when `cancellable`; show retry only when `retryable`. Refresh compact/history state after an action resolves.

- [ ] **Step 5: Implement current-process logs**

Subscribe only while the log page is active. Support source, level, and text filters; follow latest by default; pause/resume; copy selected visible text; export the bounded snapshot through a Desktop save dialog or a narrowly-scoped Desktop export IPC. Use TanStack Virtual for log lines.

- [ ] **Step 6: Apply the approved visual contract**

The essential CSS contract is:

```css
.notification-window-body {
  display: grid;
  grid-template-columns: 184px minmax(0, 1fr);
  min-height: 0;
  background:
    linear-gradient(90deg, var(--secondary-sidebar-material) 0 184px, transparent 184px),
    radial-gradient(circle at 28% 10%, var(--secondary-content-highlight), transparent 44%),
    linear-gradient(135deg, var(--secondary-content-light), var(--secondary-content-dark));
}
.notification-window-nav {
  border-inline-end: 0;
  background: transparent;
}
.notification-window-content {
  min-width: 0;
  overflow: hidden;
  background: transparent;
}
```

Use existing Settings/Plugins material variables; add variables only if the same variables are then shared by all three secondary windows.

- [ ] **Step 7: Extend the real Electron visual harness**

Open the compact popover, verify active/result sections and no legacy buttons, open `#/notifications`, verify one window instance on repeated open, inspect secondary reveal attributes, capture the status page, switch to logs, then close the window. Do not use screenshot pixel classification for acrylic quality.

- [ ] **Step 8: Verify and commit**

Run:

```powershell
npm run check:web
npm run check:ui-design
```

Expected: PASS.

Commit:

```powershell
git add apps/web/src/features/notifications apps/web/src/styles.css tools/checks/validate-ui-design.mjs .tmp/visual-check/desktop-ui-regression.mjs
git commit -m "feat(web): add full notification center"
```

---

### Task 14: Route broad errors and publish shared thumbnail summaries

**Files:**
- Create: `apps/web/src/core/notificationReporting.ts`
- Modify: `apps/web/src/core/useLibraryData.ts`
- Modify: `apps/web/src/features/settings/settingsPanels.tsx`
- Modify: `apps/web/src/features/library/LibraryView.tsx`
- Modify: `apps/web/src/styles.css`
- Test: `apps/web/tests/notificationModel.test.mts`

- [ ] **Step 1: Add failing reporting tests**

Test thumbnail summary math and renderer-error deduplication input:

```ts
assert.deepEqual(thumbnailSummary(stats, "lib-a", "Artists", 1000), {
  libraryId: "lib-a",
  libraryName: "Artists",
  cachedCount: 80,
  remainingCount: 20,
  failedCount: 2,
  activeBulkTaskCount: 1,
  percent: 80,
  observedAt: 1000
});
assert.equal(rendererErrorKey({ source: "media-load", code: "timeout", target: "folder:4" }),
  "media-load:timeout:folder:4");
```

- [ ] **Step 2: Implement one reporting boundary**

`notificationReporting.ts` wraps only Desktop bridge methods. It never throws into browsing code. It drops overlong strings, normalizes dedupe keys, and returns immediately when no Desktop bridge exists.

- [ ] **Step 3: Publish existing thumbnail snapshots without a new polling loop**

Whenever `useLibraryData` or the Settings thumbnail tab already receives a fresh `thumbnailCacheStats`, convert and publish it once. The compact popover may schedule one deferred refresh through the existing `refreshThumbnailCacheStats` callback when no snapshot exists; it must not create an interval. The full center displays persisted last-known summaries for inactive libraries.

- [ ] **Step 4: Remove the broad center-pane error strip**

Delete:

```tsx
{library.error ? <div className="error-strip">{library.error}</div> : null}
```

Replace broad `setError(...)` paths in `useLibraryData` with `reportLibraryError({ source, code, target, message })`. Remove `LibraryState.error` and its backing state after all references are gone. Keep dialog-local server errors and `LibraryLoadingOverlay` recovery behavior. Do not double-report file-operation failures already emitted by Core.

- [ ] **Step 5: Verify and commit**

Run:

```powershell
npm --workspace @mangoo/web run test -- --test-name-pattern="notification"
npm run check:web
```

Expected: PASS and `rg -n "error-strip|library\.error" apps/web/src` returns no broad workspace banner.

Commit:

```powershell
git add apps/web/src/core/notificationReporting.ts apps/web/src/core/useLibraryData.ts apps/web/src/features/settings/settingsPanels.tsx apps/web/src/features/library/LibraryView.tsx apps/web/src/styles.css apps/web/tests/notificationModel.test.mts
git commit -m "feat(web): route workspace status into notifications"
```

---

### Task 15: Remove legacy presentation, update validators, and document the feature

**Files:**
- Delete: `apps/web/src/features/tasks/TaskPanel.tsx`
- Delete: `apps/web/src/features/tasks/TaskCenter.tsx`
- Delete: `apps/web/src/features/tasks/TaskOverlay.tsx`
- Delete: `apps/web/src/features/file-ops/RecentOpsPanel.tsx`
- Modify: `apps/web/src/core/useLibraryData.ts`
- Modify: `tools/checks/validate-web-client.mjs`
- Modify: `tools/checks/validate-ui-design.mjs`
- Modify: `tools/checks/validate-structure.mjs`
- Modify: `README.md`
- Modify: `CLAUDE.md`
- Modify: `docs/project-structure.md`

- [ ] **Step 1: Prove legacy UI is unreferenced**

Run:

```powershell
rg -n "TaskPanel|TaskCenter|TaskOverlay|RecentOpsPanel|recentOpsOpen|taskPaletteOpen|taskCenterOpen" apps/web/src tools/checks
```

Expected before cleanup: remaining legacy definitions/references are listed. Remove or replace every result before deletion.

- [ ] **Step 2: Remove Recent-only renderer state and old CSS**

Remove `recentOps`, `recentOpsLoading`, `loadRecentOps`, and file-operation callbacks that refreshed Recent solely for display. Preserve the Core audit endpoint for migration/history. Remove `.task-*`, `.recent-ops-*`, old drawer selectors, and obsolete responsive rules only after `rg` proves no remaining consumer.

- [ ] **Step 3: Delete legacy components and update validators**

Validators must reject reintroduction of separate Tasks/Recent titlebar surfaces and require the unified notification primitives. Do not weaken unrelated Liquid Glass, pointer-highlight, or secondary-window checks.

- [ ] **Step 4: Update project documentation**

Add to `README.md` that Mangoo has a persistent all-library notification center with aggregated task progress and local process logs. Add a `Unified Notification Center` section to `CLAUDE.md` covering:

```text
Core library DB -> notification operation head + Outbox
Desktop userData/notifications.sqlite -> all-library durable mirror
Preload -> typed snapshots, history, actions, and subscriptions
Web -> no raw fetch, no notification polling, no per-thumbnail notifications
```

Document new modules in `docs/project-structure.md` and state that every future background task must declare a logical grouping key and presentation class.

- [ ] **Step 5: Verify and commit**

Run:

```powershell
npm run check:structure
npm run check:web
npm run check:ui-design
rg -n "TaskPanel|TaskCenter|TaskOverlay|RecentOpsPanel|data-titlebar-control=.?tasks|recent-operations" apps/web/src tools/checks
```

Expected: all checks PASS; final `rg` returns no legacy presentation references.

Commit:

```powershell
git add -A apps/web/src/features/tasks apps/web/src/features/file-ops/RecentOpsPanel.tsx apps/web/src/core/useLibraryData.ts apps/web/src/styles.css tools/checks README.md CLAUDE.md docs/project-structure.md
git commit -m "refactor(web): retire separate tasks and recent surfaces"
```

---

### Task 16: Run performance, desktop interaction, and full verification gates

**Files:**
- Create: `tools/bench/notification-center-bench.mjs`
- Modify only for real defects found: files owned by Tasks 1-15
- Report: `docs/benchmarks/2026-07-20-notification-center-verification.md`

- [ ] **Step 1: Add a deterministic notification benchmark**

The benchmark seeds at least 10,000 thumbnail completions, 10,000 rapid progress updates, 2,100 completed operations, and active operations. Assert:

```js
assert.equal(perThumbnailNotificationRows, 0);
assert.ok(thumbnailAggregateRows <= libraryCount);
assert.ok(persistedProgressEvents <= activeOperationSeconds + terminalEventCount);
assert.ok(completedHistoryRows <= 2000);
assert.equal(activeRowsRetained, seededActiveRows);
```

Record elapsed store-worker time and event counts, not a machine-specific hard latency threshold.

- [ ] **Step 2: Run focused subsystem gates**

Run:

```powershell
npm run check:core-api
npm run check:core-client
npm run check:schema
npm run check:rust
npm run check:desktop
npm run check:web
npm run check:ui-design
node tools/bench/notification-center-bench.mjs
```

Expected: every command PASS and benchmark invariants hold.

- [ ] **Step 3: Run the real Electron OS-backdrop harness**

Run:

```powershell
$env:MANGOO_VISUAL_OS_BACKDROP='1'
node .tmp\visual-check\desktop-ui-regression.mjs
Remove-Item Env:\MANGOO_VISUAL_OS_BACKDROP
```

Expected: PASS for the compact popover, one-instance notification window, shared material, titlebar outline, no pane divider, and secondary-window reveal lifecycle.

- [ ] **Step 4: Perform the real interaction pass**

Using a real library, verify:

1. Start a root scan and confirm phase/progress appears without a new notification row per progress tick.
2. Remove a root and confirm determinate progress, final success before task disappearance, and restart recovery.
3. Run move/rename/delete batches and confirm one logical result per user command.
4. Generate/scroll thumbnails and confirm one aggregate summary and no per-item notifications.
5. Switch libraries and restart Mangoo; confirm all-library history persists with source labels.
6. Trigger a broad non-blocking error; confirm no center-pane banner and one deduplicated notification.
7. Open process logs; confirm Main/Core/Renderer sources, filters, pause/follow, copy, and export.
8. Browse, scroll, resize dividers, switch layouts, and preview media while tasks run; confirm no visible notification-induced stalls or request amplification.

- [ ] **Step 5: Run the final repository gate**

Run: `npm test`

Expected: PASS.

- [ ] **Step 6: Write the verification report and commit**

The report records commit range, commands, pass/fail output summary, benchmark event counts, real library used, task scenarios, visual evidence paths, and any residual risk. Do not claim a visual check that was not performed on the real Electron window.

Commit:

```powershell
git add tools/bench/notification-center-bench.mjs docs/benchmarks/2026-07-20-notification-center-verification.md
git commit -m "test: verify unified notification center"
```

---

## Final Review Checklist

- One icon-only titlebar notification button; no Tasks/Recent buttons.
- No read/unread state or badge count.
- Compact popover is read-only and shows all active progress, thumbnail aggregate, latest three important results, and full-center entry.
- Full window defaults to status and has history and current-process log pages.
- Cancel/retry appears only in the full center and only when valid.
- Root purge has determinate progress and a terminal event before self-deletion.
- File operations aggregate by user command.
- Thumbnail work never creates per-item notification records.
- History is global across libraries and survives restart.
- Core Outbox replay and Desktop cursor update are idempotent.
- Desktop SQLite runs in a worker and logs are bounded in memory.
- The center-pane global error strip is gone; contextual blocking UI remains.
- Settings/Plugins/Notifications share the approved secondary-window material and no pane divider.
- README, CLAUDE, and project structure describe the unified notification center.
- All narrow checks, benchmark invariants, real desktop interaction checks, OS-backdrop harness, and `npm test` pass.
