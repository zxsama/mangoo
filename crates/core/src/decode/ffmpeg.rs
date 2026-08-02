//! The capability decoder: an ffmpeg subprocess for everything the `image`
//! crate cannot decode without new C dependencies — AVIF, HEIC/HEIF, PSD, and
//! TIFF variants `image` chokes on. ffmpeg is already a dependency of the video
//! poster path, so this adds reach without adding a build dependency.

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use image::ImageReader;

use super::subprocess::{wait_with_timeout, ChildWaitOutcome};
use super::{DecodePurpose, DecodeRequest, DecodedImage, MediaDecoder, DECODER_UNAVAILABLE_MARKER};

/// Wall-clock cap for one still-image decode. Shorter than the 30s video-poster
/// budget: a still frame that takes longer than this is pathological, and the
/// preview path is interactive.
const FFMPEG_DECODE_TIMEOUT: Duration = Duration::from_secs(20);

/// Longest-side ceiling applied when the caller supplies no `target_max_px`.
///
/// 8192 covers every real photograph (up to ~67 MP), so reported source
/// dimensions stay exact for anything a user actually owns; it exists purely so
/// a pathological gigapixel scan cannot write an unbounded temp PNG or blow the
/// readback's allocation limit. See the rationale at the call site.
const FFMPEG_DECODE_CEILING_PX: u32 = 8192;

/// Extensions this decoder claims. Images only — video thumbnails keep their
/// dedicated `generate_video_thumbnail_bytes` path, which already emits WebP
/// directly and needs no RGBA round trip.
///
/// `.tif`/`.tiff` are claimed even though `ImageCrateDecoder` claims them too:
/// it runs first, and ffmpeg is the fall-through for the compression variants
/// (old JPEG-in-TIFF, LZW predictors, 32-bit float) that `image` rejects.
/// `.psd` is claimed speculatively — ffmpeg decodes many flattened PSDs; when
/// it cannot, the honest chain error is still better than today's silence.
const SUPPORTED_EXTENSIONS: &[&str] = &[".avif", ".heic", ".heif", ".psd", ".tif", ".tiff"];

/// Detects whether `ffmpeg` is on PATH. Cached for the process lifetime so the
/// worker pays the spawn cost once.
pub fn ffmpeg_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| match Command::new("ffmpeg").arg("-version").output() {
        Ok(output) => output.status.success(),
        Err(_) => false,
    })
}

fn unavailable_error() -> anyhow::Error {
    anyhow::anyhow!("{DECODER_UNAVAILABLE_MARKER}: ffmpeg is not installed or not on PATH")
}

/// "Fit inside a `max_px` box, never upscale, preserve aspect ratio."
/// The single quotes protect the commas from ffmpeg's filtergraph parser;
/// no shell is involved, so they reach ffmpeg literally.
pub fn ffmpeg_scale_filter(max_px: u32) -> String {
    format!(
        "scale=w='min({max_px},iw)':h='min({max_px},ih)':force_original_aspect_ratio=decrease:flags=area"
    )
}

pub struct FfmpegDecoder;

impl MediaDecoder for FfmpegDecoder {
    fn name(&self) -> &'static str {
        "ffmpeg"
    }

    fn handles(&self, ext: &str, _purpose: DecodePurpose) -> bool {
        SUPPORTED_EXTENSIONS.contains(&ext)
    }

    fn decode(&self, request: &DecodeRequest<'_>) -> anyhow::Result<DecodedImage> {
        if !ffmpeg_available() {
            return Err(unavailable_error());
        }
        let source_path = request.source.path();
        // ffmpeg's stderr is its own English prose and carries none of
        // `classify_thumbnail_failure`'s transient signatures. Because this
        // decoder is the ONLY chain link for .avif/.heic/.heif/.psd, an
        // unreachable NAS would otherwise produce an aggregate error with no
        // transient signature at all and be marked permanently failed. Stat
        // first so the OS error (e.g. `os error 53`, 找不到网络路径) is the leaf,
        // and skip a pointless subprocess spawn while we are at it.
        fs::metadata(source_path).map_err(|error| anyhow::anyhow!("ffmpeg source: {error}"))?;
        let scratch = scratch_path();
        if let Some(parent) = scratch.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| anyhow::anyhow!("ffmpeg scratch dir: {error}"))?;
        }

        let mut command = Command::new("ffmpeg");
        command
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(source_path)
            .args(["-frames:v", "1"]);
        // ALWAYS scale, even when the caller supplied no cap.
        //
        // The thumbnail path deliberately passes `target_max_px: None` so
        // `media.width/height` stay the true source dimensions. Without a
        // ceiling here that meant ffmpeg wrote a FULL-RESOLUTION PNG to %TEMP%
        // and Core decoded it back: ~97 MB of heap and ~40 MB of temp I/O for
        // an ordinary 12 MP phone HEIC — to produce a 320px tile — times up to
        // 13 concurrent thumbnail workers. Above ~134 MP the readback also blew
        // `image`'s default 512 MB allocation limit, and that failure text
        // ("Memory limit exceeded") is a TRANSIENT signature, so the 60s sweep
        // re-ran the whole doomed round trip every minute, forever.
        //
        // The ceiling is set high enough (8192 long side = up to 67 MP) that no
        // real photograph is scaled, so reported source dimensions stay exact
        // for every file a user actually owns. Only pathological gigapixel
        // scans are downscaled, where slightly approximate dimensions are
        // vastly better than an unbounded temp file and a permanent retry loop.
        let max_px = request.target_max_px.unwrap_or(FFMPEG_DECODE_CEILING_PX);
        command.args(["-vf", &ffmpeg_scale_filter(max_px)]);
        // PNG, not rawvideo: rawvideo carries no dimensions, so reading it back
        // would need a second ffprobe subprocess. PNG is self-describing and
        // keeps alpha unambiguously (HEIC/AVIF can be transparent).
        //
        // `-compression_level 1`: this PNG exists for microseconds and is read
        // back by this same process. Paying maximum zlib on the write and then
        // full inflate on the read is pure waste on the hot thumbnail path.
        //
        // The frame goes to a FILE, not stdout: `wait_with_timeout` does not
        // drain pipes, so a multi-megabyte frame on a 64 KB stdout pipe would
        // deadlock ffmpeg until the timeout. This mirrors the proven shape of
        // `thumbnails::generate_video_thumbnail`.
        command
            .args([
                "-f",
                "image2",
                "-c:v",
                "png",
                "-compression_level",
                "1",
                "-y",
            ])
            .arg(&scratch)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let result = run_and_read(&mut command, &scratch);
        let _ = fs::remove_file(&scratch);
        result
    }
}

fn run_and_read(command: &mut Command, scratch: &std::path::Path) -> anyhow::Result<DecodedImage> {
    let mut child = command
        .spawn()
        .map_err(|error| anyhow::anyhow!("ffmpeg spawn: {error}"))?;
    let outcome = wait_with_timeout(&mut child, FFMPEG_DECODE_TIMEOUT)
        .map_err(|error| anyhow::anyhow!("ffmpeg wait: {error}"))?;
    let status = match outcome {
        ChildWaitOutcome::Exited(status) => status,
        ChildWaitOutcome::TimedOut => {
            return Err(anyhow::anyhow!(
                "ffmpeg timed out after {}s",
                FFMPEG_DECODE_TIMEOUT.as_secs()
            ));
        }
    };

    // The child has exited; drain both pipes now that they cannot block.
    let mut stderr_buf = Vec::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_end(&mut stderr_buf);
    }
    if let Some(mut stdout) = child.stdout.take() {
        let mut sink = Vec::new();
        let _ = stdout.read_to_end(&mut sink);
    }

    if !status.success() {
        let code = match status.code() {
            Some(code) => code.to_string(),
            None => "signal".to_string(),
        };
        return Err(anyhow::anyhow!(
            "ffmpeg exit {code} stderr: {}",
            String::from_utf8_lossy(&stderr_buf).trim()
        ));
    }

    let decoded = ImageReader::open(scratch)
        .map_err(|error| anyhow::anyhow!("ffmpeg output open: {error}"))?
        .with_guessed_format()
        .map_err(|error| anyhow::anyhow!("ffmpeg output format: {error}"))?
        .decode()
        .map_err(|error| anyhow::anyhow!("ffmpeg output decode: {error}"))?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 {
        return Err(anyhow::anyhow!("ffmpeg produced a zero-sized frame"));
    }
    Ok(DecodedImage {
        rgba: decoded.to_rgba8().into_raw(),
        width,
        height,
    })
}

fn scratch_path() -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "megle-decode-{}-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::decode::{DecodePurpose, DecodeRequest, MediaSource, DECODER_UNAVAILABLE_MARKER};

    fn unique_temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "megle_decode_ffmpeg_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn claims_the_browser_and_image_crate_gaps() {
        let decoder = FfmpegDecoder;
        for ext in [".avif", ".heic", ".heif", ".psd", ".tif", ".tiff"] {
            assert!(
                decoder.handles(ext, DecodePurpose::Thumbnail),
                "should claim {ext}"
            );
            assert!(
                decoder.handles(ext, DecodePurpose::Preview),
                "should claim {ext} for preview too"
            );
        }
    }

    #[test]
    fn does_not_claim_video_or_unknown_extensions() {
        let decoder = FfmpegDecoder;
        // Video thumbnails keep their dedicated WebP-producing path.
        for ext in [".mp4", ".mkv", ".mov", ".webm", ".wmv", ".txt", ""] {
            assert!(
                !decoder.handles(ext, DecodePurpose::Thumbnail),
                "should not claim {ext}"
            );
        }
    }

    #[test]
    fn scale_filter_fits_inside_the_cap_without_upscaling() {
        let filter = ffmpeg_scale_filter(320);
        assert!(filter.contains("min(320,iw)"), "{filter}");
        assert!(filter.contains("min(320,ih)"), "{filter}");
        assert!(
            filter.contains("force_original_aspect_ratio=decrease"),
            "{filter}"
        );
    }

    #[test]
    fn missing_ffmpeg_is_reported_as_decoder_unavailable_not_unsupported_format() {
        // Drive the pure branch directly so the test result does not depend on
        // whether this machine happens to have ffmpeg installed.
        let error = unavailable_error();
        let text = error.to_string();
        assert!(text.contains(DECODER_UNAVAILABLE_MARKER), "{text}");
        assert!(text.contains("ffmpeg"), "{text}");
    }

    #[test]
    fn decodes_a_real_avif_through_ffmpeg() {
        if !ffmpeg_available() {
            eprintln!("SKIP decodes_a_real_avif_through_ffmpeg: ffmpeg not on PATH");
            return;
        }
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("fixture.avif");
        // Let ffmpeg itself author the fixture, so the test needs no binary
        // blob in the repo and exercises the same codec path end to end.
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=64x48:d=1",
                "-frames:v",
                "1",
                "-y",
            ])
            .arg(&source)
            .status()
            .expect("spawn ffmpeg to author the fixture");
        if !status.success() {
            eprintln!("SKIP decodes_a_real_avif_through_ffmpeg: this ffmpeg cannot encode avif");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        // Pin that ffmpeg really produced AVIF and did not silently fall back
        // to another container under the .avif name.
        assert_eq!(
            &std::fs::read(&source).expect("read fixture")[4..12],
            b"ftypavif"
        );

        let decoded = FfmpegDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(&source),
                ext: ".avif",
                target_max_px: None,
                purpose: DecodePurpose::Preview,
            })
            .expect("ffmpeg decodes avif");

        assert_eq!((decoded.width, decoded.height), (64, 48));
        assert_eq!(decoded.rgba.len(), 64 * 48 * 4);
        // color=c=red — a YUV->RGB regression producing a black frame would
        // otherwise pass every dimension assertion.
        assert!(decoded.rgba[0] > 200, "red channel was {}", decoded.rgba[0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn honours_the_target_max_px_cap() {
        if !ffmpeg_available() {
            eprintln!("SKIP honours_the_target_max_px_cap: ffmpeg not on PATH");
            return;
        }
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("wide.png");
        let mut buffer = image::RgbaImage::new(800, 400);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgba([9, 9, 9, 255]);
        }
        image::DynamicImage::ImageRgba8(buffer)
            .save(&source)
            .expect("write fixture");

        let decoded = FfmpegDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(&source),
                ext: ".tif", // any claimed extension; content is sniffed by ffmpeg
                target_max_px: Some(200),
                purpose: DecodePurpose::Preview,
            })
            .expect("ffmpeg decodes and scales");

        assert_eq!(decoded.width, 200, "cap must be filled, not undershot");
        assert!(decoded.height <= 200, "height {}", decoded.height);
        // Aspect ratio 2:1 preserved within a pixel of rounding.
        assert!(
            (decoded.width as i64 - decoded.height as i64 * 2).abs() <= 2,
            "{}x{}",
            decoded.width,
            decoded.height
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_source_fails_with_the_ffmpeg_stderr_text() {
        if !ffmpeg_available() {
            eprintln!("SKIP a_corrupt_source_fails_with_the_ffmpeg_stderr_text: no ffmpeg");
            return;
        }
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("garbage.heic");
        std::fs::write(&source, b"not an image at all").expect("write garbage");

        let error = FfmpegDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(&source),
                ext: ".heic",
                target_max_px: None,
                purpose: DecodePurpose::Preview,
            })
            .expect_err("garbage cannot decode");

        let text = error.to_string();
        assert!(text.contains("ffmpeg"), "{text}");
        assert!(text.contains("stderr:"), "{text}");
        assert!(!text.contains(DECODER_UNAVAILABLE_MARKER), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreachable_source_surfaces_the_os_error_not_ffmpeg_prose() {
        if !ffmpeg_available() {
            eprintln!(
                "SKIP an_unreachable_source_surfaces_the_os_error_not_ffmpeg_prose: no ffmpeg"
            );
            return;
        }
        let missing = std::env::temp_dir().join("megle-decode-does-not-exist-12345.heic");
        let _ = std::fs::remove_file(&missing);

        let error = FfmpegDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(&missing),
                ext: ".heic",
                target_max_px: None,
                purpose: DecodePurpose::Preview,
            })
            .expect_err("missing source cannot decode");

        // `classify_thumbnail_failure` substring-matches this text; an OS error
        // code must be present or a transient outage reads as permanent.
        let text = error.to_string();
        assert!(text.contains("os error"), "{text}");
        assert!(text.contains("ffmpeg source"), "{text}");
    }

    #[test]
    fn a_cap_larger_than_the_source_does_not_upscale() {
        if !ffmpeg_available() {
            eprintln!("SKIP a_cap_larger_than_the_source_does_not_upscale: no ffmpeg");
            return;
        }
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("small.png");
        let mut buffer = image::RgbaImage::new(64, 48);
        for pixel in buffer.pixels_mut() {
            *pixel = image::Rgba([5, 5, 5, 255]);
        }
        image::DynamicImage::ImageRgba8(buffer)
            .save(&source)
            .expect("write fixture");

        let decoded = FfmpegDecoder
            .decode(&DecodeRequest {
                source: MediaSource::Path(&source),
                ext: ".tif",
                target_max_px: Some(400),
                purpose: DecodePurpose::Preview,
            })
            .expect("decodes");

        assert_eq!(
            (decoded.width, decoded.height),
            (64, 48),
            "must not upscale"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_registry_puts_the_cheap_decoder_first() {
        let registry = crate::decode::default_registry();
        assert_eq!(registry.decoder_names(), vec!["image_crate", "ffmpeg"]);
    }
}
