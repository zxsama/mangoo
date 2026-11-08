import { useCallback, useEffect, useRef, useState } from "react";

/**
 * One reveal model for every piece of floating chrome (titlebar, toolbar,
 * edge arrows). Each zone is revealed while the pointer is NEAR it — not
 * merely anywhere in the window — and hides after a short grace delay once
 * the pointer leaves, so crossing the boundary never flickers. Zones stay up
 * while the pointer is over them, while a menu/dialog is open, or while the
 * zone is pinned.
 */

export type ChromeZone = "top" | "bottom" | "left" | "right";

export interface ProximityOptions {
  /** Element per zone; the zone's band is measured from its edge, and the
   * pointer anywhere over the element keeps it shown. */
  anchors?: Partial<Record<ChromeZone, () => Element | null>>;
  /** Distance from the window edge that reveals each zone (px). */
  reach: Record<ChromeZone, number>;
  /** Extra distance the pointer may move away before a visible zone hides. */
  hysteresis: number;
  /** Delay before a zone hides after the pointer leaves it (ms). */
  hideDelay: number;
  /** Zones that are always visible. */
  pinned: Partial<Record<ChromeZone, boolean>>;
  /** Freeze every zone visible (menus, dialogs, dragging a panel). */
  hold: boolean;
  /** Elements that block a zone's reveal while the pointer is over them. */
  blockers?: Partial<Record<ChromeZone, string>>;
}

export function zoneWanted(zone: ChromeZone, x: number, y: number, width: number, height: number, distance: number): boolean {
  switch (zone) {
    case "top": return y <= distance;
    case "bottom": return y >= height - distance;
    case "left": return x <= distance && y > height * 0.2 && y < height * 0.8;
    case "right": return x >= width - distance && y > height * 0.2 && y < height * 0.8;
  }
}

export function useProximityChrome(options: ProximityOptions) {
  const [visible, setVisible] = useState<Record<ChromeZone, boolean>>({ top: false, bottom: false, left: false, right: false });
  const timers = useRef<Partial<Record<ChromeZone, number>>>({});
  const optionsRef = useRef(options);
  optionsRef.current = options;
  const visibleRef = useRef(visible);
  visibleRef.current = visible;

  const setZone = useCallback((zone: ChromeZone, show: boolean) => {
    const pending = timers.current[zone];
    if (show) {
      if (pending) window.clearTimeout(pending);
      timers.current[zone] = undefined;
      if (!visibleRef.current[zone]) setVisible((current) => ({ ...current, [zone]: true }));
      return;
    }
    if (!visibleRef.current[zone] || pending) return;
    timers.current[zone] = window.setTimeout(() => {
      timers.current[zone] = undefined;
      setVisible((current) => ({ ...current, [zone]: false }));
    }, optionsRef.current.hideDelay);
  }, []);

  const evaluate = useCallback((x: number, y: number) => {
    const { reach, hysteresis, anchors, blockers } = optionsRef.current;
    const hit = document.elementFromPoint(x, y);
    const width = window.innerWidth;
    const height = window.innerHeight;
    for (const zone of ["top", "bottom", "left", "right"] as const) {
      // A visible zone gets a larger catch area so small moves never toggle it.
      const distance = reach[zone] + (visibleRef.current[zone] ? hysteresis : 0);
      const element = anchors?.[zone]?.() ?? null;
      const box = element?.getBoundingClientRect();
      let wanted: boolean;
      if (box && box.width > 0 && box.height > 0) {
        const over = x >= box.left && x <= box.right && y >= box.top && y <= box.bottom;
        // Band measured from the element's far edge toward the picture.
        const near = zone === "top" ? y <= box.bottom + distance
          : zone === "bottom" ? y >= box.top - distance
          : zone === "left" ? x <= box.right + distance && y >= box.top - 40 && y <= box.bottom + 40
          : x >= box.left - distance && y >= box.top - 40 && y <= box.bottom + 40;
        wanted = over || near;
      } else {
        wanted = zoneWanted(zone, x, y, width, height, distance);
      }
      const blocker = blockers?.[zone];
      // The outermost 6px always reveals (window buttons stay reachable even
      // through a panel docked in the corner).
      const atVeryEdge = zone === "top" ? y <= 6 : zone === "bottom" ? y >= height - 6 : false;
      if (wanted && !atVeryEdge && blocker && hit?.closest(blocker) && !(element && hit && element.contains(hit))) wanted = false;
      setZone(zone, wanted);
    }
  }, [setZone]);

  useEffect(() => {
    const onMove = (event: PointerEvent) => evaluate(event.clientX, event.clientY);
    const onLeave = () => {
      for (const zone of ["top", "bottom", "left", "right"] as const) setZone(zone, false);
    };
    window.addEventListener("pointermove", onMove, { passive: true });
    document.documentElement.addEventListener("pointerleave", onLeave);
    return () => {
      window.removeEventListener("pointermove", onMove);
      document.documentElement.removeEventListener("pointerleave", onLeave);
      for (const timer of Object.values(timers.current)) if (timer) window.clearTimeout(timer);
    };
  }, [evaluate, setZone]);

  /** Briefly show a zone (e.g. after a keyboard navigation). */
  const flash = useCallback((zone: ChromeZone) => {
    setZone(zone, true);
    setZone(zone, false);
  }, [setZone]);

  const shown = (zone: ChromeZone) => Boolean(options.pinned[zone]) || options.hold || visible[zone];
  return { shown, flash };
}
