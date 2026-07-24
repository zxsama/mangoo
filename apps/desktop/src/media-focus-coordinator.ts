import { randomUUID } from "node:crypto";
import type {
  MediaFocusCommand,
  MediaFocusCompletion,
  MediaFocusRequest
} from "@megle/notification-contract";

interface PendingMediaFocus {
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
    deliver: (command: MediaFocusCommand) => void
  ): Promise<boolean> {
    const requestId = randomUUID();
    return new Promise<boolean>((resolve) => {
      const timeout = setTimeout(() => this.settle(requestId, false), this.timeoutMs);
      this.pending.set(requestId, { resolve, timeout });
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
    for (const requestId of [...this.pending.keys()]) this.settle(requestId, false);
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
