# Mangoo Global Priority Disclosure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the current viewport and clicked media refresh first by adding a front-channel interactive folder scan, viewport-driven thumbnail priority, and a higher-throughput background scan default.

**Architecture:** Keep the existing background `root_scan` as the long-running ingest path, but raise its default batch to `256` based on the real-directory benchmark. Add a new `interactive_folder_scan` task path that only accelerates the currently viewed folder, then wire the web client to continuously publish current viewport scope so clicked item, visible tiles, and ahead-of-scroll tiles get higher thumbnail refresh priority than background work.

**Tech Stack:** Rust + Axum + rusqlite task scheduler, TypeScript + React + Electron/Vite client, existing `@mangoo/core-client`, repo validators under `tools/checks`, and real-directory smoke coverage in `tools/dev/real-load-test.mts`.

---

### Task 1: Raise Background Scan Default And Add Interactive Scan Batch Profiles

**Files:**
- Modify: `crates/core/src/scan/mod.rs`
- Modify: `crates/core/src/tasks.rs`
- Test: `crates/core/src/scan/mod.rs`

- [ ] **Step 1: Write the failing scan tests**

```rust
#[test]
fn scan_options_default_uses_background_batch_256() {
    assert_eq!(ScanOptions::default().write_batch_size, 256);
}

#[test]
fn interactive_scan_priority_uses_single_file_batches() {
    let options = ScanOptions::for_priority(ScanPriority::Interactive);
    assert_eq!(options.write_batch_size, 1);
}
```

- [ ] **Step 2: Run the focused tests to verify RED**

Run: `cargo test -p mangoo-core scan::tests::scan_options_default_uses_background_batch_256 scan::tests::interactive_scan_priority_uses_single_file_batches -- --exact`
Expected: FAIL because the default is still `10` and no interactive-priority constructor exists.

- [ ] **Step 3: Add explicit background vs interactive scan option constructors**

```rust
pub const DEFAULT_SCAN_WRITE_BATCH_SIZE: usize = 256;
pub const INTERACTIVE_SCAN_WRITE_BATCH_SIZE: usize = 1;

impl ScanOptions<'_> {
    pub fn for_priority(priority: ScanPriority) -> Self {
        let write_batch_size = match priority {
            ScanPriority::Interactive => INTERACTIVE_SCAN_WRITE_BATCH_SIZE,
            ScanPriority::Background => DEFAULT_SCAN_WRITE_BATCH_SIZE,
        };
        Self {
            write_batch_size,
            progress_callback: None,
            cancellation_callback: None,
            task_attempt_guard: None,
            #[cfg(test)]
            batch_observer: None,
        }
    }
}
```

- [ ] **Step 4: Use the explicit constructors in the task runner**

```rust
let options = match task.kind.as_str() {
    "interactive_folder_scan" => ScanOptions::for_priority(crate::scan::ScanPriority::Interactive),
    _ => ScanOptions::for_priority(crate::scan::ScanPriority::Background),
};
```

- [ ] **Step 5: Run scan tests to verify GREEN**

Run: `cargo test -p mangoo-core scan::tests -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/scan/mod.rs crates/core/src/tasks.rs
git commit -m "feat(scan): add background and interactive batch profiles"
```

### Task 2: Add Interactive Folder Scan Task Type And API

**Files:**
- Modify: `crates/core/src/db/mod.rs`
- Modify: `crates/core/src/tasks.rs`
- Modify: `crates/core/src/api/routes.rs`
- Modify: `packages/core-client/src/generated-contract.ts`
- Modify: `packages/core-client/src/client.ts`
- Test: `crates/core/src/api/routes.rs`
- Test: `crates/core/src/db/mod.rs`

- [ ] **Step 1: Write the failing task/API tests**

```rust
#[tokio::test]
async fn interactive_folder_scan_route_creates_interactive_task() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tasks/interactive-folder-scan")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"rootId":1,"folderId":2}"#))
                .unwrap(),
        )
        .await
        .expect("send request");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body: serde_json::Value = read_json(response).await;
    assert_eq!(body["accepted"], true);
    assert_eq!(body["taskId"].is_number(), true);
}

#[test]
fn db_lists_pending_interactive_folder_scans_in_scheduler_order() {
    let mut database = Database::open_in_memory().expect("open db");
    database.apply_migrations().expect("migrate");
    let first = database
        .create_interactive_folder_scan_task(1, 2)
        .expect("create first");
    let second = database
        .create_interactive_folder_scan_task(1, 3)
        .expect("create second");
    let pending = database
        .list_pending_interactive_folder_scan_task_ids()
        .expect("list pending");
    assert_eq!(pending, vec![first, second]);
}
```

- [ ] **Step 2: Run the focused tests to verify RED**

Run: `cargo test -p mangoo-core interactive_folder_scan_route_creates_interactive_task db::tests::db_lists_pending_interactive_folder_scans_in_scheduler_order -- --exact`
Expected: FAIL because the route, task kind, and DB helpers do not exist.

- [ ] **Step 3: Add DB helpers and task-kind support**

```rust
pub fn create_interactive_folder_scan_task(
    &self,
    root_id: i64,
    folder_id: i64,
) -> anyhow::Result<i64> {
    self.create_task(TaskCreate {
        kind: "interactive_folder_scan",
        priority: 200,
        root_id: Some(root_id),
        folder_id: Some(folder_id),
        file_id: None,
        thumbnail_source_fingerprint: None,
    })
}
```

- [ ] **Step 4: Add the API route and client contract**

```rust
.route(
    "/api/tasks/interactive-folder-scan",
    post(enqueue_interactive_folder_scan),
)
```

```ts
export type TaskKind = "root_scan" | "interactive_folder_scan" | "thumbnail";

enqueueInteractiveFolderScan: (rootId: number, folderId: number) =>
  request<AcceptedRootResponse>("/tasks/interactive-folder-scan", {
    method: "POST",
    body: JSON.stringify({ rootId, folderId })
  }),
```

- [ ] **Step 5: Execute the new task kind in the worker**

```rust
"interactive_folder_scan" => {
    database.mark_task_running_for_attempt(task_id, attempt_generation)?;
    let root_id = task.root_id.ok_or_else(|| anyhow::anyhow!("interactive scan missing root id"))?;
    let folder_id = task.folder_id.ok_or_else(|| anyhow::anyhow!("interactive scan missing folder id"))?;
    run_interactive_folder_scan_for_attempt(database, task_id, attempt_generation, root_id, folder_id)?;
}
```

- [ ] **Step 6: Run the new tests and task suite to verify GREEN**

Run: `cargo test -p mangoo-core interactive_folder_scan -- --nocapture`
Expected: PASS.

Run: `cargo test -p mangoo-core tasks::tests -- --nocapture`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/db/mod.rs crates/core/src/tasks.rs crates/core/src/api/routes.rs packages/core-client/src/generated-contract.ts packages/core-client/src/client.ts
git commit -m "feat(core): add interactive folder scan tasks"
```

### Task 3: Make Interactive Folder Scan Publish Current Folder Rows Before Background Scan

**Files:**
- Modify: `crates/core/src/scan/mod.rs`
- Modify: `crates/core/src/db/mod.rs`
- Test: `crates/core/src/scan/mod.rs`
- Test: `crates/core/src/db/mod.rs`

- [ ] **Step 1: Write the failing conflict/race tests**

```rust
#[test]
fn interactive_folder_scan_publishes_current_folder_rows_before_background_batch_threshold() {
    let temp_root = unique_temp_dir();
    fs::create_dir_all(temp_root.join("visible")).expect("create visible dir");
    fs::write(temp_root.join("visible").join("a.jpg"), b"a").expect("write a");
    fs::write(temp_root.join("visible").join("b.jpg"), b"b").expect("write b");

    let mut database = Database::open_in_memory().expect("open db");
    database.apply_migrations().expect("migrate");
    let root_id = add_test_root(&database, &temp_root, "interactive-priority");
    let root = test_root(&database, root_id);

    scan_folder_with_options(
        &mut database,
        &root,
        root.root_folder_id.expect("root folder id"),
        ScanOptions::for_priority(ScanPriority::Interactive),
    )
    .expect("interactive scan");

    let visible = database
        .list_media_page(MediaPageQuery {
            root_id: Some(root_id),
            folder_id: Some(only_child_named(&database, root.root_folder_id.unwrap(), "visible").id),
            limit: 10,
            cursor: None,
            sort: "name_asc".to_string(),
            kind: None,
        })
        .expect("list media");
    assert_eq!(visible.items.len(), 2);
}

#[test]
fn background_revisit_cannot_regress_interactive_folder_scan_publication() {
    // interactive path publishes current rows first, then background scan revisits
    // the same files; the final listing must stay current and non-empty.
}
```

- [ ] **Step 2: Run the focused tests to verify RED**

Run: `cargo test -p mangoo-core interactive_folder_scan_publishes_current_folder_rows_before_background_batch_threshold background_revisit_cannot_regress_interactive_folder_scan_publication -- --exact`
Expected: FAIL because there is no folder-scoped scan path yet.

- [ ] **Step 3: Add a folder-scoped scan entrypoint that reuses the fast ingest pipeline**

```rust
pub fn scan_folder_with_options(
    database: &mut Database,
    root: &RootRecord,
    folder_id: i64,
    mut options: ScanOptions<'_>,
) -> anyhow::Result<ScanSummary> {
    let folder = database
        .get_folder(folder_id)?
        .ok_or_else(|| anyhow::anyhow!("folder not found: {folder_id}"))?;
    let root_path = PathBuf::from(&root.path);
    let folder_path = database
        .resolve_folder_path(folder_id, &root_path)?
        .ok_or_else(|| anyhow::anyhow!("folder path missing: {folder_id}"))?;
    scan_walk_path(database, root, Some(folder_id), &folder_path, &mut options)
}
```

- [ ] **Step 4: Keep conflict resolution identity-based, not channel-based**

```rust
// The DB upsert path continues to trust file identity. If the interactive scan
// publishes first and the background scan reaches the same row later, the later
// write is allowed only when the same source identity is still current.
assert!(database.task_attempt_is_current(task_id, attempt_generation)?);
```

- [ ] **Step 5: Run the focused scan/db tests to verify GREEN**

Run: `cargo test -p mangoo-core interactive_folder_scan -- --nocapture`
Expected: PASS.

Run: `cargo test -p mangoo-core db::tests::stale_root_scan_attempt_cannot_commit_batch_or_mark_root_scanned_after_retry -- --exact`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/scan/mod.rs crates/core/src/db/mod.rs
git commit -m "feat(scan): publish interactive folder rows ahead of background scan"
```

### Task 4: Drive Clicked, Visible, And Ahead-Of-Viewport Priority From The Web Client

**Files:**
- Modify: `apps/web/src/core/useLibraryData.ts`
- Modify: `apps/web/src/core/mediaResources.ts`
- Modify: `apps/web/src/features/media-grid/MediaGrid.tsx`
- Modify: `apps/web/src/features/library/LibraryView.tsx`
- Modify: `apps/web/src/app/App.tsx`
- Modify: `tools/checks/validate-web-client.mjs`
- Test: `tools/checks/validate-web-client.mjs`

- [ ] **Step 1: Write the failing web validator checks**

```javascript
if (!/enqueueInteractiveFolderScan/.test(useLibraryData) || !/selectedFolderId/.test(useLibraryData)) {
  fail("useLibraryData must enqueue an interactive folder scan for the current folder");
}
if (!/visibleAhead/.test(mediaGrid) || !/aheadItemCount/.test(mediaGrid)) {
  fail("MediaGrid must compute an ahead-of-viewport prefetch scope");
}
if (!/selectedMedia\\.id/.test(useLibraryData) || !/requestThumbnailStates\\(\\[selectedMedia\\.id\\]\\)/.test(useLibraryData)) {
  fail("clicked media must stay above generic visible media priority");
}
```

- [ ] **Step 2: Run the validator to verify RED**

Run: `node tools/checks/validate-web-client.mjs`
Expected: FAIL because there is no interactive folder scan API usage or ahead-of-viewport scope.

- [ ] **Step 3: Publish viewport scope and enqueue the interactive folder scan**

```ts
const VISIBLE_AHEAD_ITEM_COUNT = 24;

useEffect(() => {
  if (selectedRootId === null || selectedFolderId === null) {
    return;
  }
  void client.enqueueInteractiveFolderScan(selectedRootId, selectedFolderId).catch((cause) => {
    setError(errorMessage(cause));
  });
}, [client, selectedFolderId, selectedRootId]);
```

- [ ] **Step 4: Compute clicked, visible, and ahead-of-viewport media IDs separately**

```ts
const priorityMedia = useMemo(() => {
  const visibleIds = collectVisibleMediaIds(rows, virtualItems);
  const aheadIds = collectAheadMediaIds(rows, virtualItems, VISIBLE_AHEAD_ITEM_COUNT);
  return {
    clickedIds: selectedMediaId ? [selectedMediaId] : [],
    visibleIds,
    aheadIds,
  };
}, [rows, selectedMediaId, virtualItems]);
```

- [ ] **Step 5: Tighten thumbnail refresh cadence for the current viewport**

```ts
useEffect(() => {
  onRequestThumbnailStates(priorityMedia.clickedIds);
  onRequestThumbnailStates(priorityMedia.visibleIds);
  if (priorityMedia.aheadIds.length > 0) {
    const timer = window.setTimeout(() => {
      onRequestThumbnailStates(priorityMedia.aheadIds);
    }, 150);
    return () => window.clearTimeout(timer);
  }
}, [onRequestThumbnailStates, priorityMedia]);
```

- [ ] **Step 6: Preserve stale-request rejection across folder switches and scroll changes**

```ts
const requestedMediaSignature = mediaContentSignature(mediaRecord);
if (currentMediaSignature !== requestedMediaSignature) {
  return removeThumbnailState(current, mediaRecord.id);
}
```

- [ ] **Step 7: Run web checks to verify GREEN**

Run: `node tools/checks/validate-web-client.mjs`
Expected: PASS.

Run: `npm --workspace @mangoo/web run typecheck`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add apps/web/src/core/useLibraryData.ts apps/web/src/core/mediaResources.ts apps/web/src/features/media-grid/MediaGrid.tsx apps/web/src/features/library/LibraryView.tsx apps/web/src/app/App.tsx tools/checks/validate-web-client.mjs
git commit -m "feat(web): drive viewport-based disclosure priority"
```

### Task 5: Verify Cross-Channel Consistency And Real-Directory Priority Behavior

**Files:**
- Modify: `tools/dev/real-load-test.mts`
- Modify: `docs/release-checklist.md`
- Test: `tools/dev/real-load-test.mts`

- [ ] **Step 1: Add failing smoke assertions for clicked, visible, and ahead scopes**

```ts
if (!results.clickedClearsBeforeVisible) {
  throw new Error("clicked media did not clear before generic visible scope");
}
if (!results.aheadScopeClearsBeforeBackground) {
  throw new Error("ahead-of-viewport scope did not clear before background thumbnails");
}
if (!results.folderSwitchInvalidatedOldScope) {
  throw new Error("old folder scope survived after interactive folder switch");
}
```

- [ ] **Step 2: Run the real-directory smoke to verify RED**

Run: `node --experimental-strip-types tools/dev/real-load-test.mts`
Expected: FAIL until the new priority behavior and assertions are wired in.

- [ ] **Step 3: Extend the smoke script to measure the new priority tiers**

```ts
log(`Clicked tile clear: ${formatMs(clickedClearMs)}`);
log(`Visible tier clear: ${formatMs(visibleClearMs)}`);
log(`Ahead tier first clear: ${formatMs(aheadClearMs)}`);
log(`Background first clear after request: ${formatMs(backgroundFirstClearMs)}`);
```

- [ ] **Step 4: Update the release checklist**

```md
- [ ] Confirm clicked media clears before other visible tiles.
- [ ] Confirm ahead-of-viewport tiles clear before background folders.
- [ ] Confirm switching folders invalidates the old interactive scope immediately.
```

- [ ] **Step 5: Run final verification**

Run: `node --experimental-strip-types tools/dev/real-load-test.mts`
Expected: PASS.

Run: `npm run check:web`
Expected: PASS.

Run: `npm run check:rust`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add tools/dev/real-load-test.mts docs/release-checklist.md
git commit -m "test: verify global priority disclosure flow"
```
