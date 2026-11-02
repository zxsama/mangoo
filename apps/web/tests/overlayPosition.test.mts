import assert from "node:assert/strict";
import { describe, it } from "node:test";
import * as overlayPosition from "../src/design/overlays/overlayPosition.ts";

const {
  pointOverlayAnchor,
  resolveOverlayPosition
} = overlayPosition;
const positioningApi = overlayPosition as typeof overlayPosition & {
  resolveAnchoredOverlayLayout: (input: {
    trigger: ReturnType<typeof rect>;
    overlay: { measuredWidth: number; height: number };
    viewport: { width: number; height: number };
    preferredPlacement: "bottom-start";
    preferredWidth?: number;
    minWidth?: number;
    maxWidth?: number;
    offset: number;
    gutter: number;
  }) => {
    left: number;
    maxWidth: number;
    width: number;
  };
  resolveOverlayAnchorOffset: (
    anchor: { getBoundingClientRect: () => ReturnType<typeof rect> },
    nested: boolean
  ) => number;
  resolveOverlayWidth: (input: {
    measuredWidth: number;
    preferredWidth?: number;
    minWidth?: number;
    maxWidth?: number;
    viewportWidth: number;
    gutter: number;
  }) => number;
};

const rect = (left: number, top: number, width: number, height: number) => ({
  left,
  top,
  right: left + width,
  bottom: top + height,
  width,
  height
});

describe("resolveOverlayPosition", () => {
  it("joins the dropdown header to the trigger below and above without a gap", () => {
    for (const [top, placement, expectedTop] of [
      [50, "bottom-start", 50], [540, "top-start", 372]
    ] as const) {
      const result = overlayPosition.resolveAnchoredOverlayLayout({
        trigger: rect(100, top, 180, 32),
        overlay: { measuredWidth: 300, height: 200 },
        viewport: { width: 800, height: 600 },
        preferredPlacement: "bottom-start", integratedAnchor: true, offset: 6, gutter: 12
      });
      assert.equal(result.width, 180);
      assert.equal(result.anchorHeight, 32);
      assert.equal(result.placement, placement);
      assert.equal(result.top, expectedTop);
      if (placement === "top-start") assert.equal(result.top + 200, top + 32);
    }
  });

  it("budgets the integrated header inside the available height for long lists", () => {
    const result = overlayPosition.resolveAnchoredOverlayLayout({
      trigger: rect(280, 170, 90, 42),
      overlay: { measuredWidth: 300, height: 362 },
      viewport: { width: 320, height: 240 },
      preferredPlacement: "bottom-start", integratedAnchor: true, offset: 6, gutter: 12
    });
    assert.equal(result.top, 12);
    assert.equal(result.placement, "top-start");
    assert.equal(result.maxHeight, 200);
    assert.equal(result.top + result.maxHeight, 212);
    assert.equal(result.maxHeight - result.anchorHeight, 158);
    assert.equal(result.left + result.width, 308);
  });

  it("places a root popover below its trigger", () => {
    assert.deepEqual(resolveOverlayPosition({
      trigger: rect(100, 20, 28, 28),
      overlay: { width: 240, height: 180 },
      viewport: { width: 800, height: 600 },
      preferredPlacement: "bottom-start",
      offset: 6,
      gutter: 12
    }), {
      left: 100,
      top: 54,
      maxWidth: 776,
      maxHeight: 534,
      placement: "bottom-start"
    });
  });

  it("flips above when the bottom side cannot fit", () => {
    const result = resolveOverlayPosition({
      trigger: rect(400, 540, 28, 28),
      overlay: { width: 220, height: 180 },
      viewport: { width: 800, height: 600 },
      preferredPlacement: "bottom-end",
      offset: 6,
      gutter: 12
    });
    assert.equal(result.placement, "top-end");
    assert.equal(result.top, 354);
  });

  it("flips a nested menu from right to left", () => {
    const result = resolveOverlayPosition({
      trigger: rect(740, 120, 48, 32),
      overlay: { width: 190, height: 200 },
      viewport: { width: 800, height: 600 },
      preferredPlacement: "right-start",
      offset: 4,
      gutter: 12
    });
    assert.equal(result.placement, "left-start");
    assert.equal(result.left, 546);
  });

  it("shifts a side overlay vertically before shrinking it", () => {
    const result = resolveOverlayPosition({
      trigger: rect(100, 500, 40, 32),
      overlay: { width: 190, height: 200 },
      viewport: { width: 800, height: 600 },
      preferredPlacement: "right-start",
      offset: 4,
      gutter: 12
    });
    assert.equal(result.top, 388);
    assert.equal(result.maxHeight, 200);
  });

  it("clamps width and height to the viewport gutter", () => {
    const result = resolveOverlayPosition({
      trigger: rect(8, 8, 20, 20),
      overlay: { width: 500, height: 500 },
      viewport: { width: 320, height: 240 },
      preferredPlacement: "bottom-start",
      offset: 6,
      gutter: 12
    });
    assert.equal(result.left, 12);
    assert.equal(result.maxWidth, 296);
    assert.ok(result.maxHeight <= 206);
  });

  it("creates a zero-size virtual anchor for context menus", () => {
    assert.deepEqual(pointOverlayAnchor(44, 66).getBoundingClientRect(), rect(44, 66, 0, 0));
  });

  it("uses one effective width for constraints and geometry", () => {
    assert.equal(typeof positioningApi.resolveOverlayWidth, "function");
    assert.equal(positioningApi.resolveOverlayWidth({
      measuredWidth: 180,
      preferredWidth: 100,
      minWidth: 300,
      maxWidth: 500,
      viewportWidth: 320,
      gutter: 12
    }), 296);

    assert.equal(typeof positioningApi.resolveAnchoredOverlayLayout, "function");
    const layout = positioningApi.resolveAnchoredOverlayLayout({
      trigger: rect(300, 20, 20, 20),
      overlay: { measuredWidth: 180, height: 80 },
      viewport: { width: 320, height: 240 },
      preferredPlacement: "bottom-start",
      preferredWidth: 100,
      minWidth: 300,
      maxWidth: 500,
      offset: 6,
      gutter: 12
    });
    assert.equal(layout.width, 296);
    assert.equal(layout.maxWidth, 296);
    assert.equal(layout.left, 12);
  });

  it("infers point, root element, and nested offsets from anchor kind", () => {
    assert.equal(typeof positioningApi.resolveOverlayAnchorOffset, "function");
    const point = pointOverlayAnchor(44, 66);
    const elementAnchor = { getBoundingClientRect: () => rect(100, 20, 28, 28) };
    assert.equal(positioningApi.resolveOverlayAnchorOffset(point, false), 0);
    assert.equal(positioningApi.resolveOverlayAnchorOffset(elementAnchor, false), 6);
    assert.equal(positioningApi.resolveOverlayAnchorOffset(elementAnchor, true), 4);
    assert.equal(positioningApi.resolveOverlayAnchorOffset(point, true), 4);

    const result = resolveOverlayPosition({
      trigger: point.getBoundingClientRect(),
      overlay: { width: 120, height: 80 },
      viewport: { width: 800, height: 600 },
      preferredPlacement: "bottom-start",
      offset: positioningApi.resolveOverlayAnchorOffset(point, false),
      gutter: 12
    });
    assert.equal(result.left, 44);
    assert.equal(result.top, 66);
  });
});
