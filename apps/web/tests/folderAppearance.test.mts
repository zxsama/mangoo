import assert from "node:assert/strict";
import { it } from "node:test";
import { reconcileFolderAppearanceRead, replaceFolderAppearanceInMap } from "../src/core/folderAppearanceState.ts";
import type { FolderRecord } from "@mangoo/core-client";

it("late folder reads preserve confirmed appearance but current reads remain authoritative", () => {
  const before: FolderRecord = { id: 2, rootId: 1, parentId: 1, name: "Renamed", status: "active", hasChildren: true, mediaCount: 5, fileCount: 8, icon: null, color: null };
  const saved = { ...before, icon: "camera", color: "#123456" };
  const overrides = new Map([[2, { folder: saved, version: 1 }]]);
  const late = reconcileFolderAppearanceRead([before], 0, overrides)[0];
  assert.equal(late.icon, "camera");
  assert.equal(late.name, "Renamed");
  assert.equal(late.fileCount, 8);
  assert.equal(reconcileFolderAppearanceRead([before], 1, overrides)[0], before);
  assert.equal(replaceFolderAppearanceInMap({ 1: [before] }, saved)[1][0].color, "#123456");
});
