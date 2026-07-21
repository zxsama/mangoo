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
  DesktopNotificationHistoryQuery,
  DesktopNotificationOperation,
  DesktopNotificationStatus
} from "@megle/notification-contract";
import type { NotificationHistoryState } from "./useNotifications";

const PAGE_SIZE = 100;
const MAX_CLIENT_FILTER_PAGES = 50;

type NotificationAction = (
  libraryId: string,
  operationId: string
) => Promise<{ ok: boolean; message: string | null }>;

interface NotificationHistoryViewProps {
  cancel: NotificationAction;
  history: NotificationHistoryState;
  loadHistory(query: DesktopNotificationHistoryQuery): Promise<NotificationHistoryState>;
  loadMoreHistory(): Promise<NotificationHistoryState>;
  retry: NotificationAction;
}

export function NotificationHistoryView({
  cancel,
  history,
  loadHistory,
  loadMoreHistory,
  retry
}: NotificationHistoryViewProps) {
  const [search, setSearch] = useState("");
  const deferredSearch = useDeferredValue(search.trim());
  const [libraryId, setLibraryId] = useState("");
  const [status, setStatus] = useState<DesktopNotificationStatus | "">("");
  const [category, setCategory] = useState<DesktopNotificationCategory | "">("");
  const [source, setSource] = useState<DesktopNotificationOperation["origin"] | "">("");
  const [kind, setKind] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [actionState, setActionState] = useState<{
    key: string;
    action: "cancel" | "retry";
    error: string | null;
  } | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  const query = useMemo<DesktopNotificationHistoryQuery>(
    () => ({
      ...(libraryId ? { libraryId } : {}),
      ...(status ? { statuses: [status] } : {}),
      ...(category ? { categories: [category] } : {}),
      ...(deferredSearch ? { search: deferredSearch } : {}),
      limit: PAGE_SIZE
    }),
    [category, deferredSearch, libraryId, status]
  );
  const queryRef = useRef<DesktopNotificationHistoryQuery>(query);

  const loadCurrentQuery = useCallback(
    async (nextQuery: DesktopNotificationHistoryQuery) => {
      let next = await loadHistory(nextQuery);
      if (!source && !kind) return next;
      let pageCount = 0;
      while (next.nextOffset !== null && pageCount < MAX_CLIENT_FILTER_PAGES) {
        next = await loadMoreHistory();
        pageCount += 1;
      }
      return next;
    },
    [kind, loadHistory, loadMoreHistory, source]
  );

  useEffect(() => {
    queryRef.current = query;
    void loadCurrentQuery(query);
  }, [loadCurrentQuery, query]);

  const libraries = useMemo(() => {
    const values = new Map<string, string>();
    for (const item of history.items) {
      values.set(item.libraryId, item.libraryName || item.libraryId);
    }
    return [...values.entries()].sort((left, right) =>
      left[1].localeCompare(right[1], "zh-CN")
    );
  }, [history.items]);

  const kinds = useMemo(
    () => [...new Set(history.items.map((item) => item.kind))].sort(),
    [history.items]
  );

  const visibleItems = useMemo(
    () =>
      history.items.filter(
        (item) => (!source || item.origin === source) && (!kind || item.kind === kind)
      ),
    [history.items, kind, source]
  );

  const virtualizer = useVirtualizer({
    count: visibleItems.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 82,
    overscan: 7,
    getItemKey: (index) => {
      const item = visibleItems[index];
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
    const key = `${operation.libraryId}:${operation.operationId}`;
    setActionState({ key, action, error: null });
    const result = await (action === "cancel" ? cancel : retry)(
      operation.libraryId,
      operation.operationId
    );
    if (!result.ok) {
      setActionState({ key, action, error: result.message || "操作未完成" });
      return;
    }
    setActionState(null);
    await loadCurrentQuery(queryRef.current);
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
            onClick={() => void loadCurrentQuery(query)}
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
          {libraries.map(([id, name]) => <option key={id} value={id}>{name}</option>)}
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
          {kinds.map((value) => <option key={value} value={value}>{kindLabel(value)}</option>)}
        </FilterSelect>
        <button className="notification-text-action" onClick={resetFilters} type="button">
          重置
        </button>
      </div>

      {history.error ? <p className="notification-inline-error" role="alert">{history.error}</p> : null}

      <div className="notification-history-list" ref={scrollRef}>
        {visibleItems.length === 0 && !history.loading ? (
          <div className="notification-list-empty">没有符合当前筛选条件的通知。</div>
        ) : (
          <div
            className="notification-virtual-space"
            style={{ height: virtualizer.getTotalSize() }}
          >
            {virtualizer.getVirtualItems().map((virtualRow) => {
              const operation = visibleItems[virtualRow.index];
              const key = `${operation.libraryId}:${operation.operationId}`;
              const isExpanded = expanded.has(key);
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
                    data-notification-operation={`${operation.libraryId}:${operation.operationId}`}
                  >
                    <button
                      aria-expanded={isExpanded}
                      aria-label={`${isExpanded ? "收起" : "展开"}${operation.title}`}
                      className="notification-history-disclosure"
                      onClick={() => toggleExpanded(key)}
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
                      {actionState?.key === key && actionState.error ? (
                        <p className="notification-row-action-error" role="alert">{actionState.error}</p>
                      ) : null}
                    </div>
                    <div className="notification-history-actions">
                      {operation.cancellable ? (
                        <button
                          aria-label={`取消 ${operation.title}`}
                          className="notification-row-action"
                          disabled={actionState?.key === key}
                          onClick={() => void runAction("cancel", operation)}
                          type="button"
                        >
                          <Square aria-hidden="true" size={13} />
                          {actionState?.key === key && actionState.action === "cancel" ? "取消中…" : "取消"}
                        </button>
                      ) : null}
                      {operation.retryable ? (
                        <button
                          aria-label={`重试 ${operation.title}`}
                          className="notification-row-action"
                          disabled={actionState?.key === key}
                          onClick={() => void runAction("retry", operation)}
                          type="button"
                        >
                          <RotateCcw aria-hidden="true" size={13} />
                          {actionState?.key === key && actionState.action === "retry" ? "重试中…" : "重试"}
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
          {source || kind
            ? history.nextOffset === null
              ? "，来源和类型筛选已应用于当前上限内记录"
              : "，来源和类型筛选正在分页加载"
            : ""}
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
