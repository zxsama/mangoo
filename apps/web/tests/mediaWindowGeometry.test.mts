import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  buildWindowedMediaGeometry,
  resolveMediaGeometryStrategy
} from "../src/features/media-grid/windowedMediaGeometry.ts";

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
});
