import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type {
  HostPluginListResponse,
  HostPluginProjection,
  PluginDiscoveryError,
  PluginDiscoveryResponse
} from "@megle/core-client";
import { CoreApiError } from "@megle/core-client";
import { createCoreClient } from "../../core/client";
import { PLUGINS_QUERY_KEY } from "./usePluginAuth";

export const PLUGIN_LIST_REFRESH_MS = 30_000;
export const PLUGIN_DISCOVERY_COOLDOWN_MS = 5_000;
const EMPTY_PLUGINS: HostPluginProjection[] = [];

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
  removalError: string | null;
  lastDiscovery: PluginsDiscoverySummary | null;
  selectPlugin: (id: string | null) => void;
  refresh: () => Promise<void>;
  enable: (id: string) => Promise<void>;
  disable: (id: string) => Promise<void>;
  approvePermissions: (id: string) => Promise<void>;
  retry: (id: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
  dismissRemovalError: () => void;
}

interface PluginDiscoveryCoordinatorOptions {
  cooldownMs: number;
  discover: () => Promise<PluginDiscoveryResponse>;
  isActive: () => boolean;
  now?: () => number;
  onError: (cause: unknown) => void | Promise<void>;
  onSuccess: (response: PluginDiscoveryResponse) => void | Promise<void>;
}

export interface PluginDiscoveryCoordinator {
  run: () => Promise<void>;
}

export function createPluginDiscoveryCoordinator(
  options: PluginDiscoveryCoordinatorOptions
): PluginDiscoveryCoordinator {
  let inFlight: Promise<void> | null = null;
  let lastStartedAt = Number.NEGATIVE_INFINITY;

  return {
    run: () => {
      if (inFlight) return inFlight;

      const now = options.now?.() ?? Date.now();
      if (now - lastStartedAt < options.cooldownMs) return Promise.resolve();
      lastStartedAt = now;

      let operation: Promise<void>;
      operation = Promise.resolve()
        .then(options.discover)
        .then(async (response) => {
          if (options.isActive()) await options.onSuccess(response);
        })
        .catch(async (cause: unknown) => {
          if (options.isActive()) await options.onError(cause);
        })
        .finally(() => {
          if (inFlight === operation) inFlight = null;
        });
      inFlight = operation;
      return operation;
    }
  };
}

export function reconcilePluginSelection(
  current: string | null,
  plugins: HostPluginProjection[]
): string | null {
  if (plugins.length === 0) return null;
  return current && plugins.some((plugin) => plugin.pluginId === current)
    ? current
    : plugins[0].pluginId;
}

type PluginMutationRequest =
  | { kind: "enable"; pluginId: string }
  | { kind: "disable"; pluginId: string }
  | { kind: "approve-permissions"; plugin: HostPluginProjection }
  | { kind: "retry"; pluginId: string }
  | { kind: "remove"; pluginId: string };

type PluginMutationResult =
  | { kind: "replace"; plugin: HostPluginProjection }
  | { kind: "refresh" }
  | { kind: "remove"; pluginId: string };

export function usePluginsData(): PluginsState {
  const client = useMemo(() => createCoreClient(), []);
  const queryClient = useQueryClient();
  const mountedRef = useRef(false);
  const [selectedPluginId, setSelectedPluginId] = useState<string | null>(null);
  const [busyPluginIds, setBusyPluginIds] = useState<Set<string>>(() => new Set());
  const [actionError, setActionError] = useState<string | null>(null);
  const [removalError, setRemovalError] = useState<string | null>(null);
  const [discoveryError, setDiscoveryError] = useState<string | null>(null);
  const [lastDiscovery, setLastDiscovery] = useState<PluginsDiscoverySummary | null>(null);

  const {
    data: pluginsResponse,
    error: pluginsQueryError,
    isLoading,
    refetch: refetchPlugins
  } = useQuery({
    queryKey: PLUGINS_QUERY_KEY,
    queryFn: () => client.listPlugins(),
    refetchInterval: PLUGIN_LIST_REFRESH_MS
  });
  const plugins = pluginsResponse?.items ?? EMPTY_PLUGINS;

  const discoveryCoordinator = useMemo(
    () =>
      createPluginDiscoveryCoordinator({
        cooldownMs: PLUGIN_DISCOVERY_COOLDOWN_MS,
        discover: async () => {
          const response = await client.discoverPlugins();
          await queryClient.invalidateQueries({ queryKey: PLUGINS_QUERY_KEY, exact: true });
          return response;
        },
        isActive: () => mountedRef.current,
        onError: (cause) => {
          setLastDiscovery(null);
          setDiscoveryError(errorMessage(cause));
        },
        onSuccess: async (response) => {
          setDiscoveryError(null);
          setLastDiscovery({ discovered: response.discovered, errors: response.errors });
        }
      }),
    [client, queryClient]
  );

  useEffect(() => {
    mountedRef.current = true;
    const discover = () => void discoveryCoordinator.run();

    discover();
    window.addEventListener("focus", discover);
    window.addEventListener("online", discover);
    return () => {
      mountedRef.current = false;
      window.removeEventListener("focus", discover);
      window.removeEventListener("online", discover);
    };
  }, [discoveryCoordinator]);

  useEffect(() => {
    setSelectedPluginId((current) => reconcilePluginSelection(current, plugins));
  }, [plugins]);

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

  const { mutateAsync: mutatePlugin } = useMutation<
    PluginMutationResult,
    unknown,
    PluginMutationRequest
  >({
    mutationFn: async (request) => {
      switch (request.kind) {
        case "enable":
          return { kind: "replace", plugin: await client.enablePlugin(request.pluginId) };
        case "disable":
          return { kind: "replace", plugin: await client.disablePlugin(request.pluginId) };
        case "approve-permissions":
          return {
            kind: "replace",
            plugin: await client.updatePluginGrants(request.plugin.pluginId, {
              permissions: request.plugin.requestedPermissions
            })
          };
        case "retry":
          await client.retryPlugin(request.pluginId);
          return { kind: "refresh" };
        case "remove":
          await client.unregisterPlugin(request.pluginId);
          return { kind: "remove", pluginId: request.pluginId };
      }
    },
    onMutate: (request) => {
      setActionError(null);
      if (request.kind === "remove") setRemovalError(null);
      markBusy(pluginMutationId(request), true);
    },
    onSuccess: async (result) => {
      if (result.kind === "replace") {
        queryClient.setQueryData<HostPluginListResponse>(PLUGINS_QUERY_KEY, (current) => ({
          items: replacePlugin(current?.items ?? [], result.plugin)
        }));
        return;
      }
      if (result.kind === "remove") {
        queryClient.setQueryData<HostPluginListResponse>(PLUGINS_QUERY_KEY, (current) => ({
          items: (current?.items ?? []).filter((plugin) => plugin.pluginId !== result.pluginId)
        }));
        return;
      }
      await queryClient.invalidateQueries({ queryKey: PLUGINS_QUERY_KEY, exact: true });
    },
    onError: (cause, request) => {
      const message = errorMessage(cause);
      if (request.kind === "remove" && cause instanceof CoreApiError && cause.status === 409) {
        setRemovalError("此插件仍被账号或资源库使用，暂时无法移除。请先退出账号并解除相关资源库绑定。");
      } else {
        setActionError(message);
      }
    },
    onSettled: (_data, _error, request) => markBusy(pluginMutationId(request), false)
  });

  const runMutation = useCallback(
    async (request: PluginMutationRequest) => {
      try {
        await mutatePlugin(request);
      } catch {
        // The mutation callback exposes the error through this controller.
      }
    },
    [mutatePlugin]
  );

  const refresh = useCallback(async () => {
    await refetchPlugins();
  }, [refetchPlugins]);

  const enable = useCallback(
    (pluginId: string) => runMutation({ kind: "enable", pluginId }),
    [runMutation]
  );
  const disable = useCallback(
    (pluginId: string) => runMutation({ kind: "disable", pluginId }),
    [runMutation]
  );
  const approvePermissions = useCallback(
    async (pluginId: string) => {
      const plugin = plugins.find((item) => item.pluginId === pluginId);
      if (plugin) await runMutation({ kind: "approve-permissions", plugin });
    },
    [plugins, runMutation]
  );
  const retry = useCallback(
    (pluginId: string) => runMutation({ kind: "retry", pluginId }),
    [runMutation]
  );
  const remove = useCallback(
    (pluginId: string) => runMutation({ kind: "remove", pluginId }),
    [runMutation]
  );
  const selectPlugin = useCallback((id: string | null) => setSelectedPluginId(id), []);

  return {
    plugins,
    selectedPluginId,
    selectedPlugin,
    loading: isLoading,
    busyPluginIds,
    error:
      actionError ??
      discoveryError ??
      (pluginsQueryError ? errorMessage(pluginsQueryError) : null),
    removalError,
    lastDiscovery,
    selectPlugin,
    refresh,
    enable,
    disable,
    approvePermissions,
    retry,
    remove,
    dismissRemovalError: () => setRemovalError(null)
  };
}

function pluginMutationId(request: PluginMutationRequest): string {
  return request.kind === "approve-permissions" ? request.plugin.pluginId : request.pluginId;
}

function replacePlugin(
  current: HostPluginProjection[],
  record: HostPluginProjection
): HostPluginProjection[] {
  return current.some((plugin) => plugin.pluginId === record.pluginId)
    ? current.map((plugin) => (plugin.pluginId === record.pluginId ? record : plugin))
    : [...current, record];
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
