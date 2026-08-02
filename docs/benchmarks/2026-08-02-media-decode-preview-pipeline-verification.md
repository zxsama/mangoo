# Media Decode & Preview Pipeline — Verification

Date: 2026-08-02
Plan: `docs/superpowers/plans/2026-07-24-media-decode-preview-pipeline.md`
Spec: `docs/superpowers/specs/2026-07-24-media-decode-preview-pipeline-design.md`
Branch: `perf/million-thumbnail-hotloop`
Machine: Windows 11 Pro for Workstations, ffmpeg present (with libdav1d)

All numbers below were measured on the real desktop app driven over CDP, not estimated.

## 1. Full gate

`npm test` — **exit 0**, every stage PASS:

```
PASS: structure and contracts
PASS: core api contract alignment
PASS: core-client contract boundary
PASS: shared notification contract boundary
PASS: desktop core orchestration boundaries
PASS: web client boundaries
PASS: UI liquid glass design boundaries
PASS: sqlite schema
PASS: rustc 1.95.0 / cargo 1.95.0
test result: ok. 466 passed; 0 failed; 2 ignored
```

Web unit tests: 301 passed / 0 failed (baseline before this plan: 253).

## 2. Continuous navigation — the headline result

Setup: `MEGLE_AUTO_ADD_ROOT="C:/Users/84460/Pictures/normal"` (31 multi-megabyte
illustrations), preview opened on the first image, then N synthetic `ArrowRight`
key events dispatched through CDP `Input.dispatchKeyEvent` at a fixed interval.
"Frames painted" counts distinct `data-preview-media-id` values committed to the
canvas, observed by a `MutationObserver`.

| Input interval | Frames painted | Wall clock | Input window | Median inter-frame gap | Max gap |
|---|---|---|---|---|---|
| 60 ms | 32 | 1386 ms | 1800 ms | — | — |
| 30 ms (Windows auto-repeat) | 32 | **898 ms** | 900 ms | 40 ms | 102 ms |

**The walk completes inside the input window at Windows auto-repeat speed.**
Painting is no longer the bottleneck — the key repeat rate is. For reference the
pre-pipeline baseline for this same folder was **~8 s**, against BandiView's
~2 s (both from the design spec's measurements, taken by holding the arrow key).

Earlier in the same session, before the sharpening upgrade was working (see §5),
the same walk ran at a median 11 ms gap. The rise to 40 ms is the cost of the
background full-resolution decodes now running per image — a deliberate trade
for correct zoom quality, and still comfortably inside the input window.

## 3. The canvas render lane, and the memory it avoids

Observed on the live preview surface:

```
canvas.preview-canvas present : true
legacy <img> preview present  : false
```

For a 10240×5760 original with a `preview_2048` tier pooled:

```
canvas backing store : 2048x1152     (9.4 MB decoded)
canvas CSS box       : 10240px x 5760px
```

This is the backing-store/CSS-box split working exactly as designed. Had the
backing store been sized to the natural dimensions it would have been
10240×5760×4 = **236 MB** of GPU memory for one frame. The CSS box stays at the
original's size so `.central-preview-transform`'s existing zoom/pan transform
and "actual pixels" zoom are untouched.

After the background upgrade lands, backing store and CSS box converge:

```
canvas backing store : 10240x5760    (full original resolution)
canvas CSS box       : 10240px x 5760px
```

## 4. Preview tier resolution

`x-megle-preview-tier` values actually observed on the wire during one 31-image
walk (CDP `Network.responseReceived`):

```
preview_2048   53
original       32     (one background sharpening upgrade per image)
preview_native  2
```

## 5. Two real defects found only by running the app

Both were invisible to the entire unit-test suite.

### 5a. The tier header was unreadable by the renderer (fixed, `ee5947a`)

The renderer runs at `http://127.0.0.1:5173` and Core at a dynamic loopback port
(`http://127.0.0.1:6274` in this run) — **cross-origin**. Only CORS-safelisted
response headers are readable from JS, so `x-megle-preview-tier` was invisible:

```
readableHeaders: ["cache-control", "content-length", "content-type"]
tier: null
```

`getPreviewResource` therefore always fell back to `"original"`, which silently
disabled the background sharpening upgrade — a >4096px image would have stayed
at tier resolution no matter how far you zoomed. Evidence: `original` requests
across a walk were **6** before the fix and **32** after (one per image), and
the canvas backing store went from a permanent 2048×1152 to the full 10240×5760.

Fix: `.expose_headers([x-megle-preview-tier])` on the CORS layer, plus an
assertion in `configured_cors_allows_only_the_exact_development_origin`. That
assertion is mutation-proofed: removing the `expose_headers` call fails it.

### 5b. Every navigation re-fetched already-pooled neighbours (fixed, `91d59b9`)

`requestPreviewBitmap` cannot consult the pool before fetching, because the tier
is part of the cache key and only the response reveals it. So the prefetch
effect re-requested every neighbour's bytes on every navigation even though the
pool already held their decoded bitmaps.

Measured over one identical 31-image walk:

| | preview_2048 | original | total |
|---|---|---|---|
| before | 350 | 53 | 403 |
| after | 54 | 6 | 60 |

**85% fewer requests**, with no change in navigation speed.

## 6. Format coverage (Issue 5)

Fixtures authored with ffmpeg, dropped into the watched root, picked up by the
watcher, then removed afterwards.

| File | Indexed kind | Thumbnail | Preview tier | Preview content-type |
|---|---|---|---|---|
| `fmt_test.avif` | image | **ready** | original | `image/avif` |
| `fmt_test.tiff` | image | **ready** | preview_native | **`image/webp`** |

AVIF previously produced a *permanent* thumbnail failure — the `image` crate
cannot decode it and the chain had no fallback. It now succeeds through
`FfmpegDecoder`. TIFF is browser-unrenderable, so the resolver transcodes it to
WebP rather than streaming bytes Chromium cannot paint.

`.heif` is also now indexed; before this work only `.heic` was, because
`scan/mod.rs` and `watch/mod.rs` held byte-identical private copies of the
extension table that had drifted. They now share `decode/formats.rs`.

**Still not supported, by design:** `.psd` is routed to ffmpeg speculatively and
`.raw` has no decoder at all — both report a precise `no decoder handles` /
chain-failure message instead of a misleading "unsupported format". Pure-Rust
`psd`/`rawler` decoders remain deferred in the spec.

## 7. Memory stability

`HeapProfiler.collectGarbage` before each sample, 240 additional navigations
(4 laps forward and back over the 31-image folder) between them:

```
BEFORE (after 60 navigations) : 26 MB
AFTER  (+240 navigations)     : 24 MB
DELTA                         : -2 MB
```

No accumulation. Caveat worth recording: `performance.memory` measures the JS
heap, and `ImageBitmap` pixel storage lives outside it, so this demonstrates
JS-side stability rather than directly proving GPU-side release. The pool's
`.close()` lifecycle on evict/overwrite/delete/clear is covered by unit tests
(`apps/web/tests/previewBitmapPool.test.mts`), each of which fails if the
corresponding `close()` is removed.

## 8. Not verified in this pass

- **Library-switch bitmap release.** A second library was not configured on this
  machine, so `releasePreviewBitmaps` was exercised only by unit tests, not in
  the running app.
- **Zoom past the tier by hand.** The upgrade was verified by the canvas backing
  store reaching full resolution and by the `original` request count, not by
  driving the zoom gesture.
- **A large real HEIC/PSD library.** Two deferred notes in the plan ledger want
  measurement against one: the full-resolution PNG round trip ffmpeg writes to
  `%TEMP%` per HEIC/AVIF/PSD thumbnail, and the absence of a negative cache
  (a permanently undecodable file re-runs generation on every preview request).
