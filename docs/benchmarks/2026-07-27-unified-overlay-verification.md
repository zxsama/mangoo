# Unified Overlay UI Verification

Date: 2026-08-08

This report records Task 8 of
[`2026-07-27-unified-overlay-ui.md`](../superpowers/plans/2026-07-27-unified-overlay-ui.md)
against
[`2026-07-27-unified-overlay-ui-design.md`](../superpowers/specs/2026-07-27-unified-overlay-ui-design.md).

## Immutable Scope

- Task 8 range: `6561d5a0ab3b3e27ff38f3478a7030a7063f2572..92d565a9e0d35f99f32026588dddb2a4f4e45e41`.
- Review-fix range: `5e29d71fe12fd63c17e40e66897d64929e30d3a5..92d565a9e0d35f99f32026588dddb2a4f4e45e41`.
- Exact clean source snapshot: `92d565a9e0d35f99f32026588dddb2a4f4e45e41`.
- The live worktree retained the user's 17 modified and 12 untracked files. The index was empty before every Task 8 commit.
- `apps/web/src/app/App.tsx` remained SHA-256 `5CDB9648949EF64AD2612D53C7029635EB5EFCC6F15F6305780454CB3F965A56`.
- Settings and Plugins production sources were not changed. Task 8 changed only tracked harness and documentation files.

The clean snapshot was created with `git archive`, not a worktree, junction, or
symbolic link. It was extracted to
`D:\Megle\.tmp\task8-exact-92d565a9e0d3-20260808-062605` and was not deleted.
No package install was run. The archive reused the existing
`D:\Megle\node_modules`, a shared Cargo target, a normal two-line Electron CMD
launcher, and a copied Core binary built from the archived source. Source files
were exact; the dependency installation was not independently rebuilt.

## Automated Gates

| Tree | Command | Exit | Result | Evidence |
| --- | --- | ---: | --- | --- |
| Exact `92d565a9` | `node --test .tmp\visual-check\task8-harness-reproducibility.test.mjs` | 0 | 5 passed; root resolution, generated media, rejection, diagnostics, DPI/preferences/appearance contracts | Exact archive console run |
| Exact `92d565a9` | `npm test` | 0 | Desktop 222/222; web 367/367; Rust 470 passed, 2 ignored | `D:\Megle\.tmp\visual-check\logs\task8-review-2026-08-08\19-exact-final-npm-test.log` |
| Dirty WIP | `npm test` | 1 | Web LibrarySwitcher before-hook failed; 10 children were cancelled | `D:\Megle\.tmp\visual-check\logs\task8-review-2026-08-08\10-dirty-wip-npm-test.log` |
| Dirty WIP | focused LibrarySwitcher test | 0 | 10/10 passed immediately after the failed full attempt | Console run |
| Dirty WIP | final post-document `npm test` | 0 | Desktop 222/222; web 371/371; Rust 477 passed, 2 ignored | `D:\Megle\.tmp\visual-check\logs\task8-review-2026-08-08\21-final-dirty-wip-npm-test.log` |

The clean counts are 367 web and 470 Rust. The 371 web and 477 Rust counts are
only dirty-WIP evidence and are not attributed to the immutable snapshot.

## Harness Results

| Tree | Harness | Exit | Summary |
| --- | --- | ---: | --- |
| Dirty WIP | Notification-only controlled fixture | 0 | `D:\Megle\.tmp\visual-check\logs\task8-review-2026-08-08\01-controlled-notification-summary.json` |
| Dirty WIP | Required OS backdrop regression | 0 | `D:\Megle\.tmp\visual-check\logs\task8-review-2026-08-08\05-os-backdrop-summary.json` |
| Dirty WIP | Exhaustive interaction sweep | 1 | `D:\Megle\.tmp\visual-check\logs\task8-review-2026-08-08\08-os-exhaustive-summary.json` |
| Exact `bfdc1386` | Notification-only controlled fixture | 1 | `D:\Megle\.tmp\visual-check\logs\task8-review-2026-08-08\15-exact-controlled-notification-summary.json` |
| Exact `3874326b` | Required OS backdrop regression | 1 | `D:\Megle\.tmp\visual-check\logs\task8-review-2026-08-08\17-exact-os-backdrop-summary.json` |
| Exact `92d565a9` | Exhaustive interaction sweep | 1 | `D:\Megle\.tmp\visual-check\logs\task8-review-2026-08-08\20-exact-final-os-exhaustive-summary.json` |

The dirty notification run generated and validated its own media root, observed
the exact root in the `auto-added root` startup line, and ended with no startup
or hard failure. It retained two classified Electron sandbox console entries
and four expected cancelled navigation requests in the JSON.

The dirty OS run reported `run.ok: true`, `required-os-composited: passed`, an
empty hard-failure list, and no startup problem. It retained two classified
Electron sandbox entries and 21 expected `net::ERR_ABORTED` selection/navigation
requests. The successful run covered compact full/failure/empty/degraded and
narrow states, anchor relocation/remount, full notification status/history/logs,
failure drill-down, degraded/narrow layouts, preview fit/actual modes, secondary
windows, and the production appearance matrix.

Both exhaustive runs completed all 18 named scenarios with no scenario failure:
startup/library load; Settings and Plugins secondary windows; library switcher;
notifications; filter, sort, and layout sweeps; child-content toggle; subfolder
cards; search/clear; tree switching; zero-result restore; preview toolbar;
Settings grid controls; and media, folder, and root context menus. This includes
normal, disabled, danger, edge-clamped, rename, move, recycle, permanent-delete,
and confirmation behavior. The dirty run still exited 1 because it recorded four
Core task-persistence startup lines and two Electron sandbox console errors. The
exact final run verified `http://127.0.0.1:5178/`, completed 18/18, and had zero
scenario, console, and hard failures, but exited 1 because two Core
`thumbnail source changed while processing` startup lines remained. Success is
therefore gated by startup, console, hard, and scenario diagnostics rather than
being assigned unconditionally.

## Controlled Media

Both harnesses resolve the repository from `import.meta.url` and validate the
repository entrypoints before launch. Every run creates a unique ignored
`runtime-media/<run-id>/outputs` tree containing deterministic BMP files:

- `landscape-sample.bmp` and `portrait-sample.bmp` for preview geometry.
- `img2img-images/nested-sample.bmp` and
  `txt2img-images/txt2img-sample.bmp` for nested/tree switching.
- Six `img2img-grids/grid-sample-*.bmp` files for zero-result restoration and
  multi-column layout coverage.

The helper validates the marker, BMP signature, dimensions, byte length, and
SHA-256 for every file before Electron starts. No run depends on the old
untracked `.tmp/visual-check/media` directory or an external drive.

## Appearance Matrix

The successful dirty OS run opened a real production Rename modal without
submitting it. For each control it sampled minimum, default, maximum, and final
restored state after CSS transitions settled. Evidence is under
`evidence.settingsInterfaceStyle.appearanceMatrix` in the OS JSON.

| Group | Controls | Min / Default / Max | Production result |
| --- | --- | --- | --- |
| Side | blur, opacity, overlay strength, saturation, stroke opacity | `0/1/2`; saturation `1/1.55/2.2` | Main left/right mapped values and rendered fill/filter/border effects changed; center/dialog remained stable |
| Center | blur, opacity, overlay strength, saturation, stroke opacity | `0/1/2`; saturation `1/1.55/2.2` | Center mapped values and rendered effects changed; side remained stable; intentional center saturation/stroke dialog mappings were included |
| Dialog | blur, opacity, overlay strength, backdrop dim | `0/1/2`; dim `0/0.52/0.85` | Modal surface/backdrop mapped values and rendered effects changed; side/center remained stable |

All 14 entries had mapped production-variable changes and rendered-effect
changes. Surface custom values changed for every entry except backdrop dim,
which maps directly to the modal backdrop background; that background effect
changed. Every declared unaffected group was byte-for-byte stable across
min/default/max. All sliders and all production groups returned to the initial
state at the end.

## DPI And Preferences

The desktop Electron target did not apply `Emulation.setPageScaleFactor`: both
protocol visual viewport scale and renderer visual viewport scale remained 1.
The harness therefore does not claim that sending that command changed scale.
It applies controlled CDP device metrics and hard-asserts the effective
`window.devicePixelRatio` instead:

| Controlled state | Effective DPR | Viewport | Result |
| --- | ---: | --- | --- |
| 100% | 1.00 | `784 x 655` | Anchor contained and reset confirmed |
| 125% | 1.25 | `784 x 655` | Effective DPR asserted; anchor contained |
| 150% | 1.50 | `784 x 655` | Effective DPR asserted; anchor contained |

This is controlled DPR evidence, not a claim that Windows display scaling was
changed. The agent did not change Windows Settings or relaunch the desktop at
actual OS 100%, 125%, and 150% scaling. Real system-scaling titlebar, corner,
menu, and physical anti-aliasing checks remain manual.

CDP media emulation was also asserted rather than assumed:

- `prefers-reduced-motion: reduce` matched `true`; animation name was `none`,
  duration `0s`, and transform `none`.
- `prefers-reduced-transparency: reduce` matched `true`; the surface was opaque
  `rgba(18, 24, 30, 0.98)`, and all three relevant root/child backdrop-filter
  records were `none` with `filter: none`.
- Chromium cannot safely emulate an engine that reports backdrop-filter as
  unsupported. The unsupported-engine fallback remains statically covered by
  `check:ui-design` but was not visually executed.

## Native Evidence

The successful OS run used the Windows desktop capture path, not
`Page.captureScreenshot`, for compositor evidence:

`D:\Megle\.tmp\visual-check\screenshots\ui-window-backdrop-evidence-megle-visual-msjhd3yn-40428.png`

The file exists at report time. Its JSON records `maxDelta: 23.9`,
`bottomMaxDelta: 32.88`, and `uiSurfaceDelta: 169.07`; the OS verification status
is `passed`. This is backdrop-composition evidence only. It is not physical DWM
text/corner anti-aliasing evidence. Generated screenshots remain ignored and
are not committed.

## Residuals

- Exact notification-only auto-add succeeded but exited 1 on two
  `discarding worker error for stale task attempt` startup diagnostics.
- Exact OS/full reached the controlled notification fixture but exited 1. The
  timeout diagnostic is
  `D:\Megle\.tmp\visual-check\logs\task8-review-2026-08-08\17-exact-notification-failure-timeout.json`.
  The page was complete at `?state=failure`, the fixture shell and `failure`
  state were present, `#notifications-root` was absent, and there were no
  fixture console or network errors. This is not the old landscape-preview
  timeout, and exact OS/full is not marked feature-complete.
- Exact exhaustive completed all scenarios but strictly failed the remaining
  Core task-persistence startup diagnostics. No assertion was removed or
  downgraded to make it pass.
- Actual Windows 125%/150% scaling, unsupported-backdrop visual fallback, and
  physical display anti-aliasing remain unverified limitations.

No Task 1-7 production source was changed to hide these residuals.
