import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);

const {
  decodedBitmapBytes,
  PreviewBitmapPool,
  previewBitmapKey
} = await import("../src/features/preview/previewBitmapPool.ts");

type FakeBitmap = { width: number; height: number; close(): void; closed: boolean };

function fakeBitmap(width: number, height: number): FakeBitmap {
  const bitmap: FakeBitmap = {
    width,
    height,
    closed: false,
    close() {
      bitmap.closed = true;
    }
  };
  return bitmap;
}

// 1000 x 1000 x 4 = 4,000,000 bytes each.
const ONE_MEGAPIXEL_BYTES = 1000 * 1000 * 4;

describe("preview bitmap pool", () => {
  it("keys by epoch, media id, and tier so a library switch can never collide", () => {
    assert.equal(previewBitmapKey(3, 42, "preview_2048"), "3:42:preview_2048");
    assert.notEqual(previewBitmapKey(3, 42, "original"), previewBitmapKey(4, 42, "original"));
    assert.notEqual(
      previewBitmapKey(3, 42, "original"),
      previewBitmapKey(3, 42, "preview_2048")
    );
  });

  it("sizes an entry by decoded bytes, not by any file size", () => {
    assert.equal(decodedBitmapBytes(fakeBitmap(1000, 1000)), ONE_MEGAPIXEL_BYTES);
    assert.equal(decodedBitmapBytes(fakeBitmap(3072, 1728)), 3072 * 1728 * 4);
  });

  it("returns what it stored", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 4);
    const bitmap = fakeBitmap(1000, 1000);
    pool.set("a", bitmap);

    assert.equal(pool.get("a"), bitmap);
    assert.equal(pool.get("missing"), null);
    assert.equal(pool.size, 1);
    assert.equal(pool.sizeBytes, ONE_MEGAPIXEL_BYTES);
  });

  it("evicts the least recently used entry and closes it", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 2);
    const first = fakeBitmap(1000, 1000);
    const second = fakeBitmap(1000, 1000);
    const third = fakeBitmap(1000, 1000);
    pool.set("first", first);
    pool.set("second", second);

    pool.set("third", third);

    assert.equal(first.closed, true, "oldest bitmap must be closed, not just dropped");
    assert.equal(pool.has("first"), false);
    assert.equal(pool.has("second"), true);
    assert.equal(pool.has("third"), true);
  });

  it("treats a read as a recency touch", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 2);
    const first = fakeBitmap(1000, 1000);
    const second = fakeBitmap(1000, 1000);
    pool.set("first", first);
    pool.set("second", second);

    pool.get("first"); // first is now the most recent
    pool.set("third", fakeBitmap(1000, 1000));

    assert.equal(pool.has("first"), true, "touched entry must survive");
    assert.equal(second.closed, true, "untouched entry is the victim");
  });

  it("peek does not touch recency", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 2);
    const first = fakeBitmap(1000, 1000);
    pool.set("first", first);
    pool.set("second", fakeBitmap(1000, 1000));

    pool.peek("first");
    pool.set("third", fakeBitmap(1000, 1000));

    assert.equal(first.closed, true, "peek must not rescue the oldest entry");
  });

  it("never evicts the pinned frame that is currently on screen", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 2);
    const displayed = fakeBitmap(1000, 1000);
    pool.set("displayed", displayed);
    pool.pin("displayed");
    pool.set("neighbor-a", fakeBitmap(1000, 1000));

    pool.set("neighbor-b", fakeBitmap(1000, 1000));
    pool.set("neighbor-c", fakeBitmap(1000, 1000));

    assert.equal(displayed.closed, false, "pinned frame must never be closed");
    assert.equal(pool.has("displayed"), true);
  });

  it("admits an oversized bitmap rather than refusing to paint", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES);
    const huge = fakeBitmap(4000, 4000); // 64 MB, way past the budget
    const small = fakeBitmap(1000, 1000);
    pool.set("small", small);

    pool.set("huge", huge);

    assert.equal(pool.has("huge"), true, "the frame the user asked for must be available");
    assert.equal(small.closed, true, "everything else makes way for it");
  });

  it("closes the previous bitmap when a key is overwritten", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 4);
    const stale = fakeBitmap(1000, 1000);
    const fresh = fakeBitmap(1000, 1000);
    pool.set("a", stale);

    pool.set("a", fresh);

    assert.equal(stale.closed, true);
    assert.equal(pool.get("a"), fresh);
    assert.equal(pool.sizeBytes, ONE_MEGAPIXEL_BYTES, "bytes must not double-count");
  });

  it("closes everything on clear, including the pinned frame", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 4);
    const displayed = fakeBitmap(1000, 1000);
    const neighbor = fakeBitmap(1000, 1000);
    pool.set("displayed", displayed);
    pool.set("neighbor", neighbor);
    pool.pin("displayed");

    pool.clear();

    assert.equal(displayed.closed, true, "a library switch releases even the pinned frame");
    assert.equal(neighbor.closed, true);
    assert.equal(pool.size, 0);
    assert.equal(pool.sizeBytes, 0);
  });

  it("closes on explicit delete", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 4);
    const bitmap = fakeBitmap(1000, 1000);
    pool.set("a", bitmap);

    pool.delete("a");

    assert.equal(bitmap.closed, true);
    assert.equal(pool.sizeBytes, 0);
  });

  it("shrinking the budget evicts down to the new limit immediately", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 4);
    const oldest = fakeBitmap(1000, 1000);
    pool.set("oldest", oldest);
    pool.set("newest", fakeBitmap(1000, 1000));

    pool.configure(ONE_MEGAPIXEL_BYTES);

    assert.equal(oldest.closed, true);
    assert.equal(pool.sizeBytes, ONE_MEGAPIXEL_BYTES);
  });
});
