import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CoreApiError } from "@mangoo/core-client";
import type { PluginAuthBeginResponse, PluginConfigurationPlan, PluginExternalLinkRequest } from "@mangoo/core-client";
import { createCoreClient } from "../../core/client";
import { copyText, openPluginExternalLink, requestOAuthReturnFocus } from "../../core/desktop";
import { pluginAuthPrimaryAction } from "./pluginAuthModel";
import { broadcastPluginChange, usePluginQuerySync } from "./pluginQuerySync";

export const PLUGINS_QUERY_KEY = ["plugins"] as const;
export const pluginConfigurationQueryKey = (pluginId: string) => ["plugins", pluginId, "configuration"] as const;

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
  return cause instanceof Error ? cause.message : "插件请求失败";
}

export type PluginAuthController = ReturnType<typeof usePluginAuth>;

export function usePluginAuth(selectedPluginId?: string, onChanged?: () => void | Promise<void>) {
  const client = useMemo(() => createCoreClient(), []);
  const queryClient = useQueryClient();
  usePluginQuerySync(queryClient);
  const [flow, setFlow] = useState<PluginAuthBeginResponse | null>(null);
  const flowRef = useRef<PluginAuthBeginResponse | null>(null);
  const [responseUri, setResponseUri] = useState("");
  const [actionError, setActionError] = useState<string | null>(null);
  const [directBusy, setDirectBusy] = useState(false);
  const busyRef = useRef(false);
  const operationRef = useRef(0);
  const locallyStartedFlowRef = useRef<string | null>(null);
  const previousAuthStateRef = useRef<string | null | undefined>(undefined);
  const [configurationPlan, setConfigurationPlan] = useState<PluginConfigurationPlan | null>(null);

  const pluginsQuery = useQuery({
    queryKey: PLUGINS_QUERY_KEY,
    queryFn: () => client.listPlugins(),
    refetchInterval: (query) => flow || query.state.data?.items.some((item) => item.authState === "authorizing" || item.runtimeState === "stopping") ? 1_000 : 30_000,
  });
  const availablePlugins = pluginsQuery.data?.items.filter((item) => item.contributions?.authentication) ?? [];
  const pluginId = selectedPluginId ?? availablePlugins[0]?.pluginId ?? "";
  const plugin = pluginsQuery.data?.items.find((item) => item.pluginId === pluginId) ?? null;
  const currentPluginIdRef = useRef(pluginId);
  currentPluginIdRef.current = pluginId;
  const configurationQuery = useQuery({
    enabled: Boolean(plugin?.contributions),
    queryKey: pluginConfigurationQueryKey(pluginId),
    queryFn: () => client.getPluginConfiguration(pluginId),
  });
  const configuration = configurationQuery.data ?? null;
  const remoteAvailable = Boolean(plugin?.enabledIntent && !plugin.permissionRequired
    && !["disabled", "stopping", "incompatible", "permission-required", "configuration-required"].includes(plugin.runtimeState));

  const clearFlow = useCallback(() => {
    operationRef.current += 1;
    flowRef.current = null;
    locallyStartedFlowRef.current = null;
    setFlow(null);
    setResponseUri("");
  }, []);
  useEffect(() => {
    clearFlow();
    setConfigurationPlan(null);
    setActionError(null);
    previousAuthStateRef.current = undefined;
    return () => { operationRef.current += 1; flowRef.current = null; locallyStartedFlowRef.current = null; };
  }, [clearFlow, pluginId]);

  useEffect(() => {
    const previous = previousAuthStateRef.current;
    const current = plugin?.authState;
    previousAuthStateRef.current = current;
    if (current === "authenticated" && previous !== "authenticated" && locallyStartedFlowRef.current && remoteAvailable) {
      void requestOAuthReturnFocus().catch(() => undefined);
    }
    if (!remoteAvailable || current === "authenticated" || current === "signed-out" || current === "error" || current === "expired") clearFlow();
  }, [clearFlow, plugin?.authState, remoteAvailable]);

  // Poll exact ownership as runtime/account epochs can change without changing the visible status.
  useEffect(() => {
    if (!flow) return;
    let disposed = false;
    const verify = async () => {
      if (flow.expiresAt <= Date.now()) { clearFlow(); return; }
      try { await client.resolvePluginExternalLink(pluginId, { kind: "authorization", id: flow.flowId }); }
      catch (cause) {
        if (!disposed && flowRef.current?.flowId === flow.flowId && cause instanceof CoreApiError && [400, 403, 404, 409, 410].includes(cause.status)) clearFlow();
      }
    };
    const timer = window.setInterval(() => void verify(), 1_000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [clearFlow, client, flow, pluginId]);

  const invalidate = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: PLUGINS_QUERY_KEY }),
    ]);
    broadcastPluginChange();
    await onChanged?.();
  }, [onChanged, queryClient]);
  const run = useMutation({ mutationFn: async (action: () => Promise<unknown>) => action(), onError: (cause) => setActionError(errorMessage(cause)), onSuccess: () => setActionError(null) });
  const enable = async () => { await run.mutateAsync(() => client.enablePlugin(pluginId)); await invalidate(); };
  const approve = async () => { if (plugin) { await run.mutateAsync(() => client.updatePluginGrants(pluginId, { permissions: plugin.requestedPermissions })); await invalidate(); } };
  const prepareConfiguration = async () => { if (plugin?.permissionRequired || plugin?.runtimeState === "permission-required") await approve(); };
  const retry = async () => { await run.mutateAsync(() => client.retryPlugin(pluginId)); await invalidate(); };

  const resolveFlowLink = async (active: PluginAuthBeginResponse) => {
    if (flowRef.current !== active || active.expiresAt <= Date.now() || currentPluginIdRef.current !== pluginId) throw new Error("授权流程已失效，请重新登录。");
    const link = await client.resolvePluginExternalLink(pluginId, { kind: "authorization", id: active.flowId });
    if (flowRef.current !== active || active.expiresAt <= Date.now() || currentPluginIdRef.current !== pluginId) throw new Error("授权流程已失效，请重新登录。");
    return link;
  };
  const openLink = async (request: PluginExternalLinkRequest, current?: () => boolean) => {
    const result = await openPluginExternalLink({ pluginId, ...request });
    if (current?.() === false) throw new Error("授权流程已失效，请重新登录。");
    if (result && !result.ok) throw new Error(result.error === "external_browser_rejected" ? "链接已失效或未通过安全校验。" : "无法打开系统浏览器。");
    if (!result) {
      const link = await client.resolvePluginExternalLink(pluginId, request);
      if (current?.() === false) throw new Error("授权流程已失效，请重新登录。");
      if (!window.open(link.url, "_blank", "noopener,noreferrer")) throw new Error("浏览器阻止了新标签页，请允许后重新登录。");
    }
  };
  const cancelAfterOpenFailure = async (active: PluginAuthBeginResponse) => {
    if (flowRef.current === active) clearFlow();
    await client.cancelPluginAuth(pluginId, { flowId: active.flowId }).catch(() => undefined);
    await invalidate();
  };
  const begin = async (switchAccount = false) => {
    if (busyRef.current) return;
    if (!remoteAvailable) throw new Error("请先启用插件并完成配置与权限设置。");
    busyRef.current = true; setDirectBusy(true); setActionError(null); clearFlow();
    const operation = operationRef.current;
    let nextFlow: PluginAuthBeginResponse | null = null;
    try {
      if (switchAccount) await client.signOutPlugin(pluginId);
      nextFlow = await client.beginPluginAuth(pluginId);
      if (operation !== operationRef.current || currentPluginIdRef.current !== pluginId) { await client.cancelPluginAuth(pluginId, { flowId: nextFlow.flowId }); return; }
      flowRef.current = nextFlow; setFlow(nextFlow);
      await resolveFlowLink(nextFlow);
      await openLink({ kind: "authorization", id: nextFlow.flowId }, () => flowRef.current === nextFlow);
      locallyStartedFlowRef.current = nextFlow.flowId;
      await invalidate();
    } catch (cause) {
      if (nextFlow) await cancelAfterOpenFailure(nextFlow);
      setActionError(errorMessage(cause));
      throw cause;
    } finally { busyRef.current = false; setDirectBusy(false); }
  };
  const openAuthorizationUrl = async () => {
    const active = flowRef.current;
    if (!active) return;
    try {
      await resolveFlowLink(active);
      await openLink({ kind: "authorization", id: active.flowId }, () => flowRef.current === active);
      locallyStartedFlowRef.current = active.flowId;
    } catch (cause) { await cancelAfterOpenFailure(active); setActionError(errorMessage(cause)); }
  };
  const copyAuthorizationUrl = async () => {
    const active = flowRef.current;
    if (!active) return false;
    try {
      const link = await resolveFlowLink(active);
      const copied = await copyText(link.url);
      if (!copied) setActionError("无法复制授权链接。");
      return copied;
    } catch (cause) { clearFlow(); setActionError(errorMessage(cause)); return false; }
  };
  const complete = async () => {
    const active = flowRef.current;
    if (!active || active.completionMode !== "manual-response-uri" || !responseUri.trim()) return;
    const submittedUri = responseUri; setResponseUri("");
    try {
      await resolveFlowLink(active);
      await run.mutateAsync(() => client.completePluginAuth(pluginId, { flowId: active.flowId, responseUri: submittedUri }));
      if (flowRef.current === active && locallyStartedFlowRef.current === active.flowId) {
        locallyStartedFlowRef.current = null;
        await requestOAuthReturnFocus().catch(() => undefined);
      }
    } finally { clearFlow(); await invalidate(); }
  };
  const cancel = async () => {
    const active = flowRef.current; clearFlow();
    try { if (active) await run.mutateAsync(() => client.cancelPluginAuth(pluginId, { flowId: active.flowId })); }
    finally { await invalidate(); }
  };
  const signOut = async () => { clearFlow(); await run.mutateAsync(() => client.signOutPlugin(pluginId)); await invalidate(); };
  const openHelp = async (id: string) => {
    try { await openLink({ kind: "help", id }); }
    catch (cause) { setActionError(errorMessage(cause)); }
  };

  const commitConfiguration = async (plan: PluginConfigurationPlan, confirmed: boolean) => {
    if (!configuration || plan.expectedRevision !== configuration.revision || plan.expiresAt <= Date.now()) {
      setConfigurationPlan(null); throw new Error("配置已变化或确认已过期，请重新保存。");
    }
    const committed = await client.commitPluginConfiguration(pluginId, { planUuid: plan.planUuid, expectedRevision: plan.expectedRevision, confirmed });
    if (currentPluginIdRef.current !== pluginId) return null;
    setConfigurationPlan(null);
    if (plan.effectiveChanged) clearFlow();
    queryClient.setQueryData(pluginConfigurationQueryKey(pluginId), committed);
    await invalidate();
    return committed;
  };
  const saveConfiguration = async (values: Record<string, string | null>) => {
    if (!configuration || busyRef.current) throw new Error("配置尚未加载或正在处理其他请求。");
    busyRef.current = true; setDirectBusy(true); setActionError(null); setConfigurationPlan(null);
    try {
      const plan = await client.planPluginConfiguration(pluginId, { values, expectedRevision: configuration.revision });
      if (currentPluginIdRef.current !== pluginId) return null;
      if (plan.requiresConfirmation) { setConfigurationPlan(plan); return null; }
      return await commitConfiguration(plan, false);
    } catch (cause) { throw new Error(errorMessage(cause)); }
    finally { busyRef.current = false; setDirectBusy(false); }
  };
  const confirmConfiguration = async () => {
    if (!configurationPlan || busyRef.current) return null;
    busyRef.current = true; setDirectBusy(true);
    try { return await commitConfiguration(configurationPlan, true); }
    catch (cause) { throw new Error(errorMessage(cause)); }
    finally { busyRef.current = false; setDirectBusy(false); }
  };

  return {
    plugin, availablePlugins, configuration, configurationPlan, remoteAvailable,
    primaryAction: pluginAuthPrimaryAction(plugin, configuration), flow, responseUri, setResponseUri,
    busy: run.isPending || directBusy, loading: pluginsQuery.isLoading || configurationQuery.isLoading,
    error: actionError ?? (pluginsQuery.error ? errorMessage(pluginsQuery.error) : configurationQuery.error ? errorMessage(configurationQuery.error) : null),
    refresh: invalidate, enable, approve, prepareConfiguration, retry, begin, openAuthorizationUrl, complete, cancel, signOut, copyAuthorizationUrl,
    saveConfiguration, confirmConfiguration, discardConfigurationPlan: () => setConfigurationPlan(null), openHelp,
  };
}
