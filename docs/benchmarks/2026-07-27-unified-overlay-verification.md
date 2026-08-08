# Unified Overlay UI Verification

Date: 2026-08-09

This report records the final Task 8 verification for
[`2026-07-27-unified-overlay-ui.md`](../superpowers/plans/2026-07-27-unified-overlay-ui.md)
against
[`2026-07-27-unified-overlay-ui-design.md`](../superpowers/specs/2026-07-27-unified-overlay-ui-design.md).

## Immutable Scope

- Exact visual verification range: `28ade3c08157c14346179839f759b8f6965019ce..0a702b506abcf64b64dc44be921d851bba403159`.
- Exact visual source snapshot: `0a702b506abcf64b64dc44be921d851bba403159`.
- The range includes the committed Core compile baseline and Core race fix ancestry (`98b3036`), harness stabilization (`eb19bcd`), expected stale-attempt log demotion (`f7917e8`), backdrop frame-settle fallback (`b371960`), and settings-window refocus before the pointer probe (`0a702b5`).
- `f7917e8` changes only the severity of two already-handled stale task-attempt messages from warning to debug; task ownership and state transitions are unchanged. This prevents benign late worker races from being classified as startup failures.
- Final archive: `D:\Megle\.tmp\task8-exact-0a702b5-20260809-final2.tar`.
- Final archive SHA-256: `FEF7B46F8CFF1474C5E7BB60A4F546398042A261AC322E3B0FAC5912B331E3FB`.
- The archive was extracted as a normal directory to `D:\Megle\.tmp\task8-exact-0a702b5-20260809-final2`; it was not a worktree, junction, or symbolic link and was retained. Earlier `f7917e8` and `b371960` archives were also retained.
- No `npm install` or `npm ci` was run. Existing dependencies from `D:\Megle\node_modules` were reused through the local Electron shim.
- The live worktree retained the user's 17 modified and 12 untracked files. The index was empty before and after the verification commits.
- `apps/web/src/app/App.tsx` remained SHA-256 `5CDB9648949EF64AD2612D53C7029635EB5EFCC6F15F6305780454CB3F965A56`.
- Settings and Plugins production sources were unchanged.

## Final Status

The exact committed tree is green for the required Task 8 harnesses and full
repository gates. All required visual runs exit 0 with `run.ok=true`, zero
startup failures, zero unexpected console failures, zero unexpected network
failures, zero hard failures, and zero scenario failures where applicable.

The successful exact evidence is in
`D:\Megle\.tmp\visual-check\logs\task8-final-2026-08-09`:

| Command | Exit | Result | Evidence |
| --- | ---: | --- | --- |
| `node --test .tmp\\visual-check\\task8-harness-reproducibility.test.mjs` | 0 | 7/7 passed, including stop-before-final-diagnostics, generated-root validation, and one-open notification handshake contracts | `30-exact-0a702b5-harness-tests.log` |
| `$env:MEGLE_VISUAL_NOTIFICATIONS_ONLY='1'; node .tmp\\visual-check\\desktop-ui-regression.mjs` | 0 | controlled notification mode; startup/console/network/hard failures 0; raw known console 2 and expected teardown network 11; generated media 10 | `29-exact-0a702b5-notification-only-summary.json` |
| `$env:MEGLE_VISUAL_OS_BACKDROP='1'; node .tmp\\visual-check\\desktop-ui-regression.mjs` | 0 | required OS-composited backdrop; startup/console/network/hard failures 0; raw known console 2 and expected teardown network 27 | `28-exact-0a702b5-os-backdrop-summary.json` |
| `node .tmp\\visual-check\\desktop-ui-exhaustive-interaction-sweep.mjs` | 0 | 18/18 scenarios, scenario failures 0, startup/console/hard failures 0 | `31-exact-0a702b5-exhaustive-summary.json` |
| `npm test` | 0 | structure, API, client, notification contract, desktop, web, UI design, schema, and Rust gates passed | `32-exact-0a702b5-npm-test.log` |

The exact `npm test` counts were notification contract 45/45, desktop 222/222,
web 369/369, and Rust 472 passed, 0 failed, 2 ignored. The current dirty WIP
tree also passed `npm test` before this document edit: notification contract
45/45, desktop 222/222, web 373/373, and Rust 479 passed, 0 failed, 2 ignored;
evidence is `D:\Megle\.tmp\visual-check\logs\task8-final-2026-08-09\33-current-tree-npm-test-pre-doc.log`.

## Lifecycle Regressions

The notification fixture uses an explicit deterministic handshake. After each
navigation the harness waits for `ready=true` and the requested fixture state,
focuses the fixture target, issues exactly one open request, and requires the
same request ID to be both requested and rendered with `open=true` and
`#notifications-root` present. The exact notification-only run covered the
`status`, `historyInitial`, `history`, `logsGap`, `logs`, `cleanup`, `narrow`,
and `degraded` fixture states, and reported zero fixture console/network errors.

Notification-only finalization stops the child process, waits for output
delivery, then collects stdout/stderr and all startup, console, network, and
hard diagnostics. Harness test 4 injects an error during `stop()` and verifies
that it is present in the final diagnostics, so stop-time output cannot be
dropped.

The exact notification controlled-media root was:

`D:\Megle\.tmp\task8-exact-0a702b5-20260809-final2\.tmp\visual-check\runtime-media\megle-visual-msksqc3y-47184\outputs`

It contained 10 generated files, including 1600x900 landscape, 900x1600
portrait, nested 72x72 media, one 320x180 txt2img sample, and six 320x180
grid samples. The exhaustive run independently generated and invoked:

`D:\Megle\.tmp\task8-exact-0a702b5-20260809-final2\.tmp\visual-check\runtime-media\megle-exhaustive-msksrgjj-73756\outputs`

Its summary records the harness path, repository root, child cwd, and page URL,
all within the exact archive root.

## Appearance Matrix

The required matrix is stored at
`evidence.settingsInterfaceStyle.appearanceMatrix` in the exact OS summary.
It requires 18 production surface probes:

- Side/shared: library navigation, notification nav, notification body, and their containment mapping.
- Center: notification content, center material, field control, select control and element, readable inner region, selected fill, and selected notification tab.
- Dialog: popover, nested, context, modal, and modal backdrop.
- Unaffected: media grid and preview canvas.

All 18 required surfaces were present. The matrix sampled all 14 appearance
sliders (5 side, 5 center, 4 dialog) at minimum/default/maximum/restored
values. Every sample changed its required production targets, had zero
unexpected changed leaf paths, and restored its initial value. Every declared
unaffected group remained byte-stable. The fixture used production glass and
overlay surfaces and emitted zero fixture console or network errors. The
notification nav and content are asserted as transparent children of the
shared notification body rather than being treated as independent fills.

## DPI And Preferences

The exact controlled-device-scale evidence records effective DPR 1.25 and 1.5,
then reset to 1.0. The protocol visual viewport scale remained 1 in each case;
the harness hard-asserted the renderer's actual `window.devicePixelRatio` after
`Emulation.setDeviceMetricsOverride`, not the page-scale field.

The exact reduced-preference assertions passed:

- `prefers-reduced-motion: reduce` matched; animation name was `none`, duration was `0s`, and transform was `none`.
- `prefers-reduced-transparency: reduce` matched; the surface became opaque and all inspected root/child backdrop and filter values were `none`.
- The appearance matrix used reduced motion for settled sampling and recorded `reducedMotionForSettledSampling=true`.

The required OS evidence was captured by the Windows desktop capture bridge in
`required-os-composited` mode, not by `Page.captureScreenshot`:

`D:\Megle\.tmp\task8-exact-0a702b5-20260809-final2\.tmp\visual-check\screenshots\ui-window-backdrop-evidence-megle-visual-msksnak2-59492.png`

The summary records the native capture process, window geometry, `maxDelta`
37.9, `bottomMaxDelta` 49.4, and `uiSurfaceDelta` 197.08.

## Residuals

- Actual Windows display relaunches at 125% and 150% were not performed; the values above are controlled DPR evidence, not a claim about physical display scaling.
- Chromium could not emulate an engine that reports `backdrop-filter` unsupported; that fallback remains unverified.
- `Page.captureScreenshot` and the captured native image do not prove physical DWM text, corner, or compositor anti-aliasing. Real display AA remains a Windows/on-display residual.
- The raw known Electron sandbox startup entries and expected canceled teardown requests remain recorded in JSON, but are classified as non-failures by the existing exact diagnostics contract.

No whitelist, weakened assertion, reset, checkout, worktree, junction, symlink,
recursive cleanup, package installation, or CI command was used.
