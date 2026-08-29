export const OAUTH_RETURN_FOCUS_ENTITLEMENT_TTL_MS = 10 * 60 * 1_000;

interface OAuthReturnFocusEntitlement {
  expiresAt: number;
  generation: number;
  timeout: ReturnType<typeof setTimeout>;
}

export interface OAuthReturnFocusWindow {
  isDestroyed(): boolean;
  isMinimized(): boolean;
  restore(): void;
  show(): void;
  focus(): void;
}

export interface OAuthReturnFocusEntitlementsOptions {
  ttlMs?: number;
  now?: () => number;
  schedule?: (callback: () => void, delayMs: number) => ReturnType<typeof setTimeout>;
  cancelSchedule?: (timeout: ReturnType<typeof setTimeout>) => void;
}

export class OAuthReturnFocusEntitlements {
  private readonly entries = new Map<number, OAuthReturnFocusEntitlement>();
  private readonly ttlMs: number;
  private readonly now: () => number;
  private readonly schedule: (
    callback: () => void,
    delayMs: number
  ) => ReturnType<typeof setTimeout>;
  private readonly cancelSchedule: (timeout: ReturnType<typeof setTimeout>) => void;
  private nextGeneration = 1;

  constructor(options: OAuthReturnFocusEntitlementsOptions = {}) {
    this.ttlMs = options.ttlMs ?? OAUTH_RETURN_FOCUS_ENTITLEMENT_TTL_MS;
    this.now = options.now ?? Date.now;
    this.schedule = options.schedule ?? setTimeout;
    this.cancelSchedule = options.cancelSchedule ?? clearTimeout;
  }

  grant(senderId: number): void {
    this.revoke(senderId);
    const generation = this.nextGeneration++;
    const expiresAt = this.now() + this.ttlMs;
    const timeout = this.schedule(() => {
      const current = this.entries.get(senderId);
      if (current?.generation === generation) this.entries.delete(senderId);
    }, this.ttlMs);
    this.entries.set(senderId, { expiresAt, generation, timeout });
  }

  consume(senderId: number): boolean {
    const entitlement = this.entries.get(senderId);
    if (!entitlement) return false;
    this.entries.delete(senderId);
    this.cancelSchedule(entitlement.timeout);
    return entitlement.expiresAt > this.now();
  }

  revoke(senderId: number): void {
    const entitlement = this.entries.get(senderId);
    if (!entitlement) return;
    this.entries.delete(senderId);
    this.cancelSchedule(entitlement.timeout);
  }

  clear(): void {
    for (const entitlement of this.entries.values()) {
      this.cancelSchedule(entitlement.timeout);
    }
    this.entries.clear();
  }

  get size(): number {
    return this.entries.size;
  }
}

export function restoreOAuthReturnFocus(window: OAuthReturnFocusWindow): boolean {
  if (window.isDestroyed()) return false;
  if (window.isMinimized()) window.restore();
  window.show();
  window.focus();
  return true;
}
