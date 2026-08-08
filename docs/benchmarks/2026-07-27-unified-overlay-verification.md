# Unified Overlay UI Verification

Date: 2026-08-09

Status: **NEEDS_CONTEXT**. Automated committed-tree verification is green. The
physical Windows 100%/125%/150% relaunch matrix still requires a user-controlled
display session and is not marked passed.

## Immutable scope

- Plan range: `28ade3c08157c14346179839f759b8f6965019ce..e2814b32453a3078c6fe9e5ac753479d031945b1`.
- Task 8 review-fix range: `055b667a5c3a0a7560a67a39f54b897ebdb8cd84..e2814b32453a3078c6fe9e5ac753479d031945b1`.
- Exact visual source: `e2814b32453a3078c6fe9e5ac753479d031945b1` (tree `c4493a4056f37f6960f25059ab8e2693edd6784e`).
- Archive: `D:\Megle\.tmp\task8-exact-e2814b3-20260809-round2-final.tar`.
- Archive SHA-256: `F863E4F564BBDC235DF237EC1D2C5DA6C0C4F52FBA7FA09FC2EE36169DEEEBA0`.
- Extraction: `D:\Megle\.tmp\task8-exact-e2814b3-20260809-round2-final`; ordinary `git archive` directory, no `.git`, worktree, junction, symlink, or reparse dependency.
- Only `node_modules\.bin\electron.cmd` was added as an ordinary forwarding file. No `npm install`, `npm ci`, reset, checkout, recursive cleanup, or deletion was used. Visual runs built `target\debug\megle-core.exe` inside the archive and did not use the dirty live-tree binary.

The live worktree guard remained 17 pre-existing modified plus 12 untracked
files, empty index between Task 8 commits, and App SHA-256
`5CDB9648949EF64AD2612D53C7029635EB5EFCC6F15F6305780454CB3F965A56`.
No production Settings or Plugins feature source changed.

## Exact automated results

Evidence directory:
`D:\Megle\.tmp\task8-exact-e2814b3-20260809-round2-final\.tmp\visual-check\logs\task8-round2-e2814b3`.

| Command | Exit | Result |
| --- | ---: | --- |
| `node --test .tmp\visual-check\task8-harness-reproducibility.test.mjs` | 0 | 11/11; normal OS stop-before-diagnostics control flow, strict network/console classification, Cargo pre-`Finished` diagnostics, raw identity, DPI/fallback, and notification handshake covered |
| `$env:MEGLE_VISUAL_NOTIFICATIONS_ONLY='1'; node .tmp\visual-check\desktop-ui-regression.mjs` | 0 | `run.ok=true`; startup/console/network/hard/scenario failures 0; Cargo diagnostics 0 |
| `$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs` | 0 | `required-os-composited: passed`; all five failure counts 0; post-stop final diagnostics proven |
| `node .tmp\visual-check\desktop-ui-exhaustive-interaction-sweep.mjs` | 0 | 18/18; startup/console/network/hard/scenario failures 0; 25 raw canceled-abort network records, `networkFailures=0` |
| `npm test` in the exact archive | 0 | all structure/API/client/notification/desktop/web/UI/schema/Rust gates passed |
| `npm test` in the current dirty checkout | 0 | all gates passed; this is current WIP evidence, not committed-source counting |
| `git diff --check` | 0 | no whitespace errors |

The exact full gate reports core-client 45/45, desktop 222/222, web 369/369,
and Rust 472 passed, 0 failed, 2 ignored. The notification-contract boundary is
the package alignment check plus TypeScript typecheck, followed by the same
alignment check as its `test` script; it has no TAP count. The 45/45 count
belongs only to `@megle/core-client`.

## Run IDs and raw identity

| Run | Run ID | Copied summary | Raw stdout / stderr |
| --- | --- | --- | --- |
| Notification-only | `megle-visual-msl06w41-76280` | `notification-megle-visual-msl06w41-76280-summary.json` | `desktop-ui-regression-megle-visual-msl06w41-76280.stdout.log` / `.stderr.log` |
| Required OS backdrop | `megle-visual-msl08txr-39812` | `os-megle-visual-msl08txr-39812-summary.json` | `desktop-ui-regression-megle-visual-msl08txr-39812.stdout.log` / `.stderr.log` |
| Exhaustive | `megle-exhaustive-msl0ank4-81768` | `exhaustive-megle-exhaustive-msl0ank4-81768-summary.json` | `desktop-ui-exhaustive-megle-exhaustive-msl0ank4-81768.stdout.log` / `.stderr.log` |

All six raw files exist, every filename contains its summary `run.id`, and all
six paths are unique. Each run reports `cargoBuildFinished=true` and empty
`cargoDiagnostics`. The required OS summary records
`childStoppedBeforeDiagnostics=true` and
`outputWaitCompletedBeforeDiagnostics=true`; the final summary is constructed
and written only after that sequence.

## Controlled coverage

- Notification generated, validated, and auto-added 10 media files under `D:\Megle\.tmp\task8-exact-e2814b3-20260809-round2-final\.tmp\visual-check\runtime-media\megle-visual-msl06w41-76280\outputs`.
- Exhaustive generated and invoked `D:\Megle\.tmp\task8-exact-e2814b3-20260809-round2-final\.tmp\visual-check\runtime-media\megle-exhaustive-msl0ank4-81768\outputs`; `invokedTree` records the archive harness, repository root, child cwd, controlled root, and `http://127.0.0.1:5178/`.
- Notification fixture requires ready state, foreground focus, one explicit `open()`, matching requested/rendered IDs, `open=true`, and mounted root before sampling.
- Required production surfaces: 18/18. Appearance controls: 14/14 at min/default/max/restored. Unexpected changed paths and unaffected leakage: 0. Media grid and preview canvas stayed stable.
- Controlled DPR: effective 1.25, 1.5, then 1.0 reset. Reduced motion and reduced transparency branches matched and passed.
- Production solid fallback: actual `overlay.css` variables are shared by `@supports not`, reduced transparency, and `data-overlay-solid-fallback="forced"`. Popover, nested, context, and modal surfaces all had an opaque 0.98 backdrop, no effective backdrop filter, 10.39:1 composited text contrast, and real CDP Enter activation. The modal was opened only for its phase so the production focus trap was not bypassed.
- OS capture: `ui-window-backdrop-evidence-megle-visual-msl08txr-39812.png`; native Electron PID 79216, window 1456x928, `maxDelta=85.53`, `bottomMaxDelta=153.99`, `uiSurfaceDelta=194.42`; status `passed`.

## Current dirty WIP (separate)

The current checkout passed the same 11/11 focused suite, Web typecheck/build,
UI design checks, and full `npm test`. Its larger web/Rust counts include the
user's unrelated WIP and must not be described as exact committed counts.

## Manual physical Windows relaunch blocker

No automated step changed Windows display scaling, the registry, or the user
session. A user with access to the physical monitor must complete all three
rows below; until then the task remains `NEEDS_CONTEXT`.

For each of 100%, 125%, and 150%:

1. In Windows Settings > System > Display, set Scale through the UI. Do not use registry edits. Close Megle completely and relaunch the exact archive command `$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs` from `D:\Megle\.tmp\task8-exact-e2814b3-20260809-round2-final`.
2. Exercise titlebar filter/sort/layout/notification roots, nested library removal, file/folder/root context menus at all four corners, a keyboard-opened dialog, compact and full notification views, and a narrow resize.
3. Confirm 12 CSS px overlay gutters, readable 32 px controls, no clipping or action loss, stable divider gaps, visible focus, correct anchor attachment, and successful close/return behavior.
4. Record configured scale, observed Windows effective scale, monitor identifier, resolution, commit, timestamp, run ID, summary path, stdout/stderr paths, screenshot/video paths, pass/fail, and notes for every row.

## Residual limits

- Physical Windows 100%/125%/150% relaunch behavior is not verified.
- The controlled forced path executes the same production unsupported-feature declarations, but a separate browser engine that natively reports `backdrop-filter` unsupported was not available.
- Native/CDP captures prove layout and compositing evidence, not physical DWM text, corner, or real-display anti-aliasing. Physical AA remains unverified.
