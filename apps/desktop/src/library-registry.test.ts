import { test } from "node:test";
import assert from "node:assert/strict";
import os from "node:os";
import fs from "node:fs";
import path from "node:path";
import {
  loadRegistry, saveRegistry, addLibrary, removeLibrary, setActive, renameLibrary, type LibraryRegistry
} from "./library-registry.ts";

function tmpDir(): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), "megle-reg-"));
}

test("loadRegistry returns empty registry when file is missing", () => {
  const dir = tmpDir();
  assert.deepEqual(loadRegistry(dir), { activeLibraryId: null, libraries: [] });
});

test("loadRegistry recovers from corrupt JSON", () => {
  const dir = tmpDir();
  fs.writeFileSync(path.join(dir, "libraries.json"), "{ not json");
  assert.deepEqual(loadRegistry(dir), { activeLibraryId: null, libraries: [] });
});

test("addLibrary assigns an id and persists via save/load round-trip", () => {
  const dir = tmpDir();
  let reg = loadRegistry(dir);
  const { registry, entry } = addLibrary(reg, { name: "A", dbPath: "C:/a.sqlite", now: 1000 });
  assert.match(entry.id, /^lib_[0-9a-f]{8}$/);
  assert.equal(entry.name, "A");
  assert.equal(registry.libraries.length, 1);
  saveRegistry(dir, registry);
  assert.deepEqual(loadRegistry(dir), registry);
});

test("addLibrary is idempotent by dbPath", () => {
  let reg: LibraryRegistry = { activeLibraryId: null, libraries: [] };
  reg = addLibrary(reg, { name: "A", dbPath: "C:/a.sqlite", now: 1 }).registry;
  const second = addLibrary(reg, { name: "A again", dbPath: "C:/a.sqlite", now: 2 });
  assert.equal(second.registry.libraries.length, 1);
  assert.equal(second.entry.name, "A"); // existing entry returned, not duplicated
});

test("setActive sets id and bumps lastOpenedAt", () => {
  let reg: LibraryRegistry = { activeLibraryId: null, libraries: [] };
  const added = addLibrary(reg, { name: "A", dbPath: "C:/a.sqlite", now: 1 });
  reg = setActive(added.registry, added.entry.id, 5000);
  assert.equal(reg.activeLibraryId, added.entry.id);
  assert.equal(reg.libraries[0].lastOpenedAt, 5000);
});

test("removeLibrary clears active id when removing the active library", () => {
  let reg: LibraryRegistry = { activeLibraryId: null, libraries: [] };
  const added = addLibrary(reg, { name: "A", dbPath: "C:/a.sqlite", now: 1 });
  reg = setActive(added.registry, added.entry.id, 2);
  reg = removeLibrary(reg, added.entry.id);
  assert.equal(reg.activeLibraryId, null);
  assert.equal(reg.libraries.length, 0);
});

test("renameLibrary changes the name only", () => {
  let reg: LibraryRegistry = { activeLibraryId: null, libraries: [] };
  const added = addLibrary(reg, { name: "A", dbPath: "C:/a.sqlite", now: 1 });
  reg = renameLibrary(added.registry, added.entry.id, "B");
  assert.equal(reg.libraries[0].name, "B");
  assert.equal(reg.libraries[0].dbPath, "C:/a.sqlite");
});
