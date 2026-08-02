# Preview Scrubber Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a BandiView-style floating control bar to the centre image preview — a glass track with a draggable thumb, a position tooltip, and a pin button — that scrubs to any position in the current folder.

**Architecture:** Three units following the split this codebase already uses for preview features. `previewScrubber.ts` is pure logic (pointer-x ↔ absolute index, and the visibility truth table), unit-tested with no DOM. `PreviewScrubber.tsx` renders the glass widget and owns only its own drag state. `CentralPreviewStage` mounts it and supplies pointer proximity from the `onPointerMove` it already has. Position comes from `resolvePreviewTitlePosition`, the same source that renders the titlebar's "13/31", so thumb and title cannot disagree. The pin is one new persisted `PreviewPreferences` field.

**Tech Stack:** React 18 + TypeScript + Vite; `node --test` for web units; existing `design/liquid-glass` primitives; `useLibraryData`'s `requestMediaWindow` for far jumps.

## Global Constraints

- **The thumb position and the displayed image never disagree.** A drag commits the index immediately — no placeholder, no deferred commit, no catch-up lag.
- **Dragging BYPASSES the first-paint navigation gate.** `canStartPreviewNavigation` exists to stop the keyboard and wheel outrunning the display; applying it to a drag would make the thumb stick. The drag calls `library.setSelectedMediaId` directly. Keys and wheel keep the gate unchanged.
- **The scrubber addresses ABSOLUTE folder position**, never the loaded window, via `resolvePreviewTitlePosition`'s `{current, total}` (`mediaSlots` + `mediaTotalCount`).
- **It must work unchanged when `showChildFolderContents` flattens a subtree.** No special case — `mediaTotalCount` already reports the flattened total.
- **It never hides mid-drag**, even if the pointer leaves the reveal band.
- **No division by zero.** A single-image folder (`total === 1`) and an empty one (`total === 0`) must both produce finite values, never `NaN`.
- **Pin default is `false`** and persists across restarts via `PreviewPreferences`.
- **Web tests are plain `node --test` with NO DOM** (`node --experimental-strip-types --test tests/*.test.mts`). Follow `apps/web/tests/previewRenderSettings.test.mts` for the `register("./resolve-source-imports.mjs", import.meta.url)` pattern.
- **No raw `fetch(` anywhere under `apps/web/src`** — `tools/checks/validate-web-client.mjs` fails the build on it.
- **Every scripted file write must pass `newline="\n"`.** Python's default text write converts the file to CRLF on Windows, which breaks `check:ui-design` (its regex matches `\n  \}\n\n  function`). This has cost a debugging cycle before.
- **Commit BEFORE mutation-testing a guard.** `git checkout` on a file carrying uncommitted work destroys it — this has happened twice on earlier plans.
- **`git add` explicit paths only — never `-A`, never `.`** The user has unrelated uncommitted work: `docs/benchmarks/2026-07-20-notification-center-verification.md`, `docs/superpowers/plans/2026-07-24-notification-failure-drilldown.md`, and six `tools/dev/*.mjs` probe scripts.
- **Full gate before done:** `npm test`. Rust needs `export PATH="$HOME/.cargo/bin:$PATH"`.

---

## File Structure

**Create:**
- `apps/web/src/features/preview/previewScrubber.ts` — pure mapping and visibility logic. No DOM, no React, no imports from the app.
- `apps/web/src/features/preview/PreviewScrubber.tsx` — the widget. Glass track, thumb, tooltip, pin button. Owns drag state only.
- `apps/web/tests/previewScrubber.test.mts` — unit tests for the pure module.

**Modify:**
- `apps/web/src/features/preview/previewPreferences.ts` — one field, `previewScrubberPinned`.
- `apps/web/src/features/preview/CentralPreviewStage.tsx` — mount the scrubber, supply pointer proximity, thread the position and callbacks.
- `apps/web/src/app/App.tsx` — pass position, pinned state, and the two commit callbacks down.
- `apps/web/src/styles.css` — layout, glass tuning, fade.
- `tools/checks/validate-web-client.mjs` — guard that the drag path bypasses the gate.

---

### Task 1: `previewScrubber` — the pure mapping and visibility rules

**Files:**
- Create: `apps/web/src/features/preview/previewScrubber.ts`
- Test: `apps/web/tests/previewScrubber.test.mts`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `scrubberIndexForPointerX(pointerX: number, track: { left: number; width: number }, total: number): number` — 0-based, clamped to `[0, total-1]`.
  - `scrubberFractionForIndex(index: number, total: number): number` — 0..1, for positioning the thumb.
  - `shouldShowScrubber(input: { previewOpen: boolean; pinned: boolean; pointerNearBottom: boolean; dragging: boolean }): boolean`
  - `SCRUBBER_REVEAL_BAND_PX: number`

  Tasks 2–4 consume all of it.

**Why a fraction rather than a pixel offset:** the thumb is positioned with a CSS percentage, so the component never needs to know the track's pixel width for rendering — only for hit-testing. That keeps the pure module free of layout assumptions and makes the round-trip test meaningful.

- [ ] **Step 1: Write the failing test**

Create `apps/web/tests/previewScrubber.test.mts`:

```typescript
import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);

const {
  SCRUBBER_REVEAL_BAND_PX,
  scrubberFractionForIndex,
  scrubberIndexForPointerX,
  shouldShowScrubber
} = await import("../src/features/preview/previewScrubber.ts");

const TRACK = { left: 100, width: 800 };

describe("scrubber pointer mapping", () => {
  it("maps the track ends to the first and last image", () => {
    assert.equal(scrubberIndexForPointerX(100, TRACK, 31), 0);
    assert.equal(scrubberIndexForPointerX(900, TRACK, 31), 30);
  });

  it("maps the midpoint near the middle of the folder", () => {
    const middle = scrubberIndexForPointerX(500, TRACK, 31);
    assert.ok(middle >= 14 && middle <= 16, `got ${middle}`);
  });

  it("clamps a pointer dragged outside the track", () => {
    assert.equal(scrubberIndexForPointerX(-9999, TRACK, 31), 0);
    assert.equal(scrubberIndexForPointerX(9999, TRACK, 31), 30);
  });

  it("survives a single-image folder without dividing by zero", () => {
    for (const x of [100, 500, 900]) {
      const index = scrubberIndexForPointerX(x, TRACK, 1);
      assert.equal(index, 0, `x=${x}`);
      assert.ok(Number.isFinite(index));
    }
    assert.equal(scrubberFractionForIndex(0, 1), 0);
    assert.ok(Number.isFinite(scrubberFractionForIndex(0, 1)));
  });

  it("survives an empty folder without producing NaN", () => {
    const index = scrubberIndexForPointerX(500, TRACK, 0);
    assert.equal(index, 0);
    assert.ok(Number.isFinite(index));
    assert.ok(Number.isFinite(scrubberFractionForIndex(0, 0)));
  });

  it("survives a zero-width track", () => {
    const index = scrubberIndexForPointerX(500, { left: 100, width: 0 }, 31);
    assert.ok(Number.isFinite(index), `got ${index}`);
    assert.ok(index >= 0 && index <= 30);
  });

  it("round-trips index -> fraction -> index across a 20000-image folder", () => {
    // This is the property that makes thumb and image agree: wherever the thumb
    // is drawn, hit-testing that same spot must return the index it represents.
    const total = 20000;
    for (const index of [0, 1, 4999, 10000, 19998, 19999]) {
      const fraction = scrubberFractionForIndex(index, total);
      const pointerX = TRACK.left + fraction * TRACK.width;
      assert.equal(
        scrubberIndexForPointerX(pointerX, TRACK, total),
        index,
        `index ${index} did not round-trip (fraction ${fraction})`
      );
    }
  });

  it("produces the coarse step a large folder implies", () => {
    // 20000 images over 800px is ~25 per pixel. This is inherent to the
    // mapping, not throttling — it is why a drag that looks continuous visits
    // hundreds of images rather than 20000.
    const total = 20000;
    const atX = (x: number) => scrubberIndexForPointerX(x, TRACK, total);
    assert.equal(atX(500), atX(500), "same pixel must be stable");
    const step = atX(501) - atX(500);
    assert.ok(step > 10, `expected a coarse step, got ${step}`);
  });

  it("keeps fractions inside 0..1", () => {
    for (const [index, total] of [[0, 31], [30, 31], [0, 1], [19999, 20000]] as const) {
      const fraction = scrubberFractionForIndex(index, total);
      assert.ok(fraction >= 0 && fraction <= 1, `${index}/${total} -> ${fraction}`);
    }
  });
});

describe("scrubber visibility", () => {
  const base = {
    previewOpen: true,
    pinned: false,
    pointerNearBottom: false,
    dragging: false
  };

  it("is hidden by default while the pointer is elsewhere", () => {
    assert.equal(shouldShowScrubber(base), false);
  });

  it("appears when the pointer nears the bottom", () => {
    assert.equal(shouldShowScrubber({ ...base, pointerNearBottom: true }), true);
  });

  it("stays visible when pinned regardless of the pointer", () => {
    assert.equal(shouldShowScrubber({ ...base, pinned: true }), true);
  });

  it("NEVER hides mid-drag, even when the pointer leaves the band", () => {
    // Releasing the thumb outside the bar must not make it vanish under the
    // cursor.
    assert.equal(
      shouldShowScrubber({ ...base, dragging: true, pointerNearBottom: false }),
      true
    );
  });

  it("is hidden whenever the preview is closed, whatever else is true", () => {
    for (const extra of [
      { pinned: true },
      { pointerNearBottom: true },
      { dragging: true },
      { pinned: true, dragging: true, pointerNearBottom: true }
    ]) {
      assert.equal(
        shouldShowScrubber({ ...base, ...extra, previewOpen: false }),
        false,
        JSON.stringify(extra)
      );
    }
  });

  it("exposes a reveal band tall enough to find without hunting", () => {
    assert.ok(SCRUBBER_REVEAL_BAND_PX >= 80, `got ${SCRUBBER_REVEAL_BAND_PX}`);
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd /d/Megle
npm --workspace @megle/web run test 2>&1 | grep -E "Cannot find module|^# fail" | head -3
```

Expected: FAIL — `Cannot find module '../src/features/preview/previewScrubber.ts'`.

- [ ] **Step 3: Write the implementation**

Create `apps/web/src/features/preview/previewScrubber.ts`:

```typescript
// Pure logic for the preview scrubber: where a pointer lands in the folder,
// where the thumb is drawn, and whether the bar is visible at all.
//
// No DOM, no React, no app imports — the whole contract is testable under
// node --test, which is where the interesting cases live (a 20000-image folder,
// a single-image folder, a zero-width track).

/**
 * How far above the bottom of the preview viewport the pointer counts as
 * "near the bottom". Generous on purpose: the bar auto-hides, so a narrow band
 * would mean hunting for it.
 */
export const SCRUBBER_REVEAL_BAND_PX = 120;

function clampIndex(index: number, total: number): number {
  if (!Number.isFinite(index) || total <= 0) {
    return 0;
  }
  return Math.min(total - 1, Math.max(0, Math.round(index)));
}

/**
 * The 0-based folder index a pointer position addresses.
 *
 * On a large folder one pixel of track spans many images (20000 images over an
 * 800px track is ~25 per pixel). That coarse step is inherent to the mapping
 * rather than a throttle, and it is why a drag that looks continuous visits
 * hundreds of images instead of twenty thousand.
 */
export function scrubberIndexForPointerX(
  pointerX: number,
  track: { left: number; width: number },
  total: number
): number {
  if (total <= 1 || !Number.isFinite(pointerX)) {
    return 0;
  }
  if (!Number.isFinite(track.width) || track.width <= 0) {
    // A zero-width track carries no position information; addressing the first
    // image is the only answer that cannot be wrong by an arbitrary amount.
    return 0;
  }
  const fraction = (pointerX - track.left) / track.width;
  return clampIndex(fraction * (total - 1), total);
}

/**
 * Where to draw the thumb for an index, as a 0..1 fraction of the track.
 *
 * A fraction rather than a pixel offset so the component positions the thumb
 * with a CSS percentage and this module stays free of layout assumptions. It is
 * the exact inverse of `scrubberIndexForPointerX`, which is what guarantees the
 * thumb and the displayed image agree.
 */
export function scrubberFractionForIndex(index: number, total: number): number {
  if (total <= 1 || !Number.isFinite(index)) {
    return 0;
  }
  const clamped = clampIndex(index, total);
  return clamped / (total - 1);
}

/**
 * Whether the bar is on screen.
 *
 * `dragging` wins over `pointerNearBottom` deliberately: releasing the thumb
 * outside the band must not make the bar vanish underneath the cursor.
 */
export function shouldShowScrubber(input: {
  previewOpen: boolean;
  pinned: boolean;
  pointerNearBottom: boolean;
  dragging: boolean;
}): boolean {
  if (!input.previewOpen) {
    return false;
  }
  return input.pinned || input.dragging || input.pointerNearBottom;
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd /d/Megle
npm --workspace @megle/web run test 2>&1 | grep -E "^# (tests|pass|fail)|not ok" | head -5
npm --workspace @megle/web run typecheck 2>&1 | tail -2
```

Expected: 15 new assertions pass, 0 failing; typecheck clean.

- [ ] **Step 5: Commit**

```bash
cd /d/Megle
git add apps/web/src/features/preview/previewScrubber.ts apps/web/tests/previewScrubber.test.mts
git commit -m "feat(web): add pure preview scrubber mapping and visibility logic"
```

---

### Task 2: Persist the pin

**Files:**
- Modify: `apps/web/src/features/preview/previewPreferences.ts`
- Test: `apps/web/tests/previewScrubber.test.mts` (extend)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `PreviewPreferences.previewScrubberPinned: boolean`, default `false`. Tasks 3 and 4 read and write it.

**Why this is small:** `normalizePreviewPreferences` is the single merge point for every field and `readStoredPreviewPreferences` routes through it, so a blob written by an older build simply lacks the key and falls back. That behaviour needs a test, not new machinery.

- [ ] **Step 1: Write the failing test**

Append to `apps/web/tests/previewScrubber.test.mts`:

```typescript
const { DEFAULT_PREVIEW_PREFERENCES, normalizePreviewPreferences } = await import(
  "../src/features/preview/previewPreferences.ts"
);

describe("scrubber pin persistence", () => {
  it("defaults to unpinned", () => {
    assert.equal(DEFAULT_PREVIEW_PREFERENCES.previewScrubberPinned, false);
  });

  it("falls back for a blob written before the key existed", () => {
    const normalized = normalizePreviewPreferences({ previewBufferLimitMb: 900 });
    assert.equal(normalized.previewScrubberPinned, false);
    assert.equal(normalized.previewBufferLimitMb, 900, "unrelated fields survive");
  });

  it("round-trips a pinned bar", () => {
    assert.equal(
      normalizePreviewPreferences({ previewScrubberPinned: true }).previewScrubberPinned,
      true
    );
  });

  it("coerces a non-boolean stored value rather than passing it through", () => {
    assert.equal(
      normalizePreviewPreferences({ previewScrubberPinned: "yes" as never })
        .previewScrubberPinned,
      false
    );
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd /d/Megle
npm --workspace @megle/web run test 2>&1 | grep -E "^# fail|not ok" | head -4
```

Expected: FAIL — `previewScrubberPinned` is `undefined`.

- [ ] **Step 3: Add the field**

In `apps/web/src/features/preview/previewPreferences.ts`, add to the `PreviewPreferences` interface, next to `previewChannel`:

```typescript
  // Whether the preview scrubber stays visible instead of auto-hiding.
  // BandiView calls the equivalent `lockControlBar`.
  previewScrubberPinned: boolean;
```

Add to `DEFAULT_PREVIEW_PREFERENCES`:

```typescript
  previewScrubberPinned: false,
```

Add to the object `normalizePreviewPreferences` returns:

```typescript
    // `=== true` rather than a truthiness check: a stored string like "yes"
    // must normalise to false, not sneak through as truthy.
    previewScrubberPinned: value.previewScrubberPinned === true,
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd /d/Megle
npm --workspace @megle/web run test 2>&1 | grep -E "^# (tests|pass|fail)" | head -4
npm --workspace @megle/web run typecheck 2>&1 | tail -2
npm run check:web 2>&1 | grep -E "^FAIL|^PASS: web" | head -2
```

Expected: 4 new assertions pass; typecheck clean; `check:web` PASS.

- [ ] **Step 5: Commit**

```bash
cd /d/Megle
git add apps/web/src/features/preview/previewPreferences.ts apps/web/tests/previewScrubber.test.mts
git commit -m "feat(web): persist the preview scrubber pin state"
```

---

### Task 3: The `PreviewScrubber` widget

**Files:**
- Create: `apps/web/src/features/preview/PreviewScrubber.tsx`
- Modify: `apps/web/src/styles.css`

**Interfaces:**
- Consumes: Task 1's `scrubberFractionForIndex`, `scrubberIndexForPointerX`, `shouldShowScrubber`.
- Produces:

```typescript
interface PreviewScrubberProps {
  /** 1-based position, as rendered in the titlebar. */
  current: number;
  /** Total images in the current folder (flattened if includeDescendants). */
  total: number;
  pinned: boolean;
  pointerNearBottom: boolean;
  previewOpen: boolean;
  /** Commits a 0-based index. Called on every distinct index during a drag. */
  onScrubTo: (index: number) => void;
  onTogglePinned: () => void;
}
```

Task 4 renders it.

**Pointer capture matters here.** The drag must keep tracking after the pointer leaves the thumb — `setPointerCapture` on pointerdown, released on pointerup — otherwise a fast drag drops the moment the cursor outruns the thumb. `CentralPreviewStage` already uses this pattern for pan.

- [ ] **Step 1: Write the component**

Create `apps/web/src/features/preview/PreviewScrubber.tsx`:

```tsx
import { Pin } from "lucide-react";
import { useCallback, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";

import { LiquidGlassButton, LiquidGlassSurface } from "../../design/liquid-glass";

import {
  scrubberFractionForIndex,
  scrubberIndexForPointerX,
  shouldShowScrubber
} from "./previewScrubber";

interface PreviewScrubberProps {
  current: number;
  total: number;
  pinned: boolean;
  pointerNearBottom: boolean;
  previewOpen: boolean;
  onScrubTo: (index: number) => void;
  onTogglePinned: () => void;
}

// A floating browsing control bar for the centre preview, modelled on
// BandiView's bottom control bar: a track, a draggable thumb, a position
// tooltip, and a pin that defeats auto-hide (BandiView calls it lockControlBar).
//
// The thumb position and the displayed image never disagree: a drag commits its
// index immediately rather than deferring to pointerup, and the thumb is drawn
// from the committed position rather than from local drag state.
export function PreviewScrubber({
  current,
  total,
  pinned,
  pointerNearBottom,
  previewOpen,
  onScrubTo,
  onTogglePinned
}: PreviewScrubberProps) {
  const trackRef = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  // The last index handed to onScrubTo, so pointer movement within one pixel of
  // track (which re-computes the same index) does no work.
  const lastCommittedRef = useRef<number | null>(null);

  const visible = shouldShowScrubber({ previewOpen, pinned, pointerNearBottom, dragging });

  const commitFromPointer = useCallback(
    (clientX: number) => {
      const track = trackRef.current;
      if (!track) {
        return;
      }
      const rect = track.getBoundingClientRect();
      const index = scrubberIndexForPointerX(
        clientX,
        { left: rect.left, width: rect.width },
        total
      );
      if (lastCommittedRef.current === index) {
        return;
      }
      lastCommittedRef.current = index;
      onScrubTo(index);
    },
    [onScrubTo, total]
  );

  const handlePointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (event.button !== 0 || total <= 0) {
        return;
      }
      event.preventDefault();
      // Capture so the drag survives the pointer outrunning the thumb.
      event.currentTarget.setPointerCapture(event.pointerId);
      setDragging(true);
      lastCommittedRef.current = null;
      commitFromPointer(event.clientX);
    },
    [commitFromPointer, total]
  );

  const handlePointerMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (!dragging) {
        return;
      }
      commitFromPointer(event.clientX);
    },
    [commitFromPointer, dragging]
  );

  const endDrag = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    setDragging(false);
  }, []);

  if (total <= 0) {
    return null;
  }

  // `current` is 1-based (it is the same value the titlebar renders).
  const fraction = scrubberFractionForIndex(Math.max(0, current - 1), total);
  const thumbPercent = `${fraction * 100}%`;

  return (
    <div
      className="preview-scrubber"
      data-preview-scrubber-visible={visible ? "true" : "false"}
      data-no-drag="true"
    >
      <LiquidGlassSurface className="preview-scrubber-shell" tone="control">
        <div
          aria-label="Seek through folder"
          aria-valuemax={total}
          aria-valuemin={1}
          aria-valuenow={current}
          aria-valuetext={`${current} / ${total}`}
          className="preview-scrubber-track"
          onPointerCancel={endDrag}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={endDrag}
          ref={trackRef}
          role="slider"
          tabIndex={-1}
        >
          <div
            aria-hidden="true"
            className="preview-scrubber-thumb"
            style={{ left: thumbPercent }}
          />
          <div
            aria-hidden="true"
            className="preview-scrubber-tooltip"
            style={{ left: thumbPercent }}
          >
            {current}/{total}
          </div>
        </div>
        <LiquidGlassButton
          active={pinned}
          aria-label={pinned ? "Unpin control bar" : "Pin control bar"}
          aria-pressed={pinned}
          className="preview-scrubber-pin"
          onClick={onTogglePinned}
          title={pinned ? "Unpin control bar" : "Pin control bar"}
          tone="control"
          type="button"
        >
          <Pin aria-hidden="true" size={14} />
        </LiquidGlassButton>
      </LiquidGlassSurface>
    </div>
  );
}
```

- [ ] **Step 2: Add the styles**

Append to `apps/web/src/styles.css`:

```css
/* Floating browsing control bar, bottom-centre of the preview stage. */
.preview-scrubber {
  position: absolute;
  bottom: 18px;
  left: 50%;
  transform: translateX(-50%);
  z-index: 4;
  opacity: 0;
  /* `visibility` rather than `display` so the fade can play both ways, and
     `pointer-events: none` while hidden so it cannot swallow clicks on the
     image underneath. */
  visibility: hidden;
  pointer-events: none;
  transition: opacity 140ms ease, visibility 0s linear 140ms;
}

.preview-scrubber[data-preview-scrubber-visible="true"] {
  opacity: 1;
  visibility: visible;
  pointer-events: auto;
  transition: opacity 140ms ease, visibility 0s linear 0s;
}

.preview-scrubber-shell {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border-radius: 999px;
}

.preview-scrubber-track {
  position: relative;
  width: min(46vw, 620px);
  height: 22px;
  border-radius: 999px;
  background: rgba(0, 0, 0, 0.42);
  cursor: pointer;
  touch-action: none;
}

.preview-scrubber-thumb {
  position: absolute;
  top: 50%;
  width: 16px;
  height: 16px;
  margin-left: -8px;
  border-radius: 50%;
  background: #fff;
  transform: translateY(-50%);
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.45);
  pointer-events: none;
}

.preview-scrubber-tooltip {
  position: absolute;
  bottom: calc(100% + 10px);
  transform: translateX(-50%);
  padding: 2px 8px;
  border-radius: 6px;
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  background: rgba(20, 20, 20, 0.82);
  color: #fff;
  pointer-events: none;
}

.preview-scrubber-pin {
  flex: 0 0 auto;
}
```

- [ ] **Step 3: Verify it compiles**

```bash
cd /d/Megle
npm --workspace @megle/web run typecheck 2>&1 | tail -3
npm run check:ui-design 2>&1 | tail -2
```

Expected: typecheck clean, `PASS: UI liquid glass design boundaries`. (It is not mounted yet; Task 4 does that.)

`LiquidGlassSurface` accepts `tone="control"` and `className` — verified against `apps/web/src/design/liquid-glass/LiquidGlassSurface.tsx:21-33`, where `LiquidGlassSurfaceProps extends HTMLAttributes<HTMLElement>` and `tone` is `"chrome" | "panel" | "elevated" | "control" | "primary" | "danger"`. `LiquidGlassButton` takes the same `tone` plus `active`.

- [ ] **Step 4: Commit**

```bash
cd /d/Megle
git add apps/web/src/features/preview/PreviewScrubber.tsx apps/web/src/styles.css
git commit -m "feat(web): add the preview scrubber control bar widget"
```

---

### Task 4: Mount it and wire the drag

**Files:**
- Modify: `apps/web/src/features/preview/CentralPreviewStage.tsx`
- Modify: `apps/web/src/app/App.tsx`
- Modify: `tools/checks/validate-web-client.mjs`

**Interfaces:**
- Consumes: Task 1's `SCRUBBER_REVEAL_BAND_PX`, Task 2's preference field, Task 3's `PreviewScrubber`.
- Produces: the working feature.

**The critical rule:** the scrub callback calls `library.setSelectedMediaId` **directly**, NOT `startPreviewNavigation`. Routing it through the gate would make the thumb stick while an image loaded, contradicting the spec's central requirement.

- [ ] **Step 1: Add the scrub handler in App.tsx**

`App.tsx` already computes `previewTitlePosition` via `resolvePreviewTitlePosition` and already holds `library`. Add beside `handlePreviewNext`:

```tsx
  const handleScrubToIndex = useCallback(
    (index: number) => {
      // Deliberately NOT startPreviewNavigation: the first-paint gate exists to
      // stop the keyboard and wheel outrunning the display, but a drag must
      // keep the thumb and the image in agreement, so it commits immediately.
      const slotMedia = library.mediaSlots.get(index);
      if (slotMedia) {
        library.setSelectedMediaId(slotMedia.id);
        return;
      }
      // Outside the loaded window: ask for that region. The thumb and tooltip
      // have already moved; the image follows when the slot resolves. This is
      // the one place position and picture can briefly disagree, and it is
      // bounded by a single round-trip.
      library.requestMediaWindow(index, index + 1);
    },
    [library]
  );

  const handleToggleScrubberPinned = useCallback(() => {
    setPreviewPreferences((current) => ({
      ...current,
      previewScrubberPinned: !current.previewScrubberPinned
    }));
  }, []);
```

- [ ] **Step 2: Thread them to CentralPreviewStage**

`CentralPreviewStage` is rendered from `LibraryView`, which already forwards a set of preview props. Add four to that chain — `scrubberCurrent`, `scrubberTotal`, `scrubberPinned`, `onScrubTo`, `onToggleScrubberPinned` — sourcing the first two from the `previewTitlePosition` App already computes:

```tsx
          scrubberCurrent={previewTitlePosition?.current ?? 0}
          scrubberTotal={previewTitlePosition?.total ?? 0}
          scrubberPinned={previewPreferences.previewScrubberPinned}
          onScrubTo={handleScrubToIndex}
          onToggleScrubberPinned={handleToggleScrubberPinned}
```

Read `LibraryView.tsx`'s existing preview prop list and follow the same shape; do not restructure it.

- [ ] **Step 3: Track pointer proximity and mount the scrubber**

In `CentralPreviewStage.tsx`, add state and feed it from the stage's existing `onPointerMove`:

```tsx
  const [pointerNearBottom, setPointerNearBottom] = useState(false);
```

Inside the existing `handlePointerMove`, before its current body:

```tsx
    const stage = stageRef.current;
    if (stage) {
      const rect = stage.getBoundingClientRect();
      setPointerNearBottom(event.clientY >= rect.bottom - SCRUBBER_REVEAL_BAND_PX);
    }
```

Add a leave handler on `.central-preview-viewport` so the bar hides when the pointer exits the stage entirely:

```tsx
          onPointerLeave={() => setPointerNearBottom(false)}
```

Render the scrubber as the last child of `.central-preview-stage`, after the loading indicator and the video controls:

```tsx
        <PreviewScrubber
          current={scrubberCurrent}
          onScrubTo={onScrubTo}
          onTogglePinned={onToggleScrubberPinned}
          pinned={scrubberPinned}
          pointerNearBottom={pointerNearBottom}
          previewOpen
          total={scrubberTotal}
        />
```

`previewOpen` is literally `true` here: `CentralPreviewStage` only renders while the preview is open. Passing the prop keeps `shouldShowScrubber`'s contract honest and testable rather than assuming it.

Import `SCRUBBER_REVEAL_BAND_PX` from `./previewScrubber` and `PreviewScrubber` from `./PreviewScrubber`.

- [ ] **Step 4: Guard the bypass**

The bypass is a deliberate, non-obvious decision that a future reader could "fix" by routing the drag through the gate. Add to `tools/checks/validate-web-client.mjs`, near the existing navigation-gate guards:

```javascript
// The scrubber drag MUST bypass the first-paint gate: routing it through
// startPreviewNavigation would make the thumb stick while an image loaded,
// breaking the rule that thumb position and displayed image always agree.
const scrubHandler = /const handleScrubToIndex = useCallback\(\s*\(index: number\) => \{([\s\S]*?)\n  \}/.exec(app);
if (!scrubHandler) {
  fail("App must define handleScrubToIndex for the preview scrubber");
} else if (/startPreviewNavigation/.test(scrubHandler[1])) {
  fail("The preview scrubber drag must not route through the first-paint navigation gate");
}
if (!/requestMediaWindow/.test(app)) {
  fail("App scrubber must request the media window for an index outside the loaded range");
}
```

- [ ] **Step 5: Verify**

```bash
cd /d/Megle
python -c "
import subprocess, os
for f in subprocess.run(['git','diff','--name-only','HEAD'],capture_output=True,text=True).stdout.split():
    if os.path.isfile(f):
        d=open(f,'rb').read()
        if b'\r\n' in d:
            open(f,'wb').write(d.replace(b'\r\n',b'\n')); print('normalised',f)
"
npm --workspace @megle/web run typecheck 2>&1 | tail -2
npm --workspace @megle/web run test 2>&1 | grep -E "^# (tests|pass|fail)" | head -3
npm run check:web 2>&1 | grep -E "^FAIL|^PASS: web" | head -3
npm run check:ui-design 2>&1 | tail -2
```

Expected: all clean.

- [ ] **Step 6: Commit, THEN mutation-test the guard**

Commit first — `git checkout` on uncommitted work destroys it.

```bash
cd /d/Megle
git add apps/web/src/features/preview/CentralPreviewStage.tsx apps/web/src/features/library/LibraryView.tsx apps/web/src/app/App.tsx tools/checks/validate-web-client.mjs
git commit -m "feat(web): mount the preview scrubber and wire scrubbing"
```

Now prove the guard fires:

```bash
cd /d/Megle
python -c "
import io
p='apps/web/src/app/App.tsx'
s=io.open(p,encoding='utf-8').read()
s=s.replace('      const slotMedia = library.mediaSlots.get(index);','      startPreviewNavigation(undefined, \"next\");',1)
io.open(p,'w',encoding='utf-8',newline='\n').write(s)
"
npm run check:web 2>&1 | grep -E "^FAIL" | head -2
git checkout -- apps/web/src/app/App.tsx
npm run check:web 2>&1 | grep -E "^PASS: web" | head -1
```

Expected: the mutated run FAILS with the bypass message; the restored run passes.

---

### Task 5: Full gate and real-app verification

**Files:**
- Create: `docs/benchmarks/2026-08-02-preview-scrubber-verification.md`

**This task is not optional and its findings may not be guessed.** The whole feature is interaction behaviour that no unit test can confirm.

- [ ] **Step 1: Full gate**

```bash
cd /d/Megle
export PATH="$HOME/.cargo/bin:$PATH"
npm test 2>&1 | grep -E "^(PASS|FAIL)|test result:" | head -16
```

Expected: exit 0, every stage PASS.

- [ ] **Step 2: Launch**

```bash
cd /d/Megle
export MEGLE_AUTO_ADD_ROOT="C:/Users/84460/Pictures/normal"
export MEGLE_REMOTE_DEBUG=1
nohup npm run dev > /tmp/dev_scrub.log 2>&1 &
sleep 85
```

- [ ] **Step 3: Verify the behaviour**

Record each of these, including anything that does not work:

- **Auto-hide:** with the preview open and the pointer in the middle of the image, the bar is absent. Move toward the bottom — it fades in.
- **Pin:** click the pin; move the pointer away; the bar stays. Restart the app and confirm it is still pinned. Unpin and confirm auto-hide returns.
- **Never hides mid-drag:** start a drag, move the pointer up out of the reveal band while still holding, and confirm the bar stays visible until release.
- **Position agreement:** drag slowly and confirm the tooltip number, the thumb, and the displayed image all correspond, and that the titlebar's own position indicator matches the tooltip.
- **Coarse step on a large folder:** open a folder with thousands of images (or switch on 显示子文件夹内容 to flatten a subtree) and confirm a small drag jumps many images, as it does in BandiView.
- **Flattened mode:** with `showChildFolderContents` on, confirm the total reflects the whole subtree and scrubbing crosses folder boundaries.
- **Single-image folder:** open one and confirm the bar renders without error and the thumb does not jump.
- **Keys and wheel still gated:** hold the arrow key on a cold folder and confirm images are still not skipped — Task 4 must not have disturbed the first-paint gate.

- [ ] **Step 4: Record and clean up**

Write `docs/benchmarks/2026-08-02-preview-scrubber-verification.md` with what was observed for each bullet, then:

```bash
cd /d/Megle
taskkill //F //IM megle-core.exe 2>/dev/null | head -1
taskkill //F //IM electron.exe 2>/dev/null | head -1
git status --short
```

`git status` must show only the user's own eight pre-existing files.

```bash
cd /d/Megle
git add docs/benchmarks/2026-08-02-preview-scrubber-verification.md
git commit -m "docs: record preview scrubber verification"
```

---

## Plan-level self-review notes

- **Spec coverage.** §3 drag model → Tasks 1 and 4 (immediate commit, distinct-index-only, gate bypass). §4 position model → Task 4's use of `previewTitlePosition` and `requestMediaWindow`, including the flattened case. §5 visibility and pin → Tasks 1 (`shouldShowScrubber`), 2 (persistence), 3 (fade), 4 (proximity). §6 structure → the file layout. §7 testing → Task 1's cases plus Task 5.
- **Every "must not divide by zero" case has a test**: `total === 1`, `total === 0`, and a zero-width track, all asserting `Number.isFinite`.
- **The round-trip test is the one that matters.** `index → fraction → pointerX → index` across a 20 000-image folder is what actually proves the thumb and the image agree; the endpoint tests alone would pass with an off-by-one mapping.
- **The gate bypass is guarded**, because it is exactly the kind of deliberate oddity a later reader would "correct". The guard is mutation-tested in Task 4 Step 6.
- **Type consistency:** `scrubberIndexForPointerX` and `scrubberFractionForIndex` are inverses over the same `total`; `current` is 1-based everywhere (matching the titlebar) and converted once, in `PreviewScrubber`, before calling into the 0-based pure functions.
- **API assumptions verified, not assumed:** `LiquidGlassSurface` accepts `tone="control"` + `className` (`LiquidGlassSurface.tsx:21-33`); `requestMediaWindow(startIndex, endIndex)` is on `LibraryState` (`useLibraryData.ts:198`) and already clamps, de-duplicates against loaded ranges, and honours `includeDescendants`; `LibraryView` already forwards a preview prop set to `CentralPreviewStage` (`LibraryView.tsx:460-469`) that Task 4 extends rather than restructures.
