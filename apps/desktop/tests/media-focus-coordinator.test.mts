import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  CoreLifecycleQueue,
  LifecycleCancelledError
} from "../src/core-lifecycle-queue.ts";
import { MediaFocusCoordinator } from "../src/media-focus-coordinator.ts";

const request = { libraryId: "library-a", fileId: 42, folderId: 7, rootId: 3 };

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

describe("media focus coordinator", () => {
  it("waits for Main acknowledgement instead of treating event delivery as success", async () => {
    const delivered: unknown[] = [];
    const coordinator = new MediaFocusCoordinator({ timeoutMs: 1_000 });
    const result = coordinator.request(request, (command) => delivered.push(command));

    assert.equal(delivered.length, 1);
    let settled = false;
    void result.then(() => { settled = true; });
    await Promise.resolve();
    assert.equal(settled, false);
    const command = delivered[0] as { requestId: string };
    assert.equal(coordinator.complete({ requestId: command.requestId, ok: true }), true);
    assert.equal(await result, true);
  });

  it("clears failed requests so the same media can retry", async () => {
    const delivered: Array<{ requestId: string }> = [];
    const coordinator = new MediaFocusCoordinator({ timeoutMs: 1_000 });
    const first = coordinator.request(request, (command) => delivered.push(command));
    assert.equal(coordinator.complete({ requestId: delivered[0]!.requestId, ok: false }), true);
    assert.equal(await first, false);

    const second = coordinator.request(request, (command) => delivered.push(command));
    assert.notEqual(delivered[1]!.requestId, delivered[0]!.requestId);
    assert.equal(coordinator.complete({ requestId: delivered[1]!.requestId, ok: true }), true);
    assert.equal(await second, true);
  });

  it("cancels timed-out Main work, rejects late acknowledgement, and permits retry", async () => {
    const delivered: Array<{ requestId: string }> = [];
    const cancelled: Array<{ requestId: string }> = [];
    const coordinator = new MediaFocusCoordinator({ timeoutMs: 5 });
    const first = coordinator.request(
      request,
      (command) => delivered.push(command),
      (cancellation) => cancelled.push(cancellation)
    );

    assert.equal(await first, false);
    assert.deepEqual(cancelled, [{ requestId: delivered[0]!.requestId }]);
    assert.equal(coordinator.complete({ requestId: delivered[0]!.requestId, ok: true }), false);

    const second = coordinator.request(
      request,
      (command) => delivered.push(command),
      (cancellation) => cancelled.push(cancellation)
    );
    assert.equal(coordinator.complete({ requestId: delivered[1]!.requestId, ok: true }), true);
    assert.equal(await second, true);
    assert.equal(cancelled.length, 1);
  });

  it("bounds the switching stage, cancels late work, and advances a queued retry", async () => {
    const delivered: Array<{ requestId: string; libraryId: string }> = [];
    const cancelled: Array<{ requestId: string }> = [];
    const coordinator = new MediaFocusCoordinator({ timeoutMs: 1_000, switchingTimeoutMs: 10 });
    const updateStage = Reflect.get(coordinator, "updateStage");
    assert.equal(typeof updateStage, "function", "the coordinator must accept explicit focus stages");

    const first = coordinator.request(
      { ...request, libraryId: "library-b" },
      (command) => delivered.push(command),
      (cancellation) => cancelled.push(cancellation)
    );
    const firstId = delivered[0]!.requestId;
    assert.equal(updateStage.call(coordinator, { requestId: firstId, stage: "switching" }), true);

    const second = coordinator.request(
      { ...request, libraryId: "library-c" },
      (command) => delivered.push(command),
      (cancellation) => cancelled.push(cancellation)
    );
    const firstOutcome = await Promise.race([
      first,
      new Promise<"still-pending">((resolve) => setTimeout(() => resolve("still-pending"), 30))
    ]);
    assert.equal(firstOutcome, false, "switching must have a bounded renderer watchdog");
    assert.deepEqual(cancelled, [{ requestId: firstId }]);
    assert.equal(coordinator.complete({ requestId: firstId, ok: true }), false);
    assert.equal(delivered.length, 2, "the next request must not remain blocked by a dead renderer");
    assert.equal(coordinator.complete({ requestId: delivered[1]!.requestId, ok: true }), true);
    assert.equal(await second, true);
  });

  it("settles a switching crash and permits a clean post-reload retry", async () => {
    const delivered: Array<{ requestId: string }> = [];
    const cancelled: Array<{ requestId: string }> = [];
    const coordinator = new MediaFocusCoordinator({ timeoutMs: 1_000 });
    const first = coordinator.request(
      request,
      (command) => delivered.push(command),
      (cancellation) => cancelled.push(cancellation)
    );
    const firstId = delivered[0]!.requestId;
    assert.equal(coordinator.updateStage({ requestId: firstId, stage: "switching" }), true);
    const queued = coordinator.request(request, (command) => delivered.push(command));

    coordinator.failAll();
    assert.equal(await first, false);
    assert.equal(await queued, false);
    assert.deepEqual(cancelled, [{ requestId: firstId }]);

    const retry = coordinator.request(request, (command) => delivered.push(command));
    const retryId = delivered.at(-1)!.requestId;
    assert.notEqual(retryId, firstId);
    assert.equal(coordinator.complete({ requestId: retryId, ok: true }), true);
    assert.equal(await retry, true);
  });

  it("aborts the active operation signal on watchdog expiry and renderer reload", async () => {
    const delivered: Array<{ requestId: string }> = [];
    const coordinator = new MediaFocusCoordinator({ timeoutMs: 1_000, switchingTimeoutMs: 10 });
    const first = coordinator.request(request, (command) => delivered.push(command));
    const firstId = delivered[0]!.requestId;
    assert.equal(coordinator.updateStage({ requestId: firstId, stage: "switching" }), true);
    const firstSignal = coordinator.operationSignal(firstId);
    assert.ok(firstSignal);
    assert.equal(await first, false);
    assert.equal(firstSignal.aborted, true);

    const retry = coordinator.request(request, (command) => delivered.push(command));
    const retryId = delivered[1]!.requestId;
    assert.equal(coordinator.updateStage({ requestId: retryId, stage: "switching" }), true);
    const retrySignal = coordinator.operationSignal(retryId);
    assert.ok(retrySignal);
    coordinator.failAll();
    assert.equal(await retry, false);
    assert.equal(retrySignal.aborted, true);
  });

  it("refreshes a progressing switch lease without allowing a late acknowledgement", async () => {
    const delivered: Array<{ requestId: string }> = [];
    const coordinator = new MediaFocusCoordinator({ timeoutMs: 1_000, switchingTimeoutMs: 25 });
    const result = coordinator.request(request, (command) => delivered.push(command));
    const requestId = delivered[0]!.requestId;
    assert.equal(coordinator.updateStage({ requestId, stage: "switching" }), true);

    await new Promise((resolve) => setTimeout(resolve, 15));
    assert.equal(coordinator.refreshSwitching(requestId), true);
    await new Promise((resolve) => setTimeout(resolve, 15));
    assert.equal(coordinator.refreshSwitching(requestId), true);
    await new Promise((resolve) => setTimeout(resolve, 15));
    assert.equal(coordinator.complete({ requestId, ok: true }), true);
    assert.equal(await result, true);
    assert.equal(coordinator.operationSignal(requestId), null);
  });

  it("cancels queued and running lifecycle work end to end without overlapping a retry", async () => {
    const coordinator = new MediaFocusCoordinator({ timeoutMs: 1_000, switchingTimeoutMs: 10 });
    const queue = new CoreLifecycleQueue();
    const delivered: Array<{ requestId: string }> = [];
    const blocker = deferred();
    const blockerStarted = deferred();
    const staleBoundary = deferred();
    const events: string[] = [];

    const blocking = queue.enqueue(async () => {
      blockerStarted.resolve();
      await blocker.promise;
    });
    await blockerStarted.promise;
    const queuedFocus = coordinator.request(request, (command) => delivered.push(command));
    const queuedId = delivered[0]!.requestId;
    assert.equal(coordinator.updateStage({ requestId: queuedId, stage: "switching" }), true);
    const queuedSignal = coordinator.operationSignal(queuedId);
    assert.ok(queuedSignal);
    const queuedSwitch = queue.enqueue(async () => {
      events.push("timed-out-queued-start");
    }, { signal: queuedSignal });
    assert.equal(await queuedFocus, false);
    await assert.rejects(queuedSwitch, LifecycleCancelledError);
    blocker.resolve();
    await blocking;
    assert.deepEqual(events, []);

    const runningFocus = coordinator.request(request, (command) => delivered.push(command));
    const runningId = delivered[1]!.requestId;
    assert.equal(coordinator.updateStage({ requestId: runningId, stage: "switching" }), true);
    const runningSignal = coordinator.operationSignal(runningId);
    assert.ok(runningSignal);
    const runningSwitch = queue.enqueue(async (signal) => {
      events.push("running-start");
      await staleBoundary.promise;
      if (signal.aborted) throw new LifecycleCancelledError("stale after renderer reload");
      events.push("stale-publish");
    }, { signal: runningSignal });
    await Promise.resolve();
    coordinator.failAll();
    assert.equal(await runningFocus, false);

    const retryFocus = coordinator.request(request, (command) => delivered.push(command));
    const retryId = delivered[2]!.requestId;
    assert.equal(coordinator.updateStage({ requestId: retryId, stage: "switching" }), true);
    const retrySignal = coordinator.operationSignal(retryId);
    assert.ok(retrySignal);
    const retrySwitch = queue.enqueue(async () => {
      events.push("retry-start");
    }, { signal: retrySignal });
    await Promise.resolve();
    assert.deepEqual(events, ["running-start"]);

    staleBoundary.resolve();
    await assert.rejects(runningSwitch, LifecycleCancelledError);
    await retrySwitch;
    assert.deepEqual(events, ["running-start", "retry-start"]);
    assert.equal(coordinator.complete({ requestId: runningId, ok: true }), false);
    assert.equal(coordinator.complete({ requestId: retryId, ok: true }), true);
    assert.equal(await retryFocus, true);
  });
});
