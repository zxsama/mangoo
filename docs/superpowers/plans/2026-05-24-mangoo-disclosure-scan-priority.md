# Mangoo Dynamic Priority Disclosure Scan Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make new roots and large folders browsable during scanning, keep folder switching responsive, and prioritize clear previews for the current view instead of doing heavy preview work inside the root scan loop.

**Architecture:** Split the current “scan + preview prep” path into a fast ingest scan and a UI-driven preview priority path. Root scanning should quickly persist folders/files/media rows, while the current root/current folder refresh loop and visible-item preview requests drive `grid_320` generation and original preview prefetch outside the scan critical path.

**Tech Stack:** Rust + Axum + rusqlite, TypeScript + React + Electron/Vite, existing `@mangoo/core-client`, repo checks under `tools/checks`, and real-directory smoke scripts under `tools/dev`.

---

### Task 1: Remove Synchronous Preview Work From Root Scan

**Files:**
- Modify: `crates/core/src/scan/mod.rs`
- Modify: `crates/core/src/db/mod.rs`
- Test: `crates/core/src/scan/mod.rs`

- [ ] **Step 1: Write the failing scan tests**

```rust
#[test]
fn scan_root_does_not_generate_preview_placeholder_inline() {
    let temp_root = unique_temp_dir();
    fs::create_dir_all(&temp_root).expect("create media root");
    write_test_image(&temp_root.join("image.jpg"), 800, 400);

    let mut database = Database::open_in_memory().expect("open database");
    database.apply_migrations().expect("apply migrations");
    let root_id = add_test_root(&database, &temp_root, "disclosure-scan");
    let root = database.get_root(root_id).expect("get root").expect("root exists");

    scan_root(&mut database, &root).expect("scan root");

    let media = database.get_media(1).expect("get media").expect("media exists");
    assert!(media.preview_placeholder.is_none());
    let source = database
        .get_thumbnail_source(media.id)
        .expect("get source")
        .expect("source exists");
    assert_eq!(source.metadata_status.as_deref(), Some("pending"));
}
```

- [ ] **Step 2: Run the focused test to verify RED**

Run: `cargo test -p mangoo-core scan::tests::scan_root_does_not_generate_preview_placeholder_inline -- --exact`
Expected: FAIL because `probe_image_dimensions()` still generates placeholders and sets metadata ready during scan.

- [ ] **Step 3: Remove the synchronous post-flush probe from the ingest path**

```rust
fn flush_scan_files(
    database: &mut Database,
    root_id: i64,
    scan_generation: i64,
    options: &mut ScanOptions<'_>,
    pending_files: &mut Vec<ScanFileUpsert>,
    pending_file_paths: &mut Vec<Option<PathBuf>>,
) -> anyhow::Result<()> {
    if pending_files.is_empty() {
        pending_file_paths.clear();
        return Ok(());
    }

    observe_scan_batch(options, 0, pending_files.len());
    check_scan_cancelled(options)?;
    ensure_root_enabled(database, root_id)?;
    pending_file_paths.clear();
    commit_scan_batch_with_optional_task_guard(
        database,
        ScanWriteBatch {
            folders: Vec::new(),
            files: std::mem::take(pending_files),
            scan_generation: Some(scan_generation),
        },
        options,
    )?;
    Ok(())
}
```

- [ ] **Step 4: Remove or quarantine scan-only placeholder generation helpers**

```rust
// crates/core/src/scan/mod.rs
// Delete probe_image_dimensions(...) from the root scan critical path.
// Keep placeholder generation available for later preview/background work,
// but do not call it from root_scan_with_options().
```

- [ ] **Step 5: Run focused scan tests to verify GREEN**

Run: `cargo test -p mangoo-core scan::tests::scan_root_does_not_generate_preview_placeholder_inline -- --exact`
Expected: PASS.

Run: `cargo test -p mangoo-core scan::tests -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/scan/mod.rs crates/core/src/db/mod.rs
git commit -m "refactor(scan): keep root scan on fast ingest path"
```

### Task 2: Refresh Current Root And Folder Incrementally During Scans

**Files:**
- Modify: `apps/web/src/core/useLibraryData.ts`
- Modify: `apps/web/src/features/library/LibrarySidebar.tsx`
- Modify: `tools/checks/validate-web-client.mjs`

- [ ] **Step 1: Write the failing validator checks**

```javascript
if (!/scanActive/.test(useLibraryData) || !/loadTasks/.test(useLibraryData)) {
  fail("useLibraryData must track active root scans");
}
if (!/selectedFolderId/.test(useLibraryData) || !/listMedia/.test(useLibraryData)) {
  fail("useLibraryData must reload current folder media while scanning");
}
if (!/setInterval/.test(useLibraryData) || !/loadFolderChildren/.test(useLibraryData)) {
  fail("useLibraryData must incrementally refresh current folder children during scan");
}
```

- [ ] **Step 2: Run the validator to verify RED**

Run: `node tools/checks/validate-web-client.mjs`
Expected: FAIL because scanning currently only polls tasks and reloads the library on task success.

- [ ] **Step 3: Add a scan-time incremental refresh loop**

```typescript
useEffect(() => {
  if (!scanActive || selectedRootId === null) {
    return;
  }

  const timer = window.setInterval(() => {
    const currentFolder = selectedFolderId;
    void loadTasks();
    if (currentFolder !== null) {
      void loadFolderChildren(currentFolder);
      void reloadCurrentFolderMedia(currentFolder);
    } else {
      void reloadCurrentRootMedia(selectedRootId);
    }
  }, 800);

  return () => window.clearInterval(timer);
}, [scanActive, selectedRootId, selectedFolderId, loadTasks, loadFolderChildren, reloadCurrentFolderMedia, reloadCurrentRootMedia]);
```

- [ ] **Step 4: Keep folder switching immediate during scan**

```typescript
setSelectedFolder: (folder: FolderRecord) => {
  mediaPageGeneration.current += 1;
  selectRoot(folder.rootId);
  selectFolder(folder.id);
  void reloadCurrentFolderMedia(folder.id);
}
```

- [ ] **Step 5: Run web typecheck and validator to verify GREEN**

Run: `node tools/checks/validate-web-client.mjs`
Expected: PASS.

Run: `npm --workspace @mangoo/web run typecheck`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/web/src/core/useLibraryData.ts apps/web/src/features/library/LibrarySidebar.tsx tools/checks/validate-web-client.mjs
git commit -m "feat(web): refresh current folder during root scan"
```

### Task 3: Prioritize Current-View `grid_320` And Center Original Work

**Files:**
- Modify: `apps/web/src/core/mediaResources.ts`
- Modify: `apps/web/src/features/media-grid/MediaGrid.tsx`
- Modify: `apps/web/src/features/preview/MediaPreview.tsx`
- Modify: `apps/web/src/app/App.tsx`
- Modify: `tools/checks/validate-web-client.mjs`
- Modify: `tools/checks/validate-ui-design.mjs`

- [ ] **Step 1: Write the failing validator checks**

```javascript
if (!/prefetchOriginalPreview/.test(appSource) || !/selectedMediaIndex/.test(appSource)) {
  fail("App must prefetch previous/next original preview items while center preview is open");
}
if (!/requestThumbnailStates\\(visibleMediaIds\\)/.test(mediaGrid)) {
  fail("MediaGrid must continue to prioritize visible items");
}
if (!/source=\"thumbnail\"/.test(previewPanel) || !/source=\"original\"/.test(centralPreviewStage)) {
  fail("Preview split between right panel and center preview must remain intact");
}
```

- [ ] **Step 2: Run the validators to verify RED**

Run: `node tools/checks/validate-web-client.mjs`
Expected: FAIL until current-view and center-prefetch assertions are satisfied against the new disclosure-flow logic.

- [ ] **Step 3: Keep visible-item thumbnail requests as the highest thumbnail priority**

```typescript
const visibleMedia = useMemo(() => {
  // visible IDs + signatures already computed
}, [rows, virtualItems]);

useEffect(() => {
  onRequestThumbnailStates(visibleMedia.ids);
}, [onRequestThumbnailStates, visibleMedia.key, visibleMedia.signatureKey]);
```

- [ ] **Step 4: Add bounded center original prefetch and reuse**

```typescript
useEffect(() => {
  if (!previewOpen || selectedMediaIndex < 0) return;
  const previous = library.media[selectedMediaIndex - 1];
  const next = library.media[selectedMediaIndex + 1];
  if (previous) prefetchOriginalPreview(previous);
  if (next) prefetchOriginalPreview(next);
}, [library.media, previewOpen, selectedMediaIndex]);
```

- [ ] **Step 5: Run web checks to verify GREEN**

Run: `node tools/checks/validate-web-client.mjs`
Expected: PASS.

Run: `node tools/checks/validate-ui-design.mjs`
Expected: PASS.

Run: `npm --workspace @mangoo/web run typecheck`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/web/src/core/mediaResources.ts apps/web/src/features/media-grid/MediaGrid.tsx apps/web/src/features/preview/MediaPreview.tsx apps/web/src/app/App.tsx tools/checks/validate-web-client.mjs tools/checks/validate-ui-design.mjs
git commit -m "feat(web): prioritize current disclosure preview work"
```

### Task 4: Benchmark And Verify Against The Real Outputs Directory

**Files:**
- Modify: `tools/dev/real-load-test.mts`
- Modify: `docs/release-checklist.md`

- [ ] **Step 1: Add failing smoke assertions for disclosure-scan behavior**

```typescript
// tools/dev/real-load-test.mts
// Validate:
// - current folder becomes visible before root scan succeeds
// - placeholders are present before thumbnails settle
// - center preview uses original media
```

- [ ] **Step 2: Run the full suite before smoke**

Run: `npm test`
Expected: PASS.

- [ ] **Step 3: Run the real-directory benchmark smoke**

Run: `node --experimental-strip-types tools/dev/real-load-test.mts`
Expected: PASS against `G:\\AI_Painter\\stable-diffusion\\stable-diffusion-webui\\outputs` with recorded timing for:
- add root to first visible media
- visible media to clear `grid_320`
- center preview to original

- [ ] **Step 4: Run a bounded dev smoke**

Run: `npm run dev`
Expected: App boots, current folder is switchable while root scan is still running, placeholders appear first, right panel stays on thumbnail path, center preview stays on original path.

- [ ] **Step 5: Re-run `npm test` if smoke required code changes**

Run: `npm test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add tools/dev/real-load-test.mts docs/release-checklist.md
git commit -m "test: verify disclosure scan priority flow"
```
