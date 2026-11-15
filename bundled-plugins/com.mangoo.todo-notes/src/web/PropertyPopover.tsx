import { useEffect, useState, type ReactNode } from "react";
import { AnchoredOverlay, OverlayMenuItem, OverlayMenuList, icons, pointOverlayAnchor } from "@mangoo/web/plugin-host";

import {
  MAX_TAGS,
  PRIORITIES,
  PRIORITY_LABELS,
  RECURRENCE_LABELS,
  addDays,
  formatDueLabel,
  localTimestamp,
  normalizeTag,
  pad,
  toDateKey,
  type Priority,
  type RecurrenceFrequency,
  type TodoItem,
  type TodoList
} from "../shared/model.ts";
import { ColorDot } from "./components.tsx";
import type { TodoStore } from "./store.ts";

/** Properties a note can carry; the reminder is part of the date. */
export type Property = "date" | "repeat" | "priority" | "tags" | "list";

export const PROPERTY_LABELS: Record<Property, string> = { date: "日期与提醒", repeat: "重复", priority: "优先级", tags: "标签", list: "清单" };

export const PROPERTY_ICONS: Record<Property, typeof icons.Calendar> = {
  date: icons.Calendar,
  repeat: icons.Repeat,
  priority: icons.Flag,
  tags: icons.Tag,
  list: icons.ListTodo
};

/** Offsets of a reminder before the due time (minutes). */
export const REMINDER_OFFSETS: Array<{ minutes: number; label: string }> = [
  { minutes: 0, label: "准时" },
  { minutes: 10, label: "提前 10 分钟" },
  { minutes: 60, label: "提前 1 小时" },
  { minutes: 1440, label: "提前 1 天" }
];

/** The reminder's offset before the due time, or null (none / custom). */
export function reminderOffset(item: Pick<TodoItem, "dueDate" | "dueTime" | "remindAt">): number | null {
  if (item.remindAt === null || !item.dueDate) return null;
  return Math.round((localTimestamp(item.dueDate, item.dueTime ?? "09:00") - item.remindAt) / 60_000);
}

function toLocalInput(ms: number): string {
  const date = new Date(ms);
  return `${toDateKey(date)}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** A property is "set" when it holds a value (set ones show as chips). */
export function propertySet(item: TodoItem, property: Property): boolean {
  switch (property) {
    case "date": return Boolean(item.dueDate) || item.remindAt !== null;
    case "repeat": return Boolean(item.recurrence);
    case "priority": return item.priority > 0;
    case "tags": return item.tags.length > 0;
    case "list": return Boolean(item.listId);
  }
}

interface EditorProps {
  store: TodoStore;
  item: TodoItem;
  lists: readonly TodoList[];
  today: string;
  close(): void;
}

function Choice({ selected, onClick, children, title }: { selected: boolean; onClick(): void; children: ReactNode; title?: string }) {
  return (
    <button aria-checked={selected} className="todo-pill" data-selected={selected ? "true" : "false"} onClick={onClick} role="radio" title={title} type="button">
      {children}
    </button>
  );
}

function DateEditor({ store, item, today }: EditorProps) {
  const offset = reminderOffset(item);
  const custom = item.remindAt !== null && (offset === null || !REMINDER_OFFSETS.some((entry) => entry.minutes === offset));
  const setDue = (dueDate: string | null, dueTime: string | null = item.dueTime) => {
    const patch: Partial<TodoItem> = { dueDate, dueTime: dueDate ? dueTime : null };
    // The reminder keeps its offset when the date / time moves.
    if (dueDate && offset !== null && !custom) patch.remindAt = localTimestamp(dueDate, dueTime ?? "09:00") - offset * 60_000;
    if (!dueDate) {
      patch.recurrence = null;
      if (!custom) patch.remindAt = null;
    }
    store.updateItem(item.id, patch);
  };
  const setReminder = (minutes: number | null) => {
    if (minutes === null) {
      store.updateItem(item.id, { remindAt: null });
      return;
    }
    const dueDate = item.dueDate ?? today;
    const dueTime = item.dueTime ?? "09:00";
    store.updateItem(item.id, { dueDate, dueTime, remindAt: localTimestamp(dueDate, dueTime) - minutes * 60_000 });
  };
  const reminderOn = item.remindAt !== null;
  return (
    <div className="todo-pop-body">
      <div className="todo-pop-row">
        <Choice onClick={() => setDue(today)} selected={item.dueDate === today}>今天</Choice>
        <Choice onClick={() => setDue(addDays(today, 1))} selected={item.dueDate === addDays(today, 1)}>明天</Choice>
        <Choice onClick={() => setDue(addDays(today, 7))} selected={item.dueDate === addDays(today, 7)}>下周</Choice>
      </div>
      <div className="todo-pop-row">
        <input aria-label="日期" className="todo-input" onChange={(event) => setDue(event.target.value || null)} type="date" value={item.dueDate ?? ""} />
        <input aria-label="时间" className="todo-input todo-time-input" disabled={!item.dueDate} onChange={(event) => setDue(item.dueDate, event.target.value || null)} type="time" value={item.dueTime ?? ""} />
      </div>
      <label className="todo-pop-switch">
        <icons.Bell aria-hidden="true" size={13} />
        <span>提醒</span>
        <span className={`plugin-toggle todo-switch${reminderOn ? " on" : ""}`}>
          <input aria-label="提醒" checked={reminderOn} onChange={(event) => setReminder(event.target.checked ? 0 : null)} role="switch" type="checkbox" />
          <span aria-hidden="true" className="plugin-toggle-track"><span className="plugin-toggle-thumb" /></span>
        </span>
      </label>
      {reminderOn ? (
        <>
          <div aria-label="提醒时间" className="todo-pop-row todo-pop-wrap" role="radiogroup">
            {REMINDER_OFFSETS.map((entry) => (
              <Choice key={entry.minutes} onClick={() => setReminder(entry.minutes)} selected={!custom && offset === entry.minutes}>{entry.label}</Choice>
            ))}
            <Choice onClick={() => store.updateItem(item.id, { remindAt: item.remindAt ?? Date.now() + 3_600_000 })} selected={custom}>自定义</Choice>
          </div>
          {custom && item.remindAt !== null ? (
            <input aria-label="提醒时间" className="todo-input" onChange={(event) => event.target.value && store.updateItem(item.id, { remindAt: new Date(event.target.value).getTime() })} type="datetime-local" value={toLocalInput(item.remindAt)} />
          ) : null}
        </>
      ) : null}
      {item.dueDate || reminderOn ? (
        <button className="todo-pop-clear" onClick={() => store.updateItem(item.id, { dueDate: null, dueTime: null, remindAt: null, recurrence: null })} type="button">
          <icons.X aria-hidden="true" size={12} />
          清除日期和提醒
        </button>
      ) : null}
    </div>
  );
}

function RepeatEditor({ store, item, today }: EditorProps) {
  const choices: Array<[RecurrenceFrequency | "", string]> = [["", "不重复"], ...(Object.entries(RECURRENCE_LABELS) as Array<[RecurrenceFrequency, string]>)];
  return (
    <div className="todo-pop-body">
      <div aria-label="重复" className="todo-pop-row todo-pop-wrap" role="radiogroup">
        {choices.map(([freq, label]) => (
          <Choice
            key={freq || "none"}
            onClick={() => {
              if (!freq) store.updateItem(item.id, { recurrence: null });
              else store.updateItem(item.id, { recurrence: { freq, interval: item.recurrence?.interval ?? 1 }, ...(item.dueDate ? {} : { dueDate: today }) });
            }}
            selected={(item.recurrence?.freq ?? "") === freq}
          >
            {label}
          </Choice>
        ))}
      </div>
      {item.recurrence && item.recurrence.freq !== "weekdays" ? (
        <label className="todo-interval">
          <span>每</span>
          <input
            aria-label="间隔"
            className="todo-input"
            max={365}
            min={1}
            onChange={(event) => item.recurrence && store.updateItem(item.id, { recurrence: { ...item.recurrence, interval: Math.min(365, Math.max(1, Number(event.target.value) || 1)) } })}
            type="number"
            value={item.recurrence.interval}
          />
          <span>{{ daily: "天", weekly: "周", monthly: "个月", yearly: "年", weekdays: "" }[item.recurrence.freq]}</span>
        </label>
      ) : null}
    </div>
  );
}

function PriorityEditor({ store, item, close }: EditorProps) {
  return (
    <div className="todo-pop-body">
      <div aria-label="优先级" className="todo-pop-row" role="radiogroup">
        {PRIORITIES.map((priority) => (
          <Choice key={priority} onClick={() => { store.updateItem(item.id, { priority: priority as Priority }); close(); }} selected={item.priority === priority} title={priority ? `${PRIORITY_LABELS[priority]}优先级` : "无优先级"}>
            <span className="todo-flag" data-priority={priority}><icons.Flag aria-hidden="true" size={12} weight={priority ? "fill" : "regular"} /></span>
            {priority ? PRIORITY_LABELS[priority] : "无"}
          </Choice>
        ))}
      </div>
    </div>
  );
}

function TagsEditor({ store, item }: EditorProps) {
  const [draft, setDraft] = useState("");
  const add = (value: string) => {
    const tag = normalizeTag(value);
    setDraft("");
    if (!tag || item.tags.includes(tag) || item.tags.length >= MAX_TAGS) return;
    store.updateItem(item.id, { tags: [...item.tags, tag] });
  };
  const needle = normalizeTag(draft);
  const suggestions = store.tags().filter((tag) => !item.tags.includes(tag) && (!needle || tag.includes(needle))).slice(0, 8);
  return (
    <div className="todo-pop-body">
      {item.tags.length ? (
        <div className="todo-pop-row todo-pop-wrap">
          {item.tags.map((tag) => (
            <span className="todo-tag" key={tag}>
              #{tag}
              <button aria-label={`移除标签 ${tag}`} onClick={() => store.updateItem(item.id, { tags: item.tags.filter((entry) => entry !== tag) })} type="button">
                <icons.X aria-hidden="true" size={10} />
              </button>
            </span>
          ))}
        </div>
      ) : null}
      {item.tags.length < MAX_TAGS ? (
        <input
          aria-label="添加标签"
          autoFocus
          className="todo-input"
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if ((event.key === "Enter" || event.key === "," || event.key === " ") && !event.nativeEvent.isComposing) {
              event.preventDefault();
              add(draft);
            }
          }}
          placeholder="输入标签，回车添加"
          value={draft}
        />
      ) : null}
      {suggestions.length ? (
        <div aria-label="已有标签" className="todo-pop-row todo-pop-wrap">
          {suggestions.map((tag) => (
            <button className="todo-pill" key={tag} onClick={() => add(tag)} type="button">#{tag}</button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function ListEditor({ store, item, lists, close }: EditorProps) {
  return (
    <OverlayMenuList ariaLabel="清单">
      <OverlayMenuItem checked={item.listId === null} icon={<icons.Inbox size={14} />} onClick={() => { store.moveToList(item.id, null); close(); }} role="menuitemradio" shortcut={item.listId === null ? <icons.Check aria-hidden="true" className="todo-menu-current" size={13} /> : undefined}>
        无清单
      </OverlayMenuItem>
      {lists.map((list) => (
        <OverlayMenuItem checked={item.listId === list.id} icon={<ColorDot color={list.color} />} key={list.id} onClick={() => { store.moveToList(item.id, list.id); close(); }} role="menuitemradio" shortcut={item.listId === list.id ? <icons.Check aria-hidden="true" className="todo-menu-current" size={13} /> : undefined}>
          {list.name}
        </OverlayMenuItem>
      ))}
    </OverlayMenuList>
  );
}

export interface PopoverState {
  kind: Property | "add";
  x: number;
  y: number;
}

/**
 * The property popover: a small floating panel whose top-left corner sits at
 * the pointer (it flips / slides to stay in the window), so editing a
 * property never pushes the card's content around.
 */
export function PropertyPopover({ state, store, item, lists, today, available, onPick, onClose }: {
  state: PopoverState;
  store: TodoStore;
  item: TodoItem;
  lists: readonly TodoList[];
  today: string;
  /** For "add": properties not set yet. */
  available: Property[];
  onPick(property: Property, x: number, y: number): void;
  onClose(): void;
}) {
  const props: EditorProps = { store, item, lists, today, close: onClose };
  // A deleted / vanished note closes its popover.
  useEffect(() => {
    if (item.deleted) onClose();
  }, [item.deleted, onClose]);
  const menu = state.kind === "add" || state.kind === "list";
  return (
    <AnchoredOverlay
      anchor={pointOverlayAnchor(state.x, state.y)}
      ariaLabel={state.kind === "add" ? "添加属性" : PROPERTY_LABELS[state.kind]}
      className={`context-menu todo-menu${menu ? "" : " todo-pop"}`}
      fitToViewport
      id="todo-property-popover"
      onDismiss={onClose}
      open
      placement="bottom-start"
      preferredWidth={state.kind === "date" ? 272 : state.kind === "tags" ? 248 : menu ? 196 : 240}
      role={menu ? "group" : "dialog"}
      rootId="context-menu"
    >
      {state.kind === "add" ? (
        <OverlayMenuList ariaLabel="添加属性">
          {available.map((property) => {
            const Icon = PROPERTY_ICONS[property];
            return (
              <OverlayMenuItem icon={<Icon size={14} />} key={property} onClick={() => onPick(property, state.x, state.y)} role="menuitem">
                {PROPERTY_LABELS[property]}
              </OverlayMenuItem>
            );
          })}
        </OverlayMenuList>
      ) : (
        <>
          {menu ? null : <div className="todo-pop-title">{PROPERTY_LABELS[state.kind]}</div>}
          {state.kind === "date" ? <DateEditor {...props} /> : null}
          {state.kind === "repeat" ? <RepeatEditor {...props} /> : null}
          {state.kind === "priority" ? <PriorityEditor {...props} /> : null}
          {state.kind === "tags" ? <TagsEditor {...props} /> : null}
          {state.kind === "list" ? <ListEditor {...props} /> : null}
        </>
      )}
    </AnchoredOverlay>
  );
}

/** The date chip's label: date, plus the bell when a reminder is on. */
export function dateLabel(item: TodoItem, today: string): string {
  if (item.dueDate) return formatDueLabel(item.dueDate, item.dueTime, today);
  if (item.remindAt !== null) {
    const date = new Date(item.remindAt);
    return formatDueLabel(toDateKey(date), `${pad(date.getHours())}:${pad(date.getMinutes())}`, today);
  }
  return "日期";
}
