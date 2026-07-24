import type {
  NotificationOperationSnapshot,
  NotificationOperationStatus,
  NotificationOrigin,
  NotificationPresentation
} from "@megle/core-client/generated-contract";

export type {
  NotificationOperationSnapshot,
  NotificationOperationStatus,
  NotificationOrigin,
  NotificationPresentation
} from "@megle/core-client/generated-contract";

export type NotificationOperationInput = NotificationOperationSnapshot;
export type DesktopNotificationStatus = NotificationOperationStatus;
export type DesktopNotificationCategory = NotificationOperationSnapshot["category"];
export type NotificationCategory = DesktopNotificationCategory;

export interface NotificationLibraryIdentity {
  id: string;
  name: string;
}

export interface DesktopNotificationOperation extends NotificationOperationSnapshot {
  libraryId: string;
  libraryName: string;
}

export interface NotificationStoreEvent {
  sequence: number;
  operation: NotificationOperationSnapshot;
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
  failureDetails: ThumbnailFailureDetail[];
  failureDetailsTruncated: boolean;
}

export interface ThumbnailFailureDetail {
  libraryId: string;
  fileId: number;
  folderId: number;
  rootId: number;
  relativePath: string;
  failureKind: string;
  error: string;
  updatedAt: number;
}

export type ThumbnailCacheSummary = DesktopThumbnailCacheSummary;

export interface NotificationSyncHealth {
  state: "healthy" | "syncing" | "degraded";
  libraryId: string | null;
  detail: string | null;
  updatedAt: number;
}

export interface NotificationCompactSnapshot {
  active: DesktopNotificationOperation[];
  thumbnailSummaries: ThumbnailCacheSummary[];
  latestImportantResults: DesktopNotificationOperation[];
  hasRetryableFailure: boolean;
  syncHealth: NotificationSyncHealth;
}

export type DesktopNotificationCompactSnapshot = NotificationCompactSnapshot;

export interface NotificationHistoryQuery {
  libraryId?: string;
  statuses?: NotificationOperationStatus[];
  categories?: NotificationCategory[];
  origins?: NotificationOrigin[];
  kinds?: string[];
  search?: string;
  limit?: number;
  offset?: number;
}

export type DesktopNotificationHistoryQuery = NotificationHistoryQuery;

export interface NotificationHistoryPage {
  items: DesktopNotificationOperation[];
  total: number;
  nextOffset: number | null;
}

export type DesktopNotificationHistoryPage = NotificationHistoryPage;

export interface NotificationHistoryFacets {
  libraries: NotificationLibraryIdentity[];
  kinds: string[];
}

export type DesktopNotificationHistoryFacets = NotificationHistoryFacets;

export interface RendererErrorReport {
  /** Stable logical occurrence id supplied by the renderer; absent for legacy callers. */
  reportId?: string;
  libraryId: string;
  libraryName: string;
  source: string;
  code: string;
  target: string;
  title: string;
  message: string;
  detail: string | null;
}

export interface RendererErrorIdentity {
  source: string;
  code: string;
  target: string;
}

export function rendererErrorIdentityKey(input: RendererErrorIdentity): string {
  return JSON.stringify([input.source, input.code, input.target]);
}

export function rendererErrorOperationId(
  libraryId: string,
  input: RendererErrorIdentity
): string {
  return `renderer-error:v2:${JSON.stringify([
    libraryId,
    input.source,
    input.code,
    input.target
  ])}`;
}

export function legacyRendererErrorOperationId(
  libraryId: string,
  input: RendererErrorIdentity
): string {
  return `renderer-error:${libraryId}:${input.source}:${input.code}:${input.target}`;
}

export type DesktopRendererErrorReport = RendererErrorReport;

export interface NotificationActionResult {
  ok: boolean;
  message: string | null;
}

export type ProcessLogSource = "main" | "core" | "renderer";
export type ProcessLogLevel = "trace" | "debug" | "info" | "warn" | "error";

export interface ProcessLogMetadata {
  readonly [key: string]: unknown;
}

export interface ProcessLogLine {
  readonly sequence: number;
  readonly timestamp: number;
  readonly source: ProcessLogSource;
  readonly level: ProcessLogLevel;
  readonly message: string;
  readonly metadata?: ProcessLogMetadata;
}

export type ProcessLogEntry = ProcessLogLine;
export type DesktopProcessLogSource = ProcessLogSource;
export type DesktopProcessLogLevel = ProcessLogLevel;
export type DesktopProcessLogLine = ProcessLogLine;

export interface ProcessLogFilter {
  readonly source?: ProcessLogSource;
  readonly sources?: readonly ProcessLogSource[];
  readonly level?: ProcessLogLevel;
  readonly levels?: readonly ProcessLogLevel[];
  readonly search?: string;
  readonly eventKind?: "thumbnail_failure";
  readonly libraryId?: string;
  readonly afterSequence?: number;
  readonly beforeSequence?: number;
  readonly limit?: number;
}

export interface NotificationWindowIntent {
  readonly page: "logs";
  readonly logFilter: {
    readonly eventKind: "thumbnail_failure";
    readonly libraryId: string;
  };
  readonly failureSnapshot: ThumbnailFailureSnapshot;
}

export interface ThumbnailFailureSnapshot {
  readonly totalCount: number;
  readonly visibleCount: number;
  readonly truncated: boolean;
}

export interface MediaFocusRequest {
  readonly libraryId: string;
  readonly fileId: number;
  readonly folderId: number;
  readonly rootId: number;
}

export interface MediaFocusCommand extends MediaFocusRequest {
  readonly requestId: string;
}

export interface MediaFocusCompletion {
  readonly requestId: string;
  readonly ok: boolean;
}

export type DesktopProcessLogFilter = ProcessLogFilter;

export interface ProcessLogPage {
  readonly lines: readonly ProcessLogLine[];
  readonly hasMore: boolean;
  readonly nextBeforeSequence: number | null;
}

export type DesktopProcessLogPage = ProcessLogPage;

export interface ProcessLogExportResult {
  readonly saved: boolean;
  readonly canceled: boolean;
  readonly filePath: string | null;
  readonly lineCount: number;
  readonly error: string | null;
}

export type DesktopProcessLogExportResult = ProcessLogExportResult;

export interface ProcessLogBatch {
  readonly lines: readonly ProcessLogLine[];
  readonly afterSequence: number;
  readonly droppedBeforeSequence: number | null;
}

export type DesktopProcessLogBatch = ProcessLogBatch;
export type ProcessLogSubscriptionId = string;

export interface ProcessLogSubscriptionStart {
  readonly subscriptionId: ProcessLogSubscriptionId;
  readonly snapshot: readonly ProcessLogLine[];
  readonly afterSequence: number;
}

export interface ProcessLogSubscriptionEvent {
  readonly subscriptionId: ProcessLogSubscriptionId;
  readonly batch: ProcessLogBatch;
}

export interface ProcessLogSubscriptionInitialSnapshot {
  readonly snapshot: readonly ProcessLogLine[];
  readonly afterSequence: number;
}

export interface ProcessLogSubscriptionHandle {
  readonly ready: Promise<ProcessLogSubscriptionInitialSnapshot>;
  unsubscribe(): void;
}

export interface RendererLogEntryInput {
  readonly level: ProcessLogLevel;
  readonly message: string;
  readonly metadata?: ProcessLogMetadata;
}

export interface NotificationIpcBridge {
  getCompactSnapshot(): Promise<NotificationCompactSnapshot>;
  getHistoryFacets(): Promise<NotificationHistoryFacets>;
  listHistory(query: NotificationHistoryQuery): Promise<NotificationHistoryPage>;
  cancel(libraryId: string, operationId: string): Promise<NotificationActionResult>;
  retry(libraryId: string, operationId: string): Promise<NotificationActionResult>;
  reportError(input: RendererErrorReport): Promise<boolean>;
  publishThumbnailSummary(input: ThumbnailCacheSummary): Promise<boolean>;
  onChanged(cb: (snapshot: NotificationCompactSnapshot) => void): () => void;
  getLogs(query: ProcessLogFilter): Promise<ProcessLogPage>;
  exportLogs(query: ProcessLogFilter): Promise<ProcessLogExportResult>;
  snapshotAndSubscribeLogs(
    query: ProcessLogFilter,
    cb: (batch: ProcessLogBatch) => void
  ): ProcessLogSubscriptionHandle;
  clearLogs(): Promise<boolean>;
  emitRendererLogs(entries: RendererLogEntryInput[]): void;
}

export type DesktopNotificationsBridge = NotificationIpcBridge;
