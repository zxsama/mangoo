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
}

export type ThumbnailCacheSummary = DesktopThumbnailCacheSummary;

export interface NotificationRetryableFailureReference {
  libraryId: string;
  operationId: string;
  revision: number;
}

export interface NotificationStoredSnapshot {
  active: DesktopNotificationOperation[];
  thumbnailSummaries: ThumbnailCacheSummary[];
  latestImportantResults: DesktopNotificationOperation[];
  retryableFailures: NotificationRetryableFailureReference[];
  hasRetryableFailure: boolean;
}

export interface NotificationSyncHealth {
  state: "healthy" | "syncing" | "degraded";
  libraryId: string | null;
  detail: string | null;
  updatedAt: number;
}

export interface NotificationCompactSnapshot extends NotificationStoredSnapshot {
  syncHealth: NotificationSyncHealth;
}

export type DesktopNotificationCompactSnapshot = NotificationCompactSnapshot;

export interface NotificationHistoryQuery {
  libraryId?: string;
  statuses?: NotificationOperationStatus[];
  categories?: NotificationCategory[];
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

export interface RendererErrorReport {
  libraryId: string;
  libraryName: string;
  source: string;
  code: string;
  target: string;
  title: string;
  message: string;
  detail: string | null;
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
  readonly afterSequence?: number;
  readonly beforeSequence?: number;
  readonly limit?: number;
}

export type DesktopProcessLogFilter = ProcessLogFilter;

export interface ProcessLogPage {
  readonly lines: readonly ProcessLogLine[];
  readonly hasMore: boolean;
  readonly nextBeforeSequence: number | null;
}

export type DesktopProcessLogPage = ProcessLogPage;

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
  listHistory(query: NotificationHistoryQuery): Promise<NotificationHistoryPage>;
  cancel(libraryId: string, operationId: string): Promise<NotificationActionResult>;
  retry(libraryId: string, operationId: string): Promise<NotificationActionResult>;
  reportError(input: RendererErrorReport): Promise<boolean>;
  publishThumbnailSummary(input: ThumbnailCacheSummary): Promise<boolean>;
  onChanged(cb: (snapshot: NotificationCompactSnapshot) => void): () => void;
  getLogs(query: ProcessLogFilter): Promise<ProcessLogPage>;
  snapshotAndSubscribeLogs(
    query: ProcessLogFilter,
    cb: (batch: ProcessLogBatch) => void
  ): ProcessLogSubscriptionHandle;
  clearLogs(): Promise<boolean>;
  emitRendererLogs(entries: RendererLogEntryInput[]): void;
}

export type DesktopNotificationsBridge = NotificationIpcBridge;
