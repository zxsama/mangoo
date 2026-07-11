import { useEffect, useState } from "react";
import { Check, FolderPlus, FolderMinus, Search, X } from "lucide-react";
import { LiquidGlassSurface } from "../../design/liquid-glass";
import {
  listLibraries,
  switchLibrary,
  createLibrary,
  openLibrary,
  pickLibraryFile,
  pickNewLibraryPath,
  pickNativeFolder,
} from "../../core/desktop";
import type { LibrarySummary } from "../../core/libraryTypes";
import type { LibraryState } from "../../core/useLibraryData";

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

  const reload = () => void listLibraries().then(setLibs);
  useEffect(() => {
    if (open) reload();
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

  return (
    <div className="library-switcher-backdrop" onClick={onClose}>
      <LiquidGlassSurface
        as="div"
        className="library-switcher"
        tone="elevated"
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
          {filtered.map((l) => (
            <li
              key={l.id}
              className={`library-switcher-row${l.active ? " active" : ""}${l.available ? "" : " unavailable"}`}
            >
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
                <button
                  type="button"
                  aria-label="Remove folder"
                  disabled={!l.active}
                  onClick={() => {
                    // Wired to a root-picker submenu in the integration task.
                  }}
                >
                  <FolderMinus size={14} />
                </button>
              </div>
            </li>
          ))}
        </ul>
        <div className="library-switcher-footer">
          <button type="button" onClick={onCreate}>
            创建资源库…
          </button>
          <button type="button" onClick={onOpen}>
            打开其它资源库…
          </button>
        </div>
      </LiquidGlassSurface>
    </div>
  );
}
