//! Pluggable media decode chain.
//!
//! Every Core decode (thumbnail generation, preview transcode) asks this
//! registry for RGBA pixels instead of calling `image::open` directly, so
//! adding a format family is adding one decoder to the chain and nothing else.
//! The chain is a responsibility chain with **graceful fall-through**: a
//! decoder that claims an extension but fails does not end the attempt, the
//! next claiming decoder runs. That is what lets `image` lose on an exotic
//! TIFF variant and ffmpeg still produce a picture.

pub mod exif;
pub mod ffmpeg;
pub mod formats;
pub mod image_crate;
pub mod psd;
pub mod rawler;
pub mod subprocess;

use std::fmt::Write as _;
use std::path::Path;

/// Hard ceiling on source pixels any in-process decoder may expand.
///
/// The check must run before allocating a full RGBA/developed raster. Rust OOM
/// aborts the whole Core process, so a blocking task or semaphore cannot turn an
/// unbounded allocation into an ordinary decode error.
pub(super) const MAX_DECODE_PIXELS: u64 = 268_000_000;

pub(super) fn ensure_decode_pixel_budget(
    decoder: &str,
    width: u64,
    height: u64,
) -> anyhow::Result<()> {
    if width == 0 || height == 0 {
        anyhow::bail!("{decoder} decode: zero-sized source image");
    }
    let pixels = width.checked_mul(height).ok_or_else(|| {
        anyhow::anyhow!("{decoder} decode: source dimensions overflow ({width}x{height})")
    })?;
    if pixels > MAX_DECODE_PIXELS {
        anyhow::bail!(
            "{decoder} decode: source is too large to decode ({width}x{height}, \
             {pixels} pixels exceeds the {MAX_DECODE_PIXELS} pixel budget)"
        );
    }
    Ok(())
}

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
            let outcome =
                decoder
                    .decode(request)
                    .and_then(|decoded| match decoded.expected_len() {
                        Some(expected) if expected == decoded.rgba.len() => Ok(decoded),
                        _ => Err(anyhow::anyhow!(
                            "decoder returned {} bytes for {}x{}",
                            decoded.rgba.len(),
                            decoded.width,
                            decoded.height
                        )),
                    });
            match outcome {
                Ok(decoded) => return Ok(decoded),
                Err(error) => {
                    // Fall through to the next claiming decoder. Every failure
                    // text is preserved verbatim in the aggregate error so
                    // `classify_thumbnail_failure` still sees a transient
                    // signature contributed by ANY link in the chain. Use the
                    // alternate `{:#}` Display, which walks the whole `anyhow`
                    // context chain — plain `{}` prints only the outermost
                    // frame and would silently drop a leaf cause like
                    // `os error 53` behind a decoder's `.context(...)` call.
                    if !report.is_empty() {
                        report.push_str("; ");
                    }
                    let _ = write!(report, "{}: {error:#}", decoder.name());
                }
            }
        }
        if attempts == 0 {
            return Err(anyhow::anyhow!("{NO_DECODER_MARKER} \"{}\"", request.ext));
        }
        Err(anyhow::anyhow!(
            "all decoders failed for \"{}\": {report}",
            request.ext
        ))
    }
}

impl DecoderRegistry {
    /// Registered decoder names in chain order. Exposed so a test can assert
    /// the capability-ascending ordering that the whole design depends on.
    #[cfg(test)]
    pub fn decoder_names(&self) -> Vec<&'static str> {
        self.decoders.iter().map(|decoder| decoder.name()).collect()
    }
}

/// The production chain, ordered capability-ascending: cheapest and most
/// commonly correct first, so the JPEG/PNG majority never pays a subprocess.
pub fn default_registry() -> DecoderRegistry {
    DecoderRegistry::new(vec![
        Box::new(image_crate::ImageCrateDecoder),
        Box::new(psd::PsdDecoder),
        Box::new(rawler::RawlerDecoder),
        Box::new(ffmpeg::FfmpegDecoder),
    ])
}

/// Process-wide chain. Construction is trivial, but the thumbnail worker asks
/// for it once per task on a hot path, so build it once.
pub fn shared_registry() -> &'static DecoderRegistry {
    static REGISTRY: std::sync::OnceLock<DecoderRegistry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(default_registry)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn production_registry_includes_native_psd_and_raw_before_ffmpeg() {
        assert_eq!(
            default_registry().decoder_names(),
            vec!["image_crate", "psd", "rawler", "ffmpeg"]
        );
    }

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
    fn decode_pixel_budget_rejects_zero_sized_and_oversized_rasters() {
        let zero = ensure_decode_pixel_budget("test", 0, 10)
            .expect_err("zero-sized rasters must be rejected before allocation");
        assert!(
            zero.to_string().contains("zero-sized source image"),
            "{zero}"
        );

        let oversized = ensure_decode_pixel_budget("test", 268_000_001, 1)
            .expect_err("over-budget rasters must be rejected before allocation");
        assert!(
            oversized
                .to_string()
                .contains("source is too large to decode"),
            "{oversized}"
        );
    }

    #[test]
    fn decode_pixel_budget_accepts_the_existing_ceiling() {
        ensure_decode_pixel_budget("test", 268_000_000, 1)
            .expect("the established maximum pixel budget remains decodable");
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
    fn aggregate_error_keeps_the_leaf_cause_behind_a_context_frame() {
        static ONLY: AtomicUsize = AtomicUsize::new(0);
        let registry = DecoderRegistry::new(vec![Box::new(FakeDecoder {
            name: "ffmpeg",
            claims: &[".heic"],
            // Exactly the shape a real decoder produces: a readable outer message
            // wrapping the OS error that carries the transient signature.
            result: || {
                Err(anyhow::anyhow!("找不到网络路径。 (os error 53)")
                    .context("ffmpeg subprocess failed"))
            },
            calls: &ONLY,
        })]);

        let text = registry
            .decode(&request(Path::new("a.heic"), ".heic"))
            .expect_err("chain failed")
            .to_string();

        // Without the leaf, classify_thumbnail_failure would mark a retryable
        // network drop as a permanent unsupported-format failure.
        assert!(text.contains("ffmpeg subprocess failed"), "{text}");
        assert!(text.contains("os error 53"), "{text}");
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
