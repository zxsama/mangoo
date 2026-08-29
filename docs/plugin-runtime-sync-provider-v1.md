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

## OneDrive public-client authorization

Each user or deployment operator supplies a self-managed Microsoft Entra
public-client registration for Personal and/or Business accounts with delegated
`Files.ReadWrite.AppFolder` and `offline_access`. Release artifacts embed no
publisher-owned Client ID. `MEGLE_ONEDRIVE_CLIENT_ID` is an optional non-secret,
read-only deployment value; otherwise the five-step setup wizard stores a
revisioned Host override. The public projection reports only
source/configured/read-only/revision state, never the effective ID. Missing
configuration safely keeps OneDrive `configuration-required`. No mode uses or
accepts a client secret.

Desktop uses Authorization Code + PKCE S256 with a Provider-owned ephemeral
IPv4 `127.0.0.1` listener while advertising
`http://localhost:<port>/plugin-auth/callback`. The Entra public-client
registration uses **Mobile and desktop applications** with
`http://localhost/plugin-auth/callback`; localhost port matching is ignored but
the path must match. `auth.poll` observes
the pending flow without consuming it; only a callback terminal result,
expiry, explicit `auth.cancel`, or runtime-session fencing ends the flow.
Electron opens only a narrowly validated Microsoft authorize URL through the
typed preload bridge.

Docker sets `MEGLE_ONEDRIVE_AUTH_MODE=manual-response-uri`. `auth.begin` uses
`https://login.microsoftonline.com/common/oauth2/nativeclient`, and the browser
UI displays the authorization URL plus a transient field for the complete final
response URI, matching the `abraunegg/onedrive` workflow. Core bounds and
strictly parses the URI, removes inert `session_state`, and sends only an
allowlisted field map to the Provider. The URI is cleared after submit/cancel
and never enters RPC, SQLite, logs, errors, diagnostics, or query cache. Docker
does not require `MEGLE_PUBLIC_URL`, an inbound callback, or a confidential
registration.

OAuth tokens and refresh tokens remain behind the Secret Broker: Credential
Manager on Windows, or authenticated encrypted files under `/data/secrets`
using the independently mounted `MEGLE_SECRET_KEY_FILE` in Docker. Client IDs
are configuration; authorization codes, state, response URIs, tokens, and
preauthenticated Graph URLs are sensitive material.
