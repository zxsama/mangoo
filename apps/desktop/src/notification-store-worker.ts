import { parentPort, workerData } from "node:worker_threads";
import { DatabaseSync } from "node:sqlite";

import {
  legacyRendererErrorOperationId,
  rendererErrorOperationId,
  type RendererErrorIdentity
} from "@megle/notification-contract";

import type {
  DesktopNotificationOperation,
  NotificationHistoryFacets,
  NotificationHistoryPage,
  NotificationHistoryQuery,
  NotificationLocalRecoveryBatch,
  NotificationLibraryIdentity,
  NotificationOperationInput,
  NotificationRendererErrorRecordResult,
  NotificationRetentionResult,
  NotificationRetryableFailureReference,
  NotificationStoreRequest,
  NotificationStoreSerializedError,
  NotificationStoredSnapshot,
  NotificationSyncState,
  NotificationStoreWorkerMessage,
  RendererErrorReport,
  ThumbnailCacheSummary
} from "./notification-types.js";

const SCHEMA_VERSION = 4;
const SQLITE_BUSY_TIMEOUT_MS = 5_000;
const SQLITE_INITIALIZATION_RETRY_DELAYS_MS = [2, 4, 8, 16, 32, 64, 128, 256] as const;
const SQLITE_RETRY_SLEEP = new Int32Array(new SharedArrayBuffer(Int32Array.BYTES_PER_ELEMENT));
const COMPLETED_RETENTION_LIMIT = 2_000;
const COMPLETED_RETENTION_MS = 30 * 24 * 60 * 60 * 1_000;
const OPERATION_STATUSES = new Set(["pending", "running", "succeeded", "failed", "cancelled"]);
const CATEGORIES = new Set(["task", "file_operation", "error", "thumbnail_cache"]);
const ORIGINS = new Set(["user", "background", "recovery", "system"]);
const PRESENTATIONS = new Set(["important", "history_only", "status_only"]);
const HISTORY_INDEX_SQL = `
  CREATE INDEX IF NOT EXISTS idx_operations_history
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
  action_error_detail?: string | null;
}

interface StoredRendererErrorIdentity extends RendererErrorIdentity {
  libraryId: string;
}

type StoredNotificationOperation = NotificationOperationInput & {
  rendererErrorIdentity?: StoredRendererErrorIdentity;
};

function verifiedLegacyRendererOperation(
  row: OperationRow,
  legacyOperationId: string,
  canonicalOperationId: string
): DesktopNotificationOperation | null {
  try {
    const stored = JSON.parse(row.payload_json) as StoredNotificationOperation;
    const identity = stored.rendererErrorIdentity;
    if (
      !identity ||
      typeof identity.libraryId !== "string" ||
      typeof identity.source !== "string" ||
      typeof identity.code !== "string" ||
      typeof identity.target !== "string" ||
      identity.libraryId !== row.library_id ||
      stored.operationId !== legacyOperationId ||
      stored.kind !== "renderer_error" ||
      stored.errorCode !== identity.code ||
      legacyRendererErrorOperationId(identity.libraryId, identity) !== legacyOperationId ||
      rendererErrorOperationId(identity.libraryId, identity) !== canonicalOperationId
    ) {
      return null;
    }
    return rowToOperation(row);
  } catch {
    return null;
  }
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

function escapeSqlLikeLiteral(value: string): string {
  return value.replaceAll("\\", "\\\\").replaceAll("%", "\\%").replaceAll("_", "\\_");
}

function rendererReportMarkerId(reportId: string): string {
  return `renderer-report:${reportId}`;
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
  const details = summary.failureDetails ?? [];
  const truncated = summary.failureDetailsTruncated === true;
  const unavailable = summary.failureDetailsUnavailable === true;
  if (typeof summary.failureDetailsUnavailable !== "boolean") {
    throw new TypeError("summary.failureDetailsUnavailable must be a boolean");
  }
  if (!Array.isArray(details) || details.length > 100) {
    throw new RangeError("summary.failureDetails must contain at most 100 entries");
  }
  for (const detail of details) {
    assertObject(detail, "thumbnail failure detail");
    assertString(detail.libraryId, "thumbnail failure detail.libraryId");
    if (detail.libraryId !== summary.libraryId) {
      throw new RangeError("thumbnail failure detail library must match summary library");
    }
    assertSafeInteger(detail.fileId, "thumbnail failure detail.fileId", 1);
    assertSafeInteger(detail.folderId, "thumbnail failure detail.folderId", 1);
    assertSafeInteger(detail.rootId, "thumbnail failure detail.rootId", 1);
    assertString(detail.relativePath, "thumbnail failure detail.relativePath");
    assertString(detail.failureKind, "thumbnail failure detail.failureKind");
    if (typeof detail.error !== "string") throw new TypeError("thumbnail failure detail.error must be a string");
    assertSafeInteger(detail.updatedAt, "thumbnail failure detail.updatedAt");
  }
  if (unavailable) {
    if (summary.failedCount <= 0 || details.length !== 0 || truncated) {
      throw new RangeError("unavailable failure details require a positive count without detail rows");
    }
    return;
  }
  if (summary.failedCount === 0 && (details.length !== 0 || truncated)) {
    throw new RangeError("zero failedCount requires an empty untruncated failure snapshot");
  }
  if (
    summary.failedCount > 0 &&
    truncated &&
    (details.length !== 100 || summary.failedCount <= details.length)
  ) {
    throw new RangeError("truncated failureDetails must contain the first 100 of a larger total");
  }
  if (summary.failedCount > 0 && !truncated && details.length !== summary.failedCount) {
    throw new RangeError("untruncated failureDetails must match failedCount");
  }
}

function normalizeThumbnailSummary(summary: ThumbnailCacheSummary): ThumbnailCacheSummary {
  const detailsFieldMissing = !Object.prototype.hasOwnProperty.call(summary, "failureDetails");
  const details = Array.isArray(summary.failureDetails) ? summary.failureDetails : [];
  const legacyTruncationFieldMissing = !Object.prototype.hasOwnProperty.call(
    summary,
    "failureDetailsTruncated"
  );
  const legacyUnavailableFieldMissing = !Object.prototype.hasOwnProperty.call(
    summary,
    "failureDetailsUnavailable"
  );
  const truncated =
    summary.failureDetailsTruncated === true ||
    (legacyTruncationFieldMissing &&
      summary.failedCount > details.length &&
      details.length === 100);
  const unavailable = summary.failureDetailsUnavailable === true ||
    (detailsFieldMissing &&
      legacyTruncationFieldMissing &&
      legacyUnavailableFieldMissing &&
      summary.failedCount > 0);
  const hasConsistentFailureSnapshot =
    !unavailable && (
      (summary.failedCount === 0 && details.length === 0 && !truncated) ||
      (summary.failedCount > 0 && !truncated && details.length === summary.failedCount) ||
      (summary.failedCount > details.length && details.length === 100 && truncated)
    );
  const hasUnavailableFailureSnapshot =
    unavailable && summary.failedCount > 0 && details.length === 0 && !truncated;
  const preserveFailureCount = hasConsistentFailureSnapshot || hasUnavailableFailureSnapshot;
  return {
    ...summary,
    failedCount: preserveFailureCount ? summary.failedCount : 0,
    failureDetails: hasConsistentFailureSnapshot ? details : [],
    failureDetailsTruncated: hasConsistentFailureSnapshot ? truncated : false,
    failureDetailsUnavailable: hasUnavailableFailureSnapshot
  };
}

function readSchemaVersion(database: DatabaseSync): number {
  const versionRow = database.prepare("PRAGMA user_version").get() as
    | { user_version?: unknown }
    | undefined;
  const version = Number(versionRow?.user_version ?? 0);
  if (!Number.isSafeInteger(version) || version < 0 || version > SCHEMA_VERSION) {
    throw new Error(`Unsupported notification database schema version: ${version}`);
  }
  return version;
}

function withImmediateTransaction(database: DatabaseSync, operation: () => void): void {
  database.exec("BEGIN IMMEDIATE;");
  try {
    operation();
    database.exec("COMMIT;");
  } catch (error) {
    try {
      database.exec("ROLLBACK;");
    } catch {
      // Preserve the operation error; the outer initializer owns retry/close.
    }
    throw error;
  }
}

function initializeSchema(database: DatabaseSync): void {
  const observedVersion = readSchemaVersion(database);
  if (observedVersion === SCHEMA_VERSION) return;

  withImmediateTransaction(database, () => {
    // Another worker may have completed initialization while this worker
    // waited for the write lock, so schema decisions must use the locked view.
    const lockedVersion = readSchemaVersion(database);
    if (lockedVersion === 0) {
      database.exec(`
        CREATE TABLE IF NOT EXISTS operations (
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
        CREATE INDEX IF NOT EXISTS idx_operations_status
          ON operations(status, updated_at DESC);
        CREATE TABLE IF NOT EXISTS sync_cursors (
          library_id TEXT PRIMARY KEY,
          persisted_sequence INTEGER NOT NULL,
          acked_sequence INTEGER NOT NULL DEFAULT 0,
          CHECK(acked_sequence <= persisted_sequence)
        );
        CREATE TABLE IF NOT EXISTS action_errors (
          library_id TEXT NOT NULL,
          operation_id TEXT NOT NULL,
          operation_revision INTEGER NOT NULL,
          detail TEXT NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(library_id, operation_id),
          FOREIGN KEY(library_id, operation_id)
            REFERENCES operations(library_id, operation_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS thumbnail_summaries (
          library_id TEXT PRIMARY KEY,
          library_name TEXT NOT NULL,
          observed_at INTEGER NOT NULL,
          payload_json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS legacy_imports (
          library_id TEXT PRIMARY KEY,
          imported_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS local_recovery_batches (
          batch_id TEXT PRIMARY KEY,
          applied_at INTEGER NOT NULL
        );
      `);
    }
    if (lockedVersion > 0 && lockedVersion < SCHEMA_VERSION) {
      if (lockedVersion === 1) {
        database.exec("DROP INDEX IF EXISTS idx_operations_history;");
        database.exec(HISTORY_INDEX_SQL);
      }
      if (lockedVersion < 3) {
        database.exec(`
          ALTER TABLE sync_cursors RENAME TO sync_cursors_legacy;
          CREATE TABLE sync_cursors (
            library_id TEXT PRIMARY KEY,
            persisted_sequence INTEGER NOT NULL,
            acked_sequence INTEGER NOT NULL DEFAULT 0,
            CHECK(acked_sequence <= persisted_sequence)
          );
          INSERT INTO sync_cursors(library_id, persisted_sequence, acked_sequence)
            SELECT library_id, sequence, 0 FROM sync_cursors_legacy;
          DROP TABLE sync_cursors_legacy;
          CREATE TABLE IF NOT EXISTS action_errors (
            library_id TEXT NOT NULL,
            operation_id TEXT NOT NULL,
            operation_revision INTEGER NOT NULL,
            detail TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(library_id, operation_id),
            FOREIGN KEY(library_id, operation_id)
              REFERENCES operations(library_id, operation_id) ON DELETE CASCADE
          );
        `);
      }
      if (lockedVersion < 4) {
        database.exec(`
          CREATE TABLE IF NOT EXISTS local_recovery_batches (
            batch_id TEXT PRIMARY KEY,
            applied_at INTEGER NOT NULL
          );
        `);
      }
    }

    database.exec(`PRAGMA user_version = ${SCHEMA_VERSION};`);
  });
}

function isSqliteBusyOrLocked(error: unknown): boolean {
  if (!error || typeof error !== "object") return false;
  const sqliteError = error as { code?: unknown; errcode?: unknown; message?: unknown };
  if (sqliteError.code === "SQLITE_BUSY" || sqliteError.code === "SQLITE_LOCKED") return true;
  if (sqliteError.errcode === 5 || sqliteError.errcode === 6) return true;
  return (
    typeof sqliteError.message === "string" &&
    /\b(?:database(?: table| schema)? is locked|database is busy)\b/i.test(sqliteError.message)
  );
}

function initializeWithBusyRetry(database: DatabaseSync): void {
  for (let attempt = 0; ; attempt += 1) {
    try {
      const journalMode = database.prepare("PRAGMA journal_mode = WAL").get() as
        | { journal_mode?: unknown }
        | undefined;
      if (String(journalMode?.journal_mode ?? "").toLowerCase() !== "wal") {
        throw new Error("Notification database failed to enter WAL journal mode");
      }
      database.exec("PRAGMA synchronous = NORMAL;");
      initializeSchema(database);
      database.exec("PRAGMA foreign_keys = ON;");
      const verifiedMode = database.prepare("PRAGMA journal_mode").get() as
        | { journal_mode?: unknown }
        | undefined;
      if (String(verifiedMode?.journal_mode ?? "").toLowerCase() !== "wal") {
        throw new Error("Notification database journal mode is not WAL after initialization");
      }
      return;
    } catch (error) {
      const delayMs = SQLITE_INITIALIZATION_RETRY_DELAYS_MS[attempt];
      if (!isSqliteBusyOrLocked(error) || delayMs === undefined) throw error;
      Atomics.wait(SQLITE_RETRY_SLEEP, 0, 0, delayMs);
    }
  }
}

function openDatabase(databasePath: string): DatabaseSync {
  assertString(databasePath, "databasePath");
  const database = new DatabaseSync(databasePath);
  try {
    database.exec(`PRAGMA busy_timeout = ${SQLITE_BUSY_TIMEOUT_MS};`);
    initializeWithBusyRetry(database);
    return database;
  } catch (error) {
    database.close();
    throw error;
  }
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

    const cursorStatement = this.database.prepare(`
      INSERT INTO sync_cursors (library_id, persisted_sequence, acked_sequence) VALUES (?, ?, 0)
      ON CONFLICT(library_id) DO UPDATE SET
        persisted_sequence = MAX(sync_cursors.persisted_sequence, excluded.persisted_sequence)
    `);

    this.database.exec("BEGIN IMMEDIATE;");
    try {
      this.upsertOperationRows(
        library,
        events.map((item) => item.operation)
      );
      cursorStatement.run(library.id, maxSequence);
      this.database.exec("COMMIT;");
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
  }

  upsertOperations(
    library: NotificationLibraryIdentity,
    operations: NotificationOperationInput[]
  ): void {
    validateLibrary(library);
    if (!Array.isArray(operations)) {
      throw new TypeError("operations must be an array");
    }
    for (const operation of operations) validateOperation(operation);

    this.database.exec("BEGIN IMMEDIATE;");
    try {
      this.upsertOperationRows(library, operations);
      this.database.exec("COMMIT;");
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
  }

  private upsertOperationRows(
    library: NotificationLibraryIdentity,
    operations: NotificationOperationInput[]
  ): void {
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
    const clearStaleActionError = this.database.prepare(`
      DELETE FROM action_errors
      WHERE library_id = ? AND operation_id = ? AND operation_revision < ?
    `);
    for (const operation of operations) {
      const payloadJson = JSON.stringify(operation);
      const existing = existingStatement.get(library.id, operation.operationId) as
        | { revision: number; payload_json: string }
        | undefined;
      if (existing && operation.revision < existing.revision) continue;
      if (existing && operation.revision === existing.revision) {
        if (existing.payload_json !== payloadJson) {
          throw new Error(
            `Conflicting payload for ${library.id}/${operation.operationId} revision ${operation.revision}`
          );
        }
        continue;
      }
      insertStatement.run(
        library.id,
        operation.operationId,
        library.name,
        operation.revision,
        operation.status,
        operation.category,
        operation.presentation,
        operation.updatedAt,
        operation.completedAt,
        payloadJson
      );
      clearStaleActionError.run(library.id, operation.operationId, operation.revision);
    }
  }

  cursor(libraryId: string): number {
    return this.syncState(libraryId).persistedSequence;
  }

  syncState(libraryId: string): NotificationSyncState {
    assertString(libraryId, "libraryId");
    const row = this.database
      .prepare(
        `SELECT persisted_sequence, acked_sequence
         FROM sync_cursors WHERE library_id = ?`
      )
      .get(libraryId) as
      | { persisted_sequence: number; acked_sequence: number }
      | undefined;
    return {
      persistedSequence: row?.persisted_sequence ?? 0,
      ackedSequence: row?.acked_sequence ?? 0
    };
  }

  markAcknowledged(libraryId: string, sequence: number): void {
    assertString(libraryId, "libraryId");
    assertSafeInteger(sequence, "acknowledged sequence");
    this.database.exec("BEGIN IMMEDIATE;");
    try {
      const state = this.syncState(libraryId);
      if (sequence > state.persistedSequence) {
        throw new RangeError("acknowledged sequence cannot exceed persisted sequence");
      }
      this.database
        .prepare(
          `UPDATE sync_cursors
           SET acked_sequence = MAX(acked_sequence, ?)
           WHERE library_id = ?`
        )
        .run(sequence, libraryId);
      this.database.exec("COMMIT;");
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
  }

  compactSnapshot(): NotificationStoredSnapshot {
    const active = this.database
      .prepare(
        `SELECT operations.library_id, operations.library_name, operations.payload_json,
                action_errors.detail AS action_error_detail
         FROM operations
         LEFT JOIN action_errors USING(library_id, operation_id)
         WHERE operations.status IN ('pending', 'running')
         ORDER BY operations.updated_at DESC, operations.library_id ASC,
                  operations.operation_id ASC`
      )
      .all() as unknown as OperationRow[];
    const latestImportantResults = this.database
      .prepare(
        `SELECT operations.library_id, operations.library_name, operations.payload_json,
                action_errors.detail AS action_error_detail
         FROM operations
         LEFT JOIN action_errors USING(library_id, operation_id)
         WHERE operations.status NOT IN ('pending', 'running')
           AND operations.presentation = 'important'
         ORDER BY COALESCE(operations.completed_at, operations.updated_at) DESC,
                  operations.updated_at DESC, operations.library_id ASC,
                  operations.operation_id ASC
         LIMIT 3`
      )
      .all() as unknown as OperationRow[];
    const thumbnailRows = this.database
      .prepare(
        `SELECT payload_json FROM thumbnail_summaries
         ORDER BY library_name COLLATE NOCASE ASC, library_id ASC`
      )
      .all() as unknown as Array<{ payload_json: string }>;
    const retryableFailures = this.database
      .prepare(
        `SELECT operations.library_id, operations.operation_id, operations.revision
         FROM operations
         LEFT JOIN action_errors
           ON action_errors.library_id = operations.library_id
          AND action_errors.operation_id = operations.operation_id
          AND action_errors.operation_revision = operations.revision
         WHERE operations.status NOT IN ('pending', 'running')
           AND (
             (operations.status = 'failed'
               AND json_extract(operations.payload_json, '$.retryable') = 1)
             OR action_errors.detail IS NOT NULL
           )
         ORDER BY operations.library_id ASC, operations.operation_id ASC`
      )
      .all()
      .map((row) => {
        const value = row as {
          library_id: string;
          operation_id: string;
          revision: number;
        };
        return {
          libraryId: value.library_id,
          operationId: value.operation_id,
          revision: value.revision
        } satisfies NotificationRetryableFailureReference;
      });
    return {
      active: active.map(rowToOperation),
      thumbnailSummaries: thumbnailRows.map((row) =>
        normalizeThumbnailSummary(JSON.parse(row.payload_json) as ThumbnailCacheSummary)
      ),
      latestImportantResults: latestImportantResults.map(rowToOperation),
      retryableFailures
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

    const clauses: string[] = [];
    const bindings: Array<string | number> = [];
    if (query.libraryId !== undefined) {
      assertString(query.libraryId, "history libraryId");
      clauses.push("operations.library_id = ?");
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
      clauses.push(`operations.status IN (${query.statuses.map(() => "?").join(", ")})`);
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
      clauses.push(`operations.category IN (${query.categories.map(() => "?").join(", ")})`);
      bindings.push(...query.categories);
    }
    if (query.origins !== undefined) {
      if (!Array.isArray(query.origins) || query.origins.length === 0) {
        throw new TypeError("history origins must be a non-empty array");
      }
      for (const origin of query.origins) {
        if (!ORIGINS.has(origin)) {
          throw new TypeError("history origin is invalid");
        }
      }
      clauses.push(
        `json_extract(operations.payload_json, '$.origin') IN (${query.origins.map(() => "?").join(", ")})`
      );
      bindings.push(...query.origins);
    }
    if (query.kinds !== undefined) {
      if (!Array.isArray(query.kinds) || query.kinds.length === 0 || query.kinds.length > 50) {
        throw new TypeError("history kinds must be a non-empty array");
      }
      for (const kind of query.kinds) {
        assertString(kind, "history kind");
        if (kind.length > 128) throw new RangeError("history kind exceeds 128 characters");
      }
      clauses.push(
        `json_extract(operations.payload_json, '$.kind') IN (${query.kinds.map(() => "?").join(", ")})`
      );
      bindings.push(...query.kinds);
    }
    if (query.search !== undefined) {
      if (typeof query.search !== "string") {
        throw new TypeError("history search must be a string");
      }
      const search = query.search.trim();
      if (search.length > 0) {
        clauses.push(`(
          json_extract(operations.payload_json, '$.title') LIKE ? ESCAPE '\\'
          OR json_extract(operations.payload_json, '$.summary') LIKE ? ESCAPE '\\'
          OR json_extract(operations.payload_json, '$.errorDetail') LIKE ? ESCAPE '\\'
          OR json_extract(operations.payload_json, '$.errorCode') LIKE ? ESCAPE '\\'
          OR EXISTS (
            SELECT 1 FROM action_errors
            WHERE action_errors.library_id = operations.library_id
              AND action_errors.operation_id = operations.operation_id
              AND action_errors.detail LIKE ? ESCAPE '\\'
          )
          OR ('Root ' || json_extract(operations.payload_json, '$.rootId')) LIKE ? ESCAPE '\\'
          OR ('Folder ' || json_extract(operations.payload_json, '$.folderId')) LIKE ? ESCAPE '\\'
          OR ('File ' || json_extract(operations.payload_json, '$.fileId')) LIKE ? ESCAPE '\\'
        )`);
        const pattern = `%${escapeSqlLikeLiteral(search)}%`;
        bindings.push(pattern, pattern, pattern, pattern, pattern, pattern, pattern, pattern);
      }
    }

    const where = clauses.length > 0 ? clauses.join(" AND ") : "1 = 1";
    const totalRow = this.database
      .prepare(`SELECT COUNT(*) AS total FROM operations WHERE ${where}`)
      .get(...bindings) as { total: number };
    const rows = this.database
      .prepare(
        `SELECT operations.library_id, operations.library_name, operations.payload_json,
                action_errors.detail AS action_error_detail
         FROM operations
         LEFT JOIN action_errors USING(library_id, operation_id)
         WHERE ${where}
         ORDER BY COALESCE(operations.completed_at, operations.updated_at) DESC,
                  operations.updated_at DESC, operations.library_id ASC,
                  operations.operation_id ASC
         LIMIT ? OFFSET ?`
      )
      .all(...bindings, limit, offset) as unknown as OperationRow[];
    const nextOffset = offset + rows.length < totalRow.total ? offset + rows.length : null;
    return { items: rows.map(rowToOperation), total: totalRow.total, nextOffset };
  }

  historyFacets(): NotificationHistoryFacets {
    const libraries = this.database
      .prepare(
        `SELECT library_id, MAX(library_name) AS library_name
         FROM operations
         GROUP BY library_id
         ORDER BY library_name COLLATE NOCASE ASC, library_id ASC`
      )
      .all()
      .map((row) => {
        const value = row as { library_id: string; library_name: string };
        return { id: value.library_id, name: value.library_name };
      });
    const kinds = this.database
      .prepare(
        `SELECT DISTINCT json_extract(payload_json, '$.kind') AS kind
         FROM operations
         WHERE json_type(payload_json, '$.kind') = 'text'
           AND json_extract(payload_json, '$.kind') <> ''
         ORDER BY kind COLLATE NOCASE ASC`
      )
      .all()
      .map((row) => (row as { kind: string }).kind);
    return { libraries, kinds };
  }

  getOperation(libraryId: string, operationId: string): DesktopNotificationOperation | null {
    assertString(libraryId, "libraryId");
    assertString(operationId, "operationId");
    const row = this.database
      .prepare(
        `SELECT operations.library_id, operations.library_name, operations.payload_json,
                action_errors.detail AS action_error_detail
         FROM operations
         LEFT JOIN action_errors USING(library_id, operation_id)
         WHERE operations.library_id = ? AND operations.operation_id = ?`
      )
      .get(libraryId, operationId) as OperationRow | undefined;
    return row ? rowToOperation(row) : null;
  }

  setOperationActionError(
    libraryId: string,
    operationId: string,
    expectedRevision: number,
    detail: string | null,
    updatedAt: number
  ): DesktopNotificationOperation | null {
    assertString(libraryId, "libraryId");
    assertString(operationId, "operationId");
    assertSafeInteger(expectedRevision, "expectedRevision");
    assertNullableString(detail, "action error detail");
    assertSafeInteger(updatedAt, "action error updatedAt");

    this.database.exec("SAVEPOINT set_operation_action_error;");
    try {
      const row = this.database
        .prepare(
          `SELECT revision FROM operations
           WHERE library_id = ? AND operation_id = ?`
        )
        .get(libraryId, operationId) as { revision: number } | undefined;
      if (!row || row.revision !== expectedRevision) {
        this.database.exec("RELEASE set_operation_action_error;");
        return null;
      }
      if (detail === null) {
        this.database
          .prepare("DELETE FROM action_errors WHERE library_id = ? AND operation_id = ?")
          .run(libraryId, operationId);
      } else {
        this.database
          .prepare(
            `INSERT INTO action_errors(
               library_id, operation_id, operation_revision, detail, updated_at
             ) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(library_id, operation_id) DO UPDATE SET
               operation_revision = excluded.operation_revision,
               detail = excluded.detail,
               updated_at = excluded.updated_at`
          )
          .run(libraryId, operationId, expectedRevision, detail, updatedAt);
      }
      this.database.exec("RELEASE set_operation_action_error;");
    } catch (error) {
      this.database.exec(
        "ROLLBACK TO set_operation_action_error; RELEASE set_operation_action_error;"
      );
      throw error;
    }
    return this.getOperation(libraryId, operationId);
  }

  recordRendererError(
    library: NotificationLibraryIdentity,
    report: RendererErrorReport,
    reportIds: string[],
    occurredAt: number,
    occurrenceCount = reportIds.length
  ): NotificationRendererErrorRecordResult {
    validateLibrary(library);
    assertObject(report, "renderer error report");
    for (const [name, value] of [
      ["source", report.source],
      ["code", report.code],
      ["target", report.target],
      ["title", report.title],
      ["message", report.message]
    ] as const) {
      assertString(value, `renderer error ${name}`);
    }
    assertNullableString(report.detail, "renderer error detail");
    if (!Array.isArray(reportIds)) throw new TypeError("renderer reportIds must be an array");
    const uniqueReportIds = [...new Set(reportIds)];
    for (const reportId of uniqueReportIds) assertString(reportId, "renderer reportId");
    assertSafeInteger(occurredAt, "renderer error occurredAt");
    assertSafeInteger(occurrenceCount, "renderer error occurrenceCount", 1);
    if (occurrenceCount < uniqueReportIds.length) {
      throw new RangeError("renderer error occurrenceCount must include every reportId");
    }

    const operationIdentity = {
      source: report.source,
      code: report.code,
      target: report.target
    };
    const operationId = rendererErrorOperationId(library.id, operationIdentity);
    const legacyOperationId = legacyRendererErrorOperationId(library.id, operationIdentity);
    this.database.exec("SAVEPOINT record_renderer_error;");
    try {
      const markerExists = this.database.prepare(
        "SELECT 1 FROM local_recovery_batches WHERE batch_id = ?"
      );
      const insertMarker = this.database.prepare(
        "INSERT INTO local_recovery_batches(batch_id, applied_at) VALUES (?, ?)"
      );
      let newReportCount = occurrenceCount - uniqueReportIds.length;
      for (const reportId of uniqueReportIds) {
        const markerId = rendererReportMarkerId(reportId);
        if (markerExists.get(markerId)) continue;
        insertMarker.run(markerId, occurredAt);
        newReportCount += 1;
      }
      let existing = this.getOperation(library.id, operationId);
      if (!existing && legacyOperationId !== operationId) {
        const legacyRow = this.database
          .prepare(
            `SELECT library_id, library_name, payload_json
             FROM operations WHERE library_id = ? AND operation_id = ?`
          )
          .get(library.id, legacyOperationId) as OperationRow | undefined;
        const legacy = legacyRow
          ? verifiedLegacyRendererOperation(legacyRow, legacyOperationId, operationId)
          : null;
        if (legacy) {
          const { libraryId: _libraryId, libraryName: _libraryName, ...legacyPayload } = legacy;
          const migratedOperation: StoredNotificationOperation = {
            ...legacyPayload,
            operationId,
            rendererErrorIdentity: { libraryId: library.id, ...operationIdentity }
          };
          this.upsertOperationRows(library, [migratedOperation]);
          this.database
            .prepare("DELETE FROM operations WHERE library_id = ? AND operation_id = ?")
            .run(library.id, legacyOperationId);
          existing = { ...legacy, operationId };
        }
      }
      if (newReportCount === 0) {
        this.database.exec("RELEASE record_renderer_error;");
        return { outcome: "duplicate", operation: existing };
      }
      const revision = (existing?.revision ?? 0) + 1;
      const nextOccurrenceCount = (existing?.occurrenceCount ?? 0) + newReportCount;
      assertSafeInteger(revision, "renderer error revision", 1);
      assertSafeInteger(nextOccurrenceCount, "renderer error occurrenceCount", 1);
      const operation: StoredNotificationOperation = {
        operationId,
        revision,
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
        currentValue: nextOccurrenceCount,
        totalValue: null,
        unit: "occurrences",
        errorCode: report.code,
        errorDetail: report.detail,
        cancellable: false,
        retryable: false,
        attempt: 1,
        occurrenceCount: nextOccurrenceCount,
        startedAt: existing?.startedAt ?? occurredAt,
        updatedAt: occurredAt,
        completedAt: occurredAt,
        rendererErrorIdentity: { libraryId: library.id, ...operationIdentity }
      };
      this.upsertOperationRows(library, [operation]);
      this.database.exec("RELEASE record_renderer_error;");
      return {
        outcome: "applied",
        operation: { ...operation, libraryId: library.id, libraryName: library.name }
      };
    } catch (error) {
      this.database.exec("ROLLBACK TO record_renderer_error; RELEASE record_renderer_error;");
      throw error;
    }
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

  applyLocalRecoveryBatch(batch: NotificationLocalRecoveryBatch, appliedAt: number): boolean {
    assertObject(batch, "local recovery batch");
    assertString(batch.batchId, "local recovery batchId");
    assertSafeInteger(batch.createdAt, "local recovery createdAt");
    assertSafeInteger(appliedAt, "local recovery appliedAt");
    if (
      !Array.isArray(batch.rendererErrors) ||
      !Array.isArray(batch.thumbnails) ||
      !Array.isArray(batch.actionErrors)
    ) {
      throw new TypeError("local recovery batch entries must be arrays");
    }

    this.database.exec("BEGIN IMMEDIATE;");
    try {
      const applied = this.database
        .prepare("SELECT 1 FROM local_recovery_batches WHERE batch_id = ?")
        .get(batch.batchId);
      if (applied) {
        this.database.exec("COMMIT;");
        return false;
      }
      for (const pending of batch.rendererErrors) {
        assertObject(pending, "pending renderer error");
        assertSafeInteger(pending.occurrenceCount, "pending renderer error occurrenceCount", 1);
        assertNullableString(
          pending.uncertainReportId,
          "pending renderer error uncertainReportId"
        );
        this.recordRendererError(
          pending.library,
          pending.report,
          pending.uncertainReportId ? [pending.uncertainReportId] : [],
          pending.occurredAt,
          pending.occurrenceCount
        );
      }
      for (const summary of batch.thumbnails) this.publishThumbnailSummary(summary);
      for (const pending of batch.actionErrors) {
        assertObject(pending, "pending action error");
        this.setOperationActionError(
          pending.libraryId,
          pending.operationId,
          pending.expectedRevision,
          pending.detail,
          pending.updatedAt
        );
      }
      this.database
        .prepare("INSERT INTO local_recovery_batches(batch_id, applied_at) VALUES (?, ?)")
        .run(batch.batchId, appliedAt);
      this.database.exec("COMMIT;");
      return true;
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
  }

  hasLegacyImport(libraryId: string): boolean {
    assertString(libraryId, "libraryId");
    return Boolean(
      this.database.prepare("SELECT 1 FROM legacy_imports WHERE library_id = ?").get(libraryId)
    );
  }

  importLegacyOperations(
    library: NotificationLibraryIdentity,
    operations: NotificationOperationInput[],
    importedAt: number
  ): boolean {
    validateLibrary(library);
    if (!Array.isArray(operations)) throw new TypeError("operations must be an array");
    for (const operation of operations) validateOperation(operation);
    assertSafeInteger(importedAt, "importedAt");

    this.database.exec("BEGIN IMMEDIATE;");
    try {
      if (this.hasLegacyImport(library.id)) {
        this.database.exec("COMMIT;");
        return false;
      }
      const existing = this.database.prepare(
        "SELECT 1 FROM operations WHERE library_id = ? AND operation_id = ?"
      );
      this.upsertOperationRows(
        library,
        operations.filter((operation) => !existing.get(library.id, operation.operationId))
      );
      this.database
        .prepare("INSERT INTO legacy_imports (library_id, imported_at) VALUES (?, ?)")
        .run(library.id, importedAt);
      this.database.exec("COMMIT;");
      return true;
    } catch (error) {
      this.database.exec("ROLLBACK;");
      throw error;
    }
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
      this.database
        .prepare("DELETE FROM local_recovery_batches WHERE applied_at < ?")
        .run(cutoff);
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
  const stored = JSON.parse(row.payload_json) as StoredNotificationOperation;
  const { rendererErrorIdentity: _rendererErrorIdentity, ...operation } = stored;
  return {
    ...operation,
    errorDetail: row.action_error_detail ?? operation.errorDetail,
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
    case "upsertOperations":
      store.upsertOperations(request.params.library, request.params.operations);
      return null;
    case "cursor":
      return store.cursor(request.params.libraryId);
    case "syncState":
      return store.syncState(request.params.libraryId);
    case "markAcknowledged":
      store.markAcknowledged(request.params.libraryId, request.params.sequence);
      return null;
    case "compactSnapshot":
      return store.compactSnapshot();
    case "history":
      return store.history(request.params.query);
    case "historyFacets":
      return store.historyFacets();
    case "getOperation":
      return store.getOperation(request.params.libraryId, request.params.operationId);
    case "setOperationActionError":
      return store.setOperationActionError(
        request.params.libraryId,
        request.params.operationId,
        request.params.expectedRevision,
        request.params.detail,
        request.params.updatedAt
      );
    case "recordRendererError":
      return store.recordRendererError(
        request.params.library,
        request.params.report,
        [request.params.reportId],
        request.params.occurredAt
      );
    case "publishThumbnailSummary":
      store.publishThumbnailSummary(request.params.summary);
      return null;
    case "applyLocalRecoveryBatch":
      return store.applyLocalRecoveryBatch(request.params.batch, request.params.appliedAt);
    case "hasLegacyImport":
      return store.hasLegacyImport(request.params.libraryId);
    case "importLegacyOperations":
      return store.importLegacyOperations(
        request.params.library,
        request.params.operations,
        request.params.importedAt
      );
    case "cleanupRetention":
      return store.cleanupRetention(request.params.now);
    case "close":
      return null;
  }
}

const REQUEST_METHODS = new Set<NotificationStoreRequest["method"]>([
  "applyOutboxBatch",
  "upsertOperations",
  "cursor",
  "syncState",
  "markAcknowledged",
  "compactSnapshot",
  "history",
  "historyFacets",
  "getOperation",
  "setOperationActionError",
  "recordRendererError",
  "publishThumbnailSummary",
  "applyLocalRecoveryBatch",
  "hasLegacyImport",
  "importLegacyOperations",
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
