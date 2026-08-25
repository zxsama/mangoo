import type {
  CreateLibraryRequest,
  LibraryLoadPhase,
  LibraryPreflightRequest,
  LibraryPreflightResponse,
  LibrarySummary,
} from "./libraryTypes";
import { createCoreClient } from "./client.ts";
import type {
  DesktopNotificationsBridge,
  MediaFocusCommand,
  MediaFocusCancellation,
  MediaFocusCompletion,
  MediaFocusRequest,
  MediaFocusStageUpdate,
  NotificationWindowIntent
} from "@megle/notification-contract";

export type {
  DesktopNotificationCategory,
  DesktopNotificationCompactSnapshot,
  DesktopNotificationHistoryFacets,
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
  DesktopThumbnailCacheSummary,
  MediaFocusRequest,
  MediaFocusCommand,
  MediaFocusCancellation,
  MediaFocusCompletion,
  MediaFocusStageUpdate,
  NotificationWindowIntent,
  ThumbnailFailureDetail
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
  openExternalAuthorizationUrl: (
    url: string
  ) => Promise<
    | { ok: true }
    | { ok: false; error: "external_browser_rejected" | "external_browser_open_failed" }
  >;
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
  openNotificationsWindow?: (intent?: NotificationWindowIntent) => Promise<boolean>;
  closeNotificationsWindow?: () => Promise<boolean>;
  onNotificationWindowIntent?: (cb: (intent: NotificationWindowIntent) => void) => () => void;
  focusMedia?: (input: MediaFocusRequest) => Promise<boolean>;
  onFocusMedia?: (cb: (input: MediaFocusCommand) => void) => () => void;
  onFocusMediaCancelled?: (cb: (input: MediaFocusCancellation) => void) => () => void;
  completeMediaFocus?: (completion: MediaFocusCompletion) => Promise<boolean>;
  updateMediaFocusStage?: (update: MediaFocusStageUpdate) => Promise<boolean>;
  notifications?: DesktopNotificationsBridge;
  windowControls?: DesktopWindowControls;
  shell?: DesktopShellActions;
  getLibraryPhase?: () => Promise<LibraryLoadPhase>;
  retryLibraryLoad?: () => Promise<{ ok: boolean; error?: string }>;
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

export async function openNotificationsWindow(intent?: NotificationWindowIntent): Promise<boolean> {
  const desktopOpen = getDesktopBridge()?.openNotificationsWindow;
  if (desktopOpen) return desktopOpen(intent);

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

export async function openExternalAuthorizationUrl(
  url: string
): Promise<
  | { ok: true }
  | { ok: false; error: "external_browser_rejected" | "external_browser_open_failed" }
  | null
> {
  return (await getDesktopShellActions()?.openExternalAuthorizationUrl(url)) ?? null;
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
  return (await createCoreClient().listLibraries()).items;
}
export async function getActiveLibraryId(): Promise<string | null> {
  return (await createCoreClient().getActiveLibrary()).library?.libraryUuid ?? null;
}
export async function switchLibrary(
  libraryId: string,
  mediaFocusRequestId?: string
): Promise<{ ok: boolean; error?: string }> {
  void mediaFocusRequestId;
  const { contextRevision } = await createCoreClient().getActiveLibrary();
  await createCoreClient().activateLibrary(libraryId, contextRevision);
  return { ok: true };
}
export async function createLibrary(
  input: CreateLibraryRequest
): Promise<{ ok: boolean; library?: LibrarySummary; error?: string }> {
  const result = await createCoreClient().createLibrary(input);
  return { ok: true, library: result.library };
}
export async function renameLibraryEntry(libraryId: string, name: string): Promise<{ ok: boolean }> {
  const { contextRevision } = await createCoreClient().getActiveLibrary();
  await createCoreClient().updateLibrary(libraryId, contextRevision, { name });
  return { ok: true };
}
export async function setLibraryIcon(libraryId: string, icon: string, color: string): Promise<{ ok: boolean }> {
  const { contextRevision } = await createCoreClient().getActiveLibrary();
  await createCoreClient().updateLibrary(libraryId, contextRevision, { icon, color });
  return { ok: true };
}
export async function deleteLibraryFile(libraryId: string): Promise<{ ok: boolean; error?: string }> {
  const { contextRevision } = await createCoreClient().getActiveLibrary();
  await createCoreClient().removeLocalLibrary(libraryId, contextRevision);
  return { ok: true };
}
export async function pickNewLibraryPath(defaultName?: string): Promise<string | null> {
  return (await getDesktopBridge()?.pickNewLibraryPath?.(defaultName)) ?? null;
}
export async function preflightLibrary(input: LibraryPreflightRequest): Promise<LibraryPreflightResponse> {
  return createCoreClient().preflightLibrary(input);
}
export async function retryLibraryLoad(): Promise<boolean> {
  try {
    return (await getDesktopBridge()?.retryLibraryLoad?.())?.ok ?? false;
  } catch {
    return false;
  }
}
export function subscribeLibraryPhase(cb: (phase: LibraryLoadPhase) => void): () => void {
  const bridge = getDesktopBridge();
  if (!bridge) return () => undefined;
  let disposed = false;
  let observedLivePhase = false;
  const unsubscribe = bridge.onLibraryPhase?.((phase) => {
    if (disposed) return;
    observedLivePhase = true;
    cb(phase);
  }) ?? (() => undefined);
  void bridge.getLibraryPhase?.()
    .then((phase) => {
      if (!disposed && !observedLivePhase) cb(phase);
    })
    .catch(() => undefined);
  return () => {
    disposed = true;
    unsubscribe();
  };
}
