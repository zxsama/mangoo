/**
 * Backup file of the TODO notes plugin: every live list, note (with its
 * subtasks) and pasted image, as the plugin's own records. Pure functions so
 * they are unit-tested; the settings window reads / writes the file.
 */

import { COLLECTION_ATTACHMENTS, COLLECTION_ITEMS, COLLECTION_LISTS, TODO_PLUGIN_ID } from "./model.ts";

export const BACKUP_FORMAT = "mangoo-todo-notes-backup";
export const BACKUP_VERSION = 1;
/** Lists first, then notes, then images: a restore never shows a hole. */
export const BACKUP_COLLECTIONS = [COLLECTION_LISTS, COLLECTION_ITEMS, COLLECTION_ATTACHMENTS] as const;
export type BackupCollection = (typeof BACKUP_COLLECTIONS)[number];

export interface BackupRecord {
  id: string;
  collection: BackupCollection;
  fields: Record<string, unknown>;
}

export interface TodoBackup {
  format: typeof BACKUP_FORMAT;
  version: typeof BACKUP_VERSION;
  pluginId: typeof TODO_PLUGIN_ID;
  exportedAt: string;
  records: BackupRecord[];
}

export interface BackupSummary {
  lists: number;
  notes: number;
  subtasks: number;
  images: number;
}

const ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const FIELD = /^[A-Za-z][A-Za-z0-9_]{0,63}$/;
/** Matches Core's per-record field limit. */
const MAX_FIELDS = 64;
export const MAX_BACKUP_RECORDS = 50_000;

export function createBackup(records: readonly BackupRecord[], now = new Date()): TodoBackup {
  const order = (collection: string) => BACKUP_COLLECTIONS.indexOf(collection as BackupCollection);
  return {
    format: BACKUP_FORMAT,
    version: BACKUP_VERSION,
    pluginId: TODO_PLUGIN_ID,
    exportedAt: now.toISOString(),
    records: [...records].sort((a, b) => order(a.collection) - order(b.collection))
  };
}

export function summarizeBackup(records: readonly BackupRecord[]): BackupSummary {
  const summary: BackupSummary = { lists: 0, notes: 0, subtasks: 0, images: 0 };
  for (const record of records) {
    if (record.collection === COLLECTION_LISTS) summary.lists += 1;
    else if (record.collection === COLLECTION_ATTACHMENTS) summary.images += 1;
    else if (typeof record.fields.parentId === "string" && record.fields.parentId) summary.subtasks += 1;
    else summary.notes += 1;
  }
  return summary;
}

/** Validate a backup file's text; throws a user-facing message. */
export function parseBackup(text: string): TodoBackup {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    throw new Error("不是有效的备份文件（无法解析）");
  }
  const value = raw as Partial<TodoBackup> | null;
  if (!value || typeof value !== "object" || value.format !== BACKUP_FORMAT) throw new Error("不是便笺的备份文件");
  if (value.version !== BACKUP_VERSION) throw new Error("备份文件的版本不受支持");
  if (!Array.isArray(value.records) || value.records.length > MAX_BACKUP_RECORDS) throw new Error("备份文件内容无效");
  const seen = new Set<string>();
  const records: BackupRecord[] = [];
  for (const entry of value.records as unknown[]) {
    const record = entry as Partial<BackupRecord> | null;
    if (!record || typeof record !== "object") continue;
    if (typeof record.id !== "string" || !ID.test(record.id) || seen.has(record.id.toLowerCase())) continue;
    if (!BACKUP_COLLECTIONS.includes(record.collection as BackupCollection)) continue;
    if (!record.fields || typeof record.fields !== "object" || Array.isArray(record.fields)) continue;
    const fields: Record<string, unknown> = {};
    for (const [name, field] of Object.entries(record.fields).slice(0, MAX_FIELDS)) {
      if (FIELD.test(name) && name !== "purged") fields[name] = field;
    }
    seen.add(record.id.toLowerCase());
    records.push({ id: record.id, collection: record.collection as BackupCollection, fields });
  }
  return createBackup(records, new Date(typeof value.exportedAt === "string" ? value.exportedAt : Date.now()));
}

/** `mangoo-便笺-备份-2026-11-15.json` */
export function backupFileName(now = new Date()): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `mangoo-便笺-备份-${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}.json`;
}
