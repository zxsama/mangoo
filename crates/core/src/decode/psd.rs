use std::fs;

use anyhow::Context as _;

use super::{ensure_decode_pixel_budget, DecodePurpose, DecodeRequest, DecodedImage, MediaDecoder};

pub struct PsdDecoder;

impl MediaDecoder for PsdDecoder {
    fn name(&self) -> &'static str {
        "psd"
    }

    fn handles(&self, ext: &str, _purpose: DecodePurpose) -> bool {
        ext == ".psd"
    }

    fn decode(&self, request: &DecodeRequest<'_>) -> anyhow::Result<DecodedImage> {
        let path = request.source.path();
        let bytes = fs::read(path).with_context(|| format!("read PSD {}", path.display()))?;
        let document = psd::Psd::from_bytes(&bytes)
            .map_err(|error| anyhow::anyhow!("parse PSD {}: {error}", path.display()))?;
        let width = document.width();
        let height = document.height();
        ensure_decode_pixel_budget("psd", u64::from(width), u64::from(height))?;
        Ok(DecodedImage {
            rgba: document.rgba(),
            width,
            height,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::decode::MediaSource;

    #[test]
    fn claims_only_psd_images() {
        let decoder = PsdDecoder;
        assert!(decoder.handles(".psd", DecodePurpose::Thumbnail));
        assert!(decoder.handles(".psd", DecodePurpose::Preview));
        assert!(!decoder.handles(".raw", DecodePurpose::Preview));
    }

    #[test]
    fn malformed_psd_reports_a_decoder_error() {
        let path = std::env::temp_dir().join(format!(
            "megle-psd-decoder-{}-{}.psd",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        std::fs::write(&path, b"not a PSD").expect("write fixture");
        let result = PsdDecoder.decode(&DecodeRequest {
            source: MediaSource::Path(Path::new(&path)),
            ext: ".psd",
            target_max_px: None,
            purpose: DecodePurpose::Preview,
        });
        let _ = std::fs::remove_file(path);

        assert!(result
            .expect_err("malformed PSD must fail")
            .to_string()
            .contains("parse PSD"));
    }

    #[test]
    fn decodes_a_minimal_composite_psd_to_rgba() {
        let path = std::env::temp_dir().join(format!(
            "megle-psd-decoder-valid-{}-{}.psd",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        // PSD v1, 1x1 RGB8, no resources/layers, raw planar composite pixel.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"8BPS");
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&[0; 6]);
        bytes.extend_from_slice(&3u16.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&8u16.to_be_bytes());
        bytes.extend_from_slice(&3u16.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.extend_from_slice(&[0, 255, 0]);
        std::fs::write(&path, bytes).expect("write PSD fixture");

        let decoded = PsdDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(Path::new(&path)),
                ext: ".psd",
                target_max_px: None,
                purpose: DecodePurpose::Preview,
            })
            .expect("decode PSD");
        let _ = std::fs::remove_file(path);

        assert_eq!((decoded.width, decoded.height), (1, 1));
        assert_eq!(decoded.rgba, vec![0, 255, 0, 255]);
    }
}
