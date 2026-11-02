import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";

type BenchResult = {
  mode: string;
  itemCount: number;
  assetCount: number;
  iterations: number;
  preloadRadius: number;
  preloadAllMs?: number;
  durationMs: number;
  switchesPerSecond: number;
  latencyP50Ms: number;
  latencyP95Ms: number;
  latencyP99Ms: number;
  latencyMaxMs: number;
  decodeP50Ms: number;
  decodeP95Ms: number;
  decodeP99Ms: number;
  decodeMaxMs: number;
  committedMismatches: number;
  finalSelectedId: number;
};

declare global {
  interface Window {
    __mangooPreviewReady?: boolean;
    __lastCommittedId?: number;
    runPreviewSwitchBenchmark?: (options?: {
      mode?: "cached" | "prefetch";
      iterations?: number;
      step?: number;
      preloadRadius?: number;
    }) => Promise<BenchResult>;
    runPreviewBurstBenchmark?: (options?: {
      bursts?: number;
      changesPerBurst?: number;
      step?: number;
    }) => Promise<{
      itemCount: number;
      assetCount: number;
      bursts: number;
      changesPerBurst: number;
      totalChanges: number;
      durationMs: number;
      changesPerSecond: number;
      burstLatencyP50Ms: number;
      burstLatencyP95Ms: number;
      burstLatencyP99Ms: number;
      burstLatencyMaxMs: number;
      committedMismatches: number;
      finalSelectedId: number;
    }>;
  }
}

const params = new URLSearchParams(window.location.search);
const ITEM_COUNT = Number(params.get("count") ?? "1000000");
const DEFAULT_PRELOAD_RADIUS = Number(params.get("preloadRadius") ?? "12");

function percentile(values: number[], p: number): number {
  if (!values.length) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const idx = Math.min(sorted.length - 1, Math.round((sorted.length - 1) * p));
  return sorted[idx];
}

function round(value: number): number {
  return Math.round(value * 1000) / 1000;
}

async function loadManifest(): Promise<string[]> {
  const response = await fetch("/thumbs/manifest.json");
  if (!response.ok) throw new Error("missing thumbnail manifest");
  const manifest = await response.json();
  return manifest.files as string[];
}

function loadImage(src: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve();
    image.onerror = () => reject(new Error(`image failed: ${src}`));
    image.src = src;
  });
}

function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

function App() {
  const [assets, setAssets] = useState<string[]>([]);
  const [selectedId, setSelectedId] = useState(0);
  const [preloadRadius, setPreloadRadius] = useState(DEFAULT_PRELOAD_RADIUS);
  const imgRef = useRef<HTMLImageElement | null>(null);
  const cache = useRef<Map<string, Promise<void>>>(new Map());

  useEffect(() => {
    loadManifest().then(setAssets).catch((error) => {
      console.error(error);
    });
  }, []);

  const srcFor = (id: number): string => {
    if (!assets.length) return "";
    return `/thumbs/${assets[((id % assets.length) + assets.length) % assets.length]}`;
  };

  const preloadSrc = (src: string) => {
    if (!src || cache.current.has(src)) return;
    cache.current.set(src, loadImage(src).catch(() => undefined));
  };

  useEffect(() => {
    if (!assets.length) return;
    for (let offset = -preloadRadius; offset <= preloadRadius; offset += 1) {
      preloadSrc(srcFor(selectedId + offset));
    }
  }, [assets, selectedId, preloadRadius]);

  useLayoutEffect(() => {
    window.__lastCommittedId = selectedId;
  }, [selectedId]);

  useEffect(() => {
    if (!assets.length) return;
    window.__mangooPreviewReady = true;
    window.runPreviewSwitchBenchmark = async (options = {}) => {
      const mode = options.mode ?? "cached";
      const iterations = options.iterations ?? 1000;
      const step = options.step ?? 17;
      const radius = options.preloadRadius ?? preloadRadius;
      setPreloadRadius(radius);

      let preloadAllMs: number | undefined;
      if (mode === "cached") {
        const preloadStart = performance.now();
        await Promise.all(assets.map((asset) => {
          const src = `/thumbs/${asset}`;
          preloadSrc(src);
          return cache.current.get(src);
        }));
        preloadAllMs = performance.now() - preloadStart;
      }

      const latencies: number[] = [];
      const decodeLatencies: number[] = [];
      let mismatches = 0;
      const start = performance.now();

      for (let i = 0; i < iterations; i += 1) {
        const targetId = (i * step) % ITEM_COUNT;
        const switchStart = performance.now();
        setSelectedId(targetId);
        await nextFrame();
        if (window.__lastCommittedId !== targetId) {
          await nextFrame();
        }
        const committedAt = performance.now();
        if (window.__lastCommittedId !== targetId) {
          mismatches += 1;
        }
        latencies.push(committedAt - switchStart);

        const image = imgRef.current;
        const decodeStart = performance.now();
        if (image?.decode) {
          try {
            await image.decode();
          } catch {
            // decode() may reject if the source changed during the loop.
          }
        }
        decodeLatencies.push(performance.now() - decodeStart);
      }

      const duration = performance.now() - start;
      return {
        mode,
        itemCount: ITEM_COUNT,
        assetCount: assets.length,
        iterations,
        preloadRadius: radius,
        preloadAllMs: preloadAllMs === undefined ? undefined : round(preloadAllMs),
        durationMs: round(duration),
        switchesPerSecond: round((iterations / duration) * 1000),
        latencyP50Ms: round(percentile(latencies, 0.5)),
        latencyP95Ms: round(percentile(latencies, 0.95)),
        latencyP99Ms: round(percentile(latencies, 0.99)),
        latencyMaxMs: round(Math.max(...latencies)),
        decodeP50Ms: round(percentile(decodeLatencies, 0.5)),
        decodeP95Ms: round(percentile(decodeLatencies, 0.95)),
        decodeP99Ms: round(percentile(decodeLatencies, 0.99)),
        decodeMaxMs: round(Math.max(...decodeLatencies)),
        committedMismatches: mismatches,
        finalSelectedId: window.__lastCommittedId ?? -1
      };
    };

    window.runPreviewBurstBenchmark = async (options = {}) => {
      const bursts = options.bursts ?? 200;
      const changesPerBurst = options.changesPerBurst ?? 20;
      const step = options.step ?? 19;
      const latencies: number[] = [];
      let mismatches = 0;
      const start = performance.now();

      for (let burst = 0; burst < bursts; burst += 1) {
        let targetId = 0;
        const burstStart = performance.now();
        for (let change = 0; change < changesPerBurst; change += 1) {
          targetId = ((burst * changesPerBurst + change) * step) % ITEM_COUNT;
          setSelectedId(targetId);
        }
        await nextFrame();
        if (window.__lastCommittedId !== targetId) {
          await nextFrame();
        }
        const committedAt = performance.now();
        if (window.__lastCommittedId !== targetId) {
          mismatches += 1;
        }
        latencies.push(committedAt - burstStart);
      }

      const duration = performance.now() - start;
      const totalChanges = bursts * changesPerBurst;
      return {
        itemCount: ITEM_COUNT,
        assetCount: assets.length,
        bursts,
        changesPerBurst,
        totalChanges,
        durationMs: round(duration),
        changesPerSecond: round((totalChanges / duration) * 1000),
        burstLatencyP50Ms: round(percentile(latencies, 0.5)),
        burstLatencyP95Ms: round(percentile(latencies, 0.95)),
        burstLatencyP99Ms: round(percentile(latencies, 0.99)),
        burstLatencyMaxMs: round(Math.max(...latencies)),
        committedMismatches: mismatches,
        finalSelectedId: window.__lastCommittedId ?? -1
      };
    };
  }, [assets, preloadRadius]);

  const selectedSrc = srcFor(selectedId);
  const filmstrip = useMemo(() => {
    return Array.from({ length: 15 }, (_, idx) => selectedId - 7 + idx);
  }, [selectedId]);

  return (
    <main className="app">
      <aside className="panel">
        <div className="title">Preview Bench</div>
        <div className="row"><span>Items</span><strong>{ITEM_COUNT.toLocaleString()}</strong></div>
        <div className="row"><span>Assets</span><strong>{assets.length}</strong></div>
        <div className="row"><span>Selected</span><strong>{selectedId.toLocaleString()}</strong></div>
        <div className="row"><span>Preload</span><strong>{preloadRadius}</strong></div>
      </aside>
      <section className="preview">
        <div className="topbar">
          <button>Previous</button>
          <button>Next</button>
          <button>Fit</button>
          <button>100%</button>
        </div>
        <div className="stage">
          {selectedSrc ? (
            <img
              ref={imgRef}
              className="main-image"
              src={selectedSrc}
              data-selected-id={selectedId}
              alt=""
            />
          ) : (
            <div className="loading">Loading</div>
          )}
        </div>
        <div className="filmstrip">
          {filmstrip.map((id) => (
            <img
              key={id}
              className={id === selectedId ? "strip-item active" : "strip-item"}
              src={srcFor(id)}
              alt=""
            />
          ))}
        </div>
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
