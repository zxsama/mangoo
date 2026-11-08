import { useCallback, useEffect, useLayoutEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { LiquidGlassSurface, icons } from "@mangoo/web/plugin-host";

import {
  clampRect,
  defaultPlacement,
  parsePlacement,
  relayout,
  snapRect,
  type PanelPlacement,
  type PanelRect
} from "../shared/floatingPanel.ts";

/**
 * A draggable frosted panel that floats over the picture and snaps to the
 * window's edges and corners (magnetic within 18px). Its placement (and
 * which edges it hugs) is remembered per panel id, so a panel docked to the
 * right edge stays docked when the window is resized.
 */
export function FloatingPanel({
  id,
  title,
  icon,
  defaultSize,
  insets,
  onClose,
  children,
  headerExtra,
  resizable = true,
  autoHeight = true,
  onDockChange,
  className
}: {
  id: string;
  title: string;
  icon?: ReactNode;
  defaultSize: { width: number; height: number };
  /** Space reserved for the titlebar / filmstrip, in px. */
  insets: { top: number; bottom: number };
  onClose(): void;
  children: ReactNode;
  headerExtra?: ReactNode;
  resizable?: boolean;
  /** Grow with the content up to the stored height instead of a fixed box. */
  autoHeight?: boolean;
  /** Which side edge the panel hugs, so the window can move its edge arrows. */
  onDockChange?(side: "left" | "right" | null): void;
  className?: string;
}) {
  const storageKey = `mangoo.imageViewer.panel.${id}`;
  const hostRef = useRef<HTMLDivElement | null>(null);
  const [bounds, setBounds] = useState({ width: window.innerWidth, height: window.innerHeight });
  const [placement, setPlacement] = useState<PanelPlacement>(() => {
    try {
      const stored = parsePlacement(JSON.parse(window.localStorage.getItem(storageKey) ?? "null"));
      if (stored) return stored;
    } catch {
      // ignore
    }
    return defaultPlacement(defaultSize, { width: window.innerWidth, height: window.innerHeight }, insets);
  });
  const [dragging, setDragging] = useState(false);
  /** Rendered height of an auto-height panel, so bottom docking hugs its real edge. */
  const [measuredHeight, setMeasuredHeight] = useState<number | null>(null);
  useLayoutEffect(() => {
    if (!autoHeight) return;
    const host = hostRef.current;
    if (!host) return;
    const update = () => setMeasuredHeight(host.getBoundingClientRect().height);
    update();
    const observer = new ResizeObserver(update);
    observer.observe(host);
    return () => observer.disconnect();
  }, [autoHeight]);
  const drag = useRef<{ startX: number; startY: number; origin: PanelRect; mode: "move" | "resize" } | null>(null);

  useLayoutEffect(() => {
    const onResize = () => setBounds({ width: window.innerWidth, height: window.innerHeight });
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const persist = useCallback((next: PanelPlacement) => {
    try {
      window.localStorage.setItem(storageKey, JSON.stringify(next));
    } catch {
      // storage unavailable: placement still applies for this session
    }
  }, [storageKey]);

  const effective: PanelPlacement = autoHeight && measuredHeight
    ? { ...placement, rect: { ...placement.rect, height: Math.min(placement.rect.height, measuredHeight) } }
    : placement;
  const laidOut = dragging ? clampRect(effective.rect, bounds, insets) : relayout(effective, bounds, insets);
  const rect = autoHeight ? { ...laidOut, height: placement.rect.height } : laidOut;

  function begin(event: ReactPointerEvent<HTMLElement>, mode: "move" | "resize") {
    if (event.button !== 0) return;
    if (mode === "move" && (event.target as HTMLElement).closest("button, input, select, a")) return;
    event.preventDefault();
    event.stopPropagation();
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    drag.current = { startX: event.clientX, startY: event.clientY, origin: rect, mode };
    setDragging(true);
  }

  function move(event: ReactPointerEvent<HTMLElement>) {
    const state = drag.current;
    if (!state) return;
    const dx = event.clientX - state.startX;
    const dy = event.clientY - state.startY;
    const next: PanelRect = state.mode === "move"
      ? { ...state.origin, x: state.origin.x + dx, y: state.origin.y + dy }
      : { ...state.origin, width: Math.max(220, state.origin.width + dx), height: Math.max(160, state.origin.height + dy) };
    // Live magnetic preview while moving; resizing keeps the current dock.
    setPlacement(state.mode === "move" ? snapRect(next, bounds, insets) : { ...placement, rect: next });
  }

  function end(event: ReactPointerEvent<HTMLElement>) {
    if (!drag.current) return;
    (event.currentTarget as HTMLElement).releasePointerCapture?.(event.pointerId);
    const finalPlacement = drag.current.mode === "move"
      ? snapRect(placement.rect, bounds, insets)
      : { ...placement, rect: clampRect(placement.rect, bounds, insets) };
    drag.current = null;
    setDragging(false);
    setPlacement(finalPlacement);
    persist(finalPlacement);
  }

  function resetPosition() {
    const next = defaultPlacement(defaultSize, bounds, insets);
    setPlacement(next);
    persist(next);
  }

  useEffect(() => {
    // Keyboard: Escape on the panel closes it without closing the window.
    const host = hostRef.current;
    if (!host) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    host.addEventListener("keydown", onKey);
    return () => host.removeEventListener("keydown", onKey);
  }, [onClose]);

  const docked = placement.dock.horizontal || placement.dock.vertical;
  const horizontalDock = placement.dock.horizontal;
  useEffect(() => {
    onDockChange?.(horizontalDock);
  }, [horizontalDock, onDockChange]);
  useEffect(() => () => onDockChange?.(null), [onDockChange]);
  return (
    <div
      className={`iv-float${className ? ` ${className}` : ""}`}
      data-docked={docked ? "true" : "false"}
      data-dragging={dragging ? "true" : "false"}
      ref={hostRef}
      role="dialog"
      aria-label={title}
      data-auto-height={autoHeight ? "true" : "false"}
      style={autoHeight
        ? { left: rect.x, top: rect.y, width: rect.width, maxHeight: rect.height }
        : { left: rect.x, top: rect.y, width: rect.width, height: rect.height }}
    >
      <LiquidGlassSurface as="div" className="iv-float-surface" tone="elevated">
        <header
          className="iv-float-header"
          onDoubleClick={resetPosition}
          onPointerCancel={end}
          onPointerDown={(event) => begin(event, "move")}
          onPointerMove={move}
          onPointerUp={end}
          title="拖动以移动，靠近窗口边缘时会自动吸附；双击复位"
        >
          {icon ? <span className="iv-float-icon">{icon}</span> : null}
          <span className="iv-float-title">{title}</span>
          <span className="iv-float-header-extra">{headerExtra}</span>
          <button aria-label={`关闭${title}`} className="iv-float-close" data-pointer-glow="off" onClick={onClose} type="button">
            <icons.X aria-hidden="true" size={14} />
          </button>
        </header>
        <div className="iv-float-body">{children}</div>
        {resizable ? (
          <span
            aria-hidden="true"
            className="iv-float-resize"
            onPointerCancel={end}
            onPointerDown={(event) => begin(event, "resize")}
            onPointerMove={move}
            onPointerUp={end}
          />
        ) : null}
      </LiquidGlassSurface>
    </div>
  );
}
