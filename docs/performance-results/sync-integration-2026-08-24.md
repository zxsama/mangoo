# Sync integration performance result — 2026-08-24

## Reproduce

```powershell
node tools/bench/renderer-sync-performance.mjs --output .tmp/sync-release/renderer-evidence.json
node tools/bench/sync-integration-bench.mjs --renderer-evidence .tmp/sync-release/renderer-evidence.json
```

The renderer row consumes JSON produced by the committed production desktop
collector (`npm run bench:renderer-sync`). When selecting the evidence path,
run `node tools/bench/renderer-sync-performance.mjs --output <file>`. Pass it through
`--renderer-evidence <file>` or `MEGLE_SYNC_RENDERER_EVIDENCE`. The file must
contain numeric `baselineLongFramePercent`, `stressLongFramePercent`,
`baselineDroppedFramePercent`, `stressDroppedFramePercent`, and
`inputSyncIpcCalls` fields. Omitting it is a required blocked row.

For a fast development diagnostic:

```powershell
node tools/bench/renderer-sync-performance.mjs --quick --output .tmp/sync-release/renderer-evidence.json
node tools/bench/sync-integration-bench.mjs --quick --renderer-evidence .tmp/sync-release/renderer-evidence.json
```

The release profile uses 1,000,000 assets, eight Libraries, 100,000 media jobs,
1,000,000 chunks and a physical 10 GiB transfer. Raw databases, byte corpora and
logs are ignored under `.tmp/sync-release/<run-id>/`.

## Required thresholds

| Metric | Gate |
| --- | ---: |
| Browse p95 / p99 | regression <=10% and <=50 ms / regression <=15% and <=100 ms |
| Search p95 / p99 | regression <=10% and <=150 ms / regression <=15% and <=300 ms |
| Ready thumbnail p95 | regression <=10%, Provider calls 0 |
| Capture p95 / p99 | <=5 ms / <=15 ms |
| Apply p95 / p99 | <=10 ms / <=25 ms |
| Apply batch | <=100 events and <=1 MiB decoded |
| Idle CPU / dormant private memory | <1% / <100 MiB |
| Stress CPU / private memory | <15% / <256 MiB |
| Renderer dropped/long-frame increase | <=2 percentage points; sync IPC on input path 0 |

The benchmark result is valid only when paired baseline/stress data was produced
on the same commit, fixture, machine, power policy and warm-cache policy. A row
with missing samples is a failure, not “not applicable”.

Capture latency is reported per edited field from bounded four-edit writer
turns. This measures durable SQLite/event capture amortized across the short
interactive bursts used by the production writer rather than unbounded bulk
ingest.

## Physical transfer result

The Windows development run physically generated, uploaded, range-downloaded,
materialized and SHA-256-read 10,737,418,240 bytes. Both transfer children were
hard-killed after a durable halfway marker and restarted from their recorded
offset/range inventory. Final counters were exactly one object length per
direction; 640 ranges of 16 MiB were verified; the working buffer was 1 MiB.
Source, provider and restored digests were all
`c7c26272defe8457bcdb66d25f9d20b090b5b5bc10713784e9234a2c90a68c47`.
The current implementation now routes this probe through the production
`MediaPublicationWorker` and `MediaRestoreWorker` over framed Provider RPC. A
fresh 2026-08-25 Windows run transferred 10,737,786,159 upload bytes (including
the immutable manifest) and exactly 10,737,418,240 data-range bytes, restored
all 10 GiB with the same digest, resumed upload from byte 5,375,172,642, used
870 verified ranges, limited the Provider buffer to 16 MiB, and peaked at
185,757,696 bytes of worker RSS.

## Release state

The latest full 1,000,000-asset Windows sample passed all measured Core gates.
Capture p95/p99 was 2.649/2.897 ms, apply p95/p99 was 3.398/3.523 ms, stress
CPU was 11.026%, stress private memory was 41.543 MiB, idle CPU was 0%, and the
eight dormant Library descriptors added 0.652 MiB. Browse/search/thumbnail
remained local with zero Provider calls; their absolute p95/p99 values were
0.134/0.156 ms, 36.163/39.402 ms and 0.014/0.017 ms respectively. Relative
regression calculations use a 1 ms floor for sub-millisecond measurements,
while the unchanged 50/100 ms absolute browse ceilings continue to fail closed.
The committed renderer collector's current quick diagnostic passed with zero
synchronous renderer IPC, 0.321 percentage-point long-frame increase and 1.577
percentage-point dropped-frame increase. Exact-tree release evidence remains a
separate required row.

This document records development evidence, not a release waiver. The exact-tree
Docker/Linux and live Personal/Business Graph rows are blocked on this machine;
therefore the release gate must remain nonzero until those rows run elsewhere on
the same commit.
