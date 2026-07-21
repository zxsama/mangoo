export type NotificationActionName = "cancel" | "retry";
export type NotificationActionStates = ReadonlyMap<string, NotificationActionState>;

export interface NotificationActionState {
  requestId: number;
  pending: boolean;
  error: string | null;
}

export function notificationActionKey(
  libraryId: string,
  operationId: string,
  action: NotificationActionName
): string {
  return `${libraryId}\u0000${operationId}\u0000${action}`;
}

export function beginNotificationAction(
  states: NotificationActionStates,
  key: string,
  requestId: number
): { states: NotificationActionStates; started: boolean } {
  if (states.get(key)?.pending) return { states, started: false };
  const next = new Map(states);
  next.set(key, { requestId, pending: true, error: null });
  return { states: next, started: true };
}

export function settleNotificationAction(
  states: NotificationActionStates,
  key: string,
  requestId: number,
  error: string | null
): NotificationActionStates {
  const current = states.get(key);
  if (!current || current.requestId !== requestId) return states;
  const next = new Map(states);
  if (error === null) next.delete(key);
  else next.set(key, { requestId, pending: false, error });
  return next;
}
