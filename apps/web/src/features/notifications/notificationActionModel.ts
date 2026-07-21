export type NotificationActionName = "cancel" | "retry";
export type NotificationActionErrors = Partial<Record<NotificationActionName, string>>;
export type NotificationActionStates = ReadonlyMap<string, NotificationOperationActionState>;

export interface NotificationOperationActionState {
  requestId: number;
  pendingAction: NotificationActionName | null;
  errors: NotificationActionErrors;
}

export function notificationOperationActionKey(
  libraryId: string,
  operationId: string
): string {
  return `${libraryId}\u0000${operationId}`;
}

export function beginNotificationAction(
  states: NotificationActionStates,
  key: string,
  action: NotificationActionName,
  requestId: number
): { states: NotificationActionStates; started: boolean } {
  const current = states.get(key);
  if (current?.pendingAction) return { states, started: false };
  const errors = { ...(current?.errors ?? {}) };
  delete errors[action];
  const next = new Map(states);
  next.set(key, { requestId, pendingAction: action, errors });
  return { states: next, started: true };
}

export function settleNotificationAction(
  states: NotificationActionStates,
  key: string,
  action: NotificationActionName,
  requestId: number,
  error: string | null
): NotificationActionStates {
  const current = states.get(key);
  if (
    !current ||
    current.requestId !== requestId ||
    current.pendingAction !== action
  ) {
    return states;
  }
  const errors = { ...current.errors };
  if (error === null) delete errors[action];
  else errors[action] = error;
  const next = new Map(states);
  if (Object.keys(errors).length === 0) next.delete(key);
  else next.set(key, { requestId, pendingAction: null, errors });
  return next;
}
