# Checks

Fast checks that should run during normal development:

- `validate-structure.mjs`: repository shape and contract presence.
- `validate-core-api.mjs`: OpenAPI, Rust route constants, router, and migration-runner alignment.
- `validate-desktop-core.mjs`: Electron/Core process lifecycle and session bridge checks.
- `validate-web-client.mjs`: Web Core client package boundary and renderer safety checks.
- `packages/core-client/scripts/check-contract.mjs`: `@mangoo/core-client` generated-boundary alignment with the Core API contract.
- `validate_sqlite_schema.py`: SQLite migration smoke test and required tables/indexes.
- `verify-rust.mjs`: Rust toolchain detector. It skips cleanly until Rust is installed.

Run from the repository root:

```text
npm test
```

These checks intentionally avoid Rust and installed frontend dependencies until the toolchain is available.
