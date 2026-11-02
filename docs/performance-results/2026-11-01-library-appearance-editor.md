# Library appearance repair and editor acceptance

Date: 2026-11-01. Status: implemented and verified in isolated Dev and packaged Windows runs. The user's already-running old development Core is preserved and requires normal restart to load the repair.

## Failure and correction

The actual development renderer uses an HTTP origin distinct from Core. Its icon/color PATCH was rejected by browser preflight because Core's allowed-method list contained `GET,POST,PUT,DELETE,OPTIONS` but omitted PATCH. Owned reproduction recorded `MethodDisallowedByPreflightResponse`, and the requested metadata did not change. Evidence: `.tmp/library-appearance-v2/switcher-review-1793484790699/network-repro.json` and `before-fix-cors-save-failure.png`. The earlier provisional saved-but-stale diagnosis was invalid and is explicitly excluded from runtime proof.

Core now permits PATCH within the existing exact-origin/header policy. The new real-router regression failed on the missing method before the fix and passed afterward, alongside the existing CORS test (2/2). It also verifies rejected alternate origins/methods and unchanged authentication requirements. Formatting and release build passed; no origin wildcard or renderer security bypass was added.

A separate metadata-notification gap was repaired: successful appearance updates publish registry-only invalidation, and the real App titlebar reads authoritative Library identity through a generation/unmount/phase-fenced consumer. This updates titlebar/list/Settings without a media-context transition. The production App fixture exercises delayed/out-of-order responses, inactive edits, phase switches and cross-window invalidation. Removing its generation guard deliberately reproduced the late-response regression; the correct guard is restored.

## New interaction

One shared editor replaces the old cascading/absolute pickers in the Library switcher, Settings and creation form. It contains a live preview, separate icon and color selection groups, Reset, Cancel and Apply. The sixteen glyph keys, eleven palette values, white default and saved custom values remain compatible.

Choices and Reset change only the local draft. Apply submits one pair, prevents duplicate submission, and closes on success; failure retains the chosen values and a retryable error. Creation applies only to its parent form draft, with persistence still owned by the existing create flow. The selection patterns and project-specific commit decision are documented with official references in [the design contract](../superpowers/specs/2026-11-01-library-appearance-editor.md).

Visual review caught a dark-custom-color usability defect: coloring the entire glyph grid made choices difficult to see. The grid now uses the theme foreground while preview/persistence retain the chosen color. Selected swatch checks compare light/dark contrast using the browser-parsed color, including custom CSS values, without normalizing saved data. Network failures receive readable Chinese connection/retry feedback. Mounted regression covers dark custom values, white selection and an unchanged submitted pair.

## Development-mode acceptance

New isolated run: `.tmp/library-appearance-v2/switcher-review-1793485519644/`. Actual renderer origin `http://127.0.0.1:59631` used Core `http://127.0.0.1:62042`; successful PATCH returned 200. `running-source.json` identifies the loaded corrected Core binary and records its full SHA-256.

`native-main.json` and `native-settings.json` record:

- All 27 choices update drafts with zero writes; Apply commits one complete pair and immediately updates the actual titlebar and Library row.
- Inactive Library edits do not repaint the active titlebar, and Tab stays with the editor rather than toggling background sidebars.
- Main and Settings windows synchronize in both directions.
- A deliberately blocked request keeps the draft/error; retry succeeds. Unknown saved glyph keys and custom colors survive edits to the other field.

The owned Dev run was closed normally. The unrelated existing user Dev process and its data were not changed.

## Final checks and package

Final full Web passed **508/508** after the contrast and network-message corrections. Desktop passed **233/233**; type/boundary and UI-design checks passed. Core's focused CORS regressions passed **2/2** with formatting/release build green. The first Web run had 505/506 because an older source guard required App's identity clearing to remain inline; it was updated to assert the actual extracted hook contract, its seven focused cases passed, and the subsequent complete 506/506 run passed before the final contrast additions. No runtime failure was hidden by that source-guard update.

The packaged run at `.tmp/library-appearance-v2/switcher-review-1793485853837/` recorded actual renderer/Core origin `http://127.0.0.1:52662`. Main/Settings saving, both directions of synchronization, retained custom values and failed-request recovery passed on the distributed UI and corrected Core. Final release/package inventory, generated trust and Core identity agree; detailed source/package audits are in `.tmp/library-appearance-v2/check-report.md` and its referenced JSON files.

Main independently reviewed these final images in that run directory:

- `final-contrast-editor-os-dark.png`: neutral glyph choices with an unchanged dark custom preview and visible light swatch check.
- `final-contrast-white-os-light.png`: white selection with dark check, keyboard focus and shared pointer highlight.
- `settings-connection-error.png`: readable Chinese connection error, retained draft and reachable retry action.

OS captures use the exact owned window with five `WindowFromPoint` ownership samples before `CopyFromScreen`. Normal, compact, enlarged, narrow and short editor geometry checks passed, including scrollable content and reachable footer actions. On the final package, adaptive/waterfall/grid/list each retained the selected media, exact scroll position and the same production grid DOM instance after an appearance save; `media-preservation.json` records the results using eighty owned PNG fixtures.

All reviewer-owned launchers and child processes exited normally; `cleanup.json` records zero remaining owned processes. The unrelated user development application remains running. Final runtime/test/build inputs match the verified package; subsequent edits are main-owned documentation only. No complete repository npm/Rust/Linux gate is claimed for this focused follow-up. Detailed logs/reports remain under `.tmp/library-appearance-v2/`.

## Loading the fix and evidence limits

The already-running old Core cannot hot-reload the CORS change. Close all old Mangoo development application windows, wait for normal exit, then run `start-desktop.cmd` again. That default entry rebuilds debug Core before launching; refreshing only the frontend does not load the server correction. Explicit external-Core/custom-binary configurations have their own lifecycle.

The baseline is `.tmp/task-baselines/20261101-library-appearance-v2` (338 dirty paths). Final delta audit retains 312 baseline paths byte-identically, updates 26 intended baseline paths and records five newly dirty paths (not all newly created files). Staging is byte-identical; no commit was made. Native mutations use owned fixtures only. Renderer/OS captures do not certify physical-monitor antialiasing, Linux or real network-share behavior.
