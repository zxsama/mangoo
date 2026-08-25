import { useEffect, useMemo, useRef, useState } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import type { RemoteLibrarySummary, SyncRestorePlan, SyncRestorePreflight } from "@megle/core-client";
import {
  createLibrary,
  pickNewLibraryPath,
  preflightLibrary,
} from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  ModalSurface,
  OverlayDialogActions,
  OverlayDialogHeader,
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
  const [restoreItems, setRestoreItems] = useState<Array<{ accountUuid: string; libraryUuid: string; name: string; managedRoot: string }> | null>(null);
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
      const selected = await pickNewLibraryPath(remote.name);
      if (selected) {
        setRestoreRoots((current) => ({ ...current, [remote.libraryUuid]: selected }));
        setRestorePreflight(null);
        setRestoreItems(null);
      }
    } catch (cause) {
      setError(String(cause));
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
      const items = [];
      for (const remote of selectedRemote) {
        const checked = await preflightLibrary({ name: remote.name, managedRoot: restoreRoots[remote.libraryUuid] });
        if (!checked.ok) throw new Error(`“${remote.name}”无法使用该目录：${checked.reason}`);
        items.push({ accountUuid: remote.accountUuid, libraryUuid: remote.libraryUuid, name: remote.name, managedRoot: checked.normalizedRoot });
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
    if (!restorePreflight || !restoreItems || busy) return;
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
      id="managed-library-wizard"
      initialFocusRef={nameRef}
      onDismiss={() => {
        if (!busy) onClose();
      }}
      open={open}
      role="dialog"
      rootId="library"
    >
      <OverlayDialogHeader
        description={mode === "restore" ? "选择一个或多个 OneDrive 资源库，并为每个资源库指定本机根目录。" : firstRun ? "选择固定根目录，创建此设备上的第一个资源库。" : "先创建本机资源库；同步可选且不会阻塞本地使用。"}
        descriptionId="managed-library-wizard-description"
        title={firstRun ? "开始使用 Megle" : mode === "restore" ? "从 OneDrive 恢复" : "创建资源库"}
        titleId="managed-library-wizard-title"
      />
      <div aria-label="资源库来源" className="managed-library-mode" role="tablist">
        <button aria-selected={mode === "create"} data-selected={mode === "create"} onClick={() => { authResumeRef.current = null; setAuthResumePending(false); setMode("create"); setRestorePreflight(null); setRestoreItems(null); }} role="tab" type="button">新建本机资源库</button>
        <button aria-selected={mode === "restore"} data-selected={mode === "restore"} onClick={() => { authResumeRef.current = null; setAuthResumePending(false); setMode("restore"); setRestorePreflight(null); setRestoreItems(null); }} ref={restoreModeRef} role="tab" type="button">从 OneDrive 恢复</button>
      </div>
      {auth.plugin?.authState !== "authenticated" && (mode === "restore" || enableSync) ? (
        <div className="managed-library-auth-required">
          <p>连接 OneDrive 后会回到此流程；登录期间只在内存中保留当前模式和目标。</p>
          <OneDriveAccountSetupContent
            compact
            controller={auth}
            onBeforeBegin={prepareAuthResume}
            onCancelled={() => { authResumeRef.current = null; setAuthResumePending(false); }}
          />
        </div>
      ) : null}
      {mode === "create" ? <><label className="overlay-field">
        <span>名称</span>
        <input ref={nameRef} value={name} onChange={(event) => setName(event.target.value)} />
      </label>
      <LibraryIconPicker
        color={color}
        icon={icon}
        onPick={(nextIcon, nextColor) => {
          setIcon(nextIcon);
          setColor(nextColor);
        }}
        presentation="overlay"
      />
      <div className="managed-library-wizard-root">
        <span>{managedRoot ?? "尚未选择目录"}</span>
        <button data-interactive-pointer-target="true" disabled={busy} onClick={() => void chooseRoot()} type="button">
          选择目录…
        </button>
      </div>
      <label className="managed-library-enable-sync">
        <input checked={enableSync} onChange={(event) => setEnableSync(event.target.checked)} type="checkbox" />
        <span>创建后启用 OneDrive 同步（登录或网络失败不会删除本机资源库）</span>
      </label></> : <div aria-busy={remoteLoading} className="managed-library-remote-list" role="group">
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
                if (event.target.checked) next.add(remote.libraryUuid); else next.delete(remote.libraryUuid);
                setRestorePreflight(null);
                setRestoreItems(null);
                return next;
              })} type="checkbox" />
              <span><strong>{remote.name}</strong><small>{remote.boundLocally ? "已在本机绑定" : remote.compatible ? "可恢复" : remote.errorCode ?? "描述损坏或版本不兼容"}</small>{remoteDetails ? <small>{remoteDetails}</small> : null}</span>
            </label>
            {selected ? <div className="managed-library-wizard-root"><span>{restoreRoots[remote.libraryUuid] ?? "尚未选择目录"}</span><button data-interactive-pointer-target="true" disabled={busy} onClick={() => void chooseRestoreRoot(remote)} type="button">选择目录…</button></div> : null}
          </div>;
        })}
      </div>}
      {mode === "restore" && restorePreflight ? <div className="managed-library-restore-confirm" role="status">
        <strong>恢复预检已通过</strong>
        <span>{restorePreflight.totalItems} 个资源库 · 预计占用 {formatRemoteSize(restorePreflight.totalBytes)}</span>
        <small>确认后才会创建本机资源库和启动恢复任务。</small>
      </div> : null}
      {mode === "restore" && restorePlans.length ? <div aria-live="polite" className="managed-library-restore-progress">
        {visibleRestorePlans.map((plan) => <div key={plan.restoreUuid}>
          <div className="managed-library-restore-progress-copy"><strong>{plan.name}</strong><span>{plan.phase} · {plan.mediaItemsDone}/{plan.mediaItemsTotal ?? "?"}</span><progress max={Math.max(plan.mediaItemsTotal ?? 1, 1)} value={plan.mediaItemsDone} /></div>
          <div className="managed-library-restore-progress-actions">
            {plan.state === "paused" ? <button data-interactive-pointer-target={!busy ? "true" : undefined} disabled={busy} onClick={() => void changeRestoreState(plan, "resume")} type="button">继续</button> : !["complete", "cancelled", "failed"].includes(plan.state) ? <button data-interactive-pointer-target={!busy ? "true" : undefined} disabled={busy} onClick={() => void changeRestoreState(plan, "pause")} type="button">暂停</button> : null}
            {!plan.catalogReady && !["complete", "cancelled", "failed"].includes(plan.state) ? <button data-interactive-pointer-target={!busy ? "true" : undefined} disabled={busy} onClick={() => void changeRestoreState(plan, "cancel")} type="button">取消恢复</button> : null}
          </div>
        </div>)}
      </div> : null}
      {error ? <div className="settings-inline-error">{error}</div> : null}
      <OverlayDialogActions>
        {!firstRun ? <button disabled={busy} onClick={onClose} type="button">取消</button> : null}
        {mode === "restore" && restorePlans.length ? <LiquidGlassButton onClick={onClose} type="button">完成</LiquidGlassButton> : <LiquidGlassButton disabled={busy || (mode === "restore" && selectedRemote.length === 0)} onClick={() => void (mode === "create" ? submit() : restorePreflight ? confirmRestore() : submitRestore())} type="button">
          {busy ? (mode === "restore" ? "正在处理…" : "正在创建…") : mode === "restore" ? restorePreflight ? `确认并恢复 ${restorePreflight.totalItems} 个资源库` : `预检 ${selectedRemote.length || ""} 个资源库` : "创建并打开"}
        </LiquidGlassButton>}
      </OverlayDialogActions>
    </ModalSurface>
  );
}
