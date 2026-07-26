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
import { resolveOverlayPosition } from "./overlayPosition";
import {
  NESTED_OVERLAY_OFFSET_PX,
  OVERLAY_VIEWPORT_GUTTER_PX,
  ROOT_OVERLAY_OFFSET_PX,
  type AnchoredOverlayStyle,
  type OverlayAnchor,
  type OverlayPlacement,
  type OverlayPositionResult,
  type OverlayRootId
} from "./overlayTypes";

export interface AnchoredOverlayProps {
  anchor: OverlayAnchor | null;
  ariaLabel: string;
  children: ReactNode;
  className?: string;
  id: string;
  minWidth?: number;
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
  const [position, setPosition] = useState<OverlayPositionResult | null>(null);
  const { openRoot, registerLayer } = useOverlayStack();
  const resolvedSurfaceRole = surfaceRole ?? (parentId ? "nested" : "popover");
  const kind = resolvedSurfaceRole === "nested" ? "nested" : "root";

  useLayoutEffect(() => {
    surfaceRef.current = open ? document.getElementById(id) : null;
    if (!open) setPosition(null);
    return () => {
      surfaceRef.current = null;
    };
  }, [id, open]);

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
      const next = resolveOverlayPosition({
        trigger: triggerRect,
        overlay: {
          width: preferredWidth ?? Math.max(surfaceRect.width, minWidth ?? 0),
          height: surfaceRect.height
        },
        viewport: { width: window.innerWidth, height: window.innerHeight },
        preferredPlacement: placement,
        offset: kind === "nested" ? NESTED_OVERLAY_OFFSET_PX : ROOT_OVERLAY_OFFSET_PX,
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
  }, [anchor, kind, minWidth, open, placement, preferredWidth]);

  if (!open || !anchor) return null;

  const fallbackMaxWidth = Math.max(0, window.innerWidth - OVERLAY_VIEWPORT_GUTTER_PX * 2);
  const fallbackMaxHeight = Math.max(0, window.innerHeight - OVERLAY_VIEWPORT_GUTTER_PX * 2);
  const style: AnchoredOverlayStyle = {
    position: "fixed",
    left: position?.left ?? 0,
    top: position?.top ?? 0,
    maxWidth: position?.maxWidth ?? fallbackMaxWidth,
    maxHeight: position?.maxHeight ?? fallbackMaxHeight,
    minWidth,
    overflowY: "auto",
    width: preferredWidth,
    visibility: position ? "visible" : "hidden"
  };

  return createPortal(
    <OverlaySurface
      aria-label={ariaLabel}
      className={className}
      data-overlay-layer-kind={kind}
      data-overlay-placement={position?.placement ?? placement}
      data-overlay-state="opening"
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
