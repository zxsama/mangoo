import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { describe, it } from "node:test";
import { Worker } from "node:worker_threads";

import {
  createNotificationStore,
  notificationDatabasePath,
  NotificationStore
} from "../src/notification-store.ts";
import type {
  NotificationLibraryIdentity,
  NotificationLocalRecoveryBatch,
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

function createTrackedTestStores(databasePath: string, workerUrls: URL[]): NotificationStore[] {
  return workerUrls.map(
    (workerUrl) => new NotificationStore({ databasePath, workerUrl })
  );
}

async function settleTrackedStoreReadiness(
  stores: NotificationStore[]
): Promise<PromiseSettledResult<NotificationStore>[]> {
  return Promise.allSettled(
    stores.map(async (store) => {
      await store.ready();
      return store;
    })
  );
}

async function closeTrackedStores(stores: NotificationStore[]): Promise<void> {
  const settled = await Promise.allSettled(stores.map((store) => store.close()));
  const rejected = settled.find(
    (result): result is PromiseRejectedResult => result.status === "rejected"
  );
  if (rejected) throw rejected.reason;
}

function activeMessagePortCount(): number {
  return process.getActiveResourcesInfo().filter((resource) => resource === "MessagePort").length;
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
      assert.equal(onlyA.total, 2);
      assert.deepEqual(onlyA.items.map((item) => item.operationId), ["a:active", "a:done"]);
      const all = await store.history({ limit: 20, offset: 0 });
      assert.equal(all.total, 4);
      assert.deepEqual(all.items.map((item) => item.operationId), [
        "b:active",
        "a:active",
        "b:done",
        "a:done"
      ]);
      const active = await store.history({ statuses: ["pending", "running"], limit: 20, offset: 0 });
      assert.equal(active.total, 2);
      assert.deepEqual(active.items.map((item) => item.operationId), ["b:active", "a:active"]);
    } finally {
      await store.close();
    }
  });

  it("applies origin and kind filters and searches targets and error details", async () => {
    const store = await createTestStore();
    try {
      await store.upsertOperations(LIBRARY_A, [
        operation("target-error", 1, "failed", {
          kind: "delete_folder",
          origin: "background",
          summary: null,
          errorCode: "folder_delete_failed",
          errorDetail: "permission denied for folder",
          rootId: 7,
          folderId: 8,
          fileId: 9
        }),
        operation("other-kind", 1, "succeeded", {
          kind: "root_scan",
          origin: "user"
        })
      ]);

      const filtered = await store.history({
        origins: ["background"],
        kinds: ["delete_folder"],
        limit: 1,
        offset: 0
      });
      assert.equal(filtered.total, 1);
      assert.equal(filtered.items[0]?.operationId, "target-error");

      for (const search of ["permission denied", "folder_delete_failed", "Root 7", "Folder 8", "File 9"]) {
        const result = await store.history({ search, limit: 20, offset: 0 });
        assert.equal(result.total, 1, `search should match ${search}`);
        assert.equal(result.items[0]?.operationId, "target-error");
      }
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

  it("keeps retryable failure state independent from the latest-three result window", async () => {
    const store = await createTestStore();
    try {
      await store.upsertOperations(LIBRARY_A, [
        operation("failed:durable", 1, "failed", {
          updatedAt: NOW - 100_000,
          completedAt: NOW - 100_000,
          retryable: true
        }),
        operation("success:1", 1, "succeeded", { updatedAt: NOW - 3_000 }),
        operation("success:2", 1, "succeeded", { updatedAt: NOW - 2_000 }),
        operation("success:3", 1, "succeeded", { updatedAt: NOW - 1_000 })
      ]);

      let compact = await store.compactSnapshot();
      assert.deepEqual(
        compact.latestImportantResults.map((item) => item.operationId),
        ["success:3", "success:2", "success:1"]
      );
      assert.deepEqual(compact.retryableFailures, [
        {
          libraryId: LIBRARY_A.id,
          operationId: "failed:durable",
          revision: 1
        }
      ]);

      await store.upsertOperations(LIBRARY_A, [
        operation("failed:durable", 2, "succeeded", {
          updatedAt: NOW + 1,
          completedAt: NOW + 1,
          retryable: false
        })
      ]);
      compact = await store.compactSnapshot();
      assert.deepEqual(compact.retryableFailures, []);
    } finally {
      await store.close();
    }
  });

  it("treats a current terminal action error as an authoritative failure", async () => {
    const store = await createTestStore();
    try {
      await store.upsertOperations(LIBRARY_A, [
        operation("failed:action", 1, "failed", { retryable: false })
      ]);
      assert.deepEqual((await store.compactSnapshot()).retryableFailures, []);

      await store.setOperationActionError(
        LIBRARY_A.id,
        "failed:action",
        1,
        "retry request failed"
      );
      let compact = await store.compactSnapshot();
      assert.deepEqual(compact.retryableFailures, [
        {
          libraryId: LIBRARY_A.id,
          operationId: "failed:action",
          revision: 1
        }
      ]);

      await store.upsertOperations(LIBRARY_A, [
        operation("failed:action", 2, "succeeded", { retryable: false })
      ]);
      compact = await store.compactSnapshot();
      assert.deepEqual(compact.retryableFailures, []);
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
      assert.equal(await store.importLegacyOperations(LIBRARY_A, [], NOW), true);
      assert.equal(await store.importLegacyOperations(LIBRARY_A, [], NOW + 1_000), false);
      assert.equal(await store.hasLegacyImport(LIBRARY_A.id), true);
      assert.equal(await store.hasLegacyImport(LIBRARY_B.id), false);
    } finally {
      await store.close();
    }
  });

  it("atomically imports legacy operations with their marker and retries changed payloads", async () => {
    const databasePath = testDatabasePath();
    const initialized = await createTestStore(databasePath);
    await initialized.close();
    const inspection = new DatabaseSync(databasePath);
    inspection.exec(`
      CREATE TRIGGER fail_legacy_marker
      BEFORE INSERT ON legacy_imports
      BEGIN
        SELECT RAISE(ABORT, 'legacy marker failed');
      END;
    `);
    inspection.close();

    let store = await createTestStore(databasePath);
    const first = operation("legacy-import:library-a", 0, "succeeded", {
      kind: "legacy_file_operations",
      category: "file_operation",
      taskId: null,
      currentValue: 2,
      totalValue: 2
    });
    await assert.rejects(
      store.importLegacyOperations(LIBRARY_A, [first], NOW),
      /legacy marker failed/i
    );
    assert.equal(await store.getOperation(LIBRARY_A.id, first.operationId), null);
    assert.equal(await store.hasLegacyImport(LIBRARY_A.id), false);
    await store.close();

    const repair = new DatabaseSync(databasePath);
    repair.exec("DROP TRIGGER fail_legacy_marker;");
    repair.close();
    store = await createTestStore(databasePath);
    try {
      const changedAfterCutoff = {
        ...first,
        currentValue: 1,
        totalValue: 1,
        summary: "cutoff changed"
      };
      assert.equal(
        await store.importLegacyOperations(LIBRARY_A, [changedAfterCutoff], NOW + 1),
        true
      );
      assert.equal(await store.hasLegacyImport(LIBRARY_A.id), true);
      assert.equal(
        (await store.getOperation(LIBRARY_A.id, first.operationId))?.summary,
        "cutoff changed"
      );
      assert.equal(
        await store.importLegacyOperations(LIBRARY_A, [first], NOW + 2),
        false,
        "the marker must make repeated imports a no-op without revision conflicts"
      );
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

      await store.recordRendererError(LIBRARY_A, report, "aggregate-1", NOW);
      const aggregated = await store.recordRendererError(
        LIBRARY_A,
        report,
        "aggregate-2",
        NOW + 1
      );
      assert.equal(aggregated.outcome, "applied");
      assert.equal(aggregated.operation?.revision, 2);
      assert.equal(aggregated.operation?.occurrenceCount, 2);
      assert.equal(
        (
          await store.getOperation(
            LIBRARY_A.id,
            `renderer-error:${LIBRARY_A.id}:${report.source}:${report.code}:${report.target}`
          )
        )?.occurrenceCount,
        2
      );
    } finally {
      await store.close();
    }
  });

  it("counts distinct renderer report ids once across process restarts", async () => {
    const databasePath = testDatabasePath();
    const report: RendererErrorReport = {
      libraryId: LIBRARY_A.id,
      libraryName: LIBRARY_A.name,
      source: "renderer",
      code: "load-failed",
      target: "folder:4",
      title: "Folder load failed",
      message: "Could not load folder",
      detail: "database busy"
    };
    let store = await createTestStore(databasePath);
    try {
      await store.recordRendererError(LIBRARY_A, report, "report-a", NOW);
      const distinct = await store.recordRendererError(
        LIBRARY_A,
        report,
        "report-b",
        NOW + 1
      );
      assert.equal(distinct.outcome, "applied");
      assert.equal(distinct.operation?.occurrenceCount, 2);
    } finally {
      await store.close();
    }

    store = await createTestStore(databasePath);
    try {
      const replayed = await store.recordRendererError(
        LIBRARY_A,
        report,
        "report-a",
        NOW + 2
      );
      assert.equal(replayed.outcome, "duplicate");
      assert.equal(
        replayed.operation?.occurrenceCount,
        2,
        "the same report id must survive restart"
      );
    } finally {
      await store.close();
    }
  });

  it("keeps renderer report ids idempotent after their operation is evicted", async () => {
    const databasePath = testDatabasePath();
    const report: RendererErrorReport = {
      libraryId: LIBRARY_A.id,
      libraryName: LIBRARY_A.name,
      source: "renderer",
      code: "overflow-evicted",
      target: "folder:overflow",
      title: "Overflow evicted report",
      message: "The operation may age out before its report marker",
      detail: null
    };
    const operationId =
      `renderer-error:${LIBRARY_A.id}:${report.source}:${report.code}:${report.target}`;
    let store = await createTestStore(databasePath);
    try {
      await store.recordRendererError(
        LIBRARY_A,
        report,
        "overflow-report",
        NOW - 10_000
      );

      const newerEvents: NotificationStoreEvent[] = [];
      for (let index = 0; index < 2_000; index += 1) {
        const timestamp = NOW - (2_000 - index);
        newerEvents.push(
          event(
            index + 1,
            operation(`newer:${index}`, 1, "succeeded", {
              updatedAt: timestamp,
              completedAt: timestamp
            })
          )
        );
      }
      await store.applyOutboxBatch(LIBRARY_A, newerEvents, newerEvents.length);
      assert.equal((await store.cleanupRetention(NOW)).deletedOverflow, 1);
      assert.equal(await store.getOperation(LIBRARY_A.id, operationId), null);
    } finally {
      await store.close();
    }

    store = await createTestStore(databasePath);
    try {
      const duplicate = await store.recordRendererError(
        LIBRARY_A,
        report,
        "overflow-report",
        NOW + 1
      );
      assert.deepEqual(duplicate, { outcome: "duplicate", operation: null });
      assert.equal(await store.getOperation(LIBRARY_A.id, operationId), null);

      const distinct = await store.recordRendererError(
        LIBRARY_A,
        report,
        "overflow-report-distinct",
        NOW + 2
      );
      assert.equal(distinct.outcome, "applied");
      assert.equal(distinct.operation?.occurrenceCount, 1);
    } finally {
      await store.close();
    }
  });

  it("accepts a renderer report id again after its marker retention expires", async () => {
    const store = await createTestStore();
    const report: RendererErrorReport = {
      libraryId: LIBRARY_A.id,
      libraryName: LIBRARY_A.name,
      source: "renderer",
      code: "expired-marker",
      target: "folder:expired",
      title: "Expired marker report",
      message: "The report id can be reused after the retention window",
      detail: null
    };
    try {
      await store.recordRendererError(
        LIBRARY_A,
        report,
        "expired-report",
        NOW - 31 * DAY_MS
      );
      await store.cleanupRetention(NOW);

      const replayed = await store.recordRendererError(
        LIBRARY_A,
        report,
        "expired-report",
        NOW + 1
      );
      assert.equal(replayed.outcome, "applied");
      assert.equal(replayed.operation?.occurrenceCount, 1);
    } finally {
      await store.close();
    }
  });

  it("applies local recovery batches atomically and idempotently", async () => {
    const store = await createTestStore();
    const report: RendererErrorReport = {
      libraryId: LIBRARY_A.id,
      libraryName: LIBRARY_A.name,
      source: "recovery",
      code: "load-failed",
      target: "folder:4",
      title: "Folder load failed",
      message: "Could not load folder",
      detail: "database busy"
    };
    const task = operation("task:recovery", 2, "running", { errorDetail: null });
    const batch: NotificationLocalRecoveryBatch = {
      batchId: "recovery-batch-1",
      rendererErrors: [
        {
          library: LIBRARY_A,
          report,
          reportIds: ["recovery-report-1", "recovery-report-2"],
          occurredAt: NOW
        }
      ],
      thumbnails: [thumbnailSummary(LIBRARY_A, 70)],
      actionErrors: [
        {
          libraryId: LIBRARY_A.id,
          operationId: task.operationId,
          expectedRevision: task.revision,
          detail: "action failed",
          updatedAt: NOW
        }
      ],
      createdAt: NOW
    };
    try {
      await store.upsertOperations(LIBRARY_A, [task]);
      assert.equal(await store.applyLocalRecoveryBatch(batch, NOW), true);
      assert.equal(await store.applyLocalRecoveryBatch(batch, NOW + 1), false);
      assert.equal(
        (
          await store.getOperation(
            LIBRARY_A.id,
            `renderer-error:${LIBRARY_A.id}:${report.source}:${report.code}:${report.target}`
          )
        )?.occurrenceCount,
        2
      );
      assert.equal(
        (await store.getOperation(LIBRARY_A.id, task.operationId))?.errorDetail,
        "action failed"
      );
      assert.equal((await store.compactSnapshot()).thumbnailSummaries[0]?.cachedCount, 70);
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
            operation(`completed:${index}`, 1, "failed", {
              updatedAt: timestamp,
              completedAt: timestamp,
              retryable: true
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
      assert.equal(compact.retryableFailures.length, 2_000);
      const history = await store.history({ limit: 100, offset: 0 });
      assert.equal(history.total, 2_001);
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
      assert.equal(inspection.prepare("PRAGMA user_version").get()?.user_version, 4);
      const tables = inspection
        .prepare(
          "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        )
        .all()
        .map((row) => row.name);
      assert.deepEqual(tables, [
        "action_errors",
        "legacy_imports",
        "local_recovery_batches",
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
      assert.equal(upgraded.prepare("PRAGMA user_version").get()?.user_version, 4);
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
    const stores = createTrackedTestStores(databasePath, [TEST_WORKER_URL, TEST_WORKER_URL]);
    try {
      const settled = await settleTrackedStoreReadiness(stores);
      assert.ok(
        settled.every((result) => result.status === "fulfilled"),
        `concurrent open failed: ${settled
          .filter((result): result is PromiseRejectedResult => result.status === "rejected")
          .map((result) => String(result.reason))
          .join("; ")}`
      );
      const [first, second] = stores;
      await first.applyOutboxBatch(
        LIBRARY_A,
        [event(1, operation("concurrent-init", 1, "running"))],
        1
      );
      assert.equal(await second.cursor(LIBRARY_A.id), 1);
    } finally {
      await closeTrackedStores(stores);
    }
  });

  it("stably initializes WAL across repeated concurrent first opens", async () => {
    for (let round = 0; round < 64; round += 1) {
      const databasePath = testDatabasePath();
      const stores = createTrackedTestStores(databasePath, [TEST_WORKER_URL, TEST_WORKER_URL]);
      let settled: PromiseSettledResult<NotificationStore>[] = [];
      try {
        settled = await settleTrackedStoreReadiness(stores);
        assert.ok(
          settled.every((result) => result.status === "fulfilled"),
          `round ${round} concurrent open failed: ${settled
            .filter((result): result is PromiseRejectedResult => result.status === "rejected")
            .map((result) => String(result.reason))
            .join("; ")}`
        );
      } finally {
        await closeTrackedStores(stores);
      }

      const inspection = new DatabaseSync(databasePath, { readOnly: true });
      try {
        assert.equal(inspection.prepare("PRAGMA journal_mode").get()?.journal_mode, "wal");
      } finally {
        inspection.close();
      }
    }
  });

  it("fault-injected partial open releases every MessagePort for natural process exit", async () => {
    const baselinePorts = activeMessagePortCount();
    const failingWorkerUrl = dataWorkerUrl(`
      import { parentPort } from "node:worker_threads";
      parentPort.postMessage({
        type: "startupError",
        error: { name: "InjectedStartupError", message: "injected startup failure" }
      });
      setInterval(() => {}, 1_000);
    `);
    const stores = createTrackedTestStores(testDatabasePath(), [TEST_WORKER_URL, failingWorkerUrl]);
    try {
      const settled = await settleTrackedStoreReadiness(stores);
      assert.equal(settled.filter((result) => result.status === "fulfilled").length, 1);
      assert.equal(settled.filter((result) => result.status === "rejected").length, 1);
    } finally {
      await closeTrackedStores(stores);
    }
    await new Promise<void>((resolve) => setImmediate(resolve));
    assert.equal(activeMessagePortCount(), baselinePorts);
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

  it("bounds startup when a worker never reports ready", async () => {
    const workerUrl = dataWorkerUrl(`
      setInterval(() => {}, 1_000);
    `);
    const startedAt = Date.now();
    await assert.rejects(
      createNotificationStore({
        databasePath: testDatabasePath(),
        workerUrl,
        startupTimeoutMs: 30
      }),
      /startup.*timed out/i
    );
    assert.ok(Date.now() - startedAt < 500, "startup timeout must terminate the worker promptly");
  });

  it("aborts startup and terminates a worker that never reports ready", async () => {
    const workerUrl = dataWorkerUrl(`
      setInterval(() => {}, 1_000);
    `);
    const controller = new AbortController();
    const opening = createNotificationStore({
      databasePath: testDatabasePath(),
      workerUrl,
      startupTimeoutMs: 5_000,
      signal: controller.signal
    });
    controller.abort(new DOMException("Library switched", "AbortError"));
    await assert.rejects(opening, /Library switched|AbortError/i);
  });

  it("close rejects ready and terminates a never-ready worker", async () => {
    const workerUrl = dataWorkerUrl(`
      setInterval(() => {}, 1_000);
    `);
    const store = new NotificationStore({
      databasePath: testDatabasePath(),
      workerUrl,
      startupTimeoutMs: 5_000
    });
    const ready = store.ready();
    const readyRejected = assert.rejects(ready, /closing|closed/i);
    await store.close();
    await readyRejected;
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
