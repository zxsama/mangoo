# Megle Sync Protocol v1

Megle keeps browsing and search on each Library's local indexed SQLite. The
database file and WAL are never transport objects. Providers move immutable
event segments, checkpoints, and (in later phases) media objects.

## Identity and streams

- Catalog integer keys remain local query keys. Roots, folders, assets, tags,
  revisions, events, devices, streams, actor epochs, binding epochs,
  checkpoints, and conflicts use stable UUIDs.
- Host Library name/icon and Catalog content have independent streams and
  sequences because they live in separate SQLite databases.
- Actor sequence is allocated only when an event is remotely releasable.
  Pending content hashing never creates a sequence gap.

## Durability and capture

User-authored mutations use `DbWriter::write_authoritative`, which switches the
sole write connection to SQLite `synchronous=FULL` outside the transaction and
restores `NORMAL` afterward. Projection/cache work remains derived.

`local_api` and verified `external_fs` origins may publish. Remote apply,
bootstrap, recovery, scan, and media materialization never create outbox rows.
The local mutation, event ledger row, and outbox row share one short
transaction. Content changes first commit a revision plus publication intent;
the manifest-binding transaction later creates the canonical event.

## Merge rules

- scalar fields: LWW by HLC, device, actor epoch, actor sequence, event UUID;
- tag membership: observed-remove set;
- location and content: multi-value revisions with explicit supersedes;
- deletion: tombstone tokens, with restore removing only observed tokens;
- invalid parents, cycles, and duplicate sibling names: deterministic local
  legalization that does not emit events.

Folder deletion freezes the observed stream vector and local mutation
generation, then expands the observed subtree in resumable keyset batches.
Objects created concurrently or moved out before capture are not hidden.

## Intake, compatibility, and checkpoints

Segments contain one actor's contiguous range plus predecessor and payload
hashes. Remote data is verified before apply. Each FULL writer turn is limited
to 100 events and 1 MiB decoded bytes. Exact duplicates are no-ops; gaps and
forks block only their actor. Unknown wire/envelope/reducer versions put the
Library in a read-only compatibility lock without advancing a frontier.

Checkpoints retain CRDT truth, conflicts, tombstones, unfinished bulk work, and
independent Host/Catalog vectors. Selection uses maximal vector coverage, not
creation time. V1 performs no automatic event, conflict, tombstone, or segment
garbage collection.

The production Host polls synchronized Libraries with a provider-neutral
coordinator. Active Libraries submit one bounded authoritative turn to their
existing writer. Inactive Libraries acquire the same per-Library runtime lease
and open a 4 MiB cache/16 MiB mmap dormant writer only for that turn. A turn
reconciles durable `sync-spool` objects, applies one bounded event batch,
advances one persisted subtree-delete batch, and builds or recovers checkpoints.

Watcher and full rescan persist the OS volume/file identity. A stable watcher
rename may publish one external location event; bootstrap/rescan updates the
local identity map without echo. Ambiguous fingerprint fallback records a
local conflict instead of guessing or propagating a delete.

New-device restore replays the checkpoint event stream into an isolated staging
Catalog through the production decoder/reducer, validates its digest and
frontiers, checkpoints the staging database, then promotes only if the target
Catalog does not already exist. Run `npm run bench:sync-protocol` for the
production-path capture, browse, and eight-dormant-Library resource/fairness
gate.
