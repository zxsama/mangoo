import { useEffect, useMemo, useState } from "react";
import { icons } from "@mangoo/web/plugin-host";

import {
  RESERVED_KEYS,
  SHORTCUT_ACTIONS,
  SHORTCUT_GROUPS,
  assignChord,
  chordFromEvent,
  chordParts,
  findConflict,
  formatChord,
  isCustomized,
  removeChord,
  resetAction,
  resolveBindings,
  type ShortcutActionId,
  type ShortcutOverrides
} from "../shared/shortcuts.ts";
import { acceleratorLabel, normalizeAccelerator } from "../shared/ipc.ts";

function Keycaps({ chord }: { chord: string }) {
  return <>{chordParts(chord).map((part, index) => <kbd className="todo-keycap" key={index}>{part}</kbd>)}</>;
}

/**
 * Shortcut editor, the image viewer's pattern: click a key chip (or +) to
 * record a chord; pressing a chord already used elsewhere moves it here and
 * says so (with undo). Esc cancels recording, Backspace removes the chord.
 */
export function ShortcutEditor({ overrides, onChange }: { overrides: ShortcutOverrides; onChange(next: ShortcutOverrides): void }) {
  const bindings = useMemo(() => resolveBindings(overrides), [overrides]);
  const [recording, setRecording] = useState<{ action: ShortcutActionId; index: number | null } | null>(null);
  const [notice, setNotice] = useState<{ text: string; tone: "info" | "warning"; undo?: ShortcutOverrides } | null>(null);
  const label = (id: ShortcutActionId) => SHORTCUT_ACTIONS.find((action) => action.id === id)?.label ?? "";

  useEffect(() => {
    if (!recording) return;
    const onKey = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        setRecording(null);
        return;
      }
      if ((event.key === "Backspace" || event.key === "Delete") && recording.index !== null && !event.ctrlKey && !event.altKey && !event.shiftKey) {
        const chord = bindings[recording.action][recording.index];
        if (chord) {
          onChange(removeChord(overrides, recording.action, chord));
          setNotice({ text: `已删除“${label(recording.action)}”的 ${formatChord(chord)}。`, tone: "info", undo: overrides });
        }
        setRecording(null);
        return;
      }
      const chord = chordFromEvent(event);
      if (!chord) return; // modifier only: keep waiting
      if (RESERVED_KEYS.has(chord)) {
        setNotice({ text: `${formatChord(chord)} 由窗口保留（关闭、切换焦点、打开），不能使用。`, tone: "warning" });
        return;
      }
      const conflict = findConflict(bindings, chord, recording.action);
      onChange(assignChord(overrides, recording.action, chord, recording.index));
      setNotice(conflict
        ? { text: `${formatChord(chord)} 原本属于“${label(conflict)}”，现已改给“${label(recording.action)}”。`, tone: "warning", undo: overrides }
        : { text: `已将 ${formatChord(chord)} 设为“${label(recording.action)}”。`, tone: "info", undo: overrides });
      setRecording(null);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [bindings, onChange, overrides, recording]);

  const changed = SHORTCUT_ACTIONS.some((action) => isCustomized(overrides, action.id));
  const resetOne = (id: ShortcutActionId) => {
    const previous = overrides;
    onChange(resetAction(overrides, id));
    setNotice({ text: `已恢复“${label(id)}”的默认快捷键。`, tone: "info", undo: previous });
  };

  return (
    <div className="todo-shortcut-editor">
      <div className="todo-shortcut-toolbar">
        <p className="todo-shortcut-hint" data-tone={!recording && notice?.tone === "warning" ? "warning" : "info"} role="status">
          {recording ? "请按下新的快捷键…（Esc 取消，Backspace 删除）" : notice?.text ?? "点击快捷键修改，点击 + 添加。"}
          {!recording && notice?.undo ? (
            <button className="todo-link-button" onClick={() => { onChange(notice.undo ?? {}); setNotice({ text: "已撤销。", tone: "info" }); }} type="button">撤销</button>
          ) : null}
        </p>
        <button
          className="todo-button"
          disabled={!changed}
          onClick={() => {
            const previous = overrides;
            onChange({});
            setNotice({ text: "已恢复全部默认快捷键。", tone: "info", undo: previous });
          }}
          type="button"
        >
          <icons.RotateCcw aria-hidden="true" size={13} />
          全部恢复默认
        </button>
      </div>
      {SHORTCUT_GROUPS.map((group) => (
        <section className="todo-shortcut-group" key={group}>
          <h4 className="todo-shortcut-group-title">{group}</h4>
          <div className="todo-settings-rows">
            {SHORTCUT_ACTIONS.filter((action) => action.group === group).map((action) => {
              const chords = bindings[action.id];
              const customized = isCustomized(overrides, action.id);
              return (
                <div className="todo-setting-row todo-shortcut-row" data-customized={customized ? "true" : "false"} key={action.id}>
                  <div className="todo-setting-title">{action.label}</div>
                  <div className="todo-shortcut-chords">
                    {chords.map((chord, index) => {
                      const active = recording?.action === action.id && recording.index === index;
                      return (
                        <span className="todo-chord-slot" key={chord}>
                        {index > 0 ? <span className="todo-chord-or">或</span> : null}
                        <button
                          aria-label={`${action.label}：${formatChord(chord)}，点击修改`}
                          className="todo-chord"
                          data-pointer-glow="off"
                          data-recording={active ? "true" : "false"}
                          onClick={() => setRecording(active ? null : { action: action.id, index })}
                          type="button"
                        >
                          {active ? "按下快捷键…" : <Keycaps chord={chord} />}
                        </button>
                        </span>
                      );
                    })}
                    {chords.length === 0 ? <span className="todo-chord-none">未设置</span> : null}
                    {chords.length < 4 ? (
                      <button
                        aria-label={`为“${action.label}”添加快捷键`}
                        className="todo-chord todo-chord-add"
                        data-pointer-glow="off"
                        data-recording={recording?.action === action.id && recording.index === null ? "true" : "false"}
                        onClick={() => setRecording({ action: action.id, index: null })}
                        title="添加快捷键"
                        type="button"
                      >
                        {recording?.action === action.id && recording.index === null ? "按下快捷键…" : <icons.Plus aria-hidden="true" size={12} />}
                      </button>
                    ) : null}
                    <button
                      aria-label={`恢复“${action.label}”的默认快捷键`}
                      className="todo-icon-button todo-shortcut-reset"
                      data-pointer-glow="off"
                      disabled={!customized}
                      onClick={() => resetOne(action.id)}
                      title={customized ? `恢复默认：${action.defaults.map(formatChord).join("、") || "无"}` : "默认快捷键"}
                      type="button"
                    >
                      <icons.RotateCcw aria-hidden="true" size={12} />
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        </section>
      ))}
      <p className="todo-shortcut-footnote">Esc 收起 / 关闭，Enter 打开选中的便笺。描述里的文字格式沿用常见编辑器：Ctrl+B 加粗、Ctrl+I 斜体、Ctrl+U 下划线，也可以直接输入 Markdown。</p>
    </div>
  );
}

/**
 * Recorder for the GLOBAL quick-add shortcut (works in any application).
 * Needs Ctrl or Alt (or a function key) so it never swallows normal typing.
 */
export function AcceleratorRecorder({ value, taken, disabled, onChange }: { value: string | null; taken: boolean; disabled: boolean; onChange(next: string | null): void }) {
  const [recording, setRecording] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!recording) return;
    const onKey = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        setRecording(false);
        return;
      }
      const chord = chordFromEvent(event);
      if (!chord) return;
      const accelerator = normalizeAccelerator(chord.replace(/^Ctrl\+/, "CommandOrControl+").replace("ArrowUp", "Up").replace("ArrowDown", "Down").replace("ArrowLeft", "Left").replace("ArrowRight", "Right"));
      if (!accelerator) {
        setError("需要包含 Ctrl 或 Alt，例如 Ctrl+Alt+N。");
        return;
      }
      setError(null);
      setRecording(false);
      onChange(accelerator);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onChange, recording]);
  return (
    <div className="todo-accelerator">
      <button
        aria-label={value ? `全局快速记录：${acceleratorLabel(value)}，点击修改` : "设置全局快速记录快捷键"}
        className="todo-chord todo-chord-field"
        data-pointer-glow="off"
        data-recording={recording ? "true" : "false"}
        disabled={disabled}
        onClick={() => {
          setError(null);
          setRecording(!recording);
        }}
        type="button"
      >
        {recording ? "按下快捷键…" : value ? <Keycaps chord={acceleratorLabel(value).replace("空格", "Space")} /> : <span className="todo-chord-none">点击设置</span>}
      </button>
      {value ? (
        <button aria-label="不使用全局快捷键" className="todo-icon-button" data-pointer-glow="off" disabled={disabled} onClick={() => onChange(null)} title="不使用" type="button">
          <icons.X aria-hidden="true" size={12} />
        </button>
      ) : null}
      {error ? <span className="todo-accelerator-note" data-tone="warning">{error}</span> : taken ? <span className="todo-accelerator-note" data-tone="warning">已被其他程序占用</span> : null}
    </div>
  );
}
