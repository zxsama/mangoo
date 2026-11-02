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
import { useOverlayFocusTrap } from "./useOverlayFocusTrap";

export interface AnchoredOverlayProps {
  anchor: OverlayAnchor | null;
  ariaLabel: string;
  autoFocus?: boolean;
  busy?: boolean;
  isDismissLocked?: () => boolean;
  trapFocus?: boolean;
  children: ReactNode;
  className?: string;
  id: string;
  initialFocusRef?: RefObject<HTMLElement | null>;
  integratedHeader?: ReactNode;
  interactivePointerOwner?: boolean;
  minWidth?: number;
  onDismiss: () => void;
  onPresenceChange?: (present: boolean) => void;
  open: boolean;
  parentId?: string;
  placement?: OverlayPlacement;
  preferredWidth?: number;
  role?: "menu" | "listbox" | "dialog" | "group" | "status";
  restoreFocusTarget?: HTMLElement | null;
  restoreFocus?: boolean;
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
    && current.anchorHeight === next.anchorHeight
  );
}

export function AnchoredOverlay({
  anchor,
  ariaLabel,
  autoFocus = false,
  busy = false,
  isDismissLocked,
  trapFocus = false,
  children,
  className,
  id,
  initialFocusRef,
  integratedHeader,
  interactivePointerOwner = false,
  minWidth,
  onDismiss,
  onPresenceChange,
  open,
  parentId,
  placement = "bottom-start",
  preferredWidth,
  role = "menu",
  restoreFocusTarget,
  restoreFocus = true,
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
  const integratedAnchor = integratedHeader !== undefined;

  useLayoutEffect(() => {
    // The integrated header replaces the trigger. Keep that trigger painted
    // until the portal has a measured, visible surface to take its place.
    onPresenceChange?.(presence.present && (!integratedAnchor || layout !== null));
  }, [integratedAnchor, layout, onPresenceChange, presence.present]);

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

  useOverlayFocusTrap(open && presence.present && layout !== null && trapFocus, surfaceRef, { initialFocusRef });

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
      dismissOnEscape: !busy,
      dismissOnFocusOutside: !busy,
      dismissOnOutsidePointer: !busy,
      dismissOnWindowBlur: !busy,
      isDismissLocked,
      restoreFocus: restoreFocus && trigger !== null,
      onDismiss
    });
  }, [
    anchor,
    busy,
    isDismissLocked,
    id,
    kind,
    onDismiss,
    open,
    openRoot,
    parentId,
    presence.present,
    registerLayer,
    restoreFocus,
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
      const optionsViewport = surface.querySelector<HTMLElement>(".overlay-scroll-viewport");
      const measuredHeight = integratedAnchor && optionsViewport
        ? triggerRect.height + Math.min(320, optionsViewport.scrollHeight)
        : surfaceRect.height;
      const viewport = { width: window.innerWidth, height: window.innerHeight };
      const viewportMaxWidth = Math.max(
        0,
        viewport.width - OVERLAY_VIEWPORT_GUTTER_PX * 2
      );
      const next = resolveAnchoredOverlayLayout({
        trigger: triggerRect,
        overlay: { measuredWidth: intrinsicWidth, height: measuredHeight },
        integratedAnchor,
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
    if (integratedAnchor) {
      const content = surface.querySelector(".overlay-scroll-viewport")?.firstElementChild;
      if (content) resizeObserver.observe(content);
    }
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
  }, [anchor, integratedAnchor, kind, minWidth, open, placement, preferredWidth, presence.present]);

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
    "--overlay-anchor-height": `${layout?.anchorHeight ?? 0}px`,
    visibility: layout ? "visible" : "hidden"
  };

  return createPortal(
    <OverlaySurface
      aria-label={ariaLabel}
      aria-busy={busy || undefined}
      aria-hidden={!open || undefined}
      className={className}
      data-overlay-layer-kind={kind}
      data-overlay-placement={layout?.placement ?? placement}
      data-overlay-state={presence.phase}
      data-interactive-pointer-owner={interactivePointerOwner ? "true" : undefined}
      id={id}
      interactive={interactivePointerOwner}
      role={role}
      style={style}
      surfaceRole={resolvedSurfaceRole}
    >
      {integratedHeader}
      <div
        className="overlay-scroll-viewport"
        data-overlay-scroll-viewport="true"
        style={{ maxHeight: integratedAnchor
          ? Math.min(320, Math.max(0, (layout?.maxHeight ?? fallbackMaxHeight) - (layout?.anchorHeight ?? 0)))
          : layout?.maxHeight ?? fallbackMaxHeight }}
      >
        {children}
      </div>
    </OverlaySurface>,
    document.body
  );
}
