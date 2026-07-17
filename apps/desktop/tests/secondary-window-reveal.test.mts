import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  NATIVE_WINDOW_WARMUP_MS,
  createSecondaryWindowRevealState,
  revealSecondaryNativeWindow,
  waitForSecondaryNativeWindowWarmup,
  type SecondaryNativeWindowHost
} from "../src/secondary-window-reveal.ts";

type RevealResult = false | {
  fallback: boolean;
};

function fakeNativeWindow(calls: string[]): SecondaryNativeWindowHost {
  return {
    setOpacity(opacity) {
      calls.push(`opacity:${opacity}`);
    },
    focus() {
      calls.push("focus");
    }
  };
}

describe("secondary native window reveal state", () => {
  it("fallback reveals an opacity-zero window and ignores late warm-up completion", async () => {
    const state = createSecondaryWindowRevealState<RevealResult>();
    state.nativeReady = true;
    state.rendererReady = true;
    const token = state.beginWarmup();
    assert.equal(typeof token, "number");

    let finishFrame!: () => void;
    const compositionFrame = new Promise<void>((resolve) => {
      finishFrame = resolve;
    });
    const warmup = waitForSecondaryNativeWindowWarmup(
      state,
      token as number,
      compositionFrame
    );
    const readyIpc = state.result;
    const calls: string[] = [];
    const fallback = { fallback: true };

    assert.deepEqual(calls, [], "normal warm-up must not reveal the native window early");
    assert.equal(revealSecondaryNativeWindow(state, fallback, fakeNativeWindow(calls)), true);
    assert.deepEqual(calls, ["opacity:1", "focus"]);
    assert.deepEqual(await readyIpc, fallback);

    finishFrame();
    assert.equal(await warmup, false);
    assert.equal(state.warmupInFlight, false);
    assert.equal(
      revealSecondaryNativeWindow(state, { fallback: false }, fakeNativeWindow(calls)),
      false
    );
    assert.deepEqual(calls, ["opacity:1", "focus"]);
    assert.deepEqual(state.value, fallback);
  });

  it("close resolves ready IPC false without revealing the native window", async () => {
    const state = createSecondaryWindowRevealState<RevealResult>();
    state.nativeReady = true;
    state.rendererReady = true;
    const token = state.beginWarmup();
    assert.equal(typeof token, "number");

    const warmup = waitForSecondaryNativeWindowWarmup(
      state,
      token as number,
      new Promise<void>(() => {})
    );
    const calls: string[] = [];

    assert.equal(state.settle(false), true);
    assert.equal(await state.result, false);
    assert.equal(await warmup, false);
    assert.equal(state.warmupInFlight, false);
    assert.deepEqual(calls, []);
    assert.equal(
      revealSecondaryNativeWindow(state, { fallback: true }, fakeNativeWindow(calls)),
      false
    );
    assert.deepEqual(calls, []);
  });

  it("reveals once only after the native opacity-zero warm-up", async () => {
    const state = createSecondaryWindowRevealState<RevealResult>();
    state.nativeReady = true;
    state.rendererReady = true;
    const token = state.beginWarmup();
    assert.equal(typeof token, "number");

    const calls: string[] = [];
    let readySettled = false;
    void state.result.then(() => {
      readySettled = true;
    });
    const warmupStartedAt = Date.now();
    const didWarm = await waitForSecondaryNativeWindowWarmup(
      state,
      token as number,
      Promise.resolve()
    );
    const warmupCompletedAt = Date.now();

    assert.equal(didWarm, true);
    assert.equal(readySettled, false);
    assert.deepEqual(calls, []);
    assert.ok(warmupCompletedAt - warmupStartedAt >= NATIVE_WINDOW_WARMUP_MS);

    const normal = { fallback: false };
    assert.equal(revealSecondaryNativeWindow(state, normal, fakeNativeWindow(calls)), true);
    assert.deepEqual(calls, ["opacity:1", "focus"]);
    assert.deepEqual(await state.result, normal);
    assert.equal(
      revealSecondaryNativeWindow(state, { fallback: true }, fakeNativeWindow(calls)),
      false
    );
    assert.deepEqual(calls, ["opacity:1", "focus"]);
  });
});
