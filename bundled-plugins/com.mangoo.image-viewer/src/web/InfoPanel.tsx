import { useEffect, useRef, useState, type ReactNode } from "react";
import { createCoreClient, icons } from "@mangoo/web/plugin-host";
import type { ImageViewerMetadata } from "@mangoo/core-client";

import { formatBytes, type ImageEntry } from "../shared/navigation.ts";
import { computeHistogram, type Histogram, type LoadedImage } from "./imageSource.ts";
import { formatFrameTime } from "../shared/frames.ts";
import type { FramePlaybackState } from "./FrameInspector.tsx";
import { describeError } from "./useViewerSession.ts";

function orientationLabel(value: number): string {
  return ({ 2: "水平翻转", 3: "旋转 180°", 4: "垂直翻转", 5: "转置", 6: "顺时针 90°", 7: "反转置", 8: "逆时针 90°" } as Record<number, string>)[value] ?? String(value);
}

function formatDate(ms: number | null | undefined): string | null {
  if (!ms) return null;
  return new Date(ms).toLocaleString();
}

function exifDate(value: string | null): string | null {
  if (!value) return null;
  const match = /^(\d{4}):(\d{2}):(\d{2})[ T](\d{2}):(\d{2}):(\d{2})/.exec(value);
  return match ? `${match[1]}-${match[2]}-${match[3]} ${match[4]}:${match[5]}:${match[6]}` : value;
}

function Row({ label, value }: { label: string; value: ReactNode }) {
  if (value === null || value === undefined || value === "") return null;
  return (
    <div className="iv-info-row">
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

/** A titled section that folds open/closed (state remembered per section). */
function Fold({ id, title, icon, children }: { id: string; title: string; icon: ReactNode; children: ReactNode }) {
  const storageKey = `mangoo.imageViewer.info.fold.${id}`;
  const [open, setOpen] = useState(() => {
    try {
      return window.localStorage.getItem(storageKey) !== "closed";
    } catch {
      return true;
    }
  });
  return (
    <section className="iv-info-section" data-open={open ? "true" : "false"}>
      <button
        aria-expanded={open}
        className="iv-info-fold"
        data-pointer-glow="off"
        onClick={() => {
          setOpen((value) => {
            try {
              window.localStorage.setItem(storageKey, value ? "closed" : "open");
            } catch {
              // ignore
            }
            return !value;
          });
        }}
        type="button"
      >
        <icons.ChevronRight aria-hidden="true" className="iv-info-fold-chevron" size={12} />
        {icon}
        <span>{title}</span>
      </button>
      {open ? <div className="iv-info-fold-body">{children}</div> : null}
    </section>
  );
}

type HistogramMode = "rgb" | "luma";

function percent(value: number): string {
  if (value <= 0) return "0%";
  if (value < 0.001) return "<0.1%";
  return `${(value * 100).toFixed(value < 0.1 ? 1 : 0)}%`;
}

/** Tonal reading of the luma distribution, in the photographer's words. */
function exposureLabel(histogram: Histogram): string {
  if (histogram.samples === 0) return "—";
  if (histogram.mean < 70) return "偏暗";
  if (histogram.mean > 185) return "偏亮";
  if (histogram.deviation < 35) return "低反差";
  if (histogram.deviation > 75) return "高反差";
  return "正常";
}

function HistogramView({ image }: { image: LoadedImage | null }) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [histogram, setHistogram] = useState<Histogram | null>(null);
  const [mode, setMode] = useState<HistogramMode>(() => {
    try {
      return window.localStorage.getItem("mangoo.imageViewer.histogram.mode") === "luma" ? "luma" : "rgb";
    } catch {
      return "rgb";
    }
  });
  /** Hovered tone (0..255) and its share of pixels. */
  const [probe, setProbe] = useState<{ value: number; x: number } | null>(null);
  useEffect(() => {
    let cancelled = false;
    setHistogram(null);
    if (!image) return;
    const timer = window.setTimeout(() => {
      computeHistogram(image).then((value) => {
        if (!cancelled) setHistogram(value);
      }, () => undefined);
    }, 120);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [image]);
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !histogram) return;
    const context = canvas.getContext("2d");
    if (!context) return;
    const { width, height } = canvas;
    context.clearRect(0, 0, width, height);
    // Quarter guides (shadows / midtones / highlights), drawn under the data.
    context.fillStyle = "rgba(255, 255, 255, 0.06)";
    for (const quarter of [0.25, 0.5, 0.75]) context.fillRect(Math.round(width * quarter), 0, 1, height);
    if (mode === "luma") {
      context.fillStyle = "rgba(226, 232, 238, 0.62)";
      context.beginPath();
      context.moveTo(0, height);
      for (let value = 0; value < 256; value += 1) {
        context.lineTo((value / 255) * width, height - Math.min(1, histogram.luma[value] / histogram.max) * height);
      }
      context.lineTo(width, height);
      context.closePath();
      context.fill();
      return;
    }
    context.globalCompositeOperation = "lighter";
    const draw = (channel: Uint32Array, color: string) => {
      context.fillStyle = color;
      context.beginPath();
      context.moveTo(0, height);
      for (let value = 0; value < 256; value += 1) {
        const x = (value / 255) * width;
        const y = height - Math.min(1, channel[value] / histogram.max) * height;
        context.lineTo(x, y);
      }
      context.lineTo(width, height);
      context.closePath();
      context.fill();
    };
    draw(histogram.red, "rgba(239, 120, 110, 0.55)");
    draw(histogram.green, "rgba(120, 214, 150, 0.55)");
    draw(histogram.blue, "rgba(110, 160, 239, 0.55)");
    context.globalCompositeOperation = "source-over";
  }, [histogram, mode]);

  const chooseMode = (next: HistogramMode) => {
    setMode(next);
    try {
      window.localStorage.setItem("mangoo.imageViewer.histogram.mode", next);
    } catch {
      // ignore
    }
  };
  const probeShare = probe && histogram && histogram.samples
    ? (mode === "luma" ? histogram.luma[probe.value] : Math.max(histogram.red[probe.value], histogram.green[probe.value], histogram.blue[probe.value])) / histogram.samples
    : 0;

  return (
    <div className="iv-histo-block">
      <div className="iv-histo-head">
        <span className="iv-histo-title">直方图</span>
        <span aria-label="直方图模式" className="iv-histo-modes" role="radiogroup">
          {(["rgb", "luma"] as const).map((value) => (
            <button
              aria-checked={mode === value}
              className="iv-histo-mode"
              data-pointer-glow="off"
              data-selected={mode === value ? "true" : "false"}
              key={value}
              onClick={() => chooseMode(value)}
              role="radio"
              type="button"
            >
              {value === "rgb" ? "RGB" : "亮度"}
            </button>
          ))}
        </span>
      </div>
      <div
        className="iv-histogram"
        data-ready={histogram ? "true" : "false"}
        onPointerLeave={() => setProbe(null)}
        onPointerMove={(event) => {
          const box = event.currentTarget.getBoundingClientRect();
          const x = Math.min(box.width, Math.max(0, event.clientX - box.left));
          setProbe({ value: Math.round((x / Math.max(1, box.width)) * 255), x });
        }}
      >
        <canvas height={112} ref={canvasRef} width={280} />
        {histogram && histogram.clippedShadows >= 0.005 ? <span className="iv-histo-clip" data-side="shadows" title={`暗部溢出 ${percent(histogram.clippedShadows)}`} /> : null}
        {histogram && histogram.clippedHighlights >= 0.005 ? <span className="iv-histo-clip" data-side="highlights" title={`高光溢出 ${percent(histogram.clippedHighlights)}`} /> : null}
        {probe && histogram ? (
          <>
            <span aria-hidden="true" className="iv-histo-cursor" style={{ left: probe.x }} />
            <span className="iv-histo-readout" data-align={probe.x > 140 ? "left" : "right"} style={{ left: probe.x }}>
              {probe.value} · {percent(probeShare)}
            </span>
          </>
        ) : null}
      </div>
      <div aria-hidden="true" className="iv-histo-scale">
        <span>暗部</span>
        <span>中间调</span>
        <span>高光</span>
      </div>
      {histogram ? (
        <dl className="iv-histo-stats">
          <div><dt>平均亮度</dt><dd>{Math.round(histogram.mean)}</dd></div>
          <div><dt>中位数</dt><dd>{histogram.median}</dd></div>
          <div><dt>反差</dt><dd>{Math.round(histogram.deviation)}</dd></div>
          <div><dt>曝光</dt><dd>{exposureLabel(histogram)}</dd></div>
          <div data-tone={histogram.clippedShadows >= 0.005 ? "warn" : undefined}><dt>暗部溢出</dt><dd>{percent(histogram.clippedShadows)}</dd></div>
          <div data-tone={histogram.clippedHighlights >= 0.005 ? "warn" : undefined}><dt>高光溢出</dt><dd>{percent(histogram.clippedHighlights)}</dd></div>
          {histogram.transparent > 0 ? <div><dt>透明像素</dt><dd>{percent(histogram.transparent)}</dd></div> : null}
        </dl>
      ) : (
        <div className="iv-histo-stats-placeholder" />
      )}
    </div>
  );
}

export function InfoPanel({ entry, image, animation }: { entry: ImageEntry | null; image: LoadedImage | null; animation?: FramePlaybackState | null }) {
  const [metadata, setMetadata] = useState<ImageViewerMetadata | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setMetadata(null);
    setError(null);
    if (!entry) return;
    const controller = new AbortController();
    createCoreClient()
      .getImageViewerMetadata(entry.path, { signal: controller.signal })
      .then(setMetadata, (cause) => {
        if (!controller.signal.aborted) setError(describeError(cause));
      });
    return () => controller.abort();
  }, [entry?.path, entry?.modifiedAtMs]);

  if (!entry) {
    return (
      <aside aria-label="图片信息" className="iv-info">
        <div className="iv-info-empty">没有打开的图片</div>
      </aside>
    );
  }
  const exif = metadata?.exif ?? null;
  const width = metadata?.width ?? image?.width ?? null;
  const height = metadata?.height ?? image?.height ?? null;
  const format = image?.kind === "animated"
    ? ({ ".png": "APNG（动态 PNG）", ".apng": "APNG", ".gif": "GIF 动图", ".webp": "WebP 动图", ".avif": "AVIF 动图" } as Record<string, string>)[entry.ext] ?? entry.ext.replace(".", "").toUpperCase()
    : entry.ext.replace(".", "").toUpperCase();
  const camera = [exif?.make, exif?.model && exif?.make && exif.model.startsWith(exif.make) ? exif.model.slice(exif.make.length).trim() : exif?.model]
    .filter(Boolean)
    .join(" ");
  const exposure = [
    exif?.exposureTime ? `${exif.exposureTime}s` : null,
    exif?.fNumber ? `f/${exif.fNumber.toFixed(1).replace(/\.0$/, "")}` : null,
    exif?.iso ? `ISO ${exif.iso}` : null,
    exif?.focalLengthMm ? `${Math.round(exif.focalLengthMm)}mm` : null
  ].filter(Boolean).join("  ·  ");
  const gps = exif?.gpsLatitude !== null && exif?.gpsLatitude !== undefined && exif?.gpsLongitude !== null && exif?.gpsLongitude !== undefined
    ? `${exif.gpsLatitude.toFixed(5)}, ${exif.gpsLongitude.toFixed(5)}`
    : null;

  return (
    <aside aria-label="图片信息" className="iv-info">
      <div className="iv-info-title" title={entry.name}>{entry.name}</div>
      {/* The histogram stays put; only the sections below it scroll. */}
      {image ? <div className="iv-info-pinned"><HistogramView image={image} /></div> : null}
      <div className="iv-info-scroll">
      <Fold icon={<icons.File aria-hidden="true" size={13} />} id="file" title="文件">
        <dl>
          <Row label="名称" value={<span className="iv-info-name">{entry.name}</span>} />
          <Row label="尺寸" value={width && height ? `${width} × ${height}` : null} />
          <Row label="大小" value={formatBytes(metadata?.sizeBytes ?? entry.sizeBytes)} />
          <Row label="格式" value={format} />
          <Row label="帧数" value={animation ? `${animation.frameCount} 帧` : null} />
          <Row label="时长" value={animation ? formatFrameTime(animation.totalMs) : null} />
          <Row label="修改时间" value={formatDate(metadata?.modifiedAtMs ?? entry.modifiedAtMs)} />
          <Row label="创建时间" value={formatDate(metadata?.createdAtMs)} />
          <Row label="位置" value={<span className="iv-info-path">{metadata?.directory ?? null}</span>} />
        </dl>
      </Fold>
      <Fold icon={<icons.Camera aria-hidden="true" size={13} />} id="exif" title="EXIF">
        {!metadata ? (
          <p className="iv-exif-empty">正在读取…</p>
        ) : !exif ? (
          <p className="iv-exif-empty">这张图片没有 EXIF 信息。</p>
        ) : null}
      {exif ? (
        <>
          {exposure ? <div className="iv-info-exposure">{exposure}</div> : null}
          <dl>
            <Row label="相机" value={camera || null} />
            <Row label="镜头" value={exif.lensModel} />
            <Row label="拍摄时间" value={exifDate(exif.dateTimeOriginal)} />
            <Row label="快门" value={exif.exposureTime ? `${exif.exposureTime} s` : null} />
            <Row label="光圈" value={exif.fNumber ? `f/${exif.fNumber.toFixed(1).replace(/\.0$/, "")}` : null} />
            <Row label="ISO" value={exif.iso ? String(exif.iso) : null} />
            <Row label="焦距" value={exif.focalLengthMm ? `${Math.round(exif.focalLengthMm)} mm` : null} />
            <Row label="等效焦距" value={exif.focalLength35mm ? `${exif.focalLength35mm}mm` : null} />
            <Row label="曝光补偿" value={exif.exposureBiasEv !== null ? `${exif.exposureBiasEv > 0 ? "+" : ""}${exif.exposureBiasEv.toFixed(1)} EV` : null} />
            <Row label="曝光程序" value={exif.exposureProgram} />
            <Row label="测光" value={exif.meteringMode} />
            <Row label="闪光灯" value={exif.flash} />
            <Row label="白平衡" value={exif.whiteBalance} />
            <Row label="色彩空间" value={exif.colorSpace} />
            <Row label="方向" value={exif.orientation && exif.orientation !== 1 ? orientationLabel(exif.orientation) : null} />
            <Row label="原始尺寸" value={exif.pixelWidth && exif.pixelHeight ? `${exif.pixelWidth} × ${exif.pixelHeight}` : null} />
            <Row label="GPS" value={gps} />
            <Row label="海拔" value={exif.gpsAltitudeM !== null ? `${Math.round(exif.gpsAltitudeM)} m` : null} />
            <Row label="软件" value={exif.software} />
            <Row label="作者" value={exif.artist} />
            <Row label="版权" value={exif.copyright} />
          </dl>
        </>
      ) : null}
      </Fold>
      {error ? <div className="iv-info-error" role="status">{error}</div> : null}
      </div>
    </aside>
  );
}
