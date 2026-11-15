import {
  createCoreClient,
  createDecodeDispatcher,
  createPreviewDecodeWorker,
  decodeBlobOnMainThread,
  type DecodeTransport
} from "@mangoo/web/plugin-host";

import { isAnimatedImageHeader } from "../shared/formats.ts";
import type { ImageEntry } from "../shared/navigation.ts";

/**
 * Loads viewer pictures through Core (`/api/image-viewer/image`) and decodes
 * them off the main thread with the Library preview's decode Worker. Decoded
 * `ImageBitmap`s live in a byte-budgeted LRU (explicitly `.close()`d on
 * eviction) so prev/next and the filmstrip are instant once warmed. Animated
 * formats stay on an object URL so an <img> can play them.
 */

export type LoadedImage =
  | {
      kind: "bitmap";
      key: string;
      bitmap: ImageBitmap;
      /** Display size of the source (orientation applied). */
      width: number;
      height: number;
      tier: string;
      upgradable: boolean;
    }
  | {
      kind: "animated";
      key: string;
      url: string;
      blob: Blob;
      width: number;
      height: number;
      tier: string;
      upgradable: false;
    };

type DecodeDispatcher = ReturnType<typeof createDecodeDispatcher>;

/** Camera RAW: developed sensor data, or the camera's embedded JPEG. */
export type RawSource = "developed" | "embedded";

export interface LoadOptions {
  viewportLongPx: number;
  full: boolean;
  animatedExtensions: readonly string[];
  signal?: AbortSignal;
  priority?: "navigation" | "interactive" | "resource";
  rawSource?: RawSource;
}

let dispatcher: DecodeDispatcher | null | undefined;

function decoder(): DecodeDispatcher | null {
  if (dispatcher !== undefined) return dispatcher;
  const worker = createPreviewDecodeWorker();
  if (!worker) {
    dispatcher = null;
    return null;
  }
  let onMessage: Parameters<DecodeTransport["subscribe"]>[0] | null = null;
  worker.onmessage = (event: MessageEvent) => onMessage?.(event.data);
  const created = createDecodeDispatcher({
    post: (message, transfer) => worker.postMessage(message, transfer),
    subscribe: (handler) => {
      onMessage = handler;
    }
  });
  worker.onerror = () => {
    created.rejectAll(new Error("image decode worker failed"));
    worker.terminate();
    dispatcher = null;
  };
  dispatcher = created;
  return created;
}

async function decodeBitmap(blob: Blob, signal?: AbortSignal): Promise<ImageBitmap> {
  const worker = decoder();
  if (worker) return (await worker.decode(blob, signal)) as unknown as ImageBitmap;
  return (await decodeBlobOnMainThread(blob, signal, async (input) => (await createImageBitmap(input)) as never)) as unknown as ImageBitmap;
}

function imageElementSize(url: string): Promise<{ width: number; height: number }> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve({ width: image.naturalWidth, height: image.naturalHeight });
    image.onerror = () => reject(new Error("图片无法显示"));
    image.src = url;
  });
}

interface CacheRecord {
  image: LoadedImage;
  bytes: number;
  used: number;
}

export class ViewerImageCache {
  private readonly records = new Map<string, CacheRecord>();
  private readonly inflight = new Map<string, Promise<LoadedImage>>();
  private readonly pinned = new Set<string>();
  private clock = 0;
  private used = 0;

  private readonly budgetBytes: number;

  constructor(budgetBytes = 768 * 1024 * 1024) {
    this.budgetBytes = budgetBytes;
  }

  static key(entry: ImageEntry, full: boolean, rawSource: RawSource = "developed"): string {
    return `${entry.path}\u0000${entry.modifiedAtMs}\u0000${entry.sizeBytes}\u0000${full ? "full" : "fit"}${rawSource === "embedded" ? "\u0000embedded" : ""}`;
  }

  peek(key: string): LoadedImage | null {
    const record = this.records.get(key);
    if (!record) return null;
    record.used = ++this.clock;
    return record.image;
  }

  /** Keep these keys resident (the picture on screen and its upgrade). */
  pin(keys: readonly string[]): void {
    this.pinned.clear();
    for (const key of keys) this.pinned.add(key);
  }

  load(
    entry: ImageEntry,
    options: LoadOptions
  ): Promise<LoadedImage> {
    const key = ViewerImageCache.key(entry, options.full, options.rawSource);
    const hit = this.peek(key);
    if (hit) return Promise.resolve(hit);
    const pending = this.inflight.get(key);
    if (pending) return pending;
    const promise = this.fetch(entry, key, options).finally(() => this.inflight.delete(key));
    this.inflight.set(key, promise);
    return promise;
  }

  private async fetch(
    entry: ImageEntry,
    key: string,
    options: LoadOptions
  ): Promise<LoadedImage> {
    const client = createCoreClient();
    // Core sniffs animation itself and then always serves the original bytes
    // (x-mangoo-animated), so a large still PNG still gets an adaptive tier.
    const resource = await client.getImageViewerImage(entry.path, {
      viewportLongPx: options.viewportLongPx,
      full: options.full,
      // The picture on screen jumps every queue (filmstrip thumbnails, neighbours).
      requestPriority: options.priority ?? "navigation",
      ...(options.rawSource === "embedded" ? { rawSource: "embedded" as const } : {})
    });
    if (resource.animated || (options.animatedExtensions.includes(entry.ext) && resource.tier === "original")) {
      const header = new Uint8Array(await resource.blob.slice(0, 64).arrayBuffer());
      if (isAnimatedImageHeader(header, entry.ext)) {
        const url = URL.createObjectURL(resource.blob);
        try {
          const size = await imageElementSize(url);
          const image: LoadedImage = {
            kind: "animated", key, url, blob: resource.blob, width: size.width, height: size.height, tier: resource.tier, upgradable: false
          };
          this.store(key, image, resource.blob.size + size.width * size.height * 4);
          return image;
        } catch (error) {
          URL.revokeObjectURL(url);
          throw error;
        }
      }
    }
    const bitmap = await decodeBitmap(resource.blob);
    const width = resource.sourceWidth ?? bitmap.width;
    const height = resource.sourceHeight ?? bitmap.height;
    const image: LoadedImage = {
      kind: "bitmap",
      key,
      bitmap,
      width,
      height,
      tier: resource.tier,
      // Only adaptive tiers of a browser-paintable original can sharpen further.
      // A transcoded RAW / HEIC also has a sharper (native-size) version.
      upgradable: resource.tier === "preview_2048" || resource.tier === "preview_3072"
    };
    this.store(key, image, bitmap.width * bitmap.height * 4);
    return image;
  }

  private store(key: string, image: LoadedImage, bytes: number): void {
    const previous = this.records.get(key);
    if (previous) this.release(key, previous);
    this.records.set(key, { image, bytes, used: ++this.clock });
    this.used += bytes;
    this.evict();
  }

  private release(key: string, record: CacheRecord): void {
    this.records.delete(key);
    this.used -= record.bytes;
    if (record.image.kind === "bitmap") record.image.bitmap.close();
    else URL.revokeObjectURL(record.image.url);
  }

  private evict(): void {
    if (this.used <= this.budgetBytes) return;
    const candidates = [...this.records.entries()]
      .filter(([key]) => !this.pinned.has(key))
      .sort((left, right) => left[1].used - right[1].used);
    for (const [key, record] of candidates) {
      if (this.used <= this.budgetBytes) break;
      this.release(key, record);
    }
  }

  /** Drop every cached rendition of one file (renamed / deleted / changed). */
  forget(path: string): void {
    for (const [key, record] of [...this.records.entries()]) {
      if (key.startsWith(`${path}\u0000`)) this.release(key, record);
    }
  }

  clear(): void {
    for (const [key, record] of [...this.records.entries()]) this.release(key, record);
  }
}

/** Longest window side in device pixels, rounded up to 128 like the Library preview. */
export function viewerViewportLongPx(): number {
  const ratio = window.devicePixelRatio || 1;
  const longest = Math.max(window.innerWidth, window.innerHeight) * ratio;
  return Math.max(128, Math.ceil(longest / 128) * 128);
}

/**
 * The picture on screen always loads before filmstrip thumbnails: thumbnail
 * requests wait on this gate, which the viewer closes while it is loading the
 * main picture and opens once it is painted.
 */
let thumbnailGate: Promise<void> = Promise.resolve();
let openThumbnailGate: (() => void) | null = null;
let thumbnailGateTimer: ReturnType<typeof setTimeout> | null = null;
/** A slow RAW develop or a hung fetch must not freeze the strip. */
const THUMBNAIL_HOLD_MAX_MS = 1500;
export function holdThumbnails(hold: boolean): void {
  if (hold && !openThumbnailGate) {
    thumbnailGate = new Promise((resolve) => {
      openThumbnailGate = resolve;
    });
    thumbnailGateTimer = setTimeout(() => holdThumbnails(false), THUMBNAIL_HOLD_MAX_MS);
  } else if (!hold && openThumbnailGate) {
    if (thumbnailGateTimer) clearTimeout(thumbnailGateTimer);
    thumbnailGateTimer = null;
    openThumbnailGate();
    openThumbnailGate = null;
  }
}

const thumbnailUrls = new Map<string, Promise<string>>();
const THUMBNAIL_URL_LIMIT = 600;

/** Filmstrip thumbnail object URL, cached per path + mtime. */
/** Resolved thumbnail URLs, readable synchronously (instant scrub preview). */
const readyThumbnails = new Map<string, string>();
/** Keys whose request still waits at the thumbnail gate (not yet sent). */
const gatedThumbnails = new Set<string>();

function thumbnailKey(entry: ImageEntry, size: number): string {
  return `${entry.path}\u0000${entry.modifiedAtMs}\u0000${size}`;
}

/** A thumbnail already fetched, or null. */
export function readyThumbnailUrl(entry: ImageEntry, size: number): string | null {
  return readyThumbnails.get(thumbnailKey(entry, size)) ?? null;
}

/**
 * `urgent`: the user is looking at it right now (scrub-bar preview). It skips
 * the "picture first" gate and jumps Core's background queue.
 */
export function thumbnailUrl(entry: ImageEntry, size: number, urgent = false): Promise<string> {
  const key = thumbnailKey(entry, size);
  const existing = thumbnailUrls.get(key);
  // An urgent caller does not wait behind a request still parked at the
  // "picture first" gate: it issues its own (the parked one is dropped).
  if (existing && !(urgent && gatedThumbnails.has(key))) {
    thumbnailUrls.delete(key);
    thumbnailUrls.set(key, existing);
    return existing;
  }
  const request = () => {
    gatedThumbnails.delete(key);
    return createCoreClient().getImageViewerThumbnail(entry.path, { size, requestPriority: urgent ? "interactive" : "background" });
  };
  if (!urgent) gatedThumbnails.add(key);
  const promise: Promise<string> = (urgent ? request() : thumbnailGate.then(request))
    .then((blob) => {
      const url = URL.createObjectURL(blob);
      // Evicted (or superseded) while in flight: nobody owns this URL.
      if (thumbnailUrls.get(key) !== promise) {
        URL.revokeObjectURL(url);
        throw new Error("thumbnail superseded");
      }
      readyThumbnails.set(key, url);
      return url;
    });
  promise.catch(() => {
    if (thumbnailUrls.get(key) === promise) thumbnailUrls.delete(key);
  });
  thumbnailUrls.set(key, promise);
  while (thumbnailUrls.size > THUMBNAIL_URL_LIMIT) {
    const [oldestKey, oldest] = thumbnailUrls.entries().next().value as [string, Promise<string>];
    thumbnailUrls.delete(oldestKey);
    readyThumbnails.delete(oldestKey);
    gatedThumbnails.delete(oldestKey);
    void oldest.then((url) => URL.revokeObjectURL(url), () => undefined);
  }
  return promise;
}

/** Encode what is on screen as PNG bytes for the clipboard. */
export async function encodePng(image: LoadedImage): Promise<Uint8Array> {
  const bitmap = image.kind === "bitmap" ? image.bitmap : await createImageBitmap(image.blob);
  try {
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const context = canvas.getContext("2d");
    if (!context) throw new Error("无法创建画布");
    context.drawImage(bitmap, 0, 0);
    const blob = await canvas.convertToBlob({ type: "image/png" });
    return new Uint8Array(await blob.arrayBuffer());
  } finally {
    if (image.kind !== "bitmap") bitmap.close();
  }
}

export interface Histogram {
  red: Uint32Array;
  green: Uint32Array;
  blue: Uint32Array;
  luma: Uint32Array;
  max: number;
  /** Opaque pixels sampled. */
  samples: number;
  /** Luma mean / median / standard deviation (0..255). */
  mean: number;
  median: number;
  deviation: number;
  /** Share of pixels with any channel at 0 / 255 (clipped shadows / highlights). */
  clippedShadows: number;
  clippedHighlights: number;
  /** Share of pixels that are not fully opaque. */
  transparent: number;
}

/** RGB + luma histogram from a downscaled copy of the picture. */
export async function computeHistogram(image: LoadedImage): Promise<Histogram> {
  const source = image.kind === "bitmap" ? image.bitmap : await createImageBitmap(image.blob);
  try {
    const scale = Math.min(1, 384 / Math.max(source.width, source.height));
    const width = Math.max(1, Math.round(source.width * scale));
    const height = Math.max(1, Math.round(source.height * scale));
    const canvas = new OffscreenCanvas(width, height);
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) throw new Error("无法创建画布");
    context.drawImage(source, 0, 0, width, height);
    const pixels = context.getImageData(0, 0, width, height).data;
    const red = new Uint32Array(256);
    const green = new Uint32Array(256);
    const blue = new Uint32Array(256);
    const luma = new Uint32Array(256);
    let samples = 0;
    let shadows = 0;
    let highlights = 0;
    let translucent = 0;
    let sum = 0;
    let sumSquares = 0;
    for (let index = 0; index < pixels.length; index += 4) {
      const alpha = pixels[index + 3];
      if (alpha < 255) translucent += 1;
      if (alpha === 0) continue;
      const r = pixels[index];
      const g = pixels[index + 1];
      const b = pixels[index + 2];
      red[r] += 1;
      green[g] += 1;
      blue[b] += 1;
      const y = Math.round(0.2126 * r + 0.7152 * g + 0.0722 * b);
      luma[y] += 1;
      samples += 1;
      sum += y;
      sumSquares += y * y;
      if (r === 0 || g === 0 || b === 0) shadows += 1;
      if (r === 255 || g === 255 || b === 255) highlights += 1;
    }
    const mean = samples ? sum / samples : 0;
    const deviation = samples ? Math.sqrt(Math.max(0, sumSquares / samples - mean * mean)) : 0;
    let median = 0;
    for (let value = 0, seen = 0; value < 256; value += 1) {
      seen += luma[value];
      if (seen * 2 >= samples) {
        median = value;
        break;
      }
    }
    const total = pixels.length / 4 || 1;
    let max = 1;
    for (const channel of [red, green, blue, luma]) {
      // Ignore the clipped extremes when scaling, like most editors.
      for (let value = 1; value < 255; value += 1) max = Math.max(max, channel[value]);
    }
    return {
      red, green, blue, luma, max, samples, mean, median, deviation,
      clippedShadows: samples ? shadows / samples : 0,
      clippedHighlights: samples ? highlights / samples : 0,
      transparent: translucent / total
    };
  } finally {
    if (image.kind !== "bitmap") source.close();
  }
}
