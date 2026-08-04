import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

import type { NotificationOperationViewModel } from "../src/features/notifications/notificationModel.ts";
import { activeOperationPresentation } from "../src/features/notifications/notificationPopoverPresentation.ts";

const read = (path: string) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

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
    scanPaths: [],
    sourceOperations: [],
    ...overrides
  };
}

describe("notification popover content presentation", () => {
  it("keeps phase, percentage, and current-total visible together", () => {
    assert.deepEqual(activeOperationPresentation(operation()), {
      phase: "索引文件",
      progressLabel: "42 / 100",
      percentLabel: "42%"
    });
  });

  it("keeps active scan paths in the compact activity view", () => {
    const popoverSource = read("src/features/notifications/NotificationPopover.tsx");

    assert.match(popoverSource, /operation\.scanPaths/);
    assert.match(popoverSource, /notification-scan-paths/);
  });
});

describe("compact notification product contract", () => {
  const popoverSource = read("src/features/notifications/NotificationPopover.tsx");
  const appSource = read("src/app/App.tsx");
  const fixtureSource = read("tests/fixtures/notificationPopoverVisualFixture.tsx");
  const hostSource = read("src/app-shell/ShellOverlayHost.tsx");

  it("renders current activity without terminal results or controls", () => {
    assert.match(popoverSource, /后台活动/);
    assert.match(popoverSource, /model\.active/);
    assert.match(popoverSource, /model\.thumbnail\.summaries/);
    assert.match(popoverSource, /model\.degraded/);
    assert.match(popoverSource, /onOpenCenter/);
    assert.doesNotMatch(popoverSource, /model\.results/);
    assert.doesNotMatch(popoverSource, /cancel\s*\(|retry\s*\(/);
    assert.doesNotMatch(popoverSource, /notification-result-list|最近结果/);
    assert.match(popoverSource, /AnchoredOverlay/);
  });

  it("uses the shared close and focus wiring in production", () => {
    assert.match(popoverSource, /onClose: \(\) => void/);
    assert.match(popoverSource, /onDismiss=\{onClose\}/);
    assert.match(popoverSource, /initialFocusRef=\{openCenterButtonRef\}/);
    assert.match(hostSource, /onCloseNotifications/);
    assert.match(appSource, /onCloseNotifications=\{closeNotifications\}/);
  });

  it("exposes deterministic scan, thumbnail, empty, and degraded fixture states", () => {
    assert.match(
      fixtureSource,
      /type FixtureState = "full" \| "scan" \| "thumbnail" \| "empty" \| "degraded"/
    );
    assert.match(fixtureSource, /design\/overlays\/overlay\.css/);
    assert.match(fixtureSource, /Artists\\\\Portraits/);
    assert.match(fixtureSource, /results: \[\]/);
  });
});
