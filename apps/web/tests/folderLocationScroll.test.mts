import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { resolveFolderLocationScrollFrame } from "../src/features/library/folderLocationScroll.ts";

describe("folder location scroll animation", () => {
  it("does not complete before the target scroll duration", () => {
    assert.deepEqual(
      resolveFolderLocationScrollFrame({
        from: 20,
        to: 220,
        elapsedMs: 80,
        durationMs: 200
      }),
      { done: false, top: 176.8 }
    );
  });

  it("finishes at the exact target before the caller starts highlighting", () => {
    assert.deepEqual(
      resolveFolderLocationScrollFrame({
        from: 20,
        to: 220,
        elapsedMs: 200,
        durationMs: 200
      }),
      { done: true, top: 220 }
    );
  });
});
