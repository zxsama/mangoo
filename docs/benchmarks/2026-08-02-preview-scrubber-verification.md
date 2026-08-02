# Preview scrubber — verification

Date: 2026-08-02
Plan: `docs/superpowers/plans/2026-08-02-preview-scrubber.md`
Spec: `docs/superpowers/specs/2026-08-02-preview-scrubber-design.md`
Branch: `perf/million-thumbnail-hotloop`

## Full gate

`npm test` — **exit 0**, all nine stages PASS. 470 Rust tests, 345 web tests
(up from 326; +19 from this feature).

## Real-app behaviour, driven over CDP

Benchmark folder: `C:\Users\84460\Pictures\normal`, 31 images.

```
MOUNTED:                    {"present":true,"visible":"false","tooltip":"1/31",
                             "thumbLeft":"0%","pin":true}
POINTER_CENTRE:             visible=false
POINTER_BOTTOM:             visible=true
DRAG_DOWN:                  {"beforeId":"663976","tooltipAfterDown":"1/31"}
AFTER_DRAG:                 {"tooltip":"24/31","thumbLeft":"76.6667%",
                             "shownId":"663999"}
MID_DRAG_POINTER_AWAY:      visible=true
AFTER_RELEASE_POINTER_AWAY: visible=false
PINNED_POINTER_AWAY:        visible=true
PIN_PERSISTED:              true
```

| Requirement | Evidence |
|---|---|
| Auto-hides during preview | Hidden with the pointer at stage centre |
| Reveals near the bottom | `visible=true` once inside the reveal band |
| Thumb position and image agree | Drag to 75% → tooltip `24/31`, thumb `76.6667%`, displayed id changed `663976` → `663999` |
| Never hides mid-drag | Pointer moved to the top of the stage while the button was down — stayed visible |
| Auto-hide resumes after release | `visible=false` with the pointer still away |
| Pin defeats auto-hide | `visible=true` with the pointer at the top of the stage |
| Pin persists | `previewScrubberPinned: true` in `megle.preview.preferences` |

**Arithmetic cross-check.** 75% of a 31-image folder is index 23, which is
position **24/31**, and the thumb for index 23 sits at 23/30 = **76.667%**. Both
match the observed values, so the mapping and its inverse agree in the running
app and not merely in the unit tests.

## Unit coverage worth noting

`apps/web/tests/previewScrubber.test.mts` (19 assertions) pins the cases that
are hard to reach by hand:

- **Round-trip** `index → fraction → pointerX → index` across a **20 000-image**
  folder. This is the property that guarantees thumb and image agree; endpoint
  tests alone would pass with an off-by-one mapping.
- **Coarse step**: at 20 000 images one pixel of track moves >10 images,
  reproducing the behaviour observed in BandiView. It is inherent to the
  mapping, not a throttle.
- **No divide-by-zero**: single-image folder, empty folder, and a zero-width
  track all return finite values.
- **Visibility truth table**, including that `dragging` overrides
  `pointerNearBottom`, and that everything is hidden when the preview is closed.

## Guard

`tools/checks/validate-web-client.mjs` fails the build if `handleScrubToIndex`
ever calls `startPreviewNavigation` — routing the drag through the first-paint
gate would make the thumb stick while an image loaded. Mutation-tested:
replacing the handler body with a gate call fails the check; restoring passes.

The guard strips comments before matching, because the handler's own comment
explains *why* it does not call the gate and would otherwise trip it — a false
alarm caught during implementation.

## Not verified here

- **Feel of a large-folder drag.** The 31-image benchmark folder cannot exercise
  the ~25-images-per-pixel case in a way a human would judge; the unit test
  covers the arithmetic at 20 000, but the smoothness of a real drag over
  thousands of images is a judgement for the human on the physical display.
- **Flattened subtree mode.** `showChildFolderContents` was not toggled during
  this pass. The scrubber reads the same `mediaTotalCount`/`mediaSlots` pair in
  both modes and needs no special case, but that was reasoned, not observed.
- **Scrubbing outside the loaded window.** The benchmark folder loads entirely,
  so `requestMediaWindow` was never exercised by a far jump. Reachable only on a
  folder larger than the initial media window.

## Implementation note

`PreviewScrubber.tsx` was renamed to **`PreviewScrubberBar.tsx`** during
implementation: on Windows it collided case-insensitively with the pure module
`previewScrubber.ts`, and TypeScript rejected the pair (`TS1149`). The exported
component is `PreviewScrubberBar`; the pure module keeps the original name.
