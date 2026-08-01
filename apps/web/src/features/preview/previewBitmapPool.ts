// A budget-bounded LRU of DECODED preview frames.
//
// This is the accelerator behind instant prev/next: a pooled frame paints with
// a synchronous `drawImage`, while an unpooled one costs a full-resolution
// decode (measured: 337ms for a 16384px image, 323ms for 10032px).
//
// Pure on purpose — no DOM, no Core client — so `node --test` can drive the
// whole eviction/lifecycle contract with plain objects. `ImageBitmap`
// structurally satisfies `PoolableBitmap`.

/** Anything with pixels and an explicit release. A real `ImageBitmap` fits. */
export type PoolableBitmap = {
  readonly width: number;
  readonly height: number;
  close(): void;
};

// Bitmaps are RGBA8 in GPU/CPU memory regardless of how well the source file
// compressed. This is the ONLY correct unit for this budget: the sibling
// `originalPreviewBlobCache` counts compressed bytes, and a 3 MB JPEG is 130 MB
// decoded.
const BYTES_PER_DECODED_PIXEL = 4;

// Share of the user's "预览缓冲区" budget that goes to decoded frames rather
// than to the compressed-byte cache. Decoded frames are what actually make
// navigation instant; the byte cache is only their staging buffer, and Core
// re-serves those bytes cheaply from the OS cache.
export const PREVIEW_BITMAP_POOL_BUDGET_RATIO = 0.75;

/**
 * Cache key. The epoch comes from `mediaCacheEpochToken()` so a library switch
 * can never serve the previous library's frame for a reused file id — the same
 * "串图" guard the thumbnail caches already use. The tier is part of the key
 * because a media item legitimately has both a `preview_2048` frame and a
 * sharper `original` one.
 */
export function previewBitmapKey(epoch: number, mediaId: number, tier: string): string {
  return `${epoch}:${mediaId}:${tier}`;
}

export function decodedBitmapBytes(bitmap: PoolableBitmap): number {
  return Math.max(0, bitmap.width) * Math.max(0, bitmap.height) * BYTES_PER_DECODED_PIXEL;
}

type PoolEntry = {
  bitmap: PoolableBitmap;
  bytes: number;
};

export class PreviewBitmapPool {
  // Insertion order IS recency order: JS Map preserves it, and re-inserting
  // moves a key to the end. Same technique the thumbnail object-URL cache uses.
  private readonly entries = new Map<string, PoolEntry>();
  private budgetBytes: number;
  private currentBytes = 0;
  private pinnedKey: string | null = null;

  constructor(budgetBytes: number) {
    this.budgetBytes = Math.max(0, Math.floor(budgetBytes));
  }

  get size(): number {
    return this.entries.size;
  }

  get sizeBytes(): number {
    return this.currentBytes;
  }

  keys(): string[] {
    return [...this.entries.keys()];
  }

  configure(budgetBytes: number): void {
    this.budgetBytes = Math.max(0, Math.floor(budgetBytes));
    this.evictUntilWithinBudget();
  }

  /** Marks the frame currently painted on the canvas, which is never evicted. */
  pin(key: string | null): void {
    this.pinnedKey = key;
  }

  has(key: string): boolean {
    return this.entries.has(key);
  }

  /** Read + recency touch. Use this on the paint path. */
  get(key: string): PoolableBitmap | null {
    const entry = this.entries.get(key);
    if (!entry) {
      return null;
    }
    this.entries.delete(key);
    this.entries.set(key, entry);
    return entry.bitmap;
  }

  /** Read WITHOUT touching recency. Use this for "is it warm?" probes. */
  peek(key: string): PoolableBitmap | null {
    return this.entries.get(key)?.bitmap ?? null;
  }

  set(key: string, bitmap: PoolableBitmap): void {
    this.delete(key);
    const bytes = decodedBitmapBytes(bitmap);
    this.entries.set(key, { bitmap, bytes });
    this.currentBytes += bytes;
    this.evictUntilWithinBudget();
  }

  delete(key: string): void {
    const entry = this.entries.get(key);
    if (!entry) {
      return;
    }
    this.entries.delete(key);
    this.currentBytes -= entry.bytes;
    // An ImageBitmap that is dropped without close() leaks GPU memory until GC
    // decides otherwise, which on a browsing session is never.
    entry.bitmap.close();
  }

  /** Releases every frame, pinned included. This is the library-switch path. */
  clear(): void {
    for (const entry of this.entries.values()) {
      entry.bitmap.close();
    }
    this.entries.clear();
    this.currentBytes = 0;
    this.pinnedKey = null;
  }

  private evictUntilWithinBudget(): void {
    for (const key of [...this.entries.keys()]) {
      if (this.currentBytes <= this.budgetBytes) {
        return;
      }
      // The pinned frame is on screen right now; closing it would blank the
      // canvas. The newest entry is the one the caller just asked for, so
      // evicting it would defeat the request that triggered this sweep.
      if (key === this.pinnedKey || this.entries.size <= 1) {
        continue;
      }
      this.delete(key);
    }
  }
}
