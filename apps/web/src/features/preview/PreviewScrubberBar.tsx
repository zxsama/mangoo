import { Pin } from "lucide-react";
import { useCallback, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";

import { LiquidGlassButton, LiquidGlassSurface } from "../../design/liquid-glass";

import {
  scrubberFractionForIndex,
  scrubberIndexForPointerX,
  shouldShowScrubber
} from "./previewScrubber";

interface PreviewScrubberProps {
  /** 1-based position, the same value the titlebar renders. */
  current: number;
  /** Total images in the current folder, flattened when includeDescendants. */
  total: number;
  pinned: boolean;
  pointerNearBottom: boolean;
  previewOpen: boolean;
  /** Commits a 0-based index. Called on every distinct index during a drag. */
  onScrubTo: (index: number) => void;
  onTogglePinned: () => void;
}

// A floating browsing control bar for the centre preview, modelled on
// BandiView's bottom control bar: a track, a draggable thumb, a position
// tooltip, and a pin that defeats auto-hide (BandiView calls it lockControlBar).
//
// The thumb position and the displayed image never disagree: a drag commits its
// index immediately rather than deferring to pointerup, and the thumb is drawn
// from the committed position rather than from local drag state.
export function PreviewScrubber({
  current,
  total,
  pinned,
  pointerNearBottom,
  previewOpen,
  onScrubTo,
  onTogglePinned
}: PreviewScrubberProps) {
  const trackRef = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  // The last index handed to onScrubTo, so pointer movement within one pixel of
  // track (which re-computes the same index) does no work.
  const lastCommittedRef = useRef<number | null>(null);

  const visible = shouldShowScrubber({ previewOpen, pinned, pointerNearBottom, dragging });

  const commitFromPointer = useCallback(
    (clientX: number) => {
      const track = trackRef.current;
      if (!track) {
        return;
      }
      const rect = track.getBoundingClientRect();
      const index = scrubberIndexForPointerX(
        clientX,
        { left: rect.left, width: rect.width },
        total
      );
      if (lastCommittedRef.current === index) {
        return;
      }
      lastCommittedRef.current = index;
      onScrubTo(index);
    },
    [onScrubTo, total]
  );

  const handlePointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (event.button !== 0 || total <= 0) {
        return;
      }
      event.preventDefault();
      // Capture so the drag survives the pointer outrunning the thumb.
      event.currentTarget.setPointerCapture(event.pointerId);
      setDragging(true);
      lastCommittedRef.current = null;
      commitFromPointer(event.clientX);
    },
    [commitFromPointer, total]
  );

  const handlePointerMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (!dragging) {
        return;
      }
      commitFromPointer(event.clientX);
    },
    [commitFromPointer, dragging]
  );

  const endDrag = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    setDragging(false);
  }, []);

  if (total <= 0) {
    return null;
  }

  // `current` is 1-based (it is the same value the titlebar renders).
  const fraction = scrubberFractionForIndex(Math.max(0, current - 1), total);
  const thumbPercent = `${fraction * 100}%`;

  return (
    <div
      className="preview-scrubber"
      data-no-drag="true"
      data-preview-scrubber-visible={visible ? "true" : "false"}
    >
      <LiquidGlassSurface className="preview-scrubber-shell" tone="control">
        <div
          aria-label="Seek through folder"
          aria-valuemax={total}
          aria-valuemin={1}
          aria-valuenow={current}
          aria-valuetext={`${current} / ${total}`}
          className="preview-scrubber-track"
          onPointerCancel={endDrag}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={endDrag}
          ref={trackRef}
          role="slider"
          tabIndex={-1}
        >
          <div
            aria-hidden="true"
            className="preview-scrubber-thumb"
            style={{ left: thumbPercent }}
          />
          <div
            aria-hidden="true"
            className="preview-scrubber-tooltip"
            style={{ left: thumbPercent }}
          >
            {current}/{total}
          </div>
        </div>
        <LiquidGlassButton
          active={pinned}
          aria-label={pinned ? "Unpin control bar" : "Pin control bar"}
          aria-pressed={pinned}
          className="preview-scrubber-pin"
          onClick={onTogglePinned}
          title={pinned ? "Unpin control bar" : "Pin control bar"}
          tone="control"
          type="button"
        >
          <Pin aria-hidden="true" size={14} />
        </LiquidGlassButton>
      </LiquidGlassSurface>
    </div>
  );
}
