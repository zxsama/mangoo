import assert from "node:assert/strict";
import { test } from "node:test";
import { acquireVideoHover, stopVideoHover, horizontalHoverRatio, hoverSeekTime } from "../src/features/media-grid/videoHoverModel.ts";

test("horizontal hover ignores jitter/vertical motion and clamps edge seeks", () => {
  assert.equal(horizontalHoverRatio(20, 20, 10, 100), null);
  assert.equal(horizontalHoverRatio(20, 22, 10, 100), null);
  assert.equal(horizontalHoverRatio(20, 60, 10, 100), 0.5);
  assert.equal(horizontalHoverRatio(20, -20, 10, 100), 0);
  assert.equal(horizontalHoverRatio(20, 200, 10, 100), 1);
  assert.equal(horizontalHoverRatio(20, 60, 10, 0), null);
  assert.equal(horizontalHoverRatio(20, NaN, 10, 100), null);
  for (const duration of [0, -1, NaN, Infinity]) assert.equal(hoverSeekTime(0.5, duration), null);
  assert.equal(hoverSeekTime(NaN, 10), null);
  assert.equal(hoverSeekTime(0.5, 10), 5);
  assert.equal(hoverSeekTime(1, 10), 9.96);
});

test("acquiring another hover cancels the old owner and old disposal cannot release the new owner", () => {
  const stopped: string[] = [];
  const releaseA = acquireVideoHover(() => stopped.push("a"));
  const releaseB = acquireVideoHover(() => stopped.push("b"));
  assert.deepEqual(stopped, ["a"]);
  releaseA(); stopVideoHover();
  assert.deepEqual(stopped, ["a", "b"]);
  releaseB(); stopVideoHover();
  assert.deepEqual(stopped, ["a", "b"]);
});
