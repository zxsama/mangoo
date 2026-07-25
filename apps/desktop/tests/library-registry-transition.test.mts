import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  activateLibraryIfCurrent,
  addLibrary,
  removeLibraryIfInactive,
  renameLibrary,
  setLibraryIcon,
  type LibraryRegistry
} from "../src/library-registry.ts";

const previousPath = "D:\\Libraries\\previous.sqlite";
const targetPath = "D:\\Libraries\\target.sqlite";

function registry(): LibraryRegistry {
  return {
    activeLibraryId: "previous",
    libraries: [
      { id: "previous", name: "Previous", dbPath: previousPath, lastOpenedAt: 1 },
      { id: "target", name: "Target", dbPath: targetPath, lastOpenedAt: 2 },
      { id: "removed", name: "Removed", dbPath: "D:\\Libraries\\removed.sqlite", lastOpenedAt: 3 }
    ]
  };
}

function removeInactive(registryValue: LibraryRegistry, id: string): LibraryRegistry {
  const result = removeLibraryIfInactive(registryValue, id);
  assert.equal(result.ok, true);
  if (!result.ok) throw new Error(`Expected ${id} to be inactive`);
  return result.registry;
}

describe("conditional active library mutation", () => {
  it("commits a switch on the latest registry without losing rename, icon, add or remove updates", () => {
    let latest = renameLibrary(registry(), "target", "Renamed target");
    latest = setLibraryIcon(latest, "target", "folder-heart", "#d98268");
    latest = removeInactive(latest, "removed");
    const added = addLibrary(latest, {
      name: "Concurrent add",
      dbPath: "D:\\Libraries\\added.sqlite",
      now: 4
    });

    const result = activateLibraryIfCurrent(added.registry, {
      id: "target",
      dbPath: targetPath,
      now: 10
    });

    assert.equal(result.ok, true);
    if (!result.ok) return;
    assert.equal(result.registry.activeLibraryId, "target");
    assert.deepEqual(
      result.registry.libraries.map(({ id, name, icon, color }) => ({ id, name, icon, color })),
      [
        { id: "previous", name: "Previous", icon: undefined, color: undefined },
        { id: "target", name: "Renamed target", icon: "folder-heart", color: "#d98268" },
        { id: added.entry.id, name: "Concurrent add", icon: undefined, color: undefined }
      ]
    );
  });

  it("does not resurrect a target removed or deleted during a switch", () => {
    const latest = removeInactive(registry(), "target");
    const result = activateLibraryIfCurrent(latest, {
      id: "target",
      dbPath: targetPath,
      now: 10
    });

    assert.deepEqual(result, {
      ok: false,
      reason: "missing",
      registry: latest
    });
    assert.equal(result.registry.libraries.some((library) => library.id === "target"), false);
  });

  it("rejects a same-id target whose database path changed", () => {
    const latest = {
      ...registry(),
      libraries: registry().libraries.map((library) =>
        library.id === "target" ? { ...library, dbPath: "D:\\Libraries\\replacement.sqlite" } : library
      )
    };

    const result = activateLibraryIfCurrent(latest, {
      id: "target",
      dbPath: targetPath,
      now: 10
    });
    assert.equal(result.ok, false);
    if (!result.ok) assert.equal(result.reason, "db-path-changed");
    assert.equal(result.registry.activeLibraryId, "previous");
  });

  it("restores a still-registered previous library without losing concurrent target removal and add", () => {
    const renamedPrevious = renameLibrary(registry(), "previous", "Renamed previous");
    const withoutTarget = removeInactive(renamedPrevious, "target");
    const added = addLibrary(withoutTarget, {
      name: "Concurrent add",
      dbPath: "D:\\Libraries\\added.sqlite",
      now: 4
    });

    const result = activateLibraryIfCurrent(added.registry, {
      id: "previous",
      dbPath: previousPath,
      now: 10
    });
    assert.equal(result.ok, true);
    if (!result.ok) return;
    assert.equal(result.registry.activeLibraryId, "previous");
    assert.equal(result.registry.libraries.find((library) => library.id === "previous")?.name, "Renamed previous");
    assert.equal(result.registry.libraries.some((library) => library.id === "target"), false);
    assert.equal(result.registry.libraries.some((library) => library.id === added.entry.id), true);
  });

  it("returns an explicit conflict when the previous library was removed before recovery commit", () => {
    const latest = removeInactive(
      { ...registry(), activeLibraryId: "target" },
      "previous"
    );
    const result = activateLibraryIfCurrent(latest, {
      id: "previous",
      dbPath: previousPath,
      now: 10
    });

    assert.deepEqual(result, {
      ok: false,
      reason: "missing",
      registry: latest
    });
    assert.equal(result.registry.libraries.some((library) => library.id === "previous"), false);
  });
});
