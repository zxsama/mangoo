import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { NotebookNote, NotebookTree, NoteRequestOptions, NoteSummary } from "@mangoo/core-client";
import {
  CoreApiError,
  createCoreClient,
  icons,
  LiquidGlassButton,
  subscribeBeforeLibraryChange,
} from "@mangoo/web/plugin-host";
import { NotebookIndex } from "./NotebookIndex";
import { NoteEditor, type NoteTabView } from "./editor/NoteEditor";
import { NotebookTabStrip } from "./NotebookTabStrip";
import { readTabs, reorderTab, retainTabs, selectTab, writeTabs, type NotebookTabs } from "./notebookTabs";
import { DocumentReader } from "./readers/DocumentReader";
import { exportNote } from "./exportNote";
import { NoteSession, orphanDrafts } from "./noteSession";
import { loadDraft, removeDraft, type DraftInfo } from "./noteDrafts";
import { importFormats, importNoteFiles } from "./importNotes";
import "./notebook.css";

const statusLabel = {
  saved: "已保存",
  saving: "正在保存…",
  pending: "等待保存",
  failed: "尚未保存",
  conflict: "版本冲突",
  deleted: "待恢复草稿",
};

function AttachmentView({
  noteId,
  options,
  id,
  name,
  view,
}: {
  noteId: string;
  options: NoteRequestOptions;
  id: string;
  name: string;
  view?: NoteTabView;
}) {
  const load = useCallback(
    () => createCoreClient().getNoteAttachment(noteId, id, options),
    [options, noteId, id],
  );
  return <DocumentReader name={name} load={load} view={view} />;
}

export function NotebookWorkspace({
  libraryId,
  contextRevision,
  titlebarTarget,
  sidebarTarget,
  sidebarOpen,
  onSidebarOpenChange,
  locateRequest,
}: {
  libraryId: string;
  contextRevision: number;
  titlebarTarget?: HTMLElement | null;
  sidebarTarget: HTMLElement | null;
  sidebarOpen: boolean;
  onSidebarOpenChange: (open: boolean) => void;
  locateRequest: number;
}) {
  const client = useMemo(() => createCoreClient(), []);
  const options = useMemo(() => ({ libraryId, contextRevision }), [libraryId, contextRevision]);
  const [orphans, setOrphans] = useState<DraftInfo[]>([]);
  const [notes, setNotes] = useState<NoteSummary[]>([]);
  const [tree, setTree] = useState<NotebookTree>({ folders: [], noteParents: {} });
  const [folderId, setFolderId] = useState<string | null>(null);
  const [reveal, setReveal] = useState<{ folderId: string | null; noteId?: string; stamp: number }>();
  const [loaded, setLoaded] = useState(false);
  const [search, setSearch] = useState("");
  const [bodyMatches, setBodyMatches] = useState<Set<string> | null>(null);
  const [searching, setSearching] = useState(false);
  const searchRequest = useRef<AbortController>();
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const inserts = useRef(new Set<Promise<void>>());
  const trackInsert = useCallback((operation: Promise<void>) => {
    inserts.current.add(operation);
    const settled = () => {
      inserts.current.delete(operation);
    };
    void operation.then(settled, settled);
  }, []);
  const [, redraw] = useState(0);
  const layout = useRef<NotebookTabs | null>(null);
  const sessions = useRef(new Map<string, NoteSession>());
  const views = useRef(new Map<string, NoteTabView>());
  const workspace = useRef<HTMLDivElement>(null);
  const mounted = useRef(true);
  const busyRef = useRef(false);
  const loadGeneration = useRef(0);
  const chooser = useRef<HTMLInputElement>(null);
  const folderChooser = useRef<HTMLInputElement>(null);
  const titleInput = useRef<HTMLInputElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const selectedId = layout.current?.activeId ?? undefined;
  const active = selectedId ? sessions.current.get(selectedId) : undefined;
  const note = active?.note;
  const tabView = selectedId ? views.current.get(selectedId) : undefined;
  const annotating = tabView?.annotating ?? false;
  const report = (e: unknown) => {
    if (mounted.current) setError(e instanceof Error ? e.message : String(e));
  };

  // An inactive clean tab only needs a summary and its view position. Dirty sessions
  // stay alive until their own write finishes, including recoverable conflicts.
  const setLayout = useCallback((next: NotebookTabs) => {
    if (layout.current?.activeId !== next.activeId) setConfirmDelete(false);
    layout.current = next;
    writeTabs(libraryId, next);
    const open = new Set(next.ids);
    for (const [id, entry] of sessions.current) {
      if (!open.has(id) || (id !== next.activeId && entry.status === "saved")) {
        entry.dispose();
        sessions.current.delete(id);
      }
    }
    for (const id of views.current.keys()) if (!open.has(id)) views.current.delete(id);
    if (next.activeId) {
      const view = views.current.get(next.activeId) ?? {};
      views.current.delete(next.activeId);
      views.current.set(next.activeId, view);
    }
    const oldest = [...views.current.values()].slice(0, -4);
    for (const view of oldest) delete view.undo;
    redraw(i => i + 1);
  }, [libraryId]);

  const show = useCallback((item: NotebookNote) => {
    const id = item.id;
    if (!sessions.current.has(id)) {
      let title = item.title;
      let revision = item.revision;
      const entry = new NoteSession(options, item, () => {
        if (!mounted.current || sessions.current.get(id) !== entry) return;
        const current = entry.note;
        const view = views.current.get(id);
        if (view) view.revision = current.revision;
        if (current.title !== title || current.revision !== revision) {
          title = current.title;
          revision = current.revision;
          setNotes(rows => rows.map(row => row.id === id
            ? { ...row, title: current.title, updatedAt: current.updatedAt, revision: current.revision } : row));
        }
        if (layout.current?.activeId !== id && entry.status === "saved") {
          entry.dispose();
          sessions.current.delete(id);
        }
        redraw(i => i + 1);
      });
      sessions.current.set(id, entry);
    }
    setLayout(selectTab(layout.current ?? { ids: [], activeId: null }, id));
    const view = views.current.get(id)!;
    if (view.revision !== item.revision) delete view.undo;
    view.revision = item.revision;
    if (view.annotating === undefined) view.annotating = item.format === "document" && /\n\S/.test(item.markdown.trim());
  }, [options, setLayout]);

  const refresh = useCallback(async () => {
    const owner = ++loadGeneration.current;
    const [rows, folders] = await Promise.all([client.listNotes(options), client.getNotebookTree(options)]);
    if (!mounted.current || owner !== loadGeneration.current) return;
    const live = new Set(rows.map(row => row.id));
    setTree(folders);
    setFolderId(current => current && folders.folders.some(folder => folder.id === current) ? current : null);
    setNotes(rows.map(row => {
      const entry = sessions.current.get(row.id);
      return entry && entry.status !== "saved" ? { ...row, title: entry.note.title } : row;
    }));
    setLoaded(true);
    void orphanDrafts(libraryId, live).then(drafts => {
      if (mounted.current && owner === loadGeneration.current) setOrphans(drafts);
    }).catch(report);
    for (const [id, entry] of sessions.current) {
      if (!live.has(id) && entry.status !== "saved") entry.markDeleted();
    }
    const previous = layout.current;
    const next = previous ? retainTabs(previous, id => {
      if (live.has(id)) return true;
      const entry = sessions.current.get(id);
      return !!entry && (entry.status !== "saved" || (id === previous.activeId && inserts.current.size > 0));
    }) : readTabs(libraryId, rows);
    if (!previous || previous.ids.length !== next.ids.length || previous.activeId !== next.activeId) {
      setLayout(next);
    }
    if (!next.activeId) return;
    const summary = rows.find(row => row.id === next.activeId);
    const current = sessions.current.get(next.activeId);
    if (summary && (!current || (current.status === "saved" && current.note.revision !== summary.revision))) {
      const document = await client.getNote(summary.id, options);
      if (!mounted.current || owner !== loadGeneration.current || layout.current?.activeId !== document.id) return;
      if (current) current.refresh(document);
      else {
        show(document);
        setFolderId(folders.noteParents[document.id] ?? null);
      }
    }
  }, [client, options, libraryId, setLayout, show]);

  const waitForInsertions = useCallback(async () => {
    while (inserts.current.size) await Promise.all([...inserts.current]);
  }, []);
  const saveAll = useCallback(async () => {
    await waitForInsertions();
    await Promise.all([...sessions.current.values()].map(async entry => {
      if (entry.status === "deleted") await entry.waitForDraft();
      if (!(entry.status === "deleted" && entry.draftOnDisk)) await entry.flush();
    }));
  }, [waitForInsertions]);

  useEffect(() => {
    mounted.current = true;
    void refresh().catch(report);
    const onFocus = () => {
      if (!document.hidden && !busyRef.current) void refresh().catch(report);
    };
    const interval = setInterval(onFocus, 15000);
    window.addEventListener("focus", onFocus);
    const unsubscribe = subscribeBeforeLibraryChange(async () => {
      if (busyRef.current) throw new Error("当前笔记操作完成后可切换资料库。");
      await saveAll();
    });
    const beforeUnload = (e: BeforeUnloadEvent) => {
      if (inserts.current.size || [...sessions.current.values()].some(entry => entry.status !== "saved")) {
        void saveAll().catch(() => {});
        e.preventDefault();
        e.returnValue = "";
      }
    };
    window.addEventListener("beforeunload", beforeUnload);
    return () => {
      mounted.current = false;
      searchRequest.current?.abort();
      ++loadGeneration.current;
      clearInterval(interval);
      unsubscribe();
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("beforeunload", beforeUnload);
      for (const entry of sessions.current.values()) entry.dispose();
      sessions.current.clear();
    };
  }, [refresh, saveAll]);

  const run = async (label: string, action: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    ++loadGeneration.current;
    setBusy(label);
    setError("");
    try {
      await action();
    } catch (e) {
      report(e);
    } finally {
      busyRef.current = false;
      if (mounted.current) setBusy("");
    }
  };
  const open = async (id: string) => {
    const current = sessions.current.get(id);
    const item = current?.note ?? await client.getNote(id, options).catch(async cause => {
      if (cause instanceof CoreApiError && cause.status === 404 && JSON.stringify(cause.body).includes("note_not_found") && mounted.current) {
        await refresh();
        throw new Error("笔记已删除。");
      }
      throw cause;
    });
    if (!mounted.current) return;
    show(item);
    setFolderId(tree.noteParents[id] ?? null);
  };
  const choose = (id: string) => {
    if (id === layout.current?.activeId && sessions.current.has(id)) return;
    void run("正在打开…", async () => {
      await waitForInsertions();
      await open(id);
    });
  };
  const closeTab = (id: string) => void run("正在关闭…", async () => {
    await waitForInsertions();
    const entry = sessions.current.get(id);
    if (entry?.status === "deleted") await entry.waitForDraft();
    if (entry && !(entry.status === "deleted" && entry.draftOnDisk)) {
      try { await entry.flush(); }
      catch (cause) {
        if (mounted.current) show(entry.note);
        throw cause;
      }
    }
    if (!mounted.current || !layout.current) return;
    const wasActive = layout.current.activeId === id;
    const next = retainTabs(layout.current, key => key !== id);
    setLayout(next);
    if (wasActive && next.activeId) await open(next.activeId);
    if (entry?.status === "deleted") await refresh();
  });
  const create = (format: "markdown" | "html") =>
    void run("正在新建…", async () => {
      await waitForInsertions();
      const item = await client.createNote({ title: "无标题", markdown: "", styles: {}, format, folderId }, options);
      if (!mounted.current) return;
      changeSearch("");
      show(item);
      await refresh();
      requestAnimationFrame(() => {
        titleInput.current?.focus();
        titleInput.current?.select();
      });
    });
  const importFiles = (list: FileList | null) => {
    if (!list?.length) return;
    const files = Array.from(list);
    void run("正在导入…", async () => {
      await waitForInsertions();
      let last: NotebookNote | undefined;
      try {
        const result = await importNoteFiles(options, files, name => {
          if (mounted.current) setBusy("正在导入 " + name);
        }, folderId, item => { last = item; });
        if (mounted.current && result.missing.length)
          setError("已导入正文。未收集的外部资源：" + result.missing.map(name => name.slice(0, 120)).join("、"));
      } finally {
        if (mounted.current) {
          if (last) { changeSearch(""); show(last); }
          await refresh();
        }
      }
    });
  };
  const remove = () => void run("正在删除…", async () => {
    if (!active) return;
    await waitForInsertions();
    await active.flush();
    await client.deleteNote(active.note.id, active.note.revision, options);
    if (mounted.current) await refresh();
  });

  useEffect(() => {
    const handle = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey || event.defaultPrevented || event.isComposing) return;
      const target = event.target;
      if (target instanceof Element && target !== document.body) {
        if (target.closest('[role="dialog"],[role="menu"]')) return;
        if (!workspace.current?.contains(target) && !titlebarTarget?.contains(target) &&
          !(sidebarOpen && sidebarTarget?.contains(target))) return;
      }
      const key = event.key.toLowerCase();
      const current = layout.current;
      if (key === "w" && !event.shiftKey && current?.activeId) {
        event.preventDefault();
        closeTab(current.activeId);
      } else if (key === "tab" && current && current.ids.length > 1) {
        event.preventDefault();
        const index = current.ids.indexOf(current.activeId!);
        choose(current.ids[(index + current.ids.length + (event.shiftKey ? -1 : 1)) % current.ids.length]);
      } else if (key === "s" && !event.shiftKey && active) {
        event.preventDefault();
        if (!busyRef.current) void waitForInsertions().then(() => active.flush()).catch(report);
      } else if (key === "k" && !event.shiftKey) {
        event.preventDefault();
        onSidebarOpenChange(true);
        requestAnimationFrame(() => searchInput.current?.focus());
      }
    };
    window.addEventListener("keydown", handle);
    return () => window.removeEventListener("keydown", handle);
  });
  const original = note?.format === "document" ? note.attachments[0] : undefined;
  const resolveAttachment = useCallback(
    (id: string) => client.getNoteAttachment(selectedId!, id, options),
    [client, selectedId, options],
  );
  const addAttachment = useCallback(
    async (file: File) => {
      const resource = await client.uploadNoteAttachment(selectedId!, file, options);
      const current = selectedId && sessions.current.get(selectedId);
      if (!current || layout.current?.activeId !== selectedId) throw new Error("笔记已切换。");
      if (!current.note.attachments.some((existing) => existing.id === resource.id)) {
        current.note = { ...current.note, attachments: [...current.note.attachments, resource] };
        redraw((i) => i + 1);
      }
      return resource.id;
    },
    [client, selectedId, options],
  );
  const renderAttachment = useCallback(
    (id: string, name: string) => (
      <AttachmentView
        key={id}
        id={id}
        name={sessions.current.get(selectedId!)?.note.attachments.find((file) => file.id === id)?.name || name}
        noteId={selectedId!}
        options={options}
      />
    ),
    [selectedId, options],
  );
  const changeSearch = useCallback((value: string) => {
    searchRequest.current?.abort();
    setSearching(false);
    setBodyMatches(null);
    setSearch(value);
  }, []);
  const lastLocate = useRef(locateRequest);
  useEffect(() => {
    if (lastLocate.current === locateRequest) return;
    lastLocate.current = locateRequest;
    changeSearch("");
    const parent = selectedId ? tree.noteParents[selectedId] ?? null : folderId;
    setReveal(previous => ({ folderId: parent, noteId: selectedId, stamp: (previous?.stamp ?? 0) + 1 }));
  }, [locateRequest, selectedId, tree.noteParents, folderId, changeSearch]);
  const searchBody = async () => {
    searchRequest.current?.abort();
    if (!search.trim()) {
      setBodyMatches(null);
      return;
    }
    const request = new AbortController();
    searchRequest.current = request;
    setSearching(true);
    try {
      const matches = await client.listNotes({
        ...options,
        query: search.trim(),
        signal: request.signal,
      });
      if (!request.signal.aborted) setBodyMatches(new Set(matches.map((item) => item.id)));
    } catch (cause) {
      if (!request.signal.aborted) report(cause);
    } finally {
      if (!request.signal.aborted) setSearching(false);
    }
  };
  const filtered = useMemo(
    () =>
      notes
        .filter((item) =>
          search.trim() && bodyMatches
            ? bodyMatches.has(item.id)
            : item.title.toLowerCase().includes(search.trim().toLowerCase()),
        )
        .sort((a, b) => b.updatedAt - a.updatedAt || a.title.localeCompare(b.title)),
    [notes, search, bodyMatches],
  );
  const summaries = useMemo(() => new Map(notes.map(row => [row.id, row])), [notes]);
  const openTabs = (layout.current?.ids ?? []).map(id => {
    const entry = sessions.current.get(id);
    const item = entry?.note ?? summaries.get(id);
    return { id, title: item?.title || "无标题", format: item?.format ?? "markdown", status: entry?.status };
  });
  const breadcrumb = useMemo(() => {
    const folders = new Map(tree.folders.map(folder => [folder.id, folder]));
    const path: { name: string; id: string | null }[] = [];
    const visited = new Set<string>();
    let id: string | null = selectedId ? tree.noteParents[selectedId] ?? null : folderId;
    while (id && !visited.has(id)) {
      const folder = folders.get(id);
      if (!folder) break;
      visited.add(id);
      path.unshift({ name: folder.name, id: folder.id });
      id = folder.parentId;
    }
    return [{ name: "笔记库", id: null }, ...path];
  }, [tree, folderId, selectedId]);
  const tabStrip = (
    <NotebookTabStrip
      tabs={openTabs}
      activeId={selectedId}
      disabled={!!busy || !loaded}
      select={choose}
      reorder={(id, target, side) => {
        const current = layout.current;
        if (!current || busyRef.current) return;
        const next = reorderTab(current, id, target, side);
        if (next !== current) setLayout(next);
      }}
      close={closeTab}
      create={() => create("markdown")}
    />
  );
  return (
    <div className="notebook-workspace" ref={workspace}>
      {sidebarTarget && createPortal(
        <aside className="notebook-index" aria-label="笔记列表">
          <header>
            <h1>笔记</h1>
            <span aria-label={notes.length + " 篇笔记"}>{notes.length}</span>
            <div className="notebook-create" role="group" aria-label="新建笔记">
              <button type="button" disabled={!!busy || !loaded} aria-label="新建 Markdown 笔记" title="新建 Markdown 笔记" onClick={() => create("markdown")}><icons.Pencil size={16} /></button>
              <button type="button" disabled={!!busy || !loaded} aria-label="新建 HTML 笔记" title="新建 HTML 笔记" onClick={() => create("html")}><icons.FileCode2 size={16} /></button>
            </div>
          </header>
          <form
            className="notebook-search"
            role="search"
            title="输入时筛选标题，回车搜索正文"
            onSubmit={(event) => {
              event.preventDefault();
              void searchBody();
            }}
          >
            <button type="submit" aria-label="搜索标题和正文" disabled={searching}>
              <icons.Search size={15} />
            </button>
            <input
              aria-label="搜索笔记，回车搜索正文"
              placeholder="搜索笔记"
              ref={searchInput}
              value={search}
              onChange={(e) => changeSearch(e.target.value)}
            />
            {search && (
              <button type="button" aria-label="清空搜索" onClick={() => changeSearch("")}>
                <icons.X size={13} />
              </button>
            )}
            {!search && <kbd>Ctrl K</kbd>}
          </form>
          {searching && <span role="status">正在搜索正文…</span>}
          <NotebookIndex
            rows={filtered}
            selected={selectedId}
            busy={!!busy}
            choose={choose}
            tree={tree}
            folderId={folderId}
            selectFolder={setFolderId}
            reveal={reveal}
            options={options}
            refresh={refresh}
            search={search}
            clearSearch={() => changeSearch("")}
            emptyMessage={loaded && search && !filtered.length ? "没有匹配的笔记" : undefined}
          />
          <footer>
            <span>导入</span>
            <button
              onClick={() => chooser.current?.click()}
              disabled={!!busy || !loaded}
              aria-label="导入文件"
              title="导入文件，可同时选择正文和引用的图片"
            >
              <icons.Download size={16} />
            </button>
            <button
              disabled={!!busy || !loaded}
              aria-label="导入文件夹"
              onClick={() => folderChooser.current?.click()}
              title="导入文件夹，保留笔记的目录层次"
            >
              <icons.FolderOpen size={16} />
            </button>
          </footer>
        </aside>, sidebarTarget
      )}
      <main className="notebook-document" aria-busy={!!busy}>
        {titlebarTarget === undefined ? tabStrip : titlebarTarget && createPortal(tabStrip, titlebarTarget)}
        <div className="notebook-statusbar">
          <nav aria-label="笔记路径" className="notebook-breadcrumb">
            <icons.FolderOpen size={14} aria-hidden="true" />
            {breadcrumb.map((part, index) => <span key={part.id ?? "root"}>{index > 0 && <icons.ChevronRight size={11} aria-hidden="true" />}
              <button type="button" disabled={!!busy} title={"在目录中定位：" + part.name} onClick={() => {
                changeSearch(""); onSidebarOpenChange(true);
                setReveal(previous => ({ folderId: part.id, stamp: (previous?.stamp ?? 0) + 1 }));
              }}><span data-button-content>{part.name}</span></button>
            </span>)}
          </nav>
          <div className="notebook-save-state" data-status={active?.status} aria-live="polite">
            {busy ? <icons.Loader2 size={13} /> : active?.status === "saved" ? <icons.Check size={13} /> : null}
            {busy || (active ? statusLabel[active.status] : "")}
          </div>
          {active && (
            <button
              className="nb-icon"
              title="导出正文和附件"
              aria-label="导出笔记"
              disabled={!!busy}
              onClick={() =>
                void run("正在导出…", async () => {
                  await Promise.all([...inserts.current]);
                  await exportNote(active.note, options);
                })
              }
            >
              <icons.Download size={15} />
            </button>
          )}
          {original && (
            <button className="notebook-mode-button" disabled={!!busy} onClick={() => {
              if (tabView) void run("正在切换…", async () => {
                await waitForInsertions();
                tabView.annotating = !annotating;
                redraw(i => i + 1);
              });
            }}>
              <span data-button-content>{annotating ? "查看原件" : "编辑批注"}</span>
            </button>
          )}
          {active && (
            <button
              className="nb-icon"
              aria-label="删除当前笔记"
              title="删除笔记"
              disabled={!!busy}
              onClick={() => setConfirmDelete(true)}
            >
              <icons.Trash2 size={15} />
            </button>
          )}
        </div>
        {error && (
          <div className="notebook-message" role="alert">
            {error}
            <button
              onClick={() => {
                setError("");
                void refresh().catch(report);
              }}
            >
              <span data-button-content>刷新</span>
            </button>
          </div>
        )}
        {orphans
          .filter((item) => !sessions.current.has(item.id))
          .map((draft) => (
            <div className="notebook-message" key={draft.key}>
              “{draft.title}”的原笔记已删除，可恢复本机草稿。附件需重新添加。
              <button
                disabled={!!busy}
                onClick={() =>
                  void run("正在恢复…", async () => {
                    await waitForInsertions();
                    const snapshot = await loadDraft(libraryId, draft);
                    const copy = await client.createNote(
                      {
                        ...snapshot.fields,
                        folderId,
                        format: snapshot.fields.format === "document" ? "markdown" : snapshot.fields.format,
                      },
                      options,
                    );
                    await removeDraft(libraryId, draft.id, draft.key);
                    if (mounted.current) {
                      show(copy);
                      await refresh();
                    }
                  })
                }
              >
                <span data-button-content>恢复为新笔记</span>
              </button>
              <button
                disabled={!!busy}
                onClick={() => void run("正在处理…", async () => {
                  await removeDraft(libraryId, draft.id, draft.key);
                  setOrphans((rows) => rows.filter((item) => item.key !== draft.key));
                })}
              >
                <span data-button-content>放弃草稿</span>
              </button>
            </div>
          ))}
        {active?.otherDrafts.map((draft) => (
          <div className="notebook-message" key={draft.key}>
            <span>
              这篇笔记还有一份本机草稿
              {draft.updatedAt ? " · " + new Date(draft.updatedAt).toLocaleString() : ""}。
              {draft.preview || draft.title}
            </span>
            <button
              disabled={!!busy || active.status !== "saved"}
              onClick={() => void run("正在恢复…", async () => active.restoreOtherDraft(draft))}
            >
              <span data-button-content>恢复此版本</span>
            </button>
            <button
              disabled={!!busy}
              onClick={() => void run("正在处理…", async () => active.dismissOtherDraft(draft))}
            >
              <span data-button-content>放弃此版本</span>
            </button>
          </div>
        ))}
        {active?.status === "deleted" && (
          <div className="notebook-message">
            <button
              disabled={!!busy}
              onClick={() =>
                void run("正在恢复…", async () => {
                  const { title, markdown, styles, format } = active.note;
                  const copy = await client.createNote(
                    { title, markdown, styles, folderId, format: format === "document" ? "markdown" : format },
                    options,
                  );
                  await active.clearDraft();
                  if (mounted.current) {
                    setLayout(retainTabs(layout.current!, id => id !== active.note.id));
                    show(copy);
                    await refresh();
                  }
                })
              }
            >
              <span data-button-content>恢复为新笔记</span>
            </button>
            <button
              disabled={!!busy}
              onClick={() => void run("正在处理…", async () => {
                await active.clearDraft();
                if (!mounted.current) return;
                setLayout(retainTabs(layout.current!, id => id !== active.note.id));
                await refresh();
              })}
            >
              <span data-button-content>放弃草稿</span>
            </button>
          </div>
        )}
        {active?.error && (
          <div className="notebook-message" role="status">
            {active.error}
            {active.status === "failed" && (
              <button onClick={() => void active.flush().catch(report)}><span data-button-content>重试保存</span></button>
            )}
          </div>
        )}
        {active?.draftError && <div className="notebook-message" role="alert">{active.draftError}</div>}
        {active?.status === "conflict" && (
          <div className="notebook-message" role="alert">
            另一个版本已保存。选择要保留的正文。
            <button disabled={!!busy} onClick={() => void run("正在保存…", () => active.resolve(true))}>
              <span data-button-content>保留本机草稿</span>
            </button>
            <button disabled={!!busy} onClick={() => void run("正在打开…", () => active.resolve(false))}>
              <span data-button-content>使用库中版本</span>
            </button>
          </div>
        )}
        {confirmDelete && (
          <div className="notebook-message" role="alert">
            删除“{note?.title}”及其附件？此操作将随库同步。
            <button onClick={() => setConfirmDelete(false)}><span data-button-content>取消</span></button>
            <button disabled={!!busy} onClick={remove}>
              <span data-button-content>确认删除</span>
            </button>
          </div>
        )}
        {note && active ? (
          <div
            className="notebook-writing"
            role="tabpanel"
            id="nb-active-document"
            aria-labelledby={"nb-tab-" + note.id}
            ref={(node) => {
              if (node) node.inert = Boolean(busy);
            }}
          >
            <input
              ref={titleInput}
              className="notebook-title"
              aria-label="笔记标题"
              maxLength={240}
              placeholder="无标题"
              value={note.title}
              onChange={(e) => active.edit({ title: e.target.value })}
            />
            {original && !annotating ? (
              <div className="notebook-original">
                <AttachmentView
                  key={note.id}
                  noteId={note.id}
                  options={options}
                  id={original.id}
                  name={original.name}
                  view={tabView}
                />
              </div>
            ) : (
              <NoteEditor
                key={note.id}
                tabView={tabView}
                onPendingInsert={trackInsert}
                format={note.format === "html" ? "html" : "markdown"}
                value={note.markdown}
                onChange={(markdown) => active.edit({ markdown })}
                styles={note.styles}
                onStylesChange={(styles) => active.edit({ styles })}
                addAttachment={addAttachment}
                resolveAttachment={resolveAttachment}
                renderAttachment={renderAttachment}
              />
            )}
          </div>
        ) : (
          <div className="notebook-empty">
            <icons.Notepad size={38} />
            <h2>{loaded ? "打开或新建笔记" : "正在打开笔记库…"}</h2>
            {loaded && (
              <div className="notebook-empty-actions">
              <LiquidGlassButton disabled={!!busy} onClick={() => create("markdown")}>
                <icons.Plus size={15} />
                新建 Markdown 笔记
              </LiquidGlassButton>
              {notes.length > 0 && <button onClick={() => {
                onSidebarOpenChange(true); requestAnimationFrame(() => searchInput.current?.focus());
              }}><icons.Search size={15} /><span data-button-content>查找笔记</span></button>}
              </div>
            )}
          </div>
        )}
      </main>
      <input
        hidden
        type="file"
        ref={chooser}
        multiple
        accept={importFormats}
        onChange={(e) => {
          importFiles(e.target.files);
          e.target.value = "";
        }}
      />
      <input
        hidden
        type="file"
        ref={folderChooser}
        multiple
        {...{ webkitdirectory: "" }}
        onChange={(e) => {
          importFiles(e.target.files);
          e.target.value = "";
        }}
      />
    </div>
  );
}
