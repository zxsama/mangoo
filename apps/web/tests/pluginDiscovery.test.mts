import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { after, before, describe, it } from "node:test";
import { createServer, type ViteDevServer } from "vite";

const webRoot = fileURLToPath(new URL("../", import.meta.url));
let server: ViteDevServer;
let pluginDataModule: typeof import("../src/features/plugins/usePluginsData.ts");

before(async () => {
  server = await createServer({
    root: webRoot,
    appType: "custom",
    logLevel: "error",
    server: { middlewareMode: true }
  });
  pluginDataModule = await server.ssrLoadModule("/src/features/plugins/usePluginsData.ts");
});

after(async () => server?.close());

describe("plugin automatic discovery", () => {
  it("coalesces concurrent triggers and enforces the cooldown", async () => {
    let now = 1_000;
    let discoveryCalls = 0;
    let resolveDiscovery: ((value: { discovered: number; errors: [] }) => void) | null = null;
    const successes: number[] = [];
    const coordinator = pluginDataModule.createPluginDiscoveryCoordinator({
      cooldownMs: 5_000,
      discover: () => {
        discoveryCalls += 1;
        return new Promise((resolve) => {
          resolveDiscovery = resolve;
        });
      },
      isActive: () => true,
      now: () => now,
      onError: () => assert.fail("discovery should not fail"),
      onSuccess: (response) => successes.push(response.discovered)
    });

    const first = coordinator.run();
    const duplicate = coordinator.run();
    assert.equal(first, duplicate);
    await Promise.resolve();
    assert.equal(discoveryCalls, 1);

    assert.ok(resolveDiscovery);
    resolveDiscovery({ discovered: 2, errors: [] });
    await first;
    assert.deepEqual(successes, [2]);

    now = 5_999;
    await coordinator.run();
    assert.equal(discoveryCalls, 1);

    now = 6_000;
    const afterCooldown = coordinator.run();
    await Promise.resolve();
    assert.equal(discoveryCalls, 2);
    assert.ok(resolveDiscovery);
    resolveDiscovery({ discovered: 3, errors: [] });
    await afterCooldown;
    assert.deepEqual(successes, [2, 3]);
  });

  it("does not publish a completion after its consumer becomes inactive", async () => {
    let active = true;
    let resolveDiscovery: ((value: { discovered: number; errors: [] }) => void) | null = null;
    let successCalls = 0;
    const coordinator = pluginDataModule.createPluginDiscoveryCoordinator({
      cooldownMs: 0,
      discover: () =>
        new Promise((resolve) => {
          resolveDiscovery = resolve;
        }),
      isActive: () => active,
      onError: () => assert.fail("discovery should not fail"),
      onSuccess: () => {
        successCalls += 1;
      }
    });

    const pending = coordinator.run();
    await Promise.resolve();
    active = false;
    assert.ok(resolveDiscovery);
    resolveDiscovery({ discovered: 1, errors: [] });
    await pending;
    assert.equal(successCalls, 0);
  });

  it("reports failures without rejecting the event handler", async () => {
    const errors: unknown[] = [];
    const failure = new Error("discovery unavailable");
    const coordinator = pluginDataModule.createPluginDiscoveryCoordinator({
      cooldownMs: 0,
      discover: async () => {
        throw failure;
      },
      isActive: () => true,
      onError: (cause) => errors.push(cause),
      onSuccess: () => assert.fail("discovery should not succeed")
    });

    await coordinator.run();
    assert.deepEqual(errors, [failure]);
  });

  it("keeps the current selection when possible and otherwise selects the first plugin", () => {
    const plugins = [
      { pluginId: "com.mangoo.fake-sync" },
      { pluginId: "com.mangoo.configurable-sync" }
    ] as Parameters<typeof pluginDataModule.reconcilePluginSelection>[1];

    assert.equal(
      pluginDataModule.reconcilePluginSelection("com.mangoo.configurable-sync", plugins),
      "com.mangoo.configurable-sync"
    );
    assert.equal(
      pluginDataModule.reconcilePluginSelection("com.mangoo.removed", plugins),
      "com.mangoo.fake-sync"
    );
    assert.equal(pluginDataModule.reconcilePluginSelection(null, plugins), "com.mangoo.fake-sync");
    assert.equal(pluginDataModule.reconcilePluginSelection("com.mangoo.configurable-sync", []), null);
  });

  it("keeps discovery separate from the list poll and removes manual scan UI", () => {
    const hookSource = readFileSync(
      new URL("../src/features/plugins/usePluginsData.ts", import.meta.url),
      "utf8"
    );
    const viewSource = readFileSync(
      new URL("../src/features/plugins/PluginsView.tsx", import.meta.url),
      "utf8"
    );

    assert.match(hookSource, /queryKey:\s*PLUGINS_QUERY_KEY/);
    assert.match(hookSource, /refetchInterval:\s*PLUGIN_LIST_REFRESH_MS/);
    assert.match(hookSource, /window\.addEventListener\("focus", discover\)/);
    assert.match(hookSource, /window\.addEventListener\("online", discover\)/);
    assert.doesNotMatch(hookSource, /queryFn:\s*\(\)\s*=>\s*client\.discoverPlugins/);
    assert.doesNotMatch(viewSource, /Re-scan|registered\s*·|plugins-rescan/);
    assert.match(viewSource, /detected automatically/);
    assert.match(viewSource, /role="tablist"/);
    assert.match(viewSource, /aria-orientation="vertical"/);
    assert.match(hookSource, /cause instanceof CoreApiError && cause\.status === 409/);
    assert.match(viewSource, /title="无法移除插件"/);
    assert.match(viewSource, /plugins\.removalError/);
    assert.doesNotMatch(viewSource, /账号或资源库仍引用此 Provider 时无法移除/);
  });
});
