import { useEffect, useRef, useState } from "react";
import type { CSSProperties, SyntheticEvent } from "react";
import { flushSync } from "react-dom";
import type { MediaRecord, ThumbnailResponse } from "@megle/core-client";
import {
  mediaContentSignature,
  mediaFileContentSignature,
  preloadImageObjectUrl,
  previewPlaceholderDataUrl,
  readCachedThumbnailObjectUrl,
  requestOriginalPreviewBlob,
  requestThumbnailBlob,
  thumbnailObjectUrlCacheKey
} from "../../core/mediaResources";
import { resolveVideoPreviewNaturalSize } from "./videoPreviewSizing";

const STALE_PREVIEW_OBJECT_URL_REVOKE_DELAY_MS = 8000;

type PreviewNaturalSize = {
  naturalHeight: number;
  naturalWidth: number;
};

type PendingBufferedPreview = {
  frameStyle?: CSSProperties;
  naturalSize: PreviewNaturalSize | null;
  src: string;
};

export function MediaPreview({
  media,
  onLoadingChange,
  onMediaReady,
  onNaturalSize,
  onVideoElementChange,
  preferOriginalWhilePending = false,
  preserveNaturalFrame,
  source = "thumbnail",
  stableFrame = false,
  thumbnail
}: {
  media: MediaRecord;
  onLoadingChange?: (loading: boolean) => void;
  onMediaReady?: () => void;
  onNaturalSize?: (size: { naturalHeight: number; naturalWidth: number }) => void;
  onVideoElementChange?: (element: HTMLVideoElement | null) => void;
  preferOriginalWhilePending?: boolean;
  preserveNaturalFrame?: boolean;
  source?: "thumbnail" | "original";
  stableFrame?: boolean;
  thumbnail?: ThumbnailResponse;
}) {
  const previewPlaceholderUrl = previewPlaceholderDataUrl(media);
  const stableFrameStyle = stableFrame ? previewStableFrameStyle(media) : undefined;
  const shouldPreserveNaturalFrame =
    preserveNaturalFrame ?? (source === "original" && stableFrame);
  const originalVersionKey = mediaFileContentSignature(media);
  const hasLiveReadyThumbnail = thumbnail?.state === "ready" && thumbnail.updatedAt !== null;
  const shouldUseOriginalWhilePending =
    source === "thumbnail" &&
    preferOriginalWhilePending &&
    media.kind === "image" &&
    thumbnail?.state !== "failed" &&
    thumbnail?.state !== "skipped_small" &&
    !hasLiveReadyThumbnail;
  const fallbackThumbnail = useThumbnailFallbackUrl(
    source === "original" && hasLiveReadyThumbnail ? media.id : null,
    mediaContentSignature(media),
    hasLiveReadyThumbnail ? thumbnail.updatedAt : null
  );
  const fallbackUrl = fallbackThumbnail ?? previewPlaceholderUrl;

  if (source === "original" || shouldUseOriginalWhilePending) {
    return (
      <ReadyPreviewMedia
        alt={media.name}
        fallbackUrl={fallbackUrl}
        frameStyle={stableFrameStyle}
        kind={media.kind}
        media={media}
        onLoadingChange={onLoadingChange}
        onMediaReady={onMediaReady}
        onNaturalSize={onNaturalSize}
        onVideoElementChange={onVideoElementChange}
        preserveNaturalFrame={shouldPreserveNaturalFrame}
        source="original"
        versionKey={originalVersionKey}
      />
    );
  }

  if (hasLiveReadyThumbnail) {
    return (
      <ReadyPreviewMedia
        alt={media.name}
        fallbackUrl={previewPlaceholderUrl}
        kind={media.kind}
        media={media}
        onLoadingChange={onLoadingChange}
        onMediaReady={onMediaReady}
        preserveNaturalFrame={false}
        source="thumbnail"
        versionKey={hasLiveReadyThumbnail ? thumbnail.updatedAt : null}
      />
    );
  }

  if (thumbnail?.state === "failed") {
    return (
      <div className="preview-placeholder failed" style={stableFrameStyle}>
        <span>thumbnail failed</span>
      </div>
    );
  }

  if (thumbnail?.state === "skipped_small") {
    if (previewPlaceholderUrl) {
      return (
        <PreviewFallbackImage
          alt={media.name}
          frameStyle={stableFrameStyle}
          src={previewPlaceholderUrl}
          state="skipped"
        />
      );
    }
    return (
      <div className="preview-placeholder skipped" style={stableFrameStyle}>
        <span>{media.kind ?? "file"}</span>
      </div>
    );
  }

  if (previewPlaceholderUrl) {
    return (
      <PreviewFallbackImage
        alt={media.name}
        frameStyle={stableFrameStyle}
        src={previewPlaceholderUrl}
        state="pending"
      />
    );
  }

  return <div className="preview-placeholder pending preview-placeholder-empty" style={stableFrameStyle} />;
}

function schedulePreviewObjectUrlRevoke(objectUrl: string): void {
  window.setTimeout(() => URL.revokeObjectURL(objectUrl), STALE_PREVIEW_OBJECT_URL_REVOKE_DELAY_MS);
}

function useThumbnailFallbackUrl(
  fileId: number | null,
  mediaSignature: string,
  versionKey: number | null
): string | null {
  const cacheKey =
    fileId === null ? null : thumbnailObjectUrlCacheKey(fileId, mediaSignature, versionKey);
  const [fallbackThumbnail, setFallbackThumbnail] = useState<string | null>(() =>
    cacheKey ? readCachedThumbnailObjectUrl(cacheKey) : null
  );

  useEffect(() => {
    if (fileId === null || cacheKey === null) {
      setFallbackThumbnail(null);
      return undefined;
    }

    const cachedObjectUrl = readCachedThumbnailObjectUrl(cacheKey);
    if (cachedObjectUrl) {
      setFallbackThumbnail(cachedObjectUrl);
      return undefined;
    }

    let revoked = false;
    let objectUrl: string | null = null;
    const controller = new AbortController();
    setFallbackThumbnail(null);

    requestThumbnailBlob(fileId, versionKey, {
      requestPriority: "interactive",
      resourcePriority: "preview",
      signal: controller.signal
    })
      .then((blob) => {
        if (revoked) return;
        objectUrl = URL.createObjectURL(blob);
        setFallbackThumbnail(objectUrl);
      })
      .catch(() => {
        if (!revoked) setFallbackThumbnail(null);
      });

    return () => {
      revoked = true;
      controller.abort();
      if (objectUrl) schedulePreviewObjectUrlRevoke(objectUrl);
    };
  }, [cacheKey, fileId, versionKey]);

  return fallbackThumbnail;
}

function PreviewFallbackImage({
  alt,
  frameStyle,
  src,
  state
}: {
  alt: string;
  frameStyle?: CSSProperties;
  src: string;
  state: "pending" | "skipped";
}) {
  return (
    <div
      className={`preview-placeholder ${state} preview-placeholder-image`}
      data-preview-placeholder="preview"
      style={frameStyle}
    >
      <img alt={alt} className="preview-image preview-fallback-image" src={src} />
    </div>
  );
}

function ReadyPreviewMedia({
  alt,
  fallbackUrl,
  frameStyle,
  kind,
  media,
  onLoadingChange,
  onMediaReady,
  onNaturalSize,
  onVideoElementChange,
  preserveNaturalFrame,
  source,
  versionKey
}: {
  alt: string;
  fallbackUrl: string | null;
  frameStyle?: CSSProperties;
  kind?: string | null;
  media: MediaRecord;
  onLoadingChange?: (loading: boolean) => void;
  onMediaReady?: () => void;
  onNaturalSize?: (size: { naturalHeight: number; naturalWidth: number }) => void;
  onVideoElementChange?: (element: HTMLVideoElement | null) => void;
  preserveNaturalFrame: boolean;
  source: "thumbnail" | "original";
  versionKey?: number | string | null;
}) {
  const [src, setSrc] = useState<string | null>(null);
  const [pendingPreview, setPendingPreview] = useState<PendingBufferedPreview | null>(null);
  const [error, setError] = useState(false);
  const [containedMediaStyle, setContainedMediaStyle] = useState<CSSProperties | undefined>(undefined);
  const [naturalFrameStyle, setNaturalFrameStyle] = useState<CSSProperties | undefined>(undefined);
  const objectUrlRef = useRef<string | null>(null);
  const pendingObjectUrlRef = useRef<string | null>(null);
  const shouldUseBufferedSwap = source === "original" && preserveNaturalFrame && kind !== "video";

  useEffect(() => {
    return () => {
      if (pendingObjectUrlRef.current) {
        schedulePreviewObjectUrlRevoke(pendingObjectUrlRef.current);
        pendingObjectUrlRef.current = null;
      }
      if (objectUrlRef.current) {
        schedulePreviewObjectUrlRevoke(objectUrlRef.current);
        objectUrlRef.current = null;
      }
    };
  }, []);

  const commitBufferedPreview = (preview: PendingBufferedPreview, size: PreviewNaturalSize) => {
    if (pendingObjectUrlRef.current !== preview.src) {
      return;
    }
    const previousObjectUrl = objectUrlRef.current;
    flushSync(() => {
      objectUrlRef.current = preview.src;
      pendingObjectUrlRef.current = null;
      setContainedMediaStyle(undefined);
      setNaturalFrameStyle(naturalFrameStyleForSize(size));
      setSrc(preview.src);
      setPendingPreview(null);
      setError(false);
      onNaturalSize?.(size);
      onLoadingChange?.(false);
      onMediaReady?.();
    });
    if (previousObjectUrl && previousObjectUrl !== preview.src) {
      schedulePreviewObjectUrlRevoke(previousObjectUrl);
    }
  };

  useEffect(() => {
    let revoked = false;
    let objectUrl: string | null = null;
    onLoadingChange?.(true);
    setError(false);
    if (shouldUseBufferedSwap) {
      setPendingPreview(null);
    }
    const initialNaturalSize =
      source === "original" &&
      preserveNaturalFrame &&
      kind !== "video" &&
      media.width &&
      media.height &&
      media.width > 0 &&
      media.height > 0
        ? { naturalHeight: media.height, naturalWidth: media.width }
        : null;

    const controller = new AbortController();
    const request = source === "original"
      ? requestOriginalPreviewBlob(media, {
          requestPriority: "interactive",
          resourcePriority: "preview",
          signal: controller.signal
        })
      : requestThumbnailBlob(media.id, typeof versionKey === "number" ? versionKey : null, {
          requestPriority: "interactive",
          resourcePriority: "preview",
          signal: controller.signal
        });
    request
      .then((blob) => {
        objectUrl = URL.createObjectURL(blob);
        if (revoked) {
          URL.revokeObjectURL(objectUrl);
          return;
        }
        const nextObjectUrl = objectUrl;

        if (shouldUseBufferedSwap) {
          pendingObjectUrlRef.current = nextObjectUrl;
          setPendingPreview({
            frameStyle: initialNaturalSize ? naturalFrameStyleForSize(initialNaturalSize) : undefined,
            naturalSize: initialNaturalSize,
            src: nextObjectUrl
          });
          return;
        }

        if (initialNaturalSize) {
          onNaturalSize?.(initialNaturalSize);
          setContainedMediaStyle(undefined);
          setNaturalFrameStyle({
            height: `${initialNaturalSize.naturalHeight}px`,
            width: `${initialNaturalSize.naturalWidth}px`
          });
        } else if (source !== "original" || !preserveNaturalFrame) {
          setNaturalFrameStyle(undefined);
          setContainedMediaStyle(undefined);
        }
        const previousObjectUrl = objectUrlRef.current;
        objectUrlRef.current = nextObjectUrl;
        if (source === "original" && kind === "video") {
          setNaturalFrameStyle(undefined);
          setContainedMediaStyle(undefined);
        }
        setSrc(nextObjectUrl);
        if (!(source === "original" && kind === "video")) {
          onLoadingChange?.(false);
        }
        if (previousObjectUrl) {
          schedulePreviewObjectUrlRevoke(previousObjectUrl);
        }
        if (source === "original" && kind === "video") {
          return;
        }
        void preloadImageObjectUrl(nextObjectUrl)
          .then((naturalSize) => {
            if (revoked || objectUrlRef.current !== nextObjectUrl) {
              return;
            }
            if (source === "original" && preserveNaturalFrame) {
              onNaturalSize?.(naturalSize);
              setContainedMediaStyle(undefined);
              setNaturalFrameStyle({
                height: `${naturalSize.naturalHeight}px`,
                width: `${naturalSize.naturalWidth}px`
              });
            } else if (!preserveNaturalFrame) {
              setNaturalFrameStyle(undefined);
              setContainedMediaStyle(containedPreviewMediaStyle(naturalSize));
            }
          })
          .catch(() => undefined);
      })
      .catch(() => {
        if (objectUrl && objectUrlRef.current !== objectUrl) {
          schedulePreviewObjectUrlRevoke(objectUrl);
          objectUrl = null;
        }
        if (!revoked && !controller.signal.aborted) {
          setError(true);
          onLoadingChange?.(false);
          onMediaReady?.();
        }
      });

    return () => {
      revoked = true;
      controller.abort();
      if (objectUrl && objectUrlRef.current !== objectUrl) {
        if (pendingObjectUrlRef.current === objectUrl) {
          pendingObjectUrlRef.current = null;
        }
        schedulePreviewObjectUrlRevoke(objectUrl);
      }
    };
  }, [
    kind,
    media.height,
    media.id,
    media.width,
    onLoadingChange,
    onNaturalSize,
    preserveNaturalFrame,
    shouldUseBufferedSwap,
    source,
    versionKey
  ]);

  const effectiveFrameStyle = shouldUseBufferedSwap
    ? naturalFrameStyle ?? frameStyle
    : frameStyle ?? naturalFrameStyle;
  const renderedFrameStyle =
    source === "original" && kind === "video"
      ? naturalFrameStyle ?? effectiveFrameStyle
      : effectiveFrameStyle;
  const pendingImage = pendingPreview ? (
    <img
      alt=""
      aria-hidden="true"
      className="preview-image preview-image-pending"
      decoding="async"
      onError={() => {
        if (pendingObjectUrlRef.current !== pendingPreview.src) {
          return;
        }
        pendingObjectUrlRef.current = null;
        setPendingPreview(null);
        setError(true);
        onLoadingChange?.(false);
        onMediaReady?.();
        schedulePreviewObjectUrlRevoke(pendingPreview.src);
      }}
      onLoad={(event) => {
        const image = event.currentTarget;
        const naturalSize =
          image.naturalWidth > 0 && image.naturalHeight > 0
            ? {
                naturalHeight: image.naturalHeight,
                naturalWidth: image.naturalWidth
              }
            : pendingPreview.naturalSize;
        if (naturalSize) {
          decodePendingPreviewImage(image)
            .then(() => {
              commitBufferedPreview(pendingPreview, naturalSize);
            })
            .catch(() => {
              commitBufferedPreview(pendingPreview, naturalSize);
            });
        }
      }}
      src={pendingPreview.src}
      style={pendingPreview.frameStyle}
    />
  ) : null;

  if (error && !src) {
    return (
      <>
        <div className="preview-placeholder failed" style={renderedFrameStyle}>
          <span>load error</span>
        </div>
        {pendingImage}
      </>
    );
  }

  if (!src) {
    if (fallbackUrl) {
      return (
        <>
          <PreviewFallbackImage alt={alt} frameStyle={renderedFrameStyle} src={fallbackUrl} state="pending" />
          {pendingImage}
        </>
      );
    }
    return (
      <>
        <div className="preview-placeholder pending preview-placeholder-empty" style={renderedFrameStyle} />
        {pendingImage}
      </>
    );
  }

  if (source === "original" && kind === "video") {
    const handleVideoMetadata = (event: SyntheticEvent<HTMLVideoElement>) => {
      const posterSize =
        media.width && media.height && media.width > 0 && media.height > 0
          ? { naturalHeight: media.height, naturalWidth: media.width }
          : null;
      const naturalSize = resolveVideoPreviewNaturalSize(event.currentTarget, posterSize);
      if (naturalSize) {
        onNaturalSize?.(naturalSize);
        setContainedMediaStyle(undefined);
        setNaturalFrameStyle(naturalFrameStyleForSize(naturalSize));
      }
      onLoadingChange?.(false);
      onMediaReady?.();
    };

    return (
      <div className="preview-placeholder ready" data-preview-source={source} style={renderedFrameStyle}>
        <video
          aria-label={alt}
          className="preview-image preview-video"
          data-preview-media-id={media.id}
          key={src}
          onError={() => {
            setError(true);
            onLoadingChange?.(false);
            onMediaReady?.();
          }}
          onLoadedMetadata={handleVideoMetadata}
          playsInline
          preload="auto"
          ref={onVideoElementChange}
          src={src}
        />
      </div>
    );
  }

  return (
    <>
      <div className="preview-placeholder ready" data-preview-source={source} style={renderedFrameStyle}>
        <img
          alt={alt}
          className="preview-image"
          decoding="async"
          onLoad={(event) => {
            const image = event.currentTarget;
            const naturalSize = {
              naturalHeight: image.naturalHeight,
              naturalWidth: image.naturalWidth
            };
            if (source === "original" && preserveNaturalFrame) {
              onNaturalSize?.(naturalSize);
              setContainedMediaStyle(undefined);
              setNaturalFrameStyle({
                height: `${naturalSize.naturalHeight}px`,
                width: `${naturalSize.naturalWidth}px`
              });
            } else if (!preserveNaturalFrame) {
              setNaturalFrameStyle(undefined);
              setContainedMediaStyle(containedPreviewMediaStyle(naturalSize));
            }
            onMediaReady?.();
          }}
          onError={() => {
            setError(true);
            onLoadingChange?.(false);
            onMediaReady?.();
          }}
          src={src}
          style={containedMediaStyle}
        />
      </div>
      {pendingImage}
    </>
  );
}

function containedPreviewMediaStyle(size: {
  naturalHeight: number;
  naturalWidth: number;
}): CSSProperties {
  return {
    height: "auto",
    maxHeight: "100cqh",
    maxWidth: "100cqw",
    width: "auto"
  };
}

function naturalFrameStyleForSize(size: PreviewNaturalSize): CSSProperties {
  return {
    height: `${size.naturalHeight}px`,
    width: `${size.naturalWidth}px`
  };
}

function decodePendingPreviewImage(image: HTMLImageElement): Promise<void> {
  if (typeof image.decode !== "function") {
    return Promise.resolve();
  }
  return image.decode().catch(() => undefined);
}

function previewStableFrameStyle(media: MediaRecord): CSSProperties | undefined {
  if (!media.width || !media.height || media.width <= 0 || media.height <= 0) {
    return undefined;
  }
  return naturalFrameStyleForSize({
    naturalHeight: media.height,
    naturalWidth: media.width
  });
}
