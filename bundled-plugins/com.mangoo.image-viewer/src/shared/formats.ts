/**
 * Format helpers shared by the Desktop host and the viewer UI. The list of
 * supported extensions is NOT defined here: it always comes from Core
 * (`GET /api/image-viewer/formats`), which mirrors the project decode registry
 * and running decoder plugins. This module only maps those extensions to the
 * MIME types an operating system association needs.
 */

const MIME_BY_EXTENSION: Record<string, string> = {
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".png": "image/png",
  ".gif": "image/gif",
  ".webp": "image/webp",
  ".bmp": "image/bmp",
  ".tif": "image/tiff",
  ".tiff": "image/tiff",
  ".avif": "image/avif",
  ".heic": "image/heic",
  ".heif": "image/heif",
  ".psd": "image/vnd.adobe.photoshop",
  ".dng": "image/x-adobe-dng",
  ".cr2": "image/x-canon-cr2",
  ".nef": "image/x-nikon-nef",
  ".arw": "image/x-sony-arw",
  ".orf": "image/x-olympus-orf",
  ".rw2": "image/x-panasonic-rw2",
  ".raw": "image/x-panasonic-raw",
  ".ppm": "image/x-portable-pixmap",
  ".qoi": "image/qoi",
  ".tga": "image/x-tga",
  ".ico": "image/vnd.microsoft.icon"
};

/** Lowercase dotted extension (".jpg") of a path, or "" when it has none. */
export function extensionOf(path: string): string {
  const name = path.slice(Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1);
  const dot = name.lastIndexOf(".");
  return dot <= 0 ? "" : name.slice(dot).toLowerCase();
}

export function mimeTypeForExtension(extension: string): string | null {
  return MIME_BY_EXTENSION[extension.toLowerCase()] ?? null;
}

/** Valid association extension: dotted, short, lowercase alphanumeric. */
export function isAssociableExtension(extension: string): boolean {
  return /^\.[a-z0-9]{1,16}$/.test(extension);
}

/** Folder + file name split that works for both separators. */
export function splitPath(path: string): { directory: string; name: string } {
  const index = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return index < 0
    ? { directory: "", name: path }
    : { directory: path.slice(0, index) || path.slice(0, 1), name: path.slice(index + 1) };
}

/**
 * Whether an image's leading bytes declare animation, so the viewer keeps it
 * on an animating <img> instead of freezing frame one on a canvas.
 * GIF: always (single-frame GIFs render identically either way).
 * WebP: VP8X header with the animation flag. AVIF: an "avis" brand in ftyp.
 */
export function isAnimatedImageHeader(bytes: Uint8Array, extension: string): boolean {
  const ascii = (start: number, length: number) =>
    String.fromCharCode(...bytes.subarray(start, Math.min(bytes.length, start + length)));
  if (extension === ".gif") return ascii(0, 3) === "GIF";
  if (extension === ".webp") {
    return ascii(0, 4) === "RIFF" && ascii(8, 4) === "WEBP" && ascii(12, 4) === "VP8X" && bytes.length > 20 && (bytes[20] & 0x02) !== 0;
  }
  if (extension === ".avif") {
    if (ascii(4, 4) !== "ftyp") return false;
    const size = ((bytes[0] << 24) | (bytes[1] << 16) | (bytes[2] << 8) | bytes[3]) >>> 0;
    for (let offset = 8; offset + 4 <= Math.min(size, bytes.length); offset += 4) {
      if (ascii(offset, 4) === "avis") return true;
    }
  }
  return false;
}
