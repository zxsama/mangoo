import assert from "node:assert/strict";
import { getEventListeners } from "node:events";
import { createServer } from "node:http";
import { test } from "node:test";
import {
  CoreLifecycleQueue,
  LifecycleCancelledError,
  LifecycleClosedError
} from "../src/core-lifecycle-queue.ts";
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

async function listenOnLoopback(server: ReturnType<typeof createServer>): Promise<string> {
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  assert.ok(address && typeof address !== "string");
  return `http://127.0.0.1:${address.port}/api`;
}

async function closeHttpServer(server: ReturnType<typeof createServer>): Promise<void> {
  server.closeAllConnections();
  await new Promise<void>((resolve, reject) => {
    server.close((error) => error ? reject(error) : resolve());
  });
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

test("an externally cancelled queued operation never starts and a retry still runs", async () => {
  const queue = new CoreLifecycleQueue();
  const blocker = deferred();
  const blockerStarted = deferred();
  const cancelled = new AbortController();
  const events: string[] = [];

  const first = queue.enqueue(async () => {
    events.push("blocker-start");
    blockerStarted.resolve();
    await blocker.promise;
  });
  await blockerStarted.promise;
  const stale = queue.enqueue(async () => {
    events.push("stale-start");
  }, { signal: cancelled.signal });
  cancelled.abort(new DOMException("focus timed out", "AbortError"));
  blocker.resolve();
  await first;
  await assert.rejects(stale, LifecycleCancelledError);

  const retry = queue.enqueue(async () => {
    events.push("retry-start");
    return "retry-ok";
  });
  assert.equal(await retry, "retry-ok");
  assert.deepEqual(events, ["blocker-start", "retry-start"]);
});

test("active external cancellation blocks late commit and keeps retry serialized", async () => {
  const queue = new CoreLifecycleQueue();
  const staleStarted = deferred();
  const staleBoundary = deferred();
  const cancelled = new AbortController();
  const events: string[] = [];
  let activeSwitches = 0;
  let maxActiveSwitches = 0;

  const stale = queue.enqueue(async (signal) => {
    activeSwitches += 1;
    maxActiveSwitches = Math.max(maxActiveSwitches, activeSwitches);
    events.push("stale-start");
    staleStarted.resolve();
    await staleBoundary.promise;
    try {
      if (signal.aborted) throw new LifecycleCancelledError("stale media focus switch");
      events.push("stale-publish");
    } finally {
      activeSwitches -= 1;
    }
  }, { signal: cancelled.signal });
  await staleStarted.promise;
  cancelled.abort(new DOMException("renderer reloaded", "AbortError"));

  const retry = queue.enqueue(async () => {
    activeSwitches += 1;
    maxActiveSwitches = Math.max(maxActiveSwitches, activeSwitches);
    events.push("retry-start");
    activeSwitches -= 1;
    return "retry-ok";
  });
  await Promise.resolve();
  assert.deepEqual(events, ["stale-start"], "retry must wait for stale lifecycle cleanup");

  staleBoundary.resolve();
  await assert.rejects(stale, LifecycleCancelledError);
  assert.equal(await retry, "retry-ok");
  assert.deepEqual(events, ["stale-start", "retry-start"]);
  assert.equal(maxActiveSwitches, 1);
});

test("deferred active cancellation leaves the lifecycle signal alive until transaction recovery", async () => {
  const queue = new CoreLifecycleQueue();
  const operation = new AbortController();
  const started = deferred();
  const recovery = deferred();
  let lifecycleAborted = false;

  const switching = queue.enqueue(async (signal) => {
    started.resolve();
    await recovery.promise;
    lifecycleAborted = signal.aborted;
    return "recovered";
  }, { signal: operation.signal, activeCancellation: "defer" });
  await started.promise;
  operation.abort(new DOMException("focus obsolete", "AbortError"));
  recovery.resolve();

  assert.equal(await switching, "recovered");
  assert.equal(lifecycleAborted, false);
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

test("health timeout aborts a connected request that never returns headers", async () => {
  const accepted = deferred();
  const sockets: Array<{ destroyed: boolean }> = [];
  const server = createServer(() => accepted.resolve());
  server.on("connection", (socket) => sockets.push(socket));
  const baseUrl = await listenOnLoopback(server);
  const controller = new AbortController();
  const baselineAbortListeners = getEventListeners(controller.signal, "abort").length;
  const safetyAbort = setTimeout(
    () => controller.abort(new Error("test safety abort")),
    750
  );
  const startedAt = performance.now();
  try {
    const rejection = assert.rejects(
      waitForCoreHealth(baseUrl, "session-token", 50, controller.signal),
      /Core did not become healthy|timed out/i
    );
    await accepted.promise;
    await rejection;
    const elapsedMs = performance.now() - startedAt;
    assert.ok(elapsedMs >= 25, `health timeout fired too early after ${elapsedMs}ms`);
    assert.ok(elapsedMs < 500, `health timeout remained pending for ${elapsedMs}ms`);
    assert.equal(getEventListeners(controller.signal, "abort").length, baselineAbortListeners);
  } finally {
    clearTimeout(safetyAbort);
    await closeHttpServer(server);
  }
  assert.ok(sockets.length > 0, "health request must establish a real server connection");
  assert.ok(sockets.every((socket) => socket.destroyed), "server sockets must close after timeout");
});

test("one-second health recheck releases the lifecycle queue after its deadline", async () => {
  const accepted = deferred();
  const server = createServer(() => accepted.resolve());
  const baseUrl = await listenOnLoopback(server);
  const queue = new CoreLifecycleQueue();
  const health = queue.enqueue((signal) =>
    waitForCoreHealth(baseUrl, "session-token", 1_000, signal)
  );
  const following = queue.enqueue(async () => "queue-released");
  const followingOutcome = following.then(
    (value) => ({ status: "fulfilled" as const, value }),
    (error: unknown) => ({ status: "rejected" as const, error })
  );
  const safetyShutdown = setTimeout(() => {
    void queue.shutdown(async () => undefined).catch(() => undefined);
  }, 3_000);
  try {
    const rejection = assert.rejects(health, /Core did not become healthy|timed out/i);
    await accepted.promise;
    await rejection;
    assert.deepEqual(await followingOutcome, {
      status: "fulfilled",
      value: "queue-released"
    });
  } finally {
    clearTimeout(safetyShutdown);
    await closeHttpServer(server);
  }
});

test("external lifecycle abort wins over the health deadline", async () => {
  const accepted = deferred();
  const server = createServer(() => accepted.resolve());
  const baseUrl = await listenOnLoopback(server);
  const controller = new AbortController();
  const baselineAbortListeners = getEventListeners(controller.signal, "abort").length;
  try {
    const rejection = assert.rejects(
      waitForCoreHealth(baseUrl, "session-token", 5_000, controller.signal),
      LifecycleCancelledError
    );
    await accepted.promise;
    controller.abort(new DOMException("Library switched", "AbortError"));
    await rejection;
    assert.equal(getEventListeners(controller.signal, "abort").length, baselineAbortListeners);
  } finally {
    await closeHttpServer(server);
  }
});

test("healthy Core response completes without retaining lifecycle listeners", async () => {
  let observedPath = "";
  let observedSession = "";
  const server = createServer((request, response) => {
    observedPath = request.url ?? "";
    observedSession = String(request.headers["x-megle-session"] ?? "");
    response.writeHead(200, { "content-type": "application/json" });
    response.end('{"status":"ok"}');
  });
  const baseUrl = await listenOnLoopback(server);
  const controller = new AbortController();
  const baselineAbortListeners = getEventListeners(controller.signal, "abort").length;
  try {
    await waitForCoreHealth(baseUrl, "session-token", 1_000, controller.signal);
    assert.equal(observedPath, "/api/health");
    assert.equal(observedSession, "session-token");
    assert.equal(getEventListeners(controller.signal, "abort").length, baselineAbortListeners);
  } finally {
    await closeHttpServer(server);
  }
});

test("health polling aborts promptly when shutdown cancels the active lifecycle", async () => {
  const server = createServer(() => {
    // Leave the response pending so the AbortSignal is the only completion.
  });
  const baseUrl = await listenOnLoopback(server);
  const controller = new AbortController();
  const health = waitForCoreHealth(baseUrl, "session-token", 5_000, controller.signal);

  setTimeout(() => controller.abort(), 10);
  try {
    await assert.rejects(health, LifecycleCancelledError);
  } finally {
    await closeHttpServer(server);
  }
});
