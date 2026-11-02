# Library switcher and safe registration

## 1. Scope / trigger

The 2026-11-01 request combines a switcher redesign with white default Library icons. The user additionally authorized opening an existing Mangoo managed Library and removing a Library only from the list. The switcher follows Mangoo's creation-form typography, shared acrylic layers, Phosphor icons, one thin boundary and global pointer highlight. Eagle is an interaction reference, not an import format or a replacement design system.

## 2. Signatures

- `POST /libraries/open`, body `OpenLibraryRequest { databasePath: string }`, with the existing `X-Mangoo-Context-Revision` header. The [separated storage contract](2026-11-02-library-storage-separation.md) supersedes the original managed-root payload/layout.
- `OpenLibraryResponse { library: LibrarySummary; activated: boolean; activationError?: string | null }`.
- `DELETE /libraries/{libraryUuid}/registration`, with the existing context-revision header, returns `ActiveLibraryResponse`.
- Typed client methods: `openLibrary(input, contextRevision)` and `forgetLibrary(libraryUuid, contextRevision)`.

These are separate from creation/preflight and destructive local-copy removal. The Web renderer calls typed adapters/client methods; it must not access the filesystem or manufacture Core DTOs. Native folder selection and Explorer are Desktop capabilities, not substitutes for registration validation.

## 3. Contracts

### Identity, data and visibility

Opening validates the supported format-2 marker, existing Catalog and canonical Database directory identity before registration. It does not initialize an arbitrary directory, create a missing Catalog, rewrite an existing Library UUID, or replace user files. Marker reads are bounded and no-follow; conflicting paths and identities are rejected.

Same-Host opening reuses the retained UUID, metadata and existing catalog/cache. Reopening a forgotten Library restores visibility. Different-device Catalog identity is rejected rather than silently replaced or rebound. Old storage formats are unsupported without migration; their files remain intact and unavailable old active entries must not prevent Host startup.

Visibility is stored separately from retained Host registration/data. Internal identity/overlap/recovery queries retain hidden records; public list results exclude them. Hidden records must not become a route around ownership, path-overlap or runtime admission checks. Activation by a hidden UUID is rejected until an explicit open restores visibility.

Forgetting never invokes `removeLocalLibrary`, `deleteLibraryFile` or the destructive quarantine/delete journal. The operation preserves managed-root files, markers, local catalog/cache, stored metadata and binding configuration. Host-owned synchronization is independent of the published browsing runtime: list removal does not pause synchronization, alter its generation/configuration, or pretend that a catalog lease joins Provider work. Already-enabled background synchronization continues under its existing contract and may make its ordinary file/data changes. Confirmation text must state this distinction. Hidden registration does not authorize publishing a browsing runtime without explicit opening.

### Active context and partial outcomes

Forgetting an inactive Library preserves the current active runtime and context revision. Forgetting the active Library stops/joins its browsing runtime, then atomically hides it, clears the active registration and advances the context revision. Host-owned background synchronization remains independent as described above. No automatic fallback Library is selected. Repeated forget requests are idempotent under the current revision; ordinary stale-context fences still apply.

Open activation is authoritative Core behavior. If registration succeeds but activation fails, preserve the listed registration and return the partial outcome with an actionable error. UI feedback must distinguish registration from successful opening, and permit retry through the list. Do not reactivate a successfully activated Library or emit a synthetic ready state.

Once Core accepts a registration/forget operation, an HTTP disconnect must not release its mutation lock or abandon runtime-stop ownership halfway through the transition. Accepted work has an explicit lifetime owner. Host shutdown fences new registration operations and drains those accepted operations and their pending runtime stops before finishing its existing shutdown path. Detached `tokio::spawn` alone is insufficient; cancellation and shutdown-barrier tests must prove this ownership chain closes. This contract does not claim a redesign of all preexisting Host/normal-active-runtime shutdown behavior.

The existing `shutdown_sync_coordinator` entrypoint participates in this terminal Host fence; its production caller is the main shutdown path after the HTTP server finishes. Do not reuse it as a pause/restart switch while expecting further Library activation/registration. Tests that require stopped background providers must arrange their Library first and retain equivalent isolation and assertions.

The renderer reuses the shared context lifecycle and other-window invalidation owner. Cross-window list/active state must reconcile after open/forget. Native-picker blur, stale popup generations, duplicate submissions and failed operations retain their existing protections.

Inactive forgetting uses the same authoritative active snapshot for its target comparison and revision header, then sends registry-only invalidation so current media/selection/scroll are not torn down. The existing same-origin, Core-endpoint-scoped BroadcastChannel gains the literal `registry-settled` signal and a `subscribeLibraryRegistry` consumer. `context-settled` also invalidates registry consumers. These messages carry no paths or Library names; receivers reread authoritative Core state. Activating open/active forget continue through the existing teardown/reconciliation owner. Stale-revision failures remain visible and reconcile actual state rather than publishing a requested identity.

All mounted registry-list consumers must subscribe, including the switcher and the Settings Library management panel. Subscription callbacks retain request-generation/unmount fences so a late old list cannot reintroduce a forgotten entry. A registry-only change must refresh an already-open secondary window without clearing its current media context.

Clearing the active browsing context must clear pagination totals, slots, loaded ranges and cursor state as well as visible media. Otherwise the no-active early return can leave the titlebar showing the previous Library's count. Keep this reset in the shared context teardown; inactive registry-only removal must not trigger it.

### Presentation

Default color is `#ffffff` for creation, titlebar/list/settings glyphs with no saved color, and explicit appearance reset. Preserve stored custom colors, including old blue: there is no provenance flag to distinguish an earlier automatic default from an intentional color. Add white as a selectable choice without removing existing glyph/color options. Reset uses explicit default icon/color values rather than relying on ambiguous null metadata patches.

The switcher's reset action belongs inside its `图标与颜色` editor, not as a duplicate sibling action in the more menu. It resets both fields in the local draft; Apply saves the pair and Cancel discards it. The shared editor replaces the old immediate-save picker and cascading appearance submenu. Detailed selection, commit, registry refresh and Dev CORS behavior: [Library appearance editor](2026-11-01-library-appearance-editor.md).

The panel contains search, Library rows, a per-row more menu and quiet create/open footer actions. Name text uses readable 14px/20px sizing; secondary paths may use 12px/16px regular caption text. Long text retains full accessible content. The active check and menu trigger are distinct from the switch target. More-menu operations must not inadvertently activate a row. Appearance closes the action menu and opens the shared anchored popup within the Library overlay family, preserving a stable live anchor and focus-return target. Escape and focus restoration respect overlay ownership. Preserve loading, no-result, unavailable, busy and error states, existing folder actions and `library-menu` plugin contributions.

Each Library row is one visual interaction object, including its ellipsis action. The entire row owns selection fill, its single thin boundary and the shared pointer highlight. Switch and more-menu buttons retain independent semantics and hit areas inside that shape, with no separate button edge or glow. Keyboard focus on either action emphasizes the same row boundary and keeps the focused action identifiable without adding another ring. Verify pointer movement across the row body and ellipsis, keyboard focus and menu-open states; menu activation must never switch the Library.

Opening an existing Library must remain reachable with no active Library: the first-run creation form provides a concise alternative open action, and forgetting the active Library while others remain must still permit choosing them. Desktop selection should open the chosen valid Library directly without a second redundant path confirmation; Web/manual entry and recoverable errors may use the shared dialog pattern. Cancelling a native picker leaves a reachable retry/selection surface.

Behavior primitives do not imply complete visual styling: menu and dialog compositions must explicitly use the existing shared layout/control classes. Verify the settled menu is a vertical list with transparent item controls, and dialog fields/actions have the same spacing, typography and single boundary as creation. White appearance selection needs a contrasting visible check, not only ARIA state. Core error payloads use `error` and `code`; translate known registration/path/identity failures into actionable Chinese while retaining a useful unknown-error fallback and the entered path.

Do not add drag handles without persistent ordering support. Removal wording must clearly mean list removal; existing destructive operations keep their separate names and confirmation semantics.

## 4. Validation / error matrix

| Case | Required outcome |
| --- | --- |
| Already listed same root | Reuse registration; activate coherently without duplicates |
| Forgotten same root | Restore list visibility and retained identity/data |
| Valid other-Host root | Preserve source identity/content; create only owned local catalog |
| Ordinary, missing, malformed or unsupported root | Actionable rejection; no source initialization or identity rewrite |
| UUID/path/overlap/local-data collision | Reject without replacing or deleting existing content |
| Inactive forget | List entry hidden; active runtime/revision unchanged |
| Active/last forget | Browsing runtime drained; empty active state published; no deletion |
| Library with enabled synchronization | Binding/configuration unchanged; existing Host synchronization continues; UI explains this |
| Partial registration/activation failure | Registered entry retained and retryable; no false success |
| Picker cancellation, stale session, overlapping request | No duplicate/stale UI effects; errors and current Core state remain authoritative |
| Restart/recovery | Visibility, identities and active-context decisions persist consistently |

## 5. Good / base / bad cases

Good: a Library disappears from the switcher, all sentinel files/catalog metadata remain intact, and selecting the same root later restores it. Base: switching between already-listed Libraries continues without Core restart or renderer reload. Bad: changing the label of destructive local-copy deletion to `从列表移除`, creating a fresh identity in an ordinary selected directory, or migrating every saved blue glyph to white.

## 6. Tests required

Contract/client tests verify endpoint, method, revision/session headers and typed partial outcomes. Core tests cover same-root and foreign-Host opening, strict marker validation, collision/no-overwrite behavior, preserved sentinel bytes/metadata/catalog/cache, active/inactive/last forget, repeated operations, concurrency and restart/recovery. Validate the host migration and distinguish visibility queries from identity/overlap queries.

Mounted UI tests cover white defaults/custom-color retention, search/empty/loading, activation, more-menu and child-panel keyboard/focus, appearance/reset, open/forget confirmation and errors, busy/stale/native-picker blur, existing folder actions and plugin slots. Real isolated Desktop/Core tests verify native selection and data preservation/reopen, plus OS-composited popup/menu/appearance captures. Run current Web/Desktop/UI/client/schema/Rust checks and the full project gate; older gate results are not current-tree evidence.

For the row/appearance follow-up, audit every switcher and child-menu button against its actual outcome: search/clear or dismiss, activation, more-menu isolation, glyph/color/reset persistence, Explorer path, add/remove folder, create/open/list removal, and relevant cancel/disabled/error states. Use isolated owned Library fixtures for native filesystem actions. A successful click alone is not evidence that the intended operation completed; check the corresponding UI, Core state or native destination. Pure presentation follow-ups need current Web/UI/desktop checks and focused visual/interaction coverage; unchanged Core gates do not need repetition.

## 7. Wrong vs correct

Wrong: `onForget = () => deleteLibraryFile(uuid)` or `onOpen = () => createLibrary({rootPath})`.

Correct: call the dedicated typed registration APIs, reconcile authoritative active state, and keep source files and retained local state outside deletion paths. Put appearance/search/menu behavior in the existing shared overlay system rather than page-specific borders, hover effects or duplicated state machines.
