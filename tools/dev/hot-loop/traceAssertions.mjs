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
