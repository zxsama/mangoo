import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { resolveAnchoredOverlayLayout } from "../src/design/overlays/overlayPosition.ts";

interface OverlayMeasurementApi {
  measureOverlayIntrinsicWidth(surface: {
    style: { width: string; maxWidth: string };
    getBoundingClientRect(): { width: number };
  }): number;
}

const loadMeasurement = async (): Promise<OverlayMeasurementApi> => {
  try {
    return await import("../src/design/overlays/overlayMeasurement.ts") as OverlayMeasurementApi;
  } catch (error) {
    assert.fail(`overlay measurement seam is unavailable: ${String(error)}`);
  }
};

const rect = (left: number, top: number, width: number, height: number) => ({
  left,
  top,
  right: left + width,
  bottom: top + height,
  width,
  height
});

describe("intrinsic overlay measurement", () => {
  it("re-expands a viewport-clamped overlay when the viewport widens", async () => {
    const measurement = await loadMeasurement();
    const style = { width: "296px", maxWidth: "296px" };
    const surface = {
      style,
      getBoundingClientRect: () => ({
        width: style.width === "max-content" && style.maxWidth === "none"
          ? 360
          : Number.parseFloat(style.width)
      })
    };

    const narrowIntrinsicWidth = measurement.measureOverlayIntrinsicWidth(surface);
    assert.equal(narrowIntrinsicWidth, 360);
    assert.deepEqual(style, { width: "296px", maxWidth: "296px" });
    const narrow = resolveAnchoredOverlayLayout({
      trigger: rect(100, 20, 28, 28),
      overlay: { measuredWidth: narrowIntrinsicWidth, height: 100 },
      viewport: { width: 320, height: 240 },
      preferredPlacement: "bottom-start",
      maxWidth: 296,
      offset: 6,
      gutter: 12
    });
    assert.equal(narrow.width, 296);

    const wideIntrinsicWidth = measurement.measureOverlayIntrinsicWidth(surface);
    assert.equal(wideIntrinsicWidth, 360);
    assert.deepEqual(style, { width: "296px", maxWidth: "296px" });
    const wide = resolveAnchoredOverlayLayout({
      trigger: rect(100, 20, 28, 28),
      overlay: { measuredWidth: wideIntrinsicWidth, height: 100 },
      viewport: { width: 800, height: 600 },
      preferredPlacement: "bottom-start",
      maxWidth: 776,
      offset: 6,
      gutter: 12
    });
    assert.equal(wide.width, 360);
  });
});
