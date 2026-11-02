# Design: Preview scrubber (browsing control bar)

Date: 2026-08-02
Status: Design (brainstorming) — pending implementation plan

## 1. Problem

Jumping to an arbitrary position in a folder means holding an arrow key or
scrolling repeatedly. BandiView solves this with a floating control bar at the
bottom of the image: a track, a draggable thumb, a position tooltip, and a pin
to stop it auto-hiding. Mangoo has no equivalent.

## 2. Reference: what BandiView actually does

Inspected at `C:\Program Files\BandiView` — its language file, registry
settings, and the `BandiView.exe` string tables — rather than inferred from the
screenshot.

**Persisted settings (`HKCU\Software\BandiView`):**

| Setting | Value | Meaning |
|---|---|---|
| `showControlbar` | 1 | The feature is enabled |
| `lockControlBar` | 0 | Defeats auto-hide — this is the pin |
| `cacheImage` | 1 | Decoded-image cache on |
| `cacheSizeMB` | 1200 | **1200 MB** of decoded images |
| `clickCanvas2ShowControls` | 0 | Alternative reveal trigger |

**Strings:** `CHK_SHOW_CONTROL_BAR` = "Show the bottom control bar";
`$TT_LOCKCONTROLBAR` = "Lock Control Bar (Hotkey: ~)". The pin is a *lock*, and
enablement and locking are two independent flags.

**Implementation:** the bar is a skinned `Slider` control in an XML skin engine
(`ThumbImage`, `EnableTooltip` properties). A `BandiView.Cache.DB.oii` file plus
`libsqlite.x64.dll` provide a persistent cache behind the in-memory one.

**Why its scrubbing feels instant** — and this is the load-bearing finding — is
not a clever algorithm. It is (a) a 1200 MB decoded cache, so most drag
positions hit, and (b) the fact that on a large folder one pixel of track spans
many images, so a drag visits far fewer images than it appears to. The user
observed exactly this: "20k 图片拖动时有最小拖动张数，小小的拖动一下就会跳过几十张图".

**Mangoo already has every equivalent**: the decoded `previewBitmapPool` (75% of
the same 1200 MB default), `preview_blobs` as the persistent cache, and the
glass/overlay primitives for the UI. This design needs no new mechanism.

**Same problem as ours, solved with different timing.** BandiView also browses
a whole folder tree flattened into one list — you can drop a directory on it and
page through every descendant. It pays for that upfront: a progress bar while it
enumerates, after which it owns a complete in-memory list and every index is
immediately addressable.

Mangoo has the same capability — `showChildFolderContents` drives the
`includeDescendants` query flag in `useLibraryData`, and `mediaTotalCount` then
reports the flattened total. The difference is only *when* the cost is paid: we
load incrementally in the background instead of blocking on a progress bar, so
`mediaSlots` is sparse and a far-away index may not be resident yet. §4 covers
the consequence for the scrubber, which is a brief lag rather than a wait.

## 3. Drag model

**The thumb position and the displayed image never disagree.** Dragging maps
pointer-x to an absolute folder index and selects that media immediately — the
same path a keypress takes. No placeholder, no deferred commit, no
"catch-up" lag.

Three properties make this hold at 20k images:

1. **The minimum step is inherent, not throttling.** 20 000 images over an
   ~800px track is ~25 images per pixel. Nothing implements this; it falls out
   of the mapping. It is also why a drag that *looks* continuous visits only a
   few hundred images, not 20 000.
2. **Only distinct indices commit.** Pointer movement within one pixel
   re-computes the same index and does nothing.
3. **Requests supersede rather than queue.** Each new index aborts the
   in-flight one — `PreviewCanvas` already does this with its `AbortController`.
   Measured median cost per image on the real library is **14 ms**, so scrubbing
   tracks the pointer.

**The first-paint navigation gate does NOT apply to scrubbing.** That gate
(`canStartPreviewNavigation`) exists to stop the keyboard and wheel outrunning
the display. Applying it to a drag would contradict this section's requirement —
the thumb would stick while an image loaded. Dragging calls
`library.setSelectedMediaId` directly; the gate continues to govern keys and
wheel unchanged.

## 4. Position model

The scrubber addresses **absolute folder position**, never the loaded window.

`resolvePreviewTitlePosition` (`apps/web/src/app/previewTitle.ts`) already
computes `{ current, total }` from `mediaSlots` + `mediaTotalCount`, and already
renders the "13/31" in the titlebar. The scrubber consumes the same value, so
thumb, tooltip, and title cannot drift apart.

This holds whether the list is a single folder or a whole subtree flattened by
`showChildFolderContents` — the scrubber reads the same
`mediaTotalCount`/`mediaSlots` pair either way and needs no special case.

**Scrubbing outside the loaded window.** `mediaSlots` is sparse: dragging to
image 8000 of 20000 may land on an unloaded slot, where there is no media id to
select. The drag sets the target index and the existing media-window machinery
loads that region — the same mechanism the grid uses when jumping to a position.
Until the slot resolves, the thumb and tooltip move while the image follows.

This is the one place position and picture can briefly disagree, and it is the
price of loading in the background instead of behind a progress bar. It is
bounded by a single API round-trip, and it does not occur within the loaded
window, which extends well beyond the visible neighbourhood. The alternative —
blocking on a full enumeration the way BandiView does — was rejected: it would
stall opening a large library for the sake of a rarely-used far jump.

## 5. Visibility and the pin

Mirroring BandiView's two-flag factoring, but with only the flag we need.

One new preference, `previewScrubberPinned: boolean` (default `false`), joins
`PreviewPreferences` — inheriting localStorage persistence and cross-window
`broadcastPreference` sync, exactly as `previewRenderFilter` and
`previewChannel` do.

**No separate "enable the scrubber" setting.** BandiView has `showControlbar`
because its bar occupies permanent chrome. Ours auto-hides, so it already costs
nothing when unused. YAGNI.

Visible when the preview is open AND (`pinned` OR the pointer is inside a reveal
band across the bottom of the preview viewport):

- The band is the bar's own height plus a margin above it, so it does not have
  to be hunted for.
- It fades out after a short delay once the pointer leaves.
- **It never hides mid-drag**, even if the pointer leaves the band — releasing
  the thumb outside the bar must not make it vanish underneath the cursor.
- The pin sits at the right end of the bar, as in the reference screenshot, and
  renders a filled/rotated pushpin when active.

## 6. Structure

Three units, following the pure-logic/component split the codebase already uses
(`previewRenderSettings.ts` + `PreviewRenderMenu.tsx`).

**Create `apps/web/src/features/preview/previewScrubber.ts`** — pure, no DOM,
no React:

```ts
export function scrubberIndexForPointerX(
  pointerX: number,
  track: { left: number; width: number },
  total: number
): number;                       // clamped to [0, total-1]

export function scrubberOffsetForIndex(
  index: number,
  trackWidth: number,
  total: number
): number;                       // px offset of the thumb centre

export function shouldShowScrubber(input: {
  previewOpen: boolean;
  pinned: boolean;
  pointerNearBottom: boolean;
  dragging: boolean;
}): boolean;
```

**Create `apps/web/src/features/preview/PreviewScrubber.tsx`** — the widget:
`LiquidGlassSurface` track, thumb, tooltip, and a `LiquidGlassButton` pin.
Stateless beyond its own drag state; position and pinned flag come from props.

**Modify `apps/web/src/features/preview/CentralPreviewStage.tsx`** — render it
inside `.central-preview-stage`, positioned bottom-centre, and supply the
pointer-proximity flag from the stage's existing `onPointerMove`.

**Modify** `previewPreferences.ts` (one field), `App.tsx` (wire the preference
and the position), `styles.css` (layout and the fade).

## 7. Testing

**Pure, `node --test`:**
- `scrubberIndexForPointerX`: both endpoints land on 0 and `total-1`; the
  midpoint lands mid-folder; pointer-x outside the track clamps rather than
  going out of range; a **single-image folder** (`total === 1`) must not divide
  by zero; `total === 0` returns 0 and never `NaN`.
- Round-trip: `scrubberOffsetForIndex` then `scrubberIndexForPointerX` returns
  the original index for a sample across a 20 000-image folder — this is what
  proves thumb and image agree.
- `shouldShowScrubber`: the full truth table, including that `dragging === true`
  keeps it visible even when `pointerNearBottom` is false, and that everything
  is false when the preview is closed.

**Real-app:** scrub a large folder and confirm the picture tracks the thumb;
confirm the ~25-images-per-pixel step appears naturally; confirm auto-hide,
reveal, pin, and that the pin survives a restart; confirm the bar does not
appear when the preview is closed.

## 8. Out of scope

Thumbnail strip along the track (BandiView has this in a separate "Select Image"
navigator, not the control bar), keyboard control of the scrubber, and any
additional control-bar buttons beyond the pin. The existing titlebar already
carries back / prev / next / zoom-mode / render settings.
