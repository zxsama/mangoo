import { useCallback, useEffect, useRef, useState } from "react";
import type { MediaRecord } from "@mangoo/core-client";
import { mediaFileContentSignature, registerMediaCacheResetListener } from "../../core/mediaResources";
import { createVideoPlaybackLease } from "../../core/videoPlaybackSession";
import { formatMediaTime, playbackProgress } from "../preview/videoPlayback";
import { acquireVideoHover, horizontalHoverRatio, hoverSeekTime, VIDEO_HOVER_RESUME_MS } from "./videoHoverModel";

/** Passive overlay: selection, preview, keyboard and locate stay on MediaTile. */
export function VideoThumbnailHover({ item }: { item: MediaRecord }) {
  const host = useRef<HTMLDivElement>(null);
  const video = useRef<HTMLVideoElement | null>(null);
  const attachVideo = useCallback((player: HTMLVideoElement | null) => {
    const previous = video.current;
    if (previous && previous !== player) { previous.pause(); previous.removeAttribute("src"); previous.load(); }
    video.current = player;
  }, []);
  const [source, setSource] = useState<string | null>(null);
  const [visible, setVisible] = useState(false);
  const [clock, setClock] = useState({ current: 0, duration: 0, progress: 0 });
  const events = useRef({ metadata: () => {}, seeked: () => {}, time: () => {}, error: () => {} });
  const signature = `${mediaFileContentSignature(item)}:${item.rootId}:${item.folderId}:${item.name}`;

  useEffect(() => {
    const shell = host.current?.parentElement;
    if (!shell) return;
    let generation = 0;
    let controller: AbortController | null = null;
    let releaseOwner: (() => void) | undefined;
    let lease: Awaited<ReturnType<typeof createVideoPlaybackLease>> | undefined;
    let resume: ReturnType<typeof setTimeout> | undefined;
    let seekFrame = 0;
    let progressFrame = 0;
    let previousX = 0;
    let ratio: number | null = null;
    // Pointer presentation stays independent of the decoder's pending seek.
    let displayRatio: number | null = null;
    let seekingTarget: number | null = null;
    let shouldPlay = true;
    let cleanupActive: (() => void) | undefined;

    const stop = () => {
      generation++;
      clearTimeout(resume);
      cancelAnimationFrame(seekFrame); cancelAnimationFrame(progressFrame);
      seekFrame = 0; progressFrame = 0;
      controller?.abort(); controller = null;
      lease?.release(); lease = undefined;
      releaseOwner?.(); releaseOwner = undefined;
      cleanupActive?.(); cleanupActive = undefined;
      const player = video.current;
      if (player) { player.pause(); player.removeAttribute("src"); player.load(); }
      ratio = null; displayRatio = null; seekingTarget = null;
      setSource(null); setVisible(false);
      setClock({ current: 0, duration: 0, progress: 0 });
    };
    const alive = () => controller !== null && !controller.signal.aborted;
    const time = () => {
      const player = video.current;
      if (!alive()) return;
      const duration = player && Number.isFinite(player.duration) && player.duration > 0 ? player.duration : 0;
      const current = displayRatio !== null ? displayRatio * duration :
        player && Number.isFinite(player.currentTime) ? player.currentTime : 0;
      const progress = displayRatio ?? playbackProgress(current, duration);
      setClock((previous) => previous.current === current && previous.duration === duration && previous.progress === progress
        ? previous : { current, duration, progress });
    };
    const play = () => {
      const player = video.current;
      if (!alive() || !player || !shouldPlay || ratio !== null || seekingTarget !== null || player.seeking) return;
      displayRatio = null;
      time();
      player.muted = true; player.volume = 0; player.playbackRate = 1;
      const currentGeneration = generation;
      void player.play().then(() => {
        if (generation === currentGeneration && alive() && !player.paused && !progressFrame) progressFrame = requestAnimationFrame(tick);
      }).catch((error: unknown) => {
        if (generation !== currentGeneration || !alive()) return;
        if (error instanceof DOMException && error.name === "AbortError") return;
        stop();
      });
    };
    const seek = () => {
      seekFrame = 0;
      const player = video.current;
      if (!alive() || !player || ratio === null || player.seeking || seekingTarget !== null) return;
      if (player.readyState < 1) return;
      const target = hoverSeekTime(ratio, player.duration);
      ratio = null;
      // Metadata can be loaded without a finite duration. An unusable seek
      // must not keep the idle-resume path permanently paused.
      if (target === null) { play(); return; }
      seekingTarget = target;
      try { player.currentTime = target; } catch { stop(); return; }
      time();
      if (!player.seeking) seeked();
    };
    const seeked = () => {
      const player = video.current;
      if (!alive() || !player || player.seeking) return;
      // A late event from an older position must not release the latest intent.
      if (seekingTarget !== null && Math.abs(player.currentTime - seekingTarget) > 0.05) return;
      seekingTarget = null;
      if (ratio !== null) seek(); else play();
      time();
    };
    const tick = () => {
      progressFrame = 0;
      if (!alive() || !video.current || video.current.paused) return;
      time();
      progressFrame = requestAnimationFrame(tick);
    };
    events.current = {
      metadata: () => {
        const player = video.current;
        if (!alive() || !player) return;
        player.muted = true; player.volume = 0; player.playbackRate = 1;
        if (ratio !== null) seek(); else play();
        time();
      }, seeked, time, error: stop
    };
    const isLocate = (target: EventTarget | null) => target instanceof Element && !!target.closest(".media-tile-locate-button");
    const begin = (event: PointerEvent) => {
      if (event.pointerType !== "mouse" || !window.matchMedia("(any-hover: hover) and (any-pointer: fine)").matches ||
          document.hidden || !document.hasFocus() || isLocate(event.target)) return;
      stop();
      releaseOwner = acquireVideoHover(stop);
      previousX = event.clientX; shouldPlay = true;
      controller = new AbortController();
      const pending = controller;
      const currentGeneration = generation;
      const hide = () => { if (document.hidden) stop(); };
      window.addEventListener("blur", stop);
      window.addEventListener("scroll", stop, true);
      window.addEventListener("resize", stop);
      document.addEventListener("visibilitychange", hide);
      const observer = new IntersectionObserver((entries) => {
        if (entries.some((entry) => !entry.isIntersecting)) stop();
      });
      observer.observe(shell);
      cleanupActive = () => {
        observer.disconnect();
        window.removeEventListener("blur", stop);
        window.removeEventListener("scroll", stop, true);
        window.removeEventListener("resize", stop);
        document.removeEventListener("visibilitychange", hide);
      };
      void createVideoPlaybackLease(item.id, pending.signal, stop, (src) => {
        if (generation !== currentGeneration || pending.signal.aborted) return;
        const player = video.current;
        if (displayRatio !== null) ratio = displayRatio;
        else if (player && Number.isFinite(player.duration) && player.duration > 0) ratio = player.currentTime / player.duration;
        seekingTarget = null;
        setVisible(false);
        setSource(src);
      }).then((next) => {
        if (generation !== currentGeneration || pending.signal.aborted) { next.release(); return; }
        lease = next;
        setSource(next.session.src);
      }).catch(() => { if (generation === currentGeneration) stop(); });
    };
    const move = (event: PointerEvent) => {
      if (isLocate(event.target)) { stop(); return; }
      if (!alive() || event.pointerType !== "mouse") return;
      const bounds = shell.getBoundingClientRect();
      const next = horizontalHoverRatio(previousX, event.clientX, bounds.left, bounds.width);
      if (next === null) return;
      previousX = event.clientX;
      ratio = next; displayRatio = next; shouldPlay = false;
      video.current?.pause();
      cancelAnimationFrame(progressFrame); progressFrame = 0;
      time();
      clearTimeout(resume);
      if (!seekFrame) seekFrame = requestAnimationFrame(seek);
      resume = setTimeout(() => { shouldPlay = true; if (ratio !== null) seek(); else play(); }, VIDEO_HOVER_RESUME_MS);
    };
    shell.addEventListener("pointerenter", begin);
    shell.addEventListener("pointermove", move);
    shell.addEventListener("pointerleave", stop);
    shell.addEventListener("pointercancel", stop);
    shell.addEventListener("contextmenu", stop);
    const unregister = registerMediaCacheResetListener(stop);
    return () => {
      stop(); unregister();
      shell.removeEventListener("pointerenter", begin);
      shell.removeEventListener("pointermove", move);
      shell.removeEventListener("pointerleave", stop);
      shell.removeEventListener("pointercancel", stop);
      shell.removeEventListener("contextmenu", stop);
    };
  }, [item.id, signature]);

  return <div aria-hidden="true" className="video-hover" ref={host} data-playing={source !== null && visible ? "true" : undefined}>
    {source && <>
      <video className="video-hover-player" ref={attachVideo} src={source} muted loop playsInline preload="metadata" tabIndex={-1}
        onLoadedMetadata={() => events.current.metadata()} onLoadedData={() => setVisible(true)}
        onSeeked={() => events.current.seeked()} onTimeUpdate={() => events.current.time()}
        onError={() => events.current.error()} />
      <span className="video-hover-time">{formatMediaTime(clock.current)} / {formatMediaTime(clock.duration)}</span>
      <span className="video-hover-progress"><span style={{ transform: `scaleX(${clock.progress})` }} /></span>
    </>}
  </div>;
}
