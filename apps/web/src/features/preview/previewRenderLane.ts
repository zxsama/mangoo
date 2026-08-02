export type PreviewRenderLane = "canvas" | "image" | "video";

// Formats that may carry more than one frame. A canvas paints exactly one
// bitmap, so these must stay on <img> or they freeze on frame one. Mirrors
// `decode::formats::ANIMATION_CAPABLE_IMAGE_EXTENSIONS` in Core.
export const ANIMATION_CAPABLE_EXTENSIONS = ["gif", "webp", "avif"] as const;

function normalizeExtension(ext?: string | null): string {
  if (!ext) {
    return "";
  }
  return ext.replace(/^\./, "").toLowerCase();
}

/**
 * Which element paints this preview.
 *
 * Only the ORIGINAL of a still image takes the canvas fast path — that is the
 * one that costs a full-resolution main-thread decode per navigation (measured
 * 337ms for a 16384px image). The thumbnail fallback is a 320px WebP that
 * decodes instantly and would only spend pool budget; video needs a real
 * `<video>`; animation-capable formats need `<img>` to keep animating.
 */
export function selectPreviewRenderLane(input: {
  source: "thumbnail" | "original";
  kind?: string | null;
  ext?: string | null;
}): PreviewRenderLane {
  if (input.kind === "video") {
    return "video";
  }
  if (input.source !== "original" || input.kind !== "image") {
    return "image";
  }
  const ext = normalizeExtension(input.ext);
  if (!ext || (ANIMATION_CAPABLE_EXTENSIONS as readonly string[]).includes(ext)) {
    return "image";
  }
  return "canvas";
}

/**
 * Backing store = the decoded bitmap's size. A 16384x8192 backing store would
 * be 536 MB of GPU memory; a preview_3072 one is 21 MB and the browser scales
 * it into the layout box for free.
 *
 * CSS box = the bitmap's SHAPE scaled to the media's natural SIZE. Taking the
 * shape from `natural` instead was a real bug: Core and the browser can
 * disagree about orientation, and then a portrait bitmap got sized into a
 * landscape box — `object-fit: contain` letterboxed it and both fit-long-edge
 * and "actual pixels" zoom were wrong by that factor. Core now applies EXIF so
 * the two agree, but deriving shape from the thing actually being painted means
 * a future disagreement degrades to a slightly-wrong scale rather than a
 * visibly broken frame.
 */
export function canvasPaintTarget(
  bitmap: { width: number; height: number },
  natural: { naturalWidth: number; naturalHeight: number } | null
): { backingWidth: number; backingHeight: number; cssWidth: number; cssHeight: number } {
  const hasNatural = !!natural && natural.naturalWidth > 0 && natural.naturalHeight > 0;
  const bitmapLongEdge = Math.max(bitmap.width, bitmap.height);
  if (!hasNatural || bitmapLongEdge <= 0) {
    return {
      backingWidth: bitmap.width,
      backingHeight: bitmap.height,
      cssWidth: bitmap.width,
      cssHeight: bitmap.height
    };
  }
  // Scale the bitmap's own aspect up to the media's long edge. For a tier this
  // restores the original's display size; for a full-resolution frame it is a
  // no-op (scale === 1).
  const scale = Math.max(natural.naturalWidth, natural.naturalHeight) / bitmapLongEdge;
  return {
    backingWidth: bitmap.width,
    backingHeight: bitmap.height,
    cssWidth: Math.max(1, Math.round(bitmap.width * scale)),
    cssHeight: Math.max(1, Math.round(bitmap.height * scale))
  };
}
