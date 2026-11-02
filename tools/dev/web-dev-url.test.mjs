import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import http from "node:http";
import test from "node:test";
import { availableLoopbackPort, isMangooWebServerReady, resolveWebDevUrl, waitForMangooWebServer } from "./web-dev-url.mjs";

test("automatic ports stay browser-usable and retry only unavailable candidates", async () => {
  const attempts = [];
  const port = await availableLoopbackPort(0, async (candidate) => {
    attempts.push(candidate);
    if (attempts.length < 3) throw Object.assign(new Error("unavailable"), {code: attempts.length === 1 ? "EACCES" : "EADDRINUSE"});
    return candidate;
  });
  assert.equal(attempts.length, 3);
  assert.ok(attempts.every((candidate) => candidate >= 49152 && candidate <= 65535));
  assert.equal(port, attempts[2]);
  const actual = await availableLoopbackPort(0);
  assert.ok(actual >= 49152 && actual <= 65535);
  assert.equal(await availableLoopbackPort(5173, async (candidate) => candidate), 5173);
});

test("automatic port retries are bounded and unexpected errors propagate immediately", async () => {
  let attempts = 0;
  const unavailable = Object.assign(new Error("occupied"), {code: "EADDRINUSE"});
  await assert.rejects(availableLoopbackPort(0, async () => { attempts += 1; throw unavailable; }), (error) => error === unavailable);
  assert.equal(attempts, 128);
  attempts = 0;
  await assert.rejects(availableLoopbackPort(0, async () => { attempts += 1; throw Object.assign(new Error("invalid"), {code: "EINVAL"}); }), {code: "EINVAL"});
  assert.equal(attempts, 1);
});

test("explicit web address stays unchanged without probing or reuse", async () => {
  const unexpected = () => { throw new Error("unexpected probe"); };
  assert.equal(await resolveWebDevUrl({configuredUrl: "http://127.0.0.1:5210", isReady: unexpected, probePort: unexpected}), "http://127.0.0.1:5210");
});

test("existing Mangoo server is reused without binding", async () => {
  assert.equal(await resolveWebDevUrl({configuredUrl: "", isReady: async () => true, probePort: () => assert.fail("must not bind")}), "http://127.0.0.1:5173");
});

test("free default stays stable; reserved and occupied defaults select a free port", async () => {
  for (const code of [null, "EACCES", "EADDRINUSE"]) {
    const attempts = [];
    const logs = [];
    const result = await resolveWebDevUrl({
      configuredUrl: "", isReady: async () => false, log: (line) => logs.push(line),
      probePort: async (port) => {
        attempts.push(port);
        if (port && code) throw Object.assign(new Error(code), {code});
        return port || 19517;
      }
    });
    assert.equal(result, `http://127.0.0.1:${code ? 19517 : 5173}`);
    assert.deepEqual(attempts, code ? [5173, 0] : [5173]);
    assert.equal(logs.length, code ? 1 : 0);
    if (code) assert.match(logs[0], new RegExp(code));
  }
});

test("unexpected bind failures remain errors", async () => {
  await assert.rejects(resolveWebDevUrl({ configuredUrl: "", isReady: async () => false, probePort: async () => { throw Object.assign(new Error("bind failed"), {code: "EINVAL"}); } }), {code: "EINVAL"});
});

test("readiness requires Mangoo HTML; an unrelated successful response is rejected", async (t) => {
  let contentType = "application/json";
  let body = "{}";
  const server = http.createServer((_request, response) => { response.setHeader("Content-Type", contentType); response.end(body); });
  server.listen(await availableLoopbackPort(0), "127.0.0.1");
  await new Promise((resolve) => server.once("listening", resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const url = `http://127.0.0.1:${server.address().port}`;
  assert.equal(await isMangooWebServerReady(url), false);
  contentType = "text/html";
  body = "<title>Other app</title>";
  assert.equal(await isMangooWebServerReady(url), false);
  body = '<title>Mangoo</title><script src="/src/main.tsx"></script>';
  assert.equal(await isMangooWebServerReady(url), true);
});

test("readiness stops promptly when Vite exits and retains spawn failures", async () => {
  const child = Object.assign(new EventEmitter(), {exitCode: 1, signalCode: null});
  await assert.rejects(waitForMangooWebServer("http://127.0.0.1:5173", child), /exited before becoming ready.*exit 1/);
  child.exitCode = null;
  await assert.rejects(waitForMangooWebServer("http://127.0.0.1:5173", child, {
    isReady: async () => { child.emit("error", Object.assign(new Error("spawn failed"), {code: "ENOENT"})); return false; }
  }), {code: "ENOENT"});
  assert.equal(child.listenerCount("error"), 0);
});
