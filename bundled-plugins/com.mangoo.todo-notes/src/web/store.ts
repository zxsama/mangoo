import { useSyncExternalStore } from "react";
import { CoreApiError, createCoreClient } from "@mangoo/web/plugin-host";
import type { PluginRecord, PluginRecordOperation } from "@mangoo/core-client";

import {
  COLLECTION_ATTACHMENTS,
  COLLECTION_ITEMS,
  COLLECTION_LISTS,
  MAX_TITLE_LENGTH,
  TODO_PLUGIN_ID,
  attachmentFromRecord,
  descendantIds,
  itemFields,
  itemFromRecord,
  keyBetween,
  listFromRecord,
  localTimestamp,
  newId,
  nextOccurrence,
  type ItemPatch,
  type TodoAttachment,
  type TodoItem,
  type TodoList
} from "../shared/model.ts";
import { todoBridge } from "./bridge.ts";
import { sanitizeTitle } from "./richText.ts";
import { attachmentUrl } from "./images.ts";

export type TodoStatusKind = "loading" | "ready" | "disabled" | "error";

export interface TodoSnapshot {
  status: TodoStatusKind;
  items: TodoItem[];
  lists: TodoList[];
  /** Last write / load problem (cleared by the next success). */
  error: string | null;
  version: number;
}

const BATCH_LIMIT = 200;

function errorCode(error: unknown): string | null {
  if (error instanceof CoreApiError && error.body && typeof error.body === "object") {
    const code = (error.body as { code?: unknown }).code;
    return typeof code === "string" ? code : null;
  }
  return null;
}

function describe(error: unknown): string {
  if (error instanceof CoreApiError) {
    const body = error.body as { error?: unknown } | null;
    if (body && typeof body.error === "string") return body.error;
    return `请求失败（${error.status}）`;
  }
  return error instanceof Error ? error.message : String(error);
}

function emptyItem(id: string, now: number): TodoItem {
  return {
    id,
    listId: null,
    parentId: null,
    title: "",
    notes: "",
    done: false,
    completedAt: null,
    priority: 0,
    dueDate: null,
    dueTime: null,
    remindAt: null,
    recurrence: null,
    tags: [],
    order: "i",
    createdAt: now,
    updatedAt: now,
    deleted: false
  };
}

/** Item fields as a full put (everything but identity / bookkeeping). */
function fullFields(item: TodoItem): Record<string, unknown> {
  const { id: _id, createdAt: _c, updatedAt: _u, deleted: _d, ...fields } = item;
  return itemFields(fields);
}

/**
 * The TODO data of this device's notes window / pinned notes. Every record
 * lives in the plugin's shared scope (no Library affiliation). Writes are
 * optimistic; the Desktop change feed (`dataChanged`) reloads after
 * synchronized or other-window edits.
 */
export class TodoStore {
  private snapshot: TodoSnapshot = { status: "loading", items: [], lists: [], error: null, version: 0 };
  private readonly listeners = new Set<() => void>();
  private loadSequence = 0;
  private reloadTimer: ReturnType<typeof setTimeout> | null = null;
  private pendingWrites = 0;
  private reloadAfterWrites = false;
  private readonly attachments = new Map<string, Promise<TodoAttachment | null>>();
  /** Display URLs of attachments already loaded (synchronous lookups). */
  private readonly attachmentUrls = new Map<string, string>();
  private unsubscribe: (() => void) | null = null;

  start(): void {
    void this.load();
    this.unsubscribe = todoBridge()?.onDataChanged(() => this.scheduleReload()) ?? null;
  }

  stop(): void {
    this.unsubscribe?.();
    this.unsubscribe = null;
    if (this.reloadTimer) clearTimeout(this.reloadTimer);
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getSnapshot = (): TodoSnapshot => this.snapshot;

  private set(patch: Partial<TodoSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch, version: this.snapshot.version + 1 };
    for (const listener of this.listeners) listener();
  }

  scheduleReload(delay = 120): void {
    if (this.pendingWrites > 0) {
      this.reloadAfterWrites = true;
      return;
    }
    if (this.reloadTimer) clearTimeout(this.reloadTimer);
    this.reloadTimer = setTimeout(() => {
      this.reloadTimer = null;
      void this.load();
    }, delay);
  }

  private async fetch(collection: string, includeDeleted: boolean): Promise<PluginRecord[]> {
    const response = await createCoreClient().listSharedPluginRecords(TODO_PLUGIN_ID, { collection, includeDeleted });
    return response.items;
  }

  async load(): Promise<void> {
    const sequence = ++this.loadSequence;
    try {
      const [items, lists] = await Promise.all([this.fetch(COLLECTION_ITEMS, true), this.fetch(COLLECTION_LISTS, false)]);
      if (sequence !== this.loadSequence) return;
      this.set({
        status: "ready",
        items: items.filter((record) => record.fields.purged !== true).map(itemFromRecord),
        lists: lists.map(listFromRecord).filter((list) => !list.deleted),
        error: null
      });
    } catch (error) {
      if (sequence !== this.loadSequence) return;
      if (errorCode(error) === "plugin_data_disabled") this.set({ status: "disabled", items: [], lists: [], error: null });
      else this.set({ status: this.snapshot.status === "ready" ? "ready" : "error", error: describe(error) });
    }
  }

  // ------------------------------------------------------------- writing

  private async send(operations: PluginRecordOperation[]): Promise<boolean> {
    if (operations.length === 0) return true;
    this.pendingWrites += 1;
    try {
      const client = createCoreClient();
      for (let index = 0; index < operations.length; index += BATCH_LIMIT) {
        await client.applySharedPluginRecordBatch(TODO_PLUGIN_ID, operations.slice(index, index + BATCH_LIMIT));
      }
      if (this.snapshot.error) this.set({ error: null });
      return true;
    } catch (error) {
      this.set({ error: describe(error) });
      this.reloadAfterWrites = true;
      return false;
    } finally {
      this.pendingWrites -= 1;
      if (this.pendingWrites === 0 && this.reloadAfterWrites) {
        this.reloadAfterWrites = false;
        this.scheduleReload(0);
      }
    }
  }

  private applyLocal(change: (items: Map<string, TodoItem>) => void): void {
    const map = new Map(this.snapshot.items.map((item) => [item.id, item]));
    change(map);
    this.set({ items: [...map.values()] });
  }

  item(id: string): TodoItem | undefined {
    return this.snapshot.items.find((item) => item.id === id);
  }

  list(id: string | null): TodoList | undefined {
    return id ? this.snapshot.lists.find((list) => list.id === id) : undefined;
  }

  private edgeOrder(listId: string | null, parentId: string | null, edge: "first" | "last"): string | null {
    let value: string | null = null;
    for (const item of this.snapshot.items) {
      if (item.deleted || item.parentId !== parentId) continue;
      if (!parentId && item.listId !== listId) continue;
      if (value === null || (edge === "last" ? item.order > value : item.order < value)) value = item.order;
    }
    return value;
  }

  createItem(patch: ItemPatch & { title: string }, options: { position?: "start" | "end" } = {}): TodoItem {
    patch = { ...patch, title: sanitizeTitle(patch.title, MAX_TITLE_LENGTH) };
    const id = newId();
    const now = Date.now();
    const parent = patch.parentId ? this.item(patch.parentId) : undefined;
    const listId = parent ? parent.listId : patch.listId ?? null;
    const parentId = patch.parentId ?? null;
    const order = options.position === "start"
      ? keyBetween(null, this.edgeOrder(listId, parentId, "first"))
      : keyBetween(this.edgeOrder(listId, parentId, "last"), null);
    const item: TodoItem = { ...emptyItem(id, now), ...patch, listId, parentId, order };
    this.applyLocal((items) => items.set(id, item));
    void this.send([{ op: "put", id, collection: COLLECTION_ITEMS, fields: fullFields(item) }]);
    return item;
  }

  updateItem(id: string, patch: ItemPatch): void {
    // Every title write is sanitized here, whatever produced it.
    if (patch.title !== undefined) patch = { ...patch, title: sanitizeTitle(patch.title, MAX_TITLE_LENGTH) };
    const current = this.item(id);
    if (!current || current.deleted) return;
    this.applyLocal((items) => items.set(id, { ...current, ...patch, updatedAt: Date.now() }));
    void this.send([{ op: "put", id, collection: COLLECTION_ITEMS, fields: itemFields(patch) }]);
  }

  /** Move an item (with its subtasks) to a list. */
  moveToList(id: string, listId: string | null): void {
    if (!this.item(id)) return;
    const order = keyBetween(this.edgeOrder(listId, null, "last"), null);
    const ids = [id, ...descendantIds(this.snapshot.items, id)];
    const operations: PluginRecordOperation[] = [];
    this.applyLocal((items) => {
      for (const target of ids) {
        const entry = items.get(target);
        if (!entry) continue;
        const patch: ItemPatch = target === id ? { listId, order, parentId: null } : { listId };
        items.set(target, { ...entry, ...patch });
        operations.push({ op: "put", id: target, collection: COLLECTION_ITEMS, fields: itemFields(patch) });
      }
    });
    void this.send(operations);
  }

  /**
   * Toggle completion. Completing a recurring item rolls it to its next date
   * instead (and returns that date).
   */
  toggleDone(id: string): { rolledTo: string | null } {
    const item = this.item(id);
    if (!item) return { rolledTo: null };
    if (!item.done && item.recurrence && item.dueDate) {
      const nextDate = nextOccurrence(item.dueDate, item.recurrence);
      const offset = item.remindAt !== null ? item.remindAt - localTimestamp(item.dueDate, item.dueTime) : null;
      const remindAt = offset !== null ? localTimestamp(nextDate, item.dueTime) + offset : null;
      const children = descendantIds(this.snapshot.items, id);
      this.updateItem(id, { dueDate: nextDate, remindAt, done: false, completedAt: null });
      for (const child of children) {
        if (this.item(child)?.done) this.updateItem(child, { done: false, completedAt: null });
      }
      return { rolledTo: nextDate };
    }
    this.updateItem(id, { done: !item.done, completedAt: item.done ? null : Date.now() });
    return { rolledTo: null };
  }

  /** Soft delete (to 最近删除), with every subtask. Returns the ids for undo. */
  deleteItems(ids: string[]): string[] {
    const all = new Set<string>();
    for (const id of ids) {
      all.add(id);
      for (const child of descendantIds(this.snapshot.items, id)) all.add(child);
    }
    const targets = [...all].filter((id) => this.item(id) && !this.item(id)?.deleted);
    this.applyLocal((items) => {
      for (const id of targets) {
        const item = items.get(id);
        if (item) items.set(id, { ...item, deleted: true, updatedAt: Date.now() });
      }
    });
    void this.send(targets.map((id): PluginRecordOperation => ({ op: "delete", id })));
    return targets;
  }

  restoreItems(ids: string[]): void {
    const targets = ids.filter((id) => this.item(id)?.deleted);
    const orphan = (item: TodoItem) => Boolean(item.parentId && (!this.item(item.parentId) || this.item(item.parentId)?.deleted) && !targets.includes(item.parentId));
    const operations: PluginRecordOperation[] = [];
    for (const id of targets) {
      const item = this.item(id) as TodoItem;
      operations.push({ op: "restore", id });
      if (orphan(item)) operations.push({ op: "put", id, collection: COLLECTION_ITEMS, fields: { parentId: null } });
    }
    this.applyLocal((items) => {
      for (const id of targets) {
        const item = items.get(id);
        if (item) items.set(id, { ...item, deleted: false, parentId: orphan(item) ? null : item.parentId });
      }
    });
    void this.send(operations);
  }

  /** Permanently clear: content is erased, only an empty marker remains for sync. */
  purgeItems(ids: string[]): void {
    const targets = ids.filter((id) => this.item(id)?.deleted);
    const operations = targets.flatMap((id): PluginRecordOperation[] => [
      { op: "restore", id },
      { op: "put", id, collection: COLLECTION_ITEMS, fields: { purged: true, title: null, notes: null, tags: null, remindAt: null } },
      { op: "delete", id }
    ]);
    this.applyLocal((items) => {
      for (const id of targets) items.delete(id);
    });
    void this.send(operations);
  }

  reorder(id: string, before: TodoItem | null, after: TodoItem | null): void {
    const low = before?.order ?? null;
    const high = after?.order ?? null;
    let order: string;
    try {
      order = keyBetween(low, high !== null && low !== null && high <= low ? null : high);
    } catch {
      order = keyBetween(low, null);
    }
    this.updateItem(id, { order });
  }

  // ------------------------------------------------------------------ lists

  createList(name: string, color: string): TodoList {
    const id = newId();
    const last = this.snapshot.lists.reduce<string | null>((max, list) => (max === null || list.order > max ? list.order : max), null);
    const list: TodoList = { id, name, color, order: keyBetween(last, null), createdAt: Date.now(), deleted: false };
    this.set({ lists: [...this.snapshot.lists, list] });
    void this.send([{ op: "put", id, collection: COLLECTION_LISTS, fields: { name, color, order: list.order } }]);
    return list;
  }

  updateList(id: string, patch: Partial<Pick<TodoList, "name" | "color" | "order">>): void {
    if (!this.list(id)) return;
    this.set({ lists: this.snapshot.lists.map((entry) => (entry.id === id ? { ...entry, ...patch } : entry)) });
    void this.send([{ op: "put", id, collection: COLLECTION_LISTS, fields: { ...patch } }]);
  }

  /** Deleting a list moves its items to the trash with it. Returns what undo needs. */
  deleteList(id: string): { list: TodoList; items: string[] } | null {
    const list = this.list(id);
    if (!list) return null;
    const itemIds = this.snapshot.items.filter((item) => item.listId === id && !item.deleted && !item.parentId).map((item) => item.id);
    const removed = this.deleteItems(itemIds);
    this.set({ lists: this.snapshot.lists.filter((entry) => entry.id !== id) });
    void this.send([{ op: "delete", id }]);
    return { list, items: removed };
  }

  /** Undo `deleteList`: the list and the notes that went with it come back. */
  restoreList(deleted: { list: TodoList; items: string[] }): void {
    const { list } = deleted;
    if (!this.list(list.id)) this.set({ lists: [...this.snapshot.lists, { ...list, deleted: false }] });
    void this.send([{ op: "restore", id: list.id }]);
    this.restoreItems(deleted.items);
  }

  /** Every tag in use (for suggestions), most used first. */
  tags(): string[] {
    const counts = new Map<string, number>();
    for (const item of this.snapshot.items) {
      if (item.deleted) continue;
      for (const tag of item.tags) counts.set(tag, (counts.get(tag) ?? 0) + 1);
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).map(([tag]) => tag);
  }

  // ------------------------------------------------------------ attachments

  cachedAttachmentUrl(id: string): string | null {
    return this.attachmentUrls.get(id) ?? null;
  }

  attachment(id: string): Promise<TodoAttachment | null> {
    let pending = this.attachments.get(id);
    if (!pending) {
      pending = createCoreClient().getSharedPluginRecord(TODO_PLUGIN_ID, id)
        .then((record) => {
          const attachment = record.deleted ? null : attachmentFromRecord(record);
          if (attachment) this.attachmentUrls.set(id, attachmentUrl(attachment));
          return attachment;
        })
        .catch(() => null);
      this.attachments.set(id, pending);
    }
    return pending;
  }

  async addAttachment(item: TodoItem, image: { data: string; mime: string; width: number; height: number; name: string }): Promise<TodoAttachment | null> {
    const id = newId();
    const ok = await this.send([{ op: "put", id, collection: COLLECTION_ATTACHMENTS, fields: { itemId: item.id, ...image } }]);
    if (!ok) return null;
    const attachment: TodoAttachment = { id, itemId: item.id, ...image };
    this.attachments.set(id, Promise.resolve(attachment));
    this.attachmentUrls.set(id, attachmentUrl(attachment));
    return attachment;
  }
}

export function useTodoSnapshot(store: TodoStore): TodoSnapshot {
  return useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
}
