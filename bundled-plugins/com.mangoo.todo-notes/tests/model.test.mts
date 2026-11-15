import assert from "node:assert/strict";
import test from "node:test";

import { backupFileName, createBackup, parseBackup, summarizeBackup, type BackupRecord } from "../src/shared/backup.ts";
import { asSingleUrl, blockRule, inlineRule, looksLikeMarkdown, markdownToHtml, trailingUrl } from "../src/shared/markdown.ts";
import { assignChord, chordFromEvent, formatChord, matchAction, normalizeOverrides, resolveBindings, shortcutHint } from "../src/shared/shortcuts.ts";

import {
  addDays,
  compareItems,
  descendantIds,
  dueState,
  formatDueLabel,
  itemFromRecord,
  itemInView,
  keyBetween,
  keysAfter,
  matchesSearch,
  newId,
  nextOccurrence,
  normalizeUrl,
  parseQuickAdd,
  stripHtml,
  type TodoItem
} from "../src/shared/model.ts";
import { mergeAppearance, mergeSettings, normalizeAccelerator, DEFAULT_TODO_SETTINGS, DEFAULT_STICKY_APPEARANCE, isRecordId } from "../src/shared/ipc.ts";

function item(fields: Record<string, unknown>, extra: Partial<{ id: string; deleted: boolean; createdAt: number }> = {}): TodoItem {
  return itemFromRecord({
    id: extra.id ?? newId(),
    collection: "items",
    fields,
    deleted: extra.deleted ?? false,
    createdAt: extra.createdAt ?? 1,
    updatedAt: 1
  });
}

test("records parse defensively into items", () => {
  const parsed = item({ title: "a", priority: 7, dueDate: "2026-13", dueTime: "25:00", tags: ["x", 3, ""] });
  assert.equal(parsed.priority, 0);
  assert.equal(parsed.dueDate, null);
  assert.equal(parsed.dueTime, null);
  assert.deepEqual(parsed.tags, ["x"]);
  assert.equal(item({ dueTime: "09:30" }).dueTime, null, "time needs a date");
  assert.equal(item({ dueDate: "2026-11-14", dueTime: "09:30" }).dueTime, "09:30");
});

test("fractional keys always fit between neighbours and stay ordered", () => {
  const first = keyBetween(null, null);
  const after = keyBetween(first, null);
  const before = keyBetween(null, first);
  assert.ok(before < first && first < after);
  // Repeated insertion at the same gap never runs out or collides.
  let low = first;
  const high = after;
  for (let index = 0; index < 200; index += 1) {
    const middle = keyBetween(low, high);
    assert.ok(low < middle && middle < high, `${low} < ${middle} < ${high}`);
    assert.notEqual(middle.at(-1), "0");
    low = middle;
  }
  let top = first;
  for (let index = 0; index < 200; index += 1) {
    const next = keyBetween(null, top);
    assert.ok(next < top);
    top = next;
  }
  const keys = keysAfter("i", 5);
  assert.deepEqual([...keys].sort(), keys);
  assert.throws(() => keyBetween("b", "a"));
});

test("recurrence rolls dates forward, clamping month ends and skipping weekends", () => {
  assert.equal(nextOccurrence("2026-11-14", { freq: "daily", interval: 2 }), "2026-11-16");
  assert.equal(nextOccurrence("2026-11-14", { freq: "weekly", interval: 1 }), "2026-11-21");
  assert.equal(nextOccurrence("2026-01-31", { freq: "monthly", interval: 1 }), "2026-02-28");
  assert.equal(nextOccurrence("2028-02-29", { freq: "yearly", interval: 1 }), "2029-02-28");
  // 2026-11-13 is a Friday.
  assert.equal(nextOccurrence("2026-11-13", { freq: "weekdays", interval: 1 }), "2026-11-16");
  assert.equal(addDays("2026-12-31", 1), "2027-01-01");
});

test("quick add extracts dates, times, priority and tags", () => {
  const today = "2026-11-14"; // Saturday
  assert.deepEqual(parseQuickAdd("写周报 明天 17:30 !高 #工作", today), {
    title: "写周报",
    dueDate: "2026-11-15",
    dueTime: "17:30",
    priority: 3,
    tags: ["工作"]
  });
  assert.equal(parseQuickAdd("开会 下午3点", today).dueTime, "15:00");
  assert.equal(parseQuickAdd("开会 下午3点", today).dueDate, today, "a bare time means today");
  assert.equal(parseQuickAdd("复盘 9点半", today).dueTime, "09:30");
  assert.equal(parseQuickAdd("交房租 周一", today).dueDate, "2026-11-16");
  assert.equal(parseQuickAdd("体检 下周三", today).dueDate, "2026-11-18");
  assert.equal(parseQuickAdd("续费 12月1日", today).dueDate, "2026-12-01");
  assert.equal(parseQuickAdd("年会 1-10", today).dueDate, "2027-01-10", "a past month/day means next year");
  assert.equal(parseQuickAdd("买 3 个苹果", today).title, "买 3 个苹果", "plain numbers stay in the title");
  assert.equal(parseQuickAdd("C# 学习", today).title, "C# 学习", "a hash inside a word is not a tag");
  assert.equal(parseQuickAdd("整理 !", today).priority, 1);
  assert.equal(parseQuickAdd("整理 !!", today).priority, 2);
  assert.equal(parseQuickAdd("整理 !!!", today).priority, 3);
});

test("views, search, sorting and hierarchy", () => {
  const today = "2026-11-14";
  const parent = item({ title: "Parent", dueDate: "2026-11-13", priority: 3, order: "b" });
  const child = item({ title: "Child", parentId: parent.id, order: "a" });
  const done = item({ title: "Done", done: true, order: "c" });
  const trashed = item({ title: "Trash" }, { deleted: true });
  assert.equal(itemInView(parent, "today", today), true, "overdue shows in today");
  assert.equal(itemInView(parent, "important", today), true);
  assert.equal(itemInView(done, "today", today), false);
  assert.equal(itemInView(done, "completed", today), true);
  assert.equal(itemInView(trashed, "all", today), false);
  assert.equal(itemInView(trashed, "trash", today), true);
  assert.equal(dueState(parent, today), "overdue");
  assert.equal(descendantIds([parent, child, done], parent.id).join(), child.id);
  assert.ok(matchesSearch(item({ title: "季度 OKR", notes: "<p>链接 <b>重要</b></p>" }), "okr 重要"));
  assert.equal(stripHtml("<p>a&amp;b</p><p>c</p>"), "a&b c");
  const sorted = [done, parent].sort((a, b) => compareItems(a, b, "priority"));
  assert.equal(sorted[0].id, parent.id);
});

test("due labels are relative and localized", () => {
  const today = "2026-11-14";
  assert.equal(formatDueLabel(today, null, today), "今天");
  assert.equal(formatDueLabel("2026-11-15", "09:00", today), "明天 09:00");
  assert.equal(formatDueLabel("2026-11-13", null, today), "昨天");
  assert.equal(formatDueLabel("2026-11-30", null, today), "11月30日");
  assert.equal(formatDueLabel("2027-02-01", null, today), "2027年2月1日");
});

test("links are restricted to web and mail", () => {
  assert.equal(normalizeUrl("example.com/a"), "https://example.com/a");
  assert.equal(normalizeUrl("https://x.y"), "https://x.y");
  assert.equal(normalizeUrl("mailto:a@b.c"), "mailto:a@b.c");
  assert.equal(normalizeUrl("file:///C:/Windows"), null);
  assert.equal(normalizeUrl("javascript:alert(1)"), null);
});

test("ids are UUIDv7 and settings merge strictly", () => {
  const id = newId();
  assert.ok(isRecordId(id));
  assert.equal(id[14], "7");
  const merged = mergeSettings(DEFAULT_TODO_SETTINGS, { trayIcon: false, closeToTray: true, quickAddShortcut: "Ctrl+Q", reminders: "yes" });
  assert.equal(merged.trayIcon, false);
  assert.equal(merged.closeToTray, false, "no tray means closing really closes");
  assert.equal(merged.quickAddShortcut, "CommandOrControl+Q", "recorded accelerators are normalized");
  assert.equal(mergeSettings(DEFAULT_TODO_SETTINGS, { quickAddShortcut: "Shift+A" }).quickAddShortcut, null, "plain typing is not a global shortcut");
  assert.equal(mergeSettings(DEFAULT_TODO_SETTINGS, { quickAddShortcut: "Ctrl+Alt+Bogus" }).quickAddShortcut, null);
  assert.equal(normalizeAccelerator("F9"), "F9");
  assert.equal(normalizeAccelerator("alt+shift+space"), "Alt+Shift+Space");
  assert.equal(merged.reminders, true);
  const look = mergeAppearance(DEFAULT_STICKY_APPEARANCE, { color: "pink", opacity: 0.1, locked: true, x: 3 });
  assert.deepEqual(look, { color: "pink", expanded: false });
});

test("backups round-trip, are validated and summarized", () => {
  const list = newId();
  const note = newId();
  const records: BackupRecord[] = [
    { id: newId(), collection: "attachments", fields: { itemId: note, mime: "image/webp", data: "AA" } },
    { id: note, collection: "items", fields: { title: "<b>周报</b>", listId: list } },
    { id: newId(), collection: "items", fields: { title: "子项", parentId: note } },
    { id: list, collection: "lists", fields: { name: "工作" } }
  ];
  const backup = createBackup(records, new Date("2026-11-15T10:00:00Z"));
  assert.deepEqual(backup.records.map((record) => record.collection), ["lists", "items", "items", "attachments"], "lists, notes, then images");
  const parsed = parseBackup(JSON.stringify(backup));
  assert.equal(parsed.records.length, 4);
  assert.deepEqual(summarizeBackup(parsed.records), { lists: 1, notes: 1, subtasks: 1, images: 1 });
  const hostile = JSON.stringify({ ...backup, records: [...backup.records, { id: "nope", collection: "items", fields: {} }, { id: newId(), collection: "secrets", fields: {} }, { id: list, collection: "lists", fields: {} }, { id: newId(), collection: "items", fields: { "bad-name": 1, purged: true, title: "x" } }] });
  const cleaned = parseBackup(hostile);
  assert.equal(cleaned.records.length, 5, "bad ids, unknown collections and duplicates are dropped");
  assert.deepEqual(cleaned.records.find((record) => record.fields.title === "x")?.fields, { title: "x" }, "invalid field names and purge markers are stripped");
  assert.throws(() => parseBackup("{}"), /不是便笺的备份文件/);
  assert.throws(() => parseBackup("nope"), /无法解析/);
  assert.match(backupFileName(new Date(2026, 10, 5)), /2026-11-05\.json$/);
});

test("shortcuts resolve, remap with conflict moves and match events", () => {
  const defaults = resolveBindings({});
  assert.deepEqual(defaults.newNote, ["Ctrl+N"]);
  const event = { key: "n", code: "KeyN", ctrlKey: true, metaKey: false, altKey: false, shiftKey: false };
  assert.equal(chordFromEvent(event), "Ctrl+N");
  assert.equal(matchAction(defaults, event), "newNote");
  // Give Ctrl+N to search: it leaves 记一条.
  const moved = assignChord({}, "search", "Ctrl+N", 0);
  const bindings = resolveBindings(moved);
  assert.deepEqual(bindings.search, ["Ctrl+N"]);
  assert.deepEqual(bindings.newNote, []);
  assert.equal(matchAction(bindings, event), "search");
  assert.equal(shortcutHint(bindings, "newNote"), undefined);
  assert.equal(formatChord("Space"), "空格");
  assert.equal(chordFromEvent({ key: " ", code: "Space", ctrlKey: false, metaKey: false, altKey: false, shiftKey: false }), "Space");
  // Reserved keys and garbage are dropped from stored overrides.
  assert.deepEqual(normalizeOverrides({ search: ["Escape", "ctrl+shift+f", "Hyper+X"], bogus: ["A"] }), { search: ["Ctrl+Shift+F"] });
});

test("markdown input rules", () => {
  assert.equal(blockRule("#"), "h1");
  assert.equal(blockRule("-"), "ul");
  assert.equal(blockRule("12."), "ol");
  assert.equal(blockRule("[ ]"), "checklist");
  assert.equal(blockRule("[x]"), "checked");
  assert.equal(blockRule(">"), "blockquote");
  assert.equal(blockRule("hello"), null);
  assert.deepEqual(inlineRule("say **hi**"), { mark: "bold", start: 4, content: "hi" });
  assert.deepEqual(inlineRule("a `x`"), { mark: "code", start: 2, content: "x" });
  assert.deepEqual(inlineRule("~~gone~~"), { mark: "strike", start: 0, content: "gone" });
  assert.equal(inlineRule("2 * 3 *"), null, "spaces inside delimiters are not a mark");
  assert.equal(inlineRule("**"), null);
  assert.equal(asSingleUrl(" https://example.com/a?b=1 "), "https://example.com/a?b=1");
  assert.equal(asSingleUrl("www.example.com"), "https://www.example.com");
  assert.equal(asSingleUrl("see https://example.com"), null);
  assert.equal(asSingleUrl("javascript:alert(1)"), null);
  assert.deepEqual(trailingUrl("look https://a.io/x."), { start: 5, url: "https://a.io/x", href: "https://a.io/x" });
});

test("pasted markdown converts to the notes subset", () => {
  assert.ok(looksLikeMarkdown("# Title\n- a"));
  assert.ok(!looksLikeMarkdown("just a sentence."));
  const html = markdownToHtml("# Plan\n- [ ] call\n- [x] mail\n1. one\n> quoted\n```\n<b>raw</b>\n```\nSee **this** and [docs](https://example.com) or https://a.io");
  assert.match(html, /^<h1>Plan<\/h1>/);
  assert.match(html, /<ul data-type="checklist"><li>call<\/li><li data-checked="true">mail<\/li><\/ul>/);
  assert.match(html, /<ol><li>one<\/li><\/ol>/);
  assert.match(html, /<blockquote><p>quoted<\/p><\/blockquote>/);
  assert.match(html, /<pre>&lt;b&gt;raw&lt;\/b&gt;<\/pre>/);
  assert.match(html, /<b>this<\/b>/);
  assert.match(html, /<a href="https:\/\/example.com">docs<\/a>/);
  assert.match(html, /<a href="https:\/\/a.io">https:\/\/a.io<\/a>/);
  assert.ok(!markdownToHtml("[x](javascript:alert(1))").includes("href"), "unsafe links stay text");
});

test("blank lines between markdown blocks are not empty paragraphs", () => {
  assert.equal(markdownToHtml("one\n\ntwo"), "<p>one</p><p>two</p>");
  assert.equal(markdownToHtml("one\n\n\n\ntwo"), "<p>one</p><p><br></p><p>two</p>", "a long gap keeps one blank line");
});
