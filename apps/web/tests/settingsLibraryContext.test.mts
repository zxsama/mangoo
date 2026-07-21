import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { createLibraryContextTracker } from "../src/core/libraryContextTracker.ts";
import type { LibrarySummary } from "../src/core/libraryTypes.ts";

function library(id: string, active = true): LibrarySummary {
  return {
    id,
    name: `Library ${id.toUpperCase()}`,
    dbPath: `${id}.sqlite`,
    available: true,
    active
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe("settings library context tracker", () => {
  it("refreshes A to B from the real loading-media completion event", async () => {
    let libraries = [library("a")];
    const contexts: Array<{ libraryId: string; libraryName: string } | null> = [];
    const tracker = createLibraryContextTracker({
      loadLibraries: async () => libraries,
      onContext: (context) => contexts.push(context)
    });

    await tracker.start();
    await tracker.onPhase("starting-core");
    libraries = [library("a", false), library("b")];
    await tracker.onPhase("loading-media");

    assert.deepEqual(contexts.at(-1), {
      libraryId: "b",
      libraryName: "Library B"
    });
    tracker.dispose();
  });

  it("keeps context cleared when a switch fails or is interrupted", async () => {
    const contexts: Array<{ libraryId: string; libraryName: string } | null> = [];
    const tracker = createLibraryContextTracker({
      loadLibraries: async () => [library("a")],
      onContext: (context) => contexts.push(context)
    });

    await tracker.start();
    await tracker.onPhase("starting-core");
    await tracker.onPhase("opening-library");
    await tracker.onPhase({ kind: "error", message: "Core failed" });

    assert.equal(contexts.at(-1), null);
    tracker.dispose();
  });

  it("ignores a stale B refresh when a rapid switch to C completes first", async () => {
    const initial = deferred<LibrarySummary[]>();
    const switchedToB = deferred<LibrarySummary[]>();
    const switchedToC = deferred<LibrarySummary[]>();
    const loads = [initial, switchedToB, switchedToC];
    const contexts: Array<{ libraryId: string; libraryName: string } | null> = [];
    const tracker = createLibraryContextTracker({
      loadLibraries: () => loads.shift()!.promise,
      onContext: (context) => contexts.push(context)
    });

    const initialLoad = tracker.start();
    initial.resolve([library("a")]);
    await initialLoad;

    await tracker.onPhase("starting-core");
    const loadingB = tracker.onPhase("loading-media");
    await tracker.onPhase("starting-core");
    const loadingC = tracker.onPhase("loading-media");
    switchedToC.resolve([library("a", false), library("b", false), library("c")]);
    await loadingC;
    switchedToB.resolve([library("a", false), library("b")]);
    await loadingB;

    assert.deepEqual(contexts.at(-1), {
      libraryId: "c",
      libraryName: "Library C"
    });
    assert.equal(
      contexts.some((context) => context?.libraryId === "b"),
      false
    );
    tracker.dispose();
  });
});
