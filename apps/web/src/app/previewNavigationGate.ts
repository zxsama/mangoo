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
