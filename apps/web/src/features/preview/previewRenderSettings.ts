// Pure mapping from the two preview render preferences to the CSS that applies
// them. No DOM, no React — the whole contract is testable under node --test.
//
// Both settings are CSS-level on purpose, applied to the shared `.preview-image`
// class that the canvas, <img> and <video> lanes all carry. One rule set covers
// every lane, and nothing repaints or re-decodes — which matters, because the
// preview pipeline exists to decode once ahead of time and a per-pixel
// implementation would undo it.

export type PreviewRenderFilter = "bilinear" | "nearest";
export type PreviewChannel = "rgba" | "rgb" | "r" | "g" | "b" | "a";

export const DEFAULT_PREVIEW_RENDER_FILTER: PreviewRenderFilter = "bilinear";
export const DEFAULT_PREVIEW_CHANNEL: PreviewChannel = "rgba";

// Named after the resampling algorithm the browser actually runs, never after a
// quality adjective. `high-quality` is deliberately absent: Chromium 150 reports
// `CSS.supports("image-rendering", "high-quality") === false`, so a third row
// would silently render as plain `auto` — a label for behaviour that does not
// happen. Measured in docs/benchmarks/2026-08-02-preview-render-browser-checks.md.
export const PREVIEW_RENDER_FILTERS: ReadonlyArray<{
  value: PreviewRenderFilter;
  label: string;
}> = [
  { value: "bilinear", label: "双线性 Bilinear" },
  { value: "nearest", label: "最近邻 Nearest neighbour" }
];

export const PREVIEW_CHANNELS: ReadonlyArray<{ value: PreviewChannel; label: string }> = [
  { value: "rgba", label: "RGBA" },
  { value: "rgb", label: "RGB" },
  { value: "r", label: "R" },
  { value: "g", label: "G" },
  { value: "b", label: "B" },
  { value: "a", label: "A" }
];

export function imageRenderingForFilter(filter: PreviewRenderFilter): string {
  return filter === "nearest" ? "pixelated" : "auto";
}

const CHANNEL_FILTER_IDS: Record<Exclude<PreviewChannel, "rgba">, string> = {
  rgb: "mangoo-channel-rgb",
  r: "mangoo-channel-r",
  g: "mangoo-channel-g",
  b: "mangoo-channel-b",
  a: "mangoo-channel-a"
};

/**
 * The SVG filter id for a channel view, or `null` for the default RGBA view.
 *
 * `null` is load-bearing: the default must apply NO `filter` property at all, so
 * a user who never opens the panel pays nothing.
 */
export function channelFilterId(channel: PreviewChannel): string | null {
  return channel === "rgba" ? null : CHANNEL_FILTER_IDS[channel];
}

// 4x5 feColorMatrix rows: R, G, B, A. Each row is [r g b a offset].
//
// Every matrix ends with the alpha row `0 0 0 0 1`, forcing A=1. That is what
// makes these views show the RAW stored value: SVG defines feColorMatrix to
// operate on NON-premultiplied colour, so passing RGB through with alpha forced
// opaque yields the stored RGB undarkened — the whole point for formats like
// TGA where A carries data rather than transparency.
//
// Verified empirically on Chromium 150 before this was written: a 50%-alpha red
// pixel through the RGB matrix reads back R=255, not R=128. See
// docs/benchmarks/2026-08-02-preview-render-browser-checks.md.
export const CHANNEL_MATRICES: ReadonlyArray<{ id: string; values: string }> = [
  // Colour as stored, alpha ignored.
  { id: CHANNEL_FILTER_IDS.rgb, values: "1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 0 1" },
  // One channel broadcast to R, G and B so it reads as greyscale.
  { id: CHANNEL_FILTER_IDS.r, values: "1 0 0 0 0  1 0 0 0 0  1 0 0 0 0  0 0 0 0 1" },
  { id: CHANNEL_FILTER_IDS.g, values: "0 1 0 0 0  0 1 0 0 0  0 1 0 0 0  0 0 0 0 1" },
  { id: CHANNEL_FILTER_IDS.b, values: "0 0 1 0 0  0 0 1 0 0  0 0 1 0 0  0 0 0 0 1" },
  { id: CHANNEL_FILTER_IDS.a, values: "0 0 0 1 0  0 0 0 1 0  0 0 0 1 0  0 0 0 0 1" }
];
