# Notification Failure Drilldown And Scan Aggregation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make thumbnail-cache failures traceable from Settings and Notifications to filtered process-log entries and the affected media, aggregate active folder scans into one useful status item, fix the empty process-log pipeline, and align the notification status dot.

**Architecture:** Core remains the authority for task identity, folder-relative paths, and thumbnail failure context. Core emits structured JSON tracing events; Desktop parses them into the existing bounded process-log buffer, enriches Core entries with the active library id, and owns launch/focus IPC between secondary windows and Main. Web consumes typed launch intents and structured log metadata, aggregates only the active `interactive_folder_scan` presentation while retaining independent underlying task operations, and reuses the existing library/media navigation state to locate a failed file.

**Tech Stack:** Rust + Axum + SQLite, Electron IPC/preload, React + TypeScript, `@megle/core-client`, `@megle/notification-contract`, TanStack Virtual, Node test runner.

---

## Frozen Behavior

- Clicking a non-zero **失败** metric in the full notification thumbnail card or Settings thumbnail status opens/reuses Notifications on the **进程日志** page with structured filters for `eventKind=thumbnail_failure` and that library. A zero count remains inert without a disabled visual treatment.
- The compact notification popover remains read-only. Its failure text may open the full filtered log page, but it must not add per-thumbnail notification rows or polling.
- Every thumbnail failure emitted during the current process carries `libraryId`, `fileId`, `folderId`, `rootId`, `relativePath`, `failureKind`, and the error detail as structured log metadata. Filtering must inspect metadata, not parse localized message text.
- Clicking a log row with valid media metadata focuses Main, switches to its library when necessary, opens the containing folder, loads/selects the exact file in the media viewport, and does not open preview. Rows without media metadata remain selectable text only.
- Process logs must show Main/Core/Renderer activity in a normal clean launch. Core tracing is initialized once with an info default and `RUST_LOG` override; no unbounded or per-thumbnail success logging is added.
- All active `interactive_folder_scan` operations are rendered as one aggregate status item. Its path list contains one line per active underlying operation and removes a line as soon as that operation becomes terminal. Underlying operations remain independent in Core persistence/history/action handling.
- Scan paths are library-relative and include the root display name to disambiguate multiple roots, for example `Artists\\Portraits\\Concept`. Root scan displays the root display name instead of `Root 7`. Long paths use one-line ellipsis and expose the complete path with `title`.
- The aggregate progress sums known current/total values. It is determinate only when every active underlying scan has a known total; otherwise it stays indeterminate. Aggregate cancellation, if shown in the full center, cancels every still-active underlying operation and reports partial failures without inventing a synthetic Core operation id.
- The notification status dot uses `left: 50%` plus horizontal translation so its center aligns with the Bell icon; vertical placement and existing colors/opacity stay unchanged.

## Task 1: Restore Structured Core Process Logs

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/core/Cargo.toml`
- Modify: `crates/core/src/main.rs`
- Modify: `crates/core/src/tasks.rs`
- Modify: `apps/desktop/src/core-process.ts`
- Modify: `apps/desktop/src/main.ts`
- Modify: `apps/desktop/src/process-log-buffer.ts`
- Modify: `packages/notification-contract/src/index.ts`
- Test: `apps/desktop/tests/process-log-buffer.test.mts`
- Test: focused Rust task tests in `crates/core/src/tasks.rs`

- [ ] **Step 1: Add failing tests for structured Core parsing and metadata filtering**

Test a JSON tracing line such as:

```json
{"timestamp":"2026-07-24T12:00:00.000Z","level":"ERROR","fields":{"message":"thumbnail generation failed","eventKind":"thumbnail_failure","fileId":42,"folderId":7,"rootId":3,"relativePath":"Artists\\Portraits\\a.jpg","failureKind":"permanent","error":"decode failed"},"target":"megle_core::tasks"}
```

Assert Desktop maps it to an error log with those metadata fields, adds the launch-time `libraryId`, and `ProcessLogBuffer.page({ eventKind: "thumbnail_failure", libraryId: "lib-a" })` returns it while excluding unrelated errors.

- [ ] **Step 2: Verify the focused tests fail before implementation**

Run: `npm --workspace @megle/desktop run test -- --test-name-pattern="structured Core|metadata filter"`

- [ ] **Step 3: Initialize Core tracing and emit one structured failure event**

Add `tracing-subscriber` with `fmt`, `json`, and `env-filter`. Initialize it at the start of `main` with `RUST_LOG` support and an `info` fallback. In the existing thumbnail error handler, resolve the current task/file context once and emit one `tracing::error!` event with the frozen metadata fields. Do not log successful thumbnails.

- [ ] **Step 4: Parse structured Core output without breaking legacy text output**

Extend the Core log callback to accept optional metadata. Parse only valid JSON objects with a recognized level and string message; malformed/non-JSON lines continue through the existing text classifier. Main enriches parsed Core metadata with the library id captured for that Core process before appending to `desktopLogger`.

- [ ] **Step 5: Add typed metadata filters**

Extend `ProcessLogFilter` with optional `eventKind` and `libraryId`. Validate both as bounded strings in Desktop IPC and match them against exact metadata values in `ProcessLogBuffer`. Keep search/source/level behavior unchanged.

- [ ] **Step 6: Run focused tests**

Run: `cargo test -p megle-core thumbnail_error_handler_records_failed_with_attempt_fingerprint_when_source_matches -- --nocapture`

Run: `npm --workspace @megle/desktop run test`

## Task 2: Add Notification Launch Intent And Main Media Focus IPC

**Files:**
- Modify: `packages/notification-contract/src/index.ts`
- Modify: `apps/desktop/src/notification-ipc-channels.cjs`
- Modify: `apps/desktop/src/notification-ipc.ts`
- Modify: `apps/desktop/src/preload.cjs`
- Modify: `apps/desktop/src/main.ts`
- Modify: `apps/web/src/core/desktop.ts`
- Modify: `apps/web/src/features/notifications/NotificationWindow.tsx`
- Modify: `apps/web/src/features/notifications/ProcessLogView.tsx`
- Modify: `apps/web/src/app/App.tsx`
- Modify: `apps/web/src/core/useLibraryData.ts`
- Test: `apps/desktop/tests/notification-integration.test.mts`
- Test: `apps/web/tests/notificationModel.test.mts`

- [ ] **Step 1: Add failing contract and reducer tests**

Define bounded, discriminated inputs:

```ts
type NotificationWindowIntent = {
  page: "logs";
  logFilter: { eventKind: "thumbnail_failure"; libraryId: string };
};

type MediaFocusRequest = {
  libraryId: string;
  fileId: number;
  folderId: number;
  rootId: number;
};
```

Test opening a new notification window and reusing an existing one both deliver the latest intent exactly once. Test only trusted `main/settings/plugins/notifications` roles can issue focus requests and only Main receives them.

- [ ] **Step 2: Verify the focused tests fail**

Run: `npm --workspace @megle/desktop run test -- --test-name-pattern="notification intent|media focus"`

- [ ] **Step 3: Implement typed open/reuse intent delivery**

Change `openNotificationsWindow(intent?)` across Web, preload, and Main. A reused window receives an IPC event and focuses; a newly created window receives the intent after `did-finish-load`. `NotificationWindow` changes page to `logs` and passes the filter to `ProcessLogView`. Direct user tab changes clear the one-shot intent without clearing manually selected filters.

- [ ] **Step 4: Implement media-focus delivery to Main**

Add a validated `focusMedia` bridge method for Settings/Notifications and an `onFocusMedia` subscription only on Main. Main focuses/restores its BrowserWindow and forwards the request. Never expose a generic arbitrary IPC send surface.

- [ ] **Step 5: Locate the exact media without opening preview**

Add a `LibraryState.locateMedia(request)` operation. It switches library through the existing library lifecycle when needed, loads `getMedia(fileId)`, reveals/navigates to its folder, applies a bounded exact-name search scoped to that folder when the file is outside the loaded media window, selects `fileId`, and lets MediaGrid's existing selection-centering effect bring it into view. Preserve the request until the switched library reaches ready; discard it if the file no longer exists. Do not call the preview-open handler.

- [ ] **Step 6: Make structured log rows actionable**

Rows whose metadata passes numeric/id validation render as buttons with a clear accessible label. Clicking calls `focusMedia`; other rows retain the existing non-button markup and text-selection behavior.

- [ ] **Step 7: Run focused Desktop and Web tests**

Run: `npm run check:notification-contract`

Run: `npm run check:desktop`

Run: `npm run check:web`

## Task 3: Wire Thumbnail Failure Entry Points

**Files:**
- Modify: `apps/web/src/features/notifications/NotificationStatusView.tsx`
- Modify: `apps/web/src/features/notifications/NotificationPopover.tsx`
- Modify: `apps/web/src/features/settings/settingsPanels.tsx`
- Modify: `apps/web/src/features/settings/SettingsWindow.tsx` or the current settings composition owner
- Modify: `apps/web/src/styles.css`
- Test: `apps/web/tests/notificationModel.test.mts`

- [ ] **Step 1: Add failing view-model tests**

Test that a non-zero failed count yields `{ eventKind: "thumbnail_failure", libraryId }`, while zero yields no action. Test Settings uses the current `thumbnailCacheStatsLibraryId` rather than an inferred name.

- [ ] **Step 2: Implement shared failure intent helper**

Create one pure helper in the notification feature and reuse it from full status, compact popover, and Settings. Do not duplicate intent literals or add polling.

- [ ] **Step 3: Make only the failed metric interactive**

Use a semantic button inside the metric cell/card, retain current typography/layout, opt into the global pointer affordance rather than adding custom hover borders, and call `openNotificationsWindow(intent)`. A zero count renders the existing plain metric.

- [ ] **Step 4: Verify Web and UI design checks**

Run: `npm run check:web`

Run: `npm run check:ui-design`

## Task 4: Aggregate Active Folder Scans And Publish Relative Paths

**Files:**
- Modify: `crates/core/src/db/mod.rs`
- Modify: `apps/web/src/features/notifications/notificationModel.ts`
- Modify: `apps/web/src/features/notifications/notificationPopoverPresentation.ts`
- Modify: `apps/web/src/features/notifications/NotificationPopover.tsx`
- Modify: `apps/web/src/features/notifications/NotificationStatusView.tsx`
- Modify: `apps/web/src/styles.css`
- Test: focused Core notification tests in `crates/core/src/db/mod.rs`
- Test: `apps/web/tests/notificationModel.test.mts`

- [ ] **Step 1: Add failing Core summary tests**

Seed two roots and nested folders. Assert root scan summary equals the root display name and interactive folder scan summary equals `Root display name\\child\\grandchild`, never an absolute path or `Root <id>`.

- [ ] **Step 2: Implement a transaction-local relative-path resolver**

Build the display path from the folder ancestor chain plus root display name inside the same notification transaction. Use it for `root_scan` and `interactive_folder_scan`; leave root purge wording aligned with its existing remove-library semantics.

- [ ] **Step 3: Add failing aggregate model tests**

Given three active folder scans and one root scan, assert the model contains one folder-scan aggregate plus the root scan, preserves three underlying identities, exposes three ordered unique path lines, and removes only the completed path on the next snapshot. Test known-total summing and indeterminate fallback.

- [ ] **Step 4: Implement presentation-only aggregation**

Aggregate after operation deduplication and active-status filtering. Keep history and Desktop storage untouched. Add explicit aggregate source-operation references so full-center cancellation fans out to real operation ids and reports any partial failure.

- [ ] **Step 5: Render paths above progress**

Render each active path immediately above the progress track. Use `overflow: hidden`, `text-overflow: ellipsis`, `white-space: nowrap`, and `title={fullPath}`. Keep compact height bounded; when many scans are active, show the first bounded set plus a `+N` summary rather than growing the popover without limit. Full center may show the complete scrollable list.

- [ ] **Step 6: Run focused Core/Web tests**

Run: `cargo test -p megle-core notification -- --nocapture`

Run: `npm run check:web`

## Task 5: Align Indicator And Complete Verification

**Files:**
- Modify: `apps/web/src/styles.css`
- Modify as needed: `tools/checks/validate-web-client.mjs`
- Modify as needed: `tools/checks/validate-ui-design.mjs`
- Modify as needed: `.tmp/visual-check/desktop-ui-regression.mjs`

- [ ] **Step 1: Align the dot without changing its visual tone**

Replace horizontal `right` positioning with:

```css
.notification-status-dot {
  left: 50%;
  right: auto;
  transform: translateX(-50%);
}
```

Keep the existing `top`, size, opacity, active color, and failure color.

- [ ] **Step 2: Add structural guards for the new contracts**

Checks must require structured filters, explicit launch/focus IPC, aggregate folder-scan behavior, relative-path rendering, and centered dot positioning. Do not relax existing notification, pointer, process-log, or no-per-thumbnail-notification guards.

- [ ] **Step 3: Run complete verification**

Run: `npm test`

Run:

```powershell
$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs; Remove-Item Env:\MEGLE_VISUAL_OS_BACKDROP
```

- [ ] **Step 4: Perform a real desktop interaction pass**

Verify: clean launch logs are non-empty; Settings failure click opens filtered logs; notification failure click reuses the same window; clicking a failure row focuses and selects the exact file without preview; simultaneous folder scans render one card with live path removal; long paths ellipsize with full tooltip; Bell dot centers correctly.

