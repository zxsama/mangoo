import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { describe, it } from "node:test";
import type {
  FileOperationRecord,
  NotificationOutboxAckResponse
} from "@megle/core-client/generated-contract";

import {
  createNotificationService,
  type NotificationServiceStore
} from "../src/notification-service.ts";
import {
  createNotificationStore,
  notificationDatabasePath
} from "../src/notification-store.ts";
import type {
  DesktopNotificationOperation,
  NotificationCompactSnapshot,
  NotificationHistoryPage,
  NotificationHistoryQuery,
  NotificationLibraryIdentity,
  NotificationLocalRecoveryBatch,
  NotificationOperationInput,
  NotificationRendererErrorRecordResult,
  NotificationStoreEvent,
  NotificationStoredSnapshot,
  RendererErrorReport,
  ThumbnailCacheSummary
} from "../src/notification-types.ts";

const NOW = Date.UTC(2026, 6, 20, 12);
const LIBRARY_A: NotificationLibraryIdentity = { id: "library-a", name: "Library A" };
const LIBRARY_B: NotificationLibraryIdentity = { id: "library-b", name: "Library B" };
const SESSION_A = {
  baseUrl: "http://127.0.0.1:48101/api",
  sessionToken: "token-a"
};
const SESSION_B = {
  baseUrl: "http://127.0.0.1:48102/api",
  sessionToken: "token-b"
};
const TEST_WORKER_URL = new URL("../src/notification-store-worker.ts", import.meta.url);

function testDatabasePath(): string {
  return notificationDatabasePath(fs.mkdtempSync(path.join(os.tmpdir(), "megle-service-")));
}

function operation(
  operationId: string,
  revision = 1,
  status: NotificationOperationInput["status"] = "running",
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
    title: operationId,
    summary: null,
    taskId: 7,
    rootId: 3,
    folderId: null,
    fileId: null,
    currentValue: terminal ? 10 : 2,
    totalValue: 10,
    unit: "items",
    errorCode: status === "failed" ? "failed" : null,
    errorDetail: status === "failed" ? "failed" : null,
    cancellable: !terminal,
    retryable: status === "failed",
    attempt: 1,
    occurrenceCount: 1,
    startedAt: NOW - 1_000,
    updatedAt: NOW,
    completedAt: terminal ? NOW : null,
    ...overrides
  };
}

function event(sequence: number, value = operation(`task:${sequence}`)): NotificationStoreEvent {
  return { sequence, operation: value };
}

function fileOperationRecord(
  overrides: Partial<FileOperationRecord> = {}
): FileOperationRecord {
  return {
    id: 1,
    operation: "move",
    sourcePath: "source",
    targetPath: "target",
    status: "succeeded",
    createdAt: NOW - 1_000,
    finishedAt: NOW - 500,
    error: null,
    ...overrides
  };
}

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" }
  });
}

function ackResponse(body: NotificationOutboxAckResponse): Response {
  return response(body);
}

function rendererErrorOperationIdForTest(report: RendererErrorReport): string {
  return `renderer-error:${report.libraryId}:${report.source}:${report.code}:${report.target}`;
}

function emptySnapshot(): NotificationStoredSnapshot {
  return {
    active: [],
    thumbnailSummaries: [],
    latestImportantResults: [],
    retryableFailures: []
  };
}

class FakeStore implements NotificationServiceStore {
  readonly operations = new Map<string, DesktopNotificationOperation>();
  readonly cursors = new Map<string, number>();
  readonly acknowledgements = new Map<string, number>();
  readonly summaries = new Map<string, ThumbnailCacheSummary>();
  readonly imported = new Set<string>();
  readonly recoveryBatches = new Set<string>();
  readonly rendererReportIds = new Set<string>();
  readonly calls: string[] = [];
  readonly getOperationCalls: string[] = [];
  cleanupCalls = 0;
  failApply = false;
  failActionError = false;
  failLegacyImport = false;
  failSnapshot = false;

  async applyOutboxBatch(
    library: NotificationLibraryIdentity,
    events: NotificationStoreEvent[],
    maxSequence: number
  ): Promise<void> {
    this.calls.push("apply");
    if (this.failApply) throw new Error("store offline");
    await this.upsertOperations(
      library,
      events.map((item) => item.operation)
    );
    this.cursors.set(library.id, Math.max(this.cursors.get(library.id) ?? 0, maxSequence));
  }

  async upsertOperations(
    library: NotificationLibraryIdentity,
    operations: NotificationOperationInput[]
  ): Promise<void> {
    for (const value of operations) {
      const key = `${library.id}/${value.operationId}`;
      const existing = this.operations.get(key);
      if (!existing || value.revision > existing.revision) {
        this.operations.set(key, { ...value, libraryId: library.id, libraryName: library.name });
      }
    }
  }

  async cursor(libraryId: string): Promise<number> {
    return this.cursors.get(libraryId) ?? 0;
  }

  async syncState(libraryId: string): Promise<{ persistedSequence: number; ackedSequence: number }> {
    return {
      persistedSequence: this.cursors.get(libraryId) ?? 0,
      ackedSequence: this.acknowledgements.get(libraryId) ?? 0
    };
  }

  async markAcknowledged(libraryId: string, sequence: number): Promise<void> {
    this.acknowledgements.set(
      libraryId,
      Math.max(this.acknowledgements.get(libraryId) ?? 0, sequence)
    );
  }

  async compactSnapshot(): Promise<NotificationStoredSnapshot> {
    if (this.failSnapshot) throw new Error("snapshot offline");
    const all = [...this.operations.values()];
    const retryableFailures = all
      .filter(
        (item) =>
          item.status !== "pending" &&
          item.status !== "running" &&
          item.status === "failed" &&
          item.retryable
      )
      .map((item) => ({
        libraryId: item.libraryId,
        operationId: item.operationId,
        revision: item.revision
      }))
      .sort((left, right) =>
        left.libraryId.localeCompare(right.libraryId) ||
        left.operationId.localeCompare(right.operationId)
      );
    return {
      active: all
        .filter((item) => item.status === "pending" || item.status === "running")
        .sort((a, b) => b.updatedAt - a.updatedAt),
      thumbnailSummaries: [...this.summaries.values()],
      latestImportantResults: all
        .filter(
          (item) =>
            item.status !== "pending" &&
            item.status !== "running" &&
            item.presentation === "important"
        )
        .sort((a, b) => (b.completedAt ?? b.updatedAt) - (a.completedAt ?? a.updatedAt))
        .slice(0, 3),
      retryableFailures,
    };
  }

  async history(query: NotificationHistoryQuery): Promise<NotificationHistoryPage> {
    const offset = query.offset ?? 0;
    const limit = query.limit ?? 50;
    let items = [...this.operations.values()].filter(
      (item) => item.status !== "pending" && item.status !== "running"
    );
    if (query.libraryId) items = items.filter((item) => item.libraryId === query.libraryId);
    if (query.statuses) items = items.filter((item) => query.statuses?.includes(item.status));
    if (query.categories) items = items.filter((item) => query.categories?.includes(item.category));
    if (query.search) {
      const search = query.search.toLowerCase();
      items = items.filter((item) => `${item.title} ${item.summary ?? ""}`.toLowerCase().includes(search));
    }
    items.sort((a, b) => (b.completedAt ?? b.updatedAt) - (a.completedAt ?? a.updatedAt));
    return {
      items: items.slice(offset, offset + limit),
      total: items.length,
      nextOffset: offset + limit < items.length ? offset + limit : null
    };
  }

  async historyFacets() {
    const libraries = new Map<string, string>();
    const kinds = new Set<string>();
    for (const item of this.operations.values()) {
      libraries.set(item.libraryId, item.libraryName);
      kinds.add(item.kind);
    }
    return {
      libraries: [...libraries]
        .map(([id, name]) => ({ id, name }))
        .sort((left, right) => left.name.localeCompare(right.name)),
      kinds: [...kinds].sort()
    };
  }

  async getOperation(libraryId: string, operationId: string): Promise<DesktopNotificationOperation | null> {
    this.getOperationCalls.push(`${libraryId}/${operationId}`);
    return this.operations.get(`${libraryId}/${operationId}`) ?? null;
  }

  async setOperationActionError(
    libraryId: string,
    operationId: string,
    expectedRevision: number,
    detail: string | null
  ): Promise<DesktopNotificationOperation | null> {
    if (this.failActionError) throw new Error("action store offline");
    const key = `${libraryId}/${operationId}`;
    const existing = this.operations.get(key);
    if (!existing || existing.revision !== expectedRevision) return null;
    const updated = { ...existing, errorDetail: detail };
    this.operations.set(key, updated);
    return updated;
  }

  async publishThumbnailSummary(summary: ThumbnailCacheSummary): Promise<void> {
    const existing = this.summaries.get(summary.libraryId);
    if (!existing || summary.observedAt >= existing.observedAt) {
      this.summaries.set(summary.libraryId, summary);
    }
  }

  async applyLocalRecoveryBatch(
    batch: NotificationLocalRecoveryBatch
  ): Promise<boolean> {
    if (this.recoveryBatches.has(batch.batchId)) return false;
    for (const pending of batch.rendererErrors) {
      for (const reportId of pending.reportIds) {
        await this.recordRendererError(
          pending.library,
          pending.report,
          reportId,
          pending.occurredAt
        );
      }
    }
    for (const summary of batch.thumbnails) await this.publishThumbnailSummary(summary);
    for (const pending of batch.actionErrors) {
      await this.setOperationActionError(
        pending.libraryId,
        pending.operationId,
        pending.expectedRevision,
        pending.detail
      );
    }
    this.recoveryBatches.add(batch.batchId);
    return true;
  }

  async recordRendererError(
    library: NotificationLibraryIdentity,
    report: RendererErrorReport,
    reportId: string,
    occurredAt: number
  ): Promise<NotificationRendererErrorRecordResult> {
    const operationId = `renderer-error:${library.id}:${report.source}:${report.code}:${report.target}`;
    const key = `${library.id}/${operationId}`;
    const existing = this.operations.get(key);
    if (this.rendererReportIds.has(reportId)) {
      return { outcome: "duplicate", operation: existing ?? null };
    }
    this.rendererReportIds.add(reportId);
    const value = operation(operationId, (existing?.revision ?? 0) + 1, "failed", {
      kind: "renderer_error",
      category: "error",
      origin: "system",
      taskId: null,
      rootId: null,
      title: report.title,
      summary: report.message,
      errorCode: report.code,
      errorDetail: report.detail,
      occurrenceCount: (existing?.occurrenceCount ?? 0) + 1,
      startedAt: existing?.startedAt ?? occurredAt,
      updatedAt: occurredAt,
      completedAt: occurredAt,
      cancellable: false,
      retryable: false
    });
    const desktop = { ...value, libraryId: library.id, libraryName: library.name };
    this.operations.set(key, desktop);
    return { outcome: "applied", operation: desktop };
  }

  async hasLegacyImport(libraryId: string): Promise<boolean> {
    return this.imported.has(libraryId);
  }

  async importLegacyOperations(
    library: NotificationLibraryIdentity,
    operations: NotificationOperationInput[]
  ): Promise<boolean> {
    if (this.imported.has(library.id)) return false;
    if (this.failLegacyImport) throw new Error("legacy marker failed");
    const fresh = operations.filter(
      (value) => !this.operations.has(`${library.id}/${value.operationId}`)
    );
    await this.upsertOperations(library, fresh);
    this.imported.add(library.id);
    return true;
  }

  async cleanupRetention(): Promise<{ deletedExpired: number; deletedOverflow: number }> {
    this.cleanupCalls += 1;
    return { deletedExpired: 0, deletedOverflow: 0 };
  }
}

function thumbnailSummary(library = LIBRARY_A): ThumbnailCacheSummary {
  return {
    libraryId: library.id,
    libraryName: library.name,
    observedAt: NOW,
    cachedCount: 80,
    totalCandidateCount: 100,
    missingCount: 20,
    staleCount: 0,
    failedCount: 0,
    pendingCount: 0,
    activeBulkTaskCount: 0,
    totalCacheBytes: 1_024,
    completionPercent: 80
  };
}

describe("notification service", () => {
  it("advances ackedSequence only after true ACK and continues with the next GET", async () => {
    const calls: string[] = [];
    const store = new FakeStore();
    let listCalls = 0;
    const originalApply = store.applyOutboxBatch.bind(store);
    store.applyOutboxBatch = async (...args) => {
      await originalApply(...args);
      calls.push("store");
    };
    const service = await createNotificationService({
      store,
      fetch: async (input, init) => {
        const url = String(input);
        if (url.endsWith("/notifications/outbox/ack")) {
          calls.push("ack");
          assert.equal(init?.headers && new Headers(init.headers).get("x-megle-session"), "token-a");
          return ackResponse({ acknowledged: true });
        }
        listCalls += 1;
        return response(
          listCalls === 1 ? { items: [event(1)], maxSequence: 1 } : { items: [], maxSequence: 1 }
        );
      },
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.deepEqual(calls, ["store", "ack"]);
    assert.deepEqual(await store.syncState(LIBRARY_A.id), {
      persistedSequence: 1,
      ackedSequence: 1
    });
    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.equal(listCalls, 2, "a successful ACK must leave the local cursor ready for the next GET");
    assert.deepEqual(await store.syncState(LIBRARY_A.id), {
      persistedSequence: 1,
      ackedSequence: 1
    });
    await service.close();
  });

  it("retries a failed acknowledgement without reapplying the committed batch", async () => {
    const store = new FakeStore();
    let listCalls = 0;
    let ackCalls = 0;
    const service = await createNotificationService({
      store,
      fetch: async (input) => {
        if (String(input).endsWith("/notifications/outbox/ack")) {
          ackCalls += 1;
          return ackCalls === 1 ? response({ error: "offline" }, 503) : ackResponse({ acknowledged: true });
        }
        listCalls += 1;
        return response(
          listCalls === 1 ? { items: [event(1)], maxSequence: 1 } : { items: [], maxSequence: 1 }
        );
      },
      now: () => NOW
    });

    await assert.rejects(service.runOneDrainForTest(LIBRARY_A, SESSION_A), /503/);
    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.equal(store.calls.filter((call) => call === "apply").length, 1);
    assert.equal(ackCalls, 2);
    await service.close();
  });

  it("does not advance ackedSequence for false or malformed ACK responses", async () => {
    const store = new FakeStore();
    let ackCalls = 0;
    let listCalls = 0;
    const service = await createNotificationService({
      store,
      fetch: async (input) => {
        if (String(input).endsWith("/notifications/outbox/ack")) {
          ackCalls += 1;
          if (ackCalls === 1) return ackResponse({ acknowledged: false });
          if (ackCalls === 2) return response({ acknowledged: "yes" });
          return ackResponse({ acknowledged: true });
        }
        listCalls += 1;
        return response(
          listCalls === 1 ? { items: [event(1)], maxSequence: 1 } : { items: [], maxSequence: 1 }
        );
      },
      now: () => NOW
    });

    await assert.rejects(
      service.runOneDrainForTest(LIBRARY_A, SESSION_A),
      /did not acknowledge/i
    );
    assert.deepEqual(await store.syncState(LIBRARY_A.id), {
      persistedSequence: 1,
      ackedSequence: 0
    });
    await assert.rejects(
      service.runOneDrainForTest(LIBRARY_A, SESSION_A),
      /invalid notification outbox acknowledgement/i
    );
    assert.deepEqual(await store.syncState(LIBRARY_A.id), {
      persistedSequence: 1,
      ackedSequence: 0
    });
    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.equal(listCalls, 2, "a true retry must proceed to GET after the ACK is accepted");
    assert.deepEqual(await store.syncState(LIBRARY_A.id), {
      persistedSequence: 1,
      ackedSequence: 1
    });
    await service.close();
  });

  it("persists an acknowledgement intent across a real store restart", async () => {
    const databasePath = testDatabasePath();
    const firstStore = await createNotificationStore({
      databasePath,
      workerUrl: TEST_WORKER_URL
    });
    let firstAckCalls = 0;
    const firstService = await createNotificationService({
      store: firstStore,
      fetch: async (input) => {
        if (String(input).endsWith("/notifications/outbox/ack")) {
          firstAckCalls += 1;
          return response({ error: "offline" }, 503);
        }
        return response({ items: [event(1)], maxSequence: 1 });
      },
      now: () => NOW
    });
    let secondService: Awaited<ReturnType<typeof createNotificationService>> | null = null;
    try {
      await assert.rejects(firstService.runOneDrainForTest(LIBRARY_A, SESSION_A), /503/);
      assert.deepEqual(await firstStore.syncState(LIBRARY_A.id), {
        persistedSequence: 1,
        ackedSequence: 0
      });
      assert.equal(firstAckCalls, 1);
      await firstService.close();

      const secondStore = await createNotificationStore({
        databasePath,
        workerUrl: TEST_WORKER_URL
      });
      const calls: string[] = [];
      secondService = await createNotificationService({
        store: secondStore,
        fetch: async (input) => {
          if (String(input).endsWith("/notifications/outbox/ack")) {
            calls.push("ack");
            return ackResponse({ acknowledged: true });
          }
          calls.push("get");
          return response({ items: [], maxSequence: 1 });
        },
        now: () => NOW
      });

      await secondService.runOneDrainForTest(LIBRARY_A, SESSION_A);
      assert.deepEqual(calls, ["ack", "get"], "restart must finish the durable ACK before GET");
      assert.deepEqual(await secondStore.syncState(LIBRARY_A.id), {
        persistedSequence: 1,
        ackedSequence: 1
      });
      await secondService.runOneDrainForTest(LIBRARY_A, SESSION_A);
      assert.deepEqual(calls, ["ack", "get", "get"], "an acknowledged cursor must not be replayed");
      assert.equal((await secondStore.compactSnapshot()).active.length, 1);
    } finally {
      await firstService.close();
      await secondService?.close();
    }
  });

  it("replays a true ACK after restart when Core already removed the Outbox row", async () => {
    const databasePath = testDatabasePath();
    const firstStore = await createNotificationStore({
      databasePath,
      workerUrl: TEST_WORKER_URL
    });
    const firstService = await createNotificationService({
      store: firstStore,
      fetch: async (input) =>
        String(input).endsWith("/notifications/outbox/ack")
          ? ackResponse({ acknowledged: true })
          : response({ items: [event(1)], maxSequence: 1 }),
      now: () => NOW
    });
    const originalMarkAcknowledged = firstStore.markAcknowledged.bind(firstStore);
    firstStore.markAcknowledged = async () => {
      throw new Error("simulated crash after Core ACK");
    };
    try {
      await assert.rejects(
        firstService.runOneDrainForTest(LIBRARY_A, SESSION_A),
        /simulated crash after Core ACK/i
      );
    } finally {
      firstStore.markAcknowledged = originalMarkAcknowledged;
      await firstService.close();
    }

    const secondStore = await createNotificationStore({
      databasePath,
      workerUrl: TEST_WORKER_URL
    });
    const calls: string[] = [];
    const secondService = await createNotificationService({
      store: secondStore,
      fetch: async (input) => {
        if (String(input).endsWith("/notifications/outbox/ack")) {
          calls.push("ack");
          return ackResponse({ acknowledged: true });
        }
        calls.push("get");
        return response({ items: [], maxSequence: 1 });
      },
      now: () => NOW
    });
    try {
      await secondService.runOneDrainForTest(LIBRARY_A, SESSION_A);
      assert.deepEqual(calls, ["ack", "get"]);
      assert.deepEqual(await secondStore.syncState(LIBRARY_A.id), {
        persistedSequence: 1,
        ackedSequence: 1
      });
      await secondService.runOneDrainForTest(LIBRARY_A, SESSION_A);
      assert.deepEqual(calls, ["ack", "get", "get"], "repeat after idempotent ACK must not ACK again");
    } finally {
      await secondService.close();
    }
  });

  it("does not acknowledge when the durable store rejects a batch", async () => {
    const store = new FakeStore();
    store.failApply = true;
    let ackCalls = 0;
    const service = await createNotificationService({
      store,
      fetch: async (input) => {
        if (String(input).endsWith("/notifications/outbox/ack")) {
          ackCalls += 1;
          return ackResponse({ acknowledged: true });
        }
        return response({ items: [event(1)], maxSequence: 1 });
      },
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    const snapshot = await service.compactSnapshot();
    assert.equal(ackCalls, 0);
    assert.equal(snapshot.syncHealth.state, "degraded");
    assert.equal(snapshot.active[0]?.operationId, "task:1");
    await service.close();
  });

  it("aborts the previous long poll on library switch and never starts duplicate loops", async () => {
    const store = new FakeStore();
    const started: string[] = [];
    const aborted: string[] = [];
    let active = 0;
    let maxActive = 0;
    const service = await createNotificationService({
      store,
      fetch: async (input, init) => {
        const url = String(input);
        if (url.includes("/notifications/outbox?") && init?.signal) {
          const library = url.includes("48101") ? "a" : "b";
          started.push(library);
          active += 1;
          maxActive = Math.max(maxActive, active);
          return new Promise<Response>((_resolve, reject) => {
            init.signal?.addEventListener(
              "abort",
              () => {
                active -= 1;
                aborted.push(library);
                reject(init.signal?.reason ?? new DOMException("Aborted", "AbortError"));
              },
              { once: true }
            );
          });
        }
        return ackResponse({ acknowledged: true });
      },
      retryDelay: async (_delay, signal) => {
        if (signal.aborted) throw signal.reason;
      },
      now: () => NOW
    });

    await service.attachLibrary(LIBRARY_A, SESSION_A);
    await service.attachLibrary(LIBRARY_A, SESSION_A);
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(started, ["a"]);
    await service.attachLibrary(LIBRARY_B, SESSION_B);
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(started, ["a", "b"]);
    assert.deepEqual(aborted, ["a"]);
    assert.equal(maxActive, 1);
    await service.close();
  });

  it("aggregates contract-shaped legacy rows by operationGroupId", async () => {
    const store = new FakeStore();
    const requested: string[] = [];
    const recent = NOW - 1_000;
    const service = await createNotificationService({
      store,
      fetch: async (input) => {
        const url = new URL(String(input));
        requested.push(url.searchParams.get("cursor") ?? "first");
        return response({
          items: [
            fileOperationRecord({
              id: 9,
              operationGroupId: "file-group:9",
              operation: "delete_recycle",
              sourcePath: "a",
              targetPath: null,
              createdAt: recent,
              finishedAt: recent
            }),
            fileOperationRecord({
              id: 8,
              operationGroupId: "file-group:9",
              operation: "delete_recycle",
              sourcePath: "b",
              targetPath: null,
              status: "failed",
              createdAt: recent - 1,
              finishedAt: recent,
              error: "locked"
            })
          ],
          nextCursor: null
        });
      },
      now: () => NOW
    });

    await service.runLegacyImportForTest(LIBRARY_A, SESSION_A);
    await service.runLegacyImportForTest(LIBRARY_A, SESSION_A);
    assert.deepEqual(requested, ["first"]);
    assert.equal(store.imported.has(LIBRARY_A.id), true);
    assert.equal(store.operations.size, 1);
    const grouped = store.operations.get(`${LIBRARY_A.id}/file-group:9`);
    assert.equal(grouped?.totalValue, 2);
    assert.equal(grouped?.status, "failed");
    assert.equal(grouped?.revision, 0, "legacy group snapshots must yield to Core revisions");
    await service.close();
  });

  it("summarizes truly ungrouped legacy rows once for the library", async () => {
    const store = new FakeStore();
    const service = await createNotificationService({
      store,
      fetch: async () =>
        response({
          items: [
            fileOperationRecord({ id: 4, operation: "move" }),
            fileOperationRecord({ id: 3, operation: "move", status: "failed", error: "busy" }),
            fileOperationRecord({ id: 2, operation: "rename" }),
            fileOperationRecord({ id: 1, operation: "delete_recycle", targetPath: null })
          ],
          nextCursor: null
        }),
      now: () => NOW
    });

    await service.runLegacyImportForTest(LIBRARY_A, SESSION_A);
    assert.equal(store.operations.size, 1);
    const summary = store.operations.get(`${LIBRARY_A.id}/legacy-import:${LIBRARY_A.id}`);
    assert.equal(summary?.kind, "legacy_file_operations");
    assert.equal(summary?.presentation, "history_only");
    assert.equal(summary?.totalValue, 4);
    assert.equal(summary?.status, "failed");
    assert.match(summary?.summary ?? "", /move: 2/i);
    assert.match(summary?.summary ?? "", /rename: 1/i);
    assert.match(summary?.summary ?? "", /recycle bin: 1/i);
    await service.close();
  });

  it("does not duplicate a legacy group already represented by an Outbox operation", async () => {
    const store = new FakeStore();
    await store.upsertOperations(LIBRARY_A, [
      operation("file-group:existing", 3, "succeeded", {
        kind: "move",
        category: "file_operation"
      })
    ]);
    const service = await createNotificationService({
      store,
      fetch: async () =>
        response({
          items: [
            fileOperationRecord({
              id: 1,
              operationGroupId: "file-group:existing",
              operation: "move",
              sourcePath: "a",
              targetPath: "b",
              status: "succeeded",
              createdAt: NOW - 100,
              finishedAt: NOW - 50,
              error: null
            })
          ],
          nextCursor: null
        }),
      now: () => NOW
    });

    await service.runLegacyImportForTest(LIBRARY_A, SESSION_A);
    assert.equal(store.operations.size, 1);
    assert.equal(store.operations.get(`${LIBRARY_A.id}/file-group:existing`)?.revision, 3);
    assert.deepEqual(store.getOperationCalls, [`${LIBRARY_A.id}/file-group:existing`]);
    await service.close();
  });

  it("imports 1,000 ungrouped rows through bounded pages into one summary", async () => {
    const store = new FakeStore();
    const rows = Array.from({ length: 1_000 }, (_, index) =>
      fileOperationRecord({
        id: 1_000 - index,
        operation: index % 2 === 0 ? "move" : "rename",
        sourcePath: `source-${index}`,
        targetPath: `target-${index}`,
        status: index === 999 ? "failed" : "succeeded",
        createdAt: NOW - 1_000 - index,
        finishedAt: NOW - 500 - index,
        error: index === 999 ? "busy" : null
      })
    );
    const requestedLimits: string[] = [];
    const service = await createNotificationService({
      store,
      fetch: async (input) => {
        const url = new URL(String(input));
        requestedLimits.push(url.searchParams.get("limit") ?? "missing");
        const offset = Number(url.searchParams.get("cursor") ?? 0);
        const items = rows.slice(offset, offset + 200);
        const nextOffset = offset + items.length;
        return response({
          items,
          nextCursor: nextOffset < rows.length ? String(nextOffset) : null
        });
      },
      now: () => NOW
    });

    await service.runLegacyImportForTest(LIBRARY_A, SESSION_A);
    assert.deepEqual(requestedLimits, ["200", "200", "200", "200", "200"]);
    assert.equal(store.operations.size, 1);
    const summary = store.operations.get(`${LIBRARY_A.id}/legacy-import:${LIBRARY_A.id}`);
    assert.equal(summary?.totalValue, 1_000);
    assert.match(summary?.summary ?? "", /999 succeeded.*1 failed/i);
    await service.close();
  });

  it("rolls back a failed legacy marker and retries safely after the cutoff changes", async () => {
    const store = new FakeStore();
    store.failLegacyImport = true;
    let now = NOW;
    const cutoffEdge = NOW - 30 * 24 * 60 * 60 * 1_000 + 1;
    const service = await createNotificationService({
      store,
      fetch: async () =>
        response({
          items: [
            fileOperationRecord({
              id: 1,
              operation: "move",
              createdAt: cutoffEdge,
              finishedAt: cutoffEdge,
              error: null
            })
          ],
          nextCursor: null
        }),
      now: () => now
    });

    await assert.rejects(
      service.runLegacyImportForTest(LIBRARY_A, SESSION_A),
      /legacy marker failed/i
    );
    assert.equal(store.operations.size, 0);
    assert.equal(store.imported.has(LIBRARY_A.id), false);

    store.failLegacyImport = false;
    now += 2;
    await service.runLegacyImportForTest(LIBRARY_A, SESSION_A);
    assert.equal(store.operations.size, 0, "the now-expired row must not conflict with the retry");
    assert.equal(store.imported.has(LIBRARY_A.id), true);
    await service.close();
  });

  it("proxies task actions only for the active library", async () => {
    const store = new FakeStore();
    store.imported.add(LIBRARY_A.id);
    await store.upsertOperations(LIBRARY_A, [
      operation("task:7", 2, "failed", { taskId: 7, cancellable: true, retryable: true })
    ]);
    const actionPaths: string[] = [];
    const service = await createNotificationService({
      store,
      fetch: async (input, init) => {
        const url = String(input);
        if (url.includes("/notifications/outbox?")) {
          return new Promise<Response>((_resolve, reject) => {
            init?.signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")), { once: true });
          });
        }
        actionPaths.push(new URL(url).pathname);
        return response({ taskId: 7 });
      },
      now: () => NOW
    });
    await service.attachLibrary(LIBRARY_A, SESSION_A);

    assert.equal((await service.cancelOperation(LIBRARY_B.id, "task:7")).ok, false);
    assert.equal((await service.cancelOperation(LIBRARY_A.id, "task:7")).ok, true);
    assert.equal((await service.retryOperation(LIBRARY_A.id, "task:7")).ok, true);
    assert.deepEqual(actionPaths, ["/api/tasks/7/cancel", "/api/tasks/7/retry"]);
    await service.close();
  });

  it("projects action capabilities to the active library and refreshes them after switching", async () => {
    const store = new FakeStore();
    store.imported.add(LIBRARY_A.id);
    store.imported.add(LIBRARY_B.id);
    await store.upsertOperations(LIBRARY_A, [
      operation("a:active", 1, "running", { cancellable: true, retryable: false }),
      operation("a:failed", 1, "failed", { cancellable: false, retryable: true })
    ]);
    await store.upsertOperations(LIBRARY_B, [
      operation("b:active", 1, "running", { cancellable: true, retryable: false }),
      operation("b:failed", 1, "failed", { cancellable: false, retryable: true })
    ]);
    const service = await createNotificationService({
      store,
      fetch: async (_input, init) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener(
            "abort",
            () => reject(init.signal?.reason ?? new DOMException("Aborted", "AbortError")),
            { once: true }
          );
        }),
      now: () => NOW
    });
    const pushed: NotificationCompactSnapshot[] = [];
    const unsubscribe = service.subscribe((snapshot) => pushed.push(snapshot));

    await service.attachLibrary(LIBRARY_A, SESSION_A);
    let compact = await service.compactSnapshot();
    assert.equal(compact.active.find((item) => item.libraryId === LIBRARY_A.id)?.cancellable, true);
    assert.equal(compact.active.find((item) => item.libraryId === LIBRARY_B.id)?.cancellable, false);
    let history = await service.history({ statuses: ["failed"], limit: 20, offset: 0 });
    assert.equal(history.items.find((item) => item.libraryId === LIBRARY_A.id)?.retryable, true);
    assert.equal(history.items.find((item) => item.libraryId === LIBRARY_B.id)?.retryable, false);

    await service.attachLibrary(LIBRARY_B, SESSION_B);
    compact = await service.compactSnapshot();
    assert.equal(compact.active.find((item) => item.libraryId === LIBRARY_A.id)?.cancellable, false);
    assert.equal(compact.active.find((item) => item.libraryId === LIBRARY_B.id)?.cancellable, true);
    history = await service.history({ statuses: ["failed"], limit: 20, offset: 0 });
    assert.equal(history.items.find((item) => item.libraryId === LIBRARY_A.id)?.retryable, false);
    assert.equal(history.items.find((item) => item.libraryId === LIBRARY_B.id)?.retryable, true);
    assert.equal(
      pushed.at(-1)?.active.find((item) => item.libraryId === LIBRARY_B.id)?.cancellable,
      true,
      "the library switch broadcast must refresh projected capabilities"
    );

    unsubscribe();
    await service.close();
  });

  it("does not POST a task action when the library switches after operation lookup starts", async () => {
    const store = new FakeStore();
    store.imported.add(LIBRARY_A.id);
    store.imported.add(LIBRARY_B.id);
    await store.upsertOperations(LIBRARY_A, [
      operation("task:library-race", 1, "running", { taskId: 7, cancellable: true })
    ]);
    let lookupStarted!: () => void;
    const lookupReached = new Promise<void>((resolve) => {
      lookupStarted = resolve;
    });
    let releaseLookup!: () => void;
    const lookupBarrier = new Promise<void>((resolve) => {
      releaseLookup = resolve;
    });
    const originalGetOperation = store.getOperation.bind(store);
    store.getOperation = async (...args) => {
      lookupStarted();
      await lookupBarrier;
      return originalGetOperation(...args);
    };
    let actionPosts = 0;
    const service = await createNotificationService({
      store,
      fetch: async (input, init) => {
        const url = String(input);
        if (url.includes("/notifications/outbox?")) {
          return new Promise<Response>((_resolve, reject) => {
            init?.signal?.addEventListener(
              "abort",
              () => reject(init.signal?.reason ?? new DOMException("Aborted", "AbortError")),
              { once: true }
            );
          });
        }
        actionPosts += 1;
        return response({ taskId: 7 });
      },
      now: () => NOW
    });
    await service.attachLibrary(LIBRARY_A, SESSION_A);

    const action = service.cancelOperation(LIBRARY_A.id, "task:library-race");
    await lookupReached;
    const switched = service.attachLibrary(LIBRARY_B, SESSION_B);
    await new Promise((resolve) => setImmediate(resolve));
    releaseLookup();
    await switched;

    assert.equal((await action).ok, false);
    assert.equal(actionPosts, 0, "a stale action must not reach either Core session");
    await service.close();
  });

  it("records action failures on the existing operation and clears them after success", async () => {
    const store = new FakeStore();
    store.imported.add(LIBRARY_A.id);
    await store.upsertOperations(LIBRARY_A, [
      operation("task:action", 2, "running", {
        taskId: 7,
        cancellable: true,
        retryable: false,
        errorDetail: null
      })
    ]);
    let actionCalls = 0;
    const service = await createNotificationService({
      store,
      fetch: async (input, init) => {
        const url = String(input);
        if (url.includes("/notifications/outbox?")) {
          return new Promise<Response>((_resolve, reject) => {
            init?.signal?.addEventListener(
              "abort",
              () => reject(new DOMException("Aborted", "AbortError")),
              { once: true }
            );
          });
        }
        actionCalls += 1;
        return actionCalls === 1
          ? response({ error: "Core refused cancellation" }, 409)
          : response({ taskId: 7 });
      },
      now: () => NOW
    });
    await service.attachLibrary(LIBRARY_A, SESSION_A);

    const failed = await service.cancelOperation(LIBRARY_A.id, "task:action");
    assert.equal(failed.ok, false);
    assert.match((await service.compactSnapshot()).active[0]?.errorDetail ?? "", /409/);

    const succeeded = await service.cancelOperation(LIBRARY_A.id, "task:action");
    assert.equal(succeeded.ok, true);
    assert.equal((await service.compactSnapshot()).active[0]?.errorDetail, null);

    await store.setOperationActionError(LIBRARY_A.id, "task:action", 2, "stale action error");
    await store.upsertOperations(LIBRARY_A, [operation("task:action", 3, "running")]);
    assert.equal((await store.getOperation(LIBRARY_A.id, "task:action"))?.errorDetail, null);
    await service.close();
  });

  it("replays a degraded action error into the recovered store without a new operation", async () => {
    const failedStore = new FakeStore();
    failedStore.imported.add(LIBRARY_A.id);
    failedStore.failActionError = true;
    const recoveredStore = new FakeStore();
    recoveredStore.imported.add(LIBRARY_A.id);
    const input = operation("task:recover-action", 2, "running", {
      taskId: 7,
      cancellable: true,
      errorDetail: null
    });
    await failedStore.upsertOperations(LIBRARY_A, [input]);
    await recoveredStore.upsertOperations(LIBRARY_A, [input]);
    let listCalls = 0;
    const service = await createNotificationService({
      store: failedStore,
      openStore: async () => recoveredStore,
      fetch: async (inputUrl, init) => {
        const url = String(inputUrl);
        if (url.includes("/notifications/outbox?")) {
          listCalls += 1;
          if (listCalls === 1) {
            return new Promise<Response>((_resolve, reject) => {
              init?.signal?.addEventListener(
                "abort",
                () => reject(new DOMException("Aborted", "AbortError")),
                { once: true }
              );
            });
          }
          return response({ items: [], maxSequence: 0 });
        }
        return response({ error: "Core refused cancellation" }, 409);
      },
      now: () => NOW
    });
    try {
      await service.attachLibrary(LIBRARY_A, SESSION_A);
      await new Promise((resolve) => setImmediate(resolve));
      assert.equal(listCalls, 1);
      const failed = await service.cancelOperation(LIBRARY_A.id, "task:recover-action");
      assert.equal(failed.ok, false);
      assert.match((await service.compactSnapshot()).active[0]?.errorDetail ?? "", /409/);

      await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
      const recovered = await recoveredStore.getOperation(
        LIBRARY_A.id,
        "task:recover-action"
      );
      assert.match(recovered?.errorDetail ?? "", /409/);
      assert.equal(recoveredStore.operations.size, 1);
    } finally {
      await service.close();
    }
  });

  it("serializes reports and action overlays injected while local recovery is flushing", async () => {
    const failedStore = new FakeStore();
    failedStore.imported.add(LIBRARY_A.id);
    failedStore.failActionError = true;
    const task = operation("task:recovery-barrier", 2, "running", {
      taskId: 7,
      cancellable: true,
      errorDetail: null
    });
    await failedStore.upsertOperations(LIBRARY_A, [task]);
    const recoveredStore = new FakeStore();
    recoveredStore.imported.add(LIBRARY_A.id);
    await recoveredStore.upsertOperations(LIBRARY_A, [task]);
    let flushStarted!: () => void;
    const flushReached = new Promise<void>((resolve) => {
      flushStarted = resolve;
    });
    let releaseFlush!: () => void;
    const flushBarrier = new Promise<void>((resolve) => {
      releaseFlush = resolve;
    });
    const originalApplyRecovery = recoveredStore.applyLocalRecoveryBatch.bind(recoveredStore);
    let recoveryCalls = 0;
    recoveredStore.applyLocalRecoveryBatch = async (batch) => {
      recoveryCalls += 1;
      if (recoveryCalls === 1) {
        flushStarted();
        await flushBarrier;
      }
      return originalApplyRecovery(batch);
    };
    const initialError: RendererErrorReport = {
      libraryId: LIBRARY_A.id,
      libraryName: LIBRARY_A.name,
      source: "recovery",
      code: "initial",
      target: "folder:1",
      title: "Initial error",
      message: "initial",
      detail: null
    };
    const injectedError = { ...initialError, code: "injected", title: "Injected error" };
    const service = await createNotificationService({
      store: failedStore,
      openStore: async () => recoveredStore,
      fetch: async (input, init) => {
        const url = String(input);
        if (url.includes("/notifications/outbox?")) {
          if (init?.signal) {
            return new Promise<Response>((resolve, reject) => {
              const timeoutId = setTimeout(
                () => resolve(response({ items: [], maxSequence: 0 })),
                5
              );
              init.signal?.addEventListener(
                "abort",
                () => {
                  clearTimeout(timeoutId);
                  reject(init.signal?.reason ?? new DOMException("Aborted", "AbortError"));
                },
                { once: true }
              );
            });
          }
          return response({ items: [], maxSequence: 0 });
        }
        return response({ error: "Core refused cancellation" }, 409);
      },
      now: () => NOW
    });
    await service.attachLibrary(LIBRARY_A, SESSION_A);
    assert.equal((await service.cancelOperation(LIBRARY_A.id, task.operationId)).ok, false);
    await service.reportRendererError(initialError);
    await service.publishThumbnailSummary({ ...thumbnailSummary(), observedAt: 100 });

    const recovering = service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    await flushReached;
    const injectedReports = Promise.all([
      service.reportRendererError(injectedError),
      service.publishThumbnailSummary({ ...thumbnailSummary(), observedAt: 200 }),
      service.cancelOperation(LIBRARY_A.id, task.operationId)
    ]);
    releaseFlush();
    await recovering;
    await injectedReports;

    assert.ok(
      await recoveredStore.getOperation(
        LIBRARY_A.id,
        `renderer-error:${LIBRARY_A.id}:recovery:initial:folder:1`
      )
    );
    assert.ok(
      await recoveredStore.getOperation(
        LIBRARY_A.id,
        `renderer-error:${LIBRARY_A.id}:recovery:injected:folder:1`
      )
    );
    assert.equal(recoveredStore.summaries.get(LIBRARY_A.id)?.observedAt, 200);
    assert.match(
      (await recoveredStore.getOperation(LIBRARY_A.id, task.operationId))?.errorDetail ?? "",
      /409/
    );
    await service.close();
  });

  it("retains an idempotent local recovery batch when snapshot activation fails", async () => {
    const recoveredStore = new FakeStore();
    recoveredStore.failSnapshot = true;
    let opens = 0;
    const report: RendererErrorReport = {
      libraryId: LIBRARY_A.id,
      libraryName: LIBRARY_A.name,
      source: "recovery",
      code: "snapshot-failed",
      target: "folder:9",
      title: "Snapshot failed",
      message: "snapshot failed",
      detail: null
    };
    const service = await createNotificationService({
      openStore: async () => {
        opens += 1;
        if (opens === 1) throw new Error("initial worker unavailable");
        return recoveredStore;
      },
      fetch: async () => response({ items: [], maxSequence: 0 }),
      now: () => NOW
    });
    await service.reportRendererError(report);
    await service.reportRendererError(report);
    await service.publishThumbnailSummary(thumbnailSummary());

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    const degraded = await service.compactSnapshot();
    assert.equal(degraded.syncHealth.state, "degraded");
    assert.equal(degraded.latestImportantResults[0]?.occurrenceCount, 2);
    assert.equal(degraded.thumbnailSummaries.length, 1);

    recoveredStore.failSnapshot = false;
    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    const operationId = rendererErrorOperationIdForTest(report);
    assert.equal(
      (await recoveredStore.getOperation(LIBRARY_A.id, operationId))?.occurrenceCount,
      2,
      "replaying a committed recovery batch must not double-count reports"
    );
    assert.equal(recoveredStore.recoveryBatches.size, 1);
    assert.equal((await service.compactSnapshot()).syncHealth.state, "healthy");
    await service.close();
  });

  it("deduplicates renderer errors, publishes thumbnail summaries, and broadcasts snapshots", async () => {
    const store = new FakeStore();
    const service = await createNotificationService({ store, now: () => NOW });
    const reports: NotificationCompactSnapshot[] = [];
    const unsubscribe = service.subscribe((snapshot) => reports.push(snapshot));
    const rendererError: RendererErrorReport = {
      libraryId: LIBRARY_A.id,
      libraryName: LIBRARY_A.name,
      source: "library-data",
      code: "load-failed",
      target: "folder:4",
      title: "Folder load failed",
      message: "Could not load folder",
      detail: "database busy"
    };

    await service.reportRendererError(rendererError);
    await service.reportRendererError(rendererError);
    await service.publishThumbnailSummary(thumbnailSummary());

    const history = await service.history({ limit: 20, offset: 0 });
    assert.equal(history.total, 1);
    assert.equal(history.items[0]?.occurrenceCount, 2);
    assert.equal((await service.compactSnapshot()).thumbnailSummaries.length, 1);
    assert.ok(reports.length >= 3);
    unsubscribe();
    await service.close();
  });

  it("keeps the store healthy when a retained renderer marker has no operation", async () => {
    const store = new FakeStore();
    store.rendererReportIds.add("retained-report");
    const service = await createNotificationService({
      store,
      createReportId: () => "retained-report",
      now: () => NOW
    });

    await service.reportRendererError({
      libraryId: LIBRARY_A.id,
      libraryName: LIBRARY_A.name,
      source: "renderer",
      code: "retained-marker",
      target: "folder:retained",
      title: "Retained renderer marker",
      message: "Its operation has already been evicted",
      detail: null
    });

    assert.equal((await service.compactSnapshot()).syncHealth.state, "healthy");
    assert.equal((await service.history({ limit: 20, offset: 0 })).total, 0);
    await service.close();
  });

  it("keeps committed local reports visible when snapshot refresh fails", async () => {
    const store = new FakeStore();
    const service = await createNotificationService({ store, now: () => NOW });
    store.failSnapshot = true;
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

    await service.reportRendererError(report);
    let snapshot = await service.compactSnapshot();
    assert.equal(snapshot.syncHealth.state, "degraded");
    assert.equal(snapshot.latestImportantResults[0]?.operationId.includes("renderer-error"), true);

    await service.publishThumbnailSummary(thumbnailSummary());
    snapshot = await service.compactSnapshot();
    assert.equal(snapshot.thumbnailSummaries[0]?.libraryId, LIBRARY_A.id);
    await service.close();
  });

  it("keeps thumbnail fallback and recovery monotonic when an older summary arrives", async () => {
    const recoveredStore = new FakeStore();
    let opens = 0;
    const service = await createNotificationService({
      openStore: async () => {
        opens += 1;
        if (opens === 1) throw new Error("worker unavailable");
        return recoveredStore;
      },
      fetch: async () => response({ items: [], maxSequence: 0 }),
      now: () => NOW
    });
    await service.publishThumbnailSummary({ ...thumbnailSummary(), observedAt: 200 });
    await service.publishThumbnailSummary({ ...thumbnailSummary(), observedAt: 100 });
    assert.equal((await service.compactSnapshot()).thumbnailSummaries[0]?.observedAt, 200);

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.equal(recoveredStore.summaries.get(LIBRARY_A.id)?.observedAt, 200);
    await service.close();
  });

  it("keeps a bounded 500-operation fallback and reports degraded health", async () => {
    const service = await createNotificationService({
      openStore: async () => {
        throw new Error("worker unavailable");
      },
      now: () => NOW
    });
    for (let index = 0; index < 510; index += 1) {
      await service.reportRendererError({
        libraryId: LIBRARY_A.id,
        libraryName: LIBRARY_A.name,
        source: "test",
        code: `error-${index}`,
        target: `target-${index}`,
        title: `Error ${index}`,
        message: "failed",
        detail: null
      });
    }

    const history = await service.history({ limit: 200, offset: 0 });
    assert.equal(history.total, 500);
    assert.equal((await service.compactSnapshot()).syncHealth.state, "degraded");
    await service.close();
  });

  it("keeps retryable failure state in degraded fallback beyond latest results", async () => {
    const service = await createNotificationService({
      openStore: async () => {
        throw new Error("worker unavailable");
      },
      fetch: async () =>
        response({
          items: [
            event(
              1,
              operation("failed:fallback", 1, "failed", {
                updatedAt: NOW - 100_000,
                completedAt: NOW - 100_000,
                retryable: true
              })
            ),
            event(2, operation("success:1", 1, "succeeded", { updatedAt: NOW - 3_000 })),
            event(3, operation("success:2", 1, "succeeded", { updatedAt: NOW - 2_000 })),
            event(4, operation("success:3", 1, "succeeded", { updatedAt: NOW - 1_000 }))
          ],
          maxSequence: 4
        }),
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    const snapshot = await service.compactSnapshot();
    assert.deepEqual(
      snapshot.latestImportantResults.map((item) => item.operationId),
      ["success:3", "success:2", "success:1"]
    );
    assert.equal(snapshot.hasRetryableFailure, true);
    await service.close();
  });

  it("projects 2000 internal failure references into one bounded renderer boolean", async () => {
    const store = new FakeStore();
    await store.upsertOperations(
      LIBRARY_A,
      Array.from({ length: 2_000 }, (_, index) =>
        operation(`failed:compact:${index}`, 1, "failed", { retryable: true })
      )
    );
    const service = await createNotificationService({ store, now: () => NOW });

    const snapshot = await service.compactSnapshot();
    assert.equal(snapshot.hasRetryableFailure, true);
    assert.equal("retryableFailures" in snapshot, false);
    assert.ok(
      Buffer.byteLength(JSON.stringify(snapshot), "utf8") < 16_384,
      "renderer compact payload must stay independent from internal failure count"
    );
    await service.close();
  });

  it("clears a stored failure when fallback receives a newer success and keeps it clear after recovery", async () => {
    const store = new FakeStore();
    await store.upsertOperations(LIBRARY_A, [
      operation("failed:durable", 1, "failed", { retryable: true })
    ]);
    store.failApply = true;
    let acknowledged = false;
    const success = event(
      1,
      operation("failed:durable", 2, "succeeded", { retryable: false })
    );
    const service = await createNotificationService({
      store,
      openStore: async () => store,
      fetch: async (input) => {
        if (String(input).endsWith("/notifications/outbox/ack")) {
          acknowledged = true;
          return ackResponse({ acknowledged: true });
        }
        return response(
          acknowledged
            ? { items: [], maxSequence: 1 }
            : { items: [success], maxSequence: 1 }
        );
      },
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    let snapshot = await service.compactSnapshot();
    assert.equal(snapshot.hasRetryableFailure, false);

    store.failApply = false;
    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    snapshot = await service.compactSnapshot();
    assert.equal(store.operations.get(`${LIBRARY_A.id}/failed:durable`)?.revision, 2);
    assert.equal(snapshot.hasRetryableFailure, false);
    await service.close();
  });

  it("reconciles retryable failures through a newer running revision and later failure", async () => {
    const createDegradedSnapshot = async (events: NotificationStoreEvent[]) => {
      const store = new FakeStore();
      await store.upsertOperations(LIBRARY_A, [
        operation("failed:retry", 1, "failed", { retryable: true })
      ]);
      store.failApply = true;
      const service = await createNotificationService({
        store,
        fetch: async () => response({ items: events, maxSequence: events.at(-1)?.sequence ?? 0 }),
        now: () => NOW
      });
      await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
      const snapshot = await service.compactSnapshot();
      await service.close();
      return snapshot;
    };

    const running = await createDegradedSnapshot([
      event(
        1,
        operation("failed:retry", 2, "running", {
          completedAt: null,
          retryable: false
        })
      )
    ]);
    assert.equal(running.hasRetryableFailure, false);

    const failedAgain = await createDegradedSnapshot([
      event(
        1,
        operation("failed:retry", 2, "running", {
          completedAt: null,
          retryable: false
        })
      ),
      event(2, operation("failed:retry", 3, "failed", { retryable: true }))
    ]);
    assert.equal(failedAgain.hasRetryableFailure, true);
  });

  it("rejects conflicting fallback payloads at the same revision", async () => {
    const failed = operation("failed:conflict", 1, "failed", {
      currentValue: 4,
      errorDetail: "original failure",
      retryable: true
    });
    const runFallback = async (operations: NotificationOperationInput[]) => {
      const events = operations.map((item, index) => event(index + 1, item));
      const service = await createNotificationService({
        openStore: async () => {
          throw new Error("worker unavailable");
        },
        fetch: async () => response({ items: events, maxSequence: events.length }),
        now: () => NOW
      });
      await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
      const compact = await service.compactSnapshot();
      const history = await service.history({ limit: 20, offset: 0 });
      await service.close();
      return { compact, history };
    };

    const conflict = await runFallback([
      failed,
      {
        ...failed,
        status: "succeeded",
        currentValue: 99,
        errorDetail: "conflicting detail",
        retryable: false
      }
    ]);
    assert.equal(conflict.compact.hasRetryableFailure, true);
    assert.equal(conflict.compact.latestImportantResults[0]?.status, "failed");
    assert.equal(conflict.compact.latestImportantResults[0]?.currentValue, 4);
    assert.equal(conflict.compact.latestImportantResults[0]?.errorDetail, "original failure");
    assert.equal(conflict.history.total, 1);
    assert.equal(conflict.history.items[0]?.status, "failed");
    assert.equal(conflict.history.items[0]?.currentValue, 4);
    assert.equal(conflict.history.items[0]?.errorDetail, "original failure");
    assert.match(conflict.compact.syncHealth.detail ?? "", /conflict.*revision 1/i);

    const identical = await runFallback([failed, { ...failed }]);
    assert.equal(identical.history.total, 1);
    assert.equal(identical.history.items[0]?.status, "failed");
    assert.equal(identical.compact.hasRetryableFailure, true);

    const succeeded = await runFallback([
      failed,
      operation("failed:conflict", 2, "succeeded", {
        currentValue: 100,
        errorDetail: null,
        retryable: false
      })
    ]);
    assert.equal(succeeded.compact.hasRetryableFailure, false);
    assert.equal(succeeded.compact.latestImportantResults[0]?.status, "succeeded");
    assert.equal(succeeded.history.items[0]?.revision, 2);
    assert.equal(succeeded.history.items[0]?.status, "succeeded");
  });

  it("does not clear a stored failure with an older or same-revision fallback success", async () => {
    const store = new FakeStore();
    await store.upsertOperations(LIBRARY_A, [
      operation("failed:durable", 1, "failed", { retryable: true })
    ]);
    store.failApply = true;
    const service = await createNotificationService({
      store,
      fetch: async () =>
        response({
          items: [
            event(
              1,
              operation("failed:durable", 0, "succeeded", { retryable: false })
            ),
            event(
              2,
              operation("failed:durable", 1, "succeeded", { retryable: false })
            )
          ],
          maxSequence: 2
        }),
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    const snapshot = await service.compactSnapshot();
    assert.equal(snapshot.hasRetryableFailure, true);
    await service.close();
  });

  it("keeps another stored failure when fallback makes only one operation non-retryable", async () => {
    const store = new FakeStore();
    await store.upsertOperations(LIBRARY_A, [
      operation("failed:first", 1, "failed", { retryable: true }),
      operation("failed:second", 1, "failed", { retryable: true })
    ]);
    store.failApply = true;
    const service = await createNotificationService({
      store,
      fetch: async () =>
        response({
          items: [
            event(
              1,
              operation("failed:first", 2, "failed", { retryable: false })
            )
          ],
          maxSequence: 1
        }),
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    const snapshot = await service.compactSnapshot();
    assert.equal(snapshot.hasRetryableFailure, true);
    await service.close();
  });

  it("caps fallback-origin retryable references with the 500 logical-record limit", async () => {
    const failures = Array.from({ length: 510 }, (_, index) =>
      event(
        index + 1,
        operation(`failed:fallback:${index}`, 1, "failed", { retryable: true })
      )
    );
    const resolutions = Array.from({ length: 500 }, (_, index) =>
      event(
        index + 511,
        operation(`failed:fallback:${index + 10}`, 2, "succeeded", {
          retryable: false
        })
      )
    );
    const events = [...failures, ...resolutions];
    const service = await createNotificationService({
      openStore: async () => {
        throw new Error("worker unavailable");
      },
      fetch: async () => response({ items: events, maxSequence: events.length }),
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    const snapshot = await service.compactSnapshot();
    const history = await service.history({ limit: 200, offset: 0 });
    assert.equal(history.total, 500);
    assert.equal(snapshot.hasRetryableFailure, false);
    await service.close();
  });

  it("evicts only fallback-origin failure references and adopts durable state on recovery", async () => {
    const failedStore = new FakeStore();
    await failedStore.upsertOperations(LIBRARY_A, [
      operation("failed:durable", 1, "failed", { retryable: true })
    ]);
    failedStore.failApply = true;

    const recoveredStore = new FakeStore();
    await recoveredStore.upsertOperations(LIBRARY_B, [
      operation("failed:recovered", 4, "failed", { retryable: true })
    ]);
    const failureEvents: NotificationStoreEvent[] = [
      event(1, operation("failed:reused", 1, "failed", { retryable: true })),
      ...Array.from({ length: 500 }, (_, index) =>
        event(
          index + 2,
          operation(`failed:unique:${index}`, 1, "failed", { retryable: true })
        )
      )
    ];
    const resolutionEvents: NotificationStoreEvent[] = [
      event(502, operation("failed:reused", 2, "succeeded", { retryable: false })),
      ...Array.from({ length: 499 }, (_, index) =>
        event(
          index + 503,
          operation(`failed:unique:${index + 1}`, 2, "succeeded", { retryable: false })
        )
      )
    ];
    const events = [...failureEvents, ...resolutionEvents];
    let listCalls = 0;
    const service = await createNotificationService({
      store: failedStore,
      openStore: async () => recoveredStore,
      fetch: async (input) => {
        if (String(input).endsWith("/notifications/outbox/ack")) {
          return ackResponse({ acknowledged: true });
        }
        listCalls += 1;
        return response(
          listCalls === 1
            ? { items: events, maxSequence: events.length }
            : { items: [], maxSequence: 0 }
        );
      },
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    let snapshot = await service.compactSnapshot();
    assert.equal((await service.history({ limit: 200, offset: 0 })).total, 500);
    assert.equal(snapshot.hasRetryableFailure, true);
    assert.ok(
      (await service.history({ limit: 200, offset: 0 })).items.every(
        (item) => item.status === "succeeded"
      ),
      "only the unoverridden durable baseline should keep the failure boolean set"
    );

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    snapshot = await service.compactSnapshot();
    assert.equal(snapshot.hasRetryableFailure, true);
    assert.deepEqual(
      (await service.history({ limit: 20, offset: 0 })).items.map((item) => item.operationId),
      ["failed:recovered"]
    );
    await service.close();
  });

  it("caps fallback and pending records globally across many libraries and releases them after recovery", async () => {
    const recoveredStore = new FakeStore();
    let opens = 0;
    const service = await createNotificationService({
      openStore: async () => {
        opens += 1;
        if (opens === 1) throw new Error("worker unavailable");
        return recoveredStore;
      },
      fetch: async () => response({ items: [], maxSequence: 0 }),
      now: () => NOW
    });
    for (let index = 0; index < 260; index += 1) {
      const library = { id: `library-${index}`, name: `Library ${index}` };
      await service.reportRendererError({
        libraryId: library.id,
        libraryName: library.name,
        source: "cap",
        code: `error-${index}`,
        target: "library",
        title: `Error ${index}`,
        message: "failed",
        detail: null
      });
      await service.publishThumbnailSummary({ ...thumbnailSummary(library), observedAt: index });
    }

    const degradedHistory = await service.history({ limit: 200, offset: 0 });
    const degradedSnapshot = await service.compactSnapshot();
    assert.ok(
      degradedHistory.total + degradedSnapshot.thumbnailSummaries.length <= 500,
      "all fallback record kinds must share one logical cap"
    );

    recoveredStore.failSnapshot = true;
    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.equal((await service.compactSnapshot()).syncHealth.state, "degraded");
    await service.reportRendererError({
      libraryId: "library-overflow",
      libraryName: "Library Overflow",
      source: "cap",
      code: "overflow",
      target: "library",
      title: "Overflow",
      message: "overflow",
      detail: null
    });
    recoveredStore.failSnapshot = false;
    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.ok(
      recoveredStore.operations.size + recoveredStore.summaries.size <= 500,
      "the bounded pending state must be all that migrates into the recovered store"
    );
    assert.equal((await service.compactSnapshot()).syncHealth.state, "healthy");
    await service.close();
  });

  it("replays an unacknowledged fallback batch after the store recovers", async () => {
    const failedStore = new FakeStore();
    failedStore.failApply = true;
    const recoveredStore = new FakeStore();
    recoveredStore.imported.add(LIBRARY_A.id);
    let opens = 0;
    let acknowledgements = 0;
    const service = await createNotificationService({
      store: failedStore,
      openStore: async () => {
        opens += 1;
        return recoveredStore;
      },
      fetch: async (input) => {
        if (String(input).endsWith("/notifications/outbox/ack")) {
          acknowledgements += 1;
          return ackResponse({ acknowledged: true });
        }
        return response(
          acknowledgements > 0
            ? { items: [], maxSequence: 1 }
            : { items: [event(1)], maxSequence: 1 }
        );
      },
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.equal(opens, 1);
    assert.equal(await recoveredStore.cursor(LIBRARY_A.id), 1);
    assert.equal(acknowledgements, 1);
    assert.equal((await service.compactSnapshot()).syncHealth.state, "healthy");
    await service.close();
  });

  it("adds degraded renderer occurrences to an existing durable error after recovery", async () => {
    const recoveredStore = new FakeStore();
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
    for (let index = 0; index < 5; index += 1) {
      await recoveredStore.recordRendererError(
        LIBRARY_A,
        report,
        `recovered-report-${index}`,
        NOW - 100 + index
      );
    }
    let opens = 0;
    const service = await createNotificationService({
      openStore: async () => {
        opens += 1;
        if (opens === 1) throw new Error("worker unavailable");
        return recoveredStore;
      },
      fetch: async () => response({ items: [], maxSequence: 0 }),
      now: () => NOW
    });
    await service.reportRendererError(report);
    await service.reportRendererError(report);

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    const stored = await recoveredStore.getOperation(
      LIBRARY_A.id,
      `renderer-error:${LIBRARY_A.id}:${report.source}:${report.code}:${report.target}`
    );
    assert.equal(stored?.occurrenceCount, 7);
    await service.close();
  });

  it("replays a committed renderer report with the same id after its response is lost", async () => {
    const databasePath = testDatabasePath();
    const firstStore = await createNotificationStore({
      databasePath,
      workerUrl: TEST_WORKER_URL
    });
    let closePromise = Promise.resolve();
    const lossyStore = new Proxy(firstStore, {
      get(target, property, receiver) {
        if (property === "recordRendererError") {
          return async (...args: unknown[]) => {
            await Reflect.apply(target.recordRendererError, target, args);
            throw new Error("worker response lost after commit");
          };
        }
        if (property === "close") {
          return () => {
            closePromise = target.close();
            return closePromise;
          };
        }
        const value = Reflect.get(target, property, receiver) as unknown;
        return typeof value === "function" ? value.bind(target) : value;
      }
    }) as NotificationServiceStore;
    const report: RendererErrorReport = {
      libraryId: LIBRARY_A.id,
      libraryName: LIBRARY_A.name,
      source: "renderer",
      code: "response-lost",
      target: "folder:7",
      title: "Response lost",
      message: "The worker response was lost",
      detail: null
    };
    let recoveredStore: Awaited<ReturnType<typeof createNotificationStore>> | null = null;
    const service = await createNotificationService({
      store: lossyStore,
      openStore: async (signal) => {
        await closePromise;
        recoveredStore = await createNotificationStore({
          databasePath,
          workerUrl: TEST_WORKER_URL,
          signal
        });
        return recoveredStore;
      },
      fetch: async () => response({ items: [], maxSequence: 0 }),
      createReportId: () => "stable-report-id",
      now: () => NOW
    });
    try {
      await service.reportRendererError(report);
      await service.runOneDrainForTest(LIBRARY_A, SESSION_A);

      assert.ok(recoveredStore);
      assert.equal(
        (
          await recoveredStore.getOperation(
            LIBRARY_A.id,
            rendererErrorOperationIdForTest(report)
          )
        )?.occurrenceCount,
        1,
        "recovery must not count a report whose transaction already committed"
      );
    } finally {
      await service.close();
    }
  });

  it("runs the one-time legacy import after a degraded store recovers", async () => {
    const recoveredStore = new FakeStore();
    let opens = 0;
    const service = await createNotificationService({
      openStore: async () => {
        opens += 1;
        if (opens === 1) throw new Error("worker unavailable");
        return recoveredStore;
      },
      fetch: async (input) =>
        String(input).includes("/file-ops?")
          ? response({ items: [], nextCursor: null })
          : response({ items: [], maxSequence: 0 }),
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(recoveredStore.imported.has(LIBRARY_A.id), true);
    await service.close();
  });

  it("backs off store recovery instead of busy-looping while a batch remains unacknowledged", async () => {
    const store = new FakeStore();
    store.failApply = true;
    let listCalls = 0;
    let delayStarted!: () => void;
    const delayed = new Promise<void>((resolve) => {
      delayStarted = resolve;
    });
    const service = await createNotificationService({
      store,
      openStore: async () => {
        throw new Error("worker unavailable");
      },
      fetch: async (input) => {
        if (String(input).includes("/file-ops?")) {
          return response({ items: [], nextCursor: null });
        }
        listCalls += 1;
        return response({ items: [event(1)], maxSequence: 1 });
      },
      retryDelay: async (_delay, signal) => {
        delayStarted();
        await new Promise<void>((_resolve, reject) => {
          signal.addEventListener("abort", () => reject(signal.reason), { once: true });
        });
      },
      now: () => NOW
    });

    await service.attachLibrary(LIBRARY_A, SESSION_A);
    await delayed;
    assert.equal(listCalls, 1);
    await service.close();
  });

  it("retries a degraded store on bounded backoff even when the Outbox is empty", async () => {
    let opens = 0;
    let listCalls = 0;
    let delayStarted!: () => void;
    const delayed = new Promise<void>((resolve) => {
      delayStarted = resolve;
    });
    const service = await createNotificationService({
      openStore: async () => {
        opens += 1;
        throw new Error("worker unavailable");
      },
      fetch: async () => {
        listCalls += 1;
        await new Promise((resolve) => setImmediate(resolve));
        return response({ items: [], maxSequence: 0 });
      },
      retryDelay: async (delay, signal) => {
        assert.equal(delay, 250);
        delayStarted();
        await new Promise<void>((_resolve, reject) => {
          signal.addEventListener("abort", () => reject(signal.reason), { once: true });
        });
      },
      now: () => NOW
    });

    try {
      await service.attachLibrary(LIBRARY_A, SESSION_A);
      await Promise.race([
        delayed,
        new Promise<never>((_resolve, reject) =>
          setTimeout(() => reject(new Error("degraded retry did not enter backoff")), 250)
        )
      ]);
      assert.ok(opens >= 2, "the service must retry opening the store");
      assert.equal(listCalls, 1, "an empty degraded GET must not busy-loop");
    } finally {
      await service.close();
    }
  });

  it("cancels a never-ready recovery worker on library switch and close", async () => {
    const workerUrl = new URL(
      "data:text/javascript,setInterval%28%28%29%3D%3E%7B%7D%2C1000%29",
      import.meta.url
    );
    let opens = 0;
    let recoveryStarted!: () => void;
    const recoveryReached = new Promise<void>((resolve) => {
      recoveryStarted = resolve;
    });
    const service = await createNotificationService({
      openStore: async (signal) => {
        opens += 1;
        if (opens === 1) throw new Error("initial worker unavailable");
        recoveryStarted();
        return createNotificationStore({
          databasePath: testDatabasePath(),
          workerUrl,
          startupTimeoutMs: 5_000,
          signal
        });
      },
      fetch: async () => response({ items: [], maxSequence: 0 }),
      now: () => NOW
    });
    await service.attachLibrary(LIBRARY_A, SESSION_A);
    await recoveryReached;

    await Promise.race([
      service.attachLibrary(LIBRARY_B, SESSION_B),
      new Promise<never>((_resolve, reject) =>
        setTimeout(() => reject(new Error("library switch waited for never-ready worker")), 500)
      )
    ]);
    await Promise.race([
      service.close(),
      new Promise<never>((_resolve, reject) =>
        setTimeout(() => reject(new Error("close waited for never-ready worker")), 500)
      )
    ]);
    assert.ok(opens >= 3, "the switched library should own a fresh recovery attempt");
  });

  it("runs retention at startup and throttles cleanup after durable writes", async () => {
    const store = new FakeStore();
    let now = NOW;
    const service = await createNotificationService({
      store,
      fetch: async (input) =>
        String(input).endsWith("/notifications/outbox/ack")
          ? ackResponse({ acknowledged: true })
          : response({ items: [event(1)], maxSequence: 1 }),
      now: () => now
    });
    assert.equal(store.cleanupCalls, 1);

    await service.publishThumbnailSummary(thumbnailSummary());
    await service.reportRendererError({
      libraryId: LIBRARY_A.id,
      libraryName: LIBRARY_A.name,
      source: "retention",
      code: "first",
      target: "test",
      title: "test",
      message: "test",
      detail: null
    });
    assert.equal(store.cleanupCalls, 1, "same-cadence writes must not rescan retention");

    now += 24 * 60 * 60 * 1_000;
    await service.publishThumbnailSummary({ ...thumbnailSummary(), observedAt: now });
    await service.reportRendererError({
      libraryId: LIBRARY_A.id,
      libraryName: LIBRARY_A.name,
      source: "retention",
      code: "second",
      target: "test",
      title: "test",
      message: "test",
      detail: null
    });
    assert.equal(store.cleanupCalls, 2, "only the first write after the cadence runs cleanup");

    now += 24 * 60 * 60 * 1_000;
    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.equal(store.cleanupCalls, 3, "the first durable Outbox batch after cadence cleans up");
    await service.close();
  });

  it("runs throttled retention after empty long polls while the service is idle", async () => {
    const store = new FakeStore();
    let now = NOW;
    const service = await createNotificationService({
      store,
      fetch: async () => response({ items: [], maxSequence: 0 }),
      now: () => now
    });
    assert.equal(store.cleanupCalls, 1);

    now += 5 * 60 * 1_000 + 1;
    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.equal(store.cleanupCalls, 2, "an empty poll must still enforce retention cadence");
    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.equal(store.cleanupCalls, 2, "empty polls inside the cadence must stay throttled");
    await service.close();
  });

  it("rejects unsafe operation revisions before they enter the fallback", async () => {
    const service = await createNotificationService({
      openStore: async () => {
        throw new Error("worker unavailable");
      },
      fetch: async () =>
        response({
          items: [event(1, operation("unsafe", Number.MAX_SAFE_INTEGER + 1))],
          maxSequence: 1
        }),
      now: () => NOW
    });

    await assert.rejects(service.runOneDrainForTest(LIBRARY_A, SESSION_A), /revision.*safe integer/i);
    assert.equal((await service.history({ limit: 20, offset: 0 })).total, 0);
    await service.close();
  });

  it("keeps degraded fallback state when snapshot refresh fails after a durable commit", async () => {
    const store = new FakeStore();
    const originalApply = store.applyOutboxBatch.bind(store);
    store.applyOutboxBatch = async (...args) => {
      await originalApply(...args);
      store.failSnapshot = true;
    };
    let acknowledgements = 0;
    const service = await createNotificationService({
      store,
      fetch: async (input) => {
        if (String(input).endsWith("/notifications/outbox/ack")) {
          acknowledgements += 1;
          return ackResponse({ acknowledged: true });
        }
        return response({ items: [event(1)], maxSequence: 1 });
      },
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    const snapshot = await service.compactSnapshot();
    assert.equal(acknowledgements, 0, "fallback mode must never acknowledge Core");
    assert.equal(snapshot.syncHealth.state, "degraded");
    assert.equal(snapshot.active[0]?.operationId, "task:1");
    await service.close();
  });

  it("serializes concurrent library attachments so only the final library owns a loop", async () => {
    const store = new FakeStore();
    store.imported.add(LIBRARY_A.id);
    store.imported.add(LIBRARY_B.id);
    const started: string[] = [];
    let activeRequests = 0;
    let maxActiveRequests = 0;
    const service = await createNotificationService({
      store,
      fetch: async (input, init) => {
        const libraryId = String(input).includes("48101") ? "a" : "b";
        started.push(libraryId);
        activeRequests += 1;
        maxActiveRequests = Math.max(maxActiveRequests, activeRequests);
        return new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener(
            "abort",
            () => {
              activeRequests -= 1;
              reject(init.signal?.reason ?? new DOMException("Aborted", "AbortError"));
            },
            { once: true }
          );
        });
      },
      now: () => NOW
    });

    await Promise.all([
      service.attachLibrary(LIBRARY_A, SESSION_A),
      service.attachLibrary(LIBRARY_B, SESSION_B)
    ]);
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(started.at(-1), "b");
    assert.equal(maxActiveRequests, 1);
    await service.close();
  });

  it("persists an inactive library fallback batch after recovery and defers its ack", async () => {
    const failedStore = new FakeStore();
    failedStore.failApply = true;
    const recoveredStore = new FakeStore();
    let aLists = 0;
    let aAcks = 0;
    const service = await createNotificationService({
      store: failedStore,
      openStore: async () => recoveredStore,
      fetch: async (input) => {
        const url = String(input);
        if (url.endsWith("/notifications/outbox/ack")) {
          if (url.includes("48101")) aAcks += 1;
          return ackResponse({ acknowledged: true });
        }
        if (url.includes("48101")) {
          aLists += 1;
          return response(
            aLists === 1 ? { items: [event(1)], maxSequence: 1 } : { items: [], maxSequence: 1 }
          );
        }
        return response({ items: [], maxSequence: 0 });
      },
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    await service.runOneDrainForTest(LIBRARY_B, SESSION_B);
    assert.equal(await recoveredStore.cursor(LIBRARY_A.id), 1);
    assert.equal(aAcks, 0, "an inactive Core session must not be acknowledged through another library");

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.equal(aAcks, 1);
    await service.close();
  });

  it("keeps history query validation identical while the durable store is degraded", async () => {
    const service = await createNotificationService({
      openStore: async () => {
        throw new Error("worker unavailable");
      },
      now: () => NOW
    });

    await assert.rejects(service.history({ limit: 0, offset: 0 }), /limit/i);
    await assert.rejects(service.history({ limit: 20, offset: -1 }), /offset/i);
    await service.close();
  });

  it("isolates subscriber failures from synchronization and other listeners", async () => {
    const store = new FakeStore();
    const service = await createNotificationService({ store, now: () => NOW });
    let healthyListenerCalls = 0;
    service.subscribe(() => {
      throw new Error("listener closed");
    });
    service.subscribe(() => {
      healthyListenerCalls += 1;
    });

    await service.publishThumbnailSummary(thumbnailSummary());
    assert.ok(healthyListenerCalls >= 2);
    await service.close();
  });
});
