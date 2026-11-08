import { useCallback, useEffect, useRef, useState } from "react";

import { frameAt, stepFrame } from "../shared/frames.ts";
import { AnimatedFrames } from "./animatedFrames.ts";
import type { FramePlaybackState } from "./FrameInspector.tsx";
import type { LoadedImage } from "./imageSource.ts";

/**
 * Frame control for the picture on screen. While the user just watches, the
 * browser's own <img> keeps playing the animation (cheapest, exact timing).
 * As soon as the frame inspector is opened, the canvas takes over: frames are
 * decoded with WebCodecs and painted by this hook, so pause / step / scrub /
 * speed / loop all work, and closing the inspector hands back to <img>.
 */
export function useAnimationFrames(image: LoadedImage | null, ext: string | null, active: boolean) {
  const [frames, setFrames] = useState<AnimatedFrames | null>(null);
  const [bitmap, setBitmap] = useState<ImageBitmap | null>(null);
  const [index, setIndex] = useState(0);
  const [playing, setPlaying] = useState(true);
  const [speed, setSpeed] = useState(1);
  const [loop, setLoop] = useState(true);
  const clock = useRef({ origin: 0, offset: 0 });
  /** When <img> started playing natively, to resume on the frame on screen. */
  const nativeStart = useRef(performance.now());
  const indexRef = useRef(0);
  indexRef.current = index;

  // Probe: is the current picture animated (and frame-decodable)?
  useEffect(() => {
    let cancelled = false;
    setFrames(null);
    setBitmap(null);
    setIndex(0);
    setPlaying(true);
    nativeStart.current = performance.now();
    if (!image || image.kind !== "animated" || !ext) return;
    void AnimatedFrames.open(image.blob, ext).then((opened) => {
      if (cancelled) {
        opened?.dispose();
        return;
      }
      setFrames(opened);
    });
    return () => {
      cancelled = true;
    };
  }, [image, ext]);

  useEffect(() => () => frames?.dispose(), [frames]);

  // Taking over from native <img> playback: start on the frame being shown.
  const wasActive = useRef(false);
  useEffect(() => {
    if (active && !wasActive.current && frames) {
      setIndex(frameAt(frames.timeline, performance.now() - nativeStart.current));
    }
    if (!active && wasActive.current) nativeStart.current = performance.now();
    wasActive.current = active;
  }, [active, frames]);

  // Paint the selected frame while the inspector controls playback.
  useEffect(() => {
    if (!frames || !active) {
      setBitmap(null);
      return;
    }
    let cancelled = false;
    void frames.frame(index).then((next) => {
      if (!cancelled) setBitmap(next);
    }, () => undefined);
    return () => {
      cancelled = true;
    };
  }, [frames, index, active]);

  // Playback clock.
  useEffect(() => {
    if (!frames || !active || !playing) return;
    const timeline = frames.timeline;
    clock.current = { origin: performance.now(), offset: timeline.starts[indexRef.current] ?? 0 };
    let raf = 0;
    const tick = () => {
      const elapsed = clock.current.offset + (performance.now() - clock.current.origin) * speed;
      if (!loop && elapsed >= timeline.total) {
        setIndex(frames.frameCount - 1);
        setPlaying(false);
        return;
      }
      const next = frameAt(timeline, elapsed);
      if (next !== indexRef.current) setIndex(next);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [frames, active, playing, speed, loop]);

  const playPause = useCallback(() => {
    if (!frames) return;
    setPlaying((value) => {
      // Restart from the beginning when play is pressed at the end of a non-looping run.
      if (!value && !loop && indexRef.current === frames.frameCount - 1) setIndex(0);
      return !value;
    });
  }, [frames, loop]);

  const step = useCallback((delta: number) => {
    if (!frames) return;
    setPlaying(false);
    setIndex((value) => stepFrame(frames.frameCount, value, delta));
  }, [frames]);

  const seek = useCallback((target: number) => {
    if (!frames) return;
    setPlaying(false);
    setIndex(Math.max(0, Math.min(frames.frameCount - 1, Math.round(target))));
  }, [frames]);

  const state: FramePlaybackState | null = frames
    ? {
        frameCount: frames.frameCount,
        index,
        playing,
        speed,
        loop,
        durations: frames.timeline.durations,
        startTimes: frames.timeline.starts,
        totalMs: frames.timeline.total
      }
    : null;

  return { state, bitmap: active ? bitmap : null, playPause, step, seek, setSpeed, setLoop };
}
