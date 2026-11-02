# Library storage separation implementation plan

Status: user approved implementation on 2026-11-02; completed on 2026-11-03. Governing design: `docs/superpowers/specs/2026-11-02-library-storage-separation.md`. Task: `.trellis/tasks/11-02-library-storage-separation`. Results and evidence boundaries: `docs/performance-results/2026-11-02-library-storage-separation.md`.

## Scope and baseline

The original behavior registered its selected managed root as the sole browsable root while storing its Catalog elsewhere under the Host. The new flow places the Catalog/format marker in a chosen Database directory and registers only independently selected content folders. This required lifecycle, admission/scanning, public DTO and creation UI changes together.

The user approved per-Library Database/Files siblings under Documents and format-version metadata; old-Library compatibility is excluded. No migration, automatic old-data removal, media history, new Provider feature or release publication. Worktree baseline: `.tmp/task-baselines/20261102-library-storage-separation`, 387 dirty paths with hashes, exact file copies and staged/unstaged patches. No staging/commit or broad cleanup.

## Ordered work and ownership

### 1. Contract and data plan

Owner chain A: API/client and shared Desktop contracts. Freeze the agreed `databasePath`, ordered `contentRoots`, creation flags and all-or-nothing nullable normalized preflight result in OpenAPI first. Add the three overlap/root-list reasons with field/index mapping. Mirror client interfaces and runtime plugin validators. Retain current create activation semantics and explicit open/activate revision fences.

Expected files: `contracts/core-api/openapi.yaml`, `packages/core-client/src/generated-contract.ts`, `client.ts`, `index.ts`, client tests and `scripts/check-contract.mjs`; `packages/notification-contract/src/index.ts` for the read-only native resolver payload. Core owns Rust DTO edits in step 2; Web owns consumer edits in step 3. Coordinate before parallel writes.

Update the existing single-root restore placement contract explicitly, matching the design's parent-container semantics. Keep root-aware selected destinations and existing management-only restore behavior. Update relevant plugin runtime validation/SDK call sites only where they actually consume the changed Core request; do not repurpose or remove the unrelated plugin-exposed picker method.

### 2. Core storage, recovery and exclusion

Owner chain B: Rust Core. Introduce supported format 2 and independent Catalog root identities. Add the next Host migration for versioned creation/restore ownership and final/staging paths; register it in HostRegistry and `validate_sqlite_schema.py`. Do not add a Catalog table or a separate path reservation table without new evidence requiring a design revision.

Implement whole-plan validation, safe absent ancestor/default-folder creation, same-volume Catalog staging, all-root preparation, guarded publication, exact cleanup/forward recovery and partial activation semantics. Adapt existing-file open, storage-only removal, unsupported active-entry startup isolation and restore constructor/materializer targets together.

Use shared path admission for create, ordinary add-root and v2 path-changing operations. Add cheap read-only `roots.path` queries for inactive Catalogs and include journal reservations. Fix operation/request-gate lock ordering before ordinary add-root acquires admission. Publish read-only storage snapshots to full/partial scan and watch paths; prune known storage and verified orphan new-format markers before folder insertion. Preserve user content with similar filenames.

Expected files: `crates/core/src/library_manager.rs`, `library_path.rs`, `host_registry.rs`, `api/routes.rs`, `api/mod.rs`, `db/mod.rs`, `sync/file_policy.rs`, `scan/mod.rs`, `watch/mod.rs`, root-path validation/cutover call sites and `sync/coordinator.rs`; new numbered Host migration and schema validator. Reuse ownership/no-follow helpers and existing writer/runtime fences rather than adding an independent filesystem service.

Required focused cases include missing parent chains; one/multiple/existing populated roots; preflight token changes; collisions/aliases; cross-Library overlap; concurrent create/add-root/activate without deadlock; every durable failure boundary; identity-swapped/concurrently populated cleanup targets; independent UUIDs; no database browse/media rows; new-format reopen; unsupported active entries; foreign device rejection; storage-only deletion; restore targets outside Database.

### 3. Desktop defaults and shared creation UI

Owner chain C: Desktop/Web. Add permission-checked read-only Documents path resolution, extracted native path calculation and typed preload/renderer bridge. Do not create directories in preview/pickers and do not assume an existing Library UUID for first-run selection. Default paths follow the name until explicitly overridden; existing content selections stay pinned. Web-only operation accepts explicit Core-host paths.

Refactor `ManagedLibraryWizard` to name/appearance → Database path → compact folder list, with at least one required content root and a prefilled sibling Files row. Submit one full plan. Preserve current font tokens, glass, single boundaries, fixed footer, appearance popup, async fences, duplicate-submit protection and cross-window activation/registry updates. Update open wording, summary path consumers and meaningful mounted fixtures.

Expected files: Desktop `main.ts`, `preload.cjs`, `desktop-ipc-permissions.ts`, new `library-creation-paths.ts` and tests; Web `core/desktop.ts`, `features/library/ManagedLibraryWizard.tsx`, an optional focused draft model, `OpenLibraryDialog.tsx`, `libraryOperationError.ts`, `LibrarySwitcher.tsx`, Settings summary accesses, plugin Core-method validation and scoped `styles.css`; associated mounted/adapter/startup/permission fixtures. Avoid rewriting unrelated OnboardingHero/add-folder behavior.

### 4. Integration and independent check

After the contract is frozen, chains B and C can implement concurrently on their owned files. Main coordinates DTO changes and updates specs; one independent reviewer may cover at most these three chains. Review the combined feature, not just the final UI diff. Keep product source frozen for final gates and native acceptance; rerun only the gates affected by an actual subsequent fix.

Update `tools/dev/managed-library-harness.mjs` and relevant lifecycle benchmark assumptions: new DTO, database-versus-content semantics, explicit freshly built Core path. Do not reuse an old approved binary or assume `target/debug` when an isolated Cargo target is configured.

## Validation sequence

1. Focused client contract tests; Desktop resolver/permissions; mounted wizard draft/path/async cases; Core path/lifecycle/recovery/scan/watch tests with real filesystem assertions.
2. `npm run check:structure`, `check:core-api`, `check:core-client`, `check:schema`, `check:web`, `check:desktop`, `check:ui-design`, and `cargo fmt --all --check`.
3. Full `npm test` for the cross-layer feature. If the normal debug executable is in use, set a task-specific `CARGO_TARGET_DIR`; inspect output for actual Rust execution rather than treating a SKIP as a pass.
4. Build the exact new Core into the isolated target and launch isolated Electron with distinct Host/profile/Libraries/plugin-state paths and ports. Explicitly select the built binary and record its hash. Serialize foreground tests and preserve original user processes.
5. Create two Libraries through UI with Documents defaults, inspect Database/Files/catalog/format version, add real fixture media and confirm only selected roots are visible. Verify an external populated directory, multiple roots, path replacement/removal, zero-root prevention, cancellation/no app-created files, field errors, no overwrite and old-format rejection without mutation.
6. Verify new-format restart/forget/open, unavailable old active startup, storage-only removal preserving media, actual activation failure recovery, and frozen all-root registration. Validate four layouts/preview/context preservation plus normal/narrow/enlarged UI, fixed actions and icon popup layering.
7. Run the expected OS-backdrop harness on an owned window and retain actual OS-composited screenshots. Separate unavailable/stale harness results from alternate evidence and do not claim physical-display AA.

## Completion and recovery notes

Implementation and scoped acceptance are complete. The durable result report records exact counts, binary/source identity, filesystem evidence, process cleanup, baseline/staging delta and limitations. Stale current specs were updated; the new UI selects a storage directory, not a SQLite file. Native activation-failure injection, physical Windows DPI changes, unsupported manual marker-version reversal and optional live-provider behavior are not claimed beyond the evidence explicitly recorded there.

If implementation uncovers a broader product decision, stop that dependent branch and surface it; do not invent cross-device migration or cloud-synced SQLite support. Preserve original files and durable ownership evidence on uncertain cleanup. No commit/archive of the parent all-file task is part of this work.
