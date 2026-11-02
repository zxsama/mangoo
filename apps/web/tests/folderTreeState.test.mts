import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  expandFolderPath,
  compareFolderRecords,
  folderShowsOpenIcon,
  isFolderKnownChildless,
  mergeFolderAncestorPath,
  mergeFolderChildrenByParent,
  reconcileFolderExpansionForSelection
} from "../src/core/folderTreeState.ts";

function folder(id: number, name: string, parentId: number | null): {
  id: number;
  rootId: number;
  parentId: number | null;
  name: string;
  status: string;
  hasChildren: boolean;
  mediaCount: number;
} {
  return {
    id,
    rootId: 1,
    parentId,
    name,
    status: "active",
    hasChildren: true,
    mediaCount: 0
  };
}

describe("folder tree interaction state", () => {
  it("keeps descending keyset pages and hydrated ancestors in SQLite BINARY name/id order", () => {
    const rows = [folder(1, "a", 10), folder(2, "A", 10), folder(3, "\uE000", 10), folder(4, "😀", 10), folder(5, "a", 10)];
    const sorted = [...rows].sort((a, b) => compareFolderRecords(a, b, "name_desc"));
    assert.deepEqual(sorted.map(({ id }) => id), [4, 3, 5, 1, 2]);
    const first = { 10: sorted.slice(0, 2) };
    const hydrated = mergeFolderChildrenByParent(first, 10, [rows[0]], "name_desc");
    const paged = mergeFolderChildrenByParent(hydrated, 10, sorted.slice(2), "name_desc");
    assert.deepEqual(paged[10].map(({ id }) => id), [4, 3, 5, 1, 2]);
  });
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

  it("keeps unknown, paged, loading, failed and newly populated branches reachable", () => {
    const empty = { childrenLoaded: false, loadedChildCount: 0, loading: false };
    assert.equal(isFolderKnownChildless(empty), false, "missing metadata is unknown");
    assert.equal(isFolderKnownChildless({ ...empty, hasChildren: false }), true);
    assert.equal(isFolderKnownChildless({ ...empty, childrenLoaded: true }), true);
    for (const evidence of [{ loadedChildCount: 1 }, { hasMore: true }, { loading: true }, { loadFailed: true }]) {
      assert.equal(isFolderKnownChildless({ ...empty, hasChildren: false, childrenLoaded: true, ...evidence }), false);
    }
    assert.equal(isFolderKnownChildless({ ...empty, hasChildren: true, childrenLoaded: false }), false, "invalidated empty page is unknown again");
  });

  it("merges an ancestor path without replacing or duplicating paged children", () => {
    const existing = {
      10: [folder(12, "bravo", 10), folder(11, "alpha", 10)]
    };
    const merged = mergeFolderAncestorPath(existing, [
      folder(10, "root", null),
      folder(11, "alpha", 10),
      folder(20, "target", 11)
    ]);

    assert.deepEqual(
      merged[10].map((item) => item.id),
      [11, 12]
    );
    assert.deepEqual(merged[11].map((item) => item.id), [20]);
    assert.deepEqual(existing[10].map((item) => item.id), [12, 11]);
    assert.equal(mergeFolderChildrenByParent(merged, 11, [merged[11][0]]), merged);
  });

  it("expands every ancestor but not the target and preserves existing state", () => {
    const current = new Set([99]);
    const path = [folder(10, "root", null), folder(11, "parent", 10), folder(20, "target", 11)];
    const expanded = expandFolderPath(current, path);

    assert.deepEqual([...expanded], [99, 10, 11]);
    assert.equal(expandFolderPath(expanded, path), expanded);
  });
});
