import type { LibrarySyncStatus, SyncAllowedAction, SyncStatusSnapshot } from "@megle/core-client";

export interface SyncSummaryModel {
  attentionCount: number;
  activeCount: number;
  pausedCount: number;
  label: string;
}

export function syncSummaryModel(snapshot: SyncStatusSnapshot | null): SyncSummaryModel {
  const libraries = snapshot?.libraries ?? [];
  const attentionCount = libraries.filter((item) => item.lifecycle === "error" || item.conflictCount > 0).length;
  const pausedCount = libraries.filter((item) => item.lifecycle === "paused").length;
  const activeCount = libraries.filter((item) => !["local-only", "paused", "error"].includes(item.lifecycle)).length;
  return {
    attentionCount,
    activeCount,
    pausedCount,
    label: attentionCount > 0 ? `${attentionCount} 个资源库需要处理` : activeCount > 0 ? `正在同步 ${activeCount} 个资源库` : pausedCount > 0 ? `${pausedCount} 个资源库已暂停` : "同步已就绪"
  };
}

export function syncStatusLabel(status: LibrarySyncStatus | undefined): string {
  if (!status) return "读取中";
  if (status.conflictCount > 0) return `${status.conflictCount} 个冲突`;
  if (status.lastErrorCode) return "需要处理";
  const labels: Record<LibrarySyncStatus["lifecycle"], string> = {
    "local-only": "仅本机",
    enabling: "正在设置",
    syncing: "同步中",
    paused: "已暂停",
    unbinding: "正在取消同步",
    restoring: status.catalogReadiness === "catalog-ready" ? "媒体恢复中" : "正在恢复目录",
    error: "同步异常"
  };
  return labels[status.lifecycle];
}

export function actionLabel(action: SyncAllowedAction): string {
  return ({ enable: "启用同步", pause: "暂停", resume: "继续", retry: "重试", "sync-now": "立即同步", "cancel-before-catalog": "取消恢复", unbind: "取消同步", "resolve-conflicts": "处理冲突", reconnect: "重新连接" })[action];
}
