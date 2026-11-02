# Mangoo Advanced Media Plan

> **For agentic workers:** Implement task-by-task. Steps use `- [ ]` checkboxes.

**Goal:** Real thumbnail and metadata extraction for images and videos so the existing grid/preview/inspector flow handles real files instead of placeholder WebP. The same UX must keep working — only the decoding pipeline changes.

**Architecture:** Replace the placeholder `write_placeholder_thumbnail` path with a real image decoder + resizer for common formats and an FFmpeg sidecar for video poster frames. Populate `media.width/height/duration_ms/codec` during scan or as a follow-up task. Graceful degradation: unsupported or failing formats fall through to the existing `failed` thumbnail state with a clear error.

**Tech Stack:** Rust + image crate (`image = "0.25"`, `webp = "0.3"`), FFmpeg (system binary, invoked via `tokio::process::Command`). React unchanged except for any small UI tweaks for new metadata fields.

**Scope cuts (deferred):**
- libvips (Rust bindings churn; image crate covers JPG/PNG/WebP/GIF/BMP for Phase 7).
- HEIC/RAW/PSD/AVIF (need libheif/dcraw/libraw/libavif; out — Phase 8 or later).
- Video proxy generation (out — phase 9+).
- exif metadata extraction beyond width/height (out — handled by existing `media.metadata_status` field; can be filled later by a metadata task).
- Multiple thumbnail profiles beyond grid_320 (out — schema already constrains profile to grid_320).

---

## File Structure

**Modify (backend):**

- `D:/Mangoo/crates/core/Cargo.toml` (add `image = "0.25"`, `webp = "0.3"`).
- `D:/Mangoo/crates/core/src/thumbnails/mod.rs` (real decoder + resizer + WebP encoder; replaces placeholder).
- `D:/Mangoo/crates/core/src/scan/mod.rs` or a new `crates/core/src/media/mod.rs` for image-dimension probing on scan (decide based on what stays small).
- `D:/Mangoo/crates/core/src/tasks.rs` (drive ffmpeg poster path for video task kind; fall through to failed state on missing ffmpeg).

**Create (backend):**

- `D:/Mangoo/crates/core/src/thumbnails/image_decoder.rs` (only if `mod.rs` becomes too long).
- `D:/Mangoo/crates/core/src/thumbnails/video_poster.rs` (FFmpeg invocation; reusable from tasks).

**Frontend:** No new files for Phase 7. Possibly minor PreviewPanel tweak to show duration_ms / codec when populated.

---

## Contract surface

No new endpoints. `MediaRecord` already exposes `kind/width/height/durationMs/codec`. `ThumbnailResponse.asset.{width,height,byteSize}` already exists. The contract stays the same; only the values change from "placeholder 320x320 minimal WebP" to "real resized WebP with the source's aspect ratio".

If the existing thumbnail task pipeline conflates "poster frame for video" with "image thumbnail" without distinguishing them, that's fine — the worker will branch internally on `media.kind`.

---

## Task 1 — Real image thumbnails

- [ ] Add `image = "0.25"` and `webp = "0.3"` to Cargo. Verify they compile on Windows.
- [ ] Replace `write_placeholder_thumbnail`:
  - decode source via `image::open` (handles JPG/PNG/WebP/GIF/BMP/TIFF if features enabled).
  - if short side < 320 px, return `ThumbnailDecision::SkippedSmall` (keep existing semantics).
  - else, resize so short side = 320 with `image::imageops::resize(.., FilterType::Triangle)`.
  - encode as WebP via `webp::Encoder::from_image(&image).encode(75.0)` and write to cache file.
  - return `GeneratedThumbnail { width, height, byte_size }`.
- [ ] On decode failure (corrupt file, unsupported codec via image crate), return an `Err` whose message starts with `"thumbnail decode failed:"` so the existing error handler routes it to `failed` state with a useful message.
- [ ] Update existing tests to expect real WebP bytes (not placeholder). Where tests assert specific dimensions, accept either the existing 320x320 (for square sources) or the actual resized output.

## Task 2 — Image dimensions on scan

- [ ] During scan, after upserting a media file, attempt a quick header-only probe of width/height. Use `image::ImageReader::open(path)?.into_dimensions()` which doesn't decode pixel data.
- [ ] On success, update `media.width/height` and set `metadata_status = 'ready'`. On failure, leave defaults and `metadata_status = 'pending'` so a later metadata task can retry.
- [ ] Add a Rust test: scanning a 800x600 JPG sets media.width=800, height=600.

## Task 3 — Video poster frames via FFmpeg

- [ ] Detect `ffmpeg` on PATH at Core startup. If missing, log a warn and route all video thumbnail tasks to `failed` with "ffmpeg not available" so UI shows a clear state.
- [ ] When the worker picks up a thumbnail task for `media.kind == 'video'`:
  1. Spawn `ffmpeg -ss 00:00:01.0 -i <source> -frames:v 1 -vf "scale='if(gt(a,1),320,-2)':'if(gt(a,1),-2,320)'" -f image2 -codec:v webp -y <cache_path>` (use `tokio::process::Command`).
  2. On exit code 0, read back the WebP, get dimensions via `image::ImageReader`, write thumbnail row.
  3. On non-zero exit, capture stderr, return error.
- [ ] Add test: skipped if ffmpeg not on PATH; only verify the worker doesn't crash and writes a `failed` state with the error message.

## Task 4 — Verification + commit

- [ ] `cargo test -p mangoo-core` (existing 130 tests + new ones).
- [ ] `npm test`.
- [ ] Update `CLAUDE.md` snippet, master plan checkboxes.
- [ ] Commit `feat: add real image and video thumbnails (phase 7)`.

---

## Acceptance

- A user adds a root containing JPG + PNG + MP4. Grid shows actual scaled previews. Preview panel shows actual dimensions (e.g. 800 × 600). Video items show a poster frame.
- Corrupt files surface a clean `failed` state with the error visible in the inspector / task center.
- ffmpeg-missing systems still load images and only fail video poster generation.
