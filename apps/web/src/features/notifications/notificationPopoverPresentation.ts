import type {
  CompactNotificationModel,
  NotificationOperationViewModel
} from "./notificationModel";

export interface ActiveOperationPresentation {
  phase: string;
  progressLabel: string;
  percentLabel: string | null;
}

export interface CompactNotificationPresentation {
  failureSummary: string | null;
  health: "degraded" | "failure" | "healthy";
  healthLabel: string;
  idleLabel: string | null;
}

export function compactNotificationPresentation(
  model: CompactNotificationModel
): CompactNotificationPresentation {
  const hasCurrentActivity =
    model.active.length > 0 || model.thumbnail.summaries.length > 0;
  const failureOnly =
    !model.degraded && model.indicator === "failure" && !hasCurrentActivity;

  if (model.degraded) {
    return {
      failureSummary: null,
      health: "degraded",
      healthLabel: "同步受限",
      idleLabel: hasCurrentActivity ? null : "等待通知同步恢复"
    };
  }

  const failureSummary = failureOnly
    ? model.results.find((result) => result.status === "failed")?.summary?.trim() ||
      "有后台活动失败，请打开完整通知中心查看。"
    : null;
  return {
    failureSummary,
    health: model.indicator === "failure" ? "failure" : "healthy",
    healthLabel: model.indicator === "failure" ? "需要关注" : "运行正常",
    idleLabel: hasCurrentActivity || failureOnly ? null : "当前没有后台活动"
  };
}

export function activeOperationPresentation(
  operation: NotificationOperationViewModel
): ActiveOperationPresentation {
  return {
    phase: operation.phase?.trim() || "处理中",
    progressLabel: operation.progress.label,
    percentLabel:
      operation.progress.determinate && operation.progress.percent != null
        ? `${operation.progress.percent}%`
        : null
  };
}
