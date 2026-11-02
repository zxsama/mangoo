import {
  ChevronDown,
  ChevronRight,
  FolderX
} from "../../design/icons";
import { DEFAULT_FOLDER_COLOR, FolderGlyph } from "./FolderGlyph";
import type { CSSProperties, RefObject } from "react";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { folderShowsOpenIcon, isFolderKnownChildless } from "../../core/folderTreeState";
import { resolveFolderLocationScrollFrame } from "./folderLocationScroll";
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
  const [highlightedFolderId, setHighlightedFolderId] = useState<number | null>(null);
  const locateAnimationActiveRef = useRef(false);
  const locateAnimationCancelledRef = useRef(false);
  const revealFolderPath = library.revealFolderPath;
  useTreeStickyOcclusion(sidebarRef);

  const stopLocateHighlight = useCallback(() => {
    locateAnimationCancelledRef.current = true;
    setHighlightedFolderId(null);
  }, []);

  useEffect(() => {
    const stop = () => {
      if (!locateAnimationActiveRef.current) {
        stopLocateHighlight();
      } else {
        locateAnimationCancelledRef.current = true;
      }
    };
    window.addEventListener("click", stop, true);
    window.addEventListener("contextmenu", stop, true);
    window.addEventListener("focusin", stop, true);
    window.addEventListener("keydown", stop, true);
    window.addEventListener("pointerdown", stop, true);
    window.addEventListener("wheel", stop, { capture: true, passive: true });
    return () => {
      window.removeEventListener("click", stop, true);
      window.removeEventListener("contextmenu", stop, true);
      window.removeEventListener("focusin", stop, true);
      window.removeEventListener("keydown", stop, true);
      window.removeEventListener("pointerdown", stop, true);
      window.removeEventListener("wheel", stop, true);
    };
  }, [stopLocateHighlight]);

  useEffect(() => {
    if (!locateRequest) return;
    setLocateTarget(null);
    stopLocateHighlight();
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
  }, [locateRequest?.folderId, locateRequest?.requestId, revealFolderPath, stopLocateHighlight]);

  useLayoutEffect(() => {
    if (!locateTarget) return;
    const sidebar = sidebarRef.current;
    if (!sidebar) return;

    let frame = 0;
    let attempts = 0;
    let scrollFrame = 0;
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
      const targetTop = Math.max(0, Math.min(maxScrollTop, centeredTop));
      const startTop = sidebar.scrollTop;
      const distance = Math.abs(targetTop - startTop);
      const durationMs = distance === 0 ? 0 : Math.min(420, Math.max(180, distance * 0.45));
      const startedAt = performance.now();
      locateAnimationActiveRef.current = true;
      locateAnimationCancelledRef.current = false;
      const finish = () => {
        sidebar.scrollTop = targetTop;
        locateAnimationActiveRef.current = false;
        if (!locateAnimationCancelledRef.current) {
          setHighlightedFolderId(locateTarget.folderId);
        }
      };
      const animate = (now: number) => {
        const next = resolveFolderLocationScrollFrame({
          from: startTop,
          to: targetTop,
          elapsedMs: now - startedAt,
          durationMs
        });
        sidebar.scrollTop = next.top;
        if (next.done) {
          finish();
          return;
        }
        scrollFrame = window.requestAnimationFrame(animate);
      };
      if (durationMs === 0) {
        finish();
      } else {
        scrollFrame = window.requestAnimationFrame(animate);
      }
    };

    frame = window.requestAnimationFrame(locate);
    return () => {
      if (frame) window.cancelAnimationFrame(frame);
      if (scrollFrame) window.cancelAnimationFrame(scrollFrame);
      locateAnimationActiveRef.current = false;
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
            highlightedFolderId={highlightedFolderId}
          />
        ))}
        {library.roots.length === 0 && !library.switching ? (
          <div className="empty-panel">No roots</div>
        ) : null}
      </div>
    </aside>
  );
}

type TreeGuide = { color: string; continues: boolean };

/** Row-local segments share the sticky row's clipping and never intercept input. */
function TreeGuides({ guides }: { guides: TreeGuide[] }) {
  return <span className="tree-guides" aria-hidden="true">
    {guides.map((guide, index) => {
      const directParent = index === guides.length - 1;
      if (!directParent && !guide.continues) return null;
      const terminal = directParent && !guide.continues;
      return <span
        key={index}
        className="tree-guide"
        data-tree-guide-connector={terminal ? "true" : undefined}
        data-tree-guide-last={terminal ? "true" : undefined}
        style={{ "--tree-guide-depth": String(index), color: guide.color } as CSSProperties}
      />;
    })}
  </span>;
}

function folderIsKnownChildless(library: LibraryState, folderId: number, hasChildren?: boolean) {
  return isFolderKnownChildless({
    hasChildren,
    // Ancestor hydration can populate the cache without completing a page.
    childrenLoaded: library.folderChildNextCursorByParent[folderId] === null,
    loadedChildCount: library.folderChildrenByParent[folderId]?.length ?? 0,
    hasMore: Boolean(library.folderChildNextCursorByParent[folderId]),
    loading: library.loadingFolderIds.has(folderId) || library.loadingMoreFolderIds.has(folderId),
    loadFailed: library.failedFolderIds.has(folderId)
  });
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
  onRootContextMenu?: LibrarySidebarProps["onRootContextMenu"];
  onFolderContextMenu?: LibrarySidebarProps["onFolderContextMenu"];
}) {
  const rootFolderId = root.rootFolderId;
  const knownChildless = rootFolderId !== null && folderIsKnownChildless(library, rootFolderId);
  const expandable = rootFolderId !== null && !knownChildless;
  const expanded = expandable && library.expandedFolderIds.has(rootFolderId);
  const children = rootFolderId !== null ? library.folderChildrenByParent[rootFolderId] ?? [] : [];
  const selected = root.id === library.selectedRootId && library.selectedFolderId === rootFolderId;
  const color = root.color ?? DEFAULT_FOLDER_COLOR;
  const hasMore = rootFolderId !== null && Boolean(library.folderChildNextCursorByParent[rootFolderId]);

  return (
    <div className="tree-branch">
      <div
        className={selected ? "tree-item selected" : "tree-item"}
        data-interactive-pointer-target-selector=".tree-label"
        data-depth="0"
        data-folder-id={rootFolderId ?? undefined}
        data-root-id={root.id}
        data-tree-locate-highlight={
          rootFolderId !== null && rootFolderId === highlightedFolderId ? "true" : undefined
        }
        role="treeitem"
        aria-expanded={expandable ? expanded : undefined}
        aria-selected={selected}
        style={treeItemStyle(0, color)}
      >
        {expandable ? <button
          className="tree-disclosure"
          data-pointer-glow="off"
          onClick={() => library.toggleFolderExpanded(rootFolderId)}
          type="button"
          aria-label={expanded ? "Collapse root" : "Expand root"}
        >
          {expanded ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
        </button> : <span className="tree-disclosure tree-disclosure--leaf" aria-hidden="true" />}
        <button
          className="tree-label"
          onClick={() => library.setSelectedRootId(root.id)}
          onContextMenu={(event) => {
            if (!onRootContextMenu) return;
            event.preventDefault();
            onRootContextMenu({ root, x: event.clientX, y: event.clientY, shiftKey: event.shiftKey });
          }}
          type="button"
        >
          <FolderGlyph icon={root.icon} color={root.color} open={folderShowsOpenIcon({ expanded, knownChildless, selected })} />
          <span className="tree-name">{root.displayName}</span>
          {root.reachable === false ? <FolderX size={14} className="tree-root-offline" aria-label="资源库离线" /> : null}
          <span
            aria-label={`${root.fileCount ?? root.mediaCount} files`}
            className="tree-media-count"
            title={`${(root.fileCount ?? root.mediaCount).toLocaleString()} files including subfolders`}
          >
            {(root.fileCount ?? root.mediaCount).toLocaleString()}
          </span>
        </button>
      </div>
      {expanded ? (
        <div role="group">
          {children.map((folder, index) => (
            <FolderNode
              depth={1}
              guides={[{ color, continues: index < children.length - 1 || hasMore }]}
              folder={folder}
              key={folder.id}
              library={library}
              highlightedFolderId={highlightedFolderId}
              onFolderContextMenu={onFolderContextMenu}
            />
          ))}
          <LoadMoreChildren folderId={rootFolderId} library={library} guides={[{ color, continues: false }]} />
        </div>
      ) : null}
    </div>
  );
}

function FolderNode({
  depth,
  guides,
  folder,
  highlightedFolderId,
  library,
  onFolderContextMenu
}: {
  depth: number;
  guides: TreeGuide[];
  folder: FolderRecord;
  highlightedFolderId: number | null;
  library: LibraryState;
  onFolderContextMenu?: LibrarySidebarProps["onFolderContextMenu"];
}) {
  const children = library.folderChildrenByParent[folder.id] ?? [];
  const selected = library.selectedFolderId === folder.id;
  const knownChildless = folderIsKnownChildless(library, folder.id, folder.hasChildren);
  const expanded = !knownChildless && library.expandedFolderIds.has(folder.id);
  const showsOpenIcon = folderShowsOpenIcon({ expanded, knownChildless, selected });
  const color = folder.color ?? DEFAULT_FOLDER_COLOR;
  const hasMore = Boolean(library.folderChildNextCursorByParent[folder.id]);

  return (
    <div className="tree-branch">
      <div
        className={selected ? "tree-item selected" : "tree-item"}
        data-interactive-pointer-target-selector=".tree-label"
        data-depth={depth}
        data-folder-id={folder.id}
        data-root-id={folder.rootId}
        data-tree-locate-highlight={folder.id === highlightedFolderId ? "true" : undefined}
        role="treeitem"
        aria-expanded={knownChildless ? undefined : expanded}
        aria-selected={selected}
        style={treeItemStyle(depth, color)}
      >
        <TreeGuides guides={guides} />
        {knownChildless ? (
          <span className="tree-disclosure tree-disclosure--leaf" aria-hidden="true" />
        ) : (
          <button
            className="tree-disclosure"
            data-pointer-glow="off"
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
          onContextMenu={(event) => {
            if (!onFolderContextMenu) return;
            event.preventDefault();
            onFolderContextMenu({ folder, x: event.clientX, y: event.clientY, shiftKey: event.shiftKey });
          }}
          type="button"
        >
          <FolderGlyph icon={folder.icon} color={folder.color} open={showsOpenIcon} size={16} />
          <span className="tree-name">{folder.name}</span>
          <span
            aria-label={`${folder.fileCount ?? folder.mediaCount} files`}
            className="tree-media-count"
            title={`${(folder.fileCount ?? folder.mediaCount).toLocaleString()} files including subfolders`}
          >
            {(folder.fileCount ?? folder.mediaCount).toLocaleString()}
          </span>
        </button>
      </div>
      {expanded ? (
        <div role="group">
          {children.map((child, index) => (
            <FolderNode
              depth={depth + 1}
              guides={[...guides, { color, continues: index < children.length - 1 || hasMore }]}
              folder={child}
              key={child.id}
              library={library}
              highlightedFolderId={highlightedFolderId}
              onFolderContextMenu={onFolderContextMenu}
            />
          ))}
          <LoadMoreChildren depth={depth + 1} folderId={folder.id} library={library} guides={[...guides, { color, continues: false }]} />
        </div>
      ) : null}
    </div>
  );
}

function treeItemStyle(depth: number, color = DEFAULT_FOLDER_COLOR): CSSProperties {
  return {
    "--tree-depth": String(depth),
    "--tree-identity-color": color,
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

  const rows = Array.from(tree.querySelectorAll<HTMLElement>(".tree-item[role='treeitem'], .tree-page-row"));
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
    .querySelectorAll<HTMLElement>("[data-tree-sticky-occluded='true']")
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
  guides,
  library
}: {
  depth?: number;
  folderId: number | null;
  guides: TreeGuide[];
  library: LibraryState;
}) {
  if (!folderId || !library.folderChildNextCursorByParent[folderId]) {
    return null;
  }

  const loading = library.loadingMoreFolderIds.has(folderId);

  return (
    <div className="tree-page-row" style={treeItemStyle(depth)}>
      <TreeGuides guides={guides} />
      <button
      className="tree-load-more"
      disabled={loading}
      onClick={() => void library.loadMoreFolderChildren(folderId)}
      type="button"
    >
      Load more children
      </button>
    </div>
  );
}
