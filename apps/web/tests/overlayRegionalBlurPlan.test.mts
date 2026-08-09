import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  buildOverlayRegionalBlurPlans,
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

  it("clamps a clipped rounded mask to half its smallest dimension", () => {
    const [plan] = buildOverlayRegionalBlurPlans({
      sigma: 4,
      targets: [{ id: "small", layerIndex: -1, rect: rect(0, 0, 20, 20) }],
      layers: [{ id: "large-radius", kind: "root", radius: 100, rect: rect(10, 10, 30, 30) }]
    });
    assert.deepEqual(plan.masks, [
      { left: 10, top: 10, width: 10, height: 10, radius: 5 }
    ]);
  });
});
