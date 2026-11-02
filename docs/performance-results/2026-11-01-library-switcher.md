# Library switcher and safe registration acceptance

Date: 2026-11-01. Status: implemented and verified on Windows; final Web, corrected full Rust, packaged native flows and OS visual acceptance passed. Changes remain uncommitted.

Subsequent appearance repair: the later user report exposed a Dev cross-origin PATCH preflight failure and missing visible metadata invalidation. The packaged/metadata checks below did not certify that Dev path. Current repair, replacement editor and end-to-end evidence are in [Library appearance acceptance](2026-11-01-library-appearance-editor.md).

## Scope delivered

- White (`#ffffff`) default Library glyphs, including creation and unset-color titlebar/list/settings rendering; stored custom colors are preserved. The palette retains the existing ten colors and adds white. Sixteen glyph keys remain stable; the curated icon module now contains 92 glyphs.
- Searchable Library rows with a separate more menu, active indication and readable name/path hierarchy; shared child appearance, reset, Explorer and existing folder actions; quiet create/open footer. Native Desktop selection opens directly; Web has manual server-path entry. First-run creation exposes an open-existing alternative.
- Dedicated safe registration APIs, with list visibility separate from retained Library ownership/data. List removal does not invoke destructive local-copy removal. An active removal clears browsing context without selecting another Library; inactive removal preserves the current workspace and uses registry-only invalidation.
- Files, marker, catalog/cache, metadata and synchronization configuration are retained by the removal operation. Already-enabled Host synchronization continues; confirmation explains this. Reopening a retained Library restores its original identity/data. A Library from another Host gets a new local index while source identity/content stay intact; this does not import the other machine's local catalog/appearance.

The exact API, persistence, active-context, synchronization and UI contracts are in [the design specification](../superpowers/specs/2026-11-01-library-switcher-and-registration.md).

## Implementation verification

- Core safe-registration/route/cancellation/shutdown coverage: 12 tests passed. Cases include sentinel preservation/reopen, visibility across restart, conflicting identity/path/local-data collisions, junction/no-follow validation, partial activation failure, owned recovery, stop failure/retry, synchronization retention and request-cancellation/shutdown barriers.
- Existing manager regression sweep: 27 passed, 1 ignored; two heavyweight scenarios were left to the full workspace gate. No filtered process tests are counted as exercised.
- Core-client checks: 68 tests passed; API/schema/structure/format checks passed.
- UI focused suite: 23 tests passed, retaining all 13 old switcher lifecycle cases. The added default-white creation-payload scenario also passed its subsequent mounted rerun. White/custom colors, safe registration/removal, cancel/busy/stale/native-picker blur, child focus and registry-vs-context invalidation are covered.
- The icon suite initially exposed an obsolete ten-color expectation. The reviewer updated the exact eleven-color/default-white assertions and added explicit-custom-color preservation; 4/4 passed. No behavioral assertions were weakened.
- Current full Web gate passed 504/504 with zero failed/cancelled; earlier focus failures from the pre-task baseline did not reproduce in this run. This is current evidence, not a retrospective diagnosis of the older failures.
- The initial full `npm test` passed artifact 16/16, SDK 7/7, client 68/68, Desktop 233/233, Web 504/504, notification checks, structure/API/type/boundary/UI-design/schema. Its Rust segment failed three stale schema/fixture cases, so that command exited 101. The Host schema assertion was updated for migration 0021; two tests now create their Library before terminal Host shutdown while retaining stopped-provider isolation and every original assertion. Each repaired case passed individually, then the complete `npm run check:rust` passed 879 tests with 13 ignored. These are split gate results, not a claim that a second full `npm test` command exited successfully.
- The Settings Library management panel now subscribes to registry invalidation with generation/unmount fences. Mounted coverage and the real second window verified that inactive removal updates the list without tearing down browsing state. A full Web rerun after this fix again passed 504/504.
- After the menu/dialog/error/count corrections, the final full Web gate passed **506/506**, 72 suites, with zero failures/cancellations (`web-final-visual.log`, exit 0). The final serialized focused suite passed 16/16, including all 13 prior switcher lifecycle tests, new registration presentation/count/Settings coverage, the creation Wizard and startup scenario. UI-design passed on this final source.

### Native flow and preservation evidence

The isolated packaged Windows run at `.tmp/library-switcher-redesign/switcher-review-1793470795624/` used real native folder selection and two owned Libraries. Both new Libraries stored `#ffffff`. Inactive list removal retained the same grid instance, selection (12 items), scroll offset (180) and context revision (2); the already-open Settings window removed the row. Reopening restored the same UUID and catalog tag. An ordinary folder was rejected and remained empty; the active Library stayed intact.

Active removal cleared the active context without fallback; another listed Library remained selectable. Removing the last Library left an accessible open-existing entry. Four layout modes, subfolder entry and preview/return passed. `native-data-audit.json` records unchanged SHA-256 values for six marker/source/cache sentinels and 122 PNG files, retained catalog files and tag rows. The recorded requests include three successful registration removals, reopening and ordinary-folder rejection; none used `/local-copy`.

Visual acceptance caught missing shared menu/dialog layout classes and a white swatch without a visible check. These were corrected using `overlay-titlebar-menu` and a `library-operation-dialog` alias for the existing creation-form rules, with a centered contrasting Check on white. Known Core `{error, code}` responses now produce Chinese recovery instructions. Stale titlebar totals after active removal were fixed in the shared context teardown, including slots/ranges/cursor cleanup; the inactive registry-only path remains unchanged. Mounted computed-style/geometry/error/state assertions and the final Web gate passed.

The final refreshed package reopened the same isolated Host after both registrations were hidden. The first-run form exposed opening an existing Library; cancelling the native selector left the titlebar entry usable. Reopening restored the original UUID/catalog/tag. An explicit blue appearance (`#7cc4ff`) survived active removal and reopening; reset returned it to white. Actual active removal produced `active=null`, revision 9 and **0 files**, with no automatic fallback.

### Final visual evidence

The refreshed packaged UI was captured on the actual owned Windows window against controlled dark/light backgrounds. The capture helper verifies five `WindowFromPoint` samples resolve to that exact window before `CopyFromScreen`; early captures showing another window were discarded and are not evidence. Final review confirmed vertical transparent menu rows, white-selected contrast, shared dialog padding/typography, Chinese error recovery and one thin boundary per control.

Files below are in `.tmp/library-switcher-redesign/switcher-review-1793470795624/`; each named OS image has a corresponding light-background capture:

- `final-switcher-os-dark.png`: two-Library list and create/open footer.
- `final-more-os-dark.png`: separate action menu.
- `final-appearance-clean-os-dark.png`: child appearance panel with white selection check, captured without native title tooltip obstruction.
- `final-appearance-125-os-dark.png`: enlarged appearance panel.
- `final-open-invalid-os-dark.png`: ordinary-folder rejection, retained path and Chinese recovery text.
- `final-forget-os-dark.png`: list-removal confirmation and current-Library closure explanation.

Renderer captures are layout evidence. These actual OS captures establish compositing for the tested window/backgrounds, not physical-monitor antialiasing or every display-scaling configuration.

### Initial redesign package and workspace integrity

The final release package was refreshed after all production changes. Release/packaged plugin inventories, generated trust and Core binary match (`package-inventory.json`). Packaged Core SHA-256: `12419a6bc1f4077e9a46c934bec10446a1a8a73c8eb9f9708c436facc8dcb73f`; packaged `app.asar`: `de3728a3a559822531f20296f2f7f8dfcaca66c3e74dc190bd1821b25770f2d4`. Tested and packaged source inventories contain 766 files; subsequent differences were limited to this acceptance document, with production files unchanged.

Both owned launcher trees were stopped after verification; final Mangoo/Electron/Core process count is zero. The baseline audit reports 302 of the original 329 dirty paths byte-identical, 27 intentionally updated baseline paths and 8 newly dirty paths. The staging area remains byte-identical. Scoped `git diff --check` passed; existing CRLF normalization notices were not whitespace errors.

## Preservation and limits

Migration 0021 introduces `library_list_visibility` and a distinct `library_open_journal`. Internal ownership snapshots keep hidden records; public lists filter them. Proven interrupted registration can recover without auto-activation. If a process stops between creating a local directory/file and recording its identity, recovery preserves that unproven collision and reports a conflict instead of adopting or deleting it. Arbitrary power-loss injection is not claimed.

The new accepted-operation/pending-stop shutdown ownership is tested. This is not a redesign or certification of every preexisting normal-active-runtime Host shutdown path. Linux and actual UNC/SMB environments remain untested unless separately recorded below. No Eagle-format import or persistent manual ordering is included.

The 329-path pre-task dirty baseline is retained at `.tmp/task-baselines/20261101-library-switcher/`. No staging, commit, task archive or modification of real user Libraries is authorized or performed. Current detailed reports are `.tmp/library-switcher-redesign/{ui-report,core-report,check-report}.md`; temporary fixtures and captures stay under ignored `.tmp/` paths.

## Follow-up: unified rows and complete button audit

The subsequent request makes each Library row, including its ellipsis, one visual interaction shape and moves appearance reset into the icon/color panel. The row now uses the existing `data-interactive-pointer-owner` contract for one shared highlight and inward focus boundary. The two native buttons retain independent switch/menu actions; keyboard focus identifies the main label or ellipsis without a second edge. Reset is an optional action in the shared picker and restores both `library` and `#ffffff`.

The button audit found and fixed two additional defects. Saving appearance could disable the parent launcher, causing its roving-focus owner to dismiss the child and hide errors; the open launcher now remains focusable while mutation controls stay disabled. Actual packaged Tab input was also intercepted by the global sidebar shortcut. `useShortcuts` now respects already-handled events and registered overlay ownership through `getContainingLayer`. Integration tests mount the real hook with the switcher, reproduce the old failure, and prove popup navigation plus normal workspace shortcuts after dismissal.

### Button outcomes

All current built-in switcher actions were exercised in an isolated packaged Host at `.tmp/switcher-row-unified/switcher-review-1793476833434/`.

| Controls | Verified result |
| --- | --- |
| Search, clear, close; main row and ellipsis | Filtering/empty state/dismissal correct; switching changes active identity; inactive ellipsis does not activate its Library |
| All 16 glyphs, 11 colors and reset | Every choice persisted to real Core metadata; reset exists only in the appearance panel and restores both defaults |
| Create and native directory selection | Required-field disabling, cancellation and two successful distinct Libraries verified |
| Explorer | New native window opened the exact owned fixture path; only that newly opened window was closed |
| Add/remove folder | Cancel preserves roots; add indexes the fixture file; confirmed removal drops the catalog root while retaining original file bytes |
| Safe open | Native/manual retry, picker cancellation, empty-input disabling and ordinary-folder rejection verified; original Library identities restored |
| List removal | Cancel, inactive and active/last cases verified; source sentinels/markers retained and no automatic fallback |
| Keyboard | Tab navigates root/menu/appearance without toggling sidebars; Enter opens more without activation and resets appearance; Escape restores focus; workspace Tab works after dismissal |

Mounted production components additionally cover unavailable/busy/duplicate/stale/error states and failed appearance-save retry. No plugin-contributed button was installed in this fixture, so this is not a live plugin-action claim.

### Final follow-up verification and evidence

After the shortcut repair, full Web passed **506/506**, 72 suites, with type/boundary and UI-design checks green. The original 13 lifecycle scenarios also passed with the real shortcut hook. Desktop passed **233/233** plus 8 dev URL tests; Desktop source did not change afterward. No full npm/Rust gate was rerun for this local Web follow-up.

Web/Desktop resources were rebuilt and the package refreshed. The official release plugin generator corrected debug trust drift; release/package inventory, generated trust and the unchanged Core binary agree. Current source/package audit and process cleanup are recorded in `.tmp/switcher-row-unified/check-report.md` and `package-inventory.json`. Unrelated running dev applications are retained; cleanup applies only to owned test runs.

Final guarded OS captures in the run directory include `final-row-focus-main-os-{dark,light}.png`, `final-row-focus-more-os-{dark,light}.png`, `final-row-menu-os-{dark,light}.png` and `final-row-appearance-os-{dark,light}.png`. Main review confirmed one whole-row boundary, no ellipsis box, reset inside the child panel, and normal sidebar/titlebar visibility. These prove tested layout/compositing, not physical-monitor antialiasing. The fresh 337-path baseline is `.tmp/task-baselines/20261101-switcher-row-unified`; staging remains unchanged and no commit was made.
