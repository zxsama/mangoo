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
  DesktopProcessLogSource,
  MediaFocusRequest,
  ThumbnailFailureSnapshot,
  ThumbnailFailureDetail
} from "@megle/notification-contract";
import { copyText, getDesktopBridge } from "../../core/desktop";
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
  createProcessLogQueryToken,
  buildProcessLogRows,
  thumbnailFailureTruncationMessage,
  type ProcessLogRow
} from "./processLogModel";

const MAX_RENDERED_LOG_LINES = 5_000;
const LOG_PAGE_SIZE = 200;

interface ProcessLogViewProps {
  authoritativeFailures: readonly ThumbnailFailureDetail[];
  failureSnapshot?: ThumbnailFailureSnapshot | null;
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
  initialFilter?: DesktopProcessLogFilter;
}

export function ProcessLogView({
  authoritativeFailures,
  failureSnapshot,
  clearLogs,
  exportLogs,
  getLogPage,
  initialFilter,
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
      ...(initialFilter?.eventKind ? { eventKind: initialFilter.eventKind } : {}),
      ...(initialFilter?.libraryId ? { libraryId: initialFilter.libraryId } : {}),
      ...(source ? { source } : {}),
      ...(level ? { level } : {}),
      ...(deferredSearch ? { search: deferredSearch } : {}),
      limit: LOG_PAGE_SIZE
    }),
    [deferredSearch, initialFilter?.eventKind, initialFilter?.libraryId, level, source]
  );
  const filteredAuthoritativeFailures = useMemo(() => {
    if (initialFilter?.eventKind !== "thumbnail_failure") return [];
    if (source && source !== "core") return [];
    if (level && level !== "error") return [];
    const normalizedSearch = deferredSearch.toLocaleLowerCase();
    return authoritativeFailures.filter((detail) =>
      (!initialFilter.libraryId || detail.libraryId === initialFilter.libraryId) &&
      (!normalizedSearch || `${detail.relativePath} ${detail.failureKind} ${detail.error}`
        .toLocaleLowerCase()
        .includes(normalizedSearch))
    );
  }, [authoritativeFailures, deferredSearch, initialFilter?.eventKind, initialFilter?.libraryId, level, source]);
  const rows = useMemo(
    () => buildProcessLogRows(lines, filteredAuthoritativeFailures),
    [filteredAuthoritativeFailures, lines]
  );
  const failureTruncationMessage = thumbnailFailureTruncationMessage(failureSnapshot);
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
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 25,
    overscan: 12,
    getItemKey: (index) => processLogRowKey(rows[index], index)
  });

  useEffect(() => {
    if (!follow || paused || rows.length === 0) return;
    virtualizer.scrollToIndex(rows.length - 1, { align: "end" });
  }, [follow, paused, rows.length, virtualizer]);

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
      .map((item) => rows[item.index])
      .filter((row): row is ProcessLogRow => Boolean(row))
      .map(formatProcessLogRow)
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

  const handleFocusMedia = async (row: ProcessLogRow) => {
    const request = mediaFocusRequest(row);
    if (!request) return;
    const focusMedia = getDesktopBridge()?.focusMedia;
    if (!focusMedia) {
      setActionMessage("当前环境无法定位媒体");
      return;
    }
    setActionMessage((await focusMedia(request)) ? "已在主窗口定位媒体" : "媒体定位失败");
  };

  return (
    <div className="notification-view notification-process-log-view">
      <header className="notification-view-heading notification-log-heading">
        <div>
          <span className="notification-view-eyebrow">
            {initialFilter?.eventKind === "thumbnail_failure" ? "FAILURE DIAGNOSTICS" : "CURRENT SESSION"}
          </span>
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
      {initialFilter?.eventKind === "thumbnail_failure" ? (
        <p className="notification-log-filter-state">
          缩略图失败 · {initialFilter.libraryId}
          {failureTruncationMessage ? ` · ${failureTruncationMessage}` : ""}
        </p>
      ) : null}

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
        {rows.length === 0 && !loading ? (
          <div className="notification-list-empty">当前筛选条件下没有日志。</div>
        ) : (
          <div className="notification-virtual-space" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((virtualRow) => {
              const row = rows[virtualRow.index];
              if (!row) return null;
              const request = mediaFocusRequest(row);
              const content = processLogRowContent(row);
              const common = {
                className: `notification-log-line level-${content.level}${request ? " actionable" : ""}`,
                "data-index": virtualRow.index,
                ref: virtualizer.measureElement,
                style: { transform: `translateY(${virtualRow.start}px)` } as CSSProperties
              };
              return request ? (
                <button
                  {...common}
                  key={virtualRow.key}
                  onClick={() => void handleFocusMedia(row)}
                  title={`在主窗口定位 ${content.path ?? "媒体"}`}
                  type="button"
                >
                  <LogRowContent content={content} />
                </button>
              ) : (
                <div {...common} key={virtualRow.key}>
                  <LogRowContent content={content} />
                </div>
              );
            })}
          </div>
        )}
      </div>
      <footer className="notification-log-footer">
        <span>{rows.length.toLocaleString()} 行 · 会话日志上限 {MAX_RENDERED_LOG_LINES.toLocaleString()}</span>
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

interface LogRowContentValue {
  sequence: string | number;
  timestamp: number;
  source: string;
  level: DesktopProcessLogLevel;
  message: string;
  metadata: string;
  path?: string;
}

function LogRowContent({ content }: { content: LogRowContentValue }) {
  return (
    <>
      <span className="notification-log-sequence">{content.sequence}</span>
      <time dateTime={new Date(content.timestamp).toISOString()}>{formatLogTime(content.timestamp)}</time>
      <span className="notification-log-source">{content.source}</span>
      <span className="notification-log-level">{content.level}</span>
      <span className="notification-log-message">{content.message}{content.metadata}</span>
    </>
  );
}

function processLogRowContent(row: ProcessLogRow): LogRowContentValue {
  if (row.kind === "log") {
    return {
      sequence: row.line.sequence,
      timestamp: row.line.timestamp,
      source: row.line.source,
      level: row.line.level,
      message: row.line.message,
      metadata: formatMetadata(row.line),
      path: typeof row.line.metadata?.relativePath === "string"
        ? row.line.metadata.relativePath
        : undefined
    };
  }
  return {
    sequence: "saved",
    timestamp: row.detail.updatedAt,
    source: "core",
    level: "error",
    message: `${row.detail.relativePath} · ${row.detail.error}`,
    metadata: ` ${safeJson({ failureKind: row.detail.failureKind })}`,
    path: row.detail.relativePath
  };
}

function processLogRowKey(row: ProcessLogRow | undefined, index: number): string | number {
  if (!row) return index;
  return row.kind === "log"
    ? `log:${row.line.sequence}`
    : `failure:${row.detail.libraryId}:${row.detail.fileId}`;
}

function mediaFocusRequest(row: ProcessLogRow): MediaFocusRequest | null {
  if (row.kind === "failure") {
    return {
      libraryId: row.detail.libraryId,
      fileId: row.detail.fileId,
      folderId: row.detail.folderId,
      rootId: row.detail.rootId
    };
  }
  const metadata = row.line.metadata;
  if (metadata?.eventKind !== "thumbnail_failure") return null;
  if (
    typeof metadata.libraryId !== "string" ||
    !positiveSafeInteger(metadata.fileId) ||
    !positiveSafeInteger(metadata.folderId) ||
    !positiveSafeInteger(metadata.rootId)
  ) {
    return null;
  }
  return {
    libraryId: metadata.libraryId,
    fileId: metadata.fileId,
    folderId: metadata.folderId,
    rootId: metadata.rootId
  };
}

function positiveSafeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

function formatProcessLogRow(row: ProcessLogRow): string {
  if (row.kind === "log") return formatLogLine(row.line);
  return `${new Date(row.detail.updatedAt).toISOString()} [core/error] ${row.detail.relativePath} ${row.detail.error}`;
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
