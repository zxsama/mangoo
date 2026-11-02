import type { createCoreClient, HostPluginProjection } from "@mangoo/core-client/node";
import type { NotificationPluginEntry, NotificationPluginOpenRequest, PluginViewWindowIntent } from "@mangoo/notification-contract";

type Client = Pick<ReturnType<typeof createCoreClient>, "getPlugin" | "getActiveLibrary">;

export function notificationPluginEntries(plugins: HostPluginProjection[]): NotificationPluginEntry[] {
  return plugins.filter((plugin) => plugin.enabledIntent && !plugin.permissionRequired
    && !["disabled", "stopping", "incompatible", "permission-required"].includes(plugin.runtimeState))
    .flatMap((plugin) => (plugin.contributions?.ui?.views ?? [])
      .filter((view) => view.slot === "notification-center")
      .map((view) => ({ pluginId: plugin.pluginId, viewId: view.id, title: view.title })));
}

export function parseNotificationPluginOpenRequest(input: unknown): NotificationPluginOpenRequest | null {
  if (!input || typeof input !== "object" || Array.isArray(input)) return null;
  const value = input as Record<string, unknown>;
  if (Object.keys(value).length !== 3 || Object.keys(value).some((key) => !["pluginId", "viewId", "libraryUuid"].includes(key))) return null;
  if (typeof value.pluginId !== "string" || value.pluginId.length > 255 || !/^[a-z0-9]+(\.[a-z0-9][a-z0-9-]*)+$/.test(value.pluginId)) return null;
  if (typeof value.viewId !== "string" || !/^[A-Za-z][A-Za-z0-9-]{0,63}$/.test(value.viewId)) return null;
  if (value.libraryUuid !== null && (typeof value.libraryUuid !== "string" || !value.libraryUuid || value.libraryUuid.length > 128)) return null;
  return { pluginId: value.pluginId, viewId: value.viewId, libraryUuid: value.libraryUuid };
}

export async function resolveNotificationPluginIntent(input: unknown, client: Client): Promise<PluginViewWindowIntent> {
  const request = parseNotificationPluginOpenRequest(input);
  if (!request) throw new Error("Invalid plugin notification request");
  const [plugin, active] = await Promise.all([client.getPlugin(request.pluginId), client.getActiveLibrary()]);
  if (!notificationPluginEntries([plugin]).some((entry) => entry.pluginId === request.pluginId && entry.viewId === request.viewId)) throw new Error("Plugin notification entry is unavailable");
  if ((active.library?.libraryUuid ?? null) !== request.libraryUuid) throw new Error("Plugin notification library changed");
  return { ...request, lifecycleEpoch: plugin.lifecycleEpoch, contextRevision: active.contextRevision };
}
