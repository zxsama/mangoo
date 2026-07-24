import { useEffect, useMemo, useRef, useState } from "react";
import type { FolderRecord, MediaRecord } from "@megle/core-client";
import type { LibraryState } from "../../core/useLibraryData";
import { MediaGrid, MediaGridRefreshIndicator } from "../media-grid/MediaGrid";
import type { LibraryGridPreferences } from "../media-grid/gridPreferences";
import type { LibraryLayoutMode } from "../media-grid/layoutMode";
import { CentralPreviewStage } from "../preview/CentralPreviewStage";
import { InspectorMetadata } from "../preview/InspectorMetadata";
import { PreviewPanel } from "../preview/PreviewPanel";
import { SubfolderCard, SubfolderStrip } from "./SubfolderStrip";
import { buildVisibleSubfolderEntries } from "./subfolderHierarchy";
import { useFolderCovers } from "./useFolderCovers";
import type { MediaGridLocateRequest } from "../../app/mediaFocusWorkflow";

const AHEAD_THUMBNAIL_ROW_COUNT = 4;
const SUBFOLDER_CHILD_PROBE_LIMIT = 12;
const SUBFOLDER_STRIP_COLLAPSED_STORAGE_KEY = "megle.library.subfolder-strip-collapsed";

interface LibraryViewProps {
  gridPreferences: LibraryGridPreferences;
  library: LibraryState;
  layoutMode: LibraryLayoutMode;
  previewOpen: boolean;
  sidebarLayoutTransitionKey: string;
  mediaLocateRequest?: MediaGridLocateRequest | null;
  onMediaLocateSettled?: (requestId: string, ok: boolean) => void;
  onLocateFolder: (folderId: number) => void;
  onOpenPreview: (mediaId: number) => void;
  onClosePreview: () => void;
  onPreviewPrevious: () => void;
  onPreviewNext: () => void;
  onPreviewMediaSettled: (mediaId: number) => void;
  onPreviewViewStateChange: (state: { mode: "fit-long-edge" | "actual"; scale: number }) => void;
  previewViewState: { mode: "fit-long-edge" | "actual"; scale: number };
  onPreviewCommandChange: (commands: {
    reset: () => void;
    toggleActualSize: () => void;
  } | null) => void;
  onMediaContextMenu?: (event: {
    item: MediaRecord;
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

export function LibraryView({
  gridPreferences,
  library,
  layoutMode,
  onClosePreview,
  onFolderContextMenu,
  onLocateFolder,
  mediaLocateRequest,
  onMediaLocateSettled,
  onMediaContextMenu,
  onOpenPreview,
  onPreviewCommandChange,
  onPreviewMediaSettled,
  onPreviewNext,
  onPreviewPrevious,
  onPreviewViewStateChange,
  previewViewState,
  previewOpen,
  sidebarLayoutTransitionKey
}: LibraryViewProps) {
  return (
    <>
      <LibraryCenterPane
        gridPreferences={gridPreferences}
        library={library}
        layoutMode={layoutMode}
        onClosePreview={onClosePreview}
        onFolderContextMenu={onFolderContextMenu}
        onLocateFolder={onLocateFolder}
        mediaLocateRequest={mediaLocateRequest}
        onMediaLocateSettled={onMediaLocateSettled}
        onMediaContextMenu={onMediaContextMenu}
        onOpenPreview={onOpenPreview}
        onPreviewCommandChange={onPreviewCommandChange}
        onPreviewMediaSettled={onPreviewMediaSettled}
        onPreviewNext={onPreviewNext}
        onPreviewPrevious={onPreviewPrevious}
        onPreviewViewStateChange={onPreviewViewStateChange}
        previewViewState={previewViewState}
        previewOpen={previewOpen}
        sidebarLayoutTransitionKey={sidebarLayoutTransitionKey}
      />
      <LibraryInspectorPane library={library} previewOpen={previewOpen} />
    </>
  );
}

export function LibraryCenterPane({
  gridPreferences,
  library,
  layoutMode,
  onClosePreview,
  onFolderContextMenu,
  onLocateFolder,
  mediaLocateRequest,
  onMediaLocateSettled,
  onMediaContextMenu,
  onOpenPreview,
  onPreviewCommandChange,
  onPreviewMediaSettled,
  onPreviewNext,
  onPreviewPrevious,
  onPreviewViewStateChange,
  previewViewState,
  previewOpen,
  sidebarLayoutTransitionKey
}: LibraryViewProps) {
  const selectedRoot = library.roots.find((root) => root.id === library.selectedRootId) ?? null;
  const selectedMedia = library.selectedMedia;
  const previewMedia = previewOpen && selectedMedia ? selectedMedia : null;
  const mediaScrollKey = mediaScrollPositionKey(
    layoutMode,
    library.selectedRootId,
    library.selectedFolderId
  );
  const [subfolderStripCollapsed, setSubfolderStripCollapsed] = useState<boolean>(() =>
    readStoredSubfolderStripCollapsed()
  );
  const [folderCoverPriorityIndexes, setFolderCoverPriorityIndexes] = useState<number[]>([]);
  const [expandedSubfolderIds, setExpandedSubfolderIds] = useState<Set<number>>(() => new Set());
  // Dedup which subfolders we've already asked to load children / paginate for.
  // The probe + load-more effects below depend on the very state their loads
  // mutate (folderChildrenByParent / loadingFolderIds / cursors), so without a
  // request dedup every completed load re-triggers them and they re-issue —
  // an unbounded feedback cascade that hit React's update-depth limit and froze
  // the grid. These refs make effect re-runs idempotent: a folder/page is
  // requested at most once, so re-runs do no work. Cleared on folder/mode change.
  const probedChildFolderIdsRef = useRef<Set<number>>(new Set());
  const requestedLoadMorePagesRef = useRef<Set<string>>(new Set());
  const currentFolderId = library.selectedFolderId ?? selectedRoot?.rootFolderId ?? null;
  const visibleSubfolderEntries = useMemo(
    () =>
      buildVisibleSubfolderEntries({
        childFoldersByParentId: library.folderChildrenByParent,
        expandedFolderIds: expandedSubfolderIds,
        parentFolderId: currentFolderId,
        recursiveExpansionEnabled: library.showChildFolderContents
      }),
    [
      currentFolderId,
      expandedSubfolderIds,
      library.folderChildrenByParent,
      library.showChildFolderContents
    ]
  );
  const childFoldersLoading = currentFolderId
    ? library.loadingFolderIds.has(currentFolderId)
    : false;
  // "Loaded at least once" — folderChildrenByParent[id] is set (possibly to [])
  // after the first successful load. During a scan the background refresh
  // re-fetches children with `force`, which toggles loadingFolderIds on/off
  // every ~800ms; showing the strip merely because a (repeating) load is in
  // flight made an EMPTY folder's strip flicker in and out. So only let the
  // loading flag reveal the strip on the FIRST load — once we know the folder's
  // children, its emptiness is stable across background re-fetches.
  const childFoldersLoadedOnce =
    currentFolderId != null && library.folderChildrenByParent[currentFolderId] !== undefined;
  const showSubfolderStrip =
    visibleSubfolderEntries.length > 0 || (childFoldersLoading && !childFoldersLoadedOnce);
  const folderCoverPriorityFolders = useMemo(() => {
    const indexes = folderCoverPriorityIndexes;
    const seen = new Set<number>();
    const folders: FolderRecord[] = [];
    for (const index of indexes) {
      const folder = visibleSubfolderEntries[index]?.folder;
      if (!folder || seen.has(folder.id)) {
        continue;
      }
      seen.add(folder.id);
      folders.push(folder);
    }
    return folders;
  }, [folderCoverPriorityIndexes, visibleSubfolderEntries]);
  const coverMediaByFolderId = useFolderCovers(folderCoverPriorityFolders, {
    disabled: library.loading,
    activeScan: library.scanActive
  });
  const contentCount = Math.max(library.mediaTotalCount, library.media.length);
  const contentCountLabel = `${contentCount}`;
  const folderSection =
    showSubfolderStrip
      ? {
          collapsed: subfolderStripCollapsed,
          header: (
            <SubfolderStrip
              collapsed={subfolderStripCollapsed}
              folderCount={visibleSubfolderEntries.length}
              loading={childFoldersLoading}
              showChildContents={library.showChildFolderContents}
              onToggleCollapsed={() => setSubfolderStripCollapsed((current) => !current)}
              onToggleShowChildContents={library.toggleShowChildFolderContents}
            />
          ),
          itemCount: visibleSubfolderEntries.length,
          loading: childFoldersLoading,
          onVisibleFolderIndexesChange: (indexes: number[]) => {
            setFolderCoverPriorityIndexes((current) => {
              if (
                current.length === indexes.length &&
                current.every((value, index) => value === indexes[index])
              ) {
                return current;
              }
              return indexes;
            });
          },
          renderFolder: (index: number) => {
            const entry = visibleSubfolderEntries[index];
            const folder = entry?.folder;
            if (!folder) {
              return null;
            }
            const folderChildrenLoaded = library.folderChildrenByParent[folder.id] !== undefined;
            const folderChildren = library.folderChildrenByParent[folder.id] ?? [];
            const folderChildrenLoading = library.loadingFolderIds.has(folder.id);
            const hasLoadedChildren = folderChildren.length > 0;
            const childStatus = folderChildrenLoading
              ? "loading"
              : folderChildrenLoaded
                ? hasLoadedChildren
                  ? "has-children"
                  : "empty"
                : "unknown";
            const folderExpanded = expandedSubfolderIds.has(folder.id) && hasLoadedChildren;
            const coverLoaded = coverMediaByFolderId.has(folder.id);
            return (
              <SubfolderCard
                childStatus={childStatus}
                coverLoaded={coverLoaded}
                coverMedia={coverMediaByFolderId.get(folder.id) ?? []}
                depth={entry.depth}
                expandable={library.showChildFolderContents && hasLoadedChildren}
                expanded={folderExpanded}
                folder={folder}
                hasExpandedChildren={folderExpanded}
                inheritedGroupPosition={entry.inheritedGroupPosition}
                loadingChildren={folderChildrenLoading}
                nestedGroupPosition={entry.depth > 0 ? entry.siblingPosition : null}
                onFolderContextMenu={onFolderContextMenu}
                onOpenFolder={library.setSelectedFolder}
                onSelectFolder={library.setSelectedFolderInfo}
                onToggleExpanded={() => {
                  const expanding = !folderExpanded;
                  setExpandedSubfolderIds((current) => {
                    const next = new Set(current);
                    if (next.has(folder.id)) {
                      next.delete(folder.id);
                    } else {
                      next.add(folder.id);
                    }
                    return next;
                  });
                  if (expanding && !folderChildrenLoaded && !folderChildrenLoading) {
                    void library.requestFolderChildren(folder.id).catch(() => undefined);
                  }
                }}
                selected={folder.id === library.selectedFolderInfo?.id}
              />
            );
          }
        }
      : undefined;
  const contentHeader = contentCount > 0 ? (
    <div className="library-browser-content-header">
      <div className="library-browser-content-title">{`内容 (${contentCountLabel})`}</div>
    </div>
  ) : null;

  useEffect(() => {
    if (previewOpen && !selectedMedia) {
      onClosePreview();
    }
  }, [onClosePreview, previewOpen, selectedMedia]);

  useEffect(() => {
    try {
      window.localStorage.setItem(
        SUBFOLDER_STRIP_COLLAPSED_STORAGE_KEY,
        subfolderStripCollapsed ? "1" : "0"
      );
    } catch {
      // Ignore storage failures.
    }
  }, [subfolderStripCollapsed]);

  useEffect(() => {
    setFolderCoverPriorityIndexes([]);
    probedChildFolderIdsRef.current = new Set();
    requestedLoadMorePagesRef.current = new Set();
  }, [currentFolderId, library.showChildFolderContents]);

  useEffect(() => {
    if (library.showChildFolderContents) {
      return;
    }
    setExpandedSubfolderIds(new Set());
  }, [library.showChildFolderContents]);

  useEffect(() => {
    if (!library.showChildFolderContents || subfolderStripCollapsed) {
      return;
    }
    const probeIndexes =
      folderCoverPriorityIndexes.length > 0
        ? folderCoverPriorityIndexes
        : visibleSubfolderEntries
            .slice(0, SUBFOLDER_CHILD_PROBE_LIMIT)
            .map((_, index) => index);
    // SUBFOLDER_CHILD_PROBE_LIMIT is a *concurrency* cap, not a per-pass cap.
    // Count folder-child loads already in flight and only probe up to the
    // remaining budget. Without this, each pass probed a fresh batch of LIMIT
    // and the synchronous `setLoadingFolderIds` inside requestFolderChildren
    // re-ran this effect (loadingFolderIds is a dep), which probed the NEXT
    // batch, and so on — a synchronous nested-update cascade that walked the
    // whole priority list and tripped React's "maximum update depth" guard on
    // libraries with many subfolders. Gating on in-flight count bounds the
    // cascade: once LIMIT loads are outstanding a re-run probes nothing and
    // waits for async completions to free budget.
    const probeBudget = SUBFOLDER_CHILD_PROBE_LIMIT - library.loadingFolderIds.size;
    if (probeBudget <= 0) {
      return;
    }
    const folderIds = new Set<number>();
    for (const index of probeIndexes) {
      const folder = visibleSubfolderEntries[index]?.folder;
      if (!folder) {
        continue;
      }
      if (
        !probedChildFolderIdsRef.current.has(folder.id) &&
        library.folderChildrenByParent[folder.id] === undefined &&
        !library.loadingFolderIds.has(folder.id)
      ) {
        folderIds.add(folder.id);
      }
      if (folderIds.size >= probeBudget) {
        break;
      }
    }
    for (const folderId of folderIds) {
      probedChildFolderIdsRef.current.add(folderId);
      void library.requestFolderChildren(folderId).catch(() => {
        // Allow a genuine failure to be retried on a later viewport change.
        probedChildFolderIdsRef.current.delete(folderId);
      });
    }
  }, [
    folderCoverPriorityIndexes,
    library.folderChildrenByParent,
    library.loadingFolderIds,
    library.requestFolderChildren,
    library.showChildFolderContents,
    subfolderStripCollapsed,
    visibleSubfolderEntries
  ]);

  useEffect(() => {
    if (!library.showChildFolderContents || folderCoverPriorityIndexes.length === 0) {
      return;
    }
    const parentIds = new Set<number>();
    for (const index of folderCoverPriorityIndexes) {
      const entry = visibleSubfolderEntries[index];
      if (!entry) {
        continue;
      }
      const cursor = library.folderChildNextCursorByParent[entry.parentId];
      if (
        entry.siblingIndex >= entry.siblingCount - 8 &&
        cursor &&
        !library.loadingMoreFolderIds.has(entry.parentId)
      ) {
        // Request each (parent, cursor) page at most once. The cursor advances
        // after a page loads, so genuine pagination still proceeds; only the
        // re-run re-issue that fed the render loop is suppressed.
        const pageKey = `${entry.parentId}:${cursor}`;
        if (!requestedLoadMorePagesRef.current.has(pageKey)) {
          requestedLoadMorePagesRef.current.add(pageKey);
          parentIds.add(entry.parentId);
        }
      }
    }
    for (const parentId of parentIds) {
      void library.loadMoreFolderChildren(parentId);
    }
  }, [
    folderCoverPriorityIndexes,
    library.folderChildNextCursorByParent,
    library.folderChildrenByParent,
    library.loadMoreFolderChildren,
    library.loadingMoreFolderIds,
    library.showChildFolderContents,
    visibleSubfolderEntries
  ]);

  function handleOpenPreview(mediaId: number) {
    onOpenPreview(mediaId);
  }

  return (
    <section className="workspace" aria-label="Library workbench">
      <section
        className={previewMedia ? "grid-surface grid-surface-preview-active" : "grid-surface"}
        aria-label="Media workspace"
      >
        <div className="library-grid-content">
          <div
            aria-hidden={previewMedia ? true : undefined}
            className={
              previewMedia
                ? "library-browser-layout library-browser-layout--preview-covered"
                : "library-browser-layout"
            }
          >
            <MediaGrid
              aheadRowCount={AHEAD_THUMBNAIL_ROW_COUNT}
              contentHeader={contentHeader}
              emptyContent={undefined}
              folderSection={folderSection}
              gridPreferences={gridPreferences}
              items={library.media}
              layoutMode={layoutMode}
              loading={library.loading}
              loadingMore={library.loadingMoreMedia}
              mediaSlots={library.mediaSlots}
              onContextMenu={onMediaContextMenu}
              onLocateFolder={onLocateFolder}
              locateRequest={mediaLocateRequest}
              onLocateRequestSettled={onMediaLocateSettled}
              onOpenPreview={handleOpenPreview}
              onRequestMediaWindow={library.requestMediaWindow}
              onRequestThumbnailStates={library.requestThumbnailStates}
              onSelect={library.setSelectedMediaId}
              scrollPositionKey={mediaScrollKey}
              selectedMediaId={library.selectedMediaId}
              sidebarLayoutTransitionKey={sidebarLayoutTransitionKey}
              thumbnailStatesByMediaId={library.thumbnailStatesByMediaId}
              totalCount={library.mediaTotalCount}
            />
          </div>
        </div>
        <MediaGridRefreshIndicator
          active={library.loading || library.loadingMoreMedia}
          label="Refreshing media"
        />
        {previewMedia ? (
          <div className="central-preview-overlay">
            <CentralPreviewStage
              selectedMedia={previewMedia}
              thumbnail={library.thumbnailStatesByMediaId[previewMedia.id]}
              onClosePreview={onClosePreview}
              onCommandChange={onPreviewCommandChange}
              onPreviewMediaSettled={onPreviewMediaSettled}
              onPreviewNext={onPreviewNext}
              onPreviewPrevious={onPreviewPrevious}
              initialViewState={previewViewState}
              onViewStateChange={onPreviewViewStateChange}
            />
          </div>
        ) : null}
      </section>
    </section>
  );
}

export function LibraryInspectorPane({
  library,
  previewOpen
}: Pick<LibraryViewProps, "library" | "previewOpen">) {
  const selectedMedia = library.selectedMedia;
  const selectedFolder = library.selectedFolderInfo;
  const selectedFolderCoverMediaByFolderId = useFolderCovers(
    selectedFolder ? [selectedFolder] : [],
    { disabled: selectedFolder === null, activeScan: library.scanActive }
  );
  const selectedFolderCoverMedia = selectedFolder
    ? selectedFolderCoverMediaByFolderId.get(selectedFolder.id) ?? []
    : [];
  const selectedFolderCover = selectedFolderCoverMedia[0] ?? null;

  return (
    <PreviewPanel
      selectedFolder={library.selectedFolderInfo}
      selectedFolderCoverMedia={selectedFolderCoverMedia}
      selectedMedia={selectedMedia}
      showPreviewImage={!previewOpen}
      thumbnail={selectedMedia ? library.thumbnailStatesByMediaId[selectedMedia.id] : undefined}
      selectedFolderCoverThumbnail={
        selectedFolderCover ? library.thumbnailStatesByMediaId[selectedFolderCover.id] : undefined
      }
    >
      {selectedMedia ? (
        <InspectorMetadata
          fileId={selectedMedia.id}
          metadata={library.selectedMetadata}
          tags={library.tags}
          tagsById={library.tagsById}
          saving={library.metadataSaving}
          onUpdate={library.updateMetadata}
          onAddTag={library.addFileTag}
          onRemoveTag={library.removeFileTag}
          onCreateTag={library.createTag}
        />
      ) : null}
    </PreviewPanel>
  );
}

function mediaScrollPositionKey(
  layoutMode: LibraryLayoutMode,
  rootId: number | null,
  folderId: number | null
): string {
  return `${layoutMode}:${rootId ?? "root"}:${folderId ?? "folder"}`;
}

function readStoredSubfolderStripCollapsed(): boolean {
  try {
    return window.localStorage.getItem(SUBFOLDER_STRIP_COLLAPSED_STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}
