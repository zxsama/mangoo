import { Pause, Play, Volume2, VolumeX } from "../../design/icons";
import { ParameterSlider } from "../../design/ParameterSlider";
import { LiquidGlassButton } from "../../design/liquid-glass";
import {
  EMPTY_VIDEO_PLAYBACK_SNAPSHOT,
  formatMediaTime,
  playbackProgress,
  type VideoPlaybackSnapshot
} from "./videoPlayback";

export function VideoPlaybackControls({
  disabled,
  onSeek,
  onToggleMuted,
  onTogglePlayback,
  onVolumeChange,
  state
}: {
  disabled: boolean;
  onSeek: (time: number) => void;
  onToggleMuted: () => void;
  onTogglePlayback: () => void;
  onVolumeChange: (volume: number) => void;
  state: VideoPlaybackSnapshot;
}) {
  const duration = Number.isFinite(state.duration) && state.duration > 0 ? state.duration : 0;
  const progress = playbackProgress(state.currentTime, duration);

  return (
    <div
      aria-label="Video playback controls"
      className="video-playback-controls"
      data-skip-preview-pan="true"
      role="group"
    >
      <LiquidGlassButton
        aria-label={state.playing ? "Pause video" : "Play video"}
        className="titlebar-icon-button video-playback-button"
        disabled={disabled}
        onClick={onTogglePlayback}
        title={state.playing ? "Pause" : "Play"}
        tone="control"
        variant="clear"
      >
        {state.playing ? <Pause aria-hidden="true" size={15} /> : <Play aria-hidden="true" size={15} />}
      </LiquidGlassButton>

      <span className="video-playback-time">{formatMediaTime(state.currentTime)}</span>
      <label className="video-playback-range-control" data-pointer-glow="off">
        <input
          aria-label="Video position"
          disabled={disabled || duration <= 0}
          max="1000"
          min="0"
          onChange={(event) => onSeek((Number(event.currentTarget.value) / 1000) * duration)}
          step="1"
          type="range"
          value={Math.round(progress * 1000)}
        />
      </label>
      <span className="video-playback-time">{formatMediaTime(duration)}</span>

      <LiquidGlassButton
        aria-label={state.muted ? "Unmute video" : "Mute video"}
        className="titlebar-icon-button video-playback-button"
        disabled={disabled}
        onClick={onToggleMuted}
        title={state.muted ? "Unmute" : "Mute"}
        tone="control"
        variant="clear"
      >
        {state.muted || state.volume === 0 ? (
          <VolumeX aria-hidden="true" size={15} />
        ) : (
          <Volume2 aria-hidden="true" size={15} />
        )}
      </LiquidGlassButton>
      <label className="video-volume-range-control" data-pointer-glow="off">
        <ParameterSlider
          aria-label="Video volume"
          disabled={disabled}
          max="1"
          min="0"
          onChange={onVolumeChange}
          resetLabel={`${EMPTY_VIDEO_PLAYBACK_SNAPSHOT.volume * 100}%`}
          resetValue={EMPTY_VIDEO_PLAYBACK_SNAPSHOT.volume}
          step="0.01"
          value={state.volume}
        />
      </label>
      {state.autoplayBlocked ? (
        <span className="video-autoplay-status" role="status">Click play to start</span>
      ) : null}
    </div>
  );
}
