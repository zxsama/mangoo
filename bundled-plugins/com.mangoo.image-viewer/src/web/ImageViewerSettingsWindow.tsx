import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  LiquidGlassLayer,
  LiquidGlassSurface,
  getWindowControls,
  icons,
  useOverlayScrollbars,
  useSecondaryWindowInterfaceStyle
} from "@mangoo/web/plugin-host";

import { IMAGE_VIEWER_SETTINGS_ROUTE, IMAGE_VIEWER_SETTINGS_SECTIONS, type ImageViewerSettingsSection } from "../shared/ipc.ts";
import { settingsBridge } from "./bridge.ts";
import { useViewerPreferences } from "./preferences.ts";
import { SETTINGS_SECTION_LABELS, ViewerSettingsForm } from "./ViewerSettings.tsx";
import "./viewer.css";

const SECTION_ICONS: Record<ImageViewerSettingsSection, typeof icons.Image> = {
  display: icons.Image,
  browse: icons.Images,
  window: icons.LayoutPanelTop,
  actions: icons.SlidersHorizontal,
  shortcuts: icons.Keyboard,
  system: icons.Settings
};

/** Pixels below the scroller's top edge where a section counts as "current". */
const SPY_OFFSET = 28;

function sectionFromHash(): ImageViewerSettingsSection {
  const tail = window.location.hash.slice(IMAGE_VIEWER_SETTINGS_ROUTE.length + 1);
  return (IMAGE_VIEWER_SETTINGS_SECTIONS as readonly string[]).includes(tail) ? (tail as ImageViewerSettingsSection) : "display";
}

/**
 * The viewer's settings as an independent OS window (frameless, acrylic):
 * it can be dragged anywhere, even off the viewer, and keeps the system blur.
 * Left: one label per section. Right: every setting in one scrolling column.
 * Scrolling highlights the label of the section in view; clicking a label
 * scrolls its section to the top. Preferences sync with the viewer through
 * localStorage `storage` events (useViewerPreferences).
 */
export function ImageViewerSettingsWindow() {
  useSecondaryWindowInterfaceStyle();
  useOverlayScrollbars();
  const [preferences, setPreferences] = useViewerPreferences();
  const [active, setActive] = useState<ImageViewerSettingsSection>(sectionFromHash);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  // While a label-initiated smooth scroll runs, the spy must not fight it.
  const spyLock = useRef<number | null>(null);
  const controls = getWindowControls();

  const sectionElement = (id: ImageViewerSettingsSection) =>
    scrollRef.current?.querySelector<HTMLElement>(`[data-section="${id}"]`) ?? null;

  const jumpTo = useCallback((id: ImageViewerSettingsSection, smooth = true) => {
    const scroller = scrollRef.current;
    const target = sectionElement(id);
    if (!scroller || !target) return;
    setActive(id);
    if (spyLock.current) window.clearTimeout(spyLock.current);
    spyLock.current = window.setTimeout(() => {
      spyLock.current = null;
    }, smooth ? 700 : 50);
    const top = target.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop - 12;
    scroller.scrollTo({ top: Math.max(0, top), behavior: smooth ? "smooth" : "auto" });
  }, []);

  // Open at the requested section.
  useLayoutEffect(() => {
    const initial = sectionFromHash();
    if (initial !== "display") jumpTo(initial, false);
  }, [jumpTo]);

  // The viewer asked again while this window was already open.
  useEffect(() => settingsBridge()?.onSettingsSection((section) => jumpTo(section)), [jumpTo]);

  // Scroll spy.
  useEffect(() => {
    const scroller = scrollRef.current;
    if (!scroller) return;
    const update = () => {
      if (spyLock.current) return;
      const atBottom = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2;
      const sections = [...scroller.querySelectorAll<HTMLElement>("[data-section]")];
      if (sections.length === 0) return;
      let current = sections[0].dataset.section as ImageViewerSettingsSection;
      if (atBottom) current = sections[sections.length - 1].dataset.section as ImageViewerSettingsSection;
      else {
        const edge = scroller.getBoundingClientRect().top + SPY_OFFSET;
        for (const section of sections) {
          if (section.getBoundingClientRect().top <= edge) current = section.dataset.section as ImageViewerSettingsSection;
        }
      }
      setActive((value) => (value === current ? value : current));
    };
    scroller.addEventListener("scroll", update, { passive: true });
    // A wheel/drag by the user ends a label-initiated scroll lock at once.
    const release = () => {
      if (spyLock.current) {
        window.clearTimeout(spyLock.current);
        spyLock.current = null;
      }
    };
    scroller.addEventListener("wheel", release, { passive: true });
    scroller.addEventListener("pointerdown", release);
    update();
    return () => {
      scroller.removeEventListener("scroll", update);
      scroller.removeEventListener("wheel", release);
      scroller.removeEventListener("pointerdown", release);
    };
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      // A shortcut being recorded swallows Esc itself (capture phase).
      const target = event.target as HTMLElement | null;
      if (target?.matches?.("input, textarea") && (target as HTMLInputElement).value) return;
      if (event.key === "Escape" && !event.defaultPrevented) window.close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    document.title = "看图设置";
  }, []);

  return (
    <LiquidGlassLayer>
      <LiquidGlassSurface as="div" className="iv-settings-window" tone="chrome">
        <header className="iv-settings-window-titlebar">
          <span className="iv-settings-window-title">
            <icons.Settings aria-hidden="true" size={14} />
            看图设置
          </span>
          <button
            aria-label="关闭"
            className="iv-settings-window-close no-drag"
            onClick={() => (controls ? void controls.close() : window.close())}
            title="关闭（Esc）"
            type="button"
          >
            <icons.X aria-hidden="true" size={15} />
          </button>
        </header>
        <div className="iv-settings-window-body">
          <nav aria-label="设置分类" className="iv-settings-nav">
            {IMAGE_VIEWER_SETTINGS_SECTIONS.map((id) => {
              const Icon = SECTION_ICONS[id];
              return (
                <button
                  aria-current={active === id ? "true" : undefined}
                  className="iv-settings-nav-item"
                  data-active={active === id ? "true" : "false"}
                  key={id}
                  onClick={() => jumpTo(id)}
                  type="button"
                >
                  <Icon aria-hidden="true" size={15} />
                  <span>{SETTINGS_SECTION_LABELS[id]}</span>
                </button>
              );
            })}
            <p className="iv-settings-nav-footnote">更改会立即生效</p>
          </nav>
          <div className="iv-settings-scroll" ref={scrollRef}>
            <ViewerSettingsForm onChange={setPreferences} preferences={preferences} showShortcuts />
          </div>
        </div>
      </LiquidGlassSurface>
    </LiquidGlassLayer>
  );
}
