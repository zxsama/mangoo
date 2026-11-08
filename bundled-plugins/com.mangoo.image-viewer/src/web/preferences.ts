import { useCallback, useEffect, useState } from "react";

import type { ImageSortDirection, ImageSortKey } from "../shared/navigation.ts";
import type { FitMode } from "../shared/viewTransform.ts";
import { normalizeOverrides, type ShortcutOverrides } from "../shared/shortcuts.ts";

export type ViewerBackground = "interface" | "black" | "dark" | "checker" | "light";
export type WheelAction = "zoom" | "navigate";

export interface ViewerPreferences {
  background: ViewerBackground;
  fitMode: Extract<FitMode, "fit" | "fill" | "actual">;
  upscaleSmall: boolean;
  wheelAction: WheelAction;
  loop: boolean;
  sortKey: ImageSortKey;
  sortDirection: ImageSortDirection;
  slideshowSeconds: number;
  pixelatedZoom: boolean;
  confirmDelete: boolean;
  escapeCloses: boolean;
  showFilmstrip: boolean;
  showInfo: boolean;
  /** Keep the rotation/zoom when moving to the next picture. */
  keepTransform: boolean;
  /** Titlebar stays visible (true) or auto-hides and reveals on hover (false). */
  titlebarPinned: boolean;
  /** Show the frame panel automatically for animated images. */
  framesAutoOpen: boolean;
  shortcuts: ShortcutOverrides;
}

export const DEFAULT_VIEWER_PREFERENCES: ViewerPreferences = {
  background: "interface",
  fitMode: "fit",
  upscaleSmall: false,
  wheelAction: "navigate",
  loop: true,
  sortKey: "name",
  sortDirection: "asc",
  slideshowSeconds: 4,
  pixelatedZoom: true,
  confirmDelete: true,
  escapeCloses: true,
  showFilmstrip: true,
  showInfo: false,
  keepTransform: false,
  titlebarPinned: false,
  framesAutoOpen: true,
  shortcuts: {}
};

export const SLIDESHOW_INTERVALS = [2, 3, 4, 6, 10, 20];

// v2: wheel default changed to navigate; v1 values migrate below.
const STORAGE_KEY = "mangoo.imageViewer.preferences.v2";
const LEGACY_STORAGE_KEY = "mangoo.imageViewer.preferences.v1";

function oneOf<T extends string>(value: unknown, options: readonly T[], fallback: T): T {
  return typeof value === "string" && (options as readonly string[]).includes(value) ? (value as T) : fallback;
}

export function normalizeViewerPreferences(value: unknown): ViewerPreferences {
  const raw = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
  const d = DEFAULT_VIEWER_PREFERENCES;
  const bool = (key: keyof ViewerPreferences) => (typeof raw[key] === "boolean" ? (raw[key] as boolean) : (d[key] as boolean));
  const seconds = Number(raw.slideshowSeconds);
  return {
    background: oneOf(raw.background, ["interface", "black", "dark", "checker", "light"] as const, d.background),
    fitMode: oneOf(raw.fitMode, ["fit", "fill", "actual"] as const, d.fitMode),
    upscaleSmall: bool("upscaleSmall"),
    wheelAction: oneOf(raw.wheelAction, ["zoom", "navigate"] as const, d.wheelAction),
    loop: bool("loop"),
    sortKey: oneOf(raw.sortKey, ["name", "modified", "size", "type"] as const, d.sortKey),
    sortDirection: oneOf(raw.sortDirection, ["asc", "desc"] as const, d.sortDirection),
    slideshowSeconds: SLIDESHOW_INTERVALS.includes(seconds) ? seconds : d.slideshowSeconds,
    pixelatedZoom: bool("pixelatedZoom"),
    confirmDelete: bool("confirmDelete"),
    escapeCloses: bool("escapeCloses"),
    showFilmstrip: bool("showFilmstrip"),
    showInfo: bool("showInfo"),
    keepTransform: bool("keepTransform"),
    titlebarPinned: bool("titlebarPinned"),
    framesAutoOpen: bool("framesAutoOpen"),
    shortcuts: normalizeOverrides(raw.shortcuts)
  };
}

function readPreferences(): ViewerPreferences {
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (stored === null) {
      // Carry v1 choices over, except the wheel, whose default changed.
      const legacy = JSON.parse(window.localStorage.getItem(LEGACY_STORAGE_KEY) ?? "{}") as Record<string, unknown>;
      delete legacy.wheelAction;
      return normalizeViewerPreferences(legacy);
    }
    return normalizeViewerPreferences(JSON.parse(stored));
  } catch {
    return DEFAULT_VIEWER_PREFERENCES;
  }
}

export function useViewerPreferences(): [ViewerPreferences, (change: Partial<ViewerPreferences>) => void] {
  const [preferences, setPreferences] = useState(readPreferences);
  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key === STORAGE_KEY) setPreferences(readPreferences());
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);
  const update = useCallback((change: Partial<ViewerPreferences>) => {
    setPreferences((current) => {
      const next = normalizeViewerPreferences({ ...current, ...change });
      try {
        window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
      } catch {
        // Storage full or disabled: keep the in-memory preference.
      }
      return next;
    });
  }, []);
  return [preferences, update];
}
