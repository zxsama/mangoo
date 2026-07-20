import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  createNotificationService,
  type NotificationServiceStore
} from "../src/notification-service.ts";
import type {
  DesktopNotificationOperation,
  NotificationCompactSnapshot,
  NotificationHistoryPage,
  NotificationHistoryQuery,
  NotificationLibraryIdentity,
  NotificationOperationInput,
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

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" }
  });
}

function emptySnapshot(): NotificationStoredSnapshot {
  return { active: [], thumbnailSummaries: [], latestImportantResults: [] };
}

class FakeStore implements NotificationServiceStore {
  readonly operations = new Map<string, DesktopNotificationOperation>();
  readonly cursors = new Map<string, number>();
  readonly summaries = new Map<string, ThumbnailCacheSummary>();
  readonly imported = new Set<string>();
  readonly calls: string[] = [];
  failApply = false;
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

  async compactSnapshot(): Promise<NotificationStoredSnapshot> {
    if (this.failSnapshot) throw new Error("snapshot offline");
    const all = [...this.operations.values()];
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
        .slice(0, 3)
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

  async getOperation(libraryId: string, operationId: string): Promise<DesktopNotificationOperation | null> {
    return this.operations.get(`${libraryId}/${operationId}`) ?? null;
  }

  async publishThumbnailSummary(summary: ThumbnailCacheSummary): Promise<void> {
    this.summaries.set(summary.libraryId, summary);
  }

  async recordRendererError(
    library: NotificationLibraryIdentity,
    report: RendererErrorReport,
    occurredAt: number,
    occurrenceIncrement = 1
  ): Promise<DesktopNotificationOperation> {
    const operationId = `renderer-error:${library.id}:${report.source}:${report.code}:${report.target}`;
    const key = `${library.id}/${operationId}`;
    const existing = this.operations.get(key);
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
      occurrenceCount: (existing?.occurrenceCount ?? 0) + occurrenceIncrement,
      startedAt: existing?.startedAt ?? occurredAt,
      updatedAt: occurredAt,
      completedAt: occurredAt,
      cancellable: false,
      retryable: false
    });
    const desktop = { ...value, libraryId: library.id, libraryName: library.name };
    this.operations.set(key, desktop);
    return desktop;
  }

  async hasLegacyImport(libraryId: string): Promise<boolean> {
    return this.imported.has(libraryId);
  }

  async markLegacyImported(libraryId: string): Promise<void> {
    this.imported.add(libraryId);
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
  it("acks only after the durable batch commits", async () => {
    const calls: string[] = [];
    const store = new FakeStore();
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
          return response({ acknowledgedThrough: 1 });
        }
        return response({ items: [event(1)], maxSequence: 1 });
      },
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    assert.deepEqual(calls, ["store", "ack"]);
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
          return ackCalls === 1 ? response({ error: "offline" }, 503) : response({ acknowledgedThrough: 1 });
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

  it("does not acknowledge when the durable store rejects a batch", async () => {
    const store = new FakeStore();
    store.failApply = true;
    let ackCalls = 0;
    const service = await createNotificationService({
      store,
      fetch: async (input) => {
        if (String(input).endsWith("/notifications/outbox/ack")) {
          ackCalls += 1;
          return response({ acknowledgedThrough: 1 });
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
        return response({ acknowledgedThrough: 0 });
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

  it("imports eligible legacy rows once in bounded pages with deterministic IDs", async () => {
    const store = new FakeStore();
    const requested: string[] = [];
    const recent = NOW - 1_000;
    const old = NOW - 31 * 24 * 60 * 60 * 1_000;
    const service = await createNotificationService({
      store,
      fetch: async (input) => {
        const url = new URL(String(input));
        requested.push(url.searchParams.get("cursor") ?? "first");
        if (!url.searchParams.has("cursor")) {
          return response({
            items: [
              { id: 9, operation: "move", sourcePath: "a", targetPath: "b", status: "succeeded", createdAt: recent, finishedAt: recent, error: null },
              { id: 8, operation: "delete_recycle", sourcePath: "c", targetPath: null, status: "failed", createdAt: recent - 1, finishedAt: recent, error: "busy" }
            ],
            nextCursor: "8"
          });
        }
        return response({
          items: [
            { id: 7, operation: "rename", sourcePath: "old", targetPath: "older", status: "succeeded", createdAt: old, finishedAt: old, error: null }
          ],
          nextCursor: "7"
        });
      },
      now: () => NOW
    });

    await service.runLegacyImportForTest(LIBRARY_A, SESSION_A);
    await service.runLegacyImportForTest(LIBRARY_A, SESSION_A);
    assert.deepEqual(requested, ["first", "8"]);
    assert.equal(store.imported.has(LIBRARY_A.id), true);
    assert.deepEqual(
      [...store.operations.values()].map((item) => item.operationId).sort(),
      ["legacy-file-op:8", "legacy-file-op:9"]
    );
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
          return response({ acknowledgedThrough: 1 });
        }
        return response({ items: [event(1)], maxSequence: 1 });
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
      await recoveredStore.recordRendererError(LIBRARY_A, report, NOW - 100 + index);
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
          return response({ acknowledgedThrough: 1 });
        }
        return response({ items: [event(1)], maxSequence: 1 });
      },
      now: () => NOW
    });

    await service.runOneDrainForTest(LIBRARY_A, SESSION_A);
    const snapshot = await service.compactSnapshot();
    assert.equal(acknowledgements, 1, "a confirmed durable commit remains safe to acknowledge");
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
          return response({ acknowledgedThrough: 1 });
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
