# Million-file thumbnail hot-loop performance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a 100-operation real-usage test against the real ~500k+ library `Y:\Repository\Billfish\Artists`, use it to reproduce and quantify thumbnail-refresh / DB-persistence stutter, then fix the stutter, wrong-refresh, and never-displayed-thumbnail defects until the test passes hard latency budgets.

**Architecture:** Two harnesses over the real Core + real directory. Test A drives the Core HTTP API through a weighted 100-operation interaction loop while background scan + thumbnail generation runs, records per-category latency percentiles, and fails on hard budgets + correctness assertions. Test B is a desktop CDP scroll-trace smoke that catches render-thread jank Test A cannot see. Pure logic (percentiles, budget checks, trace assertions) is extracted into small unit-tested modules; the drivers stay thin. Fixes are applied measurement-first: baseline → confirm root cause → fix → re-measure.

**Tech Stack:** Node `node:test` + `--experimental-strip-types` (`.mts`), raw `fetch` against Core, `ws` for CDP, Rust + rusqlite for Core, React/TypeScript for the UI.

**Spec:** `docs/superpowers/specs/2026-07-04-million-file-thumbnail-hotloop-perf-design.md`

**Branch:** `perf/million-thumbnail-hotloop` (already created; spec already committed).

---

## Conventions used by every task

- Real drive path default: `Y:/Repository/Billfish/Artists`, overridable via `MEGLE_REAL_LOAD_ROOT`.
- Session token for tests: `real-load-test-token` (matches existing `real-load-test.mts`).
- Core address default: `127.0.0.1:47391`, overridable via `MEGLE_REAL_LOAD_CORE_ADDR`.
- Run a single web unit test file: `node --experimental-strip-types --test <file>`.
- Run Rust single test: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test -p megle-core <name>`.
- Commit after each task with the message shown in its final step.

---

## Phase 1 — Test A: API-layer 100-operation harness

### Task 1: Pure metrics module (percentiles + budget check)

**Files:**
- Create: `tools/dev/hot-loop/metrics.mts`
- Test: `tools/dev/hot-loop/metrics.test.mts`

- [ ] **Step 1: Write the failing test**

```ts
// tools/dev/hot-loop/metrics.test.mts
import { strict as assert } from "node:assert";
import { describe, test } from "node:test";
import { percentile, summarize, checkBudgets } from "./metrics.mts";

describe("hot-loop metrics", () => {
  test("percentile picks the nearest-rank sample", () => {
    const samples = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
    assert.equal(percentile(samples, 50), 50);
    assert.equal(percentile(samples, 95), 100);
    assert.equal(percentile(samples, 0), 10);
  });

  test("percentile of empty samples is null", () => {
    assert.equal(percentile([], 95), null);
  });

  test("summarize reports count/p50/p95/max per category", () => {
    const summary = summarize({ scroll: [5, 15, 25], preview: [100] });
    assert.equal(summary.scroll.count, 3);
    assert.equal(summary.scroll.p95, 25);
    assert.equal(summary.scroll.max, 25);
    assert.equal(summary.preview.p50, 100);
  });

  test("checkBudgets returns a violation when p95 exceeds budget", () => {
    const summary = summarize({ scroll: [10, 200] });
    const violations = checkBudgets(summary, { scroll: { p95: 50 } });
    assert.equal(violations.length, 1);
    assert.match(violations[0], /scroll/);
  });

  test("checkBudgets passes when under budget and ignores unbudgeted categories", () => {
    const summary = summarize({ scroll: [10, 20], other: [9999] });
    assert.deepEqual(checkBudgets(summary, { scroll: { p95: 50 } }), []);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `node --experimental-strip-types --test tools/dev/hot-loop/metrics.test.mts`
Expected: FAIL — `Cannot find module './metrics.mts'`.

- [ ] **Step 3: Write minimal implementation**

```ts
// tools/dev/hot-loop/metrics.mts
export type CategorySummary = {
  count: number;
  p50: number | null;
  p95: number | null;
  max: number | null;
};
export type Summary = Record<string, CategorySummary>;
export type Budget = { p50?: number; p95?: number; max?: number };
export type Budgets = Record<string, Budget>;

// Nearest-rank percentile: p in [0,100]. Returns null for no samples.
export function percentile(samples: number[], p: number): number | null {
  if (samples.length === 0) return null;
  const sorted = [...samples].sort((a, b) => a - b);
  if (p <= 0) return sorted[0];
  if (p >= 100) return sorted[sorted.length - 1];
  const rank = Math.ceil((p / 100) * sorted.length);
  return sorted[Math.min(sorted.length - 1, Math.max(0, rank - 1))];
}

export function summarize(byCategory: Record<string, number[]>): Summary {
  const summary: Summary = {};
  for (const [category, samples] of Object.entries(byCategory)) {
    summary[category] = {
      count: samples.length,
      p50: percentile(samples, 50),
      p95: percentile(samples, 95),
      max: samples.length ? Math.max(...samples) : null
    };
  }
  return summary;
}

export function checkBudgets(summary: Summary, budgets: Budgets): string[] {
  const violations: string[] = [];
  for (const [category, budget] of Object.entries(budgets)) {
    const measured = summary[category];
    if (!measured) {
      violations.push(`budget category "${category}" had no measured samples`);
      continue;
    }
    for (const metric of ["p50", "p95", "max"] as const) {
      const limit = budget[metric];
      const value = measured[metric];
      if (limit !== undefined && value !== null && value > limit) {
        violations.push(
          `${category}.${metric}=${value}ms exceeds budget ${limit}ms (n=${measured.count})`
        );
      }
    }
  }
  return violations;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `node --experimental-strip-types --test tools/dev/hot-loop/metrics.test.mts`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add tools/dev/hot-loop/metrics.mts tools/dev/hot-loop/metrics.test.mts
git commit -m "test(perf): add hot-loop metrics module (percentiles + budget check)"
```

---

### Task 2: Operation-plan builder (weighted 100-op sequence)

**Files:**
- Create: `tools/dev/hot-loop/operationPlan.mts`
- Test: `tools/dev/hot-loop/operationPlan.test.mts`

The plan is a deterministic sequence (no `Math.random` — reproducible) of typed operations the driver executes.

- [ ] **Step 1: Write the failing test**

```ts
// tools/dev/hot-loop/operationPlan.test.mts
import { strict as assert } from "node:assert";
import { describe, test } from "node:test";
import { buildOperationPlan, OPERATION_KINDS } from "./operationPlan.mts";

describe("operation plan", () => {
  test("builds exactly the requested number of operations", () => {
    const plan = buildOperationPlan({ totalOperations: 100, mediaTotal: 500_000 });
    assert.equal(plan.length, 100);
  });

  test("covers every operation kind at least once", () => {
    const plan = buildOperationPlan({ totalOperations: 100, mediaTotal: 500_000 });
    const seen = new Set(plan.map((op) => op.kind));
    for (const kind of OPERATION_KINDS) {
      assert.ok(seen.has(kind), `missing operation kind ${kind}`);
    }
  });

  test("deep-scroll offsets increase and stay within media total", () => {
    const plan = buildOperationPlan({ totalOperations: 100, mediaTotal: 1000 });
    const offsets = plan.filter((op) => op.kind === "scroll").map((op) => op.offset);
    assert.ok(offsets.every((o) => o >= 0 && o < 1000));
    for (let i = 1; i < offsets.length; i += 1) {
      assert.ok(offsets[i] >= offsets[i - 1], "scroll offsets must be non-decreasing");
    }
  });

  test("is deterministic for the same input", () => {
    const a = buildOperationPlan({ totalOperations: 100, mediaTotal: 500_000 });
    const b = buildOperationPlan({ totalOperations: 100, mediaTotal: 500_000 });
    assert.deepEqual(a, b);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `node --experimental-strip-types --test tools/dev/hot-loop/operationPlan.test.mts`
Expected: FAIL — module not found.

- [ ] **Step 3: Write minimal implementation**

```ts
// tools/dev/hot-loop/operationPlan.mts
export const OPERATION_KINDS = [
  "scroll", // deep-scroll: request a media window at an offset
  "folderSwitch", // switch to a real subfolder's first page
  "priorityScope", // POST a rolling selected/visible/ahead scope
  "thumbnailState", // poll thumbnail state for a visible window
  "preview", // open the selected item's original bytes
  "thumbnailBlob" // fetch a ready thumbnail blob
] as const;
export type OperationKind = (typeof OPERATION_KINDS)[number];

export type Operation =
  | { kind: "scroll"; offset: number; limit: number }
  | { kind: "folderSwitch"; folderIndex: number }
  | { kind: "priorityScope"; windowStart: number }
  | { kind: "thumbnailState"; windowStart: number }
  | { kind: "preview"; windowStart: number }
  | { kind: "thumbnailBlob"; windowStart: number };

const WINDOW = 96; // grid page size, matches INITIAL_MEDIA_WINDOW_LIMIT in the UI

// Deterministic weighted mix that mirrors real desktop browsing: scrolling
// dominates, priority-scope + state polling shadow it, preview/blob/folder
// switches punctuate. No RNG so runs are comparable.
export function buildOperationPlan(input: {
  totalOperations: number;
  mediaTotal: number;
}): Operation[] {
  const { totalOperations, mediaTotal } = input;
  const maxOffset = Math.max(0, mediaTotal - WINDOW);
  const ops: Operation[] = [];
  let offset = 0;
  const offsetStep = Math.max(WINDOW, Math.floor(maxOffset / Math.max(1, totalOperations)));
  for (let i = 0; i < totalOperations; i += 1) {
    const cycle = i % 10;
    const windowStart = Math.min(maxOffset, offset);
    if (cycle === 0 || cycle === 2 || cycle === 5 || cycle === 7) {
      ops.push({ kind: "scroll", offset: Math.min(maxOffset, offset), limit: WINDOW });
      offset = Math.min(maxOffset, offset + offsetStep);
    } else if (cycle === 1 || cycle === 6) {
      ops.push({ kind: "priorityScope", windowStart });
    } else if (cycle === 3) {
      ops.push({ kind: "thumbnailState", windowStart });
    } else if (cycle === 4) {
      ops.push({ kind: "folderSwitch", folderIndex: Math.floor(i / 10) });
    } else if (cycle === 8) {
      ops.push({ kind: "thumbnailBlob", windowStart });
    } else {
      ops.push({ kind: "preview", windowStart });
    }
  }
  return ops;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `node --experimental-strip-types --test tools/dev/hot-loop/operationPlan.test.mts`
Expected: PASS (4 tests).

Note: the "covers every kind" test requires each kind to appear within 100 ops; the cycle mapping above emits all six kinds within the first 10 ops.

- [ ] **Step 5: Commit**

```bash
git add tools/dev/hot-loop/operationPlan.mts tools/dev/hot-loop/operationPlan.test.mts
git commit -m "test(perf): add deterministic 100-op plan builder"
```

---

### Task 3: The driver — spawn Core, seed real root, run the loop

**Files:**
- Create: `tools/dev/real-hot-loop-test.mts`
- Reference (copy helper patterns, do not import): `tools/dev/real-load-test.mts:96-206` (process lifecycle, `fetchJson`, `fetchResponse`, `waitForHealth`).

This driver is an integration harness; it is validated by running it against the real drive (Task in Phase 3), not by a unit test. Keep pure logic in the Phase-1 modules.

- [ ] **Step 1: Create the driver skeleton (process + HTTP + index wait)**

Create `tools/dev/real-hot-loop-test.mts` with:
- Constants: `TOKEN = "real-load-test-token"`, `ADDR = process.env.MEGLE_REAL_LOAD_CORE_ADDR ?? "127.0.0.1:47391"`, `BASE = http://${ADDR}/api`, `ROOT_PATH = (process.env.MEGLE_REAL_LOAD_ROOT ?? "Y:/Repository/Billfish/Artists").replace(/\\/g,"/")`, `TOTAL_OPERATIONS = Number(process.env.MEGLE_HOTLOOP_OPS ?? 100)`, `INDEX_THRESHOLD = Number(process.env.MEGLE_HOTLOOP_INDEX_THRESHOLD ?? 20000)`, `INDEX_WAIT_TIMEOUT_MS = Number(process.env.MEGLE_HOTLOOP_INDEX_TIMEOUT_MS ?? 600_000)`.
- Copy `spawnManaged`, `terminateProcessTree`, `waitForClose`, `runTaskkill`, `isExited`, `fetchJson`, `fetchResponse`, `waitForHealth`, `getTask` from `real-load-test.mts` (verbatim — the engineer may read tasks out of order, so these must be present in this file).
- `spawn("cargo", ["run","--release","-q","-p","megle-core"], { env: { ...process.env, MEGLE_SESSION_TOKEN: TOKEN, MEGLE_CORE_ADDR: ADDR, MEGLE_DB_PATH: <temp>, RUST_LOG: "warn" }})`.
- `addRoot`: `POST /roots {path: ROOT_PATH}` → `{accepted, taskId, rootId}`.
- `waitForIndexThreshold(rootId, scanTaskId)`: poll `GET /media?rootId=..&limit=1&sort=mtime_desc` reading `totalCount`; resolve when `totalCount >= INDEX_THRESHOLD` OR scan task reaches a terminal status; throw on `INDEX_WAIT_TIMEOUT_MS`.

- [ ] **Step 2: Add the operation executors**

Add async functions, each returning elapsed ms via `const t0 = performance.now(); ...; return performance.now() - t0;`:

```ts
async function opScroll(rootId: number, offset: number, limit: number): Promise<number> {
  const t0 = performance.now();
  await fetchJson(`/media?rootId=${rootId}&limit=${limit}&offset=${offset}&sort=mtime_desc`);
  return performance.now() - t0;
}

async function opFolderSwitch(rootId: number, folder: { id: number }): Promise<number> {
  const t0 = performance.now();
  await fetchJson(`/media?rootId=${rootId}&folderId=${folder.id}&limit=96&sort=mtime_desc`);
  return performance.now() - t0;
}

async function opPriorityScope(
  rootId: number,
  ids: { selected: number[]; visible: number[]; ahead: number[] }
): Promise<number> {
  const t0 = performance.now();
  await fetchJson(`/tasks/thumbnail-priority-scope`, {
    method: "POST",
    body: JSON.stringify({
      rootId,
      selectedFileIds: ids.selected,
      visibleFileIds: ids.visible,
      aheadFileIds: ids.ahead
    })
  });
  return performance.now() - t0;
}

async function opThumbnailState(fileIds: number[], priority: string): Promise<number> {
  const t0 = performance.now();
  await Promise.all(
    fileIds.map((id) =>
      fetchJson(`/media/${id}/thumbnail?target=grid_320&priority=${priority}`)
    )
  );
  return performance.now() - t0;
}

async function opPreview(fileId: number): Promise<number> {
  const t0 = performance.now();
  const response = await fetchResponse(`/media/${fileId}/preview`);
  await response.body?.cancel();
  return performance.now() - t0;
}

async function opThumbnailBlob(fileId: number): Promise<number> {
  const t0 = performance.now();
  const response = await fetchResponse(`/media/${fileId}/thumbnail/blob?target=grid_320`);
  await response.arrayBuffer();
  return performance.now() - t0;
}
```

- [ ] **Step 3: Wire the 100-op loop with a rolling window cache**

- Maintain `windowCache: Map<number, MediaRecord[]>` filled by each `scroll`/`folderSwitch` result (offset → items) so priority/state/preview/blob ops target real file ids near the current scroll position.
- For `priorityScope`, split the current window's ids into `selected` (1), `visible` (next N), `ahead` (next N).
- Before the loop, discover a handful of real subfolders (reuse `discoverMediaFoldersOnce` pattern from `real-load-test.mts:439-471`, cap at 250 visited) for `folderSwitch`.
- **Mid-run write storm:** at operation index 20, `POST /thumbnails/enqueue-cache {rootId, includeDescendants:true, refreshMode:"staleOrMissing", limit:4096}` once, to force generate-all pressure while the remaining ops run.
- Push each op's elapsed ms into `byCategory[op.kind]`.

- [ ] **Step 4: Summarize + gate on budgets (budgets loaded from JSON, absent-file = report-only)**

```ts
import { summarize, checkBudgets, type Budgets } from "./hot-loop/metrics.mts";
import { readFileSync, existsSync } from "node:fs";

const budgetsPath = new URL("./hot-loop/budgets.json", import.meta.url);
const budgets: Budgets = existsSync(budgetsPath) ? JSON.parse(readFileSync(budgetsPath, "utf8")) : {};
const summary = summarize(byCategory);
console.log("[hotloop] summary:", JSON.stringify(summary, null, 2));
const violations = checkBudgets(summary, budgets);
```

Correctness assertions (throw on violation) — implemented in Task 4. Budget violations: if `budgets` is non-empty and `violations.length > 0`, throw at the end (after correctness checks) so the process exits non-zero.

- [ ] **Step 5: Run to verify the harness starts (short smoke, low threshold)**

Run: `MEGLE_HOTLOOP_INDEX_THRESHOLD=200 MEGLE_HOTLOOP_OPS=20 node --experimental-strip-types tools/dev/real-hot-loop-test.mts`
Expected: Core builds+starts, root added, ≥200 media indexed, 20 ops run, a `[hotloop] summary:` JSON prints, process exits 0 (no budgets.json yet → report-only). If `Y:` is not mounted, expect a clear "Test directory does not exist" error — that is acceptable for this step; note it and continue.

- [ ] **Step 6: Commit**

```bash
git add tools/dev/real-hot-loop-test.mts
git commit -m "test(perf): add real 100-op API hot-loop driver"
```

---

### Task 4: Correctness assertions in the driver

**Files:**
- Modify: `tools/dev/real-hot-loop-test.mts`

- [ ] **Step 1: Add a settle-and-verify pass after the loop**

After the 100 ops, for the last scrolled window's ids:
- Poll `GET /media/{id}/thumbnail?...&priority=visible` until every id is terminal (`ready`/`skipped_small`/`failed`) or a 60s timeout — collect `stuckIds` for any that never settle.
- For each `ready` id: `GET /media/{id}/thumbnail/blob` and assert `content-type: image/webp`, header bytes `RIFF`/`WEBP`, and `byteSize` matches the state's `byteSize` (reuse `verifyThumbnailBlob` logic from `real-load-test.mts:509-534`).
- Assert returned `fileId`/`target` equal the requested id/`grid_320` (no wrong-refresh).

- [ ] **Step 2: Add the assertions block**

```ts
if (stuckIds.length > 0) {
  throw new Error(`thumbnails never reached terminal state: ${stuckIds.join(", ")}`);
}
const lockedFailures = terminalStates.filter(
  (t) => t.state === "failed" && /database is locked/i.test(t.error ?? "")
);
if (lockedFailures.length > 0) {
  throw new Error(`${lockedFailures.length} thumbnails failed with "database is locked"`);
}
```

- [ ] **Step 3: Run the short smoke again**

Run: `MEGLE_HOTLOOP_INDEX_THRESHOLD=200 MEGLE_HOTLOOP_OPS=20 node --experimental-strip-types tools/dev/real-hot-loop-test.mts`
Expected: completes; prints summary; either passes or throws a specific correctness error (that is a real finding, not a harness bug).

- [ ] **Step 4: Commit**

```bash
git add tools/dev/real-hot-loop-test.mts
git commit -m "test(perf): assert thumbnail terminality, blob validity, no wrong-refresh"
```

---

## Phase 2 — Test B: desktop CDP scroll-trace smoke

### Task 5: Trace-assertion module (pure)

**Files:**
- Create: `tools/dev/hot-loop/traceAssertions.mjs`
- Test: `tools/dev/hot-loop/traceAssertions.test.mjs`

- [ ] **Step 1: Write the failing test**

```js
// tools/dev/hot-loop/traceAssertions.test.mjs
import { strict as assert } from "node:assert";
import { describe, test } from "node:test";
import { assessScrollTrace } from "./traceAssertions.mjs";

describe("scroll trace assessment", () => {
  test("flags a long task over budget", () => {
    const result = assessScrollTrace(
      { longTasks: [40, 220], finalTiles: { loading: 0, failed: 0, ready: 50 }, consoleErrors: [] },
      { maxLongTaskMs: 100 }
    );
    assert.equal(result.ok, false);
    assert.match(result.violations.join(" "), /long task/i);
  });

  test("flags tiles stuck loading and console errors", () => {
    const result = assessScrollTrace(
      { longTasks: [], finalTiles: { loading: 7, failed: 2, ready: 50 }, consoleErrors: ["boom"] },
      { maxLongTaskMs: 100 }
    );
    assert.equal(result.ok, false);
    assert.equal(result.violations.length, 3); // loading>0, failed>0, consoleErrors>0
  });

  test("passes a clean trace", () => {
    const result = assessScrollTrace(
      { longTasks: [10, 30], finalTiles: { loading: 0, failed: 0, ready: 80 }, consoleErrors: [] },
      { maxLongTaskMs: 100 }
    );
    assert.deepEqual(result, { ok: true, violations: [] });
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `node --experimental-strip-types --test tools/dev/hot-loop/traceAssertions.test.mjs`
Expected: FAIL — module not found.

- [ ] **Step 3: Write minimal implementation**

```js
// tools/dev/hot-loop/traceAssertions.mjs
export function assessScrollTrace(trace, budget) {
  const violations = [];
  const worstLongTask = trace.longTasks.length ? Math.max(...trace.longTasks) : 0;
  if (worstLongTask > budget.maxLongTaskMs) {
    violations.push(`main-thread long task ${worstLongTask}ms exceeds ${budget.maxLongTaskMs}ms`);
  }
  if (trace.finalTiles.loading > 0) {
    violations.push(`${trace.finalTiles.loading} tiles stuck loading after scroll settled`);
  }
  if (trace.finalTiles.failed > 0) {
    violations.push(`${trace.finalTiles.failed} tiles failed`);
  }
  if (trace.consoleErrors.length > 0) {
    violations.push(`${trace.consoleErrors.length} console errors: ${trace.consoleErrors.slice(0, 3).join(" | ")}`);
  }
  return { ok: violations.length === 0, violations };
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `node --experimental-strip-types --test tools/dev/hot-loop/traceAssertions.test.mjs`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add tools/dev/hot-loop/traceAssertions.mjs tools/dev/hot-loop/traceAssertions.test.mjs
git commit -m "test(perf): add desktop scroll-trace assessment module"
```

---

### Task 6: Desktop scroll-trace driver

**Files:**
- Create: `tools/dev/desktop-scroll-trace.mjs`
- Reference (CDP-over-ws pattern): `tools/dev/cdp-inspect.mjs:9-105`.

- [ ] **Step 1: Implement the driver**

- Take the page ws URL as `process.argv[2]` (same as `cdp-inspect.mjs`); document the wire-up one-liner in a header comment (resolve via `http://127.0.0.1:9222/json`).
- `Runtime.enable`, `Log.enable`.
- Inject a `PerformanceObserver` for `longtask` into the page that pushes durations onto `window.__megleLongTasks`:

```js
await send("Runtime.evaluate", { expression: `
  window.__megleLongTasks = [];
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) window.__megleLongTasks.push(entry.duration);
  }).observe({ entryTypes: ["longtask"] });
  true;
` });
```

- Scroll loop: 30 iterations of `document.querySelector(".virtual-grid").scrollTop += 2000` via `Runtime.evaluate`, `await` ~120ms between steps (`Emulation`/timer). After each, no assertion.
- After settling (~2s), read `window.__megleLongTasks` and tile counts using the exact selectors from `cdp-inspect.mjs:56-60` (`.tile-thumb-ready` / `.tile-thumb-loading` / `.tile-thumb-failed`), and collect console errors from captured `Runtime.consoleAPICalled` / `Log.entryAdded` events with level `error`.
- Build `{ longTasks, finalTiles: { ready, loading, failed }, consoleErrors }`, call `assessScrollTrace(trace, { maxLongTaskMs: Number(process.env.MEGLE_TRACE_MAX_LONGTASK_MS ?? 100) })`, print the result, `process.exit(result.ok ? 0 : 1)`.

- [ ] **Step 2: Manual smoke (documented, not auto-run)**

Add a header comment with the run recipe:
```
# Terminal 1: MEGLE_REMOTE_DEBUG=1 MEGLE_AUTO_ADD_ROOT="Y:/Repository/Billfish/Artists" npm run dev
# Terminal 2:
WS=$(curl -sS http://127.0.0.1:9222/json | python -c "import json,sys; print([p for p in json.load(sys.stdin) if p['type']=='page'][0]['webSocketDebuggerUrl'])")
node tools/dev/desktop-scroll-trace.mjs "$WS"
```
Run it once the app is up; expect a JSON verdict. This is the desktop render-jank baseline for Phase 3.

- [ ] **Step 3: Commit**

```bash
git add tools/dev/desktop-scroll-trace.mjs
git commit -m "test(perf): add desktop CDP scroll-trace smoke driver"
```

---

## Phase 3 — Baseline measurement + budget setting

### Task 7: Capture baseline and write budgets.json

**Files:**
- Create: `tools/dev/hot-loop/budgets.json`
- Modify (append results): `docs/superpowers/plans/2026-07-04-million-file-thumbnail-hotloop-perf.md` (Appendix A below).

- [ ] **Step 1: Run the full 100-op test against the real drive (report-only, no budgets yet)**

Run: `MEGLE_KEEP_DATA=1 node --experimental-strip-types tools/dev/real-hot-loop-test.mts`
Expected: prints `[hotloop] summary:` with per-category `count/p50/p95/max`, plus any correctness errors. Record the summary JSON and any errors (esp. `database is locked` failures and stuck tiles) into Appendix A.

- [ ] **Step 2: Run the desktop scroll-trace baseline**

Run the Task-6 recipe. Record `longTasks` worst value, final tile counts, console errors into Appendix A.

- [ ] **Step 3: Derive budgets and write budgets.json**

For each category set `p95 = ceil(baseline_p95 * 1.3)` with a documented target ceiling per category (the ceiling is the product goal; if baseline already beats the ceiling, use the ceiling). Suggested ceilings, refined by Appendix A: `priorityScope.p95 ≤ 50`, `scroll.p95 ≤ 120`, `folderSwitch.p95 ≤ 150`, `preview.p95 ≤ 200`, `thumbnailState.p95 ≤ 150`, `thumbnailBlob.p95 ≤ 80`.

```json
{
  "priorityScope": { "p95": 50 },
  "scroll": { "p95": 120 },
  "folderSwitch": { "p95": 150 },
  "preview": { "p95": 200 },
  "thumbnailState": { "p95": 150 },
  "thumbnailBlob": { "p95": 80 }
}
```

Adjust each number to `max(ceiling, ceil(baseline_p95*1.3))` only if the baseline cannot yet meet the ceiling; the fix phases must then drive it down to the ceiling.

- [ ] **Step 4: Re-run with budgets active to confirm the gate fires**

Run: `node --experimental-strip-types tools/dev/real-hot-loop-test.mts`
Expected: if the baseline is over any ceiling, the process exits non-zero listing violations. This failing state is the target the fix phases must turn green.

- [ ] **Step 5: Commit**

```bash
git add tools/dev/hot-loop/budgets.json docs/superpowers/plans/2026-07-04-million-file-thumbnail-hotloop-perf.md
git commit -m "test(perf): capture million-file baseline and set hot-loop budgets"
```

---

## Phase 4 — Fix 1: eliminate SQLITE_BUSY under write contention (Rust, high-confidence)

Root cause (verified by reading `configure_file_database_wal`, `crates/core/src/db/mod.rs:6990`): no `busy_timeout` is set, so a second writer (scroll priority-scope vs. 14 worker blob commits) gets `SQLITE_BUSY` immediately, surfacing as `database is locked` thumbnail failures and failed scroll writes.

### Task 8: Set busy_timeout on file-backed connections

**Files:**
- Modify: `crates/core/src/db/mod.rs` (constants near line 18-38; `configure_file_database_wal` at line 6990)
- Test: `crates/core/src/db/mod.rs` (add a `#[test]` in the existing test module)

- [ ] **Step 1: Write the failing test**

Add near the other db tests:

```rust
#[test]
fn file_database_sets_busy_timeout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("busy.sqlite");
    let database = Database::open(&db_path).expect("open file database");
    let timeout: i64 = database
        .connection
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .expect("query busy_timeout");
    assert_eq!(timeout, SQLITE_BUSY_TIMEOUT_MS);
}
```

(If the test module cannot reach `database.connection` directly, add a `#[cfg(test)] pub(crate) fn busy_timeout_for_test(&self) -> anyhow::Result<i64>` that runs the pragma, and assert on that instead.)

- [ ] **Step 2: Run to verify it fails**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test -p megle-core file_database_sets_busy_timeout`
Expected: FAIL — `SQLITE_BUSY_TIMEOUT_MS` not defined / timeout is 0.

- [ ] **Step 3: Implement**

Add the constant near the other pragma constants (after line 38):

```rust
// A second writer must wait for the WAL write lock instead of failing with
// SQLITE_BUSY. All thumbnail/scroll writes are short; every DB row is
// regenerable, so waiting briefly is strictly better than a spurious failure.
const SQLITE_BUSY_TIMEOUT_MS: i64 = 5_000;
```

In `configure_file_database_wal`, after the `temp_store` line:

```rust
    connection.pragma_update(None, "busy_timeout", SQLITE_BUSY_TIMEOUT_MS)?;
```

- [ ] **Step 4: Run to verify it passes**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test -p megle-core file_database_sets_busy_timeout`
Expected: PASS.

- [ ] **Step 5: Full crate tests + schema check**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test -p megle-core`
Expected: all pass (no regressions).

- [ ] **Step 6: Re-run the hot-loop; confirm zero "database is locked"**

Run: `node --experimental-strip-types tools/dev/real-hot-loop-test.mts`
Expected: the `lockedFailures` assertion no longer throws; record the new priorityScope p95 in Appendix A.

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/db/mod.rs
git commit -m "fix(db): set busy_timeout so scroll/worker writes wait instead of failing"
```

---

## Phase 5 — Fix 2: cut write-lock hold time so scroll writes don't stall (Rust, measurement-gated)

**Gate:** Only do this phase if, after Task 8, Appendix A shows `priorityScope.p95` still over its ceiling (50ms) under the mid-run generate-all storm. If it is already under budget, mark this phase skipped in Appendix A and move on.

Root cause hypothesis: 14 concurrent worker connections each take `BEGIN IMMEDIATE` per thumbnail; with `busy_timeout` now set, the scroll write waits (no longer errors) but can still queue behind many blob commits + periodic WAL checkpoints (`wal_autocheckpoint=4096`).

### Task 9: Reduce concurrent blob-writer pressure during active foreground scope

**Files:**
- Modify: `crates/core/src/tasks.rs:20-22` (worker concurrency constants)
- Test: `crates/core/src/tasks.rs` (existing worker test module)

- [ ] **Step 1: Confirm the hypothesis from the baseline**

Read Appendix A. Confirm priorityScope p95 correlates with the generate-all storm window (compare ops before vs after index 20). If it does not, STOP and instead investigate WAL checkpoint stalls (Task 10) — do not apply this task blindly.

- [ ] **Step 2: Lower background concurrency and add a foreground-scope-aware cap**

Change:

```rust
const THUMBNAIL_FOREGROUND_WORKER_CONCURRENCY: usize = 12;
const THUMBNAIL_BACKGROUND_WORKER_CONCURRENCY: usize = 2;
```

to a measured lower value that keeps the single WAL writer available for scroll writes (start conservative, tune from Appendix A):

```rust
// Blob commits serialize on the single WAL writer; too many concurrent
// foreground writers starve the interactive scroll priority-scope write.
// Tuned against the million-file hot-loop (see plan Appendix A).
const THUMBNAIL_FOREGROUND_WORKER_CONCURRENCY: usize = 6;
const THUMBNAIL_BACKGROUND_WORKER_CONCURRENCY: usize = 1;
```

- [ ] **Step 3: Run worker tests**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test -p megle-core tasks`
Expected: PASS (the drain-order / priority tests still hold).

- [ ] **Step 4: Re-measure the hot-loop; record before/after in Appendix A**

Run: `node --experimental-strip-types tools/dev/real-hot-loop-test.mts`
Expected: priorityScope p95 drops under 50ms. If it does not improve enough, proceed to Task 10; otherwise Task 10 is optional.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/tasks.rs
git commit -m "perf(thumbnails): cap foreground blob-writer concurrency to protect scroll writes"
```

### Task 10 (optional, only if Task 9 insufficient): tune WAL checkpoint cadence

**Files:**
- Modify: `crates/core/src/db/mod.rs:19` (`WAL_AUTOCHECKPOINT_PAGES`)

- [ ] **Step 1: Lower the autocheckpoint threshold to shorten checkpoint stalls**

Change `WAL_AUTOCHECKPOINT_PAGES` from `4096` to `1000` (smaller, more frequent, shorter checkpoints reduce worst-case writer stalls). Document the trade-off in a comment.

- [ ] **Step 2: Run crate tests + hot-loop; record in Appendix A**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test -p megle-core` then `node --experimental-strip-types tools/dev/real-hot-loop-test.mts`
Expected: priorityScope max/p99 stalls shrink; p95 under budget.

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/db/mod.rs
git commit -m "perf(db): shorten WAL autocheckpoint to reduce writer stalls"
```

---

## Phase 6 — Fix 3: thumbnails that never display (Web, reproduce-then-fix)

**Gate:** Only if Test B (Task 6 / Appendix A) shows `finalTiles.loading > 0` after scroll settles, or Test A reports stuck ids. If neither, mark skipped.

### Task 11: Reproduce the stuck-loading tile in a unit test, then fix

**Files:**
- Modify: `apps/web/src/features/media-grid/MediaGrid.tsx:1579-1608` (`ThumbnailStateView`) and/or `:2015-2112` (`ReadyThumbnail` effect)
- Test: `apps/web/tests/thumbnailReadiness.test.mts` (new)

Root-cause candidate (verified by reading `ThumbnailStateView`, MediaGrid.tsx:1582-1594): `canLoadCurrentThumbnailBlob = hasLiveReadyThumbnail || rowState === "ready"`. When the `/api/media` list omits per-row thumbnail metadata (`item.thumbnailState` stale as "pending") AND the live state fetch was aborted mid-scroll, the tile can never satisfy either branch, so it never mounts `ReadyThumbnail`, so it never fetches the blob → stuck loading. `shouldRefreshThumbnailState` (MediaGrid.tsx:1744) must keep re-requesting state in exactly this case.

- [ ] **Step 1: Write a failing unit test for the readiness predicate**

Extract the predicate to a pure exported function `shouldAttemptThumbnailBlob(item, thumbnail)` (mirrors `canLoadCurrentThumbnailBlob`) and test it:

```ts
// apps/web/tests/thumbnailReadiness.test.mts
import { strict as assert } from "node:assert";
import { describe, test } from "node:test";
import { shouldAttemptThumbnailBlob, shouldRefreshThumbnailState } from "../src/features/media-grid/thumbnailReadiness.ts";

const item = (state: string | null) => ({ id: 1, thumbnailState: state } as any);

describe("thumbnail readiness", () => {
  test("attempts blob when live state is ready even if row state is stale-pending", () => {
    assert.equal(
      shouldAttemptThumbnailBlob(item("pending"), { fileId: 1, target: "grid_320", state: "ready", updatedAt: 123 } as any),
      true
    );
  });
  test("keeps refreshing state when row is stale-pending and no live metadata yet", () => {
    assert.equal(shouldRefreshThumbnailState(item("pending"), undefined), true);
  });
  test("does not attempt blob for a genuinely skipped_small item", () => {
    assert.equal(shouldAttemptThumbnailBlob(item("skipped_small"), undefined), false);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `node --experimental-strip-types --test apps/web/tests/thumbnailReadiness.test.mts`
Expected: FAIL — module `thumbnailReadiness.ts` not found.

- [ ] **Step 3: Extract + fix the predicate**

Create `apps/web/src/features/media-grid/thumbnailReadiness.ts` exporting `shouldAttemptThumbnailBlob` and moving `shouldRefreshThumbnailState` + `normalizeMediaThumbnailState` there (single source of truth). `shouldAttemptThumbnailBlob` returns true when `thumbnail?.state === "ready"` OR `rowState === "ready"`. Import both into `MediaGrid.tsx`, replace the inline `canLoadCurrentThumbnailBlob` / `shouldRefreshThumbnailState` definitions with the imports. Confirm `ThumbnailStateView` mounts `ReadyThumbnail` whenever `shouldAttemptThumbnailBlob` is true, and the visible/ahead repoll effect (MediaGrid.tsx:807-832) keeps polling while `shouldRefreshThumbnailState` is true so a stuck tile self-heals.

- [ ] **Step 4: Run to verify it passes**

Run: `node --experimental-strip-types --test apps/web/tests/thumbnailReadiness.test.mts`
Expected: PASS.

- [ ] **Step 5: Typecheck the web workspace**

Run: `npm --workspace @megle/web run typecheck`
Expected: no errors.

- [ ] **Step 6: Re-run Test B; confirm `finalTiles.loading` reaches 0**

Run the Task-6 recipe. Expected: after settle, loading count → 0, ready grows, no console errors. Record in Appendix A.

- [ ] **Step 7: Commit**

```bash
git add apps/web/src/features/media-grid/thumbnailReadiness.ts apps/web/src/features/media-grid/MediaGrid.tsx apps/web/tests/thumbnailReadiness.test.mts
git commit -m "fix(web): self-heal thumbnails stuck in loading via shared readiness predicate"
```

---

## Phase 7 — Fix 4: deep-scroll main-thread jank (Web)

**Gate:** Only if Test B baseline shows a main-thread long task over budget during deep scroll. If not, mark skipped.

### Task 12: Avoid full re-sort of loaded media slots on every window load

**Files:**
- Modify: `apps/web/src/features/media-grid/MediaGrid.tsx:311-315` (`orderedLoadedMedia` / `layoutItems`) and `:1542-1546` (`compactMediaSlots`)
- Test: `apps/web/tests/mediaSlots.test.mts` (new)

Root cause (verified by reading MediaGrid.tsx:311-314, 376-385): `orderedLoadedMedia = compactMediaSlots(mediaSlots)` re-sorts ALL loaded slots on every `mediaSlots` change; it is only consumed for `layoutItems` (empty in windowed mode) and `loadedMediaById`. In windowed mode the sort is pure waste and grows O(n log n) with scroll depth.

- [ ] **Step 1: Write a failing test for a windowed-mode builder that skips the sort**

```ts
// apps/web/tests/mediaSlots.test.mts
import { strict as assert } from "node:assert";
import { describe, test } from "node:test";
import { buildLoadedMediaById } from "../src/features/media-grid/mediaSlots.ts";

describe("loaded media index", () => {
  test("indexes items from slots without sorting the whole map", () => {
    const slots = new Map<number, any>([
      [5, { id: 105 }],
      [1, { id: 101 }]
    ]);
    const byId = buildLoadedMediaById([], slots);
    assert.equal(byId.get(101).id, 101);
    assert.equal(byId.get(105).id, 105);
    assert.equal(byId.size, 2);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `node --experimental-strip-types --test apps/web/tests/mediaSlots.test.mts`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement + wire**

Create `apps/web/src/features/media-grid/mediaSlots.ts` with `compactMediaSlots` (moved verbatim) and a new `buildLoadedMediaById(items, slots)` that fills a `Map` directly (no sort). In `MediaGrid.tsx`: import both; keep `orderedLoadedMedia = compactMediaSlots(...)` ONLY for the non-windowed layout path (guard: compute it via `useMemo` only when `!windowedMedia`, else `EMPTY_MEDIA_ITEMS`); replace the `loadedMediaById` useMemo body with `buildLoadedMediaById(items, mediaSlots ?? undefined)`.

- [ ] **Step 4: Run to verify it passes**

Run: `node --experimental-strip-types --test apps/web/tests/mediaSlots.test.mts`
Expected: PASS.

- [ ] **Step 5: Typecheck**

Run: `npm --workspace @megle/web run typecheck`
Expected: no errors.

- [ ] **Step 6: Re-run Test B; confirm long-task budget met**

Run the Task-6 recipe. Expected: worst long task under `MEGLE_TRACE_MAX_LONGTASK_MS` (100). Record in Appendix A.

- [ ] **Step 7: Commit**

```bash
git add apps/web/src/features/media-grid/mediaSlots.ts apps/web/src/features/media-grid/MediaGrid.tsx apps/web/tests/mediaSlots.test.mts
git commit -m "perf(web): drop full media-slot re-sort in windowed deep scroll"
```

### Task 13: Confirm seen-persist cadence does not amplify write pressure

**Files:**
- Modify (only if needed): `apps/web/src/core/useLibraryData.ts:45-47` (`SEEN_THUMBNAIL_PERSIST_*` constants)

- [ ] **Step 1: Inspect during the hot-loop**

With `RUST_LOG=info` on Core during Test A, confirm seen-persist enqueues run at `background` priority and do not coincide with priorityScope p95 spikes. (The POST already uses `requestPriority: "background"`, useLibraryData.ts:707.)

- [ ] **Step 2: If (and only if) it correlates with spikes, raise the debounce**

Change `SEEN_THUMBNAIL_PERSIST_DEBOUNCE_MS` from `500` to `1000` and `SEEN_THUMBNAIL_PERSIST_BATCH_SIZE` to a smaller value if batches are large. Otherwise mark skipped in Appendix A.

- [ ] **Step 3: Typecheck + re-run Test A**

Run: `npm --workspace @megle/web run typecheck` then `node --experimental-strip-types tools/dev/real-hot-loop-test.mts`
Expected: budgets green.

- [ ] **Step 4: Commit (only if changed)**

```bash
git add apps/web/src/core/useLibraryData.ts
git commit -m "perf(web): relax seen-persist cadence to reduce background write pressure"
```

---

## Phase 8 — Final verification

### Task 14: Green hot-loop + existing gates

- [ ] **Step 1: Run the full 100-op test with budgets active**

Run: `node --experimental-strip-types tools/dev/real-hot-loop-test.mts`
Expected: exits 0 — all budgets met, no stuck tiles, no locked failures, blobs valid.

- [ ] **Step 2: Run the desktop scroll-trace**

Run the Task-6 recipe. Expected: `{ ok: true, violations: [] }`.

- [ ] **Step 3: Run the repo web unit tests + Rust tests**

Run: `npm --workspace @megle/web run test` and `export PATH="$HOME/.cargo/bin:$PATH"; cargo test -p megle-core`
Expected: all pass.

- [ ] **Step 4: Run the full check suite**

Run: `npm test`
Expected: all checks pass (structure, contracts, schema, typecheck, rust). Note: the new `tools/dev/hot-loop/*.test.mts` unit tests are not in `npm test` by default — decide with the user whether to wire `tools/dev/hot-loop/*.test.mts` into `check:web` or a new `check:perf` script (the real 100-op driver stays opt-in since it needs the `Y:` drive).

- [ ] **Step 5: Update Appendix A with final numbers and commit**

```bash
git add docs/superpowers/plans/2026-07-04-million-file-thumbnail-hotloop-perf.md
git commit -m "docs(perf): record final hot-loop results and fixes"
```

---

## Self-review notes (author)

- Spec coverage: Test A (Phase 1) = 100-op API test + budgets + correctness; Test B (Phase 2) = desktop render smoke; baseline+budgets (Phase 3); fixes for write-lock (Phase 4/5), stuck tiles (Phase 6), deep-scroll jank + seen-persist (Phase 7); final verification (Phase 8). All spec sections mapped.
- Measurement-first: Phases 5/6/7 are explicitly gated on Appendix A findings, honoring systematic-debugging while giving concrete primary code.
- Type consistency: `summarize`/`checkBudgets`/`percentile`, `buildOperationPlan`/`OPERATION_KINDS`, `assessScrollTrace`, `shouldAttemptThumbnailBlob`/`shouldRefreshThumbnailState`, `buildLoadedMediaById`/`compactMediaSlots` used consistently across tasks.

## Appendix A — Baseline & fix measurements (filled during execution)

_(To be filled in Phase 3 and updated after each fix: per-category p50/p95/max before and after, locked-failure counts, stuck-tile counts, worst long task.)_
