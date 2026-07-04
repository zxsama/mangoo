import { useEffect, useMemo, useState } from "react";
import type { FolderRecord, MediaRecord } from "@megle/core-client";
import type { LibraryState } from "../../core/useLibraryData";
import { MediaGrid } from "../media-grid/MediaGrid";
import type { LibraryGridPreferences } from "../media-grid/gridPreferences";
import type { LibraryLayoutMode } from "../media-grid/layoutMode";
import { CentralPreviewStage } from "../preview/CentralPreviewStage";
import { InspectorMetadata } from "../preview/InspectorMetadata";
import { PreviewPanel } from "../preview/PreviewPanel";
import { SubfolderCard, SubfolderStrip } from "./SubfolderStrip";
import { buildVisibleSubfolderEntries } from "./subfolderHierarchy";
import { useFolderCovers } from "./useFolderCovers";

const AHEAD_THUMBNAIL_ROW_COUNT = 4;
const SUBFOLDER_CHILD_PROBE_LIMIT = 12;
const SUBFOLDER_STRIP_COLLAPSED_STORAGE_KEY = "megle.library.subfolder-strip-collapsed";

interface LibraryViewProps {
  gridPreferences: LibraryGridPreferences;
  library: LibraryState;
  layoutMode: LibraryLayoutMode;
  previewOpen: boolean;
  onOpenPreview: (mediaId: number) => void;
  onClosePreview: () => void;
  onPreviewPrevious: () => void;
  onPreviewNext: () => void;
  onPreviewMediaSettled: (mediaId: number) => void;
  onPreviewViewStateChange: (state: { mode: "fit-long-edge" | "actual"; scale: number }) => void;
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
  onMediaContextMenu,
  onOpenPreview,
  onPreviewCommandChange,
  onPreviewMediaSettled,
  onPreviewNext,
  onPreviewPrevious,
  onPreviewViewStateChange,
  previewOpen
}: LibraryViewProps) {
  return (
    <>
      <LibraryCenterPane
        gridPreferences={gridPreferences}
        library={library}
        layoutMode={layoutMode}
        onClosePreview={onClosePreview}
        onFolderContextMenu={onFolderContextMenu}
        onMediaContextMenu={onMediaContextMenu}
        onOpenPreview={onOpenPreview}
        onPreviewCommandChange={onPreviewCommandChange}
        onPreviewMediaSettled={onPreviewMediaSettled}
        onPreviewNext={onPreviewNext}
        onPreviewPrevious={onPreviewPrevious}
        onPreviewViewStateChange={onPreviewViewStateChange}
        previewOpen={previewOpen}
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
  onMediaContextMenu,
  onOpenPreview,
  onPreviewCommandChange,
  onPreviewMediaSettled,
  onPreviewNext,
  onPreviewPrevious,
  onPreviewViewStateChange,
  previewOpen
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
    const folderIds = new Set<number>();
    for (const index of probeIndexes) {
      const folder = visibleSubfolderEntries[index]?.folder;
      if (!folder) {
        continue;
      }
      if (
        library.folderChildrenByParent[folder.id] === undefined &&
        !library.loadingFolderIds.has(folder.id)
      ) {
        folderIds.add(folder.id);
      }
      if (folderIds.size >= SUBFOLDER_CHILD_PROBE_LIMIT) {
        break;
      }
    }
    for (const folderId of folderIds) {
      void library.requestFolderChildren(folderId).catch(() => undefined);
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
      if (
        entry.siblingIndex >= entry.siblingCount - 8 &&
        library.folderChildNextCursorByParent[entry.parentId] &&
        !library.loadingMoreFolderIds.has(entry.parentId)
      ) {
        parentIds.add(entry.parentId);
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
        {library.error ? <div className="error-strip">{library.error}</div> : null}
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
              onOpenPreview={handleOpenPreview}
              onRequestMediaWindow={library.requestMediaWindow}
              onRequestThumbnailStates={library.requestThumbnailStates}
              onSelect={library.setSelectedMediaId}
              scrollPositionKey={mediaScrollKey}
              selectedMediaId={library.selectedMediaId}
              thumbnailStatesByMediaId={library.thumbnailStatesByMediaId}
              totalCount={library.mediaTotalCount}
            />
          </div>
        </div>
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
