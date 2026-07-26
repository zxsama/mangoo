import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
  type ReactPortal
} from "react";
import { createPortal } from "react-dom";
import { useOverlayStack } from "./OverlayProvider";
import { OverlaySurface } from "./OverlaySurface";
import {
  clampOverlayWidth,
  resolveOverlayOffset,
  resolveOverlayPosition
} from "./overlayPosition";
import {
  OVERLAY_VIEWPORT_GUTTER_PX,
  type AnchoredOverlayStyle,
  type OverlayAnchor,
  type OverlayPlacement,
  type OverlayPositionResult,
  type OverlayRootId
} from "./overlayTypes";
import { useOverlayPresence } from "./useOverlayPresence";

export interface AnchoredOverlayProps {
  anchor: OverlayAnchor | null;
  ariaLabel: string;
  children: ReactNode;
  className?: string;
  id: string;
  minWidth?: number;
  offset?: number;
  onDismiss: () => void;
  open: boolean;
  parentId?: string;
  placement?: OverlayPlacement;
  preferredWidth?: number;
  role?: "menu" | "listbox" | "dialog" | "status";
  restoreFocusTarget?: HTMLElement | null;
  rootId: OverlayRootId;
  surfaceRole?: "popover" | "nested";
}

function positionUnchanged(
  current: OverlayPositionResult | null,
  next: OverlayPositionResult
): boolean {
  return Boolean(
    current &&
    current.left === next.left &&
    current.top === next.top &&
    current.maxWidth === next.maxWidth &&
    current.maxHeight === next.maxHeight &&
    current.placement === next.placement
  );
}

export function AnchoredOverlay({
  anchor,
  ariaLabel,
  children,
  className,
  id,
  minWidth,
  offset,
  onDismiss,
  open,
  parentId,
  placement = "bottom-start",
  preferredWidth,
  role = "menu",
  restoreFocusTarget,
  rootId,
  surfaceRole
}: AnchoredOverlayProps): ReactPortal | null {
  const surfaceRef = useRef<HTMLElement | null>(null);
  const retainedAnchorRef = useRef<OverlayAnchor | null>(anchor);
  const [position, setPosition] = useState<OverlayPositionResult | null>(null);
  const { openRoot, registerLayer } = useOverlayStack();
  const resolvedSurfaceRole = surfaceRole ?? (parentId ? "nested" : "popover");
  const kind = resolvedSurfaceRole === "nested" ? "nested" : "root";
  const presence = useOverlayPresence(open, kind);
  if (anchor) retainedAnchorRef.current = anchor;
  const renderedAnchor = anchor ?? retainedAnchorRef.current;

  useLayoutEffect(() => {
    surfaceRef.current = presence.present ? document.getElementById(id) : null;
    if (!presence.present) {
      setPosition(null);
      retainedAnchorRef.current = null;
    }
    return () => {
      surfaceRef.current = null;
    };
  }, [id, presence.present]);

  useEffect(() => {
    if (!open || !anchor) return;
    const surface = surfaceRef.current;
    if (!surface) return;

    if (kind === "root") {
      openRoot(rootId);
    }
    const trigger = restoreFocusTarget !== undefined
      ? restoreFocusTarget
      : typeof HTMLElement !== "undefined" && anchor instanceof HTMLElement
        ? anchor
        : null;

    return registerLayer({
      id,
      rootId,
      parentId,
      kind,
      trigger,
      surface,
      dismissOnEscape: true,
      dismissOnFocusOutside: true,
      dismissOnOutsidePointer: true,
      dismissOnWindowBlur: true,
      restoreFocus: trigger !== null,
      onDismiss
    });
  }, [
    anchor,
    id,
    kind,
    onDismiss,
    open,
    openRoot,
    parentId,
    registerLayer,
    restoreFocusTarget,
    rootId
  ]);

  useEffect(() => {
    if (!open || !anchor) return;
    const surface = surfaceRef.current;
    if (!surface) return;

    let frame = 0;
    const measure = () => {
      frame = 0;
      const triggerRect = anchor.getBoundingClientRect();
      const surfaceRect = surface.getBoundingClientRect();
      const viewport = { width: window.innerWidth, height: window.innerHeight };
      const maxViewportWidth = Math.max(
        0,
        viewport.width - OVERLAY_VIEWPORT_GUTTER_PX * 2
      );
      const requestedWidth = preferredWidth ?? Math.max(surfaceRect.width, minWidth ?? 0);
      const next = resolveOverlayPosition({
        trigger: triggerRect,
        overlay: {
          width: clampOverlayWidth(requestedWidth, maxViewportWidth) ?? surfaceRect.width,
          height: surfaceRect.height
        },
        viewport,
        preferredPlacement: placement,
        offset: resolveOverlayOffset(offset, kind === "nested"),
        gutter: OVERLAY_VIEWPORT_GUTTER_PX
      });
      setPosition((current) => positionUnchanged(current, next) ? current : next);
    };
    const scheduleMeasure = () => {
      if (frame) cancelAnimationFrame(frame);
      frame = requestAnimationFrame(measure);
    };

    const resizeObserver = new ResizeObserver(scheduleMeasure);
    resizeObserver.observe(surface);
    if (typeof HTMLElement !== "undefined" && anchor instanceof HTMLElement) {
      resizeObserver.observe(anchor);
    }
    window.addEventListener("resize", scheduleMeasure);
    window.addEventListener("scroll", scheduleMeasure, true);
    scheduleMeasure();

    return () => {
      if (frame) cancelAnimationFrame(frame);
      resizeObserver.disconnect();
      window.removeEventListener("resize", scheduleMeasure);
      window.removeEventListener("scroll", scheduleMeasure, true);
    };
  }, [anchor, kind, minWidth, offset, open, placement, preferredWidth]);

  if (!presence.present || !renderedAnchor) return null;

  const fallbackMaxWidth = Math.max(0, window.innerWidth - OVERLAY_VIEWPORT_GUTTER_PX * 2);
  const fallbackMaxHeight = Math.max(0, window.innerHeight - OVERLAY_VIEWPORT_GUTTER_PX * 2);
  const maxWidth = position?.maxWidth ?? fallbackMaxWidth;
  const style: AnchoredOverlayStyle = {
    position: "fixed",
    left: position?.left ?? 0,
    top: position?.top ?? 0,
    maxWidth,
    maxHeight: position?.maxHeight ?? fallbackMaxHeight,
    minWidth: clampOverlayWidth(minWidth, maxWidth),
    overflowY: "auto",
    width: clampOverlayWidth(preferredWidth, maxWidth),
    visibility: position ? "visible" : "hidden"
  };

  return createPortal(
    <OverlaySurface
      aria-label={ariaLabel}
      aria-hidden={!open || undefined}
      className={className}
      data-overlay-layer-kind={kind}
      data-overlay-placement={position?.placement ?? placement}
      data-overlay-state={presence.phase}
      id={id}
      role={role}
      style={style}
      surfaceRole={resolvedSurfaceRole}
    >
      {children}
    </OverlaySurface>,
    document.body
  );
}
