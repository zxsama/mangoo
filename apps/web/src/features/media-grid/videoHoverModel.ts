export const VIDEO_HOVER_JITTER_PX = 2;
export const VIDEO_HOVER_RESUME_MS = 120;

export function horizontalHoverRatio(previousX: number, clientX: number, left: number, width: number): number | null {
  if (![previousX, clientX, left, width].every(Number.isFinite) || width <= 0 ||
      Math.abs(clientX - previousX) <= VIDEO_HOVER_JITTER_PX) return null;
  return Math.max(0, Math.min(1, (clientX - left) / width));
}

export function hoverSeekTime(ratio: number, duration: number): number | null {
  if (!Number.isFinite(ratio) || !Number.isFinite(duration) || duration <= 0) return null;
  // Leave a frame at the end so a resumed, looping preview can make progress.
  return Math.min(Math.max(0, duration - 0.04), Math.max(0, Math.min(1, ratio)) * duration);
}

// Single owner across virtualized grids in one renderer; acquiring also cancels
// pending grant creation, so rapid crossings cannot activate an older tile later.
let owner: { stop: () => void } | null = null;
export function acquireVideoHover(stop: () => void): () => void {
  owner?.stop();
  const lease = { stop };
  owner = lease;
  return () => { if (owner === lease) owner = null; };
}
export function stopVideoHover(): void { owner?.stop(); }
