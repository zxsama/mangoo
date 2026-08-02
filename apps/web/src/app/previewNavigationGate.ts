// Whether a prev/next preview step is possible RIGHT NOW.
//
// Deliberately depends only on position — never on whether the current frame
// has finished painting. The old `previewNavigationLockedRef` gate existed
// because every step cost a full-resolution main-thread decode (measured 337ms
// at 16384px), so an unlocked held arrow key queued unbounded work. The
// pre-decoded bitmap pool removed that cost — a pooled neighbour paints in the
// same tick, and an unpooled one leaves the previous canvas on screen until its
// bitmap lands — and `PreviewCanvas` aborts the request it outran. The lock is
// now pure input lag.
export function canNavigatePreview(input: {
  previewOpen: boolean;
  selectedMediaIndex: number;
  itemCount: number;
  direction: "previous" | "next";
}): boolean {
  if (!input.previewOpen || input.itemCount <= 0 || input.selectedMediaIndex < 0) {
    return false;
  }
  return input.direction === "previous"
    ? input.selectedMediaIndex > 0
    : input.selectedMediaIndex < input.itemCount - 1;
}

/**
 * Whether a prev/next step may START right now.
 *
 * Extends the position-only `canNavigatePreview` with one condition: the
 * currently-selected media must have PAINTED at least once. Not committed, not
 * fully loaded — the moment pixels appear.
 *
 * This is deliberately far weaker than the readiness lock it replaces.
 * `PreviewCanvas` reports a paint on every draw, including the synchronous one
 * that happens in the same tick when a neighbour is already pooled, so whenever
 * prefetch keeps up this gate never engages. It bites only on a cold pool —
 * exactly the case where navigation used to run past images that never
 * appeared. The wheel makes that worse: a single flick emits many `wheel`
 * events, each calling next/previous, so one gesture could advance several
 * images at once.
 *
 * Input arriving while the gate is closed is DROPPED by the caller, never
 * queued: holding a key advances exactly as fast as images can begin painting,
 * so nothing banks up and nothing is skipped.
 */
export function canStartPreviewNavigation(input: {
  previewOpen: boolean;
  selectedMediaIndex: number;
  itemCount: number;
  direction: "previous" | "next";
  selectedMediaId: number | null;
  paintedMediaId: number | null;
}): boolean {
  if (
    !canNavigatePreview({
      previewOpen: input.previewOpen,
      selectedMediaIndex: input.selectedMediaIndex,
      itemCount: input.itemCount,
      direction: input.direction
    })
  ) {
    return false;
  }
  // Nothing selected and nothing painted is the "no frame expected" state, not
  // a pending one — blocking it would wedge navigation before the first paint.
  if (input.selectedMediaId === null && input.paintedMediaId === null) {
    return true;
  }
  return input.selectedMediaId === input.paintedMediaId;
}
