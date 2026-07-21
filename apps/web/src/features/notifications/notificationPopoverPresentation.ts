import type { NotificationOperationViewModel } from "./notificationModel";

export interface ActiveOperationPresentation {
  phase: string;
  progressLabel: string;
  percentLabel: string | null;
}

export interface ResultOperationPresentation {
  libraryLabel: string;
  detail: string;
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

export function resultOperationPresentation(
  operation: NotificationOperationViewModel
): ResultOperationPresentation {
  const explicitDetail = operation.errorDetail?.trim() || operation.summary?.trim();
  const detail =
    explicitDetail ||
    (operation.status === "failed"
      ? "操作失败"
      : operation.status === "cancelled"
        ? "操作已取消"
        : "已完成");
  return {
    libraryLabel: operation.libraryLabel,
    detail
  };
}
