import type { TodoItem } from "../shared/model.ts";

/** Reminders older than this when Mangoo starts are dropped, not fired. */
export const MISSED_REMINDER_GRACE_MS = 12 * 60 * 60 * 1000;

function eligible(item: TodoItem): item is TodoItem & { remindAt: number } {
  return !item.deleted && !item.done && typeof item.remindAt === "number";
}

/**
 * Items whose reminder came due in `(lastCheck, now]` and has not fired yet
 * for that exact time. A reminder rescheduled (snoozed, recurring roll-over)
 * has a new `remindAt` and fires again.
 */
export function dueReminders(
  items: readonly TodoItem[],
  lastCheck: number,
  now: number,
  fired: Readonly<Record<string, number>>
): TodoItem[] {
  const from = Math.max(lastCheck, now - MISSED_REMINDER_GRACE_MS);
  return items
    .filter(eligible)
    .filter((item) => item.remindAt > from && item.remindAt <= now && fired[item.id] !== item.remindAt)
    .sort((a, b) => a.remindAt - b.remindAt);
}

/** The next pending reminder time after `now`, or null. */
export function nextReminderAt(items: readonly TodoItem[], now: number): number | null {
  let next: number | null = null;
  for (const item of items) {
    if (!eligible(item) || item.remindAt <= now) continue;
    if (next === null || item.remindAt < next) next = item.remindAt;
  }
  return next;
}

/** Drop bookkeeping for items that no longer have that reminder. */
export function pruneFired(items: readonly TodoItem[], fired: Readonly<Record<string, number>>): Record<string, number> {
  const byId = new Map(items.map((item) => [item.id, item]));
  const next: Record<string, number> = {};
  for (const [id, at] of Object.entries(fired)) {
    if (byId.get(id)?.remindAt === at) next[id] = at;
  }
  return next;
}

/** Undone, non-deleted top-level items due today or earlier. */
export function todayCount(items: readonly TodoItem[], today: string): number {
  return items.filter((item) => !item.deleted && !item.done && !item.parentId && item.dueDate !== null && item.dueDate <= today).length;
}
