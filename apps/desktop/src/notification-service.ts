import type { CoreSession } from "./core-session.js";
import type {
  DesktopNotificationOperation,
  NotificationActionResult,
  NotificationCompactSnapshot,
  NotificationHistoryPage,
  NotificationHistoryQuery,
  NotificationLibraryIdentity,
  NotificationOperationInput,
  NotificationStoreEvent,
  NotificationStoredSnapshot,
  NotificationSyncHealth,
  RendererErrorReport,
  ThumbnailCacheSummary
} from "./notification-types.js";

const OUTBOX_LIMIT = 200;
const OUTBOX_WAIT_MS = 30_000;
const LEGACY_PAGE_LIMIT = 200;
const LEGACY_RETENTION_MS = 30 * 24 * 60 * 60 * 1_000;
const FALLBACK_OPERATION_LIMIT = 500;
const MIN_RETRY_MS = 250;
const MAX_RETRY_MS = 5_000;
const OPERATION_CATEGORIES = new Set(["task", "file_operation", "error", "thumbnail_cache"]);
const OPERATION_ORIGINS = new Set(["user", "background", "recovery", "system"]);
const OPERATION_PRESENTATIONS = new Set(["important", "history_only", "status_only"]);
const OPERATION_STATUSES = new Set(["pending", "running", "succeeded", "failed", "cancelled"]);

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
  compactSnapshot(): Promise<NotificationStoredSnapshot>;
  history(query: NotificationHistoryQuery): Promise<NotificationHistoryPage>;
  getOperation(
    libraryId: string,
    operationId: string
  ): Promise<DesktopNotificationOperation | null>;
  publishThumbnailSummary(summary: ThumbnailCacheSummary): Promise<void>;
  recordRendererError(
    library: NotificationLibraryIdentity,
    report: RendererErrorReport,
    occurredAt: number,
    occurrenceIncrement?: number
  ): Promise<DesktopNotificationOperation>;
  hasLegacyImport(libraryId: string): Promise<boolean>;
  markLegacyImported(libraryId: string, importedAt: number): Promise<void>;
  close?(): Promise<void>;
}

export interface NotificationServiceOptions {
  store?: NotificationServiceStore;
  openStore?: () => Promise<NotificationServiceStore>;
  fetch?: typeof fetch;
  now?: () => number;
  retryDelay?: (delayMs: number, signal: AbortSignal) => Promise<void>;
}

interface ActiveLibrary {
  library: NotificationLibraryIdentity;
  session: NotificationCoreSession;
  controller: AbortController;
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
  occurredAt: number;
  occurrenceIncrement: number;
}

interface LegacyFileOperation {
  id: number;
  operation: "rename" | "move" | "delete_recycle" | "delete_permanent";
  sourcePath: string;
  targetPath: string | null;
  status: "succeeded" | "failed";
  createdAt: number;
  finishedAt: number | null;
  error: string | null;
}

interface LegacyFileOperationPage {
  items: LegacyFileOperation[];
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
  return { active: [], thumbnailSummaries: [], latestImportantResults: [] };
}

function assertSafeSequence(value: unknown, name: string): asserts value is number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) {
    throw new TypeError(`${name} must be a non-negative safe integer`);
  }
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

function cloneCompactSnapshot(snapshot: NotificationCompactSnapshot): NotificationCompactSnapshot {
  return {
    active: snapshot.active.map((item) => ({ ...item })),
    thumbnailSummaries: snapshot.thumbnailSummaries.map((item) => ({ ...item })),
    latestImportantResults: snapshot.latestImportantResults.map((item) => ({ ...item })),
    syncHealth: { ...snapshot.syncHealth }
  };
}

export class NotificationService {
  private store: NotificationServiceStore | null;
  private readonly openStore: (() => Promise<NotificationServiceStore>) | null;
  private readonly fetchImpl: typeof fetch;
  private readonly now: () => number;
  private readonly retryDelay: (delayMs: number, signal: AbortSignal) => Promise<void>;
  private active: ActiveLibrary | null = null;
  private closed = false;
  private storedSnapshot = emptyStoredSnapshot();
  private health: NotificationSyncHealth;
  private readonly listeners = new Set<(snapshot: NotificationCompactSnapshot) => void>();
  private readonly lastKnownCursors = new Map<string, number>();
  private readonly pendingAcknowledgements = new Map<string, number>();
  private readonly pendingOutbox = new Map<string, PendingOutboxBatch>();
  private readonly fallbackOperations = new Map<string, DesktopNotificationOperation>();
  private readonly fallbackThumbnails = new Map<string, ThumbnailCacheSummary>();
  private readonly fallbackLibraries = new Map<string, NotificationLibraryIdentity>();
  private readonly pendingRendererErrors = new Map<string, PendingRendererError>();
  private readonly pendingLocalThumbnails = new Map<string, ThumbnailCacheSummary>();
  private readonly legacyImports = new Map<string, Promise<void>>();
  private lifecycleTail: Promise<void> = Promise.resolve();

  constructor(options: NotificationServiceOptions) {
    this.store = options.store ?? null;
    this.openStore = options.openStore ?? null;
    this.fetchImpl = options.fetch ?? fetch;
    this.now = options.now ?? Date.now;
    this.retryDelay = options.retryDelay ?? abortableDelay;
    this.health = {
      state: this.store ? "healthy" : "degraded",
      libraryId: null,
      detail: this.store ? null : "Notification history store is unavailable",
      updatedAt: this.now()
    };
  }

  async initialize(): Promise<void> {
    if (!this.store && this.openStore) {
      try {
        this.store = await this.openStore();
      } catch (error) {
        this.setHealth("degraded", null, errorMessage(error), false);
      }
    }
    if (!this.store) return;
    try {
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
      await this.stopActiveLoop();
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
    const library = { id: report.libraryId, name: report.libraryName };
    this.fallbackLibraries.set(library.id, library);
    if (this.store) {
      try {
        await this.store.recordRendererError(library, report, this.now());
        await this.refreshStoredSnapshot();
        return;
      } catch (error) {
        await this.markStoreUnavailable(error);
      }
    }
    const value = this.recordFallbackRendererError(library, report);
    const key = libraryKey(library.id, value.operationId);
    const pending = this.pendingRendererErrors.get(key);
    this.pendingRendererErrors.set(key, {
      library,
      report,
      occurredAt: this.now(),
      occurrenceIncrement: (pending?.occurrenceIncrement ?? 0) + 1
    });
    this.emitSnapshot();
  }

  async publishThumbnailSummary(summary: ThumbnailCacheSummary): Promise<void> {
    const library = { id: summary.libraryId, name: summary.libraryName };
    this.fallbackLibraries.set(library.id, library);
    if (this.store) {
      try {
        await this.store.publishThumbnailSummary(summary);
        await this.refreshStoredSnapshot();
        return;
      } catch (error) {
        await this.markStoreUnavailable(error);
      }
    }
    this.fallbackThumbnails.set(summary.libraryId, summary);
    this.pendingLocalThumbnails.set(summary.libraryId, summary);
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
        delayMs = MIN_RETRY_MS;
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
    this.fallbackLibraries.set(library.id, library);

    const pending = this.pendingOutbox.get(library.id);
    if (!this.store && pending) {
      const recovered = await this.tryRecoverStore();
      if (!recovered) throw new Error("Notification history store is unavailable");
      void this.ensureLegacyImport(library, session, signal);
      const recoveredAck = this.pendingAcknowledgements.get(library.id);
      if (recoveredAck !== undefined && recoveredAck > 0) {
        await this.acknowledge(session, recoveredAck, signal);
        this.pendingAcknowledgements.delete(library.id);
      }
      return;
    }

    if (!this.store && (await this.tryRecoverStore())) {
      void this.ensureLegacyImport(library, session, signal);
    }
    const pendingAck = this.pendingAcknowledgements.get(library.id);
    if (pendingAck !== undefined && pendingAck > 0) {
      await this.acknowledge(session, pendingAck, signal);
      this.pendingAcknowledgements.delete(library.id);
      const acknowledgedBatch = this.pendingOutbox.get(library.id);
      if (acknowledgedBatch && acknowledgedBatch.maxSequence <= pendingAck) {
        this.pendingOutbox.delete(library.id);
      }
    }

    let cursor = this.lastKnownCursors.get(library.id) ?? 0;
    if (this.store) {
      try {
        cursor = await this.store.cursor(library.id);
        assertSafeSequence(cursor, "notification cursor");
        this.lastKnownCursors.set(library.id, cursor);
      } catch (error) {
        await this.markStoreUnavailable(error);
      }
    }

    const page = await this.listOutbox(session, cursor, signal);
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

    this.lastKnownCursors.set(library.id, page.maxSequence);
    this.pendingAcknowledgements.set(library.id, page.maxSequence);
    await this.refreshStoredSnapshot();
    if (!this.store) this.mergeFallbackEvents(library, page.events);
    await this.acknowledge(session, page.maxSequence, signal);
    this.pendingAcknowledgements.delete(library.id);
    if (this.store) {
      this.setHealth("healthy", library.id, null);
    } else {
      this.emitSnapshot();
    }
  }

  private async listOutbox(
    session: NotificationCoreSession,
    afterSequence: number,
    signal: AbortSignal
  ): Promise<{ events: NotificationStoreEvent[]; maxSequence: number }> {
    const query = new URLSearchParams({
      afterSequence: String(afterSequence),
      limit: String(OUTBOX_LIMIT),
      waitMs: String(OUTBOX_WAIT_MS)
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
    await this.requestJson(`${session.baseUrl}/notifications/outbox/ack`, session, {
      method: "POST",
      signal,
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ maxSequence })
    });
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
    if (await this.store.hasLegacyImport(library.id)) return;

    const cutoff = this.now() - LEGACY_RETENTION_MS;
    let cursor: string | null = null;
    while (!signal.aborted) {
      const query = new URLSearchParams({ limit: String(LEGACY_PAGE_LIMIT) });
      if (cursor) query.set("cursor", cursor);
      const body = await this.requestJson(`${session.baseUrl}/file-ops?${query}`, session, { signal });
      const page = this.parseLegacyPage(body);
      const eligible = page.items.filter((item) => item.createdAt >= cutoff);
      if (eligible.length > 0) {
        await this.store.upsertOperations(
          library,
          eligible.map((item) => this.legacyOperation(item))
        );
      }
      const reachedCutoff = page.items.some((item) => item.createdAt < cutoff);
      if (reachedCutoff || !page.nextCursor || page.items.length === 0) break;
      cursor = page.nextCursor;
    }
    if (signal.aborted) throw signal.reason;
    await this.store.markLegacyImported(library.id, this.now());
    await this.refreshStoredSnapshot();
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
      const item = value as Partial<LegacyFileOperation>;
      assertSafeSequence(item.id, "legacy file operation id");
      assertSafeSequence(item.createdAt, "legacy file operation createdAt");
      if (item.finishedAt !== null) assertSafeSequence(item.finishedAt, "legacy file operation finishedAt");
      if (!(["rename", "move", "delete_recycle", "delete_permanent"] as unknown[]).includes(item.operation)) {
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
      return item as LegacyFileOperation;
    });
    return { items, nextCursor: nextCursor ?? null };
  }

  private legacyOperation(item: LegacyFileOperation): NotificationOperationInput {
    const completedAt = item.finishedAt ?? item.createdAt;
    const labels: Record<LegacyFileOperation["operation"], string> = {
      rename: "Rename",
      move: "Move",
      delete_recycle: "Move to recycle bin",
      delete_permanent: "Delete permanently"
    };
    return {
      operationId: `legacy-file-op:${item.id}`,
      revision: 1,
      kind: item.operation,
      category: "file_operation",
      origin: "user",
      presentation: "important",
      status: item.status,
      phase: "completed",
      title: labels[item.operation],
      summary: item.targetPath ? `${item.sourcePath} -> ${item.targetPath}` : item.sourcePath,
      taskId: null,
      rootId: null,
      folderId: null,
      fileId: null,
      currentValue: 1,
      totalValue: 1,
      unit: "items",
      errorCode: item.status === "failed" ? "legacy_file_operation_failed" : null,
      errorDetail: item.error,
      cancellable: false,
      retryable: false,
      attempt: 1,
      occurrenceCount: 1,
      startedAt: item.createdAt,
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
    const operation = await this.findOperation(libraryId, operationId);
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
        { method: "POST" }
      );
      return { ok: true, message: null };
    } catch (error) {
      return { ok: false, message: errorMessage(error) };
    }
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

  private async tryRecoverStore(): Promise<boolean> {
    if (this.store) return true;
    if (!this.openStore) return false;
    let store: NotificationServiceStore | null = null;
    try {
      store = await this.openStore();
      await this.flushPendingLocalState(store);
      await this.flushPendingOutboxState(store);
      this.store = store;
      this.storedSnapshot = await store.compactSnapshot();
    } catch (error) {
      if (this.store === store) this.store = null;
      void store?.close?.().catch(() => undefined);
      this.setHealth("degraded", this.active?.library.id ?? null, errorMessage(error));
      return false;
    }
    this.clearFallbackFromDurableState();
    this.setHealth("healthy", this.active?.library.id ?? null, null);
    return true;
  }

  private async flushPendingLocalState(store: NotificationServiceStore): Promise<void> {
    while (this.pendingRendererErrors.size > 0) {
      const entry = this.pendingRendererErrors.entries().next().value as
        | [string, PendingRendererError]
        | undefined;
      if (!entry) break;
      const [key, pending] = entry;
      this.pendingRendererErrors.delete(key);
      try {
        await store.recordRendererError(
          pending.library,
          pending.report,
          pending.occurredAt,
          pending.occurrenceIncrement
        );
      } catch (error) {
        const newer = this.pendingRendererErrors.get(key);
        this.pendingRendererErrors.set(key, {
          ...pending,
          report: newer?.report ?? pending.report,
          occurredAt: Math.max(pending.occurredAt, newer?.occurredAt ?? pending.occurredAt),
          occurrenceIncrement:
            pending.occurrenceIncrement + (newer?.occurrenceIncrement ?? 0)
        });
        throw error;
      }
    }
    while (this.pendingLocalThumbnails.size > 0) {
      const entry = this.pendingLocalThumbnails.entries().next().value as
        | [string, ThumbnailCacheSummary]
        | undefined;
      if (!entry) break;
      const [key, summary] = entry;
      this.pendingLocalThumbnails.delete(key);
      try {
        await store.publishThumbnailSummary(summary);
      } catch (error) {
        const newer = this.pendingLocalThumbnails.get(key);
        if (!newer || summary.observedAt > newer.observedAt) {
          this.pendingLocalThumbnails.set(key, summary);
        }
        throw error;
      }
    }
  }

  private async flushPendingOutboxState(store: NotificationServiceStore): Promise<void> {
    const pending = [...this.pendingOutbox.values()];
    for (const batch of pending) {
      await store.applyOutboxBatch(batch.library, batch.events, batch.maxSequence);
    }
    for (const batch of pending) {
      this.lastKnownCursors.set(batch.library.id, batch.maxSequence);
      this.pendingAcknowledgements.set(batch.library.id, batch.maxSequence);
      this.pendingOutbox.delete(batch.library.id);
    }
  }

  private async refreshStoredSnapshot(): Promise<void> {
    if (!this.store) return;
    try {
      this.storedSnapshot = await this.store.compactSnapshot();
      this.emitSnapshot();
    } catch (error) {
      await this.markStoreUnavailable(error);
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
    for (const operation of [
      ...this.storedSnapshot.active,
      ...this.storedSnapshot.latestImportantResults
    ]) {
      this.upsertFallbackOperation(operation);
    }
    for (const summary of this.storedSnapshot.thumbnailSummaries) {
      this.fallbackThumbnails.set(summary.libraryId, summary);
      this.fallbackLibraries.set(summary.libraryId, {
        id: summary.libraryId,
        name: summary.libraryName
      });
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
    while (eventCount > FALLBACK_OPERATION_LIMIT) {
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
    report: RendererErrorReport
  ): DesktopNotificationOperation {
    const operationId = `renderer-error:${library.id}:${report.source}:${report.code}:${report.target}`;
    const key = libraryKey(library.id, operationId);
    const existing = this.fallbackOperations.get(key);
    const occurredAt = this.now();
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
    if (existing && operation.revision < existing.revision) return;
    this.fallbackOperations.delete(key);
    this.fallbackOperations.set(key, operation);
    while (this.fallbackOperations.size > FALLBACK_OPERATION_LIMIT) {
      const oldestCompleted = [...this.fallbackOperations.entries()].find(
        ([, item]) => item.status !== "pending" && item.status !== "running"
      );
      const keyToDelete = oldestCompleted?.[0] ?? this.fallbackOperations.keys().next().value;
      if (typeof keyToDelete !== "string") break;
      this.fallbackOperations.delete(keyToDelete);
      this.pendingRendererErrors.delete(keyToDelete);
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
      syncHealth: this.health
    };
  }

  private clearFallbackFromDurableState(): void {
    this.fallbackOperations.clear();
    this.fallbackThumbnails.clear();
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
