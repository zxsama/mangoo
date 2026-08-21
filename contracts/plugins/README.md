# Plugin Contracts

Manifest v2 defines trusted, bundled native process plugins. The first stable
capability is `sync-provider.v1`; plugin installation, permission grants,
accounts and runtime health are Host-owned and never stored in a Library
Catalog. The manifest-declared SHA-256 is descriptive. Core's embedded
platform allowlist is the executable trust root.

`fixtures/valid-sync-provider-v2.json` is the canonical cross-language fixture.
Files prefixed with `invalid-` must be rejected by both the JSON-schema check
and Rust decoder tests.

The `rpc.methodVersions` object is required and closed. A
`sync-provider.v1` manifest declares every v1 method at version `1`; unknown,
missing, or differently versioned methods are rejected before registration.
