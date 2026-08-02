# Preview render — browser capability checks

Date: 2026-08-02
Plan: `docs/superpowers/plans/2026-08-02-preview-navigation-gate-and-render-settings.md` (Task 1)
Spec: `docs/superpowers/specs/2026-08-02-preview-navigation-gate-and-render-settings-design.md`
Runtime: Electron 43.1.0, **Chromium 150.0.7871.47**, Windows 11 Pro for Workstations

Both checks were run against the real running app over CDP, before any feature
code was written, because the design rests on both and neither was proven.

## Raw probe output

```json
{
  "chrome": "150.0.7871.47",
  "HIGH_QUALITY_SUPPORTED": false,
  "sourcePixel":   [255, 0, 0, 128],
  "filteredPixel": [255, 0, 0, 255],
  "FECOLORMATRIX_UNPREMULTIPLIES": true
}
```

## Check 1 — is `image-rendering: high-quality` usable?

**No.** `CSS.supports("image-rendering", "high-quality")` is `false` in this
Chromium, so the declaration would be dropped and the element would fall back to
`auto` — identical to Bilinear.

**Decision: ship two filter options**, 双线性 Bilinear (`auto`) and 最近邻
Nearest neighbour (`pixelated`). A third row labelled "High quality" would name
behaviour that does not happen, which is worse than not offering it. Revisit only
if a future Chromium reports support *and* renders visibly differently.

## Check 2 — does `feColorMatrix` operate on non-premultiplied colour?

**Yes.** This is the assumption the whole channel feature rests on.

Method: fill a 1×1 canvas with `rgba(255,0,0,0.5)`, then draw it through a
`feColorMatrix` that passes RGB straight through and forces `A = 1`.

- If the filter operated on **premultiplied** data it would see R≈128, pass it
  through, and store `(128,0,0,255)` — `getImageData` would read **128**.
- If it **un-premultiplies** first it sees R=255, and stores `(255,0,0,255)` —
  `getImageData` reads **255**.

Observed: **255**. The filter un-premultiplies, applies the matrix, and
re-premultiplies, exactly as the SVG specification requires.

(The `sourcePixel` reading of 255 is not evidence either way — `getImageData` is
specified to return non-premultiplied data regardless of internal storage. The
discriminating value is `filteredPixel[0]`.)

**Decision: the design holds as written.** Channel views can show the raw stored
channel value undarkened by alpha, which is the requirement for formats like TGA
where the A channel carries data rather than transparency. No WebGL paint path is
needed, so the renderer built and reviewed on this branch stays untouched.

One implementation consequence carried into the plan: each `<filter>` must set
`color-interpolation-filters="sRGB"`. The SVG default is `linearRGB`, which would
gamma-shift every channel view and misreport the stored values.

## Results of the real-app feature verification

Run after the feature was built, driving the real desktop app over CDP.
`npm test` was green first: exit 0, all nine stages PASS, 470 Rust / 326 web.

### Navigation gate

The gate releases on FIRST PAINT, so it is invisible whenever the bitmap pool is
keeping up — which makes it easy to mis-measure. Two probes were needed.

**Naive timing probe (misleading).** Dispatching six wheel events ~12 ms apart
advanced six images, and 12 keypresses displayed 19. That is not a failure: a
pooled frame paints in well under a millisecond, so each event legitimately
found the gate open. It shows the gate is not adding lag on a warm pool.

**Synchronous burst (the real test).** Six `wheel` events dispatched inside a
single JS task, so no paint can land between them:

```
SYNCHRONOUS_BURST: {"startId":"663999","idImmediatelyAfter":"663999"}
SETTLED_ID: 664000
```

Six events advanced **exactly one** image. Five were dropped. Before this change
all six would have applied.

**Throttled network (the cold-pool case the gate exists for).** 300 ms latency,
200 KB/s, then 15 keypresses at 25 ms intervals:

```
THROTTLED_WALK: 15 keypresses -> 3 displayed, 0 skipped-over gap(s)
SEQUENCE: 664000 664001 664002
```

Consecutive ids with no jumps: every image the selection moved through actually
appeared. The 12 keypresses that outran painting were dropped, not queued.

**Broken image.** A file with a valid `.png` extension and garbage content was
placed in the root for the whole session. Navigation never wedged —
`PreviewCanvas`'s error path calls `onMediaReady`, so a media that cannot decode
releases the gate exactly like one that paints.

### Render settings

All seven options resolve to the intended CSS on the shared `.preview-image`
class, so every preview lane (canvas, `<img>`, `<video>`) is covered:

| Option | `image-rendering` | `filter` |
|---|---|---|
| 双线性 Bilinear | `auto` | `none` |
| 最近邻 Nearest neighbour | `pixelated` | `none` |
| RGBA (default) | `auto` | `none` |
| RGB | `auto` | `url("#megle-channel-rgb")` |
| R / G / B / A | `auto` | `url("#megle-channel-<c>")` |

Button present in the titlebar (`[data-titlebar-control="preview-render"]`), all
five matrices defined in the hidden SVG, and the RGBA default applies no filter
at all.

**The alpha claim, verified through the shipped filter** (not just the Task 1
probe). A 50%-alpha red pixel drawn through `#megle-channel-rgb`:

```json
{ "rgbView":   [255, 0, 0, 255], "rawStoredRGBPreserved": true,
  "alphaView": [128, 128, 128, 255] }
```

RGB reads back **255**, the raw stored value — not the 128 a premultiplied
pipeline would show. The A view renders alpha 128 as neutral grey, readable as
data. This is the behaviour the feature exists for on formats like TGA where the
alpha channel carries information rather than transparency.

### Not verified here

- **Visual confirmation of nearest vs bilinear at high zoom.** The CSS is
  provably applied, but "does it look right zoomed to 800%" is a judgement for
  the human on the real display — CDP screenshots do not reliably reflect
  on-screen resampling (see CLAUDE.md's note on capture vs realtime compositing).
- **Restart persistence.** The values round-trip through
  `normalizePreviewPreferences` under unit test, and ride the same localStorage
  path as every other preview preference, but a full app restart with a changed
  setting was not driven end to end.
