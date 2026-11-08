import { memo, useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

import type { ImageEntry } from "../shared/navigation.ts";
import { thumbnailUrl } from "./imageSource.ts";

const ITEM_WIDTH = 76;
const ITEM_GAP = 6;
const THUMBNAIL_PX = 160;

const FilmstripThumb = memo(function FilmstripThumb({ entry }: { entry: ImageEntry }) {
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setUrl(null);
    setFailed(false);
    // Defer slightly so a fast scrub through the strip does not decode
    // every thumbnail it passes over.
    const timer = window.setTimeout(() => {
      thumbnailUrl(entry, THUMBNAIL_PX).then(
        (value) => {
          if (!cancelled) setUrl(value);
        },
        () => {
          if (!cancelled) setFailed(true);
        }
      );
    }, 60);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [entry]);
  if (url) return <img alt="" className="iv-film-image" draggable={false} src={url} />;
  return <span className="iv-film-placeholder" data-failed={failed ? "true" : "false"}>{entry.ext.replace(".", "").toUpperCase()}</span>;
});

export function Filmstrip({
  entries,
  index,
  onSelect
}: {
  entries: readonly ImageEntry[];
  index: number;
  onSelect(index: number): void;
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const virtualizer = useVirtualizer({
    count: entries.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ITEM_WIDTH + ITEM_GAP,
    horizontal: true,
    overscan: 8
  });

  useEffect(() => {
    if (index >= 0) virtualizer.scrollToIndex(index, { align: "center" });
  }, [index, virtualizer]);

  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      if (Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
      event.preventDefault();
      element.scrollLeft += event.deltaY;
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    // The Host overlay scrollbar appears on scroll activity; pointer movement
    // over the strip counts as activity, so it comes back every time.
    let last = 0;
    const onPointerMove = () => {
      const now = performance.now();
      if (now - last < 100 || element.scrollWidth <= element.clientWidth + 1) return;
      last = now;
      element.dispatchEvent(new Event("scroll"));
    };
    element.addEventListener("pointermove", onPointerMove, { passive: true });
    return () => {
      element.removeEventListener("wheel", onWheel);
      element.removeEventListener("pointermove", onPointerMove);
    };
  }, []);

  return (
    <div aria-label="缩略图" className="iv-filmstrip" ref={scrollRef} role="listbox">
      <div className="iv-filmstrip-track" style={{ width: `${virtualizer.getTotalSize()}px` }}>
        {virtualizer.getVirtualItems().map((item) => {
          const entry = entries[item.index];
          const selected = item.index === index;
          return (
            <button
              aria-label={entry.name}
              aria-selected={selected}
              className="iv-film-item"
              data-pointer-glow="off"
              data-selected={selected ? "true" : "false"}
              key={entry.path}
              onClick={() => onSelect(item.index)}
              role="option"
              style={{ transform: `translateX(${item.start}px)`, width: `${ITEM_WIDTH}px` }}
              title={entry.name}
              type="button"
            >
              <FilmstripThumb entry={entry} />
            </button>
          );
        })}
      </div>
    </div>
  );
}
