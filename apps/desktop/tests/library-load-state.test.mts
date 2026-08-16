import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { it } from "node:test";
import {
  LibraryLoadState,
  prepareColdLibraryStartup
} from "../src/library-load-state.ts";

it("retains a recovery error for a renderer that reloads after the broadcast", () => {
  const state = new LibraryLoadState();
  const recoveryError = {
    kind: "error" as const,
    message: "Library recovery failed; retry or switch library"
  };

  state.publish(recoveryError);

  assert.deepEqual(state.snapshot(), recoveryError);
});

it("publishes loading-media after Core and cold library preparation complete", async () => {
  const state = new LibraryLoadState();
  const events: string[] = [];
  state.publish({ kind: "error", message: "previous recovery failed" });

  const session = await prepareColdLibraryStartup({
    ensureCoreReady: async () => {
      events.push("core-ready");
      return { id: "cold-session" };
    },
    prepareLibrary: async (readySession) => {
      assert.equal(readySession.id, "cold-session");
      events.push("library-prepared");
    },
    publishPhase: (phase) => {
      events.push(`phase:${phase}`);
      state.publish(phase);
    }
  });

  assert.equal(session.id, "cold-session");
  assert.deepEqual(events, ["core-ready", "library-prepared", "phase:loading-media"]);
  assert.equal(state.snapshot(), "loading-media");
});

it("does not publish loading-media when cold library preparation fails", async () => {
  const state = new LibraryLoadState();

  await assert.rejects(
    prepareColdLibraryStartup({
      ensureCoreReady: async () => ({ id: "cold-session" }),
      prepareLibrary: async () => {
        throw new Error("preparation failed");
      },
      publishPhase: (phase) => state.publish(phase)
    }),
    /preparation failed/
  );

  assert.equal(state.snapshot(), "starting-core");
});

it("publishes loading-media for a normal cold start with no auto-add work", async () => {
  const state = new LibraryLoadState();

  await prepareColdLibraryStartup({
    ensureCoreReady: async () => "existing-library-session",
    prepareLibrary: async () => undefined,
    publishPhase: (phase) => state.publish(phase)
  });

  assert.equal(state.snapshot(), "loading-media");
});

it("uses the replayable cold-start boundary before creating the renderer", async () => {
  const mainSource = await readFile(new URL("../src/main.ts", import.meta.url), "utf8");
  const normalizedSource = mainSource.replace(/\r\n/g, "\n");
  const coldStartup = normalizedSource.slice(
    normalizedSource.indexOf("app\n  .whenReady()"),
    normalizedSource.indexOf("function handleDesktopStartupFailure")
  );

  assert.match(coldStartup, /await prepareColdLibraryStartup\(\{/);
  assert.match(coldStartup, /ensureCoreReady,[\s\S]*prepareLibrary: maybeAutoAddRoot/);
  assert.ok(
    coldStartup.indexOf("await prepareColdLibraryStartup") <
      coldStartup.indexOf("await createWindow()")
  );
});
