import {
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  FolderPlus,
  RefreshCw,
  RotateCw
} from "lucide-react";
import type { CSSProperties } from "react";
import { FormEvent, useState } from "react";
import type { FolderRecord, RootRecord } from "../../core/types";
import { canPickNativeFolder, pickNativeFolder } from "../../core/desktop";
import type { LibraryState } from "../../core/useLibraryData";

interface LibrarySidebarProps {
  library: LibraryState;
  onRootContextMenu?: (event: {
    root: RootRecord;
    x: number;
    y: number;
    shiftKey: boolean;
  }) => void;
  onFolderContextMenu?: (event: {
    folder: FolderRecord;
    x: number;
    y: number;
    shiftKey: boolean;
  }) => void;
}

export function LibrarySidebar({
  library,
  onFolderContextMenu,
  onRootContextMenu
}: LibrarySidebarProps) {
  const [rootPath, setRootPath] = useState("");
  const canPickFolder = canPickNativeFolder();
  const selectedRootId = library.selectedRootId;
  const selectedRoot = library.roots.find((root) => root.id === selectedRootId) ?? null;
  const canRescan = selectedRootId !== null;
  const rescanning = selectedRootId !== null && library.rescanningRootIds.has(selectedRootId);

  async function submitRoot(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await library.addRoot(rootPath);
    setRootPath("");
  }

  async function chooseFolder() {
    const folder = await pickNativeFolder();
    if (folder) {
      setRootPath(folder);
    }
  }

  async function rescanSelectedRoot() {
    if (selectedRootId === null) return;
    await library.rescanRoot(selectedRootId);
  }

  return (
    <aside
      className="library-sidebar"
      aria-label="Library folders"
    >
      <div className="sidebar-heading">
        <div>
          <div className="panel-title">Library</div>
          <div className="panel-subtitle">{library.roots.length} roots</div>
        </div>
        <button
          className="icon-button"
          onClick={() => void library.refresh()}
          title="Refresh"
          type="button"
          aria-label="Refresh library"
        >
          <RefreshCw size={16} />
        </button>
      </div>

      <form className="root-form" onSubmit={submitRoot}>
        <input
          aria-label="Root path"
          disabled={library.addingRoot}
          onChange={(event) => setRootPath(event.target.value)}
          placeholder="D:\\Pictures"
          value={rootPath}
        />
        <button
          className="icon-button"
          disabled={!canPickFolder || library.addingRoot}
          onClick={() => void chooseFolder()}
          title={canPickFolder ? "Choose folder" : "Use path input"}
          type="button"
          aria-label="Choose folder"
        >
          <FolderPlus size={16} />
        </button>
        <button disabled={library.addingRoot || rootPath.trim().length === 0} type="submit">
          Add
        </button>
        <button
          className="icon-button"
          disabled={!canRescan || rescanning}
          onClick={() => void rescanSelectedRoot()}
          title={
            canRescan
              ? `Rescan ${selectedRoot?.displayName ?? "selected root"}`
              : "Select a root to rescan"
          }
          type="button"
          aria-label="Rescan selected root"
        >
          <RotateCw size={16} />
        </button>
      </form>

      <div className="tree-list" role="tree" aria-label="Folder tree">
        {library.roots.map((root) => (
          <RootNode
            key={root.id}
            library={library}
            onFolderContextMenu={onFolderContextMenu}
            onRootContextMenu={onRootContextMenu}
            root={root}
          />
        ))}
        {library.roots.length === 0 ? <div className="empty-panel">No roots</div> : null}
      </div>
    </aside>
  );
}

function RootNode({
  library,
  root,
  onFolderContextMenu,
  onRootContextMenu
}: {
  library: LibraryState;
  root: RootRecord;
  onRootContextMenu?: (event: {
    root: RootRecord;
    x: number;
    y: number;
    shiftKey: boolean;
  }) => void;
  onFolderContextMenu?: (event: {
    folder: FolderRecord;
    x: number;
    y: number;
    shiftKey: boolean;
  }) => void;
}) {
  const rootFolderId = root.rootFolderId;
  const expanded = rootFolderId ? library.expandedFolderIds.has(rootFolderId) : false;
  const children = rootFolderId ? library.folderChildrenByParent[rootFolderId] ?? [] : [];
  const selected = root.id === library.selectedRootId && library.selectedFolderId === rootFolderId;

  return (
    <div className="tree-branch">
      <div
        className={selected ? "tree-item selected" : "tree-item"}
        data-depth="0"
        data-folder-id={rootFolderId ?? undefined}
        data-root-id={root.id}
        role="treeitem"
        aria-expanded={rootFolderId ? expanded : undefined}
        aria-selected={selected}
        onContextMenu={(event) => {
          if (!onRootContextMenu) return;
          event.preventDefault();
          onRootContextMenu({
            root,
            x: event.clientX,
            y: event.clientY,
            shiftKey: event.shiftKey
          });
        }}
      >
        <button
          className="tree-disclosure"
          disabled={!rootFolderId}
          onClick={() => rootFolderId && library.toggleFolderExpanded(rootFolderId)}
          type="button"
          aria-label={expanded ? "Collapse root" : "Expand root"}
        >
          {expanded ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
        </button>
        <button
          className="tree-label"
          onClick={() => library.setSelectedRootId(root.id)}
          type="button"
        >
          {expanded ? <FolderOpen size={16} /> : <Folder size={16} />}
          <span>{root.displayName}</span>
          <small>{root.rootFolderId ? "indexed" : "pending"}</small>
        </button>
      </div>
      {expanded ? (
        <div role="group">
          {children.map((folder) => (
            <FolderNode
              depth={1}
              folder={folder}
              key={folder.id}
              library={library}
              onFolderContextMenu={onFolderContextMenu}
            />
          ))}
          <LoadMoreChildren folderId={rootFolderId} library={library} />
        </div>
      ) : null}
    </div>
  );
}

function FolderNode({
  depth,
  folder,
  library,
  onFolderContextMenu
}: {
  depth: number;
  folder: FolderRecord;
  library: LibraryState;
  onFolderContextMenu?: (event: {
    folder: FolderRecord;
    x: number;
    y: number;
    shiftKey: boolean;
  }) => void;
}) {
  const expanded = library.expandedFolderIds.has(folder.id);
  const children = library.folderChildrenByParent[folder.id] ?? [];
  const selected = library.selectedFolderId === folder.id;
  const loading = library.loadingFolderIds.has(folder.id);
  // Hide the expand chevron for folders we KNOW have no subfolders (their
  // children have been loaded and the list is empty). Until a folder's children
  // are loaded we can't tell, so the chevron shows (clicking it loads them and
  // then hides it if empty). A folder currently loading keeps the chevron.
  const knownChildless =
    library.folderChildrenByParent[folder.id] !== undefined && children.length === 0 && !loading;

  return (
    <div className="tree-branch">
      <div
        className={selected ? "tree-item selected" : "tree-item"}
        data-depth={depth}
        data-folder-id={folder.id}
        data-root-id={folder.rootId}
        role="treeitem"
        aria-expanded={expanded}
        aria-selected={selected}
        onContextMenu={(event) => {
          if (!onFolderContextMenu) return;
          event.preventDefault();
          onFolderContextMenu({
            folder,
            x: event.clientX,
            y: event.clientY,
            shiftKey: event.shiftKey
          });
        }}
        style={{ "--tree-depth": String(depth) } as CSSProperties}
      >
        {knownChildless ? (
          <span className="tree-disclosure tree-disclosure--leaf" aria-hidden="true" />
        ) : (
          <button
            className="tree-disclosure"
            onClick={() => library.toggleFolderExpanded(folder.id)}
            type="button"
            aria-label={expanded ? "Collapse folder" : "Expand folder"}
          >
            {expanded ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
          </button>
        )}
        <button
          className="tree-label"
          onClick={() => library.setSelectedFolder(folder)}
          type="button"
        >
          {expanded ? <FolderOpen size={16} /> : <Folder size={16} />}
          <span>{folder.name}</span>
          <small aria-busy={loading}>{folder.status}</small>
        </button>
      </div>
      {expanded ? (
        <div role="group">
          {children.map((child) => (
            <FolderNode
              depth={depth + 1}
              folder={child}
              key={child.id}
              library={library}
              onFolderContextMenu={onFolderContextMenu}
            />
          ))}
          <LoadMoreChildren depth={depth + 1} folderId={folder.id} library={library} />
        </div>
      ) : null}
    </div>
  );
}

function LoadMoreChildren({
  depth = 1,
  folderId,
  library
}: {
  depth?: number;
  folderId: number | null;
  library: LibraryState;
}) {
  if (!folderId || !library.folderChildNextCursorByParent[folderId]) {
    return null;
  }

  const loading = library.loadingMoreFolderIds.has(folderId);

  return (
    <button
      className="tree-load-more"
      disabled={loading}
      onClick={() => void library.loadMoreFolderChildren(folderId)}
      style={{ "--tree-depth": String(depth) } as CSSProperties}
      type="button"
    >
      Load more children
    </button>
  );
}
