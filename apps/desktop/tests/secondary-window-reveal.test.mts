import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  MATERIAL_WARMUP_HOLD_MS,
  createSecondaryWindowRevealState,
  waitForSecondaryMaterialWarmup
} from "../src/secondary-window-reveal.ts";

type RevealResult = false | {
  fallback: boolean;
};

describe("secondary window reveal state", () => {
  it("unblocks ready IPC when fallback wins against a hanging compositor frame", async () => {
    const state = createSecondaryWindowRevealState<RevealResult>();
    state.nativeReady = true;
    state.rendererReady = true;
    const token = state.beginWarmup();
    assert.equal(typeof token, "number");

    const warmup = waitForSecondaryMaterialWarmup(
      state,
      token as number,
      new Promise<void>(() => {})
    );
    const readyIpc = state.result;
    const fallback = { fallback: true };

    assert.equal(state.settle(fallback), true);
    assert.deepEqual(await readyIpc, fallback);
    assert.equal(await warmup, false);
    assert.equal(state.warmupInFlight, false);
    assert.equal(state.settle({ fallback: false }), false);
    assert.deepEqual(state.value, fallback);
  });

  it("resolves ready IPC false and cancels warmup when the window closes", async () => {
    const state = createSecondaryWindowRevealState<RevealResult>();
    state.nativeReady = true;
    state.rendererReady = true;
    const token = state.beginWarmup();
    assert.equal(typeof token, "number");

    const warmup = waitForSecondaryMaterialWarmup(
      state,
      token as number,
      new Promise<void>(() => {})
    );

    assert.equal(state.settle(false), true);
    assert.equal(await state.result, false);
    assert.equal(await warmup, false);
    assert.equal(state.warmupInFlight, false);
    assert.equal(state.settle({ fallback: true }), false);
  });

  it("holds material-only visibility before the normal content reveal", async () => {
    const state = createSecondaryWindowRevealState<RevealResult>();
    state.nativeReady = true;
    state.rendererReady = true;
    const token = state.beginWarmup();
    assert.equal(typeof token, "number");

    const materialOnlyAt = Date.now();
    assert.equal(
      await waitForSecondaryMaterialWarmup(state, token as number, Promise.resolve()),
      true
    );
    const contentVisibleAt = Date.now();
    assert.ok(contentVisibleAt - materialOnlyAt >= MATERIAL_WARMUP_HOLD_MS);

    const normal = { fallback: false };
    assert.equal(state.settle(normal), true);
    assert.deepEqual(await state.result, normal);
    assert.equal(state.settle({ fallback: true }), false);
  });
});
