import {
  AlertTriangle,
  CheckCircle2,
  Image,
  Layers3,
  Radio,
  Square,
  type AppIcon
} from "../../design/icons";
import { useEffect, useRef, useState, type ReactNode } from "react";
import type {
  DesktopThumbnailCacheSummary,
  ManagedRootNotificationTarget,
  NotificationWindowIntent
} from "@mangoo/notification-contract";
import { LiquidGlassButton } from "../../design/liquid-glass";
import type {
  CompactNotificationModel,
  NotificationOperationViewModel
} from "./notificationModel";
import {
  activateThumbnailFailureLogs,
  thumbnailFailureDetailsUnavailableMessage
} from "./notificationModel";
import type { NotificationRequestResult } from "./useNotifications";
import {
  beginNotificationAction,
  notificationOperationActionKey,
  settleNotificationAction,
  type NotificationActionStates
} from "./notificationActionModel";

type NotificationAction = (
  libraryId: string,
  operationId: string
) => Promise<{ ok: boolean; message: string | null }>;

type ManagedRootAction = (
  target: ManagedRootNotificationTarget
) => Promise<{ ok: boolean; message: string | null }>;

export function NotificationStatusView({
  cancel,
  children,
  dismissManagedRoot,
  model,
  onOpenFailureLogs,
  reviewManagedRoot,
  refreshCompact
}: {
  cancel: NotificationAction;
  children?: ReactNode;
  dismissManagedRoot: ManagedRootAction;
  model: CompactNotificationModel;
  onOpenFailureLogs(intent: NotificationWindowIntent): void;
  reviewManagedRoot: ManagedRootAction;
  refreshCompact(): Promise<NotificationRequestResult<boolean>>;
}) {
  const [actionStates, setActionStates] = useState<NotificationActionStates>(() => new Map());
  const actionStatesRef = useRef(actionStates);
  const actionRequestIdRef = useRef(0);
  const commitActionStates = (next: NotificationActionStates) => {
    actionStatesRef.current = next;
    setActionStates(next);
  };
  useEffect(() => {
    const clearedActionStates: NotificationActionStates = new Map();
    actionStatesRef.current = clearedActionStates;
    setActionStates(clearedActionStates);
  }, [model.activeLibraryId]);
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
        <StatusSection
          className="notification-status-health"
          count={1}
          headingId="notification-status-health-title"
          icon={AlertTriangle}
          title="通知服务受限"
        >
          <p>{model.healthDetail || "通知服务暂时不可用，浏览不会因此中断。"}</p>
        </StatusSection>
      ) : null}

      {model.active.length > 0 ? (
        <StatusSection
          className="notification-status-active"
          count={model.active.length}
          headingId="notification-status-active-title"
          icon={Radio}
          title="正在进行"
        >
          <div className="notification-status-operation-list">
            {model.active.map((operation) => {
              const actionKey = notificationOperationActionKey(
                operation.libraryId,
                operation.operationId
              );
              const actionState = actionStates.get(actionKey);
              const managedRootTarget = operation.managedRootAction?.kind === "review-placement"
                ? {
                    libraryId: operation.managedRootAction.libraryUuid,
                    operationId: operation.operationId,
                    rootUuid: operation.managedRootAction.rootUuid,
                    inventoryRevision: operation.managedRootAction.inventoryRevision
                  }
                : null;
              const runManagedRootAction = async (action: "dismiss" | "review") => {
                if (!managedRootTarget) return;
                const requestId = ++actionRequestIdRef.current;
                const started = beginNotificationAction(
                  actionStatesRef.current,
                  actionKey,
                  action,
                  requestId
                );
                if (!started.started) return;
                commitActionStates(started.states);
                const result = action === "dismiss"
                  ? await dismissManagedRoot(managedRootTarget)
                  : await reviewManagedRoot(managedRootTarget);
                await refreshCompact();
                commitActionStates(
                  settleNotificationAction(
                    actionStatesRef.current,
                    actionKey,
                    action,
                    requestId,
                    result.ok ? null : result.message || "操作失败"
                  )
                );
              };
              return (
                <ActiveOperation
                  cancelError={actionState?.errors.cancel ?? null}
                  cancelling={actionState?.pendingAction === "cancel"}
                  dismissing={actionState?.pendingAction === "dismiss"}
                  key={`${operation.libraryId}:${operation.operationId}`}
                  onCancel={async () => {
                    const requestId = ++actionRequestIdRef.current;
                    const started = beginNotificationAction(
                      actionStatesRef.current,
                      actionKey,
                      "cancel",
                      requestId
                    );
                    if (!started.started) return;
                    commitActionStates(started.states);
                    const cancellable = operation.sourceOperations.filter(
                      (sourceOperation) => sourceOperation.cancellable
                    );
                    const results = await Promise.all(
                      cancellable.map((sourceOperation) =>
                        cancel(sourceOperation.libraryId, sourceOperation.operationId)
                      )
                    );
                    await refreshCompact();
                    const failure = results.find((result) => !result.ok);
                    commitActionStates(
                      settleNotificationAction(
                        actionStatesRef.current,
                        actionKey,
                        "cancel",
                        requestId,
                        failure ? failure.message || "部分扫描未能取消" : null
                      )
                    );
                  }}
                  onDismiss={() => runManagedRootAction("dismiss")}
                  onReview={() => runManagedRootAction("review")}
                  operation={operation}
                  reviewing={actionState?.pendingAction === "review"}
                  managedRootActionError={
                    actionState?.errors.dismiss ?? actionState?.errors.review ?? null
                  }
                />
              );
            })}
          </div>
        </StatusSection>
      ) : null}

      {model.thumbnail.summaries.length > 0 ? (
        <StatusSection
          className="notification-status-thumbnails"
          count={model.thumbnail.summaries.length}
          headingId="notification-status-thumbnails-title"
          icon={Image}
          title="缩略图缓存"
        >
          <div className="notification-thumbnail-grid">
            {model.thumbnail.summaries.map((summary) => (
              <ThumbnailStatus
                key={summary.libraryId}
                onOpenFailureLogs={onOpenFailureLogs}
                summary={summary}
              />
            ))}
          </div>
        </StatusSection>
      ) : null}

      {model.results.length > 0 ? (
        <StatusSection
          className="notification-status-results"
          count={model.results.length}
          headingId="notification-status-results-title"
          icon={Layers3}
          title="最近重要结果"
        >
          <div className="notification-status-result-list">
            {model.results.map((operation) => (
              <article
                className={`notification-status-result status-${operation.status}`}
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
        </StatusSection>
      ) : null}
      {children}
    </div>
  );
}

function ViewHeading({ detail, title }: { detail: string; title: string }) {
  return (
    <header className="notification-view-heading">
      <h1>{title}</h1>
      <p>{detail}</p>
    </header>
  );
}

export function StatusSection({
  children,
  className,
  count,
  icon,
  headingId,
  title
}: {
  children: ReactNode;
  className: string;
  count: number;
  icon: AppIcon;
  headingId: string;
  title: string;
}) {
  const SectionIcon = icon;
  return (
    <section
      aria-labelledby={headingId}
      className={`${className} notification-status-section`}
    >
      <header className="notification-section-heading">
        <SectionIcon aria-hidden="true" size={15} />
        <h2 id={headingId}>{title}</h2>
        <span className="notification-section-count">{count}</span>
      </header>
      <div className="notification-section-content">{children}</div>
    </section>
  );
}

function ActiveOperation({
  cancelError,
  cancelling,
  dismissing,
  managedRootActionError,
  onCancel,
  onDismiss,
  onReview,
  operation,
  reviewing
}: {
  cancelError: string | null;
  cancelling: boolean;
  dismissing: boolean;
  managedRootActionError: string | null;
  onCancel(): Promise<void>;
  onDismiss(): Promise<void>;
  onReview(): Promise<void>;
  operation: NotificationOperationViewModel;
  reviewing: boolean;
}) {
  const hasManagedRootAction = operation.managedRootAction?.kind === "review-placement";
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
      {operation.scanPaths.length > 0 ? (
        <div className="notification-scan-paths">
          {operation.scanPaths.map((row) => (
            <span key={row.key} title={row.path}>{row.path}</span>
          ))}
        </div>
      ) : null}
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
      <div className="notification-status-operation-footer">
        <span className="notification-status-progress-caption">{operation.progress.label}</span>
        {hasManagedRootAction ? (
          <>
            <LiquidGlassButton
              disabled={reviewing || dismissing}
              onClick={() => void onReview()}
              tone="primary"
              type="button"
            >
              {reviewing ? "打开中…" : "管理插件"}
            </LiquidGlassButton>
            <LiquidGlassButton
              disabled={reviewing || dismissing}
              onClick={() => void onDismiss()}
              type="button"
            >
              {dismissing ? "关闭中…" : "关闭"}
            </LiquidGlassButton>
          </>
        ) : null}
        {operation.cancellable ? (
          <LiquidGlassButton
            aria-label={`取消 ${operation.title}`}
            disabled={cancelling}
            onClick={() => void onCancel()}
            tone="danger"
            type="button"
          >
            <Square aria-hidden="true" size={13} />
            {cancelling ? "取消中…" : "取消"}
          </LiquidGlassButton>
        ) : null}
      </div>
      {cancelError ? <p className="notification-row-action-error" role="alert">{cancelError}</p> : null}
      {managedRootActionError ? (
        <p className="notification-row-action-error" role="alert">{managedRootActionError}</p>
      ) : null}
    </article>
  );
}

function ThumbnailStatus({
  onOpenFailureLogs,
  summary
}: {
  onOpenFailureLogs(intent: NotificationWindowIntent): void;
  summary: DesktopThumbnailCacheSummary;
}) {
  const unavailableMessage = thumbnailFailureDetailsUnavailableMessage(summary);
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
        <div>
          <dt>失败</dt>
          <dd>
            {summary.failedCount > 0 && !unavailableMessage ? (
              <button
                className="notification-thumbnail-failure-action"
                onClick={() => activateThumbnailFailureLogs(summary, onOpenFailureLogs)}
                type="button"
              >
                {summary.failedCount.toLocaleString()}
              </button>
            ) : summary.failedCount > 0 ? (
              <span title={unavailableMessage ?? undefined}>
                {summary.failedCount.toLocaleString()} · 详情不可用
              </span>
            ) : "0"}
          </dd>
        </div>
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
