import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { describe, it } from "node:test";

import {
  buildWindowedMediaGeometry,
  resolveMediaGeometryStrategy,
  windowedPhysicalScrollTopForLogicalMediaScroll,
  WINDOWED_MEDIA_PHYSICAL_SPACER_LIMIT_PX
} from "../src/features/media-grid/windowedMediaGeometry.ts";
import * as windowedMediaGeometry from "../src/features/media-grid/windowedMediaGeometry.ts";

type WindowedVirtualizationPlanBuilder = (input: {
  geometry: ReturnType<typeof buildWindowedMediaGeometry>;
  mediaOffset: number;
  overscanSegments: number;
  prefixVirtualRowCount: number;
  scrollTop: number;
  viewportHeight: number;
}) => {
  logicalMediaScrollTop: number;
  mediaRenderOffset: number;
  mediaSegmentIndexes: number[];
  spacerSize: number;
  virtualizerCount: number;
};

const SAFE_BROWSER_SPACER_PX = WINDOWED_MEDIA_PHYSICAL_SPACER_LIMIT_PX;

function compressedScrollTopForLogicalPosition(input: {
  logicalMediaScrollTop: number;
  mediaOffset: number;
  logicalTotalSize: number;
  viewportHeight: number;
}) {
  const physicalMediaExtent = Math.min(
    input.logicalTotalSize,
    SAFE_BROWSER_SPACER_PX - input.mediaOffset
  );
  const physicalRange = Math.max(0, physicalMediaExtent - input.viewportHeight);
  const logicalRange = Math.max(0, input.logicalTotalSize - input.viewportHeight);
  const progress = logicalRange === 0
    ? 0
    : Math.max(0, Math.min(1, input.logicalMediaScrollTop / logicalRange));
  return input.mediaOffset + progress * physicalRange;
}

function virtualizationPlanBuilder(): WindowedVirtualizationPlanBuilder {
  const builder = Reflect.get(windowedMediaGeometry, "buildWindowedMediaVirtualizationPlan");
  assert.equal(typeof builder, "function", "the production windowed virtualization plan must exist");
  return builder as WindowedVirtualizationPlanBuilder;
}

describe("windowed adaptive and waterfall geometry", () => {
  it("selects the bounded production branch above the justified full-total cap", () => {
    for (const layoutMode of ["adaptive", "waterfall"] as const) {
      assert.deepEqual(resolveMediaGeometryStrategy({
        hasMediaSlots: true,
        itemCount: 20_001,
        layoutMode,
        loadedItemCount: 96
      }), {
        justifiedFullTotal: false,
        windowedMedia: true
      });
      assert.deepEqual(resolveMediaGeometryStrategy({
        hasMediaSlots: true,
        itemCount: 20_000,
        layoutMode,
        loadedItemCount: 96
      }), {
        justifiedFullTotal: true,
        windowedMedia: false
      });
    }
  });

  it("preserves justified layout semantics for a million items without materializing them", () => {
    for (const layoutMode of ["adaptive", "waterfall"] as const) {
      const startedAt = performance.now();
      const geometry = buildWindowedMediaGeometry({
        count: 1_000_000,
        gap: 8,
        labelHeight: 36,
        layoutMode,
        tileMinWidth: 180,
        viewportWidth: 960
      });
      const buildDurationMs = performance.now() - startedAt;
      const first = geometry.resolvePlacement(0);
      const second = geometry.resolvePlacement(1);
      const deep = geometry.resolvePlacement(900_004);

      assert.ok(first && second && deep);
      assert.equal(geometry.layoutMode, layoutMode);
      assert.ok(geometry.segmentCount > 0);
      assert.ok(buildDurationMs < 50, `${layoutMode} geometry took ${buildDurationMs}ms`);
      assert.equal(
        Object.values(geometry).some((value) => Array.isArray(value) && value.length > 1_000),
        false,
        "the million-item geometry must stay bounded"
      );
      if (layoutMode === "adaptive") {
        assert.notEqual(first.width, second.width, "adaptive rows must retain varied widths");
      } else {
        assert.notEqual(first.thumbHeight, second.thumbHeight, "waterfall tiles must retain varied heights");
      }
      assert.ok(deep.top > 0);
    }
  });

  it("resolves bounded visible segments and an exact deep target", () => {
    for (const layoutMode of ["adaptive", "waterfall"] as const) {
      const geometry = buildWindowedMediaGeometry({
        count: 1_000_000,
        gap: 8,
        labelHeight: 36,
        layoutMode,
        tileMinWidth: 180,
        viewportWidth: 960
      });
      const targetIndex = 900_004;
      const target = geometry.resolvePlacement(targetIndex);
      assert.ok(target);
      const segmentIndex = geometry.segmentIndexForItem(targetIndex);
      assert.ok(segmentIndex !== undefined);
      assert.ok(geometry.resolveSegment(segmentIndex)?.itemIndexes.includes(targetIndex));

      const visible = geometry.itemIndexesInRange(target.top, target.top + 800);
      assert.ok(visible.includes(targetIndex));
      assert.ok(visible.length < 200, "one viewport must not expand into an unbounded item set");
    }
  });

  it("keeps a million-item production virtual range bounded outside TanStack measurements", () => {
    const buildPlan = virtualizationPlanBuilder();
    for (const layoutMode of ["adaptive", "waterfall"] as const) {
      const geometry = buildWindowedMediaGeometry({
        count: 1_000_000,
        gap: 8,
        labelHeight: 36,
        layoutMode,
        tileMinWidth: 180,
        viewportWidth: 960
      });
      const deepPlacement = geometry.resolvePlacement(900_004);
      assert.ok(deepPlacement);

      const startedAt = performance.now();
      const viewportHeight = 800;
      const plan = buildPlan({
        geometry,
        mediaOffset: 72,
        overscanSegments: 10,
        prefixVirtualRowCount: 3,
        scrollTop: windowedPhysicalScrollTopForLogicalMediaScroll({
          geometry,
          logicalMediaScrollTop: deepPlacement.top,
          mediaOffset: 72,
          viewportHeight
        }),
        viewportHeight
      });
      const durationMs = performance.now() - startedAt;

      assert.equal(plan.virtualizerCount, 3, "TanStack may measure only the bounded prefix rows");
      assert.equal(plan.spacerSize, Math.min(72 + geometry.totalSize, SAFE_BROWSER_SPACER_PX));
      assert.ok(plan.mediaSegmentIndexes.length > 0);
      assert.ok(plan.mediaSegmentIndexes.length < 64, "rendered media segments must stay viewport-bounded");
      assert.ok(durationMs < 50, `${layoutMode} virtual range took ${durationMs}ms`);
    }
  });

  it("keeps the first, middle, and tail reachable below the browser layout-height ceiling", () => {
    const buildPlan = virtualizationPlanBuilder();
    for (const layoutMode of ["adaptive", "waterfall"] as const) {
      for (const viewport of [
        { height: 640, width: 760 },
        { height: 1080, width: 1000 }
      ]) {
        const geometry = buildWindowedMediaGeometry({
          count: 1_000_000,
          gap: 8,
          labelHeight: 36,
          layoutMode,
          tileMinWidth: 180,
          viewportWidth: viewport.width
        });
        assert.ok(
          geometry.totalSize > 33_554_428,
          `${layoutMode}/${viewport.width} must exercise Chromium's oversized-layout branch`
        );

        const mediaOffset = 72;
        for (const targetIndex of [0, 500_000, 999_999]) {
          const placement = geometry.resolvePlacement(targetIndex);
          const targetSegment = geometry.segmentIndexForItem(targetIndex);
          assert.ok(placement && targetSegment !== undefined);
          const logicalMediaScrollTop = Math.max(
            0,
            Math.min(
              geometry.totalSize - viewport.height,
              placement.top + placement.frameHeight / 2 - viewport.height / 2
            )
          );
          const physicalScrollTop = compressedScrollTopForLogicalPosition({
            logicalMediaScrollTop,
            logicalTotalSize: geometry.totalSize,
            mediaOffset,
            viewportHeight: viewport.height
          });
          const plan = buildPlan({
            geometry,
            mediaOffset,
            overscanSegments: 10,
            prefixVirtualRowCount: 3,
            scrollTop: physicalScrollTop,
            viewportHeight: viewport.height
          });

          assert.ok(plan.spacerSize <= SAFE_BROWSER_SPACER_PX);
          assert.ok(plan.mediaSegmentIndexes.includes(targetSegment));
          assert.ok(plan.mediaSegmentIndexes.length < 64, "the physical range must stay O(viewport)");
          assert.ok(Number.isFinite(plan.logicalMediaScrollTop));
          assert.ok(Number.isFinite(plan.mediaRenderOffset));
          const renderedTop = plan.mediaRenderOffset + placement.top;
          assert.ok(renderedTop + placement.height > physicalScrollTop);
          assert.ok(renderedTop < physicalScrollTop + viewport.height);
        }
      }
    }
  });

  it("recomputes a deep range when content width changes without changing columns", () => {
    const buildPlan = virtualizationPlanBuilder();
    for (const layoutMode of ["adaptive", "waterfall"] as const) {
      const geometry960 = buildWindowedMediaGeometry({
        count: 1_000_000,
        gap: 8,
        labelHeight: 36,
        layoutMode,
        tileMinWidth: 180,
        viewportWidth: 960
      });
      const geometry1000 = buildWindowedMediaGeometry({
        count: 1_000_000,
        gap: 8,
        labelHeight: 36,
        layoutMode,
        tileMinWidth: 180,
        viewportWidth: 1000
      });
      assert.equal(geometry960.columnCount, geometry1000.columnCount);
      assert.notEqual(geometry960.totalSize, geometry1000.totalSize);

      const deepStart = Math.floor(geometry1000.totalSize * 0.9);
      const viewportHeight = 900;
      const plan = buildPlan({
        geometry: geometry1000,
        mediaOffset: 64,
        overscanSegments: 8,
        prefixVirtualRowCount: 2,
        scrollTop: windowedPhysicalScrollTopForLogicalMediaScroll({
          geometry: geometry1000,
          logicalMediaScrollTop: deepStart,
          mediaOffset: 64,
          viewportHeight
        }),
        viewportHeight
      });
      assert.equal(plan.spacerSize, Math.min(64 + geometry1000.totalSize, SAFE_BROWSER_SPACER_PX));
      assert.ok(plan.mediaSegmentIndexes.length > 0);

      const first = geometry1000.resolveSegment(plan.mediaSegmentIndexes[0]!);
      const last = geometry1000.resolveSegment(plan.mediaSegmentIndexes.at(-1)!);
      assert.ok(first && last);
      assert.ok(first.start <= deepStart, "overscan must begin no later than the visible deep viewport");
      assert.ok(
        last.start + last.size >= deepStart + 900,
        "overscan must continue through the visible deep viewport"
      );
      for (let index = 1; index < plan.mediaSegmentIndexes.length; index += 1) {
        assert.equal(
          plan.mediaSegmentIndexes[index],
          plan.mediaSegmentIndexes[index - 1]! + 1,
          "resized deep ranges must not contain blank segment gaps"
        );
      }
    }
  });

  it("routes the production MediaGrid windowed branch around TanStack media measurements", async () => {
    const source = await readFile(
      new URL("../src/features/media-grid/MediaGrid.tsx", import.meta.url),
      "utf8"
    );
    assert.match(source, /buildWindowedMediaVirtualizationPlan/);
    assert.match(source, /count:\s*windowedMedia\s*\?\s*windowedVirtualization\.virtualizerCount/);
    assert.match(source, /windowedMedia\s*\?\s*windowedVirtualization\.mediaSegmentIndexes/);
    assert.match(source, /windowedMedia\s*\?\s*windowedVirtualization\.spacerSize/);
    assert.match(source, /top:\s*windowedVirtualization\.mediaRenderOffset\s*\+\s*placement\.top/);
    assert.match(source, /windowedPhysicalScrollTopForLogicalMediaScroll/);
  });
});
