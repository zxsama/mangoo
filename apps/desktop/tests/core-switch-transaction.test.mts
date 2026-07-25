import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  CoreSwitchRecoveryError,
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
    cancel,
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
    "previous-stopped"
  ] as const) {
    it(`restores a healthy consistent previous library when cancelled at ${cancelAt}`, async () => {
      const harness = createHarness(cancelAt);
      const result = await runCoreSwitchTransaction(harness.options);

      assert.deepEqual(result, {
        cancelled: true,
        outcome: "previous",
        session: "previous"
      });
      assert.deepEqual(harness.snapshot(), {
        core: "previous",
        notifications: "previous",
        pending: null,
        registry: "previous"
      });

      const retry = createHarness(null);
      assert.deepEqual(await runCoreSwitchTransaction(retry.options), {
        cancelled: false,
        outcome: "target",
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

  for (const cancelAt of ["target-started", "target-attached"] as const) {
    it(`keeps the only healthy target as a coherent fallback when cancelled at ${cancelAt}`, async () => {
      const harness = createHarness(cancelAt);
      const result = await runCoreSwitchTransaction(harness.options);

      assert.deepEqual(result, {
        cancelled: true,
        outcome: "target-fallback",
        session: "target"
      });
      assert.deepEqual(harness.snapshot(), {
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

  it("returns a recovered cancellation outcome when target startup fails after cancellation", async () => {
    const harness = createHarness(null);
    const options = {
      ...harness.options,
      startTargetCore: async () => {
        harness.cancel();
        throw new Error("target health check failed after cancellation");
      }
    };

    assert.deepEqual(await runCoreSwitchTransaction(options), {
      cancelled: true,
      outcome: "previous",
      session: "previous"
    });
    assert.deepEqual(harness.snapshot(), {
      core: "previous",
      notifications: "previous",
      pending: null,
      registry: "previous"
    });
  });

  for (const failure of ["start", "health"] as const) {
    it(`surfaces an independent recovery ${failure} failure after cancellation`, async () => {
      const harness = createHarness("previous-stopped");
      const options = {
        ...harness.options,
        recoverPreviousCore: async () => {
          throw new Error(`previous ${failure} failed`);
        }
      };

      await assert.rejects(runCoreSwitchTransaction(options), (error: unknown) => {
        assert.ok(error instanceof CoreSwitchRecoveryError);
        assert.equal(error.recoveryStage, "start-or-health");
        assert.equal(error.systemHealthy, false);
        assert.match(error.message, new RegExp(`previous ${failure} failed`));
        return true;
      });
    });
  }

  it("surfaces an independent recovery notification detach failure after cancellation", async () => {
    const harness = createHarness("previous-stopped");
    let detachCalls = 0;
    const options = {
      ...harness.options,
      detachNotifications: async () => {
        detachCalls += 1;
        if (detachCalls > 1) throw new Error("recovery notification detach failed");
      }
    };

    await assert.rejects(runCoreSwitchTransaction(options), (error: unknown) => {
      assert.ok(error instanceof CoreSwitchRecoveryError);
      assert.equal(error.recoveryStage, "notification-detach");
      assert.equal(error.systemHealthy, false);
      assert.match(error.message, /recovery notification detach failed/);
      return true;
    });
  });

  it("surfaces an independent recovery commit failure after cancellation", async () => {
    const harness = createHarness("previous-stopped");
    const options = {
      ...harness.options,
      commitPreviousCore: async () => {
        throw new Error("previous commit failed");
      }
    };

    await assert.rejects(runCoreSwitchTransaction(options), (error: unknown) => {
      assert.ok(error instanceof CoreSwitchRecoveryError);
      assert.equal(error.recoveryStage, "commit");
      assert.equal(error.systemHealthy, false);
      assert.match(error.message, /previous commit failed/);
      return true;
    });
  });

  it("surfaces recovery notification failure while retaining the healthy committed Core", async () => {
    const harness = createHarness("previous-stopped");
    const options = {
      ...harness.options,
      attachNotifications: async (session: LibraryId) => {
        if (session === "previous") throw new Error("previous notification attach failed");
      }
    };

    await assert.rejects(runCoreSwitchTransaction(options), (error: unknown) => {
      assert.ok(error instanceof CoreSwitchRecoveryError);
      assert.equal(error.recoveryStage, "notification-attach");
      assert.equal(error.systemHealthy, true);
      assert.equal(error.healthySession, "previous");
      assert.match(error.message, /previous notification attach failed/);
      return true;
    });
  });
});
