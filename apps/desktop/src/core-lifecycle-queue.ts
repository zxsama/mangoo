export type CoreLifecycleTask<T> = (signal: AbortSignal) => T | Promise<T>;

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
  readonly resolve: (value: T | PromiseLike<T>) => void;
  readonly reject: (reason?: unknown) => void;
};

export class CoreLifecycleQueue {
  private readonly pending: QueueItem<unknown>[] = [];
  private active: QueueItem<unknown> | null = null;
  private shutdownRequested = false;
  private closed = false;
  private shutdownAttempt: Promise<unknown> | null = null;

  enqueue<T>(task: CoreLifecycleTask<T>): Promise<T> {
    if (this.shutdownRequested || this.closed) {
      return Promise.reject(new LifecycleClosedError());
    }
    return this.enqueueInternal(task);
  }

  shutdown<T>(task: CoreLifecycleTask<T>): Promise<T> {
    if (this.closed) {
      return Promise.reject(new LifecycleClosedError());
    }
    if (this.shutdownAttempt) {
      return this.shutdownAttempt as Promise<T>;
    }

    this.shutdownRequested = true;
    for (const item of this.pending.splice(0)) {
      item.controller.abort();
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

  private enqueueInternal<T>(task: CoreLifecycleTask<T>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      this.pending.push({
        task,
        controller: new AbortController(),
        resolve: (value) => resolve(value as T),
        reject
      });
      this.drain();
    });
  }

  private drain(): void {
    if (this.active || this.pending.length === 0) return;
    const item = this.pending.shift();
    if (!item) return;
    this.active = item;
    Promise.resolve()
      .then(() => item.task(item.controller.signal))
      .then(item.resolve, item.reject)
      .finally(() => {
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
