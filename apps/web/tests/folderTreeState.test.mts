import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  folderShowsOpenIcon,
  isFolderKnownChildless,
  reconcileFolderExpansionForSelection
} from "../src/core/folderTreeState.ts";

describe("folder tree interaction state", () => {
  it("opens only the currently selected leaf icon", () => {
    assert.equal(
      folderShowsOpenIcon({ expanded: true, knownChildless: true, selected: true }),
      true
    );
    assert.equal(
      folderShowsOpenIcon({ expanded: true, knownChildless: true, selected: false }),
      false
    );
  });

  it("uses real expansion rather than selection for folders with children", () => {
    assert.equal(
      folderShowsOpenIcon({ expanded: false, knownChildless: false, selected: true }),
      false
    );
    assert.equal(
      folderShowsOpenIcon({ expanded: true, knownChildless: false, selected: false }),
      true
    );
  });

  it("removes fake leaf expansion while preserving sibling and parent expansion", () => {
    const expanded = new Set([10, 20]);
    const withoutLeaf = reconcileFolderExpansionForSelection(expanded, 20, false);
    assert.deepEqual([...withoutLeaf], [10]);
    assert.deepEqual([...expanded], [10, 20]);

    const withParent = reconcileFolderExpansionForSelection(withoutLeaf, 30, true);
    assert.deepEqual([...withParent], [10, 30]);
    assert.equal(reconcileFolderExpansionForSelection(withParent, 30, true), withParent);
  });

  it("treats a completed empty child page as authoritative childless state", () => {
    assert.equal(
      isFolderKnownChildless({
        hasChildren: true,
        childrenLoaded: true,
        loadedChildCount: 0,
        loading: false
      }),
      true
    );
    assert.equal(
      isFolderKnownChildless({
        hasChildren: true,
        childrenLoaded: true,
        loadedChildCount: 0,
        loading: true
      }),
      false
    );
  });
});
