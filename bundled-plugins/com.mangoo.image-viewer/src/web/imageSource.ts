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

  static key(entry: ImageEntry, full: boolean): string {
    return `${entry.path}\u0000${entry.modifiedAtMs}\u0000${entry.sizeBytes}\u0000${full ? "full" : "fit"}`;
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
    options: { viewportLongPx: number; full: boolean; animatedExtensions: readonly string[]; signal?: AbortSignal; priority?: "interactive" | "resource" }
  ): Promise<LoadedImage> {
    const key = ViewerImageCache.key(entry, options.full);
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
    options: { viewportLongPx: number; full: boolean; animatedExtensions: readonly string[]; priority?: "interactive" | "resource" }
  ): Promise<LoadedImage> {
    const client = createCoreClient();
    const animatedCandidate = options.animatedExtensions.includes(entry.ext);
    // Animated candidates always need the original bytes to keep their frames.
    const resource = await client.getImageViewerImage(entry.path, {
      viewportLongPx: options.viewportLongPx,
      full: options.full || animatedCandidate,
      requestPriority: options.priority ?? "interactive"
    });
    if (animatedCandidate && resource.tier === "original") {
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

const thumbnailUrls = new Map<string, Promise<string>>();
const THUMBNAIL_URL_LIMIT = 600;

/** Filmstrip thumbnail object URL, cached per path + mtime. */
export function thumbnailUrl(entry: ImageEntry, size: number): Promise<string> {
  const key = `${entry.path}\u0000${entry.modifiedAtMs}\u0000${size}`;
  const existing = thumbnailUrls.get(key);
  if (existing) {
    thumbnailUrls.delete(key);
    thumbnailUrls.set(key, existing);
    return existing;
  }
  const promise = createCoreClient()
    .getImageViewerThumbnail(entry.path, { size, requestPriority: "background" })
    .then((blob) => URL.createObjectURL(blob));
  promise.catch(() => thumbnailUrls.delete(key));
  thumbnailUrls.set(key, promise);
  while (thumbnailUrls.size > THUMBNAIL_URL_LIMIT) {
    const [oldestKey, oldest] = thumbnailUrls.entries().next().value as [string, Promise<string>];
    thumbnailUrls.delete(oldestKey);
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
    for (let index = 0; index < pixels.length; index += 4) {
      if (pixels[index + 3] === 0) continue;
      const r = pixels[index];
      const g = pixels[index + 1];
      const b = pixels[index + 2];
      red[r] += 1;
      green[g] += 1;
      blue[b] += 1;
      luma[Math.round(0.2126 * r + 0.7152 * g + 0.0722 * b)] += 1;
    }
    let max = 1;
    for (const channel of [red, green, blue, luma]) {
      // Ignore the clipped extremes when scaling, like most editors.
      for (let value = 1; value < 255; value += 1) max = Math.max(max, channel[value]);
    }
    return { red, green, blue, luma, max };
  } finally {
    if (image.kind !== "bitmap") source.close();
  }
}
