import {
  app,
  BrowserWindow,
  clipboard,
  dialog,
  ipcMain,
  screen,
  shell,
  type IpcMainEvent,
  type IpcMainInvokeEvent,
  type OpenDialogOptions,
  type WebContents
} from "electron";
import { spawn } from "node:child_process";
import { promises as fsp } from "node:fs";
import fsSync from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  startCoreProcess,
  stopCoreProcessHandleAndClear,
  stopCoreProcessesBeforeReplacement,
  waitForCoreHealth,
  type CoreProcessHandle
} from "./core-process.js";
import { createCoreSession, type CoreSession } from "./core-session.js";
import {
  assertDesktopIpcAccess,
  type DesktopIpcWindowRole
} from "./desktop-ipc-permissions.js";
import { desktopLogger, processLogBuffer } from "./desktop-logger.js";
import {
  CoreLifecycleQueue,
  LifecycleCancelledError,
  throwIfLifecycleAborted
} from "./core-lifecycle-queue.js";
import * as libraryRegistry from "./library-registry.js";
import { sessionForDbPath } from "./core-session.js";
import {
  LatestValueBroadcastQueue,
  NOTIFICATION_IPC_CHANNELS,
  NotificationIpcBroadcastTargets,
  NotificationIpcTargetLifecycle,
  ProcessLogIpcSubscriptions,
  assertNotificationIpcAccess,
  parseNotificationAction,
  parseNotificationHistoryQuery,
  parseMediaFocusRequest,
  parseMediaFocusCompletion,
  parseNotificationWindowIntent,
  parseProcessLogFilter,
  parseProcessLogSubscriptionId,
  parseRendererErrorReport,
  parseRendererLogEntries,
  parseThumbnailCacheSummary,
  type NotificationIpcWindowRole
} from "./notification-ipc.js";
import { MediaFocusCoordinator } from "./media-focus-coordinator.js";
import { runNotificationSafeCoreTransition } from "./notification-core-transition.js";
import { NotificationRuntime } from "./notification-runtime.js";
import { createNotificationService, type NotificationService } from "./notification-service.js";
import { runRecoverableNotificationShutdown } from "./notification-shutdown.js";
import { createNotificationStore, notificationDatabasePath } from "./notification-store.js";
import type { NotificationCompactSnapshot } from "./notification-types.js";
import {
  NATIVE_WINDOW_WARMUP_MS,
  createSecondaryWindowRevealState,
  revealSecondaryNativeWindow,
  waitForSecondaryNativeWindowWarmup,
  type SecondaryWindowRevealState
} from "./secondary-window-reveal.js";
import { SecondaryWindowOpeningCoordinator } from "./secondary-window-opening.js";
import {
  createStartupLogEmitter,
  type StartupLogEmitter
} from "./startup-logging.js";

let mainWindow: BrowserWindow | null = null;
const mediaFocusCoordinator = new MediaFocusCoordinator();
const emitMainStartupReady = createStartupLogEmitter(
  desktopLogger,
  "main",
  "Megle desktop ready"
);
const rendererStartupEmitters = new WeakMap<WebContents, StartupLogEmitter>();
type SecondaryWindowRole = "settings" | "plugins" | "notifications";
type SecondaryWindowConfig = {
  title: string;
  route: string;
  width: number;
  height: number;
  minWidth: number;
  minHeight: number;
};
type SecondaryWindowPosition = { x: number; y: number };
const secondaryWindows: Record<SecondaryWindowRole, BrowserWindow | null> = {
  settings: null,
  plugins: null,
  notifications: null
};
const secondaryWindowOpenings = new SecondaryWindowOpeningCoordinator<SecondaryWindowRole>();
type SecondaryWindowReadiness = {
  windowId: number;
  revealState: SecondaryWindowRevealState<SecondaryWindowRevealResult | false>;
  nativeWarmupStartedAt: number | null;
  nativeWindowRevealedAt: number | null;
  fallbackTimer: NodeJS.Timeout | null;
};
type SecondaryWindowRevealResult = {
  nativeWarmupStartedAt: number;
  nativeWindowRevealedAt: number;
  fallback: boolean;
};
const secondaryWindowReadiness: Record<SecondaryWindowRole, SecondaryWindowReadiness | null> = {
  settings: null,
  plugins: null,
  notifications: null
};
const secondaryWindowPositions: Record<SecondaryWindowRole, SecondaryWindowPosition | null> = {
  settings: null,
  plugins: null,
  notifications: null
};
const SECONDARY_WINDOW_CONFIGS: Record<SecondaryWindowRole, SecondaryWindowConfig> = {
  settings: {
    title: "Megle 设置",
    route: "#/settings",
    width: 900,
    height: 640,
    minWidth: 560,
    minHeight: 420
  },
  plugins: {
    title: "Megle Plugins",
    route: "#/plugins",
    width: 1040,
    height: 720,
    minWidth: 720,
    minHeight: 500
  },
  notifications: {
    title: "Megle 通知中心",
    route: "#/notifications",
    width: 1040,
    height: 720,
    minWidth: 820,
    minHeight: 560
  }
};
let coreProcess: CoreProcessHandle | null = null;
let pendingCoreProcess: CoreProcessHandle | null = null;
let coreSession: CoreSession | null = null;
let pendingCoreSession: CoreSession | null = null;
let coreReadyPromise: Promise<CoreSession> | null = null;
let cachedRegistry: libraryRegistry.LibraryRegistry | null = null;
let cachedFfmpegAvailable: boolean | null = null;
let persisted = false;
let shutdownFinalized = false;
let shutdownPromise: Promise<void> | null = null;
const coreLifecycleQueue = new CoreLifecycleQueue();
let notificationRuntime: NotificationRuntime<NotificationService> | null = null;
let notificationChangeSubscription: (() => void) | null = null;
const processLogSubscriptions = new ProcessLogIpcSubscriptions(
  processLogBuffer,
  NOTIFICATION_IPC_CHANNELS.logsChanged
);
const notificationBroadcastTargets = new NotificationIpcBroadcastTargets<NotificationCompactSnapshot>(
  NOTIFICATION_IPC_CHANNELS.changed
);
const notificationIpcTargetLifecycle = new NotificationIpcTargetLifecycle((targetId) => {
  processLogSubscriptions.clearTarget(targetId);
  notificationBroadcastTargets.clearTarget(targetId);
});
const notificationBroadcastQueue = new LatestValueBroadcastQueue<NotificationCompactSnapshot>(
  broadcastNotificationSnapshot
);
let persistDebounceTimer: NodeJS.Timeout | null = null;
let mainWindowReadyToShow: Promise<void> | null = null;
let shellReadyRevealPromise: Promise<boolean> | null = null;
let shellReadyVisibleWindowId: number | null = null;
let shellReadyFailureFallbackTimer: NodeJS.Timeout | null = null;
let activeWindowDrag: {
  windowId: number;
  pointerOffsetX: number;
  pointerOffsetY: number;
} | null = null;
let launchWindowPlacement: WindowPlacement | null = null;
let launchWindowMaximized = false;
let windowStatePersistenceEnabled = false;
const WINDOW_COMPOSITION_BOOTSTRAP_MARGIN_PX = 96;
const PERSIST_DEBOUNCE_MS = 500;
const SHELL_READY_FAILURE_TIMEOUT_MS = 4000;
const SECONDARY_WINDOW_READY_TIMEOUT_MS = 4000;
const AUTO_CORE_STARTUP_MAX_ATTEMPTS = 3;
const CORE_REUSE_HEALTH_TIMEOUT_MS = 1_000;
const WINDOW_COMPOSITION_REFRESH_DELAYS_MS = [34, 140, 420] as const;
const CHROMIUM_OVERLAY_SCROLLBAR_FEATURES = [
  "OverlayScrollbar",
  "FluentOverlayScrollbar"
].join(",");

app.commandLine.appendSwitch("enable-features", CHROMIUM_OVERLAY_SCROLLBAR_FEATURES);

interface WindowState {
  width: number;
  height: number;
  x?: number;
  y?: number;
  maximized: boolean;
  secondaryWindows?: Partial<Record<SecondaryWindowRole, SecondaryWindowPosition>>;
}

interface WindowPlacement {
  width: number;
  height: number;
  x: number;
  y: number;
}

const DEFAULT_WINDOW_STATE: WindowState = {
  width: 1440,
  height: 920,
  maximized: false
};

function userDataDir(): string {
  return app.getPath("userData");
}
function getRegistry(): libraryRegistry.LibraryRegistry {
  if (!cachedRegistry) cachedRegistry = libraryRegistry.loadRegistry(userDataDir());
  return cachedRegistry;
}
function libraryIdForDbPath(dbPath: string): string | null {
  const resolved = path.resolve(dbPath);
  return getRegistry().libraries.find((library) => path.resolve(library.dbPath) === resolved)?.id ?? null;
}
function coreLogHandlerForSession(session: CoreSession) {
  const libraryId = libraryIdForDbPath(session.dbPath);
  const emitCoreStartupReady = createStartupLogEmitter(
    desktopLogger,
    "core",
    "Megle Core ready",
    libraryId ? { libraryId } : {}
  );
  return (source: "core", level: Parameters<typeof desktopLogger.log>[1], message: string, metadata?: Readonly<Record<string, unknown>>) => {
    if (level === "info" && message === "Megle Core ready" && metadata?.eventKind === "startup_ready") {
      emitCoreStartupReady(metadata);
      return;
    }
    desktopLogger.structured(source, level, message, {
      ...(metadata ?? {}),
      ...(libraryId ? { libraryId } : {})
    });
  };
}
function persistRegistry(next: libraryRegistry.LibraryRegistry): void {
  cachedRegistry = next;
  libraryRegistry.saveRegistry(userDataDir(), next);
}
function pushLibraryPhase(phase: unknown): void {
  // Secondary windows share the same Core URL/token. Broadcast restart phases
  // to every renderer so settings/plugin requests do not remain on stale state.
  for (const win of [mainWindow, ...Object.values(secondaryWindows)]) {
    if (win && !win.isDestroyed()) {
      win.webContents.send("megle:library-load-phase", phase);
    }
  }
}

function windowForIpcEvent(event: IpcMainInvokeEvent | IpcMainEvent): BrowserWindow | null {
  const window = BrowserWindow.fromWebContents(event.sender);
  return window && !window.isDestroyed() ? window : null;
}

function roleForWebContents(sender: WebContents): DesktopIpcWindowRole | null {
  const window = BrowserWindow.fromWebContents(sender);
  if (!window || window.isDestroyed()) return null;
  if (window === mainWindow) return "main";
  for (const role of Object.keys(secondaryWindows) as SecondaryWindowRole[]) {
    if (secondaryWindows[role] === window) return role;
  }
  return null;
}

function requireDesktopIpcAccess(
  event: IpcMainInvokeEvent | IpcMainEvent,
  channel: string
): DesktopIpcWindowRole {
  const role = roleForWebContents(event.sender);
  assertDesktopIpcAccess(role, channel);
  return role;
}

function requireNotificationIpcAccess(
  event: IpcMainInvokeEvent | IpcMainEvent,
  channel: string
): NotificationIpcWindowRole {
  const role = roleForWebContents(event.sender);
  assertNotificationIpcAccess(role, channel);
  return role;
}

function broadcastNotificationSnapshot(snapshot: NotificationCompactSnapshot): void {
  notificationBroadcastTargets.broadcast(snapshot);
}

function ensureNotificationRuntime(): NotificationRuntime<NotificationService> {
  if (notificationRuntime) return notificationRuntime;
  notificationRuntime = new NotificationRuntime<NotificationService>(async (runtimeSignal) => {
    const service = await createNotificationService({
      openStore: (serviceSignal) =>
        createNotificationStore({
          databasePath: notificationDatabasePath(userDataDir()),
          signal: AbortSignal.any([runtimeSignal, serviceSignal])
        })
    });
    if (runtimeSignal.aborted) {
      await service.close();
      throw new DOMException("Notification runtime closed during startup", "AbortError");
    }
    return service;
  });
  return notificationRuntime;
}

async function ensureNotificationService(): Promise<NotificationService> {
  const service = await ensureNotificationRuntime().start();
  if (!notificationChangeSubscription) {
    notificationChangeSubscription = service.subscribe((snapshot) => {
      notificationBroadcastQueue.publish(snapshot);
    });
  }
  return service;
}

async function detachNotificationServiceForCoreReplacement(): Promise<void> {
  const service = await ensureNotificationService();
  await service.detachLibrary();
}

async function attachNotificationServiceForSession(
  session: CoreSession,
  generation: number,
  signal: AbortSignal
): Promise<void> {
  assertCurrentCoreGeneration(generation, signal);
  const service = await ensureNotificationService();
  assertCurrentCoreGeneration(generation, signal);
  if (coreSession !== session) {
    throw new LifecycleCancelledError("Core session changed before notification attach");
  }
  const registry = getRegistry();
  const active = registry.libraries.find((library) => library.id === registry.activeLibraryId);
  if (!active || path.resolve(active.dbPath) !== path.resolve(session.dbPath)) {
    throw new LifecycleCancelledError("Active library changed before notification attach");
  }
  assertCurrentCoreGeneration(generation, signal);
  await service.attachLibrary(
    { id: active.id, name: active.name },
    { baseUrl: session.baseUrl, sessionToken: session.sessionToken }
  );
  assertCurrentCoreGeneration(generation, signal);
}

async function quiesceNotificationRuntime(): Promise<void> {
  const runtime = notificationRuntime;
  if (runtime) await runtime.detachLibrary();
}

async function closeNotificationRuntime(): Promise<void> {
  const runtime = notificationRuntime;
  notificationRuntime = null;
  notificationChangeSubscription?.();
  notificationChangeSubscription = null;
  notificationBroadcastQueue.close();
  notificationIpcTargetLifecycle.clear();
  notificationBroadcastTargets.clear();
  processLogSubscriptions.clear();
  await runtime?.close();
}

async function restoreNotificationsAfterFailedCoreStop(
  session: CoreSession | null,
  generation: number
): Promise<void> {
  if (
    !session ||
    coreLifecycleQueue.currentGeneration !== generation ||
    coreSession !== session
  ) {
    return;
  }
  const service = await ensureNotificationService();
  if (
    coreLifecycleQueue.currentGeneration !== generation ||
    coreSession !== session
  ) {
    await service.detachLibrary();
    return;
  }
  const registry = getRegistry();
  const active = registry.libraries.find((library) => library.id === registry.activeLibraryId);
  if (!active || path.resolve(active.dbPath) !== path.resolve(session.dbPath)) return;
  await service.attachLibrary(
    { id: active.id, name: active.name },
    { baseUrl: session.baseUrl, sessionToken: session.sessionToken }
  );
  if (
    coreLifecycleQueue.currentGeneration !== generation ||
    coreSession !== session
  ) {
    await service.detachLibrary();
  }
}

function registeredNotificationLibrary(libraryId: string): { id: string; name: string } {
  const library = getRegistry().libraries.find((candidate) => candidate.id === libraryId);
  if (!library) throw new Error("notification library not found");
  return { id: library.id, name: library.name };
}

function formatProcessLogExport(
  lines: ReturnType<typeof processLogBuffer.snapshot>
): string {
  return lines
    .map((line) => {
      let metadata = "";
      if (line.metadata) {
        try {
          metadata = ` ${JSON.stringify(line.metadata)}`;
        } catch {
          metadata = " [metadata unavailable]";
        }
      }
      return `${new Date(line.timestamp).toISOString()} #${line.sequence} [${line.source}/${line.level}] ${line.message}${metadata}`;
    })
    .join("\r\n");
}

function toSummaries(reg: libraryRegistry.LibraryRegistry): Array<{
  id: string; name: string; dbPath: string; available: boolean; active: boolean; icon?: string; color?: string;
}> {
  return reg.libraries.map((l) => ({
    id: l.id,
    name: l.name,
    dbPath: l.dbPath,
    available: fsSync.existsSync(l.dbPath),
    active: l.id === reg.activeLibraryId,
    icon: l.icon,
    color: l.color
  }));
}

ipcMain.handle("megle:pick-folder", async (event) => {
  requireDesktopIpcAccess(event, "megle:pick-folder");
  const options: OpenDialogOptions = { properties: ["openDirectory"] };
  const result = mainWindow
    ? await dialog.showOpenDialog(mainWindow, options)
    : await dialog.showOpenDialog(options);

  return result.canceled ? null : result.filePaths[0] ?? null;
});

ipcMain.handle("megle:diagnostics", async (event) => {
  requireDesktopIpcAccess(event, "megle:diagnostics");
  const session = coreSession;
  return {
    ffmpegAvailable: await detectFfmpeg(),
    dbPath: session?.dbPath ?? null,
    pluginsDir: session ? resolvePluginsDir(session.dbPath) : null
  };
});

ipcMain.handle("megle:window-minimize", (event) => {
  requireDesktopIpcAccess(event, "megle:window-minimize");
  windowForIpcEvent(event)?.minimize();
});

ipcMain.handle("megle:window-maximize", (event) => {
  requireDesktopIpcAccess(event, "megle:window-maximize");
  const window = windowForIpcEvent(event);
  if (!window) return false;
  if (window.isMaximized()) {
    window.unmaximize();
    return false;
  }
  window.maximize();
  return true;
});

ipcMain.handle("megle:window-close", (event) => {
  requireDesktopIpcAccess(event, "megle:window-close");
  windowForIpcEvent(event)?.close();
});

ipcMain.handle("megle:window-is-maximized", (event) => {
  requireDesktopIpcAccess(event, "megle:window-is-maximized");
  return windowForIpcEvent(event)?.isMaximized() ?? false;
});

ipcMain.handle("megle:window-get-bounds", (event) => {
  requireDesktopIpcAccess(event, "megle:window-get-bounds");
  const bounds = windowForIpcEvent(event)?.getBounds();
  if (!bounds) {
    return null;
  }
  return bounds;
});

ipcMain.handle("megle:window-set-position", (event, x: number, y: number) => {
  requireDesktopIpcAccess(event, "megle:window-set-position");
  const window = windowForIpcEvent(event);
  if (!window) return false;
  window.setPosition(Math.round(x), Math.round(y));
  return true;
});

ipcMain.handle(
  "megle:window-begin-drag",
  (
    event,
    payload: { clientX: number; screenX: number; screenY: number; titlebarOffsetY: number; viewportWidth: number }
  ) => {
    requireDesktopIpcAccess(event, "megle:window-begin-drag");
    const window = windowForIpcEvent(event);
    if (!window) return null;

    const cursor = screen.getCursorScreenPoint();
    if (window.isMaximized()) {
      const restored = window.getNormalBounds();
      const pointerRatioX = clampNumber(payload.clientX / Math.max(payload.viewportWidth, 1), 0, 1);
      const targetX = Math.round(cursor.x - restored.width * pointerRatioX);
      const targetY = Math.round(cursor.y - Math.max(0, payload.titlebarOffsetY));
      window.unmaximize();
      window.setBounds({
        x: targetX,
        y: targetY,
        width: restored.width,
        height: restored.height
      });
      refreshWindowComposition(window);
      activeWindowDrag = {
        windowId: window.id,
        pointerOffsetX: cursor.x - targetX,
        pointerOffsetY: cursor.y - targetY
      };
      return {
        x: targetX,
        y: targetY,
        width: restored.width,
        height: restored.height,
        restoredFromMaximized: true
      };
    }

    const bounds = window.getBounds();
    activeWindowDrag = {
      windowId: window.id,
      pointerOffsetX: cursor.x - bounds.x,
      pointerOffsetY: cursor.y - bounds.y
    };
    return {
      ...bounds,
      restoredFromMaximized: false
    };
  }
);

ipcMain.handle("megle:window-drag-move", (event) => {
  requireDesktopIpcAccess(event, "megle:window-drag-move");
  const window = windowForIpcEvent(event);
  if (!window || !activeWindowDrag || activeWindowDrag.windowId !== window.id) return false;
  const cursor = screen.getCursorScreenPoint();
  window.setPosition(
    Math.round(cursor.x - activeWindowDrag.pointerOffsetX),
    Math.round(cursor.y - activeWindowDrag.pointerOffsetY)
  );
  return true;
});

ipcMain.handle("megle:window-end-drag", (event) => {
  requireDesktopIpcAccess(event, "megle:window-end-drag");
  const window = windowForIpcEvent(event);
  if (!window || activeWindowDrag?.windowId !== window.id) return false;
  activeWindowDrag = null;
  return true;
});

ipcMain.handle("megle:shell-reveal-path", (event, targetPath: string) => {
  requireDesktopIpcAccess(event, "megle:shell-reveal-path");
  if (typeof targetPath !== "string" || targetPath.trim().length === 0) {
    return false;
  }
  shell.showItemInFolder(targetPath);
  return true;
});

ipcMain.handle("megle:shell-open-path", async (event, targetPath: string) => {
  requireDesktopIpcAccess(event, "megle:shell-open-path");
  if (typeof targetPath !== "string" || targetPath.trim().length === 0) {
    return false;
  }
  const error = await shell.openPath(targetPath);
  return error.length === 0;
});

ipcMain.handle("megle:clipboard-write-text", (event, text: string) => {
  requireDesktopIpcAccess(event, "megle:clipboard-write-text");
  if (typeof text !== "string") {
    return false;
  }
  clipboard.writeText(text);
  return true;
});

ipcMain.handle("megle:shell-ready", async (event) => {
  requireDesktopIpcAccess(event, "megle:shell-ready");
  const window = mainWindow;
  if (!window || window.isDestroyed()) {
    return false;
  }
  if (shellReadyVisibleWindowId === window.id) {
    return true;
  }
  if (shellReadyRevealPromise) {
    return shellReadyRevealPromise;
  }

  clearShellReadyFailureFallback();
  const revealPromise = revealMainWindowForShellReady(window);
  shellReadyRevealPromise = revealPromise;
  try {
    return await revealPromise;
  } finally {
    if (shellReadyRevealPromise === revealPromise) {
      shellReadyRevealPromise = null;
    }
  }
});

ipcMain.handle("megle:open-settings-window", async (event) => {
  requireDesktopIpcAccess(event, "megle:open-settings-window");
  await openSecondaryWindow("settings");
  return true;
});

ipcMain.handle("megle:close-settings-window", (event) => {
  requireDesktopIpcAccess(event, "megle:close-settings-window");
  closeSecondaryWindow("settings");
  return true;
});

ipcMain.handle("megle:open-plugins-window", async (event) => {
  requireDesktopIpcAccess(event, "megle:open-plugins-window");
  await openSecondaryWindow("plugins");
  return true;
});

ipcMain.handle("megle:close-plugins-window", (event) => {
  requireDesktopIpcAccess(event, "megle:close-plugins-window");
  closeSecondaryWindow("plugins");
  return true;
});

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.openWindow, async (event, input: unknown) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.openWindow);
  const intent = input == null ? null : parseNotificationWindowIntent(input);
  await openSecondaryWindow("notifications");
  const window = secondaryWindows.notifications;
  if (intent && window && !window.isDestroyed()) {
    window.webContents.send(NOTIFICATION_IPC_CHANNELS.windowIntent, intent);
  }
  return true;
});

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.closeWindow, (event) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.closeWindow);
  closeSecondaryWindow("notifications");
  return true;
});

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.focusMedia, async (event, input: unknown) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.focusMedia);
  const request = parseMediaFocusRequest(input);
  const window = mainWindow;
  if (!window || window.isDestroyed()) return false;
  if (window.isMinimized()) window.restore();
  window.show();
  window.focus();
  return mediaFocusCoordinator.request(request, (command) => {
    if (window.isDestroyed()) throw new Error("main window closed before media focus delivery");
    window.webContents.send(NOTIFICATION_IPC_CHANNELS.focusMediaRequested, command);
  });
});

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.focusMediaCompleted, (event, input: unknown) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.focusMediaCompleted);
  return mediaFocusCoordinator.complete(parseMediaFocusCompletion(input));
});

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.compact, async (event) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.compact);
  notificationBroadcastTargets.register(event.sender);
  const service = await ensureNotificationService();
  return service.compactSnapshot();
});

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.historyFacets, async (event) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.historyFacets);
  const facets = await (await ensureNotificationService()).historyFacets();
  const libraries = new Map(facets.libraries.map(({ id, name }) => [id, name]));
  for (const { id, name } of getRegistry().libraries) libraries.set(id, name);
  return {
    libraries: [...libraries]
      .map(([id, name]) => ({ id, name }))
      .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id)),
    kinds: facets.kinds
  };
});

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.history, async (event, input: unknown) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.history);
  return (await ensureNotificationService()).history(parseNotificationHistoryQuery(input));
});

ipcMain.handle(
  NOTIFICATION_IPC_CHANNELS.cancel,
  async (event, libraryId: unknown, operationId: unknown) => {
    requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.cancel);
    const action = parseNotificationAction(libraryId, operationId);
    return (await ensureNotificationService()).cancelOperation(
      action.libraryId,
      action.operationId
    );
  }
);

ipcMain.handle(
  NOTIFICATION_IPC_CHANNELS.retry,
  async (event, libraryId: unknown, operationId: unknown) => {
    requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.retry);
    const action = parseNotificationAction(libraryId, operationId);
    return (await ensureNotificationService()).retryOperation(
      action.libraryId,
      action.operationId
    );
  }
);

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.reportError, async (event, input: unknown) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.reportError);
  const report = parseRendererErrorReport(input);
  const library = registeredNotificationLibrary(report.libraryId);
  await (await ensureNotificationService()).reportRendererError({
    ...report,
    libraryName: library.name
  });
  return true;
});

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.thumbnailSummary, async (event, input: unknown) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.thumbnailSummary);
  const summary = parseThumbnailCacheSummary(input);
  const library = registeredNotificationLibrary(summary.libraryId);
  if (getRegistry().activeLibraryId !== library.id) {
    throw new Error("thumbnail summary must belong to the active library");
  }
  await (await ensureNotificationService()).publishThumbnailSummary({
    ...summary,
    libraryName: library.name
  });
  return true;
});

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.logsPage, (event, input: unknown) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.logsPage);
  return processLogBuffer.page(parseProcessLogFilter(input));
});

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.logsExport, async (event, input: unknown) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.logsExport);
  const { limit: _limit, ...filter } = parseProcessLogFilter(input);
  const lines = processLogBuffer.snapshot(filter);
  const owner = windowForIpcEvent(event);
  const defaultPath = path.join(
    app.getPath("documents"),
    `megle-process-logs-${new Date().toISOString().replaceAll(":", "-")}.txt`
  );
  const options = {
    defaultPath,
    filters: [{ name: "Text", extensions: ["txt"] }]
  };
  const result = owner
    ? await dialog.showSaveDialog(owner, options)
    : await dialog.showSaveDialog(options);
  if (result.canceled || !result.filePath) {
    return { saved: false, canceled: true, filePath: null, lineCount: 0, error: null };
  }
  try {
    await fsp.writeFile(result.filePath, formatProcessLogExport(lines), "utf8");
    return {
      saved: true,
      canceled: false,
      filePath: result.filePath,
      lineCount: lines.length,
      error: null
    };
  } catch (error) {
    return {
      saved: false,
      canceled: false,
      filePath: result.filePath,
      lineCount: 0,
      error: error instanceof Error ? error.message : String(error)
    };
  }
});

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.logsSubscribe, (event, input: unknown) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.logsSubscribe);
  return processLogSubscriptions.snapshotAndSubscribe(
    event.sender,
    parseProcessLogFilter(input)
  );
});

ipcMain.on(NOTIFICATION_IPC_CHANNELS.logsUnsubscribe, (event, subscriptionId: unknown) => {
  try {
    requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.logsUnsubscribe);
    processLogSubscriptions.unsubscribe(
      event.sender.id,
      parseProcessLogSubscriptionId(subscriptionId)
    );
  } catch {
    // Fire-and-forget unsubscribe is intentionally silent for stale windows.
  }
});

ipcMain.handle(NOTIFICATION_IPC_CHANNELS.logsClear, (event) => {
  requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.logsClear);
  processLogBuffer.clear();
  return true;
});

ipcMain.on(NOTIFICATION_IPC_CHANNELS.rendererLogs, (event, input: unknown) => {
  try {
    requireNotificationIpcAccess(event, NOTIFICATION_IPC_CHANNELS.rendererLogs);
    for (const entry of parseRendererLogEntries(input)) {
      if (
        entry.level === "info" &&
        entry.message === "Megle renderer ready" &&
        entry.metadata?.eventKind === "startup_ready"
      ) {
        let emitStartupReady = rendererStartupEmitters.get(event.sender);
        if (!emitStartupReady) {
          emitStartupReady = createStartupLogEmitter(
            desktopLogger,
            "renderer",
            "Megle renderer ready"
          );
          rendererStartupEmitters.set(event.sender, emitStartupReady);
        }
        emitStartupReady(entry.metadata);
        continue;
      }
      if (entry.metadata) {
        desktopLogger.structured("renderer", entry.level, entry.message, entry.metadata);
      } else {
        desktopLogger.log("renderer", entry.level, entry.message);
      }
    }
  } catch (error) {
    desktopLogger.warn("main", "[megle] rejected renderer log batch:", error);
  }
});

ipcMain.handle("megle:secondary-window-ready", async (event, role: unknown) => {
  if (!isSecondaryWindowRole(role)) return false;
  const window = windowForIpcEvent(event);
  if (!window || secondaryWindows[role] !== window) return false;
  const readiness = secondaryWindowReadiness[role];
  if (!readiness || readiness.windowId !== window.id) return false;
  readiness.revealState.rendererReady = true;
  revealSecondaryWindowWhenReady(role, window);
  return readiness.revealState.result;
});

ipcMain.handle("megle:visual-capture-page", async (event) => {
  const window = windowForIpcEvent(event);
  if (!isVisualHarnessEnabled() || !window) {
    return null;
  }
  if (window.isMinimized()) {
    window.restore();
  }
  window.show();
  window.focus();
  await new Promise((resolve) => setTimeout(resolve, 120));

  const bounds = window.getContentBounds();
  const image = await window.webContents.capturePage({
    x: 0,
    y: 0,
    width: Math.max(1, bounds.width),
    height: Math.max(1, bounds.height)
  });
  if (image.isEmpty()) {
    return null;
  }
  return image.toPNG().toString("base64");
});

ipcMain.handle("megle:list-libraries", (event) => {
  requireDesktopIpcAccess(event, "megle:list-libraries");
  return toSummaries(getRegistry());
});
ipcMain.handle("megle:get-active-library-id", (event) => {
  requireDesktopIpcAccess(event, "megle:get-active-library-id");
  return getRegistry().activeLibraryId;
});

ipcMain.handle("megle:switch-library", async (event, libraryId: string) => {
  requireDesktopIpcAccess(event, "megle:switch-library");
  try {
    return await coreLifecycleQueue.enqueue(async (signal) => {
      const generation = coreLifecycleQueue.beginGeneration();
      const reg = getRegistry();
      const target = reg.libraries.find((l) => l.id === libraryId);
      if (!target) return { ok: false, error: "library not found" };
      if (!fsSync.existsSync(target.dbPath)) return { ok: false, error: "library file missing" };
      await restartCoreForLibrary(target.dbPath, signal, generation, () => {
        persistRegistry(libraryRegistry.setActive(reg, libraryId, Date.now()));
      });
      pushLibraryPhase("loading-media"); // renderer takes over from here
      return { ok: true };
    });
  } catch (error) {
    pushLibraryPhase({ kind: "error", message: String(error) });
    return { ok: false, error: String(error) };
  }
});

ipcMain.handle("megle:create-library", async (event, input: { name: string; dbPath: string }) => {
  requireDesktopIpcAccess(event, "megle:create-library");
  try {
    return await coreLifecycleQueue.enqueue(async (signal) => {
      const generation = coreLifecycleQueue.beginGeneration();
      throwIfLifecycleAborted(signal);
      const reg = getRegistry();
      // A brand-new empty file: Core builds the schema on first open. Never
      // silently degrade "create" into "open" of an existing file — that would
      // discard the typed name via addLibrary's dbPath dedupe.
      if (fsSync.existsSync(input.dbPath)) {
        return { ok: false, error: 'A file already exists at that path; use "Open other library" instead.' };
      }
      fsSync.writeFileSync(input.dbPath, ""); // touch; Core opens + migrates
      const added = libraryRegistry.addLibrary(reg, { name: input.name, dbPath: input.dbPath, now: Date.now() });
      // Keep the candidate discoverable while its Core is starting, but do
      // not change activeLibraryId until health succeeds.
      persistRegistry(added.registry);
      await restartCoreForLibrary(added.entry.dbPath, signal, generation, () => {
        persistRegistry(libraryRegistry.setActive(getRegistry(), added.entry.id, Date.now()));
      });
      pushLibraryPhase("loading-media");
      const summary = toSummaries(getRegistry()).find((s) => s.id === added.entry.id);
      return { ok: true, library: summary };
    });
  } catch (error) {
    pushLibraryPhase({ kind: "error", message: String(error) });
    return { ok: false, error: String(error) };
  }
});

ipcMain.handle("megle:open-library", async (event, dbPath: string) => {
  requireDesktopIpcAccess(event, "megle:open-library");
  try {
    return await coreLifecycleQueue.enqueue(async (signal) => {
      const generation = coreLifecycleQueue.beginGeneration();
      throwIfLifecycleAborted(signal);
      const reg = getRegistry();
      if (!fsSync.existsSync(dbPath)) return { ok: false, error: "file not found" };
      const name = path.basename(dbPath).replace(/\.sqlite$/i, "");
      const added = libraryRegistry.addLibrary(reg, { name, dbPath, now: Date.now() });
      // Keep the candidate discoverable while its Core is starting, but do
      // not change activeLibraryId until health succeeds.
      persistRegistry(added.registry);
      await restartCoreForLibrary(added.entry.dbPath, signal, generation, () => {
        persistRegistry(libraryRegistry.setActive(getRegistry(), added.entry.id, Date.now()));
      });
      pushLibraryPhase("loading-media");
      const summary = toSummaries(getRegistry()).find((s) => s.id === added.entry.id);
      return { ok: true, library: summary };
    });
  } catch (error) {
    pushLibraryPhase({ kind: "error", message: String(error) });
    return { ok: false, error: String(error) };
  }
});

ipcMain.handle("megle:remove-library-from-list", (event, libraryId: string) => {
  requireDesktopIpcAccess(event, "megle:remove-library-from-list");
  persistRegistry(libraryRegistry.removeLibrary(getRegistry(), libraryId));
  return { ok: true };
});

ipcMain.handle("megle:rename-library", (event, libraryId: string, name: string) => {
  requireDesktopIpcAccess(event, "megle:rename-library");
  persistRegistry(libraryRegistry.renameLibrary(getRegistry(), libraryId, name));
  return { ok: true };
});

ipcMain.handle("megle:set-library-icon", (event, libraryId: string, icon: string, color: string) => {
  requireDesktopIpcAccess(event, "megle:set-library-icon");
  persistRegistry(libraryRegistry.setLibraryIcon(getRegistry(), libraryId, icon, color));
  return { ok: true };
});

// Destructive: delete the library's .sqlite (plus its WAL/SHM sidecars) from
// disk AND drop it from the registry. Forbidden on the active library — Core
// holds that file open (Windows would fail the unlink anyway), and the UI greys
// the action. The renderer performs its own second confirmation before calling.
ipcMain.handle("megle:delete-library-file", (event, libraryId: string) => {
  requireDesktopIpcAccess(event, "megle:delete-library-file");
  const reg = getRegistry();
  const target = reg.libraries.find((l) => l.id === libraryId);
  if (!target) return { ok: false, error: "library not found" };
  if (reg.activeLibraryId === libraryId) {
    return { ok: false, error: "cannot delete the active library" };
  }
  try {
    for (const suffix of ["", "-wal", "-shm"]) {
      const filePath = target.dbPath + suffix;
      if (fsSync.existsSync(filePath)) {
        fsSync.rmSync(filePath, { force: true });
      }
    }
    persistRegistry(libraryRegistry.removeLibrary(reg, libraryId));
    return { ok: true };
  } catch (error) {
    return { ok: false, error: String(error) };
  }
});

ipcMain.handle("megle:pick-library-file", async (event) => {
  requireDesktopIpcAccess(event, "megle:pick-library-file");
  const result = mainWindow
    ? await dialog.showOpenDialog(mainWindow, { properties: ["openFile"], filters: [{ name: "Megle Library", extensions: ["sqlite"] }] })
    : await dialog.showOpenDialog({ properties: ["openFile"], filters: [{ name: "Megle Library", extensions: ["sqlite"] }] });
  return result.canceled ? null : (result.filePaths[0] ?? null);
});

ipcMain.handle("megle:pick-new-library-path", async (event, defaultName?: string) => {
  requireDesktopIpcAccess(event, "megle:pick-new-library-path");
  const result = mainWindow
    ? await dialog.showSaveDialog(mainWindow, { defaultPath: `${defaultName ?? "library"}.sqlite`, filters: [{ name: "Megle Library", extensions: ["sqlite"] }] })
    : await dialog.showSaveDialog({ defaultPath: `${defaultName ?? "library"}.sqlite`, filters: [{ name: "Megle Library", extensions: ["sqlite"] }] });
  return result.canceled ? null : (result.filePath ?? null);
});

async function createWindow(): Promise<void> {
  const session = await ensureCoreReady();
  const state = await loadWindowState();
  restoreSecondaryWindowPositions(state.secondaryWindows);
  const placement = resolveVisibleWindowPlacement(clampWindowPlacement(state));
  const bootstrapPlacement = compositionBootstrapPlacement(placement);
  const runId = visualHarnessRunId();
  launchWindowPlacement = placement;
  launchWindowMaximized = state.maximized;
  windowStatePersistenceEnabled = false;

  mainWindow = new BrowserWindow({
    title: runId ? `Megle ${runId}` : "Megle",
    width: placement.width,
    height: placement.height,
    x: bootstrapPlacement.x,
    y: bootstrapPlacement.y,
    minWidth: 1100,
    minHeight: 720,
    backgroundMaterial: "acrylic",
    transparent: false,
    backgroundColor: "#00000000",
    roundedCorners: true,
    show: true,
    frame: false,
    skipTaskbar: true,
    webPreferences: {
      preload: fileURLToPath(new URL("./preload.cjs", import.meta.url)),
      contextIsolation: true,
      nodeIntegration: false,
      additionalArguments: [
        `--megle-core-url=${session.baseUrl}`,
        `--megle-session-token=${session.sessionToken}`,
        `--megle-window=main`,
        ...(isVisualHarnessEnabled() ? ["--megle-visual-harness=1"] : [])
      ]
    }
  });
  const window = mainWindow;
  const releaseIpcTargetLifecycle = notificationIpcTargetLifecycle.track(window.webContents);
  applyWindowMaterial(window);
  shellReadyVisibleWindowId = null;
  shellReadyRevealPromise = null;
  clearShellReadyFailureFallback();
  mainWindowReadyToShow = new Promise<void>((resolve) => {
    window.once("ready-to-show", () => {
      void (async () => {
        await prepareWindowCompositionForReveal(window);
        armShellReadyFailureFallback(window);
        resolve();
      })();
    });
  });

  window.webContents.on(
    "did-fail-load",
    (_event, errorCode, _errorDescription, _validatedUrl, isMainFrame) => {
      if (!isMainFrame) return;
      desktopLogger.error("renderer", "Main renderer failed to load", { errorCode });
      void revealMainWindowForLaunchFailure(window, `did-fail-load:${errorCode}`);
    }
  );
  window.webContents.on("render-process-gone", (_event, details) => {
    desktopLogger.error("renderer", "Main renderer process gone", {
      reason: details.reason,
      exitCode: details.exitCode
    });
    void revealMainWindowForLaunchFailure(window, `render-process-gone:${details.reason}`);
  });

  // Persist on move/resize so a forced shutdown loses at most
  // PERSIST_DEBOUNCE_MS of window state. The `before-quit` handler still
  // writes synchronously so a clean exit always flushes the latest bounds.
  window.on("move", schedulePersistWindowState);
  window.on("resize", () => {
    refreshWindowComposition(window);
    schedulePersistWindowState();
  });
  window.on("maximize", () => refreshWindowComposition(window));
  window.on("unmaximize", () => refreshWindowComposition(window));
  window.on("blur", () => {
    if (activeWindowDrag?.windowId === window.id) {
      activeWindowDrag = null;
    }
  });
  window.on("close", closeAllSecondaryWindows);

  window.on("closed", () => {
    mediaFocusCoordinator.failAll();
    releaseIpcTargetLifecycle();
    if (activeWindowDrag?.windowId === window.id) {
      activeWindowDrag = null;
    }
    launchWindowPlacement = null;
    launchWindowMaximized = false;
    windowStatePersistenceEnabled = false;
    clearShellReadyFailureFallback();
    mainWindowReadyToShow = null;
    shellReadyRevealPromise = null;
    shellReadyVisibleWindowId = null;
    mainWindow = null;
  });

  const devServer = process.env.MEGLE_WEB_URL ?? "http://127.0.0.1:5173";
  try {
    await window.loadURL(devServer);
    armShellReadyFailureFallback(window);
  } catch (error) {
    await revealMainWindowForLaunchFailure(window, "loadURL-rejected");
    desktopLogger.warn("main", "[megle] desktop renderer loadURL rejected:", error);
  }
}

function focusExistingSecondaryWindow(role: SecondaryWindowRole): boolean {
  const existing = secondaryWindows[role];
  if (!existing || existing.isDestroyed()) return false;
  if (existing.isMinimized()) existing.restore();
  const readiness = secondaryWindowReadiness[role];
  if (readiness?.windowId === existing.id && !readiness.revealState.settled) {
    void revealSecondaryWindowWhenReady(role, existing);
    return true;
  }
  existing.show();
  existing.focus();
  return true;
}

// Settings, Plugins, and Notifications use the same controlled secondary-window lifecycle:
// non-modal child windows, shared preload/Core session, frameless acrylic, and
// one live instance per role. Re-opening restores and focuses the existing one.
function openSecondaryWindow(role: SecondaryWindowRole): Promise<void> {
  if (focusExistingSecondaryWindow(role)) return Promise.resolve();
  return secondaryWindowOpenings.run(role, async (openingToken) => {
    const session = await ensureCoreReady();
    if (focusExistingSecondaryWindow(role)) return;
    await createSecondaryWindow(role, session, openingToken);
  });
}

async function createSecondaryWindow(
  role: SecondaryWindowRole,
  session: CoreSession,
  openingToken: symbol
): Promise<void> {
  const config = SECONDARY_WINDOW_CONFIGS[role];
  const parent = mainWindow && !mainWindow.isDestroyed() ? mainWindow : undefined;
  const position = resolveSecondaryWindowPosition(role, parent, config);
  const window = new BrowserWindow({
    title: config.title,
    width: config.width,
    height: config.height,
    x: position.x,
    y: position.y,
    minWidth: config.minWidth,
    minHeight: config.minHeight,
    parent,
    modal: false,
    backgroundMaterial: "acrylic",
    opacity: 0,
    transparent: false,
    backgroundColor: "#00000000",
    roundedCorners: true,
    show: false,
    frame: false,
    skipTaskbar: false,
    webPreferences: {
      preload: fileURLToPath(new URL("./preload.cjs", import.meta.url)),
      contextIsolation: true,
      nodeIntegration: false,
      additionalArguments: [
        `--megle-core-url=${session.baseUrl}`,
        `--megle-session-token=${session.sessionToken}`,
        `--megle-window=${role}`,
        ...(isVisualHarnessEnabled() ? ["--megle-visual-harness=1"] : [])
      ]
    }
  });
  const releaseIpcTargetLifecycle = notificationIpcTargetLifecycle.track(window.webContents);
  secondaryWindows[role] = window;
  secondaryWindowReadiness[role] = {
    windowId: window.id,
    revealState: createSecondaryWindowRevealState<SecondaryWindowRevealResult | false>(),
    nativeWarmupStartedAt: null,
    nativeWindowRevealedAt: null,
    fallbackTimer: setTimeout(() => {
      forceSecondaryWindowReveal(role, window);
    }, SECONDARY_WINDOW_READY_TIMEOUT_MS)
  };
  applyWindowMaterial(window);
  window.once("ready-to-show", () => {
    const readiness = secondaryWindowReadiness[role];
    if (!readiness || readiness.windowId !== window.id) return;
    readiness.revealState.nativeReady = true;
    revealSecondaryWindowWhenReady(role, window);
  });
  window.webContents.on(
    "did-fail-load",
    (_event, errorCode, _errorDescription, _validatedUrl, isMainFrame) => {
      if (!isMainFrame) return;
      desktopLogger.error("renderer", `${role} renderer failed to load`, { errorCode });
      destroyFailedSecondaryWindow(
        role,
        window,
        openingToken,
        releaseIpcTargetLifecycle
      );
    }
  );
  window.webContents.on("render-process-gone", (_event, details) => {
    desktopLogger.error("renderer", `${role} renderer process exited`, {
      reason: details.reason,
      exitCode: details.exitCode
    });
    destroyFailedSecondaryWindow(
      role,
      window,
      openingToken,
      releaseIpcTargetLifecycle
    );
  });
  window.on("blur", () => {
    if (activeWindowDrag?.windowId === window.id) {
      activeWindowDrag = null;
    }
  });
  window.on("move", () => {
    rememberSecondaryWindowPosition(role, window);
    schedulePersistWindowState();
  });
  window.on("closed", () => {
    clearSecondaryWindowState(
      role,
      window,
      openingToken,
      releaseIpcTargetLifecycle
    );
  });

  const devServer = process.env.MEGLE_WEB_URL ?? "http://127.0.0.1:5173";
  try {
    await window.loadURL(`${devServer}${config.route}`);
  } catch (error) {
    desktopLogger.warn("main", `[megle] ${role} window loadURL rejected:`, error);
    destroyFailedSecondaryWindow(
      role,
      window,
      openingToken,
      releaseIpcTargetLifecycle
    );
    throw error;
  }
}

function clearSecondaryWindowState(
  role: SecondaryWindowRole,
  window: BrowserWindow,
  openingToken: symbol,
  releaseIpcTargetLifecycle: () => void
): void {
  secondaryWindowOpenings.clear(role, openingToken);
  releaseIpcTargetLifecycle();
  if (activeWindowDrag?.windowId === window.id) activeWindowDrag = null;
  if (secondaryWindows[role] === window) secondaryWindows[role] = null;
  clearSecondaryWindowReadyFallback(role, window.id);
  const readiness = secondaryWindowReadiness[role];
  if (readiness?.windowId === window.id) {
    readiness.revealState.settle(false);
    secondaryWindowReadiness[role] = null;
  }
}

function destroyFailedSecondaryWindow(
  role: SecondaryWindowRole,
  window: BrowserWindow,
  openingToken: symbol,
  releaseIpcTargetLifecycle: () => void
): void {
  clearSecondaryWindowState(
    role,
    window,
    openingToken,
    releaseIpcTargetLifecycle
  );
  if (!window.isDestroyed()) window.destroy();
}

function isSecondaryWindowRole(value: unknown): value is SecondaryWindowRole {
  return value === "settings" || value === "plugins" || value === "notifications";
}

function restoreSecondaryWindowPositions(
  positions: Partial<Record<SecondaryWindowRole, SecondaryWindowPosition>> | undefined
): void {
  for (const role of Object.keys(secondaryWindowPositions) as SecondaryWindowRole[]) {
    const position = positions?.[role];
    secondaryWindowPositions[role] =
      position && Number.isFinite(position.x) && Number.isFinite(position.y)
        ? { x: Math.round(position.x), y: Math.round(position.y) }
        : null;
  }
}

function resolveSecondaryWindowPosition(
  role: SecondaryWindowRole,
  parent: BrowserWindow | undefined,
  config: SecondaryWindowConfig
): SecondaryWindowPosition {
  const saved = secondaryWindowPositions[role];
  if (saved) {
    return clampSecondaryWindowPosition(saved, config.width, config.height);
  }

  const anchor = parent && !parent.isDestroyed() ? parent.getBounds() : screen.getPrimaryDisplay().workArea;
  return clampSecondaryWindowPosition(
    {
      x: Math.round(anchor.x + (anchor.width - config.width) / 2),
      y: Math.round(anchor.y + (anchor.height - config.height) / 2)
    },
    config.width,
    config.height
  );
}

function clampSecondaryWindowPosition(
  position: SecondaryWindowPosition,
  width: number,
  height: number
): SecondaryWindowPosition {
  const display = screen.getDisplayMatching({ x: position.x, y: position.y, width, height });
  const workArea = display.workArea;
  const maxX = Math.max(workArea.x, workArea.x + workArea.width - width);
  const maxY = Math.max(workArea.y, workArea.y + workArea.height - height);
  return {
    x: Math.round(Math.min(maxX, Math.max(workArea.x, position.x))),
    y: Math.round(Math.min(maxY, Math.max(workArea.y, position.y)))
  };
}

function rememberSecondaryWindowPosition(role: SecondaryWindowRole, window: BrowserWindow): void {
  if (window.isDestroyed()) return;
  const bounds = window.isMaximized() ? window.getNormalBounds() : window.getBounds();
  secondaryWindowPositions[role] = { x: bounds.x, y: bounds.y };
}

function clearSecondaryWindowReadyFallback(role: SecondaryWindowRole, windowId: number): void {
  const readiness = secondaryWindowReadiness[role];
  if (!readiness || readiness.windowId !== windowId || !readiness.fallbackTimer) return;
  clearTimeout(readiness.fallbackTimer);
  readiness.fallbackTimer = null;
}

function revealSecondaryWindowWhenReady(
  role: SecondaryWindowRole,
  window: BrowserWindow
): void {
  if (window.isDestroyed() || secondaryWindows[role] !== window) return;
  const readiness = secondaryWindowReadiness[role];
  if (!readiness || readiness.windowId !== window.id) return;
  const warmupToken = readiness.revealState.beginWarmup();
  if (warmupToken === null) return;
  void warmSecondaryWindowNativeHost(role, window, readiness, warmupToken)
    .catch((error) => {
      desktopLogger.warn("main", `[megle] ${role} native window warm-up failed:`, error);
      forceSecondaryWindowReveal(role, window);
    })
    .finally(() => {
      readiness.revealState.finishWarmup(warmupToken);
    });
}

async function warmSecondaryWindowNativeHost(
  role: SecondaryWindowRole,
  window: BrowserWindow,
  readiness: SecondaryWindowReadiness,
  warmupToken: number
): Promise<void> {
  if (window.isMinimized()) window.restore();
  applyWindowMaterial(window);
  window.showInactive();
  if (readiness.nativeWarmupStartedAt === null) {
    readiness.nativeWarmupStartedAt = Date.now();
  }
  const didWarm = await waitForSecondaryNativeWindowWarmup(
    readiness.revealState,
    warmupToken,
    waitForRendererFrame(window),
    NATIVE_WINDOW_WARMUP_MS
  );
  if (!didWarm) return;
  if (window.isDestroyed() || secondaryWindows[role] !== window) {
    readiness.revealState.settle(false);
    return;
  }
  clearSecondaryWindowReadyFallback(role, window.id);
  readiness.nativeWindowRevealedAt = Date.now();
  const result: SecondaryWindowRevealResult = {
    nativeWarmupStartedAt: readiness.nativeWarmupStartedAt,
    nativeWindowRevealedAt: readiness.nativeWindowRevealedAt,
    fallback: false
  };
  revealSecondaryNativeWindow(readiness.revealState, result, window);
}

function forceSecondaryWindowReveal(
  role: SecondaryWindowRole,
  window: BrowserWindow
): SecondaryWindowRevealResult | null {
  if (window.isDestroyed() || secondaryWindows[role] !== window) return null;
  const readiness = secondaryWindowReadiness[role];
  if (!readiness || readiness.windowId !== window.id) return null;
  if (readiness.revealState.settled) {
    const value = readiness.revealState.value;
    return value === false ? null : (value ?? null);
  }
  clearSecondaryWindowReadyFallback(role, window.id);
  if (window.isMinimized()) window.restore();
  applyWindowMaterial(window);
  if (!window.isVisible()) window.showInactive();
  if (readiness.nativeWarmupStartedAt === null) {
    readiness.nativeWarmupStartedAt = Date.now();
  }
  readiness.nativeWindowRevealedAt = Date.now();
  const result: SecondaryWindowRevealResult = {
    nativeWarmupStartedAt: readiness.nativeWarmupStartedAt,
    nativeWindowRevealedAt: readiness.nativeWindowRevealedAt,
    fallback: true
  };
  if (!revealSecondaryNativeWindow(readiness.revealState, result, window)) {
    const value = readiness.revealState.value;
    return value === false ? null : (value ?? null);
  }
  return result;
}

function closeSecondaryWindow(role: SecondaryWindowRole): void {
  const window = secondaryWindows[role];
  if (window && !window.isDestroyed()) {
    window.close();
  }
}

function closeAllSecondaryWindows(): void {
  for (const role of Object.keys(secondaryWindows) as SecondaryWindowRole[]) {
    closeSecondaryWindow(role);
  }
}

async function ensureCoreReady(): Promise<CoreSession> {
  if (coreReadyPromise) {
    return coreReadyPromise;
  }

  const startup = coreLifecycleQueue.enqueue(async (signal) => {
    throwIfLifecycleAborted(signal);
    const reusable = await currentHealthyCoreSession(signal);
    if (reusable) return reusable;

    const generation = coreLifecycleQueue.beginGeneration();
    return runNotificationSafeCoreTransition({
      assertCurrent: () => assertCurrentCoreGeneration(generation, signal),
      detachNotifications: detachNotificationServiceForCoreReplacement,
      stopOldCore: stopDesktopCoreProcesses,
      startHealthyCore: () => startCoreSession(signal, generation),
      commitActiveLibrary: () => undefined,
      attachNotifications: (session) =>
        attachNotificationServiceForSession(session, generation, signal)
    });
  });
  coreReadyPromise = startup;
  const clearReadyPromise = () => {
    if (coreReadyPromise === startup) coreReadyPromise = null;
  };
  void startup.then(clearReadyPromise, clearReadyPromise);
  return startup;
}

async function currentHealthyCoreSession(signal: AbortSignal): Promise<CoreSession | null> {
  throwIfLifecycleAborted(signal);
  const handle = coreProcess;
  const session = coreSession;
  if (!handle || !session) return null;
  const generation = coreLifecycleQueue.currentGeneration;
  try {
    await waitForCoreHealth(
      session.baseUrl,
      session.sessionToken,
      CORE_REUSE_HEALTH_TIMEOUT_MS,
      signal
    );
  } catch (error) {
    throwIfLifecycleAborted(signal);
    desktopLogger.warn("main", "[megle] current Core health recheck failed; restarting tracked process:", error);
    return null;
  }
  throwIfLifecycleAborted(signal);
  if (
    !coreLifecycleQueue.isCurrentGeneration(generation) ||
    coreProcess !== handle ||
    coreSession !== session
  ) {
    return null;
  }
  return session;
}

function assertCurrentCoreGeneration(generation: number, signal: AbortSignal): void {
  throwIfLifecycleAborted(signal);
  if (!coreLifecycleQueue.isCurrentGeneration(generation)) {
    throw new LifecycleCancelledError("Stale Core lifecycle generation");
  }
}

async function startCoreSession(signal: AbortSignal, generation: number): Promise<CoreSession> {
  const maxAttempts = shouldRetryAutoCoreStartup() ? AUTO_CORE_STARTUP_MAX_ATTEMPTS : 1;

  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    let pendingProcess: CoreProcessHandle | null = null;

    assertCurrentCoreGeneration(generation, signal);
    try {
      await stopPendingCoreProcess();
    } catch (error) {
      // A failed pending child may still own the bind port. Do not launch a
      // second child while it is retained for the next switch or shutdown.
      coreReadyPromise = null;
      desktopLogger.warn("main", "[megle] pending Core cleanup failed before startup; retaining process for retry:", error);
      throw error;
    }

    try {
      assertCurrentCoreGeneration(generation, signal);
      const session = coreSession ?? (await createColdStartSession());
      assertCurrentCoreGeneration(generation, signal);
      if (coreProcess) {
        const reusable = await currentHealthyCoreSession(signal);
        if (reusable) return reusable;
        throw new LifecycleCancelledError("Tracked Core changed before startup");
      }
      pendingProcess = startCoreProcess(session, coreLogHandlerForSession(session));
      pendingCoreProcess = pendingProcess;
      await waitForCoreHealth(session.baseUrl, session.sessionToken, 12_000, signal);
      assertCurrentCoreGeneration(generation, signal);
      if (coreProcess && coreProcess !== pendingProcess) {
        throw new LifecycleCancelledError("A newer Core process was committed before startup completed");
      }
      coreProcess = pendingProcess;
      pendingCoreProcess = null;
      coreSession = session;
      if (attempt > 1) {
        desktopLogger.info("main", `[megle] Core startup recovered on attempt ${attempt}/${maxAttempts}`);
      }
      return session;
    } catch (error) {
      let cleanupFailed = false;
      if (pendingProcess) {
        try {
          await stopPendingCoreProcess();
        } catch (stopError) {
          cleanupFailed = true;
          desktopLogger.warn("main", "[megle] pending Core cleanup failed; retaining process for retry:", stopError);
        }
      }
      if (coreProcess === pendingProcess) coreProcess = null;

      // Do not start another child while an unhealthy pending child may still
      // own the bind port. The next library switch or shutdown retries it.
      if (cleanupFailed) {
        coreReadyPromise = null;
        throw error;
      }
      assertCurrentCoreGeneration(generation, signal);

      if (attempt < maxAttempts) {
        desktopLogger.warn(
          "main",
          `[megle] Core startup attempt ${attempt}/${maxAttempts} failed; retrying with a fresh session/port:`,
          error
        );
        continue;
      }

      coreReadyPromise = null;
      desktopLogger.error(
        "main",
        `[megle] Core startup attempt ${attempt}/${maxAttempts} failed; no retries remain:`,
        error
      );
      throw error;
    }
  }

  coreReadyPromise = null;
  throw new Error("Core startup retry loop exited without a session");
}

// On cold start, point Core at the active library from the registry (if any),
// otherwise fall back to createCoreSession()'s default dbPath (first-run / dev).
async function createColdStartSession(): Promise<CoreSession> {
  const base = await createCoreSession();
  const reg = getRegistry();
  const active = reg.libraries.find((l) => l.id === reg.activeLibraryId);
  if (active && fsSync.existsSync(active.dbPath)) {
    return sessionForDbPath(base, active.dbPath);
  }
  // No active library yet: register the default dbPath as the first library so
  // the switcher has something to show, and make it active.
  const added = libraryRegistry.addLibrary(reg, {
    name: "Default",
    dbPath: base.dbPath,
    now: Date.now()
  });
  persistRegistry(libraryRegistry.setActive(added.registry, added.entry.id, Date.now()));
  return base;
}

// Gracefully restart Core pointed at a new database file, reusing the same
// bind address + session token so the renderer keeps its injected coreUrl/token
// (spec Decision 6). Pushes load phases to the renderer as it goes.
async function stopPendingCoreProcess(): Promise<void> {
  const handle = pendingCoreProcess;
  await stopCoreProcessHandleAndClear(handle, () => {
    if (pendingCoreProcess === handle) pendingCoreProcess = null;
  });
}

async function stopDesktopCoreProcesses(): Promise<void> {
  const pending = pendingCoreProcess;
  const current = coreProcess;
  await stopCoreProcessesBeforeReplacement(
    pending,
    current,
    () => {
      if (pendingCoreProcess === pending) pendingCoreProcess = null;
    },
    () => {
      if (coreProcess === current) coreProcess = null;
    }
  );
}

async function restartCoreForLibrary(
  dbPath: string,
  signal: AbortSignal,
  generation: number,
  commitActiveLibrary: (session: CoreSession) => void | Promise<void>
): Promise<CoreSession> {
  assertCurrentCoreGeneration(generation, signal);
  pushLibraryPhase("starting-core");
  const base = coreSession ?? pendingCoreSession ?? (await createCoreSession());
  assertCurrentCoreGeneration(generation, signal);
  const session = sessionForDbPath(base, dbPath, pendingCoreSession);
  // Keep both the last-known renderer session and the candidate session until
  // the replacement Core is healthy. The renderer keeps using baseUrl/token
  // throughout this transaction, while the candidate is reusable on retry.
  pendingCoreSession = session;
  let handle: CoreProcessHandle | null = null;
  try {
    return await runNotificationSafeCoreTransition({
      assertCurrent: () => assertCurrentCoreGeneration(generation, signal),
      detachNotifications: detachNotificationServiceForCoreReplacement,
      stopOldCore: async () => {
        // Stop a failed pending Core before the active Core and wait for the port to free up.
        await stopDesktopCoreProcesses();
        assertCurrentCoreGeneration(generation, signal);
        await waitForPortFree(base.bindAddress, signal);
        assertCurrentCoreGeneration(generation, signal);
        if (coreProcess) {
          throw new LifecycleCancelledError("Tracked Core changed before library restart");
        }
      },
      startHealthyCore: async () => {
        handle = startCoreProcess(session, coreLogHandlerForSession(session));
        pendingCoreProcess = handle;
        pushLibraryPhase("opening-library");
        await waitForCoreHealth(session.baseUrl, session.sessionToken, 12_000, signal);
        assertCurrentCoreGeneration(generation, signal);
        if (coreProcess && coreProcess !== handle) {
          throw new LifecycleCancelledError(
            "A newer Core process was committed before library restart completed"
          );
        }
        return session;
      },
      commitActiveLibrary: async (healthySession) => {
        await commitActiveLibrary(healthySession);
        assertCurrentCoreGeneration(generation, signal);
        if (!handle || pendingCoreProcess !== handle) {
          throw new LifecycleCancelledError("Candidate Core changed before library commit");
        }
        coreProcess = handle;
        pendingCoreProcess = null;
        coreSession = healthySession;
        pendingCoreSession = null;
      },
      attachNotifications: (healthySession) =>
        attachNotificationServiceForSession(healthySession, generation, signal)
    });
  } catch (error) {
    if (handle && pendingCoreProcess === handle) {
      try {
        await stopPendingCoreProcess();
      } catch (stopError) {
        desktopLogger.warn("main", "[megle] pending Core cleanup failed after library switch; retaining process:", stopError);
      }
    }
    if (!signal.aborted) {
      desktopLogger.error("main", "[megle] library Core replacement failed; retaining session metadata for retry:", error);
    }
    throw error;
  }
}

// Poll until the host:port can be bound again (Core's listener has closed),
// guarding the known probe-and-close race. Bounded; throws on timeout.
async function waitForPortFree(bindAddress: string, signal: AbortSignal): Promise<void> {
  const [host, portStr] = bindAddress.split(":");
  const port = Number(portStr);
  const deadline = Date.now() + 5000;
  // eslint-disable-next-line no-constant-condition
  while (true) {
    throwIfLifecycleAborted(signal);
    const free = await new Promise<boolean>((resolve) => {
      const probe = net.createServer();
      probe.once("error", () => resolve(false));
      probe.listen(port, host, () => probe.close(() => resolve(true)));
    });
    if (free) return;
    if (Date.now() > deadline) throw new Error(`Port ${bindAddress} did not free up`);
    await waitWithLifecycleSignal(80, signal);
  }
}

function waitWithLifecycleSignal(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    let settled = false;
    let timer: NodeJS.Timeout;
    const cleanup = () => signal.removeEventListener("abort", onAbort);
    timer = setTimeout(() => {
      settled = true;
      cleanup();
      resolve();
    }, ms);
    const onAbort = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      cleanup();
      reject(new LifecycleCancelledError());
    };
    if (signal.aborted) {
      onAbort();
      return;
    }
    signal.addEventListener("abort", onAbort, { once: true });
  });
}

function shouldRetryAutoCoreStartup(): boolean {
  return (
    process.env.MEGLE_CORE_EXTERNAL !== "1" &&
    !process.env.MEGLE_CORE_ADDR?.trim() &&
    !process.env.MEGLE_CORE_URL?.trim()
  );
}

function isVisualHarnessEnabled(): boolean {
  return (
    process.env.MEGLE_VISUAL_HARNESS === "1" ||
    process.argv.some((arg) => arg === "--megle-visual-harness=1")
  );
}

function visualHarnessRunId(): string | null {
  const fromEnv = process.env.MEGLE_VISUAL_RUN_ID?.trim();
  if (fromEnv) return fromEnv;
  const arg = process.argv.find((item) => item.startsWith("--megle-visual-run-id="));
  return arg ? arg.slice("--megle-visual-run-id=".length) : null;
}

async function waitForRendererFrame(window: BrowserWindow) {
  if (window.isDestroyed()) return;
  try {
    await window.webContents.executeJavaScript(
      `new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)))`,
      true
    );
  } catch {
    await new Promise((resolve) => setTimeout(resolve, 34));
  }
}

function clearShellReadyFailureFallback() {
  if (!shellReadyFailureFallbackTimer) {
    return;
  }
  clearTimeout(shellReadyFailureFallbackTimer);
  shellReadyFailureFallbackTimer = null;
}

function armShellReadyFailureFallback(window: BrowserWindow) {
  if (window.isDestroyed() || shellReadyVisibleWindowId === window.id || shellReadyRevealPromise) {
    return;
  }
  clearShellReadyFailureFallback();
  shellReadyFailureFallbackTimer = setTimeout(() => {
    shellReadyFailureFallbackTimer = null;
    void revealMainWindowForLaunchFailure(window, "shell-ready-timeout");
  }, SHELL_READY_FAILURE_TIMEOUT_MS);
}

async function revealMainWindow(
  window: BrowserWindow,
  { waitForRendererPaint = false }: { waitForRendererPaint?: boolean } = {}
): Promise<boolean> {
  clearShellReadyFailureFallback();
  if (window.isDestroyed()) {
    return false;
  }
  if (shellReadyVisibleWindowId === window.id) {
    return true;
  }
  applyWindowMaterial(window);
  restoreWindowFromCompositionBootstrap(window);
  if (waitForRendererPaint) {
    await waitForRendererFrame(window);
    await new Promise((resolve) => setTimeout(resolve, 80));
  }
  if (window.isDestroyed()) {
    return false;
  }
  scheduleWindowCompositionRefresh(window);
  window.setSkipTaskbar(false);
  window.show();
  window.focus();
  windowStatePersistenceEnabled = true;
  shellReadyVisibleWindowId = window.id;
  return true;
}

async function revealMainWindowForShellReady(window: BrowserWindow): Promise<boolean> {
  await prepareWindowCompositionForReveal(window);
  return revealMainWindow(window);
}

async function revealMainWindowForLaunchFailure(
  window: BrowserWindow,
  reason: string
): Promise<boolean> {
  if (window.isDestroyed() || shellReadyVisibleWindowId === window.id) {
    clearShellReadyFailureFallback();
    return !window.isDestroyed();
  }
  const didReveal = await revealMainWindow(window);
  if (didReveal) {
    desktopLogger.warn("main", `[megle] revealing window after renderer startup failure: ${reason}`);
  }
  return didReveal;
}

function windowStatePath(): string {
  return path.join(app.getPath("userData"), "window-state.json");
}

async function loadWindowState(): Promise<WindowState> {
  try {
    const raw = await fsp.readFile(windowStatePath(), "utf8");
    const parsed = JSON.parse(raw) as Partial<WindowState>;
    return {
      width: typeof parsed.width === "number" ? parsed.width : DEFAULT_WINDOW_STATE.width,
      height: typeof parsed.height === "number" ? parsed.height : DEFAULT_WINDOW_STATE.height,
      x: typeof parsed.x === "number" ? parsed.x : undefined,
      y: typeof parsed.y === "number" ? parsed.y : undefined,
      maximized: parsed.maximized === true,
      secondaryWindows: readSecondaryWindowPositions(parsed.secondaryWindows)
    };
  } catch {
    return { ...DEFAULT_WINDOW_STATE };
  }
}

function readSecondaryWindowPositions(value: unknown):
  | Partial<Record<SecondaryWindowRole, SecondaryWindowPosition>>
  | undefined {
  if (!value || typeof value !== "object") return undefined;
  const source = value as Record<string, unknown>;
  const positions: Partial<Record<SecondaryWindowRole, SecondaryWindowPosition>> = {};
  for (const role of ["settings", "plugins", "notifications"] as const) {
    const candidate = source[role];
    if (!candidate || typeof candidate !== "object") continue;
    const position = candidate as { x?: unknown; y?: unknown };
    if (typeof position.x === "number" && typeof position.y === "number") {
      positions[role] = { x: position.x, y: position.y };
    }
  }
  return positions;
}

/// Bounds-check a saved window placement against the currently connected
/// displays. After a monitor disconnect a saved (x, y) can land entirely
/// offscreen, so we look up a display that contains the saved rect and
/// either clamp the position into its `workArea` or drop x/y entirely
/// (Electron then picks a sensible default). Width/height are always
/// honored — a too-wide window is still preferable to ignoring the user's
/// preference outright.
function clampWindowPlacement(state: WindowState): {
  width: number;
  height: number;
  x?: number;
  y?: number;
} {
  if (typeof state.x !== "number" || typeof state.y !== "number") {
    return { width: state.width, height: state.height };
  }

  const savedBounds = {
    x: state.x,
    y: state.y,
    width: state.width,
    height: state.height
  };
  const display = screen.getDisplayMatching(savedBounds);
  const workArea = display.workArea;

  // Reject the saved x/y if the saved rect doesn't actually overlap the
  // matched display's work area. `getDisplayMatching` always returns *a*
  // display, even when the saved rect is far outside any monitor, so this
  // overlap check is what catches the post-disconnect case.
  const overlapsHorizontally =
    savedBounds.x + savedBounds.width > workArea.x &&
    savedBounds.x < workArea.x + workArea.width;
  const overlapsVertically =
    savedBounds.y + savedBounds.height > workArea.y &&
    savedBounds.y < workArea.y + workArea.height;

  if (!overlapsHorizontally || !overlapsVertically) {
    return { width: state.width, height: state.height };
  }

  // Clamp x/y so the window is fully within the matched display's work
  // area. Partial overlap becomes fully visible.
  const maxX = workArea.x + workArea.width - savedBounds.width;
  const maxY = workArea.y + workArea.height - savedBounds.height;
  const clampedX = Math.max(workArea.x, Math.min(savedBounds.x, maxX));
  const clampedY = Math.max(workArea.y, Math.min(savedBounds.y, maxY));

  return {
    width: state.width,
    height: state.height,
    x: clampedX,
    y: clampedY
  };
}

function resolveVisibleWindowPlacement(placement: {
  width: number;
  height: number;
  x?: number;
  y?: number;
}): WindowPlacement {
  if (typeof placement.x === "number" && typeof placement.y === "number") {
    return {
      width: placement.width,
      height: placement.height,
      x: placement.x,
      y: placement.y
    };
  }

  const workArea = screen.getPrimaryDisplay().workArea;
  return {
    width: placement.width,
    height: placement.height,
    x: Math.round(workArea.x + Math.max(0, (workArea.width - placement.width) / 2)),
    y: Math.round(workArea.y + Math.max(0, (workArea.height - placement.height) / 2))
  };
}

function schedulePersistWindowState(): void {
  if (!windowStatePersistenceEnabled) {
    return;
  }
  if (persistDebounceTimer) {
    clearTimeout(persistDebounceTimer);
  }
  persistDebounceTimer = setTimeout(() => {
    persistDebounceTimer = null;
    void persistWindowState();
  }, PERSIST_DEBOUNCE_MS);
}

function applyWindowMaterial(window: BrowserWindow): void {
  if (window.isDestroyed()) {
    return;
  }
  window.setBackgroundColor("#00000000");
  window.setBackgroundMaterial("acrylic");
}

async function prepareWindowCompositionForReveal(window: BrowserWindow): Promise<void> {
  if (window.isDestroyed()) {
    return;
  }
  window.setSkipTaskbar(true);
  applyWindowMaterial(window);
  primeWindowComposition(window);
  await new Promise((resolve) => setTimeout(resolve, 80));
  if (!window.isDestroyed()) {
    primeWindowComposition(window);
  }
}

function restoreWindowFromCompositionBootstrap(window: BrowserWindow): void {
  const placement = launchWindowPlacement;
  if (!placement || window.isDestroyed()) {
    return;
  }

  windowStatePersistenceEnabled = false;
  window.setBounds(
    {
      x: placement.x,
      y: placement.y,
      width: placement.width,
      height: placement.height
    },
    false
  );
  refreshWindowComposition(window);
  if (launchWindowMaximized) {
    window.maximize();
  }
}

function refreshWindowComposition(window: BrowserWindow): void {
  applyWindowMaterial(window);
}

function scheduleWindowCompositionRefresh(window: BrowserWindow): void {
  refreshWindowComposition(window);
  for (const delay of WINDOW_COMPOSITION_REFRESH_DELAYS_MS) {
    setTimeout(() => {
      if (!window.isDestroyed()) {
        refreshWindowComposition(window);
      }
    }, delay);
  }
}

function primeWindowComposition(window: BrowserWindow): void {
  if (window.isDestroyed()) {
    return;
  }
  const bounds = window.getBounds();
  if (!window.isMaximized() && bounds.width > 2 && bounds.height > 2) {
    window.setBounds({ ...bounds, width: bounds.width + 1 }, false);
    window.setBounds(bounds, false);
  }
  refreshWindowComposition(window);
}

function clampNumber(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function compositionBootstrapPlacement(placement: {
  width: number;
  height: number;
}): { x: number; y: number } {
  const displays = screen.getAllDisplays();
  const union = displays.reduce(
    (acc, display) => {
      const bounds = display.bounds;
      return {
        left: Math.min(acc.left, bounds.x),
        top: Math.min(acc.top, bounds.y),
        right: Math.max(acc.right, bounds.x + bounds.width),
        bottom: Math.max(acc.bottom, bounds.y + bounds.height)
      };
    },
    {
      left: Number.POSITIVE_INFINITY,
      top: Number.POSITIVE_INFINITY,
      right: Number.NEGATIVE_INFINITY,
      bottom: Number.NEGATIVE_INFINITY
    }
  );

  if (!Number.isFinite(union.left) || !Number.isFinite(union.bottom)) {
    const primary = screen.getPrimaryDisplay().bounds;
    return {
      x: primary.x - placement.width - WINDOW_COMPOSITION_BOOTSTRAP_MARGIN_PX,
      y: primary.y - placement.height - WINDOW_COMPOSITION_BOOTSTRAP_MARGIN_PX
    };
  }

  return {
    x: union.left,
    y: union.bottom + WINDOW_COMPOSITION_BOOTSTRAP_MARGIN_PX
  };
}

async function persistWindowState(): Promise<void> {
  if (!mainWindow) return;
  const maximized = mainWindow.isMaximized();
  const bounds = maximized ? mainWindow.getNormalBounds() : mainWindow.getBounds();
  const state: WindowState = {
    width: bounds.width,
    height: bounds.height,
    x: bounds.x,
    y: bounds.y,
    maximized,
    secondaryWindows: Object.fromEntries(
      Object.entries(secondaryWindowPositions).filter((entry): entry is [string, SecondaryWindowPosition] => entry[1] !== null)
    ) as Partial<Record<SecondaryWindowRole, SecondaryWindowPosition>>
  };
  const targetPath = windowStatePath();
  // Atomic write: render to a sibling temp file, then rename. A crash
  // mid-write leaves `window-state.json` untouched instead of corrupting it
  // to truncated JSON the next launch can't parse.
  const tempPath = `${targetPath}.${process.pid}.tmp`;
  try {
    await fsp.mkdir(path.dirname(targetPath), { recursive: true });
    await fsp.writeFile(tempPath, JSON.stringify(state), "utf8");
    await fsp.rename(tempPath, targetPath);
  } catch {
    // Best-effort persistence; never block app shutdown on this. Try to
    // clean up the temp file if we created one.
    try {
      await fsp.unlink(tempPath);
    } catch {
      // ignore
    }
  }
}

async function detectFfmpeg(): Promise<boolean> {
  if (cachedFfmpegAvailable !== null) {
    return cachedFfmpegAvailable;
  }
  cachedFfmpegAvailable = await new Promise<boolean>((resolve) => {
    try {
      const child = spawn("ffmpeg", ["-version"], {
        stdio: "ignore",
        windowsHide: true
      });
      child.on("error", () => resolve(false));
      child.on("exit", (code) => resolve(code === 0));
    } catch {
      resolve(false);
    }
  });
  return cachedFfmpegAvailable;
}

function resolvePluginsDir(dbPath: string): string {
  const fromEnv = process.env.MEGLE_PLUGINS_DIR;
  if (fromEnv && fromEnv.length > 0) {
    return fromEnv;
  }
  const parent = path.dirname(dbPath);
  if (parent && parent !== "." && parent !== "") {
    return path.join(parent, "plugins");
  }
  return path.join(".", "plugins");
}

app
  .whenReady()
  .then(async () => {
    void ensureNotificationService().catch((error) => {
      desktopLogger.warn("main", "[megle] notification service startup degraded:", error);
    });
    const session = await ensureCoreReady();
    await maybeAutoAddRoot(session);
    await createWindow();
    emitMainStartupReady();
  })
  .catch(handleDesktopStartupFailure);

function handleDesktopStartupFailure(error: unknown): void {
  desktopLogger.error("main", "[megle] desktop startup failed:", error);
  requestDesktopShutdown(1);
}

function requestDesktopShutdown(exitCode: number): void {
  if (shutdownFinalized || shutdownPromise) return;
  const shutdown = coreLifecycleQueue.shutdown(async () => {
    const generation = coreLifecycleQueue.currentGeneration;
    const session = coreSession;
    await runRecoverableNotificationShutdown({
      quiesceNotifications: quiesceNotificationRuntime,
      stopCore: stopDesktopCoreProcesses,
      restoreNotifications: () =>
        restoreNotificationsAfterFailedCoreStop(session, generation),
      closeNotifications: closeNotificationRuntime
    });
    await desktopLogger.close();
    pendingCoreProcess = null;
    coreProcess = null;
    pendingCoreSession = null;
    coreReadyPromise = null;
    coreSession = null;
    shutdownFinalized = true;
    app.exit(exitCode);
  });
  shutdownPromise = shutdown;
  void shutdown.catch((error) => {
    desktopLogger.error("main", "[megle] Core shutdown failed; retaining process for retry:", error);
    if (shutdownPromise === shutdown) shutdownPromise = null;
  });
}

async function maybeAutoAddRoot(session: CoreSession): Promise<void> {
  const target = process.env.MEGLE_AUTO_ADD_ROOT?.trim();
  if (!target) return;
  try {
    const response = await fetch(`${session.baseUrl}/roots`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-megle-session": session.sessionToken
      },
      body: JSON.stringify({ path: target, displayName: path.basename(target) })
    });
    if (!response.ok) {
      const text = await response.text();
      desktopLogger.warn("main", `[megle] MEGLE_AUTO_ADD_ROOT addRoot failed (${response.status}): ${text}`);
    } else {
      desktopLogger.info("main", `[megle] auto-added root ${target}`);
    }
  } catch (error) {
    desktopLogger.warn("main", "[megle] MEGLE_AUTO_ADD_ROOT request failed:", error);
  }
}

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") {
    app.quit();
  }
});

app.on("activate", () => {
  if (BrowserWindow.getAllWindows().length === 0) {
    void createWindow();
  }
});

app.on("before-quit", (event) => {
  // Persist window state synchronously before tearing Core down. The
  // `persisted` guard prevents the second `before-quit` (after `app.exit`)
  // from re-entering this branch — Node would otherwise double-stop Core
  // and double-flush the JSON.
  if (!persisted) {
    event.preventDefault();
    persisted = true;
    if (persistDebounceTimer) {
      clearTimeout(persistDebounceTimer);
      persistDebounceTimer = null;
    }
    void (async () => {
      try {
        await persistWindowState();
      } catch (error) {
        desktopLogger.warn("main", "[megle] window state persistence failed during shutdown:", error);
      } finally {
        requestDesktopShutdown(0);
      }
    })();
    return;
  }

  if (shutdownFinalized) return;
  event.preventDefault();
  requestDesktopShutdown(0);
});
