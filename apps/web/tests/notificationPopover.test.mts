import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { NotificationOperationViewModel } from "../src/features/notifications/notificationModel.ts";
import { resolveNotificationPopoverPosition } from "../src/features/notifications/notificationPopoverPosition.ts";
import {
  activeOperationPresentation,
  resultOperationPresentation
} from "../src/features/notifications/notificationPopoverPresentation.ts";

function operation(
  overrides: Partial<NotificationOperationViewModel> = {}
): NotificationOperationViewModel {
  return {
    operationId: "scan:artists",
    revision: 2,
    kind: "root_scan",
    category: "task",
    origin: "user",
    presentation: "important",
    status: "running",
    phase: "索引文件",
    title: "扫描 Artists",
    summary: "扫描完成",
    taskId: 1,
    rootId: 1,
    folderId: null,
    fileId: null,
    currentValue: 42,
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
    libraryId: "artists",
    libraryName: "Artists",
    libraryLabel: "Artists",
    progress: { determinate: true, percent: 42, label: "42 / 100" },
    ...overrides
  };
}

describe("notification popover anchor geometry", () => {
  it("right-aligns to the button at an 800px viewport", () => {
    assert.deepEqual(
      resolveNotificationPopoverPosition({
        anchor: { left: 740, top: 10, right: 768, bottom: 38, width: 28, height: 28 },
        popover: { width: 360, height: 420 },
        viewport: { width: 800, height: 700 },
        gap: 4,
        margin: 8
      }),
      { left: 408, top: 42, maxHeight: 650 }
    );
  });

  it("clamps a narrow viewport without detaching vertically from the button", () => {
    assert.deepEqual(
      resolveNotificationPopoverPosition({
        anchor: { left: 276, top: 10, right: 304, bottom: 38, width: 28, height: 28 },
        popover: { width: 304, height: 500 },
        viewport: { width: 320, height: 600 },
        gap: 4,
        margin: 8
      }),
      { left: 8, top: 42, maxHeight: 550 }
    );
  });
});

describe("notification popover content presentation", () => {
  it("keeps phase, percentage, and current-total visible together", () => {
    assert.deepEqual(activeOperationPresentation(operation()), {
      phase: "索引文件",
      progressLabel: "42 / 100",
      percentLabel: "42%"
    });
  });

  it("always leads terminal detail with its library label", () => {
    assert.deepEqual(
      resultOperationPresentation(
        operation({ status: "failed", errorDetail: "读取失败", retryable: true })
      ),
      { libraryLabel: "Artists", detail: "读取失败" }
    );
    assert.deepEqual(
      resultOperationPresentation(operation({ status: "succeeded", summary: "完成" })),
      { libraryLabel: "Artists", detail: "完成" }
    );
  });
});
