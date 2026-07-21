import type { LibrarySummary, LibraryLoadPhase } from "./libraryTypes";
import type { DesktopNotificationsBridge } from "@megle/notification-contract";

export type {
  DesktopNotificationCategory,
  DesktopNotificationCompactSnapshot,
  DesktopNotificationHistoryPage,
  DesktopNotificationHistoryQuery,
  DesktopNotificationOperation,
  DesktopNotificationsBridge,
  DesktopNotificationStatus,
  DesktopProcessLogBatch,
  DesktopProcessLogExportResult,
  DesktopProcessLogFilter,
  DesktopProcessLogLevel,
  DesktopProcessLogLine,
  DesktopProcessLogPage,
  DesktopProcessLogSource,
  DesktopRendererErrorReport,
  DesktopThumbnailCacheSummary
} from "@megle/notification-contract";

export type DesktopSecondaryWindowRole = "settings" | "plugins" | "notifications";
export interface DesktopSecondaryWindowRevealResult {
  nativeWarmupStartedAt: number;
  nativeWindowRevealedAt: number;
  fallback: boolean;
}

export interface DesktopDiagnostics {
  ffmpegAvailable: boolean;
  dbPath: string | null;
  pluginsDir: string | null;
}

export interface DesktopWindowControls {
  minimize: () => Promise<void>;
  maximize: () => Promise<boolean>;
  close: () => Promise<void>;
  isMaximized: () => Promise<boolean>;
  beginDrag: (payload: {
    clientX: number;
    screenX: number;
    screenY: number;
    titlebarOffsetY: number;
    viewportWidth: number;
  }) => Promise<{
    x: number;
    y: number;
    width: number;
    height: number;
    restoredFromMaximized?: boolean;
  } | null>;
  moveDrag: () => Promise<boolean>;
  endDrag: () => Promise<boolean>;
  getBounds: () => Promise<{ x: number; y: number; width: number; height: number } | null>;
  setPosition: (x: number, y: number) => Promise<boolean>;
}

export interface DesktopShellActions {
  revealPath: (path: string) => Promise<boolean>;
  openPath: (path: string) => Promise<boolean>;
  copyText: (text: string) => Promise<boolean>;
}

export interface MegleDesktopBridge {
  coreUrl?: string;
  sessionToken?: string;
  windowRole?: "main" | DesktopSecondaryWindowRole;
  notifyShellReady?: () => Promise<boolean>;
  notifySecondaryWindowReady?: (
    role: DesktopSecondaryWindowRole
  ) => Promise<DesktopSecondaryWindowRevealResult | false>;
  pickFolder?: () => Promise<string | null>;
  diagnostics?: () => Promise<DesktopDiagnostics>;
  openSettingsWindow?: () => Promise<boolean>;
  closeSettingsWindow?: () => Promise<boolean>;
  openPluginsWindow?: () => Promise<boolean>;
  closePluginsWindow?: () => Promise<boolean>;
  openNotificationsWindow?: () => Promise<boolean>;
  closeNotificationsWindow?: () => Promise<boolean>;
  notifications?: DesktopNotificationsBridge;
  windowControls?: DesktopWindowControls;
  shell?: DesktopShellActions;
  listLibraries?: () => Promise<LibrarySummary[]>;
  getActiveLibraryId?: () => Promise<string | null>;
  switchLibrary?: (libraryId: string) => Promise<{ ok: boolean; error?: string }>;
  createLibrary?: (input: { name: string; dbPath: string }) => Promise<{ ok: boolean; library?: LibrarySummary; error?: string }>;
  openLibrary?: (dbPath: string) => Promise<{ ok: boolean; library?: LibrarySummary; error?: string }>;
  removeLibraryFromList?: (libraryId: string) => Promise<{ ok: boolean }>;
  renameLibrary?: (libraryId: string, name: string) => Promise<{ ok: boolean }>;
  setLibraryIcon?: (libraryId: string, icon: string, color: string) => Promise<{ ok: boolean }>;
  deleteLibraryFile?: (libraryId: string) => Promise<{ ok: boolean; error?: string }>;
  pickLibraryFile?: () => Promise<string | null>;
  pickNewLibraryPath?: (defaultName?: string) => Promise<string | null>;
  onLibraryPhase?: (cb: (phase: LibraryLoadPhase) => void) => () => void;
}

declare global {
  interface Window {
    megleDesktop?: MegleDesktopBridge;
  }
}

export function getDesktopBridge(): MegleDesktopBridge | null {
  return window.megleDesktop ?? null;
}

const secondaryWindowRevealResults = new Map<
  DesktopSecondaryWindowRole,
  DesktopSecondaryWindowRevealResult
>();

export async function notifySecondaryWindowReady(
  role: DesktopSecondaryWindowRole
): Promise<DesktopSecondaryWindowRevealResult | false> {
  const bridge = getDesktopBridge();
  const notifyReady = bridge?.notifySecondaryWindowReady;
  if (!notifyReady || bridge.windowRole !== role) return false;
  const previousResult = secondaryWindowRevealResults.get(role);
  if (previousResult) return previousResult;
  try {
    const result = await notifyReady(role);
    if (result) secondaryWindowRevealResults.set(role, result);
    return result;
  } catch {
    return false;
  }
}

export async function openPluginsWindow(): Promise<boolean> {
  const desktopOpen = getDesktopBridge()?.openPluginsWindow;
  if (desktopOpen) {
    return desktopOpen();
  }

  const pluginsUrl = new URL(window.location.href);
  pluginsUrl.hash = "/plugins";
  const opened = window.open(
    pluginsUrl,
    "megle-plugins",
    "popup=yes,width=1040,height=720"
  );
  opened?.focus();
  return opened !== null;
}

export async function openNotificationsWindow(): Promise<boolean> {
  const desktopOpen = getDesktopBridge()?.openNotificationsWindow;
  if (desktopOpen) return desktopOpen();

  const notificationsUrl = new URL(window.location.href);
  notificationsUrl.hash = "/notifications";
  const opened = window.open(
    notificationsUrl,
    "megle-notifications",
    "popup=yes,width=1040,height=720"
  );
  opened?.focus();
  return opened !== null;
}

let desktopShellReadyNotified = false;

export async function notifyDesktopShellReady(): Promise<boolean> {
  const notifyShellReady = getDesktopBridge()?.notifyShellReady;
  if (!notifyShellReady) {
    return false;
  }
  if (desktopShellReadyNotified) {
    return true;
  }
  desktopShellReadyNotified = true;
  try {
    const didNotify = await notifyShellReady();
    if (!didNotify) {
      desktopShellReadyNotified = false;
    }
    return didNotify;
  } catch {
    desktopShellReadyNotified = false;
    return false;
  }
}

export function canPickNativeFolder(): boolean {
  return typeof getDesktopBridge()?.pickFolder === "function";
}

export async function pickNativeFolder(): Promise<string | null> {
  return (await getDesktopBridge()?.pickFolder?.()) ?? null;
}

export function getWindowControls(): DesktopWindowControls | null {
  return getDesktopBridge()?.windowControls ?? null;
}

export function getDesktopShellActions(): DesktopShellActions | null {
  return getDesktopBridge()?.shell ?? null;
}

export async function revealPath(path: string): Promise<boolean> {
  return (await getDesktopShellActions()?.revealPath(path)) ?? false;
}

export async function openPath(path: string): Promise<boolean> {
  return (await getDesktopShellActions()?.openPath(path)) ?? false;
}

export async function copyText(text: string): Promise<boolean> {
  const desktop = getDesktopShellActions();
  if (desktop) {
    return desktop.copyText(text);
  }
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export async function readDesktopDiagnostics(): Promise<DesktopDiagnostics | null> {
  const bridge = getDesktopBridge();
  if (!bridge?.diagnostics) {
    return null;
  }
  try {
    return await bridge.diagnostics();
  } catch {
    return null;
  }
}

export async function listLibraries(): Promise<LibrarySummary[]> {
  return (await getDesktopBridge()?.listLibraries?.()) ?? [];
}
export async function getActiveLibraryId(): Promise<string | null> {
  return (await getDesktopBridge()?.getActiveLibraryId?.()) ?? null;
}
export async function switchLibrary(libraryId: string): Promise<{ ok: boolean; error?: string }> {
  return (await getDesktopBridge()?.switchLibrary?.(libraryId)) ?? { ok: false, error: "no desktop bridge" };
}
export async function createLibrary(
  input: { name: string; dbPath: string }
): Promise<{ ok: boolean; library?: LibrarySummary; error?: string }> {
  return (await getDesktopBridge()?.createLibrary?.(input)) ?? { ok: false, error: "no desktop bridge" };
}
export async function openLibrary(
  dbPath: string
): Promise<{ ok: boolean; library?: LibrarySummary; error?: string }> {
  return (await getDesktopBridge()?.openLibrary?.(dbPath)) ?? { ok: false, error: "no desktop bridge" };
}
export async function removeLibraryFromList(libraryId: string): Promise<{ ok: boolean }> {
  return (await getDesktopBridge()?.removeLibraryFromList?.(libraryId)) ?? { ok: false };
}
export async function renameLibraryEntry(libraryId: string, name: string): Promise<{ ok: boolean }> {
  return (await getDesktopBridge()?.renameLibrary?.(libraryId, name)) ?? { ok: false };
}
export async function setLibraryIcon(libraryId: string, icon: string, color: string): Promise<{ ok: boolean }> {
  return (await getDesktopBridge()?.setLibraryIcon?.(libraryId, icon, color)) ?? { ok: false };
}
export async function deleteLibraryFile(libraryId: string): Promise<{ ok: boolean; error?: string }> {
  return (await getDesktopBridge()?.deleteLibraryFile?.(libraryId)) ?? { ok: false, error: "no desktop bridge" };
}
export async function pickLibraryFile(): Promise<string | null> {
  return (await getDesktopBridge()?.pickLibraryFile?.()) ?? null;
}
export async function pickNewLibraryPath(defaultName?: string): Promise<string | null> {
  return (await getDesktopBridge()?.pickNewLibraryPath?.(defaultName)) ?? null;
}
export function subscribeLibraryPhase(cb: (phase: LibraryLoadPhase) => void): () => void {
  return getDesktopBridge()?.onLibraryPhase?.(cb) ?? (() => undefined);
}
