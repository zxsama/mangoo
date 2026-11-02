import type {
  ThumbnailCacheEnqueueResponse,
  ThumbnailCacheRefreshMode,
  ThumbnailCacheTaskRequest
} from "@mangoo/core-client";

// One bounded enqueue window. Kept equal to the media page window (256): each
// request scans at most this many files, so every DB-writer job stays short.
export const THUMBNAIL_CACHE_ENQUEUE_WINDOW_LIMIT = 256;

export interface ThumbnailCacheEnqueueScope {
  rootId?: number;
  folderId?: number;
  includeDescendants?: boolean;
}

// The minimal Core-client surface the enqueue loop needs. Declaring it here
// (instead of taking the whole `createCoreClient` return) keeps the loop unit-
// testable with a tiny fake client.
export interface ThumbnailCacheEnqueueClient {
  enqueueThumbnailCache: (
    input: ThumbnailCacheTaskRequest,
    options?: { requestPriority?: "background"; signal?: AbortSignal }
  ) => Promise<ThumbnailCacheEnqueueResponse>;
}

// Drive a whole-scope thumbnail-cache enqueue to completion, batch by batch,
// under the caller's own AbortController.
//
// `staleOrMissing` (generation) walks the scope with a KEYSET CURSOR: each
// request scans one bounded window (Core returns `nextCursor`) and enqueues the
// candidates in it, so the whole scope costs one linear scan. The old
// non-cursor loop re-requested the same scope with no cursor, and Core answered
// each call with `ORDER BY file_id LIMIT` filtered to not-yet-enqueued files —
// which re-scans from file_id 0 every batch (O(files²)). Because the enqueue
// runs inside the single DB writer, that O(files²) re-scan monopolized the
// writer for 100+ seconds on a large freshly-scanned folder, so concurrent
// writes like add-root queued behind it and hit the client's request timeout.
// The cursor keeps each writer job short so interactive writes interleave. This
// mirrors the whole-library "generate all" loop.
//
// `retryFailedAndStale` (revive the usually-few failed rows in scope) keeps the
// ACCEPT-COUNT termination instead: it stops as soon as a batch accepts nothing
// left to revive, reading the real per-batch stats. A keyset walk would scan
// every file in scope (thousands of batches on a large tree) just to confirm
// there is nothing failed left, and the windowed path returns zeroed stats.
export async function runThumbnailCacheEnqueueBatches(
  client: ThumbnailCacheEnqueueClient,
  scope: ThumbnailCacheEnqueueScope,
  refreshMode: ThumbnailCacheRefreshMode,
  signal: AbortSignal,
  yieldToBrowser: () => Promise<void>
): Promise<void> {
  if (refreshMode === "retryFailedAndStale") {
    // Revive the (usually few) failed/stale rows in scope: stop as soon as a
    // batch accepts nothing left to revive. Reads real per-batch stats; a
    // cursor walk would instead scan every file in scope just to confirm there
    // is nothing failed (and the windowed path returns zeroed stats).
    while (!signal.aborted) {
      const response = await client.enqueueThumbnailCache(
        {
          ...scope,
          refreshMode,
          limit: THUMBNAIL_CACHE_ENQUEUE_WINDOW_LIMIT
        },
        {
          requestPriority: "background",
          signal
        }
      );
      const nothingLeft =
        response.pendingCandidateCount === 0 && response.failedCount === 0;
      if (response.acceptedCount === 0 || nothingLeft) {
        break;
      }
      await yieldToBrowser();
    }
    return;
  }

  // Generate everything missing/stale in scope with a keyset cursor: each batch
  // scans one bounded window (Core returns `nextCursor`) and resumes after it,
  // so the whole scope costs one linear scan of short DB-writer jobs that
  // interleave with interactive writes. Terminate when the cursor runs out
  // (`nextCursor` null); the windowed path returns zeroed stats, so the
  // accept-count/pending signals are not available here.
  let afterFileId = 0;
  while (!signal.aborted) {
    const response = await client.enqueueThumbnailCache(
      {
        ...scope,
        refreshMode,
        limit: THUMBNAIL_CACHE_ENQUEUE_WINDOW_LIMIT,
        afterFileId
      },
      {
        requestPriority: "background",
        signal
      }
    );
    if (response.nextCursor === undefined || response.nextCursor === null) {
      break;
    }
    afterFileId = response.nextCursor;
    await yieldToBrowser();
  }
}
