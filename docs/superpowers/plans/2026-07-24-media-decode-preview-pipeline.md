# Media Decode & Preview Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make prev/next preview navigation of large images near-instant (BandiView-class, ~8s → ~2s over the 31-image `C:\Users\84460\Pictures\normal` folder) and make AVIF/HEIC/TIFF/PSD/RAW previewable, by decoding **once, ahead of time, off the main thread, into a paint-ready form** and by putting a pluggable decoder chain behind every Core decode.

**Architecture:** Three units. (1) A Rust `DecoderRegistry` responsibility chain (`crates/core/src/decode/`) that all thumbnail and preview decoding flows through — `image` crate first, ffmpeg subprocess second, format-specific crates later. (2) `GET /api/media/{fileId}/preview` becomes a **preview-source resolver**: browser-renderable originals ≤4096px still stream raw bytes byte-for-byte as today; larger ones return a discretized, on-demand, cached downscaled WebP tier from a **new `preview_blobs` table**; browser-unrenderable images transcode to WebP through the registry. (3) The React preview surface paints a **`<canvas>` from a pre-decoded `ImageBitmap` pool** filled by a Web Worker, so navigating to a pooled neighbor is a synchronous `drawImage` instead of a fresh full-resolution main-thread `<img>` decode.

**Tech Stack:** Rust + axum + rusqlite + `image` 0.25 + `webp` 0.3 + ffmpeg subprocess; React 18 + TypeScript + Vite; SQLite WAL; `node --test` for web units, `cargo test` for Rust.

## Global Constraints

- **Decoders always output RGBA.** Scaling and WebP encoding stay in the layer above the registry (`DynamicImage::thumbnail_exact` + `webp::Encoder`), exactly as today. A decoder's single responsibility is decode.
- **No new C-crate compilation.** `dav1d`/`libheif` Rust bindings need cmake+nasm, which are not available on this machine. AVIF/HEIC/HEVC support comes from the **already-installed ffmpeg binary** invoked as a subprocess, reusing the existing `wait_with_timeout` / `Stdio::null()` stdin / stderr-drain pattern from `generate_video_thumbnail`.
- **Never rebuild `thumb_blobs`.** The real library has 663k rows / **17.8 GB** in that table; widening its `CHECK(profile = 'grid_320')` would copy 18 GB on app open. The preview tier goes in a **new, empty `preview_blobs` table**.
- **Never rewrite the zoom/pan/rotate gesture state machine.** Zoom is a CSS transform on the wrapper div `.central-preview-transform` (`CentralPreviewStage.tsx:697`), *not* on the `<img>`. Swapping the inner element for `<canvas>` leaves every gesture untouched.
- **Canvas backing store = the decoded bitmap's size; canvas CSS box = the media's natural size.** A 16384×8192 backing store would be 536 MB of GPU memory. Sizing the CSS box to natural pixels keeps `.central-preview-transform`'s math byte-identical to the `<img>` it replaces.
- **`ImageBitmap` must be explicitly `.close()`d.** Every eviction, preview close, and library switch closes its bitmaps or GPU memory leaks.
- **Pool budget counts DECODED bytes (`width * height * 4`), not file bytes.** The existing `originalPreviewBlobCache` budget counts compressed bytes; a bitmap pool using that number would over-commit by 10–50×.
- **The Worker must be a same-origin module Worker.** `apps/web/vite.config.ts` sets a CSP with no `worker-src`, so it falls back to `default-src 'self'`: `new Worker(new URL("./x.ts", import.meta.url), { type: "module" })` is allowed, a `blob:` Worker is **blocked**.
- **No raw `fetch(` anywhere under `apps/web/src`.** `tools/checks/validate-web-client.mjs:2122` fails the build on `/\bfetch\s*\(/`. The Worker receives `Blob`s over `postMessage`; it never fetches.
- **Web tests are plain `node --test`, no DOM.** `apps/web/package.json` runs `node --experimental-strip-types --test tests/*.test.mts`. Testable logic must live in pure modules with no React and no unstubbed DOM globals; follow `apps/web/tests/mediaResources.test.mts` for the `register("./resolve-source-imports.mjs", import.meta.url)` + global-stub pattern.
- **Every diagnostic goes behind the existing single debug switch.** Rust: `crate::debug_log!` / `debug::enabled()` (`MANGOO_DEBUG=1`). Do not add ad-hoc `eprintln!`.
- **Do not disturb the user's uncommitted working-tree changes.** Every `git add` stages explicit paths only — never `git add -A` / `git add .`.
- **Full gate before declaring done:** `npm test` (structure, core-api, core-client, notification-contract, desktop, web, ui-design, schema, rust). Rust needs `export PATH="$HOME/.cargo/bin:$PATH"`.

---

## File Structure

**Rust Core — create:**
- `crates/core/src/decode/mod.rs` — `DecodePurpose`, `MediaSource`, `DecodeRequest`, `DecodedImage`, `MediaDecoder` trait, `DecoderRegistry` (chain + fall-through), `default_registry()`. Owns the *contract*, not any decoding.
- `crates/core/src/decode/image_crate.rs` — `ImageCrateDecoder`. Wraps today's `ImageReader::open().with_guessed_format().decode()`.
- `crates/core/src/decode/ffmpeg.rs` — `FfmpegDecoder`. One-frame decode to `rawvideo rgba` on stdout.
- `crates/core/migrations/0025_preview_blobs.sql` — new `preview_blobs` table + index + self-registration.

**Rust Core — modify:**
- `crates/core/src/main.rs` — add `mod decode;`.
- `crates/core/src/db/migrations.rs` — declare `PREVIEW_BLOBS_MIGRATION`.
- `crates/core/src/db/mod.rs` — gate migration 25; `PreviewBlobRecord`, `get_preview_blob`, `upsert_preview_blob`; drop preview blobs alongside thumb blobs on reset/purge.
- `crates/core/src/thumbnails/mod.rs` — preview-tier profile constants + selection; `generate_image_thumbnail_bytes_with_checkpoint` and `generate_preview_tier_bytes` go through the registry; `classify_thumbnail_failure` gains the chain-failure and decoder-unavailable shapes.
- `crates/core/src/api/routes.rs` — `get_preview` becomes the source resolver (raw-stream branch preserved verbatim); new `PreviewQuery`; `x-mangoo-preview-tier` response header.
- `crates/core/src/scan/mod.rs` + `crates/core/src/watch/mod.rs` — the two duplicated `media_kind` extension lists gain `.heif` (and stay identical to each other).
- `tools/checks/validate_sqlite_schema.py`, `tools/checks/validate-core-api.mjs`, `packages/core-client/scripts/check-contract.mjs`, `contracts/core-api/openapi.yaml`, `packages/core-client/src/client.ts` — contract + checks.

**Web — create:**
- `apps/web/src/features/preview/previewBitmapPool.ts` — pure LRU pool over decoded bitmaps. No DOM beyond the `ImageBitmap`-shaped `{width, height, close()}` structural type, so `node --test` can drive it with a fake.
- `apps/web/src/features/preview/previewDecodeWorker.ts` — the module Worker. `createImageBitmap` only.
- `apps/web/src/features/preview/previewBitmapSource.ts` — bytes → Worker → pool orchestration, epoch-scoped like `mediaResources.ts`.

**Web — modify:**
- `apps/web/src/features/preview/MediaPreview.tsx` — static-image lane paints a stable `<canvas>`; animated images keep `<img>`; video keeps `<video>`.
- `apps/web/src/app/App.tsx` — prefetch drives the pool; navigation lock removed.
- `apps/web/src/core/mediaResources.ts` — `resetMediaCachesForLibrarySwitch` also drains the bitmap pool.
- `apps/web/tests/*.test.mts` — new unit tests.

---

### Task 1: Decoder registry contract and fall-through chain

**Files:**
- Create: `crates/core/src/decode/mod.rs`
- Modify: `crates/core/src/main.rs:1-11` (add `mod decode;`)
- Test: inline `#[cfg(test)] mod tests` at the bottom of `crates/core/src/decode/mod.rs`

**Interfaces:**
- Consumes: nothing (first task).
- Produces: `decode::DecodePurpose{Thumbnail,Preview}`, `decode::MediaSource<'a>::Path(&'a Path)` with `.path() -> &'a Path`, `decode::DecodeRequest<'a>{source, ext: &'a str, target_max_px: Option<u32>, purpose}`, `decode::DecodedImage{rgba: Vec<u8>, width: u32, height: u32}` with `.into_dynamic() -> anyhow::Result<image::DynamicImage>`, `trait decode::MediaDecoder { fn name(&self) -> &'static str; fn handles(&self, ext: &str, purpose: DecodePurpose) -> bool; fn decode(&self, request: &DecodeRequest<'_>) -> anyhow::Result<DecodedImage>; }`, `decode::DecoderRegistry::new(Vec<Box<dyn MediaDecoder>>)` + `.decode(&DecodeRequest) -> anyhow::Result<DecodedImage>`, and the error markers `decode::NO_DECODER_MARKER` / `decode::DECODER_UNAVAILABLE_MARKER`.

`ext` is always **lowercase and includes the dot** (`".heic"`), matching what `scan::extension` already produces.

- [ ] **Step 1: Write the failing tests**

Create `crates/core/src/decode/mod.rs` containing ONLY the test module for now (the compiler errors are the failing test):

```rust
#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct FakeDecoder {
        name: &'static str,
        claims: &'static [&'static str],
        result: fn() -> anyhow::Result<DecodedImage>,
        calls: &'static AtomicUsize,
    }

    impl MediaDecoder for FakeDecoder {
        fn name(&self) -> &'static str {
            self.name
        }

        fn handles(&self, ext: &str, _purpose: DecodePurpose) -> bool {
            self.claims.contains(&ext)
        }

        fn decode(&self, _request: &DecodeRequest<'_>) -> anyhow::Result<DecodedImage> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            (self.result)()
        }
    }

    fn one_red_pixel() -> anyhow::Result<DecodedImage> {
        Ok(DecodedImage {
            rgba: vec![255, 0, 0, 255],
            width: 1,
            height: 1,
        })
    }

    fn request<'a>(path: &'a Path, ext: &'a str) -> DecodeRequest<'a> {
        DecodeRequest {
            source: MediaSource::Path(path),
            ext,
            target_max_px: None,
            purpose: DecodePurpose::Thumbnail,
        }
    }

    #[test]
    fn registry_falls_through_to_the_next_handling_decoder() {
        static FIRST: AtomicUsize = AtomicUsize::new(0);
        static SECOND: AtomicUsize = AtomicUsize::new(0);
        let registry = DecoderRegistry::new(vec![
            Box::new(FakeDecoder {
                name: "first",
                claims: &[".tif"],
                result: || Err(anyhow::anyhow!("exotic tiff variant")),
                calls: &FIRST,
            }),
            Box::new(FakeDecoder {
                name: "second",
                claims: &[".tif"],
                result: one_red_pixel,
                calls: &SECOND,
            }),
        ]);

        let decoded = registry
            .decode(&request(Path::new("a.tif"), ".tif"))
            .expect("second decoder should rescue the first's failure");

        assert_eq!(decoded.width, 1);
        assert_eq!(FIRST.load(Ordering::Relaxed), 1);
        assert_eq!(SECOND.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn registry_skips_decoders_that_do_not_claim_the_extension() {
        static SKIPPED: AtomicUsize = AtomicUsize::new(0);
        static USED: AtomicUsize = AtomicUsize::new(0);
        let registry = DecoderRegistry::new(vec![
            Box::new(FakeDecoder {
                name: "skipped",
                claims: &[".png"],
                result: one_red_pixel,
                calls: &SKIPPED,
            }),
            Box::new(FakeDecoder {
                name: "used",
                claims: &[".heic"],
                result: one_red_pixel,
                calls: &USED,
            }),
        ]);

        registry
            .decode(&request(Path::new("a.heic"), ".heic"))
            .expect("claiming decoder should run");

        assert_eq!(SKIPPED.load(Ordering::Relaxed), 0);
        assert_eq!(USED.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn registry_preserves_every_failure_text_when_the_whole_chain_fails() {
        static FIRST: AtomicUsize = AtomicUsize::new(0);
        static SECOND: AtomicUsize = AtomicUsize::new(0);
        let registry = DecoderRegistry::new(vec![
            Box::new(FakeDecoder {
                name: "image_crate",
                claims: &[".tif"],
                result: || Err(anyhow::anyhow!("unsupported compression")),
                calls: &FIRST,
            }),
            Box::new(FakeDecoder {
                name: "ffmpeg",
                claims: &[".tif"],
                result: || Err(anyhow::anyhow!("找不到网络路径。 (os error 53)")),
                calls: &SECOND,
            }),
        ]);

        let error = registry
            .decode(&request(Path::new("a.tif"), ".tif"))
            .expect_err("all decoders failed");
        let text = error.to_string();

        // Both names and both messages survive, so `classify_thumbnail_failure`
        // still sees the transient signature contributed by the ffmpeg link.
        assert!(text.contains("image_crate"), "{text}");
        assert!(text.contains("unsupported compression"), "{text}");
        assert!(text.contains("ffmpeg"), "{text}");
        assert!(text.contains("os error 53"), "{text}");
    }

    #[test]
    fn registry_reports_no_decoder_when_nothing_claims_the_extension() {
        let registry = DecoderRegistry::new(vec![]);

        let error = registry
            .decode(&request(Path::new("a.psd"), ".psd"))
            .expect_err("empty chain cannot decode");

        assert!(error.to_string().contains(NO_DECODER_MARKER), "{error}");
        assert!(error.to_string().contains(".psd"), "{error}");
    }

    #[test]
    fn registry_treats_a_buffer_dimension_mismatch_as_a_decoder_failure() {
        static BAD: AtomicUsize = AtomicUsize::new(0);
        static GOOD: AtomicUsize = AtomicUsize::new(0);
        let registry = DecoderRegistry::new(vec![
            Box::new(FakeDecoder {
                name: "bad",
                claims: &[".png"],
                // Claims 4x4 but returns one pixel: a contract violation that
                // must not reach the caller as a valid image.
                result: || {
                    Ok(DecodedImage {
                        rgba: vec![1, 2, 3, 4],
                        width: 4,
                        height: 4,
                    })
                },
                calls: &BAD,
            }),
            Box::new(FakeDecoder {
                name: "good",
                claims: &[".png"],
                result: one_red_pixel,
                calls: &GOOD,
            }),
        ]);

        let decoded = registry
            .decode(&request(Path::new("a.png"), ".png"))
            .expect("chain should recover from the malformed decoder");

        assert_eq!(decoded.width, 1);
        assert_eq!(GOOD.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn decoded_image_converts_into_a_dynamic_image() {
        let dynamic = DecodedImage {
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
            width: 2,
            height: 1,
        }
        .into_dynamic()
        .expect("valid rgba buffer");

        assert_eq!(dynamic.width(), 2);
        assert_eq!(dynamic.height(), 1);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core decode:: 2>&1 | tail -20
```

Expected: FAIL — `error[E0433]`/`cannot find type DecodedImage`, `cannot find trait MediaDecoder`, and `file not found for module decode` until `mod decode;` is added.

- [ ] **Step 3: Write the implementation**

Add `mod decode;` to `crates/core/src/main.rs` in alphabetical position (after `mod db;`, before `mod debug;`):

```rust
mod api;
mod db;
mod decode;
mod debug;
```

Prepend the implementation above the test module in `crates/core/src/decode/mod.rs`:

```rust
//! Pluggable media decode chain.
//!
//! Every Core decode (thumbnail generation, preview transcode) asks this
//! registry for RGBA pixels instead of calling `image::open` directly, so
//! adding a format family is adding one decoder to the chain and nothing else.
//! The chain is a responsibility chain with **graceful fall-through**: a
//! decoder that claims an extension but fails does not end the attempt, the
//! next claiming decoder runs. That is what lets `image` lose on an exotic
//! TIFF variant and ffmpeg still produce a picture.

use std::fmt::Write as _;
use std::path::Path;

/// Why the caller wants pixels. A decoder may claim an extension for one
/// purpose and not the other (a video decoder produces a `Thumbnail` poster
/// but is not a `Preview` source — video preview streams the original).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodePurpose {
    Thumbnail,
    Preview,
}

/// Where the bytes live. A real file path today; `ArchiveEntry` / `Stream`
/// variants land here later so archive-internal media reuses the whole chain
/// without touching a single decoder.
#[derive(Debug, Clone, Copy)]
pub enum MediaSource<'a> {
    Path(&'a Path),
}

impl<'a> MediaSource<'a> {
    pub fn path(&self) -> &'a Path {
        match self {
            MediaSource::Path(path) => path,
        }
    }
}

pub struct DecodeRequest<'a> {
    pub source: MediaSource<'a>,
    /// Lowercase extension INCLUDING the dot, e.g. ".heic"; empty when the file
    /// has none. Matches what `scan::extension` already produces.
    pub ext: &'a str,
    /// Longest-side cap the caller can tolerate. A decoder MAY use it to decode
    /// more cheaply (ffmpeg scales in-process) and MAY ignore it entirely — the
    /// layer above always re-checks the returned dimensions and downscales.
    pub target_max_px: Option<u32>,
    pub purpose: DecodePurpose,
}

/// The one product type every decoder returns: RGBA8, tightly packed, with
/// `rgba.len() == width * height * 4`. Scaling and encoding are the caller's
/// job, so a decoder's single responsibility stays "decode".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl DecodedImage {
    fn expected_len(&self) -> Option<usize> {
        (self.width as usize)
            .checked_mul(self.height as usize)?
            .checked_mul(4)
    }

    pub fn into_dynamic(self) -> anyhow::Result<image::DynamicImage> {
        let (width, height) = (self.width, self.height);
        let buffer = image::RgbaImage::from_raw(width, height, self.rgba).ok_or_else(|| {
            anyhow::anyhow!("decoded rgba buffer does not match {width}x{height}")
        })?;
        Ok(image::DynamicImage::ImageRgba8(buffer))
    }
}

pub trait MediaDecoder: Send + Sync {
    fn name(&self) -> &'static str;
    fn handles(&self, ext: &str, purpose: DecodePurpose) -> bool;
    fn decode(&self, request: &DecodeRequest<'_>) -> anyhow::Result<DecodedImage>;
}

/// Embedded in the error when a decoder that *would* handle the format cannot
/// run because its external dependency is missing. Kept lexically distinct from
/// [`NO_DECODER_MARKER`] so diagnostics can say "install ffmpeg" instead of
/// "unsupported format".
pub const DECODER_UNAVAILABLE_MARKER: &str = "decoder unavailable";
/// Embedded in the error when no registered decoder claims the format at all.
pub const NO_DECODER_MARKER: &str = "no decoder handles";

pub struct DecoderRegistry {
    decoders: Vec<Box<dyn MediaDecoder>>,
}

impl DecoderRegistry {
    pub fn new(decoders: Vec<Box<dyn MediaDecoder>>) -> Self {
        Self { decoders }
    }

    pub fn decode(&self, request: &DecodeRequest<'_>) -> anyhow::Result<DecodedImage> {
        let mut attempts = 0usize;
        let mut report = String::new();
        for decoder in &self.decoders {
            if !decoder.handles(request.ext, request.purpose) {
                continue;
            }
            attempts += 1;
            let outcome = decoder.decode(request).and_then(|decoded| {
                match decoded.expected_len() {
                    Some(expected) if expected == decoded.rgba.len() => Ok(decoded),
                    _ => Err(anyhow::anyhow!(
                        "decoder returned {} bytes for {}x{}",
                        decoded.rgba.len(),
                        decoded.width,
                        decoded.height
                    )),
                }
            });
            match outcome {
                Ok(decoded) => return Ok(decoded),
                Err(error) => {
                    // Fall through to the next claiming decoder. Every failure
                    // text is preserved verbatim in the aggregate error so
                    // `classify_thumbnail_failure` still sees a transient
                    // signature contributed by ANY link in the chain.
                    if !report.is_empty() {
                        report.push_str("; ");
                    }
                    let _ = write!(report, "{}: {error}", decoder.name());
                }
            }
        }
        if attempts == 0 {
            return Err(anyhow::anyhow!(
                "{NO_DECODER_MARKER} \"{}\"",
                request.ext
            ));
        }
        Err(anyhow::anyhow!(
            "all decoders failed for \"{}\": {report}",
            request.ext
        ))
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core decode:: 2>&1 | tail -20
cargo fmt --all --check
```

Expected: 6 passed, `cargo fmt` silent.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/decode/mod.rs crates/core/src/main.rs
git commit -m "feat(core): add pluggable media decoder registry"
```

---

### Task 2: `ImageCrateDecoder` — the fast default link

**Files:**
- Create: `crates/core/src/decode/image_crate.rs`
- Modify: `crates/core/src/decode/mod.rs` (add `pub mod image_crate;` at the top, below the module doc)
- Test: inline `#[cfg(test)] mod tests` at the bottom of `crates/core/src/decode/image_crate.rs`

**Interfaces:**
- Consumes: `super::{DecodePurpose, DecodeRequest, DecodedImage, MediaDecoder, MediaSource}` from Task 1.
- Produces: `decode::image_crate::ImageCrateDecoder` (unit struct) implementing `MediaDecoder` with `name() == "image_crate"`.

**Why the extension list excludes AVIF/HEIC/PSD/RAW:** `image` 0.25 with default features *recognises* those extensions but has no working decoder for them (AVIF decode needs `dav1d` — C, cmake+nasm, unavailable here). Claiming them would insert a guaranteed-failing link ahead of ffmpeg and produce noisier errors for zero benefit.

- [ ] **Step 1: Write the failing tests**

Create `crates/core/src/decode/image_crate.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn unique_temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "mangoo_decode_image_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// Writes a `width x height` image where every pixel is `rgb`, encoded by
    /// extension, and returns its path. The parent dir is the caller's to clean.
    fn write_image(dir: &std::path::Path, name: &str, width: u32, height: u32) -> PathBuf {
        std::fs::create_dir_all(dir).expect("create temp dir");
        let mut buffer = image::RgbaImage::new(width, height);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgba([12, 34, 56, 255]);
        }
        let path = dir.join(name);
        image::DynamicImage::ImageRgba8(buffer)
            .save(&path)
            .expect("encode fixture");
        path
    }

    #[test]
    fn claims_the_formats_the_image_crate_can_actually_decode() {
        let decoder = ImageCrateDecoder;
        for ext in [".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".tif", ".tiff"] {
            assert!(
                decoder.handles(ext, DecodePurpose::Thumbnail),
                "should claim {ext}"
            );
        }
    }

    #[test]
    fn does_not_claim_formats_it_has_no_working_decoder_for() {
        let decoder = ImageCrateDecoder;
        // Claiming these would add a guaranteed-failing link ahead of ffmpeg.
        for ext in [".avif", ".heic", ".heif", ".psd", ".raw", ".mp4", ""] {
            assert!(
                !decoder.handles(ext, DecodePurpose::Thumbnail),
                "should not claim {ext}"
            );
        }
    }

    #[test]
    fn decodes_png_to_tightly_packed_rgba() {
        let dir = unique_temp_dir();
        let path = write_image(&dir, "fixture.png", 6, 4);

        let decoded = ImageCrateDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(&path),
                ext: ".png",
                target_max_px: None,
                purpose: DecodePurpose::Thumbnail,
            })
            .expect("png decodes");

        assert_eq!((decoded.width, decoded.height), (6, 4));
        assert_eq!(decoded.rgba.len(), 6 * 4 * 4);
        assert_eq!(&decoded.rgba[0..4], &[12, 34, 56, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decodes_jpeg_by_sniffing_content_not_only_extension() {
        let dir = unique_temp_dir();
        let path = write_image(&dir, "fixture.jpg", 5, 5);

        let decoded = ImageCrateDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(&path),
                ext: ".jpg",
                target_max_px: None,
                purpose: DecodePurpose::Thumbnail,
            })
            .expect("jpeg decodes");

        assert_eq!((decoded.width, decoded.height), (5, 5));
        assert_eq!(decoded.rgba.len(), 5 * 5 * 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reports_a_readable_error_for_a_missing_file() {
        let error = ImageCrateDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(std::path::Path::new("does-not-exist.png")),
                ext: ".png",
                target_max_px: None,
                purpose: DecodePurpose::Thumbnail,
            })
            .expect_err("missing file cannot decode");

        assert!(error.to_string().contains("image_crate"), "{error}");
    }

    #[test]
    fn rejects_a_zero_sized_decode() {
        // A 0-width source would divide by zero downstream in the resizer.
        assert!(super::rejects_zero_dimensions(0, 10).is_err());
        assert!(super::rejects_zero_dimensions(10, 0).is_err());
        assert!(super::rejects_zero_dimensions(10, 10).is_ok());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core decode::image_crate 2>&1 | tail -20
```

Expected: FAIL — `cannot find struct ImageCrateDecoder`, `cannot find function rejects_zero_dimensions`, and `file not found for module image_crate`.

- [ ] **Step 3: Write the implementation**

Add to the top of `crates/core/src/decode/mod.rs`, immediately below the `//!` doc comment:

```rust
pub mod image_crate;
```

Prepend to `crates/core/src/decode/image_crate.rs`:

```rust
//! The default, fastest link in the chain: the `image` crate with its default
//! features. Registered FIRST so JPEG/PNG/GIF/WebP/BMP/common-TIFF — the
//! overwhelming majority of any real library — never pay a subprocess spawn.

use image::ImageReader;

use super::{DecodePurpose, DecodeRequest, DecodedImage, MediaDecoder};

/// Extensions `image` 0.25 with DEFAULT features can actually decode.
///
/// AVIF/HEIC/HEIF/PSD/RAW are deliberately absent: the crate recognises some of
/// those extensions but has no working decoder (AVIF decode needs `dav1d`, a C
/// dependency requiring cmake+nasm that this project explicitly avoids), so
/// claiming them would only add a guaranteed-failing link ahead of ffmpeg.
const SUPPORTED_EXTENSIONS: &[&str] = &[
    ".bmp", ".gif", ".ico", ".jpeg", ".jpg", ".png", ".qoi", ".tga", ".tif", ".tiff", ".webp",
];

pub struct ImageCrateDecoder;

fn rejects_zero_dimensions(width: u32, height: u32) -> anyhow::Result<()> {
    if width == 0 || height == 0 {
        return Err(anyhow::anyhow!("zero-sized source image"));
    }
    Ok(())
}

impl MediaDecoder for ImageCrateDecoder {
    fn name(&self) -> &'static str {
        "image_crate"
    }

    fn handles(&self, ext: &str, _purpose: DecodePurpose) -> bool {
        SUPPORTED_EXTENSIONS.contains(&ext)
    }

    fn decode(&self, request: &DecodeRequest<'_>) -> anyhow::Result<DecodedImage> {
        let path = request.source.path();
        // `with_guessed_format` sniffs the content, so a mis-extensioned file
        // still decodes — the extension only decides WHICH decoder is asked.
        let reader = ImageReader::open(path)
            .map_err(|error| anyhow::anyhow!("image_crate open: {error}"))?
            .with_guessed_format()
            .map_err(|error| anyhow::anyhow!("image_crate format: {error}"))?;
        let decoded = reader
            .decode()
            .map_err(|error| anyhow::anyhow!("image_crate decode: {error}"))?;
        rejects_zero_dimensions(decoded.width(), decoded.height())
            .map_err(|error| anyhow::anyhow!("image_crate decode: {error}"))?;

        let (width, height) = (decoded.width(), decoded.height());
        // `to_rgba8` is the one normalisation every consumer relies on: it
        // flattens paletted, grayscale, and 16-bit decodes into the single
        // product type, which is also exactly what `webp::Encoder` accepts.
        Ok(DecodedImage {
            rgba: decoded.to_rgba8().into_raw(),
            width,
            height,
        })
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core decode:: 2>&1 | tail -20
cargo fmt --all --check
```

Expected: 12 passed (6 from Task 1 + 6 here), `cargo fmt` silent.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/decode/mod.rs crates/core/src/decode/image_crate.rs
git commit -m "feat(core): add image-crate decoder to the decode chain"
```

---

### Task 3: `FfmpegDecoder` — AVIF / HEIC / HEIF / PSD / exotic TIFF

**Files:**
- Create: `crates/core/src/decode/subprocess.rs`
- Create: `crates/core/src/decode/ffmpeg.rs`
- Modify: `crates/core/src/decode/mod.rs` (add `pub mod ffmpeg;` + `pub mod subprocess;`, add `default_registry()` / `shared_registry()`)
- Modify: `crates/core/src/thumbnails/mod.rs:17-45` (delete the moved helpers, import them instead) and `:471-478` (delete `ffmpeg_available`, re-export it)
- Test: inline `#[cfg(test)] mod tests` at the bottom of `crates/core/src/decode/ffmpeg.rs`

**Interfaces:**
- Consumes: Task 1's `MediaDecoder`/`DecodeRequest`/`DecodedImage`/`DECODER_UNAVAILABLE_MARKER`; Task 2's `ImageCrateDecoder`.
- Produces: `decode::subprocess::{ChildWaitOutcome, wait_with_timeout}`, `decode::ffmpeg::{FfmpegDecoder, ffmpeg_available, ffmpeg_scale_filter}`, `decode::default_registry() -> DecoderRegistry`, `decode::shared_registry() -> &'static DecoderRegistry`, and `thumbnails::ffmpeg_available` continues to resolve (re-export) so `tasks.rs:207` is untouched.

**Three decisions that matter, and why:**

1. **Write the frame to a temp PNG, do not pipe it on stdout.** `wait_with_timeout`'s own doc warns that a piped stdout must be drained *before* the child exits. A multi-megabyte frame fills the 64 KB pipe buffer and deadlocks ffmpeg until the timeout fires. `generate_video_thumbnail` already solves this by writing to a file and reading it back; reuse that exact shape rather than inventing a reader thread.
2. **PNG, not `rawvideo`.** `rawvideo` output carries no dimensions, so reading it back would need a second `ffprobe` subprocess. PNG is self-describing, keeps alpha unambiguously (HEIC/AVIF can be transparent), and the `image` crate decodes it. The encode cost is paid once and the result is cached.
3. **`handles()` is true even when ffmpeg is missing; `decode()` returns the distinct `DECODER_UNAVAILABLE_MARKER` error.** `handles()` must stay pure and cheap, and the spec requires a missing ffmpeg to be diagnosable as "install ffmpeg" rather than mis-reported as "unsupported format". It still classifies as `permanent` (Task 4) so an uninstalled ffmpeg cannot turn into a 60s retry storm across every AVIF in the library — matching how `FFMPEG_NOT_AVAILABLE_ERROR` already behaves for video.

**Scope note:** the decoder claims **images only**. Video thumbnails keep their existing dedicated `generate_video_thumbnail_bytes` path (which already works and produces WebP directly); routing video through the RGBA registry would be churn with no user-visible gain.

- [ ] **Step 1: Write the failing tests**

Create `crates/core/src/decode/ffmpeg.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::decode::{DecodePurpose, DecodeRequest, MediaSource, DECODER_UNAVAILABLE_MARKER};

    fn unique_temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "mangoo_decode_ffmpeg_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn claims_the_browser_and_image_crate_gaps() {
        let decoder = FfmpegDecoder;
        for ext in [".avif", ".heic", ".heif", ".psd", ".tif", ".tiff"] {
            assert!(
                decoder.handles(ext, DecodePurpose::Thumbnail),
                "should claim {ext}"
            );
            assert!(
                decoder.handles(ext, DecodePurpose::Preview),
                "should claim {ext} for preview too"
            );
        }
    }

    #[test]
    fn does_not_claim_video_or_unknown_extensions() {
        let decoder = FfmpegDecoder;
        // Video thumbnails keep their dedicated WebP-producing path.
        for ext in [".mp4", ".mkv", ".mov", ".webm", ".wmv", ".txt", ""] {
            assert!(
                !decoder.handles(ext, DecodePurpose::Thumbnail),
                "should not claim {ext}"
            );
        }
    }

    #[test]
    fn scale_filter_fits_inside_the_cap_without_upscaling() {
        let filter = ffmpeg_scale_filter(320);
        assert!(filter.contains("min(320,iw)"), "{filter}");
        assert!(filter.contains("min(320,ih)"), "{filter}");
        assert!(
            filter.contains("force_original_aspect_ratio=decrease"),
            "{filter}"
        );
    }

    #[test]
    fn missing_ffmpeg_is_reported_as_decoder_unavailable_not_unsupported_format() {
        // Drive the pure branch directly so the test result does not depend on
        // whether this machine happens to have ffmpeg installed.
        let error = unavailable_error();
        let text = error.to_string();
        assert!(text.contains(DECODER_UNAVAILABLE_MARKER), "{text}");
        assert!(text.contains("ffmpeg"), "{text}");
    }

    #[test]
    fn decodes_a_real_avif_through_ffmpeg() {
        if !ffmpeg_available() {
            eprintln!("SKIP decodes_a_real_avif_through_ffmpeg: ffmpeg not on PATH");
            return;
        }
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("fixture.avif");
        // Let ffmpeg itself author the fixture, so the test needs no binary
        // blob in the repo and exercises the same codec path end to end.
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=64x48:d=1",
                "-frames:v",
                "1",
                "-y",
            ])
            .arg(&source)
            .status()
            .expect("spawn ffmpeg to author the fixture");
        if !status.success() {
            eprintln!("SKIP decodes_a_real_avif_through_ffmpeg: this ffmpeg cannot encode avif");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let decoded = FfmpegDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(&source),
                ext: ".avif",
                target_max_px: None,
                purpose: DecodePurpose::Preview,
            })
            .expect("ffmpeg decodes avif");

        assert_eq!((decoded.width, decoded.height), (64, 48));
        assert_eq!(decoded.rgba.len(), 64 * 48 * 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn honours_the_target_max_px_cap() {
        if !ffmpeg_available() {
            eprintln!("SKIP honours_the_target_max_px_cap: ffmpeg not on PATH");
            return;
        }
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("wide.png");
        let mut buffer = image::RgbaImage::new(800, 400);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgba([9, 9, 9, 255]);
        }
        image::DynamicImage::ImageRgba8(buffer)
            .save(&source)
            .expect("write fixture");

        let decoded = FfmpegDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(&source),
                ext: ".tif", // any claimed extension; content is sniffed by ffmpeg
                target_max_px: Some(200),
                purpose: DecodePurpose::Preview,
            })
            .expect("ffmpeg decodes and scales");

        assert!(decoded.width <= 200, "width {}", decoded.width);
        assert!(decoded.height <= 200, "height {}", decoded.height);
        // Aspect ratio 2:1 preserved within a pixel of rounding.
        assert!(
            (decoded.width as i64 - decoded.height as i64 * 2).abs() <= 2,
            "{}x{}",
            decoded.width,
            decoded.height
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_source_fails_with_the_ffmpeg_stderr_text() {
        if !ffmpeg_available() {
            eprintln!("SKIP a_corrupt_source_fails_with_the_ffmpeg_stderr_text: no ffmpeg");
            return;
        }
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("garbage.heic");
        std::fs::write(&source, b"not an image at all").expect("write garbage");

        let error = FfmpegDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(&source),
                ext: ".heic",
                target_max_px: None,
                purpose: DecodePurpose::Preview,
            })
            .expect_err("garbage cannot decode");

        let text = error.to_string();
        assert!(text.contains("ffmpeg"), "{text}");
        assert!(!text.contains(DECODER_UNAVAILABLE_MARKER), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_registry_puts_the_cheap_decoder_first() {
        let registry = crate::decode::default_registry();
        assert_eq!(registry.decoder_names(), vec!["image_crate", "ffmpeg"]);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core decode::ffmpeg 2>&1 | tail -20
```

Expected: FAIL — `file not found for module ffmpeg`, `cannot find function ffmpeg_scale_filter`, `no method decoder_names`.

- [ ] **Step 3a: Move the generic subprocess helpers into `decode/subprocess.rs`**

Create `crates/core/src/decode/subprocess.rs` with the code **cut verbatim** from `crates/core/src/thumbnails/mod.rs:17-45` (only the visibility changes):

```rust
//! Dependency-free child-process supervision shared by every subprocess-backed
//! decoder. Kept out of any one decoder so the "never leak a zombie, never
//! block forever" discipline has exactly one implementation.

use std::process::Child;
use std::time::{Duration, Instant};

/// Polling cadence for [`wait_with_timeout`]. Deliberately dependency-free so
/// the worker doesn't pull in tokio/wait_timeout just to bound a subprocess.
const CHILD_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Outcome of [`wait_with_timeout`]. The exit-status case is mapped onto
/// `std::process::Output` by callers that piped streams.
pub enum ChildWaitOutcome {
    Exited(std::process::ExitStatus),
    TimedOut,
}

/// Polls `child.try_wait()` until it exits or `timeout` elapses. On timeout the
/// child is killed and reaped so we don't leak a zombie. Stdout/stderr are
/// *not* drained here — callers that piped them must read before the child
/// exits to avoid deadlocking on a full pipe.
pub fn wait_with_timeout(
    child: &mut Child,
    timeout: Duration,
) -> std::io::Result<ChildWaitOutcome> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(ChildWaitOutcome::Exited(status));
        }
        if Instant::now() >= deadline {
            // Best-effort kill + reap. If the child already exited between the
            // try_wait above and the kill, both calls succeed cleanly.
            let _ = child.kill();
            let _ = child.wait();
            return Ok(ChildWaitOutcome::TimedOut);
        }
        std::thread::sleep(CHILD_WAIT_POLL_INTERVAL);
    }
}
```

In `crates/core/src/thumbnails/mod.rs`, delete lines 17–45 (`CHILD_WAIT_POLL_INTERVAL`, `enum ChildWaitOutcome`, `fn wait_with_timeout`) and the now-unused `use std::process::Child;` / `use std::time::Instant;` imports, then add:

```rust
use crate::decode::subprocess::{wait_with_timeout, ChildWaitOutcome};
```

`generate_video_thumbnail`'s body is unchanged — it still calls `wait_with_timeout(&mut child, FFMPEG_THUMBNAIL_TIMEOUT)`.

- [ ] **Step 3b: Move `ffmpeg_available` and write the decoder**

In `crates/core/src/thumbnails/mod.rs`, delete the `pub fn ffmpeg_available()` body at `:471-478` and replace it with a re-export near the other `use` lines, so `tasks.rs:207`'s `crate::thumbnails::ffmpeg_available()` keeps resolving:

```rust
pub use crate::decode::ffmpeg::ffmpeg_available;
```

Add to `crates/core/src/decode/mod.rs`, next to `pub mod image_crate;`:

```rust
pub mod ffmpeg;
pub mod subprocess;
```

and append to the same file, after the `DecoderRegistry` impl:

```rust
impl DecoderRegistry {
    /// Registered decoder names in chain order. Exposed so a test can assert
    /// the capability-ascending ordering that the whole design depends on.
    pub fn decoder_names(&self) -> Vec<&'static str> {
        self.decoders.iter().map(|decoder| decoder.name()).collect()
    }
}

/// The production chain, ordered capability-ascending: cheapest and most
/// commonly correct first, so the JPEG/PNG majority never pays a subprocess.
pub fn default_registry() -> DecoderRegistry {
    DecoderRegistry::new(vec![
        Box::new(image_crate::ImageCrateDecoder),
        Box::new(ffmpeg::FfmpegDecoder),
    ])
}

/// Process-wide chain. Construction is trivial, but the thumbnail worker asks
/// for it once per task on a hot path, so build it once.
pub fn shared_registry() -> &'static DecoderRegistry {
    static REGISTRY: std::sync::OnceLock<DecoderRegistry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(default_registry)
}
```

Prepend the implementation to `crates/core/src/decode/ffmpeg.rs`:

```rust
//! The capability decoder: an ffmpeg subprocess for everything the `image`
//! crate cannot decode without new C dependencies — AVIF, HEIC/HEIF, PSD, and
//! TIFF variants `image` chokes on. ffmpeg is already a dependency of the video
//! poster path, so this adds reach without adding a build dependency.

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use image::ImageReader;

use super::subprocess::{wait_with_timeout, ChildWaitOutcome};
use super::{DecodePurpose, DecodeRequest, DecodedImage, MediaDecoder, DECODER_UNAVAILABLE_MARKER};

/// Wall-clock cap for one still-image decode. Shorter than the 30s video-poster
/// budget: a still frame that takes longer than this is pathological, and the
/// preview path is interactive.
const FFMPEG_DECODE_TIMEOUT: Duration = Duration::from_secs(20);

/// Extensions this decoder claims. Images only — video thumbnails keep their
/// dedicated `generate_video_thumbnail_bytes` path, which already emits WebP
/// directly and needs no RGBA round trip.
///
/// `.tif`/`.tiff` are claimed even though `ImageCrateDecoder` claims them too:
/// it runs first, and ffmpeg is the fall-through for the compression variants
/// (old JPEG-in-TIFF, LZW predictors, 32-bit float) that `image` rejects.
/// `.psd` is claimed speculatively — ffmpeg decodes many flattened PSDs; when
/// it cannot, the honest chain error is still better than today's silence.
const SUPPORTED_EXTENSIONS: &[&str] = &[".avif", ".heic", ".heif", ".psd", ".tif", ".tiff"];

/// Detects whether `ffmpeg` is on PATH. Cached for the process lifetime so the
/// worker pays the spawn cost once.
pub fn ffmpeg_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| match Command::new("ffmpeg").arg("-version").output() {
        Ok(output) => output.status.success(),
        Err(_) => false,
    })
}

fn unavailable_error() -> anyhow::Error {
    anyhow::anyhow!("{DECODER_UNAVAILABLE_MARKER}: ffmpeg is not installed or not on PATH")
}

/// "Fit inside a `max_px` box, never upscale, preserve aspect ratio."
/// The single quotes protect the commas from ffmpeg's filtergraph parser;
/// no shell is involved, so they reach ffmpeg literally.
pub fn ffmpeg_scale_filter(max_px: u32) -> String {
    format!(
        "scale=w='min({max_px},iw)':h='min({max_px},ih)':force_original_aspect_ratio=decrease:flags=area"
    )
}

pub struct FfmpegDecoder;

impl MediaDecoder for FfmpegDecoder {
    fn name(&self) -> &'static str {
        "ffmpeg"
    }

    fn handles(&self, ext: &str, _purpose: DecodePurpose) -> bool {
        SUPPORTED_EXTENSIONS.contains(&ext)
    }

    fn decode(&self, request: &DecodeRequest<'_>) -> anyhow::Result<DecodedImage> {
        if !ffmpeg_available() {
            return Err(unavailable_error());
        }
        let source_path = request.source.path();
        let scratch = scratch_path();
        if let Some(parent) = scratch.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| anyhow::anyhow!("ffmpeg scratch dir: {error}"))?;
        }

        let mut command = Command::new("ffmpeg");
        command
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(source_path)
            .args(["-frames:v", "1"]);
        if let Some(max_px) = request.target_max_px {
            command.args(["-vf", &ffmpeg_scale_filter(max_px)]);
        }
        // PNG, not rawvideo: rawvideo carries no dimensions, so reading it back
        // would need a second ffprobe subprocess. PNG is self-describing and
        // keeps alpha unambiguously (HEIC/AVIF can be transparent).
        //
        // The frame goes to a FILE, not stdout: `wait_with_timeout` does not
        // drain pipes, so a multi-megabyte frame on a 64 KB stdout pipe would
        // deadlock ffmpeg until the timeout. This mirrors the proven shape of
        // `thumbnails::generate_video_thumbnail`.
        command
            .args(["-f", "image2", "-c:v", "png", "-y"])
            .arg(&scratch)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let result = run_and_read(&mut command, &scratch);
        let _ = fs::remove_file(&scratch);
        result
    }
}

fn run_and_read(command: &mut Command, scratch: &std::path::Path) -> anyhow::Result<DecodedImage> {
    let mut child = command
        .spawn()
        .map_err(|error| anyhow::anyhow!("ffmpeg spawn: {error}"))?;
    let outcome = wait_with_timeout(&mut child, FFMPEG_DECODE_TIMEOUT)
        .map_err(|error| anyhow::anyhow!("ffmpeg wait: {error}"))?;
    let status = match outcome {
        ChildWaitOutcome::Exited(status) => status,
        ChildWaitOutcome::TimedOut => {
            return Err(anyhow::anyhow!(
                "ffmpeg timed out after {}s",
                FFMPEG_DECODE_TIMEOUT.as_secs()
            ));
        }
    };

    // The child has exited; drain both pipes now that they cannot block.
    let mut stderr_buf = Vec::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_end(&mut stderr_buf);
    }
    if let Some(mut stdout) = child.stdout.take() {
        let mut sink = Vec::new();
        let _ = stdout.read_to_end(&mut sink);
    }

    if !status.success() {
        let code = match status.code() {
            Some(code) => code.to_string(),
            None => "signal".to_string(),
        };
        return Err(anyhow::anyhow!(
            "ffmpeg exit {code} stderr: {}",
            String::from_utf8_lossy(&stderr_buf).trim()
        ));
    }

    let decoded = ImageReader::open(scratch)
        .map_err(|error| anyhow::anyhow!("ffmpeg output open: {error}"))?
        .with_guessed_format()
        .map_err(|error| anyhow::anyhow!("ffmpeg output format: {error}"))?
        .decode()
        .map_err(|error| anyhow::anyhow!("ffmpeg output decode: {error}"))?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 {
        return Err(anyhow::anyhow!("ffmpeg produced a zero-sized frame"));
    }
    Ok(DecodedImage {
        rgba: decoded.to_rgba8().into_raw(),
        width,
        height,
    })
}

fn scratch_path() -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "mangoo-decode-{}-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core decode:: 2>&1 | tail -25
cargo test -p mangoo-core thumbnails:: 2>&1 | tail -10
cargo fmt --all --check
```

Expected: all `decode::` tests pass (ffmpeg-dependent ones print `SKIP` and pass if ffmpeg is absent); the pre-existing `thumbnails::` suite still passes after the helper move; `cargo fmt` silent.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/decode/mod.rs crates/core/src/decode/ffmpeg.rs crates/core/src/decode/subprocess.rs crates/core/src/thumbnails/mod.rs
git commit -m "feat(core): add ffmpeg decoder for avif/heic/psd/exotic tiff"
```

---

### Task 4: One format table, thumbnails through the registry, `.heif` indexed

**Files:**
- Create: `crates/core/src/decode/formats.rs`
- Modify: `crates/core/src/decode/mod.rs` (add `pub mod formats;`)
- Modify: `crates/core/src/scan/mod.rs:704-720` (delete `media_kind` + `extension`, import them)
- Modify: `crates/core/src/watch/mod.rs:392-408` (delete `media_kind` + `extension`, import them)
- Modify: `crates/core/src/thumbnails/mod.rs:231-276` (`generate_image_thumbnail_bytes_with_checkpoint` decodes via the registry) and `:498-512` (`classify_thumbnail_failure` doc + tests)
- Test: inline `#[cfg(test)] mod tests` in `crates/core/src/decode/formats.rs`; extend the existing `#[cfg(test)] mod tests` in `crates/core/src/thumbnails/mod.rs`

**Interfaces:**
- Consumes: Task 1–3's `decode::{shared_registry, DecodeRequest, DecodePurpose, MediaSource}`.
- Produces: `decode::formats::{lowercase_extension, media_kind_for_path, media_kind_for_extension, is_browser_renderable_image, is_animated_capable_image, IMAGE_EXTENSIONS, VIDEO_EXTENSIONS}`. Task 7 consumes `is_browser_renderable_image`; Task 10's web lane mirrors `is_animated_capable_image` in TypeScript.

**Why de-duplicate instead of "add `.heif` to both lists":** `scan/mod.rs:704` and `watch/mod.rs:392` hold byte-identical copies of `media_kind` + `extension`. The spec's instruction is "any new indexed extension must be added in both" — a standing invitation for exactly the `.heic`-without-`.heif` drift being fixed here. One table, imported twice, retires the whole bug class. No structural check references either private function (verified), so the move is safe.

**Why the thumbnail decode passes `target_max_px: None`:** `GeneratedThumbnail.source_width/source_height` are persisted straight into `media.width/height` and drive the grid's aspect ratios *and* the preview's natural frame. If ffmpeg pre-scaled the intermediate, those numbers would be the scaled ones, not the original's. Correctness wins; the cost is paid once per file, only for formats that produce nothing at all today.

**Why `classify_thumbnail_failure` gains tests but no new branch:** the chain's aggregate error embeds every link's message verbatim, so an existing transient signature contributed by *any* link is already matched by the current `contains` logic — that is precisely the spec's "only permanent when every handling decoder fails". `DECODER_UNAVAILABLE_MARKER` correctly falls to `Permanent`: an uninstalled ffmpeg is not going to install itself, and marking it transient would arm the 60s `spawn_transient_retry_sweep` against every AVIF in the library forever. Adding a branch that returns the default would be dead code; locking the shapes with tests is the real work.

- [ ] **Step 1: Write the failing tests**

Create `crates/core/src/decode/formats.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn classifies_indexed_image_and_video_extensions() {
        for name in [
            "a.avif", "a.bmp", "a.gif", "a.heic", "a.heif", "a.jpeg", "a.jpg", "a.png", "a.psd",
            "a.raw", "a.tif", "a.tiff", "a.webp",
        ] {
            assert_eq!(
                media_kind_for_path(Path::new(name)),
                Some("image"),
                "{name}"
            );
        }
        for name in ["a.avi", "a.m4v", "a.mkv", "a.mov", "a.mp4", "a.webm", "a.wmv"] {
            assert_eq!(
                media_kind_for_path(Path::new(name)),
                Some("video"),
                "{name}"
            );
        }
        for name in ["a.txt", "a.zip", "a", "a.PDF"] {
            assert_eq!(media_kind_for_path(Path::new(name)), None, "{name}");
        }
    }

    #[test]
    fn heif_is_indexed_alongside_heic() {
        // Pre-existing inconsistency: `.heic` was indexed and `.heif` was not,
        // even though they are the same container.
        assert_eq!(media_kind_for_path(Path::new("a.heif")), Some("image"));
    }

    #[test]
    fn extension_is_lowercased_with_a_leading_dot() {
        assert_eq!(lowercase_extension(Path::new("A.JPG")), ".jpg");
        assert_eq!(lowercase_extension(Path::new("a.tar.GZ")), ".gz");
        assert_eq!(lowercase_extension(Path::new("noext")), "");
    }

    #[test]
    fn browser_renderable_set_excludes_what_chromium_cannot_paint() {
        for ext in [".jpg", ".jpeg", ".png", ".gif", ".webp", ".avif", ".bmp"] {
            assert!(is_browser_renderable_image(ext), "{ext} should render");
        }
        for ext in [".heic", ".heif", ".psd", ".raw", ".tif", ".tiff"] {
            assert!(!is_browser_renderable_image(ext), "{ext} must transcode");
        }
    }

    #[test]
    fn animation_capable_set_is_the_canvas_opt_out() {
        // These may hold multiple frames, so the preview must keep using <img>
        // (a canvas would freeze on frame one).
        for ext in [".gif", ".webp", ".avif"] {
            assert!(is_animated_capable_image(ext), "{ext}");
        }
        for ext in [".jpg", ".png", ".bmp", ".heic"] {
            assert!(!is_animated_capable_image(ext), "{ext}");
        }
    }
}
```

Add to `crates/core/src/thumbnails/mod.rs`'s existing `mod tests`:

```rust
    #[test]
    fn chain_failures_stay_permanent_but_inherit_a_transient_link() {
        use super::{classify_thumbnail_failure, ThumbnailFailureKind::*};
        // Nothing in the chain claimed the format: genuinely unsupported.
        assert!(matches!(
            classify_thumbnail_failure(
                "thumbnail decode failed: no decoder handles \".raw\""
            ),
            Permanent
        ));
        // Every claiming decoder failed on format grounds: still permanent.
        assert!(matches!(
            classify_thumbnail_failure(
                "thumbnail decode failed: all decoders failed for \".tif\": image_crate: unsupported compression; ffmpeg: ffmpeg exit 1 stderr: Invalid data"
            ),
            Permanent
        ));
        // One link hit a network drop: the whole attempt is retryable, because
        // the aggregate preserves each link's text verbatim.
        assert!(matches!(
            classify_thumbnail_failure(
                "thumbnail decode failed: all decoders failed for \".tif\": image_crate: os error 53; ffmpeg: ffmpeg spawn: 找不到网络路径。"
            ),
            Transient
        ));
    }

    #[test]
    fn a_missing_ffmpeg_is_permanent_and_names_itself() {
        use super::{classify_thumbnail_failure, ThumbnailFailureKind::*};
        let error = "thumbnail decode failed: all decoders failed for \".heic\": ffmpeg: decoder unavailable: ffmpeg is not installed or not on PATH";
        // Permanent on purpose: an uninstalled ffmpeg will not install itself,
        // and transient would arm the 60s retry sweep against every AVIF/HEIC
        // in the library forever.
        assert!(matches!(classify_thumbnail_failure(error), Permanent));
        assert!(error.contains("decoder unavailable"));
    }

    #[test]
    fn image_thumbnail_generation_flows_through_the_decoder_registry() {
        let dir = std::env::temp_dir().join(format!(
            "mangoo_thumb_registry_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("wide.png");
        let mut buffer = image::RgbaImage::new(900, 450);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgba([200, 100, 50, 255]);
        }
        image::DynamicImage::ImageRgba8(buffer)
            .save(&source)
            .expect("write fixture");

        let generated =
            generate_image_thumbnail_bytes(&source).expect("registry-backed generation");

        // Short side is exactly the profile; the true source dimensions survive
        // for media.width/height.
        assert_eq!(generated.height, GRID_320_SHORT_SIDE_PX);
        assert_eq!(generated.source_width, 900);
        assert_eq!(generated.source_height, 450);
        assert!(generated.byte_size > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unclaimed_format_reports_the_no_decoder_marker() {
        let dir = std::env::temp_dir().join(format!(
            "mangoo_thumb_nodecoder_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("camera.raw");
        std::fs::write(&source, b"raw sensor bytes").expect("write fixture");

        let error = generate_image_thumbnail_bytes(&source).expect_err("no decoder claims .raw");

        assert!(
            error.to_string().contains(crate::decode::NO_DECODER_MARKER),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core decode::formats 2>&1 | tail -15
cargo test -p mangoo-core thumbnails::tests 2>&1 | tail -15
```

Expected: FAIL — `file not found for module formats`, plus `assertion failed` on the `.raw` marker test (today's error text is `The file extension ... was not recognized`).

- [ ] **Step 3a: Create the one format table**

Add `pub mod formats;` to `crates/core/src/decode/mod.rs`. Prepend to `crates/core/src/decode/formats.rs`:

```rust
//! The single table of what a file extension means.
//!
//! Scan, the watcher, thumbnail generation, and the preview resolver all read
//! from here. This used to be two byte-identical private copies in
//! `scan/mod.rs` and `watch/mod.rs`, which is how `.heic` ended up indexed and
//! `.heif` did not.

use std::path::Path;

/// Extensions indexed as `media.kind = "image"`. Indexing is deliberately wider
/// than decoding: a `.psd` or `.raw` is still a real picture the user expects to
/// SEE in the grid, even while the decode chain can only report why it failed.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    ".avif", ".bmp", ".gif", ".heic", ".heif", ".jpeg", ".jpg", ".png", ".psd", ".raw", ".tif",
    ".tiff", ".webp",
];

/// Extensions indexed as `media.kind = "video"`.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    ".avi", ".m4v", ".mkv", ".mov", ".mp4", ".webm", ".wmv",
];

/// What Chromium paints from raw bytes in an `<img>`/`createImageBitmap`.
/// Anything outside this set must be transcoded to WebP by Core before the
/// frontend can show it.
const BROWSER_RENDERABLE_IMAGE_EXTENSIONS: &[&str] = &[
    ".avif", ".bmp", ".gif", ".jpeg", ".jpg", ".png", ".webp",
];

/// Formats that can carry more than one frame. The preview surface keeps these
/// on `<img>` for native animation — a canvas would freeze them on frame one.
const ANIMATION_CAPABLE_IMAGE_EXTENSIONS: &[&str] = &[".avif", ".gif", ".webp"];

/// Lowercase extension INCLUDING the leading dot, or `""` when the file has
/// none. This exact shape is what `files.ext` stores and what
/// `DecodeRequest::ext` expects.
pub fn lowercase_extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_default()
}

pub fn media_kind_for_extension(ext: &str) -> Option<&'static str> {
    if IMAGE_EXTENSIONS.contains(&ext) {
        return Some("image");
    }
    if VIDEO_EXTENSIONS.contains(&ext) {
        return Some("video");
    }
    None
}

pub fn media_kind_for_path(path: &Path) -> Option<&'static str> {
    media_kind_for_extension(lowercase_extension(path).as_str())
}

pub fn is_browser_renderable_image(ext: &str) -> bool {
    BROWSER_RENDERABLE_IMAGE_EXTENSIONS.contains(&ext)
}

pub fn is_animated_capable_image(ext: &str) -> bool {
    ANIMATION_CAPABLE_IMAGE_EXTENSIONS.contains(&ext)
}
```

In `crates/core/src/scan/mod.rs`, delete `fn media_kind` (`:704-711`) and `fn extension` (`:713-720`), and add to the imports:

```rust
use crate::decode::formats::{lowercase_extension as extension, media_kind_for_path as media_kind};
```

Apply the identical deletion + import to `crates/core/src/watch/mod.rs` (`:392-408`). Both files' call sites (`scan:294`, `scan:307`, `scan:514`, `scan:527`, `watch:266`, `watch:284`) are unchanged because the aliases keep the local names.

- [ ] **Step 3b: Route thumbnail decoding through the registry**

Replace the decode preamble of `generate_image_thumbnail_bytes_with_checkpoint` in `crates/core/src/thumbnails/mod.rs` (the `ImageReader::open(...)...decode()` block, `:231-244`) with:

```rust
pub fn generate_image_thumbnail_bytes_with_checkpoint(
    source_path: &Path,
    mut checkpoint: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<GeneratedThumbnail> {
    let ext = crate::decode::formats::lowercase_extension(source_path);
    // `target_max_px: None` on purpose. `source_width`/`source_height` below are
    // persisted straight into `media.width/height` and drive both the grid's
    // aspect ratios and the preview's natural frame; letting a decoder pre-scale
    // would report the scaled size as the original's. The cost lands once per
    // file, and only on formats that produce nothing at all today.
    let decoded = crate::decode::shared_registry()
        .decode(&crate::decode::DecodeRequest {
            source: crate::decode::MediaSource::Path(source_path),
            ext: ext.as_str(),
            target_max_px: None,
            purpose: crate::decode::DecodePurpose::Thumbnail,
        })
        .map_err(|error| anyhow::anyhow!("thumbnail decode failed: {error}"))?;
    let decoded = decoded
        .into_dynamic()
        .map_err(|error| anyhow::anyhow!("thumbnail decode failed: {error}"))?;
    checkpoint()?;
```

Everything below `checkpoint()?` — the zero-size guard, `target_dimensions`, `thumbnail_exact`, `to_rgba8`, `webp::Encoder`, and the `GeneratedThumbnail` construction — is **unchanged**. `use image::ImageReader;` stays: `generate_preview_placeholder` and `generate_video_thumbnail` still use it.

- [ ] **Step 3c: Widen the classification doc comment**

Above `classify_thumbnail_failure` in `crates/core/src/thumbnails/mod.rs`, extend the existing doc comment with:

```rust
/// Errors now arrive from the decode chain, so a single string can carry every
/// link's failure ("all decoders failed for \".tif\": image_crate: ...; ffmpeg:
/// ..."). That is exactly the semantics the classifier wants: a `contains`
/// match means ANY link hit a transient condition, so the attempt is worth
/// retrying, while a chain that failed purely on format grounds — or a
/// `no decoder handles` / `decoder unavailable` result — falls through to
/// permanent and is never auto-retried.
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core decode:: 2>&1 | tail -15
cargo test -p mangoo-core thumbnails:: 2>&1 | tail -15
cargo test -p mangoo-core scan:: 2>&1 | tail -8
cargo test -p mangoo-core watch:: 2>&1 | tail -8
cargo fmt --all --check
```

Expected: all pass; the pre-existing scan/watch suites are unaffected by the import swap.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/decode/mod.rs crates/core/src/decode/formats.rs crates/core/src/scan/mod.rs crates/core/src/watch/mod.rs crates/core/src/thumbnails/mod.rs
git commit -m "feat(core): unify format table and decode thumbnails via the registry"
```

---

### Task 5: `preview_blobs` table (migration 0025) and its accessors

**Files:**
- Create: `crates/core/migrations/0025_preview_blobs.sql`
- Modify: `crates/core/src/db/migrations.rs` (append `PREVIEW_BLOBS_MIGRATION`)
- Modify: `crates/core/src/db/mod.rs` — gate 25 in `apply_migrations` after the 24 gate; add `PreviewBlobRecord` next to `ThumbBlobRecord:442`; add accessors next to `get_thumb_blob:5977`; extend `clear_thumbnail_cache:4655`
- Modify: `tools/checks/validate_sqlite_schema.py` (`MIGRATIONS`, `REQUIRED_TABLES`, `REQUIRED_INDEXES`, new `EXPECTED_PREVIEW_BLOBS_COLUMNS` block after the thumb-blob one at `:386`)
- Modify: `tools/checks/validate-core-api.mjs` (assert the migration is declared, mirroring the `0013` assertion at `:168`)
- Test: extend `crates/core/src/db/mod.rs`'s existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing from Tasks 1–4.
- Produces: `db::PreviewBlobRecord { file_id: i64, profile: String, data: Vec<u8>, width: i64, height: i64, byte_size: i64, output_format: String, source_size: i64, source_mtime: i64, created_at: i64, updated_at: i64 }`; `Database::get_preview_blob(&self, file_id: i64, profile: &str) -> anyhow::Result<Option<PreviewBlobRecord>>`; `Database::upsert_preview_blob(&self, blob: PreviewBlobRecord) -> anyhow::Result<()>` (which also enforces the LRU cap); `db::PREVIEW_BLOB_CACHE_LIMIT_BYTES: i64`. Tasks 6–7 consume all of these.

**Why a separate table and not a widened `thumb_blobs` CHECK:** measured on the real library, `thumb_blobs` holds **663,000 rows / 17.8 GB**. SQLite cannot alter a `CHECK` constraint in place — it needs a full table rebuild, which would copy ~18 GB during `apply_migrations` on app open. A new empty table costs nothing, and preview tiers want a different lifecycle anyway (big, disposable, LRU-evicted) than 320px grid thumbs (small, permanent, state-machine-owned).

**Why `source_size` + `source_mtime` instead of a fingerprint hash:** the `get_preview` route already `stat`s the file (`file.metadata()` at `routes.rs`), so staleness is a free comparison against two integers it is holding anyway. Computing `source_fingerprint_for` would need the full `CacheIdentity` (root/folder/name/file_key) and therefore an extra query on an interactive path.

**Why an LRU cap:** a tier is 1–3 MB and is created for every large image the user opens. Without a bound, a heavy browsing session grows the library file without limit. `idx_preview_blobs_updated_at` makes "delete oldest until under budget" a single indexed scan.

- [ ] **Step 1: Write the failing tests**

Add to `crates/core/src/db/mod.rs`'s existing `mod tests`:

```rust
    #[test]
    fn preview_blob_round_trips_and_reports_staleness_inputs() {
        let database = Database::open_in_memory().expect("open");
        database.apply_migrations().expect("migrate");
        let file_id = seed_single_media_file(&database);

        assert!(database
            .get_preview_blob(file_id, "preview_2048")
            .expect("query empty")
            .is_none());

        database
            .upsert_preview_blob(PreviewBlobRecord {
                file_id,
                profile: "preview_2048".to_string(),
                data: vec![1, 2, 3, 4],
                width: 2048,
                height: 1152,
                byte_size: 4,
                output_format: "image/webp".to_string(),
                source_size: 9_000_000,
                source_mtime: 1_700_000_000,
                created_at: 10,
                updated_at: 10,
            })
            .expect("insert");

        let stored = database
            .get_preview_blob(file_id, "preview_2048")
            .expect("query")
            .expect("row present");
        assert_eq!(stored.data, vec![1, 2, 3, 4]);
        assert_eq!(stored.width, 2048);
        assert_eq!(stored.source_size, 9_000_000);
        assert_eq!(stored.source_mtime, 1_700_000_000);
    }

    #[test]
    fn preview_blob_upsert_replaces_the_same_profile_in_place() {
        let database = Database::open_in_memory().expect("open");
        database.apply_migrations().expect("migrate");
        let file_id = seed_single_media_file(&database);

        for (byte, updated_at) in [(1u8, 10i64), (2u8, 20i64)] {
            database
                .upsert_preview_blob(PreviewBlobRecord {
                    file_id,
                    profile: "preview_2048".to_string(),
                    data: vec![byte],
                    width: 2048,
                    height: 1152,
                    byte_size: 1,
                    output_format: "image/webp".to_string(),
                    source_size: 1,
                    source_mtime: 1,
                    created_at: 10,
                    updated_at,
                })
                .expect("upsert");
        }

        let count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM preview_blobs", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 1);
        let stored = database
            .get_preview_blob(file_id, "preview_2048")
            .expect("query")
            .expect("row");
        assert_eq!(stored.data, vec![2]);
        assert_eq!(stored.updated_at, 20);
    }

    #[test]
    fn preview_blob_cache_evicts_least_recently_used_rows_past_the_budget() {
        let database = Database::open_in_memory().expect("open");
        database.apply_migrations().expect("migrate");
        let file_ids = seed_media_files(&database, 3);
        // Each blob is half the budget, so the third insert must evict the
        // oldest by updated_at.
        let half = PREVIEW_BLOB_CACHE_LIMIT_BYTES / 2;
        for (index, file_id) in file_ids.iter().enumerate() {
            database
                .upsert_preview_blob(PreviewBlobRecord {
                    file_id: *file_id,
                    profile: "preview_2048".to_string(),
                    data: vec![index as u8],
                    width: 2048,
                    height: 1152,
                    byte_size: half,
                    output_format: "image/webp".to_string(),
                    source_size: 1,
                    source_mtime: 1,
                    created_at: 10,
                    updated_at: 100 + index as i64,
                })
                .expect("upsert");
        }

        assert!(
            database
                .get_preview_blob(file_ids[0], "preview_2048")
                .expect("query")
                .is_none(),
            "oldest tier should be evicted"
        );
        assert!(database
            .get_preview_blob(file_ids[2], "preview_2048")
            .expect("query")
            .is_some());
    }

    #[test]
    fn clearing_the_thumbnail_cache_also_drops_preview_tiers() {
        let database = Database::open_in_memory().expect("open");
        database.apply_migrations().expect("migrate");
        let file_id = seed_single_media_file(&database);
        database
            .upsert_preview_blob(PreviewBlobRecord {
                file_id,
                profile: "preview_2048".to_string(),
                data: vec![7],
                width: 2048,
                height: 1152,
                byte_size: 1,
                output_format: "image/webp".to_string(),
                source_size: 1,
                source_mtime: 1,
                created_at: 10,
                updated_at: 10,
            })
            .expect("upsert");

        database.clear_thumbnail_cache().expect("clear");

        assert!(database
            .get_preview_blob(file_id, "preview_2048")
            .expect("query")
            .is_none());
    }

    #[test]
    fn deleting_a_file_cascades_to_its_preview_tiers() {
        let database = Database::open_in_memory().expect("open");
        database.apply_migrations().expect("migrate");
        let file_id = seed_single_media_file(&database);
        database
            .upsert_preview_blob(PreviewBlobRecord {
                file_id,
                profile: "preview_2048".to_string(),
                data: vec![7],
                width: 2048,
                height: 1152,
                byte_size: 1,
                output_format: "image/webp".to_string(),
                source_size: 1,
                source_mtime: 1,
                created_at: 10,
                updated_at: 10,
            })
            .expect("upsert");

        database
            .connection
            .execute("PRAGMA foreign_keys = ON", [])
            .ok();
        database
            .connection
            .execute("DELETE FROM files WHERE id = ?1", [file_id])
            .expect("delete file");

        let count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM preview_blobs", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 0);
    }
```

`seed_single_media_file` / `seed_media_files` are helpers this task adds to the same test module (an existing test-module helper may already cover it — reuse it if so):

```rust
    fn seed_media_files(database: &Database, count: usize) -> Vec<i64> {
        let root_id = database
            .upsert_root("C:/fixtures", "fixtures")
            .expect("root")
            .id;
        let folder_id = database
            .upsert_folder(root_id, None, "fixtures", "C:/fixtures")
            .expect("folder");
        (0..count)
            .map(|index| {
                database
                    .upsert_file(folder_id, &format!("photo-{index}.jpg"), ".jpg", 1, 1)
                    .expect("file")
            })
            .collect()
    }

    fn seed_single_media_file(database: &Database) -> i64 {
        seed_media_files(database, 1)[0]
    }
```

> **Implementer note:** `upsert_root`/`upsert_folder`/`upsert_file` above are placeholders for whatever the existing `db::tests` module already uses to seed a file row — read the neighbouring tests and call the same helpers rather than inventing new ones. Only the `preview_blobs` assertions are new.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core preview_blob 2>&1 | tail -20
```

Expected: FAIL — `cannot find struct PreviewBlobRecord`, `no method get_preview_blob`.

- [ ] **Step 3a: Write the migration**

Create `crates/core/migrations/0025_preview_blobs.sql`:

```sql
-- Adaptive downscaled preview tiers, cached as WebP BLOBs.
--
-- Deliberately NOT a new profile inside `thumb_blobs`: that table's
-- `CHECK(profile = 'grid_320')` cannot be altered in place, and on a real
-- library it holds ~663k rows / ~18 GB, so widening it would mean copying 18 GB
-- during app open. Preview tiers also want their own lifecycle — large,
-- regenerable, LRU-evicted — while grid thumbs are small, permanent, and owned
-- by the `thumbs` state machine.
--
-- `source_size` / `source_mtime` are the staleness key: the preview route
-- already stats the file, so validating a cached tier costs two integer
-- comparisons and no extra query.
BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS preview_blobs (
  file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  profile TEXT NOT NULL CHECK(profile IN ('preview_2048','preview_3072','preview_native')),
  data BLOB NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  byte_size INTEGER NOT NULL,
  output_format TEXT NOT NULL DEFAULT 'image/webp' CHECK(output_format = 'image/webp'),
  source_size INTEGER NOT NULL,
  source_mtime INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(file_id, profile)
);

-- Supports "evict least recently used until under budget" as one indexed scan.
CREATE INDEX IF NOT EXISTS idx_preview_blobs_updated_at
ON preview_blobs(updated_at);

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (25, 'preview_blobs', unixepoch());

COMMIT;
```

Append to `crates/core/src/db/migrations.rs`:

```rust
pub const PREVIEW_BLOBS_MIGRATION: &str = include_str!("../../migrations/0025_preview_blobs.sql");
```

Add the gate to `Database::apply_migrations` in `crates/core/src/db/mod.rs`, immediately after the `migration_applied(24)` block and before `self.backfill_media_fts_if_empty()?;`:

```rust
        if !self.migration_applied(25)? {
            self.connection
                .execute_batch(migrations::PREVIEW_BLOBS_MIGRATION)?;
        }
```

- [ ] **Step 3b: Add the record type and accessors**

Add next to `ThumbBlobRecord` in `crates/core/src/db/mod.rs`:

```rust
/// Total bytes of cached preview tiers this library keeps before evicting the
/// least recently used. Tiers are 1–3 MB each and are created for every large
/// image the user opens, so an unbounded cache would grow the library file
/// without limit. Regenerating an evicted tier costs one decode.
pub const PREVIEW_BLOB_CACHE_LIMIT_BYTES: i64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PreviewBlobRecord {
    pub file_id: i64,
    pub profile: String,
    pub data: Vec<u8>,
    pub width: i64,
    pub height: i64,
    pub byte_size: i64,
    pub output_format: String,
    /// Source file size at generation time; the tier is stale when it differs.
    pub source_size: i64,
    /// Source file mtime at generation time; the tier is stale when it differs.
    pub source_mtime: i64,
    pub created_at: i64,
    pub updated_at: i64,
}
```

Add next to `get_thumb_blob`:

```rust
    pub fn get_preview_blob(
        &self,
        file_id: i64,
        profile: &str,
    ) -> anyhow::Result<Option<PreviewBlobRecord>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT file_id, profile, data, width, height, byte_size, output_format,
                   source_size, source_mtime, created_at, updated_at
            FROM preview_blobs
            WHERE file_id = ?1 AND profile = ?2
            "#,
        )?;
        let mut rows = statement.query((file_id, profile))?;
        Ok(rows.next()?.map(preview_blob_from_row).transpose()?)
    }

    pub fn upsert_preview_blob(&self, blob: PreviewBlobRecord) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            r#"
            INSERT INTO preview_blobs(
                file_id, profile, data, width, height, byte_size, output_format,
                source_size, source_mtime, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(file_id, profile) DO UPDATE SET
                data = excluded.data,
                width = excluded.width,
                height = excluded.height,
                byte_size = excluded.byte_size,
                output_format = excluded.output_format,
                source_size = excluded.source_size,
                source_mtime = excluded.source_mtime,
                updated_at = excluded.updated_at
            "#,
            (
                blob.file_id,
                &blob.profile,
                &blob.data,
                blob.width,
                blob.height,
                blob.byte_size,
                &blob.output_format,
                blob.source_size,
                blob.source_mtime,
                blob.created_at,
                blob.updated_at,
            ),
        )?;
        // Evict least-recently-used tiers until the cache is inside budget. The
        // row just written is the most recent, so it is never its own victim.
        transaction.execute(
            r#"
            DELETE FROM preview_blobs
            WHERE rowid IN (
              SELECT rowid FROM (
                SELECT rowid,
                       SUM(byte_size) OVER (
                         ORDER BY updated_at DESC, rowid DESC
                         ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
                       ) AS running_bytes
                FROM preview_blobs
              )
              WHERE running_bytes > ?1
            )
            "#,
            [PREVIEW_BLOB_CACHE_LIMIT_BYTES],
        )?;
        transaction.commit()?;
        Ok(())
    }
```

Add the row mapper next to `thumb_blob_from_row:7971`:

```rust
fn preview_blob_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PreviewBlobRecord> {
    Ok(PreviewBlobRecord {
        file_id: row.get(0)?,
        profile: row.get(1)?,
        data: row.get(2)?,
        width: row.get(3)?,
        height: row.get(4)?,
        byte_size: row.get(5)?,
        output_format: row.get(6)?,
        source_size: row.get(7)?,
        source_mtime: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}
```

In `clear_thumbnail_cache`, immediately after the `DELETE FROM thumb_blobs` line, add:

```rust
        // Preview tiers are regenerable from the same originals; clearing the
        // thumbnail cache must not leave a second stale blob store behind.
        transaction.execute("DELETE FROM preview_blobs", [])?;
```

- [ ] **Step 3c: Update the schema and API checks**

`tools/checks/validate_sqlite_schema.py`:

```python
# append to MIGRATIONS
    ROOT / "crates" / "core" / "migrations" / "0025_preview_blobs.sql",

# add to REQUIRED_TABLES
    "preview_blobs",

# add to REQUIRED_INDEXES
    "idx_preview_blobs_updated_at",

# add next to EXPECTED_THUMB_BLOBS_COLUMNS
EXPECTED_PREVIEW_BLOBS_COLUMNS = {
    "file_id",
    "profile",
    "data",
    "width",
    "height",
    "byte_size",
    "output_format",
    "source_size",
    "source_mtime",
    "created_at",
    "updated_at",
}
```

and, right after the `missing_thumb_blobs_columns` check (`:386-388`):

```python
            preview_blobs_columns = {
                row[1] for row in conn.execute("PRAGMA table_info(preview_blobs)").fetchall()
            }
            missing_preview_blobs_columns = (
                EXPECTED_PREVIEW_BLOBS_COLUMNS - preview_blobs_columns
            )
            if missing_preview_blobs_columns:
                fail(
                    f"missing preview blob columns: {sorted(missing_preview_blobs_columns)}"
                )
```

`tools/checks/validate-core-api.mjs`, next to the `0013` assertion at `:168`:

```javascript
if (!dbMigrationsRs.includes('include_str!("../../migrations/0025_preview_blobs.sql")')) {
  fail("db preview_blobs migration include path changed or missing");
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core preview_blob 2>&1 | tail -20
cargo test -p mangoo-core db:: 2>&1 | tail -8
npm run check:schema
npm run check:core-api
cargo fmt --all --check
```

Expected: 5 new tests pass, the existing `db::` suite passes, `check:schema` prints PASS, `check:core-api` prints PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/migrations/0025_preview_blobs.sql crates/core/src/db/migrations.rs crates/core/src/db/mod.rs tools/checks/validate_sqlite_schema.py tools/checks/validate-core-api.mjs
git commit -m "feat(core): add preview_blobs tier cache table"
```

---

### Task 6: Preview tier profiles, selection rule, and WebP generation

**Files:**
- Modify: `crates/core/src/thumbnails/mod.rs` — constants next to `GRID_320_PROFILE:64`, generator next to `generate_preview_placeholder:277`, dimension helper next to `bounded_dimensions:452`
- Test: extend `crates/core/src/thumbnails/mod.rs`'s existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: Task 1–4's `decode::{shared_registry, DecodeRequest, DecodePurpose, MediaSource, formats::lowercase_extension}`.
- Produces:
  - `thumbnails::PREVIEW_2048_PROFILE: &str = "preview_2048"`, `PREVIEW_3072_PROFILE: &str = "preview_3072"`, `PREVIEW_NATIVE_PROFILE: &str = "preview_native"`, `PREVIEW_PROFILE_VALUES: &[&str]` (the two *tiers* only), `PREVIEW_TIER_MIN_SOURCE_MAX_PX: u32 = 4096`, `PREVIEW_NATIVE_MAX_PX: u32 = 4096`
  - `thumbnails::preview_profile_max_px(profile: &str) -> Option<u32>`
  - `thumbnails::select_preview_profile(source_max_px: u32, viewport_long_px: u32) -> Option<&'static str>`
  - `thumbnails::preview_tier_dimensions(width: u32, height: u32, max_px: u32) -> (u32, u32)`
  - `thumbnails::GeneratedPreviewTier { data: Vec<u8>, width: i64, height: i64, byte_size: i64, output_format: &'static str }`
  - `thumbnails::generate_preview_webp(source_path: &Path, max_px: u32) -> anyhow::Result<GeneratedPreviewTier>`

  Task 7 consumes every one of these.

**Deviation from the spec, and why: tiers are keyed by LONGEST side, and the query parameter is `viewportLongPx`, not `viewportShortPx`.** Three reasons, all checked against the code:
1. The preview's default view mode is `fit-long-edge` (`CentralPreviewStage.tsx`), so the pixels the user actually sees are governed by the long edge.
2. Longest-side is what bounds decoded memory. A short-side-2048 tier on a 2:1 panorama is 4096×2048×4 = 33 MB; on a 6:1 panorama it is 100 MB. A longest-side-2048 tier is at most 2048×2048×4 = 17 MB, always.
3. `DecodeRequest::target_max_px` and `ffmpeg_scale_filter` are already longest-side caps, so nothing needs a second convention.

**Two tiers, not the spec's three.** `preview_2048` covers ≤1706px-long viewports (1366×768 laptops) and `preview_3072` covers everything up to a 4K window; a third tier would triple the cache footprint for a gap that the background original-upgrade (Task 10) already closes.

- [ ] **Step 1: Write the failing tests**

Add to `crates/core/src/thumbnails/mod.rs`'s existing `mod tests`:

```rust
    #[test]
    fn small_originals_never_get_a_downscaled_tier() {
        // At or below the threshold the original IS the preview.
        for source_max_px in [320, 1920, 4096] {
            assert_eq!(
                select_preview_profile(source_max_px, 2560),
                None,
                "source {source_max_px} should serve the original"
            );
        }
    }

    #[test]
    fn tier_selection_picks_the_smallest_profile_that_covers_the_viewport() {
        // 1366x768 laptop: 1366 * 1.2 = 1639 -> 2048 is enough.
        assert_eq!(
            select_preview_profile(16384, 1366),
            Some(PREVIEW_2048_PROFILE)
        );
        // 1920x1080: 1920 * 1.2 = 2304 -> needs 3072.
        assert_eq!(
            select_preview_profile(16384, 1920),
            Some(PREVIEW_3072_PROFILE)
        );
        // 4K window: clamps at the largest tier; the background original
        // upgrade closes the remaining gap.
        assert_eq!(
            select_preview_profile(16384, 3840),
            Some(PREVIEW_3072_PROFILE)
        );
    }

    #[test]
    fn tier_selection_refuses_to_upscale_the_original() {
        // A 5000px original with a 1366px viewport wants preview_2048 (2048 <
        // 5000): fine. But a 2500px original would be UPSCALED by preview_3072,
        // so serve the original instead.
        assert_eq!(
            select_preview_profile(5000, 1366),
            Some(PREVIEW_2048_PROFILE)
        );
        assert_eq!(select_preview_profile(4200, 3840), None);
    }

    #[test]
    fn an_unknown_viewport_serves_the_original_unchanged() {
        // A client that sends no viewport gets today's exact behaviour.
        assert_eq!(select_preview_profile(16384, 0), None);
    }

    #[test]
    fn preview_profile_max_px_is_defined_for_every_declared_profile() {
        for profile in PREVIEW_PROFILE_VALUES {
            assert!(
                preview_profile_max_px(profile).is_some(),
                "no cap for {profile}"
            );
        }
        assert_eq!(preview_profile_max_px("grid_320"), None);
        assert_eq!(preview_profile_max_px("preview_9999"), None);
    }

    #[test]
    fn tier_dimensions_fit_the_box_and_never_upscale() {
        assert_eq!(preview_tier_dimensions(8000, 4000, 2048), (2048, 1024));
        assert_eq!(preview_tier_dimensions(4000, 8000, 2048), (1024, 2048));
        // Already inside the box: unchanged.
        assert_eq!(preview_tier_dimensions(1000, 500, 2048), (1000, 500));
        // Never collapse a dimension to zero on extreme ratios.
        assert_eq!(preview_tier_dimensions(20000, 3, 2048), (2048, 1));
    }

    #[test]
    fn generates_a_downscaled_webp_tier_from_a_large_original() {
        let dir = std::env::temp_dir().join(format!(
            "mangoo_preview_tier_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("huge.png");
        let mut buffer = image::RgbaImage::new(5000, 500);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgba([30, 60, 90, 255]);
        }
        image::DynamicImage::ImageRgba8(buffer)
            .save(&source)
            .expect("write fixture");

        let tier = generate_preview_webp(&source, 2048).expect("tier generation");

        assert_eq!(tier.width, 2048);
        assert_eq!(tier.height, 205);
        assert_eq!(tier.output_format, GENERATED_FORMAT);
        assert_eq!(tier.byte_size, tier.data.len() as i64);
        // RIFF....WEBP
        assert_eq!(&tier.data[0..4], b"RIFF");
        assert_eq!(&tier.data[8..12], b"WEBP");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cap_larger_than_the_source_transcodes_without_upscaling() {
        let dir = std::env::temp_dir().join(format!(
            "mangoo_preview_transcode_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("medium.png");
        let mut buffer = image::RgbaImage::new(640, 480);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgba([7, 7, 7, 255]);
        }
        image::DynamicImage::ImageRgba8(buffer)
            .save(&source)
            .expect("write fixture");

        let transcoded =
            generate_preview_webp(&source, PREVIEW_NATIVE_MAX_PX).expect("transcode");

        assert_eq!((transcoded.width, transcoded.height), (640, 480));
        assert_eq!(&transcoded.data[8..12], b"WEBP");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_generation_reports_chain_failures_verbatim() {
        let dir = std::env::temp_dir().join(format!(
            "mangoo_preview_fail_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("sensor.raw");
        std::fs::write(&source, b"not decodable").expect("write fixture");

        let error = generate_preview_webp(&source, 2048).expect_err("no decoder for .raw");

        assert!(
            error.to_string().contains(crate::decode::NO_DECODER_MARKER),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core thumbnails::tests 2>&1 | tail -20
```

Expected: FAIL — `cannot find function select_preview_profile`, `cannot find value PREVIEW_2048_PROFILE`, `cannot find function generate_preview_webp`.

- [ ] **Step 3: Write the implementation**

Add next to the `GRID_320_*` constants in `crates/core/src/thumbnails/mod.rs`:

```rust
/// Adaptive downscaled preview tiers, keyed by the image's LONGEST side.
///
/// Longest-side (not short-side) because the preview's default view mode is
/// `fit-long-edge`, and because it bounds decoded memory unconditionally: a
/// longest-side-2048 tier is at most 2048*2048*4 = 17 MB no matter the aspect
/// ratio, while a short-side-2048 tier on a 6:1 panorama is 100 MB.
pub const PREVIEW_2048_PROFILE: &str = "preview_2048";
pub const PREVIEW_3072_PROFILE: &str = "preview_3072";
/// Cache profile for a browser-UNRENDERABLE image small enough that no tier
/// applies (PSD/HEIC/TIFF under the threshold): the goal is format conversion,
/// not shrinking, so this is "native resolution" — bounded by
/// [`PREVIEW_NATIVE_MAX_PX`] purely as a safety rail for the case where
/// `media.width/height` are still `pending` and the source turns out to be huge.
pub const PREVIEW_NATIVE_PROFILE: &str = "preview_native";
pub const PREVIEW_NATIVE_MAX_PX: u32 = 4096;
/// The downscaled TIERS, ordered smallest-first; selection walks this list.
/// `PREVIEW_NATIVE_PROFILE` is deliberately absent — it is not a tier.
pub const PREVIEW_PROFILE_VALUES: &[&str] = &[PREVIEW_2048_PROFILE, PREVIEW_3072_PROFILE];
/// Originals whose longest side is at or below this never get a tier — the
/// original is already a reasonable preview and streaming it costs nothing.
pub const PREVIEW_TIER_MIN_SOURCE_MAX_PX: u32 = 4096;
/// Resolution headroom over the viewport (1.2x) so a modest zoom or a
/// fractional device pixel ratio does not immediately look soft.
const PREVIEW_TIER_HEADROOM_NUMERATOR: u32 = 6;
const PREVIEW_TIER_HEADROOM_DENOMINATOR: u32 = 5;
/// WebP quality for preview tiers. Higher than the 75 used for 320px grid
/// thumbs because this IS the image the user looks at.
const PREVIEW_WEBP_QUALITY: f32 = 82.0;

pub fn preview_profile_max_px(profile: &str) -> Option<u32> {
    match profile {
        PREVIEW_2048_PROFILE => Some(2048),
        PREVIEW_3072_PROFILE => Some(3072),
        _ => None,
    }
}

/// Which cached tier (if any) should back a preview of an image whose longest
/// side is `source_max_px`, viewed in a window whose longest side is
/// `viewport_long_px`. `None` means "serve the original bytes" — which is both
/// the small-image case and the no-viewport-reported case, so a client that
/// sends nothing gets exactly today's behaviour.
pub fn select_preview_profile(source_max_px: u32, viewport_long_px: u32) -> Option<&'static str> {
    if viewport_long_px == 0 || source_max_px <= PREVIEW_TIER_MIN_SOURCE_MAX_PX {
        return None;
    }
    let required = viewport_long_px
        .saturating_mul(PREVIEW_TIER_HEADROOM_NUMERATOR)
        .div_ceil(PREVIEW_TIER_HEADROOM_DENOMINATOR);
    let profile = PREVIEW_PROFILE_VALUES
        .iter()
        .copied()
        .find(|profile| preview_profile_max_px(profile).unwrap_or(0) >= required)
        // Beyond the largest tier, clamp to it: the frontend's background
        // original-upgrade closes the remaining sharpness gap.
        .unwrap_or(PREVIEW_3072_PROFILE);
    // A tier that is not smaller than the original would be pure loss.
    if preview_profile_max_px(profile).unwrap_or(0) >= source_max_px {
        return None;
    }
    Some(profile)
}

/// Fit `width x height` inside a `max_px` box, preserving aspect ratio and
/// never upscaling. Neither dimension is ever allowed to round to zero.
pub fn preview_tier_dimensions(width: u32, height: u32, max_px: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest == 0 || max_px == 0 || longest <= max_px {
        return (width, height);
    }
    let scaled_width = ((width as u64 * max_px as u64) / longest as u64) as u32;
    let scaled_height = ((height as u64 * max_px as u64) / longest as u64) as u32;
    (scaled_width.max(1), scaled_height.max(1))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedPreviewTier {
    pub data: Vec<u8>,
    pub width: i64,
    pub height: i64,
    pub byte_size: i64,
    pub output_format: &'static str,
}
```

Add the generator next to `generate_preview_placeholder`:

```rust
/// Decode `source_path` through the registry and re-encode it as WebP, capped
/// to `max_px` on its longest side. A cap larger than the source is a no-op —
/// this never upscales — so `PREVIEW_NATIVE_MAX_PX` doubles as "native
/// resolution, with a safety rail".
pub fn generate_preview_webp(
    source_path: &Path,
    max_px: u32,
) -> anyhow::Result<GeneratedPreviewTier> {
    let ext = crate::decode::formats::lowercase_extension(source_path);
    let decoded = crate::decode::shared_registry()
        .decode(&crate::decode::DecodeRequest {
            source: crate::decode::MediaSource::Path(source_path),
            ext: ext.as_str(),
            // Passed as a hint: ffmpeg scales in-process (so a 16k HEIC never
            // materialises at full size), while `image` ignores it and the
            // resize below runs instead. Either way the output honours the cap.
            target_max_px: Some(max_px),
            purpose: crate::decode::DecodePurpose::Preview,
        })
        .map_err(|error| anyhow::anyhow!("preview decode failed: {error}"))?;
    if decoded.width == 0 || decoded.height == 0 {
        return Err(anyhow::anyhow!(
            "preview decode failed: zero-sized source image"
        ));
    }
    let (target_width, target_height) =
        preview_tier_dimensions(decoded.width, decoded.height, max_px);
    let dynamic = decoded
        .into_dynamic()
        .map_err(|error| anyhow::anyhow!("preview decode failed: {error}"))?;
    let resized = if (dynamic.width(), dynamic.height()) == (target_width, target_height) {
        dynamic
    } else {
        // Same single-pass area-average downscale the thumbnail path uses.
        dynamic.thumbnail_exact(target_width, target_height)
    };
    let rgba = resized.to_rgba8();
    let encoded = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
        .encode(PREVIEW_WEBP_QUALITY);
    let bytes: &[u8] = &encoded;
    Ok(GeneratedPreviewTier {
        data: bytes.to_vec(),
        width: rgba.width() as i64,
        height: rgba.height() as i64,
        byte_size: bytes.len() as i64,
        output_format: GENERATED_FORMAT,
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core thumbnails:: 2>&1 | tail -20
cargo fmt --all --check
```

Expected: 9 new tests pass alongside the existing suite.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/thumbnails/mod.rs
git commit -m "feat(core): add adaptive preview tier profiles and webp generation"
```

---

### Task 7: `get_preview` becomes the preview-source resolver

**Files:**
- Modify: `crates/core/src/api/routes.rs` — `get_preview` (currently at the `async fn get_preview` definition), new `PreviewQuery` struct next to `ThumbnailQuery`, new pure `resolve_preview_plan`, new `serve_cached_preview_webp` helper
- Modify: `contracts/core-api/openapi.yaml:341-360` (`/media/{fileId}/preview`)
- Modify: `packages/core-client/src/client.ts` — new `fetchBlobWithTier` + `getPreviewResource`
- Modify: `packages/core-client/scripts/check-contract.mjs` — add `getPreviewResource` to the method list and one shape assertion
- Modify: `tools/checks/validate-core-api.mjs` — new resolver assertions
- Test: extend `crates/core/src/api/routes.rs`'s existing `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: Task 4's `decode::formats::{is_browser_renderable_image, lowercase_extension}`; Task 5's `db::PreviewBlobRecord` + `get_preview_blob`/`upsert_preview_blob`; Task 6's `select_preview_profile`, `preview_profile_max_px`, `PREVIEW_NATIVE_PROFILE`, `PREVIEW_NATIVE_MAX_PX`, `generate_preview_webp`.
- Produces:
  - `routes::PreviewPlan { profile: &'static str, max_px: u32 }` and `routes::resolve_preview_plan(media_kind: Option<&str>, ext: &str, source_max_px: u32, viewport_long_px: u32) -> Option<PreviewPlan>`
  - HTTP: `GET /api/media/{fileId}/preview?viewportLongPx=<u32>&v=<version>`, response header `x-mangoo-preview-tier: original | preview_2048 | preview_3072 | preview_native`
  - `@mangoo/core-client`: `PreviewResource { blob: Blob; tier: string }` and `getPreviewResource(fileId: number, options: PreviewResourceOptions = {}) => Promise<PreviewResource>`, where `PreviewResourceOptions extends BlobRequestOptions { viewportLongPx?: number }`. Tasks 9–11 consume `getPreviewResource`; the untouched `getPreviewBlob` remains the "give me the original" call used for the background sharpening upgrade.

**Three constraints this task must not break:**
1. `tools/checks/validate-core-api.mjs` asserts `functionBody("get_preview")` still contains `resolve_file_source_path`, `tokio::fs::File::open`, `Body::from_stream`, `CONTENT_TYPE`, and `preview_content_type`, and must not contain `StatusCode::ACCEPTED`. Keeping the raw-stream branch *inside* `get_preview` (rather than extracting it) satisfies all of that unchanged.
2. `packages/core-client/scripts/check-contract.mjs` pins `getPreviewBlob`'s exact source shape with a regex. Do **not** edit that method — add a sibling.
3. **Decode must never run inside the writer closure.** `run_shared_database` executes on the single `DbWriter` connection; a 300 ms decode there would block every write in the process. Decode on `spawn_blocking`, then submit only the short upsert.

**Not delivered here (stated plainly):** unplayable-video transcode streaming. Detecting "this HEVC/WMV will not play in Chromium" needs codec probing that the spec itself defers, so video keeps streaming raw bytes exactly as today. `resolve_preview_plan` returns `None` for every video, which is the plug point where that branch will go.

- [ ] **Step 1: Write the failing tests**

Add to `crates/core/src/api/routes.rs`'s existing `mod tests`:

```rust
    #[test]
    fn preview_plan_streams_the_original_for_ordinary_browser_images() {
        // The overwhelming majority path: a 3000px JPEG needs no work at all.
        assert!(resolve_preview_plan(Some("image"), ".jpg", 3000, 1920).is_none());
        assert!(resolve_preview_plan(Some("image"), ".png", 4096, 3840).is_none());
        // And a client that reports no viewport gets today's behaviour exactly.
        assert!(resolve_preview_plan(Some("image"), ".jpg", 16384, 0).is_none());
    }

    #[test]
    fn preview_plan_tiers_large_browser_images() {
        let plan = resolve_preview_plan(Some("image"), ".jpg", 16384, 1920)
            .expect("large image should be tiered");
        assert_eq!(plan.profile, PREVIEW_3072_PROFILE);
        assert_eq!(plan.max_px, 3072);
    }

    #[test]
    fn preview_plan_transcodes_unrenderable_images_even_when_small() {
        let plan = resolve_preview_plan(Some("image"), ".heic", 2000, 1920)
            .expect("heic is never browser renderable");
        assert_eq!(plan.profile, PREVIEW_NATIVE_PROFILE);
        assert_eq!(plan.max_px, PREVIEW_NATIVE_MAX_PX);
    }

    #[test]
    fn preview_plan_prefers_the_tier_over_native_for_large_unrenderable_images() {
        let plan = resolve_preview_plan(Some("image"), ".heic", 12000, 1366)
            .expect("large heic should be tiered, not transcoded at native size");
        assert_eq!(plan.profile, PREVIEW_2048_PROFILE);
        assert_eq!(plan.max_px, 2048);
    }

    #[test]
    fn preview_plan_never_touches_video() {
        // Unplayable-video transcode is deliberately out of scope; this is the
        // plug point where that branch will go.
        assert!(resolve_preview_plan(Some("video"), ".mkv", 4000, 1920).is_none());
        assert!(resolve_preview_plan(Some("video"), ".mp4", 16384, 1920).is_none());
        assert!(resolve_preview_plan(None, ".bin", 16384, 1920).is_none());
    }

    #[tokio::test]
    async fn preview_route_streams_original_bytes_when_no_viewport_is_reported() {
        let harness = preview_harness().await;

        let response = harness
            .get(&format!("/api/media/{}/preview", harness.file_id))
            .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-mangoo-preview-tier")
                .and_then(|value| value.to_str().ok()),
            Some("original")
        );
    }

    #[tokio::test]
    async fn preview_route_generates_caches_and_reuses_a_tier() {
        let harness = preview_harness_with_large_image().await;
        let path = format!(
            "/api/media/{}/preview?viewportLongPx=1366",
            harness.file_id
        );

        let first = harness.get(&path).await;
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(
            first
                .headers()
                .get("x-mangoo-preview-tier")
                .and_then(|value| value.to_str().ok()),
            Some(PREVIEW_2048_PROFILE)
        );
        assert_eq!(
            first
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/webp")
        );

        // The generated tier is now persisted, and the second request is a
        // pure cache read (no decode).
        let cached = harness
            .database()
            .get_preview_blob(harness.file_id, PREVIEW_2048_PROFILE)
            .expect("query")
            .expect("tier cached");
        assert!(cached.byte_size > 0);
        assert_eq!(cached.width, 2048);

        let second = harness.get(&path).await;
        assert_eq!(second.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn preview_route_regenerates_a_tier_whose_source_changed() {
        let harness = preview_harness_with_large_image().await;
        let path = format!(
            "/api/media/{}/preview?viewportLongPx=1366",
            harness.file_id
        );
        harness.get(&path).await;

        // Poison the cached tier's staleness key, as a real edit would.
        harness
            .database()
            .upsert_preview_blob(PreviewBlobRecord {
                source_size: 1,
                source_mtime: 1,
                ..harness
                    .database()
                    .get_preview_blob(harness.file_id, PREVIEW_2048_PROFILE)
                    .expect("query")
                    .expect("row")
            })
            .expect("poison");

        let response = harness.get(&path).await;
        assert_eq!(response.status(), StatusCode::OK);

        let refreshed = harness
            .database()
            .get_preview_blob(harness.file_id, PREVIEW_2048_PROFILE)
            .expect("query")
            .expect("row");
        assert_ne!(refreshed.source_size, 1, "stale tier must be regenerated");
    }
```

> **Implementer note:** `preview_harness()` / `preview_harness_with_large_image()` are thin wrappers over whatever router+temp-DB fixture the surrounding `routes::tests` module already uses (see the existing `unique_temp_dir()`-based tests). Build the router the same way the neighbouring route tests do, seed one real image file on disk (640×480 for the plain harness, 5000×500 for the large one, written with `image::DynamicImage::save`), and index it. Do not invent a new fixture framework.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core preview_plan 2>&1 | tail -20
cargo test -p mangoo-core preview_route 2>&1 | tail -20
```

Expected: FAIL — `cannot find function resolve_preview_plan`, `no header x-mangoo-preview-tier`.

- [ ] **Step 3a: Add the query struct and the pure resolver**

Add next to `ThumbnailQuery` in `crates/core/src/api/routes.rs`:

```rust
#[derive(Debug, Deserialize)]
struct PreviewQuery {
    /// Cache-busting fingerprint; opaque to the server.
    #[allow(dead_code)]
    v: Option<String>,
    /// Longest side of the preview viewport, in CSS pixels. Omitted by clients
    /// that want the original bytes verbatim.
    #[serde(rename = "viewportLongPx")]
    viewport_long_px: Option<u32>,
}

/// What `get_preview` decided to serve instead of the raw original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreviewPlan {
    profile: &'static str,
    max_px: u32,
}

/// Pure preview-source resolution. `None` means "stream the original file
/// bytes", which is the zero-cost majority path and the exact behaviour that
/// shipped before this pipeline existed.
fn resolve_preview_plan(
    media_kind: Option<&str>,
    ext: &str,
    source_max_px: u32,
    viewport_long_px: u32,
) -> Option<PreviewPlan> {
    // Video always streams. Detecting "Chromium cannot play this HEVC/WMV" needs
    // codec probing and a transcode pipeline, both explicitly later work; this
    // early return is that plug point.
    if media_kind != Some("image") {
        return None;
    }
    if let Some(profile) = select_preview_profile(source_max_px, viewport_long_px) {
        return Some(PreviewPlan {
            profile,
            max_px: preview_profile_max_px(profile)?,
        });
    }
    if !crate::decode::formats::is_browser_renderable_image(ext) {
        // Small enough that no tier applies, but the browser still cannot paint
        // it — transcode at (bounded) native resolution.
        return Some(PreviewPlan {
            profile: PREVIEW_NATIVE_PROFILE,
            max_px: PREVIEW_NATIVE_MAX_PX,
        });
    }
    None
}
```

- [ ] **Step 3b: Rewrite `get_preview` around the resolver**

Two small prerequisites this task also lands, because `routes.rs` has neither today:

```rust
// In crates/core/src/db/mod.rs, widen the existing private helper (there is a
// second copy in tasks.rs; leave that one alone, it is out of scope here):
pub(crate) fn unix_timestamp() -> i64 {
```

```rust
// In crates/core/src/api/routes.rs, add to the existing `impl CoreError` block
// next to bad_request / conflict / not_found:
    fn internal(message: String) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message,
            code: None,
        }
    }
```

Replace `get_preview` in `crates/core/src/api/routes.rs` with:

```rust
async fn get_preview(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Query(query): Query<PreviewQuery>,
) -> ApiResult<Response> {
    use axum::body::Body;
    use axum::http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE};
    use tokio_util::io::ReaderStream;

    let (source_path, media_kind, source_max_px) =
        run_read_database(state.clone(), move |database| {
            let media = database
                .get_media(file_id)?
                .ok_or_else(|| CoreError::not_found(format!("media item not found: {file_id}")))?;
            let source_path = database
                .resolve_file_source_path(file_id)?
                .ok_or_else(|| CoreError::not_found(format!("media source not found: {file_id}")))?;
            // `width`/`height` stay NULL until the thumbnail pass fills them in;
            // 0 then means "unknown", which resolves to "stream the original".
            let source_max_px = media
                .width
                .unwrap_or(0)
                .max(media.height.unwrap_or(0))
                .clamp(0, u32::MAX as i64) as u32;
            Ok((source_path, media.kind, source_max_px))
        })
        .await?;

    let file = tokio::fs::File::open(&source_path)
        .await
        .map_err(|err| CoreError::not_found(format!("media source file missing: {err}")))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|err| CoreError::not_found(format!("media source metadata missing: {err}")))?;
    if !metadata.is_file() {
        return Err(CoreError::not_found(format!(
            "media source is not a file: {}",
            source_path.display()
        )));
    }

    let ext = crate::decode::formats::lowercase_extension(&source_path);
    if let Some(plan) = resolve_preview_plan(
        media_kind.as_deref(),
        ext.as_str(),
        source_max_px,
        query.viewport_long_px.unwrap_or(0),
    ) {
        return serve_cached_preview_webp(state, file_id, plan, source_path, &metadata).await;
    }

    let body = Body::from_stream(ReaderStream::new(file));
    let mut response = Response::builder()
        .header(
            CONTENT_TYPE,
            preview_content_type(&source_path, media_kind.as_deref()),
        )
        .header(CACHE_CONTROL, "private, max-age=0, must-revalidate")
        .header(PREVIEW_TIER_HEADER, "original");
    response = response.header(CONTENT_LENGTH, metadata.len().to_string());
    response
        .body(body)
        .map_err(|err| CoreError::bad_request(format!("failed to build preview response: {err}")))
}

/// Tells the client WHICH representation it received, so it knows whether a
/// background full-resolution upgrade is still worth requesting.
const PREVIEW_TIER_HEADER: &str = "x-mangoo-preview-tier";

async fn serve_cached_preview_webp(
    state: AppState,
    file_id: i64,
    plan: PreviewPlan,
    source_path: std::path::PathBuf,
    metadata: &std::fs::Metadata,
) -> ApiResult<Response> {
    use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};

    let source_size = metadata.len() as i64;
    let source_mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);

    let cached = run_read_database(state.clone(), move |database| {
        Ok(database.get_preview_blob(file_id, plan.profile)?)
    })
    .await?
    .filter(|blob| blob.source_size == source_size && blob.source_mtime == source_mtime);

    let blob = match cached {
        Some(blob) => blob,
        None => {
            // Decode on the blocking pool, NEVER inside the writer closure: the
            // writer owns the process's single write connection, and a
            // multi-hundred-millisecond decode there would stall every write.
            let generation_path = source_path.clone();
            let generated = tokio::task::spawn_blocking(move || {
                crate::thumbnails::generate_preview_webp(&generation_path, plan.max_px)
            })
            .await
            .map_err(|err| CoreError::internal(format!("preview generation panicked: {err}")))?
            .map_err(|err| CoreError::internal(err.to_string()))?;

            let now = crate::db::unix_timestamp();
            let record = PreviewBlobRecord {
                file_id,
                profile: plan.profile.to_string(),
                data: generated.data,
                width: generated.width,
                height: generated.height,
                byte_size: generated.byte_size,
                output_format: generated.output_format.to_string(),
                source_size,
                source_mtime,
                created_at: now,
                updated_at: now,
            };
            let stored = record.clone();
            // Short write: one upsert plus the LRU trim.
            run_shared_database(state, move |database| {
                database.upsert_preview_blob(stored)?;
                Ok(())
            })
            .await?;
            record
        }
    };

    Response::builder()
        .header(CONTENT_TYPE, blob.output_format)
        // Same policy as the original-bytes branch: the frontend owns caching,
        // and a tier can legitimately change when the source file changes.
        .header(CACHE_CONTROL, "private, max-age=0, must-revalidate")
        .header(PREVIEW_TIER_HEADER, plan.profile)
        .body(axum::body::Body::from(blob.data))
        .map_err(|err| CoreError::bad_request(format!("failed to build preview response: {err}")))
}
```

Add the imports this needs at the top of `routes.rs` (merge into the existing `use crate::...` groups):

```rust
use crate::db::PreviewBlobRecord;
use crate::thumbnails::{
    generate_preview_webp, preview_profile_max_px, select_preview_profile, PREVIEW_NATIVE_MAX_PX,
    PREVIEW_NATIVE_PROFILE,
};
```

(`PREVIEW_2048_PROFILE` / `PREVIEW_3072_PROFILE` are only referenced from tests — import them inside `mod tests` to avoid an unused-import warning in the non-test build.)

- [ ] **Step 3c: Update the OpenAPI contract**

Replace the `/media/{fileId}/preview` block in `contracts/core-api/openapi.yaml`:

```yaml
  /media/{fileId}/preview:
    get:
      operationId: getPreview
      description: >-
        Resolves the best preview source for one media item. Browser-renderable
        images at or below the tier threshold, and every video, stream the
        original indexed bytes unchanged. Larger images return an adaptive
        downscaled WebP tier, and browser-unrenderable images return a
        transcoded WebP. The x-mangoo-preview-tier response header names which
        representation was served.
      parameters:
        - $ref: "#/components/parameters/fileId"
        - name: viewportLongPx
          in: query
          required: false
          description: >-
            Longest side of the preview viewport in CSS pixels. Omit to receive
            the original bytes verbatim.
          schema:
            type: integer
            minimum: 1
            maximum: 16384
      responses:
        "200":
          description: Original indexed media bytes, or a WebP preview representation.
          headers:
            x-mangoo-preview-tier:
              description: original | preview_2048 | preview_3072 | preview_native
              schema:
                type: string
          content:
            application/octet-stream:
              schema:
                type: string
                format: binary
            image/webp:
              schema:
                type: string
                format: binary
        "404":
          description: Media item or source file not found.
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/ErrorResponse"
```

- [ ] **Step 3d: Add the typed client call**

**Critical:** `query()` in `client.ts` is a **whitelist** — it only serialises keys it explicitly knows about, so a new parameter that is merely added to the options object is silently dropped and the server would never see a viewport. Widen `QueryParams` and `query()` first:

```typescript
type QueryParams = Partial<ListMediaParams & ListFolderChildrenParams> & {
  priority?: ThumbnailPriority;
  target?: "grid_320";
  v?: number | string | null;
  viewportLongPx?: number;
};
```

```typescript
  // inside function query(params: QueryParams), next to the "v" handling:
  if ("viewportLongPx" in params && typeof params.viewportLongPx === "number") {
    search.set("viewportLongPx", String(Math.max(1, Math.round(params.viewportLongPx))));
  }
```

Add to `packages/core-client/src/client.ts`, next to `fetchBlob`:

```typescript
export interface PreviewResourceOptions extends BlobRequestOptions {
  /** Longest side of the preview viewport in CSS pixels. */
  viewportLongPx?: number;
}

export interface PreviewResource {
  blob: Blob;
  /** original | preview_2048 | preview_3072 | preview_native */
  tier: string;
}
```

```typescript
  async function fetchBlobWithTier(
    path: string,
    options: BlobRequestOptions = {}
  ): Promise<PreviewResource> {
    const headers = new Headers();
    if (config.sessionToken) {
      headers.set("x-mangoo-session", config.sessionToken);
    }
    const response = await scheduleCoreRequest(
      options.requestPriority ?? "resource",
      options.signal,
      () =>
        fetchWithTimeout(resolveUrl(config.baseUrl, path), { headers }, options.signal),
      path
    );
    if (!response.ok) {
      const body = await readResponseBody(response);
      throw new CoreApiError(response.status, body);
    }
    return {
      blob: await response.blob(),
      tier: response.headers.get("x-mangoo-preview-tier") ?? "original"
    };
  }
```

and the method itself, immediately after `getPreviewBlob` (which stays byte-for-byte unchanged):

```typescript
    getPreviewResource: (fileId: number, options: PreviewResourceOptions = {}) =>
      fetchBlobWithTier(
        `/media/${fileId}/preview${query({
          v: options.version,
          viewportLongPx: options.viewportLongPx
        })}`,
        {
          ...options,
          requestPriority: options.requestPriority ?? "interactive"
        }
      ),
```

- [ ] **Step 3e: Extend the checks**

`packages/core-client/scripts/check-contract.mjs` — add `"getPreviewResource"` to the method list (next to `"getPreviewBlob"`), and after the existing `getPreviewBlob` assertion:

```javascript
if (!/getPreviewResource:\s*\(fileId:\s*number,\s*options:\s*PreviewResourceOptions\s*=\s*\{\}\)\s*=>\s*fetchBlobWithTier\(/.test(client)) {
  fail("client.ts getPreviewResource must request the resolved preview representation");
}
if (!/x-mangoo-preview-tier/.test(client)) {
  fail("client.ts must read the x-mangoo-preview-tier response header");
}
// The query serialiser is a whitelist; a parameter it does not know is dropped
// silently and the server never sees a viewport.
if (!/search\.set\("viewportLongPx"/.test(client)) {
  fail("client.ts query() must serialize viewportLongPx");
}
```

and, next to the other `assertOperationParameters` calls (around `:385`):

```javascript
assertOperationParameters("getPreview", ["fileId", "viewportLongPx"]);
```

`tools/checks/validate-core-api.mjs` — after the existing `previewBody` block:

```javascript
for (const value of [
  "resolve_preview_plan",
  "serve_cached_preview_webp",
  "PREVIEW_TIER_HEADER",
  "viewport_long_px"
]) {
  if (!previewBody.includes(value) && !routesRs.includes(value)) {
    fail(`GET /api/media/{fileId}/preview must resolve its source: missing ${value}`);
  }
}
const previewPlanBody = functionBody("resolve_preview_plan");
for (const value of ["select_preview_profile", "is_browser_renderable_image", "PREVIEW_NATIVE_PROFILE"]) {
  if (!previewPlanBody.includes(value)) {
    fail(`resolve_preview_plan must apply the documented rule: missing ${value}`);
  }
}
// Decode must never run on the single writer connection.
const serveBody = functionBody("serve_cached_preview_webp");
if (!serveBody.includes("spawn_blocking")) {
  fail("preview tier generation must decode on a blocking thread, not inside the writer closure");
}
if (/run_shared_database[\s\S]{0,400}generate_preview_webp/.test(serveBody)) {
  fail("preview tier generation must not decode inside the writer closure");
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p mangoo-core preview 2>&1 | tail -25
cargo fmt --all --check
npm run check:core-api
npm run check:core-client
```

Expected: the 5 pure plan tests and 3 route tests pass; both checks print PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/api/routes.rs contracts/core-api/openapi.yaml packages/core-client/src/client.ts packages/core-client/scripts/check-contract.mjs tools/checks/validate-core-api.mjs
git commit -m "feat(core): resolve preview source by format and viewport"
```

---

### Task 8: `previewBitmapPool` — the pure, budget-bounded LRU of decoded frames

**Files:**
- Create: `apps/web/src/features/preview/previewBitmapPool.ts`
- Test: `apps/web/tests/previewBitmapPool.test.mts`

**Interfaces:**
- Consumes: nothing (pure module, no DOM, no Core client).
- Produces:
  - `type PoolableBitmap = { readonly width: number; readonly height: number; close(): void }` — structural, so a real `ImageBitmap` satisfies it and a plain object satisfies it under `node --test`.
  - `previewBitmapKey(epoch: number, mediaId: number, tier: string): string`
  - `decodedBitmapBytes(bitmap: PoolableBitmap): number`
  - `PREVIEW_BITMAP_POOL_BUDGET_RATIO: number`
  - `class PreviewBitmapPool` with `constructor(budgetBytes: number)`, `configure(budgetBytes: number): void`, `get(key): PoolableBitmap | null`, `peek(key): PoolableBitmap | null`, `set(key, bitmap): void`, `has(key): boolean`, `delete(key): void`, `clear(): void`, `pin(key: string | null): void`, `get sizeBytes(): number`, `get size(): number`, `keys(): string[]`.

  Tasks 9–11 consume all of it.

**Why the budget is decoded bytes and why the pool gets 75% of the preview buffer:** `originalPreviewBlobCache` already bounds *compressed* bytes, and a decoded bitmap is 10–50× its file size — reusing that number would over-commit memory catastrophically. And once a neighbour is decoded, its compressed bytes are nearly redundant (Core re-serves them from the OS/HTTP cache), so the byte cache is a staging buffer and the bitmap pool is the thing that actually makes navigation instant. Splitting the user's one "预览缓冲区" number 25/75 honours the setting instead of silently doubling it.

**Why explicit pinning:** the frame currently on screen must never be closed underneath the canvas. LRU alone almost gets there, but one oversized neighbour admitted mid-sweep could evict it. `pin(key)` is six lines and removes the entire "preview went blank" failure class.

- [ ] **Step 1: Write the failing test**

Create `apps/web/tests/previewBitmapPool.test.mts`:

```typescript
import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);

const {
  decodedBitmapBytes,
  PreviewBitmapPool,
  previewBitmapKey
} = await import("../src/features/preview/previewBitmapPool.ts");

type FakeBitmap = { width: number; height: number; close(): void; closed: boolean };

function fakeBitmap(width: number, height: number): FakeBitmap {
  const bitmap: FakeBitmap = {
    width,
    height,
    closed: false,
    close() {
      bitmap.closed = true;
    }
  };
  return bitmap;
}

// 1000 x 1000 x 4 = 4,000,000 bytes each.
const ONE_MEGAPIXEL_BYTES = 1000 * 1000 * 4;

describe("preview bitmap pool", () => {
  it("keys by epoch, media id, and tier so a library switch can never collide", () => {
    assert.equal(previewBitmapKey(3, 42, "preview_2048"), "3:42:preview_2048");
    assert.notEqual(previewBitmapKey(3, 42, "original"), previewBitmapKey(4, 42, "original"));
    assert.notEqual(
      previewBitmapKey(3, 42, "original"),
      previewBitmapKey(3, 42, "preview_2048")
    );
  });

  it("sizes an entry by decoded bytes, not by any file size", () => {
    assert.equal(decodedBitmapBytes(fakeBitmap(1000, 1000)), ONE_MEGAPIXEL_BYTES);
    assert.equal(decodedBitmapBytes(fakeBitmap(3072, 1728)), 3072 * 1728 * 4);
  });

  it("returns what it stored", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 4);
    const bitmap = fakeBitmap(1000, 1000);
    pool.set("a", bitmap);

    assert.equal(pool.get("a"), bitmap);
    assert.equal(pool.get("missing"), null);
    assert.equal(pool.size, 1);
    assert.equal(pool.sizeBytes, ONE_MEGAPIXEL_BYTES);
  });

  it("evicts the least recently used entry and closes it", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 2);
    const first = fakeBitmap(1000, 1000);
    const second = fakeBitmap(1000, 1000);
    const third = fakeBitmap(1000, 1000);
    pool.set("first", first);
    pool.set("second", second);

    pool.set("third", third);

    assert.equal(first.closed, true, "oldest bitmap must be closed, not just dropped");
    assert.equal(pool.has("first"), false);
    assert.equal(pool.has("second"), true);
    assert.equal(pool.has("third"), true);
  });

  it("treats a read as a recency touch", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 2);
    const first = fakeBitmap(1000, 1000);
    const second = fakeBitmap(1000, 1000);
    pool.set("first", first);
    pool.set("second", second);

    pool.get("first"); // first is now the most recent
    pool.set("third", fakeBitmap(1000, 1000));

    assert.equal(pool.has("first"), true, "touched entry must survive");
    assert.equal(second.closed, true, "untouched entry is the victim");
  });

  it("peek does not touch recency", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 2);
    const first = fakeBitmap(1000, 1000);
    pool.set("first", first);
    pool.set("second", fakeBitmap(1000, 1000));

    pool.peek("first");
    pool.set("third", fakeBitmap(1000, 1000));

    assert.equal(first.closed, true, "peek must not rescue the oldest entry");
  });

  it("never evicts the pinned frame that is currently on screen", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 2);
    const displayed = fakeBitmap(1000, 1000);
    pool.set("displayed", displayed);
    pool.pin("displayed");
    pool.set("neighbor-a", fakeBitmap(1000, 1000));

    pool.set("neighbor-b", fakeBitmap(1000, 1000));
    pool.set("neighbor-c", fakeBitmap(1000, 1000));

    assert.equal(displayed.closed, false, "pinned frame must never be closed");
    assert.equal(pool.has("displayed"), true);
  });

  it("admits an oversized bitmap rather than refusing to paint", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES);
    const huge = fakeBitmap(4000, 4000); // 64 MB, way past the budget
    const small = fakeBitmap(1000, 1000);
    pool.set("small", small);

    pool.set("huge", huge);

    assert.equal(pool.has("huge"), true, "the frame the user asked for must be available");
    assert.equal(small.closed, true, "everything else makes way for it");
  });

  it("closes the previous bitmap when a key is overwritten", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 4);
    const stale = fakeBitmap(1000, 1000);
    const fresh = fakeBitmap(1000, 1000);
    pool.set("a", stale);

    pool.set("a", fresh);

    assert.equal(stale.closed, true);
    assert.equal(pool.get("a"), fresh);
    assert.equal(pool.sizeBytes, ONE_MEGAPIXEL_BYTES, "bytes must not double-count");
  });

  it("closes everything on clear, including the pinned frame", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 4);
    const displayed = fakeBitmap(1000, 1000);
    const neighbor = fakeBitmap(1000, 1000);
    pool.set("displayed", displayed);
    pool.set("neighbor", neighbor);
    pool.pin("displayed");

    pool.clear();

    assert.equal(displayed.closed, true, "a library switch releases even the pinned frame");
    assert.equal(neighbor.closed, true);
    assert.equal(pool.size, 0);
    assert.equal(pool.sizeBytes, 0);
  });

  it("closes on explicit delete", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 4);
    const bitmap = fakeBitmap(1000, 1000);
    pool.set("a", bitmap);

    pool.delete("a");

    assert.equal(bitmap.closed, true);
    assert.equal(pool.sizeBytes, 0);
  });

  it("shrinking the budget evicts down to the new limit immediately", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 4);
    const oldest = fakeBitmap(1000, 1000);
    pool.set("oldest", oldest);
    pool.set("newest", fakeBitmap(1000, 1000));

    pool.configure(ONE_MEGAPIXEL_BYTES);

    assert.equal(oldest.closed, true);
    assert.equal(pool.sizeBytes, ONE_MEGAPIXEL_BYTES);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
npm --workspace @mangoo/web run test 2>&1 | tail -20
```

Expected: FAIL — `Cannot find module '../src/features/preview/previewBitmapPool.ts'`.

- [ ] **Step 3: Write the implementation**

Create `apps/web/src/features/preview/previewBitmapPool.ts`:

```typescript
// A budget-bounded LRU of DECODED preview frames.
//
// This is the accelerator behind instant prev/next: a pooled frame paints with
// a synchronous `drawImage`, while an unpooled one costs a full-resolution
// decode (measured: 337ms for a 16384px image, 323ms for 10032px).
//
// Pure on purpose — no DOM, no Core client — so `node --test` can drive the
// whole eviction/lifecycle contract with plain objects. `ImageBitmap`
// structurally satisfies `PoolableBitmap`.

/** Anything with pixels and an explicit release. A real `ImageBitmap` fits. */
export type PoolableBitmap = {
  readonly width: number;
  readonly height: number;
  close(): void;
};

// Bitmaps are RGBA8 in GPU/CPU memory regardless of how well the source file
// compressed. This is the ONLY correct unit for this budget: the sibling
// `originalPreviewBlobCache` counts compressed bytes, and a 3 MB JPEG is 130 MB
// decoded.
const BYTES_PER_DECODED_PIXEL = 4;

// Share of the user's "预览缓冲区" budget that goes to decoded frames rather
// than to the compressed-byte cache. Decoded frames are what actually make
// navigation instant; the byte cache is only their staging buffer, and Core
// re-serves those bytes cheaply from the OS cache.
export const PREVIEW_BITMAP_POOL_BUDGET_RATIO = 0.75;

/**
 * Cache key. The epoch comes from `mediaCacheEpochToken()` so a library switch
 * can never serve the previous library's frame for a reused file id — the same
 * "串图" guard the thumbnail caches already use. The tier is part of the key
 * because a media item legitimately has both a `preview_2048` frame and a
 * sharper `original` one.
 */
export function previewBitmapKey(epoch: number, mediaId: number, tier: string): string {
  return `${epoch}:${mediaId}:${tier}`;
}

export function decodedBitmapBytes(bitmap: PoolableBitmap): number {
  return Math.max(0, bitmap.width) * Math.max(0, bitmap.height) * BYTES_PER_DECODED_PIXEL;
}

type PoolEntry = {
  bitmap: PoolableBitmap;
  bytes: number;
};

export class PreviewBitmapPool {
  // Insertion order IS recency order: JS Map preserves it, and re-inserting
  // moves a key to the end. Same technique the thumbnail object-URL cache uses.
  private readonly entries = new Map<string, PoolEntry>();
  private budgetBytes: number;
  private currentBytes = 0;
  private pinnedKey: string | null = null;

  constructor(budgetBytes: number) {
    this.budgetBytes = Math.max(0, Math.floor(budgetBytes));
  }

  get size(): number {
    return this.entries.size;
  }

  get sizeBytes(): number {
    return this.currentBytes;
  }

  keys(): string[] {
    return [...this.entries.keys()];
  }

  configure(budgetBytes: number): void {
    this.budgetBytes = Math.max(0, Math.floor(budgetBytes));
    this.evictUntilWithinBudget();
  }

  /** Marks the frame currently painted on the canvas, which is never evicted. */
  pin(key: string | null): void {
    this.pinnedKey = key;
  }

  has(key: string): boolean {
    return this.entries.has(key);
  }

  /** Read + recency touch. Use this on the paint path. */
  get(key: string): PoolableBitmap | null {
    const entry = this.entries.get(key);
    if (!entry) {
      return null;
    }
    this.entries.delete(key);
    this.entries.set(key, entry);
    return entry.bitmap;
  }

  /** Read WITHOUT touching recency. Use this for "is it warm?" probes. */
  peek(key: string): PoolableBitmap | null {
    return this.entries.get(key)?.bitmap ?? null;
  }

  set(key: string, bitmap: PoolableBitmap): void {
    this.delete(key);
    const bytes = decodedBitmapBytes(bitmap);
    this.entries.set(key, { bitmap, bytes });
    this.currentBytes += bytes;
    this.evictUntilWithinBudget();
  }

  delete(key: string): void {
    const entry = this.entries.get(key);
    if (!entry) {
      return;
    }
    this.entries.delete(key);
    this.currentBytes -= entry.bytes;
    // An ImageBitmap that is dropped without close() leaks GPU memory until GC
    // decides otherwise, which on a browsing session is never.
    entry.bitmap.close();
  }

  /** Releases every frame, pinned included. This is the library-switch path. */
  clear(): void {
    for (const entry of this.entries.values()) {
      entry.bitmap.close();
    }
    this.entries.clear();
    this.currentBytes = 0;
    this.pinnedKey = null;
  }

  private evictUntilWithinBudget(): void {
    for (const key of [...this.entries.keys()]) {
      if (this.currentBytes <= this.budgetBytes) {
        return;
      }
      // The pinned frame is on screen right now; closing it would blank the
      // canvas. The newest entry is the one the caller just asked for, so
      // evicting it would defeat the request that triggered this sweep.
      if (key === this.pinnedKey || this.entries.size <= 1) {
        continue;
      }
      this.delete(key);
    }
  }
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
npm --workspace @mangoo/web run test 2>&1 | tail -20
npm --workspace @mangoo/web run typecheck
```

Expected: 12 new assertions pass alongside the existing web suite; typecheck clean.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/features/preview/previewBitmapPool.ts apps/web/tests/previewBitmapPool.test.mts
git commit -m "feat(web): add budget-bounded decoded preview bitmap pool"
```

---

### Task 9: Worker decode, the pooled bitmap source, and library-switch release

**Files:**
- Create: `apps/web/src/features/preview/previewDecodeWorker.ts`
- Create: `apps/web/src/features/preview/previewBitmapSource.ts`
- Modify: `apps/web/src/core/mediaResources.ts` — add a reset-listener registry and call it from `resetMediaCachesForLibrarySwitch`
- Test: `apps/web/tests/previewBitmapSource.test.mts`; extend `apps/web/tests/mediaResources.test.mts`

**Interfaces:**
- Consumes: Task 8's pool; Task 7's `getPreviewResource`; existing `mediaCacheEpochToken()` from `core/mediaResources.ts`.
- Produces:
  - `previewBitmapSource`: `type DecodeRequestMessage = { id: number; blob: Blob }`; `type DecodeResponseMessage = { id: number; ok: true; bitmap: PoolableBitmap } | { id: number; ok: false; error: string }`; `type DecodeTransport = { post(message: DecodeRequestMessage, transfer: Transferable[]): void; subscribe(handler: (message: DecodeResponseMessage) => void): void }`; `createDecodeDispatcher(transport: DecodeTransport): { decode(blob: Blob, signal?: AbortSignal): Promise<PoolableBitmap>; rejectAll(reason: Error): void; pendingCount(): number }`
  - `previewBitmapPool: PreviewBitmapPool` (the app-wide singleton), `configurePreviewBitmapBudget(previewBufferBytes: number): void`, `requestPreviewBitmap(media, options): Promise<PooledPreviewFrame>`, `prefetchPreviewBitmap(media, options): void`, `readPooledPreviewFrame(media, tier): PoolableBitmap | null`, `readAnyPooledPreviewFrame(media): PooledPreviewFrame | null`, `releasePreviewBitmaps(): void`, `pinPreviewFrame(key: string | null): void`
  - `type PooledPreviewFrame = { bitmap: PoolableBitmap; tier: string; key: string }`
  - `type PreviewBitmapRequestOptions = { viewportLongPx?: number; original?: boolean; resourcePriority?: "preview" | "ahead"; signal?: AbortSignal }`
  - `mediaResources`: `registerMediaCacheResetListener(listener: () => void): () => void`

  Tasks 10–11 consume `requestPreviewBitmap`, `prefetchPreviewBitmap`, `readPooledPreviewFrame`, `pinPreviewFrame`, `configurePreviewBitmapBudget`.

**Why a listener registry instead of `mediaResources` importing the pool:** `core/` must not depend on `features/`. `resetMediaCachesForLibrarySwitch` already exists and already aborts in-flight requests; a one-line listener fan-out lets the preview feature attach its `.close()`-everything without inverting the layering or creating an import cycle.

**Why the Worker is lazy and has a main-thread fallback:** creating it at module load would break `node --test` (no `Worker`) and would pay the cost for users who never open a preview. `createImageBitmap` on the main thread is the fallback when `Worker` construction throws — slower, but correct, and it keeps the feature working if the CSP or the bundler ever changes underneath.

**CSP:** the Worker MUST be constructed as `new Worker(new URL("./previewDecodeWorker.ts", import.meta.url), { type: "module" })`. `apps/web/vite.config.ts` ships a CSP without `worker-src`, which falls back to `default-src 'self'` — a same-origin module URL is allowed and a `blob:` worker is blocked.

- [ ] **Step 1: Write the failing tests**

Create `apps/web/tests/previewBitmapSource.test.mts`:

```typescript
import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);
(globalThis as typeof globalThis & { window: Record<string, unknown> }).window = {
  mangooDesktop: { coreUrl: "http://127.0.0.1:47321/api" }
};

const { createDecodeDispatcher } = await import(
  "../src/features/preview/previewBitmapSource.ts"
);

type FakeBitmap = { width: number; height: number; close(): void; closed: boolean };

function fakeBitmap(): FakeBitmap {
  const bitmap: FakeBitmap = {
    width: 10,
    height: 10,
    closed: false,
    close() {
      bitmap.closed = true;
    }
  };
  return bitmap;
}

function fakeTransport() {
  const sent: { id: number }[] = [];
  let handler: ((message: unknown) => void) | null = null;
  return {
    sent,
    transport: {
      post(message: { id: number }) {
        sent.push(message);
      },
      subscribe(next: (message: unknown) => void) {
        handler = next;
      }
    },
    reply(message: unknown) {
      handler?.(message);
    }
  };
}

describe("preview decode dispatcher", () => {
  it("resolves each request with the bitmap carrying its id", async () => {
    const { transport, sent, reply } = fakeTransport();
    const dispatcher = createDecodeDispatcher(transport as never);
    const first = dispatcher.decode(new Blob(["a"]));
    const second = dispatcher.decode(new Blob(["b"]));

    assert.equal(sent.length, 2);
    const firstBitmap = fakeBitmap();
    const secondBitmap = fakeBitmap();
    // Reply out of order: correlation must be by id, not by arrival.
    reply({ id: sent[1].id, ok: true, bitmap: secondBitmap });
    reply({ id: sent[0].id, ok: true, bitmap: firstBitmap });

    assert.equal(await first, firstBitmap);
    assert.equal(await second, secondBitmap);
    assert.equal(dispatcher.pendingCount(), 0);
  });

  it("rejects with the worker's error text", async () => {
    const { transport, sent, reply } = fakeTransport();
    const dispatcher = createDecodeDispatcher(transport as never);
    const pending = dispatcher.decode(new Blob(["a"]));

    reply({ id: sent[0].id, ok: false, error: "unsupported image" });

    await assert.rejects(pending, /unsupported image/);
    assert.equal(dispatcher.pendingCount(), 0);
  });

  it("closes a bitmap that arrives after its request was aborted", async () => {
    const { transport, sent, reply } = fakeTransport();
    const dispatcher = createDecodeDispatcher(transport as never);
    const controller = new AbortController();
    const pending = dispatcher.decode(new Blob(["a"]), controller.signal);

    controller.abort();
    await assert.rejects(pending, /abort/i);

    const late = fakeBitmap();
    reply({ id: sent[0].id, ok: true, bitmap: late });
    assert.equal(late.closed, true, "an orphaned bitmap must not leak GPU memory");
  });

  it("rejects immediately when the signal is already aborted", async () => {
    const { transport, sent } = fakeTransport();
    const dispatcher = createDecodeDispatcher(transport as never);
    const controller = new AbortController();
    controller.abort();

    await assert.rejects(dispatcher.decode(new Blob(["a"]), controller.signal), /abort/i);
    assert.equal(sent.length, 0, "an aborted request must not reach the worker");
  });

  it("rejectAll fails every in-flight decode, for worker death or library switch", async () => {
    const { transport } = fakeTransport();
    const dispatcher = createDecodeDispatcher(transport as never);
    const pending = dispatcher.decode(new Blob(["a"]));

    dispatcher.rejectAll(new Error("decode worker terminated"));

    await assert.rejects(pending, /terminated/);
    assert.equal(dispatcher.pendingCount(), 0);
  });
});
```

Add to `apps/web/tests/mediaResources.test.mts` (extend the existing destructured import with `registerMediaCacheResetListener`):

```typescript
describe("media cache reset listeners", () => {
  it("notifies registered listeners on a library switch", () => {
    let calls = 0;
    const unregister = registerMediaCacheResetListener(() => {
      calls += 1;
    });

    resetMediaCachesForLibrarySwitch();
    assert.equal(calls, 1);

    unregister();
    resetMediaCachesForLibrarySwitch();
    assert.equal(calls, 1, "an unregistered listener must not fire again");
  });

  it("keeps clearing caches even if a listener throws", () => {
    const unregister = registerMediaCacheResetListener(() => {
      throw new Error("listener exploded");
    });
    let secondRan = false;
    const unregisterSecond = registerMediaCacheResetListener(() => {
      secondRan = true;
    });

    resetMediaCachesForLibrarySwitch();

    assert.equal(secondRan, true, "one bad listener must not strand the rest");
    unregister();
    unregisterSecond();
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
npm --workspace @mangoo/web run test 2>&1 | tail -20
```

Expected: FAIL — `Cannot find module '../src/features/preview/previewBitmapSource.ts'` and `registerMediaCacheResetListener is not a function`.

- [ ] **Step 3a: The Worker**

Create `apps/web/src/features/preview/previewDecodeWorker.ts`:

```typescript
/// <reference lib="webworker" />

// Decodes preview bytes into an ImageBitmap OFF the main thread.
//
// This is the whole point of the pipeline: measured on the real library, a
// full-resolution main-thread decode costs 337ms (16384px) / 323ms (10032px)
// per navigation step. Here it costs the main thread only a postMessage.
//
// The bitmap is TRANSFERRED (not copied) back, so ownership moves to the main
// thread and nothing is duplicated in memory.

type DecodeRequestMessage = { id: number; blob: Blob };

const worker = self as unknown as DedicatedWorkerGlobalScope;

worker.onmessage = async (event: MessageEvent<DecodeRequestMessage>) => {
  const { id, blob } = event.data;
  try {
    const bitmap = await createImageBitmap(blob);
    worker.postMessage({ id, ok: true, bitmap }, [bitmap]);
  } catch (cause) {
    worker.postMessage({
      id,
      ok: false,
      error: cause instanceof Error ? cause.message : String(cause)
    });
  }
};
```

- [ ] **Step 3b: The reset-listener registry**

In `apps/web/src/core/mediaResources.ts`, add next to `mediaCacheEpochToken`:

```typescript
// Consumers outside `core/` (the preview bitmap pool lives in `features/`) need
// to release resources on a library switch. A listener registry keeps the
// dependency pointing the right way — core never imports a feature.
const mediaCacheResetListeners = new Set<() => void>();

export function registerMediaCacheResetListener(listener: () => void): () => void {
  mediaCacheResetListeners.add(listener);
  return () => {
    mediaCacheResetListeners.delete(listener);
  };
}
```

and append to the end of `resetMediaCachesForLibrarySwitch`:

```typescript
  for (const listener of [...mediaCacheResetListeners]) {
    try {
      listener();
    } catch {
      // One listener failing must never strand the rest of the invalidation:
      // a half-cleared cache is exactly the cross-library "串图" bug.
    }
  }
```

- [ ] **Step 3c: The pooled bitmap source**

Create `apps/web/src/features/preview/previewBitmapSource.ts`:

```typescript
import type { MediaRecord } from "@mangoo/core-client";

import { createCoreClient } from "../../core/client";
import {
  mediaCacheEpochToken,
  registerMediaCacheResetListener
} from "../../core/mediaResources";
import {
  PreviewBitmapPool,
  PREVIEW_BITMAP_POOL_BUDGET_RATIO,
  previewBitmapKey,
  type PoolableBitmap
} from "./previewBitmapPool";

export type DecodeRequestMessage = { id: number; blob: Blob };
export type DecodeResponseMessage =
  | { id: number; ok: true; bitmap: PoolableBitmap }
  | { id: number; ok: false; error: string };

export type DecodeTransport = {
  post(message: DecodeRequestMessage, transfer: Transferable[]): void;
  subscribe(handler: (message: DecodeResponseMessage) => void): void;
};

export type PooledPreviewFrame = {
  bitmap: PoolableBitmap;
  tier: string;
  key: string;
};

export type PreviewBitmapRequestOptions = {
  /** Longest side of the preview viewport, in CSS pixels. */
  viewportLongPx?: number;
  /** Ask Core for the untiered original — the background sharpening upgrade. */
  original?: boolean;
  resourcePriority?: "preview" | "ahead";
  signal?: AbortSignal;
};

const ORIGINAL_TIER = "original";
const client = createCoreClient();

/**
 * Correlates decode replies to their requests by id. Extracted from the Worker
 * plumbing so the interesting behaviour — out-of-order replies, abort, orphaned
 * bitmaps, transport death — is unit-testable without a browser.
 */
export function createDecodeDispatcher(transport: DecodeTransport) {
  type Pending = {
    resolve: (bitmap: PoolableBitmap) => void;
    reject: (reason: Error) => void;
    aborted: boolean;
  };
  const pending = new Map<number, Pending>();
  let nextId = 0;

  transport.subscribe((message) => {
    const entry = pending.get(message.id);
    if (!entry) {
      // Nothing is waiting: the request was already settled. If a bitmap still
      // arrived, release it — an unreferenced ImageBitmap leaks until GC.
      if (message.ok) {
        message.bitmap.close();
      }
      return;
    }
    pending.delete(message.id);
    if (!message.ok) {
      entry.reject(new Error(message.error));
      return;
    }
    if (entry.aborted) {
      message.bitmap.close();
      return;
    }
    entry.resolve(message.bitmap);
  });

  return {
    decode(blob: Blob, signal?: AbortSignal): Promise<PoolableBitmap> {
      if (signal?.aborted) {
        return Promise.reject(new DOMException("aborted", "AbortError"));
      }
      const id = (nextId += 1);
      return new Promise<PoolableBitmap>((resolve, reject) => {
        const entry: Pending = { resolve, reject, aborted: false };
        pending.set(id, entry);
        signal?.addEventListener(
          "abort",
          () => {
            // Keep the entry registered so the late bitmap can be closed above;
            // only the caller's promise settles now.
            entry.aborted = true;
            reject(new DOMException("aborted", "AbortError"));
          },
          { once: true }
        );
        transport.post({ id, blob }, []);
      });
    },
    rejectAll(reason: Error): void {
      for (const entry of [...pending.values()]) {
        entry.reject(reason);
      }
      pending.clear();
    },
    pendingCount(): number {
      return pending.size;
    }
  };
}

type Dispatcher = ReturnType<typeof createDecodeDispatcher>;
let dispatcher: Dispatcher | null = null;

function workerDispatcher(): Dispatcher | null {
  if (dispatcher) {
    return dispatcher;
  }
  if (typeof Worker === "undefined") {
    return null;
  }
  try {
    // Same-origin module URL: allowed by the app CSP (which has no worker-src
    // and therefore falls back to default-src 'self'). A blob: worker would be
    // blocked outright.
    const worker = new Worker(new URL("./previewDecodeWorker.ts", import.meta.url), {
      type: "module"
    });
    let onMessage: ((message: DecodeResponseMessage) => void) | null = null;
    worker.onmessage = (event: MessageEvent<DecodeResponseMessage>) => onMessage?.(event.data);
    const created = createDecodeDispatcher({
      post: (message) => worker.postMessage(message),
      subscribe: (handler) => {
        onMessage = handler;
      }
    });
    worker.onerror = () => created.rejectAll(new Error("preview decode worker failed"));
    dispatcher = created;
    return dispatcher;
  } catch {
    // Bundler or CSP refused the worker; fall back to main-thread decoding
    // rather than losing preview entirely.
    return null;
  }
}

async function decodeBlob(blob: Blob, signal?: AbortSignal): Promise<PoolableBitmap> {
  const worker = workerDispatcher();
  if (worker) {
    return worker.decode(blob, signal);
  }
  if (signal?.aborted) {
    throw new DOMException("aborted", "AbortError");
  }
  return (await createImageBitmap(blob)) as unknown as PoolableBitmap;
}

export const previewBitmapPool = new PreviewBitmapPool(0);

/** Splits the user's preview-buffer budget; see the pool module for the ratio. */
export function configurePreviewBitmapBudget(previewBufferBytes: number): void {
  previewBitmapPool.configure(
    Math.max(0, Math.floor(previewBufferBytes * PREVIEW_BITMAP_POOL_BUDGET_RATIO))
  );
}

export function pinPreviewFrame(key: string | null): void {
  previewBitmapPool.pin(key);
}

export function readPooledPreviewFrame(
  media: MediaRecord,
  tier: string
): PoolableBitmap | null {
  return previewBitmapPool.get(previewBitmapKey(mediaCacheEpochToken(), media.id, tier));
}

// Sharpest first: an already-upgraded original beats a tier, and a bigger tier
// beats a smaller one. This is the synchronous "can I paint RIGHT NOW?" probe
// the canvas runs before it awaits anything.
const TIER_PREFERENCE = [ORIGINAL_TIER, "preview_3072", "preview_2048", "preview_native"];

export function readAnyPooledPreviewFrame(media: MediaRecord): PooledPreviewFrame | null {
  const epoch = mediaCacheEpochToken();
  for (const tier of TIER_PREFERENCE) {
    const key = previewBitmapKey(epoch, media.id, tier);
    const bitmap = previewBitmapPool.get(key);
    if (bitmap) {
      return { bitmap, tier, key };
    }
  }
  return null;
}

export async function requestPreviewBitmap(
  media: MediaRecord,
  options: PreviewBitmapRequestOptions = {}
): Promise<PooledPreviewFrame> {
  const epoch = mediaCacheEpochToken();
  if (options.original) {
    const pooledOriginal = previewBitmapPool.get(
      previewBitmapKey(epoch, media.id, ORIGINAL_TIER)
    );
    if (pooledOriginal) {
      return { bitmap: pooledOriginal, tier: ORIGINAL_TIER, key: previewBitmapKey(epoch, media.id, ORIGINAL_TIER) };
    }
  }

  const resource = await client.getPreviewResource(media.id, {
    requestPriority: options.resourcePriority === "ahead" ? "resource" : "interactive",
    signal: options.signal,
    viewportLongPx: options.original ? undefined : options.viewportLongPx
  });

  const key = previewBitmapKey(epoch, media.id, resource.tier);
  const pooled = previewBitmapPool.get(key);
  if (pooled) {
    return { bitmap: pooled, tier: resource.tier, key };
  }

  const bitmap = await decodeBlob(resource.blob, options.signal);
  // A library switch during the decode invalidates this frame entirely: the
  // epoch moved, so pooling it under the old key would strand memory nothing
  // will ever read.
  if (mediaCacheEpochToken() !== epoch) {
    bitmap.close();
    throw new DOMException("aborted", "AbortError");
  }
  previewBitmapPool.set(key, bitmap);
  return { bitmap, tier: resource.tier, key };
}

export function prefetchPreviewBitmap(
  media: MediaRecord,
  options: PreviewBitmapRequestOptions = {}
): void {
  void requestPreviewBitmap(media, { ...options, resourcePriority: "ahead" }).catch(
    () => undefined
  );
}

export function releasePreviewBitmaps(): void {
  previewBitmapPool.clear();
  dispatcher?.rejectAll(new Error("library switched"));
}

// A library switch must release every decoded frame: ImageBitmaps do not free
// themselves, and the next library reuses the same file ids.
registerMediaCacheResetListener(releasePreviewBitmaps);
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
npm --workspace @mangoo/web run test 2>&1 | tail -20
npm --workspace @mangoo/web run typecheck
npm run check:web
```

Expected: 7 new assertions pass; typecheck clean; `check:web` prints PASS (no raw `fetch(`, no `node:fs`, no duplicate contract types).

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/features/preview/previewDecodeWorker.ts apps/web/src/features/preview/previewBitmapSource.ts apps/web/src/core/mediaResources.ts apps/web/tests/previewBitmapSource.test.mts apps/web/tests/mediaResources.test.mts
git commit -m "feat(web): decode preview frames in a worker into the bitmap pool"
```

---

### Task 10: Paint the preview from a stable `<canvas>`

**Files:**
- Create: `apps/web/src/features/preview/PreviewCanvas.tsx`
- Create: `apps/web/src/features/preview/previewRenderLane.ts`
- Modify: `apps/web/src/features/preview/MediaPreview.tsx` — `ReadyPreviewMedia` dispatches the static-image original lane to `PreviewCanvas`
- Modify: `apps/web/src/styles.css` — `.preview-canvas` shares `.preview-image`'s box
- Test: `apps/web/tests/previewRenderLane.test.mts`

**Interfaces:**
- Consumes: Task 9's `requestPreviewBitmap`, `readAnyPooledPreviewFrame`, `pinPreviewFrame`.
- Produces:
  - `previewRenderLane`: `type PreviewRenderLane = "canvas" | "image" | "video"`; `selectPreviewRenderLane(input: { source: "thumbnail" | "original"; kind?: string | null; ext?: string | null }): PreviewRenderLane`; `ANIMATION_CAPABLE_EXTENSIONS: readonly string[]`; `canvasPaintTarget(bitmap: {width: number; height: number}, natural: {naturalWidth: number; naturalHeight: number} | null): { backingWidth: number; backingHeight: number; cssWidth: number; cssHeight: number }`
  - `PreviewCanvas` React component.

  Task 11 consumes nothing new from here.

**The whole reason this is fast, and the one thing not to break:** zoom/pan is a CSS transform on the wrapper div `.central-preview-transform` (`CentralPreviewStage.tsx:697`), not on the media element. So the gesture state machine — `scale`, `viewMode`, `zoomAtPoint`, wheel, pinch, double-click — is **completely untouched** by this task. The canvas is simply a differently-painted element of the same intrinsic size, and zooming stays GPU-composited with no repaint.

**Backing store vs CSS box.** `canvas.width/height` (the backing store) is the **bitmap's** size; `style.width/height` is the **media's natural** size in px, exactly what the `<img>` used. A 16384×8192 backing store would be 536 MB of GPU memory; a `preview_3072` backing store is 21 MB and the browser scales it into the natural box for free. Keeping the CSS box at natural size is also what makes "actual pixels" zoom still mean actual pixels.

**Lane selection is by extension, and `.gif`/`.webp`/`.avif` stay on `<img>`.** A canvas freezes an animated image on frame one. Byte-sniffing the WebP `ANIM` chunk / AVIF `avis` brand would let still WebP/AVIF onto the fast lane too, but it costs a probe on the interactive path for formats whose originals are already small and fast to decode — the measured problem is multi-megabyte PNG/JPEG. Revisit only if a large still WebP shows up slow in practice.

- [ ] **Step 1: Write the failing test**

Create `apps/web/tests/previewRenderLane.test.mts`:

```typescript
import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);

const { canvasPaintTarget, selectPreviewRenderLane } = await import(
  "../src/features/preview/previewRenderLane.ts"
);

describe("preview render lane", () => {
  it("sends still images to the canvas fast path", () => {
    for (const ext of ["jpg", "jpeg", "png", "bmp", "tif", "tiff", "heic", "psd"]) {
      assert.equal(
        selectPreviewRenderLane({ source: "original", kind: "image", ext }),
        "canvas",
        ext
      );
    }
  });

  it("keeps animation-capable formats on <img> so they still animate", () => {
    for (const ext of ["gif", "webp", "avif", "GIF", ".WebP"]) {
      assert.equal(
        selectPreviewRenderLane({ source: "original", kind: "image", ext }),
        "image",
        ext
      );
    }
  });

  it("keeps video on <video> and thumbnails on <img>", () => {
    assert.equal(
      selectPreviewRenderLane({ source: "original", kind: "video", ext: "mp4" }),
      "video"
    );
    // The thumbnail fallback is a tiny 320px WebP; pooling it would waste the
    // budget and it is already instant.
    assert.equal(
      selectPreviewRenderLane({ source: "thumbnail", kind: "image", ext: "jpg" }),
      "image"
    );
  });

  it("falls back to <img> when the kind or extension is unknown", () => {
    assert.equal(selectPreviewRenderLane({ source: "original", kind: null, ext: "jpg" }), "image");
    assert.equal(
      selectPreviewRenderLane({ source: "original", kind: "image", ext: null }),
      "image"
    );
  });

  it("sizes the backing store to the bitmap and the css box to natural pixels", () => {
    const target = canvasPaintTarget(
      { width: 3072, height: 1536 },
      { naturalWidth: 16384, naturalHeight: 8192 }
    );

    // 21 MB of backing store instead of 536 MB, and the layout box is still the
    // original's, so "actual pixels" zoom stays honest.
    assert.deepEqual(target, {
      backingWidth: 3072,
      backingHeight: 1536,
      cssWidth: 16384,
      cssHeight: 8192
    });
  });

  it("falls back to the bitmap size when natural dimensions are still pending", () => {
    const target = canvasPaintTarget({ width: 2048, height: 1024 }, null);

    assert.deepEqual(target, {
      backingWidth: 2048,
      backingHeight: 1024,
      cssWidth: 2048,
      cssHeight: 1024
    });
  });

  it("ignores non-positive natural dimensions", () => {
    const target = canvasPaintTarget(
      { width: 800, height: 600 },
      { naturalWidth: 0, naturalHeight: 0 }
    );

    assert.equal(target.cssWidth, 800);
    assert.equal(target.cssHeight, 600);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
npm --workspace @mangoo/web run test 2>&1 | tail -20
```

Expected: FAIL — `Cannot find module '../src/features/preview/previewRenderLane.ts'`.

- [ ] **Step 3a: The pure lane module**

Create `apps/web/src/features/preview/previewRenderLane.ts`:

```typescript
export type PreviewRenderLane = "canvas" | "image" | "video";

// Formats that may carry more than one frame. A canvas paints exactly one
// bitmap, so these must stay on <img> or they freeze on frame one. Mirrors
// `decode::formats::ANIMATION_CAPABLE_IMAGE_EXTENSIONS` in Core.
export const ANIMATION_CAPABLE_EXTENSIONS = ["gif", "webp", "avif"] as const;

function normalizeExtension(ext?: string | null): string {
  if (!ext) {
    return "";
  }
  return ext.replace(/^\./, "").toLowerCase();
}

/**
 * Which element paints this preview.
 *
 * Only the ORIGINAL of a still image takes the canvas fast path. The thumbnail
 * fallback is a 320px WebP that decodes instantly and would only waste pool
 * budget; video needs a real `<video>`; animation-capable formats need `<img>`.
 */
export function selectPreviewRenderLane(input: {
  source: "thumbnail" | "original";
  kind?: string | null;
  ext?: string | null;
}): PreviewRenderLane {
  if (input.kind === "video") {
    return "video";
  }
  if (input.source !== "original" || input.kind !== "image") {
    return "image";
  }
  const ext = normalizeExtension(input.ext);
  if (!ext || (ANIMATION_CAPABLE_EXTENSIONS as readonly string[]).includes(ext)) {
    return "image";
  }
  return "canvas";
}

/**
 * Backing store = the decoded bitmap's size (a 16384x8192 backing store would
 * be 536 MB of GPU memory; a preview_3072 one is 21 MB). CSS box = the media's
 * NATURAL size, which is exactly the box the `<img>` occupied, so the wrapper's
 * `translate(...) scale(...)` transform and "actual pixels" zoom keep working
 * without a single change.
 */
export function canvasPaintTarget(
  bitmap: { width: number; height: number },
  natural: { naturalWidth: number; naturalHeight: number } | null
): { backingWidth: number; backingHeight: number; cssWidth: number; cssHeight: number } {
  const hasNatural =
    !!natural && natural.naturalWidth > 0 && natural.naturalHeight > 0;
  return {
    backingWidth: bitmap.width,
    backingHeight: bitmap.height,
    cssWidth: hasNatural ? natural.naturalWidth : bitmap.width,
    cssHeight: hasNatural ? natural.naturalHeight : bitmap.height
  };
}
```

- [ ] **Step 3b: The canvas component**

Create `apps/web/src/features/preview/PreviewCanvas.tsx`:

```tsx
import { useEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";

import type { MediaRecord } from "@mangoo/core-client";

import {
  pinPreviewFrame,
  readAnyPooledPreviewFrame,
  requestPreviewBitmap
} from "./previewBitmapSource";
import { canvasPaintTarget } from "./previewRenderLane";
import type { PoolableBitmap } from "./previewBitmapPool";

// Paints the preview from a PRE-DECODED ImageBitmap.
//
// The canvas element is deliberately stable across navigation — no `key` — so
// React never remounts it. That is the entire fix for issue 4: the old path
// mounted a fresh `<img key={src}>` per frame, and a decoded bitmap does not
// transfer to a new element even with the same object URL, so every step paid a
// full-resolution main-thread decode (337ms at 16384px).

export function PreviewCanvas({
  alt,
  frameStyle,
  media,
  onLoadingChange,
  onMediaReady,
  onNaturalSize,
  viewportLongPx
}: {
  alt: string;
  frameStyle?: CSSProperties;
  media: MediaRecord;
  onLoadingChange?: (loading: boolean) => void;
  onMediaReady?: () => void;
  onNaturalSize?: (size: { naturalHeight: number; naturalWidth: number }) => void;
  viewportLongPx: number;
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [boxStyle, setBoxStyle] = useState<CSSProperties | undefined>(undefined);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let disposed = false;
    const controller = new AbortController();
    setFailed(false);

    const natural =
      media.width && media.height && media.width > 0 && media.height > 0
        ? { naturalHeight: media.height, naturalWidth: media.width }
        : null;

    const paint = (bitmap: PoolableBitmap, key: string) => {
      const canvas = canvasRef.current;
      if (!canvas || disposed) {
        return;
      }
      const target = canvasPaintTarget(bitmap, natural);
      if (canvas.width !== target.backingWidth || canvas.height !== target.backingHeight) {
        // Assigning width/height also clears the canvas.
        canvas.width = target.backingWidth;
        canvas.height = target.backingHeight;
      }
      const context = canvas.getContext("2d");
      if (!context) {
        setFailed(true);
        return;
      }
      // Explicit clear: when the dimensions happen to match, a transparent PNG
      // would otherwise composite over the previous frame.
      context.clearRect(0, 0, canvas.width, canvas.height);
      context.drawImage(bitmap as unknown as CanvasImageSource, 0, 0);
      setBoxStyle({ height: `${target.cssHeight}px`, width: `${target.cssWidth}px` });
      // Never let the LRU close the frame that is on screen right now.
      pinPreviewFrame(key);
      onNaturalSize?.({ naturalHeight: target.cssHeight, naturalWidth: target.cssWidth });
      onLoadingChange?.(false);
      onMediaReady?.();
    };

    // 1. Synchronous path. A pooled neighbour paints in this very tick — no
    //    await, no decode, no loading flash. This is the BandiView feel.
    const warm = readAnyPooledPreviewFrame(media);
    if (warm) {
      paint(warm.bitmap, warm.key);
    } else {
      onLoadingChange?.(true);
    }

    // 2. Resolve the intended representation. When the warm frame already was
    //    it, `requestPreviewBitmap` returns the same pooled bitmap and this
    //    repaint is a no-op.
    void requestPreviewBitmap(media, { signal: controller.signal, viewportLongPx })
      .then((frame) => {
        if (disposed) {
          return;
        }
        paint(frame.bitmap, frame.key);
        // 3. Sharpening upgrade — only when what we painted really is a
        //    DOWNSCALE of the original. `original` is already sharp, and
        //    `preview_native` is a format transcode at native size, so asking
        //    for an upgrade there would just re-fetch the identical bytes.
        if (frame.tier === "original" || frame.tier === "preview_native") {
          return;
        }
        // The tier is what makes navigation instant and memory bounded; the
        // original is what keeps zooming honest. Fetch it behind the scenes and
        // repaint when it lands.
        void requestPreviewBitmap(media, {
          original: true,
          resourcePriority: "ahead",
          signal: controller.signal
        })
          .then((sharp) => {
            if (!disposed) {
              paint(sharp.bitmap, sharp.key);
            }
          })
          .catch(() => undefined);
      })
      .catch((cause) => {
        if (disposed || controller.signal.aborted) {
          return;
        }
        if (cause instanceof DOMException && cause.name === "AbortError") {
          return;
        }
        setFailed(true);
        onLoadingChange?.(false);
        onMediaReady?.();
      });

    return () => {
      disposed = true;
      controller.abort();
    };
  }, [
    media,
    onLoadingChange,
    onMediaReady,
    onNaturalSize,
    viewportLongPx
  ]);

  useEffect(() => () => pinPreviewFrame(null), []);

  if (failed) {
    return (
      <div className="preview-placeholder failed" style={boxStyle ?? frameStyle}>
        <span>load error</span>
      </div>
    );
  }

  return (
    <div className="preview-placeholder ready" data-preview-source="original" style={boxStyle ?? frameStyle}>
      <canvas
        aria-label={alt}
        className="preview-image preview-canvas"
        data-preview-media-id={media.id}
        ref={canvasRef}
        role="img"
        style={boxStyle}
      />
    </div>
  );
}
```

- [ ] **Step 3c: Dispatch to it from `MediaPreview`**

`MediaPreview` is **already** a dispatcher — a sequence of early returns, with every hook (`useThumbnailFallbackUrl` and friends) called above them. The lane branch goes there, immediately **before** the existing `if (source === "original" || shouldUseOriginalWhilePending)` block at `MediaPreview.tsx:73`. Putting it inside `ReadyPreviewMedia` instead would require an early return above that component's ten `useState`/`useRef`/`useEffect` calls — legal only by an argument about prop stability that no linter can check. Do not do that.

Add the imports:

```typescript
import { PreviewCanvas } from "./PreviewCanvas";
import { selectPreviewRenderLane } from "./previewRenderLane";
```

and insert, directly above the `if (source === "original" || shouldUseOriginalWhilePending)` return:

```tsx
  // Gate on the LITERAL `source` prop, not on `shouldUseOriginalWhilePending`.
  // The inspector panel (PreviewPanel.tsx) renders source="thumbnail" with
  // preferOriginalWhilePending, and routing that small sidebar surface into the
  // bitmap pool would spend the preview budget on thumbnails.
  if (
    source === "original" &&
    selectPreviewRenderLane({ source, kind: media.kind, ext: media.ext }) === "canvas"
  ) {
    return (
      <PreviewCanvas
        alt={media.name}
        frameStyle={stableFrameStyle}
        media={media}
        onLoadingChange={onLoadingChange}
        onMediaReady={onMediaReady}
        onNaturalSize={onNaturalSize}
        viewportLongPx={previewViewportLongPx()}
      />
    );
  }
```

`ReadyPreviewMedia` itself is **not modified by this task** — it keeps serving the thumbnail lane, the animated-image lane, and video, exactly as today.

Add the viewport helper at the bottom of `MediaPreview.tsx` (exported, because Task 11's prefetch effect needs the same number):

```typescript
// Longest side of the window in device pixels, which is what the
// fit-long-edge preview fills. Core turns this into a tier choice; 0 means
// "unknown", which Core reads as "send the original bytes".
export function previewViewportLongPx(): number {
  if (typeof window === "undefined") {
    return 0;
  }
  const longest = Math.max(window.innerWidth, window.innerHeight);
  return Math.max(1, Math.round(longest * (window.devicePixelRatio || 1)));
}
```

- [ ] **Step 3d: Style the canvas like the image it replaces**

In `apps/web/src/styles.css`, next to the existing `.preview-image` rule:

```css
/* The canvas occupies the same box as the <img> it replaces; the wrapper's
   transform still owns zoom and pan. `display: block` avoids the inline
   baseline gap that would shift the frame by a pixel or two. */
.preview-canvas {
  display: block;
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
npm --workspace @mangoo/web run test 2>&1 | tail -20
npm --workspace @mangoo/web run typecheck
npm run check:web
npm run check:ui-design
```

Expected: 7 new lane assertions pass; typecheck clean; both checks PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/features/preview/PreviewCanvas.tsx apps/web/src/features/preview/previewRenderLane.ts apps/web/src/features/preview/MediaPreview.tsx apps/web/src/styles.css apps/web/tests/previewRenderLane.test.mts
git commit -m "feat(web): paint still-image previews from a stable canvas"
```

---

### Task 11: Pool-driven neighbour prefetch and removal of the navigation lock

**Files:**
- Modify: `apps/web/src/features/preview/previewBitmapSource.ts` — add `configurePreviewBuffers`
- Modify: `apps/web/src/app/App.tsx` — prefetch effect (`:212-230`), buffer configuration (`:470-478`), navigation gating (`:122-123`, `:203-210`, `:539-578`)
- Test: `apps/web/tests/previewNavigationGate.test.mts`; extend `apps/web/tests/previewPrefetchWindow.test.mts`

**Interfaces:**
- Consumes: Task 8's `PREVIEW_BITMAP_POOL_BUDGET_RATIO`; Task 9's `prefetchPreviewBitmap`, `configurePreviewBitmapBudget`; Task 10's `selectPreviewRenderLane`; existing `buildPreviewPrefetchWindow`, `prefetchOriginalPreview`, `configureOriginalPreviewBuffer`.
- Produces: `previewBitmapSource.configurePreviewBuffers(previewBufferBytes: number): void`; `app/previewNavigationGate.ts` with `canNavigatePreview(input: { previewOpen: boolean; selectedMediaIndex: number; itemCount: number; direction: "previous" | "next" }): boolean`.

**Why removing the lock is safe now, and was not before.** The lock existed because navigating before the previous frame committed produced a torn/blank stage: every step cost a fresh full-resolution decode, so without the lock a held-down arrow key queued unbounded decodes. With the pool, a pooled neighbour paints synchronously in the same tick, and an unpooled one leaves the previously painted canvas on screen untouched until its bitmap lands. There is nothing left to tear. `AbortController` in `PreviewCanvas` cancels the outrun request, so a held arrow key cancels rather than queues.

**Why prefetch splits by lane:** the canvas lane fetches its own bytes through `getPreviewResource` (tier-aware), so also calling `prefetchOriginalPreview` for those neighbours would double-fetch the original for nothing. Animated images still take the `<img>` lane and still want the byte cache warmed.

- [ ] **Step 1: Write the failing tests**

Create `apps/web/tests/previewNavigationGate.test.mts`:

```typescript
import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);

const { canNavigatePreview } = await import("../src/app/previewNavigationGate.ts");

describe("preview navigation gate", () => {
  it("allows stepping while a frame is still decoding", () => {
    // The whole point of removing the lock: a held-down arrow key must keep
    // moving instead of waiting on each decode.
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: 4,
        itemCount: 31,
        direction: "next"
      }),
      true
    );
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: 4,
        itemCount: 31,
        direction: "previous"
      }),
      true
    );
  });

  it("stops at both ends of the list", () => {
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: 0,
        itemCount: 31,
        direction: "previous"
      }),
      false
    );
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: 30,
        itemCount: 31,
        direction: "next"
      }),
      false
    );
  });

  it("refuses to navigate with no selection or an empty list", () => {
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: -1,
        itemCount: 31,
        direction: "next"
      }),
      false
    );
    assert.equal(
      canNavigatePreview({
        previewOpen: true,
        selectedMediaIndex: 0,
        itemCount: 0,
        direction: "next"
      }),
      false
    );
  });

  it("is inert while the preview is closed", () => {
    assert.equal(
      canNavigatePreview({
        previewOpen: false,
        selectedMediaIndex: 4,
        itemCount: 31,
        direction: "next"
      }),
      false
    );
  });
});
```

Add to `apps/web/tests/previewPrefetchWindow.test.mts`:

```typescript
describe("prefetch lane split", () => {
  it("routes still images to the bitmap pool and animated ones to the byte cache", async () => {
    const { selectPreviewRenderLane } = await import(
      "../src/features/preview/previewRenderLane.ts"
    );
    // Every member of a prefetch window is an image; the lane decides which
    // warm-up call it gets, and a still image must never be double-fetched.
    assert.equal(
      selectPreviewRenderLane({ source: "original", kind: "image", ext: "png" }),
      "canvas"
    );
    assert.equal(
      selectPreviewRenderLane({ source: "original", kind: "image", ext: "gif" }),
      "image"
    );
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
npm --workspace @mangoo/web run test 2>&1 | tail -20
```

Expected: FAIL — `Cannot find module '../src/app/previewNavigationGate.ts'`.

- [ ] **Step 3a: The pure gate**

Create `apps/web/src/app/previewNavigationGate.ts`:

```typescript
// Whether a prev/next step is possible RIGHT NOW.
//
// Deliberately depends only on position, never on whether the current frame has
// finished painting. The old `previewNavigationLockedRef` gate existed because
// each step cost a full-resolution main-thread decode, so an unlocked held
// arrow key queued unbounded work. The pre-decoded bitmap pool removed that
// cost, and `PreviewCanvas` aborts a request it outran, so the lock is now pure
// input lag.
export function canNavigatePreview(input: {
  previewOpen: boolean;
  selectedMediaIndex: number;
  itemCount: number;
  direction: "previous" | "next";
}): boolean {
  if (!input.previewOpen || input.itemCount <= 0 || input.selectedMediaIndex < 0) {
    return false;
  }
  return input.direction === "previous"
    ? input.selectedMediaIndex > 0
    : input.selectedMediaIndex < input.itemCount - 1;
}
```

- [ ] **Step 3b: One place that splits the preview buffer**

Add to `apps/web/src/features/preview/previewBitmapSource.ts` (merge the import into the existing `../../core/mediaResources` import added in Task 9):

```typescript
import { configureOriginalPreviewBuffer } from "../../core/mediaResources";

/**
 * Applies the user's one "预览缓冲区" number to BOTH preview caches.
 *
 * The decoded-bitmap pool takes the larger share because it is what actually
 * makes navigation instant; the compressed-byte cache is its staging buffer and
 * Core re-serves those bytes cheaply. Splitting here (rather than handing the
 * same number to both) keeps the setting honest instead of silently doubling
 * the app's memory ceiling.
 */
export function configurePreviewBuffers(previewBufferBytes: number): void {
  configurePreviewBitmapBudget(previewBufferBytes);
  configureOriginalPreviewBuffer(
    Math.max(0, Math.floor(previewBufferBytes * (1 - PREVIEW_BITMAP_POOL_BUDGET_RATIO)))
  );
}
```

- [ ] **Step 3c: Rewire `App.tsx`**

Imports — add:

```typescript
import {
  configurePreviewBuffers,
  prefetchPreviewBitmap
} from "../features/preview/previewBitmapSource";
import { selectPreviewRenderLane } from "../features/preview/previewRenderLane";
import { canNavigatePreview } from "./previewNavigationGate";
```

and remove `configureOriginalPreviewBuffer` from the `../core/mediaResources` import list (`configurePreviewBuffers` now owns that call). Keep `prefetchOriginalPreview`.

Replace the navigation-readiness block (`:203-210`):

```tsx
  const canPreviewPrevious = canNavigatePreview({
    previewOpen,
    selectedMediaIndex,
    itemCount: orderedPreviewMedia.length,
    direction: "previous"
  });
  const canPreviewNext = canNavigatePreview({
    previewOpen,
    selectedMediaIndex,
    itemCount: orderedPreviewMedia.length,
    direction: "next"
  });
```

Delete `const previewNavigationReady = ...` and the `previewNavigationLockedRef` declaration (`:123`). Keep `previewDisplayedMediaId` — `CentralPreviewStage` still reports settle, and the state is read elsewhere.

Replace the prefetch effect (`:212-230`):

```tsx
  useEffect(() => {
    if (!previewOpen || selectedMediaIndex < 0) return;
    const controller = new AbortController();
    const bufferBytes = previewBufferLimitMbToBytes(previewPreferences.previewBufferLimitMb);
    const prefetchWindow = buildPreviewPrefetchWindow(
      orderedPreviewMedia,
      selectedMediaIndex,
      bufferBytes
    );
    const viewportLongPx = previewViewportLongPx();
    for (const neighbor of prefetchWindow) {
      // Still images are warmed as DECODED bitmaps — that is what makes the
      // next step paint in one tick. Animated ones keep the <img> lane, so warm
      // their compressed bytes instead; warming both would double-fetch.
      if (
        selectPreviewRenderLane({
          source: "original",
          kind: neighbor.kind,
          ext: neighbor.ext
        }) === "canvas"
      ) {
        prefetchPreviewBitmap(neighbor, { signal: controller.signal, viewportLongPx });
      } else {
        prefetchOriginalPreview(neighbor, { signal: controller.signal });
      }
    }
    return () => controller.abort();
  }, [
    orderedPreviewMedia,
    previewOpen,
    previewPreferences.previewBufferLimitMb,
    selectedMediaIndex
  ]);
```

Export the viewport helper from `MediaPreview.tsx` so both files share one definition (Task 10 already declares it `export function`) and import it in `App.tsx`:

```typescript
import { previewViewportLongPx } from "../features/preview/MediaPreview";
```

Replace the buffer-configuration call (`:472-474`):

```tsx
    configurePreviewBuffers(
      previewBufferLimitMbToBytes(previewPreferences.previewBufferLimitMb)
    );
```

Replace `handleOpenPreview` and `startPreviewNavigation` (`:539-558`):

```tsx
  const handleOpenPreview = useCallback(
    (mediaId: number) => {
      setPreviewDisplayedMediaId(null);
      library.setSelectedMediaId(mediaId);
      setPreviewViewCommands(null);
      setPreviewOpen(true);
    },
    [library]
  );

  function startPreviewNavigation(target: MediaRecord | undefined) {
    if (!target) return;
    // No readiness gate: a pooled neighbour paints in this tick, and an
    // unpooled one leaves the current canvas on screen until its bitmap lands.
    library.setSelectedMediaId(target.id);
  }
```

and drop `previewNavigationLockedRef.current = false;` from `handleClosePreview` (`:235`) and from `handlePreviewMediaSettled` (`:576`). Remove `previewNavigationReady` from the `handlePreviewPrevious`/`handlePreviewNext` dependency arrays (`:564`, `:570`).

- [ ] **Step 4: Run the tests to verify they pass**

```bash
npm --workspace @mangoo/web run test 2>&1 | tail -20
npm --workspace @mangoo/web run typecheck
npm run check:web
```

Expected: 5 new assertions pass; typecheck clean (no unused `previewNavigationLockedRef`); `check:web` PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/web/src/app/previewNavigationGate.ts apps/web/src/app/App.tsx apps/web/src/features/preview/previewBitmapSource.ts apps/web/src/features/preview/MediaPreview.tsx apps/web/tests/previewNavigationGate.test.mts apps/web/tests/previewPrefetchWindow.test.mts
git commit -m "feat(web): prefetch decoded neighbours and drop the preview nav lock"
```

---

### Task 12: Full gate and real-app verification

**Files:**
- Create: `docs/benchmarks/2026-07-24-media-decode-preview-pipeline-verification.md`
- Modify: `CLAUDE.md` — record the new migration, the decode module, and the preview tier rules

**Interfaces:**
- Consumes: everything above.
- Produces: a benchmark record with real measured numbers, not estimates.

**This task is not optional and its numbers may not be guessed.** The stated goal is "~8s → toward BandiView's ~2s" over `C:\Users\84460\Pictures\normal`; a plan that ships without measuring has not delivered it.

- [ ] **Step 1: Run the full gate**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
npm test 2>&1 | tail -40
```

Expected: every stage prints PASS. Fix anything that does not before continuing — do not proceed with a red gate.

- [ ] **Step 2: Launch the real app against the benchmark folder**

```bash
MANGOO_AUTO_ADD_ROOT="C:/Users/84460/Pictures/normal" MANGOO_REMOTE_DEBUG=1 npm run dev
```

Wait for the grid to populate and thumbnails to finish (so `media.width/height` are real, which is what the tier selector reads).

- [ ] **Step 3: Measure continuous navigation**

Open the first image, then hold the Right arrow key to walk all 31 images. Time the walk with a stopwatch, three times, and record the median. Then, over CDP, confirm the mechanism rather than just the wall-clock:

```bash
WS=$(curl -sS http://127.0.0.1:9500/json | python -c "import json,sys; print([p for p in json.load(sys.stdin) if p['type']=='page'][0]['webSocketDebuggerUrl'])")
node tools/dev/cdp-inspect.mjs "$WS"
```

Record in the benchmark doc:
- median wall-clock for the 31-image walk, before (~8s, from the existing measurement) and after
- that the preview stage renders `canvas.preview-canvas`, not `img.preview-image`
- `x-mangoo-preview-tier` values actually observed in the network log
- `performance.memory.usedJSHeapSize` after 200+ continuous navigation steps, sampled twice five minutes apart, to show the ImageBitmap pool stabilises rather than grows

- [ ] **Step 4: Verify format coverage and memory release**

- Drop one `.avif`, one `.heic`, and one `.psd` into the root, rescan, and record which now produce a grid thumbnail and a viewable preview, and what error text the ones that still fail report (it must name the decoder, not "unsupported format").
- Switch libraries with a preview open, then switch back, and confirm the previous library's frames are gone (heap does not retain them) and no cross-library image appears.
- Zoom past 100% on a >4096px image and confirm it sharpens (the background original upgrade landed) rather than staying soft.

- [ ] **Step 5: Update the project docs**

In `CLAUDE.md`:
- Contract-First Boundaries: bump "SQLite schema is current through **0025**" and name `crates/core/migrations/0025_preview_blobs.sql`.
- Rust Core Layout: add `decode` to the module list.
- Thumbnail and Preview Rules: record that all decoding flows through `crates/core/src/decode/`'s capability-ordered registry (image crate → ffmpeg), that `GET /api/media/{fileId}/preview` resolves its source by format and `viewportLongPx`, that tiers are longest-side `preview_2048`/`preview_3072` plus `preview_native` cached in `preview_blobs` under a 2 GiB LRU, and that the preview surface paints a stable `<canvas>` from a Worker-decoded `ImageBitmap` pool (75% of the preview-buffer budget) with no navigation lock.

- [ ] **Step 6: Commit**

```bash
git add docs/benchmarks/2026-07-24-media-decode-preview-pipeline-verification.md CLAUDE.md
git commit -m "docs: record media decode and preview pipeline verification"
```

---

## Plan-level self-review notes

Recorded so the implementer knows what was already checked, and what to watch:

- **Spec coverage.** §3a → Tasks 1–3; §3b → Tasks 5–7; §3c → Tasks 8–11; §4's `classify_thumbnail_failure` and duplicated `media_kind` → Task 4; §5's routing table → Task 7's `resolve_preview_plan`; §6's pool, `.close()` lifecycle, and no-lock placeholder swap → Tasks 8–11; §7's testing list → the per-task tests plus Task 12.
- **Knowingly not delivered** (all explicitly "later" in the spec, restated here so nobody expects them): `PsdDecoder`/`RawDecoder` as pure-Rust crates — `.psd` is routed to ffmpeg speculatively and `.raw` will report `no decoder handles`; video transcode streaming for unplayable codecs — `resolve_preview_plan` returns `None` for all video and that early return is the plug point; archives/documents/3D.
- **Three deliberate deviations from the spec, each justified in its task:** a new `preview_blobs` table instead of widening `thumb_blobs` (18 GB rebuild); tiers keyed by longest side with a `viewportLongPx` parameter instead of short side / `viewportShortPx` (bounds decoded memory unconditionally and matches `fit-long-edge`); two tiers plus `preview_native` instead of three tiers.
- **No zoom/rotate work appears anywhere in this plan** — verified that `.central-preview-transform` owns the transform, so the gesture state machine is untouched by design, not by omission.
- **Guarded checks this plan edits on purpose:** `validate-core-api.mjs` (preview markers + new resolver assertions + migration include), `check-contract.mjs` (new `getPreviewResource`, `getPreviewBlob` left byte-identical so its pinned regex still matches), `validate_sqlite_schema.py` (migration, table, index, columns).
- **Highest-risk task is 10** (introducing a second render lane into a component tree that already has six). Its Step 3c pins the branch to `MediaPreview`'s existing early-return chain — above `ReadyPreviewMedia`, whose hooks must not be reordered — and gates on the literal `source` prop so the inspector panel (`PreviewPanel.tsx`, `source="thumbnail"` + `preferOriginalWhilePending`) never enters the bitmap pool.
