# Mangoo

<img src="apps/web/public/favicon.svg" alt="Mangoo" width="96" height="96" />

Windows-first local media browser and manager for existing folders.

Current phase: Phase 1 skeleton. The repository is contract-first so the Electron desktop shell, reusable React UI, Rust Core, SQLite schema, plugin protocol, and tests stay aligned.

Mangoo uses the `@mangoo/*` packages, `MANGOO_*` configuration and its own storage identity. Desktop launchers keep Host state in `.data/mangoo/host`; new Libraries default to `Documents/Mangoo/Libraries/<name>/`, with separate `Database` and `Files` directories. Existing profiles and Libraries from the previous product identity are not migrated.

## Unified Notification Center

Mangoo includes a persistent all-library notification center with aggregated task progress and local process logs. Notification history survives library switches and desktop restarts while active work remains grouped into concise operation-level status.

### Capabilities

- **Scope**: `all-library-persistent`
- **Progress**: `aggregated-logical-task-progress`
- **Logs**: `local-process-logs`

## Start Here

- [Docs index](docs/README.md)
- [Final solution](docs/final-solution.md)
- [Project structure](docs/project-structure.md)
- [Testing strategy](docs/testing-strategy.md)

## Optional process plugins

Mangoo's default build contains only the fake sync Provider used for development
and conformance. OneDrive Sync is maintained in the independent
[`Mangoo_OneDriveSync` repository](../Mangoo_OneDriveSync/README.md).

To include an independently built plugin in a packaged Windows launch, select
its artifact directory explicitly:

```powershell
node tools/dev/run-prod.mjs --external-plugin '<artifact-root>'
```

The artifact directory contains `plugin.json` and its platform executable.
The build validates and stages the artifact, generates exact executable trust,
then rebuilds Core before packaging. This is an explicit build-time selection;
copying a manifest into a running installation cannot grant executable trust.
See [plugin ownership and extraction](docs/onedrive-extraction.md) for the
separate build step and retained compatibility boundaries.

## Verification

Fast verification:

```text
npm test
```

The verification command checks repository structure, Core API alignment, SQLite migration behavior, Rust formatting, and Rust workspace tests. If the current terminal PATH has not picked up Rust yet, `tools/checks/verify-rust.mjs` also checks the standard `.cargo/bin` install path.
