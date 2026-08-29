import type { HostPluginProjection, OneDriveConfiguration } from "@megle/core-client";

export const ONEDRIVE_PLUGIN_ID = "com.megle.onedrive";

export type PluginAuthPrimaryAction =
  | "unavailable"
  | "enable"
  | "approve"
  | "configure"
  | "connect"
  | "wait"
  | "retry"
  | "manage";

export function pluginAuthPrimaryAction(
  plugin: HostPluginProjection | null,
  configuration: OneDriveConfiguration | null,
): PluginAuthPrimaryAction {
  if (!plugin) return "unavailable";
  if (!plugin.enabledIntent) return "enable";
  if (plugin.permissionRequired || plugin.runtimeState === "permission-required") return "approve";
  if (plugin.runtimeState === "configuration-required" || configuration?.configured === false) return "configure";
  if (plugin.authState === "authorizing") return "wait";
  if (plugin.authState === "authenticated") return "manage";
  if (plugin.authState === "error" || plugin.authState === "expired" || plugin.runtimeState === "degraded" || plugin.runtimeState === "crashed-backoff") return "retry";
  return "connect";
}

export function accountKindLabel(kind: HostPluginProjection["accountKind"]): string {
  if (kind === "personal") return "OneDrive Personal";
  if (kind === "business") return "OneDrive Business";
  return "OneDrive";
}

export function pluginAuthStateLabel(state: HostPluginProjection["authState"]): string {
  switch (state) {
    case "authenticated": return "已连接";
    case "authorizing": return "正在授权";
    case "authorization-required": return "需要授权";
    case "expired": return "授权已过期";
    case "error": return "授权失败";
    case "signed-out": return "已退出";
    default: return "未连接";
  }
}

export function configurationSourceLabel(source: OneDriveConfiguration["source"]): string {
  switch (source) {
    case "environment": return "部署环境（只读）";
    case "host-override": return "此设备高级覆盖";
    case "missing": return "尚未配置";
  }
}

export function formatAuthRemaining(expiresAt: number, now = Date.now()): string {
  const seconds = Math.max(0, Math.ceil((expiresAt - now) / 1000));
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}
