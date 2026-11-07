//! Bounded, dependency-free EXIF summary reader for the image viewer.
//!
//! Only the handful of tags a photo inspector shows are decoded. Every read is
//! bounds-checked against the in-memory prefix of the file, IFD walks are
//! capped, and any malformed structure simply yields fewer fields — metadata
//! is decoration and must never fail an image open.

use std::io::Read;
use std::path::Path;

use serde::Serialize;

/// Bytes read from the head of a file while looking for EXIF. JPEG APP1 is
/// capped at 64 KiB and sits before the scan data; TIFF-based RAW formats keep
/// IFD0/EXIF near the start; HEIF stores metadata items before `mdat` in the
/// files cameras and phones write.
const EXIF_SCAN_LIMIT: u64 = 8 * 1024 * 1024;
const MAX_IFD_ENTRIES: u16 = 512;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExifSummary {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens_model: Option<String>,
    pub software: Option<String>,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    pub date_time_original: Option<String>,
    pub exposure_time: Option<String>,
    pub f_number: Option<f64>,
    pub iso: Option<u32>,
    pub focal_length_mm: Option<f64>,
    pub focal_length_35mm: Option<u32>,
    pub exposure_bias_ev: Option<f64>,
    pub exposure_program: Option<String>,
    pub metering_mode: Option<String>,
    pub flash: Option<String>,
    pub white_balance: Option<String>,
    pub color_space: Option<String>,
    pub orientation: Option<u16>,
    pub pixel_width: Option<u32>,
    pub pixel_height: Option<u32>,
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
    pub gps_altitude_m: Option<f64>,
}

impl ExifSummary {
    fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Read the EXIF summary of `path`, whose lowercase dotted extension is `ext`.
/// `None` when the container has no (parseable) EXIF block.
pub fn read_exif_summary(path: &Path, ext: &str) -> Option<ExifSummary> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(EXIF_SCAN_LIMIT)
        .read_to_end(&mut bytes)
        .ok()?;
    let tiff = match ext {
        ".jpg" | ".jpeg" => jpeg_exif(&bytes)?,
        ".png" => png_exif(&bytes)?,
        ".webp" => webp_exif(&bytes)?,
        ".heic" | ".heif" | ".avif" => heif_exif(&bytes)?,
        ".tif" | ".tiff" | ".dng" | ".cr2" | ".nef" | ".arw" | ".orf" | ".rw2" | ".raw" => {
            bytes.as_slice()
        }
        _ => return None,
    };
    let summary = parse_tiff(tiff)?;
    (!summary.is_empty()).then_some(summary)
}

fn strip_exif_prefix(data: &[u8]) -> &[u8] {
    data.strip_prefix(b"Exif\0\0").unwrap_or(data)
}

fn jpeg_exif(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut offset = 2usize;
    while offset + 4 <= bytes.len() {
        if bytes[offset] != 0xFF {
            return None;
        }
        let marker = bytes[offset + 1];
        if marker == 0xFF {
            offset += 1;
            continue;
        }
        // Start of scan / end of image: no metadata segments follow.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let length = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
        if length < 2 {
            return None;
        }
        let start = offset + 4;
        let end = (offset + 2).checked_add(length)?;
        let segment = bytes.get(start..end.min(bytes.len()))?;
        if marker == 0xE1 && segment.starts_with(b"Exif\0\0") {
            return Some(&segment[6..]);
        }
        offset = end;
    }
    None
}

fn png_exif(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(0..8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut offset = 8usize;
    while offset + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().ok()?) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        let start = offset + 8;
        let end = start.checked_add(length)?;
        if kind == b"eXIf" {
            return bytes.get(start..end).map(strip_exif_prefix);
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        offset = end.checked_add(4)?;
    }
    None
}

fn webp_exif(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(0..4)? != b"RIFF" || bytes.get(8..12)? != b"WEBP" {
        return None;
    }
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let kind = &bytes[offset..offset + 4];
        let length = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?) as usize;
        let start = offset + 8;
        let end = start.checked_add(length)?;
        if kind == b"EXIF" {
            return bytes.get(start..end).map(strip_exif_prefix);
        }
        offset = end.checked_add(length & 1)?;
    }
    None
}

struct BoxHeader<'a> {
    kind: [u8; 4],
    body: &'a [u8],
}

fn iso_boxes(mut data: &[u8]) -> impl Iterator<Item = BoxHeader<'_>> {
    std::iter::from_fn(move || {
        if data.len() < 8 {
            return None;
        }
        let size = u32::from_be_bytes(data[0..4].try_into().ok()?) as u64;
        let kind: [u8; 4] = data[4..8].try_into().ok()?;
        let (header, size) = match size {
            0 => (8u64, data.len() as u64),
            1 => {
                let large = u64::from_be_bytes(data.get(8..16)?.try_into().ok()?);
                (16u64, large)
            }
            value => (8u64, value),
        };
        if size < header || size > data.len() as u64 {
            data = &[];
            return None;
        }
        let body = &data[header as usize..size as usize];
        data = &data[size as usize..];
        Some(BoxHeader { kind, body })
    })
}

fn read_be(data: &[u8], offset: usize, size: usize) -> Option<u64> {
    let slice = data.get(offset..offset.checked_add(size)?)?;
    Some(
        slice
            .iter()
            .fold(0u64, |value, byte| (value << 8) | *byte as u64),
    )
}

/// HEIF/AVIF keep EXIF as an `Exif` item: find its id in `iinf`, its byte
/// range in `iloc`, then skip the 4-byte TIFF-header offset prefix.
fn heif_exif(bytes: &[u8]) -> Option<&[u8]> {
    let meta = iso_boxes(bytes).find(|item| &item.kind == b"meta")?;
    let meta_body = meta.body.get(4..)?; // full box version + flags
    let mut exif_item: Option<u32> = None;
    let mut locations: Vec<(u32, u64, u64)> = Vec::new();
    for child in iso_boxes(meta_body) {
        match &child.kind {
            b"iinf" => {
                let version = *child.body.first()?;
                let header = if version == 0 { 6 } else { 8 };
                for entry in iso_boxes(child.body.get(header..)?) {
                    if &entry.kind != b"infe" {
                        continue;
                    }
                    let entry_version = *entry.body.first()?;
                    if entry_version < 2 {
                        continue;
                    }
                    let (id, type_offset) = if entry_version == 2 {
                        (read_be(entry.body, 4, 2)? as u32, 8)
                    } else {
                        (read_be(entry.body, 4, 4)? as u32, 10)
                    };
                    if entry.body.get(type_offset..type_offset + 4)? == b"Exif" {
                        exif_item = Some(id);
                    }
                }
            }
            b"iloc" => {
                let body = child.body;
                let version = *body.first()?;
                let sizes = *body.get(4)?;
                let offset_size = (sizes >> 4) as usize;
                let length_size = (sizes & 0x0F) as usize;
                let sizes2 = *body.get(5)?;
                let base_offset_size = (sizes2 >> 4) as usize;
                let index_size = if version >= 1 {
                    (sizes2 & 0x0F) as usize
                } else {
                    0
                };
                let mut cursor = 6usize;
                let (count, count_size) = if version < 2 {
                    (read_be(body, cursor, 2)?, 2)
                } else {
                    (read_be(body, cursor, 4)?, 4)
                };
                cursor += count_size;
                for _ in 0..count.min(4096) {
                    let id_size = if version < 2 { 2 } else { 4 };
                    let id = read_be(body, cursor, id_size)? as u32;
                    cursor += id_size;
                    if version >= 1 {
                        cursor += 2; // construction method
                    }
                    cursor += 2; // data reference index
                    let base = read_be(body, cursor, base_offset_size)?;
                    cursor += base_offset_size;
                    let extents = read_be(body, cursor, 2)?;
                    cursor += 2;
                    for extent in 0..extents.min(64) {
                        cursor += index_size;
                        let offset = read_be(body, cursor, offset_size)?;
                        cursor += offset_size;
                        let length = read_be(body, cursor, length_size)?;
                        cursor += length_size;
                        if extent == 0 {
                            locations.push((id, base + offset, length));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let id = exif_item?;
    let (_, offset, length) = locations.into_iter().find(|(item, _, _)| *item == id)?;
    let start = usize::try_from(offset).ok()?;
    let end = start.checked_add(usize::try_from(length).ok()?)?;
    let item = bytes.get(start..end)?;
    let header_offset = read_be(item, 0, 4)? as usize;
    item.get(4usize.checked_add(header_offset)?..)
}

#[derive(Clone, Copy)]
struct Tiff<'a> {
    data: &'a [u8],
    little: bool,
}

#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    value_offset: usize,
}

impl<'a> Tiff<'a> {
    fn u16(&self, offset: usize) -> Option<u16> {
        let bytes: [u8; 2] = self.data.get(offset..offset + 2)?.try_into().ok()?;
        Some(if self.little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    }

    fn u32(&self, offset: usize) -> Option<u32> {
        let bytes: [u8; 4] = self.data.get(offset..offset + 4)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    fn entries(&self, ifd: usize) -> Vec<Entry> {
        let Some(count) = self.u16(ifd) else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        for index in 0..count.min(MAX_IFD_ENTRIES) as usize {
            let base = ifd + 2 + index * 12;
            let (Some(tag), Some(kind), Some(count)) =
                (self.u16(base), self.u16(base + 2), self.u32(base + 4))
            else {
                break;
            };
            let unit = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 => 4,
                5 | 10 | 12 => 8,
                _ => continue,
            };
            let Some(total) = (unit as u64).checked_mul(count as u64) else {
                continue;
            };
            let value_offset = if total <= 4 {
                base + 8
            } else {
                match self.u32(base + 8) {
                    Some(offset) => offset as usize,
                    None => continue,
                }
            };
            entries.push(Entry {
                tag,
                kind,
                count,
                value_offset,
            });
        }
        entries
    }

    fn ascii(&self, entry: &Entry) -> Option<String> {
        if entry.kind != 2 && entry.kind != 7 {
            return None;
        }
        let length = (entry.count as usize).min(512);
        let raw = self
            .data
            .get(entry.value_offset..entry.value_offset + length)?;
        let text = raw.split(|byte| *byte == 0).next().unwrap_or(raw);
        let text = String::from_utf8_lossy(text).trim().to_owned();
        (!text.is_empty() && !text.chars().all(|c| c.is_control())).then_some(text)
    }

    fn integer(&self, entry: &Entry, index: usize) -> Option<u32> {
        match entry.kind {
            1 | 7 => self
                .data
                .get(entry.value_offset + index)
                .map(|value| *value as u32),
            3 => self.u16(entry.value_offset + index * 2).map(u32::from),
            4 | 9 => self.u32(entry.value_offset + index * 4),
            _ => None,
        }
    }

    fn rational(&self, entry: &Entry, index: usize) -> Option<(f64, u32, u32)> {
        let offset = entry.value_offset + index * 8;
        let (numerator, denominator) = (self.u32(offset)?, self.u32(offset + 4)?);
        if denominator == 0 {
            return None;
        }
        let value = if entry.kind == 10 {
            numerator as i32 as f64 / denominator as i32 as f64
        } else if entry.kind == 5 {
            numerator as f64 / denominator as f64
        } else {
            return None;
        };
        value.is_finite().then_some((value, numerator, denominator))
    }

    fn number(&self, entry: &Entry) -> Option<f64> {
        match entry.kind {
            5 | 10 => self.rational(entry, 0).map(|(value, _, _)| value),
            _ => self.integer(entry, 0).map(f64::from),
        }
    }
}

fn parse_tiff(data: &[u8]) -> Option<ExifSummary> {
    let little = match data.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let tiff = Tiff { data, little };
    // 42 is TIFF; Olympus ORF uses 0x4F52/0x5352 and Panasonic RW2 0x0055.
    if !matches!(tiff.u16(2)?, 42 | 0x4F52 | 0x5352 | 0x0055) {
        return None;
    }
    let ifd0 = tiff.u32(4)? as usize;
    let mut summary = ExifSummary::default();
    let mut exif_ifd = None;
    let mut gps_ifd = None;
    for entry in tiff.entries(ifd0) {
        match entry.tag {
            0x010F => summary.make = tiff.ascii(&entry),
            0x0110 => summary.model = tiff.ascii(&entry),
            0x0112 => {
                summary.orientation = tiff
                    .integer(&entry, 0)
                    .and_then(|value| u16::try_from(value).ok())
                    .filter(|value| (1..=8).contains(value))
            }
            0x0131 => summary.software = tiff.ascii(&entry),
            0x0132 => {
                if summary.date_time_original.is_none() {
                    summary.date_time_original = tiff.ascii(&entry);
                }
            }
            0x013B => summary.artist = tiff.ascii(&entry),
            0x8298 => summary.copyright = tiff.ascii(&entry),
            0x8769 => exif_ifd = tiff.integer(&entry, 0),
            0x8825 => gps_ifd = tiff.integer(&entry, 0),
            _ => {}
        }
    }
    if let Some(offset) = exif_ifd {
        for entry in tiff.entries(offset as usize) {
            match entry.tag {
                0x829A => {
                    summary.exposure_time = tiff.rational(&entry, 0).map(|(value, n, d)| {
                        if value >= 1.0 || n == 0 {
                            format_number(value)
                        } else if n == 1 {
                            format!("1/{d}")
                        } else {
                            format!("1/{}", (1.0 / value).round() as u64)
                        }
                    })
                }
                0x829D => summary.f_number = tiff.number(&entry),
                0x8822 => {
                    summary.exposure_program =
                        tiff.integer(&entry, 0).and_then(exposure_program_label)
                }
                0x8827 => summary.iso = tiff.integer(&entry, 0).filter(|value| *value > 0),
                0x9003 => summary.date_time_original = tiff.ascii(&entry),
                0x9204 => summary.exposure_bias_ev = tiff.number(&entry),
                0x9207 => {
                    summary.metering_mode = tiff.integer(&entry, 0).and_then(metering_mode_label)
                }
                0x9209 => summary.flash = tiff.integer(&entry, 0).map(flash_label),
                0x920A => summary.focal_length_mm = tiff.number(&entry),
                0xA001 => {
                    summary.color_space = tiff.integer(&entry, 0).map(|value| match value {
                        1 => "sRGB".to_owned(),
                        2 => "Adobe RGB".to_owned(),
                        0xFFFF => "未校准".to_owned(),
                        other => format!("{other}"),
                    })
                }
                0xA002 => summary.pixel_width = tiff.integer(&entry, 0).filter(|v| *v > 0),
                0xA003 => summary.pixel_height = tiff.integer(&entry, 0).filter(|v| *v > 0),
                0xA403 => {
                    summary.white_balance = tiff.integer(&entry, 0).map(|value| {
                        if value == 1 {
                            "手动".to_owned()
                        } else {
                            "自动".to_owned()
                        }
                    })
                }
                0xA405 => summary.focal_length_35mm = tiff.integer(&entry, 0).filter(|v| *v > 0),
                0xA434 => summary.lens_model = tiff.ascii(&entry),
                _ => {}
            }
        }
    }
    if let Some(offset) = gps_ifd {
        let mut latitude_ref = None;
        let mut longitude_ref = None;
        let mut latitude = None;
        let mut longitude = None;
        let mut altitude_below = false;
        let mut altitude = None;
        for entry in tiff.entries(offset as usize) {
            match entry.tag {
                0x0001 => latitude_ref = tiff.ascii(&entry),
                0x0002 => latitude = degrees(&tiff, &entry),
                0x0003 => longitude_ref = tiff.ascii(&entry),
                0x0004 => longitude = degrees(&tiff, &entry),
                0x0005 => altitude_below = tiff.integer(&entry, 0) == Some(1),
                0x0006 => altitude = tiff.number(&entry),
                _ => {}
            }
        }
        summary.gps_latitude = latitude.filter(|value| *value <= 90.0).map(|value| {
            if latitude_ref.as_deref() == Some("S") {
                -value
            } else {
                value
            }
        });
        summary.gps_longitude = longitude.filter(|value| *value <= 180.0).map(|value| {
            if longitude_ref.as_deref() == Some("W") {
                -value
            } else {
                value
            }
        });
        summary.gps_altitude_m = altitude.map(|value| if altitude_below { -value } else { value });
    }
    Some(summary)
}

fn degrees(tiff: &Tiff<'_>, entry: &Entry) -> Option<f64> {
    if entry.count < 3 {
        return None;
    }
    let d = tiff.rational(entry, 0)?.0;
    let m = tiff.rational(entry, 1)?.0;
    let s = tiff.rational(entry, 2)?.0;
    let value = d + m / 60.0 + s / 3600.0;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn format_number(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.1}")
    }
}

fn exposure_program_label(value: u32) -> Option<String> {
    Some(
        match value {
            1 => "手动",
            2 => "程序自动",
            3 => "光圈优先",
            4 => "快门优先",
            5 => "创意程序",
            6 => "运动程序",
            7 => "人像模式",
            8 => "风景模式",
            _ => return None,
        }
        .to_owned(),
    )
}

fn metering_mode_label(value: u32) -> Option<String> {
    Some(
        match value {
            1 => "平均测光",
            2 => "中央重点",
            3 => "点测光",
            4 => "多点测光",
            5 => "评价测光",
            6 => "局部测光",
            _ => return None,
        }
        .to_owned(),
    )
}

fn flash_label(value: u32) -> String {
    if value & 1 == 1 {
        "已闪光".to_owned()
    } else {
        "未闪光".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Little-endian TIFF with IFD0 {Make, Orientation, ExifIFD} and an EXIF
    /// IFD {ExposureTime 1/250, FNumber 2.8, ISO 400, FocalLength 35}.
    fn sample_tiff() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(b"II");
        data.extend_from_slice(&42u16.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());
        // IFD0 at 8: 3 entries -> 2 + 36 + 4 = 42 bytes, ends at 50.
        let make_offset = 50u32;
        let exif_offset = 56u32;
        data.extend_from_slice(&3u16.to_le_bytes());
        for (tag, kind, count, value) in [
            (0x010Fu16, 2u16, 6u32, make_offset),
            (0x0112, 3, 1, 6),
            (0x8769, 4, 1, exif_offset),
        ] {
            data.extend_from_slice(&tag.to_le_bytes());
            data.extend_from_slice(&kind.to_le_bytes());
            data.extend_from_slice(&count.to_le_bytes());
            data.extend_from_slice(&value.to_le_bytes());
        }
        data.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(data.len(), 50);
        data.extend_from_slice(b"Canon\0");
        assert_eq!(data.len(), 56);
        // EXIF IFD: 4 entries -> 2 + 48 + 4 = 54 bytes, rationals after (110).
        let rationals = 56 + 54;
        data.extend_from_slice(&4u16.to_le_bytes());
        for (tag, kind, count, value) in [
            (0x829Au16, 5u16, 1u32, rationals as u32),
            (0x829D, 5, 1, rationals as u32 + 8),
            (0x8827, 3, 1, 400),
            (0x920A, 5, 1, rationals as u32 + 16),
        ] {
            data.extend_from_slice(&tag.to_le_bytes());
            data.extend_from_slice(&kind.to_le_bytes());
            data.extend_from_slice(&count.to_le_bytes());
            data.extend_from_slice(&value.to_le_bytes());
        }
        data.extend_from_slice(&0u32.to_le_bytes());
        for (n, d) in [(1u32, 250u32), (28, 10), (35, 1)] {
            data.extend_from_slice(&n.to_le_bytes());
            data.extend_from_slice(&d.to_le_bytes());
        }
        data
    }

    #[test]
    fn parses_core_camera_fields_from_tiff() {
        let summary = parse_tiff(&sample_tiff()).unwrap();
        assert_eq!(summary.make.as_deref(), Some("Canon"));
        assert_eq!(summary.orientation, Some(6));
        assert_eq!(summary.exposure_time.as_deref(), Some("1/250"));
        assert_eq!(summary.f_number, Some(2.8));
        assert_eq!(summary.iso, Some(400));
        assert_eq!(summary.focal_length_mm, Some(35.0));
    }

    #[test]
    fn finds_exif_in_jpeg_app1_and_rejects_garbage() {
        let tiff = sample_tiff();
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&tiff);
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        let found = jpeg_exif(&jpeg).unwrap();
        assert_eq!(parse_tiff(found).unwrap().iso, Some(400));
        assert!(jpeg_exif(&[0xFF, 0xD8, 0xFF, 0xE1, 0x00]).is_none());
        assert!(parse_tiff(b"XX\0\0garbage").is_none());
        // Truncated IFDs degrade to fewer fields instead of panicking.
        let truncated = &sample_tiff()[..60];
        assert!(parse_tiff(truncated).is_some());
    }

    #[test]
    fn finds_exif_in_png_and_webp_chunks() {
        let tiff = sample_tiff();
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&(tiff.len() as u32).to_be_bytes());
        png.extend_from_slice(b"eXIf");
        png.extend_from_slice(&tiff);
        png.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(parse_tiff(png_exif(&png).unwrap()).unwrap().iso, Some(400));

        let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
        webp.extend_from_slice(b"EXIF");
        webp.extend_from_slice(&(tiff.len() as u32).to_le_bytes());
        webp.extend_from_slice(&tiff);
        assert_eq!(
            parse_tiff(webp_exif(&webp).unwrap()).unwrap().iso,
            Some(400)
        );
    }
}
