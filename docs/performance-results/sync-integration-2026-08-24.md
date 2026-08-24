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
1,000,000 chunks and a physical 10 GiB transfer. Raw databases and logs are
ignored under `.tmp/sync-release/<run-id>/`; the release runner places the large
byte corpus in a capacity-qualified system temporary directory and removes it
after preserving the parsed counters in the benchmark result.

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

The clean immutable Windows run for commit
`89a67e807a79eb6d7edc6f5367d4c9fe2cb2759e` physically generated, uploaded,
range-downloaded, materialized and SHA-256-read 10,737,418,240 bytes through
`MediaPublicationWorker` and `MediaRestoreWorker` over framed Provider RPC.
Both transfer children were hard-killed after durable halfway markers and
restarted from their recorded offset/range inventory. Upload resumed at byte
5,375,172,642 and download at byte 5,373,152,353. The run transferred
10,737,786,159 upload bytes (including the immutable manifest) and exactly
10,737,418,240 data-range bytes, used 870 verified ranges, limited the Provider
buffer to 16 MiB, and peaked at 185,757,696 bytes of worker RSS. Source and
restored SHA-256 were both
`c7c26272defe8457bcdb66d25f9d20b090b5b5bc10713784e9234a2c90a68c47`.

## Release state

The full immutable 1,000,000-asset Windows sample for commit `89a67e8` passed
all measured Core gates. Capture p95/p99 was 2.689/2.935 ms, apply p95/p99 was
3.419/3.566 ms, stress CPU was 9.324%, stress private memory was 44.867 MiB,
idle CPU was 0%, and the eight dormant Library descriptors added 1.352 MiB.
Browse/search/thumbnail remained local with zero Provider calls; their absolute
p95/p99 values were 0.134/0.152 ms, 37.047/37.986 ms and 0.013/0.016 ms
respectively. Relative
regression calculations use a 1 ms floor for sub-millisecond measurements,
while the unchanged 50/100 ms absolute browse ceilings continue to fail closed.
The exact-tree renderer collector passed with 80 production sync requests, zero
request failures, zero synchronous renderer IPC, a 0.097 percentage-point
long-frame increase and a 0.388 percentage-point dropped-frame increase. The
exact-tree full gate, sync integration, sync benchmark and native OS-backdrop
rows also passed.

The release run remains fail-closed only because this machine has no usable
Docker daemon and no OneDrive Personal or Business live canary credentials.
Those three required external rows are recorded as blocked and keep the release
result nonzero; all immutable Windows exact-tree rows for commit `89a67e8`
passed.
