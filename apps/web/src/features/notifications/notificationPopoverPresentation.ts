import type { NotificationOperationViewModel } from "./notificationModel";

export interface ActiveOperationPresentation {
  phase: string;
  progressLabel: string;
  percentLabel: string | null;
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
