# Persistent Thumbnail Cache Pressure Smoke (2026-06-02)

Dataset: `Y:\Repository\Billfish\Artists`

Harness:

- `tools/dev/artists-million-desktop-sweep.mjs`
- `tools/bench/thumbnail/persistent_cache_pressure_harness.mjs`

Run shape:

- 50 deterministic desktop operations per scenario
- real Electron desktop shell
- real Rust core
- partial-coverage mode enabled for iterative pressure smoke only

## Harness fixes found during Phase 5

The original pressure harness had three runtime issues that blocked real runs:

1. Electron DevTools was pinned to a Windows-excluded TCP range (`9251` and nearby ports), which caused `bind() access denied` before CDP could attach.
2. The bulk-active scenario fully enqueued the library before browsing started, which turned a foreground-latency test into a long preflight phase.
3. All scenarios reused one data directory, so reruns could fail with `EBUSY` while deleting a still-locked SQLite file.

The harness now:

- probes for a bindable debug port at runtime
- starts browsing immediately after triggering bulk generation for the bulk-active scenario
- uses scenario-scoped data directories, with hot-cache reusing the cold-cache data directory on purpose

## Scenario summary

### Cold cache

- Runtime: about 73s
- Persistent cache before run: `0` cached blobs
- Persistent cache after run: `105` cached blobs, `2.3 MB`
- Folder switch p95: `5178 ms`
- Visible thumbnail wait p95: `2413 ms`
- Preview open max: `248 ms`

### Bulk active

- Runtime: about 62s
- Persistent cache after run: `21` cached blobs, `0.5 MB`
- Folder switch p95: `3734 ms`
- Visible thumbnail wait p95: `793 ms`
- Preview open max: `245 ms`

Result:

- Bulk work did not starve foreground thumbnail delivery in this smoke.
- Foreground visible-thumbnail latency improved materially versus cold cache while bulk work was active.

### Hot cache

- Runtime: about 66s
- Persistent cache before run: `105` cached blobs, `2.3 MB`
- Persistent cache after run: `129` cached blobs, `2.8 MB`
- Folder switch p95: `3167 ms`
- Visible thumbnail wait p95: `1947 ms`
- Preview open max: `422 ms`

Result:

- Hot-cache revisit improved folder-switch latency versus cold cache.
- Hot-cache visible-thumbnail latency improved versus cold cache, but not as strongly as the bulk-active run in this 50-op smoke.

## Current tuning call

No core scheduling constants were changed from this smoke alone.

Reasoning:

- `THUMBNAIL_BULK_PRIORITY = -10` preserved foreground priority correctly.
- The current bounded enqueue size of `256` did not cause visible starvation in the bulk-active run.
- The slowest paths remain recursive deep-scroll and recursive total-directory traversal, which look more browse-shape dependent than bulk-cache starvation dependent in this sample.

## Next recommended run

Use the same harness with a larger operation count after this smoke:

```powershell
$env:MANGOO_ARTISTS_SWEEP_OPERATION_COUNT='200'
npm --prefix tools/bench/thumbnail run bench:persistent-cache
```

That should give a better signal on whether recursive deep-scroll and tree-directory spikes need actual scheduler tuning.
