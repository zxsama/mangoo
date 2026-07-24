import type {
  DesktopNotificationCompactSnapshot,
  DesktopNotificationHistoryPage,
  DesktopNotificationOperation,
  DesktopProcessLogBatch,
  DesktopThumbnailCacheSummary,
  NotificationWindowIntent
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
  sourceOperations: DesktopNotificationOperation[];
  paths: string[];
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
  activeLibraryId: string | null;
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
  conflictError: string | null;
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

export function processLogGapSequence(batch: DesktopProcessLogBatch): number | null {
  return batch.droppedBeforeSequence;
}

export function buildCompactNotificationModel(
  snapshot: DesktopNotificationCompactSnapshot
): CompactNotificationModel {
  const activeOperations = deduplicateOperations(snapshot.active)
    .filter((operation) => ACTIVE_STATUSES.has(operation.status))
    .sort(compareActiveOperations);
  const active = aggregateInteractiveFolderScans(activeOperations)
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
    activeLibraryId: snapshot.syncHealth.libraryId,
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

  const deduplicated = deduplicateOperationsWithConflicts(
    [...pages.entries()]
      .sort(([left], [right]) => left - right)
      .flatMap(([, page]) => page.items)
  );
  const items = deduplicated.items.sort(compareTerminalOperations);
  const total = Math.max(0, ...[...pages.values()].map((page) => page.total));

  let nextOffset: number | null = startOffset;
  const visited = new Set<number>();
  while (nextOffset !== null && !visited.has(nextOffset)) {
    visited.add(nextOffset);
    const page = pages.get(nextOffset);
    if (!page) break;
    nextOffset = page.nextOffset;
  }

  return {
    items,
    total,
    nextOffset,
    conflictError:
      deduplicated.conflicts.length > 0
        ? `Notification history conflict for ${deduplicated.conflicts[0]}`
        : null
  };
}

function operationViewModel(
  operation: DesktopNotificationOperation
): NotificationOperationViewModel {
  const presentation = operation as DesktopNotificationOperation & {
    sourceOperations?: DesktopNotificationOperation[];
    paths?: string[];
  };
  return {
    ...operation,
    libraryLabel: operation.libraryName || operation.libraryId,
    progress: progressView(operation),
    sourceOperations: presentation.sourceOperations ?? [operation],
    paths: presentation.paths ?? (
      operation.kind === "interactive_folder_scan" && operationSummary(operation)
        ? [operationSummary(operation)]
        : []
    )
  };
}

function aggregateInteractiveFolderScans(
  operations: readonly DesktopNotificationOperation[]
): DesktopNotificationOperation[] {
  const scans = operations
    .filter((operation) => operation.kind === "interactive_folder_scan")
    .sort((left, right) =>
      operationSummary(left).localeCompare(operationSummary(right)) || compareActiveOperations(left, right)
    );
  if (scans.length <= 1) return [...operations];
  const currentValue = scans.reduce((total, operation) => total + operation.currentValue, 0);
  const determinate = scans.every(
    (operation) => operation.totalValue !== null && operation.totalValue > 0
  );
  const totalValue = determinate
    ? scans.reduce((total, operation) => total + (operation.totalValue ?? 0), 0)
    : null;
  const libraries = new Map(scans.map((operation) => [operation.libraryId, operation.libraryName]));
  const base = scans[0]!;
  const aggregate = {
    ...base,
    operationId: "presentation:interactive-folder-scans",
    libraryId: libraries.size === 1 ? base.libraryId : "presentation:multiple-libraries",
    libraryName: libraries.size === 1 ? base.libraryName : `${libraries.size} 个资源库`,
    title: "Scan folders",
    summary: `${scans.length} folders`,
    currentValue,
    totalValue,
    cancellable: scans.some((operation) => operation.cancellable),
    updatedAt: Math.max(...scans.map((operation) => operation.updatedAt)),
    startedAt: Math.min(...scans.map((operation) => operation.startedAt)),
    sourceOperations: scans,
    paths: [...new Set(scans.map(operationSummary).filter(Boolean))]
  };
  return [
    ...operations.filter((operation) => operation.kind !== "interactive_folder_scan"),
    aggregate
  ].sort(compareActiveOperations);
}

function operationSummary(operation: DesktopNotificationOperation): string {
  return operation.summary?.trim() ?? "";
}

export function thumbnailFailureLogIntent(
  summary: DesktopThumbnailCacheSummary
): NotificationWindowIntent | null {
  return thumbnailFailureLogIntentForLibrary(summary.libraryId, summary.failedCount);
}

export function thumbnailFailureLogIntentForLibrary(
  libraryId: string | null | undefined,
  failedCount: number
): NotificationWindowIntent | null {
  if (!libraryId || failedCount <= 0) return null;
  return {
    page: "logs",
    logFilter: {
      eventKind: "thumbnail_failure",
      libraryId
    }
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
    .sort(compareThumbnailLibraries);
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

function compareThumbnailLibraries(
  left: DesktopThumbnailCacheSummary,
  right: DesktopThumbnailCacheSummary
): number {
  return left.libraryId < right.libraryId ? -1 : left.libraryId > right.libraryId ? 1 : 0;
}

function deduplicateOperations(
  input: readonly DesktopNotificationOperation[]
): DesktopNotificationOperation[] {
  return deduplicateOperationsWithConflicts(input).items;
}

function deduplicateOperationsWithConflicts(
  input: readonly DesktopNotificationOperation[]
): { items: DesktopNotificationOperation[]; conflicts: string[] } {
  const operations = new Map<string, DesktopNotificationOperation>();
  const conflicts = new Set<string>();
  for (const operation of input) {
    const key = `${operation.libraryId}\u0000${operation.operationId}`;
    const current = operations.get(key);
    if (!current || operation.revision > current.revision) {
      operations.set(key, operation);
      continue;
    }
    if (
      operation.revision === current.revision &&
      !sameNotificationOperationPayload(current, operation)
    ) {
      conflicts.add(
        `${operation.libraryId}/${operation.operationId} revision ${operation.revision}`
      );
    }
  }
  return { items: [...operations.values()], conflicts: [...conflicts] };
}

function sameNotificationOperationPayload(
  left: DesktopNotificationOperation,
  right: DesktopNotificationOperation
): boolean {
  const { libraryId: _leftLibraryId, libraryName: _leftLibraryName, ...leftPayload } = left;
  const { libraryId: _rightLibraryId, libraryName: _rightLibraryName, ...rightPayload } = right;
  return JSON.stringify(leftPayload) === JSON.stringify(rightPayload);
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
