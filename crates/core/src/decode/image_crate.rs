//! The default, fastest link in the chain: the `image` crate with its default
//! features. Registered FIRST so JPEG/PNG/GIF/WebP/BMP/common-TIFF — the
//! overwhelming majority of any real library — never pay a subprocess spawn.

use image::ImageReader;

use super::{DecodePurpose, DecodeRequest, DecodedImage, MediaDecoder, MediaSource};

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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

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
}
