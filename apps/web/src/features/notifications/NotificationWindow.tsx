import { Activity, History, ScrollText } from "lucide-react";
import { useCallback, useState, type KeyboardEvent } from "react";
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
  { id: "history", label: "通知历史", icon: History },
  { id: "logs", label: "进程日志", icon: ScrollText }
] as const;

export function NotificationWindow() {
  useSecondaryWindowInterfaceStyle();
  useOverlayScrollbars();
  const notifications = useNotifications();
  const [page, setPage] = useState<NotificationPage>("status");

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
    setPage(nextPage.id);
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
        <div aria-orientation="vertical" className="notification-window-tabs" role="tablist">
          {PAGES.map(({ id, icon: Icon, label }) => (
            <button
              aria-controls={`notification-panel-${id}`}
              aria-selected={page === id}
              className={`notification-window-tab${page === id ? " active" : ""}`}
              id={`notification-tab-${id}`}
              key={id}
              onClick={() => setPage(id)}
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
          >
            <NotificationStatusView
              cancel={notifications.cancel}
              model={notifications.compact}
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
          >
            <NotificationHistoryView
              cancel={notifications.cancel}
              history={notifications.history}
              listLibraries={notifications.listLibraries}
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
          >
            <ProcessLogView
              clearLogs={notifications.clearLogs}
              exportLogs={notifications.exportLogs}
              getLogPage={notifications.getLogPage}
              openLogSubscription={notifications.openLogSubscription}
            />
          </section>
        ) : null}
      </main>
    </SecondaryWindowFrame>
  );
}
