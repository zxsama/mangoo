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

export interface ProcessLogBatch {
  readonly lines: readonly ProcessLogLine[];
  /** Cursor to use for the next batch, including filtered-out retained lines. */
  readonly afterSequence: number;
  /** First retained sequence when older entries were evicted before this batch. */
  readonly droppedBeforeSequence: number | null;
}

export interface ProcessLogSubscribeOptions {
  readonly afterSequence?: number;
  readonly filter?: ProcessLogFilter;
}

export interface ProcessLogSnapshotSubscription {
  readonly snapshot: readonly ProcessLogLine[];
  readonly afterSequence: number;
  readonly unsubscribe: () => void;
}

export interface ProcessLogBufferOptions {
  readonly maxEntries?: number;
  readonly maxBytes?: number;
  readonly now?: () => number;
}

type StoredLine = { readonly line: ProcessLogLine; readonly bytes: number };
type ProcessLogSubscriber = (batch: ProcessLogBatch) => void | Promise<void>;
type SubscriberState = {
  readonly subscriber: ProcessLogSubscriber;
  readonly filter: ProcessLogFilter;
  afterSequence: number;
  generation: number;
  delivering: boolean;
  active: boolean;
};
type SanitizeContext = { readonly seen: WeakSet<object>; keys: number };

const MAX_MESSAGE_BYTES = 16 * 1024;
const MAX_METADATA_DEPTH = 6;
const MAX_METADATA_KEYS = 128;
const MAX_METADATA_ARRAY_ITEMS = 128;
const MAX_METADATA_STRING_LENGTH = 4 * 1024;
const REDACTED = "[redacted]";
const UNREADABLE = "[unreadable]";
const ACCESSOR = "[accessor]";

const SENSITIVE_TEXT_KEY_SOURCE = String.raw`(?:megle[-_ ]?session(?:[-_ ]?token)?|x[-_ ]?megle[-_ ]?session(?:[-_ ]?token)?|x[-_ ]?session[-_ ]?token|session[-_ ]?token|access[-_ ]?token|refresh[-_ ]?token|auth[-_ ]?token|id[-_ ]?token|csrf[-_ ]?token|client[-_ ]?secret|token|auth|authorization|proxy[-_ ]?authorization|bearer|secret|password|cookie|set[-_ ]?cookie|api[-_ ]?key|x[-_ ]?api[-_ ]?key)`;
const SECRET_JSON_DOUBLE_PATTERN = new RegExp(
  `("(?:${SENSITIVE_TEXT_KEY_SOURCE})"\\s*:\\s*")([^"]*)(")`,
  "gi"
);
const SECRET_JSON_SINGLE_PATTERN = new RegExp(
  `('(?:${SENSITIVE_TEXT_KEY_SOURCE})'\\s*:\\s*')([^']*)(')`,
  "gi"
);
const SECRET_HEADER_PATTERN = new RegExp(
  `(^|\\r?\\n)(\\s*(?:${SENSITIVE_TEXT_KEY_SOURCE})\\s*:\\s*)([^\\r\\n]*)`,
  "gim"
);
const SECRET_ASSIGNMENT_PATTERN = new RegExp(
  `((?:${SENSITIVE_TEXT_KEY_SOURCE})\\s*=\\s*)([^\\s,;]+)`,
  "gi"
);
const BEARER_TOKEN_PATTERN = /\b(Bearer\s+)([^\s,;]+)/gi;
const WINDOWS_PATH_PATTERN = /(?:[A-Za-z]:[\\/]|\\\\)[^\s"'`<>]+/g;
const UNIX_SENSITIVE_PATH_PATTERN = /(?:\/Users\/|\/home\/|\/private\/var\/|\/tmp\/)[^\s"'`<>]+/gi;
const SENSITIVE_NORMALIZED_KEYS = new Set([
  "meglesession",
  "meglesessiontoken",
  "xmeglesession",
  "xmeglesessiontoken",
  "xsessiontoken",
  "sessiontoken",
  "accesstoken",
  "refreshtoken",
  "authtoken",
  "idtoken",
  "csrftoken",
  "clientsecret",
  "token",
  "auth",
  "authorization",
  "proxyauthorization",
  "bearer",
  "secret",
  "password",
  "cookie",
  "setcookie",
  "apikey",
  "xapikey"
]);
const SENSITIVE_PATH_KEYS = new Set(["path", "filepath", "directory", "rootpath"]);

export function sanitizeProcessLogMessage(value: string): string {
  let message = value;
  message = message.replace(SECRET_JSON_DOUBLE_PATTERN, `$1${REDACTED}$3`);
  message = message.replace(SECRET_JSON_SINGLE_PATTERN, `$1${REDACTED}$3`);
  message = message.replace(SECRET_HEADER_PATTERN, `$1$2${REDACTED}`);
  message = message.replace(BEARER_TOKEN_PATTERN, `$1${REDACTED}`);
  message = message.replace(SECRET_ASSIGNMENT_PATTERN, `$1${REDACTED}`);
  message = message.replace(WINDOWS_PATH_PATTERN, REDACTED);
  message = message.replace(UNIX_SENSITIVE_PATH_PATTERN, REDACTED);
  message = message.replace(/\r\n?/g, "\\n");
  return truncateUtf8(message, MAX_MESSAGE_BYTES);
}

export function sanitizeProcessLogValue(value: unknown): unknown {
  try {
    return sanitizeMetadataValue(value, "value", { seen: new WeakSet<object>(), keys: 0 }, 0);
  } catch {
    return UNREADABLE;
  }
}

export function sanitizeProcessLogMetadata(value: unknown): ProcessLogMetadata | undefined {
  if (value === undefined) return undefined;
  const normalized = sanitizeProcessLogValue(value);
  if (normalized === undefined) return undefined;
  if (isRecord(normalized)) return normalized;
  return { value: normalized };
}

export function safeSerializeProcessLogValue(value: unknown): string {
  const normalized = sanitizeProcessLogValue(value);
  try {
    const serialized = JSON.stringify(normalized);
    return serialized === undefined ? UNREADABLE : serialized;
  } catch {
    return UNREADABLE;
  }
}

export function formatProcessLogValue(value: unknown): string {
  if (typeof value === "string") return sanitizeProcessLogMessage(value);
  if (value === null) return "null";
  if (value === undefined) return "undefined";
  if (typeof value === "bigint") return `${value.toString()}n`;
  if (typeof value !== "object") return sanitizeProcessLogMessage(String(value));
  return safeSerializeProcessLogValue(value);
}

export class ProcessLogBuffer {
  private readonly maxEntries: number;
  private readonly maxBytes: number;
  private readonly now: () => number;
  private readonly ring: Array<StoredLine | undefined>;
  private readonly subscribers = new Set<SubscriberState>();
  private readonly flushWaiters = new Set<() => void>();
  private head = 0;
  private count = 0;
  private totalBytes = 0;
  private nextSequence = 1;
  private notificationScheduled = false;
  private notificationGeneration = 0;

  constructor(options: ProcessLogBufferOptions = {}) {
    this.maxEntries = positiveInteger(options.maxEntries, DEFAULT_PROCESS_LOG_MAX_ENTRIES);
    this.maxBytes = positiveInteger(options.maxBytes, DEFAULT_PROCESS_LOG_MAX_BYTES);
    this.now = options.now ?? Date.now;
    this.ring = new Array<StoredLine | undefined>(this.maxEntries);
  }

  append(
    source: ProcessLogSource,
    level: ProcessLogLevel,
    message: string,
    metadata?: unknown,
    timestamp = this.now()
  ): ProcessLogLine | null {
    if (!Number.isSafeInteger(this.nextSequence)) return null;
    const sequence = this.nextSequence++;
    let candidate: StoredLine | null;
    try {
      candidate = this.fitLine({
        sequence,
        timestamp: Number.isFinite(timestamp) ? timestamp : this.now(),
        source,
        level,
        message: sanitizeProcessLogMessage(message),
        metadata: sanitizeProcessLogMetadata(metadata)
      });
    } catch {
      candidate = this.fitLine({ sequence, timestamp: this.now(), source, level, message: UNREADABLE });
    }
    if (!candidate) return null;

    this.push(candidate);
    if (this.subscribers.size > 0) this.scheduleNotificationFlush();
    return candidate.line;
  }

  snapshot(filter: ProcessLogFilter = {}): ProcessLogLine[] {
    const matching: ProcessLogLine[] = [];
    this.forEachStored((stored) => {
      if (matchesFilter(stored.line, filter)) matching.push(stored.line);
    });
    const selected = filter.limit === undefined
      ? matching
      : matching.slice(-positiveInteger(filter.limit, 1));
    return selected.map(cloneProcessLogLine);
  }

  page(filter: ProcessLogFilter & { readonly limit?: number } = {}): ProcessLogPage {
    const limit = positiveInteger(filter.limit, 100);
    const matching: ProcessLogLine[] = [];
    this.forEachStored((stored) => {
      if (matchesFilter(stored.line, filter)) matching.push(stored.line);
    });
    const selected = matching.slice(-limit);
    return {
      lines: selected.map(cloneProcessLogLine),
      hasMore: matching.length > selected.length,
      nextBeforeSequence: matching.length > selected.length ? (selected[0]?.sequence ?? null) : null
    };
  }

  subscribe(subscriber: ProcessLogSubscriber, options: ProcessLogSubscribeOptions = {}): () => void {
    const requestedAfter = options.afterSequence;
    const afterSequence = Number.isSafeInteger(requestedAfter) && requestedAfter !== undefined && requestedAfter >= 0
      ? requestedAfter
      : this.latestSequence;
    const state: SubscriberState = {
      subscriber,
      filter: cloneProcessLogFilter(options.filter),
      afterSequence,
      generation: this.notificationGeneration,
      delivering: false,
      active: true
    };
    this.subscribers.add(state);
    if (this.latestSequence > state.afterSequence) this.scheduleNotificationFlush();
    return () => {
      state.active = false;
      this.subscribers.delete(state);
    };
  }

  snapshotAndSubscribe(
    subscriber: ProcessLogSubscriber,
    filter: ProcessLogFilter = {}
  ): ProcessLogSnapshotSubscription {
    // JavaScript execution is single-threaded here, so no append can interleave
    // between capturing this cursor and registering the subscriber.
    const afterSequence = this.latestSequence;
    const snapshot = this.snapshot(filter);
    const unsubscribe = this.subscribe(subscriber, { afterSequence, filter });
    return { snapshot, afterSequence, unsubscribe };
  }

  clear(): void {
    for (let index = 0; index < this.count; index += 1) {
      this.ring[(this.head + index) % this.maxEntries] = undefined;
    }
    this.head = 0;
    this.count = 0;
    this.totalBytes = 0;
    this.notificationGeneration += 1;
    const afterSequence = this.latestSequence;
    for (const state of this.subscribers) {
      state.generation = this.notificationGeneration;
      state.afterSequence = afterSequence;
    }
  }

  get size(): number {
    return this.count;
  }

  get bytes(): number {
    return this.totalBytes;
  }

  async flush(): Promise<void> {
    if (!this.notificationScheduled) return;
    await new Promise<void>((resolve) => {
      this.flushWaiters.add(resolve);
    });
  }

  async shutdown(): Promise<void> {
    await this.flush();
    for (const state of this.subscribers) state.active = false;
    this.subscribers.clear();
    for (const resolve of this.flushWaiters) resolve();
    this.flushWaiters.clear();
  }

  private fitLine(line: ProcessLogLine): StoredLine | null {
    let candidate = line;
    let bytes = estimateLineBytes(candidate);
    if (bytes <= this.maxBytes) return freezeStoredLine(candidate, bytes);

    candidate = { ...candidate, metadata: undefined };
    bytes = estimateLineBytes(candidate);
    if (bytes <= this.maxBytes) return freezeStoredLine(candidate, bytes);

    let low = 0;
    let high = candidate.message.length;
    let best: ProcessLogLine | null = null;
    while (low <= high) {
      const middle = Math.floor((low + high) / 2);
      const shortened = { ...candidate, message: candidate.message.slice(0, middle) };
      if (estimateLineBytes(shortened) <= this.maxBytes) {
        best = shortened;
        low = middle + 1;
      } else {
        high = middle - 1;
      }
    }
    if (!best) return null;
    return freezeStoredLine(best, estimateLineBytes(best));
  }

  private push(stored: StoredLine): void {
    while (this.count >= this.maxEntries || (this.count > 0 && this.totalBytes + stored.bytes > this.maxBytes)) {
      this.evictOldest();
    }
    const index = (this.head + this.count) % this.maxEntries;
    this.ring[index] = stored;
    this.count += 1;
    this.totalBytes += stored.bytes;
  }

  private evictOldest(): void {
    if (this.count === 0) return;
    const removed = this.ring[this.head];
    this.ring[this.head] = undefined;
    this.head = (this.head + 1) % this.maxEntries;
    this.count -= 1;
    if (removed) this.totalBytes -= removed.bytes;
  }

  private forEachStored(visitor: (stored: StoredLine) => void): void {
    for (let index = 0; index < this.count; index += 1) {
      const stored = this.ring[(this.head + index) % this.maxEntries];
      if (stored) visitor(stored);
    }
  }

  private get latestSequence(): number {
    return this.nextSequence - 1;
  }

  private scheduleNotificationFlush(): void {
    if (this.notificationScheduled || this.subscribers.size === 0) return;
    this.notificationScheduled = true;
    queueMicrotask(() => this.flushNotifications());
  }

  private flushNotifications(): void {
    this.notificationScheduled = false;
    for (const state of this.subscribers) {
      if (!state.active || state.delivering) continue;
      const batch = this.createBatch(state);
      if (!batch) continue;
      if (batch.lines.length === 0 && batch.droppedBeforeSequence === null) {
        state.afterSequence = batch.afterSequence;
        continue;
      }
      state.delivering = true;
      const generation = state.generation;
      try {
        const result = state.subscriber(batch);
        if (result && typeof result.then === "function") {
          void Promise.resolve(result).then(
            () => this.completeDelivery(state, generation, batch.afterSequence),
            () => this.completeDelivery(state, generation, batch.afterSequence)
          );
        } else {
          this.completeDelivery(state, generation, batch.afterSequence);
        }
      } catch {
        this.completeDelivery(state, generation, batch.afterSequence);
      }
    }

    if (this.hasReadySubscriber()) this.scheduleNotificationFlush();
    if (!this.notificationScheduled) {
      for (const resolve of this.flushWaiters) resolve();
      this.flushWaiters.clear();
    }
  }

  private createBatch(state: SubscriberState): ProcessLogBatch | null {
    const afterSequence = this.latestSequence;
    if (afterSequence <= state.afterSequence) return null;
    if (this.count === 0) return { lines: [], afterSequence, droppedBeforeSequence: null };

    const oldest = this.ring[this.head]?.line.sequence ?? afterSequence;
    const droppedBeforeSequence = state.afterSequence + 1 < oldest ? oldest : null;
    const lines: ProcessLogLine[] = [];
    this.forEachStored((stored) => {
      if (stored.line.sequence <= state.afterSequence) return;
      if (matchesFilter(stored.line, state.filter)) lines.push(cloneProcessLogLine(stored.line));
    });
    return { lines, afterSequence, droppedBeforeSequence };
  }

  private completeDelivery(state: SubscriberState, generation: number, afterSequence: number): void {
    if (!state.active) return;
    state.delivering = false;
    if (state.generation === generation) state.afterSequence = Math.max(state.afterSequence, afterSequence);
    if (this.latestSequence > state.afterSequence) this.scheduleNotificationFlush();
  }

  private hasReadySubscriber(): boolean {
    for (const state of this.subscribers) {
      if (state.active && !state.delivering && this.latestSequence > state.afterSequence) return true;
    }
    return false;
  }
}

function positiveInteger(value: number | undefined, fallback: number): number {
  return Number.isInteger(value) && value !== undefined && value > 0 ? value : fallback;
}

function freezeStoredLine(line: ProcessLogLine, bytes: number): StoredLine {
  const metadata = line.metadata ? deepFreeze(line.metadata) : undefined;
  const frozenLine = Object.freeze({ ...line, metadata });
  return Object.freeze({ line: frozenLine, bytes });
}

function cloneProcessLogLine(line: ProcessLogLine): ProcessLogLine {
  const metadata = line.metadata
    ? cloneSanitizedValue(line.metadata) as ProcessLogMetadata
    : undefined;
  return { ...line, metadata };
}

function cloneProcessLogFilter(filter: ProcessLogFilter | undefined): ProcessLogFilter {
  if (!filter) return {};
  return {
    ...filter,
    sources: filter.sources ? [...filter.sources] : undefined,
    levels: filter.levels ? [...filter.levels] : undefined
  };
}

function cloneSanitizedValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(cloneSanitizedValue);
  if (!isRecord(value)) return value;
  const output: Record<string, unknown> = {};
  for (const key of Object.keys(value)) output[key] = cloneSanitizedValue(value[key]);
  return output;
}

function estimateLineBytes(line: ProcessLogLine): number {
  return Buffer.byteLength(JSON.stringify(line), "utf8");
}

function matchesFilter(line: ProcessLogLine, filter: ProcessLogFilter): boolean {
  if (filter.source && line.source !== filter.source) return false;
  if (filter.sources && !filter.sources.includes(line.source)) return false;
  if (filter.level && line.level !== filter.level) return false;
  if (filter.levels && !filter.levels.includes(line.level)) return false;
  if (filter.afterSequence !== undefined && line.sequence <= filter.afterSequence) return false;
  if (filter.beforeSequence !== undefined && line.sequence >= filter.beforeSequence) return false;
  if (filter.search && !line.message.toLocaleLowerCase().includes(filter.search.toLocaleLowerCase())) return false;
  return true;
}

function truncateUtf8(value: string, maxBytes: number): string {
  if (Buffer.byteLength(value, "utf8") <= maxBytes) return value;
  return Buffer.from(value, "utf8").subarray(0, maxBytes).toString("utf8");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function normalizeKey(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]/g, "");
}

function isSensitiveKey(key: string): boolean {
  return SENSITIVE_NORMALIZED_KEYS.has(normalizeKey(key));
}

function isSensitiveMetadataKey(key: string): boolean {
  const normalized = normalizeKey(key);
  return SENSITIVE_NORMALIZED_KEYS.has(normalized) || SENSITIVE_PATH_KEYS.has(normalized);
}

function sanitizeMetadataValue(value: unknown, key: string, context: SanitizeContext, depth: number): unknown {
  if (isSensitiveMetadataKey(key)) return REDACTED;
  if (value === null || typeof value === "boolean" || typeof value === "number") return value;
  if (typeof value === "string") return truncateUtf8(sanitizeProcessLogMessage(value), MAX_METADATA_STRING_LENGTH);
  if (typeof value === "bigint") return `${value.toString()}n`;
  if (typeof value === "undefined") return undefined;
  if (typeof value === "function" || typeof value === "symbol") return "[unserializable]";
  if (depth >= MAX_METADATA_DEPTH) return "[truncated]";
  if (context.seen.has(value)) return "[circular]";
  context.seen.add(value);

  let descriptors: PropertyDescriptorMap;
  try {
    descriptors = Object.getOwnPropertyDescriptors(value);
  } catch {
    return UNREADABLE;
  }

  try {
    if (value instanceof Error) {
      return {
        name: sanitizeErrorProperty(descriptors.name, "Error"),
        message: sanitizeErrorProperty(descriptors.message),
        stack: sanitizeErrorProperty(descriptors.stack)
      };
    }
    if (value instanceof Date) {
      const time = Date.prototype.getTime.call(value);
      return Number.isFinite(time) ? new Date(time).toISOString() : "[invalid date]";
    }
    if (Array.isArray(value)) return sanitizeArray(descriptors, context, depth);

    const output: Record<string, unknown> = {};
    for (const [childKey, descriptor] of Object.entries(descriptors)) {
      if (!descriptor.enumerable) continue;
      if (context.keys++ >= MAX_METADATA_KEYS) {
        output.__truncated__ = true;
        break;
      }
      if (isSensitiveMetadataKey(childKey)) {
        output[childKey] = REDACTED;
      } else if (!("value" in descriptor)) {
        output[childKey] = ACCESSOR;
      } else {
        output[childKey] = sanitizeMetadataValue(descriptor.value, childKey, context, depth + 1);
      }
    }
    return output;
  } catch {
    return UNREADABLE;
  }
}

function sanitizeArray(descriptors: PropertyDescriptorMap, context: SanitizeContext, depth: number): unknown[] {
  const rawLength = descriptors.length && "value" in descriptors.length ? descriptors.length.value : 0;
  const actualLength = typeof rawLength === "number" && Number.isSafeInteger(rawLength) && rawLength >= 0
    ? rawLength
    : 0;
  const length = Math.min(actualLength, MAX_METADATA_ARRAY_ITEMS);
  const firstDescriptor = descriptors["0"];
  const firstItem = firstDescriptor && "value" in firstDescriptor ? firstDescriptor.value : undefined;
  if (length === 2 && typeof firstItem === "string" && isSensitiveKey(firstItem)) {
    return [sanitizeMetadataValue(firstItem, "value", context, depth + 1), REDACTED];
  }

  const output: unknown[] = [];
  for (let index = 0; index < length; index += 1) {
    const descriptor = descriptors[String(index)];
    if (!descriptor) {
      output.push(undefined);
    } else if (!("value" in descriptor)) {
      output.push(ACCESSOR);
    } else {
      output.push(sanitizeMetadataValue(descriptor.value, "value", context, depth + 1));
    }
  }
  if (actualLength > length) output.push("[truncated]");
  return output;
}

function sanitizeErrorProperty(descriptor: PropertyDescriptor | undefined, fallback?: string): string | undefined {
  if (!descriptor) return fallback;
  if (!("value" in descriptor)) return ACCESSOR;
  const value = descriptor.value;
  if (value === undefined) return fallback;
  if (typeof value !== "string" && typeof value !== "number" && typeof value !== "boolean") return UNREADABLE;
  return sanitizeProcessLogMessage(String(value));
}

function deepFreeze<T>(value: T, seen = new WeakSet<object>()): T {
  if (typeof value !== "object" || value === null || seen.has(value)) return value;
  seen.add(value);
  for (const child of Object.values(value)) deepFreeze(child, seen);
  return Object.freeze(value);
}
