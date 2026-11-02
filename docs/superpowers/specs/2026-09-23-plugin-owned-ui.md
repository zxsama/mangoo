# Plugin-owned feature UI

## Agreed boundary

The user explicitly keeps the generic Core sync engine. Sync settings, folder-management UI, remote restore, enrollment and their presentation/workflow state belong to the independent OneDriveSync plugin. Host must not retain built-in feature panels and merely hide them. Host has no source/build dependency on the plugin checkout. Existing local Library browsing/creation remains independent.

No Trellis task is created (explicit user choice). Preserve unrelated dirty work, actual media and remote objects. Pre-change UI source is snapshotted in `.tmp/plugin-ui-extraction-20260923/source`.

## v1 extension boundary

- Optional `contributions.ui`: `{ apiVersion: 1, entry: string, sha256: string, views: [{ id, title, slot }] }`.
- `entry` is one bounded, self-contained UTF-8 HTML document in the plugin artifact; no external resources or arbitrary asset paths. Script/CSS/dependencies are bundled into it. Max 8 MiB. Relative no-follow path and exact SHA-256 required.
- `slot`: `library-menu`, `library-settings`, `plugin-detail`, `root-menu`, `notification-center`. Slots are generic insertion locations, not feature names. A view identifier is opaque to Host; context contains current Library/root identity when applicable.
- `notification-center` contributes an entry on the current-status page. The plugin owns its entry title and the status/workflow inside its isolated document. Host uses the existing availability, grant and lifecycle checks and performs no sync-engine polling or provider-status inference for this slot. An empty contribution set creates no section. The legacy `NotificationCompactSnapshot.syncHealth` wire field is local Core/Desktop notification transport/store health; Host labels its failure as “通知服务受限”, never as provider synchronization being limited. Existing Core-engine task progress and historical notification records are unchanged by this slot extension.
- Artifact inventory/trust includes manifest and UI document integrity in addition to executable integrity. Core-embedded manifest hash pins contributions and permissions; a self-declared UI hash is not a trust root. Existing no-UI artifacts remain supported.
- `GET /api/plugins/{pluginId}/views/{viewId}` returns `{ pluginId, version, lifecycleEpoch, viewId, title, document }` only for installed/current, trusted, enabled, granted and non-stopping/non-incompatible plugin versions. Reading verified bytes is offloaded from async runtime. It does not launch a Provider or require an authenticated account.
- Host renders the verified document in an iframe sandbox with `allow-scripts` only, without same-origin/top-navigation/popups/forms/download grants. Host owns a restrictive CSP (`default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; connect-src 'none'; base-uri 'none'; form-action 'none'`) prepended before plugin content. Plugin document never runs in Host renderer and never receives its session token/preload.
- Message wire v1: plugin sends `{ type: 'mangoo-plugin-ui:ready' }`. Host responds `{ type: 'mangoo-plugin-ui:init', sessionId, pluginId, viewId, context: { libraryUuid, rootId, rootUuid }, theme }`. `rootId` is the generic local RootRecord identity; Host slots do not resolve feature-specific roots. Requests `{ type: 'mangoo-plugin-ui:request', sessionId, requestId, method, args }`; responses `{ type: 'mangoo-plugin-ui:response', sessionId, requestId, result }` or `error: { message, status?, body? }`. Request IDs are canonical positive safe-integer decimal strings with a monotonic high-water mark, not an unbounded replay set or lifetime request ceiling.
- Host validates source window, exact live session, bounds, method and argument shape; max 32 concurrent calls; session revoked on unmount/navigation/plugin change. No raw HTTP/URL/IPC proxy, arbitrary property dispatch or token forwarding.
- API methods are an explicit typed allowlist of public Core client operations required by migrated UI. Host adapter is infrastructure over the retained Core engine, not a sync UI/workflow controller. Plugin/account identity and existing Library binding ownership are checked; plugin lifecycle/permission state is rechecked at each dispatch. Engine context/preflight/idempotence validation remains authoritative.
- Minimal desktop bridge methods: `host.pickFolder`, `host.pickNewLibraryPath`, `host.revealPath`, `host.copyText`, `host.openPluginExternalLink`, `host.requestOAuthReturnFocus`, `host.changed`, `host.close`. No unrestricted openPath/URL. Folder arguments reuse existing typed contracts. Reveals restricted to picked/current Library paths or exact owned local root paths, respecting the selected root context. External links limited to current plugin IDs and existing validated link identifiers.
- Plugin declares a new requested permission `{ kind: 'host-ui', operations: ['core-api', 'pick-folder', 'reveal-path', 'clipboard-write', 'external-link'] }`; operations bound to UI calls, existing permission acceptance drives grants. Unknown values reject.
- Generic Host query invalidation after successful plugin mutations uses a generic library/plugin refresh notification, not sync-specific query keys in plugin management.
- Host pins the verified document lifecycle epoch and current Library context revision. It revalidates lifecycle after asynchronous ownership checks, injects the fixed revision through typed mutation adapters and sends the pinned epoch with Core calls. A successful self configuration/account mutation that changes the epoch or account acknowledges success before revoking the old iframe; Host shows “插件状态已更新，请重新打开视图继续”. Reopening obtains a new document/session. Never automatically continue across disable/re-enable or inherit prior session grants. Server-owned operations remain recoverable through their public APIs.
- Core serializes UI admission against mutating plugin, Library and sync APIs, including ordinary non-UI callers. It checks current target/account/plan ownership after admission, so a binding change without an epoch change cannot redirect an old plugin action. Self mutations acquire the write lease directly; cancelling an HTTP waiter does not release an in-flight handler's lease. This adds an interface safety fence, not a second synchronization engine.
- Legacy review-placement notification actions open generic plugin management with the label “管理插件”; Host settings no longer resolves old feature-specific sync-folders navigation or renders a built-in feature panel.

## Ownership and delivery

Desktop notification contribution entries cross a narrow list/open IPC boundary.
The notification window has no Core URL, session token or general shell bridge.
Main validates the contributed slot and current Library context before handing a
pinned view intent to the existing plugin window; that window uses the shared
isolated view launcher. A missing restricted IPC implementation does not trigger
a direct Core-client fallback. This preserves the notification window's existing
privilege boundary while allowing plugin-owned status documents.

Host owns generic view metadata, integrity/loading, iframe lifecycle, bridge and slots, and public SDK contract. Plugin owns feature UI/components/models/styles/tests plus account workflow composition. Host local Library wizard is create-only. Ordinary add-folder always stays local; plugin contributes its own enrollment action.

Independent plugin consumes a versioned, locally packaged SDK artifact (not a sibling checkout/path dependency). SDK can expose existing generic design primitives and Core DTOs/bridge adapters but no sync panels or workflows. Its build must have no OneDrive-specific input.

Existing explicit external-artifact staging stays in scope. A new runtime marketplace/installer/updater is not introduced. Validate independent Host build without any plugin checkout and plugin build against the published SDK artifact.

## Verification

- Ordinary Core notifications serialize `managedRootAction: null` when there is no managed-root action. The OpenAPI/client accept null or omission; Desktop validates objects only for non-null values. A null action must survive HTTP ingestion, SQLite persistence, ACK and store reopen without degrading notification delivery. Malformed non-null actions still reject atomically without advancing the cursor.

- Manifest/schema/client alignment and artifact tamper/path/extra-file cases.
- Isolation, denied/unknown bridge methods, stale window/session, disabled/replaced plugin, cross-plugin IDs/accounts/bindings, bounded messages and error/abort handling.
- Mounted no-plugin UI: no restore/sync labels/queries; local create/add/switch works.
- Installed plugin artifact contributes real settings/restore/enrollment views; actual sandbox bridge and representative preflight-before-commit flow tested without live accounts/cloud writes.
- Preserve migrated workflow tests in plugin repo; Host tests cover generic extension slots and negative absence, not plugin business details.
- Full Host gate, independent plugin checks/build/package and real Desktop layout/interaction evidence. Do not claim live remote success from fixtures.

### Executed evidence (2026-09-23)

- Full Host gate: 15 artifact, 6 SDK, 63 client, 219 Desktop, 464 Web, 825 Core and 8 process tests passed; 13 Rust cases explicitly ignored. Structure/API/schema/UI-design checks passed.
- Independent UI: typecheck and 18 tests passed; isolated temporary checkout builds using only the vendored SDK. Package/evidence tests (8) and copied-emulator process tests (12) passed. Production artifact remains emulator-free.
- Real Electron + Host + external plugin artifact: view loading, unbound local Library handling, restore dialog keyboard isolation and disabled-session revocation passed (`desktop/plugin-zm6iTg/result.json`). No account configured.
- Default debug and packaged desktop: no built-in restore/sync settings; real screenshot evidence at `desktop/local-CrtXtt` and `desktop/local-ygmi24`. Windows OS-backdrop passed. Default artifacts restored to fake-only.
- All evidence paths above are under `.tmp/plugin-ui-extraction-20260923/`. No live service actions or commit/push performed.

### Notification-center follow-up (2026-09-23)

- Added the generic `notification-center` slot end to end, including artifact validation, restricted notification-window list/open IPC, and the existing isolated plugin-view window. No external plugin checkout was changed. A real plugin must declare the slot and own the document it opens.
- Reproduced the misleading limited-status warning with ordinary Core scan notifications: `managedRootAction: null` failed the Desktop worker's object validator. Aligned null/omitted action handling and the wire contract. Real SQLite/ACK/reopen and malformed-object regression tests cover the boundary.
- Fixed a second real transport error: ACK omitted `x-mangoo-context-revision`. Outbox response Library identity is validated before persistence; ACK uses that page's fixed revision, including pending-ACK recovery. Cross-Library and A-to-B-to-A races still reject.
- Real no-plugin Desktop: `.tmp/notification-extension-desktop-20260923/local-8iUJWK/result.json` records `notificationHealth.state = healthy`, with no warning or plugin section. The earlier `local-ZkpD6X` captures the original null-action failure.
- Real declared test artifact: `.tmp/notification-extension-desktop-20260923/notification-jVBkeS/result.json` verifies the credential-free notification window, opening the plugin-owned status document with the correct Library, and disable/entry/view revocation through the normal plugin-change broadcast. No account or remote service was used.
- Notification-window state/history/log/responsive evidence is preserved in `.tmp/notification-extension-evidence-20260923/`; the separate native OS-backdrop check passed at `.tmp/notification-extension-os-backdrop-20260923/os-backdrop-result.json`.
- Desktop 230 and Web 469 tests passed on the final tree. One retry encountered an existing lifecycle HTTP-test timeout; isolated reproduction passed 13/13. Default-parallel Rust hit two real-child startup timeouts; both passed the sequential focused rerun. Full Rust passed with `RUST_TEST_THREADS=2`: Core 825 and process 8 tests, with 13 explicitly ignored cases, in `.tmp/notification-extension-rust-final-20260923.log`. Final full-gate non-Rust results are in `.tmp/notification-extension-final-gate-retry-20260923.log`; no single default-parallel final gate run is claimed green.
- The running production package was not replaced. Source and fresh debug/Desktop builds were verified; normal production launch must rebuild before these changes appear there. No commit or push was performed.
