import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  runCoreSwitchTransaction,
  startHealthyCoreWithRetry,
  type CoreSwitchPhase
} from "../src/core-switch-transaction.ts";

type LibraryId = "previous" | "target";

function createHarness(cancelAt: CoreSwitchPhase | "during-detach" | "during-stop" | null) {
  const cancellation = new AbortController();
  const events: string[] = [];
  let registry: LibraryId = "previous";
  let core: LibraryId | null = "previous";
  let pending: LibraryId | null = null;
  let notifications: LibraryId | null = "previous";

  const cancel = () => cancellation.abort(new DOMException("focus obsolete", "AbortError"));
  const options = {
    previousSession: "previous" as LibraryId,
    assertLifecycleCurrent: () => undefined,
    isCancellationRequested: () => cancellation.signal.aborted,
    onPhase: (phase: CoreSwitchPhase) => {
      events.push(phase);
      if (cancelAt === phase) cancel();
    },
    detachNotifications: async () => {
      notifications = null;
      if (cancelAt === "during-detach") cancel();
    },
    attachNotifications: async (session: LibraryId) => {
      assert.equal(core, session, "notifications must attach to the healthy Core");
      assert.equal(registry, session, "notifications must attach to the registered library");
      notifications = session;
    },
    stopCurrentCore: async () => {
      core = null;
      pending = null;
      if (cancelAt === "during-stop") cancel();
    },
    startTargetCore: async () => {
      pending = "target";
      return "target" as LibraryId;
    },
    commitTargetCore: async (session: LibraryId) => {
      assert.equal(pending, session);
      core = session;
      pending = null;
      registry = session;
    },
    recoverPreviousCore: async () => {
      core = null;
      pending = "previous";
      return "previous" as LibraryId;
    },
    commitPreviousCore: async (session: LibraryId) => {
      assert.equal(pending, session);
      core = session;
      pending = null;
      registry = session;
    }
  };

  return {
    events,
    options,
    snapshot: () => ({ core, notifications, pending, registry })
  };
}

describe("Core switch transaction cancellation", () => {
  for (const cancelAt of [
    "during-detach",
    "previous-detached",
    "during-stop",
    "previous-stopped",
    "target-started",
    "target-attached"
  ] as const) {
    it(`restores a healthy consistent previous library when cancelled at ${cancelAt}`, async () => {
      const harness = createHarness(cancelAt);
      const result = await runCoreSwitchTransaction(harness.options);

      assert.deepEqual(result, { cancelled: true, session: "previous" });
      assert.deepEqual(harness.snapshot(), {
        core: "previous",
        notifications: "previous",
        pending: null,
        registry: "previous"
      });

      const retry = createHarness(null);
      assert.deepEqual(await runCoreSwitchTransaction(retry.options), {
        cancelled: false,
        session: "target"
      });
      assert.deepEqual(retry.snapshot(), {
        core: "target",
        notifications: "target",
        pending: null,
        registry: "target"
      });
    });
  }

  it("retries candidate startup with bounded cleanup before committing", async () => {
    const attempts: number[] = [];
    const cleanups: number[] = [];
    const result = await startHealthyCoreWithRetry({
      maxAttempts: 3,
      assertLifecycleCurrent: () => undefined,
      start: async (attempt) => {
        attempts.push(attempt);
        if (attempt < 3) throw new Error(`health-${attempt}`);
        return "healthy";
      },
      cleanupFailedAttempt: async (attempt) => {
        cleanups.push(attempt);
      }
    });

    assert.equal(result, "healthy");
    assert.deepEqual(attempts, [1, 2, 3]);
    assert.deepEqual(cleanups, [1, 2]);
  });

  it("cleans every failed candidate when the bounded startup budget is exhausted", async () => {
    const attempts: number[] = [];
    const cleanups: number[] = [];

    await assert.rejects(
      startHealthyCoreWithRetry({
        maxAttempts: 3,
        assertLifecycleCurrent: () => undefined,
        start: async (attempt) => {
          attempts.push(attempt);
          throw new Error(`health-${attempt}`);
        },
        cleanupFailedAttempt: async (attempt) => {
          cleanups.push(attempt);
        }
      }),
      /health-3/
    );

    assert.deepEqual(attempts, [1, 2, 3]);
    assert.deepEqual(cleanups, [1, 2, 3]);
  });

  it("restores the previous committed library when target startup fails", async () => {
    const harness = createHarness(null);
    const options = {
      ...harness.options,
      startTargetCore: async () => {
        throw new Error("target health check failed");
      }
    };

    await assert.rejects(runCoreSwitchTransaction(options), /target health check failed/);
    assert.deepEqual(harness.snapshot(), {
      core: "previous",
      notifications: "previous",
      pending: null,
      registry: "previous"
    });
  });
});
