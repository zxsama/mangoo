# Mangoo identity and integration verification — 2026-11-03

## Delivered scope

The product identity is Mangoo throughout the Host, Core API/client, plugin SDK,
Electron shell, Web UI, launchers, contracts, tests and maintained documentation.
Package scopes, crate names, environment variables, IPC/bridge identifiers,
Library markers and plugin IDs use the new identity. There are no former-name
aliases or automatic migration paths. Existing user data is preserved rather
than silently opened or rewritten.

The working checkouts are `D:\Mangoo` and `D:\Mangoo_OneDriveSync`. Five absolute
npm workspace junctions were recreated under the new Host root. The Host remote
is `https://github.com/zxsama/mangoo.git`. The independent provider has no Git
remote configured; Host synchronization does not publish that repository.

The working tree already contained Library creation/storage separation,
appearance controls, expanded folder icon catalogs, folder sorting/tree layout,
shared dropdown/outline refinements, all-file browsing and video hover work.
These changes were preserved and validated together. An exact pre-rename copy
and Git status/patch inventory remain in ignored local evidence.

## Brand assets and startup

`apps/web/public/favicon.svg` is the editable source: a golden mango with a
leaf and an M. The existing generator produces the 256px PNG and a Windows ICO
with 16/24/32/48/64/128/256px entries. Desktop prebuild generates these resources;
development and packaged main/secondary windows share the icon resolver.

Electron identifies as Mangoo / `com.mangoo.desktop` before profile access.
The default profile is the Mangoo application-data directory, and development
and production launchers use `.data/mangoo/host`. Explicit profile/Host storage
overrides remain honored. Electron remains pinned to 43.1.0.

The independent provider uses `com.mangoo.onedrive` and a genuine rebuilt
`mangoo-plugin-sdk-0.1.0.tgz`. Its production and emulator artifacts were tested
without accessing a live account. The main package is built with explicit
external-artifact selection so compiled trust matches the bundled bytes.

## Defects found during integration

- A thread-local read connection could return the previous Library's roots
  with the newly activated Library's response identity. Reads now lease from a
  bounded cache owned by one Library runtime. The runtime is captured before
  dispatch, so a cancelled queued request cannot adopt another Library.
  Retirement waits for actual connection release.
- Task shutdown inferred worker ownership from persisted task status and could
  overlook dispatched or finishing workers still holding Windows database
  handles. Completion notifications now determine ownership. An RAII guard
  signals only after Database and permit release, including unwind paths.
- Accepted activation requests now retain their operation owner after the HTTP waiter disconnects, matching open/forget. A real Router regression cancels after shutdown drains its background handles and proves retry and Host exit still wait for native release.
- The Library-switcher fixture was updated to open the Database disclosure
  before choosing a directory, matching the current creation UI.
- The visual harness uses available local ports and an exact-origin,
  fixture-only window lifecycle hook to show its test child windows. This
  hook is disabled for ordinary product launches and has focused origin,
  path, redirect and lifecycle tests.

- The notification HTTP fixture could receive a Fetch-blocked low port from Windows. It now binds its actual server in the usable high-port range with bounded collision retries. A forced bad-port reproduction failed before and passed after this test-only correction; notification assertions remain unchanged.

## Validation status

The relocated `npm test` run passed the non-Rust gates: structure, plugin
artifact checks (16), plugin SDK (7), Core API, core-client (74), notification
contract, Desktop (240), Web (528), UI-design and SQLite schema. Its Rust phase
exposed moved build-cache binaries with compile-time executable paths pointing
to the former root. These integration targets were recompiled without changing
their test semantics.

After the final activation cancellation correction, fresh checks passed 897
Core tests (14 pre-existing ignored), 4 Core process integration tests, 4 fake
Provider process integration tests, and 2 PPM unit plus 3 PPM process integration
tests: 910 Rust tests passed in total, with 14 unchanged ignored tests. The exact
workspace-feature test executables were refreshed and rerun
for the remaining crates as well. This is combined gate evidence, not a claim
that one uninterrupted final `npm test` invocation succeeded.

Production Web and Desktop builds passed from `D:\Mangoo`. Vite's existing
chunk-size advisory remains; it is a build advisory rather than a startup error.
The independent provider also passed `npm test` and `test:emulator` from its new
root (36 tests each, with 2 live-service cases intentionally ignored), followed
by complete `package:verify` production/emulator builds and packaged process
conformance. No live account operation was performed.

The final Host release was rebuilt from the new root with the explicitly
selected production OneDrive artifact, then packaged as
`.data/desktop-release/win-unpacked/Mangoo.exe`.

- Raw HTTP verification: 4 Libraries, 12 roots reads per Library, correct body
  and Library identity throughout activation (`raw-library-identity-sVl77s`).
- Packaged smoke: main/secondary windows and native icon resources, two-Library
  creation/switching, format-2 markers, Database/Files exclusion, plugin iframe
  handshake and accountless UI, and notification health passed
  (`brand-smoke.run-xRKBp9`).
- The final run recorded zero console issues, HTTP error responses or runtime
  exceptions. No diagnostic category was filtered. All test-owned processes
  exited. This result is independent of the legacy visual-harness limitation
  below.
- Packaged executable SHA-256:
  `489d4447ffd732cf3bde4c7d7eea12d1861ccbec49511b3ccbfe6937281d4530`.
- Packaged Core SHA-256:
  `b19e7e90b866333482d092fa96566155d9ed6638dbd0fdf02b2cec542f28f589`.

The maintained-source name audit found no previous product identity. Final
format and diff checks passed. The explicit staging manifest excludes runtime
data, caches, build outputs and local verification evidence.

Focused Core evidence after both lifecycle corrections: 47 task tests, 14
storage tests, 80/80 repetitions of the former Windows handle failure at
concurrency 8, and both real activation/queued-cancellation API regressions.

## Visual evidence boundary

The full legacy OS-backdrop harness does **not** pass. It reaches native input
and compact notification scenarios, then fails at an obsolete Host sync-panel
fixture removed when that UI moved into the independent plugin. Final aggregate
pixel/material assertions are therefore not reached. No scenario was skipped
or weakened to report this gate as green. Packaged brand/Library/plugin smoke
results are separate evidence and do not replace the full material gate.

An earlier inspector startup exception was isolated to CDP Runtime commands
sent before Electron's initial page load, including a minimal no-preload
reproduction. The smoke harness now waits for Page load before enabling Runtime;
ordinary-launch security settings and preload isolation were not changed.

## Local cleanup

4,809 enumerated obsolete generated entries were removed, including former
product binaries, matching Cargo outputs, obsolete plugin packages and old
bundle backup directories. Current artifacts, dependencies, source, user data
and validation evidence were preserved. No compatibility junction was created.

All source and Git metadata are at the new roots. An empty shell at the former
Host root is still held by the running conversation applications; no source,
Git object or user file remains there. A hidden, bounded local helper waits up to 24 hours for those holders to release
it, then preserves the empty shell under ignored evidence. It refuses to move
anything if new content appears. Its state is recorded in local
`directory-move.json`; reopen subsequent development from `D:\Mangoo`.

Detailed logs, hash inventories, isolated test data and screenshots are under
`.tmp/mangoo-rename/`; they are intentionally excluded from Git. Historical
benchmark records retain their original measurement context and are not fresh
performance measurements of this change.
