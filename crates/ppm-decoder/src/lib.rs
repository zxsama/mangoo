//! PPM P3/P6, Maxval=255, single-image bounded decoder example.
use std::io::{BufReader, Read};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use image::codecs::pnm::{PnmDecoder, PnmSubtype};
use image::{DynamicImage, ImageDecoder, RgbImage};

pub const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_SOURCE_PIXELS: u64 = 16_000_000;
pub const MAX_OUTPUT_PIXELS: u64 = 10_000_000;
pub const MAX_HEADER_BYTES: u64 = 64 * 1024;
pub const CHUNK_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Unsupported,
    InvalidData,
    ResourceLimit,
}
impl DecodeError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::InvalidData => "invalid_data",
            Self::ResourceLimit => "resource_limit",
        }
    }
}

struct LimitedReader<R> {
    inner: R,
    remaining: Arc<AtomicU64>,
}
impl<R: Read> Read for LimitedReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.remaining.load(Ordering::Relaxed) as usize;
        // PNM comment scanning filters read errors until newline. A bounded
        // synthetic EOF must be used here, or a long comment can spin forever.
        if remaining == 0 {
            return Ok(0);
        }
        let length = bytes.len().min(remaining);
        let count = self.inner.read(&mut bytes[..length])?;
        self.remaining.fetch_sub(count as u64, Ordering::Relaxed);
        Ok(count)
    }
}

pub fn decode_ppm(
    source: impl Read,
    max_edge: u32,
    max_source_pixels: u64,
    max_output_pixels: u64,
    max_header_bytes: u64,
) -> Result<image::RgbaImage, DecodeError> {
    if max_edge == 0
        || max_edge > 3072
        || max_source_pixels == 0
        || max_source_pixels > MAX_SOURCE_PIXELS
        || max_output_pixels == 0
        || max_output_pixels > MAX_OUTPUT_PIXELS
        || max_header_bytes == 0
        || max_header_bytes > MAX_HEADER_BYTES
    {
        return Err(DecodeError::ResourceLimit);
    }
    let remaining = Arc::new(AtomicU64::new(max_header_bytes));
    let mut reader = LimitedReader {
        inner: BufReader::new(source),
        remaining: remaining.clone(),
    };
    let decoder = PnmDecoder::new(&mut reader).map_err(|_| {
        if remaining.load(Ordering::Relaxed) == 0 {
            DecodeError::ResourceLimit
        } else {
            DecodeError::InvalidData
        }
    })?;
    if !matches!(decoder.subtype(), PnmSubtype::Pixmap(_))
        || decoder.header().maximal_sample() != 255
    {
        return Err(DecodeError::Unsupported);
    }
    let (width, height) = decoder.dimensions();
    let pixels = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || pixels > max_source_pixels
        || decoder.total_bytes() != pixels * 3
    {
        return Err(DecodeError::ResourceLimit);
    }
    remaining.store(MAX_INPUT_BYTES + 1, Ordering::Relaxed);
    let mut rgb = vec![0; (pixels * 3) as usize];
    decoder
        .read_image(&mut rgb)
        .map_err(|_| DecodeError::InvalidData)?;
    // Single image: whitespace is accepted after ASCII pixels; another image or
    // arbitrary trailing payload is unsupported rather than silently discarded.
    let mut trailing = [0; 4096];
    loop {
        let count = reader
            .read(&mut trailing)
            .map_err(|_| DecodeError::InvalidData)?;
        if count == 0 {
            break;
        }
        if trailing[..count]
            .iter()
            .any(|byte| !byte.is_ascii_whitespace())
        {
            return Err(DecodeError::Unsupported);
        }
    }
    let image = DynamicImage::ImageRgb8(
        RgbImage::from_raw(width, height, rgb).ok_or(DecodeError::InvalidData)?,
    );
    let image = if width > max_edge || height > max_edge {
        image.thumbnail(max_edge, max_edge)
    } else {
        image
    };
    if u64::from(image.width()) * u64::from(image.height()) > max_output_pixels {
        return Err(DecodeError::ResourceLimit);
    }
    Ok(image.into_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn decode(bytes: &[u8]) -> Result<image::RgbaImage, DecodeError> {
        decode_ppm(
            bytes,
            1024,
            MAX_SOURCE_PIXELS,
            MAX_OUTPUT_PIXELS,
            MAX_HEADER_BYTES,
        )
    }
    #[test]
    fn real_p3_p6_content_controls_pixels() {
        let red = decode(b"P3\n2 1\n255\n255 0 0 0 255 0\n").unwrap();
        assert_eq!(red.into_raw(), [255, 0, 0, 255, 0, 255, 0, 255]);
        let blue = decode(b"P6\n1 1\n255\n\x00\x00\xff").unwrap();
        assert_eq!(blue.into_raw(), [0, 0, 255, 255]);
    }
    #[test]
    fn bounds_and_unsupported_subsets_fail_before_large_allocation() {
        assert_eq!(
            decode(b"P3\n99999 99999\n255\n"),
            Err(DecodeError::ResourceLimit)
        );
        assert_eq!(decode(b"P3\n1 1\n15\n1 2 3"), Err(DecodeError::Unsupported));
        assert_eq!(decode(b"P6\n1 1\n255\n\x01"), Err(DecodeError::InvalidData));
        assert_eq!(
            decode(b"P6\n1 1\n255\n\x01\x02\x03P6"),
            Err(DecodeError::Unsupported)
        );
        let mut header = b"P3\n#".to_vec();
        header.resize(MAX_HEADER_BYTES as usize + 1, b'x');
        assert_eq!(decode(&header), Err(DecodeError::ResourceLimit));
    }
}
