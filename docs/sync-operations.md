# Sync Operations Runbook

Megle synchronization moves immutable events, checkpoints, manifests and media
objects. It does not copy a live SQLite database or its WAL, and it is not a
backup replacement. Keep an independent backup of source media and exported
metadata.

## Release gates

Run these from a clean commit:

```powershell
npm run check:sync-integration
npm run check:desktop-os-backdrop
node tools/bench/renderer-sync-performance.mjs --output .tmp/sync-release/renderer-evidence.json
node tools/bench/sync-integration-bench.mjs --renderer-evidence .tmp/sync-release/renderer-evidence.json
node tools/checks/sync-release.mjs --renderer-evidence .tmp/sync-release/renderer-evidence.json
```

Raw data is written to the ignored `.tmp/sync-release/<run-id>/` directory.
`check:sync-release` archives `HEAD`, hashes the archive, extracts it into an
ordinary directory and runs the gates there. A dirty diagnostic run can be
requested with `--allow-dirty`, but it cannot satisfy the immutable-tree row.
Docker/Linux and Personal/Business Graph canaries are required independent
rows. Docker qualification includes two separate containers sharing only the
evidence volume and proving Host, active Library, capture, and Provider object
persistence. Live Graph qualification covers Delta, immutable object, Range,
and upload-session behavior. An unavailable daemon or missing token keeps the
release result nonzero.

## Enable and rollout

Promote one stage at a time:

1. Deploy schema with synchronization dormant.
2. Enable local shadow capture and compare event/frontier counts after reopen.
3. Enable the bundled fake Provider and perform a fresh-root restore.
4. Enable OneDrive for one device and compare vector, projection and content hashes.
5. Restore a fresh second device and wait for catalog-ready plus complete media.
6. Enable writes on both devices and verify deterministic conflict convergence.
7. Enable multiple Libraries only after fairness and browse/search/thumbnail gates pass.

Every stage requires a fresh-root restore. Stop promotion on a digest mismatch,
repeating retry loop, unexplained conflict, performance regression or missing
platform evidence.

## Pause, retry and diagnostics

- Pause stops new transport turns. Local edits remain durable and queued.
- Resume or Sync now wakes the scheduler; it does not discard cursors or receipts.
- Retry is appropriate for transient network, throttle and Provider failures.
- `authorization-required` requires a new OAuth authorization. Do not repeatedly retry it.
- Export diagnostics from Library Sync settings. Exports redact bearer tokens,
  upload URLs and secret material; keep the raw release directory private.
- Compare binding epoch and transport generation before acting on old logs.

## Unbind and reconnect

Unbind uses a plan/commit sequence. It fences the active generation, reconciles
observable remote receipts, and then returns the Library to `local-only`.
Remote immutable objects and other devices are never deleted. A reconnect is
an explicit three-way decision and creates a new binding epoch; old-epoch work
must not be replayed automatically.

Before account sign-out or account switching, pause affected Libraries. An
empty AppFolder is not proof that remote content was deleted. Rebind only after
the account and remote Library identity are confirmed.

## Restore

Restore preflight validates the selected managed root before writing. Before
`catalog-ready`, cancellation may discard the staging Catalog. After
`catalog-ready`, the local Library exists and cancellation becomes pause/unbind.
Original media is automatically queued; thumbnails, previews and FTS are local,
regenerable data and are never downloaded as sync truth.

## Rollback

Rollback is forward-disable only:

1. Pause the affected Library or globally disable scheduling.
2. Preserve schema, UUIDs, outbox, inbox, conflicts, publication intents,
   checkpoints, local media and remote immutable objects.
3. Export redacted diagnostics and record the last verified vector.
4. Fix or replace the Provider, then retry or explicitly reconnect.

Never downgrade the schema, delete remote objects as rollback, copy an active
SQLite/WAL pair, or clear durable receipts to force a resend.
