import type { CoreRequestPriority, MediaRecord, ThumbnailResponse } from "@mangoo/core-client";
import { createCoreClient } from "./client";
import {
  mediaContentSignature as createMediaContentSignature,
  normalizeMediaThumbnailState
} from "./mediaContentSignature";
import {
  clearThumbnailObjectUrlCache as clearThumbnailObjectUrlCacheInternal,
  readCachedThumbnailObjectUrl as readCachedThumbnailObjectUrlInternal,
  rememberThumbnailObjectUrl as rememberThumbnailObjectUrlInternal
} from "./thumbnailObjectUrlCache";

// The durable thumbnail object-URL cache is an isolated, unit-tested unit;
// re-export it so existing consumers keep importing it from here.
export {
  clearThumbnailObjectUrlCache,
  configureThumbnailCache,
  DEFAULT_THUMBNAIL_CACHE_LIMIT_BYTES,
  THUMBNAIL_OBJECT_URL_CACHE_MIN_ENTRIES,
  thumbnailObjectUrlCache
} from "./thumbnailObjectUrlCache";

export type ThumbnailStateByMediaId = Record<number, ThumbnailResponse>;

export function mediaPreviewKind(media: MediaRecord) {
  if (media.presentation) return media.presentation.previewKind;
  if (media.kind === "image") return "original-image";
  if (media.kind === "video") return "original-video";
  return "static-thumbnail";
}

export function isThumbnailUnavailable(media: MediaRecord): boolean {
  return media.presentation?.thumbnail === "unavailable" || (!media.presentation && mediaPreviewKind(media) === "static-thumbnail");
}
export type ThumbnailRequestPriority = "background" | "ahead" | "visible" | "selected";
type CachedThumbnailEntry = {
  mediaSignature: string;
  thumbnail: ThumbnailResponse;
};
type CachedOriginalPreviewEntry = {
  byteSize: number;
  mediaSignature: string;
  blob: Blob;
};
export type OriginalFallbackObjectUrlToken = Readonly<{
  cacheKey: string;
  epoch: number;
}>;
type SharedRequest<T> = {
  request: Promise<T>;
  controller: AbortController;
  activeConsumers: number;
  hasUnabortableConsumer: boolean;
};
type OriginalPreviewRequestOptions = {
  requestPriority?: CoreRequestPriority;
  resourcePriority?: ForegroundResourcePriority;
  signal?: AbortSignal;
};

type ThumbnailStateRequestOptions = {
  signal?: AbortSignal;
};

type ThumbnailBlobRequestOptions = {
  requestPriority?: CoreRequestPriority;
  resourcePriority?: ForegroundResourcePriority;
  signal?: AbortSignal;
};

export const GRID_THUMBNAIL_TARGET = "grid_320";

const thumbnailClient = createCoreClient();
export const thumbnailResourceCache = new Map<number, CachedThumbnailEntry>();
export const inFlightThumbnailRequests = new Map<string, SharedRequest<ThumbnailResponse>>();
export const inFlightThumbnailBlobRequests = new Map<string, SharedRequest<Blob>>();
export const originalPreviewBlobCache = new Map<number, CachedOriginalPreviewEntry>();
export const inFlightOriginalPreviewRequests = new Map<string, SharedRequest<Blob>>();
export const DEFAULT_ORIGINAL_PREVIEW_BUFFER_LIMIT_BYTES = 1200 * 1024 * 1024;
const ORIGINAL_FALLBACK_OBJECT_URL_CACHE_LIMIT = 128;
const originalFallbackObjectUrlCache = new Map<string, string>();
const MAX_FOREGROUND_RESOURCE_REQUESTS = 12;
const MAX_INTERACTIVE_FOREGROUND_RESOURCE_REQUESTS = 2;
// Off-screen prefetch (one screen above/below the viewport) is "ahead" priority.
// Visible requests still win by scheduler rank, so a slightly higher ahead
// concurrency lets the prefetched screen finish before it scrolls into view
// without starving the visible tiles.
const MAX_AHEAD_FOREGROUND_RESOURCE_REQUESTS = 6;
const MAX_FALLBACK_FOREGROUND_RESOURCE_REQUESTS = 6;

type ForegroundResourcePriority = ThumbnailRequestPriority | "preview" | "fallback";
type QueuedForegroundResourceRequest = {
  sequence: number;
  priority: ForegroundResourcePriority;
  signal: AbortSignal;
  run: (signal: AbortSignal) => Promise<unknown>;
  resolve: (value: unknown) => void;
  reject: (reason: unknown) => void;
  abort: () => void;
};

let foregroundResourceRequestSequence = 0;
let activeForegroundResourceRequests = 0;
let activeInteractiveForegroundResourceRequests = 0;
let activeAheadForegroundResourceRequests = 0;
let activeFallbackForegroundResourceRequests = 0;
let originalPreviewBlobCacheBytes = 0;
let originalPreviewBlobCacheLimitBytes = DEFAULT_ORIGINAL_PREVIEW_BUFFER_LIMIT_BYTES;
// Monotonic per-library epoch. Every switch to a different library bumps this,
// and it is baked into every string cache/dedup key below. A fresh .sqlite
// reuses the same fileId integers (1, 2, 3, ...), so a cache/in-flight entry
// keyed by fileId alone would be served for the NEW library's same-id file —
// the cross-library "串图" bleed. Threading the epoch guarantees old-library
// keys can never match new-library lookups. Bumping (vs only clearing) also
// neutralises in-flight thumbnail fetches that deliberately run without an
// AbortSignal: they resolve into a now-dead epoch key that nothing reads.
let mediaCacheEpoch = 0;

export function mediaCacheEpochToken(): number {
  return mediaCacheEpoch;
}

// Consumers outside `core/` (the preview bitmap pool lives in `features/`) need
// to release resources on a library switch. A listener registry keeps the
// dependency pointing the right way — core never imports a feature.
const mediaCacheResetListeners = new Set<() => void>();

export function registerMediaCacheResetListener(listener: () => void): () => void {
  mediaCacheResetListeners.add(listener);
  return () => {
    mediaCacheResetListeners.delete(listener);
  };
}

// Invalidate ALL media/thumbnail/preview caches for a library switch: bump the
// epoch (so any straggler in-flight request resolves into a dead key) and clear
// the number-keyed caches (which carry no epoch in their key). Call this the
// instant a switch begins ("starting-core"), before the new Core is healthy.
export function resetMediaCachesForLibrarySwitch(): void {
  mediaCacheEpoch += 1;
  abortSharedRequests(inFlightThumbnailRequests);
  abortSharedRequests(inFlightThumbnailBlobRequests);
  abortSharedRequests(inFlightOriginalPreviewRequests);
  thumbnailResourceCache.clear();
  originalPreviewBlobCache.clear();
  originalPreviewBlobCacheBytes = 0;
  clearThumbnailObjectUrlCacheInternal();
  clearOriginalFallbackObjectUrlCache();

  for (const listener of [...mediaCacheResetListeners]) {
    try {
      listener();
    } catch {
      // One listener failing must never strand the rest of the invalidation:
      // a half-cleared cache is exactly the cross-library "串图" bug.
    }
  }
}
const foregroundResourceRequestQueue: QueuedForegroundResourceRequest[] = [];

export async function requestThumbnailState(
  mediaRecord: MediaRecord,
  priority: ThumbnailRequestPriority = "background",
  options: ThumbnailStateRequestOptions = {}
): Promise<ThumbnailResponse> {
  const mediaId = mediaRecord.id;
  const requestKey = thumbnailRequestKey(mediaRecord);
  const cached = thumbnailResourceCache.get(mediaId);
  if (cached) {
    if (isFreshCachedThumbnailForMediaRecord(mediaRecord, cached)) {
      if (cached.thumbnail.state !== "pending" && cached.thumbnail.state !== "queued") {
        return cached.thumbnail;
      }
    } else {
      thumbnailResourceCache.delete(mediaId);
    }
  }

  if (isThumbnailUnavailable(mediaRecord) && mediaRecord.thumbnailState !== "ready") {
    return {
      fileId: mediaId, target: GRID_THUMBNAIL_TARGET, state: "unavailable", updatedAt: null,
      shortSidePx: 320, outputFormat: "image/webp", width: null, height: null,
      byteSize: null, servedBy: null, asset: null, error: null
    };
  }

  const inFlight = inFlightThumbnailRequests.get(requestKey);
  if (inFlight && !inFlight.controller.signal.aborted) {
    return withSharedAbortSignal(inFlight, options.signal);
  }

  if (options.signal?.aborted) {
    return Promise.reject(createAbortError());
  }

  const controller = new AbortController();
  const schedulerPriority = thumbnailStateSchedulerPriority(priority);
  const request = scheduleForegroundResourceRequest(
    schedulerPriority,
    (signal) => fetchThumbnailState(mediaRecord, priority, signal),
    controller
  ).finally(() => {
    if (inFlightThumbnailRequests.get(requestKey)?.request === request) {
      inFlightThumbnailRequests.delete(requestKey);
    }
  });
  const entry: SharedRequest<ThumbnailResponse> = {
    request,
    controller,
    activeConsumers: 0,
    hasUnabortableConsumer: false
  };
  inFlightThumbnailRequests.set(requestKey, entry);
  return withSharedAbortSignal(entry, options.signal);
}

export async function requestThumbnailBlob(
  fileId: number,
  versionKey: number | null = null,
  options: ThumbnailBlobRequestOptions = {}
): Promise<Blob> {
  if (options.signal?.aborted) {
    return Promise.reject(createAbortError());
  }

  const resourcePriority = options.resourcePriority ?? "visible";
  const coreRequestPriority =
    options.requestPriority ?? foregroundResourceCoreRequestPriority(resourcePriority);
  // Dedup by FILE + target only — NOT priority and NOT versionKey. The same
  // tile is requested at different priorities (visible/ahead) as it scrolls, and
  // at different versionKeys as its thumbnail-state poll resolves
  // (thumbnailUpdatedAt null -> timestamp). All of these fetch the SAME current
  // thumbnail bytes for the file, so keying priority or version into the dedup
  // key fetched the same blob 2-3x under scroll churn (the residual "reload"
  // flashes). Coalescing on file+target means the first in-flight request is
  // reused; the version stays in the actual URL only as an HTTP cache-buster.
  const requestKey = [mediaCacheEpoch, fileId, GRID_THUMBNAIL_TARGET].join(":");
  const inFlight = inFlightThumbnailBlobRequests.get(requestKey);
  if (inFlight && !inFlight.controller.signal.aborted) {
    return withSharedAbortSignal(inFlight, options.signal);
  }

  const controller = new AbortController();
  const request = scheduleForegroundResourceRequest(
    resourcePriority,
    (signal) => fetchThumbnailBlob(fileId, versionKey, signal, coreRequestPriority),
    controller
  ).finally(() => {
    if (inFlightThumbnailBlobRequests.get(requestKey)?.request === request) {
      inFlightThumbnailBlobRequests.delete(requestKey);
    }
  });
  const entry: SharedRequest<Blob> = {
    request,
    controller,
    activeConsumers: 0,
    hasUnabortableConsumer: false
  };
  inFlightThumbnailBlobRequests.set(requestKey, entry);
  return withSharedAbortSignal(entry, options.signal);
}

export function thumbnailObjectUrlCacheKey(
  fileId: number,
  mediaSignature: string,
  versionKey: number | null
): string {
  return `${mediaCacheEpoch}:${fileId}:${GRID_THUMBNAIL_TARGET}:${versionKey ?? mediaSignature}`;
}

export function readCachedThumbnailObjectUrl(cacheKey: string): string | null {
  if (!isCurrentMediaCacheKey(cacheKey)) return null;
  return readCachedThumbnailObjectUrlInternal(cacheKey);
}

export function rememberThumbnailObjectUrl(
  cacheKey: string,
  objectUrl: string,
  byteSize: number
): boolean {
  if (!isCurrentMediaCacheKey(cacheKey)) return false;
  return rememberThumbnailObjectUrlInternal(cacheKey, objectUrl, byteSize);
}

// Consumes a newly-created thumbnail object URL. A current-epoch candidate is
// either adopted by the shared cache or replaced by the cache's racing URL;
// a stale-epoch candidate is revoked immediately and is never publishable.
export function adoptThumbnailObjectUrl(
  cacheKey: string,
  candidateObjectUrl: string,
  byteSize: number
): string | null {
  if (rememberThumbnailObjectUrl(cacheKey, candidateObjectUrl, byteSize)) {
    return candidateObjectUrl;
  }

  const cachedObjectUrl = readCachedThumbnailObjectUrl(cacheKey);
  if (cachedObjectUrl === candidateObjectUrl) return cachedObjectUrl;
  URL.revokeObjectURL(candidateObjectUrl);
  return cachedObjectUrl;
}

export function captureOriginalFallbackObjectUrlToken(
  mediaRecordOrSignature: MediaRecord | string
): OriginalFallbackObjectUrlToken {
  const epoch = mediaCacheEpoch;
  const mediaSignature =
    typeof mediaRecordOrSignature === "string"
      ? mediaRecordOrSignature
      : mediaFileContentSignature(mediaRecordOrSignature);
  return {
    cacheKey: `${epoch}:original:${mediaSignature}`,
    epoch
  };
}

export function readCachedOriginalFallbackObjectUrl(
  token: OriginalFallbackObjectUrlToken
): string | null {
  if (token.epoch !== mediaCacheEpoch) return null;
  const objectUrl = originalFallbackObjectUrlCache.get(token.cacheKey);
  if (!objectUrl) return null;
  originalFallbackObjectUrlCache.delete(token.cacheKey);
  originalFallbackObjectUrlCache.set(token.cacheKey, objectUrl);
  return objectUrl;
}

export function createOriginalFallbackObjectUrl(
  token: OriginalFallbackObjectUrlToken,
  blob: Blob
): string | null {
  const objectUrl = URL.createObjectURL(blob);
  if (token.epoch !== mediaCacheEpoch) {
    URL.revokeObjectURL(objectUrl);
    return null;
  }

  const existing = originalFallbackObjectUrlCache.get(token.cacheKey);
  if (existing) {
    URL.revokeObjectURL(objectUrl);
    originalFallbackObjectUrlCache.delete(token.cacheKey);
    originalFallbackObjectUrlCache.set(token.cacheKey, existing);
    return existing;
  }

  originalFallbackObjectUrlCache.set(token.cacheKey, objectUrl);
  while (originalFallbackObjectUrlCache.size > ORIGINAL_FALLBACK_OBJECT_URL_CACHE_LIMIT) {
    const firstKey = originalFallbackObjectUrlCache.keys().next().value;
    if (firstKey === undefined) break;
    const staleObjectUrl = originalFallbackObjectUrlCache.get(firstKey);
    originalFallbackObjectUrlCache.delete(firstKey);
    if (staleObjectUrl) URL.revokeObjectURL(staleObjectUrl);
  }
  return objectUrl;
}

export function discardOriginalFallbackObjectUrl(
  token: OriginalFallbackObjectUrlToken,
  expectedObjectUrl: string
): void {
  if (originalFallbackObjectUrlCache.get(token.cacheKey) !== expectedObjectUrl) return;
  originalFallbackObjectUrlCache.delete(token.cacheKey);
  URL.revokeObjectURL(expectedObjectUrl);
}

function clearOriginalFallbackObjectUrlCache(): void {
  for (const objectUrl of originalFallbackObjectUrlCache.values()) {
    URL.revokeObjectURL(objectUrl);
  }
  originalFallbackObjectUrlCache.clear();
}

function isCurrentMediaCacheKey(cacheKey: string): boolean {
  return cacheKey.startsWith(`${mediaCacheEpoch}:`);
}

/// Prefetch a ready thumbnail's WebP bytes into the decoded object-URL cache so
/// a tile that later scrolls into view renders instantly — no blob fetch, no
/// decode, no placeholder flash or fade-in at mount. This is the "ahead"
/// counterpart to `ReadyThumbnail`'s own on-mount fetch: it uses the SAME
/// content-keyed cache (`thumbnailObjectUrlCacheKey`) and the same shared,
/// deduped `requestThumbnailBlob`, so when the tile mounts,
/// `readCachedThumbnailObjectUrl` already returns the URL.
///
/// No-ops when the URL is already cached, when the media isn't a fresh ready
/// thumbnail, or on any fetch/decode error (best-effort prefetch, never surfaces
/// an error). `resourcePriority` defaults to "ahead" so it yields to visible
/// tile fetches in the request scheduler. `assumeReady` lets a caller that
/// already knows the polled thumbnail state is "ready" bypass the record-based
/// readiness check — needed for a freshly-scanned folder whose media records
/// don't yet carry thumbnail state (readiness arrives via the state poll).
export function prefetchThumbnailBlob(
  mediaRecord: MediaRecord,
  options: {
    resourcePriority?: "visible" | "ahead";
    signal?: AbortSignal;
    assumeReady?: boolean;
  } = {}
): void {
  if (!options.assumeReady && !isReadyThumbnailMediaRecord(mediaRecord)) {
    return;
  }
  const contentSignature = mediaFileContentSignature(mediaRecord);
  const cacheKey = thumbnailObjectUrlCacheKey(mediaRecord.id, contentSignature, null);
  if (readCachedThumbnailObjectUrl(cacheKey)) {
    return;
  }
  const resourcePriority = options.resourcePriority ?? "ahead";
  void requestThumbnailBlob(mediaRecord.id, null, {
    requestPriority: "resource",
    resourcePriority,
    signal: options.signal
  })
    .then((blob) => {
      // Re-check under the async boundary: the tile may have mounted and cached
      // it, or the media content may have changed. Only cache a still-missing,
      // still-valid entry.
      if (readCachedThumbnailObjectUrl(cacheKey)) {
        return;
      }
      const objectUrl = URL.createObjectURL(blob);
      // Decode before publishing so the tile's <img> paints synchronously on
      // mount instead of decoding on the main thread mid-scroll.
      void preloadImageObjectUrl(objectUrl)
        .then(() => {
          if (!rememberThumbnailObjectUrl(cacheKey, objectUrl, blob.size)) {
            URL.revokeObjectURL(objectUrl);
          }
        })
        .catch(() => {
          URL.revokeObjectURL(objectUrl);
        });
    })
    .catch(() => {
      // Best-effort prefetch: swallow aborts and fetch failures. The tile's own
      // on-mount fetch will retry if it actually scrolls into view.
    });
}

export function preloadImageObjectUrl(
  objectUrl: string
): Promise<{ naturalHeight: number; naturalWidth: number }> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.decoding = "async";

    const cleanup = () => {
      image.onload = null;
      image.onerror = null;
    };

    image.onload = () => {
      const decode = typeof image.decode === "function" ? image.decode() : Promise.resolve();
      decode
        .catch(() => undefined)
        .then(() => {
          cleanup();
          resolve({
            naturalHeight: image.naturalHeight,
            naturalWidth: image.naturalWidth
          });
        });
    };
    image.onerror = () => {
      cleanup();
      reject(new Error("image preload failed"));
    };
    image.src = objectUrl;
  });
}

export function configureOriginalPreviewBuffer(maxBytes: number): void {
  originalPreviewBlobCacheLimitBytes = Math.max(0, Math.floor(maxBytes));
  pruneOriginalPreviewCache();
}

export function requestOriginalPreviewBlob(
  mediaRecord: MediaRecord,
  options: OriginalPreviewRequestOptions = {}
): Promise<Blob> {
  if (mediaPreviewKind(mediaRecord) === "static-thumbnail" && isThumbnailUnavailable(mediaRecord)) {
    return Promise.reject(new Error("Original preview unavailable for this file"));
  }
  const mediaSignature = mediaPreviewContentSignature(mediaRecord);
  const cached = originalPreviewBlobCache.get(mediaRecord.id);
  if (cached) {
    if (cached.mediaSignature === mediaSignature) {
      originalPreviewBlobCache.delete(mediaRecord.id);
      originalPreviewBlobCache.set(mediaRecord.id, cached);
      return withAbortSignal(Promise.resolve(cached.blob), options.signal);
    }
    deleteOriginalPreviewCacheEntry(mediaRecord.id);
  }

  if (options.signal?.aborted) {
    return Promise.reject(createAbortError());
  }

  const resourcePriority = options.resourcePriority ?? "preview";
  const coreRequestPriority =
    options.requestPriority ?? foregroundResourceCoreRequestPriority(resourcePriority);
  const requestKey = [
    originalPreviewRequestKey(mediaRecord)
  ].join(":");
  const inFlight = inFlightOriginalPreviewRequests.get(requestKey);
  if (inFlight && !inFlight.controller.signal.aborted) {
    return withSharedAbortSignal(inFlight, options.signal);
  }

  const controller = new AbortController();
  const requestEpoch = mediaCacheEpoch;
  const request = scheduleForegroundResourceRequest(
    resourcePriority,
    (signal) => fetchOriginalPreviewBlob(
      mediaRecord,
      mediaSignature,
      requestEpoch,
      signal,
      coreRequestPriority
    ),
    controller
  ).finally(() => {
    if (inFlightOriginalPreviewRequests.get(requestKey)?.request === request) {
      inFlightOriginalPreviewRequests.delete(requestKey);
    }
  });
  const entry: SharedRequest<Blob> = {
    request,
    controller,
    activeConsumers: 0,
    hasUnabortableConsumer: false
  };
  inFlightOriginalPreviewRequests.set(requestKey, entry);
  return withSharedAbortSignal(entry, options.signal);
}

export function prefetchOriginalPreview(
  mediaRecord: MediaRecord,
  options: OriginalPreviewRequestOptions = {}
): void {
  void requestOriginalPreviewBlob(mediaRecord, {
    ...options,
    requestPriority: options.requestPriority ?? "resource",
    resourcePriority: options.resourcePriority ?? "ahead"
  }).catch((cause) => {
    if (isAbortError(cause)) {
      return;
    }
  });
}

export function readCachedThumbnailStates(mediaRecords: MediaRecord[]): ThumbnailStateByMediaId {
  const states: ThumbnailStateByMediaId = {};
  for (const mediaRecord of mediaRecords) {
    const entry = thumbnailResourceCache.get(mediaRecord.id);
    if (!entry) {
      continue;
    }
    if (isFreshCachedThumbnailForMediaRecord(mediaRecord, entry)) {
      states[mediaRecord.id] = entry.thumbnail;
    } else {
      thumbnailResourceCache.delete(mediaRecord.id);
    }
  }
  return states;
}

export function shouldRequestThumbnailState(mediaRecord: MediaRecord): boolean {
  if (isThumbnailUnavailable(mediaRecord) && mediaRecord.thumbnailState !== "ready") return false;
  const mediaState = normalizeMediaThumbnailState(mediaRecord.thumbnailState);
  if (mediaState === "failed" || mediaState === "skipped_small" || mediaState === "unavailable") {
    return false;
  }
  return true;
}

/// Whether this media has a ready thumbnail worth prefetching. True when either
/// the record itself reports "ready" or the polled thumbnail-state cache has a
/// fresh "ready" entry for it. Used to gate ahead-of-viewport blob prefetch so
/// we never fetch blobs for pending/failed/skipped tiles.
function isReadyThumbnailMediaRecord(mediaRecord: MediaRecord): boolean {
  if (explicitMediaThumbnailState(mediaRecord.thumbnailState) === "ready") {
    return true;
  }
  const cached = thumbnailResourceCache.get(mediaRecord.id);
  return Boolean(
    cached &&
      cached.thumbnail.state === "ready" &&
      isFreshCachedThumbnailForMediaRecord(mediaRecord, cached)
  );
}

export function isFreshThumbnailForMediaRecord(
  mediaRecord: MediaRecord,
  thumbnail: ThumbnailResponse
): boolean {
  return isFreshThumbnailResponseForMediaRecord(mediaRecord, thumbnail);
}

export function pickPreferredThumbnailResponse(
  current: ThumbnailResponse | undefined,
  next: ThumbnailResponse
): ThumbnailResponse {
  if (!current) {
    return next;
  }

  const currentRank = thumbnailStateRank(current.state);
  const nextRank = thumbnailStateRank(next.state);
  if (nextRank > currentRank) {
    return next;
  }
  if (nextRank < currentRank) {
    return current;
  }

  if (next.state === "ready" && current.state === "ready") {
    const currentUpdatedAt = current.updatedAt ?? -1;
    const nextUpdatedAt = next.updatedAt ?? -1;
    if (nextUpdatedAt > currentUpdatedAt) {
      return next;
    }
    if (nextUpdatedAt < currentUpdatedAt) {
      return current;
    }
  }

  return next;
}

export function isFreshCachedThumbnailForMediaRecord(
  mediaRecord: MediaRecord,
  entry: CachedThumbnailEntry
): boolean {
  if (entry.mediaSignature !== mediaContentSignature(mediaRecord)) {
    return false;
  }
  return isFreshThumbnailResponseForMediaRecord(mediaRecord, entry.thumbnail);
}

function isFreshThumbnailResponseForMediaRecord(
  mediaRecord: MediaRecord,
  thumbnail: ThumbnailResponse
): boolean {
  if (thumbnail.fileId !== mediaRecord.id) {
    return false;
  }
  if (thumbnail.target !== GRID_THUMBNAIL_TARGET) {
    return false;
  }
  const mediaState = explicitMediaThumbnailState(mediaRecord.thumbnailState);
  if (mediaState === "ready" && thumbnail.state !== "ready") {
    return false;
  }
  return true;
}

export function isLiveThumbnailResponseForMediaRecord(
  mediaRecord: MediaRecord,
  thumbnail: ThumbnailResponse
): boolean {
  return thumbnail.fileId === mediaRecord.id && thumbnail.target === GRID_THUMBNAIL_TARGET;
}

function explicitMediaThumbnailState(
  value: string | null | undefined
): ThumbnailResponse["state"] | null {
  if (
    value === "pending" ||
    value === "queued" ||
    value === "ready" ||
    value === "failed" ||
    value === "skipped_small" ||
    value === "unavailable"
  ) {
    return value;
  }
  return null;
}

function isTerminalThumbnailState(value: ThumbnailResponse["state"]): boolean {
  return value === "ready" || value === "failed" || value === "skipped_small" || value === "unavailable";
}

export function previewPlaceholderDataUrl(mediaRecord: MediaRecord): string | null {
  const bytes = mediaRecord.previewPlaceholder;
  if (!bytes || bytes.length === 0) {
    return null;
  }
  const mediaType = mediaRecord.previewPlaceholderFormat ?? "image/webp";
  return `data:${mediaType};base64,${bytesToBase64(bytes)}`;
}

function thumbnailRequestKey(mediaRecord: MediaRecord): string {
  // Dedup by content identity only, not priority (see requestThumbnailBlob): the
  // same record polled at "visible" then "ahead" must share one in-flight state
  // request, not fire two.
  return [mediaCacheEpoch, mediaContentSignature(mediaRecord), GRID_THUMBNAIL_TARGET].join(":");
}

function thumbnailStateSchedulerPriority(
  priority: ThumbnailRequestPriority
): ForegroundResourcePriority {
  return priority;
}

function originalPreviewRequestKey(mediaRecord: MediaRecord): string {
  return [mediaCacheEpoch, mediaPreviewContentSignature(mediaRecord), "original"].join(":");
}

function rememberOriginalPreviewBlob(
  mediaRecord: MediaRecord,
  mediaSignature: string,
  blob: Blob
): void {
  deleteOriginalPreviewCacheEntry(mediaRecord.id);
  const byteSize = blob.size;
  originalPreviewBlobCache.set(mediaRecord.id, {
    blob,
    byteSize,
    mediaSignature
  });
  originalPreviewBlobCacheBytes += byteSize;
  pruneOriginalPreviewCache();
}

function abortSharedRequests<T>(requests: Map<string, SharedRequest<T>>): void {
  for (const request of requests.values()) {
    request.controller.abort();
  }
  requests.clear();
}

function deleteOriginalPreviewCacheEntry(fileId: number): void {
  const cached = originalPreviewBlobCache.get(fileId);
  if (cached) {
    originalPreviewBlobCacheBytes = Math.max(0, originalPreviewBlobCacheBytes - cached.byteSize);
  }
  originalPreviewBlobCache.delete(fileId);
}

function pruneOriginalPreviewCache(): void {
  while (originalPreviewBlobCacheBytes > originalPreviewBlobCacheLimitBytes) {
    const firstKey = originalPreviewBlobCache.keys().next().value;
    if (firstKey === undefined) break;
    deleteOriginalPreviewCacheEntry(firstKey);
  }
}

function scheduleForegroundResourceRequest<T>(
  priority: ForegroundResourcePriority,
  run: (signal: AbortSignal) => Promise<T>,
  controller: AbortController
): Promise<T> {
  const signal = controller.signal;
  if (signal.aborted) {
    return Promise.reject(createAbortError());
  }

  return new Promise<T>((resolve, reject) => {
    const entry: QueuedForegroundResourceRequest = {
      sequence: foregroundResourceRequestSequence++,
      priority,
      signal,
      run: run as (signal: AbortSignal) => Promise<unknown>,
      resolve: resolve as (value: unknown) => void,
      reject,
      abort: () => undefined
    };
    entry.abort = () => {
      const queuedIndex = foregroundResourceRequestQueue.indexOf(entry);
      if (queuedIndex >= 0) {
        foregroundResourceRequestQueue.splice(queuedIndex, 1);
      }
      signal.removeEventListener("abort", entry.abort);
      reject(createAbortError());
    };

    signal.addEventListener("abort", entry.abort, { once: true });
    foregroundResourceRequestQueue.push(entry);
    pumpForegroundResourceRequests();
  });
}

function pumpForegroundResourceRequests(): void {
  if (
    activeNonInteractiveForegroundResourceRequests() >= MAX_FOREGROUND_RESOURCE_REQUESTS &&
    activeInteractiveForegroundResourceRequests >= MAX_INTERACTIVE_FOREGROUND_RESOURCE_REQUESTS
  ) {
    return;
  }

  foregroundResourceRequestQueue.sort((left, right) => {
    const priorityDelta =
      thumbnailRequestPriorityRank(right.priority) - thumbnailRequestPriorityRank(left.priority);
    return priorityDelta === 0 ? left.sequence - right.sequence : priorityDelta;
  });

  while (foregroundResourceRequestQueue.length > 0) {
    const entryIndex = foregroundResourceRequestQueue.findIndex((candidate) =>
      canStartForegroundResourceRequest(candidate.priority)
    );
    if (entryIndex < 0) {
      return;
    }
    const entry = foregroundResourceRequestQueue.splice(entryIndex, 1)[0];
    if (!entry) {
      return;
    }
    if (entry.signal.aborted) {
      entry.signal.removeEventListener("abort", entry.abort);
      entry.reject(createAbortError());
      continue;
    }

    activeForegroundResourceRequests += 1;
    const isInteractiveRequest = isInteractiveForegroundResourceRequest(entry.priority);
    const isAheadRequest = isAheadForegroundResourceRequest(entry.priority);
    const isFallbackRequest = isFallbackForegroundResourceRequest(entry.priority);
    if (isInteractiveRequest) {
      activeInteractiveForegroundResourceRequests += 1;
    }
    if (isAheadRequest) {
      activeAheadForegroundResourceRequests += 1;
    }
    if (isFallbackRequest) {
      activeFallbackForegroundResourceRequests += 1;
    }
    entry
      .run(entry.signal)
      .then(entry.resolve, entry.reject)
      .finally(() => {
        entry.signal.removeEventListener("abort", entry.abort);
        activeForegroundResourceRequests = Math.max(0, activeForegroundResourceRequests - 1);
        if (isInteractiveRequest) {
          activeInteractiveForegroundResourceRequests = Math.max(
            0,
            activeInteractiveForegroundResourceRequests - 1
          );
        }
        if (isAheadRequest) {
          activeAheadForegroundResourceRequests = Math.max(
            0,
            activeAheadForegroundResourceRequests - 1
          );
        }
        if (isFallbackRequest) {
          activeFallbackForegroundResourceRequests = Math.max(
            0,
            activeFallbackForegroundResourceRequests - 1
          );
        }
        pumpForegroundResourceRequests();
      });
  }
}

function activeNonInteractiveForegroundResourceRequests(): number {
  return Math.max(0, activeForegroundResourceRequests - activeInteractiveForegroundResourceRequests);
}

function canStartForegroundResourceRequest(priority: ForegroundResourcePriority): boolean {
  if (isInteractiveForegroundResourceRequest(priority)) {
    return activeInteractiveForegroundResourceRequests < MAX_INTERACTIVE_FOREGROUND_RESOURCE_REQUESTS;
  }
  if (
    isAheadForegroundResourceRequest(priority) &&
    activeAheadForegroundResourceRequests >= MAX_AHEAD_FOREGROUND_RESOURCE_REQUESTS
  ) {
    return false;
  }
  if (
    isFallbackForegroundResourceRequest(priority) &&
    activeFallbackForegroundResourceRequests >= MAX_FALLBACK_FOREGROUND_RESOURCE_REQUESTS
  ) {
    return false;
  }
  return activeNonInteractiveForegroundResourceRequests() < MAX_FOREGROUND_RESOURCE_REQUESTS;
}

function isInteractiveForegroundResourceRequest(priority: ForegroundResourcePriority): boolean {
  return priority === "selected" || priority === "preview";
}

function isAheadForegroundResourceRequest(priority: ForegroundResourcePriority): boolean {
  return priority === "ahead" || priority === "background";
}

function isFallbackForegroundResourceRequest(priority: ForegroundResourcePriority): boolean {
  return priority === "fallback";
}

function thumbnailRequestPriorityRank(priority: ForegroundResourcePriority): number {
  switch (priority) {
    case "selected":
    case "preview":
      return 4;
    case "visible":
      return 3;
    case "fallback":
      return 2.5;
    case "ahead":
      return 2;
    case "background":
    default:
      return 1;
  }
}

function fetchThumbnailBlob(
  fileId: number,
  versionKey: number | null,
  signal: AbortSignal | undefined,
  coreRequestPriority: CoreRequestPriority
): Promise<Blob> {
  return thumbnailClient.getThumbnailBlob(fileId, GRID_THUMBNAIL_TARGET, {
    requestPriority: coreRequestPriority,
    signal,
    version: versionKey
  });
}

function fetchThumbnailState(
  mediaRecord: MediaRecord,
  priority: ThumbnailRequestPriority,
  signal?: AbortSignal
): Promise<ThumbnailResponse> {
  return thumbnailClient
    .getThumbnail(mediaRecord.id, GRID_THUMBNAIL_TARGET, priority, { signal })
    .then((thumbnail) => {
      if (isLiveThumbnailResponseForMediaRecord(mediaRecord, thumbnail)) {
        const mediaSignature = mediaContentSignature(mediaRecord);
        const cachedEntry = thumbnailResourceCache.get(mediaRecord.id);
        thumbnailResourceCache.set(mediaRecord.id, {
          mediaSignature,
          thumbnail:
            cachedEntry?.mediaSignature === mediaSignature
              ? pickPreferredThumbnailResponse(cachedEntry.thumbnail, thumbnail)
              : thumbnail
        });
      } else {
        thumbnailResourceCache.delete(mediaRecord.id);
      }
      return thumbnail;
    });
}

function fetchOriginalPreviewBlob(
  mediaRecord: MediaRecord,
  mediaSignature: string,
  requestEpoch: number,
  signal: AbortSignal | undefined,
  coreRequestPriority: CoreRequestPriority
): Promise<Blob> {
  return thumbnailClient
    .getPreviewBlob(mediaRecord.id, {
      requestPriority: coreRequestPriority,
      signal,
      version: mediaSignature
    })
    .then((blob) => {
      if (requestEpoch === mediaCacheEpoch) {
        rememberOriginalPreviewBlob(mediaRecord, mediaSignature, blob);
      }
      return blob;
    });
}

function foregroundResourceCoreRequestPriority(
  priority: ForegroundResourcePriority
): CoreRequestPriority {
  switch (priority) {
    case "selected":
    case "preview":
      return "interactive";
    case "visible":
    case "ahead":
    case "fallback":
      return "resource";
    case "background":
    default:
      return "background";
  }
}

function withAbortSignal<T>(request: Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return request;
  if (signal.aborted) return Promise.reject(createAbortError());

  return new Promise((resolve, reject) => {
    const abort = () => reject(createAbortError());
    signal.addEventListener("abort", abort, { once: true });
    request.then(resolve, reject).finally(() => {
      signal.removeEventListener("abort", abort);
    });
  });
}

function withSharedAbortSignal<T>(
  entry: {
    request: Promise<T>;
    controller: AbortController;
    activeConsumers: number;
    hasUnabortableConsumer: boolean;
  },
  signal?: AbortSignal
): Promise<T> {
  if (!signal) {
    entry.hasUnabortableConsumer = true;
    return entry.request;
  }
  if (signal.aborted) return Promise.reject(createAbortError());

  entry.activeConsumers += 1;
  let released = false;

  return new Promise((resolve, reject) => {
    const release = () => {
      if (released) return;
      released = true;
      signal.removeEventListener("abort", abort);
      entry.activeConsumers = Math.max(0, entry.activeConsumers - 1);
      // A departed view rejects immediately, but the shared request keeps
      // running so its bounded result can populate the cache for the next
      // consumer. Only a library-epoch reset aborts the underlying request.
    };
    const abort = () => {
      release();
      reject(createAbortError());
    };

    signal.addEventListener("abort", abort, { once: true });
    entry.request.then(
      (value) => {
        release();
        resolve(value);
      },
      (error) => {
        release();
        reject(error);
      }
    );
  });
}

function createAbortError(): Error {
  const error = new Error("The operation was aborted.");
  error.name = "AbortError";
  return error;
}

function isAbortError(cause: unknown): boolean {
  return cause instanceof Error && cause.name === "AbortError";
}

export function mediaContentSignature(mediaRecord: MediaRecord): string {
  return createMediaContentSignature(mediaRecord);
}

export function mediaFileContentSignature(mediaRecord: MediaRecord): string {
  return [mediaRecord.id, mediaRecord.mtime, mediaRecord.size].join(":");
}

export function mediaPreviewContentSignature(mediaRecord: MediaRecord): string {
  // Decoder output may change with capabilities even when source bytes do not.
  return mediaPreviewKind(mediaRecord) === "static-thumbnail"
    ? [mediaFileContentSignature(mediaRecord), mediaRecord.presentation?.generation ?? "legacy"].join(":")
    : mediaFileContentSignature(mediaRecord);
}

function thumbnailStateRank(state: ThumbnailResponse["state"]): number {
  switch (state) {
    case "ready":
      return 4;
    case "failed":
      return 3;
    case "skipped_small":
    case "unavailable":
      return 2;
    case "queued":
      return 1;
    case "pending":
    default:
      return 0;
  }
}

function bytesToBase64(bytes: number[]): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8192) {
    const chunk = bytes.slice(offset, offset + 8192);
    binary += String.fromCharCode(...chunk);
  }
  return btoa(binary);
}
