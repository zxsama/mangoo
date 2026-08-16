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

function createNavigationTargetHarness(ids = ["subscription-old", "subscription-new"]) {
  assert.ok(ipcModule);
  assert.equal(typeof ipcModule.NotificationIpcBroadcastTargets, "function");
  assert.equal(typeof ipcModule.NotificationIpcTargetLifecycle, "function");
  const subscribers: Array<(batch: unknown) => void> = [];
  let underlyingUnsubscribes = 0;
  const logs = new ipcModule.ProcessLogIpcSubscriptions(
    {
      snapshotAndSubscribe(subscriber: (batch: unknown) => void) {
        subscribers.push(subscriber);
        return {
          snapshot: [],
          afterSequence: 0,
          unsubscribe: () => {
            underlyingUnsubscribes += 1;
          }
        };
      }
    },
    channels.logsChanged,
    () => ids.shift()
  );
  const broadcasts = new ipcModule.NotificationIpcBroadcastTargets(channels.changed);
  const lifecycle = new ipcModule.NotificationIpcTargetLifecycle((targetId: number) => {
    logs.clearTarget(targetId);
    broadcasts.clearTarget(targetId);
  });
  const target = new LogTarget();
  lifecycle.track(target);
  return {
    broadcasts,
    lifecycle,
    logs,
    subscribers,
    target,
    get underlyingUnsubscribes() {
      return underlyingUnsubscribes;
    }
  };
}

test("main-frame navigation clears notification and log targets before later delivery", () => {
  const harness = createNavigationTargetHarness();
  harness.broadcasts.register(harness.target);
  harness.logs.snapshotAndSubscribe(harness.target, {});

  harness.target.emit("did-start-navigation", {}, "http://localhost/#/notifications", false, true);

  assert.equal(harness.logs.size, 0);
  assert.equal(harness.broadcasts.size, 0);
  assert.equal(harness.underlyingUnsubscribes, 1);
  harness.subscribers[0]?.({ lines: [], afterSequence: 1, droppedBeforeSequence: null });
  harness.broadcasts.broadcast({ marker: "stale" });
  assert.deepEqual(harness.target.sent, [], "old renderer targets must receive no post-navigation IPC");
  harness.lifecycle.release(harness.target.id);
});

test("subframe navigation preserves notification and log targets", () => {
  const harness = createNavigationTargetHarness();
  harness.broadcasts.register(harness.target);
  harness.logs.snapshotAndSubscribe(harness.target, {});

  harness.target.emit("did-start-navigation", {}, "http://localhost/embedded", false, false);

  assert.equal(harness.logs.size, 1);
  assert.equal(harness.broadcasts.size, 1);
  assert.equal(harness.underlyingUnsubscribes, 0);
  harness.subscribers[0]?.({ lines: [], afterSequence: 2, droppedBeforeSequence: null });
  harness.broadcasts.broadcast({ marker: "live" });
  assert.deepEqual(
    harness.target.sent.map(([channel]) => channel),
    [channels.logsChanged, channels.changed]
  );
  harness.lifecycle.release(harness.target.id);
  assert.equal(harness.target.listenerCount("did-start-navigation"), 0);
});

test("reloaded pages can resubscribe without stale cleanup cancelling the new token", () => {
  const harness = createNavigationTargetHarness();
  harness.lifecycle.track(harness.target);
  assert.equal(harness.target.listenerCount("did-start-navigation"), 1);
  harness.broadcasts.register(harness.target);
  const first = harness.logs.snapshotAndSubscribe(harness.target, {});
  harness.target.emit("did-start-navigation", {}, "http://localhost/#/notifications", false, true);

  harness.broadcasts.register(harness.target);
  const second = harness.logs.snapshotAndSubscribe(harness.target, {});
  assert.equal(harness.logs.unsubscribe(harness.target.id, first.subscriptionId), false);
  harness.subscribers[0]?.({ lines: [], afterSequence: 3, droppedBeforeSequence: null });
  harness.subscribers[1]?.({ lines: [], afterSequence: 4, droppedBeforeSequence: null });

  assert.equal(harness.logs.size, 1);
  assert.equal(harness.broadcasts.size, 1);
  assert.deepEqual(harness.target.sent, [
    [
      channels.logsChanged,
      {
        subscriptionId: second.subscriptionId,
        batch: { lines: [], afterSequence: 4, droppedBeforeSequence: null }
      }
    ]
  ]);
  harness.lifecycle.release(harness.target.id);
  assert.equal(harness.target.listenerCount("did-start-navigation"), 0);
});

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
    "updateMediaFocusStage",
    "createLibrary",
    "openLibrary",
    "removeLibraryFromList",
    "deleteLibraryFile"
  ]) {
    assert.equal(forbidden in notificationBridge, false, `${forbidden} must not be exposed to notifications`);
  }

  const mainBridge = evaluatePreload("main").bridge;
  const settingsBridge = evaluatePreload("settings").bridge;
  assert.equal("switchLibrary" in mainBridge, false);
  assert.equal(typeof mainBridge.getLibraryPhase, "function");
  assert.equal(typeof mainBridge.retryLibraryLoad, "function");
  assert.equal(typeof mainBridge.updateMediaFocusStage, "function");
  assert.equal(typeof mainBridge.shell, "object");
  assert.equal("switchLibrary" in settingsBridge, false);
  assert.equal(typeof settingsBridge.shell, "object");

  const unknownBridge = evaluatePreload("unknown").bridge;
  assert.deepEqual(Object.keys(unknownBridge), ["windowRole"]);
});

test("Main role permissions reject notification senders for library and shell IPC", () => {
  assert.ok(permissionsModule, "desktop IPC role permissions must exist");
  for (const channel of [
    "megle:retry-library-load",
    "megle:pick-new-library-path",
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

test("Library lifecycle is absent from Desktop IPC and owned by Core Host", () => {
  const root = fileURLToPath(new URL("../../..", import.meta.url));
  const main = readFileSync(`${root}/apps/desktop/src/main.ts`, "utf8");
  const executableMain = main.replace(/\/\*[\s\S]*?\*\//g, "");
  assert.doesNotMatch(executableMain, /libraries\.json|restartCoreForLibrary|megle:open-library|megle:delete-library-file/);
  assert.match(main, /megle:pick-new-library-path/);
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
    "megle:pick-new-library-path",
    "megle:shell-reveal-path",
    "megle:shell-open-path"
  ]) {
    assert.match(main, new RegExp(`requireDesktopIpcAccess\\(event, "${channel}"\\)`));
  }
  assert.match(main, /const session = await ensureCoreReady\(\);[\s\S]*focusExistingSecondaryWindow\(role\)/);
  assert.match(main, /render-process-gone[\s\S]*destroyFailedSecondaryWindow/);
  assert.match(main, /did-fail-load[\s\S]*destroyFailedSecondaryWindow/);
  assert.match(
    main,
    /did-start-navigation[\s\S]{0,500}mediaFocusCoordinator\.failAll\(\)/,
    "a main-frame reload must settle pending media focus requests"
  );
  assert.match(
    main,
    /Main renderer failed to load[\s\S]{0,300}mediaFocusCoordinator\.failAll\(\)/,
    "a main renderer load failure must settle pending media focus requests"
  );
  assert.match(
    main,
    /Main renderer process gone[\s\S]{0,300}mediaFocusCoordinator\.failAll\(\)/,
    "a main renderer crash must settle pending media focus requests"
  );
  assert.match(
    main,
    /ipcMain\.handle\(NOTIFICATION_IPC_CHANNELS\.compact, async \(event\) => \{\s*requireNotificationIpcAccess\([^;]+;\s*notificationBroadcastTargets\.register\(event\.sender\);\s*const service = await ensureNotificationService\(\)/
  );
  assert.match(main, /notificationIpcTargetLifecycle\.track\(window\.webContents\)/);
  assert.match(main, /releaseIpcTargetLifecycle\(\)/);
  assert.match(read("apps/desktop/src/notification-ipc.ts"), /did-start-navigation/);
  assert.match(preload, /windowRole === "notifications"/);
  assert.doesNotMatch(preload, /onLogs:/);
  assert.match(preload, /NOTIFICATION_IPC_CHANNELS\.historyFacets/);
  assert.match(preload, /NOTIFICATION_IPC_CHANNELS\.logsExport/);
  assert.match(main, /dialog\.showSaveDialog/);
  assert.match(main, /NOTIFICATION_IPC_CHANNELS\.logsExport/);
  assert.match(main, /processLogBuffer\.snapshot/);
});
