import { randomUUID } from "node:crypto";
import type {
  MediaFocusCommand,
  MediaFocusCancellation,
  MediaFocusCompletion,
  MediaFocusRequest,
  MediaFocusStage,
  MediaFocusStageUpdate
} from "@megle/notification-contract";

interface PendingMediaFocus {
  cancel: (cancellation: MediaFocusCancellation) => void;
  command: MediaFocusCommand;
  controller: AbortController;
  deliver: (command: MediaFocusCommand) => void;
  resolve: (ok: boolean) => void;
  stage: "queued" | "awaiting" | MediaFocusStage;
  timeout: ReturnType<typeof setTimeout> | null;
}

export class MediaFocusCoordinator {
  private readonly pending = new Map<string, PendingMediaFocus>();
  private readonly queue: string[] = [];
  private readonly switchingTimeoutMs: number;
  private readonly timeoutMs: number;
  private activeRequestId: string | null = null;

  constructor(options: { switchingTimeoutMs?: number; timeoutMs?: number } = {}) {
    this.timeoutMs = options.timeoutMs ?? 15_000;
    // A healthy cross-library switch can include three 12s Core health
    // attempts. Keep that irreversible lifecycle serialized in Main while
    // still bounding a renderer that disappears before its locating ACK.
    this.switchingTimeoutMs = options.switchingTimeoutMs ?? 60_000;
  }

  request(
    request: MediaFocusRequest,
    deliver: (command: MediaFocusCommand) => void,
    cancel: (cancellation: MediaFocusCancellation) => void = () => undefined
  ): Promise<boolean> {
    const requestId = randomUUID();
    return new Promise<boolean>((resolve) => {
      this.pending.set(requestId, {
        cancel,
        command: { ...request, requestId },
        controller: new AbortController(),
        deliver,
        resolve,
        stage: "queued",
        timeout: null
      });
      this.queue.push(requestId);
      this.activateNext();
    });
  }

  complete(completion: MediaFocusCompletion): boolean {
    if (completion.requestId !== this.activeRequestId) return false;
    return this.settle(completion.requestId, completion.ok);
  }

  operationSignal(requestId: string): AbortSignal | null {
    if (requestId !== this.activeRequestId) return null;
    const request = this.pending.get(requestId);
    if (!request || request.stage !== "switching" || request.controller.signal.aborted) return null;
    return request.controller.signal;
  }

  refreshSwitching(requestId: string): boolean {
    if (requestId !== this.activeRequestId) return false;
    const request = this.pending.get(requestId);
    if (!request || request.stage !== "switching" || request.controller.signal.aborted) return false;
    this.armRequestTimeout(requestId, request, this.switchingTimeoutMs);
    return true;
  }

  updateStage(update: MediaFocusStageUpdate): boolean {
    if (update.requestId !== this.activeRequestId) return false;
    const request = this.pending.get(update.requestId);
    if (!request) return false;
    if (update.stage === "switching") {
      if (request.stage !== "awaiting" && request.stage !== "switching") return false;
      request.stage = "switching";
      this.armRequestTimeout(update.requestId, request, this.switchingTimeoutMs);
      return true;
    }
    if (request.stage !== "awaiting" && request.stage !== "switching" && request.stage !== "locating") {
      return false;
    }
    request.stage = "locating";
    this.armRequestTimeout(update.requestId, request);
    return true;
  }

  failAll(): void {
    const activeRequestId = this.activeRequestId;
    const requests = [...this.pending.entries()];
    this.activeRequestId = null;
    this.queue.length = 0;
    this.pending.clear();
    for (const [requestId, request] of requests) {
      this.clearRequestTimeout(request);
      request.controller.abort(new DOMException("Main renderer unavailable", "AbortError"));
      if (requestId === activeRequestId) {
        try {
          request.cancel({ requestId });
        } catch {
          // Main is closing; cancellation remains best-effort.
        }
      }
      request.resolve(false);
    }
  }

  private cancelAndSettle(requestId: string): boolean {
    if (requestId !== this.activeRequestId) return false;
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
    this.clearRequestTimeout(request);
    if (!ok) {
      request.controller.abort(new DOMException("Media focus operation cancelled", "AbortError"));
    }
    const wasActive = this.activeRequestId === requestId;
    if (wasActive) this.activeRequestId = null;
    request.resolve(ok);
    if (wasActive) this.activateNext();
    return true;
  }

  private activateNext(): void {
    if (this.activeRequestId !== null) return;
    while (this.queue.length > 0) {
      const requestId = this.queue.shift()!;
      const request = this.pending.get(requestId);
      if (!request) continue;
      this.activeRequestId = requestId;
      request.stage = "awaiting";
      this.armRequestTimeout(requestId, request);
      try {
        request.deliver(request.command);
      } catch {
        this.settle(requestId, false);
      }
      return;
    }
  }

  private armRequestTimeout(
    requestId: string,
    request: PendingMediaFocus,
    timeoutMs = this.timeoutMs
  ): void {
    this.clearRequestTimeout(request);
    request.timeout = setTimeout(() => this.cancelAndSettle(requestId), timeoutMs);
  }

  private clearRequestTimeout(request: PendingMediaFocus): void {
    if (request.timeout === null) return;
    clearTimeout(request.timeout);
    request.timeout = null;
  }
}
