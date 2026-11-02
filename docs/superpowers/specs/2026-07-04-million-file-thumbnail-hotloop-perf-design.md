# Million-file thumbnail hot-loop performance: 100-operation real-usage test + stutter fixes

- Status: approved (brainstorming), pending implementation plan
- Date: 2026-07-04
- Scope: Desktop-first. The Web/Docker headless deployment line is explicitly out of scope for now; `apps/web` React code IS in scope because it is the same UI the Electron desktop shell renders.

## Problem

On million-file libraries, real-time thumbnail refresh and DB persistence of thumbnails cause interaction stutter. The user-visible goals are:

1. Thumbnails in the user's visible area refresh as fast as possible.
2. A clicked/opened file displays as fast as possible.
3. Scrolling, switching files, and deep-scroll never stutter; the UI stays fluid at all times.
4. Background thumbnail ingestion into the DB is invisible to the user.

The existing real test (`tools/dev/real-load-test.mts`) verifies scan/priority ordering and blob correctness against a real directory, but it does NOT exercise sustained (100-operation) interaction on a large library, and does not enforce hard latency budgets. We need a new 100-operation real-usage test that reproduces, quantifies, and guards against stutter / wrong-refresh / never-displayed thumbnails, then fix what it surfaces.

## Data source

Real image directory (not synthetic seed): `Y:\Repository\Billfish\Artists`, ~500k+ files, deeply nested. Overridable via `MANGOO_REAL_LOAD_ROOT`. Because it is genuinely million-scale, no synthetic row top-up is needed — the real scan produces real deep-scroll depth and real write-lock contention.

## Verified hot-path map (from code)

- Worker (`crates/core/src/tasks.rs`): per-bucket Tokio semaphores selected=1, foreground=12, background=2. Each thumbnail task runs on `spawn_blocking` with its own reopened DB connection.
- Blob publish (`crates/core/src/db/mod.rs` `publish_thumbnail_and_succeed_task_if_source_fingerprint_and_task_attempt_current`, ~4029): one `BEGIN IMMEDIATE` folds blob + state + media dims + task-succeeded, single commit per thumbnail.
- Priority-scope sync (`sync_thumbnail_priority_scope_with_promotions`, ~3115): the per-scroll WRITE; `BEGIN IMMEDIATE`, scoped to viewport file_ids + already-elevated tasks; runs on the SHARED write mutex (`run_shared_database`, `api/routes.rs` ~1071).
- Reads (`get_thumb_blob` ~4090, `list_media_page` ~2094) run on a per-thread cached read connection (`run_read_database`), off the write mutex.
- DB engine: WAL + `synchronous=NORMAL` + 64MiB cache + 256MiB mmap + `temp_store=MEMORY` (`configure_file_database_wal` ~6990).
- Web: `MediaGrid.tsx` `ReadyThumbnail` (~1982) does loading→ready blob fetch, object URL keyed by `id:mtime:size`; `shouldRefreshThumbnailState` (~1744) governs re-polling (visible 150ms / ahead 1000ms). `useLibraryData.ts` `scheduleThumbnailPriorityScopeSync` (~607) coalesces scope POSTs (120ms leading+trailing); seen-persist enqueues bulk cache tasks ~every 500ms. `compactMediaSlots` (~1542) re-sorts all loaded slots on change.

## Architecture: two harnesses + one fix loop

### Test A — API-layer "100 operations" test (primary, CI-able, hard budgets)

New file `tools/dev/real-hot-loop-test.mts`, reusing the process lifecycle + HTTP helpers from `real-load-test.mts`.

1. Spawn real Core (`cargo run --release -p mangoo-core`) against a fresh temp `MANGOO_DB_PATH`.
2. `addRoot` → `Y:\Repository\Billfish\Artists`. Wait until enough media is indexed for deep pagination (do not require full scan completion).
3. Run the 100 operations WHILE background scan + thumbnail generation is still active (worst case: browsing while ingesting). Mid-run, trigger one `enqueueThumbnailCache` generate-all to create the write-lock storm.
4. The 100 operations are a weighted real-interaction sequence, each timed:
   - Deep-scroll pagination: `list_media` windows at increasing offsets (0 → near tail).
   - Real subfolder switching.
   - Rolling `POST /api/tasks/thumbnail-priority-scope` (selected/visible/ahead scrolling windows).
   - Visible-window thumbnail state polling.
   - Open preview: fetch original bytes for the selected item.
   - Thumbnail blob fetch for ready items.
5. Record P95/max latency per operation category.

Hard perf budgets (test FAILS if exceeded). First run measures the real baseline; budgets are then set from the baseline and become the regression line:
- priority-scope sync P95 (under generate-all load) — the anti-stutter guarantee.
- `list_media` window P95 at deep offset.
- folder-switch first-media P95.
- preview header load P95.
- visible thumbnail first-ready P95.

Correctness assertions (test FAILS on violation):
- Every visible item eventually reaches a terminal thumbnail state (no permanent pending/loading).
- Blob bytes are valid WebP (`RIFF`/`WEBP` header) and byte-size matches the reported state.
- No wrong refresh: returned `fileId`/`target` match the request.
- Unrequested background folders stay deferred (not ready before request).
- Priority ordering: selected first-clear ≤ visible ≤ ahead.

### Test B — Desktop CDP scroll-trace smoke (complementary, manual/opt-in)

New file `tools/dev/desktop-scroll-trace.mjs`, reusing `cdp-inspect.mjs` wiring.

1. Launch `run-dev.mjs` with `MANGOO_REMOTE_DEBUG=1` and `MANGOO_AUTO_ADD_ROOT` = the real dir.
2. Attach CDP to the renderer, run a scripted scroll sequence, capture a performance trace.
3. Assert render-thread health that Test A cannot see: no main-thread long task over budget during scroll, tile counts resolve (ready grows; no permanent loading/failed), no console errors, no wrong-image flicker.

Division of labor: Test A is the hard-assertion number test; Test B is the desktop render-jank smoke.

### Fix loop

Systematic-debugging: run baseline → the test exposes over-budget/bugs → targeted fix → re-measure until green. The fixes below are prior hypotheses that the test will confirm or refute; do not implement blind — measure first.

Prior hypotheses, by expected impact:
1. Write-lock contention (Rust, most critical). 14 workers each `BEGIN IMMEDIATE` per thumbnail; under WAL's single writer, the scroll's priority-scope write starves. Direction: group-commit blobs / serialize blob writes through a batched path + `busy_timeout`, leaving WAL write windows for scroll writes; keep the priority-scope transaction minimal.
2. `ReadyThumbnail` stuck-loading / wrong-refresh (Web): effect dependency on `thumbnailUpdatedAt` + `canLoadCurrentThumbnailBlob` has a timing hole when the media list omits per-row thumbnail metadata; a tile can fail to fetch the blob and stay in `loading`.
3. `compactMediaSlots` O(n log n) full re-sort on every slots change (Web): deep-scroll main-thread jank.
4. seen-persist every 500ms amplifies write pressure: tune cadence / confirm background priority so it does not contend.

## Non-goals

- Web/Docker headless deployment fixes.
- Synthetic million-row seeding (real directory is already million-scale).
- Rewriting the scan pipeline; only touch thumbnail refresh + persistence hot paths.

## Risks / notes

- First `cargo run --release` build and the 500k+ scan are slow; the test needs generous timeouts and should start operations before full-scan completion.
- Budgets must be measured on the target machine first; hard-coding foreign numbers would produce false failures.
- Whether Test A joins `npm test` is deferred to the plan (it needs the real drive `Y:` present, so likely an opt-in script, not the default gate).
