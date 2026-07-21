import type { ThumbnailCacheStatsResponse } from "@megle/core-client";
import type {
  DesktopRendererErrorReport,
  DesktopThumbnailCacheSummary
} from "@megle/notification-contract";
import { getDesktopBridge, type MegleDesktopBridge } from "./desktop.ts";

const LIBRARY_ID_MAX_LENGTH = 256;
const LIBRARY_NAME_MAX_LENGTH = 256;
const ERROR_SOURCE_MAX_LENGTH = 128;
const ERROR_CODE_MAX_LENGTH = 128;
const ERROR_TARGET_MAX_LENGTH = 512;
const ERROR_MESSAGE_MAX_LENGTH = 4_000;
const inFlightRendererErrorKeys = new Set<string>();
// Preserve the first operation boundary when the same failure propagates through nested catches.
const reportedRendererErrorCauses = new WeakSet<object>();

export interface ThumbnailSummarySnapshot {
  libraryId: string;
  libraryName: string;
  cachedCount: number;
  remainingCount: number;
  failedCount: number;
  activeBulkTaskCount: number;
  percent: number;
  observedAt: number;
}

export interface RendererErrorKeyInput {
  source: string;
  code: string;
  target: string;
}

export interface LibraryErrorReportInput extends RendererErrorKeyInput {
  cause?: unknown;
  message: string;
}

export function thumbnailSummary(
  stats: ThumbnailCacheStatsResponse,
  libraryId: string,
  libraryName: string,
  observedAt: number
): ThumbnailSummarySnapshot {
  const remainingCount = stats.missingCount + stats.staleCount;
  const totalCount = stats.cachedCount + remainingCount;
  const percent =
    totalCount > 0 ? Math.min(100, Math.round((stats.cachedCount / totalCount) * 100)) : 100;

  return {
    libraryId,
    libraryName,
    cachedCount: stats.cachedCount,
    remainingCount,
    failedCount: stats.failedCount,
    activeBulkTaskCount: stats.activeBulkTaskCount,
    percent,
    observedAt
  };
}

export function rendererErrorKey(input: RendererErrorKeyInput): string {
  return [input.source, input.code, input.target].map(normalizeDedupePart).join(":");
}

export function reportLibraryError(input: LibraryErrorReportInput): void {
  const bridge = desktopBridge();
  const notifications = bridge?.notifications;
  const normalized = normalizeLibraryError(input);
  if (!bridge || !notifications?.reportError || !normalized) return;
  const cause = errorCauseObject(input.cause);
  if (cause && reportedRendererErrorCauses.has(cause)) return;
  if (cause) reportedRendererErrorCauses.add(cause);
  const dedupeKey = rendererErrorKey(normalized);
  if (inFlightRendererErrorKeys.has(dedupeKey)) return;
  inFlightRendererErrorKeys.add(dedupeKey);

  void (async () => {
    try {
      const library = await activeLibraryIdentity(bridge);
      if (!library) return;
      const report: DesktopRendererErrorReport = {
        ...library,
        ...normalized,
        title: rendererErrorTitle(normalized.source),
        detail: null
      };
      await notifications.reportError(report);
    } finally {
      inFlightRendererErrorKeys.delete(dedupeKey);
    }
  })().catch(() => undefined);
}

export function publishThumbnailCacheStats(
  stats: ThumbnailCacheStatsResponse,
  observedAt = Date.now(),
  expectedLibraryId?: string | null
): void {
  const bridge = desktopBridge();
  const notifications = bridge?.notifications;
  if (!bridge || !notifications?.publishThumbnailSummary) return;

  void (async () => {
    const library = await activeLibraryIdentity(bridge);
    if (!library) return;
    const boundedExpectedLibraryId = expectedLibraryId
      ? boundedString(expectedLibraryId, LIBRARY_ID_MAX_LENGTH)
      : null;
    if (boundedExpectedLibraryId && library.libraryId !== boundedExpectedLibraryId) return;
    const snapshot = thumbnailSummary(
      stats,
      library.libraryId,
      library.libraryName,
      observedAt
    );
    const summary: DesktopThumbnailCacheSummary = {
      libraryId: snapshot.libraryId,
      libraryName: snapshot.libraryName,
      observedAt: snapshot.observedAt,
      cachedCount: snapshot.cachedCount,
      totalCandidateCount: snapshot.cachedCount + snapshot.remainingCount,
      missingCount: stats.missingCount,
      staleCount: stats.staleCount,
      failedCount: snapshot.failedCount,
      pendingCount: stats.pendingCandidateCount,
      activeBulkTaskCount: snapshot.activeBulkTaskCount,
      totalCacheBytes: stats.totalBlobBytes,
      completionPercent: snapshot.percent
    };
    await notifications.publishThumbnailSummary(summary);
  })().catch(() => undefined);
}

function desktopBridge(): MegleDesktopBridge | null {
  if (typeof window === "undefined") return null;
  return getDesktopBridge();
}

async function activeLibraryIdentity(
  bridge: MegleDesktopBridge
): Promise<{ libraryId: string; libraryName: string } | null> {
  const libraries = await bridge.listLibraries?.();
  let active = libraries?.find((library) => library.active);
  if (!active && bridge.getActiveLibraryId) {
    const activeLibraryId = await bridge.getActiveLibraryId();
    active = libraries?.find((library) => library.id === activeLibraryId);
    if (!active && activeLibraryId) {
      const libraryId = boundedString(activeLibraryId, LIBRARY_ID_MAX_LENGTH);
      return libraryId ? { libraryId, libraryName: libraryId } : null;
    }
  }
  if (!active) return null;

  const libraryId = boundedString(active.id, LIBRARY_ID_MAX_LENGTH);
  const libraryName = boundedString(active.name, LIBRARY_NAME_MAX_LENGTH);
  return libraryId && libraryName ? { libraryId, libraryName } : null;
}

function normalizeLibraryError(
  input: LibraryErrorReportInput
): Pick<DesktopRendererErrorReport, "source" | "code" | "target" | "message"> | null {
  const source = boundedString(input.source, ERROR_SOURCE_MAX_LENGTH);
  const code = boundedString(input.code, ERROR_CODE_MAX_LENGTH);
  const target = boundedString(input.target, ERROR_TARGET_MAX_LENGTH);
  const message = boundedString(input.message, ERROR_MESSAGE_MAX_LENGTH);
  if (!source || !code || !target || !message) return null;
  return {
    source: normalizeDedupePart(source),
    code: normalizeDedupePart(code),
    target: normalizeDedupePart(target),
    message
  };
}

function normalizeDedupePart(value: string): string {
  return value.trim().toLowerCase().replace(/\s+/g, "-");
}

function errorCauseObject(cause: unknown): object | null {
  return typeof cause === "object" && cause !== null ? cause : null;
}

function rendererErrorTitle(source: string): string {
  const words = source.replace(/[-_]+/g, " ");
  return `${words.charAt(0).toUpperCase()}${words.slice(1)} failed`;
}

function boundedString(value: string, maxLength: number): string | null {
  const normalized = value.trim();
  return normalized.length > 0 && normalized.length <= maxLength ? normalized : null;
}
