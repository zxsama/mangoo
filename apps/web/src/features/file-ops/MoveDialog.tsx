import { ChevronDown, ChevronRight, Folder } from "lucide-react";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type RefObject,
} from "react";
import type { FolderRecord } from "@megle/core-client";
import type { LibraryState } from "../../core/useLibraryData";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  ModalSurface,
  OverlayDialogActions,
  OverlayDialogBody,
  OverlayDialogHeader,
  OverlayInlineStatus,
} from "../../design/overlays";

export interface MoveDialogProps {
  open: boolean;
  library: LibraryState;
  /** File ids being moved */
  fileIds: number[];
  /** Folder ids being moved */
  folderIds: number[];
  busy?: boolean;
  serverError?: string | null;
  serverErrorCode?: string | null;
  onCancel: () => void;
  onSubmit: (targetFolderId: number) => void;
  restoreFocusRef?: RefObject<HTMLElement | null>;
}

interface MoveBranchSharedProps {
  excludedFolderIds: Set<number>;
  focusedItemKey: string;
  library: LibraryState;
  loadingFolderIds: Set<number>;
  onFocusItem: (itemKey: string) => void;
  onLoadMoreFocusOwnershipChange: (sourceItemKey: string, ownsFocus: boolean) => void;
  onLoadMore: (folderId: number, sourceItemKey: string) => void;
  onSelect: (folderId: number) => void;
  selectedTarget: number | null;
}

interface PageFocusHandoff {
  owner: symbol;
  parentFolderId: number;
  previousChildIds: Set<number>;
  sourceItemKey: string;
  sourceOwnsDomFocus: boolean;
}

interface MoveTreeRowProps {
  depth: number;
  disabled: boolean;
  expanded: boolean;
  expandable: boolean;
  folderId: number;
  focusedItemKey: string;
  label: string;
  onExpandedChange: (expanded: boolean) => void;
  onFocusItem: (itemKey: string) => void;
  onSelect: (folderId: number) => void;
  selected: boolean;
}

function folderItemKey(folderId: number): string {
  return `folder:${folderId}`;
}

function loadMoreItemKey(folderId: number): string {
  return `load-more:${folderId}`;
}

function visibleTreeItems(currentTarget: HTMLElement): HTMLButtonElement[] {
  const tree = currentTarget.closest('[role="tree"]');
  return tree
    ? Array.from(tree.querySelectorAll<HTMLButtonElement>('button[role="treeitem"]'))
    : [];
}

function focusTreeItem(items: HTMLButtonElement[], index: number): void {
  items[Math.max(0, Math.min(index, items.length - 1))]?.focus();
}

function handleMoveTreeKey(
  event: KeyboardEvent<HTMLButtonElement>,
  options?: {
    expanded?: boolean;
    expandable?: boolean;
    onActivate?: () => void;
    onExpandedChange?: (expanded: boolean) => void;
  }
): void {
  const items = visibleTreeItems(event.currentTarget);
  const currentIndex = items.indexOf(event.currentTarget);
  if (currentIndex < 0) return;

  const currentLevel = Number(event.currentTarget.getAttribute("aria-level")) || 1;
  switch (event.key) {
    case "ArrowDown":
      event.preventDefault();
      focusTreeItem(items, currentIndex + 1);
      break;
    case "ArrowUp":
      event.preventDefault();
      focusTreeItem(items, currentIndex - 1);
      break;
    case "Home":
      event.preventDefault();
      focusTreeItem(items, 0);
      break;
    case "End":
      event.preventDefault();
      focusTreeItem(items, items.length - 1);
      break;
    case "Enter":
    case " ":
      if (!options?.onActivate) return;
      event.preventDefault();
      options.onActivate();
      break;
    case "ArrowRight": {
      if (!options?.expandable || !options.onExpandedChange) return;
      event.preventDefault();
      if (!options.expanded) {
        options.onExpandedChange(true);
        return;
      }
      const next = items[currentIndex + 1];
      const nextLevel = Number(next?.getAttribute("aria-level")) || 0;
      if (nextLevel > currentLevel) next.focus();
      break;
    }
    case "ArrowLeft": {
      event.preventDefault();
      if (options?.expandable && options.expanded && options.onExpandedChange) {
        options.onExpandedChange(false);
        return;
      }
      for (let index = currentIndex - 1; index >= 0; index -= 1) {
        const level = Number(items[index]?.getAttribute("aria-level")) || 0;
        if (level < currentLevel) {
          items[index]?.focus();
          break;
        }
      }
      break;
    }
  }
}

export function MoveDialog({
  open,
  library,
  fileIds,
  folderIds,
  busy = false,
  serverError = null,
  serverErrorCode = null,
  onCancel,
  onSubmit,
  restoreFocusRef,
}: MoveDialogProps) {
  const firstRootFolderId = library.roots.find((root) => root.rootFolderId !== null)?.rootFolderId;
  const [target, setTarget] = useState<number | null>(null);
  const [focusedItemKey, setFocusedItemKey] = useState("");
  const [loadingFolderIds, setLoadingFolderIds] = useState<Set<number>>(() => new Set());
  const [pageError, setPageError] = useState<string | null>(null);
  const pageLoadOwnersRef = useRef(new Map<number, symbol>());
  const pageFocusHandoffRef = useRef<PageFocusHandoff | null>(null);
  const treeRef = useRef<HTMLDivElement | null>(null);
  const librarySnapshotRef = useRef(library);
  librarySnapshotRef.current = library;

  useEffect(() => {
    if (!open) {
      pageLoadOwnersRef.current.clear();
      pageFocusHandoffRef.current = null;
      setLoadingFolderIds(new Set());
      return;
    }
    setTarget(null);
    setFocusedItemKey(firstRootFolderId === null || firstRootFolderId === undefined
      ? ""
      : folderItemKey(firstRootFolderId));
    setPageError(null);
  }, [firstRootFolderId, open]);

  useEffect(() => () => {
    pageLoadOwnersRef.current.clear();
    pageFocusHandoffRef.current = null;
  }, []);

  // Compute the set of folder ids that should be excluded as drop targets
  // (folders being moved themselves and their currently-loaded descendants).
  const excludedFolderIds = useMemo(() => {
    const blocked = new Set<number>(folderIds);
    const queue = [...folderIds];
    while (queue.length > 0) {
      const id = queue.shift();
      if (id === undefined) break;
      const children = library.folderChildrenByParent[id] ?? [];
      for (const child of children) {
        if (!blocked.has(child.id)) {
          blocked.add(child.id);
          queue.push(child.id);
        }
      }
    }
    return blocked;
  }, [folderIds, library.folderChildrenByParent]);

  async function handleLoadMore(folderId: number, sourceItemKey: string): Promise<void> {
    if (
      busy ||
      pageLoadOwnersRef.current.has(folderId) ||
      loadingFolderIds.has(folderId) ||
      library.loadingMoreFolderIds?.has(folderId)
    ) {
      return;
    }

    const owner = Symbol(`move-folder-page:${folderId}`);
    const requestedCursor = library.folderChildNextCursorByParent?.[folderId] ?? null;
    pageLoadOwnersRef.current.set(folderId, owner);
    const activeTreeItem = document.activeElement instanceof HTMLElement
      ? document.activeElement.dataset.moveTreeItemKey
      : undefined;
    pageFocusHandoffRef.current = {
      owner,
      parentFolderId: folderId,
      previousChildIds: new Set(
        (library.folderChildrenByParent[folderId] ?? []).map((child) => child.id)
      ),
      sourceItemKey,
      sourceOwnsDomFocus: activeTreeItem === sourceItemKey,
    };
    setLoadingFolderIds((current) => new Set(current).add(folderId));
    setPageError(null);
    try {
      await library.loadMoreFolderChildren(folderId);
      // The library reports page failures globally and resolves. Give its state
      // update one frame to commit, then treat an unchanged cursor as retryable.
      await new Promise<void>((resolve) => window.requestAnimationFrame(() => resolve()));
      if (
        pageLoadOwnersRef.current.get(folderId) === owner &&
        librarySnapshotRef.current.folderChildNextCursorByParent?.[folderId] === requestedCursor
      ) {
        setPageError("Could not load more folders. Try again.");
      } else if (
        pageFocusHandoffRef.current?.owner === owner &&
        librarySnapshotRef.current.folderChildNextCursorByParent?.[folderId] !== null
      ) {
        pageFocusHandoffRef.current = null;
      }
    } catch {
      if (pageLoadOwnersRef.current.get(folderId) === owner) {
        if (pageFocusHandoffRef.current?.owner === owner) {
          pageFocusHandoffRef.current = null;
        }
        setPageError("Could not load more folders. Try again.");
      }
    } finally {
      if (pageLoadOwnersRef.current.get(folderId) === owner) {
        pageLoadOwnersRef.current.delete(folderId);
        setLoadingFolderIds((current) => {
          const next = new Set(current);
          next.delete(folderId);
          return next;
        });
      }
    }
  }

  const itemCount = fileIds.length + folderIds.length;
  const isCrossRoot = serverErrorCode === "cross_root";
  const canSubmit = !busy && target !== null;
  const focusHandoff = pageFocusHandoffRef.current;
  let renderedFocusedItemKey = focusedItemKey;
  if (
    focusHandoff &&
    library.folderChildNextCursorByParent?.[focusHandoff.parentFolderId] === null
  ) {
    const firstNewChild = (
      library.folderChildrenByParent[focusHandoff.parentFolderId] ?? []
    ).find((child) => !focusHandoff.previousChildIds.has(child.id));
    renderedFocusedItemKey = firstNewChild
      ? folderItemKey(firstNewChild.id)
      : folderItemKey(focusHandoff.parentFolderId);
  }

  useLayoutEffect(() => {
    if (renderedFocusedItemKey === focusedItemKey || !focusHandoff) return;
    if (pageFocusHandoffRef.current !== focusHandoff) return;

    const target = Array.from(
      treeRef.current?.querySelectorAll<HTMLButtonElement>('[role="treeitem"]') ?? []
    ).find((item) => item.dataset.moveTreeItemKey === renderedFocusedItemKey);
    const restoreDomFocus = focusHandoff.sourceOwnsDomFocus;
    pageFocusHandoffRef.current = null;
    setFocusedItemKey(renderedFocusedItemKey);
    setPageError(null);
    if (restoreDomFocus) target?.focus();
  }, [focusHandoff, focusedItemKey, renderedFocusedItemKey]);

  const branchProps: Omit<MoveBranchSharedProps, "library"> = {
    excludedFolderIds,
    focusedItemKey: renderedFocusedItemKey,
    loadingFolderIds,
    onFocusItem: setFocusedItemKey,
    onLoadMoreFocusOwnershipChange: (sourceItemKey, ownsFocus) => {
      const current = pageFocusHandoffRef.current;
      if (current?.sourceItemKey === sourceItemKey) {
        current.sourceOwnsDomFocus = ownsFocus;
      }
    },
    onLoadMore: (folderId, sourceItemKey) => {
      void handleLoadMore(folderId, sourceItemKey);
    },
    onSelect: setTarget,
    selectedTarget: target,
  };

  return (
    <ModalSurface
      ariaLabelledBy="move-dialog-title"
      busy={busy}
      className="file-operation-dialog overlay-dialog-wide"
      id="move-dialog"
      onDismiss={onCancel}
      open={open}
      restoreFocusRef={restoreFocusRef}
      rootId="file-operation"
    >
      <OverlayDialogHeader
        description="Pick a destination folder"
        title={`Move ${itemCount} item${itemCount === 1 ? "" : "s"}`}
        titleId="move-dialog-title"
      />
      <OverlayDialogBody>
        <div
          aria-label="Destination folder"
          className="move-tree"
          ref={treeRef}
          role="tree"
        >
          {library.roots.map((root) => (
            <RootBranch
              {...branchProps}
              key={root.id}
              library={library}
              root={root}
            />
          ))}
          {library.roots.length === 0 ? (
            <div className="empty-panel">No roots available</div>
          ) : null}
        </div>

        {pageError ? (
          <OverlayInlineStatus tone="error">{pageError}</OverlayInlineStatus>
        ) : null}

        {isCrossRoot ? (
          <OverlayInlineStatus tone="error">
            {serverError ?? "Cross-root moves are not supported yet. Pick a destination inside the same root."}
          </OverlayInlineStatus>
        ) : serverError ? (
          <OverlayInlineStatus tone="error">{serverError}</OverlayInlineStatus>
        ) : null}

        <OverlayDialogActions>
          <button
            className="overlay-dialog-button"
            disabled={busy}
            onClick={onCancel}
            type="button"
          >
            Cancel
          </button>
          <LiquidGlassButton
            className="overlay-dialog-button overlay-dialog-button-primary"
            disabled={!canSubmit}
            onClick={() => {
              if (target !== null) onSubmit(target);
            }}
            tone="primary"
            type="button"
          >
            {busy ? "Moving…" : "Move here"}
          </LiquidGlassButton>
        </OverlayDialogActions>
      </OverlayDialogBody>
    </ModalSurface>
  );
}

function RootBranch({
  library,
  root,
  ...shared
}: MoveBranchSharedProps & {
  root: { id: number; rootFolderId: number | null; displayName: string };
}) {
  const folderId = root.rootFolderId;
  const [expanded, setExpanded] = useState(true);

  useEffect(() => {
    if (!folderId) return;
    if (!library.folderChildrenByParent[folderId]) {
      // Trigger lazy load through the library owner so its cursor state stays authoritative.
      if (!library.expandedFolderIds.has(folderId)) {
        library.toggleFolderExpanded(folderId);
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [folderId]);

  if (!folderId) return null;

  const children = library.folderChildrenByParent[folderId] ?? [];
  const cursor = library.folderChildNextCursorByParent?.[folderId] ?? null;

  return (
    <div className="move-branch">
      <MoveTreeRow
        depth={0}
        disabled={shared.excludedFolderIds.has(folderId)}
        expanded={expanded}
        expandable
        folderId={folderId}
        focusedItemKey={shared.focusedItemKey}
        label={root.displayName}
        onExpandedChange={setExpanded}
        onFocusItem={shared.onFocusItem}
        onSelect={shared.onSelect}
        selected={shared.selectedTarget === folderId}
      />
      {expanded ? (
        <div role="group">
          {children.map((child) => (
            <FolderBranch
              {...shared}
              depth={1}
              folder={child}
              key={child.id}
              library={library}
            />
          ))}
          {cursor ? (
            <MoveLoadMoreItem
              depth={1}
              focusedItemKey={shared.focusedItemKey}
              folderId={folderId}
              loading={shared.loadingFolderIds.has(folderId)}
              onFocusItem={shared.onFocusItem}
              onFocusOwnershipChange={shared.onLoadMoreFocusOwnershipChange}
              onLoadMore={shared.onLoadMore}
            />
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function FolderBranch({
  depth,
  folder,
  library,
  ...shared
}: MoveBranchSharedProps & {
  depth: number;
  folder: FolderRecord;
}) {
  const [expanded, setExpanded] = useState(false);
  const children = library.folderChildrenByParent[folder.id] ?? [];
  const cursor = library.folderChildNextCursorByParent?.[folder.id] ?? null;
  const expandable = folder.hasChildren || children.length > 0 || cursor !== null;

  function handleExpandedChange(next: boolean) {
    if (next && !library.folderChildrenByParent[folder.id]) {
      library.toggleFolderExpanded(folder.id);
    }
    setExpanded(next);
  }

  return (
    <div className="move-branch">
      <MoveTreeRow
        depth={depth}
        disabled={shared.excludedFolderIds.has(folder.id)}
        expanded={expanded}
        expandable={expandable}
        folderId={folder.id}
        focusedItemKey={shared.focusedItemKey}
        label={folder.name}
        onExpandedChange={handleExpandedChange}
        onFocusItem={shared.onFocusItem}
        onSelect={shared.onSelect}
        selected={shared.selectedTarget === folder.id}
      />
      {expanded ? (
        <div role="group">
          {children.map((child) => (
            <FolderBranch
              {...shared}
              depth={depth + 1}
              folder={child}
              key={child.id}
              library={library}
            />
          ))}
          {cursor ? (
            <MoveLoadMoreItem
              depth={depth + 1}
              focusedItemKey={shared.focusedItemKey}
              folderId={folder.id}
              loading={shared.loadingFolderIds.has(folder.id)}
              onFocusItem={shared.onFocusItem}
              onFocusOwnershipChange={shared.onLoadMoreFocusOwnershipChange}
              onLoadMore={shared.onLoadMore}
            />
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function MoveTreeRow({
  depth,
  disabled,
  expanded,
  expandable,
  folderId,
  focusedItemKey,
  label,
  onExpandedChange,
  onFocusItem,
  onSelect,
  selected,
}: MoveTreeRowProps) {
  const itemKey = folderItemKey(folderId);

  return (
    <button
      aria-disabled={disabled || undefined}
      aria-expanded={expandable ? expanded : undefined}
      aria-level={depth + 1}
      aria-selected={selected}
      className={`move-row${selected ? " move-row-selected" : ""}${
        disabled ? " move-row-disabled" : ""
      }`}
      data-move-tree-item-key={itemKey}
      onClick={() => {
        if (!disabled) onSelect(folderId);
      }}
      onFocus={() => onFocusItem(itemKey)}
      onKeyDown={(event) => handleMoveTreeKey(event, {
        expanded,
        expandable,
        onActivate: disabled ? undefined : () => onSelect(folderId),
        onExpandedChange,
      })}
      role="treeitem"
      style={{ "--move-depth": String(depth) } as CSSProperties}
      tabIndex={focusedItemKey === itemKey ? 0 : -1}
      type="button"
    >
      <span
        aria-hidden="true"
        className="move-disclosure"
        onClick={(event) => {
          event.stopPropagation();
          if (expandable) onExpandedChange(!expanded);
        }}
      >
        {expandable ? (
          expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />
        ) : null}
      </span>
      <span className="move-label">
        <Folder size={14} />
        <span>{label}</span>
      </span>
    </button>
  );
}

function MoveLoadMoreItem({
  depth,
  focusedItemKey,
  folderId,
  loading,
  onFocusItem,
  onFocusOwnershipChange,
  onLoadMore,
}: {
  depth: number;
  focusedItemKey: string;
  folderId: number;
  loading: boolean;
  onFocusItem: (itemKey: string) => void;
  onFocusOwnershipChange: (sourceItemKey: string, ownsFocus: boolean) => void;
  onLoadMore: (folderId: number, sourceItemKey: string) => void;
}) {
  const itemKey = loadMoreItemKey(folderId);

  return (
    <button
      aria-disabled={loading || undefined}
      aria-level={depth + 1}
      aria-selected={false}
      className="move-row move-load-more"
      data-move-tree-item-key={itemKey}
      onBlur={() => onFocusOwnershipChange(itemKey, false)}
      onClick={() => {
        if (!loading) onLoadMore(folderId, itemKey);
      }}
      onFocus={() => {
        onFocusItem(itemKey);
        onFocusOwnershipChange(itemKey, true);
      }}
      onKeyDown={(event) => handleMoveTreeKey(event, {
        onActivate: loading ? undefined : () => onLoadMore(folderId, itemKey),
      })}
      role="treeitem"
      style={{ "--move-depth": String(depth) } as CSSProperties}
      tabIndex={focusedItemKey === itemKey ? 0 : -1}
      type="button"
    >
      <span aria-hidden="true" className="move-disclosure" />
      <span className="move-label">
        <span>{loading ? "Loading more…" : "Load more children"}</span>
      </span>
    </button>
  );
}
