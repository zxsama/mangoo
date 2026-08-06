import { Activity, History, ScrollText, X } from "lucide-react";
import { StrictMode, useCallback, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import type {
  DesktopNotificationCompactSnapshot,
  DesktopNotificationHistoryQuery,
  DesktopNotificationOperation,
  DesktopProcessLogBatch,
  DesktopProcessLogFilter,
  DesktopProcessLogLine,
  NotificationWindowIntent,
  ThumbnailFailureDetail
} from "@megle/notification-contract";
import { useOverlayScrollbars } from "../../src/app-shell/useOverlayScrollbars";
import { LiquidGlassLayer, LiquidGlassSurface } from "../../src/design/liquid-glass";
import { NotificationHistoryView } from "../../src/features/notifications/NotificationHistoryView";
import { NotificationStatusView } from "../../src/features/notifications/NotificationStatusView";
import { useNotificationTabOrientation } from "../../src/features/notifications/NotificationWindow";
import { ProcessLogView } from "../../src/features/notifications/ProcessLogView";
import { buildCompactNotificationModel } from "../../src/features/notifications/notificationModel";
import type { NotificationHistoryState } from "../../src/features/notifications/useNotifications";
import "../../src/styles.css";
import "../../src/design/overlays/overlay.css";
import "./notificationWindowVisualFixture.css";

type FixturePage = "status" | "history" | "logs";
type FixtureState = "full" | "degraded";

interface FixtureMetrics {
  historyQueries: number;
  cancelActions: number;
  retryActions: number;
  logSubscriptions: number;
  activeLogSubscriptions: number;
  logUnsubscriptions: number;
}

function fixtureFailureDetails(
  libraryId: string,
  count: number,
  firstFileId: number
): ThumbnailFailureDetail[] {
  return Array.from({ length: count }, (_, index) => ({
    libraryId,
    fileId: firstFileId + index,
    folderId: 7,
    rootId: 3,
    relativePath: `${libraryId}\\failure-${index + 1}.jpg`,
    failureKind: "decode",
    error: "decode failed",
    updatedAt: now + index
  }));
}

interface NotificationWindowFixtureController {
  emitGap(): void;
  getMetrics(): FixtureMetrics;
  selectPage(page: FixturePage): void;
}

declare global {
  interface Window {
    notificationWindowFixture?: NotificationWindowFixtureController;
  }
}

const parameters = new URLSearchParams(window.location.search);
const fixtureState: FixtureState = parameters.get("state") === "degraded" ? "degraded" : "full";
const now = Date.now();

function operation(index: number, overrides: Partial<DesktopNotificationOperation> = {}): DesktopNotificationOperation {
  const libraryId = index % 2 === 0 ? "artists" : "references";
  return {
    operationId: `fixture-operation-${index}`,
    revision: 1,
    kind: index % 3 === 0 ? "root_scan" : "file_move",
    category: index % 3 === 0 ? "task" : "file_operation",
    origin: index % 4 === 0 ? "background" : "user",
    presentation: "important",
    status: index % 7 === 0 ? "failed" : "succeeded",
    phase: "完成",
    title: `Fixture operation ${index}`,
    summary: `Processed ${index + 1} items`,
    taskId: index,
    rootId: index % 4,
    folderId: index % 9,
    fileId: index,
    currentValue: index + 1,
    totalValue: index + 1,
    unit: "items",
    errorCode: index % 7 === 0 ? "fixture_error" : null,
    errorDetail: index % 7 === 0 ? "Fixture action failed" : null,
    cancellable: false,
    retryable: libraryId === "artists" && index % 7 === 0,
    attempt: 1,
    occurrenceCount: 1,
    startedAt: now - index * 2_000 - 4_000,
    updatedAt: now - index * 2_000,
    completedAt: now - index * 2_000,
    libraryId,
    libraryName: libraryId === "artists" ? "Artists" : "References",
    ...overrides
  };
}

const activeOperations = [
  operation(900, {
    operationId: "active-scan",
    status: "running",
    phase: "扫描媒体",
    title: "重新索引 Artists",
    currentValue: 42,
    totalValue: 100,
    completedAt: null,
    cancellable: true
  }),
  operation(901, {
    operationId: "active-purge",
    status: "running",
    phase: "准备删除",
    title: "清理 References",
    currentValue: 0,
    totalValue: null,
    completedAt: null,
    cancellable: false
  })
];

const compactSnapshot: DesktopNotificationCompactSnapshot = {
  active: activeOperations,
  thumbnailSummaries: [
    {
      libraryId: "artists",
      libraryName: "Artists",
      observedAt: now,
      cachedCount: 820,
      totalCandidateCount: 1_000,
      missingCount: 120,
      staleCount: 24,
      failedCount: 4,
      pendingCount: 32,
      activeBulkTaskCount: 1,
      totalCacheBytes: 86 * 1024 * 1024,
      completionPercent: 82,
      failureDetails: fixtureFailureDetails("artists", 4, 42),
      failureDetailsTruncated: false,
      failureDetailsUnavailable: false
    },
    {
      libraryId: "references",
      libraryName: "References",
      observedAt: now,
      cachedCount: 500,
      totalCandidateCount: 800,
      missingCount: 220,
      staleCount: 48,
      failedCount: 9,
      pendingCount: 18,
      activeBulkTaskCount: 2,
      totalCacheBytes: 54 * 1024 * 1024,
      completionPercent: 63,
      failureDetails: fixtureFailureDetails("references", 9, 84),
      failureDetailsTruncated: false,
      failureDetailsUnavailable: false
    }
  ],
  latestImportantResults: [
    operation(1, { title: "删除 Archive 索引" }),
    operation(6, { operationId: "retry-error", status: "failed", retryable: true, title: "移动 Portraits" }),
    operation(2, { status: "cancelled", title: "扫描已取消" })
  ],
  hasRetryableFailure: true,
  syncHealth: {
    state: fixtureState === "degraded" ? "degraded" : "healthy",
    libraryId: "artists",
    detail: fixtureState === "degraded" ? "测试同步连接已暂停" : null,
    updatedAt: now
  }
};

const allHistory = Array.from({ length: 240 }, (_, index) => {
  if (index === 5) {
    return operation(index, {
      operationId: "history-cancel",
      status: "running",
      completedAt: null,
      cancellable: true
    });
  }
  return operation(
    index,
    index === 6 ? { operationId: "retry-error", status: "failed", retryable: true } : {}
  );
});

function filterHistory(query: DesktopNotificationHistoryQuery) {
  return allHistory.filter((item) => {
    if (query.libraryId && item.libraryId !== query.libraryId) return false;
    if (query.statuses?.length && !query.statuses.includes(item.status)) return false;
    if (query.categories?.length && !query.categories.includes(item.category)) return false;
    if (query.origins?.length && !query.origins.includes(item.origin)) return false;
    if (query.kinds?.length && !query.kinds.includes(item.kind)) return false;
    if (
      query.search &&
      !`${item.title} ${item.summary} ${item.errorDetail}`
        .toLowerCase()
        .includes(query.search.toLowerCase())
    ) return false;
    return true;
  });
}

const allLogs: DesktopProcessLogLine[] = Array.from({ length: 260 }, (_, index) => ({
  sequence: index + 1,
  timestamp: now - (260 - index) * 200,
  source: (["main", "core", "renderer"] as const)[index % 3],
  level: (["debug", "info", "warn", "error"] as const)[index % 4],
  message: `Fixture log line ${index + 1}`,
  metadata: index % 9 === 0 ? { fixture: true, index } : undefined
}));

function NotificationWindowVisualFixture() {
  useOverlayScrollbars();
  const tabOrientation = useNotificationTabOrientation();
  const [page, setPage] = useState<FixturePage>("status");
  const [logIntent, setLogIntent] = useState<NotificationWindowIntent | null>(null);
  const [history, setHistory] = useState<NotificationHistoryState>({
    items: [],
    total: allHistory.length,
    nextOffset: 0,
    conflictError: null,
    query: null,
    loading: false,
    error: null
  });
  const metricsRef = useRef<FixtureMetrics>({
    historyQueries: 0,
    cancelActions: 0,
    retryActions: 0,
    logSubscriptions: 0,
    activeLogSubscriptions: 0,
    logUnsubscriptions: 0
  });
  const logSubscriptionsRef = useRef(
    new Set<{ listener: (batch: DesktopProcessLogBatch) => void }>()
  );

  const loadHistory = useCallback(async (query: DesktopNotificationHistoryQuery) => {
    metricsRef.current.historyQueries += 1;
    const offset = query.offset ?? 0;
    const filtered = filterHistory(query);
    const items = filtered.slice(offset, offset + (query.limit ?? 100));
    const nextOffset = offset + items.length < filtered.length ? offset + items.length : null;
    const next: NotificationHistoryState = {
      items,
      total: filtered.length,
      nextOffset,
      conflictError: null,
      query: { ...query, offset: undefined },
      loading: false,
      error: null
    };
    setHistory(next);
    return next;
  }, []);

  const loadMoreHistory = useCallback(async () => {
    const query = history.query ?? { limit: 100 };
    const offset = history.nextOffset ?? history.items.length;
    const filtered = filterHistory(query);
    const pageItems = filtered.slice(offset, offset + (query.limit ?? 100));
    const next: NotificationHistoryState = {
      ...history,
      items: [...history.items, ...pageItems],
      total: filtered.length,
      nextOffset: offset + pageItems.length < filtered.length ? offset + pageItems.length : null,
      loading: false
    };
    setHistory(next);
    return next;
  }, [history]);

  const openLogSubscription = useCallback(async (
    query: DesktopProcessLogFilter,
    listener: (batch: DesktopProcessLogBatch) => void
  ) => {
    metricsRef.current.logSubscriptions += 1;
    const subscription = { listener };
    logSubscriptionsRef.current.add(subscription);
    metricsRef.current.activeLogSubscriptions = logSubscriptionsRef.current.size;
    const snapshot = filterLogs(query).slice(-(query.limit ?? 5_000));
    let active = true;
    return {
      ok: true as const,
      value: {
        snapshot: { snapshot, afterSequence: snapshot.at(-1)?.sequence ?? 0 },
        unsubscribe() {
          if (!active) return;
          active = false;
          logSubscriptionsRef.current.delete(subscription);
          metricsRef.current.activeLogSubscriptions = logSubscriptionsRef.current.size;
          metricsRef.current.logUnsubscriptions += 1;
        }
      }
    };
  }, []);

  const selectPage = useCallback((nextPage: FixturePage) => {
    setPage(nextPage);
    if (nextPage !== "logs") setLogIntent(null);
  }, []);
  const cancel = useCallback(async () => {
    metricsRef.current.cancelActions += 1;
    return { ok: true, message: null };
  }, []);
  const retry = useCallback(async (_libraryId: string, operationId: string) => {
    metricsRef.current.retryActions += 1;
    return operationId === "retry-error"
      ? { ok: false, message: "Fixture retry rejected" }
      : { ok: true, message: null };
  }, []);

  window.notificationWindowFixture = {
    emitGap() {
      const batch: DesktopProcessLogBatch = {
        lines: [{
          sequence: 900,
          timestamp: Date.now(),
          source: "core",
          level: "warn",
          message: "Fixture gap delivery"
        }],
        afterSequence: 900,
        droppedBeforeSequence: 850
      };
      for (const subscription of logSubscriptionsRef.current) subscription.listener(batch);
    },
    getMetrics: () => ({ ...metricsRef.current }),
    selectPage
  };

  const tabs = [
    { id: "status", label: "当前状态", icon: Activity },
    { id: "history", label: "历史记录", icon: History },
    { id: "logs", label: "进程日志", icon: ScrollText }
  ] as const;

  return (
    <LiquidGlassLayer>
      <LiquidGlassSurface
        as="div"
        className="secondary-window notification-window notification-window-fixture"
        data-notification-window-fixture={fixtureState}
        data-secondary-content-visible="true"
        tone="chrome"
      >
        <header className="secondary-window-titlebar">
          <span className="secondary-window-title">通知中心</span>
          <button aria-label="关闭" className="secondary-window-close" type="button"><X aria-hidden="true" size={16} /></button>
        </header>
        <div className="secondary-window-body notification-window-body">
          <nav aria-label="通知中心页面" className="notification-window-nav">
            <div className="notification-window-nav-heading">通知中心</div>
            <div
              aria-label="通知中心页面"
              aria-orientation={tabOrientation}
              className="notification-window-tabs"
              role="tablist"
            >
              {tabs.map(({ id, icon: Icon, label }) => (
                <button
                  aria-controls={`notification-fixture-panel-${id}`}
                  aria-selected={page === id}
                  className={`notification-window-tab${page === id ? " active" : ""}`}
                  data-fixture-page={id}
                  id={`notification-fixture-tab-${id}`}
                  key={id}
                  onClick={() => selectPage(id)}
                  role="tab"
                  tabIndex={page === id ? 0 : -1}
                  type="button"
                >
                  <Icon aria-hidden="true" size={16} /><span>{label}</span>
                </button>
              ))}
            </div>
            <p className="notification-window-nav-note">跨资源库状态与本次运行日志</p>
          </nav>
          <main className="notification-window-content">
            <section
              aria-labelledby={`notification-fixture-tab-${page}`}
              className="notification-window-panel"
              id={`notification-fixture-panel-${page}`}
              role="tabpanel"
            >
              {page === "status" ? (
                <NotificationStatusView
                  cancel={cancel}
                  model={buildCompactNotificationModel(compactSnapshot)}
                  onOpenFailureLogs={(intent) => {
                    setLogIntent(intent);
                    setPage("logs");
                  }}
                  refreshCompact={async () => ({ ok: true, value: true })}
                />
              ) : null}
              {page === "history" ? (
                <NotificationHistoryView
                  activeLibraryId="artists"
                  cancel={cancel}
                  history={history}
                  getHistoryFacets={async () => ({
                    ok: true,
                    value: {
                      libraries: [
                        { id: "artists", name: "Artists" },
                        { id: "references", name: "References" }
                      ],
                      kinds: ["file_move", "root_scan"]
                    }
                  })}
                  loadHistory={loadHistory}
                  loadMoreHistory={loadMoreHistory}
                  refreshCompact={async () => ({ ok: true, value: true })}
                  retry={retry}
                />
              ) : null}
              {page === "logs" ? (
                <ProcessLogView
                  authoritativeFailures={
                    logIntent?.logFilter.eventKind === "thumbnail_failure"
                      ? compactSnapshot.thumbnailSummaries.find(
                          (summary) => summary.libraryId === logIntent.logFilter.libraryId
                        )?.failureDetails ?? []
                      : []
                  }
                  clearLogs={async () => ({ ok: true, value: true })}
                  exportLogs={async () => ({
                    ok: true,
                    value: {
                      saved: true,
                      canceled: false,
                      filePath: "fixture.txt",
                      lineCount: allLogs.length,
                      error: null
                    }
                  })}
                  failureSnapshot={logIntent?.failureSnapshot}
                  getLogPage={async (query) => ({ ok: true, value: pageLogs(query) })}
                  initialFilter={logIntent?.logFilter}
                  key={`${logIntent?.logFilter.eventKind ?? "session"}:${logIntent?.logFilter.libraryId ?? "all"}`}
                  openLogSubscription={openLogSubscription}
                />
              ) : null}
            </section>
          </main>
        </div>
      </LiquidGlassSurface>
    </LiquidGlassLayer>
  );
}

function filterLogs(query: DesktopProcessLogFilter): DesktopProcessLogLine[] {
  return allLogs.filter((line) => {
    if (query.source && line.source !== query.source) return false;
    if (query.level && line.level !== query.level) return false;
    if (query.search && !line.message.toLowerCase().includes(query.search.toLowerCase())) return false;
    if (query.beforeSequence != null && line.sequence >= query.beforeSequence) return false;
    if (query.afterSequence != null && line.sequence <= query.afterSequence) return false;
    return true;
  });
}

function pageLogs(query: DesktopProcessLogFilter) {
  const matching = filterLogs(query);
  const limit = query.limit ?? 200;
  const lines = matching.slice(-limit);
  return {
    lines,
    hasMore: matching.length > lines.length,
    nextBeforeSequence: matching.length > lines.length ? lines[0]?.sequence ?? null : null
  };
}

const root = document.getElementById("root");
if (!root) throw new Error("notification window fixture root is missing");
createRoot(root).render(<StrictMode><NotificationWindowVisualFixture /></StrictMode>);
