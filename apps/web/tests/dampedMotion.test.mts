import { strict as assert } from "node:assert";
import { describe, test } from "node:test";

import { DampedMotionAxis, normalizeWheelDelta } from "../src/core/dampedMotion.ts";

describe("damped wheel motion", () => {
  test("spreads one wheel delta across decelerating frames", () => {
    const axis = new DampedMotionAxis({ friction: 0.84 });
    axis.push(100);

    const first = axis.step();
    const second = axis.step();
    const third = axis.step();

    assert.ok(first > 0 && first < 100, `first frame should be eased, got ${first}`);
    assert.ok(second > 0 && second < first, `second frame should decelerate, got ${second}`);
    assert.ok(third > 0 && third < second, `third frame should decelerate, got ${third}`);

    let total = first + second + third;
    let guard = 0;
    while (axis.active && guard < 120) {
      total += axis.step();
      guard += 1;
    }

    assert.ok(Math.abs(total - 100) < 1, `total motion should preserve wheel distance, got ${total}`);
  });

  test("accelerates when wheel input arrives during decay", () => {
    const freeDecay = new DampedMotionAxis({ friction: 0.84 });
    freeDecay.push(100);
    freeDecay.step();
    const decayedSecondFrame = freeDecay.step();

    const accelerated = new DampedMotionAxis({ friction: 0.84 });
    accelerated.push(100);
    accelerated.step();
    accelerated.push(100);
    const acceleratedSecondFrame = accelerated.step();

    assert.ok(
      acceleratedSecondFrame > decayedSecondFrame,
      `new input should accelerate ${acceleratedSecondFrame} > ${decayedSecondFrame}`
    );
  });

  test("can make large wheel deltas respond with stronger initial acceleration", () => {
    const slow = new DampedMotionAxis({
      acceleration: 0.5,
      accelerationDelta: 400,
      friction: 0.84
    });
    const fast = new DampedMotionAxis({
      acceleration: 0.5,
      accelerationDelta: 400,
      friction: 0.84
    });

    slow.push(80);
    fast.push(400);

    const slowFrameRatio = slow.step() / 80;
    const fastFrameRatio = fast.step() / 400;

    assert.ok(
      fastFrameRatio > slowFrameRatio,
      `fast input should ramp harder ${fastFrameRatio} > ${slowFrameRatio}`
    );
  });

  test("normalizes wheel line and page units to pixels", () => {
    assert.equal(normalizeWheelDelta(2, 0, 16, 800), 2);
    assert.equal(normalizeWheelDelta(2, 1, 16, 800), 32);
    assert.equal(normalizeWheelDelta(2, 2, 16, 800), 1600);
  });
});
