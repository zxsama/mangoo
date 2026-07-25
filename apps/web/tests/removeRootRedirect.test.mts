import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  decideRemoveRootRedirect,
  shouldClearInspectedFolder
} from "../src/core/removeRootRedirect.ts";

describe("decideRemoveRootRedirect", () => {
  it("redirects the open view to the first surviving root when the open root is removed", () => {
    const r = decideRemoveRootRedirect({
      removedRootId: 2,
      selectedRootId: 2,
      inspectedFolderRootId: null,
      survivingRoots: [
        { id: 1, rootFolderId: 100 },
        { id: 3, rootFolderId: 300 }
      ]
    });
    assert.deepEqual(r, { kind: "redirect", rootId: 1, folderId: 100 });
  });

  it("blanks the view when the open root is removed and none survive", () => {
    const r = decideRemoveRootRedirect({
      removedRootId: 2,
      selectedRootId: 2,
      inspectedFolderRootId: null,
      survivingRoots: []
    });
    assert.deepEqual(r, { kind: "blank" });
  });

  it("keeps navigation when a NON-open root is removed", () => {
    const r = decideRemoveRootRedirect({
      removedRootId: 5,
      selectedRootId: 2,
      inspectedFolderRootId: null,
      survivingRoots: [{ id: 2, rootFolderId: 200 }]
    });
    assert.deepEqual(r, { kind: "keep" });
  });

  it("keeps navigation when nothing is open (selectedRootId null)", () => {
    const r = decideRemoveRootRedirect({
      removedRootId: 5,
      selectedRootId: null,
      inspectedFolderRootId: null,
      survivingRoots: [{ id: 2, rootFolderId: 200 }]
    });
    assert.deepEqual(r, { kind: "keep" });
  });

  it("redirect target uses null folderId when the surviving root has no root folder yet", () => {
    const r = decideRemoveRootRedirect({
      removedRootId: 2,
      selectedRootId: 2,
      inspectedFolderRootId: null,
      survivingRoots: [{ id: 1, rootFolderId: null }]
    });
    assert.deepEqual(r, { kind: "redirect", rootId: 1, folderId: null });
  });
});

describe("shouldClearInspectedFolder", () => {
  it("clears the inspected folder when its root was removed (even if a DIFFERENT root is open)", () => {
    assert.equal(
      shouldClearInspectedFolder({
        removedRootId: 5,
        selectedRootId: 2, // a different root is open
        inspectedFolderRootId: 5, // but the inspected folder belongs to the removed root
        survivingRoots: [{ id: 2, rootFolderId: 200 }]
      }),
      true
    );
  });

  it("does not clear the inspected folder when it belongs to a surviving root", () => {
    assert.equal(
      shouldClearInspectedFolder({
        removedRootId: 5,
        selectedRootId: 2,
        inspectedFolderRootId: 2,
        survivingRoots: [{ id: 2, rootFolderId: 200 }]
      }),
      false
    );
  });

  it("does not clear when nothing is inspected", () => {
    assert.equal(
      shouldClearInspectedFolder({
        removedRootId: 5,
        selectedRootId: 2,
        inspectedFolderRootId: null,
        survivingRoots: []
      }),
      false
    );
  });
});
