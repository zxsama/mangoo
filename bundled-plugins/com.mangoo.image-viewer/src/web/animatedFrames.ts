import { buildTimeline, normalizeFrameDuration, type FrameTimeline } from "../shared/frames.ts";

/**
 * Frame-accurate access to animated images through WebCodecs `ImageDecoder`
 * (GIF, APNG, animated WebP / AVIF in Chromium). Frames decode on demand and
 * are cached as `ImageBitmap`s within a byte budget; every bitmap is closed
 * when evicted or when the session is disposed.
 */

const MIME_BY_EXTENSION: Record<string, string> = {
  ".gif": "image/gif",
  ".png": "image/png",
  ".apng": "image/apng",
  ".webp": "image/webp",
  ".avif": "image/avif"
};

// Minimal structural types: lib.dom in this TypeScript version may lack them.
interface DecodedVideoFrame {
  displayWidth: number;
  displayHeight: number;
  duration: number | null;
  close(): void;
}
interface ImageDecoderLike {
  tracks: { ready: Promise<void>; selectedTrack: { frameCount: number; repetitionCount: number; animated: boolean } | null };
  completed: Promise<void>;
  decode(options: { frameIndex: number; completeFramesOnly?: boolean }): Promise<{ image: DecodedVideoFrame }>;
  close(): void;
}
type ImageDecoderConstructor = {
  new (init: { data: ArrayBuffer | ReadableStream; type: string }): ImageDecoderLike;
  isTypeSupported(type: string): Promise<boolean>;
};

function decoderClass(): ImageDecoderConstructor | null {
  return (globalThis as { ImageDecoder?: ImageDecoderConstructor }).ImageDecoder ?? null;
}

export function frameInspectionSupported(): boolean {
  return decoderClass() !== null;
}

export class AnimatedFrames {
  readonly frameCount: number;
  readonly width: number;
  readonly height: number;
  /** 0 = loop forever (GIF NETSCAPE semantics), otherwise play this many extra times. */
  readonly repetitionCount: number;
  readonly timeline: FrameTimeline;
  private readonly decoder: ImageDecoderLike;
  private readonly cache = new Map<number, ImageBitmap>();
  private readonly pending = new Map<number, Promise<ImageBitmap>>();
  private readonly budgetFrames: number;
  private disposed = false;

  private constructor(decoder: ImageDecoderLike, frameCount: number, width: number, height: number, repetition: number, durations: number[]) {
    this.decoder = decoder;
    this.frameCount = frameCount;
    this.width = width;
    this.height = height;
    this.repetitionCount = repetition;
    this.timeline = buildTimeline(durations);
    const frameBytes = Math.max(1, width * height * 4);
    this.budgetFrames = Math.max(8, Math.floor((256 * 1024 * 1024) / frameBytes));
  }

  /** null when the platform cannot decode frames or the image is not animated. */
  static async open(blob: Blob, extension: string): Promise<AnimatedFrames | null> {
    const Decoder = decoderClass();
    const type = MIME_BY_EXTENSION[extension];
    if (!Decoder || !type || !(await Decoder.isTypeSupported(type).catch(() => false))) return null;
    const decoder = new Decoder({ data: await blob.arrayBuffer(), type });
    try {
      await decoder.tracks.ready;
      await decoder.completed.catch(() => undefined);
      const track = decoder.tracks.selectedTrack;
      if (!track || track.frameCount < 2) {
        decoder.close();
        return null;
      }
      // Durations need a decode per frame; frames are small for typical
      // animations, and we close each VideoFrame immediately.
      const durations: number[] = [];
      let width = 0;
      let height = 0;
      for (let index = 0; index < track.frameCount; index += 1) {
        const { image } = await decoder.decode({ frameIndex: index, completeFramesOnly: true });
        if (index === 0) {
          width = image.displayWidth;
          height = image.displayHeight;
        }
        durations.push(normalizeFrameDuration(image.duration === null ? null : image.duration / 1000));
        image.close();
      }
      const repetition = Number.isFinite(track.repetitionCount) ? track.repetitionCount : 0;
      return new AnimatedFrames(decoder, track.frameCount, width, height, repetition, durations);
    } catch {
      decoder.close();
      return null;
    }
  }

  async frame(index: number): Promise<ImageBitmap> {
    if (this.disposed) throw new Error("frames disposed");
    const cached = this.cache.get(index);
    if (cached) {
      this.cache.delete(index);
      this.cache.set(index, cached);
      return cached;
    }
    const inflight = this.pending.get(index);
    if (inflight) return inflight;
    const promise = (async () => {
      const { image } = await this.decoder.decode({ frameIndex: index, completeFramesOnly: true });
      try {
        const bitmap = await createImageBitmap(image as unknown as CanvasImageSource);
        if (this.disposed) {
          bitmap.close();
          throw new Error("frames disposed");
        }
        this.cache.set(index, bitmap);
        while (this.cache.size > this.budgetFrames) {
          const [oldestIndex, oldest] = this.cache.entries().next().value as [number, ImageBitmap];
          this.cache.delete(oldestIndex);
          oldest.close();
        }
        return bitmap;
      } finally {
        image.close();
      }
    })().finally(() => this.pending.delete(index));
    this.pending.set(index, promise);
    return promise;
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    for (const bitmap of this.cache.values()) bitmap.close();
    this.cache.clear();
    this.decoder.close();
  }
}
