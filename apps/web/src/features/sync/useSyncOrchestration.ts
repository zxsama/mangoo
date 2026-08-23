import { useCallback, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { LibrarySyncStatus, SyncAllowedAction, SyncOperation, SyncProviderStatus, SyncReconnectPlan } from "@megle/core-client";
import { createCoreClient } from "../../core/client";
import { syncSummaryModel } from "./syncModel";

export const SYNC_STATUS_QUERY_KEY = ["sync", "status"] as const;

function hasActiveWork(libraries: LibrarySyncStatus[] | undefined): boolean {
  return Boolean(libraries?.some((item) =>
    ["enabling", "syncing", "restoring", "unbinding"].includes(item.lifecycle)
  ));
}

export function useSyncOrchestration() {
  const queryClient = useQueryClient();
  const [busyLibraryId, setBusyLibraryId] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const statusQuery = useQuery({
    queryKey: SYNC_STATUS_QUERY_KEY,
    queryFn: async ({ signal }) => {
      const next = await createCoreClient().getSyncStatus({ requestPriority: "background", signal });
      const current = queryClient.getQueryData<typeof next>(SYNC_STATUS_QUERY_KEY);
      return current && current.revision > next.revision ? current : next;
    },
    refetchInterval: (query) => hasActiveWork(query.state.data?.libraries) ? 5_000 : 30_000,
  });
  const refresh = useCallback(async () => { await statusQuery.refetch(); }, [statusQuery]);
  const invalidate = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: SYNC_STATUS_QUERY_KEY });
  }, [queryClient]);

  const command = useMutation({
    mutationFn: async ({ status, action }: { status: LibrarySyncStatus; action: SyncAllowedAction }) => {
      const client = createCoreClient();
      const contextRevision = (await client.getActiveLibrary()).contextRevision;
      const body = { operationUuid: crypto.randomUUID() };
      if (action === "pause") return client.pauseLibrarySync(status.libraryUuid, contextRevision, body);
      if (action === "resume") return client.resumeLibrarySync(status.libraryUuid, contextRevision, body);
      if (action === "retry") return client.retryLibrarySync(status.libraryUuid, contextRevision, body);
      if (action === "sync-now") return client.syncLibraryNow(status.libraryUuid, contextRevision, body);
      if (action === "cancel-before-catalog" && status.restoreUuid) return client.cancelSyncRestore(status.restoreUuid);
      return null;
    },
    onSettled: invalidate,
  });

  const runAction = useCallback(async (status: LibrarySyncStatus, action: SyncAllowedAction): Promise<SyncOperation | null> => {
    setBusyLibraryId(status.libraryUuid); setActionError(null);
    try {
      const result = await command.mutateAsync({ status, action });
      return result && "kind" in result ? result : null;
    }
    catch (cause) { setActionError(cause instanceof Error ? cause.message : String(cause)); return null; }
    finally { setBusyLibraryId(null); }
  }, [command]);

  const enable = useCallback(async (status: LibrarySyncStatus, provider: SyncProviderStatus) => {
    setBusyLibraryId(status.libraryUuid); setActionError(null);
    try {
      const client = createCoreClient();
      const [active, preflight] = await Promise.all([
        client.getActiveLibrary(),
        client.preflightLibrarySync(status.libraryUuid, { providerId: provider.pluginId, accountUuid: provider.accountUuid, remoteLibraryUuid: null }),
      ]);
      if (!preflight.ok || !preflight.preflightToken) throw new Error(preflight.reason);
      const operation = await client.startLibrarySyncEnable(status.libraryUuid, active.contextRevision, {
        operationUuid: crypto.randomUUID(), providerId: provider.pluginId, accountUuid: provider.accountUuid, preflightToken: preflight.preflightToken,
      });
      await invalidate(); return operation;
    } catch (cause) { setActionError(cause instanceof Error ? cause.message : String(cause)); return null; }
    finally { setBusyLibraryId(null); }
  }, [invalidate]);

  const unbind = useCallback(async (status: LibrarySyncStatus, choice: "drain-before-unbind" | "keep-local-fork") => {
    setBusyLibraryId(status.libraryUuid); setActionError(null);
    try {
      const client = createCoreClient();
      const [plan, active] = await Promise.all([client.planLibrarySyncUnbind(status.libraryUuid), client.getActiveLibrary()]);
      const operation = await client.commitLibrarySyncUnbind(status.libraryUuid, active.contextRevision, { operationUuid: crypto.randomUUID(), planUuid: plan.planUuid, choice });
      await invalidate(); return operation;
    } catch (cause) { setActionError(cause instanceof Error ? cause.message : String(cause)); return null; }
    finally { setBusyLibraryId(null); }
  }, [invalidate]);

  const previewReconnect = useCallback(async (status: LibrarySyncStatus): Promise<SyncReconnectPlan | null> => {
    if (!status.accountUuid || !status.remoteLibraryUuid) { setActionError("缺少原始远端身份，无法重新连接。"); return null; }
    setBusyLibraryId(status.libraryUuid); setActionError(null);
    try {
      return await createCoreClient().preflightLibrarySyncReconnect(status.libraryUuid, { accountUuid: status.accountUuid, remoteLibraryUuid: status.remoteLibraryUuid });
    } catch (cause) { setActionError(cause instanceof Error ? cause.message : String(cause)); return null; }
    finally { setBusyLibraryId(null); }
  }, []);

  const commitReconnect = useCallback(async (status: LibrarySyncStatus, plan: SyncReconnectPlan, choice: "merge-local-fork" | "publish-new-remote") => {
    setBusyLibraryId(status.libraryUuid); setActionError(null);
    try {
      if (choice === "merge-local-fork" && !plan.mergeAllowed) throw new Error("三方预览不允许合并原远端；请选择发布为新远端。");
      const client = createCoreClient();
      const active = await client.getActiveLibrary();
      const operation = await client.commitLibrarySyncReconnect(status.libraryUuid, active.contextRevision, { operationUuid: crypto.randomUUID(), planUuid: plan.planUuid, choice });
      await invalidate(); return operation;
    } catch (cause) { setActionError(cause instanceof Error ? cause.message : String(cause)); return null; }
    finally { setBusyLibraryId(null); }
  }, [invalidate]);

  const changeAccount = useCallback(async (pluginId: string, switchAccount: boolean) => {
    setBusyLibraryId(`account:${pluginId}`); setActionError(null);
    try {
      const client = createCoreClient();
      await client.signOutPlugin(pluginId);
      if (switchAccount) {
        const auth = await client.beginPluginAuth(pluginId);
        window.open(auth.authorizationUrl, "_blank", "noopener,noreferrer");
      }
      await invalidate();
    } catch (cause) { setActionError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusyLibraryId(null); }
  }, [invalidate]);

  const snapshot = statusQuery.data ?? null;
  return { snapshot, summary: useMemo(() => syncSummaryModel(snapshot), [snapshot]), error: actionError ?? (statusQuery.error instanceof Error ? statusQuery.error.message : null), busyLibraryId, refresh, runAction, enable, unbind, previewReconnect, commitReconnect, changeAccount };
}
