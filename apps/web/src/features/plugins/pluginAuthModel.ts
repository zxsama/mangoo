import type { HostPluginProjection, PluginConfiguration } from "@mangoo/core-client";

export type PluginAuthPrimaryAction =
  | "unavailable"
  | "configure"
  | "connect"
  | "wait"
  | "retry"
  | "manage";

export function pluginAuthPrimaryAction(
  plugin: HostPluginProjection | null,
  configuration: PluginConfiguration | null,
): PluginAuthPrimaryAction {
  if (!plugin) return "unavailable";
  if (!plugin.enabledIntent || plugin.permissionRequired || plugin.runtimeState === "permission-required") return "configure";
  if (plugin.runtimeState === "configuration-required" || configuration?.configured === false) return "configure";
  if (plugin.authState === "authorizing") return "wait";
  if (plugin.authState === "authenticated") return "manage";
  if (plugin.authState === "error" || plugin.authState === "expired" || plugin.runtimeState === "degraded" || plugin.runtimeState === "crashed-backoff") return "retry";
  return "connect";
}

export function accountKindLabel(kind: HostPluginProjection["accountKind"]): string {
  if (kind === "personal") return "个人账户";
  if (kind === "business") return "组织账户";
  return "账户";
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

export function configurationSourceLabel(source: PluginConfiguration["source"]): string {
  switch (source) {
    case "environment": return "部署环境";
    case "host-override": return "此设备配置";
    case "missing": return "尚未配置";
    case "default": return "插件默认配置";
  }
}

export function formatAuthRemaining(expiresAt: number, now = Date.now()): string {
  const seconds = Math.max(0, Math.ceil((expiresAt - now) / 1000));
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}
