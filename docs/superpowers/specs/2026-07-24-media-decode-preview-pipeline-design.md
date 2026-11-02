# Design: Media decode & preview pipeline

Date: 2026-07-24
Status: Design (brainstorming) — pending implementation plan

## 1. Problem & principle

Two problems in Mangoo share one root principle — **decode once, into a
paint-ready form, ahead of time** — plus a shared need for pluggable format
support:

- **Issue 4 (preview perf).** Prev/next between large images is slow (~8s to walk
  the 31-image `C:\Users\84460\Pictures\normal` folder; BandiView ~2s).
  Proven by instrumentation: the preview renders each frame as a fresh
  `<img key={src}>`, so **every navigation re-decodes the full-resolution
  original on the main thread** (12 steps = 12 `Image.decode()` calls; a 16384px
  image = 337ms, 10032px = 323ms; 0 network fetches — bytes are cached, decode is
  the entire cost). A decoded bitmap does NOT transfer to a new `<img>` element
  even with the same object URL, so a per-URL cache cannot help. There is no
  downscaled preview tier — only a `grid_320` thumbnail tier exists.
- **Issue 5 (format support).** Scan already indexes `.avif/.psd/.heic/.raw/.tiff`
  etc., but the Rust `image` crate 0.25 (default features) cannot DECODE
  avif/heic/psd/raw → permanent thumbnail failures. Browser preview covers only
  what Chromium renders (AVIF yes; HEIC/PSD/TIFF/RAW no).

**Reference architecture (BandiView, inspected at `C:\Program Files\BandiView`):**
ships `skia.x64.dll` (GPU-composited decoded-bitmap surface — decode once, paint
synchronously, no re-decode on navigate), `vcomp140.dll` (OpenMP parallel
decode), and a plugin codec registry (`plugins/` + `dll/`: libheif, libavif,
libjxl, libraw, libdng, libopenexr, libbpg, ark archive engine, libsqlite). This
validates both a decode-once paint-ready surface with parallel pre-decode, and a
pluggable per-format decoder layer.

## 2. Scope

This spec designs the **full architecture**; the first implementation delivers a
**working core subset**:

**In scope now (core implementation):**
- Frontend preview surface: `createImageBitmap` (in a Web Worker) + `<canvas>`
  render, a pre-decoded ImageBitmap pool for neighbors, no hard navigation lock
  (placeholder-swap instead). Removes the ineffective decode-ahead object-URL
  code.
- Backend decoder abstraction (`crates/core/src/decode/`) as a capability-ordered
  chain: `image` crate → **ffmpeg** (AVIF/HEIC/TIFF/HEVC frames, reusing the
  already-installed binary, NO new C-crate compilation) → format-specific pure-
  Rust crates.
- Preview-source resolution + transcode fallback in `get_preview`: browser-
  unrenderable IMAGES → decode to WebP; images >4096px → adaptive downscaled WebP
  preview tier (on-demand + cached); videos unplayable → leave a plug point.
- Adaptive downscaled preview tier (discretized profiles), generated on demand
  and cached in `thumb_blobs`.

**In scope as design + plug points only (implemented later):**
- Format-specific decoders PSD (`psd` crate), RAW (`rawler`), JXL, etc.
- Video transcode streaming (HEVC/WMV → H.264/VP9).
- Future format families: archives (rar/7z), documents (pdf/epub/txt/md), 3D
  models (fbx/obj/stl).

**Constraint:** Do NOT disturb the user's uncommitted working-tree changes. All
commits stage explicit paths only.

## 3. Architecture overview

Three units spanning Rust Core and the React frontend:

### 3a. Backend decoder abstraction — `crates/core/src/decode/`

A `MediaDecoder` trait + a `DecoderRegistry` (capability-ordered responsibility
chain). Thumbnail generation and preview transcode both ask the registry for a
decoded RGBA image; neither calls `image::open` directly anymore. Adding a future
format family = adding one decoder to the chain.

### 3b. Preview source & transcode fallback — `crates/core/src/api/routes.rs` + a new preview tier

`get_preview` becomes a **preview-source resolver**: browser-renderable originals
stream raw bytes (unchanged, zero cost); unrenderable images decode to WebP;
>4096px images return an adaptive downscaled WebP preview tier; unplayable video
returns a marker (plug point). The preview tier is a new, on-demand,
`thumb_blobs`-cached profile family distinct from `grid_320`.

### 3c. Frontend preview surface — `apps/web/src/features/preview/`

Static images render via `<canvas>` with `ctx.drawImage(imageBitmap)` (decoded
once in a Worker, pre-decoded into a pool, painted synchronously — no re-decode
on navigate). Animated images (GIF / animated WebP / animated AVIF) keep using
`<img>` (native animation). Video keeps using `<video>`. The hard navigation lock
is removed: a ready pooled bitmap paints immediately; an un-ready frame shows the
downscaled preview tier as a placeholder and is seamlessly replaced when its
full-resolution bitmap finishes decoding.

**Data flow:** browse/scroll → frontend requests the preview endpoint (raw
original OR adaptive downscaled WebP, passing the preview viewport size) → backend
decoder produces bytes → frontend `createImageBitmap` (Worker) pre-decodes into
the pool → canvas paints synchronously.

## 4. Backend decoder abstraction (detail)

```rust
pub enum DecodePurpose { Thumbnail, Preview }

pub struct DecodeRequest<'a> {
    pub source: MediaSource<'a>,   // file path today; archive-entry etc. later
    pub ext: &'a str,              // lowercase incl. dot, e.g. ".heic"
    pub target_max_px: Option<u32>,// longest-side cap; None = native resolution
    pub purpose: DecodePurpose,
}

pub struct DecodedImage {
    pub rgba: Vec<u8>,             // decoders ALWAYS output RGBA
    pub width: u32,
    pub height: u32,
}

pub trait MediaDecoder: Send + Sync {
    fn name(&self) -> &'static str;
    fn handles(&self, ext: &str, purpose: DecodePurpose) -> bool;
    fn decode(&self, req: &DecodeRequest) -> anyhow::Result<DecodedImage>;
}
```

`MediaSource` is an abstraction (a real file path today) so a future
`ArchiveProvider` can present an archive entry through the same decode chain
without changing decoders.

**Registry = responsibility chain.** `DecoderRegistry` asks each decoder
`handles(ext, purpose)` in order; the first `true` runs `decode()`. **On failure
it continues to the next handling decoder** (e.g. `image` fails on an exotic TIFF
variant → falls through to ffmpeg). This graceful fall-through is what lets edge
formats still produce an image.

**Registration order (capability-ascending):**
1. `ImageCrateDecoder` — JPEG/PNG/GIF/WebP/BMP/common TIFF. Fast, no external deps.
2. `FfmpegDecoder` — AVIF/HEIC/HEIF/TIFF variants/HEVC video frames. Reuses the
   already-installed ffmpeg subprocess (same pattern as the existing
   `generate_video_thumbnail`); invokes it to decode a single frame and reads it
   back as RGBA (via rawvideo or an intermediate PNG that is decoded to RGBA — so
   the registry's uniform RGBA contract holds). **No new C-crate compilation**
   (the `dav1d` Rust crate would need cmake+nasm, which are not available — avoid
   it; ffmpeg already bundles libdav1d/HEVC/TIFF).
3. `PsdDecoder` — pure-Rust `psd` crate, merged/composite layer. (Later.)
4. `RawDecoder` — pure-Rust `rawler`, embedded preview or bayer decode. (Later.)

**Decoders always output RGBA** (single responsibility: decode only). Scaling +
WebP encoding stay in the upper layer, reusing the existing `thumbnail_exact` +
`webp::Encoder` path. The ffmpeg decoder internally converts its frame back to
RGBA rather than emitting WebP directly, keeping one uniform product type.

**Consumers switch to the registry:**
- `thumbnails/mod.rs::generate_image_thumbnail_bytes` → `registry.decode({purpose:
  Thumbnail, target_max_px: 320})` then WebP-encode (existing encode logic
  unchanged).
- The new preview transcode fallback (§5) → `registry.decode({purpose: Preview,
  target_max_px: adaptive})`.

**`classify_thumbnail_failure` update:** a format is only `permanent` when **every**
handling decoder in the chain fails or none handles the extension. A missing/
incapable ffmpeg is a distinct, diagnosable error (not "format unsupported"), and
formats the chain can now handle stop being mis-flagged permanent.

**Format detection is duplicated** in `scan/mod.rs:704` (`media_kind`) and
`watch/mod.rs:392` — any new indexed extension must be added in both. Also fix the
pre-existing inconsistency where `.heif` is not indexed (only `.heic` is).

## 5. Preview source & transcode fallback (detail)

`get_preview` resolves what to return by format and size:

| Case | Returned |
|---|---|
| Browser-renderable image (JPEG/PNG/GIF/WebP/AVIF) ≤ 4096px | raw file bytes (unchanged, zero cost) |
| Browser-renderable image > 4096px | **adaptive downscaled WebP preview tier** (short side = preview viewport × 1.2, never exceeding the original) |
| Browser-UNrenderable image (PSD/HEIC/RAW/TIFF/HEVC-still) | decoder chain → WebP (same size rule: >4096 downscaled, else full-resolution WebP) |
| Playable video (H.264/VP9/AV1 in mp4/webm) | raw file bytes (unchanged) |
| Unplayable video (HEVC/WMV/MKV…) | **plug point** — a marker the frontend renders as "格式暂不支持预览播放" (transcode streaming is later work) |

**New preview tier** (distinct from `grid_320`):
- Generated **on demand** the moment a preview opens, cached into `thumb_blobs`
  (reusing existing BLOB storage) under a **different profile name**.
- Preview size depends on the viewport, but `thumb_blobs` is keyed by
  `(file_id, profile)` (unique). Resolve the tension by **discretizing** the
  preview tier into a small set of fixed size profiles (e.g. `preview_2048`,
  `preview_3072`, `preview_4096`). The frontend reports its preview viewport short
  side (`viewportShortPx` query param); the backend picks the **smallest profile
  ≥ viewport × 1.2** (capped at the original's size). This keeps
  `(file_id, profile)` unique + cacheable while never generating a bespoke size
  per pixel dimension.
- **Only images > 4096px get a downscaled tier.** ≤ 4096px decodes the original
  directly (browser-renderable ones stream raw; unrenderable ones decode to a
  full-resolution WebP).

## 6. Frontend preview surface (detail)

**Render dispatch by media type (three clean lanes):**
- **Static image → `<canvas>`.** `ctx.drawImage(imageBitmap, …)`; zoom / fit /
  rotate / double-click-zoom become canvas transform parameters (scale/translate/
  rotate matrix). This is the main refactor: the current `CentralPreviewStage`
  `<img>` + CSS-transform gesture logic migrates to canvas draw parameters.
- **Animated image (GIF / animated WebP / animated AVIF) → `<img>`** (native
  animation; a canvas would freeze on one frame).
- **Video → `<video>`** (unchanged).

**Pre-decoded ImageBitmap pool** (the accelerator):
- `Map<mediaId, ImageBitmap>`, LRU-bounded by the preview-buffer budget (the
  existing setting), holding the current ± N neighbors' **decoded** ImageBitmaps.
- Bitmaps produced via `createImageBitmap(blob)` **in a Web Worker**, so
  large-image decode stays off the main thread; the main thread receives finished
  bitmaps.
- Navigating to a pooled media id → `drawImage` immediately (synchronous). While
  viewing the current frame, neighbors are fetched + decoded into the pool in the
  background.

**No hard navigation lock — placeholder swap instead:**
- Target frame's bitmap is pooled → paint immediately, accept the next step (no
  lock wait).
- Not ready (rapid nav outran the pre-decode window) → paint the downscaled
  preview tier (fast to decode) or the previous frame scaled, as a placeholder;
  when the full-resolution ImageBitmap finishes decoding, replace seamlessly. This
  is the BandiView feel: fast continuous nav, occasionally a frame is briefly
  low-res then sharpens.

**Memory management (critical for ImageBitmap):**
- ImageBitmap must be explicitly `.close()`d (else GPU memory leaks). Close on LRU
  eviction; clear + close all on preview close and on library switch (the existing
  `resetMediaCachesForLibrarySwitch` path, extended).
- Pool size = preview-buffer budget / estimated bytes-per-decoded-bitmap.

## 7. Extensibility, error handling, testing

**Future format plug points** (design only now):
- **Archives (rar/7z):** a container, not a decoder — a front `ArchiveProvider`
  that lists/extracts inner media. `MediaSource` in `DecodeRequest` abstracts the
  path so archive-internal images reuse the whole decode chain.
- **Documents (pdf/epub):** a `DocumentDecoder` (render first page/cover to RGBA)
  on the chain, producing Thumbnail/Preview. txt/md → render first screen of text
  to an image, or a text preview-source type on the frontend.
- **3D (fbx/obj/stl):** a `MeshThumbnailDecoder` (offscreen-render one view), or a
  three.js interactive preview-source type.
- **Key abstraction:** the **preview-source type** grows from "image/video" to an
  enum (`Image | Video | Document | Model | Text | …`); the frontend preview
  surface dispatches by type to the right renderer (canvas / video / pdf-viewer /
  three-canvas). This implementation ships Image + Video and leaves the enum +
  dispatch point in place.

**Error handling:**
- Whole decode chain fails → `classify_thumbnail_failure` classifies precisely:
  genuinely undecodable format = `permanent`; ffmpeg missing / environment issue =
  a distinct diagnosable error (not mis-flagged permanent).
- Frontend: a neighbor that fails to pre-decode is not pooled; navigating to it
  falls back to the downscaled tier or a "load failed" placeholder, without
  blocking continuous nav.
- ImageBitmap leak guard: pool/preview/library-switch `.close()` lifecycle is
  test-covered.

**Testing:**
- **Rust:** decoder-chain unit tests (each decoder's handles/decode + fall-through
  on failure); the ffmpeg decoder against real small samples (AVIF/HEIC/TIFF)
  producing an image; the updated `classify_thumbnail_failure` classification;
  preview-tier profile selection (viewport → profile) + on-demand generation +
  cache; `get_preview` source-resolution routing.
- **Web:** pre-decode pool pure logic (LRU / budget / `.close()` lifecycle)
  extracted as a unit-testable module; the no-lock placeholder-swap decision as a
  pure function; canvas draw transforms (scale/fit/rotate matrix) as pure
  functions.
- **Real-app verification:** continuous nav through
  `C:\Users\84460\Pictures\normal` drops from ~8s toward BandiView's ~2s;
  zoom past the preview tier pulls the original on demand; PSD/HEIC/TIFF previews
  are viewable; memory does not leak (ImageBitmap count stabilizes after
  hundreds of continuous nav steps).

## 8. Affected files (map)

- Create: `crates/core/src/decode/mod.rs` (+ `image_crate.rs`, `ffmpeg.rs`,
  registry) — decoder abstraction + chain.
- Modify: `crates/core/src/thumbnails/mod.rs` — thumbnail generation via registry;
  `classify_thumbnail_failure` update.
- Modify: `crates/core/src/api/routes.rs` — `get_preview` source resolution +
  transcode fallback + `viewportShortPx` param.
- Modify: `crates/core/src/scan/mod.rs` + `crates/core/src/watch/mod.rs` —
  `media_kind` extension lists (add `.heif`; keep both copies in sync).
- New preview-tier profile constants + on-demand generation (thumbnails module +
  blob store).
- Create: `apps/web/src/features/preview/decodeWorker.ts` (Worker),
  `previewBitmapPool.ts` (pool logic), canvas render + transform module.
- Modify: `apps/web/src/features/preview/MediaPreview.tsx`,
  `CentralPreviewStage.tsx` — canvas render + type dispatch; remove `<img key=>`
  re-decode path.
- Modify: `apps/web/src/app/App.tsx` — prefetch effect drives the pool (remove the
  ineffective decode-ahead + no-abort experiment); nav-lock removal.
- Modify: `apps/web/src/core/mediaResources.ts` — remove the ineffective
  `decodedObjectUrl` decode-ahead code; keep byte cache + epoch; extend the
  library-switch reset to close pooled bitmaps.
- Contract: preview endpoint gains `viewportShortPx`; new preview profile values
  reflected in schema/contract checks as needed.
