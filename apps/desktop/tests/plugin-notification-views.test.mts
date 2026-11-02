import assert from "node:assert/strict";
import test from "node:test";
import type { HostPluginProjection } from "@mangoo/core-client/node";
import { notificationPluginEntries, parseNotificationPluginOpenRequest, resolveNotificationPluginIntent } from "../src/plugin-notification-views.ts";

const plugin: HostPluginProjection = {
  pluginId: "org.example.extension", lifecycleEpoch: 7, name: "Example", selectedVersion: "1.0.0", knownGoodVersion: "1.0.0", enabledIntent: true, runtimeState: "ready", authState: "authenticated", accountUuid: "private-account", accountDisplayName: null, accountKind: null, capabilities: [], permissionRequired: false, requestedPermissions: [], grantedPermissions: [], compatibilityReason: null, backoffUntil: null, lastHealthyAt: null, lastErrorCode: null, lastErrorDetail: null,
  contributions: { configuration: [], authentication: null, presentation: null, primaryCredential: null, ui: { apiVersion: 1, entry: "ui.html", sha256: "a".repeat(64), views: [{ id: "status", title: "Status", slot: "notification-center" }, { id: "settings", title: "Settings", slot: "library-settings" }] } },
};
const request = { pluginId: plugin.pluginId, viewId: "status", libraryUuid: null };
const client = (current = plugin) => ({ getPlugin: async () => current, getActiveLibrary: async () => ({ library: null, contextRevision: 8, runtimeGeneration: 1 }) });

test("notification entry projection contains only eligible minimal metadata", () => {
  assert.deepEqual(notificationPluginEntries([plugin]), [{ pluginId: plugin.pluginId, viewId: "status", title: "Status" }]);
  for (const changed of [{ enabledIntent: false }, { permissionRequired: true }, { runtimeState: "disabled" as const }, { contributions: null }]) assert.deepEqual(notificationPluginEntries([{ ...plugin, ...changed }]), []);
});

test("notification open validates exact input, active context, slot and current lifecycle", async () => {
  assert.deepEqual(await resolveNotificationPluginIntent(request, client()), { ...request, lifecycleEpoch: 7, contextRevision: 8 });
  for (const input of [null, { ...request, document: "extra" }, { ...request, viewId: "../status" }, { pluginId: plugin.pluginId, viewId: "status" }]) assert.equal(parseNotificationPluginOpenRequest(input), null);
  for (const input of [{ ...request, viewId: "settings" }, { ...request, viewId: "missing" }, { ...request, pluginId: "forged" }, { ...request, libraryUuid: "other-library" }]) await assert.rejects(resolveNotificationPluginIntent(input, client()));
  await assert.rejects(resolveNotificationPluginIntent(request, client({ ...plugin, enabledIntent: false })));
  await assert.rejects(resolveNotificationPluginIntent(request, client({ ...plugin, permissionRequired: true })));
  assert.equal((await resolveNotificationPluginIntent(request, client({ ...plugin, lifecycleEpoch: 9 }))).lifecycleEpoch, 9);
});

test("notification requests preserve manifest plugin and view identifier bounds", () => {
  const longest = { ...request, pluginId: `org.${"a".repeat(251)}`, viewId: `V${"a".repeat(63)}` };
  assert.deepEqual(parseNotificationPluginOpenRequest(longest), longest);
  for (const pluginId of [`${longest.pluginId}a`, "single", "org..example", "org.example_name", "Org.example"]) {
    assert.equal(parseNotificationPluginOpenRequest({ ...request, pluginId }), null, pluginId);
  }
  for (const viewId of [`${longest.viewId}a`, "1status", "status.name", "status_name"]) {
    assert.equal(parseNotificationPluginOpenRequest({ ...request, viewId }), null, viewId);
  }
});
