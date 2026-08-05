import { Activity, History, ScrollText } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import type {
  DesktopProcessLogFilter,
  NotificationWindowIntent,
  ThumbnailFailureSnapshot
} from "@megle/notification-contract";
import { SecondaryWindowFrame } from "../../app-shell/SecondaryWindowFrame";
import { useOverlayScrollbars } from "../../app-shell/useOverlayScrollbars";
import { useSecondaryWindowInterfaceStyle } from "../../app-shell/useSecondaryWindowInterfaceStyle";
import { getDesktopBridge } from "../../core/desktop";
import { NotificationHistoryView } from "./NotificationHistoryView";
import { NotificationStatusView } from "./NotificationStatusView";
import { ProcessLogView } from "./ProcessLogView";
import { useNotifications } from "./useNotifications";

type NotificationPage = "status" | "history" | "logs";

const PAGES = [
  { id: "status", label: "当前状态", icon: Activity },
  { id: "history", label: "历史记录", icon: History },
  { id: "logs", label: "进程日志", icon: ScrollText }
] as const;

export function NotificationWindow() {
  useSecondaryWindowInterfaceStyle();
  useOverlayScrollbars();
  const notifications = useNotifications();
  const [page, setPage] = useState<NotificationPage>("status");
  const [logFilter, setLogFilter] = useState<DesktopProcessLogFilter | null>(null);
  const [failureSnapshot, setFailureSnapshot] = useState<ThumbnailFailureSnapshot | null>(null);
  const [logIntentVersion, setLogIntentVersion] = useState(0);
  const focusIntentPanelRef = useRef(false);

  const applyIntent = useCallback((intent: NotificationWindowIntent) => {
    focusIntentPanelRef.current = true;
    setLogFilter(intent.logFilter);
    setFailureSnapshot(intent.failureSnapshot);
    setLogIntentVersion((version) => version + 1);
    setPage("logs");
  }, []);

  useEffect(() => getDesktopBridge()?.onNotificationWindowIntent?.(applyIntent), [applyIntent]);

  useEffect(() => {
    if (page !== "logs" || !focusIntentPanelRef.current) return;
    focusIntentPanelRef.current = false;
    document.getElementById("notification-panel-logs")?.focus();
  }, [logIntentVersion, page]);

  const authoritativeFailures = useMemo(() => {
    if (logFilter?.eventKind !== "thumbnail_failure" || !logFilter.libraryId) return [];
    return notifications.compact.thumbnail.summaries
      .find((summary) => summary.libraryId === logFilter.libraryId)
      ?.failureDetails ?? [];
  }, [logFilter, notifications.compact.thumbnail.summaries]);

  const selectPage = useCallback((nextPage: NotificationPage) => {
    setLogFilter(null);
    setFailureSnapshot(null);
    setPage(nextPage);
  }, []);

  const handleClose = useCallback(async () => {
    const bridge = getDesktopBridge();
    if (bridge?.closeNotificationsWindow) {
      await bridge.closeNotificationsWindow();
      return;
    }
    if (bridge?.windowControls) {
      await bridge.windowControls.close();
      return;
    }
    window.close();
  }, []);

  const handleTabKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    const currentIndex = PAGES.findIndex((candidate) => candidate.id === page);
    let nextIndex: number | null = null;
    if (event.key === "ArrowDown" || event.key === "ArrowRight") {
      nextIndex = (currentIndex + 1) % PAGES.length;
    } else if (event.key === "ArrowUp" || event.key === "ArrowLeft") {
      nextIndex = (currentIndex - 1 + PAGES.length) % PAGES.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = PAGES.length - 1;
    }
    if (nextIndex === null) return;
    event.preventDefault();
    const nextPage = PAGES[nextIndex];
    selectPage(nextPage.id);
    document.getElementById(`notification-tab-${nextPage.id}`)?.focus();
  };

  return (
    <SecondaryWindowFrame
      bodyClassName="notification-window-body"
      onRequestClose={handleClose}
      role="notifications"
      title="通知中心"
      windowClassName="notification-window"
    >
      <nav aria-label="通知中心页面" className="notification-window-nav">
        <div className="notification-window-nav-heading">通知中心</div>
        <div aria-label="通知中心页面" className="notification-window-tabs" role="tablist">
          {PAGES.map(({ id, icon: Icon, label }) => (
            <button
              aria-controls={`notification-panel-${id}`}
              aria-selected={page === id}
              className={`notification-window-tab${page === id ? " active" : ""}`}
              id={`notification-tab-${id}`}
              key={id}
              onClick={() => selectPage(id)}
              onKeyDown={handleTabKeyDown}
              role="tab"
              tabIndex={page === id ? 0 : -1}
              type="button"
            >
              <Icon aria-hidden="true" size={16} />
              <span>{label}</span>
            </button>
          ))}
        </div>
        <p className="notification-window-nav-note">跨资源库状态与本次运行日志</p>
      </nav>

      <main className="notification-window-content">
        {page === "status" ? (
          <section
            aria-labelledby="notification-tab-status"
            className="notification-window-panel"
            id="notification-panel-status"
            role="tabpanel"
            tabIndex={-1}
          >
            <NotificationStatusView
              cancel={notifications.cancel}
              model={notifications.compact}
              onOpenFailureLogs={applyIntent}
              refreshCompact={notifications.refreshCompact}
            />
          </section>
        ) : null}
        {page === "history" ? (
          <section
            aria-labelledby="notification-tab-history"
            className="notification-window-panel"
            id="notification-panel-history"
            role="tabpanel"
            tabIndex={-1}
          >
            <NotificationHistoryView
              activeLibraryId={notifications.compact.activeLibraryId}
              cancel={notifications.cancel}
              getHistoryFacets={notifications.getHistoryFacets}
              history={notifications.history}
              loadHistory={notifications.loadHistory}
              loadMoreHistory={notifications.loadMoreHistory}
              refreshCompact={notifications.refreshCompact}
              retry={notifications.retry}
            />
          </section>
        ) : null}
        {page === "logs" ? (
          <section
            aria-labelledby="notification-tab-logs"
            className="notification-window-panel"
            id="notification-panel-logs"
            role="tabpanel"
            tabIndex={-1}
          >
            <ProcessLogView
              authoritativeFailures={authoritativeFailures}
              clearLogs={notifications.clearLogs}
              exportLogs={notifications.exportLogs}
              getLogPage={notifications.getLogPage}
              failureSnapshot={failureSnapshot}
              initialFilter={logFilter ?? undefined}
              key={logIntentVersion}
              openLogSubscription={notifications.openLogSubscription}
            />
          </section>
        ) : null}
      </main>
    </SecondaryWindowFrame>
  );
}
