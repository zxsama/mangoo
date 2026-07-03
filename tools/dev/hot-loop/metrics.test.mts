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
