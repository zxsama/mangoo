import { useEffect, useRef, useState } from "react";
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

const LIBRARY_SWITCHER_FALLBACK_ANCHOR = pointOverlayAnchor(
  OVERLAY_VIEWPORT_GUTTER_PX,
  OVERLAY_VIEWPORT_GUTTER_PX,
);

export interface LibrarySwitcherProps {
  anchor: HTMLElement | null;
  library: LibraryState;
  onClose: () => void;
  open: boolean;
}

export function LibrarySwitcher({
  anchor,
  library,
  onClose,
  open,
}: LibrarySwitcherProps) {
  const [libs, setLibs] = useState<LibrarySummary[]>([]);
  const [query, setQuery] = useState("");
  const [removeMenuLibraryId, setRemoveMenuLibraryId] = useState<string | null>(null);
  const [removeMenuAnchor, setRemoveMenuAnchor] = useState<HTMLButtonElement | null>(null);
  const [iconMenuLibraryId, setIconMenuLibraryId] = useState<string | null>(null);
  const [iconMenuAnchor, setIconMenuAnchor] = useState<HTMLButtonElement | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<{ rootId: number; name: string } | null>(
    null,
  );
  const cancelRef = useRef<HTMLButtonElement>(null);
  const removeTriggerRef = useRef<HTMLButtonElement>(null);

  const reload = () => void listLibraries().then(setLibs);
  useEffect(() => {
    if (open) {
      reload();
    } else {
      setRemoveMenuLibraryId(null);
      setRemoveMenuAnchor(null);
      setIconMenuLibraryId(null);
      setIconMenuAnchor(null);
      setConfirmRemove(null);
    }
  }, [open]);

  const filtered = libs.filter(
    (l) =>
      l.name.toLowerCase().includes(query.toLowerCase()) ||
      l.dbPath.toLowerCase().includes(query.toLowerCase()),
  );

  async function onSwitch(l: LibrarySummary) {
    if (l.active || !l.available) return;
    onClose();
    await switchLibrary(l.id);
  }

  async function onCreate() {
    const p = await pickNewLibraryPath("library");
    if (!p) return;
    const name = p.split(/[\\/]/).pop()?.replace(/\.sqlite$/i, "") ?? "library";
    onClose();
    await createLibrary({ name, dbPath: p });
  }

  async function onOpen() {
    const p = await pickLibraryFile();
    if (!p) return;
    onClose();
    await openLibrary(p);
  }

  async function onAddFolder() {
    const folder = await pickNativeFolder();
    if (folder) await library.addRoot(folder);
    reload();
  }

  async function confirmRootRemoval() {
    if (!confirmRemove) return;
    await library.removeRoot(confirmRemove.rootId);
    setConfirmRemove(null);
    setRemoveMenuLibraryId(null);
    reload();
  }

  async function applyIcon(l: LibrarySummary, icon: string, color: string) {
    await setLibraryIcon(l.id, icon, color);
    reload();
  }

  const searchField = (
    <div className="library-switcher-search">
      <Search size={14} aria-hidden="true" />
      <input
        aria-label="搜索资源库"
        onChange={(event) => setQuery(event.target.value)}
        placeholder="搜索…"
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

  const libraryRows = filtered.map((l) => {
    const icon = l.icon ?? DEFAULT_LIBRARY_ICON;
    const color = l.color ?? DEFAULT_LIBRARY_COLOR;
    const iconMenuOpen = iconMenuLibraryId === l.id;
    const removeMenuOpen = l.active && removeMenuLibraryId === l.id;
    return (
      <li
        className={`library-switcher-row${l.active ? " active" : ""}${l.available ? "" : " unavailable"}`}
        key={l.id}
      >
        <div className="library-switcher-icon-anchor">
          <button
            aria-controls={`library-icon-${l.id}`}
            aria-expanded={iconMenuOpen}
            aria-haspopup="menu"
            aria-label="更改图标"
            className="library-switcher-icon"
            data-interactive-pointer-target="true"
            onClick={(event) => {
              const nextOpen = iconMenuLibraryId !== l.id;
              setIconMenuAnchor(nextOpen ? event.currentTarget : null);
              setIconMenuLibraryId(nextOpen ? l.id : null);
            }}
            type="button"
          >
            <LibraryGlyph icon={l.icon} color={l.color} size={16} />
          </button>
          <AnchoredOverlay
            anchor={iconMenuAnchor}
            ariaLabel="选择资源库图标"
            id={`library-icon-${l.id}`}
            onDismiss={() => {
              setIconMenuLibraryId(null);
              setIconMenuAnchor(null);
            }}
            open={iconMenuOpen}
            parentId="library-root"
            placement="right-start"
            preferredWidth={208}
            role="menu"
            rootId="library"
            surfaceRole="nested"
          >
            <LibraryIconPicker
              color={color}
              icon={icon}
              onPick={(nextIcon, nextColor) => void applyIcon(l, nextIcon, nextColor)}
              presentation="overlay"
            />
          </AnchoredOverlay>
        </div>
        <button
          className="library-switcher-main"
          data-interactive-pointer-target={!l.active && l.available ? "true" : undefined}
          onClick={() => onSwitch(l)}
          type="button"
        >
          <span className="library-switcher-name">{l.name}</span>
          <span className="library-switcher-path">{l.dbPath}</span>
        </button>
        {l.active ? (
          <Check aria-hidden="true" className="library-switcher-check" size={14} />
        ) : null}
        <div className="library-switcher-actions">
          <button
            aria-label="Add folder"
            data-interactive-pointer-target={l.active ? "true" : undefined}
            disabled={!l.active}
            onClick={onAddFolder}
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
              data-interactive-pointer-target={l.active ? "true" : undefined}
              disabled={!l.active}
              onClick={(event) => {
                const nextOpen = removeMenuLibraryId !== l.id;
                setRemoveMenuAnchor(nextOpen ? event.currentTarget : null);
                setRemoveMenuLibraryId(nextOpen ? l.id : null);
              }}
              ref={l.active ? removeTriggerRef : undefined}
              type="button"
            >
              <FolderMinus aria-hidden="true" size={14} />
            </button>
            <AnchoredOverlay
              anchor={removeMenuAnchor}
              ariaLabel="移除文件夹"
              id="library-remove-folders"
              onDismiss={() => {
                setRemoveMenuLibraryId(null);
                setRemoveMenuAnchor(null);
              }}
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
                        setConfirmRemove({ rootId: root.id, name: root.displayName })
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
      <button data-interactive-pointer-target="true" onClick={onCreate} type="button">
        创建资源库…
      </button>
      <button data-interactive-pointer-target="true" onClick={onOpen} type="button">
        打开其它资源库…
      </button>
    </>
  );

  return (
    <>
      <AnchoredOverlay
        anchor={anchor ?? LIBRARY_SWITCHER_FALLBACK_ANCHOR}
        ariaLabel="资源库"
        className="library-switcher"
        id="library-root"
        onDismiss={onClose}
        open={open}
        placement="bottom-start"
        preferredWidth={460}
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
        id="library-remove-confirm"
        initialFocusRef={cancelRef}
        onDismiss={() => setConfirmRemove(null)}
        open={confirmRemove !== null}
        restoreFocusRef={removeTriggerRef}
        role="alertdialog"
        rootId="library"
      >
        <OverlayDialogHeader
          description={REMOVE_ROOT_CONFIRM.body}
          descriptionId="library-remove-confirm-description"
          title={confirmRemove ? REMOVE_ROOT_CONFIRM.title(confirmRemove.name) : ""}
          titleId="library-remove-confirm-title"
        />
        <OverlayDialogActions>
          <button
            data-interactive-pointer-target="true"
            onClick={() => setConfirmRemove(null)}
            ref={cancelRef}
            type="button"
          >
            取消
          </button>
          <LiquidGlassButton
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
