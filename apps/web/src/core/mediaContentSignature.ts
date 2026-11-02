import type { MediaRecord } from "@mangoo/core-client";

export function normalizeMediaThumbnailState(
  value: string | null | undefined
): "pending" | "queued" | "ready" | "failed" | "skipped_small" | "unavailable" {
  if (
    value === "pending" ||
    value === "queued" ||
    value === "ready" ||
    value === "failed" ||
    value === "skipped_small" ||
    value === "unavailable"
  ) {
    return value;
  }
  return "pending";
}

export function mediaContentSignature(mediaRecord: MediaRecord): string {
  return [
    mediaRecord.id,
    mediaRecord.mtime,
    mediaRecord.size,
    normalizeMediaThumbnailState(mediaRecord.thumbnailState),
    mediaRecord.presentation?.generation ?? "legacy"
  ].join(":");
}
