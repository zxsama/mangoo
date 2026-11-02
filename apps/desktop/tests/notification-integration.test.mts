import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import test from "node:test";
import vm from "node:vm";

const ipcModuleUrl = new URL("../src/notification-ipc.ts", import.meta.url);
const runtimeModuleUrl = new URL("../src/notification-runtime.ts", import.meta.url);
const transitionModuleUrl = new URL("../src/notification-core-transition.ts", import.meta.url);
const lifecycleModuleUrl = new URL("../src/core-lifecycle-queue.ts", import.meta.url);
const channelsUrl = new URL("../src/notification-ipc-channels.cjs", import.meta.url);
const ipcModule = existsSync(fileURLToPath(ipcModuleUrl)) ? await import(ipcModuleUrl.href) : null;
const runtimeModule = existsSync(fileURLToPath(runtimeModuleUrl))
  ? await import(runtimeModuleUrl.href)
  : null;
const transitionModule = existsSync(fileURLToPath(transitionModuleUrl))
  ? await import(transitionModuleUrl.href)
  : null;
const lifecycleModule = existsSync(fileURLToPath(lifecycleModuleUrl))
  ? await import(lifecycleModuleUrl.href)
  : null;
const channels = existsSync(fileURLToPath(channelsUrl))
  ? createRequire(import.meta.url)(fileURLToPath(channelsUrl))
  : null;

test("notification IPC permissions isolate privileged history, action and log access", () => {
  assert.ok(ipcModule, "notification IPC boundary module must exist");
  assert.ok(channels, "notification IPC channels must be centralized");
  const canAccess = ipcModule.canAccessNotificationIpc;

  assert.equal(canAccess("main", channels.compact), true);
  assert.equal(canAccess("notifications", channels.compact), true);
  assert.equal(canAccess("settings", channels.compact), false);
  assert.equal(canAccess("notifications", channels.history), true);
  assert.equal(canAccess("main", channels.history), false);
  assert.equal(canAccess("notifications", channels.historyFacets), true);
  assert.equal(canAccess("main", channels.historyFacets), false);
  assert.equal(canAccess("notifications", channels.cancel), true);
  assert.equal(canAccess("plugins", channels.cancel), false);
  assert.equal(canAccess("notifications", channels.logsPage), true);
  assert.equal(canAccess("main", channels.logsPage), false);
  assert.equal(canAccess("notifications", channels.logsExport), true);
  assert.equal(canAccess("main", channels.logsExport), false);
  assert.equal(canAccess("main", channels.thumbnailSummary), true);
  assert.equal(canAccess("settings", channels.thumbnailSummary), true);
  assert.equal(canAccess("notifications", channels.thumbnailSummary), false);
  assert.equal(canAccess("settings", channels.openWindow), true);
  assert.equal(canAccess("notifications", channels.focusMedia), true);
  assert.equal(canAccess("settings", channels.focusMedia), false);
  assert.equal(canAccess("main", channels.focusMediaCompleted), true);
  assert.equal(canAccess("notifications", channels.focusMediaCompleted), false);
  assert.equal(canAccess("main", channels.focusMediaStage), true);
  assert.equal(canAccess("notifications", channels.focusMediaStage), false);
  assert.equal(canAccess("plugins", channels.rendererLogs), true);
  assert.equal(canAccess(null, channels.rendererLogs), false);
});

test("notification launch intents and media focus requests are narrow and bounded", () => {
  const libraryUuid = "00000000-0000-7000-8000-000000000101";
  const rootUuid = "00000000-0000-7000-8000-000000000102";
  assert.deepEqual(
    ipcModule.parseNotificationWindowIntent({
      page: "logs",
      logFilter: { eventKind: "thumbnail_failure", libraryId: "library-a" },
      failureSnapshot: { totalCount: 101, visibleCount: 100, truncated: true }
    }),
    {
      page: "logs",
      logFilter: { eventKind: "thumbnail_failure", libraryId: "library-a" },
      failureSnapshot: { totalCount: 101, visibleCount: 100, truncated: true }
    }
  );
  assert.deepEqual(
    ipcModule.parseNotificationWindowIntent({
      page: "sync-folders",
      libraryUuid,
      rootUuid
    }),
    { page: "sync-folders", libraryUuid, rootUuid }
  );
  assert.deepEqual(
    ipcModule.parseManagedRootNotificationTarget({
      libraryId: libraryUuid,
      operationId: "placement-operation",
      rootUuid,
      inventoryRevision: 3
    }),
    { libraryId: libraryUuid, operationId: "placement-operation", rootUuid, inventoryRevision: 3 }
  );
  assert.deepEqual(
    ipcModule.parseMediaFocusRequest({ libraryId: "library-a", fileId: 42, folderId: 7, rootId: 3 }),
    { libraryId: "library-a", fileId: 42, folderId: 7, rootId: 3 }
  );
  assert.deepEqual(
    ipcModule.parseMediaFocusCompletion({ requestId: "focus-1", ok: true }),
    { requestId: "focus-1", ok: true }
  );
  assert.deepEqual(
    ipcModule.parseMediaFocusStageUpdate({ requestId: "focus-1", stage: "switching" }),
    { requestId: "focus-1", stage: "switching" }
  );
  assert.throws(
    () => ipcModule.parseNotificationWindowIntent({ page: "history", logFilter: {} }),
    /intent/i
  );
  assert.throws(
    () => ipcModule.parseNotificationWindowIntent({ page: "sync-folders", libraryUuid: "bad", rootUuid }),
    /UUID/i
  );
  assert.throws(
    () => ipcModule.parseManagedRootNotificationTarget({ libraryId: libraryUuid, operationId: "", rootUuid, inventoryRevision: 3 }),
    /operationId/i
  );
  assert.throws(
    () => ipcModule.parseMediaFocusRequest({ libraryId: "library-a", fileId: 0, folderId: 7, rootId: 3 }),
    /fileId/i
  );
  assert.throws(
    () => ipcModule.parseMediaFocusCompletion({ requestId: "focus-1", ok: "yes" }),
    /boolean/i
  );
  assert.throws(
    () => ipcModule.parseMediaFocusStageUpdate({ requestId: "focus-1", stage: "done" }),
    /stage/i
  );
  assert.throws(
    () => ipcModule.parseThumbnailCacheSummary({
      libraryId: "library-a",
      libraryName: "Artists",
      observedAt: 1,
      cachedCount: 1,
      totalCandidateCount: 2,
      missingCount: 0,
      staleCount: 0,
      failedCount: 1,
      pendingCount: 0,
      activeBulkTaskCount: 0,
      totalCacheBytes: 1,
      completionPercent: 50,
      failureDetails: [],
      failureDetailsTruncated: false,
      failureDetailsUnavailable: false
    }),
    /failureDetails/i
  );
});

test("notification IPC validators reject malformed or unbounded renderer payloads", () => {
  assert.ok(ipcModule);

  assert.throws(() => ipcModule.parseNotificationHistoryQuery({ limit: 201 }), /limit/i);
  assert.throws(() => ipcModule.parseNotificationHistoryQuery({ search: "x".repeat(501) }), /search/i);
  assert.throws(() => ipcModule.parseNotificationHistoryQuery({ origins: ["invalid"] }), /origins/i);
  assert.throws(() => ipcModule.parseNotificationHistoryQuery({ kinds: ["x".repeat(129)] }), /kinds/i);
  assert.throws(() => ipcModule.parseNotificationAction("", "task:1"), /library/i);
  assert.throws(() => ipcModule.parseNotificationAction("library", "x".repeat(257)), /operation/i);
  assert.throws(
    () =>
      ipcModule.parseRendererErrorReport({
        libraryId: "library",
        libraryName: "Library",
        source: "view",
        code: "E_VIEW",
        target: "grid",
        title: "Failure",
        message: "x".repeat(4_001),
        detail: null
      }),
    /message/i
  );
  assert.throws(
    () =>
      ipcModule.parseRendererErrorReport({
        reportId: "x".repeat(257),
        libraryId: "library",
        libraryName: "Library",
        source: "view",
        code: "E_VIEW",
        target: "grid",
        title: "Failure",
        message: "failed",
        detail: null
      }),
    /reportId/i
  );
  assert.throws(
    () =>
      ipcModule.parseThumbnailCacheSummary({
        libraryId: "library",
        libraryName: "Library",
        observedAt: 1,
        cachedCount: -1,
        totalCandidateCount: 0,
        missingCount: 0,
        staleCount: 0,
        failedCount: 0,
        pendingCount: 0,
        activeBulkTaskCount: 0,
        totalCacheBytes: null,
        completionPercent: null,
        failureDetails: [],
        failureDetailsTruncated: false,
        failureDetailsUnavailable: false
      }),
    /cachedCount/i
  );
  assert.throws(() => ipcModule.parseProcessLogFilter({ limit: 0 }), /limit/i);
  assert.throws(() => ipcModule.parseRendererLogEntries(new Array(101).fill({ level: "info", message: "x" })), /100/);

  assert.deepEqual(ipcModule.parseNotificationHistoryQuery({ limit: 25, offset: 5 }), {
    limit: 25,
    offset: 5
  });
  assert.deepEqual(
    ipcModule.parseNotificationHistoryQuery({ origins: ["background"], kinds: ["root_scan"] }),
    { origins: ["background"], kinds: ["root_scan"] }
  );
  assert.deepEqual(ipcModule.parseNotificationAction("library", "task:1"), {
    libraryId: "library",
    operationId: "task:1"
  });
  assert.equal(
    ipcModule.parseRendererErrorReport({
      reportId: "renderer-attempt-1",
      libraryId: "library",
      libraryName: "Library",
      source: "view",
      code: "E_VIEW",
      target: "grid",
      title: "Failure",
      message: "failed",
      detail: null
    }).reportId,
    "renderer-attempt-1"
  );
});

test("notification snapshot broadcasts coalesce to the newest bounded update", () => {
  assert.ok(ipcModule);
  const scheduled: Array<() => void> = [];
  const delivered: number[] = [];
  const queue = new ipcModule.LatestValueBroadcastQueue<number>(
    (value: number) => delivered.push(value),
    (flush: () => void) => scheduled.push(flush)
  );

  queue.publish(1);
  queue.publish(2);
  queue.publish(3);
  assert.equal(scheduled.length, 1);
  scheduled.shift()?.();
  assert.deepEqual(delivered, [3]);

  queue.close();
  queue.publish(4);
  assert.equal(scheduled.length, 0);
});

test("process log subscriptions replace tokens, preserve gap batches and clean up on destroy", () => {
  assert.ok(ipcModule);
  let subscriber: ((batch: unknown) => void) | null = null;
  let unsubscribeCount = 0;
  const buffer = {
    snapshotAndSubscribe(next: (batch: unknown) => void) {
      subscriber = next;
      return {
        snapshot: [{ sequence: 4 }],
        afterSequence: 4,
        unsubscribe: () => {
          unsubscribeCount += 1;
        }
      };
    }
  };
  class Target extends EventEmitter {
    readonly id = 17;
    destroyed = false;
    sent: Array<[string, unknown]> = [];
    isDestroyed() {
      return this.destroyed;
    }
    send(channel: string, payload: unknown) {
      this.sent.push([channel, payload]);
    }
  }
  const target = new Target();
  const subscriptionIds = ["first", "second"];
  const registry = new ipcModule.ProcessLogIpcSubscriptions(
    buffer,
    "logs-changed",
    () => subscriptionIds.shift()
  );

  const first = registry.snapshotAndSubscribe(target, {});
  assert.deepEqual(first, {
    subscriptionId: "first",
    snapshot: [{ sequence: 4 }],
    afterSequence: 4
  });
  const second = registry.snapshotAndSubscribe(target, {});
  assert.equal(unsubscribeCount, 1, "a sender may own only one live log subscription");
  assert.equal(registry.unsubscribe(target.id, first.subscriptionId), false);

  subscriber?.({ lines: [], afterSequence: 9, droppedBeforeSequence: 6 });
  assert.deepEqual(target.sent, [
    [
      "logs-changed",
      {
        subscriptionId: "second",
        batch: { lines: [], afterSequence: 9, droppedBeforeSequence: 6 }
      }
    ]
  ]);

  target.destroyed = true;
  target.emit("destroyed");
  assert.equal(unsubscribeCount, 2);
  assert.equal(registry.size, 0);
});

test("notification runtime starts once, switches libraries in order and closes once", async () => {
  assert.ok(runtimeModule, "notification lifecycle coordinator must exist");
  let starts = 0;
  let closes = 0;
  const attached: string[] = [];
  const service = {
    async attachLibrary(library: { id: string }) {
      attached.push(library.id);
    },
    async detachLibrary() {
      attached.push("detached");
    },
    async close() {
      closes += 1;
    }
  };
  const runtime = new runtimeModule.NotificationRuntime(async () => {
    starts += 1;
    await Promise.resolve();
    return service;
  });

  const [first, second] = await Promise.all([runtime.start(), runtime.start()]);
  assert.equal(first, service);
  assert.equal(second, service);
  assert.equal(starts, 1);

  await runtime.attachLibrary({ id: "a", name: "A" }, { baseUrl: "http://a", sessionToken: "t" });
  await runtime.attachLibrary({ id: "b", name: "B" }, { baseUrl: "http://b", sessionToken: "t" });
  await runtime.detachLibrary();
  assert.deepEqual(attached, ["a", "b", "detached"]);

  await Promise.all([runtime.close(), runtime.close()]);
  assert.equal(closes, 1);
  await assert.rejects(() => runtime.start(), /closed/i);
});

function deferredBarrier(): { promise: Promise<void>; resolve: () => void } {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test("Core replacement waits for a hanging notification long-poll to abort before stop and start", async () => {
  assert.ok(transitionModule, "notification-safe Core transition must exist");
  const detachStarted = deferredBarrier();
  const detachFinished = deferredBarrier();
  const events: string[] = [];

  const transition = transitionModule.runNotificationSafeCoreTransition({
    assertCurrent: () => undefined,
    detachNotifications: async () => {
      events.push("detach:start");
      detachStarted.resolve();
      await detachFinished.promise;
      events.push("detach:done");
    },
    stopOldCore: async () => {
      events.push("stop");
    },
    startHealthyCore: async () => {
      events.push("start");
      return "new-session";
    },
    commitActiveLibrary: async () => {
      events.push("commit");
    },
    attachNotifications: async () => {
      events.push("attach:new");
    }
  });

  await detachStarted.promise;
  assert.deepEqual(events, ["detach:start"]);
  detachFinished.resolve();

  assert.equal(await transition, "new-session");
  assert.deepEqual(events, [
    "detach:start",
    "detach:done",
    "stop",
    "start",
    "commit",
    "attach:new"
  ]);
});

test("same-URL replacement events cannot reach the detached old library store or ACK", async () => {
  assert.ok(transitionModule);
  const detachStarted = deferredBarrier();
  const oldPollAborted = deferredBarrier();
  let oldLoopAttached = true;
  let oldLibraryWrites = 0;
  let oldLibraryAcks = 0;
  let newLibraryAttached = false;
  const sharedUrl = "http://127.0.0.1:48101/api";

  const transition = transitionModule.runNotificationSafeCoreTransition({
    assertCurrent: () => undefined,
    detachNotifications: async () => {
      oldLoopAttached = false;
      detachStarted.resolve();
      await oldPollAborted.promise;
    },
    stopOldCore: async () => undefined,
    startHealthyCore: async () => {
      assert.equal(oldLoopAttached, false, "old consumer must be gone before the URL is reused");
      const eventFromReplacementCore = { baseUrl: sharedUrl, sequence: 1 };
      if (oldLoopAttached && eventFromReplacementCore.baseUrl === sharedUrl) {
        oldLibraryWrites += 1;
        oldLibraryAcks += 1;
      }
      return { baseUrl: sharedUrl };
    },
    commitActiveLibrary: async () => undefined,
    attachNotifications: async () => {
      newLibraryAttached = true;
    }
  });

  await detachStarted.promise;
  oldPollAborted.resolve();
  await transition;

  assert.equal(oldLibraryWrites, 0);
  assert.equal(oldLibraryAcks, 0);
  assert.equal(newLibraryAttached, true);
});

test("a generation change immediately before attach leaves notifications detached", async () => {
  assert.ok(transitionModule);
  let currentGeneration = 7;
  let detachCount = 0;
  let attachCount = 0;

  await assert.rejects(
    transitionModule.runNotificationSafeCoreTransition({
      assertCurrent: () => {
        if (currentGeneration !== 7) throw new Error("stale generation");
      },
      detachNotifications: async () => {
        detachCount += 1;
      },
      stopOldCore: async () => undefined,
      startHealthyCore: async () => "candidate",
      commitActiveLibrary: async () => {
        currentGeneration = 8;
      },
      attachNotifications: async () => {
        attachCount += 1;
      }
    }),
    /stale generation/
  );

  assert.equal(attachCount, 0);
  assert.equal(detachCount, 2, "failed transitions must re-establish the detached state");
});

test("consecutive notification-safe switches leave only the final library loop active", async () => {
  assert.ok(transitionModule);
  assert.ok(lifecycleModule);
  const queue = new lifecycleModule.CoreLifecycleQueue();
  let activeLibrary: string | null = "old";
  let activeLoops = 1;
  let maxActiveLoops = 1;
  const attached: string[] = [];
  const transitionTo = (libraryId: string) => queue.enqueue(async () => {
    const generation = queue.beginGeneration();
    return transitionModule.runNotificationSafeCoreTransition({
      assertCurrent: () => {
        if (!queue.isCurrentGeneration(generation)) throw new Error("stale generation");
      },
      detachNotifications: async () => {
        activeLibrary = null;
        activeLoops = 0;
      },
      stopOldCore: async () => undefined,
      startHealthyCore: async () => libraryId,
      commitActiveLibrary: async () => undefined,
      attachNotifications: async (session: string) => {
        activeLibrary = session;
        activeLoops += 1;
        maxActiveLoops = Math.max(maxActiveLoops, activeLoops);
        attached.push(session);
      }
    });
  });

  const first = transitionTo("library-a");
  const second = transitionTo("library-b");
  await Promise.all([first, second]);

  assert.deepEqual(attached, ["library-a", "library-b"]);
  assert.equal(activeLibrary, "library-b");
  assert.equal(activeLoops, 1);
  assert.equal(maxActiveLoops, 1);
});

test("Task 10 source contract exposes one notifications window and typed preload bridge", () => {
  const root = fileURLToPath(new URL("../../..", import.meta.url));
  const read = (relativePath: string) => readFileSync(`${root}/${relativePath}`, "utf8");
  const main = read("apps/desktop/src/main.ts");
  const preload = read("apps/desktop/src/preload.cjs");
  const desktop = read("apps/web/src/core/desktop.ts");
  const webMain = read("apps/web/src/main.tsx");
  const windowPath = `${root}/apps/web/src/features/notifications/NotificationWindow.tsx`;

  assert.match(main, /type SecondaryWindowRole = "settings" \| "plugins" \| "notifications"/);
  assert.match(main, /route: "#\/notifications"/);
  assert.match(main, /notifications:\s*{[\s\S]*width: 1040,[\s\S]*height: 720,[\s\S]*minWidth: 820,[\s\S]*minHeight: 560/);
  assert.match(main, /backgroundMaterial: "acrylic"/);
  assert.match(main, /resolveSecondaryWindowPosition\(role, parent, config\)/);
  assert.match(main, /const existing = secondaryWindows\[role\][\s\S]*existing\.focus\(\)[\s\S]*return/);
  assert.match(main, /rememberSecondaryWindowPosition\(role, window\)/);
  assert.match(main, /\["settings", "plugins", "notifications"\] as const/);
  assert.match(main, /notificationDatabasePath\(userDataDir\(\)\)/);
  assert.match(main, /runNotificationSafeCoreTransition/);
  assert.doesNotMatch(main, /scheduleNotificationServiceAttach/);
  assert.match(main, /runRecoverableNotificationShutdown/);
  assert.match(main, /quiesceNotifications: quiesceNotificationRuntime[\s\S]*stopCore: stopDesktopCoreProcesses[\s\S]*closeNotifications: closeNotificationRuntime/);
  assert.match(preload, /notifications:\s*notificationsBridge/);
  assert.match(preload, /return \(\) => ipcRenderer\.removeListener/);
  assert.match(desktop, /DesktopNotificationsBridge[\s\S]*from "@mangoo\/notification-contract"/);
  assert.match(desktop, /windowRole\?: "main" \| DesktopSecondaryWindowRole/);
  assert.match(webMain, /windowRole === "notifications"/);
  assert.match(webMain, /import\("\.\/features\/notifications\/NotificationWindow"\)/);
  assert.equal(existsSync(windowPath), true);
});

test("preload bridge removes exact listeners and buffers log batches until snapshot readiness", async () => {
  assert.ok(channels);
  const preloadPath = fileURLToPath(new URL("../src/preload.cjs", import.meta.url));
  const source = `${readFileSync(fileURLToPath(channelsUrl), "utf8")}\n${readFileSync(preloadPath, "utf8")}`;
  const listeners = new Map<string, Set<(...args: unknown[]) => void>>();
  const removed: Array<[string, (...args: unknown[]) => void]> = [];
  const sent: Array<[string, ...unknown[]]> = [];
  const invoked: Array<[string, ...unknown[]]> = [];
  let exposed: Record<string, unknown> | null = null;
  let resolveLogSnapshot: ((value: unknown) => void) | null = null;
  const ipcRenderer = {
    invoke(channel: string, ...args: unknown[]) {
      invoked.push([channel, ...args]);
      if (channel === channels.logsSubscribe) {
        return new Promise((resolve) => {
          resolveLogSnapshot = resolve;
        });
      }
      return Promise.resolve(true);
    },
    on(channel: string, listener: (...args: unknown[]) => void) {
      const bucket = listeners.get(channel) ?? new Set();
      bucket.add(listener);
      listeners.set(channel, bucket);
    },
    removeListener(channel: string, listener: (...args: unknown[]) => void) {
      removed.push([channel, listener]);
      listeners.get(channel)?.delete(listener);
    },
    send(channel: string, ...args: unknown[]) {
      sent.push([channel, ...args]);
    }
  };
  vm.runInNewContext(source, {
    module: { exports: {} },
    process: { argv: ["--mangoo-window=notifications"] },
    require(id: string) {
      if (id === "electron") {
        return {
          contextBridge: {
            exposeInMainWorld(_name: string, value: Record<string, unknown>) {
              exposed = value;
            }
          },
          ipcRenderer
        };
      }
      throw new Error(`unexpected preload require: ${id}`);
    }
  });

  assert.ok(exposed);
  const bridge = exposed.notifications as {
    onChanged(cb: (value: unknown) => void): () => void;
    snapshotAndSubscribeLogs(
      query: unknown,
      cb: (value: unknown) => void
    ): {
      ready: Promise<{ snapshot: unknown[]; afterSequence: number }>;
      unsubscribe: () => void;
    };
  };
  assert.equal(typeof bridge.onChanged, "function");
  assert.equal(typeof bridge.snapshotAndSubscribeLogs, "function");

  const windowBridge = exposed as {
    focusMedia(input: unknown): Promise<boolean>;
    onNotificationWindowIntent(cb: (value: unknown) => void): () => void;
  };
  const intent = {
    page: "logs",
    logFilter: { eventKind: "thumbnail_failure", libraryId: "library-a" }
  };
  const intentListener = [...(listeners.get(channels.windowIntent) ?? [])][0];
  assert.ok(intentListener, "notification launch intent listener must exist before React mounts");
  intentListener({}, intent);
  const deliveredIntents: unknown[] = [];
  const stopIntents = windowBridge.onNotificationWindowIntent((value) => deliveredIntents.push(value));
  await Promise.resolve();
  assert.deepEqual(JSON.parse(JSON.stringify(deliveredIntents)), [intent]);
  stopIntents();
  await windowBridge.focusMedia({ libraryId: "library-a", fileId: 42, folderId: 7, rootId: 3 });
  assert.deepEqual(invoked.at(-1), [
    channels.focusMedia,
    { libraryId: "library-a", fileId: 42, folderId: 7, rootId: 3 }
  ]);
  const changed = () => undefined;
  const stopChanged = bridge.onChanged(changed);
  const changedListener = [...(listeners.get(channels.changed) ?? [])][0];
  assert.ok(changedListener);
  stopChanged();
  assert.deepEqual(removed.at(-1), [channels.changed, changedListener]);

  const delivered: unknown[] = [];
  const subscription = bridge.snapshotAndSubscribeLogs({}, (batch) => delivered.push(batch));
  const logListener = [...(listeners.get(channels.logsChanged) ?? [])][0];
  assert.ok(logListener);
  const earlyBatch = { lines: [], afterSequence: 7, droppedBeforeSequence: 4 };
  logListener({}, { subscriptionId: "logs-1", batch: earlyBatch });
  assert.deepEqual(delivered, []);
  resolveLogSnapshot?.({ subscriptionId: "logs-1", snapshot: [{ sequence: 6 }], afterSequence: 6 });
  const initial = await subscription.ready;
  assert.deepEqual(JSON.parse(JSON.stringify(delivered)), [earlyBatch]);
  assert.deepEqual(JSON.parse(JSON.stringify(initial.snapshot)), [{ sequence: 6 }]);
  subscription.unsubscribe();
  assert.deepEqual(removed.at(-1), [channels.logsChanged, logListener]);
  assert.deepEqual(sent.at(-1), [channels.logsUnsubscribe, "logs-1"]);
});
