import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  buildOverlayRegionalBlurPlans,
  buildOverlayRegionalMaskSvg,
  type OverlayRegionalRect
} from "../src/design/overlays/overlayRegionalBlurPlan.ts";

const rect = (left: number, top: number, width: number, height: number): OverlayRegionalRect => ({
  left,
  top,
  right: left + width,
  bottom: top + height,
  width,
  height
});

describe("overlay regional blur planning", () => {
  it("expands a root popup mask by three Gaussian sigmas", () => {
    assert.deepEqual(buildOverlayRegionalBlurPlans({
      sigma: 34,
      targets: [{ id: "app-root", layerIndex: -1, rect: rect(0, 0, 1000, 700) }],
      layers: [{ id: "root", kind: "root", radius: 12, rect: rect(200, 100, 300, 200) }]
    }), [{
      targetId: "app-root",
      targetRect: rect(0, 0, 1000, 700),
      masks: [{ left: 200, top: 100, width: 300, height: 200, radius: 12 }],
      blurBounds: { left: 98, top: 0, width: 504, height: 402 }
    }]);
  });

  it("adds every popup to the app root but only the higher intersection to a lower surface", () => {
    const appRoot = { id: "app-root", layerIndex: -1, rect: rect(0, 0, 1000, 700) };
    const rootSurface = { id: "root", layerIndex: 0, rect: rect(200, 100, 300, 200) };

    assert.deepEqual(buildOverlayRegionalBlurPlans({
      sigma: 34,
      targets: [appRoot, rootSurface],
      layers: [
        { id: "root", kind: "root", radius: 12, rect: rootSurface.rect },
        { id: "nested", kind: "nested", radius: 16, rect: rect(460, 140, 180, 160) }
      ]
    }), [
      {
        targetId: "app-root",
        targetRect: appRoot.rect,
        masks: [
          { left: 200, top: 100, width: 300, height: 200, radius: 12 },
          { left: 460, top: 140, width: 180, height: 160, radius: 16 }
        ],
        blurBounds: { left: 98, top: 0, width: 644, height: 402 }
      },
      {
        targetId: "root",
        targetRect: rootSurface.rect,
        masks: [{ left: 260, top: 40, width: 40, height: 160, radius: 16 }],
        blurBounds: { left: 158, top: 0, width: 142, height: 200 }
      }
    ]);
  });

  it("turns a modal into one full-target mask for every lower render plane", () => {
    assert.deepEqual(buildOverlayRegionalBlurPlans({
      sigma: 34,
      targets: [
        { id: "app-root", layerIndex: -1, rect: rect(0, 0, 1000, 700) },
        { id: "root", layerIndex: 0, rect: rect(200, 100, 300, 200) }
      ],
      layers: [
        { id: "root", kind: "root", radius: 12, rect: rect(200, 100, 300, 200) },
        { id: "modal", kind: "modal", radius: 8, rect: rect(360, 220, 280, 180) }
      ]
    }), [
      {
        targetId: "app-root",
        targetRect: rect(0, 0, 1000, 700),
        masks: [{ left: 0, top: 0, width: 1000, height: 700, radius: 0 }],
        blurBounds: { left: 0, top: 0, width: 1000, height: 700 }
      },
      {
        targetId: "root",
        targetRect: rect(200, 100, 300, 200),
        masks: [{ left: 0, top: 0, width: 300, height: 200, radius: 0 }],
        blurBounds: { left: 0, top: 0, width: 300, height: 200 }
      }
    ]);
  });

  it("omits zero blur and non-intersecting higher layers", () => {
    const input = {
      targets: [{ id: "app-root", layerIndex: -1, rect: rect(0, 0, 200, 100) }],
      layers: [{ id: "outside", kind: "root" as const, radius: 8, rect: rect(300, 200, 50, 50) }]
    };
    assert.deepEqual(buildOverlayRegionalBlurPlans({ ...input, sigma: 34 }), []);
    assert.deepEqual(buildOverlayRegionalBlurPlans({
      ...input,
      sigma: 0,
      layers: [{ id: "inside", kind: "root", radius: 8, rect: rect(20, 20, 50, 50) }]
    }), []);
  });

  it("serializes rounded masks independently from the filter implementation", () => {
    const markup = buildOverlayRegionalMaskSvg({
      targetId: "app-root",
      targetRect: rect(0, 0, 640, 480),
      masks: [
        { left: 12, top: 18, width: 240, height: 144, radius: 8 },
        { left: 300, top: 210, width: 180, height: 120, radius: 16 }
      ],
      blurBounds: { left: 0, top: 0, width: 640, height: 480 }
    });

    assert.match(markup, /^<svg xmlns="http:\/\/www\.w3\.org\/2000\/svg" viewBox="0 0 640 480">/);
    assert.match(markup, /<rect x="12" y="18" width="240" height="144" rx="8" ry="8" fill="white"\/>/);
    assert.match(markup, /<rect x="300" y="210" width="180" height="120" rx="16" ry="16" fill="white"\/>/);
    assert.equal((markup.match(/<rect /g) ?? []).length, 2);
  });
});
