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

Filled in by Task 8, after the feature is built.
