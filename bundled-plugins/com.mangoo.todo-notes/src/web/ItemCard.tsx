import { useMemo, useState, type DragEvent, type ReactNode } from "react";
import { icons } from "@mangoo/web/plugin-host";

import {
  MAX_SUBTASK_DEPTH,
  PRIORITY_LABELS,
  RECURRENCE_LABELS,
  childrenOf,
  depthOf,
  dueState,
  pad,
  progressOf,
  stripHtml,
  titleText,
  type TodoItem,
  type TodoList
} from "../shared/model.ts";
import { Checkbox, ColorDot, PriorityFlag } from "./components.tsx";
import { compressImage } from "./images.ts";
import { InlineRichEditor, TitleView, cleanTitle } from "./InlineRichEditor.tsx";
import { PROPERTY_LABELS, PropertyPopover, dateLabel, propertySet, type PopoverState, type Property } from "./PropertyPopover.tsx";
import { NotesView, RichTextEditor } from "./RichTextEditor.tsx";
import type { TodoStore } from "./store.ts";

function formatStamp(ms: number): string {
  const date = new Date(ms);
  return `${date.getMonth() + 1}月${date.getDate()}日 ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export interface ItemCardProps {
  store: TodoStore;
  item: TodoItem;
  items: readonly TodoItem[];
  lists: readonly TodoList[];
  today: string;
  expanded: boolean;
  selected: boolean;
  pinned: boolean;
  /** Show which list the item belongs to (not inside that list's view). */
  showList: boolean;
  draggable: boolean;
  dropMark: "before" | "after" | null;
  onExpand(id: string | null): void;
  onToggleDone(id: string): void;
  onDelete(id: string): void;
  onPinToggle(id: string): void;
  onMenu(x: number, y: number, id: string): void;
  onNotice(message: string): void;
  onOpenImage(url: string): void;
  onDragStart(event: DragEvent, item: TodoItem): void;
  onDragOver(event: DragEvent, item: TodoItem): void;
  onDrop(event: DragEvent, item: TodoItem): void;
  onDragEnd(): void;
}

/** Collapsed: what you need at a glance. Expanded: everything, in place. */
export function ItemCard(props: ItemCardProps) {
  const { item, expanded } = props;
  return (
    <article
      aria-expanded={expanded}
      className="todo-card"
      data-deleted={item.deleted ? "true" : "false"}
      data-done={item.done ? "true" : "false"}
      data-drop={props.dropMark ?? undefined}
      data-expanded={expanded ? "true" : "false"}
      data-interactive-pointer-owner={expanded ? undefined : "true"}
      data-item-id={item.id}
      data-selected={props.selected ? "true" : "false"}
      draggable={props.draggable && !expanded}
      onClick={(event) => {
        if (expanded) return;
        if ((event.target as HTMLElement).closest("button, a, input, label")) return;
        props.onExpand(item.id);
      }}
      onContextMenu={(event) => {
        event.preventDefault();
        props.onMenu(event.clientX, event.clientY, item.id);
      }}
      onDragEnd={props.onDragEnd}
      onDragOver={(event) => props.onDragOver(event, item)}
      onDragStart={(event) => props.onDragStart(event, item)}
      onDrop={(event) => props.onDrop(event, item)}
    >
      {expanded ? <ExpandedCard {...props} /> : <CollapsedCard {...props} />}
    </article>
  );
}

function MetaChips({ item, lists, today, showList }: Pick<ItemCardProps, "item" | "lists" | "today" | "showList">) {
  const list = item.listId ? lists.find((entry) => entry.id === item.listId) : undefined;
  const chips: ReactNode[] = [];
  if (item.dueDate || item.remindAt !== null) {
    chips.push(
      <span className="todo-chip todo-due" data-state={dueState(item, today)} key="due">
        <icons.Calendar aria-hidden="true" size={11} />
        {dateLabel(item, today)}
        {item.remindAt !== null && !item.done ? <icons.Bell aria-hidden="true" className="todo-due-bell" size={11} /> : null}
        {item.recurrence ? <icons.Repeat aria-hidden="true" size={11} /> : null}
      </span>
    );
  }
  for (const tag of item.tags) chips.push(<span className="todo-chip todo-chip-tag" key={`tag:${tag}`}>#{tag}</span>);
  if (showList && list) chips.push(<span className="todo-chip" key="list"><ColorDot color={list.color} />{list.name}</span>);
  return chips.length ? <div className="todo-card-meta">{chips}</div> : null;
}

function CollapsedCard({ item, items, today, onToggleDone, onPinToggle, onMenu, pinned, ...rest }: ItemCardProps) {
  const children = item.deleted ? [] : childrenOf(items, item.id);
  const progress = progressOf(items, item.id);
  const open = children.filter((child) => !child.done);
  const shown = (open.length ? open : children).slice(0, 3);
  const preview = stripHtml(item.notes).slice(0, 160);
  const parent = item.parentId ? items.find((entry) => entry.id === item.parentId) : undefined;
  return (
    <>
      <div className="todo-card-head">
        {item.deleted ? (
          <icons.Trash2 aria-hidden="true" className="todo-card-trash" size={14} />
        ) : (
          <Checkbox checked={item.done} label={titleText(item.title) || "便笺"} onToggle={() => onToggleDone(item.id)} priority={item.priority} />
        )}
        <div className="todo-card-title">
          {/* Priority leads the title (high only; lower ones colour the ring). */}
          {item.priority === 3 ? <span className="todo-card-flag"><PriorityFlag priority={item.priority} /></span> : null}
          {item.parentId ? <span className="todo-card-parent">{titleText(parent?.title ?? "") || "上级事项"} ›</span> : null}
          <TitleView html={item.title} placeholder="无标题" />
        </div>
        <div className="todo-card-slot">
          <span className="todo-card-status">
            {pinned ? <icons.Pin aria-hidden="true" className="todo-card-pinned" size={12} /> : null}
          </span>
          <div className="todo-card-actions">
            {!item.deleted ? (
              <button aria-label={pinned ? "从桌面取下" : "贴到桌面"} className="todo-icon-button" data-active={pinned ? "true" : "false"} onClick={() => onPinToggle(item.id)} title={pinned ? "从桌面取下" : "贴到桌面"} type="button">
                {pinned ? <icons.PinOff aria-hidden="true" size={13} /> : <icons.Pin aria-hidden="true" size={13} />}
              </button>
            ) : null}
            <button
              aria-label="更多"
              className="todo-icon-button"
              onClick={(event) => {
                const rect = event.currentTarget.getBoundingClientRect();
                onMenu(rect.right - 4, rect.bottom + 2, item.id);
              }}
              title="更多"
              type="button"
            >
              <icons.MoreHorizontal aria-hidden="true" size={14} />
            </button>
          </div>
        </div>
      </div>
      {preview && !item.done ? <p className="todo-card-preview">{preview}</p> : null}
      {shown.length && !item.done ? (
        <ul className="todo-card-subtasks">
          {shown.map((child) => (
            <li data-done={child.done ? "true" : "false"} key={child.id}>
              <Checkbox checked={child.done} label={titleText(child.title) || "子事项"} onToggle={() => onToggleDone(child.id)} priority={child.priority} size="small" />
              <TitleView html={child.title} placeholder="未命名子事项" />
            </li>
          ))}
          {progress.total > shown.length ? (
            <li className="todo-card-more">{progress.done === progress.total - shown.length ? `+${progress.done} 已完成` : `还有 ${progress.total - shown.length} 项`}</li>
          ) : null}
        </ul>
      ) : null}
      <MetaChips item={item} lists={rest.lists} showList={rest.showList} today={today} />
    </>
  );
}

function PropertyChip({ active, filled, icon, label, onOpen, title }: { active: boolean; filled: boolean; icon: ReactNode; label?: ReactNode; onOpen(x: number, y: number): void; title: string }) {
  return (
    <button
      aria-expanded={active}
      aria-haspopup="dialog"
      className="todo-prop"
      data-active={active ? "true" : "false"}
      data-filled={filled ? "true" : "false"}
      onClick={(event) => {
        // The popover's top-left corner sits at the pointer (keyboard: below the chip).
        const rect = event.currentTarget.getBoundingClientRect();
        const pointer = event.detail > 0;
        onOpen(pointer ? event.clientX : rect.left, pointer ? event.clientY : rect.bottom + 4);
      }}
      title={title}
      type="button"
    >
      {icon}
      {label ? <span>{label}</span> : null}
    </button>
  );
}

const PROPERTY_ORDER: Property[] = ["date", "priority", "tags", "repeat", "list"];

function ExpandedCard({ store, item, items, lists, today, pinned, onExpand, onToggleDone, onDelete, onPinToggle, onNotice }: ItemCardProps) {
  const [popover, setPopover] = useState<PopoverState | null>(null);
  const [subtaskKey, setSubtaskKey] = useState(0);
  const byId = useMemo(() => new Map(items.map((entry) => [entry.id, entry])), [items]);
  const depth = depthOf(byId, item);
  const subtasks = childrenOf(items, item.id);
  const readOnly = item.deleted;
  const list = item.listId ? lists.find((entry) => entry.id === item.listId) : undefined;
  const properties = PROPERTY_ORDER.filter((property) => property !== "list" || !item.parentId);
  const shown = properties.filter((property) => propertySet(item, property));
  const available = properties.filter((property) => !propertySet(item, property));
  const open = (kind: PopoverState["kind"]) => (x: number, y: number) => setPopover({ kind, x, y });

  const uploadImage = async (file: File): Promise<{ id: string; url: string } | null> => {
    try {
      const image = await compressImage(file, file.name || "图片");
      const attachment = await store.addAttachment(item, image);
      if (!attachment) {
        onNotice("图片保存失败");
        return null;
      }
      return { id: attachment.id, url: `data:${attachment.mime};base64,${attachment.data}` };
    } catch (error) {
      onNotice(error instanceof Error ? error.message : String(error));
      return null;
    }
  };

  const chip = (property: Property) => {
    switch (property) {
      case "date": {
        const reminder = item.remindAt !== null;
        return (
          <PropertyChip
            active={popover?.kind === "date"}
            filled
            icon={reminder ? <icons.Bell aria-hidden="true" size={13} /> : <icons.Calendar aria-hidden="true" size={13} />}
            key="date"
            label={dateLabel(item, today)}
            onOpen={open("date")}
            title={reminder ? "日期（已开启提醒）" : "日期"}
          />
        );
      }
      case "priority":
        return <PropertyChip active={popover?.kind === "priority"} filled icon={<span data-button-content className="todo-flag" data-priority={item.priority}><icons.Flag aria-hidden="true" size={13} weight="fill" /></span>} key="priority" label={PRIORITY_LABELS[item.priority]} onOpen={open("priority")} title="优先级" />;
      case "tags":
        return <PropertyChip active={popover?.kind === "tags"} filled icon={<icons.Tag aria-hidden="true" size={13} />} key="tags" label={item.tags.map((tag) => `#${tag}`).join(" ")} onOpen={open("tags")} title="标签" />;
      case "repeat":
        return <PropertyChip active={popover?.kind === "repeat"} filled icon={<icons.Repeat aria-hidden="true" size={13} />} key="repeat" label={item.recurrence ? RECURRENCE_LABELS[item.recurrence.freq] : ""} onOpen={open("repeat")} title="重复" />;
      case "list":
        return <PropertyChip active={popover?.kind === "list"} filled icon={list ? <ColorDot buttonContent color={list.color} /> : <icons.ListTodo aria-hidden="true" size={13} />} key="list" label={list?.name ?? "清单"} onOpen={open("list")} title="清单" />;
    }
  };

  return (
    <div
      className="todo-card-editor"
      onKeyDown={(event) => {
        if (event.key !== "Escape" || event.defaultPrevented) return;
        // Esc inside a floating property editor (portaled, but its events
        // bubble through this tree) closes that editor, not the card.
        if ((event.target as HTMLElement).closest(".overlay-surface") || popover) {
          event.stopPropagation();
          setPopover(null);
          return;
        }
        event.stopPropagation();
        onExpand(null);
      }}
    >
      <div className="todo-card-head">
        {readOnly ? (
          <icons.Trash2 aria-hidden="true" className="todo-card-trash" size={14} />
        ) : (
          <Checkbox checked={item.done} label={titleText(item.title) || "便笺"} onToggle={() => onToggleDone(item.id)} priority={item.priority} />
        )}
        <InlineRichEditor
          ariaLabel="标题"
          autoFocus={!item.title}
          className="todo-card-title-input"
          disabled={readOnly}
          documentKey={item.id}
          onCommit={(html) => store.updateItem(item.id, { title: html })}
          onEnter={() => {
            const editor = document.querySelector<HTMLElement>(`[data-item-id="${item.id}"] .todo-editor-content`);
            editor?.focus();
          }}
          placeholder="标题"
          value={item.title}
        />
        <button aria-label="收起" className="todo-icon-button" onClick={() => onExpand(null)} title="收起（Esc）" type="button">
          <icons.ChevronUp aria-hidden="true" size={14} />
        </button>
      </div>

      {readOnly ? (
        <div className="todo-banner">
          <span>在“最近删除”中</span>
          <button className="todo-link-button" onClick={() => store.restoreItems([item.id])} type="button"><span data-button-content>恢复</span></button>
        </div>
      ) : (
        <div aria-label="属性" className="todo-props" role="toolbar">
          {shown.map(chip)}
          {available.length ? (
            <button
              aria-expanded={popover?.kind === "add"}
              aria-haspopup="menu"
              aria-label="添加属性"
              className="todo-prop todo-prop-more"
              data-active={popover?.kind === "add" ? "true" : "false"}
              onClick={(event) => {
                const rect = event.currentTarget.getBoundingClientRect();
                const pointer = event.detail > 0;
                setPopover({ kind: "add", x: pointer ? event.clientX : rect.left, y: pointer ? event.clientY : rect.bottom + 4 });
              }}
              title={available.map((property) => PROPERTY_LABELS[property]).join("、")}
              type="button"
            >
              <icons.Plus aria-hidden="true" size={12} />
              <span>属性</span>
            </button>
          ) : null}
        </div>
      )}

      {popover && !readOnly ? (
        <PropertyPopover
          available={available}
          item={item}
          lists={lists}
          onClose={() => setPopover(null)}
          onPick={(property, x, y) => setPopover({ kind: property, x, y })}
          state={popover}
          store={store}
          today={today}
        />
      ) : null}

      <ul className="todo-subtasks">
        {subtasks.map((subtask, index) => (
          <SubtaskRow
            item={subtask}
            key={subtask.id}
            onBackspaceEmpty={() => {
              store.deleteItems([subtask.id]);
              const previous = subtasks[index - 1];
              window.requestAnimationFrame(() => {
                const target = previous
                  ? document.querySelector<HTMLElement>(`[data-subtask-id="${previous.id}"] .todo-inline-editor`)
                  : document.querySelector<HTMLElement>(`[data-item-id="${item.id}"] .todo-subtask-add .todo-inline-editor`);
                focusEnd(target);
              });
            }}
            onEnter={() => focusEnd(document.querySelector<HTMLElement>(`[data-item-id="${item.id}"] .todo-subtask-add .todo-inline-editor`))}
            onToggle={() => onToggleDone(subtask.id)}
            store={store}
          />
        ))}
        {!readOnly && depth < MAX_SUBTASK_DEPTH ? (
          <li className="todo-subtask-add">
            <icons.Plus aria-hidden="true" size={12} />
            <InlineRichEditor
              ariaLabel="添加子事项"
              documentKey={`${item.id}:new:${subtaskKey}`}
              onCommit={() => undefined}
              onEnter={() => {
                const field = document.querySelector<HTMLElement>(`[data-item-id="${item.id}"] .todo-subtask-add .todo-inline-editor`);
                const html = field ? field.innerHTML : "";
                if (!titleText(html).trim()) return;
                store.createItem({ title: cleanTitle(html), parentId: item.id });
                setSubtaskKey((value) => value + 1);
                window.requestAnimationFrame(() => focusEnd(document.querySelector<HTMLElement>(`[data-item-id="${item.id}"] .todo-subtask-add .todo-inline-editor`)));
              }}
              placeholder="子事项"
              value=""
            />
          </li>
        ) : null}
      </ul>

      {readOnly ? (
        <NotesView html={item.notes} store={store} />
      ) : (
        <RichTextEditor documentKey={item.id} onChange={(html) => store.updateItem(item.id, { notes: html })} onImage={uploadImage} store={store} value={item.notes} />
      )}

      <footer className="todo-card-footer">
        <span className="todo-card-stamp" title={`创建于 ${formatStamp(item.createdAt)}`}>
          {item.done && item.completedAt ? `完成于 ${formatStamp(item.completedAt)}` : `编辑于 ${formatStamp(item.updatedAt)}`}
        </span>
        <div className="todo-card-add">
          {readOnly ? (
            <button className="todo-link-button" onClick={() => store.purgeItems([item.id])} type="button"><span data-button-content>永久删除</span></button>
          ) : (
            <>
              <button aria-label={pinned ? "从桌面取下" : "贴到桌面"} aria-pressed={pinned} className="todo-icon-button" data-active={pinned ? "true" : "false"} onClick={() => onPinToggle(item.id)} title={pinned ? "从桌面取下" : "贴到桌面"} type="button">
                {pinned ? <icons.PinOff aria-hidden="true" size={14} /> : <icons.Pin aria-hidden="true" size={14} />}
              </button>
              <button aria-label="删除" className="todo-icon-button todo-danger" onClick={() => onDelete(item.id)} title="删除（可撤销）" type="button">
                <icons.Trash2 aria-hidden="true" size={14} />
              </button>
            </>
          )}
        </div>
      </footer>
    </div>
  );
}

function focusEnd(element: HTMLElement | null): void {
  if (!element) return;
  element.focus();
  const range = document.createRange();
  range.selectNodeContents(element);
  range.collapse(false);
  window.getSelection()?.removeAllRanges();
  window.getSelection()?.addRange(range);
}

function SubtaskRow({ store, item, onToggle, onEnter, onBackspaceEmpty }: { store: TodoStore; item: TodoItem; onToggle(): void; onEnter(): void; onBackspaceEmpty(): void }) {
  return (
    <li className="todo-subtask" data-done={item.done ? "true" : "false"} data-subtask-id={item.id}>
      <Checkbox checked={item.done} label={titleText(item.title) || "子事项"} onToggle={onToggle} priority={item.priority} size="small" />
      <InlineRichEditor
        ariaLabel="子事项"
        className="todo-subtask-title"
        documentKey={item.id}
        onCommit={(html) => store.updateItem(item.id, { title: html })}
        onEmptyBackspace={onBackspaceEmpty}
        onEnter={onEnter}
        placeholder="子事项"
        value={item.title}
      />
      <button aria-label="删除子事项" className="todo-icon-button" onClick={() => store.deleteItems([item.id])} title="删除" type="button">
        <icons.X aria-hidden="true" size={12} />
      </button>
    </li>
  );
}

