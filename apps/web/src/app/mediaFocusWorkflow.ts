import type { MediaRecord } from "@mangoo/core-client";
import type { MediaFocusCommand } from "@mangoo/notification-contract";
import type {
  LayoutGeometry,
  LayoutPlacement
} from "../features/media-grid/layoutGeometry.ts";

export interface MediaGridLocateRequest {
  requestId: string;
  mediaId: number;
}

export interface JustifiedMediaLocateTarget {
  placement: LayoutPlacement;
  placementIndex: number;
  segmentIndex: number;
}

export function resolveJustifiedMediaLocateTarget(
  geometry: LayoutGeometry,
  mediaSlots: ReadonlyMap<number, MediaRecord>,
  mediaId: number
): JustifiedMediaLocateTarget | undefined {
  let placementIndex: number | undefined;
  for (const [index, item] of mediaSlots) {
    if (item.id === mediaId) {
      placementIndex = index;
      break;
    }
  }
  if (placementIndex === undefined) return undefined;
  const placement = geometry.placements[placementIndex];
  if (!placement || placement.item?.id !== mediaId) return undefined;
  const segmentIndex = geometry.segments.findIndex((segment) =>
    segment.itemIndexes.includes(placementIndex!)
  );
  if (segmentIndex < 0) return undefined;
  return { placement, placementIndex, segmentIndex };
}

export async function prepareMediaFocusNavigation(
  command: MediaFocusCommand,
  dependencies: {
    closePreview: () => void;
    locateMedia: (command: MediaFocusCommand, signal: AbortSignal) => Promise<boolean>;
  },
  signal: AbortSignal = new AbortController().signal
): Promise<MediaGridLocateRequest | null> {
  if (signal.aborted) return null;
  dependencies.closePreview();
  if (!(await dependencies.locateMedia(command, signal)) || signal.aborted) return null;
  return { requestId: command.requestId, mediaId: command.fileId };
}

export function resolveMediaLocateVirtualIndex(input: {
  mediaIndex: number;
  columnCount: number;
  mediaRowStartIndex: number;
  mediaRowCount: number;
}): number | undefined {
  if (
    input.mediaIndex < 0 ||
    input.columnCount <= 0 ||
    input.mediaRowStartIndex < 0 ||
    input.mediaRowCount <= 0
  ) {
    return undefined;
  }
  const row = Math.floor(input.mediaIndex / input.columnCount);
  if (row >= input.mediaRowCount) return undefined;
  return input.mediaRowStartIndex + row;
}
