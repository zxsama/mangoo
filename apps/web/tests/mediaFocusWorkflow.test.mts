import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { MediaRecord } from "@megle/core-client";
import {
  prepareMediaFocusNavigation,
  resolveJustifiedMediaLocateTarget,
  resolveMediaLocateVirtualIndex
} from "../src/app/mediaFocusWorkflow.ts";
import { runCancellableMediaFocusLocate } from "../src/core/mediaFocusLocateWorkflow.ts";
import * as mediaFocusLocateWorkflow from "../src/core/mediaFocusLocateWorkflow.ts";
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

  it("does not emit a grid scroll request when cancellation wins a late locate", async () => {
    const controller = new AbortController();
    let resolveLocate: (value: boolean) => void = () => undefined;
    const locate = new Promise<boolean>((resolve) => { resolveLocate = resolve; });
    let previewOpen = true;
    const result = prepareMediaFocusNavigation(request, {
      closePreview: () => { previewOpen = false; },
      locateMedia: async () => locate
    }, controller.signal);

    controller.abort();
    resolveLocate(true);
    assert.equal(await result, null);
    assert.equal(previewOpen, false);
  });

  it("stops late locate work after cancellation and lets a new generation retry", async () => {
    const controller = new AbortController();
    let releaseLoad = () => undefined;
    const loading = new Promise<void>((resolve) => { releaseLoad = resolve; });
    let markLoadStarted = () => undefined;
    const loadStarted = new Promise<void>((resolve) => { markLoadStarted = resolve; });
    const calls: string[] = [];
    const stages = {
      resolveTarget: async () => { calls.push("target"); return true; },
      resolvePosition: async () => { calls.push("position"); return 900_004; },
      resetFilters: () => { calls.push("filters"); },
      loadLibrary: async () => { calls.push("load"); markLoadStarted(); await loading; },
      revealFolderPath: async () => { calls.push("reveal"); return true; },
      loadMediaWindow: async () => { calls.push("window"); },
      selectMedia: () => { calls.push("select"); }
    };
    const first = runCancellableMediaFocusLocate(stages, controller.signal);
    await loadStarted;
    controller.abort();
    releaseLoad();

    assert.equal(await first, false);
    assert.deepEqual(calls, ["target", "position", "filters", "load"]);

    calls.length = 0;
    assert.equal(await runCancellableMediaFocusLocate(stages, new AbortController().signal), true);
    assert.deepEqual(calls, ["target", "position", "filters", "load", "reveal", "window", "select"]);
  });

  it("does not reset filters when cancellation wins the position lookup race", async () => {
    const controller = new AbortController();
    const calls: string[] = [];
    const result = runCancellableMediaFocusLocate({
      resolveTarget: async () => true,
      resolvePosition: async () => {
        controller.abort();
        return 12;
      },
      resetFilters: () => { calls.push("filters"); },
      loadLibrary: async () => undefined,
      revealFolderPath: async () => true,
      loadMediaWindow: async () => undefined,
      selectMedia: () => { calls.push("select"); }
    }, controller.signal);

    assert.equal(await result, false);
    assert.deepEqual(calls, []);
  });

  it("enters switching before restart and never continues late after cancellation", async () => {
    const runSwitch = Reflect.get(mediaFocusLocateWorkflow, "runCoordinatedMediaFocusSwitch");
    assert.equal(typeof runSwitch, "function", "the renderer must coordinate irreversible switches");
    const controller = new AbortController();
    const calls: string[] = [];
    let releaseSwitch = () => undefined;
    const switching = new Promise<{ ok: boolean }>((resolve) => {
      releaseSwitch = () => resolve({ ok: true });
    });
    const result = runSwitch({
      libraryId: "library-b",
      requestId: "focus-1",
      signal: controller.signal,
      switchLibrary: async () => {
        calls.push("switch");
        return switching;
      },
      updateStage: async (update: { stage: string }) => {
        calls.push(`stage:${update.stage}`);
        return true;
      }
    });
    await Promise.resolve();
    assert.deepEqual(calls, ["stage:switching", "switch"]);

    controller.abort();
    releaseSwitch();
    assert.equal(await result, false);

    calls.length = 0;
    assert.equal(await runSwitch({
      libraryId: "library-c",
      requestId: "focus-2",
      signal: new AbortController().signal,
      switchLibrary: async () => {
        calls.push("switch");
        return { ok: true };
      },
      updateStage: async () => false
    }), false);
    assert.deepEqual(calls, [], "an obsolete request must not begin a restart");
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
