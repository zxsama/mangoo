import fs from "node:fs";
import path from "node:path";

import type { PluginRecordOperation } from "@mangoo/core-client/node";
import type { BrowserWindow, IpcMainInvokeEvent, Rectangle, Tray } from "electron";
import { createCoreClient } from "@mangoo/core-client/node";

import {
  MAX_PINNED,
  STICKY_HEIGHT,
  STICKY_WIDTH,
  TodoStateStore,
  nextStickyAnchor,
  stickyBounds,
  visibleWindowBounds,
  type PinnedNote,
  type Rect
} from "./desktopState.ts";
import { dueReminders, nextReminderAt, pruneFired, todayCount } from "./reminders.ts";
import {
  DEFAULT_STICKY_APPEARANCE,
  TODO_CHANNELS,
  TODO_ROUTE,
  TODO_SETTINGS_ROUTE,
  TODO_SETTINGS_SECTIONS,
  TODO_SETTINGS_WINDOW_ROLE,
  TODO_STICKY_ROUTE,
  TODO_STICKY_WINDOW_ROLE,
  TODO_WINDOW_ROLE,
  isRecordId,
  mergeAppearance,
  mergeSettings,
  type StickyAppearance,
  type TodoActionResult,
  type TodoBackupResult,
  type TodoBootState,
  type TodoDesktopSettings,
  type TodoStatus
} from "../shared/ipc.ts";
import {
  COLLECTION_ITEMS,
  TODO_CAPABILITY,
  TODO_PLUGIN_ID,
  formatDueLabel,
  isSafeUrl,
  itemFromRecord,
  titleText,
  toDateKey,
  type TodoItem
} from "../shared/model.ts";
import { BACKUP_COLLECTIONS, backupFileName, createBackup, parseBackup, summarizeBackup, type BackupRecord } from "../shared/backup.ts";

type ElectronModule = typeof import("electron");

export interface TodoCoreSession {
  baseUrl: string;
  sessionToken: string;
}

export interface TodoNotesHostOptions {
  /** Directory for `todo-notes.json` (Electron userData). */
  userDataDir: string;
  preloadPath: string;
  windowIconPath: string | null;
  /** Notification-area icon (ICO on Windows, PNG elsewhere). */
  trayIconPath: string | null;
  /** Healthy Core session (starts Core when needed). */
  ensureCoreSession(): Promise<TodoCoreSession>;
  /** URL of the shared web bundle for `route` under `session`. */
  rendererUrl(session: TodoCoreSession, route: string): string;
  /** Host material (acrylic) application, identical to other Host windows. */
  applyWindowMaterial(window: BrowserWindow): void;
  /** Show or create the main Library window. */
  openMainWindow(): Promise<void>;
  /** Quit the whole application (tray "退出 Mangoo"). */
  quitApplication(): void;
  /** Whether a non-plugin sender is a privileged Host window for `channel`. */
  authorizeHostWindow(event: IpcMainInvokeEvent, channel: string): boolean;
  /** Extra renderer switches (e.g. the visual harness flag). */
  extraRendererArguments?: () => string[];
  log(level: "info" | "warn" | "error", message: string, data?: Record<string, unknown>): void;
}

export interface TodoNotesHost {
  /** Called once Core is healthy at startup. */
  start(): Promise<{ enabled: boolean }>;
  /** Re-read the plugin lifecycle (after any plugin enable/disable/grant). */
  refresh(): Promise<void>;
  /** The tray keeps Mangoo running after its last window closes. */
  keepsAppAlive(): boolean;
  ownsWindow(window: BrowserWindow | null | undefined): boolean;
  isEnabled(): boolean;
  /** Show the TODO window (tray, plugin panel, second launch). */
  openTodoWindow(): Promise<boolean>;
  /** Called on app quit: persist state, release OS integrations. */
  dispose(): void;
}

/** Launch switch for a login-item start: tray and pinned notes only. */
export const BACKGROUND_LAUNCH_SWITCH = "--mangoo-background";

const READY_FALLBACK_MS = 4000;
/** A notes column beside your work: wide enough for one property row. */
const PORTRAIT_SIZE = { width: 400, height: 680 };
const FEED_WAIT_MS = 25_000;
const FEED_RETRY_MS = [1_000, 3_000, 8_000, 15_000];
const DISPLAY_NAME = "Mangoo 便笺";

interface StickyEntry {
  window: BrowserWindow;
  itemId: string;
  expanded: boolean;
  collapsedHeight: number;
  /** Natural height of the expanded content (0 = unknown). */
  contentHeight: number;
  /** note.y - applied window y (non-zero when the note grew upward). */
  offsetY: number;
  /** Bounds this host last applied; resize events matching it are ours. */
  applied: Rect | null;
  ready: boolean;
}

/**
 * Windows drops a window's acrylic to a flat fill when it loses focus. Notes
 * are glanced at while you work in another app, so every plugin window keeps
 * its frosted backdrop by re-applying the material synchronously inside
 * `blur`, before the inactive transition starts. Measured on Win11: deferring
 * it (or resetting to "none" first) lets the fade run for ~80 ms and snap
 * back, which reads as a flicker; the synchronous call shows no change.
 */
function keepMaterialWhenInactive(window: BrowserWindow, apply: (window: BrowserWindow) => void): void {
  window.on("blur", () => {
    if (!window.isDestroyed()) apply(window);
  });
}

function sameRect(a: Rect | null, b: Rect): boolean {
  return Boolean(a && Math.abs(a.x - b.x) <= 1 && Math.abs(a.y - b.y) <= 1 && Math.abs(a.width - b.width) <= 1 && Math.abs(a.height - b.height) <= 1);
}

export function createTodoNotesHost(electron: ElectronModule, options: TodoNotesHostOptions): TodoNotesHost {
  const { app, BrowserWindow, Menu, Notification, Tray, clipboard, dialog, globalShortcut, ipcMain, nativeImage, screen, shell } = electron;
  const store = new TodoStateStore(path.join(options.userDataDir, "todo-notes.json"));

  let enabled = false;
  let started = false;
  let quitting = false;
  let todoWindow: BrowserWindow | null = null;
  let settingsWindow: BrowserWindow | null = null;
  let todoReady = false;
  let pendingFocusItem: string | null = null;
  let pendingQuickAdd = false;
  const stickies = new Map<string, StickyEntry>();
  let tray: Tray | null = null;
  let registeredShortcut: string | null = null;
  let refreshQueue: Promise<void> = Promise.resolve();
  let persistTimer: ReturnType<typeof setTimeout> | null = null;

  // Change feed + cached items (tray count, reminders, pinned-note pruning).
  let revision = -1;
  let sharedRevision = -1;
  let items: TodoItem[] = [];
  let feedGeneration = 0;
  let reminderTimer: ReturnType<typeof setTimeout> | null = null;
  let dayTimer: ReturnType<typeof setTimeout> | null = null;

  const settings = () => store.get().settings;

  async function coreClient() {
    const session = await options.ensureCoreSession();
    return createCoreClient({ baseUrl: session.baseUrl, sessionToken: session.sessionToken });
  }

  async function readPluginEnabled(): Promise<boolean | null> {
    try {
      const plugin = await (await coreClient()).getPlugin(TODO_PLUGIN_ID);
      return plugin.enabledIntent && plugin.runtimeState === "ready" && plugin.capabilities.includes(TODO_CAPABILITY);
    } catch (error) {
      const status = (error as { status?: number }).status;
      if (status === 404) return false;
      options.log("warn", "todo notes plugin state unavailable", { error: String(error) });
      return null;
    }
  }

  function pluginWindows(): BrowserWindow[] {
    const windows: BrowserWindow[] = [];
    if (todoWindow && !todoWindow.isDestroyed()) windows.push(todoWindow);
    if (settingsWindow && !settingsWindow.isDestroyed()) windows.push(settingsWindow);
    for (const entry of stickies.values()) if (!entry.window.isDestroyed()) windows.push(entry.window);
    return windows;
  }

  function broadcast(channel: string, payload: unknown): void {
    for (const window of pluginWindows()) window.webContents.send(channel, payload);
  }

  function rendererArguments(session: TodoCoreSession, role: string, extra: string[] = []): string[] {
    return [
      `--mangoo-core-url=${session.baseUrl}`,
      `--mangoo-session-token=${session.sessionToken}`,
      `--mangoo-window=${role}`,
      ...extra,
      ...(options.extraRendererArguments?.() ?? [])
    ];
  }

  function workAreas(): Rect[] {
    const primary = screen.getPrimaryDisplay().workArea;
    return [primary, ...screen.getAllDisplays().map((display) => display.workArea).filter((area) => area.x !== primary.x || area.y !== primary.y)];
  }

  // ------------------------------------------------------------ TODO window

  function persistTodoWindow(): void {
    if (!todoWindow || todoWindow.isDestroyed() || todoWindow.isFullScreen()) return;
    const maximized = todoWindow.isMaximized();
    const bounds: Rectangle = maximized ? todoWindow.getNormalBounds() : todoWindow.getBounds();
    const alwaysOnTop = todoWindow.isAlwaysOnTop();
    store.update((state) => {
      state.window = { bounds: { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height }, maximized, alwaysOnTop };
    });
  }

  function schedulePersist(): void {
    if (persistTimer) clearTimeout(persistTimer);
    persistTimer = setTimeout(() => {
      persistTimer = null;
      persistTodoWindow();
    }, 400);
  }

  function revealTodo(window: BrowserWindow): void {
    if (window.isDestroyed()) return;
    if (!window.isVisible()) {
      window.show();
      if (store.get().window?.maximized) window.maximize();
    }
    if (window.isMinimized()) window.restore();
    window.focus();
  }

  async function createTodoWindow(): Promise<BrowserWindow> {
    const session = await options.ensureCoreSession();
    const saved = store.get().window;
    // Portrait by default: a notes column beside your work, not a dashboard.
    const bounds = visibleWindowBounds(saved?.bounds ?? null, workAreas(), PORTRAIT_SIZE);
    const window = new BrowserWindow({
      title: DISPLAY_NAME,
      ...bounds,
      minWidth: 320,
      minHeight: 440,
      backgroundMaterial: "acrylic",
      transparent: false,
      backgroundColor: "#00000000",
      roundedCorners: true,
      ...(options.windowIconPath ? { icon: options.windowIconPath } : {}),
      show: false,
      frame: false,
      autoHideMenuBar: true,
      webPreferences: {
        preload: options.preloadPath,
        contextIsolation: true,
        nodeIntegration: false,
        additionalArguments: rendererArguments(session, TODO_WINDOW_ROLE)
      }
    });
    todoWindow = window;
    todoReady = false;
    if (saved?.alwaysOnTop) window.setAlwaysOnTop(true);
    options.applyWindowMaterial(window);
    const fallback = setTimeout(() => revealTodo(window), READY_FALLBACK_MS);
    window.once("ready-to-show", () => options.applyWindowMaterial(window));
    keepMaterialWhenInactive(window, options.applyWindowMaterial);
    for (const event of ["move", "resize", "maximize", "unmaximize"] as const) window.on(event as "move", schedulePersist);
    window.on("close", (event) => {
      persistTodoWindow();
      if (!quitting && enabled && settings().closeToTray && tray) {
        event.preventDefault();
        window.hide();
      }
    });
    window.on("closed", () => {
      clearTimeout(fallback);
      if (todoWindow === window) {
        todoWindow = null;
        todoReady = false;
      }
    });
    window.webContents.on("render-process-gone", (_event, details) => {
      options.log("error", "todo notes renderer exited", { reason: details.reason });
      if (!window.isDestroyed()) window.destroy();
    });
    window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
    window.webContents.on("will-navigate", (event) => event.preventDefault());
    (window as BrowserWindow & { __mangooTodoFallback?: ReturnType<typeof setTimeout> }).__mangooTodoFallback = fallback;
    await window.loadURL(options.rendererUrl(session, TODO_ROUTE));
    return window;
  }

  async function openTodoWindow(focusItemId: string | null = null, quickAdd = false): Promise<boolean> {
    if (!enabled) return false;
    if (todoWindow && !todoWindow.isDestroyed()) {
      if (todoReady) {
        if (focusItemId) todoWindow.webContents.send(TODO_CHANNELS.focusItem, focusItemId);
        if (quickAdd) todoWindow.webContents.send(TODO_CHANNELS.quickAdd, null);
      } else {
        pendingFocusItem = focusItemId ?? pendingFocusItem;
        pendingQuickAdd = pendingQuickAdd || quickAdd;
      }
      revealTodo(todoWindow);
      return true;
    }
    pendingFocusItem = focusItemId;
    pendingQuickAdd = quickAdd;
    try {
      await createTodoWindow();
      return true;
    } catch (error) {
      options.log("error", "todo notes window failed to open", { error: String(error) });
      return false;
    }
  }

  /**
   * Settings as their own frameless acrylic window (like the image viewer's):
   * they sit beside the narrow notes column instead of covering it.
   */
  async function openSettingsWindow(section: string): Promise<boolean> {
    if (!enabled) return false;
    const target = (TODO_SETTINGS_SECTIONS as readonly string[]).includes(section) ? section : "capture";
    if (settingsWindow && !settingsWindow.isDestroyed()) {
      settingsWindow.webContents.send(TODO_CHANNELS.settingsSection, target);
      if (settingsWindow.isMinimized()) settingsWindow.restore();
      settingsWindow.show();
      settingsWindow.focus();
      return true;
    }
    const session = await options.ensureCoreSession();
    const width = 720;
    const height = 560;
    const anchor = todoWindow && !todoWindow.isDestroyed() && todoWindow.isVisible() ? todoWindow.getBounds() : null;
    const area = anchor ? screen.getDisplayMatching(anchor).workArea : screen.getPrimaryDisplay().workArea;
    // Beside the notes column when there is room, otherwise centred.
    let position: { x: number; y: number } | null = null;
    if (anchor) {
      const y = Math.min(Math.max(area.y, anchor.y), area.y + area.height - height);
      const right = anchor.x + anchor.width + 12;
      const left = anchor.x - width - 12;
      if (right + width <= area.x + area.width) position = { x: right, y };
      else if (left >= area.x) position = { x: left, y };
    }
    position ??= { x: Math.round(area.x + (area.width - width) / 2), y: Math.round(area.y + (area.height - height) / 2) };
    const window = new BrowserWindow({
      title: "便笺设置",
      width,
      height,
      ...position,
      minWidth: 600,
      minHeight: 420,
      maximizable: false,
      fullscreenable: false,
      backgroundMaterial: "acrylic",
      transparent: false,
      backgroundColor: "#00000000",
      roundedCorners: true,
      ...(options.windowIconPath ? { icon: options.windowIconPath } : {}),
      show: false,
      frame: false,
      autoHideMenuBar: true,
      webPreferences: {
        preload: options.preloadPath,
        contextIsolation: true,
        nodeIntegration: false,
        // No Core URL / session token: settings never talk to Core.
        additionalArguments: [`--mangoo-window=${TODO_SETTINGS_WINDOW_ROLE}`, ...(options.extraRendererArguments?.() ?? [])]
      }
    });
    settingsWindow = window;
    options.applyWindowMaterial(window);
    window.once("ready-to-show", () => {
      options.applyWindowMaterial(window);
      window.show();
    });
    keepMaterialWhenInactive(window, options.applyWindowMaterial);
    window.on("closed", () => {
      if (settingsWindow === window) settingsWindow = null;
    });
    window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
    window.webContents.on("will-navigate", (event) => event.preventDefault());
    await window.loadURL(options.rendererUrl(session, `${TODO_SETTINGS_ROUTE}/${target}`));
    return true;
  }

  function toggleTodoWindow(): void {
    if (todoWindow && !todoWindow.isDestroyed() && todoWindow.isVisible() && todoWindow.isFocused()) {
      todoWindow.hide();
      return;
    }
    void openTodoWindow();
  }

  // ---------------------------------------------------------- pinned notes

  function pinnedMap(): Record<string, PinnedNote> {
    return store.get().pinned;
  }

  function pinnedEntries(): Array<{ itemId: string; note: PinnedNote }> {
    return Object.entries(pinnedMap())
      .map(([itemId, note]) => ({ itemId, note }))
      .sort((a, b) => a.note.pinnedAt - b.note.pinnedAt);
  }

  function pinnedIds(): string[] {
    return pinnedEntries().map((entry) => entry.itemId);
  }

  function updateNote(itemId: string, change: (note: PinnedNote) => void): PinnedNote | null {
    let result: PinnedNote | null = null;
    store.update((state) => {
      const note = state.pinned[itemId];
      if (!note) return;
      change(note);
      result = note;
    });
    return result;
  }

  function applyStickyBounds(entry: StickyEntry): void {
    const note = pinnedMap()[entry.itemId];
    if (!note || entry.window.isDestroyed()) return;
    const expanded = entry.expanded || note.appearance.expanded;
    // Expanded notes hug short content; the saved height is the cap.
    const height = expanded
      ? entry.contentHeight > 0 ? Math.min(note.expandedHeight, Math.max(entry.collapsedHeight, entry.contentHeight)) : note.expandedHeight
      : entry.collapsedHeight;
    const bounds = stickyBounds(note, height, workAreas());
    entry.offsetY = note.y - bounds.y;
    entry.applied = bounds;
    entry.window.setBounds(bounds);
  }

  function onStickyMoved(entry: StickyEntry): void {
    if (entry.window.isDestroyed()) return;
    const bounds = entry.window.getBounds();
    if (sameRect(entry.applied, bounds)) return;
    const expanded = entry.expanded || pinnedMap()[entry.itemId]?.appearance.expanded;
    entry.applied = bounds;
    updateNote(entry.itemId, (note) => {
      note.x = bounds.x;
      note.y = bounds.y + entry.offsetY;
      note.width = Math.min(STICKY_WIDTH.max, Math.max(STICKY_WIDTH.min, bounds.width));
      if (expanded && bounds.height >= STICKY_HEIGHT.expandedMin) note.expandedHeight = Math.min(STICKY_HEIGHT.expandedMax, bounds.height);
    });
  }

  async function openSticky(itemId: string): Promise<void> {
    if (stickies.has(itemId) || settings().stickiesHidden) return;
    const note = pinnedMap()[itemId];
    if (!note) return;
    const session = await options.ensureCoreSession();
    if (!pinnedMap()[itemId] || stickies.has(itemId)) return;
    const collapsedHeight = 64;
    const bounds = stickyBounds(note, note.appearance.expanded ? note.expandedHeight : collapsedHeight, workAreas());
    const window = new BrowserWindow({
      title: "便笺",
      ...bounds,
      minWidth: STICKY_WIDTH.min,
      minHeight: STICKY_HEIGHT.collapsedMin,
      maxWidth: STICKY_WIDTH.max,
      backgroundMaterial: "acrylic",
      transparent: false,
      backgroundColor: "#00000000",
      roundedCorners: true,
      ...(options.windowIconPath ? { icon: options.windowIconPath } : {}),
      show: false,
      frame: false,
      skipTaskbar: true,
      minimizable: false,
      maximizable: false,
      fullscreenable: false,
      alwaysOnTop: settings().stickiesOnTop,
      autoHideMenuBar: true,
      webPreferences: {
        preload: options.preloadPath,
        contextIsolation: true,
        nodeIntegration: false,
        additionalArguments: rendererArguments(session, TODO_STICKY_WINDOW_ROLE, [`--mangoo-todo-item=${itemId}`])
      }
    });
    const entry: StickyEntry = { window, itemId, expanded: false, collapsedHeight, contentHeight: 0, offsetY: note.y - bounds.y, applied: bounds, ready: false };
    stickies.set(itemId, entry);
    if (settings().stickiesOnTop) window.setAlwaysOnTop(true, "floating");
    options.applyWindowMaterial(window);
    const fallback = setTimeout(() => {
      if (!window.isDestroyed() && !window.isVisible()) window.showInactive();
    }, READY_FALLBACK_MS);
    window.once("ready-to-show", () => options.applyWindowMaterial(window));
    keepMaterialWhenInactive(window, options.applyWindowMaterial);
    window.on("move", () => onStickyMoved(entry));
    window.on("resize", () => onStickyMoved(entry));
    window.on("closed", () => {
      clearTimeout(fallback);
      if (stickies.get(itemId) === entry) stickies.delete(itemId);
    });
    window.webContents.on("render-process-gone", () => {
      if (!window.isDestroyed()) window.destroy();
    });
    window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
    window.webContents.on("will-navigate", (event) => event.preventDefault());
    (window as BrowserWindow & { __mangooTodoFallback?: ReturnType<typeof setTimeout> }).__mangooTodoFallback = fallback;
    try {
      await window.loadURL(options.rendererUrl(session, `${TODO_STICKY_ROUTE}/${itemId}`));
    } catch (error) {
      options.log("warn", "pinned note failed to load", { error: String(error) });
    }
  }

  function closeSticky(itemId: string): void {
    const entry = stickies.get(itemId);
    stickies.delete(itemId);
    if (entry && !entry.window.isDestroyed()) entry.window.destroy();
  }

  function closeAllStickies(): void {
    for (const itemId of [...stickies.keys()]) closeSticky(itemId);
  }

  /** Make the open notes match the pins. */
  async function reconcileStickies(): Promise<void> {
    const wanted = new Set(enabled && !settings().stickiesHidden ? pinnedIds() : []);
    for (const itemId of [...stickies.keys()]) if (!wanted.has(itemId)) closeSticky(itemId);
    for (const itemId of wanted) if (!stickies.has(itemId)) await openSticky(itemId);
  }

  function pinItem(itemId: string): TodoActionResult {
    if (!items.some((item) => item.id === itemId && !item.deleted)) return { ok: false, error: "事项不存在" };
    if (pinnedMap()[itemId]) {
      if (settings().stickiesHidden) setStickiesHidden(false);
      const entry = stickies.get(itemId);
      if (entry && !entry.window.isDestroyed()) entry.window.showInactive();
      return { ok: true };
    }
    if (pinnedEntries().length >= MAX_PINNED) return { ok: false, error: `最多可贴 ${MAX_PINNED} 条便笺` };
    const anchor = nextStickyAnchor(pinnedEntries().map((entry) => entry.note), screen.getPrimaryDisplay().workArea);
    store.update((state) => {
      state.pinned[itemId] = { ...anchor, width: STICKY_WIDTH.default, expandedHeight: STICKY_HEIGHT.expandedDefault, appearance: { ...DEFAULT_STICKY_APPEARANCE }, pinnedAt: Date.now() };
    });
    if (settings().stickiesHidden) setStickiesHidden(false);
    else void openSticky(itemId);
    publishPinned();
    return { ok: true };
  }

  function unpinItem(itemId: string): TodoActionResult {
    store.update((state) => {
      delete state.pinned[itemId];
    });
    closeSticky(itemId);
    publishPinned();
    return { ok: true };
  }

  function publishPinned(): void {
    todoWindow?.webContents.send(TODO_CHANNELS.pinnedChanged, pinnedIds());
    updateTray();
  }

  function setStickiesHidden(hidden: boolean): void {
    applySettings({ stickiesHidden: hidden });
  }

  // ----------------------------------------------------------- change feed

  async function loadItems(): Promise<TodoItem[]> {
    const client = await coreClient();
    const shared = await client.listSharedPluginRecords(TODO_PLUGIN_ID, { collection: COLLECTION_ITEMS }, { requestPriority: "background" });
    return shared.items.map(itemFromRecord);
  }

  async function runFeed(generation: number): Promise<void> {
    let failures = 0;
    while (enabled && generation === feedGeneration) {
      try {
        const client = await coreClient();
        const change = await client.waitPluginRecordChanges(
          TODO_PLUGIN_ID,
          { afterRevision: revision, afterSharedRevision: sharedRevision, waitMs: FEED_WAIT_MS },
          { requestPriority: "background" }
        );
        if (generation !== feedGeneration) return;
        failures = 0;
        if (!change.changed && revision >= 0) continue;
        // Library switches also move `revision`; only the shared scope holds notes.
        const first = sharedRevision < 0;
        revision = change.revision;
        if (!first && change.sharedRevision === sharedRevision) continue;
        sharedRevision = change.sharedRevision;
        items = await loadItems();
        if (generation !== feedGeneration) return;
        prunePinned();
        if (first) {
          await reconcileStickies();
          publishPinned();
        }
        broadcast(TODO_CHANNELS.dataChanged, { revision: sharedRevision });
        updateTray();
        scheduleReminders();
      } catch (error) {
        if (generation !== feedGeneration) return;
        const status = (error as { status?: number }).status;
        if (status === 403) {
          // The plugin was turned off underneath us: settle the lifecycle.
          void refresh();
          return;
        }
        if (failures === 0) {
          options.log("warn", "todo notes change feed interrupted", { error: String(error) });
        }
        const delay = FEED_RETRY_MS[Math.min(failures, FEED_RETRY_MS.length - 1)];
        failures += 1;
        await new Promise((resolve) => setTimeout(resolve, delay));
      }
    }
  }

  function startFeed(): void {
    feedGeneration += 1;
    revision = -1;
    sharedRevision = -1;
    void runFeed(feedGeneration);
  }

  function stopFeed(): void {
    feedGeneration += 1;
  }

  /** A pinned item that no longer exists (or was deleted) loses its note. */
  function prunePinned(): void {
    const alive = new Set(items.filter((item) => !item.deleted).map((item) => item.id));
    const gone = pinnedIds().filter((itemId) => !alive.has(itemId));
    if (gone.length === 0) return;
    store.update((state) => {
      for (const itemId of gone) delete state.pinned[itemId];
    });
    for (const itemId of gone) closeSticky(itemId);
    publishPinned();
  }

  // -------------------------------------------------------------- reminders

  function fireReminders(): void {
    if (!enabled) return;
    const now = Date.now();
    const bookkeeping = store.get().reminders;
    const due = settings().reminders ? dueReminders(items, bookkeeping.lastCheck, now, bookkeeping.fired) : [];
    const today = toDateKey(new Date());
    for (const item of due.slice(0, 5)) {
      if (!Notification.isSupported()) break;
      const body = item.dueDate ? `截止 ${formatDueLabel(item.dueDate, item.dueTime, today)}` : "提醒";
      const notification = new Notification({
        title: titleText(item.title) || "未命名事项",
        body,
        silent: false,
        ...(options.windowIconPath ? { icon: options.windowIconPath } : {})
      });
      notification.on("click", () => void openTodoWindow(item.id));
      notification.show();
    }
    store.update((state) => {
      const fired = pruneFired(items, state.reminders.fired);
      for (const item of due) fired[item.id] = item.remindAt as number;
      state.reminders = { lastCheck: now, fired };
    });
  }

  function scheduleReminders(): void {
    if (reminderTimer) clearTimeout(reminderTimer);
    reminderTimer = null;
    if (!enabled) return;
    fireReminders();
    const next = nextReminderAt(items, Date.now());
    if (next === null) return;
    // Re-arm at most hourly so sleep / clock changes cannot strand a timer.
    const delay = Math.min(Math.max(250, next - Date.now() + 50), 60 * 60 * 1000);
    reminderTimer = setTimeout(scheduleReminders, delay);
  }

  /** Midnight rolls "today": refresh the tray count. */
  function scheduleDayRollover(): void {
    if (dayTimer) clearTimeout(dayTimer);
    const now = new Date();
    const midnight = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1, 0, 0, 5);
    dayTimer = setTimeout(() => {
      updateTray();
      broadcast(TODO_CHANNELS.dataChanged, { revision: sharedRevision });
      scheduleDayRollover();
    }, Math.min(midnight.getTime() - now.getTime(), 6 * 60 * 60 * 1000));
  }

  // ------------------------------------------------------------------ tray

  function trayMenu() {
    const count = todayCount(items, toDateKey(new Date()));
    const hidden = settings().stickiesHidden;
    const pinnedCount = pinnedIds().length;
    return Menu.buildFromTemplate([
      { label: count > 0 ? `今天待办 ${count} 项` : "今天没有待办", enabled: false },
      { type: "separator" },
      { label: "打开便笺", click: () => void openTodoWindow() },
      { label: "新建事项…", click: () => void openTodoWindow(null, true) },
      {
        label: hidden ? `显示贴到桌面的便笺（${pinnedCount}）` : `隐藏贴到桌面的便笺（${pinnedCount}）`,
        enabled: pinnedCount > 0,
        click: () => setStickiesHidden(!hidden)
      },
      { type: "separator" },
      { label: "便笺设置…", click: () => void openSettingsWindow("capture") },
      { label: "打开 Mangoo", click: () => void options.openMainWindow() },
      { label: "退出 Mangoo", click: () => options.quitApplication() }
    ]);
  }

  function updateTray(): void {
    const wanted = enabled && settings().trayIcon && Boolean(options.trayIconPath);
    if (!wanted) {
      if (tray && !tray.isDestroyed()) tray.destroy();
      tray = null;
      return;
    }
    if (!tray || tray.isDestroyed()) {
      const source = nativeImage.createFromPath(options.trayIconPath as string);
      if (source.isEmpty()) {
        options.log("warn", "todo notes tray icon could not be loaded", { path: options.trayIconPath });
        return;
      }
      // 16 px at 1x plus 2x / 3x representations keep the icon crisp on
      // high-DPI notification areas instead of a downsampled app icon.
      const image = nativeImage.createEmpty();
      for (const scaleFactor of [1, 2, 3]) {
        const size = 16 * scaleFactor;
        image.addRepresentation({ scaleFactor, width: size, height: size, buffer: source.resize({ width: size, height: size, quality: "best" }).toPNG() });
      }
      tray = new Tray(image);
      tray.on("click", () => toggleTodoWindow());
      tray.on("double-click", () => void openTodoWindow());
    }
    const count = todayCount(items, toDateKey(new Date()));
    tray.setToolTip(count > 0 ? `${DISPLAY_NAME} · 今天 ${count} 项待办` : DISPLAY_NAME);
    tray.setContextMenu(trayMenu());
  }

  // ------------------------------------------------- OS-level integrations

  function canLaunchAtLogin(): boolean {
    return app.isPackaged && (process.platform === "win32" || process.platform === "darwin");
  }

  function applyLoginItem(): void {
    if (!canLaunchAtLogin()) return;
    const want = enabled && settings().launchAtLogin;
    const state = store.get();
    try {
      if (want) {
        app.setLoginItemSettings({ openAtLogin: true, args: [BACKGROUND_LAUNCH_SWITCH] });
        if (!state.loginItemSet) store.update((next) => void (next.loginItemSet = true));
      } else if (state.loginItemSet) {
        // Only remove the login item this plugin created.
        app.setLoginItemSettings({ openAtLogin: false, args: [BACKGROUND_LAUNCH_SWITCH] });
        store.update((next) => void (next.loginItemSet = false));
      }
    } catch (error) {
      options.log("warn", "todo notes login item update failed", { error: String(error) });
    }
  }

  function applyShortcut(): void {
    const wanted = enabled ? settings().quickAddShortcut : null;
    if (registeredShortcut === wanted) return;
    if (registeredShortcut) {
      try {
        globalShortcut.unregister(registeredShortcut);
      } catch {
        // Already gone.
      }
      registeredShortcut = null;
    }
    if (!wanted || !app.isReady()) return;
    try {
      if (globalShortcut.register(wanted, () => void openTodoWindow(null, true))) registeredShortcut = wanted;
      else options.log("warn", "todo notes quick-add shortcut is taken by another application", { accelerator: wanted });
    } catch (error) {
      options.log("warn", "todo notes quick-add shortcut failed", { error: String(error) });
    }
  }

  function applySettings(patch: Partial<TodoDesktopSettings>): TodoDesktopSettings {
    const previous = settings();
    const next = mergeSettings(previous, patch);
    store.update((state) => {
      state.settings = next;
    });
    if (previous.stickiesOnTop !== next.stickiesOnTop) {
      for (const entry of stickies.values()) {
        if (!entry.window.isDestroyed()) entry.window.setAlwaysOnTop(next.stickiesOnTop, "floating");
      }
    }
    if (previous.stickiesHidden !== next.stickiesHidden) void reconcileStickies();
    updateTray();
    applyShortcut();
    applyLoginItem();
    if (previous.reminders !== next.reminders) scheduleReminders();
    broadcast(TODO_CHANNELS.settingsChanged, next);
    return next;
  }

  // ---------------------------------------------------------- reconciliation

  async function reconcile(): Promise<void> {
    const next = await readPluginEnabled();
    if (next === null) return;
    const changed = next !== enabled;
    enabled = next;
    if (enabled) {
      if (changed) {
        startFeed();
        scheduleDayRollover();
      }
      updateTray();
      applyShortcut();
      applyLoginItem();
    } else {
      // Plugin off => every TODO surface is off: windows, tray, shortcut,
      // reminders and the login item this plugin created. Records stay in the
      // Library untouched.
      stopFeed();
      if (reminderTimer) clearTimeout(reminderTimer);
      if (dayTimer) clearTimeout(dayTimer);
      reminderTimer = null;
      dayTimer = null;
      closeAllStickies();
      if (settingsWindow && !settingsWindow.isDestroyed()) settingsWindow.destroy();
      settingsWindow = null;
      if (todoWindow && !todoWindow.isDestroyed()) {
        persistTodoWindow();
        todoWindow.destroy();
      }
      todoWindow = null;
      todoReady = false;
      updateTray();
      applyShortcut();
      applyLoginItem();
      items = [];
    }
  }

  function refresh(): Promise<void> {
    refreshQueue = refreshQueue.then(reconcile, reconcile).catch((error) => {
      options.log("warn", "todo notes refresh failed", { error: String(error) });
    });
    return refreshQueue;
  }

  // -------------------------------------------------------------------- IPC

  function senderWindow(event: IpcMainInvokeEvent): BrowserWindow | null {
    const window = BrowserWindow.fromWebContents(event.sender);
    return window && !window.isDestroyed() ? window : null;
  }

  function stickyFor(event: IpcMainInvokeEvent): StickyEntry | null {
    const window = senderWindow(event);
    if (!window) return null;
    for (const entry of stickies.values()) if (entry.window === window) return entry;
    return null;
  }

  function isSettingsSender(event: IpcMainInvokeEvent): boolean {
    const window = senderWindow(event);
    return Boolean(window && settingsWindow && window === settingsWindow);
  }

  function isTodoSender(event: IpcMainInvokeEvent): boolean {
    const window = senderWindow(event);
    return Boolean(window && todoWindow && window === todoWindow);
  }

  type Scope = "todo" | "sticky" | "plugin" | "plugin-or-host" | "settings";

  function handle(channel: string, scope: Scope, handler: (event: IpcMainInvokeEvent, ...args: unknown[]) => unknown): void {
    ipcMain.handle(channel, async (event, ...args) => {
      const todo = isTodoSender(event);
      const sticky = Boolean(stickyFor(event));
      // The settings window may use only the settings-scoped channels.
      const settingsSender = isSettingsSender(event);
      const allowed =
        (scope === "settings" && (todo || sticky || settingsSender || options.authorizeHostWindow(event, channel)))
        || (scope === "todo" && todo)
        || (scope === "sticky" && sticky)
        || ((scope === "plugin" || scope === "plugin-or-host") && (todo || sticky))
        || (scope === "plugin-or-host" && !todo && !sticky && options.authorizeHostWindow(event, channel));
      if (!allowed) throw new Error(`TODO notes IPC access denied: ${channel}`);
      return handler(event, ...args);
    });
  }

  handle(TODO_CHANNELS.ready, "plugin", (event) => {
    const window = senderWindow(event);
    if (window) {
      const fallback = (window as BrowserWindow & { __mangooTodoFallback?: ReturnType<typeof setTimeout> }).__mangooTodoFallback;
      if (fallback) clearTimeout(fallback);
    }
    const sticky = stickyFor(event);
    if (sticky) {
      sticky.ready = true;
      applyStickyBounds(sticky);
      if (!sticky.window.isVisible()) sticky.window.showInactive();
      const note = pinnedMap()[sticky.itemId];
      return {
        role: "sticky",
        itemId: sticky.itemId,
        appearance: note?.appearance ?? { ...DEFAULT_STICKY_APPEARANCE },
        settings: settings(),
        focusItemId: null,
        quickAdd: false
      } satisfies TodoBootState;
    }
    todoReady = true;
    if (window) revealTodo(window);
    const boot: TodoBootState = {
      role: "todo",
      itemId: null,
      appearance: null,
      settings: settings(),
      focusItemId: pendingFocusItem,
      quickAdd: pendingQuickAdd
    };
    pendingFocusItem = null;
    pendingQuickAdd = false;
    return boot;
  });

  handle(TODO_CHANNELS.getSettings, "settings", () => settings());
  handle(TODO_CHANNELS.setSettings, "settings", (_event, patch) => applySettings(patch as Partial<TodoDesktopSettings>));
  handle(TODO_CHANNELS.openSettings, "settings", (event, section) =>
    isSettingsSender(event) ? true : openSettingsWindow(typeof section === "string" ? section : "capture"));

  handle(TODO_CHANNELS.pin, "plugin", (_event, itemId) => (isRecordId(itemId) ? pinItem(itemId) : { ok: false, error: "事项无效" }));
  handle(TODO_CHANNELS.unpin, "plugin", (_event, itemId) => (isRecordId(itemId) ? unpinItem(itemId) : { ok: false, error: "事项无效" }));
  handle(TODO_CHANNELS.listPinned, "todo", () => pinnedIds());
  handle(TODO_CHANNELS.setStickiesVisible, "todo", (_event, visible) => {
    setStickiesHidden(visible !== true);
    return visible === true;
  });

  handle(TODO_CHANNELS.stickyExpand, "sticky", (event, expanded) => {
    const entry = stickyFor(event);
    if (!entry) return;
    const want = expanded === true && settings().expandOnHover;
    if (entry.expanded === want) return;
    entry.expanded = want;
    applyStickyBounds(entry);
  });

  handle(TODO_CHANNELS.stickyMeasure, "sticky", (event, height, contentHeight) => {
    const entry = stickyFor(event);
    if (!entry || typeof height !== "number" || !Number.isFinite(height)) return;
    const next = Math.min(STICKY_HEIGHT.collapsedMax, Math.max(STICKY_HEIGHT.collapsedMin, Math.ceil(height)));
    const content = typeof contentHeight === "number" && Number.isFinite(contentHeight) ? Math.min(STICKY_HEIGHT.expandedMax, Math.ceil(contentHeight)) : entry.contentHeight;
    if (next === entry.collapsedHeight && content === entry.contentHeight) return;
    entry.collapsedHeight = next;
    entry.contentHeight = content;
    applyStickyBounds(entry);
  });

  handle(TODO_CHANNELS.stickyUpdate, "sticky", (event, patch) => {
    const entry = stickyFor(event);
    if (!entry) return { ...DEFAULT_STICKY_APPEARANCE };
    const note = updateNote(entry.itemId, (value) => {
      value.appearance = mergeAppearance(value.appearance, patch);
    });
    const appearance: StickyAppearance = note?.appearance ?? { ...DEFAULT_STICKY_APPEARANCE };
    if (!entry.window.isDestroyed()) {
      entry.window.webContents.send(TODO_CHANNELS.stickyStateChanged, appearance);
    }
    applyStickyBounds(entry);
    return appearance;
  });

  handle(TODO_CHANNELS.openItem, "plugin", async (_event, itemId) => (isRecordId(itemId) ? openTodoWindow(itemId) : false));

  handle(TODO_CHANNELS.openLink, "plugin", async (_event, url) => {
    if (typeof url !== "string" || !isSafeUrl(url)) return { ok: false, error: "只能打开网页或邮件链接" };
    try {
      await shell.openExternal(url);
      return { ok: true };
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error.message : String(error) };
    }
  });

  handle(TODO_CHANNELS.copyText, "plugin", (_event, text) => {
    if (typeof text !== "string" || text.length > 256 * 1024) return { ok: false, error: "文本无效" };
    clipboard.writeText(text);
    return { ok: true };
  });

  handle(TODO_CHANNELS.setAlwaysOnTop, "todo", (event, value) => {
    const window = senderWindow(event);
    if (!window) return false;
    window.setAlwaysOnTop(value === true);
    schedulePersist();
    return window.isAlwaysOnTop();
  });

  handle(TODO_CHANNELS.openMainWindow, "plugin", async () => {
    await options.openMainWindow();
    return true;
  });

  handle(TODO_CHANNELS.openWindow, "plugin-or-host", () => openTodoWindow());

  // ---------------------------------------------------------------- backup

  async function readAllRecords(): Promise<BackupRecord[]> {
    const client = await coreClient();
    const records: BackupRecord[] = [];
    for (const collection of BACKUP_COLLECTIONS) {
      const page = await client.listSharedPluginRecords(TODO_PLUGIN_ID, { collection }, { requestPriority: "interactive" });
      for (const record of page.items) {
        if (record.deleted || record.fields.purged === true) continue;
        records.push({ id: record.id, collection, fields: record.fields });
      }
    }
    return records;
  }

  async function writeAllRecords(records: readonly BackupRecord[]): Promise<void> {
    const client = await coreClient();
    // Bounded by count AND bytes: an image is up to ~150 KiB.
    const MAX_BATCH_BYTES = 1_200_000;
    let batch: PluginRecordOperation[] = [];
    let bytes = 0;
    const flush = async () => {
      if (!batch.length) return;
      await client.applySharedPluginRecordBatch(TODO_PLUGIN_ID, batch, { requestPriority: "interactive" });
      batch = [];
      bytes = 0;
    };
    for (const record of records) {
      const size = JSON.stringify(record.fields).length + 200;
      if (batch.length + 2 > 200 || (batch.length && bytes + size > MAX_BATCH_BYTES)) await flush();
      // Restore first: a note deleted since the backup comes back.
      batch.push({ op: "restore", id: record.id }, { op: "put", id: record.id, collection: record.collection, fields: record.fields });
      bytes += size;
    }
    await flush();
  }

  handle(TODO_CHANNELS.exportBackup, "settings", async (event): Promise<TodoBackupResult> => {
    if (!enabled) return { ok: false, error: "便笺插件未启用" };
    try {
      const owner = senderWindow(event);
      const options_ = { title: "导出便笺备份", defaultPath: path.join(app.getPath("documents"), backupFileName()), filters: [{ name: "便笺备份", extensions: ["json"] }] };
      const choice = owner ? await dialog.showSaveDialog(owner, options_) : await dialog.showSaveDialog(options_);
      if (choice.canceled || !choice.filePath) return { ok: false, cancelled: true };
      const records = await readAllRecords();
      const temporary = `${choice.filePath}.${process.pid}.tmp`;
      await fs.promises.writeFile(temporary, `${JSON.stringify(createBackup(records))}\n`, "utf8");
      await fs.promises.rename(temporary, choice.filePath);
      return { ok: true, path: choice.filePath, ...summarizeBackup(records) };
    } catch (error) {
      options.log("warn", "todo notes export failed", { error: String(error) });
      return { ok: false, error: error instanceof Error ? error.message : String(error) };
    }
  });

  handle(TODO_CHANNELS.importBackup, "settings", async (event): Promise<TodoBackupResult> => {
    if (!enabled) return { ok: false, error: "便笺插件未启用" };
    try {
      const owner = senderWindow(event);
      const options_ = { title: "从备份恢复便笺", properties: ["openFile" as const], filters: [{ name: "便笺备份", extensions: ["json"] }] };
      const choice = owner ? await dialog.showOpenDialog(owner, options_) : await dialog.showOpenDialog(options_);
      const file = choice.filePaths[0];
      if (choice.canceled || !file) return { ok: false, cancelled: true };
      const stat = await fs.promises.stat(file);
      if (stat.size > 256 * 1024 * 1024) return { ok: false, error: "备份文件过大" };
      const backup = parseBackup(await fs.promises.readFile(file, "utf8"));
      await writeAllRecords(backup.records);
      return { ok: true, ...summarizeBackup(backup.records) };
    } catch (error) {
      options.log("warn", "todo notes import failed", { error: String(error) });
      return { ok: false, error: error instanceof Error ? error.message : String(error) };
    }
  });

  handle(TODO_CHANNELS.status, "settings", (): TodoStatus => ({
    pluginEnabled: enabled,
    windowOpen: Boolean(todoWindow && !todoWindow.isDestroyed() && todoWindow.isVisible()),
    pinnedCount: pinnedIds().length,
    canLaunchAtLogin: canLaunchAtLogin(),
    quickAddShortcutTaken: Boolean(enabled && settings().quickAddShortcut && registeredShortcut !== settings().quickAddShortcut),
    settings: settings()
  }));

  return {
    async start() {
      if (!started) {
        // `screen` is only usable after the app's ready event (start() runs
        // then); a note on an unplugged monitor comes back to a visible one.
        screen.on("display-removed", () => {
          for (const entry of stickies.values()) applyStickyBounds(entry);
        });
      }
      started = true;
      await refresh();
      return { enabled };
    },
    refresh,
    keepsAppAlive() {
      return started && enabled && Boolean(tray && !tray.isDestroyed());
    },
    ownsWindow(window) {
      if (!window) return false;
      if (todoWindow && window === todoWindow) return true;
      if (settingsWindow && window === settingsWindow) return true;
      for (const entry of stickies.values()) if (entry.window === window) return true;
      return false;
    },
    isEnabled() {
      return enabled;
    },
    openTodoWindow: () => openTodoWindow(),
    dispose() {
      quitting = true;
      if (persistTimer) clearTimeout(persistTimer);
      persistTodoWindow();
      stopFeed();
      if (reminderTimer) clearTimeout(reminderTimer);
      if (dayTimer) clearTimeout(dayTimer);
      if (registeredShortcut) {
        try {
          globalShortcut.unregister(registeredShortcut);
        } catch {
          // Shutting down.
        }
        registeredShortcut = null;
      }
      if (tray && !tray.isDestroyed()) tray.destroy();
      tray = null;
    }
  };
}
