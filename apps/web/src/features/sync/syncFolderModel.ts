import type { RootOperationPhase, SyncFolderAllowedAction, SyncFolderRecord } from "@megle/core-client";
import { CoreApiError } from "@megle/core-client";

export function syncFolderCoreErrorCode(cause: unknown): string | null {
  if (!(cause instanceof CoreApiError) || !cause.body || typeof cause.body !== "object") return null;
  const code = (cause.body as { code?: unknown }).code;
  return typeof code === "string" ? code : null;
}

const STATE_LABELS: Record<NonNullable<SyncFolderRecord["binding"]>["state"], string> = {
  materializing: "正在放置",
  active: "同步中",
  paused: "已暂停",
  "local-only-fork": "仅本机分叉",
  "reconcile-required": "需要比较",
  relocating: "正在移动",
  "local-rename-required": "需要调整本机名称",
  unavailable: "位置不可用",
  error: "需要处理"
};

const INVENTORY_LABELS: Record<SyncFolderRecord["descriptor"]["inventoryState"], string> = {
  unselected: "未在此设备同步",
  "placement-required": "等待选择位置",
  tombstoned: "已删除",
  incompatible: "版本不兼容",
  "inventory-error": "清单异常"
};

export function syncFolderStateLabel(folder: SyncFolderRecord): string {
  return folder.binding ? STATE_LABELS[folder.binding.state] : INVENTORY_LABELS[folder.descriptor.inventoryState];
}

export function syncFolderIdentitySuffix(folder: SyncFolderRecord, siblings: SyncFolderRecord[]): string | null {
  const duplicates = siblings.filter((item) => item.descriptor.requestedName === folder.descriptor.requestedName);
  return duplicates.length > 1 ? folder.descriptor.rootUuid.slice(0, 8) : null;
}

export function visibleSyncFolderActions(folder: SyncFolderRecord): SyncFolderAllowedAction[] {
  if (folder.binding?.state === "local-only-fork") {
    return folder.allowedActions.filter((action) => action === "re-enable" || action === "publish-as-new" || action === "retry");
  }
  return folder.allowedActions;
}

export function rootOperationPhaseLabel(phase: RootOperationPhase): string {
  const labels: Partial<Record<RootOperationPhase, string>> = {
    planned: "已计划",
    preflighting: "正在检查位置",
    comparing: "正在比较本机与远端",
    "comparison-ready": "比较完成，等待确认",
    quiescing: "正在暂停文件活动",
    transferring: "正在移动文件",
    verifying: "正在验证文件",
    "catalog-cutover": "正在切换目录",
    "host-cutover": "正在更新本机绑定",
    cleanup: "正在清理旧位置",
    "local-rename-required": "远端名称已更新，等待处理本机目录",
    completed: "已完成",
    cancelled: "已取消",
    failed: "失败"
  };
  return labels[phase] ?? phase;
}

export function canCancelRootOperation(phase: RootOperationPhase, cutoverCommitted: boolean): boolean {
  return !cutoverCommitted && !["catalog-cutover", "host-cutover", "cleanup", "completed", "cancelled", "failed"].includes(phase);
}
