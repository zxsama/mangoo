import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import type { ImageEntry } from "../shared/navigation.ts";
import { ViewerImageCache, viewerViewportLongPx, type LoadedImage, type RawSource } from "./imageSource.ts";

/**
 * Comic / webtoon mode: every picture of the folder in ONE continuous
 * vertical strip, read by scrolling, like a webtoon in a browser.
 *
 * - Width: fills the window by default ("铺满宽度"); Ctrl+wheel, + / - and
 *   the toolbar zoom it (narrower or wider than the window), 0 returns to
 *   full width. The zoom is a column width, so tall pages keep their aspect.
 * - Gap between pages: none by default (seamless strips), adjustable.
 * - Navigation: wheel / trackpad / PgUp PgDn / Space scroll; ← → and
 *   Shift+PgUp/PgDn jump to the previous / next picture's top; Home / End.
 * - The picture whose area covers the middle of the window is the "current"
 *   one: it drives the title, counter, info panel and filmstrip.
 * - Virtualised: only pages near the viewport are mounted and decoded;
 *   unknown heights are estimated from the folder's first measured pages and
 *   corrected as they load, keeping the reading position anchored.
 */

export interface ComicViewProps {
  entries: readonly ImageEntry[];
  index: number;
  cache: ViewerImageCache;
  animatedExtensions: readonly string[];
  /** RAW pages: developed sensor data or the embedded JPEG (the user's pick). */
  rawSource: RawSource;
  /** Column width relative to the window (1 = fill the width). */
  zoom: number;
  gap: number;
  background: string;
  onCurrentChange(index: number): void;
  onZoom(next: number): void;
  onActivity(): void;
  /** Ask to scroll a picture to the top (set from outside). */
  jump: { index: number; token: number } | null;
}

/** Aspect ratios (height / width) learned so far, per path. */
const knownAspects = new Map<string, number>();

const Page = memo(function Page({
  entry,
  cache,
  animatedExtensions,
  width,
  rawSource,
  onAspect
}: {
  entry: ImageEntry;
  cache: ViewerImageCache;
  animatedExtensions: readonly string[];
  width: number;
  rawSource: RawSource;
  onAspect(path: string, aspect: number): void;
}) {
  const [image, setImage] = useState<LoadedImage | null>(null);
  const [failed, setFailed] = useState(false);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    let cancelled = false;
    const source: RawSource = entry.cameraRaw ? rawSource : "developed";
    const cached = cache.peek(ViewerImageCache.key(entry, false, source));
    if (cached) {
      setImage(cached);
      onAspect(entry.path, cached.height / Math.max(1, cached.width));
      return;
    }
    // Pages decode at the strip's display width, not the window's long side.
    cache.load(entry, { viewportLongPx: viewerViewportLongPx(), full: false, animatedExtensions, priority: "interactive", rawSource: source }).then((loaded) => {
      if (cancelled) return;
      setImage(loaded);
      onAspect(entry.path, loaded.height / Math.max(1, loaded.width));
    }, () => {
      if (!cancelled) setFailed(true);
    });
    return () => {
      cancelled = true;
    };
  }, [entry, cache, animatedExtensions, onAspect, rawSource]);

  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !image || image.kind !== "bitmap") return;
    const bitmap = image.bitmap;
    if (canvas.width !== bitmap.width || canvas.height !== bitmap.height) {
      canvas.width = bitmap.width;
      canvas.height = bitmap.height;
    }
    const context = canvas.getContext("2d");
    if (!context) return;
    context.imageSmoothingQuality = "high";
    try {
      context.drawImage(bitmap, 0, 0);
    } catch {
      // Bitmap evicted between render and paint: the next load repaints.
    }
  }, [image]);

  const aspect = knownAspects.get(entry.path);
  return (
    <div className="iv-comic-page" data-loaded={image ? "true" : "false"} style={{ width, height: aspect ? width * aspect : undefined }}>
      {image?.kind === "bitmap" ? <canvas className="iv-comic-canvas" ref={canvasRef} /> : null}
      {image?.kind === "animated" ? <img alt="" className="iv-comic-canvas" draggable={false} src={image.url} /> : null}
      {!image ? (
        <span className="iv-comic-placeholder" data-failed={failed ? "true" : "false"}>
          {failed ? `无法显示 ${entry.name}` : entry.name}
        </span>
      ) : null}
    </div>
  );
});

export function ComicView(props: ComicViewProps) {
  const { entries, index, cache, animatedExtensions, rawSource, zoom, gap, onCurrentChange, onZoom, onActivity, jump } = props;
  const scrollRef = useRef<HTMLDivElement | null>(null);
  /** Latest zoom, also advanced by wheel events before React re-renders. */
  const zoomRef = useRef(zoom);
  useEffect(() => {
    zoomRef.current = zoom;
  }, [zoom]);
  const [viewportWidth, setViewportWidth] = useState(0);
  const [, setVersion] = useState(0);
  const pageWidth = Math.max(120, Math.round(viewportWidth * zoom));

  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setViewportWidth(element.clientWidth));
    observer.observe(element);
    setViewportWidth(element.clientWidth);
    return () => observer.disconnect();
  }, []);

  /**
   * Typical aspect of this folder, for pages not measured yet: the median of
   * the first (at most 64) aspects learned, recomputed only when one is
   * learned, never per estimate (estimates run per page).
   */
  const [typicalAspect, setTypicalAspect] = useState(1.4);
  const learned = useRef<number[]>([]);

  const getItemKey = useCallback((at: number) => entries[at]?.path ?? at, [entries]);
  const estimateSize = useCallback(
    (at: number) => (knownAspects.get(entries[at]?.path ?? "") ?? typicalAspect) * pageWidth + gap,
    [entries, typicalAspect, pageWidth, gap]
  );
  const getScrollElement = useCallback(() => scrollRef.current, []);
  const virtualizer = useVirtualizer({ count: entries.length, getScrollElement, estimateSize, overscan: 3, getItemKey });

  // Width or gap changed: every size changes; keep the reading position.
  useEffect(() => {
    virtualizer.measure();
  }, [pageWidth, gap, virtualizer]);

  const onAspect = useCallback((path: string, aspect: number) => {
    if (knownAspects.get(path) === aspect) return;
    knownAspects.set(path, aspect);
    if (learned.current.length < 64) {
      learned.current.push(aspect);
      const sorted = [...learned.current].sort((a, b) => a - b);
      setTypicalAspect(sorted[Math.floor(sorted.length / 2)]);
    }
    const at = entries.findIndex((entry) => entry.path === path);
    if (at >= 0) {
      // Re-measure that page only; the virtualizer keeps the scroll anchored
      // to the first visible page.
      virtualizer.resizeItem(at, aspect * pageWidth + gap);
      setVersion((value) => value + 1);
    }
  }, [entries, virtualizer, pageWidth, gap]);

  // Current page = the one covering the window's vertical middle.
  const lastReported = useRef(index);
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    let frame = 0;
    const onScroll = () => {
      onActivity();
      if (frame) return;
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        if (settling.current) return;
        // At the very top / bottom the first / last page is current even
        // when it cannot reach the middle of the window.
        const atEnd = element.scrollTop + element.clientHeight >= element.scrollHeight - 2;
        const middle = element.scrollTop + element.clientHeight / 2;
        const found = element.scrollTop <= 1
          ? 0
          : atEnd
            ? entries.length - 1
            : virtualizer.getVirtualItems().find((candidate) => candidate.start <= middle && candidate.end > middle)?.index;
        if (found !== undefined && found !== lastReported.current) {
          lastReported.current = found;
          onCurrentChange(found);
        }
      });
    };
    element.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      element.removeEventListener("scroll", onScroll);
      if (frame) window.cancelAnimationFrame(frame);
    };
  }, [virtualizer, onCurrentChange, onActivity, entries.length]);

  // Entering the mode, or a jump request (← →, filmstrip, scrub bar).
  // Waits for the first width measurement: before it, every page estimate is
  // the 120px minimum and the offset would land far too early. Until the
  // jump settles, scroll events are not reported as a page change.
  const jumped = useRef<string | null>(null);
  /** Tokens issued before this view mounted belong to an earlier session. */
  const mountedToken = useRef(jump?.token ?? 0);
  const settling = useRef(false);
  useEffect(() => {
    if (viewportWidth <= 0) return;
    // A jump the parent issued after this view mounted, else the current
    // picture (entering the mode by any path, or a new folder opened in it).
    const fresh = jump && jump.token > mountedToken.current ? jump : null;
    const target = fresh?.index ?? index;
    const token = [fresh?.token ?? "mount", entries[0]?.path ?? "", entries.length].join("|");
    if (jumped.current === token) return;
    jumped.current = token;
    lastReported.current = target;
    settling.current = true;
    virtualizer.scrollToIndex(target, { align: "start" });
    // Pages measured on the way in move the offset: re-align once more.
    const frame = window.requestAnimationFrame(() => {
      virtualizer.scrollToIndex(target, { align: "start" });
      window.requestAnimationFrame(() => {
        settling.current = false;
      });
    });
    return () => window.cancelAnimationFrame(frame);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [jump?.token, virtualizer, viewportWidth > 0, entries]);

  // Ctrl+wheel zooms the column width around the pointer's page position.
  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      if (!event.ctrlKey) return;
      event.preventDefault();
      const factor = Math.exp(-Math.max(-240, Math.min(240, event.deltaY)) * 0.0015);
      const before = element.scrollTop + event.clientY - element.getBoundingClientRect().top;
      const previous = zoomRef.current;
      const next = Math.min(3, Math.max(0.2, previous * factor));
      zoomRef.current = next;
      onZoom(next);
      // Keep the point under the pointer in place: heights scale with width.
      const ratio = next / previous;
      window.requestAnimationFrame(() => {
        element.scrollTop = before * ratio - (event.clientY - element.getBoundingClientRect().top);
      });
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, [onZoom]);

  const items = virtualizer.getVirtualItems();
  const total = virtualizer.getTotalSize();
  const style = useMemo(() => ({ height: `${total}px`, width: `${Math.max(pageWidth, viewportWidth)}px` }), [total, pageWidth, viewportWidth]);

  return (
    <div className="iv-comic" ref={scrollRef} style={{ background: props.background }} tabIndex={-1}>
      <div className="iv-comic-track" style={style}>
        {items.map((item) => {
          const entry = entries[item.index];
          return (
            <div
              className="iv-comic-slot"
              data-index={item.index}
              key={item.key}
              style={{ transform: `translate(${Math.max(0, (viewportWidth - pageWidth) / 2)}px, ${item.start}px)`, paddingBottom: gap }}
            >
              <Page animatedExtensions={animatedExtensions} cache={cache} entry={entry} onAspect={onAspect} rawSource={rawSource} width={pageWidth} />
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** Scroll helpers for the window's key handler. */
export function comicScrollBy(element: HTMLElement | null, fraction: number): void {
  element?.scrollBy({ top: element.clientHeight * fraction, behavior: "smooth" });
}
