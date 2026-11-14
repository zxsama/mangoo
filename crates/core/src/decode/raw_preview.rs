//! Vendor-agnostic access to the JPEG previews cameras embed in RAW files.
//!
//! `rawler` refuses a file outright when it does not know the camera model or
//! the compression (e.g. Nikon HE/HE* "high efficiency" NEF from Z8/Z9/Zf,
//! bodies released after the pinned rawler) — and with it, the embedded
//! preview. Nearly every camera (Canon, Nikon, Sony, Fujifilm, Panasonic,
//! Olympus/OM, Pentax, Leica, Hasselblad, Sigma, DNG writers) stores a
//! baseline JPEG preview inside the RAW, usually full-size. This scanner finds
//! those JPEG streams by their markers without understanding the container,
//! so a RAW rawler cannot develop still shows the camera's own rendering.
//!
//! Cost is bounded for hostile input. The file is read in fixed windows
//! (never held whole). A candidate header is parsed only inside the current
//! window and is capped in segments, so each SOI costs O(window) at most and
//! a failed candidate advances the scan past it. Entropy data is skipped with
//! a single forward pass. Total work is O(bytes read).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Previews live in the first part of every container we know; even 100+ MP
/// files keep the full-size JPEG well inside this window.
const SCAN_LIMIT: u64 = 160 * 1024 * 1024;
/// Read window. A header (SOI .. SOS) must fit in one window; real headers
/// are a few KB (EXIF APP1 at most 64 KB).
const WINDOW: usize = 1024 * 1024;
/// Bytes carried into the next window so a header straddling the boundary is
/// still seen whole.
const CARRY: usize = 128 * 1024;
/// Header segments (APPn, DQT, DHT, SOF, ...) accepted before SOS. Real
/// previews have fewer than 20.
const MAX_HEADER_SEGMENTS: usize = 64;
/// Ignore tiny thumbnails (long side under 160 px) when picking a preview.
const MIN_USEFUL_LONG_SIDE: u32 = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedJpeg {
    pub offset: u64,
    pub len: u64,
    pub width: u32,
    pub height: u32,
}

/// A header marker may only be a real segment marker: not SOI/EOI/RSTn/TEM,
/// not fill or stuffing.
fn header_marker(marker: u8) -> bool {
    (0xC0..=0xFE).contains(&marker) && !(0xD0..=0xD9).contains(&marker)
}

enum Header {
    /// Frame size and the window index where entropy-coded data begins.
    Jpeg {
        width: u32,
        height: u32,
        data: usize,
    },
    Invalid,
    /// The header runs past the window: retry with the next window.
    NeedMore,
}

/// Walk segments from `index` (pointing at FF xx) to the SOS. `size` carries
/// a frame size seen earlier (progressive continuation).
#[cfg(test)]
thread_local! {
    /// Segment hops performed by this thread's scans (complexity tests).
    static HOPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn walk_segments(bytes: &[u8], mut index: usize, mut size: Option<(u32, u32)>) -> Header {
    for _ in 0..MAX_HEADER_SEGMENTS {
        #[cfg(test)]
        HOPS.with(|hops| hops.set(hops.get() + 1));
        let (Some(&ff), Some(&marker), Some(&hi), Some(&lo)) = (
            bytes.get(index),
            bytes.get(index + 1),
            bytes.get(index + 2),
            bytes.get(index + 3),
        ) else {
            return Header::NeedMore;
        };
        if ff != 0xFF || !header_marker(marker) {
            return Header::Invalid;
        }
        let length = u16::from_be_bytes([hi, lo]) as usize;
        if length < 2 {
            return Header::Invalid;
        }
        match marker {
            0xC0 | 0xC1 | 0xC2 => {
                let Some(frame) = bytes.get(index + 5..index + 9) else {
                    return Header::NeedMore;
                };
                let height = u16::from_be_bytes([frame[0], frame[1]]) as u32;
                let width = u16::from_be_bytes([frame[2], frame[3]]) as u32;
                if width == 0 || height == 0 {
                    return Header::Invalid;
                }
                size = Some((width, height));
            }
            // Lossless / arithmetic frames are sensor data, not previews.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return Header::Invalid,
            0xDA => {
                let Some((width, height)) = size else {
                    return Header::Invalid;
                };
                let data = index + 2 + length;
                if data > bytes.len() {
                    return Header::NeedMore;
                }
                return Header::Jpeg {
                    width,
                    height,
                    data,
                };
            }
            _ => {}
        }
        index += 2 + length;
    }
    Header::Invalid
}

#[derive(Clone, Copy)]
enum State {
    /// Looking for the next SOI.
    Seeking,
    /// Inside entropy-coded data of a JPEG that started at `start`.
    Entropy { start: u64, width: u32, height: u32 },
}

/// Find every embedded JPEG in the first `limit` bytes of `reader`.
pub fn scan_jpegs<R: Read>(mut reader: R, limit: u64) -> std::io::Result<Vec<EmbeddedJpeg>> {
    let mut found = Vec::new();
    let mut window: Vec<u8> = Vec::with_capacity(WINDOW + CARRY);
    // File offset of window[0].
    let mut base: u64 = 0;
    let mut remaining = limit;
    let mut state = State::Seeking;
    // Index in the window where scanning resumes.
    let mut cursor = 0usize;
    loop {
        // Fill the window.
        let want = (WINDOW as u64).min(remaining) as usize;
        let start_len = window.len();
        window.resize(start_len + want, 0);
        let mut filled = 0;
        while filled < want {
            let read = reader.read(&mut window[start_len + filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        window.truncate(start_len + filled);
        remaining -= filled as u64;
        let last = filled < want || remaining == 0;

        // Scan; stop before the tail unless this is the last window, so a
        // structure straddling the boundary is resolved next time.
        let horizon = if last {
            window.len()
        } else {
            window.len().saturating_sub(CARRY)
        };
        while cursor < horizon {
            match state {
                State::Seeking => {
                    let Some(relative) = window[cursor..horizon]
                        .windows(3)
                        .position(|w| w[0] == 0xFF && w[1] == 0xD8 && w[2] == 0xFF)
                    else {
                        // An SOI starting in the last two bytes is re-checked
                        // with the next window.
                        cursor = horizon.saturating_sub(2).max(cursor);
                        break;
                    };
                    let at = cursor + relative;
                    match walk_segments(&window, at + 2, None) {
                        Header::Jpeg {
                            width,
                            height,
                            data,
                        } => {
                            state = State::Entropy {
                                start: base + at as u64,
                                width,
                                height,
                            };
                            cursor = data;
                        }
                        // Not a JPEG, or a header too large to fit a window.
                        // A header running past this window (large ICC /
                        // EXIF) is retried from its SOI with the next one.
                        Header::NeedMore if !last && at > 0 => {
                            cursor = at;
                            break;
                        }
                        Header::Invalid | Header::NeedMore => cursor = at + 2,
                    }
                }
                State::Entropy {
                    start,
                    width,
                    height,
                } => {
                    let Some(relative) = window[cursor..].iter().position(|&b| b == 0xFF) else {
                        cursor = window.len();
                        break;
                    };
                    let at = cursor + relative;
                    let Some(&next) = window.get(at + 1) else {
                        cursor = at;
                        break;
                    };
                    match next {
                        // Stuffing, restart markers and fill bytes are data.
                        0x00 | 0xD0..=0xD7 => cursor = at + 2,
                        0xFF => cursor = at + 1,
                        0xD9 => {
                            let end = base + (at + 2) as u64;
                            found.push(EmbeddedJpeg {
                                offset: start,
                                len: end - start,
                                width,
                                height,
                            });
                            state = State::Seeking;
                            cursor = at + 2;
                        }
                        // Progressive: more tables and another scan follow.
                        _ => match walk_segments(&window, at, Some((width, height))) {
                            Header::Jpeg { data, .. } => cursor = data,
                            Header::NeedMore if !last => {
                                cursor = at;
                                break;
                            }
                            _ => {
                                state = State::Seeking;
                                cursor = at + 2;
                            }
                        },
                    }
                }
            }
        }
        if last {
            break;
        }
        // Slide: keep everything from the cursor (bounded by CARRY plus a
        // stalled progressive header, itself bounded by the window).
        let keep = cursor.min(window.len());
        window.drain(..keep);
        base += keep as u64;
        cursor = 0;
        if window.len() > WINDOW {
            // A structure larger than a window cannot be a real header:
            // give up on it and continue scanning.
            state = State::Seeking;
            let drop = window.len() - CARRY;
            window.drain(..drop);
            base += drop as u64;
        }
    }
    Ok(found)
}

/// The largest preview by pixel count.
pub fn largest(jpegs: &[EmbeddedJpeg]) -> Option<EmbeddedJpeg> {
    jpegs
        .iter()
        .copied()
        .filter(|jpeg| jpeg.width.max(jpeg.height) >= MIN_USEFUL_LONG_SIDE)
        .max_by_key(|jpeg| jpeg.width as u64 * jpeg.height as u64)
}

/// Locate the largest embedded JPEG of `path` without keeping the file in
/// memory: only one window is resident at a time.
pub fn locate_largest_jpeg(path: &Path) -> std::io::Result<Option<EmbeddedJpeg>> {
    Ok(largest(&scan_jpegs(File::open(path)?, SCAN_LIMIT)?))
}

/// The bytes of the largest embedded JPEG of `path` (only that slice is read).
pub fn read_largest_jpeg(path: &Path) -> std::io::Result<Option<(Vec<u8>, EmbeddedJpeg)>> {
    let Some(jpeg) = locate_largest_jpeg(path)? else {
        return Ok(None);
    };
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(jpeg.offset))?;
    let mut bytes = vec![0u8; jpeg.len as usize];
    file.read_exact(&mut bytes)?;
    Ok(Some((bytes, jpeg)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        });
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
            .encode_image(&image)
            .expect("encode");
        out
    }

    fn scan(bytes: &[u8]) -> Vec<EmbeddedJpeg> {
        scan_jpegs(bytes, bytes.len() as u64).expect("scan")
    }

    #[test]
    fn finds_every_embedded_jpeg_and_picks_the_largest() {
        // A fake container: header, a thumbnail, noise with stray FFD8, a big preview.
        let mut file = b"IIRO\x08\x00\x00\x00junk".to_vec();
        let small = jpeg(160, 120);
        let big = jpeg(640, 480);
        file.extend_from_slice(&small);
        file.extend_from_slice(&[0xFF, 0xD8, 0x12, 0x34, 0xFF, 0xD8, 0xFF, 0xE0, 0x00]);
        file.extend(std::iter::repeat_n(0x5A, 1000));
        let big_offset = file.len() as u64;
        file.extend_from_slice(&big);
        file.extend_from_slice(b"trailing sensor data");

        let all = scan(&file);
        assert_eq!(all.len(), 2, "{all:?}");
        let best = largest(&all).expect("preview");
        assert_eq!((best.width, best.height), (640, 480));
        assert_eq!(best.offset, big_offset);
        let slice = &file[best.offset as usize..(best.offset + best.len) as usize];
        assert_eq!(slice, big.as_slice());
        image::load_from_memory(slice).expect("decodable");
    }

    #[test]
    fn previews_spanning_window_boundaries_are_found() {
        for lead in [
            WINDOW - CARRY - 10,
            WINDOW - 100,
            WINDOW - 3,
            WINDOW + 7,
            3 * WINDOW - 500,
        ] {
            let mut file = vec![0x11u8; lead];
            let big = jpeg(1200, 900);
            let offset = file.len() as u64;
            file.extend_from_slice(&big);
            file.extend(std::iter::repeat_n(0x22, 3000));
            let best = largest(&scan(&file)).unwrap_or_else(|| panic!("lead {lead}"));
            assert_eq!(
                (best.offset, best.len as usize),
                (offset, big.len()),
                "lead {lead}"
            );
        }
    }

    #[test]
    fn large_headers_and_boundary_sois_are_found() {
        // A preview with a ~300 KB APP2 (ICC) block before its frame, placed
        // so its header straddles a window boundary.
        let base = jpeg(900, 600);
        let mut big_header = base[..2].to_vec();
        for _ in 0..5 {
            big_header.extend_from_slice(&[0xFF, 0xE2, 0xFF, 0xFF]);
            big_header.extend(std::iter::repeat_n(0x33, 0xFFFF - 2));
        }
        big_header.extend_from_slice(&base[2..]);
        for lead in [WINDOW - CARRY - 50, WINDOW - CARRY - 2, WINDOW - CARRY - 1] {
            let mut file = vec![0x11u8; lead];
            let offset = file.len() as u64;
            file.extend_from_slice(&big_header);
            file.extend(std::iter::repeat_n(0x22, 2000));
            let best = largest(&scan(&file)).unwrap_or_else(|| panic!("lead {lead}"));
            assert_eq!(
                (best.offset, best.len as usize),
                (offset, big_header.len()),
                "lead {lead}"
            );
        }
    }

    #[test]
    fn noise_and_truncated_streams_are_not_previews() {
        assert!(scan(&[0xFF, 0xD8, 0xFF, 0xE0]).is_empty());
        let mut truncated = jpeg(320, 200);
        truncated.truncate(truncated.len() / 2);
        assert!(largest(&scan(&truncated)).is_none());
        assert!(scan(b"").is_empty());
    }

    #[test]
    fn hostile_marker_floods_finish_in_linear_time() {
        // A candidate every 6 bytes whose header chain links to the next one.
        let mut hostile = vec![0xFF, 0xD8];
        for _ in 0..2_000_000 {
            hostile.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x04, 0xFF, 0xD8]);
        }
        let mut repeated = Vec::new();
        for _ in 0..4_000_000 {
            repeated.extend_from_slice(&[0xFF, 0xD8]);
        }
        // SOIs whose 64-segment header chains are all plausible: each walk is
        // capped at 64 segments, so the scan stays linear.
        let mut chained = Vec::new();
        for _ in 0..40_000 {
            chained.extend_from_slice(&[0xFF, 0xD8]);
            for _ in 0..70 {
                chained.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x02]);
            }
        }
        // Deterministic bound instead of wall-clock time: header work is at
        // most MAX_HEADER_SEGMENTS hops per input byte region, i.e. linear.
        for input in [&chained, &hostile, &repeated] {
            HOPS.with(|hops| hops.set(0));
            assert!(scan(input).is_empty());
            let hops = HOPS.with(|hops| hops.get());
            let bound = (input.len() as u64 / 2 + 1) * MAX_HEADER_SEGMENTS as u64;
            assert!(hops <= bound, "{hops} hops for {} bytes", input.len());
            // And far below quadratic (which would be ~len^2 / 6).
            assert!(
                hops < input.len() as u64 * 40,
                "{hops} hops for {} bytes",
                input.len()
            );
        }
    }
}
