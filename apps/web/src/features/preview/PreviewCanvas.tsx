import { useEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";

import type { MediaRecord } from "@megle/core-client";

import type { PoolableBitmap } from "./previewBitmapPool";
import {
  pinPreviewFrame,
  readAnyPooledPreviewFrame,
  requestPreviewBitmap
} from "./previewBitmapSource";
import { canvasPaintTarget } from "./previewRenderLane";

// Paints the preview from a PRE-DECODED ImageBitmap.
//
// The canvas element is deliberately stable across navigation — no `key` — so
// React never remounts it. That is the entire fix for the slow prev/next: the
// old path mounted a fresh `<img key={src}>` per frame, and a decoded bitmap
// does not transfer to a new element even with the same object URL, so every
// step paid a full-resolution main-thread decode (measured 337ms at 16384px).

export function PreviewCanvas({
  alt,
  frameStyle,
  media,
  onLoadingChange,
  onMediaReady,
  onNaturalSize,
  viewportLongPx
}: {
  alt: string;
  frameStyle?: CSSProperties;
  media: MediaRecord;
  onLoadingChange?: (loading: boolean) => void;
  onMediaReady?: () => void;
  onNaturalSize?: (size: { naturalHeight: number; naturalWidth: number }) => void;
  viewportLongPx: number;
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [boxStyle, setBoxStyle] = useState<CSSProperties | undefined>(undefined);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let disposed = false;
    const controller = new AbortController();
    setFailed(false);

    const natural =
      media.width && media.height && media.width > 0 && media.height > 0
        ? { naturalHeight: media.height, naturalWidth: media.width }
        : null;

    const paint = (bitmap: PoolableBitmap, key: string) => {
      const canvas = canvasRef.current;
      if (!canvas || disposed) {
        return;
      }
      const target = canvasPaintTarget(bitmap, natural);
      if (canvas.width !== target.backingWidth || canvas.height !== target.backingHeight) {
        // Assigning width/height also clears the canvas.
        canvas.width = target.backingWidth;
        canvas.height = target.backingHeight;
      }
      const context = canvas.getContext("2d");
      if (!context) {
        setFailed(true);
        return;
      }
      // Explicit clear: when the dimensions happen to match, a transparent PNG
      // would otherwise composite over the previous frame.
      context.clearRect(0, 0, canvas.width, canvas.height);
      context.drawImage(bitmap as unknown as CanvasImageSource, 0, 0);
      setBoxStyle({ height: `${target.cssHeight}px`, width: `${target.cssWidth}px` });
      // Never let the LRU close the frame that is on screen right now.
      pinPreviewFrame(key);
      onNaturalSize?.({ naturalHeight: target.cssHeight, naturalWidth: target.cssWidth });
      onLoadingChange?.(false);
      onMediaReady?.();
    };

    // 1. Synchronous path. A pooled neighbour paints in this very tick — no
    //    await, no decode, no loading flash.
    const warm = readAnyPooledPreviewFrame(media);
    if (warm) {
      paint(warm.bitmap, warm.key);
    } else {
      onLoadingChange?.(true);
    }

    // 2. Resolve the intended representation. When the warm frame already was
    //    it, `requestPreviewBitmap` returns the same pooled bitmap and this
    //    repaint is a no-op.
    void requestPreviewBitmap(media, { signal: controller.signal, viewportLongPx })
      .then((frame) => {
        if (disposed) {
          return;
        }
        paint(frame.bitmap, frame.key);
        // 3. Sharpening upgrade — only when what we painted really is a
        //    DOWNSCALE of the original. `original` is already sharp, and
        //    `preview_native` is a format transcode at native size, so asking
        //    for an upgrade there would re-fetch identical bytes.
        if (frame.tier === "original" || frame.tier === "preview_native") {
          return;
        }
        void requestPreviewBitmap(media, {
          original: true,
          resourcePriority: "ahead",
          signal: controller.signal
        })
          .then((sharp) => {
            if (!disposed) {
              paint(sharp.bitmap, sharp.key);
            }
          })
          .catch(() => undefined);
      })
      .catch((cause) => {
        if (disposed || controller.signal.aborted) {
          return;
        }
        if (cause instanceof DOMException && cause.name === "AbortError") {
          return;
        }
        setFailed(true);
        onLoadingChange?.(false);
        onMediaReady?.();
      });

    return () => {
      disposed = true;
      controller.abort();
    };
  }, [media, onLoadingChange, onMediaReady, onNaturalSize, viewportLongPx]);

  useEffect(() => () => pinPreviewFrame(null), []);

  if (failed) {
    return (
      <div className="preview-placeholder failed" style={boxStyle ?? frameStyle}>
        <span>load error</span>
      </div>
    );
  }

  return (
    <div
      className="preview-placeholder ready"
      data-preview-source="original"
      style={boxStyle ?? frameStyle}
    >
      <canvas
        aria-label={alt}
        className="preview-image preview-canvas"
        data-preview-media-id={media.id}
        ref={canvasRef}
        role="img"
        style={boxStyle}
      />
    </div>
  );
}
