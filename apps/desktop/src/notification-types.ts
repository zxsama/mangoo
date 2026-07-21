import type {
  DesktopNotificationOperation,
  NotificationActionResult,
  NotificationCategory,
  NotificationCompactSnapshot,
  NotificationHistoryPage,
  NotificationHistoryFacets,
  NotificationHistoryQuery,
  NotificationLibraryIdentity,
  NotificationOperationInput,
  NotificationOperationStatus,
  NotificationStoreEvent,
  NotificationSyncHealth,
  ProcessLogEntry,
  RendererErrorReport,
  ThumbnailCacheSummary
} from "@megle/notification-contract";

export type {
  DesktopNotificationOperation,
  NotificationActionResult,
  NotificationCategory,
  NotificationCompactSnapshot,
  NotificationHistoryPage,
  NotificationHistoryFacets,
  NotificationHistoryQuery,
  NotificationLibraryIdentity,
  NotificationOperationInput,
  NotificationOperationStatus,
  NotificationStoreEvent,
  NotificationSyncHealth,
  ProcessLogEntry,
  RendererErrorReport,
  ThumbnailCacheSummary
} from "@megle/notification-contract";

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
}

export interface NotificationSyncState {
  persistedSequence: number;
  ackedSequence: number;
}

export type NotificationRendererErrorRecordResult =
  | { outcome: "applied"; operation: DesktopNotificationOperation }
  | { outcome: "duplicate"; operation: DesktopNotificationOperation | null };

export interface NotificationRetentionResult {
  deletedExpired: number;
  deletedOverflow: number;
}

export interface NotificationPendingRendererError {
  library: NotificationLibraryIdentity;
  report: RendererErrorReport;
  occurrenceCount: number;
  uncertainReportId: string | null;
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
  | { requestId: string; method: "historyFacets"; params: Record<string, never> }
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
