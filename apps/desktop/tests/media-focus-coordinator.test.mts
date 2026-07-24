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
});
