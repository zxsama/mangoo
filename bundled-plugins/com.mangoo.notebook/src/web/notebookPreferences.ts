import { useSyncExternalStore } from "react";
import { normalizeNoteEditorStyles, type NoteEditorStyles } from "./editor/noteFormat.ts";

export interface NotebookPreferences {
  markdownMode: "live" | "source";
  htmlMode: "code" | "split" | "preview";
  outlineOpen: boolean;
  wrapSource: boolean;
  elementStyles: NoteEditorStyles;
}

export const defaultNotebookPreferences: NotebookPreferences = {
  markdownMode: "live", htmlMode: "split", outlineOpen: false, wrapSource: true, elementStyles: {}
};

const storageKey = "mangoo.notebook.preferences.v1";
const changedEvent = "mangoo-notebook-preferences-changed";

export function normalizeNotebookPreferences(value: unknown): NotebookPreferences {
  const raw = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
  return {
    markdownMode: raw.markdownMode === "source" ? "source" : defaultNotebookPreferences.markdownMode,
    htmlMode: raw.htmlMode === "code" || raw.htmlMode === "preview" ? raw.htmlMode : defaultNotebookPreferences.htmlMode,
    outlineOpen: typeof raw.outlineOpen === "boolean" ? raw.outlineOpen : defaultNotebookPreferences.outlineOpen,
    wrapSource: typeof raw.wrapSource === "boolean" ? raw.wrapSource : defaultNotebookPreferences.wrapSource,
    elementStyles: normalizeNoteEditorStyles(raw.elementStyles)
  };
}

let cachedRaw: string | null | undefined;
let snapshot = defaultNotebookPreferences;

export function readNotebookPreferences(): NotebookPreferences {
  if (typeof window === "undefined") return snapshot;
  let raw: string | null;
  try {
    raw = window.localStorage.getItem(storageKey);
  } catch {
    return snapshot;
  }
  if (raw !== cachedRaw) {
    cachedRaw = raw;
    try {
      snapshot = raw === null ? defaultNotebookPreferences : normalizeNotebookPreferences(JSON.parse(raw));
    } catch {
      snapshot = defaultNotebookPreferences;
    }
  }
  return snapshot;
}

export function subscribeNotebookPreferences(notify: () => void): () => void {
  const onStorage = (event: StorageEvent) => {
    if (event.key === storageKey || event.key === null) notify();
  };
  window.addEventListener("storage", onStorage);
  window.addEventListener(changedEvent, notify);
  return () => {
    window.removeEventListener("storage", onStorage);
    window.removeEventListener(changedEvent, notify);
  };
}

export function useNotebookPreferences(): NotebookPreferences {
  return useSyncExternalStore(subscribeNotebookPreferences, readNotebookPreferences, () => defaultNotebookPreferences);
}

export function updateNotebookPreferences(patch: Partial<NotebookPreferences>): void {
  const next = normalizeNotebookPreferences({ ...readNotebookPreferences(), ...patch });
  const serialized = JSON.stringify(next);
  try {
    window.localStorage.setItem(storageKey, serialized);
    cachedRaw = serialized;
  } catch {
    // Keep working in this window when storage is disabled or full.
  }
  snapshot = next;
  if (typeof window !== "undefined") window.dispatchEvent(new Event(changedEvent));
}

export function resetNotebookPreferences(): void {
  updateNotebookPreferences(defaultNotebookPreferences);
}
