import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

register("./resolve-source-imports.mjs", import.meta.url);

const {
  decodedBitmapBytes,
  PREVIEW_BITMAP_POOL_BUDGET_RATIO,
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

  // C1 regression: a pinned entry plus the entry just admitted can together
  // exceed the budget on their own. `pin()` exists precisely so the sweep
  // does not "resolve" that conflict by closing the bitmap the caller just
  // handed in — the one prior 12-test suite never forced this conflict
  // because it always paired a 2-frame budget with 1-frame bitmaps, so the
  // older unpinned neighbour alone always covered the whole sweep.
  it("keeps both the pinned frame and a newly admitted frame when together they exceed budget", () => {
    // Budget holds exactly one frame plus a sliver — pinned + incoming (2
    // frames) forces the sweep to make a choice; a budget of 2 whole frames
    // would never trigger eviction at all.
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES + 1);
    const pinned = fakeBitmap(1000, 1000);
    pool.set("pinned", pinned);
    pool.pin("pinned");

    const incoming = fakeBitmap(1000, 1000);
    pool.set("incoming", incoming);

    assert.equal(pinned.closed, false, "pin() must survive a conflicting admission");
    assert.equal(incoming.closed, false, "the bitmap the caller just handed in must not be closed");
    assert.equal(pool.has("pinned"), true);
    assert.equal(pool.has("incoming"), true, "caller must be able to draw the frame it just set");
  });

  // M4: delete() must clear a matching pinnedKey the same way clear() does,
  // otherwise a later set() under the same key silently inherits eviction
  // immunity without anyone calling pin() again.
  it("clears a matching pin on delete so a reused key does not inherit stale immunity", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 2);
    const displayed = fakeBitmap(1000, 1000);
    pool.set("displayed", displayed);
    pool.pin("displayed");

    pool.delete("displayed");
    pool.set("displayed", fakeBitmap(1000, 1000));
    pool.set("neighbor-a", fakeBitmap(1000, 1000));
    const evictionVictim = fakeBitmap(1000, 1000);
    pool.set("neighbor-b", evictionVictim);

    // "displayed" was never re-pinned, so once it ages out of recency it must
    // be evictable like any other entry.
    assert.equal(pool.has("displayed"), false, "reused key must not keep the old pin");
  });

  // M5: non-finite dimensions or budget must not poison the pool with NaN,
  // which would make `currentBytes <= budgetBytes` false forever.
  it("treats non-finite bitmap dimensions as zero bytes instead of poisoning the pool with NaN", () => {
    const nanBitmap = fakeBitmap(Number.NaN, 1000);
    assert.equal(decodedBitmapBytes(nanBitmap), 0);

    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 2);
    pool.set("nan-entry", nanBitmap);
    assert.equal(pool.sizeBytes, 0, "a NaN-dimension bitmap must contribute zero bytes");

    const normal = fakeBitmap(1000, 1000);
    pool.set("normal", normal);
    assert.equal(
      pool.sizeBytes,
      ONE_MEGAPIXEL_BYTES,
      "currentBytes must not have become NaN and stay poisoned"
    );
  });

  it("treats a non-finite budget as zero instead of poisoning the pool with NaN", () => {
    const pool = new PreviewBitmapPool(Number.NaN);
    assert.equal(pool.sizeBytes, 0);

    const bitmap = fakeBitmap(1000, 1000);
    pool.set("a", bitmap);
    // Budget sanitizes to 0, so the just-admitted entry is still protected
    // (M6) during set()'s own sweep.
    assert.equal(pool.has("a"), true);

    // configure() passes no protected key, so a NaN-sanitized-to-0 budget
    // evicts everything unpinned — proving NaN was sanitized rather than
    // poisoning currentBytes/budgetBytes into a permanently-stuck NaN
    // comparison (which would have left "a" stuck in the pool instead).
    pool.configure(Number.NaN);
    assert.equal(pool.has("a"), false, "configure() must sanitize NaN, not silently keep the old budget");
    assert.equal(pool.sizeBytes, 0);
  });

  // M6: budget 0 is an intentional, documented behaviour, not an accident.
  it("with budget 0, set() keeps only the just-admitted frame and configure(0) evicts to the pinned frame", () => {
    const pool = new PreviewBitmapPool(0);
    const first = fakeBitmap(1000, 1000);
    pool.set("first", first);
    assert.equal(pool.has("first"), true, "the on-screen frame must exist even at budget 0");
    assert.equal(pool.sizeBytes, ONE_MEGAPIXEL_BYTES);

    const second = fakeBitmap(1000, 1000);
    pool.set("second", second);
    assert.equal(first.closed, true, "budget 0 means only the newest admission survives");
    assert.equal(pool.has("second"), true);

    const pinned = fakeBitmap(1000, 1000);
    pool.set("pinned", pinned);
    pool.pin("pinned");
    pool.configure(0);
    assert.equal(pool.has("pinned"), true, "configure(0) evicts down to the pinned frame, not past it");
    assert.equal(pinned.closed, false);
  });

  // M7: set(key, bitmapAlreadyStoredUnderThatKey) must be a no-op, since the
  // naive `delete(key)` then store would close the bitmap the caller still
  // holds and then store the now-closed object.
  it("is a no-op when set() is called with the bitmap already stored under that key", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 4);
    const bitmap = fakeBitmap(1000, 1000);
    pool.set("a", bitmap);

    pool.set("a", bitmap);

    assert.equal(bitmap.closed, false, "re-setting the same object must not close it");
    assert.equal(pool.get("a"), bitmap);
    assert.equal(pool.sizeBytes, ONE_MEGAPIXEL_BYTES, "bytes must not double-count");
  });

  // M8: keys(), PREVIEW_BITMAP_POOL_BUDGET_RATIO, and pin(null) had no
  // coverage at all.
  it("exposes recency order via keys()", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 4);
    pool.set("a", fakeBitmap(1000, 1000));
    pool.set("b", fakeBitmap(1000, 1000));
    pool.set("c", fakeBitmap(1000, 1000));

    assert.deepEqual(pool.keys(), ["a", "b", "c"]);

    pool.get("a"); // touch moves "a" to the end

    assert.deepEqual(pool.keys(), ["b", "c", "a"]);
  });

  it("splits the preview buffer 75/25 in favor of decoded frames", () => {
    assert.equal(PREVIEW_BITMAP_POOL_BUDGET_RATIO, 0.75);
  });

  it("un-pins with pin(null), making the previously pinned frame evictable again", () => {
    const pool = new PreviewBitmapPool(ONE_MEGAPIXEL_BYTES * 2);
    const displayed = fakeBitmap(1000, 1000);
    pool.set("displayed", displayed);
    pool.pin("displayed");

    pool.pin(null);
    pool.set("neighbor-a", fakeBitmap(1000, 1000));
    pool.set("neighbor-b", fakeBitmap(1000, 1000));

    assert.equal(displayed.closed, true, "pin(null) must remove eviction immunity");
    assert.equal(pool.has("displayed"), false);
  });
});
