import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { MediaRecord } from "@megle/core-client";
import {
  prepareMediaFocusNavigation,
  resolveJustifiedMediaLocateTarget,
  resolveMediaLocateVirtualIndex
} from "../src/app/mediaFocusWorkflow.ts";
import { buildLayoutGeometry } from "../src/features/media-grid/layoutGeometry.ts";

const request = {
  requestId: "focus-1",
  libraryId: "library-a",
  fileId: 42,
  folderId: 7,
  rootId: 3
};

describe("media focus workflow", () => {
  it("resolves a deep offscreen media slot to its exact virtual row", () => {
    assert.equal(resolveMediaLocateVirtualIndex({
      mediaIndex: 900_004,
      columnCount: 5,
      mediaRowStartIndex: 3,
      mediaRowCount: 200_000
    }), 180_003);
  });

  it("resolves adaptive and waterfall targets from the production full-total justified geometry", () => {
    const targetIndex = 87;
    const target: MediaRecord = {
      id: 42,
      rootId: 3,
      folderId: 7,
      name: "target.jpg",
      ext: "jpg",
      size: 1_024,
      mtime: 100,
      width: 1_920,
      height: 1_080
    };
    const mediaSlots = new Map<number, MediaRecord>([[targetIndex, target]]);

    for (const layoutMode of ["adaptive", "waterfall"] as const) {
      const geometry = buildLayoutGeometry({
        gap: 8,
        items: [],
        labelHeight: 36,
        layoutMode,
        viewportWidth: 960,
        tileMinWidth: 180,
        totalCount: 120,
        getItemAt: (index) => mediaSlots.get(index),
        estimatedRatio: 1
      });
      const resolved = resolveJustifiedMediaLocateTarget(geometry, mediaSlots, target.id);

      assert.equal(resolved?.placementIndex, targetIndex);
      assert.equal(resolved?.placement.item?.id, target.id);
      assert.equal(
        geometry.segments[resolved!.segmentIndex]?.itemIndexes.includes(targetIndex),
        true
      );
    }
  });

  it("keeps downstream justified geometry stable when an extreme aspect ratio arrives", () => {
    const targetIndex = 2;
    const extreme: MediaRecord = {
      id: 42,
      rootId: 3,
      folderId: 7,
      name: "panorama.jpg",
      ext: "jpg",
      size: 1_024,
      mtime: 100,
      width: 12_000,
      height: 300
    };
    const stableRatioContract = {
      getItemAspectRatio: () => 1
    } as Record<string, unknown>;

    for (const layoutMode of ["adaptive", "waterfall"] as const) {
      const baseline = buildLayoutGeometry({
        gap: 8,
        items: [],
        labelHeight: 36,
        layoutMode,
        viewportWidth: 960,
        tileMinWidth: 180,
        totalCount: 40,
        getItemAt: () => undefined,
        estimatedRatio: 1,
        ...stableRatioContract
      });
      const loaded = buildLayoutGeometry({
        gap: 8,
        items: [],
        labelHeight: 36,
        layoutMode,
        viewportWidth: 960,
        tileMinWidth: 180,
        totalCount: 40,
        getItemAt: (index) => index === targetIndex ? extreme : undefined,
        estimatedRatio: 1,
        ...stableRatioContract
      });

      assert.deepEqual(
        placementGeometry(loaded.placements[targetIndex]),
        placementGeometry(baseline.placements[targetIndex]),
        `${layoutMode} must not resize the arriving tile's stable slot`
      );
      assert.deepEqual(
        placementGeometry(loaded.placements[20]),
        placementGeometry(baseline.placements[20]),
        `${layoutMode} must not repack downstream placeholders`
      );
      assert.equal(loaded.placements[targetIndex]?.item?.id, extreme.id);
    }
  });

  it("closes preview before navigation and returns an explicit grid locate request", async () => {
    let previewOpen = true;
    let previewOpened = false;
    const result = await prepareMediaFocusNavigation(request, {
      closePreview: () => { previewOpen = false; },
      locateMedia: async () => {
        assert.equal(previewOpen, false);
        return true;
      }
    });

    assert.deepEqual(result, { requestId: "focus-1", mediaId: 42 });
    assert.equal(previewOpen, false);
    assert.equal(previewOpened, false);
  });

  it("returns no scroll request after failure and allows the same media to retry", async () => {
    let attempts = 0;
    const dependencies = {
      closePreview: () => undefined,
      locateMedia: async () => ++attempts > 1
    };

    assert.equal(await prepareMediaFocusNavigation(request, dependencies), null);
    assert.deepEqual(await prepareMediaFocusNavigation(
      { ...request, requestId: "focus-2" },
      dependencies
    ), { requestId: "focus-2", mediaId: 42 });
  });
});

function placementGeometry(placement: {
  left: number;
  top: number;
  width: number;
  height: number;
  thumbWidth: number;
  thumbHeight: number;
} | undefined) {
  assert.ok(placement);
  return {
    left: placement.left,
    top: placement.top,
    width: placement.width,
    height: placement.height,
    thumbWidth: placement.thumbWidth,
    thumbHeight: placement.thumbHeight
  };
}
