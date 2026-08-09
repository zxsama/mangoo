# Unified Overlay UI Verification

Date: 2026-08-09

Status: **NEEDS_CONTEXT**. All automatable committed-tree checks are green. The
physical Windows 100%/125%/150% relaunch matrix still requires a user-controlled
display session and is not marked passed.

## Immutable scope

- Plan range: `28ade3c08157c14346179839f759b8f6965019ce..e2cbae835b3f551fe9ce735afea55ff665aff9af`.
- Review-fix round 4 source range: `22a720496aebd504941f8ac64443406c1cb2a30a..4d2636b41dbf024c2982d809b6c7e4d8470dddfc`.
- Exact visual source: `4d2636b41dbf024c2982d809b6c7e4d8470dddfc` (tree `44ec7deaee8fd67b70893bc43f835ee04d221ba9`).
- Archive: `D:\Megle\.tmp\task8-exact-4d2636b-20260809-round4-final.tar`.
- Archive SHA-256: `21957B33D7718FB2DD365903486B06FBF1D27E4A61819C8F419178E963185536`.
- Extraction: `D:\Megle\.tmp\task8-exact-4d2636b-20260809-round4-final`; ordinary `git archive` directory, no `.git`, worktree, junction, symlink, or reparse dependency.
- Only `node_modules\.bin\electron.cmd` was added as an ordinary forwarding file, targeting the already-installed Electron executable. No install, reset, checkout, cleanup, or evidence deletion was used. Each visual run built and launched the archive-local Core binary rather than dirty live-tree output.

The live-worktree guard returned to the protected 17 modified plus 12 untracked
files after every source commit, with an empty index, App SHA-256
`5CDB9648949EF64AD2612D53C7029635EB5EFCC6F15F6305780454CB3F965A56`,
and no Settings/Plugins production diff.

## Exact automated results

Evidence directory:
`D:\Megle\.tmp\task8-exact-4d2636b-20260809-round4-final\.tmp\visual-check\logs\task8-round4-4d2636b`.

| Command | Exit | Result |
| --- | ---: | --- |
| `node --test .tmp\visual-check\task8-harness-reproducibility.test.mjs` | 0 | 14/14; actual late-output finalization, phase-aware network classification, Cargo diagnostics, controlled media, raw identity, DPI/fallback, product-request lifecycle, and notification handshake coverage |
| `$env:MEGLE_VISUAL_NOTIFICATIONS_ONLY='1'; node .tmp\visual-check\desktop-ui-regression.mjs` | 0 | run `megle-visual-msl55wkh-22300`; scenario/startup/console/network/hard failures 0; Cargo diagnostics 0 |
| `$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs` | 0 | run `megle-visual-msl56z4d-49812`; `required-os-composited: passed`; all five failure counts 0; actual child/stream finalization complete |
| `node .tmp\visual-check\desktop-ui-exhaustive-interaction-sweep.mjs` | 0 | run `megle-exhaustive-msl595vd-68720`; 18/18; all five failure counts 0; `networkProblems=[]`, `networkFailures=[]` |
| `npm test` in the exact archive | 0 | every structure/API/client/notification/desktop/web/UI/schema/Rust gate passed |
| `git diff --check` | 0 | no whitespace errors |

The exact full gate reports core-client 46/46, desktop 222/222, web 372/372,
and Rust 472 passed, 0 failed, 2 ignored. The notification-contract boundary is
its alignment check plus TypeScript typecheck, followed by the same alignment
check as its `test` script; it has no TAP count. The 46/46 count belongs only to
`@megle/core-client`.

## Run IDs, summaries, and raw identity

| Run | Copied summary | Raw stdout / stderr |
| --- | --- | --- |
| Notification-only `megle-visual-msl55wkh-22300` | `notification-megle-visual-msl55wkh-22300-summary.json` | `desktop-ui-regression-megle-visual-msl55wkh-22300.stdout.log` / `.stderr.log` |
| Required OS `megle-visual-msl56z4d-49812` | `required-os-megle-visual-msl56z4d-49812-summary.json` | `desktop-ui-regression-megle-visual-msl56z4d-49812.stdout.log` / `.stderr.log` |
| Exhaustive `megle-exhaustive-msl595vd-68720` | `exhaustive-megle-exhaustive-msl595vd-68720-summary.json` | `desktop-ui-exhaustive-megle-exhaustive-msl595vd-68720.stdout.log` / `.stderr.log` |

All six raw files exist, every raw filename contains the matching summary
`run.id`, and no run shares a raw path. Every run reports
`cargoBuildFinished=true`, empty `cargoDiagnostics`, and empty failure arrays.

The required OS finalization fields come from actual observations, not
constants: `childCloseObserved=true`, `childExitCode=1` after the harness stop,
`childSignal=null`, stdout/stderr `finished=true` and `closed=true`,
`streamErrors=[]`, and `complete=true`. Summary construction occurs only after
child close and both raw output streams drain/close. The focused behavior test
injects warning/error output after 340 ms and proves it reaches final diagnostics
before the summary is constructed.

The first notification-only archive attempt, run
`megle-visual-msl4eu8t-55176`, did not reach the app because the initially copied
stock `electron.cmd` referenced the absent archive-local Electron package. The
tool call reached its timeout after the archive-local Core had built; inspection
was limited to that run ID and its harness PID, and only PID 55176 was stopped.
The failed logs remain retained. Replacing the ordinary command forwarder with
one targeting the already-installed Electron executable produced the three green
final visual runs above; no unrelated process or prior evidence was touched.

## Round 4 request-lifecycle coverage

- Shared preview requests now have a real 60-second upper bound at the Core-client
  fetch boundary. The focused test leaves every consumer and makes fetch never
  settle; at the production deadline the underlying signals abort, caller/cache
  promises settle, both foreground slots release, the queued request advances,
  and the cache becomes empty without an unhandled rejection.
- Same-library refresh still retires old media pages without aborting their
  network work, preserving the generation fence and cache-friendly completion.
  The production mounted-hook test then unmounts after retirement and proves the
  underlying signals for both active and retired fetches are actually aborted,
  while stale responses cannot publish state.
- Library starting-core and unmount are hard ownership boundaries for active and
  retired media-page controllers. Scenario churn remains non-aborting, so the
  strict Network gate was neither allowlisted nor weakened.

## Controlled coverage

- Notification generated, validated, and auto-added 10 media files under `D:\Megle\.tmp\task8-exact-4d2636b-20260809-round4-final\.tmp\visual-check\runtime-media\megle-visual-msl55wkh-22300\outputs`.
- Exhaustive generated and invoked `D:\Megle\.tmp\task8-exact-4d2636b-20260809-round4-final\.tmp\visual-check\runtime-media\megle-exhaustive-msl595vd-68720\outputs`; `invokedTree` records the archive harness, repository root, child cwd, controlled root, and `http://127.0.0.1:5178/`.
- Notification navigation waits for ready state, foreground focus, one explicit `open()`, matching requested/rendered IDs, `open=true`, and a mounted root. Navigation diagnostics contain zero console and network problems.
- Required production surfaces: 18/18. Appearance controls: 14/14 at min/default/max/restored. Unexpected changed paths and unaffected leakage: 0; media grid and preview canvas remained stable.
- The Settings range slider exposes exactly one production pointer owner. At screen `(592.6, 593)`, the owner recorded local `(75, 14)`, opacity 1, while the range input did not become a competing owner.
- Controlled DPR: effective 1.25, 1.5, then 1.0 reset. Reduced motion matched with animation `none`, duration `0s`, and transform `none`. Reduced transparency matched with opaque fill and all effective filters `none`.
- Production solid fallback uses the same declarations as `@supports not`: popover, nested, context, and modal all had an opaque 0.98 backdrop, no effective blur, 10.39:1 composited contrast, focus, and real CDP Enter activation.
- The exhaustive phase classifier records `scenario` or `teardown` on raw network problems. Only the exact teardown canceled-abort signature is expected; the focused behavior test proves the same signature in `scenario` remains an unexpected failure. The final sweep had no raw or unexpected network failures.
- OS capture: `ui-window-backdrop-evidence-megle-visual-msl56z4d-49812.png`; Electron PID 74848, window 1456x928, `maxDelta=169.74`, `bottomMaxDelta=79.21`, `uiSurfaceDelta=205.31`; status `passed`.

## Current dirty WIP (separate)

Current-checkout counts and results are recorded separately in the Task 8
execution report. They include unrelated user WIP and are never attributed to
the immutable committed source above.

## Manual physical Windows relaunch blocker

No automated step changed Windows display scaling, the registry, or the user
session. A user with the physical monitor must complete every row below; until
then this verification remains `NEEDS_CONTEXT`.

At each of 100%, 125%, and 150%:

1. Set Scale through Windows Settings > System > Display. Do not edit the registry. Fully exit Megle, then relaunch from `D:\Megle\.tmp\task8-exact-4d2636b-20260809-round4-final` with `$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs; $code=$LASTEXITCODE; Remove-Item Env:\MEGLE_VISUAL_OS_BACKDROP; exit $code`.
2. Exercise titlebar filter/sort/layout/notification roots; nested library removal; file/folder/root context menus at all four corners; one keyboard-only dialog; compact and full notifications; and the narrow notification-window layout.
3. Confirm 12 CSS-pixel overlay gutters, readable 32 px controls, no clipped text/action, no one-pixel divider gap, visible focus, correct anchor attachment, and successful close/return behavior.
4. Record configured scale, Windows effective scale, monitor identifier, resolution, commit, timestamp, run ID, summary and raw paths, screenshot/video paths, pass/fail, and notes for each row.

## Residual limits

- Physical Windows 100%/125%/150% relaunch behavior is not verified.
- The controlled forced path executes the same production unsupported-feature declarations, but no available browser engine natively reported `backdrop-filter` unsupported.
- Native/CDP captures prove layout and compositing evidence, not physical DWM text, corner, or real-display anti-aliasing. Physical AA remains unverified.
