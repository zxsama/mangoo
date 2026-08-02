//! The default, fastest link in the chain: the `image` crate with its default
//! features. Registered FIRST so JPEG/PNG/GIF/WebP/BMP/common-TIFF — the
//! overwhelming majority of any real library — never pay a subprocess spawn.

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader};

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

/// Hard ceiling on how many pixels this decoder will allocate for, checked from
/// the file HEADER before a single pixel is read.
///
/// Decoding is `width * height * 4` bytes of RGBA plus, historically, a second
/// buffer of the same size. A 20000x20000 scanned TIFF is ~1.2 GB + a ~1.6 GB
/// clone live at once, which on the preview route's unbounded blocking pool is
/// an OOM abort — and a Rust OOM kills the whole Core process, not just the
/// request.
///
/// The rejection message deliberately avoids every substring in
/// `classify_thumbnail_failure`'s transient list. An oversized image is a
/// PERMANENT property of the file: classifying it transient would arm the 60s
/// retry sweep to re-attempt a doomed gigapixel decode forever.
const MAX_DECODE_PIXELS: u64 = 268_000_000;

pub struct ImageCrateDecoder;

fn rejects_zero_dimensions(width: u32, height: u32) -> anyhow::Result<()> {
    if width == 0 || height == 0 {
        return Err(anyhow::anyhow!("zero-sized source image"));
    }
    Ok(())
}

/// Header-only guard. Returns the source dimensions when they are decodable
/// within budget. Cheap: `image_dimensions` parses the header and stops.
fn checked_source_dimensions(path: &std::path::Path) -> anyhow::Result<(u32, u32)> {
    let (width, height) = image::image_dimensions(path)
        .map_err(|error| anyhow::anyhow!("image_crate dimensions: {error}"))?;
    rejects_zero_dimensions(width, height)
        .map_err(|error| anyhow::anyhow!("image_crate dimensions: {error}"))?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_DECODE_PIXELS {
        return Err(anyhow::anyhow!(
            "image_crate decode: source is too large to decode ({width}x{height}, \
             {pixels} pixels exceeds the {MAX_DECODE_PIXELS} pixel budget)"
        ));
    }
    Ok((width, height))
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
        checked_source_dimensions(path)?;

        // `with_guessed_format` sniffs the content, so a mis-extensioned file
        // still decodes — the extension only decides WHICH decoder is asked.
        let reader = ImageReader::open(path)
            .map_err(|error| anyhow::anyhow!("image_crate open: {error}"))?
            .with_guessed_format()
            .map_err(|error| anyhow::anyhow!("image_crate format: {error}"))?;

        // Read EXIF orientation from the decoder, then apply it, so this
        // decoder returns DISPLAY-oriented pixels.
        //
        // This is not cosmetic. Every consumer downstream assumes one
        // orientation: `media.width/height` come from these dimensions and drive
        // the grid's aspect ratios, and the browser's own `createImageBitmap`
        // applies EXIF unconditionally. Returning raw raster here made a rotated
        // phone photo show sideways in the grid, and made the preview canvas
        // size a portrait bitmap into a landscape box. It also disagreed with
        // the ffmpeg decoder, which autorotates by default — so the same photo
        // shot as HEIC and as JPEG produced transposed dimensions.
        let mut decoder = reader
            .into_decoder()
            .map_err(|error| anyhow::anyhow!("image_crate decoder: {error}"))?;
        let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
        let mut decoded = DynamicImage::from_decoder(decoder)
            .map_err(|error| anyhow::anyhow!("image_crate decode: {error}"))?;
        decoded.apply_orientation(orientation);

        rejects_zero_dimensions(decoded.width(), decoded.height())
            .map_err(|error| anyhow::anyhow!("image_crate decode: {error}"))?;

        let (width, height) = (decoded.width(), decoded.height());
        // `into_rgba8` is the one normalisation every consumer relies on: it
        // flattens paletted, grayscale, and 16-bit decodes into the single
        // product type, which is also exactly what `webp::Encoder` accepts.
        // `into_` rather than `to_` so an already-RGBA8 decode moves instead of
        // copying — that copy is up to 67 MB on a large source.
        Ok(DecodedImage {
            rgba: decoded.into_rgba8().into_raw(),
            width,
            height,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::decode::MediaSource;

    fn unique_temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "megle_decode_image_{}_{}_{}",
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
        for ext in [
            ".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".tif", ".tiff",
        ] {
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

    /// Writes a landscape JPEG carrying EXIF orientation 6 ("rotate 90 CW to
    /// display"), i.e. raw raster `width x height` but display `height x width`.
    fn write_jpeg_with_exif_orientation_6(dir: &std::path::Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).expect("create temp dir");
        let mut buffer = image::RgbaImage::new(40, 20);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgba([10, 20, 30, 255]);
        }
        let plain = dir.join("plain.jpg");
        image::DynamicImage::ImageRgba8(buffer)
            .to_rgb8()
            .save(&plain)
            .expect("encode base jpeg");
        let bytes = std::fs::read(&plain).expect("read base jpeg");

        // Minimal little-endian TIFF IFD carrying only Orientation(0x0112) = 6.
        let mut tiff: Vec<u8> = Vec::new();
        tiff.extend_from_slice(b"II* ");
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&6u16.to_le_bytes());
        tiff.extend_from_slice(&0u16.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());

        let mut app1: Vec<u8> = b"Exif  ".to_vec();
        app1.extend_from_slice(&tiff);
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&bytes[0..2]); // SOI
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&bytes[2..]);

        let path = dir.join(name);
        std::fs::write(&path, out).expect("write exif jpeg");
        path
    }

    #[test]
    fn applies_exif_orientation_so_dimensions_match_what_a_browser_decodes() {
        // The browser's createImageBitmap applies EXIF unconditionally. When
        // this decoder returned raw raster, media.width/height disagreed with
        // it: the grid showed rotated phone photos sideways and the preview
        // canvas sized a portrait bitmap into a landscape box.
        let dir = unique_temp_dir();
        let path = write_jpeg_with_exif_orientation_6(&dir, "rotated.jpg");

        let decoded = ImageCrateDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(&path),
                ext: ".jpg",
                target_max_px: None,
                purpose: DecodePurpose::Thumbnail,
            })
            .expect("decodes");

        assert_eq!(
            (decoded.width, decoded.height),
            (20, 40),
            "orientation 6 must present the 40x20 raster as 20x40"
        );
        assert_eq!(decoded.rgba.len(), 20 * 40 * 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_image_without_exif_is_unchanged() {
        let dir = unique_temp_dir();
        let path = write_image(&dir, "plain.png", 30, 12);

        let decoded = ImageCrateDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(&path),
                ext: ".png",
                target_max_px: None,
                purpose: DecodePurpose::Thumbnail,
            })
            .expect("decodes");

        assert_eq!((decoded.width, decoded.height), (30, 12));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_an_oversized_source_from_the_header_without_decoding() {
        // A 20000x20000 TIFF is ~1.2 GB decoded plus a same-size clone; on the
        // preview route's unbounded blocking pool that is an OOM abort, which
        // kills the whole Core process rather than failing one request.
        assert!(super::MAX_DECODE_PIXELS < 20000u64 * 20000u64);

        // And the rejection must NOT read as transient, or the 60s retry sweep
        // would re-attempt a doomed gigapixel decode forever.
        let message = format!(
            "image_crate decode: source is too large to decode (20000x20000,              400000000 pixels exceeds the {} pixel budget)",
            super::MAX_DECODE_PIXELS
        );
        assert!(matches!(
            crate::thumbnails::classify_thumbnail_failure(&message),
            crate::thumbnails::ThumbnailFailureKind::Permanent
        ));
    }
}
