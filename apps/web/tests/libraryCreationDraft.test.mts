import assert from "node:assert/strict";
import { test } from "node:test";
import { applyLibraryCreationPaths, initialLibraryPathDraft, libraryContentInputs } from "../src/features/library/libraryCreationDraft.ts";

test("automatic paths follow native results without replacing existing or removed content roots", () => {
  const initial = initialLibraryPathDraft();
  const a = applyLibraryCreationPaths(initial, { databasePath: "A/Database", defaultContentPath: "A/Files" });
  a.roots.push({ id: 1, source: "existing", path: "external", createIfMissing: false });
  const b = applyLibraryCreationPaths(a, { databasePath: "B/Database", defaultContentPath: "B/Files" });
  assert.equal(b.databasePath, "B/Database");
  assert.deepEqual(libraryContentInputs(b), [{ path: "B/Files", createIfMissing: true }, { path: "external", createIfMissing: false }]);
  b.roots.splice(0, 1);
  assert.deepEqual(libraryContentInputs(applyLibraryCreationPaths(b, { databasePath: "C/Database", defaultContentPath: "C/Files" })), [{ path: "external", createIfMissing: false }]);
});

test("manual Database stays pinned and request snapshot has no draft metadata or shared objects", () => {
  const draft = { ...initialLibraryPathDraft(), databasePath: "custom/catalog", databaseSource: "manual" as const };
  const resolved = applyLibraryCreationPaths(draft, { databasePath: "ignored", defaultContentPath: "custom/Files" });
  assert.equal(resolved.databasePath, "custom/catalog");
  const snapshot = libraryContentInputs(resolved);
  resolved.roots[0].path = "later edit";
  assert.deepEqual(snapshot, [{ path: "custom/Files", createIfMissing: true }]);
});
