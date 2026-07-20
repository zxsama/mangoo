import { randomUUID } from "node:crypto";

import type {
  FileOperationKind,
  FileOperationRecord,
  NotificationOutboxAckRequest,
  NotificationOutboxAckResponse
} from "@megle/core-client/generated-contract";

import type { CoreSession } from "./core-session.js";
import type {
  DesktopNotificationOperation,
  NotificationActionResult,
  NotificationCompactSnapshot,
  NotificationHistoryPage,
  NotificationHistoryQuery,
  NotificationLibraryIdentity,
  NotificationLocalRecoveryBatch,
  NotificationOperationInput,
  NotificationRendererErrorRecordResult,
  NotificationRetentionResult,
  NotificationRetryableFailureReference,
  NotificationStoreEvent,
  NotificationStoredSnapshot,
  NotificationSyncHealth,
  NotificationSyncState,
  RendererErrorReport,
  ThumbnailCacheSummary
} from "./notification-types.js";

const OUTBOX_LIMIT = 200;
const OUTBOX_WAIT_MS = 30_000;
const LEGACY_PAGE_LIMIT = 200;
const LEGACY_LOGICAL_LIMIT = 2_000;
const LEGACY_RETENTION_MS = 30 * 24 * 60 * 60 * 1_000;
const RETENTION_CADENCE_MS = 5 * 60 * 1_000;
const FALLBACK_RECORD_LIMIT = 500;
const MIN_RETRY_MS = 250;
const MAX_RETRY_MS = 5_000;
const OPERATION_CATEGORIES = new Set(["task", "file_operation", "error", "thumbnail_cache"]);
const OPERATION_ORIGINS = new Set(["user", "background", "recovery", "system"]);
const OPERATION_PRESENTATIONS = new Set(["important", "history_only", "status_only"]);
const OPERATION_STATUSES = new Set(["pending", "running", "succeeded", "failed", "cancelled"]);
const LEGACY_OPERATION_KINDS = [
  "rename",
  "move",
  "delete_recycle",
  "delete_permanent"
] as const satisfies readonly FileOperationKind[];
const LEGACY_OPERATION_LABELS: Record<FileOperationKind, string> = {
  rename: "Rename",
  move: "Move",
  delete_recycle: "Move to recycle bin",
  delete_permanent: "Delete permanently"
};

export type NotificationCoreSession = Pick<CoreSession, "baseUrl" | "sessionToken">;

export interface NotificationServiceStore {
  applyOutboxBatch(
    library: NotificationLibraryIdentity,
    events: NotificationStoreEvent[],
    maxSequence: number
  ): Promise<void>;
  upsertOperations(
    library: NotificationLibraryIdentity,
    operations: NotificationOperationInput[]
  ): Promise<void>;
  cursor(libraryId: string): Promise<number>;
  syncState(libraryId: string): Promise<NotificationSyncState>;
  markAcknowledged(libraryId: string, sequence: number): Promise<void>;
  compactSnapshot(): Promise<NotificationStoredSnapshot>;
  history(query: NotificationHistoryQuery): Promise<NotificationHistoryPage>;
  getOperation(
    libraryId: string,
    operationId: string
  ): Promise<DesktopNotificationOperation | null>;
  setOperationActionError(
    libraryId: string,
    operationId: string,
    expectedRevision: number,
    detail: string | null,
    updatedAt?: number
  ): Promise<DesktopNotificationOperation | null>;
  publishThumbnailSummary(summary: ThumbnailCacheSummary): Promise<void>;
  applyLocalRecoveryBatch(
    batch: NotificationLocalRecoveryBatch,
    appliedAt: number
  ): Promise<boolean>;
  recordRendererError(
    library: NotificationLibraryIdentity,
    report: RendererErrorReport,
    reportId: string,
    occurredAt: number
  ): Promise<NotificationRendererErrorRecordResult>;
  hasLegacyImport(libraryId: string): Promise<boolean>;
  importLegacyOperations(
    library: NotificationLibraryIdentity,
    operations: NotificationOperationInput[],
    importedAt: number
  ): Promise<boolean>;
  cleanupRetention(now: number): Promise<NotificationRetentionResult>;
  close?(): Promise<void>;
}

export interface NotificationServiceOptions {
  store?: NotificationServiceStore;
  openStore?: (signal: AbortSignal) => Promise<NotificationServiceStore>;
  fetch?: typeof fetch;
  now?: () => number;
  retryDelay?: (delayMs: number, signal: AbortSignal) => Promise<void>;
  createReportId?: () => string;
}

interface ActiveLibrary {
  library: NotificationLibraryIdentity;
  session: NotificationCoreSession;
  controller: AbortController;
  generation: number;
  loop: Promise<void>;
}

interface PendingOutboxBatch {
  library: NotificationLibraryIdentity;
  events: NotificationStoreEvent[];
  maxSequence: number;
}

interface PendingRendererError {
  library: NotificationLibraryIdentity;
  report: RendererErrorReport;
  reportIds: string[];
  occurredAt: number;
}

interface PendingActionError {
  operation: DesktopNotificationOperation;
  detail: string | null;
  updatedAt: number;
}

interface LegacyFileOperationPage {
  items: FileOperationRecord[];
  nextCursor: string | null;
}

function abortableDelay(delayMs: number, signal: AbortSignal): Promise<void> {
  if (signal.aborted) return Promise.reject(signal.reason);
  return new Promise((resolve, reject) => {
    const timeoutId = setTimeout(() => {
      signal.removeEventListener("abort", onAbort);
      resolve();
    }, delayMs);
    const onAbort = () => {
      clearTimeout(timeoutId);
      reject(signal.reason);
    };
    signal.addEventListener("abort", onAbort, { once: true });
  });
}

function isAbort(error: unknown, signal?: AbortSignal): boolean {
  return Boolean(
    signal?.aborted ||
      (error instanceof DOMException && error.name === "AbortError") ||
      (error instanceof Error && error.name === "AbortError")
  );
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function emptyStoredSnapshot(): NotificationStoredSnapshot {
  return {
    active: [],
    thumbnailSummaries: [],
    latestImportantResults: [],
    retryableFailures: [],
    hasRetryableFailure: false
  };
}

function assertSafeSequence(value: unknown, name: string): asserts value is number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) {
    throw new TypeError(`${name} must be a non-negative safe integer`);
  }
}

function parseNotificationOutboxAckResponse(value: unknown): NotificationOutboxAckResponse {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError("invalid notification outbox acknowledgement response");
  }
  const acknowledged = (value as { acknowledged?: unknown }).acknowledged;
  if (typeof acknowledged !== "boolean") {
    throw new TypeError("invalid notification outbox acknowledgement response");
  }
  return { acknowledged };
}

function validateOutboxOperation(value: unknown): NotificationOperationInput {
  if (!value || typeof value !== "object") throw new TypeError("invalid notification operation");
  const operation = value as Partial<NotificationOperationInput>;
  for (const [name, field] of [
    ["operationId", operation.operationId],
    ["kind", operation.kind],
    ["title", operation.title]
  ] as const) {
    if (typeof field !== "string" || field.length === 0) {
      throw new TypeError(`notification ${name} must be a non-empty string`);
    }
  }
  assertSafeSequence(operation.revision, "notification revision");
  assertSafeSequence(operation.currentValue, "notification currentValue");
  assertSafeSequence(operation.attempt, "notification attempt");
  assertSafeSequence(operation.occurrenceCount, "notification occurrenceCount");
  assertSafeSequence(operation.startedAt, "notification startedAt");
  assertSafeSequence(operation.updatedAt, "notification updatedAt");
  for (const [name, field] of [
    ["taskId", operation.taskId],
    ["rootId", operation.rootId],
    ["folderId", operation.folderId],
    ["fileId", operation.fileId],
    ["totalValue", operation.totalValue],
    ["completedAt", operation.completedAt]
  ] as const) {
    if (field !== null) assertSafeSequence(field, `notification ${name}`);
  }
  if (!(OPERATION_CATEGORIES as Set<unknown>).has(operation.category)) {
    throw new TypeError("invalid notification category");
  }
  if (!(OPERATION_ORIGINS as Set<unknown>).has(operation.origin)) {
    throw new TypeError("invalid notification origin");
  }
  if (!(OPERATION_PRESENTATIONS as Set<unknown>).has(operation.presentation)) {
    throw new TypeError("invalid notification presentation");
  }
  if (!(OPERATION_STATUSES as Set<unknown>).has(operation.status)) {
    throw new TypeError("invalid notification status");
  }
  for (const [name, field] of [
    ["phase", operation.phase],
    ["summary", operation.summary],
    ["unit", operation.unit],
    ["errorCode", operation.errorCode],
    ["errorDetail", operation.errorDetail]
  ] as const) {
    if (field !== null && typeof field !== "string") {
      throw new TypeError(`notification ${name} must be a string or null`);
    }
  }
  if (typeof operation.cancellable !== "boolean" || typeof operation.retryable !== "boolean") {
    throw new TypeError("notification capabilities must be boolean");
  }
  return operation as NotificationOperationInput;
}

interface LegacyOperationAggregate {
  operationId: string;
  operation: FileOperationKind;
  total: number;
  succeeded: number;
  failed: number;
  startedAt: number;
  completedAt: number;
  firstError: string | null;
}

interface LegacyKindSummary {
  total: number;
  succeeded: number;
  failed: number;
}

interface LegacyImportSummaryAggregate extends LegacyKindSummary {
  startedAt: number | null;
  completedAt: number | null;
  firstError: string | null;
  byKind: Record<FileOperationKind, LegacyKindSummary>;
}

function validateHistoryQuery(query: NotificationHistoryQuery): void {
  if (!query || typeof query !== "object") throw new TypeError("history query must be an object");
  const limit = query.limit ?? 50;
  const offset = query.offset ?? 0;
  assertSafeSequence(limit, "history limit");
  assertSafeSequence(offset, "history offset");
  if (limit < 1 || limit > 200) throw new RangeError("history limit must be between 1 and 200");
  if (query.libraryId !== undefined && typeof query.libraryId !== "string") {
    throw new TypeError("history libraryId must be a string");
  }
  if (query.search !== undefined && typeof query.search !== "string") {
    throw new TypeError("history search must be a string");
  }
  if (query.statuses !== undefined) {
    if (!Array.isArray(query.statuses) || query.statuses.length === 0) {
      throw new TypeError("history statuses must be a non-empty array");
    }
    if (query.statuses.some((status) => !(OPERATION_STATUSES as Set<unknown>).has(status))) {
      throw new TypeError("history status is invalid");
    }
  }
  if (query.categories !== undefined) {
    if (!Array.isArray(query.categories) || query.categories.length === 0) {
      throw new TypeError("history categories must be a non-empty array");
    }
    if (query.categories.some((category) => !(OPERATION_CATEGORIES as Set<unknown>).has(category))) {
      throw new TypeError("history category is invalid");
    }
  }
}

function libraryKey(libraryId: string, operationId: string): string {
  return `${libraryId}\u0000${operationId}`;
}

function fallbackOperationKey(libraryId: string, operationId: string): string {
  return `operation\u0000${libraryId}\u0000${operationId}`;
}

function fallbackThumbnailKey(libraryId: string): string {
  return `thumbnail\u0000${libraryId}`;
}

function sameNotificationOperationPayload(
  left: DesktopNotificationOperation,
  right: DesktopNotificationOperation
): boolean {
  const { libraryId: _leftLibraryId, libraryName: _leftLibraryName, ...leftPayload } = left;
  const { libraryId: _rightLibraryId, libraryName: _rightLibraryName, ...rightPayload } = right;
  return JSON.stringify(leftPayload) === JSON.stringify(rightPayload);
}

function rendererErrorOperationId(libraryId: string, report: RendererErrorReport): string {
  return `renderer-error:${libraryId}:${report.source}:${report.code}:${report.target}`;
}

function cloneCompactSnapshot(snapshot: NotificationCompactSnapshot): NotificationCompactSnapshot {
  return {
    active: snapshot.active.map((item) => ({ ...item })),
    thumbnailSummaries: snapshot.thumbnailSummaries.map((item) => ({ ...item })),
    latestImportantResults: snapshot.latestImportantResults.map((item) => ({ ...item })),
    retryableFailures: snapshot.retryableFailures.map((item) => ({ ...item })),
    hasRetryableFailure: snapshot.hasRetryableFailure,
    syncHealth: { ...snapshot.syncHealth }
  };
}

export class NotificationService {
  private store: NotificationServiceStore | null;
  private readonly openStore: ((signal: AbortSignal) => Promise<NotificationServiceStore>) | null;
  private readonly fetchImpl: typeof fetch;
  private readonly now: () => number;
  private readonly retryDelay: (delayMs: number, signal: AbortSignal) => Promise<void>;
  private readonly createReportId: () => string;
  private active: ActiveLibrary | null = null;
  private activeGeneration = 0;
  private closed = false;
  private storedSnapshot = emptyStoredSnapshot();
  private health: NotificationSyncHealth;
  private readonly listeners = new Set<(snapshot: NotificationCompactSnapshot) => void>();
  private readonly pendingOutbox = new Map<string, PendingOutboxBatch>();
  private readonly fallbackOperations = new Map<string, DesktopNotificationOperation>();
  private readonly durableRetryableFailures = new Map<
    string,
    NotificationRetryableFailureReference
  >();
  private readonly fallbackRetryableFailures = new Map<
    string,
    NotificationRetryableFailureReference
  >();
  private readonly fallbackThumbnails = new Map<string, ThumbnailCacheSummary>();
  private readonly fallbackRecordOrder = new Map<string, true>();
  private readonly pendingRendererErrors = new Map<string, PendingRendererError>();
  private readonly pendingLocalThumbnails = new Map<string, ThumbnailCacheSummary>();
  private readonly pendingActionErrors = new Map<string, PendingActionError>();
  private readonly pendingRecoveryBatches: NotificationLocalRecoveryBatch[] = [];
  private readonly protectedRecoveryBatchIds = new Set<string>();
  private readonly legacyImports = new Map<string, Promise<void>>();
  private readonly fallbackActionBaseDetails = new Map<string, string | null>();
  private lastRetentionAt: number | null = null;
  private retentionPromise: Promise<void> | null = null;
  private lifecycleTail: Promise<void> = Promise.resolve();
  private storeMutationTail: Promise<void> = Promise.resolve();
  private readonly closeController = new AbortController();

  constructor(options: NotificationServiceOptions) {
    this.store = options.store ?? null;
    this.openStore = options.openStore ?? null;
    this.fetchImpl = options.fetch ?? fetch;
    this.now = options.now ?? Date.now;
    this.retryDelay = options.retryDelay ?? abortableDelay;
    this.createReportId = options.createReportId ?? randomUUID;
    this.health = {
      state: this.store ? "healthy" : "degraded",
      libraryId: null,
      detail: this.store ? null : "Notification history store is unavailable",
      updatedAt: this.now()
    };
  }

  async initialize(): Promise<void> {
    if (!this.store && this.openStore) {
      await this.tryRecoverStore(this.closeController.signal);
      return;
    }
    if (!this.store) return;
    try {
      await this.cleanupRetentionIfDue(this.store, true);
      this.storedSnapshot = await this.store.compactSnapshot();
      this.setHealth("healthy", null, null, false);
    } catch (error) {
      await this.markStoreUnavailable(error);
    }
  }

  async attachLibrary(
    library: NotificationLibraryIdentity,
    session: NotificationCoreSession
  ): Promise<void> {
    return this.enqueueLifecycle(() => this.attachLibraryNow(library, session));
  }

  private async attachLibraryNow(
    library: NotificationLibraryIdentity,
    session: NotificationCoreSession
  ): Promise<void> {
    if (this.closed) throw new Error("notification service is closed");
    if (
      this.active?.library.id === library.id &&
      this.active.session.baseUrl === session.baseUrl &&
      this.active.session.sessionToken === session.sessionToken &&
      !this.active.controller.signal.aborted
    ) {
      return;
    }
    await this.stopActiveLoop();

    const controller = new AbortController();
    const active: ActiveLibrary = {
      library,
      session,
      controller,
      generation: ++this.activeGeneration,
      loop: Promise.resolve()
    };
    this.active = active;
    this.setHealth(this.store ? "syncing" : "degraded", library.id, this.health.detail);
    active.loop = this.runDrainLoop(active).catch((error) => {
      if (!isAbort(error, controller.signal)) {
        this.setHealth(this.store ? "syncing" : "degraded", library.id, errorMessage(error));
      }
    });
    void this.ensureLegacyImport(library, session, controller.signal);
  }

  async detachLibrary(): Promise<void> {
    return this.enqueueLifecycle(async () => {
      await this.stopActiveLoop();
      this.setHealth(this.store ? "healthy" : "degraded", null, this.health.detail);
    });
  }

  async close(): Promise<void> {
    return this.enqueueLifecycle(async () => {
      if (this.closed) return;
      this.closed = true;
      this.closeController.abort(new DOMException("Notification service closed", "AbortError"));
      await this.stopActiveLoop();
      await this.storeMutationTail.catch(() => undefined);
      const store = this.store;
      this.store = null;
      await store?.close?.().catch(() => undefined);
      this.listeners.clear();
    });
  }

  compactSnapshot(): Promise<NotificationCompactSnapshot> {
    return Promise.resolve(cloneCompactSnapshot(this.currentCompactSnapshot()));
  }

  async history(query: NotificationHistoryQuery): Promise<NotificationHistoryPage> {
    validateHistoryQuery(query);
    if (this.store) {
      try {
        return await this.store.history(query);
      } catch (error) {
        await this.markStoreUnavailable(error);
      }
    }
    return this.fallbackHistory(query);
  }

  subscribe(listener: (snapshot: NotificationCompactSnapshot) => void): () => void {
    this.listeners.add(listener);
    this.notifyListener(listener, this.currentCompactSnapshot());
    return () => this.listeners.delete(listener);
  }

  async cancelOperation(
    libraryId: string,
    operationId: string
  ): Promise<NotificationActionResult> {
    return this.performTaskAction("cancel", libraryId, operationId);
  }

  async retryOperation(
    libraryId: string,
    operationId: string
  ): Promise<NotificationActionResult> {
    return this.performTaskAction("retry", libraryId, operationId);
  }

  async reportRendererError(report: RendererErrorReport): Promise<void> {
    const reportId = this.createReportId();
    if (typeof reportId !== "string" || reportId.length === 0) {
      throw new TypeError("renderer report id must be a non-empty string");
    }
    const occurredAt = this.now();
    return this.enqueueStoreMutation(() =>
      this.reportRendererErrorNow(report, reportId, occurredAt)
    );
  }

  private async reportRendererErrorNow(
    report: RendererErrorReport,
    reportId: string,
    occurredAt: number
  ): Promise<void> {
    const library = { id: report.libraryId, name: report.libraryName };
    if (this.store) {
      const store = this.store;
      let result: NotificationRendererErrorRecordResult | null = null;
      try {
        result = await store.recordRendererError(library, report, reportId, occurredAt);
      } catch (error) {
        await this.markStoreUnavailable(error);
      }
      if (result) {
        if (this.store === store) {
          try {
            await this.cleanupRetentionIfDue(store);
          } catch (error) {
            await this.markStoreUnavailable(error);
          }
        }
        if (this.store === store && (await this.refreshStoredSnapshot())) return;
        if (result.operation) {
          this.upsertFallbackOperation(result.operation);
          this.emitSnapshot();
        }
        return;
      }
    }
    const operationId = rendererErrorOperationId(library.id, report);
    const key = libraryKey(library.id, operationId);
    const pending = this.pendingRendererErrors.get(key);
    if (pending?.reportIds.includes(reportId)) return;
    this.recordFallbackRendererError(library, report, occurredAt);
    if (!this.fallbackOperations.has(key)) return;
    this.pendingRendererErrors.set(key, {
      library,
      report,
      reportIds: [...(pending?.reportIds ?? []), reportId],
      occurredAt: Math.max(pending?.occurredAt ?? occurredAt, occurredAt)
    });
    this.emitSnapshot();
  }

  async publishThumbnailSummary(summary: ThumbnailCacheSummary): Promise<void> {
    return this.enqueueStoreMutation(() => this.publishThumbnailSummaryNow(summary));
  }

  private async publishThumbnailSummaryNow(summary: ThumbnailCacheSummary): Promise<void> {
    const library = { id: summary.libraryId, name: summary.libraryName };
    if (this.store) {
      const store = this.store;
      let committed = false;
      try {
        await store.publishThumbnailSummary(summary);
        committed = true;
      } catch (error) {
        await this.markStoreUnavailable(error);
      }
      if (committed) {
        if (this.store === store) {
          try {
            await this.cleanupRetentionIfDue(store);
          } catch (error) {
            await this.markStoreUnavailable(error);
          }
        }
        if (this.store === store && (await this.refreshStoredSnapshot())) return;
        this.upsertFallbackThumbnail(summary);
        this.emitSnapshot();
        return;
      }
    }
    if (!this.upsertFallbackThumbnail(summary)) return;
    const pending = this.pendingLocalThumbnails.get(summary.libraryId);
    if (!pending || summary.observedAt >= pending.observedAt) {
      this.pendingLocalThumbnails.set(summary.libraryId, summary);
    }
    this.emitSnapshot();
  }

  async runOneDrainForTest(
    library: NotificationLibraryIdentity,
    session: NotificationCoreSession,
    signal = new AbortController().signal
  ): Promise<void> {
    await this.runOneDrain(library, session, signal);
  }

  async runLegacyImportForTest(
    library: NotificationLibraryIdentity,
    session: NotificationCoreSession,
    signal = new AbortController().signal
  ): Promise<void> {
    await this.importLegacyFileOperations(library, session, signal);
  }

  private async runDrainLoop(active: ActiveLibrary): Promise<void> {
    let delayMs = MIN_RETRY_MS;
    while (!active.controller.signal.aborted && this.active === active) {
      try {
        await this.runOneDrain(active.library, active.session, active.controller.signal);
        if (this.store) {
          delayMs = MIN_RETRY_MS;
        } else {
          await this.retryDelay(delayMs, active.controller.signal);
          delayMs = Math.min(delayMs * 2, MAX_RETRY_MS);
        }
      } catch (error) {
        if (isAbort(error, active.controller.signal)) return;
        this.setHealth(
          this.store ? "syncing" : "degraded",
          active.library.id,
          errorMessage(error)
        );
        await this.retryDelay(delayMs, active.controller.signal);
        delayMs = Math.min(delayMs * 2, MAX_RETRY_MS);
      }
    }
  }

  private async runOneDrain(
    library: NotificationLibraryIdentity,
    session: NotificationCoreSession,
    signal: AbortSignal
  ): Promise<void> {

    const pending = this.pendingOutbox.get(library.id);
    if (!this.store && (await this.tryRecoverStore(signal))) {
      void this.ensureLegacyImport(library, session, signal);
    }
    if (!this.store && pending) {
      return;
    }

    let cursor = 0;
    if (this.store) {
      const store = this.store;
      let syncState: NotificationSyncState;
      try {
        syncState = await store.syncState(library.id);
        assertSafeSequence(syncState.persistedSequence, "persisted notification cursor");
        assertSafeSequence(syncState.ackedSequence, "acknowledged notification cursor");
        if (syncState.ackedSequence > syncState.persistedSequence) {
          throw new RangeError("acknowledged notification cursor exceeds persisted cursor");
        }
      } catch (error) {
        await this.markStoreUnavailable(error);
        throw error;
      }
      if (syncState.persistedSequence > syncState.ackedSequence) {
        await this.acknowledge(session, syncState.persistedSequence, signal);
        try {
          await store.markAcknowledged(library.id, syncState.persistedSequence);
          const acknowledgedBatch = this.pendingOutbox.get(library.id);
          if (
            acknowledgedBatch &&
            acknowledgedBatch.maxSequence <= syncState.persistedSequence
          ) {
            this.pendingOutbox.delete(library.id);
          }
        } catch (error) {
          await this.markStoreUnavailable(error);
          throw error;
        }
      }
      cursor = syncState.persistedSequence;
    }

    const page = await this.listOutbox(session, cursor, signal, this.store ? OUTBOX_WAIT_MS : 0);
    if (this.store) {
      const store = this.store;
      try {
        if (signal.aborted) throw signal.reason;
        await this.cleanupRetentionIfDue(store);
        if (signal.aborted) throw signal.reason;
      } catch (error) {
        if (isAbort(error, signal)) throw error;
        await this.markStoreUnavailable(error);
        if (page.events.length > 0) {
          const batch: PendingOutboxBatch = {
            library,
            events: page.events,
            maxSequence: page.maxSequence
          };
          this.rememberPendingOutbox(batch);
          this.mergeFallbackEvents(library, page.events);
          this.emitSnapshot();
        }
        return;
      }
    }
    if (page.events.length === 0) {
      if (this.store) this.setHealth("healthy", library.id, null);
      return;
    }
    const batch: PendingOutboxBatch = {
      library,
      events: page.events,
      maxSequence: page.maxSequence
    };

    if (!this.store) {
      this.rememberPendingOutbox(batch);
      this.mergeFallbackEvents(library, page.events);
      this.emitSnapshot();
      return;
    }

    try {
      await this.store.applyOutboxBatch(library, page.events, page.maxSequence);
    } catch (error) {
      await this.markStoreUnavailable(error);
      this.rememberPendingOutbox(batch);
      this.mergeFallbackEvents(library, page.events);
      this.emitSnapshot();
      return;
    }

    const refreshed = await this.refreshStoredSnapshot();
    if (!refreshed || !this.store) {
      this.mergeFallbackEvents(library, page.events);
      this.emitSnapshot();
      return;
    }
    await this.acknowledge(session, page.maxSequence, signal);
    try {
      await this.store.markAcknowledged(library.id, page.maxSequence);
    } catch (error) {
      await this.markStoreUnavailable(error);
      throw error;
    }
    this.pendingOutbox.delete(library.id);
    this.setHealth("healthy", library.id, null);
  }

  private async listOutbox(
    session: NotificationCoreSession,
    afterSequence: number,
    signal: AbortSignal,
    waitMs = OUTBOX_WAIT_MS
  ): Promise<{ events: NotificationStoreEvent[]; maxSequence: number }> {
    const query = new URLSearchParams({
      afterSequence: String(afterSequence),
      limit: String(OUTBOX_LIMIT),
      waitMs: String(waitMs)
    });
    const body = await this.requestJson(
      `${session.baseUrl}/notifications/outbox?${query}`,
      session,
      { signal }
    );
    if (!body || typeof body !== "object" || !Array.isArray((body as { items?: unknown }).items)) {
      throw new TypeError("invalid notification outbox response");
    }
    const maxSequence = (body as { maxSequence?: unknown }).maxSequence;
    assertSafeSequence(maxSequence, "notification maxSequence");
    if (maxSequence < afterSequence) {
      throw new RangeError("notification maxSequence cannot move backwards");
    }
    const events = (body as { items: unknown[] }).items.map((item) => {
      if (!item || typeof item !== "object") throw new TypeError("invalid notification outbox event");
      const sequence = (item as { sequence?: unknown }).sequence;
      assertSafeSequence(sequence, "notification sequence");
      if (sequence > maxSequence) throw new RangeError("notification sequence exceeds maxSequence");
      const operation = validateOutboxOperation((item as { operation?: unknown }).operation);
      return { sequence, operation };
    });
    for (let index = 1; index < events.length; index += 1) {
      if (events[index]!.sequence <= events[index - 1]!.sequence) {
        throw new RangeError("notification sequences must be strictly increasing");
      }
    }
    return { events, maxSequence };
  }

  private async acknowledge(
    session: NotificationCoreSession,
    maxSequence: number,
    signal: AbortSignal
  ): Promise<void> {
    const request: NotificationOutboxAckRequest = { maxSequence };
    const body = await this.requestJson(`${session.baseUrl}/notifications/outbox/ack`, session, {
      method: "POST",
      signal,
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request)
    });
    const acknowledgement = parseNotificationOutboxAckResponse(body);
    if (!acknowledgement.acknowledged) {
      throw new Error("Core did not acknowledge the notification outbox sequence");
    }
  }

  private async requestJson(
    url: string,
    session: NotificationCoreSession,
    init: RequestInit = {}
  ): Promise<unknown> {
    const headers = new Headers(init.headers);
    headers.set("x-megle-session", session.sessionToken);
    const result = await this.fetchImpl(url, { ...init, headers });
    const text = await result.text();
    if (!result.ok) {
      throw new Error(`Core request failed (${result.status}): ${text || result.statusText}`);
    }
    if (!text) return null;
    try {
      return JSON.parse(text) as unknown;
    } catch {
      throw new TypeError("Core returned invalid JSON");
    }
  }

  private async ensureLegacyImport(
    library: NotificationLibraryIdentity,
    session: NotificationCoreSession,
    signal: AbortSignal
  ): Promise<void> {
    const existing = this.legacyImports.get(library.id);
    if (existing) return existing;
    const running = this.importLegacyFileOperations(library, session, signal)
      .catch((error) => {
        if (!isAbort(error, signal)) {
          this.setHealth(this.store ? "syncing" : "degraded", library.id, errorMessage(error));
        }
      })
      .finally(() => this.legacyImports.delete(library.id));
    this.legacyImports.set(library.id, running);
    return running;
  }

  private async importLegacyFileOperations(
    library: NotificationLibraryIdentity,
    session: NotificationCoreSession,
    signal: AbortSignal
  ): Promise<void> {
    if (!this.store) return;
    const store = this.store;
    if (await store.hasLegacyImport(library.id)) return;

    const cutoff = this.now() - LEGACY_RETENTION_MS;
    const groupedOperations = new Map<string, LegacyOperationAggregate>();
    const ungroupedSummary: LegacyImportSummaryAggregate = {
      total: 0,
      succeeded: 0,
      failed: 0,
      startedAt: null,
      completedAt: null,
      firstError: null,
      byKind: {
        rename: { total: 0, succeeded: 0, failed: 0 },
        move: { total: 0, succeeded: 0, failed: 0 },
        delete_recycle: { total: 0, succeeded: 0, failed: 0 },
        delete_permanent: { total: 0, succeeded: 0, failed: 0 }
      }
    };
    let cursor: string | null = null;
    let reachedLogicalLimit = false;
    while (!signal.aborted) {
      const query = new URLSearchParams({ limit: String(LEGACY_PAGE_LIMIT) });
      if (cursor) query.set("cursor", cursor);
      const body = await this.requestJson(`${session.baseUrl}/file-ops?${query}`, session, { signal });
      const page = this.parseLegacyPage(body);
      const eligible = page.items.filter((item) => item.createdAt >= cutoff);
      for (const item of eligible) {
        const completedAt = item.finishedAt ?? item.createdAt;
        if (item.operationGroupId) {
          const existing = groupedOperations.get(item.operationGroupId);
          if (!existing && groupedOperations.size >= LEGACY_LOGICAL_LIMIT) {
            reachedLogicalLimit = true;
            break;
          }
          groupedOperations.set(item.operationGroupId, {
            operationId: item.operationGroupId,
            operation: existing?.operation ?? item.operation,
            total: (existing?.total ?? 0) + 1,
            succeeded: (existing?.succeeded ?? 0) + (item.status === "succeeded" ? 1 : 0),
            failed: (existing?.failed ?? 0) + (item.status === "failed" ? 1 : 0),
            startedAt: Math.min(existing?.startedAt ?? item.createdAt, item.createdAt),
            completedAt: Math.max(existing?.completedAt ?? completedAt, completedAt),
            firstError: existing?.firstError ?? item.error
          });
          continue;
        }

        const kind = ungroupedSummary.byKind[item.operation];
        ungroupedSummary.total += 1;
        ungroupedSummary.succeeded += item.status === "succeeded" ? 1 : 0;
        ungroupedSummary.failed += item.status === "failed" ? 1 : 0;
        ungroupedSummary.startedAt = Math.min(
          ungroupedSummary.startedAt ?? item.createdAt,
          item.createdAt
        );
        ungroupedSummary.completedAt = Math.max(
          ungroupedSummary.completedAt ?? completedAt,
          completedAt
        );
        ungroupedSummary.firstError ??= item.error;
        kind.total += 1;
        kind.succeeded += item.status === "succeeded" ? 1 : 0;
        kind.failed += item.status === "failed" ? 1 : 0;
      }
      const reachedCutoff = page.items.some((item) => item.createdAt < cutoff);
      if (
        reachedLogicalLimit ||
        reachedCutoff ||
        !page.nextCursor ||
        page.items.length === 0
      ) {
        break;
      }
      cursor = page.nextCursor;
    }
    if (signal.aborted) throw signal.reason;
    const operations: NotificationOperationInput[] = [];
    for (const aggregate of groupedOperations.values()) {
      if (await store.getOperation(library.id, aggregate.operationId)) continue;
      operations.push(this.legacyOperation(aggregate));
    }
    if (ungroupedSummary.total > 0) {
      operations.push(this.legacyImportSummaryOperation(library.id, ungroupedSummary));
    }
    await store.importLegacyOperations(library, operations, this.now());
    await this.cleanupRetentionIfDue(store);
    if (this.store === store) await this.refreshStoredSnapshot();
  }

  private parseLegacyPage(body: unknown): LegacyFileOperationPage {
    if (!body || typeof body !== "object" || !Array.isArray((body as { items?: unknown }).items)) {
      throw new TypeError("invalid legacy file operation response");
    }
    const nextCursor = (body as { nextCursor?: unknown }).nextCursor;
    if (nextCursor !== null && nextCursor !== undefined && typeof nextCursor !== "string") {
      throw new TypeError("invalid legacy file operation cursor");
    }
    const items = (body as { items: unknown[] }).items.map((value) => {
      if (!value || typeof value !== "object") throw new TypeError("invalid legacy file operation");
      const item = value as Partial<FileOperationRecord>;
      assertSafeSequence(item.id, "legacy file operation id");
      assertSafeSequence(item.createdAt, "legacy file operation createdAt");
      if (item.finishedAt !== null) assertSafeSequence(item.finishedAt, "legacy file operation finishedAt");
      if (!(LEGACY_OPERATION_KINDS as readonly unknown[]).includes(item.operation)) {
        throw new TypeError("invalid legacy file operation kind");
      }
      if (item.status !== "succeeded" && item.status !== "failed") {
        throw new TypeError("invalid legacy file operation status");
      }
      if (typeof item.sourcePath !== "string") throw new TypeError("invalid legacy sourcePath");
      if (item.targetPath !== null && typeof item.targetPath !== "string") {
        throw new TypeError("invalid legacy targetPath");
      }
      if (item.error !== null && typeof item.error !== "string") {
        throw new TypeError("invalid legacy file operation error");
      }
      if (
        item.operationGroupId !== null &&
        item.operationGroupId !== undefined &&
        (typeof item.operationGroupId !== "string" || item.operationGroupId.length === 0)
      ) {
        throw new TypeError("invalid legacy operationGroupId");
      }
      return item as FileOperationRecord;
    });
    return { items, nextCursor: nextCursor ?? null };
  }

  private legacyOperation(aggregate: LegacyOperationAggregate): NotificationOperationInput {
    const status = aggregate.failed > 0 ? "failed" : "succeeded";
    return {
      operationId: aggregate.operationId,
      revision: 0,
      kind: aggregate.operation,
      category: "file_operation",
      origin: "user",
      presentation: "history_only",
      status,
      phase: "completed",
      title: LEGACY_OPERATION_LABELS[aggregate.operation],
      summary: `${aggregate.succeeded} succeeded, ${aggregate.failed} failed (${aggregate.total} total)`,
      taskId: null,
      rootId: null,
      folderId: null,
      fileId: null,
      currentValue: aggregate.total,
      totalValue: aggregate.total,
      unit: "items",
      errorCode: status === "failed" ? "legacy_file_operation_failed" : null,
      errorDetail: aggregate.firstError,
      cancellable: false,
      retryable: false,
      attempt: 1,
      occurrenceCount: 1,
      startedAt: aggregate.startedAt,
      updatedAt: aggregate.completedAt,
      completedAt: aggregate.completedAt
    };
  }

  private legacyImportSummaryOperation(
    libraryId: string,
    aggregate: LegacyImportSummaryAggregate
  ): NotificationOperationInput {
    const status = aggregate.failed > 0 ? "failed" : "succeeded";
    const kindSummary = LEGACY_OPERATION_KINDS.filter(
      (kind) => aggregate.byKind[kind].total > 0
    )
      .map((kind) => {
        const counts = aggregate.byKind[kind];
        return `${LEGACY_OPERATION_LABELS[kind]}: ${counts.total} (${counts.succeeded} succeeded, ${counts.failed} failed)`;
      })
      .join("; ");
    const completedAt = aggregate.completedAt ?? this.now();
    return {
      operationId: `legacy-import:${libraryId}`,
      revision: 0,
      kind: "legacy_file_operations",
      category: "file_operation",
      origin: "user",
      presentation: "history_only",
      status,
      phase: "imported",
      title: "Imported legacy file operations",
      summary: `${aggregate.succeeded} succeeded, ${aggregate.failed} failed (${aggregate.total} total); ${kindSummary}`,
      taskId: null,
      rootId: null,
      folderId: null,
      fileId: null,
      currentValue: aggregate.total,
      totalValue: aggregate.total,
      unit: "items",
      errorCode: status === "failed" ? "legacy_file_operation_failed" : null,
      errorDetail: aggregate.firstError,
      cancellable: false,
      retryable: false,
      attempt: 1,
      occurrenceCount: 1,
      startedAt: aggregate.startedAt ?? completedAt,
      updatedAt: completedAt,
      completedAt
    };
  }

  private async performTaskAction(
    action: "cancel" | "retry",
    libraryId: string,
    operationId: string
  ): Promise<NotificationActionResult> {
    const active = this.active;
    if (!active || active.library.id !== libraryId) {
      return { ok: false, message: "This operation belongs to an inactive library." };
    }
    const activeLibraryId = active.library.id;
    const generation = active.generation;
    const signal = active.controller.signal;
    const operation = await this.findOperation(libraryId, operationId);
    if (
      signal.aborted ||
      this.active !== active ||
      this.active.generation !== generation ||
      this.active.library.id !== activeLibraryId
    ) {
      return { ok: false, message: "This operation belongs to an inactive library." };
    }
    if (!operation || operation.taskId === null) {
      return { ok: false, message: "The task is no longer available." };
    }
    if (action === "cancel" && !operation.cancellable) {
      return { ok: false, message: "This operation cannot be cancelled." };
    }
    if (action === "retry" && !operation.retryable) {
      return { ok: false, message: "This operation cannot be retried." };
    }
    try {
      await this.requestJson(
        `${active.session.baseUrl}/tasks/${operation.taskId}/${action}`,
        active.session,
        { method: "POST", signal }
      );
      await this.persistActionError(operation, null);
      return { ok: true, message: null };
    } catch (error) {
      if (isAbort(error, signal) || this.active !== active) {
        return { ok: false, message: "This operation belongs to an inactive library." };
      }
      const message = errorMessage(error);
      await this.persistActionError(operation, message);
      return { ok: false, message };
    }
  }

  private async persistActionError(
    operation: DesktopNotificationOperation,
    detail: string | null
  ): Promise<void> {
    return this.enqueueStoreMutation(() => this.persistActionErrorNow(operation, detail));
  }

  private async persistActionErrorNow(
    operation: DesktopNotificationOperation,
    detail: string | null
  ): Promise<void> {
    const key = libraryKey(operation.libraryId, operation.operationId);
    const updatedAt = this.now();
    if (this.store) {
      const store = this.store;
      try {
        const updated = await store.setOperationActionError(
          operation.libraryId,
          operation.operationId,
          operation.revision,
          detail,
          updatedAt
        );
        if (updated) {
          try {
            await this.cleanupRetentionIfDue(store);
          } catch (error) {
            await this.markStoreUnavailable(error);
          }
          if (this.store === store && (await this.refreshStoredSnapshot())) return;
          this.upsertFallbackOperation(operation);
          this.applyFallbackActionOverlay(operation, detail);
          this.reconcileFallbackRetryableFailure(operation, detail !== null);
          this.emitSnapshot();
        }
        return;
      } catch (error) {
        await this.markStoreUnavailable(error);
      }
    }
    if (this.applyFallbackActionOverlay(operation, detail)) {
      this.pendingActionErrors.set(key, { operation, detail, updatedAt });
    }
    this.reconcileFallbackRetryableFailure(operation, detail !== null);
    this.emitSnapshot();
  }

  private async findOperation(
    libraryId: string,
    operationId: string
  ): Promise<DesktopNotificationOperation | null> {
    if (this.store) {
      try {
        return await this.store.getOperation(libraryId, operationId);
      } catch (error) {
        await this.markStoreUnavailable(error);
      }
    }
    return this.fallbackOperations.get(libraryKey(libraryId, operationId)) ?? null;
  }

  private async tryRecoverStore(signal: AbortSignal): Promise<boolean> {
    return this.enqueueStoreMutation(() => this.tryRecoverStoreNow(signal));
  }

  private async tryRecoverStoreNow(signal: AbortSignal): Promise<boolean> {
    if (this.store) return true;
    if (!this.openStore) return false;
    let store: NotificationServiceStore | null = null;
    try {
      store = await this.openStoreWithSignal(signal);
      if (signal.aborted) throw signal.reason;
      await this.flushPendingOutboxState(store);
      await this.flushPendingLocalState(store);
      await this.cleanupRetentionIfDue(store, true);
      const snapshot = await store.compactSnapshot();
      if (signal.aborted) throw signal.reason;
      this.store = store;
      this.storedSnapshot = snapshot;
    } catch (error) {
      void store?.close?.().catch(() => undefined);
      if (isAbort(error, signal)) throw error;
      this.setHealth("degraded", this.active?.library.id ?? null, errorMessage(error));
      return false;
    }
    this.pendingOutbox.clear();
    this.pendingRecoveryBatches.length = 0;
    this.protectedRecoveryBatchIds.clear();
    this.clearFallbackFromDurableState();
    this.setHealth("healthy", this.active?.library.id ?? null, null);
    return true;
  }

  private async flushPendingLocalState(store: NotificationServiceStore): Promise<void> {
    if (
      this.pendingRendererErrors.size > 0 ||
      this.pendingLocalThumbnails.size > 0 ||
      this.pendingActionErrors.size > 0
    ) {
      this.pendingRecoveryBatches.push({
        batchId: randomUUID(),
        rendererErrors: [...this.pendingRendererErrors.values()],
        thumbnails: [...this.pendingLocalThumbnails.values()],
        actionErrors: [...this.pendingActionErrors.values()].map((pending) => ({
          libraryId: pending.operation.libraryId,
          operationId: pending.operation.operationId,
          expectedRevision: pending.operation.revision,
          detail: pending.detail,
          updatedAt: pending.updatedAt
        })),
        createdAt: this.now()
      });
      this.pendingRendererErrors.clear();
      this.pendingLocalThumbnails.clear();
      this.pendingActionErrors.clear();
    }
    for (const batch of this.pendingRecoveryBatches) {
      // Once transmission starts, keep the payload stable even if the commit response is lost.
      this.protectedRecoveryBatchIds.add(batch.batchId);
      await store.applyLocalRecoveryBatch(batch, this.now());
    }
  }

  private async flushPendingOutboxState(store: NotificationServiceStore): Promise<void> {
    const pending = [...this.pendingOutbox.values()];
    for (const batch of pending) {
      await store.applyOutboxBatch(batch.library, batch.events, batch.maxSequence);
    }
  }

  private async refreshStoredSnapshot(): Promise<boolean> {
    if (!this.store) return false;
    try {
      this.storedSnapshot = await this.store.compactSnapshot();
      this.emitSnapshot();
      return true;
    } catch (error) {
      await this.markStoreUnavailable(error);
      return false;
    }
  }

  private async cleanupRetentionIfDue(
    store: NotificationServiceStore,
    force = false
  ): Promise<void> {
    const now = this.now();
    if (
      !force &&
      this.lastRetentionAt !== null &&
      now - this.lastRetentionAt < RETENTION_CADENCE_MS
    ) {
      return;
    }
    if (this.retentionPromise) {
      await this.retentionPromise;
      return;
    }
    const cleanup = (async () => {
      await store.cleanupRetention(now);
      this.lastRetentionAt = now;
    })();
    this.retentionPromise = cleanup;
    try {
      await cleanup;
    } finally {
      if (this.retentionPromise === cleanup) this.retentionPromise = null;
    }
  }

  private async markStoreUnavailable(error: unknown): Promise<void> {
    const failed = this.store;
    this.store = null;
    this.seedFallbackFromStoredSnapshot();
    this.setHealth("degraded", this.active?.library.id ?? null, errorMessage(error));
    void failed?.close?.().catch(() => undefined);
  }

  private seedFallbackFromStoredSnapshot(): void {
    this.durableRetryableFailures.clear();
    this.fallbackRetryableFailures.clear();
    for (const failure of this.storedSnapshot.retryableFailures) {
      const key = libraryKey(failure.libraryId, failure.operationId);
      const existing = this.durableRetryableFailures.get(key);
      if (!existing || failure.revision > existing.revision) {
        this.durableRetryableFailures.set(key, { ...failure });
      }
    }
    for (const operation of [
      ...this.storedSnapshot.active,
      ...this.storedSnapshot.latestImportantResults
    ]) {
      this.seedFallbackOperation(operation);
    }
    for (const summary of this.storedSnapshot.thumbnailSummaries) {
      this.upsertFallbackThumbnail(summary);
    }
  }

  private mergeFallbackEvents(
    library: NotificationLibraryIdentity,
    events: NotificationStoreEvent[]
  ): void {
    for (const event of events) {
      this.upsertFallbackOperation({
        ...event.operation,
        libraryId: library.id,
        libraryName: library.name
      });
    }
  }

  private rememberPendingOutbox(batch: PendingOutboxBatch): void {
    this.pendingOutbox.delete(batch.library.id);
    this.pendingOutbox.set(batch.library.id, batch);
    let eventCount = [...this.pendingOutbox.values()].reduce(
      (total, item) => total + item.events.length,
      0
    );
    while (eventCount > FALLBACK_RECORD_LIMIT) {
      const oldest = this.pendingOutbox.entries().next().value as
        | [string, PendingOutboxBatch]
        | undefined;
      if (!oldest) break;
      this.pendingOutbox.delete(oldest[0]);
      eventCount -= oldest[1].events.length;
    }
  }

  private recordFallbackRendererError(
    library: NotificationLibraryIdentity,
    report: RendererErrorReport,
    occurredAt: number
  ): DesktopNotificationOperation {
    const operationId = rendererErrorOperationId(library.id, report);
    const key = libraryKey(library.id, operationId);
    const existing = this.fallbackOperations.get(key);
    const occurrenceCount = (existing?.occurrenceCount ?? 0) + 1;
    const value: DesktopNotificationOperation = {
      operationId,
      revision: (existing?.revision ?? 0) + 1,
      kind: "renderer_error",
      category: "error",
      origin: "system",
      presentation: "important",
      status: "failed",
      phase: "reported",
      title: report.title,
      summary: report.message,
      taskId: null,
      rootId: null,
      folderId: null,
      fileId: null,
      currentValue: occurrenceCount,
      totalValue: null,
      unit: "occurrences",
      errorCode: report.code,
      errorDetail: report.detail,
      cancellable: false,
      retryable: false,
      attempt: 1,
      occurrenceCount,
      startedAt: existing?.startedAt ?? occurredAt,
      updatedAt: occurredAt,
      completedAt: occurredAt,
      libraryId: library.id,
      libraryName: library.name
    };
    this.upsertFallbackOperation(value);
    return value;
  }

  private upsertFallbackOperation(operation: DesktopNotificationOperation): void {
    const key = libraryKey(operation.libraryId, operation.operationId);
    const existing = this.fallbackOperations.get(key);
    const durableFailure = this.durableRetryableFailures.get(key);
    const fallbackFailure = this.fallbackRetryableFailures.get(key);
    const knownRevision = Math.max(
      existing?.revision ?? -1,
      durableFailure?.revision ?? -1,
      fallbackFailure?.revision ?? -1
    );
    if (operation.revision < knownRevision) return;
    if (operation.revision === knownRevision) {
      if (!existing) return;
      const comparableExisting = this.fallbackActionBaseDetails.has(key)
        ? {
            ...existing,
            errorDetail: this.fallbackActionBaseDetails.get(key) ?? null
          }
        : existing;
      if (sameNotificationOperationPayload(comparableExisting, operation)) return;
      return;
    }
    if (existing && operation.revision > existing.revision) {
      this.fallbackActionBaseDetails.delete(key);
      this.pendingActionErrors.delete(key);
    } else if (existing && this.fallbackActionBaseDetails.has(key)) {
      operation = { ...operation, errorDetail: existing.errorDetail };
    }
    this.fallbackOperations.delete(key);
    this.fallbackOperations.set(key, operation);
    this.reconcileFallbackRetryableFailure(operation, undefined, knownRevision);
    this.touchFallbackRecord(fallbackOperationKey(operation.libraryId, operation.operationId));
  }

  private seedFallbackOperation(operation: DesktopNotificationOperation): void {
    const key = libraryKey(operation.libraryId, operation.operationId);
    const existing = this.fallbackOperations.get(key);
    if (existing && operation.revision <= existing.revision) return;
    this.fallbackOperations.set(key, operation);
    this.touchFallbackRecord(fallbackOperationKey(operation.libraryId, operation.operationId));
  }

  private applyFallbackActionOverlay(
    operation: DesktopNotificationOperation,
    detail: string | null
  ): boolean {
    const key = libraryKey(operation.libraryId, operation.operationId);
    const existing = this.fallbackOperations.get(key);
    if (!existing || existing.revision !== operation.revision) return false;

    const baseDetail = this.fallbackActionBaseDetails.has(key)
      ? this.fallbackActionBaseDetails.get(key) ?? null
      : existing.errorDetail;
    if (detail === null) {
      this.fallbackActionBaseDetails.delete(key);
      this.fallbackOperations.set(key, { ...existing, errorDetail: baseDetail });
    } else {
      this.fallbackActionBaseDetails.set(key, baseDetail);
      this.fallbackOperations.set(key, { ...existing, errorDetail: detail });
    }
    this.touchFallbackRecord(fallbackOperationKey(operation.libraryId, operation.operationId));
    return this.fallbackOperations.has(key);
  }

  private reconcileFallbackRetryableFailure(
    operation: DesktopNotificationOperation,
    actionFailure?: boolean,
    previousRevision?: number
  ): void {
    const key = libraryKey(operation.libraryId, operation.operationId);
    const durableFailure = this.durableRetryableFailures.get(key);
    const fallbackFailure = this.fallbackRetryableFailures.get(key);
    const knownRevision =
      previousRevision ??
      Math.max(
        this.fallbackOperations.get(key)?.revision ?? -1,
        durableFailure?.revision ?? -1,
        fallbackFailure?.revision ?? -1
      );
    if (operation.revision < knownRevision) return;

    const terminal = operation.status !== "pending" && operation.status !== "running";
    if (actionFailure !== undefined) {
      if (!terminal) return;
      if (actionFailure) {
        this.durableRetryableFailures.delete(key);
        this.fallbackRetryableFailures.set(key, {
          libraryId: operation.libraryId,
          operationId: operation.operationId,
          revision: operation.revision
        });
        return;
      }

      this.fallbackRetryableFailures.delete(key);
      if (operation.status === "failed" && operation.retryable) {
        if (durableFailure?.revision === operation.revision) return;
        this.durableRetryableFailures.delete(key);
        this.fallbackRetryableFailures.set(key, {
          libraryId: operation.libraryId,
          operationId: operation.operationId,
          revision: operation.revision
        });
      } else {
        this.durableRetryableFailures.delete(key);
      }
      return;
    }

    if (operation.revision <= knownRevision) return;

    this.durableRetryableFailures.delete(key);
    this.fallbackRetryableFailures.delete(key);

    if (terminal && operation.status === "failed" && operation.retryable) {
      this.fallbackRetryableFailures.set(key, {
        libraryId: operation.libraryId,
        operationId: operation.operationId,
        revision: operation.revision
      });
    }
  }

  private fallbackHistory(query: NotificationHistoryQuery): NotificationHistoryPage {
    const offset = query.offset ?? 0;
    const limit = Math.min(query.limit ?? 50, 200);
    let items = [...this.fallbackOperations.values()].filter(
      (item) => item.status !== "pending" && item.status !== "running"
    );
    if (query.libraryId) items = items.filter((item) => item.libraryId === query.libraryId);
    if (query.statuses) items = items.filter((item) => query.statuses?.includes(item.status));
    if (query.categories) items = items.filter((item) => query.categories?.includes(item.category));
    if (query.search?.trim()) {
      const search = query.search.trim().toLocaleLowerCase();
      items = items.filter((item) =>
        `${item.title} ${item.summary ?? ""}`.toLocaleLowerCase().includes(search)
      );
    }
    items.sort(
      (left, right) =>
        (right.completedAt ?? right.updatedAt) - (left.completedAt ?? left.updatedAt) ||
        right.updatedAt - left.updatedAt ||
        left.operationId.localeCompare(right.operationId)
    );
    return {
      items: items.slice(offset, offset + limit),
      total: items.length,
      nextOffset: offset + limit < items.length ? offset + limit : null
    };
  }

  private currentCompactSnapshot(): NotificationCompactSnapshot {
    if (this.store) return { ...this.storedSnapshot, syncHealth: this.health };
    const operations = [...this.fallbackOperations.values()];
    const retryableFailureMap = new Map(this.durableRetryableFailures);
    for (const [key, failure] of this.fallbackRetryableFailures) {
      const existing = retryableFailureMap.get(key);
      if (!existing || failure.revision > existing.revision) {
        retryableFailureMap.set(key, failure);
      }
    }
    const retryableFailures = [...retryableFailureMap.values()].sort(
      (left, right) =>
        left.libraryId.localeCompare(right.libraryId) ||
        left.operationId.localeCompare(right.operationId)
    );
    return {
      active: operations
        .filter((item) => item.status === "pending" || item.status === "running")
        .sort((left, right) => right.updatedAt - left.updatedAt),
      thumbnailSummaries: [...this.fallbackThumbnails.values()].sort((left, right) =>
        left.libraryName.localeCompare(right.libraryName)
      ),
      latestImportantResults: operations
        .filter(
          (item) =>
            item.status !== "pending" &&
            item.status !== "running" &&
            item.presentation === "important"
        )
        .sort(
          (left, right) =>
            (right.completedAt ?? right.updatedAt) - (left.completedAt ?? left.updatedAt) ||
            right.updatedAt - left.updatedAt
        )
        .slice(0, 3),
      retryableFailures,
      hasRetryableFailure: retryableFailures.length > 0,
      syncHealth: this.health
    };
  }

  private clearFallbackFromDurableState(): void {
    this.fallbackOperations.clear();
    this.durableRetryableFailures.clear();
    this.fallbackRetryableFailures.clear();
    this.fallbackThumbnails.clear();
    this.fallbackActionBaseDetails.clear();
    this.fallbackRecordOrder.clear();
  }

  private upsertFallbackThumbnail(summary: ThumbnailCacheSummary): boolean {
    const existing = this.fallbackThumbnails.get(summary.libraryId);
    if (existing && summary.observedAt < existing.observedAt) return false;
    this.fallbackThumbnails.delete(summary.libraryId);
    this.fallbackThumbnails.set(summary.libraryId, summary);
    this.touchFallbackRecord(fallbackThumbnailKey(summary.libraryId));
    return this.fallbackThumbnails.has(summary.libraryId);
  }

  private touchFallbackRecord(recordKey: string): void {
    this.fallbackRecordOrder.delete(recordKey);
    this.fallbackRecordOrder.set(recordKey, true);
    while (this.fallbackRecordOrder.size > FALLBACK_RECORD_LIMIT) {
      const oldest = [...this.fallbackRecordOrder.keys()].find(
        (key) => !this.isAppliedRecoveryRecord(key)
      );
      if (typeof oldest !== "string") break;
      this.evictFallbackRecord(oldest);
    }
  }

  private isAppliedRecoveryRecord(recordKey: string): boolean {
    const [kind, libraryId, ...rest] = recordKey.split("\u0000");
    const operationId = rest.join("\u0000");
    return this.pendingRecoveryBatches.some((batch) => {
      if (!this.protectedRecoveryBatchIds.has(batch.batchId)) return false;
      if (kind === "thumbnail") {
        return batch.thumbnails.some((summary) => summary.libraryId === libraryId);
      }
      if (kind !== "operation") return false;
      return (
        batch.rendererErrors.some(
          (pending) =>
            pending.library.id === libraryId &&
            rendererErrorOperationId(pending.library.id, pending.report) === operationId
        ) ||
        batch.actionErrors.some(
          (pending) => pending.libraryId === libraryId && pending.operationId === operationId
        )
      );
    });
  }

  private evictFallbackRecord(recordKey: string): void {
    this.fallbackRecordOrder.delete(recordKey);
    const [kind, libraryId, ...rest] = recordKey.split("\u0000");
    if (kind === "thumbnail") {
      this.fallbackThumbnails.delete(libraryId);
      this.pendingLocalThumbnails.delete(libraryId);
      for (let index = this.pendingRecoveryBatches.length - 1; index >= 0; index -= 1) {
        const batch = this.pendingRecoveryBatches[index]!;
        batch.thumbnails = batch.thumbnails.filter((item) => item.libraryId !== libraryId);
        if (
          batch.rendererErrors.length === 0 &&
          batch.thumbnails.length === 0 &&
          batch.actionErrors.length === 0
        ) {
          this.pendingRecoveryBatches.splice(index, 1);
        }
      }
      return;
    }
    if (kind !== "operation") return;
    const operationId = rest.join("\u0000");
    const key = libraryKey(libraryId, operationId);
    this.fallbackOperations.delete(key);
    this.fallbackRetryableFailures.delete(key);
    this.pendingRendererErrors.delete(key);
    this.pendingActionErrors.delete(key);
    this.fallbackActionBaseDetails.delete(key);
    for (const [pendingLibraryId, batch] of this.pendingOutbox) {
      if (
        batch.library.id === libraryId &&
        batch.events.some((event) => event.operation.operationId === operationId)
      ) {
        this.pendingOutbox.delete(pendingLibraryId);
      }
    }
    for (let index = this.pendingRecoveryBatches.length - 1; index >= 0; index -= 1) {
      const batch = this.pendingRecoveryBatches[index]!;
      batch.rendererErrors = batch.rendererErrors.filter(
        (pending) =>
          pending.library.id !== libraryId ||
          rendererErrorOperationId(pending.library.id, pending.report) !== operationId
      );
      batch.actionErrors = batch.actionErrors.filter(
        (pending) => pending.libraryId !== libraryId || pending.operationId !== operationId
      );
      if (
        batch.rendererErrors.length === 0 &&
        batch.thumbnails.length === 0 &&
        batch.actionErrors.length === 0
      ) {
        this.pendingRecoveryBatches.splice(index, 1);
      }
    }
  }

  private setHealth(
    state: NotificationSyncHealth["state"],
    libraryId: string | null,
    detail: string | null,
    emit = true
  ): void {
    this.health = { state, libraryId, detail, updatedAt: this.now() };
    if (emit) this.emitSnapshot();
  }

  private emitSnapshot(): void {
    if (this.listeners.size === 0) return;
    const snapshot = this.currentCompactSnapshot();
    for (const listener of this.listeners) this.notifyListener(listener, snapshot);
  }

  private notifyListener(
    listener: (snapshot: NotificationCompactSnapshot) => void,
    snapshot: NotificationCompactSnapshot
  ): void {
    try {
      listener(cloneCompactSnapshot(snapshot));
    } catch {
      this.listeners.delete(listener);
    }
  }

  private async stopActiveLoop(): Promise<void> {
    const active = this.active;
    if (!active) return;
    this.active = null;
    active.controller.abort(new DOMException("Library detached", "AbortError"));
    await active.loop.catch(() => undefined);
  }

  private async openStoreWithSignal(signal: AbortSignal): Promise<NotificationServiceStore> {
    if (!this.openStore) throw new Error("notification history store opener is unavailable");
    if (signal.aborted) throw signal.reason;
    const opening = Promise.resolve().then(() => this.openStore!(signal));
    let onAbort!: () => void;
    const aborted = new Promise<never>((_resolve, reject) => {
      onAbort = () => reject(signal.reason);
      signal.addEventListener("abort", onAbort, { once: true });
    });
    try {
      return await Promise.race([opening, aborted]);
    } catch (error) {
      if (signal.aborted) {
        void opening.then(
          (store) => store.close?.().catch(() => undefined),
          () => undefined
        );
      }
      throw error;
    } finally {
      signal.removeEventListener("abort", onAbort);
    }
  }

  private enqueueStoreMutation<T>(operation: () => Promise<T>): Promise<T> {
    const run = this.storeMutationTail.then(operation, operation);
    this.storeMutationTail = run.then(
      () => undefined,
      () => undefined
    );
    return run;
  }

  private enqueueLifecycle(operation: () => Promise<void>): Promise<void> {
    const run = this.lifecycleTail.then(operation, operation);
    this.lifecycleTail = run.catch(() => undefined);
    return run;
  }
}

export async function createNotificationService(
  options: NotificationServiceOptions
): Promise<NotificationService> {
  const service = new NotificationService(options);
  await service.initialize();
  return service;
}
