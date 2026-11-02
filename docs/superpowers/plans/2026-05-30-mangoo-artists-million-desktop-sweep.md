# Mangoo Artists Million Desktop Sweep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run a real Electron desktop sweep against `Y:\Repository\Billfish\Artists` with 200 deterministic browsing operations, measure folder/media/preview responsiveness, and fix measured bottlenecks.

**Architecture:** Reuse the existing Electron dev harness pattern: isolated SQLite DB, isolated Electron user-data dir, `MANGOO_AUTO_ADD_ROOT`, and CDP remote debugging. The sweep records every operation with elapsed time, long-task data, DOM state, and Core API timing so bottlenecks can be traced to UI rendering, Core paging, scan/disclosure, thumbnail generation, or preview loading.

**Tech Stack:** Electron, React, Vite, Rust Core, SQLite WAL, Chrome DevTools Protocol, Node.js `ws`, repo validation scripts.

---

### Task 1: Add The Artists Desktop Sweep Harness

**Files:**
- Create: `tools/dev/artists-million-desktop-sweep.mjs`
- Output: `.tmp/visual-check/logs/artists-million-desktop-sweep-summary.json`
- Output: `.tmp/visual-check/logs/artists-million-desktop-sweep.stdout.log`
- Output: `.tmp/visual-check/logs/artists-million-desktop-sweep.stderr.log`

- [ ] **Step 1: Write the harness**

Create a Node script that:

```js
const autoRoot = process.env.MANGOO_ARTISTS_SWEEP_ROOT ?? "Y:\\Repository\\Billfish\\Artists";
const targetOperationCount = Number(process.env.MANGOO_ARTISTS_SWEEP_OPERATION_COUNT ?? 200);
const webUrl = process.env.MANGOO_ARTISTS_SWEEP_WEB_URL ?? "http://127.0.0.1:5181";
const debugPort = Number(process.env.MANGOO_ARTISTS_SWEEP_DEBUG_PORT ?? 9251);
```

It must start `npm run dev` with:

```js
{
  MANGOO_WEB_URL: webUrl,
  MANGOO_DB_PATH: path.join(dataDir, "mangoo.sqlite"),
  MANGOO_ELECTRON_USER_DATA_DIR: electronUserDataDir,
  MANGOO_AUTO_ADD_ROOT: autoRoot,
  MANGOO_REMOTE_DEBUG: "1",
  MANGOO_REMOTE_DEBUG_PORT: String(debugPort)
}
```

It must connect to the Electron page through CDP, install a `PerformanceObserver` for `longtask`, then run operation categories:

```js
[
  "startup",
  "root initial display",
  "layout adaptive",
  "layout waterfall",
  "layout grid",
  "layout list",
  "toggle recursive child contents on",
  "toggle recursive child contents off",
  "subfolder collapse",
  "subfolder expand",
  "folder click",
  "folder history back",
  "folder history forward",
  "grid scroll small",
  "grid scroll medium",
  "grid scroll deep",
  "select visible media",
  "open preview",
  "preview next",
  "preview previous",
  "close preview",
  "filter images on/off",
  "sort name asc",
  "sort newest",
  "search set/clear"
]
```

Repeat folder, scroll, layout, child-content, preview, and search actions until exactly 200 operation records are written.

- [ ] **Step 2: Include per-operation assertions**

Each operation record must include:

```js
{
  index,
  label,
  ok,
  elapsedMs,
  longTaskCount,
  longTaskDurationMs,
  dom: {
    treeItems,
    subfolderCards,
    mediaTiles,
    readyThumbs,
    loadingThumbs,
    previewOpen,
    layoutClass,
    scrollTop
  },
  api: {
    mediaPageMs,
    folderChildrenMs
  },
  error
}
```

The harness should fail when:

```js
elapsedMs > 2500 for folder switching, layout switching, search clear, or preview close
elapsedMs > 3500 for preview open or deep scroll
longTaskDurationMs > 1000 for any single operation
document.querySelector(".virtual-grid") is missing while not in preview
document.querySelector(".central-preview-stage") is missing after preview open
```

- [ ] **Step 3: Run the baseline**

Run:

```powershell
node tools/dev/artists-million-desktop-sweep.mjs
```

Expected before fixes: the script completes all 200 operations or fails with a summary pointing to the first operation that violates a threshold.

### Task 2: Fix The Measured Bottleneck

**Files:**
- Modify only the file(s) implicated by the failed operation evidence.

- [ ] **Step 1: Identify the layer**

Use summary fields:

```js
if api media/folder calls are slow -> inspect Core/API/DB query path
if api is fast but elapsed/longtask is slow -> inspect React layout/render path
if preview open is slow with fast api -> inspect preview image sizing/loading path
if scroll is slow -> inspect virtualization/windowed-layout path
```

- [ ] **Step 2: Add a validator or focused regression probe**

Add a static validator when the failure is a known contract regression, or keep the Artists sweep as the executable regression when the issue depends on real data.

- [ ] **Step 3: Patch the root cause**

Use `apply_patch` for manual edits. Do not revert unrelated dirty files.

### Task 3: Verify And Report

**Files:**
- Read: `.tmp/visual-check/logs/artists-million-desktop-sweep-summary.json`

- [ ] **Step 1: Rerun the real desktop sweep**

Run:

```powershell
node tools/dev/artists-million-desktop-sweep.mjs
```

Expected: 200 operation records, no failed operation, no renderer console errors.

- [ ] **Step 2: Run narrow repo checks**

Run:

```powershell
npm run check:web
npm run check:ui-design
```

Expected: both pass.

- [ ] **Step 3: Report evidence**

Final report must include:

```text
200 operation count
slowest 10 operations
folder switch p50/p95/max
scroll p50/p95/max
preview open p50/p95/max
layout switch p50/p95/max
any remaining bottlenecks or skipped checks
```
