export interface NotificationSafeCoreTransitionOptions<TSession> {
  assertCurrent(): void;
  detachNotifications(): Promise<void>;
  stopOldCore(): Promise<void>;
  startHealthyCore(): Promise<TSession>;
  commitActiveLibrary(session: TSession): void | Promise<void>;
  attachNotifications(session: TSession): Promise<void>;
}

export async function runNotificationSafeCoreTransition<TSession>(
  options: NotificationSafeCoreTransitionOptions<TSession>
): Promise<TSession> {
  options.assertCurrent();
  await options.detachNotifications();

  try {
    options.assertCurrent();
    await options.stopOldCore();
    options.assertCurrent();
    const session = await options.startHealthyCore();
    options.assertCurrent();
    await options.commitActiveLibrary(session);
    options.assertCurrent();
    await options.attachNotifications(session);
    options.assertCurrent();
    return session;
  } catch (error) {
    try {
      await options.detachNotifications();
    } catch (detachError) {
      throw new AggregateError(
        [error, detachError],
        "Core transition failed and notifications could not be detached"
      );
    }
    throw error;
  }
}
