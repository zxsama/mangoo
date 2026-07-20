import {
  AlertTriangle,
  CheckCircle2,
  CircleDashed,
  Image,
  ListTodo,
  XCircle
} from "lucide-react";
import type { CSSProperties, ReactNode } from "react";
import { LiquidGlassButton, LiquidGlassSurface } from "../../design/liquid-glass";
import type {
  CompactNotificationModel,
  NotificationOperationViewModel
} from "./notificationModel";

interface NotificationPopoverProps {
  model: CompactNotificationModel;
  onOpenCenter: () => void;
}

export function NotificationPopover({
  model,
  onOpenCenter
}: NotificationPopoverProps) {
  const hasContent =
    model.active.length > 0 || model.thumbnail.summaries.length > 0 || model.results.length > 0;

  return (
    <LiquidGlassSurface
      as="section"
      aria-label="通知状态"
      className="floating-popover notification-popover popup-surface"
      data-compact-popover-root="notifications"
      interactive
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
            <div className="notification-thumbnail-summary">
              <div className="notification-row-heading">
                <strong>
                  {model.thumbnail.libraryCount > 1
                    ? `${model.thumbnail.libraryCount} 个资源库`
                    : model.thumbnail.summaries[0]?.libraryName || "当前资源库"}
                </strong>
                <span>
                  {model.thumbnail.completionPercent == null
                    ? "统计中"
                    : `${model.thumbnail.completionPercent}%`}
                </span>
              </div>
              <div
                aria-label="缩略图缓存进度"
                aria-valuemax={100}
                aria-valuemin={0}
                aria-valuenow={model.thumbnail.completionPercent ?? undefined}
                className="notification-progress-track"
                role="progressbar"
              >
                {model.thumbnail.completionPercent == null ? (
                  <span className="notification-progress-indeterminate" />
                ) : (
                  <span
                    className="notification-progress-fill"
                    style={progressStyle(model.thumbnail.completionPercent)}
                  />
                )}
              </div>
              <div className="notification-thumbnail-metrics">
                <span>
                  {model.thumbnail.cachedCount.toLocaleString()} /{" "}
                  {model.thumbnail.totalCandidateCount.toLocaleString()} 已缓存
                </span>
                <span>
                  {model.thumbnail.pendingCount.toLocaleString()} 待处理
                  {model.thumbnail.failedCount > 0
                    ? ` · ${model.thumbnail.failedCount.toLocaleString()} 失败`
                    : ""}
                </span>
                {model.thumbnail.totalCacheBytes == null ? null : (
                  <span>{formatBytes(model.thumbnail.totalCacheBytes)}</span>
                )}
              </div>
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
                    <span>{resultSummary(operation)}</span>
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
          onClick={onOpenCenter}
          tone="control"
          type="button"
        >
          打开完整通知中心
        </LiquidGlassButton>
      </footer>
    </LiquidGlassSurface>
  );
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
  return (
    <div className="notification-row-heading">
      <div>
        <strong>{operation.title}</strong>
        <span>{operation.libraryLabel}</span>
      </div>
      <span>{operation.progress.determinate ? `${operation.progress.percent}%` : operation.phase}</span>
    </div>
  );
}

function OperationProgress({ operation }: { operation: NotificationOperationViewModel }) {
  const progress = operation.progress;
  return (
    <>
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
      <span className="notification-progress-label">{progress.label}</span>
    </>
  );
}

function progressStyle(percent: number): CSSProperties {
  return { "--notification-progress": `${percent}%` } as CSSProperties;
}

function resultIcon(status: NotificationOperationViewModel["status"]) {
  if (status === "failed") return <AlertTriangle size={14} />;
  if (status === "cancelled") return <XCircle size={14} />;
  return <CheckCircle2 size={14} />;
}

function resultSummary(operation: NotificationOperationViewModel): string {
  const detail = operation.errorDetail?.trim() || operation.summary?.trim();
  if (detail) return detail;
  if (operation.status === "failed") return "操作失败";
  if (operation.status === "cancelled") return "操作已取消";
  return `${operation.libraryLabel} · 已完成`;
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
