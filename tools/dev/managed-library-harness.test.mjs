import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import { createAndActivateManagedLibrary, resolveCoreBinary } from "./managed-library-harness.mjs";

test("Core binary selection honors explicit binaries before the isolated Cargo target", () => {
  const root = path.resolve("fixture-project");
  const executable = process.platform === "win32" ? "mangoo-core.exe" : "mangoo-core";
  assert.equal(resolveCoreBinary({ root, env: {} }), path.join(root, "target", "debug", executable));
  assert.equal(resolveCoreBinary({ root, env: { CARGO_TARGET_DIR: "isolated-target" } }), path.join(root, "isolated-target", "debug", executable));
  assert.equal(resolveCoreBinary({ root, env: { CARGO_TARGET_DIR: "ignored", MANGOO_CORE_BINARY: "explicit-core" } }), path.join(root, "explicit-core"));
  assert.equal(resolveCoreBinary({ root, coreBinary: "cli-core", env: { MANGOO_CORE_BINARY: "ignored" } }), path.join(root, "cli-core"));
});

test("creation submits the normalized whole plan and re-reads the active revision", async (t) => {
  const requests = [];
  const contentRoots = [{ path: "/existing", createIfMissing: false }, { path: "/new/Files", createIfMissing: true }];
  const normalizedContentRoots = [{ path: "/normalized/existing", createIfMissing: false }, { path: "/normalized/new/Files", createIfMissing: true }];
  const responses = [
    { ok: true, normalizedDatabasePath: "/normalized/Database", normalizedContentRoots, preflightToken: "plan" },
    { activated: true, library: { libraryUuid: "new-library", contextRevision: 7 } },
    { library: { libraryUuid: "new-library" }, contextRevision: 8 }
  ];
  t.mock.method(globalThis, "fetch", async (url, init) => {
    requests.push({ url, init });
    return Response.json(responses.shift());
  });
  const created = await createAndActivateManagedLibrary({ baseUrl: "http://localhost/api", token: "fixture", name: "Example", databasePath: "/Database", contentRoots });
  assert.deepEqual(JSON.parse(requests[0].init.body), { name: "Example", databasePath: "/Database", contentRoots });
  assert.deepEqual(JSON.parse(requests[1].init.body), {
    name: "Example", databasePath: "/normalized/Database", contentRoots: normalizedContentRoots,
    initialMode: "local-only", preflightToken: "plan", activate: true
  });
  assert.equal(requests[2].url, "http://localhost/api/libraries/active");
  assert.equal(created.contextRevision, 8);
  assert.ok(requests.every(({ init }) => init.headers.get("x-mangoo-session") === "fixture"));
});

test("rejected or incomplete normalized plans never reach creation", async (t) => {
  for (const preflight of [
    { ok: false, normalizedDatabasePath: null, normalizedContentRoots: null, preflightToken: null },
    { ok: true, normalizedDatabasePath: "/Database", normalizedContentRoots: null, preflightToken: "plan" }
  ]) {
    let requests = 0;
    t.mock.method(globalThis, "fetch", async () => { requests += 1; return Response.json(preflight); });
    await assert.rejects(createAndActivateManagedLibrary({ baseUrl: "http://localhost/api", token: "fixture", name: "Example", databasePath: "/Database", contentRoots: [] }), /preflight rejected/);
    assert.equal(requests, 1);
    t.mock.restoreAll();
  }
});
