import { Buffer } from "node:buffer";
import type {
  ProcessLogBatch,
  ProcessLogFilter,
  ProcessLogLevel,
  ProcessLogLine,
  ProcessLogMetadata,
  ProcessLogPage,
  ProcessLogSource
} from "@megle/notification-contract";

export type {
  ProcessLogBatch,
  ProcessLogFilter,
  ProcessLogLevel,
  ProcessLogLine,
  ProcessLogMetadata,
  ProcessLogPage,
  ProcessLogSource
} from "@megle/notification-contract";

export const DEFAULT_PROCESS_LOG_MAX_ENTRIES = 5_000;
export const DEFAULT_PROCESS_LOG_MAX_BYTES = 5 * 1024 * 1024;

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
  "credential",
  "credentials",
  "privatekey",
  "jwt",
  "sessionid",
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
const SENSITIVE_HEADER_KEYS = new Set([
  "authorization",
  "proxyauthorization",
  "cookie",
  "setcookie",
  "xapikey",
  "apikey",
  "meglesession",
  "meglesessiontoken",
  "xmeglesession",
  "xmeglesessiontoken",
  "xsessiontoken"
]);

export function sanitizeProcessLogMessage(value: string): string {
  let message = value;
  message = scanSensitiveKeyValues(message);
  message = redactAuthorizationSchemes(message);
  message = message.replace(WINDOWS_PATH_PATTERN, REDACTED);
  message = message.replace(UNIX_SENSITIVE_PATH_PATTERN, REDACTED);
  message = message.replace(/\r\n?/g, "\\n");
  return truncateUtf8(message, MAX_MESSAGE_BYTES);
}

type SensitiveValueMatch = {
  readonly valueStart: number;
  readonly valueEnd: number;
  readonly quote: "'" | '"' | null;
  readonly closedQuote: boolean;
};

function scanSensitiveKeyValues(input: string): string {
  let output = "";
  let cursor = 0;
  let index = 0;
  while (index < input.length) {
    const match = parseSensitiveValueAt(input, index);
    if (!match) {
      index += 1;
      continue;
    }
    output += input.slice(cursor, match.valueStart);
    if (match.quote) {
      output += match.quote;
      output += REDACTED;
      if (match.closedQuote) output += match.quote;
    } else {
      output += REDACTED;
    }
    cursor = match.valueEnd;
    index = match.valueEnd;
  }
  return output + input.slice(cursor);
}

function parseSensitiveValueAt(input: string, start: number): SensitiveValueMatch | null {
  const first = input[start];
  let keyEnd = start;
  let key: string;
  if (first === '"' || first === "'") {
    const closing = findQuotedEnd(input, start + 1, first);
    if (closing < 0) return null;
    key = input.slice(start + 1, closing);
    keyEnd = closing + 1;
  } else {
    if (!isKeyStart(first) || (start > 0 && isKeyCharacter(input[start - 1]))) return null;
    keyEnd = start + 1;
    while (keyEnd < input.length && isKeyCharacter(input[keyEnd])) keyEnd += 1;
    key = input.slice(start, keyEnd);
  }

  let separator = keyEnd;
  while (separator < input.length && isWhitespace(input[separator])) separator += 1;
  if (input[separator] !== ":" && input[separator] !== "=") return null;
  if (!isSensitiveKey(key)) return null;

  let valueStart = separator + 1;
  while (valueStart < input.length && isWhitespace(input[valueStart])) valueStart += 1;
  const quoteCandidate = input[valueStart];
  const valueQuote: '"' | "'" | null = quoteCandidate === '"' || quoteCandidate === "'"
    ? quoteCandidate
    : null;
  if (valueQuote) {
    const closing = findQuotedEnd(input, valueStart + 1, valueQuote);
    return {
      valueStart,
      valueEnd: closing < 0 ? input.length : closing + 1,
      quote: valueQuote,
      closedQuote: closing >= 0
    };
  }

  const valueEnd = SENSITIVE_HEADER_KEYS.has(normalizeKey(key))
    ? findHeaderValueEnd(input, valueStart)
    : findBareValueEnd(input, valueStart);
  return { valueStart, valueEnd, quote: null, closedQuote: false };
}

function redactAuthorizationSchemes(input: string): string {
  const schemePattern = /\b(Bearer|Basic)(\s+)/gi;
  let output = "";
  let cursor = 0;
  let match: RegExpExecArray | null;
  while ((match = schemePattern.exec(input)) !== null) {
    const tokenStart = match.index + match[0].length;
    const tokenEnd = findAuthTokenEnd(input, tokenStart);
    if (tokenEnd <= tokenStart) continue;
    output += input.slice(cursor, tokenStart);
    output += REDACTED;
    cursor = tokenEnd;
    schemePattern.lastIndex = tokenEnd;
  }
  return output + input.slice(cursor);
}

function findQuotedEnd(input: string, start: number, quote: string): number {
  let escaped = false;
  for (let index = start; index < input.length; index += 1) {
    const character = input[index];
    if (escaped) {
      escaped = false;
    } else if (character === "\\") {
      escaped = true;
    } else if (character === quote) {
      return index;
    }
  }
  return -1;
}

function findBareValueEnd(input: string, start: number): number {
  let index = start;
  while (index < input.length && !isWhitespace(input[index]) && !",;}".includes(input[index])) index += 1;
  return index;
}

function findHeaderValueEnd(input: string, start: number): number {
  let index = start;
  // A Cookie value may contain several semicolon-separated pairs. Keep the
  // whole header value covered instead of exposing a later credential pair.
  while (index < input.length && !"|}\r\n".includes(input[index])) index += 1;
  return index;
}

function findAuthTokenEnd(input: string, start: number): number {
  if (input[start] === '"' || input[start] === "'") {
    const closing = findQuotedEnd(input, start + 1, input[start]);
    return closing < 0 ? input.length : closing + 1;
  }
  let index = start;
  while (index < input.length && !isWhitespace(input[index]) && !",;}|".includes(input[index])) index += 1;
  return index;
}

function isWhitespace(value: string | undefined): boolean {
  return value !== undefined && /\s/.test(value);
}

function isKeyStart(value: string | undefined): boolean {
  return value !== undefined && /[A-Za-z]/.test(value);
}

function isKeyCharacter(value: string | undefined): boolean {
  return value !== undefined && /[A-Za-z0-9_-]/.test(value);
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
