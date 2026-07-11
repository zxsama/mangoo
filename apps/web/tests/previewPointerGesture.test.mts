import { strict as assert } from "node:assert";
import { describe, test } from "node:test";

import {
  classifyVideoPointerGesture,
  pointerDisplacement
} from "../src/features/preview/previewPointerGesture.ts";

describe("video preview pointer gestures", () => {
  test("classifies stationary and small movements as taps", () => {
    assert.equal(classifyVideoPointerGesture({ distancePx: 0 }), "tap");
    assert.equal(classifyVideoPointerGesture({ distancePx: 5 }), "tap");
  });

  test("classifies movement beyond the click tolerance as a drag", () => {
    assert.equal(classifyVideoPointerGesture({ distancePx: 5.01 }), "drag");
    assert.equal(classifyVideoPointerGesture({ distancePx: 18 }), "drag");
  });

  test("calculates displacement from pointerdown without an activation delay", () => {
    assert.deepEqual(pointerDisplacement(120, 80, 151, 96), { x: 31, y: 16 });
    assert.deepEqual(pointerDisplacement(120, 80, 101, 62), { x: -19, y: -18 });
  });
});
