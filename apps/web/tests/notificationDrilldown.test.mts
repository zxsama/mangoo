import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { describe, it } from "node:test";
import type {
  DesktopNotificationCompactSnapshot,
  DesktopNotificationOperation,
  DesktopProcessLogLine,
  DesktopThumbnailCacheSummary
} from "@mangoo/notification-contract";
import {
  activateThumbnailFailureLogs,
  buildCompactNotificationModel,
  thumbnailFailureDetailsUnavailableMessage,
  thumbnailFailureLogIntent,
  thumbnailFailureLogIntentForLibrary
} from "../src/features/notifications/notificationModel.ts";
import {
  buildProcessLogRows,
  thumbnailFailureTruncationMessage
} from "../src/features/notifications/processLogModel.ts";

function scan(id: string, summary: string, current: number, total: number | null): DesktopNotificationOperation {
  return {
    operationId: id,
    revision: 1,
    kind: "interactive_folder_scan",
    category: "task",
    origin: "user",
    presentation: "important",
    status: "running",
    phase: "working",
    title: "Scan folder",
    summary,
    taskId: Number(id.split(":")[1]),
    rootId: 1,
    folderId: Number(id.split(":")[1]),
    fileId: null,
    currentValue: current,
    totalValue: total,
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
    libraryName: "Artists"
  };
}

function summary(failedCount: number): DesktopThumbnailCacheSummary {
  const visibleCount = Math.min(failedCount, 100);
  return {
    libraryId: "library-a",
    libraryName: "Artists",
    observedAt: 500,
    cachedCount: 10,
    totalCandidateCount: 12,
    missingCount: 1,
    staleCount: 0,
    failedCount,
    pendingCount: 1,
    activeBulkTaskCount: 0,
    totalCacheBytes: 100,
    completionPercent: 83,
    failureDetails: Array.from({ length: visibleCount }, (_, index) => ({
      libraryId: "library-a",
      fileId: 42 + index,
      folderId: 7,
      rootId: 3,
      relativePath: `Artists\\Portraits\\${index}.jpg`,
      failureKind: "permanent",
      error: "decode failed",
      updatedAt: 123 + index
    })),
    failureDetailsTruncated: failedCount > visibleCount,
    failureDetailsUnavailable: false
  };
}

describe("notification drilldown", () => {
  it("aggregates active folder scans only at presentation time", () => {
    const snapshot: DesktopNotificationCompactSnapshot = {
      active: [
        scan("task:7", "Artists\\Portraits", 20, 100),
        scan("task:8", "Artists\\Archive", 10, 50),
        { ...scan("task:9", "Artists", 1, 2), kind: "root_scan", title: "Scan library" }
      ],
      thumbnailSummaries: [],
      latestImportantResults: [],
      hasRetryableFailure: false,
      syncHealth: { state: "healthy", libraryId: "library-a", detail: null, updatedAt: 1 }
    };
    const model = buildCompactNotificationModel(snapshot);

    assert.equal(model.active.length, 2);
    const aggregate = model.active.find((item) => item.kind === "interactive_folder_scan");
    assert.deepEqual(aggregate?.scanPaths, [
      {
        key: "library-a\u0000task:8",
        libraryId: "library-a",
        operationId: "task:8",
        path: "Artists\\Archive"
      },
      {
        key: "library-a\u0000task:7",
        libraryId: "library-a",
        operationId: "task:7",
        path: "Artists\\Portraits"
      }
    ]);
    assert.deepEqual(aggregate?.sourceOperations.map((item) => item.operationId), ["task:8", "task:7"]);
    assert.deepEqual(aggregate?.progress, { determinate: true, percent: 20, label: "30 / 150" });
  });

  it("makes only non-zero thumbnail failures actionable with a typed filter", () => {
    assert.deepEqual(thumbnailFailureLogIntent(summary(1)), {
      page: "logs",
      logFilter: { eventKind: "thumbnail_failure", libraryId: "library-a" },
      failureSnapshot: { totalCount: 1, visibleCount: 1, truncated: false }
    });
    assert.equal(thumbnailFailureLogIntent(summary(0)), null);
    assert.equal(thumbnailFailureLogIntent({ ...summary(1), failureDetails: [] }), null);
  });

  it("delivers the same typed click intent from status surfaces and keeps zero inert", () => {
    const opened: unknown[] = [];
    assert.equal(activateThumbnailFailureLogs(summary(1), (intent) => opened.push(intent)), true);
    assert.equal(activateThumbnailFailureLogs(summary(0), (intent) => opened.push(intent)), false);
    assert.deepEqual(opened, [{
      page: "logs",
      logFilter: { eventKind: "thumbnail_failure", libraryId: "library-a" },
      failureSnapshot: { totalCount: 1, visibleCount: 1, truncated: false }
    }]);
  });

  it("builds the Settings failure click intent only from a consistent detail snapshot", () => {
    assert.deepEqual(thumbnailFailureLogIntentForLibrary("library-a", 2, 2, false), {
      page: "logs",
      logFilter: { eventKind: "thumbnail_failure", libraryId: "library-a" },
      failureSnapshot: { totalCount: 2, visibleCount: 2, truncated: false }
    });
    assert.equal(thumbnailFailureLogIntentForLibrary("library-a", 2, 1, false), null);
    assert.equal(thumbnailFailureLogIntentForLibrary("library-a", 0, 0, false), null);
  });

  it("carries a 101+ truncation snapshot into logs and states the visible bound", () => {
    const intent = thumbnailFailureLogIntent(summary(101));
    assert.deepEqual(intent?.failureSnapshot, {
      totalCount: 101,
      visibleCount: 100,
      truncated: true
    });
    assert.equal(
      thumbnailFailureTruncationMessage(intent?.failureSnapshot),
      "仅显示前 100 条失败记录，共 101 条。"
    );
  });

  it("removes completed scan paths from the aggregate on the next snapshot", () => {
    const compact = (active: DesktopNotificationOperation[]): DesktopNotificationCompactSnapshot => ({
      active,
      thumbnailSummaries: [],
      latestImportantResults: [],
      hasRetryableFailure: false,
      syncHealth: { state: "healthy", libraryId: "library-a", detail: null, updatedAt: 1 }
    });
    const both = buildCompactNotificationModel(compact([
      scan("task:7", "Artists\\Portraits", 20, 100),
      scan("task:8", "Artists\\Archive", 10, 50)
    ]));
    const one = buildCompactNotificationModel(compact([
      scan("task:8", "Artists\\Archive", 20, 50)
    ]));
    const none = buildCompactNotificationModel(compact([]));

    assert.deepEqual(both.active[0]?.scanPaths.map((row) => row.operationId), ["task:8", "task:7"]);
    assert.deepEqual(one.active[0]?.scanPaths.map((row) => row.operationId), ["task:8"]);
    assert.equal(none.active.length, 0);
  });

  it("keeps one path row per active scan operation even when display paths match", () => {
    const snapshot: DesktopNotificationCompactSnapshot = {
      active: [
        scan("task:7", "Artists\\Portraits", 20, 100),
        scan("task:8", "Artists\\Portraits", 10, 50)
      ],
      thumbnailSummaries: [],
      latestImportantResults: [],
      hasRetryableFailure: false,
      syncHealth: { state: "healthy", libraryId: "library-a", detail: null, updatedAt: 1 }
    };

    const aggregate = buildCompactNotificationModel(snapshot).active[0];
    assert.deepEqual(aggregate?.scanPaths.map((row) => row.path), [
      "Artists\\Portraits",
      "Artists\\Portraits"
    ]);
    assert.deepEqual(aggregate?.scanPaths.map((row) => row.key), [
      "library-a\u0000task:7",
      "library-a\u0000task:8"
    ]);
    assert.equal(aggregate?.sourceOperations.length, 2);
  });

  it("renders stable operation identities in both scan path surfaces", async () => {
    const [statusSource, popoverSource] = await Promise.all([
      readFile(new URL("../src/features/notifications/NotificationStatusView.tsx", import.meta.url), "utf8"),
      readFile(new URL("../src/features/notifications/NotificationPopover.tsx", import.meta.url), "utf8")
    ]);
    for (const source of [statusSource, popoverSource]) {
      assert.match(source, /operation\.scanPaths/);
      assert.match(source, /key=\{row\.key\}/);
    }
  });

  it("preserves an honest inert state for legacy failure counts without details", () => {
    const legacy = {
      ...summary(0),
      failedCount: 7,
      failureDetails: [],
      failureDetailsTruncated: false,
      failureDetailsUnavailable: true
    };

    assert.equal(thumbnailFailureLogIntent(legacy), null);
    assert.equal(activateThumbnailFailureLogs(legacy, () => assert.fail("must remain inert")), false);
    assert.equal(
      thumbnailFailureDetailsUnavailableMessage(legacy),
      "旧记录未包含失败详情"
    );
  });

  it("merges authoritative persisted failures without duplicating current-session rows", () => {
    const current: DesktopProcessLogLine = {
      sequence: 9,
      timestamp: 200,
      source: "core",
      level: "error",
      message: "thumbnail generation failed",
      metadata: {
        eventKind: "thumbnail_failure",
        libraryId: "library-a",
        fileId: 42,
        folderId: 7,
        rootId: 3,
        relativePath: "Artists\\Portraits\\a.jpg",
        failureKind: "permanent",
        error: "decode failed"
      }
    };
    const rows = buildProcessLogRows([current], summary(2).failureDetails ?? []);
    assert.equal(rows.length, 2);
    assert.equal(rows.filter((row) => row.kind === "log").length, 1);
    assert.equal(rows.filter((row) => row.kind === "failure").length, 1);
  });

  it("chronologically merges persistent failures with live logs so follow lands newest", () => {
    const logs: DesktopProcessLogLine[] = [
      { sequence: 2, timestamp: 400, source: "main", level: "info", message: "newest" },
      { sequence: 1, timestamp: 200, source: "main", level: "info", message: "middle" }
    ];
    const failures = [
      { ...summary(2).failureDetails[0]!, fileId: 80, updatedAt: 300 },
      { ...summary(2).failureDetails[1]!, fileId: 70, updatedAt: 100 }
    ];

    const rows = buildProcessLogRows(logs, failures);
    assert.deepEqual(
      rows.map((row) => row.kind === "log" ? row.line.timestamp : row.detail.updatedAt),
      [100, 200, 300, 400]
    );
  });
});
