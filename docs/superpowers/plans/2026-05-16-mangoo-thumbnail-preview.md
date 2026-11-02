# Mangoo Thumbnail And Preview Plan

> **Status:** Implemented prior to the introduction of per-phase plan files for Phase 3, then expanded by `2026-05-16-mangoo-advanced-media.md` for real image and video decoding in Phase 7.
>
> Coverage today:
>
> - `crates/core/src/thumbnails/mod.rs` (real image decode + WebP encode + ffmpeg poster)
> - `crates/core/src/scan/mod.rs` (header-only image dimension probe)
> - `crates/core/src/tasks.rs` (worker branches on media kind, ffmpeg availability)
> - `apps/web/src/features/preview/PreviewPanel.tsx` (preview surface)
> - `apps/web/src/features/media-grid/MediaGrid.tsx` (grid loading states)

**Goal (recap):** tiny / grid / preview thumbnail pipeline, viewport-priority scheduling, tile loading states, preview transitions, neighbor prefetch.

**Constraints kept:**

- glass on control layers only
- dark, stable content surfaces
- no layout shift on tile load
- no expensive blur over the grid

**Reached at commits:**

- `c82e30a` feat: add thumbnail state contract
- `8eda2be` feat: add thumbnail worker and preview foundation
- `f3536dd` feat(core): real image and video thumbnails (phase 7)

See those commits and `2026-05-16-mangoo-advanced-media.md` for the Phase 7 expansion that wired real decoding.
