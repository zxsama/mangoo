export interface VideoMetadataSize {
  videoHeight: number;
  videoWidth: number;
}

export function videoPreviewNaturalSize(
  video: VideoMetadataSize
): { naturalHeight: number; naturalWidth: number } | null {
  if (
    !Number.isFinite(video.videoWidth) ||
    !Number.isFinite(video.videoHeight) ||
    video.videoWidth <= 0 ||
    video.videoHeight <= 0
  ) {
    return null;
  }
  return {
    naturalHeight: Math.trunc(video.videoHeight),
    naturalWidth: Math.trunc(video.videoWidth)
  };
}
