export type DesktopIpcWindowRole = "main" | "settings" | "plugins" | "notifications";

const ALL_WINDOW_ROLES = new Set<DesktopIpcWindowRole>([
  "main",
  "settings",
  "plugins",
  "notifications"
]);
const PRIVILEGED_WINDOW_ROLES = new Set<DesktopIpcWindowRole>([
  "main",
  "settings",
  "plugins"
]);
const MAIN_WINDOW_ONLY = new Set<DesktopIpcWindowRole>(["main"]);

const COMMON_WINDOW_CHANNELS = new Set([
  "megle:window-minimize",
  "megle:window-maximize",
  "megle:window-close",
  "megle:window-is-maximized",
  "megle:window-get-bounds",
  "megle:window-set-position",
  "megle:window-begin-drag",
  "megle:window-drag-move",
  "megle:window-end-drag"
]);
const MAIN_ONLY_CHANNELS = new Set(["megle:shell-ready"]);
const PRIVILEGED_CHANNELS = new Set([
  "megle:pick-folder",
  "megle:diagnostics",
  "megle:shell-reveal-path",
  "megle:shell-open-path",
  "megle:open-external-authorization-url",
  "megle:clipboard-write-text",
  "megle:open-settings-window",
  "megle:close-settings-window",
  "megle:open-plugins-window",
  "megle:close-plugins-window",
  "megle:get-library-load-phase",
  "megle:retry-library-load",
  "megle:pick-new-library-path"
]);

export function canAccessDesktopIpc(
  role: DesktopIpcWindowRole | null,
  channel: string
): boolean {
  if (!role) return false;
  if (COMMON_WINDOW_CHANNELS.has(channel)) return ALL_WINDOW_ROLES.has(role);
  if (MAIN_ONLY_CHANNELS.has(channel)) return MAIN_WINDOW_ONLY.has(role);
  if (PRIVILEGED_CHANNELS.has(channel)) return PRIVILEGED_WINDOW_ROLES.has(role);
  return false;
}

export function assertDesktopIpcAccess(
  role: DesktopIpcWindowRole | null,
  channel: string
): asserts role is DesktopIpcWindowRole {
  if (!canAccessDesktopIpc(role, channel)) {
    throw new Error(`Desktop IPC access denied for ${role ?? "unknown"}: ${channel}`);
  }
}
