// Durable in-memory cache of decoded thumbnail object URLs, kept isolated from
// the rest of `mediaResources` so it has no bundler/browser-coupled imports
// (e.g. `import.meta.env`) and can be unit-tested in plain Node.
//
// Retention is bounded by max(byte budget, count floor):
//   - The COUNT FLOOR guarantees the most-recently-used working set is never
//     evicted on normal scrolling, even when the user's byte budget is small or
//     zero. Grid thumbnails are tiny (~10-30 KB), so the floor is cheap.
//   - The BYTE BUDGET only caps growth far beyond a working set.
// A tile scrolled out of and back into view therefore reuses its cached image
// instead of re-fetching it.

type CachedThumbnailObjectUrlEntry = {
  byteSize: number;
  objectUrl: string;
};

export const thumbnailObjectUrlCache = new Map<string, CachedThumbnailObjectUrlEntry>();
export const DEFAULT_THUMBNAIL_CACHE_LIMIT_BYTES = 5120 * 1024 * 1024;
// Retain at least this many most-recently-used thumbnail object URLs even when
// the byte budget (a user setting) is smaller, so scrolling out of and back
// into view never re-fetches a thumbnail. ~40-120 MB at typical sizes.
export const THUMBNAIL_OBJECT_URL_CACHE_MIN_ENTRIES = 4000;

let thumbnailObjectUrlCacheBytes = 0;
let thumbnailObjectUrlCacheLimitBytes = DEFAULT_THUMBNAIL_CACHE_LIMIT_BYTES;

export function readCachedThumbnailObjectUrl(cacheKey: string): string | null {
  const cached = thumbnailObjectUrlCache.get(cacheKey);
  if (!cached) {
    return null;
  }
  // Re-insert to mark most-recently-used (Map preserves insertion order).
  thumbnailObjectUrlCache.delete(cacheKey);
  thumbnailObjectUrlCache.set(cacheKey, cached);
  return cached.objectUrl;
}

export function rememberThumbnailObjectUrl(
  cacheKey: string,
  objectUrl: string,
  byteSize: number
): boolean {
  const normalizedByteSize = Math.max(0, Math.floor(byteSize));
  const previous = thumbnailObjectUrlCache.get(cacheKey);
  if (previous?.objectUrl === objectUrl) {
    thumbnailObjectUrlCacheBytes = Math.max(
      0,
      thumbnailObjectUrlCacheBytes - previous.byteSize + normalizedByteSize
    );
    thumbnailObjectUrlCache.delete(cacheKey);
    thumbnailObjectUrlCache.set(cacheKey, {
      ...previous,
      byteSize: normalizedByteSize
    });
    pruneThumbnailObjectUrlCache();
    return thumbnailObjectUrlCache.get(cacheKey)?.objectUrl === objectUrl;
  }

  if (previous) {
    deleteThumbnailObjectUrlCacheEntry(cacheKey);
  }
  // Always retain the freshly loaded thumbnail (it is the most-recently-used
  // entry); pruning below enforces the byte budget but never drops below the
  // count floor, so a small byte setting cannot evict the visible working set.
  thumbnailObjectUrlCache.set(cacheKey, { byteSize: normalizedByteSize, objectUrl });
  thumbnailObjectUrlCacheBytes += normalizedByteSize;
  pruneThumbnailObjectUrlCache();
  return thumbnailObjectUrlCache.get(cacheKey)?.objectUrl === objectUrl;
}

export function configureThumbnailCache(maxBytes: number): void {
  thumbnailObjectUrlCacheLimitBytes = Math.max(0, Math.floor(maxBytes));
  pruneThumbnailObjectUrlCache();
}

/// Revoke and drop every cached object URL. Used when the persistent thumbnail
/// cache is cleared/regenerated so stale images cannot be served, and to reset
/// state between tests.
export function clearThumbnailObjectUrlCache(): void {
  for (const entry of thumbnailObjectUrlCache.values()) {
    URL.revokeObjectURL(entry.objectUrl);
  }
  thumbnailObjectUrlCache.clear();
  thumbnailObjectUrlCacheBytes = 0;
}

function pruneThumbnailObjectUrlCache(): void {
  // Evict least-recently-used entries only while over the byte budget AND above
  // the count floor, so out-of-view thumbnails survive normal scrolling
  // regardless of the (user-configurable) byte limit.
  while (
    thumbnailObjectUrlCacheBytes > thumbnailObjectUrlCacheLimitBytes &&
    thumbnailObjectUrlCache.size > THUMBNAIL_OBJECT_URL_CACHE_MIN_ENTRIES
  ) {
    const firstKey = thumbnailObjectUrlCache.keys().next().value;
    if (firstKey === undefined) break;
    deleteThumbnailObjectUrlCacheEntry(firstKey);
  }
}

function deleteThumbnailObjectUrlCacheEntry(cacheKey: string): void {
  const cached = thumbnailObjectUrlCache.get(cacheKey);
  if (cached) {
    thumbnailObjectUrlCacheBytes = Math.max(0, thumbnailObjectUrlCacheBytes - cached.byteSize);
    URL.revokeObjectURL(cached.objectUrl);
  }
  thumbnailObjectUrlCache.delete(cacheKey);
}
