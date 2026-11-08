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
 * Compact animation control: a frame readout, a scrubber, and one row of
 * controls (step back, play/pause, step forward, speed, loop).
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
  const duration = state.durations[state.index] ?? 0;
  const elapsed = state.startTimes[state.index] ?? 0;
  return (
    <div className="iv-frames">
      <div className="iv-frames-top" aria-live="polite">
        <strong>{state.index + 1} / {state.frameCount}</strong>
        <span title="本帧时长 · 当前位置 / 总时长">
          本帧 {formatFrameTime(duration)} · {formatFrameTime(elapsed)} / {formatFrameTime(state.totalMs)}
        </span>
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
      <div className="iv-frames-row">
        <button aria-label="上一帧" className="iv-frames-button" data-pointer-glow="off" onClick={() => onStep(-1)} title="上一帧（,）" type="button">
          <icons.ChevronLeft aria-hidden="true" size={14} />
        </button>
        <button aria-label={state.playing ? "暂停" : "播放"} className="iv-frames-button iv-frames-primary" data-pointer-glow="off" onClick={onPlayPause} title={state.playing ? "暂停（空格）" : "播放（空格）"} type="button">
          {state.playing ? <icons.Pause aria-hidden="true" size={14} /> : <icons.Play aria-hidden="true" size={14} />}
        </button>
        <button aria-label="下一帧" className="iv-frames-button" data-pointer-glow="off" onClick={() => onStep(1)} title="下一帧（.）" type="button">
          <icons.ChevronRight aria-hidden="true" size={14} />
        </button>
        <span className="iv-frames-spacer" />
        <select aria-label="播放速度" className="iv-frames-select" onChange={(event) => onSpeed(Number(event.target.value))} title="播放速度" value={state.speed}>
          {PLAYBACK_SPEEDS.map((speed) => (
            <option key={speed} value={speed}>{`${speed}×`}</option>
          ))}
        </select>
        <button
          aria-label="循环播放"
          aria-pressed={state.loop}
          className="iv-frames-button iv-frames-loop"
          data-pointer-glow="off"
          onClick={() => onLoop(!state.loop)}
          title={state.loop ? "循环播放：开" : "循环播放：关"}
          type="button"
        >
          <icons.RefreshCw aria-hidden="true" size={13} />
        </button>
      </div>
    </div>
  );
}
