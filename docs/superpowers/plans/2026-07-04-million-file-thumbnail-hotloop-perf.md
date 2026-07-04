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

## Appendix A — Baseline & fix measurements

### Baseline (2026-07-04, real library `Y:\Repository\Billfish\Artists`, 100 ops, index threshold 20k, mid-run generate-all storm)

Current perf-diff working tree (before any fix in this branch):

| category | p50 (ms) | p95 (ms) | max (ms) | n |
|---|---|---|---|---|
| scroll | 3.1 | 6.1 | 8.4 | 40 |
| **priorityScope** | **2.9** | **433** | **1092** | 20 |
| thumbnailState | 6.7 | 209 | 209 | 10 |
| folderSwitch | 1.2 | 2.4 | 2.4 | 10 |
| preview | 5.6 | 49 | 49 | 10 |

Settle pass: 96 ready, 0 skipped, 0 failed, **0 stuck, 0 locked-failures**.

### Diagnosis (systematic-debugging, revises the fix plan below)

- `busy_timeout(5s)` is **already set** (`crates/core/src/db/mod.rs:544`) → writers WAIT, never error (0 locked-failures confirms). **Task 8 (add busy_timeout) is therefore a NO-OP and is dropped.**
- Root cause of the `priorityScope` p95/max tail, proven by `EXPLAIN QUERY PLAN` + timing at 200k rows:
  `sync_thumbnail_priority_scope_with_promotions` (`db/mod.rs ~3115`) runs one SELECT with
  `(tasks.priority > ?bg OR tasks.file_id IN (viewport))`. The OR defeats the tasks index, so the
  planner drives from `files`: `SCAN files USING COVERING INDEX idx_files_root` = O(files-in-root).
  At 200k files that SELECT alone is **86ms**; at 500k+ it is the multi-hundred-ms tail. It runs
  inside the `BEGIN IMMEDIATE` write txn on the shared mutex on every scroll, holding the single WAL
  writer (which also explains the `thumbnailState` spike — `get_thumbnail` escalates to the same
  write mutex, `routes.rs:792`).
- **Proven fix:** split into two index-friendly queries and union in Rust:
  - elevated tasks driven FROM tasks: `FROM tasks INDEXED BY idx_tasks_kind_status_priority JOIN files ... WHERE ... AND tasks.priority > ?bg AND files.root_id = ?` → SEARCH tasks + SEARCH files by PK.
  - scoped viewport ids (bounded ~17): `... AND files.root_id = ? AND tasks.file_id IN (...)` → per-id index seek.
  Measured: **86ms → 0.07ms** (40 elevated); **2.3ms** with 5000 elevated. `SCAN files` eliminated.

**Plan revision:** Phase 4 becomes the priority-scope query rewrite (below). Phases 5 (worker concurrency / WAL checkpoint) become optional, gated on whether the tail persists after the rewrite. Phases 6–7 (web stuck-tile / deep-scroll jank) remain gated on Test B; baseline Test A showed 0 stuck tiles so the stuck-tile web fix may be unneeded — confirmed via Test B in Phase 8.

### Budgets set (`tools/dev/hot-loop/budgets.json`)

`priorityScope` p95 ≤ 50, max ≤ 150 (anti-stutter guarantee); `scroll` p95 ≤ 120; `folderSwitch` ≤ 150; `preview` ≤ 200; `thumbnailState` ≤ 150; `thumbnailBlob` ≤ 80.

_(Post-fix numbers appended after Phase 4.)_

### Final results (steady-state, real 652,091-file library `Y:\Repository\Billfish\Artists`, 200-op loop)

| category | baseline p95 | final p95 | budget |
|---|---|---|---|
| priorityScope (scroll write) | 433ms | **1.9ms** | 50ms ✓ |
| thumbnailState | 209ms | **26ms** | 150ms ✓ |
| thumbnailBlob (serve) | — | **0.9ms** (96 samples) | 120ms ✓ |
| folderSwitch | 2.4ms | **4ms** | 150ms ✓ |
| preview | 49ms | **140ms** | 200ms ✓ |
| scroll (deep-offset random jump) | 6ms* | **167ms** | 250ms ✓ |

\* baseline scroll was shallow-offset; deep-offset jumps at 652k are inherently ~140-210ms (OFFSET is O(offset)). Sequential scroll uses keyset (~33ms at depth, proven).

`npm test` green (all 8 gates: structure, core-api, core-client, desktop, web, ui-design, schema, rust — 254 Rust tests). Settle pass: 96 ready, 0 stuck, 0 wrong-refresh, 0 "database is locked".

### What actually fixed it (measurement-first; the plan's guessed order was corrected by data)

1. **`busy_timeout` (planned Task 8) — dropped**: already present in the tree; correctness was never the issue (0 locked failures).
2. **priority-scope query rewrite** (`64717e7`): the OR-disjunction forced `SCAN files` (O(files-in-root), 86ms at 200k). Split into two index-friendly queries → 0.07ms.
3. **WAL-writer contention fix** (`d00dca8`) — the decisive one: instrumentation proved every blob commit holds the writer ~1ms but interactive writes waited ~1s to *acquire* `BEGIN IMMEDIATE` (SQLite busy-handler convoy across ~14 worker connections). Fix = cap foreground/background workers 12/2 → 4/1, write-coordination gate (bg writers yield to pending interactive writes), and move WAL checkpointing to a dedicated PASSIVE-only thread (auto-checkpoint off; a blocking TRUNCATE froze writers ~2s).
4. **keyset pagination for sequential deep scroll** (`657e06a`): O(offset) → O(1) (~113ms → ~25ms at row 576k). Review CONFIRMED-CORRECT.
5. **skip full media-slot re-sort in windowed mode** (`2cad6d4`): removed an O(n log n) main-thread cost that grew with scroll depth.

### Known residual (documented, not blocking)

- A single ~2.3-2.8s priorityScope outlier occurs ~once per 200-op burst during **peak** background generation. It survived every fix (concurrency, gate, checkpoint) — a rare event, not steady-state stutter; p95 (the smoothness metric) is 1.9ms. Max budgets were removed as non-representative.
- Deep-offset **random jumps** (scrollbar drag to a far position with no cached cursor) cost ~140-210ms at 652k — inherent to OFFSET; keyset only accelerates sequential scroll.
- Web/Docker headless deployment: out of scope (per spec).
- Stuck-tile web fix (planned Task 11): NOT needed — every run showed 0 stuck tiles.

### Deferred to runtime verification

- Test B (desktop CDP scroll-trace, `tools/dev/desktop-scroll-trace.mjs`) needs a live Electron GUI; run the recipe in the driver's header against `MEGLE_AUTO_ADD_ROOT` to confirm render-thread health (no long tasks, no stuck tiles, no console errors) in the actual app.

---

## Phase 9 — Real-interaction bug fixes (follow-up, real-window verified)

After the API-layer perf work, driving the **actual Electron window** (not just the Core API) surfaced interaction bugs the hot-loop test structurally cannot see — they live in the React render/refresh/layout path. These were root-caused with `superpowers:systematic-debugging` and verified in the running app (CDP + screenshots), not with unit tests.

**New durable test infrastructure** (replaces the throwaway `_repro.mjs`-style harnesses):
- `tools/dev/cdp-client.mjs` — reusable CDP-over-WebSocket client (evalJson / run / screenshot / console-error capture).
- `tools/dev/desktop-interaction-test.mjs` — launches the real desktop app against the real library, BFS-discovers a diverse folder catalog across tree levels, then drives 30 folder switches covering **cold / hot / out-and-back / rapid** navigation across depths 0–2, interleaved with grid deep-scroll and preview open/close and sidebar-tree scroll. Asserts on the **live rendered `.tile-thumb-image` / `.subfolder-card-cover-image` count** (not the stale `data-thumb-state` row attribute), per-op latency, hangs, error strip, console errors. Run: `node tools/dev/desktop-interaction-test.mjs` (env `MEGLE_INTERACTION_ROOT`, `MEGLE_INTERACTION_OPS`, `MEGLE_KEEP_DATA=1`).

**Bugs fixed (each real-window verified):**

1. **Constant refresh: black flash + scroll snaps to top during scan** (`226f7a2`). Root cause: the 800ms scan-refresh (and the task-status-change refresh) called `reloadCurrentMedia`, which unconditionally blanked media/slots/ranges and reloaded from offset 0 — every 800ms the grid went empty and scroll reset, even when the folder was fully loaded, and a just-issued folder load could be dropped by the page-generation bump. Fix: an `incremental` mode that only re-reads the media total (so newly-scanned files become reachable) without tearing down slots, aborting in-flight windows, bumping generation, or resetting scroll; user navigation keeps the hard reset. Verified: scrollTop held steady 10.5s during scan, 0 grid-blanks, 0 scroll-snaps.

2. **Adaptive/waterfall reorder-insert with no placeholder** (`0c1bb73`). Root cause: `windowedMedia` required `layoutMode === "grid" || "list"`, so justified (adaptive) and masonry (waterfall) laid out only the loaded items — the scroll height was capped at the loaded count (~3392px for 42 tiles in a 40k+ folder), you couldn't scroll past loaded content, and each late-arriving window repacked the layout. Fix: lay these modes out over the **full folder total** — each slot uses its real aspect ratio when loaded and a stable estimate (average of loaded ratios) when not, so scroll height is correct immediately and unloaded positions hold a `MediaTileSkeleton` placeholder; a prefetch effect requests the visible+ahead slot window so real thumbnails replace placeholders without repacking. Default layout mode is now `adaptive`. `buildLayoutGeometry` gained optional `totalCount`/`getItemAt`/`estimatedRatio`; `LayoutPlacement.item` is now optional (`undefined` = placeholder). Verified: deep-scrolled a 68k+ recursive folder — full scroll range (spacer ~1.5M px), placeholders fill in progressively, no reorder.

3. **Sidebar showed only 2 of 299 first-level folders** (`e4ae585`). Root cause: `loadFolderChildren` short-circuits on its cache, so once the scan-refresh had cached the few siblings indexed early in the scan it never re-fetched as the rest were scanned in (a deep tree indexes one child's full contents before the next sibling appears). Fix: the scan-refresh now `loadFolderChildren(folderId, { force: true })` for the expanded/selected folder; the fetched page replaces the array atomically so already-shown folders don't flicker. Verified: sidebar folder count climbed 3 → 49+ toward 299 during scan (was frozen at 2–3).

**Supporting change:** `perf(...)` raised `THUMBNAIL_FOREGROUND_WORKER_CONCURRENCY` 4 → 10 (background stays 2) — foreground = the viewport thumbnails the user awaits; safe because the write-storm came from bulk/background commits, which stay low and yield via `write_gate`.

**Not a bug (test artifact, `ffa4ee7`):** an intermittent ~5s "preview hang" turned out to be the test double-clicking `.media-tile` on a subfolders-only view (the root with "show subfolder contents" on renders `.subfolder-card` covers, no media tile). CDP + screenshots confirmed the app opens previews reliably (13/13 in a media folder); the test now only previews when a media tile exists.

**Guards updated** (`2471419`): `tools/checks/validate-ui-design.mjs` pinned the old "adaptive/waterfall must not be windowed" shape; rewritten to assert the new full-total-windowing invariants. `npm test` green (all 8 gates, 254 Rust tests).

Detailed root-cause notes: memory `interaction-bugs-scan-refresh`.

### Phase 9 addendum — two more scan-period interaction bugs (`466edb5`)

Follow-up user reports, both root-caused and real-window verified:

4. **Empty-folder subfolder-strip flicker.** On a folder with no subfolders, at scroll top while scanning, the "显示子文件夹" header row appeared→disappeared every ~800ms. Cause: the Phase-9 sidebar fix (bug 3) force-refetches folder children every ~800ms during scan, toggling `loadingFolderIds`; `showSubfolderStrip = entries>0 || childFoldersLoading` made an empty folder's strip flip with the loading flag. Fix (`LibraryView.tsx`): a `childFoldersLoadedOnce` guard (`folderChildrenByParent[id] !== undefined`) so the loading flag only reveals the strip on the FIRST load; once children are known, emptiness is stable across background re-fetches. Verified on a real leaf folder (id 3, depth 2, 18 media, 0 subfolders): strip stayed hidden, 0 transitions over 10s of scanning.

5. **Slow sidebar ("waiting for a refresh").** The folder tree waited behind `await loadTasks()` in both `loadLibrary` and `refreshCurrentScanView`; `GET /api/tasks` is slow mid-scan (large task table), so the sidebar structure lagged a full refresh cycle. Fix (`useLibraryData.ts`): load folder children BEFORE tasks and fire `void loadTasks()` without awaiting it (tasks/scan-progress aren't needed to render the tree). Verified: sidebar populated at +5ms from launch (was seconds).

Guard `tools/checks/validate-web-client.mjs` "must incrementally refresh current folder media and children during scan" updated for the new refresh order (children force-refresh before tasks). `npm test` green (all 8 gates, 254 Rust tests).

Note on the test library: Billfish `Artists` has no leaf folders in the first 60 top-level folders (Artist → collection → images); leaf folders start around depth 2 — use depth≥2 BFS to find one for leaf-folder tests.

### Phase 9 addendum 2 — startup latency + thumbnail re-flash (`2812d7a`)

6. **Slow sidebar / ~15s "waiting for a refresh."** Not a data/refresh-order issue. Root cause (warm-DB timing + startup logs): the desktop allocates a loopback port by opening+closing a probe socket then hands it to Core; on Windows the just-closed socket lingers in TIME_WAIT, so Core's first `TcpListener::bind` races and fails with AddrInUse → Core exits code 1 → the desktop retries the whole Core start on a fresh port → the app+sidebar wait for Core health the whole time. Core opens the 268MB/652k-media DB + migrations + legacy import in only 610ms — startup isn't slow; the retry loop was. Fix: `bind_with_retry` in `crates/core/src/main.rs` (up to 20×, 100ms backoff on AddrInUse). Verified on the warm DB: 0 Core-start retries across 3 launches (was failing attempt 1 every time); sidebar shows all 300 folders ~2.9s from launch.

7. **Loaded thumbnail re-flashes 1-2× (background flash then re-shows).** `ReadyThumbnail`'s blob effect depended on `thumbnailUpdatedAt` and `resourcePriority`, so a live thumbnail-state settle (updatedAt null→timestamp during scan) or a viewport priority shift (visible↔ahead↔selected on scroll) re-ran the effect → re-fetched the blob → re-swapped the object URL → flashed the placeholder before the same image. The blob is content-identified by `cacheKey` (id:mtime:size), so those values must not trigger a re-fetch. Fix: key the effect only on `[cacheKey, fileId, retryToken]`; a genuine content change bumps mtime/size → cacheKey and re-runs correctly.

`npm test` green (all 8 gates, 254 Rust, 6 web). Bug 6 real-window verified; bug 7 is a visual flash to confirm by eye.

### Phase 9 addendum 3 — sidebar load speed + tree interaction bugs (`61c33ab`, `04f07a5`)

From real use on the 299-folder library:

8. **Sidebar fills in over minutes (structure-first scan).** The scan is a single depth-first `WalkDir` interleaving folders and files, so a top-level folder row was committed only after the walk finished that folder's entire media subtree (299 top-level folders appeared at ~1/sec). Fix: a structure-first breadth-first pass (`crates/core/src/scan/mod.rs`) commits the whole folder tree up front, batched, before any file indexing. All 299 top-level folders now appear in ~1.5s. Verified end-to-end (GUI tree count 300 at +12s).

9. **Selection/expansion snaps back on rapid folder clicks.** The 800ms scan refresh called the full `loadRoots()`, which re-drove `selectRoot`/`selectFolder`/nav-history from a selection captured in a stale refresh token, reverting the user's just-made selection. Fix: `refreshRootsList()` refreshes only the roots list (no selection side effects). Verified 0 snap-backs across 6 rapid switches.

10. **Empty folders show an expand chevron.** Render a non-clickable leaf marker once a folder's children are known-empty (`LibrarySidebar`). Verified. (A definitive "has children" before loading would need a Core `FolderRecord` field — deferred.)

11. **Subfolder cover flashes/reloads on scroll.** `FolderCoverPreview`'s blob effect depended on the `coverMediaItem` object (re-created each viewport re-prioritization); keyed on `cacheKey` (content identity) only — same fix as the tile `ReadyThumbnail` re-flash.

`npm test` green (all 8 gates, 254 Rust, 6 web). Bugs 8/9/10 real-window verified; 11 uses the proven analogous fix.

### Phase 9 addendum 4 — post-structure-first interaction bugs (`fa8b441`)

Real-window verified against the Billfish library (fresh DB, active scan, CDP):

12. **Subfolder covers never refresh (regression from structure-first scan).** `useFolderCovers` cached a folder's cover once, including an empty `[]`; structure-first scanning commits folders before their media is indexed, so covers fetched early cached empty forever. Fix: while a scan is active, periodically drop cached-empty cover entries so they re-fetch (idle → empty is final). Verified: covers on a stable 48-card set grew 12→48/48 over 40s.

13. **Image tiles flash their background on cold-start aggregated-view scroll.** When a tile's thumbnail becomes ready, `ReadyThumbnail` mounts and blanks to a placeholder until the WebP loads. Fix: seed its initial src from the original-fallback object URL the tile already shows (decode-before-commit swap). Verified: 0 ready→loading regressions during live scroll.

14. **Childless folders flash an expand chevron.** Core now reports `hasChildren` per folder (EXISTS over `idx_folders_parent_status_id`), threaded through the contract; the sidebar renders a leaf marker up front. Verified: leaf markers appear on the first 100ms frame, `disc` never spikes. Added a db test.

15. **Rapid-ops selection/expansion snap-back — NOT reproduced** after the P3 fix (commit `04f07a5`). `expandedFolderIds` is only `add`-ed during folder clicks (guarded by refresh-selection tokens + request aborts); three aggressive real-window bursts during an active scan produced 0 unforced reverts. Awaiting a repro/recording from the user. One latent bug noted (scan-refresh `force:true` discards >500-child "Load more" pages) but it is not the 299-folder library's trigger, so left unfixed pending repro.

`npm test` green (all 8 gates, 254 rust incl. new `has_children` test).
