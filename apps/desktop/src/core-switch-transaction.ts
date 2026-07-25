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
  session: TSession;
};

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

  const enter = (phase: CoreSwitchPhase) => {
    options.onPhase?.(phase);
  };
  const restorePrevious = async (): Promise<TSession> => {
    const previousSession = options.previousSession;
    if (previousSession === null) {
      throw new Error("Core switch cannot recover without a previous session");
    }
    recoveryAttempted = true;
    options.assertLifecycleCurrent();
    let restoredSession: TSession = previousSession as TSession;
    if (destructive) {
      enter("recovering-previous");
      await options.detachNotifications();
      options.assertLifecycleCurrent();
      restoredSession = await options.recoverPreviousCore();
      options.assertLifecycleCurrent();
      await options.commitPreviousCore(restoredSession);
      options.assertLifecycleCurrent();
    }
    await options.attachNotifications(restoredSession);
    options.assertLifecycleCurrent();
    enter("previous-recovered");
    return restoredSession;
  };
  const cancelAtSafeBoundary = async (): Promise<CoreSwitchTransactionResult<TSession> | null> => {
    if (!options.isCancellationRequested() || options.previousSession === null) return null;
    if (!destructive && !notificationsMayBeDetached) {
      return { cancelled: true, session: options.previousSession };
    }
    return { cancelled: true, session: await restorePrevious() };
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
    const targetSession = await options.startTargetCore();
    options.assertLifecycleCurrent();
    enter("target-started");
    const cancelledAfterStart = await cancelAtSafeBoundary();
    if (cancelledAfterStart) return cancelledAfterStart;

    enter("committing-target");
    await options.commitTargetCore(targetSession);
    options.assertLifecycleCurrent();
    enter("target-committed");
    const cancelledAfterCommit = await cancelAtSafeBoundary();
    if (cancelledAfterCommit) return cancelledAfterCommit;

    enter("attaching-target");
    await options.attachNotifications(targetSession);
    options.assertLifecycleCurrent();
    enter("target-attached");
    const cancelledAfterAttach = await cancelAtSafeBoundary();
    if (cancelledAfterAttach) return cancelledAfterAttach;
    return { cancelled: false, session: targetSession };
  } catch (error) {
    if (
      options.previousSession !== null &&
      !recoveryAttempted &&
      (destructive || notificationsMayBeDetached)
    ) {
      try {
        await restorePrevious();
      } catch (recoveryError) {
        throw new AggregateError(
          [error, recoveryError],
          "Core switch failed and the previous library could not be restored"
        );
      }
    }
    throw error;
  }
}
