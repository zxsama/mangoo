# Plugin runtime and sync Provider baseline — 2026-08-21

This baseline exercises the packaged `com.megle.fake-sync` executable through
the production framed stdin/stdout protocol. It is not an in-memory provider
simulation: the harness builds the bundled child, performs the handshake,
streams a 9 MiB immutable object in both directions with sliding credit,
completes a resumable upload and authentication, and issues concurrent delta
requests for eight Library namespaces through one process.

## Reproduce

Run from the repository root:

```powershell
npm run bench:plugin-runtime
```

`CARGO_TARGET_DIR` may be set to an alternate build directory; the packaging
and benchmark scripts resolve the bundled executable from the same target.

## Result

Environment: Windows 11 x64 (10.0.26200), Node 22.14.0, Rust/Cargo 1.95.0,
debug Provider built from the working tree.

| Scenario | Result |
| --- | ---: |
| Provider processes for eight Libraries | 1 |
| Concurrent eight-Library delta calls | 27.98 ms |
| Bidirectionally streamed object | 9,437,184 bytes |
| Process startup plus protocol exercise | 336.28 ms |
| Provider private memory | 10,194,944 bytes (9.72 MiB) |
| Provider RPC calls | 19 |
| Provider calls during 100 local browse probes | 0 |
| Local browse probe time | 1.40 ms |

The harness fails if the concurrent delta batch exceeds two seconds, any
Provider call occurs during the local browse probe, a pending RPC remains, or
the streamed object differs after the round trip.
