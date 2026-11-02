import type {
  ThumbnailCacheSnapshotResponse,
  ThumbnailCacheStatsResponse
} from "@mangoo/core-client";
import {
  rendererErrorIdentityKey,
  type DesktopRendererErrorReport,
  type DesktopThumbnailCacheSummary
} from "@mangoo/notification-contract";
import { getDesktopBridge, type MangooDesktopBridge } from "./desktop.ts";

const LIBRARY_ID_MAX_LENGTH = 256;
const LIBRARY_NAME_MAX_LENGTH = 256;
const REPORT_ID_MAX_LENGTH = 256;
const ERROR_SOURCE_MAX_LENGTH = 128;
const ERROR_CODE_MAX_LENGTH = 128;
const ERROR_TARGET_MAX_LENGTH = 512;
const ERROR_MESSAGE_MAX_LENGTH = 4_000;

export interface NotificationLibraryContext {
  libraryId: string;
  libraryName: string;
}

export interface LibraryErrorReportAttempt extends NotificationLibraryContext {
  reportId: string;
}

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
  message: string;
}

interface AttemptState {
  completed: boolean;
  inFlight: Promise<boolean> | null;
  normalized: Pick<DesktopRendererErrorReport, "source" | "code" | "target" | "message"> | null;
}

const attemptStates = new WeakMap<LibraryErrorReportAttempt, AttemptState>();
let reportSequence = 0;
let rendererStartupLogEmitted = false;

export function emitRendererStartupLog(): boolean {
  const bridge = desktopBridge();
  const emit = bridge?.notifications?.emitRendererLogs;
  if (rendererStartupLogEmitted || !emit) return false;
  rendererStartupLogEmitted = true;
  try {
    emit([{
      level: "info",
      message: "Mangoo renderer ready",
      metadata: {
        eventKind: "startup_ready",
        windowRole: bridge.windowRole ?? "main"
      }
    }]);
    return true;
  } catch {
    rendererStartupLogEmitted = false;
    return false;
  }
}

export class ReportedLibraryFailure extends Error {
  readonly attempt: LibraryErrorReportAttempt | null;
  readonly originalCause: unknown;

  constructor(attempt: LibraryErrorReportAttempt | null, originalCause: unknown) {
    super(errorMessage(originalCause));
    this.name = "ReportedLibraryFailure";
    this.attempt = attempt;
    this.originalCause = originalCause;
  }
}

export function createLibraryErrorReportAttempt(
  context: NotificationLibraryContext | null | undefined,
  reportId?: string
): LibraryErrorReportAttempt | null {
  if (!context) return null;
  const libraryId = boundedString(context.libraryId, LIBRARY_ID_MAX_LENGTH);
  const libraryName = boundedString(context.libraryName, LIBRARY_NAME_MAX_LENGTH);
  const boundedReportId =
    reportId === undefined ? null : boundedString(reportId, REPORT_ID_MAX_LENGTH);
  if (!libraryId || !libraryName || (reportId !== undefined && !boundedReportId)) return null;
  let logicalReportId = boundedReportId;
  const attempt = Object.freeze({
    libraryId,
    libraryName,
    get reportId() {
      logicalReportId ??= createReportId();
      return logicalReportId;
    }
  });
  attemptStates.set(attempt, { completed: false, inFlight: null, normalized: null });
  return attempt;
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
  return rendererErrorIdentityKey({
    source: normalizeDedupePart(input.source),
    code: normalizeDedupePart(input.code),
    target: normalizeDedupePart(input.target)
  });
}

export function reportLibraryError(
  attempt: LibraryErrorReportAttempt | null | undefined,
  input: LibraryErrorReportInput
): Promise<boolean> {
  if (!attempt) return Promise.resolve(false);
  const normalized = normalizeLibraryError(input);
  if (!normalized) return Promise.resolve(false);
  const state = attemptStates.get(attempt);
  if (!state) return Promise.resolve(false);
  if (state.completed) return Promise.resolve(true);
  state.normalized ??= normalized;
  if (state.inFlight) return state.inFlight;

  const bridge = desktopBridge();
  const notifications = bridge?.notifications;
  if (!bridge || !notifications?.reportError) return Promise.resolve(false);

  const report: DesktopRendererErrorReport = {
    reportId: attempt.reportId,
    libraryId: attempt.libraryId,
    libraryName: attempt.libraryName,
    ...state.normalized,
    title: rendererErrorTitle(state.normalized.source),
    detail: null
  };
  const request = Promise.resolve()
    .then(() => notifications.reportError(report))
    .then((accepted) => {
      if (accepted === true) state.completed = true;
      return accepted === true;
    })
    .catch(() => false);
  state.inFlight = request.finally(() => {
    state.inFlight = null;
  });
  return state.inFlight;
}

export async function reportLibraryErrorAndRethrow(
  attempt: LibraryErrorReportAttempt | null | undefined,
  input: LibraryErrorReportInput,
  cause: unknown
): Promise<never> {
  if (cause instanceof ReportedLibraryFailure) {
    await retryLibraryErrorReport(cause);
    throw cause;
  }
  await reportLibraryError(attempt, input);
  throw new ReportedLibraryFailure(attempt ?? null, cause);
}

export function retryLibraryErrorReport(cause: unknown): Promise<boolean> {
  if (!(cause instanceof ReportedLibraryFailure)) return Promise.resolve(false);
  if (!cause.attempt) return Promise.resolve(false);
  const state = attemptStates.get(cause.attempt);
  if (!state?.normalized) return Promise.resolve(false);
  return reportLibraryError(cause.attempt, state.normalized);
}

export function publishThumbnailCacheStats(
  stats: ThumbnailCacheSnapshotResponse,
  context: NotificationLibraryContext | null | undefined,
  observedAt = Date.now()
): Promise<boolean> {
  const bridge = desktopBridge();
  const notifications = bridge?.notifications;
  if (
    !bridge ||
    (bridge.windowRole !== undefined && bridge.windowRole !== "main" && bridge.windowRole !== "settings") ||
    !notifications?.publishThumbnailSummary ||
    !context
  ) {
    return Promise.resolve(false);
  }
  const libraryId = boundedString(context.libraryId, LIBRARY_ID_MAX_LENGTH);
  const libraryName = boundedString(context.libraryName, LIBRARY_NAME_MAX_LENGTH);
  if (!libraryId || !libraryName) return Promise.resolve(false);
  const failureSnapshotConsistent = stats.failureDetailsTruncated
    ? stats.failureDetails.length === 100 && stats.failedCount > stats.failureDetails.length
    : stats.failedCount === stats.failureDetails.length;
  if (!failureSnapshotConsistent) {
    return Promise.resolve(false);
  }

  const snapshot = thumbnailSummary(stats, libraryId, libraryName, observedAt);
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
    completionPercent: snapshot.percent,
    failureDetailsTruncated: stats.failureDetailsTruncated,
    failureDetailsUnavailable: false,
    failureDetails: stats.failureDetails.slice(0, 100).map((detail) => ({
      libraryId,
      ...detail
    }))
  };
  return Promise.resolve()
    .then(() => notifications.publishThumbnailSummary(summary))
    .then((accepted) => accepted === true)
    .catch(() => false);
}

function desktopBridge(): MangooDesktopBridge | null {
  if (typeof window === "undefined") return null;
  return getDesktopBridge();
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

function rendererErrorTitle(source: string): string {
  const words = source.replace(/[-_]+/g, " ");
  return `${words.charAt(0).toUpperCase()}${words.slice(1)} failed`;
}

function boundedString(value: unknown, maxLength: number): string | null {
  if (typeof value !== "string") return null;
  const normalized = value.trim();
  return normalized.length > 0 && normalized.length <= maxLength ? normalized : null;
}

function createReportId(): string {
  const cryptoObject = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  const uuid = cryptoObject?.randomUUID?.();
  if (uuid) return uuid;
  reportSequence += 1;
  return `renderer-${Date.now().toString(36)}-${reportSequence.toString(36)}`;
}

function errorMessage(cause: unknown): string {
  if (cause instanceof Error && cause.message) return cause.message;
  return typeof cause === "string" ? cause : String(cause);
}
