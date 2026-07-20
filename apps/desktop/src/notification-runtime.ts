import type { NotificationCoreSession } from "./notification-service.js";
import type { NotificationLibraryIdentity } from "./notification-types.js";

export interface NotificationRuntimeService {
  attachLibrary(
    library: NotificationLibraryIdentity,
    session: NotificationCoreSession
  ): Promise<void>;
  detachLibrary(): Promise<void>;
  close(): Promise<void>;
}

type NotificationRuntimeFactory<T extends NotificationRuntimeService> = (
  signal: AbortSignal
) => Promise<T>;

export class NotificationRuntime<T extends NotificationRuntimeService = NotificationRuntimeService> {
  private readonly createService: NotificationRuntimeFactory<T>;
  private readonly controller = new AbortController();
  private service: T | null = null;
  private startPromise: Promise<T> | null = null;
  private closePromise: Promise<void> | null = null;
  private closed = false;

  constructor(createService: NotificationRuntimeFactory<T>) {
    this.createService = createService;
  }

  start(): Promise<T> {
    if (this.closed) return Promise.reject(new Error("Notification runtime is closed"));
    if (this.service) return Promise.resolve(this.service);
    if (this.startPromise) return this.startPromise;

    const pending = this.createService(this.controller.signal).then(async (service) => {
      if (this.closed || this.controller.signal.aborted) {
        await service.close();
        throw new Error("Notification runtime closed during startup");
      }
      this.service = service;
      return service;
    });
    this.startPromise = pending;
    void pending.finally(() => {
      if (this.startPromise === pending) this.startPromise = null;
    }).catch(() => undefined);
    return pending;
  }

  async attachLibrary(
    library: NotificationLibraryIdentity,
    session: NotificationCoreSession
  ): Promise<void> {
    const service = await this.start();
    if (this.closed) throw new Error("Notification runtime is closed");
    await service.attachLibrary(library, session);
  }

  async detachLibrary(): Promise<void> {
    const service = await this.start();
    if (this.closed) throw new Error("Notification runtime is closed");
    await service.detachLibrary();
  }

  close(): Promise<void> {
    if (this.closePromise) return this.closePromise;
    this.closed = true;
    this.controller.abort(new DOMException("Notification runtime closed", "AbortError"));
    const pendingStart = this.startPromise;
    const pendingClose = (async () => {
      let service = this.service;
      if (!service && pendingStart) {
        try {
          service = await pendingStart;
        } catch {
          service = null;
        }
      }
      this.service = null;
      await service?.close();
    })();
    this.closePromise = pendingClose;
    return pendingClose;
  }
}
