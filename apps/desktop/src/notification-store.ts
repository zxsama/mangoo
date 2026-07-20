import crypto from "node:crypto";
import path from "node:path";
import { Worker } from "node:worker_threads";

import type {
  NotificationCompactSnapshot,
  NotificationHistoryPage,
  NotificationHistoryQuery,
  NotificationLibraryIdentity,
  NotificationRetentionResult,
  NotificationStoreEvent,
  NotificationStoreRequest,
  NotificationStoreSerializedError,
  NotificationStoreWorkerMessage,
  ThumbnailCacheSummary
} from "./notification-types.js";

export interface NotificationStoreOptions {
  databasePath: string;
  workerUrl?: URL;
}

const WORKER_CLOSE_TIMEOUT_MS = 1_000;

interface PendingRequest {
  resolve: (value: unknown) => void;
  reject: (reason?: unknown) => void;
}

export function notificationDatabasePath(userDataDir: string): string {
  return path.join(userDataDir, "notifications.sqlite");
}

function errorFromSerialized(error: NotificationStoreSerializedError): Error {
  const result = new Error(error.message);
  result.name = error.name;
  if (error.code) {
    Object.assign(result, { code: error.code });
  }
  return result;
}

export class NotificationStore {
  private readonly worker: Worker;
  private readonly pending = new Map<string, PendingRequest>();
  private readonly readyPromise: Promise<void>;
  private readyResolve!: () => void;
  private readyReject!: (reason?: unknown) => void;
  private requestCounter = 0;
  private closed = false;
  private closing = false;
  private closePromise: Promise<void> | null = null;
  private failure: Error | null = null;

  constructor(options: NotificationStoreOptions) {
    this.readyPromise = new Promise<void>((resolve, reject) => {
      this.readyResolve = resolve;
      this.readyReject = reject;
    });
    this.worker = new Worker(
      options.workerUrl ?? new URL("./notification-store-worker.js", import.meta.url),
      { workerData: { databasePath: options.databasePath } }
    );
    this.worker.on("message", (message: NotificationStoreWorkerMessage) => {
      this.handleMessage(message);
    });
    this.worker.on("error", (error) => {
      this.fail(error);
    });
    this.worker.on("exit", (code) => {
      if (code !== 0 && !this.failure) {
        this.fail(new Error(`notification store worker exited with code ${code}`));
      } else if (code === 0 && !this.closed && !this.closing) {
        this.fail(new Error("notification store worker exited unexpectedly"));
      }
    });
  }

  static async open(options: NotificationStoreOptions): Promise<NotificationStore> {
    const store = new NotificationStore(options);
    try {
      await store.ready();
      return store;
    } catch (error) {
      await store.close().catch(() => undefined);
      throw error;
    }
  }

  async ready(): Promise<void> {
    await this.readyPromise;
  }

  async applyOutboxBatch(
    library: NotificationLibraryIdentity,
    events: NotificationStoreEvent[],
    maxSequence: number
  ): Promise<void> {
    await this.request<void>({
      requestId: this.nextRequestId(),
      method: "applyOutboxBatch",
      params: { library, events, maxSequence }
    });
  }

  async cursor(libraryId: string): Promise<number> {
    return this.request<number>({
      requestId: this.nextRequestId(),
      method: "cursor",
      params: { libraryId }
    });
  }

  async compactSnapshot(): Promise<NotificationCompactSnapshot> {
    return this.request<NotificationCompactSnapshot>({
      requestId: this.nextRequestId(),
      method: "compactSnapshot",
      params: {}
    });
  }

  async history(query: NotificationHistoryQuery): Promise<NotificationHistoryPage> {
    return this.request<NotificationHistoryPage>({
      requestId: this.nextRequestId(),
      method: "history",
      params: { query }
    });
  }

  async publishThumbnailSummary(summary: ThumbnailCacheSummary): Promise<void> {
    await this.request<void>({
      requestId: this.nextRequestId(),
      method: "publishThumbnailSummary",
      params: { summary }
    });
  }

  async hasLegacyImport(libraryId: string): Promise<boolean> {
    return this.request<boolean>({
      requestId: this.nextRequestId(),
      method: "hasLegacyImport",
      params: { libraryId }
    });
  }

  async markLegacyImported(libraryId: string, importedAt: number): Promise<void> {
    await this.request<void>({
      requestId: this.nextRequestId(),
      method: "markLegacyImported",
      params: { libraryId, importedAt }
    });
  }

  async cleanupRetention(now: number): Promise<NotificationRetentionResult> {
    return this.request<NotificationRetentionResult>({
      requestId: this.nextRequestId(),
      method: "cleanupRetention",
      params: { now }
    });
  }

  async close(): Promise<void> {
    if (this.closed) {
      return;
    }
    if (this.closePromise) {
      return this.closePromise;
    }
    this.closePromise = this.closeWorker();
    return this.closePromise;
  }

  private async closeWorker(): Promise<void> {
    this.closing = true;
    try {
      if (!this.failure) {
        const closeRequest = this.request<void>({
          requestId: this.nextRequestId(),
          method: "close",
          params: {}
        });
        let timeoutId: ReturnType<typeof setTimeout> | undefined;
        const timeout = new Promise<void>((resolve) => {
          timeoutId = setTimeout(resolve, WORKER_CLOSE_TIMEOUT_MS);
        });
        try {
          await Promise.race([closeRequest, timeout]);
        } catch {
          // Closing is best effort once the worker has failed; termination below is definitive.
        } finally {
          if (timeoutId !== undefined) {
            clearTimeout(timeoutId);
          }
        }
      }
    } finally {
      this.closed = true;
      this.rejectPending(new Error("notification store closed"));
      await this.worker.terminate();
    }
  }

  private nextRequestId(): string {
    this.requestCounter = (this.requestCounter + 1) % Number.MAX_SAFE_INTEGER;
    return `${this.requestCounter}-${crypto.randomUUID()}`;
  }

  private async request<T>(request: NotificationStoreRequest): Promise<T> {
    if (this.closed || (this.closing && request.method !== "close")) {
      throw new Error("notification store is closed");
    }
    await this.readyPromise;
    if (this.failure) {
      throw this.failure;
    }
    return new Promise<T>((resolve, reject) => {
      this.pending.set(request.requestId, {
        resolve: resolve as (value: unknown) => void,
        reject
      });
      try {
        this.worker.postMessage(request);
      } catch (error) {
        this.pending.delete(request.requestId);
        reject(error);
      }
    });
  }

  private handleMessage(message: NotificationStoreWorkerMessage): void {
    if (message.type === "ready") {
      this.readyResolve();
      return;
    }
    if (message.type === "startupError") {
      this.fail(errorFromSerialized(message.error));
      return;
    }
    const pending = this.pending.get(message.requestId);
    if (!pending) {
      return;
    }
    this.pending.delete(message.requestId);
    if (message.ok) {
      pending.resolve(message.result);
    } else {
      pending.reject(errorFromSerialized(message.error));
    }
  }

  private fail(error: unknown): void {
    if (this.failure) {
      return;
    }
    this.failure = error instanceof Error ? error : new Error(String(error));
    this.readyReject(this.failure);
    this.rejectPending(this.failure);
  }

  private rejectPending(error: Error): void {
    for (const pending of this.pending.values()) {
      pending.reject(error);
    }
    this.pending.clear();
  }
}

export async function createNotificationStore(
  options: NotificationStoreOptions
): Promise<NotificationStore> {
  return NotificationStore.open(options);
}
