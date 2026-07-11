import { useEffect, useState } from "react";
import { Check, FolderPlus, FolderMinus, Search, X } from "lucide-react";
import {
  listLibraries,
  switchLibrary,
  createLibrary,
  openLibrary,
  pickLibraryFile,
  pickNewLibraryPath,
  pickNativeFolder,
  setLibraryIcon,
} from "../../core/desktop";
import type { LibrarySummary } from "../../core/libraryTypes";
import type { LibraryState } from "../../core/useLibraryData";
import {
  LibraryGlyph,
  LibraryIconPicker,
  DEFAULT_LIBRARY_ICON,
  DEFAULT_LIBRARY_COLOR,
} from "./libraryGlyph";

export function LibrarySwitcher({
  open,
  onClose,
  library,
}: {
  open: boolean;
  onClose: () => void;
  library: LibraryState;
}) {
  const [libs, setLibs] = useState<LibrarySummary[]>([]);
  const [query, setQuery] = useState("");
  const [removeMenuLibraryId, setRemoveMenuLibraryId] = useState<string | null>(null);
  const [iconMenuLibraryId, setIconMenuLibraryId] = useState<string | null>(null);

  const reload = () => void listLibraries().then(setLibs);
  useEffect(() => {
    if (open) {
      reload();
    } else {
      setRemoveMenuLibraryId(null);
      setIconMenuLibraryId(null);
    }
  }, [open]);
  if (!open) return null;

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

  async function onRemoveRoot(rootId: number) {
    await library.removeRoot(rootId);
    setRemoveMenuLibraryId(null);
    reload();
  }

  async function applyIcon(l: LibrarySummary, icon: string, color: string) {
    await setLibraryIcon(l.id, icon, color);
    reload();
  }

  return (
    <div className="library-switcher-backdrop" onClick={onClose}>
      <div
        className="library-switcher"
        role="dialog"
        aria-label="资源库"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="library-switcher-search">
          <Search size={14} aria-hidden="true" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索…"
          />
          <button type="button" aria-label="Close" onClick={onClose}>
            <X size={14} />
          </button>
        </div>
        <ul className="library-switcher-list">
          {filtered.map((l) => {
            const icon = l.icon ?? DEFAULT_LIBRARY_ICON;
            const color = l.color ?? DEFAULT_LIBRARY_COLOR;
            return (
              <li
                key={l.id}
                className={`library-switcher-row${l.active ? " active" : ""}${l.available ? "" : " unavailable"}`}
              >
                <div className="library-switcher-icon-anchor">
                  <button
                    type="button"
                    className="library-switcher-icon"
                    aria-label="更改图标"
                    onClick={() =>
                      setIconMenuLibraryId((current) => (current === l.id ? null : l.id))
                    }
                  >
                    <LibraryGlyph icon={l.icon} color={l.color} size={16} />
                  </button>
                  {iconMenuLibraryId === l.id ? (
                    <LibraryIconPicker
                      icon={icon}
                      color={color}
                      onPick={(nextIcon, nextColor) => void applyIcon(l, nextIcon, nextColor)}
                    />
                  ) : null}
                </div>
                <button
                  type="button"
                  className="library-switcher-main"
                  onClick={() => onSwitch(l)}
                >
                  <span className="library-switcher-name">{l.name}</span>
                  <span className="library-switcher-path">{l.dbPath}</span>
                </button>
                {l.active ? (
                  <Check
                    size={14}
                    className="library-switcher-check"
                    aria-hidden="true"
                  />
                ) : null}
                <div className="library-switcher-actions">
                  <button
                    type="button"
                    aria-label="Add folder"
                    disabled={!l.active}
                    onClick={onAddFolder}
                  >
                    <FolderPlus size={14} />
                  </button>
                  <div className="library-switcher-remove-menu-anchor">
                    <button
                      type="button"
                      aria-label="Remove folder"
                      disabled={!l.active}
                      onClick={() =>
                        setRemoveMenuLibraryId((current) => (current === l.id ? null : l.id))
                      }
                    >
                      <FolderMinus size={14} />
                    </button>
                    {l.active && removeMenuLibraryId === l.id ? (
                      <ul className="library-switcher-remove-menu">
                        {library.roots.length === 0 ? (
                          <li className="library-switcher-remove-menu-empty">No folders</li>
                        ) : (
                          library.roots.map((root) => (
                            <li key={root.id}>
                              <button
                                type="button"
                                className="library-switcher-remove-menu-item"
                                onClick={() => void onRemoveRoot(root.id)}
                              >
                                <span>{root.displayName}</span>
                                <FolderMinus size={12} aria-hidden="true" />
                              </button>
                            </li>
                          ))
                        )}
                      </ul>
                    ) : null}
                  </div>
                </div>
              </li>
            );
          })}
        </ul>
        <div className="library-switcher-footer">
          <button type="button" onClick={onCreate}>
            创建资源库…
          </button>
          <button type="button" onClick={onOpen}>
            打开其它资源库…
          </button>
        </div>
      </div>
    </div>
  );
}
