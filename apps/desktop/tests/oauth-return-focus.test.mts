import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  OAuthReturnFocusEntitlements,
  restoreOAuthReturnFocus
} from "../src/oauth-return-focus.ts";

function createEntitlements(ttlMs = 1_000) {
  let now = 100;
  let nextTimeout = 1;
  const callbacks = new Map<number, () => void>();
  const cancelled: number[] = [];
  const entitlements = new OAuthReturnFocusEntitlements({
    ttlMs,
    now: () => now,
    schedule: (callback) => {
      const id = nextTimeout++;
      callbacks.set(id, callback);
      return id as unknown as ReturnType<typeof setTimeout>;
    },
    cancelSchedule: (timeout) => {
      const id = timeout as unknown as number;
      cancelled.push(id);
      callbacks.delete(id);
    }
  });
  return {
    entitlements,
    advance(ms: number) {
      now += ms;
    },
    expire(id: number) {
      callbacks.get(id)?.();
      callbacks.delete(id);
    },
    cancelled
  };
}

describe("OAuth return-focus entitlements", () => {
  it("is sender-bound and consumes a successful grant exactly once", () => {
    const harness = createEntitlements();
    harness.entitlements.grant(11);
    assert.equal(harness.entitlements.consume(12), false);
    assert.equal(harness.entitlements.consume(11), true);
    assert.equal(harness.entitlements.consume(11), false);
    assert.equal(harness.entitlements.size, 0);
  });

  it("expires grants and removes them when the expiry callback runs", () => {
    const lazyExpiry = createEntitlements();
    lazyExpiry.entitlements.grant(11);
    lazyExpiry.advance(1_001);
    assert.equal(lazyExpiry.entitlements.consume(11), false);

    const scheduledExpiry = createEntitlements();
    scheduledExpiry.entitlements.grant(22);
    scheduledExpiry.expire(1);
    assert.equal(scheduledExpiry.entitlements.size, 0);
    assert.equal(scheduledExpiry.entitlements.consume(22), false);
  });

  it("replacement and lifecycle revocation cancel stale expiry work", () => {
    const harness = createEntitlements();
    harness.entitlements.grant(11);
    harness.entitlements.grant(11);
    assert.deepEqual(harness.cancelled, [1]);
    harness.expire(1);
    assert.equal(harness.entitlements.consume(11), true);

    harness.entitlements.grant(12);
    harness.entitlements.revoke(12);
    assert.equal(harness.entitlements.consume(12), false);
  });
});

it("restores, shows, and focuses the exact live originating window in order", () => {
  const calls: string[] = [];
  const window = {
    isDestroyed: () => false,
    isMinimized: () => true,
    restore: () => calls.push("restore"),
    show: () => calls.push("show"),
    focus: () => calls.push("focus")
  };
  assert.equal(restoreOAuthReturnFocus(window), true);
  assert.deepEqual(calls, ["restore", "show", "focus"]);

  assert.equal(restoreOAuthReturnFocus({
    ...window,
    isDestroyed: () => true
  }), false);
  assert.deepEqual(calls, ["restore", "show", "focus"]);
});
