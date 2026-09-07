import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";
import { createServer, type ViteDevServer } from "vite";

interface Result {
  ok: boolean;
  error?: string;
  duplicateRows: boolean;
  localOnlyGated: boolean;
  liveRegion: boolean;
  failureVisible: boolean;
  duplicateBlocked: boolean;
  successClosed: boolean;
  focusRestored: boolean;
  escapeClosed: boolean;
  compact: boolean;
  longTextWrapped: boolean;
  renameCommitted: boolean;
  rebindCommitted: boolean;
}

const webRoot = fileURLToPath(new URL("../", import.meta.url));
const fixtureDirectory = join(webRoot, ".tmp", "sync-folders-mounted");
const fixtureHtmlPath = join(fixtureDirectory, "fixture.html");
const fixtureSourcePath = join(fixtureDirectory, "fixture.tsx");
const electronRunner = fileURLToPath(new URL("./fixtures/runOverlayLifecycle.mjs", import.meta.url));
const resultPrefix = "OVERLAY_LIFECYCLE_RESULT:";
let server: ViteDevServer;
let result: Result;

const fixtureSource = String.raw`
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { OverlayProvider } from "/src/design/overlays/index.ts";
import { SyncFoldersPanel } from "/src/features/sync/SyncFoldersPanel.tsx";
import "/src/styles.css";
import "/src/design/overlays/overlay.css";

const libraryUuid = "018f26a2-c25e-7e51-9a82-42866f27d504";
const firstRoot = "018f26a2-c25e-7e51-9a82-42866f27d505";
const secondRoot = "018f26a2-c25e-7e51-9a82-42866f27d506";
let stopCommitCount = 0;
let renameCommitCount = 0;
let rebindCommitCount = 0;
let releaseFirstCommit;
const firstCommitGate = new Promise((resolve) => { releaseFirstCommit = resolve; });

const descriptor = (rootUuid) => ({ libraryUuid, libraryRootUuid: "018f26a2-c25e-7e51-9a82-42866f27d507", rootUuid, rootFolderUuid: "018f26a2-c25e-7e51-9a82-42866f27d508", requestedName: "A very long synchronized folder name that must remain readable without clipping", materializedName: "Artists", materializationFingerprint: "fingerprint", logicalRevision: 4, lifecycle: "active", inventoryState: "ready", inventoryRevision: 8, operationRevision: 3, logicalBytes: 2048, checkpointUuid: "018f26a2-c25e-7e51-9a82-42866f27d509" });
const binding = (state) => ({ localBindingUuid: "018f26a2-c25e-7e51-9a82-42866f27d510", localPath: "D:\\\\A very long path segment that must wrap safely\\\\Artists", placementParent: "D:\\\\", state, bindingGeneration: 2, desiredMaterializedName: "Artists", desiredMaterializationFingerprint: "fingerprint", appliedMaterializedName: "Artists", appliedMaterializationFingerprint: "fingerprint", operationRevision: 3, lastErrorCode: state === "local-only-fork" ? "a-very-long-error-code-that-must-wrap" : null });
const folders = [
  { descriptor: descriptor(firstRoot), binding: binding("active"), allowedActions: ["pause", "stop-sync", "relocate", "rename", "rebind-existing"], locationHeads: ["018f26a2-c25e-7e51-9a82-42866f27d520"], rebindEvidence: { markerIdentity: "opaque-marker", priorCutoverSnapshotUuid: "018f26a2-c25e-7e51-9a82-42866f27d521" } },
  { descriptor: descriptor(secondRoot), binding: binding("local-only-fork"), allowedActions: ["re-enable", "publish-as-new"] }
];

window.megleDesktop = { pickFolder: async () => "D:\\Existing\\Artists" };

window.fetch = async (input, init = {}) => {
  const url = String(input);
  if (url.endsWith("/libraries/" + libraryUuid + "/sync-folders")) return Response.json({ revision: 1, items: folders });
  if (url.endsWith("/libraries/active")) return Response.json({ contextRevision: 11, library: null });
  if (url.endsWith("/rename/preflight")) return Response.json({ planUuid: "018f26a2-c25e-7e51-9a82-42866f27d522", requestedName: "Renamed", materializedName: "Renamed", materializationFingerprint: "renamed-fingerprint", expectedLocationHeads: folders[0].locationHeads, supersedes: folders[0].locationHeads, expiresAt: Date.now() + 60000 });
  if (url.endsWith("/rename/commit")) { renameCommitCount += 1; return Response.json({ operationUuid: "018f26a2-c25e-7e51-9a82-42866f27d523", libraryUuid, rootUuid: firstRoot, kind: "logical-rename", state: "completed", phase: "completed", operationRevision: 4, cutoverCommitted: true, createdAt: Date.now(), updatedAt: Date.now() }, { status: 202 }); }
  if (url.endsWith("/rebind/preflight")) return Response.json({ planUuid: "018f26a2-c25e-7e51-9a82-42866f27d524", rootUuid: firstRoot, path: "D:\\Existing\\Artists", markerMatches: true, contentEvidenceDigest: "opaque-content-evidence", expiresAt: Date.now() + 60000 });
  if (url.endsWith("/rebind/commit")) { rebindCommitCount += 1; return Response.json({ operationUuid: "018f26a2-c25e-7e51-9a82-42866f27d525", libraryUuid, rootUuid: firstRoot, kind: "rebind", state: "completed", phase: "completed", operationRevision: 4, cutoverCommitted: true, createdAt: Date.now(), updatedAt: Date.now() }, { status: 202 }); }
  if (url.endsWith("/stop/preflight")) return Response.json({ planUuid: "018f26a2-c25e-7e51-9a82-42866f27d511", operationUuid: "018f26a2-c25e-7e51-9a82-42866f27d512", rootUuid: firstRoot, localBindingUuid: folders[0].binding.localBindingUuid, bindingGeneration: 2, rootCaptureEpoch: "1", rootActorEpoch: "1", pendingWork: 0, publishedWork: 0, checkpointUuid: null, eventFrontier: {}, catalogSnapshotDigest: "digest", providerReceipts: [], expiresAt: Date.now() + 60000 });
  if (url.endsWith("/stop/commit")) {
    stopCommitCount += 1;
    if (stopCommitCount === 1) {
      await firstCommitGate;
      return Response.json({ code: "operation-in-progress", message: "A deliberately long failure message remains visible and wraps in the live error region" }, { status: 409 });
    }
    return Response.json({ operationUuid: "018f26a2-c25e-7e51-9a82-42866f27d512", libraryUuid, rootUuid: firstRoot, kind: "stop-sync", state: "completed", phase: "completed", operationRevision: 4, cutoverCommitted: true, createdAt: Date.now(), updatedAt: Date.now() }, { status: 202 });
  }
  throw new Error("Unexpected request " + (init.method ?? "GET") + " " + url);
};

const delay = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));
const waitFor = async (condition, label) => { for (let i = 0; i < 100; i += 1) { if (condition()) return; await delay(10); } throw new Error("Timed out waiting for " + label); };
const findButton = (text) => Array.from(document.querySelectorAll("button")).find((button) => button.textContent?.includes(text));

async function run() {
  try {
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    await act(async () => root.render(<QueryClientProvider client={client}><OverlayProvider><SyncFoldersPanel libraryUuid={libraryUuid} /></OverlayProvider></QueryClientProvider>));
    await waitFor(() => document.querySelectorAll(".settings-sync-folder-row").length === 2, "rows");
    const duplicateRows = document.querySelectorAll(".settings-sync-folder-name code").length === 2;
    const rows = Array.from(document.querySelectorAll(".settings-sync-folder-row"));
    const localOnlyGated = !rows[1].textContent.includes("选择位置") && !rows[1].textContent.includes("重新绑定");
    const liveRegion = document.querySelector('.settings-sync-folder-list[aria-live="polite"]') !== null;
    await act(async () => findButton("重命名").click());
    await waitFor(() => findButton("预览名称变更"), "rename dialog");
    const nameInput = document.querySelector('.sync-folder-name-field input');
    await act(async () => { const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set; setter.call(nameInput, "Renamed"); nameInput.dispatchEvent(new Event("input", { bubbles: true })); });
    await act(async () => findButton("预览名称变更").click());
    await waitFor(() => findButton("确认重命名"), "rename preview");
    await act(async () => findButton("确认重命名").click());
    await waitFor(() => renameCommitCount === 1 && !document.getElementById("sync-folder-dialog"), "rename commit");
    await act(async () => findButton("重新绑定现有副本").click());
    await waitFor(() => findButton("确认重新绑定"), "rebind preview");
    await act(async () => findButton("确认重新绑定").click());
    await waitFor(() => rebindCommitCount === 1 && !document.getElementById("sync-folder-dialog"), "rebind commit");
    const trigger = findButton("停止同步");
    trigger.focus();
    await act(async () => trigger.click());
    await waitFor(() => findButton("保留本机副本并停止"), "stop dialog");
    const submit = findButton("保留本机副本并停止");
    await act(async () => { submit.click(); submit.click(); });
    await delay(30);
    await waitFor(() => stopCommitCount === 1, "first stop commit");
    const duplicateBlocked = stopCommitCount === 1;
    releaseFirstCommit();
    await waitFor(() => document.querySelector('[role="alert"]')?.textContent?.includes("Core API request failed"), "failure");
    const failureVisible = true;
    await act(async () => findButton("保留本机副本并停止").click());
    await waitFor(() => document.getElementById("sync-folder-dialog") === null, "successful close");
    const successClosed = document.getElementById("sync-folder-dialog") === null;
    await delay(20);
    const focusRestored = document.activeElement === trigger;
    await act(async () => trigger.click());
    await waitFor(() => document.getElementById("sync-folder-dialog"), "reopened dialog");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await waitFor(() => document.getElementById("sync-folder-dialog") === null, "escape close");
    const escapeClosed = document.activeElement === trigger;
    window.resizeTo(640, 600);
    await delay(60);
    const compact = getComputedStyle(rows[0]).gridTemplateColumns.split(" ").length === 1;
    const path = rows[0].querySelector(".settings-sync-folder-path");
    const longTextWrapped = getComputedStyle(path).overflowWrap === "anywhere";
    window.__overlayLifecycleResult = { ok: true, duplicateRows, localOnlyGated, liveRegion, failureVisible, duplicateBlocked, successClosed, focusRestored, escapeClosed, compact, longTextWrapped, renameCommitted: renameCommitCount === 1, rebindCommitted: rebindCommitCount === 1 };
  } catch (error) {
    window.__overlayLifecycleResult = { ok: false, error: error instanceof Error ? error.stack ?? error.message : String(error) };
  }
}
void run();
`;

async function runFixture(url: string): Promise<Result> {
  const electronPath = (await import("electron")).default as unknown as string;
  const environment = { ...process.env };
  delete environment.ELECTRON_RUN_AS_NODE;
  return await new Promise((resolve, reject) => {
    const child = spawn(electronPath, [electronRunner, url], { cwd: webRoot, env: environment, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    const timeout = setTimeout(() => { child.kill(); reject(new Error(`sync-folder fixture timed out\n${stderr}`)); }, 45_000);
    child.stdout.setEncoding("utf8"); child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => { stdout += chunk; });
    child.stderr.on("data", (chunk: string) => { stderr += chunk; });
    child.on("error", reject);
    child.on("exit", (code) => {
      clearTimeout(timeout);
      const line = stdout.split(/\r?\n/).find((candidate) => candidate.startsWith(resultPrefix));
      if (code !== 0 || !line) reject(new Error(`fixture failed (${code})\n${stdout}\n${stderr}`));
      else resolve(JSON.parse(line.slice(resultPrefix.length)) as Result);
    });
  });
}

before(async () => {
  mkdirSync(fixtureDirectory, { recursive: true });
  writeFileSync(fixtureHtmlPath, '<!doctype html><html><body><script type="module" src="/.tmp/sync-folders-mounted/fixture.tsx"></script></body></html>');
  writeFileSync(fixtureSourcePath, fixtureSource);
  server = await createServer({ root: webRoot, logLevel: "error", server: { host: "127.0.0.1", port: 0, strictPort: false } });
  await server.listen();
  const address = server.httpServer?.address();
  assert.ok(address && typeof address === "object");
  result = await runFixture(`http://127.0.0.1:${address.port}/.tmp/sync-folders-mounted/fixture.html`);
  assert.equal(result.ok, true, result.error);
});

after(async () => {
  await server?.close();
  if (dirname(fixtureSourcePath) === fixtureDirectory) rmSync(fixtureDirectory, { recursive: true, force: true });
});

describe("mounted synchronized-folder management", () => {
  it("keeps stable identities, local-only gating, and live status", () => {
    assert.equal(result.duplicateRows, true);
    assert.equal(result.localOnlyGated, true);
    assert.equal(result.liveRegion, true);
  });
  it("keeps dialog mutations single-flight, retryable, and focus-safe", () => {
    assert.equal(result.duplicateBlocked, true);
    assert.equal(result.failureVisible, true);
    assert.equal(result.successClosed, true);
    assert.equal(result.focusRestored, true);
    assert.equal(result.escapeClosed, true);
    assert.equal(result.renameCommitted, true);
    assert.equal(result.rebindCommitted, true);
  });
  it("wraps long text and collapses rows in the compact layout", () => {
    assert.equal(result.longTextWrapped, true);
    assert.equal(result.compact, true);
  });
});
