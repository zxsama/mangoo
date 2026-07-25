import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  activateLibraryIfCurrent,
  removeLibraryIfInactive,
  type LibraryRegistry
} from "../src/library-registry.ts";

const previousPath = "D:\\Libraries\\previous.sqlite";
const targetPath = "D:\\Libraries\\target.sqlite";

function registry(): LibraryRegistry {
  return {
    activeLibraryId: "previous",
    libraries: [
      { id: "previous", name: "Previous", dbPath: previousPath, lastOpenedAt: 1 },
      { id: "target", name: "Target", dbPath: targetPath, lastOpenedAt: 2 }
    ]
  };
}

describe("active library removal guard", () => {
  it("rejects an active library even when Settings acts from a stale inactive snapshot", () => {
    const latest = registry();

    const result = removeLibraryIfInactive(latest, "previous");

    assert.deepEqual(result, {
      ok: false,
      reason: "active",
      registry: latest
    });
  });

  it("rejects the target after a background media-focus switch has committed it", () => {
    const committed = activateLibraryIfCurrent(registry(), {
      id: "target",
      dbPath: targetPath,
      now: 10
    });
    assert.equal(committed.ok, true);
    if (!committed.ok) return;

    const result = removeLibraryIfInactive(committed.registry, "target");

    assert.equal(result.ok, false);
    if (!result.ok) assert.equal(result.reason, "active");
    assert.equal(result.registry.libraries.some((library) => library.id === "target"), true);
  });

  it("removes a non-active library and preserves the latest active entry", () => {
    const latest: LibraryRegistry = {
      ...registry(),
      libraries: registry().libraries.map((library) =>
        library.id === "previous" ? { ...library, name: "Renamed previous" } : library
      )
    };

    const result = removeLibraryIfInactive(latest, "target");

    assert.equal(result.ok, true);
    if (!result.ok) return;
    assert.equal(result.registry.activeLibraryId, "previous");
    assert.equal(result.registry.libraries.length, 1);
    assert.equal(result.registry.libraries[0]?.name, "Renamed previous");
    assert.equal(result.entry.id, "target");
  });

  it("has deterministic latest-registry ordering with a concurrent switch commit", () => {
    const removedBeforeCommit = removeLibraryIfInactive(registry(), "target");
    assert.equal(removedBeforeCommit.ok, true);
    if (!removedBeforeCommit.ok) return;
    const lateCommit = activateLibraryIfCurrent(removedBeforeCommit.registry, {
      id: "target",
      dbPath: targetPath,
      now: 10
    });
    assert.equal(lateCommit.ok, false);
    if (!lateCommit.ok) assert.equal(lateCommit.reason, "missing");

    const committedBeforeRemove = activateLibraryIfCurrent(registry(), {
      id: "target",
      dbPath: targetPath,
      now: 10
    });
    assert.equal(committedBeforeRemove.ok, true);
    if (!committedBeforeRemove.ok) return;
    const lateRemove = removeLibraryIfInactive(committedBeforeRemove.registry, "target");
    assert.equal(lateRemove.ok, false);
    if (!lateRemove.ok) assert.equal(lateRemove.reason, "active");
  });

  it("returns an explicit missing result instead of reporting a no-op success", () => {
    const latest = registry();

    assert.deepEqual(removeLibraryIfInactive(latest, "unknown"), {
      ok: false,
      reason: "missing",
      registry: latest
    });
  });
});
