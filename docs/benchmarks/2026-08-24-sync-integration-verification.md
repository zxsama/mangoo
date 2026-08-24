# Sync integration and release verification — 2026-08-24

## Scope

The final verification layer composes the production Library lifecycle,
protocol reducer, checkpoint/media restore, Core fault points, real Provider
children, OneDrive Graph emulator and orchestration state tests behind three
public commands:

```powershell
npm run check:sync-integration
npm run bench:sync-integration
npm run check:sync-release
```

Machine results, logs and large samples stay under
`.tmp/sync-release/<run-id>/`. Each command records environment facts, duration,
exit status and per-row log paths. The release command fails closed if its
immutable Windows, Docker/Linux, Personal Graph or Business Graph row is absent.

## Evidence topology

| Evidence | Production boundary | Command |
| --- | --- | --- |
| Empty Host bootstrap/reopen | `LibraryManager`, Host Registry, managed marker, Catalog migrations and sync identity | `npm run check:sync-integration` |
| Two-device convergence | three isolated Hosts/Catalogs plus one verified framed fake Provider child; two no-Catalog restores, independent offline edits, and repeated-delivery idempotence | `npm run check:sync-integration` |
| Conflicts, duplicates and ordering | sealed protocol events and reducer/inbox rules | `npm run check:sync-integration` |
| Fresh restore | checkpoint builder, external media indexes and staging Catalog | `npm run check:sync-integration` |
| Provider process | framed fake Provider and OneDrive child against Graph emulator | `npm run check:sync-integration` |
| Crash matrix | real child termination at Core durability boundaries | `node tools/checks/sync-fault-matrix.mjs` |
| Scale and hot paths | production coordinator, Catalog queries, scheduler, media inventory | `npm run bench:sync-integration` |
| Renderer frames | real Electron renderer, production sync endpoints, long/dropped frames and synchronous IPC scan | `npm run bench:renderer-sync` |
| Physical transfer | deterministic 10 GiB files through media workers and framed Provider RPC, upload/download child kills, durable offsets/ranges and full SHA-256 | `npm run bench:sync-integration` |
| Windows acrylic | committed native-capture OS-backdrop harness | `npm run check:desktop-os-backdrop` |
| Immutable release | `git archive` extraction plus Windows/Docker/canary rows | `npm run check:sync-release` |

The physical transfer probe launches the production media workers in killed and
resumed child processes, with all upload and range download bytes crossing the
framed fake-Provider RPC boundary. Renderer frame evidence is an independent
required row; the committed collector binds it to commit, machine and run ID
and does not substitute Core query timings.

## Immutable Windows exact-tree evidence

The release run at
`.tmp/sync-release/2026-08-24T21-18-58-855Z-release-19fd259f/` was produced from
the clean `git archive` of commit
`89a67e807a79eb6d7edc6f5367d4c9fe2cb2759e`. Every in-repository Windows row
passed: archive/extraction, `npm ci`, legacy selector/adoption absence, full
`npm test`, sync integration, renderer performance, the full sync benchmark and
the native-capture OS-backdrop harness.

- The framed Provider two-device/third-device restore, 100,000
  shuffled/duplicated events, Core crash points, fake Provider child and
  OneDrive emulator child passed.
- The physical 10 GiB production-worker/Provider-RPC probe resumed upload at
  byte 5,375,172,642 and download at byte 5,373,152,353. Upload wire bytes
  equalled the sealed immutable object bytes (10,737,786,159), range download
  bytes equalled the original 10,737,418,240 bytes, 870 durable ranges completed,
  source/restored SHA-256 matched
  (`c7c26272defe8457bcdb66d25f9d20b090b5b5bc10713784e9234a2c90a68c47`),
  peak Provider buffering was 16 MiB, and peak worker RSS was 185,757,696 bytes.
- The paired million-asset benchmark passed: capture p95/p99 2.689/2.935 ms,
  apply p95/p99 3.419/3.566 ms, stress CPU 9.324%, stress private memory
  44.867 MiB, idle CPU 0%, dormant-library memory 1.352 MiB, and zero Provider
  calls from browse/search/thumbnail.
- The exact-tree renderer collector passed with 80 production sync requests,
  zero request failures, zero synchronous input-path IPC, a 0.097 percentage-
  point long-frame increase and a 0.388 percentage-point dropped-frame
  increase. The exact-tree OS-backdrop native capture also passed.

## External release blockers

This workstation currently has no usable Docker daemon and no Personal or
Business OneDrive live canary credentials. `check:sync-release` records each as
a required blocked row and exits nonzero. Emulator results never satisfy a live
Graph row, and Windows never substitutes for Docker/Linux.

The deterministic fault manifest now includes production-path permission loss,
reordered Delta pages, media-worker disk-full recovery, and short Range
validation. Missing Docker or live Graph credentials remain external blocked
rows rather than being inferred from emulator results.

## Parent AC1–AC20 manifest

| AC | Behavior | Owner | Evidence command | Required platform | Current disposition |
| --- | --- | --- | --- | --- | --- |
| AC1–AC3 | Offline edits, deterministic conflicts, duplicate/out-of-order idempotence | Core protocol | `npm run check:sync-integration` | Windows + Linux | immutable Windows exact-tree passed; Docker/Linux blocked |
| AC4–AC5 | Originals/events/checkpoints only; automatic verified media restore | media worker + restore | integration + benchmark | Windows + Linux | immutable Windows integration and physical 10 GiB rows passed; Docker/Linux blocked |
| AC6 | browse/search/thumbnail remain local under sync load | coordinator + DB | `npm run bench:sync-integration` | Windows + Linux | immutable Windows paired Core and renderer probes passed; Docker/Linux blocked |
| AC7 | throttle, network, token, cursor and resumable upload recovery | OneDrive Provider | fault matrix + canaries | emulator + Personal + Business | emulator covered; both live rows blocked without credentials |
| AC8 | Provider crash/hang/malformed process isolation | Provider runtime | fault matrix | Windows + Linux | immutable Windows exact-tree passed; Docker/Linux blocked |
| AC9 | per-Library enable/pause/unbind isolation | orchestration + coordinator | integration + web gate | Windows + Linux | immutable Windows exact-tree passed; Docker/Linux blocked |
| AC10 | aggregate protocol/RPC/schema/stress/fault gate | release harness | all three public commands | Windows + Linux | all immutable Windows rows passed; external rows remain fail-closed |
| AC11 | resumable, deduplicated and corruption-safe large objects | media + Provider | benchmark + fault matrix | Windows + Linux | immutable production media-worker/Provider-RPC 10 GiB Windows row passed; Docker/Linux blocked |
| AC12 | fresh checkpoint/event/media restore and local cache rebuild | persistence + media | integration + visual harness | Windows + Linux | immutable Windows third-device restore/cache-regeneration evidence passed; Docker/Linux blocked |
| AC13 | managed-root validation and no legacy roots | Library lifecycle | integration | Windows + Linux | immutable Windows bootstrap/reopen and legacy-absence rows passed; Docker/Linux blocked |
| AC14 | discovery, staged restore and cancel boundaries | orchestration UI/Core | web tests + OS-backdrop harness | Windows | immutable Windows full gate and OS-backdrop row passed |
| AC15 | multi-Library fairness and foreground latency | coordinator | integration + benchmark | Windows + Linux | immutable Windows eight-Library production probe passed; Docker/Linux blocked |
| AC16 | offline publication intent and dependency-before-event | media publication | integration + fault matrix | Windows + Linux | immutable Windows exact-tree passed; Docker/Linux blocked |
| AC17 | hidden-until-complete first enable and resumable failure | orchestration + Provider | integration + fault matrix | Windows + Linux | immutable Windows emulator/state-machine evidence passed; Docker/Linux blocked |
| AC18 | account switch/rebind and unsafe root rejection | lifecycle + OneDrive | integration + canaries | Windows + Graph | live account rows blocked without credentials |
| AC19 | stable external changes, missing-only delete and echo suppression | watcher + identity | full gate | Windows + Linux | immutable Windows full gate passed; Docker/Linux blocked |
| AC20 | generation-fenced unbind and explicit reconnect | orchestration + coordinator | integration + fault matrix | Windows + Linux | immutable Windows exact-tree passed; Docker/Linux blocked |

Commit `89a67e8` has complete immutable Windows exact-tree evidence. The overall
release remains blocked until the Docker/Linux row and both credential-gated
OneDrive canaries pass against this exact commit; no emulator or Windows row is
used as a substitute for those external gates.
