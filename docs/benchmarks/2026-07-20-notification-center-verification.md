# Unified Notification Center Verification

Date: 2026-07-22, updated 2026-07-24 (Asia/Shanghai)

## Scope and Fixed Revisions

- Task 1-15 implementation range reviewed: `c2cb5d95d2e040b6951b8522ea0a1707d8df0674..dcc83bb9108fe53c98bbdb13f5292621d6ad6b80`.
- Initial Task 16 verification commit: `38f21e38d507275372e94b6ac44180e8f5cbd784` (`test: verify unified notification center`).
- First findings follow-up: `6f523577d6aec6526606b19d412b68ff2965a15d` (`test: verify unified notification center`).
- Production-path benchmark hardening: `606f992d6c37a17e3103413107f32cb440cc5510` (`test: harden notification center benchmark`).
- Monotonic timestamp and real Core fault-probe fix: `55142d46b3703cb0f75fa0a7596b86b9471f28a7` (`fix(core): keep notification timestamps monotonic`).
- Focused gates, three benchmark runs, and `npm test` reported below ran against the exact code content committed as `55142d46b3703cb0f75fa0a7596b86b9471f28a7`.
- Environment: Windows, Node `v22.14.0`, npm `10.9.2`, rustc `1.95.0`, cargo `1.95.0`, Python `3.11.9`.

Three Task 1-15 files were changed only for real defects found during Task 16:

- `apps/web/src/core/notificationReporting.ts`: the Settings renderer used shared library state and attempted the Main-role-only `mangoo:notifications-thumbnail-summary` IPC. The fix role-gates notification publishing and has regression coverage.
- `crates/core/src/db/mod.rs`: the benchmark needed observable production persistence behavior, not a parallel algorithm. Progress and terminal notifications now use the same millisecond clock seam, and the test probe drives the real task/outbox and thumbnail completion/DB update functions. The probe itself is test-only, creates its own OS temporary directories, accepts no external DB/output path, and emits only prefixed JSON on stdout.
- `crates/core/src/db/notification_outbox.rs`: each new revision clamps `updated_at`, and terminal `completed_at`, to the preceding head timestamp. This prevents notification history from moving backward if the system clock rolls back.

No validator or visual baseline was weakened or changed.

## Automated Gates

| Command | Result | Evidence summary |
| --- | --- | --- |
| `npm run check:core-api` | PASS | Core API contract alignment. |
| `npm run check:core-client` | PASS | Contract/typecheck and 41/41 tests. |
| `npm run check:schema` | PASS | SQLite migrations/schema/index checks. |
| `npm run check:rust` | PASS | 391 passed, 0 failed, 2 ignored benchmark probes; the `jpeg-decoder` profile warning remains unattributed. |
| `npm run check:desktop` | PASS | Desktop boundary/typecheck and 156/156 tests. |
| `npm run check:web` | PASS | Web boundary/typecheck and 122/122 tests. |
| `npm run check:ui-design` | PASS | Liquid Glass design boundaries. |
| `node tools/bench/notification-center-bench.mjs` | PASS x3 | Identical deterministic counts in all three runs; elapsed observations differed as expected. |
| `npm test` | PASS | Final full repository gate, exit 0 in 63.4 seconds. |

Rust, Desktop, Web, and the benchmark were run individually before the final aggregate gate. The final `npm test` reran them and covered the remaining listed repository gates.

## Production-Boundary Benchmark

The benchmark invokes `cargo test -p mangoo-core db::tests::notification_center_benchmark_probe -- --ignored --exact --nocapture --test-threads=1`. The Rust probe internally creates four file-backed Core SQLite libraries. For every one of 10,000 thumbnails it creates real media, requests and starts the production thumbnail task, and calls `publish_thumbnail_and_succeed_task_if_source_fingerprint_and_task_attempt_current`, exercising the production cache/blob/media/task DB updates. It then queries production cache statistics.

The progress workload sends 10,000 updates through `update_task_scan_progress_for_attempt` using a deterministic synthetic schedule from `+1 ms` through `+10,000 ms`. The production persistence function receives the test clock through its clock seam, then the terminal transition uses the same seam at `+10,001 ms`. Exact persisted progress offsets must be `1,000` through `10,000 ms` in one-second steps and terminal history must remain monotonic. A 500 ms implementation would persist 20 progress events and fail; a per-input implementation would persist 10,000 and fail.

Ordinary Rust regression tests also cover a progress and terminal transition in the same millisecond, terminal completion after synthetic `+10,000 ms` progress, and a terminal clock rollback from `+10,000 ms` to `+2,000 ms`. In the rollback case the terminal head, outbox payload, and `completed_at` remain at the last progress timestamp, so history sorting does not retreat.

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
| Terminal updated offset (ms) | 10001 |
| Progress/terminal history timestamps monotonic | true |
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
| Core child PID | 38980 | 37368 | 28904 |
| Fault-probe Core child PID | 73716 | 14268 | 65872 |
| Core process | 18314.932 | 18317.968 | 18938.253 |
| Progress inputs | 1765.939 | 1645.915 | 1728.906 |
| All thumbnail completions | 16014.372 | 16193.428 | 16635.636 |
| Library 1 completions | 3495.776 | 3524.941 | 3572.682 |
| Library 2 completions | 3781.763 | 3847.052 | 4012.599 |
| Library 3 completions | 3808.022 | 3796.562 | 4039.711 |
| Library 4 completions | 3727.964 | 3880.218 | 3848.216 |
| Web reporting | 1.571 | 1.129 | 1.221 |
| Worker startup | 48.936 | 52.251 | 52.708 |
| Worker progress apply | 1.876 | 1.891 | 1.912 |
| Worker thumbnail reporting | 1.571 | 1.129 | 1.221 |
| Worker terminal batches | 28.149 | 29.537 | 33.841 |
| Worker retention | 1.926 | 2.087 | 2.187 |
| Worker compact read | 1.635 | 1.855 | 1.668 |
| Worker close | 13.791 | 14.339 | 14.416 |
| Total store/worker elapsed | 97.884 | 103.089 | 107.953 |
| End-to-end benchmark | 18487.217 | 18501.986 | 19132.397 |

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
| Fault-probe Core test binary started and reported a PID distinct from Cargo | PASS |
| Fault-probe Core database initialized with schema migrations 24/24 | PASS |
| Fault-probe Core DB, WAL, and SHM observed before failure | PASS |
| Cargo test command exit code after the intentional Core test panic | 101 |
| Cargo process stopped | PASS |
| Core test binary stopped | PASS |
| Store worker paired with Core failure stopped | PASS |
| Fault-probe Core temporary DB directory and sidecars removed | PASS |
| Store run directory paired with Core failure removed | PASS |
| Rust probe internal temporary directories removed | PASS |
| Successful-run Core child stopped | PASS |
| Stale prior success deleted before forced failure | PASS |
| Forced failure published no success evidence | PASS |
| Forced-failure child stopped / exit code | PASS / 1 |

The nonzero cleanup path invokes the real ignored `notification_center_benchmark_fault_probe`. Cargo successfully builds and starts the `mangoo-core` test binary; that binary creates and migrates its internal temporary SQLite database, observes `fault.sqlite`, `fault.sqlite-wal`, and `fault.sqlite-shm`, emits prefixed startup evidence, and then intentionally panics. The benchmark accepts the nonzero result only after proving the Cargo process, distinct Core test-binary process, and paired production Desktop store worker have all stopped and both temporary directories have been removed. It does not use a missing-package or command-line parse failure.

The benchmark deletes stale success output before starting and publishes temporary output atomically only after successful store closure and cleanup. This report embeds the authoritative counts and does not depend on an ignored benchmark result file.

## Default Profile Cleanup and Isolation

Before cleanup, no Electron or Mangoo Core process was running. The three exact files below were created at the start of the prior Task 16 interaction (`2026-07-21 21:22:19 +08:00`), while `.tmp/interaction-test/summary.json` completed at `21:23:11`:

| Exact file | Size | SHA-256 |
| --- | ---: | --- |
| `C:\Users\84460\AppData\Roaming\Electron\notifications.sqlite` | 4,096 | `0AB48B25CBA617ED3A4ACCA0161813B314C095EF63544BB4AF60769EB1012977` |
| `C:\Users\84460\AppData\Roaming\Electron\notifications.sqlite-wal` | 399,672 | `CFE172F82F882111D077EF4C3EAC90D1211317FD732B3B587BA98AA0E9E98114` |
| `C:\Users\84460\AppData\Roaming\Electron\notifications.sqlite-shm` | 32,768 | `3CFD44DB739B6F6764164B69E39DE1B1F383028A6B41AA2577EB2B5ADB37ED55` |

The database contained only the `mangoo-dev` library, one running `root_scan`, one pending `interactive_folder_scan`, and one `mangoo-dev` thumbnail summary from that interaction. No evidence indicated older profile content. A non-recursive cleanup asserted the exact parent and allowed basenames, deleted only these three files, and verified all three absent. No directory or other userData file was removed.

All later Electron launches used controlled worktree profiles. The OS harness process command line recorded:

```text
--user-data-dir=C:\Users\84460\.config\superpowers\worktrees\Mangoo\unified-notification-center\.tmp\visual-check\data-desktop-ui\electron-user-data
```

The Artists follow-up used `.tmp/task16-followup-artists-user-data`. No Electron process was launched for the benchmark hardening or monotonic timestamp follow-ups through `55142d46b3703cb0f75fa0a7596b86b9471f28a7`. The final audit again confirmed the three default AppData notification files absent.

Cleanup evidence:

| Evidence | Repository-relative path | Absolute path |
| --- | --- | --- |
| File metadata and hashes before cleanup | `.tmp/task16-appdata-cleanup/file-metadata-before.json` | `C:\Users\84460\.config\superpowers\worktrees\Mangoo\unified-notification-center\.tmp\task16-appdata-cleanup\file-metadata-before.json` |
| SQLite content before cleanup | `.tmp/task16-appdata-cleanup/sqlite-content-before.json` | `C:\Users\84460\.config\superpowers\worktrees\Mangoo\unified-notification-center\.tmp\task16-appdata-cleanup\sqlite-content-before.json` |
| Exact-file absence after cleanup | `.tmp/task16-appdata-cleanup/file-metadata-after.json` | `C:\Users\84460\.config\superpowers\worktrees\Mangoo\unified-notification-center\.tmp\task16-appdata-cleanup\file-metadata-after.json` |
| Follow-up OS profile and default-file check | `.tmp/task16-appdata-cleanup/followup-os-user-data.json` | `C:\Users\84460\.config\superpowers\worktrees\Mangoo\unified-notification-center\.tmp\task16-appdata-cleanup\followup-os-user-data.json` |

## OS Backdrop Harness

The required harness was rerun on 2026-07-24 with `MANGOO_VISUAL_OS_BACKDROP=1`. The final run completed in 35.9 seconds with exit 0, zero hard failures, no fatal error, no startup problems, and an explicit `required-os-composited` backdrop PASS.

**Overall harness status: PASS.** The earlier report grouped 24 hard failures as non-notification and unattributed; a fresh reproduction exposed 25 before cleanup. They were stale harness assumptions rather than 25 product regressions: retired pointer attributes and halo geometry, an obsolete preview selector, outdated preview-pane coordinates, attempting a CDP double-click on a native drag region, and broad error matching that counted exact canceled `net::ERR_ABORTED` requests and expected Electron auxiliary-renderer close messages as failures. The harness now checks the current shared pointer system and native drag contract, uses current preview geometry, and classifies only those exact expected cancellation/close cases. The real Rust startup warning found in the same pass came from three test-only database helpers compiled into production and was removed by gating them with `#[cfg(test)]`. No visual baseline was weakened or changed.

**Notification-specific assertions: PASS.** The summary confirms one icon-only notification titlebar control, no legacy Tasks/Recent controls, valid compact anchoring before/maximized/restored, one reused notification window target, three full-window pages, five history filters, bounded history rows, shared secondary-window material, native reveal without fallback, and process-log source/level/pause/follow/copy/clear/export controls. Full, empty, degraded, narrow, status, history, and log fixtures rendered and passed the notification assertions.

| Evidence | Repository-relative path | Absolute path |
| --- | --- | --- |
| Harness summary | `.tmp/visual-check/logs/desktop-ui-regression-summary.json` | `D:\Mangoo\.tmp\visual-check\logs\desktop-ui-regression-summary.json` |
| Real notification window | `.tmp/visual-check/screenshots/ui-notifications-secondary-window.png` | `D:\Mangoo\.tmp\visual-check\screenshots\ui-notifications-secondary-window.png` |
| Real process-log page | `.tmp/visual-check/screenshots/ui-notifications-secondary-window-logs.png` | `D:\Mangoo\.tmp\visual-check\screenshots\ui-notifications-secondary-window-logs.png` |
| Compact fixture | `.tmp/visual-check/screenshots/ui-notification-compact-fixture-full.png` | `D:\Mangoo\.tmp\visual-check\screenshots\ui-notification-compact-fixture-full.png` |
| Full status fixture | `.tmp/visual-check/screenshots/ui-notification-window-fixture-status.png` | `D:\Mangoo\.tmp\visual-check\screenshots\ui-notification-window-fixture-status.png` |
| Full history fixture | `.tmp/visual-check/screenshots/ui-notification-window-fixture-history.png` | `D:\Mangoo\.tmp\visual-check\screenshots\ui-notification-window-fixture-history.png` |
| Full logs fixture | `.tmp/visual-check/screenshots/ui-notification-window-fixture-logs.png` | `D:\Mangoo\.tmp\visual-check\screenshots\ui-notification-window-fixture-logs.png` |

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
| Broad Artists browse/scroll/preview follow-up | VERIFIED | A strict 500-operation run completed 500/500 with all four layouts, 116 distinct selected folders, 52 clicked media labels, 19 preview opens, 36 preview navigations, zero required skips, zero console errors, and zero unexpected network failures. All 3,000 network failures were exact canceled `net::ERR_ABORTED` requests. |

Authoritative controlled interaction evidence remains under `.tmp/task16-notification-1784641021852` and `.tmp/task16-fsops-1784641292966`, each with its own `electron-user-data`. The old `.tmp/interaction-test/summary.json` from the default-profile run is not authoritative notification-isolation evidence.

The Artists follow-up harness was corrected before drawing a product conclusion. Its old `button.media-tile` selector no longer matched the current role-based tile markup, and nested random searches repeatedly selected empty folders. It now uses bounded Core-index-driven media selection, expands the real ancestor chain, and distinguishes exact canceled requests from unexpected network errors. This produced a reproducible interaction run instead of increasing timeouts around a stale driver.

The corrected driver then exposed a real MediaGrid blob lifecycle race: when concurrent thumbnail consumers populated the shared object-URL cache, the losing consumer revoked its duplicate URL and immediately attempted to preload that revoked URL. A 30-operation diagnostic produced 700 `ERR_FILE_NOT_FOUND` failures from this path. MediaGrid now re-reads and adopts the durable shared URL before creating or preloading a duplicate; the final 500-operation run produced zero such failures.

The final run used the isolated persistent database at `.tmp/task16-followup-artists-20260724-2224`, with `resetData=false` after its cold scan, and ran from `2026-07-24T15:25:37.347Z` through `15:45:34.787Z`. Preview readiness was complete for all 34 measured waits (p95 440 ms, max 589 ms), selected-thumbnail waits were complete for all 19 opens (p95/max 1,288 ms), and preview navigation p95 was 1,154 ms. The authoritative summary is `.tmp/visual-check/logs/artists-million-desktop-sweep-summary.json` (`D:\Mangoo\.tmp\visual-check\logs\artists-million-desktop-sweep-summary.json`).

## Residual Risk

- The strict Artists run is functionally complete, but broad browsing still has visible performance outliers: recursive deep-scroll p95 was 10,085 ms (max 11,509 ms), the slowest indexed media-preparation operation was 16,310 ms, and 41 of 378 general visible-thumbnail waits did not complete within the harness observation window. Preview readiness itself had no incomplete samples.
- The final 500-operation run exercised a warm persistent isolated database. The earlier reset/cold-scan run reached only 21 interactions before the stale harness path stalled, so this evidence does not claim a separate 500-operation cold-start pass.
- Root purge running determinate progress was too fast to sample before terminal.
- Controlled real-renderer duplicate-error injection and simultaneous visible Main/Core/Renderer log content remain unverified.
- Screenshot evidence covers layout/state/rendering, not human judgment of physical-display anti-aliasing.

## Final Integration Reverification

After the fixed Task 16 evidence above, the cross-task review found and fixed
legacy-import retry, degraded-store bounds, online trailing progress flush,
partial Core log buffering, and compact-popover recount coupling. The final
reviewed code endpoint is `2559f42fad7e62653931f9e914409531e7b3c529`.

Fresh verification at that endpoint:

- `node tools/bench/notification-center-bench.mjs`: PASS in 14,687.943 ms.
  The production-path probe completed 10,000 thumbnails, emitted zero per-item
  notification rows, persisted four exact library aggregates, sampled ten
  progress events at 1,000 ms intervals, retained 2,000 completed rows, deleted
  exactly 100 overflow rows, and retained all 20 active rows. The Desktop
  Store/Worker portion completed in 96.871 ms.
- `npm test`: PASS in 63.5 seconds. This includes structure, Core API/client,
  notification contract, Desktop, Web, UI design, schema, and Rust gates. Rust
  reported 395 passed and 2 ignored tests.
- The benchmark cleanup self-tests confirmed Store/Worker and Core child
  termination plus temporary SQLite/WAL/SHM cleanup. The three default Electron
  profile notification files remained absent after this run.
- A final read-only integration review reported no remaining integration
  findings after `2559f42`.

The OS-backdrop harness and broad Artists interaction were subsequently rerun
on 2026-07-24. The OS harness passed with zero hard failures, and the strict
Artists run completed all 500 operations with zero console errors and zero
unexpected network failures. The measured browsing outliers and cold-start
coverage limit remain recorded above rather than being promoted away by the
functional PASS.

After the harness and MediaGrid corrections, the current working tree also
passed `npm run check:web`, `npm run check:desktop`, `npm run check:ui-design`,
`cargo test -p mangoo-core` (395 passed, 2 ignored), and the full `npm test`
repository gate in 64.9 seconds.
