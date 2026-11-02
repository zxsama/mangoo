import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { describe, it } from "node:test";
import { CoreApiError, type HostPluginProjection } from "@mangoo/core-client";
import { createPluginUiBridge, pluginUiAvailable, pluginUiDocument, type PluginUiHost, type PluginUiBridgeOptions } from "../src/features/plugins/pluginUiBridge.ts";
import { pluginUiCoreMethods, type PluginUiCoreClient } from "../src/features/plugins/pluginUiCoreMethods.ts";
import type { PluginUiContext } from "../src/features/plugins/pluginUiBridge.ts";

const plugin: HostPluginProjection = {
  pluginId: "org.example.extension", lifecycleEpoch: 1, name: "Example", selectedVersion: "1.0.0", knownGoodVersion: "1.0.0", enabledIntent: true, runtimeState: "ready", authState: "authenticated", accountUuid: "account-a", accountDisplayName: null, accountKind: null, capabilities: [], permissionRequired: false, requestedPermissions: [], grantedPermissions: [{ kind: "host-ui", operations: ["core-api", "pick-folder", "reveal-path", "clipboard-write", "external-link"] }], compatibilityReason: null, backoffUntil: null, lastHealthyAt: null, lastErrorCode: null, lastErrorDetail: null,
  contributions: { configuration: [], authentication: null, presentation: null, primaryCredential: null, ui: { apiVersion: 1, entry: "ui.html", sha256: "a".repeat(64), views: [{ id: "main", title: "Example", slot: "library-settings" }] } },
};
function fixture(overrides: Partial<PluginUiCoreClient> = {}, context: Partial<PluginUiContext> = {}, onChanged?: PluginUiBridgeOptions["changed"]) {
  const calls: string[] = [], output: unknown[] = [], revealed: string[] = [];
  let current = structuredClone(plugin), revision = 1, aborted = false;
  const source = {};
  const client = {
    getPlugin: async () => current,
    getActiveLibrary: async () => ({ library: null, contextRevision: revision }),
    getSyncStatus: async () => ({ libraries: [{ libraryUuid: "library-a", providerId: current.pluginId, accountUuid: current.accountUuid }], providers: [{ pluginId: current.pluginId, accountUuid: current.accountUuid }, { pluginId: "other", accountUuid: "other" }] }),
    listLibraries: async () => ({ items: [{ libraryUuid: "library-a", bindingState: "synchronized", active: true, databasePath: "C:/Library" }] }),
    listSyncFolders: async () => ({ items: [] }),
    preflightLibrarySync: async () => { calls.push("preflight"); return { ok: true }; },
    pauseLibrarySync: async () => { calls.push("pause"); return { accepted: true }; },
    getSyncRestore: async () => ({ accountUuid: "other" }),
    beginPluginAuth: async () => { calls.push("auth"); return {}; },
    ...overrides,
  } as unknown as PluginUiCoreClient;
  const host = { pickNativeFolder: async () => { calls.push("pick"); return "C:/Picked"; }, pickNewLibraryPath: async () => "C:/New", revealPath: async (path) => { calls.push("reveal"); revealed.push(path); return true; }, copyText: async () => true, openPluginExternalLink: async () => ({ ok: true }), requestOAuthReturnFocus: async () => true } as PluginUiHost;
  const bridge = createPluginUiBridge({ client, plugin, viewId: "main", context: { libraryUuid: "library-a", rootId: null, rootUuid: null, ...context }, contextRevision: 1, theme: { colorScheme: "dark" }, sessionId: "session-1", source, post: (message) => output.push(message), abort: () => { aborted = true; }, host, changed: onChanged ?? (() => { calls.push("changed"); }), close: () => {} });
  let sequence = 0;
  const ready = () => bridge.receive({ source, data: { type: "mangoo-plugin-ui:ready" } });
  const request = (method: string, args: unknown[] = [], extras = {}) => bridge.receive({ source, data: { type: "mangoo-plugin-ui:request", sessionId: "session-1", requestId: String(++sequence), method, args, ...extras } });
  return { bridge, calls, output, revealed, source, ready, request, setPlugin: (p: HostPluginProjection) => { current = p; }, setRevision: (n: number) => { revision = n; }, get aborted() { return aborted; } };
}
const response = (f: ReturnType<typeof fixture>) => f.output.at(-1) as { result?: unknown; error?: { message: string; status?: number; body?: unknown } };

describe("isolated plugin UI bridge", () => {
  it("validates all creation paths and explicit restore placement without legacy fields", () => {
    const methods = pluginUiCoreMethods({} as PluginUiCoreClient, 1);
    assert.equal(methods.preflightLibrary.validate([{ name: "Photos", databasePath: "D:/Photos/Database", contentRoots: [{ path: "D:/Photos/Files", createIfMissing: true }, { path: "E:/Existing", createIfMissing: false }] }]), true);
    assert.equal(methods.preflightLibrary.validate([{ name: "Photos", managedRoot: "D:/Photos" }]), false);
    assert.equal(methods.preflightLibrary.validate([{ name: "Photos", databasePath: "D:/Photos/Database", contentRoots: [{ path: "E:/Existing" }] }]), false);
    assert.equal(methods.preflightSyncRestore.validate([{ items: [{ accountUuid: "a", libraryUuid: "l", name: "Photos", placementBase: "D:/Photos" }] }]), true);
    assert.equal(methods.preflightSyncRestore.validate([{ items: [{ accountUuid: "a", libraryUuid: "l", name: "Photos", managedRoot: "D:/Photos" }] }]), false);
  });
  it("puts Host CSP before every document and has no same-origin or preload route", () => {
    const document = pluginUiDocument('<script>window.example=1</script>');
    assert.ok(document.indexOf("Content-Security-Policy") < document.indexOf("<script>"));
    assert.match(document, /connect-src 'none'/);
    const view = readFileSync(new URL("../src/features/plugins/PluginView.tsx", import.meta.url), "utf8");
    assert.match(view, /sandbox="allow-scripts"/);
    assert.doesNotMatch(view, /allow-same-origin|allow-popups|allow-top-navigation/);
  });
  it("only initializes the current window and does not forward Host credentials", async () => {
    const f = fixture();
    await f.bridge.receive({ source: {}, data: { type: "mangoo-plugin-ui:ready" } });
    assert.equal(f.output.length, 0);
    await f.ready();
    assert.deepEqual(f.output[0], { type: "mangoo-plugin-ui:init", sessionId: "session-1", pluginId: plugin.pluginId, viewId: "main", context: { libraryUuid: "library-a", rootId: null, rootUuid: null }, theme: { colorScheme: "dark" } });
    await f.request("core.beginPluginAuth", [plugin.pluginId], { sessionId: "old" });
    assert.deepEqual(f.calls, []);
  });
  it("denies arbitrary dispatch, malformed arguments, cross-plugin IDs, accounts and libraries", async () => {
    const f = fixture(); await f.ready();
    for (const [method, args] of [ ["core.fetch", ["http://example.invalid"]], ["core.constructor", []], ["core.beginPluginAuth", ["other"]], ["core.pauseLibrarySync", ["library-a", "1", { operationUuid: "o" }]], ["core.pauseLibrarySync", ["other", 1, { operationUuid: "o" }]], ["core.preflightLibrarySync", ["library-a", { providerId: plugin.pluginId, accountUuid: "other", remoteLibraryUuid: null }]], ["core.cancelSyncRestore", ["other-restore"]] ] as const) {
      await f.request(method, [...args]); assert.ok(response(f).error, method);
    }
    assert.deepEqual(f.calls, []);
  });
  it("rejects other provider bindings and filters status to the current provider", async () => {
    const f = fixture({ getSyncStatus: async () => ({ providers: [{ pluginId: "other", accountUuid: "other" }], libraries: [{ libraryUuid: "library-a", providerId: "other", accountUuid: "other" }] }) as never }); await f.ready();
    await f.request("core.pauseLibrarySync", ["library-a", 1, { operationUuid: "o" }]); assert.ok(response(f).error);
    await f.request("core.getSyncStatus"); assert.deepEqual(response(f).result, { providers: [], libraries: [] });
  });
  it("preserves typed Core error status and body", async () => {
    const body = { code: "stale-operation-revision", message: "Refresh the plan" };
    const f = fixture({ pauseLibrarySync: async () => { throw new CoreApiError(409, body); } }); await f.ready();
    await f.request("core.pauseLibrarySync", ["library-a", 1, { operationUuid: "o" }]);
    assert.equal(response(f).error?.status, 409); assert.deepEqual(response(f).error?.body, body);
  });
  it("requires the operation grant and restricts path reveal to picked or active library paths", async () => {
    const f = fixture(); await f.ready();
    await f.request("host.revealPath", ["C:/Elsewhere"]); assert.ok(response(f).error);
    await f.request("host.pickFolder"); await f.request("host.revealPath", ["C:/Picked"]); assert.equal(response(f).result, true);
    f.setPlugin({ ...plugin, grantedPermissions: [{ kind: "host-ui", operations: ["core-api"] }] });
    await f.request("host.copyText", ["hello"]); assert.ok(response(f).error);
  });
  it("revokes on disable, replacement and library context changes", async () => {
    for (const replacement of [{ ...plugin, enabledIntent: false }, { ...plugin, selectedVersion: "2.0.0" }]) {
      const f = fixture(); await f.ready(); f.setPlugin(replacement); await f.request("core.beginPluginAuth", [plugin.pluginId]); assert.equal(f.aborted, true); assert.deepEqual(f.calls, []);
    }
    const f = fixture(); await f.ready(); f.setRevision(2); await f.request("core.getActiveLibrary"); assert.equal(f.aborted, true);
  });
  it("bounds concurrency and ignores late results after revocation", async () => {
    let release!: (p: HostPluginProjection) => void;
    const gate = new Promise<HostPluginProjection>((resolve) => { release = resolve; });
    const f = fixture({ getPlugin: () => gate }); await f.ready();
    const pending = Array.from({ length: 32 }, () => f.request("core.getActiveLibrary"));
    await f.request("core.getActiveLibrary"); assert.equal(response(f).error?.status, 429);
    const count = f.output.length; f.bridge.revoke(); release(plugin); await Promise.all(pending); assert.equal(f.output.length, count); assert.equal(f.aborted, true);
  });
  it("drops oversized messages and cannot replay an accepted request ID", async () => {
    const f = fixture(); await f.ready();
    const count = f.output.length;
    await f.request("host.copyText", ["x".repeat(1024 * 1024 + 1)]);
    assert.equal(f.output.length, count);
    await f.request("core.pauseLibrarySync", ["library-a", 1, { operationUuid: "o" }], { requestId: "100" });
    await f.request("core.pauseLibrarySync", ["library-a", 1, { operationUuid: "o" }], { requestId: "100" });
    assert.deepEqual(f.calls, ["pause", "changed"]);
  });
  it("does not mount or load built-in feature code when no UI contribution exists", () => {
    assert.equal(pluginUiAvailable({ ...plugin, contributions: null }), false);
    assert.equal(existsSync(new URL("../src/features/sync/SyncManagementPanel.tsx", import.meta.url)), false);
    for (const path of ["library/ManagedLibraryWizard.tsx", "library/LibrarySwitcher.tsx", "settings/settingsPanels.tsx"]) {
      const source = readFileSync(new URL(`../src/features/${path}`, import.meta.url), "utf8");
      assert.doesNotMatch(source, /useSyncOrchestration|SyncManagementPanel|SyncFoldersPanel|usePluginAuth|从远端恢复|启用同步/);
    }
    const slot = readFileSync(new URL("../src/features/plugins/PluginContributionsSlot.tsx", import.meta.url), "utf8");
    assert.doesNotMatch(slot, /getSyncStatus|listSyncFolders|openForRoot/);
    assert.equal(existsSync(new URL("../src/features/plugins/LegacyPluginViewIntent.tsx", import.meta.url)), false);
  });
  it("keeps replay memory constant and remains usable after ten thousand requests", async () => {
    const f = fixture(); await f.ready();
    for (let i = 0; i < 10010; i++) await f.request("host.copyText", ["ok"]);
    assert.equal(response(f).result, true);
    const count = f.output.length;
    for (const requestId of ["1", "10009", "01", "0", "1e4", "9007199254740992", "r-10011"]) await f.request("host.copyText", ["bad"], { requestId });
    assert.equal(f.output.length, count);
    await f.request("host.copyText", ["next"]); assert.equal(f.output.length, count + 1);
  });
  it("revalidates the epoch after delayed ownership reads before Core or desktop dispatch", async () => {
    for (const method of ["core.pauseLibrarySync", "host.pickFolder"]) {
      let release!: () => void;
      let entered!: () => void;
      const waiting = new Promise<void>((resolve) => { entered = resolve; });
      const gate = new Promise<void>((resolve) => { release = resolve; });
      const f = fixture({ getSyncStatus: async () => { entered(); await gate; return { libraries: [{ libraryUuid: "library-a", providerId: plugin.pluginId, accountUuid: plugin.accountUuid }], providers: [] } as never; } }); await f.ready();
      const request = f.request(method, method.startsWith("core.") ? ["library-a", 1, { operationUuid: "o" }] : [{ libraryUuid: "library-a", rootUuid: null, mode: "enroll-existing", suggestedPath: null, dialogTitle: "Pick" }]);
      await waiting;
      f.setPlugin({ ...plugin, enabledIntent: false, lifecycleEpoch: 2 });
      f.setPlugin({ ...plugin, enabledIntent: true, lifecycleEpoch: 3 });
      release(); await request;
      assert.equal(f.aborted, true); assert.deepEqual(f.calls, []);
    }
  });
  it("rejects future revisions before ownership I/O and preserves the fixed revision", async () => {
    let reads = 0;
    const f = fixture({ getSyncStatus: async () => { reads++; return { providers: [], libraries: [{ libraryUuid: "library-a", providerId: plugin.pluginId, accountUuid: plugin.accountUuid }] } as never; } }); await f.ready();
    await f.request("core.pauseLibrarySync", ["library-a", 2, { operationUuid: "o" }]);
    assert.ok(response(f).error); assert.equal(reads, 0); assert.deepEqual(f.calls, []);
    await f.request("core.pauseLibrarySync", ["library-a", 1, { operationUuid: "o" }]); assert.deepEqual(f.calls, ["pause", "changed"]);
    const typed = readFileSync(new URL("../../../packages/core-client/src/client.ts", import.meta.url), "utf8");
    const adapters = readFileSync(new URL("../src/features/plugins/pluginUiCoreMethods.ts", import.meta.url), "utf8");
    for (const line of adapters.split("\n")) {
      const name = line.match(/^\s+(\w+): expose\(/)?.[1]; if (!name) continue;
      const signature = typed.match(new RegExp(`    ${name}: \\(([^\\n]*?)\\) =>`))?.[1];
      if (!signature?.includes("contextRevision: number")) continue;
      const revisionIndex = signature.slice(0, signature.indexOf("contextRevision: number")).split(",").length - 1;
      assert.match(line, new RegExp(`\\], ${revisionIndex}\\),$`), `${name} must pin its typed contextRevision position`);
    }
    const methods = pluginUiCoreMethods({ pauseLibrarySync: async (_library, revision) => revision } as unknown as PluginUiCoreClient, 7);
    assert.equal(methods.pauseLibrarySync.validate(["library-a", 8, { operationUuid: "o" }]), false);
    assert.equal(await methods.pauseLibrarySync.invoke(["library-a", 7, { operationUuid: "o" }]), 7);
  });
  it("reveals only exact owned local roots, constrained by the selected root", async () => {
    const overrides = {
      getActiveLibrary: async () => ({ library: { libraryUuid: "library-a" }, contextRevision: 1 }),
      listRoots: async () => ({ items: [{ id: 4, path: "C:/RootA" }, { id: 5, path: "C:/RootB" }] }),
      listSyncFolders: async () => ({ items: [{ descriptor: { rootUuid: "root-a" }, binding: { localPath: "C:/RootA" } }, { descriptor: { rootUuid: "root-b" }, binding: { localPath: "C:/RootB" } }] }),
    } as unknown as Partial<PluginUiCoreClient>;
    const f = fixture(overrides, { rootId: 4 }); await f.ready();
    await f.request("host.revealPath", ["C:/RootA"]); assert.equal(response(f).result, true);
    for (const path of ["C:/RootB", "C:/RootA/private", "C:/Library", "C:/Elsewhere"]) { await f.request("host.revealPath", [path]); assert.ok(response(f).error, path); }
    assert.deepEqual(f.calls, ["reveal"]);
    const other = fixture({ ...overrides, getSyncStatus: async () => ({ libraries: [{ libraryUuid: "library-a", providerId: plugin.pluginId, accountUuid: "other-account" }], providers: [] }) as never }); await other.ready();
    await other.request("host.revealPath", ["C:/RootA"]); assert.ok(response(other).error); assert.deepEqual(other.calls, []);
  });
  it("acknowledges successful mutations before refresh revocation without a duplicate failure response", async () => {
    let acknowledged = false;
    const f = fixture({}, {}, async () => {
      acknowledged = (response(f).result as { accepted: boolean }).accepted;
      f.bridge.revoke();
      throw new Error("Refresh unavailable after success");
    }); await f.ready();
    await f.request("core.pauseLibrarySync", ["library-a", 1, { operationUuid: "o" }]);
    assert.equal(acknowledged, true); assert.equal(f.output.length, 2); assert.equal(f.aborted, true);
  });
  it("preserves admission errors and immediately revokes a stale transport session", async () => {
    for (const code of ["plugin-ui-session-changed", "plugin-ui-target-changed"]) {
      const body = { code, error: "Plugin UI session or target is no longer available" };
      const f = fixture({ getActiveLibrary: async () => { throw new CoreApiError(409, body); } }); await f.ready();
      await f.request("core.getActiveLibrary");
      assert.equal(response(f).error?.status, 409); assert.deepEqual(response(f).error?.body, body); assert.equal(f.aborted, true);
      assert.deepEqual(f.calls, []);
    }
  });
  it("matches Windows spelling differences but reveals only the Host canonical selected-root path", async () => {
    const canonical = String.raw`d:\fixture\selected`;
    const overrides = {
      getActiveLibrary: async () => ({ library: { libraryUuid: "library-a" }, contextRevision: 1 }),
      listRoots: async () => ({ items: [{ id: 4, path: "D:\\Fixture\\Selected\\" }] }),
      listSyncFolders: async () => ({ items: [{ descriptor: { rootUuid: "root-a" }, binding: { localPath: canonical } }] }),
    } as unknown as Partial<PluginUiCoreClient>;
    const f = fixture(overrides, { rootId: 4, rootUuid: "root-a" }); await f.ready();
    for (const path of ["D:/Fixture/Selected/", "D:\\FIXTURE\\SELECTED\\", canonical]) { await f.request("host.revealPath", [path]); assert.equal(response(f).result, true); }
    assert.deepEqual(f.revealed, [canonical, canonical, canonical]);
    for (const path of ["D:/Fixture/Selected/../Selected", "D:/Fixture/Selected/./", String.raw`\\?\d:\fixture\selected`, "D:fixture/selected", "D:/Fixture/Other", "D:/Fixture/Selected/subfolder"]) { await f.request("host.revealPath", [path]); assert.ok(response(f).error, path); }
    assert.equal(f.revealed.length, 3);
    const otherRoot = fixture(overrides, { rootId: 5 }); await otherRoot.ready(); await otherRoot.request("host.revealPath", ["D:/Fixture/Selected"]); assert.ok(response(otherRoot).error);
    const otherUuid = fixture(overrides, { rootId: 4, rootUuid: "root-other" }); await otherUuid.ready(); await otherUuid.request("host.revealPath", ["D:/Fixture/Selected"]); assert.ok(response(otherUuid).error);
  });
  it("normalizes picked and UNC Windows paths without changing POSIX case or resolving segments", async () => {
    const picked = fixture(); await picked.ready(); await picked.request("host.pickFolder"); await picked.request("host.revealPath", ["c:\\PICKED\\"]); assert.deepEqual(picked.revealed, ["C:/Picked"]);
    const canonical = String.raw`\\Storage\Media\Selected`;
    const f = fixture({ listSyncFolders: async () => ({ items: [{ descriptor: { rootUuid: "root-a" }, binding: { localPath: canonical } }] }) as never }); await f.ready();
    await f.request("host.revealPath", ["//storage/media/selected/"]); assert.deepEqual(f.revealed, [canonical]);
    await f.request("host.revealPath", [String.raw`\\?\UNC\Storage\Media\Selected`]); assert.ok(response(f).error);
    const posix = fixture({ listSyncFolders: async () => ({ items: [{ descriptor: { rootUuid: "root-a" }, binding: { localPath: "/media/Selected" } }] }) as never }); await posix.ready();
    await posix.request("host.revealPath", ["/media/Selected"]); assert.deepEqual(posix.revealed, ["/media/Selected"]);
    for (const path of ["/media/selected", "/media/Selected/../Selected", "\\media\\Selected"]) { await posix.request("host.revealPath", [path]); assert.ok(response(posix).error); }
  });
  it("rejects ambiguous equivalent Windows Host candidates instead of selecting one", async () => {
    const f = fixture({ listSyncFolders: async () => ({ items: ["D:/Fixture/Selected", String.raw`d:\fixture\selected`].map((localPath, index) => ({ descriptor: { rootUuid: `root-${index}` }, binding: { localPath } })) }) as never }); await f.ready();
    await f.request("host.revealPath", ["D:/FIXTURE/SELECTED/"]);
    assert.ok(response(f).error); assert.deepEqual(f.revealed, []);
  });
});
