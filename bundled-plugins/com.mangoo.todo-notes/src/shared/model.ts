/**
 * Pure data model of the TODO notes plugin, shared by the Desktop host (tray,
 * reminders, pinned notes) and the web UI. Records live in Core's generic
 * plugin-data SHARED scope (`/api/plugin-data/com.mangoo.todo-notes/shared/*`):
 * they belong to no Library, are visible from every Library and synchronize
 * through each one. Every top-level field below is one independently merged
 * (last-writer-wins) field. Keep values small: a field is one sync event.
 */

export const TODO_PLUGIN_ID = "com.mangoo.todo-notes";
export const TODO_CAPABILITY = "todo-notes.v1";

export const COLLECTION_ITEMS = "items";
export const COLLECTION_LISTS = "lists";
export const COLLECTION_ATTACHMENTS = "attachments";

/** 0 none, 1 low, 2 medium, 3 high. */
export type Priority = 0 | 1 | 2 | 3;
export const PRIORITIES: readonly Priority[] = [3, 2, 1, 0];
export const PRIORITY_LABELS: Record<Priority, string> = { 0: "无优先级", 1: "低", 2: "中", 3: "高" };

export type RecurrenceFrequency = "daily" | "weekdays" | "weekly" | "monthly" | "yearly";
export interface Recurrence {
  freq: RecurrenceFrequency;
  /** Every N units (ignored for weekdays). */
  interval: number;
}
export const RECURRENCE_LABELS: Record<RecurrenceFrequency, string> = {
  daily: "每天",
  weekdays: "每个工作日",
  weekly: "每周",
  monthly: "每月",
  yearly: "每年"
};

export interface TodoItem {
  id: string;
  /** Owning list; null = no list. Subtasks follow their root's list. */
  listId: string | null;
  /** Parent item for a subtask; null for a top-level item. */
  parentId: string | null;
  /** Sanitized INLINE rich text (b / i / u / s / code / mark / a); see `titleText`. */
  title: string;
  /** Sanitized rich-text HTML (see web/richText.ts). */
  notes: string;
  done: boolean;
  completedAt: number | null;
  priority: Priority;
  /** Local calendar date `YYYY-MM-DD`. */
  dueDate: string | null;
  /** Local time `HH:MM`, only with a dueDate. */
  dueTime: string | null;
  /** Epoch ms of a reminder, or null. */
  remindAt: number | null;
  recurrence: Recurrence | null;
  tags: string[];
  /** Fractional ordering key among siblings (see `keyBetween`). */
  order: string;
  createdAt: number;
  updatedAt: number;
  deleted: boolean;
}

export interface TodoList {
  id: string;
  name: string;
  color: string;
  order: string;
  createdAt: number;
  deleted: boolean;
}

export interface TodoAttachment {
  id: string;
  itemId: string;
  mime: string;
  /** Base64 payload (no data: prefix). */
  data: string;
  width: number;
  height: number;
  name: string;
}

/** Wire record shape (Core `PluginRecord`). */
export interface RecordLike {
  id: string;
  collection: string;
  fields: Record<string, unknown>;
  deleted: boolean;
  createdAt: number;
  updatedAt: number;
}

export const LIST_COLORS = ["#5b8def", "#3fb27f", "#e5a33b", "#e5534b", "#a371f7", "#3ab0c4", "#d16ba5", "#8b949e"] as const;

/** Of the stored inline HTML (marks included). */
export const MAX_TITLE_LENGTH = 2000;
export const MAX_NOTES_BYTES = 120 * 1024;
export const MAX_TAGS = 16;
/** Base64 bytes of one image attachment; keeps the field below Core's 160 KiB. */
export const MAX_ATTACHMENT_BASE64 = 150 * 1024;
export const MAX_SUBTASK_DEPTH = 3;

// ------------------------------------------------------------------ parsing

function str(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}
function num(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}
const DATE = /^\d{4}-\d{2}-\d{2}$/;
const TIME = /^([01]\d|2[0-3]):[0-5]\d$/;

export function parsePriority(value: unknown): Priority {
  return value === 1 || value === 2 || value === 3 ? value : 0;
}

export function parseRecurrence(value: unknown): Recurrence | null {
  if (!value || typeof value !== "object") return null;
  const { freq, interval } = value as { freq?: unknown; interval?: unknown };
  if (typeof freq !== "string" || !(freq in RECURRENCE_LABELS)) return null;
  const n = typeof interval === "number" && Number.isInteger(interval) ? Math.min(Math.max(interval, 1), 365) : 1;
  return { freq: freq as RecurrenceFrequency, interval: n };
}

export function itemFromRecord(record: RecordLike): TodoItem {
  const f = record.fields;
  const dueDate = typeof f.dueDate === "string" && DATE.test(f.dueDate) ? f.dueDate : null;
  return {
    id: record.id,
    listId: typeof f.listId === "string" && f.listId ? f.listId : null,
    parentId: typeof f.parentId === "string" && f.parentId ? f.parentId : null,
    title: str(f.title),
    notes: str(f.notes),
    done: f.done === true,
    completedAt: num(f.completedAt),
    priority: parsePriority(f.priority),
    dueDate,
    dueTime: dueDate && typeof f.dueTime === "string" && TIME.test(f.dueTime) ? f.dueTime : null,
    remindAt: num(f.remindAt),
    recurrence: parseRecurrence(f.recurrence),
    tags: Array.isArray(f.tags) ? f.tags.filter((tag): tag is string => typeof tag === "string" && tag.length > 0).slice(0, MAX_TAGS) : [],
    order: str(f.order, "i"),
    createdAt: record.createdAt,
    updatedAt: record.updatedAt,
    deleted: record.deleted
  };
}

export function listFromRecord(record: RecordLike): TodoList {
  const f = record.fields;
  return {
    id: record.id,
    name: str(f.name, "未命名清单"),
    color: typeof f.color === "string" && /^#[0-9a-f]{6}$/i.test(f.color) ? f.color : LIST_COLORS[0],
    order: str(f.order, "i"),
    createdAt: record.createdAt,
    deleted: record.deleted
  };
}

export function attachmentFromRecord(record: RecordLike): TodoAttachment | null {
  const f = record.fields;
  if (typeof f.data !== "string" || typeof f.mime !== "string" || !/^image\/(webp|png|jpeg|gif)$/.test(f.mime)) return null;
  return {
    id: record.id,
    itemId: str(f.itemId),
    mime: f.mime,
    data: f.data,
    width: num(f.width) ?? 0,
    height: num(f.height) ?? 0,
    name: str(f.name)
  };
}

/** Field patch for a put; only the keys present are written. */
export type ItemPatch = Partial<Omit<TodoItem, "id" | "createdAt" | "updatedAt" | "deleted">>;

export function itemFields(patch: ItemPatch): Record<string, unknown> {
  const fields: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) continue;
    fields[key] = value;
  }
  return fields;
}

// ------------------------------------------------------------- validation

/** Only web and mail links are ever opened. */
export function isSafeUrl(value: string): boolean {
  if (value.length === 0 || value.length > 2048) return false;
  try {
    const url = new URL(value);
    return url.protocol === "http:" || url.protocol === "https:" || url.protocol === "mailto:";
  } catch {
    return false;
  }
}

/** Accept `example.com/x` as `https://example.com/x`. */
export function normalizeUrl(value: string): string | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  if (isSafeUrl(trimmed)) return trimmed;
  if (/^[\w-]+(\.[\w-]+)+(\/\S*)?$/.test(trimmed)) {
    const candidate = `https://${trimmed}`;
    return isSafeUrl(candidate) ? candidate : null;
  }
  return null;
}

export function normalizeTag(value: string): string {
  return value.trim().replace(/^#+/, "").replace(/\s+/g, "-").slice(0, 32);
}

// ---------------------------------------------------------------- ordering

const DIGITS = "0123456789abcdefghijklmnopqrstuvwxyz";

/**
 * Fractional ordering key strictly between `a` and `b` (either may be null
 * for an open end). Keys never end in "0", so a key always exists between two
 * distinct keys and inserting never rewrites siblings (no reorder conflicts
 * across synchronized devices).
 */
export function keyBetween(a: string | null, b: string | null): string {
  const lo = a ?? "";
  if (b !== null && lo >= b) throw new Error(`keyBetween: ${lo} >= ${b}`);
  let result = "";
  let upperOpen = b === null;
  for (let index = 0; index < 64; index += 1) {
    const da = index < lo.length ? DIGITS.indexOf(lo[index]) : 0;
    const db = upperOpen ? DIGITS.length : index < (b as string).length ? DIGITS.indexOf((b as string)[index]) : 0;
    if (!upperOpen && da === db) {
      result += DIGITS[da];
      continue;
    }
    const mid = Math.floor((da + db) / 2);
    if (mid > da) return result + DIGITS[mid];
    // Adjacent digits: keep lo's digit; everything after it is unbounded above.
    result += DIGITS[da];
    upperOpen = true;
  }
  throw new Error("keyBetween: key too long");
}

/** `count` increasing keys after `after` (for bulk appends / renumbering). */
export function keysAfter(after: string | null, count: number): string[] {
  const keys: string[] = [];
  let previous = after;
  for (let index = 0; index < count; index += 1) {
    previous = keyBetween(previous, null);
    keys.push(previous);
  }
  return keys;
}

export function compareOrder(a: { order: string; createdAt: number }, b: { order: string; createdAt: number }): number {
  if (a.order !== b.order) return a.order < b.order ? -1 : 1;
  return a.createdAt - b.createdAt;
}

// ----------------------------------------------------------------- dates

export function pad(value: number): string {
  return String(value).padStart(2, "0");
}

export function toDateKey(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

export function fromDateKey(key: string): Date {
  const [year, month, day] = key.split("-").map(Number);
  return new Date(year, month - 1, day);
}

export function addDays(key: string, days: number): string {
  const date = fromDateKey(key);
  date.setDate(date.getDate() + days);
  return toDateKey(date);
}

function addMonthsClamped(key: string, months: number): string {
  const [year, month, day] = key.split("-").map(Number);
  const target = new Date(year, month - 1 + months, 1);
  const last = new Date(target.getFullYear(), target.getMonth() + 1, 0).getDate();
  target.setDate(Math.min(day, last));
  return toDateKey(target);
}

/** The next due date of a recurring item after `from` (its current due date). */
export function nextOccurrence(from: string, recurrence: Recurrence): string {
  const interval = Math.max(1, recurrence.interval);
  switch (recurrence.freq) {
    case "daily":
      return addDays(from, interval);
    case "weekly":
      return addDays(from, 7 * interval);
    case "weekdays": {
      let next = addDays(from, 1);
      while ([0, 6].includes(fromDateKey(next).getDay())) next = addDays(next, 1);
      return next;
    }
    case "monthly":
      return addMonthsClamped(from, interval);
    case "yearly":
      return addMonthsClamped(from, 12 * interval);
  }
}

/** Epoch ms of a local date + time. */
export function localTimestamp(dateKey: string, time: string | null): number {
  const date = fromDateKey(dateKey);
  if (time) {
    const [hours, minutes] = time.split(":").map(Number);
    date.setHours(hours, minutes, 0, 0);
  }
  return date.getTime();
}

const WEEKDAYS = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

/** Short, relative Chinese label: 今天 / 明天 / 昨天 / 周三 / 11月20日 / 2027年1月3日. */
export function formatDueLabel(dateKey: string, time: string | null, today: string): string {
  const diff = Math.round((fromDateKey(dateKey).getTime() - fromDateKey(today).getTime()) / 86_400_000);
  const date = fromDateKey(dateKey);
  let label: string;
  if (diff === 0) label = "今天";
  else if (diff === 1) label = "明天";
  else if (diff === 2) label = "后天";
  else if (diff === -1) label = "昨天";
  else if (diff > 2 && diff < 7) label = WEEKDAYS[date.getDay()];
  else if (date.getFullYear() === fromDateKey(today).getFullYear()) label = `${date.getMonth() + 1}月${date.getDate()}日`;
  else label = `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日`;
  return time ? `${label} ${time}` : label;
}

export type DueState = "none" | "overdue" | "today" | "soon" | "later";

export function dueState(item: Pick<TodoItem, "dueDate" | "dueTime" | "done">, today: string, now = Date.now()): DueState {
  if (!item.dueDate || item.done) return "none";
  if (item.dueDate < today) return "overdue";
  if (item.dueDate === today) {
    if (item.dueTime && localTimestamp(item.dueDate, item.dueTime) < now) return "overdue";
    return "today";
  }
  return item.dueDate <= addDays(today, 3) ? "soon" : "later";
}

// ------------------------------------------------------------------ views

export type SmartView = "today" | "upcoming" | "important" | "all" | "completed" | "trash";
export type ViewKey = SmartView | `list:${string}` | `tag:${string}`;

export const SMART_VIEW_LABELS: Record<SmartView, string> = {
  today: "今天",
  upcoming: "计划",
  important: "重要",
  all: "全部",
  completed: "已完成",
  trash: "最近删除"
};

export type SortMode = "manual" | "due" | "priority" | "created" | "title";
export const SORT_LABELS: Record<SortMode, string> = {
  manual: "手动排序",
  due: "按日期",
  priority: "按优先级",
  created: "按创建时间",
  title: "按标题"
};

/** Whether a top-level item belongs to a view (subtasks follow their root). */
export function itemInView(item: TodoItem, view: ViewKey, today: string): boolean {
  if (view === "trash") return item.deleted;
  if (item.deleted) return false;
  switch (view) {
    case "today":
      return !item.done && item.dueDate !== null && item.dueDate <= today;
    case "upcoming":
      return !item.done && item.dueDate !== null;
    case "important":
      return !item.done && item.priority === 3;
    case "all":
      return true;
    case "completed":
      return item.done;
    default:
      if (view.startsWith("list:")) return item.listId === view.slice(5);
      if (view.startsWith("tag:")) return item.tags.includes(view.slice(4));
      return false;
  }
}

/** Views where completed items are listed in a collapsible group. */
export function viewShowsCompletedGroup(view: ViewKey): boolean {
  return view === "all" || view.startsWith("list:") || view.startsWith("tag:");
}

export function compareItems(a: TodoItem, b: TodoItem, sort: SortMode): number {
  switch (sort) {
    case "due": {
      const ad = a.dueDate ? `${a.dueDate} ${a.dueTime ?? "99:99"}` : "9999";
      const bd = b.dueDate ? `${b.dueDate} ${b.dueTime ?? "99:99"}` : "9999";
      if (ad !== bd) return ad < bd ? -1 : 1;
      return b.priority - a.priority || compareOrder(a, b);
    }
    case "priority":
      return b.priority - a.priority || compareItems(a, b, "due");
    case "created":
      return b.createdAt - a.createdAt;
    case "title":
      return titleText(a.title).localeCompare(titleText(b.title), "zh-Hans-CN") || compareOrder(a, b);
    case "manual":
    default:
      return compareOrder(a, b);
  }
}

export function matchesSearch(item: TodoItem, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  const haystack = [titleText(item.title), stripHtml(item.notes), item.tags.join(" ")]
    .join(" ")
    .toLowerCase();
  return needle.split(/\s+/).every((part) => haystack.includes(part));
}

/** Plain text of stored HTML for search / previews (no DOM needed). */
export function stripHtml(html: string): string {
  return html
    .replace(/<(br|\/p|\/li|\/h[1-3]|\/blockquote|\/pre)\s*\/?>/gi, " ")
    .replace(/<[^>]*>/g, "")
    .replace(/&nbsp;/g, " ")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ")
    .trim();
}

/** Plain text of an item title (stored as inline rich text). */
export function titleText(title: string): string {
  return stripHtml(title);
}

/** Plain text → the inline-HTML form a title is stored in. */
export function escapeTitle(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// ---------------------------------------------------------------- hierarchy

export function childrenOf(items: readonly TodoItem[], parentId: string, includeDeleted = false): TodoItem[] {
  return items.filter((item) => item.parentId === parentId && (includeDeleted || !item.deleted)).sort(compareOrder);
}

/** Every descendant id of `id` (bounded by the subtask depth). */
export function descendantIds(items: readonly TodoItem[], id: string): string[] {
  const byParent = new Map<string, TodoItem[]>();
  for (const item of items) {
    if (!item.parentId) continue;
    const bucket = byParent.get(item.parentId) ?? [];
    bucket.push(item);
    byParent.set(item.parentId, bucket);
  }
  const result: string[] = [];
  const stack = [id];
  const seen = new Set<string>([id]);
  while (stack.length) {
    const current = stack.pop() as string;
    for (const child of byParent.get(current) ?? []) {
      if (seen.has(child.id)) continue;
      seen.add(child.id);
      result.push(child.id);
      stack.push(child.id);
    }
  }
  return result;
}

export function depthOf(items: ReadonlyMap<string, TodoItem>, item: TodoItem): number {
  let depth = 0;
  let current: TodoItem | undefined = item;
  const seen = new Set<string>();
  while (current?.parentId && !seen.has(current.id) && depth < 16) {
    seen.add(current.id);
    current = items.get(current.parentId);
    depth += 1;
  }
  return depth;
}

export function progressOf(items: readonly TodoItem[], id: string): { done: number; total: number } {
  const children = childrenOf(items, id);
  return { done: children.filter((child) => child.done).length, total: children.length };
}

// ---------------------------------------------------------------- quick add

export interface QuickAddResult {
  title: string;
  dueDate: string | null;
  dueTime: string | null;
  priority: Priority;
  tags: string[];
}

const CN_DIGITS: Record<string, number> = { 一: 1, 二: 2, 三: 3, 四: 4, 五: 5, 六: 6, 日: 0, 天: 0, 七: 0 };

/**
 * Light natural-language parsing for the quick-add field, e.g.
 * `周报 明天 17:30 !高 #工作`. Recognized tokens are removed from the title.
 */
export function parseQuickAdd(input: string, today: string): QuickAddResult {
  let text = ` ${input} `;
  let dueDate: string | null = null;
  let dueTime: string | null = null;
  let priority: Priority = 0;
  const tags: string[] = [];

  text = text.replace(/\s#([^\s#]{1,32})(?=\s)/g, (_match, tag: string) => {
    const normalized = normalizeTag(tag);
    if (normalized && !tags.includes(normalized)) tags.push(normalized);
    return " ";
  });
  text = text.replace(/\s!(高|中|低|1|2|3|!{0,2})(?=\s)/g, (_match, level: string) => {
    // !高 !中 !低 · !3 !2 !1 · !!! !! !
    priority = level === "高" || level === "3" || level === "!!" ? 3 : level === "中" || level === "2" || level === "!" ? 2 : 1;
    return " ";
  });
  const relative: Record<string, number> = { 今天: 0, 明天: 1, 后天: 2, 大后天: 3 };
  text = text.replace(/\s(大后天|后天|明天|今天)(?=\s|\d|上午|下午|晚上|早上|中午)/g, (_match, word: string) => {
    dueDate = addDays(today, relative[word]);
    return " ";
  });
  text = text.replace(/\s(下)?(?:周|星期)([一二三四五六日天七])(?=\s|\d|上午|下午|晚上|早上|中午)/g, (_match, next: string | undefined, day: string) => {
    const target = CN_DIGITS[day];
    const current = fromDateKey(today).getDay();
    if (next) {
      // 下周X: that weekday in the next Monday-based week.
      const toNextMonday = (8 - current) % 7 || 7;
      dueDate = addDays(today, toNextMonday + (target === 0 ? 6 : target - 1));
    } else {
      // 周X: the coming such weekday (today counts).
      dueDate = addDays(today, (target - current + 7) % 7);
    }
    return " ";
  });
  text = text.replace(/\s(?:(\d{4})[-/年])?(\d{1,2})[-/月](\d{1,2})日?(?=\s|\d|上午|下午|晚上)/g, (match, year: string | undefined, month: string, day: string) => {
    const m = Number(month);
    const d = Number(day);
    if (m < 1 || m > 12 || d < 1 || d > 31) return match;
    let y = year ? Number(year) : fromDateKey(today).getFullYear();
    let key = `${y}-${pad(m)}-${pad(d)}`;
    if (!year && key < today) {
      y += 1;
      key = `${y}-${pad(m)}-${pad(d)}`;
    }
    dueDate = key;
    return " ";
  });
  // 17:30 · 下午3点 · 9点半 · 晚上8点15分
  text = text.replace(/\s(上午|早上|中午|下午|晚上)?(\d{1,2})(?::(\d{2})|点(?:(半)|(\d{1,2})分?)?)(?=\s)/g, (match, period: string | undefined, hour: string, minute: string | undefined, half: string | undefined, minutes: string | undefined) => {
    let h = Number(hour);
    const m = minute ? Number(minute) : half ? 30 : minutes ? Number(minutes) : 0;
    if ((period === "下午" || period === "晚上") && h < 12) h += 12;
    if (period === "中午" && h < 11) h += 12;
    if (h > 23 || m > 59) return match;
    dueTime = `${pad(h)}:${pad(m)}`;
    return " ";
  });
  if (dueTime && !dueDate) dueDate = today;
  return { title: text.replace(/\s+/g, " ").trim(), dueDate, dueTime, priority, tags };
}

// ----------------------------------------------------------------- misc

export function newId(): string {
  // UUIDv7: time ordered, so Core's keyset listing follows creation order.
  const bytes = new Uint8Array(16);
  globalThis.crypto.getRandomValues(bytes);
  const now = Date.now();
  bytes[0] = Math.floor(now / 2 ** 40) & 0xff;
  bytes[1] = Math.floor(now / 2 ** 32) & 0xff;
  bytes[2] = Math.floor(now / 2 ** 24) & 0xff;
  bytes[3] = Math.floor(now / 2 ** 16) & 0xff;
  bytes[4] = Math.floor(now / 2 ** 8) & 0xff;
  bytes[5] = now & 0xff;
  bytes[6] = (bytes[6] & 0x0f) | 0x70;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
