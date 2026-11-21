import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent, type KeyboardEvent } from "react";
import { defaultRangeExtractor, useVirtualizer, type Range } from "@tanstack/react-virtual";
import type { NotebookTree, NoteRequestOptions, NoteSummary } from "@mangoo/core-client";
import { CoreApiError, createCoreClient, icons, type AppIcon } from "@mangoo/web/plugin-host";
import { canMoveItem, flattenNotebook, indexNotebook, moveDestinations, type TreeItem } from "./notebookTree";
import "./notebookTree.css";

interface TreeProps {
  rows: NoteSummary[];
  selected?: string;
  busy: boolean;
  emptyMessage?: string;
  choose: (id: string) => void;
  tree: NotebookTree;
  folderId: string | null;
  selectFolder: (id: string | null) => void;
  reveal?: { folderId: string | null; noteId?: string; stamp: number };
  options: NoteRequestOptions;
  refresh: () => Promise<void>;
  search: string;
  clearSearch?: () => void;
}

type Action =
  | { type: "create"; parentId: string | null }
  | { type: "rename" | "move" | "delete"; item: TreeItem };

const labels = { create: "新建文件夹", rename: "重命名文件夹", move: "移动到文件夹", delete: "删除空文件夹" };
const confirmLabels = { create: "确认新建文件夹", rename: "确认重命名", move: "确认移动", delete: "确认删除空文件夹" };
const treeDragType = "application/x-mangoo-notebook-tree-item";

function treeError(cause: unknown) {
  if (!(cause instanceof CoreApiError)) return cause instanceof Error ? cause.message : String(cause);
  const body = cause.body as { error?: unknown; code?: string } | null;
  if (typeof body?.error === "string" && /\p{Script=Han}/u.test(body.error)) return body.error;
  if (body?.code === "stale_library_context") return "当前笔记库已切换，请在新目录中重试。";
  if (cause.status === 404) return "笔记或目标文件夹已删除，请重新选择。";
  if (cause.status === 409) return "目录已更改，请核对最新位置后重试。";
  if (cause.status === 403) return "笔记库插件已停用或缺少资料库权限。";
  if (cause.status === 400) return "目录操作未通过校验，请检查文件夹名称和目标位置。";
  return "目录操作失败（HTTP " + cause.status + "），请稍后重试。";
}

function IconAction({ label, icon: Icon, onClick, disabled }: { label: string; icon: AppIcon; onClick: () => void; disabled?: boolean }) {
  return <button type="button" className="nb-tree-icon" title={label} aria-label={label} disabled={disabled} onClick={onClick}><Icon size={15} /></button>;
}

function noteIcon(note: NoteSummary) {
  if (note.format === "html") return icons.FileCode2;
  if (note.format === "markdown") return icons.FileText;
  if (/\.(?:png|jpe?g|gif|webp|avif|bmp|svg)$/i.test(note.title)) return icons.FileImage;
  return icons.File;
}

function readExpanded(key: string) {
  try {
    const values: unknown = JSON.parse(localStorage.getItem(key) ?? "[]");
    return new Set<string>(Array.isArray(values) ? values.filter((id) => typeof id === "string") : []);
  } catch { return new Set<string>(); }
}

export function NotebookIndex({
  rows, tree, selected, busy, choose, emptyMessage, folderId, selectFolder, reveal, options, refresh, search, clearSearch,
}: TreeProps) {
  const client = useMemo(() => createCoreClient(), []);
  const nav = useRef<HTMLElement>(null);
  const rootButton = useRef<HTMLButtonElement>(null);
  const submitting = useRef(false);
  const [working, setWorking] = useState(false);
  const [message, setMessage] = useState("");
  const [action, setAction] = useState<Action>();
  const [value, setValue] = useState("");
  const key = "mangoo.notebook.expanded:" + options.libraryId;
  const [expanded, setExpanded] = useState(() => readExpanded(key));
  const [targetId, setTargetId] = useState<string | null>(folderId ?? selected ?? null);
  const [focusId, setFocusId] = useState<string>();
  const dragged = useRef<TreeItem>();
  const [dragging, setDragging] = useState(false);
  const [dropTarget, setDropTarget] = useState<string | null>();
  const index = useMemo(() => indexNotebook(tree, rows, search), [tree, rows, search]);
  const visible = useMemo(() => flattenNotebook(index, expanded), [index, expanded]);
  const target = targetId ? index.items.get(targetId) : undefined;
  const destinations = useMemo(() => action?.type === "move" ? moveDestinations(tree.folders, action.item.folder?.id) : [],
    [tree.folders, action]);
  const focusIndex = useMemo(() => Math.max(0, visible.findIndex((row) => row.item.id === (focusId ?? targetId))),
    [visible, focusId, targetId]);
  const includeKeyboardFocus = useCallback((range: Range) => {
    const indexes = defaultRangeExtractor(range);
    if (!indexes.length || indexes.includes(focusIndex)) return indexes;
    return [...indexes, focusIndex].sort((a, b) => a - b);
  }, [focusIndex]);
  const list = useVirtualizer({
    count: visible.length, estimateSize: () => 33, overscan: 7,
    rangeExtractor: includeKeyboardFocus,
    getScrollElement: () => nav.current, getItemKey: (i) => visible[i].item.id,
  });
  const locked = busy || working;

  useEffect(() => {
    try { localStorage.setItem(key, JSON.stringify([...expanded])); } catch { /* Optional view preference. */ }
  }, [expanded, key]);
  const parents = useMemo(() => new Map(tree.folders.map((folder) => [folder.id, folder.parentId])), [tree.folders]);
  const expandPath = useCallback((id: string | null) => {
    if (!id) return;
    setExpanded((old) => {
      const next = new Set(old);
      let ancestor: string | null = id;
      while (ancestor) {
        next.add(ancestor);
        ancestor = parents.get(ancestor) ?? null;
      }
      return next.size === old.size ? old : next;
    });
  }, [parents]);
  const lastOpened = useRef<{ id?: string; parent?: string }>();
  const scrollTarget = useRef<string>();
  const handledReveal = useRef<number>();
  // Tab switches reveal the opened note. Polling unchanged data leaves the
  // user's directory selection, disclosure state and scroll position alone.
  useEffect(() => {
    const parent = selected ? tree.noteParents[selected] : undefined;
    const previous = lastOpened.current;
    if (previous && previous.id === selected && previous.parent === parent) return;
    lastOpened.current = { id: selected, parent };
    if (!selected || (selected === previous?.id && selected !== targetId)) return;
    setTargetId(selected);
    setFocusId(selected);
    selectFolder(parent ?? null);
    expandPath(parent ?? null);
    scrollTarget.current = selected;
  }, [selected, tree.noteParents, selectFolder, expandPath, targetId]);
  useEffect(() => {
    if (!reveal || handledReveal.current === reveal.stamp) return;
    handledReveal.current = reveal.stamp;
    const id = reveal.noteId ?? reveal.folderId;
    selectFolder(reveal.folderId);
    setTargetId(id); setFocusId(id ?? undefined);
    setAction(undefined); setMessage("");
    expandPath(reveal.folderId);
    scrollTarget.current = id ?? undefined;
    if (!id) list.scrollToOffset(0);
  }, [reveal, expandPath, list, selectFolder]);
  useEffect(() => {
    if (!scrollTarget.current) return;
    const position = visible.findIndex((row) => row.item.id === scrollTarget.current);
    if (position >= 0) {
      list.scrollToIndex(position, { align: "auto" });
      scrollTarget.current = undefined;
    }
  }, [visible, list, selected, reveal]);

  function toggle(id: string, open: boolean) {
    setExpanded((old) => {
      const next = new Set(old);
      if (open) next.add(id); else next.delete(id);
      return next;
    });
  }
  function root() {
    setTargetId(null);
    setFocusId(undefined);
    selectFolder(null);
    setAction(undefined);
    setMessage("");
    list.scrollToOffset(0);
  }
  function select(item: TreeItem) {
    setAction(undefined);
    setMessage("");
    setTargetId(item.id);
    setFocusId(item.id);
    selectFolder(item.folder ? item.id : item.parentId);
    if (item.note) choose(item.id);
    else if (!index.matches) toggle(item.id, !expanded.has(item.id));
  }
  function begin(next: Action) {
    setAction(next);
    setMessage("");
    setValue(next.type === "rename" ? next.item.name : next.type === "move" ? next.item.parentId ?? "" : "");
  }
  function focusRow(position: number) {
    const next = Math.max(0, Math.min(position, visible.length - 1));
    if (!visible[next]) return;
    setFocusId(visible[next].item.id);
    list.scrollToIndex(next, { align: "auto" });
    requestAnimationFrame(() => nav.current?.querySelector<HTMLButtonElement>(`[data-tree-row="${next}"]`)?.focus());
  }
  function keydown(event: KeyboardEvent<HTMLButtonElement>, i: number) {
    const item = visible[i].item;
    switch (event.key) {
      case "ArrowUp": focusRow(i - 1); break;
      case "ArrowDown": focusRow(i + 1); break;
      case "Home": focusRow(0); break;
      case "End": focusRow(visible.length - 1); break;
      case "ArrowRight":
        if (item.folder) {
          if (!index.matches && !expanded.has(item.id)) toggle(item.id, true);
          else if (visible[i + 1]?.item.parentId === item.id) focusRow(i + 1);
        }
        break;
      case "ArrowLeft":
        if (item.folder && !index.matches && expanded.has(item.id)) toggle(item.id, false);
        else if (item.parentId) focusRow(visible.findIndex((row) => row.item.id === item.parentId));
        else rootButton.current?.focus();
        break;
      case "F2":
        if (item.folder) begin({ type: "rename", item });
        break;
      default: return;
    }
    event.preventDefault();
  }

  function finishDrag() {
    dragged.current = undefined;
    setDragging(false);
    setDropTarget(undefined);
  }
  function allowDrop(event: DragEvent, parent: string | null) {
    const item = dragged.current;
    if (locked || !item || !event.dataTransfer.types.includes(treeDragType)) return false;
    if (!canMoveItem(item, parent, parents)) {
      event.dataTransfer.dropEffect = "none";
      setDropTarget(undefined);
      return false;
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    setDropTarget(parent);
    return true;
  }
  function drop(event: DragEvent, parent: string | null) {
    const item = dragged.current;
    if (item && event.dataTransfer.getData(treeDragType) === item.id && allowDrop(event, parent)) {
      void submit({ type: "move", item }, parent ?? "");
    }
    finishDrag();
  }
  async function submit(operation = action, input = value) {
    if (!operation || locked || submitting.current) return;
    if (operation.type === "move" && !canMoveItem(operation.item, input || null, parents)) return;
    submitting.current = true;
    setWorking(true);
    setMessage("");
    try {
      let destination: string | null;
      let selectedItem: string | null;
      if (operation.type === "create") {
        const folder = await client.createNotebookFolder({ name: input.trim(), parentId: operation.parentId }, options);
        destination = folder.id;
        selectedItem = folder.id;
        expandPath(operation.parentId);
      } else {
        const { item } = operation;
        destination = item.parentId;
        selectedItem = item.id;
        if (operation.type === "delete" && item.folder) {
          await client.deleteNotebookFolder(item.id, item.folder.revision, options);
          selectedItem = item.parentId;
        } else if (item.folder) {
          destination = operation.type === "move" ? input || null : item.parentId;
          await client.updateNotebookFolder(item.id, {
            name: operation.type === "rename" ? input.trim() : item.name,
            parentId: destination, expectedRevision: item.folder.revision,
          }, options);
          expandPath(destination);
          destination = item.id;
        } else {
          destination = input || null;
          await client.moveNotebookNote(item.id, { parentId: destination, expectedParentId: item.parentId }, options);
        }
      }
      setAction(undefined);
      clearSearch?.();
      selectFolder(destination);
      setTargetId(selectedItem);
      setFocusId(selectedItem ?? undefined);
      expandPath(destination);
      scrollTarget.current = selectedItem ?? undefined;
      await refresh();
    } catch (cause) {
      setMessage(treeError(cause));
      // Re-enable decisions against the latest revision after a remote move.
      if (!(cause instanceof CoreApiError && (cause.body as { code?: string } | null)?.code === "notebook_duplicate_folder_name")) {
        setAction(undefined);
      }
      try { await refresh(); } catch { /* Keep the original error visible. */ }
    } finally {
      submitting.current = false;
      setWorking(false);
    }
  }

  return (
    <div className="nb-tree">
      <div className="nb-tree-tools">
        <button ref={rootButton} className="nb-tree-root" onClick={root} aria-label="选择根目录" title="选择根目录" disabled={locked}
          data-drop-target={dropTarget === null || undefined}
          onDragOver={event => allowDrop(event, null)} onDragLeave={() => setDropTarget(undefined)} onDrop={event => drop(event, null)}>
          <span data-button-content>{dragging ? "移至根目录" : "文件目录"}</span></button>
        <span className="nb-tree-actions">
          <IconAction icon={icons.FolderPlus} label={labels.create} disabled={locked}
            onClick={() => begin({ type: "create", parentId: folderId })} />
          {target?.folder && <IconAction icon={icons.Pencil} label={labels.rename} disabled={locked} onClick={() => begin({ type: "rename", item: target })} />}
          {target && <IconAction icon={icons.FolderInput} label={labels.move} disabled={locked} onClick={() => begin({ type: "move", item: target })} />}
          {target?.folder && <IconAction icon={icons.Trash2} label={labels.delete} disabled={locked} onClick={() => begin({ type: "delete", item: target })} />}
        </span>
      </div>
      {action && <form className="nb-tree-form" aria-label={labels[action.type]} onSubmit={(event) => { event.preventDefault(); void submit(); }}
        onKeyDown={(event) => { if (!locked && event.key === "Escape") { event.preventDefault(); setAction(undefined); rootButton.current?.focus(); } }}>
        {action.type === "move" ? <label>移动「{action.item.name}」到
          <select autoFocus aria-label="目标文件夹" value={value} onChange={(event) => setValue(event.target.value)}>
            <option value="" disabled={action.item.parentId === null}>根目录</option>
            {destinations.map(({ id, label }) => <option key={id} value={id} disabled={action.item.parentId === id}>{label}</option>)}
          </select></label> : action.type === "delete" ? <span>删除空文件夹「{action.item.name}」？</span> :
          <label>{action.type === "create" ? "新建文件夹" : "重命名文件夹"}
            <input autoFocus required aria-label="文件夹名称" placeholder="文件夹名称" maxLength={200} value={value}
              onFocus={(event) => event.target.select()} onChange={(event) => setValue(event.target.value)} /></label>}
        <div className="nb-tree-form-actions">
          <button type="button" disabled={locked} onClick={() => setAction(undefined)}><span data-button-content>取消</span></button>
          <button type="submit" aria-label={confirmLabels[action.type]} disabled={locked ||
            (action.type === "move" && !canMoveItem(action.item, value || null, parents)) ||
            ((action.type === "create" || action.type === "rename") && !value.trim())}>
            <span data-button-content>{action.type === "delete" ? "删除" : "确定"}</span></button>
        </div>
      </form>}
      {message && <div className="nb-tree-message" role="alert">{message}</div>}
      {target?.folder && expanded.has(target.id) && !index.matches && !index.children.has(target.id) &&
        <p className="nb-tree-empty" role="status">「{target.name}」为空，可在此新建或导入笔记。</p>}
      <nav ref={nav} className="nb-tree-scroll" aria-label="当前笔记库">
        <div role="tree" aria-label="笔记文件目录" style={{ position: "relative", height: list.getTotalSize() }}>
          {list.getVirtualItems().map((virtual) => {
            const { item, depth, position, siblings } = visible[virtual.index];
            const isOpen = !!index.matches || expanded.has(item.id);
            const Icon = item.note ? noteIcon(item.note) : isOpen ? icons.FolderOpen : icons.Folder;
            const childCount = index.children.get(item.id)?.length ?? 0;
            const format = item.note?.format === "markdown" ? "MD" : item.note?.format === "html" ? "HTML"
              : item.note?.title.match(/\.([\w]{1,8})$/)?.[1].toUpperCase();
            return <button key={item.id} type="button" role="treeitem" className={"nb-tree-row " + (item.note ? "notebook-item" : "nb-tree-folder")}
              data-note-id={item.note?.id} data-folder-id={item.folder?.id} data-tree-row={virtual.index} data-note-row={item.note ? virtual.index : undefined}
              draggable={!locked} data-drop-target={dropTarget === item.id || undefined}
              onDragStart={event => {
                if (locked) { event.preventDefault(); return; }
                event.dataTransfer.setData(treeDragType, item.id);
                event.dataTransfer.effectAllowed = "move";
                dragged.current = item; setDragging(true); setAction(undefined);
              }}
              onDragOver={item.folder ? event => allowDrop(event, item.id) : undefined}
              onDragLeave={() => setDropTarget(undefined)}
              onDrop={item.folder ? event => drop(event, item.id) : undefined} onDragEnd={finishDrag}
              aria-level={depth} aria-posinset={position} aria-setsize={siblings} aria-selected={item.id === targetId} aria-label={item.name}
              aria-expanded={item.folder ? isOpen : undefined} aria-current={item.id === selected ? "page" : undefined}
              title={item.name} tabIndex={virtual.index === focusIndex ? 0 : -1} disabled={locked}
              style={{ height: virtual.size, transform: `translateY(${virtual.start}px)`, paddingInlineStart: 3 + Math.min(depth - 1, 8) * 12 }}
              onFocus={() => setFocusId(item.id)} onKeyDown={(event) => keydown(event, virtual.index)} onClick={() => select(item)}>
              <span data-button-content className="nb-tree-disclosure">{item.folder && (isOpen ? <icons.ChevronDown size={11} /> : <icons.ChevronRight size={11} />)}</span>
              <Icon size={16} /><strong data-button-content>{item.name}</strong><small data-button-content>{item.folder ? childCount || "" : format}</small>
            </button>;
          })}
        </div>
        {!visible.length && <p className="nb-tree-empty">{emptyMessage || (search.trim() ? "没有匹配的笔记或文件夹" : "还没有笔记或文件夹")}</p>}
      </nav>
    </div>
  );
}
