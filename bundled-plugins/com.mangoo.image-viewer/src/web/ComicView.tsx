import React, { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import type { ImageEntry } from "../shared/navigation.ts";
import { ViewerImageCache, readyThumbnailUrl, thumbnailUrl, viewerViewportLongPx, type LoadedImage, type RawSource } from "./imageSource.ts";

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
/** Stand-in thumbnail size for pages still loading: the filmstrip / scrub
 * bar size, so a page the strip already showed paints its stand-in at once. */
const COMIC_PREVIEW_PX = 160;
/** A page must stay mounted this long before it requests anything, so a fast
 * glide / scrollbar drag does not queue a decode for every page it passes. */
const LOAD_DELAY_MS = 80;
/** Pages kept mounted (and so loading) beyond the viewport, each way. */
const READ_AHEAD = 4;

const Page = memo(function Page({
  entry,
  cache,
  animatedExtensions,
  rawSource,
  urgent,
  aspect,
  onAspect
}: {
  entry: ImageEntry;
  cache: ViewerImageCache;
  animatedExtensions: readonly string[];
  rawSource: RawSource;
  /** In (or right next to) the viewport: load before the read-ahead pages. */
  urgent: boolean;
  /** Height / width once known (decoded, else thumbnail); drives the box. */
  aspect: number | undefined;
  onAspect(path: string, aspect: number, decoded: boolean): void;
}) {
  const source: RawSource = entry.cameraRaw ? rawSource : "developed";
  const key = ViewerImageCache.key(entry, false, source);
  // Like a web page: a page that was already loaded paints synchronously on
  // mount (no blank frame when it scrolls back in, or when zoom remounts
  // nothing — pages are keyed by path, so zoom never remounts them).
  const [image, setImage] = useState<LoadedImage | null>(() => cache.peek(key));
  const [failed, setFailed] = useState(false);
  /** Low-res stand-in (the filmstrip thumbnail) until the page itself lands. */
  const [preview, setPreview] = useState<string | null>(() => readyThumbnailUrl(entry, COMIC_PREVIEW_PX));
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  const urgentRef = useRef(urgent);
  urgentRef.current = urgent;
  /** The started request's priority, to upgrade it once it becomes visible. */
  const started = useRef<"interactive" | "resource" | null>(null);
  const [retry, setRetry] = useState(0);

  useEffect(() => {
    let cancelled = false;
    started.current = null;
    const cached = cache.peek(key);
    if (cached) {
      setImage(cached);
      onAspect(entry.path, cached.height / Math.max(1, cached.width), true);
      return;
    }
    setImage(null);
    // Read-ahead pages wait a moment (skipped entirely when only flashed past
    // during a glide); the page in view starts at once.
    const begin = () => {
      if (cancelled) return;
      // The thumbnail lands in a few ms and fixes the page's height early, so
      // the strip stops shifting long before the full page decodes.
      void thumbnailUrl(entry, COMIC_PREVIEW_PX, urgentRef.current).then((url) => {
        if (!cancelled) setPreview(url);
      }, () => undefined);
      const priority = urgentRef.current ? "interactive" : "resource";
      started.current = priority;
      cache.load(entry, { viewportLongPx: viewerViewportLongPx(), full: false, animatedExtensions, priority, rawSource: source }).then((loaded) => {
        if (cancelled) return;
        setImage(loaded);
        onAspect(entry.path, loaded.height / Math.max(1, loaded.width), true);
      }, () => {
        if (!cancelled) setFailed(true);
      });
    };
    const timer = urgentRef.current && retry === 0 ? 0 : window.setTimeout(begin, LOAD_DELAY_MS);
    if (!timer) begin();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [key, cache, animatedExtensions, onAspect, retry]);

  // A read-ahead page scrolled into view while its request is still queued at
  // "resource": ask again at "interactive" (the cache joins the same fetch if
  // it is already in flight, Core re-prioritises a queued one).
  useEffect(() => {
    if (!urgent || image || failed || started.current !== "resource") return;
    started.current = "interactive";
    let cancelled = false;
    cache.load(entry, { viewportLongPx: viewerViewportLongPx(), full: false, animatedExtensions, priority: "interactive", rawSource: source }).then((loaded) => {
      if (cancelled) return;
      setImage(loaded);
      onAspect(entry.path, loaded.height / Math.max(1, loaded.width), true);
    }, () => undefined);
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [urgent]);

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

  // Width comes from CSS (var(--iv-comic-page-width)), height from the aspect:
  // a zoom step changes ONE custom property on the track instead of
  // re-rendering every page, so pages never repaint blank while zooming.
  return (
    <div
      className="iv-comic-page"
      data-loaded={image ? "true" : "false"}
      data-measured={aspect ? "true" : "false"}
      style={aspect ? { aspectRatio: `1 / ${aspect}` } : undefined}
    >
      {image?.kind === "bitmap" ? <canvas className="iv-comic-canvas" ref={canvasRef} /> : null}
      {image?.kind === "animated" ? <img alt="" className="iv-comic-canvas" draggable={false} src={image.url} /> : null}
      {!image && preview && !failed ? (
        <img
          alt=""
          className="iv-comic-canvas iv-comic-preview"
          draggable={false}
          onError={() => setPreview(null)}
          onLoad={(event) => {
            const img = event.currentTarget;
            if (img.naturalWidth > 0) onAspect(entry.path, img.naturalHeight / img.naturalWidth, false);
          }}
          src={preview}
        />
      ) : null}
      {!image && (!preview || failed) ? (
        <span
          className="iv-comic-placeholder"
          data-failed={failed ? "true" : "false"}
          onDoubleClick={failed ? () => {
            setFailed(false);
            setRetry((value) => value + 1);
          } : undefined}
          title={failed ? "双击重试" : undefined}
        >
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
  /** True while a programmatic scroll settles: not a page change by the user. */
  /** >0 while a programmatic scroll settles (counter: jumps and zoom overlap). */
  const settling = useRef(0);
  useEffect(() => {
    zoomRef.current = zoom;
  }, [zoom]);
  const [viewportWidth, setViewportWidth] = useState(0);
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
  const virtualizer = useVirtualizer({ count: entries.length, getScrollElement, estimateSize, overscan: READ_AHEAD, getItemKey });

  // Width or gap changed: every size changes. Keep the reading position by
  // anchoring on (page under the pointer / window middle, fraction into it)
  // and restore it BEFORE paint, so a zoom step never shows a frame with new
  // widths at old offsets (the flicker) or jumps elsewhere in the strip.
  const anchor = useRef<{ index: number; fraction: number; screenY: number; scrollTop: number } | null>(null);
  const captureAnchor = useCallback((screenY?: number) => {
    const element = scrollRef.current;
    if (!element) return;
    const y = screenY ?? element.clientHeight / 2;
    const at = element.scrollTop + y;
    const item = virtualizer.getVirtualItems().find((candidate) => candidate.start <= at && candidate.end > at);
    if (!item) return;
    anchor.current = { index: item.index, fraction: (at - item.start) / Math.max(1, item.size), screenY: y, scrollTop: element.scrollTop };
  }, [virtualizer]);
  const measuredWidth = useRef(pageWidth);
  const measuredGap = useRef(gap);
  useLayoutEffect(() => {
    if (measuredWidth.current === pageWidth && measuredGap.current === gap) return;
    // An anchor from a zoom step that changed nothing (rounding, the 120px
    // floor) is stale once the user scrolled: take a fresh one.
    if (anchor.current && Math.abs(anchor.current.scrollTop - (scrollRef.current?.scrollTop ?? 0)) > 1) anchor.current = null;
    if (!anchor.current) captureAnchor();
    measuredWidth.current = pageWidth;
    measuredGap.current = gap;
    virtualizer.measure();
    const saved = anchor.current;
    anchor.current = null;
    const element = scrollRef.current;
    if (!saved || !element) return;
    // Rebuild the measurements (lazy after measure()) at the new width.
    virtualizer.getTotalSize();
    const item = virtualizer.measurementsCache[saved.index];
    if (!item) return;
    settling.current += 1;
    element.scrollTop = item.start + saved.fraction * item.size - saved.screenY;
    window.requestAnimationFrame(() => {
      settling.current = Math.max(0, settling.current - 1);
    });
  }, [pageWidth, gap, virtualizer, captureAnchor]);

  const indexOf = useMemo(() => new Map(entries.map((entry, at) => [entry.path, at])), [entries]);
  /** Paths whose aspect came from the decoded page (a thumbnail may not override). */
  const decodedAspects = useRef(new Set<string>());
  const [, setAspectVersion] = useState(0);
  const onAspect = useCallback((path: string, aspect: number, decoded: boolean) => {
    const known = knownAspects.get(path);
    if (!decoded && decodedAspects.current.has(path)) return;
    if (decoded) decodedAspects.current.add(path);
    // Thumbnail vs decoded rounding: under 0.5% is not worth a reflow.
    if (known !== undefined && Math.abs(known - aspect) / known < 0.005) return;
    knownAspects.set(path, aspect);
    setAspectVersion((value) => value + 1);
    if (learned.current.length < 64) {
      learned.current.push(aspect);
      const sorted = [...learned.current].sort((a, b) => a - b);
      setTypicalAspect(sorted[Math.floor(sorted.length / 2)]);
    }
    const at = indexOf.get(path) ?? -1;
    if (at >= 0) {
      // Re-measure that page only; the virtualizer keeps the scroll anchored
      // to the first visible page.
      virtualizer.resizeItem(at, aspect * measuredWidth.current + measuredGap.current);
    }
  }, [indexOf, virtualizer]);

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
        if (settling.current > 0) return;
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

  // Drag glide (declared here: jumps stop it).
  const glide = useRef(0);
  const [grabbing, setGrabbing] = useState(false);
  const stopGlide = useCallback(() => {
    if (glide.current) window.cancelAnimationFrame(glide.current);
    glide.current = 0;
  }, []);

  // Entering the mode, or a jump request (← →, filmstrip, scrub bar).
  // Waits for the first width measurement: before it, every page estimate is
  // the 120px minimum and the offset would land far too early. Until the
  // jump settles, scroll events are not reported as a page change.
  const jumped = useRef<string | null>(null);
  /** Tokens issued before this view mounted belong to an earlier session. */
  const mountedToken = useRef(jump?.token ?? 0);
  useEffect(() => {
    if (viewportWidth <= 0) return;
    // A jump the parent issued after this view mounted, else the current
    // picture (entering the mode by any path, or a new folder opened in it).
    const fresh = jump && jump.token > mountedToken.current ? jump : null;
    const target = fresh?.index ?? index;
    const token = [fresh?.token ?? "mount", entries[0]?.path ?? "", entries.length].join("|");
    if (jumped.current === token) return;
    jumped.current = token;
    stopGlide();
    lastReported.current = target;
    settling.current += 1;
    virtualizer.scrollToIndex(target, { align: "start" });
    // Pages measured on the way in move the offset: re-align once more.
    const frame = window.requestAnimationFrame(() => {
      virtualizer.scrollToIndex(target, { align: "start" });
      window.requestAnimationFrame(() => {
        settling.current = Math.max(0, settling.current - 1);
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
      const previous = zoomRef.current;
      const next = Math.min(3, Math.max(0.2, previous * factor));
      if (next === previous) return;
      zoomRef.current = next;
      // The point under the pointer stays put (restored in the layout effect).
      captureAnchor(event.clientY - element.getBoundingClientRect().top);
      onZoom(next);
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, [onZoom, captureAnchor]);

  // Left-button drag scrolls the strip like a touch screen / reader app; a
  // quick flick keeps gliding (inertia), a press-and-hold stops it.
  const grab = useRef<{ id: number; y: number; top: number; t: number; v: number; moved: boolean } | null>(null);
  useEffect(() => {
    const element = scrollRef.current;
    element?.addEventListener("iv-comic-stop-glide", stopGlide);
    return () => {
      element?.removeEventListener("iv-comic-stop-glide", stopGlide);
      stopGlide();
    };
  }, [stopGlide]);
  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || event.pointerType === "touch") return;
    const element = scrollRef.current;
    if (!element) return;
    // The native scrollbars keep their own behaviour.
    const box = element.getBoundingClientRect();
    if (event.clientX >= box.left + element.clientWidth || event.clientY >= box.top + element.clientHeight) return;
    stopGlide();
    element.setPointerCapture(event.pointerId);
    grab.current = { id: event.pointerId, y: event.clientY, top: element.scrollTop, t: performance.now(), v: 0, moved: false };
  };
  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    const state = grab.current;
    const element = scrollRef.current;
    if (!state || !element || state.id !== event.pointerId) return;
    const dy = event.clientY - state.y;
    if (!state.moved && Math.abs(dy) < 3) return;
    if (!state.moved) setGrabbing(true);
    state.moved = true;
    const now = performance.now();
    const before = element.scrollTop;
    element.scrollTop = state.top - dy;
    const dt = Math.max(1, now - state.t);
    // Smoothed velocity in px/ms (scroll direction).
    state.v = state.v * 0.6 + ((element.scrollTop - before) / dt) * 0.4;
    state.t = now;
  };
  const onPointerUp = (event: React.PointerEvent<HTMLDivElement>) => {
    const state = grab.current;
    const element = scrollRef.current;
    if (!state || !element || state.id !== event.pointerId) return;
    grab.current = null;
    setGrabbing(false);
    if (element.hasPointerCapture(event.pointerId)) element.releasePointerCapture(event.pointerId);
    if (event.type === "pointercancel") return;
    // Released after a pause: no glide.
    // Capped (px/ms) so a burst of synthetic / high-rate events cannot fling
    // the strip across the whole folder.
    let velocity = performance.now() - state.t > 80 ? 0 : Math.max(-4, Math.min(4, state.v));
    if (!state.moved || Math.abs(velocity) < 0.05) return;
    let last = performance.now();
    const step = (now: number) => {
      const dt = now - last;
      last = now;
      element.scrollTop += velocity * dt;
      velocity *= Math.pow(0.995, dt);
      glide.current = Math.abs(velocity) > 0.02 ? window.requestAnimationFrame(step) : 0;
    };
    glide.current = window.requestAnimationFrame(step);
  };

  const items = virtualizer.getVirtualItems();
  // The page in the window's middle (and its neighbours) load first.
  const middle = (scrollRef.current?.scrollTop ?? 0) + (scrollRef.current?.clientHeight ?? 0) / 2;
  const centre = items.find((item) => item.start <= middle && item.end > middle)?.index ?? index;
  const total = virtualizer.getTotalSize();
  const style = useMemo(() => ({
    height: `${total}px`,
    width: `${Math.max(pageWidth, viewportWidth)}px`,
    "--iv-comic-page-width": `${pageWidth}px`
  }) as React.CSSProperties, [total, pageWidth, viewportWidth]);

  return (
    <div
      className="iv-comic"
      data-grabbing={grabbing ? "true" : "false"}
      onKeyDownCapture={stopGlide}
      onLostPointerCapture={() => {
        grab.current = null;
        setGrabbing(false);
      }}
      onPointerCancel={onPointerUp}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onWheelCapture={stopGlide}
      ref={scrollRef}
      style={{ background: props.background }}
      tabIndex={-1}
    >
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
              <Page animatedExtensions={animatedExtensions} aspect={knownAspects.get(entry.path)} cache={cache} entry={entry} onAspect={onAspect} rawSource={rawSource} urgent={Math.abs(item.index - centre) <= 1} />
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** Scroll helpers for the window's key handler. */
export function comicScrollBy(element: HTMLElement | null, fraction: number): void {
  // A drag glide in progress would cancel the smooth scroll every frame.
  element?.dispatchEvent(new Event("iv-comic-stop-glide"));
  element?.scrollBy({ top: element.clientHeight * fraction, behavior: "smooth" });
}
