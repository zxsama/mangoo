import { useVirtualizer } from "@tanstack/react-virtual";
import type { KeyboardEvent, ReactNode } from "react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties } from "react";
import type { MediaRecord, ThumbnailResponse } from "@megle/core-client";
import {
  mediaContentSignature,
  mediaFileContentSignature,
  preloadImageObjectUrl,
  previewPlaceholderDataUrl,
  prefetchOriginalPreview,
  prefetchThumbnailBlob,
  readCachedThumbnailObjectUrl,
  rememberThumbnailObjectUrl,
  requestOriginalPreviewBlob,
  requestThumbnailBlob,
  thumbnailObjectUrlCacheKey,
  type ThumbnailRequestPriority
} from "../../core/mediaResources";
import { workbenchLayout } from "../../design/tokens";
import type { LibraryGridPreferences } from "./gridPreferences";
import type { LibraryLayoutMode } from "./layoutMode";
import {
  buildLayoutGeometry,
  collectPlacementIndexesFromSegmentRange,
  collectScopedMediaInViewport,
  findDirectionalNeighborIndex,
  resolveScrollTopForPlacement,
  type FolderCoverLayoutPlacement
} from "./layoutGeometry";

const AHEAD_THUMBNAIL_ROW_COUNT = 4;
const VISIBLE_THUMBNAIL_REPOLL_MS = 150;
const AHEAD_THUMBNAIL_REPOLL_MS = 1000;
// Cap on ahead/behind tiles state-requested per sync. Sized to cover the full
// symmetric retention band (aheadRows above + below the viewport). The actual
// concurrent blob fetches are separately bounded by the foreground-resource
// scheduler (ahead=6), so a larger list here just means more tiles get their
// state tracked, not a request storm.
const AHEAD_PRIORITY_ITEM_LIMIT = 48;
const RECENT_VISIBLE_THUMBNAIL_STICKY_ITEM_LIMIT = 1024;
const RECENT_VISIBLE_THUMBNAIL_STICKY_MS = 10 * 60_000;
const ORIGINAL_FALLBACK_OBJECT_URL_CACHE_LIMIT = 128;
const SECTION_HEADER_HEIGHT = 32;
const CONTENT_HEADER_TOP_GAP = 24;
const CONTENT_SECTION_HEADER_HEIGHT = SECTION_HEADER_HEIGHT + CONTENT_HEADER_TOP_GAP;
const EMPTY_ROW_HEIGHT = 132;
const PROGRESSIVE_EXACT_LAYOUT_PAGE_SIZE = 96;
const LIST_WINDOW_ROW_HEIGHT_PX = 104;
const LIST_WINDOW_FRAME_HEIGHT_PX = 90;
const LIST_WINDOW_THUMBNAIL_WIDTH_PX = 108;
const LIST_WINDOW_THUMBNAIL_HEIGHT_PX = 72;
const WINDOWED_MEDIA_COUNT_THRESHOLD = 1000;
// The full-total justified/masonry layout materializes a placement + segment for
// EVERY item in the folder (O(totalCount)) and the layout rebuilds on each
// mediaSlots / estimatedRatio change while scrolling. That is fine for moderate
// folders, but at hundreds of thousands of items it allocates that many objects
// per scroll step and freezes the renderer — media-fetch callbacks starve, so
// tiles blank out and never refresh. Above this cap, justified/masonry folders
// fall back to the uniform windowed grid (O(viewport)), which already provides a
// correct total scroll height and placeholder slots.
const JUSTIFIED_FULL_TOTAL_MAX_ITEMS = 20000;
const ORIGINAL_FALLBACK_IMAGE_EXTENSIONS = new Set([
  "avif",
  "bmp",
  "gif",
  "jpeg",
  "jpg",
  "png",
  "tif",
  "tiff",
  "webp"
]);
const scrollPositionByKey = new Map<string, number>();
const originalFallbackObjectUrlCache = new Map<string, string>();
const EMPTY_MEDIA_ITEMS: MediaRecord[] = [];
type MediaGridCssVariables = CSSProperties & {
  "--library-tile-label-gap": string;
  "--library-tile-label-visible-height": string;
  "--subfolder-edge-shadow-alpha": string;
  "--subfolder-label-gap": string;
  "--subfolder-label-height": string;
  "--subfolder-label-visible-height": string;
  "--subfolder-cover-height": string;
  "--subfolder-row-gap": string;
  "--subfolder-tile-gap": string;
};

function measureVirtualGridViewport(element: HTMLElement) {
  const style = window.getComputedStyle(element);
  const horizontalPadding =
    Number.parseFloat(style.paddingLeft || "0") + Number.parseFloat(style.paddingRight || "0");
  const verticalPadding =
    Number.parseFloat(style.paddingTop || "0") + Number.parseFloat(style.paddingBottom || "0");

  return {
    height: Math.max(0, element.clientHeight - verticalPadding),
    width: Math.max(0, element.clientWidth - horizontalPadding)
  };
}

type VirtualSegment =
  | { type: "folder-header"; size: number }
  | { type: "folder-row"; folderSegmentIndex: number; size: number }
  | { type: "content-header"; size: number }
  | { type: "empty"; size: number }
  | { type: "media-row"; mediaSegmentIndex: number; size: number };

interface VirtualSectionLayout {
  contentHeaderIndex: number;
  emptyIndex: number;
  folderHeaderIndex: number;
  folderRowCount: number;
  folderRowStartIndex: number;
  mediaRowCount: number;
  mediaRowStartIndex: number;
  totalCount: number;
}

interface MediaWindowGeometry {
  contentWidth: number;
  columnCount: number;
  rowCount: number;
  rowHeight: number;
  tileWidth: number;
  totalSize: number;
}

interface FolderWindowGeometry {
  columnCount: number;
  coverHeight: number;
  frameHeight: number;
  rowCount: number;
  rowGap: number;
  rowHeight: number;
  tileWidth: number;
  totalSize: number;
}

interface MediaGridFolderSection {
  collapsed: boolean;
  itemCount: number;
  loading: boolean;
  header: ReactNode;
  onVisibleFolderIndexesChange?: (indexes: number[]) => void;
  renderFolder: (index: number, placement: FolderCoverLayoutPlacement) => ReactNode;
}

interface MediaGridProps {
  aheadRowCount?: number;
  contentHeader?: ReactNode;
  emptyContent?: ReactNode;
  folderSection?: MediaGridFolderSection;
  gridPreferences: LibraryGridPreferences;
  items: MediaRecord[];
  layoutMode: LibraryLayoutMode;
  mediaSlots?: Map<number, MediaRecord>;
  totalCount?: number;
  selectedMediaId: number | null;
  loading: boolean;
  loadingMore: boolean;
  onSelect: (mediaId: number) => void;
  onOpenPreview: (mediaId: number) => void;
  onRequestMediaWindow?: (startIndex: number, endIndex: number) => void;
  onRequestThumbnailStates: (mediaIds: number[], priority: ThumbnailRequestPriority) => void;
  scrollPositionKey?: string;
  thumbnailStatesByMediaId: Record<number, ThumbnailResponse>;
  onContextMenu?: (event: { item: MediaRecord; x: number; y: number; shiftKey: boolean }) => void;
}

function buildMediaWindowGeometry({
  count,
  gap,
  labelHeight,
  layoutMode,
  tileMinWidth,
  viewportWidth
}: {
  count: number;
  gap: number;
  labelHeight: number;
  layoutMode: LibraryLayoutMode;
  tileMinWidth: number;
  viewportWidth: number;
}): MediaWindowGeometry {
  const contentWidth = Math.max(tileMinWidth, Math.floor(viewportWidth || tileMinWidth));
  if (layoutMode === "list") {
    return {
      contentWidth,
      columnCount: 1,
      rowCount: Math.max(0, count),
      rowHeight: LIST_WINDOW_ROW_HEIGHT_PX,
      tileWidth: contentWidth,
      totalSize: Math.max(0, count) * LIST_WINDOW_ROW_HEIGHT_PX
    };
  }

  const columnCount = Math.max(1, Math.floor((contentWidth + gap) / (tileMinWidth + gap)));
  const tileWidth = Math.max(1, Math.floor((contentWidth - gap * (columnCount - 1)) / columnCount));
  const rowHeight = tileWidth + labelHeight + gap;
  const rowCount = Math.ceil(Math.max(0, count) / columnCount);
  return {
    contentWidth,
    columnCount,
    rowCount,
    rowHeight,
    tileWidth,
    totalSize: rowCount * rowHeight
  };
}

function buildFolderWindowGeometry({
  count,
  gap,
  labelHeight,
  tileMinWidth,
  viewportWidth
}: {
  count: number;
  gap: number;
  labelHeight: number;
  tileMinWidth: number;
  viewportWidth: number;
}): FolderWindowGeometry {
  const contentWidth = Math.max(tileMinWidth, Math.floor(viewportWidth || tileMinWidth));
  const columnCount = Math.max(1, Math.floor((contentWidth + gap) / (tileMinWidth + gap)));
  const tileWidth = Math.max(1, Math.floor((contentWidth - gap * (columnCount - 1)) / columnCount));
  const coverHeight = Math.round(tileWidth * 4 / 3);
  const contentInsetY = Math.max(24, Math.round(gap * 3));
  const frameHeight = coverHeight + labelHeight + contentInsetY;
  const rowGap = Math.max(3, Math.round(gap * 0.35));
  const rowHeight = frameHeight + rowGap;
  const rowCount = Math.ceil(Math.max(0, count) / columnCount);
  return {
    columnCount,
    coverHeight,
    frameHeight,
    rowCount,
    rowGap,
    rowHeight,
    tileWidth,
    totalSize: rowCount * rowHeight
  };
}

export function MediaGrid({
  aheadRowCount = AHEAD_THUMBNAIL_ROW_COUNT,
  contentHeader,
  emptyContent,
  folderSection,
  gridPreferences,
  items,
  layoutMode,
  mediaSlots,
  totalCount,
  selectedMediaId,
  loading,
  loadingMore,
  onRequestMediaWindow,
  onRequestThumbnailStates,
  onOpenPreview,
  onSelect,
  scrollPositionKey,
  thumbnailStatesByMediaId,
  onContextMenu
}: MediaGridProps) {
  const savedScrollTop = scrollPositionKey ? (scrollPositionByKey.get(scrollPositionKey) ?? 0) : 0;
  const parentRef = useRef<HTMLDivElement | null>(null);
  const previousMediaOffsetRef = useRef(0);
  const scrollPersistenceUnlockedRef = useRef(false);
  const restoringScrollRef = useRef(false);
  const lastRestoredScrollKeyRef = useRef<string | undefined>(undefined);
  const [viewportSize, setViewportSize] = useState({ height: 0, width: 0 });
  const [scrollTop, setScrollTop] = useState(savedScrollTop);

  useEffect(() => {
    const element = parentRef.current;
    if (!element) return;

    scrollPersistenceUnlockedRef.current = false;
    const updateViewportSize = () => {
      setViewportSize(measureVirtualGridViewport(element));
    };
    const observer = new ResizeObserver(updateViewportSize);
    observer.observe(element);

    const unlockScrollPersistence = () => {
      scrollPersistenceUnlockedRef.current = true;
    };
    const updateScrollTop = () => {
      const nextScrollTop = element.scrollTop;
      setScrollTop(nextScrollTop);
      if (!restoringScrollRef.current) {
        scrollPersistenceUnlockedRef.current = true;
      }
      if (scrollPositionKey && !restoringScrollRef.current) {
        scrollPositionByKey.set(scrollPositionKey, nextScrollTop);
      }
    };

    element.addEventListener("scroll", updateScrollTop, { passive: true });
    element.addEventListener("wheel", unlockScrollPersistence, { passive: true });
    element.addEventListener("pointerdown", unlockScrollPersistence, { passive: true });
    element.addEventListener("keydown", unlockScrollPersistence);
    updateViewportSize();
    setScrollTop(savedScrollTop);
    if (scrollPositionKey) {
      scrollPositionByKey.set(scrollPositionKey, savedScrollTop);
    }

    return () => {
      element.removeEventListener("scroll", updateScrollTop);
      element.removeEventListener("wheel", unlockScrollPersistence);
      element.removeEventListener("pointerdown", unlockScrollPersistence);
      element.removeEventListener("keydown", unlockScrollPersistence);
      observer.disconnect();
    };
  }, [savedScrollTop, scrollPositionKey]);

  const mediaItemCount = Math.max(items.length, totalCount ?? items.length);
  // grid/list use the uniform absolute-index window (buildMediaWindowGeometry).
  // adaptive/waterfall are justified/masonry: they now also lay out over the
  // FULL folder total using real ratios for loaded slots and a stable estimate
  // for unloaded ones, so the scroll height is correct immediately, unloaded
  // positions hold a placeholder, and real thumbnails fade in without repacking.
  const uniformWindowLayoutMode = layoutMode === "grid" || layoutMode === "list";
  const justifiedLayoutMode = layoutMode === "adaptive" || layoutMode === "waterfall";
  const largeMediaSet =
    mediaItemCount > items.length || mediaItemCount > WINDOWED_MEDIA_COUNT_THRESHOLD;
  // A justified/masonry folder past the cap cannot use the O(totalCount) full-
  // total layout without freezing the renderer, so it renders through the uniform
  // windowed grid instead (see JUSTIFIED_FULL_TOTAL_MAX_ITEMS).
  const justifiedTooLargeForFullTotal =
    justifiedLayoutMode && mediaItemCount > JUSTIFIED_FULL_TOTAL_MAX_ITEMS;
  // `windowedMedia` = the uniform absolute-index window path (grid/list, plus
  // over-cap justified folders). Kept narrow so the many render/virtual-row
  // branches that switch on it still mean "uniform mediaWindow rows".
  const windowedMedia =
    mediaSlots !== undefined &&
    (uniformWindowLayoutMode || justifiedTooLargeForFullTotal) &&
    largeMediaSet;
  // Justified/masonry full-total layout: drive the existing non-windowed
  // (layout.segments) render path, but over placements that span the whole
  // folder total (real ratio when loaded, estimate otherwise) and prefetch the
  // visible/ahead slot range so real thumbnails arrive before they scroll in.
  // Only for folders at or below the cap; larger ones use windowedMedia above.
  const justifiedFullTotal =
    mediaSlots !== undefined &&
    justifiedLayoutMode &&
    !justifiedTooLargeForFullTotal &&
    largeMediaSet;
  // Estimated aspect ratio for not-yet-loaded slots: average of the loaded
  // slots' real ratios (falls back to 1). Keeps the justified/masonry rows
  // stable so a late-arriving real ratio does not reflow the layout.
  const estimatedRatio = useMemo(() => {
    if (!mediaSlots || mediaSlots.size === 0) return 1;
    let sum = 0;
    let n = 0;
    for (const item of mediaSlots.values()) {
      if (item && item.width && item.height && item.width > 0 && item.height > 0) {
        sum += item.width / item.height;
        n += 1;
      }
    }
    return n > 0 ? sum / n : 1;
  }, [mediaSlots]);
  // The uniform-window (grid/list) and justified-full-total paths both drive the
  // layout without `layoutItems` (window rows / slot accessor respectively). Only
  // the small-folder fallback (no windowing) uses the ordered loaded list.
  const orderedLoadedMedia = useMemo(
    () =>
      mediaSlots !== undefined && !windowedMedia && !justifiedFullTotal
        ? compactMediaSlots(mediaSlots)
        : items,
    [items, mediaSlots, windowedMedia, justifiedFullTotal]
  );
  const layoutItems =
    windowedMedia || justifiedFullTotal ? EMPTY_MEDIA_ITEMS : orderedLoadedMedia;
  const layout = useMemo(
    () =>
      buildLayoutGeometry({
        gap: gridPreferences.tileGap,
        items: layoutItems,
        labelHeight: gridPreferences.tileLabelHeight,
        layoutMode,
        viewportWidth: viewportSize.width,
        tileMinWidth: workbenchLayout.tileMinWidth,
        ...(justifiedFullTotal
          ? {
              totalCount: mediaItemCount,
              getItemAt: (index: number) => mediaSlots?.get(index),
              estimatedRatio
            }
          : {})
      }),
    [
      gridPreferences.tileGap,
      gridPreferences.tileLabelHeight,
      layoutItems,
      layoutMode,
      viewportSize.width,
      justifiedFullTotal,
      mediaItemCount,
      mediaSlots,
      estimatedRatio
    ]
  );
  const mediaWindow = useMemo(
    () =>
      buildMediaWindowGeometry({
        count: mediaItemCount,
        gap: gridPreferences.tileGap,
        labelHeight: gridPreferences.tileLabelHeight,
        layoutMode,
        tileMinWidth: workbenchLayout.tileMinWidth,
        viewportWidth: viewportSize.width
      }),
    [
      gridPreferences.tileGap,
      gridPreferences.tileLabelHeight,
      layoutMode,
      mediaItemCount,
      viewportSize.width
    ]
  );
  const activeMediaEstimatedSegmentSize = windowedMedia
    ? mediaWindow.rowHeight
    : layout.estimatedSegmentSize;
  const folderWindow = useMemo(
    () =>
      buildFolderWindowGeometry({
        count: folderSection && !folderSection.collapsed ? folderSection.itemCount : 0,
        gap: gridPreferences.folderTileGap,
        labelHeight: gridPreferences.folderTileLabelHeight,
        tileMinWidth: workbenchLayout.tileMinWidth,
        viewportWidth: viewportSize.width
      }),
    [
      folderSection?.collapsed,
      folderSection?.itemCount,
      gridPreferences.folderTileGap,
      gridPreferences.folderTileLabelHeight,
      viewportSize.width
    ]
  );
  const folderSectionVisible = Boolean(folderSection);
  const folderHeaderHeight = folderSectionVisible ? SECTION_HEADER_HEIGHT : 0;
  const folderGridOffset = folderHeaderHeight;
  const contentHeaderOffset = folderGridOffset + folderWindow.totalSize;
  const contentHeaderHeight = contentHeader
    ? folderSectionVisible
      ? CONTENT_SECTION_HEADER_HEIGHT
      : SECTION_HEADER_HEIGHT
    : 0;
  const mediaOffset = contentHeaderOffset + contentHeaderHeight;
  const loadedMediaById = useMemo(() => {
    const map = new Map<number, MediaRecord>();
    for (const item of items) {
      map.set(item.id, item);
    }
    // Read slot records directly (order-independent) so windowed mode, which
    // skips the sorted `orderedLoadedMedia`, still resolves loaded media by id.
    if (mediaSlots !== undefined) {
      for (const item of mediaSlots.values()) {
        if (item) {
          map.set(item.id, item);
        }
      }
    } else {
      for (const item of orderedLoadedMedia) {
        map.set(item.id, item);
      }
    }
    return map;
  }, [items, mediaSlots, orderedLoadedMedia]);
  const layoutItemIndexById = useMemo(() => {
    const map = new Map<number, number>();
    layoutItems.forEach((item, index) => {
      map.set(item.id, index);
    });
    return map;
  }, [layoutItems]);
  const slotIndexById = useMemo(() => {
    const map = new Map<number, number>();
    mediaSlots?.forEach((item, index) => {
      if (item) {
        map.set(item.id, index);
      }
    });
    return map;
  }, [mediaSlots]);
  const placementSegmentIndexByPlacementIndex = useMemo(() => {
    const map = new Map<number, number>();
    layout.segments.forEach((segment, segmentIndex) => {
      segment.itemIndexes.forEach((placementIndex) => {
        if (!map.has(placementIndex)) {
          map.set(placementIndex, segmentIndex);
        }
      });
    });
    return map;
  }, [layout.segments]);

  const virtualSectionLayout = useMemo<VirtualSectionLayout>(() => {
    let nextIndex = 0;
    const folderHeaderIndex = folderSectionVisible ? nextIndex++ : -1;
    const folderRowCount =
      folderSectionVisible && !folderSection?.collapsed ? folderWindow.rowCount : 0;
    const folderRowStartIndex = folderRowCount > 0 ? nextIndex : -1;
    nextIndex += folderRowCount;
    const contentHeaderIndex = contentHeader ? nextIndex++ : -1;
    const emptyIndex = mediaItemCount === 0 && emptyContent !== undefined ? nextIndex++ : -1;
    const mediaRowCount = windowedMedia ? mediaWindow.rowCount : layout.segments.length;
    const mediaRowStartIndex = mediaRowCount > 0 ? nextIndex : -1;
    nextIndex += mediaRowCount;
    return {
      contentHeaderIndex,
      emptyIndex,
      folderHeaderIndex,
      folderRowCount,
      folderRowStartIndex,
      mediaRowCount,
      mediaRowStartIndex,
      totalCount: nextIndex
    };
  }, [
    contentHeader,
    emptyContent,
    folderSection?.collapsed,
    folderSectionVisible,
    folderWindow.rowCount,
    layout.segments.length,
    mediaItemCount,
    mediaWindow.rowCount,
    windowedMedia
  ]);
  const resolveVirtualSegment = useCallback(
    (index: number): VirtualSegment | null => {
      if (index === virtualSectionLayout.folderHeaderIndex) {
        return { type: "folder-header", size: SECTION_HEADER_HEIGHT };
      }
      if (
        virtualSectionLayout.folderRowStartIndex >= 0 &&
        index >= virtualSectionLayout.folderRowStartIndex &&
        index < virtualSectionLayout.folderRowStartIndex + virtualSectionLayout.folderRowCount
      ) {
        return {
          type: "folder-row",
          folderSegmentIndex: index - virtualSectionLayout.folderRowStartIndex,
          size: folderWindow.rowHeight
        };
      }
      if (index === virtualSectionLayout.contentHeaderIndex) {
        return {
          type: "content-header",
          size: folderSectionVisible ? CONTENT_SECTION_HEADER_HEIGHT : SECTION_HEADER_HEIGHT
        };
      }
      if (index === virtualSectionLayout.emptyIndex) {
        return { type: "empty", size: EMPTY_ROW_HEIGHT };
      }
      if (
        virtualSectionLayout.mediaRowStartIndex >= 0 &&
        index >= virtualSectionLayout.mediaRowStartIndex &&
        index < virtualSectionLayout.mediaRowStartIndex + virtualSectionLayout.mediaRowCount
      ) {
        const mediaSegmentIndex = index - virtualSectionLayout.mediaRowStartIndex;
        return {
          type: "media-row",
          mediaSegmentIndex,
          size: windowedMedia
            ? mediaWindow.rowHeight
            : layout.segments[mediaSegmentIndex]?.size ?? activeMediaEstimatedSegmentSize
        };
      }
      return null;
    },
    [
      activeMediaEstimatedSegmentSize,
      folderSectionVisible,
      folderWindow.rowHeight,
      layout.segments,
      mediaWindow.rowHeight,
      virtualSectionLayout,
      windowedMedia
    ]
  );
  const resolveMediaVirtualSegmentIndex = useCallback(
    (mediaSegmentIndex: number) => {
      if (
        mediaSegmentIndex < 0 ||
        mediaSegmentIndex >= virtualSectionLayout.mediaRowCount ||
        virtualSectionLayout.mediaRowStartIndex < 0
      ) {
        return undefined;
      }
      return virtualSectionLayout.mediaRowStartIndex + mediaSegmentIndex;
    },
    [virtualSectionLayout]
  );
  const virtualRows = useVirtualizer({
    count: virtualSectionLayout.totalCount,
    getScrollElement: () => parentRef.current,
    estimateSize: (index) => resolveVirtualSegment(index)?.size ?? activeMediaEstimatedSegmentSize,
    initialOffset: savedScrollTop,
    // Keep more rows mounted beyond the viewport so a small scroll-up/-down does
    // not unmount+remount tiles. A remounted <img> — even with a cached blob src
    // — must RE-DECODE before it paints, which is the brief blank "reload" flash.
    // Larger overscan means the working set stays mounted (already decoded), so
    // back-and-forth scrolling never re-decodes. Bounded (the virtualizer only
    // ever mounts viewport+overscan), so memory stays capped — not permanent.
    overscan: layoutMode === "waterfall" ? 10 : 8,
  });
  const virtualItems = virtualRows.getVirtualItems();

  useLayoutEffect(() => {
    const element = parentRef.current;
    const previousMediaOffset = previousMediaOffsetRef.current;
    previousMediaOffsetRef.current = mediaOffset;
    if (!element || previousMediaOffset === mediaOffset) {
      return;
    }

    const mediaOffsetDelta = mediaOffset - previousMediaOffset;
    if (Math.abs(mediaOffsetDelta) < 1 || element.scrollTop <= previousMediaOffset + 1) {
      return;
    }

    const nextScrollTop = Math.max(0, element.scrollTop + mediaOffsetDelta);
    element.scrollTop = nextScrollTop;
    setScrollTop(nextScrollTop);
    if (scrollPositionKey) {
      scrollPositionByKey.set(scrollPositionKey, nextScrollTop);
    }
  }, [mediaOffset, scrollPositionKey]);

  useLayoutEffect(() => {
    const element = parentRef.current;
    if (!element) return;
    const scrollKeyChanged = lastRestoredScrollKeyRef.current !== scrollPositionKey;
    lastRestoredScrollKeyRef.current = scrollPositionKey;
    if (!scrollKeyChanged && scrollPersistenceUnlockedRef.current) {
      return;
    }
    if (savedScrollTop > 0 && mediaItemCount === 0 && !(folderSection?.itemCount ?? 0)) {
      return;
    }
    if (!scrollKeyChanged && savedScrollTop === 0 && element.scrollTop !== 0) {
      return;
    }
    if (
      !scrollKeyChanged &&
      savedScrollTop > 0 &&
      element.scrollTop > 0 &&
      Math.abs(element.scrollTop - savedScrollTop) > 2
    ) {
      return;
    }

    restoringScrollRef.current = true;
    if (element.scrollTop !== savedScrollTop) {
      element.scrollTop = savedScrollTop;
    }
    setScrollTop(savedScrollTop);
    if (scrollPositionKey) {
      scrollPositionByKey.set(scrollPositionKey, savedScrollTop);
    }

    const restoreFrame = window.requestAnimationFrame(() => {
      restoringScrollRef.current = false;
    });
    return () => {
      window.cancelAnimationFrame(restoreFrame);
      restoringScrollRef.current = false;
    };
  }, [folderSection?.itemCount, mediaItemCount, savedScrollTop, scrollPositionKey]);

  const mediaScrollTop = Math.max(0, scrollTop - mediaOffset);
  const visibleRangeEnd = Math.max(
    0,
    scrollTop + Math.max(viewportSize.height, activeMediaEstimatedSegmentSize) - mediaOffset
  );
  const visibleMediaIndexRange = useMemo(() => {
    if (!windowedMedia) return null;
    const startRow = Math.max(0, Math.floor(mediaScrollTop / mediaWindow.rowHeight));
    const endRow = Math.max(
      startRow,
      Math.floor(Math.max(mediaScrollTop, visibleRangeEnd - 1) / mediaWindow.rowHeight)
    );
    return {
      start: Math.min(mediaItemCount, startRow * mediaWindow.columnCount),
      end: Math.min(mediaItemCount, (endRow + 1) * mediaWindow.columnCount)
    };
  }, [
    mediaWindow.columnCount,
    mediaWindow.rowHeight,
    mediaItemCount,
    mediaScrollTop,
    visibleRangeEnd,
    windowedMedia
  ]);
  const aheadMediaIndexRange = useMemo(() => {
    if (!windowedMedia || !visibleMediaIndexRange) return null;
    const aheadRows = Math.max(1, aheadRowCount);
    // Symmetric retention band: `aheadRows` both BELOW and ABOVE the viewport.
    // Without the "behind" band, tiles just scrolled past (above the viewport)
    // left the visible/ahead scope, so their prioritized tasks were demoted and
    // their thumbnails re-fetched on scroll-back ("already-loaded images
    // reload"). Symmetric coverage means scrolling either way finds tiles warm.
    // Safe to be full-symmetric now: the concurrent fetch set is bounded by the
    // foreground-resource scheduler (MAX_FOREGROUND_RESOURCE_REQUESTS=12,
    // ahead=6) and the client request scheduler has a hard timeout + no slot
    // leak, so this no longer risks the 6-connection-pool deadlock that forced
    // the earlier one-directional prefetch (memory UPDATE 10/11, since fixed).
    const columns = mediaWindow.columnCount;
    return {
      start: Math.max(0, visibleMediaIndexRange.start - aheadRows * columns),
      end: Math.min(mediaItemCount, visibleMediaIndexRange.end + aheadRows * columns)
    };
  }, [
    aheadRowCount,
    mediaWindow.columnCount,
    mediaItemCount,
    visibleMediaIndexRange,
    windowedMedia
  ]);
  const visibleMedia = useMemo(
    () => {
      if (windowedMedia && visibleMediaIndexRange) {
        const ids: number[] = [];
        const signatures: string[] = [];
        for (let index = visibleMediaIndexRange.start; index < visibleMediaIndexRange.end; index += 1) {
          const item = mediaSlots?.get(index);
          if (!item || item.id === selectedMediaId) continue;
          ids.push(item.id);
          signatures.push(mediaContentSignature(item));
        }
        return {
          ids,
          key: `${visibleMediaIndexRange.start}:${visibleMediaIndexRange.end}`,
          signatureKey: signatures.join("|")
        };
      }
      return collectScopedMediaInViewport(layout, mediaScrollTop, visibleRangeEnd, {
        excludeMediaId: selectedMediaId
      });
    },
    [
      layout,
      mediaScrollTop,
      mediaSlots,
      selectedMediaId,
      visibleMediaIndexRange,
      visibleRangeEnd,
      windowedMedia
    ]
  );
  const aheadMedia = useMemo(() => {
    if (windowedMedia && aheadMediaIndexRange) {
      const ids: number[] = [];
      const signatures: string[] = [];
      for (let index = aheadMediaIndexRange.start; index < aheadMediaIndexRange.end; index += 1) {
        const item = mediaSlots?.get(index);
        if (!item || item.id === selectedMediaId) continue;
        ids.push(item.id);
        signatures.push(mediaContentSignature(item));
      }
      return {
        ids,
        key: `${aheadMediaIndexRange.start}:${aheadMediaIndexRange.end}`,
        signatureKey: signatures.join("|")
      };
    }
    const aheadStart = visibleRangeEnd;
    const aheadEnd = aheadStart + aheadRowCount * activeMediaEstimatedSegmentSize;
    return collectScopedMediaInViewport(layout, aheadStart, aheadEnd, {
      excludeMediaId: selectedMediaId
    });
  }, [
    activeMediaEstimatedSegmentSize,
    aheadMediaIndexRange,
    aheadRowCount,
    layout,
    mediaSlots,
    selectedMediaId,
    visibleRangeEnd,
    windowedMedia
  ]);

  useEffect(() => {
    if (!windowedMedia || !onRequestMediaWindow || !visibleMediaIndexRange) {
      return;
    }
    onRequestMediaWindow(
      visibleMediaIndexRange.start,
      aheadMediaIndexRange?.end ?? visibleMediaIndexRange.end
    );
  }, [
    aheadMediaIndexRange,
    onRequestMediaWindow,
    visibleMediaIndexRange,
    windowedMedia
  ]);

  const progressiveExactNextMissingIndex =
    !windowedMedia && !justifiedFullTotal && mediaSlots !== undefined
      ? firstMissingMediaSlotIndex(mediaSlots, mediaItemCount)
      : null;
  const progressiveExactLayoutHasMore = progressiveExactNextMissingIndex !== null;

  const visibleMediaIds = visibleMedia.ids;
  const visibleMediaContentSignatures = useMemo(
    () =>
      visibleMediaIds
        .map((mediaId) => {
          const item = loadedMediaById.get(mediaId);
          return item ? mediaContentSignature(item) : "";
        })
        .filter(Boolean)
        .join("|"),
    [loadedMediaById, visibleMediaIds]
  );
  const visibleMediaSignatureKey = visibleMedia.signatureKey || visibleMediaContentSignatures;
  const aheadMediaIds = aheadMedia.ids;
  const aheadMediaSignatureKey = aheadMedia.signatureKey;
  const visiblePriorityMediaIds = visibleMediaIds;
  const visiblePriorityMediaKey = visiblePriorityMediaIds.join(":");
  const recentVisibleThumbnailIdsRef = useRef<Map<number, number>>(new Map());
  const visiblePriorityMediaIdSet = useMemo(
    () => new Set(visiblePriorityMediaIds),
    [visiblePriorityMediaIds]
  );
  useEffect(() => {
    const now = Date.now();
    const recent = recentVisibleThumbnailIdsRef.current;
    for (const mediaId of visiblePriorityMediaIds) {
      recent.set(mediaId, now);
    }
    for (const [mediaId, seenAt] of recent) {
      if (now - seenAt > RECENT_VISIBLE_THUMBNAIL_STICKY_MS) {
        recent.delete(mediaId);
      }
    }
    while (recent.size > RECENT_VISIBLE_THUMBNAIL_STICKY_ITEM_LIMIT * 4) {
      const oldestKey = recent.keys().next().value;
      if (oldestKey === undefined) {
        break;
      }
      recent.delete(oldestKey);
    }
  }, [visiblePriorityMediaKey]);
  const aheadPriorityMediaIds = useMemo(
    () => {
      const aheadIds = aheadMediaIds
        .filter((mediaId) => !visiblePriorityMediaIdSet.has(mediaId))
        .slice(0, AHEAD_PRIORITY_ITEM_LIMIT);
      const stickyPendingIds = Array.from(recentVisibleThumbnailIdsRef.current.entries())
        .sort((left, right) => right[1] - left[1])
        .map(([mediaId]) => mediaId)
        .filter((mediaId) => !visiblePriorityMediaIdSet.has(mediaId))
        .filter((mediaId) => !aheadIds.includes(mediaId))
        .filter((mediaId) => {
          const item = loadedMediaById.get(mediaId);
          return item ? shouldRefreshThumbnailState(item, thumbnailStatesByMediaId[mediaId]) : false;
        })
        .slice(0, RECENT_VISIBLE_THUMBNAIL_STICKY_ITEM_LIMIT);
      return [...aheadIds, ...stickyPendingIds];
    },
    [aheadMediaIds, loadedMediaById, thumbnailStatesByMediaId, visiblePriorityMediaIdSet]
  );
  const aheadPriorityMediaKey = aheadPriorityMediaIds.join(":");
  const hasVisiblePending = useMemo(
    () =>
      visiblePriorityMediaIds.some((mediaId) => {
        const item = loadedMediaById.get(mediaId);
        return item ? shouldRefreshThumbnailState(item, thumbnailStatesByMediaId[mediaId]) : false;
      }),
    [loadedMediaById, thumbnailStatesByMediaId, visiblePriorityMediaIds]
  );
  const hasAheadPending = useMemo(
    () =>
      aheadPriorityMediaIds.some((mediaId) => {
        const item = loadedMediaById.get(mediaId);
        return item ? shouldRefreshThumbnailState(item, thumbnailStatesByMediaId[mediaId]) : false;
      }),
    [aheadPriorityMediaIds, loadedMediaById, thumbnailStatesByMediaId]
  );
  // Which ahead tiles are currently READY (either the record says so or the
  // polled state does). Keyed so the prefetch effect below re-fires when a
  // freshly-scanned folder's thumbnail states resolve to ready — otherwise
  // ahead prefetch would never start for a new folder (the record carries no
  // per-row thumbnail state; readiness only arrives via the state poll).
  const aheadReadyStateKey = useMemo(
    () =>
      aheadPriorityMediaIds
        .filter((mediaId) => thumbnailStatesByMediaId[mediaId]?.state === "ready")
        .join(":"),
    [aheadPriorityMediaIds, thumbnailStatesByMediaId]
  );

  useEffect(() => {
    if (visiblePriorityMediaIds.length === 0) {
      return;
    }
    onRequestThumbnailStates(visiblePriorityMediaIds, "visible");
  }, [
    onRequestThumbnailStates,
    visiblePriorityMediaKey,
    visibleMediaSignatureKey
  ]);

  useEffect(() => {
    if (aheadPriorityMediaIds.length === 0) {
      return;
    }
    onRequestThumbnailStates(aheadPriorityMediaIds, "ahead");
  }, [
    aheadPriorityMediaKey,
    aheadMediaSignatureKey,
    onRequestThumbnailStates
  ]);

  // Prefetch the actual thumbnail BYTES for ahead tiles into the decoded
  // object-URL cache, so a tile that scrolls into view renders instantly
  // instead of blob-fetching + decoding + fading in at mount. Requesting only
  // the thumbnail STATE (above) is not enough — that leaves the image fetch to
  // fire on mount, which is exactly the "loads only once it scrolls in" the
  // user reported. Blob prefetch runs at "ahead" resource priority so it yields
  // to visible tiles in the request scheduler.
  useEffect(() => {
    if (aheadPriorityMediaIds.length === 0) {
      return;
    }
    for (const mediaId of aheadPriorityMediaIds) {
      const item = loadedMediaById.get(mediaId);
      if (!item) {
        continue;
      }
      // Prefetch when the record OR the polled state reports ready. Passing the
      // polled state lets a fresh folder start prefetching as soon as its state
      // poll resolves, without waiting for the record to carry thumbnail state.
      const polledReady = thumbnailStatesByMediaId[mediaId]?.state === "ready";
      prefetchThumbnailBlob(item, { resourcePriority: "ahead", assumeReady: polledReady });
    }
  }, [aheadPriorityMediaKey, aheadMediaSignatureKey, aheadReadyStateKey, loadedMediaById, thumbnailStatesByMediaId]);

  useEffect(() => {
    if (!hasVisiblePending && !hasAheadPending) {
      return;
    }

    const repollDelayMs = hasVisiblePending
      ? VISIBLE_THUMBNAIL_REPOLL_MS
      : AHEAD_THUMBNAIL_REPOLL_MS;
    const timer = window.setTimeout(() => {
      if (hasVisiblePending) {
        onRequestThumbnailStates(visiblePriorityMediaIds, "visible");
      }
      if (hasAheadPending) {
        onRequestThumbnailStates(aheadPriorityMediaIds, "ahead");
      }
    }, repollDelayMs);
    return () => window.clearTimeout(timer);
  }, [
    aheadPriorityMediaKey,
    aheadMediaSignatureKey,
    hasAheadPending,
    hasVisiblePending,
    onRequestThumbnailStates,
    visiblePriorityMediaKey,
    visibleMediaSignatureKey
  ]);

  const tileLabelGap = resolveTileLabelGap(gridPreferences.tileLabelHeight);
  const tileLabelVisibleHeight = Math.max(
    12,
    gridPreferences.tileLabelHeight - tileLabelGap
  );
  const folderTileLabelGap = resolveTileLabelGap(gridPreferences.folderTileLabelHeight);
  const folderTileLabelVisibleHeight = Math.max(
    12,
    gridPreferences.folderTileLabelHeight - folderTileLabelGap
  );
  const gridStyle = useMemo<MediaGridCssVariables>(
    () => ({
      "--library-tile-label-gap": `${tileLabelGap}px`,
      "--library-tile-label-visible-height": `${tileLabelVisibleHeight}px`,
      "--subfolder-edge-shadow-alpha": `${gridPreferences.folderEdgeShadowAlpha / 100}`,
      "--subfolder-label-gap": `${folderTileLabelGap}px`,
      "--subfolder-label-height": `${gridPreferences.folderTileLabelHeight}px`,
      "--subfolder-label-visible-height": `${folderTileLabelVisibleHeight}px`,
      "--subfolder-cover-height": `${folderWindow.coverHeight}px`,
      "--subfolder-row-gap": `${folderWindow.rowGap}px`,
      "--subfolder-tile-gap": `${gridPreferences.folderTileGap}px`
    }),
    [
      folderTileLabelGap,
      folderTileLabelVisibleHeight,
      folderWindow.coverHeight,
      folderWindow.rowGap,
      gridPreferences.folderEdgeShadowAlpha,
      gridPreferences.folderTileGap,
      gridPreferences.folderTileLabelHeight,
      tileLabelGap,
      tileLabelVisibleHeight
    ]
  );

  const renderedMediaSegmentIndexes = virtualItems
    .map((item) => resolveVirtualSegment(item.index))
    .filter((segment): segment is Extract<VirtualSegment, { type: "media-row" }> =>
      segment?.type === "media-row"
    )
    .map((segment) => segment.mediaSegmentIndex);
  const renderedPlacementIndexes =
    !windowedMedia && renderedMediaSegmentIndexes.length > 0
      ? collectPlacementIndexesFromSegmentRange(
          layout,
          Math.min(...renderedMediaSegmentIndexes),
          Math.max(...renderedMediaSegmentIndexes)
        )
      : [];
  const renderedWindowedMediaIndexes = windowedMedia
    ? renderedMediaSegmentIndexes.flatMap((rowIndex) => {
        const start = rowIndex * mediaWindow.columnCount;
        const end = Math.min(mediaItemCount, start + mediaWindow.columnCount);
        return Array.from({ length: Math.max(0, end - start) }, (_, index) => start + index);
      })
    : [];
  // Justified/masonry full-total prefetch: the rendered placement indices are
  // absolute slot indices, so request the media window covering the visible
  // range plus an ahead overscan. This loads real thumbnails before they scroll
  // into view, so the estimated-ratio placeholders are replaced without a
  // visible reflow.
  const justifiedPrefetchRange = useMemo(() => {
    if (!justifiedFullTotal || renderedPlacementIndexes.length === 0) {
      return null;
    }
    let min = Infinity;
    let max = -Infinity;
    for (const index of renderedPlacementIndexes) {
      if (index < min) min = index;
      if (index > max) max = index;
    }
    if (min === Infinity) return null;
    const span = Math.max(1, max - min + 1);
    return {
      start: Math.max(0, min),
      end: Math.min(mediaItemCount, max + 1 + span) // one screenful of ahead overscan
    };
  }, [justifiedFullTotal, renderedPlacementIndexes, mediaItemCount]);
  const justifiedPrefetchKey = justifiedPrefetchRange
    ? `${justifiedPrefetchRange.start}:${justifiedPrefetchRange.end}`
    : null;
  useEffect(() => {
    if (!justifiedFullTotal || !onRequestMediaWindow || !justifiedPrefetchRange) {
      return;
    }
    onRequestMediaWindow(justifiedPrefetchRange.start, justifiedPrefetchRange.end);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [justifiedFullTotal, onRequestMediaWindow, justifiedPrefetchKey]);
  const renderedFolderSegmentIndexes = virtualItems
    .map((item) => resolveVirtualSegment(item.index))
    .filter((segment): segment is Extract<VirtualSegment, { type: "folder-row" }> =>
      segment?.type === "folder-row"
    )
    .map((segment) => segment.folderSegmentIndex);
  const renderedFolderIndexes =
    renderedFolderSegmentIndexes.length > 0
      ? renderedFolderSegmentIndexes.flatMap((rowIndex) => {
          const start = rowIndex * folderWindow.columnCount;
          const end = Math.min(folderSection?.itemCount ?? 0, start + folderWindow.columnCount);
          return Array.from({ length: Math.max(0, end - start) }, (_, index) => start + index);
        })
      : [];
  const visibleFolderIndexes = useMemo(() => {
    if (
      !folderSection ||
      folderSection.collapsed ||
      folderWindow.rowHeight <= 0 ||
      folderWindow.columnCount <= 0 ||
      folderWindow.totalSize <= 0
    ) {
      return [];
    }

    const folderVisibleStart = Math.max(0, scrollTop - folderGridOffset);
    const folderVisibleEnd = Math.min(
      folderWindow.totalSize,
      Math.max(0, scrollTop + viewportSize.height - folderGridOffset)
    );
    if (folderVisibleEnd <= folderVisibleStart) {
      return [];
    }

    const startRow = Math.max(0, Math.floor(folderVisibleStart / folderWindow.rowHeight));
    const endRow = Math.max(
      startRow,
      Math.floor(Math.max(folderVisibleStart, folderVisibleEnd - 1) / folderWindow.rowHeight)
    );
    const indexes: number[] = [];
    for (let rowIndex = startRow; rowIndex <= endRow; rowIndex += 1) {
      const start = rowIndex * folderWindow.columnCount;
      const end = Math.min(folderSection.itemCount, start + folderWindow.columnCount);
      for (let index = start; index < end; index += 1) {
        indexes.push(index);
      }
    }
    return indexes;
  }, [
    folderGridOffset,
    folderSection,
    folderWindow.columnCount,
    folderWindow.rowHeight,
    folderWindow.totalSize,
    scrollTop,
    viewportSize.height
  ]);
  const prioritizedRenderedFolderIndexes = useMemo(() => {
    const seen = new Set<number>();
    const indexes: number[] = [];
    for (const index of visibleFolderIndexes) {
      if (seen.has(index)) continue;
      seen.add(index);
      indexes.push(index);
    }
    for (const index of renderedFolderIndexes) {
      if (seen.has(index)) continue;
      seen.add(index);
      indexes.push(index);
    }
    return indexes;
  }, [renderedFolderIndexes, visibleFolderIndexes]);
  const renderedFolderIndexKey = prioritizedRenderedFolderIndexes.join(":");

  useEffect(() => {
    if (
      !progressiveExactLayoutHasMore ||
      loadingMore ||
      !onRequestMediaWindow ||
      renderedMediaSegmentIndexes.length === 0 ||
      layout.segments.length === 0
    ) {
      return;
    }

    const lastRenderedMediaSegment = Math.max(...renderedMediaSegmentIndexes);
    if (lastRenderedMediaSegment < layout.segments.length - 2) {
      return;
    }

    const nextStartIndex = progressiveExactNextMissingIndex;
    if (nextStartIndex === null) {
      return;
    }

    onRequestMediaWindow(
      nextStartIndex,
      Math.min(mediaItemCount, nextStartIndex + PROGRESSIVE_EXACT_LAYOUT_PAGE_SIZE)
    );
  }, [
    layout.segments.length,
    loadingMore,
    mediaItemCount,
    onRequestMediaWindow,
    progressiveExactLayoutHasMore,
    progressiveExactNextMissingIndex,
    renderedMediaSegmentIndexes
  ]);

  useEffect(() => {
    if (!folderSection?.onVisibleFolderIndexesChange) {
      return;
    }
    folderSection.onVisibleFolderIndexesChange(prioritizedRenderedFolderIndexes);
  }, [
    folderSection,
    renderedFolderIndexKey,
    prioritizedRenderedFolderIndexes
  ]);

  function moveSelection(direction: "left" | "right" | "up" | "down") {
    if (windowedMedia) {
      if (mediaItemCount === 0) return;
      const currentIndex =
        selectedMediaId !== null
          ? slotIndexById.get(selectedMediaId) ?? visibleMediaIndexRange?.start ?? 0
          : visibleMediaIndexRange?.start ?? 0;
      const step = direction === "up" || direction === "down" ? mediaWindow.columnCount : 1;
      const delta = direction === "left" || direction === "up" ? -step : step;
      selectWindowedIndex(Math.max(0, Math.min(mediaItemCount - 1, currentIndex + delta)));
      return;
    }

    if (layoutItems.length === 0) return;
    const currentIndex =
      selectedMediaId !== null ? layoutItemIndexById.get(selectedMediaId) ?? 0 : 0;
    const nextIndex = findDirectionalNeighborIndex(layout.placements, currentIndex, direction);
    selectIndex(nextIndex);
  }

  function selectIndex(index: number) {
    const item = layoutItems[index];
    const placement = layout.placements[index];
    if (!item || !placement) return;

    onSelect(item.id);
    const segmentIndex = placementSegmentIndexByPlacementIndex.get(index);
    const virtualSegmentIndex =
      segmentIndex !== undefined ? resolveMediaVirtualSegmentIndex(segmentIndex) : undefined;
    if (virtualSegmentIndex !== undefined) {
      virtualRows.scrollToIndex(virtualSegmentIndex, { align: "auto" });
    }
    const element = parentRef.current;
    if (!element) {
      return;
    }
    const nextScrollTop = mediaOffset + resolveScrollTopForPlacement(
      placement,
      element.clientHeight,
      Math.max(0, element.scrollTop - mediaOffset),
      layout.totalSize
    );
    if (nextScrollTop !== element.scrollTop) {
      scrollPersistenceUnlockedRef.current = true;
      element.scrollTop = nextScrollTop;
    }
  }

  function selectWindowedIndex(index: number) {
    if (mediaItemCount === 0) return;
    const clampedIndex = Math.max(0, Math.min(mediaItemCount - 1, index));
    const item = mediaSlots?.get(clampedIndex);
    if (item) {
      onSelect(item.id);
    } else {
      onRequestMediaWindow?.(
        clampedIndex,
        Math.min(mediaItemCount, clampedIndex + mediaWindow.columnCount)
      );
    }

    const rowIndex = Math.floor(clampedIndex / mediaWindow.columnCount);
    const virtualSegmentIndex = resolveMediaVirtualSegmentIndex(rowIndex);
    if (virtualSegmentIndex !== undefined) {
      virtualRows.scrollToIndex(virtualSegmentIndex, { align: "auto" });
    }
    const element = parentRef.current;
    if (!element) {
      return;
    }
    const nextScrollTop = mediaOffset + rowIndex * mediaWindow.rowHeight;
    if (nextScrollTop !== element.scrollTop) {
      scrollPersistenceUnlockedRef.current = true;
      element.scrollTop = nextScrollTop;
    }
  }

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "ArrowRight") {
      event.preventDefault();
      moveSelection("right");
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      moveSelection("left");
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      moveSelection("down");
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      moveSelection("up");
    } else if (event.key === "Home" && (windowedMedia ? mediaItemCount > 0 : layoutItems[0])) {
      event.preventDefault();
      if (windowedMedia) selectWindowedIndex(0);
      else selectIndex(0);
    } else if (event.key === "End" && (windowedMedia ? mediaItemCount > 0 : layoutItems[layoutItems.length - 1])) {
      event.preventDefault();
      if (windowedMedia) selectWindowedIndex(mediaItemCount - 1);
      else selectIndex(layoutItems.length - 1);
    } else if ((event.key === "Enter" || event.key === " ") && selectedMediaId !== null) {
      event.preventDefault();
      onOpenPreview(selectedMediaId);
    }
  }

  return (
    <div
      aria-label={`Media ${layoutMode} view`}
      aria-busy={loading || loadingMore}
      className={`virtual-grid virtual-grid--${layoutMode}${mediaItemCount === 0 ? " media-grid-empty-shell" : ""}`}
      ref={parentRef}
      role="grid"
      style={gridStyle}
      tabIndex={0}
      onKeyDown={handleKeyDown}
    >
      <div className="virtual-grid-spacer" style={{ height: virtualRows.getTotalSize() }}>
        {folderSectionVisible && virtualItems.some((item) => resolveVirtualSegment(item.index)?.type === "folder-header") ? (
          <div
            className="virtual-grid-section-header virtual-grid-folder-header"
            style={{
              height: SECTION_HEADER_HEIGHT,
              left: 0,
              position: "absolute",
              top: 0,
              width: "100%"
            }}
          >
            {folderSection?.header}
          </div>
        ) : null}

        {folderSection && !folderSection.collapsed
          ? renderedFolderIndexes.map((folderIndex) => {
            const placement = resolveFolderWindowPlacement(
              folderIndex,
              folderWindow,
              gridPreferences.folderTileGap
            );
            const folderColumnIndex = placement.itemIndex % folderWindow.columnCount;
            const rowStart = folderColumnIndex === 0;
            const rowEnd =
              folderColumnIndex === folderWindow.columnCount - 1 ||
              placement.itemIndex === (folderSection?.itemCount ?? 0) - 1;
              return (
                <div
                  className={`folder-gridcell${rowStart ? " folder-gridcell--row-start" : ""}${rowEnd ? " folder-gridcell--row-end" : ""}`}
                  key={`folder-${placement.itemIndex}`}
                  role="listitem"
                  style={{
                    height: placement.height,
                    left: placement.left,
                    position: "absolute",
                    top: folderGridOffset + placement.top,
                    width: placement.width
                  }}
                >
                  {folderSection.renderFolder(placement.itemIndex, placement)}
                </div>
              );
            })
          : null}

        {contentHeader && virtualItems.some((item) => resolveVirtualSegment(item.index)?.type === "content-header") ? (
          <div
            className="virtual-grid-section-header virtual-grid-content-header"
            style={{
              boxSizing: "border-box",
              height: folderSectionVisible ? CONTENT_SECTION_HEADER_HEIGHT : SECTION_HEADER_HEIGHT,
              left: 0,
              paddingTop: folderSectionVisible ? CONTENT_HEADER_TOP_GAP : 0,
              position: "absolute",
              top: contentHeaderOffset,
              width: "100%"
            }}
          >
            {contentHeader}
          </div>
        ) : null}

        {mediaItemCount === 0 &&
        emptyContent !== undefined &&
        virtualItems.some((item) => resolveVirtualSegment(item.index)?.type === "empty") ? (
          <div
            className="virtual-grid-empty-row"
            style={{
              height: EMPTY_ROW_HEIGHT,
              left: 0,
              position: "absolute",
              top: mediaOffset,
              width: "100%"
            }}
          >
            {emptyContent}
          </div>
        ) : null}

        {renderedWindowedMediaIndexes.map((mediaIndex) => {
          const item = mediaSlots?.get(mediaIndex);
          const placement = resolveMediaWindowPlacement(
            mediaIndex,
            mediaWindow,
            layoutMode,
            gridPreferences.tileGap,
            gridPreferences.tileLabelHeight
          );
          const selected = item ? item.id === selectedMediaId : false;
          const thumbnailLoadPriority: ThumbnailRequestPriority = selected
            ? "selected"
            : item && visiblePriorityMediaIdSet.has(item.id)
              ? "visible"
              : "ahead";

          return (
            <div
              className={`media-gridcell media-gridcell--${layoutMode}`}
              key={item ? item.id : `media-window-placeholder-${mediaIndex}`}
              role="row"
              style={{
                height: placement.height,
                left: placement.left,
                position: "absolute",
                top: mediaOffset + placement.top,
                width: placement.width
              }}
            >
              <div
                aria-selected={item ? item.id === selectedMediaId : undefined}
                className="media-gridcell-content"
                role="gridcell"
              >
                {item ? (
                  <MediaTile
                    labelGap={tileLabelGap}
                    item={item}
                    layoutMode={layoutMode}
                    onContextMenu={onContextMenu}
                    onOpenPreview={onOpenPreview}
                    onRequestThumbnailStates={onRequestThumbnailStates}
                    onSelect={onSelect}
                    placement={placement}
                    selected={selected}
                    thumbnail={thumbnailStatesByMediaId[item.id]}
                    thumbnailLoadPriority={thumbnailLoadPriority}
                    allowOriginalFallback={thumbnailLoadPriority !== "ahead"}
                  />
                ) : (
                  <MediaTileSkeleton layoutMode={layoutMode} placement={placement} />
                )}
              </div>
            </div>
          );
        })}

        {renderedPlacementIndexes.map((placementIndex) => {
          const placement = layout.placements[placementIndex];
          if (!placement) {
            return null;
          }
          const item = placement.item;
          // Unloaded slot in the full-total justified/masonry layout: hold a
          // placeholder at its estimated-ratio geometry so the row does not
          // reflow and the real thumbnail can fade in once its window loads.
          if (!item) {
            return (
              <div
                className={`media-gridcell media-gridcell--${layoutMode}`}
                key={`placement-placeholder-${placementIndex}`}
                role="row"
                style={{
                  height: placement.height,
                  left: placement.left,
                  position: "absolute",
                  top: mediaOffset + placement.top,
                  width: placement.width
                }}
              >
                <div className="media-gridcell-content" role="gridcell">
                  <MediaTileSkeleton layoutMode={layoutMode} placement={placement} />
                </div>
              </div>
            );
          }
          const selected = item.id === selectedMediaId;
          const thumbnailLoadPriority: ThumbnailRequestPriority = selected
            ? "selected"
            : visiblePriorityMediaIdSet.has(item.id)
              ? "visible"
              : "ahead";

          return (
            <div
              className={`media-gridcell media-gridcell--${layoutMode}`}
              key={item.id}
              role="row"
              style={{
                height: placement.height,
                left: placement.left,
                position: "absolute",
                top: mediaOffset + placement.top,
                width: placement.width
              }}
            >
              <div
                aria-selected={item.id === selectedMediaId}
                className="media-gridcell-content"
                role="gridcell"
              >
                <MediaTile
                  labelGap={tileLabelGap}
                  item={item}
                  layoutMode={layoutMode}
                  onContextMenu={onContextMenu}
                  onOpenPreview={onOpenPreview}
                  onRequestThumbnailStates={onRequestThumbnailStates}
                  onSelect={onSelect}
                  placement={placement}
                  selected={selected}
                  thumbnail={thumbnailStatesByMediaId[item.id]}
                  thumbnailLoadPriority={thumbnailLoadPriority}
                  allowOriginalFallback={thumbnailLoadPriority !== "ahead"}
                />
              </div>
            </div>
          );
        })}

      </div>
      {loading || loadingMore ? <MediaGridRefreshIndicator label="Refreshing media" /> : null}
    </div>
  );
}

function resolveMediaWindowPlacement(
  mediaIndex: number,
  geometry: MediaWindowGeometry,
  layoutMode: LibraryLayoutMode,
  gap: number,
  labelHeight: number
) {
  if (layoutMode === "list") {
    return {
      frameHeight: LIST_WINDOW_FRAME_HEIGHT_PX,
      height: geometry.rowHeight,
      left: 0,
      thumbHeight: LIST_WINDOW_THUMBNAIL_HEIGHT_PX,
      thumbWidth: LIST_WINDOW_THUMBNAIL_WIDTH_PX,
      top: mediaIndex * geometry.rowHeight,
      width: geometry.contentWidth
    };
  }

  const rowIndex = Math.floor(mediaIndex / geometry.columnCount);
  const columnIndex = mediaIndex % geometry.columnCount;
  return {
    frameHeight: geometry.tileWidth + labelHeight,
    height: geometry.rowHeight,
    left: columnIndex * (geometry.tileWidth + gap),
    thumbHeight: geometry.tileWidth,
    thumbWidth: geometry.tileWidth,
    top: rowIndex * geometry.rowHeight,
    width: geometry.tileWidth
  };
}

function resolveFolderWindowPlacement(
  folderIndex: number,
  geometry: FolderWindowGeometry,
  gap: number
): FolderCoverLayoutPlacement {
  const rowIndex = Math.floor(folderIndex / geometry.columnCount);
  const columnIndex = folderIndex % geometry.columnCount;
  return {
    coverHeight: geometry.coverHeight,
    frameHeight: geometry.frameHeight,
    height: geometry.rowHeight,
    itemIndex: folderIndex,
    left: columnIndex * (geometry.tileWidth + gap),
    top: rowIndex * geometry.rowHeight,
    width: geometry.tileWidth
  };
}

function MediaTile({
  allowOriginalFallback,
  labelGap,
  item,
  layoutMode,
  onSelect,
  onOpenPreview,
  onRequestThumbnailStates,
  onContextMenu,
  placement,
  selected,
  thumbnail,
  thumbnailLoadPriority
}: {
  allowOriginalFallback: boolean;
  labelGap: number;
  item: MediaRecord;
  layoutMode: LibraryLayoutMode;
  onSelect: (mediaId: number) => void;
  onOpenPreview: (mediaId: number) => void;
  onRequestThumbnailStates: (mediaIds: number[], priority: ThumbnailRequestPriority) => void;
  onContextMenu?: (event: { item: MediaRecord; x: number; y: number; shiftKey: boolean }) => void;
  placement: { frameHeight: number; thumbHeight: number; thumbWidth: number; width: number };
  selected: boolean;
  thumbnail?: ThumbnailResponse;
  thumbnailLoadPriority: ThumbnailRequestPriority;
}) {
  const listMode = layoutMode === "list";
  const thumbnailState = thumbnail?.state ?? normalizeMediaThumbnailState(item.thumbnailState);
  const prioritizeSelectedMedia = () => {
    onRequestThumbnailStates([item.id], "selected");
    if (isLikelyImageMedia(item)) {
      prefetchOriginalPreview(item, {
        requestPriority: "interactive",
        resourcePriority: "preview"
      });
    }
  };

  return (
    <button
      aria-label={`Select ${item.name}; press Enter or Space, or double-click, to open preview`}
      className={selected ? `media-tile media-tile--${layoutMode} selected` : `media-tile media-tile--${layoutMode}`}
      data-media-id={item.id}
      data-thumb-state={thumbnailState}
      data-interactive-pointer-target-selector=".tile-thumb"
      onClick={() => {
        prioritizeSelectedMedia();
        onSelect(item.id);
      }}
      onDoubleClick={() => {
        prioritizeSelectedMedia();
        onSelect(item.id);
        onOpenPreview(item.id);
      }}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          event.stopPropagation();
          prioritizeSelectedMedia();
          onSelect(item.id);
          onOpenPreview(item.id);
        }
      }}
      onContextMenu={(event) => {
        if (!onContextMenu) return;
        event.preventDefault();
        prioritizeSelectedMedia();
        onSelect(item.id);
        onContextMenu({ item, x: event.clientX, y: event.clientY, shiftKey: event.shiftKey });
      }}
      style={
        listMode
          ? {
              alignItems: "center",
              columnGap: 12,
              display: "grid",
              gridTemplateColumns: `${placement.thumbWidth}px minmax(0, 1fr)`,
              height: placement.frameHeight,
              padding: 8,
              width: "100%"
            }
          : {
              display: "flex",
              flexDirection: "column",
              gap: labelGap,
              height: placement.frameHeight,
              padding: 0,
              width: "100%"
            }
      }
      type="button"
    >
      <ThumbnailStateView
        allowOriginalFallback={allowOriginalFallback}
        item={item}
        layoutMode={layoutMode}
        selected={selected}
        thumbnail={thumbnail}
        thumbnailLoadPriority={thumbnailLoadPriority}
        thumbHeight={placement.thumbHeight}
        thumbWidth={placement.thumbWidth}
      />
      {listMode ? (
        <div className="media-tile-text" style={{ minWidth: 0 }}>
          <div
            className="tile-label tile-label--list"
            style={{
              height: "auto",
              marginTop: 0,
              textAlign: "left",
              whiteSpace: "nowrap"
            }}
            title={item.name}
          >
            {item.name}
          </div>
          <div
            className="tile-meta"
            style={{
              color: "var(--text-muted)",
              fontSize: 12,
              lineHeight: 1.3,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap"
            }}
            title={mediaMetadataSummary(item)}
          >
            {mediaMetadataSummary(item)}
          </div>
        </div>
      ) : (
        <div className="tile-label" title={item.name}>
          {item.name}
        </div>
      )}
    </button>
  );
}

function MediaTileSkeleton({
  layoutMode,
  placement
}: {
  layoutMode: LibraryLayoutMode;
  placement: { frameHeight: number; thumbHeight: number; thumbWidth: number; width: number };
}) {
  return (
    <div
      aria-hidden="true"
      className={`media-tile media-tile--${layoutMode} skeleton`}
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 0,
        height: placement.frameHeight,
        width: placement.width
      }}
    >
      <div
        className="tile-thumb tile-thumb-loading"
        style={{ height: placement.thumbHeight, width: placement.thumbWidth }}
      />
      <span className="tile-label" />
    </div>
  );
}

function resolveTileLabelGap(tileLabelHeight: number) {
  return Math.max(1, Math.round(tileLabelHeight / 6));
}

function compactMediaSlots(mediaSlots: Map<number, MediaRecord>) {
  return Array.from(mediaSlots.entries())
    .sort(([leftIndex], [rightIndex]) => leftIndex - rightIndex)
    .map(([, item]) => item);
}

function firstMissingMediaSlotIndex(
  mediaSlots: Map<number, MediaRecord>,
  totalCount: number
) {
  for (let index = 0; index < totalCount; index += 1) {
    if (!mediaSlots.has(index)) {
      return index;
    }
  }
  return null;
}

function ThumbnailStateView({
  allowOriginalFallback,
  item,
  layoutMode,
  selected,
  thumbnail,
  thumbnailLoadPriority,
  thumbHeight,
  thumbWidth
}: {
  allowOriginalFallback: boolean;
  item: MediaRecord;
  layoutMode: LibraryLayoutMode;
  selected: boolean;
  thumbnail?: ThumbnailResponse;
  thumbnailLoadPriority: ThumbnailRequestPriority;
  thumbHeight: number;
  thumbWidth: number;
}) {
  const rowState = normalizeMediaThumbnailState(item.thumbnailState);
  const state = thumbnail?.state ?? rowState;
  const previewPlaceholderUrl = previewPlaceholderDataUrl(item);
  const hasLiveReadyThumbnail = thumbnail?.state === "ready" && thumbnail.updatedAt !== null;
  // If the decoded thumbnail is ALREADY in the object-URL cache, the thumbnail
  // is definitively ready and instantly displayable — render ReadyThumbnail even
  // when neither the polled state nor the media record reports "ready". This is
  // the fix for "already-loaded tiles reload on scroll-back": /api/media omits
  // per-row thumbnail state and the polled state is not retained after a tile
  // unmounts, so a remounted tile used to fall through to the loading
  // placeholder and re-request — showing a reload — even though its blob was
  // cached. Trusting the cache lets it render the cached image immediately.
  const hasCachedThumbnailBlob =
    readCachedThumbnailObjectUrl(
      thumbnailObjectUrlCacheKey(item.id, mediaFileContentSignature(item), null)
    ) !== null;
  const canLoadCurrentThumbnailBlob =
    hasLiveReadyThumbnail || rowState === "ready" || hasCachedThumbnailBlob;
  const presentationStyle = thumbnailPresentationStyle(layoutMode, thumbWidth, thumbHeight);
  const resourcePriority = selected
    ? "preview"
    : thumbnailLoadPriority === "visible"
      ? "visible"
      : "ahead";
  const originalFallbackResourcePriority =
    selected ? "preview" : thumbnailLoadPriority === "visible" ? "visible" : "fallback";
  const requestPriority = selected ? "interactive" : "resource";

  if (canLoadCurrentThumbnailBlob) {
    return (
      <ReadyThumbnail
        alt={item.name}
        className={`tile-thumb tile-thumb--${layoutMode}`}
        fileId={item.id}
        contentSignature={mediaFileContentSignature(item)}
        previewPlaceholderUrl={previewPlaceholderUrl}
        requestPriority={requestPriority}
        resourcePriority={resourcePriority}
        style={presentationStyle}
        thumbnailUpdatedAt={hasLiveReadyThumbnail ? thumbnail.updatedAt : null}
      />
    );
  }

  if (state === "failed") {
    return (
      <div className={`tile-thumb tile-thumb-failed tile-thumb--${layoutMode}`} style={presentationStyle}>
        <span>failed</span>
        <ThumbnailInteractionRing />
      </div>
    );
  }

  if (state === "skipped_small") {
    if (previewPlaceholderUrl) {
      return (
        <PlaceholderThumbnail
          alt={item.name}
          className={`tile-thumb tile-thumb-placeholder tile-thumb--${layoutMode}`}
          src={previewPlaceholderUrl}
          style={presentationStyle}
        />
      );
    }
    return (
      <div className={`tile-thumb tile-thumb-skipped tile-thumb--${layoutMode}`} style={presentationStyle}>
        <span>{item.kind ?? "file"}</span>
        <ThumbnailInteractionRing />
      </div>
    );
  }

  if (state === "queued") {
    if (allowOriginalFallback && isLikelyImageMedia(item)) {
      return (
        <OriginalFallbackThumbnail
          alt={item.name}
          className={`tile-thumb tile-thumb--${layoutMode}`}
          fallbackUrl={previewPlaceholderUrl}
          item={item}
          requestPriority={requestPriority}
          resourcePriority={originalFallbackResourcePriority}
          style={presentationStyle}
        />
      );
    }
    if (previewPlaceholderUrl) {
      return (
        <PlaceholderThumbnail
          alt={item.name}
          className={`tile-thumb tile-thumb-placeholder tile-thumb--${layoutMode}`}
          src={previewPlaceholderUrl}
          style={presentationStyle}
        />
      );
    }
    return (
      <div className={`tile-thumb tile-thumb-loading tile-thumb--${layoutMode}`} style={presentationStyle}>
        <ThumbnailInteractionRing />
      </div>
    );
  }

  if (allowOriginalFallback && isLikelyImageMedia(item)) {
    return (
      <OriginalFallbackThumbnail
        alt={item.name}
        className={`tile-thumb tile-thumb--${layoutMode}`}
        fallbackUrl={previewPlaceholderUrl}
        item={item}
        requestPriority={requestPriority}
        resourcePriority={originalFallbackResourcePriority}
        style={presentationStyle}
      />
    );
  }

  if (previewPlaceholderUrl) {
    return (
      <PlaceholderThumbnail
        alt={item.name}
        className={`tile-thumb tile-thumb-placeholder tile-thumb--${layoutMode}`}
        src={previewPlaceholderUrl}
        style={presentationStyle}
      />
    );
  }

  return (
    <div className={`tile-thumb tile-thumb-loading tile-thumb--${layoutMode}`} style={presentationStyle}>
      <ThumbnailInteractionRing />
    </div>
  );
}

function ThumbnailInteractionRing() {
  return <span aria-hidden="true" className="library-thumbnail-interaction-ring" />;
}

function isLikelyImageMedia(item: MediaRecord): boolean {
  if (item.kind?.toLowerCase() === "image") {
    return true;
  }
  const normalizedExt = (item.ext ?? "").trim().toLowerCase().replace(/^\./, "");
  return ORIGINAL_FALLBACK_IMAGE_EXTENSIONS.has(normalizedExt);
}

function thumbnailPresentationStyle(
  layoutMode: LibraryLayoutMode,
  thumbWidth: number,
  thumbHeight: number
): CSSProperties {
  if (layoutMode === "list") {
    return {
      height: thumbHeight,
      width: thumbWidth
    };
  }

  return {
    aspectRatio: `${Math.max(1, Math.round(thumbWidth))} / ${Math.max(1, Math.round(thumbHeight))}`,
    width: "100%"
  };
}

function normalizeMediaThumbnailState(value: string | null | undefined): ThumbnailResponse["state"] {
  if (
    value === "pending" ||
    value === "queued" ||
    value === "ready" ||
    value === "failed" ||
    value === "skipped_small"
  ) {
    return value;
  }
  return "pending";
}

function shouldRefreshThumbnailState(
  item: MediaRecord,
  thumbnail: ThumbnailResponse | undefined
): boolean {
  const rowState = normalizeMediaThumbnailState(item.thumbnailState);
  const hasLiveThumbnailMetadata = thumbnail?.state === "ready" && thumbnail.updatedAt !== null;
  if (!hasLiveThumbnailMetadata && rowState === "ready") {
    return true;
  }
  const state = thumbnail?.state ?? rowState;
  return state === "pending" || state === "queued";
}

function MediaGridRefreshIndicator({ label }: { label: string }) {
  return (
    <div aria-label={label} className="media-grid-refresh-indicator" role="status">
      <span className="central-preview-loading-spinner" aria-hidden="true" />
    </div>
  );
}

function mediaMetadataSummary(item: MediaRecord) {
  const parts: string[] = [];
  if (item.kind) {
    parts.push(item.kind);
  } else if (item.ext) {
    parts.push(item.ext.toUpperCase());
  }
  if (item.width && item.height) {
    parts.push(`${item.width}×${item.height}`);
  }
  if (item.size > 0) {
    parts.push(formatByteSize(item.size));
  }
  return parts.join(" • ");
}

function formatByteSize(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }

  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = bytes;
  let unitIndex = 0;
  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }
  return `${size.toFixed(unitIndex === 0 ? 0 : size >= 10 ? 1 : 2)} ${units[unitIndex]}`;
}

function PlaceholderThumbnail({
  alt,
  className,
  src,
  style
}: {
  alt: string;
  className: string;
  src: string;
  style: CSSProperties;
}) {
  return (
    <div className={className} data-preview-placeholder="grid" style={style}>
      <img alt={alt} className="tile-thumb-image" decoding="async" loading="eager" src={src} />
      <ThumbnailInteractionRing />
    </div>
  );
}

function rememberOriginalFallbackObjectUrl(cacheKey: string, objectUrl: string) {
  const existing = originalFallbackObjectUrlCache.get(cacheKey);
  if (existing && existing !== objectUrl) {
    URL.revokeObjectURL(existing);
  }
  originalFallbackObjectUrlCache.delete(cacheKey);
  originalFallbackObjectUrlCache.set(cacheKey, objectUrl);
  while (originalFallbackObjectUrlCache.size > ORIGINAL_FALLBACK_OBJECT_URL_CACHE_LIMIT) {
    const firstKey = originalFallbackObjectUrlCache.keys().next().value;
    if (!firstKey) break;
    const staleObjectUrl = originalFallbackObjectUrlCache.get(firstKey);
    originalFallbackObjectUrlCache.delete(firstKey);
    if (staleObjectUrl) URL.revokeObjectURL(staleObjectUrl);
  }
}

function OriginalFallbackThumbnail({
  alt,
  className,
  fallbackUrl,
  item,
  requestPriority,
  resourcePriority,
  style
}: {
  alt: string;
  className: string;
  fallbackUrl: string | null;
  item: MediaRecord;
  requestPriority: "interactive" | "resource";
  resourcePriority: "preview" | "visible" | "fallback";
  style: CSSProperties;
}) {
  const cacheKey = `original:${mediaFileContentSignature(item)}`;
  const [src, setSrc] = useState<string | null>(
    () => originalFallbackObjectUrlCache.get(cacheKey) ?? null
  );
  const [error, setError] = useState(false);
  const [retryToken, setRetryToken] = useState(0);
  const itemRef = useRef(item);
  const objectUrlRef = useRef<string | null>(src);
  itemRef.current = item;
  // Fade in only when the image was NOT already available at mount. During a scan
  // most visible tiles render through this original-decode fallback (the WebP
  // thumbnail is still pending), so without this the "pale -> image" appearance
  // never fades. A tile scrolling back in with its object URL still cached renders
  // instantly (no re-flash); the later fallback -> ready WebP swap does not re-fade
  // because ReadyThumbnail seeds its src from this same cached original URL.
  const shouldFadeInRef = useRef(src === null);

  useEffect(() => {
    const cachedObjectUrl = originalFallbackObjectUrlCache.get(cacheKey);
    if (cachedObjectUrl) {
      objectUrlRef.current = cachedObjectUrl;
      setSrc(cachedObjectUrl);
      setError(false);
      return undefined;
    }

    let revoked = false;
    let objectUrl: string | null = null;
    let retryTimer: number | null = null;
    const controller = new AbortController();
    setError(false);

    // The cache key captures media identity/content. Do not restart this request
    // when thumbnail polling replaces the row object with the same file state.
    requestOriginalPreviewBlob(itemRef.current, {
      requestPriority,
      resourcePriority,
      signal: controller.signal
    })
      .then((blob) => {
        objectUrl = URL.createObjectURL(blob);
        const nextObjectUrl = objectUrl;
        if (revoked) {
          URL.revokeObjectURL(nextObjectUrl);
          return;
        }

        const commitObjectUrl = () => {
          if (revoked) {
            return;
          }
          objectUrlRef.current = nextObjectUrl;
          rememberOriginalFallbackObjectUrl(cacheKey, nextObjectUrl);
          setSrc(nextObjectUrl);
        };

        if (!objectUrlRef.current) {
          commitObjectUrl();
          void preloadImageObjectUrl(nextObjectUrl).catch(() => {
            if (revoked || objectUrlRef.current !== nextObjectUrl) {
              return;
            }
            originalFallbackObjectUrlCache.delete(cacheKey);
            setError(true);
          });
          return;
        }

        void preloadImageObjectUrl(nextObjectUrl)
          .then(() => {
            commitObjectUrl();
          })
          .catch(() => {
            if (revoked) {
              return;
            }
            if (objectUrl && objectUrlRef.current !== objectUrl) {
              URL.revokeObjectURL(objectUrl);
              objectUrl = null;
            }
            setError(true);
          });
      })
      .catch((cause) => {
        if (objectUrl && objectUrlRef.current !== objectUrl) {
          URL.revokeObjectURL(objectUrl);
          objectUrl = null;
        }
        if (revoked) {
          return;
        }
        if (cause instanceof Error && cause.name === "AbortError") {
          retryTimer = window.setTimeout(() => {
            if (!revoked) setRetryToken((current) => current + 1);
          }, 80);
          return;
        }
        setError(true);
      });

    return () => {
      revoked = true;
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      controller.abort();
      if (objectUrl && objectUrlRef.current !== objectUrl) {
        URL.revokeObjectURL(objectUrl);
      }
    };
  }, [cacheKey, requestPriority, resourcePriority, retryToken]);

  if (error && !src) {
    if (fallbackUrl) {
      return <PlaceholderThumbnail alt={alt} className={className} src={fallbackUrl} style={style} />;
    }
    return (
      <div className={`${className} tile-thumb-failed`} style={style}>
        <span>load error</span>
        <ThumbnailInteractionRing />
      </div>
    );
  }

  if (!src) {
    if (fallbackUrl) {
      return <PlaceholderThumbnail alt={alt} className={className} src={fallbackUrl} style={style} />;
    }
    return (
      <div className={`${className} tile-thumb-loading`} style={style}>
        <ThumbnailInteractionRing />
      </div>
    );
  }

  return (
    <div className={`${className} tile-thumb-original-fallback`} style={style}>
      <img
        alt={alt}
        className={
          shouldFadeInRef.current ? "tile-thumb-image tile-thumb-image--fade" : "tile-thumb-image"
        }
        decoding="async"
        loading="eager"
        src={src}
      />
      <ThumbnailInteractionRing />
    </div>
  );
}

function ReadyThumbnail({
  alt,
  className,
  fileId,
  contentSignature,
  previewPlaceholderUrl,
  requestPriority,
  resourcePriority,
  style,
  thumbnailUpdatedAt
}: {
  alt: string;
  className: string;
  fileId: number;
  contentSignature: string;
  previewPlaceholderUrl: string | null;
  requestPriority: "interactive" | "resource";
  resourcePriority: "preview" | "visible" | "ahead";
  style: CSSProperties;
  thumbnailUpdatedAt: number | null;
}) {
  // Key the durable object-URL cache by stable file-content identity
  // (id:mtime:size), NOT by live thumbnail state/updatedAt. The key then stays
  // constant across row-state -> live-state transitions and across scroll, so a
  // tile scrolled out of and back into view reuses its cached image instead of
  // re-fetching. A real content change (mtime/size) naturally busts it.
  const cacheKey = thumbnailObjectUrlCacheKey(fileId, contentSignature, null);
  // Bridge the fallback -> ready swap. While a thumbnail is still generating the
  // tile shows OriginalFallbackThumbnail (the decoded original). When state
  // flips to "ready" this ReadyThumbnail mounts; with no cached WebP its src
  // would start null and the tile would blank to a placeholder/background until
  // the WebP fetch+decode finishes — the "flash background then re-show" seen
  // while scrolling a cold-start aggregated view. Seed the initial src from the
  // original-fallback object URL the tile was already displaying so the decoded
  // WebP is swapped in only once it is ready (via the decode-before-commit path
  // below). The bridge URL is owned by originalFallbackObjectUrlCache and is
  // never assigned to `objectUrl`, so this component never revokes it.
  const originalBridgeUrl = originalFallbackObjectUrlCache.get(`original:${contentSignature}`) ?? null;
  const [src, setSrc] = useState<string | null>(
    () => readCachedThumbnailObjectUrl(cacheKey) ?? originalBridgeUrl
  );
  const [error, setError] = useState(false);
  const [retryToken, setRetryToken] = useState(0);
  const objectUrlRef = useRef<string | null>(src);
  const cacheOwnsObjectUrlRef = useRef(src !== null);
  // Fade in only when the image was NOT already available at mount. A tile that
  // scrolls back into view with its object URL still cached renders instantly
  // (no fade, no placeholder re-flash); a freshly fetched thumbnail fades in.
  const shouldFadeInRef = useRef(src === null);

  useEffect(() => {
    const cachedObjectUrl = readCachedThumbnailObjectUrl(cacheKey);
    if (cachedObjectUrl) {
      objectUrlRef.current = cachedObjectUrl;
      cacheOwnsObjectUrlRef.current = true;
      setSrc(cachedObjectUrl);
      setError(false);
      return undefined;
    }

    let revoked = false;
    let objectUrl: string | null = null;
    let retryTimer: number | null = null;
    const controller = new AbortController();
    setError(false);

    requestThumbnailBlob(fileId, thumbnailUpdatedAt, {
      requestPriority,
      resourcePriority
      // Deliberately NO abort signal: if the tile scrolls out of view mid-fetch
      // we let the (small) thumbnail fetch COMPLETE and cache it (the `revoked`
      // branch below), instead of aborting and re-fetching on scroll-back. That
      // abort+re-fetch was the "flashes once or twice then settles" churn — a
      // tile scrolled past before its blob arrived discarded the in-flight fetch,
      // so returning to it re-fetched. The scheduler (12 slots) + the client's
      // request timeout bound the in-flight set; the `revoked` flag still guards
      // against setState after unmount.
    })
      .then((blob) => {
        objectUrl = URL.createObjectURL(blob);
        const nextObjectUrl = objectUrl;
        if (revoked) {
          // The tile unmounted while the fetch was in flight (scrolled past
          // fast). DON'T throw the blob away — cache it so the next mount of
          // this tile hits instead of re-fetching. Re-fetch-on-unmount was the
          // "flashes once or twice then settles" churn: a tile scrolled past
          // mid-fetch discarded its result, so scrolling back re-fetched. The
          // cache owns the URL's lifetime (LRU + count floor); we only revoke
          // our redundant URL if the cache already holds one for this content.
          const owns = rememberThumbnailObjectUrl(cacheKey, nextObjectUrl, blob.size);
          if (!owns) {
            URL.revokeObjectURL(nextObjectUrl);
          }
          objectUrl = null; // cache (or revoke) owns it; cleanup must not touch it
          return;
        }

        const commitObjectUrl = () => {
          if (revoked) {
            return;
          }
          const owns = rememberThumbnailObjectUrl(cacheKey, nextObjectUrl, blob.size);
          if (owns) {
            // The cache adopted OUR url — display and own it.
            objectUrlRef.current = nextObjectUrl;
            cacheOwnsObjectUrlRef.current = true;
            setSrc(nextObjectUrl);
            return;
          }
          // The cache already holds a URL for this content (a racing tile or the
          // ahead-prefetch cached it first). Adopt the DURABLE cached url, revoke
          // our now-redundant one immediately, and display the cached one. This
          // stops the fast-scroll churn where a tile displayed its own url,
          // revoked it on unmount, and re-fetched on scroll-back: now the tile
          // always ends up pointing at the single cached url that outlives it.
          const cached = readCachedThumbnailObjectUrl(cacheKey);
          if (cached && cached !== nextObjectUrl) {
            URL.revokeObjectURL(nextObjectUrl);
            objectUrl = null;
            objectUrlRef.current = cached;
            cacheOwnsObjectUrlRef.current = true; // cache owns it; we never revoke it
            setSrc(cached);
            return;
          }
          // Fallback (cache empty despite reject — shouldn't happen): keep ours.
          objectUrlRef.current = nextObjectUrl;
          cacheOwnsObjectUrlRef.current = true;
          setSrc(nextObjectUrl);
        };

        if (!objectUrlRef.current) {
          commitObjectUrl();
          void preloadImageObjectUrl(nextObjectUrl).catch(() => {
            if (revoked || objectUrlRef.current !== nextObjectUrl) {
              return;
            }
            setError(true);
          });
          return;
        }

        void preloadImageObjectUrl(nextObjectUrl)
          .then(() => {
            commitObjectUrl();
          })
          .catch(() => {
            if (revoked) {
              return;
            }
            if (objectUrl && objectUrlRef.current !== objectUrl) {
              URL.revokeObjectURL(objectUrl);
              objectUrl = null;
            }
            setError(true);
          });
      })
      .catch((cause) => {
        if (objectUrl && objectUrlRef.current !== objectUrl) {
          URL.revokeObjectURL(objectUrl);
          objectUrl = null;
        }
        if (revoked) {
          return;
        }
        if (cause instanceof Error && cause.name === "AbortError") {
          retryTimer = window.setTimeout(() => {
            if (!revoked) setRetryToken((current) => current + 1);
          }, 80);
          return;
        }
        setError(true);
      });

    return () => {
      revoked = true;
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      controller.abort();
      if (objectUrl && objectUrlRef.current !== objectUrl) {
        URL.revokeObjectURL(objectUrl);
      }
      if (objectUrl && objectUrlRef.current === objectUrl && !cacheOwnsObjectUrlRef.current) {
        URL.revokeObjectURL(objectUrl);
        objectUrlRef.current = null;
      }
    };
    // Deliberately NOT keyed on `thumbnailUpdatedAt` or `resourcePriority`: the
    // blob is identified by content identity (`cacheKey` = id:mtime:size), so a
    // live thumbnail-state update (updatedAt null -> timestamp) or a viewport
    // priority shift (visible <-> ahead <-> selected) must NOT re-fetch and
    // re-swap the object URL — that made a loaded tile flash its background and
    // reload 1-2x as its state settled during a scan. A genuine content change
    // bumps mtime/size -> cacheKey, which re-runs this effect correctly.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cacheKey, fileId, retryToken]);

  if (error && !src) {
    return (
      <div className={`${className} tile-thumb-failed`} style={style}>
        <span>load error</span>
        <ThumbnailInteractionRing />
      </div>
    );
  }

  if (!src) {
    if (previewPlaceholderUrl) {
      return (
        <PlaceholderThumbnail alt={alt} className={className} src={previewPlaceholderUrl} style={style} />
      );
    }
    return (
      <div className={`${className} tile-thumb-loading`} style={style}>
        <ThumbnailInteractionRing />
      </div>
    );
  }

  return (
    <div className={`${className} tile-thumb-ready`} style={style}>
      <img
        alt={alt}
        className={
          shouldFadeInRef.current ? "tile-thumb-image tile-thumb-image--fade" : "tile-thumb-image"
        }
        decoding="async"
        loading="eager"
        src={src}
      />
      <ThumbnailInteractionRing />
    </div>
  );
}
