import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type {
  DesktopNotificationCompactSnapshot,
  DesktopNotificationOperation,
  DesktopProcessLogLine,
  DesktopThumbnailCacheSummary
} from "@megle/notification-contract";
import {
  buildCompactNotificationModel,
  thumbnailFailureLogIntent
} from "../src/features/notifications/notificationModel.ts";
import { buildProcessLogRows } from "../src/features/notifications/processLogModel.ts";

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
    failureDetails: failedCount > 0 ? [{
      libraryId: "library-a",
      fileId: 42,
      folderId: 7,
      rootId: 3,
      relativePath: "Artists\\Portraits\\a.jpg",
      failureKind: "permanent",
      error: "decode failed",
      updatedAt: 123
    }] : []
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
    assert.deepEqual(aggregate?.paths, ["Artists\\Archive", "Artists\\Portraits"]);
    assert.deepEqual(aggregate?.sourceOperations.map((item) => item.operationId), ["task:8", "task:7"]);
    assert.deepEqual(aggregate?.progress, { determinate: true, percent: 20, label: "30 / 150" });
  });

  it("makes only non-zero thumbnail failures actionable with a typed filter", () => {
    assert.deepEqual(thumbnailFailureLogIntent(summary(1)), {
      page: "logs",
      logFilter: { eventKind: "thumbnail_failure", libraryId: "library-a" }
    });
    assert.equal(thumbnailFailureLogIntent(summary(0)), null);
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
    assert.equal(rows.length, 1);
    assert.equal(rows[0]?.kind, "log");
  });
});
