/**
 * Geometry for floating panels inside the viewer window (image info, frame
 * inspector). A panel floats freely, snaps magnetically to the window edges
 * and corners while dragged, and remembers where it was. Pure and testable.
 */

export interface PanelRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Bounds {
  width: number;
  height: number;
}

/** Which window edges a panel is attached to (null = floating on that axis). */
export interface PanelDock {
  horizontal: "left" | "right" | null;
  vertical: "top" | "bottom" | null;
}

export interface PanelPlacement {
  /** Position relative to the docked edge (or the window origin when floating). */
  rect: PanelRect;
  dock: PanelDock;
}

/** Distance (px) at which a dragged panel snaps to an edge. */
export const SNAP_DISTANCE = 18;
/** Gap kept between a snapped panel and the window edge (flush). */
export const EDGE_GAP = 0;

/** Smallest panel size; also the CSS min-width, so stored == rendered. */
export const MIN_PANEL_WIDTH = 260;
export const MIN_PANEL_HEIGHT = 120;

export function clampRect(rect: PanelRect, bounds: Bounds, insets: { top: number; bottom: number }): PanelRect {
  const width = Math.min(Math.max(MIN_PANEL_WIDTH, rect.width), Math.max(MIN_PANEL_WIDTH, bounds.width - EDGE_GAP * 2));
  const maxHeight = Math.max(120, bounds.height - insets.top - insets.bottom - EDGE_GAP * 2);
  const height = Math.min(rect.height, maxHeight);
  return {
    width,
    height,
    x: Math.min(Math.max(EDGE_GAP, rect.x), Math.max(EDGE_GAP, bounds.width - width - EDGE_GAP)),
    y: Math.min(Math.max(insets.top + EDGE_GAP, rect.y), Math.max(insets.top + EDGE_GAP, bounds.height - insets.bottom - height - EDGE_GAP))
  };
}

/**
 * Snap a dragged rect to window edges within SNAP_DISTANCE. Returns the
 * snapped rect and which edges it now hugs, so it stays attached when the
 * window is resized.
 */
export function snapRect(rect: PanelRect, bounds: Bounds, insets: { top: number; bottom: number }): PanelPlacement {
  const clamped = clampRect(rect, bounds, insets);
  const left = EDGE_GAP;
  const right = bounds.width - clamped.width - EDGE_GAP;
  const top = insets.top + EDGE_GAP;
  const bottom = bounds.height - insets.bottom - clamped.height - EDGE_GAP;
  const dock: PanelDock = { horizontal: null, vertical: null };
  let { x, y } = clamped;
  if (Math.abs(x - left) <= SNAP_DISTANCE) {
    x = left;
    dock.horizontal = "left";
  } else if (Math.abs(x - right) <= SNAP_DISTANCE) {
    x = right;
    dock.horizontal = "right";
  }
  if (Math.abs(y - top) <= SNAP_DISTANCE) {
    y = top;
    dock.vertical = "top";
  } else if (Math.abs(y - bottom) <= SNAP_DISTANCE) {
    y = bottom;
    dock.vertical = "bottom";
  }
  return { rect: { ...clamped, x, y }, dock };
}

/**
 * A horizontal band near the bottom edge that panels must not cover (the
 * centred floating toolbar). Panels whose span overlaps it are lifted above
 * it; panels in the corners stay flush with the bottom edge.
 */
export interface BottomObstacle {
  left: number;
  right: number;
  height: number;
}

export function avoidObstacle(rect: PanelRect, bounds: Bounds, insets: { top: number; bottom: number }, obstacle: BottomObstacle | null): PanelRect {
  if (!obstacle) return rect;
  const overlapsHorizontally = rect.x < obstacle.right && rect.x + rect.width > obstacle.left;
  const limit = bounds.height - insets.bottom - obstacle.height;
  if (!overlapsHorizontally || rect.y + rect.height <= limit) return rect;
  return { ...rect, y: Math.max(insets.top, limit - rect.height) };
}

/** Re-apply a docked placement after the window resized. */
export function relayout(placement: PanelPlacement, bounds: Bounds, insets: { top: number; bottom: number }): PanelRect {
  const rect = clampRect(placement.rect, bounds, insets);
  if (placement.dock.horizontal === "left") rect.x = EDGE_GAP;
  if (placement.dock.horizontal === "right") rect.x = bounds.width - rect.width - EDGE_GAP;
  if (placement.dock.vertical === "top") rect.y = insets.top + EDGE_GAP;
  if (placement.dock.vertical === "bottom") rect.y = bounds.height - insets.bottom - rect.height - EDGE_GAP;
  return clampRect(rect, bounds, insets);
}

/** Default: snapped to a corner (top-right below the titlebar, or bottom-left). */
export function defaultPlacement(
  size: { width: number; height: number },
  corner: "top-right" | "bottom-left",
  bounds: Bounds,
  insets: { top: number; bottom: number }
): PanelPlacement {
  const x = corner === "top-right" ? bounds.width : 0;
  const y = corner === "top-right" ? 0 : bounds.height;
  return snapRect({ x, y, width: size.width, height: size.height }, bounds, insets);
}

export function parsePlacement(value: unknown): PanelPlacement | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as { rect?: Partial<PanelRect>; dock?: Partial<PanelDock> };
  const rect = raw.rect;
  if (!rect || ![rect.x, rect.y, rect.width, rect.height].every((n) => typeof n === "number" && Number.isFinite(n))) return null;
  const horizontal = raw.dock?.horizontal === "left" || raw.dock?.horizontal === "right" ? raw.dock.horizontal : null;
  const vertical = raw.dock?.vertical === "top" || raw.dock?.vertical === "bottom" ? raw.dock.vertical : null;
  return { rect: rect as PanelRect, dock: { horizontal, vertical } };
}
