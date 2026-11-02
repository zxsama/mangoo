import assert from "node:assert/strict";
import { it } from "node:test";

it("safe registration wrappers share context reconciliation while inactive forget only invalidates the registry", async () => {
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const previousChannel = globalThis.BroadcastChannel;
  const previousFetch = globalThis.fetch;
  const sent: unknown[] = [];
  let receive: ((event: { data: unknown }) => void) | undefined;
  class Channel {
    constructor(name: string) { assert.equal(name, "mangoo-library-context:http://registration.test/api"); }
    addEventListener(_event: string, callback: typeof receive) { receive = callback; }
    postMessage(message: unknown) { sent.push(message); }
    close() {}
  }
  Object.defineProperty(globalThis, "window", { configurable: true, value: {
    mangooDesktop: { coreUrl: "http://registration.test/api", sessionToken: "fixture" },
    addEventListener() {},
  } });
  globalThis.BroadcastChannel = Channel as unknown as typeof BroadcastChannel;
  let active = "active";
  let rejectOpen = false;
  let partialOpen = false;
  let rejectForget = false;
  let rejectAppearance = false;
  const calls: Array<{ path: string; revision: string | null; body: unknown }> = [];
  globalThis.fetch = async (input, init) => {
    const url = new URL(String(input));
    const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
    if (url.pathname.endsWith("/active") && (!init?.method || init.method === "GET")) return json({ contextRevision: 17, library: { libraryUuid: active } });
    calls.push({ path: url.pathname, revision: new Headers(init?.headers).get("X-Mangoo-Context-Revision"), body: init?.body ? JSON.parse(String(init.body)) : null });
    if (url.pathname.endsWith("/open")) {
      if (rejectOpen) return json({ message: "invalid managed root" }, 400);
      return json({ library: { libraryUuid: "opened" }, activated: !partialOpen, activationError: partialOpen ? "storage unavailable" : null });
    }
    if (url.pathname.endsWith("/registration")) {
      if (rejectForget) return json({ message: "registry conflict" }, 409);
      return json({ library: null, contextRevision: 18 });
    }
    if (init?.method === "PATCH") {
      if (rejectAppearance) return json({ message: "appearance unavailable" }, 500);
      return json({ library: { libraryUuid: active } });
    }
    throw new Error(`Unexpected endpoint ${url.pathname}`);
  };
  const { openExistingLibrary, forgetLibraryEntry, setLibraryIcon, subscribeLibraryPhase, subscribeLibraryRegistry } = await import("../src/core/desktop.ts");
  const phases: unknown[] = [];
  let registryEvents = 0;
  const stopPhase = subscribeLibraryPhase((phase) => phases.push(phase));
  const stopRegistry = subscribeLibraryRegistry(() => { registryEvents++; });
  try {
    await openExistingLibrary("C:/Existing");
    assert.deepEqual(phases, ["starting-core", "loading-media"]);
    assert.equal(registryEvents, 1);
    assert.deepEqual(calls.at(-1), { path: "/api/libraries/open", revision: "17", body: { databasePath: "C:/Existing" } });
    assert.deepEqual(sent, ["context-settled"]);
    phases.length = 0;
    partialOpen = true;
    await assert.rejects(openExistingLibrary("C:/Existing"), /已添加到列表.*storage unavailable/);
    assert.deepEqual(phases, ["starting-core", "loading-media"]);
    partialOpen = false;
    rejectOpen = true;
    phases.length = 0;
    await assert.rejects(openExistingLibrary("C:/Invalid"));
    assert.deepEqual(phases, ["starting-core", "loading-media"]);
    phases.length = 0;
    await forgetLibraryEntry("other");
    assert.deepEqual(phases, []);
    assert.equal(sent.at(-1), "registry-settled");
    assert.deepEqual(calls.at(-1), { path: "/api/libraries/other/registration", revision: "17", body: null });
    const beforeRemote = registryEvents;
    receive?.({ data: "registry-settled" });
    assert.equal(registryEvents, beforeRemote + 1);
    assert.deepEqual(phases, []);
    receive?.({ data: { type: "registry-settled", databasePath: "ignored" } });
    assert.equal(registryEvents, beforeRemote + 1);
    rejectForget = true;
    await assert.rejects(forgetLibraryEntry("other"));
    assert.deepEqual(phases, []);
    assert.equal(sent.at(-1), "registry-settled");
    rejectForget = false;
    await forgetLibraryEntry("active");
    assert.deepEqual(phases, ["starting-core", "loading-media"]);
    assert.equal(sent.at(-1), "context-settled");
    phases.length = 0;
    const beforeContext = registryEvents;
    receive?.({ data: "context-settled" });
    assert.equal(registryEvents, beforeContext + 1);
    assert.deepEqual(phases, ["starting-core", "loading-media"]);
    phases.length = 0;
    const beforeAppearance = registryEvents;
    await setLibraryIcon("active", "camera", "#123456");
    assert.deepEqual(calls.at(-1), { path: "/api/libraries/active", revision: "17", body: { icon: "camera", color: "#123456" } });
    assert.equal(registryEvents, beforeAppearance + 1);
    assert.equal(sent.at(-1), "registry-settled");
    assert.deepEqual(phases, []);
    rejectAppearance = true;
    const sentBeforeFailure = sent.length;
    await assert.rejects(setLibraryIcon("other", "camera", "#ffffff"));
    assert.equal(registryEvents, beforeAppearance + 1);
    assert.equal(sent.length, sentBeforeFailure);
    assert.deepEqual(phases, []);
  } finally {
    stopPhase(); stopRegistry();
    globalThis.fetch = previousFetch;
    globalThis.BroadcastChannel = previousChannel;
    if (previousWindow) Object.defineProperty(globalThis, "window", previousWindow);
    else Reflect.deleteProperty(globalThis, "window");
  }
});
