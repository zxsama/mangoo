import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
  type ReactPortal,
  type RefObject
} from "react";
import { createPortal } from "react-dom";
import { useOverlayStack } from "./OverlayProvider";
import { OverlaySurface } from "./OverlaySurface";
import { measureOverlayIntrinsicWidth } from "./overlayMeasurement";
import {
  resolveAnchoredOverlayLayout,
  resolveOverlayAnchorOffset,
  type AnchoredOverlayLayoutResult
} from "./overlayPosition";
import {
  OVERLAY_VIEWPORT_GUTTER_PX,
  type AnchoredOverlayStyle,
  type OverlayAnchor,
  type OverlayPlacement,
  type OverlayRootId
} from "./overlayTypes";
import { useOverlayPresence } from "./useOverlayPresence";

export interface AnchoredOverlayProps {
  anchor: OverlayAnchor | null;
  ariaLabel: string;
  autoFocus?: boolean;
  children: ReactNode;
  className?: string;
  id: string;
  initialFocusRef?: RefObject<HTMLElement | null>;
  minWidth?: number;
  onDismiss: () => void;
  open: boolean;
  parentId?: string;
  placement?: OverlayPlacement;
  preferredWidth?: number;
  role?: "menu" | "listbox" | "dialog" | "group" | "status";
  restoreFocusTarget?: HTMLElement | null;
  rootId: OverlayRootId;
  surfaceRole?: "popover" | "nested";
}

function layoutUnchanged(
  current: AnchoredOverlayLayoutResult | null,
  next: AnchoredOverlayLayoutResult
): boolean {
  return Boolean(
    current &&
    current.left === next.left &&
    current.top === next.top &&
    current.maxWidth === next.maxWidth &&
    current.maxHeight === next.maxHeight &&
    current.placement === next.placement &&
    current.width === next.width
  );
}

export function AnchoredOverlay({
  anchor,
  ariaLabel,
  autoFocus = false,
  children,
  className,
  id,
  initialFocusRef,
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
  const autoFocusCompletedRef = useRef(false);
  const surfaceRef = useRef<HTMLElement | null>(null);
  const retainedAnchorRef = useRef<OverlayAnchor | null>(anchor);
  const [layout, setLayout] = useState<AnchoredOverlayLayoutResult | null>(null);
  const { openRoot, registerLayer } = useOverlayStack();
  const resolvedSurfaceRole = surfaceRole ?? (parentId ? "nested" : "popover");
  const kind = resolvedSurfaceRole === "nested" ? "nested" : "root";
  const presence = useOverlayPresence(open, kind);
  if (anchor) retainedAnchorRef.current = anchor;
  const renderedAnchor = anchor ?? retainedAnchorRef.current;

  useLayoutEffect(() => {
    surfaceRef.current = presence.present ? document.getElementById(id) : null;
    if (!presence.present) {
      autoFocusCompletedRef.current = false;
      setLayout(null);
      retainedAnchorRef.current = null;
    }
    return () => {
      surfaceRef.current = null;
    };
  }, [id, presence.present]);

  useLayoutEffect(() => {
    if (!open || !presence.present || !autoFocus) {
      autoFocusCompletedRef.current = false;
      return;
    }
    if (!layout || autoFocusCompletedRef.current) return;
    const surface = surfaceRef.current;
    const target = initialFocusRef?.current;
    if (!surface || !target || !surface.contains(target)) return;
    autoFocusCompletedRef.current = true;
    target.focus();
  }, [autoFocus, initialFocusRef, layout, open, presence.present]);

  useEffect(() => {
    if (!open || !presence.present || autoFocus || !initialFocusRef) return;
    const trigger =
      typeof HTMLElement !== "undefined" && anchor instanceof HTMLElement
        ? anchor
        : null;
    if (!trigger) return;

    const handOffTriggerTab = (event: KeyboardEvent) => {
      if (
        event.key !== "Tab" ||
        event.shiftKey ||
        event.defaultPrevented ||
        document.activeElement !== trigger
      ) {
        return;
      }
      const surface = surfaceRef.current;
      const target = initialFocusRef.current;
      if (!surface || !target || !surface.contains(target)) return;
      event.preventDefault();
      target.focus();
    };

    trigger.addEventListener("keydown", handOffTriggerTab);
    return () => trigger.removeEventListener("keydown", handOffTriggerTab);
  }, [anchor, autoFocus, initialFocusRef, open, presence.present]);

  useLayoutEffect(() => {
    if (!open || !presence.present || !anchor) return;
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
    presence.present,
    registerLayer,
    restoreFocusTarget,
    rootId
  ]);

  useEffect(() => {
    if (!open || !presence.present || !anchor) return;
    const surface = surfaceRef.current;
    if (!surface) return;

    let frame = 0;
    const measure = () => {
      frame = 0;
      const triggerRect = anchor.getBoundingClientRect();
      const intrinsicWidth = measureOverlayIntrinsicWidth(surface);
      const surfaceRect = surface.getBoundingClientRect();
      const viewport = { width: window.innerWidth, height: window.innerHeight };
      const viewportMaxWidth = Math.max(
        0,
        viewport.width - OVERLAY_VIEWPORT_GUTTER_PX * 2
      );
      const next = resolveAnchoredOverlayLayout({
        trigger: triggerRect,
        overlay: { measuredWidth: intrinsicWidth, height: surfaceRect.height },
        viewport,
        preferredPlacement: placement,
        preferredWidth,
        minWidth,
        maxWidth: viewportMaxWidth,
        offset: resolveOverlayAnchorOffset(anchor, kind === "nested"),
        gutter: OVERLAY_VIEWPORT_GUTTER_PX
      });
      setLayout((current) => layoutUnchanged(current, next) ? current : next);
    };
    const scheduleMeasure = () => {
      if (frame) cancelAnimationFrame(frame);
      frame = requestAnimationFrame(measure);
    };
    const handleScroll = (event: Event) => {
      if (event.target instanceof Node && surface.contains(event.target)) return;
      scheduleMeasure();
    };

    const resizeObserver = new ResizeObserver(scheduleMeasure);
    resizeObserver.observe(surface);
    if (typeof HTMLElement !== "undefined" && anchor instanceof HTMLElement) {
      resizeObserver.observe(anchor);
    }
    window.addEventListener("resize", scheduleMeasure);
    window.addEventListener("scroll", handleScroll, true);
    scheduleMeasure();

    return () => {
      if (frame) cancelAnimationFrame(frame);
      resizeObserver.disconnect();
      window.removeEventListener("resize", scheduleMeasure);
      window.removeEventListener("scroll", handleScroll, true);
    };
  }, [anchor, kind, minWidth, open, placement, preferredWidth, presence.present]);

  if (!presence.present || !renderedAnchor) return null;

  const fallbackMaxWidth = Math.max(0, window.innerWidth - OVERLAY_VIEWPORT_GUTTER_PX * 2);
  const fallbackMaxHeight = Math.max(0, window.innerHeight - OVERLAY_VIEWPORT_GUTTER_PX * 2);
  const style: AnchoredOverlayStyle = {
    position: "fixed",
    left: layout?.left ?? 0,
    top: layout?.top ?? 0,
    maxWidth: layout?.maxWidth ?? fallbackMaxWidth,
    maxHeight: layout?.maxHeight ?? fallbackMaxHeight,
    width: layout?.width,
    visibility: layout ? "visible" : "hidden"
  };

  return createPortal(
    <OverlaySurface
      aria-label={ariaLabel}
      aria-hidden={!open || undefined}
      className={className}
      data-overlay-layer-kind={kind}
      data-overlay-placement={layout?.placement ?? placement}
      data-overlay-state={presence.phase}
      id={id}
      role={role}
      style={style}
      surfaceRole={resolvedSurfaceRole}
    >
      <div
        className="overlay-scroll-viewport"
        data-overlay-scroll-viewport="true"
        style={{ maxHeight: layout?.maxHeight ?? fallbackMaxHeight }}
      >
        {children}
      </div>
    </OverlaySurface>,
    document.body
  );
}
