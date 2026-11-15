import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent, type MouseEvent as ReactMouseEvent } from "react";
import {
  AnchoredOverlay,
  LiquidGlassLayer,
  LiquidGlassSurface,
  ModalSurface,
  OverlayDialogBody,
  OverlayDialogHeader,
  OverlayMenuItem,
  OverlayMenuList,
  getWindowControls,
  icons,
  pointOverlayAnchor,
  useOverlayScrollbars,
  useSecondaryWindowInterfaceStyle
} from "@mangoo/web/plugin-host";

import {
  LIST_COLORS,
  PRIORITY_LABELS,
  SMART_VIEW_LABELS,
  SORT_LABELS,
  addDays,
  compareItems,
  compareOrder,
  dueState,
  escapeTitle,
  formatDueLabel,
  itemInView,
  matchesSearch,
  parseQuickAdd,
  titleText,
  toDateKey,
  viewShowsCompletedGroup,
  type Priority,
  type SmartView,
  type SortMode,
  type TodoItem,
  type TodoList,
  type ViewKey
} from "../shared/model.ts";
import { SELECTION_ACTIONS, matchAction, resolveBindings, shortcutHint, type ShortcutActionId } from "../shared/shortcuts.ts";
import { todoBridge } from "./bridge.ts";
import { ColorDot, EmptyState, PriorityFlag } from "./components.tsx";
import { ItemCard } from "./ItemCard.tsx";
import { useTodoPreferences } from "./preferences.ts";
import { textToHtml } from "./richText.ts";
import { TodoStore, useTodoSnapshot } from "./store.ts";
import "./todo.css";

/** Trailing mark of a menu radio / checkbox item that is the current choice. */
function Current({ on }: { on: boolean }) {
  return on ? <icons.Check aria-hidden="true" className="todo-menu-current" size={13} /> : null;
}

/** Quick views in the chip row; 已完成 / 最近删除 live in the view menu. */
const CHIP_VIEWS: SmartView[] = ["all", "today", "upcoming", "important"];
const MENU_VIEWS: SmartView[] = ["all", "today", "upcoming", "important", "completed", "trash"];
const SMART_ICONS: Record<SmartView, typeof icons.Sun> = {
  today: icons.Sun,
  upcoming: icons.Calendar,
  important: icons.Flag,
  all: icons.StickyNote,
  completed: icons.CheckCircle2,
  trash: icons.Trash2
};
const LIST_COLOR_NAMES = ["蓝", "绿", "橙", "红", "紫", "青", "粉", "灰"];
const DEFAULT_SORT: Partial<Record<SmartView, SortMode>> = { today: "due", upcoming: "due", important: "due", completed: "created", trash: "created" };

function useToday(): string {
  const [today, setToday] = useState(() => toDateKey(new Date()));
  useEffect(() => {
    const update = () => setToday(toDateKey(new Date()));
    const timer = window.setInterval(update, 60_000);
    window.addEventListener("focus", update);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", update);
    };
  }, []);
  return today;
}

interface Notice {
  id: number;
  message: string;
  action?: { label: string; run(): void };
}

interface Group {
  key: string;
  title: string | null;
  tone?: "overdue";
  items: TodoItem[];
  collapsible?: boolean;
}

type MenuState = { x: number; y: number; kind: "item" | "list" | "more" | "view" | "sort"; id: string | null };
type ListDraft = { id: string | null; name: string; color: string };

function viewTitle(view: ViewKey, lists: readonly TodoList[]): string {
  if (view.startsWith("list:")) return lists.find((list) => `list:${list.id}` === view)?.name ?? "清单";
  if (view.startsWith("tag:")) return `#${view.slice(4)}`;
  return view === "all" ? "便笺" : SMART_VIEW_LABELS[view as SmartView] ?? "便笺";
}

/**
 * The notes window: one portrait column without a titlebar. Capture is
 * always one keystroke away at the top; notes are cards whose summary
 * (title, preview, a few tickable subtasks, chips) answers most questions
 * without a click; clicking a card edits it in place.
 */
export function TodoNotesWindow() {
  useSecondaryWindowInterfaceStyle();
  useOverlayScrollbars();
  const store = useMemo(() => new TodoStore(), []);
  useEffect(() => {
    store.start();
    return () => store.stop();
  }, [store]);
  const snapshot = useTodoSnapshot(store);
  const [preferences, setPreferences] = useTodoPreferences();
  const today = useToday();
  const bridge = todoBridge();
  const controls = getWindowControls();

  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  const [capture, setCapture] = useState("");
  const [pinned, setPinned] = useState<string[]>([]);
  const [onTop, setOnTop] = useState(false);
  const [stickiesShown, setStickiesShown] = useState(true);
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [notices, setNotices] = useState<Notice[]>([]);
  const [editingList, setEditingList] = useState<ListDraft | null>(null);
  const [drag, setDrag] = useState<{ id: string; over: string | null; position: "before" | "after" } | null>(null);
  /** Items just (un)completed keep their place for a moment instead of jumping. */
  const [settling, setSettling] = useState<Map<string, boolean>>(() => new Map());
  const settleTimer = useRef<number | null>(null);
  const [lightbox, setLightbox] = useState<string | null>(null);
  const captureRef = useRef<HTMLTextAreaElement | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const streamRef = useRef<HTMLDivElement | null>(null);
  const chipsRef = useRef<HTMLElement | null>(null);
  const noticeId = useRef(0);

  const view = preferences.view;
  const lists = useMemo(() => [...snapshot.lists].sort(compareOrder), [snapshot.lists]);
  const listIds = useMemo(() => new Set(lists.map((list) => list.id)), [lists]);
  useEffect(() => {
    if (snapshot.status === "ready" && view.startsWith("list:") && !listIds.has(view.slice(5))) setPreferences({ view: "all" });
  }, [listIds, setPreferences, snapshot.status, view]);

  const notify = useCallback((message: string, action?: Notice["action"]) => {
    const id = ++noticeId.current;
    setNotices((current) => [...current.slice(-2), { id, message, action }]);
    window.setTimeout(() => setNotices((current) => current.filter((notice) => notice.id !== id)), action ? 6000 : 3200);
  }, []);

  useEffect(() => {
    if (snapshot.error) notify(`未保存：${snapshot.error}`);
  }, [notify, snapshot.error]);

  // Boot: Desktop state, pinned set, focus / quick-add requests.
  useEffect(() => {
    document.title = "Mangoo 便笺";
    if (!bridge) return;
    void bridge.ready().then((boot) => {
      if (boot.focusItemId) {
        setExpandedId(boot.focusItemId);
        setSelectedId(boot.focusItemId);
      }
      window.setTimeout(() => captureRef.current?.focus(), 50);
    });
    void bridge.listPinned().then(setPinned);
    void bridge.getSettings().then((settings) => setStickiesShown(!settings.stickiesHidden));
    const offSettings = bridge.onSettingsChanged((settings) => setStickiesShown(!settings.stickiesHidden));
    const offPinned = bridge.onPinnedChanged(setPinned);
    const offFocus = bridge.onFocusItem((id) => {
      setPreferences({ view: "all" });
      setExpandedId(id);
      setSelectedId(id);
    });
    const offQuick = bridge.onQuickAdd(() => {
      setExpandedId(null);
      captureRef.current?.focus();
      captureRef.current?.select();
    });
    return () => {
      offSettings();
      offPinned();
      offFocus();
      offQuick();
    };
  }, [bridge, setPreferences]);

  // Chip row: reachable with a normal wheel, fades only where it continues,
  // and the current view's chip is always scrolled into sight.
  useEffect(() => {
    const row = chipsRef.current;
    if (!row) return;
    const update = () => {
      row.dataset.overflowStart = row.scrollLeft > 1 ? "true" : "false";
      row.dataset.overflowEnd = row.scrollLeft + row.clientWidth < row.scrollWidth - 1 ? "true" : "false";
    };
    const onWheel = (event: WheelEvent) => {
      if (Math.abs(event.deltaY) <= Math.abs(event.deltaX) || row.scrollWidth <= row.clientWidth) return;
      event.preventDefault();
      row.scrollBy({ left: event.deltaY, behavior: "auto" });
    };
    update();
    row.addEventListener("scroll", update, { passive: true });
    row.addEventListener("wheel", onWheel, { passive: false });
    const observer = new ResizeObserver(update);
    observer.observe(row);
    row.querySelector<HTMLElement>('[data-selected="true"]')?.scrollIntoView({ block: "nearest", inline: "nearest" });
    return () => {
      row.removeEventListener("scroll", update);
      row.removeEventListener("wheel", onWheel);
      observer.disconnect();
    };
  });

  // Keep the open / selected card in view.
  useEffect(() => {
    const id = expandedId ?? selectedId;
    if (!id) return;
    streamRef.current?.querySelector<HTMLElement>(`[data-item-id="${id}"]`)?.scrollIntoView({ block: "nearest" });
  }, [expandedId, selectedId]);

  const itemsById = useMemo(() => new Map(snapshot.items.map((item) => [item.id, item])), [snapshot.items]);
  const liveItems = useMemo(() => snapshot.items.filter((item) => !item.deleted), [snapshot.items]);
  const sort: SortMode = preferences.sort[view] ?? DEFAULT_SORT[view as SmartView] ?? "manual";
  const bindings = useMemo(() => resolveBindings(preferences.shortcuts), [preferences.shortcuts]);
  const hint = (action: ShortcutActionId) => shortcutHint(bindings, action);
  const manual = sort === "manual" && !query.trim() && view !== "completed" && view !== "trash";

  const counts = useMemo(() => {
    const result: Record<string, number> = {};
    for (const smart of ["all", "today", "upcoming", "important"] as SmartView[]) {
      result[smart] = liveItems.filter((item) => !item.parentId && !item.done && itemInView(item, smart, today)).length;
    }
    for (const list of lists) result[`list:${list.id}`] = liveItems.filter((item) => !item.parentId && !item.done && item.listId === list.id).length;
    return result;
  }, [liveItems, lists, today]);

  const groups = useMemo<Group[]>(() => {
    const search = query.trim();
    if (search) {
      const found = snapshot.items.filter((item) => !item.deleted && matchesSearch(item, search)).sort((a, b) => compareItems(a, b, "due"));
      return [{ key: "search", title: found.length ? `“${search}” · ${found.length} 条` : null, items: found }];
    }
    const flat = view === "completed" || view === "trash";
    // While settling, an item is grouped / filtered by its previous done state.
    const items = settling.size ? snapshot.items.map((item) => (settling.has(item.id) ? { ...item, done: settling.get(item.id) as boolean, completedAt: item.completedAt } : item)) : snapshot.items;
    const real = new Map(snapshot.items.map((item) => [item.id, item]));
    const candidates = items.filter((item) => {
      if (view === "trash") return item.deleted && !(item.parentId && itemsById.get(item.parentId)?.deleted);
      if (!flat && item.parentId) return false;
      if (view === "completed") return item.done && !item.deleted;
      return itemInView(item, view, today);
    });
    const ordered = candidates.sort((a, b) => compareItems(a, b, sort)).map((item) => (settling.has(item.id) ? real.get(item.id) ?? item : item));
    if (view === "today") {
      const overdue = ordered.filter((item) => dueState(item, today) === "overdue");
      const rest = ordered.filter((item) => dueState(item, today) !== "overdue");
      return [
        ...(overdue.length ? [{ key: "overdue", title: "已逾期", tone: "overdue" as const, items: overdue }] : []),
        { key: "today", title: overdue.length ? "今天" : null, items: rest }
      ];
    }
    if (view === "upcoming") {
      const byDate = new Map<string, TodoItem[]>();
      for (const item of ordered) {
        const key = item.dueDate && item.dueDate < today ? "overdue" : item.dueDate ?? "none";
        byDate.set(key, [...(byDate.get(key) ?? []), item]);
      }
      return [...byDate.entries()]
        .sort((a, b) => (a[0] === "overdue" ? -1 : b[0] === "overdue" ? 1 : a[0] < b[0] ? -1 : 1))
        .map(([key, entries]) => ({
          key,
          title: key === "overdue" ? "已逾期" : formatDueLabel(key, null, today),
          tone: key === "overdue" ? ("overdue" as const) : undefined,
          items: entries
        }));
    }
    if (viewShowsCompletedGroup(view)) {
      const doneOf = (item: TodoItem) => (settling.has(item.id) ? settling.get(item.id) as boolean : item.done);
      const open = ordered.filter((item) => !doneOf(item));
      const done = ordered.filter((item) => doneOf(item)).sort((a, b) => (b.completedAt ?? 0) - (a.completedAt ?? 0));
      return [
        { key: "open", title: null, items: open },
        ...(done.length ? [{ key: "done", title: `已完成 ${done.length}`, items: done, collapsible: true }] : [])
      ];
    }
    return [{ key: "all", title: null, items: ordered }];
  }, [itemsById, query, settling, snapshot.items, sort, today, view]);

  const visibleIds = useMemo(
    () => groups.flatMap((group) => (group.collapsible && !preferences.showCompleted ? [] : group.items.map((item) => item.id))),
    [groups, preferences.showCompleted]
  );

  // ----------------------------------------------------------- actions

  const viewDefaults = (): Partial<TodoItem> => {
    if (view.startsWith("list:")) return { listId: view.slice(5) };
    if (view.startsWith("tag:")) return { tags: [view.slice(4)] };
    if (view === "today") return { dueDate: today };
    if (view === "upcoming") return { dueDate: addDays(today, 1) };
    if (view === "important") return { priority: 3 };
    return {};
  };

  /** First line: title (+ parsed date / priority / tags); the rest: notes. */
  const addCapture = () => {
    const text = capture.trim();
    if (!text) return;
    const [firstLine, ...rest] = text.split(/\r?\n/);
    const parsed = parseQuickAdd(firstLine, today);
    const defaults = viewDefaults();
    const notes = rest.join("\n").trim();
    const created = store.createItem(
      {
        ...defaults,
        // The capture box is plain text: store it escaped (titles are inline HTML).
        title: escapeTitle((parsed.title || firstLine).slice(0, 500)),
        notes: notes ? textToHtml(notes) : "",
        dueDate: parsed.dueDate ?? defaults.dueDate ?? null,
        dueTime: parsed.dueTime,
        priority: parsed.priority || (defaults.priority as Priority | undefined) || 0,
        tags: [...new Set([...(defaults.tags ?? []), ...parsed.tags])]
      },
      { position: "start" }
    );
    setCapture("");
    if (view === "completed" || view === "trash") {
      setPreferences({ view: "all" });
      return;
    }
    // Saved somewhere this view does not show: say so instead of vanishing.
    if (!itemInView(created, view, today)) {
      notify(created.dueDate ? `已记下（${formatDueLabel(created.dueDate, created.dueTime, today)}）` : "已记下", {
        label: "查看",
        run: () => {
          setPreferences({ view: "all" });
          setExpandedId(created.id);
        }
      });
    }
  };

  const deleteWithUndo = useCallback((ids: string[]) => {
    const removed = store.deleteItems(ids);
    if (!removed.length) return;
    if (expandedId && removed.includes(expandedId)) setExpandedId(null);
    for (const id of removed) if (pinned.includes(id)) void bridge?.unpin(id);
    // Count what the user picked; subtasks go (and come back) with their note.
    const picked = ids.filter((id) => removed.includes(id)).length;
    notify(picked > 1 ? `已删除 ${picked} 条` : "已删除", { label: "撤销", run: () => store.restoreItems(removed) });
  }, [bridge, expandedId, notify, pinned, store]);

  const toggleDone = useCallback((id: string) => {
    const item = store.item(id);
    if (!item) return;
    const wasDone = item.done;
    if (!item.parentId) {
      setSettling((current) => new Map(current).set(id, current.get(id) ?? wasDone));
      if (settleTimer.current) window.clearTimeout(settleTimer.current);
      settleTimer.current = window.setTimeout(() => setSettling(new Map()), 1400);
    }
    const { rolledTo } = store.toggleDone(id);
    if (rolledTo) notify(`本次完成，下次：${formatDueLabel(rolledTo, item.dueTime, today)}`);
    else if (!wasDone && !item.parentId) notify("已完成", { label: "撤销", run: () => store.updateItem(id, { done: false, completedAt: null }) });
  }, [notify, store, today]);

  const togglePin = useCallback(async (id: string) => {
    if (!bridge) {
      notify("贴到桌面需要在桌面版中使用");
      return;
    }
    const result = pinned.includes(id) ? await bridge.unpin(id) : await bridge.pin(id);
    if (!result.ok && result.error) notify(result.error);
  }, [bridge, notify, pinned]);

  const selectView = (next: ViewKey) => {
    setPreferences({ view: next });
    setQuery("");
    setSearchOpen(false);
    setExpandedId(null);
  };

  const newListDraft = (): ListDraft => {
    const used = new Set(lists.map((list) => list.color.toLowerCase()));
    return { id: null, name: "", color: LIST_COLORS.find((color) => !used.has(color.toLowerCase())) ?? LIST_COLORS[lists.length % LIST_COLORS.length] };
  };

  // ------------------------------------------------------- drag reorder

  const onDragStart = (event: DragEvent, item: TodoItem) => {
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", titleText(item.title));
    setDrag({ id: item.id, over: null, position: "before" });
  };
  const onDragOver = (event: DragEvent, item: TodoItem) => {
    if (!drag || drag.id === item.id || !manual) return;
    event.preventDefault();
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const position = event.clientY < rect.top + rect.height / 2 ? "before" : "after";
    if (drag.over !== item.id || drag.position !== position) setDrag({ ...drag, over: item.id, position });
  };
  const onDrop = (event: DragEvent, target: TodoItem) => {
    event.preventDefault();
    if (!drag || !manual) return;
    const dragged = itemsById.get(drag.id);
    setDrag(null);
    if (!dragged || dragged.id === target.id) return;
    const siblings = groups.flatMap((group) => group.items).filter((item) => item.id !== dragged.id).sort(compareOrder);
    const index = siblings.findIndex((item) => item.id === target.id);
    const before = drag.position === "before" ? siblings[index - 1] ?? null : target;
    const after = drag.position === "before" ? target : siblings[index + 1] ?? null;
    store.reorder(dragged.id, before, after);
  };

  // ---------------------------------------------------------- keyboard

  const focusSearch = useCallback(() => {
    setSearchOpen(true);
    window.setTimeout(() => searchRef.current?.focus(), 0);
  }, []);

  /**
   * Keyboard. Esc / Enter / Tab are fixed; everything else goes through the
   * user's (remappable) bindings. Note actions act on the selected card and
   * never fire while typing.
   */
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing) return;
      const target = event.target as HTMLElement | null;
      const typing = Boolean(target?.closest("input, textarea, select, [contenteditable='true']"));
      // A floating popover (property editors, menus) owns its keys and its Esc.
      if (target?.closest(".overlay-surface, [data-overlay-root], .context-menu")) return;
      if (event.key === "Escape") {
        if (lightbox) {
          setLightbox(null);
          return;
        }
        if (menu) return;
        if (typing) {
          if (target === searchRef.current) {
            setQuery("");
            setSearchOpen(false);
          }
          target?.blur();
          return;
        }
        if (expandedId) setExpandedId(null);
        else if (selectedId) setSelectedId(null);
        else if (searchOpen) {
          setQuery("");
          setSearchOpen(false);
        }
        return;
      }
      if (menu || lightbox) return;
      const onControl = Boolean(target?.closest("button, a, summary"));
      const selected = selectedId ? itemsById.get(selectedId) : undefined;
      if (event.key === "Enter" && !typing && !onControl && !expandedId && selected) {
        event.preventDefault();
        setExpandedId(selected.id);
        return;
      }
      const action = matchAction(bindings, event);
      if (!action) return;
      const modified = event.ctrlKey || event.metaKey || event.altKey;
      // Plain keys type text in fields; only chords with Ctrl / Alt reach the window there.
      if (typing && !modified) return;
      if (SELECTION_ACTIONS.has(action)) {
        if (typing || expandedId) return;
        if (action === "selectNext" || action === "selectPrevious") {
          event.preventDefault();
          const index = selectedId ? visibleIds.indexOf(selectedId) : -1;
          const next = visibleIds[Math.min(visibleIds.length - 1, Math.max(0, index + (action === "selectNext" ? 1 : -1)))];
          if (next) setSelectedId(next);
          return;
        }
        if (!selected) return;
        if (onControl && (action === "toggleDone" || action === "edit")) return;
        event.preventDefault();
        const neighbour = () => {
          const index = visibleIds.indexOf(selected.id);
          return visibleIds[index + 1] ?? visibleIds[index - 1] ?? null;
        };
        if (action === "delete") {
          const next = neighbour();
          if (selected.deleted) store.purgeItems([selected.id]);
          else deleteWithUndo([selected.id]);
          setSelectedId(next);
          return;
        }
        if (selected.deleted) return;
        switch (action) {
          case "edit": setExpandedId(selected.id); break;
          case "toggleDone": toggleDone(selected.id); break; // the card stays put while settling, so the selection does too
          case "togglePin": void togglePin(selected.id); break;
          case "dueToday": store.updateItem(selected.id, { dueDate: today }); break;
          case "dueTomorrow": store.updateItem(selected.id, { dueDate: addDays(today, 1) }); break;
          case "clearDue": store.updateItem(selected.id, { dueDate: null, dueTime: null, recurrence: null }); break;
          case "priorityHigh": store.updateItem(selected.id, { priority: 3 }); break;
          case "priorityMedium": store.updateItem(selected.id, { priority: 2 }); break;
          case "priorityLow": store.updateItem(selected.id, { priority: 1 }); break;
          case "priorityNone": store.updateItem(selected.id, { priority: 0 }); break;
          default: break;
        }
        return;
      }
      event.preventDefault();
      switch (action) {
        case "newNote":
          setExpandedId(null);
          if (view === "trash" || view === "completed") setPreferences({ view: "all" });
          setSearchOpen(false);
          setQuery("");
          window.setTimeout(() => captureRef.current?.focus(), 0);
          break;
        case "search": focusSearch(); break;
        case "toggleCompact": setPreferences({ compact: !preferences.compact }); break;
        case "toggleOnTop": void bridge?.setAlwaysOnTop(!onTop).then(setOnTop); break;
        case "openSettings": void bridge?.openSettings("capture"); break;
        case "closeWindow": if (controls) void controls.close(); else window.close(); break;
        default: break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [bindings, bridge, controls, deleteWithUndo, expandedId, focusSearch, itemsById, lightbox, menu, onTop, preferences.compact, searchOpen, selectedId, setPreferences, store, today, toggleDone, togglePin, view, visibleIds]);

  /** Clicking the empty stream collapses the open card. */
  const onStreamClick = (event: ReactMouseEvent) => {
    const target = event.target as HTMLElement;
    // React bubbles clicks from portaled popovers (property editors) through
    // the card tree: only a click on the stream's own empty space collapses.
    if (!event.currentTarget.contains(target)) return;
    if (!target.closest(".todo-card")) setExpandedId(null);
  };

  // ------------------------------------------------------------- menus

  const closeMenu = () => setMenu(null);
  const menuItem = menu?.kind === "item" && menu.id ? itemsById.get(menu.id) : undefined;
  const menuList = menu?.kind === "list" && menu.id ? lists.find((list) => list.id === menu.id) : undefined;

  /**
   * Item menu, in the order people act: act on it · when · how important ·
   * where · copy · delete (last, apart). Lists are a submenu-like second
   * level so the first level stays short.
   */
  const itemMenu = (item: TodoItem) => {
    if (item.deleted) {
      return (
        <>
          <OverlayMenuItem icon={<icons.RotateCcw size={14} />} onClick={() => { store.restoreItems([item.id]); closeMenu(); }} role="menuitem">恢复</OverlayMenuItem>
          <div className="overlay-menu-separator" role="separator" />
          <OverlayMenuItem danger icon={<icons.Trash2 size={14} />} onClick={() => { store.purgeItems([item.id]); closeMenu(); }} role="menuitem">永久删除</OverlayMenuItem>
        </>
      );
    }
    return (
      <>
        <OverlayMenuItem icon={<icons.CheckCircle2 size={14} />} onClick={() => { toggleDone(item.id); closeMenu(); }} role="menuitem" shortcut={hint("toggleDone")}>{item.done ? "标记为未完成" : "完成"}</OverlayMenuItem>
        <OverlayMenuItem icon={<icons.Pencil size={14} />} onClick={() => { setExpandedId(item.id); closeMenu(); }} role="menuitem" shortcut={hint("edit") ? `Enter / ${hint("edit")}` : "Enter"}>编辑</OverlayMenuItem>
        <OverlayMenuItem icon={pinned.includes(item.id) ? <icons.PinOff size={14} /> : <icons.Pin size={14} />} onClick={() => { void togglePin(item.id); closeMenu(); }} role="menuitem" shortcut={hint("togglePin")}>
          {pinned.includes(item.id) ? "从桌面取下" : "贴到桌面"}
        </OverlayMenuItem>
        <div className="overlay-menu-separator" role="separator" />
        <div aria-label="日期" className="todo-menu-row" role="group">
          <icons.Calendar aria-hidden="true" className="todo-menu-row-icon" size={14} />
          <button className="todo-menu-chip" data-selected={item.dueDate === today ? "true" : "false"} onClick={() => { store.updateItem(item.id, { dueDate: today }); closeMenu(); }} title={hint("dueToday") ? `今天（${hint("dueToday")}）` : "今天"} type="button">今天</button>
          <button className="todo-menu-chip" data-selected={item.dueDate === addDays(today, 1) ? "true" : "false"} onClick={() => { store.updateItem(item.id, { dueDate: addDays(today, 1) }); closeMenu(); }} title={hint("dueTomorrow") ? `明天（${hint("dueTomorrow")}）` : "明天"} type="button">明天</button>
          <button className="todo-menu-chip" onClick={() => { store.updateItem(item.id, { dueDate: addDays(today, 7) }); closeMenu(); }} type="button">下周</button>
          {item.dueDate ? <button aria-label="清除日期" className="todo-menu-chip" onClick={() => { store.updateItem(item.id, { dueDate: null, dueTime: null, recurrence: null }); closeMenu(); }} title="清除日期" type="button"><icons.X aria-hidden="true" size={12} /></button> : null}
        </div>
        <div aria-label="优先级" className="todo-menu-row" role="radiogroup">
          <icons.Flag aria-hidden="true" className="todo-menu-row-icon" size={14} />
          {([3, 2, 1, 0] as Priority[]).map((priority) => (
            <button aria-checked={item.priority === priority} className="todo-menu-chip todo-menu-chip-priority" data-selected={item.priority === priority ? "true" : "false"} key={priority} onClick={() => { store.updateItem(item.id, { priority }); closeMenu(); }} role="radio" title={(() => {
                const name = priority ? `${PRIORITY_LABELS[priority]}优先级` : "无优先级";
                const key = hint((["priorityNone", "priorityLow", "priorityMedium", "priorityHigh"] as const)[priority]);
                return key ? `${name}（${key}）` : name;
              })()} type="button">
              <span className="todo-flag" data-priority={priority}><icons.Flag aria-hidden="true" size={13} weight={priority ? "fill" : "regular"} /></span>
              {priority ? PRIORITY_LABELS[priority] : null}
            </button>
          ))}
        </div>
        <div className="overlay-menu-separator" role="separator" />
        {!item.parentId ? (
          <>
            <div className="todo-menu-label" role="presentation">移到清单</div>
            <OverlayMenuItem checked={item.listId === null} icon={<icons.Inbox size={14} />} onClick={() => { store.moveToList(item.id, null); closeMenu(); }} role="menuitemradio" shortcut={<Current on={item.listId === null} />}>无清单</OverlayMenuItem>
            {lists.map((list) => (
              <OverlayMenuItem checked={item.listId === list.id} icon={<ColorDot color={list.color} />} key={list.id} onClick={() => { store.moveToList(item.id, list.id); notify(`已移到“${list.name}”`); closeMenu(); }} role="menuitemradio" shortcut={<Current on={item.listId === list.id} />}>
                {list.name}
              </OverlayMenuItem>
            ))}
            <div className="overlay-menu-separator" role="separator" />
          </>
        ) : null}
        <OverlayMenuItem icon={<icons.Copy size={14} />} onClick={() => { void bridge?.copyText(titleText(item.title)); closeMenu(); }} role="menuitem">复制标题</OverlayMenuItem>
        <div className="overlay-menu-separator" role="separator" />
        <OverlayMenuItem danger icon={<icons.Trash2 size={14} />} onClick={() => { deleteWithUndo([item.id]); closeMenu(); }} role="menuitem" shortcut={hint("delete")}>删除</OverlayMenuItem>
      </>
    );
  };

  const listMenu = (list: TodoList) => (
    <>
      <OverlayMenuItem icon={<icons.Pencil size={14} />} onClick={() => { setEditingList({ id: list.id, name: list.name, color: list.color }); closeMenu(); }} role="menuitem">编辑清单</OverlayMenuItem>
      <div className="overlay-menu-separator" role="separator" />
      <OverlayMenuItem
        danger
        icon={<icons.Trash2 size={14} />}
        onClick={() => {
          const removed = store.deleteList(list.id);
          if (view === `list:${list.id}`) setPreferences({ view: "all" });
          if (removed) {
            notify(removed.items.length ? `已删除清单和其中 ${removed.items.length} 条便笺` : "已删除清单", {
              label: "撤销",
              run: () => store.restoreList(removed)
            });
          }
          closeMenu();
        }}
        role="menuitem"
      >
        删除清单
      </OverlayMenuItem>
    </>
  );

  /** Header view menu: where you are going. Smart views, lists, archive. */
  const viewMenu = (
    <>
      {(["all", "today", "upcoming", "important"] as SmartView[]).map((smart) => {
        const Icon = SMART_ICONS[smart];
        return (
          <OverlayMenuItem checked={view === smart} icon={<Icon size={14} />} key={smart} onClick={() => { selectView(smart); closeMenu(); }} role="menuitemradio" shortcut={counts[smart] ? <span className="todo-menu-count">{counts[smart]}</span> : undefined}>
            {smart === "all" ? "全部" : SMART_VIEW_LABELS[smart]}
          </OverlayMenuItem>
        );
      })}
      <div className="todo-menu-label" role="presentation">清单</div>
      {lists.map((list) => (
        <OverlayMenuItem checked={view === `list:${list.id}`} icon={<ColorDot color={list.color} />} key={list.id} onClick={() => { selectView(`list:${list.id}`); closeMenu(); }} role="menuitemradio" shortcut={counts[`list:${list.id}`] ? <span className="todo-menu-count">{counts[`list:${list.id}`]}</span> : undefined}>
          {list.name}
        </OverlayMenuItem>
      ))}
      <OverlayMenuItem icon={<icons.Plus size={14} />} onClick={() => { setEditingList(newListDraft()); closeMenu(); }} role="menuitem">新建清单…</OverlayMenuItem>
      <div className="overlay-menu-separator" role="separator" />
      <OverlayMenuItem checked={view === "completed"} icon={<icons.CheckCircle2 size={14} />} onClick={() => { selectView("completed"); closeMenu(); }} role="menuitemradio">已完成</OverlayMenuItem>
      <OverlayMenuItem checked={view === "trash"} icon={<icons.Trash2 size={14} />} onClick={() => { selectView("trash"); closeMenu(); }} role="menuitemradio">最近删除</OverlayMenuItem>
    </>
  );

  /** Sort button: how this view is ordered (remembered per view). */
  const sortMenu = (
    <>
      <div className="todo-menu-label" role="presentation">排序（{viewTitle(view, lists)}）</div>
      {(Object.keys(SORT_LABELS) as SortMode[]).map((mode) => (
        <OverlayMenuItem checked={sort === mode} key={mode} onClick={() => { setPreferences({ sort: { ...preferences.sort, [view]: mode } }); closeMenu(); }} role="menuitemradio" shortcut={<Current on={sort === mode} />}>{SORT_LABELS[mode]}</OverlayMenuItem>
      ))}
    </>
  );

  /** ⋯: how the window looks · the desktop · the app. */
  const moreMenu = (
    <>
      {/* An action named after what it switches TO (no check mark). */}
      <OverlayMenuItem icon={<icons.LayoutList size={14} />} onClick={() => { setPreferences({ compact: !preferences.compact }); closeMenu(); }} role="menuitem" shortcut={hint("toggleCompact")}>{preferences.compact ? "标准显示" : "紧凑显示"}</OverlayMenuItem>
      <OverlayMenuItem checked={onTop} icon={<icons.StackTop size={14} />} onClick={() => { void bridge?.setAlwaysOnTop(!onTop).then(setOnTop); closeMenu(); }} role="menuitemcheckbox" shortcut={<>{hint("toggleOnTop")}<Current on={onTop} /></>}>窗口置顶</OverlayMenuItem>
      <OverlayMenuItem checked={stickiesShown} icon={<icons.Pin size={14} />} onClick={() => { void bridge?.setStickiesVisible(!stickiesShown); setStickiesShown(!stickiesShown); closeMenu(); }} role="menuitemcheckbox" shortcut={<Current on={stickiesShown} />}>
        显示桌面便笺
      </OverlayMenuItem>
      {view === "trash" && groups[0]?.items.length ? (
        <>
          <div className="overlay-menu-separator" role="separator" />
          <OverlayMenuItem icon={<icons.RotateCcw size={14} />} onClick={() => { store.restoreItems(groups[0].items.map((item) => item.id)); closeMenu(); }} role="menuitem">全部恢复</OverlayMenuItem>
          <OverlayMenuItem danger icon={<icons.Trash2 size={14} />} onClick={() => { store.purgeItems(groups[0].items.map((item) => item.id)); closeMenu(); }} role="menuitem">清空最近删除</OverlayMenuItem>
        </>
      ) : null}
      <div className="overlay-menu-separator" role="separator" />
      <OverlayMenuItem icon={<icons.Settings size={14} />} onClick={() => { void bridge?.openSettings("capture"); closeMenu(); }} role="menuitem" shortcut={hint("openSettings")}>设置…</OverlayMenuItem>
      <OverlayMenuItem icon={<icons.ExternalLink size={14} />} onClick={() => { void bridge?.openMainWindow(); closeMenu(); }} role="menuitem">打开 Mangoo</OverlayMenuItem>
    </>
  );

  const menuContent = menuItem ? itemMenu(menuItem) : menuList ? listMenu(menuList) : menu?.kind === "view" ? viewMenu : menu?.kind === "more" ? moreMenu : menu?.kind === "sort" ? sortMenu : null;

  // ------------------------------------------------------------- body

  const body = (() => {
    if (snapshot.status === "loading") return <EmptyState icon={<icons.Loader2 size={24} />} title="正在载入…" />;
    if (snapshot.status === "disabled") return <EmptyState icon={<icons.Plug size={24} weight="light" />} title="便笺插件未启用">请在插件管理中启用 Mangoo 便笺。</EmptyState>;
    if (snapshot.status === "error") {
      return (
        <EmptyState icon={<icons.AlertTriangle size={24} weight="light" />} title="无法读取便笺">
          {snapshot.error}
          <button className="todo-button" onClick={() => void store.load()} type="button">重试</button>
        </EmptyState>
      );
    }
    const total = groups.reduce((sum, group) => sum + group.items.length, 0);
    if (total === 0) {
      const empty: Partial<Record<SmartView, [string, string]>> = {
        all: ["还没有便笺", "在上方输入，回车即可记下。"],
        today: ["今天没有待办", "享受这一刻。"],
        upcoming: ["没有计划中的事项", "输入“明天 15:00”之类的时间即可安排。"],
        important: ["没有高优先级事项", "输入“!!!”或按 3 标记为高优先级。"],
        completed: ["还没有完成的事项", ""],
        trash: ["最近删除为空", "删除的便笺会先放在这里。"]
      };
      const [title, text] = query.trim() ? ["没有找到", "换个关键词试试。"] : empty[view as SmartView] ?? ["这里还没有便笺", "在上方输入，回车即可记下。"];
      return <EmptyState icon={<icons.StickyNote size={24} weight="light" />} title={title}>{text}</EmptyState>;
    }
    return groups.map((group) => {
      if (group.items.length === 0 && group.title === null) return null;
      const hidden = group.collapsible && !preferences.showCompleted;
      return (
        <section className="todo-group" key={group.key}>
          {group.title ? (
            group.collapsible ? (
              <button className="todo-group-title" data-pointer-glow="off" onClick={() => setPreferences({ showCompleted: !preferences.showCompleted })} type="button">
                {hidden ? <icons.ChevronRight aria-hidden="true" size={12} /> : <icons.ChevronDown aria-hidden="true" size={12} />}
                {group.title}
              </button>
            ) : (
              <div className="todo-group-title" data-tone={group.tone}>{group.title}</div>
            )
          ) : null}
          {hidden ? null : group.items.map((item) => (
            <ItemCard
              draggable={!item.deleted && !query.trim()}
              dropMark={drag && drag.over === item.id ? drag.position : null}
              expanded={expandedId === item.id}
              item={item}
              items={snapshot.items}
              key={item.id}
              lists={lists}
              onDelete={(id) => deleteWithUndo([id])}
              onDragEnd={() => setDrag(null)}
              onDragOver={onDragOver}
              onDragStart={onDragStart}
              onDrop={onDrop}
              onExpand={(id) => {
                setExpandedId(id);
                if (id) setSelectedId(id);
              }}
              onMenu={(x, y, id) => {
                setSelectedId(id);
                setMenu({ x, y, kind: "item", id });
              }}
              onNotice={notify}
              onOpenImage={setLightbox}
              onPinToggle={(id) => void togglePin(id)}
              onToggleDone={toggleDone}
              pinned={pinned.includes(item.id)}
              selected={selectedId === item.id}
              showList={!view.startsWith("list:")}
              store={store}
              today={today}
            />
          ))}
        </section>
      );
    });
  })();

  const canCapture = snapshot.status === "ready" && view !== "trash" && !searchOpen;
  const parsedCapture = capture.trim() ? parseQuickAdd(capture.split(/\r?\n/)[0], today) : null;

  return (
    <LiquidGlassLayer>
      <LiquidGlassSurface as="div" className="todo-window" data-compact={preferences.compact ? "true" : "false"} tone="chrome">
        <header className="todo-header" data-search={searchOpen ? "true" : "false"}>
          {searchOpen ? (
            <label className="todo-search" data-pointer-glow="off">
              <icons.Search aria-hidden="true" size={13} />
              <input
                aria-label="搜索"
                onChange={(event) => setQuery(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    event.preventDefault();
                    event.stopPropagation();
                    setQuery("");
                    setSearchOpen(false);
                  }
                }}
                placeholder="搜索便笺"
                ref={searchRef}
                value={query}
              />
              <button aria-label="关闭搜索" className="todo-icon-button" onClick={() => { setQuery(""); setSearchOpen(false); }} title="关闭搜索（Esc）" type="button">
                <icons.X aria-hidden="true" size={12} />
              </button>
            </label>
          ) : (
            <>
              <button
                aria-haspopup="menu"
                className="todo-view-switch"
                data-pointer-glow="off"
                onClick={(event) => {
                  const rect = event.currentTarget.getBoundingClientRect();
                  setMenu({ x: rect.left, y: rect.bottom + 4, kind: "view", id: null });
                }}
                title="所有视图与清单"
                type="button"
              >
                <span className="todo-view-name">{viewTitle(view, lists)}</span>
                <icons.ChevronDown aria-hidden="true" size={12} />
              </button>
              <div className="todo-header-actions">
                {onTop ? (
                  <button aria-label="取消置顶" className="todo-icon-button" data-active="true" onClick={() => void bridge?.setAlwaysOnTop(false).then(setOnTop)} title="已置顶，点击取消" type="button">
                    <icons.StackTop aria-hidden="true" size={14} />
                  </button>
                ) : null}
                <button aria-label="搜索" className="todo-icon-button" onClick={focusSearch} title={hint("search") ? `搜索（${hint("search")}）` : "搜索"} type="button">
                  <icons.Search aria-hidden="true" size={14} />
                </button>
                {view !== "trash" ? (
                  <button
                    aria-haspopup="menu"
                    aria-label={`排序：${SORT_LABELS[sort]}`}
                    className="todo-icon-button"
                    data-active={sort !== (DEFAULT_SORT[view as SmartView] ?? "manual") ? "true" : "false"}
                    onClick={(event) => {
                      const rect = event.currentTarget.getBoundingClientRect();
                      setMenu({ x: rect.right, y: rect.bottom + 4, kind: "sort", id: null });
                    }}
                    title={`排序：${SORT_LABELS[sort]}`}
                    type="button"
                  >
                    <icons.ArrowDownUp aria-hidden="true" size={14} />
                  </button>
                ) : null}
                <button
                  aria-label="更多"
                  className="todo-icon-button"
                  onClick={(event) => {
                    const rect = event.currentTarget.getBoundingClientRect();
                    setMenu({ x: rect.right, y: rect.bottom + 4, kind: "more", id: null });
                  }}
                  title="更多"
                  type="button"
                >
                  <icons.MoreHorizontal aria-hidden="true" size={14} />
                </button>
              </div>
            </>
          )}
          {controls && !searchOpen ? (
            <button aria-label="关闭" className="todo-icon-button todo-close" onClick={() => void controls.close()} title="关闭" type="button">
              <icons.X aria-hidden="true" size={14} />
            </button>
          ) : null}
        </header>

        {canCapture ? (
          <div className="todo-capture" data-active={capture ? "true" : "false"}>
            <textarea
              aria-label="记点什么"
              onChange={(event) => setCapture(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
                  event.preventDefault();
                  addCapture();
                } else if (event.key === "Escape") {
                  setCapture("");
                  event.currentTarget.blur();
                }
              }}
              placeholder="记点什么…  例：明天 15:00 开会 !高 #工作"
              ref={captureRef}
              rows={Math.min(6, Math.max(1, capture.split("\n").length))}
              value={capture}
            />
            {capture.trim() ? (
              <div className="todo-capture-hints">
                {parsedCapture?.dueDate ? (
                  <span className="todo-chip todo-due" data-state={dueState({ dueDate: parsedCapture.dueDate, dueTime: parsedCapture.dueTime, done: false }, today)}>
                    <icons.Calendar aria-hidden="true" size={11} />
                    {formatDueLabel(parsedCapture.dueDate, parsedCapture.dueTime, today)}
                  </span>
                ) : null}
                {parsedCapture?.priority ? <PriorityFlag priority={parsedCapture.priority} /> : null}
                {parsedCapture?.tags.map((tag) => <span className="todo-chip todo-chip-tag" key={tag}>#{tag}</span>)}
                <kbd className="todo-capture-key" title="回车保存，Shift+回车换行写备注">↵</kbd>
              </div>
            ) : null}
          </div>
        ) : null}

        {query.trim() ? null : (
          <div className="todo-chips-row">
          <nav aria-label="视图" className="todo-chips" ref={chipsRef}>
            {CHIP_VIEWS.map((smart) => (
              <button aria-current={view === smart ? "page" : undefined} className="todo-view-chip" data-selected={view === smart ? "true" : "false"} data-view={smart} key={smart} onClick={() => selectView(smart)} type="button">
                {smart === "all" ? "全部" : SMART_VIEW_LABELS[smart]}
                {counts[smart] ? <span className="todo-chip-count">{counts[smart]}</span> : null}
              </button>
            ))}
            {lists.length ? <span aria-hidden="true" className="todo-chips-divider" /> : null}
            {lists.map((list) => (
              <button
                aria-current={view === `list:${list.id}` ? "page" : undefined}
                className="todo-view-chip"
                data-selected={view === `list:${list.id}` ? "true" : "false"}
                key={list.id}
                onClick={() => selectView(`list:${list.id}`)}
                onContextMenu={(event) => {
                  event.preventDefault();
                  setMenu({ x: event.clientX, y: event.clientY, kind: "list", id: list.id });
                }}
                onDoubleClick={() => setEditingList({ id: list.id, name: list.name, color: list.color })}
                onDragOver={(event) => {
                  if (drag) event.preventDefault();
                }}
                onDrop={(event) => {
                  event.preventDefault();
                  if (!drag) return;
                  store.moveToList(drag.id, list.id);
                  notify(`已移到“${list.name}”`);
                  setDrag(null);
                }}
                title="双击编辑 · 右键管理 · 可把便笺拖到这里"
                type="button"
              >
                <ColorDot color={list.color} />
                {list.name}
              </button>
            ))}

          </nav>
          <button aria-label="新建清单" className="todo-view-chip todo-view-chip-add" onClick={() => setEditingList(newListDraft())} title="新建清单" type="button">
              <icons.Plus aria-hidden="true" size={12} />
            </button>
          </div>
        )}

        <div className="todo-stream" onClick={onStreamClick} ref={streamRef}>
          {body}
        </div>
      </LiquidGlassSurface>

      <div aria-live="polite" className="todo-notices">
        {notices.map((notice) => (
          <div className="todo-notice" key={notice.id}>
            <span>{notice.message}</span>
            {notice.action ? (
              <button className="todo-link-button" onClick={() => { notice.action?.run(); setNotices((current) => current.filter((entry) => entry.id !== notice.id)); }} type="button">
                {notice.action.label}
              </button>
            ) : null}
          </div>
        ))}
      </div>

      {lightbox ? (
        <div className="todo-lightbox" onClick={() => setLightbox(null)} role="dialog">
          <img alt="" src={lightbox} />
        </div>
      ) : null}

      {menu && menuContent ? (
        <AnchoredOverlay
          anchor={pointOverlayAnchor(menu.x, menu.y)}
          ariaLabel="操作"
          className="context-menu todo-menu"
          id="todo-notes-menu"
          onDismiss={closeMenu}
          open
          fitToViewport
          placement={menu.kind === "more" || menu.kind === "sort" ? "bottom-end" : "bottom-start"}
          preferredWidth={menu.kind === "item" ? 256 : 208}
          role="group"
          rootId="context-menu"
        >
          <OverlayMenuList ariaLabel="操作">{menuContent}</OverlayMenuList>
        </AnchoredOverlay>
      ) : null}

      <ModalSurface ariaLabelledBy="todo-list-dialog-title" className="file-operation-dialog todo-dialog" id="todo-list-dialog" onDismiss={() => setEditingList(null)} open={editingList !== null} rootId="file-operation">
        <OverlayDialogHeader title={editingList?.id ? "编辑清单" : "新建清单"} titleId="todo-list-dialog-title" />
        <OverlayDialogBody>
          {editingList ? (
            <form
              className="todo-list-form"
              onSubmit={(event) => {
                event.preventDefault();
                const name = editingList.name.trim().slice(0, 60);
                if (!name) return;
                if (editingList.id) store.updateList(editingList.id, { name, color: editingList.color });
                else selectView(`list:${store.createList(name, editingList.color).id}`);
                setEditingList(null);
              }}
            >
              <input aria-label="清单名称" autoFocus className="todo-input" maxLength={60} onChange={(event) => setEditingList({ ...editingList, name: event.target.value })} placeholder="清单名称" value={editingList.name} />
              <div aria-label="颜色" className="todo-color-picker" role="radiogroup">
                {LIST_COLORS.map((color, index) => (
                  <button aria-checked={editingList.color === color} aria-label={LIST_COLOR_NAMES[index] ?? color} title={LIST_COLOR_NAMES[index]} className="todo-color-swatch" data-selected={editingList.color === color ? "true" : "false"} key={color} onClick={(event) => {
                    setEditingList({ ...editingList, color });
                    (event.currentTarget.closest("form")?.querySelector("input") as HTMLInputElement | null)?.focus();
                  }} role="radio" style={{ background: color }} type="button" />
                ))}
              </div>
              <div className="todo-dialog-actions">
                <button className="todo-button" onClick={() => setEditingList(null)} type="button">取消</button>
                <button className="todo-button todo-button-primary" disabled={!editingList.name.trim()} type="submit">{editingList.id ? "保存" : "创建"}</button>
              </div>
            </form>
          ) : null}
        </OverlayDialogBody>
      </ModalSurface>

    </LiquidGlassLayer>
  );
}
