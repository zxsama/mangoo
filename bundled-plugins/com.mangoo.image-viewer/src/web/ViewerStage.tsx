import { useEffect, useLayoutEffect, useRef, type DragEvent, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";

import { cssTransform, type Size, type ViewTransform } from "../shared/viewTransform.ts";
import type { LoadedImage } from "./imageSource.ts";
import type { ViewerBackground } from "./preferences.ts";

export interface ViewerStageProps {
  image: LoadedImage | null;
  /** Image box in CSS px at scale 1 (source px / devicePixelRatio). */
  imageSize: Size | null;
  transform: ViewTransform;
  animate: boolean;
  background: ViewerBackground;
  pixelated: boolean;
  /** Backing-store scale for the canvas (see renderLevel). */
  renderLevel: number;
  /** Animation frame to paint instead of the image (frame inspector). */
  frameBitmap?: ImageBitmap | null;
  overlay?: ReactNode;
  onViewportChange(size: Size): void;
  onWheelZoom(factor: number, anchor: { x: number; y: number }): void;
  onWheelNavigate(direction: 1 | -1): void;
  wheelNavigates: boolean;
  /** Picture larger than the window: plain wheel pans instead of navigating. */
  zoomedIn?: boolean;
  onPan(deltaX: number, deltaY: number): void;
  onPinch(factor: number, anchor: { x: number; y: number }): void;
  onToggleActualSize(anchor: { x: number; y: number }): void;
  onNavigate(direction: 1 | -1): void;
  onContextMenu(x: number, y: number): void;
  onDropFiles(files: File[]): void;
  onActivity(): void;
}

function pointIn(element: HTMLElement, clientX: number, clientY: number) {
  const rect = element.getBoundingClientRect();
  return { x: clientX - rect.left, y: clientY - rect.top };
}

export function ViewerStage(props: ViewerStageProps) {
  const stageRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const propsRef = useRef(props);
  propsRef.current = props;
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const pinchDistance = useRef<number | null>(null);
  const dragged = useRef(false);

  // Measure the viewport.
  useLayoutEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const report = () => propsRef.current.onViewportChange({ width: stage.clientWidth, height: stage.clientHeight });
    report();
    const observer = new ResizeObserver(report);
    observer.observe(stage);
    return () => observer.disconnect();
  }, []);

  // Paint the stable canvas from the decoded bitmap (never remounted).
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    const image = props.image;
    const frame = props.frameBitmap ?? null;
    if (!canvas || !image || (image.kind !== "bitmap" && !frame)) return;
    const bitmap = frame ?? (image.kind === "bitmap" ? image.bitmap : null);
    if (!bitmap) return;
    const level = frame ? 1 : props.renderLevel;
    const width = Math.max(1, Math.round(bitmap.width * level));
    const height = Math.max(1, Math.round(bitmap.height * level));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    const context = canvas.getContext("2d", { alpha: true });
    if (!context) return;
    context.imageSmoothingEnabled = true;
    context.imageSmoothingQuality = "high";
    context.clearRect(0, 0, width, height);
    try {
      context.drawImage(bitmap, 0, 0, width, height);
    } catch {
      // A bitmap closed by eviction between render and paint: the next
      // load repaints, so leaving the previous frame is the right fallback.
    }
  }, [props.image, props.renderLevel, props.frameBitmap]);

  // Non-passive wheel so the page never scrolls.
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    let navigateCooldown = 0;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const current = propsRef.current;
      current.onActivity();
      const zoomGesture = event.ctrlKey || !current.wheelNavigates;
      if (!zoomGesture && current.zoomedIn) {
        current.onPan(-event.deltaX, -event.deltaY);
        return;
      }
      if (!zoomGesture) {
        const now = performance.now();
        if (now < navigateCooldown || Math.abs(event.deltaY) < 4) return;
        navigateCooldown = now + 140;
        current.onWheelNavigate(event.deltaY > 0 ? 1 : -1);
        return;
      }
      const pixels = event.deltaMode === 1 ? event.deltaY * 16 : event.deltaMode === 2 ? event.deltaY * 400 : event.deltaY;
      // A mouse notch (|delta| >= 50) zooms ~1.2x; trackpad pinches arrive as
      // ctrl+wheel with small deltas and get a finer, proportional response.
      const clamped = Math.max(-240, Math.min(240, pixels));
      const pinch = event.ctrlKey && event.deltaMode === 0 && Math.abs(pixels) < 50;
      const factor = Math.exp(-clamped * (pinch ? 0.01 : 0.0015));
      current.onWheelZoom(factor, pointIn(stage, event.clientX, event.clientY));
    };
    stage.addEventListener("wheel", onWheel, { passive: false });
    return () => stage.removeEventListener("wheel", onWheel);
  }, []);

  function handlePointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    props.onActivity();
    if (event.button === 3 || event.button === 4) {
      event.preventDefault();
      props.onNavigate(event.button === 3 ? -1 : 1);
      return;
    }
    if (event.button !== 0 && event.pointerType === "mouse") return;
    // Controls rendered over the stage (empty state, messages) keep their clicks.
    if ((event.target as HTMLElement).closest("button, a, input, [role=\"button\"]")) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    dragged.current = false;
    if (pointers.current.size === 2) {
      const [a, b] = [...pointers.current.values()];
      pinchDistance.current = Math.hypot(a.x - b.x, a.y - b.y);
    }
  }

  function handlePointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    props.onActivity();
    const previous = pointers.current.get(event.pointerId);
    if (!previous) return;
    const next = { x: event.clientX, y: event.clientY };
    pointers.current.set(event.pointerId, next);
    if (pointers.current.size >= 2 && pinchDistance.current) {
      const [a, b] = [...pointers.current.values()];
      const distance = Math.hypot(a.x - b.x, a.y - b.y);
      if (distance > 0) {
        const stage = stageRef.current;
        if (stage) props.onPinch(distance / pinchDistance.current, pointIn(stage, (a.x + b.x) / 2, (a.y + b.y) / 2));
        pinchDistance.current = distance;
      }
      dragged.current = true;
      return;
    }
    const deltaX = next.x - previous.x;
    const deltaY = next.y - previous.y;
    if (Math.abs(deltaX) + Math.abs(deltaY) > 0) {
      if (Math.abs(deltaX) + Math.abs(deltaY) > 2) dragged.current = true;
      props.onPan(deltaX, deltaY);
    }
  }

  function handlePointerUp(event: ReactPointerEvent<HTMLDivElement>) {
    pointers.current.delete(event.pointerId);
    if (pointers.current.size < 2) pinchDistance.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
  }

  function handleDoubleClick(event: ReactMouseEvent<HTMLDivElement>) {
    const stage = stageRef.current;
    if (!stage || !props.image) return;
    props.onToggleActualSize(pointIn(stage, event.clientX, event.clientY));
  }

  function handleContextMenu(event: ReactMouseEvent<HTMLDivElement>) {
    event.preventDefault();
    props.onContextMenu(event.clientX, event.clientY);
  }

  function handleDragOver(event: DragEvent<HTMLDivElement>) {
    if ([...event.dataTransfer.types].includes("Files")) {
      event.preventDefault();
      event.dataTransfer.dropEffect = "copy";
      event.currentTarget.dataset.dropActive = "true";
    }
  }

  function handleDrop(event: DragEvent<HTMLDivElement>) {
    event.preventDefault();
    delete event.currentTarget.dataset.dropActive;
    const files = [...event.dataTransfer.files];
    if (files.length) props.onDropFiles(files);
  }

  const size = props.imageSize;
  const layerStyle = size
    ? {
        width: `${size.width}px`,
        height: `${size.height}px`,
        marginLeft: `${-size.width / 2}px`,
        marginTop: `${-size.height / 2}px`,
        transform: cssTransform(props.transform)
      }
    : undefined;

  return (
    <div
      className="iv-stage"
      data-background={props.background}
      data-has-image={props.image ? "true" : "false"}
      onContextMenu={handleContextMenu}
      onDoubleClick={handleDoubleClick}
      onDragLeave={(event) => delete event.currentTarget.dataset.dropActive}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
      onPointerCancel={handlePointerUp}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      ref={stageRef}
    >
      <div
        className="iv-image-layer"
        data-animate={props.animate ? "true" : "false"}
        data-pixelated={props.pixelated ? "true" : "false"}
        hidden={!props.image}
        style={layerStyle}
      >
        <canvas className="iv-canvas" hidden={props.image?.kind !== "bitmap" && !props.frameBitmap} ref={canvasRef} />
        {props.image?.kind === "animated" && !props.frameBitmap ? (
          <img alt="" className="iv-animated" draggable={false} src={props.image.url} />
        ) : null}
      </div>
      {props.overlay}
    </div>
  );
}
