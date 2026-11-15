import { useCallback, useEffect, useState } from "react";

import type { SortMode, ViewKey } from "../shared/model.ts";
import { normalizeOverrides, type ShortcutOverrides } from "../shared/shortcuts.ts";

/**
 * Per-device UI preferences of the notes window (localStorage). Every TODO
 * window of this device shares them through `storage` events; they are
 * presentation only and never synchronized with the Library.
 */
export interface TodoPreferences {
  view: ViewKey;
  /** Sort per view key. */
  sort: Record<string, SortMode>;
  showCompleted: boolean;
  sidebarCollapsed: boolean;
  detailWidth: number;
  /** Compact rows (one line per item). */
  compact: boolean;
  /** Collapsed subtask groups by item id. */
  collapsed: string[];
  /** Remapped keyboard shortcuts (only changed actions). */
  shortcuts: ShortcutOverrides;
}

const KEY = "mangoo.todo-notes.preferences.v2";

export const DEFAULT_PREFERENCES: TodoPreferences = {
  view: "all",
  sort: {},
  showCompleted: false,
  sidebarCollapsed: false,
  detailWidth: 400,
  compact: false,
  collapsed: [],
  shortcuts: {}
};

function read(): TodoPreferences {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "null") as Partial<TodoPreferences> | null;
    if (!raw || typeof raw !== "object") return { ...DEFAULT_PREFERENCES };
    return {
      view: typeof raw.view === "string" ? (raw.view as ViewKey) : DEFAULT_PREFERENCES.view,
      sort: raw.sort && typeof raw.sort === "object" ? raw.sort : {},
      showCompleted: raw.showCompleted === true,
      sidebarCollapsed: raw.sidebarCollapsed === true,
      detailWidth: typeof raw.detailWidth === "number" ? Math.min(720, Math.max(320, raw.detailWidth)) : DEFAULT_PREFERENCES.detailWidth,
      compact: raw.compact === true,
      collapsed: Array.isArray(raw.collapsed) ? raw.collapsed.filter((id): id is string => typeof id === "string").slice(-500) : [],
      shortcuts: normalizeOverrides(raw.shortcuts)
    };
  } catch {
    return { ...DEFAULT_PREFERENCES };
  }
}

export function useTodoPreferences(): [TodoPreferences, (patch: Partial<TodoPreferences>) => void] {
  const [preferences, setPreferences] = useState<TodoPreferences>(read);
  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key === KEY) setPreferences(read());
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);
  const update = useCallback((patch: Partial<TodoPreferences>) => {
    setPreferences((current) => {
      const next = { ...current, ...patch };
      try {
        localStorage.setItem(KEY, JSON.stringify(next));
      } catch {
        // Storage full / unavailable: keep the in-memory value.
      }
      return next;
    });
  }, []);
  return [preferences, update];
}
