import { useEffect, useRef, useState } from "react";
import type { RootLocationConflict, RootOperation, SyncFolderRecord } from "@megle/core-client";
import { FolderInput, Pause, Play, RefreshCw, SquarePen, TriangleAlert, Unlink } from "lucide-react";
import { pickSyncFolder, revealPath } from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";
import { ModalSurface, OverlayDialogActions, OverlayDialogBody, OverlayDialogHeader, OverlayInlineStatus } from "../../design/overlays";
import { canCancelRootOperation, rootOperationPhaseLabel, syncFolderIdentitySuffix, syncFolderStateLabel, visibleSyncFolderActions } from "./syncFolderModel";
import { type ReenablePreview, type RootRebindDraft, type RootRelocationDraft, type RootRenameDraft, useSyncFolders } from "./useSyncFolders";

type DialogState =
  | { kind: "stop"; folder: SyncFolderRecord }
  | { kind: "relocate"; folder: SyncFolderRecord; draft: RootRelocationDraft | null; operation: RootOperation | null }
  | { kind: "rename"; folder: SyncFolderRecord; requestedName: string; draft: RootRenameDraft | null }
  | { kind: "rebind"; folder: SyncFolderRecord; draft: RootRebindDraft }
  | { kind: "reenable"; folder: SyncFolderRecord; preview: ReenablePreview }
  | { kind: "conflicts"; folder: SyncFolderRecord; conflicts: RootLocationConflict[]; selections: Record<string, string> }
  | null;

export function SyncFoldersPanel({ libraryUuid, focusRootUuid = null }: { libraryUuid: string | null; focusRootUuid?: string | null }) {
  const controller = useSyncFolders(libraryUuid);
  const [dialog, setDialog] = useState<DialogState>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const relocationFolder = dialog?.kind === "relocate" ? dialog.folder : null;
  const relocationOperation = dialog?.kind === "relocate" ? dialog.operation : null;
  useEffect(() => {
    if (!focusRootUuid || controller.loading) return;
    document.querySelector<HTMLElement>(`[data-sync-root-uuid="${CSS.escape(focusRootUuid)}"]`)?.focus();
  }, [controller.loading, focusRootUuid]);

  function openDialog(next: Exclude<DialogState, null>, source?: HTMLElement | null) {
    restoreFocusRef.current = source ?? document.activeElement as HTMLElement | null;
    setDialog(next);
  }

  function closeDialog() {
    setDialog(null);
    window.requestAnimationFrame(() => restoreFocusRef.current?.focus());
  }

  useEffect(() => {
    if (!relocationFolder || !relocationOperation || !["pending", "running", "waiting"].includes(relocationOperation.state)) return;
    let cancelled = false;
    const timer = window.setInterval(() => {
      void controller.getRelocation(relocationFolder, relocationOperation.operationUuid).then((operation) => {
        if (!cancelled && operation) setDialog((current) => current?.kind === "relocate" ? { ...current, operation } : current);
      });
    }, 1_000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [controller.getRelocation, relocationFolder, relocationOperation?.operationUuid, relocationOperation?.state]);

  async function chooseFor(folder: SyncFolderRecord, mode: "materialize-remote" | "relocate", source?: HTMLElement | null) {
    if (!libraryUuid) return;
    const suggestedPath = mode === "relocate" ? folder.binding?.localPath ?? null : folder.proposedLocalPath ?? null;
    const selected = await pickSyncFolder({ libraryUuid, rootUuid: folder.descriptor.rootUuid, mode, suggestedPath, dialogTitle: mode === "relocate" ? "选择新的同步位置" : "选择同步文件夹位置" });
    if (!selected) return;
    if (mode === "materialize-remote") {
      await controller.select(folder, selected);
      return;
    }
    const preflight = await controller.preflightRelocation(folder, selected);
    if (preflight) openDialog({ kind: "relocate", folder, draft: preflight, operation: null }, source);
  }

  async function chooseRebind(folder: SyncFolderRecord, source?: HTMLElement | null) {
    if (!libraryUuid || !folder.rebindEvidence) return;
    const selected = await pickSyncFolder({ libraryUuid, rootUuid: folder.descriptor.rootUuid, mode: "rebind-existing", suggestedPath: folder.proposedLocalPath ?? folder.binding?.localPath ?? null, dialogTitle: "选择已有同步副本" });
    if (!selected) return;
    const draft = await controller.preflightRebind(folder, selected);
    if (draft) openDialog({ kind: "rebind", folder, draft }, source);
  }

  return (
    <section aria-labelledby="sync-folders-title" className="settings-sync-folders">
      <header className="settings-sync-folders-header">
        <div><h3 id="sync-folders-title">同步文件夹</h3><p>每个顶层文件夹独立选择位置、暂停、移动或停止同步。</p></div>
        <LiquidGlassButton disabled={!libraryUuid || controller.loading} onClick={() => void controller.refresh()} type="button"><RefreshCw aria-hidden="true" size={13} />刷新</LiquidGlassButton>
      </header>
      {controller.error ? <div className="settings-inline-error" role="alert">{controller.error}</div> : null}
      {!libraryUuid ? <p className="settings-sync-folder-empty">当前没有活动资源库。</p> : null}
      {libraryUuid && !controller.loading && controller.folders.length === 0 ? <p className="settings-sync-folder-empty">尚无同步文件夹。</p> : null}
      <div aria-busy={controller.loading} aria-live="polite" className="settings-sync-folder-list">
        {controller.folders.map((folder) => {
          const rootUuid = folder.descriptor.rootUuid;
          const actions = visibleSyncFolderActions(folder);
          const suffix = syncFolderIdentitySuffix(folder, controller.folders);
          const busy = controller.busyRootUuid === rootUuid;
          return <article className="settings-sync-folder-row" data-sync-root-uuid={rootUuid} key={`${folder.descriptor.libraryUuid}:${rootUuid}`} tabIndex={focusRootUuid === rootUuid ? -1 : undefined}>
            <div className="settings-sync-folder-copy">
              <div className="settings-sync-folder-name"><strong>{folder.descriptor.requestedName}</strong>{suffix ? <code>{suffix}</code> : null}<span>{syncFolderStateLabel(folder)}</span></div>
              {folder.descriptor.materializedName !== folder.descriptor.requestedName ? <p>本机名称：{folder.descriptor.materializedName}</p> : null}
              <p className="settings-sync-folder-path" title={folder.binding?.localPath ?? undefined}>{folder.binding?.localPath ?? "未在此设备同步"}</p>
              <p className="settings-sync-folder-meta">逻辑版本 {folder.descriptor.logicalRevision} · 清单版本 {folder.descriptor.inventoryRevision} · 操作版本 {folder.descriptor.operationRevision}{folder.descriptor.logicalBytes != null ? ` · ${folder.descriptor.logicalBytes.toLocaleString()} 字节` : ""}</p>
              {folder.descriptor.checkpointUuid ? <p className="settings-sync-folder-meta" title={folder.descriptor.checkpointUuid}>检查点：{folder.descriptor.checkpointUuid}</p> : null}
              {folder.binding ? <p className="settings-sync-folder-meta">绑定代次 {folder.binding.bindingGeneration} · 应用状态 {folder.binding.appliedMaterializedName ?? "尚未应用"}</p> : null}
              {folder.binding?.lastErrorCode ? <p className="settings-sync-folder-error" role="alert">{folder.binding.lastErrorCode}</p> : null}
              {folder.binding?.state === "local-rename-required" ? <p>远端名称已更新；本机目录仍保留旧名称，需要重试或选择新位置。</p> : null}
              {folder.activeOperation ? <OperationStatus operation={folder.activeOperation} /> : folder.latestOperation?.state === "failed" ? <OperationStatus operation={folder.latestOperation} /> : null}
            </div>
            <div className="settings-sync-folder-actions">
              {actions.includes("select") ? <LiquidGlassButton disabled={busy} onClick={(event) => void chooseFor(folder, "materialize-remote", event.currentTarget)} type="button"><FolderInput aria-hidden="true" size={13} />选择位置</LiquidGlassButton> : null}
              {actions.includes("pause") ? <LiquidGlassButton disabled={busy} onClick={() => void controller.command(folder, "pause")} type="button"><Pause aria-hidden="true" size={13} />暂停</LiquidGlassButton> : null}
              {actions.includes("resume") ? <LiquidGlassButton disabled={busy} onClick={() => void controller.command(folder, "resume")} type="button"><Play aria-hidden="true" size={13} />继续</LiquidGlassButton> : null}
              {folder.binding ? <LiquidGlassButton disabled={busy} onClick={() => void revealPath(folder.binding!.localPath)} type="button">在资源管理器中显示</LiquidGlassButton> : null}
              {actions.includes("relocate") ? <LiquidGlassButton disabled={busy} onClick={(event) => void chooseFor(folder, "relocate", event.currentTarget)} type="button">{folder.binding?.state === "local-rename-required" ? "选择新位置…" : "移动位置…"}</LiquidGlassButton> : null}
              {folder.activeOperation ? <LiquidGlassButton disabled={busy} onClick={(event) => openDialog({ kind: "relocate", folder, draft: null, operation: folder.activeOperation! }, event.currentTarget)} type="button">查看进度…</LiquidGlassButton> : null}
              {actions.includes("rename") ? <LiquidGlassButton disabled={busy} onClick={(event) => openDialog({ kind: "rename", folder, requestedName: folder.descriptor.requestedName, draft: null }, event.currentTarget)} type="button"><SquarePen aria-hidden="true" size={13} />重命名…</LiquidGlassButton> : null}
              {actions.includes("re-enable") ? <LiquidGlassButton disabled={busy} onClick={async (event) => { const preview = await controller.preflightReenable(folder); if (preview) openDialog({ kind: "reenable", folder, preview }, event.currentTarget); }} type="button">重新启用同步…</LiquidGlassButton> : null}
              {actions.includes("resolve-conflicts") ? <LiquidGlassButton disabled={busy} onClick={async (event) => { const conflicts = await controller.listConflicts(folder); if (conflicts) openDialog({ kind: "conflicts", folder, conflicts, selections: {} }, event.currentTarget); }} type="button"><TriangleAlert aria-hidden="true" size={13} />解决冲突…</LiquidGlassButton> : null}
              {actions.includes("stop-sync") ? <LiquidGlassButton disabled={busy} onClick={(event) => openDialog({ kind: "stop", folder }, event.currentTarget)} tone="danger" type="button"><Unlink aria-hidden="true" size={13} />停止同步…</LiquidGlassButton> : null}
              {actions.includes("rebind-existing") && folder.rebindEvidence ? <LiquidGlassButton disabled={busy} onClick={(event) => void chooseRebind(folder, event.currentTarget)} type="button">重新绑定现有副本…</LiquidGlassButton> : null}
              {actions.includes("retry") && folder.latestOperation ? <LiquidGlassButton disabled={busy} onClick={() => void controller.commandRelocation(folder, folder.latestOperation!, "retry")} type="button">重试操作</LiquidGlassButton> : null}
            </div>
          </article>;
        })}
      </div>
      <ModalSurface ariaDescribedBy="sync-folder-dialog-description" ariaLabelledBy="sync-folder-dialog-title" busy={Boolean(controller.busyRootUuid)} id="sync-folder-dialog" initialFocusRef={cancelRef} onDismiss={closeDialog} open={dialog !== null} rootId="library" role={dialog?.kind === "stop" ? "alertdialog" : "dialog"}>
        <OverlayDialogHeader description={dialog?.kind === "stop" ? "本机文件、目录记录、元数据和缩略图都会保留；之后的修改仅保存在此设备。" : dialog?.kind === "relocate" ? "Core 会检查路径重叠、可用空间和卷类型；确认后操作可在关闭窗口后继续。" : dialog?.kind === "reenable" ? "提交前请检查本机、基线和远端的三方比较；冲突必须先完成选择。" : dialog?.kind === "rename" ? "先由 Core 生成名称及本机目录影响计划，再明确确认。" : dialog?.kind === "rebind" ? "已验证目录标记和历史切换证据；确认前不会声明或修改目录。" : "请选择要保留的位置版本，其余版本将被替代。"} descriptionId="sync-folder-dialog-description" title={dialog?.kind === "stop" ? `停止同步“${dialog.folder.descriptor.requestedName}”` : dialog?.kind === "relocate" ? "移动同步文件夹" : dialog?.kind === "reenable" ? "重新启用同步" : dialog?.kind === "rename" ? "重命名同步文件夹" : dialog?.kind === "rebind" ? "重新绑定现有副本" : "解决位置冲突"} titleId="sync-folder-dialog-title" />
        <OverlayDialogBody className="sync-folder-dialog-body">
          {dialog?.kind === "relocate" ? <><p>来源：{dialog.folder.binding?.localPath}</p>{dialog.draft ? <><p>目标：{dialog.draft.preflight.normalizedPath}</p><p>可用空间：{dialog.draft.preflight.availableBytes?.toLocaleString() ?? "未知"} 字节</p></> : <p>该操作由 Core 持久化，重新打开应用后仍可继续查看。</p>}{dialog.operation ? <OperationStatus operation={dialog.operation} /> : null}</> : null}
          {dialog?.kind === "rename" ? <><label className="sync-folder-name-field"><span>新名称</span><input autoComplete="off" disabled={Boolean(dialog.draft)} onChange={(event) => setDialog({ ...dialog, requestedName: event.target.value, draft: null })} value={dialog.requestedName} /></label>{dialog.draft ? <div className="sync-folder-rename-preview"><p>逻辑名称：{dialog.draft.plan.requestedName}</p><p>本机目录名称：{dialog.draft.plan.materializedName}</p><p>位置版本：{dialog.draft.plan.expectedLocationHeads.length}</p></div> : null}</> : null}
          {dialog?.kind === "rebind" ? <><p>目录：{dialog.draft.plan.path}</p><p>标记验证：{dialog.draft.plan.markerMatches ? "匹配" : "不匹配"}</p><p title={dialog.draft.plan.contentEvidenceDigest}>内容证据：{dialog.draft.plan.contentEvidenceDigest}</p></> : null}
          {dialog?.kind === "reenable" ? <><OverlayInlineStatus tone={dialog.preview.plan.status === "failed" || dialog.preview.plan.status === "expired" ? "error" : "info"}>比较状态：{dialog.preview.plan.status} · 到期时间 {new Date(dialog.preview.plan.expiresAt).toLocaleString()}</OverlayInlineStatus><dl className="sync-folder-comparison-summary"><div><dt>基线</dt><dd>{dialog.preview.plan.baseDigest}</dd></div><div><dt>本机</dt><dd>{dialog.preview.plan.localDigest}</dd></div><div><dt>远端</dt><dd>{dialog.preview.plan.remoteDigest}</dd></div><div><dt>项目</dt><dd>{dialog.preview.plan.itemCount}</dd></div><div><dt>冲突</dt><dd>{dialog.preview.plan.conflictCount}</dd></div></dl><ul className="sync-folder-comparison-items">{dialog.preview.items.map((item) => <li key={item.itemOrdinal}><span>{item.classification}</span><span>{item.fieldName}</span>{item.classification === "conflict" ? <select aria-label={`${item.fieldName} 冲突处理`} value={item.resolution ?? ""} onChange={async (event) => { const resolution = event.target.value as "keep-local" | "keep-remote" | "keep-both"; const updated = await controller.resolveComparisonItem(dialog.folder, dialog.preview.plan, item, resolution); if (updated) setDialog({ ...dialog, preview: { ...dialog.preview, items: dialog.preview.items.map((candidate) => candidate.itemOrdinal === updated.itemOrdinal ? updated : candidate) } }); }}><option value="">请选择</option><option value="keep-local">保留本机</option><option value="keep-remote">保留远端</option><option value="keep-both">两者都保留</option></select> : <span>{item.resolution ?? item.suggestedAction ?? "无需处理"}</span>}</li>)}</ul>{dialog.preview.nextCursor !== null ? <LiquidGlassButton onClick={async () => { const preview = await controller.loadMoreComparisonItems(dialog.folder, dialog.preview); if (preview) setDialog({ ...dialog, preview }); }} type="button">加载更多比较项</LiquidGlassButton> : null}</> : null}
          {dialog?.kind === "conflicts" ? dialog.conflicts.length ? <ul className="sync-folder-conflict-list">{dialog.conflicts.map((conflict) => <li key={conflict.conflictUuid}><code>{conflict.conflictUuid}</code><select aria-label={`${conflict.conflictUuid} 保留的位置版本`} value={dialog.selections[conflict.conflictUuid] ?? ""} onChange={(event) => setDialog({ ...dialog, selections: { ...dialog.selections, [conflict.conflictUuid]: event.target.value } })}><option value="">请选择要保留的版本</option>{conflict.locationHeads.map((head) => <option key={head} value={head}>{head}</option>)}</select><LiquidGlassButton disabled={!dialog.selections[conflict.conflictUuid]} onClick={async () => { const operation = await controller.resolveConflict(dialog.folder, conflict, dialog.selections[conflict.conflictUuid]!); if (operation) setDialog({ ...dialog, conflicts: dialog.conflicts.filter((candidate) => candidate.conflictUuid !== conflict.conflictUuid) }); }} type="button">应用解决方案</LiquidGlassButton></li>)}</ul> : <OverlayInlineStatus tone="info">没有待处理的位置冲突。</OverlayInlineStatus> : null}
        </OverlayDialogBody>
        <OverlayDialogActions>
          <LiquidGlassButton onClick={closeDialog} ref={cancelRef} type="button">取消</LiquidGlassButton>
          {dialog?.kind === "stop" ? <LiquidGlassButton onClick={async () => { if (await controller.stop(dialog.folder)) closeDialog(); }} tone="danger" type="button">保留本机副本并停止</LiquidGlassButton> : null}
          {dialog?.kind === "relocate" && dialog.draft?.preflight.ok && !dialog.operation ? <LiquidGlassButton onClick={async () => { const operation = await controller.commitRelocation(dialog.folder, dialog.draft!); if (operation) setDialog({ ...dialog, operation }); }} type="button">确认移动</LiquidGlassButton> : null}
          {dialog?.kind === "relocate" && dialog.operation && (dialog.operation.canCancel ?? canCancelRootOperation(dialog.operation.phase, dialog.operation.cutoverCommitted)) ? <LiquidGlassButton onClick={async () => { const operation = await controller.commandRelocation(dialog.folder, dialog.operation!, "cancel"); if (operation) setDialog({ ...dialog, operation }); }} type="button">取消移动</LiquidGlassButton> : null}
          {dialog?.kind === "relocate" && dialog.operation?.state === "failed" ? <LiquidGlassButton onClick={async () => { const operation = await controller.commandRelocation(dialog.folder, dialog.operation!, "retry"); if (operation) setDialog({ ...dialog, operation }); }} type="button">重试操作</LiquidGlassButton> : null}
          {dialog?.kind === "rename" && !dialog.draft ? <LiquidGlassButton disabled={!dialog.requestedName.trim()} onClick={async () => { const draft = await controller.preflightRename(dialog.folder, dialog.requestedName.trim()); if (draft) setDialog({ ...dialog, draft }); }} type="button">预览名称变更</LiquidGlassButton> : null}
          {dialog?.kind === "rename" && dialog.draft ? <LiquidGlassButton onClick={async () => { if (await controller.commitRename(dialog.folder, dialog.draft!)) closeDialog(); }} type="button">确认重命名</LiquidGlassButton> : null}
          {dialog?.kind === "rebind" ? <LiquidGlassButton disabled={!dialog.draft.plan.markerMatches} onClick={async () => { if (await controller.commitRebind(dialog.folder, dialog.draft)) closeDialog(); }} type="button">确认重新绑定</LiquidGlassButton> : null}
          {dialog?.kind === "reenable" ? <><LiquidGlassButton onClick={async () => { const preview = await controller.refreshComparison(dialog.folder, dialog.preview); if (preview) setDialog({ ...dialog, preview }); }} type="button">刷新比较</LiquidGlassButton>{dialog.folder.allowedActions.includes("publish-as-new") ? <LiquidGlassButton disabled={!canCommitReenablePreview(dialog.preview)} onClick={async () => { const result = await controller.commitReenable(dialog.folder, dialog.preview.plan, "publish-as-new"); if (result.operation) closeDialog(); else if (result.refreshedPreview) setDialog({ ...dialog, preview: result.refreshedPreview }); }} type="button">发布为新文件夹</LiquidGlassButton> : null}<LiquidGlassButton disabled={!canCommitReenablePreview(dialog.preview)} onClick={async () => { const result = await controller.commitReenable(dialog.folder, dialog.preview.plan, "merge-local-fork"); if (result.operation) closeDialog(); else if (result.refreshedPreview) setDialog({ ...dialog, preview: result.refreshedPreview }); }} type="button">合并到原文件夹</LiquidGlassButton></> : null}
        </OverlayDialogActions>
      </ModalSurface>
    </section>
  );
}

function canCommitReenablePreview(preview: ReenablePreview): boolean {
  return preview.plan.status === "ready"
    && preview.nextCursor === null
    && !preview.items.some((item) => item.classification === "conflict" && !item.resolution);
}

function OperationStatus({ operation }: { operation: RootOperation }) {
  const byteProgress = operation.bytesDone != null && operation.bytesTotal != null ? `${operation.bytesDone.toLocaleString()} / ${operation.bytesTotal.toLocaleString()} 字节` : null;
  const itemProgress = operation.itemsDone != null && operation.itemsTotal != null ? `${operation.itemsDone.toLocaleString()} / ${operation.itemsTotal.toLocaleString()} 项` : null;
  const mode = operation.transferMode === "explicit-move" ? "用户移动" : operation.transferMode === "automatic-rename" ? "自动目录重命名" : null;
  const cancellable = operation.canCancel ?? canCancelRootOperation(operation.phase, operation.cutoverCommitted);
  return <OverlayInlineStatus tone={operation.state === "failed" ? "error" : "info"}>{rootOperationPhaseLabel(operation.phase)} · {operation.state}{mode ? ` · ${mode}` : ""}{byteProgress ? ` · ${byteProgress}` : itemProgress ? ` · ${itemProgress}` : ""}{operation.availableBytes != null ? ` · 可用 ${operation.availableBytes.toLocaleString()} 字节` : ""}{operation.errorCode ? ` · ${operation.errorCode}` : ""}{operation.recoveryState ? ` · ${operation.recoveryState}` : ""} · {cancellable ? "尚可取消" : "不可取消"}</OverlayInlineStatus>;
}
