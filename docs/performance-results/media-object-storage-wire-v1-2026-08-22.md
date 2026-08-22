# Media object storage wire v1 benchmark

Date: 2026-08-22

Command:

```powershell
npm run bench:media-objects -- --output .tmp/media-object-storage-check/final-report.json
```

The benchmark reports only aggregate media kinds and a corpus fingerprint. It
does not copy input media or include source paths or file names in its output.
The final self-contained run contained two deterministic 24 MiB inputs plus
format-shaped JPEG, PNG, WebP, DNG/RAW, PSD, and MP4 fixtures, totalling
51,904,512 bytes. Aggregate corpus SHA-256:
`c2a1d006a0547cd9ea2e2b1e4981cce9536f9f7684d9b1e0e23a7bb86acb566b`.

| Profile | Chunks | Manifest estimate | Localized-edit reuse | Reference throughput |
| --- | ---: | ---: | ---: | ---: |
| fixed 8 MiB | 12 | 2,304 B | 51.75% | 283.34 MiB/s |
| CDC 2/8/16 MiB | 11 | 2,112 B | 49.65% | 1.16 MiB/s |

The final verification run peaked at `268,353,536` RSS bytes (below 256 MiB).
Fixed chunking consumed `172 ms` CPU (`0.985` cores); the intentionally simple CDC
reference consumed `42,063 ms` CPU (`0.985` cores). These numbers characterize the benchmark
implementation, not the Rust worker's sustained CPU budget.

The JavaScript benchmark deliberately favors reproducibility over production
throughput and is not the runtime implementation. Aggregate reuse is depressed
by the intentionally non-reusable transcode case and by very small JPEGs. CDC
restores stable boundaries after insertions (`24.29%` at the start and `30.74%`
in the middle), while fixed chunks retain `0%` after a start insertion. The
measured corpus does not justify CDC on aggregate reuse or throughput alone;
wire v1 nevertheless freezes CDC because insertion stability is a product
requirement and the Rust implementation uses bounded streaming rather than the
benchmark's BigInt-per-byte reference loop. The tradeoff is explicit and must
be revisited against the production performance probes before release.

Pack simulation produced two objects at 32 MiB and one at 64/96/128 MiB for
this corpus. A 64 MiB target retains bounded retry granularity while avoiding
an extra object for the representative corpus. Its measured reference
time-to-first-seal was `42.72 s` in the deliberately slow JavaScript
CDC implementation, with estimated peak temporary pack bytes capped at
`64 MiB`; runtime throughput is evaluated separately in Rust.

Each simulated pack size also records CPU time and two recovery amplification
bounds. Exact chunk-range reads have `1.0x` byte amplification; the reported
whole-object fallback value models independently requesting every chunk from a
Provider that ignores ranges. This makes the object-count/retry tradeoff
explicit without treating the fallback as the normal restore path. At the
selected 64 MiB target, that conservative fallback amplification was `11.0x`
for CDC on this corpus.

## Frozen v1 constants

- format version: `1`
- chunker: deterministic gear CDC v1
- small file: one chunk at `<= 2 MiB`
- CDC minimum/target/maximum: `2 / 8 / 16 MiB`
- data-pack target: `64 MiB`
- ordinary seal envelope: `32–96 MiB`
- manifest-pack target/cap: `4 / 8 MiB`
- file, chunk, pack, manifest and index identities: SHA-256

The chunking profile is embedded in each content manifest, so later defaults
can coexist without rechunking historical content. Provider upload fragment
sizes remain transport-local and do not affect these bytes or identities.
