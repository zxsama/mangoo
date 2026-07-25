import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  buildPreviewPrefetchWindow,
  MAX_PREVIEW_PREFETCH_CANDIDATES
} from "../src/app/previewPrefetchWindow.ts";

type Media = { id: number; kind: string; size: number };

function media(count: number, kind = "image", size = 1_000): Media[] {
  return Array.from({ length: count }, (_, i) => ({ id: i + 1, kind, size }));
}

// Cast helper: the pure function only touches id/kind/size.
function build(items: Media[], index: number, bufferBytes: number): number[] {
  return buildPreviewPrefetchWindow(items as never, index, bufferBytes).map((m) => (m as Media).id);
}

describe("buildPreviewPrefetchWindow", () => {
  it("prefetches BOTH directions, nearest-first (regression: forward-only bug)", () => {
    // 5 images [1..5], selected index 2 (id=3), generous budget.
    const ids = build(media(5), 2, 1_000_000);
    // Nearest-first interleave from id=3: next(4), prev(2), next+1(5), prev+1(1).
    assert.deepEqual(ids, [4, 2, 5, 1]);
  });

  it("still prefetches the previous neighbor when on the LAST item", () => {
    const ids = build(media(4), 3, 1_000_000); // selected = id 4 (last)
    // No forward neighbor; backward walk warms 3, 2, 1.
    assert.deepEqual(ids, [3, 2, 1]);
  });

  it("still prefetches the next neighbor when on the FIRST item", () => {
    const ids = build(media(4), 0, 1_000_000);
    assert.deepEqual(ids, [2, 3, 4]);
  });

  it("respects the byte budget but always warms at least the nearest neighbor", () => {
    // Each image 1000 bytes; budget 1 byte -> only the single nearest is warmed.
    const ids = build(media(5), 2, 1);
    assert.equal(ids.length, 1);
    assert.equal(ids[0], 4); // nearest forward
  });

  it("stops adding once the budget would overflow", () => {
    // budget 2500, images 1000 each -> 2 full (2000), a 3rd (3000) overflows.
    const ids = build(media(9), 4, 2500);
    assert.equal(ids.length, 2);
  });

  it("skips non-image neighbors (video is served by its own path)", () => {
    const items: Media[] = [
      { id: 1, kind: "image", size: 1000 },
      { id: 2, kind: "video", size: 1000 },
      { id: 3, kind: "image", size: 1000 }, // selected
      { id: 4, kind: "video", size: 1000 },
      { id: 5, kind: "image", size: 1000 }
    ];
    const ids = build(items, 2, 1_000_000);
    assert.deepEqual(ids, [5, 1]); // only the image neighbors, nearest-first
  });

  it("returns nothing for a zero/negative buffer or out-of-range index", () => {
    assert.deepEqual(build(media(5), 2, 0), []);
    assert.deepEqual(build(media(5), 2, -10), []);
    assert.deepEqual(build(media(5), -1, 1_000_000), []);
    assert.deepEqual(build(media(5), 99, 1_000_000), []);
  });

  it("never exceeds the candidate cap", () => {
    const ids = build(media(5000), 2500, Number.MAX_SAFE_INTEGER);
    assert.ok(ids.length <= MAX_PREVIEW_PREFETCH_CANDIDATES);
  });
});
