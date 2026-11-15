import type { ReactNode } from "react";
import { icons } from "@mangoo/web/plugin-host";

import { PRIORITY_LABELS, dueState, formatDueLabel, type Priority, type TodoItem } from "../shared/model.ts";

/**
 * The completion control. Ring and tick are one SVG drawn on the device pixel
 * grid (geometric anti-aliasing), not a CSS border-radius border: a 1.5px
 * border on a small circle is rasterized without sub-pixel coverage and reads
 * as jagged on a 100% display.
 */
export function Checkbox({ checked, onToggle, label, priority = 0, size = "normal" }: {
  checked: boolean;
  onToggle(): void;
  label: string;
  priority?: Priority;
  size?: "normal" | "small";
}) {
  return (
    <button
      aria-checked={checked}
      aria-label={label}
      className="todo-check"
      data-checked={checked ? "true" : "false"}
      data-priority={priority}
      data-size={size}
      onClick={(event) => {
        event.stopPropagation();
        onToggle();
      }}
      role="checkbox"
      title={checked ? "标记为未完成" : "标记为完成"}
      type="button"
    >
      <svg aria-hidden="true" className="todo-check-svg" viewBox="0 0 20 20">
        <circle className="todo-check-ring" cx="10" cy="10" r="8.6" />
        {checked ? <path className="todo-check-mark" d="M6.1 10.4 8.7 12.9 13.9 7.3" /> : null}
      </svg>
    </button>
  );
}

export function PriorityFlag({ priority }: { priority: Priority }) {
  if (priority === 0) return null;
  return (
    <span aria-label={`优先级：${PRIORITY_LABELS[priority]}`} className="todo-flag" data-priority={priority} title={`优先级：${PRIORITY_LABELS[priority]}`}>
      <icons.Flag aria-hidden="true" size={12} weight="fill" />
    </span>
  );
}

export function DueChip({ item, today }: { item: Pick<TodoItem, "dueDate" | "dueTime" | "done" | "recurrence" | "remindAt">; today: string }) {
  if (!item.dueDate) return item.remindAt ? <span className="todo-chip"><icons.Bell aria-hidden="true" size={11} /></span> : null;
  const state = dueState(item, today);
  return (
    <span className="todo-chip todo-due" data-state={state} title={state === "overdue" ? "已逾期" : "截止日期"}>
      <icons.Calendar aria-hidden="true" size={11} />
      <span>{formatDueLabel(item.dueDate, item.dueTime, today)}</span>
      {item.recurrence ? <icons.Repeat aria-hidden="true" size={11} /> : null}
      {item.remindAt ? <icons.Bell aria-hidden="true" size={11} /> : null}
    </span>
  );
}

export function EmptyState({ icon, title, children }: { icon: ReactNode; title: string; children?: ReactNode }) {
  return (
    <div className="todo-empty">
      <span aria-hidden="true" className="todo-empty-icon">{icon}</span>
      <strong>{title}</strong>
      {children ? <p>{children}</p> : null}
    </div>
  );
}

/** A small colored dot (list color). */
export function ColorDot({ color }: { color: string }) {
  return <span aria-hidden="true" className="todo-dot" style={{ background: color }} />;
}
