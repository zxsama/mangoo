export const VIDEO_DRAG_DISTANCE_PX = 5;

export type VideoPointerGesture = "tap" | "drag";

export function pointerDistance(startX: number, startY: number, x: number, y: number): number {
  return Math.hypot(x - startX, y - startY);
}

export function pointerDisplacement(startX: number, startY: number, x: number, y: number) {
  return { x: x - startX, y: y - startY };
}

export function classifyVideoPointerGesture({
  distancePx
}: {
  distancePx: number;
}): VideoPointerGesture {
  return distancePx > VIDEO_DRAG_DISTANCE_PX ? "drag" : "tap";
}
