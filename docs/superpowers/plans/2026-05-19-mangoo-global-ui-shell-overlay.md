# Mangoo Global UI Shell Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Mangoo's renderer from an ad hoc shell toward a long-term global app shell and overlay model while fixing preview, task, radius, background, and liquid-glass pointer regressions.

**Architecture:** Create shell and overlay boundaries first, then migrate existing UI into them. Use the existing React, TypeScript, Vite, CSS variables, and `tools/checks/validate-ui-design.mjs` contract checks. Keep Core API, Electron bridge, and media-grid virtualization boundaries intact.

**Tech Stack:** Electron 33, React 18, TypeScript, Vite, CSS variables, Lucide, `@tanstack/react-virtual`, Rust Core.

---

## File Structure

- Create: `D:/Mangoo/apps/web/src/app-shell/AppShell.tsx`
  - Owns persistent shell grid and slots.
- Create: `D:/Mangoo/apps/web/src/app-shell/ShellTopBar.tsx`
  - Owns top chrome, primary navigation buttons, task action, recent action, and window chrome slot.
- Create: `D:/Mangoo/apps/web/src/app-shell/ShellOverlayHost.tsx`
  - Owns overlay composition for task palette, task center, recent operations, context menus, and dialogs.
- Create: `D:/Mangoo/apps/web/src/features/tasks/TaskOverlay.tsx`
  - Wraps compact and full task content in floating windows.
- Modify: `D:/Mangoo/apps/web/src/app/App.tsx`
  - Becomes state wiring and shell composition; removes permanent Task workspace path.
- Modify: `D:/Mangoo/apps/web/src/features/library/LibraryView.tsx`
  - Owns inline preview state handoff and previous/next callbacks.
- Modify: `D:/Mangoo/apps/web/src/features/media-grid/MediaGrid.tsx`
  - Changes click behavior to select-only and double-click to open.
- Modify: `D:/Mangoo/apps/web/src/features/preview/PreviewPanel.tsx`
  - Replaces modal preview path with summary/inline preview states and wheel navigation.
- Modify: `D:/Mangoo/apps/web/src/features/tasks/TaskPanel.tsx`
  - Keeps compact task rendering but removes drawer-specific backdrop assumptions.
- Modify: `D:/Mangoo/apps/web/src/features/tasks/TaskCenter.tsx`
  - Makes task center content embeddable in a floating overlay.
- Modify: `D:/Mangoo/apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`
  - Adds unified pointer opacity state.
- Modify: `D:/Mangoo/apps/web/src/design/tokens.ts`
  - Adds shared radius/layout tokens for TypeScript usage.
- Modify: `D:/Mangoo/apps/web/src/styles.css`
  - Adds radius tokens, shell/overlay layout classes, preview inline styles, task overlay styles, and pointer-opacity glass CSS.
- Modify: `D:/Mangoo/tools/checks/validate-ui-design.mjs`
  - Encodes static design contract for shell, overlay, preview, media interaction, radius, and glass pointer behavior.

## Task 1: Guard The New UI Contract First

**Files:**

- Modify: `D:/Mangoo/tools/checks/validate-ui-design.mjs`

- [ ] **Step 1: Add failing static checks**

Add checks for these exact requirements:

```js
if (app.includes('activeView === "tasks"')) {
  fail("tasks must not render as a permanent workspace view");
}

if (libraryView.includes("<PreviewDialog") || previewPanel.includes("preview-dialog-backdrop")) {
  fail("library preview must be inline, not a modal dialog/backdrop");
}

if (/onClick=\\{\\(\\) => \\{[\\s\\S]*?onOpenPreview\\(item\\.id\\)/.test(mediaGrid)) {
  fail("media tile single click must select only; preview opens on double click or keyboard");
}

for (const value of [
  "onDoubleClick",
  "onPreviewWheel",
  "onPreviewPrevious",
  "onPreviewNext",
  "preview-panel-inline",
  "ShellOverlayHost",
  "TaskOverlay",
  "--radius-window",
  "--radius-overlay",
  "--radius-panel",
  "--radius-control",
  "--glass-pointer-opacity"
]) {
  if (!app.includes(value) && !libraryView.includes(value) && !mediaGrid.includes(value) && !previewPanel.includes(value) && !styles.includes(value) && !liquidGlassSurface.includes(value)) {
    fail(`global shell/overlay contract missing ${value}`);
  }
}
```

- [ ] **Step 2: Run the check and confirm it fails on current code**

Run:

```powershell
npm run check:ui-design
```

Expected before implementation: failure messages for permanent task workspace, modal preview path, single-click preview, missing inline preview wheel navigation, missing shell overlay host, missing task overlay, missing radius tokens, and missing glass pointer opacity.

- [ ] **Step 3: Commit or stage nothing**

Do not commit. Report the failing messages in the agent result.

## Task 2: Add Shell And Overlay Boundaries

**Files:**

- Create: `D:/Mangoo/apps/web/src/app-shell/AppShell.tsx`
- Create: `D:/Mangoo/apps/web/src/app-shell/ShellTopBar.tsx`
- Create: `D:/Mangoo/apps/web/src/app-shell/ShellOverlayHost.tsx`
- Modify: `D:/Mangoo/apps/web/src/app/App.tsx`

- [ ] **Step 1: Create `AppShell.tsx`**

Implement a slot-only shell:

```tsx
import type { ReactNode } from "react";

interface AppShellProps {
  topbar: ReactNode;
  sidebar: ReactNode;
  workspace: ReactNode;
  overlays: ReactNode;
}

export function AppShell({ topbar, sidebar, workspace, overlays }: AppShellProps) {
  return (
    <main className="app-shell">
      {topbar}
      {sidebar}
      <section className="app-workspace-slot">{workspace}</section>
      {overlays}
    </main>
  );
}
```

- [ ] **Step 2: Create `ShellTopBar.tsx`**

Move the topbar rendering from `App.tsx` into a component that receives `activeView`, `onSelectView`, `onOpenTasks`, `onToggleRecent`, and `scanActive`. Do not include a Tasks tab in the persistent workspace tabs. Use Library, Plugins, and Settings as workspace tabs; keep Tasks as a top action button that opens an overlay.

- [ ] **Step 3: Create `ShellOverlayHost.tsx`**

Move existing floating render calls from `App.tsx` into one host:

```tsx
export function ShellOverlayHost(props: ShellOverlayHostProps) {
  return (
    <>
      {/* TaskOverlay, recent ops, context menu, rename, move, delete */}
    </>
  );
}
```

The host receives already-built handlers and state from `App.tsx`; it must not call `useLibraryData`.

- [ ] **Step 4: Migrate `App.tsx` to shell composition**

`App.tsx` should keep state and handlers, then render:

```tsx
<AppShell
  topbar={<ShellTopBar ... />}
  sidebar={<LibrarySidebar ... />}
  workspace={workspace}
  overlays={<ShellOverlayHost ... />}
/>
```

Remove the `activeView === "tasks"` workspace branch. If a Tasks navigation affordance remains, it must call `setTaskCenterOpen(true)` and not change the workspace.

- [ ] **Step 5: Run targeted checks**

Run:

```powershell
npm run check:web
```

Expected after this task: TypeScript passes or only reports issues in the next unimplemented task's explicit preview/task props. Fix shell-related type errors before handing off.

## Task 3: Make Tasks Fully Floating

**Files:**

- Create: `D:/Mangoo/apps/web/src/features/tasks/TaskOverlay.tsx`
- Modify: `D:/Mangoo/apps/web/src/features/tasks/TaskPanel.tsx`
- Modify: `D:/Mangoo/apps/web/src/features/tasks/TaskCenter.tsx`
- Modify: `D:/Mangoo/apps/web/src/app-shell/ShellOverlayHost.tsx`
- Modify: `D:/Mangoo/apps/web/src/styles.css`

- [ ] **Step 1: Create `TaskOverlay.tsx`**

Create a wrapper with `mode: "compact" | "center"`, `open`, `onClose`, and children. It renders fixed-position floating liquid-glass windows and a lightweight backdrop for the full center mode.

- [ ] **Step 2: Refactor `TaskPanel.tsx`**

Keep task summary and rows, but remove `task-drawer-backdrop` rendering from the component. Closing is controlled by `TaskOverlay`.

- [ ] **Step 3: Refactor `TaskCenter.tsx`**

Return only task center content, not a `workspace simple-workspace` root. Keep filters, refresh, task rows, cancel, and retry behavior.

- [ ] **Step 4: Wire both task overlays in `ShellOverlayHost.tsx`**

Support compact task palette and full task center. Opening full center closes compact palette. Escape closes the active task overlay.

- [ ] **Step 5: Run contract and type checks**

Run:

```powershell
npm run check:ui-design
npm run check:web
```

Expected after this task: no task-workspace contract failure; typecheck passes except failures explicitly owned by later preview/liquid-glass tasks.

## Task 4: Convert Preview To Inline Mode

**Files:**

- Modify: `D:/Mangoo/apps/web/src/features/library/LibraryView.tsx`
- Modify: `D:/Mangoo/apps/web/src/features/preview/PreviewPanel.tsx`
- Modify: `D:/Mangoo/apps/web/src/app/App.tsx`
- Modify: `D:/Mangoo/apps/web/src/styles.css`

- [ ] **Step 1: Remove modal preview rendering from Library**

Stop importing and rendering `PreviewDialog` from `LibraryView.tsx`. Keep `previewOpen` as inline state passed into `PreviewPanel`.

- [ ] **Step 2: Add previous/next callbacks in `LibraryView.tsx`**

Use `library.media` and `library.selectedMediaId` to implement:

```ts
function selectPreviewOffset(offset: number) {
  const currentIndex = library.media.findIndex((item) => item.id === library.selectedMediaId);
  if (currentIndex < 0) return;
  const nextIndex = Math.min(library.media.length - 1, Math.max(0, currentIndex + offset));
  const next = library.media[nextIndex];
  if (next) library.setSelectedMediaId(next.id);
}
```

- [ ] **Step 3: Refactor `PreviewPanel.tsx` props**

Use props:

```ts
interface PreviewPanelProps {
  selectedMedia: MediaRecord | null;
  thumbnail?: ThumbnailResponse;
  inlineOpen: boolean;
  onOpenInline?: () => void;
  onCloseInline: () => void;
  onPreviewPrevious: () => void;
  onPreviewNext: () => void;
  children?: ReactNode;
}
```

- [ ] **Step 4: Add wheel navigation**

In the inline preview stage:

```tsx
function onPreviewWheel(event: WheelEvent<HTMLDivElement>) {
  if (!inlineOpen || !selectedMedia) return;
  event.preventDefault();
  if (event.deltaY > 0) onPreviewNext();
  if (event.deltaY < 0) onPreviewPrevious();
}
```

- [ ] **Step 5: Delete or orphan modal-specific code**

Remove `PreviewDialog` export and modal classes if no other file imports them. If keeping CSS temporarily, ensure `LibraryView` no longer uses it and the validator rejects the old path.

- [ ] **Step 6: Run checks**

Run:

```powershell
npm run check:ui-design
npm run check:web
```

Expected after this task: no modal preview contract failure; no TypeScript errors.

## Task 5: Fix Media Tile Input Semantics

**Files:**

- Modify: `D:/Mangoo/apps/web/src/features/media-grid/MediaGrid.tsx`

- [ ] **Step 1: Make single click select only**

Change media tile button `onClick` to:

```tsx
onClick={() => {
  onSelect(item.id);
}}
```

- [ ] **Step 2: Keep double click as open**

Keep:

```tsx
onDoubleClick={() => {
  onSelect(item.id);
  onOpenPreview(item.id);
}}
```

- [ ] **Step 3: Keep keyboard open**

Enter and Space should still select and open preview.

- [ ] **Step 4: Run checks**

Run:

```powershell
npm run check:ui-design
npm run check:web
```

Expected: no single-click preview contract failure; typecheck passes.

## Task 6: Normalize Radius, Background, And Liquid Glass Pointer State

**Files:**

- Modify: `D:/Mangoo/apps/web/src/design/tokens.ts`
- Modify: `D:/Mangoo/apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx`
- Modify: `D:/Mangoo/apps/web/src/styles.css`

- [ ] **Step 1: Add radius tokens**

Add CSS variables in `:root`:

```css
--radius-window: 28px;
--radius-overlay: 22px;
--radius-panel: 18px;
--radius-surface: 14px;
--radius-control: 10px;
--radius-content: 8px;
--radius-tight: 6px;
--radius-pill: 999px;
```

Add matching TypeScript constants in `tokens.ts` only if components need numeric values.

- [ ] **Step 2: Replace major one-off radii**

Use tokens for app shell, overlays, panels, controls, thumbnails, task rows, dialogs, menus, and preview stage. Keep `50%` for circles and `--radius-pill` for pills.

- [ ] **Step 3: Add pointer opacity initial state**

In `LiquidGlassSurface.tsx`, `withInitialPointer` should set:

```ts
"--glass-pointer-opacity": 0,
```

On pointer move/down set it to `1`; on pointer leave set it to `0`.

- [ ] **Step 4: Make CSS radial pointer light invisible when idle**

Update glass radial gradients so pointer light uses `var(--glass-pointer-opacity, 0)`. Hover may change border/shadow, but the pointer radial highlight must not appear from center when idle.

- [ ] **Step 5: Remove mismatched panel background gradients**

Consolidate shell background at `.app-shell`; use glass tokens for control layers and stable dark tokens for content layers.

- [ ] **Step 6: Run checks**

Run:

```powershell
npm run check:ui-design
npm run check:web
```

Expected: radius and pointer contract passes; content-layer no-backdrop checks still pass.

## Task 7: Browser Screenshot And Interaction Verification

**Files:**

- Verify running app only. Do not modify source unless defects are found.

- [ ] **Step 1: Start or reuse the dev server**

If no Vite server is running:

```powershell
npm --workspace @mangoo/web run dev -- --host 127.0.0.1 --port 5173 --strictPort
```

If port 5173 is occupied by the existing Mangoo dev server, reuse it.

- [ ] **Step 2: Open the app in browser tooling**

Open:

```text
http://127.0.0.1:5173/
```

- [ ] **Step 3: Capture screenshots**

Capture at least:

- desktop viewport, Library idle
- desktop viewport, inline preview open
- desktop viewport, task center floating open
- narrow viewport, Library idle

- [ ] **Step 4: Verify interactions**

Confirm:

- single click selects a tile and does not open inline preview
- double click opens inline preview
- wheel over inline preview changes selected media
- task action opens floating task center and does not change workspace
- idle glass surfaces do not show a default center glow
- no obvious text overlap, panel overlap, or broken background alignment

- [ ] **Step 5: If defects are found, return to the owning task**

Do not patch blindly. Identify whether the defect belongs to shell, task overlay, preview, media grid, radius, or pointer model, then rerun the relevant checks.

## Task 8: Final Verification And Review

**Files:**

- Verify current workspace.

- [ ] **Step 1: Run targeted verification**

Run:

```powershell
npm run check:ui-design
npm run check:web
npm --workspace @mangoo/web run build
git diff --check
```

Expected: all commands exit 0.

- [ ] **Step 2: Request code review**

Dispatch a reviewer with:

- requirements from `D:/Mangoo/docs/superpowers/specs/2026-05-19-mangoo-global-ui-shell-overlay-design.md`
- changed files
- verification output
- screenshots captured in Task 7

- [ ] **Step 3: Fix Critical and Important findings**

Any Critical or Important review issue must be fixed before reporting completion. Minor issues may be documented if they are outside this slice.

- [ ] **Step 4: Report outcome**

Report:

- changed files
- verification commands and exit status
- screenshot paths
- reviewer findings and fixes
- remaining risks
