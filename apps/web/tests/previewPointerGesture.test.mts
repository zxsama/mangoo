import { strict as assert } from "node:assert";
import { describe, test } from "node:test";

import {
  classifyVideoPointerGesture,
  shouldStartVideoDrag
} from "../src/features/preview/previewPointerGesture.ts";

describe("video preview pointer gestures", () => {
  test("classifies a short stationary press as a tap", () => {
    assert.equal(classifyVideoPointerGesture({ dragging: false, elapsedMs: 120, distancePx: 3 }), "tap");
  });

  test("starts dragging only after hold and movement thresholds", () => {
    assert.equal(shouldStartVideoDrag(199, 8), false);
    assert.equal(shouldStartVideoDrag(220, 5), false);
    assert.equal(shouldStartVideoDrag(220, 6), true);
    assert.equal(classifyVideoPointerGesture({ dragging: true, elapsedMs: 260, distancePx: 18 }), "drag");
  });

  test("does not turn a long hold or quick slip into playback", () => {
    assert.equal(classifyVideoPointerGesture({ dragging: false, elapsedMs: 240, distancePx: 0 }), "hold");
    assert.equal(classifyVideoPointerGesture({ dragging: false, elapsedMs: 120, distancePx: 8 }), "slip");
  });
});
