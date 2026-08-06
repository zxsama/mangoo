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
  const overlayCss = read("src/design/overlays/overlay.css");
  const styles = read("src/styles.css");

  it("uses the shared secondary window frame and three accessible pages", () => {
    assert.match(windowSource, /SecondaryWindowFrame/);
    assert.match(windowSource, /role="notifications"/);
    assert.match(windowSource, /role="tablist"/);
    assert.match(windowSource, /aria-orientation=\{tabOrientation\}/);
    assert.match(windowSource, /window\.matchMedia\(NOTIFICATION_WINDOW_NARROW_QUERY\)/);
    assert.match(windowSource, /addEventListener\("change"/);
    assert.match(windowSource, /removeEventListener\("change"/);
    assert.match(windowSource, /window\.addEventListener\("resize", syncOrientation\)/);
    assert.match(windowSource, /window\.removeEventListener\("resize", syncOrientation\)/);
    assert.match(windowSource, /NotificationStatusView/);
    assert.match(windowSource, /NotificationHistoryView/);
    assert.match(windowSource, /ProcessLogView/);
    assert.match(windowSource, /当前状态/);
    assert.match(windowSource, /历史记录/);
    assert.match(windowSource, /进程日志/);
    assert.match(historySource, /OverlayField/);
    assert.match(historySource, /OverlaySelect/);
    assert.match(logSource, /OverlayField/);
    assert.match(logSource, /OverlaySelect/);
    assert.doesNotMatch(windowSource, /unread|已读|未读/i);
    assert.doesNotMatch(
      windowSource + statusSource + historySource + logSource,
      /notification-view-eyebrow/
    );
  });

  it("keeps history actions capability-gated and rendering bounded", () => {
    assert.match(historySource, /cancellable/);
    assert.match(historySource, /retryable/);
    assert.match(historySource, /useVirtualizer/);
    assert.match(historySource, /loadMoreHistory/);
    assert.match(historySource, /origins/);
    assert.match(historySource, /kinds/);
    assert.match(historySource, /getHistoryFacets/);
    assert.match(historySource, /cancel/);
    assert.match(historySource, /retry/);
    assert.doesNotMatch(historySource, /MAX_CLIENT_FILTER_PAGES/);
    const forbiddenClientFilter = /visibleItems\s*=\s*useMemo\s*\(/;
    assert.match("const visibleItems = useMemo(() => []);", forbiddenClientFilter);
    assert.doesNotMatch(historySource, forbiddenClientFilter);
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
    assert.match(logSource, /focusMedia/);
    assert.match(logSource, /aria-label=\{`序列/);
    assert.doesNotMatch(logSource, /new Blob|URL\.createObjectURL|anchor\.download/);
    assert.match(logSource, /LOG_PAGE_SIZE = 200/);
    assert.match(logSource, /useLayoutEffect/);
    assert.match(logSource, /queueMicrotask/);
    assert.match(logSource, /dispatchEvent\(new Event\("scroll"\)\)/);
    assert.doesNotMatch(logSource, /setInterval/);
    assert.match(logSource, /加载更早日志/);
    assert.doesNotMatch(logSource, /firstSequence.*lastSequenceRef/);
    assert.doesNotMatch(logSource, /node:fs|fetch\s*\(/);
  });

  it("uses the approved no-divider secondary-window material", () => {
    const fullCenterCss = overlayCss.match(
      /\/\* Full notification center \*\/([\s\S]*?)(?=\n\/\* Library switcher \*\/)/
    )?.[1] ?? "";
    assert.match(overlayCss, /\/\* Full notification center \*\//);
    assert.match(overlayCss, /\.notification-window-body\s*\{/);
    assert.match(overlayCss, /\.notification-window-nav\s*\{[^}]*border-inline-end:\s*0;/s);
    assert.match(overlayCss, /\.notification-window-content\s*\{[^}]*overflow:\s*hidden;/s);
    assert.match(overlayCss, /--secondary-window-pane-dark/);
    assert.match(overlayCss, /--secondary-window-pane-light/);
    assert.match(overlayCss, /--secondary-window-pane-end/);
    assert.match(
      overlayCss,
      /\.notification-log-line\s*\{[^}]*grid-template-columns:\s*72px 92px 56px minmax\(0, 1fr\);[^}]*align-items:\s*baseline;/s
    );
    assert.match(overlayCss, /@media \(max-width:\s*760px\)[\s\S]*grid-template-rows:\s*auto minmax\(0, 1fr\)/);
    assert.doesNotMatch(
      overlayCss,
      /notification-window-nav[^}]*border-(right|inline-end):\s*1px/s
    );
    assert.doesNotMatch(fullCenterCss, /:hover/);
    assert.doesNotMatch(styles, /\/\* Full notification center|\.notification-window-body\s*\{/);
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
      "history-cancel",
      "cancelActions",
      "retryActions",
      "emitGap",
      "activeLogSubscriptions",
      "logUnsubscriptions",
      "droppedBeforeSequence",
      "NotificationWindowIntent",
      "useNotificationTabOrientation",
      "aria-orientation={tabOrientation}",
      "failureSnapshot",
      "degraded"
    ]) {
      assert.match(fixtureSource, new RegExp(token));
    }
  });
});
