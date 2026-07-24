import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type {
  DesktopNotificationCompactSnapshot,
  DesktopNotificationHistoryPage,
  DesktopNotificationOperation,
  DesktopNotificationsBridge,
  DesktopProcessLogBatch,
  DesktopThumbnailCacheSummary
} from "@megle/notification-contract";
import {
  buildCompactNotificationModel,
  mergeNotificationHistoryPages,
  notificationIndicator,
  processLogGapSequence,
  progressView
} from "../src/features/notifications/notificationModel.ts";
import { createNotificationStore } from "../src/features/notifications/useNotifications.ts";
import {
  createLibraryErrorReportAttempt,
  emitRendererStartupLog,
  publishThumbnailCacheStats,
  reportLibraryError,
  reportLibraryErrorAndRethrow,
  rendererErrorKey,
  ReportedLibraryFailure,
  retryLibraryErrorReport,
  thumbnailSummary
} from "../src/core/notificationReporting.ts";

function operation(
  operationId: string,
  overrides: Partial<DesktopNotificationOperation> = {}
): DesktopNotificationOperation {
  return {
    operationId,
    revision: 1,
    kind: "root_scan",
    category: "task",
    origin: "user",
    presentation: "important",
    status: "running",
    phase: "扫描中",
    title: operationId,
    summary: null,
    taskId: 1,
    rootId: 1,
    folderId: null,
    fileId: null,
    currentValue: 30,
    totalValue: 100,
    unit: "items",
    errorCode: null,
    errorDetail: null,
    cancellable: true,
    retryable: false,
    attempt: 1,
    occurrenceCount: 1,
    startedAt: 100,
    updatedAt: 200,
    completedAt: null,
    libraryId: "library-a",
    libraryName: "Artists",
    ...overrides
  };
}

function terminal(
  operationId: string,
  completedAt: number,
  overrides: Partial<DesktopNotificationOperation> = {}
): DesktopNotificationOperation {
  return operation(operationId, {
    status: "succeeded",
    phase: "完成",
    currentValue: 100,
    completedAt,
    updatedAt: completedAt,
    cancellable: false,
    ...overrides
  });
}

function thumbnail(
  libraryId: string,
  overrides: Partial<DesktopThumbnailCacheSummary> = {}
): DesktopThumbnailCacheSummary {
  return {
    libraryId,
    libraryName: libraryId === "library-a" ? "Artists" : "References",
    observedAt: 500,
    cachedCount: 80,
    totalCandidateCount: 100,
    missingCount: 10,
    staleCount: 4,
    failedCount: 1,
    pendingCount: 5,
    activeBulkTaskCount: 1,
    totalCacheBytes: 1024,
    completionPercent: 80,
    ...overrides
  };
}

function compact(
  overrides: Partial<DesktopNotificationCompactSnapshot> = {}
): DesktopNotificationCompactSnapshot {
  return {
    active: [],
    thumbnailSummaries: [],
    latestImportantResults: [],
    hasRetryableFailure: false,
    syncHealth: {
      state: "healthy",
      libraryId: "library-a",
      detail: null,
      updatedAt: 1
    },
    ...overrides
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

function bridge(
  overrides: Partial<DesktopNotificationsBridge> = {}
): DesktopNotificationsBridge {
  return {
    getCompactSnapshot: async () => compact(),
    getHistoryFacets: async () => ({ libraries: [], kinds: [] }),
    listHistory: async () => ({ items: [], total: 0, nextOffset: null }),
    cancel: async () => ({ ok: true, message: null }),
    retry: async () => ({ ok: true, message: null }),
    reportError: async () => true,
    publishThumbnailSummary: async () => true,
    onChanged: () => () => undefined,
    getLogs: async () => ({ lines: [], hasMore: false, nextBeforeSequence: null }),
    exportLogs: async () => ({
      saved: true,
      canceled: false,
      filePath: "logs.txt",
      lineCount: 0,
      error: null
    }),
    snapshotAndSubscribeLogs: () => ({
      ready: Promise.resolve({ snapshot: [], afterSequence: 0 }),
      unsubscribe: () => undefined
    }),
    clearLogs: async () => true,
    emitRendererLogs: () => undefined,
    ...overrides
  };
}

describe("notification reporting", () => {
  it("maps shared thumbnail stats with the approved completion math", () => {
    assert.deepEqual(
      thumbnailSummary(
        {
          cachedCount: 80,
          missingCount: 12,
          staleCount: 8,
          failedCount: 2,
          pendingCandidateCount: 20,
          totalBlobBytes: 1024,
          activeBulkTaskCount: 1
        },
        "lib-a",
        "Artists",
        1000
      ),
      {
        libraryId: "lib-a",
        libraryName: "Artists",
        cachedCount: 80,
        remainingCount: 20,
        failedCount: 2,
        activeBulkTaskCount: 1,
        percent: 80,
        observedAt: 1000
      }
    );
  });

  it("builds a stable renderer-error deduplication key", () => {
    assert.equal(
      rendererErrorKey({ source: "media-load", code: "timeout", target: "folder:4" }),
      '["media-load","timeout","folder:4"]'
    );
    assert.equal(
      rendererErrorKey({ source: " Media Load ", code: " HTTP 504 ", target: " Folder:4 " }),
      '["media-load","http-504","folder:4"]'
    );
    assert.notEqual(
      rendererErrorKey({ source: "a:b", code: "c", target: "d" }),
      rendererErrorKey({ source: "a", code: "b:c", target: "d" })
    );
  });

  it("bounds renderer reports and never throws bridge failures into browsing code", async () => {
    const previousWindow = (globalThis as { window?: unknown }).window;
    try {
      (globalThis as { window?: unknown }).window = {};
      const attempt = createLibraryErrorReportAttempt(
        { libraryId: "lib-a", libraryName: "Artists" },
        "attempt-bounds"
      );
      assert.equal(
        await reportLibraryError(attempt, {
          source: "media-load",
          code: "timeout",
          target: "folder:4",
          message: "request timed out"
        }),
        false
      );

      (globalThis as { window?: unknown }).window = {
        megleDesktop: {
          notifications: bridge({
            reportError: async () => {
              throw new Error("notification store unavailable");
            }
          })
        }
      };

      assert.equal(
        await reportLibraryError(attempt, {
          source: " Media Load ",
          code: " HTTP 504 ",
          target: " Folder:4 ",
          message: "request timed out"
        }),
        false
      );

      assert.equal(
        await reportLibraryError(
          createLibraryErrorReportAttempt(
            { libraryId: "lib-a", libraryName: "Artists" },
            "attempt-overlong"
          ),
          {
            source: "media-load",
            code: "timeout",
            target: "folder:4",
            message: "x".repeat(4001)
          }
        ),
        false
      );
    } finally {
      if (previousWindow === undefined) {
        delete (globalThis as { window?: unknown }).window;
      } else {
        (globalThis as { window?: unknown }).window = previousWindow;
      }
    }
  });

  it("retries a failed bridge call with the same logical report id", async () => {
    const previousWindow = (globalThis as { window?: unknown }).window;
    const reports: unknown[] = [];
    let calls = 0;
    try {
      (globalThis as { window?: unknown }).window = {
        megleDesktop: {
          notifications: bridge({
            reportError: async (input) => {
              calls += 1;
              reports.push(input);
              if (calls === 1) throw new Error("IPC response lost");
              return true;
            }
          })
        }
      };

      const attempt = createLibraryErrorReportAttempt(
        { libraryId: "lib-a", libraryName: "Artists" },
        "attempt-retry"
      );
      const input = {
        source: "media-load",
        code: "reload-failed",
        target: "folder:4",
        message: "media reload failed"
      };

      assert.equal(await reportLibraryError(attempt, input), false);
      assert.equal(await reportLibraryError(attempt, input), true);
      assert.equal(await reportLibraryError(attempt, input), true);
      assert.equal(calls, 2);
      assert.deepEqual(reports, [
        {
          reportId: "attempt-retry",
          libraryId: "lib-a",
          libraryName: "Artists",
          source: "media-load",
          code: "reload-failed",
          target: "folder:4",
          title: "Media load failed",
          message: "media reload failed",
          detail: null
        },
        {
          reportId: "attempt-retry",
          libraryId: "lib-a",
          libraryName: "Artists",
          source: "media-load",
          code: "reload-failed",
          target: "folder:4",
          title: "Media load failed",
          message: "media reload failed",
          detail: null
        }
      ]);
    } finally {
      if (previousWindow === undefined) {
        delete (globalThis as { window?: unknown }).window;
      } else {
        (globalThis as { window?: unknown }).window = previousWindow;
      }
    }
  });

  it("reports one logical attempt once and preserves primitive failures for backoff", async () => {
    const previousWindow = (globalThis as { window?: unknown }).window;
    const reports: unknown[] = [];
    try {
      (globalThis as { window?: unknown }).window = {
        megleDesktop: {
          notifications: bridge({
            reportError: async (input) => {
              reports.push(input);
              return true;
            }
          })
        }
      };

      const attempt = createLibraryErrorReportAttempt(
        { libraryId: "lib-a", libraryName: "Artists" },
        "attempt-scan"
      );
      const input = {
        source: "scan-refresh",
        code: "refresh-failed",
        target: "root:2",
        message: "primitive rejection"
      };
      let propagated: unknown;
      try {
        await reportLibraryErrorAndRethrow(attempt, input, "primitive rejection");
      } catch (cause) {
        propagated = cause;
      }

      assert.ok(propagated instanceof ReportedLibraryFailure);
      assert.equal(propagated.originalCause, "primitive rejection");
      assert.equal(await retryLibraryErrorReport(propagated), true);
      assert.equal(reports.length, 1);
      assert.deepEqual(reports[0], {
        reportId: "attempt-scan",
        libraryId: "lib-a",
        libraryName: "Artists",
        source: "scan-refresh",
        code: "refresh-failed",
        target: "root:2",
        title: "Scan refresh failed",
        message: "primitive rejection",
        detail: null
      });
    } finally {
      if (previousWindow === undefined) {
        delete (globalThis as { window?: unknown }).window;
      } else {
        (globalThis as { window?: unknown }).window = previousWindow;
      }
    }
  });

  it("keeps nested failure reporting at exactly one business occurrence", async () => {
    const previousWindow = (globalThis as { window?: unknown }).window;
    const reports: Array<{ source: string; reportId?: string }> = [];
    try {
      (globalThis as { window?: unknown }).window = {
        megleDesktop: {
          notifications: bridge({
            reportError: async (input) => {
              reports.push(input);
              return true;
            }
          })
        }
      };

      const attempt = createLibraryErrorReportAttempt(
        { libraryId: "lib-a", libraryName: "Artists" },
        "attempt-nested"
      );
      let propagated: unknown;
      try {
        await reportLibraryErrorAndRethrow(
          attempt,
          {
            source: "media-load",
            code: "window-load-failed",
            target: "folder:4",
            message: "Core unavailable"
          },
          new Error("Core unavailable")
        );
      } catch (cause) {
        propagated = cause;
      }

      let outerPropagated: unknown;
      try {
        await reportLibraryErrorAndRethrow(
          createLibraryErrorReportAttempt(
            { libraryId: "lib-a", libraryName: "Artists" },
            "attempt-nested-outer"
          ),
          {
            source: "scan-refresh",
            code: "refresh-failed",
            target: "root:2",
            message: "Core unavailable"
          },
          propagated
        );
      } catch (cause) {
        outerPropagated = cause;
      }

      assert.equal(outerPropagated, propagated);
      assert.deepEqual(reports, [
        {
          reportId: "attempt-nested",
          source: "media-load",
          code: "window-load-failed",
          target: "folder:4",
          libraryId: "lib-a",
          libraryName: "Artists",
          title: "Media load failed",
          message: "Core unavailable",
          detail: null
        }
      ]);
    } finally {
      if (previousWindow === undefined) {
        delete (globalThis as { window?: unknown }).window;
      } else {
        (globalThis as { window?: unknown }).window = previousWindow;
      }
    }
  });

  it("treats the same Error reused by independent operations as distinct occurrences", async () => {
    const previousWindow = (globalThis as { window?: unknown }).window;
    const reports: unknown[] = [];
    try {
      (globalThis as { window?: unknown }).window = {
        megleDesktop: {
          notifications: bridge({
            reportError: async (input) => {
              reports.push(input);
              return true;
            }
          })
        }
      };
      const sharedCause = new Error("shared object");
      const input = {
        source: "media-load",
        code: "reload-failed",
        target: "folder:4",
        message: sharedCause.message
      };

      const results = await Promise.allSettled([
        reportLibraryErrorAndRethrow(
          createLibraryErrorReportAttempt(
            { libraryId: "lib-a", libraryName: "Artists" },
            "attempt-independent-a"
          ),
          input,
          sharedCause
        ),
        reportLibraryErrorAndRethrow(
          createLibraryErrorReportAttempt(
            { libraryId: "lib-a", libraryName: "Artists" },
            "attempt-independent-b"
          ),
          input,
          sharedCause
        )
      ]);

      assert.deepEqual(results.map((result) => result.status), ["rejected", "rejected"]);
      assert.deepEqual(
        reports.map((report) => (report as { reportId: string }).reportId),
        ["attempt-independent-a", "attempt-independent-b"]
      );
    } finally {
      if (previousWindow === undefined) {
        delete (globalThis as { window?: unknown }).window;
      } else {
        (globalThis as { window?: unknown }).window = previousWindow;
      }
    }
  });

  it("keeps async errors and thumbnail summaries owned by their captured library", async () => {
    const previousWindow = (globalThis as { window?: unknown }).window;
    const reports: unknown[] = [];
    const summaries: unknown[] = [];
    let libraryReads = 0;
    try {
      (globalThis as { window?: unknown }).window = {
        megleDesktop: {
          listLibraries: async () => [
            {
              id: "lib-b",
              name: "References",
              dbPath: "references.sqlite",
              available: true,
              active: true
            }
          ],
          notifications: bridge({
            reportError: async (input) => {
              reports.push(input);
              return true;
            },
            publishThumbnailSummary: async (input) => {
              summaries.push(input);
              return true;
            }
          })
        }
      };
      const desktop = (globalThis as { window: { megleDesktop: { listLibraries: () => Promise<unknown> } } }).window
        .megleDesktop;
      const listLibraries = desktop.listLibraries;
      desktop.listLibraries = async () => {
        libraryReads += 1;
        return listLibraries();
      };

      const library = { libraryId: "lib-a", libraryName: "Artists" };
      await reportLibraryError(
        createLibraryErrorReportAttempt(library, "attempt-library-a"),
        {
          source: "media-load",
          code: "reload-failed",
          target: "folder:4",
          message: "A failed after the switch"
        }
      );

      await publishThumbnailCacheStats(
        {
          cachedCount: 80,
          missingCount: 12,
          staleCount: 8,
          failedCount: 2,
          pendingCandidateCount: 20,
          totalBlobBytes: 1024,
          activeBulkTaskCount: 1,
          failureDetails: [{
            fileId: 42,
            folderId: 7,
            rootId: 3,
            relativePath: "Artists\\Portraits\\a.jpg",
            failureKind: "decode",
            error: "decode failed",
            updatedAt: 900
          }],
          failureDetailsTruncated: true
        },
        library,
        1000
      );

      assert.equal(libraryReads, 0);
      assert.equal((reports[0] as { libraryId: string }).libraryId, "lib-a");
      assert.deepEqual(summaries, [
        {
          libraryId: "lib-a",
          libraryName: "Artists",
          observedAt: 1000,
          cachedCount: 80,
          totalCandidateCount: 100,
          missingCount: 12,
          staleCount: 8,
          failedCount: 2,
          pendingCount: 20,
          activeBulkTaskCount: 1,
          totalCacheBytes: 1024,
          completionPercent: 80,
          failureDetails: [{
            libraryId: "lib-a",
            fileId: 42,
            folderId: 7,
            rootId: 3,
            relativePath: "Artists\\Portraits\\a.jpg",
            failureKind: "decode",
            error: "decode failed",
            updatedAt: 900
          }]
        }
      ]);
    } finally {
      if (previousWindow === undefined) {
        delete (globalThis as { window?: unknown }).window;
      } else {
        (globalThis as { window?: unknown }).window = previousWindow;
      }
    }
  });

  it("publishes authoritative thumbnail summaries from the Settings renderer", async () => {
    const previousWindow = (globalThis as { window?: unknown }).window;
    let publishCalls = 0;
    try {
      (globalThis as { window?: unknown }).window = {
        megleDesktop: {
          windowRole: "settings",
          notifications: bridge({
            publishThumbnailSummary: async () => {
              publishCalls += 1;
              return true;
            }
          })
        }
      };

      const accepted = await publishThumbnailCacheStats(
        {
          cachedCount: 1,
          missingCount: 1,
          staleCount: 0,
          failedCount: 0,
          pendingCandidateCount: 1,
          totalBlobBytes: 1024,
          activeBulkTaskCount: 0,
          failureDetails: [],
          failureDetailsTruncated: false
        },
        { libraryId: "lib-a", libraryName: "Artists" },
        1000
      );

      assert.equal(accepted, true);
      assert.equal(publishCalls, 1);
    } finally {
      if (previousWindow === undefined) {
        delete (globalThis as { window?: unknown }).window;
      } else {
        (globalThis as { window?: unknown }).window = previousWindow;
      }
    }
  });

  it("rejects a positive failure count without details instead of publishing an inconsistent snapshot", async () => {
    const previousWindow = (globalThis as { window?: unknown }).window;
    let publishCalls = 0;
    try {
      (globalThis as { window?: unknown }).window = {
        megleDesktop: {
          windowRole: "main",
          notifications: bridge({
            publishThumbnailSummary: async () => {
              publishCalls += 1;
              return true;
            }
          })
        }
      };

      const accepted = await publishThumbnailCacheStats({
        cachedCount: 1,
        missingCount: 0,
        staleCount: 0,
        failedCount: 1,
        pendingCandidateCount: 0,
        totalBlobBytes: 1024,
        activeBulkTaskCount: 0,
        failureDetails: [],
        failureDetailsTruncated: false
      }, { libraryId: "lib-a", libraryName: "Artists" });

      assert.equal(accepted, false);
      assert.equal(publishCalls, 0);
    } finally {
      if (previousWindow === undefined) {
        delete (globalThis as { window?: unknown }).window;
      } else {
        (globalThis as { window?: unknown }).window = previousWindow;
      }
    }
  });

  it("emits the renderer startup log once through the bounded structured bridge", () => {
    const previousWindow = (globalThis as { window?: unknown }).window;
    const batches: unknown[] = [];
    try {
      (globalThis as { window?: unknown }).window = {
        megleDesktop: {
          windowRole: "main",
          notifications: bridge({
            emitRendererLogs: (entries) => batches.push(entries)
          })
        }
      };

      assert.equal(emitRendererStartupLog(), true);
      assert.equal(emitRendererStartupLog(), false);
      assert.deepEqual(batches, [[{
        level: "info",
        message: "Megle renderer ready",
        metadata: { eventKind: "startup_ready", windowRole: "main" }
      }]]);
    } finally {
      if (previousWindow === undefined) {
        delete (globalThis as { window?: unknown }).window;
      } else {
        (globalThis as { window?: unknown }).window = previousWindow;
      }
    }
  });
});

describe("notification selectors", () => {
  it("reports only Desktop-declared log gaps, not filtered sequence jumps", () => {
    assert.equal(
      processLogGapSequence({
        lines: [
          { sequence: 41, timestamp: 1, source: "core", level: "warn", message: "filtered" }
        ],
        afterSequence: 41,
        droppedBeforeSequence: null
      }),
      null
    );
    assert.equal(
      processLogGapSequence({
        lines: [],
        afterSequence: 80,
        droppedBeforeSequence: 63
      }),
      63
    );
  });

  it("keeps all active logical operations and limits important terminal results to three", () => {
    const model = buildCompactNotificationModel(
      compact({
        active: [
          operation("pending", { status: "pending", origin: "background" }),
          operation("running", { presentation: "status_only" }),
          terminal("not-active", 600)
        ],
        latestImportantResults: [
          terminal("result:2", 200),
          terminal("history-only", 700, { presentation: "history_only" }),
          terminal("result:4", 400),
          terminal("result:1", 100),
          terminal("result:3", 300),
          operation("still-running", { presentation: "important" })
        ]
      })
    );

    assert.deepEqual(model.active.map((item) => item.operationId), ["pending", "running"]);
    assert.equal(model.activeLibraryId, "library-a");
    assert.deepEqual(model.results.map((item) => item.operationId), [
      "result:4",
      "result:3",
      "result:2"
    ]);
    assert.equal(model.active[0]?.libraryLabel, "Artists");
  });

  it("deduplicates operation revisions before sorting", () => {
    const model = buildCompactNotificationModel(
      compact({
        active: [
          operation("scan", { revision: 1, currentValue: 10 }),
          operation("scan", { revision: 2, currentValue: 40 })
        ],
        latestImportantResults: [
          terminal("done", 100, { revision: 1, summary: "old" }),
          terminal("done", 100, { revision: 2, summary: "new" })
        ]
      })
    );

    assert.equal(model.active.length, 1);
    assert.equal(model.active[0]?.revision, 2);
    assert.equal(model.results.length, 1);
    assert.equal(model.results[0]?.summary, "new");
  });

  it("uses retryable failure over active for the subtle status dot", () => {
    assert.equal(notificationIndicator({ active: true, retryableFailure: true }), "failure");
    assert.equal(notificationIndicator({ active: true, retryableFailure: false }), "active");
    assert.equal(notificationIndicator({ active: false, retryableFailure: false }), "idle");

    const model = buildCompactNotificationModel(
      compact({
        active: [operation("scan")],
        hasRetryableFailure: true,
        latestImportantResults: [
          terminal("newer-success", 300),
          terminal("newest-success", 400)
        ]
      })
    );
    assert.equal(model.indicator, "failure");

    const resolved = buildCompactNotificationModel(
      compact({
        hasRetryableFailure: false,
        latestImportantResults: [
          terminal("old-failure", 300, {
            status: "failed",
            retryable: true,
            origin: "background"
          })
        ]
      })
    );
    assert.equal(resolved.indicator, "idle");
  });

  it("builds determinate and indeterminate progress without false percentages", () => {
    assert.deepEqual(progressView(operation("known")), {
      determinate: true,
      percent: 30,
      label: "30 / 100"
    });
    assert.deepEqual(
      progressView(operation("unknown", { totalValue: null, phase: "等待媒体" })),
      { determinate: false, percent: null, label: "等待媒体" }
    );
    assert.equal(progressView(operation("overflow", { currentValue: 140 })).percent, 100);
  });

  it("keeps one thumbnail aggregate per library in stable library id order", () => {
    const model = buildCompactNotificationModel(
      compact({
        thumbnailSummaries: [
          thumbnail("library-b", {
            libraryName: "A References",
            cachedCount: 20,
            totalCandidateCount: 50,
            pendingCount: 7,
            failedCount: 3,
            staleCount: 2,
            completionPercent: 40
          }),
          thumbnail("library-a", {
            libraryName: "Z Artists",
            cachedCount: 80,
            totalCandidateCount: 100,
            pendingCount: 5,
            failedCount: 1,
            staleCount: 4,
            completionPercent: 80
          }),
          thumbnail("library-a", { observedAt: 400, cachedCount: 1 })
        ]
      })
    );

    assert.deepEqual(model.thumbnail.summaries.map((item) => item.libraryId), [
      "library-a",
      "library-b"
    ]);
    assert.equal(model.thumbnail.summaries.length, model.thumbnail.libraryCount);
    assert.ok(model.thumbnail.summaries.length <= model.thumbnail.libraryCount);
    assert.deepEqual(
      model.thumbnail.summaries.map((item) => ({
        libraryId: item.libraryId,
        libraryName: item.libraryName,
        cachedCount: item.cachedCount,
        totalCandidateCount: item.totalCandidateCount,
        pendingCount: item.pendingCount,
        failedCount: item.failedCount,
        staleCount: item.staleCount,
        completionPercent: item.completionPercent
      })),
      [
        {
          libraryId: "library-a",
          libraryName: "Z Artists",
          cachedCount: 80,
          totalCandidateCount: 100,
          pendingCount: 5,
          failedCount: 1,
          staleCount: 4,
          completionPercent: 80
        },
        {
          libraryId: "library-b",
          libraryName: "A References",
          cachedCount: 20,
          totalCandidateCount: 50,
          pendingCount: 7,
          failedCount: 3,
          staleCount: 2,
          completionPercent: 40
        }
      ]
    );
    assert.equal(model.thumbnail.cachedCount, 100);
    assert.equal(model.thumbnail.totalCandidateCount, 150);
    assert.equal(model.thumbnail.completionPercent, 67);
    assert.equal(model.thumbnail.active, true);
  });

  it("keeps the empty compact model free of thumbnail aggregate rows", () => {
    const model = buildCompactNotificationModel(compact({ thumbnailSummaries: [] }));

    assert.deepEqual(model.thumbnail.summaries, []);
    assert.equal(model.thumbnail.libraryCount, 0);
  });

  it("exposes degraded health and never introduces read state", () => {
    const model = buildCompactNotificationModel(
      compact({
        syncHealth: {
          state: "degraded",
          libraryId: null,
          detail: "store unavailable",
          updatedAt: 2
        }
      })
    );

    assert.equal(model.degraded, true);
    assert.equal(model.healthDetail, "store unavailable");
    assert.equal("read" in model, false);
    assert.equal("unread" in model, false);
  });
});

describe("notification history model", () => {
  it("merges overlapping pages by revision with stable terminal ordering", () => {
    const first: DesktopNotificationHistoryPage = {
      items: [
        terminal("b", 300, { libraryId: "library-b", libraryName: "References" }),
        terminal("same", 200, { revision: 1, summary: "old" })
      ],
      total: 3,
      nextOffset: 2
    };
    const second: DesktopNotificationHistoryPage = {
      items: [
        terminal("same", 200, { revision: 2, summary: "new" }),
        terminal("a", 300, { libraryId: "library-a", libraryName: "Artists" })
      ],
      total: 3,
      nextOffset: null
    };

    const merged = mergeNotificationHistoryPages([
      { offset: 0, page: first },
      { offset: 2, page: second }
    ]);

    assert.deepEqual(merged.items.map((item) => item.operationId), ["a", "b", "same"]);
    assert.equal(merged.items.find((item) => item.operationId === "same")?.revision, 2);
    assert.equal(merged.total, 3);
    assert.equal(merged.nextOffset, null);
  });

  it("keeps the next offset at the first missing contiguous page", () => {
    const page = { items: [terminal("late", 100)], total: 3, nextOffset: null };
    assert.equal(mergeNotificationHistoryPages([{ offset: 2, page }]).nextOffset, 0);
  });

  it("keeps the first same-revision payload and reports conflicting history", () => {
    const first = terminal("conflict", 100, {
      revision: 3,
      summary: "first payload",
      currentValue: 10
    });
    const conflict = {
      ...first,
      summary: "conflicting payload",
      currentValue: 90,
      updatedAt: first.updatedAt + 1_000
    };
    const merged = mergeNotificationHistoryPages([
      {
        offset: 0,
        page: { items: [first], total: 1, nextOffset: 1 }
      },
      {
        offset: 1,
        page: { items: [conflict], total: 1, nextOffset: null }
      }
    ]);

    assert.equal(merged.items[0]?.summary, "first payload");
    assert.equal(merged.items[0]?.currentValue, 10);
    assert.match(merged.conflictError ?? "", /conflict.*revision 3/i);

    const replay = mergeNotificationHistoryPages([
      {
        offset: 0,
        page: { items: [first], total: 1, nextOffset: 1 }
      },
      {
        offset: 1,
        page: { items: [{ ...first }], total: 1, nextOffset: null }
      }
    ]);
    assert.equal(replay.conflictError, null);
  });
});

describe("notification external store", () => {
  it("invokes the default microtask scheduler with its browser receiver", () => {
    const originalQueueMicrotask = globalThis.queueMicrotask;
    globalThis.queueMicrotask = function queueMicrotaskWithReceiver(
      this: typeof globalThis,
      callback: VoidFunction
    ) {
      if (this !== globalThis) throw new TypeError("Illegal invocation");
      callback();
    };
    try {
      const store = createNotificationStore({ getBridge: () => bridge() });
      const off = store.subscribe(() => undefined);
      assert.doesNotThrow(off);
    } finally {
      globalThis.queueMicrotask = originalQueueMicrotask;
    }
  });

  it("shares one bridge subscription across consumers and survives StrictMode resubscribe", async () => {
    let subscribeCalls = 0;
    let unsubscribeCalls = 0;
    const disconnects: Array<() => void> = [];
    const testBridge = bridge({
      onChanged: () => {
        subscribeCalls += 1;
        return () => {
          unsubscribeCalls += 1;
        };
      }
    });
    const store = createNotificationStore({
      getBridge: () => testBridge,
      deferDisconnect: (callback) => disconnects.push(callback)
    });

    const offFirst = store.subscribe(() => undefined);
    const offSecond = store.subscribe(() => undefined);
    offFirst();
    offSecond();
    const offStrictModeReplay = store.subscribe(() => undefined);

    assert.equal(subscribeCalls, 1);
    disconnects.splice(0).forEach((callback) => callback());
    assert.equal(unsubscribeCalls, 0);

    offStrictModeReplay();
    disconnects.splice(0).forEach((callback) => callback());
    assert.equal(unsubscribeCalls, 1);
  });

  it("loads the initial snapshot and applies changed snapshots", async () => {
    let changed: ((snapshot: DesktopNotificationCompactSnapshot) => void) | null = null;
    const testBridge = bridge({
      getCompactSnapshot: async () => compact({ active: [operation("initial")] }),
      onChanged: (callback) => {
        changed = callback;
        return () => undefined;
      }
    });
    const store = createNotificationStore({ getBridge: () => testBridge });
    const off = store.subscribe(() => undefined);
    await store.whenIdle();
    assert.deepEqual(store.getSnapshot().compact.active.map((item) => item.operationId), ["initial"]);

    changed?.(compact({ active: [operation("changed")] }));
    assert.deepEqual(store.getSnapshot().compact.active.map((item) => item.operationId), ["changed"]);
    off();
  });

  it("ignores a stale initial snapshot that resolves after a change event", async () => {
    const initial = deferred<DesktopNotificationCompactSnapshot>();
    let changed: ((snapshot: DesktopNotificationCompactSnapshot) => void) | null = null;
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          getCompactSnapshot: () => initial.promise,
          onChanged: (callback) => {
            changed = callback;
            return () => undefined;
          }
        })
    });

    const off = store.subscribe(() => undefined);
    changed?.(compact({ active: [operation("newer")] }));
    initial.resolve(compact({ active: [operation("stale")] }));
    await store.whenIdle();

    assert.deepEqual(store.getSnapshot().compact.active.map((item) => item.operationId), ["newer"]);
    off();
  });

  it("ignores an initial snapshot from a disconnected bridge generation", async () => {
    const stale = deferred<DesktopNotificationCompactSnapshot>();
    const disconnects: Array<() => void> = [];
    let generation = 0;
    const store = createNotificationStore({
      getBridge: () => {
        generation += 1;
        return bridge({
          getCompactSnapshot: () =>
            generation === 1
              ? stale.promise
              : Promise.resolve(compact({ active: [operation("current")] }))
        });
      },
      deferDisconnect: (callback) => disconnects.push(callback)
    });

    const offStale = store.subscribe(() => undefined);
    offStale();
    disconnects.splice(0).forEach((callback) => callback());
    const offCurrent = store.subscribe(() => undefined);
    await store.whenIdle();
    stale.resolve(compact({ active: [operation("stale-generation")] }));
    await Promise.resolve();

    assert.deepEqual(store.getSnapshot().compact.active.map((item) => item.operationId), [
      "current"
    ]);
    offCurrent();
  });

  it("keeps a changed snapshot when the concurrent initial request fails", async () => {
    const initial = deferred<DesktopNotificationCompactSnapshot>();
    let changed: ((snapshot: DesktopNotificationCompactSnapshot) => void) | null = null;
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          getCompactSnapshot: () => initial.promise,
          onChanged: (callback) => {
            changed = callback;
            return () => undefined;
          }
        })
    });

    const off = store.subscribe(() => undefined);
    changed?.(compact({ active: [operation("newer")] }));
    initial.reject(new Error("initial unavailable"));
    await store.whenIdle();

    assert.deepEqual(store.getSnapshot().compact.active.map((item) => item.operationId), ["newer"]);
    off();
  });

  it("does not let a stale explicit compact refresh overwrite a newer push", async () => {
    const refresh = deferred<DesktopNotificationCompactSnapshot>();
    let snapshotCalls = 0;
    let changed: ((snapshot: DesktopNotificationCompactSnapshot) => void) | null = null;
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          getCompactSnapshot: () => {
            snapshotCalls += 1;
            return snapshotCalls === 1
              ? Promise.resolve(compact({ active: [operation("initial")] }))
              : refresh.promise;
          },
          onChanged: (callback) => {
            changed = callback;
            return () => undefined;
          }
        })
    });
    const off = store.subscribe(() => undefined);
    await store.whenIdle();

    const refreshing = store.refreshCompact();
    changed?.(compact({ active: [operation("pushed", { revision: 3 })] }));
    refresh.resolve(compact({ active: [operation("stale-refresh", { revision: 2 })] }));
    await refreshing;

    assert.deepEqual(store.getSnapshot().compact.active.map((item) => item.operationId), [
      "pushed"
    ]);
    off();
  });

  it("provides a stable degraded model when the bridge is unavailable or fails", async () => {
    const unavailable = createNotificationStore({ getBridge: () => null });
    const before = unavailable.getSnapshot();
    const offUnavailable = unavailable.subscribe(() => undefined);
    await unavailable.whenIdle();
    assert.equal(unavailable.getSnapshot(), before);
    assert.equal(before.compact.degraded, true);
    offUnavailable();

    const failing = createNotificationStore({
      getBridge: () =>
        bridge({
          getCompactSnapshot: async () => {
            throw new Error("IPC closed");
          },
          onChanged: () => {
            throw new Error("subscription failed");
          }
        })
    });
    const offFailing = failing.subscribe(() => undefined);
    await failing.whenIdle();
    assert.equal(failing.getSnapshot().compact.degraded, true);
    assert.match(failing.getSnapshot().compact.healthDetail ?? "", /IPC closed|subscription failed/);
    offFailing();
  });

  it("deduplicates concurrent history pages and ignores stale query responses", async () => {
    const first = deferred<DesktopNotificationHistoryPage>();
    const second = deferred<DesktopNotificationHistoryPage>();
    const calls: Array<{ search?: string; offset?: number }> = [];
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          listHistory: (query) => {
            calls.push({ search: query.search, offset: query.offset });
            return query.search === "first" ? first.promise : second.promise;
          }
        })
    });

    const staleRequest = store.loadHistory({ search: "first", limit: 20 });
    const currentRequest = store.loadHistory({ search: "second", limit: 20 });
    second.resolve({ items: [terminal("current", 200)], total: 1, nextOffset: null });
    await currentRequest;
    first.resolve({ items: [terminal("stale", 300)], total: 1, nextOffset: null });
    await staleRequest;

    assert.deepEqual(store.getSnapshot().history.items.map((item) => item.operationId), [
      "current"
    ]);

    const page = deferred<DesktopNotificationHistoryPage>();
    let pagingCalls = 0;
    const pagingStore = createNotificationStore({
      getBridge: () =>
        bridge({
          listHistory: () => {
            pagingCalls += 1;
            return page.promise;
          }
        })
    });
    const one = pagingStore.loadHistory({ limit: 20 });
    const two = pagingStore.loadMoreHistory();
    assert.equal(pagingCalls, 1);
    page.resolve({ items: [], total: 0, nextOffset: null });
    await Promise.all([one, two]);
  });

  it("reuses one in-flight first-page promise for the same normalized query", async () => {
    const page = deferred<DesktopNotificationHistoryPage>();
    let calls = 0;
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          listHistory: () => {
            calls += 1;
            return page.promise;
          }
        })
    });
    const query = {
      search: " same ",
      statuses: ["failed" as const],
      origins: ["background" as const, "user" as const],
      kinds: ["root_scan", "delete_folder"],
      limit: 20,
      offset: 0
    };

    const first = store.loadHistory(query);
    const strictModeReplay = store.loadHistory({
      search: "same",
      statuses: ["failed"],
      origins: ["user", "background"],
      kinds: ["delete_folder", "root_scan"],
      limit: 20,
      offset: 0
    });

    assert.equal(first, strictModeReplay);
    assert.equal(calls, 1);
    page.resolve({ items: [], total: 0, nextOffset: null });
    await first;
  });

  it("refreshes a completed query after page two and clears prior pages", async () => {
    const refresh = deferred<DesktopNotificationHistoryPage>();
    let calls = 0;
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          listHistory: async (query) => {
            calls += 1;
            if (calls === 1) {
              return { items: [terminal("page-1", 300)], total: 2, nextOffset: 1 };
            }
            if (calls === 2) {
              assert.equal(query.offset, 1);
              return { items: [terminal("page-2", 200)], total: 2, nextOffset: null };
            }
            return refresh.promise;
          }
        })
    });
    const query = { search: "same", limit: 20, offset: 0 };

    await store.loadHistory(query);
    await store.loadMoreHistory();
    assert.deepEqual(
      store.getSnapshot().history.items.map((item) => item.operationId),
      ["page-1", "page-2"]
    );

    const refreshing = store.loadHistory(query);
    assert.equal(calls, 3);
    assert.deepEqual(store.getSnapshot().history.items, []);
    assert.equal(store.getSnapshot().history.total, 0);
    refresh.resolve({ items: [terminal("refreshed", 400)], total: 1, nextOffset: null });
    await refreshing;
    assert.deepEqual(
      store.getSnapshot().history.items.map((item) => item.operationId),
      ["refreshed"]
    );
  });

  it("ignores an old append that resolves after an explicit refresh", async () => {
    const append = deferred<DesktopNotificationHistoryPage>();
    const refresh = deferred<DesktopNotificationHistoryPage>();
    let calls = 0;
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          listHistory: async () => {
            calls += 1;
            if (calls === 1) {
              return { items: [terminal("initial", 300)], total: 2, nextOffset: 1 };
            }
            return calls === 2 ? append.promise : refresh.promise;
          }
        })
    });
    const query = { limit: 20, offset: 0 };

    await store.loadHistory(query);
    const oldAppend = store.loadMoreHistory();
    const refreshing = store.loadHistory(query);
    assert.equal(calls, 3);

    append.resolve({ items: [terminal("stale-append", 500)], total: 2, nextOffset: null });
    await oldAppend;
    assert.deepEqual(store.getSnapshot().history.items, []);

    refresh.resolve({ items: [terminal("fresh", 600)], total: 1, nextOffset: null });
    await refreshing;
    assert.deepEqual(
      store.getSnapshot().history.items.map((item) => item.operationId),
      ["fresh"]
    );
  });

  it("surfaces same-revision page conflicts through history error", async () => {
    const first = terminal("conflict", 100, { revision: 2, summary: "first" });
    let calls = 0;
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          listHistory: async () => {
            calls += 1;
            return calls === 1
              ? { items: [first], total: 1, nextOffset: 1 }
              : {
                  items: [{ ...first, summary: "second", updatedAt: first.updatedAt + 1 }],
                  total: 1,
                  nextOffset: null
                };
          }
        })
    });

    await store.loadHistory({ limit: 20 });
    await store.loadMoreHistory();
    assert.equal(store.getSnapshot().history.items[0]?.summary, "first");
    assert.match(store.getSnapshot().history.error ?? "", /conflict.*revision 2/i);
  });

  it("returns typed action failures without mutating a newer compact revision", async () => {
    const action = deferred<{ ok: boolean; message: string | null }>();
    let changed: ((snapshot: DesktopNotificationCompactSnapshot) => void) | null = null;
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          cancel: () => action.promise,
          onChanged: (callback) => {
            changed = callback;
            return () => undefined;
          }
        })
    });
    const off = store.subscribe(() => undefined);
    const pending = store.cancel("library-a", "scan");
    changed?.(compact({ active: [operation("scan", { revision: 2, currentValue: 60 })] }));
    action.resolve({ ok: false, message: "already completed" });

    assert.deepEqual(await pending, { ok: false, message: "already completed" });
    assert.equal(store.getSnapshot().compact.active[0]?.revision, 2);
    off();
  });

  it("coalesces duplicate actions while keeping different operations independent", async () => {
    const cancelAction = deferred<{ ok: boolean; message: string | null }>();
    const retryAction = deferred<{ ok: boolean; message: string | null }>();
    let cancelCalls = 0;
    let retryCalls = 0;
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          cancel: () => {
            cancelCalls += 1;
            return cancelAction.promise;
          },
          retry: () => {
            retryCalls += 1;
            return retryAction.promise;
          }
        })
    });

    const firstCancel = store.cancel("library-a", "operation-a");
    const duplicateCancel = store.cancel("library-a", "operation-a");
    const blockedRetry = store.retry("library-a", "operation-a");
    const independentRetry = store.retry("library-a", "operation-b");
    assert.equal(cancelCalls, 1);
    assert.equal(retryCalls, 1);
    assert.deepEqual(await blockedRetry, {
      ok: false,
      message: "该操作已有动作正在执行"
    });

    retryAction.resolve({ ok: false, message: "retry failed" });
    assert.deepEqual(await independentRetry, { ok: false, message: "retry failed" });
    cancelAction.resolve({ ok: true, message: null });
    assert.deepEqual(await Promise.all([firstCancel, duplicateCancel]), [
      { ok: true, message: null },
      { ok: true, message: null }
    ]);

    await store.cancel("library-a", "operation-a");
    assert.equal(cancelCalls, 2, "a settled action may be submitted again after capabilities refresh");
  });

  it("converts bridge action exceptions into typed failures", async () => {
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          retry: async () => {
            throw new Error("IPC unavailable");
          }
        })
    });

    assert.deepEqual(await store.retry("library-a", "scan"), {
      ok: false,
      message: "IPC unavailable"
    });
  });

  it("does not subscribe to logs until explicitly requested", () => {
    let subscriptions = 0;
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          snapshotAndSubscribeLogs: () => {
            subscriptions += 1;
            return {
              ready: Promise.resolve({ snapshot: [], afterSequence: 0 }),
              unsubscribe: () => undefined
            };
          }
        })
    });

    const off = store.subscribe(() => undefined);
    assert.equal(subscriptions, 0);
    off();
  });

  it("opens logs atomically and delivers buffered gap batches after the initial snapshot", async () => {
    const callbacks: Array<(batch: DesktopProcessLogBatch) => void> = [];
    const firstReady = deferred<{ snapshot: []; afterSequence: number }>();
    const deferredDeliveries: Array<() => void> = [];
    let unsubscribes = 0;
    const store = createNotificationStore({
      deferLogDelivery: (callback) => deferredDeliveries.push(callback),
      getBridge: () =>
        bridge({
          snapshotAndSubscribeLogs: (_query, callback) => {
            callbacks.push(callback);
            return {
              ready:
                callbacks.length === 1
                  ? firstReady.promise
                  : Promise.resolve({ snapshot: [], afterSequence: callbacks.length * 10 }),
              unsubscribe: () => {
                unsubscribes += 1;
              }
            };
          }
        })
    });
    const batches: DesktopProcessLogBatch[] = [];

    const openingFirst = store.openLogSubscription({}, (batch) => batches.push(batch));
    callbacks[0]?.({ lines: [], afterSequence: 11, droppedBeforeSequence: 4 });
    assert.deepEqual(batches, []);
    firstReady.resolve({ snapshot: [], afterSequence: 10 });
    const first = await openingFirst;
    assert.deepEqual(first.ok && first.value.snapshot, {
      snapshot: [],
      afterSequence: 10
    });
    assert.deepEqual(batches, [], "the caller must apply the initial snapshot before batches");
    deferredDeliveries.splice(0).forEach((callback) => callback());
    assert.deepEqual(batches, [
      { lines: [], afterSequence: 11, droppedBeforeSequence: 4 }
    ]);

    const openingSecond = store.openLogSubscription({ sources: ["core"] }, (batch) =>
      batches.push(batch)
    );
    assert.equal(unsubscribes, 1);
    callbacks[0]?.({ lines: [], afterSequence: 11, droppedBeforeSequence: 4 });
    callbacks[1]?.({ lines: [], afterSequence: 21, droppedBeforeSequence: 9 });
    const second = await openingSecond;
    assert.equal(second.ok, true);
    assert.equal(batches.length, 1);
    if (!second.ok) assert.fail(second.error);
    second.value.unsubscribe();
    deferredDeliveries.splice(0).forEach((callback) => callback());
    assert.deepEqual(batches, [
      { lines: [], afterSequence: 11, droppedBeforeSequence: 4 }
    ]);

    store.dispose();
    assert.equal(unsubscribes, 2);
  });

  it("wraps library, compact, log page, export and clear requests without throwing", async () => {
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          getLogs: async (query) => {
            if (query.search === "fail") throw new Error("page unavailable");
            return { lines: [], hasMore: true, nextBeforeSequence: 12 };
          },
          getHistoryFacets: async () => ({
            libraries: [{ id: "library-a", name: "Artists" }],
            kinds: ["root_scan"]
          }),
          exportLogs: async () => ({
            saved: true,
            canceled: false,
            filePath: "logs.txt",
            lineCount: 4,
            error: null
          }),
          clearLogs: async () => {
            throw new Error("clear unavailable");
          }
        })
    });

    assert.deepEqual(await store.getLogPage({}), {
      ok: true,
      value: { lines: [], hasMore: true, nextBeforeSequence: 12 }
    });
    assert.deepEqual(await store.getLogPage({ search: "fail" }), {
      ok: false,
      error: "page unavailable"
    });
    assert.deepEqual(await store.getHistoryFacets(), {
      ok: true,
      value: {
        libraries: [{ id: "library-a", name: "Artists" }],
        kinds: ["root_scan"]
      }
    });
    assert.deepEqual(await store.exportLogs({ source: "core" }), {
      ok: true,
      value: {
        saved: true,
        canceled: false,
        filePath: "logs.txt",
        lineCount: 4,
        error: null
      }
    });
    assert.deepEqual(await store.refreshCompact(), { ok: true, value: true });
    assert.deepEqual(await store.clearLogs(), {
      ok: false,
      error: "clear unavailable"
    });
  });

  it("turns log subscription startup errors into typed failures and cleans up", async () => {
    let unsubscribes = 0;
    const store = createNotificationStore({
      getBridge: () =>
        bridge({
          snapshotAndSubscribeLogs: () => ({
            ready: Promise.reject(new Error("subscription unavailable")),
            unsubscribe: () => {
              unsubscribes += 1;
            }
          })
        })
    });

    assert.deepEqual(await store.openLogSubscription({}, () => undefined), {
      ok: false,
      error: "subscription unavailable"
    });
    assert.equal(unsubscribes, 1);
  });
});
