import type { MediaRecord } from "@mangoo/core-client";

// Byte estimate used for a neighbor whose `size` is unknown/zero, so the buffer
// budget still bounds how many such neighbors we warm.
export const PREVIEW_PREFETCH_UNKNOWN_MEDIA_BYTES = 32 * 1024 * 1024;
// Hard cap on how many neighbors we ever consider, independent of the byte
// budget, so a huge buffer can't schedule an unbounded prefetch fan-out.
export const MAX_PREVIEW_PREFETCH_CANDIDATES = 512;

// Decide which neighbor media to warm into the in-memory original-preview
// buffer around `selectedIndex`, bounded by `bufferBytes` and the candidate cap.
//
// Walks BOTH directions, nearest-first (next, prev, next+1, prev+1, ...), so
// browsing forward AND backward both hit the warmed cache. (A prior refactor
// walked forward-only, so pressing "previous" never benefited from the buffer.)
//
// Only `image` neighbors are prefetched: video preview streams/downloads the
// full original on demand via its own path, so pulling a whole large video into
// this blob buffer ahead of time is pure waste — images are the only kind this
// buffer actually accelerates.
export function buildPreviewPrefetchWindow(
  media: MediaRecord[],
  selectedIndex: number,
  bufferBytes: number
): MediaRecord[] {
  if (bufferBytes <= 0 || selectedIndex < 0 || selectedIndex >= media.length) {
    return [];
  }

  const windowItems: MediaRecord[] = [];
  let estimatedBytes = 0;

  // Nearest-first interleave: offset 1 (next), -1 (prev), 2, -2, ... Each step
  // yields one forward then one backward candidate, so the closest neighbors on
  // both sides are warmed before farther ones.
  for (
    let distance = 1;
    windowItems.length < MAX_PREVIEW_PREFETCH_CANDIDATES;
    distance += 1
  ) {
    const forwardIndex = selectedIndex + distance;
    const backwardIndex = selectedIndex - distance;
    const forwardInRange = forwardIndex < media.length;
    const backwardInRange = backwardIndex >= 0;
    if (!forwardInRange && !backwardInRange) {
      break;
    }

    let budgetExhausted = false;
    for (const index of [forwardIndex, backwardIndex]) {
      if (index < 0 || index >= media.length || windowItems.length >= MAX_PREVIEW_PREFETCH_CANDIDATES) {
        continue;
      }
      const neighbor = media[index];
      if (!neighbor || neighbor.kind !== "image") {
        continue;
      }
      const neighborBytes =
        neighbor.size > 0 ? neighbor.size : PREVIEW_PREFETCH_UNKNOWN_MEDIA_BYTES;
      // Always allow the first neighbor even if it alone exceeds the budget, so a
      // small buffer still warms the single nearest image; stop once adding more
      // would overflow.
      if (windowItems.length > 0 && estimatedBytes + neighborBytes > bufferBytes) {
        budgetExhausted = true;
        break;
      }
      windowItems.push(neighbor);
      estimatedBytes += neighborBytes;
      if (estimatedBytes >= bufferBytes) {
        budgetExhausted = true;
        break;
      }
    }
    if (budgetExhausted) {
      break;
    }
  }

  return windowItems;
}
