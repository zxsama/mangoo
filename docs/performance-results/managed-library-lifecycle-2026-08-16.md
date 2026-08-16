# Managed Library lifecycle baseline — 2026-08-16

This is the AC15 no-sync baseline for the first Host-owned managed-library implementation. It measures the real Core HTTP boundary with two local-only Libraries and 100 indexed PNG fixtures.

## Reproduce

Run from the repository root on Windows:

```powershell
node tools/bench/managed-library-lifecycle-bench.mjs
```

The machine-readable result is written to the ignored path `.tmp/bench/managed-library-lifecycle.json`. Override it with `MEGLE_LIBRARY_BENCH_OUTPUT`; set `MEGLE_LIBRARY_BENCH_KEEP=1` to retain the temporary Host and Library files for diagnosis.

## Result

Environment: Windows x64, Node 22.14.0, debug Core built from the working tree, 100 one-pixel PNG fixtures, 5-second idle sample.

| Scenario | Samples | Result |
| --- | ---: | ---: |
| First preflight + create + activate | 1 | 128.30 ms |
| Library activation | 20 | p50 118.08 ms; p95/max 122.30 ms |
| Hot `GET /media?limit=96` | 100 | p50 1.65 ms; p95 2.55 ms; max 2.91 ms |
| Idle Core CPU | 5 seconds | 0% |
| Idle Core private memory | final sample | 7,467,008 bytes (7.12 MiB) |

These numbers are a regression reference, not a release budget. Release comparisons should use the same command, build profile, fixture count, and idle window and should retain the JSON result with the test evidence.
