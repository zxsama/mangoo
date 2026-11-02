import React, { useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { useVirtualizer } from "@tanstack/react-virtual";
import "./styles.css";

type BenchResult = {
  itemCount: number;
  columns: number;
  rows: number;
  durationMs: number;
  frames: number;
  fps: number;
  frameP50Ms: number;
  frameP95Ms: number;
  frameP99Ms: number;
  frameMaxMs: number;
  renderedTiles: number;
  renderedRows: number;
  scrollHeight: number;
  virtualTotalSize: number;
};

declare global {
  interface Window {
    __mangooReady?: boolean;
    runVirtualGridBenchmark?: () => Promise<BenchResult>;
  }
}

const params = new URLSearchParams(window.location.search);
const ITEM_COUNT = Number(params.get("count") ?? "1000000");
const TILE_WIDTH = Number(params.get("tileWidth") ?? "132");
const TILE_HEIGHT = Number(params.get("tileHeight") ?? "168");
const GAP = Number(params.get("gap") ?? "8");
const OVERSCAN = Number(params.get("overscan") ?? "8");

function percentile(values: number[], p: number): number {
  if (!values.length) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const idx = Math.min(sorted.length - 1, Math.round((sorted.length - 1) * p));
  return sorted[idx];
}

function formatId(id: number): string {
  return id.toString().padStart(7, "0");
}

function App() {
  const parentRef = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(1280);

  useEffect(() => {
    const node = parentRef.current;
    if (!node) return;
    const observer = new ResizeObserver((entries) => {
      const nextWidth = entries[0]?.contentRect.width;
      if (nextWidth) setWidth(nextWidth);
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  const columns = Math.max(1, Math.floor((width + GAP) / (TILE_WIDTH + GAP)));
  const rows = Math.ceil(ITEM_COUNT / columns);

  const rowVirtualizer = useVirtualizer({
    count: rows,
    getScrollElement: () => parentRef.current,
    estimateSize: () => TILE_HEIGHT + GAP,
    overscan: OVERSCAN
  });

  const virtualRows = rowVirtualizer.getVirtualItems();

  useEffect(() => {
    window.__mangooReady = true;
    window.runVirtualGridBenchmark = async () => {
      const scrollEl = parentRef.current;
      if (!scrollEl) throw new Error("missing scroll element");
      const durationMs = 5000;
      const intervals: number[] = [];
      const start = performance.now();
      let last = start;
      let frames = 0;
      const maxScrollTop = Math.max(0, scrollEl.scrollHeight - scrollEl.clientHeight);

      await new Promise<void>((resolve) => {
        const tick = (time: number) => {
          intervals.push(time - last);
          last = time;
          frames += 1;
          const progress = Math.min(1, (time - start) / durationMs);
          scrollEl.scrollTop = maxScrollTop * progress;
          if (progress < 1) {
            requestAnimationFrame(tick);
          } else {
            resolve();
          }
        };
        requestAnimationFrame(tick);
      });

      const elapsed = performance.now() - start;
      return {
        itemCount: ITEM_COUNT,
        columns,
        rows,
        durationMs: Math.round(elapsed),
        frames,
        fps: Math.round((frames / elapsed) * 1000 * 10) / 10,
        frameP50Ms: Math.round(percentile(intervals, 0.5) * 1000) / 1000,
        frameP95Ms: Math.round(percentile(intervals, 0.95) * 1000) / 1000,
        frameP99Ms: Math.round(percentile(intervals, 0.99) * 1000) / 1000,
        frameMaxMs: Math.round(Math.max(...intervals) * 1000) / 1000,
        renderedTiles: document.querySelectorAll(".tile").length,
        renderedRows: document.querySelectorAll(".grid-row").length,
        scrollHeight: scrollEl.scrollHeight,
        virtualTotalSize: rowVirtualizer.getTotalSize()
      };
    };
  }, [columns, rows]);

  const columnIndexes = useMemo(
    () => Array.from({ length: columns }, (_, idx) => idx),
    [columns]
  );

  return (
    <main className="shell">
      <aside className="sidebar">
        <div className="brand">Mangoo Bench</div>
        <div className="metric">
          <span>Items</span>
          <strong>{ITEM_COUNT.toLocaleString()}</strong>
        </div>
        <div className="metric">
          <span>Columns</span>
          <strong>{columns}</strong>
        </div>
        <div className="metric">
          <span>Rows</span>
          <strong>{rows.toLocaleString()}</strong>
        </div>
      </aside>
      <section className="content">
        <div className="toolbar">
          <button>Grid</button>
          <button>Sort</button>
          <button>Filter</button>
          <button>Preview</button>
        </div>
        <div ref={parentRef} className="viewport">
          <div
            className="grid-space"
            style={{ height: rowVirtualizer.getTotalSize() }}
          >
            {virtualRows.map((virtualRow) => {
              const rowStart = virtualRow.index * columns;
              return (
                <div
                  className="grid-row"
                  key={virtualRow.key}
                  style={{ transform: `translateY(${virtualRow.start}px)` }}
                >
                  {columnIndexes.map((column) => {
                    const itemId = rowStart + column;
                    if (itemId >= ITEM_COUNT) return null;
                    const hue = (itemId * 47) % 360;
                    return (
                      <div className="tile" key={itemId}>
                        <div
                          className="thumb"
                          style={{
                            background: `linear-gradient(135deg, hsl(${hue} 58% 55%), hsl(${(hue + 42) % 360} 46% 34%))`
                          }}
                        />
                        <div className="caption">IMG_{formatId(itemId)}</div>
                      </div>
                    );
                  })}
                </div>
              );
            })}
          </div>
        </div>
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
