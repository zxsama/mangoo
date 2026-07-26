import assert from "node:assert/strict";
import { it } from "node:test";
import type { LibraryLoadPhase } from "../src/core/libraryTypes.ts";

it("replays a persisted recovery error to a newly loaded renderer", async () => {
  const recoveryError: LibraryLoadPhase = {
    kind: "error",
    message: "Library recovery failed; retry or switch library"
  };
  let listener: ((phase: LibraryLoadPhase) => void) | null = null;
  (globalThis as typeof globalThis & { window: unknown }).window = {
    megleDesktop: {
      onLibraryPhase: (next: (phase: LibraryLoadPhase) => void) => {
        listener = next;
        return () => {
          listener = null;
        };
      },
      getLibraryPhase: async () => recoveryError
    }
  };
  const { subscribeLibraryPhase } = await import("../src/core/desktop.ts");
  const observed: LibraryLoadPhase[] = [];

  const unsubscribe = subscribeLibraryPhase((phase) => observed.push(phase));
  await Promise.resolve();

  assert.deepEqual(observed, [recoveryError]);
  unsubscribe();
  assert.equal(listener, null);
});

it("does not overwrite a live phase with an older reload snapshot", async () => {
  let resolveSnapshot!: (phase: LibraryLoadPhase) => void;
  const snapshot = new Promise<LibraryLoadPhase>((resolve) => {
    resolveSnapshot = resolve;
  });
  let listener: ((phase: LibraryLoadPhase) => void) | null = null;
  (globalThis as typeof globalThis & { window: unknown }).window = {
    megleDesktop: {
      onLibraryPhase: (next: (phase: LibraryLoadPhase) => void) => {
        listener = next;
        return () => undefined;
      },
      getLibraryPhase: () => snapshot
    }
  };
  const { subscribeLibraryPhase } = await import("../src/core/desktop.ts?phase-race");
  const observed: LibraryLoadPhase[] = [];

  subscribeLibraryPhase((phase) => observed.push(phase));
  listener?.("loading-media");
  resolveSnapshot("opening-library");
  await Promise.resolve();

  assert.deepEqual(observed, ["loading-media"]);
});

it("replays loading-media when Core became ready before the renderer subscribed", async () => {
  const phaseBeforeSubscribe: LibraryLoadPhase = "loading-media";
  (globalThis as typeof globalThis & { window: unknown }).window = {
    megleDesktop: {
      onLibraryPhase: () => () => undefined,
      getLibraryPhase: async () => phaseBeforeSubscribe
    }
  };
  const { subscribeLibraryPhase } = await import("../src/core/desktop.ts?phase-before-subscribe");
  const observed: LibraryLoadPhase[] = [];

  subscribeLibraryPhase((phase) => observed.push(phase));
  await Promise.resolve();

  assert.deepEqual(observed, ["loading-media"]);
});

it("replays the current loading phase again after a renderer reload", async () => {
  let snapshotReads = 0;
  (globalThis as typeof globalThis & { window: unknown }).window = {
    megleDesktop: {
      onLibraryPhase: () => () => undefined,
      getLibraryPhase: async () => {
        snapshotReads += 1;
        return "loading-media" as const;
      }
    }
  };
  const { subscribeLibraryPhase } = await import("../src/core/desktop.ts?phase-reload");
  const firstRenderer: LibraryLoadPhase[] = [];
  const secondRenderer: LibraryLoadPhase[] = [];

  const unsubscribeFirst = subscribeLibraryPhase((phase) => firstRenderer.push(phase));
  await Promise.resolve();
  unsubscribeFirst();
  subscribeLibraryPhase((phase) => secondRenderer.push(phase));
  await Promise.resolve();

  assert.equal(snapshotReads, 2);
  assert.deepEqual(firstRenderer, ["loading-media"]);
  assert.deepEqual(secondRenderer, ["loading-media"]);
});

it("keeps the live subscription usable when the reload snapshot request fails", async () => {
  let listener: ((phase: LibraryLoadPhase) => void) | null = null;
  (globalThis as typeof globalThis & { window: unknown }).window = {
    megleDesktop: {
      onLibraryPhase: (next: (phase: LibraryLoadPhase) => void) => {
        listener = next;
        return () => undefined;
      },
      getLibraryPhase: async () => {
        throw new Error("snapshot unavailable");
      }
    }
  };
  const { subscribeLibraryPhase } = await import("../src/core/desktop.ts?phase-failure");
  const observed: LibraryLoadPhase[] = [];

  subscribeLibraryPhase((phase) => observed.push(phase));
  await Promise.resolve();
  await Promise.resolve();
  listener?.("loading-media");

  assert.deepEqual(observed, ["loading-media"]);
});

it("routes retry through Desktop recovery instead of only reloading the renderer", async () => {
  let retryCalls = 0;
  (globalThis as typeof globalThis & { window: unknown }).window = {
    megleDesktop: {
      retryLibraryLoad: async () => {
        retryCalls += 1;
        return { ok: true };
      }
    }
  };
  const { retryLibraryLoad } = await import("../src/core/desktop.ts?phase-retry");

  assert.equal(await retryLibraryLoad(), true);
  assert.equal(retryCalls, 1);
});

it("wires the loading overlay retry button to Desktop recovery", async () => {
  const { readFile } = await import("node:fs/promises");
  const appSource = await readFile(new URL("../src/app/App.tsx", import.meta.url), "utf8");

  assert.match(appSource, /onRetry=\{\(\)\s*=>\s*void retryLibraryLoad\(\)\}/);
  assert.doesNotMatch(appSource, /onRetry=\{\(\)\s*=>\s*window\.location\.reload\(\)\}/);
});
