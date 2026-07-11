export interface VideoPlaybackSnapshot {
  autoplayBlocked: boolean;
  currentTime: number;
  duration: number;
  muted: boolean;
  playing: boolean;
  volume: number;
}

export type VideoAutoplayResult = "playing" | "playing-muted" | "blocked" | "failed";

interface AutoplayVideo {
  muted: boolean;
  play: () => Promise<void>;
}

export const EMPTY_VIDEO_PLAYBACK_SNAPSHOT: VideoPlaybackSnapshot = {
  autoplayBlocked: false,
  currentTime: 0,
  duration: 0,
  muted: false,
  playing: false,
  volume: 1
};

export function formatMediaTime(value: number): string {
  const seconds = Number.isFinite(value) ? Math.max(0, Math.floor(value)) : 0;
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remainder = seconds % 60;
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, "0")}:${String(remainder).padStart(2, "0")}`;
  }
  return `${minutes}:${String(remainder).padStart(2, "0")}`;
}

export function clampSeekTime(value: number, duration: number): number {
  if (!Number.isFinite(value)) return 0;
  if (!Number.isFinite(duration) || duration <= 0) return Math.max(0, value);
  return Math.min(duration, Math.max(0, value));
}

export function playbackProgress(currentTime: number, duration: number): number {
  if (!Number.isFinite(currentTime) || !Number.isFinite(duration) || duration <= 0) return 0;
  return Math.min(1, Math.max(0, currentTime / duration));
}

export function normalizeVolume(value: number): number {
  if (!Number.isFinite(value)) return 1;
  return Math.min(1, Math.max(0, value));
}

export function isAutoplayPolicyError(error: unknown): boolean {
  return error instanceof DOMException
    ? error.name === "NotAllowedError"
    : typeof error === "object" && error !== null && "name" in error && error.name === "NotAllowedError";
}

export async function attemptVideoAutoplay(video: AutoplayVideo): Promise<VideoAutoplayResult> {
  video.muted = false;
  try {
    await video.play();
    return "playing";
  } catch (error) {
    if (!isAutoplayPolicyError(error)) return "failed";
  }

  video.muted = true;
  try {
    await video.play();
    return "playing-muted";
  } catch (error) {
    return isAutoplayPolicyError(error) ? "blocked" : "failed";
  }
}
