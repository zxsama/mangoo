import {
  AlertTriangle,
  CheckCircle2,
  CircleDashed,
  Image,
  Layers3,
  Radio
} from "lucide-react";
import type { DesktopThumbnailCacheSummary } from "@megle/notification-contract";
import type {
  CompactNotificationModel,
  NotificationOperationViewModel
} from "./notificationModel";

export function NotificationStatusView({ model }: { model: CompactNotificationModel }) {
  const hasStatus =
    model.active.length > 0 ||
    model.thumbnail.summaries.length > 0 ||
    model.results.length > 0 ||
    model.degraded;

  return (
    <div className="notification-view notification-status-view">
      <ViewHeading
        detail="所有资源库正在进行的操作、缩略图缓存与最近重要结果。"
        title="当前状态"
      />

      {!hasStatus ? (
        <div className="notification-status-empty" role="status">
          <CheckCircle2 aria-hidden="true" size={20} />
          <div>
            <strong>当前没有后台操作</strong>
            <span>新的任务和重要结果会在这里汇总。</span>
          </div>
        </div>
      ) : null}

      {model.degraded ? (
        <section
          aria-labelledby="notification-status-health-title"
          className="notification-status-health notification-status-section"
        >
          <SectionHeading
            icon={<AlertTriangle aria-hidden="true" size={16} />}
            id="notification-status-health-title"
            title="通知同步受限"
          />
          <p>{model.healthDetail || "通知服务暂时不可用，浏览不会因此中断。"}</p>
        </section>
      ) : null}

      {model.active.length > 0 ? (
        <section
          aria-labelledby="notification-status-active-title"
          className="notification-status-active notification-status-section"
        >
          <SectionHeading
            count={model.active.length}
            icon={<Radio aria-hidden="true" size={16} />}
            id="notification-status-active-title"
            title="正在进行"
          />
          <div className="notification-status-operation-list">
            {model.active.map((operation) => (
              <ActiveOperation key={`${operation.libraryId}:${operation.operationId}`} operation={operation} />
            ))}
          </div>
        </section>
      ) : null}

      {model.thumbnail.summaries.length > 0 ? (
        <section
          aria-labelledby="notification-status-thumbnails-title"
          className="notification-status-thumbnails notification-status-section"
        >
          <SectionHeading
            count={model.thumbnail.summaries.length}
            icon={<Image aria-hidden="true" size={16} />}
            id="notification-status-thumbnails-title"
            title="缩略图缓存"
          />
          <div className="notification-thumbnail-grid">
            {model.thumbnail.summaries.map((summary) => (
              <ThumbnailStatus key={summary.libraryId} summary={summary} />
            ))}
          </div>
        </section>
      ) : null}

      {model.results.length > 0 ? (
        <section
          aria-labelledby="notification-status-results-title"
          className="notification-status-results notification-status-section"
        >
          <SectionHeading
            icon={<Layers3 aria-hidden="true" size={16} />}
            id="notification-status-results-title"
            title="最近重要结果"
          />
          <div className="notification-status-result-list">
            {model.results.map((operation) => (
              <article
                className={`notification-status-result notification-status-${operation.status}`}
                key={`${operation.libraryId}:${operation.operationId}`}
              >
                <div className="notification-status-row-heading">
                  <strong>{operation.title}</strong>
                  <span>{operation.libraryLabel}</span>
                </div>
                <p>{operation.errorDetail || operation.summary || statusLabel(operation.status)}</p>
                <time dateTime={new Date(operation.updatedAt).toISOString()}>
                  {formatDateTime(operation.updatedAt)}
                </time>
              </article>
            ))}
          </div>
        </section>
      ) : null}
    </div>
  );
}

function ViewHeading({ detail, title }: { detail: string; title: string }) {
  return (
    <header className="notification-view-heading">
      <div>
        <span className="notification-view-eyebrow">OVERVIEW</span>
        <h1>{title}</h1>
      </div>
      <p>{detail}</p>
    </header>
  );
}

function SectionHeading({
  count,
  icon,
  id,
  title
}: {
  count?: number;
  icon: React.ReactNode;
  id: string;
  title: string;
}) {
  return (
    <header className="notification-status-section-heading">
      <span className="notification-status-section-icon">{icon}</span>
      <h2 id={id}>{title}</h2>
      {count == null ? null : <span className="notification-status-count">{count}</span>}
    </header>
  );
}

function ActiveOperation({ operation }: { operation: NotificationOperationViewModel }) {
  return (
    <article className="notification-status-operation">
      <div className="notification-status-row-heading">
        <strong>{operation.title}</strong>
        <span>{operation.libraryLabel}</span>
      </div>
      <div className="notification-status-phase-row">
        <span>{operation.phase || operation.progress.label}</span>
        {operation.progress.determinate ? <strong>{operation.progress.percent}%</strong> : null}
      </div>
      <div
        aria-label={operation.progress.label}
        aria-valuemax={operation.progress.determinate ? 100 : undefined}
        aria-valuemin={operation.progress.determinate ? 0 : undefined}
        aria-valuenow={operation.progress.percent ?? undefined}
        className={`notification-window-progress${operation.progress.determinate ? "" : " indeterminate"}`}
        role="progressbar"
      >
        <span style={{ width: operation.progress.percent == null ? undefined : `${operation.progress.percent}%` }} />
      </div>
      <span className="notification-status-progress-caption">{operation.progress.label}</span>
    </article>
  );
}

function ThumbnailStatus({ summary }: { summary: DesktopThumbnailCacheSummary }) {
  const percent =
    summary.completionPercent ??
    (summary.totalCandidateCount > 0
      ? Math.round((summary.cachedCount / summary.totalCandidateCount) * 100)
      : 0);
  return (
    <article className="notification-thumbnail-card">
      <div className="notification-status-row-heading">
        <strong>{summary.libraryName || summary.libraryId}</strong>
        <span>{percent}%</span>
      </div>
      <div
        aria-label={`${summary.libraryName || summary.libraryId} 缩略图缓存 ${percent}%`}
        aria-valuemax={100}
        aria-valuemin={0}
        aria-valuenow={percent}
        className="notification-window-progress"
        role="progressbar"
      >
        <span style={{ width: `${percent}%` }} />
      </div>
      <dl className="notification-window-thumbnail-metrics">
        <div><dt>已缓存</dt><dd>{summary.cachedCount.toLocaleString()}</dd></div>
        <div><dt>待处理</dt><dd>{summary.pendingCount.toLocaleString()}</dd></div>
        <div><dt>失败</dt><dd>{summary.failedCount.toLocaleString()}</dd></div>
      </dl>
    </article>
  );
}

function statusLabel(status: NotificationOperationViewModel["status"]): string {
  if (status === "succeeded") return "已完成";
  if (status === "failed") return "失败";
  if (status === "cancelled") return "已取消";
  return "处理中";
}

function formatDateTime(value: number): string {
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit"
  }).format(value);
}
