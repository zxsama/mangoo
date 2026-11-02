# Mangoo Real Desktop UI Regression Sweep Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Verify Mangoo's current UI interaction quality on the real Electron desktop app, cover as many major user flows as practical, and fix confirmed regressions with one subagent at a time.

**Architecture:** Use the existing Electron visual harness and remote-debugging path for repeatable real-desktop checks, then layer targeted scripted/manual interaction passes over the running desktop app. Treat each failure as a debugging task: reproduce, isolate root cause, fix with a single subagent, and re-run the affected desktop flow plus a focused regression subset.

**Tech Stack:** Electron desktop shell, React + TypeScript + Vite UI, Rust core, PowerShell, existing `.tmp/visual-check` harness, Chrome DevTools Protocol, repo validation scripts.

---

## File Map

- Read: `.tmp/visual-check/desktop-ui-regression.mjs`
- Read: `.tmp/visual-check/run-visual-check.mjs`
- Read: `tools/dev/run-dev.mjs`
- Modify if needed after verification: `.tmp/visual-check/desktop-ui-regression.mjs`
- Modify if needed after verification: `apps/web/src/**`
- Modify if needed after verification: `apps/desktop/src/**`
- Modify if needed after verification: `crates/core/src/**`
- Record evidence in: `.tmp/visual-check/logs/`
- Save screenshots in: `.tmp/visual-check/screenshots/`
- Save this plan in: `docs/superpowers/plans/2026-05-28-mangoo-real-desktop-ui-regression-sweep.md`

## Coverage Matrix

- Library shell startup and hydration
- Left sidebar root / folder tree selection
- Center titlebar controls: filter, sort, layout, refresh, search
- Subfolder strip and child-content toggle
- Media grid scrolling, selection, hover, double-click preview open
- Preview open / close / previous / next / actual-size toggle / back navigation
- Right inspector metadata shell visibility and selection updates
- Tasks / Recent compact popovers
- Context menus for root / folder / media
- Settings screen controls, especially grid controls added in this session
- Empty states / loading states / large-library responsive behavior

## Execution Tasks

### Task 1: Baseline Desktop Harness Check

**Files:**
- Read: `.tmp/visual-check/desktop-ui-regression.mjs`
- Read: `tools/dev/run-dev.mjs`
- Evidence: `.tmp/visual-check/logs/desktop-ui-regression-summary.json`

- [ ] **Step 1: Confirm current harness entrypoints and environment variables**

Run:

```powershell
Get-Content .tmp\visual-check\desktop-ui-regression.mjs -TotalCount 220
Get-Content tools\dev\run-dev.mjs -TotalCount 220
```

Expected: confirm `MANGOO_REMOTE_DEBUG_PORT`, `MANGOO_VISUAL_HARNESS`, `MANGOO_VISUAL_RUN_ID`, and Electron user-data isolation are available.

- [ ] **Step 2: Run the existing desktop regression harness once**

Run:

```powershell
node .tmp\visual-check\desktop-ui-regression.mjs
```

Expected: exit code `0`, refreshed screenshots under `.tmp\visual-check\screenshots\`, and a summary JSON under `.tmp\visual-check\logs\`.

- [ ] **Step 3: Read the summary and list current hard failures or blind spots**

Run:

```powershell
Get-Content .tmp\visual-check\logs\desktop-ui-regression-summary.json
```

Expected: identify what the harness already covers and what still requires manual/scripted interaction passes.

### Task 2: Real Desktop Interaction Sweep

**Files:**
- Use running desktop app from `npm run dev` or harness-launched app
- Evidence: `.tmp/visual-check/screenshots/*`
- Evidence notes: local session log / commentary updates

- [ ] **Step 1: Launch a clean real Electron session with isolated user data**

Run:

```powershell
$env:MANGOO_REMOTE_DEBUG='1'
$env:MANGOO_REMOTE_DEBUG_PORT='9239'
$env:MANGOO_ELECTRON_USER_DATA_DIR=(Resolve-Path .tmp\visual-check\manual-electron-user-data)
npm run dev
```

Expected: Vite, desktop build, Rust core, and Electron window all start successfully with a debuggable desktop session.

- [ ] **Step 2: Exercise startup, shell hydration, and root/folder browsing**

Checklist:

```text
1. Confirm window paints correctly and no startup blocker overlays remain.
2. Switch roots and folders repeatedly.
3. Verify scroll restoration, selection state, and subfolder strip/header alignment.
4. Verify browsing remains responsive during background scan/thumbnail activity.
```

- [ ] **Step 3: Exercise titlebar controls and content interactions**

Checklist:

```text
1. Open/close filter and sort menus.
2. Change layout modes: adaptive, waterfall, grid, list.
3. Type into search, clear search, and verify result count/title updates.
4. Toggle child-folder-content visibility and verify immediate content response.
```

- [ ] **Step 4: Exercise grid + preview interactions**

Checklist:

```text
1. Scroll shallow and deep.
2. Click media, keyboard navigate, double-click to open preview.
3. Use preview back, previous, next, and actual-size toggle.
4. Return to grid and verify scroll/selection persistence.
```

- [ ] **Step 5: Exercise right-panel and overlay interactions**

Checklist:

```text
1. Inspect metadata panel updates across selection changes.
2. Open Tasks and Recent popovers.
3. Open root/folder/media context menus.
4. Verify popover layering, blur, focus, and dismissal behavior.
```

- [ ] **Step 6: Exercise Settings interactions**

Checklist:

```text
1. Open Settings from the shell.
2. Verify the new Library grid controls persist and affect the library view.
3. Verify interface-style sliders still behave correctly.
4. Return to Library and confirm settings took effect.
```

### Task 3: Failure Triage And Root-Cause Capture

**Files:**
- Modify if needed: target files implicated by failures
- Evidence: `.tmp/visual-check/logs/*`, screenshots, reproduction notes

- [ ] **Step 1: Record each failure with exact reproduction**

Template:

```text
Failure:
Repro steps:
Expected:
Actual:
First suspected area:
```

- [ ] **Step 2: For each failure, isolate the owning layer before fixing**

Checklist:

```text
1. Shell/layout only?
2. Web interaction logic?
3. Desktop bridge / Electron behavior?
4. Core API / scan / thumbnail behavior?
```

- [ ] **Step 3: Only after root cause is clear, dispatch one subagent for that issue**

Constraint:

```text
One subagent at a time. Each subagent receives:
- exact repro
- expected result
- suspected files
- required verification command(s)
```

### Task 4: Fix Verification Loop

**Files:**
- Modify: issue-specific implementation files
- Verify: `npm run check:web`, `npm run check:desktop`, `npm run check:ui-design`, focused Rust/core-client tests as needed

- [ ] **Step 1: After each fix, run the narrowest matching checks**

Command set:

```powershell
npm run check:web
npm run check:desktop
npm run check:ui-design
```

Add focused commands when required:

```powershell
npm run check:core-client
cargo test -p mangoo-core <focused_test_name> -- --nocapture
```

- [ ] **Step 2: Re-run the exact desktop repro that previously failed**

Expected: the original failure no longer reproduces.

- [ ] **Step 3: Re-run nearby interaction flows to guard regressions**

Checklist:

```text
If folder switching was fixed, also verify search, preview return, and subfolder toggle.
If preview was fixed, also verify grid selection, keyboard navigation, and inspector updates.
If popover layering was fixed, also verify Tasks, Recent, and titlebar menus.
```

### Task 5: Final Regression Pass

**Files:**
- Evidence: `.tmp/visual-check/logs/desktop-ui-regression-summary.json`
- Evidence: refreshed screenshots and manual notes

- [ ] **Step 1: Run the harness again after fixes**

Run:

```powershell
node .tmp\visual-check\desktop-ui-regression.mjs
```

- [ ] **Step 2: Run one final manual/scripted desktop pass over the highest-risk flows**

Checklist:

```text
1. Folder switch during load
2. Deep scroll + selection + preview return
3. Child-folder-content toggle
4. Tasks/Recent/settings popovers
5. Layout switching + persisted grid controls
```

- [ ] **Step 3: Summarize verified passes, remaining gaps, and any blocked areas**

Required output:

```text
- Verified flows
- Remaining known issues
- Commands run
- Desktop-only risks not fully automated yet
```
