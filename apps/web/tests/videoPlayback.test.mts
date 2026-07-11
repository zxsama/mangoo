import { strict as assert } from "node:assert";
import { describe, test } from "node:test";

import {
  attemptVideoAutoplay,
  clampSeekTime,
  formatMediaTime,
  normalizeVolume,
  playbackProgress
} from "../src/features/preview/videoPlayback.ts";

describe("video playback helpers", () => {
  test("formats safe media times", () => {
    assert.equal(formatMediaTime(Number.NaN), "0:00");
    assert.equal(formatMediaTime(-3), "0:00");
    assert.equal(formatMediaTime(65), "1:05");
    assert.equal(formatMediaTime(3661), "1:01:01");
  });

  test("clamps seek, progress, and volume values", () => {
    assert.equal(clampSeekTime(-2, 10), 0);
    assert.equal(clampSeekTime(12, 10), 10);
    assert.equal(playbackProgress(5, 10), 0.5);
    assert.equal(playbackProgress(5, 0), 0);
    assert.equal(normalizeVolume(-1), 0);
    assert.equal(normalizeVolume(2), 1);
  });

  test("autoplays with sound when permitted", async () => {
    const video = { muted: true, play: async () => undefined };
    assert.equal(await attemptVideoAutoplay(video), "playing");
    assert.equal(video.muted, false);
  });

  test("retries muted only after a policy rejection", async () => {
    let attempts = 0;
    const video = {
      muted: false,
      play: async () => {
        attempts += 1;
        if (attempts === 1) throw { name: "NotAllowedError" };
      }
    };
    assert.equal(await attemptVideoAutoplay(video), "playing-muted");
    assert.equal(video.muted, true);
    assert.equal(attempts, 2);
  });

  test("does not retry non-policy playback errors", async () => {
    let attempts = 0;
    const video = {
      muted: false,
      play: async () => {
        attempts += 1;
        throw { name: "NotSupportedError" };
      }
    };
    assert.equal(await attemptVideoAutoplay(video), "failed");
    assert.equal(attempts, 1);
  });

  test("reports blocked when both policy-gated attempts are rejected", async () => {
    let attempts = 0;
    const video = {
      muted: false,
      play: async () => {
        attempts += 1;
        throw { name: "NotAllowedError" };
      }
    };
    assert.equal(await attemptVideoAutoplay(video), "blocked");
    assert.equal(video.muted, true);
    assert.equal(attempts, 2);
  });
});
