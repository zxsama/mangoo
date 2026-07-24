import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  prepareMediaFocusNavigation,
  resolveMediaLocateVirtualIndex
} from "../src/app/mediaFocusWorkflow.ts";

const request = {
  requestId: "focus-1",
  libraryId: "library-a",
  fileId: 42,
  folderId: 7,
  rootId: 3
};

describe("media focus workflow", () => {
  it("resolves a deep offscreen media slot to its exact virtual row", () => {
    assert.equal(resolveMediaLocateVirtualIndex({
      mediaIndex: 900_004,
      columnCount: 5,
      mediaRowStartIndex: 3,
      mediaRowCount: 200_000
    }), 180_003);
  });

  it("closes preview before navigation and returns an explicit grid locate request", async () => {
    let previewOpen = true;
    let previewOpened = false;
    const result = await prepareMediaFocusNavigation(request, {
      closePreview: () => { previewOpen = false; },
      locateMedia: async () => {
        assert.equal(previewOpen, false);
        return true;
      }
    });

    assert.deepEqual(result, { requestId: "focus-1", mediaId: 42 });
    assert.equal(previewOpen, false);
    assert.equal(previewOpened, false);
  });

  it("returns no scroll request after failure and allows the same media to retry", async () => {
    let attempts = 0;
    const dependencies = {
      closePreview: () => undefined,
      locateMedia: async () => ++attempts > 1
    };

    assert.equal(await prepareMediaFocusNavigation(request, dependencies), null);
    assert.deepEqual(await prepareMediaFocusNavigation(
      { ...request, requestId: "focus-2" },
      dependencies
    ), { requestId: "focus-2", mediaId: 42 });
  });
});
