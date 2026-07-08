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

const SCROLLBAR_THICKNESS_PX = 10;
const SCROLLBAR_MIN_THUMB_PX = 32;

export function useOverlayScrollbars() {
  useEffect(() => {
    const vertical = createScrollbar("vertical");
    const horizontal = createScrollbar("horizontal");
    let activeElement: HTMLElement | null = null;
    let dragging: DragState | null = null;
    let pointerX = Number.NaN;
    let pointerY = Number.NaN;
    let animationFrame = 0;

    document.body.append(vertical.rail, horizontal.rail);

    const cleanup = () => {
      if (animationFrame) {
        window.cancelAnimationFrame(animationFrame);
        animationFrame = 0;
      }
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

    function clearActiveElement() {
      if (dragging) return;
      setActiveElement(null);
    }

    function syncOverlay() {
      const element = activeElement;
      if (!element || !element.isConnected) {
        setOverlayVisible(vertical, false);
        setOverlayVisible(horizontal, false);
        return;
      }

      if (!dragging && !isPointInsideElement(element, pointerX, pointerY)) {
        clearActiveElement();
        return;
      }

      const rect = element.getBoundingClientRect();
      const canScrollVertical = element.scrollHeight > element.clientHeight + 1;
      const canScrollHorizontal = element.scrollWidth > element.clientWidth + 1;
      updateScrollbar(vertical, "vertical", element, rect, canScrollVertical);
      updateScrollbar(horizontal, "horizontal", element, rect, canScrollHorizontal);
    }

    function updateScrollbar(
      scrollbar: ScrollbarElements,
      axis: ScrollAxis,
      element: HTMLElement,
      rect: DOMRect,
      visible: boolean
    ) {
      setOverlayVisible(scrollbar, visible);
      scrollbar.rail.setAttribute(
        "data-scrollbar-dragging",
        dragging?.axis === axis ? "true" : "false"
      );
      if (!visible) return;

      if (axis === "vertical") {
        const inlineInset = readOverlayScrollbarInset(
          element,
          "--overlay-scrollbar-inline-inset"
        );
        const viewportSize = element.clientHeight;
        const scrollSize = element.scrollHeight;
        const railSize = Math.max(0, rect.height);
        const thumbSize = resolveThumbSize(viewportSize, scrollSize, railSize);
        const thumbTravel = Math.max(1, railSize - thumbSize);
        const scrollRange = Math.max(1, scrollSize - viewportSize);
        const thumbOffset = (element.scrollTop / scrollRange) * thumbTravel;

        scrollbar.rail.style.left = `${Math.round(
          rect.right - SCROLLBAR_THICKNESS_PX - inlineInset
        )}px`;
        scrollbar.rail.style.top = `${Math.round(rect.top)}px`;
        scrollbar.rail.style.width = `${SCROLLBAR_THICKNESS_PX}px`;
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
      scrollbar.rail.style.top = `${Math.round(rect.bottom - SCROLLBAR_THICKNESS_PX)}px`;
      scrollbar.rail.style.width = `${Math.round(rect.width)}px`;
      scrollbar.rail.style.height = `${SCROLLBAR_THICKNESS_PX}px`;
      scrollbar.thumb.style.width = `${Math.round(thumbSize)}px`;
      scrollbar.thumb.style.height = "";
      scrollbar.thumb.style.transform = `translate3d(${Math.round(thumbOffset)}px, 0, 0)`;
    }

    function handlePointerOver(event: PointerEvent) {
      pointerX = event.clientX;
      pointerY = event.clientY;
      setActiveElement(findScrollableElementAtPoint(pointerX, pointerY));
    }

    function handlePointerMove(event: PointerEvent) {
      pointerX = event.clientX;
      pointerY = event.clientY;
      if (dragging && activeElement) {
        const currentPointer = dragging.axis === "vertical" ? event.clientY : event.clientX;
        const pointerDelta = currentPointer - dragging.pointerStart;
        const scrollDelta = (pointerDelta / dragging.thumbTravel) * dragging.scrollRange;
        if (dragging.axis === "vertical") {
          activeElement.scrollTop = dragging.scrollStart + scrollDelta;
        } else {
          activeElement.scrollLeft = dragging.scrollStart + scrollDelta;
        }
        scheduleOverlaySync();
        return;
      }

      // Fast path: while the pointer stays inside the currently-active
      // scrollable, the answer can't change — skip the expensive hit-test
      // (document.elementsFromPoint + a getComputedStyle walk), which otherwise
      // ran on EVERY pointer-move and scaled with DOM size, contributing to the
      // main-thread jank while the media grid mutates during thumbnail
      // generation. `isPointInsideElement` is a cheap rect check.
      if (activeElement && isPointInsideElement(activeElement, pointerX, pointerY)) {
        return;
      }

      const scrollable = findScrollableElementAtPoint(pointerX, pointerY);
      if (scrollable) {
        setActiveElement(scrollable);
      } else if (activeElement && !isPointInsideElement(activeElement, pointerX, pointerY)) {
        clearActiveElement();
      }
    }

    function handlePointerLeave() {
      if (!dragging) clearActiveElement();
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
      if (!activeElement || !isPointInsideElement(activeElement, pointerX, pointerY)) {
        clearActiveElement();
      } else {
        scheduleOverlaySync();
      }
    }

    function handleScroll(event: Event) {
      const target = event.target;
      if (target instanceof HTMLElement && isScrollableElement(target)) {
        setActiveElement(target);
      }
    }

    function handleResize() {
      scheduleOverlaySync();
    }

    vertical.thumb.addEventListener("pointerdown", handlePointerDown);
    horizontal.thumb.addEventListener("pointerdown", handlePointerDown);
    vertical.thumb.addEventListener("pointerenter", () => {
      vertical.rail.dataset.scrollbarHover = "true";
    });
    horizontal.thumb.addEventListener("pointerenter", () => {
      horizontal.rail.dataset.scrollbarHover = "true";
    });
    vertical.thumb.addEventListener("pointerleave", () => {
      vertical.rail.dataset.scrollbarHover = "false";
    });
    horizontal.thumb.addEventListener("pointerleave", () => {
      horizontal.rail.dataset.scrollbarHover = "false";
    });
    document.addEventListener("pointerover", handlePointerOver, { passive: true });
    document.addEventListener("pointermove", handlePointerMove, { passive: !dragging });
    document.addEventListener("pointerleave", handlePointerLeave);
    document.addEventListener("pointerup", handlePointerUp);
    document.addEventListener("scroll", handleScroll, true);
    window.addEventListener("resize", handleResize, { passive: true });

    return () => {
      activeElement?.removeEventListener("scroll", scheduleOverlaySync);
      vertical.thumb.removeEventListener("pointerdown", handlePointerDown);
      horizontal.thumb.removeEventListener("pointerdown", handlePointerDown);
      document.removeEventListener("pointerover", handlePointerOver);
      document.removeEventListener("pointermove", handlePointerMove);
      document.removeEventListener("pointerleave", handlePointerLeave);
      document.removeEventListener("pointerup", handlePointerUp);
      document.removeEventListener("scroll", handleScroll, true);
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

function findScrollableElementAtPoint(x: number, y: number): HTMLElement | null {
  if (!Number.isFinite(x) || !Number.isFinite(y)) return null;
  for (const element of document.elementsFromPoint(x, y)) {
    let current: Element | null = element;
    while (current && current !== document.body && current !== document.documentElement) {
      if (current instanceof HTMLElement && isScrollableElement(current)) {
        return current;
      }
      current = current.parentElement;
    }
  }
  return null;
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

function isPointInsideElement(element: HTMLElement, x: number, y: number) {
  if (!Number.isFinite(x) || !Number.isFinite(y)) return false;
  const rect = element.getBoundingClientRect();
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}
