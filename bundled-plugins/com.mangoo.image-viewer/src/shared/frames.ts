/**
 * Frame timeline model for animated images (GIF, APNG, animated WebP/AVIF).
 * Holds per-frame durations and turns a playback clock into a frame index,
 * honouring speed and loop count. Pure: the decoder lives in the web layer.
 */

/** Browsers render 0/10ms GIF delays as ~100ms; match that so playback looks right. */
export const MIN_FRAME_MS = 20;
export const FALLBACK_FRAME_MS = 100;

export function normalizeFrameDuration(ms: number | null | undefined): number {
  if (!Number.isFinite(ms ?? NaN) || (ms as number) <= 10) return FALLBACK_FRAME_MS;
  return Math.max(MIN_FRAME_MS, ms as number);
}

export interface FrameTimeline {
  durations: number[];
  /** Cumulative start time of each frame (ms). */
  starts: number[];
  total: number;
}

export function buildTimeline(durations: readonly number[]): FrameTimeline {
  const normalized = durations.map(normalizeFrameDuration);
  const starts: number[] = [];
  let total = 0;
  for (const duration of normalized) {
    starts.push(total);
    total += duration;
  }
  return { durations: normalized, starts, total };
}

/** Frame shown at `elapsed` ms of looping playback. */
export function frameAt(timeline: FrameTimeline, elapsed: number): number {
  if (timeline.durations.length === 0) return 0;
  const time = ((elapsed % timeline.total) + timeline.total) % timeline.total;
  let low = 0;
  let high = timeline.starts.length - 1;
  while (low < high) {
    const middle = (low + high + 1) >> 1;
    if (timeline.starts[middle] <= time) low = middle;
    else high = middle - 1;
  }
  return low;
}

export function stepFrame(count: number, index: number, delta: number): number {
  if (count <= 0) return 0;
  return (((index + delta) % count) + count) % count;
}

export const PLAYBACK_SPEEDS = [0.25, 0.5, 1, 1.5, 2, 4] as const;

export function formatFrameTime(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;
}
