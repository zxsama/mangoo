import assert from "node:assert/strict";
import { test } from "node:test";
import { createCoreClient } from "../src/client.ts";

test("notebook directory reads and moves carry the same library and optimistic parent fence", async (t) => {
  const calls: Request[] = [];
  t.mock.method(globalThis, "fetch", async (url: RequestInfo | URL, init?: RequestInit) => {
    const req = new Request(url, init);
    calls.push(req);
    return req.method === "DELETE" || req.url.endsWith("/folder")
      ? new Response(null, { status: 204 }) : Response.json({});
  });
  const client = createCoreClient({ baseUrl: "http://localhost/api", sessionToken: "local" });
  const scope = { libraryId: "active", contextRevision: 12 };
  await client.getNotebookTree(scope);
  await client.createNotebookFolder({ name: "数据", parentId: null }, scope);
  await client.updateNotebookFolder("child", { name: "索引", parentId: "parent", expectedRevision: 2 }, scope);
  await client.moveNotebookNote("note", { parentId: "child", expectedParentId: null }, scope);
  await client.deleteNotebookFolder("child", 4, scope);
  assert.ok(calls.every(req => req.headers.get("x-mangoo-context-revision") === "12" &&
    req.headers.get("x-mangoo-session") === "local"));
  assert.deepEqual(calls.map(req => req.method + " " + req.url.split("/api/notebooks/active/")[1]), [
    "GET tree", "POST folders", "PUT folders/child", "PUT notes/note/folder", "DELETE folders/child?expectedRevision=4",
  ]);
  assert.deepEqual(await calls[3].json(), { parentId: "child", expectedParentId: null });
  assert.equal(calls[3].headers.get("content-type"), "application/json");
});

test("notebook reads, mutations and original binary bytes share the caller's library fence", async (t) => {
  const calls: Request[] = [];
  const bytes = new Uint8Array([0, 255, 2, 254]);
  t.mock.method(globalThis, "fetch", async (url: RequestInfo | URL, init?: RequestInit) => {
    const request = new Request(url, init);
    calls.push(request);
    if (request.method === "DELETE") return new Response(null, { status: 204 });
    if (request.url.endsWith("/attachments/image")) return new Response(bytes, { headers: { "content-type": "image/png" } });
    return Response.json({}, { headers: { "x-mangoo-context-revision": "9" } });
  });
  const client = createCoreClient({ baseUrl: "http://localhost/api", sessionToken: "local" });
  const scope = { libraryId: "active-id", contextRevision: 7 };
  const input = { title: "笔记", markdown: "<p>Hello</p>", styles: {}, format: "html" as const };
  await client.listNotes({ ...scope, query: "中文 & %" });
  await client.createNote(input, scope);
  await client.updateNote("note", { ...input, expectedRevision: 3 }, scope);
  const file = new File([bytes], "剪贴 图.png", { type: "image/png" });
  await client.uploadNoteAttachment("note", file, scope);
  const blob = await client.getNoteAttachment("note", "image", scope);
  await client.deleteNote("note", 4, scope);
  assert.ok(calls.every(req => req.headers.get("x-mangoo-context-revision") === "7"));
  assert.ok(calls.every(req => req.headers.get("x-mangoo-session") === "local"));
  assert.ok(calls.every(req => new URL(req.url).pathname.startsWith("/api/notebooks/active-id/notes")));
  assert.equal((await calls[2].json()).expectedRevision, 3);
  assert.equal(new URL(calls[0].url).searchParams.get("query"), "中文 & %");
  assert.equal(new URL(calls[3].url).searchParams.get("name"), file.name);
  assert.deepEqual(new Uint8Array(await calls[3].arrayBuffer()), bytes);
  assert.deepEqual(new Uint8Array(await blob.arrayBuffer()), bytes);
  assert.equal(new URL(calls[5].url).searchParams.get("expectedRevision"), "4");
});
