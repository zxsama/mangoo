# Megle Docs

## Current Decision

Megle will be built as:

```text
Electron desktop shell
React + TypeScript UI
Rust Core Service
SQLite WAL + FTS5
libvips / FFmpeg media pipeline
process-based plugin architecture
```

The current plan is component-based. We are not forking an existing image-management product.

## Read First

1. [Final Solution](final-solution.md)
   - Final product and technical direction.

2. [Component Library Review](component-library-review.md)
   - Component-level open source library selection.

3. [Performance Plan](performance-plan.md)
   - Performance targets and constraints for million-scale libraries.

4. [SQLite 1M Benchmark](performance-results/sqlite-1m-2026-05-16.md)
   - First Phase 0 performance result.

5. [SQLite 5M Benchmark](performance-results/sqlite-5m-2026-05-16.md)
   - 5M metadata and concurrent read/write performance result.

6. [Virtual Grid Benchmark](performance-results/virtual-grid-2026-05-16.md)
   - React/TanStack Virtual million-scale grid result and browser scroll-height limit.

7. [Thumbnail Pipeline Benchmark](performance-results/thumbnail-pipeline-2026-05-16.md)
   - Image/video thumbnail generation, 320px short-side rule, and concurrency guidance.

8. [Filesystem Scan Benchmark](performance-results/filesystem-scan-2026-05-16.md)
   - Existing directory traversal and first-pass SQLite insert performance.

9. [File Operation Consistency Benchmark](performance-results/file-ops-2026-05-16.md)
   - Rename/move/delete ordering, conflict logging, and DB/filesystem consistency.

10. [Preview Switch Benchmark](performance-results/preview-switch-2026-05-16.md)
   - Cached WebP preview switching, neighbor prefetch, and rapid key-repeat behavior.

11. [Raw Phase 0 Benchmark JSON](performance-results/raw/2026-05-16/README.md)
   - Archived raw JSON reports. Heavy generated databases, media samples, and dependency directories were deleted because they are reproducible.

12. [Architecture](architecture.md)
   - Detailed architecture, process model, data model, thumbnails, and file operations.

13. [Implementation Roadmap](implementation-roadmap.md)
   - Phased implementation plan.

14. [Project Structure](project-structure.md)
   - Monorepo directory ownership and anti-patterns.

15. [Testing Strategy](testing-strategy.md)
   - Fast checks, schema validation, and future test gates.

16. [UI Layered Acrylic Frosted Glass Design](superpowers/specs/2026-05-16-megle-ui-liquid-glass-design.md)
   - Approved UI direction, shell model, component rules, and performance constraints.

17. [UI Foundation Implementation Plan](superpowers/plans/2026-05-16-megle-ui-foundation.md)
   - Implementation plan for the shared app shell, frameless chrome, design tokens, and UI primitives.

18. [Complete Product Implementation Plan](superpowers/plans/2026-05-16-megle-complete-product-plan.md)
   - Master execution order that inserts the UI plan into the full product roadmap through release hardening.

19. [Unified Overlay UI Design](superpowers/specs/2026-07-27-unified-overlay-ui-design.md)
   - Shared overlay, dialog, context-menu, notification, and appearance rules.

20. [Unified Overlay UI Implementation Plan](superpowers/plans/2026-07-27-unified-overlay-ui.md)
   - Task-by-task implementation and verification plan.

21. [Unified Overlay UI Verification](benchmarks/2026-07-27-unified-overlay-verification.md)
   - Task 8 automated, controlled-fixture, native-window, appearance, DPI, and limitation evidence.

## Supporting

- [Product Brief](product-brief.md)
  - Product goals and boundaries.

- [Plugin And Web Roadmap](plugin-and-web-roadmap.md)
  - Plugin and future Web/Docker direction.

- [Open Questions](open-questions.md)
  - Remaining decisions and risk items.

## Archived Research

Historical research is archived under [archive](archive/). It is not the current implementation plan.

Archived documents:

- [Open Source Stack Review](archive/open-source-stack-review.md)
- [Open Source Products Review](archive/open-source-products-review.md)

## Current Work

Phase 10 (Release Hardening) is complete: the master plan in
[`superpowers/plans/2026-05-16-megle-complete-product-plan.md`](superpowers/plans/2026-05-16-megle-complete-product-plan.md)
has all ten tasks ticked, and the manual smoke-test list lives in
[`release-checklist.md`](release-checklist.md). Phases 1–10 ship the full product loop —
add a root, browse, preview, organize, file-ops, watcher, plugins, Web/Docker reuse, and a
demoable frameless desktop build.

A real-GUI test against a 31-image photo directory on 2026-05-17 surfaced and fixed three
integration bugs that the unit tests missed: the preload script needed to be CommonJS, the
thumbnail freshness check assumed metadata that `/api/media` does not include, and the
grid only rendered placeholder text instead of `<img>` elements. The Core API now exposes
`/media/{fileId}/thumbnail/blob` and the React grid + preview render real WebP bytes.
See [`release-checklist.md` §2.1](release-checklist.md) for the automated CDP-driven
smoke test that catches these issues end-to-end.

The approved UI direction is layered acrylic frosted glass with a shared desktop app shell. The
foundation track lives in `docs/superpowers/plans/2026-05-16-megle-ui-foundation.md`. The
full-product execution order lives in
`docs/superpowers/plans/2026-05-16-megle-complete-product-plan.md`.

Completed Phase 0 gates:

1. SQLite 1M/5M rows benchmark.
2. Virtual grid benchmark.
3. Thumbnail queue benchmark.
4. Preview switching benchmark.
5. Real file operation consistency benchmark.

Cleanup status:

- Small raw JSON reports are archived under [raw/2026-05-16](performance-results/raw/2026-05-16/README.md).
- Heavy generated benchmark artifacts were deleted and remain ignored by `.gitignore`.
