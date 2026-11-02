# All-file synchronization and provider boundaries

## Approved scope

Enrolled sync folders collect ordinary files regardless of whether Mangoo has a media decoder for them. This includes archives, source files, unknown extensions, extensionless files and ordinary hidden files. The 2026-09-24 all-file browsing extension supersedes the earlier media-only presentation restriction: ordinary local roots also collect files, and valid files participate in browsing, search and file counts. Media metadata remains optional; thumbnail work requires a built-in or enabled plugin decoder. See [all-file library presentation](2026-09-24-all-file-library-presentation.md).

Exclude only control paths whose marker, journal or materialization-job evidence establishes Mangoo ownership. A reserved-looking name alone is insufficient, including a user directory named `.mangoo-staging`. User files do not become internal files merely because their names begin with a dot or end in `.db` or `.tmp`. Do not follow symbolic links or reparse entries outside the enrolled folder. Preserve supported relative path components and distinct hardlink directory entries; report an unsupported path or recoverable I/O/capacity failure explicitly.

The encryption requirement is transport protection with HTTPS/TLS and ordinary certificate verification. Existing saved authorization reconnects automatically. There is no new Mangoo password, recovery code, content key or encrypted cloud-object format. Initial authorization on an independent Host is still required when that Host has no saved account authorization.

## Ownership

Core owns Library and root identity, local file observation, sync events, checkpoints, transactions, queues, integrity validation and materialization. A provider plugin owns its service authentication, remote API protocol and opaque object transport. OneDrive-specific service details remain in the OneDrive provider and its trusted manifest contribution.

Shared Core/Desktop/Web code exposes bounded typed plugin configuration, authentication and presentation. Configuration values are non-secret, revisioned and subject to impact confirmation. Trusted declarations provide field formats, labels, help links and supported authentication modes. They do not execute renderer code. Existing Client ID, accounts, credentials, grants and enabled intent survive migration.

Browser actions resolve a plugin ID and pending-flow/help-link ID through authenticated Core APIs. Core checks exact flow ownership, current runtime and expiry. Desktop validates and opens the returned link. The renderer cannot provide an arbitrary URL. Manual authentication completion uses the redirect pinned to the pending flow.

## Disabled plugin behavior

Disabling a plugin revokes new work admission, advances its persisted lifecycle epoch, cancels and joins all owned work and stops the child process. Final disabled acknowledgement follows completion of that boundary. Ownership includes account discovery without a Library binding, metadata, reconciliation, queued local effects and binary streams that outlive their control response.

Old handles, responses and queued effects remain fenced after reenable. A remote immutable write accepted before cancellation can still exist; resume reconciles that same immutable identity rather than claiming rollback. Preserve durable work, account credentials, root identities and the original sync phase. Reenable resumes plugin-suspended work only; independent user pause and error states remain intact. Local browsing and local capture continue to work while the provider is unavailable.

Within the same remote scope and binding, completed immutable publication receipts remain usable after a transport restart. Upload reuse and publication-frontier advancement must agree about historical receipt generations while retaining the current caller generation fence. An effective plugin configuration change or changed remote account identity invalidates prior-scope publication evidence atomically; it must not make old receipts appear newly verified. This includes remote descriptor evidence, derived publication watermarks and uncommitted plans that relied on them. Cancelled authorization, unchanged configuration and ordinary same-scope restart preserve valid evidence. Local content, canonical history and completed historical operations remain intact.

## Compatibility and recovery

Catalog migration 0034 records all-file collection/backfill policy and file identity independently of media presentation. Existing enrolled roots need a bounded new scan/seed even if their old media-only scan was already complete. Capture and incremental receive both use the optional media model; a new received ordinary file must create its local projection.

Catalog migration 0035 records the physical identities of a materialization job's private staging directory and partial file. Creation, ownership registration and final promotion retain the corresponding handles. A pending path that cannot yet be proven owned is retried, never silently excluded or published as user content; a preexisting colliding path remains untouched. The collector excludes only the recorded partial, so user files added inside the working directory remain eligible. Staging cleanup removes only owned partials and empty containers.

New events use reducer version 3, while the new reader accepts versions 2 and 3. Existing version 2 events, seed identities and hashes remain immutable. Checkpoint schema remains version 5; the reducer-version field fences old readers. Compatibility checks must cover actual old serialized data, mixed histories, checkpoint restore and database reopen.

Restoring a legacy seed-only root can create a local bootstrap location for navigation. That event-less local location must not alter the seed's portable root truth, logical revision or fingerprint. Canonical root identity and location events retain their normal projection rules; bootstrap preservation does not invent missing canonical history or repair an already-divergent remote snapshot.

Version 3 preserves legal file and folder name text through 255 UTF-16 units, including decomposed Unicode. Version 2 keeps its existing normalization and length rules so old history does not change. Invalid Windows names still follow deterministic legalization; nonrepresentable paths produce an explicit limitation. Unchanged-source hash reuse checks high-resolution modification/change time, size and identity, so a same-size rewrite with restored modification time is not mistaken for unchanged content.

Large-file work reserves a bounded working window and checks actual destination/cache capacity. An identical existing destination can be adopted without reserving space for a second copy. Failure releases leases and retains a retryable durable state. Completion must validate current plugin/root ownership and the full expected file hash before promotion.

## Acceptance boundaries

The existing two source roots provisionally contain 52 ordinary files, including four formats omitted by the previous media-only capture. Real acceptance rehashes a stable source baseline, publishes into the existing Library namespace and verifies remote inventory and receipts.

A fresh download requires a separate Host with absent Catalog, range cache and destination files. The selected destinations are empty sibling folders; the original Host, source files and bindings remain intact. Same-Host duplicate Library restore is rejected before probes or plans, repeated at the atomic claim boundary, while exact successful-operation replay stays idempotent. Compensation may affect only state created by that operation.

Restore cleanup checks creation-journal, marker and binding identities. It uses nonrecursive empty-directory removal so content arriving after an early empty check survives. Historical restore plans without sufficient recorded ownership refuse cleanup and retain local data.

Acceptance distinguishes deterministic process/TLS fixtures, packaged UI/API behavior and actual OneDrive operations. Fresh absence, successful production receives, newly verified range receipts and independent output hashes establish fresh download. Verified logical bytes are not exact HTTP-body bytes or TLS wire bytes. Live in-flight disable and natural token-expiry refresh are claimed only when observed. Finish with original source integrity and at least five minutes of stable converged state; record any unavailable independent-Host authorization or platform coverage explicitly.

This document defines the approved behavior. Test and real-account completion are reported separately with fresh evidence.
