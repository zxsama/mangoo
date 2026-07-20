import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { describe, it } from "node:test";
import { Worker } from "node:worker_threads";

import {
  createNotificationStore,
  notificationDatabasePath
} from "../src/notification-store.ts";
import type {
  NotificationLibraryIdentity,
  NotificationOperationInput,
  RendererErrorReport,
  NotificationStoreEvent,
  NotificationStoreWorkerMessage,
  ThumbnailCacheSummary
} from "../src/notification-types.ts";

const DAY_MS = 24 * 60 * 60 * 1_000;
const NOW = Date.UTC(2026, 6, 20, 12);
const LIBRARY_A: NotificationLibraryIdentity = { id: "library-a", name: "Library A" };
const LIBRARY_B: NotificationLibraryIdentity = { id: "library-b", name: "Library B" };
const TEST_WORKER_URL = new URL("../src/notification-store-worker.ts", import.meta.url);

function temporaryDirectory(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), "megle-notifications-"));
}

function testDatabasePath(): string {
  return notificationDatabasePath(temporaryDirectory());
}

function createTestStore(databasePath = testDatabasePath()) {
  return createNotificationStore({ databasePath, workerUrl: TEST_WORKER_URL });
}

function waitForWorkerMessage(worker: Worker, timeoutMs = 500): Promise<NotificationStoreWorkerMessage> {
  return new Promise((resolve, reject) => {
    const timeoutId = setTimeout(() => {
      cleanup();
      reject(new Error("timed out waiting for worker message"));
    }, timeoutMs);
    const onMessage = (message: NotificationStoreWorkerMessage) => {
      cleanup();
      resolve(message);
    };
    const onError = (error: Error) => {
      cleanup();
      reject(error);
    };
    const cleanup = () => {
      clearTimeout(timeoutId);
      worker.off("message", onMessage);
      worker.off("error", onError);
    };
    worker.once("message", onMessage);
    worker.once("error", onError);
  });
}

function dataWorkerUrl(source: string): URL {
  return new URL(`data:text/javascript,${encodeURIComponent(source)}`, import.meta.url);
}

function operation(
  operationId: string,
  revision: number,
  status: NotificationOperationInput["status"],
  overrides: Partial<NotificationOperationInput> = {}
): NotificationOperationInput {
  const terminal = status === "succeeded" || status === "failed" || status === "cancelled";
  return {
    operationId,
    revision,
    kind: "root_scan",
    category: "task",
    origin: "user",
    presentation: "important",
    status,
    phase: status,
    title: `Operation ${operationId}`,
    summary: null,
    taskId: 1,
    rootId: 2,
    folderId: null,
    fileId: null,
    currentValue: terminal ? 10 : 2,
    totalValue: 10,
    unit: "items",
    errorCode: null,
    errorDetail: null,
    cancellable: !terminal,
    retryable: status === "failed",
    attempt: 1,
    occurrenceCount: 1,
    startedAt: NOW - 2_000,
    updatedAt: NOW,
    completedAt: terminal ? NOW : null,
    ...overrides
  };
}

function event(sequence: number, input: NotificationOperationInput): NotificationStoreEvent {
  return { sequence, operation: input };
}

function thumbnailSummary(
  library: NotificationLibraryIdentity,
  cachedCount: number
): ThumbnailCacheSummary {
  return {
    libraryId: library.id,
    libraryName: library.name,
    observedAt: NOW,
    cachedCount,
    totalCandidateCount: 100,
    missingCount: 100 - cachedCount,
    staleCount: 0,
    failedCount: 0,
    pendingCount: 0,
    activeBulkTaskCount: 0,
    totalCacheBytes: cachedCount * 1_024,
    completionPercent: cachedCount
  };
}

describe("notification store", () => {
  it("commits operation revisions and cursor atomically and rejects stale revisions", async () => {
    const store = await createTestStore();
    try {
      await store.applyOutboxBatch(
        LIBRARY_A,
        [event(1, operation("task:1", 1, "running"))],
        1
      );
      await store.applyOutboxBatch(
        LIBRARY_A,
        [event(1, operation("task:1", 1, "running"))],
        1
      );
      await store.applyOutboxBatch(
        LIBRARY_A,
        [event(2, operation("task:1", 0, "pending", { updatedAt: NOW + 1_000 }))],
        2
      );

      let snapshot = await store.compactSnapshot();
      assert.equal(snapshot.active.length, 1);
      assert.equal(snapshot.active[0]?.revision, 1);
      assert.equal(snapshot.active[0]?.status, "running");
      assert.equal(await store.cursor(LIBRARY_A.id), 2);
      assert.deepEqual(await store.syncState(LIBRARY_A.id), {
        persistedSequence: 2,
        ackedSequence: 0
      });

      await assert.rejects(
        store.applyOutboxBatch(
          LIBRARY_A,
          [
            event(4, operation("rolled-back", 1, "running")),
            event(5, operation("task:1", 1, "failed", { summary: "conflict" }))
          ],
          5
        ),
        /conflicting payload/i
      );
      assert.equal(await store.cursor(LIBRARY_A.id), 2, "failed batch must not advance the cursor");
      assert.equal((await store.history({ search: "rolled-back", limit: 10, offset: 0 })).total, 0);

      await store.applyOutboxBatch(
        LIBRARY_A,
        [event(3, operation("task:1", 2, "succeeded", { completedAt: NOW + 2_000 }))],
        3
      );
      snapshot = await store.compactSnapshot();
      assert.equal(snapshot.active.length, 0);
      assert.equal(snapshot.latestImportantResults[0]?.revision, 2);
      assert.equal(await store.cursor(LIBRARY_A.id), 3);

      await store.markAcknowledged(LIBRARY_A.id, 3);
      assert.deepEqual(await store.syncState(LIBRARY_A.id), {
        persistedSequence: 3,
        ackedSequence: 3
      });
      await store.markAcknowledged(LIBRARY_A.id, 2);
      await assert.rejects(store.markAcknowledged(LIBRARY_A.id, 4), /exceed persisted/i);
      assert.equal((await store.syncState(LIBRARY_A.id)).ackedSequence, 3);

      await store.applyOutboxBatch(LIBRARY_A, [], 1);
      assert.equal(await store.cursor(LIBRARY_A.id), 3, "cursor must never move backwards");
    } finally {
      await store.close();
    }
  });

  it("keeps all-library state isolated while exposing global compact and history views", async () => {
    const store = await createTestStore();
    try {
      await store.applyOutboxBatch(
        LIBRARY_A,
        [
          event(1, operation("a:active", 1, "running", { updatedAt: NOW + 10 })),
          event(2, operation("a:done", 1, "succeeded", { updatedAt: NOW + 20 }))
        ],
        2
      );
      await store.applyOutboxBatch(
        LIBRARY_B,
        [
          event(1, operation("b:active", 1, "pending", { updatedAt: NOW + 30 })),
          event(2, operation("b:done", 1, "failed", { updatedAt: NOW + 40 }))
        ],
        2
      );

      const compact = await store.compactSnapshot();
      assert.deepEqual(
        compact.active.map((item) => `${item.libraryId}:${item.operationId}`),
        ["library-b:b:active", "library-a:a:active"]
      );
      assert.deepEqual(
        compact.latestImportantResults.map((item) => item.operationId),
        ["b:done", "a:done"]
      );

      const onlyA = await store.history({ libraryId: LIBRARY_A.id, limit: 20, offset: 0 });
      assert.equal(onlyA.total, 1);
      assert.deepEqual(onlyA.items.map((item) => item.operationId), ["a:done"]);
      const all = await store.history({ limit: 20, offset: 0 });
      assert.equal(all.total, 2);
      assert.deepEqual(all.items.map((item) => item.operationId), ["b:done", "a:done"]);
    } finally {
      await store.close();
    }
  });

  it("limits compact important results to the newest three without a time cutoff", async () => {
    const store = await createTestStore();
    try {
      await store.applyOutboxBatch(
        LIBRARY_A,
        [
          event(1, operation("result:1", 1, "succeeded", { updatedAt: NOW - 100_000 })),
          event(2, operation("result:2", 1, "failed", { updatedAt: NOW - 90_000 })),
          event(3, operation("result:3", 1, "cancelled", { updatedAt: NOW - 80_000 })),
          event(4, operation("result:4", 1, "succeeded", { updatedAt: NOW - 70_000 })),
          event(
            5,
            operation("history-only", 1, "succeeded", {
              presentation: "history_only",
              updatedAt: NOW
            })
          )
        ],
        5
      );
      const compact = await store.compactSnapshot();
      assert.deepEqual(
        compact.latestImportantResults.map((item) => item.operationId),
        ["result:4", "result:3", "result:2"]
      );
    } finally {
      await store.close();
    }
  });

  it("upserts thumbnail summaries independently for each library", async () => {
    const store = await createTestStore();
    try {
      await store.publishThumbnailSummary(thumbnailSummary(LIBRARY_A, 20));
      await store.publishThumbnailSummary(thumbnailSummary(LIBRARY_B, 40));
      await store.publishThumbnailSummary({
        ...thumbnailSummary(LIBRARY_A, 80),
        observedAt: NOW + 1
      });

      const compact = await store.compactSnapshot();
      assert.deepEqual(
        compact.thumbnailSummaries.map((summary) => [summary.libraryId, summary.cachedCount]),
        [
          [LIBRARY_A.id, 80],
          [LIBRARY_B.id, 40]
        ]
      );
    } finally {
      await store.close();
    }
  });

  it("marks legacy imports idempotently per library", async () => {
    const store = await createTestStore();
    try {
      assert.equal(await store.hasLegacyImport(LIBRARY_A.id), false);
      await store.markLegacyImported(LIBRARY_A.id, NOW);
      await store.markLegacyImported(LIBRARY_A.id, NOW + 1_000);
      assert.equal(await store.hasLegacyImport(LIBRARY_A.id), true);
      assert.equal(await store.hasLegacyImport(LIBRARY_B.id), false);
    } finally {
      await store.close();
    }
  });

  it("upserts local operations and atomically aggregates renderer error occurrences", async () => {
    const store = await createTestStore();
    const report: RendererErrorReport = {
      libraryId: LIBRARY_A.id,
      libraryName: LIBRARY_A.name,
      source: "library-data",
      code: "load-failed",
      target: "folder:4",
      title: "Folder load failed",
      message: "Could not load folder",
      detail: "database busy"
    };
    try {
      await store.upsertOperations(LIBRARY_A, [
        operation("legacy-file-op:9", 1, "succeeded", {
          kind: "move",
          category: "file_operation",
          taskId: null
        })
      ]);
      assert.equal(
        (await store.getOperation(LIBRARY_A.id, "legacy-file-op:9"))?.kind,
        "move"
      );

      await store.recordRendererError(LIBRARY_A, report, NOW, 1);
      const aggregated = await store.recordRendererError(LIBRARY_A, report, NOW + 1, 3);
      assert.equal(aggregated.revision, 2);
      assert.equal(aggregated.occurrenceCount, 4);
      assert.equal(
        (
          await store.getOperation(
            LIBRARY_A.id,
            `renderer-error:${LIBRARY_A.id}:${report.source}:${report.code}:${report.target}`
          )
        )?.occurrenceCount,
        4
      );
    } finally {
      await store.close();
    }
  });

  it("atomically overlays action errors without competing with Core revisions", async () => {
    const store = await createTestStore();
    try {
      await store.upsertOperations(LIBRARY_A, [
        operation("task:action", 2, "running", { errorDetail: "core detail" })
      ]);
      const failed = await store.setOperationActionError(
        LIBRARY_A.id,
        "task:action",
        2,
        "cancel failed"
      );
      assert.equal(failed?.errorDetail, "cancel failed");
      assert.equal(
        await store.setOperationActionError(LIBRARY_A.id, "task:action", 1, "stale"),
        null
      );
      assert.equal(
        (await store.getOperation(LIBRARY_A.id, "task:action"))?.errorDetail,
        "cancel failed"
      );

      await store.setOperationActionError(LIBRARY_A.id, "task:action", 2, null);
      assert.equal(
        (await store.getOperation(LIBRARY_A.id, "task:action"))?.errorDetail,
        "core detail"
      );

      await store.setOperationActionError(LIBRARY_A.id, "task:action", 2, "retry failed");
      await store.upsertOperations(LIBRARY_A, [
        operation("task:action", 3, "running", { errorDetail: "new core detail" })
      ]);
      assert.equal(
        (await store.getOperation(LIBRARY_A.id, "task:action"))?.errorDetail,
        "new core detail"
      );
    } finally {
      await store.close();
    }
  });

  it("retains active operations, removes completed records older than 30 days, and caps completed history at 2000", async () => {
    const store = await createTestStore();
    try {
      const events: NotificationStoreEvent[] = [
        event(
          1,
          operation("active:old", 1, "running", {
            startedAt: NOW - 90 * DAY_MS,
            updatedAt: NOW - 90 * DAY_MS,
            completedAt: null
          })
        ),
        event(
          2,
          operation("completed:old", 1, "succeeded", {
            updatedAt: NOW - 31 * DAY_MS,
            completedAt: NOW - 31 * DAY_MS
          })
        )
      ];
      for (let index = 0; index < 2_005; index += 1) {
        const timestamp = NOW - (2_005 - index) * 1_000;
        events.push(
          event(
            index + 3,
            operation(`completed:${index}`, 1, "succeeded", {
              updatedAt: timestamp,
              completedAt: timestamp
            })
          )
        );
      }
      await store.applyOutboxBatch(LIBRARY_A, events, events.length);

      const result = await store.cleanupRetention(NOW);
      assert.equal(result.deletedExpired, 1);
      assert.equal(result.deletedOverflow, 5);
      const compact = await store.compactSnapshot();
      assert.deepEqual(compact.active.map((item) => item.operationId), ["active:old"]);
      const history = await store.history({ limit: 100, offset: 0 });
      assert.equal(history.total, 2_000);
      assert.equal(history.items[0]?.operationId, "completed:2004");
      assert.equal(history.items.at(-1)?.operationId, "completed:1905");
    } finally {
      await store.close();
    }
  });

  it("initializes a versioned WAL schema and preserves data across reopen", async () => {
    const databasePath = testDatabasePath();
    let store = await createTestStore(databasePath);
    await store.applyOutboxBatch(
      LIBRARY_A,
      [event(1, operation("persisted", 1, "succeeded"))],
      1
    );
    await store.close();

    const inspection = new DatabaseSync(databasePath, { readOnly: true });
    try {
      assert.equal(inspection.prepare("PRAGMA journal_mode").get()?.journal_mode, "wal");
      assert.equal(inspection.prepare("PRAGMA user_version").get()?.user_version, 3);
      const tables = inspection
        .prepare(
          "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        )
        .all()
        .map((row) => row.name);
      assert.deepEqual(tables, [
        "action_errors",
        "legacy_imports",
        "operations",
        "sync_cursors",
        "thumbnail_summaries"
      ]);
    } finally {
      inspection.close();
    }

    const v2Database = new DatabaseSync(databasePath);
    v2Database.exec(`
      DROP TABLE action_errors;
      ALTER TABLE sync_cursors RENAME TO sync_cursors_v3;
      CREATE TABLE sync_cursors (
        library_id TEXT PRIMARY KEY,
        sequence INTEGER NOT NULL
      );
      INSERT INTO sync_cursors(library_id, sequence)
        SELECT library_id, persisted_sequence FROM sync_cursors_v3;
      DROP TABLE sync_cursors_v3;
      PRAGMA user_version = 2;
    `);
    v2Database.close();

    store = await createTestStore(databasePath);
    try {
      assert.equal((await store.history({ limit: 10, offset: 0 })).items[0]?.operationId, "persisted");
      assert.equal(await store.cursor(LIBRARY_A.id), 1);
      assert.deepEqual(await store.syncState(LIBRARY_A.id), {
        persistedSequence: 1,
        ackedSequence: 0
      });
    } finally {
      await store.close();
    }

    const upgraded = new DatabaseSync(databasePath, { readOnly: true });
    try {
      assert.equal(upgraded.prepare("PRAGMA user_version").get()?.user_version, 3);
      const indexSql = upgraded
        .prepare("SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'idx_operations_history'")
        .get()?.sql as string;
      assert.match(indexSql, /COALESCE\s*\(completed_at, updated_at\)/i);
      assert.match(indexSql, /WHERE\s+status\s+NOT\s+IN/i);
      const queryPlan = upgraded
        .prepare(
          `EXPLAIN QUERY PLAN
           SELECT library_id, library_name, payload_json
           FROM operations
           WHERE status NOT IN ('pending', 'running')
           ORDER BY COALESCE(completed_at, updated_at) DESC, updated_at DESC`
        )
        .all()
        .map((row) => String(row.detail))
        .join("\n");
      assert.match(queryPlan, /idx_operations_history/i);
    } finally {
      upgraded.close();
    }
  });

  it("allows concurrent first opens of the same empty v0 database", async () => {
    const databasePath = testDatabasePath();
    const [first, second] = await Promise.all([
      createTestStore(databasePath),
      createTestStore(databasePath)
    ]);
    try {
      await first.applyOutboxBatch(
        LIBRARY_A,
        [event(1, operation("concurrent-init", 1, "running"))],
        1
      );
      assert.equal(await second.cursor(LIBRARY_A.id), 1);
    } finally {
      await Promise.all([first.close(), second.close()]);
    }
  });

  it("serializes concurrent requests and closes without leaking pending calls", async () => {
    const store = await createTestStore();
    await Promise.all(
      Array.from({ length: 40 }, (_, index) =>
        store.applyOutboxBatch(
          index % 2 === 0 ? LIBRARY_A : LIBRARY_B,
          [event(index + 1, operation(`parallel:${index}`, 1, "running"))],
          index + 1
        )
      )
    );
    const snapshot = await store.compactSnapshot();
    assert.equal(snapshot.active.length, 40);

    await store.close();
    await assert.rejects(store.cursor(LIBRARY_A.id), /closed/i);
    await store.close();
  });

  it("rejects unsafe integers and clone failures without corrupting the worker", async () => {
    const store = await createTestStore();
    try {
      await assert.rejects(
        store.applyOutboxBatch(
          LIBRARY_A,
          [event(Number.MAX_SAFE_INTEGER + 1, operation("unsafe", 1, "running"))],
          Number.MAX_SAFE_INTEGER + 1
        ),
        /safe integer/i
      );
      await assert.rejects(
        store.history({ search: (() => "not cloneable") as unknown as string, limit: 10, offset: 0 }),
        /clone|serializ/i
      );
      assert.equal(await store.cursor(LIBRARY_A.id), 0);
    } finally {
      await store.close();
    }
  });

  it("propagates worker startup failures", async () => {
    const missingParent = path.join(temporaryDirectory(), "missing", "notifications.sqlite");
    await assert.rejects(
      createNotificationStore({ databasePath: missingParent, workerUrl: TEST_WORKER_URL }),
      /open|database|sqlite/i
    );
  });

  it("guards malformed worker messages without undefined responses or worker crashes", async () => {
    const worker = new Worker(TEST_WORKER_URL, { workerData: { databasePath: testDatabasePath() } });
    try {
      assert.deepEqual(await waitForWorkerMessage(worker), { type: "ready" });

      worker.postMessage(null);
      await assert.rejects(waitForWorkerMessage(worker, 80), /timed out/);

      worker.postMessage({ requestId: 42, method: "cursor", params: {} });
      await assert.rejects(waitForWorkerMessage(worker, 80), /timed out/);

      worker.postMessage({ requestId: "unknown-method", method: "explode", params: {} });
      const protocolError = await waitForWorkerMessage(worker);
      assert.equal(protocolError.type, "response");
      assert.equal(protocolError.requestId, "unknown-method");
      assert.equal(protocolError.ok, false);
      if (protocolError.type === "response" && !protocolError.ok) {
        assert.match(protocolError.error.message, /unknown.*method/i);
      }

      worker.postMessage({ requestId: "valid", method: "cursor", params: { libraryId: "library-a" } });
      const validResponse = await waitForWorkerMessage(worker);
      assert.deepEqual(validResponse, {
        type: "response",
        requestId: "valid",
        ok: true,
        result: 0
      });

      worker.postMessage({ requestId: "close", method: "close", params: {} });
      const closeResponse = await waitForWorkerMessage(worker);
      assert.equal(closeResponse.type, "response");
      assert.equal(closeResponse.requestId, "close");
    } finally {
      await worker.terminate();
    }
  });

  it("rejects pending requests when a ready worker crashes", async () => {
    const workerUrl = new URL(
      "data:text/javascript,import%20%7BparentPort%7D%20from%20%27node%3Aworker_threads%27%3BparentPort.postMessage(%7Btype%3A%27ready%27%7D)%3BparentPort.on(%27message%27%2C()%3D%3E%7Bthrow%20new%20Error(%27worker-boom%27)%7D)",
      import.meta.url
    );
    const store = await createNotificationStore({
      databasePath: testDatabasePath(),
      workerUrl
    });
    const pending = store.cursor(LIBRARY_A.id);
    await assert.rejects(pending, /worker-boom|worker|exited/i);
    await store.close();
  });

  it("terminates an unresponsive worker instead of hanging during close", async () => {
    const workerUrl = new URL(
      "data:text/javascript,import%20%7BparentPort%7D%20from%20%27node%3Aworker_threads%27%3BparentPort.postMessage(%7Btype%3A%27ready%27%7D)",
      import.meta.url
    );
    const store = await createNotificationStore({
      databasePath: testDatabasePath(),
      workerUrl
    });
    const startedAt = Date.now();
    await store.close();
    assert.ok(Date.now() - startedAt < 3_000, "close should have a bounded fallback");
  });

  it("rejects an in-flight request immediately during close and ignores its late response", async () => {
    const workerUrl = dataWorkerUrl(`
      import { parentPort } from "node:worker_threads";
      parentPort.postMessage({ type: "ready" });
      parentPort.on("message", (message) => {
        if (message.method === "cursor") {
          setTimeout(() => parentPort.postMessage({
            type: "response", requestId: message.requestId, ok: true, result: 99
          }), 20);
        }
        if (message.method === "close") {
          setTimeout(() => parentPort.postMessage({
            type: "response", requestId: message.requestId, ok: true, result: null
          }), 80);
        }
      });
    `);
    const store = await createNotificationStore({
      databasePath: testDatabasePath(),
      workerUrl
    });
    const pending = store.cursor(LIBRARY_A.id);
    await new Promise<void>((resolve) => setImmediate(resolve));
    const startedAt = Date.now();
    const firstClose = store.close();
    const secondClose = store.close();
    await assert.rejects(pending, /closing|closed/i);
    assert.ok(Date.now() - startedAt < 50, "pending requests should reject when close starts");
    await Promise.all([firstClose, secondClose]);
    await assert.rejects(store.cursor(LIBRARY_A.id), /closed/i);
  });

  it("does not send a cursor request when close races it in the same tick", async () => {
    const workerUrl = dataWorkerUrl(`
      import { parentPort } from "node:worker_threads";
      parentPort.postMessage({ type: "ready" });
      parentPort.on("message", (message) => {
        if (message.method === "cursor") {
          throw new Error("cursor request was sent after close began");
        }
        if (message.method === "close") {
          parentPort.postMessage({
            type: "response", requestId: message.requestId, ok: true, result: null
          });
        }
      });
    `);
    const store = await createNotificationStore({
      databasePath: testDatabasePath(),
      workerUrl
    });
    const startedAt = Date.now();
    const cursorPromise = store.cursor(LIBRARY_A.id);
    const firstClose = store.close();
    const secondClose = store.close();
    await assert.rejects(cursorPromise, /closed/i);
    await Promise.all([firstClose, secondClose]);
    assert.ok(Date.now() - startedAt < 500, "same-tick close race should not hit the close timeout");
  });
});
