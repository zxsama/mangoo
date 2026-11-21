import { useEffect } from "react";
import {
  LiquidGlassLayer,
  LiquidGlassSurface,
  getWindowControls,
  icons,
  useOverlayScrollbars,
  useSettingsNavigation,
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
  const { active, jumpTo, scrollRef } = useSettingsNavigation(IMAGE_VIEWER_SETTINGS_SECTIONS, sectionFromHash());
  const controls = getWindowControls();

  // The viewer asked again while this window was already open.
  useEffect(() => settingsBridge()?.onSettingsSection((section) => jumpTo(section)), [jumpTo]);

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
        <div className="iv-settings-window-body settings-layout-body">
          <nav aria-label="设置分类" className="iv-settings-nav settings-layout-nav">
            {IMAGE_VIEWER_SETTINGS_SECTIONS.map((id) => {
              const Icon = SECTION_ICONS[id];
              return (
                <button
                  aria-current={active === id ? "true" : undefined}
                  className="iv-settings-nav-item settings-layout-nav-item"
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
          </nav>
          <div className="iv-settings-scroll settings-layout-scroll" ref={scrollRef}>
            <ViewerSettingsForm onChange={setPreferences} preferences={preferences} showShortcuts />
          </div>
        </div>
      </LiquidGlassSurface>
    </LiquidGlassLayer>
  );
}
