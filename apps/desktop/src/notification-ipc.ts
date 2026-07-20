import { randomUUID } from "node:crypto";
import { createRequire } from "node:module";
import type {
  ProcessLogSubscriptionEvent,
  ProcessLogSubscriptionId,
  ProcessLogSubscriptionStart,
  RendererLogEntryInput
} from "@megle/notification-contract";

import type {
  NotificationHistoryQuery,
  RendererErrorReport,
  ThumbnailCacheSummary
} from "./notification-types.js";
import type {
  ProcessLogBatch,
  ProcessLogFilter,
  ProcessLogLevel,
  ProcessLogSource,
  ProcessLogSnapshotSubscription
} from "./process-log-buffer.js";

export interface NotificationIpcChannels {
  readonly openWindow: string;
  readonly closeWindow: string;
  readonly compact: string;
  readonly history: string;
  readonly cancel: string;
  readonly retry: string;
  readonly reportError: string;
  readonly thumbnailSummary: string;
  readonly changed: string;
  readonly logsPage: string;
  readonly logsSubscribe: string;
  readonly logsUnsubscribe: string;
  readonly logsClear: string;
  readonly logsChanged: string;
  readonly rendererLogs: string;
}

export const NOTIFICATION_IPC_CHANNELS = createRequire(import.meta.url)(
  "./notification-ipc-channels.cjs"
) as NotificationIpcChannels;

export type NotificationIpcWindowRole = "main" | "settings" | "plugins" | "notifications";

const ALL_RENDERER_ROLES = new Set<NotificationIpcWindowRole>([
  "main",
  "settings",
  "plugins",
  "notifications"
]);
const MAIN_AND_NOTIFICATION_ROLES = new Set<NotificationIpcWindowRole>(["main", "notifications"]);
const NOTIFICATION_WINDOW_ONLY = new Set<NotificationIpcWindowRole>(["notifications"]);
const MAIN_WINDOW_ONLY = new Set<NotificationIpcWindowRole>(["main"]);

const CHANNEL_PERMISSIONS = new Map<string, ReadonlySet<NotificationIpcWindowRole>>([
  [NOTIFICATION_IPC_CHANNELS.openWindow, MAIN_WINDOW_ONLY],
  [NOTIFICATION_IPC_CHANNELS.closeWindow, NOTIFICATION_WINDOW_ONLY],
  [NOTIFICATION_IPC_CHANNELS.compact, MAIN_AND_NOTIFICATION_ROLES],
  [NOTIFICATION_IPC_CHANNELS.history, NOTIFICATION_WINDOW_ONLY],
  [NOTIFICATION_IPC_CHANNELS.cancel, NOTIFICATION_WINDOW_ONLY],
  [NOTIFICATION_IPC_CHANNELS.retry, NOTIFICATION_WINDOW_ONLY],
  [NOTIFICATION_IPC_CHANNELS.reportError, ALL_RENDERER_ROLES],
  [NOTIFICATION_IPC_CHANNELS.thumbnailSummary, MAIN_WINDOW_ONLY],
  [NOTIFICATION_IPC_CHANNELS.logsPage, NOTIFICATION_WINDOW_ONLY],
  [NOTIFICATION_IPC_CHANNELS.logsSubscribe, NOTIFICATION_WINDOW_ONLY],
  [NOTIFICATION_IPC_CHANNELS.logsUnsubscribe, NOTIFICATION_WINDOW_ONLY],
  [NOTIFICATION_IPC_CHANNELS.logsClear, NOTIFICATION_WINDOW_ONLY],
  [NOTIFICATION_IPC_CHANNELS.rendererLogs, ALL_RENDERER_ROLES]
]);

const OPERATION_STATUSES = new Set(["pending", "running", "succeeded", "failed", "cancelled"]);
const OPERATION_CATEGORIES = new Set(["task", "file_operation", "error", "thumbnail_cache"]);
const LOG_SOURCES = new Set<ProcessLogSource>(["main", "core", "renderer"]);
const LOG_LEVELS = new Set<ProcessLogLevel>(["trace", "debug", "info", "warn", "error"]);
type Mutable<T> = { -readonly [K in keyof T]: T[K] };

export function canAccessNotificationIpc(
  role: NotificationIpcWindowRole | null,
  channel: string
): boolean {
  if (!role) return false;
  return CHANNEL_PERMISSIONS.get(channel)?.has(role) === true;
}

export function assertNotificationIpcAccess(
  role: NotificationIpcWindowRole | null,
  channel: string
): asserts role is NotificationIpcWindowRole {
  if (!canAccessNotificationIpc(role, channel)) {
    throw new Error(`Notification IPC access denied for ${role ?? "unknown"}: ${channel}`);
  }
}

export function parseNotificationHistoryQuery(value: unknown): NotificationHistoryQuery {
  const input = record(value, "history query");
  const query: NotificationHistoryQuery = {};
  if (input.libraryId !== undefined) query.libraryId = boundedString(input.libraryId, "libraryId", 256);
  if (input.search !== undefined) query.search = boundedString(input.search, "search", 500, true);
  if (input.limit !== undefined) query.limit = boundedInteger(input.limit, "limit", 1, 200);
  if (input.offset !== undefined) query.offset = boundedInteger(input.offset, "offset", 0, 2_000_000_000);
  if (input.statuses !== undefined) {
    query.statuses = boundedStringArray(input.statuses, "statuses", OPERATION_STATUSES, 5) as NonNullable<
      NotificationHistoryQuery["statuses"]
    >;
  }
  if (input.categories !== undefined) {
    query.categories = boundedStringArray(
      input.categories,
      "categories",
      OPERATION_CATEGORIES,
      4
    ) as NonNullable<NotificationHistoryQuery["categories"]>;
  }
  return query;
}

export function parseNotificationAction(
  libraryId: unknown,
  operationId: unknown
): { libraryId: string; operationId: string } {
  return {
    libraryId: boundedString(libraryId, "libraryId", 256),
    operationId: boundedString(operationId, "operationId", 256)
  };
}

export function parseRendererErrorReport(value: unknown): RendererErrorReport {
  const input = record(value, "renderer error report");
  return {
    libraryId: boundedString(input.libraryId, "libraryId", 256),
    libraryName: boundedString(input.libraryName, "libraryName", 256),
    source: boundedString(input.source, "source", 128),
    code: boundedString(input.code, "code", 128),
    target: boundedString(input.target, "target", 512),
    title: boundedString(input.title, "title", 256),
    message: boundedString(input.message, "message", 4_000, true),
    detail:
      input.detail === null || input.detail === undefined
        ? null
        : boundedString(input.detail, "detail", 16_000, true)
  };
}

export function parseThumbnailCacheSummary(value: unknown): ThumbnailCacheSummary {
  const input = record(value, "thumbnail summary");
  const totalCacheBytes =
    input.totalCacheBytes === null || input.totalCacheBytes === undefined
      ? null
      : boundedInteger(input.totalCacheBytes, "totalCacheBytes", 0, Number.MAX_SAFE_INTEGER);
  const completionPercent =
    input.completionPercent === null || input.completionPercent === undefined
      ? null
      : boundedNumber(input.completionPercent, "completionPercent", 0, 100);
  return {
    libraryId: boundedString(input.libraryId, "libraryId", 256),
    libraryName: boundedString(input.libraryName, "libraryName", 256),
    observedAt: boundedInteger(input.observedAt, "observedAt", 0, Number.MAX_SAFE_INTEGER),
    cachedCount: boundedInteger(input.cachedCount, "cachedCount", 0, Number.MAX_SAFE_INTEGER),
    totalCandidateCount: boundedInteger(
      input.totalCandidateCount,
      "totalCandidateCount",
      0,
      Number.MAX_SAFE_INTEGER
    ),
    missingCount: boundedInteger(input.missingCount, "missingCount", 0, Number.MAX_SAFE_INTEGER),
    staleCount: boundedInteger(input.staleCount, "staleCount", 0, Number.MAX_SAFE_INTEGER),
    failedCount: boundedInteger(input.failedCount, "failedCount", 0, Number.MAX_SAFE_INTEGER),
    pendingCount: boundedInteger(input.pendingCount, "pendingCount", 0, Number.MAX_SAFE_INTEGER),
    activeBulkTaskCount: boundedInteger(
      input.activeBulkTaskCount,
      "activeBulkTaskCount",
      0,
      Number.MAX_SAFE_INTEGER
    ),
    totalCacheBytes,
    completionPercent
  };
}

export function parseProcessLogFilter(value: unknown): ProcessLogFilter {
  if (value === undefined || value === null) return {};
  const input = record(value, "process log filter");
  const filter: Mutable<ProcessLogFilter> = {};
  if (input.source !== undefined) filter.source = enumString(input.source, "source", LOG_SOURCES);
  if (input.sources !== undefined) {
    filter.sources = boundedStringArray(input.sources, "sources", LOG_SOURCES, 3) as ProcessLogFilter["sources"];
  }
  if (input.level !== undefined) filter.level = enumString(input.level, "level", LOG_LEVELS);
  if (input.levels !== undefined) {
    filter.levels = boundedStringArray(input.levels, "levels", LOG_LEVELS, 5) as ProcessLogFilter["levels"];
  }
  if (input.search !== undefined) filter.search = boundedString(input.search, "search", 500, true);
  if (input.afterSequence !== undefined) {
    filter.afterSequence = boundedInteger(
      input.afterSequence,
      "afterSequence",
      0,
      Number.MAX_SAFE_INTEGER
    );
  }
  if (input.beforeSequence !== undefined) {
    filter.beforeSequence = boundedInteger(
      input.beforeSequence,
      "beforeSequence",
      0,
      Number.MAX_SAFE_INTEGER
    );
  }
  if (input.limit !== undefined) filter.limit = boundedInteger(input.limit, "limit", 1, 200);
  return filter;
}

export function parseRendererLogEntries(value: unknown): RendererLogEntryInput[] {
  if (!Array.isArray(value)) throw new TypeError("renderer logs must be an array");
  if (value.length > 100) throw new RangeError("renderer logs are limited to 100 entries per batch");
  return value.map((candidate, index) => {
    const input = record(candidate, `renderer log ${index}`);
    const level = enumString(input.level, `renderer log ${index} level`, LOG_LEVELS);
    const message = boundedString(input.message, `renderer log ${index} message`, 8_000, true);
    let metadata: Readonly<Record<string, unknown>> | undefined;
    if (input.metadata !== undefined) {
      metadata = record(input.metadata, `renderer log ${index} metadata`);
      const serialized = JSON.stringify(metadata);
      if (serialized.length > 16_000) {
        throw new RangeError(`renderer log ${index} metadata exceeds 16000 characters`);
      }
    }
    return { level, message, ...(metadata ? { metadata } : {}) };
  });
}

export function parseProcessLogSubscriptionId(value: unknown): ProcessLogSubscriptionId {
  return boundedString(value, "process log subscriptionId", 128);
}

export class LatestValueBroadcastQueue<T> {
  private readonly deliver: (value: T) => void;
  private readonly schedule: (flush: () => void) => void;
  private latest: T | undefined;
  private scheduled = false;
  private closed = false;

  constructor(
    deliver: (value: T) => void,
    schedule: (flush: () => void) => void = queueMicrotask
  ) {
    this.deliver = deliver;
    this.schedule = schedule;
  }

  publish(value: T): void {
    if (this.closed) return;
    this.latest = value;
    if (this.scheduled) return;
    this.scheduled = true;
    this.schedule(() => this.flush());
  }

  close(): void {
    this.closed = true;
    this.latest = undefined;
  }

  private flush(): void {
    this.scheduled = false;
    if (this.closed || this.latest === undefined) return;
    const value = this.latest;
    this.latest = undefined;
    this.deliver(value);
  }
}

export interface NotificationIpcBroadcastTarget {
  readonly id: number;
  isDestroyed(): boolean;
  send(channel: string, payload: unknown): void;
}

export class NotificationIpcBroadcastTargets<T> {
  private readonly targets = new Map<number, NotificationIpcBroadcastTarget>();
  private readonly channel: string;

  constructor(channel: string) {
    this.channel = channel;
  }

  get size(): number {
    return this.targets.size;
  }

  register(target: NotificationIpcBroadcastTarget): void {
    if (target.isDestroyed()) {
      this.targets.delete(target.id);
      return;
    }
    this.targets.set(target.id, target);
  }

  clearTarget(targetId: number): boolean {
    return this.targets.delete(targetId);
  }

  clear(): void {
    this.targets.clear();
  }

  broadcast(payload: T): void {
    for (const [targetId, target] of this.targets) {
      if (target.isDestroyed()) {
        this.targets.delete(targetId);
        continue;
      }
      try {
        target.send(this.channel, payload);
      } catch {
        this.targets.delete(targetId);
      }
    }
  }
}

type NavigationListener = (
  event: unknown,
  navigationUrl: string,
  isInPlace: boolean,
  isMainFrame: boolean,
  frameProcessId?: number,
  frameRoutingId?: number
) => void;

export interface NotificationIpcNavigableTarget {
  readonly id: number;
  on(event: "did-start-navigation", listener: NavigationListener): unknown;
  removeListener(event: "did-start-navigation", listener: NavigationListener): unknown;
}

export class NotificationIpcTargetLifecycle {
  private readonly tracked = new Map<
    number,
    { target: NotificationIpcNavigableTarget; listener: NavigationListener }
  >();
  private readonly clearTargetCallback: (targetId: number) => void;

  constructor(clearTarget: (targetId: number) => void) {
    this.clearTargetCallback = clearTarget;
  }

  track(target: NotificationIpcNavigableTarget): () => void {
    const existing = this.tracked.get(target.id);
    if (existing?.target !== target) {
      this.release(target.id);
      const listener: NavigationListener = (
        _event,
        _navigationUrl,
        _isInPlace,
        isMainFrame
      ) => {
        if (isMainFrame === true) this.clearTargetCallback(target.id);
      };
      target.on("did-start-navigation", listener);
      this.tracked.set(target.id, { target, listener });
    }
    return () => {
      if (this.tracked.get(target.id)?.target === target) this.release(target.id);
    };
  }

  release(targetId: number): boolean {
    const current = this.tracked.get(targetId);
    if (!current) return false;
    this.tracked.delete(targetId);
    current.target.removeListener("did-start-navigation", current.listener);
    this.clearTargetCallback(targetId);
    return true;
  }

  clear(): void {
    for (const targetId of [...this.tracked.keys()]) this.release(targetId);
  }
}

interface ProcessLogSubscriptionBuffer {
  snapshotAndSubscribe(
    subscriber: (batch: ProcessLogBatch) => void,
    filter: ProcessLogFilter
  ): ProcessLogSnapshotSubscription;
}

export interface ProcessLogIpcTarget {
  readonly id: number;
  isDestroyed(): boolean;
  send(channel: string, payload: ProcessLogSubscriptionEvent): void;
  once(event: "destroyed", listener: () => void): unknown;
  removeListener(event: "destroyed", listener: () => void): unknown;
}

export class ProcessLogIpcSubscriptions {
  private readonly buffer: ProcessLogSubscriptionBuffer;
  private readonly changedChannel: string;
  private readonly createSubscriptionId: () => string;
  private readonly subscriptions = new Map<
    number,
    {
      subscriptionId: ProcessLogSubscriptionId;
      target: ProcessLogIpcTarget;
      destroyListener: () => void;
      unsubscribe: () => void;
    }
  >();

  constructor(
    buffer: ProcessLogSubscriptionBuffer,
    changedChannel: string,
    createSubscriptionId: () => string = randomUUID
  ) {
    this.buffer = buffer;
    this.changedChannel = changedChannel;
    this.createSubscriptionId = createSubscriptionId;
  }

  get size(): number {
    return this.subscriptions.size;
  }

  snapshotAndSubscribe(
    target: ProcessLogIpcTarget,
    filter: ProcessLogFilter
  ): ProcessLogSubscriptionStart {
    this.clearTarget(target.id);
    const subscriptionId = this.createSubscriptionId();
    if (!subscriptionId) throw new Error("process log subscriptionId must not be empty");
    const subscription = this.buffer.snapshotAndSubscribe((batch) => {
      if (this.subscriptions.get(target.id)?.subscriptionId !== subscriptionId) return;
      if (target.isDestroyed()) {
        this.unsubscribe(target.id, subscriptionId);
        return;
      }
      target.send(this.changedChannel, { subscriptionId, batch });
    }, filter);
    const destroyListener = () => this.unsubscribe(target.id, subscriptionId);
    target.once("destroyed", destroyListener);
    this.subscriptions.set(target.id, {
      subscriptionId,
      target,
      destroyListener,
      unsubscribe: subscription.unsubscribe
    });
    return {
      subscriptionId,
      snapshot: subscription.snapshot,
      afterSequence: subscription.afterSequence
    };
  }

  unsubscribe(targetId: number, subscriptionId: ProcessLogSubscriptionId): boolean {
    const current = this.subscriptions.get(targetId);
    if (!current || current.subscriptionId !== subscriptionId) return false;
    this.clearTarget(targetId);
    return true;
  }

  clearTarget(targetId: number): boolean {
    const current = this.subscriptions.get(targetId);
    if (!current) return false;
    this.subscriptions.delete(targetId);
    current.target.removeListener("destroyed", current.destroyListener);
    current.unsubscribe();
    return true;
  }

  clear(): void {
    for (const targetId of [...this.subscriptions.keys()]) this.clearTarget(targetId);
  }
}

function record(value: unknown, name: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`${name} must be an object`);
  }
  return value as Record<string, unknown>;
}

function boundedString(value: unknown, name: string, maxLength: number, allowEmpty = false): string {
  if (typeof value !== "string") throw new TypeError(`${name} must be a string`);
  if (!allowEmpty && value.trim().length === 0) throw new RangeError(`${name} must not be empty`);
  if (value.length > maxLength) throw new RangeError(`${name} exceeds ${maxLength} characters`);
  return value;
}

function boundedNumber(value: unknown, name: string, min: number, max: number): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value < min || value > max) {
    throw new RangeError(`${name} must be between ${min} and ${max}`);
  }
  return value;
}

function boundedInteger(value: unknown, name: string, min: number, max: number): number {
  const result = boundedNumber(value, name, min, max);
  if (!Number.isSafeInteger(result)) throw new RangeError(`${name} must be a safe integer`);
  return result;
}

function enumString<T extends string>(value: unknown, name: string, values: ReadonlySet<T>): T {
  if (typeof value !== "string" || !values.has(value as T)) {
    throw new RangeError(`${name} is invalid`);
  }
  return value as T;
}

function boundedStringArray<T extends string>(
  value: unknown,
  name: string,
  allowed: ReadonlySet<T>,
  maxItems: number
): T[] {
  if (!Array.isArray(value) || value.length === 0 || value.length > maxItems) {
    throw new RangeError(`${name} must contain between 1 and ${maxItems} items`);
  }
  return value.map((item) => enumString(item, name, allowed));
}
