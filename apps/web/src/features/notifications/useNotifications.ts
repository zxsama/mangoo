import { startTransition, useSyncExternalStore } from "react";
import type {
  DesktopNotificationCompactSnapshot,
  DesktopNotificationHistoryQuery,
  DesktopNotificationsBridge,
  DesktopProcessLogBatch,
  DesktopProcessLogExportResult,
  DesktopProcessLogFilter,
  DesktopProcessLogPage,
  NotificationLibraryIdentity,
  ProcessLogSubscriptionHandle as DesktopProcessLogSubscriptionHandle,
  ProcessLogSubscriptionInitialSnapshot
} from "@megle/notification-contract";
import { getDesktopBridge } from "../../core/desktop.ts";
import {
  buildCompactNotificationModel,
  mergeNotificationHistoryPages,
  type CompactNotificationModel,
  type NotificationHistoryCollection,
  type NotificationHistoryPageSlice
} from "./notificationModel.ts";

export interface NotificationHistoryState extends NotificationHistoryCollection {
  query: DesktopNotificationHistoryQuery | null;
  loading: boolean;
  error: string | null;
}

export interface NotificationStoreSnapshot {
  compact: CompactNotificationModel;
  history: NotificationHistoryState;
}

export interface NotificationStoreOptions {
  getBridge?: () => DesktopNotificationsBridge | null;
  deferDisconnect?: (callback: () => void) => void;
  deferLogDelivery?: (callback: () => void) => void;
}

export interface LoadNotificationHistoryOptions {
  append?: boolean;
}

export type NotificationRequestResult<T> =
  | { ok: true; value: T }
  | { ok: false; error: string };

export interface OpenNotificationLogSubscription {
  snapshot: ProcessLogSubscriptionInitialSnapshot;
  unsubscribe(): void;
}

const EMPTY_COMPACT_SNAPSHOT: DesktopNotificationCompactSnapshot = {
  active: [],
  thumbnailSummaries: [],
  latestImportantResults: [],
  hasRetryableFailure: false,
  syncHealth: {
    state: "degraded",
    libraryId: null,
    detail: "通知服务仅在桌面模式可用",
    updatedAt: 0
  }
};

const EMPTY_HISTORY: NotificationHistoryState = {
  items: [],
  total: 0,
  nextOffset: null,
  conflictError: null,
  query: null,
  loading: false,
  error: null
};

const EMPTY_STORE_SNAPSHOT: NotificationStoreSnapshot = {
  compact: buildCompactNotificationModel(EMPTY_COMPACT_SNAPSHOT),
  history: EMPTY_HISTORY
};

export class NotificationExternalStore {
  private readonly bridgeProvider: () => DesktopNotificationsBridge | null;
  private readonly deferDisconnect: (callback: () => void) => void;
  private readonly deferLogDelivery: (callback: () => void) => void;
  private readonly listeners = new Set<() => void>();
  private snapshot = EMPTY_STORE_SNAPSHOT;
  private bridge: DesktopNotificationsBridge | null = null;
  private disconnectBridge: (() => void) | null = null;
  private disconnectGeneration = 0;
  private compactConnectionGeneration = 0;
  private compactEventRevision = 0;
  private compactLoad: Promise<void> | null = null;
  private historyGeneration = 0;
  private historyStartOffset = 0;
  private historyPages = new Map<number, NotificationHistoryPageSlice>();
  private historyRequests = new Map<string, Promise<NotificationHistoryState>>();
  private logGeneration = 0;
  private activeLogSubscription: { generation: number; unsubscribe: () => void } | null = null;

  constructor(options: NotificationStoreOptions = {}) {
    this.bridgeProvider = options.getBridge ?? defaultBridge;
    this.deferDisconnect =
      options.deferDisconnect ?? ((callback) => globalThis.queueMicrotask(callback));
    this.deferLogDelivery = options.deferLogDelivery ?? ((callback) => setTimeout(callback, 0));
  }

  readonly getSnapshot = (): NotificationStoreSnapshot => this.snapshot;

  readonly getServerSnapshot = (): NotificationStoreSnapshot => EMPTY_STORE_SNAPSHOT;

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    this.disconnectGeneration += 1;
    if (this.listeners.size === 1) this.connect();
    let active = true;
    return () => {
      if (!active) return;
      active = false;
      this.listeners.delete(listener);
      if (this.listeners.size === 0) this.scheduleDisconnect();
    };
  };

  whenIdle(): Promise<void> {
    return this.compactLoad ?? Promise.resolve();
  }

  loadHistory(
    query: DesktopNotificationHistoryQuery,
    options: LoadNotificationHistoryOptions = {}
  ): Promise<NotificationHistoryState> {
    const bridge = this.currentBridge();
    if (!bridge) {
      return Promise.resolve(this.setHistoryError("通知历史仅在桌面模式可用"));
    }

    const append = options.append === true;
    const baseQuery = withoutOffset(query);
    const identity = historyQueryIdentity(baseQuery);
    const requestedStartOffset = append ? this.historyStartOffset : query.offset ?? 0;
    const currentIdentity = this.snapshot.history.query
      ? historyQueryIdentity(withoutOffset(this.snapshot.history.query))
      : null;
    const queryChanged =
      identity !== currentIdentity || requestedStartOffset !== this.historyStartOffset;
    if (!append && !queryChanged) {
      const existing = this.historyRequests.get(
        `${this.historyGeneration}:${requestedStartOffset}`
      );
      if (existing) return existing;
    }
    if (queryChanged || !append) {
      this.historyGeneration += 1;
      this.historyPages.clear();
      this.historyRequests.clear();
      this.historyStartOffset = requestedStartOffset;
      this.updateHistory({
        items: [],
        total: 0,
        nextOffset: this.historyStartOffset,
        conflictError: null,
        query: baseQuery,
        loading: true,
        error: null
      });
    }

    const generation = this.historyGeneration;
    const offset = append
      ? this.snapshot.history.nextOffset
      : requestedStartOffset;
    if (offset === null) return Promise.resolve(this.snapshot.history);
    const requestKey = `${generation}:${offset}`;
    const existing = this.historyRequests.get(requestKey);
    if (existing) return existing;

    if (!this.snapshot.history.loading) {
      this.updateHistory({ ...this.snapshot.history, loading: true, error: null });
    }
    const request = this.requestHistoryPage(
      bridge,
      baseQuery,
      offset,
      generation,
      requestKey
    );
    this.historyRequests.set(requestKey, request);
    return request;
  }

  loadMoreHistory(): Promise<NotificationHistoryState> {
    const query = this.snapshot.history.query;
    if (!query || this.snapshot.history.nextOffset === null) {
      return Promise.resolve(this.snapshot.history);
    }
    return this.loadHistory(query, { append: true });
  }

  cancel(libraryId: string, operationId: string) {
    return this.performAction("cancel", libraryId, operationId);
  }

  retry(libraryId: string, operationId: string) {
    return this.performAction("retry", libraryId, operationId);
  }

  async refreshCompact(): Promise<NotificationRequestResult<boolean>> {
    const bridge = this.currentBridge();
    if (!bridge) return { ok: false, error: "通知状态仅在桌面模式可用" };
    try {
      this.applyCompactSnapshot(await bridge.getCompactSnapshot());
      return { ok: true, value: true };
    } catch (error) {
      this.applyBridgeError(error);
      return { ok: false, error: errorMessage(error) };
    }
  }

  async listLibraries(): Promise<NotificationRequestResult<NotificationLibraryIdentity[]>> {
    const bridge = this.currentBridge();
    if (!bridge) return { ok: false, error: "资源库列表仅在桌面模式可用" };
    try {
      return { ok: true, value: await bridge.listLibraries() };
    } catch (error) {
      return { ok: false, error: errorMessage(error) };
    }
  }

  async getLogPage(
    query: DesktopProcessLogFilter
  ): Promise<NotificationRequestResult<DesktopProcessLogPage>> {
    const bridge = this.currentBridge();
    if (!bridge) return { ok: false, error: "进程日志仅在桌面模式可用" };
    try {
      return { ok: true, value: await bridge.getLogs(query) };
    } catch (error) {
      return { ok: false, error: errorMessage(error) };
    }
  }

  async exportLogs(
    query: DesktopProcessLogFilter
  ): Promise<NotificationRequestResult<DesktopProcessLogExportResult>> {
    const bridge = this.currentBridge();
    if (!bridge) return { ok: false, error: "进程日志仅在桌面模式可用" };
    try {
      return { ok: true, value: await bridge.exportLogs(query) };
    } catch (error) {
      return { ok: false, error: errorMessage(error) };
    }
  }

  async openLogSubscription(
    query: DesktopProcessLogFilter,
    listener: (batch: DesktopProcessLogBatch) => void
  ): Promise<NotificationRequestResult<OpenNotificationLogSubscription>> {
    const generation = ++this.logGeneration;
    this.activeLogSubscription?.unsubscribe();
    this.activeLogSubscription = null;
    const bridge = this.currentBridge();
    if (!bridge) return { ok: false, error: "进程日志仅在桌面模式可用" };

    let active = true;
    let delivering = false;
    const bufferedBatches: DesktopProcessLogBatch[] = [];
    let bridgeHandle: DesktopProcessLogSubscriptionHandle;
    try {
      bridgeHandle = bridge.snapshotAndSubscribeLogs(query, (batch) => {
        if (!active || generation !== this.logGeneration) return;
        if (!delivering) {
          bufferedBatches.push(batch);
          return;
        }
        listener(batch);
      });
    } catch (error) {
      return { ok: false, error: errorMessage(error) };
    }

    let resolveClosed!: () => void;
    const closed = new Promise<null>((resolve) => {
      resolveClosed = () => resolve(null);
    });
    const unsubscribe = () => {
      if (!active) return;
      active = false;
      bufferedBatches.length = 0;
      bridgeHandle.unsubscribe();
      resolveClosed();
      if (this.activeLogSubscription?.generation === generation) {
        this.activeLogSubscription = null;
      }
      if (this.logGeneration === generation) this.logGeneration += 1;
    };
    this.activeLogSubscription = { generation, unsubscribe };

    let snapshot: ProcessLogSubscriptionInitialSnapshot | null;
    try {
      snapshot = await Promise.race([bridgeHandle.ready, closed]);
    } catch (error) {
      unsubscribe();
      return { ok: false, error: errorMessage(error) };
    }
    if (!snapshot || !active || generation !== this.logGeneration) {
      unsubscribe();
      return { ok: false, error: "日志订阅已关闭" };
    }

    this.deferLogDelivery(() => {
      if (!active || generation !== this.logGeneration) return;
      delivering = true;
      const pending = bufferedBatches.splice(0);
      for (const batch of pending) listener(batch);
    });
    return { ok: true, value: { snapshot, unsubscribe } };
  }

  async clearLogs(): Promise<NotificationRequestResult<boolean>> {
    const bridge = this.currentBridge();
    if (!bridge) return { ok: false, error: "进程日志仅在桌面模式可用" };
    try {
      const cleared = await bridge.clearLogs();
      return cleared
        ? { ok: true, value: true }
        : { ok: false, error: "进程日志未能清空" };
    } catch (error) {
      return { ok: false, error: errorMessage(error) };
    }
  }

  dispose(): void {
    this.disconnectGeneration += 1;
    this.disconnectBridge?.();
    this.disconnectBridge = null;
    this.bridge = null;
    this.compactConnectionGeneration += 1;
    this.compactLoad = null;
    this.historyGeneration += 1;
    this.historyRequests.clear();
    if (this.snapshot.history.loading) {
      this.snapshot = {
        ...this.snapshot,
        history: { ...this.snapshot.history, loading: false }
      };
    }
    this.activeLogSubscription?.unsubscribe();
    this.activeLogSubscription = null;
    this.logGeneration += 1;
  }

  private connect(): void {
    if (this.bridge || this.disconnectBridge || this.compactLoad) return;
    const bridge = this.currentBridge();
    if (!bridge) return;
    this.bridge = bridge;
    const connectionGeneration = ++this.compactConnectionGeneration;
    const loadEventRevision = this.compactEventRevision;
    let subscriptionError: string | null = null;

    try {
      this.disconnectBridge = bridge.onChanged((snapshot) => {
        if (connectionGeneration !== this.compactConnectionGeneration) return;
        this.compactEventRevision += 1;
        this.applyCompactSnapshot(snapshot);
      });
    } catch (error) {
      subscriptionError = errorMessage(error);
      this.applyBridgeError(subscriptionError);
    }

    const load = bridge
      .getCompactSnapshot()
      .then((snapshot) => {
        if (
          connectionGeneration === this.compactConnectionGeneration &&
          this.compactEventRevision === loadEventRevision
        ) {
          this.applyCompactSnapshot(
            subscriptionError
              ? {
                  ...snapshot,
                  syncHealth: {
                    state: "degraded",
                    libraryId: snapshot.syncHealth.libraryId,
                    detail: subscriptionError,
                    updatedAt: Date.now()
                  }
                }
              : snapshot
          );
        }
      })
      .catch((error) => {
        if (
          connectionGeneration === this.compactConnectionGeneration &&
          this.compactEventRevision === loadEventRevision
        ) {
          this.applyBridgeError(error);
        }
      })
      .finally(() => {
        if (this.compactLoad === load) this.compactLoad = null;
      });
    this.compactLoad = load;
  }

  private scheduleDisconnect(): void {
    const generation = ++this.disconnectGeneration;
    this.deferDisconnect(() => {
      if (this.listeners.size > 0 || generation !== this.disconnectGeneration) return;
      this.disconnectBridge?.();
      this.disconnectBridge = null;
      this.bridge = null;
      this.compactConnectionGeneration += 1;
      this.compactLoad = null;
    });
  }

  private currentBridge(): DesktopNotificationsBridge | null {
    try {
      return this.bridge ?? this.bridgeProvider();
    } catch {
      return null;
    }
  }

  private applyCompactSnapshot(snapshot: DesktopNotificationCompactSnapshot): void {
    this.setSnapshot({
      ...this.snapshot,
      compact: buildCompactNotificationModel(snapshot)
    });
  }

  private applyBridgeError(error: unknown): void {
    const detail = errorMessage(error);
    this.applyCompactSnapshot({
      active: [],
      thumbnailSummaries: [],
      latestImportantResults: [],
      hasRetryableFailure: false,
      syncHealth: {
        state: "degraded",
        libraryId: null,
        detail,
        updatedAt: Date.now()
      }
    });
  }

  private async requestHistoryPage(
    bridge: DesktopNotificationsBridge,
    baseQuery: DesktopNotificationHistoryQuery,
    offset: number,
    generation: number,
    requestKey: string
  ): Promise<NotificationHistoryState> {
    try {
      const page = await bridge.listHistory({ ...baseQuery, offset });
      if (generation !== this.historyGeneration) return this.snapshot.history;
      this.historyPages.set(offset, { offset, page });
      const merged = mergeNotificationHistoryPages(
        [...this.historyPages.values()],
        this.historyStartOffset
      );
      this.updateHistory({
        ...merged,
        query: baseQuery,
        loading: this.hasOtherHistoryRequest(requestKey, generation),
        error: merged.conflictError
      });
      return this.snapshot.history;
    } catch (error) {
      if (generation !== this.historyGeneration) return this.snapshot.history;
      return this.setHistoryError(errorMessage(error));
    } finally {
      this.historyRequests.delete(requestKey);
      if (
        generation === this.historyGeneration &&
        !this.hasOtherHistoryRequest(requestKey, generation) &&
        this.snapshot.history.loading
      ) {
        this.updateHistory({ ...this.snapshot.history, loading: false });
      }
    }
  }

  private hasOtherHistoryRequest(requestKey: string, generation: number): boolean {
    return [...this.historyRequests.keys()].some(
      (key) => key !== requestKey && key.startsWith(`${generation}:`)
    );
  }

  private setHistoryError(detail: string): NotificationHistoryState {
    this.updateHistory({ ...this.snapshot.history, loading: false, error: detail });
    return this.snapshot.history;
  }

  private updateHistory(history: NotificationHistoryState): void {
    this.setSnapshot({ ...this.snapshot, history });
  }

  private async performAction(
    action: "cancel" | "retry",
    libraryId: string,
    operationId: string
  ) {
    const bridge = this.currentBridge();
    if (!bridge) return { ok: false, message: "通知操作仅在桌面模式可用" };
    try {
      return await bridge[action](libraryId, operationId);
    } catch (error) {
      return { ok: false, message: errorMessage(error) };
    }
  }

  private setSnapshot(snapshot: NotificationStoreSnapshot): void {
    if (snapshot === this.snapshot) return;
    this.snapshot = snapshot;
    for (const listener of this.listeners) listener();
  }
}

export function createNotificationStore(
  options: NotificationStoreOptions = {}
): NotificationExternalStore {
  return new NotificationExternalStore(options);
}

const notificationStore = createNotificationStore();
const loadHistory = notificationStore.loadHistory.bind(notificationStore);
const loadMoreHistory = notificationStore.loadMoreHistory.bind(notificationStore);
const cancel = notificationStore.cancel.bind(notificationStore);
const retry = notificationStore.retry.bind(notificationStore);
const refreshCompact = notificationStore.refreshCompact.bind(notificationStore);
const listLibraries = notificationStore.listLibraries.bind(notificationStore);
const getLogPage = notificationStore.getLogPage.bind(notificationStore);
const exportLogs = notificationStore.exportLogs.bind(notificationStore);
const openLogSubscription = notificationStore.openLogSubscription.bind(notificationStore);
const clearLogs = notificationStore.clearLogs.bind(notificationStore);

if (typeof window !== "undefined") {
  window.addEventListener("pagehide", () => notificationStore.dispose());
}

function subscribeWithTransition(listener: () => void): () => void {
  return notificationStore.subscribe(() => startTransition(listener));
}

export function useNotifications() {
  const snapshot = useSyncExternalStore(
    subscribeWithTransition,
    notificationStore.getSnapshot,
    notificationStore.getServerSnapshot
  );
  return {
    ...snapshot,
    loadHistory,
    loadMoreHistory,
    cancel,
    retry,
    refreshCompact,
    listLibraries,
    getLogPage,
    exportLogs,
    openLogSubscription,
    clearLogs
  };
}

function defaultBridge(): DesktopNotificationsBridge | null {
  if (typeof window === "undefined") return null;
  return getDesktopBridge()?.notifications ?? null;
}

function withoutOffset(query: DesktopNotificationHistoryQuery): DesktopNotificationHistoryQuery {
  const { offset: _offset, ...base } = query;
  return base;
}

function historyQueryIdentity(query: DesktopNotificationHistoryQuery): string {
  return JSON.stringify({
    libraryId: query.libraryId ?? null,
    statuses: [...(query.statuses ?? [])].sort(),
    categories: [...(query.categories ?? [])].sort(),
    origins: [...(query.origins ?? [])].sort(),
    kinds: [...(query.kinds ?? [])].sort(),
    search: query.search?.trim() ?? "",
    limit: query.limit ?? 50
  });
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
