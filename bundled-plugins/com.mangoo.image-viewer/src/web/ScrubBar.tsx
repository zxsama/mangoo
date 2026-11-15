import { memo, useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { createPortal } from "react-dom";

import type { ImageEntry } from "../shared/navigation.ts";
import { thumbnailUrl } from "./imageSource.ts";

/**
 * Scrub bar: a thin track spanning the window, one position per picture.
 *
 * - Dragging (or clicking) along it shows a floating preview bubble — the
 *   picture's thumbnail, its name and "n / N" — that follows the pointer.
 *   Nothing heavier than that thumbnail loads while scrubbing; the picture
 *   itself opens on release (or after a short pause), so a 50 000-file
 *   folder scrubs as fast as a 20-file one.
 * - Positions map to pixels, not the other way around: with more pictures
 *   than track pixels, each pixel stands for a range and only that range's
 *   first picture is ever requested (the rest are skipped, as asked).
 * - Hover alone previews too, so the track doubles as a "where am I in this
 *   folder" ruler. Keyboard: Home / End / ←→ (±1) / PgUp PgDn (±10%).
 */

const PREVIEW_PX = 160;

const Preview = memo(function Preview({ entry }: { entry: ImageEntry }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    // A short delay: a fast scrub passes over many positions; only ones the
    // pointer lingers on (>~60 ms) cost a thumbnail request.
    const timer = window.setTimeout(() => {
      thumbnailUrl(entry, PREVIEW_PX).then((value) => {
        if (!cancelled) setUrl(value);
      }, () => undefined);
    }, 60);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [entry]);
  // The previous thumbnail stays until the next one is ready (no blink).
  const shown = useRef<string | null>(null);
  if (url) shown.current = url;
  return shown.current
    ? <img alt="" className="iv-scrub-preview-image" draggable={false} src={shown.current} />
    : <span className="iv-scrub-preview-placeholder">{entry.ext.replace(".", "").toUpperCase()}</span>;
});

export function ScrubBar({
  entries,
  index,
  onSelect
}: {
  entries: readonly ImageEntry[];
  index: number;
  onSelect(index: number): void;
}) {
  const trackRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);
  /** Index under the pointer while hovering / dragging, and its x in px. */
  const [probe, setProbe] = useState<{ index: number; x: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  const frame = useRef(0);
  const pending = useRef<number | null>(null);
  const commitTimer = useRef<number | null>(null);
  const count = entries.length;

  useEffect(() => {
    const track = trackRef.current;
    if (!track) return;
    const observer = new ResizeObserver(() => setWidth(track.clientWidth));
    observer.observe(track);
    setWidth(track.clientWidth);
    return () => observer.disconnect();
  }, []);

  /**
   * Pixel -> picture. With more pictures than pixels each pixel column is a
   * bucket; the bucket's FIRST picture represents it, so positions between
   * buckets are simply never visited.
   */
  const indexAt = useCallback((x: number) => {
    if (count <= 1 || width <= 0) return 0;
    const ratio = Math.min(1, Math.max(0, x / width));
    if (count <= width) return Math.round(ratio * (count - 1));
    const columns = Math.max(1, Math.floor(width));
    const column = Math.min(columns - 1, Math.floor(ratio * columns));
    // The last column is the folder's last picture, so the end is reachable.
    if (column === columns - 1) return count - 1;
    return Math.floor((column * count) / columns);
  }, [count, width]);

  /** Picture -> x, the inverse of indexAt in both modes (thumb under pointer). */
  const xOf = useCallback((at: number) => {
    if (count <= 1) return 0;
    if (count <= width) return (at / (count - 1)) * width;
    if (at >= count - 1) return width;
    const columns = Math.max(1, Math.floor(width));
    return ((Math.floor((at * columns) / count) + 0.5) / columns) * width;
  }, [count, width]);

  const probeAt = useCallback((clientX: number) => {
    const track = trackRef.current;
    if (!track) return null;
    const box = track.getBoundingClientRect();
    const x = Math.min(box.width, Math.max(0, clientX - box.left));
    return { index: indexAt(x), x };
  }, [indexAt]);

  /** Coalesce pointer moves to one update per animation frame. */
  const schedule = useCallback((clientX: number) => {
    pending.current = clientX;
    if (frame.current) return;
    frame.current = window.requestAnimationFrame(() => {
      frame.current = 0;
      if (pending.current === null) return;
      const next = probeAt(pending.current);
      if (next) setProbe((current) => (current && current.index === next.index && Math.abs(current.x - next.x) < 1 ? current : next));
    });
  }, [probeAt]);

  useEffect(() => () => {
    if (frame.current) window.cancelAnimationFrame(frame.current);
    if (commitTimer.current) window.clearTimeout(commitTimer.current);
  }, []);

  // While dragging, a pause of 220 ms on one position opens it (so the main
  // view follows a slow, deliberate scrub), but a moving drag never does.
  useEffect(() => {
    if (!dragging || !probe) return;
    if (commitTimer.current) window.clearTimeout(commitTimer.current);
    commitTimer.current = window.setTimeout(() => {
      commitTimer.current = null;
      if (probe.index !== index) onSelect(probe.index);
    }, 220);
    // A cancelled / finished drag (or a new probe) drops the pending commit.
    return () => {
      if (commitTimer.current) window.clearTimeout(commitTimer.current);
      commitTimer.current = null;
    };
  }, [dragging, probe, index, onSelect]);

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    setDragging(true);
    const next = probeAt(event.clientX);
    if (next) setProbe(next);
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => schedule(event.clientX);
  const onPointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!dragging) return;
    setDragging(false);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    if (commitTimer.current) window.clearTimeout(commitTimer.current);
    const next = probeAt(event.clientX);
    if (next && next.index !== index) onSelect(next.index);
  };
  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const step = Math.max(1, Math.round(count / 10));
    const target = event.key === "Home" ? 0
      : event.key === "End" ? count - 1
      : event.key === "ArrowLeft" ? index - 1
      : event.key === "ArrowRight" ? index + 1
      : event.key === "PageUp" ? index - step
      : event.key === "PageDown" ? index + step
      : null;
    if (target === null) return;
    event.preventDefault();
    event.stopPropagation();
    onSelect(Math.min(count - 1, Math.max(0, target)));
  };

  if (count < 2) return null;
  const shown = probe ?? null;
  const thumbIndex = dragging && shown ? shown.index : index;
  const bubbleEntry = shown ? entries[shown.index] : null;
  // The bubble is portalled to the window (the glass pane clips its overflow),
  // placed in viewport px above the track.
  const trackBox = shown ? trackRef.current?.getBoundingClientRect() ?? null : null;
  const bubbleLeft = trackBox && shown ? Math.min(Math.max(trackBox.left + shown.x, 78), window.innerWidth - 78) : 0;
  const bubbleBottom = trackBox ? window.innerHeight - trackBox.top + 14 : 0;
  const host = trackRef.current?.closest(".iv-window") ?? null;

  return (
    <div
      aria-label="快速浏览文件夹"
      aria-valuemax={count}
      aria-valuemin={1}
      aria-valuenow={index + 1}
      aria-valuetext={`第 ${index + 1} 张，共 ${count} 张`}
      className="iv-scrub"
      data-dragging={dragging ? "true" : "false"}
      data-pointer-glow="off"
      onKeyDown={onKeyDown}
      onLostPointerCapture={() => setDragging(false)}
      onPointerCancel={() => setDragging(false)}
      onPointerDown={onPointerDown}
      onPointerLeave={() => {
        if (!dragging) setProbe(null);
      }}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      role="slider"
      tabIndex={0}
    >
      <div className="iv-scrub-track" ref={trackRef}>
        <div className="iv-scrub-fill" style={{ width: `${xOf(thumbIndex)}px` }} />
        <div className="iv-scrub-thumb" style={{ transform: `translateX(${xOf(thumbIndex)}px)` }} />
        {shown && !dragging ? <div aria-hidden="true" className="iv-scrub-hover" style={{ transform: `translateX(${shown.x}px)` }} /> : null}
      </div>
      {bubbleEntry && host ? createPortal(
        <div aria-hidden="true" className="iv-scrub-bubble" style={{ left: bubbleLeft, bottom: bubbleBottom }}>
          <div className="iv-scrub-preview">
            <Preview entry={bubbleEntry} />
          </div>
          <div className="iv-scrub-caption">
            <span className="iv-scrub-count">{(shown?.index ?? 0) + 1} / {count}</span>
            <span className="iv-scrub-name" title={bubbleEntry.name}>{bubbleEntry.name}</span>
          </div>
        </div>,
        host
      ) : null}
    </div>
  );
}
