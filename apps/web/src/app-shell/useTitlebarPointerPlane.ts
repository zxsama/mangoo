import { useEffect, type HTMLAttributes, type RefObject } from "react";

const TITLEBAR_CONTROL_SELECTOR = "[data-titlebar-control]";
const TITLEBAR_SURFACE_SELECTOR = '[data-titlebar-surface="true"]';
const TITLEBAR_PLANE_HALO_PX = 24;
const TITLEBAR_CONTROL_POINTER_EDGE_PROXIMITY_PX = 156;

export function useTitlebarPointerPlane(rootRef: RefObject<HTMLElement | null>) {
  useEffect(() => {
    let animationFrame = 0;
    let latestPointer: { x: number; y: number } | null = null;

    function cancelPendingFrame() {
      latestPointer = null;
      if (!animationFrame) return;
      window.cancelAnimationFrame(animationFrame);
      animationFrame = 0;
    }

    function requestPlaneUpdate(clientX: number, clientY: number) {
      latestPointer = { x: clientX, y: clientY };
      if (animationFrame) return;
      animationFrame = window.requestAnimationFrame(() => {
        animationFrame = 0;
        if (!latestPointer) return;
        updateTitlebarPointerPlane(rootRef.current, latestPointer.x, latestPointer.y);
      });
    }

    function handleWindowPointerMove(event: globalThis.PointerEvent) {
      requestPlaneUpdate(event.clientX, event.clientY);
    }

    function handleWindowPointerExit() {
      cancelPendingFrame();
      hideTitlebarControlPointers(rootRef.current);
    }

    window.addEventListener("pointermove", handleWindowPointerMove, { passive: true });
    window.addEventListener("pointerleave", handleWindowPointerExit);
    window.addEventListener("blur", handleWindowPointerExit);

    return () => {
      cancelPendingFrame();
      window.removeEventListener("pointermove", handleWindowPointerMove);
      window.removeEventListener("pointerleave", handleWindowPointerExit);
      window.removeEventListener("blur", handleWindowPointerExit);
      hideTitlebarControlPointers(rootRef.current);
    };
  }, [rootRef]);

  const titlebarSurfaceProps: Pick<
    HTMLAttributes<HTMLElement>,
    "onPointerCancel" | "onPointerMove"
  > & { "data-titlebar-surface": "true" } = {
    "data-titlebar-surface": "true",
    // Window drag + double-click-maximize + Aero Snap are handled NATIVELY via
    // `-webkit-app-region: drag` on .shell-titlebar (see styles.css). We only
    // keep the pointer-move highlight here; the former JS setPosition drag (which
    // offset the cursor after restoring from maximized and had no edge snapping)
    // is gone. Titlebar controls stay clickable via `-webkit-app-region: no-drag`.
    onPointerMove: (event) => {
      updateTitlebarPointerPlane(rootRef.current, event.clientX, event.clientY);
    },
    onPointerCancel: () => {
      hideTitlebarControlPointers(rootRef.current);
    }
  };

  return {
    titlebarSurfaceProps
  };
}

function updateTitlebarPointerPlane(
  root: HTMLElement | null,
  clientX: number,
  clientY: number
) {
  const controls = getTitlebarControls(root);
  if (controls.length === 0) {
    return;
  }

  if (!pointerInsideTitlebarPlane(root, clientX, clientY)) {
    for (const control of controls) {
      resetTitlebarControlPointer(control);
    }
    return;
  }

  for (const control of controls) {
    if (control.matches(":disabled")) {
      resetTitlebarControlPointer(control);
      continue;
    }

    const rect = control.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      resetTitlebarControlPointer(control);
      continue;
    }

    const distance = distanceToRectEdge(clientX, clientY, rect);
    if (distance > TITLEBAR_CONTROL_POINTER_EDGE_PROXIMITY_PX) {
      resetTitlebarControlPointer(control);
      continue;
    }

    const pointer = pointerPercentForRect(clientX, clientY, rect);
    const opacity = Math.pow(1 - distance / TITLEBAR_CONTROL_POINTER_EDGE_PROXIMITY_PX, 1.55);
    control.dataset.glassPointer = "active";
    control.style.setProperty("--glass-pointer-x", `${pointer.x}%`);
    control.style.setProperty("--glass-pointer-y", `${pointer.y}%`);
    control.style.setProperty("--glass-pointer-opacity", opacity.toFixed(3));
  }
}

function getTitlebarControls(root: HTMLElement | null) {
  if (!root) return [];
  return Array.from(root.querySelectorAll<HTMLElement>(TITLEBAR_CONTROL_SELECTOR));
}

function pointerInsideTitlebarPlane(
  root: HTMLElement | null,
  clientX: number,
  clientY: number
) {
  if (!root) return false;

  const surfaces = root.querySelectorAll<HTMLElement>(TITLEBAR_SURFACE_SELECTOR);
  return Array.from(surfaces).some((surface) => {
    const rect = surface.getBoundingClientRect();
    return pointInsideExpandedRect(rect, clientX, clientY, TITLEBAR_PLANE_HALO_PX);
  });
}

function hideTitlebarControlPointers(root: HTMLElement | null) {
  for (const control of getTitlebarControls(root)) {
    resetTitlebarControlPointer(control);
  }
}

function resetTitlebarControlPointer(control: HTMLElement) {
  control.dataset.glassPointer = "idle";
  control.style.setProperty("--glass-pointer-opacity", "0");
}

function nearestPointOnRect(clientX: number, clientY: number, rect: DOMRect) {
  const x = clamp(clientX, rect.left, rect.right);
  const y = clamp(clientY, rect.top, rect.bottom);
  const inside =
    clientX >= rect.left && clientX <= rect.right && clientY >= rect.top && clientY <= rect.bottom;

  if (!inside) {
    return { x, y };
  }

  const candidates = [
    { x, y: rect.top, distance: clientY - rect.top },
    { x: rect.right, y, distance: rect.right - clientX },
    { x, y: rect.bottom, distance: rect.bottom - clientY },
    { x: rect.left, y, distance: clientX - rect.left }
  ];

  return candidates.reduce((nearest, candidate) =>
    candidate.distance < nearest.distance ? candidate : nearest
  );
}

function distanceToRectEdge(clientX: number, clientY: number, rect: DOMRect) {
  const edgePoint = nearestPointOnRect(clientX, clientY, rect);
  return Math.hypot(clientX - edgePoint.x, clientY - edgePoint.y);
}

function pointInsideExpandedRect(rect: DOMRect, clientX: number, clientY: number, expandBy: number) {
  return (
    clientX >= rect.left - expandBy &&
    clientX <= rect.right + expandBy &&
    clientY >= rect.top - expandBy &&
    clientY <= rect.bottom + expandBy
  );
}

function pointerPercentForRect(clientX: number, clientY: number, rect: DOMRect) {
  const x = ((clamp(clientX, rect.left, rect.right) - rect.left) / rect.width) * 100;
  const y = ((clamp(clientY, rect.top, rect.bottom) - rect.top) / rect.height) * 100;
  return {
    x: clampPercent(x),
    y: clampPercent(y)
  };
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function clampPercent(value: number) {
  return Math.min(100, Math.max(0, value));
}
