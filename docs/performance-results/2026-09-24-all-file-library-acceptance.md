# All-file Library acceptance — 2026-09-24

## Scope and environment

Windows validation of ordinary-file browsing, format artwork/badges, native video hover, and the bundled PPM static-image decoder. All integration runs use generated local files and isolated Host/Library databases. No synchronization account or user media is needed. Existing unrelated worktree changes were preserved; this report describes the combined working tree, not an isolated release commit.

The acceptance scripts are `tools/checks/all-file-library-desktop.mjs`, `tools/checks/file-decoder-integration.mjs`, `tools/checks/video-hover-integration.mjs`, and `tools/bench/file-decoder-resources.mjs`. Temporary outputs are under `.tmp/all-file-library-validation/` and `.tmp/video-hover-integration/`.

## Mixed-file Desktop

The real Desktop run at `.tmp/all-file-library-validation/desktop-zDUaYP/result.json` passed four layouts, nested-folder navigation/default cover, unknown-format preview and return, actual plugin-manager enable/grant, automatic thumbnail update and a 1600×900 central content preview. Disabling retained the verified image; creating another PPM while disabled produced a visible format-artwork card. The renderer capture contained 1,220,980 bytes; the harness calls it `nativeCapture`, but that field does not establish Windows compositor pixels.

Follow-up validation found the sidebar count remained at 13 while the grid/API showed 14 files, even after 15 seconds (`desktop-ASVPFU`). A bounded roots refresh when the complete current-root list count disagrees fixed this without a resident polling loop. The full run at `.tmp/all-file-library-validation/desktop-NBIgmC/result.json` passed again, including matching sidebar and grid counts of 14. The mounted test verifies the same change from two to three files.

An actual small-directory failure led to a production fix: non-windowed layouts must refresh their rendered records when capabilities change even though every file was already loaded. The mounted regression now renders the real grid with an unselected target file, rather than manually invoking its page-request callback.

## Video hover

The real Electron/Core run at `.tmp/video-hover-integration/run-1790187133096/result.json` passed adaptive, waterfall, grid and list layouts. Large-file first frames appeared in 373–475 ms; horizontal seeking reached approximately 9.6 seconds at 80%, then resumed playback when stationary.

- The sparse, decodable source was 537,332,513 bytes. The full repeated scenario matrix transferred 11,269,462 bytes through 40 HTTP 206 responses; hover made no full-original preview request.
- All 40 response bodies closed; 17 closed before EOF. All 23 old grants tested after Library activation returned 404.
- Fifteen lifecycle/fallback cases covered leaving/crossing, scrolling, blur, hidden state, preview/content replacement, offscreen state, context/locate/keyboard/double-click actions, decode failure, missing source and actual Library activation.
- Observed Core peak working set was 46,215,168 bytes. Renderer peak was 124,208 KiB, with 67,976 KiB private memory. This is a bounded-read observation for a sparse valid MP4, not a high-bitrate decode benchmark.

## Decoder process and resources

Real API acceptance verifies content-derived P3/P6 pixels, a 1024-pixel thumbnail and larger generated static preview, invalid-input failure stability, disable/cache retention, source invalidation and same-file re-enable. Additional cases verify recovery across decoder epochs and replacement with equal size/restored modification time.

The resource run at `.tmp/all-file-library-validation/decoder-resources-durZOh/result.json` repeated 2 MiB and 48 MiB inputs with the same 1×1 source pixels and legal trailing whitespace. Increasing source length by 46 MiB increased the maximum observed Core historical peak by 2,506,752 bytes; the corresponding decoder peak difference was zero. Core peaks were 50,266,112–52,908,032 bytes and decoder peaks were 5,193,728–5,197,824 bytes. Each disable response was followed by a PID check confirming child exit. The result records the tested Core SHA-256 and per-run samples.

Six Host tests use actual child pipes for slow credit/large input, malformed output hash, extra output streams, delayed End/deadline, abandoned owner and early control response. Production-task tests use the trusted PPM process, real task queue and DbWriter, pausing after encoding and before publication to exercise plugin shutdown, Library cancellation, source revision, root disable and successful publication. Windows source-pinning assertions check rejection of concurrent write and rename. These are explicit checkpoints; they do not establish every possible failure at every transport phase.

## Platform limits

The prescribed `.tmp/visual-check/desktop-ui-regression.mjs` command ran with `MANGOO_VISUAL_OS_BACKDROP=1`. It passed the compact-notification assertions, then stopped at `sync orchestration lifecycle fixture`. The fixture HTML/TSX/CSS were already deleted in the preserved pre-task worktree, so the timeout is a stale harness dependency rather than a new failure in this task. The original command is not recorded as passing, and the removed files were not restored. Its diagnostic summary is `.tmp/visual-check/logs/desktop-ui-regression-summary.json`, run `mangoo-visual-mueg0z70-17344`.

The Linux/Docker build was attempted twice, including the permitted retry, but Docker Hub TLS negotiation failed while resolving the Dockerfile frontend before compilation. The Docker staging shell was separately syntax-checked and exercised with isolated fixture artifacts, validating both plugin manifests and executable/manifest digests. Linux compilation and process execution remain unverified.

OS resource exhaustion, disk-full behavior and adversarial high-bitrate video decoding are outside the measured cases. Desktop screenshots establish layout and state; they do not establish the physical display's anti-aliasing quality.

## Final gate

The original continuation ended before its final gate. The following dated closeout supersedes that pending status while retaining the historical failures and measurement limits above.

## Continuation closeout — 2026-10-25

The approved parent and four child tasks were resumed from session `01a0cf5c-873e-7ee3-8cf1-653ff8780b46`. Current dirty bytes/hashes/patches were saved under `.tmp/task-baselines/20261025-resume-all-file-library/`; unrelated work was preserved. This is validation of the integrated working tree, not a committed release or production-package rebuild.

### Changes needed to complete verification

- Applied Rust formatting to the 18 reported Core/PPM files and regenerated trust through the normal bundled-plugin build.
- Fixed the cancellation-recovery unit fixture to verify/register the real PPM artifact from Cargo's profile directory. Production packaged-root discovery and trust restrictions remain unchanged.
- Corrected five stale test assumptions introduced by all-file collection: assert all three named fixture files while retaining two-media counts; include ordinary files in scan progress; verify schema 37 and both new tables; initialize preview source identity with a real scan; wait for startup scans before strict notification/outbox assertions. The notification fixture now joins its owned runtime before deleting Windows SQLite files.
- Clarified cancellation recovery, executing-epoch failure provenance and renderer-versus-OS capture evidence in the contract/spec. No product behavior was changed in this continuation to satisfy the tests, and no validator was weakened.

### Current Windows evidence

The final default-concurrency `npm test` exited **0**. Log: `.tmp/all-file-library-validation/20261025-gates/npm-test-final-fixed.log`; exit marker: `npm-test-final-fixed.exit`. Counted tests: **1,663 passed, 0 failed, 13 ignored**. This comprises 796 JavaScript tests (artifact 16, SDK 6, client 67, Desktop 230, Web 477) and 867 Rust tests (Core 854, Core process 4, fake Provider process 4, PPM unit 2/process 3). Structure, contracts, TypeScript, UI-design, schema and formatting checks also passed. The 13 ignored cases are not counted as validated.

The initial format failure and subsequent full run with five failed Core tests remain in `npm-test-default.log` and `npm-test-final-default.log`. All five reproduced alone and were repaired through the fixture/assertion changes above; they are not dismissed as parallel timing. The five exact tests and the 15-test preview group passed before the final whole-repository run.

The gate's normal bundled build regenerated `generated_trust.rs` before its Rust tests; the other 601 frozen executable/validation inputs were unchanged. Both bundled executable/manifest digest pairs matched the generated trust. The post-gate 602-input fingerprint is `552cf9e140b5107e07d32c8bf97c2beb0ff8117563ba8981897478b858c115f6`; final Core executable hash is `3e45bc3a9af972cf5c386de1b5f492638f3a70413ebf840c3e5bfd4f85a9ad4d`. The trusted bundled PPM runtime hash is `b06a2c03d71ca57eae63f2452bb8db77776e69d6f20855fba2b7a05ab38d4aef`, distinct from the standalone Cargo PPM executable. These final artifacts are not conflated with the earlier functional-integration binary below.

- Fresh API: `.tmp/all-file-library-validation/decoder-api-x3eiM7/result.json` verifies actual P3/P6 pixels, 2048×1024 static preview, negative-cache/lifecycle recovery, equal-size/restored-mtime replacement and preserved identity.
- Fresh Desktop: `.tmp/all-file-library-validation/desktop-ODPSpA/result.json` verifies four layouts, unknown-file preview/return, folder navigation/covers, real plugin-manager enable/grant/disable, automatic content upgrade, 1600×900 central preview, new-file artwork and sidebar/grid counts of 14.
- Fresh video: `.tmp/video-hover-integration/run-1792930629852/result.json` verifies four layouts and 15 lifecycle/fallback scenarios. The 537,332,513-byte sparse source transferred 11,269,462 bytes in 40 range responses; all response bodies closed, all 23 tested old grants returned 404, and whole-original preview requests were zero.
- These integrations used rebuilt Core SHA-256 `d308ed1a995acd0529020d38eb0db6a0ca8c3c6e5ec0027611800f530c29eeb8`; later changes were confined to test fixtures/assertions and documentation. Production behavior remained frozen.

### Windows compositing and legacy-suite boundary

The current `tools/checks/desktop-os-backdrop.mjs` passed its source/bridge/renderer assertions. Its `nativeCapture` bytes come from `webContents.capturePage` and do not themselves prove OS compositing.

An independent ignored harness copied the legacy Windows `CopyFromScreen` helper and its pixel thresholds unchanged, bounded to the isolated test window. Actual compositor capture and threshold assertions passed: `.tmp/all-file-library-validation/20261025-gates/all-file-os-1792930801370-98580/result.json` and `os-composited.png`. The captured window's PID/profile and diagnostics are retained.

The prescribed complete legacy command was rerun unmodified with `MANGOO_VISUAL_OS_BACKDROP=1` and still exited **1** at the missing pre-task sync-orchestration fixture, after compact-notification assertions passed. Its fresh snapshot is `20261025-gates/legacy-summary.json`. Neither the current renderer check nor independent compositor check is presented as a pass of that legacy suite. Physical-display anti-aliasing still requires user observation.

### Linux/Docker evidence

The previously blocked `sync-verifier` Docker build now succeeds, including Linux compilation and 11 native process tests. That image copied sources before the final test-fixture corrections, so its packaging result is kept separate from final-source validation.

An isolated verifier with Node then compiled the final mounted sources and passed **25 decoder + 22 all-file + 2 PPM unit + 3 PPM process tests**, with no failed/ignored cases in those selected suites. The Windows-only case explains the all-file count difference. Source hashes and exact commands are in `20261025-gate-review.md` / `20261025-gates/linux-final-focused.log`. The container retains platform-generated Linux trust; this is not a claim that Windows trust bytes were used on Linux, nor that the entire Rust workspace suite ran on Linux.

The first Linux focused attempt lacked Node and failed six child-pipe fixture prerequisites. After Node was supplied, all selected tests passed; its wrapper later exited 1 in a trailing artifact-hash command because of a CR line ending. That failed wrapper result is retained. The hash-only follow-up exited 0 and identifies the packaging image; final-source test output has its own source/build provenance. No release image or application deployment was performed.

Detailed reviews are `.tmp/all-file-library-validation/20261025-core-review.md`, `20261025-ui-review.md`, and `20261025-gate-review.md`. Task status remains open for the legacy-harness/commit closeout; no work commit, push or task archive was performed.

## Integrated dropdown follow-up — 2026-10-25

The user replaced the native-dropdown color-only requirement with a rounded custom popup joined to its trigger, one integrated highlight, and the existing glass language. The follow-up baseline is `.tmp/task-baselines/20261025-integrated-dropdown/`; this remains part of the existing task and preserves unrelated work.

`OverlaySelect` now renders that joined surface through the shared anchored overlay/stack, with real select form and event backing. The same implementation reaches History, process logs, plugin configuration and SDK consumers. Shared placement accounts for the trigger/header, modal children inherit the actual parent, and the global pointer driver owns the complete surface. No feature-specific popup, scrollbar or hover subsystem was added. Native multiple/size listboxes remain a separate compatibility case. The public and lifecycle contract is `docs/superpowers/specs/2026-10-25-integrated-select.md`.

Independent review found an upward-placement CSS specificity issue: the header could render at the list's top instead of covering the original trigger at the bottom. It was fixed before source freeze, with a new rendered geometry assertion. Final mounted coverage also verifies native label activation and clicks without pointerdown when transferring between sibling selects.

### Focused validation and visuals

- Web validation/typecheck and **493/493** tests passed; final mounted dropdown coverage is **13/13** groups, included in that total.
- SDK build and **6/6** tests passed. A separate fixture rendered the built JS and CSS, confirming an 8px popup radius and zero independent descendant pointer targets.
- UI-design validation passed. Focused positioning/stack/lifecycle/focus coverage passed **39/39**.
- Actual History filtering changed the real select to Artists, issued history queries and displayed 120 matching records. Actual Log filtering changed to Main and created the expected subscription. These consumers kept their existing native change-event contracts.
- Six renderer captures cover below/above placement, long-list scrolling, disabled/grouped options, the shared modal, actual History/Log consumers, and built SDK output. Mounted hit-testing and selection establish that the child is above the modal even where the parent's edge remains visible through the glass.

Evidence is under `.tmp/integrated-dropdown/`: `check-report.md`, `mounted-final.log`, `web.log`, `sdk.log`, `ui-design.log`, `consumer-results.json`, and `screenshots/01-integrated-below.png` through `06-built-sdk.png`. Renderer evidence establishes shape, placement, state and highlight ownership; physical-display anti-aliasing remains outside those captures.

### Final gate boundary

Final source/test inputs were frozen before the complete default-concurrency command. Its initial `npm test` exited 1 during bundled-plugin directory staging before Rust formatting/tests: Windows returned EPERM while renaming the staged directory. All preceding gates passed, including Desktop 230/230 and Web 493/493. All 607 recorded executable/validation input hashes were unchanged. The cause of that transient filesystem error was not proven; it is not described as an assertion failure or silently counted as a passing full command. Only the failed `check:rust` segment was retried, preserving default concurrency and the first failure log.

The default-concurrency `check:rust` retry exited 0: **867 passed, 0 failed, 13 ignored** (Core 854, Core process 4, fake Provider process 4, PPM unit 2/process 3). Combined with the initial command's passing Node stages, the executed suites total **1,679 passed**. The full `npm test` command itself still has its recorded exit 1; this is a segmented validation result. All 607 frozen input hashes remained identical before the first run, after its failure, and after the successful retry: `33799f18bad0435a841850047ca174e47c0de18b2814ecd0d8bb1104730fa005`.

One bounded current-build Windows compositor capture passed, using the existing `CopyFromScreen` helper and pixel predicates unchanged. Run `integrated-select-os-1792935962033-59356` produced an inspected 1512×984 capture and passed the acrylic variation predicate (UI-surface delta 35.5). Its isolated window and two owned ports were confirmed closed. This verifies the main-window OS backdrop; the custom dropdown geometry is covered by the separate renderer/mounted evidence above.

That OS dev build refreshed only `crates/core/src/plugins/runtime/generated_trust.rs`; the other 606 frozen inputs were unchanged. Both current bundled binary/manifest pairs matched generated trust (2/2). The post-OS input fingerprint is `f4028caafa417a94b2cc782af48c805ca2b2ce97b12e60a9855ed320f1034b34`; Core executable SHA-256 is `9fa94761e4ba447483846537c7d3a05b7c5dcc42c55e539c9927bcb24bc365bb`. The tests preceded this normal generated-trust refresh; the current artifact digest verification and OS evidence are recorded separately, without claiming a second full exact-tree test run.

Final evidence: `.tmp/integrated-dropdown/final-gate/review.md`, `npm-test-default.log`, `check-rust-retry.log`, input manifests, `post-os-artifacts.json`, `cleanup.json`, and the compositor run directory. The follow-up inventory (`.tmp/integrated-dropdown/task-delta.json`) has 22 paths, six overlapping the 255-path dirty baseline; the other 249 baseline paths retain their bytes/deletion state. Ignored Trellis notes and temporary evidence are excluded from that Git inventory.

No release package replacement, commit, push or task archive is part of this dropdown follow-up. Historical Linux evidence and the known deleted-sync-fixture limitation of the complete legacy visual suite remain separate from this UI verification.

### Opening flicker and spacing refinement

The next user report identified opening flicker and excessive option vertical spacing. A new baseline at `.tmp/task-baselines/20261025-dropdown-refinement/` preserved 271 existing dirty paths and 12 additional clean overlay files. Real RAF traces reproduced the cause: the original select was hidden from `open` before the portal had completed its scheduled layout measurement. Captured first, repeated and above openings had 2, 15 and 1 blank samples respectively; the settled component tests alone had not caught this.

The correction is three product files: `AnchoredOverlay` reports integrated presence after measured layout exists, `OverlaySelect` covers the original from that readiness, and `overlay.css` reduces option rows from 30px to 24px, option vertical padding from 6px to 3px and list vertical padding from 4px to 2px. Font, line height, horizontal spacing and the trigger/header height remain unchanged. Shared animation/presence helpers, positioning algorithms and the pointer driver were not changed.

The existing mounted fixture now samples actual opening frames and the independent reviewer strengthened it to consider effective ancestor/header visibility and positive geometry. All seven first/repeated/above/reduced-motion/modal/sibling scenarios passed with zero blank or detached-header frames. The raw baseline traces fail the continuity condition. Web **494/494** (including mounted **14/14**), Desktop **230/230**, SDK **6/6**, TypeScript and UI-design checks passed. Actual History status, process Log and built SDK renderings measured 24px rows, `3px 8px` row padding, `2px 4px` list padding and an unchanged 32px fixture header; query/subscription callbacks still worked. Root inspected the compact control and History status screenshots.

One bounded current-build Windows compositor attempt also passed using the previous capture helper and pixel predicates unchanged. All 607 executable/validation input hashes stayed identical across the checks and OS capture, including generated trust. This evidence verifies the main-window backdrop; the frame and renderer captures establish the dropdown behavior and geometry. It is not a physical-display anti-aliasing measurement or a fresh full Rust/Linux/legacy-suite claim.

Evidence: `.tmp/integrated-dropdown-refinement/implementation-report.md`, `before.json`, and `check/report.md`, `check/frame-summary.json`, `check/mounted.json`, `check/consumer-results.json`, gate logs and screenshots. The exact user-reported History status control is shown in `check/screenshots/07-history-status.png`. No commit, staging, archive or release replacement occurred.

### Format badge appearance and preview placement

The subsequent visual follow-up requested a weaker format-badge outline, 60% overall opacity, 10% smaller size and placement inside the actual inspector preview image. Baseline `.tmp/task-baselines/20261025-format-badge-refinement/` preserves 271 dirty paths plus 15 clean preview files. The product change is confined to shared `styles.css`; two existing format test files provide geometry coverage.

Badge border accent contribution changed from 70% to 40%, overall opacity is 0.6 and the top-left visual transform is 0.9. The inspector uses a per-stage CSS anchor on its displayed image or fallback artwork. In the landscape MP4 fixture, the image-relative badge inset changed from `(5,-40.5)` to `(5,5)` and its dimensions from `33.46875×18` to approximately `30.121872×16.2`. All captured content rectangles match the original positions and sizes exactly. Format colors, pointer behavior, JPG/JPEG/PNG exclusions and the central preview placement remain unchanged.

Review caught a narrow-inspector long-label case where an anchored percentage max-width increased the clipping width. Reusing the existing stage container's `100cqw - 34px` limit preserves the original clipping baseline: the 54px label now renders at 48.6px. The mounted fixture explicitly displays its inspector and rejects zero-size rectangles; previously its 800px viewport could hide that region and allow empty-geometry assertions to pass.

Final `check:web` passed **494/494** with TypeScript and no skipped cases; `check:ui-design` passed. Actual Electron renderer captures cover four layouts and open/locate interactions, landscape/portrait/square, source replacement, sidebar resize, failed artwork, simultaneous preview instances and format exclusions. A paused real image request confirms that waiting uses fallback artwork with the badge inside at `(5,5)`. Final source hashes stayed unchanged through verification and captures, and owned processes were cleaned up.

Report and final evidence: `.tmp/format-badge-refinement/check/report.md`, `check/final/metrics.json`, final screenshots and `check/pending.json`. Earlier `after/` evidence predates the long-label correction. CSS anchor behavior was verified in the current Electron/Chromium runtime; no other browser-engine or OS-compositor coverage is implied. This content-only refinement did not repeat unrelated Core, Docker or release checks, and no commit, staging, archive or release-package replacement occurred.

### Immediate hover response and time-label follow-up — 2026-10-28

The user increased badge opacity to 80%, requested immediate hover activation and pointer-first seek feedback, and requested matching time/badge visual typography with 90% time-label opacity. Baseline `.tmp/task-baselines/20261027-video-hover-response/` preserves 271 existing dirty paths plus five clean media-grid files.

The hover controller now invokes the existing playback lease on entry without its former 150ms dwell. Displayed progress/time track the latest pointer ratio separately from decoder work. Actual seeks coalesce through RAF, retain at most one in flight and the newest queued target, and do not let old completion events replace current intent. The existing 120ms stationary resume waits for pending work to finish. Unknown metadata keeps progress responsive without inventing time, and HUD visibility no longer waits for loadeddata. Muted playback, one-owner admission, bounded transport and cancellation remain unchanged. No MediaGrid, Core or client transport code changed.

The shared badge is now opacity 0.8; the time label is opacity 0.9 and matches the scaled badge's visual font: 9px normally, 7.2px in list mode. Existing badge scale, border contribution, inset and image anchoring are preserved. Real mounted checks confirm effective typography, visible HUD with first-frame readiness held closed, delayed-seek progress before decoder advancement, rapid reversal, stale-event rejection, pending-grant re-entry, metadata recovery and lifecycle cleanup.

Final Web **494/494**, TypeScript and UI-design checks passed. Fresh official Electron/Core integration at `.tmp/video-hover-integration/run-1793117822908/result.json` passed all four layouts and 15 lifecycle cases. Its 537,332,513-byte source transferred 11,460,122 bytes, including the distant 512MiB range; all 42 bodies closed (23 completed), no whole-preview endpoint was used, and all old grants were rejected after actual Library activation. Observed Core peak was 46,374,912 bytes and renderer peak 124,004KiB. Observed first-frame times of 217.9–289.5ms include transport/metadata/decode and polling; immediate admission and optimistic progress are established by separate deterministic ordering assertions, not by that timing range.

The first integration and a diagnostic repeat failed the old blanket 200/206 assertion. Recorded request/response phases and bounded error JSON proved the 409 was `playback_source_changed` during the fixture's deliberate source corruption. The formal harness now accepts only that exact negative case and retains 200/206 requirements for ordinary playback; no production error contract was changed. Its successful run is separate from the retained failed originals.

Two earlier Web runs are also retained: the first had unrelated LibrarySwitcher/float-geometry failures that later passed without source changes; the second had one supplemental rapid-re-entry count failure whose original diagnostic data was insufficient to establish its cause. Two focused reruns and the final full Web run passed. The exact +3 admission count remains enforced with richer error context; no sleep or count relaxation was added, and no unproven product fix was made.

Review and provenance are in `.tmp/video-hover-response/check/report.md`, `inputs-final.json`, gate logs and native-video captures. `cargo build -p mangoo-core` verified the current binary; the eight recorded final executable/test/binary hashes stayed unchanged through final verification. Owned processes exited. These are current UI/real-video results, not a new whole-repository, full Rust, Docker, OS-backdrop or release-package gate. No commit, staging, archive or release replacement occurred.
