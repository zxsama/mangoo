import {
  Clipboard,
  Download,
  Pause,
  Play,
  Radio,
  Trash2
} from "lucide-react";
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
  DesktopProcessLogBatch,
  DesktopProcessLogExportResult,
  DesktopProcessLogFilter,
  DesktopProcessLogLevel,
  DesktopProcessLogLine,
  DesktopProcessLogPage,
  DesktopProcessLogSource
} from "@megle/notification-contract";
import { copyText } from "../../core/desktop";
import type {
  NotificationRequestResult,
  OpenNotificationLogSubscription
} from "./useNotifications";
import { processLogGapSequence } from "./notificationModel";
import {
  appendPausedProcessLogBatch,
  coalescePausedProcessLogBuffer,
  createPausedProcessLogBuffer,
  createProcessLogPageRequestController,
  createProcessLogQueryToken
} from "./processLogModel";

const MAX_RENDERED_LOG_LINES = 5_000;
const LOG_PAGE_SIZE = 200;

interface ProcessLogViewProps {
  clearLogs(): Promise<NotificationRequestResult<boolean>>;
  exportLogs(
    query: DesktopProcessLogFilter
  ): Promise<NotificationRequestResult<DesktopProcessLogExportResult>>;
  getLogPage(
    query: DesktopProcessLogFilter
  ): Promise<NotificationRequestResult<DesktopProcessLogPage>>;
  openLogSubscription(
    query: DesktopProcessLogFilter,
    listener: (batch: DesktopProcessLogBatch) => void
  ): Promise<NotificationRequestResult<OpenNotificationLogSubscription>>;
}

export function ProcessLogView({
  clearLogs,
  exportLogs,
  getLogPage,
  openLogSubscription
}: ProcessLogViewProps) {
  const [source, setSource] = useState<DesktopProcessLogSource | "">("");
  const [level, setLevel] = useState<DesktopProcessLogLevel | "">("");
  const [search, setSearch] = useState("");
  const deferredSearch = useDeferredValue(search.trim());
  const [lines, setLines] = useState<DesktopProcessLogLine[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [gapSequence, setGapSequence] = useState<number | null>(null);
  const [paused, setPaused] = useState(false);
  const [follow, setFollow] = useState(true);
  const [queuedLineCount, setQueuedLineCount] = useState(0);
  const [hasOlder, setHasOlder] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [actionMessage, setActionMessage] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const pausedRef = useRef(false);
  const pausedBufferRef = useRef(createPausedProcessLogBuffer());

  const query = useMemo<DesktopProcessLogFilter>(
    () => ({
      ...(source ? { source } : {}),
      ...(level ? { level } : {}),
      ...(deferredSearch ? { search: deferredSearch } : {}),
      limit: LOG_PAGE_SIZE
    }),
    [deferredSearch, level, source]
  );
  const queryToken = useMemo(() => createProcessLogQueryToken(query), [query]);
  const [pageRequestController] = useState(() =>
    createProcessLogPageRequestController(queryToken)
  );

  const deliverBatch = useCallback((batch: DesktopProcessLogBatch) => {
    const gap = processLogGapSequence(batch);
    if (gap !== null) setGapSequence(gap);
    if (batch.lines.length === 0) return;
    setLines((current) => mergeLogLines(current, batch.lines));
  }, []);

  const receiveBatch = useCallback((batch: DesktopProcessLogBatch) => {
    if (!pausedRef.current) {
      deliverBatch(batch);
      return;
    }
    const next = appendPausedProcessLogBatch(pausedBufferRef.current, batch);
    pausedBufferRef.current = next;
    setQueuedLineCount(next.lineCount);
  }, [deliverBatch]);

  useEffect(() => {
    let cancelled = false;
    let unsubscribe: (() => void) | null = null;
    pageRequestController.commitQuery(queryToken);
    setLoading(true);
    setError(null);
    setGapSequence(null);
    setLines([]);
    pausedBufferRef.current = createPausedProcessLogBuffer();
    setQueuedLineCount(0);
    setHasOlder(false);
    setLoadingOlder(false);

    void openLogSubscription(query, receiveBatch).then((result) => {
      if (!result.ok) {
        if (!cancelled) {
          setLoading(false);
          setError(result.error);
        }
        return;
      }
      unsubscribe = result.value.unsubscribe;
      if (cancelled) {
        result.value.unsubscribe();
        return;
      }
      const snapshot = [...result.value.snapshot.snapshot]
        .sort((left, right) => left.sequence - right.sequence)
        .slice(-MAX_RENDERED_LOG_LINES);
      setLines(snapshot);
      setHasOlder(snapshot.length >= LOG_PAGE_SIZE);
      setLoading(false);
    });

    return () => {
      cancelled = true;
      pageRequestController.invalidate();
      unsubscribe?.();
    };
  }, [openLogSubscription, pageRequestController, query, queryToken, receiveBatch]);

  useEffect(() => {
    pausedRef.current = paused;
  }, [paused]);

  const virtualizer = useVirtualizer({
    count: lines.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 25,
    overscan: 12,
    getItemKey: (index) => lines[index]?.sequence ?? index
  });

  useEffect(() => {
    if (!follow || paused || lines.length === 0) return;
    virtualizer.scrollToIndex(lines.length - 1, { align: "end" });
  }, [follow, lines.length, paused, virtualizer]);

  const togglePause = () => {
    if (!paused) {
      pausedRef.current = true;
      setPaused(true);
      return;
    }
    pausedRef.current = false;
    setPaused(false);
    const merged = coalescePausedProcessLogBuffer(pausedBufferRef.current);
    pausedBufferRef.current = createPausedProcessLogBuffer();
    setQueuedLineCount(0);
    if (merged.droppedBeforeSequence !== null) {
      setGapSequence(merged.droppedBeforeSequence);
    }
    if (merged.lines.length > 0) {
      setLines((current) => mergeLogLines(current, merged.lines));
    }
  };

  const handleCopy = async () => {
    const selection = window.getSelection();
    const selected =
      selection && selection.anchorNode && scrollRef.current?.contains(selection.anchorNode)
        ? selection.toString().trim()
        : "";
    const visibleText = virtualizer
      .getVirtualItems()
      .map((item) => lines[item.index])
      .filter((line): line is DesktopProcessLogLine => Boolean(line))
      .map(formatLogLine)
      .join("\n");
    const text = selected || visibleText;
    if (!text) {
      setActionMessage("当前没有可复制的日志");
      return;
    }
    setActionMessage((await copyText(text)) ? "已复制可见日志" : "复制失败");
  };

  const handleClear = async () => {
    setActionMessage(null);
    const result = await clearLogs();
    if (!result.ok) {
      setActionMessage(result.error);
      return;
    }
    setLines([]);
    pausedBufferRef.current = createPausedProcessLogBuffer();
    setQueuedLineCount(0);
    setGapSequence(null);
    setHasOlder(false);
    pageRequestController.invalidate();
    setLoadingOlder(false);
    setActionMessage("已清空当前会话日志");
  };

  const handleExport = async () => {
    setActionMessage(null);
    const result = await exportLogs(query);
    if (!result.ok) {
      setActionMessage(result.error);
      return;
    }
    if (result.value.canceled) {
      setActionMessage("已取消导出");
      return;
    }
    if (!result.value.saved) {
      setActionMessage(result.value.error || "日志导出失败");
      return;
    }
    setActionMessage(`已导出 ${result.value.lineCount.toLocaleString()} 行日志`);
  };

  const handleLoadOlder = async () => {
    const oldestSequence = lines[0]?.sequence;
    if (oldestSequence == null) return;
    const request = pageRequestController.begin(queryToken);
    if (!request) return;
    setLoadingOlder(true);
    try {
      const result = await getLogPage({
        ...query,
        beforeSequence: oldestSequence,
        limit: LOG_PAGE_SIZE
      });
      if (!pageRequestController.isCurrent(request)) return;
      if (!result.ok) {
        setActionMessage(result.error);
        return;
      }
      setLines((current) => mergeLogLines(current, result.value.lines));
      setHasOlder(result.value.hasMore && result.value.nextBeforeSequence !== null);
    } catch (error) {
      if (pageRequestController.isCurrent(request)) {
        setActionMessage(errorMessage(error));
      }
    } finally {
      if (pageRequestController.finish(request)) setLoadingOlder(false);
    }
  };

  return (
    <div className="notification-view notification-process-log-view">
      <header className="notification-view-heading notification-log-heading">
        <div>
          <span className="notification-view-eyebrow">CURRENT SESSION</span>
          <h1>进程日志</h1>
        </div>
        <div className="notification-log-live-state">
          <Radio aria-hidden="true" size={13} />
          {paused ? `已暂停${queuedLineCount ? ` · ${queuedLineCount} 条待显示` : ""}` : "实时"}
        </div>
      </header>

      <div aria-label="进程日志筛选和操作" className="notification-log-toolbar">
        <label className="notification-filter-search notification-log-search">
          <span className="sr-only">搜索进程日志</span>
          <input
            onChange={(event) => setSearch(event.currentTarget.value)}
            placeholder="搜索本次运行日志"
            type="search"
            value={search}
          />
        </label>
        <label className="notification-filter-select compact">
          <span>来源</span>
          <select onChange={(event) => setSource(event.currentTarget.value as DesktopProcessLogSource | "")} value={source}>
            <option value="">全部</option>
            <option value="main">Main</option>
            <option value="core">Core</option>
            <option value="renderer">Renderer</option>
          </select>
        </label>
        <label className="notification-filter-select compact">
          <span>级别</span>
          <select onChange={(event) => setLevel(event.currentTarget.value as DesktopProcessLogLevel | "")} value={level}>
            <option value="">全部</option>
            <option value="trace">Trace</option>
            <option value="debug">Debug</option>
            <option value="info">Info</option>
            <option value="warn">Warn</option>
            <option value="error">Error</option>
          </select>
        </label>
        <div className="notification-log-actions">
          <button aria-label={paused ? "继续日志" : "暂停日志"} className="notification-toolbar-action" onClick={togglePause} type="button">
            {paused ? <Play aria-hidden="true" size={14} /> : <Pause aria-hidden="true" size={14} />}
            {paused ? "继续" : "暂停"}
          </button>
          <button aria-label="自动跟随最新日志" aria-pressed={follow} className={`notification-toolbar-action${follow ? " active" : ""}`} onClick={() => setFollow((value) => !value)} type="button">
            跟随
          </button>
          <button aria-label="复制选中或可见日志" className="notification-icon-action" onClick={() => void handleCopy()} type="button">
            <Clipboard aria-hidden="true" size={14} />
          </button>
          <button aria-label="导出当前日志" className="notification-icon-action" onClick={() => void handleExport()} type="button">
            <Download aria-hidden="true" size={14} />
          </button>
          <button aria-label="清空当前会话日志" className="notification-icon-action danger" onClick={() => void handleClear()} type="button">
            <Trash2 aria-hidden="true" size={14} />
          </button>
        </div>
      </div>

      {gapSequence !== null ? (
        <p className="notification-log-gap" role="status">
          日志缓冲已覆盖较早内容，序列 {gapSequence.toLocaleString()} 之前或附近可能存在缺口。
        </p>
      ) : null}
      {error ? <p className="notification-inline-error" role="alert">{error}</p> : null}

      <div
        aria-busy={loading}
        aria-label="进程日志内容"
        className="notification-log-list"
        onScroll={(event) => {
          if (!follow) return;
          const element = event.currentTarget;
          if (element.scrollHeight - element.scrollTop - element.clientHeight > 32) setFollow(false);
        }}
        ref={scrollRef}
        role="log"
      >
        {lines.length === 0 && !loading ? (
          <div className="notification-list-empty">当前筛选条件下没有日志。</div>
        ) : (
          <div className="notification-virtual-space" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((virtualRow) => {
              const line = lines[virtualRow.index];
              return (
                <div
                  className={`notification-log-line level-${line.level}`}
                  data-index={virtualRow.index}
                  key={virtualRow.key}
                  ref={virtualizer.measureElement}
                  style={{ transform: `translateY(${virtualRow.start}px)` } as CSSProperties}
                >
                  <span className="notification-log-sequence">{line.sequence}</span>
                  <time dateTime={new Date(line.timestamp).toISOString()}>{formatLogTime(line.timestamp)}</time>
                  <span className="notification-log-source">{line.source}</span>
                  <span className="notification-log-level">{line.level}</span>
                  <span className="notification-log-message">{line.message}{formatMetadata(line)}</span>
                </div>
              );
            })}
          </div>
        )}
      </div>
      <footer className="notification-log-footer">
        <span>{lines.length.toLocaleString()} / {MAX_RENDERED_LOG_LINES.toLocaleString()} 行上限</span>
        {hasOlder ? (
          <button
            className="notification-text-action"
            disabled={loadingOlder}
            onClick={() => void handleLoadOlder()}
            type="button"
          >
            {loadingOlder ? "加载中…" : "加载更早日志"}
          </button>
        ) : null}
        <span aria-live="polite">{actionMessage}</span>
      </footer>
    </div>
  );
}

function mergeLogLines(
  current: readonly DesktopProcessLogLine[],
  incoming: readonly DesktopProcessLogLine[]
): DesktopProcessLogLine[] {
  const bySequence = new Map(current.map((line) => [line.sequence, line]));
  for (const line of incoming) bySequence.set(line.sequence, line);
  return [...bySequence.values()]
    .sort((left, right) => left.sequence - right.sequence)
    .slice(-MAX_RENDERED_LOG_LINES);
}

function formatLogLine(line: DesktopProcessLogLine): string {
  const metadata = line.metadata ? ` ${safeJson(line.metadata)}` : "";
  return `${new Date(line.timestamp).toISOString()} #${line.sequence} [${line.source}/${line.level}] ${line.message}${metadata}`;
}

function formatMetadata(line: DesktopProcessLogLine): string {
  return line.metadata ? ` ${safeJson(line.metadata)}` : "";
}

function safeJson(value: unknown): string {
  try {
    return JSON.stringify(value);
  } catch {
    return "[metadata unavailable]";
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function formatLogTime(value: number): string {
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    fractionalSecondDigits: 3
  }).format(value);
}
