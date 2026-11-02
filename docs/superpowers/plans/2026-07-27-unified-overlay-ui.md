# Unified Overlay UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Mangoo's independent popup, menu, dialog, library, search, and notification presentation systems with one appearance-aware overlay architecture while preserving existing behavior and data contracts.

**Architecture:** Add framework-free overlay primitives under `apps/web/src/design/overlays/`: pure positioning and stack state, one runtime provider, shared acrylic surfaces, anchored portals, menu semantics, modal focus management, and shared fields. Migrate feature surfaces in isolated tasks, append each task's styles to `overlay.css`, and remove legacy `styles.css` blocks only after every feature uses the new system.

**Tech Stack:** React 18, TypeScript 5.6, native DOM/Portal APIs, existing `LiquidGlassSurface`, existing interface-style CSS variables, Node test runner, Vite, Electron visual harness. No new third-party dependency.

---

## Execution Rules

- Read `docs/superpowers/specs/2026-07-27-unified-overlay-ui-design.md` before starting each task.
- Do not modify Settings or Plugins components or their visual output.
- Do not add appearance preferences. Semantic CSS aliases may derive from existing variables only.
- Do not change Core APIs, SQLite, `@mangoo/notification-contract`, notification state ownership, or notification persistence.
- Do not add raw `fetch`, Node filesystem imports in Web production code, or third-party UI/positioning libraries.
- Preserve unrelated working-tree changes, especially:
  - `docs/benchmarks/2026-07-20-notification-center-verification.md`
  - `docs/superpowers/plans/2026-07-24-notification-failure-drilldown.md`
  - `tools/dev/_halfpage.mjs`
  - `tools/dev/_img_track.mjs`
  - `tools/dev/_newtiles.mjs`
  - `tools/dev/_shot_burst.mjs`
  - `tools/dev/_timeline.mjs`
  - `tools/dev/probe-settings-bg.mjs`
- Run `git status --short` before every commit and stage only the files listed for that task.
- Do not weaken an existing validator to make a failing implementation pass.

## Locked File Structure

### Shared overlay files

| File | Responsibility |
| --- | --- |
| `apps/web/src/design/overlays/overlayTypes.ts` | Public IDs, placement, geometry, registration, and surface-role types |
| `apps/web/src/design/overlays/overlayPosition.ts` | Pure collision-safe position calculation and virtual point anchors |
| `apps/web/src/design/overlays/overlayStack.ts` | Pure overlay stack transitions and top-layer selection |
| `apps/web/src/design/overlays/OverlayProvider.tsx` | Runtime root exclusivity, layer registration, dismissal, and focus restoration |
| `apps/web/src/design/overlays/OverlaySurface.tsx` | Material role wrapper around `LiquidGlassSurface` |
| `apps/web/src/design/overlays/AnchoredOverlay.tsx` | Portal, measurement, position updates, and layer registration |
| `apps/web/src/design/overlays/OverlayMenu.tsx` | Menu list, menu item, and roving-keyboard behavior |
| `apps/web/src/design/overlays/useOverlayFocusTrap.ts` | Shared modal tab loop, initial focus, and explicit restoration target |
| `apps/web/src/design/overlays/OverlayDialog.tsx` | Modal shell, focus trap, header, body, inline status, and actions |
| `apps/web/src/design/overlays/OverlayField.tsx` | Search/input/select frames and field error/helper presentation |
| `apps/web/src/design/overlays/overlay.css` | All new overlay-system and migrated feature styles |
| `apps/web/src/design/overlays/index.ts` | Public overlay exports |

### Shared tests

| File | Responsibility |
| --- | --- |
| `apps/web/tests/overlayPosition.test.mts` | Placement, flip, clamp, virtual anchors, and max-height behavior |
| `apps/web/tests/overlayStack.test.mts` | Root exclusivity, nesting, modal precedence, and dismissal ordering |
| `apps/web/tests/overlayFocus.test.mts` | Trigger restoration, shared modal trap reuse, and menu keyboard contract |
| `apps/web/tests/overlayContract.test.mts` | Source-level architecture, appearance, pointer, and dependency boundaries |

### Style ownership

- Tasks 1-6 append only to their named section in `apps/web/src/design/overlays/overlay.css`.
- Tasks 1-6 do not remove or rewrite legacy blocks in `apps/web/src/styles.css`.
- Task 7 is the only task that removes legacy popup, menu, dialog, library-switcher, search, and notification CSS from `styles.css`.
- `apps/web/src/main.tsx` imports `overlay.css` immediately after `styles.css`, allowing migrated surfaces to use the new rules during staged migration.
- Task 7 must leave no duplicated visual ownership between `styles.css` and `overlay.css`.

## Locked Public API

The implementation must use these signatures consistently across all tasks.

```ts
// apps/web/src/design/overlays/overlayTypes.ts
import type { CSSProperties, ReactNode } from "react";

export const OVERLAY_VIEWPORT_GUTTER_PX = 12;
export const ROOT_OVERLAY_OFFSET_PX = 6;
export const NESTED_OVERLAY_OFFSET_PX = 4;

export type OverlayRootId =
  | "library"
  | "filter"
  | "sort"
  | "layout"
  | "notifications"
  | "context-menu"
  | "file-operation";

export type OverlayLayerKind = "root" | "nested" | "modal";
export type OverlaySurfaceRole = "popover" | "nested" | "modal" | "secondary-content";
export type OverlayPlacement =
  | "bottom-start"
  | "bottom-end"
  | "top-start"
  | "top-end"
  | "right-start"
  | "left-start";

export interface OverlayRect {
  left: number;
  top: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
}

export interface OverlaySize {
  width: number;
  height: number;
}

export interface OverlayViewport {
  width: number;
  height: number;
}

export interface OverlayPositionResult {
  left: number;
  top: number;
  maxWidth: number;
  maxHeight: number;
  placement: OverlayPlacement;
}

export interface OverlayVirtualAnchor {
  getBoundingClientRect(): OverlayRect;
}

export type OverlayAnchor = HTMLElement | OverlayVirtualAnchor;

export interface OverlayLayerRegistration {
  id: string;
  rootId: OverlayRootId;
  parentId?: string;
  kind: OverlayLayerKind;
  trigger: HTMLElement | null;
  surface: HTMLElement | null;
  dismissOnEscape: boolean;
  dismissOnFocusOutside: boolean;
  dismissOnOutsidePointer: boolean;
  dismissOnWindowBlur: boolean;
  restoreFocus: boolean;
  onDismiss: () => void;
}

export interface OverlayStackApi {
  activeRootId: OverlayRootId | null;
  openRoot(rootId: OverlayRootId): void;
  closeRoot(rootId?: OverlayRootId): void;
  isRootOpen(rootId: OverlayRootId): boolean;
  registerLayer(layer: OverlayLayerRegistration): () => void;
  dismissTop(): boolean;
}

export interface AnchoredOverlayStyle extends CSSProperties {
  left: number;
  top: number;
  maxWidth: number;
  maxHeight: number;
}

export interface OverlayHeaderContent {
  description?: ReactNode;
  descriptionId?: string;
  title: ReactNode;
  titleId: string;
}
```

```ts
// apps/web/src/design/overlays/OverlaySurface.tsx
export interface OverlaySurfaceProps extends React.HTMLAttributes<HTMLDivElement> {
  surfaceRole: OverlaySurfaceRole;
  interactive?: boolean;
}

export function OverlaySurface(props: OverlaySurfaceProps): JSX.Element;
```

```ts
// apps/web/src/design/overlays/AnchoredOverlay.tsx
export interface AnchoredOverlayProps {
  anchor: OverlayAnchor | null;
  ariaLabel: string;
  children: React.ReactNode;
  className?: string;
  id: string;
  minWidth?: number;
  onDismiss: () => void;
  open: boolean;
  parentId?: string;
  placement?: OverlayPlacement;
  preferredWidth?: number;
  role?: "menu" | "listbox" | "dialog" | "status";
  restoreFocusTarget?: HTMLElement | null;
  rootId: OverlayRootId;
  surfaceRole?: "popover" | "nested";
}

export function AnchoredOverlay(props: AnchoredOverlayProps): React.ReactPortal | null;
```

```ts
// apps/web/src/design/overlays/OverlayMenu.tsx
export interface OverlayMenuListProps extends React.HTMLAttributes<HTMLDivElement> {
  ariaLabel: string;
  autoFocus?: boolean;
  role?: "menu" | "listbox";
}

export interface OverlayMenuItemProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  checked?: boolean;
  danger?: boolean;
  icon?: React.ReactNode;
  selected?: boolean;
  secondary?: React.ReactNode;
  shortcut?: React.ReactNode;
  submenu?: boolean;
}

export function OverlayMenuList(props: OverlayMenuListProps): JSX.Element;
export function OverlayMenuItem(props: OverlayMenuItemProps): JSX.Element;
```

```ts
// apps/web/src/design/overlays/OverlayDialog.tsx
export interface ModalSurfaceProps {
  ariaDescribedBy?: string;
  ariaLabelledBy: string;
  busy?: boolean;
  children: React.ReactNode;
  className?: string;
  id: string;
  initialFocusRef?: React.RefObject<HTMLElement | null>;
  onDismiss: () => void;
  open: boolean;
  restoreFocusRef?: React.RefObject<HTMLElement | null>;
  rootId: OverlayRootId;
  role?: "dialog" | "alertdialog";
}

export function ModalSurface(props: ModalSurfaceProps): JSX.Element | null;
export function OverlayDialogHeader(props: OverlayHeaderContent): JSX.Element;
export function OverlayDialogBody(props: React.HTMLAttributes<HTMLDivElement>): JSX.Element;
export function OverlayDialogActions(props: React.HTMLAttributes<HTMLElement>): JSX.Element;
export function OverlayInlineStatus(props: {
  children: React.ReactNode;
  tone: "error" | "warning" | "info";
}): JSX.Element;
```

```ts
// apps/web/src/design/overlays/useOverlayFocusTrap.ts
export interface UseOverlayFocusTrapOptions {
  initialFocusRef?: React.RefObject<HTMLElement | null>;
  restoreFocusRef?: React.RefObject<HTMLElement | null>;
}

export function useOverlayFocusTrap(
  open: boolean,
  containerRef: React.RefObject<HTMLElement | null>,
  options?: UseOverlayFocusTrapOptions
): void;
```

```ts
// apps/web/src/design/overlays/OverlayField.tsx
export interface OverlayFieldProps extends React.InputHTMLAttributes<HTMLInputElement> {
  error?: string | null;
  inputRef?: React.Ref<HTMLInputElement>;
  label?: string;
  leadingIcon?: React.ReactNode;
  trailingAction?: React.ReactNode;
}

export interface OverlaySelectProps extends React.SelectHTMLAttributes<HTMLSelectElement> {
  label: string;
}

export function OverlayField(props: OverlayFieldProps): JSX.Element;
export function OverlaySelect(props: OverlaySelectProps): JSX.Element;
```

---

### Task 1: Shared Overlay Primitives, Positioning, Stack, and Focus

**Files:**
- Create: `apps/web/src/design/overlays/overlayTypes.ts`
- Create: `apps/web/src/design/overlays/overlayPosition.ts`
- Create: `apps/web/src/design/overlays/overlayStack.ts`
- Create: `apps/web/src/design/overlays/OverlayProvider.tsx`
- Create: `apps/web/src/design/overlays/OverlaySurface.tsx`
- Create: `apps/web/src/design/overlays/AnchoredOverlay.tsx`
- Create: `apps/web/src/design/overlays/OverlayMenu.tsx`
- Create: `apps/web/src/design/overlays/useOverlayFocusTrap.ts`
- Create: `apps/web/src/design/overlays/OverlayDialog.tsx`
- Create: `apps/web/src/design/overlays/OverlayField.tsx`
- Create: `apps/web/src/design/overlays/overlay.css`
- Create: `apps/web/src/design/overlays/index.ts`
- Create: `apps/web/tests/overlayPosition.test.mts`
- Create: `apps/web/tests/overlayStack.test.mts`
- Create: `apps/web/tests/overlayFocus.test.mts`
- Create: `apps/web/tests/overlayContract.test.mts`
- Modify: `apps/web/src/main.tsx:1-8`
- Modify: `docs/project-structure.md`
- Modify: `tools/checks/validate-structure.mjs`

**Regression risks:** Incorrect viewport math can make menus unreachable; document listeners can leak; focus restoration can target detached elements; provider wrapping can affect Settings or Plugins if mounted globally.

**Required checks:** Provider wraps only the main `App`; pure functions have no DOM dependency; `overlay.css` uses existing material variables; Settings and Plugins source files remain unchanged.

- [ ] **Step 1: Write failing positioning tests**

Create `apps/web/tests/overlayPosition.test.mts` with explicit bottom placement, vertical flip, nested horizontal flip, viewport clamp, max-height, and point-anchor cases:

```ts
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  pointOverlayAnchor,
  resolveOverlayPosition
} from "../src/design/overlays/overlayPosition.ts";

const rect = (left: number, top: number, width: number, height: number) => ({
  left,
  top,
  right: left + width,
  bottom: top + height,
  width,
  height
});

describe("resolveOverlayPosition", () => {
  it("places a root popover below its trigger", () => {
    assert.deepEqual(resolveOverlayPosition({
      trigger: rect(100, 20, 28, 28),
      overlay: { width: 240, height: 180 },
      viewport: { width: 800, height: 600 },
      preferredPlacement: "bottom-start",
      offset: 6,
      gutter: 12
    }), {
      left: 100,
      top: 54,
      maxWidth: 776,
      maxHeight: 534,
      placement: "bottom-start"
    });
  });

  it("flips above when the bottom side cannot fit", () => {
    const result = resolveOverlayPosition({
      trigger: rect(400, 540, 28, 28),
      overlay: { width: 220, height: 180 },
      viewport: { width: 800, height: 600 },
      preferredPlacement: "bottom-end",
      offset: 6,
      gutter: 12
    });
    assert.equal(result.placement, "top-end");
    assert.equal(result.top, 354);
  });

  it("flips a nested menu from right to left", () => {
    const result = resolveOverlayPosition({
      trigger: rect(740, 120, 48, 32),
      overlay: { width: 190, height: 200 },
      viewport: { width: 800, height: 600 },
      preferredPlacement: "right-start",
      offset: 4,
      gutter: 12
    });
    assert.equal(result.placement, "left-start");
    assert.equal(result.left, 546);
  });

  it("clamps width and height to the viewport gutter", () => {
    const result = resolveOverlayPosition({
      trigger: rect(8, 8, 20, 20),
      overlay: { width: 500, height: 500 },
      viewport: { width: 320, height: 240 },
      preferredPlacement: "bottom-start",
      offset: 6,
      gutter: 12
    });
    assert.equal(result.left, 12);
    assert.equal(result.maxWidth, 296);
    assert.ok(result.maxHeight <= 206);
  });

  it("creates a zero-size virtual anchor for context menus", () => {
    assert.deepEqual(pointOverlayAnchor(44, 66).getBoundingClientRect(), rect(44, 66, 0, 0));
  });
});
```

- [ ] **Step 2: Run the positioning test and verify the red state**

Run:

```powershell
node --experimental-strip-types --test apps/web/tests/overlayPosition.test.mts
```

Expected: FAIL with `ERR_MODULE_NOT_FOUND` for `overlayPosition.ts`.

- [ ] **Step 3: Implement positioning types and pure geometry**

Create `overlayTypes.ts` with the locked public API. Create `overlayPosition.ts` with these exports:

```ts
import type {
  OverlayPlacement,
  OverlayPositionResult,
  OverlayRect,
  OverlaySize,
  OverlayViewport,
  OverlayVirtualAnchor
} from "./overlayTypes";

export interface ResolveOverlayPositionInput {
  trigger: OverlayRect;
  overlay: OverlaySize;
  viewport: OverlayViewport;
  preferredPlacement: OverlayPlacement;
  offset: number;
  gutter: number;
}

export function pointOverlayAnchor(x: number, y: number): OverlayVirtualAnchor {
  const value: OverlayRect = {
    left: x,
    top: y,
    right: x,
    bottom: y,
    width: 0,
    height: 0
  };
  return { getBoundingClientRect: () => value };
}

export function resolveOverlayPosition(
  input: ResolveOverlayPositionInput
): OverlayPositionResult {
  const { trigger, overlay, viewport, offset, gutter } = input;
  const maxWidth = Math.max(0, viewport.width - gutter * 2);
  const availableBelow = viewport.height - gutter - trigger.bottom - offset;
  const availableAbove = trigger.top - gutter - offset;
  const availableRight = viewport.width - gutter - trigger.right - offset;
  const availableLeft = trigger.left - gutter - offset;
  let placement = input.preferredPlacement;

  if (placement.startsWith("bottom") && overlay.height > availableBelow && availableAbove > availableBelow) {
    placement = placement === "bottom-end" ? "top-end" : "top-start";
  } else if (placement.startsWith("top") && overlay.height > availableAbove && availableBelow > availableAbove) {
    placement = placement === "top-end" ? "bottom-end" : "bottom-start";
  } else if (placement === "right-start" && overlay.width > availableRight && availableLeft > availableRight) {
    placement = "left-start";
  } else if (placement === "left-start" && overlay.width > availableLeft && availableRight > availableLeft) {
    placement = "right-start";
  }

  const isTop = placement.startsWith("top");
  const isBottom = placement.startsWith("bottom");
  const isEnd = placement.endsWith("end");
  const rawLeft = placement === "right-start"
    ? trigger.right + offset
    : placement === "left-start"
      ? trigger.left - overlay.width - offset
      : isEnd
        ? trigger.right - overlay.width
        : trigger.left;
  const rawTop = isTop
    ? trigger.top - overlay.height - offset
    : isBottom
      ? trigger.bottom + offset
      : trigger.top;
  const left = Math.min(
    Math.max(gutter, rawLeft),
    Math.max(gutter, viewport.width - gutter - Math.min(overlay.width, maxWidth))
  );
  const top = Math.max(gutter, rawTop);
  const maxHeight = Math.max(
    0,
    placement.startsWith("top")
      ? trigger.top - offset - gutter
      : placement.startsWith("bottom")
        ? viewport.height - trigger.bottom - offset - gutter
        : viewport.height - top - gutter
  );

  return { left, top, maxWidth, maxHeight, placement };
}
```

- [ ] **Step 4: Run positioning tests and make the exact assertions green**

Run the focused command from Step 2.

Expected: PASS with zero failed tests. If a numeric assertion differs, fix the pure geometry rather than loosening the test.

- [ ] **Step 5: Write failing stack-state tests**

Create `apps/web/tests/overlayStack.test.mts`:

```ts
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  closeOverlayRoot,
  createOverlayStackState,
  openOverlayRoot,
  popOverlayLayer,
  pushOverlayLayer,
  topOverlayLayer
} from "../src/design/overlays/overlayStack.ts";

const layer = (id: string, rootId: "filter" | "library", kind: "root" | "nested" | "modal") => ({
  id,
  rootId,
  kind,
  trigger: null,
  surface: null,
  dismissOnEscape: true,
  dismissOnFocusOutside: true,
  dismissOnOutsidePointer: true,
  dismissOnWindowBlur: true,
  restoreFocus: true,
  onDismiss: () => undefined
});

describe("overlay stack", () => {
  it("replaces the active root when another titlebar overlay opens", () => {
    let state = openOverlayRoot(createOverlayStackState(), "filter");
    state = pushOverlayLayer(state, layer("filter-root", "filter", "root"));
    state = openOverlayRoot(state, "library");
    assert.equal(state.activeRootId, "library");
    assert.deepEqual(state.layers, []);
  });

  it("keeps nested and modal layers ordered above their root", () => {
    let state = openOverlayRoot(createOverlayStackState(), "library");
    state = pushOverlayLayer(state, layer("library-root", "library", "root"));
    state = pushOverlayLayer(state, layer("icon-picker", "library", "nested"));
    state = pushOverlayLayer(state, layer("remove-confirm", "library", "modal"));
    assert.equal(topOverlayLayer(state)?.id, "remove-confirm");
    state = popOverlayLayer(state, "remove-confirm");
    assert.equal(topOverlayLayer(state)?.id, "icon-picker");
  });

  it("closes only the requested root family", () => {
    let state = openOverlayRoot(createOverlayStackState(), "filter");
    state = pushOverlayLayer(state, layer("filter-root", "filter", "root"));
    state = closeOverlayRoot(state, "filter");
    assert.equal(state.activeRootId, null);
    assert.deepEqual(state.layers, []);
  });
});
```

- [ ] **Step 6: Run stack tests and verify the red state**

Run:

```powershell
node --experimental-strip-types --test apps/web/tests/overlayStack.test.mts
```

Expected: FAIL with `ERR_MODULE_NOT_FOUND` for `overlayStack.ts`.

- [ ] **Step 7: Implement the pure stack and runtime provider**

Create `overlayStack.ts` with immutable transitions:

```ts
import type { OverlayLayerRegistration, OverlayRootId } from "./overlayTypes";

export interface OverlayStackState {
  activeRootId: OverlayRootId | null;
  layers: OverlayLayerRegistration[];
}

export const createOverlayStackState = (): OverlayStackState => ({
  activeRootId: null,
  layers: []
});

export function openOverlayRoot(state: OverlayStackState, rootId: OverlayRootId): OverlayStackState {
  if (state.activeRootId === rootId) return state;
  return { activeRootId: rootId, layers: [] };
}

export function closeOverlayRoot(
  state: OverlayStackState,
  rootId?: OverlayRootId
): OverlayStackState {
  if (state.activeRootId === null || (rootId !== undefined && state.activeRootId !== rootId)) {
    return state;
  }
  return createOverlayStackState();
}

export function pushOverlayLayer(
  state: OverlayStackState,
  layer: OverlayLayerRegistration
): OverlayStackState {
  return {
    ...state,
    layers: [...state.layers.filter((item) => item.id !== layer.id), layer]
  };
}

export function popOverlayLayer(state: OverlayStackState, id: string): OverlayStackState {
  return { ...state, layers: state.layers.filter((layer) => layer.id !== id) };
}

export function topOverlayLayer(state: OverlayStackState): OverlayLayerRegistration | null {
  return state.layers.at(-1) ?? null;
}
```

Implement `OverlayProvider.tsx` using this pure state. It must:

- expose the locked `OverlayStackApi` through `useOverlayStack()`;
- call the previous top layer's `onDismiss` before switching roots;
- attach one capture `pointerdown`, one capture `focusin`, one `keydown`, and window `blur/pagehide` listener while layers exist;
- dismiss only the top layer on Escape when `dismissOnEscape` is true, or on outside pointer when `dismissOnOutsidePointer` is true;
- dismiss only the top non-modal layer on focus leaving its surface/trigger family when `dismissOnFocusOutside` is true;
- restore focus only when the dismissed layer requested it and the trigger remains connected;
- remove every listener on provider unmount.

- [ ] **Step 8: Implement the shared React primitives**

Create the locked components. The material wrapper must be:

```tsx
export function OverlaySurface({
  className,
  interactive = false,
  surfaceRole,
  ...props
}: OverlaySurfaceProps) {
  return (
    <LiquidGlassSurface
      {...props}
      as="div"
      className={`overlay-surface overlay-surface-${surfaceRole}${className ? ` ${className}` : ""}`}
      data-overlay-surface-role={surfaceRole}
      interactive={interactive}
      tone="elevated"
    />
  );
}
```

`AnchoredOverlay` must use `createPortal`, `ResizeObserver`, `requestAnimationFrame`, `resolveOverlayPosition`, and provider registration. It must skip state updates when `left`, `top`, `maxWidth`, `maxHeight`, and `placement` are unchanged. Register `restoreFocusTarget` as the layer trigger when supplied; otherwise register an `HTMLElement` anchor as the trigger and use `null` for a virtual point anchor.

`OverlayMenuList` must default `autoFocus` to `true`. On mount, it focuses the first enabled descendant marked selected/checked; if none exists, it focuses the first enabled `data-overlay-menu-item="true"`. It must handle Arrow Up, Arrow Down, Home, End, Enter, and Space from that same enabled-descendant query.

Extract the existing tab-loop behavior from `apps/web/src/features/file-ops/useFocusTrap.ts` into `useOverlayFocusTrap.ts`. Keep the legacy hook unchanged until Task 4 migrates all three callers. The shared hook must prefer `restoreFocusRef.current` on cleanup; if it is absent or detached, fall back to the element focused before the modal opened.

`ModalSurface` must call `useOverlayFocusTrap(open, containerRef, { initialFocusRef, restoreFocusRef })`. It must not add its own document-level key listener.

When `busy` is true, `ModalSurface` registers `dismissOnEscape: false` and `dismissOnOutsidePointer: false`; when false, both are true. A modal always registers `dismissOnFocusOutside: false` and `dismissOnWindowBlur: false` because its focus trap owns focus containment.

`OverlayDialogHeader` must render its heading with `id={titleId}` and its optional description with `id={descriptionId}`. Every `ModalSurface.ariaLabelledBy` value must equal the associated header `titleId`; when `ariaDescribedBy` is supplied it must equal `descriptionId`.

`OverlayField` forwards native input attributes, including `data-pointer-glow="off"`, to the `<input>` rather than the frame. This opts the field itself out while leaving a trailing clear/action button eligible for the global pointer system.

- [ ] **Step 9: Add core overlay styles and import them after legacy styles**

Create the first section of `overlay.css`:

```css
.overlay-surface {
  --glass-fill: var(--glass-dialog-fill);
  --glass-blur-current: var(--glass-dialog-blur);
  --glass-border-current: var(--glass-center-stroke);
  color: var(--text-soft);
  border-radius: var(--radius-overlay);
  box-shadow: var(--glass-shadow);
}

[data-overlay-layer-kind="root"] {
  z-index: 120;
}

[data-overlay-layer-kind="nested"] {
  z-index: 130;
}

[data-overlay-layer-kind="modal"] {
  z-index: 140;
}

.overlay-surface-popover,
.overlay-surface-nested {
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.065), rgba(255, 255, 255, 0.018)),
    var(--glass-dialog-fill);
}

.overlay-surface-nested {
  background:
    linear-gradient(rgba(255, 255, 255, 0.04), rgba(255, 255, 255, 0.04)),
    var(--glass-dialog-fill);
  box-shadow: var(--glass-shadow-tight);
}

.overlay-modal-backdrop {
  position: fixed;
  inset: 0;
  display: grid;
  place-items: center;
  background: var(--dialog-backdrop-dim);
  -webkit-backdrop-filter: blur(var(--dialog-blur)) saturate(var(--glass-center-saturation));
  backdrop-filter: blur(var(--dialog-blur)) saturate(var(--glass-center-saturation));
}

[data-overlay-menu-item="true"]:focus-visible,
.overlay-field-control:focus-within {
  outline: 2px solid var(--interactive-focus);
  outline-offset: -2px;
}

@media (prefers-reduced-motion: no-preference) {
  .overlay-surface-popover[data-overlay-state="opening"] {
    animation: overlay-root-enter 140ms ease-out both;
  }
  .overlay-surface-nested[data-overlay-state="opening"] {
    animation: overlay-nested-enter 110ms ease-out both;
  }
}

@media (prefers-reduced-transparency: reduce) {
  .overlay-surface,
  .overlay-modal-backdrop {
    backdrop-filter: none;
  }
  .overlay-surface {
    background: rgba(18, 24, 30, 0.98);
  }
}

@supports not ((-webkit-backdrop-filter: blur(1px)) or (backdrop-filter: blur(1px))) {
  .overlay-surface {
    background: rgba(18, 24, 30, 0.98);
  }
}
```

Modify `apps/web/src/main.tsx`:

```ts
import "./styles.css";
import "./design/overlays/overlay.css";
```

Wrap only the main `<App />` render with `<OverlayProvider>`. Do not wrap Settings or Plugins.

- [ ] **Step 10: Update structure documentation and validator**

Add `apps/web/src/design/overlays/` to `docs/project-structure.md`. Add the directory and `index.ts` to `tools/checks/validate-structure.mjs` required paths so `npm run check:structure` recognizes the new design boundary.

- [ ] **Step 11: Write the shared focus and keyboard contract test**

Create `apps/web/tests/overlayFocus.test.mts` before implementing the final provider/menu behavior:

```ts
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

describe("shared overlay focus contract", () => {
  const provider = read("src/design/overlays/OverlayProvider.tsx");
  const menu = read("src/design/overlays/OverlayMenu.tsx");
  const dialog = read("src/design/overlays/OverlayDialog.tsx");

  it("restores only a connected trigger requested by the dismissed layer", () => {
    assert.match(provider, /restoreFocus/);
    assert.match(provider, /trigger\?\.isConnected|trigger\.isConnected/);
    assert.match(provider, /trigger\?\.focus\(|trigger\.focus\(/);
    assert.match(provider, /focusin/);
  });

  it("keeps one menu keyboard implementation", () => {
    for (const key of ["ArrowUp", "ArrowDown", "Home", "End", "Enter"]) {
      assert.match(menu, new RegExp(JSON.stringify(key).slice(1, -1)));
    }
    assert.match(menu, /["'] ["']/);
    assert.match(menu, /data-overlay-menu-item/);
    assert.match(menu, /disabled|aria-disabled/);
  });

  it("reuses the existing modal focus trap", () => {
    assert.match(dialog, /useOverlayFocusTrap/);
    assert.doesNotMatch(dialog, /document\.addEventListener\(["']keydown["']/);
  });
});
```

Run it before completing the provider/menu/dialog implementations:

```powershell
node --experimental-strip-types --test apps/web/tests/overlayFocus.test.mts
```

Expected red state: FAIL until trigger restoration, menu-key handling, and `useOverlayFocusTrap` reuse are present. After implementing those behaviors, rerun and expect PASS.

- [ ] **Step 12: Write the shared source contract test**

Create `overlayContract.test.mts` and assert:

```ts
assert.match(mainSource, /design\/overlays\/overlay\.css/);
assert.match(mainSource, /<OverlayProvider>/);
assert.doesNotMatch(settingsWindowSource, /OverlayProvider|AnchoredOverlay|ModalSurface/);
assert.doesNotMatch(pluginsWindowSource, /OverlayProvider|AnchoredOverlay|ModalSurface/);
assert.match(overlayCss, /var\(--glass-dialog-fill\)/);
assert.match(overlayCss, /var\(--glass-center-stroke\)/);
assert.doesNotMatch(overlaySourceBundle, /fetch\s*\(|node:fs|@radix|@floating-ui/);
```

- [ ] **Step 13: Run focused and boundary verification**

Run:

```powershell
node --experimental-strip-types --test apps/web/tests/overlayPosition.test.mts apps/web/tests/overlayStack.test.mts apps/web/tests/overlayFocus.test.mts apps/web/tests/overlayContract.test.mts
npm run check:structure
npm run check:web
```

Expected: all focused tests PASS, structure validation PASS, Web validation and typecheck exit `0`.

- [ ] **Step 14: Commit Task 1**

```powershell
git add apps/web/src/design/overlays apps/web/src/main.tsx apps/web/tests/overlayPosition.test.mts apps/web/tests/overlayStack.test.mts apps/web/tests/overlayFocus.test.mts apps/web/tests/overlayContract.test.mts docs/project-structure.md tools/checks/validate-structure.mjs
git commit -m "feat(web): add shared overlay primitives"
```

---

### Task 2: Titlebar Filter, Sort, Layout, Search, and Mutual Exclusion

**Files:**
- Create: `apps/web/tests/titlebarOverlays.test.mts`
- Modify: `apps/web/src/app/App.tsx:92-110, 480-560`
- Modify: `apps/web/src/app-shell/ShellTopBar.tsx:201-310`
- Modify: `apps/web/src/features/library/FilterMenu.tsx`
- Modify: `apps/web/src/features/library/SortMenu.tsx`
- Modify: `apps/web/src/features/library/LayoutMenu.tsx`
- Modify: `apps/web/src/features/library/SearchBar.tsx`
- Modify: `apps/web/src/design/overlays/overlay.css` under `/* Titlebar overlays */`

**Regression risks:** Layout can remain independently open; controlled menus can lose focus restoration; search styling can accidentally re-enable pointer glow; query updates can lag if wrapped in overlay state.

**Required checks:** Filter, sort, layout, and notification are mutually exclusive; search remains synchronous and opens no result popover; selected sort/layout options retain keyboard focus behavior.

- [ ] **Step 1: Write failing titlebar overlay contract tests**

Create `titlebarOverlays.test.mts` with source assertions:

```ts
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

describe("titlebar overlay migration", () => {
  const app = read("src/app/App.tsx");
  const filter = read("src/features/library/FilterMenu.tsx");
  const sort = read("src/features/library/SortMenu.tsx");
  const layout = read("src/features/library/LayoutMenu.tsx");
  const search = read("src/features/library/SearchBar.tsx");

  it("uses one shared root state", () => {
    assert.match(app, /useOverlayStack/);
    assert.doesNotMatch(app, /type CompactPopover/);
    assert.doesNotMatch(layout, /useState\(false\)/);
  });

  it("ports all titlebar menus through AnchoredOverlay", () => {
    for (const source of [filter, sort, layout]) assert.match(source, /AnchoredOverlay/);
    for (const source of [filter, sort, layout]) assert.doesNotMatch(source, /createPortal/);
  });

  it("keeps search inline and pointer-glow-free", () => {
    assert.match(search, /OverlayField/);
    assert.match(search, /data-pointer-glow="off"/);
    assert.doesNotMatch(search, /AnchoredOverlay|createPortal/);
  });
});
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```powershell
node --experimental-strip-types --test apps/web/tests/titlebarOverlays.test.mts
```

Expected: FAIL because current components still use local state and `createPortal`.

- [ ] **Step 3: Replace App compact-popover state with the provider API**

In `App.tsx`, remove `CompactPopover`, `activeCompactPopover`, `setCompactPopover`, and `toggleCompactPopover`. Add:

```ts
const overlays = useOverlayStack();
const notificationsOpen = overlays.isRootOpen("notifications");
const filterMenuOpen = overlays.isRootOpen("filter");
const sortMenuOpen = overlays.isRootOpen("sort");
const layoutMenuOpen = overlays.isRootOpen("layout");

const setRootOpen = useCallback(
  (rootId: OverlayRootId, open: boolean) => {
    if (open) overlays.openRoot(rootId);
    else overlays.closeRoot(rootId);
  },
  [overlays]
);
```

Use `setRootOpen` for filter, sort, layout, and notifications. Preserve `handleOpenNotificationCenter` behavior and close the notifications root before opening the secondary window.

- [ ] **Step 4: Make Filter, Sort, and Layout controlled components**

Use these required prop contracts:

```ts
interface FilterMenuProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  kind?: "image" | "video" | "other";
  minRating?: 1 | 2 | 3 | 4 | 5;
  favorite?: boolean;
  tagIds: number[];
  tags: TagRecord[];
  onSetKind: (kind: "image" | "video" | "other" | undefined) => void;
  onSetMinRating: (rating: 1 | 2 | 3 | 4 | 5 | undefined) => void;
  onToggleFavorite: () => void;
  onToggleTag: (tagId: number) => void;
  onClear: () => void;
}

interface SortMenuProps {
  iconOnly?: boolean;
  open: boolean;
  value: SortOption;
  onChange: (sort: SortOption) => void;
  onOpenChange: (open: boolean) => void;
}

interface LayoutMenuProps {
  iconOnly?: boolean;
  open: boolean;
  value: LibraryLayoutMode;
  onChange: (mode: LibraryLayoutMode) => void;
  onOpenChange: (open: boolean) => void;
  titlebarControlId?: string;
}
```

Remove uncontrolled state and feature-level document listeners. Render each popup through `AnchoredOverlay` with IDs `filter-root`, `sort-root`, and `layout-root`, respective root IDs, `bottom-start`, and widths `320`, `180`, and `180`.

- [ ] **Step 5: Use shared menu primitives**

Render sort and layout options with `OverlayMenuList` and `OverlayMenuItem`. Render filter sections inside `AnchoredOverlay`; each checkbox item must use `role="menuitemcheckbox"` and `aria-checked`, not `aria-pressed`.

Selected option fragment:

```tsx
<OverlayMenuItem
  aria-selected={selected}
  icon={<OptionIcon aria-hidden="true" size={14} />}
  onClick={() => handleSelect(option.value)}
  role="option"
  selected={selected}
>
  {option.label}
</OverlayMenuItem>
```

- [ ] **Step 6: Migrate SearchBar to OverlayField without behavior changes**

Define the trailing action before rendering the field:

```tsx
const clearButton = value ? (
  <button
    aria-label="Clear search"
    className="search-bar-clear"
    onClick={() => {
      onChange("");
      inputRef.current?.focus();
    }}
    type="button"
  >
    <X aria-hidden="true" size={13} />
  </button>
) : undefined;
```

Replace the outer `LiquidGlassSurface` with:

```tsx
<div
  className="search-bar no-drag"
  data-titlebar-control="library-search"
  data-titlebar-search="true"
>
  <OverlayField
    aria-label="Search library"
    data-pointer-glow="off"
    inputRef={inputRef}
    leadingIcon={<Search aria-hidden="true" size={15} />}
    onChange={(event) => onChange(event.target.value)}
    placeholder={placeholder}
    trailingAction={clearButton}
    type="search"
    value={value}
  />
</div>
```

The clear button must call `onChange("")` and synchronously return focus to `inputRef.current`.

- [ ] **Step 7: Add the titlebar section to overlay.css**

Add only selectors prefixed by `.overlay-titlebar-`, `.filter-menu-`, `.sort-menu-`, `.layout-menu-`, or `.search-bar` in this section. Use `--radius-control`, `--radius-tight`, `--glass-center-fill`, and `--text-*`. Do not use feature-local `backdrop-filter`, `box-shadow`, or z-index.

- [ ] **Step 8: Run focused and Web checks**

```powershell
node --experimental-strip-types --test apps/web/tests/titlebarOverlays.test.mts apps/web/tests/overlayStack.test.mts
npm run check:web
npm run check:ui-design
```

Expected: focused tests PASS; Web and UI design checks exit `0`; no filter, sort, or layout component imports `createPortal`.

- [ ] **Step 9: Manual regression check for Task 2**

Run `npm run dev`, then verify:

- Filter open, then Sort: Filter closes before Sort appears.
- Sort open, then Layout: Sort closes before Layout appears.
- Notification open, then Filter: Notification closes.
- Search query still updates the grid on every input event.
- Escape returns focus to the menu trigger.

Expected: no overlapping root popovers and no search behavior change.

- [ ] **Step 10: Commit Task 2**

```powershell
git add apps/web/src/app/App.tsx apps/web/src/app-shell/ShellTopBar.tsx apps/web/src/features/library/FilterMenu.tsx apps/web/src/features/library/SortMenu.tsx apps/web/src/features/library/LayoutMenu.tsx apps/web/src/features/library/SearchBar.tsx apps/web/src/design/overlays/overlay.css apps/web/tests/titlebarOverlays.test.mts
git commit -m "feat(web): unify titlebar overlays"
```

---

### Task 3: LibrarySwitcher and Nested Library Surfaces

**Files:**
- Create: `apps/web/tests/librarySwitcherOverlay.test.mts`
- Modify: `apps/web/src/app/App.tsx`
- Modify: `apps/web/src/app-shell/ShellTopBar.tsx:111-199`
- Modify: `apps/web/src/features/library/LibrarySwitcher.tsx`
- Modify: `apps/web/src/features/library/libraryGlyph.tsx:89-128`
- Modify: `apps/web/src/design/overlays/overlay.css` under `/* Library switcher */`
- Modify: `apps/web/tests/removeRootConfirm.test.mts`

**Regression risks:** Changing anchors can break the titlebar location; nested menus can close the parent; shared icon-picker styling can alter Settings; library reload or switch behavior can accidentally change.

**Required checks:** Settings icon picker remains visually unchanged; create/open/add/remove/switch callbacks are unchanged; cancellation of remove confirmation returns to the remove trigger.

- [ ] **Step 1: Write failing library overlay tests**

Create `librarySwitcherOverlay.test.mts` asserting:

```ts
assert.match(appSource, /isRootOpen\("library"\)/);
assert.match(switcherSource, /AnchoredOverlay/);
assert.match(switcherSource, /ModalSurface/);
assert.match(switcherSource, /surfaceRole="nested"/);
assert.doesNotMatch(switcherSource, /library-switcher-backdrop/);
assert.doesNotMatch(switcherSource, /role="dialog"[^]*onClick=\{\(e\) => e\.stopPropagation/);
assert.doesNotMatch(libraryGlyphSource, /OverlaySurface|AnchoredOverlay/);
assert.doesNotMatch(settingsPanelsSource, /nested-overlay|library-switcher/);
```

Also preserve `REMOVE_ROOT_CONFIRM` text assertions in `removeRootConfirm.test.mts`.

- [ ] **Step 2: Run the tests and verify the red state**

```powershell
node --experimental-strip-types --test apps/web/tests/librarySwitcherOverlay.test.mts apps/web/tests/removeRootConfirm.test.mts
```

Expected: the new contract test FAILS because LibrarySwitcher uses custom backdrops and absolute nested menus; existing confirmation tests remain PASS.

- [ ] **Step 3: Expose the library trigger element**

Add this prop to `ShellTitlebarPrimaryActions`:

```ts
librarySwitcherButtonRef?: React.Ref<HTMLButtonElement>;
```

Assign it to the library button's `ref`. In `App.tsx`, store the element with `useState<HTMLButtonElement | null>(null)`, open root `library`, and pass the anchor to `LibrarySwitcher`.

Required LibrarySwitcher props:

```ts
export interface LibrarySwitcherProps {
  anchor: HTMLElement | null;
  library: LibraryState;
  onClose: () => void;
  open: boolean;
}
```

- [ ] **Step 4: Migrate the root switcher surface**

Replace the full-screen custom backdrop and raw panel with the shared root. Move the current search header, library list, and footer JSX directly inside this root rather than introducing another material wrapper:

```tsx
<AnchoredOverlay
  anchor={anchor}
  ariaLabel="资源库"
  className="library-switcher"
  id="library-root"
  onDismiss={onClose}
  open={open}
  placement="bottom-start"
  preferredWidth={460}
  role="dialog"
  rootId="library"
>
  <div className="library-switcher-header">{searchField}{closeButton}</div>
  <ul className="library-switcher-list">{libraryRows}</ul>
  <footer className="library-switcher-footer">{footerActions}</footer>
</AnchoredOverlay>
```

In the same function, define `searchField`, `closeButton`, `libraryRows`, and `footerActions` as JSX constants immediately before `return`. They must contain the complete existing query input, close callback, filtered-library row mapping, and create/open callbacks respectively; do not omit row actions, create a second component, or create a second surface.

Keep current `listLibraries`, `switchLibrary`, `createLibrary`, `openLibrary`, `pickLibraryFile`, `pickNewLibraryPath`, `pickNativeFolder`, `setLibraryIcon`, `library.addRoot`, and `library.removeRoot` calls unchanged.

- [ ] **Step 5: Migrate icon and remove-folder nested overlays**

Keep `LibraryIconPicker` as content-only so Settings does not acquire overlay behavior. Replace its anonymous prop type with this exact contract:

```ts
export interface LibraryIconPickerProps {
  color?: string;
  icon?: string;
  onPick: (icon: string, color: string) => void;
  presentation?: "embedded" | "overlay";
}
```

Default `presentation` to `"embedded"` and render ``className={`library-icon-picker library-icon-picker--${presentation}`}``. Existing Settings call sites pass no new prop and retain the embedded presentation. Wrap the overlay presentation from `LibrarySwitcher`:

```tsx
<AnchoredOverlay
  anchor={iconMenuAnchor}
  ariaLabel="选择资源库图标"
  id={`library-icon-${libraryId}`}
  onDismiss={() => setIconMenuLibraryId(null)}
  open={iconMenuLibraryId === libraryId}
  parentId="library-root"
  placement="right-start"
  preferredWidth={208}
  role="menu"
  rootId="library"
  surfaceRole="nested"
>
  <LibraryIconPicker
    color={color}
    icon={icon}
    onPick={applyIcon}
    presentation="overlay"
  />
</AnchoredOverlay>
```

Wrap the remove-folder list the same way with ID `library-remove-folders`, `right-start`, and width `220`. Use `OverlayMenuList` and `OverlayMenuItem`.

- [ ] **Step 6: Migrate remove confirmation to ModalSurface**

Use:

```tsx
<ModalSurface
  ariaDescribedBy="library-remove-confirm-description"
  ariaLabelledBy="library-remove-confirm-title"
  id="library-remove-confirm"
  initialFocusRef={cancelRef}
  onDismiss={() => setConfirmRemove(null)}
  open={confirmRemove !== null}
  role="alertdialog"
  rootId="library"
>
  <OverlayDialogHeader
    description={REMOVE_ROOT_CONFIRM.body}
    descriptionId="library-remove-confirm-description"
    title={confirmRemove ? REMOVE_ROOT_CONFIRM.title(confirmRemove.name) : ""}
    titleId="library-remove-confirm-title"
  />
  <OverlayDialogActions>
    <button ref={cancelRef} type="button" onClick={() => setConfirmRemove(null)}>取消</button>
    <LiquidGlassButton tone="danger" type="button" onClick={() => void confirmRootRemoval()}>移除</LiquidGlassButton>
  </OverlayDialogActions>
</ModalSurface>
```

Pass `cancelRef` as `initialFocusRef`. Cancelling must leave the switcher open.

- [ ] **Step 7: Add library-switcher styles to overlay.css**

Define a structural header hairline, dark list tone, graded footer, 40px rows, 28px row actions, ellipsized paths, filled active rows, and nested icon grid. Use only existing side, center, dialog, radius, text, accent, and danger variables.

- [ ] **Step 8: Run focused and boundary checks**

```powershell
node --experimental-strip-types --test apps/web/tests/librarySwitcherOverlay.test.mts apps/web/tests/removeRootConfirm.test.mts apps/web/tests/overlayPosition.test.mts apps/web/tests/overlayStack.test.mts
npm run check:web
npm run check:ui-design
```

Expected: all focused tests PASS and both checks exit `0`.

- [ ] **Step 9: Manual library regression check**

Verify switch, create, open, add folder, open icon picker, choose icon/color, open remove menu, cancel confirmation, and confirm removal. Resize the main window so nested menus must flip.

Expected: no parent dismissal during nested interaction, no clipped nested surface, and Settings icon presentation unchanged.

- [ ] **Step 10: Commit Task 3**

```powershell
git add apps/web/src/app/App.tsx apps/web/src/app-shell/ShellTopBar.tsx apps/web/src/features/library/LibrarySwitcher.tsx apps/web/src/features/library/libraryGlyph.tsx apps/web/src/design/overlays/overlay.css apps/web/tests/librarySwitcherOverlay.test.mts apps/web/tests/removeRootConfirm.test.mts
git commit -m "feat(web): unify library switcher overlays"
```

---

### Task 4: ContextMenu and File-Operation Dialogs

**Files:**
- Create: `apps/web/tests/fileOperationOverlays.test.mts`
- Modify: `apps/web/src/app/App.tsx` context-menu open/close handlers
- Modify: `apps/web/src/app-shell/ShellOverlayHost.tsx`
- Modify: `apps/web/src/features/file-ops/ContextMenu.tsx`
- Modify: `apps/web/src/features/file-ops/RenameDialog.tsx`
- Modify: `apps/web/src/features/file-ops/MoveDialog.tsx`
- Modify: `apps/web/src/features/file-ops/DeleteConfirm.tsx`
- Modify: `apps/web/src/design/overlays/overlay.css` under `/* Context menus and file dialogs */`

**Regression risks:** Context-menu point positioning can move from the click origin; index-to-item keyboard mapping can break around separators; busy dialogs can become dismissible; move-tree focus can be trapped incorrectly.

**Required checks:** Existing rename validation, move exclusions, cross-root error, recycle behavior, permanent typed confirmation, and file-operation controller calls remain unchanged.

- [ ] **Step 1: Write failing file-operation overlay tests**

Create `fileOperationOverlays.test.mts`:

```ts
assert.match(contextSource, /pointOverlayAnchor/);
assert.match(contextSource, /AnchoredOverlay/);
assert.match(contextSource, /OverlayMenuList/);
assert.match(renameSource, /ModalSurface/);
assert.match(moveSource, /ModalSurface/);
assert.match(deleteSource, /ModalSurface/);
for (const source of [renameSource, moveSource, deleteSource]) {
  assert.match(source, /rootId="file-operation"/);
  assert.doesNotMatch(source, /\.\/useFocusTrap/);
}
assert.doesNotMatch(contextSource, /window\.addEventListener\("mousedown"/);
assert.doesNotMatch(renameSource + moveSource + deleteSource, /className="dialog-backdrop"/);
assert.match(deleteSource, /confirmText\.trim\(\) === expected/);
assert.match(moveSource, /excludedFolderIds/);
```

- [ ] **Step 2: Run the focused test and verify the red state**

```powershell
node --experimental-strip-types --test apps/web/tests/fileOperationOverlays.test.mts
```

Expected: FAIL because the existing menu and dialogs use custom surfaces.

- [ ] **Step 3: Integrate context-menu and file-operation root state**

Capture `document.activeElement` before replacing `menu` state and store it in `contextMenuReturnFocusRef: React.MutableRefObject<HTMLElement | null>`. When file, folder, or root context-menu state is created in `App.tsx`, call `overlays.openRoot("context-menu")`.

Keep two callbacks with distinct ownership:

```ts
const dismissContextMenuState = useCallback(() => setMenu(null), []);
const closeContextMenu = useCallback(() => {
  setMenu(null);
  overlays.closeRoot("context-menu");
}, [overlays]);
```

Pass `dismissContextMenuState` to `ContextMenu.onDismiss` because `OverlayProvider` already mutates stack state for Escape, outside pointer, blur, and root replacement. Pass `closeContextMenu` into ordinary menu items that explicitly close the root. This separation prevents an `onDismiss` callback from recursively closing a newly opened root.

For rename, move, recycle, and permanent-delete menu items, do not call the normal close helper followed by a later modal open. Use one transition helper:

```ts
const beginFileOperation = useCallback(
  (openOperation: () => void) => {
    overlays.openRoot("file-operation");
    dismissContextMenuState();
    openOperation();
  },
  [dismissContextMenuState, overlays]
);
```

Pass this helper into the file/folder menu builders and wrap each existing `fileOps.openRename`, `fileOps.openMove`, or `fileOps.openDelete` call with it. This root switch dismisses the context-menu layer before the modal mounts without briefly clearing the shared stack. Derive `fileOperationOpen` from the three controller targets and close only `file-operation` when a previous `true` value transitions to `false`; do not close it on the initial false render.

```ts
const fileOperationOpen = Boolean(
  fileOps.rename.target || fileOps.move.target || fileOps.remove.target
);
const previousFileOperationOpenRef = useRef(false);

useEffect(() => {
  if (previousFileOperationOpenRef.current && !fileOperationOpen) {
    overlays.closeRoot("file-operation");
  }
  previousFileOperationOpenRef.current = fileOperationOpen;
}, [fileOperationOpen, overlays]);
```

Add `contextMenuReturnFocusRef: React.RefObject<HTMLElement | null>` and `onDismissContextMenu: () => void` to `ShellOverlayHostProps`. Pass the ref itself to all file-operation dialogs and `contextMenuReturnFocusRef.current` to `ContextMenu.restoreFocusTarget`.

- [ ] **Step 4: Migrate ContextMenu to a virtual anchored overlay**

Create the anchor once per open:

```ts
const anchor = useMemo(() => pointOverlayAnchor(x, y), [x, y]);
```

Add `restoreFocusTarget?: HTMLElement | null` to `ContextMenuProps`, rename its callback prop to `onDismiss`, and render:

```tsx
<AnchoredOverlay
  anchor={anchor}
  ariaLabel={ariaLabel}
  className="context-menu"
  id="context-menu-root"
  onDismiss={onDismiss}
  open
  placement="bottom-start"
  preferredWidth={180}
  restoreFocusTarget={restoreFocusTarget}
  role="menu"
  rootId="context-menu"
>
  <OverlayMenuList ariaLabel={ariaLabel}>
    {items.map((item) => item.separator ? (
      <div className="overlay-menu-separator" key={item.id} role="separator" />
    ) : (
      <OverlayMenuItem
        danger={item.danger}
        disabled={item.disabled}
        icon={item.icon}
        key={item.id}
        onClick={item.onSelect}
        role="menuitem"
      >
        {item.label}
      </OverlayMenuItem>
    ))}
  </OverlayMenuList>
</AnchoredOverlay>
```

Keep separators aligned to the original item indices so Space and Enter invoke the correct item.

- [ ] **Step 5: Migrate RenameDialog**

Replace its backdrop and surface with `ModalSurface rootId="file-operation"`, remove its feature-local `useFocusTrap` call, keep selection behavior, and render validation/server errors through `OverlayInlineStatus tone="error"`.

Use `OverlayField` with `inputRef={inputRef}`, `error={clientError ?? serverError}`, and unchanged submit validation. Pass `restoreFocusRef={contextMenuReturnFocusRef}` from `ShellOverlayHost`.

- [ ] **Step 6: Migrate MoveDialog**

Use `ModalSurface rootId="file-operation"` with `className="overlay-dialog-wide"` and `restoreFocusRef={contextMenuReturnFocusRef}`. Remove the feature-local focus-trap call. Keep `FolderBranch`, `RootBranch`, `excludedFolderIds`, target selection, and submit behavior unchanged. Render `.move-tree` as the dominant scroll region with no independent acrylic background.

- [ ] **Step 7: Migrate DeleteConfirm**

Use `ModalSurface rootId="file-operation" role="alertdialog" restoreFocusRef={contextMenuReturnFocusRef}`. Remove the feature-local focus-trap call. For permanent delete, preserve `confirmInputRef` as initial focus. For recycle-bin confirmation, pass a cancel-button ref as initial focus. Pass `busy` to prevent Escape and outside dismissal during submission.

- [ ] **Step 8: Add context and dialog styles**

Append shared menu rows, sparse separators, modal header hairline, 32px fields/actions, dominant move tree, inline status tones, and disabled/busy states to the owned overlay.css section. Do not add per-item hover backgrounds.

- [ ] **Step 9: Run focused and Web checks**

```powershell
node --experimental-strip-types --test apps/web/tests/fileOperationOverlays.test.mts apps/web/tests/overlayPosition.test.mts apps/web/tests/overlayStack.test.mts
npm run check:web
npm run check:ui-design
```

Expected: all focused tests PASS; no TypeScript or validator failures.

- [ ] **Step 10: Manual context and dialog regression check**

Open media, folder, and root menus at all four viewport corners. Complete rename, move, recycle, permanent delete, server-error, and busy-state paths with keyboard only.

Expected: menus remain fully visible, focus restores correctly, busy dialogs do not dismiss, and destructive actions require the same confirmation as before.

- [ ] **Step 11: Commit Task 4**

```powershell
git add apps/web/src/app/App.tsx apps/web/src/app-shell/ShellOverlayHost.tsx apps/web/src/features/file-ops/ContextMenu.tsx apps/web/src/features/file-ops/RenameDialog.tsx apps/web/src/features/file-ops/MoveDialog.tsx apps/web/src/features/file-ops/DeleteConfirm.tsx apps/web/src/design/overlays/overlay.css apps/web/tests/fileOperationOverlays.test.mts
git commit -m "feat(web): unify file operation overlays"
```

---

### Task 5: Compact Notification Product Redesign

**Files:**
- Modify: `apps/web/src/features/notifications/NotificationPopover.tsx`
- Modify: `apps/web/src/features/notifications/notificationPopoverPresentation.ts`
- Delete: `apps/web/src/features/notifications/notificationPopoverPosition.ts`
- Delete: `apps/web/src/features/notifications/notificationPopoverMeasurement.ts`
- Modify: `apps/web/tests/notificationPopover.test.mts`
- Modify: `apps/web/tests/fixtures/notificationPopoverVisualFixture.tsx`
- Modify: `apps/web/tests/fixtures/notificationPopoverVisualFixture.css`
- Modify: `apps/web/src/design/overlays/overlay.css` under `/* Compact notification */`
- Modify: `tools/checks/validate-web-client.mjs`

**Regression risks:** Removing recent results can accidentally remove active failure visibility; replacing custom measurement can alter right-edge positioning; the footer can scroll out of view; compact rendering can acquire cancel/retry actions.

**Required checks:** Active operations, scan paths, thumbnail aggregation, degraded health, and open-center intent remain; `model.results` is not rendered; notification model and contract are unchanged.

- [ ] **Step 1: Rewrite compact notification tests to the approved product contract**

Update `notificationPopover.test.mts` to assert:

```ts
assert.match(popoverSource, /后台活动/);
assert.match(popoverSource, /model\.active/);
assert.match(popoverSource, /model\.thumbnail\.summaries/);
assert.match(popoverSource, /model\.degraded/);
assert.match(popoverSource, /onOpenCenter/);
assert.doesNotMatch(popoverSource, /model\.results/);
assert.doesNotMatch(popoverSource, /cancel\s*\(|retry\s*\(/);
assert.doesNotMatch(popoverSource, /notification-result-list|最近结果/);
assert.match(popoverSource, /AnchoredOverlay/);
```

Remove tests for the feature-specific geometry/measurement modules. Keep tests for active operation phase, progress label, percentage, and scan paths.

- [ ] **Step 2: Run the notification popover test and verify the red state**

```powershell
node --experimental-strip-types --test apps/web/tests/notificationPopover.test.mts
```

Expected: FAIL because the current compact popover renders recent results and uses custom geometry modules.

- [ ] **Step 3: Replace custom positioning with AnchoredOverlay**

Delete the local position hook, measurement observer, portal call, and feature geometry imports. Render the existing header, activity body, and footer directly inside:

```tsx
<AnchoredOverlay
  anchor={anchor}
  ariaLabel="后台活动"
  className="notification-popover"
  id="notifications-root"
  onDismiss={onClose}
  open
  placement="bottom-end"
  preferredWidth={360}
  role="status"
  rootId="notifications"
>
  <header className="notification-popover-header">
    <h2>后台活动</h2>
    <span data-notification-health={model.degraded ? "degraded" : "healthy"}>
      {model.degraded ? "同步受限" : "运行正常"}
    </span>
  </header>
  <div className="notification-popover-body" data-overlay-scroll-container>
    {activityContent}
  </div>
  <footer className="notification-popover-footer">
    <LiquidGlassButton onClick={() => onOpenCenter()} tone="control" type="button">
      打开完整通知中心
    </LiquidGlassButton>
  </footer>
</AnchoredOverlay>
```

Define `activityContent` immediately before `return` from the existing degraded, `model.active`, `model.thumbnail.summaries`, and idle branches. It must not read `model.results`.

Add `onClose: () => void` to `NotificationPopoverProps` and pass it from `ShellOverlayHost`.

- [ ] **Step 4: Remove terminal-result rendering from compact content**

Delete `ResultDescription`, `resultIcon`, result-row JSX, and `resultOperationPresentation` if it has no remaining production references. Keep failed active operation state and degraded health visible through existing active/health models.

Compact content order is fixed:

1. Structural header with “后台活动” and concise health text.
2. Degraded detail when present.
3. Active logical operations.
4. Thumbnail-cache summaries.
5. One-line idle state when neither active nor thumbnail work exists.
6. Fixed footer action “打开完整通知中心”.

- [ ] **Step 5: Update compact visual fixture**

Remove recent-results fixture rows and add states for active scan, active thumbnail generation, empty, and degraded. The fixture must still expose deterministic controls used by the desktop visual harness.

- [ ] **Step 6: Add compact notification styles**

Append a 360px bounded flex surface, structural header hairline, scrollable body, fixed footer, 40px activity rows, 3px progress track, tabular percentage, truncated scan paths, and compact empty state. Do not add nested cards around each section.

- [ ] **Step 7: Remove obsolete validator requirements**

In `validate-web-client.mjs`, remove requirements for `notificationPopoverMeasurement.ts` and replace them with assertions that `NotificationPopover.tsx` imports `AnchoredOverlay` and does not contain `ResizeObserver`, `MutationObserver`, or `model.results`.

- [ ] **Step 8: Run focused and boundary checks**

```powershell
node --experimental-strip-types --test apps/web/tests/notificationPopover.test.mts apps/web/tests/notificationModel.test.mts apps/web/tests/notificationConcurrency.test.mts
npm run check:web
npm run check:ui-design
```

Expected: notification tests PASS, Web/UI checks exit `0`, and deleted modules have no references under `apps/web/src`, `apps/web/tests`, or `tools/checks`.

- [ ] **Step 9: Manual compact notification check**

Verify active scan, multiple scan paths, thumbnail generation, degraded health, empty activity, opening the full center, narrow width, and mutual exclusion with other titlebar overlays.

Expected: compact surface shows progress only and never shows cancel, retry, history, logs, or recent results.

- [ ] **Step 10: Commit Task 5**

```powershell
git add apps/web/src/features/notifications/NotificationPopover.tsx apps/web/src/features/notifications/notificationPopoverPresentation.ts apps/web/tests/notificationPopover.test.mts apps/web/tests/fixtures/notificationPopoverVisualFixture.tsx apps/web/tests/fixtures/notificationPopoverVisualFixture.css apps/web/src/design/overlays/overlay.css tools/checks/validate-web-client.mjs
git rm apps/web/src/features/notifications/notificationPopoverPosition.ts apps/web/src/features/notifications/notificationPopoverMeasurement.ts
git commit -m "feat(web): redesign compact notification activity"
```

---

### Task 6: Full Notification Center IA and Control Redesign

**Files:**
- Modify: `apps/web/src/features/notifications/NotificationWindow.tsx`
- Modify: `apps/web/src/features/notifications/NotificationStatusView.tsx`
- Modify: `apps/web/src/features/notifications/NotificationHistoryView.tsx`
- Modify: `apps/web/src/features/notifications/ProcessLogView.tsx`
- Modify: `apps/web/tests/notificationWindow.test.mts`
- Modify: `apps/web/tests/notificationDrilldown.test.mts`
- Modify: `apps/web/tests/fixtures/notificationWindowVisualFixture.tsx`
- Modify: `apps/web/tests/fixtures/notificationWindowVisualFixture.css`
- Modify: `apps/web/src/design/overlays/overlay.css` under `/* Full notification center */`

**Regression risks:** Visual reordering can break failure drill-down; filter controls can change query values; history virtualization can lose measurement ownership; process-log rows can become expensive; narrow navigation can lose tab semantics.

**Required checks:** Three pages remain; status cancel, history cancel/retry, pagination, virtualization, process-log subscription/export/clear/media focus, failure snapshots, and notification data types remain unchanged.

- [ ] **Step 1: Update full-center contract tests before implementation**

Change `notificationWindow.test.mts` to read `overlay.css` and assert:

```ts
assert.match(windowSource, /当前状态/);
assert.match(windowSource, /历史记录/);
assert.match(windowSource, /进程日志/);
assert.match(windowSource, /role="tablist"/);
assert.match(windowSource, /OverlayField|OverlaySelect/);
assert.match(overlayCss, /--secondary-window-pane-dark/);
assert.match(overlayCss, /--secondary-window-pane-light/);
assert.match(overlayCss, /--secondary-window-pane-end/);
assert.doesNotMatch(overlayCss, /notification-window-nav[^}]*border-(right|inline-end):\s*1px/s);
assert.doesNotMatch(windowSource + statusSource, /notification-view-eyebrow/);
assert.match(historySource, /cancel/);
assert.match(historySource, /retry/);
assert.match(logSource, /focusMedia/);
```

Keep all existing data-boundary and virtualization assertions.

- [ ] **Step 2: Run notification-window and drill-down tests to verify red**

```powershell
node --experimental-strip-types --test apps/web/tests/notificationWindow.test.mts apps/web/tests/notificationDrilldown.test.mts
```

Expected: FAIL on new overlay-control and eyebrow-removal assertions; existing drill-down assertions remain PASS.

- [ ] **Step 3: Restructure NotificationWindow navigation without changing page state**

Keep `NotificationPage = "status" | "history" | "logs"`, `applyIntent`, `logFilter`, `failureSnapshot`, and arrow-key tab behavior. Change labels to `当前状态`, `历史记录`, and `进程日志`. Preserve `SecondaryWindowFrame` and `useSecondaryWindowInterfaceStyle`.

The body remains two zones at normal width and becomes top tabs under the narrow breakpoint. No vertical divider is added.

- [ ] **Step 4: Redesign current status as continuous sections**

Preserve `NotificationStatusView` props and operation mapping. Replace card wrappers with:

```tsx
<section className="notification-status-section" aria-labelledby={headingId}>
  <header className="notification-section-heading">
    <SectionIcon aria-hidden="true" size={15} />
    <h2 id={headingId}>{title}</h2>
    <span className="notification-section-count">{count}</span>
  </header>
  <div className="notification-section-content">{children}</div>
</section>
```

Keep cancel actions only for `cancellable` operations and keep thumbnail failure drill-down unchanged.

- [ ] **Step 5: Redesign history controls and rows**

Use `OverlayField` for history search and `OverlaySelect` for status, category, source, kind, and library filters. Do not change `DesktopNotificationHistoryQuery`, `PAGE_SIZE`, `useVirtualizer`, or page loading.

Keep details inline under the selected row. Use `OverlayMenuItem` only for button styling when semantics remain a button, not to change list semantics.

- [ ] **Step 6: Redesign process-log toolbar and rows**

Use `OverlayField` and `OverlaySelect`. Keep `LOG_PAGE_SIZE = 200`, `MAX_RENDERED_LOG_LINES`, `useVirtualizer`, bounded paused batches, gap handling, export, clear, and subscription cleanup.

At normal width, render stable columns through CSS grid:

```css
.notification-log-line {
  display: grid;
  grid-template-columns: 72px 92px 56px minmax(0, 1fr);
  align-items: baseline;
}
```

At narrow width, switch to two rows while preserving DOM order and accessible text.

- [ ] **Step 7: Update the notification visual fixture**

Retain controlled status/history/logs/degraded states, retry error, gap emission, subscription counters, and thumbnail failure intent. Update fixture labels and control classes to the new IA.

- [ ] **Step 8: Add full-center styles to overlay.css**

Define:

- dark 184px navigation using `--secondary-window-pane-dark`;
- content gradient using pane light, side fill, and pane end;
- no vertical divider;
- 20px content padding at normal width;
- 14px section spacing without card borders around every section;
- 32px shared fields/buttons;
- sticky filter/log toolbar;
- narrow top-tab layout;
- tabular time and progress values.

- [ ] **Step 9: Run notification and Web verification**

```powershell
node --experimental-strip-types --test apps/web/tests/notificationWindow.test.mts apps/web/tests/notificationDrilldown.test.mts apps/web/tests/notificationModel.test.mts apps/web/tests/notificationConcurrency.test.mts
npm run check:web
npm run check:ui-design
```

Expected: all notification tests PASS; Web/UI checks exit `0`; no notification DTO or contract file changes appear in `git status`.

- [ ] **Step 10: Manual notification-center regression check**

Verify status active/cancel, thumbnail failure drill-down, history filtering/pagination/cancel/retry, log live/pause/export/clear/gap/media focus, degraded state, and narrow window tabs.

Expected: data operations match the pre-redesign behavior and only presentation/interaction layout changes.

- [ ] **Step 11: Commit Task 6**

```powershell
git add apps/web/src/features/notifications/NotificationWindow.tsx apps/web/src/features/notifications/NotificationStatusView.tsx apps/web/src/features/notifications/NotificationHistoryView.tsx apps/web/src/features/notifications/ProcessLogView.tsx apps/web/tests/notificationWindow.test.mts apps/web/tests/notificationDrilldown.test.mts apps/web/tests/fixtures/notificationWindowVisualFixture.tsx apps/web/tests/fixtures/notificationWindowVisualFixture.css apps/web/src/design/overlays/overlay.css
git commit -m "feat(web): redesign notification center"
```

---

### Task 7: Legacy Style Removal and Validator Enforcement

**Files:**
- Modify: `apps/web/src/styles.css`
- Modify: `apps/web/src/design/overlays/overlay.css`
- Delete: `apps/web/src/features/library/anchoredPopover.ts`
- Delete: `apps/web/src/features/file-ops/useFocusTrap.ts`
- Modify: `apps/web/tests/overlayContract.test.mts`
- Modify: `apps/web/tests/titlebarOverlays.test.mts`
- Modify: `tools/checks/validate-ui-design.mjs`
- Modify: `tools/checks/validate-web-client.mjs`

**Regression risks:** Removing broad CSS selectors can alter unrelated inspector/settings/plugin controls; duplicate selectors can remain hidden later in the file; validators can become too broad and reject legitimate focus or selected states.

**Required checks:** Delete only in-scope legacy ownership; preserve Settings/Plugins rules; allow keyboard focus outlines and selected fills; reject only pointer-hover backgrounds/borders on migrated overlay rows.

- [ ] **Step 1: Extend contract tests to fail while legacy ownership remains**

Add assertions that read both CSS files:

```ts
for (const selector of [
  ".context-menu",
  ".dialog-backdrop",
  ".library-switcher-backdrop",
  ".sort-menu-list",
  ".filter-menu-popover",
  ".notification-popover",
  ".notification-window-body"
]) {
  const escapedSelector = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  assert.doesNotMatch(legacyStyles, new RegExp(`^${escapedSelector}(?:[\\s,{:.#]|$)`, "m"));
  assert.match(overlayCss, new RegExp(escapedSelector));
}

assert.doesNotMatch(legacyStyles, /rgba\(20,\s*20,\s*22,\s*0\.98\)/);
assert.doesNotMatch(legacyStyles, /rgba\(23,\s*27,\s*32,\s*0\.99\)/);
assert.doesNotMatch(legacyStyles, /#1c1c1e/i);
```

Scope the actual test helper to complete CSS rule selectors so comments and unrelated substrings do not create false failures.

- [ ] **Step 2: Run the contract test and verify the red state**

```powershell
node --experimental-strip-types --test apps/web/tests/overlayContract.test.mts
```

Expected: FAIL listing legacy selectors still owned by `styles.css`.

- [ ] **Step 3: Remove legacy blocks from styles.css in one pass**

Delete the old blocks for:

- `.search-bar*` presentation now owned by overlay.css;
- `.floating-popover`, `.popup-surface`, `.filter-menu*`, `.sort-menu*`, and `.layout-menu*` presentation;
- `.context-menu*` and `.dialog*` presentation;
- `.library-switcher*` and `.library-icon-picker--overlay*` presentation used by the main switcher; scope the retained embedded picker rules to `.settings-dialog-content .library-icon-picker` and its descendants so Settings output remains unchanged;
- `.notification-popover*` presentation;
- `.notification-window*`, `.notification-view*`, `.notification-status*`, `.notification-history*`, `.notification-log*`, and `.notification-filter*` presentation;
- later generic override selectors that reassign these migrated surfaces.

Do not remove shared root variables, secondary-window shell rules, Settings rules, Plugins rules, global pointer rules, global scrollbar rules, or reduced-transparency variables.

- [ ] **Step 4: Delete the retired anchored-popover helper**

Run:

```powershell
rg -n "anchoredPopover|useAnchoredPopoverStyle" apps/web/src apps/web/tests tools/checks
```

Expected before deletion: no production or test references. Then run:

```powershell
git rm apps/web/src/features/library/anchoredPopover.ts
```

Also verify the legacy focus hook has no callers, then delete it:

```powershell
rg -n "features/file-ops/useFocusTrap|from \"\./useFocusTrap\"" apps/web/src apps/web/tests tools/checks
git rm apps/web/src/features/file-ops/useFocusTrap.ts
```

Expected: the search prints no imports before `git rm`; shared modal code imports only `useOverlayFocusTrap`.

- [ ] **Step 5: Add validator coverage for the shared overlay contract**

Update `validate-ui-design.mjs` to read `apps/web/src/design/overlays/overlay.css` and all shared primitive sources. Add failures for:

- missing use of `--glass-dialog-fill`, `--glass-dialog-blur`, `--glass-center-fill`, `--secondary-window-pane-dark`, and radius variables;
- feature-local `z-index` in migrated TSX/CSS;
- `createPortal` in migrated feature components;
- per-element `:hover` background or border on overlay menu items, library rows, notification actions, and dialog buttons;
- missing `data-pointer-glow="off"` in SearchBar;
- compact notification rendering `model.results`, cancel, or retry;
- changes to Settings/Plugins structural tokens expected by the existing validator.

Update `validate-web-client.mjs` to require the shared positioning and stack modules and reject imports of the retired positioning helpers.

- [ ] **Step 6: Normalize overlay.css after legacy deletion**

Remove migration-only duplicate declarations from overlay.css. Keep sections ordered:

1. Core variables and surfaces.
2. Anchored positioning and modal shell.
3. Menu and field controls.
4. Titlebar overlays.
5. Library switcher.
6. Context menus and file dialogs.
7. Compact notification.
8. Full notification center.
9. Responsive, reduced-motion, reduced-transparency, and backdrop-filter fallback.

- [ ] **Step 7: Run style and validator checks**

```powershell
node --experimental-strip-types --test apps/web/tests/overlayContract.test.mts apps/web/tests/titlebarOverlays.test.mts apps/web/tests/librarySwitcherOverlay.test.mts apps/web/tests/fileOperationOverlays.test.mts apps/web/tests/notificationPopover.test.mts apps/web/tests/notificationWindow.test.mts
npm run check:web
npm run check:ui-design
git diff --check
```

Expected: all focused tests PASS, both validators exit `0`, and `git diff --check` prints no errors.

- [ ] **Step 8: Confirm Settings and Plugins source and visual ownership are untouched**

Run:

```powershell
git diff --name-only HEAD~6..HEAD | Select-String -Pattern 'features/(settings|plugins)/|SettingsWindow|PluginsWindow'
```

Expected: no Settings or Plugins component file is listed. A shared nonvisual import is acceptable only if it was explicitly required by Task 1; the plan does not require one.

- [ ] **Step 9: Commit Task 7**

```powershell
git add apps/web/src/styles.css apps/web/src/design/overlays/overlay.css apps/web/tests/overlayContract.test.mts apps/web/tests/titlebarOverlays.test.mts tools/checks/validate-ui-design.mjs tools/checks/validate-web-client.mjs
git add -u apps/web/src/features/library/anchoredPopover.ts apps/web/src/features/file-ops/useFocusTrap.ts
git commit -m "refactor(web): remove legacy overlay styles"
```

---

### Task 8: Full Verification, Visual/DPI/Appearance Matrix, and Documentation

**Files:**
- Create: `docs/benchmarks/2026-07-27-unified-overlay-verification.md`
- Modify: `docs/README.md`
- Modify only if fixture coverage requires it: `.tmp/visual-check/desktop-ui-regression.mjs`
- Modify only if exhaustive interaction coverage requires it: `.tmp/visual-check/desktop-ui-exhaustive-interaction-sweep.mjs`

**Regression risks:** Harness-only success can miss physical DWM composition; fixture screenshots can hide production positioning errors; DPI emulation is not equivalent to real Windows display scaling; visual harness edits can accidentally weaken assertions.

**Required checks:** Run production components and controlled fixtures; preserve every existing hard failure; record real compositor results separately from CDP layout evidence; do not claim AA-sensitive physical rendering from `Page.captureScreenshot`.

- [ ] **Step 1: Run the complete automated gate before visual work**

```powershell
git diff --check
npm run check:structure
npm run check:web
npm run check:desktop
npm run check:ui-design
npm test
```

Expected: every command exits `0`; full Rust suite reports zero failures; no unrelated dirty file is staged.

- [ ] **Step 2: Run controlled notification and overlay fixture checks**

Exercise fixture states for:

- filter selected and empty tags;
- sort and layout selected options;
- library normal, unavailable, empty search, nested icon, nested remove, confirmation;
- context menu normal, disabled, danger, and edge-clamped;
- rename validation error, move cross-root error, delete permanent confirmation;
- compact notification active, thumbnail, empty, degraded;
- full notification status, history, logs, failure drill-down, degraded, narrow.

Expected: no clipped surface, no unexpected divider, no competing hover rectangle, and no hidden primary action.

- [ ] **Step 3: Run the Windows compositor harness**

```powershell
$env:MANGOO_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs; $code=$LASTEXITCODE; Remove-Item Env:\MANGOO_VISUAL_OS_BACKDROP; exit $code
```

Expected: `required-os-composited: passed`, `hardFailures=0`, `startupProblems=0`.

- [ ] **Step 4: Run the exhaustive real interaction sweep**

```powershell
$env:MANGOO_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-exhaustive-interaction-sweep.mjs; $code=$LASTEXITCODE; Remove-Item Env:\MANGOO_VISUAL_OS_BACKDROP; exit $code
```

Expected: zero hard failures covering titlebar root exclusivity, nested surfaces, context-menu edge positions, dialog layering, notification center, preview return, layout switching, and subfolder interaction.

- [ ] **Step 5: Verify the appearance-parameter matrix**

For side, center, and dialog groups, test minimum, default, and maximum supported values through Settings. Record each result:

| Parameter group | Expected affected surfaces | Expected unaffected surfaces |
| --- | --- | --- |
| Side | Notification nav, dark library/navigation zones, graded pane derivation | Popup outer blur, modal backdrop |
| Center | Fields, selects, readable inner regions, selected fills, notification content gradient | Popup outer blur |
| Dialog | Popover, nested, context, modal outer material and modal backdrop | Media grid, preview canvas |

Expected: every surface responds only to its mapped group; Settings and Plugins remain their existing baseline.

- [ ] **Step 6: Verify real Windows scale factors**

At 100%, 125%, and 150% Windows display scaling, relaunch production Mangoo and repeat:

1. Open every titlebar root overlay.
2. Open library nested overlays and remove confirmation.
3. Open a context menu at each viewport corner.
4. Complete one keyboard-only dialog flow.
5. Open compact and full notification surfaces.
6. Resize the notification window to its narrow layout.

Expected at every scale: 12 logical-pixel viewport gutter, readable 32px controls, no clipped text/action, no one-pixel divider gap, correct focus outline, and no popup detached from its trigger.

- [ ] **Step 7: Verify reduced motion, reduced transparency, and fallback**

Use DevTools rendering emulation for reduced motion and reduced transparency, then disable backdrop-filter in the controlled browser fixture.

Expected: no translation animation under reduced motion; opaque readable overlay fallback under reduced transparency or missing backdrop-filter; all content remains immediately keyboard accessible.

- [ ] **Step 8: Record verification evidence**

Create `docs/benchmarks/2026-07-27-unified-overlay-verification.md` with:

- commit range tested;
- automated command results;
- OS compositor harness summary path;
- exhaustive sweep summary path;
- scale-factor matrix;
- appearance-parameter matrix;
- reduced-motion/transparency results;
- real-window screenshots by surface/state;
- any residual risk stated without marking the feature complete.

Add links to the approved spec, implementation plan, and verification report in `docs/README.md`.

- [ ] **Step 9: Re-run the full gate after documentation or harness changes**

```powershell
git diff --check
npm test
$env:MANGOO_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs; $code=$LASTEXITCODE; Remove-Item Env:\MANGOO_VISUAL_OS_BACKDROP; exit $code
```

Expected: full gate PASS and compositor harness reports zero hard failures and startup problems.

- [ ] **Step 10: Commit Task 8**

Stage only files changed for verified coverage:

```powershell
git add docs/benchmarks/2026-07-27-unified-overlay-verification.md docs/README.md
git add .tmp/visual-check/desktop-ui-regression.mjs .tmp/visual-check/desktop-ui-exhaustive-interaction-sweep.mjs
git commit -m "docs: record unified overlay verification"
```

If neither harness source required a change, omit both `.tmp/visual-check` paths from `git add`. Do not stage generated screenshots, logs, user-data directories, or unrelated existing modifications.

---

## Plan Self-Review

### Specification coverage

| Specification area | Implemented by |
| --- | --- |
| Scope and exclusions | Execution Rules, all tasks |
| Existing component inventory | Tasks 2-6 |
| Shared architecture and API | Task 1 |
| Surface hierarchy | Tasks 1, 3-6 |
| Appearance mapping | Tasks 1-7, Task 8 matrix |
| Pointer contract | Tasks 1-7 validators |
| Position, stack, focus, keyboard | Tasks 1-4 |
| Library migration | Task 3 |
| Context and dialogs | Task 4 |
| Search behavior | Task 2 |
| Compact notification active-only contract | Task 5 |
| Full notification IA and actions | Task 6 |
| Responsive, accessibility, and motion | Tasks 1-6, Task 8 |
| Migration and legacy removal | Task 7 |
| Performance constraints | Tasks 1, 5, 6, validators |
| Automated and real desktop acceptance | Task 8 |

### Marker scan

- No unresolved implementation marker is present.
- Every created function or type used by a later task is defined in the Locked Public API or the task that introduces it.
- Every implementation task (Tasks 1-7) includes a red test, minimal implementation direction, green verification, regression checks, and a focused commit; Task 8 is the final full/visual verification task.

### Type consistency

- `OverlayRootId`, `OverlayLayerKind`, `OverlaySurfaceRole`, `OverlayPlacement`, `OverlayAnchor`, `OverlayLayerRegistration`, and `OverlayStackApi` have one definition in `overlayTypes.ts`.
- `AnchoredOverlay` consistently receives `anchor`, `id`, `rootId`, `open`, `onDismiss`, `placement`, and optional `parentId`.
- `ModalSurface` consistently receives `id`, `rootId`, `open`, `onDismiss`, `ariaLabelledBy`, and optional busy/focus props.
- Feature components retain their existing domain DTOs and callbacks.
- Notification props and contract types are not renamed or duplicated.

### Final scope decision

The eight tasks form one coherent overlay-system migration. Tasks 1-7 each leave a testable repository state with a focused commit; Task 8 is the release-facing verification and evidence gate. No additional subsystem plan is required.
