import assert from "node:assert/strict";
import { it } from "node:test";
import { LibraryLoadState } from "../src/library-load-state.ts";

it("retains a recovery error for a renderer that reloads after the broadcast", () => {
  const state = new LibraryLoadState();
  const recoveryError = {
    kind: "error" as const,
    message: "Library recovery failed; retry or switch library"
  };

  state.publish(recoveryError);

  assert.deepEqual(state.snapshot(), recoveryError);
});
