# Mangoo Global UI Material Layout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor Mangoo's global shell, Liquid Glass material, floating popovers, and preview stages so the workbench reads as one unified material system without root backing plates or framed media.

**Architecture:** Keep the existing React/Electron shell boundaries and refactor material ownership through CSS variables, `LiquidGlassSurface`, titlebar slots, and overlay coordination. Work sequentially with one implementation subagent at a time; each task ends with a review gate before the next task starts.

**Tech Stack:** React 18, TypeScript, Vite, Electron, CSS custom properties, existing Liquid Glass primitives, Lucide icons, Node-based static and visual checks.

---

## File Structure

- Modify: `D:/Mangoo/apps/web/src/styles.css`
  - Normalize Workbench Material Layer tokens, transparent root rules, joined titlebar/content edges for left, center, and right regions, icon-only middle titlebar buttons, popover material, right preview stage sizing, and center preview unframing.
- Modify: `D:/Mangoo/apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`
  - Ensure material blur, pointer glow, and edge highlight variables apply consistently to glass borders and surfaces.
- Modify: `D:/Mangoo/apps/web/src/features/settings/interfaceStyle.ts`
  - Ensure blur/highlight sliders map to CSS variables used by major surfaces.
- Modify: `D:/Mangoo/apps/web/src/app-shell/AppShell.tsx`
  - Preserve transparent root shell and support fused left, center, and right material regions without adding a root/app-shell backing plate.
- Modify: `D:/Mangoo/apps/web/src/app-shell/ShellTopBar.tsx`
  - Make middle titlebar buttons icon-only/unframed, keep accessible labels, right-align search, and center preview title.
- Modify: `D:/Mangoo/apps/web/src/app-shell/ShellOverlayHost.tsx`
  - Coordinate one active compact popover among Tasks, Recent, Filter, and Sort where state currently lives in the shell.
- Modify: `D:/Mangoo/apps/web/src/features/library/FilterMenu.tsx`
  - Match floating popover material and shared close behavior.
- Modify: `D:/Mangoo/apps/web/src/features/library/SortMenu.tsx`
  - Match floating popover material and shared close behavior.
- Modify: `D:/Mangoo/apps/web/src/features/preview/PreviewPanel.tsx`
  - Keep right preview stage around `260px` and center successful media.
- Modify: `D:/Mangoo/apps/web/src/features/preview/CentralPreviewStage.tsx`
  - Remove center preview frame/padding and expose or preserve titlebar title state.
- Modify: `D:/Mangoo/apps/web/src/features/tasks/TaskOverlay.tsx`
  - Keep Tasks floating material aligned with Recent/Filter/Sort and close compact popovers when center Task Center opens.
- Modify: `D:/Mangoo/tools/checks/validate-ui-design.mjs`
  - Encode the new global material/layout contract.
- Modify: `D:/Mangoo/.tmp/visual-check/desktop-ui-regression.mjs`
  - Update screenshots, startup log checks, blur/material assertions, popover dismissal checks, and preview assertions when the file exists.

Execution discipline: the worktree may contain unrelated user or agent changes. At the start of each task, run `git status --short` only to observe. Do not stage, commit, reset, checkout, or revert unrelated changes. Execute tasks in order with one subagent active at a time.

## Task 1: Normalize Material Tokens And Glass Blur/Highlight Behavior

**Files:**
- Modify: `D:/Mangoo/apps/web/src/styles.css`
- Modify: `D:/Mangoo/apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`
- Modify: `D:/Mangoo/apps/web/src/features/settings/interfaceStyle.ts`

- [ ] **Step 1: Inspect current material token usage**

Run:

```powershell
git status --short
rg -n -e "--glass-blur|--glass-elevated-blur|--glass-control-blur|--glass-pointer|--glass-edge|backdrop-filter|app-shell|#root|body|html" apps/web/src/styles.css apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx apps/web/src/features/settings/interfaceStyle.ts
```

Expected: current blur variables, pointer variables, edge variables, root selectors, and glass surface rules are visible for review.

- [ ] **Step 2: Ensure root selectors stay transparent**

In `D:/Mangoo/apps/web/src/styles.css`, ensure `html`, `body`, `#root`, and `.app-shell` do not set a non-transparent full-window background, backdrop filter, or large opaque pseudo-element. The intended root contract is:

```css
html,
body,
#root {
  width: 100%;
  height: 100%;
  margin: 0;
  background: transparent;
}

.app-shell {
  background: transparent;
}
```

Expected: any contrast layers are scoped to actual panels, titlebars, overlays, or content wells, not the renderer root.

- [ ] **Step 3: Map interface sliders to major surface blur**

In `D:/Mangoo/apps/web/src/features/settings/interfaceStyle.ts`, confirm `glassBlur` maps to these variables or equivalent existing variables used by major surfaces:

```ts
"--glass-blur"
"--glass-elevated-blur"
"--glass-control-blur"
```

Expected: changing the blur slider updates variables consumed by titlebar regions, side panels, inspector, and popovers.

- [ ] **Step 4: Apply real backdrop blur to glass material selectors**

In `D:/Mangoo/apps/web/src/styles.css`, ensure major glass surfaces use actual backdrop material:

```css
.liquid-glass-surface,
.shell-titlebar,
.library-sidebar,
.inspector-panel,
.task-overlay,
.recent-ops-popover,
.filter-menu-popover,
.sort-menu-popover {
  backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturation, 1.25));
  -webkit-backdrop-filter: blur(var(--glass-blur)) saturate(var(--glass-saturation, 1.25));
}
```

Use the repo's actual selectors if names differ, but preserve the requirement that titlebar, sidebar, inspector, and floating popovers visibly blur.

- [ ] **Step 5: Unify pointer and edge highlight variables**

In `D:/Mangoo/apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx` and `D:/Mangoo/apps/web/src/styles.css`, ensure every glass surface receives shared pointer variables such as:

```ts
"--glass-pointer-x"
"--glass-pointer-y"
"--glass-pointer-opacity"
```

and shared edge brightness variables such as:

```css
--glass-pointer-glow-brightness: 1;
--glass-edge-highlight-brightness: 5;
```

Expected: pointer/edge highlighting is not implemented by separate one-off gradients for only selected components.

- [ ] **Step 6: Task review gate**

Run:

```powershell
npm run check:ui-design
npm run check:web
```

Expected: checks either pass or fail only on later-task contracts not yet implemented. Review the diff and confirm no full rectangular root backing plate was added.

## Task 2: Fuse Titlebar And Content Material Regions

**Files:**
- Modify: `D:/Mangoo/apps/web/src/styles.css`
- Modify: `D:/Mangoo/apps/web/src/app-shell/AppShell.tsx`
- Modify: `D:/Mangoo/apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`

- [ ] **Step 1: Inspect shell grid and surface boundaries**

Run:

```powershell
git status --short
rg -n "shell-titlebar|shell-titlebar-center|titlebarLeft|titlebarRight|library-sidebar|inspector-panel|grid-surface|workspace|app-shell|grid-template|border|box-shadow" apps/web/src/app-shell/AppShell.tsx apps/web/src/styles.css apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx
```

Expected: all selectors and component props controlling titlebar, sidebar, center workspace content, and inspector boundaries are visible.

- [ ] **Step 2: Add or use internal edge suppression**

In `D:/Mangoo/apps/web/src/styles.css`, add explicit internal edge rules for joined regions. Use the repo's current classes, including `.shell-titlebar-center` and `.grid-surface` or the actual center workspace content selector, but the intended result is:

```css
.shell-titlebar-left {
  border-bottom-color: transparent;
  border-bottom-left-radius: 0;
  border-bottom-right-radius: 0;
}

.library-sidebar {
  border-top-color: transparent;
  border-top-left-radius: 0;
  border-top-right-radius: 0;
}

.shell-titlebar-right {
  border-bottom-color: transparent;
  border-bottom-left-radius: 0;
  border-bottom-right-radius: 0;
}

.inspector-panel {
  border-top-color: transparent;
  border-top-left-radius: 0;
  border-top-right-radius: 0;
}

.shell-titlebar-center {
  border-bottom-color: transparent;
}

.grid-surface,
.workspace-content {
  border-top-color: transparent;
}
```

Expected: only the internal joining edges are suppressed; the outer window/panel outline and necessary column separators remain visible. The center titlebar and center content area must not each draw their own border at the join.

- [ ] **Step 3: Align titlebar and panel material tokens**

In `D:/Mangoo/apps/web/src/styles.css`, ensure the fused pairs use the same material variables:

```css
.shell-titlebar-left,
.library-sidebar,
.shell-titlebar-center,
.grid-surface,
.shell-titlebar-right,
.inspector-panel {
  background: var(--glass-panel-background);
  box-shadow: var(--glass-panel-shadow);
}
```

Use existing variable names if the repo already has them, and do not implement this by adding a global root/app-shell backing plate. Expected: the titlebar and adjoining content no longer look like separate sheets.

- [ ] **Step 4: Check layout adjacency**

Run:

```powershell
rg -n "grid-area|titlebar-left|titlebar-center|titlebar-right|shell-titlebar-center|grid-surface|sidebar|inspector|workspace" apps/web/src/styles.css apps/web/src/app-shell/AppShell.tsx
```

Expected: left titlebar bottom aligns directly with sidebar top, middle titlebar bottom aligns directly with center workspace content top, and right titlebar bottom aligns directly with inspector top. There is no spacer, gap, duplicate wrapper border, or margin between each pair.

- [ ] **Step 5: Task review gate**

Run:

```powershell
npm run check:ui-design
npm run check:web
```

Expected: no static failure for transparent root or fused titlebar/content selectors. Review computed CSS mentally from the diff before moving to Task 3.

## Task 3: Redesign Middle Titlebar Controls

**Files:**
- Modify: `D:/Mangoo/apps/web/src/app-shell/ShellTopBar.tsx`
- Modify: `D:/Mangoo/apps/web/src/styles.css`

- [ ] **Step 1: Inspect middle titlebar controls**

Run:

```powershell
git status --short
rg -n "LibraryTitlebarToolbar|PreviewTitlebarToolbar|shell-titlebar-center|search|aria-label|title=|span|button|LiquidGlassButton" apps/web/src/app-shell/ShellTopBar.tsx apps/web/src/styles.css
```

Expected: every visible middle titlebar control and its styling is visible.

- [ ] **Step 2: Remove visible labels from middle titlebar buttons**

In `D:/Mangoo/apps/web/src/app-shell/ShellTopBar.tsx`, update middle titlebar tool buttons so each non-search button renders an icon only:

```tsx
<button
  aria-label="Refresh library"
  className="titlebar-tool-button no-drag"
  title="Refresh library"
  type="button"
>
  <RefreshCw size={16} aria-hidden="true" />
</button>
```

Expected: no visible `<span>` label remains inside middle titlebar tool buttons. Labels remain in `aria-label` and `title`.

- [ ] **Step 3: Make middle titlebar buttons unframed**

In `D:/Mangoo/apps/web/src/styles.css`, style middle titlebar icon buttons as transparent controls:

```css
.shell-titlebar-center .titlebar-tool-button {
  display: inline-grid;
  width: 32px;
  height: 32px;
  place-items: center;
  border: 0;
  border-radius: var(--radius-control);
  background: transparent;
  box-shadow: none;
  color: var(--text-soft);
}

.shell-titlebar-center .titlebar-tool-button:hover,
.shell-titlebar-center .titlebar-tool-button:focus-visible {
  background: color-mix(in srgb, var(--glass-control-tint) 16%, transparent);
  outline: none;
}
```

Use existing variable names if needed. Expected: persistent border and persistent button fill are gone.

- [ ] **Step 4: Keep search as the only visible glass input and right-align it**

In `D:/Mangoo/apps/web/src/styles.css`, align the search group to the right side of the middle titlebar:

```css
.shell-titlebar-center {
  display: flex;
  align-items: center;
  gap: 8px;
}

.shell-titlebar-center .search-bar,
.shell-titlebar-center .titlebar-search {
  margin-left: auto;
  max-width: min(360px, 42vw);
}
```

Expected: search remains visibly glass-framed and appears right-aligned relative to other middle titlebar tools.

- [ ] **Step 5: Center opened image title/name**

In `D:/Mangoo/apps/web/src/app-shell/ShellTopBar.tsx` and `D:/Mangoo/apps/web/src/styles.css`, ensure preview-open title text is centered in the middle titlebar:

```tsx
<div className="titlebar-preview-title" title={selectedMediaName}>
  {selectedMediaName}
</div>
```

```css
.titlebar-preview-title {
  position: absolute;
  left: 50%;
  max-width: min(420px, 42vw);
  transform: translateX(-50%);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: center;
  pointer-events: none;
}
```

Expected: the opened image name is centered without blocking buttons or search.

- [ ] **Step 6: Task review gate**

Run:

```powershell
npm run check:ui-design
npm run check:web
```

Expected: no TypeScript errors, no static failures for visible middle titlebar labels, and no visible middle titlebar button frames in local review.

## Task 4: Unify Floating Popover Close Behavior

**Files:**
- Modify: `D:/Mangoo/apps/web/src/app-shell/ShellOverlayHost.tsx`
- Modify: `D:/Mangoo/apps/web/src/app-shell/ShellTopBar.tsx`
- Modify: `D:/Mangoo/apps/web/src/features/library/FilterMenu.tsx`
- Modify: `D:/Mangoo/apps/web/src/features/library/SortMenu.tsx`
- Modify: `D:/Mangoo/apps/web/src/features/tasks/TaskOverlay.tsx`
- Modify: `D:/Mangoo/apps/web/src/styles.css`

- [ ] **Step 1: Inspect current popover state and close handlers**

Run:

```powershell
git status --short
rg -n "recentOpsOpen|taskPaletteOpen|filter|sort|Escape|keydown|pointerdown|mousedown|outside|popover|TaskOverlay|Recent" apps/web/src/app-shell/ShellOverlayHost.tsx apps/web/src/app-shell/ShellTopBar.tsx apps/web/src/features/library/FilterMenu.tsx apps/web/src/features/library/SortMenu.tsx apps/web/src/features/tasks/TaskOverlay.tsx apps/web/src/styles.css
```

Expected: existing compact popup state and handlers are identified before changing ownership.

- [ ] **Step 2: Use one active compact popover state**

Where shell-level overlay state is owned, represent compact popovers with a single state:

```ts
type CompactPopover = "tasks" | "recent" | "filter" | "sort" | null;
```

Expected: opening one compact popover closes the previous compact popover by replacing this state.

- [ ] **Step 3: Wire trigger toggles**

Update trigger handlers to follow this behavior:

```ts
function toggleCompactPopover(next: Exclude<CompactPopover, null>) {
  setActiveCompactPopover((current) => (current === next ? null : next));
}
```

Expected: Tasks, Recent, Filter, and Sort all toggle consistently. Opening center Task Center sets active compact popover to `null`.

- [ ] **Step 4: Add shared Escape and outside-click close**

In the owner component or overlay host, add document-level close behavior:

```ts
useEffect(() => {
  if (!activeCompactPopover) return;

  function onKeyDown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      setActiveCompactPopover(null);
    }
  }

  function onPointerDown(event: PointerEvent) {
    const target = event.target;
    if (!(target instanceof Node)) return;
    if (document.querySelector("[data-compact-popover-root]")?.contains(target)) return;
    if (document.querySelector("[data-compact-popover-trigger]")?.contains(target)) return;
    setActiveCompactPopover(null);
  }

  document.addEventListener("keydown", onKeyDown);
  document.addEventListener("pointerdown", onPointerDown, true);
  return () => {
    document.removeEventListener("keydown", onKeyDown);
    document.removeEventListener("pointerdown", onPointerDown, true);
  };
}, [activeCompactPopover]);
```

Adapt the selector strategy if multiple roots/triggers are needed. Expected: clicking any unrelated area closes the active compact popover.

- [ ] **Step 5: Match Filter and Sort material to Tasks/Recent**

In `D:/Mangoo/apps/web/src/features/library/FilterMenu.tsx`, `D:/Mangoo/apps/web/src/features/library/SortMenu.tsx`, and `D:/Mangoo/apps/web/src/styles.css`, make Filter and Sort content use the same floating material class family:

```tsx
<LiquidGlassSurface
  className="floating-popover filter-menu-popover"
  data-compact-popover-root
  tone="overlay"
>
```

Expected: Filter and Sort visually behave like Tasks and Recent floating popovers.

- [ ] **Step 6: Task review gate**

Run:

```powershell
npm run check:ui-design
npm run check:web
```

Expected: checks pass or only fail on later preview/visual harness tasks. Manually inspect the diff for one active compact popover state and no duplicate Escape handlers fighting each other.

## Task 5: Fix Preview Stage Sizing, Centering, And Center Preview Framing

**Files:**
- Modify: `D:/Mangoo/apps/web/src/features/preview/PreviewPanel.tsx`
- Modify: `D:/Mangoo/apps/web/src/features/preview/CentralPreviewStage.tsx`
- Modify: `D:/Mangoo/apps/web/src/app-shell/ShellTopBar.tsx`
- Modify: `D:/Mangoo/apps/web/src/styles.css`

- [ ] **Step 1: Inspect preview stage selectors and rendering**

Run:

```powershell
git status --short
rg -n "preview-stage|preview-panel|central-preview|CentralPreviewStage|preview-image|object-fit|padding|border|titlebar-preview-title|selectedMedia" apps/web/src/features/preview/PreviewPanel.tsx apps/web/src/features/preview/CentralPreviewStage.tsx apps/web/src/app-shell/ShellTopBar.tsx apps/web/src/styles.css
```

Expected: right preview and center preview wrappers, image classes, title handoff, and frame-like styles are visible.

- [ ] **Step 2: Keep right preview stage fixed near 260px**

In `D:/Mangoo/apps/web/src/styles.css`, ensure the right preview stage has stable height and centering:

```css
.preview-stage {
  display: grid;
  width: 100%;
  height: 260px;
  max-height: 260px;
  place-items: center;
  overflow: hidden;
  border: 0;
  background: transparent;
  padding: 0;
}

.preview-panel .preview-image,
.preview-panel video,
.preview-panel img {
  width: auto;
  height: auto;
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
}
```

Expected: landscape, portrait, and square media are centered and contained inside the fixed stage.

- [ ] **Step 3: Remove center preview frame styles**

In `D:/Mangoo/apps/web/src/styles.css`, remove or override center preview frame styles:

```css
.central-preview,
.central-preview-stage,
.central-preview-transform {
  background: transparent;
}

.central-preview-stage {
  border: 0;
  border-radius: 0;
  box-shadow: none;
  outline: none;
  padding: 0;
  margin: 0;
}

.central-preview-stage .preview-placeholder.ready {
  background: transparent;
  border: 0;
  padding: 0;
  margin: 0;
  overflow: visible;
}
```

Expected: no visible stage frame, padding, border, or inset mat remains around the opened image.

- [ ] **Step 4: Confirm center preview fit mode does not crop**

In `D:/Mangoo/apps/web/src/features/preview/CentralPreviewStage.tsx`, ensure fit-long-edge or fit-to-stage math uses `Math.min(stageWidth / naturalWidth, stageHeight / naturalHeight)` for contained fit behavior:

```ts
const fitScale = Math.min(stageWidth / naturalWidth, stageHeight / naturalHeight);
```

Expected: fit mode does not crop the image.

- [ ] **Step 5: Confirm preview title is centered**

In `D:/Mangoo/apps/web/src/app-shell/ShellTopBar.tsx`, verify the preview-open state renders the opened media title/name into `.titlebar-preview-title` from Task 3.

Expected: the title appears centered in the titlebar while preview is open.

- [ ] **Step 6: Task review gate**

Run:

```powershell
npm run check:ui-design
npm run check:web
```

Expected: no static failure for right preview fixed height, media centering, central preview frame removal, or centered preview title.

## Task 6: Update Static UI Validator

**Files:**
- Modify: `D:/Mangoo/tools/checks/validate-ui-design.mjs`

- [ ] **Step 1: Inspect existing validator helpers and file reads**

Run:

```powershell
git status --short
Get-Content -Path tools/checks/validate-ui-design.mjs
```

Expected: current helper functions, file reads, and pass/fail message are visible.

- [ ] **Step 2: Add root transparency checks**

Add checks that fail if `html`, `body`, `#root`, or `.app-shell` contain non-transparent full-window backgrounds or `backdrop-filter`.

Expected failure message when violated:

```text
root/app-shell must remain transparent; no global gray backing plate
```

- [ ] **Step 3: Add fused material region checks**

Add checks requiring selectors or component evidence for:

```js
"shell-titlebar-left"
"library-sidebar"
"shell-titlebar-center"
"grid-surface"
"shell-titlebar-right"
"inspector-panel"
```

and internal-edge suppression evidence such as transparent joining borders or explicit join classes for left titlebar/sidebar, middle titlebar/center content, and right titlebar/inspector.

Expected failure message when violated:

```text
left, center, and right titlebar/content material regions must be visually fused
```

- [ ] **Step 4: Add middle titlebar control checks**

Add checks that inspect `D:/Mangoo/apps/web/src/app-shell/ShellTopBar.tsx` and `D:/Mangoo/apps/web/src/styles.css` for:

- `.titlebar-tool-button`
- `aria-label`
- `title=`
- no visible text label class inside middle titlebar buttons
- transparent button styles with `border: 0`, `background: transparent`, and `box-shadow: none`
- search alignment evidence with `margin-left: auto`

Expected failure message when violated:

```text
middle titlebar buttons must be icon-only, accessible, unframed, and search must be right-aligned
```

- [ ] **Step 5: Add popover behavior and material checks**

Add checks requiring:

```js
"type CompactPopover"
"activeCompactPopover"
"Escape"
"pointerdown"
"data-compact-popover-root"
"filter-menu-popover"
"sort-menu-popover"
"floating-popover"
```

Expected failure message when violated:

```text
Tasks/Recent/Filter/Sort compact popovers must share floating material and close behavior
```

- [ ] **Step 6: Add preview checks**

Add checks requiring:

```js
"height: 260px"
"place-items: center"
"object-fit: contain"
"central-preview-stage"
"padding: 0"
"border: 0"
"titlebar-preview-title"
```

Expected failure message when violated:

```text
right preview must be fixed and centered; center preview must be unframed with centered title
```

- [ ] **Step 7: Task review gate**

Run:

```powershell
npm run check:ui-design
```

Expected: validator exits `0` only when Tasks 1-5 are implemented. If it fails, every failure points to a concrete contract in the spec.

## Task 7: Update Visual Harness And Screenshot Assertions

**Files:**
- Modify: `D:/Mangoo/.tmp/visual-check/desktop-ui-regression.mjs`

- [ ] **Step 1: Check whether the visual harness exists**

Run:

```powershell
if (Test-Path .tmp\visual-check\desktop-ui-regression.mjs) { Write-Output "visual harness present" } else { Write-Output "visual harness missing; skipped" }
```

Expected: if missing, skip the remaining steps in Task 7 and document the skip in the final implementation report. If present, continue.

- [ ] **Step 2: Inspect current visual harness**

Run:

```powershell
Get-Content -Path .tmp/visual-check/desktop-ui-regression.mjs
```

Expected: current screenshot names, console warning checks, layout evidence, and preview assertions are visible.

- [ ] **Step 3: Update layout evidence**

Add evidence for:

```js
const titlebarLeft = box(".shell-titlebar-left");
const titlebarCenter = box(".shell-titlebar-center");
const titlebarRight = box(".shell-titlebar-right");
const sidebar = box(".library-sidebar");
const centerWorkspace = box(".grid-surface");
const inspector = box(".inspector-panel");
```

Expected visual assertions:

- `titlebarLeft.bottom` is within 1px of `sidebar.top`.
- `titlebarCenter.bottom` is within 1px of `centerWorkspace.top`.
- `titlebarRight.bottom` is within 1px of `inspector.top`.
- `titlebarLeft` and `sidebar` have no visible internal gap.
- `titlebarCenter` and center workspace content have no visible internal seam, gap, or double border.
- `titlebarRight` and `inspector` have no visible internal gap.
- `html`, `body`, `#root`, and `.app-shell` computed backgrounds remain transparent.

- [ ] **Step 4: Update material and blur checks**

Add screenshot and computed-style checks for changing the blur slider:

```js
"ui-settings-interface-style.png"
```

Expected visual assertions:

- Settings Interface style section exists.
- Blur slider changes `--glass-blur`.
- Titlebar, sidebar, inspector, and popover computed `backdrop-filter` values change with the blur variable.
- No global root backing appears after slider changes.

- [ ] **Step 5: Update middle titlebar screenshot assertions**

Use or update:

```js
"ui-integrated-titlebar-main.png"
```

Expected visual assertions:

- Middle titlebar buttons contain icons only.
- No visible text labels appear inside middle titlebar tool buttons.
- Middle titlebar buttons have no persistent border or background.
- Search input is visible and right-aligned in the middle titlebar.
- Center titlebar and center content area do not each draw a border at their join.

- [ ] **Step 6: Add popover dismissal checks**

Add interactions for Tasks, Recent, Filter, and Sort:

- Open Tasks, then open Recent; Tasks closes.
- Open Recent, then open Filter; Recent closes.
- Open Filter, then open Sort; Filter closes.
- Press Escape; Sort closes.
- Open Tasks, click workspace; Tasks closes.

Expected visual assertions:

- At most one compact popover root exists at a time.
- Each compact popover root uses floating glass material.
- Center Task Center may remain modal and is excluded from the compact-popover count.

- [ ] **Step 7: Update preview screenshots and assertions**

Use or update:

```js
"ui-selected-portrait-right-preview.png"
"ui-central-landscape-fit-long-edge.png"
"ui-central-landscape-actual-100.png"
"ui-central-portrait-fit-long-edge.png"
```

Expected visual assertions:

- Right `.preview-stage` height is within 250px to 270px.
- Right preview media is centered horizontally and vertically.
- Right preview successful media uses `object-fit: contain`.
- Center preview stage has `border-width: 0`, `padding: 0`, transparent background, and no frame-like box shadow.
- Center titlebar and center content have no internal seam, gap, or double border before and after opening the preview.
- Center preview fit mode does not crop the image.
- Opened image title/name is centered in the titlebar.

- [ ] **Step 8: Add startup warning/error checks**

Ensure the summary JSON records:

```js
consoleWarnings
consoleErrors
networkProblems
fatalError
hardFailures
```

Expected: implementation passes only when no new startup warning/error, failed app asset, fatal error, or hard failure is present.

- [ ] **Step 9: Task review gate**

Run:

```powershell
if (Test-Path .tmp\visual-check\desktop-ui-regression.mjs) { node .tmp\visual-check\desktop-ui-regression.mjs } else { Write-Output "visual harness missing; skipped" }
```

Expected: when present, the command exits `0`, updates `.tmp\visual-check\logs\desktop-ui-regression-summary.json`, and writes the listed screenshots.

## Task 8: Full Verification

**Files:**
- Review only: all files changed by Tasks 1-7

- [ ] **Step 1: Static UI design check**

Run:

```powershell
npm run check:ui-design
```

Expected: exit code `0`; no failures for root backing, titlebar/panel fusion, middle titlebar labels, popover behavior, blur material, right preview sizing, or center preview framing.

- [ ] **Step 2: Web check**

Run:

```powershell
npm run check:web
```

Expected: exit code `0`; Web boundary checks and TypeScript checks pass.

- [ ] **Step 3: Desktop check**

Run:

```powershell
npm run check:desktop
```

Expected: exit code `0`; Electron desktop checks and TypeScript checks pass.

- [ ] **Step 4: Web build**

Run:

```powershell
npm --workspace @mangoo/web run build
```

Expected: exit code `0`; Vite build completes without TypeScript or bundling errors.

- [ ] **Step 5: Desktop build**

Run:

```powershell
npm --workspace @mangoo/desktop run build
```

Expected: exit code `0`; desktop main and preload build completes.

- [ ] **Step 6: Conditional visual harness**

Run:

```powershell
if (Test-Path .tmp\visual-check\desktop-ui-regression.mjs) { node .tmp\visual-check\desktop-ui-regression.mjs } else { Write-Output "visual harness missing; skipped" }
```

Expected when present: exit code `0`, screenshots are updated, and `.tmp\visual-check\logs\desktop-ui-regression-summary.json` has no `fatalError`, no `consoleErrors`, no startup warnings introduced by this phase, no `networkProblems`, and no `hardFailures`.

- [ ] **Step 7: Manual visual assertion checklist**

Review the latest screenshots and confirm:

- Left titlebar and left sidebar read as one continuous material region.
- Middle titlebar and center content column read as one continuous visual surface, with no internal seam, gap, or double border at their join.
- Right titlebar and right inspector read as one continuous material region.
- Only the outer outline and necessary column separators remain visible around joined titlebar/content regions.
- No full rectangular gray root or app-shell backing plate is visible.
- Blur slider visibly changes titlebar, sidebar, inspector, and popover blur.
- Local edge highlight appears on all visible tested glass borders.
- Middle titlebar buttons are icon-only, unframed, and transparent.
- Middle titlebar search input is right-aligned and remains visibly glass.
- Filter and Sort popovers match Tasks and Recent floating material.
- Tasks, Recent, Filter, and Sort close on outside click, Escape, and another popover toggle.
- Right preview stage remains around `260px`; media is centered both ways.
- Center preview image has no border, padding, margin frame, or crop in fit mode.
- Opened image title/name is centered in the titlebar.

- [ ] **Step 8: Final implementation report**

Report:

- Files changed.
- Verification commands and exit statuses.
- Visual screenshot paths.
- Startup warning/error summary.
- Any remaining concerns.

Do not stage or commit.

## Self-Review Checklist For Implementer

- Requirement coverage:
  - Unified Workbench Material Layer: Tasks 1 and 2.
  - Left/right titlebar and panel fusion without internal separation: Task 2 and Task 7.
  - Middle titlebar and center content fusion without internal seam, gap, or double border: Task 2, Task 7, and Task 8.
  - Effective Liquid Glass blur and blur slider behavior: Task 1 and Task 7.
  - Consistent pointer/edge highlight: Task 1 and Task 7.
  - Middle titlebar icon-only unframed buttons with accessible labels: Task 3 and Task 6.
  - Search remains glass and right-aligned: Task 3 and Task 7.
  - Filter/Sort popovers match Tasks/Recent: Task 4 and Task 7.
  - Compact popover auto-dismiss and Escape behavior: Task 4 and Task 7.
  - Right preview fixed height and media centering: Task 5 and Task 7.
  - Center preview unframed and title centered: Task 5 and Task 7.
  - Transparent root/app-shell with no global backing plate: Task 1, Task 6, and Task 7.
  - Screenshot visual checks and startup warning/error checks: Task 7 and Task 8.
- Placeholder scan:
  - The plan contains no implementation placeholders, no deferred behavior, and no stage/commit steps.
- Execution model:
  - Tasks are sequential and suitable for one subagent at a time, with review gates before continuing.
