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
