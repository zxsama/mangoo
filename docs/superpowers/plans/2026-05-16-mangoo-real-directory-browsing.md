# Mangoo Real Directory Browsing Plan

> **Status:** Implemented prior to the introduction of per-phase plan files. Coverage:
>
> - `apps/web/src/features/library/LibrarySidebar.tsx`, `LibraryView.tsx`, `MediaGrid.tsx` (UI)
> - `crates/core/src/scan/mod.rs`, `crates/core/src/db/mod.rs`, `crates/core/src/api/routes.rs` (Core)
>
> The watcher and scan-hardening follow-ups live in `2026-05-16-mangoo-task-center-and-watcher.md`. This file is preserved so the master plan's reference resolves.

**Goal (recap):** Add root, scan root, populate SQLite, show real folder tree, show media grid, open preview entry point, all inside the shared app shell.

**Acceptance:** A user adds a real root and browses a real directory in the designed UI.

**Reached at commits:**

- `99ae0dd` chore: establish phase 1 foundation
- `96740ca` feat: harden root scan task pipeline
- `0f23d07` feat: complete phase 2 browsing workbench

See those commits and the watcher plan for the followups that closed this task.
