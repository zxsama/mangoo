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
const NOTIFICATION_PLUGIN_CHANNELS = new Set(["mangoo:notification-plugin-entries", "mangoo:open-notification-plugin-view"]);

const COMMON_WINDOW_CHANNELS = new Set([
  "mangoo:window-minimize",
  "mangoo:window-maximize",
  "mangoo:window-close",
  "mangoo:window-is-maximized",
  "mangoo:window-get-bounds",
  "mangoo:window-set-position",
  "mangoo:window-begin-drag",
  "mangoo:window-drag-move",
  "mangoo:window-end-drag"
]);
const MAIN_ONLY_CHANNELS = new Set(["mangoo:shell-ready"]);
const LIBRARY_CREATION_CHANNELS = new Set(["mangoo:resolve-library-creation-paths"]);
const PRIVILEGED_CHANNELS = new Set([
  "mangoo:pick-folder",
  "mangoo:diagnostics",
  "mangoo:shell-reveal-path",
  "mangoo:shell-open-path",
  "mangoo:open-plugin-external-link",
  "mangoo:request-oauth-return-focus",
  "mangoo:clipboard-write-text",
  "mangoo:open-settings-window",
  "mangoo:close-settings-window",
  "mangoo:open-plugins-window",
  "mangoo:close-plugins-window",
  "mangoo:get-library-load-phase",
  "mangoo:retry-library-load",
  "mangoo:pick-new-library-path"
]);

export function canAccessDesktopIpc(
  role: DesktopIpcWindowRole | null,
  channel: string
): boolean {
  if (!role) return false;
  if (NOTIFICATION_PLUGIN_CHANNELS.has(channel)) return role === "notifications";
  if (COMMON_WINDOW_CHANNELS.has(channel)) return ALL_WINDOW_ROLES.has(role);
  if (MAIN_ONLY_CHANNELS.has(channel)) return MAIN_WINDOW_ONLY.has(role);
  if (LIBRARY_CREATION_CHANNELS.has(channel)) return role === "main" || role === "settings";
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
