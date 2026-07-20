import type { LibrarySummary, LibraryLoadPhase } from "./libraryTypes";

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

export type DesktopNotificationStatus =
  | "pending"
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled";
export type DesktopNotificationCategory = "task" | "file_operation" | "error" | "thumbnail_cache";

export interface DesktopNotificationOperation {
  libraryId: string;
  libraryName: string;
  operationId: string;
  revision: number;
  kind: string;
  category: DesktopNotificationCategory;
  origin: "user" | "background" | "recovery" | "system";
  presentation: "important" | "history_only" | "status_only";
  status: DesktopNotificationStatus;
  phase: string | null;
  title: string;
  summary: string | null;
  taskId: number | null;
  rootId: number | null;
  folderId: number | null;
  fileId: number | null;
  currentValue: number;
  totalValue: number | null;
  unit: string | null;
  errorCode: string | null;
  errorDetail: string | null;
  cancellable: boolean;
  retryable: boolean;
  attempt: number;
  occurrenceCount: number;
  startedAt: number;
  updatedAt: number;
  completedAt: number | null;
}

export interface DesktopThumbnailCacheSummary {
  libraryId: string;
  libraryName: string;
  observedAt: number;
  cachedCount: number;
  totalCandidateCount: number;
  missingCount: number;
  staleCount: number;
  failedCount: number;
  pendingCount: number;
  activeBulkTaskCount: number;
  totalCacheBytes: number | null;
  completionPercent: number | null;
}

export interface DesktopNotificationCompactSnapshot {
  active: DesktopNotificationOperation[];
  thumbnailSummaries: DesktopThumbnailCacheSummary[];
  latestImportantResults: DesktopNotificationOperation[];
  syncHealth: {
    state: "healthy" | "syncing" | "degraded";
    libraryId: string | null;
    detail: string | null;
    updatedAt: number;
  };
}

export interface DesktopNotificationHistoryQuery {
  libraryId?: string;
  statuses?: DesktopNotificationStatus[];
  categories?: DesktopNotificationCategory[];
  search?: string;
  limit?: number;
  offset?: number;
}

export interface DesktopNotificationHistoryPage {
  items: DesktopNotificationOperation[];
  total: number;
  nextOffset: number | null;
}

export interface DesktopRendererErrorReport {
  libraryId: string;
  libraryName: string;
  source: string;
  code: string;
  target: string;
  title: string;
  message: string;
  detail: string | null;
}

export type DesktopProcessLogSource = "main" | "core" | "renderer";
export type DesktopProcessLogLevel = "trace" | "debug" | "info" | "warn" | "error";

export interface DesktopProcessLogLine {
  sequence: number;
  timestamp: number;
  source: DesktopProcessLogSource;
  level: DesktopProcessLogLevel;
  message: string;
  metadata?: Readonly<Record<string, unknown>>;
}

export interface DesktopProcessLogFilter {
  source?: DesktopProcessLogSource;
  sources?: DesktopProcessLogSource[];
  level?: DesktopProcessLogLevel;
  levels?: DesktopProcessLogLevel[];
  search?: string;
  afterSequence?: number;
  beforeSequence?: number;
  limit?: number;
}

export interface DesktopProcessLogPage {
  lines: DesktopProcessLogLine[];
  hasMore: boolean;
  nextBeforeSequence: number | null;
}

export interface DesktopProcessLogBatch {
  lines: DesktopProcessLogLine[];
  afterSequence: number;
  droppedBeforeSequence: number | null;
}

export interface DesktopNotificationsBridge {
  getCompactSnapshot: () => Promise<DesktopNotificationCompactSnapshot>;
  listHistory: (query: DesktopNotificationHistoryQuery) => Promise<DesktopNotificationHistoryPage>;
  cancel: (libraryId: string, operationId: string) => Promise<{ ok: boolean; message: string | null }>;
  retry: (libraryId: string, operationId: string) => Promise<{ ok: boolean; message: string | null }>;
  reportError: (input: DesktopRendererErrorReport) => Promise<boolean>;
  publishThumbnailSummary: (input: DesktopThumbnailCacheSummary) => Promise<boolean>;
  onChanged: (cb: (snapshot: DesktopNotificationCompactSnapshot) => void) => () => void;
  getLogs: (query: DesktopProcessLogFilter) => Promise<DesktopProcessLogPage>;
  onLogs: (cb: (batch: DesktopProcessLogBatch) => void) => () => void;
  snapshotAndSubscribeLogs: (
    query: DesktopProcessLogFilter,
    cb: (batch: DesktopProcessLogBatch) => void
  ) => Promise<{
    snapshot: DesktopProcessLogLine[];
    afterSequence: number;
    unsubscribe: () => void;
  }>;
  clearLogs: () => Promise<boolean>;
  emitRendererLogs: (
    entries: Array<{
      level: DesktopProcessLogLevel;
      message: string;
      metadata?: Readonly<Record<string, unknown>>;
    }>
  ) => void;
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
