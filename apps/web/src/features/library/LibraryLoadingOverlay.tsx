import { Loader2 } from "lucide-react";
import { LiquidGlassSurface } from "../../design/liquid-glass";
import { isErrorPhase, type LibraryLoadPhase } from "../../core/libraryTypes";

const PHASE_TEXT: Record<string, string> = {
  "starting-core": "正在启动核心…",
  "opening-library": "正在打开库…",
  "loading-media": "正在加载媒体…"
};

export function LibraryLoadingOverlay({
  phase,
  onRetry,
  onSwitchLibrary
}: {
  phase: LibraryLoadPhase;
  onRetry: () => void;
  onSwitchLibrary: (source: HTMLButtonElement, autoFocus: boolean) => void;
}) {
  if (phase === "ready") return null;
  const error = isErrorPhase(phase);
  const mediaLoading = phase === "loading-media";
  // During the actual teardown/restart window (starting-core / opening-library /
  // error) the old library's Core is gone and its content must not show — cover
  // the stage with an opaque backdrop so only the loading/switch UI is visible.
  // `loading-media` overlays content that is already being repopulated for the
  // correct library, so it stays a light, non-blocking progress card.
  const solidBackdrop = !mediaLoading;
  return (
    <div
      className={`library-loading-overlay${solidBackdrop ? " library-loading-overlay--solid" : ""}`}
      aria-live="polite"
      aria-busy={!error}
    >
      <LiquidGlassSurface
        as="div"
        className={`library-loading-card${mediaLoading ? " library-loading-card--progress" : ""}`}
        tone="elevated"
      >
        {error ? (
          <>
            <div className="library-loading-error-text">{phase.message}</div>
            <div className="library-loading-actions">
              <button type="button" onClick={onRetry}>重试</button>
              <button
                type="button"
                onClick={(event) => onSwitchLibrary(event.currentTarget, event.detail === 0)}
              >
                切换其它库
              </button>
            </div>
          </>
        ) : (
          <>
            <Loader2 className="library-loading-spinner" aria-hidden="true" size={22} />
            <div className="library-loading-text">{PHASE_TEXT[phase] ?? "加载中…"}</div>
          </>
        )}
      </LiquidGlassSurface>
    </div>
  );
}
