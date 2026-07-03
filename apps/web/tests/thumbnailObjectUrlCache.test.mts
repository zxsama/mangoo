import { strict as assert } from "node:assert";
import { beforeEach, describe, test } from "node:test";

// The cache revokes browser object URLs on eviction; stub it for Node.
(globalThis.URL as { revokeObjectURL?: (url: string) => void }).revokeObjectURL = () => {};

import {
  DEFAULT_THUMBNAIL_CACHE_LIMIT_BYTES,
  THUMBNAIL_OBJECT_URL_CACHE_MIN_ENTRIES as FLOOR,
  clearThumbnailObjectUrlCache,
  configureThumbnailCache,
  readCachedThumbnailObjectUrl,
  rememberThumbnailObjectUrl
} from "../src/core/thumbnailObjectUrlCache.ts";

const urlFor = (i: number) => `blob:thumb-${i}`;

describe("thumbnail object-URL cache retention", () => {
  beforeEach(() => {
    clearThumbnailObjectUrlCache();
    configureThumbnailCache(DEFAULT_THUMBNAIL_CACHE_LIMIT_BYTES);
  });

  test("remembers and reads back an object URL, reporting ownership", () => {
    const owned = rememberThumbnailObjectUrl("k1", "blob:1", 1000);
    assert.equal(owned, true);
    assert.equal(readCachedThumbnailObjectUrl("k1"), "blob:1");
  });

  test("returns null for an unknown key", () => {
    assert.equal(readCachedThumbnailObjectUrl("missing"), null);
  });

  test("retains the working set even when the byte budget is tiny (count floor)", () => {
    // A small/zero byte budget must NOT evict the working set: this is the
    // regression fix — scrolling out of and back into view keeps the image.
    configureThumbnailCache(1);
    const count = 250; // far over the byte budget, far under the count floor
    for (let i = 0; i < count; i += 1) {
      assert.equal(rememberThumbnailObjectUrl(`k${i}`, urlFor(i), 10_000), true);
    }
    for (let i = 0; i < count; i += 1) {
      assert.equal(readCachedThumbnailObjectUrl(`k${i}`), urlFor(i), `entry ${i} must survive`);
    }
  });

  test("evicts least-recently-used entries only beyond the count floor", () => {
    configureThumbnailCache(1);
    const total = FLOOR + 2;
    for (let i = 0; i < total; i += 1) {
      rememberThumbnailObjectUrl(`k${i}`, urlFor(i), 10_000);
    }
    // The two oldest are evicted; the floor's worth of newest entries remain.
    assert.equal(readCachedThumbnailObjectUrl("k0"), null);
    assert.equal(readCachedThumbnailObjectUrl("k1"), null);
    assert.equal(readCachedThumbnailObjectUrl(`k${total - 1}`), urlFor(total - 1));
    assert.equal(readCachedThumbnailObjectUrl(`k${FLOOR}`), urlFor(FLOOR));
  });

  test("reading an entry protects it from eviction (LRU bump)", () => {
    configureThumbnailCache(1);
    for (let i = 0; i < FLOOR; i += 1) {
      rememberThumbnailObjectUrl(`k${i}`, urlFor(i), 10_000);
    }
    // At exactly the floor nothing is evicted yet. Bump the oldest to MRU...
    assert.equal(readCachedThumbnailObjectUrl("k0"), urlFor(0));
    // ...then one more insert (floor+1, over budget) evicts the NEW oldest (k1),
    // not the freshly-read k0.
    rememberThumbnailObjectUrl("kX", "blob:X", 10_000);
    assert.equal(readCachedThumbnailObjectUrl("k0"), urlFor(0), "bumped entry survives");
    assert.equal(readCachedThumbnailObjectUrl("k1"), null, "new oldest evicted");
  });

  test("a generous byte budget keeps everything (no eviction under the limit)", () => {
    // Default 5 GiB budget: a normal working set is never evicted.
    const count = 500;
    for (let i = 0; i < count; i += 1) {
      rememberThumbnailObjectUrl(`k${i}`, urlFor(i), 20_000);
    }
    assert.equal(readCachedThumbnailObjectUrl("k0"), urlFor(0));
    assert.equal(readCachedThumbnailObjectUrl(`k${count - 1}`), urlFor(count - 1));
  });
});
