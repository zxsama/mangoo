import { useEffect, useReducer, useRef, useState } from "react";
import { Check, FolderMinus, FolderOpen, FolderPlus, Library, Minus, MoreHorizontal, Palette, Search, X } from "../../design/icons";
import { createCoreClient } from "../../core/client";
import {
  listLibraries,
  forgetLibraryEntry,
  getDesktopBridge,
  getDesktopShellActions,
  openPath,
  pickNativeFolder,
  setLibraryIcon,
  switchLibrary,
  subscribeLibraryRegistry,
} from "../../core/desktop";
import type { LibrarySummary } from "../../core/libraryTypes";
import {
  createLibraryErrorReportAttempt,
  reportLibraryError,
  type NotificationLibraryContext,
} from "../../core/notificationReporting";
import { REMOVE_ROOT_CONFIRM } from "../../core/removeRootConfirm";
import type { LibraryState } from "../../core/useLibraryData";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  AnchoredOverlay,
  ModalSurface,
  OverlayDialogActions,
  OverlayDialogBody,
  OverlayDialogHeader,
  OverlayInlineStatus,
  OverlayMenuItem,
  OverlayMenuList,
  OVERLAY_VIEWPORT_GUTTER_PX,
  pointOverlayAnchor,
  useOverlayStack,
} from "../../design/overlays";
import {
  DEFAULT_LIBRARY_COLOR,
  DEFAULT_LIBRARY_ICON,
  LibraryGlyph,
} from "./libraryGlyph";
import { LibraryAppearanceEditor } from "./LibraryAppearanceEditor";
import { ManagedLibraryWizard } from "./ManagedLibraryWizard";
import { OpenLibraryDialog } from "./OpenLibraryDialog";
import { libraryOperationError } from "./libraryOperationError";
import { PluginContributionsSlot } from "../plugins/PluginContributionsSlot";
import {
  INITIAL_LIBRARY_SWITCHER_INTERACTION_STATE,
  librarySwitcherInteractionReducer,
} from "./librarySwitcherState";

const LIBRARY_SWITCHER_FALLBACK_ANCHOR = pointOverlayAnchor(
  OVERLAY_VIEWPORT_GUTTER_PX,
  OVERLAY_VIEWPORT_GUTTER_PX,
);

export interface LibrarySwitcherProps {
  anchor: HTMLElement | null;
  autoFocus: boolean;
  library: LibraryState;
  notificationContext: NotificationLibraryContext | null;
  onClose: () => void;
  open: boolean;
  restoreFocusTarget: HTMLElement | null;
}

function failureMessage(cause: unknown): string {
  return libraryOperationError(cause);
}

function ensureDesktopMutationSucceeded(
  result: { error?: string; ok: boolean },
  fallbackMessage: string,
): void {
  if (!result.ok) throw new Error(result.error ?? fallbackMessage);
}

export function LibrarySwitcher({
  anchor,
  autoFocus,
  library,
  notificationContext,
  onClose,
  open,
  restoreFocusTarget,
}: LibrarySwitcherProps) {
  const overlays = useOverlayStack();
  const [libs, setLibs] = useState<LibrarySummary[]>([]);
  const [query, setQuery] = useState("");
  const [wizardOpen, setWizardOpen] = useState(false);
  const [firstRun, setFirstRun] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ libraryId: string; anchor: HTMLButtonElement } | null>(null);
  const [appearanceTarget, setAppearanceTarget] = useState<LibrarySummary | null>(null);
  const appearanceRestoreFocusRef = useRef<HTMLElement | null>(null);
  const [forgetTarget, setForgetTarget] = useState<LibrarySummary | null>(null);
  const [forgetPending, setForgetPending] = useState(false);
  const [forgetError, setForgetError] = useState<string | null>(null);
  const [openDialog, setOpenDialog] = useState(false);
  const [interaction, dispatch] = useReducer(
    librarySwitcherInteractionReducer,
    INITIAL_LIBRARY_SWITCHER_INTERACTION_STATE,
  );
  const cancelRef = useRef<HTMLButtonElement>(null);
  const removeTriggerRef = useRef<HTMLButtonElement | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const mountedRef = useRef(false);
  const previousOpenRef = useRef(open);
  const reloadGenerationRef = useRef(0);
  const sessionGenerationRef = useRef(0);
  const removePendingRef = useRef<number | null>(null);
  const wizardRestoreFocusRef = useRef<HTMLElement | null>(null);
  const forgetPendingRef = useRef(false);
  const forgetCancelRef = useRef<HTMLButtonElement>(null);
  const forgetTriggerRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    mountedRef.current = true;
    void reload(true);
    return () => {
      mountedRef.current = false;
      reloadGenerationRef.current += 1;
      sessionGenerationRef.current += 1;
    };
  }, []);

  useEffect(() => subscribeLibraryRegistry(() => {
    if (mountedRef.current) void reload();
  }), []);

  useEffect(() => {
    const wasOpen = previousOpenRef.current;
    previousOpenRef.current = open;
    sessionGenerationRef.current += 1;
    if (open) {
      void reload();
      return;
    }
    if (wasOpen) {
      reloadGenerationRef.current += 1;
      removePendingRef.current = null;
      setMenu(null);
      setAppearanceTarget(null);
      setForgetTarget(null);
      dispatch({ type: "close-root" });
    }
  }, [open]);

  function ownsSession(generation: number): boolean {
    return mountedRef.current && generation === sessionGenerationRef.current;
  }

  async function reportFailure(source: string, target: string, cause: unknown): Promise<void> {
    const attempt = createLibraryErrorReportAttempt(notificationContext);
    await reportLibraryError(attempt, {
      code: `${source}-failed`,
      message: failureMessage(cause),
      source,
      target,
    });
  }

  async function reload(detectFirstRun = false): Promise<void> {
    const generation = ++reloadGenerationRef.current;
    setLoading(true);
    setError(null);
    try {
      const nextLibraries = await listLibraries();
      if (!mountedRef.current || generation !== reloadGenerationRef.current) return;
      setLibs(nextLibraries);
      setLoading(false);
      if (detectFirstRun && nextLibraries.length === 0) {
        setFirstRun(true);
        wizardRestoreFocusRef.current = restoreFocusTarget;
        overlays.openRoot("managed-library");
        setWizardOpen(true);
      }
    } catch (cause) {
      if (!mountedRef.current || generation !== reloadGenerationRef.current) return;
      setLoading(false);
      setError(failureMessage(cause));
      await reportFailure("library-list", "library-registry", cause);
    }
  }

  async function runMutation(
    source: string,
    target: string,
    operation: () => Promise<void>,
  ): Promise<boolean> {
    const session = sessionGenerationRef.current;
    setError(null);
    try {
      await operation();
      return true;
    } catch (cause) {
      if (ownsSession(session)) setError(failureMessage(cause));
      await reportFailure(source, target, cause);
      return false;
    }
  }

  const filtered = libs.filter(
    (item) =>
      item.name.toLowerCase().includes(query.trim().toLowerCase()) ||
      item.databasePath.toLowerCase().includes(query.trim().toLowerCase()),
  );
  const removeTarget = interaction.remove.kind === "closed" ? null : interaction.remove;
  const removePending = interaction.remove.kind === "pending";

  async function onSwitch(item: LibrarySummary) {
    if (item.active || item.availability !== "available") return;
    onClose();
    await runMutation("library-switch", item.libraryUuid, async () => {
      ensureDesktopMutationSucceeded(await switchLibrary(item.libraryUuid), "Library switch failed");
    });
  }

  function openManagedLibraryWizard() {
    wizardRestoreFocusRef.current = restoreFocusTarget;
    setFirstRun(false);
    overlays.openRoot("managed-library");
    setWizardOpen(true);
  }

  function closeManagedLibraryWizard() {
    setWizardOpen(false);
    overlays.closeRoot("managed-library");
  }

  function openExistingLibraryDialog() {
    wizardRestoreFocusRef.current = restoreFocusTarget;
    overlays.openRoot("managed-library");
    setOpenDialog(true);
  }

  async function confirmForget() {
    if (!forgetTarget || forgetPendingRef.current) return;
    const session = sessionGenerationRef.current;
    forgetPendingRef.current = true;
    setForgetPending(true);
    setForgetError(null);
    try {
      await forgetLibraryEntry(forgetTarget.libraryUuid);
      if (ownsSession(session)) {
        setForgetTarget(null);
        setMenu(null);
        dispatch({ type: "close-root" });
        void reload();
      }
    } catch (cause) {
      if (ownsSession(session)) setForgetError(failureMessage(cause));
      await reportFailure("library-forget", forgetTarget.libraryUuid, cause);
    } finally {
      forgetPendingRef.current = false;
      if (mountedRef.current) setForgetPending(false);
    }
  }

  async function onAddFolder() {
    const session = sessionGenerationRef.current;
    let folder: string | null;
    try {
      folder = await pickNativeFolder();
    } catch (cause) {
      await reportFailure("library-add-root", "folder-picker", cause);
      return;
    }
    if (!folder) return;
    const activeLibrary = libs.find((item) => item.active);
    const succeeded = await runMutation(
      "library-add-root",
      folder,
      async () => {
        const active = await createCoreClient().getActiveLibrary();
        if (!activeLibrary || active.library?.libraryUuid !== activeLibrary.libraryUuid) {
          throw new Error("活动资源库已改变，请重新选择文件夹。");
        }
        await library.addRoot(folder);
      },
    );
    if (succeeded) {
      if (ownsSession(session)) void reload();
    }
  }

  async function confirmRootRemoval() {
    if (interaction.remove.kind !== "confirming" || removePendingRef.current !== null) return;
    const session = sessionGenerationRef.current;
    const target = interaction.remove;
    removePendingRef.current = session;
    dispatch({ type: "begin-remove" });
    let removalSucceeded = false;
    const requestSucceeded = await runMutation(
      "library-remove-root",
      String(target.rootId),
      async () => {
        removalSucceeded = await library.removeRoot(target.rootId);
      },
    );
    const succeeded = requestSucceeded && removalSucceeded;
    if (removePendingRef.current === session) removePendingRef.current = null;
    if (!ownsSession(session)) return;
    if (!succeeded) {
      dispatch({ type: "fail-remove" });
      return;
    }
    dispatch({ type: "complete-remove" });
    void reload();
  }

  const searchField = (
    <div className="library-switcher-search">
      <Search size={14} aria-hidden="true" />
      <input
        aria-label="搜索资源库"
        onChange={(event) => { setQuery(event.target.value); setMenu(null); dispatch({ type: "close-root" }); }}
        placeholder="搜索资源库"
        ref={searchRef}
        value={query}
      />
    </div>
  );

  const closeButton = (
    <button
      aria-label="关闭资源库面板"
      className="library-switcher-close"
      data-interactive-pointer-target="true"
      onClick={onClose}
      type="button"
    >
      <X aria-hidden="true" size={14} />
    </button>
  );

  const libraryRows = filtered.map((item) => {
    const icon = item.icon ?? DEFAULT_LIBRARY_ICON;
    const color = item.color ?? DEFAULT_LIBRARY_COLOR;
    const menuOpen = menu?.libraryId === item.libraryUuid;
    const nested = interaction.nested;
    const removeMenuOpen = menuOpen && item.active && nested.kind === "remove" && nested.libraryId === item.libraryUuid;
    const removeMenuAnchor = nested.kind === "remove" && nested.libraryId === item.libraryUuid ? nested.anchor : null;
    const menuId = `library-actions-${item.libraryUuid}`;
    return (
      <li
        aria-disabled={forgetPending || undefined}
        className={`library-switcher-row${item.active ? " active" : ""}`}
        data-interactive-pointer-owner="true"
        key={item.libraryUuid}
      >
        <button
          aria-current={item.active ? "true" : undefined}
          className="library-switcher-main"
          disabled={item.availability !== "available" || forgetPending}
          onClick={() => void onSwitch(item)}
          title={`${item.name}\n${item.databasePath}${item.availability === "available" ? "" : "\n资源库暂不可用"}`}
          type="button"
        >
          <LibraryGlyph color={color} icon={icon} selected={item.active} size={24} />
          <span className="library-switcher-identity">
            <span className="library-switcher-name">{item.name}</span>
            <span className="library-switcher-path">{item.databasePath}</span>
          </span>
          {item.active ? <Check aria-label="当前资源库" className="library-switcher-check" size={16} /> : null}
        </button>
        <button
          aria-controls={menuId}
          aria-expanded={menuOpen}
          aria-haspopup="menu"
          aria-label={`${item.name} 的更多操作`}
          className="library-switcher-more"
          disabled={forgetPending}
          onClick={(event) => {
            dispatch({ type: "close-root" });
            setMenu(menuOpen ? null : { libraryId: item.libraryUuid, anchor: event.currentTarget });
          }}
          type="button"
        ><MoreHorizontal aria-hidden="true" size={20} /></button>
        <AnchoredOverlay anchor={menuOpen ? menu.anchor : null} ariaLabel={`${item.name} 的操作`} className="library-switcher-action-menu" id={menuId} onDismiss={() => { setMenu(null); dispatch({ type: "close-root" }); }} open={menuOpen} parentId="library-root" placement="right-start" preferredWidth={232} role="group" rootId="library" surfaceRole="nested">
          <OverlayMenuList ariaLabel="资源库操作" className="overlay-titlebar-menu">
            <OverlayMenuItem aria-haspopup="dialog" aria-label="更改图标" icon={<Palette size={16} />} onClick={() => {
              appearanceRestoreFocusRef.current = menu?.anchor ?? restoreFocusTarget;
              overlays.dismissTop();
              setAppearanceTarget(item);
              setMenu(null);
              dispatch({ type: "close-root" });
            }}>图标与颜色…</OverlayMenuItem>
            {getDesktopShellActions()?.openPath ? <OverlayMenuItem icon={<FolderOpen size={16} />} onClick={() => void runMutation("library-open-location", item.libraryUuid, async () => { if (!await openPath(item.databasePath)) throw new Error("无法打开此文件夹，请检查位置是否可用。"); })}>在文件资源管理器中打开</OverlayMenuItem> : null}
            {item.active ? <>
              <div className="library-switcher-menu-separator" role="separator" />
              {getDesktopBridge()?.pickFolder ? <OverlayMenuItem aria-label="Add folder" icon={<FolderPlus size={16} />} onClick={() => void onAddFolder()}>添加文件夹…</OverlayMenuItem> : null}
              <OverlayMenuItem aria-controls="library-remove-folders" aria-expanded={removeMenuOpen} aria-label="Remove folder" icon={<FolderMinus size={16} />} onClick={(event) => { removeTriggerRef.current = event.currentTarget; dispatch({ anchor: event.currentTarget, kind: "remove", libraryId: item.libraryUuid, type: "toggle-nested" }); }} submenu>移除文件夹…</OverlayMenuItem>
            </> : null}
            <div className="library-switcher-menu-separator" role="separator" />
            <OverlayMenuItem icon={<Minus size={16} />} onClick={(event) => { forgetTriggerRef.current = event.currentTarget; setForgetError(null); setForgetTarget(item); }}>从列表移除…</OverlayMenuItem>
          </OverlayMenuList>
          {error ? <div className="library-switcher-status" role="alert">{error}</div> : null}
        </AnchoredOverlay>
        {item.active ? <AnchoredOverlay anchor={removeMenuAnchor} ariaLabel="移除文件夹" id="library-remove-folders" onDismiss={() => dispatch({ kind: "remove", type: "dismiss-nested" })} open={removeMenuOpen} parentId={menuId} placement="right-start" preferredWidth={232} role="menu" rootId="library" surfaceRole="nested">
          <OverlayMenuList ariaLabel="移除文件夹" className="library-switcher-remove-menu">
            {library.roots.length === 0 ? <div className="library-switcher-remove-menu-empty">没有可移除的文件夹</div> : library.roots.map((root) => <OverlayMenuItem className="library-switcher-remove-menu-item" icon={<FolderMinus size={16} />} key={root.id} onClick={() => dispatch({ name: root.displayName, rootId: root.id, type: "open-remove-confirmation" })} title={root.displayName}>{root.displayName}</OverlayMenuItem>)}
          </OverlayMenuList>
        </AnchoredOverlay> : null}
      </li>
    );
  });

  const footerActions = (
    <>
      <button className="library-switcher-footer-action" data-interactive-pointer-target="true" onClick={openManagedLibraryWizard} type="button"><Library aria-hidden="true" size={18} />创建资源库…</button>
      <button className="library-switcher-footer-action" data-interactive-pointer-target="true" onClick={openExistingLibraryDialog} type="button"><FolderOpen aria-hidden="true" size={18} />打开其它资源库…</button>
      <PluginContributionsSlot slot="library-menu" libraryUuid={libs.find((item) => item.active)?.libraryUuid ?? null} />
    </>
  );

  return (
    <>
      <AnchoredOverlay
        anchor={anchor ?? LIBRARY_SWITCHER_FALLBACK_ANCHOR}
        ariaLabel="资源库"
        autoFocus={autoFocus}
        className="library-switcher"
        id="library-root"
        initialFocusRef={searchRef}
        onDismiss={onClose}
        open={open}
        placement="bottom-start"
        preferredWidth={400}
        restoreFocusTarget={restoreFocusTarget}
        role="dialog"
        rootId="library"
      >
        <div className="library-switcher-header">
          {searchField}
          {closeButton}
        </div>
        <ul aria-busy={loading} aria-label="资源库列表" className="library-switcher-list">{libraryRows}</ul>
        {loading && libs.length === 0 ? <div className="library-switcher-status" role="status">正在加载资源库…</div> : !loading && filtered.length === 0 ? <div className="library-switcher-status" role="status">{query.trim() ? "未找到匹配的资源库" : "还没有资源库"}</div> : null}
        {error && !menu ? <div className="library-switcher-status" role="alert">{error}<button data-interactive-pointer-target="true" onClick={() => void reload()} type="button">重试</button></div> : null}
        <footer className="library-switcher-footer">{footerActions}</footer>
      </AnchoredOverlay>
      {appearanceTarget ? <LibraryAppearanceEditor
        anchor={appearanceRestoreFocusRef.current} color={appearanceTarget.color} icon={appearanceTarget.icon} id="library-appearance-editor" name={appearanceTarget.name} parentId="library-root"
        onApply={async (icon, color) => {
          ensureDesktopMutationSucceeded(await setLibraryIcon(appearanceTarget.libraryUuid, icon, color), "外观保存失败，请重试。");
        }}
        onClose={() => setAppearanceTarget(null)} onFailure={(cause) => reportFailure("library-set-icon", appearanceTarget.libraryUuid, cause)}
        open={open} restoreFocusRef={appearanceRestoreFocusRef} rootId="library"
      /> : null}
      <ModalSurface
        ariaDescribedBy="library-remove-confirm-description"
        ariaLabelledBy="library-remove-confirm-title"
        busy={removePending}
        id="library-remove-confirm"
        initialFocusRef={cancelRef}
        onDismiss={() => {
          if (removePendingRef.current === null) dispatch({ type: "cancel-remove" });
        }}
        open={removeTarget !== null}
        restoreFocusRef={removeTriggerRef}
        role="alertdialog"
        rootId="library"
      >
        <OverlayDialogHeader
          description={REMOVE_ROOT_CONFIRM.body}
          descriptionId="library-remove-confirm-description"
          title={removeTarget ? REMOVE_ROOT_CONFIRM.title(removeTarget.name) : ""}
          titleId="library-remove-confirm-title"
        />
        <OverlayDialogActions>
          <LiquidGlassButton
            disabled={removePending}
            onClick={() => dispatch({ type: "cancel-remove" })}
            ref={cancelRef}
            type="button"
          >
            取消
          </LiquidGlassButton>
          <LiquidGlassButton
            disabled={removePending}
            onClick={() => void confirmRootRemoval()}
            tone="danger"
            type="button"
          >
            移除
          </LiquidGlassButton>
        </OverlayDialogActions>
      </ModalSurface>
      <ModalSurface ariaDescribedBy="library-forget-description" ariaLabelledBy="library-forget-title" busy={forgetPending} className="library-operation-dialog library-forget-dialog" id="library-forget-confirm" initialFocusRef={forgetCancelRef} onDismiss={() => { if (!forgetPendingRef.current) setForgetTarget(null); }} open={forgetTarget !== null} restoreFocusRef={forgetTriggerRef} role="alertdialog" rootId="library">
        <OverlayDialogHeader title="从列表移除资源库？" titleId="library-forget-title" />
        <OverlayDialogBody data-overlay-scroll-container>
          <p className="library-operation-name" title={forgetTarget?.name}>{forgetTarget?.name}</p>
          <p id="library-forget-description">仅从列表移除，资源库中的文件和数据会保留。以后可通过“打开其它资源库”重新打开。{forgetTarget?.bindingState !== "local-only" ? "已启用的同步仍会继续。" : ""}</p>
          {forgetTarget?.active ? <p>当前资源库将关闭。</p> : null}
          {forgetError ? <OverlayInlineStatus tone="error">{forgetError}</OverlayInlineStatus> : null}
        </OverlayDialogBody>
        <OverlayDialogActions>
          <LiquidGlassButton disabled={forgetPending} onClick={() => setForgetTarget(null)} ref={forgetCancelRef} type="button">取消</LiquidGlassButton>
          <LiquidGlassButton disabled={forgetPending} onClick={() => void confirmForget()} tone="primary" type="button">{forgetPending ? "正在移除…" : "从列表移除"}</LiquidGlassButton>
        </OverlayDialogActions>
      </ModalSurface>
      <OpenLibraryDialog onClose={() => { setOpenDialog(false); overlays.closeRoot("managed-library"); }} onFailure={(cause) => void reportFailure("library-open", "managed-root", cause)} onOpened={() => { setFirstRun(false); void reload(); }} open={openDialog} restoreFocusRef={wizardRestoreFocusRef} />
      <ManagedLibraryWizard
        firstRun={firstRun}
        onOpenExisting={() => { setWizardOpen(false); setOpenDialog(true); }}
        onClose={closeManagedLibraryWizard}
        onCreated={() => {
          setFirstRun(false);
          void reload();
        }}
        onFailure={(cause) => void reportFailure("library-create", "managed-root", cause)}
        open={wizardOpen}
        restoreFocusRef={wizardRestoreFocusRef}
      />
    </>
  );
}
