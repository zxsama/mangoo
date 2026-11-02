import type { MediaRecord } from "@mangoo/core-client";

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

/**
 * Pools a bitmap whose requester aborted before the decode landed. Returns
 * false when there is nowhere to put it (unknown key, or the library moved on),
 * in which case the caller must close it.
 */
function adoptAbandonedBitmap(key: string | null, bitmap: PoolableBitmap): boolean {
  if (!key) {
    return false;
  }
  // The key embeds the epoch it was minted under, so a key from a previous
  // library simply will not match anything the pool is asked for again — but
  // pooling it would still hold memory nothing reads, so drop it explicitly.
  if (!key.startsWith(`${mediaCacheEpochToken()}:`)) {
    return false;
  }
  previewBitmapPool.set(key, bitmap);
  return true;
}
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
    /** Where to pool a bitmap whose caller aborted before it finished. */
    abandonKey: string | null;
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
      // The caller has already given up, but the decode is DONE. Hand it to the
      // adopter (which pools it) rather than closing it — the App prefetch
      // effect aborts on every navigation, so at a held-arrow-key repeat rate
      // essentially every prefetch was aborted mid-decode and its finished
      // bitmap destroyed, making a cold walk decode every frame twice.
      // Pooling is budget-bounded, so this cannot grow without limit.
      if (!adoptAbandonedBitmap(entry.abandonKey, message.bitmap)) {
        message.bitmap.close();
      }
      return;
    }
    entry.resolve(message.bitmap);
  });

  return {
    decode(blob: Blob, signal?: AbortSignal, abandonKey?: string): Promise<PoolableBitmap> {
      if (signal?.aborted) {
        return Promise.reject(new DOMException("aborted", "AbortError"));
      }
      const id = (nextId += 1);
      return new Promise<PoolableBitmap>((resolve, reject) => {
        const entry: Pending = { resolve, reject, aborted: false, abandonKey: abandonKey ?? null };
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

async function decodeBlob(
  blob: Blob,
  signal?: AbortSignal,
  abandonKey?: string
): Promise<PoolableBitmap> {
  const worker = workerHolder.get();
  if (worker) {
    return worker.decode(blob, signal, abandonKey);
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

/**
 * Converts the user's preview-buffer setting into a budget the neighbour
 * prefetch window can be measured against IN FILE BYTES.
 *
 * The window builder counts compressed bytes; the pool it feeds counts decoded
 * bytes and owns only `PREVIEW_BITMAP_POOL_BUDGET_RATIO` of the setting. A
 * decoded frame is far larger than its file, so we divide the pool's share by a
 * conservative expansion factor to get an equivalent file-byte budget. Getting
 * this wrong in the generous direction is what made most of the window fetch,
 * decode, and evict without ever being painted.
 *
 * The factor is deliberately a fixed, pessimistic constant rather than a
 * per-image estimate: `media.width/height` are frequently still pending while
 * thumbnails generate, so a "smart" estimate would be wrong exactly when the
 * window matters most.
 */
export function previewPrefetchBudgetBytes(
  previewBufferBytes: number,
  _viewportLongPx: number
): number {
  const poolBytes = Math.max(0, previewBufferBytes * PREVIEW_BITMAP_POOL_BUDGET_RATIO);
  return Math.max(0, Math.floor(poolBytes / DECODED_TO_FILE_BYTES_RATIO));
}

/**
 * How much larger a decoded RGBA frame is than the file it came from. A 3 MB
 * JPEG decoding to ~130 MB is ~43x; 20 is a deliberately conservative floor, so
 * the window under-fills rather than thrashes the pool.
 */
const DECODED_TO_FILE_BYTES_RATIO = 20;

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

/**
 * "Is any frame for this media already decoded?" — using `peek`, so a warmth
 * probe does not reorder the LRU. `readAnyPooledPreviewFrame` deliberately does
 * touch recency, because it is the paint path.
 */
export function isPooledPreviewFrame(media: MediaRecord): boolean {
  const epoch = mediaCacheEpochToken();
  return TIER_PREFERENCE.some(
    (tier) => previewBitmapPool.peek(previewBitmapKey(epoch, media.id, tier)) !== null
  );
}

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
    // Complete the bounded network response even if this view moves on. The
    // caller signal still cancels decode/paint below, while Core and the browser
    // avoid surfacing a scenario-phase ERR_ABORTED request.
    signal: undefined,
    viewportLongPx: options.original ? undefined : options.viewportLongPx
  });

  const key = previewBitmapKey(epoch, media.id, resource.tier);
  const pooled = previewBitmapPool.get(key);
  if (pooled) {
    return { bitmap: pooled, tier: resource.tier, key };
  }

  const bitmap = await decodeBlob(resource.blob, options.signal, key);
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

/**
 * Remembers which `viewportLongPx` produced each media's pooled frame, so the
 * prefetch short-circuit can tell "already warm" from "warm at the WRONG tier".
 *
 * Without this, resizing the window (say 1280 -> 3840) left neighbours pooled
 * at `preview_2048` while the canvas started asking for `preview_3072`: every
 * navigation missed, and `readAnyPooledPreviewFrame` reported the stale entry
 * as warm so prefetch refused to fix it for the rest of the session.
 */
const warmedViewportByMediaId = new Map<number, number>();

export function prefetchPreviewBitmap(
  media: MediaRecord,
  options: PreviewBitmapRequestOptions = {}
): void {
  // Already decoded and pooled AT THIS VIEWPORT: skip the HTTP round trip.
  //
  // `requestPreviewBitmap` cannot check the pool before fetching, because the
  // tier is part of the cache key and only the response reveals it. Without
  // this guard every navigation re-requested every neighbour's bytes even
  // though the pool already held their bitmaps — measured 350 preview requests
  // across a 31-image walk, cut to 54.
  //
  // The viewport is part of the condition so a window resize re-warms at the
  // new tier instead of being suppressed forever by a stale entry.
  const wantedViewport = options.viewportLongPx ?? 0;
  if (warmedViewportByMediaId.get(media.id) === wantedViewport && isPooledPreviewFrame(media)) {
    return;
  }
  warmedViewportByMediaId.set(media.id, wantedViewport);
  void requestPreviewBitmap(media, { ...options, resourcePriority: "ahead" }).catch(
    () => undefined
  );
}

export function releasePreviewBitmaps(): void {
  previewBitmapPool.clear();
  warmedViewportByMediaId.clear();
  workerHolder.peek()?.rejectAll(new Error("library switched"));
}

// A library switch must release every decoded frame: ImageBitmaps do not free
// themselves, and the next library reuses the same file ids.
registerMediaCacheResetListener(releasePreviewBitmaps);
