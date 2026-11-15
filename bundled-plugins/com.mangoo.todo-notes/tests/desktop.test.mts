import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

import { parseTodoState, stickyBounds, nextStickyAnchor, visibleWindowBounds, STICKY_HEIGHT } from "../src/desktop/desktopState.ts";
import { dueReminders, nextReminderAt, pruneFired, todayCount, MISSED_REMINDER_GRACE_MS } from "../src/desktop/reminders.ts";
import { TODO_CHANNELS } from "../src/shared/ipc.ts";
import { itemFromRecord, newId, type TodoItem } from "../src/shared/model.ts";

function item(fields: Record<string, unknown>, id = newId()): TodoItem {
  return itemFromRecord({ id, collection: "items", fields, deleted: false, createdAt: 1, updatedAt: 1 });
}

const area = { x: 0, y: 0, width: 1920, height: 1040 };

test("pinned notes grow downward, flip upward at the bottom edge and stay on screen", () => {
  const note = { x: 100, y: 200, width: 280 };
  assert.deepEqual(stickyBounds(note, 64, [area]), { x: 100, y: 200, width: 280, height: 64 });
  const low = { x: 100, y: 960, width: 280 };
  const expanded = stickyBounds(low, 400, [area]);
  assert.equal(expanded.y + expanded.height, 1040, "grows upward from the bottom edge");
  const collapsed = stickyBounds(low, 64, [area]);
  assert.equal(collapsed.y, 960, "collapsing returns to the anchor");
  // A note left on an unplugged monitor comes back to a visible one.
  const offscreen = stickyBounds({ x: 4000, y: 100, width: 280 }, 64, [area]);
  assert.ok(offscreen.x + offscreen.width <= 1920);
  assert.ok(stickyBounds(note, 10, [area]).height >= STICKY_HEIGHT.collapsedMin);
});

test("new notes cascade instead of stacking exactly", () => {
  const first = nextStickyAnchor([], area);
  const second = nextStickyAnchor([first], area);
  assert.notDeepEqual(first, second);
  assert.ok(first.x + 280 <= area.width);
});

test("restored windows stay visible", () => {
  const centred = visibleWindowBounds({ x: 5000, y: 5000, width: 900, height: 600 }, [area]);
  assert.ok(centred.x >= 0 && centred.x < 1920);
  assert.deepEqual(visibleWindowBounds({ x: 10, y: 10, width: 900, height: 600 }, [area]), { x: 10, y: 10, width: 900, height: 600 });
});

test("desktop state parsing is defensive", () => {
  assert.equal(parseTodoState("{oops").settings.trayIcon, true);
  const id = newId();
  const state = parseTodoState(JSON.stringify({
    settings: { trayIcon: false, quickAddShortcut: "Alt+Shift+T" },
    pinned: { [id]: { x: 1, y: 2, width: 9999, expandedHeight: 10, appearance: { color: "nope", opacity: 2, locked: true } }, "not-an-id": { x: 1, y: 1, width: 300, expandedHeight: 300 } },
    reminders: { lastCheck: 5, fired: { [id]: 9, bad: 1 } },
    window: { bounds: { x: 0, y: 0, width: 10, height: 10 } }
  }));
  assert.equal(state.settings.trayIcon, false);
  assert.equal(state.settings.closeToTray, false);
  assert.equal(state.settings.quickAddShortcut, "Alt+Shift+T");
  assert.deepEqual(Object.keys(state.pinned), [id]);
  assert.equal(state.pinned[id].width, 640);
  assert.equal(state.pinned[id].expandedHeight, STICKY_HEIGHT.expandedMin);
  assert.deepEqual(state.pinned[id].appearance, { color: "glass", expanded: false }, "only color / expanded are kept");
  assert.deepEqual(state.reminders.fired, { [id]: 9 });
  assert.equal(state.window, null, "too-small window bounds dropped");
});

test("reminders fire once per scheduled time and skip stale or finished items", () => {
  const now = 1_800_000_000_000;
  const due = item({ title: "due", remindAt: now - 1000 });
  const future = item({ title: "later", remindAt: now + 60_000 });
  const done = item({ title: "done", remindAt: now - 1000, done: true });
  const stale = item({ title: "stale", remindAt: now - MISSED_REMINDER_GRACE_MS - 1 });
  const all = [due, future, done, stale];
  assert.deepEqual(dueReminders(all, now - 5000, now, {}).map((entry) => entry.title), ["due"]);
  assert.deepEqual(dueReminders(all, now - 5000, now, { [due.id]: due.remindAt as number }), [], "already fired");
  assert.deepEqual(dueReminders(all, 0, now, {}).map((entry) => entry.title), ["due"], "missed beyond the grace window are dropped");
  assert.equal(nextReminderAt(all, now), now + 60_000);
  assert.deepEqual(pruneFired(all, { [due.id]: due.remindAt as number, [future.id]: 1, gone: 3 }), { [due.id]: due.remindAt });
  assert.equal(todayCount([item({ dueDate: "2026-11-14" }), item({ dueDate: "2026-11-20" }), item({ dueDate: "2026-11-01", done: true })], "2026-11-14"), 1);
});

test("preload exposes exactly the plugin's TODO channels", () => {
  const preload = fs.readFileSync(new URL("../../../apps/desktop/src/preload.cjs", import.meta.url), "utf8");
  for (const channel of Object.values(TODO_CHANNELS)) {
    assert.ok(preload.includes(`"${channel}"`), `preload is missing ${channel}`);
  }
  const used = [...preload.matchAll(/"(mangoo:todo-notes:[a-z-]+)"/g)].map((match) => match[1]);
  const known = new Set(Object.values(TODO_CHANNELS) as string[]);
  for (const channel of used) assert.ok(known.has(channel), `preload uses unknown channel ${channel}`);
});

test("the Host window bridge exposes only the privileged channels", () => {
  const preload = fs.readFileSync(new URL("../../../apps/desktop/src/preload.cjs", import.meta.url), "utf8");
  const start = preload.indexOf("const todoNotesHostBridge = {");
  assert.ok(start >= 0, "host bridge exists");
  const body = preload.slice(start, preload.indexOf("\n};", start));
  const used = new Set([...body.matchAll(/"(mangoo:todo-notes:[a-z-]+)"/g)].map((match) => match[1]));
  assert.deepEqual([...used].sort(), [TODO_CHANNELS.getSettings, TODO_CHANNELS.openSettings, TODO_CHANNELS.openWindow, TODO_CHANNELS.setSettings, TODO_CHANNELS.status].sort());
});

test("the settings window bridge exposes only settings, status and section pushes", () => {
  const preload = fs.readFileSync(new URL("../../../apps/desktop/src/preload.cjs", import.meta.url), "utf8");
  const start = preload.indexOf("const todoNotesSettingsWindowBridge = {");
  assert.ok(start >= 0, "settings window bridge exists");
  const body = preload.slice(start, preload.indexOf("\n  }\n};", start));
  const used = new Set([...body.matchAll(/"(mangoo:todo-notes:[a-z-]+)"/g)].map((match) => match[1]));
  assert.deepEqual([...used].sort(), [
    TODO_CHANNELS.getSettings,
    TODO_CHANNELS.setSettings,
    TODO_CHANNELS.settingsChanged,
    TODO_CHANNELS.settingsSection,
    TODO_CHANNELS.status,
    TODO_CHANNELS.exportBackup,
    TODO_CHANNELS.importBackup
  ].sort());
});
