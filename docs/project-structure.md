# Project Structure

Megle uses a contract-first monorepo so the desktop shell, reusable Web UI, Rust Core, database schema, plugin protocol, and tests do not drift apart.

## Top-Level Layout

```text
apps/
  desktop/          Electron shell and Windows desktop adapter.
  web/              React UI reused by desktop and future Web/Docker.
    src/design/overlays/ Shared overlay geometry, stack, focus, material, menu, dialog, and field primitives.
contracts/
  core-api/         HTTP API contract shared by UI, desktop, Core, and tests.
  plugins/          Plugin manifest and permission contracts.
bundled-plugins/     Source manifests for trusted Providers packaged with Megle.
crates/
  core/             Rust Core service binary and Phase 1 backend modules.
    tests/           Cross-process Core integration and fault tests.
  fake-provider/    Scriptable framed-process sync Provider used by conformance and benchmarks.
packages/
  core-client/      Generated-package boundary for the TypeScript Core API client.
  notification-contract/ Shared type-only Desktop/Web notification IPC contract.
docs/               Product, architecture, decisions, benchmark reports.
tools/
  bench/            Reproducible performance benchmarks and generated-data tools.
  checks/           Fast structural/schema checks used during normal development.
```

## Ownership Rules

- `apps/desktop` owns only desktop concerns: window lifecycle, native dialogs, session token handoff, starting/stopping Core, and desktop-only adapters.
- `apps/web` owns presentation and interaction only. It does not read or mutate the filesystem directly.
- `contracts` owns cross-process boundaries. API paths, DTO names, plugin manifest fields, and permissions start here before implementation.
- `crates/core` composes API, database, queues, tasks, and service lifecycle. During Phase 1, `api`, `db`, `roots`, `scan`, `thumbnails`, `fsops`, and `plugins` live as modules inside this crate.
- `crates/core/src/plugins/runtime` owns verified plugin process launch, framed RPC/binary streams, provider adaptation, and supervision. `crates/core/tests` exercises real child-process and process-tree faults rather than replacing those boundaries with mocks.
- `crates/fake-provider` implements the portable child-process conformance target. `bundled-plugins` supplies its manifest template; `tools/build/build-bundled-plugins.mjs` produces the executable bundle and exact build-time trust hash under Cargo's target directory.
- Future Rust crates should be split out only after the module boundaries are stable and tests show real separation pressure.
- `packages/core-client` owns the TypeScript Core API client/types boundary as `@megle/core-client`. Phase 1 keeps a hand-maintained generated-output placeholder aligned by a fast contract check until full generator output replaces it.
- `packages/notification-contract` owns the type-only notification, process-log, and preload bridge DTOs shared by Desktop and Web. It reuses generated Core operation types and has no runtime Node dependency.
- `tools/bench` is for heavier performance experiments. It may generate large ignored artifacts.
- `tools/checks` is for quick checks that should run every time the structure changes.

## Unified Notification Center Modules

- `crates/core/src/db/notification_outbox.rs` and migration `0021_notification_outbox.sql` own each library DB's operation heads, revisioned Outbox, acknowledgements, and the `file_operations.operation_group_id` grouping link. Core audit endpoints remain available for legacy migration and operation history.
- `apps/desktop/src/notification-service.ts` owns Outbox synchronization, cross-library snapshots, history/actions, and degraded fallback behavior. `notification-store.ts` plus `notification-store-worker.ts` own the durable `userData/notifications.sqlite` mirror; `process-log-buffer.ts` owns bounded local process logs.
- `apps/desktop/src/notification-ipc.ts`, `notification-ipc-channels.cjs`, and `preload.cjs` expose the narrow typed renderer boundary. Shared DTOs and bridge signatures belong in `packages/notification-contract`, not in Desktop or Web copies.
- `apps/web/src/features/notifications/` owns the compact notification popover, standalone status/history/process-log window, presentation models, and subscriptions. It consumes preload snapshots/actions only: no raw notification fetch, polling loop, or per-thumbnail event rows.
- Every future background task must declare a stable logical grouping key and a presentation class (`important`, `history_only`, or `status_only`) before it enters the notification pipeline. One logical operation may update its head many times, but it must not fan out into notification spam.

## What Not To Create Yet

- Do not create a separate `server/` product for Web/Docker. Web/Docker must reuse Core and `apps/web`.
- Do not create feature-specific Electron modules that bypass Core API.
- Do not create a plugin marketplace or browser extension implementation in Phase 1.
- Do not pre-split `crates/media`, `crates/indexer`, `crates/thumbnails`, `crates/fsops`, or `crates/plugins` as separate Cargo packages before the `crates/core` module boundaries stabilize.
- Do not create thumbnail pack storage until hash-sharded file cache pressure is measured in the integrated app.
- Do not create large generated data outside ignored benchmark/cache directories.

## Contract Placement

- Core API: `contracts/core-api/openapi.yaml`
- Plugin manifest: `contracts/plugins/manifest.schema.json`
- Per-Library catalog SQLite migrations: `crates/core/migrations/`
- Host Registry SQLite migrations: `crates/core/host_migrations/`; `host.sqlite` owns Library UUIDs, managed-root bindings, active context revision, and creation/removal journals, while inactive local-only catalogs stay closed.
- Phase-level implementation notes: `docs/implementation-roadmap.md`
- Performance benchmark results: `docs/performance-results/`
- Managed-Library lifecycle/load harness boundary: `tools/dev/managed-library-harness.mjs`; the reproducible AC15 baseline driver is `tools/bench/managed-library-lifecycle-bench.mjs`.

## Anti-Patterns

- UI importing Node filesystem APIs to access media files.
- API DTOs duplicated independently in desktop, web, and Core.
- SQLite schema changes without a migration and a schema validation test.
- Long-running scan, decode, or file-operation work inside Electron main or renderer.
- Full result sets loaded into frontend memory.
- One unsegmented scroll container for multi-million-item views.
- Generated thumbnails or database sidecars written inside user media folders.
