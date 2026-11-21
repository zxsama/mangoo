import { useEffect, useRef, useState, type DragEvent } from "react";
import type { NoteSummary } from "@mangoo/core-client";
import { icons } from "@mangoo/web/plugin-host";
import type { SaveStatus } from "./noteSession";
import "./notebookTabs.css";

export interface NotebookTab {
  id: string;
  title: string;
  format: NoteSummary["format"];
  status?: SaveStatus;
}

const tabDragType = "application/x-mangoo-notebook-tab";
type DropPosition = { id: string; side: "before" | "after" };

function revealSelectedTab(list: HTMLDivElement) {
  const active = list.querySelector<HTMLElement>('[data-active="true"]');
  if (!active) return;
  const tab = active.getBoundingClientRect(), viewport = list.getBoundingClientRect();
  if (tab.left < viewport.left || tab.width > viewport.width) list.scrollLeft += tab.left - viewport.left;
  else if (tab.right > viewport.right) list.scrollLeft += tab.right - viewport.right;
}

export function NotebookTabStrip({ tabs, activeId, disabled, select, close, reorder, create }: {
  tabs: NotebookTab[];
  activeId?: string | null;
  disabled: boolean;
  select(id: string): void;
  close(id: string): void;
  reorder(id: string, targetId: string, side: DropPosition["side"]): void;
  create(): void;
}) {
  const list = useRef<HTMLDivElement>(null);
  const restoreFocus = useRef(false);
  const dragged = useRef<string | null>(null);
  const [insertion, setInsertion] = useState<DropPosition | null>(null);
  const add = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const element = list.current;
    if (!element) return;
    const scrollTabs = (event: WheelEvent) => {
      if (event.ctrlKey || event.shiftKey || event.deltaX || !event.deltaY ||
        element.scrollWidth <= element.clientWidth) return;
      const unit = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? 16 :
        event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? element.clientWidth : 1;
      element.scrollLeft += event.deltaY * unit;
      event.preventDefault();
    };
    element.addEventListener("wheel", scrollTabs, { passive: false });
    const observer = new ResizeObserver(() => revealSelectedTab(element));
    observer.observe(element);
    return () => { observer.disconnect(); element.removeEventListener("wheel", scrollTabs); };
  }, []);
  useEffect(() => {
    const selected = list.current?.querySelector<HTMLButtonElement>('[aria-selected="true"]');
    if (list.current) revealSelectedTab(list.current);
    if (restoreFocus.current && !disabled) {
      (selected ?? add.current)?.focus({ preventScroll: true });
      restoreFocus.current = false;
    }
  }, [activeId, tabs.length, disabled]);

  function dropPosition(event: DragEvent<HTMLDivElement>): DropPosition | undefined {
    if (disabled || !dragged.current || !event.dataTransfer.types.includes(tabDragType)) return;
    const hit = event.target instanceof Element && event.target.closest<HTMLElement>("[data-tab-id]");
    const items = event.currentTarget.children;
    // The gap between two tabs and the strip's outer edges are valid targets too.
    const tab = hit || Array.from(items).find(item => event.clientX < item.getBoundingClientRect().right)
      || items[items.length - 1];
    if (!(tab instanceof HTMLElement) || !tab.dataset.tabId) return;
    const bounds = tab.getBoundingClientRect();
    return { id: tab.dataset.tabId, side: event.clientX < bounds.left + bounds.width / 2 ? "before" : "after" };
  }
  function endDrag() { dragged.current = null; setInsertion(null); }

  return <div className="notebook-tabstrip" onKeyDown={event => {
    if (disabled || event.defaultPrevented || event.altKey || !(event.ctrlKey || event.metaKey)) return;
    // Workspace shortcuts own the operation. Keep keyboard focus in the tabstrip
    // when those shortcuts are invoked from a tab that will be removed or deselected.
    if ((event.key === "Tab" && tabs.length > 1) ||
      (event.key.toLowerCase() === "w" && !event.shiftKey && activeId)) {
      restoreFocus.current = true;
    }
  }}>
    <div className="notebook-tablist" ref={list} role="tablist" aria-label="打开的笔记"
      onDragOver={event => {
        const position = dropPosition(event);
        if (!position) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "move";
        setInsertion(current => current?.id === position.id && current.side === position.side ? current : position);
      }}
      onDragLeave={event => {
        if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) setInsertion(null);
      }}
      onDrop={event => {
        const position = dropPosition(event);
        if (!position) return;
        event.preventDefault();
        const id = event.dataTransfer.getData(tabDragType);
        if (id && id === dragged.current) reorder(id, position.id, position.side);
        endDrag();
      }}
      onDragEnd={endDrag}>
      {tabs.map((tab, index) => {
        const current = tab.id === activeId;
        const dirty = tab.status && tab.status !== "saved";
        const warning = tab.status === "conflict" || tab.status === "deleted" || tab.status === "failed";
        const Icon = tab.format === "html" ? icons.FileCode2 : tab.format === "document" ? icons.File : icons.FileText;
        return <div key={tab.id} className="notebook-tab" role="presentation" data-active={current} data-tab-id={tab.id}
          data-drop-side={insertion?.id === tab.id ? insertion.side : undefined}
          onMouseDown={event => { if (event.button === 1) event.preventDefault(); }}
          onAuxClick={event => {
            if (event.button !== 1 || disabled) return;
            event.preventDefault();
            restoreFocus.current = current;
            close(tab.id);
          }}>
          <button type="button" role="tab" id={"nb-tab-" + tab.id} aria-controls={current ? "nb-active-document" : undefined}
            aria-selected={current} aria-label={tab.title + (dirty ? "，未保存" : "")} aria-disabled={disabled}
            tabIndex={current ? 0 : -1} title={tab.title} className="notebook-tab-select" draggable={!disabled && tabs.length > 1}
            aria-keyshortcuts="Control+Shift+PageUp Control+Shift+PageDown Delete"
            onDragStart={event => {
              if (disabled || tabs.length < 2) { event.preventDefault(); return; }
              dragged.current = tab.id;
              event.dataTransfer.effectAllowed = "move";
              event.dataTransfer.setData(tabDragType, tab.id);
            }}
            onClick={() => { if (!disabled) select(tab.id); }}
            onKeyDown={event => {
              if (!disabled && document.activeElement === event.currentTarget && event.ctrlKey && event.shiftKey &&
                !event.metaKey && !event.altKey && (event.key === "PageUp" || event.key === "PageDown")) {
                event.preventDefault();
                event.stopPropagation();
                const left = event.key === "PageUp";
                const neighbor = tabs[index + (left ? -1 : 1)];
                if (neighbor) {
                  const button = event.currentTarget;
                  reorder(tab.id, neighbor.id, left ? "before" : "after");
                  requestAnimationFrame(() => {
                    if (!button.isConnected) return;
                    button.focus({ preventScroll: true });
                    if (list.current) revealSelectedTab(list.current);
                  });
                }
                return;
              }
              if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || disabled) return;
              let next: number;
              switch (event.key) {
                case "ArrowLeft": next = (index + tabs.length - 1) % tabs.length; break;
                case "ArrowRight": next = (index + 1) % tabs.length; break;
                case "Home": next = 0; break;
                case "End": next = tabs.length - 1; break;
                case "Delete":
                  event.preventDefault(); restoreFocus.current = true; close(tab.id); return;
                default: return;
              }
              event.preventDefault();
              if (next !== index) { restoreFocus.current = true; select(tabs[next].id); }
            }}>
            <Icon size={15} aria-hidden="true" /><span>{tab.title}</span>
            {dirty && <i data-button-content className="notebook-tab-dirty" data-warning={warning} aria-hidden="true" />}
          </button>
          <button type="button" className="notebook-tab-close" title={"关闭「" + tab.title + "」"}
            aria-label={"关闭标签页 " + tab.title} disabled={disabled} tabIndex={current ? 0 : -1}
            onClick={() => { restoreFocus.current = current; close(tab.id); }}><icons.X size={12} /></button>
        </div>;
      })}
    </div>
    <button ref={add} type="button" className="nb-icon notebook-tab-add" aria-label="新建 Markdown 笔记"
      title="新建 Markdown 笔记" disabled={disabled} onClick={create}><icons.Plus size={16} /></button>
  </div>;
}
