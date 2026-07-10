# Unified Pointer-Distance Highlight System — Implementation Plan

> Execute task-by-task; each task ends with a build/typecheck/visual check.

**Goal:** One pointer-distance-based circular highlight (the thumbnail style: a
mouse-centered radial fill glow + radial border highlight) for ALL interactive
elements (buttons, tree rows, thumbnails, the column-resize divider), driven by a
single system. Delete the other two highlight subsystems. The glow appears only
on interactive targets, never on non-interactive background/panels. All key
params (strength, halo radius, sensing distance) adjustable in Settings.

**Architecture:** `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx` runs a
single global `pointermove` → rAF → one update pass over affordance targets,
setting `--interactive-pointer-x/y/opacity` by distance. CSS `[data-interactive-
pointer-target]::before/::after` renders the circular fill + border glow.
`apps/web/src/features/settings/interfaceStyle.ts` holds the params → CSS vars +
localStorage + Settings sliders.

## Global constraints
- `npm test` green before done (all checks + rust).
- Only interactive elements glow (they carry `data-interactive-pointer-target`);
  panels/cards/background do NOT.
- Keep the rect cache (invalidate on scroll/resize) for performance.

---

### Task 1: Delete the window-edge follow glow (req #2)
**Files:** `LiquidGlassSurface.tsx`, `styles.css`, `AppShell.tsx`
- Remove `updateWindowEdgePointers`, `hideWindowEdgePointers`, its call in the rAF
  driver + exit/cleanup, and `WINDOW_EDGE_SURFACE_SELECTOR`.
- In `styles.css`, delete `.window-edge-frame::after` (the pointer radial glow).
  KEEP `.window-edge-frame::before` (static window border).
- In `AppShell.tsx`, drop `data-window-edge-pointer` (keep the frame element +
  `data-window-edge-surface` only if `::before` still needs it; else remove both).
- Verify: window still has its static border; no glow follows the mouse near the
  window edge.

### Task 2: Delete the liquid-glass surface edge-trace highlight
**Files:** `LiquidGlassSurface.tsx`, `styles.css`
- Remove `updateLiquidGlassPointer`, `hideLiquidGlassPointers`, `hideGlassPointer`,
  their rAF/exit calls, and the `--glass-pointer-*` / `data-glass-pointer` reads.
- In `styles.css`, delete rules keyed on `[data-glass-pointer="active"]` and the
  `::after` border glow that reads `--glass-pointer-x/y`. Keep static glass
  borders/backdrops.
- Route any element that ONLY got its highlight from this system into the
  affordance system (Task 3) if it's interactive; otherwise it simply stops
  glowing (correct — panels shouldn't glow).
- Verify: buttons/controls still glow (now via affordance), panels no longer glow.

### Task 3: Ensure all interactive elements are affordance targets (req #1, #3)
**Files:** `LiquidGlassSurface.tsx` (INTERACTIVE_AFFORDANCE_SELECTOR), maybe TSX.
- Confirm the selector covers buttons, `.tree-item`, `.tile-thumb`,
  `.shell-column-resizer`, and any control that lost its glow in Task 2. Add
  `[data-liquid-glass]` interactive surfaces (buttons) that are real controls.
- Panels/cards (non-interactive `[data-liquid-glass]` surfaces) must NOT be added.
- Verify: hover glow (circular fill + border) appears on every interactive element
  and nowhere else.

### Task 4: Make sensing distance a real param + wire strength/radius (req #5)
**Files:** `interfaceStyle.ts`, `LiquidGlassSurface.tsx`, `settingsPanels.tsx`
- `pointerEdgeProximity` is hardcoded 112px. Add `pointerSensingDistance` to
  `InterfaceStylePreference` + `INTERFACE_STYLE_LIMITS` + `DEFAULT` + normalize +
  a CSS var `--glass-pointer-sensing-distance` (px). Have `pointerEdgeProximity`
  read it (via getComputedStyle on :root once, cached, or a module value synced
  from the style controller).
- Expose sliders in the Appearance tab: 高光强度 (`haloBrightness`), 光晕半径
  (`pointerResponseRadius`), 感应距离 (`pointerSensingDistance`), plus keep
  描边亮度/尺寸 (`edgeHighlightBrightness`/`edgeHighlightSize`).
- Verify: dragging each slider live-changes the glow; persists across reload;
  syncs to the main window from the settings OS window.

### Task 5: Clean up dead params (req #5 cleanup)
**Files:** `interfaceStyle.ts`, `settingsPanels.tsx`, `styles.css`
- Remove params/CSS vars that only served the deleted window-edge/glass-pointer
  systems and are now unused. Keep legacy-key migration so old localStorage still
  loads.
- Update `tools/checks/validate-ui-design.mjs` if it pins any removed marker.

### Task 6: Verify perf unchanged/better + full test
- Re-profile during thumbnail generation with pointer moving over the grid:
  confirm one querySelectorAll/frame (not three) and no getBoundingClientRect
  regression.
- `npm test` green.
