# Refreshing immutable Library discovery snapshots

## Observed product gap

The initial `library-descriptor` advertises an empty seed. Later enrollment and media publication leave the binding active, but `push_protocol_objects` only advertises roots/counts for seed-ready/publishing bindings, and checkpoint selection reuses the old portable checkpoint indefinitely. Upload receipts can advance while another device still discovers zero roots and zero media. A changing body cannot be written to the fixed immutable key.

This work is required to finish the existing two-folder OneDrive task. It extends publication/discovery consistently; it must not overwrite the seed, forge a remote cache row, change source media, or use a canary namespace.

## Immutable snapshots and ordering

- Preserve the existing fixed `library-descriptor` as a legacy seed. New discovery snapshots use a reserved content-addressed key, `descriptors/<sha256>.json`, under the same Library namespace.
- Reuse the existing descriptor identity/root/seed/dependency fields and add explicit causal snapshot evidence. The snapshot's Library UUID, checkpoint UUID/hash, roots/counts and Catalog/Host frontiers must come from one verified portable checkpoint; current mutable Catalog rows are not mixed with an older seed.
- Use the checkpoint's stable Catalog and Host actor vectors to compare candidates. A candidate can replace a cached current view only when its combined evidence dominates that view. Equal causal evidence with different advertised truth and concurrent incomparable heads require an explicit incompatible/conflict result; never select by arrival order, clock time, device-local generation or filename sort.
- Legacy descriptors remain accepted. A new snapshot can supersede a legacy seed only after verifying the seed/checkpoint relationship and causal dominance. Do not treat an empty vector as permission to replace unrelated Library/epoch truth.
- Keep all snapshot content deterministic for its checkpoint. Poll time must not create a new hash. Use the persisted checkpoint timestamp for display if needed; timestamps are not ordering authority.
- Portable semantic truth and media/root totals come from canonical content/location/root events, not a device's selected browse rows, local timestamps or materialization state. A receiver selecting zero or one local root must retain full portable truth and must not publish a smaller Library merely because fewer roots exist locally.
- Numeric vector coverage alone is insufficient. Validate shared event identities/history and state replay before accepting dominance; Catalog prefix rules and Library-filtered Host actor sequences are distinct. Old enrolled roots and media missing canonical identity/location capture are backfilled idempotently through the ordinary authoritative writer, preserving root/sentinel identity and nested parent chains.
- An old browse root sentinel can have an empty-name bootstrap location with no event. For a sentinel matched to a root descriptor, projection interprets that exact placeholder using the existing descriptor's Library-root parent and requested name, retaining the tree anchor and stored revision. Never normalize an empty canonical event or detach the sentinel's ordinary descendants.
- Capturing an equivalent canonical location must not leave an identical NULL-event bootstrap head as a second conflict branch. Retirement remains effective after that event-backed canonical revision is superseded by a later rename. Head rebuilding, bounded old-conflict repair and reopen initialization use the same exact-location equivalence rule without changing immutable history; never-matched differing bootstrap locations and independent canonical heads remain conflicts.
- Canonical identity backfill can change a root's materialization fingerprint without changing its name or physical path. A fenced directory/marker identity check permits metadata-only adoption and recovery from a false rename-required state. This path never moves a directory, rewrites its marker, or changes binding generation; real target changes keep the relocation flow.

## Publication and recovery

1. Check whether a ready checkpoint covers current authoritative Catalog/Host state. Capture a new checkpoint only after a meaningful change and when its publication prerequisites can become portable. Reuse a matching persisted checkpoint on retry/restart.
2. Flush/seal and publish the checkpoint, media-index shards and every required dependency through the existing immutable Provider boundary, recording exact receipts under the current binding epoch and transport generation.
3. Publish the deterministic descriptor snapshot last. The initial legacy seed remains immutable. Replayed upload uses the same bytes/key, and stable idle polls issue no new snapshot writes.
4. Discovery reads actual remote objects and validates namespace/key/hash/content, checkpoint identity, frontiers and dependency availability before advancing inventory. No local-cache shortcut substitutes for remote publication.

## Discovery, transport and compatibility

Per-Library delta already transports arbitrary object keys, but Core must explicitly admit and validate the reserved snapshot namespace. Account-root discovery must also expose exact candidate keys so an unbound Library can discover its latest validated state. OneDrive remains opaque transport: it enumerates descriptor candidates and verifies object integrity; causal selection belongs to Core. Pagination/deduplication/cursor commits and account-generation fences must remain bounded and durable.

Core and Host must retain the exact chosen descriptor object identity for later restore/bootstrap rather than silently reading the fixed seed. Extend existing persistence with a numbered migration if ordering/identity cannot be stored safely in current fields. Fixed-key legacy discovery remains supported. Old clients can still read the old seed; new snapshots require an updated client to display current inventory.

Host migration 18 records descriptor heads, verified candidates and exact descriptor/checkpoint keys. Existing legacy restore plans may have NULL checkpoint pins after migration; resolve those through the originally selected descriptor key and persisted hash under the same account/binding fences. Never fill missing pins by choosing a newer maximal checkpoint from the spool.

Staging restoration replays the canonical event history before creating selected browse roots and installing the final folder/file projection. Installing final locations first makes historical events collide with another entity's future location, so restore can fail depending on actor order even when the final legal names are unique. Use the existing shared event decoder/reducer, reconcile tag identities created by replay, and keep both genesis-replay and final browse-projection checks. Cover both fixed historical actor orders and none/some/all root selection.

The complete staging history is replayed in an isolated transaction with foreign keys checked at commit. This allows a valid superseded revision to appear later in cross-actor replay order while still rejecting any unresolved reference before a staging Catalog can be used. Keep shared event validation and actor/batch checks; do not defer constraints on the ordinary live apply path.

Root-preflight snapshots encode nonempty `catalogVector` values through `checkpoint::stream_vector_serde::serialize` and the JSON value serializer. The existing actor/sequence array representation is required because `ActorId` is a composite map key; direct `serde_json::to_value` on the vector only appears to work for an empty frontier.

OneDrive's exact SHA-256 HEAD verification reads object content. Repeated/new descriptor candidates must not cause unchanged large media to be repeatedly downloaded. `GraphClient::head` retains bounded process-local verified hashes keyed by authenticated scope, namespace/key, item ID, size and strong eTag; each lookup still reads current metadata. First verification hashes the full body and re-reads metadata before caching; missing/weak eTag or changed identity does not reuse proof. Core independently caches validated candidates by exact identity and account generation. Both layers must have byte/read-count regressions, not just zero-new-put assertions. Graph eTag covers the complete item, including content; see [driveItem resource properties](https://learn.microsoft.com/en-us/graph/api/resources/driveitem?view=graph-rest-1.0).

## Product status

The production scheduler must refresh publication metrics from the actual independently budgeted turns. Pending hashing/capture work must count before remote objects exist, without double-counting a job's later object transfers. An error/no-work poll cannot stand in for completed publication. Keep normal retry recovery separate from ordinary worker wake.

## Required evidence

- Production-path regression: enable empty Library, enroll two roots, publish media, then discover both roots and matching counts from a real Provider fixture.
- Snapshot determinism, duplicate/reordered/older candidates, same-frontier mismatch, concurrent incomparable snapshots, legacy seed compatibility and exact chosen-key restore.
- Deterministic restore ordering with conflicting historical/final folder and file names, tag/metadata identity preservation, and successful root preflight with a nonempty Catalog vector.
- Restart between dependency receipts and snapshot receipt; no stale epoch/account update and no immutable overwrite.
- Idle second turn creates no checkpoint/snapshot/upload; normal scheduler exposes pre-object pending work and settles only when actual work settles.
- Full repository gate, Provider emulator/conformance, packaged OneDrive run on the existing namespace, remote root/count/receipt confirmation and unchanged 48-file content digest.

Implementation should use existing checkpoint vector/verification and Provider abstractions. If current canonical mutation capture cannot supply comparable causal evidence for root changes, fix or report that prerequisite instead of introducing an unsafe ordering approximation.
