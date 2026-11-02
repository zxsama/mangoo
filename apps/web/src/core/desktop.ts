import type {
  CreateLibraryRequest,
  LibraryLoadPhase,
  LibraryPreflightRequest,
  LibraryPreflightResponse,
  LibrarySummary,
} from "./libraryTypes";
import { createCoreClient, getCoreClientConfig } from "./client.ts";
import type { PluginExternalLinkRequest } from "@mangoo/core-client";
import type {
  LibraryCreationPaths,
  ResolveLibraryCreationPathsRequest,
  DesktopNotificationsBridge,
  DesktopFolderPickerRequest,
  MediaFocusCommand,
  MediaFocusCancellation,
  MediaFocusCompletion,
  MediaFocusRequest,
  MediaFocusStageUpdate,
  NotificationWindowIntent,
  NotificationPluginEntry,
  NotificationPluginOpenRequest,
  PluginViewWindowIntent
} from "@mangoo/notification-contract";

export type {
  DesktopNotificationCategory,
  DesktopNotificationCompactSnapshot,
  DesktopNotificationHistoryFacets,
  DesktopNotificationHistoryPage,
  DesktopNotificationHistoryQuery,
  DesktopNotificationOperation,
  DesktopNotificationsBridge,
  DesktopFolderPickerRequest,
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
} from "@mangoo/notification-contract";

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
  openPluginExternalLink: (
    request: PluginExternalLinkRequest & { pluginId: string }
  ) => Promise<
    | { ok: true }
    | { ok: false; error: "external_browser_rejected" | "external_browser_open_failed" }
  >;
  requestOAuthReturnFocus?: () => Promise<boolean>;
  copyText: (text: string) => Promise<boolean>;
}

export interface MangooDesktopBridge {
  listNotificationPluginEntries?: () => Promise<NotificationPluginEntry[]>;
  openNotificationPluginView?: (request: NotificationPluginOpenRequest) => Promise<boolean>;
  onPluginViewIntent?: (callback: (intent: PluginViewWindowIntent) => void) => () => void;
  coreUrl?: string;
  sessionToken?: string;
  windowRole?: "main" | DesktopSecondaryWindowRole;
  notifyShellReady?: () => Promise<boolean>;
  notifySecondaryWindowReady?: (
    role: DesktopSecondaryWindowRole
  ) => Promise<DesktopSecondaryWindowRevealResult | false>;
  pickFolder?: (request?: DesktopFolderPickerRequest) => Promise<string | null>;
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
  resolveLibraryCreationPaths?: (input: ResolveLibraryCreationPathsRequest) => Promise<LibraryCreationPaths>;
  pickNewLibraryPath?: (defaultName?: string) => Promise<string | null>;
  onLibraryPhase?: (cb: (phase: LibraryLoadPhase) => void) => () => void;
}

declare global {
  interface Window {
    mangooDesktop?: MangooDesktopBridge;
  }
}

export function getDesktopBridge(): MangooDesktopBridge | null {
  return window.mangooDesktop ?? null;
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
    "mangoo-plugins",
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
    "mangoo-notifications",
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

export async function pickNativeFolder(request?: DesktopFolderPickerRequest): Promise<string | null> {
  return (await getDesktopBridge()?.pickFolder?.(request)) ?? null;
}

export async function pickSyncFolder(request: DesktopFolderPickerRequest): Promise<string | null> {
  return pickNativeFolder(request);
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

export async function openPluginExternalLink(
  request: PluginExternalLinkRequest & { pluginId: string }
): Promise<
  | { ok: true }
  | { ok: false; error: "external_browser_rejected" | "external_browser_open_failed" }
  | null
> {
  return (await getDesktopShellActions()?.openPluginExternalLink(request)) ?? null;
}

export async function requestOAuthReturnFocus(): Promise<boolean> {
  return (await getDesktopShellActions()?.requestOAuthReturnFocus?.()) ?? false;
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
  return withLibraryContextChange(async () => {
    const client = createCoreClient();
    const { contextRevision } = await client.getActiveLibrary();
    await client.activateLibrary(libraryId, contextRevision);
    return { ok: true };
  });
}
export class LibraryCreatedActivationError extends Error {
  readonly library: LibrarySummary;
  constructor(library: LibrarySummary, message: string) {
    super(message);
    this.name = "LibraryCreatedActivationError";
    this.library = library;
  }
}

export async function createLibrary(
  input: CreateLibraryRequest
): Promise<{ ok: boolean; library?: LibrarySummary; error?: string }> {
  const create = async () => {
    const result = await createCoreClient().createLibrary(input);
    if (input.activate !== false && !result.activated) {
      const recovery = "资源库已创建，但未能打开。请从资源库列表重试打开。";
      throw new LibraryCreatedActivationError(result.library, result.activationError ? `${recovery} ${result.activationError}` : recovery);
    }
    return { ok: true, library: result.library };
  };
  return input.activate === false ? create() : withLibraryContextChange(create);
}
export async function renameLibraryEntry(libraryId: string, name: string): Promise<{ ok: boolean }> {
  const { contextRevision } = await createCoreClient().getActiveLibrary();
  await createCoreClient().updateLibrary(libraryId, contextRevision, { name });
  return { ok: true };
}
export async function openExistingLibrary(databasePath: string): Promise<{ ok: boolean; library: LibrarySummary }> {
  return withLibraryContextChange(async () => {
    const client = createCoreClient();
    const { contextRevision } = await client.getActiveLibrary();
    const result = await client.openLibrary({ databasePath }, contextRevision);
    if (!result.activated) {
      const recovery = "资源库已添加到列表，但未能打开。请从资源库列表重试打开。";
      throw new Error(result.activationError ? `${recovery} ${result.activationError}` : recovery);
    }
    return { ok: true, library: result.library };
  });
}
export async function forgetLibraryEntry(libraryId: string): Promise<{ ok: boolean }> {
  const client = createCoreClient();
  const { library, contextRevision } = await client.getActiveLibrary();
  const forget = async () => {
    await client.forgetLibrary(libraryId, contextRevision);
    return { ok: true };
  };
  if (library?.libraryUuid === libraryId) return withLibraryContextChange(forget);
  // Hiding another entry does not change the active catalog, selection or scroll.
  ensureLibraryContextChannel();
  try {
    return await forget();
  } finally {
    publishLibraryRegistryChange();
    libraryContextChannel?.postMessage("registry-settled");
  }
}
export async function setLibraryIcon(libraryId: string, icon: string, color: string): Promise<{ ok: boolean }> {
  const { contextRevision } = await createCoreClient().getActiveLibrary();
  await createCoreClient().updateLibrary(libraryId, contextRevision, { icon, color });
  ensureLibraryContextChannel();
  publishLibraryRegistryChange();
  libraryContextChannel?.postMessage("registry-settled");
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
const libraryPhaseListeners = new Set<(phase: LibraryLoadPhase) => void>();
const libraryRegistryListeners = new Set<() => void>();
let pendingLibraryContextChanges = 0;
let libraryMutationGeneration = 0;
let libraryContextChannel: BroadcastChannel | null = null;
let libraryContextChannelInitialized = false;

function publishLibraryPhase(phase: LibraryLoadPhase): void {
  for (const listener of [...libraryPhaseListeners]) listener(phase);
}

function publishLibraryRegistryChange(): void {
  for (const listener of [...libraryRegistryListeners]) listener();
}

export function subscribeLibraryRegistry(callback: () => void): () => void {
  ensureLibraryContextChannel();
  libraryRegistryListeners.add(callback);
  return () => { libraryRegistryListeners.delete(callback); };
}

function ensureLibraryContextChannel(): void {
  if (libraryContextChannelInitialized) return;
  libraryContextChannelInitialized = true;
  if (typeof window === "undefined" || typeof BroadcastChannel === "undefined") return;
  // Settings and the main renderer share an origin, as with preferenceSync.
  // Separate Host endpoints must never close one another's catalog admission.
  try {
    libraryContextChannel = new BroadcastChannel(`mangoo-library-context:${getCoreClientConfig().baseUrl}`);
  } catch {
    return;
  }
  libraryContextChannel.addEventListener("message", (event: MessageEvent<unknown>) => {
    if (event.data === "registry-settled") {
      publishLibraryRegistryChange();
      return;
    }
    if (event.data !== "context-settled") return;
    publishLibraryRegistryChange();
    libraryMutationGeneration += 1;
    if (pendingLibraryContextChanges > 0) return;
    // Broadcast only completion, not remote pending leases: a departed window
    // must never leave another renderer's admission permanently closed.
    publishLibraryPhase("starting-core");
    publishLibraryPhase("loading-media");
  });
  window.addEventListener("pagehide", () => {
    libraryContextChannel?.close();
    libraryContextChannel = null;
  });
}

async function withLibraryContextChange<T>(change: () => Promise<T>): Promise<T> {
  ensureLibraryContextChannel();
  libraryMutationGeneration += 1;
  pendingLibraryContextChanges += 1;
  if (pendingLibraryContextChanges === 1) {
    // The existing phase is the shared renderer teardown signal. Host Core stays
    // running; only its active Library context changes.
    publishLibraryPhase("starting-core");
  }
  try {
    return await change();
  } finally {
    pendingLibraryContextChanges -= 1;
    if (pendingLibraryContextChanges === 0) {
      // Also reconcile failures/partial activation with the authoritative active
      // context. Never infer ready or identity from the requested Library.
      publishLibraryPhase("loading-media");
      publishLibraryRegistryChange();
      libraryContextChannel?.postMessage("context-settled");
    }
  }
}

export function subscribeLibraryPhase(cb: (phase: LibraryLoadPhase) => void): () => void {
  ensureLibraryContextChannel();
  const bridge = getDesktopBridge();
  let disposed = false;
  let observedLivePhase = false;
  const onMutationPhase = (phase: LibraryLoadPhase) => {
    if (disposed) return;
    observedLivePhase = true;
    cb(phase);
  };
  libraryPhaseListeners.add(onMutationPhase);
  // A consumer mounting during a mutation must also keep admission closed.
  if (pendingLibraryContextChanges > 0) onMutationPhase("opening-library");
  const unsubscribe = bridge?.onLibraryPhase?.((phase) => {
    if (disposed) return;
    observedLivePhase = true;
    if (pendingLibraryContextChanges > 0) return;
    cb(phase);
  }) ?? (() => undefined);
  // The Desktop snapshot describes Core startup, and is no longer current once
  // renderer-owned mutations have changed its Library. Live recovery still works.
  if (libraryMutationGeneration === 0) void bridge?.getLibraryPhase?.()
    .then((phase) => {
      if (!disposed && !observedLivePhase && libraryMutationGeneration === 0) cb(phase);
    })
    .catch(() => undefined);
  return () => {
    disposed = true;
    libraryPhaseListeners.delete(onMutationPhase);
    unsubscribe();
  };
}
