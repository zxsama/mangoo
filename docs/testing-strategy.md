# Testing Strategy

## OneDrive Provider

`cargo test -p megle-onedrive-provider` runs unit tests plus a real-child Graph emulator flow. `npm run check:onedrive-bundle` additionally hashes the packaged Windows executable and runs the same process conformance suite through that exact bundled path. `npm run bench:onedrive-provider` repeats the real-child flow in release mode. Live Microsoft Personal/Business checks are deliberately ignored unless an operator supplies credentials and invokes `npm run test:onedrive-live`; emulator success is never recorded as live-account evidence.

Tests and code should move together. Phase 1 starts with fast checks that enforce the repository shape, API contract, and database schema before the full Rust/Electron build is available.

## Test Layers

1. Structural checks
   - Verify required directories and contract files exist.
   - Verify generated benchmark directories are not kept in the repo tree.
   - Verify package scripts point to runnable checks.

2. Schema checks
   - Execute SQLite migrations against a temporary database.
   - Verify required tables and hot-path indexes exist.
   - Keep this runnable with Python so it works before Rust is installed.

3. API contract checks
   - Validate that Phase 1 endpoint groups exist in `contracts/core-api/openapi.yaml`.
   - Later generate TypeScript and Rust DTOs from the contract or a single schema source.

4. App and Core tests
   - Web UI: component tests for grid, preview, keyboard selection, task panel.
   - Desktop: smoke tests for window startup and Core launch/token handoff.
   - Rust Core: unit tests per crate plus integration tests for API, scan, thumbnail, and file ops.

5. Performance regression checks
   - Keep heavy tests under `tools/bench`.
   - Run small smoke variants locally.
   - Run 1M/5M gates before major architecture changes.

## Current Runnable Commands

```text
npm run check:structure
npm run check:core-api
npm run check:desktop
npm run check:web
npm run check:schema
npm run check:rust
npm test
```

`npm run check:rust` verifies workspace files and then runs Rust checks when `rustc`/`cargo` are available in PATH or the standard `.cargo/bin` install path:

```text
cargo fmt --all --check
cargo test --workspace
```

If Rust is unavailable in a future session, the check exits with a skip message instead of blocking Node/Python-only work.

## Gates Before Feature Work

- New directory: update `docs/project-structure.md` and `tools/checks/validate-structure.mjs`.
- New table/index: add a migration and update `tools/checks/validate_sqlite_schema.py`.
- New API path: update `contracts/core-api/openapi.yaml` before wiring UI/Core.
- New plugin capability: update `contracts/plugins/manifest.schema.json`.
