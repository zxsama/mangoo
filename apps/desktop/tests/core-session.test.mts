import assert from "node:assert/strict";
import crypto from "node:crypto";
import net from "node:net";
import { afterEach, beforeEach, mock, test } from "node:test";
import { createCoreSession } from "../src/core-session.ts";

const envKeys = ["MANGOO_CORE_EXTERNAL", "MANGOO_CORE_URL", "MANGOO_CORE_ADDR", "MANGOO_SERVE_WEB", "MANGOO_WEB_URL", "MANGOO_ALLOWED_ORIGIN"];
const saved = new Map<string, string | undefined>();
beforeEach(() => {
  for (const key of envKeys) { saved.set(key, process.env[key]); delete process.env[key]; }
});
afterEach(() => {
  mock.restoreAll();
  for (const [key, value] of saved) {
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
});

test("allocated Core port is browser-usable and packaged origin matches it", async () => {
  process.env.MANGOO_SERVE_WEB = "1";
  const session = await createCoreSession();
  const port = Number(new URL(session.baseUrl).port);
  assert.ok(port >= 49152 && port <= 65535);
  assert.equal(session.bindAddress, `127.0.0.1:${port}`);
  assert.equal(session.allowedOrigin, `http://127.0.0.1:${port}`);
});

test("explicit Core address is preserved without allocating another port", async () => {
  process.env.MANGOO_CORE_ADDR = "127.0.0.1:19517";
  mock.method(crypto, "randomInt", () => { throw new Error("unexpected allocation"); });
  const session = await createCoreSession();
  assert.equal(session.baseUrl, "http://127.0.0.1:19517/api");
});

test("occupied random Core candidate is retried and exhaustion is bounded", async (t) => {
  const available = await createCoreSession();
  const occupiedPort = Number(new URL(available.baseUrl).port);
  const server = net.createServer();
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(occupiedPort, "127.0.0.1", resolve);
  });
  t.after(() => new Promise<void>((resolve) => server.close(() => resolve())));
  const originalRandomInt = crypto.randomInt;
  let calls = 0;
  const choose = mock.method(crypto, "randomInt", () => ++calls === 1 ? occupiedPort : originalRandomInt(49152, 65536));
  const retried = await createCoreSession();
  assert.notEqual(Number(new URL(retried.baseUrl).port), occupiedPort);
  assert.ok(calls >= 2);
  choose.mock.restore();
  const exhausted = mock.method(crypto, "randomInt", () => occupiedPort);
  await assert.rejects(createCoreSession(), /after 128 attempts/);
  assert.equal(exhausted.mock.callCount(), 128);
});
