import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import test from "node:test";
import { ensureMeglePreview } from "./prod-preview.mjs";

const previewUrl = "http://127.0.0.1:4173";

class FakeChild extends EventEmitter {
  exitCode = null;
  signalCode = null;
  killed = false;

  exit(code, signal = null) {
    this.exitCode = code;
    this.signalCode = signal;
    this.emit("exit", code, signal);
  }

  kill() {
    this.killed = true;
  }
}

function htmlResponse(title) {
  return {
    ok: true,
    status: 200,
    text: async () => `<html><head><title>${title}</title></head></html>`
  };
}

test("reuses an existing Megle preview without spawning a competing process", async () => {
  let spawnCount = 0;

  const result = await ensureMeglePreview({
    url: previewUrl,
    fetchImpl: async () => htmlResponse("Megle"),
    spawnPreview: () => {
      spawnCount += 1;
      return new FakeChild();
    },
    logger: () => {}
  });

  assert.deepEqual(result, { reused: true, child: null });
  assert.equal(spawnCount, 0);
});

test("waits for a newly spawned preview to serve the Megle page", async () => {
  const child = new FakeChild();
  let probeCount = 0;

  const result = await ensureMeglePreview({
    url: previewUrl,
    fetchImpl: async () => {
      probeCount += 1;
      if (probeCount < 3) throw new Error("connection refused");
      return htmlResponse("Megle");
    },
    spawnPreview: () => child,
    logger: () => {},
    pollIntervalMs: 1,
    timeoutMs: 100
  });

  assert.equal(result.reused, false);
  assert.equal(result.child, child);
  assert.equal(child.killed, false);
});

test("fails promptly when the preview child exits against an unrelated port owner", async () => {
  const child = new FakeChild();
  let resolveFetch;
  const stalledFetch = new Promise((resolve) => {
    resolveFetch = resolve;
  });

  const startedAt = Date.now();
  const launch = ensureMeglePreview({
    url: previewUrl,
    fetchImpl: (() => {
      let probeCount = 0;
      return async () => {
        probeCount += 1;
        return probeCount === 1 ? htmlResponse("Unrelated service") : stalledFetch;
      };
    })(),
    spawnPreview: () => {
      setTimeout(() => child.exit(1), 5);
      return child;
    },
    logger: () => {},
    pollIntervalMs: 1,
    timeoutMs: 5_000
  });

  await assert.rejects(
    launch,
    (error) => {
      assert.match(error.message, /preview process exited with code 1/i);
      assert.match(error.message, /127\.0\.0\.1:4173/);
      assert.match(error.message, /port may already be in use/i);
      return true;
    }
  );

  resolveFetch(htmlResponse("Unrelated service"));
  assert.ok(Date.now() - startedAt < 500, "preview failure should not wait for the readiness timeout");
  assert.equal(child.killed, false);
});
