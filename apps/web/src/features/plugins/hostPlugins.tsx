import { Suspense, lazy, type ComponentType } from "react";
import { useQuery } from "@tanstack/react-query";
import type { HostPluginProjection, PluginCapability, PluginPermission } from "@mangoo/core-client";

import { createCoreClient } from "../../core/client";
import { PLUGINS_QUERY_KEY } from "./usePluginAuth";

/**
 * Host-engine plugins (`engine: "host"`) are implemented inside the Host from
 * their own plugin directory. The plugin manager renders each one's panel in
 * its detail pane, keyed by plugin id. Process plugins keep using sandboxed
 * `plugin-detail` view contributions instead.
 */
interface HostPluginPanelProps {
  enabled: boolean;
  refreshKey?: unknown;
}

const HOST_PLUGIN_PANELS: Record<string, ComponentType<HostPluginPanelProps>> = {
  "com.mangoo.image-viewer": lazy(() =>
    import("@mangoo/plugin-image-viewer/web").then((module) => ({ default: module.ImageViewerPluginPanel }))
  )
};

export function isHostPluginActive(plugin: HostPluginProjection, capability: PluginCapability): boolean {
  return plugin.enabledIntent && plugin.runtimeState === "ready" && plugin.capabilities.includes(capability);
}

export function HostPluginPanel({ plugin }: { plugin: HostPluginProjection }) {
  const Panel = HOST_PLUGIN_PANELS[plugin.pluginId];
  if (!Panel) return null;
  const enabled = plugin.enabledIntent && plugin.runtimeState === "ready";
  return (
    <Suspense fallback={null}>
      <Panel enabled={enabled} refreshKey={`${plugin.lifecycleEpoch}:${plugin.runtimeState}`} />
    </Suspense>
  );
}

/**
 * Whether an enabled Host-engine plugin currently provides `capability`.
 * Shares the plugin list query, so cross-window plugin changes (broadcast by
 * the plugin manager) refresh it without polling.
 */
export function useHostCapability(capability: PluginCapability): boolean {
  const query = useQuery({
    queryKey: PLUGINS_QUERY_KEY,
    queryFn: () => createCoreClient().listPlugins(),
    staleTime: 30_000
  });
  return Boolean(query.data?.items.some((plugin) => isHostPluginActive(plugin, capability)));
}

export function permissionLabel(permission: PluginPermission): string {
  switch (permission.kind) {
    case "local-files":
      return "读取你打开的图片及其所在文件夹";
    case "file-association":
      return "注册为当前用户的系统图片查看器";
    case "temp-storage":
      return `临时存储（${Math.round(permission.maxBytes / 1024 / 1024)} MiB）`;
    case "network":
      return `网络访问：${permission.hosts.join("、")}`;
    case "secret":
      return `凭据存储：${permission.namespace}`;
    case "oauth-loopback":
      return "本机授权回调";
    case "host-ui":
      return `界面能力：${permission.operations.join("、")}`;
    default:
      return (permission as { kind: string }).kind;
  }
}
