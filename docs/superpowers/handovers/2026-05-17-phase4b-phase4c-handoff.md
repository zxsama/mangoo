# Mangoo Phase 4B/4C Handoff

Date: 2026-05-17
Workspace: `D:/Mangoo`
Branch: `codex/phase1-foundation`

## Status (updated 2026-05-17 by Claude Code)

Phase 4B and Phase 4C are now complete and committed. The three blockers below are all resolved with regression tests, and the Task Center UI is shipped. `npm test` passes end-to-end (103 Rust tests, structure/contract/schema/web/desktop/core-client checks all green). The remainder of this document is preserved as historical context for the original Codex handoff.

## Original Stop Condition

The user explicitly asked to stop further implementation and write this handoff document.
The backend implementer agent `019e3521-0631-7342-b8ea-4e2f13dcb016` was closed while running. Its previous status was `running`, so do not assume the last requested quality fixes were completed.

## Operating Rules To Preserve

- Read `D:/Mangoo/.codex/memory.md` before continuing.
- Main conversation/controller is for direction, decomposition, and review.
- Code modifications and verification/test command execution should be delegated to subagents.
- Keep concurrency low because repeated 429 errors occurred. Prefer one writer agent at a time until limits stabilize.
- `.codex/` is local memory and should stay untracked/ignored.
- Do not revert user or unrelated changes.
- Use git checkpoints when a phase is genuinely clean.

## Git/Workspace State At Handoff

Last observed `git status --short --branch`:

```text
## codex/phase1-foundation
 M Cargo.lock
 M crates/core/Cargo.toml
 M crates/core/migrations/0001_initial.sql
 M crates/core/src/api/mod.rs
 M crates/core/src/api/routes.rs
 M crates/core/src/db/migrations.rs
 M crates/core/src/db/mod.rs
 M crates/core/src/main.rs
 M crates/core/src/scan/mod.rs
?? CLAUDE.md
?? crates/core/migrations/0009_scan_reconciliation.sql
?? crates/core/src/watch/
?? docs/superpowers/plans/2026-05-16-mangoo-task-center-and-watcher.md
```

Notes:

- `CLAUDE.md` is untracked and was not created or edited by the controller during this stop. Treat it as user/foreign state until confirmed.
- `docs/superpowers/handovers/2026-05-17-phase4b-phase4c-handoff.md` is this handoff file and will be untracked/modified after creation.
- No commit was made after Phase 4A. Phase 4B work is dirty and not approved yet.

## Last Stable Commit Context

Latest completed checkpoints before this dirty work:

- `0f23d07` `feat: complete phase 2 browsing workbench`
- `c82e30a` `feat: add thumbnail state contract`
- `8eda2be` `feat: add thumbnail worker and preview foundation`
- `f12f37d` `feat: harden task scheduler`

Phase 1 through Phase 4A are committed and verified. Phase 4B is in progress. Phase 4C has not been completed.

## Phase 4B Current Implementation Snapshot

Implemented or partially implemented backend changes:

- Added watcher dependency `notify = "8.2.0"` in `crates/core/Cargo.toml`.
- Added `crates/core/src/watch/mod.rs`.
- Wired `mod watch;` in `crates/core/src/main.rs`.
- Wired watcher startup in `AppState::new_with_worker` in `crates/core/src/api/mod.rs`.
- Retained watcher handle in `AppState` with shutdown/join on drop.
- Added watcher tests covering add/delete/rename/non-media/disabled roots/rescan events.
- Added scan reconciliation fields:
  - edited `crates/core/migrations/0001_initial.sql`
  - added `crates/core/migrations/0009_scan_reconciliation.sql`
  - added `SCAN_RECONCILIATION_MIGRATION` in `crates/core/src/db/migrations.rs`
- Scan reconciliation moved toward root `active_scan_generation` so watcher upserts during active scan can inherit the active generation.
- Directory create events were changed to queue a root scan instead of walking large subtrees inline.
- `queue_root_rescan` was changed from `blocking_send` to `try_send`.

## Phase 4B Verification Already Reported By Agents

Before the final interruption, agents reported:

- `cargo test -p mangoo-core watch:: -- --nocapture`: 10 passed
- `cargo test -p mangoo-core scan`: 30 passed
- `cargo test -p mangoo-core db`: 43 passed
- `cargo test -p mangoo-core tasks`: 19 passed
- `npm run check:schema`: PASS
- `npm run check:core-api`: PASS
- `npm run check:rust`: 99 passed
- `git diff --check`: exit 0, CRLF warnings only

However, after that, code quality review found new blockers and the follow-up fix was interrupted. Treat the current dirty state as unapproved.

## Phase 4B Review Status

Spec compliance eventually passed after multiple loops. The last spec review found no spec issues and verified:

- notify rescan/overflow events handled through `event.need_rescan()`
- one-path rename-from handled as remove
- added/deleted/renamed/non-media/disabled-root behavior
- full scan reconciliation for delete/move-out recovery
- startup wiring for file-backed DBs

Code quality review did not approve. The latest unaddressed findings are below.

## Outstanding Blockers For Next Backend Worker

These were sent to the backend implementer, then the agent was closed while running. Verify current code before assuming anything changed.

1. Directory create events can be dropped while a root scan is running.

Current risk:

- `handle_create_path` queues a rescan for directories.
- `queue_root_rescan` still checks `has_active_root_scan_task`.
- `has_active_root_scan_task` includes both `pending` and `running`.
- A directory created after `WalkDir` has passed that path during a running scan may be missed, because no follow-up pending scan is persisted.

Expected fix direction:

- Coalesce only duplicate pending root scans.
- Allow one pending follow-up scan while another scan is running.
- Add a regression test for directory create/move during an active scan.

2. Scan reconciliation is not guarded by task attempt.

Current risk:

- `scan_root_with_options` calls `reconcile_root_scan_completion` before `mark_root_scanned_for_task_attempt`.
- DB reconciliation checks root/generation but not `task_id` and `attempt_generation`.
- A stale/cancelled/retried attempt could reconcile missing rows or clear `active_scan_generation`.

Expected fix direction:

- Add a task-attempt guarded reconciliation helper.
- When `TaskAttemptGuard` exists, reconciliation and root-scanned marking should verify task id + attempt generation in the same transaction or equivalent atomic boundary.
- Add a stale-attempt regression test.

3. `try_send` can strand persisted pending root-scan tasks.

Current risk:

- `try_send` avoids watcher shutdown hangs.
- If the queue is full, the task remains pending in DB with no live worker wakeup.
- Current worker only loads pending DB tasks at startup and otherwise waits for received IDs.

Expected fix direction:

- Add a scheduler drain after each worker task finishes, or another live wakeup mechanism.
- The worker should query pending root_scan/thumbnail tasks in scheduler order so persisted tasks without queue messages are eventually processed.
- Add a focused test if practical.

## Phase 4B Suggested Next Agent Prompt

Dispatch one backend worker, no UI edits:

```text
Continue Phase 4B from current dirty workspace in D:/Mangoo. Code quality review found three blockers after the last reported verification. Backend files only, no UI edits, no commit.

Fix:
1. Running root_scan must not suppress a pending follow-up scan for directory create/move events. Coalesce duplicate pending scans, but allow one pending scan while a scan is running.
2. Root scan reconciliation must be guarded by task attempt when TaskAttemptGuard is present. Stale/cancelled/retried attempts must not reconcile missing rows or clear active_scan_generation.
3. Pending tasks persisted when watcher try_send returns Full/Closed must not be stranded. Add live scheduler draining after worker tasks or equivalent.

Add regression tests for each. Run:
- cargo test -p mangoo-core watch:: -- --nocapture
- cargo test -p mangoo-core scan
- cargo test -p mangoo-core db
- cargo test -p mangoo-core tasks
- npm run check:schema
- npm run check:core-api
- npm run check:rust
- git diff --check

Return DONE / DONE_WITH_CONCERNS / BLOCKED with exact changed files and results.
```

After worker returns DONE, run:

1. Code quality review subagent again.
2. Spec compliance spot review only if the fix changes watcher/scan semantics.
3. Final backend verification subagent.

## Phase 4C Status

Phase 4C task-center UI has not been completed.

The first UI worker `019e3521-0686-7111-bb7e-75ad37989e4f` failed with 429 Too Many Requests before producing changes. Current UI files appear unchanged from pre-dispatch except existing Phase 2/3 task panel work.

Known current UI surface:

- `apps/web/src/app/App.tsx` has a Tasks tab with a simple table.
- `apps/web/src/features/tasks/TaskPanel.tsx` has compact summary rendering.
- `apps/web/src/core/useLibraryData.ts` polls tasks and exposes `tasks`/`scanActive`.
- `@mangoo/core-client` already exposes `listTasks`, `cancelTask`, and `retryTask`.

Do not start Phase 4C until Phase 4B is approved, unless the user explicitly asks to parallelize again.

## Phase 4C Suggested Agent Prompt

```text
Implement Phase 4C Task Center UI in D:/Mangoo after Phase 4B backend is approved. Own only apps/web/src/app/App.tsx, apps/web/src/features/tasks/**, apps/web/src/core/useLibraryData.ts, apps/web/src/styles.css, and narrow web-only helpers.

Build a real task center and task drawer/panel:
- render every TaskStatus explicitly: pending, running, succeeded, failed, cancelled
- cancel only pending/running tasks
- retry only failed/cancelled tasks
- show progress, failures, and recoverability without text overflow
- use core-client methods through web hooks; no raw fetch
- polling/refresh should reload tasks and library after scan/index completion

Verify:
- npm run check:web
- npm run check:core-client
- npm --workspace @mangoo/web run typecheck
- npm --workspace @mangoo/web run build
- npm test if backend is stable

Do not edit crates/core/** or contracts/** unless a true type-contract mismatch forces it.
```

## Final Integration Criteria Before Commit

Do not commit Phase 4B/4C until all are true:

- Phase 4B backend quality review approved.
- Phase 4B spec review has no new findings after quality fixes.
- Phase 4C UI implementation reviewed for spec and quality.
- Final verification passes:
  - `npm test`
  - `npm run check:schema`
  - `npm run check:core-api`
  - `npm run check:rust`
  - relevant web build/typecheck commands
  - `git diff --check`
- Dirty/untracked files are reviewed, especially `CLAUDE.md`.

## Useful Agent IDs

- Backend implementer: `019e3521-0631-7342-b8ea-4e2f13dcb016` (closed while running at user stop request)
- Failed UI implementer: `019e3521-0686-7111-bb7e-75ad37989e4f` (429)
- Spec reviewers:
  - `019e3553-9d80-79e3-ab6b-ceb73424f13e`
  - `019e3564-91a7-7950-946e-e76dd237b317`
  - `019e356e-28ea-76d1-9b19-a6f8c796d745`
- Quality reviewers:
  - `019e3572-de1a-74a2-a1fd-3a2e9661dc5d`
  - `019e3580-bbca-77a1-b05a-610b93716e5b`

