import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { createServer } from "node:http";
import { createCoreClient } from "@mangoo/core-client/node";
import { isSafePluginExternalUrl, openPluginExternalLink, parsePluginExternalLinkInput } from "../src/plugin-external-link.ts";
import type { PluginExternalLink } from "@mangoo/core-client";

const request = { pluginId: "org.fixture.storage", kind: "authorization" as const, id: "flow-1" };
const link: PluginExternalLink = { kind: "authorization", id: "flow-1", url: "https://accounts.other.example/sign-in?state=opaque", expiresAt: Date.now() + 60_000 };

describe("Host-authorized generic external links", () => {
  it("resolves through the Desktop Core client with session authentication and identifier-only POST", async () => {
    let received: unknown;
    const server = createServer(async (request, response) => {
      const chunks: Buffer[] = [];
      for await (const chunk of request) chunks.push(Buffer.from(chunk));
      received = { method: request.method, path: request.url, session: request.headers['x-mangoo-session'], body: JSON.parse(Buffer.concat(chunks).toString()) };
      response.writeHead(200, { 'Content-Type': 'application/json' }); response.end(JSON.stringify(link));
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    try {
      const address = server.address(); assert.ok(address && typeof address === 'object');
      const client = createCoreClient({ baseUrl: `http://127.0.0.1:${address.port}/api`, sessionToken: 'fixture-session' });
      let opened: string | null = null;
      assert.deepEqual(await openPluginExternalLink(request, client.resolvePluginExternalLink, async (url) => { opened = url; }), { ok: true });
      assert.equal(opened, link.url);
      assert.deepEqual(received, { method: 'POST', path: '/api/plugins/org.fixture.storage/external-link', session: 'fixture-session', body: { kind: 'authorization', id: 'flow-1' } });
    } finally { await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve())); }
  });
  it("accepts only bounded plugin/kind/id input and rejects renderer URLs", () => {
    assert.deepEqual(parsePluginExternalLinkInput(request), request);
    for (const value of [link.url, null, [], { ...request, url: link.url }, { ...request, pluginId: "../other" }, { ...request, id: "x".repeat(129) }, { ...request, kind: "file" }]) assert.equal(parsePluginExternalLinkInput(value), null);
  });
  it("opens the exact Host response for different Provider auth and declared help links", async () => {
    const opened: string[] = [], resolved: unknown[] = [];
    for (const kind of ["authorization", "help"] as const) {
      assert.deepEqual(await openPluginExternalLink({ ...request, kind }, async (pluginId, input) => { resolved.push([pluginId, input]); return { ...link, kind, url: kind === "help" ? "https://docs.other.example/setup" : link.url }; }, async (url) => { opened.push(url); }), { ok: true });
    }
    assert.deepEqual(opened, [link.url, "https://docs.other.example/setup"]);
    assert.deepEqual(resolved, [[request.pluginId, { kind: "authorization", id: request.id }], [request.pluginId, { kind: "help", id: request.id }]]);
  });
  it("rejects unsafe destination syntax and enables only explicit loopback debugging", () => {
    for (const value of ["http://example.test/", "javascript:alert(1)", "file:///tmp/a", "https://user:pass@example.test", "https://example.test/#", "https://example.test/#fragment", "https://example.test/\n", "https://example.test/%0a", "https://example.test/\\bad", `https://example.test/${"x".repeat(8192)}`, "http://localhost:4000/auth", "https://127.0.0.1/auth"]) assert.equal(isSafePluginExternalUrl(value), false, value.slice(0, 90));
    assert.equal(isSafePluginExternalUrl("http://127.0.0.1:4000/auth", true), true);
    assert.equal(isSafePluginExternalUrl("http://remote.test/auth", true), false);
  });
  it("rejects unknown, expired, wrong-kind, wrong-id and stale Core responses before opening", async () => {
    let opens = 0;
    for (const resolved of [{ ...link, id: "another" }, { ...link, kind: "help" as const }, { ...link, expiresAt: Date.now() - 1 }, { ...link, expiresAt: null }, { ...link, url: "http://remote.test" }]) {
      assert.deepEqual(await openPluginExternalLink(request, async () => resolved, async () => { opens++; }), { ok: false, error: "external_browser_rejected" });
    }
    await openPluginExternalLink(request, async () => { throw new Error("disabled/stale"); }, async () => { opens++; });
    await openPluginExternalLink(request, async () => link, async () => { opens++; }, { isCurrent: () => false });
    assert.equal(opens, 0);
  });
  it("grants focus only for successful current authorization and reports browser failure", async () => {
    let grants = 0;
    const options = { onAuthorizationOpened: () => { grants++; } };
    assert.deepEqual(await openPluginExternalLink(request, async () => link, async () => { throw new Error("browser failure"); }, options), { ok: false, error: "external_browser_open_failed" });
    await openPluginExternalLink({ ...request, kind: "help" }, async () => ({ ...link, kind: "help" }), async () => {}, options);
    let current = true;
    await openPluginExternalLink(request, async () => link, async () => { current = false; }, { ...options, isCurrent: () => current });
    assert.equal(grants, 0);
    await openPluginExternalLink(request, async () => link, async () => {}, options);
    assert.equal(grants, 1);
  });
  it("rejects malformed Host JSON and non-finite expiry without opening", async () => {
    let opens = 0;
    for (const response of [null, undefined, {}, { ...link, expiresAt: undefined }, { ...link, expiresAt: "2999999999999" }, { ...link, expiresAt: NaN }, { ...link, expiresAt: Infinity }, { ...link, expiresAt: Number.MAX_SAFE_INTEGER + 1 }]) {
      const malformed = response as PluginExternalLink;
      assert.deepEqual(await openPluginExternalLink(request, async () => malformed, async () => { opens++; }), { ok: false, error: "external_browser_rejected" });
    }
    assert.equal(opens, 0);
  });
});
