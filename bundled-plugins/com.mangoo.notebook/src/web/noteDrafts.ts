import type { NotebookNote } from "@mangoo/core-client";

export type DraftFields = Pick<NotebookNote, "title" | "markdown" | "styles" | "format">;
export interface DraftInfo {
  id: string;
  key: string;
  writer?: string;
  revision: number;
  updatedAt: number;
  title: string;
  preview: string;
}
export interface SavedDraft extends DraftInfo { fields: DraftFields }

const legacyPrefix = "mangoo.notebook.draft:";
const summaries = "summaries";
const bodies = "bodies";
let database: Promise<IDBDatabase> | undefined;
let channel: BroadcastChannel | undefined;

export function draftToken(): string {
  return [...crypto.getRandomValues(new Uint8Array(16))].map(value => value.toString(16).padStart(2, "0")).join("");
}

function openDrafts() {
  if (!database) {
    database = new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open("mangoo.notebook.drafts", 1);
      request.onupgradeneeded = () => {
        const metadata = request.result.createObjectStore(summaries, { keyPath: "key" });
        metadata.createIndex("library", "libraryId");
        metadata.createIndex("note", ["libraryId", "id"]);
        request.result.createObjectStore(bodies);
      };
      request.onerror = () => reject(request.error);
      request.onsuccess = () => {
        const db = request.result;
        db.onversionchange = () => { db.close(); database = undefined; };
        resolve(db);
      };
    }).catch(error => { database = undefined; throw error; });
  }
  return database;
}

async function transaction<T>(stores: string[], mode: IDBTransactionMode, run: (tx: IDBTransaction) => IDBRequest<T>): Promise<T> {
  const db = await openDrafts();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(stores, mode);
    let request: IDBRequest<T>;
    try { request = run(tx); }
    catch (error) { tx.abort(); reject(error); return; }
    tx.oncomplete = () => resolve(request.result);
    tx.onabort = () => reject(tx.error ?? new Error("本机草稿写入中断。"));
  });
}

function messages() {
  if (!channel && typeof BroadcastChannel !== "undefined") channel = new BroadcastChannel("mangoo.notebook.drafts");
  return channel;
}
export function onDraftChange(libraryId: string, id: string, listener: () => void) {
  const changes = messages();
  const received = (event: MessageEvent) => {
    if (event.data?.libraryId === libraryId && event.data?.id === id) listener();
  };
  const prefix = legacyPrefix + libraryId + ":" + id;
  const legacyChanged = (event: StorageEvent) => {
    if (!event.key || event.key === prefix || event.key.startsWith(prefix + ":")) listener();
  };
  changes?.addEventListener("message", received);
  window.addEventListener("storage", legacyChanged);
  return () => {
    changes?.removeEventListener("message", received);
    window.removeEventListener("storage", legacyChanged);
  };
}

function legacyDraft(key: string, prefix: string): SavedDraft | undefined {
  const [id, writer] = key.slice(prefix.length).split(":");
  const value = JSON.parse(localStorage.getItem(key) ?? "null");
  if (typeof value?.fields?.markdown !== "string" || typeof value.fields.title !== "string" || !Number.isSafeInteger(value.revision)) return;
  return { id, writer, key, fields: value.fields, revision: value.revision, updatedAt: value.updatedAt || 0,
    title: value.fields.title, preview: value.fields.markdown.slice(0, 64) };
}

function legacyDrafts(libraryId: string, include: (id: string) => boolean) {
  const prefix = legacyPrefix + libraryId + ":";
  const drafts: DraftInfo[] = [];
  try {
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (!key?.startsWith(prefix) || !include(key.slice(prefix.length).split(":")[0])) continue;
      try {
        const draft = legacyDraft(key, prefix);
        if (draft) {
          const { fields: _fields, ...info } = draft;
          drafts.push(info);
        }
      } catch { /* Ignore individual damaged legacy entries. */ }
    }
  } catch { /* IndexedDB remains usable if legacy storage cannot be read. */ }
  return drafts;
}

export async function readDrafts(libraryId: string, noteId?: string, include: (id: string) => boolean = () => true) {
  const allowed = (id: string) => (!noteId || id === noteId) && include(id);
  const old = legacyDrafts(libraryId, allowed);
  const records: DraftInfo[] = await transaction([summaries], "readonly", tx => {
    const store = tx.objectStore(summaries);
    return noteId ? store.index("note").getAll([libraryId, noteId]) : store.index("library").getAll(libraryId);
  }).catch(error => {
    if (old.length) return [];
    throw error;
  });
  return [...old, ...records.filter(draft => allowed(draft.id))].sort((a, b) => b.updatedAt - a.updatedAt);
}

export function orphanDrafts(libraryId: string, liveIds: Set<string>) {
  return readDrafts(libraryId, undefined, id => !liveIds.has(id));
}

export async function loadDraft(libraryId: string, info: DraftInfo): Promise<SavedDraft> {
  const fields: DraftFields | undefined = info.key.startsWith(legacyPrefix)
    ? legacyDraft(info.key, legacyPrefix + libraryId + ":")?.fields
    : await transaction([bodies], "readonly", tx => tx.objectStore(bodies).get(info.key));
  if (!fields) throw new Error("这份草稿已由另一窗口更新。");
  return { ...info, fields };
}

export async function removeDraft(libraryId: string, id: string, key: string) {
  if (key.startsWith(legacyPrefix)) localStorage.removeItem(key);
  else await transaction([summaries, bodies], "readwrite", tx => {
    tx.objectStore(summaries).delete(key);
    return tx.objectStore(bodies).delete(key);
  });
  messages()?.postMessage({ libraryId, id });
}

export async function writeDraft(libraryId: string, id: string, writer: string, previousKey: string | undefined,
  revision: number, fields: DraftFields): Promise<string> {
  const key = draftToken();
  await transaction([summaries, bodies], "readwrite", tx => {
    const metadata = tx.objectStore(summaries);
    const contents = tx.objectStore(bodies);
    if (previousKey && !previousKey.startsWith(legacyPrefix)) {
      metadata.delete(previousKey);
      contents.delete(previousKey);
    }
    metadata.put({ key, libraryId, id, writer, revision, updatedAt: Date.now(), title: fields.title, preview: fields.markdown.slice(0, 64) });
    return contents.put(fields, key);
  });
  if (previousKey?.startsWith(legacyPrefix)) localStorage.removeItem(previousKey);
  messages()?.postMessage({ libraryId, id });
  return key;
}
