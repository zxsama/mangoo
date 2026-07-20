import {
  app,
  BrowserWindow,
  clipboard,
  dialog,
  ipcMain,
  screen,
  shell,
  type IpcMainInvokeEvent,
  type OpenDialogOptions
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
import { desktopLogger } from "./desktop-logger.js";
import * as libraryRegistry from "./library-registry.js";
import { sessionForDbPath } from "./core-session.js";
import {
  NATIVE_WINDOW_WARMUP_MS,
  createSecondaryWindowRevealState,
  revealSecondaryNativeWindow,
  waitForSecondaryNativeWindowWarmup,
  type SecondaryWindowRevealState
} from "./secondary-window-reveal.js";

let mainWindow: BrowserWindow | null = null;
type SecondaryWindowRole = "settings" | "plugins";
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
  plugins: null
};
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
  plugins: null
};
const secondaryWindowPositions: Record<SecondaryWindowRole, SecondaryWindowPosition | null> = {
  settings: null,
  plugins: null
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
  }
};
let coreProcess: CoreProcessHandle | null = null;
let pendingCoreProcess: CoreProcessHandle | null = null;
let coreSession: CoreSession | null = null;
let coreReadyPromise: Promise<CoreSession> | null = null;
let cachedRegistry: libraryRegistry.LibraryRegistry | null = null;
let cachedFfmpegAvailable: boolean | null = null;
let persisted = false;
let shutdownFinalized = false;
let shutdownPromise: Promise<void> | null = null;
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

function windowForIpcEvent(event: IpcMainInvokeEvent): BrowserWindow | null {
  const window = BrowserWindow.fromWebContents(event.sender);
  return window && !window.isDestroyed() ? window : null;
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

ipcMain.handle("megle:pick-folder", async () => {
  const options: OpenDialogOptions = { properties: ["openDirectory"] };
  const result = mainWindow
    ? await dialog.showOpenDialog(mainWindow, options)
    : await dialog.showOpenDialog(options);

  return result.canceled ? null : result.filePaths[0] ?? null;
});

ipcMain.handle("megle:diagnostics", async () => {
  const session = coreSession;
  return {
    ffmpegAvailable: await detectFfmpeg(),
    dbPath: session?.dbPath ?? null,
    pluginsDir: session ? resolvePluginsDir(session.dbPath) : null
  };
});

ipcMain.handle("megle:window-minimize", (event) => {
  windowForIpcEvent(event)?.minimize();
});

ipcMain.handle("megle:window-maximize", (event) => {
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
  windowForIpcEvent(event)?.close();
});

ipcMain.handle("megle:window-is-maximized", (event) => {
  return windowForIpcEvent(event)?.isMaximized() ?? false;
});

ipcMain.handle("megle:window-get-bounds", (event) => {
  const bounds = windowForIpcEvent(event)?.getBounds();
  if (!bounds) {
    return null;
  }
  return bounds;
});

ipcMain.handle("megle:window-set-position", (event, x: number, y: number) => {
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
  const window = windowForIpcEvent(event);
  if (!window || activeWindowDrag?.windowId !== window.id) return false;
  activeWindowDrag = null;
  return true;
});

ipcMain.handle("megle:shell-reveal-path", (_event, targetPath: string) => {
  if (typeof targetPath !== "string" || targetPath.trim().length === 0) {
    return false;
  }
  shell.showItemInFolder(targetPath);
  return true;
});

ipcMain.handle("megle:shell-open-path", async (_event, targetPath: string) => {
  if (typeof targetPath !== "string" || targetPath.trim().length === 0) {
    return false;
  }
  const error = await shell.openPath(targetPath);
  return error.length === 0;
});

ipcMain.handle("megle:clipboard-write-text", (_event, text: string) => {
  if (typeof text !== "string") {
    return false;
  }
  clipboard.writeText(text);
  return true;
});

ipcMain.handle("megle:shell-ready", async () => {
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

ipcMain.handle("megle:open-settings-window", async () => {
  await openSecondaryWindow("settings");
  return true;
});

ipcMain.handle("megle:close-settings-window", () => {
  closeSecondaryWindow("settings");
  return true;
});

ipcMain.handle("megle:open-plugins-window", async () => {
  await openSecondaryWindow("plugins");
  return true;
});

ipcMain.handle("megle:close-plugins-window", () => {
  closeSecondaryWindow("plugins");
  return true;
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

ipcMain.handle("megle:list-libraries", () => toSummaries(getRegistry()));
ipcMain.handle("megle:get-active-library-id", () => getRegistry().activeLibraryId);

ipcMain.handle("megle:switch-library", async (_e, libraryId: string) => {
  const reg = getRegistry();
  const target = reg.libraries.find((l) => l.id === libraryId);
  if (!target) return { ok: false, error: "library not found" };
  if (!fsSync.existsSync(target.dbPath)) return { ok: false, error: "library file missing" };
  try {
    await restartCoreForLibrary(target.dbPath);
    persistRegistry(libraryRegistry.setActive(reg, libraryId, Date.now()));
    pushLibraryPhase("loading-media"); // renderer takes over from here
    return { ok: true };
  } catch (error) {
    pushLibraryPhase({ kind: "error", message: String(error) });
    return { ok: false, error: String(error) };
  }
});

ipcMain.handle("megle:create-library", async (_e, input: { name: string; dbPath: string }) => {
  const reg = getRegistry();
  try {
    // A brand-new empty file: Core builds the schema on first open. Never
    // silently degrade "create" into "open" of an existing file — that would
    // discard the typed name via addLibrary's dbPath dedupe.
    if (fsSync.existsSync(input.dbPath)) {
      return { ok: false, error: 'A file already exists at that path; use "Open other library" instead.' };
    }
    fsSync.writeFileSync(input.dbPath, ""); // touch; Core opens + migrates
    const added = libraryRegistry.addLibrary(reg, { name: input.name, dbPath: input.dbPath, now: Date.now() });
    await restartCoreForLibrary(added.entry.dbPath);
    persistRegistry(libraryRegistry.setActive(added.registry, added.entry.id, Date.now()));
    pushLibraryPhase("loading-media");
    const summary = toSummaries(getRegistry()).find((s) => s.id === added.entry.id);
    return { ok: true, library: summary };
  } catch (error) {
    pushLibraryPhase({ kind: "error", message: String(error) });
    return { ok: false, error: String(error) };
  }
});

ipcMain.handle("megle:open-library", async (_e, dbPath: string) => {
  const reg = getRegistry();
  if (!fsSync.existsSync(dbPath)) return { ok: false, error: "file not found" };
  try {
    const name = path.basename(dbPath).replace(/\.sqlite$/i, "");
    const added = libraryRegistry.addLibrary(reg, { name, dbPath, now: Date.now() });
    await restartCoreForLibrary(added.entry.dbPath);
    persistRegistry(libraryRegistry.setActive(added.registry, added.entry.id, Date.now()));
    pushLibraryPhase("loading-media");
    const summary = toSummaries(getRegistry()).find((s) => s.id === added.entry.id);
    return { ok: true, library: summary };
  } catch (error) {
    pushLibraryPhase({ kind: "error", message: String(error) });
    return { ok: false, error: String(error) };
  }
});

ipcMain.handle("megle:remove-library-from-list", (_e, libraryId: string) => {
  persistRegistry(libraryRegistry.removeLibrary(getRegistry(), libraryId));
  return { ok: true };
});

ipcMain.handle("megle:rename-library", (_e, libraryId: string, name: string) => {
  persistRegistry(libraryRegistry.renameLibrary(getRegistry(), libraryId, name));
  return { ok: true };
});

ipcMain.handle("megle:set-library-icon", (_e, libraryId: string, icon: string, color: string) => {
  persistRegistry(libraryRegistry.setLibraryIcon(getRegistry(), libraryId, icon, color));
  return { ok: true };
});

// Destructive: delete the library's .sqlite (plus its WAL/SHM sidecars) from
// disk AND drop it from the registry. Forbidden on the active library — Core
// holds that file open (Windows would fail the unlink anyway), and the UI greys
// the action. The renderer performs its own second confirmation before calling.
ipcMain.handle("megle:delete-library-file", (_e, libraryId: string) => {
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

ipcMain.handle("megle:pick-library-file", async () => {
  const result = mainWindow
    ? await dialog.showOpenDialog(mainWindow, { properties: ["openFile"], filters: [{ name: "Megle Library", extensions: ["sqlite"] }] })
    : await dialog.showOpenDialog({ properties: ["openFile"], filters: [{ name: "Megle Library", extensions: ["sqlite"] }] });
  return result.canceled ? null : (result.filePaths[0] ?? null);
});

ipcMain.handle("megle:pick-new-library-path", async (_e, defaultName?: string) => {
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
        ...(isVisualHarnessEnabled() ? ["--megle-visual-harness=1"] : [])
      ]
    }
  });
  const window = mainWindow;
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

// Settings and Plugins use the same controlled secondary-window lifecycle:
// non-modal child windows, shared preload/Core session, frameless acrylic, and
// one live instance per role. Re-opening restores and focuses the existing one.
async function openSecondaryWindow(role: SecondaryWindowRole): Promise<void> {
  const existing = secondaryWindows[role];
  if (existing && !existing.isDestroyed()) {
    if (existing.isMinimized()) existing.restore();
    const readiness = secondaryWindowReadiness[role];
    if (
      readiness?.windowId === existing.id &&
      !readiness.revealState.settled
    ) {
      void revealSecondaryWindowWhenReady(role, existing);
      return;
    }
    existing.show();
    existing.focus();
    return;
  }

  const config = SECONDARY_WINDOW_CONFIGS[role];
  const session = await ensureCoreReady();
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
      forceSecondaryWindowReveal(role, window);
    }
  );
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
    if (activeWindowDrag?.windowId === window.id) {
      activeWindowDrag = null;
    }
    if (secondaryWindows[role] === window) {
      secondaryWindows[role] = null;
    }
    clearSecondaryWindowReadyFallback(role, window.id);
    const readiness = secondaryWindowReadiness[role];
    if (readiness?.windowId === window.id) {
      readiness.revealState.settle(false);
    }
    if (secondaryWindowReadiness[role]?.windowId === window.id) {
      secondaryWindowReadiness[role] = null;
    }
  });

  const devServer = process.env.MEGLE_WEB_URL ?? "http://127.0.0.1:5173";
  try {
    await window.loadURL(`${devServer}${config.route}`);
  } catch (error) {
    desktopLogger.warn("main", `[megle] ${role} window loadURL rejected:`, error);
    forceSecondaryWindowReveal(role, window);
  }
}

function isSecondaryWindowRole(value: unknown): value is SecondaryWindowRole {
  return value === "settings" || value === "plugins";
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
  if (coreSession) {
    return coreSession;
  }
  if (coreReadyPromise) {
    return coreReadyPromise;
  }

  coreReadyPromise = startCoreSession();
  return coreReadyPromise;
}

async function startCoreSession(): Promise<CoreSession> {
  const maxAttempts = shouldRetryAutoCoreStartup() ? AUTO_CORE_STARTUP_MAX_ATTEMPTS : 1;

  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    let pendingProcess: CoreProcessHandle | null = null;

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
      const session = await createColdStartSession();
      pendingProcess = startCoreProcess(session, (source, level, message) => {
        desktopLogger.log(source, level, message);
      });
      pendingCoreProcess = pendingProcess;
      await waitForCoreHealth(session.baseUrl, session.sessionToken);
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
      coreProcess = null;
      coreSession = null;

      // Do not start another child while an unhealthy pending child may still
      // own the bind port. The next library switch or shutdown retries it.
      if (cleanupFailed) {
        coreReadyPromise = null;
        throw error;
      }

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

async function restartCoreForLibrary(dbPath: string): Promise<void> {
  pushLibraryPhase("starting-core");
  const base = coreSession ?? (await createCoreSession());
  // Stop a failed pending Core before the active Core and wait for the port to free up.
  await stopDesktopCoreProcesses();
  coreSession = null;
  await waitForPortFree(base.bindAddress);

  const session = sessionForDbPath(base, dbPath);
  const handle = startCoreProcess(session, (source, level, message) => {
    desktopLogger.log(source, level, message);
  });
  pendingCoreProcess = handle;
  try {
    pushLibraryPhase("opening-library");
    await waitForCoreHealth(session.baseUrl, session.sessionToken);
  } catch (error) {
    try {
      await stopPendingCoreProcess();
    } catch (stopError) {
      desktopLogger.warn("main", "[megle] pending Core cleanup failed after library switch; retaining process:", stopError);
    }
    throw error;
  }
  coreProcess = handle;
  pendingCoreProcess = null;
  coreSession = session;
}

// Poll until the host:port can be bound again (Core's listener has closed),
// guarding the known probe-and-close race. Bounded; throws on timeout.
async function waitForPortFree(bindAddress: string): Promise<void> {
  const [host, portStr] = bindAddress.split(":");
  const port = Number(portStr);
  const deadline = Date.now() + 5000;
  // eslint-disable-next-line no-constant-condition
  while (true) {
    const free = await new Promise<boolean>((resolve) => {
      const probe = net.createServer();
      probe.once("error", () => resolve(false));
      probe.listen(port, host, () => probe.close(() => resolve(true)));
    });
    if (free) return;
    if (Date.now() > deadline) throw new Error(`Port ${bindAddress} did not free up`);
    await new Promise((r) => setTimeout(r, 80));
  }
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
  for (const role of ["settings", "plugins"] as const) {
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
    const session = await ensureCoreReady();
    await maybeAutoAddRoot(session);
    await createWindow();
  })
  .catch(handleDesktopStartupFailure);

function handleDesktopStartupFailure(error: unknown): void {
  desktopLogger.error("main", "[megle] desktop startup failed:", error);
  requestDesktopShutdown(1);
}

function requestDesktopShutdown(exitCode: number): void {
  if (shutdownFinalized || shutdownPromise) return;
  let run!: Promise<void>;
  run = (async () => {
    try {
      await stopDesktopCoreProcesses();
      await desktopLogger.close();
      pendingCoreProcess = null;
      coreProcess = null;
      coreReadyPromise = null;
      coreSession = null;
      shutdownFinalized = true;
      app.exit(exitCode);
    } catch (error) {
      desktopLogger.error("main", "[megle] Core shutdown failed; retaining process for retry:", error);
    } finally {
      if (!shutdownFinalized && shutdownPromise === run) shutdownPromise = null;
    }
  })();
  shutdownPromise = run;
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
