//! Thumbnail rendering for the Windows Explorer provider, platform-neutral.
//!
//! Explorer hands the provider a stream of the file and a requested size;
//! we return straight RGBA pixels. Two sources, cheapest first:
//!
//! 1. Camera RAW and other containers: the largest embedded JPEG preview
//!    (every major camera stores one), found by its markers — no RAW decode,
//!    no camera database, milliseconds even for 60 MP files.
//! 2. Formats the `image` crate reads (WebP, TIFF, PNG, ...): a plain decode.
//!
//! The result is downscaled to the requested size here, so Explorer never
//! receives a 24 MP bitmap.

use image::{DynamicImage, GenericImageView};

/// Upper bound on what is read from a file for a thumbnail. Previews live in
/// the first part of every container we know.
pub const READ_LIMIT: usize = 96 * 1024 * 1024;

/// A rendered thumbnail: tightly packed BGRA (premultiplied is not needed;
/// thumbnails are opaque except for formats with alpha).
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    /// Top-down BGRA8.
    pub bgra: Vec<u8>,
    pub has_alpha: bool,
}

/// Every complete JPEG inside `bytes` as (offset, len, width, height).
/// Linear-time: header walks are capped and failed candidates advance.
pub fn embedded_jpegs(bytes: &[u8]) -> Vec<(usize, usize, u32, u32)> {
    const MAX_SEGMENTS: usize = 64;
    let mut found = Vec::new();
    let mut cursor = 0usize;
    while cursor + 3 < bytes.len() {
        let Some(relative) = bytes[cursor..]
            .windows(3)
            .position(|w| w[0] == 0xFF && w[1] == 0xD8 && w[2] == 0xFF)
        else {
            break;
        };
        let start = cursor + relative;
        cursor = start + 2;
        // Walk the header to SOS.
        let mut index = start + 2;
        let mut size = None;
        let mut data = None;
        for _ in 0..MAX_SEGMENTS {
            let (Some(&ff), Some(&marker)) = (bytes.get(index), bytes.get(index + 1)) else {
                break;
            };
            if ff != 0xFF || !(0xC0..=0xFE).contains(&marker) || (0xD0..=0xD9).contains(&marker) {
                break;
            }
            let (Some(&hi), Some(&lo)) = (bytes.get(index + 2), bytes.get(index + 3)) else {
                break;
            };
            let length = u16::from_be_bytes([hi, lo]) as usize;
            if length < 2 {
                break;
            }
            match marker {
                0xC0 | 0xC1 | 0xC2 => {
                    let Some(frame) = bytes.get(index + 5..index + 9) else {
                        break;
                    };
                    let h = u16::from_be_bytes([frame[0], frame[1]]) as u32;
                    let w = u16::from_be_bytes([frame[2], frame[3]]) as u32;
                    if w == 0 || h == 0 {
                        break;
                    }
                    size = Some((w, h));
                }
                0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => break,
                0xDA => {
                    data = Some(index + 2 + length);
                    break;
                }
                _ => {}
            }
            index += 2 + length;
        }
        let (Some((w, h)), Some(mut scan)) = (size, data) else {
            continue;
        };
        // Entropy data to EOI (progressive scans re-enter via segment markers).
        let mut end = None;
        while scan + 1 < bytes.len() {
            if bytes[scan] != 0xFF {
                scan += 1;
                continue;
            }
            match bytes[scan + 1] {
                0x00 | 0xD0..=0xD7 => scan += 2,
                0xFF => scan += 1,
                0xD9 => {
                    end = Some(scan + 2);
                    break;
                }
                // Progressive JPEG: a table / next-scan segment. Skip it by
                // its length (bounded; a bogus length ends the candidate).
                marker if (0xC0..=0xFE).contains(&marker) => {
                    let (Some(&hi), Some(&lo)) = (bytes.get(scan + 2), bytes.get(scan + 3)) else {
                        break;
                    };
                    let length = u16::from_be_bytes([hi, lo]) as usize;
                    if length < 2 {
                        break;
                    }
                    scan += 2 + length;
                }
                _ => scan += 2,
            }
        }
        if let Some(end) = end {
            found.push((start, end - start, w, h));
            cursor = end;
        }
    }
    found
}

/// The embedded JPEG to use for a `size` px thumbnail: the smallest one whose
/// long side covers `size` (decoding a 24 MP preview for a 256 px icon is
/// wasted work), else the largest available.
fn preview_for(bytes: &[u8], size: u32) -> Option<DynamicImage> {
    let jpegs: Vec<_> = embedded_jpegs(bytes)
        .into_iter()
        .filter(|(_, _, w, h)| (*w).max(*h) >= 96)
        .collect();
    let adequate = jpegs
        .iter()
        .filter(|(_, _, w, h)| (*w).max(*h) >= size)
        .min_by_key(|(_, _, w, h)| *w as u64 * *h as u64);
    let largest = jpegs
        .iter()
        .max_by_key(|(_, _, w, h)| *w as u64 * *h as u64);
    for &(offset, len, _, _) in adequate.into_iter().chain(largest) {
        if let Ok(image) = image::load_from_memory_with_format(
            &bytes[offset..offset + len],
            image::ImageFormat::Jpeg,
        ) {
            return Some(image);
        }
    }
    None
}

fn decode_any(bytes: &[u8], size: u32) -> Option<DynamicImage> {
    // Containers (RAW, PSD with preview, DNG, ...): an embedded JPEG. A plain
    // JPEG file is its own "embedded" JPEG, which is fine.
    preview_for(bytes, size).or_else(|| image::load_from_memory(bytes).ok())
}

/// EXIF orientation (1..=8) from the first TIFF header in the file, if any.
fn orientation(bytes: &[u8]) -> u16 {
    let head = &bytes[..bytes.len().min(1 << 20)];
    let Some(at) = head.windows(4).position(|w| w == b"II*\0" || w == b"MM\0*") else {
        return 1;
    };
    let tiff = &head[at..];
    let little = tiff[0] == b'I';
    let u16_at = |o: usize| {
        tiff.get(o..o + 2).map(|b| {
            if little {
                u16::from_le_bytes([b[0], b[1]])
            } else {
                u16::from_be_bytes([b[0], b[1]])
            }
        })
    };
    let u32_at = |o: usize| {
        tiff.get(o..o + 4).map(|b| {
            if little {
                u32::from_le_bytes([b[0], b[1], b[2], b[3]])
            } else {
                u32::from_be_bytes([b[0], b[1], b[2], b[3]])
            }
        })
    };
    let Some(ifd) = u32_at(4) else { return 1 };
    let ifd = ifd as usize;
    let Some(count) = u16_at(ifd) else { return 1 };
    for entry in 0..count.min(256) as usize {
        let base = ifd + 2 + entry * 12;
        if u16_at(base) == Some(0x0112) {
            return u16_at(base + 8)
                .filter(|v| (1..=8).contains(v))
                .unwrap_or(1);
        }
    }
    1
}

fn oriented(image: DynamicImage, orientation: u16) -> DynamicImage {
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

/// Render a thumbnail whose long side is at most `size` from file bytes.
pub fn render(bytes: &[u8], size: u32) -> Option<Thumbnail> {
    finish(decode_any(bytes, size)?, bytes, size)
}

/// [`render`] from a file PREFIX: only an embedded preview that covers
/// `size` counts (a smaller one may be followed by a bigger one further in).
pub fn render_preview(prefix: &[u8], size: u32) -> Option<Thumbnail> {
    let covers = embedded_jpegs(prefix)
        .iter()
        .any(|(_, _, w, h)| (*w).max(*h) >= size.max(96));
    if !covers {
        return None;
    }
    finish(preview_for(prefix, size)?, prefix, size)
}

fn finish(image: DynamicImage, bytes: &[u8], size: u32) -> Option<Thumbnail> {
    let image = oriented(image, orientation(bytes));
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let scale = (size.max(16) as f64 / w.max(h) as f64).min(1.0);
    let tw = ((w as f64 * scale).round() as u32).max(1);
    let th = ((h as f64 * scale).round() as u32).max(1);
    let small = if (tw, th) == (w, h) {
        image
    } else {
        image.thumbnail_exact(tw, th)
    };
    let has_alpha = small.color().has_alpha();
    let rgba = small.to_rgba8();
    let mut bgra = rgba.into_raw();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        if has_alpha {
            // Explorer's WTSAT_ARGB expects premultiplied alpha.
            let a = pixel[3] as u32;
            for channel in &mut pixel[..3] {
                *channel = ((*channel as u32 * a + 127) / 255) as u8;
            }
        }
    }
    Some(Thumbnail {
        width: tw,
        height: th,
        bgra,
        has_alpha,
    })
}

/// The formats this provider registers for: those Windows Explorer does not
/// thumbnail out of the box (camera RAW families, HEIF-less builds' WebP, ...).
/// JPEG / PNG / GIF / BMP / TIFF stay with Windows' own provider.
pub const EXTENSIONS: &[&str] = &[
    ".3fr", ".ari", ".arw", ".cr2", ".cr3", ".crw", ".dcr", ".dcs", ".dng", ".erf", ".fff", ".iiq",
    ".kdc", ".mef", ".mos", ".mrw", ".nef", ".nrw", ".orf", ".ori", ".pef", ".qtk", ".raf", ".raw",
    ".rw2", ".rwl", ".srw", ".x3f", ".webp", ".psd",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, 80])
        });
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
            .encode_image(&image)
            .unwrap();
        out
    }

    #[test]
    fn container_thumbnail_comes_from_the_largest_embedded_jpeg() {
        let mut file = b"FAKERAW\0".to_vec();
        file.extend(jpeg(120, 80));
        file.extend(std::iter::repeat_n(0x42, 5000));
        file.extend(jpeg(1200, 800));
        let thumb = render(&file, 256).expect("thumbnail");
        assert_eq!((thumb.width, thumb.height), (256, 171));
        assert_eq!(thumb.bgra.len(), 256 * 171 * 4);
        assert!(!thumb.has_alpha);
    }

    #[test]
    fn plain_images_decode_and_never_upscale() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(40, 20, image::Rgba([10, 20, 30, 128]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let thumb = render(&png, 256).expect("thumbnail");
        assert_eq!((thumb.width, thumb.height), (40, 20));
        assert!(thumb.has_alpha);
        // Premultiplied BGRA: B = 30*128/255 ~ 15.
        assert_eq!(&thumb.bgra[..4], &[15, 10, 5, 128]);
    }

    #[test]
    fn garbage_is_rejected_quickly() {
        let mut junk = vec![0xFFu8, 0xD8];
        for _ in 0..200_000 {
            junk.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x04]);
        }
        let started = std::time::Instant::now();
        assert!(render(&junk, 128).is_none());
        assert!(started.elapsed().as_secs() < 5);
    }

    #[test]
    fn registered_extensions_leave_windows_native_formats_alone() {
        for native in [".jpg", ".jpeg", ".png", ".gif", ".bmp", ".tif", ".tiff"] {
            assert!(!EXTENSIONS.contains(&native), "{native}");
        }
        assert!(EXTENSIONS.contains(&".nef") && EXTENSIONS.contains(&".cr3"));
    }
}

#[cfg(test)]
mod samples {
    /// `MANGOO_RAW_SAMPLES=<dir>`: every file there renders a thumbnail.
    #[test]
    fn real_camera_files_render() {
        let Ok(dir) = std::env::var("MANGOO_RAW_SAMPLES") else {
            return;
        };
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            let started = std::time::Instant::now();
            let thumb = super::render(&bytes, 256);
            eprintln!(
                "{}: {:?} in {:?}",
                path.display(),
                thumb.as_ref().map(|t| (t.width, t.height)),
                started.elapsed()
            );
            assert!(thumb.is_some(), "{}", path.display());
        }
    }
}
