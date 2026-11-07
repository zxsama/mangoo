/**
 * Pure zoom / pan / rotate math for the viewer canvas. The image is drawn
 * centred in the viewport, then translated by `pan` (CSS px) and scaled;
 * rotation and flips are applied around the image centre. Everything here is
 * deterministic and unit tested.
 */

export type FitMode = "fit" | "fill" | "width" | "height" | "actual";
export type Rotation = 0 | 90 | 180 | 270;

export interface ViewTransform {
  scale: number;
  panX: number;
  panY: number;
  rotation: Rotation;
  flipX: boolean;
  flipY: boolean;
}

export interface Size {
  width: number;
  height: number;
}

export const MIN_SCALE = 0.02;
export const MAX_SCALE = 32;

/** Common viewer zoom stops (as fractions of actual size). */
export const ZOOM_STOPS = [
  0.02, 0.05, 0.1, 0.125, 0.167, 0.25, 0.333, 0.5, 0.667, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8, 12, 16, 24, 32
];

export function identityTransform(): ViewTransform {
  return { scale: 1, panX: 0, panY: 0, rotation: 0, flipX: false, flipY: false };
}

export function clampScale(scale: number): number {
  if (!Number.isFinite(scale)) return 1;
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}

/** Image box after rotation (90/270 swap the axes). */
export function rotatedSize(image: Size, rotation: Rotation): Size {
  return rotation === 90 || rotation === 270
    ? { width: image.height, height: image.width }
    : { width: image.width, height: image.height };
}

/**
 * Scale for a fit mode. `fit` never enlarges a picture smaller than the
 * viewport unless `upscaleSmall` is set — the default of most viewers.
 */
export function fitScale(
  image: Size,
  viewport: Size,
  rotation: Rotation,
  mode: FitMode,
  upscaleSmall = false
): number {
  const box = rotatedSize(image, rotation);
  if (box.width <= 0 || box.height <= 0 || viewport.width <= 0 || viewport.height <= 0) return 1;
  const horizontal = viewport.width / box.width;
  const vertical = viewport.height / box.height;
  let scale: number;
  switch (mode) {
    case "actual":
      return 1;
    case "fill":
      scale = Math.max(horizontal, vertical);
      break;
    case "width":
      scale = horizontal;
      break;
    case "height":
      scale = vertical;
      break;
    case "fit":
    default:
      scale = Math.min(horizontal, vertical);
      if (!upscaleSmall) scale = Math.min(scale, 1);
  }
  return clampScale(scale);
}

/**
 * Keep the picture reachable: when an axis is larger than the viewport the
 * pan may not reveal empty space beyond its edge; when smaller it stays centred.
 */
export function clampPan(transform: ViewTransform, image: Size, viewport: Size): ViewTransform {
  const box = rotatedSize(image, transform.rotation);
  const halfOverflowX = Math.max(0, (box.width * transform.scale - viewport.width) / 2);
  const halfOverflowY = Math.max(0, (box.height * transform.scale - viewport.height) / 2);
  return {
    ...transform,
    panX: Math.min(halfOverflowX, Math.max(-halfOverflowX, transform.panX)),
    panY: Math.min(halfOverflowY, Math.max(-halfOverflowY, transform.panY))
  };
}

/**
 * Zoom to `nextScale` keeping the image point under `anchor` (viewport CSS
 * px, origin top-left) fixed on screen — zoom-at-cursor.
 */
export function zoomAt(
  transform: ViewTransform,
  nextScale: number,
  anchor: { x: number; y: number },
  image: Size,
  viewport: Size
): ViewTransform {
  const scale = clampScale(nextScale);
  const ratio = scale / transform.scale;
  const offsetX = anchor.x - viewport.width / 2;
  const offsetY = anchor.y - viewport.height / 2;
  const next = {
    ...transform,
    scale,
    panX: offsetX - (offsetX - transform.panX) * ratio,
    panY: offsetY - (offsetY - transform.panY) * ratio
  };
  return clampPan(next, image, viewport);
}

export function nextZoomStop(scale: number, direction: 1 | -1): number {
  const epsilon = 1e-3;
  if (direction > 0) {
    return ZOOM_STOPS.find((stop) => stop > scale * (1 + epsilon)) ?? MAX_SCALE;
  }
  for (let index = ZOOM_STOPS.length - 1; index >= 0; index -= 1) {
    if (ZOOM_STOPS[index] < scale * (1 - epsilon)) return ZOOM_STOPS[index];
  }
  return MIN_SCALE;
}

/** Wheel notch -> multiplicative zoom factor (smooth for trackpads). */
export function wheelZoomFactor(deltaY: number, deltaMode: number): number {
  const pixels = deltaMode === 1 ? deltaY * 16 : deltaMode === 2 ? deltaY * 400 : deltaY;
  const clamped = Math.max(-240, Math.min(240, pixels));
  return Math.exp(-clamped * 0.0018);
}

export function rotate(transform: ViewTransform, quarterTurns: 1 | -1): ViewTransform {
  const rotation = ((((transform.rotation + quarterTurns * 90) % 360) + 360) % 360) as Rotation;
  return { ...transform, rotation, panX: 0, panY: 0 };
}

/** CSS transform for the image element centred at the viewport origin. */
export function cssTransform(transform: ViewTransform): string {
  const flip = `scale(${transform.flipX ? -1 : 1}, ${transform.flipY ? -1 : 1})`;
  return `translate(${round(transform.panX)}px, ${round(transform.panY)}px) rotate(${transform.rotation}deg) scale(${transform.scale}) ${flip}`;
}

export function formatZoom(scale: number): string {
  const percent = scale * 100;
  return `${percent >= 10 ? Math.round(percent) : percent.toFixed(1)}%`;
}

function round(value: number): number {
  return Math.round(value * 100) / 100;
}

/**
 * Canvas render level for a bitmap shown at `fraction` of its own resolution
 * (on-screen device px / bitmap px). Chromium minifies a canvas through a CSS
 * transform without mipmaps, so a 10k px picture at 8% aliases badly; painting
 * the backing store at the nearest power-of-two level >= the need with
 * high-quality smoothing keeps it clean, and only repaints on level changes.
 */
export function renderLevel(fraction: number): number {
  if (!Number.isFinite(fraction) || fraction >= 0.75) return 1;
  const level = 2 ** Math.ceil(Math.log2(Math.max(fraction, 1 / 64) * 1.15));
  return Math.min(1, Math.max(1 / 64, level));
}
