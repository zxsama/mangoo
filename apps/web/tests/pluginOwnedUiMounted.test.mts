import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";
import { createServer, type ViteDevServer } from "./viteTestServer.mts";
const webRoot = fileURLToPath(new URL("../", import.meta.url));
const runner = fileURLToPath(new URL("./fixtures/runOverlayLifecycle.mjs", import.meta.url));
let server: ViteDevServer;
let result: { ok: boolean; error?: string; absent: unknown; evidence: unknown; sandbox: string; modalShared: boolean; flow: string[]; keyboard: unknown; revoked: boolean; epochPinned: boolean; parentCsp: string; updatedNotice: boolean; changedDocumentRejected: boolean; pluginOwnedSetup: unknown; notifications: Record<string, unknown>; desktopNotifications: Record<string, unknown> };
before(async () => {
  const parent = await readFile(new URL("../index.html", import.meta.url), "utf8");
  const parentCsp = parent.match(/<meta\s+http-equiv="Content-Security-Policy"[\s\S]*?\/>/)?.[0];
  assert.ok(parentCsp, "Use the actual Host index CSP, not a permissive fixture policy");
  server = await createServer({ root: webRoot, logLevel: "error", plugins: [{ name: "actual-host-parent-policy", transformIndexHtml: (html) => html.replace("<head>", `<head>${parentCsp}`) }], server: { host: "127.0.0.1", port: 0, strictPort: false } }); await server.listen();
  const address = server.httpServer?.address(); assert.ok(address && typeof address === "object");
  const electron = await import("electron"); const environment = { ...process.env }; delete environment.ELECTRON_RUN_AS_NODE;
  result = await new Promise((resolve, reject) => {
    const child = spawn(electron.default as unknown as string, [runner, `http://127.0.0.1:${address.port}/tests/fixtures/plugin-owned-ui.html`], { cwd: webRoot, env: environment, stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "", stderr = ""; child.stdout.setEncoding("utf8"); child.stderr.setEncoding("utf8"); child.stdout.on("data", (c) => stdout += c); child.stderr.on("data", (c) => stderr += c);
    child.on("exit", () => { const marker = "OVERLAY_LIFECYCLE_RESULT:"; const index = stdout.lastIndexOf(marker); index < 0 ? reject(new Error(`${stdout}\n${stderr}`)) : resolve(JSON.parse(stdout.slice(index + marker.length).trim())); }); child.on("error", reject);
  }); assert.equal(result.ok, true, result.error);
});
after(async () => server?.close());
describe("mounted Host plugin extension", () => {
  it("has no feature actions, frames or sync queries without a contribution", () => assert.deepEqual(result.absent, { noFrame: true, noActions: true, noSyncRequests: true }));
  it("runs a contributed document with an opaque origin, blocked networking and no Host credentials", () => { assert.deepEqual(result.evidence, { isolated: true, networkBlocked: true, denied: true, noPreload: true, noToken: true, theme: "dark" }); assert.equal(result.sandbox, "allow-scripts"); });
  it("uses the shared modal, routes preflight before commit and revokes a disabled plugin", () => { assert.equal(result.modalShared, true); assert.deepEqual(result.flow, ["preflight", "commit"]); assert.equal(result.revoked, true); });
  it("allows native keyboard entry, nested iframe dialog Escape, and return to Host controls", () => assert.deepEqual(result.keyboard, { enteredFrame: true, innerEscapeKeptHost: true, returnedToHost: true }));
  it("runs under the actual parent CSP and pins the verified document lifecycle epoch on Core calls", () => { assert.match(result.parentCsp, /default-src 'self'/); assert.equal(result.epochPinned, true); });
  it("ends a successfully changed session with Host feedback and rejects a mismatched document epoch", () => { assert.equal(result.updatedNotice, true); assert.equal(result.changedDocumentRejected, true); });
  it("keeps enablement and grants in Host without rendering duplicate setup for a UI plugin", () => assert.deepEqual(result.pluginOwnedSetup, { noConfigurationReads: true, explicitEnable: true, explicitGrants: true, noHostSetup: true }));
  it("keeps local notification service errors visible without inventing a sync entry or querying sync", () => {
    for (const field of ['noSection', 'noFrame', 'noSyncEntry', 'localErrorVisible', 'compactServiceLabel', 'noMisleadingHealthLabel', 'noMatchingSlot', 'missingGrantHidden', 'noHostSyncRequests', 'noEagerDocument']) {
      assert.equal(result.notifications[field], true, field);
    }
  });
  it("launches notification status from the plugin document in the active library context", () => {
    assert.equal(result.notifications.notificationIsolation, true);
    assert.deepEqual(result.notifications.contributedStatus, { notificationStatus: 'Plugin queue: running', libraryUuid: 'library-a', viewId: 'notification-status', isolated: true });
  });
  it("revokes notification views on disable, library change, missing grants and uninstall while retaining local errors", () => {
    for (const field of ['disabledRevoked', 'localErrorAfterDisable', 'libraryChangedRevoked', 'grantRevoked', 'uninstalledRevoked']) assert.equal(result.notifications[field], true, field);
  });
  it("uses narrow desktop IPC without credentials, HTTP or iframe in the notification renderer", () => {
    for (const field of ['empty', 'noHttp', 'noFrame', 'noCredentials', 'errorVisible']) assert.equal(result.desktopNotifications[field], true, field);
    assert.deepEqual(result.desktopNotifications.openedRequest, { pluginId: 'org.example.extension', viewId: 'notification-status', libraryUuid: 'library-a' });
  });
  it("delivers the desktop intent to the plugin window launcher and rejects stale context", () => {
    assert.deepEqual(result.desktopNotifications.intentOpened, { notificationStatus: 'Plugin queue: running', libraryUuid: 'library-a', viewId: 'notification-status', isolated: true });
    assert.equal(result.desktopNotifications.staleIntentRejected, true);
    assert.equal(result.desktopNotifications.hotIntentReplacesView, true);
  });
  it("keeps global library settings contributions independent of the active library", async () => {
    const settings = await readFile(new URL("../src/features/settings/settingsPanels.tsx", import.meta.url), "utf8");
    assert.match(settings, /<PluginContributionsSlot slot="library-settings"\s*\/>/);
  });
});
