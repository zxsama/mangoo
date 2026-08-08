# Unified Overlay UI Verification

Date: 2026-08-08

This report records Task 8 of
[`2026-07-27-unified-overlay-ui.md`](../superpowers/plans/2026-07-27-unified-overlay-ui.md)
against
[`2026-07-27-unified-overlay-ui-design.md`](../superpowers/specs/2026-07-27-unified-overlay-ui-design.md).

## Immutable Scope

- Task 8 implementation range tested: `6561d5a0ab3b3e27ff38f3478a7030a7063f2572..ea0f52f6ba93fdc8b7e1cc030e8a261bb9f58083`.
- Review-fix range tested: `5e29d71fe12fd63c17e40e66897d64929e30d3a5..ea0f52f6ba93fdc8b7e1cc030e8a261bb9f58083`.
- Exact source snapshot: `ea0f52f6ba93fdc8b7e1cc030e8a261bb9f58083`.
- The exact snapshot includes Core commit `98b303648690fc65a1e12c9cce285d0feb4055ea`.
- The live worktree retained the user's 17 modified and 12 untracked files. The index was empty after the harness commit.
- `apps/web/src/app/App.tsx` remained SHA-256 `5CDB9648949EF64AD2612D53C7029635EB5EFCC6F15F6305780454CB3F965A56`.
- Settings and Plugins production sources were not changed.

The exact snapshot was created with `git archive`, not a worktree, junction, or
symbolic link. It was extracted to
`D:\Megle\.tmp\task8-exact-ea0f52f6ba93-20260808-074401` and was not deleted.
No package install was run. Node dependencies and the Electron launcher came
from the existing `D:\Megle` installation. This dependency reuse is a stated
limitation; archived source files themselves came from the immutable commit.

## Final Status

Task 8 harness and fixture review fixes are implemented, but the final exact
commit is **not feature-complete**. Exact Core source does not compile, so no
exact notification-only, required-OS, or exhaustive run can reach Electron.
The harnesses preserve this failure and exit nonzero.

The exact compile failure is in `crates/core/src/db/mod.rs`: a function declared
as `anyhow::Result<bool>` still returns `Result<()>` from
`ensure_one_task_attempt_updated(...)` and `Ok(())`. The live dirty WIP contains
an uncommitted `Ok(updated == 1)` change that makes the dirty tree compile, but
that user WIP was not staged, committed, copied into the archive, or used to
claim exact success.

## Automated Gates

| Tree | Command | Exit | Result | Evidence |
| --- | --- | ---: | --- | --- |
| Exact `ea0f52f6` | `node --test .tmp\visual-check\task8-harness-reproducibility.test.mjs` | 0 | 7/7 passed, including behavioral stop-before-diagnostics and focus-before-open contracts | `D:\Megle\.tmp\visual-check\logs\task8-review-fix-2026-08-08\07-exact-task8-harness-tests.log` |
| Exact `ea0f52f6` | focused mounted overlay lifecycle | 0 | 12/12 passed after the first full run's hook timeout | `D:\Megle\.tmp\visual-check\logs\task8-review-fix-2026-08-08\09-exact-overlay-lifecycle-focused.log` |
| Exact `ea0f52f6` | focused desktop Core lifecycle | 0 | 15/15 passed after the second full run hung in the parallel desktop suite | `D:\Megle\.tmp\visual-check\logs\task8-review-fix-2026-08-08\11-exact-core-lifecycle-focused.log` |
| Exact `ea0f52f6` | final `npm test` | 101 | Desktop 222/222; Web 369/369; UI design and schema passed; Rust failed to compile with two E0308 errors | `D:\Megle\.tmp\visual-check\logs\task8-review-fix-2026-08-08\12-exact-npm-test-final.log` |
| Dirty WIP | `npm run check:web` | 0 | 373/373 web tests passed | Console run |
| Dirty WIP | `npm run check:ui-design` | 0 | UI liquid-glass boundaries passed | Console run |
| Dirty WIP | final post-document `npm test` | 0 | Desktop 222/222; Web 373/373; Rust 478 passed, 2 ignored | `D:\Megle\.tmp\visual-check\logs\task8-review-fix-2026-08-08\18-final-dirty-wip-npm-test.log` |

The exact web count is 369. The dirty web count is 373. The final exact Rust
count is unavailable because the immutable source fails during compilation;
the older 470 count belonged to snapshot `92d565a9` and is not carried forward.
The dirty 478 count is WIP-only evidence and is not attributed to the exact
commit.

Two exact full-gate attempts exposed independent parallel-suite residuals before
the stable final reproduction: the first cancelled 12 mounted-overlay children
after a `#portal-descendant-root` before-hook timeout (`08-exact-npm-test.log`),
and the second hung after desktop lifecycle test 9
(`10-exact-npm-test-rerun.log`). Each affected suite passed immediately when run
focused without changing assertions. The final original `npm test` command then
passed both JS suites and reproduced the Core compile failure.

## Harness Results

| Tree | Harness | Exit | Final diagnostics | Summary |
| --- | --- | ---: | --- | --- |
| Dirty WIP | Notification-only controlled fixture | 1 | startup 4, raw console 2, network 11, hard 1 | `D:\Megle\.tmp\visual-check\logs\task8-review-fix-2026-08-08\01-dirty-notification-only-summary.json` |
| Dirty WIP | Required OS backdrop regression | 1 | startup 4, raw console 2, network 27, hard 1 | `D:\Megle\.tmp\visual-check\logs\task8-review-fix-2026-08-08\05-dirty-os-backdrop-summary.json` |
| Dirty WIP | Exhaustive interaction sweep | 1 | 18/18 scenarios, scenario failures 0, startup 5, console 0 | `D:\Megle\.tmp\visual-check\logs\task8-review-fix-2026-08-08\16-dirty-os-exhaustive-summary.json` |
| Exact `ea0f52f6` | Notification-only controlled fixture | 1 | startup 6, console 0, network 0, hard 2 | `D:\Megle\.tmp\visual-check\logs\task8-review-fix-2026-08-08\13-exact-notification-only-summary.json` |
| Exact `ea0f52f6` | Required OS backdrop regression | 1 | startup 6, console 0, network 0, hard 2 | `D:\Megle\.tmp\visual-check\logs\task8-review-fix-2026-08-08\14-exact-os-backdrop-summary.json` |
| Exact `ea0f52f6` | Exhaustive interaction sweep | 1 | scenarios 0, scenario failures 1, startup 6, console 0, hard 1 | `D:\Megle\.tmp\visual-check\logs\task8-review-fix-2026-08-08\15-exact-os-exhaustive-summary.json` |

All abbreviated paths above are under
`D:\Megle\.tmp\visual-check\logs\task8-review-fix-2026-08-08`.
There is no successful exact harness summary for this endpoint. Notification-only
and required OS timed out waiting for CDP after Cargo failed; both recorded the
same six E0308 startup diagnostics and the missing controlled-root startup as a
hard failure. Exhaustive recorded Cargo exit 101 and the web target timeout.

The dirty exhaustive run verified the invoked tree as `D:\Megle`, validated its
generated media root, and completed all 18 scenarios with no scenario or console
failure. It still exited 1 because final startup diagnostics included the dirty
Core dead-code warning and terminal-transition worker error lines. Scenario
success therefore cannot mask startup failure.

## Lifecycle Regressions

The compact notification fixture now exposes explicit `ready`, requested-open,
and rendered-open states with a monotonic request ID. After every navigation the
harness waits only for fixture readiness, focuses the target, issues exactly one
open request, and then requires the matching rendered-open ID plus
`#notifications-root`. The dirty required-OS run traversed full, failure, empty,
and degraded states without the previous failure-state timeout.

Notification-only finalization now stops the child process, waits for output
delivery, and only then aggregates stdout/stderr, startup, console, network, and
scenario diagnostics. A behavioral regression test injects an error during
`stop()` and proves it appears in final startup and hard-failure results. The
dirty notification run's nonzero result demonstrates that stop-time Core output
is no longer missed.

## Appearance Matrix

The matrix no longer depends on dirty workbench columns. A committed controlled
fixture uses production `LiquidGlassSurface`, `OverlaySurface`, `ModalSurface`,
secondary-window preference synchronization, and production CSS selectors. It
requires 18 surface probes:

- Side/shared: library navigation, notification nav, notification body, and the nav/body containment mapping.
- Center: notification content, center material, field control, select control and element, readable inner region, overlay selected fill, and selected notification tab.
- Dialog: popover, nested, context, modal, and modal backdrop.
- Unaffected: media grid and preview canvas.

The dirty OS summary stores the evidence at
`evidence.settingsInterfaceStyle.appearanceMatrix`. All 18 probes were present;
the appearance fixture emitted zero console and network errors. For all 14
sliders, minimum/default/maximum sampling produced every required changed target,
zero unexpected changed leaf paths, and byte-stable declared unaffected groups.
Every slider and production state returned to its initial value.

| Group | Controls | Min / Default / Max | Verified production response |
| --- | --- | --- | --- |
| Side | blur, opacity, overlay strength, saturation, stroke opacity | `0/1/2`; saturation `1/1.55/2.2` | Library material and side-derived notification body gradient changed; center/dialog/unaffected groups stayed stable |
| Center | blur, opacity, overlay strength, saturation, stroke opacity | `0/1/2`; saturation `1/1.55/2.2` | Center material, field/select, readable inner, and selected fills changed; only the explicitly mapped dialog saturation/stroke leaves were allowed |
| Dialog | blur, opacity, overlay strength, backdrop dim | `0/1/2`; dim `0/0.52/0.85` | Popover, nested, context, modal, and modal backdrop changed; side/center/shared/media/preview groups stayed stable |

The notification nav and content are transparent production children of the
shared notification body, so the matrix asserts their containment instead of
inventing local fills. The body gradient is currently derived from side
variables; center behavior is proven by the real content-inner surfaces. To
eliminate background-window transition timing from computed-style samples, the
appearance fixture asserts `prefers-reduced-motion: reduce` before each settled
sample; it does not remove or weaken any min/max value assertion.

## DPI And Preferences

The dirty required-OS run retained controlled scale and preference evidence even
though its overall exit was nonzero on Core startup diagnostics:

| Controlled state | Effective DPR | Viewport | Result |
| --- | ---: | --- | --- |
| 100% | 1.00 | `784 x 655` | Effective DPR and reset asserted |
| 125% | 1.25 | `784 x 655` | Effective DPR asserted |
| 150% | 1.50 | `784 x 655` | Effective DPR asserted |

`Emulation.setPageScaleFactor` itself left both protocol and renderer visual
viewport scale at 1. The harness therefore uses and records
`Emulation.setDeviceMetricsOverride`, then hard-asserts the actual
`window.devicePixelRatio`. This is controlled DPR evidence, not a claim that
Windows display scaling was changed. Actual Windows 100%, 125%, and 150% scale
relaunches were not performed.

- `prefers-reduced-motion: reduce` matched `true`; animation name was `none`, duration `0s`, and transform `none`.
- `prefers-reduced-transparency: reduce` matched `true`; the surface was `rgba(18, 24, 30, 0.98)` and every relevant root/child backdrop-filter and filter was `none`.
- Chromium could not emulate an engine that reports backdrop-filter unsupported. That visual fallback remains unverified; static UI-design coverage still applies.

## Native Evidence

The dirty required-OS run used the Windows desktop capture bridge, not
`Page.captureScreenshot`, and produced:

`D:\Megle\.tmp\visual-check\screenshots\ui-window-backdrop-evidence-megle-visual-msjl5x17-12672.png`

The file existed at report time. Its summary records `maxDelta: 5`,
`bottomMaxDelta: 4.2`, and `uiSurfaceDelta: 42.95`; no native-backdrop assertion
was added to the run's hard failures. This is OS-composited backdrop evidence
from a dirty-WIP run only. It is not evidence for physical DWM text/corner
anti-aliasing, and generated screenshots remain ignored and uncommitted.

## Residuals

- Exact `ea0f52f6` cannot compile Core because commit `98b3036` contains a `Result<bool>`/`Result<()>` mismatch. Exact notification, OS, exhaustive, and Rust verification remain red.
- Dirty notification and OS runs reach all Task 8 surfaces but remain red on stop-time Core startup diagnostics; raw Electron sandbox entries and expected cancelled requests remain visible in JSON.
- Actual Windows 125%/150% scaling, unsupported-backdrop visual fallback, and physical display anti-aliasing remain unverified limitations.
- Full-suite mounted-overlay and desktop lifecycle hooks each showed one parallel-run timeout/hang while their focused suites passed; no timeout or assertion was changed.

No Task 1-7 production source was changed or committed to hide these residuals.
