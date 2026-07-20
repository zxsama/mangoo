export type NotificationOrigin = "user" | "background" | "recovery" | "system";
export type NotificationPresentation = "important" | "history_only" | "status_only";
export type NotificationOperationStatus =
  | "pending"
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled";
export type NotificationCategory = "task" | "file_operation" | "error" | "thumbnail_cache";

export interface NotificationLibraryIdentity {
  id: string;
  name: string;
}

// Desktop owns this DTO so its persistent schema does not depend on renderer or client internals.
export interface NotificationOperationInput {
  operationId: string;
  revision: number;
  kind: string;
  category: NotificationCategory;
  origin: NotificationOrigin;
  presentation: NotificationPresentation;
  status: NotificationOperationStatus;
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

export interface DesktopNotificationOperation extends NotificationOperationInput {
  libraryId: string;
  libraryName: string;
}

export interface NotificationStoreEvent {
  sequence: number;
  operation: NotificationOperationInput;
}

export interface NotificationSyncState {
  persistedSequence: number;
  ackedSequence: number;
}

export interface ThumbnailCacheSummary {
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

export interface NotificationStoredSnapshot {
  active: DesktopNotificationOperation[];
  thumbnailSummaries: ThumbnailCacheSummary[];
  latestImportantResults: DesktopNotificationOperation[];
}

export interface NotificationCompactSnapshot extends NotificationStoredSnapshot {
  syncHealth: NotificationSyncHealth;
}

export interface NotificationHistoryQuery {
  libraryId?: string;
  statuses?: NotificationOperationStatus[];
  categories?: NotificationCategory[];
  search?: string;
  limit?: number;
  offset?: number;
}

export interface NotificationHistoryPage {
  items: DesktopNotificationOperation[];
  total: number;
  nextOffset: number | null;
}

export interface NotificationSyncHealth {
  state: "healthy" | "syncing" | "degraded";
  libraryId: string | null;
  detail: string | null;
  updatedAt: number;
}

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

export interface NotificationActionResult {
  ok: boolean;
  message: string | null;
}

export interface ProcessLogEntry {
  sequence: number;
  timestamp: number;
  source: "main" | "core" | "renderer";
  level: "debug" | "info" | "warn" | "error";
  message: string;
}

export interface NotificationRetentionResult {
  deletedExpired: number;
  deletedOverflow: number;
}

export interface NotificationPendingRendererError {
  library: NotificationLibraryIdentity;
  report: RendererErrorReport;
  reportIds: string[];
  occurredAt: number;
}

export interface NotificationPendingActionError {
  libraryId: string;
  operationId: string;
  expectedRevision: number;
  detail: string | null;
  updatedAt: number;
}

export interface NotificationLocalRecoveryBatch {
  batchId: string;
  rendererErrors: NotificationPendingRendererError[];
  thumbnails: ThumbnailCacheSummary[];
  actionErrors: NotificationPendingActionError[];
  createdAt: number;
}

export type NotificationStoreRequest =
  | {
      requestId: string;
      method: "applyOutboxBatch";
      params: {
        library: NotificationLibraryIdentity;
        events: NotificationStoreEvent[];
        maxSequence: number;
      };
    }
  | {
      requestId: string;
      method: "upsertOperations";
      params: {
        library: NotificationLibraryIdentity;
        operations: NotificationOperationInput[];
      };
    }
  | { requestId: string; method: "cursor"; params: { libraryId: string } }
  | { requestId: string; method: "syncState"; params: { libraryId: string } }
  | {
      requestId: string;
      method: "markAcknowledged";
      params: { libraryId: string; sequence: number };
    }
  | { requestId: string; method: "compactSnapshot"; params: Record<string, never> }
  | { requestId: string; method: "history"; params: { query: NotificationHistoryQuery } }
  | {
      requestId: string;
      method: "getOperation";
      params: { libraryId: string; operationId: string };
    }
  | {
      requestId: string;
      method: "setOperationActionError";
      params: {
        libraryId: string;
        operationId: string;
        expectedRevision: number;
        detail: string | null;
        updatedAt: number;
      };
    }
  | {
      requestId: string;
      method: "recordRendererError";
      params: {
        library: NotificationLibraryIdentity;
        report: RendererErrorReport;
        reportId: string;
        occurredAt: number;
      };
    }
  | {
      requestId: string;
      method: "publishThumbnailSummary";
      params: { summary: ThumbnailCacheSummary };
    }
  | {
      requestId: string;
      method: "applyLocalRecoveryBatch";
      params: { batch: NotificationLocalRecoveryBatch; appliedAt: number };
    }
  | { requestId: string; method: "hasLegacyImport"; params: { libraryId: string } }
  | {
      requestId: string;
      method: "importLegacyOperations";
      params: {
        library: NotificationLibraryIdentity;
        operations: NotificationOperationInput[];
        importedAt: number;
      };
    }
  | { requestId: string; method: "cleanupRetention"; params: { now: number } }
  | { requestId: string; method: "close"; params: Record<string, never> };

export interface NotificationStoreSerializedError {
  name: string;
  message: string;
  code?: string;
}

export type NotificationStoreWorkerMessage =
  | { type: "ready" }
  | { type: "startupError"; error: NotificationStoreSerializedError }
  | { type: "response"; requestId: string; ok: true; result: unknown }
  | {
      type: "response";
      requestId: string;
      ok: false;
      error: NotificationStoreSerializedError;
    };
