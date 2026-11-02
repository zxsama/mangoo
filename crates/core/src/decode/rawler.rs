use anyhow::Context as _;
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;

use super::{ensure_decode_pixel_budget, DecodePurpose, DecodeRequest, DecodedImage, MediaDecoder};

const SUPPORTED_EXTENSIONS: &[&str] = &[".raw", ".cr2", ".nef", ".arw", ".dng", ".orf", ".rw2"];

pub struct RawlerDecoder;

impl MediaDecoder for RawlerDecoder {
    fn name(&self) -> &'static str {
        "rawler"
    }

    fn handles(&self, ext: &str, _purpose: DecodePurpose) -> bool {
        SUPPORTED_EXTENSIONS.contains(&ext)
    }

    fn decode(&self, request: &DecodeRequest<'_>) -> anyhow::Result<DecodedImage> {
        let path = request.source.path();
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
        let rgba = developed.to_rgba8();
        let width = rgba.width();
        let height = rgba.height();
        Ok(DecodedImage {
            rgba: rgba.into_raw(),
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
    fn claims_the_indexed_camera_raw_extensions() {
        let decoder = RawlerDecoder;
        for ext in SUPPORTED_EXTENSIONS {
            assert!(decoder.handles(ext, DecodePurpose::Thumbnail), "{ext}");
            assert!(decoder.handles(ext, DecodePurpose::Preview), "{ext}");
        }
        assert!(!decoder.handles(".psd", DecodePurpose::Preview));
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
        let _ = std::fs::remove_file(path);

        assert!(error.to_string().contains("decode camera RAW"), "{error:#}");
    }
}
