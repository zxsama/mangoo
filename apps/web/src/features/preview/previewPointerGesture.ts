export const VIDEO_DRAG_HOLD_MS = 200;
export const VIDEO_DRAG_DISTANCE_PX = 5;

export type VideoPointerGesture = "tap" | "drag" | "hold" | "slip";

export function pointerDistance(startX: number, startY: number, x: number, y: number): number {
  return Math.hypot(x - startX, y - startY);
}

export function shouldStartVideoDrag(elapsedMs: number, distancePx: number): boolean {
  return elapsedMs >= VIDEO_DRAG_HOLD_MS && distancePx > VIDEO_DRAG_DISTANCE_PX;
}

export function classifyVideoPointerGesture({
  dragging,
  elapsedMs,
  distancePx
}: {
  dragging: boolean;
  elapsedMs: number;
  distancePx: number;
}): VideoPointerGesture {
  if (dragging) return "drag";
  if (elapsedMs >= VIDEO_DRAG_HOLD_MS) return "hold";
  if (distancePx > VIDEO_DRAG_DISTANCE_PX) return "slip";
  return "tap";
}
