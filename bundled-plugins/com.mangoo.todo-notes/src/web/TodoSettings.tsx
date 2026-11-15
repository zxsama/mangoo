import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import {
  LiquidGlassLayer,
  LiquidGlassSurface,
  getDesktopBridge,
  getWindowControls,
  icons,
  useOverlayScrollbars,
  useSecondaryWindowInterfaceStyle
} from "@mangoo/web/plugin-host";

import {
  TODO_SETTINGS_ROUTE,
  TODO_SETTINGS_SECTIONS,
  type TodoBackupResult,
  type TodoDesktopSettings,
  type TodoSettingsSection,
  type TodoSettingsWindowBridge
} from "../shared/ipc.ts";
import { todoBridge, todoHostBridge } from "./bridge.ts";
import { useTodoPreferences, type TodoPreferences } from "./preferences.ts";
import { AcceleratorRecorder, ShortcutEditor } from "./ShortcutEditor.tsx";
import "./todo.css";

/**
 * Settings, laid out exactly like the image viewer's: titled sections, one
 * bordered group of rows per section, one row per decision with a short
 * explanation and the control that fits it (switch for on / off, segmented
 * control for a small choice). Rendered in the standalone settings window AND
 * in Plugin Manager. Desktop settings go through IPC; display preferences are
 * per device (localStorage, synchronized with the notes window by `storage`
 * events).
 */

export const SETTINGS_SECTION_LABELS: Record<TodoSettingsSection, string> = {
  capture: "记录",
  display: "显示",
  stickies: "桌面便笺",
  reminders: "提醒",
  system: "系统",
  shortcuts: "快捷键",
  backup: "备份"
};

const SECTION_ICONS: Record<TodoSettingsSection, typeof icons.Sun> = {
  capture: icons.Pencil,
  display: icons.LayoutList,
  stickies: icons.Pin,
  reminders: icons.Bell,
  system: icons.Settings,
  shortcuts: icons.Keyboard,
  backup: icons.Archive
};

type SettingsSource = Pick<TodoSettingsWindowBridge, "getSettings" | "setSettings"> & Partial<Pick<TodoSettingsWindowBridge, "onSettingsChanged" | "status">>;

function settingsWindowBridge(): TodoSettingsWindowBridge | null {
  return (getDesktopBridge() as { todoNotesSettings?: TodoSettingsWindowBridge } | null)?.todoNotesSettings ?? null;
}

function settingsSource(): SettingsSource | null {
  return settingsWindowBridge() ?? todoBridge() ?? todoHostBridge();
}

/** Desktop settings, kept current across windows (`settingsChanged`). */
export function useDesktopSettings(): [TodoDesktopSettings | null, (patch: Partial<TodoDesktopSettings>) => void] {
  const [settings, setSettings] = useState<TodoDesktopSettings | null>(null);
  useEffect(() => {
    let cancelled = false;
    const source = settingsSource();
    void source?.getSettings().then((value) => {
      if (!cancelled) setSettings(value);
    }).catch(() => undefined);
    const unsubscribe = settingsWindowBridge()?.onSettingsChanged((value) => setSettings(value)) ?? todoBridge()?.onSettingsChanged((value) => setSettings(value));
    return () => {
      cancelled = true;
      unsubscribe?.();
    };
  }, []);
  const update = useCallback((patch: Partial<TodoDesktopSettings>) => {
    setSettings((current) => (current ? { ...current, ...patch } : current));
    void settingsSource()?.setSettings(patch).then(setSettings).catch(() => undefined);
  }, []);
  return [settings, update];
}

export function Switch({ checked, label, onChange, disabled = false }: { checked: boolean; label: string; onChange(next: boolean): void; disabled?: boolean }) {
  return (
    <label className={`plugin-toggle todo-switch${checked ? " on" : ""}${disabled ? " disabled" : ""}`}>
      <input aria-label={label} checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} role="switch" type="checkbox" />
      <span aria-hidden="true" className="plugin-toggle-track">
        <span className="plugin-toggle-thumb" />
      </span>
    </label>
  );
}

export function Segmented<T extends string>({ value, options, label, onChange, disabled = false }: {
  value: T;
  options: ReadonlyArray<{ value: T; label: ReactNode; title?: string }>;
  label: string;
  onChange(next: T): void;
  disabled?: boolean;
}) {
  return (
    <div aria-disabled={disabled} aria-label={label} className="todo-segmented" role="radiogroup">
      {options.map((option) => {
        const selected = option.value === value;
        return (
          <button
            aria-checked={selected}
            className="todo-segment"
            data-pointer-glow="off"
            data-selected={selected ? "true" : "false"}
            disabled={disabled}
            key={option.value}
            onClick={() => onChange(option.value)}
            role="radio"
            title={option.title}
            type="button"
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

export function SettingRow({ title, description, children, stacked = false }: { title: string; description?: ReactNode; children: ReactNode; stacked?: boolean }) {
  return (
    <div className="todo-setting-row" data-layout={stacked ? "stacked" : "inline"}>
      <div
        className="todo-setting-text"
        onClick={(event) => {
          // Clicking a row's text toggles its switch, like system settings.
          const row = (event.currentTarget as HTMLElement).parentElement;
          const input = row?.querySelector<HTMLInputElement>('.todo-setting-control input[type="checkbox"]');
          if (input && !input.disabled) input.click();
        }}
      >
        <div className="todo-setting-title">{title}</div>
        {description ? <p className="todo-setting-description">{description}</p> : null}
      </div>
      <div className="todo-setting-control">{children}</div>
    </div>
  );
}

function Section({ id, children, plain = false }: { id: TodoSettingsSection; children: ReactNode; plain?: boolean }) {
  const Icon = SECTION_ICONS[id];
  return (
    <section className="todo-settings-section" data-section={id} id={`todo-settings-${id}`}>
      <h3 className="todo-settings-section-title">
        <Icon aria-hidden="true" size={15} />
        <span>{SETTINGS_SECTION_LABELS[id]}</span>
      </h3>
      {plain ? children : <div className="todo-settings-rows">{children}</div>}
    </section>
  );
}

export interface TodoSettingsFormProps {
  canLaunchAtLogin: boolean;
  preferences: TodoPreferences;
  onPreferences(patch: Partial<TodoPreferences>): void;
  /** Sections to render (Plugin Manager hides the shortcut editor). */
  sections?: readonly TodoSettingsSection[];
}

/** Whether another application holds the global quick-add shortcut. */
function useShortcutTaken(shortcut: string | null | undefined): boolean {
  const [taken, setTaken] = useState(false);
  useEffect(() => {
    let cancelled = false;
    const source = settingsSource();
    if (!shortcut || !source?.status) {
      setTaken(false);
      return;
    }
    // The Desktop host re-registers right after the settings change.
    const timer = window.setTimeout(() => {
      void source.status?.().then((status) => {
        if (!cancelled) setTaken(status.quickAddShortcutTaken);
      }).catch(() => undefined);
    }, 150);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [shortcut]);
  return taken;
}

export function TodoSettingsForm({ canLaunchAtLogin, preferences, onPreferences, sections = TODO_SETTINGS_SECTIONS }: TodoSettingsFormProps) {
  const [settings, update] = useDesktopSettings();
  const shortcutTaken = useShortcutTaken(settings?.quickAddShortcut);
  const show = (id: TodoSettingsSection) => sections.includes(id);
  return (
    <div className="todo-settings-form">
      {show("capture") ? (
        <Section id="capture">
          <SettingRow description="在任何程序中按下快捷键，打开便笺并直接开始输入。点击后按下新的组合键即可修改。" title="全局快速记录">
            <AcceleratorRecorder
              disabled={!settings}
              onChange={(value) => update({ quickAddShortcut: value })}
              taken={shortcutTaken}
              value={settings?.quickAddShortcut ?? null}
            />
          </SettingRow>
        </Section>
      ) : null}
      {show("display") ? (
        <Section id="display">
          <SettingRow description="卡片只显示标题和日期等信息，不显示备注预览、图片和子事项。" title="紧凑显示">
            <Switch checked={preferences.compact} label="紧凑显示" onChange={(value) => onPreferences({ compact: value })} />
          </SettingRow>
          <SettingRow description="在全部和清单视图中，把已完成的便笺列在最后，而不是收起。" title="展开已完成">
            <Switch checked={preferences.showCompleted} label="展开已完成" onChange={(value) => onPreferences({ showCompleted: value })} />
          </SettingRow>
        </Section>
      ) : null}
      {show("stickies") ? (
        <Section id="stickies">
          <SettingRow description="平时只显示标题和日期，鼠标移到便笺上时展开备注和子事项。" title="悬停时展开">
            <Switch checked={settings?.expandOnHover ?? true} disabled={!settings} label="悬停时展开" onChange={(value) => update({ expandOnHover: value })} />
          </SettingRow>
          <SettingRow description="贴在桌面的便笺始终显示在其他窗口之上。" title="置顶显示">
            <Switch checked={settings?.stickiesOnTop ?? true} disabled={!settings} label="置顶显示" onChange={(value) => update({ stickiesOnTop: value })} />
          </SettingRow>
          <SettingRow description="暂时隐藏所有贴在桌面的便笺，随时可从托盘菜单显示。" title="隐藏桌面便笺">
            <Switch checked={settings?.stickiesHidden ?? false} disabled={!settings} label="隐藏桌面便笺" onChange={(value) => update({ stickiesHidden: value })} />
          </SettingRow>
        </Section>
      ) : null}
      {show("reminders") ? (
        <Section id="reminders">
          <SettingRow description="到了提醒时间显示系统通知，点击通知打开对应便笺。" title="系统通知">
            <Switch checked={settings?.reminders ?? true} disabled={!settings} label="系统通知" onChange={(value) => update({ reminders: value })} />
          </SettingRow>
        </Section>
      ) : null}
      {show("system") ? (
        <Section id="system">
          <SettingRow description="在通知区域显示便笺图标，点击即可打开；关闭所有窗口后 Mangoo 仍在后台运行。" title="托盘图标">
            <Switch checked={settings?.trayIcon ?? true} disabled={!settings} label="托盘图标" onChange={(value) => update({ trayIcon: value })} />
          </SettingRow>
          <SettingRow description="关闭便笺窗口时隐藏到托盘，而不是退出。" title="关闭时隐藏到托盘">
            <Switch checked={settings?.closeToTray ?? true} disabled={!settings || !settings.trayIcon} label="关闭时隐藏到托盘" onChange={(value) => update({ closeToTray: value })} />
          </SettingRow>
          <SettingRow
            description={canLaunchAtLogin ? "登录系统时在后台启动，只显示托盘图标和桌面便笺。" : "开发版不会修改系统启动项，安装正式版后即可开启。"}
            title="登录时启动"
          >
            <Switch checked={settings?.launchAtLogin ?? false} disabled={!settings || !canLaunchAtLogin} label="登录时启动" onChange={(value) => update({ launchAtLogin: value })} />
          </SettingRow>
        </Section>
      ) : null}
      {show("shortcuts") ? (
        <Section id="shortcuts" plain>
          <ShortcutEditor onChange={(next) => onPreferences({ shortcuts: next })} overrides={preferences.shortcuts} />
        </Section>
      ) : null}
      {show("backup") ? (
        <Section id="backup">
          <BackupRows />
        </Section>
      ) : null}
    </div>
  );
}

/**
 * Export every list, note and pasted image to one file, or restore one. A
 * restore writes each backed-up note back under its own id (an existing note
 * takes the backup's content, a deleted one returns); notes created after the
 * backup are kept. Only in the settings window (needs the file dialogs).
 */
function BackupRows() {
  const bridge = settingsWindowBridge();
  const [busy, setBusy] = useState<"export" | "import" | null>(null);
  const [message, setMessage] = useState<{ tone: "info" | "warning"; text: string } | null>(null);
  const counts = (result: TodoBackupResult) => {
    const parts = [
      result.lists ? `${result.lists} 个清单` : "",
      result.notes ? `${result.notes} 条便笺` : "",
      result.subtasks ? `${result.subtasks} 个子事项` : "",
      result.images ? `${result.images} 张图片` : ""
    ].filter(Boolean);
    return parts.length ? parts.join("、") : "没有便笺";
  };
  const run = async (kind: "export" | "import") => {
    if (!bridge) return;
    setBusy(kind);
    setMessage(null);
    try {
      const result = kind === "export" ? await bridge.exportBackup() : await bridge.importBackup();
      if (result.cancelled) return;
      if (!result.ok) setMessage({ tone: "warning", text: `${kind === "export" ? "导出" : "恢复"}失败：${result.error ?? "未知错误"}` });
      else setMessage({ tone: "info", text: kind === "export" ? `已导出 ${counts(result)}。` : `已恢复 ${counts(result)}。` });
    } finally {
      setBusy(null);
    }
  };
  return (
    <>
      <SettingRow description="清单、便笺、子事项和图片保存为一个文件。" title="导出备份">
        <button className="todo-button" disabled={!bridge || busy !== null} onClick={() => void run("export")} type="button">
          <icons.Download aria-hidden="true" size={13} />
          {busy === "export" ? "正在导出…" : "导出…"}
        </button>
      </SettingRow>
      <SettingRow description="备份里的便笺回到备份时的内容，之后新建的便笺保留。" title="从备份恢复">
        <button className="todo-button" disabled={!bridge || busy !== null} onClick={() => void run("import")} type="button">
          <icons.FolderOpen aria-hidden="true" size={13} />
          {busy === "import" ? "正在恢复…" : "选择文件…"}
        </button>
      </SettingRow>
      {message ? (
        <p className="todo-settings-status" data-tone={message.tone} role="status">{message.text}</p>
      ) : null}
    </>
  );
}

/** Pixels below the scroller's top edge where a section counts as "current". */
const SPY_OFFSET = 28;

function sectionFromHash(): TodoSettingsSection {
  const tail = window.location.hash.slice(TODO_SETTINGS_ROUTE.length + 1);
  return (TODO_SETTINGS_SECTIONS as readonly string[]).includes(tail) ? (tail as TodoSettingsSection) : "capture";
}

/**
 * The standalone settings window (frameless, acrylic), the image viewer's
 * pattern: left, one label per section; right, every setting in one scrolling
 * column. Scrolling highlights the section in view; a label scrolls to it.
 */
export function TodoSettingsWindow() {
  useSecondaryWindowInterfaceStyle();
  useOverlayScrollbars();
  const [preferences, setPreferences] = useTodoPreferences();
  const [active, setActive] = useState<TodoSettingsSection>(sectionFromHash);
  const [canLaunchAtLogin, setCanLaunchAtLogin] = useState(false);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const spyLock = useRef<number | null>(null);
  const controls = getWindowControls();
  const bridge = settingsWindowBridge();

  useEffect(() => {
    document.title = "便笺设置";
    void bridge?.status().then((status) => setCanLaunchAtLogin(status.canLaunchAtLogin)).catch(() => undefined);
  }, [bridge]);

  const jumpTo = useCallback((id: TodoSettingsSection, smooth = true) => {
    const scroller = scrollRef.current;
    const target = scroller?.querySelector<HTMLElement>(`[data-section="${id}"]`);
    if (!scroller || !target) return;
    setActive(id);
    if (spyLock.current) window.clearTimeout(spyLock.current);
    spyLock.current = window.setTimeout(() => {
      spyLock.current = null;
    }, smooth ? 700 : 50);
    const top = target.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop - 12;
    scroller.scrollTo({ top: Math.max(0, top), behavior: smooth ? "smooth" : "auto" });
  }, []);

  useLayoutEffect(() => {
    const initial = sectionFromHash();
    if (initial !== "capture") jumpTo(initial, false);
  }, [jumpTo]);

  useEffect(() => bridge?.onSettingsSection((section) => jumpTo(section)), [bridge, jumpTo]);

  // Scroll spy.
  useEffect(() => {
    const scroller = scrollRef.current;
    if (!scroller) return;
    const update = () => {
      if (spyLock.current) return;
      const sections = [...scroller.querySelectorAll<HTMLElement>("[data-section]")];
      if (!sections.length) return;
      let current = sections[0].dataset.section as TodoSettingsSection;
      if (scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2) current = sections[sections.length - 1].dataset.section as TodoSettingsSection;
      else {
        const edge = scroller.getBoundingClientRect().top + SPY_OFFSET;
        for (const section of sections) if (section.getBoundingClientRect().top <= edge) current = section.dataset.section as TodoSettingsSection;
      }
      setActive((value) => (value === current ? value : current));
    };
    const release = () => {
      if (spyLock.current) {
        window.clearTimeout(spyLock.current);
        spyLock.current = null;
      }
    };
    scroller.addEventListener("scroll", update, { passive: true });
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
      if (event.key === "Escape" && !event.defaultPrevented) window.close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <LiquidGlassLayer>
      <LiquidGlassSurface as="div" className="todo-settings-window" tone="chrome">
        <header className="todo-settings-window-titlebar">
          <span className="todo-settings-window-title">
            <icons.Settings aria-hidden="true" size={14} />
            便笺设置
          </span>
          <button aria-label="关闭" className="todo-icon-button no-drag" onClick={() => (controls ? void controls.close() : window.close())} title="关闭（Esc）" type="button">
            <icons.X aria-hidden="true" size={14} />
          </button>
        </header>
        <div className="todo-settings-window-body">
          <nav aria-label="设置分类" className="todo-settings-nav">
            {TODO_SETTINGS_SECTIONS.map((id) => {
              const Icon = SECTION_ICONS[id];
              return (
                <button aria-current={active === id ? "true" : undefined} className="todo-settings-nav-item" data-active={active === id ? "true" : "false"} key={id} onClick={() => jumpTo(id)} type="button">
                  <Icon aria-hidden="true" size={15} />
                  <span>{SETTINGS_SECTION_LABELS[id]}</span>
                </button>
              );
            })}
          </nav>
          <div className="todo-settings-scroll" ref={scrollRef}>
            <TodoSettingsForm canLaunchAtLogin={canLaunchAtLogin} onPreferences={setPreferences} preferences={preferences} />
          </div>
        </div>
      </LiquidGlassSurface>
    </LiquidGlassLayer>
  );
}
