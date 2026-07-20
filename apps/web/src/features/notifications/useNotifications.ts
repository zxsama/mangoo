import { startTransition, useSyncExternalStore } from "react";
import type {
  DesktopNotificationCompactSnapshot,
  DesktopNotificationHistoryQuery,
  DesktopNotificationsBridge
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
}

export interface LoadNotificationHistoryOptions {
  append?: boolean;
}

const EMPTY_COMPACT_SNAPSHOT: DesktopNotificationCompactSnapshot = {
  active: [],
  thumbnailSummaries: [],
  latestImportantResults: [],
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

  constructor(options: NotificationStoreOptions = {}) {
    this.bridgeProvider = options.getBridge ?? defaultBridge;
    this.deferDisconnect = options.deferDisconnect ?? queueMicrotask;
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

  async loadHistory(
    query: DesktopNotificationHistoryQuery,
    options: LoadNotificationHistoryOptions = {}
  ): Promise<NotificationHistoryState> {
    const bridge = this.currentBridge();
    if (!bridge) {
      return this.setHistoryError("通知历史仅在桌面模式可用");
    }

    const append = options.append === true;
    const baseQuery = withoutOffset(query);
    const identity = historyQueryIdentity(baseQuery);
    const currentIdentity = this.snapshot.history.query
      ? historyQueryIdentity(withoutOffset(this.snapshot.history.query))
      : null;
    if (!append || identity !== currentIdentity) {
      this.historyGeneration += 1;
      this.historyPages.clear();
      this.historyRequests.clear();
      this.historyStartOffset = query.offset ?? 0;
      this.updateHistory({
        items: [],
        total: 0,
        nextOffset: this.historyStartOffset,
        query: baseQuery,
        loading: true,
        error: null
      });
    }

    const generation = this.historyGeneration;
    const offset = append
      ? this.snapshot.history.nextOffset
      : query.offset ?? this.historyStartOffset;
    if (offset === null) return this.snapshot.history;
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
        error: null
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
    retry
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
    search: query.search?.trim() ?? "",
    limit: query.limit ?? 50
  });
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
