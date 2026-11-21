export const workbenchLayout = {
  navWidth: 72,
  sidebarWidth: 292,
  tileMinWidth: 148,
  tileGap: 7,
  tileLabelHeight: 17
} as const;

export const radiusTokens = {
  window: 8,
  overlay: 8,
  panel: 8,
  surface: 8,
  control: 8,
  content: 8,
  tight: 4,
  pill: 999
} as const;

// Phosphor bold adds ink on both sides of a stroke. These viewBoxes fit its
// path bounds back into the original regular glyph's footprint on each axis.
// Keep the original CSS-pixel sizes, including the smaller caption symbols.
function boldGlyph(size: number, viewBox: string, preserveAspectRatio = "none") {
  return { size, viewBox, preserveAspectRatio, weight: "bold", "aria-hidden": true } as const;
}

export const titlebarIcon = {
  switchLibrary: boldGlyph(14, "-9.2 -4.93 274.4 265.89"),
  plugins: boldGlyph(16, "-21.85 -3.46 275.7 246.93"),
  settings: boldGlyph(16, "-4.52 -4.9 265.08 265.79"),
  locate: boldGlyph(16, "-4.57 -4.57 265.14 265.14"),
  sidebar: boldGlyph(16, "-4.92 -5.82 265.85 267.64"),
  filter: boldGlyph(15, "-5.33 -6.29 266.67 268.58"),
  sort: boldGlyph(14, "-5.86 -5.84 267.71 267.7"),
  layout: {
    adaptive: boldGlyph(14, "-5.82 -5.82 267.64 267.64"),
    waterfall: boldGlyph(14, "-6.4 -5.33 268.8 266.67"),
    grid: boldGlyph(14, "-4.92 -6.4 265.85 268.8"),
    list: boldGlyph(14, "-5.33 -6.74 266.67 269.47")
  },
  notifications: boldGlyph(15, "-5.34 -4.93 266.69 265.89"),
  // Keep caption symbols centered and proportional in their square SVG boxes.
  maximize: boldGlyph(13, "-17.46 -17.46 290.91 290.91", "xMidYMid meet"),
  restore: boldGlyph(13, "-17.45 -17.45 290.89 290.89", "xMidYMid meet"),
  close: boldGlyph(14, "-6.45 -6.45 268.9 268.9", "xMidYMid meet")
} as const;
