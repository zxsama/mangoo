import type { MediaRecord } from "@megle/core-client";

import { createCoreClient } from "../../core/client";
import {
  configureOriginalPreviewBuffer,
  mediaCacheEpochToken,
  registerMediaCacheResetListener
} from "../../core/mediaResources";
import {
  PreviewBitmapPool,
  PREVIEW_BITMAP_POOL_BUDGET_RATIO,
  previewBitmapKey,
  type PoolableBitmap
} from "./previewBitmapPool";

export type DecodeRequestMessage = { id: number; blob: Blob };
export type DecodeResponseMessage =
  | { id: number; ok: true; bitmap: PoolableBitmap }
  | { id: number; ok: false; error: string };

export type DecodeTransport = {
  post(message: DecodeRequestMessage, transfer: Transferable[]): void;
  subscribe(handler: (message: DecodeResponseMessage) => void): void;
};

export type PooledPreviewFrame = {
  bitmap: PoolableBitmap;
  tier: string;
  key: string;
};

export type PreviewBitmapRequestOptions = {
  /** Longest side of the preview viewport, in CSS pixels. */
  viewportLongPx?: number;
  /** Ask Core for the untiered original — the background sharpening upgrade. */
  original?: boolean;
  resourcePriority?: "preview" | "ahead";
  signal?: AbortSignal;
};

const ORIGINAL_TIER = "original";
const client = createCoreClient();

/**
 * Correlates decode replies to their requests by id. Extracted from the Worker
 * plumbing so the interesting behaviour — out-of-order replies, abort, orphaned
 * bitmaps, transport death — is unit-testable without a browser.
 */
export function createDecodeDispatcher(transport: DecodeTransport) {
  type Pending = {
    resolve: (bitmap: PoolableBitmap) => void;
    reject: (reason: Error) => void;
    aborted: boolean;
  };
  const pending = new Map<number, Pending>();
  let nextId = 0;

  transport.subscribe((message) => {
    const entry = pending.get(message.id);
    if (!entry) {
      // Nothing is waiting: the request was already settled. If a bitmap still
      // arrived, release it — an unreferenced ImageBitmap leaks until GC.
      if (message.ok) {
        message.bitmap.close();
      }
      return;
    }
    pending.delete(message.id);
    if (!message.ok) {
      entry.reject(new Error(message.error));
      return;
    }
    if (entry.aborted) {
      message.bitmap.close();
      return;
    }
    entry.resolve(message.bitmap);
  });

  return {
    decode(blob: Blob, signal?: AbortSignal): Promise<PoolableBitmap> {
      if (signal?.aborted) {
        return Promise.reject(new DOMException("aborted", "AbortError"));
      }
      const id = (nextId += 1);
      return new Promise<PoolableBitmap>((resolve, reject) => {
        const entry: Pending = { resolve, reject, aborted: false };
        pending.set(id, entry);
        signal?.addEventListener(
          "abort",
          () => {
            // Keep the entry registered so the late bitmap can be closed above;
            // only the caller's promise settles now.
            entry.aborted = true;
            reject(new DOMException("aborted", "AbortError"));
          },
          { once: true }
        );
        transport.post({ id, blob }, []);
      });
    },
    rejectAll(reason: Error): void {
      for (const entry of [...pending.values()]) {
        entry.reject(reason);
      }
      pending.clear();
    },
    pendingCount(): number {
      return pending.size;
    }
  };
}

type Dispatcher = ReturnType<typeof createDecodeDispatcher>;

/**
 * Holds the lazily-created Worker dispatcher and — the part that matters — lets
 * it be INVALIDATED.
 *
 * A dead Worker's dispatcher must never stay cached. Posting to a terminated
 * Worker gets no reply, so the pending entry never settles and the caller's
 * promise hangs forever; every later decode would hang too. Dropping the
 * dispatcher on failure means the next decode builds a fresh Worker, or falls
 * back to the main thread, instead of posting into the void.
 */
export function createDispatcherHolder(make: () => Dispatcher | null) {
  let current: Dispatcher | null = null;
  return {
    get(): Dispatcher | null {
      if (!current) {
        current = make();
      }
      return current;
    },
    invalidate(dispatcherToDrop: Dispatcher): void {
      // Identity-checked so a late failure from an already-replaced Worker
      // cannot discard its healthy successor.
      if (current === dispatcherToDrop) {
        current = null;
      }
    },
    peek(): Dispatcher | null {
      return current;
    }
  };
}

const workerHolder = createDispatcherHolder(() => {
  if (typeof Worker === "undefined") {
    return null;
  }
  try {
    // Same-origin module URL: allowed by the app CSP (which has no worker-src
    // and therefore falls back to default-src 'self'). A blob: worker would be
    // blocked outright.
    const worker = new Worker(new URL("./previewDecodeWorker.ts", import.meta.url), {
      type: "module"
    });
    let onMessage: ((message: DecodeResponseMessage) => void) | null = null;
    worker.onmessage = (event: MessageEvent<DecodeResponseMessage>) => onMessage?.(event.data);
    const created = createDecodeDispatcher({
      post: (message) => worker.postMessage(message),
      subscribe: (handler) => {
        onMessage = handler;
      }
    });
    worker.onerror = () => {
      created.rejectAll(new Error("preview decode worker failed"));
      workerHolder.invalidate(created);
      worker.terminate();
    };
    return created;
  } catch {
    // Bundler or CSP refused the worker; fall back to main-thread decoding
    // rather than losing preview entirely.
    return null;
  }
});

/**
 * Main-thread decode, used when no Worker could be created. `createBitmap` is
 * injected so the abort contract is testable in the DOM-less test harness.
 */
export async function decodeBlobOnMainThread(
  blob: Blob,
  signal: AbortSignal | undefined,
  createBitmap: (input: Blob) => Promise<PoolableBitmap>
): Promise<PoolableBitmap> {
  if (signal?.aborted) {
    throw new DOMException("aborted", "AbortError");
  }
  const bitmap = await createBitmap(blob);
  if (signal?.aborted) {
    // The Worker path closes a bitmap that lands after an abort; this path has
    // to behave identically, or an aborted navigation silently keeps a live
    // bitmap that nothing will ever pool, paint, or close.
    bitmap.close();
    throw new DOMException("aborted", "AbortError");
  }
  return bitmap;
}

async function decodeBlob(blob: Blob, signal?: AbortSignal): Promise<PoolableBitmap> {
  const worker = workerHolder.get();
  if (worker) {
    return worker.decode(blob, signal);
  }
  return decodeBlobOnMainThread(
    blob,
    signal,
    async (input) => (await createImageBitmap(input)) as unknown as PoolableBitmap
  );
}

export const previewBitmapPool = new PreviewBitmapPool(0);

/** Splits the user's preview-buffer budget; see the pool module for the ratio. */
export function configurePreviewBitmapBudget(previewBufferBytes: number): void {
  previewBitmapPool.configure(
    Math.max(0, Math.floor(previewBufferBytes * PREVIEW_BITMAP_POOL_BUDGET_RATIO))
  );
}

/**
 * Applies the user's single "预览缓冲区" number to BOTH preview caches.
 *
 * The decoded-bitmap pool takes the larger share because it is what actually
 * makes navigation instant; the compressed-byte cache is only its staging
 * buffer, and Core re-serves those bytes cheaply from the OS cache. Splitting
 * here — rather than handing the same number to both — keeps the setting honest
 * instead of silently doubling the app's memory ceiling.
 */
export function configurePreviewBuffers(previewBufferBytes: number): void {
  configurePreviewBitmapBudget(previewBufferBytes);
  configureOriginalPreviewBuffer(
    Math.max(0, Math.floor(previewBufferBytes * (1 - PREVIEW_BITMAP_POOL_BUDGET_RATIO)))
  );
}

export function pinPreviewFrame(key: string | null): void {
  previewBitmapPool.pin(key);
}

export function readPooledPreviewFrame(
  media: MediaRecord,
  tier: string
): PoolableBitmap | null {
  return previewBitmapPool.get(previewBitmapKey(mediaCacheEpochToken(), media.id, tier));
}

// Sharpest first: an already-upgraded original beats a tier, and a bigger tier
// beats a smaller one. This is the synchronous "can I paint RIGHT NOW?" probe
// the canvas runs before it awaits anything.
const TIER_PREFERENCE = [ORIGINAL_TIER, "preview_3072", "preview_2048", "preview_native"];

export function readAnyPooledPreviewFrame(media: MediaRecord): PooledPreviewFrame | null {
  const epoch = mediaCacheEpochToken();
  for (const tier of TIER_PREFERENCE) {
    const key = previewBitmapKey(epoch, media.id, tier);
    const bitmap = previewBitmapPool.get(key);
    if (bitmap) {
      return { bitmap, tier, key };
    }
  }
  return null;
}

export async function requestPreviewBitmap(
  media: MediaRecord,
  options: PreviewBitmapRequestOptions = {}
): Promise<PooledPreviewFrame> {
  const epoch = mediaCacheEpochToken();
  if (options.original) {
    const pooledOriginal = previewBitmapPool.get(
      previewBitmapKey(epoch, media.id, ORIGINAL_TIER)
    );
    if (pooledOriginal) {
      return { bitmap: pooledOriginal, tier: ORIGINAL_TIER, key: previewBitmapKey(epoch, media.id, ORIGINAL_TIER) };
    }
  }

  const resource = await client.getPreviewResource(media.id, {
    requestPriority: options.resourcePriority === "ahead" ? "resource" : "interactive",
    signal: options.signal,
    viewportLongPx: options.original ? undefined : options.viewportLongPx
  });

  const key = previewBitmapKey(epoch, media.id, resource.tier);
  const pooled = previewBitmapPool.get(key);
  if (pooled) {
    return { bitmap: pooled, tier: resource.tier, key };
  }

  const bitmap = await decodeBlob(resource.blob, options.signal);
  // A library switch during the decode invalidates this frame entirely: the
  // epoch moved, so pooling it under the old key would strand memory nothing
  // will ever read.
  if (mediaCacheEpochToken() !== epoch) {
    bitmap.close();
    throw new DOMException("aborted", "AbortError");
  }
  previewBitmapPool.set(key, bitmap);
  return { bitmap, tier: resource.tier, key };
}

export function prefetchPreviewBitmap(
  media: MediaRecord,
  options: PreviewBitmapRequestOptions = {}
): void {
  // Already decoded and pooled: skip the HTTP round trip entirely.
  //
  // `requestPreviewBitmap` cannot check the pool before fetching, because the
  // tier is part of the cache key and only the response reveals it. Without
  // this guard every navigation re-requested every neighbour's bytes even
  // though the pool already held their bitmaps — measured 350 preview requests
  // across a 31-image walk. Warm is warm; a prefetch has nothing to add.
  if (readAnyPooledPreviewFrame(media)) {
    return;
  }
  void requestPreviewBitmap(media, { ...options, resourcePriority: "ahead" }).catch(
    () => undefined
  );
}

export function releasePreviewBitmaps(): void {
  previewBitmapPool.clear();
  workerHolder.peek()?.rejectAll(new Error("library switched"));
}

// A library switch must release every decoded frame: ImageBitmaps do not free
// themselves, and the next library reuses the same file ids.
registerMediaCacheResetListener(releasePreviewBitmaps);
