import assert from "node:assert/strict";
import { describe, it } from "node:test";
import * as overlayPosition from "../src/design/overlays/overlayPosition.ts";

const {
  pointOverlayAnchor,
  resolveOverlayPosition
} = overlayPosition;
const positioningApi = overlayPosition as typeof overlayPosition & {
  clampOverlayWidth: (requestedWidth: number | undefined, maxWidth: number) => number | undefined;
  resolveOverlayOffset: (offset: number | undefined, nested: boolean) => number;
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

  it("clamps requested width so a large minimum cannot breach the viewport gutter", () => {
    assert.equal(typeof positioningApi.clampOverlayWidth, "function");
    assert.equal(positioningApi.clampOverlayWidth(500, 296), 296);
    assert.equal(positioningApi.clampOverlayWidth(240, 296), 240);
    assert.equal(positioningApi.clampOverlayWidth(undefined, 296), undefined);
  });

  it("keeps button defaults while allowing an explicit zero point offset", () => {
    assert.equal(typeof positioningApi.resolveOverlayOffset, "function");
    assert.equal(positioningApi.resolveOverlayOffset(undefined, false), 6);
    assert.equal(positioningApi.resolveOverlayOffset(undefined, true), 4);
    assert.equal(positioningApi.resolveOverlayOffset(0, false), 0);

    const result = resolveOverlayPosition({
      trigger: pointOverlayAnchor(44, 66).getBoundingClientRect(),
      overlay: { width: 120, height: 80 },
      viewport: { width: 800, height: 600 },
      preferredPlacement: "bottom-start",
      offset: positioningApi.resolveOverlayOffset(0, false),
      gutter: 12
    });
    assert.equal(result.left, 44);
    assert.equal(result.top, 66);
  });
});
