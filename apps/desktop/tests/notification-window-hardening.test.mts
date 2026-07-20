import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import test from "node:test";
import vm from "node:vm";

const ipcModuleUrl = new URL("../src/notification-ipc.ts", import.meta.url);
const openingModuleUrl = new URL("../src/secondary-window-opening.ts", import.meta.url);
const permissionsModuleUrl = new URL("../src/desktop-ipc-permissions.ts", import.meta.url);
const shutdownModuleUrl = new URL("../src/notification-shutdown.ts", import.meta.url);
const channelsUrl = new URL("../src/notification-ipc-channels.cjs", import.meta.url);
const preloadUrl = new URL("../src/preload.cjs", import.meta.url);

const ipcModule = existsSync(fileURLToPath(ipcModuleUrl)) ? await import(ipcModuleUrl.href) : null;
const openingModule = existsSync(fileURLToPath(openingModuleUrl))
  ? await import(openingModuleUrl.href)
  : null;
const permissionsModule = existsSync(fileURLToPath(permissionsModuleUrl))
  ? await import(permissionsModuleUrl.href)
  : null;
const shutdownModule = existsSync(fileURLToPath(shutdownModuleUrl))
  ? await import(shutdownModuleUrl.href)
  : null;
const channels = createRequire(import.meta.url)(fileURLToPath(channelsUrl));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((onResolve, onReject) => {
    resolve = onResolve;
    reject = onReject;
  });
  return { promise, resolve, reject };
}

class LogTarget extends EventEmitter {
  readonly id = 71;
  destroyed = false;
  sent: Array<[string, unknown]> = [];

  isDestroyed() {
    return this.destroyed;
  }

  send(channel: string, payload: unknown) {
    this.sent.push([channel, payload]);
  }
}

test("log subscription tokens prevent stale cleanup from cancelling a replacement", () => {
  assert.ok(ipcModule);
  const subscribers: Array<(batch: unknown) => void> = [];
  let underlyingUnsubscribes = 0;
  const ids = ["subscription-old", "subscription-new"];
  const registry = new ipcModule.ProcessLogIpcSubscriptions(
    {
      snapshotAndSubscribe(subscriber: (batch: unknown) => void) {
        subscribers.push(subscriber);
        return {
          snapshot: [{ sequence: subscribers.length }],
          afterSequence: subscribers.length,
          unsubscribe: () => {
            underlyingUnsubscribes += 1;
          }
        };
      }
    },
    channels.logsChanged,
    () => ids.shift()
  );
  const target = new LogTarget();

  const first = registry.snapshotAndSubscribe(target, {});
  const second = registry.snapshotAndSubscribe(target, {});
  assert.equal(first.subscriptionId, "subscription-old");
  assert.equal(second.subscriptionId, "subscription-new");
  assert.equal(underlyingUnsubscribes, 1, "replacement must retire the previous buffer subscriber");

  assert.equal(registry.unsubscribe(target.id, first.subscriptionId), false);
  assert.equal(registry.size, 1, "late cleanup for the old token must not remove the new subscription");
  subscribers[1]?.({ lines: [], afterSequence: 9, droppedBeforeSequence: null });
  assert.deepEqual(target.sent, [
    [
      channels.logsChanged,
      {
        subscriptionId: "subscription-new",
        batch: { lines: [], afterSequence: 9, droppedBeforeSequence: null }
      }
    ]
  ]);

  assert.equal(registry.unsubscribe(target.id, second.subscriptionId), true);
  assert.equal(registry.size, 0);
  assert.equal(underlyingUnsubscribes, 2);
});

function evaluatePreload(role: "main" | "settings" | "plugins" | "notifications" | "unknown") {
  const listeners = new Map<string, Set<(...args: unknown[]) => void>>();
  const invokes: Array<{
    channel: string;
    args: unknown[];
    deferred: ReturnType<typeof deferred<unknown>>;
  }> = [];
  const sent: Array<[string, ...unknown[]]> = [];
  let exposed: Record<string, unknown> | null = null;
  const ipcRenderer = {
    invoke(channel: string, ...args: unknown[]) {
      if (channel !== channels.logsSubscribe) return Promise.resolve(true);
      const pending = deferred<unknown>();
      invokes.push({ channel, args, deferred: pending });
      return pending.promise;
    },
    on(channel: string, listener: (...args: unknown[]) => void) {
      const bucket = listeners.get(channel) ?? new Set();
      bucket.add(listener);
      listeners.set(channel, bucket);
    },
    removeListener(channel: string, listener: (...args: unknown[]) => void) {
      listeners.get(channel)?.delete(listener);
    },
    send(channel: string, ...args: unknown[]) {
      sent.push([channel, ...args]);
    }
  };
  const source = `${readFileSync(fileURLToPath(channelsUrl), "utf8")}\n${readFileSync(fileURLToPath(preloadUrl), "utf8")}`;
  vm.runInNewContext(source, {
    module: { exports: {} },
    process: { argv: [`--megle-window=${role}`] },
    require(id: string) {
      if (id !== "electron") throw new Error(`unexpected preload require: ${id}`);
      return {
        contextBridge: {
          exposeInMainWorld(_name: string, value: Record<string, unknown>) {
            exposed = value;
          }
        },
        ipcRenderer
      };
    }
  });
  assert.ok(exposed);
  return { bridge: exposed, invokes, listeners, sent };
}

test("preload log setup survives StrictMode cleanup, rapid filters, and late invoke completion", async () => {
  const harness = evaluatePreload("notifications");
  const notifications = harness.bridge.notifications as {
    snapshotAndSubscribeLogs(
      query: unknown,
      cb: (batch: unknown) => void
    ): {
      ready: Promise<{ snapshot: unknown[]; afterSequence: number }>;
      unsubscribe(): void;
    };
  };
  const firstBatches: unknown[] = [];
  const secondBatches: unknown[] = [];

  const first = notifications.snapshotAndSubscribeLogs({ search: "old" }, (batch) => {
    firstBatches.push(batch);
  });
  first.unsubscribe();
  const second = notifications.snapshotAndSubscribeLogs({ search: "new" }, (batch) => {
    secondBatches.push(batch);
  });
  assert.equal(harness.invokes.length, 2);

  const logListeners = [...(harness.listeners.get(channels.logsChanged) ?? [])];
  for (const listener of logListeners) {
    listener({}, {
      subscriptionId: "subscription-new",
      batch: {
        lines: [{ sequence: 18 }],
        afterSequence: 18,
        droppedBeforeSequence: null
      }
    });
    listener({}, {
      subscriptionId: "subscription-new",
      batch: {
        lines: [{ sequence: 19 }],
        afterSequence: 19,
        droppedBeforeSequence: null
      }
    });
  }

  harness.invokes[1]!.deferred.resolve({
    subscriptionId: "subscription-new",
    snapshot: [{ sequence: 17 }],
    afterSequence: 17
  });
  assert.deepEqual(JSON.parse(JSON.stringify(await second.ready)), {
    snapshot: [{ sequence: 17 }],
    afterSequence: 17
  });
  for (const listener of logListeners) {
    listener({}, {
      subscriptionId: "subscription-new",
      batch: { lines: [{ sequence: 21 }], afterSequence: 21, droppedBeforeSequence: null }
    });
  }

  harness.invokes[0]!.deferred.resolve({
    subscriptionId: "subscription-old",
    snapshot: [{ sequence: 10 }],
    afterSequence: 10
  });
  await first.ready;
  assert.deepEqual(firstBatches, []);
  assert.deepEqual(JSON.parse(JSON.stringify(secondBatches)), [
    {
      lines: [{ sequence: 18 }, { sequence: 19 }],
      afterSequence: 19,
      droppedBeforeSequence: null
    },
    { lines: [{ sequence: 21 }], afterSequence: 21, droppedBeforeSequence: null }
  ]);
  assert.deepEqual(harness.sent, [[channels.logsUnsubscribe, "subscription-old"]]);

  second.unsubscribe();
  assert.deepEqual(harness.sent.at(-1), [channels.logsUnsubscribe, "subscription-new"]);
  assert.equal((harness.listeners.get(channels.logsChanged) ?? new Set()).size, 0);
});

test("notifications preload exposes only notification, log, and common window capabilities", () => {
  const notificationBridge = evaluatePreload("notifications").bridge;
  assert.equal(typeof notificationBridge.notifications, "object");
  assert.equal(typeof notificationBridge.windowControls, "object");
  assert.equal(typeof notificationBridge.notifySecondaryWindowReady, "function");
  assert.equal(typeof notificationBridge.closeNotificationsWindow, "function");
  for (const forbidden of [
    "coreUrl",
    "sessionToken",
    "shell",
    "pickFolder",
    "diagnostics",
    "listLibraries",
    "switchLibrary",
    "createLibrary",
    "openLibrary",
    "removeLibraryFromList",
    "deleteLibraryFile"
  ]) {
    assert.equal(forbidden in notificationBridge, false, `${forbidden} must not be exposed to notifications`);
  }

  const mainBridge = evaluatePreload("main").bridge;
  const settingsBridge = evaluatePreload("settings").bridge;
  assert.equal(typeof mainBridge.switchLibrary, "function");
  assert.equal(typeof mainBridge.shell, "object");
  assert.equal(typeof settingsBridge.switchLibrary, "function");
  assert.equal(typeof settingsBridge.shell, "object");

  const unknownBridge = evaluatePreload("unknown").bridge;
  assert.deepEqual(Object.keys(unknownBridge), ["windowRole"]);
});

test("Main role permissions reject notification senders for library and shell IPC", () => {
  assert.ok(permissionsModule, "desktop IPC role permissions must exist");
  for (const channel of [
    "megle:switch-library",
    "megle:create-library",
    "megle:open-library",
    "megle:remove-library-from-list",
    "megle:delete-library-file",
    "megle:shell-reveal-path",
    "megle:shell-open-path"
  ]) {
    assert.equal(permissionsModule.canAccessDesktopIpc("notifications", channel), false);
    assert.equal(permissionsModule.canAccessDesktopIpc("main", channel), true);
    assert.equal(permissionsModule.canAccessDesktopIpc("settings", channel), true);
  }
});

test("secondary window opening is single-flight per role and recovers after load failure", async () => {
  assert.ok(openingModule, "secondary window opening coordinator must exist");
  const coordinator = new openingModule.SecondaryWindowOpeningCoordinator();
  const firstGate = deferred<void>();
  let creates = 0;
  const first = coordinator.run("notifications", async () => {
    creates += 1;
    await firstGate.promise;
  });
  const concurrent = coordinator.run("notifications", async () => {
    creates += 1;
  });
  assert.equal(creates, 1);
  firstGate.resolve();
  await Promise.all([first, concurrent]);
  assert.equal(creates, 1);

  await assert.rejects(
    coordinator.run("notifications", async () => {
      creates += 1;
      throw new Error("load failed");
    }),
    /load failed/
  );
  await coordinator.run("notifications", async () => {
    creates += 1;
  });
  assert.equal(creates, 3, "a failed opening must release the role for a clean retry");
});

test("shutdown stop failure restores live notifications and a later retry can finalize", async () => {
  assert.ok(shutdownModule, "recoverable notification shutdown coordinator must exist");
  const scheduled: Array<() => void> = [];
  const snapshots: string[] = [];
  const queue = new ipcModule.LatestValueBroadcastQueue<string>(
    (value: string) => snapshots.push(value),
    (flush: () => void) => scheduled.push(flush)
  );
  let attached = true;
  let runtimeOpen = true;
  let stopAttempts = 0;
  const shutdown = () =>
    shutdownModule.runRecoverableNotificationShutdown({
      quiesceNotifications: async () => {
        attached = false;
      },
      stopCore: async () => {
        stopAttempts += 1;
        if (stopAttempts === 1) throw new Error("Core stop timeout");
      },
      restoreNotifications: async () => {
        attached = true;
        queue.publish("restored-live-snapshot");
      },
      closeNotifications: async () => {
        runtimeOpen = false;
        queue.close();
      }
    });

  await assert.rejects(shutdown(), /Core stop timeout/);
  scheduled.shift()?.();
  assert.equal(attached, true);
  assert.equal(runtimeOpen, true);
  assert.deepEqual(snapshots, ["restored-live-snapshot"]);

  await shutdown();
  assert.equal(attached, false);
  assert.equal(runtimeOpen, false);
  assert.equal(stopAttempts, 2);
});

test("hardening source contract uses shared DTOs, sender authorization, and failed-window cleanup", () => {
  const root = fileURLToPath(new URL("../../..", import.meta.url));
  const read = (relativePath: string) => readFileSync(`${root}/${relativePath}`, "utf8");
  const main = read("apps/desktop/src/main.ts");
  const preload = read("apps/desktop/src/preload.cjs");
  const webDesktop = read("apps/web/src/core/desktop.ts");
  const desktopTypes = read("apps/desktop/src/notification-types.ts");
  const contractPath = `${root}/packages/notification-contract/src/index.ts`;

  assert.equal(existsSync(contractPath), true);
  const contract = existsSync(contractPath) ? read("packages/notification-contract/src/index.ts") : "";
  assert.match(contract, /import type \{[\s\S]*NotificationOperationSnapshot[\s\S]*\} from "@megle\/core-client\/generated-contract"/);
  assert.match(contract, /interface NotificationIpcBridge/);
  assert.doesNotMatch(contract, /onLogs/);
  assert.match(desktopTypes, /from "@megle\/notification-contract"/);
  assert.match(webDesktop, /from "@megle\/notification-contract"/);
  assert.doesNotMatch(webDesktop, /interface DesktopNotificationOperation/);

  assert.match(main, /`--megle-window=main`/);
  for (const channel of [
    "megle:switch-library",
    "megle:create-library",
    "megle:remove-library-from-list",
    "megle:shell-reveal-path",
    "megle:shell-open-path"
  ]) {
    assert.match(main, new RegExp(`requireDesktopIpcAccess\\(event, "${channel}"\\)`));
  }
  assert.match(main, /const session = await ensureCoreReady\(\);[\s\S]*focusExistingSecondaryWindow\(role\)/);
  assert.match(main, /render-process-gone[\s\S]*destroyFailedSecondaryWindow/);
  assert.match(main, /did-fail-load[\s\S]*destroyFailedSecondaryWindow/);
  assert.match(main, /processLogSubscriptions\.clearTarget/);
  assert.match(preload, /windowRole === "notifications"/);
  assert.doesNotMatch(preload, /onLogs:/);
});
