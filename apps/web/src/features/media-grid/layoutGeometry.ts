import type { MediaRecord } from "@megle/core-client";
import { mediaContentSignature } from "../../core/mediaContentSignature.ts";
import type { LibraryLayoutMode } from "./layoutMode.ts";

const LIST_ROW_HEIGHT_PX = 104;
const LIST_FRAME_HEIGHT_PX = 90;
const LIST_THUMBNAIL_WIDTH_PX = 108;
const LIST_THUMBNAIL_HEIGHT_PX = 72;
const ADAPTIVE_TARGET_ROW_HEIGHT_SCALE = 1;
const ADAPTIVE_MIN_ROW_HEIGHT_PX = 112;
const ADAPTIVE_LAST_ROW_MAX_SCALE = 1.16;
const ADAPTIVE_ROW_FILL_THRESHOLD = 0.88;
const WATERFALL_BAND_SCALE = 0.9;
const WATERFALL_MIN_BAND_PX = 120;

export interface LayoutPlacement {
  // undefined when this slot's media has not been loaded yet: adaptive/waterfall
  // lay out over the full folder total using a stable estimated aspect ratio for
  // unloaded slots, so the scroll height is correct immediately and unloaded
  // positions render a placeholder that fades into the real thumbnail on load.
  item: MediaRecord | undefined;
  itemIndex: number;
  left: number;
  top: number;
  width: number;
  height: number;
  frameHeight: number;
  thumbWidth: number;
  thumbHeight: number;
}

export interface LayoutSegment {
  index: number;
  start: number;
  size: number;
  itemIndexes: number[];
}

export interface LayoutGeometry {
  estimatedSegmentSize: number;
  placements: LayoutPlacement[];
  segments: LayoutSegment[];
  totalSize: number;
}

export interface FolderCoverLayoutPlacement {
  itemIndex: number;
  left: number;
  top: number;
  width: number;
  height: number;
  frameHeight: number;
  coverHeight: number;
}

export interface FolderCoverLayoutGeometry {
  estimatedSegmentSize: number;
  placements: FolderCoverLayoutPlacement[];
  segments: LayoutSegment[];
  totalSize: number;
}

export interface ScopedMedia {
  ids: number[];
  key: string;
  signatureKey: string;
}

interface LayoutBuildOptions {
  gap: number;
  items: MediaRecord[];
  labelHeight: number;
  layoutMode: LibraryLayoutMode;
  viewportWidth: number;
  tileMinWidth: number;
  // When provided, adaptive/waterfall lay out over the full folder total instead
  // of only the loaded `items`: slot `index` uses `getItemAt(index)` when loaded
  // (real aspect ratio) and a stable estimated ratio otherwise. This gives the
  // correct scroll height + placeholder slots up front and avoids the
  // reorder-on-load reflow of laying out only the loaded prefix.
  totalCount?: number;
  getItemAt?: (index: number) => MediaRecord | undefined;
  getItemAspectRatio?: (index: number, item: MediaRecord | undefined) => number;
  estimatedRatio?: number;
}

export function buildLayoutGeometry(options: LayoutBuildOptions): LayoutGeometry {
  switch (options.layoutMode) {
    case "list":
      return buildListLayout(options);
    case "adaptive":
      return buildAdaptiveLayout(options);
    case "waterfall":
      return buildWaterfallLayout(options);
    case "grid":
    default:
      return buildGridLayout(options);
  }
}

export function buildFolderCoverLayout({
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
}): FolderCoverLayoutGeometry {
  const contentWidth = normalizeViewportWidth(viewportWidth, tileMinWidth);
  const columnCount = resolveColumnCount(contentWidth, tileMinWidth, gap);
  const tileWidth = resolveColumnWidth(contentWidth, columnCount, gap);
  const coverHeight = Math.round(tileWidth * 4 / 3);
  const rowHeight = coverHeight + labelHeight + gap;
  const placements: FolderCoverLayoutPlacement[] = [];
  const segments: LayoutSegment[] = [];

  for (let index = 0; index < count; index += 1) {
    const rowIndex = Math.floor(index / columnCount);
    const columnIndex = index % columnCount;
    const top = rowIndex * rowHeight;
    const left = columnIndex * (tileWidth + gap);
    placements.push({
      itemIndex: index,
      left,
      top,
      width: tileWidth,
      height: rowHeight,
      frameHeight: coverHeight + labelHeight,
      coverHeight
    });
  }

  const rowCount = Math.ceil(count / columnCount);
  for (let rowIndex = 0; rowIndex < rowCount; rowIndex += 1) {
    const startIndex = rowIndex * columnCount;
    const endIndex = Math.min(count, startIndex + columnCount);
    segments.push({
      index: rowIndex,
      start: rowIndex * rowHeight,
      size: rowHeight,
      itemIndexes: createSequentialIndexes(startIndex, endIndex)
    });
  }

  return {
    estimatedSegmentSize: rowHeight,
    placements,
    segments,
    totalSize: rowCount * rowHeight
  };
}

export function collectScopedMediaInViewport(
  geometry: LayoutGeometry,
  rangeStart: number,
  rangeEnd: number,
  options: {
    excludeMediaId?: number | null;
  } = {}
): ScopedMedia {
  if (rangeEnd <= rangeStart || geometry.placements.length === 0 || geometry.segments.length === 0) {
    return emptyScopedMedia();
  }

  const segmentRange = findSegmentRange(geometry.segments, rangeStart, rangeEnd);
  if (!segmentRange) {
    return emptyScopedMedia();
  }

  const candidateIndexes = collectPlacementIndexesFromSegmentRange(
    geometry,
    segmentRange.startIndex,
    segmentRange.endIndex
  );
  if (candidateIndexes.length === 0) {
    return emptyScopedMedia();
  }

  const ids: number[] = [];
  const signatures: string[] = [];
  const seenMediaIds = new Set<number>();
  for (const placementIndex of candidateIndexes) {
    const placement = geometry.placements[placementIndex];
    if (!placement) {
      continue;
    }
    const item = placement.item;
    // Skip placeholder slots (unloaded media in the full-total justified layout):
    // scoped media is used for thumbnail-state requests, which only apply to
    // loaded records.
    if (!item) {
      continue;
    }
    if (options.excludeMediaId === item.id) {
      continue;
    }
    if (!intersectsRange(placement.top, placement.height, rangeStart, rangeEnd)) {
      continue;
    }
    if (seenMediaIds.has(item.id)) {
      continue;
    }
    seenMediaIds.add(item.id);
    ids.push(item.id);
    signatures.push(mediaContentSignature(item));
  }

  return {
    ids,
    key: ids.join(":"),
    signatureKey: signatures.join("|")
  };
}

export function collectPlacementIndexesFromSegmentRange(
  geometry: { segments: LayoutSegment[] },
  startIndex: number,
  endIndex: number
): number[] {
  if (geometry.segments.length === 0 || startIndex > endIndex) {
    return [];
  }

  const clampedStart = clamp(startIndex, 0, geometry.segments.length - 1);
  const clampedEnd = clamp(endIndex, clampedStart, geometry.segments.length - 1);
  const indexes = new Set<number>();
  for (let segmentIndex = clampedStart; segmentIndex <= clampedEnd; segmentIndex += 1) {
    for (const placementIndex of geometry.segments[segmentIndex]?.itemIndexes ?? []) {
      indexes.add(placementIndex);
    }
  }
  return [...indexes].sort((leftIndex, rightIndex) => leftIndex - rightIndex);
}

export function findDirectionalNeighborIndex(
  placements: LayoutPlacement[],
  currentIndex: number,
  direction: "left" | "right" | "up" | "down"
): number {
  const current = placements[currentIndex];
  if (!current) {
    return currentIndex;
  }

  const currentCenterX = current.left + current.width / 2;
  const currentCenterY = current.top + current.frameHeight / 2;
  let winnerIndex = currentIndex;
  let bestScore = Number.POSITIVE_INFINITY;

  for (let placementIndex = 0; placementIndex < placements.length; placementIndex += 1) {
    if (placementIndex === currentIndex) {
      continue;
    }
    const candidate = placements[placementIndex];
    if (!candidate) {
      continue;
    }

    const candidateCenterX = candidate.left + candidate.width / 2;
    const candidateCenterY = candidate.top + candidate.frameHeight / 2;
    const deltaX = candidateCenterX - currentCenterX;
    const deltaY = candidateCenterY - currentCenterY;

    if (direction === "left" && deltaX >= -1) {
      continue;
    }
    if (direction === "right" && deltaX <= 1) {
      continue;
    }
    if (direction === "up" && deltaY >= -1) {
      continue;
    }
    if (direction === "down" && deltaY <= 1) {
      continue;
    }

    const primaryDistance =
      direction === "left" || direction === "right" ? Math.abs(deltaX) : Math.abs(deltaY);
    const secondaryDistance =
      direction === "left" || direction === "right" ? Math.abs(deltaY) : Math.abs(deltaX);
    const overlapBonus =
      direction === "left" || direction === "right"
        ? axisOverlap(current.top, current.top + current.frameHeight, candidate.top, candidate.top + candidate.frameHeight)
        : axisOverlap(current.left, current.left + current.width, candidate.left, candidate.left + candidate.width);
    const score = primaryDistance * 1000 + secondaryDistance - overlapBonus;

    if (score < bestScore) {
      bestScore = score;
      winnerIndex = placementIndex;
    }
  }

  return winnerIndex;
}

export function resolveScrollTopForPlacement(
  placement: Pick<LayoutPlacement, "frameHeight" | "top">,
  viewportHeight: number,
  currentScrollTop: number,
  contentHeight: number
): number {
  if (viewportHeight <= 0) {
    return currentScrollTop;
  }

  const placementTop = placement.top;
  const placementBottom = placement.top + placement.frameHeight;
  const viewportBottom = currentScrollTop + viewportHeight;
  if (placementTop >= currentScrollTop && placementBottom <= viewportBottom) {
    return currentScrollTop;
  }

  if (placementTop < currentScrollTop) {
    return clamp(placementTop, 0, Math.max(0, contentHeight - viewportHeight));
  }

  return clamp(placementBottom - viewportHeight, 0, Math.max(0, contentHeight - viewportHeight));
}

function buildGridLayout({
  gap,
  items,
  labelHeight,
  tileMinWidth,
  viewportWidth
}: Omit<LayoutBuildOptions, "layoutMode">): LayoutGeometry {
  const contentWidth = normalizeViewportWidth(viewportWidth, tileMinWidth);
  const columnCount = resolveColumnCount(contentWidth, tileMinWidth, gap);
  const tileWidth = resolveColumnWidth(contentWidth, columnCount, gap);
  const rowHeight = tileWidth + labelHeight + gap;
  const placements: LayoutPlacement[] = [];
  const segments: LayoutSegment[] = [];

  for (let index = 0; index < items.length; index += 1) {
    const rowIndex = Math.floor(index / columnCount);
    const columnIndex = index % columnCount;
    const top = rowIndex * rowHeight;
    const left = columnIndex * (tileWidth + gap);
    placements.push({
      item: items[index],
      itemIndex: index,
      left,
      top,
      width: tileWidth,
      height: rowHeight,
      frameHeight: tileWidth + labelHeight,
      thumbWidth: tileWidth,
      thumbHeight: tileWidth
    });
  }

  const rowCount = Math.ceil(items.length / columnCount);
  for (let rowIndex = 0; rowIndex < rowCount; rowIndex += 1) {
    const startIndex = rowIndex * columnCount;
    const endIndex = Math.min(items.length, startIndex + columnCount);
    segments.push({
      index: rowIndex,
      start: rowIndex * rowHeight,
      size: rowHeight,
      itemIndexes: createSequentialIndexes(startIndex, endIndex)
    });
  }

  return {
    estimatedSegmentSize: rowHeight,
    placements,
    segments,
    totalSize: rowCount * rowHeight
  };
}

function buildListLayout({
  gap,
  items,
  tileMinWidth,
  viewportWidth
}: Omit<LayoutBuildOptions, "labelHeight" | "layoutMode"> & { labelHeight: number }): LayoutGeometry {
  const contentWidth = normalizeViewportWidth(viewportWidth, tileMinWidth);
  const rowHeight = LIST_ROW_HEIGHT_PX;
  const frameHeight = LIST_FRAME_HEIGHT_PX;
  const placements: LayoutPlacement[] = items.map((item, index) => ({
    item,
    itemIndex: index,
    left: 0,
    top: index * rowHeight,
    width: contentWidth,
    height: rowHeight,
    frameHeight,
    thumbWidth: LIST_THUMBNAIL_WIDTH_PX,
    thumbHeight: LIST_THUMBNAIL_HEIGHT_PX
  }));
  const segments: LayoutSegment[] = placements.map((placement, index) => ({
    index,
    start: placement.top,
    size: rowHeight,
    itemIndexes: [index]
  }));

  return {
    estimatedSegmentSize: rowHeight,
    placements,
    segments,
    totalSize: placements.length * rowHeight
  };
}

function buildAdaptiveLayout({
  gap,
  items,
  labelHeight,
  tileMinWidth,
  viewportWidth,
  totalCount,
  getItemAt,
  getItemAspectRatio,
  estimatedRatio
}: Omit<LayoutBuildOptions, "layoutMode">): LayoutGeometry {
  const contentWidth = normalizeViewportWidth(viewportWidth, tileMinWidth);
  const targetRowHeight = Math.max(ADAPTIVE_MIN_ROW_HEIGHT_PX, tileMinWidth * ADAPTIVE_TARGET_ROW_HEIGHT_SCALE);
  const placements: LayoutPlacement[] = [];
  const segments: LayoutSegment[] = [];
  let rowItems: Array<{ item: MediaRecord | undefined; itemIndex: number; ratio: number }> = [];
  let ratioSum = 0;
  let top = 0;
  let rowIndex = 0;

  // Lay out over the full folder total when a getter is supplied; otherwise fall
  // back to the loaded items (used by callers that do not window).
  const count = totalCount ?? items.length;
  const itemAt = getItemAt ?? ((index: number) => items[index]);
  const fallbackRatio = estimatedRatio && estimatedRatio > 0 ? estimatedRatio : 1;

  const flushRow = (justify: boolean) => {
    if (rowItems.length === 0) {
      return;
    }

    const widthWithoutGaps = Math.max(1, contentWidth - gap * (rowItems.length - 1));
    const rawHeight = widthWithoutGaps / Math.max(ratioSum, 0.01);
    const maxLastRowHeight = targetRowHeight * ADAPTIVE_LAST_ROW_MAX_SCALE;
    const rowThumbHeight = justify
      ? Math.max(ADAPTIVE_MIN_ROW_HEIGHT_PX, rawHeight)
      : Math.min(rawHeight, maxLastRowHeight);
    const frameHeight = rowThumbHeight + labelHeight;
    const rowHeight = frameHeight + gap;
    let left = 0;
    const itemIndexes: number[] = [];

    rowItems.forEach((entry, entryIndex) => {
      const remainingWidth = contentWidth - left - gap * Math.max(0, rowItems.length - entryIndex - 1);
      const naturalWidth = Math.max(1, Math.round(rowThumbHeight * entry.ratio));
      const itemWidth = Math.max(1, Math.min(remainingWidth, naturalWidth));
      placements.push({
        item: entry.item,
        itemIndex: entry.itemIndex,
        left,
        top,
        width: itemWidth,
        height: rowHeight,
        frameHeight,
        thumbWidth: itemWidth,
        thumbHeight: rowThumbHeight
      });
      itemIndexes.push(entry.itemIndex);
      left += itemWidth + gap;
    });

    segments.push({
      index: rowIndex,
      start: top,
      size: rowHeight,
      itemIndexes
    });
    top += rowHeight;
    rowIndex += 1;
    rowItems = [];
    ratioSum = 0;
  };

  for (let itemIndex = 0; itemIndex < count; itemIndex += 1) {
    const item = itemAt(itemIndex);
    // Real ratio when the media is loaded; a stable estimate for unloaded slots
    // so a later real ratio does not repack the row (the estimate keeps the row
    // membership stable; only the arriving tile's own width adjusts slightly).
    const ratio = getItemAspectRatio?.(itemIndex, item) ??
      (item ? resolveAspectRatio(item) : fallbackRatio);
    rowItems.push({ item, itemIndex, ratio });
    ratioSum += ratio;
    const projectedWidth = ratioSum * targetRowHeight + gap * Math.max(0, rowItems.length - 1);
    const shouldFlush = projectedWidth >= contentWidth * ADAPTIVE_ROW_FILL_THRESHOLD;
    const isFinalInputItem = itemIndex === count - 1;
    if (shouldFlush && !isFinalInputItem) {
      flushRow(true);
    }
  }

  if (rowItems.length > 0) {
    flushRow(false);
  }

  const estimatedSegmentSize =
    segments.length > 0
      ? segments.reduce((sum, segment) => sum + segment.size, 0) / segments.length
      : targetRowHeight + labelHeight + gap;

  return {
    estimatedSegmentSize,
    placements,
    segments,
    totalSize: top
  };
}

function buildWaterfallLayout({
  gap,
  items,
  labelHeight,
  tileMinWidth,
  viewportWidth,
  totalCount,
  getItemAt,
  getItemAspectRatio,
  estimatedRatio
}: Omit<LayoutBuildOptions, "layoutMode">): LayoutGeometry {
  const contentWidth = normalizeViewportWidth(viewportWidth, tileMinWidth);
  const columnCount = resolveColumnCount(contentWidth, tileMinWidth, gap);
  const tileWidth = resolveColumnWidth(contentWidth, columnCount, gap);
  const columnHeights = Array.from({ length: columnCount }, () => 0);
  const placements: LayoutPlacement[] = [];

  const count = totalCount ?? items.length;
  const itemAt = getItemAt ?? ((index: number) => items[index]);
  const fallbackRatio = estimatedRatio && estimatedRatio > 0 ? estimatedRatio : 1;

  for (let itemIndex = 0; itemIndex < count; itemIndex += 1) {
    const item = itemAt(itemIndex);
    const ratio = getItemAspectRatio?.(itemIndex, item) ??
      (item ? resolveAspectRatio(item) : fallbackRatio);
    const thumbHeight = Math.max(1, Math.round(tileWidth / ratio));
    const frameHeight = thumbHeight + labelHeight;
    const itemHeight = frameHeight + gap;
    const columnIndex = indexOfShortestColumn(columnHeights);
    const top = columnHeights[columnIndex];
    const left = columnIndex * (tileWidth + gap);
    placements.push({
      item,
      itemIndex,
      left,
      top,
      width: tileWidth,
      height: itemHeight,
      frameHeight,
      thumbWidth: tileWidth,
      thumbHeight
    });
    columnHeights[columnIndex] += itemHeight;
  }

  const totalSize = Math.max(0, ...columnHeights);
  const bandSize = Math.max(
    WATERFALL_MIN_BAND_PX,
    Math.round((tileWidth + labelHeight + gap) * WATERFALL_BAND_SCALE)
  );
  const bandCount = totalSize === 0 ? 0 : Math.ceil(totalSize / bandSize);
  const bandIndexes = Array.from({ length: bandCount }, () => [] as number[]);

  placements.forEach((placement, placementIndex) => {
    const startBand = Math.floor(placement.top / bandSize);
    const endBand = Math.floor(
      Math.max(placement.top, placement.top + placement.frameHeight - 1) / bandSize
    );
    for (let bandIndex = startBand; bandIndex <= endBand; bandIndex += 1) {
      bandIndexes[bandIndex]?.push(placementIndex);
    }
  });

  const segments: LayoutSegment[] = bandIndexes.map((itemIndexes, bandIndex) => ({
    index: bandIndex,
    start: bandIndex * bandSize,
    size:
      bandIndex === bandIndexes.length - 1
        ? Math.max(1, totalSize - bandIndex * bandSize)
        : bandSize,
    itemIndexes
  }));

  return {
    estimatedSegmentSize: bandSize,
    placements,
    segments,
    totalSize
  };
}

function findSegmentRange(
  segments: LayoutSegment[],
  rangeStart: number,
  rangeEnd: number
): { startIndex: number; endIndex: number } | null {
  if (segments.length === 0 || rangeEnd <= rangeStart) {
    return null;
  }

  let startIndex = segments.findIndex((segment) =>
    intersectsRange(segment.start, segment.size, rangeStart, rangeEnd)
  );
  if (startIndex < 0) {
    if (rangeStart <= 0) {
      startIndex = 0;
    } else {
      return null;
    }
  }

  let endIndex = startIndex;
  for (let segmentIndex = startIndex; segmentIndex < segments.length; segmentIndex += 1) {
    const segment = segments[segmentIndex];
    if (!segment) {
      break;
    }
    if (segment.start >= rangeEnd) {
      break;
    }
    endIndex = segmentIndex;
  }

  return { endIndex, startIndex };
}

function normalizeViewportWidth(viewportWidth: number, tileMinWidth: number) {
  return Math.max(tileMinWidth, Math.floor(viewportWidth));
}

function resolveColumnCount(contentWidth: number, tileMinWidth: number, gap: number) {
  return Math.max(1, Math.floor((contentWidth + gap) / (tileMinWidth + gap)));
}

function resolveColumnWidth(contentWidth: number, columnCount: number, gap: number) {
  return Math.max(1, Math.floor((contentWidth - gap * Math.max(0, columnCount - 1)) / columnCount));
}

function resolveAspectRatio(item: MediaRecord) {
  const width = item.width ?? 0;
  const height = item.height ?? 0;
  if (width > 0 && height > 0) {
    return width / height;
  }
  return 1;
}

function createSequentialIndexes(startIndex: number, endIndex: number) {
  return Array.from({ length: Math.max(0, endIndex - startIndex) }, (_, index) => startIndex + index);
}

function intersectsRange(start: number, size: number, rangeStart: number, rangeEnd: number) {
  const end = start + size;
  return end > rangeStart && start < rangeEnd;
}

function axisOverlap(startA: number, endA: number, startB: number, endB: number) {
  return Math.max(0, Math.min(endA, endB) - Math.max(startA, startB));
}

function indexOfShortestColumn(columnHeights: number[]) {
  let winnerIndex = 0;
  for (let columnIndex = 1; columnIndex < columnHeights.length; columnIndex += 1) {
    if (columnHeights[columnIndex] < columnHeights[winnerIndex]) {
      winnerIndex = columnIndex;
    }
  }
  return winnerIndex;
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function emptyScopedMedia(): ScopedMedia {
  return {
    ids: [],
    key: "",
    signatureKey: ""
  };
}
