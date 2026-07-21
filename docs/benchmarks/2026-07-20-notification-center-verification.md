# Unified Notification Center Verification

Date: 2026-07-21 (Asia/Shanghai)

## Scope and Revisions

- Verified Task 1-15 implementation range: `c2cb5d95d2e040b6951b8522ea0a1707d8df0674..dcc83bb9108fe53c98bbdb13f5292621d6ad6b80`.
- Task 16 verification commit: the commit containing this report, with subject `test: verify unified notification center`.
- Starting branch state: `codex/unified-notification-center`, clean at `dcc83bb9108fe53c98bbdb13f5292621d6ad6b80`.
- Environment: Windows, Node `v22.14.0`, npm `10.9.2`, rustc `1.95.0`, cargo `1.95.0`, Python `3.11.9`.

Task 16 found one real Task 1-15 defect. The Settings renderer used the shared library hook and attempted the Main-only `megle:notifications-thumbnail-summary` IPC. The promise was caught in Web, but Electron still emitted an IPC access-denied error to stderr. `apps/web/src/core/notificationReporting.ts` now skips thumbnail-summary publication for an explicit non-Main `windowRole`, with a red-green regression in `apps/web/tests/notificationModel.test.mts`. No validator or visual baseline was weakened.

## Automated Gates

| Command | Result | Evidence summary |
| --- | --- | --- |
| `npm run check:core-api` | PASS | Core API contract alignment. |
| `npm run check:core-client` | PASS | Contract/typecheck and 41/41 tests. |
| `npm run check:schema` | PASS | SQLite migrations/schema/index checks. |
| `npm run check:rust` | PASS | 387/387 Rust tests. Existing `jpeg-decoder` profile warning remains. |
| `npm run check:desktop` | PASS | Desktop boundary/typecheck and 156/156 tests. |
| `npm run check:web` | PASS | Initial 121/121; after the real defect fix, 122/122. |
| `npm run check:ui-design` | PASS | Liquid Glass design boundaries. |
| `node tools/bench/notification-center-bench.mjs` | PASS | All required store/worker/retention invariants. |
| `npm test` | PASS | Two complete repository gates, both exit 0 (61.9 s and 63.4 s). |

The focused commands were run individually before the final aggregate gate. The Web and UI design gates were rerun after the role-guard defect fix.

## Store/Worker Benchmark

The benchmark imports the production `NotificationStore`, passes the production `notification-store-worker.ts` URL, crosses a real `worker_threads` boundary, and writes a real temporary SQLite WAL database. It does not implement a parallel fake persistence or retention algorithm. No machine-specific elapsed-time threshold is asserted.

Final recorded seed and counts:

| Metric | Value |
| --- | ---: |
| Libraries | 4 |
| Thumbnail completions / summary updates | 10,000 |
| Rapid progress updates | 10,000 |
| Completed operations / terminal events | 2,100 |
| Active operations | 20 |
| Active operation-seconds | 200 |
| Notification events sent through the worker | 12,100 |
| Store write requests | 10,006 |
| Total store requests | 10,008 |
| Worker startup | 66.083 ms |
| Thumbnail summary writes | 580.591 ms |
| Progress batch | 78.610 ms |
| Terminal batches | 19.641 ms |
| Retention | 1.612 ms |
| Total store/worker elapsed | 794.978 ms |

Final database assertions:

- Per-thumbnail notification rows: `0`.
- Thumbnail aggregate rows: `4`, not greater than the 4-library count.
- Persisted active progress rows: `20`, within the 200 operation-second budget (and therefore within `activeOperationSeconds + terminalEventCount`).
- Completed history rows after retention: `2,000`.
- Seeded active rows retained: `20/20`.
- Retention deleted `100` overflow rows and `0` non-expired rows.
- Evidence: repository-relative `.tmp/notification-center-bench/latest.json`; absolute `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\notification-center-bench\latest.json`.

## OS Backdrop Harness

Command:

```powershell
$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs; Remove-Item Env:\MEGLE_VISUAL_OS_BACKDROP
```

The command was run twice, preserving the Node exit code while always removing the environment variable.

**Overall harness status: FAIL (exit 1).** This is not reported as a pass. The second run still failed on the existing non-notification baseline: aborted network requests, existing Cargo startup warnings, OS backdrop pixel thresholds, main titlebar hover/drag/pointer-highlight checks, preview geometry/mode checks, and shared pointer-highlight checks. The repository's old visual baseline was not modified.

**Notification-specific status: PASS.** The summary contains no notification-specific hard failure. Evidence confirms:

- One icon-only notification titlebar control and no legacy Tasks/Recent controls.
- Compact popover anchoring before maximize, while maximized, and after restore.
- One full notification window target reused by repeated open calls.
- Three full-window pages, status as default, five history filters, and bounded rows.
- Shared secondary-window material, a `1px` titlebar outline, and `0px` pane divider.
- Native secondary-window reveal completed with `rendererReady=true`, `windowVisible=true`, and `revealFallback=false`.
- Process-log source/level filters and pause, follow, copy, clear, and export controls.
- Full/empty/degraded/narrow compact fixtures and status/history/log/degraded full-window fixtures rendered.

The first OS run exposed the Settings-to-Main-only thumbnail-summary IPC defect. After the fix, the second run's startup problems no longer contained any notification IPC denial; only the pre-existing Cargo warnings remained.

Primary visual evidence:

| Evidence | Repository-relative path | Absolute path |
| --- | --- | --- |
| Harness summary | `.tmp/visual-check/logs/desktop-ui-regression-summary.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\logs\desktop-ui-regression-summary.json` |
| Real notification window | `.tmp/visual-check/screenshots/ui-notifications-secondary-window.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\screenshots\ui-notifications-secondary-window.png` |
| Real process-log page | `.tmp/visual-check/screenshots/ui-notifications-secondary-window-logs.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\screenshots\ui-notifications-secondary-window-logs.png` |
| Compact active fixture | `.tmp/visual-check/screenshots/ui-notification-compact-fixture-full.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\screenshots\ui-notification-compact-fixture-full.png` |
| Full status fixture | `.tmp/visual-check/screenshots/ui-notification-window-fixture-status.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\screenshots\ui-notification-window-fixture-status.png` |
| Full history fixture | `.tmp/visual-check/screenshots/ui-notification-window-fixture-history.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\screenshots\ui-notification-window-fixture-history.png` |
| Full logs fixture | `.tmp/visual-check/screenshots/ui-notification-window-fixture-logs.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\screenshots\ui-notification-window-fixture-logs.png` |

## Real Electron Interaction

All destructive scenarios used temporary Core databases and temporary files. `Y:\Repository\Billfish\Artists` was scanned read-only; no source file under Artists was renamed, moved, or deleted.

| Scenario | Status | Actual evidence |
| --- | --- | --- |
| Root scan aggregation | VERIFIED | An isolated Electron profile and Core DB showed one active `task:1` / `root_scan` operation in the Desktop compact snapshot while Artists was scanning. The popover displayed one running Scan library item. Raw global SQLite contained one running root-scan operation row. |
| Layout/scroll/resize during active scan | VERIFIED WITH LIMITS | Adaptive, waterfall, grid, and list switched in 288-348 ms; deep scroll and resize to 1180x760 completed with no console errors. API resource-entry delta was recorded as 17, not gated by a machine threshold. The early scan had no media tiles, so preview was not exercised in this isolated run. |
| Thumbnail aggregate/no per-item | VERIFIED | The isolated global DB contained one `thumbnail_summaries` row and no thumbnail operation. The 10,000-item benchmark independently proved zero per-item rows and bounded aggregates. |
| Remove root terminal/restart recovery | VERIFIED WITH LIMIT | A safe temporary index of Artists was removed, never the source directory. The first process was terminated immediately after observing pending `root_purge`; `interrupted=true`. After restart, the same `task:2` reached `succeeded`, `currentValue=7550`, `totalValue=7550`, and remained in terminal results. Running determinate intermediate samples were not captured because the purge completed between 50 ms polls. |
| Move/rename/delete grouping | VERIFIED | A temporary root under `.tmp/task16-fsops-root` used copied PNGs only. Core persisted 5 item audit rows but 3 group IDs: rename 1, move 2, delete-permanent 2. Desktop global SQLite contained exactly 3 matching notification rows, with operation IDs equal to the group IDs and final progress 1/1, 2/2, and 2/2. |
| Library switch and restart history | VERIFIED WITH LIMIT | A second temporary library was created and became active. After a real switch and a second Electron start, global SQLite still contained all first-library scan/file-operation history with `library_name=Default`. The driver itself exited 1 because it incorrectly expected the compact latest-three IDs to remain exact; a newly recorded renderer error legitimately displaced the oldest compact result, but no history row was lost. |
| Broad error dedupe/no banner | PARTIAL | One real `renderer_error` row was observed after a non-blocking 404 and no center error strip appeared in the Artists interaction snapshots. A controlled duplicate error was not safely injected through the real renderer, so real-window deduplication is not claimed. Automated Desktop/Web tests cover dedupe and broad-banner removal. |
| Process logs | VERIFIED WITH LIMIT | Real notification-window controls for source, level, pause, follow, copy, clear, and export were exercised by the OS harness. The controlled fixture verified buffered gaps and subscription cleanup. Simultaneous visible lines from all Main/Core/Renderer sources were not independently observed, so that exact content mix remains unverified. |
| Browse/scroll/preview under background work | MIXED / OVERALL FAIL | The existing Artists desktop interaction harness completed 52 operations and all 30 folder switches. Five previews succeeded (`p50=246 ms`, `max=3389 ms`), but one preview was not ready after 5156 ms and 648 existing `net::ERR_FILE_NOT_FOUND` console errors were recorded. This pass is not reported as clean or as proof of no visible stalls/request amplification. |

Interaction evidence:

| Evidence | Repository-relative path | Absolute path |
| --- | --- | --- |
| Active scan/layout summary | `.tmp/task16-notification-1784641021852/summary.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-notification-1784641021852\summary.json` |
| Active scan popover | `.tmp/task16-notification-1784641021852/active-root-scan-popover.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-notification-1784641021852\active-root-scan-popover.png` |
| Root purge restart summary | `.tmp/task16-notification-1784641021852/root-purge-restart/summary.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-notification-1784641021852\root-purge-restart\summary.json` |
| File-op Core DB | `.tmp/task16-fsops-1784641292966/library.sqlite` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-fsops-1784641292966\library.sqlite` |
| File-op global notification DB | `.tmp/task16-fsops-1784641292966/electron-user-data/notifications.sqlite` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-fsops-1784641292966\electron-user-data\notifications.sqlite` |
| Library registry after switch | `.tmp/task16-fsops-1784641292966/electron-user-data/libraries.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-fsops-1784641292966\electron-user-data\libraries.json` |
| Artists interaction summary | `.tmp/interaction-test/summary.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\interaction-test\summary.json` |
| Artists final screenshot | `.tmp/interaction-test/shots/99-final.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\interaction-test\shots\99-final.png` |

The first run of the pre-existing `desktop-interaction-test.mjs` requested a temporary Core DB but did not set `MEGLE_ELECTRON_USER_DATA_DIR`; an existing default-profile registry can override the requested DB, and the run may have appended notification records to the default development Electron profile. It is not used as authoritative notification-isolation evidence. All authoritative notification-specific follow-up runs used explicit temporary Core DB and userData paths under `.tmp`. No real Artists files were modified.

## Residual Risk

- The OS-backdrop harness remains overall failing on pre-existing non-notification main-window, network-cancellation, backdrop-pixel, preview, and pointer-highlight baselines.
- Real display anti-aliasing was not judged by a human during this run; screenshots are evidence for layout/state/rendering only.
- Root purge running determinate progress was too fast to sample before terminal, although restart recovery and the final 7550/7550 terminal state were observed and focused Rust tests cover intermediate determinate progress.
- Controlled real-renderer duplicate-error injection and simultaneous visible Main/Core/Renderer log content remain unverified.
- The broad Artists interaction pass was not clean because of one preview timeout and 648 existing file-resource errors; no claim of universally stall-free browsing or absence of request amplification is made.
