import { useCallback, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type {
  RootComparisonItem,
  RootComparisonPlan,
  RootLocationConflict,
  RootLogicalRenamePlan,
  RootOperation,
  RootPathPreflightResult,
  RootRebindPlan,
  SyncFolderRecord
} from "@megle/core-client";
import { createCoreClient } from "../../core/client";
import { syncFolderCoreErrorCode } from "./syncFolderModel";

export const syncFolderQueryKey = (libraryUuid: string) => ["sync-folders", libraryUuid] as const;

export interface RootRelocationDraft {
  operationUuid: string;
  preflight: RootPathPreflightResult;
  targetPath: string;
}

export interface RootRenameDraft {
  operationUuid: string;
  plan: RootLogicalRenamePlan;
}

export interface RootRebindDraft {
  operationUuid: string;
  plan: RootRebindPlan;
}

export interface ReenablePreview {
  plan: RootComparisonPlan;
  items: RootComparisonItem[];
  nextCursor: number | null;
}

const COMPARISON_POLL_ATTEMPTS = 8;
const COMPARISON_POLL_INTERVAL_MS = 500;

function delay(milliseconds: number) {
  return new Promise<void>((resolve) => window.setTimeout(resolve, milliseconds));
}

export function useSyncFolders(libraryUuid: string | null) {
  const queryClient = useQueryClient();
  const busyRootRef = useRef<string | null>(null);
  const currentLibraryUuidRef = useRef(libraryUuid);
  currentLibraryUuidRef.current = libraryUuid;
  const [busyRootUuid, setBusyRootUuid] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const query = useQuery({
    enabled: Boolean(libraryUuid),
    queryKey: libraryUuid ? syncFolderQueryKey(libraryUuid) : ["sync-folders", "inactive"],
    queryFn: ({ signal }) => createCoreClient().listSyncFolders(libraryUuid!, { signal }),
    refetchInterval: (state) => state.state.data?.items.some((item) => item.binding && ["materializing", "relocating"].includes(item.binding.state)) ? 2_000 : 15_000
  });

  const refresh = useCallback(async () => {
    if (libraryUuid) await queryClient.invalidateQueries({ queryKey: syncFolderQueryKey(libraryUuid) });
  }, [libraryUuid, queryClient]);

  const run = useCallback(async <T,>(rootUuid: string, action: () => Promise<T>): Promise<T | null> => {
    if (busyRootRef.current) return null;
    const operationLibraryUuid = libraryUuid;
    busyRootRef.current = rootUuid;
    setBusyRootUuid(rootUuid);
    if (currentLibraryUuidRef.current === operationLibraryUuid) setError(null);
    try {
      const result = await action();
      await refresh();
      return result;
    } catch (cause) {
      if (currentLibraryUuidRef.current === operationLibraryUuid) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
      return null;
    } finally {
      if (busyRootRef.current === rootUuid) busyRootRef.current = null;
      if (currentLibraryUuidRef.current === operationLibraryUuid) setBusyRootUuid(null);
    }
  }, [libraryUuid, refresh]);

  const command = useCallback(async (folder: SyncFolderRecord, action: "pause" | "resume") => {
    if (!libraryUuid || !folder.binding) return null;
    return run(folder.descriptor.rootUuid, async () => {
      const client = createCoreClient();
      const active = await client.getActiveLibrary();
      return client.commandSyncFolder(libraryUuid, folder.descriptor.rootUuid, action, active.contextRevision, {
        operationUuid: crypto.randomUUID(),
        expectedOperationRevision: folder.descriptor.operationRevision,
        expectedBindingGeneration: folder.binding!.bindingGeneration
      });
    });
  }, [libraryUuid, run]);

  const select = useCallback(async (folder: SyncFolderRecord, path: string) => {
    if (!libraryUuid) return null;
    return run(folder.descriptor.rootUuid, async () => {
      const client = createCoreClient();
      const item = {
        rootUuid: folder.descriptor.rootUuid,
        operationUuid: crypto.randomUUID(),
        expectedOperationRevision: folder.descriptor.operationRevision,
        expectedBindingGeneration: folder.binding?.bindingGeneration ?? null,
        selected: true,
        path
      };
      const preflight = await client.preflightSyncFolderSelection(libraryUuid, { items: [item] });
      const result = preflight.items[0];
      if (!result?.ok) throw new Error(`无法使用所选位置：${result?.reason ?? "invalid-path"}`);
      const active = await client.getActiveLibrary();
      return client.commitSyncFolderSelection(libraryUuid, active.contextRevision, { preflightToken: preflight.preflightToken, items: [{ ...item, path: result.normalizedPath }] });
    });
  }, [libraryUuid, run]);

  const stop = useCallback(async (folder: SyncFolderRecord) => {
    if (!libraryUuid || !folder.binding) return null;
    return run(folder.descriptor.rootUuid, async () => {
      const client = createCoreClient();
      const operationUuid = crypto.randomUUID();
      const fence = { operationUuid, expectedOperationRevision: folder.descriptor.operationRevision, expectedBindingGeneration: folder.binding!.bindingGeneration };
      const [plan, active] = await Promise.all([client.preflightSyncFolderStop(libraryUuid, folder.descriptor.rootUuid, fence), client.getActiveLibrary()]);
      return client.commitSyncFolderStop(libraryUuid, folder.descriptor.rootUuid, active.contextRevision, { ...fence, planUuid: plan.planUuid });
    });
  }, [libraryUuid, run]);

  const preflightRelocation = useCallback(async (folder: SyncFolderRecord, targetPath: string): Promise<RootRelocationDraft | null> => {
    if (!libraryUuid || !folder.binding) return null;
    const operationUuid = crypto.randomUUID();
    const preflight = await run(folder.descriptor.rootUuid, () => createCoreClient().preflightSyncFolderRelocation(libraryUuid, folder.descriptor.rootUuid, {
      operationUuid, expectedOperationRevision: folder.descriptor.operationRevision,
      expectedBindingGeneration: folder.binding!.bindingGeneration, mode: "explicit-move", targetPath
    }));
    return preflight ? { operationUuid, preflight, targetPath } : null;
  }, [libraryUuid, run]);

  const preflightRename = useCallback(async (folder: SyncFolderRecord, requestedName: string): Promise<RootRenameDraft | null> => {
    if (!libraryUuid) return null;
    const operationUuid = crypto.randomUUID();
    const plan = await run(folder.descriptor.rootUuid, () => createCoreClient().preflightSyncFolderRename(libraryUuid, folder.descriptor.rootUuid, {
      operationUuid,
      expectedOperationRevision: folder.descriptor.operationRevision,
      expectedBindingGeneration: folder.binding?.bindingGeneration ?? null,
      requestedName,
      expectedLocationHeads: folder.locationHeads ?? [],
      supersedes: folder.locationHeads ?? []
    }));
    return plan ? { operationUuid, plan } : null;
  }, [libraryUuid, run]);

  const commitRename = useCallback(async (folder: SyncFolderRecord, draft: RootRenameDraft): Promise<RootOperation | null> => {
    if (!libraryUuid) return null;
    return run(folder.descriptor.rootUuid, async () => {
      const active = await createCoreClient().getActiveLibrary();
      return createCoreClient().commitSyncFolderRename(libraryUuid, folder.descriptor.rootUuid, active.contextRevision, {
        operationUuid: draft.operationUuid,
        expectedOperationRevision: folder.descriptor.operationRevision,
        expectedBindingGeneration: folder.binding?.bindingGeneration ?? null,
        requestedName: draft.plan.requestedName,
        expectedLocationHeads: draft.plan.expectedLocationHeads,
        supersedes: draft.plan.supersedes,
        planUuid: draft.plan.planUuid
      });
    });
  }, [libraryUuid, run]);

  const preflightRebind = useCallback(async (folder: SyncFolderRecord, path: string): Promise<RootRebindDraft | null> => {
    if (!libraryUuid || !folder.rebindEvidence) return null;
    const operationUuid = crypto.randomUUID();
    const evidence = folder.rebindEvidence;
    const plan = await run(folder.descriptor.rootUuid, () => createCoreClient().preflightSyncFolderRebind(libraryUuid, folder.descriptor.rootUuid, {
      operationUuid,
      expectedOperationRevision: folder.descriptor.operationRevision,
      expectedBindingGeneration: folder.binding?.bindingGeneration ?? null,
      path,
      markerIdentity: evidence.markerIdentity,
      priorCutoverSnapshotUuid: evidence.priorCutoverSnapshotUuid
    }));
    return plan ? { operationUuid, plan } : null;
  }, [libraryUuid, run]);

  const commitRebind = useCallback(async (folder: SyncFolderRecord, draft: RootRebindDraft): Promise<RootOperation | null> => {
    if (!libraryUuid) return null;
    return run(folder.descriptor.rootUuid, async () => {
      const active = await createCoreClient().getActiveLibrary();
      return createCoreClient().commitSyncFolderRebind(libraryUuid, folder.descriptor.rootUuid, active.contextRevision, {
        operationUuid: draft.operationUuid,
        expectedOperationRevision: folder.descriptor.operationRevision,
        expectedBindingGeneration: folder.binding?.bindingGeneration ?? null,
        planUuid: draft.plan.planUuid
      });
    });
  }, [libraryUuid, run]);

  const commitRelocation = useCallback(async (folder: SyncFolderRecord, draft: RootRelocationDraft): Promise<RootOperation | null> => {
    if (!libraryUuid || !folder.binding || !draft.preflight.preflightToken) return null;
    return run(folder.descriptor.rootUuid, async () => {
      const active = await createCoreClient().getActiveLibrary();
      return createCoreClient().startSyncFolderRelocation(libraryUuid, folder.descriptor.rootUuid, active.contextRevision, {
        operationUuid: draft.operationUuid, expectedOperationRevision: folder.descriptor.operationRevision,
        expectedBindingGeneration: folder.binding!.bindingGeneration, mode: "explicit-move", targetPath: draft.targetPath,
        preflightToken: draft.preflight.preflightToken!, desiredMaterializationFingerprint: folder.descriptor.materializationFingerprint
      });
    });
  }, [libraryUuid, run]);

  const readComparison = useCallback(async (folder: SyncFolderRecord, initialPlan: RootComparisonPlan): Promise<ReenablePreview> => {
    if (!libraryUuid) throw new Error("当前没有活动资源库");
    const client = createCoreClient();
    let plan = initialPlan;
    for (let attempt = 0; plan.status === "comparing" && attempt < COMPARISON_POLL_ATTEMPTS; attempt += 1) {
      await delay(COMPARISON_POLL_INTERVAL_MS);
      plan = await client.getSyncFolderComparison(libraryUuid, folder.descriptor.rootUuid, plan.planUuid);
    }
    const page = await client.listSyncFolderComparisonItems(libraryUuid, folder.descriptor.rootUuid, plan.planUuid, { limit: 100 });
    return { plan, items: page.items, nextCursor: page.nextCursor };
  }, [libraryUuid]);

  const preflightReenable = useCallback(async (folder: SyncFolderRecord): Promise<ReenablePreview | null> => {
    if (!libraryUuid || !folder.binding) return null;
    return run(folder.descriptor.rootUuid, async () => {
      const client = createCoreClient();
      const fence = { operationUuid: crypto.randomUUID(), expectedOperationRevision: folder.descriptor.operationRevision, expectedBindingGeneration: folder.binding!.bindingGeneration };
      const plan = await client.preflightSyncFolderReenable(libraryUuid, folder.descriptor.rootUuid, fence);
      return readComparison(folder, plan);
    });
  }, [libraryUuid, readComparison, run]);

  const refreshComparison = useCallback(async (folder: SyncFolderRecord, preview: ReenablePreview): Promise<ReenablePreview | null> => {
    if (!libraryUuid) return null;
    return run(folder.descriptor.rootUuid, async () => {
      const plan = await createCoreClient().getSyncFolderComparison(libraryUuid, folder.descriptor.rootUuid, preview.plan.planUuid);
      return readComparison(folder, plan);
    });
  }, [libraryUuid, readComparison, run]);

  const loadMoreComparisonItems = useCallback(async (folder: SyncFolderRecord, preview: ReenablePreview): Promise<ReenablePreview | null> => {
    if (!libraryUuid || preview.nextCursor === null) return preview;
    return run(folder.descriptor.rootUuid, async () => {
      const page = await createCoreClient().listSyncFolderComparisonItems(libraryUuid, folder.descriptor.rootUuid, preview.plan.planUuid, { cursor: preview.nextCursor ?? undefined, limit: 100 });
      const existing = new Set(preview.items.map((item) => item.itemOrdinal));
      return { ...preview, items: [...preview.items, ...page.items.filter((item) => !existing.has(item.itemOrdinal))], nextCursor: page.nextCursor };
    });
  }, [libraryUuid, run]);

  const commitReenable = useCallback(async (folder: SyncFolderRecord, plan: RootComparisonPlan, mode: "merge-local-fork" | "publish-as-new"): Promise<{ operation: RootOperation | null; refreshedPreview: ReenablePreview | null }> => {
    if (!libraryUuid || !folder.binding) return { operation: null, refreshedPreview: null };
    const result = await run(folder.descriptor.rootUuid, async () => {
      const client = createCoreClient();
      const active = await client.getActiveLibrary();
      try {
        const operation = await client.commitSyncFolderReenable(libraryUuid, folder.descriptor.rootUuid, active.contextRevision, {
          operationUuid: plan.operationUuid, expectedOperationRevision: folder.descriptor.operationRevision,
          expectedBindingGeneration: folder.binding!.bindingGeneration, planUuid: plan.planUuid, mode
        });
        return { operation, refreshedPreview: null };
      } catch (cause) {
        if (!["comparison-expired", "stale-operation-revision", "stale-binding-generation"].includes(syncFolderCoreErrorCode(cause) ?? "")) throw cause;
        const latestFolder = await client.getSyncFolder(libraryUuid, folder.descriptor.rootUuid);
        if (!latestFolder.binding) throw cause;
        const nextPlan = await client.preflightSyncFolderReenable(libraryUuid, folder.descriptor.rootUuid, {
          operationUuid: crypto.randomUUID(), expectedOperationRevision: latestFolder.descriptor.operationRevision,
          expectedBindingGeneration: latestFolder.binding.bindingGeneration
        });
        return { operation: null, refreshedPreview: await readComparison(latestFolder, nextPlan) };
      }
    });
    return result ?? { operation: null, refreshedPreview: null };
  }, [libraryUuid, readComparison, run]);

  const getRelocation = useCallback(async (folder: SyncFolderRecord, operationUuid: string) => {
    if (!libraryUuid) return null;
    return createCoreClient().getSyncFolderRelocation(libraryUuid, folder.descriptor.rootUuid, operationUuid);
  }, [libraryUuid]);

  const commandRelocation = useCallback(async (folder: SyncFolderRecord, operation: RootOperation, action: "cancel" | "retry") => {
    if (!libraryUuid || !folder.binding) return null;
    return run(folder.descriptor.rootUuid, async () => {
      const active = await createCoreClient().getActiveLibrary();
      return createCoreClient().commandSyncFolderRelocation(libraryUuid, folder.descriptor.rootUuid, operation.operationUuid, action, active.contextRevision, {
        operationUuid: operation.operationUuid, expectedOperationRevision: operation.operationRevision,
        expectedBindingGeneration: folder.binding!.bindingGeneration
      });
    });
  }, [libraryUuid, run]);

  const resolveComparisonItem = useCallback(async (folder: SyncFolderRecord, plan: RootComparisonPlan, item: RootComparisonItem, resolution: "keep-local" | "keep-remote" | "keep-both") => {
    if (!libraryUuid || !folder.binding) return null;
    return run(folder.descriptor.rootUuid, async () => {
      const active = await createCoreClient().getActiveLibrary();
      return createCoreClient().resolveSyncFolderComparisonItem(libraryUuid, folder.descriptor.rootUuid, plan.planUuid, item.itemOrdinal, active.contextRevision, {
        operationUuid: plan.operationUuid, expectedOperationRevision: folder.descriptor.operationRevision,
        expectedBindingGeneration: folder.binding!.bindingGeneration, resolution
      });
    });
  }, [libraryUuid, run]);

  const listConflicts = useCallback(async (folder: SyncFolderRecord): Promise<RootLocationConflict[] | null> => {
    if (!libraryUuid) return null;
    return run(folder.descriptor.rootUuid, async () => {
      const client = createCoreClient();
      const items: RootLocationConflict[] = [];
      let cursor: string | undefined;
      for (let pageNumber = 0; pageNumber < 20; pageNumber += 1) {
        const page = await client.listSyncFolderConflicts(libraryUuid, folder.descriptor.rootUuid, { cursor, limit: 100 });
        items.push(...page.items);
        if (!page.nextCursor) break;
        cursor = page.nextCursor;
      }
      return items;
    });
  }, [libraryUuid, run]);

  const resolveConflict = useCallback(async (folder: SyncFolderRecord, conflict: RootLocationConflict, retainedHead: string) => {
    if (!libraryUuid) return null;
    return run(folder.descriptor.rootUuid, async () => {
      const active = await createCoreClient().getActiveLibrary();
      return createCoreClient().resolveSyncFolderConflict(libraryUuid, folder.descriptor.rootUuid, conflict.conflictUuid, active.contextRevision, {
        operationUuid: crypto.randomUUID(), expectedOperationRevision: folder.descriptor.operationRevision,
        expectedBindingGeneration: folder.binding?.bindingGeneration ?? null,
        expectedLocationHeads: conflict.locationHeads, supersedes: conflict.locationHeads.filter((head) => head !== retainedHead)
      });
    });
  }, [libraryUuid, run]);

  return useMemo(() => ({ folders: query.data?.items ?? [], revision: query.data?.revision ?? null, loading: query.isLoading, error: error ?? (query.error instanceof Error ? query.error.message : null), busyRootUuid, refresh, command, select, stop, preflightRename, commitRename, preflightRebind, commitRebind, preflightRelocation, commitRelocation, getRelocation, commandRelocation, preflightReenable, refreshComparison, loadMoreComparisonItems, commitReenable, resolveComparisonItem, listConflicts, resolveConflict }), [busyRootUuid, command, commandRelocation, commitRebind, commitReenable, commitRelocation, commitRename, error, getRelocation, listConflicts, loadMoreComparisonItems, preflightRebind, preflightReenable, preflightRelocation, preflightRename, query.data, query.error, query.isLoading, refresh, refreshComparison, resolveComparisonItem, resolveConflict, select, stop]);
}
