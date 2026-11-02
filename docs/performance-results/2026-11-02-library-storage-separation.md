# Library storage separation verification

Status: completed 2026-11-03 after implementation, independent review, complete component gates and isolated Windows desktop/filesystem acceptance.

## Result

The shared creation form proposes the actual OS Documents directory followed by `Mangoo/Libraries/<name>/Database` and its sibling `Files`. Users may replace or add content folders, including existing populated folders; local creation requires at least one. Database storage is excluded from browsing. Format 2 identifies the separated layout. Existing unsupported Libraries are not migrated or deleted.

The governing design and execution plan are `docs/superpowers/specs/2026-11-02-library-storage-separation.md` and `docs/superpowers/plans/2026-11-02-library-storage-separation.md`. The user approved implementation on 2026-11-02. The task baseline preserves 387 pre-existing dirty paths under `.tmp/task-baselines/20261102-library-storage-separation`; staging and unrelated work must remain intact.

## Focused evidence

| Area | Fresh result | Evidence |
| --- | --- | --- |
| Core client | Contract mirror, TypeScript and 74 tests passed | `.tmp/library-storage-separation/contract-implementation.md` |
| Core API alignment | Static contract/route check passed | Same report; not runtime evidence |
| Lifecycle harness | 3 helper tests and syntax checks for 9 affected tool callers passed | Same report |
| Shared creation UI | Web/Desktop type and boundary checks, UI-design check, 36 focused tests and 12 IPC checks passed | `.tmp/library-storage-separation/ui-implementation.md` |
| Mounted wizard | Ordered multi-root draft, stale responses, normalized full submission, indexed errors, zero roots and inactive completion passed | `ui-mounted-wizard.log` in the task evidence directory |
| Startup and switcher | 15 focused mounted checks passed | UI implementation report |
| Native name error correction | 4 resolver tests passed after review; custom paths do not bypass Core naming rules | `.tmp/library-storage-separation/check-report.md` |
| External restore consumer | Host SDK 7 tests, external UI 20 tests, build/typecheck and independent vendored installation plus 5 artifact checks passed | Contract implementation report and external baseline |
| Core lifecycle and indexing | Manager 38, restore admission 10 and separated-storage 14 passed; the latter passed five consecutive groups before final review fixes | `.tmp/library-storage-separation/core-implementation.md` |
| Full non-Rust gates | Structure, plugin artifacts/SDK, API/client, notification, Desktop 237, launcher 8, Web 528, UI-design and schema passed | `.tmp/library-storage-separation/final-npm-test.log` |
| Final complete Rust gate | Core 891 passed, 0 failed, 14 existing opt-in ignored; workspace 904 passes including subprocess/provider suites | `.tmp/library-storage-separation/final-rust-gate.log` |
| Final Core build | Build passed; Core emits no warnings. Two existing PPM test-helper warnings remain outside this change | `.tmp/library-storage-separation/review-final-source-freeze.json` |

UI screenshots under `.tmp/library-storage-separation/ui-captures` cover normal, narrow, short and enlarged renderers. They establish layout and state, not Windows compositor appearance. The main reviewer inspected `ready.png` and `narrow-long-path.png`.

## Independent review and final acceptance

Independent review covers the contract, Core lifecycle and UI/native chains. Fixed findings include retiring old index projections when storage becomes recognized, version-isolating removal recovery, avoiding post-hoc SQLite sidecar ownership, isolating unsupported dormant operations, checking restore batch path conflicts in both directions, retaining failed-stop ownership, preserving forward-cleanup recovery, and retaining Catalog pins until SQLite closes.

The first full `npm test` passed every preceding component and exposed 13 Rust failures. Review repaired a real trailing-separator normalization defect that bypassed Catalog root-identity conflict detection; original rejection assertions remain. Other fixes align fixtures with explicit content roots, format 2, migration 22, existing-only Catalogs and the isolated plugin bundle. A marker-mutation fixture now stops and joins its own watcher before injecting faults, preserving its original identity/status/recovery assertions. Production file-sharing protections, timeouts, concurrency and ignored-test settings were not weakened. The complete `npm run check:rust` then passed on unchanged final Rust source. Since subsequent product edits were Rust-only, already-passing frontend gates were not repeated.

Final Core: `D:/Mangoo/.tmp/library-storage-separation/cargo-target/debug/mangoo-core.exe`, 60,884,992 bytes, SHA-256 `6e4703f1467f2c84580718c1b6bb48eadb64f83996ce45b0fddc0bd3817d0a64`. The native run copied this executable and its matching bundled plugins into an owned isolated runtime. Main rechecked the binary and 19 frozen product-source hashes after acceptance; all match. Final generated trust has raw-byte SHA-256 `81c39455b03efa939197b24bcc8d2f5629d8c5d44ffc90f6019cfa486c011a55`, independently matched to the standard bundled plugin bytes. The second normal build pipeline regenerated trust; no external plugin was selected or trust entry hand-edited.

Keep one unresolved observation visible: after fixing task completion before Database release, an early focused removal-retry group once still reported Windows sharing error 32. Five subsequent groups, the final full Rust gate and both native active-removal operations passed; independent read-only diagnosis found no second deterministic cause. The first full-suite concurrency test also timed out once, then passed focused and final full runs with its original timeout and parallelism. These passes do not prove every possible external file-lock or timing condition absent.

## Real Windows acceptance

The run used isolated Host/profile/plugin state, distinct ports and the exact final Core. Its complete results, requests, filesystem receipts and limitations are in `.tmp/library-storage-separation/native-acceptance.md` and `native-run/` beneath the same evidence directory.

| Scenario | Observed result |
| --- | --- |
| Real Documents defaults | Two Libraries created through UI under actual `C:/Users/84460/Documents/Mangoo/Libraries`, with Database/Files siblings and committed format-2 markers. No Documents override or automatic-create seam. |
| Existing content roots | Two populated roots selected through the actual creation form and native picker; both registered together with independent UUIDs. Zero-root creation disabled and canceled draft produced no directory. |
| Browsing and indexing | Ordinary user `Database` directory and `catalog.sqlite` file remain visible; known Database root admission rejected. A newly added valid internal-storage subtree produced no folder/file projections, while an ordinary watched image appeared. |
| Path failures | Empty, overlapping/nested, missing-existing-only and junction paths rejected with typed field/index information and null normalized failure plans. |
| Supported lifecycle | Forget preserved Database bytes/identity; supported API and UI reopen succeeded. Restart restored two roots and 107 files. Missing/corrupt Catalog open did not create a replacement. |
| Unsupported old format | Activation preserved prior browsing context; a seeded old active entry did not block startup, was unavailable, and retained all old Database bytes. |
| Storage-only removal | Default active Library removed in about 63 ms and indexed multi-root Library in about 139 ms; both returned 204 without retry or sleep. Only Database disappeared; Files and both external roots retained exact identities/content hashes. These are observed fixture timings, not a general performance benchmark. |
| Existing UI behavior | Four layouts switched; preview return preserved selected image and scrollTop 1300. Appearance popup and fixed actions remained usable in normal and constrained windows. |

Main inspected the actual OS-composited `native-run/default-b-os.png` and the real multi-root creation capture `native-run/multi-before-create.png`. `appearance-narrow-os.png` is another valid composed-window capture. `default-a-os.png` was obscured and is explicitly excluded. 125/150% captures use Chromium DPI emulation in real Electron, not a physical Windows scale change; no physical-display anti-aliasing conclusion is claimed. The stale legacy visual script was not presented as passing.

Native creation-success-immediately-followed-by-activation-failure was not deterministically injected. Its underlying recovery and inactive-result handling have separate Core real-filesystem and mounted UI evidence. Native tests cover ready-inactive creation, unsupported activation preserving prior context and inactive Open outcomes. Manually restoring a deliberately downgraded marker did not clear cached unavailable state; this unsupported version reversal is not claimed as migration support. Crash-boundary and identity-replacement cases are Core tests, not native crash simulations.

## Cleanup and scope audit

Both real Documents test Library directories were removed only after absence/identity/entry-set/content receipts were verified. Common Documents/Mangoo/Libraries parents remain because their original ownership was not separately recorded. Ignored `.tmp` fixtures and screenshots remain as evidence. All owned Electron/Vite test trees exited.

User main Electron and Core retained their original process start times, as did two existing Electron children. One preparation-baseline child had been replaced before native acceptance began; its cause was not established, and not all historical child PIDs are claimed unchanged. Only identity-recorded test trees were stopped.

Final baseline audit: of 387 pre-existing dirty paths, 341 remain byte-identical and 46 have scoped task changes; 16 additional paths are dirty. Staging is unchanged, with no commit or parent-task archive. See `final-delta.json`. All four scoped external-file hashes match their tested results and external package.json is unchanged (`external-final-audit.json`). Main reran structure, Core API and schema checks after final specification edits; all passed. Scoped diff whitespace check passed.

## External integration and limits

The actual restore UI is in `D:/Mangoo_OneDriveSync`, as recorded by `docs/onedrive-extraction.md`. Only its restore wizard, sandbox workflow test, standard vendored SDK archive and SDK lock integrity changed; exact original bytes and hash inventory were preserved first. Single-root restore now uses an explicit placement base, while root-aware management-only restore remains supported.

The external production plugin has not been repackaged or staged. No live cloud restore, foreign-device Catalog import, Linux acceptance, release publication or physical-display anti-aliasing proof is claimed. No files have been staged or committed.
