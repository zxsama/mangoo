export type CoreSwitchPhase =
  | "initial"
  | "detaching-previous"
  | "previous-detached"
  | "stopping-previous"
  | "previous-stopped"
  | "starting-target"
  | "target-started"
  | "committing-target"
  | "target-committed"
  | "attaching-target"
  | "target-attached"
  | "recovering-previous"
  | "previous-recovered";

export type CoreSwitchTransactionResult<TSession> = {
  cancelled: boolean;
  outcome: "target" | "previous" | "target-fallback";
  session: TSession;
};

export type CoreSwitchRecoveryStage =
  | "notification-detach"
  | "start-or-health"
  | "commit"
  | "notification-attach";

export class CoreSwitchRecoveryError extends Error {
  readonly recoveryStage: CoreSwitchRecoveryStage;
  readonly systemHealthy: boolean;
  readonly healthySession: unknown | null;
  readonly operationError: unknown;
  readonly recoveryError: unknown;

  constructor(options: {
    recoveryStage: CoreSwitchRecoveryStage;
    systemHealthy: boolean;
    healthySession?: unknown | null;
    operationError: unknown;
    recoveryError: unknown;
  }) {
    const recoveryMessage =
      options.recoveryError instanceof Error
        ? options.recoveryError.message
        : String(options.recoveryError);
    super(`Library switch recovery failed during ${options.recoveryStage}: ${recoveryMessage}`);
    this.name = "CoreSwitchRecoveryError";
    this.recoveryStage = options.recoveryStage;
    this.systemHealthy = options.systemHealthy;
    this.healthySession = options.healthySession ?? null;
    this.operationError = options.operationError;
    this.recoveryError = options.recoveryError;
  }
}

export interface CoreSwitchTransactionOptions<TSession> {
  previousSession: TSession | null;
  assertLifecycleCurrent(): void;
  isCancellationRequested(): boolean;
  detachNotifications(): Promise<void>;
  attachNotifications(session: TSession): Promise<void>;
  stopCurrentCore(): Promise<void>;
  startTargetCore(): Promise<TSession>;
  commitTargetCore(session: TSession): void | Promise<void>;
  recoverPreviousCore(): Promise<TSession>;
  commitPreviousCore(session: TSession): void | Promise<void>;
  onPhase?(phase: CoreSwitchPhase): void;
}

export interface HealthyCoreRetryOptions<T> {
  maxAttempts: number;
  assertLifecycleCurrent(): void;
  start(attempt: number): Promise<T>;
  cleanupFailedAttempt(attempt: number, error: unknown): Promise<void>;
  onRetry?(attempt: number, error: unknown): void;
}

export async function startHealthyCoreWithRetry<T>(
  options: HealthyCoreRetryOptions<T>
): Promise<T> {
  const maxAttempts = Math.max(1, Math.floor(options.maxAttempts));
  let lastError: unknown;
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    options.assertLifecycleCurrent();
    try {
      const value = await options.start(attempt);
      options.assertLifecycleCurrent();
      return value;
    } catch (error) {
      lastError = error;
      await options.cleanupFailedAttempt(attempt, error);
      options.assertLifecycleCurrent();
      if (attempt < maxAttempts) options.onRetry?.(attempt, error);
    }
  }
  throw lastError ?? new Error("Core startup retry loop exited without an error");
}

export async function runCoreSwitchTransaction<TSession>(
  options: CoreSwitchTransactionOptions<TSession>
): Promise<CoreSwitchTransactionResult<TSession>> {
  let destructive = false;
  let notificationsMayBeDetached = false;
  let recoveryAttempted = false;
  let targetSession: TSession | null = null;
  let targetCommitted = false;
  let targetNotificationsAttached = false;

  const enter = (phase: CoreSwitchPhase) => {
    options.onPhase?.(phase);
  };
  const recoveryFailure = (
    recoveryStage: CoreSwitchRecoveryStage,
    operationError: unknown,
    recoveryError: unknown,
    systemHealthy = false,
    healthySession: TSession | null = null
  ): CoreSwitchRecoveryError =>
    new CoreSwitchRecoveryError({
      recoveryStage,
      systemHealthy,
      healthySession,
      operationError,
      recoveryError
    });
  const restorePrevious = async (operationError: unknown): Promise<TSession> => {
    const previousSession = options.previousSession;
    if (previousSession === null) {
      throw recoveryFailure(
        "start-or-health",
        operationError,
        new Error("Core switch cannot recover without a previous session")
      );
    }
    recoveryAttempted = true;
    options.assertLifecycleCurrent();
    let restoredSession: TSession = previousSession as TSession;
    if (destructive) {
      enter("recovering-previous");
      try {
        await options.detachNotifications();
      } catch (error) {
        throw recoveryFailure("notification-detach", operationError, error);
      }
      options.assertLifecycleCurrent();
      try {
        restoredSession = await options.recoverPreviousCore();
      } catch (error) {
        throw recoveryFailure("start-or-health", operationError, error);
      }
      options.assertLifecycleCurrent();
      try {
        await options.commitPreviousCore(restoredSession);
      } catch (error) {
        throw recoveryFailure("commit", operationError, error);
      }
      options.assertLifecycleCurrent();
    }
    try {
      await options.attachNotifications(restoredSession);
    } catch (error) {
      throw recoveryFailure(
        "notification-attach",
        operationError,
        error,
        true,
        restoredSession
      );
    }
    options.assertLifecycleCurrent();
    enter("previous-recovered");
    return restoredSession;
  };
  const keepHealthyTarget = async (
    operationError: unknown
  ): Promise<CoreSwitchTransactionResult<TSession>> => {
    if (targetSession === null) {
      const restoredSession = await restorePrevious(operationError);
      return { cancelled: true, outcome: "previous", session: restoredSession };
    }
    if (!targetCommitted) {
      try {
        await options.commitTargetCore(targetSession);
        targetCommitted = true;
      } catch (error) {
        const restoredSession = await restorePrevious(error);
        return { cancelled: true, outcome: "previous", session: restoredSession };
      }
    }
    if (!targetNotificationsAttached) {
      try {
        await options.attachNotifications(targetSession);
        targetNotificationsAttached = true;
      } catch (error) {
        throw recoveryFailure(
          "notification-attach",
          operationError,
          error,
          true,
          targetSession
        );
      }
    }
    return { cancelled: true, outcome: "target-fallback", session: targetSession };
  };
  const cancelAtSafeBoundary = async (): Promise<CoreSwitchTransactionResult<TSession> | null> => {
    if (!options.isCancellationRequested() || options.previousSession === null) return null;
    if (!destructive && !notificationsMayBeDetached) {
      return { cancelled: true, outcome: "previous", session: options.previousSession };
    }
    return keepHealthyTarget(new Error("Library switch operation was cancelled"));
  };

  enter("initial");
  const initiallyCancelled = await cancelAtSafeBoundary();
  if (initiallyCancelled) return initiallyCancelled;

  try {
    options.assertLifecycleCurrent();
    notificationsMayBeDetached = true;
    enter("detaching-previous");
    await options.detachNotifications();
    options.assertLifecycleCurrent();
    enter("previous-detached");
    const cancelledAfterDetach = await cancelAtSafeBoundary();
    if (cancelledAfterDetach) return cancelledAfterDetach;

    destructive = true;
    enter("stopping-previous");
    await options.stopCurrentCore();
    options.assertLifecycleCurrent();
    enter("previous-stopped");
    const cancelledAfterStop = await cancelAtSafeBoundary();
    if (cancelledAfterStop) return cancelledAfterStop;

    enter("starting-target");
    targetSession = await options.startTargetCore();
    options.assertLifecycleCurrent();
    enter("target-started");
    const cancelledAfterStart = await cancelAtSafeBoundary();
    if (cancelledAfterStart) return cancelledAfterStart;

    enter("committing-target");
    await options.commitTargetCore(targetSession);
    targetCommitted = true;
    options.assertLifecycleCurrent();
    enter("target-committed");
    const cancelledAfterCommit = await cancelAtSafeBoundary();
    if (cancelledAfterCommit) return cancelledAfterCommit;

    enter("attaching-target");
    try {
      await options.attachNotifications(targetSession);
      targetNotificationsAttached = true;
    } catch (error) {
      throw recoveryFailure("notification-attach", error, error, true, targetSession);
    }
    options.assertLifecycleCurrent();
    enter("target-attached");
    const cancelledAfterAttach = await cancelAtSafeBoundary();
    if (cancelledAfterAttach) return cancelledAfterAttach;
    return { cancelled: false, outcome: "target", session: targetSession };
  } catch (error) {
    if (error instanceof CoreSwitchRecoveryError) throw error;
    if (
      options.previousSession !== null &&
      !recoveryAttempted &&
      (destructive || notificationsMayBeDetached)
    ) {
      const restoredSession = await restorePrevious(error);
      if (options.isCancellationRequested()) {
        return { cancelled: true, outcome: "previous", session: restoredSession };
      }
    }
    throw error;
  }
}
