import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createServer } from "node:http";
import { once } from "node:events";
import { promisify } from "node:util";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { readTabs, retainTabs, selectTab, writeTabs } from "../../../bundled-plugins/com.mangoo.notebook/src/web/notebookTabs.ts";

test("notebook tab order, neighbour selection and deliberately empty workspaces survive refresh", () => {
  const values = new Map<string, string>();
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "sessionStorage");
  Object.defineProperty(globalThis, "sessionStorage", { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  } });
  try {
    const notes = ["a", "b", "c", "d"].map(id => ({ id }));
    let tabs = readTabs("one", notes);
    for (const { id } of notes) tabs = selectTab(tabs, id);
    tabs = selectTab(tabs, "b");
    assert.deepEqual(tabs.ids, ["a", "b", "c", "d"]);
    tabs = retainTabs(tabs, id => id !== "d");
    assert.equal(tabs.activeId, "b");
    writeTabs("one", tabs);
    assert.deepEqual(readTabs("one", notes), tabs);
    assert.deepEqual(readTabs("one", notes.filter(note => note.id !== "b")), { ids: ["a", "c"], activeId: "c" });
    assert.equal(retainTabs(tabs, id => id === "a").activeId, "a");
    writeTabs("one", { ids: [], activeId: null });
    assert.deepEqual(readTabs("one", notes), { ids: [], activeId: null });
    assert.equal(readTabs("two", notes).activeId, "a");
  } finally {
    if (descriptor) Object.defineProperty(globalThis, "sessionStorage", descriptor);
    else Reflect.deleteProperty(globalThis, "sessionStorage");
  }
});

test("two notebook writers recover large IndexedDB and legacy drafts after a real renderer reload", async t => {
  const source = fileURLToPath(new URL("../../../bundled-plugins/com.mangoo.notebook/src/web/", import.meta.url));
  const bundled = await build({
    stdin: { contents: 'export * from "./noteSession"; export * from "./noteDrafts";', resolveDir: source },
    bundle: true, write: false, format: "esm", platform: "browser",
    plugins: [{
      name: "notebook-draft-test-host",
      setup(plugin) {
        plugin.onResolve({ filter: /^@mangoo\/web\/plugin-host$/ }, args => ({ path: args.path, namespace: "host" }));
        plugin.onLoad({ filter: /.*/, namespace: "host" }, () => ({
          contents: "export class CoreApiError extends Error {}; export const createCoreClient = () => window.top.notebookTestCore;"
        }));
      }
    }]
  });
  const server = createServer((req, res) => {
    if (req.url === "/session.js") {
      res.writeHead(200, { "Content-Type": "text/javascript" }).end(bundled.outputFiles[0].contents);
    } else {
      res.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
      if (req.url === "/writer") res.end("<!doctype html><body></body>");
      else res.end('<!doctype html><body><script type="module">(' + draftScenario.toString() +
        ')().then(() => { if (!window.reloading) window.__overlayLifecycleResult={ok:true}; }, error => window.__overlayLifecycleResult={ok:false,error:String(error.stack||error)});</script></body>');
    }
  });
  server.listen(0, "127.0.0.1"); await once(server, "listening");
  t.after(() => server.close());
  const addr = server.address();
  assert.ok(addr && typeof addr === "object");
  const electron = (await import("electron")).default as unknown as string;
  const env = { ...process.env }; delete env.ELECTRON_RUN_AS_NODE;
  const { stdout } = await promisify(execFile)(electron, [
    fileURLToPath(new URL("./fixtures/runOverlayLifecycle.mjs", import.meta.url)), "http://127.0.0.1:" + addr.port + "/",
  ], { env, windowsHide: true, timeout: 60000 });
  const result = JSON.parse(stdout.slice(stdout.lastIndexOf("OVERLAY_LIFECYCLE_RESULT:") + "OVERLAY_LIFECYCLE_RESULT:".length));
  assert.equal(result.ok, true, result.error);
});

// Runs in Chromium. Each same-origin frame gets its own module instance and writer identity.
async function draftScenario() {
  const host = window as any;
  const moduleUrl = "/session.js";
  const api = await import(moduleUrl);
  const check = (value: unknown, why: string) => { if (!value) throw Error(why); };
  const scope = { libraryId: "library", contextRevision: 4 };
  const large = "甲a".repeat(3 * 1024 * 1024);
  let remote = JSON.parse(sessionStorage.getItem("remote") || "null") || {
    id: "note", title: "Notebook", markdown: "original", format: "markdown", styles: {},
    revision: 1, createdAt: 1, updatedAt: 1, attachments: [],
  };
  host.notebookTestCore = {
    getNote: async () => remote,
    updateNote: async (_id: string, input: { expectedRevision: number }, options: typeof scope) => {
      check(options.libraryId === scope.libraryId && options.contextRevision === scope.contextRevision, "Lost active context");
      if (input.expectedRevision !== remote.revision) throw Error("Unexpected revision");
      remote = { ...remote, ...input, revision: remote.revision + 1 };
      return remote;
    },
  };
  const open = async (runtime: typeof api) => {
    const session = new runtime.NoteSession(scope, remote, () => {});
    await session.ready; return session;
  };
  if (!sessionStorage.getItem("remote")) {
    const frame = document.createElement("iframe");
    const loaded = new Promise(resolve => { frame.onload = resolve; });
    frame.src = "/writer"; document.body.append(frame); await loaded;
    const secondApi = await (frame.contentWindow as any).eval('import("/session.js")');
    const first = await open(api), second = await open(secondApi);
    first.edit({ markdown: large }); await first.waitForDraft();
    check(first.draftOnDisk && !first.draftError, "Large draft failed to persist");
    first.dispose();
    let observed = false;
    for (let n = 0; n < 100 && !observed; n++) {
      observed = second.otherDrafts.some((draft: any) => draft.preview.startsWith("甲a"));
      if (!observed) await new Promise(resolve => setTimeout(resolve, 10));
    }
    check(observed, "Second writer missed first writer's IndexedDB notification");
    second.edit({ markdown: "window B" }); await second.waitForDraft();
    check((await api.readDrafts(scope.libraryId, "note")).length === 2, "One writer overwrote another draft");
    await second.flush(); second.dispose();
    check(remote.markdown === "window B", "Second window did not save");
    sessionStorage.setItem("remote", JSON.stringify(remote));
    const key = "mangoo.notebook.draft:library:removed-note";
    localStorage.setItem(key, JSON.stringify({ revision: 1, fields: { title: "Legacy", markdown: "old draft", styles: {}, format: "markdown" } }));
    check(localStorage.length === 1, "Large drafts unexpectedly used localStorage");
    host.reloading = true; location.reload(); return;
  }

  const recovery = await open(api);
  check(remote.markdown === "window B" && recovery.otherDrafts.length === 1, "Reload lost version choices");
  const old = recovery.otherDrafts[0];
  check(!("fields" in old), "Draft enumeration loaded a full body");
  await recovery.restoreOtherDraft(old);
  check(recovery.status === "conflict" && recovery.note.markdown === large, "Large conflicted draft did not round-trip");
  const orphans = await api.orphanDrafts(scope.libraryId, new Set(["note"]));
  check(orphans.length === 1 && (await api.loadDraft(scope.libraryId, orphans[0])).fields.markdown === "old draft", "Legacy orphan could not be recovered");
  await api.removeDraft(scope.libraryId, orphans[0].id, orphans[0].key);
  await recovery.resolve(true);
  check(remote.markdown === large && remote.revision === 3, "Recovered content not committed against latest revision");
  check((await api.readDrafts(scope.libraryId)).length === 0, "Committed draft left a stale competing version");
  recovery.refresh({ ...remote, revision: 1, markdown: "stale" });
  check(recovery.note.markdown === large, "Older response replaced a newer edit");
  recovery.dispose();

  const quota = await open(api);
  quota.edit({ markdown: "last durable version" }); await quota.waitForDraft();
  const put = IDBObjectStore.prototype.put;
  try {
    IDBObjectStore.prototype.put = function (...args: Parameters<typeof put>) {
      if (this.name === "bodies") throw new DOMException("Disk full", "QuotaExceededError");
      return put.apply(this, args);
    };
    quota.edit({ markdown: "unsaved newer edit" }); await quota.waitForDraft();
    check(!quota.draftOnDisk && quota.draftError.includes("空间"), "Failed write treated as durable");
    const previous = await api.readDrafts(scope.libraryId, "note");
    check(previous.length === 1 && (await api.loadDraft(scope.libraryId, previous[0])).fields.markdown === "last durable version",
      "Failed replacement destroyed the last durable version");
  } finally { IDBObjectStore.prototype.put = put; }
  await quota.flush(); quota.dispose();
  check(remote.markdown === "unsaved newer edit", "Draft quota failure blocked remote saving");

  const earlier = await api.writeDraft(scope.libraryId, "note", "older-writer", undefined, 1,
    { title: "Notebook", markdown: "earlier draft", styles: {}, format: "markdown" });
  const editing = await open(api);
  await editing.restoreOtherDraft(editing.otherDrafts[0]);
  editing.edit({ markdown: "recovery with amendments" }); await editing.waitForDraft(); editing.dispose();
  const freshFrame = document.createElement("iframe");
  const frameReady = new Promise(resolve => { freshFrame.onload = resolve; });
  freshFrame.src = "/writer"; document.body.append(freshFrame); await frameReady;
  const another = await open(await (freshFrame.contentWindow as any).eval('import("/session.js")'));
  const amended = another.otherDrafts.find((draft: any) => draft.preview === "recovery with amendments");
  check(amended?.revision === 1, "Amended recovery lost its original base revision");
  await another.restoreOtherDraft(amended);
  check(another.status === "conflict", "A fresh writer silently replaced a newer remote version");
  await another.resolve(true); another.dispose();
  const remaining = await api.readDrafts(scope.libraryId, "note");
  check(remaining.length === 1 && remaining[0].key === earlier && remote.markdown === "recovery with amendments",
    "Resolving an amended recovery cleared a different immutable version");
}
