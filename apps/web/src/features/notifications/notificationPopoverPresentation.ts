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
  failureCue: string | null;
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
      failureCue: null,
      health: "degraded",
      healthLabel: "同步受限",
      idleLabel: hasCurrentActivity ? null : "等待通知同步恢复"
    };
  }

  return {
    failureCue: failureOnly ? "部分任务失败，请打开完整通知中心查看" : null,
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
