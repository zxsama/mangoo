import { useRef, useState } from "react";
import type {
  LibrarySummary,
  LibrarySyncStatus,
  SyncProviderStatus,
  SyncAllowedAction,
  SyncConflictRecord,
  SyncReconnectPlan,
} from "@megle/core-client";
import {
  AlertTriangle,
  Cloud,
  Download,
  Pause,
  Play,
  RefreshCw,
  ShieldAlert,
  Unlink,
} from "lucide-react";
import { createCoreClient } from "../../core/client";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  ModalSurface,
  OverlayDialogActions,
  OverlayDialogBody,
  OverlayDialogHeader,
} from "../../design/overlays";
import { actionLabel, syncStatusLabel } from "./syncModel";

interface Props {
  libraries: LibrarySummary[];
  statuses: LibrarySyncStatus[];
  busyLibraryId: string | null;
  error: string | null;
  providers: SyncProviderStatus[];
  onAction(status: LibrarySyncStatus, action: SyncAllowedAction): void;
  onUnbind(
    status: LibrarySyncStatus,
    choice: "drain-before-unbind" | "keep-local-fork",
  ): void;
  onReconnectPreview(status: LibrarySyncStatus): Promise<SyncReconnectPlan | null>;
  onReconnectCommit(status: LibrarySyncStatus, plan: SyncReconnectPlan, choice: "merge-local-fork" | "publish-new-remote"): void;
  onAccountChange(pluginId: string, switchAccount: boolean): void;
  onEnable(status: LibrarySyncStatus, provider: SyncProviderStatus): void;
  onRemoveLocalCopy(library: LibrarySummary): void;
}

export function SyncManagementPanel({
  libraries,
  statuses,
  busyLibraryId,
  error,
  providers,
  onAction,
  onUnbind,
  onReconnectPreview,
  onReconnectCommit,
  onAccountChange,
  onEnable,
  onRemoveLocalCopy,
}: Props) {
  const [confirm, setConfirm] = useState<{
    kind: "unbind" | "reconnect" | "account-sign-out" | "account-switch";
    status: LibrarySyncStatus;
    plan?: SyncReconnectPlan;
    pluginId?: string;
  } | null>(null);
  const [details, setDetails] = useState<{
    status: LibrarySyncStatus;
    conflicts: SyncConflictRecord[];
    nextCursor: string | null;
    diagnostics: Record<string, unknown>;
  } | null>(null);
  const [detailsError, setDetailsError] = useState<string | null>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const [providerByLibrary, setProviderByLibrary] = useState<Record<string, string>>({});
  const [accountByLibrary, setAccountByLibrary] = useState<Record<string, string>>({});
  const byId = new Map(statuses.map((status) => [status.libraryUuid, status]));

  async function previewReconnect(status: LibrarySyncStatus) {
    const plan = await onReconnectPreview(status);
    if (plan) setConfirm({ kind: "reconnect", status, plan });
  }

  async function openDetails(status: LibrarySyncStatus) {
    setDetailsError(null);
    try {
      const client = createCoreClient();
      const [conflicts, diagnostics] = await Promise.all([
        client.listLibrarySyncConflicts(status.libraryUuid, { limit: 50 }),
        client.getLibrarySyncDiagnostics(status.libraryUuid),
      ]);
      setDetails({ status, conflicts: conflicts.items, nextCursor: conflicts.nextCursor, diagnostics });
    } catch (cause) {
      setDetailsError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function loadMoreConflicts() {
    if (!details?.nextCursor) return;
    try {
      const page = await createCoreClient().listLibrarySyncConflicts(details.status.libraryUuid, { cursor: details.nextCursor, limit: 50 });
      setDetails((value) => value ? { ...value, conflicts: [...value.conflicts, ...page.items], nextCursor: page.nextCursor } : value);
    } catch (cause) {
      setDetailsError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function resolveConflict(conflict: SyncConflictRecord) {
    if (!details) return;
    try {
      const client = createCoreClient();
      const active = await client.getActiveLibrary();
      await client.resolveLibrarySyncConflict(
        details.status.libraryUuid,
        conflict.conflictUuid,
        active.contextRevision,
        { operationUuid: crypto.randomUUID(), resolution: "resolve" },
      );
      setDetails((value) =>
        value
          ? {
              ...value,
              conflicts: value.conflicts.filter(
                (item) => item.conflictUuid !== conflict.conflictUuid,
              ),
            }
          : value,
      );
    } catch (cause) {
      setDetailsError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  async function exportDiagnostics() {
    if (!details) return;
    try {
      const data = await createCoreClient().exportLibrarySyncDiagnostics(
        details.status.libraryUuid,
      );
      const blob = new Blob([JSON.stringify(data, null, 2)], {
        type: "application/json",
      });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `megle-sync-${details.status.libraryUuid}.json`;
      anchor.click();
      URL.revokeObjectURL(url);
    } catch (cause) {
      setDetailsError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  return (
    <section aria-labelledby="sync-management-title" className="settings-sync-panel">
      <header>
        <Cloud aria-hidden="true" size={17} />
        <div>
          <h3 id="sync-management-title">同步与恢复</h3>
          <p>每个资源库独立运行；本地浏览不等待网络。</p>
        </div>
      </header>
      {error || detailsError ? (
        <div className="settings-inline-error" role="alert">
          {error ?? detailsError}
        </div>
      ) : null}
      <div aria-live="polite" className="settings-sync-list">
        {libraries.map((library) => {
          const status = byId.get(library.libraryUuid);
          const busy = busyLibraryId === library.libraryUuid;
          return (
            <article className="settings-sync-card" key={library.libraryUuid}>
              <div>
                <strong>{library.name}</strong>
                <span className={`settings-sync-state ${status?.lifecycle ?? "local-only"}`}>
                  {status?.lifecycle === "error" ? (
                    <AlertTriangle aria-hidden="true" size={13} />
                  ) : null}
                  {syncStatusLabel(status)}
                </span>
              </div>
              <p>
                {status
                  ? `阶段：${status.phase} · 待上传 ${status.pendingUploadItems} · 待恢复 ${status.pendingDownloadItems} · 冲突 ${status.conflictCount}`
                  : "正在读取同步状态…"}
              </p>
              {status?.restoreItemsTotal != null ? (
                <div className="settings-sync-progress">
                  <progress max={Math.max(status.restoreItemsTotal, 1)} value={status.restoreItemsDone ?? 0} />
                  <span>{status.restoreItemsDone ?? 0}/{status.restoreItemsTotal} · {status.restoreBytesDone ?? 0}/{status.restoreBytesTotal ?? 0} 字节</span>
                </div>
              ) : null}
              {status?.lastErrorCode ? (
                <p className="settings-sync-error">{status.lastErrorCode}</p>
              ) : null}
              <div className="settings-sync-actions">
                {status?.allowedActions.includes("enable") ? (
                  <>
                    <label className="settings-sync-provider-select">
                      <span>提供方</span>
                      <select
                        aria-label={`${library.name} 同步提供方`}
                        value={providerByLibrary[library.libraryUuid] ?? providers[0]?.pluginId ?? ""}
                        onChange={(event) => {
                          const pluginId = event.target.value;
                          const firstAccount = providers.find((provider) => provider.pluginId === pluginId)?.accountUuid ?? "";
                          setProviderByLibrary((value) => ({ ...value, [library.libraryUuid]: pluginId }));
                          setAccountByLibrary((value) => ({ ...value, [library.libraryUuid]: firstAccount }));
                        }}
                      >
                        {[...new Map(providers.map((provider) => [provider.pluginId, provider])).values()].map((provider) => <option key={provider.pluginId} value={provider.pluginId}>{provider.pluginId}</option>)}
                      </select>
                    </label>
                    <label className="settings-sync-provider-select">
                      <span>账号</span>
                      <select
                        aria-label={`${library.name} 同步账号`}
                        value={accountByLibrary[library.libraryUuid] ?? providers.find((provider) => provider.pluginId === (providerByLibrary[library.libraryUuid] ?? providers[0]?.pluginId))?.accountUuid ?? ""}
                        onChange={(event) => setAccountByLibrary((value) => ({ ...value, [library.libraryUuid]: event.target.value }))}
                      >
                        {providers.filter((provider) => provider.pluginId === (providerByLibrary[library.libraryUuid] ?? providers[0]?.pluginId)).map((provider) => <option key={provider.accountUuid} value={provider.accountUuid}>{provider.accountDisplayName ?? provider.accountUuid}</option>)}
                      </select>
                    </label>
                    <button data-interactive-pointer-target={!busy && providers.length ? "true" : undefined} disabled={busy || providers.length === 0} onClick={() => {
                      const pluginId = providerByLibrary[library.libraryUuid] ?? providers[0]?.pluginId;
                      const accountUuid = accountByLibrary[library.libraryUuid] ?? providers.find((provider) => provider.pluginId === pluginId)?.accountUuid;
                      const selected = providers.find((provider) => provider.pluginId === pluginId && provider.accountUuid === accountUuid);
                      if (selected) onEnable(status, selected);
                    }} type="button"><Cloud aria-hidden="true" size={13} />启用同步</button>
                  </>
                ) : null}
                {status?.allowedActions
                  .filter((action) => ["pause", "resume", "retry", "sync-now", "cancel-before-catalog"].includes(action))
                  .map((action) => (
                  <button
                      data-interactive-pointer-target={!busy ? "true" : undefined}
                      disabled={busy}
                      key={action}
                      onClick={() => onAction(status, action)}
                      type="button"
                    >
                      {action === "pause" ? (
                        <Pause aria-hidden="true" size={13} />
                      ) : action === "resume" ? (
                        <Play aria-hidden="true" size={13} />
                      ) : action === "cancel-before-catalog" ? (
                        <Unlink aria-hidden="true" size={13} />
                      ) : (
                        <RefreshCw aria-hidden="true" size={13} />
                      )}
                      {actionLabel(action)}
                    </button>
                  ))}
                {status &&
                (status.allowedActions.includes("resolve-conflicts") || status.conflictCount > 0) ? (
                  <button data-interactive-pointer-target={!busy ? "true" : undefined} disabled={busy} onClick={() => void openDetails(status)} type="button">
                    <ShieldAlert aria-hidden="true" size={13} />冲突与诊断
                  </button>
                ) : null}
                {status?.allowedActions.includes("unbind") ? (
                  <button data-interactive-pointer-target={!busy ? "true" : undefined} disabled={busy} onClick={() => setConfirm({ kind: "unbind", status })} type="button">
                    <Unlink aria-hidden="true" size={13} />取消同步
                  </button>
                ) : null}
                {status?.allowedActions.includes("reconnect") ? (
                  <button data-interactive-pointer-target={!busy ? "true" : undefined} disabled={busy} onClick={() => void previewReconnect(status)} type="button">
                    <RefreshCw aria-hidden="true" size={13} />重新连接
                  </button>
                ) : null}
                {status?.catalogReadiness === "ready" && status.restoreUuid && !library.active ? (
                  <button
                    data-interactive-pointer-target={!busy ? "true" : undefined}
                    disabled={busy}
                    onClick={() => onRemoveLocalCopy(library)}
                    type="button"
                  >
                    <Unlink aria-hidden="true" size={13} />移除本机副本…
                  </button>
                ) : null}
              </div>
            </article>
          );
        })}
      </div>
      {[...new Map(providers.map((provider) => [provider.pluginId, provider])).values()].map((provider) => {
        const affected = statuses.filter((status) => status.providerId === provider.pluginId && status.lifecycle !== "local-only");
        if (!affected.length) return null;
        return <div className="settings-sync-account" key={provider.pluginId}>
          <div><strong>{provider.accountDisplayName ?? provider.accountUuid}</strong><span>{affected.length} 个资源库将暂停并需要逐库重新连接</span></div>
          <button data-interactive-pointer-target="true" onClick={() => setConfirm({ kind: "account-sign-out", status: affected[0], pluginId: provider.pluginId })} type="button">退出账号…</button>
          <button data-interactive-pointer-target="true" onClick={() => setConfirm({ kind: "account-switch", status: affected[0], pluginId: provider.pluginId })} type="button">切换账号…</button>
        </div>;
      })}

      <ModalSurface
        ariaDescribedBy="sync-confirm-description"
        ariaLabelledBy="sync-confirm-title"
        id="sync-confirm"
        initialFocusRef={cancelRef}
        onDismiss={() => setConfirm(null)}
        open={confirm !== null}
        role="alertdialog"
        rootId="library"
      >
        <OverlayDialogHeader
          description={
            confirm?.kind === "unbind"
              ? "只移除此设备的同步绑定，保留本机资源库和远端对象。选择排空会先发布待处理变更；保留分叉会立即将后续编辑留在本机。"
              : confirm?.kind === "reconnect"
                ? `三方预览：本机分叉 ${confirm.plan?.localDivergenceCount ?? 0} 项；远端相对解绑基线新增 ${confirm.plan?.remoteDivergenceCount ?? 0} 个对象；原远端身份${confirm.plan?.sameRemoteIdentity ? "匹配" : "不匹配"}，协议${confirm.plan?.remoteCompatible ? "兼容" : "不兼容"}。预览后必须显式选择。`
                : `此操作会原子暂停受影响的 ${statuses.filter((item) => item.providerId === confirm?.pluginId && item.lifecycle !== "local-only").length} 个资源库，清除账户会话；之后需要逐库重新连接。`
          }
          descriptionId="sync-confirm-description"
          title={
            confirm?.kind === "unbind"
              ? `取消“${confirm.status.name}”同步`
              : confirm?.kind === "reconnect" ? `重新连接“${confirm?.status.name ?? ""}”` : confirm?.kind === "account-switch" ? "切换同步账号" : "退出同步账号"
          }
          titleId="sync-confirm-title"
        />
        <OverlayDialogActions>
          <button data-interactive-pointer-target="true" onClick={() => setConfirm(null)} ref={cancelRef} type="button">取消</button>
          {confirm?.kind === "unbind" ? (
            <>
              <button data-interactive-pointer-target="true" onClick={() => { onUnbind(confirm.status, "keep-local-fork"); setConfirm(null); }} type="button">保留本机分叉</button>
              <LiquidGlassButton onClick={() => { onUnbind(confirm.status, "drain-before-unbind"); setConfirm(null); }} type="button">排空后取消同步</LiquidGlassButton>
            </>
          ) : null}
          {confirm?.kind === "reconnect" ? (
            <>
              <button disabled={!confirm.plan?.publishNewAllowed} data-interactive-pointer-target="true" onClick={() => { if (confirm.plan) onReconnectCommit(confirm.status, confirm.plan, "publish-new-remote"); setConfirm(null); }} type="button">发布为新远端</button>
              <LiquidGlassButton disabled={!confirm.plan?.mergeAllowed} onClick={() => { if (confirm.plan) onReconnectCommit(confirm.status, confirm.plan, "merge-local-fork"); setConfirm(null); }} type="button">合并本机分叉</LiquidGlassButton>
            </>
          ) : null}
          {confirm?.kind === "account-sign-out" || confirm?.kind === "account-switch" ? (
            <LiquidGlassButton onClick={() => { if (confirm.pluginId) onAccountChange(confirm.pluginId, confirm.kind === "account-switch"); setConfirm(null); }} type="button">{confirm.kind === "account-switch" ? "暂停并切换" : "暂停并退出"}</LiquidGlassButton>
          ) : null}
        </OverlayDialogActions>
      </ModalSurface>

      <ModalSurface
        ariaDescribedBy="sync-details-description"
        ariaLabelledBy="sync-details-title"
        id="sync-details"
        onDismiss={() => setDetails(null)}
        open={details !== null}
        role="dialog"
        rootId="library"
      >
        <OverlayDialogHeader
          description="冲突按资源库隔离；导出的诊断不包含令牌、会话密钥、绝对源路径或媒体名。"
          descriptionId="sync-details-description"
          title={`${details?.status.name ?? ""} · 冲突与诊断`}
          titleId="sync-details-title"
        />
        <OverlayDialogBody>
          {details?.conflicts.length ? (
            <ul className="settings-sync-conflicts">
              {details.conflicts.map((conflict) => (
                <li key={conflict.conflictUuid}>
                  <div><strong>{conflict.conflictKind}</strong>{conflict.entityUuid ? <span>实体 {conflict.entityUuid}</span> : null}{conflict.details != null ? <pre>{JSON.stringify(conflict.details, null, 2)}</pre> : null}</div>
                  <button data-interactive-pointer-target="true" onClick={() => void resolveConflict(conflict)} type="button">标记已解决</button>
                </li>
              ))}
            </ul>
          ) : (
            <p className="overlay-dialog-text">当前没有未解决冲突。</p>
          )}
          {details?.nextCursor ? <button data-interactive-pointer-target="true" onClick={() => void loadMoreConflicts()} type="button">加载更多冲突</button> : null}
          <pre className="settings-sync-diagnostics">
            {JSON.stringify(details?.diagnostics ?? {}, null, 2)}
          </pre>
        </OverlayDialogBody>
        <OverlayDialogActions>
          <button data-interactive-pointer-target="true" onClick={() => setDetails(null)} type="button">关闭</button>
          <LiquidGlassButton onClick={() => void exportDiagnostics()} type="button">
            <Download aria-hidden="true" size={13} />导出脱敏诊断
          </LiquidGlassButton>
        </OverlayDialogActions>
      </ModalSurface>
    </section>
  );
}
