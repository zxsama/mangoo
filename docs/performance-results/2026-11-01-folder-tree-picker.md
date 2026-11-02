# Folder tree guides and expanded identity picker

This follow-up is complete: 502 grouped identities, a denser picker with working unobstructed scrollbars, and colored terminal-only folder guides aligned with their expansion icons. Final Web/Desktop/UI checks and isolated Electron/Core interaction/restart acceptance passed. Earlier 384-icon runs and all-child-connector screenshots are retained as historical evidence.

## Implemented behavior

- The shared picker contains 502 identities in 20 named groups. All previous 256 identities and the intermediate 384-icon collection retain their stable keys and artwork. The new character series contains A–Z, a–z, plain and square 0–9, uppercase and lowercase Roman 1–12, and 22 common/financial Chinese numeral forms. Circled 0–9 remains available in the numbers group.
- The picker keeps 18px artwork in 28px minimum-width/height targets with zero grid gap. Its viewport has a 172px maximum height and extends 16px into the popup's right padding, keeping the entire scrollbar hit area outside the icon targets. Short categories shrink to their content. The global 5px thumb, 5px edge gap and 10px invisible interaction rail are unchanged.
- Folder content owns selection and pointer highlighting; the separate expansion slot is excluded. Confirmed leaves use an alignment spacer. Unknown, loading, failed or paged child state preserves expansion/retry, and fresh positive parent metadata invalidates a contradicted completed-empty child cache without an eager child read.
- Continuous 1px tree trunks use the owning parent's icon color. Carets use their own folder color. Intermediate rows have no horizontal branches; only the terminal row has a short bend. Each trunk's center aligns with the actual parent caret SVG center. Paging, collapsed branches and pinned ancestors retain their existing lifecycle and clipping.

## Asset provenance and distribution

The 128 pictogram additions use the existing pinned official Fluent source. The 118 character identities use deterministic SVG outlines generated from Noto Sans CJK SC Medium 2.004 at source commit `f8d157532fbfaeda587e826d4cd5b21a49186f7c`. The input font SHA256 is `ca094f6b0001fb048ca39ddd797a0cdb0179e1e55c6561e111c49c3e6a61d7b7`; fontTools 4.60.1 reproduces the checked-in paths byte-for-byte.

Character identities have separate provenance from Fluent artwork. No runtime font, package dependency, icon request or platform-font redistribution is introduced. Full Noto copyright/OFL/provenance ships through `apps/web/public/LICENSE-NotoSansCJK.txt`. Source notice, public file and built Web output match SHA256 `ee6ecf775b972f629b50f449f09c6fa2e19324fbac5b73d45d61ffda0edee9ad`. Desktop runtime checks confirm the Web output inclusion route; a complete release package was not rebuilt.

Isolated Vite builds compare the original 256-icon catalog/artwork/renderer mapping with the 502-icon equivalents on the same UI source, before the small shared scrollbar-ownership repair. JavaScript increased from 1,629,147 to 2,066,850 bytes; per-file gzip totals increased from 490,848 to 622,430 bytes. The measured catalog contribution is therefore +437,703 JavaScript bytes / +131,582 gzip bytes; these totals are not an exact post-repair release-bundle measurement. Evidence: `.tmp/folder-tree-picker/bundle-502-size.json` and `size-review-502.mjs`.

## Verification evidence

Focused artwork checks passed 10/10. Independent catalog review verified 502 entries, 20 groups, 118 typographic bounds, Core-compatible stable keys, prior geometry preservation, generation parity and license distribution. Vite build and five desktop runtime packaging tests passed.

Mounted picker checks measured 11 columns in a 344px popup and eight in a 280px fixture. Cells measured 28.171875×28 and 30.75×28 CSS px respectively; minimum clearance beyond the full 10px interaction rail was 0.859375 and 0.84375 CSS px. Grouped keyboard navigation retains half-cell row tolerance so the existing 1px pointer lift does not split a visual row.

Mounted tree checks cover model transitions, selection ownership, known leaves, lazy cache recovery, paging, colors, terminal-only bends, collapse and pinned clipping. Normal, narrow and 125% CSS geometry each measured zero caret-to-trunk center error. The geometry test initially caught inherited button padding moving the actual SVG 1.5px off-center; zero disclosure padding and half-stroke positioning corrected it.

Final gates after the shared scrollbar-ownership repair passed: Web **525/525**, Desktop **233/233**, development-URL tests **8/8**, UI-design, boundary validation and TypeScript checks. Logs: `.tmp/folder-tree-picker/check-web-post-ownership.log`, `check-desktop-post-ownership.log` and `check-ui-design-post-ownership.log`. These supersede the pre-repair Web 524/524 and Desktop 233/233 runs. Final native acceptance is recorded below.

The first 502-icon run had passed 522/524: creation-popup Tab focus and a video-hover time assertion failed. Temporary instrumented playback and the subsequent focused 2/2 run passed. Bounded failure-only input/focus/media diagnostics were added to the two fixtures; product behavior, input sequencing and all assertions were retained. The earlier intermittent failures did not recur in either subsequent full gate, and their cause remains unproven. See `.tmp/folder-tree-picker/resume-gate-report.md`. The older 384-icon results and all-child-connector screenshots are historical.

Native creation draft/group navigation passed. Normal and narrow actual tree geometry confirmed terminal-only bends, parent colors and caret-center alignment. OS-composited captures are available under `.tmp/folder-tree-picker/folder-tree-picker-review-1793543723173/`. The files named dark/light have identical bytes and establish layout/state, not a changed-backdrop comparison.

Native scrollbar testing found a real ownership defect: pointerdown hit the body-mounted global scrollbar thumb, but overlay outside-click containment treated that thumb as external and closed the popup. Evidence: `native-popup-rail-diagnostic.json` in that run. Exact live rail/thumb ownership is now registered with a WeakMap and resolved before normal overlay containment. One rail pointerdown path covers the thumb and full invisible target, retaining grab offset and pointer cancellation. Dimensions, RAF scheduling and the single controller remain unchanged.

The new actual-popup mounted regression failed before integration with `Owned scrollbar pointerdown dismissed its popup`, then passed after the repair. Related overlay tests passed 42/42, with typecheck and UI-design passing. Independent source review accepted the repair. Tests cover vertical/horizontal thumb/rail drags, nested popup ownership, parent and background rails, ordinary outside dismissal, focus/context menus, hidden/disconnected/disposed owners and unregistered lookalikes. Evidence: `.tmp/folder-tree-picker/scrollbar-popup-ownership-report.md`.

## Final application acceptance

All phases passed in `.tmp/folder-tree-picker/native-final-summary.json`; details are in `resume-feature-report.md`. The actual application used an isolated development Electron/Core instance and the unchanged approved Core binary. Final product source remained frozen throughout the post-repair gates and native pass.

- Actual pointer input selected all 502 icons and matched their rendered SVG paths. All 20 groups, bilingual/character search, partial/group-transition keyboard rows and final-item reachability passed. Standard, compact and 125% windows show 11 columns; the 320px narrow window shows nine. The separate mounted 280px popup has eight columns. Native full-rail clearance measured 1.016px normally and 0.600px at 125%.
- Rightmost icons and scrollbar drags work without dismissing the popup. At 125%, the icon area and outer body scroll independently, HEX remains editable, and custom values save/reopen. Unknown saved keys, reset/cancel and the single focus boundary remain valid.
- Tree selection excludes the caret; toggles do not select. Known leaves, live colors/reset, rescan-restored expansion, 500→507 unique siblings, terminal transfer, descending sort and pinned clipping passed. Actual 125% geometry measured zero trunk-to-parent-SVG center error. Model/mounted tests separately cover unknown/error/retry/context transitions.
- Sidebar-only expansion/paging across 505 leaf rows issued zero leaf-child reads. The earlier whole-run count included 70 pre-existing center-pane cover probes; the final assertion isolates sidebar work, and cover behavior is unchanged.
- Six distinct identities saved and reopened: Library uppercase/lowercase A, root uppercase/lowercase Roman 8, and folder 三/叁. A fresh Electron/Core process restored the final `letter-lowercase-a`, `roman-lowercase-8` and `chinese-financial-3` values, including the visible titlebar identity.
- Adaptive, waterfall, grid and list modes preserve the grid, media selection and scroll position through folder appearance/sort changes and preview return.

Representative inspected OS captures under `folder-tree-picker-review-1793543723173/`: `tree-terminal-normal-os-dark.png`, `tree-terminal-125-os-dark.png`, `picker-normal-os-dark.png`, `picker-125-os-dark.png`, `roman-lowercase-os-dark.png` and `chinese-numerals-os-dark.png`. Settled category captures wait for the dropdown to unmount. Tree dark/light pairs are identical and do not prove a backdrop-change response or physical-display antialiasing.

## Boundaries and preservation

No Core/API/schema change or new pointer/scrollbar controller is part of this follow-up. The existing shared scrollbar and overlay containment receive the narrow ownership repair described above. Full workspace Rust, packaged-release, Linux/Docker and physical-display antialiasing acceptance are not claimed. OS captures validate actual window material, layout and state; CSS geometry checks alone do not prove OS composition.

The dirty-worktree baseline is `.tmp/task-baselines/20261101-folder-tree-picker` (371 paths). The final audit preserves 351 baseline paths byte-for-byte, with 20 scoped changed baseline paths, 16 newly dirty paths and unchanged staging; see `.tmp/folder-tree-picker/task-delta.json`. Owned processes were closed. `resume-final-cleanup.json` confirms all five protected user processes retain their original IDs/start times and no owned instance remains. The parent all-file task remains `in_progress`; no staging, commit or unrelated rollback was performed.
