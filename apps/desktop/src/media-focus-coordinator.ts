import { randomUUID } from "node:crypto";
import type {
  MediaFocusCommand,
  MediaFocusCancellation,
  MediaFocusCompletion,
  MediaFocusRequest
} from "@megle/notification-contract";

interface PendingMediaFocus {
  cancel: (cancellation: MediaFocusCancellation) => void;
  resolve: (ok: boolean) => void;
  timeout: ReturnType<typeof setTimeout>;
}

export class MediaFocusCoordinator {
  private readonly pending = new Map<string, PendingMediaFocus>();
  private readonly timeoutMs: number;

  constructor(options: { timeoutMs?: number } = {}) {
    this.timeoutMs = options.timeoutMs ?? 15_000;
  }

  request(
    request: MediaFocusRequest,
    deliver: (command: MediaFocusCommand) => void,
    cancel: (cancellation: MediaFocusCancellation) => void = () => undefined
  ): Promise<boolean> {
    const requestId = randomUUID();
    return new Promise<boolean>((resolve) => {
      const timeout = setTimeout(() => this.cancelAndSettle(requestId), this.timeoutMs);
      this.pending.set(requestId, { cancel, resolve, timeout });
      try {
        deliver({ ...request, requestId });
      } catch {
        this.settle(requestId, false);
      }
    });
  }

  complete(completion: MediaFocusCompletion): boolean {
    return this.settle(completion.requestId, completion.ok);
  }

  failAll(): void {
    for (const requestId of [...this.pending.keys()]) this.cancelAndSettle(requestId);
  }

  private cancelAndSettle(requestId: string): boolean {
    const request = this.pending.get(requestId);
    if (!request) return false;
    try {
      request.cancel({ requestId });
    } catch {
      // Cancellation is best-effort; the requester must still be released.
    }
    return this.settle(requestId, false);
  }

  private settle(requestId: string, ok: boolean): boolean {
    const request = this.pending.get(requestId);
    if (!request) return false;
    this.pending.delete(requestId);
    clearTimeout(request.timeout);
    request.resolve(ok);
    return true;
  }
}
