import { ChevronDown, ChevronRight, RefreshCw, RotateCcw, Square } from "lucide-react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties
} from "react";
import type {
  DesktopNotificationCategory,
  DesktopNotificationHistoryFacets,
  DesktopNotificationHistoryQuery,
  DesktopNotificationOperation,
  DesktopNotificationStatus
} from "@megle/notification-contract";
import type {
  NotificationHistoryState,
  NotificationRequestResult
} from "./useNotifications";
import {
  beginNotificationAction,
  notificationActionKey,
  settleNotificationAction,
  type NotificationActionStates
} from "./notificationActionModel";

const PAGE_SIZE = 100;

type NotificationAction = (
  libraryId: string,
  operationId: string
) => Promise<{ ok: boolean; message: string | null }>;

interface NotificationHistoryViewProps {
  activeLibraryId: string | null;
  cancel: NotificationAction;
  history: NotificationHistoryState;
  getHistoryFacets(): Promise<NotificationRequestResult<DesktopNotificationHistoryFacets>>;
  loadHistory(query: DesktopNotificationHistoryQuery): Promise<NotificationHistoryState>;
  loadMoreHistory(): Promise<NotificationHistoryState>;
  refreshCompact(): Promise<NotificationRequestResult<boolean>>;
  retry: NotificationAction;
}

export function NotificationHistoryView({
  activeLibraryId,
  cancel,
  getHistoryFacets,
  history,
  loadHistory,
  loadMoreHistory,
  refreshCompact,
  retry
}: NotificationHistoryViewProps) {
  const [search, setSearch] = useState("");
  const deferredSearch = useDeferredValue(search.trim());
  const [libraryId, setLibraryId] = useState("");
  const [status, setStatus] = useState<DesktopNotificationStatus | "">("");
  const [category, setCategory] = useState<DesktopNotificationCategory | "">("");
  const [source, setSource] = useState<DesktopNotificationOperation["origin"] | "">("");
  const [kind, setKind] = useState("");
  const [facets, setFacets] = useState<DesktopNotificationHistoryFacets>({
    libraries: [],
    kinds: []
  });
  const [facetError, setFacetError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [actionStates, setActionStates] = useState<NotificationActionStates>(() => new Map());
  const actionStatesRef = useRef(actionStates);
  const actionRequestIdRef = useRef(0);
  const facetRequestGenerationRef = useRef(0);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const query = useMemo<DesktopNotificationHistoryQuery>(
    () => ({
      ...(libraryId ? { libraryId } : {}),
      ...(status ? { statuses: [status] } : {}),
      ...(category ? { categories: [category] } : {}),
      ...(source ? { origins: [source] } : {}),
      ...(kind ? { kinds: [kind] } : {}),
      ...(deferredSearch ? { search: deferredSearch } : {}),
      limit: PAGE_SIZE
    }),
    [category, deferredSearch, kind, libraryId, source, status]
  );
  const queryRef = useRef<DesktopNotificationHistoryQuery>(query);
  const activeLibraryIdRef = useRef(activeLibraryId);

  const loadCurrentQuery = useCallback(
    (nextQuery: DesktopNotificationHistoryQuery) => loadHistory(nextQuery),
    [loadHistory]
  );

  useEffect(() => {
    queryRef.current = query;
    void loadCurrentQuery(query);
  }, [loadCurrentQuery, query]);

  const loadFacets = useCallback(async () => {
    const generation = ++facetRequestGenerationRef.current;
    const result = await getHistoryFacets();
    if (generation !== facetRequestGenerationRef.current) return;
    if (!result.ok) {
      setFacetError(result.error);
      return;
    }
    setFacets({
      libraries: [...result.value.libraries].sort(
        (left, right) => left.name.localeCompare(right.name, "zh-CN")
      ),
      kinds: [...result.value.kinds]
    });
    setFacetError(null);
  }, [getHistoryFacets]);

  useEffect(() => {
    if (activeLibraryIdRef.current === activeLibraryId) return;
    activeLibraryIdRef.current = activeLibraryId;
    const clearedActionStates: NotificationActionStates = new Map();
    actionStatesRef.current = clearedActionStates;
    setActionStates(clearedActionStates);
    if (!history.query) return;
    void Promise.all([loadCurrentQuery(queryRef.current), loadFacets()]);
  }, [activeLibraryId, history.query, loadCurrentQuery, loadFacets]);

  useEffect(() => {
    void loadFacets();
    return () => {
      facetRequestGenerationRef.current += 1;
    };
  }, [loadFacets]);

  const virtualizer = useVirtualizer({
    count: history.items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 82,
    overscan: 7,
    getItemKey: (index) => {
      const item = history.items[index];
      return item ? `${item.libraryId}:${item.operationId}:${item.revision}` : index;
    }
  });

  const toggleExpanded = (key: string) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const runAction = async (
    action: "cancel" | "retry",
    operation: DesktopNotificationOperation
  ) => {
    const actionKey = notificationActionKey(
      operation.libraryId,
      operation.operationId,
      action
    );
    const requestId = ++actionRequestIdRef.current;
    const started = beginNotificationAction(
      actionStatesRef.current,
      actionKey,
      requestId
    );
    if (!started.started) return;
    actionStatesRef.current = started.states;
    setActionStates(started.states);
    const result = await (action === "cancel" ? cancel : retry)(
      operation.libraryId,
      operation.operationId
    );
    await Promise.all([
      loadCurrentQuery(queryRef.current),
      refreshCompact(),
      loadFacets()
    ]);
    const settled = settleNotificationAction(
      actionStatesRef.current,
      actionKey,
      requestId,
      result.ok ? null : result.message || "操作未完成"
    );
    actionStatesRef.current = settled;
    setActionStates(settled);
  };

  const resetFilters = () => {
    setSearch("");
    setLibraryId("");
    setStatus("");
    setCategory("");
    setSource("");
    setKind("");
  };

  return (
    <div className="notification-view notification-history-view">
      <header className="notification-view-heading notification-history-heading">
        <div>
          <span className="notification-view-eyebrow">ALL LIBRARIES</span>
          <h1>通知历史</h1>
        </div>
        <div className="notification-heading-actions">
          <span>{history.total.toLocaleString()} 条记录</span>
          <button
            aria-label="刷新通知历史"
            className="notification-icon-action"
            disabled={history.loading}
            onClick={() => void Promise.all([loadCurrentQuery(query), loadFacets()])}
            type="button"
          >
            <RefreshCw aria-hidden="true" size={15} />
          </button>
        </div>
      </header>

      <div aria-label="通知历史筛选" className="notification-history-filters" role="search">
        <label className="notification-filter-search">
          <span className="sr-only">搜索通知</span>
          <input
            onChange={(event) => setSearch(event.currentTarget.value)}
            placeholder="搜索标题、目标或错误"
            type="search"
            value={search}
          />
        </label>
        <FilterSelect label="资源库" onChange={setLibraryId} value={libraryId}>
          <option value="">所有资源库</option>
          {facets.libraries.map(({ id, name }) => <option key={id} value={id}>{name}</option>)}
        </FilterSelect>
        <FilterSelect
          label="状态"
          onChange={(value) => setStatus(value as DesktopNotificationStatus | "")}
          value={status}
        >
          <option value="">所有状态</option>
          <option value="pending">等待中</option>
          <option value="running">进行中</option>
          <option value="succeeded">已完成</option>
          <option value="failed">失败</option>
          <option value="cancelled">已取消</option>
        </FilterSelect>
        <FilterSelect
          label="类别"
          onChange={(value) => setCategory(value as DesktopNotificationCategory | "")}
          value={category}
        >
          <option value="">所有类别</option>
          <option value="task">后台任务</option>
          <option value="file_operation">文件操作</option>
          <option value="error">错误</option>
          <option value="thumbnail_cache">缩略图缓存</option>
        </FilterSelect>
        <FilterSelect
          label="来源"
          onChange={(value) => setSource(value as DesktopNotificationOperation["origin"] | "")}
          value={source}
        >
          <option value="">所有来源</option>
          <option value="user">用户操作</option>
          <option value="background">后台</option>
          <option value="recovery">恢复</option>
          <option value="system">系统</option>
        </FilterSelect>
        <FilterSelect label="类型" onChange={setKind} value={kind}>
          <option value="">所有类型</option>
          {facets.kinds.map((value) => <option key={value} value={value}>{kindLabel(value)}</option>)}
        </FilterSelect>
        <button className="notification-text-action" onClick={resetFilters} type="button">
          重置
        </button>
      </div>

      {facetError ? <p className="notification-inline-error" role="alert">{facetError}</p> : null}
      {history.error ? <p className="notification-inline-error" role="alert">{history.error}</p> : null}

      <div className="notification-history-list" ref={scrollRef}>
        {history.items.length === 0 && !history.loading ? (
          <div className="notification-list-empty">没有符合当前筛选条件的通知。</div>
        ) : (
          <div
            className="notification-virtual-space"
            style={{ height: virtualizer.getTotalSize() }}
          >
            {virtualizer.getVirtualItems().map((virtualRow) => {
              const operation = history.items[virtualRow.index];
              const operationKey = `${operation.libraryId}:${operation.operationId}`;
              const cancelKey = notificationActionKey(
                operation.libraryId,
                operation.operationId,
                "cancel"
              );
              const retryKey = notificationActionKey(
                operation.libraryId,
                operation.operationId,
                "retry"
              );
              const cancelState = actionStates.get(cancelKey);
              const retryState = actionStates.get(retryKey);
              const actionError = cancelState?.error ?? retryState?.error ?? null;
              const isExpanded = expanded.has(operationKey);
              return (
                <div
                  className="notification-virtual-row"
                  data-index={virtualRow.index}
                  key={virtualRow.key}
                  ref={virtualizer.measureElement}
                  style={{
                    transform: `translateY(${virtualRow.start}px)`
                  } as CSSProperties}
                >
                  <article
                    className={`notification-history-row status-${operation.status}`}
                    data-notification-operation={operationKey}
                  >
                    <button
                      aria-expanded={isExpanded}
                      aria-label={`${isExpanded ? "收起" : "展开"}${operation.title}`}
                      className="notification-history-disclosure"
                      onClick={() => toggleExpanded(operationKey)}
                      type="button"
                    >
                      {isExpanded ? <ChevronDown aria-hidden="true" size={15} /> : <ChevronRight aria-hidden="true" size={15} />}
                    </button>
                    <div className="notification-history-summary">
                      <div className="notification-history-title-row">
                        <strong>{operation.title}</strong>
                        <span className={`notification-status-pill status-${operation.status}`}>
                          {statusLabel(operation.status)}
                        </span>
                      </div>
                      <div className="notification-history-meta">
                        <span>{operation.libraryName || operation.libraryId}</span>
                        <span>{categoryLabel(operation.category)}</span>
                        <span>{kindLabel(operation.kind)}</span>
                        <time dateTime={new Date(operation.updatedAt).toISOString()}>
                          {formatDateTime(operation.updatedAt)}
                        </time>
                      </div>
                      {isExpanded ? <OperationDetails operation={operation} /> : null}
                      {actionError ? (
                        <p className="notification-row-action-error" role="alert">{actionError}</p>
                      ) : null}
                    </div>
                    <div className="notification-history-actions">
                      {operation.cancellable ? (
                        <button
                          aria-label={`取消 ${operation.title}`}
                          className="notification-row-action"
                          disabled={cancelState?.pending === true}
                          onClick={() => void runAction("cancel", operation)}
                          type="button"
                        >
                          <Square aria-hidden="true" size={13} />
                          {cancelState?.pending ? "取消中…" : "取消"}
                        </button>
                      ) : null}
                      {operation.retryable ? (
                        <button
                          aria-label={`重试 ${operation.title}`}
                          className="notification-row-action"
                          disabled={retryState?.pending === true}
                          onClick={() => void runAction("retry", operation)}
                          type="button"
                        >
                          <RotateCcw aria-hidden="true" size={13} />
                          {retryState?.pending ? "重试中…" : "重试"}
                        </button>
                      ) : null}
                    </div>
                  </article>
                </div>
              );
            })}
          </div>
        )}
      </div>

      <footer className="notification-history-footer">
        <span>
          已加载 {history.items.length.toLocaleString()} / {history.total.toLocaleString()}
        </span>
        {history.nextOffset !== null ? (
          <button
            className="notification-primary-action"
            disabled={history.loading}
            onClick={() => void loadMoreHistory()}
            type="button"
          >
            {history.loading ? "加载中…" : `再加载 ${PAGE_SIZE} 条`}
          </button>
        ) : null}
      </footer>
    </div>
  );
}

function FilterSelect({
  children,
  label,
  onChange,
  value
}: {
  children: React.ReactNode;
  label: string;
  onChange(value: string): void;
  value: string;
}) {
  return (
    <label className="notification-filter-select">
      <span>{label}</span>
      <select onChange={(event) => onChange(event.currentTarget.value)} value={value}>
        {children}
      </select>
    </label>
  );
}

function OperationDetails({ operation }: { operation: DesktopNotificationOperation }) {
  const target = [
    operation.rootId == null ? null : `Root ${operation.rootId}`,
    operation.folderId == null ? null : `Folder ${operation.folderId}`,
    operation.fileId == null ? null : `File ${operation.fileId}`
  ].filter(Boolean).join(" / ");
  return (
    <dl className="notification-history-details">
      <div><dt>来源</dt><dd>{originLabel(operation.origin)}</dd></div>
      <div><dt>目标</dt><dd>{target || "全局操作"}</dd></div>
      <div><dt>阶段</dt><dd>{operation.phase || operation.summary || "-"}</dd></div>
      <div><dt>尝试</dt><dd>{operation.attempt}</dd></div>
      <div><dt>开始</dt><dd>{formatDateTime(operation.startedAt)}</dd></div>
      <div><dt>更新</dt><dd>{formatDateTime(operation.updatedAt)}</dd></div>
      {operation.completedAt == null ? null : (
        <div><dt>完成</dt><dd>{formatDateTime(operation.completedAt)}</dd></div>
      )}
      {operation.errorDetail ? (
        <div className="notification-history-error-detail"><dt>错误</dt><dd>{operation.errorDetail}</dd></div>
      ) : null}
    </dl>
  );
}

function statusLabel(value: DesktopNotificationStatus): string {
  return ({ pending: "等待中", running: "进行中", succeeded: "已完成", failed: "失败", cancelled: "已取消" })[value];
}

function categoryLabel(value: DesktopNotificationCategory): string {
  return ({ task: "后台任务", file_operation: "文件操作", error: "错误", thumbnail_cache: "缩略图缓存" })[value];
}

function originLabel(value: DesktopNotificationOperation["origin"]): string {
  return ({ user: "用户操作", background: "后台", recovery: "恢复", system: "系统" })[value];
}

function kindLabel(value: string): string {
  return value.replaceAll("_", " ");
}

function formatDateTime(value: number): string {
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit"
  }).format(value);
}
