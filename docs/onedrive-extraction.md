# OneDrive Sync repository extraction

As of 2026-09-23, OneDrive Sync is maintained in the independent
[`Mangoo_OneDriveSync` repository](../../Mangoo_OneDriveSync/README.md). Relative
links assume sibling checkouts for local navigation; builds accept an explicit
artifact path and do not depend on that directory layout.

The plugin repository owns the Rust Provider, OAuth and Graph transport,
manifest template, **all synchronization UI and user workflows**, emulator
process tests, benchmark, and provider release requirements. This includes
account setup, remote restore, enrollment and synchronized-folder management.
Mangoo's default build includes only the fake conformance
Provider. The former `crates/onedrive-provider/`,
`bundled-plugins/com.mangoo.onedrive/`, and
`tools/bench/onedrive-provider-bench.mjs` are no longer Host source paths.

## Explicit build-time integration

Build and package the plugin in its own repository first. To stage any selected
plugin artifact in Mangoo, run:

```powershell
node tools/build/build-bundled-plugins.mjs --release --external-plugin '<artifact-root>'
```

`<artifact-root>` contains `plugin.json`, its declared UI document and the executable for the target
platform, for example the independent plugin's `dist/com.mangoo.onedrive`.
The Host checks plugin identity, version, platform entry, exact artifact inventory
and SHA-256 for the manifest, executable and UI document, stages the
artifact, and generates the compiled trust entry. Core must be rebuilt after
the selection. Subsequent build/package steps must receive the same explicit
selection so a default build does not replace it with the default bundle.

For a packaged Windows launch, use the entry point that performs this order:

```powershell
node tools/dev/run-prod.mjs --external-plugin '<artifact-root>'
```

There is no runtime arbitrary-plugin loader. A manifest cannot grant itself
trust, and the Host does not discover source code from a neighboring checkout.

## UI ownership and retained Host contracts

Mangoo retains the **generic Core sync engine**: scheduling, local capture,
conflict handling, upload/restore execution and safety fences. It also retains
framed process RPC, supervision, Secret Broker and generic configuration/auth
interfaces. Host UI only owns local Library operations and generic contribution
slots, document isolation/lifecycle, the restricted bridge and design primitives.
There is no built-in sync panel, restore wizard, sync polling controller or
sync-folder routing hidden behind an enabled-plugin check.

The optional manifest UI contribution and `GET /api/plugins/{pluginId}/views/{viewId}`
are documented in [the UI boundary](superpowers/specs/2026-09-23-plugin-owned-ui.md).
Plugin documents run in an opaque-origin iframe, without the Host session token
or Electron preload. The standalone plugin consumes a versioned SDK tarball;
neither repository builds from the other's source directory.

Existing plugin/account identity and old database migrations remain compatible.
At the user's explicit request, the test Host's old OneDrive installation,
account and synchronization bindings were cleared separately; actual media
and remote objects were not deleted. Recovery evidence is under
`.tmp/onedrive-test-cleanup-ZY7GLM/`.

The Host fault matrix has 16 generic cases. The four Graph/emulator fault groups
and Personal/Business transport plus product OAuth release requirements belong
to the independent plugin. See its
[authorization and release guide](../../Mangoo_OneDriveSync/docs/authorization-and-release.md).

Earlier benchmark reports, combined sync plans and verification reports remain
in their original locations as historical evidence. Their old source paths and
combined release gates describe the tree and checks at the recorded date.
They do not establish completion for the extracted repositories. No new live
service validation was run for this extraction; outstanding independent restore
and phase performance budgets from the earlier sync validation remain outstanding.

## Initial Provider extraction verification

The initial Provider-only extraction was checked on 2026-09-23 (local time),
before the subsequent UI migration below:

- `npm test` passed: structure and artifact checks (11), Core client (59),
  Desktop (218), Web (451), UI design, schema, Core unit tests (812), and
  process conformance (8). The full Rust run retains 13 explicitly ignored
  cases; they are not counted as passed.
- `node tools/checks/sync-integration.mjs --quick` passed all 14 stages,
  including three isolated Hosts with actual framed test-provider processes,
  fresh restore, and the 16-case generic fault matrix. This is local integration
  evidence, not a real cloud-service run.
- `node tools/checks/desktop-os-backdrop.mjs` passed in a separate temporary
  Host and Electron profile. The working tree was dirty, so this is not evidence
  for a clean committed release.
- The standalone plugin passed its 36 unit/transport tests, 12 copied-emulator
  process tests, 7 build/evidence tests, and the copied production executable's
  identity and emulator-isolation checks. Its production artifact was also
  selected through the generic Host builder and verified against compiled trust;
  the Host default selection was restored afterward.

Local evidence and the original source snapshot remain under
`.tmp/onedrive-extraction-20260922/`; standalone evidence is under that
repository's `.tmp/`. The removed source, template and benchmark can be
recovered from the snapshot. Those initial checks did not update the production package.
Docker execution, live-service checks and a clean committed release remain
outside this verification. Neither repository was committed or pushed.

## Plugin-owned UI verification

The later UI extraction passed the full `npm test` gate: artifact tests (15),
SDK (6), Core client (63), Desktop (219), Web (464), UI design, schema,
Core unit tests (825) and process conformance (8). Rust retains 13 explicitly
ignored cases. Evidence: `.tmp/plugin-ui-extraction-20260923/full-gate-rerun.log`.
An earlier full run exposed three fixture-path assumptions and they were fixed
without relaxing production path validation; a later unrelated overlay-focus
timing failure passed on serial rerun and is retained in the earlier log.

The independent plugin passed UI typecheck and 18 UI tests, 8 package/evidence
tests, 12 copied-emulator process tests and production-artifact verification.
Its separate temporary-directory install, typecheck, build and five artifact
tests use only the vendored SDK, not the Host checkout. No real account was
configured and no live remote-service claim follows from these local checks.

For repeatable real-desktop acceptance, build Web/Desktop/Core and invoke:

```powershell
# Default Host artifact selection; OneDrive is absent.
npm run check:plugin-ui-desktop -- --mode local
# After explicit external-artifact selection and Core rebuild.
npm run check:plugin-ui-desktop -- --mode plugin
```

The harness uses a fresh isolated Host and Electron profile. It checks actual
default menu/settings absence, packaged plugin view loading, inner dialog
keyboard handling and disable revocation without configuring an account.

Both modes passed: `desktop/plugin-zm6iTg/result.json` and
`desktop/local-CrtXtt/result.json` below `.tmp/plugin-ui-extraction-20260923/`.
The independent plugin document hash was
`71012515f8712d91ee79b0ab8d56cca258eeb7c600149728735f373c141ce0c3`.
The Windows OS-backdrop harness also passed with an actual native capture;
see `os-backdrop/os-backdrop-result.json`. Screenshots are working-tree evidence,
not a clean committed release or proof of real-account synchronization.

The default `npm run build:desktop-package` completed and the resulting
`.data/desktop-release/win-unpacked/Mangoo.exe`, packaged Core, Web assets and
plugin inventory passed the same isolated local-mode acceptance
(`desktop/local-ygmi24/result.json`). The packaged and debug inventories contain
only the fake conformance Provider. A read-only check of `.data/host/host.sqlite`
confirmed no OneDrive installation, account, configuration or sync binding.
Use the rebuilt executable (or the normal launcher) after closing any old window.
