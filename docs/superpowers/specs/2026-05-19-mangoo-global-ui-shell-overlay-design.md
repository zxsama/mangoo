# Mangoo Global UI Shell And Overlay Design

Updated: 2026-05-19

## 1. Context

Mangoo is a Windows-first local media manager. It indexes existing directories, keeps original files in place, and uses the database only for Mangoo metadata such as tags, ratings, notes, favorites, task state, and thumbnails.

The current renderer already has a liquid-glass visual pass, but too much of the shell is still assembled directly in `apps/web/src/app/App.tsx` and `apps/web/src/styles.css`. That makes behavior drift likely: tasks can become a workspace page again, preview can become a modal again, and liquid-glass pointer behavior can vary by region.

This design replaces that ad hoc shape with a long-term app shell and overlay model. Future UI work must extend these boundaries instead of adding one-off layout, backdrop, or floating-layer code.

## 2. Goals

1. Establish a reusable global app shell for desktop and future Web/Docker deployment.
2. Move tasks, menus, dialogs, context menus, and future command palette into a consistent overlay layer.
3. Keep Library browsing as a high-density content workflow: single click selects, double click opens inline preview, keyboard can still open preview.
4. Render preview in the existing inspector/preview position, not as a separate modal window or full-screen backdrop.
5. Make task UI fully floating: task summary and full task center are overlays, not primary workspace pages.
6. Normalize radius, background, shadow, and liquid-glass pointer behavior through shared tokens and primitives.
7. Preserve media-grid performance by keeping thumbnails and preview canvas on stable dark content surfaces without persistent content-area blur.

## 3. Non-Goals

- Do not clone Eagle or Apple UI pixel-for-pixel.
- Do not introduce a new routing framework for this slice.
- Do not add Vitest, Playwright test runner, Tailwind, Radix, or a UI package split in this slice unless later implementation proves it necessary.
- Do not move Core API access out of existing Web/Core boundaries.
- Do not convert media grid, thumbnails, or preview canvas into glass surfaces.

## 4. Long-Term Architecture

### 4.1 Shell

`AppShell` owns only persistent layout:

- top chrome and global actions
- left library sidebar slot
- main workspace slot
- optional right inspector slot
- overlay host mount point

It must not know file-operation details, Core DTOs, thumbnail state, or task rendering internals.

### 4.2 Overlay Host

`OverlayHost` owns transient floating layers:

- compact task palette
- full task center
- recent operations
- context menu
- rename, move, delete dialogs
- future command palette and plugin overlays

The host defines z-index ordering, backdrop behavior, Escape handling conventions, and consistent floating glass surface styling.

### 4.3 Library Workspace

`LibraryView` remains the Library product workflow:

- toolbar filters and sort
- stable media grid content surface
- inspector/preview panel
- inline preview open state and next/previous preview navigation

Library state still comes from `useLibraryData`. The grid never imports Electron or filesystem APIs.

### 4.4 Preview

Preview has two states:

- `summary`: small selected-item preview and metadata inspector.
- `inline`: expanded preview in the same inspector/preview position, with close, next, previous, and wheel navigation.

Opening inline preview is not a modal. It must not render `preview-dialog-backdrop`, `role="dialog"` for image preview, or a fixed viewport-centered preview layer.

### 4.5 Tasks

Tasks are not a primary workspace view. They are system activity overlays:

- compact task palette: quick status and newest tasks.
- full task center overlay: filtering, task cards, progress, retry/cancel actions.

The topbar task action opens these overlays. The primary navigation should not reserve a permanent Tasks workspace. If a task entry remains visible in navigation for discoverability, activating it must open the floating task center and leave the current workspace context intact.

## 5. Interaction Rules

### Media Grid

- Single click selects the media item only.
- Double click selects and opens inline preview.
- Enter and Space on a focused tile select and open inline preview for accessibility.
- Right click selects the item before opening the context menu.
- Selection must remain compatible with virtualized scrolling.

### Inline Preview

- Wheel down selects the next item in the current `library.media` order.
- Wheel up selects the previous item.
- Wheel navigation only runs when inline preview is open and the pointer is over the preview stage.
- Navigation clamps at the first and last loaded media item.
- If more pages are available, preview navigation can later request more, but this slice only navigates loaded media.

### Floating Layers

- Escape closes the topmost closable overlay.
- Clicking an overlay backdrop closes that overlay when the overlay is non-destructive.
- Dialog focus trapping remains for destructive or form dialogs.
- Task overlays are floating utility windows, not modal blockers unless a destructive child dialog is open.

## 6. Visual System

### Radius Tokens

The renderer should converge on a small token set:

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

Allowed semantic exceptions:

- circles use `50%`
- pills use `--radius-pill`
- thumbnails and dense content use `--radius-content` or `--radius-tight`

New CSS must not introduce arbitrary one-off radii such as 7px, 11px, 12px, or 24px when a token fits.

### Background Rules

- Shell, topbar, sidebar, inspector, toolbar, and overlays use liquid-glass control materials.
- Grid, thumbnails, preview canvas, task list bodies, and log bodies use stable dark content surfaces.
- Content surfaces must not use persistent `backdrop-filter`.
- App background gradients must be shell-level only; individual panels should not each invent unrelated radial highlights.

### Liquid Glass Pointer Model

The primitive owns pointer behavior:

- pointer coordinates may default to `50% 50%` internally, but pointer illumination opacity defaults to `0`
- no visible center highlight appears when the pointer is outside a surface
- `onPointerMove` sets `data-glass-pointer="active"` and raises pointer opacity
- `onPointerLeave` sets `data-glass-pointer="idle"` and lowers pointer opacity to `0`
- hover may strengthen border/shadow, but radial pointer highlight only appears when pointer coordinates are active
- pressed surfaces can temporarily increase pressure and lens compression

All glass surfaces must use the same model. Component CSS must not create independent pointer-gradient systems.

## 7. File Boundaries

Target long-term structure:

```text
apps/web/src/app/
  App.tsx                         Composition and state wiring only.

apps/web/src/app-shell/
  AppShell.tsx                    Persistent shell slots and grid.
  ShellTopBar.tsx                 Top chrome, navigation, global actions.
  ShellOverlayHost.tsx            Overlay composition and z-index ordering.

apps/web/src/design/
  tokens.ts                       Shared layout/radius constants for TS.
  liquid-glass/
    LiquidGlassSurface.tsx        Pointer-aware glass primitive.

apps/web/src/features/library/
  LibraryView.tsx                 Library toolbar, grid, inspector layout.

apps/web/src/features/media-grid/
  MediaGrid.tsx                   Virtual grid and media tile input behavior.

apps/web/src/features/preview/
  PreviewPanel.tsx                Summary and inline preview states.

apps/web/src/features/tasks/
  TaskPanel.tsx                   Compact task floating palette.
  TaskCenter.tsx                  Full floating task center content.
  TaskOverlay.tsx                 Floating task window wrapper.
```

## 8. Verification Contract

Static checks in `tools/checks/validate-ui-design.mjs` should guard:

- `App.tsx` no longer renders `activeView === "tasks"` workspace content.
- Task UI has a floating overlay path.
- Media tile `onClick` does not call `onOpenPreview`.
- Media tile `onDoubleClick` calls `onOpenPreview`.
- Inline preview has wheel navigation handlers.
- Library preview path does not render `PreviewDialog` or `preview-dialog-backdrop`.
- Radius tokens exist and major selectors use them.
- Liquid glass has pointer opacity and no visible default center highlight.
- Content selectors do not gain `backdrop-filter`.

Browser verification should guard:

- desktop layout has no overlap at normal and narrow widths.
- single click selects a tile without opening inline preview.
- double click opens inline preview in the inspector/preview position.
- wheel over inline preview moves selection.
- task button opens a floating task window and does not navigate away from Library.
- no obvious default center glow is visible on idle glass surfaces.

## 9. Migration Rule

For this project, UI work should prefer long-term boundaries over local patches. When a requested change touches shell, overlays, preview, task surfaces, or design tokens, implement the boundary first, then the behavior. Small fixes are acceptable only when they do not deepen `App.tsx` or `styles.css` ownership.
