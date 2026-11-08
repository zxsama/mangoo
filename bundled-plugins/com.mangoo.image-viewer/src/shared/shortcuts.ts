/**
 * User-remappable keyboard shortcuts for the viewer. Pure model: an action
 * registry with default chords, chord parsing/formatting, conflict detection
 * and matching. The UI records chords; the window matches events here.
 */

export type ShortcutActionId =
  | "next" | "previous" | "first" | "last"
  | "zoomIn" | "zoomOut" | "fit" | "actualSize"
  | "rotateRight" | "rotateLeft" | "flipHorizontal" | "flipVertical"
  | "fullscreen" | "slideshow" | "toggleInfo" | "toggleFilmstrip" | "toggleTitlebarPin"
  | "playPause" | "nextFrame" | "previousFrame"
  | "open" | "copyImage" | "copyPath" | "rename" | "delete" | "showShortcuts" | "closeWindow";

export interface ShortcutAction {
  id: ShortcutActionId;
  label: string;
  group: "浏览" | "缩放与旋转" | "动图" | "窗口" | "文件";
  /** Default chords; the first one is shown in menus and tooltips. */
  defaults: string[];
}

export const SHORTCUT_ACTIONS: readonly ShortcutAction[] = [
  { id: "next", label: "下一张", group: "浏览", defaults: ["ArrowRight", "PageDown"] },
  { id: "previous", label: "上一张", group: "浏览", defaults: ["ArrowLeft", "PageUp"] },
  { id: "first", label: "第一张", group: "浏览", defaults: ["Home"] },
  { id: "last", label: "最后一张", group: "浏览", defaults: ["End"] },
  { id: "slideshow", label: "幻灯片放映", group: "浏览", defaults: ["S", "F5"] },
  { id: "zoomIn", label: "放大", group: "缩放与旋转", defaults: ["=", "Ctrl+="] },
  { id: "zoomOut", label: "缩小", group: "缩放与旋转", defaults: ["-", "Ctrl+-"] },
  { id: "fit", label: "适应窗口", group: "缩放与旋转", defaults: ["0", "Ctrl+0"] },
  { id: "actualSize", label: "原始大小（100%）", group: "缩放与旋转", defaults: ["1", "Ctrl+1"] },
  { id: "rotateRight", label: "向右旋转", group: "缩放与旋转", defaults: ["R"] },
  { id: "rotateLeft", label: "向左旋转", group: "缩放与旋转", defaults: ["Shift+R"] },
  { id: "flipHorizontal", label: "水平翻转", group: "缩放与旋转", defaults: ["H"] },
  { id: "flipVertical", label: "垂直翻转", group: "缩放与旋转", defaults: ["V"] },
  { id: "playPause", label: "播放 / 暂停动图", group: "动图", defaults: ["Space"] },
  { id: "nextFrame", label: "下一帧", group: "动图", defaults: ["."] },
  { id: "previousFrame", label: "上一帧", group: "动图", defaults: [","] },
  { id: "fullscreen", label: "全屏", group: "窗口", defaults: ["F11", "F"] },
  { id: "toggleInfo", label: "显示 / 隐藏图片信息", group: "窗口", defaults: ["I"] },
  { id: "toggleFilmstrip", label: "显示 / 隐藏缩略图条", group: "窗口", defaults: ["T"] },
  { id: "toggleTitlebarPin", label: "固定 / 自动隐藏标题栏", group: "窗口", defaults: ["P"] },
  { id: "showShortcuts", label: "查看快捷键", group: "窗口", defaults: ["Shift+/"] },
  { id: "closeWindow", label: "关闭窗口", group: "窗口", defaults: ["Ctrl+W"] },
  { id: "open", label: "打开图片", group: "文件", defaults: ["Ctrl+O"] },
  { id: "copyImage", label: "复制图片", group: "文件", defaults: ["Ctrl+C"] },
  { id: "copyPath", label: "复制文件路径", group: "文件", defaults: ["Ctrl+Shift+C"] },
  { id: "rename", label: "重命名", group: "文件", defaults: ["F2"] },
  { id: "delete", label: "删除到回收站", group: "文件", defaults: ["Delete"] }
];

export const SHORTCUT_GROUPS = ["浏览", "缩放与旋转", "动图", "窗口", "文件"] as const;

/** Persisted overrides: only actions the user changed are stored. */
export type ShortcutOverrides = Partial<Record<ShortcutActionId, string[]>>;

const MODIFIER_ORDER = ["Ctrl", "Alt", "Shift"] as const;
const NAMED_KEYS: Record<string, string> = {
  " ": "Space", Spacebar: "Space", Esc: "Escape", Del: "Delete",
  Left: "ArrowLeft", Right: "ArrowRight", Up: "ArrowUp", Down: "ArrowDown",
  "+": "=", _: "-"
};
/** Keys reserved by the window itself; they cannot be rebound. */
export const RESERVED_KEYS = new Set(["Escape", "Tab", "Enter"]);

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
    NumpadAdd: "=", NumpadSubtract: "-"
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
  // "+" is written as "=" (same physical key); "Ctrl++" ends with an empty piece.
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

export function resolveBindings(overrides: ShortcutOverrides): Record<ShortcutActionId, string[]> {
  const result = {} as Record<ShortcutActionId, string[]>;
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
    const normalized = chords.map((chord) => normalizeChord(String(chord))).filter((chord): chord is string => Boolean(chord) && !RESERVED_KEYS.has(chord as string));
    result[action.id] = [...new Set(normalized)].slice(0, 4);
  }
  return result;
}

/** Which other action already uses `chord` (for conflict warnings). */
export function findConflict(
  bindings: Record<ShortcutActionId, string[]>,
  chord: string,
  except: ShortcutActionId
): ShortcutActionId | null {
  for (const action of SHORTCUT_ACTIONS) {
    if (action.id !== except && bindings[action.id].includes(chord)) return action.id;
  }
  return null;
}

/** Assign `chord` to `action`, removing it from whichever action had it. */
export function assignChord(
  overrides: ShortcutOverrides,
  action: ShortcutActionId,
  chord: string,
  replaceIndex: number | null
): ShortcutOverrides {
  const bindings = resolveBindings(overrides);
  const next: ShortcutOverrides = { ...overrides };
  const conflict = findConflict(bindings, chord, action);
  if (conflict) next[conflict] = bindings[conflict].filter((value) => value !== chord);
  const current = [...bindings[action]];
  if (replaceIndex !== null && replaceIndex < current.length) current[replaceIndex] = chord;
  else current.push(chord);
  next[action] = [...new Set(current)].slice(0, 4);
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
export function matchAction(bindings: Record<ShortcutActionId, string[]>, event: ChordEvent): ShortcutActionId | null {
  const chord = chordFromEvent(event);
  if (!chord) return null;
  for (const action of SHORTCUT_ACTIONS) {
    if (bindings[action.id].includes(chord)) return action.id;
  }
  return null;
}

const DISPLAY: Record<string, string> = {
  ArrowRight: "→", ArrowLeft: "←", ArrowUp: "↑", ArrowDown: "↓",
  PageDown: "PgDn", PageUp: "PgUp", Delete: "Del", Space: "空格", "=": "+", "-": "−", "/": "/", ".": ".", ",": ","
};

/** Keycaps for a chord, e.g. ["Ctrl", "Shift", "C"]. */
export function chordParts(chord: string): string[] {
  // Shift+/ is what people call "?".
  if (chord === "Shift+/") return ["?"];
  const parts = chord.split("+");
  // "Ctrl+=" splits cleanly; a bare "+" never occurs because "+" is stored as "=".
  return parts.filter(Boolean).map((part) => DISPLAY[part] ?? part);
}

/** Human label for a chord: "Ctrl + Shift + C", "→", "空格". */
export function formatChord(chord: string): string {
  return chordParts(chord).join(" + ");
}
