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
- A titlebar button opening a render-settings popover with two controls:
  scaling filter and channel isolation (Issue B).
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

Two groups. There is **no premultiply toggle** — see 4d for why it became
unnecessary rather than merely dropped.

| Group | Options | Default |
|---|---|---|
| 缩放过滤 (scaling filter) | 最近邻 Nearest neighbour · 双线性 Bilinear | Bilinear |
| 通道 (channel) | RGBA · RGB · R · G · B · A | RGBA |

**Filter options are named after the actual resampling algorithm**, not by
vague quality words. That constrains the list to algorithms we genuinely
control:

- `image-rendering: pixelated` → **nearest neighbour**, exactly and verifiably.
- `image-rendering: auto` → Chromium's default filtered resampling, which is
  **bilinear**.

A third entry is only justified if it names a real, different algorithm.
Chromium has historically parsed `image-rendering: high-quality` but collapsed
it to `auto`, which would make a "Bicubic"/"High quality" row a label for
behaviour that does not happen. **Verify in the running app before building the
picker**: check `CSS.supports("image-rendering", "high-quality")` AND confirm it
renders differently from `auto` on an upscaled image. Ship a third row only if
both hold, and name it after whatever it actually is. Otherwise ship two.

Delivering genuinely selectable Lanczos/bicubic would mean resampling ourselves
in a shader or on the CPU. That is out of scope here — it is a different feature
from "let me pick the filter the renderer already offers".

### 4a-bis. Channel semantics

Channel views show the **raw stored channel value, never multiplied by alpha**.
This is the point of the feature: in a TGA (and in normal maps, packed masks,
and game textures generally) the A channel is often data, not transparency, and
the RGB channels carry their own meaning independent of it.

| Option | Shows |
|---|---|
| RGBA | Normal display: colour with alpha composited, as today |
| RGB | Colour with alpha forced opaque — the stored RGB, undarkened |
| R / G / B | That one channel as greyscale, opaque |
| A | The alpha channel as greyscale, opaque — readable as data |

RGBA is the default and applies no filter at all, so the common path is
untouched.

### 4b. Button and popover

A new `PreviewRenderMenu` in `apps/web/src/features/preview/`, structurally a
copy of `apps/web/src/features/library/SortMenu.tsx`:

- `LiquidGlassButton` trigger, `tone="control"`,
  `data-titlebar-control="preview-render"`, `SlidersHorizontal` icon at size 15
  to match the adjacent preview-mode button.
- `AnchoredOverlay` with `className="overlay-titlebar-popover"`,
  `placement="bottom-end"`, and a new `"preview-render"` member of
  `OverlayRootId` (`apps/web/src/design/overlays/overlayTypes.ts`).
- Two `OverlayMenuList` groups, each a single-select `listbox` of
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

Two fields join `PreviewPreferences`
(`apps/web/src/features/preview/previewPreferences.ts`):

```ts
previewRenderFilter: "bilinear" | "nearest";                    // default "bilinear"
previewChannel: "rgba" | "rgb" | "r" | "g" | "b" | "a";         // default "rgba"
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

- **Filter** → `image-rendering: auto | pixelated`.
- **Channel** → `filter: url(#megle-channel-…)` referencing an inline SVG
  `feColorMatrix`. The matrices are static, defined once in a hidden `<svg>` in
  the app shell; switching swaps which `#id` is referenced.

Nothing repaints, nothing re-decodes, and the effect is GPU-composited — so it
stays instant on a 10240px image and cannot regress the decode-pipeline work.

**Why there is no premultiply control.** An earlier draft of this design assumed
a linear colour matrix could not undo premultiplication, and proposed an
"Ignore alpha" switch as a partial substitute. That was wrong: SVG defines
`feColorMatrix` to operate on **non-premultiplied** colour values — the filter
un-premultiplies its input, applies the matrix, and re-premultiplies the result.
So a matrix that sets `A = 1` and passes RGB through yields the true stored RGB,
undarkened, which is exactly the requirement. No user-facing premultiply switch
is needed because the correct behaviour is unconditional.

**Verify this before building the matrices**, because the whole channel feature
rests on it: render a PNG with a 50%-alpha region, apply the RGB matrix, and
confirm the sampled colour matches the file's stored RGB rather than a halved
value. If Chromium deviates from the spec here, fall back to a WebGL paint path
for channel isolation only — and raise it before doing so, since that is a
materially larger change than this design assumes.

The decode Worker keeps its current `createImageBitmap` call unchanged. Passing
`premultiplyAlpha: "none"` was considered and rejected: a 2D canvas re-multiplies
on `drawImage` regardless, so the option would change nothing while adding a
pool-invalidation concern.

### 4e. Pure core

`apps/web/src/features/preview/previewRenderSettings.ts` holds the pure mapping,
unit-tested under `node --test` the way `previewRenderLane.ts` is:

```ts
export function imageRenderingForFilter(filter: PreviewRenderFilter): string;
export function channelMatrixId(channel: PreviewChannel): string | null;
//                                          null = no filter, the default RGBA case
```

Returning `null` for the default matters: the common case must apply **no** CSS
filter at all, so users who never open this panel pay nothing.

## 5. Testing

- **Pure, `node --test`:** the navigation gate across every combination
  (painted === selected, painted !== selected, both ends of the list, closed
  preview, empty list, and the failed-image release); `imageRenderingForFilter`
  for both modes; `channelMatrixId` for all six channel values
  including the `null` default, and a test asserting the RGB matrix forces A=1.
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
- Modify: `apps/web/src/features/preview/previewPreferences.ts` (two fields),
  `apps/web/src/app-shell/ShellTopBar.tsx` (right-hand slot),
  `apps/web/src/design/overlays/overlayTypes.ts` (`"preview-render"`),
  `apps/web/src/styles.css` (rules + the inline
  SVG matrices), `tools/checks/validate-web-client.mjs` (guards).
- Tests: `apps/web/tests/previewNavigationGate.test.mts` (extend),
  `apps/web/tests/previewRenderSettings.test.mts` (new).
