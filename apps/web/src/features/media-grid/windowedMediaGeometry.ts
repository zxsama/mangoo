import type { LibraryLayoutMode } from "./layoutMode.ts";

const LIST_ROW_HEIGHT_PX = 104;
const LIST_FRAME_HEIGHT_PX = 90;
const LIST_THUMBNAIL_WIDTH_PX = 108;
const LIST_THUMBNAIL_HEIGHT_PX = 72;
const ADAPTIVE_MIN_ROW_HEIGHT_PX = 112;
const WATERFALL_MIN_BAND_PX = 120;
const WATERFALL_BAND_SCALE = 0.9;
const RATIO_PATTERN = [0.72, 1.08, 1.42, 0.86, 1.76, 1.18, 0.64, 1.55] as const;
const WINDOWED_MEDIA_COUNT_THRESHOLD = 1_000;
const JUSTIFIED_FULL_TOTAL_MAX_ITEMS = 20_000;
// Chromium stops growing CSS layout boxes at roughly 33.5 million pixels.
// Stay comfortably below that ceiling and map the full logical media height
// onto this bounded physical scroll range instead of clamping away the tail.
export const WINDOWED_MEDIA_PHYSICAL_SPACER_LIMIT_PX = 16_000_000;

export function resolveMediaGeometryStrategy({
  hasMediaSlots,
  itemCount,
  layoutMode,
  loadedItemCount
}: {
  hasMediaSlots: boolean;
  itemCount: number;
  layoutMode: LibraryLayoutMode;
  loadedItemCount: number;
}) {
  const uniform = layoutMode === "grid" || layoutMode === "list";
  const justified = layoutMode === "adaptive" || layoutMode === "waterfall";
  const large = itemCount > loadedItemCount || itemCount > WINDOWED_MEDIA_COUNT_THRESHOLD;
  const justifiedOverCap = justified && itemCount > JUSTIFIED_FULL_TOTAL_MAX_ITEMS;
  return {
    justifiedFullTotal:
      hasMediaSlots && justified && !justifiedOverCap && large,
    windowedMedia:
      hasMediaSlots && (uniform || justifiedOverCap) && large
  };
}

export interface WindowedMediaPlacement {
  itemIndex: number;
  left: number;
  top: number;
  width: number;
  height: number;
  frameHeight: number;
  thumbWidth: number;
  thumbHeight: number;
}

export interface WindowedMediaSegment {
  index: number;
  start: number;
  size: number;
  itemIndexes: number[];
}

export interface WindowedMediaGeometry {
  layoutMode: LibraryLayoutMode;
  contentWidth: number;
  columnCount: number;
  segmentCount: number;
  estimatedSegmentSize: number;
  totalSize: number;
  resolvePlacement: (itemIndex: number) => WindowedMediaPlacement | undefined;
  resolveSegment: (segmentIndex: number) => WindowedMediaSegment | undefined;
  segmentIndexForItem: (itemIndex: number) => number | undefined;
  itemIndexesInRange: (rangeStart: number, rangeEnd: number) => number[];
}

export interface WindowedMediaVirtualizationPlan {
  logicalMediaScrollTop: number;
  mediaRenderOffset: number;
  mediaSegmentIndexes: number[];
  spacerSize: number;
  virtualizerCount: number;
}

export interface WindowedMediaScrollCoordinates {
  logicalMediaScrollTop: number;
  mediaRenderOffset: number;
  physicalMediaScrollTop: number;
  spacerSize: number;
}

export function resolveWindowedMediaScrollCoordinates({
  geometry,
  mediaOffset,
  scrollTop,
  viewportHeight
}: {
  geometry: WindowedMediaGeometry;
  mediaOffset: number;
  scrollTop: number;
  viewportHeight: number;
}): WindowedMediaScrollCoordinates {
  const offset = Math.max(0, mediaOffset);
  const viewport = Math.max(0, viewportHeight);
  const physicalMediaExtent = Math.min(
    geometry.totalSize,
    Math.max(0, WINDOWED_MEDIA_PHYSICAL_SPACER_LIMIT_PX - offset)
  );
  const spacerSize = offset + physicalMediaExtent;
  const physicalRange = Math.max(0, physicalMediaExtent - viewport);
  const logicalRange = Math.max(0, geometry.totalSize - viewport);
  const physicalMediaScrollTop = clamp(scrollTop - offset, 0, physicalRange);
  const logicalMediaScrollTop = physicalRange > 0
    ? physicalMediaScrollTop * logicalRange / physicalRange
    : 0;

  return {
    logicalMediaScrollTop,
    // Keep tile dimensions and local distances unscaled. Only their absolute
    // origin is rebased into the browser-safe physical coordinate window.
    mediaRenderOffset: offset + physicalMediaScrollTop - logicalMediaScrollTop,
    physicalMediaScrollTop,
    spacerSize
  };
}

export function windowedPhysicalScrollTopForLogicalMediaScroll({
  geometry,
  logicalMediaScrollTop,
  mediaOffset,
  viewportHeight
}: {
  geometry: WindowedMediaGeometry;
  logicalMediaScrollTop: number;
  mediaOffset: number;
  viewportHeight: number;
}): number {
  const offset = Math.max(0, mediaOffset);
  const viewport = Math.max(0, viewportHeight);
  const physicalMediaExtent = Math.min(
    geometry.totalSize,
    Math.max(0, WINDOWED_MEDIA_PHYSICAL_SPACER_LIMIT_PX - offset)
  );
  const physicalRange = Math.max(0, physicalMediaExtent - viewport);
  const logicalRange = Math.max(0, geometry.totalSize - viewport);
  const logical = clamp(logicalMediaScrollTop, 0, logicalRange);
  const physical = logicalRange > 0 ? logical * physicalRange / logicalRange : 0;
  return offset + physical;
}

export function buildWindowedMediaVirtualizationPlan({
  geometry,
  mediaOffset,
  overscanSegments,
  prefixVirtualRowCount,
  scrollTop,
  viewportHeight
}: {
  geometry: WindowedMediaGeometry;
  mediaOffset: number;
  overscanSegments: number;
  prefixVirtualRowCount: number;
  scrollTop: number;
  viewportHeight: number;
}): WindowedMediaVirtualizationPlan {
  const coordinates = resolveWindowedMediaScrollCoordinates({
    geometry,
    mediaOffset,
    scrollTop,
    viewportHeight
  });
  const rangeStart = coordinates.logicalMediaScrollTop;
  const rangeEnd = Math.min(
    geometry.totalSize,
    rangeStart + Math.max(0, viewportHeight)
  );
  const visibleStart = firstIntersectingSegment(geometry, rangeStart);
  const visibleEnd = firstSegmentStartingAtOrAfter(geometry, rangeEnd);
  const overscan = Math.max(0, Math.floor(overscanSegments));
  const start = Math.max(0, visibleStart - overscan);
  const end = Math.min(geometry.segmentCount, Math.max(visibleStart, visibleEnd) + overscan);

  return {
    logicalMediaScrollTop: coordinates.logicalMediaScrollTop,
    mediaRenderOffset: coordinates.mediaRenderOffset,
    mediaSegmentIndexes: sequentialIndexes(start, end),
    spacerSize: coordinates.spacerSize,
    virtualizerCount: Math.max(0, Math.floor(prefixVirtualRowCount))
  };
}

export function buildWindowedMediaGeometry({
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
}): WindowedMediaGeometry {
  const itemCount = Math.max(0, Math.floor(count));
  const contentWidth = Math.max(tileMinWidth, Math.floor(viewportWidth || tileMinWidth));
  if (layoutMode === "list") {
    return buildListGeometry(itemCount, contentWidth);
  }
  const columnCount = Math.max(1, Math.floor((contentWidth + gap) / (tileMinWidth + gap)));
  const tileWidth = Math.max(1, Math.floor((contentWidth - gap * (columnCount - 1)) / columnCount));
  if (layoutMode === "adaptive") {
    return buildAdaptiveGeometry({
      columnCount,
      contentWidth,
      count: itemCount,
      gap,
      labelHeight,
      layoutMode,
      tileMinWidth
    });
  }
  if (layoutMode === "waterfall") {
    return buildWaterfallGeometry({
      columnCount,
      contentWidth,
      count: itemCount,
      gap,
      labelHeight,
      layoutMode,
      tileWidth
    });
  }
  return buildGridGeometry({
    columnCount,
    contentWidth,
    count: itemCount,
    gap,
    labelHeight,
    layoutMode,
    tileWidth
  });
}

export function stableWindowedAspectRatio(itemIndex: number): number {
  return RATIO_PATTERN[positiveModulo(itemIndex, RATIO_PATTERN.length)]!;
}

function buildListGeometry(count: number, contentWidth: number): WindowedMediaGeometry {
  const resolvePlacement = (itemIndex: number): WindowedMediaPlacement | undefined => {
    if (!validItemIndex(itemIndex, count)) return undefined;
    return {
      itemIndex,
      left: 0,
      top: itemIndex * LIST_ROW_HEIGHT_PX,
      width: contentWidth,
      height: LIST_ROW_HEIGHT_PX,
      frameHeight: LIST_FRAME_HEIGHT_PX,
      thumbWidth: LIST_THUMBNAIL_WIDTH_PX,
      thumbHeight: LIST_THUMBNAIL_HEIGHT_PX
    };
  };
  return {
    layoutMode: "list",
    contentWidth,
    columnCount: 1,
    segmentCount: count,
    estimatedSegmentSize: LIST_ROW_HEIGHT_PX,
    totalSize: count * LIST_ROW_HEIGHT_PX,
    resolvePlacement,
    resolveSegment: (segmentIndex) => validSegmentIndex(segmentIndex, count)
      ? {
          index: segmentIndex,
          start: segmentIndex * LIST_ROW_HEIGHT_PX,
          size: LIST_ROW_HEIGHT_PX,
          itemIndexes: [segmentIndex]
        }
      : undefined,
    segmentIndexForItem: (itemIndex) => validItemIndex(itemIndex, count) ? itemIndex : undefined,
    itemIndexesInRange: (rangeStart, rangeEnd) => fixedRowIndexes(
      count,
      1,
      LIST_ROW_HEIGHT_PX,
      rangeStart,
      rangeEnd
    )
  };
}

function buildGridGeometry(input: {
  columnCount: number;
  contentWidth: number;
  count: number;
  gap: number;
  labelHeight: number;
  layoutMode: "grid";
  tileWidth: number;
}): WindowedMediaGeometry {
  const rowHeight = input.tileWidth + input.labelHeight + input.gap;
  const rowCount = Math.ceil(input.count / input.columnCount);
  const resolvePlacement = (itemIndex: number): WindowedMediaPlacement | undefined => {
    if (!validItemIndex(itemIndex, input.count)) return undefined;
    const rowIndex = Math.floor(itemIndex / input.columnCount);
    const columnIndex = itemIndex % input.columnCount;
    return {
      itemIndex,
      left: columnIndex * (input.tileWidth + input.gap),
      top: rowIndex * rowHeight,
      width: input.tileWidth,
      height: rowHeight,
      frameHeight: input.tileWidth + input.labelHeight,
      thumbWidth: input.tileWidth,
      thumbHeight: input.tileWidth
    };
  };
  return {
    layoutMode: input.layoutMode,
    contentWidth: input.contentWidth,
    columnCount: input.columnCount,
    segmentCount: rowCount,
    estimatedSegmentSize: rowHeight,
    totalSize: rowCount * rowHeight,
    resolvePlacement,
    resolveSegment: (segmentIndex) => {
      if (!validSegmentIndex(segmentIndex, rowCount)) return undefined;
      return {
        index: segmentIndex,
        start: segmentIndex * rowHeight,
        size: rowHeight,
        itemIndexes: sequentialIndexes(
          segmentIndex * input.columnCount,
          Math.min(input.count, (segmentIndex + 1) * input.columnCount)
        )
      };
    },
    segmentIndexForItem: (itemIndex) => validItemIndex(itemIndex, input.count)
      ? Math.floor(itemIndex / input.columnCount)
      : undefined,
    itemIndexesInRange: (rangeStart, rangeEnd) => fixedRowIndexes(
      input.count,
      input.columnCount,
      rowHeight,
      rangeStart,
      rangeEnd
    )
  };
}

function buildAdaptiveGeometry(input: {
  columnCount: number;
  contentWidth: number;
  count: number;
  gap: number;
  labelHeight: number;
  layoutMode: "adaptive";
  tileMinWidth: number;
}): WindowedMediaGeometry {
  const rowCount = Math.ceil(input.count / input.columnCount);
  const periodLength = RATIO_PATTERN.length;
  const fullRowSizes = Array.from({ length: periodLength }, (_, rowIndex) =>
    adaptiveRowMetrics(input, rowIndex, input.columnCount).rowHeight
  );
  const periodSize = sum(fullRowSizes);
  const topForRow = (rowIndex: number) => periodicPrefix(fullRowSizes, periodSize, rowIndex);
  const rowMetrics = (rowIndex: number) => {
    const rowStart = rowIndex * input.columnCount;
    const itemCount = Math.max(0, Math.min(input.columnCount, input.count - rowStart));
    return adaptiveRowMetrics(input, rowIndex, itemCount);
  };
  const resolvePlacement = (itemIndex: number): WindowedMediaPlacement | undefined => {
    if (!validItemIndex(itemIndex, input.count)) return undefined;
    const rowIndex = Math.floor(itemIndex / input.columnCount);
    const columnIndex = itemIndex % input.columnCount;
    const metrics = rowMetrics(rowIndex);
    return {
      itemIndex,
      left: metrics.lefts[columnIndex]!,
      top: topForRow(rowIndex),
      width: metrics.widths[columnIndex]!,
      height: metrics.rowHeight,
      frameHeight: metrics.thumbHeight + input.labelHeight,
      thumbWidth: metrics.widths[columnIndex]!,
      thumbHeight: metrics.thumbHeight
    };
  };
  const totalSize = rowCount === 0
    ? 0
    : topForRow(rowCount - 1) + rowMetrics(rowCount - 1).rowHeight;
  const resolveSegment = (segmentIndex: number): WindowedMediaSegment | undefined => {
    if (!validSegmentIndex(segmentIndex, rowCount)) return undefined;
    const metrics = rowMetrics(segmentIndex);
    return {
      index: segmentIndex,
      start: topForRow(segmentIndex),
      size: metrics.rowHeight,
      itemIndexes: sequentialIndexes(
        segmentIndex * input.columnCount,
        Math.min(input.count, (segmentIndex + 1) * input.columnCount)
      )
    };
  };
  return {
    layoutMode: input.layoutMode,
    contentWidth: input.contentWidth,
    columnCount: input.columnCount,
    segmentCount: rowCount,
    estimatedSegmentSize: periodSize / periodLength,
    totalSize,
    resolvePlacement,
    resolveSegment,
    segmentIndexForItem: (itemIndex) => validItemIndex(itemIndex, input.count)
      ? Math.floor(itemIndex / input.columnCount)
      : undefined,
    itemIndexesInRange: (rangeStart, rangeEnd) => variableSegmentIndexes(
      rowCount,
      resolveSegment,
      rangeStart,
      rangeEnd
    )
  };
}

function adaptiveRowMetrics(
  input: {
    columnCount: number;
    contentWidth: number;
    gap: number;
    labelHeight: number;
    tileMinWidth: number;
  },
  rowIndex: number,
  itemCount: number
) {
  const ratios = Array.from({ length: itemCount }, (_, columnIndex) =>
    stableWindowedAspectRatio(rowIndex * input.columnCount + columnIndex)
  );
  const availableWidth = Math.max(1, input.contentWidth - input.gap * Math.max(0, itemCount - 1));
  const ratioSum = Math.max(0.01, sum(ratios));
  const naturalHeight = availableWidth / ratioSum;
  const thumbHeight = itemCount < input.columnCount
    ? Math.min(naturalHeight, input.tileMinWidth * 1.16)
    : Math.max(ADAPTIVE_MIN_ROW_HEIGHT_PX, naturalHeight);
  const widths: number[] = [];
  const lefts: number[] = [];
  let left = 0;
  for (let columnIndex = 0; columnIndex < itemCount; columnIndex += 1) {
    lefts.push(left);
    const remainingColumns = itemCount - columnIndex - 1;
    const remainingWidth = input.contentWidth - left - input.gap * remainingColumns;
    const width = columnIndex === itemCount - 1
      ? Math.max(1, remainingWidth)
      : Math.max(1, Math.min(remainingWidth - remainingColumns, Math.round(thumbHeight * ratios[columnIndex]!)));
    widths.push(width);
    left += width + input.gap;
  }
  return {
    lefts,
    widths,
    thumbHeight,
    rowHeight: thumbHeight + input.labelHeight + input.gap
  };
}

function buildWaterfallGeometry(input: {
  columnCount: number;
  contentWidth: number;
  count: number;
  gap: number;
  labelHeight: number;
  layoutMode: "waterfall";
  tileWidth: number;
}): WindowedMediaGeometry {
  const periodLength = RATIO_PATTERN.length;
  const heightPatterns = Array.from({ length: input.columnCount }, (_, columnIndex) =>
    Array.from({ length: periodLength }, (_, rowIndex) =>
      waterfallItemHeight(input, rowIndex, columnIndex)
    )
  );
  const patternSums = heightPatterns.map(sum);
  const rowCountForColumn = (columnIndex: number) =>
    input.count <= columnIndex ? 0 : Math.ceil((input.count - columnIndex) / input.columnCount);
  const topFor = (rowIndex: number, columnIndex: number) =>
    periodicPrefix(heightPatterns[columnIndex]!, patternSums[columnIndex]!, rowIndex);
  const totalSize = Math.max(
    0,
    ...Array.from({ length: input.columnCount }, (_, columnIndex) =>
      topFor(rowCountForColumn(columnIndex), columnIndex)
    )
  );
  const bandSize = Math.max(
    WATERFALL_MIN_BAND_PX,
    Math.round((input.tileWidth + input.labelHeight + input.gap) * WATERFALL_BAND_SCALE)
  );
  const segmentCount = totalSize === 0 ? 0 : Math.ceil(totalSize / bandSize);
  const resolvePlacement = (itemIndex: number): WindowedMediaPlacement | undefined => {
    if (!validItemIndex(itemIndex, input.count)) return undefined;
    const rowIndex = Math.floor(itemIndex / input.columnCount);
    const columnIndex = itemIndex % input.columnCount;
    const height = waterfallItemHeight(input, rowIndex, columnIndex);
    const thumbHeight = height - input.labelHeight - input.gap;
    return {
      itemIndex,
      left: columnIndex * (input.tileWidth + input.gap),
      top: topFor(rowIndex, columnIndex),
      width: input.tileWidth,
      height,
      frameHeight: thumbHeight + input.labelHeight,
      thumbWidth: input.tileWidth,
      thumbHeight
    };
  };
  const indexesInVerticalRange = (rangeStart: number, rangeEnd: number) => {
    if (rangeEnd <= rangeStart || input.count === 0) return [];
    const indexes: number[] = [];
    for (let columnIndex = 0; columnIndex < input.columnCount; columnIndex += 1) {
      const columnRows = rowCountForColumn(columnIndex);
      let low = 0;
      let high = columnRows;
      while (low < high) {
        const middle = Math.floor((low + high) / 2);
        const bottom = topFor(middle + 1, columnIndex);
        if (bottom <= rangeStart) low = middle + 1;
        else high = middle;
      }
      for (let rowIndex = low; rowIndex < columnRows; rowIndex += 1) {
        const top = topFor(rowIndex, columnIndex);
        if (top >= rangeEnd) break;
        const itemIndex = rowIndex * input.columnCount + columnIndex;
        if (itemIndex < input.count) indexes.push(itemIndex);
      }
    }
    return indexes.sort((left, right) => left - right);
  };
  const resolveSegment = (segmentIndex: number): WindowedMediaSegment | undefined => {
    if (!validSegmentIndex(segmentIndex, segmentCount)) return undefined;
    const start = segmentIndex * bandSize;
    const size = segmentIndex === segmentCount - 1
      ? Math.max(1, totalSize - start)
      : bandSize;
    return {
      index: segmentIndex,
      start,
      size,
      itemIndexes: indexesInVerticalRange(start, start + size)
    };
  };
  return {
    layoutMode: input.layoutMode,
    contentWidth: input.contentWidth,
    columnCount: input.columnCount,
    segmentCount,
    estimatedSegmentSize: bandSize,
    totalSize,
    resolvePlacement,
    resolveSegment,
    segmentIndexForItem: (itemIndex) => {
      const placement = resolvePlacement(itemIndex);
      return placement ? Math.min(segmentCount - 1, Math.floor(placement.top / bandSize)) : undefined;
    },
    itemIndexesInRange: indexesInVerticalRange
  };
}

function waterfallItemHeight(
  input: { columnCount: number; gap: number; labelHeight: number; tileWidth: number },
  rowIndex: number,
  columnIndex: number
) {
  const itemIndex = rowIndex * input.columnCount + columnIndex;
  const ratio = stableWindowedAspectRatio(itemIndex);
  return Math.max(1, Math.round(input.tileWidth / ratio)) + input.labelHeight + input.gap;
}

function fixedRowIndexes(
  count: number,
  columnCount: number,
  rowHeight: number,
  rangeStart: number,
  rangeEnd: number
) {
  if (rangeEnd <= rangeStart || count === 0) return [];
  const rowCount = Math.ceil(count / columnCount);
  const startRow = clamp(Math.floor(Math.max(0, rangeStart) / rowHeight), 0, rowCount - 1);
  const endRow = clamp(Math.floor(Math.max(rangeStart, rangeEnd - 1) / rowHeight), startRow, rowCount - 1);
  return sequentialIndexes(startRow * columnCount, Math.min(count, (endRow + 1) * columnCount));
}

function variableSegmentIndexes(
  segmentCount: number,
  resolveSegment: (segmentIndex: number) => WindowedMediaSegment | undefined,
  rangeStart: number,
  rangeEnd: number
) {
  if (rangeEnd <= rangeStart || segmentCount === 0) return [];
  let low = 0;
  let high = segmentCount;
  while (low < high) {
    const middle = Math.floor((low + high) / 2);
    const segment = resolveSegment(middle)!;
    if (segment.start + segment.size <= rangeStart) low = middle + 1;
    else high = middle;
  }
  const indexes: number[] = [];
  for (let segmentIndex = low; segmentIndex < segmentCount; segmentIndex += 1) {
    const segment = resolveSegment(segmentIndex)!;
    if (segment.start >= rangeEnd) break;
    indexes.push(...segment.itemIndexes);
  }
  return indexes;
}

function firstIntersectingSegment(geometry: WindowedMediaGeometry, offset: number) {
  let low = 0;
  let high = geometry.segmentCount;
  while (low < high) {
    const middle = Math.floor((low + high) / 2);
    const segment = geometry.resolveSegment(middle)!;
    if (segment.start + segment.size <= offset) low = middle + 1;
    else high = middle;
  }
  return low;
}

function firstSegmentStartingAtOrAfter(geometry: WindowedMediaGeometry, offset: number) {
  let low = 0;
  let high = geometry.segmentCount;
  while (low < high) {
    const middle = Math.floor((low + high) / 2);
    const segment = geometry.resolveSegment(middle)!;
    if (segment.start < offset) low = middle + 1;
    else high = middle;
  }
  return low;
}

function periodicPrefix(pattern: readonly number[], patternSum: number, count: number) {
  if (count <= 0 || pattern.length === 0) return 0;
  const cycles = Math.floor(count / pattern.length);
  const remainder = count % pattern.length;
  let total = cycles * patternSum;
  for (let index = 0; index < remainder; index += 1) total += pattern[index]!;
  return total;
}

function sequentialIndexes(start: number, end: number) {
  return Array.from({ length: Math.max(0, end - start) }, (_, index) => start + index);
}

function sum(values: readonly number[]) {
  return values.reduce((total, value) => total + value, 0);
}

function validItemIndex(itemIndex: number, count: number) {
  return Number.isInteger(itemIndex) && itemIndex >= 0 && itemIndex < count;
}

function validSegmentIndex(segmentIndex: number, count: number) {
  return Number.isInteger(segmentIndex) && segmentIndex >= 0 && segmentIndex < count;
}

function positiveModulo(value: number, modulus: number) {
  return ((value % modulus) + modulus) % modulus;
}

function clamp(value: number, minimum: number, maximum: number) {
  return Math.max(minimum, Math.min(maximum, value));
}
