import { useCallback, useEffect, useMemo, useState } from "react";
import type { HostPluginProjection, PluginDiscoveryError } from "@megle/core-client";
import { CoreApiError } from "@megle/core-client";
import { createCoreClient } from "../../core/client";

export interface PluginsDiscoverySummary {
  discovered: number;
  errors: PluginDiscoveryError[];
}

export interface PluginsState {
  plugins: HostPluginProjection[];
  selectedPluginId: string | null;
  selectedPlugin: HostPluginProjection | null;
  loading: boolean;
  busyPluginIds: Set<string>;
  error: string | null;
  lastDiscovery: PluginsDiscoverySummary | null;
  selectPlugin: (id: string | null) => void;
  refresh: () => Promise<void>;
  discover: () => Promise<void>;
  enable: (id: string) => Promise<void>;
  disable: (id: string) => Promise<void>;
  approvePermissions: (id: string) => Promise<void>;
  retry: (id: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
}

export function usePluginsData(): PluginsState {
  const client = useMemo(() => createCoreClient(), []);
  const [plugins, setPlugins] = useState<HostPluginProjection[]>([]);
  const [selectedPluginId, setSelectedPluginId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busyPluginIds, setBusyPluginIds] = useState<Set<string>>(() => new Set());
  const [error, setError] = useState<string | null>(null);
  const [lastDiscovery, setLastDiscovery] = useState<PluginsDiscoverySummary | null>(null);

  const selectedPlugin = useMemo(
    () => plugins.find((plugin) => plugin.pluginId === selectedPluginId) ?? null,
    [plugins, selectedPluginId]
  );

  const markBusy = useCallback((pluginId: string, busy: boolean) => {
    setBusyPluginIds((current) => {
      const next = new Set(current);
      if (busy) {
        next.add(pluginId);
      } else {
        next.delete(pluginId);
      }
      return next;
    });
  }, []);

  const loadPlugins = useCallback(async () => {
    const response = await client.listPlugins();
    setPlugins(response.items);
    setSelectedPluginId((current) => {
      if (current === null) return current;
      return response.items.some((plugin) => plugin.pluginId === current) ? current : null;
    });
    return response.items;
  }, [client]);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      await loadPlugins();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setLoading(false);
    }
  }, [loadPlugins]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const discover = useCallback(async () => {
    setError(null);
    try {
      const response = await client.discoverPlugins();
      setLastDiscovery({ discovered: response.discovered, errors: response.errors });
      await loadPlugins();
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }, [client, loadPlugins]);

  const replacePlugin = useCallback((record: HostPluginProjection) => {
    setPlugins((current) => current.map((plugin) => (plugin.pluginId === record.pluginId ? record : plugin)));
  }, []);

  const enable = useCallback(
    async (id: string) => {
      markBusy(id, true);
      setError(null);
      try {
        const record = await client.enablePlugin(id);
        replacePlugin(record);
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        markBusy(id, false);
      }
    },
    [client, markBusy, replacePlugin]
  );

  const disable = useCallback(
    async (id: string) => {
      markBusy(id, true);
      setError(null);
      try {
        const record = await client.disablePlugin(id);
        replacePlugin(record);
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        markBusy(id, false);
      }
    },
    [client, markBusy, replacePlugin]
  );

  const approvePermissions = useCallback(
    async (id: string) => {
      const plugin = plugins.find((item) => item.pluginId === id);
      if (!plugin) return;
      markBusy(id, true);
      setError(null);
      try {
        const record = await client.updatePluginGrants(id, {
          permissions: plugin.requestedPermissions
        });
        replacePlugin(record);
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        markBusy(id, false);
      }
    },
    [client, markBusy, plugins, replacePlugin]
  );

  const retry = useCallback(
    async (id: string) => {
      markBusy(id, true);
      setError(null);
      try {
        await client.retryPlugin(id);
        await loadPlugins();
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        markBusy(id, false);
      }
    },
    [client, loadPlugins, markBusy]
  );

  const remove = useCallback(
    async (id: string) => {
      markBusy(id, true);
      setError(null);
      try {
        await client.unregisterPlugin(id);
        setPlugins((current) => current.filter((plugin) => plugin.pluginId !== id));
        setSelectedPluginId((current) => (current === id ? null : current));
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        markBusy(id, false);
      }
    },
    [client, markBusy]
  );

  const selectPlugin = useCallback((id: string | null) => {
    setSelectedPluginId(id);
  }, []);

  return {
    plugins,
    selectedPluginId,
    selectedPlugin,
    loading,
    busyPluginIds,
    error,
    lastDiscovery,
    selectPlugin,
    refresh,
    discover,
    enable,
    disable,
    approvePermissions,
    retry,
    remove
  };
}

function errorMessage(cause: unknown): string {
  if (cause instanceof CoreApiError) {
    const body = cause.body;
    if (body && typeof body === "object") {
      const record = body as Record<string, unknown>;
      if (typeof record.error === "string") return record.error;
      if (typeof record.message === "string") return record.message;
    }
    return `Request failed (${cause.status})`;
  }
  if (cause instanceof Error) {
    return cause.message;
  }
  return "Plugin request failed";
}
