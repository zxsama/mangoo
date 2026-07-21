# Unified Notification Center Verification

Date: 2026-07-21 (Asia/Shanghai)

## Scope and Revisions

- Task 1-15 implementation range reviewed: `c2cb5d95d2e040b6951b8522ea0a1707d8df0674..dcc83bb9108fe53c98bbdb13f5292621d6ad6b80`.
- Initial Task 16 commit: `38f21e38d507275372e94b6ac44180e8f5cbd784` (`test: verify unified notification center`).
- Review follow-up range: `38f21e38d507275372e94b6ac44180e8f5cbd784..HEAD`; the follow-up commit has subject `test: verify unified notification center`.
- The follow-up started from a clean `codex/unified-notification-center` worktree.
- Environment: Windows, Node `v22.14.0`, npm `10.9.2`, rustc `1.95.0`, cargo `1.95.0`, Python `3.11.9`.

The follow-up changes `crates/core/src/db/mod.rs`, a Task 1-15 file, only to add an ignored benchmark probe in its `#[cfg(test)]` module. This was required because the previous benchmark incorrectly treated final Desktop operation-head rows as persisted progress events. The probe calls the production task progress transaction and production operation-head/outbox implementation; it does not change production behavior. The production Desktop store, worker, retention code, validators, and visual baselines were not weakened or changed.

## Automated Gates

| Command | Result | Evidence summary |
| --- | --- | --- |
| `npm run check:core-api` | PASS | Core API contract alignment. |
| `npm run check:core-client` | PASS | Contract/typecheck and 41/41 tests. |
| `npm run check:schema` | PASS | SQLite migrations/schema/index checks. |
| `npm run check:rust` | PASS | 387 passed, 0 failed, 1 ignored benchmark probe; the `jpeg-decoder` profile warning remains unattributed. |
| `npm run check:desktop` | PASS | Desktop boundary/typecheck and 156/156 tests. |
| `npm run check:web` | PASS | Web boundary/typecheck and 122/122 tests. |
| `npm run check:ui-design` | PASS | Liquid Glass design boundaries. |
| `node tools/bench/notification-center-bench.mjs` | PASS | Production Core outbox plus Desktop worker/store invariants and cleanup fault injection. |
| `npm test` | PASS | Final full repository gate, exit 0 in 61.3 seconds. |

The focused gates were run individually before the final aggregate gate.

## Production-Boundary Benchmark

The benchmark invokes the ignored Core probe with `cargo test -p megle-core db::tests::notification_center_benchmark_probe -- --ignored --exact --nocapture --test-threads=1`. The probe creates a real file-backed Core SQLite database, then sends 10,000 updates through `update_task_scan_progress_for_attempt`, which publishes through the production operation-head/outbox transaction. The JavaScript benchmark independently queries the resulting Core SQLite sequence interval and head revision, then sends the actual Core outbox events through production `NotificationStore`, `worker_threads`, `NotificationStoreDatabase`, and Desktop SQLite WAL.

No machine-specific elapsed-time threshold is asserted. The recorded elapsed values are observations only.

| Metric | Value |
| --- | ---: |
| Libraries | 4 |
| Thumbnail completions / aggregate updates | 10,000 |
| Production Core rapid progress inputs | 10,000 |
| Logical Core progress revision advances | 10,000 |
| Persisted Core progress outbox events | 3 |
| Active operation-seconds | 2 |
| Core terminal events in the measured sequence interval | 1 |
| Total production Core outbox rows delivered to Desktop | 46 |
| Completed operations | 2,100 |
| Active operations | 20 |
| Desktop notification events | 2,145 |
| Desktop write/read/close requests | 10,006 / 1 / 1 |
| Core input elapsed | 1,717.660 ms |
| Core probe process elapsed | 2,293.132 ms |
| Desktop worker startup | 50.901 ms |
| Thumbnail aggregate writes | 574.498 ms |
| Core outbox Desktop apply | 1.361 ms |
| Terminal batches | 20.765 ms |
| Retention | 1.275 ms |
| Store/worker request elapsed | 661.297 ms |
| End-to-end benchmark elapsed | 2,959.125 ms |

Assertions against real persisted data:

- `persistedProgressEvents = 3`, read from Core `notification_outbox` rows between the pre-input and post-flush sequence boundaries. The bound is `activeOperationSeconds + terminalEventCount = 2 + 1 = 3`.
- An implementation that persisted all 10,000 inputs would produce `10,000 > 3` and fail this assertion.
- Core head revision advanced exactly 10,000 times, proving all input updates crossed the production boundary even though only three progress events landed in outbox.
- Per-thumbnail Desktop operation rows were `0`; thumbnail aggregate rows were `4`, equal to the library count.
- Retention kept `2,000` completed history rows, deleted `100` overflow rows, and retained all `20/20` active rows.
- The benchmark observed `notifications.sqlite`, `notifications.sqlite-wal`, and `notifications.sqlite-shm`, then injected a post-store-creation failure and verified the run directory disappeared. A separate dynamic-import failure also removed its run directory.

Benchmark evidence: repository-relative `.tmp/notification-center-bench/latest.json`; absolute `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\notification-center-bench\latest.json`.

## Default Profile Cleanup and Isolation

Before cleanup, no Electron or Megle Core process was running. The three exact files below were created at the start of the prior Task 16 interaction (`2026-07-21 21:22:19 +08:00`), while `.tmp/interaction-test/summary.json` was completed at `21:23:11`:

| Exact file | Size | SHA-256 |
| --- | ---: | --- |
| `C:\Users\84460\AppData\Roaming\Electron\notifications.sqlite` | 4,096 | `0AB48B25CBA617ED3A4ACCA0161813B314C095EF63544BB4AF60769EB1012977` |
| `C:\Users\84460\AppData\Roaming\Electron\notifications.sqlite-wal` | 399,672 | `CFE172F82F882111D077EF4C3EAC90D1211317FD732B3B587BA98AA0E9E98114` |
| `C:\Users\84460\AppData\Roaming\Electron\notifications.sqlite-shm` | 32,768 | `3CFD44DB739B6F6764164B69E39DE1B1F383028A6B41AA2577EB2B5ADB37ED55` |

The database contained only the `megle-dev` library, one running `root_scan`, one pending `interactive_folder_scan`, and one `megle-dev` thumbnail summary from that interaction. No evidence indicated older profile content. A non-recursive cleanup asserted the exact parent and allowed basenames, deleted only these three files, and verified all three absent. No directory or other userData file was removed.

All subsequent Electron launches used controlled worktree profiles. The OS harness process command line recorded:

```text
--user-data-dir=C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\data-desktop-ui\electron-user-data
```

That profile created its own notification SQLite/WAL/SHM and `libraries.json`. The follow-up Artists process was directly observed with `--user-data-dir=...\.tmp\task16-followup-artists-user-data`. After both runs, the three default AppData files remained absent.

Cleanup evidence:

| Evidence | Repository-relative path | Absolute path |
| --- | --- | --- |
| File metadata and hashes before cleanup | `.tmp/task16-appdata-cleanup/file-metadata-before.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-appdata-cleanup\file-metadata-before.json` |
| SQLite content before cleanup | `.tmp/task16-appdata-cleanup/sqlite-content-before.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-appdata-cleanup\sqlite-content-before.json` |
| Exact-file absence after cleanup | `.tmp/task16-appdata-cleanup/file-metadata-after.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-appdata-cleanup\file-metadata-after.json` |
| Follow-up OS profile and default-file check | `.tmp/task16-appdata-cleanup/followup-os-user-data.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-appdata-cleanup\followup-os-user-data.json` |

## OS Backdrop Harness

The required harness was run with `MEGLE_VISUAL_OS_BACKDROP=1` and an explicit temporary `MEGLE_ELECTRON_USER_DATA_DIR`; both variables were removed in `finally` while preserving the Node exit code.

**Overall harness status: FAIL (exit 1).** The current summary contains 24 hard failures: two network/startup-warning groups, five main-titlebar hover/drag/pointer checks, seven non-notification preview checks, and ten shared pointer-highlight checks. The prior report referenced 25; the fresh follow-up summary contains 24, so this report uses the current count. No base-commit harness was run, these failures remain non-notification/unattributed, and their origin is not assigned. No old visual baseline was changed.

**Notification-specific assertions: PASS.** None of the 24 hard failures is notification-specific. The current summary confirms one icon-only notification titlebar control, no legacy Tasks/Recent controls, valid compact anchoring before/maximized/restored, one reused notification window target, three full-window pages, five history filters, bounded history rows, shared secondary-window material, native reveal without fallback, and process-log source/level/pause/follow/copy/clear/export controls. Full, empty, degraded, narrow, status, history, and log fixtures rendered and passed the harness's notification checks.

| Evidence | Repository-relative path | Absolute path |
| --- | --- | --- |
| Harness summary | `.tmp/visual-check/logs/desktop-ui-regression-summary.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\logs\desktop-ui-regression-summary.json` |
| Real notification window | `.tmp/visual-check/screenshots/ui-notifications-secondary-window.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\screenshots\ui-notifications-secondary-window.png` |
| Real process-log page | `.tmp/visual-check/screenshots/ui-notifications-secondary-window-logs.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\screenshots\ui-notifications-secondary-window-logs.png` |
| Compact fixture | `.tmp/visual-check/screenshots/ui-notification-compact-fixture-full.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\screenshots\ui-notification-compact-fixture-full.png` |
| Full status fixture | `.tmp/visual-check/screenshots/ui-notification-window-fixture-status.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\screenshots\ui-notification-window-fixture-status.png` |
| Full history fixture | `.tmp/visual-check/screenshots/ui-notification-window-fixture-history.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\screenshots\ui-notification-window-fixture-history.png` |
| Full logs fixture | `.tmp/visual-check/screenshots/ui-notification-window-fixture-logs.png` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\screenshots\ui-notification-window-fixture-logs.png` |

## Real Electron Interaction

`Y:\Repository\Billfish\Artists` was used only as a read-only scan/browse source. Root removal deleted a temporary index, never source files. Move/rename/delete scenarios used copied files under `.tmp` only.

| Scenario | Status | Actual evidence |
| --- | --- | --- |
| Root scan progress aggregation | VERIFIED | A controlled Electron profile showed one active `root_scan` row and one compact popover row during the Artists scan. |
| Remove root progress/terminal/restart | VERIFIED WITH LIMIT | A safe temporary Artists index was interrupted at pending purge, restarted, and reached `succeeded` at `7550/7550`; running determinate samples completed too quickly to capture. |
| Move/rename/delete logical grouping | VERIFIED | Temporary copied PNGs produced 5 audit item rows but 3 logical group IDs and exactly 3 Desktop notification rows. |
| Thumbnail scrolling aggregate/no per-item | VERIFIED WITH LIMIT | Controlled global SQLite held one aggregate and no thumbnail operation; the benchmark separately proved 10,000 aggregate updates create zero per-item rows. |
| Library switch and restart history | VERIFIED WITH LIMIT | A second temporary library switched and restarted while first-library history remained in controlled global SQLite; a compact latest-three expectation in the driver was displaced by a newer renderer error, so that driver did not exit cleanly. |
| Broad error dedupe/no banner | PARTIAL | One real `renderer_error` and no broad center error strip were observed. Controlled duplicate injection through the real renderer was not performed; automated Desktop/Web tests cover dedupe. |
| Process logs | VERIFIED WITH LIMIT | The OS harness exercised source/level filters, pause, follow, copy, clear, and export. Simultaneously visible Main/Core/Renderer lines were not independently observed. |
| Layout/scroll/resize during scan | VERIFIED WITH LIMIT | The controlled notification interaction switched all four layouts, scrolled, and resized with no console errors; no media tile was available for preview in that run. |
| Broad Artists browse/scroll/preview follow-up | NOT VERIFIED | The fresh run used `.tmp/task16-followup-artists-user-data` but exceeded the 184-second outer timeout before producing a new summary. Its exact process tree was terminated. No pass or stall/request-amplification claim is made. |

Authoritative controlled interaction evidence remains under `.tmp/task16-notification-1784641021852` and `.tmp/task16-fsops-1784641292966`, each with its own `electron-user-data`. The old `.tmp/interaction-test/summary.json` from the default-profile run is not authoritative notification-isolation evidence.

## Residual Risk

- The OS backdrop harness is overall failing on 24 non-notification/unattributed checks; without a base-commit run their origin is not assigned.
- The fresh broad Artists follow-up timed out, so stall-free browsing, preview readiness, and absence of request amplification are not verified by that run.
- Root purge running determinate progress was too fast to sample before terminal.
- Controlled real-renderer duplicate-error injection and simultaneous visible Main/Core/Renderer log content remain unverified.
- Screenshot evidence covers layout/state/rendering, not human judgment of physical-display anti-aliasing.
