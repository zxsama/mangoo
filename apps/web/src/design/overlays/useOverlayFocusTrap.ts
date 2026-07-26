import { useEffect, type RefObject } from "react";
import { resolveOverlayFocusRestoreTarget } from "./overlayRuntime";

const TABBABLE_SELECTOR =
  'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

function isVisible(element: HTMLElement): boolean {
  if (element.hidden) return false;
  if (element.getAttribute("aria-hidden") === "true") return false;
  if (element.offsetParent === null && getComputedStyle(element).position !== "fixed") {
    return false;
  }
  return true;
}

function getTabbable(container: HTMLElement): HTMLElement[] {
  const nodes = Array.from(container.querySelectorAll<HTMLElement>(TABBABLE_SELECTOR));
  return nodes.filter((node) => {
    if (node.hasAttribute("disabled")) return false;
    if (node.getAttribute("aria-disabled") === "true") return false;
    if (node.getAttribute("aria-hidden") === "true") return false;
    return isVisible(node);
  });
}

export interface UseOverlayFocusTrapOptions {
  initialFocusRef?: RefObject<HTMLElement | null>;
  restoreFocusRef?: RefObject<HTMLElement | null>;
}

export function useOverlayFocusTrap(
  open: boolean,
  containerRef: RefObject<HTMLElement | null>,
  options: UseOverlayFocusTrapOptions = {}
): void {
  const { initialFocusRef, restoreFocusRef } = options;

  useEffect(() => {
    if (!open) return;
    const container = containerRef.current;
    if (!container) return;

    const previousFocus = document.activeElement as HTMLElement | null;
    const initial = initialFocusRef?.current ?? null;
    if (initial) {
      initial.focus();
    } else {
      const tabbables = getTabbable(container);
      (tabbables[0] ?? container).focus();
    }

    const handleKey = (event: KeyboardEvent) => {
      if (event.key !== "Tab") return;
      const node = containerRef.current;
      if (!node) return;
      const tabbables = getTabbable(node);
      if (tabbables.length === 0) {
        event.preventDefault();
        node.focus();
        return;
      }

      const first = tabbables[0];
      const last = tabbables[tabbables.length - 1];
      const active = document.activeElement as HTMLElement | null;
      if (event.shiftKey) {
        if (active === first || !node.contains(active)) {
          event.preventDefault();
          last.focus();
        }
      } else if (active === last || !node.contains(active)) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", handleKey, true);
    return () => {
      document.removeEventListener("keydown", handleKey, true);
      const requestedRestore = restoreFocusRef?.current ?? null;
      resolveOverlayFocusRestoreTarget(requestedRestore, previousFocus)?.focus();
    };
  }, [containerRef, initialFocusRef, open, restoreFocusRef]);
}
