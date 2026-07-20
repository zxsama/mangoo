import type {
  DesktopNotificationCompactSnapshot,
  DesktopNotificationHistoryPage,
  DesktopNotificationOperation,
  DesktopThumbnailCacheSummary
} from "@megle/notification-contract";

export type NotificationIndicator = "idle" | "active" | "failure";

export interface ProgressViewModel {
  determinate: boolean;
  percent: number | null;
  label: string;
}

export type NotificationOperationViewModel = DesktopNotificationOperation & {
  libraryLabel: string;
  progress: ProgressViewModel;
};

export interface ThumbnailAggregateViewModel {
  summaries: DesktopThumbnailCacheSummary[];
  libraryCount: number;
  cachedCount: number;
  totalCandidateCount: number;
  missingCount: number;
  staleCount: number;
  failedCount: number;
  pendingCount: number;
  activeBulkTaskCount: number;
  totalCacheBytes: number | null;
  completionPercent: number | null;
  active: boolean;
}

export interface CompactNotificationModel {
  active: NotificationOperationViewModel[];
  thumbnail: ThumbnailAggregateViewModel;
  results: NotificationOperationViewModel[];
  indicator: NotificationIndicator;
  degraded: boolean;
  healthDetail: string | null;
}

export interface NotificationHistoryPageSlice {
  offset: number;
  page: DesktopNotificationHistoryPage;
}

export interface NotificationHistoryCollection {
  items: DesktopNotificationOperation[];
  total: number;
  nextOffset: number | null;
}

const ACTIVE_STATUSES = new Set<DesktopNotificationOperation["status"]>([
  "pending",
  "running"
]);

export function progressView(operation: DesktopNotificationOperation): ProgressViewModel {
  const total = operation.totalValue;
  if (total == null || total <= 0) {
    return {
      determinate: false,
      percent: null,
      label: operation.phase?.trim() || "处理中"
    };
  }
  const percent = Math.max(
    0,
    Math.min(100, Math.round((operation.currentValue / total) * 100))
  );
  return {
    determinate: true,
    percent,
    label: `${operation.currentValue.toLocaleString()} / ${total.toLocaleString()}`
  };
}

export function notificationIndicator(input: {
  active: boolean;
  retryableFailure: boolean;
}): NotificationIndicator {
  if (input.retryableFailure) return "failure";
  return input.active ? "active" : "idle";
}

export function buildCompactNotificationModel(
  snapshot: DesktopNotificationCompactSnapshot
): CompactNotificationModel {
  const active = deduplicateOperations(snapshot.active)
    .filter((operation) => ACTIVE_STATUSES.has(operation.status))
    .sort(compareActiveOperations)
    .map(operationViewModel);
  const results = deduplicateOperations(snapshot.latestImportantResults)
    .filter(
      (operation) =>
        !ACTIVE_STATUSES.has(operation.status) && operation.presentation === "important"
    )
    .sort(compareTerminalOperations)
    .slice(0, 3)
    .map(operationViewModel);
  return {
    active,
    thumbnail: buildThumbnailAggregate(snapshot.thumbnailSummaries),
    results,
    indicator: notificationIndicator({
      active: active.length > 0,
      retryableFailure: snapshot.hasRetryableFailure
    }),
    degraded: snapshot.syncHealth.state === "degraded",
    healthDetail: snapshot.syncHealth.detail
  };
}

export function mergeNotificationHistoryPages(
  slices: readonly NotificationHistoryPageSlice[],
  startOffset = 0
): NotificationHistoryCollection {
  const pages = new Map<number, DesktopNotificationHistoryPage>();
  for (const slice of slices) {
    if (!Number.isSafeInteger(slice.offset) || slice.offset < 0) continue;
    pages.set(slice.offset, slice.page);
  }

  const items = deduplicateOperations(
    [...pages.entries()]
      .sort(([left], [right]) => left - right)
      .flatMap(([, page]) => page.items)
  ).sort(compareTerminalOperations);
  const total = Math.max(0, ...[...pages.values()].map((page) => page.total));

  let nextOffset: number | null = startOffset;
  const visited = new Set<number>();
  while (nextOffset !== null && !visited.has(nextOffset)) {
    visited.add(nextOffset);
    const page = pages.get(nextOffset);
    if (!page) break;
    nextOffset = page.nextOffset;
  }

  return { items, total, nextOffset };
}

function operationViewModel(
  operation: DesktopNotificationOperation
): NotificationOperationViewModel {
  return {
    ...operation,
    libraryLabel: operation.libraryName || operation.libraryId,
    progress: progressView(operation)
  };
}

function buildThumbnailAggregate(
  input: readonly DesktopThumbnailCacheSummary[]
): ThumbnailAggregateViewModel {
  const byLibrary = new Map<string, DesktopThumbnailCacheSummary>();
  for (const summary of input) {
    const current = byLibrary.get(summary.libraryId);
    if (!current || summary.observedAt >= current.observedAt) {
      byLibrary.set(summary.libraryId, summary);
    }
  }
  const summaries = [...byLibrary.values()]
    .sort(
      (left, right) =>
        left.libraryName.localeCompare(right.libraryName) ||
        left.libraryId.localeCompare(right.libraryId)
    );
  const sum = (pick: (summary: DesktopThumbnailCacheSummary) => number) =>
    summaries.reduce((total, summary) => total + pick(summary), 0);
  const cachedCount = sum((summary) => summary.cachedCount);
  const totalCandidateCount = sum((summary) => summary.totalCandidateCount);
  const pendingCount = sum((summary) => summary.pendingCount);
  const activeBulkTaskCount = sum((summary) => summary.activeBulkTaskCount);

  return {
    summaries,
    libraryCount: summaries.length,
    cachedCount,
    totalCandidateCount,
    missingCount: sum((summary) => summary.missingCount),
    staleCount: sum((summary) => summary.staleCount),
    failedCount: sum((summary) => summary.failedCount),
    pendingCount,
    activeBulkTaskCount,
    totalCacheBytes:
      summaries.length > 0 && summaries.every((summary) => summary.totalCacheBytes !== null)
        ? sum((summary) => summary.totalCacheBytes ?? 0)
        : null,
    completionPercent:
      totalCandidateCount > 0
        ? Math.max(0, Math.min(100, Math.round((cachedCount / totalCandidateCount) * 100)))
        : null,
    active: pendingCount > 0 || activeBulkTaskCount > 0
  };
}

function deduplicateOperations(
  input: readonly DesktopNotificationOperation[]
): DesktopNotificationOperation[] {
  const operations = new Map<string, DesktopNotificationOperation>();
  for (const operation of input) {
    const key = `${operation.libraryId}\u0000${operation.operationId}`;
    const current = operations.get(key);
    if (
      !current ||
      operation.revision > current.revision ||
      (operation.revision === current.revision && operation.updatedAt > current.updatedAt)
    ) {
      operations.set(key, operation);
    }
  }
  return [...operations.values()];
}

function compareActiveOperations(
  left: DesktopNotificationOperation,
  right: DesktopNotificationOperation
): number {
  return (
    right.updatedAt - left.updatedAt ||
    left.libraryId.localeCompare(right.libraryId) ||
    left.operationId.localeCompare(right.operationId)
  );
}

function compareTerminalOperations(
  left: DesktopNotificationOperation,
  right: DesktopNotificationOperation
): number {
  return (
    (right.completedAt ?? right.updatedAt) - (left.completedAt ?? left.updatedAt) ||
    right.updatedAt - left.updatedAt ||
    left.libraryId.localeCompare(right.libraryId) ||
    left.operationId.localeCompare(right.operationId)
  );
}
