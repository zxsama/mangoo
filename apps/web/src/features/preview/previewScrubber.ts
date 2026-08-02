// Pure logic for the preview scrubber: where a pointer lands in the folder,
// where the thumb is drawn, and whether the bar is visible at all.
//
// No DOM, no React, no app imports — the whole contract is testable under
// node --test, which is where the interesting cases live (a 20000-image folder,
// a single-image folder, a zero-width track).

/**
 * How far above the bottom of the preview viewport the pointer counts as
 * "near the bottom". Generous on purpose: the bar auto-hides, so a narrow band
 * would mean hunting for it.
 */
export const SCRUBBER_REVEAL_BAND_PX = 120;

function clampIndex(index: number, total: number): number {
  if (!Number.isFinite(index) || total <= 0) {
    return 0;
  }
  return Math.min(total - 1, Math.max(0, Math.round(index)));
}

/**
 * The 0-based folder index a pointer position addresses.
 *
 * On a large folder one pixel of track spans many images (20000 images over an
 * 800px track is ~25 per pixel). That coarse step is inherent to the mapping
 * rather than a throttle, and it is why a drag that looks continuous visits
 * hundreds of images instead of twenty thousand.
 */
export function scrubberIndexForPointerX(
  pointerX: number,
  track: { left: number; width: number },
  total: number
): number {
  if (total <= 1 || !Number.isFinite(pointerX)) {
    return 0;
  }
  if (!Number.isFinite(track.width) || track.width <= 0) {
    // A zero-width track carries no position information; addressing the first
    // image is the only answer that cannot be wrong by an arbitrary amount.
    return 0;
  }
  const fraction = (pointerX - track.left) / track.width;
  return clampIndex(fraction * (total - 1), total);
}

/**
 * Where to draw the thumb for an index, as a 0..1 fraction of the track.
 *
 * A fraction rather than a pixel offset so the component positions the thumb
 * with a CSS percentage and this module stays free of layout assumptions. It is
 * the exact inverse of `scrubberIndexForPointerX`, which is what guarantees the
 * thumb and the displayed image agree.
 */
export function scrubberFractionForIndex(index: number, total: number): number {
  if (total <= 1 || !Number.isFinite(index)) {
    return 0;
  }
  const clamped = clampIndex(index, total);
  return clamped / (total - 1);
}

/**
 * Whether the bar is on screen.
 *
 * `dragging` wins over `pointerNearBottom` deliberately: releasing the thumb
 * outside the band must not make the bar vanish underneath the cursor.
 */
export function shouldShowScrubber(input: {
  previewOpen: boolean;
  pinned: boolean;
  pointerNearBottom: boolean;
  dragging: boolean;
}): boolean {
  if (!input.previewOpen) {
    return false;
  }
  return input.pinned || input.dragging || input.pointerNearBottom;
}
