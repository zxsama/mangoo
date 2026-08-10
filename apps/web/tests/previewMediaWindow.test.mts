import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  previewLandingWindow,
  resolvePendingPreviewSelection,
  resolvePreviewMediaWindow
} from "../src/app/previewMediaWindow.ts";

describe("preview media window", () => {
  it("keeps the selected absolute slot index after ordering a far loaded window", () => {
    const window = resolvePreviewMediaWindow(
      new Map([
        [126, { id: 43 }],
        [124, { id: 41 }],
        [125, { id: 42 }]
      ]),
      42
    );

    assert.deepEqual(window.loadedMedia.map((item) => item.id), [41, 42, 43]);
    assert.equal(window.selectedLoadedIndex, 1);
    assert.equal(window.selectedAbsoluteIndex, 125);
  });

  it("does not substitute a second list when no media slot is loaded", () => {
    assert.deepEqual(resolvePreviewMediaWindow(new Map(), 42), {
      loadedMedia: [],
      selectedAbsoluteIndex: -1,
      selectedLoadedIndex: -1
    });
  });

  it("lands with neighbours while clamping to the folder bounds", () => {
    assert.deepEqual(previewLandingWindow(125, 220), [101, 150]);
    assert.deepEqual(previewLandingWindow(0, 220), [0, 25]);
    assert.deepEqual(previewLandingWindow(219, 220), [195, 220]);
  });

  it("discards a pending selection after preview closes or media context changes", () => {
    const pending = { index: 125, mediaGeneration: 7 };
    const mediaSlots = new Map([[125, { id: 42 }]]);

    assert.deepEqual(
      resolvePendingPreviewSelection({
        pending,
        previewOpen: false,
        mediaGeneration: 7,
        mediaSlots
      }),
      { status: "discard" }
    );
    assert.deepEqual(
      resolvePendingPreviewSelection({
        pending,
        previewOpen: true,
        mediaGeneration: 8,
        mediaSlots
      }),
      { status: "discard" }
    );
  });

  it("waits for the matching generation and applies only the arrived slot", () => {
    const pending = { index: 125, mediaGeneration: 7 };

    assert.deepEqual(
      resolvePendingPreviewSelection({
        pending,
        previewOpen: true,
        mediaGeneration: 7,
        mediaSlots: new Map()
      }),
      { status: "wait" }
    );
    assert.deepEqual(
      resolvePendingPreviewSelection({
        pending,
        previewOpen: true,
        mediaGeneration: 7,
        mediaSlots: new Map([[125, { id: 42 }]])
      }),
      { status: "apply", media: { id: 42 } }
    );
  });
});
