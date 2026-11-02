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

Setup: `MANGOO_AUTO_ADD_ROOT="C:/Users/84460/Pictures/normal"` (31 multi-megabyte
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

`x-mangoo-preview-tier` values actually observed on the wire during one 31-image
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
response headers are readable from JS, so `x-mangoo-preview-tier` was invisible:

```
readableHeaders: ["cache-control", "content-length", "content-type"]
tier: null
```

`getPreviewResource` therefore always fell back to `"original"`, which silently
disabled the background sharpening upgrade — a >4096px image would have stayed
at tier resolution no matter how far you zoomed. Evidence: `original` requests
across a walk were **6** before the fix and **32** after (one per image), and
the canvas backing store went from a permanent 2048×1152 to the full 10240×5760.

Fix: `.expose_headers([x-mangoo-preview-tier])` on the CORS layer, plus an
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

---

## 9. Post-final-review fix wave (2026-08-02, later)

The whole-branch review was run in five slices and returned **2 Critical + 14
Important**. All are fixed; the numbers below are re-measured on the fixed tree.

### 9a. EXIF orientation — the Critical, fixed at the root

Core applied EXIF nowhere, so it stored raw raster dimensions while the browser
decoded the same file EXIF-applied. Verified before and after with a
purpose-built orientation-6 JPEG driven through the running app:

```
before   coreStored 400x200   browserDecoded 200x400   MISMATCH
after    coreStored 200x400   browserDecoded 200x400   AGREE
```

Fixed in the decode registry (`image` 0.25's native `ImageDecoder::orientation`
+ `DynamicImage::apply_orientation`, no new dependency), so Core's stored
dimensions, grid thumbnails, preview tiers and the browser all agree. This also
fixes the **pre-existing** sideways-thumbnail bug for rotated phone photos,
rather than compensating for it in the canvas.

### 9b. The sharpening upgrade is now on demand

The plan had it fire unconditionally per image, which recreated the exact memory
blow-up the tier system exists to prevent. It is now gated on the user actually
zooming past fit — which is what the design spec said all along.

| | originals per walk | median inter-frame gap | JS heap | canvas backing store |
|---|---|---|---|---|
| unconditional | 32 | 40 ms | 45 MB | 10240x5760 (**236 MB**) |
| on demand | 17 | **14 ms** | 30 MB | 3072x1728 (**21 MB**) |

The remaining 17 are images that legitimately get no tier (at or below 4096px,
or dimensions still pending) — not upgrades.

Navigation after the whole fix wave: **32 frames in 912 ms** against a 900 ms
input window at 30 ms auto-repeat. Still keeping pace with the key repeat rate,
now without the background full-resolution decodes.

### 9c. Crash and resource-loop classes closed

- A 20000x20000 TIFF preview was ~2.8 GB peak on an unbounded blocking pool — a
  Rust OOM, which aborts the whole Core process. Now rejected from the file
  header, with wording that classifies **permanent** so it cannot arm the retry
  sweep.
- ffmpeg's full-resolution `%TEMP%` PNG tripped `image`'s 512 MB limit, and that
  failure text is a *transient* signature, so the 60 s sweep re-ran a doomed
  gigapixel decode **every minute forever**. ffmpeg now always scales (8192
  ceiling — above any real photograph, so reported dimensions stay exact) and
  writes at `-compression_level 1`.
- Preview decode is bounded by a 3-permit semaphore; it was the only decode path
  in the codebase with no concurrency limit.
- A cache-*write* failure used to 500 a successfully decoded preview. It now
  degrades, closing the last hole in "degrade, never fail".
- `preview_blobs` was missing from the inactive-artifact sweep, so recycled
  files stranded tiers that then evicted live ones. Fixed and mutation-proofed.

### 9d. Frontend efficiency

Aborted prefetches now pool their finished bitmap instead of closing it (a cold
walk was decoding every frame twice); the prefetch short-circuit is
viewport-aware so a window resize re-warms instead of being suppressed for the
rest of the session; the viewport is recomputed on resize; the prefetch window
is sized against the decoded-byte pool share rather than the raw setting.

### 9e. Process note

Python `io.open(..., 'w')` on Windows silently rewrote every edited file from LF
to CRLF, which broke `check:ui-design` (its regex matches `
  \}

  function`).
Twelve files were normalised back and the gate re-run. Any future scripted edit
in this repo must pass `newline='
'`.
