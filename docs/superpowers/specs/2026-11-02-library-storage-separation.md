# Separate Library database storage and content folders

Status: approved by the user on 2026-11-02; implemented and verified on 2026-11-03. Evidence: `docs/performance-results/2026-11-02-library-storage-separation.md`.

## 1. Scope and confirmed decisions

The user approved a common Documents parent with one directory per Library, containing sibling `Database` and `Files` directories. At least one content folder is required, and the prefilled Files folder can be replaced with an existing folder. Additional existing folders may be selected. Version files mean Library/database format identity, not media-file history. The user explicitly excludes old-Library compatibility.

```text
<OS Documents>/Mangoo/Libraries/
  <Library name>/
    Database/
      .mangoo-library       # supported separated-storage format identity
      catalog.sqlite       # Catalog schema version remains in SQLite
      ...                  # WAL/SHM and owned internal state
    Files/                 # default content root; optional if replaced
  <Another Library>/
    Database/
    Files/
```

The Database path is the exact storage directory, not its parent or a SQLite filename. Host registry, credentials and device identity stay under their current application-level ownership; this feature relocates per-Library storage. Existing files are not automatically migrated, moved or deleted.

## 2. Architecture and interfaces

Desktop resolves the real Documents location and proposes paths without creating files. Web owns the draft and uses `@mangoo/core-client`. Core alone validates and creates storage/content directories, prepares the Catalog and publishes Library registration. Initial content roots are ordinary local Catalog roots with independent UUIDs; creation does not implicitly enroll them for sync or enable a Provider.

Use existing lifecycle endpoints, replacing their single managed-root creation/open payloads:

```ts
interface LibraryContentRootInput {
  path: string;
  createIfMissing: boolean;
}
interface LibraryPreflightRequest {
  name: string;
  databasePath: string;
  contentRoots: LibraryContentRootInput[]; // at least one
}
// Create also retains icon?, color?, initialMode: "local-only",
// preflightToken? and activate? from the existing lifecycle contract.
interface OpenLibraryRequest { databasePath: string; }
interface LibraryPreflightResponse {
  ok: boolean;
  reason: LibraryPreflightReason;
  pathRole: "database" | "content-root" | null;
  rootIndex: number | null;
  normalizedDatabasePath: string | null;
  normalizedContentRoots: LibraryContentRootInput[] | null;
  preflightToken: string | null;
  availableBytes?: number | null;
}
```

The normalized paths/list and token are non-null only on success. Failure preserves the input draft; rootIndex is zero-based into the original request order. There are no empty placeholder paths in a normalized root descriptor. Success preserves root order and creation flags without sorting or silent deduplication. `availableBytes` describes the Database volume, not aggregate media capacity.

The opaque token binds normalized name, Database path, ordered root descriptors/flags and inspected filesystem or nearest-existing-ancestor identities. Commit revalidates the entire plan. `createIfMissing` permits creation but never establishes cleanup ownership; only an actually created, identity-recorded directory is operation-owned.

Public `LibrarySummary.databasePath` replaces the misleading `managedRoot` field. Internal registry column renaming is unnecessary if its single-storage semantics are explicit. Retain `{library, activated, activationError}`: failed activation after successful creation leaves a complete inactive Library and preserves the previous active context. Keep current create revision behavior and the existing context reconciliation wrapper; this task does not add a new create-revision protocol. Open/activate retain their explicit revision fences.

Add a typed native capability, separate from plugin-exposed picker methods:

```ts
resolveLibraryCreationPaths({ name, databasePath? })
  -> { databasePath, defaultContentPath }
```

Main uses `app.getPath("documents")` and native path operations. Validate the draft segment, including Windows reserved names; an invalid name requires editing the name, rather than silently renaming or suffixing it. Keep the existing Core name restrictions even when the Database path is customized. Offer a custom location when Documents cannot be resolved or its storage policy rejects that path; a custom path does not bypass name validation. Shared IPC payload types live in `packages/notification-contract`; restrict the new channel with existing sender allowlists. No renderer filesystem imports or automatic mkdir from the chooser/default resolver.

## 3. Storage format and lifecycle ownership

The new `.mangoo-library` format is version 2, denoting separated storage. Retain bounded no-follow parsing, canonical UUID checks, pending/committed state and creation transaction identity. Its rootUuid is the virtual Library identity; content-root UUIDs remain independent even when exactly one root is selected. Do not copy the root list into a second manifest: the Catalog is authoritative. Existing SQLite migration metadata provides the Catalog schema version.

Extend durable Host creation/recovery records with layout version, final/staging storage paths and the ordered actually-created directory/marker identities and roles. Add a new numbered Host migration after the current 0021 and update Rust/schema validators. Do not rewrite old SQL. Filesystem placement alone requires no new Catalog table or media-history schema.

Create under the existing lifecycle owner:

1. Validate/reserve the complete Database/content plan before permanent effects. A missing ancestor chain is valid; preflight inspects its nearest existing parent.
2. Durably record intent. Create absent ancestors/default roots via no-follow pinned ownership helpers and record each created identity. Existing populated content roots are never emptied or moved.
3. Prepare the Catalog on the Database volume in transaction-owned staging. Initialize identity, every selected root and initial scan work before Library registration is visible. Never emulate this by publishing a Library and calling public add-root repeatedly.
4. Publish completed storage with the existing guarded no-replace pattern. The committed storage marker is the forward-recovery boundary. Publish the Host descriptor only after Catalog/root-set validation.
5. Before publication, compensate only identity-matching owned artifacts and remove newly created directories only when empty after owned cleanup. Preserve pre-existing directories, concurrent user additions and replaced paths. After the publication boundary, recover forward. Retain durable retry state if ownership prevents safe cleanup.
6. Activate using the existing atomic runtime swap and previous-context recovery. A complete but inactive Library remains available for retry.

New-format open validates an existing Database marker and Catalog before registration. Missing/corrupt Catalog files must never be created as empty replacements. Same-Host restart, forget and reopen are supported. A different device identity is a clear rejection; cross-Host Catalog import/rebinding is outside this task.

Unsupported old/unknown formats fail before Catalog mutation. Old active registry entries and unsupported pending operations must not prevent Host startup: preserve their paths/bytes, mark/report them unavailable, and expose a coherent no-active Library creation/selection flow. Never reinterpret old cleanup journals as new-format deletion authority.

Local-storage removal quarantines/removes one Database tree only, after identity verification. It must not double-rename the same path or remove sibling/default/external content directories. Registry-only list removal remains non-destructive. Existing restore constructors must use the separated layout and corresponding ownership journal; the execution plan identifies that integration explicitly.

Restore's path roles must also be explicit. The current single-root `managedRoot` means a content destination, while root-aware `placementBase` is a parent container; never silently reinterpret the former. Update the single-root restore request/callers to explicit placementBase `B`, previewing `B/Database` and `B/Files`. Root-aware restore uses `B/Database` and its explicitly chosen content targets. Preserve its existing management-only zero-selected-roots case; the minimum-one rule applies to the new local creation form. Public restore plans expose required `databasePath: string`, `placementBase: string` and `contentPaths: string[]`, replacing `managedRoot`; persist these path roles in the immutable plan. The coordinator reads the persisted content destination, never derives it from the Library Database descriptor. Provider download/protocol behavior remains unchanged.

## 4. Database exclusion and concurrency

Content roots must not equal, contain or lie inside protected Database storage. Apply one shared predicate to initial creation, later ordinary add-root, v2 enrollment/rebind/relocation and restore target admission. Database creation also checks already registered content roots so another Library's Database cannot be placed inside their browsed trees. Include hidden registrations and pending storage/target reservations.

Use canonical filesystem identities and no-follow component checks, not string case folding alone. Inspect raw components before canonicalization can erase junction evidence. Invalid case/alias/reparse replacements must fail during final validation rather than acquiring deletion authority.

The persisted path spelling must stay identical when a planned directory becomes existing. Return an already resolved directory directly when its missing suffix is empty; do not introduce a trailing separator that bypasses exact root-identity lookup. Keep the conflicting-root-UUID rejection and native-string regression alongside filesystem-identity tests.

Use the existing Library operation mutex and durable operation journals; do not add a parallel root-reservation table. Query only root paths from inactive new-format Catalogs through read-only helpers. Do not call the existing root-list query that joins folders and counts media, or enumerate subtrees for a preview. Lock order is operation ownership before request admission, then current-context validation and Catalog writes. Ordinary `/roots` middleware currently acquires the request read gate first; adapt that admission before sharing the operation mutex, otherwise activation's operation→write-gate order can deadlock. Release short Host registry locks before Catalog reads or asynchronous writer work.

Admission snapshots include pending enrollment/relocation/restore targets until their journals prove completion or compensation. Keep long transfers outside the global lock after durable reservation; revalidate under it at final cutover. Read-only Catalog queries use existing-file modes and finite timeouts; unreadable supported Catalogs return an actionable admission error, never an empty-root assumption. Unsupported old entries retain conservative recorded reservations without being opened or migrated.

Shared scan/watch policy prunes known internal storage and valid orphaned version-2 storage markers before folder rows or file events are created. It covers full-root BFS, file traversal, partial scans and watcher changes. Cache bounded directory-identity classification per operation/runtime; avoid per-file Host queries or a new recursive scan. User folders named `Database`, ordinary SQLite files and malformed lookalike markers elsewhere remain visible. Known owned storage stays excluded even while unhealthy.

Publish immutable storage snapshots with an epoch for workers. Clear negative marker results at each traversal and on relevant directory/marker watcher or storage-epoch changes; a long-lived negative TTL alone cannot prove correctness. Scan/writer workers must not acquire the lifecycle mutex because activation joins them while holding it.

When an existing indexed subtree becomes recognized internal storage, retire its folder/file projections without publishing content-deletion events. A task completion signal must follow release of its Database connection and permit; shutdown joins every owner and closes the runtime fsops connection before releasing the Catalog pin or renaming storage.

## 5. Creation UI

Keep the shared wizard for first-run, Library switcher and Settings. The 2026-11-03 visual-priority refinement orders its body as name/appearance, content folders, then a quiet Database location disclosure. Desktop defaults this low-priority editor closed; Web opens it for required explicit Core-host input, and Database/path errors reveal it for recovery. Default one compact Files row; allow change/remove and an Add folder action. The user may remove the last row, but creation stays disabled with a concise inline error until one valid row exists. Do not silently re-add a removed default.

Track path provenance explicitly. Automatic Database and default Files follow name edits. Choosing/editing Database pins that exact path; a still-default Files row follows its parent. Existing added/replaced folders are never rewritten. Sequence and dialog-generation fences discard old native resolver/picker responses. With no Desktop bridge, show explicit Core-host path inputs; do not use browser cwd or invent a local Documents path.

Preserve 20px/28px heading and 14px/20px body typography, the appearance popup, acrylic materials and one boundary per control. Use compact folder rows inside the scrolling body with fixed footer actions; do not shrink fonts to fit. Keep focus/error scrolling, Escape hierarchy, busy guards and context/registry refresh across windows. Submit one immutable all-path preflight/create snapshot; no automatic preflight write probes while typing.

The 2026-11-03 layout refinement changes visual order/priority while retaining the same path and creation behavior: one integrated icon/name control, material-folder name above its editable path, one unified boundary around each path plus icon actions, and the secondary Database disclosure. Keep complete values and field-level recovery available; omit routine naming/ready explanations and duplicated generic status. See the current [creation layout contract](2026-10-31-onboarding-and-icons.md).

## 6. Validation and failure matrix

| Input/state | Result |
| --- | --- |
| Missing default parent/Database/Files | Preview remains read-only; Create materializes and journals the needed directories. |
| Existing populated selected content folder | Accepted; no relocation, clearing or hidden ownership claim. |
| Zero content roots | `content-roots-required`, content-root role, null index; no creation. |
| Duplicate/nested selected content roots | `content-roots-overlap`, later conflicting input index. |
| Content path overlaps protected Database | `database-content-overlap`, affected content index; reject before effects. |
| New Database overlaps an existing content root | `database-content-overlap`, database role, null index. |
| Existing nonempty Database or same destination | Existing typed collision/not-empty result; no overwrite or silent suffix. |
| Existing-only content path is missing | `invalid-path` at that content index; do not create it. |
| Identity changes after preflight | `path-changed`; preserve all unrelated objects. |
| Documents redirected into a disallowed cloud/reparse location | Show the real default and existing `cloud-sync-root`/`reparse-point` result, allow custom Database location; no home fallback or weaker SQLite storage policy. |
| Creation fails before publication | No visible partial Library; exact owned compensation/retry journal. |
| Activation fails after successful creation | Complete inactive Library retained; previous active Library/context preserved. |
| Unsupported format or foreign device | Clear unavailable/identity error; no automatic conversion or empty Catalog initialization. |

Keep existing reasons where accurate; only add the three content-root/Database overlap reasons above. Preserve existing HTTP error/transport handling and field-level information. Ordinary existing content folders do not inherit Database's empty-directory rule or blanket cloud restriction; sync enrollment retains its own policy.

## 7. Evidence, tests and limits

Research anchors are in the task's `research/core-storage.md`, `research/contracts-tests.md` and `research/desktop-ui.md`; current key owners include `library_manager.rs:3370`, `library_path.rs:107`, `api/routes.rs:6873`, `db/mod.rs:1146`, `ManagedLibraryWizard.tsx:35` and `main.ts:1082`. These are current source evidence, not completed implementation checks.

Acceptance must inspect actual filesystem/Catalog roots after native UI creation, two Libraries under Documents, populated external/multiple roots, cancellation/no writes, overlap/alias rejection, crash recovery, storage-only removal, new-format restart/open, unsupported old-entry startup, and four-layout/context preservation. Test both ordinal root counts and UUID independence. Add actual scan/watch assertions proving no Database folder or database-derived media rows.

Run contract/client/schema, Web/Desktop/UI, focused Rust lifecycle/path/scan/watch tests and the full `npm test` gate using an isolated Cargo target when user binaries are locked. Native runs use the exact newly built Core and isolated Host/profile/data/ports; preserve user processes. Verify normal/narrow/enlarged UI and OS-composited appearance; renderer capture alone does not prove Windows backdrop behavior.

Out of scope: old-Library compatibility/migration, file history, new Provider design, foreign-device Catalog import, live cloud-synced SQLite support, release publication and unrelated layout changes. Implementation and validation evidence are recorded under the active task and `.tmp/library-storage-separation`.
