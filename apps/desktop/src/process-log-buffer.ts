import { Buffer } from "node:buffer";

export const DEFAULT_PROCESS_LOG_MAX_ENTRIES = 5_000;
export const DEFAULT_PROCESS_LOG_MAX_BYTES = 5 * 1024 * 1024;

export type ProcessLogSource = "main" | "core" | "renderer";
export type ProcessLogLevel = "trace" | "debug" | "info" | "warn" | "error";

export interface ProcessLogMetadata {
  readonly [key: string]: unknown;
}

export interface ProcessLogLine {
  readonly sequence: number;
  readonly timestamp: number;
  readonly source: ProcessLogSource;
  readonly level: ProcessLogLevel;
  readonly message: string;
  readonly metadata?: ProcessLogMetadata;
}

export interface ProcessLogFilter {
  readonly source?: ProcessLogSource;
  readonly sources?: readonly ProcessLogSource[];
  readonly level?: ProcessLogLevel;
  readonly levels?: readonly ProcessLogLevel[];
  readonly search?: string;
  readonly afterSequence?: number;
  readonly beforeSequence?: number;
  readonly limit?: number;
}

export interface ProcessLogPage {
  readonly lines: readonly ProcessLogLine[];
  readonly hasMore: boolean;
  readonly nextBeforeSequence: number | null;
}

export interface ProcessLogBufferOptions {
  readonly maxEntries?: number;
  readonly maxBytes?: number;
  readonly now?: () => number;
}

type StoredLine = ProcessLogLine & { readonly bytes: number };
type ProcessLogSubscriber = (line: ProcessLogLine) => void;

const MAX_MESSAGE_BYTES = 16 * 1024;
const MAX_METADATA_DEPTH = 6;
const MAX_METADATA_KEYS = 128;
const MAX_METADATA_ARRAY_ITEMS = 128;
const MAX_METADATA_STRING_LENGTH = 4 * 1024;
const REDACTED = "[redacted]";

const SECRET_TEXT_PATTERN =
  /((?:MEGLE_SESSION_TOKEN|SESSION_TOKEN|SESSIONTOKEN|X-MEGLE-SESSION|AUTHORIZATION|BEARER|TOKEN)\s*[:=]\s*)([^\s,;]+)/gi;
const WINDOWS_PATH_PATTERN = /(?:[A-Za-z]:[\\/]|\\\\)[^\s"'`<>]+/g;
const UNIX_SENSITIVE_PATH_PATTERN = /(?:\/Users\/|\/home\/|\/private\/var\/|\/tmp\/)[^\s"'`<>]+/gi;
const SENSITIVE_METADATA_KEY_PATTERN =
  /(?:token|secret|password|authorization|cookie|(?:^|[_-])path$|(?:^|[_-])filepath$|(?:^|[_-])directory$|(?:^|[_-])rootpath$)/i;

export function sanitizeProcessLogMessage(value: string): string {
  let message = value.replace(/\r\n?/g, "\\n");
  message = message.replace(SECRET_TEXT_PATTERN, `$1${REDACTED}`);
  message = message.replace(WINDOWS_PATH_PATTERN, REDACTED);
  message = message.replace(UNIX_SENSITIVE_PATH_PATTERN, REDACTED);
  return truncateUtf8(message, MAX_MESSAGE_BYTES);
}

export function sanitizeProcessLogMetadata(value: unknown): ProcessLogMetadata | undefined {
  if (value === undefined) return undefined;
  const seen = new WeakSet<object>();
  const normalized = sanitizeMetadataValue(value, "metadata", seen, 0, { keys: 0 });
  if (normalized === undefined) return undefined;
  if (isRecord(normalized)) return normalized;
  return { value: normalized };
}

export function formatProcessLogValue(value: unknown): string {
  if (value instanceof Error) {
    return `${value.name}: ${value.message}`;
  }
  if (typeof value === "string") return value;
  if (value === null) return "null";
  if (value === undefined) return "undefined";
  if (typeof value === "bigint") return `${value.toString()}n`;
  if (typeof value !== "object") return String(value);

  try {
    const serialized = JSON.stringify(value, createSafeJsonReplacer());
    return serialized === undefined ? String(value) : serialized;
  } catch {
    return "[unserializable]";
  }
}

export class ProcessLogBuffer {
  private readonly maxEntries: number;
  private readonly maxBytes: number;
  private readonly now: () => number;
  private readonly lines: StoredLine[] = [];
  private readonly subscribers = new Set<ProcessLogSubscriber>();
  private readonly flushWaiters = new Set<() => void>();
  private pendingNotifications: ProcessLogLine[] = [];
  private notificationScheduled = false;
  private totalBytes = 0;
  private nextSequence = 1;

  constructor(options: ProcessLogBufferOptions = {}) {
    this.maxEntries = positiveInteger(options.maxEntries, DEFAULT_PROCESS_LOG_MAX_ENTRIES);
    this.maxBytes = positiveInteger(options.maxBytes, DEFAULT_PROCESS_LOG_MAX_BYTES);
    this.now = options.now ?? Date.now;
  }

  append(
    source: ProcessLogSource,
    level: ProcessLogLevel,
    message: string,
    metadata?: unknown,
    timestamp = this.now()
  ): ProcessLogLine | null {
    const sequence = this.nextSequence++;
    const candidate = this.fitLine(
      {
        sequence,
        timestamp: Number.isFinite(timestamp) ? timestamp : this.now(),
        source,
        level,
        message: sanitizeProcessLogMessage(message),
        metadata: sanitizeProcessLogMetadata(metadata)
      }
    );
    if (!candidate) return null;

    this.lines.push(candidate);
    this.totalBytes += candidate.bytes;
    while (this.lines.length > this.maxEntries || this.totalBytes > this.maxBytes) {
      const removed = this.lines.shift();
      if (!removed) break;
      this.totalBytes -= removed.bytes;
    }
    this.pendingNotifications.push(stripBytes(candidate));
    this.scheduleNotificationFlush();
    return stripBytes(candidate);
  }

  snapshot(filter: ProcessLogFilter = {}): ProcessLogLine[] {
    const lines = this.lines.filter((line) => matchesFilter(line, filter));
    if (filter.limit === undefined) return lines.map(stripBytes);
    const limit = positiveInteger(filter.limit, 1);
    return lines.slice(-limit).map(stripBytes);
  }

  page(filter: ProcessLogFilter & { readonly limit?: number } = {}): ProcessLogPage {
    const limit = positiveInteger(filter.limit, 100);
    const matching = this.lines.filter((line) => matchesFilter(line, filter));
    const lines = matching.slice(-limit).map(stripBytes);
    const hasMore = matching.length > lines.length;
    return {
      lines,
      hasMore,
      nextBeforeSequence: hasMore ? (lines[0]?.sequence ?? null) : null
    };
  }

  subscribe(subscriber: ProcessLogSubscriber): () => void {
    this.subscribers.add(subscriber);
    return () => this.subscribers.delete(subscriber);
  }

  clear(): void {
    this.lines.length = 0;
    this.totalBytes = 0;
  }

  get size(): number {
    return this.lines.length;
  }

  get bytes(): number {
    return this.totalBytes;
  }

  async flush(): Promise<void> {
    if (!this.notificationScheduled && this.pendingNotifications.length === 0) return;
    await new Promise<void>((resolve) => {
      this.flushWaiters.add(resolve);
      this.scheduleNotificationFlush();
    });
  }

  async shutdown(): Promise<void> {
    await this.flush();
    this.subscribers.clear();
    this.pendingNotifications.length = 0;
    for (const resolve of this.flushWaiters) resolve();
    this.flushWaiters.clear();
  }

  private fitLine(line: ProcessLogLine): StoredLine | null {
    let candidate: ProcessLogLine = line;
    let bytes = estimateLineBytes(candidate);
    if (bytes <= this.maxBytes) return { ...candidate, bytes };

    candidate = { ...candidate, metadata: undefined };
    bytes = estimateLineBytes(candidate);
    if (bytes <= this.maxBytes) return { ...candidate, bytes };

    let low = 0;
    let high = candidate.message.length;
    let best: ProcessLogLine | null = null;
    while (low <= high) {
      const middle = Math.floor((low + high) / 2);
      const shortened = { ...candidate, message: candidate.message.slice(0, middle) };
      const shortenedBytes = estimateLineBytes(shortened);
      if (shortenedBytes <= this.maxBytes) {
        best = shortened;
        low = middle + 1;
      } else {
        high = middle - 1;
      }
    }
    if (!best) return null;
    return { ...best, bytes: estimateLineBytes(best) };
  }

  private scheduleNotificationFlush(): void {
    if (this.notificationScheduled) return;
    this.notificationScheduled = true;
    queueMicrotask(() => this.flushNotifications());
  }

  private flushNotifications(): void {
    this.notificationScheduled = false;
    const pending = this.pendingNotifications;
    this.pendingNotifications = [];
    for (const line of pending) {
      for (const subscriber of this.subscribers) {
        try {
          subscriber(line);
        } catch {
          // A log consumer must never interrupt the producer or another consumer.
        }
      }
    }
    if (this.pendingNotifications.length > 0) {
      this.scheduleNotificationFlush();
      return;
    }
    for (const resolve of this.flushWaiters) resolve();
    this.flushWaiters.clear();
  }
}

function positiveInteger(value: number | undefined, fallback: number): number {
  return Number.isInteger(value) && value !== undefined && value > 0 ? value : fallback;
}

function stripBytes(line: StoredLine): ProcessLogLine {
  const { bytes: _bytes, ...publicLine } = line;
  return publicLine;
}

function estimateLineBytes(line: ProcessLogLine): number {
  return Buffer.byteLength(JSON.stringify(line), "utf8");
}

function matchesFilter(line: StoredLine, filter: ProcessLogFilter): boolean {
  if (filter.source && line.source !== filter.source) return false;
  if (filter.sources && !filter.sources.includes(line.source)) return false;
  if (filter.level && line.level !== filter.level) return false;
  if (filter.levels && !filter.levels.includes(line.level)) return false;
  if (filter.afterSequence !== undefined && line.sequence <= filter.afterSequence) return false;
  if (filter.beforeSequence !== undefined && line.sequence >= filter.beforeSequence) return false;
  if (filter.search && !line.message.toLocaleLowerCase().includes(filter.search.toLocaleLowerCase())) {
    return false;
  }
  return true;
}

function truncateUtf8(value: string, maxBytes: number): string {
  if (Buffer.byteLength(value, "utf8") <= maxBytes) return value;
  return Buffer.from(value, "utf8").subarray(0, maxBytes).toString("utf8");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function sanitizeMetadataValue(
  value: unknown,
  key: string,
  seen: WeakSet<object>,
  depth: number,
  budget: { keys: number }
): unknown {
  if (SENSITIVE_METADATA_KEY_PATTERN.test(key)) return REDACTED;
  if (value === null || typeof value === "boolean" || typeof value === "number") return value;
  if (typeof value === "string") return truncateUtf8(sanitizeProcessLogMessage(value), MAX_METADATA_STRING_LENGTH);
  if (typeof value === "bigint") return `${value.toString()}n`;
  if (typeof value === "undefined") return undefined;
  if (typeof value === "function" || typeof value === "symbol") return "[unserializable]";
  if (depth >= MAX_METADATA_DEPTH) return "[truncated]";
  if (seen.has(value)) return "[circular]";
  seen.add(value);

  if (value instanceof Error) {
    return {
      name: value.name,
      message: truncateUtf8(sanitizeProcessLogMessage(value.message), MAX_METADATA_STRING_LENGTH),
      stack: value.stack ? truncateUtf8(sanitizeProcessLogMessage(value.stack), MAX_METADATA_STRING_LENGTH) : undefined
    };
  }
  if (value instanceof Date) return value.toISOString();
  if (Array.isArray(value)) {
    return value
      .slice(0, MAX_METADATA_ARRAY_ITEMS)
      .map((item) => sanitizeMetadataValue(item, "value", seen, depth + 1, budget));
  }

  const output: Record<string, unknown> = {};
  for (const [childKey, childValue] of Object.entries(value)) {
    if (budget.keys++ >= MAX_METADATA_KEYS) {
      output.__truncated__ = true;
      break;
    }
    const normalized = sanitizeMetadataValue(childValue, childKey, seen, depth + 1, budget);
    if (normalized !== undefined) output[childKey] = normalized;
  }
  return output;
}

function createSafeJsonReplacer(): (key: string, value: unknown) => unknown {
  const seen = new WeakSet<object>();
  return (key, value) => {
    if (typeof value === "object" && value !== null) {
      if (seen.has(value)) return "[circular]";
      seen.add(value);
    }
    if (value instanceof Error) return sanitizeProcessLogMetadata(value);
    if (typeof value === "bigint") return `${value.toString()}n`;
    return value;
  };
}
