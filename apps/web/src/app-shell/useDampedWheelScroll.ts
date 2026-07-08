import { useEffect, type RefObject } from "react";
import { DampedMotionAxis, normalizeWheelDelta } from "../core/dampedMotion";

type DampedScrollState = {
  frame: number;
  x: DampedMotionAxis;
  y: DampedMotionAxis;
};

const SCROLL_FRICTION = 0.84;
const LINE_HEIGHT_PX = 18;

export function useDampedWheelScroll(rootRef: RefObject<HTMLElement | null>): void {
  useEffect(() => {
    const rootElement = rootRef.current;
    if (!rootElement) return;
    const root = rootElement;

    const states = new WeakMap<HTMLElement, DampedScrollState>();
    const activeStates = new Set<DampedScrollState>();

    function onWheel(event: WheelEvent) {
      if (event.defaultPrevented || event.ctrlKey || event.metaKey) {
        return;
      }
      const start = event.target instanceof Element ? event.target : null;
      if (!start || shouldUseNativeWheel(start)) {
        return;
      }

      const deltaX = normalizeWheelDelta(
        event.deltaX,
        event.deltaMode,
        LINE_HEIGHT_PX,
        root.clientWidth
      );
      const deltaY = normalizeWheelDelta(
        event.deltaY,
        event.deltaMode,
        LINE_HEIGHT_PX,
        root.clientHeight
      );
      if (deltaX === 0 && deltaY === 0) {
        return;
      }

      const target = findScrollableWheelTarget(start, root, deltaX, deltaY);
      if (!target) {
        return;
      }

      event.preventDefault();
      const state = stateForElement(states, activeStates, target);
      if (canScrollAxis(target, "x", deltaX)) {
        state.x.push(deltaX);
      }
      if (canScrollAxis(target, "y", deltaY)) {
        state.y.push(deltaY);
      }
      if (!state.frame) {
        state.frame = window.requestAnimationFrame(() => stepScroll(target, state));
      }
    }

    function stepScroll(element: HTMLElement, state: DampedScrollState) {
      state.frame = 0;
      let moved = false;
      const xStep = state.x.step();
      const yStep = state.y.step();

      if (xStep !== 0) {
        const before = element.scrollLeft;
        element.scrollLeft = clampScrollPosition(before + xStep, element.scrollWidth, element.clientWidth);
        moved = moved || element.scrollLeft !== before;
        if (element.scrollLeft === before && !canScrollAxis(element, "x", xStep)) {
          state.x.stop();
        }
      }

      if (yStep !== 0) {
        const before = element.scrollTop;
        element.scrollTop = clampScrollPosition(before + yStep, element.scrollHeight, element.clientHeight);
        moved = moved || element.scrollTop !== before;
        if (element.scrollTop === before && !canScrollAxis(element, "y", yStep)) {
          state.y.stop();
        }
      }

      if (moved && (state.x.active || state.y.active)) {
        state.frame = window.requestAnimationFrame(() => stepScroll(element, state));
      }
    }

    root.addEventListener("wheel", onWheel, { capture: true, passive: false });
    return () => {
      root.removeEventListener("wheel", onWheel, { capture: true });
      activeStates.forEach((state) => {
        if (state.frame) {
          window.cancelAnimationFrame(state.frame);
        }
      });
    };
  }, [rootRef]);
}

function stateForElement(
  states: WeakMap<HTMLElement, DampedScrollState>,
  activeStates: Set<DampedScrollState>,
  element: HTMLElement
): DampedScrollState {
  const existing = states.get(element);
  if (existing) {
    return existing;
  }
  const state = {
      frame: 0,
    x: new DampedMotionAxis({
      acceleration: 0.42,
      accelerationDelta: 520,
      friction: SCROLL_FRICTION
    }),
    y: new DampedMotionAxis({
      acceleration: 0.42,
      accelerationDelta: 520,
      friction: SCROLL_FRICTION
    })
  };
  states.set(element, state);
  activeStates.add(state);
  return state;
}

function findScrollableWheelTarget(
  start: Element,
  root: HTMLElement,
  deltaX: number,
  deltaY: number
): HTMLElement | null {
  let current: Element | null = start;
  while (current && root.contains(current)) {
    if (
      current instanceof HTMLElement &&
      (canScrollAxis(current, "y", deltaY) || canScrollAxis(current, "x", deltaX))
    ) {
      return current;
    }
    if (current === root) {
      break;
    }
    current = current.parentElement;
  }
  return null;
}

function canScrollAxis(element: HTMLElement, axis: "x" | "y", delta: number): boolean {
  if (delta === 0) {
    return false;
  }
  const style = window.getComputedStyle(element);
  const overflow =
    axis === "x" ? style.overflowX || style.overflow : style.overflowY || style.overflow;
  if (!/auto|scroll|overlay/i.test(overflow)) {
    return false;
  }
  const scrollPosition = axis === "x" ? element.scrollLeft : element.scrollTop;
  const scrollSize = axis === "x" ? element.scrollWidth : element.scrollHeight;
  const clientSize = axis === "x" ? element.clientWidth : element.clientHeight;
  const maxScroll = Math.max(0, scrollSize - clientSize);
  if (maxScroll <= 0) {
    return false;
  }
  return delta > 0 ? scrollPosition < maxScroll : scrollPosition > 0;
}

function shouldUseNativeWheel(target: Element): boolean {
  return Boolean(
    target.closest(
      [
        "input",
        "textarea",
        "select",
        "[contenteditable='true']",
        "[data-damped-wheel='off']"
      ].join(",")
    )
  );
}

function clampScrollPosition(value: number, scrollSize: number, clientSize: number): number {
  return Math.max(0, Math.min(value, Math.max(0, scrollSize - clientSize)));
}
