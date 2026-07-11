import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type PointerEvent
} from "react";
import type { MediaRecord, ThumbnailResponse } from "@megle/core-client";
import {
  matchShortcut,
  useShortcutBindings
} from "../shortcuts/shortcutBindings";
import { MediaPreview } from "./MediaPreview";
import { DampedMotionAxis, normalizeWheelDelta } from "../../core/dampedMotion";

interface CentralPreviewStageProps {
  selectedMedia: MediaRecord;
  thumbnail?: ThumbnailResponse;
  onClosePreview: () => void;
  onPreviewPrevious: () => void;
  onPreviewNext: () => void;
  onPreviewMediaSettled: (mediaId: number) => void;
  onViewStateChange: (state: { mode: PreviewViewMode; scale: number }) => void;
  onCommandChange: (
    commands: { reset: () => void; toggleActualSize: () => void } | null
  ) => void;
}

type PreviewViewMode = "fit-long-edge" | "actual";

export function CentralPreviewStage({
  selectedMedia,
  thumbnail,
  onClosePreview,
  onPreviewMediaSettled,
  onPreviewPrevious,
  onPreviewNext,
  onViewStateChange,
  onCommandChange
}: CentralPreviewStageProps) {
  const stageRef = useRef<HTMLDivElement | null>(null);
  const dragRef = useRef<{ pointerId: number; x: number; y: number } | null>(null);
  const scaleRef = useRef(1);
  const wheelZoomAxisRef = useRef(
    new DampedMotionAxis({
      acceleration: 0.35,
      accelerationDelta: 420,
      friction: 0.84
    })
  );
  const wheelZoomFrameRef = useRef(0);
  const wheelZoomLastFrameTimeRef = useRef(0);
  const wheelZoomAnchorRef = useRef<{ x: number; y: number } | null>(null);
  const { bindings } = useShortcutBindings();
  const [scale, setScale] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [fitSyncTick, setFitSyncTick] = useState(0);
  const [previewReadyTick, setPreviewReadyTick] = useState(0);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [measuredNaturalSize, setMeasuredNaturalSize] = useState<{
    mediaId: number;
    naturalHeight: number;
    naturalWidth: number;
  } | null>(null);
  const fittedMediaIdRef = useRef<number | null>(null);
  const measuredNaturalSizeRef = useRef(measuredNaturalSize);
  const [viewMode, setViewMode] = useState<PreviewViewMode>("fit-long-edge");
  const viewModeRef = useRef(viewMode);
  const fitLongEdgeScaleForMedia = useCallback((sizeOverride?: {
    naturalHeight: number;
    naturalWidth: number;
  }) => {
    const stage = stageRef.current;
    const measuredSize =
      measuredNaturalSize?.mediaId === selectedMedia.id ? measuredNaturalSize : null;
    const width =
      sizeOverride?.naturalWidth ?? measuredSize?.naturalWidth ?? selectedMedia.width ?? 0;
    const height =
      sizeOverride?.naturalHeight ?? measuredSize?.naturalHeight ?? selectedMedia.height ?? 0;
    return fitScaleForDimensions(stage, width, height);
  }, [measuredNaturalSize, selectedMedia.height, selectedMedia.id, selectedMedia.width]);
  const resetTransform = useCallback(() => {
    const nextScale = fitLongEdgeScaleForMedia() ?? 1;
    setPan({ x: 0, y: 0 });
    viewModeRef.current = "fit-long-edge";
    setViewMode("fit-long-edge");
    scaleRef.current = nextScale;
    setScale(nextScale);
    setFitSyncTick((value) => value + 1);
    dragRef.current = null;
  }, [fitLongEdgeScaleForMedia]);

  useLayoutEffect(() => {
    stageRef.current?.focus({ preventScroll: true });
    dragRef.current = null;
  }, [selectedMedia.id]);

  useEffect(() => {
    measuredNaturalSizeRef.current = measuredNaturalSize;
  }, [measuredNaturalSize]);

  useEffect(() => {
    viewModeRef.current = viewMode;
  }, [viewMode]);

  useEffect(() => {
    scaleRef.current = scale;
  }, [scale]);

  useEffect(() => {
    fittedMediaIdRef.current = null;
    setMeasuredNaturalSize((current) =>
      current?.mediaId === selectedMedia.id ? current : null
    );
    measuredNaturalSizeRef.current =
      measuredNaturalSizeRef.current?.mediaId === selectedMedia.id
        ? measuredNaturalSizeRef.current
        : null;
  }, [selectedMedia.id]);

  useEffect(() => {
    let cancelled = false;
    let frame = 0;
    let attempts = 0;

    function requestSync() {
      if (frame) {
        window.cancelAnimationFrame(frame);
      }
      frame = window.requestAnimationFrame(syncFitLongEdge);
    }

    function syncFitLongEdge() {
      if (cancelled || viewModeRef.current !== "fit-long-edge") return;
      frame = 0;
      const nextScale = fitLongEdgeScale();
      if (nextScale !== null) {
        scaleRef.current = nextScale;
        setScale(nextScale);
        setPan({ x: 0, y: 0 });
        return;
      }
      attempts += 1;
      if (attempts < 90) {
        requestSync();
      }
    }

    const resizeObserver =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(() => {
            if (viewMode === "fit-long-edge") {
              requestSync();
            }
          });

    if (stageRef.current && resizeObserver) {
      resizeObserver.observe(stageRef.current);
    }
    window.addEventListener("resize", requestSync);
    requestSync();

    return () => {
      cancelled = true;
      resizeObserver?.disconnect();
      window.removeEventListener("resize", requestSync);
      window.cancelAnimationFrame(frame);
    };
  }, [fitSyncTick, previewReadyTick, viewMode]);

  const handleMediaReady = useCallback(() => {
    setPreviewReadyTick((value) => value + 1);
    onPreviewMediaSettled(selectedMedia.id);
  }, [onPreviewMediaSettled, selectedMedia.id]);
  const handleMediaLoadingChange = useCallback((loading: boolean) => {
    setPreviewLoading(loading);
  }, []);
  const handleNaturalSize = useCallback(
    (size: { naturalHeight: number; naturalWidth: number }) => {
      const measuredSize = { mediaId: selectedMedia.id, ...size };
      measuredNaturalSizeRef.current = measuredSize;
      setMeasuredNaturalSize(measuredSize);
      const shouldResetTransform = fittedMediaIdRef.current !== selectedMedia.id;
      if (shouldResetTransform) {
        fittedMediaIdRef.current = selectedMedia.id;
      }
      if (shouldResetTransform || viewModeRef.current === "fit-long-edge") {
        const nextScale =
          fitScaleForDimensions(stageRef.current, size.naturalWidth, size.naturalHeight) ?? 1;
        viewModeRef.current = "fit-long-edge";
        setViewMode("fit-long-edge");
        scaleRef.current = nextScale;
        setScale(nextScale);
        setPan({ x: 0, y: 0 });
        setFitSyncTick((value) => value + 1);
      }
    },
    [selectedMedia.id]
  );

  const zoomAtPoint = useCallback(
    (clientX: number, clientY: number, nextScale: number, nextMode: PreviewViewMode = viewMode) => {
      const stage = stageRef.current;
      if (!stage) return;
      const rect = stage.getBoundingClientRect();
      const currentScale = scaleRef.current;
      const local = {
        x: clientX - rect.left - rect.width / 2,
        y: clientY - rect.top - rect.height / 2
      };
      const clampedScale =
        nextMode === "fit-long-edge" ? clampFitScale(nextScale) : clampScale(nextScale);

      setPan((current) => {
        const imagePoint = {
          x: (local.x - current.x) / currentScale,
          y: (local.y - current.y) / currentScale
        };
        return {
          x: local.x - imagePoint.x * clampedScale,
          y: local.y - imagePoint.y * clampedScale
        };
      });
      scaleRef.current = clampedScale;
      viewModeRef.current = nextMode;
      setScale(clampedScale);
      setViewMode(nextMode);
    },
    [viewMode]
  );

  const toggleActualSizeAt = useCallback(
    (clientX: number, clientY: number) => {
      if (viewMode === "actual") {
        zoomAtPoint(clientX, clientY, fitLongEdgeScale() ?? 1, "fit-long-edge");
        return;
      }
      zoomAtPoint(clientX, clientY, actualSizeScale(), "actual");
    },
    [viewMode, zoomAtPoint]
  );

  const toggleActualSizeAtCenter = useCallback(() => {
    const point = stageCenterPoint();
    if (!point) return;
    toggleActualSizeAt(point.x, point.y);
  }, [toggleActualSizeAt]);

  useEffect(() => {
    onCommandChange({
      reset: resetTransform,
      toggleActualSize: toggleActualSizeAtCenter
    });
    return () => onCommandChange(null);
  }, [onCommandChange, resetTransform, toggleActualSizeAtCenter]);

  useEffect(() => {
    onViewStateChange({ mode: viewMode, scale });
  }, [onViewStateChange, scale, viewMode]);

  const stepDampedWheelZoom = useCallback((frameTime: number) => {
    wheelZoomFrameRef.current = 0;
    const elapsedMs =
      wheelZoomLastFrameTimeRef.current > 0
        ? frameTime - wheelZoomLastFrameTimeRef.current
        : undefined;
    wheelZoomLastFrameTimeRef.current = frameTime;
    const step = wheelZoomAxisRef.current.step(elapsedMs);
    const anchor = wheelZoomAnchorRef.current ?? stageCenterPoint();
    if (step !== 0 && anchor) {
      const nextScale = scaleRef.current * Math.exp(-step * 0.0012);
      zoomAtPoint(anchor.x, anchor.y, nextScale, "actual");
    }
    if (wheelZoomAxisRef.current.active) {
      wheelZoomFrameRef.current = window.requestAnimationFrame(stepDampedWheelZoom);
    } else {
      wheelZoomLastFrameTimeRef.current = 0;
    }
  }, [zoomAtPoint]);

  const onPreviewWheel = useCallback(
    (event: globalThis.WheelEvent) => {
      event.preventDefault();
      const dominantDelta =
        Math.abs(event.deltaY) >= Math.abs(event.deltaX) ? event.deltaY : event.deltaX;

      if (event.ctrlKey) {
        wheelZoomAnchorRef.current = { x: event.clientX, y: event.clientY };
        wheelZoomAxisRef.current.push(
          normalizeWheelDelta(dominantDelta, event.deltaMode, 18, 800)
        );
        if (!wheelZoomFrameRef.current) {
          wheelZoomLastFrameTimeRef.current = 0;
          wheelZoomFrameRef.current = window.requestAnimationFrame(stepDampedWheelZoom);
        }
        return;
      }

      if (dominantDelta > 0) onPreviewNext();
      else if (dominantDelta < 0) onPreviewPrevious();
    },
    [onPreviewNext, onPreviewPrevious, stepDampedWheelZoom]
  );

  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return undefined;
    stage.addEventListener("wheel", onPreviewWheel, { passive: false });
    return () => stage.removeEventListener("wheel", onPreviewWheel);
  }, [onPreviewWheel]);

  useEffect(() => {
    return () => {
      if (wheelZoomFrameRef.current) {
        window.cancelAnimationFrame(wheelZoomFrameRef.current);
      }
      wheelZoomFrameRef.current = 0;
      wheelZoomLastFrameTimeRef.current = 0;
      wheelZoomAxisRef.current.stop();
    };
  }, [selectedMedia.id]);

  useEffect(() => {
    function handlePreviewZoom(event: Event) {
      const direction = previewZoomDirection(event);
      if (!direction) return;
      const point = stageCenterPoint();
      if (!point) return;
      zoomAtPoint(
        point.x,
        point.y,
        scaleRef.current * (direction === "in" ? 1.18 : 0.85),
        "actual"
      );
    }

    window.addEventListener("megle:preview-zoom", handlePreviewZoom);
    return () => window.removeEventListener("megle:preview-zoom", handlePreviewZoom);
  }, [scale, zoomAtPoint]);

  function actualSizeScale() {
    return 1;
  }

  function fitLongEdgeScale() {
    const stage = stageRef.current;
    const metrics = stage ? previewMetrics(stage) : null;
    if (!metrics) {
      return fitLongEdgeScaleForMedia();
    }
    return clampFitScale(
      Math.min(
        metrics.stageRect.width / metrics.naturalWidth,
        metrics.stageRect.height / metrics.naturalHeight
      )
    );
  }

  function previewMetrics(stage: HTMLDivElement) {
    const mediaElement = stage.querySelector<HTMLImageElement | HTMLVideoElement>(
      ".preview-placeholder.ready .preview-image"
    );
    if (!stage || !mediaElement) {
      return null;
    }
    const naturalWidth =
      mediaElement instanceof HTMLVideoElement
        ? mediaElement.videoWidth
        : mediaElement.naturalWidth;
    const naturalHeight =
      mediaElement instanceof HTMLVideoElement
        ? mediaElement.videoHeight
        : mediaElement.naturalHeight;
    if (naturalWidth <= 0 || naturalHeight <= 0) return null;
    const stageRect = stage.getBoundingClientRect();
    if (stageRect.width <= 0 || stageRect.height <= 0) return null;
    return { naturalWidth, naturalHeight, stageRect };
  }

  function stageCenterPoint() {
    const stage = stageRef.current;
    if (!stage) return null;
    const rect = stage.getBoundingClientRect();
    return {
      x: rect.left + rect.width / 2,
      y: rect.top + rect.height / 2
    };
  }

  function handleDoubleClick(event: MouseEvent<HTMLDivElement>) {
    event.preventDefault();
    toggleActualSizeAt(event.clientX, event.clientY);
  }

  function handlePointerDown(event: PointerEvent<HTMLDivElement>) {
    if (event.button !== 0) return;
    if (shouldSkipPreviewPan(event.target, event.currentTarget)) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY };
  }

  function handlePointerMove(event: PointerEvent<HTMLDivElement>) {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const dx = event.clientX - drag.x;
    const dy = event.clientY - drag.y;
    dragRef.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY };
    setPan((current) => ({ x: current.x + dx, y: current.y + dy }));
  }

  function handlePointerUp(event: PointerEvent<HTMLDivElement>) {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    dragRef.current = null;
    event.currentTarget.releasePointerCapture(event.pointerId);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (matchShortcut(event, bindings, "closeOrReturn")) {
      event.preventDefault();
      onClosePreview();
    } else if (matchShortcut(event, bindings, "previewPrevious")) {
      event.preventDefault();
      onPreviewPrevious();
    } else if (matchShortcut(event, bindings, "previewNext")) {
      event.preventDefault();
      onPreviewNext();
    } else if (matchShortcut(event, bindings, "zoomIn")) {
      const point = stageCenterPoint();
      if (!point) return;
      event.preventDefault();
      zoomAtPoint(point.x, point.y, scaleRef.current * 1.18, "actual");
    } else if (matchShortcut(event, bindings, "zoomOut")) {
      const point = stageCenterPoint();
      if (!point) return;
      event.preventDefault();
      zoomAtPoint(point.x, point.y, scaleRef.current * 0.85, "actual");
    }
  }

  return (
    <section className="central-preview" aria-label={`Preview for ${selectedMedia.name}`}>
      <div
        aria-label={`Preview image for ${selectedMedia.name}`}
        className="central-preview-stage"
        data-preview-mode={viewMode}
        onDoubleClick={handleDoubleClick}
        onKeyDown={handleKeyDown}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={handlePointerUp}
        ref={stageRef}
        role="img"
        tabIndex={0}
      >
        <div
          className="central-preview-transform"
          style={{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${scale}) translate(-50%, -50%)` }}
        >
          <MediaPreview
            media={selectedMedia}
            onLoadingChange={handleMediaLoadingChange}
            onMediaReady={handleMediaReady}
            onNaturalSize={handleNaturalSize}
            source="original"
            stableFrame
            thumbnail={thumbnail}
          />
        </div>
        {previewLoading ? (
          <div
            aria-label="Loading preview"
            className="central-preview-loading-indicator"
            role="status"
          >
            <span className="central-preview-loading-spinner" aria-hidden="true" />
          </div>
        ) : null}
      </div>
    </section>
  );
}

const MAX_PREVIEW_SCALE = 8;
const MIN_INTERACTIVE_SCALE = 0.1;

function fitScaleForDimensions(
  stage: HTMLDivElement | null,
  width: number,
  height: number
) {
  if (!stage || width <= 0 || height <= 0) {
    return null;
  }
  const stageRect = stage.getBoundingClientRect();
  if (stageRect.width <= 0 || stageRect.height <= 0) {
    return null;
  }
  return clampFitScale(Math.min(stageRect.width / width, stageRect.height / height));
}

function clampScale(value: number) {
  return Math.min(MAX_PREVIEW_SCALE, Math.max(MIN_INTERACTIVE_SCALE, value));
}

function clampFitScale(value: number) {
  return Math.min(MAX_PREVIEW_SCALE, Math.max(0, value));
}

const PREVIEW_PAN_SKIP_SELECTOR = [
  "video",
  "audio",
  "button",
  "input",
  "select",
  "textarea",
  "[controls]",
  "[contenteditable]",
  "a",
  "[data-skip-preview-pan]"
].join(",");

function shouldSkipPreviewPan(eventTarget: EventTarget | null, currentTarget: HTMLElement) {
  if (!(eventTarget instanceof Element)) return false;
  const skipTarget = eventTarget.closest(PREVIEW_PAN_SKIP_SELECTOR);
  return skipTarget !== null && currentTarget.contains(skipTarget);
}

function previewZoomDirection(event: Event): "in" | "out" | null {
  if (!(event instanceof CustomEvent)) return null;
  const detail = event.detail as { direction?: unknown };
  return detail.direction === "in" || detail.direction === "out" ? detail.direction : null;
}
