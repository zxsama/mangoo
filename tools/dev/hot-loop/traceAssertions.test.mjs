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
