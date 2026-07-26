import assert from "node:assert/strict";
import { describe, it } from "node:test";

interface OverlayPresenceApi {
  createOverlayPresenceState(open: boolean, reducedMotion: boolean): {
    present: boolean;
    phase: string;
  };
  overlayMotionDuration(kind: "root" | "nested" | "modal"): number;
  reduceOverlayPresence(
    state: { present: boolean; phase: string },
    action: { type: string; open?: boolean; reducedMotion?: boolean }
  ): { present: boolean; phase: string };
}

const loadPresence = async (): Promise<OverlayPresenceApi> => {
  try {
    return await import("../src/design/overlays/overlayPresence.ts") as OverlayPresenceApi;
  } catch (error) {
    assert.fail(`overlay presence seam is unavailable: ${String(error)}`);
  }
};

describe("overlay presence lifecycle", () => {
  it("keeps a surface present through opening and closing", async () => {
    const presence = await loadPresence();
    let state = presence.createOverlayPresenceState(false, false);
    state = presence.reduceOverlayPresence(state, {
      type: "sync",
      open: true,
      reducedMotion: false
    });
    assert.deepEqual(state, { present: true, phase: "opening" });
    state = presence.reduceOverlayPresence(state, { type: "entered" });
    assert.deepEqual(state, { present: true, phase: "open" });
    state = presence.reduceOverlayPresence(state, {
      type: "sync",
      open: false,
      reducedMotion: false
    });
    assert.deepEqual(state, { present: true, phase: "closing" });
    state = presence.reduceOverlayPresence(state, { type: "exited" });
    assert.deepEqual(state, { present: false, phase: "closed" });
  });

  it("reopens a closing surface and skips transitions for reduced motion", async () => {
    const presence = await loadPresence();
    const reopened = presence.reduceOverlayPresence(
      { present: true, phase: "closing" },
      { type: "sync", open: true, reducedMotion: false }
    );
    assert.deepEqual(reopened, { present: true, phase: "opening" });
    const reduced = presence.reduceOverlayPresence(reopened, {
      type: "sync",
      open: false,
      reducedMotion: true
    });
    assert.deepEqual(reduced, { present: false, phase: "closed" });
  });

  it("uses the approved root, nested, and modal timing", async () => {
    const presence = await loadPresence();
    assert.equal(presence.overlayMotionDuration("root"), 140);
    assert.equal(presence.overlayMotionDuration("nested"), 110);
    assert.equal(presence.overlayMotionDuration("modal"), 160);
  });
});
