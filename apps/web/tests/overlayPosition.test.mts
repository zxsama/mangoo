import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  pointOverlayAnchor,
  resolveOverlayPosition
} from "../src/design/overlays/overlayPosition.ts";

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
});
