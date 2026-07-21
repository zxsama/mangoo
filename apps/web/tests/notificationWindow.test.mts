import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const read = (relativePath: string) =>
  readFileSync(new URL(`../${relativePath}`, import.meta.url), "utf8");

describe("full notification center contract", () => {
  const windowSource = read("src/features/notifications/NotificationWindow.tsx");
  const statusSource = read("src/features/notifications/NotificationStatusView.tsx");
  const historySource = read("src/features/notifications/NotificationHistoryView.tsx");
  const logSource = read("src/features/notifications/ProcessLogView.tsx");
  const fixtureSource = read("tests/fixtures/notificationWindowVisualFixture.tsx");
  const styles = read("src/styles.css");

  it("uses the shared secondary window frame and three accessible pages", () => {
    assert.match(windowSource, /SecondaryWindowFrame/);
    assert.match(windowSource, /role="notifications"/);
    assert.match(windowSource, /role="tablist"/);
    assert.match(windowSource, /NotificationStatusView/);
    assert.match(windowSource, /NotificationHistoryView/);
    assert.match(windowSource, /ProcessLogView/);
    assert.doesNotMatch(windowSource, /unread|已读|未读/i);
  });

  it("keeps history actions capability-gated and rendering bounded", () => {
    assert.match(historySource, /cancellable/);
    assert.match(historySource, /retryable/);
    assert.match(historySource, /useVirtualizer/);
    assert.match(historySource, /loadMoreHistory/);
    assert.match(historySource, /origins/);
    assert.match(historySource, /kinds/);
    assert.match(historySource, /getHistoryFacets/);
    assert.doesNotMatch(historySource, /MAX_CLIENT_FILTER_PAGES/);
    assert.doesNotMatch(historySource, /visibleItemss*=s*useMemo/);
    assert.doesNotMatch(historySource, /raw fetch|node:fs/i);
  });

  it("exposes cancellation for active operations in the full status view", () => {
    assert.match(statusSource, /cancel/);
    assert.match(statusSource, /cancellable/);
    assert.match(windowSource, /refreshCompact/);
  });

  it("subscribes to logs only while mounted and renders a bounded window", () => {
    assert.match(logSource, /openLogSubscription/);
    assert.match(logSource, /unsubscribe/);
    assert.match(logSource, /useVirtualizer/);
    assert.match(logSource, /processLogGapSequence/);
    assert.match(logSource, /createProcessLogPageRequestController/);
    assert.match(logSource, /appendPausedProcessLogBatch/);
    assert.match(logSource, /coalescePausedProcessLogBuffer/);
    assert.doesNotMatch(logSource, /queryGenerationRef/);
    assert.match(logSource, /clearLogs/);
    assert.match(logSource, /exportLogs/);
    assert.doesNotMatch(logSource, /new Blob|URL\.createObjectURL|anchor\.download/);
    assert.match(logSource, /LOG_PAGE_SIZE = 200/);
    assert.match(logSource, /加载更早日志/);
    assert.doesNotMatch(logSource, /firstSequence.*lastSequenceRef/);
    assert.doesNotMatch(logSource, /node:fs|fetch\s*\(/);
  });

  it("uses the approved no-divider secondary-window material", () => {
    assert.match(styles, /\.notification-window-body\s*\{/);
    assert.match(styles, /\.notification-window-nav\s*\{[^}]*border-inline-end:\s*0;/s);
    assert.match(styles, /\.notification-window-content\s*\{[^}]*overflow:\s*hidden;/s);
    assert.match(styles, /var\(--secondary-window-pane-dark\)/);
    assert.match(styles, /var\(--secondary-window-pane-light\)/);
    assert.match(styles, /var\(--secondary-window-pane-end\)/);
  });

  it("surfaces status, thumbnail, result, and degraded states", () => {
    for (const token of [
      "notification-status-active",
      "notification-status-thumbnails",
      "notification-status-results",
      "notification-status-health",
      "notification-status-empty"
    ]) {
      assert.match(statusSource, new RegExp(token));
    }
  });

  it("ships a controlled renderer fixture for full-window interaction states", () => {
    for (const token of [
      "active-scan",
      "active-purge",
      "thumbnailSummaries",
      "retry-error",
      "emitGap",
      "activeLogSubscriptions",
      "logUnsubscriptions",
      "droppedBeforeSequence",
      "degraded"
    ]) {
      assert.match(fixtureSource, new RegExp(token));
    }
  });
});
