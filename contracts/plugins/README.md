# Plugin Contracts

Manifest v2 defines trusted, bundled native process plugins. The first stable
capability is `sync-provider.v1`; plugin installation, permission grants,
accounts and runtime health are Host-owned and never stored in a Library
Catalog. The manifest-declared SHA-256 is descriptive. Core's embedded
platform allowlist is the executable trust root.

The default build includes only the fake conformance Provider. Production
Providers such as OneDrive Sync are maintained independently and can be selected
with `node tools/build/build-bundled-plugins.mjs --external-plugin <artifact-root>`.
The build validates the selected manifest and executable, stages the artifact,
and generates exact compiled trust. Rebuild Core after that selection before
starting or packaging the Host. The packaged-launch entry point
`node tools/dev/run-prod.mjs --external-plugin <artifact-root>` preserves that
order. This integration adds no runtime arbitrary-plugin loader, marketplace or
installer. See [the extraction note](../../docs/onedrive-extraction.md).

`fixtures/valid-sync-provider-v2.json` is the canonical cross-language fixture.
Files prefixed with `invalid-` must be rejected by both the JSON-schema check
and Rust decoder tests.

The `rpc.methodVersions` object is required and closed. A
`sync-provider.v1` manifest declares every v1 method at version `1`; unknown,
missing, or differently versioned methods are rejected before registration.

Optional `contributions` declares bounded configuration fields, authentication
modes and redirects, presentation/help text, and a primary credential scope.
The JSON schema and Rust decoder validate these declarations before use.
Configuration is non-secret; fields may be locked by declared environment
overrides. Configuration/help metadata remains plain text. Optional `contributions.ui`
owns feature presentation through one bundled UTF-8 HTML document, loaded only
inside the Host sandbox. Secret values remain behind the Host's scoped secret broker.

UI v1 declares `{ apiVersion: 1, entry, sha256, views: [{ id, title, slot }] }`.
Supported generic slots are `library-menu`, `library-settings`, `plugin-detail`,
`root-menu`, and `notification-center`; at most 16 views share the single document (8 MiB maximum).

`notification-center` adds a plugin-titled entry to the notification center's
current-status page. On Desktop, the restricted notification window obtains
entry metadata through narrow IPC and asks the Host to open the verified view
in the existing plugin window. It never receives Core credentials. Opening an
entry loads the plugin's own status document through the same isolated view
bridge. The Host does not infer a provider's status or
poll the sync engine for this slot. No available contribution means no entry.
For example: `{ "id": "status", "title": "Service status", "slot": "notification-center" }`.
Local notification delivery/storage health remains Host-owned; the existing
`NotificationCompactSnapshot.syncHealth` field describes local notification
delivery, not a plugin's synchronization status.
The entry is an ASCII forward-slash relative `.html` path. Scripts, styles and
dependencies must be bundled into it. The artifact contains exactly `plugin.json`,
the selected platform executable and optional UI document. Inventory checks reject
extra files, links, altered bytes and unsafe paths. Core's compiled trust pins both
the executable and exact manifest hashes; the manifest then pins the UI hash.

UI requires requested permission `{ kind: "host-ui", operations: ["core-api"] }`,
with optional `pick-folder`, `reveal-path`, `clipboard-write`, `external-link`.
All values are closed and grants use the existing acceptance lifecycle. Unknown
values and duplicate operations/view IDs are rejected. Missing `ui` remains valid.
`GET /plugins/{pluginId}/views/{viewId}` returns `HostPluginViewDocument` only while
the trusted selected version is enabled, granted and neither stopping nor
incompatible. Configuration/authorization can be incomplete; reading UI never
starts a Provider. The Host supplies the sandbox, restrictive CSP and bounded
typed message bridge described by [the UI contract](../../docs/superpowers/specs/2026-09-23-plugin-owned-ui.md).

The view document and Host projection expose the persistent `lifecycleEpoch`.
The sandbox's isolated Core client pins `{ pluginId, lifecycleEpoch }` as
`X-Mangoo-Plugin-UI-Id` and `X-Mangoo-Plugin-UI-Epoch` on every delegated request.
Core obtains UI admission before checking the selected installed epoch and
grants. Plugin management mutations (including discovery and auth callbacks)
take an exclusive admission lease; a self configuration/sign-out request takes
that lease directly. A waiting lifecycle change prevents new UI readers from
passing, and disabling/re-enabling cannot revive an old epoch. Request cancellation
does not release an executing handler's lease before its work finishes.
Changes to effective configuration and sign-out also advance the epoch: the Host
must load a fresh view document to continue. The admission gate orders API
handlers only; existing engine generation fences still protect background work.
Active-Library context revisions remain an independent mutation requirement.

Library/sync mutations also acquire that exclusive lease, including ordinary
Host requests. After acquisition Core decodes router-owned path parameters and
the endpoint's existing request type, then checks current binding/account,
restore, and persisted plan ownership. A binding changed to another plugin or
account returns `409` / `plugin-ui-target-changed` even if the plugin epoch and
Library context revision did not change. Unowned local targets are accepted only
for explicit setup/enrollment, reconnect and restore cases; destructive commands
require current or retained detached provenance. Global Library metadata remains
available. Opening a view does not grant global wake or Library creation actions.

Shared configuration uses `/plugins/{pluginId}/configuration` and its revisioned
plan/commit flow. Browser navigation uses `/plugins/{pluginId}/external-link`
with a declared help-link ID or exact pending authorization-flow ID. Desktop
resolves those identifiers through its authenticated Core client and validates
the resulting URL; the renderer does not supply an arbitrary browser URL.
Service-specific authorization endpoints, protocol parameters and storage
requests remain inside the provider process.

Disabling a provider closes admission and joins its process, streams and owned
work before publishing the final disabled state. Reenable uses a new lifecycle
epoch and retains saved authorization and durable work. It resumes only work
suspended by that plugin transition, preserving explicit user pauses.

See [the all-file sync scope and ownership contract](../../docs/superpowers/specs/2026-09-16-all-file-plugin-sync.md)
for the corresponding local capture, compatibility and acceptance boundaries.

## Host engine

`engine: "host"` declares a trusted bundled feature implemented by the Host itself
(Core, Desktop and Web) from the plugin directory, e.g. the built-in image viewer
`com.mangoo.image-viewer`. A Host manifest declares `host: { capabilities: ["image-viewer.v1"] }`
and omits `entry`, `rpc`, `integrity` and `contributions`. Its only activation events are
`file-open-requested` / `diagnostics-requested`, and its only permissions are
`{ kind: "local-files", access: "opened-folders" }` and `{ kind: "file-association", scope: "current-user" }`.
Process manifests cannot declare Host capabilities, permissions or activation.

The artifact is exactly `plugin.json`. Compiled trust pins its digest with a zero executable digest,
and the inventory row is `{ id, version, platform, engine: "host", manifestSha256 }`.
Enable, grants and disable use the ordinary lifecycle; an enabled and granted Host plugin is `ready`
with its capabilities published, and Core gates each Host service on
`HostRegistry::host_capability_enabled`. `fixtures/valid-host-image-viewer-v2.json` is the canonical fixture.
See [the image viewer design](../../docs/superpowers/specs/2026-11-08-image-viewer-plugin.md).
