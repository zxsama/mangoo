import { useEffect } from "react";

type ScrollAxis = "vertical" | "horizontal";

interface ScrollbarElements {
  rail: HTMLDivElement;
  thumb: HTMLDivElement;
}

interface DragState {
  axis: ScrollAxis;
  pointerStart: number;
  scrollStart: number;
  scrollRange: number;
  thumbTravel: number;
}

const SCROLLBAR_RAIL_SIZE_PX = 10;
const SCROLLBAR_MIN_THUMB_PX = 32;
const SCROLLBAR_SCROLL_IDLE_MS = 180;

export function useOverlayScrollbars() {
  useEffect(() => {
    const vertical = createScrollbar("vertical");
    const horizontal = createScrollbar("horizontal");
    let activeElement: HTMLElement | null = null;
    let dragging: DragState | null = null;
    let animationFrame = 0;
    let scrollIdleFrame = 0;
    let scrollActiveUntil = 0;
    let visibilityObserver: MutationObserver | null = null;

    document.body.append(vertical.rail, horizontal.rail);

    const cleanup = () => {
      if (animationFrame) {
        window.cancelAnimationFrame(animationFrame);
        animationFrame = 0;
      }
      if (scrollIdleFrame) {
        window.cancelAnimationFrame(scrollIdleFrame);
        scrollIdleFrame = 0;
      }
      visibilityObserver?.disconnect();
      vertical.rail.remove();
      horizontal.rail.remove();
    };

    function scheduleOverlaySync() {
      if (animationFrame) return;
      animationFrame = window.requestAnimationFrame(() => {
        animationFrame = 0;
        syncOverlay();
      });
    }

    function setActiveElement(element: HTMLElement | null) {
      if (element === activeElement) {
        scheduleOverlaySync();
        return;
      }
      activeElement?.removeEventListener("scroll", scheduleOverlaySync);
      activeElement = element;
      activeElement?.addEventListener("scroll", scheduleOverlaySync, { passive: true });
      scheduleOverlaySync();
    }

    function scheduleScrollIdleCheck() {
      if (scrollIdleFrame) return;
      const tickScrollActivity = (now: number) => {
        scrollIdleFrame = 0;
        scheduleOverlaySync();
        if (now < scrollActiveUntil) {
          scrollIdleFrame = window.requestAnimationFrame(tickScrollActivity);
        }
      };
      scrollIdleFrame = window.requestAnimationFrame(tickScrollActivity);
    }

    function markScrollActivity() {
      scrollActiveUntil = performance.now() + SCROLLBAR_SCROLL_IDLE_MS;
      scheduleScrollIdleCheck();
    }

    function syncOverlay() {
      if (document.body.dataset.shellResizing === "true") {
        setOverlayVisible(vertical, false);
        setOverlayVisible(horizontal, false);
        return;
      }
      const element = activeElement;
      if (!element || !element.isConnected) {
        setOverlayVisible(vertical, false);
        setOverlayVisible(horizontal, false);
        return;
      }
      const elementStyle = getComputedStyle(element);
      if (
        elementStyle.visibility === "hidden" ||
        elementStyle.display === "none" ||
        element.getClientRects().length === 0
      ) {
        setOverlayVisible(vertical, false);
        setOverlayVisible(horizontal, false);
        return;
      }

      const scrollActive = performance.now() < scrollActiveUntil;
      const scrollbarHovered =
        vertical.rail.dataset.scrollbarHover === "true" ||
        horizontal.rail.dataset.scrollbarHover === "true" ||
        vertical.rail.dataset.scrollbarAreaHover === "true" ||
        horizontal.rail.dataset.scrollbarAreaHover === "true";
      const overlayVisible = Boolean(dragging) || scrollActive || scrollbarHovered;

      const rect = element.getBoundingClientRect();
      const canScrollVertical = element.scrollHeight > element.clientHeight + 1;
      const canScrollHorizontal = element.scrollWidth > element.clientWidth + 1;
      updateScrollbar(vertical, "vertical", element, rect, canScrollVertical, overlayVisible);
      updateScrollbar(horizontal, "horizontal", element, rect, canScrollHorizontal, overlayVisible);
    }

    function updateScrollbar(
      scrollbar: ScrollbarElements,
      axis: ScrollAxis,
      element: HTMLElement,
      rect: DOMRect,
      canScroll: boolean,
      overlayVisible: boolean
    ) {
      setOverlayVisible(scrollbar, canScroll && overlayVisible);
      scrollbar.rail.setAttribute(
        "data-scrollbar-dragging",
        dragging?.axis === axis ? "true" : "false"
      );
      if (!canScroll) return;

      if (axis === "vertical") {
        const inlineInset = readOverlayScrollbarInset(element, "--scrollbar-edge-inset");
        const viewportSize = element.clientHeight;
        const scrollSize = element.scrollHeight;
        const railSize = Math.max(0, rect.height);
        const thumbSize = resolveThumbSize(viewportSize, scrollSize, railSize);
        const thumbTravel = Math.max(1, railSize - thumbSize);
        const scrollRange = Math.max(1, scrollSize - viewportSize);
        const thumbOffset = (element.scrollTop / scrollRange) * thumbTravel;

        scrollbar.rail.style.left = `${Math.round(rect.right - SCROLLBAR_RAIL_SIZE_PX - inlineInset)}px`;
        scrollbar.rail.style.top = `${Math.round(rect.top)}px`;
        scrollbar.rail.style.width = `${SCROLLBAR_RAIL_SIZE_PX}px`;
        scrollbar.rail.style.height = `${Math.round(rect.height)}px`;
        scrollbar.thumb.style.width = "";
        scrollbar.thumb.style.height = `${Math.round(thumbSize)}px`;
        scrollbar.thumb.style.transform = `translate3d(0, ${Math.round(thumbOffset)}px, 0)`;
        return;
      }

      const viewportSize = element.clientWidth;
      const scrollSize = element.scrollWidth;
      const railSize = Math.max(0, rect.width);
      const thumbSize = resolveThumbSize(viewportSize, scrollSize, railSize);
      const thumbTravel = Math.max(1, railSize - thumbSize);
      const scrollRange = Math.max(1, scrollSize - viewportSize);
      const thumbOffset = (element.scrollLeft / scrollRange) * thumbTravel;

      scrollbar.rail.style.left = `${Math.round(rect.left)}px`;
      const inlineInset = readOverlayScrollbarInset(element, "--scrollbar-edge-inset");
      scrollbar.rail.style.top = `${Math.round(rect.bottom - SCROLLBAR_RAIL_SIZE_PX - inlineInset)}px`;
      scrollbar.rail.style.width = `${Math.round(rect.width)}px`;
      scrollbar.rail.style.height = `${SCROLLBAR_RAIL_SIZE_PX}px`;
      scrollbar.thumb.style.width = `${Math.round(thumbSize)}px`;
      scrollbar.thumb.style.height = "";
      scrollbar.thumb.style.transform = `translate3d(${Math.round(thumbOffset)}px, 0, 0)`;
    }

    function handlePointerMove(event: PointerEvent) {
      if (!dragging || !activeElement) return;
      const currentPointer = dragging.axis === "vertical" ? event.clientY : event.clientX;
      const pointerDelta = currentPointer - dragging.pointerStart;
      const scrollDelta = (pointerDelta / dragging.thumbTravel) * dragging.scrollRange;
      if (dragging.axis === "vertical") {
        activeElement.scrollTop = dragging.scrollStart + scrollDelta;
      } else {
        activeElement.scrollLeft = dragging.scrollStart + scrollDelta;
      }
      scheduleOverlaySync();
    }

    function handlePointerDown(event: PointerEvent) {
      const axis = resolveScrollbarAxis(event.target);
      if (!axis || !activeElement) return;

      event.preventDefault();
      const rail = axis === "vertical" ? vertical.rail : horizontal.rail;
      const thumb = axis === "vertical" ? vertical.thumb : horizontal.thumb;
      const railRect = rail.getBoundingClientRect();
      const thumbRect = thumb.getBoundingClientRect();
      const railSize = axis === "vertical" ? railRect.height : railRect.width;
      const thumbSize = axis === "vertical" ? thumbRect.height : thumbRect.width;
      const viewportSize = axis === "vertical" ? activeElement.clientHeight : activeElement.clientWidth;
      const scrollSize = axis === "vertical" ? activeElement.scrollHeight : activeElement.scrollWidth;

      dragging = {
        axis,
        pointerStart: axis === "vertical" ? event.clientY : event.clientX,
        scrollStart: axis === "vertical" ? activeElement.scrollTop : activeElement.scrollLeft,
        scrollRange: Math.max(1, scrollSize - viewportSize),
        thumbTravel: Math.max(1, railSize - thumbSize)
      };
      rail.setAttribute("data-scrollbar-dragging", "true");
      event.currentTarget instanceof Element && event.currentTarget.setPointerCapture?.(event.pointerId);
      scheduleOverlaySync();
    }

    function handlePointerUp(event: PointerEvent) {
      if (!dragging) return;
      const axis = dragging.axis;
      dragging = null;
      const rail = axis === "vertical" ? vertical.rail : horizontal.rail;
      rail.setAttribute("data-scrollbar-dragging", "false");
      event.currentTarget instanceof Element && event.currentTarget.releasePointerCapture?.(event.pointerId);
      markScrollActivity();
      scheduleOverlaySync();
    }

    function handleScroll(event: Event) {
      const target = event.target;
      if (target instanceof HTMLElement && isScrollableElement(target)) {
        setActiveElement(target);
        markScrollActivity();
      }
    }

    function handleShellResizeState() {
      if (document.body.dataset.shellResizing === "true") {
        scrollActiveUntil = 0;
        setActiveElement(null);
        setOverlayVisible(vertical, false);
        setOverlayVisible(horizontal, false);
        return;
      }
      scheduleOverlaySync();
    }

    function handleResize() {
      scheduleOverlaySync();
    }

    function handleVerticalThumbEnter() {
      vertical.rail.dataset.scrollbarHover = "true";
      scheduleOverlaySync();
    }

    function handleHorizontalThumbEnter() {
      horizontal.rail.dataset.scrollbarHover = "true";
      scheduleOverlaySync();
    }

    function handleVerticalThumbLeave() {
      vertical.rail.dataset.scrollbarHover = "false";
      scheduleOverlaySync();
    }

    function handleHorizontalThumbLeave() {
      horizontal.rail.dataset.scrollbarHover = "false";
      scheduleOverlaySync();
    }

    function handleVerticalRailEnter() {
      vertical.rail.dataset.scrollbarAreaHover = "true";
      scheduleOverlaySync();
    }

    function handleHorizontalRailEnter() {
      horizontal.rail.dataset.scrollbarAreaHover = "true";
      scheduleOverlaySync();
    }

    function handleVerticalRailLeave() {
      vertical.rail.dataset.scrollbarAreaHover = "false";
      scheduleOverlaySync();
    }

    function handleHorizontalRailLeave() {
      horizontal.rail.dataset.scrollbarAreaHover = "false";
      scheduleOverlaySync();
    }

    vertical.thumb.addEventListener("pointerdown", handlePointerDown);
    horizontal.thumb.addEventListener("pointerdown", handlePointerDown);
    vertical.thumb.addEventListener("pointerenter", handleVerticalThumbEnter);
    horizontal.thumb.addEventListener("pointerenter", handleHorizontalThumbEnter);
    vertical.thumb.addEventListener("pointerleave", handleVerticalThumbLeave);
    horizontal.thumb.addEventListener("pointerleave", handleHorizontalThumbLeave);
    vertical.rail.addEventListener("pointerenter", handleVerticalRailEnter);
    horizontal.rail.addEventListener("pointerenter", handleHorizontalRailEnter);
    vertical.rail.addEventListener("pointerleave", handleVerticalRailLeave);
    horizontal.rail.addEventListener("pointerleave", handleHorizontalRailLeave);

    if (typeof MutationObserver !== "undefined") {
      visibilityObserver = new MutationObserver(() => {
        scheduleOverlaySync();
      });
      visibilityObserver.observe(document.body, {
        attributes: true,
        attributeFilter: ["class", "style", "hidden"],
        subtree: true
      });
    }

    document.addEventListener("pointermove", handlePointerMove, { passive: true });
    document.addEventListener("pointerup", handlePointerUp);
    document.addEventListener("scroll", handleScroll, true);
    window.addEventListener("megle:shell-resize-state", handleShellResizeState);
    window.addEventListener("resize", handleResize, { passive: true });

    return () => {
      activeElement?.removeEventListener("scroll", scheduleOverlaySync);
      vertical.thumb.removeEventListener("pointerdown", handlePointerDown);
      horizontal.thumb.removeEventListener("pointerdown", handlePointerDown);
      vertical.thumb.removeEventListener("pointerenter", handleVerticalThumbEnter);
      horizontal.thumb.removeEventListener("pointerenter", handleHorizontalThumbEnter);
      vertical.thumb.removeEventListener("pointerleave", handleVerticalThumbLeave);
      horizontal.thumb.removeEventListener("pointerleave", handleHorizontalThumbLeave);
      vertical.rail.removeEventListener("pointerenter", handleVerticalRailEnter);
      horizontal.rail.removeEventListener("pointerenter", handleHorizontalRailEnter);
      vertical.rail.removeEventListener("pointerleave", handleVerticalRailLeave);
      horizontal.rail.removeEventListener("pointerleave", handleHorizontalRailLeave);
      document.removeEventListener("pointermove", handlePointerMove);
      document.removeEventListener("pointerup", handlePointerUp);
      document.removeEventListener("scroll", handleScroll, true);
      window.removeEventListener("megle:shell-resize-state", handleShellResizeState);
      window.removeEventListener("resize", handleResize);
      cleanup();
    };
  }, []);
}

function readOverlayScrollbarInset(element: HTMLElement, propertyName: string) {
  const value = window.getComputedStyle(element).getPropertyValue(propertyName);
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? Math.max(0, parsed) : 0;
}

function createScrollbar(axis: ScrollAxis): ScrollbarElements {
  const rail = document.createElement("div");
  const thumb = document.createElement("div");
  rail.className = `megle-overlay-scrollbar megle-overlay-scrollbar-${axis}`;
  rail.setAttribute("data-scrollbar-visible", "false");
  rail.setAttribute("data-scrollbar-hover", "false");
  rail.setAttribute("data-scrollbar-area-hover", "false");
  rail.setAttribute("data-scrollbar-dragging", "false");
  thumb.className = "megle-overlay-scrollbar-thumb";
  thumb.dataset.scrollbarAxis = axis;
  rail.append(thumb);
  return { rail, thumb };
}

function setOverlayVisible(scrollbar: ScrollbarElements, visible: boolean) {
  scrollbar.rail.setAttribute("data-scrollbar-visible", visible ? "true" : "false");
  if (!visible) {
    scrollbar.rail.setAttribute("data-scrollbar-hover", "false");
    scrollbar.rail.setAttribute("data-scrollbar-area-hover", "false");
  }
}

function resolveThumbSize(viewportSize: number, scrollSize: number, railSize: number) {
  if (scrollSize <= 0 || railSize <= 0) return 0;
  return Math.max(SCROLLBAR_MIN_THUMB_PX, (viewportSize / scrollSize) * railSize);
}

function resolveScrollbarAxis(target: EventTarget | null): ScrollAxis | null {
  if (!(target instanceof HTMLElement)) return null;
  const axis = target.dataset.scrollbarAxis;
  return axis === "vertical" || axis === "horizontal" ? axis : null;
}

function isScrollableElement(element: HTMLElement) {
  const style = window.getComputedStyle(element);
  const canScrollY =
    element.scrollHeight > element.clientHeight + 1 &&
    allowsScroll(style.overflowY);
  const canScrollX =
    element.scrollWidth > element.clientWidth + 1 &&
    allowsScroll(style.overflowX);
  return canScrollY || canScrollX;
}

function allowsScroll(value: string) {
  return value === "auto" || value === "scroll" || value === "overlay";
}
