import { test } from "node:test";
import assert from "node:assert/strict";
import { sessionForDbPath } from "./core-session.ts";

test("sessionForDbPath swaps only dbPath, reusing bind + token", () => {
  const base = {
    baseUrl: "http://127.0.0.1:47000/api",
    bindAddress: "127.0.0.1:47000",
    dbPath: "C:/old.sqlite",
    sessionToken: "tok123",
    allowedOrigin: "http://127.0.0.1:5173"
  };
  const next = sessionForDbPath(base, "C:/new.sqlite");
  assert.equal(next.dbPath, "C:/new.sqlite");
  assert.equal(next.bindAddress, base.bindAddress);
  assert.equal(next.sessionToken, base.sessionToken);
  assert.equal(next.baseUrl, base.baseUrl);
  assert.equal(next.allowedOrigin, base.allowedOrigin);
});
