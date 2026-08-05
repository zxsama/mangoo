import { CircleDashed, Image, ListTodo } from "lucide-react";
import { useRef, type CSSProperties, type ReactNode } from "react";
import type {
  DesktopThumbnailCacheSummary,
  NotificationWindowIntent
} from "@megle/notification-contract";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { AnchoredOverlay } from "../../design/overlays";
import type {
  CompactNotificationModel,
  NotificationOperationViewModel
} from "./notificationModel";
import { thumbnailStatusParts } from "./notificationModel";
import {
  activeOperationPresentation,
  compactNotificationPresentation
} from "./notificationPopoverPresentation";

interface NotificationPopoverProps {
  anchor: HTMLElement;
  autoFocus?: boolean;
  model: CompactNotificationModel;
  onClose: () => void;
  onOpenCenter: (intent?: NotificationWindowIntent) => void;
}

export function NotificationPopover({
  anchor,
  autoFocus = false,
  model,
  onClose,
  onOpenCenter
}: NotificationPopoverProps) {
  const openCenterButtonRef = useRef<HTMLButtonElement | null>(null);
  const presentation = compactNotificationPresentation(model);

  const activityContent = (
    <>
      {model.degraded ? (
        <p className="notification-health-detail">
          {model.healthDetail?.trim() || "通知同步暂时不可用，当前进度可能延迟。"}
        </p>
      ) : null}

      {presentation.failureCue ? (
        <p className="notification-failure-cue">
          {presentation.failureCue}
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
              <ThumbnailSummaryRow key={summary.libraryId} summary={summary} />
            ))}
          </div>
        </NotificationSection>
      ) : null}

      {presentation.idleLabel ? (
        <p className="notification-popover-empty">
          <CircleDashed aria-hidden="true" size={16} />
          <span>{presentation.idleLabel}</span>
        </p>
      ) : null}
    </>
  );

  return (
    <AnchoredOverlay
      anchor={anchor}
      ariaLabel="后台活动"
      autoFocus={autoFocus}
      className="notification-popover"
      id="notifications-root"
      initialFocusRef={openCenterButtonRef}
      onDismiss={onClose}
      open
      placement="bottom-end"
      preferredWidth={360}
      role="status"
      rootId="notifications"
    >
      <header className="notification-popover-header">
        <h2>后台活动</h2>
        <span data-notification-health={presentation.health}>
          {presentation.healthLabel}
        </span>
      </header>
      <div className="notification-popover-body" data-overlay-scroll-container>
        {activityContent}
      </div>
      <footer className="notification-popover-footer">
        <LiquidGlassButton
          className="notification-open-center"
          onClick={() => onOpenCenter()}
          ref={openCenterButtonRef}
          tone="control"
          type="button"
        >
          打开完整通知中心
        </LiquidGlassButton>
      </footer>
    </AnchoredOverlay>
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
      {operation.scanPaths.length > 0 ? (
        <div className="notification-scan-paths compact">
          {operation.scanPaths.slice(0, 3).map((row) => (
            <span key={row.key} title={row.path}>{row.path}</span>
          ))}
          {operation.scanPaths.length > 3 ? <span>+{operation.scanPaths.length - 3}</span> : null}
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

function ThumbnailSummaryRow({ summary }: { summary: DesktopThumbnailCacheSummary }) {
  const status = thumbnailStatusParts(summary);
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
          {status.length > 0
            ? status.map((part, index) => (
                <span key={part.kind}>
                  {index > 0 ? " · " : null}
                  {part.label}
                </span>
              ))
            : "已同步"}
        </span>
        {summary.totalCacheBytes == null ? null : <span>{formatBytes(summary.totalCacheBytes)}</span>}
      </div>
    </div>
  );
}

function progressStyle(percent: number): CSSProperties {
  return { "--notification-progress": `${percent}%` } as CSSProperties;
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
