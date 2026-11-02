import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { DesktopProcessLogBatch } from "@mangoo/notification-contract";
import {
  beginNotificationAction,
  notificationOperationActionKey,
  settleNotificationAction,
  type NotificationActionStates
} from "../src/features/notifications/notificationActionModel.ts";
import {
  MAX_PAUSED_LOG_LINES,
  appendPausedProcessLogBatch,
  coalescePausedProcessLogBuffer,
  createPausedProcessLogBuffer,
  createProcessLogPageRequestController,
  createProcessLogQueryToken
} from "../src/features/notifications/processLogModel.ts";

describe("notification action state", () => {
  it("serializes cancel and retry per operation while keeping errors action-specific", () => {
    let states: NotificationActionStates = new Map();
    const operationA = notificationOperationActionKey("library-a", "operation-a");
    const operationB = notificationOperationActionKey("library-a", "operation-b");

    const startedA = beginNotificationAction(states, operationA, "cancel", 1);
    assert.equal(startedA.started, true);
    states = startedA.states;
    const blockedRetryA = beginNotificationAction(states, operationA, "retry", 2);
    assert.equal(blockedRetryA.started, false, "retry must not overlap cancel on one operation");
    assert.equal(blockedRetryA.states, states);

    const startedB = beginNotificationAction(states, operationB, "retry", 3);
    assert.equal(startedB.started, true);
    states = startedB.states;
    states = settleNotificationAction(states, operationA, "cancel", 1, "cancel failed");
    assert.deepEqual(states.get(operationA), {
      requestId: 1,
      pendingAction: null,
      errors: { cancel: "cancel failed" }
    });
    assert.equal(states.get(operationB)?.pendingAction, "retry");

    const retryA = beginNotificationAction(states, operationA, "retry", 4);
    assert.equal(retryA.started, true);
    states = retryA.states;
    states = settleNotificationAction(states, operationA, "retry", 4, "retry failed");
    assert.deepEqual(states.get(operationA)?.errors, {
      cancel: "cancel failed",
      retry: "retry failed"
    });

    const staleA = settleNotificationAction(states, operationA, "cancel", 99, null);
    assert.equal(staleA, states);
    const clearedAfterLibrarySwitch: NotificationActionStates = new Map();
    assert.equal(
      settleNotificationAction(
        clearedAfterLibrarySwitch,
        operationB,
        "retry",
        3,
        "late failure"
      ),
      clearedAfterLibrarySwitch
    );
    states = settleNotificationAction(states, operationB, "retry", 3, null);
    assert.equal(states.has(operationB), false);
    assert.equal(states.get(operationA)?.errors.cancel, "cancel failed");
    assert.equal(states.get(operationA)?.errors.retry, "retry failed");
  });
});

describe("process log request lifecycle", () => {
  it("commits immutable query tokens without mutating lifecycle during render", () => {
    const coreToken = createProcessLogQueryToken({ source: "core", level: "warn", limit: 200 });
    const controller = createProcessLogPageRequestController(coreToken);
    const coreRequest = controller.begin(coreToken);
    assert.ok(coreRequest);
    assert.equal(controller.isCurrent(coreRequest), true);
    assert.equal(controller.finish(coreRequest), true);
    assert.equal(controller.finish(coreRequest), false, "every request can release loading once only");

    const uncommittedMainToken = createProcessLogQueryToken({
      source: "main",
      level: "warn",
      limit: 200
    });
    const interruptedRenderRequest = controller.begin(uncommittedMainToken);
    assert.ok(interruptedRenderRequest);
    assert.equal(
      controller.isCurrent(interruptedRenderRequest),
      false,
      "creating a render token must not commit it"
    );
    assert.equal(controller.finish(interruptedRenderRequest), true);

    controller.commitQuery(uncommittedMainToken);
    const mainRequest = controller.begin(uncommittedMainToken);
    assert.ok(mainRequest);
    assert.equal(controller.isCurrent(mainRequest), true);
    controller.invalidate();
    assert.equal(controller.isCurrent(mainRequest), false);
    assert.equal(controller.finish(mainRequest), false, "invalidation releases the active request");
  });

  it("keeps the newest exact 1000 lines from 1001-to-5000-line paused batches", () => {
    assert.equal(MAX_PAUSED_LOG_LINES, 1_000);
    for (const count of [1_001, 5_000]) {
      const oversized = logBatch(1, count, null);
      const buffered = appendPausedProcessLogBatch(createPausedProcessLogBuffer(), oversized);
      const merged = coalescePausedProcessLogBuffer(buffered);

      assert.equal(buffered.lineCount, MAX_PAUSED_LOG_LINES);
      assert.equal(merged.lines.length, MAX_PAUSED_LOG_LINES);
      assert.equal(merged.lines[0]?.sequence, count - MAX_PAUSED_LOG_LINES + 1);
      assert.equal(merged.lines.at(-1)?.sequence, count);
      assert.equal(merged.afterSequence, count);
      assert.equal(merged.droppedBeforeSequence, count - MAX_PAUSED_LOG_LINES);
    }
  });

  it("trims multiple paused batches by line and preserves the newest real gap", () => {
    let buffered = createPausedProcessLogBuffer();
    buffered = appendPausedProcessLogBatch(buffered, logBatch(1, 750, 300));
    buffered = appendPausedProcessLogBatch(buffered, logBatch(751, 750, 420));
    const merged = coalescePausedProcessLogBuffer(buffered);

    assert.equal(buffered.lineCount, MAX_PAUSED_LOG_LINES);
    assert.equal(merged.lines.length, MAX_PAUSED_LOG_LINES);
    assert.equal(merged.lines[0]?.sequence, 501);
    assert.equal(merged.lines.at(-1)?.sequence, 1_500);
    assert.equal(merged.droppedBeforeSequence, 500);
  });

  it("coalesces ten thousand empty gap batches without growing the batch queue", () => {
    let buffered = createPausedProcessLogBuffer();
    for (let index = 0; index < 10_000; index += 1) {
      buffered = appendPausedProcessLogBatch(buffered, {
        lines: [],
        afterSequence: index + 1,
        droppedBeforeSequence: index
      });
    }
    const merged = coalescePausedProcessLogBuffer(buffered);

    assert.equal(buffered.batches.length, 0);
    assert.equal(buffered.lineCount, 0);
    assert.equal(merged.lines.length, 0);
    assert.equal(merged.afterSequence, 10_000);
    assert.equal(merged.droppedBeforeSequence, 9_999);
  });

  it("keeps line batches bounded while coalescing intervening empty gap metadata", () => {
    let buffered = appendPausedProcessLogBatch(
      createPausedProcessLogBuffer(),
      logBatch(1, 600, null)
    );
    for (let index = 1; index <= 10_000; index += 1) {
      buffered = appendPausedProcessLogBatch(buffered, {
        lines: [],
        afterSequence: 600 + index,
        droppedBeforeSequence: 500 + index
      });
    }
    buffered = appendPausedProcessLogBatch(buffered, logBatch(10_601, 600, null));
    const merged = coalescePausedProcessLogBuffer(buffered);

    assert.equal(buffered.batches.length, 2);
    assert.equal(buffered.lineCount, MAX_PAUSED_LOG_LINES);
    assert.equal(merged.lines.length, MAX_PAUSED_LOG_LINES);
    assert.equal(merged.lines[0]?.sequence, 201);
    assert.equal(merged.lines.at(-1)?.sequence, 11_200);
    assert.equal(merged.afterSequence, 11_200);
    assert.equal(merged.droppedBeforeSequence, 10_500);
  });
});

function logBatch(
  firstSequence: number,
  count: number,
  droppedBeforeSequence: number | null
): DesktopProcessLogBatch {
  return {
    lines: Array.from({ length: count }, (_, index) => ({
      sequence: firstSequence + index,
      timestamp: firstSequence + index,
      source: "core",
      level: "info",
      message: `line ${firstSequence + index}`
    })),
    afterSequence: firstSequence + count - 1,
    droppedBeforeSequence
  };
}
