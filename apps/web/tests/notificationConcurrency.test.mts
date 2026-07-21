import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { DesktopProcessLogBatch } from "@megle/notification-contract";
import {
  beginNotificationAction,
  notificationActionKey,
  settleNotificationAction,
  type NotificationActionStates
} from "../src/features/notifications/notificationActionModel.ts";
import {
  coalesceProcessLogBatches,
  isCurrentProcessLogPageRequest,
  processLogFilterIdentity
} from "../src/features/notifications/processLogModel.ts";

describe("notification action state", () => {
  it("keeps concurrent operation actions independent and rejects duplicate pending starts", () => {
    let states: NotificationActionStates = new Map();
    const cancelA = notificationActionKey("library-a", "operation-a", "cancel");
    const retryB = notificationActionKey("library-a", "operation-b", "retry");

    const startedA = beginNotificationAction(states, cancelA, 1);
    assert.equal(startedA.started, true);
    states = startedA.states;
    const duplicateA = beginNotificationAction(states, cancelA, 2);
    assert.equal(duplicateA.started, false);
    assert.equal(duplicateA.states, states);

    const startedB = beginNotificationAction(states, retryB, 3);
    assert.equal(startedB.started, true);
    states = startedB.states;
    states = settleNotificationAction(states, cancelA, 1, "cancel failed");
    assert.deepEqual(states.get(cancelA), {
      requestId: 1,
      pending: false,
      error: "cancel failed"
    });
    assert.equal(states.get(retryB)?.pending, true);

    const staleA = settleNotificationAction(states, cancelA, 99, null);
    assert.equal(staleA, states);
    const clearedAfterLibrarySwitch: NotificationActionStates = new Map();
    assert.equal(
      settleNotificationAction(clearedAfterLibrarySwitch, retryB, 3, "late failure"),
      clearedAfterLibrarySwitch
    );
    states = settleNotificationAction(states, retryB, 3, null);
    assert.equal(states.has(retryB), false);
    assert.equal(states.get(cancelA)?.error, "cancel failed");
  });
});

describe("process log request lifecycle", () => {
  it("rejects an older-page response after the active filter or generation changes", () => {
    const coreQuery = processLogFilterIdentity({ source: "core", level: "warn", limit: 200 });
    const mainQuery = processLogFilterIdentity({ source: "main", level: "warn", limit: 200 });
    assert.equal(
      isCurrentProcessLogPageRequest(
        { queryIdentity: coreQuery, generation: 4 },
        { queryIdentity: coreQuery, generation: 4 }
      ),
      true
    );
    assert.equal(
      isCurrentProcessLogPageRequest(
        { queryIdentity: coreQuery, generation: 4 },
        { queryIdentity: mainQuery, generation: 4 }
      ),
      false
    );
    assert.equal(
      isCurrentProcessLogPageRequest(
        { queryIdentity: coreQuery, generation: 4 },
        { queryIdentity: coreQuery, generation: 5 }
      ),
      false
    );
  });

  it("coalesces two thousand paused batches into one bounded ordered delivery", () => {
    const batches: DesktopProcessLogBatch[] = Array.from({ length: 2_000 }, (_, index) => ({
      lines: [
        {
          sequence: index + 1,
          timestamp: index,
          source: "core",
          level: "info",
          message: `line ${index + 1}`
        }
      ],
      afterSequence: index + 1,
      droppedBeforeSequence: index === 800 ? 700 : null
    }));
    const merged = coalesceProcessLogBatches(batches, 5_000, 650);

    assert.equal(merged.lines.length, 2_000);
    assert.equal(merged.lines[0]?.sequence, 1);
    assert.equal(merged.lines.at(-1)?.sequence, 2_000);
    assert.equal(merged.afterSequence, 2_000);
    assert.equal(merged.droppedBeforeSequence, 700);
  });
});
