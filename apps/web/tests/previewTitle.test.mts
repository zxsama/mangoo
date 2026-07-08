import { strict as assert } from "node:assert";
import { describe, test } from "node:test";

import {
  formatPreviewTitle,
  resolvePreviewTitlePosition
} from "../src/app/previewTitle.ts";

describe("preview title media position", () => {
  test("appends the current item position and total count to the file name", () => {
    assert.equal(
      formatPreviewTitle("a_189.jpg", { current: 18, total: 31 }),
      "a_189.jpg [18/31]"
    );
  });

  test("uses mediaSlots keys as absolute zero-based positions", () => {
    const mediaSlots = new Map([
      [16, { id: 41 }],
      [17, { id: 42 }],
      [18, { id: 43 }]
    ]);

    assert.deepEqual(
      resolvePreviewTitlePosition({
        media: [],
        mediaSlots,
        mediaTotalCount: 31,
        selectedMediaId: 42
      }),
      { current: 18, total: 31 }
    );
  });

  test("falls back to the loaded media array when no slot contains the selection", () => {
    assert.deepEqual(
      resolvePreviewTitlePosition({
        media: [{ id: 10 }, { id: 11 }, { id: 12 }],
        mediaSlots: new Map(),
        mediaTotalCount: 31,
        selectedMediaId: 11
      }),
      { current: 2, total: 31 }
    );
  });
});
