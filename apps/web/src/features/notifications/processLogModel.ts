import type {
  DesktopProcessLogBatch,
  DesktopProcessLogFilter,
  DesktopProcessLogLine
} from "@megle/notification-contract";

export interface ProcessLogPageRequestIdentity {
  queryIdentity: string;
  generation: number;
}

export function processLogFilterIdentity(filter: DesktopProcessLogFilter): string {
  return JSON.stringify({
    source: filter.source ?? null,
    sources: [...(filter.sources ?? [])].sort(),
    level: filter.level ?? null,
    levels: [...(filter.levels ?? [])].sort(),
    search: filter.search?.trim() ?? "",
    afterSequence: filter.afterSequence ?? null,
    beforeSequence: filter.beforeSequence ?? null,
    limit: filter.limit ?? null
  });
}

export function isCurrentProcessLogPageRequest(
  request: ProcessLogPageRequestIdentity,
  current: ProcessLogPageRequestIdentity
): boolean {
  return (
    request.generation === current.generation &&
    request.queryIdentity === current.queryIdentity
  );
}

export function coalesceProcessLogBatches(
  batches: readonly DesktopProcessLogBatch[],
  maxLines: number,
  additionalDroppedBeforeSequence: number | null = null
): DesktopProcessLogBatch {
  const bySequence = new Map<number, DesktopProcessLogLine>();
  let afterSequence = 0;
  let droppedBeforeSequence = additionalDroppedBeforeSequence;
  for (const batch of batches) {
    afterSequence = Math.max(afterSequence, batch.afterSequence);
    if (batch.droppedBeforeSequence !== null) {
      droppedBeforeSequence = Math.max(
        droppedBeforeSequence ?? batch.droppedBeforeSequence,
        batch.droppedBeforeSequence
      );
    }
    for (const line of batch.lines) bySequence.set(line.sequence, line);
  }
  const lines = [...bySequence.values()]
    .sort((left, right) => left.sequence - right.sequence)
    .slice(-Math.max(0, maxLines));
  return { lines, afterSequence, droppedBeforeSequence };
}
