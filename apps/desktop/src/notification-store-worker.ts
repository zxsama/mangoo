import { parentPort, workerData } from "node:worker_threads";
import { DatabaseSync } from "node:sqlite";

import type {
  DesktopNotificationOperation,
  NotificationCompactSnapshot,
  NotificationHistoryPage,
  NotificationHistoryQuery,
  NotificationLibraryIdentity,
  NotificationOperationInput,
  NotificationRetentionResult,
  NotificationStoreRequest,
  NotificationStoreSerializedError,
  NotificationStoreWorkerMessage,
  ThumbnailCacheSummary
} from "./notification-types.js";

const SCHEMA_VERSION = 2;
const COMPLETED_RETENTION_LIMIT = 2_000;
const COMPLETED_RETENTION_MS = 30 * 24 * 60 * 60 * 1_000;
const ACTIVE_STATUSES = new Set(["pending", "running"]);
const OPERATION_STATUSES = new Set(["pending", "running", "succeeded", "failed", "cancelled"]);
const CATEGORIES = new Set(["task", "file_operation", "error", "thumbnail_cache"]);
const ORIGINS = new Set(["user", "background", "recovery", "system"]);
const PRESENTATIONS = new Set(["important", "history_only", "status_only"]);
const HISTORY_INDEX_SQL = `
  CREATE INDEX idx_operations_history
    ON operations(COALESCE(completed_at, updated_at) DESC, updated_at DESC)
    WHERE status NOT IN ('pending', 'running');
`;

interface WorkerOptions {
  databasePath: string;
}

interface OperationRow {
  library_id: string;
  library_name: string;
  payload_json: string;
}

function serializeError(error: unknown): NotificationStoreSerializedError {
  if (error instanceof Error) {
    const coded = error as Error & { code?: unknown };
    return {
      name: error.name,
      message: error.message,
      ...(typeof coded.code === "string" ? { code: coded.code } : {})
    };
  }
  return { name: "Error", message: String(error) };
}

function assertObject(value: unknown, label: string): asserts value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`${label} must be an object`);
  }
}

function assertString(value: unknown, label: string): asserts value is string {
  if (typeof value !== "string" || value.length === 0) {
    throw new TypeError(`${label} must be a non-empty string`);
  }
}

function assertNullableString(value: unknown, label: string): void {
  if (value !== null && typeof value !== "string") {
    throw new TypeError(`${label} must be a string or null`);
  }
}

function assertSafeInteger(value: unknown, label: string, minimum = 0): asserts value is number {
  if (!Number.isSafeInteger(value) || (value as number) < minimum) {
    throw new RangeError(`${label} must be a safe integer greater than or equal to ${minimum}`);
  }
}

function assertNullableSafeInteger(value: unknown, label: string, minimum = 0): void {
  if (value !== null) {
    assertSafeInteger(value, label, minimum);
  }
}

function validateLibrary(library: NotificationLibraryIdentity): void {
  assertObject(library, "library");
  assertString(library.id, "library.id");
  assertString(library.name, "library.name");
}

function validateOperation(operation: NotificationOperationInput): void {
  assertObject(operation, "operation");
  assertString(operation.operationId, "operation.operationId");
  assertSafeInteger(operation.revision, "operation.revision");
  assertString(operation.kind, "operation.kind");
  if (!CATEGORIES.has(operation.category)) {
    throw new TypeError("operation.category is invalid");
  }
  if (!ORIGINS.has(operation.origin)) {
    throw new TypeError("operation.origin is invalid");
  }
  if (!PRESENTATIONS.has(operation.presentation)) {
    throw new TypeError("operation.presentation is invalid");
  }
  if (!OPERATION_STATUSES.has(operation.status)) {
    throw new TypeError("operation.status is invalid");
  }
  assertNullableString(operation.phase, "operation.phase");
  assertString(operation.title, "operation.title");
  assertNullableString(operation.summary, "operation.summary");
  assertNullableSafeInteger(operation.taskId, "operation.taskId");
  assertNullableSafeInteger(operation.rootId, "operation.rootId");
  assertNullableSafeInteger(operation.folderId, "operation.folderId");
  assertNullableSafeInteger(operation.fileId, "operation.fileId");
  assertSafeInteger(operation.currentValue, "operation.currentValue");
  assertNullableSafeInteger(operation.totalValue, "operation.totalValue");
  assertNullableString(operation.unit, "operation.unit");
  assertNullableString(operation.errorCode, "operation.errorCode");
  assertNullableString(operation.errorDetail, "operation.errorDetail");
  if (typeof operation.cancellable !== "boolean" || typeof operation.retryable !== "boolean") {
    throw new TypeError("operation capabilities must be boolean");
  }
  assertSafeInteger(operation.attempt, "operation.attempt");
  assertSafeInteger(operation.occurrenceCount, "operation.occurrenceCount", 1);
  assertSafeInteger(operation.startedAt, "operation.startedAt");
  assertSafeInteger(operation.updatedAt, "operation.updatedAt");
  assertNullableSafeInteger(operation.completedAt, "operation.completedAt");
}

function validateThumbnailSummary(summary: ThumbnailCacheSummary): void {
  assertObject(summary, "thumbnail summary");
  assertString(summary.libraryId, "summary.libraryId");
  assertString(summary.libraryName, "summary.libraryName");
  assertSafeInteger(summary.observedAt, "summary.observedAt");
  assertSafeInteger(summary.cachedCount, "summary.cachedCount");
  assertSafeInteger(summary.totalCandidateCount, "summary.totalCandidateCount");
  assertSafeInteger(summary.missingCount, "summary.missingCount");
  assertSafeInteger(summary.staleCount, "summary.staleCount");
  assertSafeInteger(summary.failedCount, "summary.failedCount");
  assertSafeInteger(summary.pendingCount, "summary.pendingCount");
  assertSafeInteger(summary.activeBulkTaskCount, "summary.activeBulkTaskCount");
  assertNullableSafeInteger(summary.totalCacheBytes, "summary.totalCacheBytes");
  if (
    summary.completionPercent !== null &&
    (!Number.isFinite(summary.completionPercent) ||
      summary.completionPercent < 0 ||
      summary.completionPercent > 100)
  ) {
    throw new RangeError("summary.completionPercent must be between 0 and 100 or null");
  }
}

function openDatabase(databasePath: string): DatabaseSync {
  assertString(databasePath, "databasePath");
  const database = new DatabaseSync(databasePath);
  database.exec("PRAGMA busy_timeout = 5000;");
  database.exec("PRAGMA journal_mode = WAL;");
  database.exec("PRAGMA synchronous = NORMAL;");

  const versionRow = database.prepare("PRAGMA user_version").get() as
    | { user_version?: unknown }
    | undefined;
  const version = Number(versionRow?.user_version ?? 0);
  if (!Number.isSafeInteger(version) || version < 0 || version > SCHEMA_VERSION) {
    database.close();
    throw new Error(`Unsupported notification database schema version: ${version}`);
  }
  if (version === 0) {
    database.exec("BEGIN IMMEDIATE;");
    try {
      database.exec(`
        CREATE TABLE operations (
          library_id TEXT NOT NULL,
          operation_id TEXT NOT NULL,
          library_name TEXT NOT NULL,
          revision INTEGER NOT NULL,
          status TEXT NOT NULL,
          category TEXT NOT NULL,
          presentation TEXT NOT NULL,
          updated_at INTEGER NOT NULL,
          completed_at INTEGER,
          payload_json TEXT NOT NULL,
          PRIMARY KEY(library_id, operation_id)
        );
        ${HISTORY_INDEX_SQL}
        CREATE INDEX idx_operations_status
          ON operations(status, updated_at DESC);
        CREATE TABLE sync_cursors (
          library_id TEXT PRIMARY KEY,
          sequence INTEGER NOT NULL
        );
        CREATE TABLE thumbnail_summaries (
          library_id TEXT PRIMARY KEY,
          library_name TEXT NOT NULL,
          observed_at INTEGER NOT NULL,
          payload_json TEXT NOT NULL
        );
        CREATE TABLE legacy_imports (
          library_id TEXT PRIMARY KEY,
          imported_at INTEGER NOT NULL
        );
        PRAGMA user_version = ${SCHEMA_VERSION};
      `);
      database.exec("COMMIT;");
    } catch (error) {
      database.exec("ROLLBACK;");
      database.close();
      throw error;
    }
  }
  if (version === 1) {
    database.exec("BEGIN IMMEDIATE;");
    try {
      database.exec("DROP INDEX IF EXISTS idx_operations_history;");
      database.exec(HISTORY_INDEX_SQL);
      database.exec(`PRAGMA user_version = ${SCHEMA_VERSION};`);
      database.exec("COMMIT;");
    } catch (error) {
      database.exec("ROLLBACK;");
      database.close();
      throw error;
    }
  }
  return database;
}

class NotificationStoreDatabase {
  private readonly database: DatabaseSync;

  constructor(database: DatabaseSync) {
    this.database = database;
  }

  applyOutboxBatch(
    library: NotificationLibraryIdentity,
    events: Extract<NotificationStoreRequest, { method: "applyOutboxBatch" }>["params"]["events"],
    maxSequence: number
  ): void;
  applyOutboxBatch(
    library: NotificationLibraryIdentity,
    events: Extract<NotificationStoreRequest, { method: "applyOutboxBatch" }>["params"]["events"],
    maxSequence: number
  ): void {
    validateLibrary(library);
    assertSafeInteger(maxSequence, "maxSequence");
    if (!Array.isArray(events)) {
      throw new TypeError("events must be an array");
    }

    let previousSequence = -1;
    for (const item of events) {
      assertObject(item, "event");
      assertSafeInteger(item.sequence, "event.sequence");
      if (item.sequence <= previousSequence) {
        throw new RangeError("event sequences must be strictly increasing");
      }
      if (item.sequence > maxSequence) {
        throw new RangeError("event.sequence cannot exceed maxSequence");
      }
      previousSequence = item.sequence;
      validateOperation(item.operation);
    }

    const existingStatement = this.database.prepare(
      "SELECT revision, payload_json FROM operations WHERE library_id = ? AND operation_id = ?"
    );
    const insertStatement = this.database.prepare(`
      INSERT INTO operations (
        library_id, operation_id, library_name, revision, status, category,
        presentation, updated_at, completed_at, payload_json
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
      ON CONFLICT(library_id, operation_id) DO UPDATE SET
        library_name = excluded.library_name,
        revision = excluded.revision,
        status = excluded.status,
        category = excluded.category,
        presentation = excluded.presentation,
        updated_at = excluded.updated_at,
        completed_at = excluded.completed_at,
        payload_json = excluded.payload_json
      WHERE excluded.revision > operations.revision
    `);
    const cursorStatement = this.database.prepare(`
      INSERT INTO sync_cursors (library_id, sequence) VALUES (?, ?)
      ON CONFLICT(library_id) DO UPDATE SET sequence = MAX(sync_cursors.sequence, excluded.sequence)
    `);

    this.database.exec("BEGIN IMMEDIATE;");
    try {
      for (const item of events) {
        const payloadJson = JSON.stringify(item.operation);
        const existing = existingStatement.get(library.id, item.operation.operationId) as
          | { revision: number; payload_json: string }
          | undefined;
        if (existing && item.operation.revision < existing.revision) {
          continue;
        }
        if (existing && item.operation.revision === existing.revision) {
          if (existing.payload_json !== payloadJson) {
            throw new Error(
              `Conflicting payload for ${library.id}/${item.operation.operationId} revision ${item.operation.revision}`
            );
          }
          continue;
        }
        insertStatement.run(
          library.id,
          item.operation.operationId,
          library.name,
          item.operation.revision,
          item.operation.status,
          item.operation.category,
          item.operation.presentation,
          item.operation.updatedAt,
          item.operation.completedAt,
          payloadJson
        );
      }
      cursorStatement.run(library.id, maxSequence);
      this.database.exec("COMMIT;");
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
  }

  cursor(libraryId: string): number {
    assertString(libraryId, "libraryId");
    const row = this.database
      .prepare("SELECT sequence FROM sync_cursors WHERE library_id = ?")
      .get(libraryId) as { sequence: number } | undefined;
    return row?.sequence ?? 0;
  }

  compactSnapshot(): NotificationCompactSnapshot {
    const active = this.database
      .prepare(
        `SELECT library_id, library_name, payload_json
         FROM operations
         WHERE status IN ('pending', 'running')
         ORDER BY updated_at DESC, library_id ASC, operation_id ASC`
      )
      .all() as unknown as OperationRow[];
    const latestImportantResults = this.database
      .prepare(
        `SELECT library_id, library_name, payload_json
         FROM operations
         WHERE status NOT IN ('pending', 'running') AND presentation = 'important'
         ORDER BY COALESCE(completed_at, updated_at) DESC, updated_at DESC,
                  library_id ASC, operation_id ASC
         LIMIT 3`
      )
      .all() as unknown as OperationRow[];
    const thumbnailRows = this.database
      .prepare(
        `SELECT payload_json FROM thumbnail_summaries
         ORDER BY library_name COLLATE NOCASE ASC, library_id ASC`
      )
      .all() as unknown as Array<{ payload_json: string }>;
    return {
      active: active.map(rowToOperation),
      thumbnailSummaries: thumbnailRows.map((row) => JSON.parse(row.payload_json) as ThumbnailCacheSummary),
      latestImportantResults: latestImportantResults.map(rowToOperation)
    };
  }

  history(query: NotificationHistoryQuery): NotificationHistoryPage {
    assertObject(query, "history query");
    const limit = query.limit ?? 50;
    const offset = query.offset ?? 0;
    assertSafeInteger(limit, "history limit", 1);
    assertSafeInteger(offset, "history offset");
    if (limit > 200) {
      throw new RangeError("history limit cannot exceed 200");
    }

    const clauses = ["status NOT IN ('pending', 'running')"];
    const bindings: Array<string | number> = [];
    if (query.libraryId !== undefined) {
      assertString(query.libraryId, "history libraryId");
      clauses.push("library_id = ?");
      bindings.push(query.libraryId);
    }
    if (query.statuses !== undefined) {
      if (!Array.isArray(query.statuses) || query.statuses.length === 0) {
        throw new TypeError("history statuses must be a non-empty array");
      }
      for (const status of query.statuses) {
        if (!OPERATION_STATUSES.has(status)) {
          throw new TypeError("history status is invalid");
        }
      }
      clauses.push(`status IN (${query.statuses.map(() => "?").join(", ")})`);
      bindings.push(...query.statuses);
    }
    if (query.categories !== undefined) {
      if (!Array.isArray(query.categories) || query.categories.length === 0) {
        throw new TypeError("history categories must be a non-empty array");
      }
      for (const category of query.categories) {
        if (!CATEGORIES.has(category)) {
          throw new TypeError("history category is invalid");
        }
      }
      clauses.push(`category IN (${query.categories.map(() => "?").join(", ")})`);
      bindings.push(...query.categories);
    }
    if (query.search !== undefined) {
      if (typeof query.search !== "string") {
        throw new TypeError("history search must be a string");
      }
      const search = query.search.trim();
      if (search.length > 0) {
        clauses.push("(json_extract(payload_json, '$.title') LIKE ? OR json_extract(payload_json, '$.summary') LIKE ?)");
        bindings.push(`%${search}%`, `%${search}%`);
      }
    }

    const where = clauses.join(" AND ");
    const totalRow = this.database
      .prepare(`SELECT COUNT(*) AS total FROM operations WHERE ${where}`)
      .get(...bindings) as { total: number };
    const rows = this.database
      .prepare(
        `SELECT library_id, library_name, payload_json
         FROM operations WHERE ${where}
         ORDER BY COALESCE(completed_at, updated_at) DESC, updated_at DESC,
                  library_id ASC, operation_id ASC
         LIMIT ? OFFSET ?`
      )
      .all(...bindings, limit, offset) as unknown as OperationRow[];
    const nextOffset = offset + rows.length < totalRow.total ? offset + rows.length : null;
    return { items: rows.map(rowToOperation), total: totalRow.total, nextOffset };
  }

  publishThumbnailSummary(summary: ThumbnailCacheSummary): void {
    validateThumbnailSummary(summary);
    const payloadJson = JSON.stringify(summary);
    this.database
      .prepare(
        `INSERT INTO thumbnail_summaries (library_id, library_name, observed_at, payload_json)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(library_id) DO UPDATE SET
           library_name = excluded.library_name,
           observed_at = excluded.observed_at,
           payload_json = excluded.payload_json
         WHERE excluded.observed_at >= thumbnail_summaries.observed_at`
      )
      .run(summary.libraryId, summary.libraryName, summary.observedAt, payloadJson);
  }

  hasLegacyImport(libraryId: string): boolean {
    assertString(libraryId, "libraryId");
    return Boolean(
      this.database.prepare("SELECT 1 FROM legacy_imports WHERE library_id = ?").get(libraryId)
    );
  }

  markLegacyImported(libraryId: string, importedAt: number): void {
    assertString(libraryId, "libraryId");
    assertSafeInteger(importedAt, "importedAt");
    this.database
      .prepare(
        `INSERT INTO legacy_imports (library_id, imported_at) VALUES (?, ?)
         ON CONFLICT(library_id) DO NOTHING`
      )
      .run(libraryId, importedAt);
  }

  cleanupRetention(now: number): NotificationRetentionResult {
    assertSafeInteger(now, "retention now");
    const cutoff = now - COMPLETED_RETENTION_MS;
    this.database.exec("BEGIN IMMEDIATE;");
    try {
      const expired = this.database
        .prepare(
          `DELETE FROM operations
           WHERE status NOT IN ('pending', 'running')
             AND COALESCE(completed_at, updated_at) < ?`
        )
        .run(cutoff);
      const overflow = this.database
        .prepare(
          `DELETE FROM operations
           WHERE rowid IN (
             SELECT rowid FROM operations
             WHERE status NOT IN ('pending', 'running')
             ORDER BY COALESCE(completed_at, updated_at) DESC, updated_at DESC,
                      library_id ASC, operation_id ASC
             LIMIT -1 OFFSET ${COMPLETED_RETENTION_LIMIT}
           )`
        )
        .run();
      this.database.exec("COMMIT;");
      return {
        deletedExpired: Number(expired.changes),
        deletedOverflow: Number(overflow.changes)
      };
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
  }

  close(): void {
    this.database.close();
  }
}

function rowToOperation(row: OperationRow): DesktopNotificationOperation {
  return {
    ...(JSON.parse(row.payload_json) as NotificationOperationInput),
    libraryId: row.library_id,
    libraryName: row.library_name
  };
}

function executeRequest(store: NotificationStoreDatabase, request: NotificationStoreRequest): unknown {
  switch (request.method) {
    case "applyOutboxBatch":
      store.applyOutboxBatch(
        request.params.library,
        request.params.events,
        request.params.maxSequence
      );
      return null;
    case "cursor":
      return store.cursor(request.params.libraryId);
    case "compactSnapshot":
      return store.compactSnapshot();
    case "history":
      return store.history(request.params.query);
    case "publishThumbnailSummary":
      store.publishThumbnailSummary(request.params.summary);
      return null;
    case "hasLegacyImport":
      return store.hasLegacyImport(request.params.libraryId);
    case "markLegacyImported":
      store.markLegacyImported(request.params.libraryId, request.params.importedAt);
      return null;
    case "cleanupRetention":
      return store.cleanupRetention(request.params.now);
    case "close":
      return null;
  }
}

const REQUEST_METHODS = new Set<NotificationStoreRequest["method"]>([
  "applyOutboxBatch",
  "cursor",
  "compactSnapshot",
  "history",
  "publishThumbnailSummary",
  "hasLegacyImport",
  "markLegacyImported",
  "cleanupRetention",
  "close"
]);

function requestIdFromMessage(message: unknown): string | null {
  if (!message || typeof message !== "object" || Array.isArray(message)) {
    return null;
  }
  const requestId = (message as { requestId?: unknown }).requestId;
  return typeof requestId === "string" && requestId.length > 0 ? requestId : null;
}

function validateRequestMessage(
  message: unknown
): { request: NotificationStoreRequest } | { requestId: string | null; message: string } {
  const requestId = requestIdFromMessage(message);
  if (!requestId) {
    return {
      requestId: null,
      message: "Ignoring notification store message without a correlatable requestId"
    };
  }
  if (!message || typeof message !== "object" || Array.isArray(message)) {
    return { requestId, message: "notification store request must be an object" };
  }
  const method = (message as { method?: unknown }).method;
  if (typeof method !== "string") {
    return { requestId, message: "notification store request method must be a string" };
  }
  if (!REQUEST_METHODS.has(method as NotificationStoreRequest["method"])) {
    return { requestId, message: `unknown notification store method: ${method}` };
  }
  return { request: message as NotificationStoreRequest };
}

function postMessageSafely(message: NotificationStoreWorkerMessage): void {
  if (!port) {
    console.error("[notification-store-worker] parent port is unavailable");
    return;
  }
  try {
    port.postMessage(message);
  } catch (error) {
    console.error("[notification-store-worker] failed to post response", serializeError(error));
  }
}

function postProtocolError(requestId: string, message: string): void {
  postMessageSafely({
    type: "response",
    requestId,
    ok: false,
    error: { name: "NotificationStoreProtocolError", message }
  });
}

const port = parentPort;
if (!port) {
  throw new Error("notification store worker requires a parent port");
}

let store: NotificationStoreDatabase | null = null;
try {
  const options = workerData as WorkerOptions;
  assertObject(options, "workerData");
  store = new NotificationStoreDatabase(openDatabase(options.databasePath));
  postMessageSafely({ type: "ready" });
  port.on("message", (message: unknown) => {
    const validated = validateRequestMessage(message);
    if ("message" in validated) {
      if (validated.requestId) {
        postProtocolError(validated.requestId, validated.message);
      } else {
        console.error("[notification-store-worker]", validated.message);
      }
      return;
    }
    const request = validated.request;
    if (!store) {
      postProtocolError(request.requestId, "notification store worker is closing");
      return;
    }
    try {
      if (request.method === "close") {
        store.close();
        store = null;
        postMessageSafely({
          type: "response",
          requestId: request.requestId,
          ok: true,
          result: null
        });
        port.close();
        return;
      }
      const result = executeRequest(store, request);
      postMessageSafely({
        type: "response",
        requestId: request.requestId,
        ok: true,
        result
      });
    } catch (error) {
      postMessageSafely({
        type: "response",
        requestId: request.requestId,
        ok: false,
        error: serializeError(error)
      });
    }
  });
} catch (error) {
  postMessageSafely({
    type: "startupError",
    error: serializeError(error)
  });
  port.close();
}
