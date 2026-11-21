import type { NotebookNote, NoteInput, NoteRequestOptions } from "@mangoo/core-client";
import { CoreApiError, createCoreClient } from "@mangoo/web/plugin-host";
import { draftToken, loadDraft, onDraftChange, readDrafts, removeDraft, writeDraft, type DraftInfo, type DraftFields as Fields, type SavedDraft } from "./noteDrafts";
export { orphanDrafts } from "./noteDrafts";

// A new page, including a duplicated tab, is a distinct writer. Drafts of a
// crashed/reloaded window are offered explicitly instead of silently applied.
const pageWriter = draftToken();

export type SaveStatus = "saved" | "pending" | "saving" | "failed" | "conflict" | "deleted";

export class NoteSession {
  note: NotebookNote;
  status: SaveStatus = "saved";
  error = "";
  private editNumber = 0;
  private savedNumber = 0;
  private inFlight?: Promise<void>;
  private timer?: ReturnType<typeof setTimeout>;
  private disposed = false;
  private key?: string;
  private adopted?: SavedDraft;
  private baseRevision: number;
  draftOnDisk = false;
  otherDrafts: DraftInfo[] = [];
  draftError = "";
  readonly ready: Promise<void>;
  private stopListening: () => void;
  private draftTimer?: ReturnType<typeof setTimeout>;
  private draftVersion = 0;
  private draftListVersion = 0;
  private draftWork: Promise<void> = Promise.resolve();

  constructor(readonly options: NoteRequestOptions & { contextRevision: number }, remote: NotebookNote, private changed: () => void) {
    this.note = remote;
    this.baseRevision = remote.revision;
    this.ready = this.readOtherDrafts(true);
    this.stopListening = onDraftChange(options.libraryId, remote.id, () => { void this.readOtherDrafts(); });
  }

  private async readOtherDrafts(initial = false) {
    const owner = ++this.draftListVersion;
    try {
      const drafts = await readDrafts(this.options.libraryId, this.note.id);
      if (this.disposed || owner !== this.draftListVersion) return;
      const own = initial && !this.editNumber && drafts.find(draft => draft.writer === pageWriter);
      if (own) {
        const snapshot = await loadDraft(this.options.libraryId, own);
        if (this.disposed || owner !== this.draftListVersion) return;
        if (!this.editNumber) {
          this.key = own.key;
          if (sameFields(snapshot.fields, this.fields())) await this.clearDraft();
          else this.restoreFields(snapshot);
        }
      }
      this.otherDrafts = drafts.filter(draft => draft.key !== this.key && draft.key !== this.adopted?.key &&
        !(draft.writer === pageWriter && this.editNumber > 0));
    } catch {
      this.draftError = "无法读取本机草稿，请保持窗口打开直至笔记保存完成。";
    }
    if (!this.disposed) this.changed();
  }
  private restoreFields(draft: SavedDraft) {
    this.note = { ...this.note, ...draft.fields };
    this.baseRevision = draft.revision;
    this.draftOnDisk = true;
    this.editNumber++;
    this.status = draft.revision === this.note.revision ? "pending" : "conflict";
    if (this.status === "pending") this.schedule();
  }
  async restoreOtherDraft(info: DraftInfo) {
    if (this.status !== "saved") throw new Error("请先处理当前草稿。");
    const draft = await loadDraft(this.options.libraryId, info);
    if (this.disposed || this.status !== "saved") throw new Error("请先处理当前草稿。");
    this.adopted = draft;
    this.restoreFields(draft);
    this.remember();
    await this.waitForDraft();
    await this.readOtherDrafts();
  }
  async dismissOtherDraft(info: DraftInfo) {
    await removeDraft(this.options.libraryId, this.note.id, info.key);
    await this.readOtherDrafts();
  }

  private fields(): Fields {
    const { title, markdown, format, styles } = this.note;
    return { title, markdown, format, styles };
  }
  private retainCollectedFiles(remote: NotebookNote): NotebookNote {
    // Uploads publish immutable attachment records independently of note
    // revisions. A save response can predate an upload we already received.
    const known = new Set(remote.attachments.map(file => file.id));
    const additional = this.note.attachments.filter(file => !known.has(file.id));
    return additional.length ? { ...remote, attachments: [...remote.attachments, ...additional] } : remote;
  }
  private remember() {
    this.draftVersion++;
    this.draftOnDisk = false;
    // Sample a typing burst without queuing a full copy of the document per keystroke.
    this.draftTimer ??= setTimeout(() => this.queueDraft(), 120);
  }
  private queueDraft() {
    clearTimeout(this.draftTimer);
    this.draftTimer = undefined;
    const version = this.draftVersion;
    this.draftWork = this.draftWork.then(async () => {
      if (version !== this.draftVersion) return;
      this.key = await writeDraft(this.options.libraryId, this.note.id, pageWriter, this.key, this.baseRevision, this.fields());
      if (version === this.draftVersion) {
        this.draftOnDisk = true;
        this.draftError = "";
      }
    }).catch(() => {
      this.draftOnDisk = false;
      this.draftError = "本机草稿写入失败，存储空间可能不足。请保持窗口打开直至保存完成，或导出笔记。";
    }).finally(() => { if (!this.disposed) this.changed(); });
  }
  async waitForDraft() {
    if (this.draftTimer !== undefined) this.queueDraft();
    await this.draftWork;
  }
  clearDraft(): Promise<void> {
    clearTimeout(this.draftTimer);
    this.draftTimer = undefined;
    this.draftVersion++;
    this.draftOnDisk = false;
    this.adopted = undefined;
    const clear = this.draftWork.then(async () => {
      if (this.key) await removeDraft(this.options.libraryId, this.note.id, this.key);
      this.key = undefined;
    });
    this.draftWork = clear.catch(() => { this.draftError = "本机草稿清理失败，可重新打开并处理这份草稿。"; });
    return clear;
  }
  private schedule() {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => { void this.flush().catch(() => {}); }, 450);
  }
  edit(patch: Partial<Fields>) {
    this.note = { ...this.note, ...patch };
    this.editNumber++;
    this.remember();
    if (this.status !== "conflict" && this.status !== "deleted") { this.status = "pending"; this.schedule(); }
    this.changed();
  }
  async flush(): Promise<void> {
    clearTimeout(this.timer);
    while (this.inFlight) await this.inFlight;
    if (this.savedNumber === this.editNumber || this.disposed) return;
    if (this.status === "deleted") throw new Error("原笔记已删除，请先恢复草稿。");
    if (this.status === "conflict") throw new Error("请先处理笔记版本冲突。");
    const target = this.editNumber;
    this.status = "saving"; this.changed();
    const body: NoteInput = { ...this.fields(), title: this.note.title.trim() || "无标题" };
    this.inFlight = createCoreClient().updateNote(this.note.id, { ...body, expectedRevision: this.baseRevision }, this.options)
      .then(async remote => {
        this.savedNumber = target;
        this.baseRevision = remote.revision;
        const committed = this.retainCollectedFiles(remote);
        this.note = target === this.editNumber ? committed : { ...committed, ...this.fields() };
        const adopted = this.adopted;
        this.error = "";
        this.draftError = "";
        if (target === this.editNumber) {
          this.status = "saved";
          await this.clearDraft().catch(() => {});
        } else { this.remember(); this.status = "pending"; }
        if (adopted) {
          if (sameFields(adopted.fields, body)) {
            await removeDraft(this.options.libraryId, this.note.id, adopted.key).catch(() => {
              this.draftError = "笔记已保存，本机仍保留一份恢复草稿。";
            });
          }
          if (!this.disposed) await this.readOtherDrafts();
        }
      }).catch(cause => {
        const conflict = cause instanceof CoreApiError && cause.status === 409 && JSON.stringify(cause.body).includes("note_revision_conflict");
        const deleted = cause instanceof CoreApiError && cause.status === 404 && JSON.stringify(cause.body).includes("note_not_found");
        this.status = deleted ? "deleted" : conflict ? "conflict" : "failed";
        this.error = deleted ? "原笔记已删除。草稿可以恢复为新笔记，附件需重新添加。" : conflict ? "另一处已保存了更新。" : this.draftOnDisk ? "保存失败，编辑内容保留在本机草稿中。" : "保存失败。请保持窗口打开并导出正文。";
        throw cause;
      }).finally(() => { this.inFlight = undefined; if (!this.disposed) this.changed(); });
    await this.inFlight;
    if (target !== this.editNumber) await this.flush();
  }
  async resolve(keepDraft: boolean) {
    const remote = await createCoreClient().getNote(this.note.id, this.options);
    this.baseRevision = remote.revision;
    if (keepDraft) {
      this.note = { ...remote, ...this.fields() };
      this.status = "pending"; this.remember();
      await this.flush();
    } else {
      this.note = remote; this.savedNumber = this.editNumber; this.status = "saved";
      this.error = ""; await this.clearDraft(); await this.readOtherDrafts();
    }
  }
  refresh(remote: NotebookNote) {
    if (this.status === "saved" && remote.id === this.note.id && remote.revision >= this.note.revision) {
      this.note = this.retainCollectedFiles(remote); this.baseRevision = remote.revision; this.changed();
    }
  }
  markDeleted() {
    if (this.status === "saved" || this.status === "deleted") return;
    clearTimeout(this.timer);
    this.status = "deleted";
    this.error = "原笔记已删除。草稿可以恢复为新笔记，附件需重新添加。";
    this.changed();
  }
  dispose() {
    this.disposed = true;
    clearTimeout(this.timer);
    if (this.draftTimer !== undefined) this.queueDraft();
    this.stopListening();
  }
}

function sameFields(left: Fields, right: NoteInput) {
  return left.title === right.title && left.markdown === right.markdown && left.format === right.format &&
    JSON.stringify(left.styles) === JSON.stringify(right.styles);
}
