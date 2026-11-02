# Mangoo Desktop Workbench Shell Rearchitecture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the desktop shell around a transparent native host, renderer-owned continuous glass workbench surfaces, a unified titlebar interaction plane, and a real dialog/backdrop material stack without falling back to patch-level CSS fixes.

**Architecture:** The Electron host stays transparent and hidden until the renderer shell explicitly reports readiness. The renderer owns one clipped shell root, three continuous structural surfaces, one shared titlebar pointer plane, and one overlay material system. Settings exposes shell parameters by approved material families rather than by legacy ad hoc tokens.

**Tech Stack:** Electron frameless desktop window, React + TypeScript, Vite, CSS custom properties, existing Liquid Glass primitives, static design checks in `tools/checks`, desktop visual harness in `.tmp/visual-check`.

---

## File Structure / 代码边界

- Create: `apps/web/src/app-shell/useTitlebarPointerPlane.ts`
  - Centralize titlebar-wide pointer state, halo-zone routing, drag start/stop, and double-click maximize/restore behavior.
- Create: `apps/web/src/features/library/LibraryCenterPane.tsx`
  - Own center-column library grid / full-image preview content without the right inspector.
- Create: `apps/web/src/features/library/LibraryInspectorPane.tsx`
  - Own right-column preview/metadata content so the right structural shell can be composed outside `LibraryView`.
- Modify: `apps/desktop/src/main.ts`
  - Keep the native window transparent/hidden until renderer shell readiness and remove direct show timing from the old startup path.
- Modify: `apps/desktop/src/preload.cjs`
  - Expose renderer shell readiness bridge.
- Modify: `apps/web/src/core/desktop.ts`
  - Export shell readiness bridge API alongside existing window/shell helpers.
- Modify: `apps/web/src/app/App.tsx`
  - Compose shell slots explicitly, split library center/right content, mount shell-ready effect, and keep overlay state ownership.
- Modify: `apps/web/src/app-shell/AppShell.tsx`
  - Replace independent titlebar surfaces plus negative-margin joins with three continuous structural columns under one shell root.
- Modify: `apps/web/src/app-shell/ShellTopBar.tsx`
  - Annotate titlebar controls for the unified pointer plane and keep search-field special handling.
- Modify: `apps/web/src/app-shell/ShellOverlayHost.tsx`
  - Keep overlay ownership centralized and ensure dialogs use the shared backdrop/surface stack.
- Modify: `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`
  - Add explicit structural-vs-interactive material behavior, outline-only control mode, and tighter pointer-target gating.
- Modify: `apps/web/src/design/liquid-glass/index.ts`
  - Re-export the refactored interface-style controller and any new liquid-glass helpers.
- Modify: `apps/web/src/features/library/SearchBar.tsx`
  - Stop using a full dual-layer control material for the input; keep one outer outline only.
- Modify: `apps/web/src/features/window-chrome/WindowChrome.tsx`
  - Join window buttons to the titlebar plane annotations.
- Modify: `apps/web/src/features/library/LibraryView.tsx`
  - Reduce or delegate the mixed center/right responsibility to the new pane split.
- Modify: `apps/web/src/features/preview/CentralPreviewStage.tsx`
  - Keep the center preview unframed while preserving pan/zoom behavior.
- Modify: `apps/web/src/features/preview/PreviewPanel.tsx`
  - Keep preview content on the right structural shell without creating a second framed card.
- Modify: `apps/web/src/features/settings/interfaceStyle.ts`
  - Replace the legacy global token model with the approved parameter families.
- Modify: `apps/web/src/features/settings/SettingsView.tsx`
  - Expose grouped controls for shared shape, side shell material, center workbench material, shared liquid-glass interaction, and dialog material.
- Modify: `apps/web/src/features/file-ops/RenameDialog.tsx`
- Modify: `apps/web/src/features/file-ops/MoveDialog.tsx`
- Modify: `apps/web/src/features/file-ops/DeleteConfirm.tsx`
  - Consume the shared dialog/backdrop material rules rather than free-floating transparency.
- Modify: `apps/web/src/styles.css`
  - Own shell-root clipping, continuous-column composition, titlebar-plane selectors, material variable consumption, overlay stack, and no-diagonal-gradient rule.
- Modify: `tools/checks/validate-ui-design.mjs`
  - Express the new static contract for shell-ready startup, grouped interface-style settings, titlebar-plane annotations, search-field outline-only behavior, structural surface joins, and dialog blur.
- Modify: `tools/checks/validate-desktop-core.mjs`
  - Keep the preload bridge contract in sync when shell-ready IPC is added.
- Modify: `tools/checks/validate-web-client.mjs`
  - Keep the `window.mangooDesktop` access boundary explicit if the renderer startup bridge changes.
- Modify: `tools/checks/native-browser-window-options.test.mjs`
  - Keep the native host contract parser covered while the desktop startup contract is tightened.
- Modify: `.tmp/visual-check/desktop-ui-regression.mjs`
  - Capture the new desktop expectations: no host rectangle, joined left/right titlebars, titlebar halo refresh, dialog blur, and no center preview frame.

Execution discipline:

- The current worktree is already dirty. Do not revert unrelated changes.
- Treat `validate-ui-design.mjs` and the visual harness as the failing-test surface for UI contract work.
- Commit after each task; do not batch multiple tasks into one commit.

## Task 1: Native Transparent Host And Shell-Ready Handshake

**Files:**
- Modify: `apps/desktop/src/main.ts`
- Modify: `apps/desktop/src/preload.cjs`
- Modify: `apps/web/src/core/desktop.ts`
- Modify: `apps/web/src/app/App.tsx`
- Modify: `tools/checks/validate-ui-design.mjs`
- Modify: `tools/checks/validate-desktop-core.mjs`
- Modify: `tools/checks/validate-web-client.mjs`

- [ ] **Step 1: Write the failing startup contract check**

Add these checks to `tools/checks/validate-ui-design.mjs` so the current code fails until the renderer explicitly signals shell readiness:

```js
const desktopPreload = read("apps/desktop/src/preload.cjs");
const desktopBridge = read("apps/web/src/core/desktop.ts");

for (const value of ["mangoo:shell-ready", "notifyShellReady", "notifyDesktopShellReady"]) {
  if (!desktopMain.includes(value) && !desktopPreload.includes(value) && !desktopBridge.includes(value)) {
    fail(`desktop shell-ready startup contract missing ${value}`);
  }
}

if (!/show:\s*false\b/.test(desktopMain)) {
  fail("desktop window must stay hidden until renderer shell readiness is confirmed");
}

if (/await\s+readyToShow[\s\S]*mainWindow\.show\(\)/.test(desktopMain) && !desktopMain.includes("mangoo:shell-ready")) {
  fail("desktop window show timing must be driven by the renderer shell-ready handshake, not directly after ready-to-show");
}
```

Also extend the secondary contract checks:

```js
// tools/checks/validate-desktop-core.mjs
if (!preloadSource.includes("notifyShellReady")) {
  fail("preload bridge must expose notifyShellReady for renderer shell readiness");
}

// tools/checks/validate-web-client.mjs
if (!desktopSource.includes("notifyDesktopShellReady")) {
  fail("renderer desktop bridge helper must expose notifyDesktopShellReady");
}
```

- [ ] **Step 2: Run the failing contract**

Run:

```bash
npm run check:ui-design
```

Expected: FAIL with a missing shell-ready contract message because the current startup path still shows the window directly after `ready-to-show`.

- [ ] **Step 3: Implement the handshake**

In `apps/desktop/src/main.ts`, move the visible show path behind a new IPC handler:

```ts
ipcMain.handle("mangoo:shell-ready", async () => {
  if (!mainWindow || mainWindow.isDestroyed()) return false;
  if (!mainWindow.isVisible()) {
    mainWindow.show();
    await waitForRendererFrame(mainWindow);
    await new Promise((resolve) => setTimeout(resolve, 80));
    mainWindow.setOpacity(1);
    mainWindow.focus();
  }
  return true;
});

await mainWindow.loadURL(devServer);
await readyToShow;
// do not call show() here anymore
```

In `apps/desktop/src/preload.cjs`, expose the new bridge:

```js
contextBridge.exposeInMainWorld("mangooDesktop", {
  // ...
  notifyShellReady: () => ipcRenderer.invoke("mangoo:shell-ready"),
  // ...
});
```

In `apps/web/src/core/desktop.ts`, expose a renderer helper:

```ts
export interface MangooDesktopBridge {
  notifyShellReady?: () => Promise<boolean>;
  // existing fields...
}

export async function notifyDesktopShellReady(): Promise<boolean> {
  return (await getDesktopBridge()?.notifyShellReady?.()) ?? false;
}
```

In `apps/web/src/app/App.tsx`, signal readiness once the shell mounts:

```tsx
useEffect(() => {
  void notifyDesktopShellReady();
}, []);
```

- [ ] **Step 4: Verify the startup contract passes**

Run:

```bash
npm run check:ui-design
npm run check:desktop
npm run check:web
```

Expected: PASS. `check:ui-design` no longer reports a missing shell-ready handshake; desktop and web typechecks still succeed.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/main.ts apps/desktop/src/preload.cjs apps/web/src/core/desktop.ts apps/web/src/app/App.tsx tools/checks/validate-ui-design.mjs tools/checks/validate-desktop-core.mjs tools/checks/validate-web-client.mjs
git commit -m "feat: gate desktop window show on renderer shell readiness"
```

## Task 2: Refactor Interface Style Into Approved Material Families

**Files:**
- Modify: `apps/web/src/features/settings/interfaceStyle.ts`
- Modify: `apps/web/src/design/liquid-glass/index.ts`
- Modify: `apps/web/src/features/settings/SettingsView.tsx`
- Modify: `apps/web/src/styles.css`
- Modify: `tools/checks/validate-ui-design.mjs`

- [ ] **Step 1: Write the failing interface-style contract**

Extend `tools/checks/validate-ui-design.mjs` to require the approved settings families and key fields:

```js
for (const value of [
  "windowCornerRadius",
  "surfaceCornerRadius",
  "controlCornerRadius",
  "contentCornerRadius",
  "sideBlur",
  "sideOverlayColor",
  "centerBlur",
  "centerOverlayColor",
  "haloBrightness",
  "pointerResponseRadius",
  "dialogBlur",
  "dialogBackdropDim"
]) {
  if (!interfaceStyle.includes(value)) {
    fail(`interface style model missing ${value}`);
  }
}

for (const heading of [
  "Shared shape",
  "Side shell material",
  "Center workbench material",
  "Shared liquid glass interaction",
  "Dialog material"
]) {
  if (!settingsView.includes(heading)) {
    fail(`settings view missing interface style group ${heading}`);
  }
}
```

- [ ] **Step 2: Run the failing contract**

Run:

```bash
npm run check:ui-design
```

Expected: FAIL because `interfaceStyle.ts` still uses the older one-bucket model.

- [ ] **Step 3: Replace the preference model and settings groups**

Refactor `apps/web/src/features/settings/interfaceStyle.ts` to the approved structure:

```ts
export interface InterfaceStylePreference {
  windowCornerRadius: number;
  surfaceCornerRadius: number;
  controlCornerRadius: number;
  contentCornerRadius: number;
  sideBlur: number;
  sideOpacity: number;
  sideOverlayStrength: number;
  sideOverlayColor: string;
  sideSaturation: number;
  sideStrokeOpacity: number;
  centerBlur: number;
  centerOpacity: number;
  centerOverlayStrength: number;
  centerOverlayColor: string;
  centerSaturation: number;
  centerStrokeOpacity: number;
  edgeHighlightBrightness: number;
  edgeHighlightSize: number;
  haloBrightness: number;
  haloFalloff: number;
  pointerResponseRadius: number;
  refractionStrength: number;
  dialogBlur: number;
  dialogOpacity: number;
  dialogOverlayStrength: number;
  dialogBackdropDim: number;
}
```

Map those fields to CSS variables instead of the older `windowRadius/panelRadius/...` bucket:

```ts
return {
  "--radius-window": `${value.windowCornerRadius}px`,
  "--radius-panel": `${value.surfaceCornerRadius}px`,
  "--radius-control": `${value.controlCornerRadius}px`,
  "--radius-content": `${value.contentCornerRadius}px`,
  "--shell-side-blur": `${roundCssNumber(26 * value.sideBlur)}px`,
  "--shell-side-overlay-color": value.sideOverlayColor,
  "--shell-center-blur": `${roundCssNumber(26 * value.centerBlur)}px`,
  "--shell-center-overlay-color": value.centerOverlayColor,
  "--glass-halo-brightness": String(roundCssNumber(value.haloBrightness)),
  "--glass-pointer-response-radius": `${roundCssNumber(112 * value.pointerResponseRadius)}px`,
  "--dialog-blur": `${roundCssNumber(34 * value.dialogBlur)}px`,
  "--dialog-backdrop-dim": String(roundCssNumber(value.dialogBackdropDim))
};
```

In `apps/web/src/features/settings/SettingsView.tsx`, replace the current radius/glass/liquid-glass groups with the approved five groups. Keep sliders for numeric values and use native color inputs for overlay colors:

```tsx
<div className="settings-style-group">
  <h3 className="settings-style-group-title">Side shell material</h3>
  <StyleSlider id="side-blur" label="Blur" ... />
  <StyleSlider id="side-opacity" label="Opacity" ... />
  <StyleSlider id="side-overlay-strength" label="Overlay strength" ... />
  <ColorField id="side-overlay-color" label="Overlay color" ... />
  <StyleSlider id="side-saturation" label="Saturation" ... />
  <StyleSlider id="side-stroke-opacity" label="Stroke opacity" ... />
</div>
```

In `apps/web/src/design/liquid-glass/index.ts`, re-export the renamed controller fields and helper types.

- [ ] **Step 4: Verify settings and type contracts**

Run:

```bash
npm run check:web
npm run check:ui-design
```

Expected: PASS. The settings page and interface-style model now expose the approved families, and TypeScript still passes.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/features/settings/interfaceStyle.ts apps/web/src/design/liquid-glass/index.ts apps/web/src/features/settings/SettingsView.tsx apps/web/src/styles.css tools/checks/validate-ui-design.mjs
git commit -m "feat: group interface style controls by desktop shell material family"
```

## Task 3: Build The Unified Titlebar Pointer Plane And Search-Field Special Rule

**Files:**
- Create: `apps/web/src/app-shell/useTitlebarPointerPlane.ts`
- Modify: `apps/web/src/app-shell/AppShell.tsx`
- Modify: `apps/web/src/app-shell/ShellTopBar.tsx`
- Modify: `apps/web/src/features/library/SearchBar.tsx`
- Modify: `apps/web/src/features/window-chrome/WindowChrome.tsx`
- Modify: `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`
- Modify: `apps/web/src/design/liquid-glass/index.ts`
- Modify: `apps/web/src/styles.css`
- Modify: `tools/checks/validate-ui-design.mjs`

- [ ] **Step 1: Write the failing titlebar-plane contract**

Add checks that require a shared titlebar-plane helper and search-field outline-only handling:

```js
for (const value of ["useTitlebarPointerPlane", "data-titlebar-control", "data-titlebar-search"]) {
  if (!appShell.includes(value) && !shellTitlebar.includes(value) && !searchBar.includes(value)) {
    fail(`titlebar interaction plane contract missing ${value}`);
  }
}

if (!searchBar.includes("outlineOnly")) {
  fail("search bar must opt into the outline-only liquid-glass mode");
}

if (!windowChrome.includes("data-titlebar-control")) {
  fail("window chrome buttons must participate in the shared titlebar pointer plane");
}
```

- [ ] **Step 2: Run the failing contract**

Run:

```bash
npm run check:ui-design
```

Expected: FAIL because the titlebar still uses repeated per-surface pointer handlers and the search field still renders as a regular full-material control.

- [ ] **Step 3: Implement the shared titlebar plane**

Create `apps/web/src/app-shell/useTitlebarPointerPlane.ts`:

```ts
import { useCallback, useMemo, useRef } from "react";
import { getWindowControls } from "../core/desktop";

export function useTitlebarPointerPlane() {
  const dragRef = useRef<{
    pointerId: number;
    startClientX: number;
    startClientY: number;
    startWindowX: number;
    startWindowY: number;
  } | null>(null);

  const updateControls = useCallback((root: HTMLElement, clientX: number, clientY: number) => {
    root.style.setProperty("--titlebar-pointer-x", `${clientX}px`);
    root.style.setProperty("--titlebar-pointer-y", `${clientY}px`);
    root.querySelectorAll<HTMLElement>("[data-titlebar-control]").forEach((node) => {
      node.dataset.glassPointer = "active";
    });
  }, []);

  return useMemo(() => ({ dragRef, updateControls }), [updateControls]);
}
```

Then move the current repeated left/center/right pointer logic out of `apps/web/src/app-shell/AppShell.tsx` and bind all three titlebar regions to one plane instance. Keep drag start only on blank/halo/drag zones instead of on arbitrary controls.

In `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`, add an explicit outline-only mode:

```ts
interface LiquidGlassSurfaceProps extends HTMLAttributes<HTMLElement> {
  outlineOnly?: boolean;
  interactiveBackground?: boolean;
}
```

Use it to suppress backdrop/lens/dim layers for the search field while preserving the outer edge:

```tsx
{outlineOnly ? null : <span aria-hidden="true" className="liquid-glass-backdrop" />}
{outlineOnly ? null : <span aria-hidden="true" className="liquid-glass-lens" />}
<span aria-hidden="true" className="liquid-glass-edge" />
```

Update `apps/web/src/features/library/SearchBar.tsx`:

```tsx
<LiquidGlassSurface
  as="div"
  className="search-bar"
  data-titlebar-control="search"
  data-titlebar-search="true"
  interactive
  interactiveBackground={false}
  outlineOnly
  tone="control"
>
```

Update `apps/web/src/features/window-chrome/WindowChrome.tsx` and titlebar buttons in `ShellTopBar.tsx` to add `data-titlebar-control="..."`.

- [ ] **Step 4: Verify titlebar contracts and typing**

Run:

```bash
npm run check:web
npm run check:ui-design
```

Expected: PASS. The search bar is now outline-only, and static checks can see the shared titlebar-plane annotations.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/app-shell/useTitlebarPointerPlane.ts apps/web/src/app-shell/AppShell.tsx apps/web/src/app-shell/ShellTopBar.tsx apps/web/src/features/library/SearchBar.tsx apps/web/src/features/window-chrome/WindowChrome.tsx apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx apps/web/src/design/liquid-glass/index.ts apps/web/src/styles.css tools/checks/validate-ui-design.mjs
git commit -m "feat: unify titlebar halo refresh and drag handling"
```

## Task 4: Recompose The Shell Into Three Continuous Structural Surfaces

**Files:**
- Create: `apps/web/src/features/library/LibraryCenterPane.tsx`
- Create: `apps/web/src/features/library/LibraryInspectorPane.tsx`
- Modify: `apps/web/src/app/App.tsx`
- Modify: `apps/web/src/app-shell/AppShell.tsx`
- Modify: `apps/web/src/features/library/LibraryView.tsx`
- Modify: `apps/web/src/features/preview/CentralPreviewStage.tsx`
- Modify: `apps/web/src/features/preview/PreviewPanel.tsx`
- Modify: `apps/web/src/styles.css`
- Modify: `tools/checks/validate-ui-design.mjs`
- Modify: `tools/checks/validate-web-client.mjs`
- Modify: `.tmp/visual-check/desktop-ui-regression.mjs`

- [ ] **Step 1: Write the failing shell-composition contract**

Update `tools/checks/validate-ui-design.mjs` to require three continuous columns and the removal of top/body seam hacks:

```js
for (const value of ["centerPane", "rightPane", "workbench-column-left", "workbench-column-center", "workbench-column-right"]) {
  if (!appShell.includes(value) && !app.includes(value)) {
    fail(`continuous shell composition missing ${value}`);
  }
}

if (/must not be wrapped in a structural LiquidGlass surface/.test(validateUiDesignSource)) {
  fail("the old center-column non-structural-glass prohibition must be removed for the new shell architecture");
}

for (const forbidden of ["margin-top: calc(var(--shell-titlebar-height) * -1)", "padding-top: calc(var(--shell-titlebar-height) + 16px)"]) {
  if (stylesForChecks.includes(forbidden)) {
    fail(`continuous shell composition must remove seam-hiding layout hack ${forbidden}`);
  }
}

if (/linear-gradient\(\s*145deg|linear-gradient\(\s*135deg/.test(stylesForChecks)) {
  fail("structural shell surfaces must not use a decorative diagonal gradient");
}
```

- [ ] **Step 2: Run the failing contract**

Run:

```bash
npm run check:ui-design
```

Expected: FAIL because the current shell still relies on separate titlebar surfaces plus negative-margin joins.

- [ ] **Step 3: Split center and right library ownership**

Create `apps/web/src/features/library/LibraryCenterPane.tsx` by moving the current center-stage portion from `LibraryView.tsx`:

```tsx
export function LibraryCenterPane(props: LibraryCenterPaneProps) {
  return (
    <section className="library-center-pane" aria-label="Media workspace">
      {props.previewMedia ? (
        <CentralPreviewStage ... />
      ) : (
        <MediaGrid ... />
      )}
    </section>
  );
}
```

Create `apps/web/src/features/library/LibraryInspectorPane.tsx`:

```tsx
export function LibraryInspectorPane({ selectedMedia, thumbnail, children }: LibraryInspectorPaneProps) {
  return (
    <PreviewPanel selectedMedia={selectedMedia} showPreviewImage={!previewOpen} thumbnail={thumbnail}>
      {children}
    </PreviewPanel>
  );
}
```

In `apps/web/src/app/App.tsx`, stop passing a monolithic `workspace` node into `AppShell`; instead pass explicit `centerPane` and `rightPane` props.

In `apps/web/src/app-shell/AppShell.tsx`, compose one continuous `LiquidGlassSurface` per column:

```tsx
<main className="app-shell" data-layout={layout}>
  <LiquidGlassSurface as="section" className="workbench-column workbench-column-left" tone="chrome">
    <header className="shell-titlebar shell-titlebar-left">{titlebarLeft}</header>
    <div className="workbench-column-body">{sidebar}</div>
  </LiquidGlassSurface>
  <LiquidGlassSurface as="section" className="workbench-column workbench-column-center" tone="chrome">
    <header className="shell-titlebar shell-titlebar-center">{titlebarCenter}</header>
    <div className="workbench-column-body">{centerPane}</div>
  </LiquidGlassSurface>
  <LiquidGlassSurface as="section" className="workbench-column workbench-column-right" tone="chrome">
    <header className="shell-titlebar shell-titlebar-right">{titlebarRight}</header>
    <div className="workbench-column-body">{rightPane}</div>
  </LiquidGlassSurface>
  {overlays}
</main>
```

Update `CentralPreviewStage.tsx` and `PreviewPanel.tsx` so the center image viewer remains unframed and the right preview is simply content on the right shell, not a second structural card.

- [ ] **Step 4: Verify structure, build, and visual contract**

Run:

```bash
npm run check:web
npm run check:ui-design
npm --workspace @mangoo/web run build
node .tmp/visual-check/desktop-ui-regression.mjs
```

Expected: PASS. Static checks stop seeing seam-hiding hacks; the visual harness updates can now assert joined left/right titlebars and no center preview frame.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/features/library/LibraryCenterPane.tsx apps/web/src/features/library/LibraryInspectorPane.tsx apps/web/src/app/App.tsx apps/web/src/app-shell/AppShell.tsx apps/web/src/features/library/LibraryView.tsx apps/web/src/features/preview/CentralPreviewStage.tsx apps/web/src/features/preview/PreviewPanel.tsx apps/web/src/styles.css tools/checks/validate-ui-design.mjs tools/checks/validate-web-client.mjs .tmp/visual-check/desktop-ui-regression.mjs
git commit -m "feat: compose the desktop shell as three continuous workbench surfaces"
```

## Task 5: Restrict Liquid Response To Interactive Backgrounds And Add Real Dialog Frosting

**Files:**
- Modify: `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`
- Modify: `apps/web/src/app-shell/ShellOverlayHost.tsx`
- Modify: `apps/web/src/features/tasks/TaskOverlay.tsx`
- Modify: `apps/web/src/features/library/FilterMenu.tsx`
- Modify: `apps/web/src/features/library/SortMenu.tsx`
- Modify: `apps/web/src/features/file-ops/ContextMenu.tsx`
- Modify: `apps/web/src/features/file-ops/RenameDialog.tsx`
- Modify: `apps/web/src/features/file-ops/MoveDialog.tsx`
- Modify: `apps/web/src/features/file-ops/DeleteConfirm.tsx`
- Modify: `apps/web/src/styles.css`
- Modify: `tools/checks/validate-ui-design.mjs`
- Modify: `.tmp/visual-check/desktop-ui-regression.mjs`

- [ ] **Step 1: Write the failing interaction/dialog contract**

Add checks for interactive-only background halo and dialog blur:

```js
for (const value of ["interactiveBackground", "dialog-backdrop", "--dialog-blur", "--dialog-backdrop-dim"]) {
  if (!liquidGlassSurface.includes(value) && !stylesForChecks.includes(value)) {
    fail(`interactive background or dialog material contract missing ${value}`);
  }
}

if (!/backdrop-filter:\s*blur\(var\(--dialog-blur\)\)/.test(stylesForChecks)) {
  fail("dialog backdrop must blur the underlying UI");
}

if (!/data-interactive-pointer-target/.test(liquidGlassSurface)) {
  fail("interactive pointer halo must be explicitly opt-in");
}
```

- [ ] **Step 2: Run the failing contract**

Run:

```bash
npm run check:ui-design
```

Expected: FAIL because the current global pointer updates still light too many surfaces and dialogs rely on transparency without a dedicated blurred backdrop contract.

- [ ] **Step 3: Implement interactive-only halo and dialog stack**

In `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`, stop sending local background halo to generic structural surfaces by default:

```ts
const INTERACTIVE_AFFORDANCE_SELECTOR = [
  "[data-interactive-pointer-target=\"true\"]",
  "button[data-liquid-glass]",
  "input[data-interactive-pointer-target=\"true\"]",
  ".tree-item[data-interactive-pointer-target=\"true\"]",
  ".tile-thumb[data-interactive-pointer-target=\"true\"]"
].join(",");
```

Use `interactiveBackground={false}` on structural shells and `data-interactive-pointer-target="true"` only on real interactive affordances.

In `apps/web/src/styles.css`, create a real backdrop/surface split:

```css
.dialog-backdrop {
  position: fixed;
  inset: 0;
  display: grid;
  place-items: center;
  background: rgb(3 6 10 / var(--dialog-backdrop-dim));
  -webkit-backdrop-filter: blur(var(--dialog-blur)) saturate(1.2);
  backdrop-filter: blur(var(--dialog-blur)) saturate(1.2);
}

.liquid-glass.dialog {
  --glass-fill: transparent;
  --glass-blur-current: var(--dialog-blur);
}
```

Keep `RenameDialog.tsx`, `MoveDialog.tsx`, and `DeleteConfirm.tsx` structurally the same, but ensure they rely on the shared `dialog-backdrop` and `LiquidGlassSurface` dialog class instead of ad hoc transparency.

Move compact overlay classes onto shared presets as well:

```tsx
// TaskOverlay.tsx / FilterMenu.tsx / SortMenu.tsx / ContextMenu.tsx
<LiquidGlassSurface
  as="div"
  className="floating-popover floating-popover-compact"
  interactive
  scrollable
  tone="elevated"
>
```

- [ ] **Step 4: Verify UI contract and desktop behavior**

Run:

```bash
npm run check:ui-design
npm run check:web
node .tmp/visual-check/desktop-ui-regression.mjs
```

Expected: PASS. Static checks see explicit interactive-pointer gating and dialog blur; desktop screenshots now show a frosted dialog instead of a pure transparent plate.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx apps/web/src/app-shell/ShellOverlayHost.tsx apps/web/src/features/tasks/TaskOverlay.tsx apps/web/src/features/library/FilterMenu.tsx apps/web/src/features/library/SortMenu.tsx apps/web/src/features/file-ops/ContextMenu.tsx apps/web/src/features/file-ops/RenameDialog.tsx apps/web/src/features/file-ops/MoveDialog.tsx apps/web/src/features/file-ops/DeleteConfirm.tsx apps/web/src/styles.css tools/checks/validate-ui-design.mjs .tmp/visual-check/desktop-ui-regression.mjs
git commit -m "feat: scope liquid halo to interactive backgrounds and frost dialogs"
```

## Task 6: Final Desktop Verification And Release Gate

**Files:**
- Modify if needed after verification: `tools/checks/validate-ui-design.mjs`
- Modify if needed after verification: `.tmp/visual-check/desktop-ui-regression.mjs`
- Modify if needed after verification: any file touched in Tasks 1-5

- [ ] **Step 1: Run the full verification battery**

Run:

```bash
node tools/checks/native-browser-window-options.test.mjs
npm run check:ui-design
npm run check:web
npm run check:desktop
npm test
npm --workspace @mangoo/web run build
npm --workspace @mangoo/desktop run build
node .tmp/visual-check/desktop-ui-regression.mjs
```

Expected: PASS. If the machine collides on the default Vite port during any manual smoke run, use an alternate `MANGOO_WEB_URL` rather than weakening the verification target.

- [ ] **Step 2: Run the compositor-backed desktop evidence pass**

Run this PowerShell command exactly:

```powershell
$env:MANGOO_VISUAL_OS_BACKDROP="1"; node .tmp\visual-check\desktop-ui-regression.mjs; Remove-Item Env:\MANGOO_VISUAL_OS_BACKDROP
```

Expected: PASS. The summary should no longer show a lower rectangular host plate under rounded corners, and dialog captures should show visible backdrop blur.

- [ ] **Step 3: Fix only verification-driven regressions**

If any verification still fails, make only the minimal targeted changes needed. Typical fixes at this stage should look like:

```css
.workbench-column-right > .shell-titlebar::after,
.workbench-column-right > .workbench-column-body::before {
  content: none;
}
```

or:

```ts
if (event.key === "Escape" && busy) {
  return;
}
```

Do not redesign again here; close only the gaps exposed by the test and harness output.

- [ ] **Step 4: Re-run the full battery until clean**

Re-run:

```bash
node tools/checks/native-browser-window-options.test.mjs
npm run check:ui-design
npm run check:web
npm run check:desktop
npm test
npm --workspace @mangoo/web run build
npm --workspace @mangoo/desktop run build
node .tmp/visual-check/desktop-ui-regression.mjs
```

Expected: PASS with no introduced console errors, no failed app assets, and no gray host rectangle under the rounded shell.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/main.ts apps/desktop/src/preload.cjs apps/web/src/app/App.tsx apps/web/src/app-shell/AppShell.tsx apps/web/src/app-shell/ShellOverlayHost.tsx apps/web/src/app-shell/ShellTopBar.tsx apps/web/src/app-shell/useTitlebarPointerPlane.ts apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx apps/web/src/design/liquid-glass/index.ts apps/web/src/features/file-ops/ContextMenu.tsx apps/web/src/features/file-ops/DeleteConfirm.tsx apps/web/src/features/file-ops/MoveDialog.tsx apps/web/src/features/file-ops/RenameDialog.tsx apps/web/src/features/library/FilterMenu.tsx apps/web/src/features/library/LibraryCenterPane.tsx apps/web/src/features/library/LibraryInspectorPane.tsx apps/web/src/features/library/LibraryView.tsx apps/web/src/features/library/SearchBar.tsx apps/web/src/features/library/SortMenu.tsx apps/web/src/features/preview/CentralPreviewStage.tsx apps/web/src/features/preview/PreviewPanel.tsx apps/web/src/features/settings/SettingsView.tsx apps/web/src/features/settings/interfaceStyle.ts apps/web/src/features/tasks/TaskOverlay.tsx apps/web/src/features/window-chrome/WindowChrome.tsx apps/web/src/core/desktop.ts apps/web/src/styles.css tools/checks/native-browser-window-options.test.mjs tools/checks/validate-desktop-core.mjs tools/checks/validate-ui-design.mjs tools/checks/validate-web-client.mjs .tmp/visual-check/desktop-ui-regression.mjs
git commit -m "feat: complete desktop workbench shell rearchitecture"
```

## Self-Review

- Spec coverage:
  - native transparent host and first-show timing: Task 1
  - grouped interface-style parameters: Task 2
  - unified titlebar halo/drag plane and search-field outer-stroke-only rule: Task 3
  - left/center/right continuous structural surfaces and no seam/double-border composition: Task 4
  - interactive-only background halo and dialog frosting: Task 5
  - desktop verification and compositor evidence: Task 6
- Placeholder scan:
  - No `TODO`, `TBD`, or “similar to previous task” placeholders remain.
- Type consistency:
  - Plan uses `notifyDesktopShellReady`, `useTitlebarPointerPlane`, `centerPane`, `rightPane`, `outlineOnly`, and `interactiveBackground` consistently across tasks.
