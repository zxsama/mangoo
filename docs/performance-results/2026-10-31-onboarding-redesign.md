# Library creation, icons and typography acceptance

Date: 2026-10-31. Status: implementation and scoped packaged OS review complete; the final broad Web gate remains incomplete. The earlier onboarding/icon/activation full gate passed on its recorded pre-outline source.

## Delivered behavior

The first-run and ordinary Library creation dialogs use a compact 540px form with a name-side appearance button, storage row and primary create/open action. The appearance button opens a separate anchored glass panel containing the existing sixteen icons and ten colors. Repeated explanatory text and routine footer hints were removed. Shared acrylic materials, global pointer highlight and the existing font stack remain in use.

The final typography follows the Windows ramp in [Fluent](https://fluent2.microsoft.design/typography) and [Microsoft's typography guidance](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/typography):

| Role | Size / line height | Weight |
| --- | --- | --- |
| Dialog title | 20px / 28px | 600 |
| Labels, input, path, chooser, errors | 14px / 20px | 400 |
| Primary action, appearance heading | 14px / 20px | 600 |

The previous local styles mixed 21px/680 headings, 12px semibold labels, 13px paths and unspecified line heights. Actual computed styles confirm the final role values. Input placeholder font size/weight are 14px/400; Chromium reports its pseudo-element line height as `normal`, while the input line box is 20px. The computed font-family string is not proof of which fallback face renders individual characters.

All project interface icons now use Phosphor 2.1.10 through a curated semantic module. Selected Library/layout/rating glyphs use native filled variants; line glyphs retain readable small-size weights. The previous library dependency/imports were removed. Stored Library keys, plugin identifiers, fallback behavior and the app logo remain compatible. The SDK includes only its used glyphs without leaking an undeclared icon dependency.

Real creation testing also exposed a renderer integration defect: Core activated the new Library while the title and catalog remained stale. The shared adapter now tears down old context, reconciles authoritative state after local mutations settle and invalidates peer windows on completion. This covers first-run, ordinary and Settings-window creation without restarting Core, reloading the renderer or issuing a duplicate activation. Partial creation/activation failure explicitly preserves the created state, Core cause and list-based recovery guidance.

## Interaction and visual evidence

- Mounted form coverage retains preflight/normalized payload, duplicate-submit and pending locks, picker cancellation, error retry, draft retention, stale-generation/unmount isolation, keyboard behavior and nested overlay focus. Parent trapping defers to its owned child; standalone exported focus-hook use remains supported.
- Typography fixtures cover 100%, 125% and 150% Electron page zoom, plus narrow/short windows and long paths. Role sizes remain stable in CSS pixels; the body scrolls rather than shrinking text. Page zoom is not a Windows display-scale test. Raw before/after measurements and screenshots are in `.tmp/onboarding-redesign/typography/`.
- One scaled error fixture image has inconsistent header-paint observations between reviewers. It is not accepted as proof of complete title rendering; current packaged OS capture is the independent visual check. Renderer captures do not prove compositor antialiasing.
- Real Core/native-folder-picker verification at `.tmp/onboarding-redesign/review-1793445509567/` passed first creation, ordinary creation and separate Settings-window creation at revisions 1, 2 and 3. Main title and catalog updated without renderer reload. Network evidence records no catalog requests before creation and no HTTP errors during that run.
- Four layout modes, preview return, subfolder navigation and compact popup layering passed after fixture preparation. Externally written fixture files required a renderer reload before the layout pass; this evidence does not establish live external-file refresh.
- OS material evidence uses bounded `System.Drawing.Graphics.CopyFromScreen` captures of an isolated owned window over controlled dark/light backgrounds. The existing tool named `desktop-os-backdrop` uses renderer capture and is not treated as OS-composition proof by itself.

## Final gates

`npm test` passed with exit 0 on the frozen source before the subsequent single-outline request: artifact 16/16, SDK 7/7, Core-client 67/67, notification contract 8/8, Desktop 233/233 and Web 501/501. Rust workspace passed 867 tests with 13 ignored (Core unit suite: 854 passed). Structure, API, UI-design, schema, TypeScript and Rust formatting checks passed. Exact counts and source hashes are retained in `.tmp/onboarding-redesign/full-gate-summary.json` and `full-gate-source-hashes.json`.

The final import inventory contains 38 shared icon import sites, 91 semantic exports and 91 explicit CSR imports, with no Lucide source/dependency/lock entry. SDK runtime and declaration checks passed. Earlier combined Web output grew from 754,912 to 979,374 raw JS bytes and from 231,792 to 281,139 per-asset gzip bytes; this is an intermediate comparison, not the final post-outline bundle.

The new global outline request modifies shared visual styles after that full gate. Its current Web/Desktop/UI checks, packaged font/OS evidence, final bundle and release-generated trust are recorded separately; the older gate is not relabeled as an exact post-outline gate. All owned processes from the completed creation runs and full gate were cleaned up.

## Single-boundary follow-up

The later reference image identified a global composition problem: the shared focus rule added 1px and 5px zero-blur shadow rings while the glass pseudo-element added a border and inset bevels. The repair removes those recipes from shared control and overlay styles. Glass has one native 1px boundary; keyboard focus uses a 1px inward outline aligned to that band. Wrapped fields have one focus owner. Borderless controls remain borderless when idle, and the focused edge stays visible.

The existing pointer effect is retained. Cached native border widths align both pointer pseudo-elements with the border box. Glass-button clipping explicitly includes that box; a mounted pixel regression caught and fixed an initial clipping defect, then observed the pointer changing 101 pixels on the native top-edge band. Cold-frame border values are read before inline variables are written, avoiding repeated style resolution from interleaved reads/writes.

The SDK exports the same generic focus and forced-colors CSS. Guard mutation tests reject representative inset bevels, stacked zero-blur rings and thick/outward focus rules. Soft elevation shadows, the existing modal dim backdrop and boundaries of separate parent/child shapes remain valid. The prohibition is now recorded in AGENTS, CLAUDE, the primary UI design specification and Trellis component/quality guidance.

Final post-outline Desktop checks passed 233 tests; SDK passed 7 tests; UI-design checks, Web typecheck/build, the new mounted boundary test, mutation guards and production packaging passed. The final complete Web attempt returned 475 passed, 1 failed and 26 cancelled out of 502. Rename autofocus/selection and LibrarySwitcher operation readiness timed out in setup; the Wizard color ArrowRight assertion failed. These failures remain unresolved and are not counted as environmental skips. The final full attempt's single-boundary and video four-layout tests passed.

Earlier broad Web runs had varying input/focus failures, so their failures are retained rather than reported as green. One instrumented video reproduction captured an unexpected blur with `document.hasFocus() === false` while visible; hover correctly stopped. This explains that captured run, not every earlier failure retrospectively. The rename fixture's fixed delay before asynchronous selection was replaced with a bounded semantic wait, preserving the original selection assertion. No production blur handling was disabled. The new style-only fixture has an isolated profile and focus emulation; real native activation remains separately verified. Wizard page zoom is reset in `finally`, including failed captures.

Current packaged OS evidence was independently inspected:

- `.tmp/onboarding-redesign/review-1793448113452/button-combined-os-dark.png`: direct button-region capture with one thin focused boundary and pointer proximity.
- The same directory's `chosen-combined-os-dark.png`, `input-focused-os-dark.png` and `typography-150-os-dark.png` confirm normal and 150% page-zoom title/form/action rendering. Paired light-background captures demonstrate preserved OS acrylic. Actual Chinese platform faces were Noto Sans SC / Noto Sans SC Bold; the declared font stack is unchanged.
- `.tmp/onboarding-redesign/review-1793448621453/dropdown-os-dark.png` and its light pair show a real expanded notification-history dropdown with the joined shape. Its fill, blur, text wrapping and component geometry rules are unchanged by this follow-up.

An appearance-panel capture was dismissed by the backdrop helper changing focus and is excluded as expanded-panel evidence. The older fixed-delay layout script did not complete a fresh layout pass; prior real layout evidence and current mounted tests are recorded separately. The legacy full visual script was not rerun or labeled passed. Physical-monitor antialiasing and Windows display scaling remain outside these capture claims.

The refreshed local package contains 13 JavaScript assets totaling 981,309 bytes (282,483 bytes summed after gzip per asset). Packaged/release Core bytes and plugin inventories match; generated trust matches release with SHA256 `7ac997909f707162c72a32db79c8107f905c1a482d1b90d431fbab3fd0a7bade`. Detailed hashes, failure logs and cleanup are in `.tmp/single-outline/check-report.md`, `package-inventory.json`, `final-bundle-sizes.json` and `cleanup.json`. Final owned Mangoo/Electron/Core process count is zero.

## Workspace boundary

The preexisting 291-path dirty baseline is retained under `.tmp/task-baselines/20261031-onboarding-redesign/`. The scoped delta is tracked in `.tmp/onboarding-redesign/task-delta.json`. Before the global outline request, stylesheet changes were confined to creation/appearance rules and the shared dropdown changed only its icon import. The outline follow-up has its own 323-path baseline under `.tmp/task-baselines/20261031-single-outline/`: 313 original paths remain byte-identical, 10 changed, and 6 newly dirty paths are recorded in `.tmp/single-outline/task-delta.json`. Staging remains unchanged. Existing thumbnail badge/time values and hover behavior are preserved. No commit, task archive or publication is performed.

The parent all-file task remains in progress. Same-origin completion messages do not add crash recovery: a publisher exiting after Core commits but before sending can miss peer invalidation. Windows compositor text antialiasing and other operating systems are not certified by page-zoom fixtures.
