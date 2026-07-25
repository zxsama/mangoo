export type CoreLifecycleTask<T> = (signal: AbortSignal) => T | Promise<T>;
export type CoreLifecycleEnqueueOptions = {
  readonly signal?: AbortSignal;
  readonly activeCancellation?: "abort" | "defer";
};

export class LifecycleCancelledError extends Error {
  constructor(message = "Core lifecycle operation cancelled") {
    super(message);
    this.name = "AbortError";
  }
}

export class LifecycleClosedError extends Error {
  constructor(message = "Core lifecycle is closed") {
    super(message);
    this.name = "CoreLifecycleClosedError";
  }
}

type QueueItem<T> = {
  readonly task: CoreLifecycleTask<T>;
  readonly controller: AbortController;
  readonly detachExternalAbort: () => void;
  readonly resolve: (value: T | PromiseLike<T>) => void;
  readonly reject: (reason?: unknown) => void;
};

export class CoreLifecycleQueue {
  private readonly pending: QueueItem<unknown>[] = [];
  private active: QueueItem<unknown> | null = null;
  private shutdownRequested = false;
  private closed = false;
  private shutdownAttempt: Promise<unknown> | null = null;
  private generation = 0;

  enqueue<T>(task: CoreLifecycleTask<T>, options: CoreLifecycleEnqueueOptions = {}): Promise<T> {
    if (this.shutdownRequested || this.closed) {
      return Promise.reject(new LifecycleClosedError());
    }
    return this.enqueueInternal(task, options);
  }

  shutdown<T>(task: CoreLifecycleTask<T>): Promise<T> {
    if (this.closed) {
      return Promise.reject(new LifecycleClosedError());
    }
    if (this.shutdownAttempt) {
      return this.shutdownAttempt as Promise<T>;
    }

    this.generation += 1;
    this.shutdownRequested = true;
    for (const item of this.pending.splice(0)) {
      item.controller.abort();
      item.detachExternalAbort();
      item.reject(new LifecycleCancelledError("Core lifecycle operation cancelled by shutdown"));
    }
    this.active?.controller.abort();

    const attempt = this.enqueueInternal(task);
    this.shutdownAttempt = attempt;
    void attempt.then(
      () => {
        this.closed = true;
      },
      () => undefined
    ).finally(() => {
      if (this.shutdownAttempt === attempt) this.shutdownAttempt = null;
    });
    return attempt;
  }

  beginGeneration(): number {
    if (this.shutdownRequested || this.closed) {
      throw new LifecycleClosedError();
    }
    this.generation += 1;
    return this.generation;
  }

  get currentGeneration(): number {
    return this.generation;
  }

  isCurrentGeneration(generation: number): boolean {
    return !this.shutdownRequested && !this.closed && generation === this.generation;
  }

  private enqueueInternal<T>(
    task: CoreLifecycleTask<T>,
    options: CoreLifecycleEnqueueOptions = {}
  ): Promise<T> {
    if (options.signal?.aborted) {
      return Promise.reject(new LifecycleCancelledError("Core lifecycle operation was already cancelled"));
    }
    return new Promise<T>((resolve, reject) => {
      let detachExternalAbort: () => void = () => undefined;
      const item: QueueItem<T> = {
        task,
        controller: new AbortController(),
        detachExternalAbort: () => detachExternalAbort(),
        resolve: (value) => resolve(value as T),
        reject
      };
      const externalSignal = options.signal;
      if (externalSignal) {
        const abort = () => {
          const queuedIndex = this.pending.indexOf(item as QueueItem<unknown>);
          if (queuedIndex >= 0) {
            item.controller.abort(externalSignal.reason);
            this.pending.splice(queuedIndex, 1);
            item.detachExternalAbort();
            item.reject(new LifecycleCancelledError("Core lifecycle operation externally cancelled"));
            this.drain();
          } else if (options.activeCancellation !== "defer") {
            item.controller.abort(externalSignal.reason);
          }
        };
        externalSignal.addEventListener("abort", abort, { once: true });
        detachExternalAbort = () => externalSignal.removeEventListener("abort", abort);
      }
      this.pending.push(item as QueueItem<unknown>);
      this.drain();
    });
  }

  private drain(): void {
    if (this.active || this.pending.length === 0) return;
    const item = this.pending.shift();
    if (!item) return;
    if (item.controller.signal.aborted) {
      item.detachExternalAbort();
      item.reject(new LifecycleCancelledError("Core lifecycle operation cancelled before start"));
      this.drain();
      return;
    }
    this.active = item;
    Promise.resolve()
      .then(() => item.task(item.controller.signal))
      .then(item.resolve, item.reject)
      .finally(() => {
        item.detachExternalAbort();
        if (this.active === item) this.active = null;
        this.drain();
      });
  }
}

export function throwIfLifecycleAborted(signal: AbortSignal): void {
  if (signal.aborted) {
    throw new LifecycleCancelledError();
  }
}
