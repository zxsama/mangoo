import assert from "node:assert/strict";
import { test } from "node:test";
import { createCoreClient } from "../src/client.ts";

test("playback grants keep auth/context in headers and resolve only restricted media URLs", async () => {
  const original = globalThis.fetch;
  const calls: { url: string; options?: RequestInit }[] = [];
  const session = { id: "grant", src: `/playback/${"a".repeat(43)}`, expiresAt: Date.now() + 60_000, renewAfterMs: 30_000 };
  try {
    globalThis.fetch = async (input, options) => {
      calls.push({ url: String(input), options });
      return options?.method === "DELETE" ? new Response(null, { status: 204 }) :
        new Response(JSON.stringify(session), { status: 200, headers: { "x-mangoo-context-revision": "7" } });
    };
    const client = createCoreClient({ baseUrl: "http://localhost:1234/api", sessionToken: "fixture-auth" });
    const first = await client.createPlaybackSession(12);
    assert.equal(first.src, `http://localhost:1234/api${session.src}`);
    assert.equal(new Headers(calls[0].options?.headers).get("x-mangoo-session"), "fixture-auth");
    await client.renewPlaybackSession("grant/slash");
    assert.equal(calls[1].url, "http://localhost:1234/api/playback-sessions/grant%2Fslash/renew");
    assert.equal(new Headers(calls[1].options?.headers).get("x-mangoo-context-revision"), "7");
    await client.revokePlaybackSession("grant/slash");
    assert.equal(calls[2].options?.method, "DELETE");
    assert.equal(new Headers(calls[2].options?.headers).get("x-mangoo-context-revision"), "7");
    assert.ok(calls.every((call) => !call.url.includes("fixture-auth")));
    for (const src of ["https://elsewhere.invalid/file", "/media/1/preview?session=fixture-auth", "/playback/short", "/playback/" + "a".repeat(43) + "?other=1"]) {
      session.src = src;
      await assert.rejects(client.createPlaybackSession(1), /Invalid playback session/);
    }
    const abort = new AbortController(); abort.abort();
    const before = calls.length;
    await assert.rejects(client.createPlaybackSession(1, { signal: abort.signal }), { name: "AbortError" });
    assert.equal(calls.length, before);
  } finally { globalThis.fetch = original; }
});
