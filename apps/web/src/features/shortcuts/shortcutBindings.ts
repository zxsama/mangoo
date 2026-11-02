import { useEffect, useMemo, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";

export type ShortcutActionId =
  | "focusSearch"
  | "renameSelected"
  | "recycleDelete"
  | "permanentDelete"
  | "closeOrReturn"
  | "previewNext"
  | "previewPrevious"
  | "zoomIn"
  | "zoomOut"
  | "toggleSidebars";

export interface ShortcutActionDefinition {
  id: ShortcutActionId;
  label: string;
  defaultBinding: string;
}

export type ShortcutBindings = Record<ShortcutActionId, string>;

export const SHORTCUT_STORAGE_KEY = "mangoo.shortcutBindings.v1";
export const SHORTCUT_BINDINGS_CHANGED_EVENT = "mangoo:shortcut-bindings-changed";

export const DEFAULT_SHORTCUT_ACTIONS: ShortcutActionDefinition[] = [
  { id: "focusSearch", label: "Focus search", defaultBinding: "Ctrl+F" },
  { id: "renameSelected", label: "Rename selected file", defaultBinding: "F2" },
  { id: "recycleDelete", label: "Move selected file to recycle bin", defaultBinding: "Delete" },
  { id: "permanentDelete", label: "Delete selected file permanently", defaultBinding: "Shift+Delete" },
  { id: "closeOrReturn", label: "Close or return", defaultBinding: "Escape" },
  { id: "previewNext", label: "Preview next", defaultBinding: "ArrowRight" },
  { id: "previewPrevious", label: "Preview previous", defaultBinding: "ArrowLeft" },
  { id: "zoomIn", label: "Zoom in", defaultBinding: "Ctrl+=" },
  { id: "zoomOut", label: "Zoom out", defaultBinding: "Ctrl+-" },
  { id: "toggleSidebars", label: "Toggle sidebars", defaultBinding: "Tab" }
];

export const DEFAULT_SHORTCUT_BINDINGS: ShortcutBindings = Object.fromEntries(
  DEFAULT_SHORTCUT_ACTIONS.map((action) => [action.id, action.defaultBinding])
) as ShortcutBindings;

export function useShortcutBindings() {
  const [bindings, setBindingsState] = useState<ShortcutBindings>(() => readShortcutBindings());

  useEffect(() => {
    function syncBindings() {
      setBindingsState(readShortcutBindings());
    }

    window.addEventListener("storage", syncBindings);
    window.addEventListener(SHORTCUT_BINDINGS_CHANGED_EVENT, syncBindings);
    return () => {
      window.removeEventListener("storage", syncBindings);
      window.removeEventListener(SHORTCUT_BINDINGS_CHANGED_EVENT, syncBindings);
    };
  }, []);

  const conflicts = useMemo(() => findShortcutConflicts(bindings), [bindings]);

  function setBinding(actionId: ShortcutActionId, binding: string) {
    const normalized = normalizeShortcutString(binding);
    if (!normalized) return;
    const next = {
      ...bindings,
      [actionId]: normalized
    };
    writeShortcutBindings(next);
    setBindingsState(next);
  }

  function onReset() {
    resetShortcutBindings();
    setBindingsState(DEFAULT_SHORTCUT_BINDINGS);
  }

  return {
    actions: DEFAULT_SHORTCUT_ACTIONS,
    bindings,
    conflicts,
    onReset,
    setBinding
  };
}

export function readShortcutBindings(): ShortcutBindings {
  if (typeof window === "undefined") {
    return DEFAULT_SHORTCUT_BINDINGS;
  }

  try {
    const stored = window.localStorage.getItem(SHORTCUT_STORAGE_KEY);
    if (!stored) return DEFAULT_SHORTCUT_BINDINGS;
    const parsed = JSON.parse(stored) as Partial<Record<ShortcutActionId, unknown>>;
    return {
      ...DEFAULT_SHORTCUT_BINDINGS,
      ...Object.fromEntries(
        DEFAULT_SHORTCUT_ACTIONS.flatMap((action) => {
          const value = parsed[action.id];
          const normalized = typeof value === "string" ? normalizeShortcutString(value) : "";
          return normalized ? [[action.id, normalized]] : [];
        })
      )
    };
  } catch {
    return DEFAULT_SHORTCUT_BINDINGS;
  }
}

export function writeShortcutBindings(bindings: ShortcutBindings): void {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(SHORTCUT_STORAGE_KEY, JSON.stringify(bindings));
  window.dispatchEvent(new CustomEvent(SHORTCUT_BINDINGS_CHANGED_EVENT));
}

export function resetShortcutBindings(): void {
  if (typeof window === "undefined") return;
  window.localStorage.removeItem(SHORTCUT_STORAGE_KEY);
  window.dispatchEvent(new CustomEvent(SHORTCUT_BINDINGS_CHANGED_EVENT));
}

export function matchShortcut(
  event: KeyboardEvent | ReactKeyboardEvent,
  bindings: ShortcutBindings,
  actionId: ShortcutActionId
): boolean {
  const binding = bindings[actionId];
  if (!binding) return false;
  return normalizeShortcutEvent(event) === normalizeShortcutString(binding);
}

export function normalizeShortcutEvent(event: KeyboardEvent | ReactKeyboardEvent): string {
  const key = normalizeKey(event.key);
  if (!key || key === "Control" || key === "Shift" || key === "Alt" || key === "Meta") {
    return "";
  }

  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.shiftKey) parts.push("Shift");
  if (event.altKey) parts.push("Alt");
  if (event.metaKey) parts.push("Meta");
  parts.push(key);
  return parts.join("+");
}

export function normalizeShortcutString(value: string): string {
  const parts = value
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean);
  const modifiers = new Set<string>();
  let key = "";

  for (const part of parts) {
    const lower = part.toLowerCase();
    if (lower === "ctrl" || lower === "control") modifiers.add("Ctrl");
    else if (lower === "shift") modifiers.add("Shift");
    else if (lower === "alt" || lower === "option") modifiers.add("Alt");
    else if (lower === "meta" || lower === "cmd" || lower === "command") modifiers.add("Meta");
    else key = normalizeKey(part);
  }

  if (!key) return "";
  return [
    modifiers.has("Ctrl") ? "Ctrl" : "",
    modifiers.has("Shift") ? "Shift" : "",
    modifiers.has("Alt") ? "Alt" : "",
    modifiers.has("Meta") ? "Meta" : "",
    key
  ]
    .filter(Boolean)
    .join("+");
}

export function findShortcutConflicts(bindings: ShortcutBindings): Partial<Record<ShortcutActionId, ShortcutActionId[]>> {
  const byBinding = new Map<string, ShortcutActionId[]>();
  for (const action of DEFAULT_SHORTCUT_ACTIONS) {
    const binding = normalizeShortcutString(bindings[action.id]);
    if (!binding) continue;
    byBinding.set(binding, [...(byBinding.get(binding) ?? []), action.id]);
  }

  const conflicts: Partial<Record<ShortcutActionId, ShortcutActionId[]>> = {};
  for (const actionIds of byBinding.values()) {
    if (actionIds.length < 2) continue;
    for (const actionId of actionIds) {
      conflicts[actionId] = actionIds.filter((other) => other !== actionId);
    }
  }
  return conflicts;
}

function normalizeKey(key: string): string {
  if (key === " ") return "Space";
  if (key === "+" || key === "=") return "=";
  if (key === "-" || key === "_") return "-";
  if (key.length === 1) return key.toUpperCase();
  if (/^arrow(left|right|up|down)$/i.test(key)) {
    return `Arrow${key.slice(5, 6).toUpperCase()}${key.slice(6).toLowerCase()}`;
  }
  if (/^f\d{1,2}$/i.test(key)) return key.toUpperCase();
  if (key === "Esc") return "Escape";
  return key;
}
