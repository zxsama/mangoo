import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CoreApiError } from "@megle/core-client";
import type { PluginAuthBeginResponse } from "@megle/core-client";
import { createCoreClient } from "../../core/client";
import { copyText, openExternalAuthorizationUrl, requestOAuthReturnFocus } from "../../core/desktop";
import { ONEDRIVE_PLUGIN_ID, pluginAuthPrimaryAction } from "./pluginAuthModel";

export const PLUGINS_QUERY_KEY = ["plugins"] as const;
export const ONEDRIVE_CONFIGURATION_QUERY_KEY = ["plugins", ONEDRIVE_PLUGIN_ID, "configuration"] as const;

function errorMessage(cause: unknown): string {
  if (cause instanceof CoreApiError) {
    const body = cause.body;
    if (body && typeof body === "object") {
      const record = body as Record<string, unknown>;
      if (typeof record.message === "string") return record.message;
      if (typeof record.error === "string") return record.error;
      if (typeof record.code === "string") return record.code;
    }
    return `请求失败 (${cause.status})`;
  }
  return cause instanceof Error ? cause.message : "OneDrive 请求失败";
}

export type PluginAuthController = ReturnType<typeof usePluginAuth>;

export function usePluginAuth(pluginId = ONEDRIVE_PLUGIN_ID, onChanged?: () => void | Promise<void>) {
  const client = useMemo(() => createCoreClient(), []);
  const queryClient = useQueryClient();
  const [flow, setFlow] = useState<PluginAuthBeginResponse | null>(null);
  const [responseUri, setResponseUri] = useState("");
  const [actionError, setActionError] = useState<string | null>(null);
  const [directBusy, setDirectBusy] = useState(false);
  const locallyStartedFlowRef = useRef<string | null>(null);
  const previousAuthStateRef = useRef<string | null | undefined>(undefined);

  const pluginsQuery = useQuery({
    queryKey: PLUGINS_QUERY_KEY,
    queryFn: () => client.listPlugins(),
    refetchInterval: (query) => flow || query.state.data?.items.some((item) => item.pluginId === pluginId && item.authState === "authorizing") ? 1_000 : 30_000,
  });
  const plugin = pluginsQuery.data?.items.find((item) => item.pluginId === pluginId) ?? null;
  const configurationQuery = useQuery({
    enabled: pluginId === ONEDRIVE_PLUGIN_ID,
    queryKey: ONEDRIVE_CONFIGURATION_QUERY_KEY,
    queryFn: () => client.getOneDriveConfiguration(),
  });
  const configuration = configurationQuery.data ?? null;

  useEffect(() => {
    const previous = previousAuthStateRef.current;
    const current = plugin?.authState;
    previousAuthStateRef.current = current;
    if (current === "authenticated" && previous !== "authenticated" && locallyStartedFlowRef.current) {
      locallyStartedFlowRef.current = null;
      void requestOAuthReturnFocus().catch(() => undefined);
    } else if (current === "error" || current === "expired" || current === "signed-out") {
      locallyStartedFlowRef.current = null;
    }
  }, [plugin?.authState]);

  useEffect(() => {
    if (!flow) return;
    if (!plugin || plugin.authState === "authenticated" || plugin.authState === "error" || plugin.authState === "expired") {
      setFlow(null);
      setResponseUri("");
    }
  }, [flow, plugin?.authState]);

  useEffect(() => () => setResponseUri(""), []);

  const invalidate = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: PLUGINS_QUERY_KEY }),
      queryClient.invalidateQueries({ queryKey: ["sync", "status"] }),
    ]);
    await onChanged?.();
  }, [onChanged, queryClient]);

  const run = useMutation({
    mutationFn: async (action: () => Promise<unknown>) => action(),
    onError: (cause) => setActionError(errorMessage(cause)),
    onSuccess: () => setActionError(null),
  });

  const enable = useCallback(async () => {
    await run.mutateAsync(() => client.enablePlugin(pluginId));
    await invalidate();
  }, [client, invalidate, pluginId, run]);

  const approve = useCallback(async () => {
    if (!plugin) return;
    await run.mutateAsync(() => client.updatePluginGrants(pluginId, { permissions: plugin.requestedPermissions }));
    await invalidate();
  }, [client, invalidate, plugin, pluginId, run]);

  const prepareConfiguration = useCallback(async () => {
    if (!plugin || (!plugin.permissionRequired && plugin.runtimeState !== "permission-required")) return;
    await run.mutateAsync(() => client.updatePluginGrants(pluginId, { permissions: plugin.requestedPermissions }));
    await invalidate();
  }, [client, invalidate, plugin, pluginId, run]);

  const retry = useCallback(async () => {
    await run.mutateAsync(() => client.retryPlugin(pluginId));
    await invalidate();
  }, [client, invalidate, pluginId, run]);

  const begin = useCallback(async (switchAccount = false) => {
    if (directBusy) return;
    setDirectBusy(true);
    setActionError(null);
    setResponseUri("");
    try {
      if (switchAccount) {
        await client.signOutPlugin(pluginId);
        await invalidate();
      }
      const nextFlow = await client.beginPluginAuth(pluginId);
      setFlow(nextFlow);
      const desktopResult = await openExternalAuthorizationUrl(nextFlow.authorizationUrl);
      if (desktopResult && !desktopResult.ok) {
        await client.cancelPluginAuth(pluginId, { flowId: nextFlow.flowId }).catch(() => undefined);
        setFlow(null);
        await invalidate();
        throw new Error(desktopResult.error === "external_browser_rejected" ? "授权地址未通过安全校验。" : "无法打开系统浏览器。");
      }
      if (!desktopResult && !window.open(nextFlow.authorizationUrl, "_blank", "noopener,noreferrer")) {
        setActionError("浏览器阻止了新标签页，请复制授权链接后手动打开。");
      } else {
        locallyStartedFlowRef.current = nextFlow.flowId;
      }
      await invalidate();
    } catch (cause) {
      setActionError(errorMessage(cause));
      await invalidate().catch(() => undefined);
      throw cause;
    } finally {
      setDirectBusy(false);
    }
  }, [client, directBusy, invalidate, pluginId]);

  const openAuthorizationUrl = useCallback(async () => {
    if (!flow) return;
    try {
      const desktopResult = await openExternalAuthorizationUrl(flow.authorizationUrl);
      if (desktopResult && !desktopResult.ok) {
        setActionError(desktopResult.error === "external_browser_rejected" ? "授权地址未通过安全校验。" : "无法打开系统浏览器。");
        return;
      }
      if (!desktopResult && !window.open(flow.authorizationUrl, "_blank", "noopener,noreferrer")) {
        setActionError("浏览器阻止了新标签页，请复制授权链接后手动打开。");
      }
    } catch (cause) {
      setActionError(errorMessage(cause));
    }
  }, [flow]);

  const complete = useCallback(async () => {
    if (!flow || flow.completionMode !== "manual-response-uri" || !responseUri.trim()) return;
    const submittedUri = responseUri;
    setResponseUri("");
    await run.mutateAsync(() => client.completePluginAuth(pluginId, { flowId: flow.flowId, responseUri: submittedUri }));
    setFlow(null);
    await invalidate();
  }, [client, flow, invalidate, pluginId, responseUri, run]);

  const cancel = useCallback(async () => {
    const active = flow;
    locallyStartedFlowRef.current = null;
    setResponseUri("");
    if (active) {
      await run.mutateAsync(() => client.cancelPluginAuth(pluginId, { flowId: active.flowId }));
      setFlow((current) => current?.flowId === active.flowId ? null : current);
    }
    await invalidate();
  }, [client, flow, invalidate, pluginId, run]);

  const signOut = useCallback(async () => {
    locallyStartedFlowRef.current = null;
    setFlow(null);
    setResponseUri("");
    await run.mutateAsync(() => client.signOutPlugin(pluginId));
    await invalidate();
  }, [client, invalidate, pluginId, run]);

  const copyAuthorizationUrl = useCallback(async () => {
    if (!flow) return false;
    const copied = await copyText(flow.authorizationUrl);
    if (!copied) setActionError("无法复制授权链接，请手动选择并复制。");
    return copied;
  }, [flow]);

  const saveConfiguration = useCallback(async (clientId: string) => {
    if (!configuration) throw new Error("OneDrive 配置尚未加载，请稍后重试。");
    if (directBusy) throw new Error("OneDrive 正在处理其他请求，请稍后重试。");
    setDirectBusy(true);
    setActionError(null);
    try {
      const plan = await client.planOneDriveConfiguration({
        clientId,
        expectedRevision: configuration.revision,
      });
      const committed = await client.commitOneDriveConfiguration({
        planUuid: plan.planUuid,
        expectedRevision: plan.expectedRevision,
        confirmed: Boolean(plan.effectiveChanged && plan.requiresConfirmation),
      });
      if (!committed.configured) {
        throw new Error("OneDrive Client ID 保存后尚未生效，请重试。");
      }
      setFlow(null);
      setResponseUri("");
      await Promise.all([
        invalidate(),
        queryClient.invalidateQueries({ queryKey: ONEDRIVE_CONFIGURATION_QUERY_KEY }),
      ]);
      return committed;
    } catch (cause) {
      throw new Error(errorMessage(cause));
    } finally {
      setDirectBusy(false);
    }
  }, [client, configuration, directBusy, invalidate, queryClient]);

  return {
    plugin,
    configuration,
    primaryAction: pluginAuthPrimaryAction(plugin, configuration),
    flow,
    responseUri,
    setResponseUri,
    busy: run.isPending || directBusy,
    loading: pluginsQuery.isLoading || configurationQuery.isLoading,
    error: actionError ?? (pluginsQuery.error ? errorMessage(pluginsQuery.error) : configurationQuery.error ? errorMessage(configurationQuery.error) : null),
    refresh: invalidate,
    enable,
    approve,
    prepareConfiguration,
    retry,
    begin,
    openAuthorizationUrl,
    complete,
    cancel,
    signOut,
    copyAuthorizationUrl,
    saveConfiguration,
  };
}
