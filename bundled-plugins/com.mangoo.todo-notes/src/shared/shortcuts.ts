/**
 * User-remappable keyboard shortcuts of the notes window, the same model as
 * the image viewer's: an action registry with default chords, canonical chord
 * strings, conflict handling and matching. Overrides are per-device display
 * preferences (only changed actions are stored). Text formatting inside the
 * notes editor (Ctrl+B …) is not remappable: it follows every editor.
 */

export type ShortcutActionId =
  | "newNote" | "search" | "selectNext" | "selectPrevious" | "edit"
  | "toggleDone" | "togglePin" | "delete"
  | "dueToday" | "dueTomorrow" | "clearDue"
  | "priorityHigh" | "priorityMedium" | "priorityLow" | "priorityNone"
  | "toggleCompact" | "toggleOnTop" | "openSettings" | "closeWindow";

export type ShortcutGroup = "记录与查找" | "选中的便笺" | "日期与优先级" | "窗口";

export interface ShortcutAction {
  id: ShortcutActionId;
  label: string;
  group: ShortcutGroup;
  /** Default chords; the first one is shown in menus and tooltips. */
  defaults: string[];
}

export const SHORTCUT_ACTIONS: readonly ShortcutAction[] = [
  { id: "newNote", label: "记一条", group: "记录与查找", defaults: ["Ctrl+N"] },
  { id: "search", label: "搜索", group: "记录与查找", defaults: ["Ctrl+F"] },
  { id: "selectNext", label: "选择下一条", group: "记录与查找", defaults: ["ArrowDown"] },
  { id: "selectPrevious", label: "选择上一条", group: "记录与查找", defaults: ["ArrowUp"] },
  { id: "edit", label: "展开编辑", group: "选中的便笺", defaults: ["F2"] },
  { id: "toggleDone", label: "完成 / 取消完成", group: "选中的便笺", defaults: ["Space"] },
  { id: "togglePin", label: "贴到桌面 / 取下", group: "选中的便笺", defaults: ["P"] },
  { id: "delete", label: "删除（可撤销）", group: "选中的便笺", defaults: ["Delete"] },
  { id: "dueToday", label: "设为今天", group: "日期与优先级", defaults: ["T"] },
  { id: "dueTomorrow", label: "设为明天", group: "日期与优先级", defaults: ["Shift+T"] },
  { id: "clearDue", label: "清除日期", group: "日期与优先级", defaults: ["Shift+D"] },
  { id: "priorityHigh", label: "高优先级", group: "日期与优先级", defaults: ["3"] },
  { id: "priorityMedium", label: "中优先级", group: "日期与优先级", defaults: ["2"] },
  { id: "priorityLow", label: "低优先级", group: "日期与优先级", defaults: ["1"] },
  { id: "priorityNone", label: "无优先级", group: "日期与优先级", defaults: ["0"] },
  { id: "toggleCompact", label: "紧凑 / 标准显示", group: "窗口", defaults: ["Ctrl+Shift+M"] },
  { id: "toggleOnTop", label: "窗口置顶", group: "窗口", defaults: ["Ctrl+Shift+P"] },
  { id: "openSettings", label: "打开设置", group: "窗口", defaults: ["Ctrl+,"] },
  { id: "closeWindow", label: "关闭窗口", group: "窗口", defaults: ["Ctrl+W"] }
];

export const SHORTCUT_GROUPS: readonly ShortcutGroup[] = ["记录与查找", "选中的便笺", "日期与优先级", "窗口"];

/** Actions that act on the selected note; they never fire while typing. */
export const SELECTION_ACTIONS: ReadonlySet<ShortcutActionId> = new Set([
  "selectNext", "selectPrevious", "edit", "toggleDone", "togglePin", "delete",
  "dueToday", "dueTomorrow", "clearDue", "priorityHigh", "priorityMedium", "priorityLow", "priorityNone"
]);

/** Persisted overrides: only actions the user changed are stored. */
export type ShortcutOverrides = Partial<Record<ShortcutActionId, string[]>>;
export type ShortcutBindings = Record<ShortcutActionId, string[]>;

const MAX_CHORDS = 4;
const MODIFIER_ORDER = ["Ctrl", "Alt", "Shift"] as const;
const NAMED_KEYS: Record<string, string> = {
  " ": "Space", Spacebar: "Space", Esc: "Escape", Del: "Delete",
  Left: "ArrowLeft", Right: "ArrowRight", Up: "ArrowUp", Down: "ArrowDown",
  "+": "=", _: "-"
};
/**
 * Keys the window owns: Esc closes / cancels, Tab moves focus, Enter opens
 * the selected note and submits fields. They cannot be rebound.
 */
export const RESERVED_KEYS: ReadonlySet<string> = new Set(["Escape", "Tab", "Enter", "Shift+Tab"]);

export interface ChordEvent {
  key: string;
  code?: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/** Physical-key base so Shift+/ is "Shift+/" rather than "?" on US layouts. */
function baseKey(event: ChordEvent): string | null {
  const key = NAMED_KEYS[event.key] ?? event.key;
  if (["Control", "Shift", "Alt", "Meta", "OS", "Dead", "Unidentified", "Process"].includes(key)) return null;
  const code = event.code ?? "";
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^Numpad[0-9]$/.test(code)) return code.slice(6);
  const byCode: Record<string, string> = {
    Slash: "/", Period: ".", Comma: ",", Minus: "-", Equal: "=", Semicolon: ";", Quote: "'",
    BracketLeft: "[", BracketRight: "]", Backslash: "\\", Backquote: "`",
    NumpadAdd: "=", NumpadSubtract: "-", Space: "Space"
  };
  if (byCode[code]) return byCode[code];
  return key.length === 1 ? key.toUpperCase() : key;
}

/** Canonical chord string ("Ctrl+Shift+C") for a keyboard event, or null. */
export function chordFromEvent(event: ChordEvent): string | null {
  const key = baseKey(event);
  if (!key) return null;
  const parts: string[] = [];
  if (event.ctrlKey || event.metaKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  parts.push(key);
  return parts.join("+");
}

/** Normalise user / stored text into the canonical chord form. */
export function normalizeChord(chord: string): string | null {
  if (typeof chord !== "string") return null;
  const raw = chord.trim();
  if (!raw) return null;
  const pieces = raw === "+" ? ["="] : raw.split("+").filter(Boolean);
  if (raw.length > 1 && raw.endsWith("++")) pieces.push("=");
  const key = pieces.pop();
  if (!key) return null;
  const modifiers = new Set(pieces.map((piece) => {
    const lower = piece.toLowerCase();
    return lower === "control" || lower === "cmd" || lower === "meta" ? "Ctrl" : lower === "option" ? "Alt" : piece.charAt(0).toUpperCase() + piece.slice(1).toLowerCase();
  }));
  if ([...modifiers].some((modifier) => !MODIFIER_ORDER.includes(modifier as never))) return null;
  const mapped = NAMED_KEYS[key] ?? key;
  const normalizedKey = mapped.length === 1 ? mapped.toUpperCase() : mapped;
  return [...MODIFIER_ORDER.filter((modifier) => modifiers.has(modifier)), normalizedKey].join("+");
}

export function resolveBindings(overrides: ShortcutOverrides): ShortcutBindings {
  const result = {} as ShortcutBindings;
  for (const action of SHORTCUT_ACTIONS) {
    const custom = overrides[action.id];
    const chords = (custom ?? action.defaults).map(normalizeChord).filter((value): value is string => Boolean(value));
    result[action.id] = [...new Set(chords)];
  }
  return result;
}

export function normalizeOverrides(value: unknown): ShortcutOverrides {
  if (!value || typeof value !== "object") return {};
  const result: ShortcutOverrides = {};
  for (const action of SHORTCUT_ACTIONS) {
    const chords = (value as Record<string, unknown>)[action.id];
    if (!Array.isArray(chords)) continue;
    const normalized = chords
      .map((chord) => normalizeChord(String(chord)))
      .filter((chord): chord is string => Boolean(chord) && !RESERVED_KEYS.has(chord as string));
    result[action.id] = [...new Set(normalized)].slice(0, MAX_CHORDS);
  }
  return result;
}

/** Which other action already uses `chord` (for conflict warnings). */
export function findConflict(bindings: ShortcutBindings, chord: string, except: ShortcutActionId): ShortcutActionId | null {
  for (const action of SHORTCUT_ACTIONS) {
    if (action.id !== except && bindings[action.id].includes(chord)) return action.id;
  }
  return null;
}

/** Assign `chord` to `action`, removing it from whichever action had it. */
export function assignChord(overrides: ShortcutOverrides, action: ShortcutActionId, chord: string, replaceIndex: number | null): ShortcutOverrides {
  const bindings = resolveBindings(overrides);
  const next: ShortcutOverrides = { ...overrides };
  const conflict = findConflict(bindings, chord, action);
  if (conflict) next[conflict] = bindings[conflict].filter((value) => value !== chord);
  const current = [...bindings[action]];
  if (replaceIndex !== null && replaceIndex < current.length) current[replaceIndex] = chord;
  else current.push(chord);
  next[action] = [...new Set(current)].slice(0, MAX_CHORDS);
  return next;
}

export function removeChord(overrides: ShortcutOverrides, action: ShortcutActionId, chord: string): ShortcutOverrides {
  const bindings = resolveBindings(overrides);
  return { ...overrides, [action]: bindings[action].filter((value) => value !== chord) };
}

export function resetAction(overrides: ShortcutOverrides, action: ShortcutActionId): ShortcutOverrides {
  const next = { ...overrides };
  delete next[action];
  return next;
}

export function isCustomized(overrides: ShortcutOverrides, action: ShortcutActionId): boolean {
  return Object.hasOwn(overrides, action);
}

/** Action bound to the event's chord, if any. */
export function matchAction(bindings: ShortcutBindings, event: ChordEvent): ShortcutActionId | null {
  const chord = chordFromEvent(event);
  if (!chord) return null;
  for (const action of SHORTCUT_ACTIONS) {
    if (bindings[action.id].includes(chord)) return action.id;
  }
  return null;
}

const DISPLAY: Record<string, string> = {
  ArrowRight: "→", ArrowLeft: "←", ArrowUp: "↑", ArrowDown: "↓",
  PageDown: "PgDn", PageUp: "PgUp", Delete: "Del", Backspace: "退格", Space: "空格", "=": "+", "-": "−"
};

/** Keycaps for a chord, e.g. ["Ctrl", "Shift", "C"]. */
export function chordParts(chord: string): string[] {
  if (chord === "Shift+/") return ["?"];
  return chord.split("+").filter(Boolean).map((part) => DISPLAY[part] ?? part);
}

/** Human label for a chord: "Ctrl+Shift+C", "空格". */
export function formatChord(chord: string): string {
  return chordParts(chord).join("+");
}

/** The hint shown next to a menu item: the action's first chord, or nothing. */
export function shortcutHint(bindings: ShortcutBindings, action: ShortcutActionId): string | undefined {
  const chord = bindings[action][0];
  return chord ? formatChord(chord) : undefined;
}
