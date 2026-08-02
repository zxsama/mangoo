import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);
(globalThis as typeof globalThis & { window: Record<string, unknown> }).window = {
  megleDesktop: { coreUrl: "http://127.0.0.1:47321/api" }
};

const { createDecodeDispatcher } = await import(
  "../src/features/preview/previewBitmapSource.ts"
);

type FakeBitmap = { width: number; height: number; close(): void; closed: boolean };

function fakeBitmap(): FakeBitmap {
  const bitmap: FakeBitmap = {
    width: 10,
    height: 10,
    closed: false,
    close() {
      bitmap.closed = true;
    }
  };
  return bitmap;
}

function fakeTransport() {
  const sent: { id: number }[] = [];
  let handler: ((message: unknown) => void) | null = null;
  return {
    sent,
    transport: {
      post(message: { id: number }) {
        sent.push(message);
      },
      subscribe(next: (message: unknown) => void) {
        handler = next;
      }
    },
    reply(message: unknown) {
      handler?.(message);
    }
  };
}

describe("preview decode dispatcher", () => {
  it("resolves each request with the bitmap carrying its id", async () => {
    const { transport, sent, reply } = fakeTransport();
    const dispatcher = createDecodeDispatcher(transport as never);
    const first = dispatcher.decode(new Blob(["a"]));
    const second = dispatcher.decode(new Blob(["b"]));

    assert.equal(sent.length, 2);
    const firstBitmap = fakeBitmap();
    const secondBitmap = fakeBitmap();
    // Reply out of order: correlation must be by id, not by arrival.
    reply({ id: sent[1].id, ok: true, bitmap: secondBitmap });
    reply({ id: sent[0].id, ok: true, bitmap: firstBitmap });

    assert.equal(await first, firstBitmap);
    assert.equal(await second, secondBitmap);
    assert.equal(dispatcher.pendingCount(), 0);
  });

  it("rejects with the worker's error text", async () => {
    const { transport, sent, reply } = fakeTransport();
    const dispatcher = createDecodeDispatcher(transport as never);
    const pending = dispatcher.decode(new Blob(["a"]));

    reply({ id: sent[0].id, ok: false, error: "unsupported image" });

    await assert.rejects(pending, /unsupported image/);
    assert.equal(dispatcher.pendingCount(), 0);
  });

  it("closes a bitmap that arrives after its request was aborted", async () => {
    const { transport, sent, reply } = fakeTransport();
    const dispatcher = createDecodeDispatcher(transport as never);
    const controller = new AbortController();
    const pending = dispatcher.decode(new Blob(["a"]), controller.signal);

    controller.abort();
    await assert.rejects(pending, /abort/i);

    const late = fakeBitmap();
    reply({ id: sent[0].id, ok: true, bitmap: late });
    assert.equal(late.closed, true, "an orphaned bitmap must not leak GPU memory");
  });

  it("rejects immediately when the signal is already aborted", async () => {
    const { transport, sent } = fakeTransport();
    const dispatcher = createDecodeDispatcher(transport as never);
    const controller = new AbortController();
    controller.abort();

    await assert.rejects(dispatcher.decode(new Blob(["a"]), controller.signal), /abort/i);
    assert.equal(sent.length, 0, "an aborted request must not reach the worker");
  });

  it("rejectAll fails every in-flight decode, for worker death or library switch", async () => {
    const { transport } = fakeTransport();
    const dispatcher = createDecodeDispatcher(transport as never);
    const pending = dispatcher.decode(new Blob(["a"]));

    dispatcher.rejectAll(new Error("decode worker terminated"));

    await assert.rejects(pending, /terminated/);
    assert.equal(dispatcher.pendingCount(), 0);
  });
});
