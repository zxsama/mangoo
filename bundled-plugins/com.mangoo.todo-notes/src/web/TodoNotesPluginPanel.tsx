import { useEffect, useState } from "react";
import { LiquidGlassButton, icons } from "@mangoo/web/plugin-host";

import type { TodoStatus } from "../shared/ipc.ts";
import { todoHostBridge } from "./bridge.ts";
import { useTodoPreferences } from "./preferences.ts";
import { TodoSettingsForm } from "./TodoSettings.tsx";
import "./todo.css";

/** Rendered in the plugin manager detail pane for com.mangoo.todo-notes. */
export function TodoNotesPluginPanel({ enabled, refreshKey }: { enabled: boolean; refreshKey?: unknown }) {
  const bridge = todoHostBridge();
  const [status, setStatus] = useState<TodoStatus | null>(null);
  const [preferences, setPreferences] = useTodoPreferences();
  useEffect(() => {
    let cancelled = false;
    if (!enabled || !bridge) return;
    // The Desktop host reconciles right after the lifecycle change.
    const timer = window.setTimeout(() => {
      void bridge.status().then((value) => {
        if (!cancelled) setStatus(value);
      }).catch(() => undefined);
    }, 250);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [bridge, enabled, refreshKey]);

  const intro = (
    <div className="todo-plugin-intro">
      <icons.StickyNote aria-hidden="true" size={28} weight="light" />
      <div>
        <strong>工作事项与备忘</strong>
        <p className="todo-plugin-description">
          竖版便笺窗口和托盘图标：清单、子事项、优先级、日期与提醒、重复，标题和描述都支持富文本与 Markdown，可直接粘贴链接和图片。任意便笺都可以贴到桌面，平时只显示摘要，鼠标移上去时展开。
        </p>
        <p className="todo-plugin-description">
          便笺不属于任何资料库：打开哪个资料库都能看到同一份便笺，并随资料库同步到你的其他设备；停用插件只会关闭便笺的窗口、托盘图标和快捷键，不会删除任何便笺。
        </p>
      </div>
    </div>
  );

  if (!enabled) return <div className="plugin-detail-section todo-plugin-panel">{intro}</div>;
  return (
    <div className="plugin-detail-section todo-plugin-panel">
      {intro}
      <div className="todo-plugin-actions">
        <LiquidGlassButton className="todo-button todo-button-primary" disabled={!bridge} onClick={() => void bridge?.openWindow()} tone="primary" type="button">
          <icons.StickyNote aria-hidden="true" size={14} />
          打开便笺
        </LiquidGlassButton>
        <LiquidGlassButton className="todo-button" disabled={!bridge} onClick={() => void bridge?.openSettings("capture")} type="button">
          <icons.Settings aria-hidden="true" size={14} />
          设置窗口
        </LiquidGlassButton>
        {status?.pinnedCount ? <span className="todo-muted">已贴到桌面 {status.pinnedCount} 条</span> : null}
      </div>
      {bridge ? (
        <TodoSettingsForm
          canLaunchAtLogin={status?.canLaunchAtLogin ?? false}
          onPreferences={setPreferences}
          preferences={preferences}
          sections={["capture", "display", "stickies", "reminders", "system"]}
        />
      ) : (
        <p className="todo-muted">便笺窗口与托盘需要在 Mangoo 桌面版中使用。</p>
      )}
    </div>
  );
}
