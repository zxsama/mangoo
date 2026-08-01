//! The single table of what a file extension means.
//!
//! Scan, the watcher, thumbnail generation, and the preview resolver all read
//! from here. This used to be two byte-identical private copies in
//! `scan/mod.rs` and `watch/mod.rs`, which is how `.heic` ended up indexed and
//! `.heif` did not.

use std::path::Path;

/// Extensions indexed as `media.kind = "image"`. Indexing is deliberately wider
/// than decoding: a `.psd` or `.raw` is still a real picture the user expects to
/// SEE in the grid, even while the decode chain can only report why it failed.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    ".avif", ".bmp", ".gif", ".heic", ".heif", ".jpeg", ".jpg", ".png", ".psd", ".raw", ".tif",
    ".tiff", ".webp",
];

/// Extensions indexed as `media.kind = "video"`.
pub const VIDEO_EXTENSIONS: &[&str] = &[".avi", ".m4v", ".mkv", ".mov", ".mp4", ".webm", ".wmv"];

/// What Chromium paints from raw bytes in an `<img>`/`createImageBitmap`.
/// Anything outside this set must be transcoded to WebP by Core before the
/// frontend can show it.
const BROWSER_RENDERABLE_IMAGE_EXTENSIONS: &[&str] =
    &[".avif", ".bmp", ".gif", ".jpeg", ".jpg", ".png", ".webp"];

/// Formats that can carry more than one frame. The preview surface keeps these
/// on `<img>` for native animation — a canvas would freeze them on frame one.
const ANIMATION_CAPABLE_IMAGE_EXTENSIONS: &[&str] = &[".avif", ".gif", ".webp"];

/// Lowercase extension INCLUDING the leading dot, or `""` when the file has
/// none. This exact shape is what `files.ext` stores and what
/// `DecodeRequest::ext` expects.
pub fn lowercase_extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_default()
}

pub fn media_kind_for_extension(ext: &str) -> Option<&'static str> {
    if IMAGE_EXTENSIONS.contains(&ext) {
        return Some("image");
    }
    if VIDEO_EXTENSIONS.contains(&ext) {
        return Some("video");
    }
    None
}

/// Path-form convenience over [`lowercase_extension`] + [`media_kind_for_extension`].
/// Test-only: every production caller (scan, the watcher) needs the extension
/// string anyway to store in `files.ext`, so they call the two-step form and
/// pay one allocation per file instead of two. Drop the gate if a caller ever
/// genuinely has only a path.
#[cfg(test)]
pub fn media_kind_for_path(path: &Path) -> Option<&'static str> {
    media_kind_for_extension(lowercase_extension(path).as_str())
}

/// Task 7's preview source resolver decides transcode-vs-serve-original with
/// this; nothing outside tests reads it until then.
#[allow(dead_code)]
pub fn is_browser_renderable_image(ext: &str) -> bool {
    BROWSER_RENDERABLE_IMAGE_EXTENSIONS.contains(&ext)
}

/// Task 7/10 keep multi-frame formats on `<img>` with this; nothing outside
/// tests reads it until then.
#[allow(dead_code)]
pub fn is_animated_capable_image(ext: &str) -> bool {
    ANIMATION_CAPABLE_IMAGE_EXTENSIONS.contains(&ext)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn classifies_indexed_image_and_video_extensions() {
        for name in [
            "a.avif", "a.bmp", "a.gif", "a.heic", "a.heif", "a.jpeg", "a.jpg", "a.png", "a.psd",
            "a.raw", "a.tif", "a.tiff", "a.webp",
        ] {
            assert_eq!(
                media_kind_for_path(Path::new(name)),
                Some("image"),
                "{name}"
            );
        }
        for name in [
            "a.avi", "a.m4v", "a.mkv", "a.mov", "a.mp4", "a.webm", "a.wmv",
        ] {
            assert_eq!(
                media_kind_for_path(Path::new(name)),
                Some("video"),
                "{name}"
            );
        }
        for name in ["a.txt", "a.zip", "a", "a.PDF"] {
            assert_eq!(media_kind_for_path(Path::new(name)), None, "{name}");
        }
    }

    #[test]
    fn heif_is_indexed_alongside_heic() {
        // Pre-existing inconsistency: `.heic` was indexed and `.heif` was not,
        // even though they are the same container.
        assert_eq!(media_kind_for_path(Path::new("a.heif")), Some("image"));
    }

    #[test]
    fn extension_is_lowercased_with_a_leading_dot() {
        assert_eq!(lowercase_extension(Path::new("A.JPG")), ".jpg");
        assert_eq!(lowercase_extension(Path::new("a.tar.GZ")), ".gz");
        assert_eq!(lowercase_extension(Path::new("noext")), "");
    }

    #[test]
    fn browser_renderable_set_excludes_what_chromium_cannot_paint() {
        for ext in [".jpg", ".jpeg", ".png", ".gif", ".webp", ".avif", ".bmp"] {
            assert!(is_browser_renderable_image(ext), "{ext} should render");
        }
        for ext in [".heic", ".heif", ".psd", ".raw", ".tif", ".tiff"] {
            assert!(!is_browser_renderable_image(ext), "{ext} must transcode");
        }
    }

    #[test]
    fn animation_capable_set_is_the_canvas_opt_out() {
        // These may hold multiple frames, so the preview must keep using <img>
        // (a canvas would freeze on frame one).
        for ext in [".gif", ".webp", ".avif"] {
            assert!(is_animated_capable_image(ext), "{ext}");
        }
        for ext in [".jpg", ".png", ".bmp", ".heic"] {
            assert!(!is_animated_capable_image(ext), "{ext}");
        }
    }
}
