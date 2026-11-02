# Mangoo Core

Rust Core service.

Phase 1 responsibilities:

- expose the local Core API
- own SQLite migrations and connection setup
- add/remove roots
- scan existing directories into SQLite
- serve keyset-paged media results
- enqueue thumbnail and preview work
- execute real file-operation tasks through safe adapters
- reserve plugin registry and manifest validation

Rust is not currently installed on this machine, so repository default checks validate structure and migrations with Node/Python first. Add `cargo fmt` and `cargo test` to the default check once Rust is available.
