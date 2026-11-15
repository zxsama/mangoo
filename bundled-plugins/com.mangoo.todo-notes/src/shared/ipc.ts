/**
 * IPC contract between the TODO notes Desktop host (Electron main) and its
 * renderer windows. Every channel is owned by this plugin: main.ts only routes
 * them to the host controller, which authorizes the sender per channel. The
 * hand-written preload (`apps/desktop/src/preload.cjs`) must use exactly these
 * names (a plugin test enforces it).
 */

export const TODO_WINDOW_ROLE = "todo-notes";
export const TODO_ROUTE = "#/todo-notes";
/** One small frameless window per item pinned to the desktop. */
export const TODO_STICKY_WINDOW_ROLE = "todo-sticky";
export const TODO_STICKY_ROUTE = "#/todo-sticky";
/** The standalone settings window (same pattern as the image viewer's). */
export const TODO_SETTINGS_WINDOW_ROLE = "todo-notes-settings";
export const TODO_SETTINGS_ROUTE = "#/todo-notes-settings";
/** Sections of the settings window, in display order. */
export const TODO_SETTINGS_SECTIONS = ["capture", "display", "stickies", "reminders", "system", "shortcuts", "backup"] as const;
export type TodoSettingsSection = (typeof TODO_SETTINGS_SECTIONS)[number];

export const TODO_CHANNELS = {
  /** window -> main (invoke): renderer painted; returns the window's boot state. */
  ready: "mangoo:todo-notes:ready",
  /** main -> windows (send): records changed (local write or synchronized). */
  dataChanged: "mangoo:todo-notes:data-changed",
  /** main -> todo window (send): select / reveal one item. */
  focusItem: "mangoo:todo-notes:focus-item",
  /** main -> todo window (send): focus the quick-add field. */
  quickAdd: "mangoo:todo-notes:quick-add",
  /** todo / sticky -> main (invoke): Desktop settings. */
  getSettings: "mangoo:todo-notes:get-settings",
  setSettings: "mangoo:todo-notes:set-settings",
  /** main -> windows (send): Desktop settings changed. */
  settingsChanged: "mangoo:todo-notes:settings-changed",
  /** todo / sticky -> main (invoke): pin / unpin an item to the desktop. */
  pin: "mangoo:todo-notes:pin",
  unpin: "mangoo:todo-notes:unpin",
  /** todo -> main (invoke): ids of the pinned items. */
  listPinned: "mangoo:todo-notes:list-pinned",
  /** main -> todo window (send): pinned set changed. */
  pinnedChanged: "mangoo:todo-notes:pinned-changed",
  /** todo -> main (invoke): show / hide every pinned note. */
  setStickiesVisible: "mangoo:todo-notes:set-stickies-visible",
  /** sticky -> main (invoke): hover expand / collapse (window resizes). */
  stickyExpand: "mangoo:todo-notes:sticky-expand",
  /** sticky -> main (invoke): the collapsed summary's natural height. */
  stickyMeasure: "mangoo:todo-notes:sticky-measure",
  /** sticky -> main (invoke): per-note appearance. */
  stickyUpdate: "mangoo:todo-notes:sticky-update",
  /** main -> sticky (send): its appearance changed (e.g. from another window). */
  stickyStateChanged: "mangoo:todo-notes:sticky-state-changed",
  /** sticky -> main (invoke): open the item in the TODO window. */
  openItem: "mangoo:todo-notes:open-item",
  /** todo / sticky -> main (invoke): open a web / mail link externally. */
  openLink: "mangoo:todo-notes:open-link",
  /** todo / sticky -> main (invoke): plain text to the clipboard. */
  copyText: "mangoo:todo-notes:copy-text",
  /** todo -> main (invoke): keep the TODO window above others. */
  setAlwaysOnTop: "mangoo:todo-notes:set-always-on-top",
  /** todo -> main (invoke): bring up the main Mangoo window. */
  openMainWindow: "mangoo:todo-notes:open-main-window",
  /** privileged Host windows / tray -> main (invoke): open the TODO window. */
  openWindow: "mangoo:todo-notes:open-window",
  /** privileged Host windows -> main (invoke): status for the plugin panel. */
  status: "mangoo:todo-notes:status",
  /** todo / plugin manager -> main (invoke): open (or focus) the settings window at a section. */
  openSettings: "mangoo:todo-notes:open-settings",
  /** main -> settings (send): scroll to a section (window was already open). */
  settingsSection: "mangoo:todo-notes:settings-section",
  /** settings -> main (invoke): save every note to a backup file (save dialog). */
  exportBackup: "mangoo:todo-notes:export-backup",
  /** settings -> main (invoke): restore notes from a backup file (open dialog). */
  importBackup: "mangoo:todo-notes:import-backup"
} as const;

/** Channels a privileged Host window (main, settings, plugins) may also use. */
export const TODO_PRIVILEGED_CHANNELS: readonly string[] = [
  TODO_CHANNELS.openWindow,
  TODO_CHANNELS.status,
  TODO_CHANNELS.getSettings,
  TODO_CHANNELS.setSettings,
  TODO_CHANNELS.openSettings
];

/** Suggested global quick-add shortcut (any valid accelerator can be recorded). */
export const DEFAULT_QUICK_ADD_SHORTCUT = "CommandOrControl+Alt+N";

const ACCELERATOR_KEY = /^(?:[A-Z0-9]|F(?:[1-9]|1[0-9]|2[0-4])|Space|Insert|Delete|Home|End|PageUp|PageDown|Up|Down|Left|Right|[,./;'\[\]\\`=-])$/;
const ACCELERATOR_MODIFIERS = ["CommandOrControl", "Alt", "Shift"] as const;

/**
 * Canonical Electron accelerator for a global shortcut, or null when it is
 * not one: at least one modifier (a bare function key is allowed), never
 * Shift alone with a printable key (that is just typing).
 */
export function normalizeAccelerator(value: unknown): string | null {
  if (typeof value !== "string" || value.length > 64) return null;
  const pieces = value.split("+").map((piece) => piece.trim()).filter(Boolean);
  const key = pieces.pop();
  if (!key) return null;
  const modifiers = new Set<string>();
  for (const piece of pieces) {
    const lower = piece.toLowerCase();
    const modifier = ["ctrl", "control", "cmd", "command", "commandorcontrol", "cmdorctrl", "meta"].includes(lower) ? "CommandOrControl"
      : lower === "alt" || lower === "option" ? "Alt"
      : lower === "shift" ? "Shift" : null;
    if (!modifier) return null;
    modifiers.add(modifier);
  }
  const named: Record<string, string> = { arrowup: "Up", arrowdown: "Down", arrowleft: "Left", arrowright: "Right", space: "Space", " ": "Space", del: "Delete", delete: "Delete", insert: "Insert", home: "Home", end: "End", pageup: "PageUp", pagedown: "PageDown" };
  const normalizedKey = named[key.toLowerCase()] ?? (key.length === 1 ? key.toUpperCase() : key.charAt(0).toUpperCase() + key.slice(1));
  if (!ACCELERATOR_KEY.test(normalizedKey)) return null;
  const functionKey = /^F\d+$/.test(normalizedKey);
  if (!functionKey && !modifiers.has("CommandOrControl") && !modifiers.has("Alt")) return null;
  return [...ACCELERATOR_MODIFIERS.filter((modifier) => modifiers.has(modifier)), normalizedKey].join("+");
}

/** "CommandOrControl+Alt+N" → "Ctrl+Alt+N" (display / chord form). */
export function acceleratorLabel(accelerator: string): string {
  return accelerator.replace("CommandOrControl", "Ctrl").replace("Space", "空格");
}

export interface TodoDesktopSettings {
  /** Show the notification-area icon (closing the window keeps running). */
  trayIcon: boolean;
  /** Closing the TODO window hides it to the tray instead of closing. */
  closeToTray: boolean;
  /** Start Mangoo at login with only the tray icon and pinned notes. */
  launchAtLogin: boolean;
  /** Global quick-add accelerator, or null. */
  quickAddShortcut: string | null;
  /** Native notification when a reminder is due. */
  reminders: boolean;
  /** Pinned notes stay above other windows. */
  stickiesOnTop: boolean;
  /** Pinned notes are hidden (tray: 显示 / 隐藏贴纸). */
  stickiesHidden: boolean;
  /** Collapsed notes expand when hovered. */
  expandOnHover: boolean;
}

export const DEFAULT_TODO_SETTINGS: TodoDesktopSettings = {
  trayIcon: true,
  closeToTray: true,
  launchAtLogin: false,
  quickAddShortcut: null,
  reminders: true,
  stickiesOnTop: true,
  stickiesHidden: false,
  expandOnHover: true
};

export const STICKY_COLORS = ["glass", "yellow", "green", "blue", "pink", "purple"] as const;
export type StickyColor = (typeof STICKY_COLORS)[number];

/** Device-local appearance of one pinned note (never synchronized). */
export interface StickyAppearance {
  color: StickyColor;
  /** Always show details (no hover collapse). */
  expanded: boolean;
}

export const DEFAULT_STICKY_APPEARANCE: StickyAppearance = { color: "glass", expanded: false };

export interface TodoStatus {
  pluginEnabled: boolean;
  windowOpen: boolean;
  pinnedCount: number;
  canLaunchAtLogin: boolean;
  /** The global quick-add shortcut is set but another application holds it. */
  quickAddShortcutTaken: boolean;
  settings: TodoDesktopSettings;
}

export interface TodoBootState {
  role: "todo" | "sticky";
  /** Sticky only: the pinned item. */
  itemId: string | null;
  appearance: StickyAppearance | null;
  settings: TodoDesktopSettings;
  /** Todo window only: an item to reveal on open. */
  focusItemId: string | null;
  quickAdd: boolean;
}

export interface TodoDataChange {
  revision: number;
}

/** Outcome of an export / import (cancelled = the user closed the dialog). */
export interface TodoBackupResult {
  ok: boolean;
  cancelled?: boolean;
  error?: string;
  /** Saved / restored counts. */
  lists?: number;
  notes?: number;
  subtasks?: number;
  images?: number;
  /** Export: where the file was written. */
  path?: string;
}

export interface TodoActionResult {
  ok: boolean;
  error?: string;
}

/** Bridge exposed to the TODO window and pinned-note windows. */
export interface TodoDesktopBridge {
  ready(): Promise<TodoBootState>;
  onDataChanged(callback: (change: TodoDataChange) => void): () => void;
  onFocusItem(callback: (itemId: string) => void): () => void;
  onQuickAdd(callback: () => void): () => void;
  getSettings(): Promise<TodoDesktopSettings>;
  setSettings(patch: Partial<TodoDesktopSettings>): Promise<TodoDesktopSettings>;
  onSettingsChanged(callback: (settings: TodoDesktopSettings) => void): () => void;
  pin(itemId: string): Promise<TodoActionResult>;
  unpin(itemId: string): Promise<TodoActionResult>;
  listPinned(): Promise<string[]>;
  onPinnedChanged(callback: (itemIds: string[]) => void): () => void;
  setStickiesVisible(visible: boolean): Promise<boolean>;
  stickyExpand(expanded: boolean): Promise<void>;
  /** Summary height and full content height (an expanded note hugs short content). */
  stickyMeasure(collapsedHeight: number, contentHeight?: number): Promise<void>;
  stickyUpdate(patch: Partial<StickyAppearance>): Promise<StickyAppearance>;
  onStickyStateChanged(callback: (appearance: StickyAppearance) => void): () => void;
  openItem(itemId: string): Promise<boolean>;
  openLink(url: string): Promise<TodoActionResult>;
  copyText(text: string): Promise<TodoActionResult>;
  setAlwaysOnTop(value: boolean): Promise<boolean>;
  openMainWindow(): Promise<boolean>;
  openSettings(section?: TodoSettingsSection): Promise<boolean>;
}

/** Bridge of the standalone settings window. */
export interface TodoSettingsWindowBridge {
  getSettings(): Promise<TodoDesktopSettings>;
  setSettings(patch: Partial<TodoDesktopSettings>): Promise<TodoDesktopSettings>;
  onSettingsChanged(callback: (settings: TodoDesktopSettings) => void): () => void;
  onSettingsSection(callback: (section: TodoSettingsSection) => void): () => void;
  status(): Promise<TodoStatus>;
  exportBackup(): Promise<TodoBackupResult>;
  importBackup(): Promise<TodoBackupResult>;
}

/** Bridge exposed to privileged Host windows (plugin manager). */
export interface TodoHostWindowBridge {
  openWindow(): Promise<boolean>;
  openSettings(section?: TodoSettingsSection): Promise<boolean>;
  status(): Promise<TodoStatus>;
  getSettings(): Promise<TodoDesktopSettings>;
  setSettings(patch: Partial<TodoDesktopSettings>): Promise<TodoDesktopSettings>;
}

const ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function isRecordId(value: unknown): value is string {
  return typeof value === "string" && ID.test(value);
}

/** Strict merge of a renderer-supplied settings patch. */
export function mergeSettings(current: TodoDesktopSettings, patch: unknown): TodoDesktopSettings {
  const next = { ...current };
  if (!patch || typeof patch !== "object") return next;
  const value = patch as Record<string, unknown>;
  for (const key of ["trayIcon", "closeToTray", "launchAtLogin", "reminders", "stickiesOnTop", "stickiesHidden", "expandOnHover"] as const) {
    if (typeof value[key] === "boolean") next[key] = value[key] as boolean;
  }
  if (value.quickAddShortcut === null) next.quickAddShortcut = null;
  else {
    const accelerator = normalizeAccelerator(value.quickAddShortcut);
    if (accelerator) next.quickAddShortcut = accelerator;
  }
  // Without a tray icon nothing could bring a hidden window back.
  if (!next.trayIcon) next.closeToTray = false;
  return next;
}

export function mergeAppearance(current: StickyAppearance, patch: unknown): StickyAppearance {
  const next = { ...current };
  if (!patch || typeof patch !== "object") return next;
  const value = patch as Record<string, unknown>;
  if (typeof value.color === "string" && (STICKY_COLORS as readonly string[]).includes(value.color)) next.color = value.color as StickyColor;
  if (typeof value.expanded === "boolean") next.expanded = value.expanded;
  return next;
}
