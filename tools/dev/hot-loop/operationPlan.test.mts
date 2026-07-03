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

  test("priority-scope dominates the sample for a stable p95", () => {
    // priority-scope is the interactive-write hot path under test, so it must
    // get the most samples of any single kind — otherwise p95 is dominated by
    // one unlucky collision with the background write burst.
    const plan = buildOperationPlan({ totalOperations: 200, mediaTotal: 500_000 });
    const counts = new Map<string, number>();
    for (const op of plan) {
      counts.set(op.kind, (counts.get(op.kind) ?? 0) + 1);
    }
    const priorityScope = counts.get("priorityScope") ?? 0;
    assert.ok(priorityScope >= 70, `expected >=70 priorityScope ops, got ${priorityScope}`);
    for (const [kind, count] of counts) {
      if (kind !== "priorityScope") {
        assert.ok(
          priorityScope >= count,
          `priorityScope (${priorityScope}) must be >= ${kind} (${count})`
        );
      }
    }
  });
});
