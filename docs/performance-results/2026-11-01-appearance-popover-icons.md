# Appearance popup, Fluent identities and folder metadata — 2026-11-01

The initial rollout below used 66 identities. The compact-picker follow-up expands this to 144; the subsequent grouped-picker follow-up expands it to 256 in 14 visible types. Root appearance and directory sorting are recorded separately in `2026-11-01-directory-preferences.md`.

## Scope and result

The latest user direction retains an anchored popup and changes its artwork/color controls. Library identities now use layered flat vector artwork; the locally bundled Fluent System Icons subset contains 66 choices in 8 searchable categories. General command/navigation icons retain the existing Phosphor module. White remains the default. Presets, the native color chooser and HEX input share a local draft and one Apply action; reset and Cancel do not save independently.

The popup is 344 CSS px wide, compact and collision-clamped, with a 14px/20px heading, small preview and fixed actions. It remains in the trigger's overlay family without a standalone modal scrim. A guarded shared dismissal contract keeps pending saves/native color selection registered. The native color swatch's additional browser edge was removed to retain the single-boundary design.

Ordinary folders gain catalog-owned appearance via migration 0038 and typed PATCH /folders/{folderId}/appearance. Sidebar, subfolder fallback and folder preview consume the same metadata. Scans, rename and move preserve it. RootRecord provider/storage icons retain their existing meaning. Folder-only cache reconciliation avoids media reload, and switching Library clears catalog-local folder-ID caches.

## Final verification

- Official Fluent subset pin and MIT attribution retained, no new package/runtime network dependency; 9 focused artwork/icon tests passed and all 528 catalog size/variant combinations rendered.
- Core API/schema checks passed; Core client 69 tests passed.
- Focused Rust folder storage/Host-context tests: 2 passed; rename and move preservation: 1 each; cargo fmt and release build passed.
- Mounted folder checks passed: legacy/unknown values, paired save and three consumers, no media teardown, reset/cancel/failure retry, switch/stale callback/response fences.
- Mounted Library lifecycle 13/13, creation wizard and expanded registration interaction checks passed.
- Fresh Desktop boundary/type/development URL checks and 233 desktop tests passed.
- Real isolated Dev/Core: all 66 glyphs and 11 preset swatches, zero writes before Apply, held PATCH and pending dismissal, one paired save, inactive Library edit, Settings/main bidirectional refresh, retry after blocked request, custom HEX save/reopen and native RGB chooser save/cancel passed.
- Real folder right-click entry from sidebar/subfolder card, three visible consumers, custom/short HEX/unknown icon retention, Cancel/reset and unchanged Library identity passed.
- Five geometry states and guarded OS-composited dark/light screenshots passed after compact typography correction: normal, compact, enlarged, narrow and short. Root visually inspected final normal state and approved it.

## Gate history and final evidence

The first intermediate Library-only full Web run was 509/512 and is not a passing gate. Registration's native test helper required stable visible hit coordinates after the scrollable catalog change; the targeted retest passed. The video hover vertical-list test passed unchanged in isolated recheck. The final combined log identified the third intermediate failure: a whole-picker SVG-count assertion counted the new shared category selector chevron. It was scoped to identity symbols and the selected-swatch check, retaining the original disabled/radio/selection checks; focused icons tests passed 5/5.

Four-layout media selection/exact scroll/DOM preservation and actual process/Core restart persistence passed. The saved folder ID and leaf/#7cc4ff pair survived restart. Owned-process cleanup passed; the protected user Electron/Core remained running. Desktop/UI gates passed. The final complete check:web passed 514/514 across 72 suites (186.5s), including the earlier hover/registration failures. Final Desktop remained 233/233 with type/boundary/development-URL checks, and UI-design passed again. Thirty-two runtime-file hashes stayed unchanged between the verified snapshot and final audit. No unresolved scoped finding remains. No full Rust workspace, Docker/Linux or whole-repository npm test pass is claimed.

## Evidence and preservation

Implementation reports: `.tmp/appearance-popover-icons/{artwork-report,ui-report,folder-report}.md`. Final independent review: `.tmp/appearance-popover-icons/check-report.md`.

Native run: `.tmp/appearance-popover-icons/popover-review-1793488231085/`; readable-color showcase captures are `final-library-blue-os-dark.png` and `final-folder-blue-os-dark.png`, also inspected by root. Geometry OS captures use `editor-{normal,compact,enlarged,narrow,short}-os-{dark,light}.png`. These prove layout/state/compositing in the isolated run; they do not certify physical-display antialiasing.

Starting dirty baseline: `.tmp/task-baselines/20261101-appearance-popover-icons`, 343 paths. Final delta accounting is recorded in `.tmp/appearance-popover-icons/task-delta.json`; unrelated baseline bytes and staging were preserved. No user Library/profile or existing user Core was modified. The running user's old Core needs a normal close/restart through the launcher to build/load the new folder endpoint.

The final boundary pass caught a missing default FolderGlyph open state. The existing folderShowsOpenIcon model was restored unchanged: selected known-leaf or expanded parent uses pinned Fluent FolderOpen; custom symbols remain stable. Seven mounted actual Sidebar states and eleven focused tests passed. The original guard was preserved.

## Follow-up: compact picker and expanded identities

The selectable glyphs changed from 24px to 18px, cells from 40px to 32px high (32px minimum width), and gaps from 4px to 2px. The standard 344px popup now fits nine columns instead of seven, while retaining its 172px icon viewport. Responsive layout retains seven columns in the tested narrow 320px window. Preview, titlebar, sidebar and other consumer sizes are unchanged.

The catalog now contains 144 identities in eight categories, adding 78 meaningful icons with Chinese/English search. All original 66 keys, ordering and filled/regular paths are preserved. New artwork uses the same pinned official Fluent MIT source, with no package dependency or runtime download. Isolated Vite builds measured an increase of 197,586 JavaScript bytes / 59,147 gzip bytes.

Final checks passed: 10 focused artwork tests, mounted responsive/keyboard/draft checks, Web 517/517, Desktop 233/233, UI design and boundary/type checks. Actual isolated desktop checks covered all 144 hit-tested symbols and paths, all eight categories, bilingual search, measured arrow movement, End/wrap/last-row reachability, creation and Library/root/folder save/reopen. Dark/light OS captures cover normal, compact, enlarged and narrow states. Enlarged-body scrolling reaches the last color and HEX input while actions remain fixed. Unknown-key color-only retention and combined keyboard/pointer single-boundary checks also passed.

Native color opening/parent retention passed; this follow-up does not claim a fresh native RGB-input pass because the test's foreground ownership guard rejected input. Existing mounted color tests passed and color implementation is unchanged. No Core/API/database code changed, and no new Rust, package-release or Linux/Docker gate is claimed.

Evidence: `.tmp/icon-picker-density/check-report.md`; OS captures under `.tmp/icon-picker-density/icon-density-review-1793529820833/`, including `density-normal-os-dark.png` and `enlarged-controls-os-dark.png`. Baseline: `.tmp/task-baselines/20261101-icon-picker-density` (371 dirty paths). Final process cleanup and byte/staging accounting are recorded in the review report and `.tmp/icon-picker-density/task-delta.json`.

A fresh Electron/Core process also reloaded the saved new `robot` identity for the Library, root and ordinary folder. All owned native processes were closed and all five protected processes remained. Final audit preserved 362 baseline files byte-for-byte, with nine scoped modified paths, no newly dirty paths and unchanged staging. No commit was made.

## Follow-up: 256 identities with visible type groups

The collection now contains 256 identities, adding 112 distinct official filled/regular pairs. The previous 144 keys, flat order, labels, keywords, source identities and paths are preserved; only their category assignments changed where appropriate. Independent provenance validation matches new SVG paths and hashes to the existing pinned official Fluent source.

The 14 visible types are collections/reading (19), media (21), design (21), office (19), learning/science (18), people/social (19), home (18), food/drink (15), sports/games (15), animals/plants (12), nature/weather (17), buildings/places (12), travel (23), and devices/technology (27).

All icons and search results render compact type headings and per-type grids. Empty groups are omitted; the category dropdown filters to one type. Every icon appears once, in category order and stable within-type catalog order. An unknown saved current key remains available separately without being silently assigned to a real type. One icon radiogroup, one roving Tab entry and the shared 172px scroll area remain; 18px glyphs, 32px target height/minimum width and 2px gaps are unchanged.

Vertical keyboard movement uses viewport row coordinates and the nearest horizontal target across group headings and partial rows. Horizontal arrows and Home/End use grouped DOM order. No extra group panel boundaries, runtime asset requests, package dependencies, consumer-size changes or Core/API/schema changes were introduced.

The mounted grouped interaction sweep passed for all 256 icons and 11 swatches, search/filter/empty/unknown values, normal/narrow/enlarged geometry and cross-group/partial-row keyboard behavior. A creation fixture still expected the old flat-order next icon; its one assertion now checks the next rendered radio and checked state, retaining existing preview/draft/create checks. The expanded all-choice fixture receives a bounded 100-second follow-up deadline with a 110-second outer test limit; ordinary overlay tests keep their previous 30-second deadline.

Final isolated Vite builds measured the catalog/artwork contribution relative to the 144-icon baseline: +304,084 JavaScript bytes and +100,179 gzip bytes. Evidence is under `.tmp/icon-picker-groups/`, including catalog-report.md, ui-report.md and the final independent check-report.md. The fresh dirty baseline is `.tmp/task-baselines/20261101-icon-picker-groups` (371 paths).

Native mixed pointer/keyboard review found that the existing one-pixel hover translation could split a clicked icon into a false row under a strict top-coordinate threshold. Row matching now tolerates offsets below half the cell height while keeping adjacent 34px rows separate; the shared hover CSS is unchanged. A regression reproduces click-then-ArrowUp at normal, narrow and 125% geometry, including partial rows, and the focused grouped fixture passed again with that case included.

Final Web after this fix passed 518/518, with Desktop 233/233, UI design, types and boundary checks also passing. Actual creation and Library/root/folder interaction passed: all 256 rendered icons were hit-tested/clicked and matched to their exact SVG body paths; all 14 type sections, bilingual/grouped search, empty results and category filters were verified. Real pointer-to-keyboard navigation passed in normal, compact, 125% and narrow windows, including partial/group-transition rows and the last choice. The newly appended `usb-port` key saved and reopened through all three metadata editors, with visible titlebar/sidebar updates; creation applies it to its local draft without an early Library write.

Final OS evidence is under `.tmp/icon-picker-groups/icon-groups-review-1793531981711/`, including `creation-groups-start-os-dark.png`, `density-normal-os-dark.png` and `folder-density-os-dark.png`. Main inspected those captures. They demonstrate layout/material/state, not physical-display antialiasing. Earlier failed or pre-fix runs are retained separately and are not final acceptance evidence. This renderer-only follow-up does not claim a new packaged release, full Rust workspace or Linux/Docker acceptance.

Final extra checks passed: 125% body scroll makes HEX hit-testable above the fixed actions, unknown keys survive color-only saves, and combined keyboard/pointer focus keeps a single boundary. A fresh Electron/Core process restored the new `usb-port` key for Library, root and folder. All five protected native processes remained; all owned native processes were closed. The final audit retained 359 baseline paths byte-for-byte, with 12 scoped modified paths, no newly dirty paths and unchanged staging. No commit was made.
