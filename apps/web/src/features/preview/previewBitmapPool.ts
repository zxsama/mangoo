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

/** Non-finite (NaN/Infinity) dimensions are treated as 0, never propagated. */
function finiteNonNegative(value: number): number {
  return Number.isFinite(value) ? Math.max(0, value) : 0;
}

export function decodedBitmapBytes(bitmap: PoolableBitmap): number {
  return (
    finiteNonNegative(bitmap.width) * finiteNonNegative(bitmap.height) * BYTES_PER_DECODED_PIXEL
  );
}

type PoolEntry = {
  bitmap: PoolableBitmap;
  bytes: number;
};

/** Non-finite (NaN/Infinity) budgets are treated as 0, never propagated. */
function sanitizeBudgetBytes(budgetBytes: number): number {
  return Number.isFinite(budgetBytes) ? Math.max(0, Math.floor(budgetBytes)) : 0;
}

export class PreviewBitmapPool {
  // Insertion order IS recency order: JS Map preserves it, and re-inserting
  // moves a key to the end. Same technique the thumbnail object-URL cache uses.
  private readonly entries = new Map<string, PoolEntry>();
  private budgetBytes: number;
  private currentBytes = 0;
  private pinnedKey: string | null = null;

  constructor(budgetBytes: number) {
    this.budgetBytes = sanitizeBudgetBytes(budgetBytes);
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
    this.budgetBytes = sanitizeBudgetBytes(budgetBytes);
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
    const existing = this.entries.get(key);
    if (existing && existing.bitmap === bitmap) {
      // Re-setting the exact same object under its own key must be a no-op:
      // `delete()` below would close() the bitmap the caller is still holding
      // and then store the now-closed object under the same key.
      return;
    }
    this.delete(key);
    const bytes = decodedBitmapBytes(bitmap);
    this.entries.set(key, { bitmap, bytes });
    this.currentBytes += bytes;
    // Protect the entry just admitted — it is what the caller is about to
    // paint/hold, so the eviction sweep must never be allowed to close it.
    this.evictUntilWithinBudget(key);
  }

  delete(key: string): void {
    const entry = this.entries.get(key);
    if (!entry) {
      return;
    }
    this.entries.delete(key);
    this.currentBytes -= entry.bytes;
    if (key === this.pinnedKey) {
      // Otherwise the stale pin lingers and silently confers eviction
      // immunity on whatever gets stored under this key next, without
      // anyone calling pin() again.
      this.pinnedKey = null;
    }
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

  /**
   * Sweeps oldest-to-newest, closing unprotected entries until back within
   * budget. Two keys are protected: the pinned frame (on screen right now —
   * closing it would blank the canvas), and `protectedKey`, the entry the
   * caller just admitted via `set()` (closing it would hand back a bitmap
   * the caller is about to draw with, which is already closed).
   *
   * `configure()` deliberately passes no `protectedKey`: a budget shrink is
   * free to evict everything down to the pinned frame (or to nothing, if
   * unpinned) — there is no "entry just admitted" to protect there.
   *
   * A pinned entry plus a newly admitted entry can together exceed the
   * budget on their own (e.g. one 60 MB pinned frame and one fresh 60 MB
   * neighbour against a 100 MB budget) — that is the case `pin()` exists to
   * survive, and the sweep must leave the budget over rather than close
   * either of them.
   */
  private evictUntilWithinBudget(protectedKey: string | null = null): void {
    for (const key of [...this.entries.keys()]) {
      if (this.currentBytes <= this.budgetBytes) {
        return;
      }
      if (key === this.pinnedKey || key === protectedKey) {
        continue;
      }
      this.delete(key);
    }
  }
}
