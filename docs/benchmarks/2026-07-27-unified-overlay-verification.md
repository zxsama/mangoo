# Unified Overlay UI Verification

Date: 2026-08-08

This report records Task 8 of
[`2026-07-27-unified-overlay-ui.md`](../superpowers/plans/2026-07-27-unified-overlay-ui.md)
against the design in
[`2026-07-27-unified-overlay-ui-design.md`](../superpowers/specs/2026-07-27-unified-overlay-ui-design.md).

## Verification Basis

- Verification base: `6561d5a0ab3b3e27ff38f3478a7030a7063f2572`, the HEAD before Task 8 edits.
- The base worktree contained the user's 17 modified and 12 untracked WIP files. The index was empty before Task 8 staging and all unrelated WIP remained unstaged.
- The Task 8 endpoint is the commit containing this report, the README index update, and the two visual harness files. The tested range is `6561d5a0..HEAD` after that commit is created.
- `apps/web/src/app/App.tsx` remained SHA256 `5CDB9648949EF64AD2612D53C7029635EB5EFCC6F15F6305780454CB3F965A56`.
- Settings and Plugins production source files were not modified.

## Automated Gate

The pre-visual gate was run from `D:\Megle` on 2026-08-07. Exit evidence is in
`.tmp/visual-check/logs/task8-2026-08-07/`.

| Command | Exit | Result |
| --- | ---: | --- |
| `git diff --check` | 0 | Clean whitespace check. |
| `npm run check:structure` | 0 | Structure and contracts passed. |
| `npm run check:web` | 0 | 371 tests passed, 0 failed, 61 suites. |
| `npm run check:desktop` | 0 | 222 tests passed, 0 failed, 8 suites. |
| `npm run check:ui-design` | 0 | Liquid glass design boundaries passed. |
| `npm test` | 0 | Full gate passed; Rust reported 477 passed, 0 failed, 2 ignored. |

`check:web` executed the mounted overlay lifecycle and mounted file-operation
fixtures. These covered focus/re-entrancy, shared roots, context-menu clamping
and keyboard traversal, rename validation, move exclusions and cross-root
failure, permanent/recycle deletion confirmation, busy/failure recovery, and
notification wiring. The notification contract tests also covered compact and
full-center model behavior.

After the documentation and harness edits, `git diff --check` and `npm test`
were rerun. Their final exit evidence is recorded under
`.tmp/visual-check/logs/task8-2026-08-08/` as
`23-final-git-diff-check.*` and `24-final-npm-test.*`; both exited `0`.

## Controlled Fixtures

The final Windows run was:

```powershell
$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-regression.mjs; $code=$LASTEXITCODE; Remove-Item Env:\MEGLE_VISUAL_OS_BACKDROP; exit $code
```

Evidence:

- Exit: `0`.
- Summary: `D:\Megle\.tmp\visual-check\logs\task8-2026-08-08\21-os-backdrop-regression-final-coverage-summary.json`.
- OS mode: `required-os-composited`, status `passed`.
- `hardFailures`: empty; `startupProblems`: empty; console errors: 0.
- Cancelled `net::ERR_ABORTED` thumbnail/preview requests are expected while the harness changes selection or priority; no unexpected fixture network failure was reported.

The required post-document OS rerun also exited `0` with
`required-os-composited: passed`, no hard failures, and no startup problems.
Its summary is
`D:\Megle\.tmp\visual-check\logs\task8-2026-08-08\25-post-doc-os-backdrop-regression-summary.json`.
That rerun emitted the two known Electron sandbox startup console entries
(`sandboxed_renderer.bundle.js` and null `startupData.preloadScripts`) that the
harness identifies explicitly; no other console error was present.

The final controlled coverage included:

- Filter selected states and clear/empty restoration; all sort options; Adaptive, Waterfall, Grid, and List layout selection.
- Library switcher normal open/close, nested folder/tree interaction, search/zero-result restoration, root context actions, and secondary Settings/Plugins windows.
- Media, folder, and root context menus, including normal, disabled root actions, danger actions, viewport clamping, preview, rename, move, recycle-bin, and permanent-delete dialogs.
- Compact notification full active/thumbnail/failure, healthy empty, degraded empty, narrow, relocated/remounted anchor, and 125%/150% controlled page-scale states.
- Full notification center status/history/logs, failure drill-down, degraded, and narrow states.

Final notification evidence from the OS summary includes 2 active rows, 1
determinate and 1 indeterminate progress row, 2 thumbnail libraries, 3 tabs,
5 history filters, 13 virtualized history rows, the final history footer
`200 / 240`, rejected retry feedback, bounded logs with all five controls,
four visible/actionable failure rows, and the degraded narrow status.

The exhaustive command was:

```powershell
$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\visual-check\desktop-ui-exhaustive-interaction-sweep.mjs; $code=$LASTEXITCODE; Remove-Item Env:\MEGLE_VISUAL_OS_BACKDROP; exit $code
```

- Exit: `0`.
- Summary: `D:\Megle\.tmp\visual-check\logs\task8-2026-08-08\22-os-exhaustive-interaction-sweep-final-summary.json`.
- `ok: true`, failures empty, console errors 0.
- Results: startup/library load; Settings secondary window; Plugins secondary window; library switcher; notifications; filter/sort/layout sweeps; child-folder toggle; subfolder cards; search and clear; tree switch; zero-result restore; preview toolbar; Settings grid controls; media/folder/root context menus.

## Appearance Matrix

The OS regression opened the real Settings appearance window, exercised each
listed slider at its supported minimum, restored its initial value, exercised
its maximum, and restored it again. The final matrix is in
`evidence.settingsInterfaceStyle.appearanceMatrix` in the OS summary.

| Group | Controls exercised | Default | Minimum | Maximum | Restored |
| --- | --- | --- | --- | --- | --- |
| Side | blur, opacity, overlay strength, saturation, stroke opacity | `1`, `1`, `1`, `1.55`, `1` | `0`, `0`, `0`, `1`, `0` | `2`, `2`, `2`, `2.2`, `2` | yes |
| Center | blur, opacity, overlay strength, saturation, stroke opacity | `1`, `1`, `1`, `1.55`, `1` | `0`, `0`, `0`, `1`, `0` | `2`, `2`, `2`, `2.2`, `2` | yes |
| Dialog | blur, opacity, overlay strength, backdrop dim | `1`, `1`, `1`, `0.52` | `0`, `0`, `0`, `0` | `2`, `2`, `2`, `0.85` | yes |

Each endpoint changed at least one mapped CSS variable and the final Settings
values returned to their initial values. The final OS run passed the matrix
hard assertions. Settings and Plugins were observed as separate native
secondary windows; their production sources were not changed.

## DPI And Appearance

The default production window was captured at a `1440 x 920` renderer viewport.
The controlled notification fixture used `784 x 655` at both 125% and 150%
CDP page scale; in both cases the anchored surface had a 6px gap, was contained
in the viewport, and retained its right alignment. These are controlled browser
page-scale checks, not Windows display-scaling checks.

The agent did not change Windows Settings or relaunch at actual 100%, 125%, or
150% system display scaling. Changing system scaling is outside the harness's
safe bounded scope and would invalidate the user's live desktop state. Actual
Windows 125%/150% titlebar, corner, context-menu, and keyboard-dialog passes
remain a manual follow-up.

Reduced-preference evidence was collected through CDP media emulation:

- `prefers-reduced-motion: reduce` matched; the compact overlay computed
  `animationName: none`, `animationDuration: 0s`, and `transform: none`.
- `prefers-reduced-transparency: reduce` matched; the overlay used opaque
  `rgba(18, 24, 30, 0.98)` and both its own and child backdrop filters were
  `none`.
- The browser's `@supports not ((backdrop-filter: blur(1px)) ...)` fallback
  branch was not forcibly selected. Chromium supports the property, and CDP
  has no safe switch that makes the engine report unsupported CSS. The fallback
  rule is covered statically by the UI-design gate; an unsupported-engine pass
  remains a limitation.

## Native Window Evidence

The OS harness uses a Windows desktop capture, separate from renderer/CDP
screenshots. The native compositor evidence is:

- `D:\Megle\.tmp\visual-check\screenshots\ui-window-backdrop-evidence-megle-visual-msjentfl-12712.png`

The evidence analyzer reported transparent/native-acrylic desktop variation
around visible Megle surfaces, with maximum outside/inside backdrop delta
145.69 and UI surface delta 128.58. This supports real backdrop composition;
it is not a claim that CDP screenshots reproduce physical DWM anti-aliasing.

Additional layout/state screenshots from the same run are under
`D:\Megle\.tmp\visual-check\screenshots\`, including
`ui-settings-interface-style.png`,
`ui-notification-compact-fixture-full.png`,
`ui-notification-compact-fixture-full-150.png`,
`ui-notification-compact-fixture-degraded.png`,
`ui-notification-window-fixture-status.png`,
`ui-notification-window-fixture-history.png`,
`ui-notification-window-fixture-logs.png`,
`ui-notification-window-fixture-narrow.png`,
`ui-central-landscape-fit-long-edge.png`,
`ui-central-landscape-actual-100.png`, and
`ui-central-portrait-fit-long-edge.png`. These are layout/state evidence only.

## Harness Changes And Residuals

Only Task 8 harness coverage was changed:

- `desktop-ui-regression.mjs` now restores CDP target focus before clicks,
  diagnoses notification/landscape timeouts without converting them to passes,
  recognizes the production canvas preview readiness contract, derives preview
  scale from the transform matrix rather than canvas backing pixels, inspects
  direct glass layers, recognizes fixed-position anchored surfaces, and runs
  the appearance/DPI/reduced-preference matrix.
- `desktop-ui-exhaustive-interaction-sweep.mjs` was adapted to the current
  secondary-window and overlay selectors and current Library interaction
  architecture. Its original hard waits and failure behavior remain.

The first full OS attempt timed out at the compact degraded fixture. A later
diagnostic run reached the final assertions. The old landscape timeout was
accurately located in
`D:\Megle\.tmp\visual-check\logs\task8-2026-08-07\16-landscape-timeout-diagnostic.json`:
the selected media and ready placeholder were present, preview responses were
HTTP 200, and the element was a production `preview-canvas`, so the old
`naturalWidth` predicate could never become true. The hard 30-second wait was
retained and the predicate was corrected for canvas readiness. A transient CDP
`Runtime.evaluate` timeout also occurred in an intermediate run; the final OS
and exhaustive runs completed with exit 0.

Remaining risks are the unperformed real Windows scaling matrix, the
unsupported-engine fallback branch, and physical text/corner anti-aliasing
inspection on the user's display. No Task 8 production source changes were
made to hide these limitations.
