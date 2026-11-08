import { icons } from "@mangoo/web/plugin-host";

import { PLAYBACK_SPEEDS, formatFrameTime } from "../shared/frames.ts";

export interface FramePlaybackState {
  frameCount: number;
  index: number;
  playing: boolean;
  speed: number;
  loop: boolean;
  /** Per-frame durations in ms. */
  durations: number[];
  startTimes: number[];
  totalMs: number;
}

/**
 * Animation controls. Two modes over the same frames: "播放" plays the
 * animation (with speed and loop); "逐帧" pauses and steps frame by frame,
 * with a scrubber showing every frame's position and its own delay.
 */
export function FrameInspector({
  state,
  onPlayPause,
  onStep,
  onSeek,
  onSpeed,
  onLoop
}: {
  state: FramePlaybackState;
  onPlayPause(): void;
  onStep(delta: number): void;
  onSeek(index: number): void;
  onSpeed(speed: number): void;
  onLoop(loop: boolean): void;
}) {
  const current = state.index + 1;
  const duration = state.durations[state.index] ?? 0;
  const elapsed = state.startTimes[state.index] ?? 0;
  return (
    <div className="iv-frames">
      <div className="iv-frames-readout" aria-live="polite">
        <span className="iv-frames-counter">
          第 <strong>{current}</strong> / {state.frameCount} 帧
        </span>
        <span className="iv-frames-time">位置 {formatFrameTime(elapsed)} · 共 {formatFrameTime(state.totalMs)}</span>
      </div>

      <input
        aria-label="帧位置"
        className="iv-frames-scrubber"
        max={state.frameCount - 1}
        min={0}
        onChange={(event) => onSeek(Number(event.target.value))}
        step={1}
        type="range"
        value={state.index}
      />

      <div className="iv-frames-controls" role="group" aria-label={state.playing ? "正在播放" : "逐帧模式"}>
        <button aria-label="第一帧" className="iv-frames-button" onClick={() => onSeek(0)} title="第一帧" type="button">
          <icons.ChevronLeft aria-hidden="true" size={13} />
          <icons.ChevronLeft aria-hidden="true" size={13} className="iv-frames-double" />
        </button>
        <button aria-label="上一帧" className="iv-frames-button" onClick={() => onStep(-1)} title="上一帧（,）" type="button">
          <icons.ChevronLeft aria-hidden="true" size={15} />
        </button>
        <button aria-label={state.playing ? "暂停" : "播放"} className="iv-frames-button iv-frames-primary" onClick={onPlayPause} title="播放 / 暂停（空格）" type="button">
          {state.playing ? <icons.Pause aria-hidden="true" size={15} /> : <icons.Play aria-hidden="true" size={15} />}
        </button>
        <button aria-label="下一帧" className="iv-frames-button" onClick={() => onStep(1)} title="下一帧（.）" type="button">
          <icons.ChevronRight aria-hidden="true" size={15} />
        </button>
        <button aria-label="最后一帧" className="iv-frames-button" onClick={() => onSeek(state.frameCount - 1)} title="最后一帧" type="button">
          <icons.ChevronRight aria-hidden="true" size={13} />
          <icons.ChevronRight aria-hidden="true" size={13} className="iv-frames-double" />
        </button>
      </div>

      <p className="iv-frames-mode-hint">
        {state.playing
          ? <>正在播放 · 按 <kbd className="iv-keycap">空格</kbd> 暂停后可逐帧查看</>
          : <>逐帧模式 · 用 <kbd className="iv-keycap">,</kbd> <kbd className="iv-keycap">.</kbd> 切换上一帧 / 下一帧</>}
      </p>

      <dl className="iv-frames-facts">
        <div>
          <dt>本帧时长</dt>
          <dd>{formatFrameTime(duration)}</dd>
        </div>
        <div>
          <dt>播放速度</dt>
          <dd>
            <select aria-label="播放速度" className="iv-frames-select" onChange={(event) => onSpeed(Number(event.target.value))} value={state.speed}>
              {PLAYBACK_SPEEDS.map((speed) => (
                <option key={speed} value={speed}>{speed === 1 ? "正常 (1×)" : `${speed}×`}</option>
              ))}
            </select>
          </dd>
        </div>
        <div>
          <dt>循环播放</dt>
          <dd>
            <span className={`plugin-toggle iv-switch${state.loop ? " on" : ""}`}>
              <input aria-label="循环播放" checked={state.loop} onChange={(event) => onLoop(event.target.checked)} role="switch" type="checkbox" />
              <span aria-hidden="true" className="plugin-toggle-track">
                <span className="plugin-toggle-thumb" />
              </span>
            </span>
          </dd>
        </div>
      </dl>
    </div>
  );
}
