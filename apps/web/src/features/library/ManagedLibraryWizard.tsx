import { useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import type {
  RemoteLibraryRoot,
  RemoteLibrarySummary,
  SyncRestorePlan,
  SyncRestorePreflight,
  SyncRestoreStartItem,
} from "@megle/core-client";
import {
  createLibrary,
  pickNewLibraryPath,
  pickSyncFolder,
  preflightLibrary,
} from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  ModalSurface,
  OverlayDialogActions,
  OverlayDialogBody,
  OverlayDialogHeader,
  OverlayField,
  OverlayInlineStatus,
} from "../../design/overlays";
import {
  DEFAULT_LIBRARY_COLOR,
  DEFAULT_LIBRARY_ICON,
  LibraryIconPicker,
} from "./libraryGlyph";
import { createCoreClient } from "../../core/client";
import { OneDriveAccountSetupContent } from "../plugins/OneDriveAccountSetup";
import { usePluginAuth } from "../plugins/usePluginAuth";

export interface ManagedLibraryWizardProps {
  firstRun?: boolean;
  initialMode?: "create" | "restore";
  onClose: () => void;
  onCreated: () => void;
  onFailure: (cause: unknown) => void;
  open: boolean;
  restoreFocusRef?: RefObject<HTMLElement | null>;
}

interface RestoreRootInventory {
  inventoryFence: string;
  roots: Array<RemoteLibraryRoot & { selected: boolean; targetPath: string | null }>;
}

function proposedRestoreRootPath(placementBase: string, materializedName: string): string {
  return `${placementBase.replace(/[\\/]+$/u, "")}\\${materializedName}`;
}

function formatRemoteSize(bytes: number | null) {
  if (bytes == null) return null;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MiB`;
  return `${(bytes / 1024 ** 3).toFixed(1)} GiB`;
}

export function ManagedLibraryWizard({
  firstRun = false,
  initialMode = "create",
  onClose,
  onCreated,
  onFailure,
  open,
  restoreFocusRef,
}: ManagedLibraryWizardProps) {
  const [mode, setMode] = useState<"create" | "restore">(initialMode);
  const [name, setName] = useState("Library");
  const [icon, setIcon] = useState(DEFAULT_LIBRARY_ICON);
  const [color, setColor] = useState(DEFAULT_LIBRARY_COLOR);
  const [managedRoot, setManagedRoot] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [enableSync, setEnableSync] = useState(false);
  const [remoteLibraries, setRemoteLibraries] = useState<RemoteLibrarySummary[]>([]);
  const [remoteLoading, setRemoteLoading] = useState(false);
  const [selectedRemoteIds, setSelectedRemoteIds] = useState<Set<string>>(new Set());
  const [restoreRoots, setRestoreRoots] = useState<Record<string, string>>({});
  const [restoreRootInventories, setRestoreRootInventories] = useState<Record<string, RestoreRootInventory>>({});
  const [restoreItems, setRestoreItems] = useState<SyncRestoreStartItem[] | null>(null);
  const [restorePreflight, setRestorePreflight] = useState<SyncRestorePreflight | null>(null);
  const [restorePlans, setRestorePlans] = useState<SyncRestorePlan[]>([]);
  const nameRef = useRef<HTMLInputElement>(null);
  const restoreModeRef = useRef<HTMLButtonElement>(null);
  const auth = usePluginAuth();
  const authResumeRef = useRef<{
    contextRevision: number;
    mode: "create" | "restore";
    selectedRemoteIds: string[];
  } | null>(null);
  const [authResumePending, setAuthResumePending] = useState(false);
  const previousAuthStateRef = useRef(auth.plugin?.authState);
  const queryClient = useQueryClient();

  useEffect(() => {
    if (!open) return;
    setError(null);
    setMode(initialMode);
    setRestorePreflight(null);
    setRestoreItems(null);
    setRestorePlans([]);
    authResumeRef.current = null;
    setAuthResumePending(false);
  }, [initialMode, open]);

  async function prepareAuthResume() {
    const active = await createCoreClient().getActiveLibrary();
    authResumeRef.current = {
      contextRevision: active.contextRevision,
      mode,
      selectedRemoteIds: [...selectedRemoteIds],
    };
    setAuthResumePending(true);
  }

  async function resumeAfterAuthentication() {
    const intent = authResumeRef.current;
    authResumeRef.current = null;
    setAuthResumePending(false);
    if (!intent || !open || intent.mode !== mode) return;
    const active = await createCoreClient().getActiveLibrary();
    if (active.contextRevision !== intent.contextRevision) {
      setError("资源库上下文已变化，请重新选择同步或恢复目标。");
      return;
    }
    setSelectedRemoteIds(new Set(intent.selectedRemoteIds));
    if (mode === "restore") {
      setRemoteLoading(true);
      try {
        const response = await createCoreClient().listRemoteLibraries();
        setRemoteLibraries(response.items);
      } finally {
        setRemoteLoading(false);
      }
    }
    (mode === "create" ? nameRef.current : restoreModeRef.current)?.focus();
  }

  useEffect(() => {
    const previous = previousAuthStateRef.current;
    const current = auth.plugin?.authState;
    previousAuthStateRef.current = current;
    if (previous !== "authenticated" && current === "authenticated" && authResumeRef.current) {
      void resumeAfterAuthentication();
    }
  }, [auth.plugin?.authState]);

  useEffect(() => {
    if (!authResumePending) return;
    const state = auth.plugin?.authState;
    if (!auth.plugin || state === "error" || state === "expired") {
      authResumeRef.current = null;
      setAuthResumePending(false);
    }
  }, [auth.plugin, auth.plugin?.authState, authResumePending]);

  useEffect(() => {
    if (!authResumePending || !open) return;
    let disposed = false;
    const verifyContext = async () => {
      const intent = authResumeRef.current;
      if (!intent) return;
      try {
        const active = await createCoreClient().getActiveLibrary({ requestPriority: "background" });
        if (!disposed && active.contextRevision !== intent.contextRevision) {
          authResumeRef.current = null;
          setAuthResumePending(false);
          setError("资源库上下文已变化，请重新选择同步或恢复目标。");
        }
      } catch {
        // A transient context read must not destroy an otherwise recoverable intent.
      }
    };
    const timer = window.setInterval(() => void verifyContext(), 1_000);
    void verifyContext();
    return () => { disposed = true; window.clearInterval(timer); };
  }, [authResumePending, open]);

  useEffect(() => {
    if (!open || mode !== "restore" || auth.plugin?.authState !== "authenticated") return;
    const controller = new AbortController();
    setRemoteLoading(true);
    void createCoreClient().listRemoteLibraries({ signal: controller.signal })
      .then((response) => setRemoteLibraries(response.items))
      .catch((cause) => {
        if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => {
        if (!controller.signal.aborted) setRemoteLoading(false);
      });
    return () => controller.abort();
  }, [auth.plugin?.authState, mode, open]);

  const selectedRemote = useMemo(
    () => remoteLibraries.filter((remote) => selectedRemoteIds.has(remote.libraryUuid)),
    [remoteLibraries, selectedRemoteIds],
  );
  const restoreQueries = useQueries({
    queries: restorePlans.map((plan) => ({
      enabled: open,
      initialData: plan,
      queryFn: ({ signal }: { signal: AbortSignal }) => createCoreClient().getSyncRestore(plan.restoreUuid, { signal }),
      queryKey: ["sync", "restore", plan.restoreUuid] as const,
      refetchInterval: (query: { state: { data?: SyncRestorePlan } }) =>
        query.state.data && ["complete", "cancelled", "failed"].includes(query.state.data.state) ? false : 2_000,
    })),
  });
  const visibleRestorePlans = restorePlans.map((plan, index) => restoreQueries[index]?.data ?? plan);

  async function chooseRoot() {
    try {
      const selected = await pickNewLibraryPath(name.trim() || "Library");
      if (selected) setManagedRoot(selected);
    } catch (cause) {
      setError(String(cause));
      onFailure(cause);
    }
  }

  async function chooseRestoreRoot(remote: RemoteLibrarySummary) {
    try {
      const selected = await pickSyncFolder({
        libraryUuid: remote.libraryUuid,
        rootUuid: null,
        mode: "materialize-remote",
        suggestedPath: restoreRoots[remote.libraryUuid] ?? null,
        dialogTitle: `选择“${remote.name}”的本机位置`
      });
      if (selected) {
        setRestoreRoots((current) => ({ ...current, [remote.libraryUuid]: selected }));
        setRestoreRootInventories((current) => {
          const inventory = current[remote.libraryUuid];
          if (!inventory) return current;
          return {
            ...current,
            [remote.libraryUuid]: {
              ...inventory,
              roots: inventory.roots.map((root) => ({
                ...root,
                targetPath: root.selected
                  ? proposedRestoreRootPath(selected, root.materializedName)
                  : null,
              })),
            },
          };
        });
        setRestorePreflight(null);
        setRestoreItems(null);
      }
    } catch (cause) {
      setError(String(cause));
      onFailure(cause);
    }
  }

  async function loadRestoreRootInventory(remote: RemoteLibrarySummary) {
    const client = createCoreClient();
    const roots: RemoteLibraryRoot[] = [];
    let cursor: string | undefined;
    let inventoryFence: string | null = null;
    do {
      const page = await client.listRemoteLibraryRoots(
        remote.accountUuid,
        remote.libraryUuid,
        { cursor, limit: 200 },
      );
      if (inventoryFence !== null && inventoryFence !== page.inventoryFence) {
        throw new Error(`“${remote.name}”的远端文件夹清单已变化，请重试。`);
      }
      inventoryFence = page.inventoryFence;
      roots.push(...page.items);
      cursor = page.nextCursor ?? undefined;
    } while (cursor);
    const placementBase = restoreRoots[remote.libraryUuid] ?? null;
    const nextInventory: RestoreRootInventory = {
      inventoryFence: inventoryFence ?? "",
      roots: roots.map((root) => ({
        ...root,
        selected: root.compatible,
        targetPath: root.compatible && placementBase
          ? proposedRestoreRootPath(placementBase, root.materializedName)
          : null,
      })),
    };
    setRestoreRootInventories((current) => ({
      ...current,
      [remote.libraryUuid]: nextInventory,
    }));
    return nextInventory;
  }

  async function chooseRestoreSyncRoot(remote: RemoteLibrarySummary, root: RemoteLibraryRoot) {
    try {
      const selected = await pickSyncFolder({
        libraryUuid: remote.libraryUuid,
        rootUuid: root.rootUuid,
        mode: "materialize-remote",
        suggestedPath: restoreRootInventories[remote.libraryUuid]?.roots.find((item) => item.rootUuid === root.rootUuid)?.targetPath ?? null,
        dialogTitle: `选择“${root.requestedName}”的同步位置`,
      });
      if (!selected) return;
      setRestoreRootInventories((current) => ({
        ...current,
        [remote.libraryUuid]: {
          ...current[remote.libraryUuid]!,
          roots: current[remote.libraryUuid]!.roots.map((item) => item.rootUuid === root.rootUuid
            ? { ...item, selected: true, targetPath: selected }
            : item),
        },
      }));
      setRestorePreflight(null);
      setRestoreItems(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      onFailure(cause);
    }
  }

  async function submit() {
    if (busy) return;
    const nextName = name.trim();
    if (!nextName) {
      setError("请输入资源库名称");
      return;
    }
    if (!managedRoot) {
      setError("请选择资源库根目录");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const checked = await preflightLibrary({ name: nextName, managedRoot });
      if (!checked.ok) throw new Error(`无法使用该目录：${checked.reason}`);
      const created = await createLibrary({
        activate: true,
        color,
        icon,
        initialMode: "local-only",
        managedRoot: checked.normalizedRoot,
        name: nextName,
        preflightToken: checked.preflightToken,
      });
      if (enableSync && created.library) {
        const client = createCoreClient();
        const snapshot = await client.getSyncStatus();
        const provider = snapshot.providers.find((item) => item.runtimeState === "ready" && item.authState === "authenticated");
        if (!provider) throw new Error("本地资源库已创建，但 OneDrive 尚未登录或 Provider 未就绪。可稍后在设置中启用同步。");
        const checkedSync = await client.preflightLibrarySync(created.library.libraryUuid, {
          providerId: provider.pluginId,
          accountUuid: provider.accountUuid,
          remoteLibraryUuid: null,
        });
        if (!checkedSync.ok || !checkedSync.preflightToken) throw new Error(`本地资源库已创建，但同步预检失败：${checkedSync.reason}`);
        await client.startLibrarySyncEnable(created.library.libraryUuid, created.library.contextRevision, {
          operationUuid: crypto.randomUUID(),
          providerId: provider.pluginId,
          accountUuid: provider.accountUuid,
          preflightToken: checkedSync.preflightToken,
        });
      }
      onCreated();
      onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      onFailure(cause);
    } finally {
      setBusy(false);
    }
  }

  async function submitRestore() {
    if (busy) return;
    if (selectedRemote.length === 0) {
      setError("请至少选择一个远端资源库");
      return;
    }
    const missingRoot = selectedRemote.find((remote) => !restoreRoots[remote.libraryUuid]);
    if (missingRoot) {
      setError(`请为“${missingRoot.name}”选择本机根目录`);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const items: SyncRestoreStartItem[] = [];
      for (const remote of selectedRemote) {
        const checked = await preflightLibrary({ name: remote.name, managedRoot: restoreRoots[remote.libraryUuid] });
        if (!checked.ok) throw new Error(`“${remote.name}”无法使用该目录：${checked.reason}`);
        const inventory = restoreRootInventories[remote.libraryUuid]
          ?? await loadRestoreRootInventory(remote);
        const missingTarget = inventory.roots.find((root) => root.selected && !root.targetPath);
        if (missingTarget) throw new Error(`请为“${missingTarget.requestedName}”选择同步位置。`);
        if (inventory.roots.length === 0) {
          items.push({
            restoreMode: "legacy-library",
            accountUuid: remote.accountUuid,
            libraryUuid: remote.libraryUuid,
            name: remote.name,
            managedRoot: checked.normalizedRoot,
          });
        } else {
          items.push({
            restoreMode: "root-aware",
            accountUuid: remote.accountUuid,
            libraryUuid: remote.libraryUuid,
            placementBase: checked.normalizedRoot,
            inventoryFence: inventory.inventoryFence,
            roots: inventory.roots.map((root) => ({
              rootUuid: root.rootUuid,
              expectedLogicalRevision: root.logicalRevision,
              expectedMaterializationFingerprint: root.materializationFingerprint,
              selected: root.selected,
              ...(root.selected ? { targetPath: root.targetPath! } : {}),
            })),
          });
        }
      }
      const preflight = await createCoreClient().preflightSyncRestore({ items });
      setRestoreItems(items);
      setRestorePreflight(preflight);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      onFailure(cause);
    } finally {
      setBusy(false);
    }
  }

  async function confirmRestore() {
    if (!restorePreflight?.preflightToken || !restoreItems || busy) return;
    setBusy(true); setError(null);
    try {
      const response = await createCoreClient().startSyncRestore({ operationUuid: crypto.randomUUID(), preflightToken: restorePreflight.preflightToken, items: restoreItems });
      setRestorePlans(response.items);
      onCreated();
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); onFailure(cause); }
    finally { setBusy(false); }
  }

  async function changeRestoreState(plan: SyncRestorePlan, action: "pause" | "resume" | "cancel") {
    if (busy) return;
    setBusy(true); setError(null);
    try {
      const client = createCoreClient();
      const next = action === "pause"
        ? await client.pauseSyncRestore(plan.restoreUuid)
        : action === "resume"
          ? await client.resumeSyncRestore(plan.restoreUuid)
          : await client.cancelSyncRestore(plan.restoreUuid);
      setRestorePlans((current) => current.map((item) => item.restoreUuid === next.restoreUuid ? next : item));
      queryClient.setQueryData(["sync", "restore", next.restoreUuid], next);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      onFailure(cause);
    } finally {
      setBusy(false);
    }
  }

  return (
    <ModalSurface
      ariaDescribedBy="managed-library-wizard-description"
      ariaLabelledBy="managed-library-wizard-title"
      busy={busy}
      className="managed-library-wizard"
      id="managed-library-wizard"
      initialFocusRef={mode === "create" ? nameRef : restoreModeRef}
      onDismiss={() => {
        if (!busy) onClose();
      }}
      open={open}
      restoreFocusRef={restoreFocusRef}
      role="dialog"
      rootId="managed-library"
    >
      <OverlayDialogHeader
        description={mode === "restore" ? "选择一个或多个 OneDrive 资源库，并为每个资源库指定本机根目录。" : firstRun ? "选择固定根目录，创建此设备上的第一个资源库。" : "先创建本机资源库；同步可选且不会阻塞本地使用。"}
        descriptionId="managed-library-wizard-description"
        title={firstRun ? "开始使用 Megle" : mode === "restore" ? "从 OneDrive 恢复" : "创建资源库"}
        titleId="managed-library-wizard-title"
      />
      <OverlayDialogBody className="managed-library-wizard-body" data-overlay-scroll-container>
        <div aria-label="资源库来源" className="managed-library-mode" role="tablist">
          <LiquidGlassButton active={mode === "create"} aria-selected={mode === "create"} onClick={() => { authResumeRef.current = null; setAuthResumePending(false); setMode("create"); setRestorePreflight(null); setRestoreItems(null); }} role="tab" type="button">新建本机资源库</LiquidGlassButton>
          <LiquidGlassButton active={mode === "restore"} aria-selected={mode === "restore"} onClick={() => { authResumeRef.current = null; setAuthResumePending(false); setMode("restore"); setRestorePreflight(null); setRestoreItems(null); }} ref={restoreModeRef} role="tab" type="button">从 OneDrive 恢复</LiquidGlassButton>
        </div>
        {auth.plugin?.authState !== "authenticated" && (mode === "restore" || enableSync) ? (
          <section className="managed-library-auth-required">
            <p>连接 OneDrive 后会自动回到此流程，并保留当前模式和选择。</p>
            <OneDriveAccountSetupContent
              compact
              controller={auth}
              onBeforeBegin={prepareAuthResume}
              onCancelled={() => { authResumeRef.current = null; setAuthResumePending(false); }}
              overlayRootId="managed-library"
            />
          </section>
        ) : null}
        {mode === "create" ? <section className="managed-library-form-section">
          <OverlayField
            disabled={busy}
            inputRef={nameRef}
            label="名称"
            onChange={(event) => setName(event.target.value)}
            value={name}
          />
          <div className="managed-library-icon-field">
            <span>图标与颜色</span>
            <LibraryIconPicker
              color={color}
              icon={icon}
              onPick={(nextIcon, nextColor) => {
                setIcon(nextIcon);
                setColor(nextColor);
              }}
              presentation="overlay"
            />
          </div>
          <div className="managed-library-root-field">
            <span className="managed-library-field-label">本机根目录</span>
            <div className="managed-library-wizard-root">
              <span>{managedRoot ?? "尚未选择目录"}</span>
              <LiquidGlassButton disabled={busy} onClick={() => void chooseRoot()} type="button">
                选择目录…
              </LiquidGlassButton>
            </div>
          </div>
          <label className="managed-library-enable-sync">
            <input checked={enableSync} onChange={(event) => setEnableSync(event.target.checked)} type="checkbox" />
            <span>创建后启用 OneDrive 同步（登录或网络失败不会删除本机资源库）</span>
          </label>
        </section> : <section aria-busy={remoteLoading} className="managed-library-remote-list" role="group">
          {remoteLoading ? <p role="status">正在发现远端资源库…</p> : null}
          {!remoteLoading && remoteLibraries.length === 0 ? <p role="status">{auth.plugin?.authState === "authenticated" ? "当前账户没有可恢复的远端资源库。" : "请先连接 OneDrive 以发现可恢复的资源库。"}</p> : null}
          {remoteLibraries.map((remote) => {
            const selected = selectedRemoteIds.has(remote.libraryUuid);
            const remoteDetails = [
              remote.mediaCount == null ? null : `${remote.mediaCount.toLocaleString()} 项`,
              formatRemoteSize(remote.logicalBytes),
              remote.lastSyncedAt == null ? null : `同步于 ${new Date(remote.lastSyncedAt).toLocaleString()}`,
            ].filter(Boolean).join(" · ");
            return <div className="managed-library-remote-card" key={`${remote.accountUuid}:${remote.libraryUuid}`}>
              <label>
                <input checked={selected} disabled={!remote.compatible || remote.boundLocally} onChange={(event) => setSelectedRemoteIds((current) => {
                  const next = new Set(current);
                  if (event.target.checked) {
                    next.add(remote.libraryUuid);
                    if (!restoreRootInventories[remote.libraryUuid]) {
                      void loadRestoreRootInventory(remote).catch((cause) => {
                        setError(cause instanceof Error ? cause.message : String(cause));
                        onFailure(cause);
                      });
                    }
                  } else next.delete(remote.libraryUuid);
                  setRestorePreflight(null);
                  setRestoreItems(null);
                  return next;
                })} type="checkbox" />
                <span><strong>{remote.name}</strong><small>{remote.boundLocally ? "已在本机绑定" : remote.compatible ? "可恢复" : remote.errorCode ?? "描述损坏或版本不兼容"}</small>{remoteDetails ? <small>{remoteDetails}</small> : null}</span>
              </label>
              {selected ? <><div className="managed-library-wizard-root"><span>{restoreRoots[remote.libraryUuid] ?? "尚未选择资源库放置目录"}</span><LiquidGlassButton disabled={busy} onClick={() => void chooseRestoreRoot(remote)} type="button">选择放置目录…</LiquidGlassButton></div>{restoreRootInventories[remote.libraryUuid]?.roots.map((root) => <div className="managed-library-wizard-root" key={root.rootUuid}><label><input checked={root.selected} disabled={!root.compatible || busy} onChange={(event) => { const checked = event.target.checked; setRestoreRootInventories((current) => ({ ...current, [remote.libraryUuid]: { ...current[remote.libraryUuid]!, roots: current[remote.libraryUuid]!.roots.map((item) => item.rootUuid === root.rootUuid ? { ...item, selected: checked, targetPath: checked ? item.targetPath ?? (restoreRoots[remote.libraryUuid] ? proposedRestoreRootPath(restoreRoots[remote.libraryUuid], item.materializedName) : null) : null } : item) } })); setRestorePreflight(null); setRestoreItems(null); }} type="checkbox" /><span>{root.requestedName}</span></label><span>{root.selected ? root.targetPath ?? "尚未选择同步位置" : "仅保留远端清单"}</span>{root.selected ? <LiquidGlassButton disabled={busy} onClick={() => void chooseRestoreSyncRoot(remote, root)} type="button">选择位置…</LiquidGlassButton> : null}</div>)}</> : null}
            </div>;
          })}
        </section>}
        {mode === "restore" && restorePreflight ? <div className="managed-library-restore-confirm" role="status">
          <strong>{restorePreflight.ok ? "恢复预检已通过" : "恢复预检需要调整"}</strong>
          <span>{restorePreflight.totalItems} 个资源库 · 预计占用 {formatRemoteSize(restorePreflight.totalBytes)}</span>
          <small>{restorePreflight.ok ? "确认后才会创建本机资源库和启动恢复任务。" : "请按下方逐项检查容量与目录选择。"}</small>
          <ul className="managed-library-restore-preflight-roots">
            {restorePreflight.rootResults.map((result) => <li key={`${result.libraryUuid}:${result.rootUuid}`}><span>{result.rootUuid.slice(0, 8)} · {result.selected ? result.normalizedTargetPath ?? "未提供位置" : "此设备不放置"}</span><span>{result.ok ? `需要 ${formatRemoteSize(result.requiredBytes)}` : result.reason ?? "预检失败"}</span></li>)}
          </ul>
          {restorePreflight.volumeResults.length ? <ul aria-label="分卷容量检查" className="managed-library-restore-preflight-volumes">
            {restorePreflight.volumeResults.map((volume) => <li key={volume.volumeKey}><span>{volume.volumeKey}</span><span>需要 {formatRemoteSize(volume.requiredBytes)} · 可用 {volume.availableBytes == null ? "未知" : formatRemoteSize(volume.availableBytes)} · {volume.ok ? "容量足够" : "容量不足"}</span></li>)}
          </ul> : null}
        </div> : null}
        {mode === "restore" && restorePlans.length ? <div aria-live="polite" className="managed-library-restore-progress">
          {visibleRestorePlans.map((plan) => <div key={plan.restoreUuid}>
            <div className="managed-library-restore-progress-copy"><strong>{plan.name}</strong><span>{plan.phase} · {plan.mediaItemsDone}/{plan.mediaItemsTotal ?? "?"}</span><progress max={Math.max(plan.mediaItemsTotal ?? 1, 1)} value={plan.mediaItemsDone} /></div>
            <div className="managed-library-restore-progress-actions">
              {plan.state === "paused" ? <LiquidGlassButton disabled={busy} onClick={() => void changeRestoreState(plan, "resume")} type="button">继续</LiquidGlassButton> : !["complete", "cancelled", "failed"].includes(plan.state) ? <LiquidGlassButton disabled={busy} onClick={() => void changeRestoreState(plan, "pause")} type="button">暂停</LiquidGlassButton> : null}
              {!plan.catalogReady && !["complete", "cancelled", "failed"].includes(plan.state) ? <LiquidGlassButton disabled={busy} onClick={() => void changeRestoreState(plan, "cancel")} tone="danger" type="button">取消恢复</LiquidGlassButton> : null}
            </div>
          </div>)}
        </div> : null}
        {error ? <OverlayInlineStatus tone="error">{error}</OverlayInlineStatus> : null}
      </OverlayDialogBody>
      <OverlayDialogActions className="managed-library-wizard-actions">
        {!firstRun ? <LiquidGlassButton disabled={busy} onClick={onClose} type="button">取消</LiquidGlassButton> : null}
        {mode === "restore" && restorePlans.length ? <LiquidGlassButton onClick={onClose} type="button">完成</LiquidGlassButton> : <LiquidGlassButton disabled={busy || (mode === "restore" && selectedRemote.length === 0)} onClick={() => void (mode === "create" ? submit() : restorePreflight?.ok ? confirmRestore() : submitRestore())} type="button">
          {busy ? (mode === "restore" ? "正在处理…" : "正在创建…") : mode === "restore" ? restorePreflight?.ok ? `确认并恢复 ${restorePreflight.totalItems} 个资源库` : `预检 ${selectedRemote.length || ""} 个资源库` : "创建并打开"}
        </LiquidGlassButton>}
      </OverlayDialogActions>
    </ModalSurface>
  );
}
