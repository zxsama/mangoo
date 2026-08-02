import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

import type { MediaRecord } from "@megle/core-client";

register("./resolve-source-imports.mjs", import.meta.url);
(globalThis as typeof globalThis & { window: Record<string, unknown> }).window = {
  megleDesktop: { coreUrl: "http://127.0.0.1:47321/api" }
};

const {
  adoptThumbnailObjectUrl,
  captureOriginalFallbackObjectUrlToken,
  createOriginalFallbackObjectUrl,
  originalPreviewBlobCache,
  readCachedOriginalFallbackObjectUrl,
  readCachedThumbnailObjectUrl,
  registerMediaCacheResetListener,
  rememberThumbnailObjectUrl,
  requestOriginalPreviewBlob,
  resetMediaCachesForLibrarySwitch,
  thumbnailObjectUrlCacheKey
} = await import("../src/core/mediaResources.ts");

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

function media(id: number): MediaRecord {
  return {
    id,
    rootId: 1,
    folderId: 2,
    name: "same-id.jpg",
    ext: "jpg",
    size: 8,
    mtime: 100,
    kind: "image",
    width: 100,
    height: 100
  };
}

describe("media resource library isolation", () => {
  it("consumes a late stale thumbnail URL exactly once without publishing it", () => {
    const originalRevokeObjectUrl = URL.revokeObjectURL;
    const revoked: string[] = [];
    URL.revokeObjectURL = ((objectUrl: string) => {
      revoked.push(objectUrl);
    }) as typeof URL.revokeObjectURL;

    try {
      resetMediaCachesForLibrarySwitch();
      const oldKey = thumbnailObjectUrlCacheKey(7, "7:100:8", null);

      resetMediaCachesForLibrarySwitch();
      const displayedLateUrl = adoptThumbnailObjectUrl(oldKey, "blob:late-old", 10);
      assert.equal(displayedLateUrl, null, "a stale candidate must never become the image src");
      assert.deepEqual(revoked, ["blob:late-old"]);

      const newKey = thumbnailObjectUrlCacheKey(7, "7:100:8", null);
      const displayedNewUrl = adoptThumbnailObjectUrl(newKey, "blob:new", 10);
      assert.equal(displayedNewUrl, "blob:new");
      assert.equal(readCachedThumbnailObjectUrl(newKey), "blob:new");
      assert.deepEqual(revoked, ["blob:late-old"]);

      resetMediaCachesForLibrarySwitch();
      assert.deepEqual(
        revoked,
        ["blob:late-old", "blob:new"],
        "the consumed stale candidate and cached current URL must each be revoked once"
      );
    } finally {
      resetMediaCachesForLibrarySwitch();
      URL.revokeObjectURL = originalRevokeObjectUrl;
    }
  });

  it("revokes fallback object URLs and rejects a late same-signature old-library write", () => {
    const originalCreateObjectUrl = URL.createObjectURL;
    const originalRevokeObjectUrl = URL.revokeObjectURL;
    const revoked: string[] = [];
    let nextObjectUrl = 1;
    URL.createObjectURL = (() => `blob:media-${nextObjectUrl++}`) as typeof URL.createObjectURL;
    URL.revokeObjectURL = ((objectUrl: string) => {
      revoked.push(objectUrl);
    }) as typeof URL.revokeObjectURL;

    try {
      resetMediaCachesForLibrarySwitch();
      const oldToken = captureOriginalFallbackObjectUrlToken(media(7));
      const oldThumbnailKey = thumbnailObjectUrlCacheKey(7, "7:100:8", null);
      const oldObjectUrl = createOriginalFallbackObjectUrl(oldToken, new Blob(["old"]));
      assert.equal(oldObjectUrl, "blob:media-1");
      assert.equal(readCachedOriginalFallbackObjectUrl(oldToken), oldObjectUrl);

      resetMediaCachesForLibrarySwitch();
      assert.deepEqual(revoked, ["blob:media-1"], "switch teardown must synchronously revoke old URLs");

      const newToken = captureOriginalFallbackObjectUrlToken(media(7));
      assert.equal(readCachedOriginalFallbackObjectUrl(oldToken), null);
      assert.equal(readCachedOriginalFallbackObjectUrl(newToken), null);
      const newObjectUrl = createOriginalFallbackObjectUrl(newToken, new Blob(["new"]));
      assert.equal(newObjectUrl, "blob:media-2");

      const lateOldObjectUrl = createOriginalFallbackObjectUrl(oldToken, new Blob(["late-old"]));
      assert.equal(lateOldObjectUrl, null);
      assert.deepEqual(revoked, ["blob:media-1", "blob:media-3"]);
      assert.equal(readCachedOriginalFallbackObjectUrl(newToken), newObjectUrl);
      assert.equal(rememberThumbnailObjectUrl(oldThumbnailKey, "blob:late-thumbnail", 10), false);
      assert.equal(readCachedThumbnailObjectUrl(oldThumbnailKey), null);
    } finally {
      resetMediaCachesForLibrarySwitch();
      URL.createObjectURL = originalCreateObjectUrl;
      URL.revokeObjectURL = originalRevokeObjectUrl;
    }
  });

  it("aborts and rejects an old-library original that finishes after the new cache is populated", async () => {
    const originalFetch = globalThis.fetch;
    const oldResponse = deferred<Response>();
    const newResponse = deferred<Response>();
    const requestSignals: AbortSignal[] = [];
    let fetchCount = 0;
    globalThis.fetch = ((_input: string | URL | Request, init?: RequestInit) => {
      fetchCount += 1;
      assert.ok(init?.signal);
      requestSignals.push(init.signal);
      return fetchCount === 1 ? oldResponse.promise : newResponse.promise;
    }) as typeof fetch;

    try {
      resetMediaCachesForLibrarySwitch();
      const oldRequest = requestOriginalPreviewBlob(media(7));
      const oldOutcome = oldRequest.then(
        () => ({ error: null }),
        (error: unknown) => ({ error })
      );
      await new Promise((resolve) => setImmediate(resolve));
      assert.equal(fetchCount, 1);

      resetMediaCachesForLibrarySwitch();
      assert.equal(requestSignals[0]?.aborted, true, "switch teardown must abort the old request");
      const newRequest = requestOriginalPreviewBlob(media(7));
      await new Promise((resolve) => setImmediate(resolve));
      assert.equal(fetchCount, 2);

      newResponse.resolve(new Response("new-byte", { status: 200 }));
      assert.equal(await (await newRequest).text(), "new-byte");

      oldResponse.resolve(new Response("old-byte", { status: 200 }));
      const oldResult = await oldOutcome;
      assert.equal((oldResult.error as Error | null)?.name, "AbortError");
      await Promise.resolve();

      const cached = await requestOriginalPreviewBlob(media(7));
      assert.equal(await cached.text(), "new-byte");
      assert.equal(fetchCount, 2, "the late old request must not evict the new cache entry");
      assert.equal(await originalPreviewBlobCache.get(7)?.blob.text(), "new-byte");
    } finally {
      resetMediaCachesForLibrarySwitch();
      globalThis.fetch = originalFetch;
    }
  });
});

describe("media cache reset listeners", () => {
  it("notifies registered listeners on a library switch", () => {
    let calls = 0;
    const unregister = registerMediaCacheResetListener(() => {
      calls += 1;
    });

    resetMediaCachesForLibrarySwitch();
    assert.equal(calls, 1);

    unregister();
    resetMediaCachesForLibrarySwitch();
    assert.equal(calls, 1, "an unregistered listener must not fire again");
  });

  it("keeps clearing caches even if a listener throws", () => {
    const unregister = registerMediaCacheResetListener(() => {
      throw new Error("listener exploded");
    });
    let secondRan = false;
    const unregisterSecond = registerMediaCacheResetListener(() => {
      secondRan = true;
    });

    resetMediaCachesForLibrarySwitch();

    assert.equal(secondRan, true, "one bad listener must not strand the rest");
    unregister();
    unregisterSecond();
  });
});
