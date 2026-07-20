export interface RecoverableNotificationShutdownOptions {
  quiesceNotifications(): Promise<void>;
  stopCore(): Promise<void>;
  restoreNotifications(): Promise<void>;
  closeNotifications(): Promise<void>;
}

export async function runRecoverableNotificationShutdown(
  options: RecoverableNotificationShutdownOptions
): Promise<void> {
  await options.quiesceNotifications();
  try {
    await options.stopCore();
  } catch (stopError) {
    try {
      await options.restoreNotifications();
    } catch (restoreError) {
      throw new AggregateError(
        [stopError, restoreError],
        "Core stop failed and notification synchronization could not be restored"
      );
    }
    throw stopError;
  }
  await options.closeNotifications();
}
