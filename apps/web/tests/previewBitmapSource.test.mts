import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);
(globalThis as typeof globalThis & { window: Record<string, unknown> }).window = {
  mangooDesktop: { coreUrl: "http://127.0.0.1:47321/api" }
};

const {
  configurePreviewBitmapBudget,
  createDecodeDispatcher,
  createDispatcherHolder,
  decodeBlobOnMainThread,
  previewBitmapPool,
  previewPrefetchBudgetBytes
} = await import(
  "../src/features/preview/previewBitmapSource.ts"
);
const { resetMediaCachesForLibrarySwitch } = await import(
  "../src/core/mediaResources.ts"
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

describe("decode dispatcher holder", () => {
  it("builds the dispatcher lazily, exactly once, and reuses it", () => {
    let built = 0;
    const holder = createDispatcherHolder(() => {
      built += 1;
      return { rejectAll() {}, pendingCount: () => 0 } as never;
    });

    assert.equal(built, 0, "construction must not happen at module load");
    const first = holder.get();
    const second = holder.get();

    assert.equal(built, 1);
    assert.equal(first, second);
  });

  it("rebuilds after invalidation, so a dead worker cannot hang every later decode", () => {
    // Regression: onerror used to rejectAll but leave the dispatcher cached.
    // The next decode then posted into a TERMINATED worker, got no reply, and
    // its promise never settled — a permanent hang, not a failure.
    let built = 0;
    const holder = createDispatcherHolder(() => {
      built += 1;
      return { id: built, rejectAll() {}, pendingCount: () => 0 } as never;
    });

    const dead = holder.get();
    holder.invalidate(dead as never);
    const fresh = holder.get();

    assert.equal(built, 2, "a new dispatcher must be constructed after invalidation");
    assert.notEqual(fresh, dead);
  });

  it("ignores invalidation from an already-replaced dispatcher", () => {
    // A late failure from the old worker must not discard its healthy successor.
    let built = 0;
    const holder = createDispatcherHolder(() => {
      built += 1;
      return { id: built, rejectAll() {}, pendingCount: () => 0 } as never;
    });

    const dead = holder.get();
    holder.invalidate(dead as never);
    const fresh = holder.get();
    holder.invalidate(dead as never);

    assert.equal(holder.peek(), fresh, "the live dispatcher must survive a stale invalidation");
    assert.equal(built, 2);
  });

  it("reports null without constructing when the factory declines", () => {
    const holder = createDispatcherHolder(() => null);

    assert.equal(holder.get(), null);
    assert.equal(holder.peek(), null);
  });
});

describe("main-thread decode fallback", () => {
  function fakeBitmap() {
    const bitmap = {
      width: 4,
      height: 4,
      closed: false,
      close() {
        bitmap.closed = true;
      }
    };
    return bitmap;
  }

  it("returns the bitmap when nothing aborted", async () => {
    const bitmap = fakeBitmap();

    const decoded = await decodeBlobOnMainThread(new Blob(["a"]), undefined, async () => bitmap);

    assert.equal(decoded, bitmap);
    assert.equal(bitmap.closed, false);
  });

  it("rejects without decoding when the signal is already aborted", async () => {
    const controller = new AbortController();
    controller.abort();
    let called = false;

    await assert.rejects(
      decodeBlobOnMainThread(new Blob(["a"]), controller.signal, async () => {
        called = true;
        return fakeBitmap();
      }),
      /abort/i
    );
    assert.equal(called, false, "an already-aborted request must not decode at all");
  });

  it("closes a bitmap whose decode finished after the caller aborted", async () => {
    // Parity with the Worker path, which closes a late-arriving bitmap. Without
    // this the fallback silently keeps a live bitmap nothing will pool or close.
    const controller = new AbortController();
    const bitmap = fakeBitmap();

    await assert.rejects(
      decodeBlobOnMainThread(new Blob(["a"]), controller.signal, async () => {
        controller.abort();
        return bitmap;
      }),
      /abort/i
    );
    assert.equal(bitmap.closed, true, "a bitmap decoded into an aborted request must be closed");
  });
});

describe("prefetch budget sizing", () => {
  it("sizes the window against the DECODED-frame share, not the raw setting", () => {
    // The window builder counts compressed file bytes; the pool it feeds counts
    // decoded bytes and owns only 75% of the setting. Handing it the full
    // setting picked a window an order of magnitude larger than the pool could
    // hold, so most of it fetched, decoded, and was evicted unpainted.
    const setting = 1200 * 1024 * 1024;

    const budget = previewPrefetchBudgetBytes(setting, 2560);

    assert.ok(budget > 0, "a real setting must yield a usable window");
    assert.ok(
      budget < setting * 0.75,
      `budget ${budget} must be well under the pool's own share`
    );
    // 0.75 / 20 = 3.75% of the setting.
    assert.equal(budget, Math.floor(setting * 0.75 / 20));
  });

  it("never returns a negative budget", () => {
    assert.equal(previewPrefetchBudgetBytes(0, 2560), 0);
    assert.equal(previewPrefetchBudgetBytes(-5, 2560), 0);
  });
});

describe("abandoned decode adoption", () => {
  it("pools a bitmap whose request was aborted after the decode finished", async () => {
    // The App prefetch effect aborts on EVERY navigation, so at a held-arrow
    // repeat rate essentially every prefetch is aborted mid-decode. Closing
    // those finished bitmaps made a cold walk decode every frame twice.
    const { transport, sent, reply } = fakeTransport();
    const dispatcher = createDecodeDispatcher(transport as never);
    const controller = new AbortController();
    const key = "0:1:preview_2048";
    const pending = dispatcher.decode(new Blob(["a"]), controller.signal, key);

    controller.abort();
    await assert.rejects(pending, /abort/i);

    const late = fakeBitmap();
    reply({ id: sent[0].id, ok: true, bitmap: late });

    assert.equal(late.closed, false, "a finished bitmap must be pooled, not destroyed");
    assert.equal(dispatcher.pendingCount(), 0);
  });

  it("still closes an abandoned bitmap when there is nowhere to pool it", async () => {
    const { transport, sent, reply } = fakeTransport();
    const dispatcher = createDecodeDispatcher(transport as never);
    const controller = new AbortController();
    // No key supplied: nothing can adopt it.
    const pending = dispatcher.decode(new Blob(["a"]), controller.signal);

    controller.abort();
    await assert.rejects(pending, /abort/i);

    const late = fakeBitmap();
    reply({ id: sent[0].id, ok: true, bitmap: late });

    assert.equal(late.closed, true, "an unadoptable bitmap must not leak");
  });
});

// Last on purpose: the real reset advances the process-wide media-cache epoch,
// which is exactly what a library switch must do and must not be undone for tests.
describe("library-switch bitmap release wiring", () => {
  it("closes pooled ImageBitmaps through the real media-cache reset entrypoint", () => {
    previewBitmapPool.clear();
    configurePreviewBitmapBudget(1024 * 1024);
    const bitmap = fakeBitmap();
    previewBitmapPool.set("old-library-frame", bitmap as never);

    resetMediaCachesForLibrarySwitch();

    assert.equal(bitmap.closed, true);
    assert.equal(previewBitmapPool.size, 0);
    assert.equal(previewBitmapPool.sizeBytes, 0);
  });
});
