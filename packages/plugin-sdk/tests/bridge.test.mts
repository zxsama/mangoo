import assert from "node:assert/strict";
import { test } from "node:test";
import { createPluginBridge, createCoreClient, CoreApiError } from "../dist/index.js";

function fixture(timeout = 1000) {
  const sent: unknown[] = [];
  let listener: ((event: MessageEvent) => void) | undefined;
  const parent = { postMessage: (message: unknown) => { sent.push(message); } };
  const target = { parent, addEventListener: (_: string, next: (event: MessageEvent) => void) => { listener = next; }, removeEventListener: () => { listener = undefined; } };
  const bridge = createPluginBridge(target, timeout);
  const receive = (data: unknown, source: unknown = parent) => listener?.({ source, data } as MessageEvent);
  const init = (context = {}) => receive({ type: "mangoo-plugin-ui:init", sessionId: "session-a", pluginId: "example.plugin", viewId: "main", context: { libraryUuid: null, rootUuid: null, ...context }, theme: { colorScheme: "light" } });
  return { bridge, sent, receive, init };
}
test("bridge waits for exact parent init and correlates session/request without HTTP", async () => {
  const f = fixture();
  const response = f.bridge.call("core.getActiveLibrary");
  assert.deepEqual(f.sent, [{ type: "mangoo-plugin-ui:ready" }]);
  f.receive({ type: "mangoo-plugin-ui:init", sessionId: "forged" }, {});
  f.init();
  await f.bridge.ready;
  await Promise.resolve();
  const request = f.sent[1] as { requestId: string };
  f.receive({ type: "mangoo-plugin-ui:response", sessionId: "stale", requestId: request.requestId, result: "bad" });
  f.receive({ type: "mangoo-plugin-ui:response", sessionId: "session-a", requestId: request.requestId, result: "bad" }, {});
  f.receive({ type: "mangoo-plugin-ui:response", sessionId: "session-a", requestId: request.requestId, result: { library: null } });
  assert.deepEqual(await response, { library: null });
  f.bridge.dispose();
});
test("init accepts generic root IDs, defaults old hosts to null and rejects invalid IDs", async () => {
  const legacy = fixture(); legacy.init();
  assert.equal((await legacy.bridge.ready).context.rootId, null);
  legacy.bridge.dispose();
  for (const rootId of [0, -1, 1.5, "12", Number.MAX_SAFE_INTEGER + 1]) {
    const invalid = fixture();
    invalid.init({ rootId });
    assert.throws(() => invalid.bridge.readContext(), /not initialized/);
    invalid.init({ rootId: 12 });
    assert.equal((await invalid.bridge.ready).context.rootId, 12);
    invalid.bridge.dispose();
  }
});
test("bridge caps pre-init calls, rejects unknown methods, times out and releases capacity", async () => {
  const f = fixture(25);
  const requests = Array.from({ length: 32 }, () => f.bridge.call("core.getActiveLibrary").catch((error: Error) => error));
  await assert.rejects(f.bridge.call("core.getActiveLibrary"), /Too many/);
  await assert.rejects(f.bridge.call("core.disablePlugin"), /Unsupported/);
  const results = await Promise.all(requests);
  assert.ok(results.every((error) => error instanceof Error && /timed out/.test(error.message)));
  await assert.rejects(f.bridge.ready, /initialization timed out/);
  f.bridge.dispose();
});
test("typed adapter strips AbortSignal, preserves Core errors and rejects calls on dispose", async () => {
  const f = fixture(); f.init(); await f.bridge.ready;
  const client = createCoreClient(f.bridge);
  const controller = new AbortController();
  const response = client.getSyncStatus({ signal: controller.signal, requestPriority: "background" });
  await Promise.resolve();
  const request = f.sent[1] as { requestId: string; args: unknown[] };
  assert.deepEqual(request.args, [{ requestPriority: "background" }]);
  controller.abort();
  await assert.rejects(response, { name: "AbortError" });
  const failure = client.getPlugin("example.plugin"); await Promise.resolve();
  const next = f.sent[2] as { requestId: string };
  f.receive({ type: "mangoo-plugin-ui:response", sessionId: "session-a", requestId: next.requestId, error: { status: 409, message: "State changed", body: { code: "stale" } } });
  await assert.rejects(failure, (error: unknown) => error instanceof CoreApiError && error.status === 409 && error.body.code === "stale" && error.message === "State changed");
  const cancelled = client.getActiveLibrary(); f.bridge.dispose();
  await assert.rejects(cancelled, /closed/);
  await assert.rejects(client.getActiveLibrary(), /closed/);
});
