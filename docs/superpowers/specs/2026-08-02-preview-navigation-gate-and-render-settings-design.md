# Design: Preview navigation gate & render settings

Date: 2026-08-02
Status: Design (brainstorming) — pending implementation plan

## 1. Problem

Two independent complaints about the preview surface, both landing on the same
component tree.

**Issue A — navigation outruns the display.** Scrolling or holding an arrow key
advances past images that never appeared. Confirmed by reading the code, not
inferred: `startPreviewNavigation` (`apps/web/src/app/App.tsx`) has **no gate at
all** — it calls `library.setSelectedMediaId(target.id)` unconditionally. The
previous gate was removed on purpose in the decode-pipeline work because it
waited for `onPreviewMediaSettled` (a *fully committed* frame), which is pure
input lag once neighbours are pre-decoded. Removing it entirely went too far.

The wheel path makes it worse. `onPreviewWheel`
(`apps/web/src/features/preview/CentralPreviewStage.tsx`) calls
`onPreviewNext()` / `onPreviewPrevious()` once per `wheel` event, and a single
trackpad flick emits many events — so one gesture can skip several images. Both
the wheel and the keyboard funnel through `startPreviewNavigation`, so one gate
fixes both.

**Issue B — no control over preview rendering.** There is no way to change how
an image is filtered when zoomed, and no way to inspect individual colour
channels or see through alpha compositing.

## 2. Scope

- A first-paint navigation gate (Issue A).
- A titlebar button opening a render-settings popover with three controls:
  scaling filter, channel isolation, alpha handling (Issue B).
- Settings are global and survive restart.

Explicitly out of scope: per-image or per-folder settings, colour management /
ICC profiles, histogram or pixel-probe tooling, and any change to zoom, pan, or
the decode pipeline.

## 3. Issue A — the navigation gate

### 3a. Rule

A prev/next step is refused while the currently-selected media has not yet had
its **first paint**. Not first *commit*, not fully loaded — the moment pixels
appear.

This is deliberately weaker than the lock it replaces. `PreviewCanvas` calls
`onMediaReady` on **every** paint, including the synchronous one that happens in
the same tick when a neighbour is already pooled. So whenever prefetch is
keeping up the gate is invisible; it only engages on a cold pool, which is
exactly the case where images are currently being skipped.

### 3b. Input during the gate is DROPPED, not queued

Holding an arrow key advances exactly as fast as images can begin painting, and
a wheel flick advances one image per gesture. Queueing would let a flick bank
several steps and make input feel detached from the finger.

### 3c. Shape

The decision is a pure function beside `canNavigatePreview` in
`apps/web/src/app/previewNavigationGate.ts`, so it is unit-testable with no DOM:

```ts
export function canStartPreviewNavigation(input: {
  previewOpen: boolean;
  selectedMediaIndex: number;
  itemCount: number;
  direction: "previous" | "next";
  selectedMediaId: number | null;
  paintedMediaId: number | null;
}): boolean
```

It returns `false` when `selectedMediaId !== paintedMediaId` (the "waiting for
the next frame" state), and otherwise defers to the existing position logic.

`App.tsx` tracks `paintedMediaId` in a ref fed by the existing
`handlePreviewMediaSettled` callback — no new plumbing, and the video and
animated-image lanes report through the same callback, so they gate identically.

**Opening and closing the preview are NOT gated** — only stepping between
images. `handleOpenPreview` must stay immediate.

### 3d. Failure mode to avoid

If a media never paints (decode failure), the gate must not wedge navigation
permanently. `PreviewCanvas`'s error path already calls `onMediaReady`, so a
failed image releases the gate the same way a successful one does. This must be
covered by a test, because it is the difference between "a slow image delays
you" and "a broken image traps you".

## 4. Issue B — render settings

### 4a. Controls

| Group | Options | Default |
|---|---|---|
| 缩放过滤 (scaling filter) | Smooth · High quality · None | Smooth |
| 通道 (channel) | RGBA · R · G · B · Alpha | RGBA |
| Alpha | Composite · Ignore | Composite |

"None" is nearest-neighbour — hard pixel edges when zoomed in.

**Verify before building the picker: does this Chromium actually distinguish
three values?** Chromium's `image-rendering` support has historically been
`auto | crisp-edges | pixelated`, with `smooth` and `high-quality` parsed but
collapsing to `auto`. If `CSS.supports("image-rendering", "high-quality")` is
false — or true but visually identical to `auto` — then "High quality" is a dead
option that reads as a feature and does nothing, which is worse than not
offering it.

Check this in the running app first (`CSS.supports` over CDP takes seconds). If
the three values do not genuinely differ, ship **Smooth · None** only. The user's
stated requirement is that the set "must include 无 (none)"; a third option that
does nothing was never the requirement.

### 4b. Button and popover

A new `PreviewRenderMenu` in `apps/web/src/features/preview/`, structurally a
copy of `apps/web/src/features/library/SortMenu.tsx`:

- `LiquidGlassButton` trigger, `tone="control"`,
  `data-titlebar-control="preview-render"`, `SlidersHorizontal` icon at size 15
  to match the adjacent preview-mode button.
- `AnchoredOverlay` with `className="overlay-titlebar-popover"`,
  `placement="bottom-end"`, and a new `"preview-render"` member of
  `OverlayRootId` (`apps/web/src/design/overlays/overlayTypes.ts`).
- Three `OverlayMenuList` groups, each a single-select `listbox` of
  `OverlayMenuItem`s with a checked marker — same primitives Sort uses.

It renders in `PreviewTitlebarToolbar`
(`apps/web/src/app-shell/ShellTopBar.tsx`) on the **right**, in a new
`titlebar-preview-actions` slot placed before the existing conditional
`shellActions`, leaving the left-hand control cluster and the centred title
untouched.

**The button is always visible while the preview is open**, including for video.
Open state lives in `App.tsx` beside `sortMenuOpen`, so the overlay stack
provides Esc, click-away, and focus restore.

### 4c. Persistence

Three fields join `PreviewPreferences`
(`apps/web/src/features/preview/previewPreferences.ts`):

```ts
previewRenderFilter: "smooth" | "highQuality" | "none";   // default "smooth"
previewChannel: "rgba" | "r" | "g" | "b" | "alpha";       // default "rgba"
previewAlphaMode: "composite" | "ignore";                 // default "composite"
```

That inherits localStorage persistence, cross-window `broadcastPreference`
sync, and the existing `check:web` preference guards. No new storage mechanism.

`readStoredPreviewPreferences` must tolerate a stored blob written by an older
build that lacks these keys, falling back to the defaults per field — the
existing reader already merges over `DEFAULT_PREVIEW_PREFERENCES`, so this needs
a test rather than new code.

### 4d. How the settings reach pixels

Both are **CSS-level**, applied to the shared `.preview-image` class that the
canvas, `<img>`, and `<video>` lanes all already carry. Two data attributes on
the preview stage select the rule:

- **Filter** → `image-rendering: auto | high-quality | pixelated`.
- **Channel + Alpha** → `filter: url(#megle-channel-…)` referencing an inline
  SVG `feColorMatrix`. The matrices are static, defined once in a hidden `<svg>`
  in the app shell; switching swaps which `#id` is referenced.

Nothing repaints, nothing re-decodes, and the effect is GPU-composited — so it
stays instant on a 10240px image and cannot regress the decode-pipeline work.

**Stated limitation, to be reflected in the UI wording.** A 2D canvas stores
pixels premultiplied, and a linear `feColorMatrix` cannot divide by alpha. So
the Alpha control is honestly named **Ignore** (force `A=1`, defeating
compositing) rather than claiming true un-premultiplication. For fully opaque
images — the overwhelming majority — the result is exact. Semi-transparent
pixels will show their premultiplied RGB. Exactness there would require a WebGL
paint path, which was considered and rejected: it would replace the renderer
this branch just built and re-reviewed, for correctness visible only on
semi-transparent pixels.

Separately, the decode Worker will pass `premultiplyAlpha: "none"` to
`createImageBitmap` so the pooled bitmap holds straight values for any future
exact renderer. Because this changes decoded pixel data, the bitmap pool must be
cleared if it ever becomes user-toggleable; as a fixed decode option it is set
once and needs no invalidation.

### 4e. Pure core

`apps/web/src/features/preview/previewRenderSettings.ts` holds the pure mapping,
unit-tested under `node --test` the way `previewRenderLane.ts` is:

```ts
export function imageRenderingForFilter(filter: PreviewRenderFilter): string;
export function channelMatrixId(
  channel: PreviewChannel,
  alphaMode: PreviewAlphaMode
): string | null;   // null = no filter, the default RGBA+composite case
```

Returning `null` for the default matters: the common case must apply **no** CSS
filter at all, so users who never open this panel pay nothing.

## 5. Testing

- **Pure, `node --test`:** the navigation gate across every combination
  (painted === selected, painted !== selected, both ends of the list, closed
  preview, empty list, and the failed-image release); `imageRenderingForFilter`
  for all three modes; `channelMatrixId` for all ten channel/alpha combinations
  including the `null` default.
- **Persistence:** defaults applied when the stored blob predates these keys;
  round-trip through `readStoredPreviewPreferences` / `storePreviewPreferences`.
- **Structural:** `check:web` gains a guard that the gate is wired into
  `startPreviewNavigation`, mirroring the existing absence-guard that stops the
  old readiness lock returning — the two must not contradict each other, so the
  absence guard's message needs updating to say "first-paint gate, not a
  settled-frame lock".
- **Real app:** confirm a wheel flick advances one image; confirm holding an
  arrow key on a cold folder no longer skips; confirm each filter and channel
  visibly changes the image and survives a restart; confirm the button is
  present and functional on a video.

## 6. Affected files

- Modify: `apps/web/src/app/previewNavigationGate.ts` (add
  `canStartPreviewNavigation`), `apps/web/src/app/App.tsx` (painted-id ref, gate
  call, menu open state, new preference wiring).
- Create: `apps/web/src/features/preview/previewRenderSettings.ts`,
  `apps/web/src/features/preview/PreviewRenderMenu.tsx`.
- Modify: `apps/web/src/features/preview/previewPreferences.ts` (three fields),
  `apps/web/src/app-shell/ShellTopBar.tsx` (right-hand slot),
  `apps/web/src/design/overlays/overlayTypes.ts` (`"preview-render"`),
  `apps/web/src/features/preview/previewDecodeWorker.ts`
  (`premultiplyAlpha: "none"`), `apps/web/src/styles.css` (rules + the inline
  SVG matrices), `tools/checks/validate-web-client.mjs` (guards).
- Tests: `apps/web/tests/previewNavigationGate.test.mts` (extend),
  `apps/web/tests/previewRenderSettings.test.mts` (new).
