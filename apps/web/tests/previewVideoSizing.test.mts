import { strict as assert } from "node:assert";
import { describe, test } from "node:test";

import {
  resolveVideoPreviewNaturalSize,
  videoPreviewNaturalSize
} from "../src/features/preview/videoPreviewSizing.ts";

describe("video preview sizing", () => {
  test("uses loaded video metadata as the natural preview frame", () => {
    assert.deepEqual(videoPreviewNaturalSize({ videoWidth: 1920, videoHeight: 1080 }), {
      naturalHeight: 1080,
      naturalWidth: 1920
    });
  });

  test("ignores unavailable metadata dimensions", () => {
    assert.equal(videoPreviewNaturalSize({ videoWidth: 0, videoHeight: 1080 }), null);
    assert.equal(videoPreviewNaturalSize({ videoWidth: 1920, videoHeight: 0 }), null);
  });

  test("metadata dimensions override the temporary poster frame", () => {
    assert.deepEqual(
      resolveVideoPreviewNaturalSize(
        { videoWidth: 1920, videoHeight: 1080 },
        { naturalWidth: 320, naturalHeight: 180 }
      ),
      { naturalWidth: 1920, naturalHeight: 1080 }
    );
  });
});
