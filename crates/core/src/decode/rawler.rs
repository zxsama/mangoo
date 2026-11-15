use std::path::Path;

use anyhow::Context as _;
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;

use super::{ensure_decode_pixel_budget, DecodePurpose, DecodeRequest, DecodedImage, MediaDecoder};
use crate::decode::formats::CAMERA_RAW_EXTENSIONS;

/// Camera RAW through `rawler` (pure Rust, no subprocess): every RAW family
/// rawler reads, i.e. [`CAMERA_RAW_EXTENSIONS`].
///
/// Thumbnails prefer the JPEG preview the camera embedded in the file — it is
/// what the camera itself shows, decodes in milliseconds, and is already the
/// right size — and only develop the sensor data when no preview exists.
/// Previews develop the sensor data (the "RAW original"); the embedded JPEG is
/// a separate, explicit choice in the image viewer ([`decode_embedded_preview`]).
pub struct RawlerDecoder;

impl MediaDecoder for RawlerDecoder {
    fn name(&self) -> &'static str {
        "rawler"
    }

    fn handles(&self, ext: &str, _purpose: DecodePurpose) -> bool {
        CAMERA_RAW_EXTENSIONS.contains(&ext)
    }

    fn decode(&self, request: &DecodeRequest<'_>) -> anyhow::Result<DecodedImage> {
        let path = request.source.path();
        let tried_embedded = request.purpose == DecodePurpose::Thumbnail;
        // The Library thumbnail path asks with no size cap and records the
        // decoded size as the media's natural size, so it gets the LARGEST
        // embedded preview (usually full-size) — never a small thumbnail.
        if tried_embedded {
            if let Ok(Some(preview)) = embedded_preview(path) {
                return Ok(preview);
            }
        }
        let developed = if request.purpose == DecodePurpose::Thumbnail {
            develop_for_thumbnail(path)
        } else {
            match request.target_max_px {
                Some(max_px) => develop_sized(path, max_px),
                None => develop(path),
            }
        };
        match developed {
            Ok(developed) => Ok(developed),
            // A camera rawler cannot develop (unknown model, Nikon HE NEF,
            // ...) still shows its own embedded rendering rather than nothing.
            // A thumbnail already looked for one above: do not scan twice.
            Err(error) if tried_embedded => Err(error),
            Err(error) => match embedded_preview(path) {
                Ok(Some(preview)) => Ok(preview),
                _ => Err(error),
            },
        }
    }
}

/// rawler indexes buffers with unchecked offsets and PANICS on truncated or
/// corrupt files (an interrupted copy, a damaged card). Contain that: a bad
/// file is an ordinary decode error, never a Core crash.
fn contained<T>(path: &Path, run: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_else(|_| {
        Err(anyhow::anyhow!(
            "camera RAW is damaged or truncated: {}",
            path.display()
        ))
    })
}

/// Develop the sensor data into an sRGB picture, EXIF orientation applied.
pub fn develop(path: &Path) -> anyhow::Result<DecodedImage> {
    develop_for(path, true, None)
}

/// Develop for a display no larger than `max_px` on the long side: the
/// 16-bit develop is reduced first, so finishing (tone + sharpening) works on
/// a few megapixels instead of the full sensor — several times faster for a
/// viewport tier, identical tones.
pub fn develop_sized(path: &Path, max_px: u32) -> anyhow::Result<DecodedImage> {
    develop_for(path, true, Some(max_px))
}

/// Develop for a picture that is about to be heavily downscaled (thumbnails):
/// identical tones, no capture sharpening (it would be averaged away).
pub fn develop_for_thumbnail(path: &Path) -> anyhow::Result<DecodedImage> {
    develop_for(path, false, None)
}

/// Like [`develop_for_thumbnail`] for a caller that only needs `max_px`
/// (the viewer's filmstrip): reduced before finishing.
pub fn develop_for_thumbnail_sized(path: &Path, max_px: u32) -> anyhow::Result<DecodedImage> {
    develop_for(path, false, Some(max_px.max(256)))
}

fn develop_for(path: &Path, sharpen: bool, max_px: Option<u32>) -> anyhow::Result<DecodedImage> {
    contained(path, || develop_inner(path, sharpen, max_px))
}

fn develop_inner(path: &Path, sharpen: bool, max_px: Option<u32>) -> anyhow::Result<DecodedImage> {
    let source =
        RawSource::new(path).with_context(|| format!("open camera RAW {}", path.display()))?;
    let probe = rawler::decode_dummy(&source)
        .with_context(|| format!("decode camera RAW {} during safety probe", path.display()))?;
    ensure_decode_pixel_budget("rawler", probe.width as u64, probe.height as u64)?;

    let raw = rawler::decode(&source, &RawDecodeParams::default())
        .with_context(|| format!("decode camera RAW {}", path.display()))?;
    ensure_decode_pixel_budget("rawler", raw.width as u64, raw.height as u64)?;
    let developed = rawler::imgop::develop::RawDevelop::default()
        .develop_intermediate(&raw)
        .with_context(|| format!("develop camera RAW {}", path.display()))?
        .to_dynamic_image()
        .ok_or_else(|| anyhow::anyhow!("camera RAW produced no image: {}", path.display()))?;
    drop(raw);
    // rawler's develop has no tone curve and no sharpening; finish it the way
    // the camera did (curve fitted to its own embedded JPEG when it frames
    // the same picture). Both are in sensor orientation here.
    let reference = reference_preview(&source);
    let finished = super::raw_finish::finish_at(developed, reference.as_ref(), sharpen, max_px);
    drop(reference);
    // rawler leaves the developed raster in sensor orientation.
    Ok(into_decoded(oriented(
        image::DynamicImage::ImageRgb8(finished),
        raw_orientation(path),
    )))
}

/// The camera's embedded preview, unrotated (sensor orientation), used only
/// to fit the tone curve. Best effort and isolated: decoded lazily (the
/// medium preview first, the full-size one only if needed), and a panic in
/// rawler's preview path (e.g. CR3 box parsing) yields None instead of
/// failing the develop. Embedded previews are never larger than the sensor,
/// which already passed the pixel budget; the size check here only rejects
/// absurd dimensions after the fact.
fn reference_preview(source: &RawSource) -> Option<image::DynamicImage> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let decoder = rawler::get_decoder(source).ok()?;
        let params = RawDecodeParams::default();
        let usable = |image: &image::DynamicImage| {
            image.width().max(image.height()) >= 640
                && ensure_decode_pixel_budget(
                    "raw preview",
                    image.width() as u64,
                    image.height() as u64,
                )
                .is_ok()
        };
        decoder
            .preview_image(source, &params)
            .ok()
            .flatten()
            .filter(usable)
            .or_else(|| {
                decoder
                    .full_image(source, &params)
                    .ok()
                    .flatten()
                    .filter(usable)
            })
    }))
    .ok()
    .flatten()
}

/// The JPEG preview embedded in a camera RAW, orientation applied. `Ok(None)`
/// when the file carries no usable preview.
pub fn embedded_preview(path: &Path) -> anyhow::Result<Option<DecodedImage>> {
    contained(path, || embedded_preview_inner(path))
}

fn embedded_preview_inner(path: &Path) -> anyhow::Result<Option<DecodedImage>> {
    let from_rawler = rawler_embedded_preview(path);
    match from_rawler {
        Ok(Some(image)) => return Ok(Some(image)),
        // rawler does not know this camera / compression (new bodies, Nikon
        // HE NEF, ...) or found no preview: read the camera's JPEG directly.
        Ok(None) | Err(_) => {}
    }
    match scanned_embedded_preview(path)? {
        Some(image) => Ok(Some(image)),
        None => from_rawler,
    }
}

/// Largest JPEG stream inside the file, found by its markers (any vendor).
/// Orientation comes from the EXIF the RAW container carries, since embedded
/// previews are stored in sensor orientation.
fn scanned_embedded_preview(path: &Path) -> anyhow::Result<Option<DecodedImage>> {
    let Some((bytes, jpeg)) = super::raw_preview::read_largest_jpeg(path)
        .with_context(|| format!("read camera RAW {}", path.display()))?
    else {
        return Ok(None);
    };
    ensure_decode_pixel_budget("raw preview", jpeg.width as u64, jpeg.height as u64)?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
        .with_context(|| format!("decode embedded preview of {}", path.display()))?;
    let ext = crate::decode::formats::lowercase_extension(path);
    let orientation = crate::decode::exif::read_exif_summary(path, &ext)
        .and_then(|exif| exif.orientation)
        .filter(|value| (1..=8).contains(value))
        .unwrap_or_else(|| raw_orientation(path));
    Ok(Some(into_decoded(oriented(image, orientation))))
}

fn rawler_embedded_preview(path: &Path) -> anyhow::Result<Option<DecodedImage>> {
    let source =
        RawSource::new(path).with_context(|| format!("open camera RAW {}", path.display()))?;
    let decoder = rawler::get_decoder(&source)
        .with_context(|| format!("read camera RAW {}", path.display()))?;
    let params = RawDecodeParams::default();
    // Largest first: full-size preview, then the medium preview, then the
    // small thumbnail. Stop at the first one present, so at most one JPEG is
    // decoded (each is a full decode).
    let attempts: [&dyn Fn() -> rawler::Result<Option<image::DynamicImage>>; 3] = [
        &|| decoder.full_image(&source, &params),
        &|| decoder.preview_image(&source, &params),
        &|| decoder.thumbnail_image(&source, &params),
    ];
    let best = attempts.iter().find_map(|attempt| match attempt() {
        Ok(Some(image)) if image.width() > 0 && image.height() > 0 => Some(image),
        _ => None,
    });
    let Some(image) = best else {
        return Ok(None);
    };
    ensure_decode_pixel_budget(
        "rawler preview",
        image.width() as u64,
        image.height() as u64,
    )?;
    Ok(Some(into_decoded(oriented(image, raw_orientation(path)))))
}

/// The SMALLEST embedded JPEG of a camera RAW that still covers
/// `min_long_px` on its long side, orientation applied — for thumbnails.
/// Cameras store a ~160 px thumbnail and a ~1600 px medium preview next to
/// the full-size one; decoding the full 24 MP JPEG to show it at 160 px
/// wasted most of the time. Falls back to [`embedded_preview`].
pub fn embedded_thumbnail(path: &Path, min_long_px: u32) -> anyhow::Result<Option<DecodedImage>> {
    contained(path, || {
        let jpegs = {
            let file = std::fs::File::open(path)
                .with_context(|| format!("read camera RAW {}", path.display()))?;
            super::raw_preview::scan_jpegs(file, 64 * 1024 * 1024)
                .with_context(|| format!("read camera RAW {}", path.display()))?
        };
        // The largest preview defines the frame; small thumbnails with a
        // different aspect are letterboxed (Sony's 160x120 for a 3:2 frame)
        // and would show black bars, so only same-aspect ones qualify.
        let frame = super::raw_preview::largest(&jpegs);
        let aspect =
            |jpeg: &super::raw_preview::EmbeddedJpeg| jpeg.width as f64 / jpeg.height.max(1) as f64;
        let pick = jpegs
            .iter()
            .filter(|jpeg| jpeg.width.max(jpeg.height) >= min_long_px)
            .filter(|jpeg| {
                frame.is_none_or(|frame| {
                    ((aspect(jpeg) - aspect(&frame)) / aspect(&frame)).abs() <= 0.03
                })
            })
            .min_by_key(|jpeg| jpeg.width as u64 * jpeg.height as u64)
            .copied();
        let Some(jpeg) = pick else {
            return embedded_preview_inner(path);
        };
        let small = (|| -> anyhow::Result<DecodedImage> {
            ensure_decode_pixel_budget("raw thumbnail", jpeg.width as u64, jpeg.height as u64)?;
            use std::io::{Read as _, Seek as _};
            let mut file = std::fs::File::open(path)?;
            file.seek(std::io::SeekFrom::Start(jpeg.offset))?;
            let mut bytes = vec![0u8; jpeg.len as usize];
            file.read_exact(&mut bytes)?;
            let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
                .with_context(|| format!("decode embedded thumbnail of {}", path.display()))?;
            let ext = crate::decode::formats::lowercase_extension(path);
            let orientation = crate::decode::exif::read_exif_summary(path, &ext)
                .and_then(|exif| exif.orientation)
                .filter(|value| (1..=8).contains(value))
                .unwrap_or_else(|| raw_orientation(path));
            Ok(into_decoded(oriented(image, orientation)))
        })();
        match small {
            Ok(image) => Ok(Some(image)),
            // A damaged small JPEG: the largest preview may still be fine.
            Err(_) => embedded_preview_inner(path),
        }
    })
}

/// The image viewer's "embedded JPEG" view of a camera RAW.
pub fn decode_embedded_preview(path: &Path) -> anyhow::Result<DecodedImage> {
    embedded_preview(path)?
        .ok_or_else(|| anyhow::anyhow!("camera RAW has no embedded preview: {}", path.display()))
}

fn into_decoded(image: image::DynamicImage) -> DecodedImage {
    let rgba = image.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    DecodedImage {
        rgba: rgba.into_raw(),
        width,
        height,
    }
}

/// EXIF orientation of a RAW (1..=8), from rawler's metadata reader.
pub fn raw_orientation(path: &Path) -> u16 {
    contained(path, || Ok(raw_orientation_inner(path))).unwrap_or(1)
}

fn raw_orientation_inner(path: &Path) -> u16 {
    let Ok(source) = RawSource::new(path) else {
        return 1;
    };
    let Ok(decoder) = rawler::get_decoder(&source) else {
        return 1;
    };
    decoder
        .raw_metadata(&source, &RawDecodeParams::default())
        .ok()
        .and_then(|metadata| metadata.exif.orientation)
        .filter(|value| (1..=8).contains(value))
        .unwrap_or(1)
}

fn oriented(image: image::DynamicImage, orientation: u16) -> image::DynamicImage {
    match orientation {
        2 => image.fliph(),
        3 => image.rotate180(),
        4 => image.flipv(),
        5 => image.rotate90().fliph(),
        6 => image.rotate90(),
        7 => image.rotate270().fliph(),
        8 => image.rotate270(),
        _ => image,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::decode::MediaSource;

    #[test]
    fn claims_every_camera_raw_extension() {
        let decoder = RawlerDecoder;
        for ext in CAMERA_RAW_EXTENSIONS {
            assert!(decoder.handles(ext, DecodePurpose::Thumbnail), "{ext}");
            assert!(decoder.handles(ext, DecodePurpose::Preview), "{ext}");
        }
        for ext in [".cr3", ".raf", ".pef", ".srw", ".nrw", ".3fr", ".x3f"] {
            assert!(decoder.handles(ext, DecodePurpose::Preview), "{ext}");
        }
        assert!(!decoder.handles(".psd", DecodePurpose::Preview));
        assert!(!decoder.handles(".jpg", DecodePurpose::Preview));
    }

    #[test]
    fn orientation_rotates_and_mirrors_like_exif() {
        let image = image::DynamicImage::new_rgba8(4, 2);
        assert_eq!(
            (
                oriented(image.clone(), 1).width(),
                oriented(image.clone(), 1).height()
            ),
            (4, 2)
        );
        assert_eq!(
            (
                oriented(image.clone(), 6).width(),
                oriented(image.clone(), 6).height()
            ),
            (2, 4)
        );
        assert_eq!(
            (
                oriented(image.clone(), 8).width(),
                oriented(image.clone(), 8).height()
            ),
            (2, 4)
        );
        assert_eq!(
            (
                oriented(image.clone(), 3).width(),
                oriented(image, 3).height()
            ),
            (4, 2)
        );
    }

    #[test]
    fn malformed_raw_reports_a_decoder_error_instead_of_no_decoder() {
        let path = std::env::temp_dir().join(format!(
            "mangoo-rawler-decoder-{}-{}.dng",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::write(&path, b"not a camera RAW").expect("write fixture");
        let error = RawlerDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(Path::new(&path)),
                ext: ".dng",
                target_max_px: None,
                purpose: DecodePurpose::Preview,
            })
            .expect_err("malformed RAW must fail");
        assert!(decode_embedded_preview(&path).is_err());
        let _ = std::fs::remove_file(path);

        assert!(error.to_string().contains("camera RAW"), "{error:#}");
    }

    #[test]
    fn library_thumbnail_decode_keeps_full_dimensions_when_provided() {
        // The Library records the decoded size as the media's natural size.
        let Ok(folder) = std::env::var("MANGOO_RAW_SAMPLES") else {
            return;
        };
        for entry in std::fs::read_dir(folder).expect("folder").flatten() {
            let path = entry.path();
            let decoded = RawlerDecoder
                .decode(&DecodeRequest {
                    source: MediaSource::Path(&path),
                    ext: &crate::decode::formats::lowercase_extension(&path),
                    target_max_px: None,
                    purpose: DecodePurpose::Thumbnail,
                })
                .expect("decode");
            eprintln!(
                "LIBTHUMB {}: {}x{}",
                path.display(),
                decoded.width,
                decoded.height
            );
            assert!(
                decoded.width.max(decoded.height) >= 1600,
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn pipeline_timing_when_provided() {
        let Ok(folder) = std::env::var("MANGOO_RAW_TIMING") else {
            return;
        };
        for entry in std::fs::read_dir(folder).expect("folder").flatten() {
            let path = entry.path();
            let t = std::time::Instant::now();
            let developed = develop_sized(&path, 2048).expect("develop");
            let t_dev = t.elapsed();
            let tier =
                crate::thumbnails::generate_preview_webp_from(developed, 2048).expect("tier");
            let t_tier = t.elapsed();
            let t2 = std::time::Instant::now();
            let preview = embedded_thumbnail(&path, 160)
                .expect("preview")
                .expect("some");
            let t_prev = t2.elapsed();
            let thumb = crate::thumbnails::generate_preview_webp_from(preview, 160).expect("thumb");
            let t_thumb = t2.elapsed();
            eprintln!(
                "TIMING {}: develop+finish {:?} +webp2048 {:?} ({}x{}) | embedded {:?} +thumb {:?} ({}x{})",
                path.display(), t_dev, t_tier - t_dev, tier.width, tier.height, t_prev, t_thumb - t_prev, thumb.width, thumb.height
            );
        }
    }

    /// A camera rawler does not know (new body, HE NEF): the preview path
    /// still shows the camera's embedded JPEG. Point MANGOO_RAW_UNKNOWN at a
    /// folder of such files.
    #[test]
    fn unknown_cameras_fall_back_to_the_embedded_jpeg() {
        let Ok(folder) = std::env::var("MANGOO_RAW_UNKNOWN") else {
            return;
        };
        for entry in std::fs::read_dir(folder).expect("folder").flatten() {
            let path = entry.path();
            assert!(
                develop(&path).is_err(),
                "rawler should refuse {}",
                path.display()
            );
            let shown = RawlerDecoder
                .decode(&DecodeRequest {
                    source: MediaSource::Path(&path),
                    ext: &crate::decode::formats::lowercase_extension(&path),
                    target_max_px: None,
                    purpose: DecodePurpose::Preview,
                })
                .unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
            eprintln!(
                "{}: fallback {}x{}",
                path.display(),
                shown.width,
                shown.height
            );
            assert!(shown.width >= 1000);
        }
    }

    /// Real camera files are not committed; point MANGOO_RAW_SAMPLES at a
    /// folder of them to exercise every family end to end.
    #[test]
    fn decodes_real_samples_when_provided() {
        let Ok(folder) = std::env::var("MANGOO_RAW_SAMPLES") else {
            return;
        };
        for entry in std::fs::read_dir(folder).expect("samples folder").flatten() {
            let path = entry.path();
            let ext = crate::decode::formats::lowercase_extension(&path);
            if !CAMERA_RAW_EXTENSIONS.contains(&ext.as_str()) {
                continue;
            }
            let developed =
                develop(&path).unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
            assert!(developed.width > 0 && developed.height > 0);
            if let Ok(dir) = std::env::var("MANGOO_RAW_DUMP") {
                let name = path.file_stem().unwrap().to_string_lossy().to_string();
                image::RgbaImage::from_raw(
                    developed.width,
                    developed.height,
                    developed.rgba.clone(),
                )
                .unwrap()
                .save(format!("{dir}/{name}-developed.png"))
                .unwrap();
            }
            let preview = embedded_preview(&path)
                .unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
            eprintln!(
                "{}: developed {}x{}, embedded {:?}",
                path.display(),
                developed.width,
                developed.height,
                preview.as_ref().map(|image| (image.width, image.height))
            );
        }
    }
}
