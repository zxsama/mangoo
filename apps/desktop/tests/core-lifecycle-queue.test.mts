import assert from "node:assert/strict";
import { createServer } from "node:http";
import { test } from "node:test";
import {
  CoreLifecycleQueue,
  LifecycleCancelledError,
  LifecycleClosedError
} from "../src/core-lifecycle-queue.ts";
import { sessionForDbPath } from "../src/core-session.ts";
import { waitForCoreHealth } from "../src/core-process.ts";

function deferred<T = void>(): {
  readonly promise: Promise<T>;
  readonly resolve: (value: T | PromiseLike<T>) => void;
} {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

test("serializes concurrent Core child/session operations", async () => {
  const queue = new CoreLifecycleQueue();
  const firstGate = deferred();
  let activeChildren = 0;
  let maxActiveChildren = 0;
  const startedSessions: string[] = [];

  const start = (session: string, gate?: Promise<void>) => queue.enqueue(async () => {
    activeChildren += 1;
    maxActiveChildren = Math.max(maxActiveChildren, activeChildren);
    startedSessions.push(session);
    if (gate) await gate;
    activeChildren -= 1;
    return session;
  });

  const first = start("session-a", firstGate.promise);
  const second = start("session-b");
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.deepEqual(startedSessions, ["session-a"]);
  assert.equal(maxActiveChildren, 1);

  firstGate.resolve();
  assert.equal(await first, "session-a");
  assert.equal(await second, "session-b");
  assert.deepEqual(startedSessions, ["session-a", "session-b"]);
  assert.equal(maxActiveChildren, 1);
});

test("queued secondary open rechecks a completed switch and reuses its only child", async () => {
  const queue = new CoreLifecycleQueue();
  const switchStarted = deferred();
  const switchHealthy = deferred();
  let childStarts = 0;
  let childStops = 0;
  let currentChild: { readonly stop: () => Promise<void> } | null = null;
  let currentSession: string | null = null;

  const switching = queue.enqueue(async () => {
    const generation = queue.beginGeneration();
    childStarts += 1;
    const child = {
      stop: async () => {
        childStops += 1;
      }
    };
    switchStarted.resolve();
    await switchHealthy.promise;
    if (!queue.isCurrentGeneration(generation)) {
      await child.stop();
      throw new LifecycleCancelledError("stale switch result");
    }
    currentChild = child;
    currentSession = "switched-session";
    return currentSession;
  });
  await switchStarted.promise;

  const secondaryOpen = queue.enqueue(async () => {
    const observedGeneration = queue.currentGeneration;
    if (
      currentChild &&
      currentSession &&
      queue.isCurrentGeneration(observedGeneration)
    ) {
      return currentSession;
    }
    const generation = queue.beginGeneration();
    childStarts += 1;
    if (!queue.isCurrentGeneration(generation)) throw new LifecycleCancelledError();
    currentSession = "secondary-started-session";
    return currentSession;
  });

  switchHealthy.resolve();
  assert.equal(await switching, "switched-session");
  assert.equal(await secondaryOpen, "switched-session");
  assert.equal(childStarts, 1);

  await queue.shutdown(async () => {
    await currentChild?.stop();
    currentChild = null;
  });
  assert.equal(childStops, 1);
});

test("shutdown generation invalidates an in-flight start before handle commit", async () => {
  const queue = new CoreLifecycleQueue();
  const startCreated = deferred();
  const healthGate = deferred();
  let committedChild = false;
  let childStops = 0;

  const starting = queue.enqueue(async () => {
    const generation = queue.beginGeneration();
    const child = {
      stop: async () => {
        childStops += 1;
      }
    };
    startCreated.resolve();
    await healthGate.promise;
    if (!queue.isCurrentGeneration(generation)) {
      await child.stop();
      throw new LifecycleCancelledError("stale start result");
    }
    committedChild = true;
  });
  await startCreated.promise;

  const shutdown = queue.shutdown(async () => undefined);
  healthGate.resolve();

  await assert.rejects(starting, LifecycleCancelledError);
  await shutdown;
  assert.equal(committedChild, false);
  assert.equal(childStops, 1);
});

test("shutdown cancels queued/active library work and prevents later starts", async () => {
  const queue = new CoreLifecycleQueue();
  const activeStarted = deferred();
  const events: string[] = [];

  const active = queue.enqueue(async (signal) => {
    events.push("active-start");
    activeStarted.resolve();
    await new Promise<void>((resolve, reject) => {
      signal.addEventListener("abort", () => {
        events.push("active-cancelled");
        reject(new LifecycleCancelledError("active library operation cancelled"));
      }, { once: true });
    });
    return "unreachable";
  });
  const queued = queue.enqueue(async () => {
    events.push("queued-start");
    return "unreachable";
  });
  await activeStarted.promise;

  const shutdown = queue.shutdown(async () => {
    events.push("shutdown-start");
  });

  await assert.rejects(active, LifecycleCancelledError);
  await assert.rejects(queued, LifecycleCancelledError);
  await shutdown;
  assert.deepEqual(events, ["active-start", "active-cancelled", "shutdown-start"]);

  await assert.rejects(
    queue.enqueue(async () => "unreachable"),
    LifecycleClosedError
  );
});

test("failed shutdown can retry without reopening library work", async () => {
  const queue = new CoreLifecycleQueue();
  let attempts = 0;
  const stop = () => queue.shutdown(async () => {
    attempts += 1;
    if (attempts === 1) throw new Error("Core stop timeout");
  });

  await assert.rejects(stop(), /Core stop timeout/);
  await assert.rejects(queue.enqueue(async () => "unreachable"), LifecycleClosedError);
  await stop();

  assert.equal(attempts, 2);
  await assert.rejects(queue.enqueue(async () => "unreachable"), LifecycleClosedError);
});

test("health polling aborts promptly when shutdown cancels the active lifecycle", async () => {
  const server = createServer(() => {
    // Leave the response pending so the AbortSignal is the only completion.
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  assert.ok(address && typeof address !== "string");
  const controller = new AbortController();
  const health = waitForCoreHealth(
    `http://127.0.0.1:${address.port}/api`,
    "session-token",
    5_000,
    controller.signal
  );

  setTimeout(() => controller.abort(), 10);
  await assert.rejects(health, LifecycleCancelledError);
  server.closeAllConnections();
  await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
});

test("candidate library session retries with the same URL, port, and token", () => {
  const base = {
    baseUrl: "http://127.0.0.1:47000/api",
    bindAddress: "127.0.0.1:47000",
    dbPath: "C:/old.sqlite",
    sessionToken: "tok123",
    allowedOrigin: "http://127.0.0.1:5173"
  };
  const candidate = sessionForDbPath(base, "C:/candidate.sqlite");
  const retry = sessionForDbPath(base, "C:/candidate.sqlite", candidate);

  assert.equal(retry, candidate);
  assert.equal(retry.baseUrl, base.baseUrl);
  assert.equal(retry.bindAddress, base.bindAddress);
  assert.equal(retry.sessionToken, base.sessionToken);
  assert.equal(retry.dbPath, "C:/candidate.sqlite");
});

test("health failure keeps the candidate session for a same-port retry", async () => {
  const queue = new CoreLifecycleQueue();
  const base = {
    baseUrl: "http://127.0.0.1:47001/api",
    bindAddress: "127.0.0.1:47001",
    dbPath: "C:/old.sqlite",
    sessionToken: "tok-reused",
    allowedOrigin: "http://127.0.0.1:5173"
  };
  let candidate: typeof base | null = null;
  let healthAttempts = 0;
  const started: Array<typeof base> = [];

  const switchAttempt = () => queue.enqueue(async () => {
    candidate = sessionForDbPath(base, "C:/next.sqlite", candidate);
    started.push(candidate);
    healthAttempts += 1;
    if (healthAttempts === 1) {
      throw new Error("candidate health failed");
    }
    return candidate;
  });

  await assert.rejects(switchAttempt(), /candidate health failed/);
  const firstCandidate = candidate;
  const retried = await switchAttempt();

  assert.equal(retried, firstCandidate);
  assert.equal(started.length, 2);
  assert.deepEqual(started.map((session) => [session.baseUrl, session.bindAddress, session.sessionToken]), [
    [base.baseUrl, base.bindAddress, base.sessionToken],
    [base.baseUrl, base.bindAddress, base.sessionToken]
  ]);
});
