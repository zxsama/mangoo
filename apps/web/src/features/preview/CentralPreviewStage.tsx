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
import { MediaLoadingIndicator, MediaLoadingRing } from "../loading/MediaLoadingRing";
import { DampedMotionAxis, normalizeWheelDelta } from "../../core/dampedMotion";
import { VideoPlaybackControls } from "./VideoPlaybackControls";
import {
  attemptVideoAutoplay,
  clampSeekTime,
  EMPTY_VIDEO_PLAYBACK_SNAPSHOT,
  isAutoplayPolicyError,
  normalizeVolume,
  type VideoPlaybackSnapshot
} from "./videoPlayback";
import {
  classifyVideoPointerGesture,
  pointerDisplacement,
  pointerDistance
} from "./previewPointerGesture";

interface CentralPreviewStageProps {
  selectedMedia: MediaRecord;
  thumbnail?: ThumbnailResponse;
  onClosePreview: () => void;
  onPreviewPrevious: () => void;
  onPreviewNext: () => void;
  onPreviewMediaSettled: (mediaId: number) => void;
  initialViewState: { mode: PreviewViewMode; scale: number };
  onViewStateChange: (state: { mode: PreviewViewMode; scale: number }) => void;
  onCommandChange: (
    commands: { reset: () => void; toggleActualSize: () => void } | null
  ) => void;
}

type PreviewViewMode = "fit-long-edge" | "actual";

type VideoPress = {
  lastX: number;
  lastY: number;
  pointerId: number;
  startPanX: number;
  startPanY: number;
  startX: number;
  startY: number;
};

export function CentralPreviewStage({
  selectedMedia,
  thumbnail,
  onClosePreview,
  onPreviewMediaSettled,
  onPreviewPrevious,
  onPreviewNext,
  initialViewState,
  onViewStateChange,
  onCommandChange
}: CentralPreviewStageProps) {
  const stageRef = useRef<HTMLDivElement | null>(null);
  const dragRef = useRef<{ pointerId: number; x: number; y: number } | null>(null);
  const videoPressRef = useRef<VideoPress | null>(null);
  const autoplayAttemptRef = useRef<{ element: HTMLVideoElement; mediaId: number } | null>(null);
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
  const [scale, setScale] = useState(initialViewState.scale);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [fitSyncTick, setFitSyncTick] = useState(0);
  const [previewReadyTick, setPreviewReadyTick] = useState(0);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [hasCommittedPreviewFrame, setHasCommittedPreviewFrame] = useState(false);
  const [previewDragging, setPreviewDragging] = useState(false);
  const [videoElement, setVideoElement] = useState<HTMLVideoElement | null>(null);
  const [videoPlayback, setVideoPlayback] = useState<VideoPlaybackSnapshot>({
    ...EMPTY_VIDEO_PLAYBACK_SNAPSHOT
  });
  const [measuredNaturalSize, setMeasuredNaturalSize] = useState<{
    mediaId: number;
    naturalHeight: number;
    naturalWidth: number;
  } | null>(null);
  const fittedMediaIdRef = useRef<number | null>(null);
  const lastCommittedNaturalSizeRef = useRef<{
    mediaId: number;
    naturalHeight: number;
    naturalWidth: number;
  } | null>(null);
  const panRef = useRef(pan);
  const measuredNaturalSizeRef = useRef(measuredNaturalSize);
  const [viewMode, setViewMode] = useState<PreviewViewMode>(initialViewState.mode);
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
    videoPressRef.current = null;
    setPreviewDragging(false);
  }, [fitLongEdgeScaleForMedia]);

  useLayoutEffect(() => {
    stageRef.current?.focus({ preventScroll: true });
    dragRef.current = null;
    videoPressRef.current = null;
    setPreviewDragging(false);
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
    panRef.current = pan;
  }, [pan]);

  useEffect(() => {
    fittedMediaIdRef.current = null;
    autoplayAttemptRef.current = null;
    setVideoPlayback({ ...EMPTY_VIDEO_PLAYBACK_SNAPSHOT });
    setMeasuredNaturalSize((current) =>
      current?.mediaId === selectedMedia.id ? current : null
    );
    measuredNaturalSizeRef.current =
      measuredNaturalSizeRef.current?.mediaId === selectedMedia.id
        ? measuredNaturalSizeRef.current
        : null;
  }, [selectedMedia.id]);

  const handleVideoElementChange = useCallback((element: HTMLVideoElement | null) => {
    setVideoElement(element);
  }, []);

  const syncVideoPlayback = useCallback((element: HTMLVideoElement) => {
    setVideoPlayback((current) => ({
      autoplayBlocked: current.autoplayBlocked,
      currentTime: Number.isFinite(element.currentTime) ? Math.max(0, element.currentTime) : 0,
      duration: Number.isFinite(element.duration) ? Math.max(0, element.duration) : 0,
      muted: element.muted,
      playing: !element.paused && !element.ended,
      volume: normalizeVolume(element.volume)
    }));
  }, []);

  useEffect(() => {
    if (!videoElement) return undefined;
    const sync = () => syncVideoPlayback(videoElement);
    const events = [
      "canplay",
      "durationchange",
      "ended",
      "loadedmetadata",
      "pause",
      "play",
      "timeupdate",
      "volumechange"
    ] as const;
    events.forEach((eventName) => videoElement.addEventListener(eventName, sync));
    sync();
    return () => events.forEach((eventName) => videoElement.removeEventListener(eventName, sync));
  }, [syncVideoPlayback, videoElement]);

  useEffect(() => {
    if (!videoElement || selectedMedia.kind !== "video") return;
    if (Number(videoElement.dataset.previewMediaId) !== selectedMedia.id) return;
    if (
      autoplayAttemptRef.current?.element === videoElement &&
      autoplayAttemptRef.current.mediaId === selectedMedia.id
    ) {
      return;
    }
    autoplayAttemptRef.current = { element: videoElement, mediaId: selectedMedia.id };
    let cancelled = false;
    void attemptVideoAutoplay(videoElement).then((result) => {
      if (cancelled || autoplayAttemptRef.current?.element !== videoElement) return;
      syncVideoPlayback(videoElement);
      setVideoPlayback((current) => ({
        ...current,
        autoplayBlocked: result === "blocked"
      }));
    });
    return () => {
      cancelled = true;
    };
  }, [selectedMedia.id, selectedMedia.kind, syncVideoPlayback, videoElement]);

  const toggleVideoPlayback = useCallback(async () => {
    if (!videoElement) return;
    if (!videoElement.paused && !videoElement.ended) {
      videoElement.pause();
      return;
    }
    try {
      await videoElement.play();
      setVideoPlayback((current) => ({ ...current, autoplayBlocked: false }));
    } catch (error) {
      setVideoPlayback((current) => ({
        ...current,
        autoplayBlocked: isAutoplayPolicyError(error)
      }));
    }
  }, [videoElement]);

  const seekVideo = useCallback((time: number) => {
    if (!videoElement) return;
    videoElement.currentTime = clampSeekTime(time, videoElement.duration);
    syncVideoPlayback(videoElement);
  }, [syncVideoPlayback, videoElement]);

  const toggleVideoMuted = useCallback(() => {
    if (!videoElement) return;
    videoElement.muted = !videoElement.muted;
    syncVideoPlayback(videoElement);
  }, [syncVideoPlayback, videoElement]);

  const changeVideoVolume = useCallback((volume: number) => {
    if (!videoElement) return;
    const nextVolume = normalizeVolume(volume);
    videoElement.volume = nextVolume;
    videoElement.muted = nextVolume === 0;
    syncVideoPlayback(videoElement);
  }, [syncVideoPlayback, videoElement]);

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
      // Keep the previous frame's scale while the next image is buffered. If
      // metadata from the next media item is used here, the visible old image
      // is resized before the atomic preview swap and briefly appears warped.
      if (fittedMediaIdRef.current !== selectedMedia.id) return;
      frame = 0;
      const nextScale = fitLongEdgeScaleForMedia();
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
  }, [fitLongEdgeScaleForMedia, fitSyncTick, previewReadyTick, viewMode]);

  const handleMediaReady = useCallback(() => {
    setHasCommittedPreviewFrame(true);
    setPreviewReadyTick((value) => value + 1);
    onPreviewMediaSettled(selectedMedia.id);
  }, [onPreviewMediaSettled, selectedMedia.id]);
  const handleMediaLoadingChange = useCallback((loading: boolean) => {
    setPreviewLoading(loading);
  }, []);
  const handleNaturalSize = useCallback(
    (size: { naturalHeight: number; naturalWidth: number }) => {
      const measuredSize = { mediaId: selectedMedia.id, ...size };
      const previousNaturalSize =
        lastCommittedNaturalSizeRef.current?.mediaId === selectedMedia.id
          ? null
          : lastCommittedNaturalSizeRef.current;
      lastCommittedNaturalSizeRef.current = measuredSize;
      measuredNaturalSizeRef.current = measuredSize;
      setMeasuredNaturalSize(measuredSize);
      const shouldResetTransform = fittedMediaIdRef.current !== selectedMedia.id;
      if (shouldResetTransform) {
        fittedMediaIdRef.current = selectedMedia.id;
        if (viewModeRef.current === "fit-long-edge") {
          const nextScale =
            fitScaleForDimensions(stageRef.current, size.naturalWidth, size.naturalHeight) ?? 1;
          scaleRef.current = nextScale;
          setScale(nextScale);
          panRef.current = { x: 0, y: 0 };
          setPan({ x: 0, y: 0 });
        } else if (previousNaturalSize) {
          const nextTransform = preserveTransformForResolution(
            previousNaturalSize,
            size,
            scaleRef.current,
            panRef.current
          );
          scaleRef.current = nextTransform.scale;
          setScale(nextTransform.scale);
          panRef.current = nextTransform.pan;
          setPan(nextTransform.pan);
        }
        setFitSyncTick((value) => value + 1);
      } else if (viewModeRef.current === "fit-long-edge") {
        const nextScale =
          fitScaleForDimensions(stageRef.current, size.naturalWidth, size.naturalHeight) ?? 1;
        scaleRef.current = nextScale;
        setScale(nextScale);
        panRef.current = { x: 0, y: 0 };
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
        zoomAtPoint(clientX, clientY, fitLongEdgeScaleForMedia() ?? 1, "fit-long-edge");
        return;
      }
      zoomAtPoint(clientX, clientY, actualSizeScale(), "actual");
    },
    [fitLongEdgeScaleForMedia, viewMode, zoomAtPoint]
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
    if (isVideoSurface(event.target, event.currentTarget)) return;
    toggleActualSizeAt(event.clientX, event.clientY);
  }

  function handlePointerDown(event: PointerEvent<HTMLDivElement>) {
    if (event.button !== 0 || !event.isPrimary) return;
    if (isVideoSurface(event.target, event.currentTarget)) {
      event.preventDefault();
      event.currentTarget.setPointerCapture(event.pointerId);
      videoPressRef.current = {
        lastX: event.clientX,
        lastY: event.clientY,
        pointerId: event.pointerId,
        startPanX: pan.x,
        startPanY: pan.y,
        startX: event.clientX,
        startY: event.clientY
      };
      return;
    }
    if (shouldSkipPreviewPan(event.target, event.currentTarget)) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY };
    setPreviewDragging(true);
  }

  function handlePointerMove(event: PointerEvent<HTMLDivElement>) {
    const videoPress = videoPressRef.current;
    if (videoPress?.pointerId === event.pointerId) {
      const delta = pointerDisplacement(
        videoPress.startX,
        videoPress.startY,
        event.clientX,
        event.clientY
      );
      videoPress.lastX = event.clientX;
      videoPress.lastY = event.clientY;
      if (delta.x === 0 && delta.y === 0) return;
      setPreviewDragging(true);
      setPan({
        x: videoPress.startPanX + delta.x,
        y: videoPress.startPanY + delta.y
      });
      return;
    }
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const dx = event.clientX - drag.x;
    const dy = event.clientY - drag.y;
    dragRef.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY };
    setPan((current) => ({ x: current.x + dx, y: current.y + dy }));
  }

  function handlePointerUp(event: PointerEvent<HTMLDivElement>) {
    const videoPress = videoPressRef.current;
    if (videoPress?.pointerId === event.pointerId) {
      const distance = pointerDistance(
        videoPress.startX,
        videoPress.startY,
        event.clientX,
        event.clientY
      );
      const gesture = classifyVideoPointerGesture({
        distancePx: distance
      });
      const finalDelta = pointerDisplacement(
        videoPress.startX,
        videoPress.startY,
        event.clientX,
        event.clientY
      );
      setPan(
        gesture === "tap"
          ? { x: videoPress.startPanX, y: videoPress.startPanY }
          : {
              x: videoPress.startPanX + finalDelta.x,
              y: videoPress.startPanY + finalDelta.y
            }
      );
      videoPressRef.current = null;
      setPreviewDragging(false);
      releasePointerCapture(event.currentTarget, event.pointerId);
      if (gesture === "tap") {
        void toggleVideoPlayback();
      }
      return;
    }
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    dragRef.current = null;
    setPreviewDragging(false);
    releasePointerCapture(event.currentTarget, event.pointerId);
  }

  function handlePointerCancel(event: PointerEvent<HTMLDivElement>) {
    const videoPress = videoPressRef.current;
    if (videoPress?.pointerId === event.pointerId) {
      const distance = pointerDistance(
        videoPress.startX,
        videoPress.startY,
        videoPress.lastX,
        videoPress.lastY
      );
      if (classifyVideoPointerGesture({ distancePx: distance }) === "tap") {
        setPan({ x: videoPress.startPanX, y: videoPress.startPanY });
      }
      videoPressRef.current = null;
    }
    if (dragRef.current?.pointerId === event.pointerId) {
      dragRef.current = null;
    }
    setPreviewDragging(false);
    releasePointerCapture(event.currentTarget, event.pointerId);
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

  const isVideo = selectedMedia.kind === "video";

  return (
    <section className="central-preview" aria-label={`Preview for ${selectedMedia.name}`}>
      <div className="central-preview-stage" data-preview-video={isVideo ? "true" : undefined}>
        <div
          aria-label={`Preview media for ${selectedMedia.name}`}
          className="central-preview-viewport"
          data-preview-dragging={previewDragging ? "true" : undefined}
          data-preview-mode={viewMode}
          onDoubleClick={handleDoubleClick}
          onKeyDown={handleKeyDown}
          onLostPointerCapture={handlePointerCancel}
          onPointerCancel={handlePointerCancel}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          ref={stageRef}
          role="img"
          tabIndex={0}
        >
          <div
            className="central-preview-transform"
            data-preview-frame-ready={hasCommittedPreviewFrame ? "true" : "false"}
            style={{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${scale}) translate(-50%, -50%)` }}
          >
            <MediaPreview
              media={selectedMedia}
              source="original"
              thumbnail={thumbnail}
              onLoadingChange={handleMediaLoadingChange}
              onMediaReady={handleMediaReady}
              onNaturalSize={handleNaturalSize}
              onVideoElementChange={handleVideoElementChange}
              stableFrame
            />
          </div>
          <MediaLoadingIndicator
            active={previewLoading}
            className="central-preview-loading-indicator"
            label="Loading preview"
          >
            <MediaLoadingRing />
          </MediaLoadingIndicator>
        </div>
        {isVideo ? (
          <VideoPlaybackControls
            disabled={!videoElement}
            onSeek={seekVideo}
            onToggleMuted={toggleVideoMuted}
            onTogglePlayback={() => void toggleVideoPlayback()}
            onVolumeChange={changeVideoVolume}
            state={videoPlayback}
          />
        ) : null}
      </div>
    </section>
  );
}

const MAX_PREVIEW_SCALE = 8;
const MIN_INTERACTIVE_SCALE = 0.1;

function preserveTransformForResolution(
  previous: { naturalHeight: number; naturalWidth: number },
  next: { naturalHeight: number; naturalWidth: number },
  scale: number,
  pan: { x: number; y: number }
) {
  const previousLongEdge = Math.max(previous.naturalWidth, previous.naturalHeight);
  const nextLongEdge = Math.max(next.naturalWidth, next.naturalHeight);
  const ratio = previousLongEdge / nextLongEdge;
  return {
    pan: { x: pan.x * ratio, y: pan.y * ratio },
    scale: clampScale(scale * ratio)
  };
}

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

function isVideoSurface(eventTarget: EventTarget | null, currentTarget: HTMLElement) {
  if (!(eventTarget instanceof Element)) return false;
  const video = eventTarget.closest(".preview-video");
  return video !== null && currentTarget.contains(video);
}

function releasePointerCapture(target: HTMLElement, pointerId: number) {
  if (target.hasPointerCapture(pointerId)) {
    target.releasePointerCapture(pointerId);
  }
}

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
