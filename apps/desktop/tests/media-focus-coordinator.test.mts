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

  it("suspends timeout during an irreversible switch and serializes the next request", async () => {
    const delivered: Array<{ requestId: string; libraryId: string }> = [];
    const cancelled: Array<{ requestId: string }> = [];
    const coordinator = new MediaFocusCoordinator({ timeoutMs: 5 });
    const updateStage = Reflect.get(coordinator, "updateStage");
    assert.equal(typeof updateStage, "function", "the coordinator must accept explicit focus stages");

    let firstSettled = false;
    const first = coordinator.request(
      { ...request, libraryId: "library-b" },
      (command) => delivered.push(command),
      (cancellation) => cancelled.push(cancellation)
    );
    void first.then(() => { firstSettled = true; });
    const firstId = delivered[0]!.requestId;
    assert.equal(updateStage.call(coordinator, { requestId: firstId, stage: "switching" }), true);

    const second = coordinator.request(
      { ...request, libraryId: "library-c" },
      (command) => delivered.push(command),
      (cancellation) => cancelled.push(cancellation)
    );
    await new Promise((resolve) => setTimeout(resolve, 15));
    assert.equal(firstSettled, false, "switching must not become a retryable timeout");
    assert.equal(delivered.length, 1, "a retry must not overlap an irreversible switch");
    assert.deepEqual(cancelled, []);

    assert.equal(updateStage.call(coordinator, { requestId: firstId, stage: "locating" }), true);
    assert.equal(coordinator.complete({ requestId: firstId, ok: true }), true);
    assert.equal(await first, true);
    assert.equal(delivered.length, 2, "the queued request starts only after switch completion");
    assert.equal(coordinator.complete({ requestId: delivered[1]!.requestId, ok: true }), true);
    assert.equal(await second, true);
  });
});
