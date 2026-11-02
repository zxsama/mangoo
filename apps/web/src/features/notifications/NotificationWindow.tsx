import { Activity, History, Puzzle, ScrollText } from "../../design/icons";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import type {
  DesktopProcessLogFilter,
  NotificationWindowIntent,
  ThumbnailFailureSnapshot
} from "@mangoo/notification-contract";
import { SecondaryWindowFrame } from "../../app-shell/SecondaryWindowFrame";
import { useOverlayScrollbars } from "../../app-shell/useOverlayScrollbars";
import { useSecondaryWindowInterfaceStyle } from "../../app-shell/useSecondaryWindowInterfaceStyle";
import { getDesktopBridge } from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { usePluginContributionViews, usePluginViewLauncher } from "../plugins/PluginContributionsSlot";
import { usePluginQuerySync } from "../plugins/pluginQuerySync";
import { NotificationHistoryView } from "./NotificationHistoryView";
import { NotificationStatusView, StatusSection } from "./NotificationStatusView";
import { ProcessLogView } from "./ProcessLogView";
import { useNotifications } from "./useNotifications";

type NotificationPage = "status" | "history" | "logs";

const PAGES = [
  { id: "status", label: "当前状态", icon: Activity },
  { id: "history", label: "历史记录", icon: History },
  { id: "logs", label: "进程日志", icon: ScrollText }
] as const;

export const NOTIFICATION_WINDOW_NARROW_QUERY = "(max-width: 760px)";

export function useNotificationTabOrientation(): "horizontal" | "vertical" {
  const [orientation, setOrientation] = useState<"horizontal" | "vertical">(() =>
    typeof window !== "undefined" && window.matchMedia(NOTIFICATION_WINDOW_NARROW_QUERY).matches
      ? "horizontal"
      : "vertical"
  );

  useEffect(() => {
    const mediaQuery = window.matchMedia(NOTIFICATION_WINDOW_NARROW_QUERY);
    const syncOrientation = () => setOrientation(mediaQuery.matches ? "horizontal" : "vertical");
    syncOrientation();
    mediaQuery.addEventListener("change", syncOrientation);
    window.addEventListener("resize", syncOrientation);
    return () => {
      mediaQuery.removeEventListener("change", syncOrientation);
      window.removeEventListener("resize", syncOrientation);
    };
  }, []);

  return orientation;
}

export function NotificationWindow() {
  useSecondaryWindowInterfaceStyle();
  useOverlayScrollbars();
  const notifications = useNotifications();
  const [page, setPage] = useState<NotificationPage>("status");
  const [logFilter, setLogFilter] = useState<DesktopProcessLogFilter | null>(null);
  const [failureSnapshot, setFailureSnapshot] = useState<ThumbnailFailureSnapshot | null>(null);
  const [logIntentVersion, setLogIntentVersion] = useState(0);
  const focusIntentPanelRef = useRef(false);
  const tabOrientation = useNotificationTabOrientation();

  const applyIntent = useCallback((intent: NotificationWindowIntent) => {
    if (intent.page !== "logs") return;
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
        <div
          aria-label="通知中心页面"
          aria-orientation={tabOrientation}
          className="notification-window-tabs"
          role="tablist"
        >
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
              dismissManagedRoot={notifications.dismissManagedRoot}
              model={notifications.compact}
              onOpenFailureLogs={applyIntent}
              refreshCompact={notifications.refreshCompact}
              reviewManagedRoot={notifications.reviewManagedRoot}
            >
              <NotificationPluginSection
                key={notifications.compact.activeLibraryId}
                libraryUuid={notifications.compact.activeLibraryId}
              />
            </NotificationStatusView>
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

function NotificationPluginSection({ libraryUuid }: { libraryUuid: string | null }) {
  return getDesktopBridge()?.windowRole === "notifications"
    ? <DesktopNotificationPluginSection libraryUuid={libraryUuid} />
    : <ClientNotificationPluginSection libraryUuid={libraryUuid} />;
}

function DesktopNotificationPluginSection({ libraryUuid }: { libraryUuid: string | null }) {
  const bridge = getDesktopBridge();
  const queryClient = useQueryClient();
  usePluginQuerySync(queryClient);
  const entries = useQuery({
    queryKey: ["notification-plugin-entries"],
    queryFn: () => bridge?.listNotificationPluginEntries?.() ?? Promise.resolve([]),
    refetchInterval: 30000
  });
  const [error, setError] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);
  if (!entries.data?.length) return null;
  return (
    <StatusSection className="notification-status-plugins" count={entries.data.length} headingId="notification-status-plugins-title" icon={Puzzle} title="插件">
      {entries.data.map((entry) => (
        <LiquidGlassButton key={`${entry.pluginId}:${entry.viewId}`} disabled={opening} type="button" onClick={async () => {
          setOpening(true);
          setError(null);
          try {
            if (!await bridge?.openNotificationPluginView?.({ pluginId: entry.pluginId, viewId: entry.viewId, libraryUuid })) throw new Error("插件入口暂时不可用，请重试。");
          } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
            void entries.refetch();
          } finally { setOpening(false); }
        }}>{entry.title}</LiquidGlassButton>
      ))}
      {error ? <p role="alert">{error}</p> : null}
    </StatusSection>
  );
}

function ClientNotificationPluginSection({ libraryUuid }: { libraryUuid: string | null }) {
  const views = usePluginContributionViews("notification-center");
  const launcher = usePluginViewLauncher();
  return (
    <>
      {views.length > 0 ? (
        <StatusSection
          className="notification-status-plugins"
          count={views.length}
          headingId="notification-status-plugins-title"
          icon={Puzzle}
          title="插件"
        >
          {views.map(({ plugin, view }) => (
            <LiquidGlassButton
              key={`${plugin.pluginId}:${view.id}`}
              onClick={(event) => {
                event.stopPropagation();
                launcher.open({ plugin, viewId: view.id, libraryUuid, rootId: null, rootUuid: null }, event.currentTarget);
              }}
              type="button"
            >
              {view.title}
            </LiquidGlassButton>
          ))}
        </StatusSection>
      ) : null}
      {launcher.view}
    </>
  );
}
