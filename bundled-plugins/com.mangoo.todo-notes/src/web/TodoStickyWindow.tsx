import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { AnchoredOverlay, LiquidGlassLayer, LiquidGlassSurface, getWindowControls, icons, pointOverlayAnchor, useSecondaryWindowInterfaceStyle } from "@mangoo/web/plugin-host";

import { DEFAULT_STICKY_APPEARANCE, STICKY_COLORS, type StickyAppearance, type StickyColor } from "../shared/ipc.ts";
import { childrenOf, progressOf, stripHtml, titleText, toDateKey, type TodoItem } from "../shared/model.ts";
import { stickyItemId, todoBridge } from "./bridge.ts";
import { Checkbox, DueChip, PriorityFlag } from "./components.tsx";
import { TitleView } from "./InlineRichEditor.tsx";
import { NotesView } from "./RichTextEditor.tsx";
import { TodoStore, useTodoSnapshot } from "./store.ts";
import "./todo.css";

const COLOR_LABELS: Record<StickyColor, string> = { glass: "磨砂", yellow: "黄色", green: "绿色", blue: "蓝色", pink: "粉色", purple: "紫色" };
const EXPAND_DELAY_MS = 160;
/** Height of the expanded note's action footer (matches .todo-sticky-footer). */
const STICKY_FOOTER_PX = 34;
const COLLAPSE_DELAY_MS = 420;

/**
 * One item pinned to the desktop: a small always-on-top window. Collapsed it
 * shows only a summary line (checkbox, title, date, subtask progress); while
 * the pointer rests on it the window grows to show notes, subtasks and
 * attachments, and it shrinks back when the pointer leaves. The Desktop host
 * resizes the OS window; the renderer reports its summary height.
 */
export function TodoStickyWindow() {
  useSecondaryWindowInterfaceStyle();
  const store = useMemo(() => new TodoStore(), []);
  useEffect(() => {
    store.start();
    return () => store.stop();
  }, [store]);
  const snapshot = useTodoSnapshot(store);
  const bridge = todoBridge();
  const itemId = stickyItemId();
  const [appearance, setAppearance] = useState<StickyAppearance>(DEFAULT_STICKY_APPEARANCE);
  const [hoverExpanded, setHoverExpanded] = useState(false);
  const [expandOnHover, setExpandOnHover] = useState(true);
  /** Colour popover anchored at the palette button (floats; takes no space). */
  const [palette, setPalette] = useState<{ x: number; y: number } | null>(null);
  const paletteOpen = palette !== null;
  const summaryRef = useRef<HTMLDivElement | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const today = toDateKey(new Date());

  const item: TodoItem | undefined = itemId ? snapshot.items.find((entry) => entry.id === itemId) : undefined;
  const expanded = appearance.expanded || hoverExpanded;

  useEffect(() => {
    if (!bridge) return;
    void bridge.ready().then((boot) => {
      if (boot.appearance) setAppearance(boot.appearance);
      setExpandOnHover(boot.settings.expandOnHover);
    });
    const offState = bridge.onStickyStateChanged(setAppearance);
    const offSettings = bridge.onSettingsChanged((settings) => setExpandOnHover(settings.expandOnHover));
    return () => {
      offState();
      offSettings();
    };
  }, [bridge]);

  useEffect(() => {
    const text = titleText(item?.title ?? "");
    document.title = text ? `便笺 · ${text}` : "便笺";
  }, [item?.title]);

  // Report the collapsed summary's natural height so the window hugs it.
  useLayoutEffect(() => {
    const element = summaryRef.current;
    if (!element || !bridge) return;
    const report = () => {
      const root = element.parentElement;
      const details = root?.querySelector<HTMLElement>(".todo-sticky-details");
      const summary = element.getBoundingClientRect().height;
      // The details box stretches to the window (flex: 1); measure what is in it.
      let natural = 0;
      if (details) {
        const style = getComputedStyle(details);
        const gap = parseFloat(style.rowGap) || 0;
        const children = [...details.children] as HTMLElement[];
        natural = children.reduce((sum, child) => {
          const margins = getComputedStyle(child);
          return sum + child.getBoundingClientRect().height + (parseFloat(margins.marginTop) || 0) + (parseFloat(margins.marginBottom) || 0);
        }, 0)
          + gap * Math.max(0, children.length - 1)
          + (parseFloat(style.paddingTop) || 0) + (parseFloat(style.paddingBottom) || 0) + 1;
      }
      // Expanding adds the footer (STICKY_FOOTER_PX, see CSS) whether or not it
      // is laid out yet, so count it up front instead of reading a 0 height.
      const content = summary + natural + STICKY_FOOTER_PX + 4;
      void bridge.stickyMeasure(summary + 2, content);
    };
    report();
    const observer = new ResizeObserver(report);
    observer.observe(element);
    const root = element.parentElement;
    const mutations = new MutationObserver(() => window.requestAnimationFrame(report));
    if (root) mutations.observe(root, { childList: true, subtree: true, characterData: true });
    return () => {
      observer.disconnect();
      mutations.disconnect();
    };
  }, [bridge, item?.id]);

  const setHover = (next: boolean) => {
    if (timer.current) clearTimeout(timer.current);
    if (!expandOnHover || appearance.expanded) return;
    timer.current = setTimeout(() => {
      timer.current = null;
      if (paletteOpen && !next) return;
      setHoverExpanded(next);
      void bridge?.stickyExpand(next);
    }, next ? EXPAND_DELAY_MS : COLLAPSE_DELAY_MS);
  };

  const update = (patch: Partial<StickyAppearance>) => {
    setAppearance((current) => ({ ...current, ...patch }));
    void bridge?.stickyUpdate(patch).then(setAppearance);
  };

  // Pointer-driven window drag. A CSS drag region (-webkit-app-region) would
  // swallow the pointer events on Windows, and with them hover-to-expand.
  const controls = getWindowControls();
  const dragging = useRef(false);
  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || !controls) return;
    if ((event.target as HTMLElement).closest("button, input, a, label")) return;
    dragging.current = true;
    event.currentTarget.setPointerCapture(event.pointerId);
    void controls.beginDrag({ clientX: event.clientX, screenX: event.screenX, screenY: event.screenY, titlebarOffsetY: event.clientY, viewportWidth: window.innerWidth });
  };
  const onPointerMove = () => {
    if (dragging.current) void controls?.moveDrag();
  };
  const onPointerUp = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!dragging.current) return;
    dragging.current = false;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    void controls?.endDrag();
  };

  const subtasks = item ? childrenOf(snapshot.items, item.id) : [];
  // Nothing to reveal: expanding only shows the action row.
  const hasDetails = Boolean(item && (subtasks.length || stripHtml(item.notes) || item.notes.includes("<img")));
  const progress = item ? progressOf(snapshot.items, item.id) : { done: 0, total: 0 };

  return (
    <LiquidGlassLayer>
      <LiquidGlassSurface
        as="div"
        className="todo-sticky"
        data-color={appearance.color}
        data-done={item?.done ? "true" : "false"}
        data-expanded={expanded ? "true" : "false"}
        data-priority={item?.priority ?? 0}
        onMouseEnter={() => setHover(true)}
        onMouseLeave={() => setHover(false)}
        tone="chrome"
      >
        <div
          className="todo-sticky-summary"
          onDoubleClick={(event) => {
            if (item && !(event.target as HTMLElement).closest("button")) void bridge?.openItem(item.id);
          }}
          onPointerCancel={onPointerUp}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          ref={summaryRef}
          title="拖动以移动，双击在便笺中打开"
        >
          {item ? (
            <Checkbox checked={item.done} label={titleText(item.title) || "事项"} onToggle={() => store.toggleDone(item.id)} priority={item.priority} />
          ) : (
            <icons.StickyNote aria-hidden="true" size={14} />
          )}
          <div className="todo-sticky-heading">
            <div className="todo-sticky-title" title={titleText(item?.title ?? "")}>
              {item?.priority === 3 ? <span className="todo-card-flag"><PriorityFlag priority={item.priority} /></span> : null}
              {snapshot.status === "loading" ? "正在载入…" : item ? <TitleView html={item.title} placeholder="未命名事项" /> : snapshot.status === "ready" ? "事项已不存在" : "无法读取事项"}
            </div>
            {item && (item.dueDate || progress.total || item.remindAt) ? (
              <div className="todo-row-meta">
                <DueChip item={item} today={today} />
                {progress.total ? (
                  <span className="todo-chip">
                    <icons.CheckSquare aria-hidden="true" size={11} />
                    {progress.done}/{progress.total}
                  </span>
                ) : null}
              </div>
            ) : null}
          </div>
        </div>


        {item && expanded && hasDetails ? (
          <div className="todo-sticky-details">
            {subtasks.length ? (
              <ul className="todo-sticky-subtasks">
                {subtasks.map((subtask) => (
                  <li data-done={subtask.done ? "true" : "false"} key={subtask.id}>
                    <Checkbox checked={subtask.done} label={titleText(subtask.title) || "子事项"} onToggle={() => store.toggleDone(subtask.id)} priority={subtask.priority} size="small" />
                    <TitleView html={subtask.title} placeholder="未命名子事项" />
                  </li>
                ))}
              </ul>
            ) : null}
            <NotesView className="todo-sticky-notes" html={item.notes} store={store} />
          </div>
        ) : null}
        {/* Footer, not header: expanding grows the note downward and the summary
            never moves. Inert while collapsed so hidden controls take no focus. */}
        <div
          className="todo-sticky-actions todo-sticky-footer"
          ref={(element) => {
            if (element) element.inert = !expanded;
          }}
        >
          <button
            aria-label={appearance.expanded ? "自动收起" : "保持展开"}
            aria-pressed={appearance.expanded}
            className="todo-icon-button"
            data-active={appearance.expanded ? "true" : "false"}
            onClick={() => update({ expanded: !appearance.expanded })}
            title={appearance.expanded ? "自动收起" : "保持展开"}
            type="button"
          >
            {appearance.expanded ? <icons.Minimize2 aria-hidden="true" size={13} /> : <icons.Maximize2 aria-hidden="true" size={13} />}
          </button>
          <button
            aria-expanded={paletteOpen}
            aria-haspopup="dialog"
            aria-label="颜色"
            className="todo-icon-button"
            data-active={paletteOpen ? "true" : "false"}
            onClick={(event) => {
              const rect = event.currentTarget.getBoundingClientRect();
              setPalette(paletteOpen ? null : { x: rect.left, y: rect.top - 6 });
            }}
            title="颜色"
            type="button"
          >
            <icons.Palette aria-hidden="true" size={13} />
          </button>
          {item ? (
            <button aria-label="在便笺中打开" className="todo-icon-button" onClick={() => void bridge?.openItem(item.id)} title="在便笺中打开" type="button">
              <icons.ExternalLink aria-hidden="true" size={13} />
            </button>
          ) : null}
          {itemId ? (
            <button aria-label="从桌面取下" className="todo-icon-button" onClick={() => void bridge?.unpin(itemId)} title="从桌面取下" type="button">
              <icons.X aria-hidden="true" size={13} />
            </button>
          ) : null}
        </div>
      </LiquidGlassSurface>
      {palette ? (
        <AnchoredOverlay
          anchor={pointOverlayAnchor(palette.x, palette.y)}
          ariaLabel="颜色"
          className="context-menu todo-menu todo-pop todo-sticky-palette"
          fitToViewport
          id="todo-sticky-palette"
          onDismiss={() => setPalette(null)}
          open
          placement="top-start"
          role="dialog"
          rootId="context-menu"
        >
          <div aria-label="颜色" className="todo-color-picker" role="radiogroup">
            {STICKY_COLORS.map((color) => (
              <button
                aria-checked={appearance.color === color}
                aria-label={COLOR_LABELS[color]}
                className="todo-color-swatch todo-sticky-swatch"
                data-color={color}
                data-selected={appearance.color === color ? "true" : "false"}
                key={color}
                onClick={() => update({ color })}
                role="radio"
                title={COLOR_LABELS[color]}
                type="button"
              />
            ))}
          </div>
        </AnchoredOverlay>
      ) : null}
    </LiquidGlassLayer>
  );
}
