import { execFile } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import type { BrowserWindow, IpcMainInvokeEvent, Rectangle } from "electron";
import { createCoreClient } from "@mangoo/core-client/node";

import { ViewerAccessPolicy } from "./accessPolicy.ts";
import { DesktopStateStore, visibleBounds } from "./desktopState.ts";
import {
  associationPlatform,
  associationRegistered,
  associationSupported,
  defaultAppsSettingsTarget,
  registerAssociations,
  unregisterAssociations,
  type AssociationEnvironment
} from "./fileAssociations.ts";
import { extractOpenPaths } from "./launchArgs.ts";
import { extensionOf, splitPath } from "../shared/formats.ts";
import {
  IMAGE_VIEWER_CAPABILITY,
  IMAGE_VIEWER_CHANNELS,
  IMAGE_VIEWER_PLUGIN_ID,
  IMAGE_VIEWER_PRIVILEGED_CHANNELS,
  IMAGE_VIEWER_ROUTE,
  IMAGE_VIEWER_SETTINGS_ROUTE,
  IMAGE_VIEWER_SETTINGS_SECTIONS,
  IMAGE_VIEWER_SETTINGS_WINDOW_ROLE,
  IMAGE_VIEWER_WINDOW_ROLE,
  MAX_COPY_IMAGE_BYTES,
  parsePathList,
  type ImageViewerActionResult,
  type ImageViewerAssociationStatus,
  type ImageViewerOpenRequest
} from "../shared/ipc.ts";
import { validateFileName } from "../shared/navigation.ts";

type ElectronModule = typeof import("electron");

export interface ImageViewerCoreSession {
  baseUrl: string;
  sessionToken: string;
}

export interface ImageViewerHostOptions {
  /** Directory for `image-viewer.json` (Electron userData). */
  userDataDir: string;
  preloadPath: string;
  windowIconPath: string | null;
  /** PNG used by the Linux desktop entry. */
  associationIconPath: string | null;
  /** Healthy Core session (starts Core when needed). */
  ensureCoreSession(): Promise<ImageViewerCoreSession>;
  /** URL of the shared web bundle for `route` under `session`. */
  rendererUrl(session: ImageViewerCoreSession, route: string): string;
  /** Host material (acrylic) application, identical to other Host windows. */
  applyWindowMaterial(window: BrowserWindow): void;
  /** Show or create the main Library window. */
  openMainWindow(): Promise<void>;
  /** Whether a non-viewer sender is a privileged Host window for `channel`. */
  authorizeHostWindow(event: IpcMainInvokeEvent, channel: string): boolean;
  /** Extra renderer switches (e.g. the visual harness flag). */
  extraRendererArguments?: () => string[];
  log(level: "info" | "warn" | "error", message: string, data?: Record<string, unknown>): void;
}

export interface ImageViewerHost {
  /** File paths in this process's own command line (cold start). */
  readonly launchPaths: string[];
  /**
   * Called once Core is healthy at startup. Opens the viewer for launch paths
   * when the plugin is enabled; returns whether it did (so the Library window
   * can stay closed, as a standalone viewer launch expects).
   */
  start(): Promise<{ openedViewer: boolean }>;
  /** A second app instance was launched (packaged single-instance lock). */
  handleSecondInstance(argv: readonly string[], workingDirectory: string): Promise<boolean>;
  /** macOS `open-file`. */
  handleOpenFile(filePath: string): void;
  /** Re-read the plugin lifecycle (after any plugin enable/disable/grant). */
  refresh(): Promise<void>;
  ownsWindow(window: BrowserWindow | null | undefined): boolean;
  isEnabled(): boolean;
  /** Called on app quit: persist window state. */
  dispose(): void;
}

const READY_FALLBACK_MS = 4000;
const PROG_ID = "Mangoo.Image";
const APP_KEY = "Mangoo";
const DISPLAY_NAME = "Mangoo 看图";

function run(command: string, args: string[]): Promise<{ code: number; stdout: string; stderr: string }> {
  return new Promise((resolve) => {
    execFile(command, args, { windowsHide: true, timeout: 30_000, encoding: "utf8" }, (error, stdout, stderr) => {
      const code = error ? (typeof (error as NodeJS.ErrnoException).code === "number" ? Number((error as NodeJS.ErrnoException).code) : 1) : 0;
      resolve({ code, stdout: String(stdout ?? ""), stderr: String(stderr ?? (error?.message ?? "")) });
    });
  });
}

export function createImageViewerHost(electron: ElectronModule, options: ImageViewerHostOptions): ImageViewerHost {
  const { app, BrowserWindow, clipboard, dialog, ipcMain, nativeImage, screen, shell } = electron;
  const store = new DesktopStateStore(path.join(options.userDataDir, "image-viewer.json"));
  const access = new ViewerAccessPolicy();
  const launchPaths = extractOpenPaths(process.argv, { defaultApp: Boolean(process.defaultApp), cwd: process.cwd() });
  const pendingMacOpenFiles: string[] = [];

  let enabled = false;
  let started = false;
  let formats: string[] = [];
  let viewerWindow: BrowserWindow | null = null;
  let settingsWindow: BrowserWindow | null = null;
  let viewerReady = false;
  let pendingRequest: ImageViewerOpenRequest | null = null;
  let nextRequestId = 1;
  let refreshQueue: Promise<void> = Promise.resolve();
  let persistTimer: ReturnType<typeof setTimeout> | null = null;

  const associationEnvironment = (): AssociationEnvironment => ({
    platform: process.platform,
    executable: process.platform === "linux" && process.env.APPIMAGE ? process.env.APPIMAGE : process.execPath,
    // An unpackaged launch is `electron <main.js>`; the OS must pass the file after it.
    launchArgs: app.isPackaged || !process.argv[1] ? [] : [path.resolve(process.argv[1])],
    appKey: APP_KEY,
    displayName: DISPLAY_NAME,
    progId: PROG_ID,
    iconPath: options.associationIconPath,
    homeDir: os.homedir(),
    tempDir: os.tmpdir(),
    run
  });

  async function coreClient() {
    const session = await options.ensureCoreSession();
    return { session, client: createCoreClient({ baseUrl: session.baseUrl, sessionToken: session.sessionToken }) };
  }

  async function readPluginEnabled(): Promise<boolean | null> {
    try {
      const { client } = await coreClient();
      const plugin = await client.getPlugin(IMAGE_VIEWER_PLUGIN_ID);
      return plugin.enabledIntent && plugin.runtimeState === "ready" && plugin.capabilities.includes(IMAGE_VIEWER_CAPABILITY);
    } catch (error) {
      const status = (error as { status?: number }).status;
      // Not installed / not trusted in this build: the feature is simply off.
      if (status === 404) return false;
      options.log("warn", "image viewer plugin state unavailable", { error: String(error) });
      return null;
    }
  }

  async function loadFormats(): Promise<void> {
    try {
      const { client } = await coreClient();
      const response = await client.getImageViewerFormats();
      formats = response.imageExtensions;
    } catch (error) {
      options.log("warn", "image viewer formats unavailable", { error: String(error) });
    }
  }

  function supportedSet(): Set<string> {
    return new Set(formats);
  }

  // ---------------------------------------------------------------- windows

  function persistWindow(): void {
    if (!viewerWindow || viewerWindow.isDestroyed() || viewerWindow.isFullScreen()) return;
    const maximized = viewerWindow.isMaximized();
    const bounds: Rectangle = maximized ? viewerWindow.getNormalBounds() : viewerWindow.getBounds();
    store.update((state) => {
      state.window = { bounds: { x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height }, maximized };
    });
  }

  function schedulePersistWindow(): void {
    if (persistTimer) clearTimeout(persistTimer);
    persistTimer = setTimeout(() => {
      persistTimer = null;
      persistWindow();
    }, 400);
  }

  function revealViewer(window: BrowserWindow): void {
    if (window.isDestroyed()) return;
    if (!window.isVisible()) {
      window.show();
      if (store.get().window?.maximized) window.maximize();
    }
    if (window.isMinimized()) window.restore();
    window.focus();
  }

  async function createViewerWindow(): Promise<BrowserWindow> {
    const session = await options.ensureCoreSession();
    const saved = store.get().window;
    const workAreas = screen.getAllDisplays().map((display) => display.workArea);
    const primary = screen.getPrimaryDisplay().workArea;
    const bounds = visibleBounds(saved?.bounds ?? null, [primary, ...workAreas.filter((area) => area !== primary)]);
    const window = new BrowserWindow({
      title: DISPLAY_NAME,
      ...bounds,
      minWidth: 520,
      minHeight: 360,
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
        additionalArguments: [
          `--mangoo-core-url=${session.baseUrl}`,
          `--mangoo-session-token=${session.sessionToken}`,
          `--mangoo-window=${IMAGE_VIEWER_WINDOW_ROLE}`,
          ...(options.extraRendererArguments?.() ?? [])
        ]
      }
    });
    viewerWindow = window;
    viewerReady = false;
    options.applyWindowMaterial(window);
    const fallback = setTimeout(() => revealViewer(window), READY_FALLBACK_MS);
    window.once("ready-to-show", () => options.applyWindowMaterial(window));
    window.on("move", schedulePersistWindow);
    window.on("resize", schedulePersistWindow);
    window.on("maximize", schedulePersistWindow);
    window.on("unmaximize", schedulePersistWindow);
    const sendFullscreen = () => {
      if (!window.isDestroyed()) window.webContents.send(IMAGE_VIEWER_CHANNELS.fullscreenChanged, window.isFullScreen());
    };
    window.on("enter-full-screen", sendFullscreen);
    window.on("leave-full-screen", sendFullscreen);
    window.on("close", () => persistWindow());
    window.on("closed", () => {
      clearTimeout(fallback);
      if (viewerWindow === window) {
        viewerWindow = null;
        viewerReady = false;
        pendingRequest = null;
        access.clear();
      }
    });
    window.webContents.on("render-process-gone", (_event, details) => {
      options.log("error", "image viewer renderer exited", { reason: details.reason });
      if (!window.isDestroyed()) window.destroy();
    });
    window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
    window.webContents.on("will-navigate", (event) => event.preventDefault());
    (window as BrowserWindow & { __mangooViewerFallback?: ReturnType<typeof setTimeout> }).__mangooViewerFallback = fallback;
    await window.loadURL(options.rendererUrl(session, IMAGE_VIEWER_ROUTE));
    return window;
  }

  async function existingFiles(paths: readonly string[]): Promise<string[]> {
    const supported = supportedSet();
    const result: string[] = [];
    for (const candidate of paths) {
      try {
        const stat = await fs.promises.stat(candidate);
        if (stat.isFile() && (supported.size === 0 || supported.has(extensionOf(candidate)))) result.push(candidate);
        else if (stat.isDirectory()) result.push(candidate);
      } catch {
        // Missing or unreadable: skipped.
      }
    }
    return result;
  }

  function admit(paths: readonly string[]): void {
    for (const candidate of paths) {
      try {
        if (fs.statSync(candidate).isDirectory()) access.admitFolder(candidate);
        else access.admitFile(candidate);
      } catch {
        access.admitFile(candidate);
      }
    }
  }

  async function openInViewer(paths: readonly string[]): Promise<boolean> {
    if (!enabled) return false;
    const files = await existingFiles(paths);
    if (files.length === 0) return false;
    admit(files);
    const request: ImageViewerOpenRequest = { id: nextRequestId++, paths: files };
    if (viewerWindow && !viewerWindow.isDestroyed()) {
      if (viewerReady) viewerWindow.webContents.send(IMAGE_VIEWER_CHANNELS.openRequest, request);
      else pendingRequest = request;
      revealViewer(viewerWindow);
      return true;
    }
    pendingRequest = request;
    try {
      await createViewerWindow();
    } catch (error) {
      options.log("error", "image viewer window failed to open", { error: String(error) });
      return false;
    }
    return true;
  }

  /**
   * The viewer's settings as its own frameless acrylic OS window: it can be
   * dragged anywhere (even off the viewer) and keeps the system blur behind it.
   */
  async function openSettingsWindow(section: string): Promise<boolean> {
    const target = (IMAGE_VIEWER_SETTINGS_SECTIONS as readonly string[]).includes(section) ? section : "display";
    if (settingsWindow && !settingsWindow.isDestroyed()) {
      settingsWindow.webContents.send(IMAGE_VIEWER_CHANNELS.settingsSection, target);
      if (settingsWindow.isMinimized()) settingsWindow.restore();
      settingsWindow.show();
      settingsWindow.focus();
      return true;
    }
    const session = await options.ensureCoreSession();
    const parent = viewerWindow && !viewerWindow.isDestroyed() ? viewerWindow : undefined;
    const width = 780;
    const height = 600;
    const anchor = parent?.getBounds();
    const window = new BrowserWindow({
      title: "看图设置",
      width,
      height,
      minWidth: 640,
      minHeight: 440,
      ...(anchor ? { x: Math.round(anchor.x + (anchor.width - width) / 2), y: Math.round(anchor.y + Math.max(24, (anchor.height - height) / 2)) } : {}),
      parent,
      modal: false,
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
        additionalArguments: [
          `--mangoo-window=${IMAGE_VIEWER_SETTINGS_WINDOW_ROLE}`,
          ...(options.extraRendererArguments?.() ?? [])
        ]
      }
    });
    settingsWindow = window;
    options.applyWindowMaterial(window);
    window.once("ready-to-show", () => {
      options.applyWindowMaterial(window);
      window.show();
    });
    window.on("closed", () => {
      if (settingsWindow === window) settingsWindow = null;
    });
    window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
    window.webContents.on("will-navigate", (event) => event.preventDefault());
    await window.loadURL(options.rendererUrl(session, `${IMAGE_VIEWER_SETTINGS_ROUTE}/${target}`));
    return true;
  }

  function closeViewer(): void {
    if (settingsWindow && !settingsWindow.isDestroyed()) settingsWindow.destroy();
    settingsWindow = null;
    if (viewerWindow && !viewerWindow.isDestroyed()) {
      persistWindow();
      viewerWindow.destroy();
    }
    viewerWindow = null;
    viewerReady = false;
    pendingRequest = null;
    access.clear();
  }

  // ------------------------------------------------------------ association

  async function associationStatus(): Promise<ImageViewerAssociationStatus> {
    const env = associationEnvironment();
    const platform = associationPlatform(process.platform);
    const developmentBlocked = !app.isPackaged && process.env.MANGOO_IMAGE_VIEWER_DEV_ASSOCIATION !== "1";
    const supported = associationSupported(process.platform) && !developmentBlocked;
    const registered = supported ? await associationRegistered(env) : false;
    let detail: string;
    if (!associationSupported(process.platform)) {
      detail = process.platform === "darwin"
        ? "macOS 的默认应用由安装包声明的文档类型决定：请在访达中对图片选择“显示简介 → 打开方式 → 全部更改”。"
        : "当前系统不支持注册默认看图应用。";
    } else if (developmentBlocked) {
      detail = "开发版不会修改系统设置，安装正式版后即可开启。";
    } else if (!enabled) {
      detail = "启用看图插件后即可设置。";
    } else if (registered) {
      detail = process.platform === "win32"
        ? "已注册到“打开方式”。可在 Windows 设置的“默认应用”中将 Mangoo 看图设为默认。"
        : "已设为默认图片查看器。";
    } else {
      detail = "尚未注册到系统。";
    }
    return {
      platform,
      supported,
      registered,
      wanted: store.get().associationWanted,
      pluginEnabled: enabled,
      canOpenSystemSettings: process.platform === "win32" && supported,
      detail,
      extensions: [...formats]
    };
  }

  async function applyAssociation(wanted: boolean): Promise<void> {
    const env = associationEnvironment();
    const supported = associationSupported(process.platform)
      && (app.isPackaged || process.env.MANGOO_IMAGE_VIEWER_DEV_ASSOCIATION === "1");
    if (!supported) return;
    const recorded = store.get().registration;
    if (wanted && enabled) {
      if (formats.length === 0) await loadFormats();
      const registration = await registerAssociations(env, formats);
      store.update((state) => {
        state.registration = registration;
      });
      options.log("info", "image viewer registered as image handler", { extensions: registration.extensions.length });
    } else if (recorded) {
      await unregisterAssociations(env, recorded);
      store.update((state) => {
        state.registration = null;
      });
      options.log("info", "image viewer image handler registration removed");
    }
  }

  // ---------------------------------------------------------- reconciliation

  async function reconcile(): Promise<void> {
    const next = await readPluginEnabled();
    if (next === null) return;
    const changed = next !== enabled;
    enabled = next;
    if (enabled) {
      await loadFormats();
      const wanted = store.get().associationWanted;
      const recorded = store.get().registration;
      const formatsChanged = recorded
        ? recorded.extensions.join(",") !== [...formats].sort().join(",") || recorded.executable !== associationEnvironment().executable
        : true;
      if (wanted && formatsChanged) await applyAssociation(true).catch((error) => options.log("warn", "image viewer association failed", { error: String(error) }));
      const queued = pendingMacOpenFiles.splice(0);
      if (queued.length && started) await openInViewer(queued);
    } else {
      // Plugin off => every viewer surface is off: window, OS registration.
      if (changed || viewerWindow) closeViewer();
      if (store.get().registration) {
        await applyAssociation(false).catch((error) => options.log("warn", "image viewer association removal failed", { error: String(error) }));
      }
    }
  }

  function refresh(): Promise<void> {
    refreshQueue = refreshQueue.then(reconcile, reconcile).catch((error) => {
      options.log("warn", "image viewer refresh failed", { error: String(error) });
    });
    return refreshQueue;
  }

  // -------------------------------------------------------------------- IPC

  function isViewerSender(event: IpcMainInvokeEvent): boolean {
    const window = BrowserWindow.fromWebContents(event.sender);
    return Boolean(window && viewerWindow && window === viewerWindow && !window.isDestroyed());
  }

  function isSettingsSender(event: IpcMainInvokeEvent): boolean {
    const window = BrowserWindow.fromWebContents(event.sender);
    return Boolean(window && settingsWindow && window === settingsWindow && !window.isDestroyed());
  }

  function handle(channel: string, scope: "viewer" | "viewer-or-host", handler: (event: IpcMainInvokeEvent, ...args: unknown[]) => unknown): void {
    ipcMain.handle(channel, async (event, ...args) => {
      const viewer = isViewerSender(event);
      // The settings window may only use the shared (association) channels.
      const settings = scope === "viewer-or-host" && isSettingsSender(event);
      if (!viewer && !settings && !(scope === "viewer-or-host" && options.authorizeHostWindow(event, channel))) {
        throw new Error(`Image viewer IPC access denied: ${channel}`);
      }
      return handler(event, ...args);
    });
  }

  function requireAdmitted(file: unknown): string {
    if (typeof file !== "string") throw new Error("image path is outside the folders opened in this viewer");
    // Reject traversal before normalising, which would otherwise fold it away.
    if (file.split(/[\\/]/).some((part) => part === ".." || part === ".")) {
      throw new Error("image path is outside the folders opened in this viewer");
    }
    const normalized = path.normalize(file);
    if (!access.allows(normalized, supportedSet())) {
      throw new Error("image path is outside the folders opened in this viewer");
    }
    return normalized;
  }

  async function guarded(action: () => Promise<void> | void, extra: Partial<ImageViewerActionResult> = {}): Promise<ImageViewerActionResult> {
    try {
      await action();
      return { ok: true, ...extra };
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error.message : String(error) };
    }
  }

  handle(IMAGE_VIEWER_CHANNELS.ready, "viewer", (event) => {
    viewerReady = true;
    const window = BrowserWindow.fromWebContents(event.sender);
    if (window) {
      const fallback = (window as BrowserWindow & { __mangooViewerFallback?: ReturnType<typeof setTimeout> }).__mangooViewerFallback;
      if (fallback) clearTimeout(fallback);
      revealViewer(window);
    }
    const request = pendingRequest;
    pendingRequest = null;
    return request;
  });

  handle(IMAGE_VIEWER_CHANNELS.pickFiles, "viewer", async (event) => {
    const window = BrowserWindow.fromWebContents(event.sender) ?? undefined;
    if (formats.length === 0) await loadFormats();
    const extensions = formats.map((ext) => ext.replace(/^\./, ""));
    const result = await dialog.showOpenDialog(window as BrowserWindow, {
      title: "打开图片",
      properties: ["openFile", "multiSelections"],
      filters: [
        { name: "图片", extensions },
        { name: "所有文件", extensions: ["*"] }
      ]
    });
    if (result.canceled || result.filePaths.length === 0) return null;
    admit(result.filePaths);
    return result.filePaths;
  });

  handle(IMAGE_VIEWER_CHANNELS.pickFolder, "viewer", async (event) => {
    const window = BrowserWindow.fromWebContents(event.sender) ?? undefined;
    const result = await dialog.showOpenDialog(window as BrowserWindow, { title: "打开文件夹", properties: ["openDirectory"] });
    if (result.canceled || !result.filePaths[0]) return null;
    access.admitFolder(result.filePaths[0]);
    return result.filePaths[0];
  });

  handle(IMAGE_VIEWER_CHANNELS.admitPaths, "viewer", async (_event, value) => {
    const paths = parsePathList(value);
    if (!paths) return [];
    const files = await existingFiles(paths.filter((candidate) => path.isAbsolute(candidate)));
    admit(files);
    return files;
  });

  handle(IMAGE_VIEWER_CHANNELS.trash, "viewer", (_event, file) => guarded(async () => {
    await shell.trashItem(requireAdmitted(file));
  }));

  handle(IMAGE_VIEWER_CHANNELS.rename, "viewer", async (_event, file, newName) => {
    try {
      const source = requireAdmitted(file);
      if (typeof newName !== "string") throw new Error("名称无效");
      const problem = validateFileName(newName);
      if (problem) throw new Error(problem);
      const trimmed = newName.trim();
      if (extensionOf(trimmed) !== extensionOf(source)) throw new Error("请保留原有的文件扩展名");
      const target = path.join(splitPath(source).directory, trimmed);
      if (target === source) return { ok: true, path: source };
      const sameFileCaseChange = target.toLowerCase() === source.toLowerCase();
      if (!sameFileCaseChange) {
        const exists = await fs.promises.stat(target).then(() => true, () => false);
        if (exists) throw new Error("同名文件已存在");
      }
      await fs.promises.rename(source, target);
      return { ok: true, path: target };
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error.message : String(error) };
    }
  });

  handle(IMAGE_VIEWER_CHANNELS.reveal, "viewer", (_event, file) => guarded(() => {
    shell.showItemInFolder(requireAdmitted(file));
  }));

  handle(IMAGE_VIEWER_CHANNELS.openExternally, "viewer", (_event, file) => guarded(async () => {
    const error = await shell.openPath(requireAdmitted(file));
    if (error) throw new Error(error);
  }));

  handle(IMAGE_VIEWER_CHANNELS.copyImage, "viewer", (_event, bytes) => guarded(() => {
    if (!(bytes instanceof Uint8Array) || bytes.byteLength === 0 || bytes.byteLength > MAX_COPY_IMAGE_BYTES) {
      throw new Error("图像数据无效");
    }
    const image = nativeImage.createFromBuffer(Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength));
    if (image.isEmpty()) throw new Error("图像数据无效");
    clipboard.writeImage(image);
  }));

  handle(IMAGE_VIEWER_CHANNELS.copyText, "viewer", (_event, text) => guarded(() => {
    if (typeof text !== "string" || text.length > 32_768) throw new Error("文本无效");
    clipboard.writeText(text);
  }));

  handle(IMAGE_VIEWER_CHANNELS.toggleFullscreen, "viewer", (event) => {
    const window = BrowserWindow.fromWebContents(event.sender);
    if (!window) return false;
    window.setFullScreen(!window.isFullScreen());
    return window.isFullScreen();
  });

  handle(IMAGE_VIEWER_CHANNELS.setAlwaysOnTop, "viewer", (event, value) => {
    const window = BrowserWindow.fromWebContents(event.sender);
    if (!window) return false;
    window.setAlwaysOnTop(value === true);
    return window.isAlwaysOnTop();
  });

  handle(IMAGE_VIEWER_CHANNELS.openMainWindow, "viewer", async () => {
    await options.openMainWindow();
    return true;
  });

  handle(IMAGE_VIEWER_CHANNELS.associationStatus, "viewer-or-host", () => associationStatus());

  handle(IMAGE_VIEWER_CHANNELS.setAssociation, "viewer-or-host", async (_event, value) => {
    const wanted = value === true;
    store.update((state) => {
      state.associationWanted = wanted;
    });
    try {
      await applyAssociation(wanted);
    } catch (error) {
      options.log("warn", "image viewer association change failed", { error: String(error) });
      const status = await associationStatus();
      return { ...status, detail: `系统关联更新失败：${error instanceof Error ? error.message : String(error)}` };
    }
    return associationStatus();
  });

  handle(IMAGE_VIEWER_CHANNELS.openDefaultAppsSettings, "viewer-or-host", async () => {
    const target = defaultAppsSettingsTarget(associationEnvironment());
    if (!target) return false;
    await shell.openExternal(target);
    return true;
  });

  handle(IMAGE_VIEWER_CHANNELS.openSettings, "viewer", (_event, section) =>
    openSettingsWindow(typeof section === "string" ? section : "display"));

  handle(IMAGE_VIEWER_CHANNELS.openFiles, "viewer-or-host", async (event, value) => {
    if (isSettingsSender(event)) return false;
    const paths = parsePathList(value);
    if (!paths) return false;
    return openInViewer(paths);
  });

  return {
    launchPaths,
    async start() {
      started = true;
      await refresh();
      const queued = [...launchPaths, ...pendingMacOpenFiles.splice(0)];
      if (!enabled || queued.length === 0) return { openedViewer: false };
      return { openedViewer: await openInViewer(queued) };
    },
    async handleSecondInstance(argv, workingDirectory) {
      const paths = extractOpenPaths(argv, { defaultApp: false, cwd: workingDirectory });
      if (paths.length === 0) return false;
      await refresh();
      return openInViewer(paths);
    },
    handleOpenFile(filePath) {
      if (!started) {
        pendingMacOpenFiles.push(filePath);
        return;
      }
      void refresh().then(() => openInViewer([filePath]));
    },
    refresh,
    ownsWindow(window) {
      return Boolean(window && ((viewerWindow && window === viewerWindow) || (settingsWindow && window === settingsWindow)));
    },
    isEnabled() {
      return enabled;
    },
    dispose() {
      if (persistTimer) clearTimeout(persistTimer);
      persistWindow();
    }
  };
}

export { IMAGE_VIEWER_PRIVILEGED_CHANNELS };
