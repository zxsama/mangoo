import assert from "node:assert/strict";
import { register } from "node:module";
import { describe, it } from "node:test";

import type { MediaRecord } from "@megle/core-client";

register("./resolve-source-imports.mjs", import.meta.url);
(globalThis as typeof globalThis & { window: Record<string, unknown> }).window = {
  megleDesktop: { coreUrl: "http://127.0.0.1:47321/api" }
};

const {
  originalPreviewBlobCache,
  requestOriginalPreviewBlob,
  resetMediaCachesForLibrarySwitch
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
