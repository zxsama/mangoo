import { useEffect, useReducer, useRef, useState } from "react";
import { Check, FolderMinus, FolderPlus, Search, X } from "lucide-react";
import {
  createLibrary,
  listLibraries,
  openLibrary,
  pickLibraryFile,
  pickNativeFolder,
  pickNewLibraryPath,
  setLibraryIcon,
  switchLibrary,
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
  OverlayDialogHeader,
  OverlayMenuItem,
  OverlayMenuList,
  OVERLAY_VIEWPORT_GUTTER_PX,
  pointOverlayAnchor,
} from "../../design/overlays";
import {
  DEFAULT_LIBRARY_COLOR,
  DEFAULT_LIBRARY_ICON,
  LibraryGlyph,
  LibraryIconPicker,
} from "./libraryGlyph";
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
  if (cause instanceof Error && cause.message) return cause.message;
  return typeof cause === "string" ? cause : String(cause);
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
  const [libs, setLibs] = useState<LibrarySummary[]>([]);
  const [query, setQuery] = useState("");
  const [interaction, dispatch] = useReducer(
    librarySwitcherInteractionReducer,
    INITIAL_LIBRARY_SWITCHER_INTERACTION_STATE,
  );
  const cancelRef = useRef<HTMLButtonElement>(null);
  const removeTriggerRef = useRef<HTMLButtonElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const mountedRef = useRef(false);
  const reloadGenerationRef = useRef(0);
  const sessionGenerationRef = useRef(0);
  const removePendingRef = useRef<number | null>(null);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      reloadGenerationRef.current += 1;
      sessionGenerationRef.current += 1;
    };
  }, []);

  useEffect(() => {
    sessionGenerationRef.current += 1;
    if (open) {
      void reload();
      return;
    }
    reloadGenerationRef.current += 1;
    removePendingRef.current = null;
    dispatch({ type: "close-root" });
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

  async function reload(): Promise<void> {
    const generation = ++reloadGenerationRef.current;
    try {
      const nextLibraries = await listLibraries();
      if (!mountedRef.current || generation !== reloadGenerationRef.current) return;
      setLibs(nextLibraries);
    } catch (cause) {
      if (!mountedRef.current || generation !== reloadGenerationRef.current) return;
      await reportFailure("library-list", "library-registry", cause);
    }
  }

  async function runMutation(
    source: string,
    target: string,
    operation: () => Promise<void>,
  ): Promise<boolean> {
    try {
      await operation();
      return true;
    } catch (cause) {
      await reportFailure(source, target, cause);
      return false;
    }
  }

  const filtered = libs.filter(
    (item) =>
      item.name.toLowerCase().includes(query.toLowerCase()) ||
      item.dbPath.toLowerCase().includes(query.toLowerCase()),
  );
  const removeTarget = interaction.remove.kind === "closed" ? null : interaction.remove;
  const removePending = interaction.remove.kind === "pending";

  async function onSwitch(item: LibrarySummary) {
    if (item.active || !item.available) return;
    onClose();
    await runMutation("library-switch", item.id, async () => {
      ensureDesktopMutationSucceeded(await switchLibrary(item.id), "Library switch failed");
    });
  }

  async function onCreate() {
    let path: string | null;
    try {
      path = await pickNewLibraryPath("library");
    } catch (cause) {
      await reportFailure("library-create", "new-library-path", cause);
      return;
    }
    if (!path) return;
    const name = path.split(/[\\/]/).pop()?.replace(/\.sqlite$/i, "") ?? "library";
    onClose();
    await runMutation("library-create", path, async () => {
      ensureDesktopMutationSucceeded(
        await createLibrary({ name, dbPath: path }),
        "Library creation failed",
      );
    });
  }

  async function onOpen() {
    let path: string | null;
    try {
      path = await pickLibraryFile();
    } catch (cause) {
      await reportFailure("library-open", "library-file-picker", cause);
      return;
    }
    if (!path) return;
    onClose();
    await runMutation("library-open", path, async () => {
      ensureDesktopMutationSucceeded(await openLibrary(path), "Library open failed");
    });
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
    if (!folder || !ownsSession(session)) return;
    const succeeded = await runMutation("library-add-root", folder, () => library.addRoot(folder));
    if (succeeded && ownsSession(session)) void reload();
  }

  async function confirmRootRemoval() {
    if (interaction.remove.kind !== "confirming" || removePendingRef.current !== null) return;
    const session = sessionGenerationRef.current;
    const target = interaction.remove;
    removePendingRef.current = session;
    dispatch({ type: "begin-remove" });
    const succeeded = await runMutation(
      "library-remove-root",
      String(target.rootId),
      () => library.removeRoot(target.rootId),
    );
    if (removePendingRef.current === session) removePendingRef.current = null;
    if (!ownsSession(session)) return;
    if (!succeeded) {
      dispatch({ type: "fail-remove" });
      return;
    }
    dispatch({ type: "complete-remove" });
    void reload();
  }

  async function applyIcon(item: LibrarySummary, icon: string, color: string) {
    const session = sessionGenerationRef.current;
    const succeeded = await runMutation("library-set-icon", item.id, async () => {
      ensureDesktopMutationSucceeded(
        await setLibraryIcon(item.id, icon, color),
        "Library icon update failed",
      );
    });
    if (succeeded && ownsSession(session)) void reload();
  }

  const searchField = (
    <div className="library-switcher-search">
      <Search size={14} aria-hidden="true" />
      <input
        aria-label="搜索资源库"
        onChange={(event) => setQuery(event.target.value)}
        placeholder="搜索…"
        ref={searchRef}
        value={query}
      />
    </div>
  );

  const closeButton = (
    <button
      aria-label="Close"
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
    const iconMenuOpen =
      interaction.nested.kind === "icon" && interaction.nested.libraryId === item.id;
    const removeMenuOpen =
      item.active &&
      interaction.nested.kind === "remove" &&
      interaction.nested.libraryId === item.id;
    const iconMenuAnchor =
      interaction.nested.kind === "icon" && interaction.nested.libraryId === item.id
        ? interaction.nested.anchor
        : null;
    const removeMenuAnchor =
      interaction.nested.kind === "remove" && interaction.nested.libraryId === item.id
        ? interaction.nested.anchor
        : null;
    return (
      <li
        className={`library-switcher-row${item.active ? " active" : ""}${item.available ? "" : " unavailable"}`}
        key={item.id}
      >
        <div className="library-switcher-icon-anchor">
          <button
            aria-controls={`library-icon-${item.id}`}
            aria-expanded={iconMenuOpen}
            aria-haspopup="menu"
            aria-label="更改图标"
            className="library-switcher-icon"
            data-interactive-pointer-target="true"
            onClick={(event) =>
              dispatch({
                anchor: event.currentTarget,
                kind: "icon",
                libraryId: item.id,
                type: "toggle-nested",
              })
            }
            type="button"
          >
            <LibraryGlyph icon={item.icon} color={item.color} size={16} />
          </button>
          <AnchoredOverlay
            anchor={iconMenuAnchor}
            ariaLabel="选择资源库图标"
            id={`library-icon-${item.id}`}
            onDismiss={() => dispatch({ kind: "icon", type: "dismiss-nested" })}
            open={iconMenuOpen}
            parentId="library-root"
            placement="right-start"
            preferredWidth={208}
            role="group"
            rootId="library"
            surfaceRole="nested"
          >
            <LibraryIconPicker
              color={color}
              icon={icon}
              onPick={(nextIcon, nextColor) => void applyIcon(item, nextIcon, nextColor)}
              presentation="overlay"
            />
          </AnchoredOverlay>
        </div>
        <button
          className="library-switcher-main"
          data-interactive-pointer-target={!item.active && item.available ? "true" : undefined}
          onClick={() => void onSwitch(item)}
          type="button"
        >
          <span className="library-switcher-name">{item.name}</span>
          <span className="library-switcher-path">{item.dbPath}</span>
        </button>
        {item.active ? (
          <Check aria-hidden="true" className="library-switcher-check" size={14} />
        ) : null}
        <div className="library-switcher-actions">
          <button
            aria-label="Add folder"
            data-interactive-pointer-target={item.active ? "true" : undefined}
            disabled={!item.active}
            onClick={() => void onAddFolder()}
            type="button"
          >
            <FolderPlus aria-hidden="true" size={14} />
          </button>
          <div className="library-switcher-remove-menu-anchor">
            <button
              aria-controls="library-remove-folders"
              aria-expanded={removeMenuOpen}
              aria-haspopup="menu"
              aria-label="Remove folder"
              data-interactive-pointer-target={item.active ? "true" : undefined}
              disabled={!item.active}
              onClick={(event) =>
                dispatch({
                  anchor: event.currentTarget,
                  kind: "remove",
                  libraryId: item.id,
                  type: "toggle-nested",
                })
              }
              ref={item.active ? removeTriggerRef : undefined}
              type="button"
            >
              <FolderMinus aria-hidden="true" size={14} />
            </button>
            <AnchoredOverlay
              anchor={removeMenuAnchor}
              ariaLabel="移除文件夹"
              id="library-remove-folders"
              onDismiss={() => dispatch({ kind: "remove", type: "dismiss-nested" })}
              open={removeMenuOpen}
              parentId="library-root"
              placement="right-start"
              preferredWidth={220}
              role="menu"
              rootId="library"
              surfaceRole="nested"
            >
              <OverlayMenuList ariaLabel="移除文件夹" className="library-switcher-remove-menu">
                {library.roots.length === 0 ? (
                  <div className="library-switcher-remove-menu-empty">No folders</div>
                ) : (
                  library.roots.map((root) => (
                    <OverlayMenuItem
                      className="library-switcher-remove-menu-item"
                      icon={<FolderMinus size={12} />}
                      key={root.id}
                      onClick={() =>
                        dispatch({
                          name: root.displayName,
                          rootId: root.id,
                          type: "open-remove-confirmation",
                        })
                      }
                    >
                      {root.displayName}
                    </OverlayMenuItem>
                  ))
                )}
              </OverlayMenuList>
            </AnchoredOverlay>
          </div>
        </div>
      </li>
    );
  });

  const footerActions = (
    <>
      <button data-interactive-pointer-target="true" onClick={() => void onCreate()} type="button">
        创建资源库…
      </button>
      <button data-interactive-pointer-target="true" onClick={() => void onOpen()} type="button">
        打开其它资源库…
      </button>
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
        preferredWidth={460}
        restoreFocusTarget={restoreFocusTarget}
        role="dialog"
        rootId="library"
      >
        <div className="library-switcher-header">
          {searchField}
          {closeButton}
        </div>
        <ul className="library-switcher-list">{libraryRows}</ul>
        <footer className="library-switcher-footer">{footerActions}</footer>
      </AnchoredOverlay>
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
          <button
            data-interactive-pointer-target={!removePending ? "true" : undefined}
            disabled={removePending}
            onClick={() => dispatch({ type: "cancel-remove" })}
            ref={cancelRef}
            type="button"
          >
            取消
          </button>
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
    </>
  );
}
