# Megle

Windows-first local media browser and manager for existing folders.

Current phase: Phase 1 skeleton. The repository is contract-first so the Electron desktop shell, reusable React UI, Rust Core, SQLite schema, plugin protocol, and tests stay aligned.

## Unified Notification Center

Megle includes a persistent all-library notification center with aggregated task progress and local process logs. Notification history survives library switches and desktop restarts while active work remains grouped into concise operation-level status.

### Capabilities

- **Scope**: `all-library-persistent`
- **Progress**: `aggregated-logical-task-progress`
- **Logs**: `local-process-logs`

## Start Here

- [Docs index](docs/README.md)
- [Final solution](docs/final-solution.md)
- [Project structure](docs/project-structure.md)
- [Testing strategy](docs/testing-strategy.md)

Fast verification:

```text
npm test
```

The verification command checks repository structure, Core API alignment, SQLite migration behavior, Rust formatting, and Rust workspace tests. If the current terminal PATH has not picked up Rust yet, `tools/checks/verify-rust.mjs` also checks the standard `.cargo/bin` install path.
