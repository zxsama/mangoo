import { useEffect, useMemo, useState } from "react";
import { LiquidGlassButton, icons } from "@mangoo/web/plugin-host";

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

/**
 * Shortcut editor. Click a key chip (or "+") to record a new chord; pressing
 * a chord already used elsewhere moves it here and says so. Esc cancels
 * recording; Backspace clears. Every action can be reset individually and
 * the whole sheet can be restored to defaults.
 */
export function ShortcutEditor({
  overrides,
  onChange,
  filter = ""
}: {
  overrides: ShortcutOverrides;
  onChange(next: ShortcutOverrides): void;
  filter?: string;
}) {
  const bindings = useMemo(() => resolveBindings(overrides), [overrides]);
  const [recording, setRecording] = useState<{ action: ShortcutActionId; index: number | null } | null>(null);
  const [notice, setNotice] = useState<{ text: string; tone: "info" | "warning"; undo?: ShortcutOverrides } | null>(null);
  const [query, setQuery] = useState(filter);

  useEffect(() => {
    if (!recording) return;
    const onKey = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        setRecording(null);
        return;
      }
      if ((event.key === "Backspace" || event.key === "Delete") && recording.index !== null) {
        const chord = bindings[recording.action][recording.index];
        if (chord) onChange(removeChord(overrides, recording.action, chord));
        setRecording(null);
        return;
      }
      const chord = chordFromEvent(event);
      if (!chord) return; // modifier only: keep waiting
      if (RESERVED_KEYS.has(chord)) {
        setNotice({ text: `${formatChord(chord)} 由窗口保留，不能使用。`, tone: "warning" });
        return;
      }
      const conflict = findConflict(bindings, chord, recording.action);
      onChange(assignChord(overrides, recording.action, chord, recording.index));
      const label = SHORTCUT_ACTIONS.find((action) => action.id === recording.action)?.label ?? "";
      setNotice(conflict
        ? { text: `${formatChord(chord)} 原本属于“${SHORTCUT_ACTIONS.find((action) => action.id === conflict)?.label}”，现已改给“${label}”。`, tone: "warning", undo: overrides }
        : { text: `已将 ${formatChord(chord)} 设为“${label}”。`, tone: "info", undo: overrides });
      setRecording(null);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [bindings, onChange, overrides, recording]);

  const normalizedQuery = query.trim().toLowerCase();
  const changedCount = SHORTCUT_ACTIONS.filter((action) => isCustomized(overrides, action.id)).length;

  return (
    <div className="iv-shortcut-editor">
      <div className="iv-shortcut-toolbar">
        <label className="iv-shortcut-search">
          <icons.Search aria-hidden="true" size={13} />
          <input aria-label="搜索快捷键" onChange={(event) => setQuery(event.target.value)} placeholder="搜索操作" type="search" value={query} />
        </label>
        <LiquidGlassButton
          className="iv-shortcut-reset-all"
          disabled={changedCount === 0}
          onClick={() => {
            const previous = overrides;
            onChange({});
            setNotice({ text: "已恢复全部默认快捷键。", tone: "info", undo: previous });
          }}
          tone="control"
          type="button"
        >
          <icons.RotateCcw aria-hidden="true" size={13} />
          <span>全部恢复默认</span>
        </LiquidGlassButton>
      </div>
      <p className="iv-shortcut-hint" data-tone={!recording && notice?.tone === "warning" ? "warning" : "info"} role="status">
        {recording ? "请按下新的快捷键…（Esc 取消，Backspace 删除这个快捷键）" : notice?.text ?? "点击快捷键可修改，点击 + 可添加。"}
        {!recording && notice?.undo ? (
          <button
            className="iv-shortcut-undo"
            onClick={() => {
              onChange(notice.undo ?? {});
              setNotice({ text: "已撤销。", tone: "info" });
            }}
            type="button"
          >
            撤销
          </button>
        ) : null}
      </p>
      {SHORTCUT_GROUPS.map((group) => {
        const actions = SHORTCUT_ACTIONS.filter((action) => action.group === group && (!normalizedQuery || action.label.toLowerCase().includes(normalizedQuery)));
        if (actions.length === 0) return null;
        return (
          <section className="iv-shortcut-group-block" key={group}>
            <h4 className="iv-shortcut-group-heading">{group}</h4>
            <div className="iv-settings-rows">
              {actions.map((action) => {
                const chords = bindings[action.id];
                const customized = isCustomized(overrides, action.id);
                return (
                  <div className="iv-shortcut-edit-row" data-customized={customized ? "true" : "false"} data-empty={chords.length === 0 ? "true" : "false"} key={action.id}>
                    <span className="iv-setting-title">{action.label}</span>
                    <span className="iv-shortcut-chords">
                      {chords.map((chord, index) => {
                        const active = recording?.action === action.id && recording.index === index;
                        return (
                          <span className="iv-chord-slot" key={chord}>
                          {index > 0 ? <span className="iv-chord-or">或</span> : null}
                          <button
                            aria-label={`${action.label}：${formatChord(chord)}，点击修改`}
                            className="iv-chord"
                            data-recording={active ? "true" : "false"}
                            data-pointer-glow="off"
                            onClick={() => setRecording(active ? null : { action: action.id, index })}
                            type="button"
                          >
                            {active ? "按下快捷键…" : chordParts(chord).map((part, partIndex) => <kbd className="iv-keycap" key={partIndex}>{part}</kbd>)}
                          </button>
                          </span>
                        );
                      })}
                      {chords.length === 0 ? <span className="iv-chord-none">未设置</span> : null}
                      {chords.length < 4 ? (
                        <button
                          aria-label={`为“${action.label}”添加快捷键`}
                          className="iv-chord iv-chord-add"
                          data-recording={recording?.action === action.id && recording.index === null ? "true" : "false"}
                          data-pointer-glow="off"
                          onClick={() => setRecording({ action: action.id, index: null })}
                          title="添加快捷键"
                          type="button"
                        >
                          {recording?.action === action.id && recording.index === null ? "按下快捷键…" : <icons.Plus aria-hidden="true" size={12} />}
                        </button>
                      ) : null}

                    </span>
                    <button
                      aria-label={`恢复“${action.label}”的默认快捷键`}
                      className="iv-shortcut-reset"
                      data-pointer-glow="off"
                      disabled={!customized}
                      onClick={() => onChange(resetAction(overrides, action.id))}
                      title={customized ? `恢复默认：${action.defaults.map(formatChord).join("、")}` : "使用默认快捷键"}
                      type="button"
                    >
                      <icons.RotateCcw aria-hidden="true" size={12} />
                    </button>
                  </div>
                );
              })}
            </div>
          </section>
        );
      })}
      <p className="iv-shortcut-footnote">鼠标：滚轮默认切换图片，按住 Ctrl 滚动缩放；双击图片在适应窗口与原始大小间切换；鼠标侧键切换上一张 / 下一张。</p>
    </div>
  );
}
