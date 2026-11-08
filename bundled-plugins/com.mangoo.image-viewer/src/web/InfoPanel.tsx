import { useEffect, useRef, useState, type ReactNode } from "react";
import { createCoreClient, icons } from "@mangoo/web/plugin-host";
import type { ImageViewerMetadata } from "@mangoo/core-client";

import { formatBytes, type ImageEntry } from "../shared/navigation.ts";
import { computeHistogram, type Histogram, type LoadedImage } from "./imageSource.ts";
import { formatFrameTime } from "../shared/frames.ts";
import type { FramePlaybackState } from "./FrameInspector.tsx";
import { describeError } from "./useViewerSession.ts";

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

function HistogramView({ image }: { image: LoadedImage | null }) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [histogram, setHistogram] = useState<Histogram | null>(null);
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
  }, [histogram]);
  return (
    <div className="iv-histogram" data-ready={histogram ? "true" : "false"}>
      <canvas height={72} ref={canvasRef} width={256} />
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
  const pixels = width && height ? width * height : 0;
  const megapixels = !pixels ? null : pixels >= 1_000_000 ? `${(pixels / 1_000_000).toFixed(1)} MP` : `${(pixels / 10_000).toFixed(1)} 万像素`;
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
      {image ? <HistogramView image={image} /> : null}
      <section className="iv-info-section">
        <div className="iv-info-section-title">
          <icons.File aria-hidden="true" size={13} />
          <span>文件</span>
        </div>
        <dl>
          <Row label="名称" value={<span className="iv-info-name">{entry.name}</span>} />
          <Row label="尺寸" value={width && height ? `${width} × ${height}` : null} />
          <Row label="像素" value={megapixels} />
          <Row label="大小" value={formatBytes(metadata?.sizeBytes ?? entry.sizeBytes)} />
          <Row label="格式" value={format} />
          <Row label="帧数" value={animation ? `${animation.frameCount} 帧` : null} />
          <Row label="时长" value={animation ? formatFrameTime(animation.totalMs) : null} />
          <Row label="修改时间" value={formatDate(metadata?.modifiedAtMs ?? entry.modifiedAtMs)} />
          <Row label="创建时间" value={formatDate(metadata?.createdAtMs)} />
          <Row label="显示" value={image ? (image.tier === "original" ? "原图" : image.tier === "decoder" ? "插件解码" : "自适应预览") : null} />
          <Row label="位置" value={<span className="iv-info-path">{metadata?.directory ?? null}</span>} />
        </dl>
      </section>
      {exif ? (
        <section className="iv-info-section">
          <div className="iv-info-section-title">
            <icons.Camera aria-hidden="true" size={13} />
            <span>拍摄</span>
          </div>
          {exposure ? <div className="iv-info-exposure">{exposure}</div> : null}
          <dl>
            <Row label="相机" value={camera || null} />
            <Row label="镜头" value={exif.lensModel} />
            <Row label="拍摄时间" value={exifDate(exif.dateTimeOriginal)} />
            <Row label="等效焦距" value={exif.focalLength35mm ? `${exif.focalLength35mm}mm` : null} />
            <Row label="曝光补偿" value={exif.exposureBiasEv !== null ? `${exif.exposureBiasEv > 0 ? "+" : ""}${exif.exposureBiasEv.toFixed(1)} EV` : null} />
            <Row label="曝光程序" value={exif.exposureProgram} />
            <Row label="测光" value={exif.meteringMode} />
            <Row label="闪光灯" value={exif.flash} />
            <Row label="白平衡" value={exif.whiteBalance} />
            <Row label="色彩空间" value={exif.colorSpace} />
            <Row label="GPS" value={gps} />
            <Row label="海拔" value={exif.gpsAltitudeM !== null ? `${Math.round(exif.gpsAltitudeM)} m` : null} />
            <Row label="软件" value={exif.software} />
            <Row label="作者" value={exif.artist} />
            <Row label="版权" value={exif.copyright} />
          </dl>
        </section>
      ) : null}
      {error ? <div className="iv-info-error" role="status">{error}</div> : null}
    </aside>
  );
}
