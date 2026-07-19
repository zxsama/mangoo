import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  runThumbnailCacheEnqueueBatches,
  THUMBNAIL_CACHE_ENQUEUE_WINDOW_LIMIT,
  type ThumbnailCacheEnqueueClient
} from "../src/core/thumbnailCacheEnqueue.ts";

type EnqueueResponse = Awaited<
  ReturnType<ThumbnailCacheEnqueueClient["enqueueThumbnailCache"]>
>;

function response(partial: Partial<EnqueueResponse>): EnqueueResponse {
  return {
    cachedCount: 0,
    missingCount: 0,
    staleCount: 0,
    failedCount: 0,
    pendingCandidateCount: 0,
    totalBlobBytes: 0,
    activeBulkTaskCount: 0,
    acceptedCount: 0,
    nextCursor: null,
    ...partial
  } as unknown as EnqueueResponse;
}

const noYield = () => Promise.resolve();

function recordingClient(
  responses: EnqueueResponse[],
  maxCalls = 8
): { client: ThumbnailCacheEnqueueClient; inputs: Array<Record<string, unknown>> } {
  const inputs: Array<Record<string, unknown>> = [];
  const client: ThumbnailCacheEnqueueClient = {
    enqueueThumbnailCache: async (input) => {
      inputs.push(input as unknown as Record<string, unknown>);
      if (inputs.length > maxCalls) {
        throw new Error(
          `enqueue loop did not terminate after ${inputs.length} calls`
        );
      }
      const index = Math.min(inputs.length - 1, responses.length - 1);
      return responses[index];
    }
  };
  return { client, inputs };
}

describe("runThumbnailCacheEnqueueBatches", () => {
  it("walks staleOrMissing with an advancing keyset cursor, stopping when nextCursor is null", async () => {
    // Every batch reports work accepted + candidates still pending, so an
    // accept-count loop would never stop: termination MUST come from the cursor
    // running out. This is the regression guard for the add-root timeout — the
    // non-cursor loop re-scans the whole scope every batch (O(files²)) and pins
    // the single DB writer for 100+s on a large folder.
    const { client, inputs } = recordingClient([
      response({ acceptedCount: 256, pendingCandidateCount: 999, nextCursor: 100 }),
      response({ acceptedCount: 256, pendingCandidateCount: 999, nextCursor: 200 }),
      response({ acceptedCount: 256, pendingCandidateCount: 999, nextCursor: null })
    ]);
    const controller = new AbortController();

    await runThumbnailCacheEnqueueBatches(
      client,
      { rootId: 1, folderId: 7, includeDescendants: true },
      "staleOrMissing",
      controller.signal,
      noYield
    );

    assert.equal(inputs.length, 3, "should stop as soon as the cursor is exhausted");
    assert.deepEqual(
      inputs.map((input) => input.afterFileId),
      [0, 100, 200],
      "each batch must resume from the previous batch's nextCursor"
    );
    for (const input of inputs) {
      assert.equal(input.limit, THUMBNAIL_CACHE_ENQUEUE_WINDOW_LIMIT);
      assert.equal(input.refreshMode, "staleOrMissing");
    }
  });

  it("stops retryFailedAndStale on accept-count and does NOT drive a keyset cursor", async () => {
    // Retry looks for the (usually few) failed rows and must stop as soon as a
    // batch finds nothing left to revive — a cursor walk would scan every file
    // in scope just to confirm there is nothing failed.
    const { client, inputs } = recordingClient([
      response({ acceptedCount: 3, pendingCandidateCount: 3, failedCount: 3 }),
      response({ acceptedCount: 0, pendingCandidateCount: 0, failedCount: 0 })
    ]);
    const controller = new AbortController();

    await runThumbnailCacheEnqueueBatches(
      client,
      { rootId: 1, folderId: 7, includeDescendants: true },
      "retryFailedAndStale",
      controller.signal,
      noYield
    );

    assert.equal(inputs.length, 2, "should stop once a batch accepts nothing");
    for (const input of inputs) {
      assert.equal(
        input.afterFileId,
        undefined,
        "retry must not walk the scope with a keyset cursor"
      );
    }
  });

  it("does not start a batch once the signal is already aborted", async () => {
    const { client, inputs } = recordingClient([
      response({ acceptedCount: 256, pendingCandidateCount: 999, nextCursor: 100 })
    ]);
    const controller = new AbortController();
    controller.abort();

    await runThumbnailCacheEnqueueBatches(
      client,
      { rootId: 1 },
      "staleOrMissing",
      controller.signal,
      noYield
    );

    assert.equal(inputs.length, 0);
  });
});
