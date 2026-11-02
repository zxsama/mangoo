# Open-Issues Review Fixes Implementation Plan

> **For agentic workers:** Execute inline in the current worktree. Do not dispatch subagents, stage, or commit unless the user explicitly requests it.

**Goal:** Close the three remaining review blockers: unsafe PSD/RAW allocations, indefinitely reusable preview failure rows, and stale pending preview selections.

**Architecture:** Reuse the decoder module's existing pixel-budget policy as a shared pre-allocation guard, add a bounded freshness policy to the preview route's negative cache without changing thumbnail retry semantics or the schema, and bind pending Web preview selections to `useLibraryData`'s existing media-page generation.

**Tech Stack:** Rust, Axum, SQLite, React, TypeScript, Node test runner.

## Global Constraints

- Preserve every unrelated tracked and untracked worktree change.
- Add a failing regression test before each production change and observe the expected failure.
- Keep the existing Core HTTP, SQLite migration, and Web/Core-client boundaries unchanged.
- Do not weaken existing validators or remove existing coverage.
- Do not claim completion without fresh focused checks, `npm test`, `git diff --check`, and `cargo fmt --all --check`.

---

### Task 1: Shared Decoder Allocation Budget

**Files:**
- Modify: `crates/core/src/decode/mod.rs`
- Modify: `crates/core/src/decode/image_crate.rs`
- Modify: `crates/core/src/decode/psd.rs`
- Modify: `crates/core/src/decode/rawler.rs`

**Interfaces:**
- Produces: `ensure_decode_pixel_budget(decoder: &str, width: u64, height: u64) -> anyhow::Result<()>`
- Consumes: Rawler's `rawsource::RawSource`, `decode_dummy`, and `decoders::RawDecodeParams` APIs.

- [x] Add unit tests proving zero-sized and over-budget dimensions are rejected with a stable `source is too large to decode` message, while the maximum allowed dimensions pass.
- [x] Run `cargo test -p mangoo-core decode::tests::decode_pixel_budget -- --nocapture` and confirm the new tests fail because the shared guard does not exist.
- [x] Move `MAX_DECODE_PIXELS` and the checked multiplication into `decode/mod.rs`; make `image_crate` call the shared guard.
- [x] Call the guard in `PsdDecoder` after parsing dimensions and before `document.rgba()`.
- [x] In `RawlerDecoder`, memory-map once, call `decode_dummy` to obtain dimensions, reject an unsafe raster, then perform the full decode and guard again before development.
- [x] Run `cargo test -p mangoo-core decode:: -- --nocapture` and confirm all decoder tests pass.

### Task 2: Expiring Preview Negative Cache

**Files:**
- Modify: `crates/core/src/api/routes.rs`

**Interfaces:**
- Produces: `PREVIEW_FAILURE_CACHE_TTL_SECONDS: i64 = 300`
- Produces: a pure freshness predicate checking failure kind, source fingerprint, and `updated_at` against the current Unix timestamp.

- [x] Extend `preview_route_falls_back_to_original_bytes_when_generation_fails` so a fresh sentinel failure is reused but a row older than 300 seconds is regenerated and overwritten.
- [x] Run the exact route test and confirm the stale-row assertion fails because current code reuses failures indefinitely.
- [x] Add the freshness predicate and use it when filtering `cached_failure`; preserve successful-blob precedence and the existing fallback-to-original response.
- [x] Run the focused route test and the related preview route tests; confirm they pass.

### Task 3: Generation-Bound Pending Preview Selection

**Files:**
- Modify: `apps/web/src/core/useLibraryData.ts`
- Modify: `apps/web/src/app/previewMediaWindow.ts`
- Modify: `apps/web/src/app/App.tsx`
- Modify if required by typing: `apps/web/tests/fixtures/librarySwitcherLifecycleFixture.tsx`
- Modify: `apps/web/tests/previewMediaWindow.test.mts`

**Interfaces:**
- Produces: `LibraryState.mediaGeneration: number`
- Produces: `PendingPreviewSelection { index: number; mediaGeneration: number }`
- Produces: `resolvePendingPreviewSelection(...)` returning `idle`, `wait`, `discard`, or `apply`.

- [x] Add pure tests proving pending selection is discarded when preview is closed or the media generation changes, waits before the slot arrives, and applies only a matching-generation arrived slot.
- [x] Run `node --experimental-strip-types --test apps/web/tests/previewMediaWindow.test.mts` and confirm failure because the resolver does not exist.
- [x] Expose the existing `mediaPageGeneration.current` as `LibraryState.mediaGeneration`.
- [x] Replace the numeric pending ref in `App` with the generation-bound record; clear it synchronously on close and let the resolver gate the arrival effect.
- [x] Run the focused test, Web typecheck, and `npm run check:web`.

### Task 4: Handoff and Final Verification

**Files:**
- Modify: `docs/superpowers/handovers/2026-08-05-open-issues-handoff.md`

- [x] Add a dated follow-up section recording the three hardening fixes and their evidence without rewriting historical issue text.
- [x] Run `cargo fmt --all --check`.
- [x] Run `git diff --check`.
- [x] Run `npm test` and record any unrelated/manual-only blockers exactly.
- [x] Re-read only the touched diffs, confirm no unrelated files were modified by this task, and report the final verification results.
