# Cancel/Demote Thumbnail Tasks That Leave the Viewport Scope — Design

Date: 2026-07-09
Status: approved (design decisions confirmed with user)

## Problem

Thumbnail generation tasks that a user scrolls PAST keep running. When a tile
enters the selected/visible/ahead scope the client POSTs
`/api/tasks/thumbnail-priority-scope` and Core promotes its task. But when the
tile leaves that scope, `thumbnail_scope_priority_for_file` only demotes the task
to `THUMBNAIL_BACKGROUND_PRIORITY` (0), which is still **above** bulk (-10), so
the worker keeps generating thumbnails for files the user is no longer looking
at. On a 650k-file library this means the "active tasks" count stays huge and
the worker burns CPU on off-screen files.

User requirement (verbatim intent): "已经切换过去的、且还没有生成缩略图、没有在可视
和预加载范围内的文件就不要继续活动任务了" — files outside the viewport+ahead range
should stop being active work. **With one caveat**: don't cancel the instant a
tile leaves; delay a few seconds, because the user may scroll back and the tile
must keep showing / not have to regenerate.

## Approach

A **retention grace window** on the client + **cancel-on-leave** in Core.

### Client: retained-file-ids set

The client already keeps `recentVisibleThumbnailIdsRef` — a map of file id →
last-seen-visible timestamp, pruned after `RECENT_VISIBLE_THUMBNAIL_STICKY_MS`.
This IS the grace window. The priority-scope sync gains a `retainedFileIds`
field = union of:
- the current selected + visible + ahead ids, AND
- every id in `recentVisibleThumbnailIdsRef` still within the sticky window.

A tile that just scrolled out is still in `recentVisibleThumbnailIdsRef` for a
few seconds → still in `retainedFileIds` → NOT cancelled. Scroll back within the
window and it stays. Only after the sticky window elapses does it drop out of
`retainedFileIds`, and the next scope sync tells Core it may be cancelled. This
delivers the "delay a few seconds, survive scroll-back" requirement with the
existing mechanism — no new timer.

The request also carries the current `generationMode` so Core knows whether to
cancel (renderedOnly/cacheOnly) or demote-to-bulk (allPersist).

### Core: cancel or demote leavers

In `sync_thumbnail_priority_scope_with_promotions`, the demotion-candidate query
already finds every pending/running thumbnail task elevated above background in
the root. For each such task whose file is:
- **in the new selected/visible/ahead scope** → re-rank as today (promote/demote
  within the elevated tiers).
- **NOT in scope but IN `retainedFileIds`** → demote to
  `THUMBNAIL_BACKGROUND_PRIORITY` (today's behavior — kept warm during grace).
- **NOT in scope and NOT in `retainedFileIds`** (grace elapsed):
  - `generationMode == allPersist` → demote to `THUMBNAIL_BULK_PRIORITY` (-10):
    the user wants the whole library, so it keeps generating at the lowest
    priority behind everything else.
  - otherwise (renderedOnly/cacheOnly) → **cancel** the task if `pending`
    (set status `cancelled`); leave a `running` task alone (it is already
    decoding and finishes on its own — cheaper than aborting mid-decode). The
    `thumbs` row stays `queued`; a later viewport request re-drives it and the
    cache-status query treats queued-without-a-task as `missing`, so nothing is
    lost.

Running tasks are never cancelled (bounded, self-completing). Only pending ones.

### Why retention lives on the client, not a Core timer

Core has no clock tied to the user's scroll; the client already tracks
recency. Passing `retainedFileIds` keeps Core stateless per-request (it just
compares sets) and makes the grace window a single client constant
(`RECENT_VISIBLE_THUMBNAIL_STICKY_MS`), consistent with how the sticky set
already gates seen-persist.

## Contract changes

`ThumbnailPriorityScopeSyncRequest` gains:
- `retainedFileIds: number[]` — files to keep warm (not cancel) even if
  currently out of scope. Optional on the wire (absent ⇒ empty ⇒ old behavior).
- `generationMode: "renderedOnly" | "allPersist" | "cacheOnly"` — optional;
  absent ⇒ treated as `allPersist` (never cancels — the safe default that
  preserves pre-change behavior for any caller that doesn't send it).

## Files

- `contracts/core-api/openapi.yaml` — extend `ThumbnailPriorityScopeSyncRequest`.
- `packages/core-client/src/generated-contract.ts` + `index.ts` — mirror.
- `crates/core/src/api/routes.rs` — parse the two new fields, thread to Core.
- `crates/core/src/db/mod.rs` —
  `sync_thumbnail_priority_scope_with_promotions` gains `retained_file_ids` +
  `generation_mode`; the demotion loop cancels/demotes leavers per the rules.
- `apps/web/src/core/useLibraryData.ts` — compute `retainedFileIds` from the
  sticky set; send it + `generationMode` in every scope sync.
- Tests: Rust unit test (leaver cancelled in renderedOnly, demoted-to-bulk in
  allPersist, retained kept at background, running untouched); update the
  existing priority-scope route/db tests + `validate-core-api` markers.

## Non-goals

- No change to how tiles fetch/render. This only governs task lifetime.
- No new background timer. The grace window is the existing sticky set.
