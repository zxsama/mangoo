# Unified Overlay UI Verification

Date: 2026-08-09

This report records the final Task 8 verification for
[`2026-07-27-unified-overlay-ui.md`](../superpowers/plans/2026-07-27-unified-overlay-ui.md)
against
[`2026-07-27-unified-overlay-ui-design.md`](../superpowers/specs/2026-07-27-unified-overlay-ui-design.md).

## Immutable scope

- Exact verification range: `28ade3c08157c14346179839f759b8f6965019ce..7f407d98dc965906535baa33f12641576e7bb4d3`.
- Exact committed source: `7f407d98dc965906535baa33f12641576e7bb4d3`.
- The range includes the Core terminal-state fix (`28ade3c`), deterministic visual-harness fixes through `0a702b5`, zero-warning exact Core build/test coverage (`0744aa1`), and per-run Cargo/raw-output diagnostics (`7f407d9`).
- Archive: `D:\Megle\.tmp\task8-exact-7f407d9-20260809-final.tar`.
- Archive SHA-256: `A27863665AD4C615A79374446CFA5724BB99933AFC3AE6E5A8753E8321466CFC`.
- A second `git archive` produced the same SHA-256. The retained extraction at `D:\Megle\.tmp\task8-exact-7f407d9-20260809-final` is an ordinary directory with zero reparse points; it is not a worktree, junction, or symbolic link.
- No `npm install`, `npm ci`, recursive cleanup, reset, or checkout was used. The visual runs compiled Core inside the archive extraction at `target\debug\megle-core.exe`; they did not run the live dirty worktree binary. The existing root dependency installation supplied tooling only.
- The live worktree remained separate: 17 modified files, 12 untracked files, empty index, and `apps/web/src/app/App.tsx` SHA-256 `5CDB9648949EF64AD2612D53C7029635EB5EFCC6F15F6305780454CB3F965A56` at takeover and before the report edit. Settings and Plugins production sources remained unmodified.

## Exact committed results

Exact evidence is retained in
`D:\Megle\.tmp\visual-check\logs\task8-final-7f407d9-20260809`.

| Command | Exit | Result | Evidence |
| --- | ---: | --- | --- |
| `node --test .tmp\visual-check\task8-harness-reproducibility.test.mjs` | 0 | 8/8; includes generated-root validation, Cargo pre-`Finished` warning/error capture, ordinary-progress rejection, stop-before-final-diagnostics, unique raw paths, DPI/appearance assertions, and one-open notification handshake | `01-exact-7f407d9-harness-tests.log` |
| `$env:MEGLE_VISUAL_NOTIFICATIONS_ONLY='1'; node .tmp\visual-check\desktop-ui-regression.mjs` | 0 | controlled notification center; startup/console/network/hard failures 0; Cargo diagnostics 0 | `03-exact-7f407d9-notification-only-summary.json` |
| `$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs` | 0 | `required-os-composited: passed`; startup/console/network/hard failures 0; Cargo diagnostics 0 | `04-exact-7f407d9-os-backdrop-summary.json` |
| `$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-exhaustive-interaction-sweep.mjs` | 0 | 18/18 scenarios; scenario/startup/console/hard failures 0; Cargo diagnostics 0 | `05-exact-7f407d9-exhaustive-summary.json` |
| `npm run check:structure` | 0 | structure and contracts passed | `06b-exact-7f407d9-plan-gates.log` |
| `npm run check:web` | 0 | boundary, typecheck, and 369/369 tests passed | `06b-exact-7f407d9-plan-gates.log` |
| `npm run check:desktop` | 0 | boundary, typecheck, and 222/222 tests passed | `06b-exact-7f407d9-plan-gates.log` |
| `npm run check:ui-design` | 0 | liquid-glass design boundaries passed | `06b-exact-7f407d9-plan-gates.log` |
| `npm test` | 0 | all repository gates passed | `07-exact-7f407d9-npm-test.log` |
| `git diff --check 28ade3c..7f407d9` | 0 | no whitespace errors in the immutable range | command transcript retained with Task 8 report |

The exact full gate reports core-client 45/45, desktop 222/222, web 369/369,
and Rust 472 passed, 0 failed, 2 ignored. The notification-contract package
does not expose a TAP test count: its actual boundary is the package alignment
check plus TypeScript typecheck, followed by the same package alignment check
as its `test` script. Both checks passed. The 45/45 count belongs to
`@megle/core-client`, not the notification contract.

## Run and raw-output identity

Each successful visual summary contains a `run.id`; each stdout/stderr filename
contains that same ID, exists, and is unique across all three runs. The failed
diagnostic attempt caused by an intentionally isolated `CARGO_TARGET_DIR` was
retained separately as evidence that a missing archive-local Core binary is a
hard startup failure. It is not one of the three final required runs.

| Run | Run ID | Summary | Raw stdout / stderr |
| --- | --- | --- | --- |
| Controlled notification-only | `megle-visual-msky31yi-66372` | `03-exact-7f407d9-notification-only-summary.json` | `desktop-ui-regression-megle-visual-msky31yi-66372.stdout.log` / `.stderr.log` |
| Required OS backdrop | `megle-visual-msky566o-29612` | `04-exact-7f407d9-os-backdrop-summary.json` | `desktop-ui-regression-megle-visual-msky566o-29612.stdout.log` / `.stderr.log` |
| Exhaustive interaction | `megle-exhaustive-msky7c0k-17000` | `05-exact-7f407d9-exhaustive-summary.json` | `desktop-ui-exhaustive-megle-exhaustive-msky7c0k-17000.stdout.log` / `.stderr.log` |

Both desktop harnesses record Cargo warnings/errors emitted before Cargo's
`Finished` marker as startup failures. The focused test injects a warning and
an error before `Finished`, verifies both are retained, and verifies ordinary
`Compiling` progress is not misclassified. All successful runs report
`cargoBuildFinished=true` and `cargoDiagnostics=[]`. Finalization stops the
child before collecting diagnostics; the focused stop-time injection test
proves errors emitted during `stop()` reach the final result.

## Controlled media and notification lifecycle

The notification-only run generated and invoked this controlled root:

`D:\Megle\.tmp\task8-exact-7f407d9-20260809-final\.tmp\visual-check\runtime-media\megle-visual-msky31yi-66372\outputs`

Its summary records `added=true`, no root-failure line, and 10 validated files:
1600x900 landscape, 900x1600 portrait, nested 72x72 media, one 320x180 txt2img
sample, and six 320x180 grid samples. The exhaustive run independently
generated and invoked:

`D:\Megle\.tmp\task8-exact-7f407d9-20260809-final\.tmp\visual-check\runtime-media\megle-exhaustive-msky7c0k-17000\outputs`

Its `invokedTree` records the exact harness path, repository root, child cwd,
controlled root, and `http://127.0.0.1:5178/`, all inside the archive extraction.

The compact-notification fixture waits for `ready=true` and the requested state,
focuses the target, issues exactly one explicit `open()` request, and requires
the same request ID to be requested and rendered with `open=true` and
`#notifications-root` present. The focused source-contract test passed, and
the OS run reports zero fixture navigation console/network diagnostics. The
controlled notification-only run covered status, initial history, retry/error
history, log gap, log actions, subscription cleanup, narrow mode, and degraded
mode.

## Surface and appearance matrix

The production appearance matrix is stored at
`evidence.settingsInterfaceStyle.appearanceMatrix` in the OS summary.

- Required production surfaces: 18/18 present. These cover side/shared,
  center/control, dialog/modal, media-grid, and preview-canvas probes.
- Existing appearance controls: 14/14 sampled at minimum, default, maximum,
  and restored values (5 side, 5 center, 4 dialog).
- Every control changed its required production target and its allowed mapped
  effects. All 14 restored to their initial values.
- Unexpected changed leaf paths: 0. Declared unaffected group failures: 0.
- Media grid and preview canvas remained byte-stable for every sample.
- Fixture console failures: 0. Fixture network failures: 0.
- Settings and Plugins retained their baseline; no production source under
  either feature was changed by Task 8 completion.

## DPI, preferences, and compositor evidence

Controlled device-scale evidence hard-asserted renderer
`window.devicePixelRatio` at 1.25 and 1.5, then reset it to 1.0. The CDP visual
viewport scale remained 1.0. This is controlled renderer evidence, not a claim
that Windows was physically relaunched at those display scale factors.

- Reduced motion: media query matched; animation name `none`, duration `0s`,
  transform `none`.
- Reduced transparency: media query matched; inspected root/child backdrop and
  filter values were `none`, with opaque `rgba(18, 24, 30, 0.98)` content.
- Missing `backdrop-filter`: Chromium could not emulate an engine reporting the
  feature unsupported, so that fallback remains residual rather than passed.

The required OS screenshot was captured by the native Windows capture bridge,
not `Page.captureScreenshot`:

`D:\Megle\.tmp\task8-exact-7f407d9-20260809-final\.tmp\visual-check\screenshots\ui-window-backdrop-evidence-megle-visual-msky566o-29612.png`

It records native Electron process ID 31760, window geometry 1456x928,
`maxDelta=32.67`, `bottomMaxDelta=47.03`, and `uiSurfaceDelta=132.18`; the
summary status is `required-os-composited: passed`.

## Current dirty WIP confirmation (not committed evidence)

After this report edit, the separate live worktree passed the focused Task 8
harness tests 8/8 and a full `npm test` rerun: core-client 45/45, desktop
222/222, web 373/373, and Rust 479 passed, 0 failed, 2 ignored. These larger
web/Rust counts include the user's unrelated uncommitted work and are not
attributed to `7f407d9` or any committed Task 8 source.

The first live-worktree full-gate attempt exposed two timing-sensitive mounted
file-operation focus assertions under the parallel web suite. The exact
committed full gate had already passed them; an immediate focused rerun passed
13/13, and the subsequent unchanged full rerun passed. Both the initial failure
and the successful reruns are retained in the Task 8 evidence directory.

## Residuals

- Physical Windows display relaunches at 125% and 150% were not performed.
- An unsupported-`backdrop-filter` engine fallback was not available to the
  Chromium harness and remains unverified.
- CDP screenshots and the native captured image do not prove physical DWM text,
  corner, or compositor anti-aliasing. Real on-display AA remains unverified.
- Raw known Electron sandbox startup entries and expected canceled teardown
  requests remain visible in JSON, but the harness classifies them separately;
  unexpected console and network failure counts are zero.
- The live dirty WIP counts above are verification of the current checkout only;
  they are deliberately separated from immutable committed evidence.
