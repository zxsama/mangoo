//! Finishing for developed camera RAWs.
//!
//! `rawler`'s develop stops at a scene-referred sRGB rendering: no tone
//! curve, no sharpening. Every camera and every RAW tool applies both, so a
//! bare develop looks flat, dark in the midtones and soft next to the
//! camera's own JPEG (measured on a D750 NEF: developed luma 96 vs camera
//! 140, 128 vs 189). This module adds what the user expects to see:
//!
//! 1. A tone curve. When the RAW carries the camera's embedded JPEG *of the
//!    same framing*, the curve and colourfulness are fitted to it, so the
//!    picture matches the camera's own rendering of that shot. Otherwise a
//!    neutral film-like curve stands in.
//! 2. Capture sharpening on luma only, to restore the acutance demosaicing
//!    takes away — skipped when the result is going to be downscaled anyway.
//!
//! Memory: the 16-bit develop is toned once straight into the 8-bit output
//! buffer and dropped; every fit runs on 192×128 copies; sharpening keeps one
//! u8 luma plane and one u16 blur plane. Peak ≈ 16-bit source + 8-bit output,
//! then ≈ 1.9× the 8-bit frame during sharpening (not 5× as before).

use image::{DynamicImage, RgbImage};
use rayon::prelude::*;

const CURVE_LEN: usize = 4096;
const FIT_W: u32 = 192;
const FIT_H: u32 = 128;

/// High-precision tone curve: 4096 input steps (applied before 8-bit
/// quantisation, so highlight gain never leaves histogram gaps) to 8-bit.
pub struct ToneCurve(Vec<u8>);

impl ToneCurve {
    fn from_fn(f: impl Fn(f32) -> f32) -> Self {
        let mut previous = 0u8;
        Self(
            (0..CURVE_LEN)
                .map(|index| {
                    let x = index as f32 / (CURVE_LEN - 1) as f32;
                    let y = (f(x).clamp(0.0, 1.0) * 255.0).round() as u8;
                    previous = previous.max(y);
                    previous
                })
                .collect(),
        )
    }

    /// Output (0..255) for an input in 0..1.
    fn map(&self, x: f32) -> u8 {
        self.0[((x.clamp(0.0, 1.0) * (CURVE_LEN - 1) as f32).round() as usize).min(CURVE_LEN - 1)]
    }
}

/// Neutral film-like curve (lifted midtones, soft shoulder) for RAWs without
/// a usable embedded preview. Shaped like the curves fitted on real files.
pub fn default_curve() -> ToneCurve {
    ToneCurve::from_fn(|x| 0.82 * (1.0 - (1.0 - x).powf(1.85)) + 0.18 * x)
}

fn luma8(p: &image::Rgb<u8>) -> f32 {
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

/// The reference must be the same framing (aspect within 2%, same
/// orientation), carry real tonal range, and rank its pixels like the
/// developed frame (Spearman ≥ 0.9). Anything else — a crop-mode preview, a
/// black placeholder, an unrelated thumbnail — is rejected.
fn same_picture(
    developed: &RgbImage,
    reference: &RgbImage,
    dev_aspect: f32,
    ref_aspect: f32,
) -> bool {
    if (dev_aspect - ref_aspect).abs() / dev_aspect.max(1e-3) > 0.02 {
        return false;
    }
    let dev_luma: Vec<f32> = developed.pixels().map(luma8).collect();
    let ref_luma: Vec<f32> = reference.pixels().map(luma8).collect();
    if dev_luma.len() != ref_luma.len() || dev_luma.is_empty() {
        return false;
    }
    let mut sorted = ref_luma.clone();
    sorted.sort_by(f32::total_cmp);
    if sorted[sorted.len() * 95 / 100] - sorted[sorted.len() * 5 / 100] < 40.0 {
        return false;
    }
    let rank = |values: &[f32]| {
        let mut order: Vec<usize> = (0..values.len()).collect();
        order.sort_by(|&a, &b| values[a].total_cmp(&values[b]));
        let mut ranks = vec![0f32; values.len()];
        for (position, index) in order.into_iter().enumerate() {
            ranks[index] = position as f32;
        }
        ranks
    };
    let (ra, rb) = (rank(&dev_luma), rank(&ref_luma));
    let mean = (ra.len() as f32 - 1.0) / 2.0;
    let (mut cov, mut va, mut vb) = (0f64, 0f64, 0f64);
    for (a, b) in ra.iter().zip(&rb) {
        let (da, db) = ((a - mean) as f64, (b - mean) as f64);
        cov += da * db;
        va += da * da;
        vb += db * db;
    }
    va > 0.0 && vb > 0.0 && cov / (va.sqrt() * vb.sqrt()) >= 0.9
}

/// Fit a monotone tone curve (developed luma -> reference luma) on the small
/// copies. None when too few tones are populated.
pub fn fit_curve(developed: &RgbImage, reference: &RgbImage) -> Option<ToneCurve> {
    let mut buckets: Vec<Vec<u8>> = vec![Vec::new(); 256];
    for (pa, pb) in developed.pixels().zip(reference.pixels()) {
        buckets[(luma8(pa).round() as usize).min(255)].push(luma8(pb).round().min(255.0) as u8);
    }
    let mut anchors = vec![(0.0f32, 0.0f32)];
    for (input, bucket) in buckets.iter_mut().enumerate() {
        if bucket.len() < 3 {
            continue;
        }
        bucket.sort_unstable();
        anchors.push((
            input as f32 / 255.0,
            bucket[bucket.len() / 2] as f32 / 255.0,
        ));
    }
    if anchors.len() < 13 {
        return None;
    }
    anchors.push((1.0, 1.0));
    let sample = |x: f32| {
        let upper = anchors
            .iter()
            .position(|(ax, _)| *ax >= x)
            .unwrap_or(anchors.len() - 1)
            .max(1);
        let (x0, y0) = anchors[upper - 1];
        let (x1, y1) = anchors[upper];
        if x1 > x0 {
            y0 + (y1 - y0) * (x - x0) / (x1 - x0)
        } else {
            y0
        }
    };
    let h = 2.0 / 255.0;
    // Piecewise-linear through the medians, lightly smoothed; from_fn makes
    // it monotone.
    Some(ToneCurve::from_fn(|x| {
        (sample((x - h).max(0.0)) + sample(x) + sample((x + h).min(1.0))) / 3.0
    }))
}

/// Mean chroma (max-min over max), ignoring near-black.
fn mean_saturation(image: &RgbImage) -> f32 {
    let (mut total, mut count) = (0.0f32, 0u32);
    for pixel in image.pixels() {
        let max = pixel.0.iter().copied().max().unwrap_or(0) as f32;
        let min = pixel.0.iter().copied().min().unwrap_or(0) as f32;
        if max > 12.0 {
            total += (max - min) / max;
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        total / count as f32
    }
}

/// Tone-map and re-saturate one pixel through luma (hue preserved). The gain
/// is capped so near-black colour noise is not amplified; over-range colours
/// are compressed toward luma instead of clipping their hue.
fn tone_pixel(rgb: [f32; 3], curve: &ToneCurve, saturation: f32) -> [u8; 3] {
    let y = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
    let target = curve.map(y) as f32 / 255.0;
    let gain = if y > 1e-4 { (target / y).min(3.0) } else { 1.0 };
    let mut out = rgb.map(|value| target + (value * gain - target) * saturation);
    let peak = out.iter().copied().fold(0.0f32, f32::max);
    if peak > 1.0 {
        let scale = ((1.0 - target) / (peak - target).max(1e-4)).clamp(0.0, 1.0);
        for slot in &mut out {
            *slot = target + (*slot - target) * scale;
        }
    }
    out.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Luma-only unsharp mask in place: [1 2 1] separable kernel (~1 px), 50%
/// amount, threshold 2, so detail is crisper without boosting chroma noise.
pub fn sharpen_luma(image: &mut RgbImage) {
    let (w, h) = (image.width() as usize, image.height() as usize);
    if w < 8 || h < 8 {
        return;
    }
    let luma: Vec<u8> = image
        .as_raw()
        .par_chunks(3)
        .map(|p| {
            (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32)
                .round()
                .min(255.0) as u8
        })
        .collect();
    let mut horizontal = vec![0u16; w * h];
    horizontal
        .par_chunks_mut(w)
        .zip(luma.par_chunks(w))
        .for_each(|(out, row)| {
            for x in 0..w {
                let left = row[x.saturating_sub(1)] as u16;
                let right = row[(x + 1).min(w - 1)] as u16;
                out[x] = left + 2 * row[x] as u16 + right;
            }
        });
    let buffer: &mut [u8] = image.as_mut();
    buffer
        .par_chunks_mut(w * 3)
        .enumerate()
        .for_each(|(y, out)| {
            let up = y.saturating_sub(1) * w;
            let down = (y + 1).min(h - 1) * w;
            for x in 0..w {
                let blurred = (horizontal[up + x] as f32
                    + 2.0 * horizontal[y * w + x] as f32
                    + horizontal[down + x] as f32)
                    / 16.0;
                let detail = luma[y * w + x] as f32 - blurred;
                if detail.abs() < 2.0 {
                    continue;
                }
                let delta = detail * 0.5;
                for channel in &mut out[x * 3..x * 3 + 3] {
                    *channel = (*channel as f32 + delta).clamp(0.0, 255.0).round() as u8;
                }
            }
        });
}

/// The finishing pipeline. `reference` is the camera's embedded preview in
/// the same (sensor) orientation; `sharpen` is false when the output is about
/// to be downscaled a lot (thumbnails), where acutance is lost anyway.
pub fn finish(
    developed: DynamicImage,
    reference: Option<&DynamicImage>,
    sharpen: bool,
) -> RgbImage {
    finish_at(developed, reference, sharpen, None)
}

/// [`finish`], but when the caller only needs `max_px` on the long side the
/// 16-bit develop is first reduced to it (area average), so toning and
/// sharpening touch a few megapixels instead of the whole sensor. Sharpening
/// then runs at the display size, where it is actually seen.
pub fn finish_at(
    developed: DynamicImage,
    reference: Option<&DynamicImage>,
    sharpen: bool,
    max_px: Option<u32>,
) -> RgbImage {
    let developed = match max_px {
        Some(limit) if developed.width().max(developed.height()) > limit.max(1) => {
            let scale = limit as f64 / developed.width().max(developed.height()) as f64;
            let w = ((developed.width() as f64 * scale).round() as u32).max(1);
            let h = ((developed.height() as f64 * scale).round() as u32).max(1);
            developed.thumbnail_exact(w, h)
        }
        _ => developed,
    };
    let (width, height) = (developed.width(), developed.height());
    let dev_aspect = width as f32 / height.max(1) as f32;
    let small = |image: &DynamicImage| {
        image
            .resize_exact(FIT_W, FIT_H, image::imageops::FilterType::Triangle)
            .to_rgb8()
    };
    let dev_small = small(&developed);
    let fitted = reference.and_then(|reference| {
        let ref_small = small(reference);
        let ref_aspect = reference.width() as f32 / reference.height().max(1) as f32;
        if !same_picture(&dev_small, &ref_small, dev_aspect, ref_aspect) {
            return None;
        }
        fit_curve(&dev_small, &ref_small).map(|curve| (curve, ref_small))
    });
    let (curve, saturation) = match fitted {
        Some((curve, ref_small)) => {
            // Saturation factor measured on the small copies after toning.
            let mut toned = dev_small;
            for pixel in toned.pixels_mut() {
                pixel.0 = tone_pixel(pixel.0.map(|v| v as f32 / 255.0), &curve, 1.0);
            }
            let ours = mean_saturation(&toned);
            let theirs = mean_saturation(&ref_small);
            let factor = if ours > 0.02 && theirs > 0.02 {
                (theirs / ours).clamp(0.8, 1.5)
            } else {
                1.0
            };
            (curve, factor)
        }
        // A modest boost, like a camera's "standard" picture style.
        None => (default_curve(), 1.12),
    };
    // Tone straight from 16-bit into the 8-bit output (rows in parallel),
    // then drop the source.
    let mut output = RgbImage::new(width, height);
    let row = width as usize * 3;
    let tone_rows = |source: &[u16], scale: f32, out: &mut [u8]| {
        out.par_chunks_mut(row)
            .zip(source.par_chunks(row))
            .for_each(|(out_row, in_row)| {
                for (out_px, in_px) in out_row.chunks_exact_mut(3).zip(in_row.chunks_exact(3)) {
                    let toned = tone_pixel(
                        [
                            in_px[0] as f32 * scale,
                            in_px[1] as f32 * scale,
                            in_px[2] as f32 * scale,
                        ],
                        &curve,
                        saturation,
                    );
                    out_px.copy_from_slice(&toned);
                }
            });
    };
    match &developed {
        DynamicImage::ImageRgb16(source) => {
            tone_rows(source.as_raw(), 1.0 / 65535.0, output.as_mut())
        }
        other => {
            let source = other.to_rgb16();
            tone_rows(source.as_raw(), 1.0 / 65535.0, output.as_mut());
        }
    }
    drop(developed);
    if sharpen {
        sharpen_luma(&mut output);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(f: impl Fn(u8) -> u8) -> RgbImage {
        RgbImage::from_fn(FIT_W, FIT_H, |x, y| {
            let v = f((((x * 255) / FIT_W + y) % 256) as u8);
            image::Rgb([v, v, v])
        })
    }

    #[test]
    fn default_curve_lifts_midtones_and_is_monotone() {
        let curve = default_curve();
        assert_eq!(curve.map(0.0), 0);
        assert!(curve.map(1.0) >= 250);
        assert!(curve.map(0.376) > 120 && curve.map(0.5) > 160);
        assert!(curve.0.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn fitted_curve_reproduces_a_known_brightening() {
        let lift = |v: u8| ((v as f32 / 255.0).powf(0.6) * 255.0).round() as u8;
        let dev = gradient(|v| v);
        let reference = gradient(lift);
        assert!(same_picture(&dev, &reference, 1.5, 1.5));
        let curve = fit_curve(&dev, &reference).expect("fit");
        for probe in [32u8, 64, 128, 192] {
            let got = curve.map(probe as f32 / 255.0) as i32;
            let expected = lift(probe) as i32;
            assert!((got - expected).abs() <= 8, "{probe}: {got} vs {expected}");
        }
        assert!(curve.0.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn mismatched_flat_or_cropped_references_are_rejected() {
        let dev = gradient(|v| v);
        let noise = RgbImage::from_fn(FIT_W, FIT_H, |x, y| {
            let v = ((x * 7919 + y * 104_729) % 251) as u8;
            image::Rgb([v, v, v])
        });
        assert!(!same_picture(&dev, &noise, 1.5, 1.5), "unrelated");
        let flat = RgbImage::from_pixel(FIT_W, FIT_H, image::Rgb([3, 3, 3]));
        assert!(!same_picture(&dev, &flat, 1.5, 1.5), "black placeholder");
        assert!(!same_picture(&dev, &dev, 1.5, 16.0 / 9.0), "crop mode");
        assert!(!same_picture(&dev, &dev, 1.5, 1.0 / 1.5), "rotated preview");
    }

    #[test]
    fn toning_keeps_hue_and_never_amplifies_near_black_wildly() {
        let out = tone_pixel([0.47, 0.235, 0.118], &default_curve(), 1.0);
        assert!(out[0] > 120, "brightened {out:?}");
        assert!((out[0] as f32 / out[1] as f32 - 2.0).abs() < 0.2, "{out:?}");
        let dark = tone_pixel([0.002, 0.0005, 0.0], &default_curve(), 1.0);
        assert!(dark.iter().all(|&v| v < 8), "{dark:?}");
    }

    #[test]
    fn finish_outputs_the_frame_size_and_sharpens_edges() {
        let dev = DynamicImage::ImageRgb16(image::ImageBuffer::from_fn(64, 32, |x, _| {
            let v = if x < 32 { 9000u16 } else { 30000 };
            image::Rgb([v, v, v])
        }));
        let soft = finish(dev.clone(), None, false);
        let crisp = finish(dev, None, true);
        assert_eq!(soft.dimensions(), (64, 32));
        let edge = |image: &RgbImage| {
            image.get_pixel(32, 10)[0] as i32 - image.get_pixel(31, 10)[0] as i32
        };
        assert!(
            edge(&crisp) > edge(&soft),
            "{} vs {}",
            edge(&crisp),
            edge(&soft)
        );
    }
}
