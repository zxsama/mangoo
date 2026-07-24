import {
  AlertTriangle,
  CheckCircle2,
  CircleDashed,
  Image,
  ListTodo,
  XCircle
} from "lucide-react";
import {
  useLayoutEffect,
  useState,
  type CSSProperties,
  type ReactNode
} from "react";
import { createPortal } from "react-dom";
import type {
  DesktopThumbnailCacheSummary,
  NotificationWindowIntent
} from "@megle/notification-contract";
import { LiquidGlassButton, LiquidGlassSurface } from "../../design/liquid-glass";
import type {
  CompactNotificationModel,
  NotificationOperationViewModel
} from "./notificationModel";
import {
  activateThumbnailFailureLogs,
  thumbnailFailureDetailsUnavailableMessage
} from "./notificationModel";
import { observeNotificationPopoverMeasurements } from "./notificationPopoverMeasurement";
import { resolveNotificationPopoverPosition } from "./notificationPopoverPosition";
import {
  activeOperationPresentation,
  resultOperationPresentation
} from "./notificationPopoverPresentation";

interface NotificationPopoverProps {
  anchor: HTMLElement;
  model: CompactNotificationModel;
  onOpenCenter: (intent?: NotificationWindowIntent) => void;
}

export function NotificationPopover({
  anchor,
  model,
  onOpenCenter
}: NotificationPopoverProps) {
  const [popoverElement, setPopoverElement] = useState<HTMLElement | null>(null);
  const position = useNotificationPopoverPosition(anchor, popoverElement);
  const hasContent =
    model.active.length > 0 || model.thumbnail.summaries.length > 0 || model.results.length > 0;

  return createPortal(
    <LiquidGlassSurface
      as="section"
      aria-label="通知状态"
      className="floating-popover notification-popover popup-surface"
      data-compact-popover-root="notifications"
      interactive
      ref={setPopoverElement}
      style={
        position
          ? {
              left: position.left,
              top: position.top,
              maxHeight: position.maxHeight,
              visibility: "visible"
            }
          : { left: 0, top: 0, visibility: "hidden" }
      }
      tone="elevated"
    >
      <header className="notification-popover-header">
        <div>
          <span className="notification-popover-eyebrow">状态中心</span>
          <h2>通知</h2>
        </div>
        <span
          className={
            model.degraded
              ? "notification-health-state notification-popover-degraded"
              : "notification-health-state"
          }
          data-notification-health={model.degraded ? "degraded" : "healthy"}
          title={model.healthDetail ?? undefined}
        >
          {model.degraded ? "同步受限" : "运行正常"}
        </span>
      </header>

      <div className="notification-popover-body" data-overlay-scroll-container>
        {model.degraded ? (
          <p className="notification-health-detail">
            {model.healthDetail?.trim() || "通知同步暂时不可用，已显示最近保存的状态。"}
          </p>
        ) : null}

        {model.active.length > 0 ? (
          <NotificationSection
            icon={<ListTodo aria-hidden="true" size={14} />}
            label="进行中"
          >
            <ul className="notification-active-list">
              {model.active.map((operation) => (
                <li
                  className="notification-operation-row"
                  data-notification-active-operation={operation.operationId}
                  key={`${operation.libraryId}:${operation.operationId}`}
                >
                  <OperationHeading operation={operation} />
                  <OperationProgress operation={operation} />
                </li>
              ))}
            </ul>
          </NotificationSection>
        ) : null}

        {model.thumbnail.summaries.length > 0 ? (
          <NotificationSection
            icon={<Image aria-hidden="true" size={14} />}
            label="缩略图缓存"
          >
            <div
              className="notification-thumbnail-summary-list"
              data-notification-thumbnail-library-count={model.thumbnail.libraryCount}
            >
              {model.thumbnail.summaries.map((summary) => (
                <ThumbnailSummaryRow
                  key={summary.libraryId}
                  onOpenCenter={onOpenCenter}
                  summary={summary}
                />
              ))}
            </div>
          </NotificationSection>
        ) : null}

        {model.results.length > 0 ? (
          <NotificationSection
            icon={<CheckCircle2 aria-hidden="true" size={14} />}
            label="最近结果"
          >
            <ul className="notification-result-list">
              {model.results.map((operation) => (
                <li
                  className={`notification-result-row notification-result-${operation.status}`}
                  data-notification-result={operation.operationId}
                  key={`${operation.libraryId}:${operation.operationId}`}
                >
                  <span className="notification-result-icon" aria-hidden="true">
                    {resultIcon(operation.status)}
                  </span>
                  <div className="notification-result-copy">
                    <strong>{operation.title}</strong>
                    <ResultDescription operation={operation} />
                  </div>
                  <time dateTime={new Date(operation.updatedAt).toISOString()}>
                    {formatResultTime(operation.updatedAt)}
                  </time>
                </li>
              ))}
            </ul>
          </NotificationSection>
        ) : null}

        {!hasContent ? (
          <div className="notification-popover-empty">
            <CircleDashed aria-hidden="true" size={20} />
            <div>
              <strong>{model.degraded ? "暂无可用状态" : "当前没有进行中的任务"}</strong>
              <span>{model.degraded ? "等待通知同步恢复" : "后台状态正常"}</span>
            </div>
          </div>
        ) : null}
      </div>

      <footer className="notification-popover-footer">
        <LiquidGlassButton
          className="notification-open-center"
          onClick={() => onOpenCenter()}
          tone="control"
          type="button"
        >
          打开完整通知中心
        </LiquidGlassButton>
      </footer>
    </LiquidGlassSurface>,
    document.body
  );
}

function useNotificationPopoverPosition(anchor: HTMLElement, popover: HTMLElement | null) {
  const [position, setPosition] = useState<ReturnType<
    typeof resolveNotificationPopoverPosition
  > | null>(null);

  useLayoutEffect(() => {
    setPosition(null);
    if (!popover) return;

    return observeNotificationPopoverMeasurements({
      anchor,
      onMeasure: (measurement) => {
        const nextPosition = resolveNotificationPopoverPosition({
          anchor: measurement.anchor,
          popover: measurement.popover,
          viewport: measurement.viewport,
          gap: 4,
          margin: 8
        });
        setPosition((current) =>
          current && samePopoverPosition(current, nextPosition) ? current : nextPosition
        );
      },
      popover
    });
  }, [anchor, popover]);

  return position;
}

function samePopoverPosition(
  left: ReturnType<typeof resolveNotificationPopoverPosition>,
  right: ReturnType<typeof resolveNotificationPopoverPosition>
): boolean {
  return left.left === right.left && left.top === right.top && left.maxHeight === right.maxHeight;
}

function NotificationSection({
  children,
  icon,
  label
}: {
  children: ReactNode;
  icon: ReactNode;
  label: string;
}) {
  return (
    <section className="notification-section">
      <h3>
        {icon}
        <span>{label}</span>
      </h3>
      {children}
    </section>
  );
}

function OperationHeading({ operation }: { operation: NotificationOperationViewModel }) {
  const presentation = activeOperationPresentation(operation);
  return (
    <div className="notification-row-heading">
      <div>
        <strong>{operation.title}</strong>
        <span>{operation.libraryLabel}</span>
      </div>
      <span className="notification-operation-phase">{presentation.phase}</span>
    </div>
  );
}

function OperationProgress({ operation }: { operation: NotificationOperationViewModel }) {
  const progress = operation.progress;
  const presentation = activeOperationPresentation(operation);
  return (
    <>
      {operation.paths.length > 0 ? (
        <div className="notification-scan-paths compact">
          {operation.paths.slice(0, 3).map((path) => (
            <span key={path} title={path}>{path}</span>
          ))}
          {operation.paths.length > 3 ? <span>+{operation.paths.length - 3}</span> : null}
        </div>
      ) : null}
      <div
        aria-label={`${operation.title}进度`}
        aria-valuemax={progress.determinate ? 100 : undefined}
        aria-valuemin={progress.determinate ? 0 : undefined}
        aria-valuenow={progress.percent ?? undefined}
        aria-valuetext={progress.label}
        className="notification-progress-track"
        role="progressbar"
      >
        {progress.determinate && progress.percent != null ? (
          <span
            className="notification-progress-fill"
            style={progressStyle(progress.percent)}
          />
        ) : (
          <span className="notification-progress-indeterminate" />
        )}
      </div>
      <span className="notification-progress-meta">
        <span className="notification-progress-label">{presentation.progressLabel}</span>
        {presentation.percentLabel ? (
          <span className="notification-progress-percent">{presentation.percentLabel}</span>
        ) : null}
      </span>
    </>
  );
}

function ThumbnailSummaryRow({
  onOpenCenter,
  summary
}: {
  onOpenCenter: (intent?: NotificationWindowIntent) => void;
  summary: DesktopThumbnailCacheSummary;
}) {
  const unavailableMessage = thumbnailFailureDetailsUnavailableMessage(summary);
  const status = thumbnailStatusParts(summary).filter((part) => part.kind !== "failure");
  const libraryLabel = summary.libraryName || summary.libraryId;
  return (
    <div
      className="notification-thumbnail-summary"
      data-notification-thumbnail-library={summary.libraryId}
      data-notification-thumbnail-library-name={libraryLabel}
      data-notification-thumbnail-progress={summary.completionPercent ?? "indeterminate"}
    >
      <div className="notification-row-heading">
        <strong>{libraryLabel}</strong>
        <span className="notification-thumbnail-progress-percent">
          {summary.completionPercent == null ? "统计中" : `${summary.completionPercent}%`}
        </span>
      </div>
      <div
        aria-label={`${libraryLabel}缩略图缓存进度`}
        aria-valuemax={100}
        aria-valuemin={0}
        aria-valuenow={summary.completionPercent ?? undefined}
        className="notification-progress-track"
        role="progressbar"
      >
        {summary.completionPercent == null ? (
          <span className="notification-progress-indeterminate" />
        ) : (
          <span
            className="notification-progress-fill"
            style={progressStyle(summary.completionPercent)}
          />
        )}
      </div>
      <div className="notification-thumbnail-metrics">
        <span className="notification-thumbnail-cached-total">
          {summary.cachedCount.toLocaleString()} / {summary.totalCandidateCount.toLocaleString()} 已缓存
        </span>
        <span className="notification-thumbnail-status">
          {status.length > 0 ? status.map((part) => part.label).join(" · ") : "已同步"}
        </span>
        {summary.failedCount > 0 && !unavailableMessage ? (
          <button
            className="notification-thumbnail-failure-action"
            onClick={() => activateThumbnailFailureLogs(summary, onOpenCenter)}
            type="button"
          >
            {summary.failedCount.toLocaleString()} 失败
          </button>
        ) : summary.failedCount > 0 ? (
          <span title={unavailableMessage ?? undefined}>
            {summary.failedCount.toLocaleString()} 失败 · 详情不可用
          </span>
        ) : null}
        {summary.totalCacheBytes == null ? null : <span>{formatBytes(summary.totalCacheBytes)}</span>}
      </div>
    </div>
  );
}

function thumbnailStatusParts(summary: DesktopThumbnailCacheSummary): Array<{ kind: string; label: string }> {
  return [
    summary.pendingCount > 0 ? { kind: "pending", label: `${summary.pendingCount.toLocaleString()} 待处理` } : null,
    summary.failedCount > 0 ? { kind: "failure", label: `${summary.failedCount.toLocaleString()} 失败` } : null,
    summary.staleCount > 0 ? { kind: "stale", label: `${summary.staleCount.toLocaleString()} 过期` } : null,
    summary.missingCount > 0 ? { kind: "missing", label: `${summary.missingCount.toLocaleString()} 缺失` } : null,
    summary.activeBulkTaskCount > 0 ? { kind: "bulk", label: `${summary.activeBulkTaskCount.toLocaleString()} 批处理` } : null
  ].filter((value): value is { kind: string; label: string } => value !== null);
}

function progressStyle(percent: number): CSSProperties {
  return { "--notification-progress": `${percent}%` } as CSSProperties;
}

function resultIcon(status: NotificationOperationViewModel["status"]) {
  if (status === "failed") return <AlertTriangle size={14} />;
  if (status === "cancelled") return <XCircle size={14} />;
  return <CheckCircle2 size={14} />;
}

function ResultDescription({ operation }: { operation: NotificationOperationViewModel }) {
  const presentation = resultOperationPresentation(operation);
  return (
    <span className="notification-result-detail">
      <span className="notification-result-library">{presentation.libraryLabel}</span>
      <span aria-hidden="true"> · </span>
      {presentation.detail}
    </span>
  );
}

function formatResultTime(timestamp: number): string {
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit"
  }).format(timestamp);
}

function formatBytes(value: number): string {
  if (value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let amount = value;
  let unitIndex = 0;
  while (amount >= 1024 && unitIndex < units.length - 1) {
    amount /= 1024;
    unitIndex += 1;
  }
  const digits = amount >= 10 || unitIndex === 0 ? 0 : 1;
  return `${amount.toFixed(digits)} ${units[unitIndex]}`;
}
