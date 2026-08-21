# Plugin Runtime and Sync Provider v1

Megle owns synchronization policy in Core. A trusted bundled Provider is a
supervised native child process that supplies authentication and opaque,
immutable-object transport. It never receives a Catalog/Host database path,
media root, SQL statement, or another account's secret namespace.

## Trust and lifecycle

Only strict manifest v2 process plugins with `sync-provider.v1` may start.
The executable must remain beneath the platform bundled root and match the
Core-embedded `{id, version, platform, sha256}` trust entry. Versions install
side by side; enabled intent, runtime health, account and grants are separate
Host Registry facts. Catalog migration 0028 removes the former per-Library
plugin table.

Core uses private stdin/stdout pipes with a bounded framed protocol. The Host
sends `Hello`; the child must echo the nonce and select a supported wire and
method set within five seconds. Stream IDs are parity-separated, never reused,
and use cumulative credit. Malformed/oversized frames, stdout noise, credit
regression or sequence gaps terminate the session. Windows assigns the child
to a kill-on-close Job Object; Linux isolates a process group and installs a
parent-death signal.

Binary object bodies never use JSON/base64 control payloads. They travel on
parity-owned streams with monotonically increasing data sequence numbers,
cumulative receiver credit, bounded aggregate buffering, and a terminal
size/SHA-256 commitment. Deadline cancellation emits both request cancellation
and stream abort, and late terminal responses are drained without resurrecting
the request.

Windows creates the child suspended, checks the actual loaded image's stable
volume/file identity and hash, assigns the Job Object, then resumes it. Linux
opens each executable without following symlinks, rejects non-root-owned or
group/other-writable bundled path components, and executes the verified file
descriptor with `fexecve`, closing the verify-to-spawn pathname race.

Candidate versions enter a durable probation journal. Consecutive successful
health probes commit the candidate as known-good; activation, health, or Host
restart failures atomically restore the previous known-good version. Runtime
selection rejects a second switch while a verification, drain, or probation
journal is still open, so recovery always has one unambiguous fallback chain.
health uses bounded deadlines, degraded thresholds, exponential backoff, a
crash-window breaker, cooperative shutdown, process-tree termination, and task
join before Core shutdown completes.

## Synchronization boundary

One Provider process may serve multiple Library namespaces. The production
coordinator snapshots `{binding_epoch, transport_generation}` before I/O,
does network/RPC work without a DB writer or runtime lease, validates hashes
and sizes, durably registers spool objects, then commits the cursor. Push
receipts advance only for `Created` or `AlreadyExistsMatching`; a stale
generation cannot commit a receipt or cursor. A Library becomes active only
after its descriptor receipt is durable.

## Secrets and Docker

Secret references, never tokens, are stored in Host SQLite. Windows uses
Credential Manager. Docker uses AES-256-GCM authenticated records under
`/data/secrets` and a separately mounted 32-byte key from
`MEGLE_SECRET_KEY_FILE`; CAS generations protect refresh-token rotation.
Missing or unsafe key configuration degrades only the Provider.

Docker executes bundled Providers only from read-only `/opt/megle/plugins`.
Writable state is `/data/plugin-state`; `/data` is never executable discovery.
OAuth uses a configured `MEGLE_PUBLIC_URL`, a 256-bit one-time flow ID and a
ten-minute TTL. HTTP is accepted only for loopback development.
