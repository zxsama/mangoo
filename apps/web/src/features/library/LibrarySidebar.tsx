import {
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  FolderX
} from "lucide-react";
import type { CSSProperties, RefObject } from "react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { folderShowsOpenIcon, isFolderKnownChildless } from "../../core/folderTreeState";
import type { FolderRecord, RootRecord } from "../../core/types";
import type { LibraryState } from "../../core/useLibraryData";

export interface LibrarySidebarLocateRequest {
  folderId: number;
  requestId: number | string;
}

interface LibrarySidebarProps {
  library: LibraryState;
  locateRequest?: LibrarySidebarLocateRequest;
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

const TREE_STICKY_ROW_HEIGHT = 32;
const TREE_STICKY_TOLERANCE = 1;

export function LibrarySidebar({
  library,
  locateRequest,
  onFolderContextMenu,
  onRootContextMenu
}: LibrarySidebarProps) {
  const sidebarRef = useRef<HTMLElement | null>(null);
  const [locateTarget, setLocateTarget] = useState<LibrarySidebarLocateRequest | null>(null);
  const revealFolderPath = library.revealFolderPath;
  useTreeStickyOcclusion(sidebarRef);

  useEffect(() => {
    if (!locateRequest) return;
    let cancelled = false;
    void revealFolderPath(locateRequest.folderId)
      .then((path) => {
        if (!cancelled && path.length > 0) {
          setLocateTarget({ ...locateRequest });
        }
      })
      .catch(() => {
        // The library state owns the error; failed location must not select a row.
      });
    return () => {
      cancelled = true;
    };
  }, [locateRequest?.folderId, locateRequest?.requestId, revealFolderPath]);

  useLayoutEffect(() => {
    if (!locateTarget) return;
    const sidebar = sidebarRef.current;
    if (!sidebar) return;

    let frame = 0;
    let attempts = 0;
    let clearHighlight: () => void = () => undefined;
    const locate = () => {
      frame = 0;
      const row = sidebar.querySelector<HTMLElement>(
        `.tree-item[data-folder-id="${locateTarget.folderId}"]`
      );
      if (!row) {
        if (attempts < 8) {
          attempts += 1;
          frame = window.requestAnimationFrame(locate);
        }
        return;
      }

      const sidebarRect = sidebar.getBoundingClientRect();
      const rowRect = row.getBoundingClientRect();
      const centeredTop =
        sidebar.scrollTop +
        rowRect.top -
        sidebarRect.top -
        Math.max(0, (sidebar.clientHeight - rowRect.height) / 2);
      const maxScrollTop = Math.max(0, sidebar.scrollHeight - sidebar.clientHeight);
      sidebar.scrollTo({
        top: Math.max(0, Math.min(maxScrollTop, centeredTop)),
        behavior: "smooth"
      });
      const timeout = window.setTimeout(() => setLocateTarget(null), 900);
      clearHighlight = () => window.clearTimeout(timeout);
    };

    frame = window.requestAnimationFrame(locate);
    return () => {
      if (frame) window.cancelAnimationFrame(frame);
      clearHighlight();
    };
  }, [locateTarget]);

  return (
    <aside
      className="library-sidebar"
      ref={sidebarRef}
      aria-label="Library folders"
    >
      <div className="tree-list" role="tree" aria-label="Folder tree">
        {library.roots.map((root) => (
          <RootNode
            key={root.id}
            library={library}
            onFolderContextMenu={onFolderContextMenu}
            onRootContextMenu={onRootContextMenu}
            root={root}
            highlightedFolderId={locateTarget?.folderId ?? null}
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
  highlightedFolderId,
  onFolderContextMenu,
  onRootContextMenu
}: {
  library: LibraryState;
  root: RootRecord;
  highlightedFolderId: number | null;
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
        data-tree-locate-highlight={
          rootFolderId !== null && rootFolderId === highlightedFolderId ? "true" : undefined
        }
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
        style={treeItemStyle(0)}
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
          {root.reachable === false ? (
            <FolderX size={16} className="tree-root-offline" aria-label="资源库离线" />
          ) : expanded ? (
            <FolderOpen size={16} />
          ) : (
            <Folder size={16} />
          )}
          <span>{root.displayName}</span>
        </button>
        <span
          aria-label={`${root.mediaCount} media files`}
          className="tree-media-count"
          title={`${root.mediaCount.toLocaleString()} media files including subfolders`}
        >
          {root.mediaCount.toLocaleString()}
        </span>
      </div>
      {expanded ? (
        <div role="group">
          {children.map((folder) => (
            <FolderNode
              depth={1}
              folder={folder}
              key={folder.id}
              library={library}
              highlightedFolderId={highlightedFolderId}
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
  highlightedFolderId,
  library,
  onFolderContextMenu
}: {
  depth: number;
  folder: FolderRecord;
  highlightedFolderId: number | null;
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
  // Hide the expand chevron for folders with no subfolders. Core reports
  // `hasChildren` on every folder row, so a childless folder renders a leaf
  // marker up front — no chevron that flashes in and then vanishes once an
  // empty child page loads. The "loaded and empty" clause is a fallback that
  // also collapses the affordance if a stale `hasChildren: true` turns out empty
  // after loading. A folder still loading keeps the chevron.
  const knownChildless = isFolderKnownChildless({
    hasChildren: folder.hasChildren,
    childrenLoaded: library.folderChildrenByParent[folder.id] !== undefined,
    loadedChildCount: children.length,
    loading
  });
  const showsOpenIcon = folderShowsOpenIcon({ expanded, knownChildless, selected });

  return (
    <div className="tree-branch">
      <div
        className={selected ? "tree-item selected" : "tree-item"}
        data-depth={depth}
        data-folder-id={folder.id}
        data-root-id={folder.rootId}
        data-tree-locate-highlight={folder.id === highlightedFolderId ? "true" : undefined}
        role="treeitem"
        aria-expanded={knownChildless ? undefined : expanded}
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
        style={treeItemStyle(depth)}
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
          {showsOpenIcon ? <FolderOpen size={16} /> : <Folder size={16} />}
          <span>{folder.name}</span>
        </button>
        <span
          aria-label={`${folder.mediaCount} media files`}
          className="tree-media-count"
          title={`${folder.mediaCount.toLocaleString()} media files including subfolders`}
        >
          {folder.mediaCount.toLocaleString()}
        </span>
      </div>
      {expanded && !knownChildless ? (
        <div role="group">
          {children.map((child) => (
            <FolderNode
              depth={depth + 1}
              folder={child}
              key={child.id}
              library={library}
              highlightedFolderId={highlightedFolderId}
              onFolderContextMenu={onFolderContextMenu}
            />
          ))}
          <LoadMoreChildren depth={depth + 1} folderId={folder.id} library={library} />
        </div>
      ) : null}
    </div>
  );
}

function treeItemStyle(depth: number): CSSProperties {
  return {
    "--tree-depth": String(depth),
    "--tree-sticky-offset": "0px",
    "--tree-sticky-z": String(80 - Math.min(depth, 60))
  } as CSSProperties;
}

function useTreeStickyOcclusion(sidebarRef: RefObject<HTMLElement | null>): void {
  useLayoutEffect(() => {
    const sidebar = sidebarRef.current;
    if (!sidebar) return;
    updateTreeStickyOcclusion(sidebar);
  });

  useEffect(() => {
    const sidebar = sidebarRef.current;
    if (!sidebar) return;

    let frame = 0;
    const scheduleUpdate = () => {
      if (frame) return;
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        updateTreeStickyOcclusion(sidebar);
      });
    };

    sidebar.addEventListener("scroll", scheduleUpdate, { passive: true });
    window.addEventListener("resize", scheduleUpdate);

    const resizeObserver =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(scheduleUpdate);
    resizeObserver?.observe(sidebar);
    const tree = sidebar.querySelector<HTMLElement>(".tree-list");
    if (tree) {
      resizeObserver?.observe(tree);
    }

    scheduleUpdate();

    return () => {
      sidebar.removeEventListener("scroll", scheduleUpdate);
      window.removeEventListener("resize", scheduleUpdate);
      resizeObserver?.disconnect();
      if (frame) {
        window.cancelAnimationFrame(frame);
      }
      clearTreeStickyOcclusion(sidebar);
    };
  }, [sidebarRef]);
}

function updateTreeStickyOcclusion(sidebar: HTMLElement): void {
  const tree = sidebar.querySelector<HTMLElement>(".tree-list");
  if (!tree) return;

  const rows = Array.from(tree.querySelectorAll<HTMLElement>(".tree-item[role='treeitem']"));
  if (rows.length === 0) return;

  const sidebarRect = sidebar.getBoundingClientRect();
  const stickyBaseTop = sidebarRect.top;
  const layouts = rows.map(resolveTreeRowLayout);
  const pinnedLayouts = resolvePinnedTreeRows(layouts, stickyBaseTop);

  if (pinnedLayouts.length === 0) {
    rows.forEach(clearTreeRowState);
    return;
  }

  const occlusionTop = pinnedLayouts.reduce(
    (top, layout) => Math.min(top, layout.actualTop),
    pinnedLayouts[0]?.actualTop ?? stickyBaseTop
  );
  const occlusionBottom = pinnedLayouts.reduce(
    (bottom, layout) => Math.max(bottom, layout.actualBottom),
    stickyBaseTop
  );
  const pinnedRows = new Set(pinnedLayouts.map((layout) => layout.row));
  const pinnedClipTopByRow = resolvePinnedClipTops(pinnedLayouts);

  layouts.forEach(({ row }) => {
    if (pinnedRows.has(row)) {
      applyTreeRowClip(row, pinnedClipTopByRow.get(row) ?? 0);
      return;
    }

    const rowRect = row.getBoundingClientRect();
    if (rowRect.bottom <= occlusionTop || rowRect.top >= occlusionBottom) {
      clearTreeRowOcclusion(row);
      return;
    }

    const clippedTop = Math.max(0, Math.min(rowRect.height, occlusionBottom - rowRect.top));
    applyTreeRowClip(row, clippedTop);
  });
}

type TreeRowLayout = {
  branchBottom: number;
  currentTop: number;
  height: number;
  row: HTMLElement;
};

type PinnedTreeRowLayout = TreeRowLayout & {
  actualBottom: number;
  actualTop: number;
};

function resolveTreeRowLayout(row: HTMLElement): TreeRowLayout {
  const branch = row.closest<HTMLElement>(".tree-branch");
  const rowRect = row.getBoundingClientRect();
  const height = row.offsetHeight || TREE_STICKY_ROW_HEIGHT;
  return {
    branchBottom: branch
      ? branch.getBoundingClientRect().bottom
      : Number.POSITIVE_INFINITY,
    currentTop: rowRect.top,
    height,
    row
  };
}

function resolvePinnedTreeRows(
  layouts: TreeRowLayout[],
  stickyBaseTop: number
): PinnedTreeRowLayout[] {
  const pinned: PinnedTreeRowLayout[] = [];
  let nextTop = stickyBaseTop;

  layouts.forEach((layout) => {
    const { height, row } = layout;
    if (row.getAttribute("aria-expanded") !== "true") {
      clearTreeRowPinned(row);
      return;
    }

    if (layout.currentTop > nextTop + TREE_STICKY_TOLERANCE) {
      clearTreeRowPinned(row);
      return;
    }

    const latestTop = layout.branchBottom - height;
    if (latestTop <= stickyBaseTop - height + TREE_STICKY_TOLERANCE) {
      clearTreeRowPinned(row);
      return;
    }

    const actualTop = Math.min(nextTop, latestTop);
    const actualBottom = actualTop + height;
    row.style.setProperty("--tree-sticky-offset", `${Math.round(actualTop - stickyBaseTop)}px`);
    row.dataset.treeStickyPinned = "true";
    pinned.push({ ...layout, actualBottom, actualTop });
    nextTop = actualBottom;
  });

  return pinned;
}

function clearTreeRowPinned(row: HTMLElement): void {
  row.removeAttribute("data-tree-sticky-pinned");
  row.style.setProperty("--tree-sticky-offset", "0px");
}

function resolvePinnedClipTops(pinnedLayouts: PinnedTreeRowLayout[]): Map<HTMLElement, number> {
  const clipTopByRow = new Map<HTMLElement, number>();
  let previousBottom = Number.NEGATIVE_INFINITY;
  pinnedLayouts.forEach((layout) => {
    const clippedTop = Math.max(
      0,
      Math.min(layout.height, previousBottom - layout.actualTop)
    );
    clipTopByRow.set(layout.row, clippedTop);
    previousBottom = Math.max(previousBottom, layout.actualBottom);
  });
  return clipTopByRow;
}

function clearTreeStickyOcclusion(sidebar: HTMLElement): void {
  sidebar
    .querySelectorAll<HTMLElement>(".tree-item[data-tree-sticky-occluded='true']")
    .forEach(clearTreeRowOcclusion);
  sidebar
    .querySelectorAll<HTMLElement>(".tree-item[data-tree-sticky-pinned='true']")
    .forEach(clearTreeRowPinned);
}

function clearTreeRowOcclusion(row: HTMLElement): void {
  row.style.clipPath = "";
  row.style.pointerEvents = "";
  row.removeAttribute("data-tree-sticky-occluded");
}

function applyTreeRowClip(row: HTMLElement, clippedTop: number): void {
  if (clippedTop <= 0) {
    clearTreeRowOcclusion(row);
    return;
  }
  const rowHeight = row.getBoundingClientRect().height || TREE_STICKY_ROW_HEIGHT;
  const normalizedClipTop = Math.max(0, Math.min(rowHeight, clippedTop));
  row.style.clipPath = `inset(${Math.ceil(normalizedClipTop)}px 0 0 0)`;
  row.style.pointerEvents = normalizedClipTop >= rowHeight - 0.5 ? "none" : "";
  row.dataset.treeStickyOccluded = "true";
}

function clearTreeRowState(row: HTMLElement): void {
  clearTreeRowOcclusion(row);
  clearTreeRowPinned(row);
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
