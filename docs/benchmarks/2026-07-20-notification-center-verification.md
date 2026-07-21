# Unified Notification Center Verification

Date: 2026-07-21 (Asia/Shanghai)

## Scope and Fixed Revisions

- Task 1-15 implementation range reviewed: `c2cb5d95d2e040b6951b8522ea0a1707d8df0674..dcc83bb9108fe53c98bbdb13f5292621d6ad6b80`.
- Initial Task 16 verification commit: `38f21e38d507275372e94b6ac44180e8f5cbd784` (`test: verify unified notification center`).
- First findings follow-up: `6f523577d6aec6526606b19d412b68ff2965a15d` (`test: verify unified notification center`).
- Production-path benchmark hardening: `606f992d6c37a17e3103413107f32cb440cc5510` (`test: harden notification center benchmark`).
- Focused gates, three benchmark runs, and `npm test` reported below ran against the exact code content committed as `606f992d6c37a17e3103413107f32cb440cc5510`.
- Environment: Windows, Node `v22.14.0`, npm `10.9.2`, rustc `1.95.0`, cargo `1.95.0`, Python `3.11.9`.

Two Task 1-15 files were changed only for real defects found during Task 16:

- `apps/web/src/core/notificationReporting.ts`: the Settings renderer used shared library state and attempted the Main-role-only `megle:notifications-thumbnail-summary` IPC. The fix role-gates notification publishing and has regression coverage.
- `crates/core/src/db/mod.rs`: the benchmark needed observable production persistence behavior, not a parallel algorithm. The existing progress path now delegates through a private injectable-clock seam while retaining the normal system clock, and the test probe drives the real task/outbox and thumbnail completion/DB update functions. The probe itself is test-only, creates its own OS temporary directories, accepts no external DB/output path, and emits only prefixed JSON on stdout.

No validator or visual baseline was weakened or changed.

## Automated Gates

| Command | Result | Evidence summary |
| --- | --- | --- |
| `npm run check:core-api` | PASS | Core API contract alignment. |
| `npm run check:core-client` | PASS | Contract/typecheck and 41/41 tests. |
| `npm run check:schema` | PASS | SQLite migrations/schema/index checks. |
| `npm run check:rust` | PASS | 388 passed, 0 failed, 1 ignored benchmark probe; the `jpeg-decoder` profile warning remains unattributed. |
| `npm run check:desktop` | PASS | Desktop boundary/typecheck and 156/156 tests. |
| `npm run check:web` | PASS | Web boundary/typecheck and 122/122 tests. |
| `npm run check:ui-design` | PASS | Liquid Glass design boundaries. |
| `node tools/bench/notification-center-bench.mjs` | PASS x3 | Identical deterministic counts in all three runs; elapsed observations differed as expected. |
| `npm test` | PASS | Final full repository gate, exit 0 in 62.9 seconds. |

The focused gates were run individually before the final aggregate gate.

## Production-Boundary Benchmark

The benchmark invokes `cargo test -p megle-core db::tests::notification_center_benchmark_probe -- --ignored --exact --nocapture --test-threads=1`. The Rust probe internally creates four file-backed Core SQLite libraries. For every one of 10,000 thumbnails it creates real media, requests and starts the production thumbnail task, and calls `publish_thumbnail_and_succeed_task_if_source_fingerprint_and_task_attempt_current`, exercising the production cache/blob/media/task DB updates. It then queries production cache statistics.

The progress workload sends 10,000 updates through `update_task_scan_progress_for_attempt` using a deterministic synthetic schedule from `+1 ms` through `+10,000 ms`. The production persistence function receives the test clock through its clock seam. Exact persisted progress offsets must be `1,000` through `10,000 ms` in one-second steps. A 500 ms implementation would persist 20 progress events and fail; a per-input implementation would persist 10,000 and fail.

The JavaScript side calls the real Web `publishThumbnailCacheStats` reporting boundary once for each Core result, then uses production Desktop `NotificationStore`, `worker_threads`, `NotificationStoreDatabase`, and SQLite WAL. No machine-specific elapsed-time threshold is asserted.

### Exact Deterministic Metrics

These values were identical in all three runs.

| Seed / Core metric | Exact value |
| --- | ---: |
| Libraries | 4 |
| Thumbnail completions | 10,000 |
| Thumbnail completions per library | 2,500 |
| Rapid progress inputs | 10,000 |
| Logical progress revisions | 10,000 |
| Persisted progress events | 10 |
| Persisted progress offsets (ms) | 1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000, 9000, 10000 |
| Terminal events | 1 |
| Total Core outbox events | 53 |
| Completed operations seeded | 2,100 |
| Synthetic completed operations | 2,099 |
| Active operations seeded | 20 |
| Active operation seconds | 10 |

| Desktop/reporting metric | Exact value |
| --- | ---: |
| Web reporting calls | 4 |
| Persisted aggregate rows | 4 |
| Per-thumbnail notification rows | 0 |
| Desktop operation rows after retention | 2,020 |
| Desktop persisted progress rows | 10 |
| Completed history rows | 2,000 |
| Overflow rows deleted | 100 |
| Expired rows deleted | 0 |
| Active rows retained | 20 |
| Desktop notification events | 2,152 |
| Core outbox events applied | 53 |
| Thumbnail summary updates | 4 |
| Store write requests | 10 |
| Store read requests | 1 |
| Store close requests | 1 |
| Total store/worker requests | 12 |

Each of the four Core libraries and each persisted Desktop summary had these exact values:

| Per-library metric | Exact value |
| --- | ---: |
| Completion count | 2,500 |
| Completed thumbnail task rows | 2,500 |
| Ready thumbnail state rows | 2,500 |
| Thumbnail blob rows | 2,500 |
| Per-item thumbnail outbox rows | 0 |
| Cached count | 2,500 |
| Total candidate count | 2,500 |
| Missing / stale / failed / pending / active bulk tasks | 0 / 0 / 0 / 0 / 0 |
| Total cache bytes | 10,000 |
| Completion percent | 100 |
| Observed-at values by library | 1784548800000, 1784548800001, 1784548800002, 1784548800003 |

The aggregate assertion is exact, not an upper bound: four reporting calls produce exactly four aggregate rows, one for each library, with all counts and byte totals matching the real Core DB results. The thumbnail probe also asserts zero per-item Core outbox rows, so an implementation that emits once per completion fails before Desktop aggregation.

### Three-Run Elapsed Metrics

All values are milliseconds and are observations only.

| Metric | Run 1 | Run 2 | Run 3 |
| --- | ---: | ---: | ---: |
| Core child PID | 68156 | 72256 | 57872 |
| Core process | 18732.122 | 18868.227 | 18643.349 |
| Progress inputs | 1668.680 | 1657.553 | 1734.476 |
| All thumbnail completions | 16514.051 | 16747.332 | 16439.164 |
| Library 1 completions | 3520.847 | 3589.174 | 3555.762 |
| Library 2 completions | 4005.146 | 3906.560 | 3892.615 |
| Library 3 completions | 3868.895 | 4100.712 | 3955.823 |
| Library 4 completions | 4002.898 | 4058.662 | 3932.235 |
| Web reporting | 1.682 | 1.181 | 1.795 |
| Worker startup | 50.225 | 49.259 | 54.479 |
| Worker progress apply | 2.111 | 1.997 | 2.102 |
| Worker thumbnail reporting | 1.682 | 1.181 | 1.795 |
| Worker terminal batches | 31.676 | 30.177 | 30.588 |
| Worker retention | 2.156 | 1.625 | 2.066 |
| Worker compact read | 1.859 | 1.206 | 1.428 |
| Worker close | 13.911 | 11.970 | 12.251 |
| Total store/worker elapsed | 103.620 | 97.415 | 104.709 |
| End-to-end benchmark | 18913.968 | 19044.365 | 18830.549 |

### Cleanup and Failure Injection

All three runs produced the following exact cleanup results:

| Failure-path assertion | Result |
| --- | --- |
| Dynamic import failure run directory removed | PASS |
| Store startup failure run directory removed | PASS |
| Store startup failure worker stopped | PASS |
| Post-create failure run directory removed | PASS |
| Post-create failure worker stopped | PASS |
| SQLite, WAL, and SHM observed before injected post-create failure | PASS |
| Cargo/Core nonzero child run directory removed | PASS |
| Cargo/Core nonzero child stopped | PASS |
| Store worker paired with Cargo failure stopped | PASS |
| Injected Cargo exit code | 101 |
| Rust probe internal temporary directories removed | PASS |
| Successful-run Core child stopped | PASS |
| Stale prior success deleted before forced failure | PASS |
| Forced failure published no success evidence | PASS |
| Forced-failure child stopped / exit code | PASS / 1 |

The benchmark deletes stale success output before starting and publishes temporary output atomically only after successful store closure and cleanup. This report embeds the authoritative counts and does not depend on an ignored benchmark result file.

## Default Profile Cleanup and Isolation

Before cleanup, no Electron or Megle Core process was running. The three exact files below were created at the start of the prior Task 16 interaction (`2026-07-21 21:22:19 +08:00`), while `.tmp/interaction-test/summary.json` completed at `21:23:11`:

| Exact file | Size | SHA-256 |
| --- | ---: | --- |
| `C:\Users\84460\AppData\Roaming\Electron\notifications.sqlite` | 4,096 | `0AB48B25CBA617ED3A4ACCA0161813B314C095EF63544BB4AF60769EB1012977` |
| `C:\Users\84460\AppData\Roaming\Electron\notifications.sqlite-wal` | 399,672 | `CFE172F82F882111D077EF4C3EAC90D1211317FD732B3B587BA98AA0E9E98114` |
| `C:\Users\84460\AppData\Roaming\Electron\notifications.sqlite-shm` | 32,768 | `3CFD44DB739B6F6764164B69E39DE1B1F383028A6B41AA2577EB2B5ADB37ED55` |

The database contained only the `megle-dev` library, one running `root_scan`, one pending `interactive_folder_scan`, and one `megle-dev` thumbnail summary from that interaction. No evidence indicated older profile content. A non-recursive cleanup asserted the exact parent and allowed basenames, deleted only these three files, and verified all three absent. No directory or other userData file was removed.

All later Electron launches used controlled worktree profiles. The OS harness process command line recorded:

```text
--user-data-dir=C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\visual-check\data-desktop-ui\electron-user-data
```

The Artists follow-up used `.tmp/task16-followup-artists-user-data`. No Electron process was launched for the `606f992d6c37a17e3103413107f32cb440cc5510` benchmark hardening follow-up. Its final audit again confirmed the three default AppData notification files absent.

Cleanup evidence:

| Evidence | Repository-relative path | Absolute path |
| --- | --- | --- |
| File metadata and hashes before cleanup | `.tmp/task16-appdata-cleanup/file-metadata-before.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-appdata-cleanup\file-metadata-before.json` |
| SQLite content before cleanup | `.tmp/task16-appdata-cleanup/sqlite-content-before.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-appdata-cleanup\sqlite-content-before.json` |
| Exact-file absence after cleanup | `.tmp/task16-appdata-cleanup/file-metadata-after.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-appdata-cleanup\file-metadata-after.json` |
| Follow-up OS profile and default-file check | `.tmp/task16-appdata-cleanup/followup-os-user-data.json` | `C:\Users\84460\.config\superpowers\worktrees\Megle\unified-notification-center\.tmp\task16-appdata-cleanup\followup-os-user-data.json` |

## OS Backdrop Harness

The required harness was run during the first findings follow-up with `MEGLE_VISUAL_OS_BACKDROP=1` and an explicit temporary `MEGLE_ELECTRON_USER_DATA_DIR`; both variables were removed in `finally` while preserving the Node exit code. It was not rerun for the later benchmark-only Rust/Node changes.

**Overall harness status: FAIL (exit 1).** The captured summary contains 24 hard failures: two network/startup-warning groups, five main-titlebar hover/drag/pointer checks, seven non-notification preview checks, and ten shared pointer-highlight checks. No base-commit harness comparison was run, so these failures are non-notification/unattributed and are not assigned an origin. No visual baseline was changed.

**Notification-specific assertions: PASS.** None of the 24 hard failures is notification-specific. The summary confirms one icon-only notification titlebar control, no legacy Tasks/Recent controls, valid compact anchoring before/maximized/restored, one reused notification window target, three full-window pages, five history filters, bounded history rows, shared secondary-window material, native reveal without fallback, and process-log source/level/pause/follow/copy/clear/export controls. Full, empty, degraded, narrow, status, history, and log fixtures rendered and passed the notification assertions.

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
| Thumbnail scrolling aggregate/no per-item | VERIFIED WITH LIMIT | Controlled global SQLite held one aggregate and no thumbnail operation; the production-path benchmark separately completed 10,000 thumbnails with zero Core per-item outbox and zero Desktop per-item operation rows. |
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
