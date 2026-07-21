import type {
  DesktopProcessLogBatch,
  DesktopProcessLogFilter,
  DesktopProcessLogLine
} from "@megle/notification-contract";

export const MAX_PAUSED_LOG_LINES = 1_000;

export interface PausedProcessLogBuffer {
  batches: readonly DesktopProcessLogBatch[];
  lineCount: number;
  droppedBeforeSequence: number | null;
}

export interface ProcessLogQueryToken {
  readonly identity: string;
}

export interface ProcessLogPageRequest {
  readonly queryToken: ProcessLogQueryToken;
  readonly generation: number;
  readonly requestId: number;
}

export interface ProcessLogPageRequestController {
  begin(queryToken: ProcessLogQueryToken): ProcessLogPageRequest | null;
  commitQuery(queryToken: ProcessLogQueryToken): void;
  finish(request: ProcessLogPageRequest): boolean;
  invalidate(): void;
  isCurrent(request: ProcessLogPageRequest): boolean;
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

export function createProcessLogQueryToken(
  filter: DesktopProcessLogFilter
): ProcessLogQueryToken {
  return Object.freeze({ identity: processLogFilterIdentity(filter) });
}

export function createProcessLogPageRequestController(
  initialQueryToken: ProcessLogQueryToken
): ProcessLogPageRequestController {
  let committedQueryToken = initialQueryToken;
  let generation = 0;
  let nextRequestId = 0;
  let activeRequest: ProcessLogPageRequest | null = null;
  return {
    begin(queryToken) {
      if (activeRequest) return null;
      activeRequest = Object.freeze({
        queryToken,
        generation,
        requestId: ++nextRequestId
      });
      return activeRequest;
    },
    commitQuery(queryToken) {
      if (queryToken === committedQueryToken) return;
      committedQueryToken = queryToken;
      generation += 1;
      activeRequest = null;
    },
    finish(request) {
      if (activeRequest !== request) return false;
      activeRequest = null;
      return true;
    },
    invalidate() {
      generation += 1;
      activeRequest = null;
    },
    isCurrent(request) {
      return (
        activeRequest === request &&
        request.generation === generation &&
        request.queryToken === committedQueryToken
      );
    }
  };
}

export function createPausedProcessLogBuffer(): PausedProcessLogBuffer {
  return { batches: [], lineCount: 0, droppedBeforeSequence: null };
}

export function appendPausedProcessLogBatch(
  current: PausedProcessLogBuffer,
  batch: DesktopProcessLogBatch
): PausedProcessLogBuffer {
  const batches = [...current.batches, batch];
  let lineCount = current.lineCount + batch.lines.length;
  let droppedBeforeSequence = maxNullableSequence(
    current.droppedBeforeSequence,
    batch.droppedBeforeSequence
  );
  let excess = Math.max(0, lineCount - MAX_PAUSED_LOG_LINES);
  while (excess > 0 && batches.length > 0) {
    const first = batches[0];
    if (!first || first.lines.length === 0) {
      batches.shift();
      continue;
    }
    const removeCount = Math.min(excess, first.lines.length);
    droppedBeforeSequence = maxNullableSequence(
      droppedBeforeSequence,
      first.lines[removeCount - 1]?.sequence ?? null
    );
    lineCount -= removeCount;
    excess -= removeCount;
    if (removeCount === first.lines.length) batches.shift();
    else batches[0] = { ...first, lines: first.lines.slice(removeCount) };
  }
  return { batches, lineCount, droppedBeforeSequence };
}

export function coalescePausedProcessLogBuffer(
  buffer: PausedProcessLogBuffer
): DesktopProcessLogBatch {
  return coalesceProcessLogBatches(
    buffer.batches,
    MAX_PAUSED_LOG_LINES,
    buffer.droppedBeforeSequence
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
    droppedBeforeSequence = maxNullableSequence(
      droppedBeforeSequence,
      batch.droppedBeforeSequence
    );
    for (const line of batch.lines) bySequence.set(line.sequence, line);
  }
  const lines = [...bySequence.values()]
    .sort((left, right) => left.sequence - right.sequence)
    .slice(-Math.max(0, maxLines));
  return { lines, afterSequence, droppedBeforeSequence };
}

function maxNullableSequence(left: number | null, right: number | null): number | null {
  if (left === null) return right;
  if (right === null) return left;
  return Math.max(left, right);
}
