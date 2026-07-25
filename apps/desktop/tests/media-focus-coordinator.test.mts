import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { MediaFocusCoordinator } from "../src/media-focus-coordinator.ts";

const request = { libraryId: "library-a", fileId: 42, folderId: 7, rootId: 3 };

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
});
