import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  AnchoredOverlay,
  LiquidGlassButton,
  LiquidGlassLayer,
  LiquidGlassSurface,
  MediaLoadingRing,
  ModalSurface,
  OverlayDialogActions,
  OverlayDialogBody,
  OverlayDialogHeader,
  OverlayField,
  OverlayInlineStatus,
  OverlayMenuItem,
  OverlayMenuList,
  WindowChrome,
  createCoreClient,
  getWindowControls,
  icons,
  pointOverlayAnchor,
  useOverlayScrollbars,
  useSecondaryWindowInterfaceStyle
} from "@mangoo/web/plugin-host";

import { extensionOf, splitPath } from "../shared/formats.ts";
import { formatBytes, prefetchOrder, validateFileName } from "../shared/navigation.ts";
import { formatChord, matchAction, resolveBindings, type ShortcutActionId } from "../shared/shortcuts.ts";
import {
  fitScale,
  formatZoom,
  identityTransform,
  nextZoomStop,
  softClampPan,
  renderLevel,
  rotate,
  zoomAt,
  type FitMode,
  type Size,
  type ViewTransform
} from "../shared/viewTransform.ts";
import { FloatingPanel } from "./FloatingPanel.tsx";
import { FrameInspector } from "./FrameInspector.tsx";
import { useAnimationFrames } from "./useAnimationFrames.ts";
import { useProximityChrome } from "./useProximityChrome.ts";
import { Filmstrip } from "./Filmstrip.tsx";
import { ComicView, comicScrollBy } from "./ComicView.tsx";
import { ScrubBar } from "./ScrubBar.tsx";
import { InfoPanel } from "./InfoPanel.tsx";
import { viewerBridge } from "./bridge.ts";
import { readyThumbnailUrl, thumbnailUrl, ViewerImageCache, encodePng, holdThumbnails, viewerViewportLongPx, type LoadedImage, type RawSource } from "./imageSource.ts";
import { useViewerPreferences } from "./preferences.ts";
import { describeError, hiddenPairOf, useViewerSession } from "./useViewerSession.ts";
import { ViewerStage, type StageAnimation, type ViewerChannel } from "./ViewerStage.tsx";
import "./viewer.css";

type ZoomState = FitMode | "custom";
type Dialog =
  | { kind: "delete"; dontAskAgain: boolean }
  | { kind: "rename"; value: string; error: string | null; busy: boolean }
  | null;

/** How long each kind of zoom animates (ms); see .iv-image-layer[data-animate]. */
const ANIMATION_MS: Record<Exclude<StageAnimation, "none">, number> = { ease: 200, wheel: 170, pinch: 90 };

/** Height reserved by the titlebar when it is pinned (px). */
const TITLEBAR_HEIGHT = 40;
const FILMSTRIP_HEIGHT = 92;
/** Filmstrip pane height with the scrub bar row on top. */
const FILMSTRIP_WITH_SCRUB_HEIGHT = 106;
const TOOLBAR_CLEARANCE = 64;

const ZOOM_PRESETS = [0.25, 0.5, 1, 2, 4];

/** Desktop host errors can be English OS / Node messages; say it in Chinese. */
function friendlyActionError(error: string | undefined, action: string): string {
  if (!error) return `${action}，请重试。`;
  if (/[\u4e00-\u9fa5]/.test(error)) return error;
  if (/outside the folders|path/i.test(error)) return `${action}：文件路径无效或不在当前文件夹中。`;
  if (/EPERM|EACCES|denied|permission/i.test(error)) return `${action}：没有权限。`;
  if (/ENOENT|not found|no such/i.test(error)) return `${action}：文件已不存在。`;
  if (/EBUSY|in use|locked/i.test(error)) return `${action}：文件正被其他程序使用。`;
  return `${action}。`;
}

function isEditableTarget(target: EventTarget | null): boolean {
  const element = target as HTMLElement | null;
  return Boolean(element && (element.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(element.tagName)));
}

function IconButton({
  label,
  shortcut,
  onClick,
  active,
  disabled,
  children,
  buttonRef,
  danger
}: {
  label: string;
  shortcut?: string;
  onClick(): void;
  active?: boolean;
  disabled?: boolean;
  children: ReactNode;
  buttonRef?: React.Ref<HTMLButtonElement>;
  danger?: boolean;
}) {
  return (
    <LiquidGlassButton
      active={active}
      aria-label={label}
      aria-pressed={active === undefined ? undefined : active}
      className="iv-icon-button no-drag"
      disabled={disabled}
      onClick={onClick}
      ref={buttonRef}
      title={shortcut ? `${label}（${shortcut}）` : label}
      data-pointer-glow="off"
      tone={danger ? "danger" : "control"}
      type="button"
    >
      {children}
    </LiquidGlassButton>
  );
}

export function ImageViewerWindow() {
  useSecondaryWindowInterfaceStyle();
  // The Host's overlay scrollbars (filmstrip, panels): they come back on
  // hover and scroll, unlike the native thin bar that faded out for good.
  useOverlayScrollbars();
  const bridge = viewerBridge();
  const [preferences, setPreferences] = useViewerPreferences();
  const session = useViewerSession(preferences.sortKey, preferences.sortDirection, preferences.loop, preferences.pairRawJpeg);
  const { state, current } = session;
  const bindings = useMemo(() => resolveBindings(preferences.shortcuts), [preferences.shortcuts]);
  /** First chord of an action, formatted for tooltips and menus. */
  const keyOf = useCallback((action: ShortcutActionId) => {
    const chord = bindings[action][0];
    return chord ? formatChord(chord) : undefined;
  }, [bindings]);
  const [framesOpen, setFramesOpen] = useState(false);
  /** Single-channel view (null = normal RGBA). */
  const [channel, setChannel] = useState<ViewerChannel | null>(null);
  /** Comic mode: scroll a picture to the top (← →, filmstrip, scrub bar). */
  const [comicJump, setComicJump] = useState<{ index: number; token: number } | null>(null);
  const comicJumpToken = useRef(0);
  /** Thumbnail of a picture still loading, painted over the stage meanwhile. */
  const [standIn, setStandIn] = useState<string | null>(null);
  /** Camera RAW: developed sensor data or the camera's embedded JPEG. */
  const [rawSource, setRawSource] = useState<RawSource>("developed");
  const isRaw = Boolean(current?.cameraRaw);
  const effectiveRawSource: RawSource = isRaw ? rawSource : "developed";
  const cache = useMemo(() => new ViewerImageCache(), []);

  const [animatedExtensions, setAnimatedExtensions] = useState<string[]>([".gif", ".webp", ".avif"]);
  const [globalError, setGlobalError] = useState<string | null>(null);
  const [image, setImage] = useState<LoadedImage | null>(null);
  const [imageError, setImageError] = useState<string | null>(null);
  const [imageLoading, setImageLoading] = useState(false);
  const [viewport, setViewport] = useState<Size>({ width: 0, height: 0 });
  const [dpr, setDpr] = useState(() => window.devicePixelRatio || 1);
  const [transform, setTransform] = useState<ViewTransform>(identityTransform);
  const [zoomState, setZoomState] = useState<ZoomState>(preferences.fitMode);
  const [animate, setAnimate] = useState<StageAnimation>("none");
  const [fullscreen, setFullscreen] = useState(false);
  /** The OS window is maximized (its floating titlebar drags natively then). */
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    const controls = getWindowControls();
    if (!controls) return;
    let cancelled = false;
    const refresh = () => void controls.isMaximized().then((value) => {
      if (!cancelled) setMaximized(value);
    });
    refresh();
    window.addEventListener("resize", refresh);
    return () => {
      cancelled = true;
      window.removeEventListener("resize", refresh);
    };
  }, []);
  const [slideshow, setSlideshow] = useState(false);
  const [chromeVisible, setChromeVisible] = useState(true);
  const [zoomMenuOpen, setZoomMenuOpen] = useState(false);
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; byKeyboard?: boolean } | null>(null);
  const [dialog, setDialog] = useState<Dialog>(null);
  /** The one notice surface: wrap-around, zoom readouts, results, errors. */
  const [toast, setToast] = useState<{ message: string; action?: { label: string; run(): void } } | null>(null);
  const [slideshowTick, setSlideshowTick] = useState(0);
  const [alwaysOnTop, setAlwaysOnTop] = useState(false);

  const zoomButtonRef = useRef<HTMLButtonElement | null>(null);
  const moreButtonRef = useRef<HTMLButtonElement | null>(null);
  const renameInputRef = useRef<HTMLInputElement | null>(null);
  const hideChromeTimer = useRef<number | null>(null);
  const animateTimer = useRef<number | null>(null);
  const toastTimer = useRef<number | null>(null);
  const lastRefresh = useRef(0);

  const animation = useAnimationFrames(image, current?.ext ?? null, framesOpen);
  const isAnimated = Boolean(animation.state);
  // The user's choice to inspect frames sticks for the session: it hides on
  // still pictures and comes back on the next animated one.
  const [framesWanted, setFramesWanted] = useState<boolean | null>(null);
  useEffect(() => {
    if (isAnimated) setFramesOpen(framesWanted ?? preferences.framesAutoOpen);
    else setFramesOpen(false);
  }, [isAnimated, framesWanted, preferences.framesAutoOpen]);
  const toggleFrames = useCallback((next?: boolean) => {
    setFramesOpen((value) => {
      const open = next ?? !value;
      setFramesWanted(open);
      return open;
    });
  }, []);

  const imageSize = useMemo<Size | null>(
    () => (image ? { width: image.width / dpr, height: image.height / dpr } : null),
    [image, dpr]
  );

  const showToast = useCallback((message: string, action?: { label: string; run(): void }, duration?: number) => {
    setToast({ message, action });
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), duration ?? (action ? 6000 : 2400));
  }, []);

  // The next picture is shown only once the current one has appeared, so fast
  // key repeat / wheel can never skip pictures that never loaded. A press made
  // while loading is remembered (one step) and runs as soon as the picture is
  // up; a load that hangs (offline NAS) stops gating after a short grace.
  const imageLoadingRef = useRef(false);
  imageLoadingRef.current = imageLoading;
  const loadStartedAt = useRef(0);
  const pendingStep = useRef<number | null>(null);
  const navigationBlocked = () => imageLoadingRef.current && performance.now() - loadStartedAt.current < 1500;

  /** Step through the folder, telling the user when it wraps around. */
  const navigate = useCallback((delta: number) => {
    const count = state.entries.length;
    if (count < 2) return;
    if (navigationBlocked()) {
      pendingStep.current = delta;
      return;
    }
    pendingStep.current = null;
    const atEnd = state.index === count - 1 && delta > 0;
    const atStart = state.index === 0 && delta < 0;
    if (atEnd || atStart) {
      if (preferences.loop) showToast(atEnd ? "已回到第一张" : "已跳到最后一张");
      else showToast(atEnd ? "已是最后一张" : "已是第一张");
    }
    session.go(delta);
  }, [preferences.loop, session, showToast, state.entries.length, state.index]);

  // Run the remembered step once the picture has appeared.
  useEffect(() => {
    if (imageLoading || pendingStep.current === null) return;
    const delta = pendingStep.current;
    pendingStep.current = null;
    navigate(delta);
  }, [imageLoading, navigate]);
  // ... or once a hung load stops gating.
  useEffect(() => {
    if (!imageLoading) return;
    const timer = window.setTimeout(() => {
      if (pendingStep.current === null) return;
      const delta = pendingStep.current;
      pendingStep.current = null;
      navigate(delta);
    }, 1550);
    return () => window.clearTimeout(timer);
  }, [imageLoading, navigate]);

  const comic = preferences.comicMode && state.entries.length > 0;
  const comicRef = useRef(comic);
  comicRef.current = comic;
  /** Comic mode jumps by scrolling the strip; single-picture mode switches. */
  const goToIndex = useCallback((target: number) => {
    if (preferences.comicMode) {
      comicJumpToken.current += 1;
      setComicJump({ index: target, token: comicJumpToken.current });
    }
    session.goTo(target);
  }, [preferences.comicMode, session]);
  const toggleComic = useCallback(() => {
    const next = !preferences.comicMode;
    setPreferences({ comicMode: next });
    if (next) {
      comicJumpToken.current += 1;
      setComicJump({ index: state.index, token: comicJumpToken.current });
    }
  }, [preferences.comicMode, setPreferences, state.index]);

  /** Short readouts (zoom level, rotation) use the same notice, briefly. */
  const flashHud = useCallback((message: string) => showToast(message, undefined, 1100), [showToast]);

  // ------------------------------------------------------------- bootstrap

  useEffect(() => {
    createCoreClient()
      .getImageViewerFormats()
      .then((formats) => {
        // APNGs often keep the .png extension: sniff those too (header check in imageSource).
        setAnimatedExtensions([...new Set([...formats.animatedExtensions, ".png"])]);
        setGlobalError(null);
      }, (error) => setGlobalError(describeError(error)));
  }, []);

  useEffect(() => {
    if (!bridge) return;
    let lastId = 0;
    const accept = (request: { id: number; paths: string[] } | null) => {
      if (!request || request.id <= lastId) return;
      lastId = request.id;
      void session.open(request.paths);
    };
    const unsubscribe = bridge.onOpenRequest(accept);
    void bridge.ready().then(accept);
    const unsubscribeFullscreen = bridge.onFullscreenChanged(setFullscreen);
    return () => {
      unsubscribe();
      unsubscribeFullscreen();
    };
    // session.open is stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [bridge]);

  useEffect(() => {
    const onResize = () => setDpr(window.devicePixelRatio || 1);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    document.title = current ? `${current.name} - Mangoo 看图` : "Mangoo 看图";
  }, [current]);

  // Pick up files added or removed outside while the window was in the background.
  useEffect(() => {
    const onFocus = () => {
      const now = Date.now();
      if (now - lastRefresh.current < 2000) return;
      lastRefresh.current = now;
      void session.refresh();
    };
    window.addEventListener("focus", onFocus);
    // Files copied into the folder while the viewer stays in front show up
    // too: a cheap non-recursive re-list every few seconds while visible.
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void session.refresh();
    }, 4000);
    return () => {
      window.removeEventListener("focus", onFocus);
      window.clearInterval(timer);
    };
  }, [session.refresh]);

  // ---------------------------------------------------------- image loading

  useEffect(() => {
    if (!current) {
      setImage(null);
      setImageError(null);
      setImageLoading(false);
      return;
    }
    let cancelled = false;
    let standInTimer = 0;
    setImageError(null);
    const source = effectiveRawSource;
    const cached = cache.peek(ViewerImageCache.key(current, false, source)) ?? cache.peek(ViewerImageCache.key(current, true, source));
    if (cached) {
      setImage(cached);
      setImageLoading(false);
      setStandIn(null);
    } else {
      // Stamped per picture: a hung picture's grace never carries over.
      loadStartedAt.current = performance.now();
      setImageLoading(true);
      // Fast browsing (scrub bar, held arrow key): show THIS picture's
      // thumbnail instead of the previous picture until it is decoded. Only
      // for a DIFFERENT picture (a RAW/JPEG switch keeps the sharp one), not
      // in comic mode, and after a short grace so a quick load never flashes
      // the low-res stand-in.
      const switching = (image?.key.split("\u0000")[0] ?? null) !== current.path;
      setStandIn(null);
      if (switching && !comicRef.current) {
        const ready = readyThumbnailUrl(current, 160);
        const urgentThumb = ready ? Promise.resolve(ready) : thumbnailUrl(current, 160, true);
        standInTimer = window.setTimeout(() => {
          void urgentThumb.then((url) => {
            if (!cancelled) setStandIn(url);
          }, () => undefined);
        }, ready ? 50 : 0);
      }
    }
    const viewportLongPx = viewerViewportLongPx();
    let visibleLoad: Promise<LoadedImage> | null = null;
    if (!cached) {
      // The picture on screen first: filmstrip thumbnails wait until it is up.
      holdThumbnails(true);
      visibleLoad = cache
        .load(current, { viewportLongPx, full: false, animatedExtensions, rawSource: source });
      visibleLoad
        .then((loaded) => {
          if (cancelled) return;
          setImage(loaded);
          setImageLoading(false);
          setStandIn(null);
          holdThumbnails(false);
        }, (error) => {
          if (cancelled) return;
          setImage(null);
          setImageLoading(false);
          setStandIn(null);
          setImageError(describeError(error));
          holdThumbnails(false);
        });
    } else {
      holdThumbnails(false);
    }
    cache.pin([ViewerImageCache.key(current, false, source), ViewerImageCache.key(current, true, source)]);
    // Warm the neighbours only once the visible picture is painted: they
    // would otherwise share Core's decode slots with it.
    let prefetch = 0;
    const warmNeighbours = () => {
      if (cancelled) return;
      prefetch = window.setTimeout(() => {
        for (const index of prefetchOrder(state.entries.length, state.index, 2, preferences.loop)) {
          const entry = state.entries[index];
          if (entry) void cache.load(entry, { viewportLongPx, full: false, animatedExtensions, priority: "resource", rawSource: entry.cameraRaw ? rawSource : "developed" }).catch(() => undefined);
        }
      }, 120);
    };
    if (cached) warmNeighbours();
    else visibleLoad?.then(warmNeighbours, warmNeighbours);
    return () => {
      cancelled = true;
      window.clearTimeout(standInTimer);
      window.clearTimeout(prefetch);
      // A navigation away never leaves the strip blocked.
      holdThumbnails(false);
    };
    // Entries are read for prefetch only; navigation is keyed by `current`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, cache, animatedExtensions, effectiveRawSource]);

  // Reset the view for a new picture (unless the user asked to keep it).
  const imageKey = image?.key.split("\u0000")[0] ?? null;
  useLayoutEffect(() => {
    if (!imageKey) return;
    setAnimate("none");
    if (!preferences.keepTransform) {
      setZoomState(preferences.fitMode);
      setTransform((value) => ({ ...identityTransform(), scale: value.scale }));
    } else {
      // Keep zoom, rotation and flips; recentre so the next picture is in view.
      setTransform((value) => ({ ...value, panX: 0, panY: 0 }));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [imageKey]);

  // Fit modes follow the viewport, rotation and picture.
  useEffect(() => {
    if (!imageSize || zoomState === "custom" || viewport.width === 0) return;
    setTransform((value) => ({
      ...value,
      scale: fitScale(imageSize, viewport, value.rotation, zoomState, preferences.upscaleSmall),
      panX: 0,
      panY: 0
    }));
  }, [imageSize, viewport, zoomState, preferences.upscaleSmall, transform.rotation]);

  // Sharpen: zoomed past what the adaptive tier holds -> fetch the original.
  useEffect(() => {
    if (!current || !image || image.kind !== "bitmap" || !image.upgradable) return;
    // The scale the picture is (or is about to be) shown at. In a fit mode
    // use the fit's own target, not transform.scale: on the first frame that
    // still holds the previous picture's scale, which fetched — for a camera
    // RAW, developed — the full-size original for nothing. "actual" (100%) or
    // "fill" on a large picture still upgrades as it should.
    const scale = zoomState === "custom" || !imageSize || viewport.width === 0
      ? transform.scale
      : fitScale(imageSize, viewport, transform.rotation, zoomState, preferences.upscaleSmall);
    const needed = scale * image.width;
    if (needed <= image.bitmap.width * 1.1) return;
    let cancelled = false;
    void cache.load(current, { viewportLongPx: viewerViewportLongPx(), full: true, animatedExtensions, rawSource: effectiveRawSource }).then((full) => {
      if (!cancelled) setImage(full);
    }, () => undefined);
    return () => {
      cancelled = true;
    };
  }, [current, image, transform.scale, transform.rotation, zoomState, imageSize, viewport, preferences.upscaleSmall, cache, animatedExtensions, effectiveRawSource]);

  // ------------------------------------------------------------- transforms

  // Every zoom is interpolated: the image layer's CSS transition retargets
  // smoothly when a new wheel notch arrives mid-flight.
  const withAnimation = useCallback((kind: Exclude<StageAnimation, "none"> = "ease") => {
    setAnimate(kind);
    if (animateTimer.current) window.clearTimeout(animateTimer.current);
    animateTimer.current = window.setTimeout(() => setAnimate("none"), ANIMATION_MS[kind] + 30);
  }, []);
  const stopAnimation = useCallback(() => {
    if (animateTimer.current) window.clearTimeout(animateTimer.current);
    setAnimate("none");
  }, []);
  const transformRef = useRef(transform);
  transformRef.current = transform;

  const center = useMemo(() => ({ x: viewport.width / 2, y: viewport.height / 2 }), [viewport]);

  const zoomTo = useCallback((scale: number, anchor = center, animated = true) => {
    if (!imageSize) return;
    if (animated) withAnimation();
    else stopAnimation();
    setZoomState("custom");
    setTransform((value) => zoomAt(value, scale, anchor, imageSize, viewport));
    flashHud(formatZoom(Math.min(32, Math.max(0.02, scale))));
  }, [center, flashHud, imageSize, stopAnimation, viewport, withAnimation]);

  /**
   * Relative zoom (wheel, trackpad pinch, touch pinch). Wheel notches and
   * trackpad pinches glide to the new scale; a touch pinch follows the
   * fingers 1:1. Successive events build on the latest target, never a
   * stale render.
   */
  const zoomBy = useCallback((factor: number, anchor: { x: number; y: number }, kind: "wheel" | "pinch" | "touch" = "wheel") => {
    if (!imageSize) return;
    if (kind === "touch") stopAnimation();
    else withAnimation(kind);
    setZoomState("custom");
    const next = zoomAt(transformRef.current, transformRef.current.scale * factor, anchor, imageSize, viewport);
    transformRef.current = next;
    setTransform(next);
    flashHud(formatZoom(next.scale));
  }, [flashHud, imageSize, stopAnimation, viewport, withAnimation]);

  const zoomStep = useCallback((direction: 1 | -1) => {
    zoomTo(nextZoomStop(transform.scale, direction));
  }, [transform.scale, zoomTo]);

  const setFit = useCallback((mode: FitMode) => {
    withAnimation();
    setZoomState(mode);
    flashHud(mode === "fill" ? "填满窗口" : mode === "actual" ? "100%" : "适应窗口");
  }, [flashHud, withAnimation]);

  const toggleActualSize = useCallback((anchor = center) => {
    if (Math.abs(transform.scale - 1) < 0.01) setFit(preferences.fitMode === "actual" ? "fit" : preferences.fitMode);
    else zoomTo(1, anchor);
  }, [center, preferences.fitMode, setFit, transform.scale, zoomTo]);

  const rotateBy = useCallback((turns: 1 | -1) => {
    withAnimation();
    setTransform((value) => {
      const next = rotate(value, turns);
      flashHud(`旋转 ${next.rotation}°`);
      return next;
    });
  }, [flashHud, withAnimation]);

  const flip = useCallback((axis: "x" | "y") => {
    withAnimation();
    setTransform((value) => (axis === "x" ? { ...value, flipX: !value.flipX } : { ...value, flipY: !value.flipY }));
  }, [withAnimation]);

  const pan = useCallback((deltaX: number, deltaY: number, bounded = false) => {
    if (!imageSize) return;
    stopAnimation();
    const current = transformRef.current;
    const moved = { ...current, panX: current.panX + deltaX, panY: current.panY + deltaY };
    const next = bounded ? softClampPan(current, moved, imageSize, viewport) : moved;
    if (next.panX === current.panX && next.panY === current.panY) return;
    // A hand-placed picture keeps its place: window resizes no longer refit it.
    if (!bounded) setZoomState("custom");
    transformRef.current = next;
    setTransform(next);
  }, [imageSize, stopAnimation, viewport]);

  // -------------------------------------------------------------- actions

  const openFiles = useCallback(async () => {
    if (!bridge) return;
    const paths = await bridge.pickFiles();
    if (paths?.length) void session.open(paths);
  }, [bridge, session]);

  const openFolder = useCallback(async () => {
    if (!bridge) return;
    const folder = await bridge.pickFolder();
    if (folder) void session.open([folder]);
  }, [bridge, session]);

  const dropFiles = useCallback(async (files: File[]) => {
    if (!bridge) return;
    const paths = files.map((file) => bridge.pathForFile(file)).filter((value): value is string => Boolean(value));
    if (paths.length === 0) return;
    const admitted = await bridge.admitPaths(paths);
    if (admitted.length) void session.open(admitted);
    else showToast("无法打开拖入的文件");
  }, [bridge, session, showToast]);

  const toggleFullscreen = useCallback(async () => {
    if (!bridge) return;
    // The host answers with the settled state; the window events stay
    // authoritative for every other change.
    setFullscreen(await bridge.toggleFullscreen());
  }, [bridge]);

  const copyImage = useCallback(async () => {
    if (!bridge || !image) return;
    try {
      const result = await bridge.copyImage(await encodePng(image));
      showToast(result.ok ? "已复制图片" : result.error ?? "复制失败");
    } catch (error) {
      showToast(describeError(error));
    }
  }, [bridge, image, showToast]);

  const copyPath = useCallback(async () => {
    if (!bridge || !current) return;
    const result = await bridge.copyText(current.path);
    showToast(result.ok ? "已复制文件路径" : result.error ?? "复制失败");
  }, [bridge, current, showToast]);

  const reveal = useCallback(async () => {
    if (!bridge || !current) return;
    const result = await bridge.reveal(current.path);
    if (!result.ok) showToast(friendlyActionError(result.error, "无法在文件夹中显示"));
  }, [bridge, current, showToast]);

  const openExternally = useCallback(async () => {
    if (!bridge || !current) return;
    const result = await bridge.openExternally(current.path);
    if (!result.ok) showToast(friendlyActionError(result.error, "无法用其他应用打开"));
  }, [bridge, current, showToast]);

  /** A RAW+JPEG pair shown as one picture: its hidden half. */
  const pairedPath = hiddenPairOf(current, state.entries);

  const performDelete = useCallback(async (dontAskAgain = false) => {
    if (!bridge || !current) return;
    const target = current;
    // A picture shown as one is deleted as one: both halves of a RAW+JPEG
    // pair go to the recycle bin (otherwise the other half pops back).
    const companion = hiddenPairOf(target, state.entries);
    const result = await bridge.trash(target.path);
    setDialog(null);
    if (dontAskAgain) setPreferences({ confirmDelete: false });
    if (!result.ok) {
      showToast(friendlyActionError(result.error, "无法删除"));
      return;
    }
    cache.forget(target.path);
    const removed = [target.path];
    let companionFailed = false;
    if (companion) {
      const second = await bridge.trash(companion);
      if (second.ok) {
        cache.forget(companion);
        removed.push(companion);
      } else companionFailed = true;
    }
    session.removePaths(removed);
    showToast(companion && !companionFailed
      ? `“${target.name}”及同名 JPEG 已移到回收站，可在回收站中恢复`
      : companionFailed
        ? `“${target.name}”已移到回收站，但同名 JPEG 无法删除`
        : `“${target.name}”已移到回收站，可在回收站中恢复`);
  }, [bridge, cache, current, session, setPreferences, showToast, state.entries]);

  const requestDelete = useCallback(() => {
    if (!current) return;
    if (preferences.confirmDelete) setDialog({ kind: "delete", dontAskAgain: false });
    else void performDelete();
  }, [current, performDelete, preferences.confirmDelete]);

  const requestRename = useCallback(() => {
    if (!current) return;
    setDialog({ kind: "rename", value: current.name, error: null, busy: false });
  }, [current]);

  const performRename = useCallback(async () => {
    if (!bridge || !current || dialog?.kind !== "rename" || dialog.busy) return;
    const value = dialog.value.trim();
    if (value === current.name) {
      setDialog(null);
      return;
    }
    const problem = validateFileName(value);
    if (problem) {
      setDialog({ ...dialog, error: problem });
      return;
    }
    // Typing only the base name keeps the original extension, like Explorer.
    const finalName = extensionOf(value) === current.ext ? value : `${value}${current.ext}`;
    setDialog({ ...dialog, busy: true, error: null });
    const result = await bridge.rename(current.path, finalName);
    if (!result.ok || !result.path) {
      setDialog({ ...dialog, busy: false, error: result.error ?? "重命名失败" });
      return;
    }
    cache.forget(current.path);
    const renames = [{ from: current.path, to: result.path }];
    // Keep a RAW+JPEG pair together: rename the hidden half to the same stem.
    const companion = hiddenPairOf(current, state.entries);
    let companionFailed = false;
    if (companion) {
      const stem = finalName.slice(0, finalName.length - current.ext.length);
      const companionName = `${stem}${extensionOf(companion) ? companion.slice(companion.length - extensionOf(companion).length) : ""}`;
      const second = await bridge.rename(companion, companionName);
      if (second.ok && second.path) {
        cache.forget(companion);
        renames.push({ from: companion, to: second.path });
      } else companionFailed = true;
    }
    session.renamePaths(renames);
    setDialog(null);
    showToast(companionFailed ? "已重命名，但同名 JPEG 未能一起重命名" : companion ? "已重命名（含同名 JPEG）" : "已重命名");
  }, [bridge, cache, current, dialog, session, showToast, state.entries]);

  const renameOpen = dialog?.kind === "rename";
  useEffect(() => {
    if (!renameOpen) return;
    // The modal's focus trap focuses the field after mount; select afterwards.
    const timer = window.setTimeout(() => {
      const input = renameInputRef.current;
      if (!input) return;
      input.focus();
      const dot = input.value.lastIndexOf(".");
      input.setSelectionRange(0, dot > 0 ? dot : input.value.length);
    }, 60);
    return () => window.clearTimeout(timer);
  }, [renameOpen]);

  const openSettings = useCallback((section: "display" | "shortcuts" = "display") => {
    if (bridge) void bridge.openSettings(section);
  }, [bridge]);

  const showChannel = useCallback((next: ViewerChannel | null) => {
    setChannel((value) => {
      const resolved = value === next ? null : next;
      showToast(resolved ? `${{ red: "红色", green: "绿色", blue: "蓝色", alpha: "透明" }[resolved]}通道（${resolved[0].toUpperCase()}）` : "全部通道（RGBA）", undefined, 1100);
      return resolved;
    });
  }, [showToast]);

  const toggleRawSource = useCallback(() => {
    if (!isRaw) return;
    setRawSource((value) => {
      const next = value === "developed" ? "embedded" : "developed";
      showToast(next === "embedded" ? "RAW：显示内嵌 JPEG 预览" : "RAW：显示 RAW 原图", undefined, 1300);
      return next;
    });
  }, [isRaw, showToast]);

  const toggleAlwaysOnTop = useCallback(async () => {
    if (bridge) setAlwaysOnTop(await bridge.setAlwaysOnTop(!alwaysOnTop));
  }, [alwaysOnTop, bridge]);

  // -------------------------------------------------------- slideshow, chrome

  // Each picture stays up for the full interval counted from when it appeared
  // (a slow picture never eats into its own time).
  useEffect(() => {
    if (!slideshow || imageLoading) return;
    setSlideshowTick((value) => value + 1);
    const timer = window.setTimeout(() => {
      if (!preferences.loop && state.index >= state.entries.length - 1) {
        setSlideshow(false);
        return;
      }
      navigate(1);
    }, preferences.slideshowSeconds * 1000);
    return () => window.clearTimeout(timer);
    // navigate is re-created per index; the picture (state.index) drives this.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slideshow, imageLoading, preferences.slideshowSeconds, preferences.loop, state.index, state.entries.length]);

  const immersive = fullscreen || slideshow;
  // Cursor auto-hide in fullscreen / slideshow only.
  const noteActivity = useCallback(() => {
    setChromeVisible(true);
    if (hideChromeTimer.current) window.clearTimeout(hideChromeTimer.current);
    if (!immersive) return;
    hideChromeTimer.current = window.setTimeout(() => setChromeVisible(false), 2200);
  }, [immersive]);

  useEffect(() => {
    noteActivity();
  }, [immersive, noteActivity]);

  // ------------------------------------------------------------- keyboard

  const overlaysOpen = Boolean(dialog || zoomMenuOpen || contextMenu);
  // Titlebar, toolbar and edge arrows share one reveal model: shown while the
  // pointer is near them, hidden after a short grace delay once it leaves.
  const chrome = useProximityChrome({
    reach: { top: 28, bottom: 72, left: 96, right: 96 },
    anchors: {
      top: () => document.querySelector(".iv-titlebar"),
      bottom: () => document.querySelector(".iv-toolbar"),
      left: () => document.querySelector(".iv-nav-prev"),
      right: () => document.querySelector(".iv-nav-next")
    },
    hysteresis: 28,
    hideDelay: 450,
    pinned: { top: preferences.titlebarPinned && !immersive },
    hold: overlaysOpen,
    blockers: { top: ".iv-float", bottom: ".iv-float" },
    // Revealed on a maximized window, the titlebar is a native drag region.
    nativeRegion: { top: maximized && !fullscreen }
  });
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (overlaysOpen || isEditableTarget(event.target)) return;
      // A focused button activates on Space/Enter; do not also navigate.
      if ((event.key === " " || event.key === "Enter") && (event.target as HTMLElement | null)?.closest?.("button")) return;
      // Up/Down pan a zoomed-in picture and otherwise navigate, like most viewers.
      const zoomedIn = Boolean(imageSize) && transform.scale > fitScale(imageSize ?? { width: 1, height: 1 }, viewport, transform.rotation, "fit", preferences.upscaleSmall) + 1e-3;
      const key = event.key;
      let handled = true;
      const comicKey = comic && !event.altKey && !event.metaKey;
      if (comicKey && (key === "ArrowLeft" || key === "ArrowRight" || ((key === "PageUp" || key === "PageDown") && event.shiftKey))) {
        // ← → / Shift+PgUp PgDn: previous / next picture's top.
        const delta = key === "ArrowLeft" || key === "PageUp" ? -1 : 1;
        goToIndex(Math.min(state.entries.length - 1, Math.max(0, state.index + delta)));
      } else if (comicKey && (key === "PageDown" || key === " ") && !event.ctrlKey) {
        comicScrollBy(document.querySelector(".iv-comic"), event.shiftKey ? -0.9 : 0.9);
      } else if (comicKey && key === "PageUp" && !event.ctrlKey) {
        comicScrollBy(document.querySelector(".iv-comic"), -0.9);
      } else if (comicKey && (key === "ArrowDown" || key === "ArrowUp") && !event.ctrlKey && !event.shiftKey) {
        comicScrollBy(document.querySelector(".iv-comic"), key === "ArrowDown" ? 0.25 : -0.25);
      } else if (comicKey && (key === "Home" || key === "End") && !event.ctrlKey) {
        goToIndex(key === "Home" ? 0 : state.entries.length - 1);
      } else if (comicKey && (key === "=" || key === "+" || key === "-" || key === "0") && !event.ctrlKey) {
        const next = key === "0" ? 1 : Math.min(3, Math.max(0.2, preferences.comicZoom * (key === "-" ? 1 / 1.15 : 1.15)));
        setPreferences({ comicZoom: next });
        flashHud(`宽度 ${Math.round(next * 100)}%`);
      } else if (key === "Escape" && comic && !fullscreen && !slideshow) {
        toggleComic();
      } else if (key === "Escape") {
        if (slideshow) setSlideshow(false);
        else if (fullscreen) void toggleFullscreen();
        else if (framesOpen) toggleFrames(false);
        else if (preferences.escapeCloses) window.close();
        else handled = false;
      } else if (key === "ArrowDown" && zoomedIn) pan(0, -60, true);
      else if (key === "ArrowUp" && zoomedIn) pan(0, 60, true);
      else if (key === "ArrowDown" && !event.ctrlKey && !event.shiftKey && !event.altKey) navigate(1);
      else if (key === "ArrowUp" && !event.ctrlKey && !event.shiftKey && !event.altKey) navigate(-1);
      else {
        const action = matchAction(bindings, event);
        // Space plays/pauses an animation; elsewhere it advances like most viewers.
        const resolved: ShortcutActionId | "advance" | null = action === "playPause" && !isAnimated ? "advance" : action;
        switch (resolved) {
          case "next": case "advance": navigate(1); break;
          case "previous": navigate(-1); break;
          case "first": session.goTo(0); break;
          case "last": session.goTo(state.entries.length - 1); break;
          case "zoomIn": zoomStep(1); break;
          case "zoomOut": zoomStep(-1); break;
          case "fit": setFit("fit"); break;
          case "actualSize": zoomTo(1); break;
          case "rotateRight": rotateBy(1); break;
          case "rotateLeft": rotateBy(-1); break;
          case "flipHorizontal": flip("x"); break;
          case "flipVertical": flip("y"); break;
          case "fullscreen": void toggleFullscreen(); break;
          case "slideshow": setSlideshow((value) => !value); break;
          case "toggleInfo": setPreferences({ showInfo: !preferences.showInfo }); break;
          case "toggleFilmstrip": setPreferences({ showFilmstrip: !preferences.showFilmstrip }); break;
          case "toggleComic": toggleComic(); break;
          case "toggleTitlebarPin": setPreferences({ titlebarPinned: !preferences.titlebarPinned }); break;
          case "playPause":
            // First press freezes native playback on the frame being shown.
            if (!framesOpen) toggleFrames(true);
            else animation.playPause();
            break;
          case "nextFrame":
            if (isAnimated) { if (!framesOpen) toggleFrames(true); animation.step(1); } else handled = false;
            break;
          case "previousFrame":
            if (isAnimated) { if (!framesOpen) toggleFrames(true); animation.step(-1); } else handled = false;
            break;
          case "open": void openFiles(); break;
          case "copyImage": void copyImage(); break;
          case "copyPath": void copyPath(); break;
          case "rename": requestRename(); break;
          case "delete": requestDelete(); break;
          case "showShortcuts": openSettings("shortcuts"); break;
          case "channelAll": setChannel(null); showToast("全部通道（RGBA）", undefined, 1100); break;
          case "channelRed": showChannel("red"); break;
          case "channelGreen": showChannel("green"); break;
          case "channelBlue": showChannel("blue"); break;
          case "channelAlpha": showChannel("alpha"); break;
          case "toggleRawSource": if (isRaw) toggleRawSource(); else handled = false; break;
          case "closeWindow": window.close(); break;
          default: handled = false;
        }
      }
      if (handled) {
        event.preventDefault();
        noteActivity();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    animation, bindings, framesOpen, isAnimated, toggleFrames,
    copyImage, copyPath, flip, fullscreen, imageSize, noteActivity, openFiles, openSettings, overlaysOpen, pan, preferences,
    isRaw, showChannel, showToast, toggleRawSource, comic, goToIndex, toggleComic, flashHud,
    requestDelete, requestRename, rotateBy, session, setFit, setPreferences, slideshow, state.entries.length,
    toggleFullscreen, transform.rotation, transform.scale, viewport, zoomStep, zoomTo
  ]);

  // Only one overlay at a time: opening a dialog closes menus.
  const openDialog = useCallback((next: Dialog) => {
    setContextMenu(null);
    setZoomMenuOpen(false);
    setDialog(next);
  }, []);

  // ---------------------------------------------------------------- render

  const hasImage = Boolean(current);
  const counter = state.entries.length ? `${state.index + 1} / ${state.entries.length}` : "";
  const dims = image ? `${image.width} × ${image.height}` : null;
  // On-screen pixels per source pixel decides magnify vs minify.
  const paintedWidth = image ? (image.kind === "bitmap" ? image.bitmap.width : image.width) : 1;
  // Device pixels per painted pixel (an adaptive tier is smaller than the
  // source, so it is magnified sooner than the zoom readout suggests).
  const displayRatio = image ? (transform.scale * dpr * image.width) / Math.max(1, paintedWidth) : 1;
  const interpolation = displayRatio >= 2 ? preferences.upscaleInterpolation
    : displayRatio <= 1 ? preferences.downscaleInterpolation
    : "smooth";
  const canvasLevel = image?.kind === "bitmap" ? renderLevel((transform.scale * image.width) / Math.max(1, image.bitmap.width)) : 1;
  const zoomLabel = !image ? "—" : zoomState === "fit" ? `适应 · ${formatZoom(transform.scale)}` : zoomState === "fill" ? `填满 · ${formatZoom(transform.scale)}` : formatZoom(transform.scale);
  const fileSize = current && current.sizeBytes > 0 ? formatBytes(current.sizeBytes) : null;

  const stageOverlay = (
    <>
      {standIn && imageLoading && !comic ? (
        <div aria-hidden="true" className="iv-stage-standin">
          <img alt="" draggable={false} src={standIn} />
        </div>
      ) : null}
      {imageLoading ? (
        <div className="iv-stage-status" data-delay="true">
          <MediaLoadingRing size={22} />
        </div>
      ) : null}
      {imageError && current ? (
        <div className="iv-stage-message" role="status">
          <icons.AlertTriangle aria-hidden="true" size={22} />
          <strong>{current.name}</strong>
          <span>{imageError}</span>
          <div className="iv-empty-actions">
            <LiquidGlassButton className="no-drag" disabled={!bridge} onClick={() => void openExternally()} tone="control" type="button">
              <icons.ExternalLink aria-hidden="true" size={14} />
              <span>用其他应用打开</span>
            </LiquidGlassButton>
            <LiquidGlassButton className="no-drag" disabled={!bridge} onClick={() => void reveal()} tone="control" type="button">
              <icons.FolderOpen aria-hidden="true" size={14} />
              <span>在文件夹中显示</span>
            </LiquidGlassButton>
          </div>
          {state.entries.length > 1 ? <span className="iv-stage-hint">按 ← → 继续浏览其他图片</span> : null}
        </div>
      ) : null}
      {!hasImage && !state.loading ? (
        <div className="iv-empty">
          <icons.Images aria-hidden="true" size={42} weight="light" />
          <strong>{state.error ?? globalError ?? (state.entries.length === 0 && state.directory ? "这个文件夹里没有可以查看的图片" : "打开图片，或把图片拖到这里")}</strong>
          <span>支持 JPEG、PNG、WebP、AVIF、HEIC、TIFF、PSD 和相机 RAW 等格式。打开一张图片后，可以浏览同一文件夹里的其他图片。</span>
          <div className="iv-empty-actions">
            <LiquidGlassButton className="no-drag" disabled={!bridge} onClick={() => void openFiles()} tone="primary" type="button">
              <icons.FolderOpen aria-hidden="true" size={14} />
              <span>打开图片…</span>
            </LiquidGlassButton>
            <LiquidGlassButton className="no-drag" disabled={!bridge} onClick={() => void openFolder()} tone="control" type="button">
              <icons.Folder aria-hidden="true" size={14} />
              <span>打开文件夹…</span>
            </LiquidGlassButton>
          </div>
        </div>
      ) : null}
      {state.loading ? (
        <div className="iv-stage-status">
          <MediaLoadingRing size={22} />
        </div>
      ) : null}
      {slideshow ? (
        <div className="iv-slideshow-progress" aria-hidden="true">
          <span key={slideshowTick} style={{ animationDuration: `${preferences.slideshowSeconds}s` }} />
        </div>
      ) : null}
    </>
  );

  type MenuItem = { id: string; label?: string; icon?: ReactNode; shortcut?: string; disabled?: boolean; danger?: boolean; checked?: boolean; onSelect?: () => void; separator?: boolean; submenu?: MenuItem[] };
  const channelItems: MenuItem[] = [
    { id: "ch-all", label: "全部通道（RGBA）", icon: <icons.Layers size={14} />, shortcut: keyOf("channelAll"), checked: channel === null, onSelect: () => setChannel(null) },
    { id: "ch-r", label: "红色通道（R）", icon: <span className="iv-menu-swatch" data-channel="red" />, shortcut: keyOf("channelRed"), checked: channel === "red", onSelect: () => showChannel("red") },
    { id: "ch-g", label: "绿色通道（G）", icon: <span className="iv-menu-swatch" data-channel="green" />, shortcut: keyOf("channelGreen"), checked: channel === "green", onSelect: () => showChannel("green") },
    { id: "ch-b", label: "蓝色通道（B）", icon: <span className="iv-menu-swatch" data-channel="blue" />, shortcut: keyOf("channelBlue"), checked: channel === "blue", onSelect: () => showChannel("blue") },
    { id: "ch-a", label: "透明通道（A）", icon: <span className="iv-menu-swatch" data-channel="alpha" />, shortcut: keyOf("channelAlpha"), checked: channel === "alpha", onSelect: () => showChannel("alpha") },
    { id: "ch-s", separator: true },
    { id: "ch-pm", label: "预乘 Alpha", icon: <icons.Layers3 size={14} />, checked: preferences.channelPremultiply, onSelect: () => setPreferences({ channelPremultiply: !preferences.channelPremultiply }) }
  ];
  const menuItems: MenuItem[] = [
    { id: "copy", label: "复制图片", icon: <icons.Copy size={14} />, shortcut: keyOf("copyImage"), disabled: !image, onSelect: () => void copyImage() },
    { id: "copy-path", label: "复制文件路径", icon: <icons.Clipboard size={14} />, shortcut: keyOf("copyPath"), disabled: !hasImage, onSelect: () => void copyPath() },
    { id: "s1", separator: true },
    { id: "rotate-right", label: "向右旋转", icon: <icons.RotateCw size={14} />, shortcut: keyOf("rotateRight"), disabled: !hasImage, onSelect: () => rotateBy(1) },
    { id: "rotate-left", label: "向左旋转", icon: <icons.RotateCcw size={14} />, shortcut: keyOf("rotateLeft"), disabled: !hasImage, onSelect: () => rotateBy(-1) },
    { id: "flip-x", label: "水平翻转", icon: <icons.FlipHorizontal size={14} />, shortcut: keyOf("flipHorizontal"), disabled: !hasImage, checked: transform.flipX, onSelect: () => flip("x") },
    { id: "flip-y", label: "垂直翻转", icon: <icons.FlipVertical size={14} />, shortcut: keyOf("flipVertical"), disabled: !hasImage, checked: transform.flipY, onSelect: () => flip("y") },
    ...(isAnimated ? [{ id: "frames", label: "逐帧查看动图", icon: <icons.FilmReel size={14} />, shortcut: keyOf("nextFrame"), checked: framesOpen, onSelect: () => toggleFrames() }] : []),
    { id: "comic", label: "漫画（条漫）模式", icon: <icons.ScrollText size={14} />, shortcut: keyOf("toggleComic"), checked: comic, disabled: state.entries.length < 1, onSelect: toggleComic },
    { id: "info", label: "图片信息", icon: <icons.Info size={14} />, shortcut: keyOf("toggleInfo"), checked: preferences.showInfo, onSelect: () => setPreferences({ showInfo: !preferences.showInfo }) },
    ...(isRaw ? [{ id: "raw-source", label: effectiveRawSource === "embedded" ? "显示 RAW 原图" : "显示内嵌 JPEG 预览", icon: <icons.Camera size={14} />, shortcut: keyOf("toggleRawSource"), onSelect: toggleRawSource }] : []),
    { id: "channels", label: channel ? `通道：${{ red: "R", green: "G", blue: "B", alpha: "A" }[channel]}` : "查看通道", icon: <icons.Layers size={14} />, disabled: !image, submenu: channelItems },
    { id: "s2", separator: true },
    { id: "reveal", label: "在文件夹中显示", icon: <icons.FolderOpen size={14} />, disabled: !hasImage, onSelect: () => void reveal() },
    { id: "open-external", label: "用其他应用打开", icon: <icons.ExternalLink size={14} />, disabled: !hasImage, onSelect: () => void openExternally() },
    { id: "rename", label: "重命名…", icon: <icons.Pencil size={14} />, shortcut: keyOf("rename"), disabled: !hasImage, onSelect: requestRename },
    { id: "delete", label: "删除到回收站", icon: <icons.Trash2 size={14} />, shortcut: keyOf("delete"), danger: true, disabled: !hasImage, onSelect: requestDelete },
    { id: "s3", separator: true },
    { id: "slideshow", label: slideshow ? "停止幻灯片" : "幻灯片放映", icon: slideshow ? <icons.Pause size={14} /> : <icons.Slideshow size={14} />, shortcut: keyOf("slideshow"), disabled: state.entries.length < 2, onSelect: () => setSlideshow((value) => !value) },
    { id: "fullscreen", label: fullscreen ? "退出全屏" : "全屏", icon: fullscreen ? <icons.ExitFullscreen size={14} /> : <icons.Fullscreen size={14} />, shortcut: keyOf("fullscreen"), onSelect: () => void toggleFullscreen() }
  ];

  /** Titlebar "more" menu: window-level things that are not about the picture. */
  const moreItems: MenuItem[] = [
    { id: "open", label: "打开图片…", icon: <icons.FolderOpen size={14} />, shortcut: keyOf("open"), onSelect: () => void openFiles() },
    { id: "open-folder", label: "打开文件夹…", icon: <icons.Folder size={14} />, onSelect: () => void openFolder() },
    { id: "s1", separator: true },
    { id: "on-top", label: "窗口置顶", icon: <icons.StackTop size={14} />, checked: alwaysOnTop, onSelect: () => void toggleAlwaysOnTop() },
    { id: "main", label: "打开 Mangoo 资料库", icon: <icons.Library size={14} />, disabled: !bridge, onSelect: () => void bridge?.openMainWindow() },
    { id: "s2", separator: true },
    { id: "settings", label: "看图设置…", icon: <icons.Settings size={14} />, disabled: !bridge, onSelect: () => openSettings("display") }
  ];
  const [menuSource, setMenuSource] = useState<"stage" | "more">("stage");
  const [openSubmenu, setOpenSubmenu] = useState<{ item: MenuItem; anchor: HTMLElement } | null>(null);
  useEffect(() => {
    if (!contextMenu) setOpenSubmenu(null);
  }, [contextMenu]);
  const windowControls = getWindowControls();
  /** The OS window covers the whole screen (fullscreen, whatever React thinks). */
  const windowFillsScreen = () => window.innerWidth >= window.screen.width && window.innerHeight >= window.screen.height;
  const windowDrag = useRef<{ pointerId: number; x: number; y: number; started: boolean } | null>(null);
  const beginWindowDrag = (event: React.PointerEvent<HTMLElement>) => {
    if (event.button !== 0 || !windowControls) return;
    // A pinned titlebar outside fullscreen, and a revealed one on a
    // maximized window, are native OS drag regions.
    if (preferences.titlebarPinned && !fullscreen && !windowFillsScreen()) return;
    if (maximized && !fullscreen) return;
    if ((event.target as HTMLElement).closest("button, input, a, [data-no-drag]")) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    windowDrag.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY, started: false };
  };
  const moveWindowDrag = (event: React.PointerEvent<HTMLElement>) => {
    const drag = windowDrag.current;
    if (!drag || drag.pointerId !== event.pointerId || !windowControls) return;
    if (!drag.started) {
      // Like Windows: a press only becomes a drag past a small threshold.
      if (Math.hypot(event.clientX - drag.x, event.clientY - drag.y) < 4) return;
      drag.started = true;
      if ((fullscreen || windowFillsScreen()) && bridge) {
        // Fullscreen cannot be dragged: the Desktop host leaves fullscreen
        // AND maximized in one step, restores the normal size centred under
        // the pointer, and arms the move — the same "drag a maximized
        // window" behaviour as the pinned titlebar.
        const ratio = drag.x / Math.max(1, window.innerWidth);
        const startX = drag.x;
        const startY = drag.y;
        void bridge.dragOutOfFullscreen({ pointerRatioX: ratio, titlebarOffsetY: Math.min(drag.y, 20) }).then((armed) => {
          if (!armed) {
            // Not actually fullscreen (a maximized window filling a screen
            // with no taskbar): the ordinary maximized-window drag applies.
            if (!windowDrag.current?.started) return;
            void windowControls.beginDrag({
              clientX: startX,
              screenX: event.screenX,
              screenY: event.screenY,
              titlebarOffsetY: startY,
              viewportWidth: window.innerWidth
            }).then(() => windowControls.moveDrag());
            return;
          }
          // Released before the window came out of fullscreen: disarm.
          if (windowDrag.current?.started) void windowControls.moveDrag();
          else void windowControls.endDrag();
        });
        return;
      }
      void windowControls.beginDrag({
        clientX: drag.x,
        screenX: event.screenX,
        screenY: event.screenY,
        titlebarOffsetY: drag.y,
        viewportWidth: window.innerWidth
      }).then(() => windowControls.moveDrag());
      return;
    }
    void windowControls.moveDrag();
  };
  const endWindowDrag = (event: React.PointerEvent<HTMLElement>) => {
    const drag = windowDrag.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    windowDrag.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    if (drag.started) void windowControls?.endDrag();
  };
  const filmstripHeight = preferences.showScrubBar ? FILMSTRIP_WITH_SCRUB_HEIGHT : FILMSTRIP_HEIGHT;
  const filmstripShown = preferences.showFilmstrip && state.entries.length > 1 && !slideshow && !(fullscreen && !chromeVisible);
  // Panels float over the window and step down below the titlebar while it is
  // shown (pinned or revealed), so its controls are never covered; a panel
  // under the pointer stays put until the pointer leaves it (FloatingPanel).
  const chromeTopShown = chrome.shown("top");
  const panelInsets = useMemo(() => ({
    top: !immersive && (preferences.titlebarPinned || chromeTopShown) ? TITLEBAR_HEIGHT : 0,
    bottom: filmstripShown ? filmstripHeight : 0
  }), [immersive, filmstripShown, filmstripHeight, preferences.titlebarPinned, chromeTopShown]);
  // The centred toolbar is the only thing panels avoid at the bottom edge.
  const [toolbarSpan, setToolbarSpan] = useState<{ left: number; right: number } | null>(null);
  useLayoutEffect(() => {
    const toolbar = document.querySelector(".iv-toolbar");
    if (!toolbar) {
      setToolbarSpan(null);
      return;
    }
    const measure = () => {
      const rect = toolbar.getBoundingClientRect();
      setToolbarSpan((current) => (current && current.left === Math.round(rect.left) && current.right === Math.round(rect.right) ? current : { left: Math.round(rect.left), right: Math.round(rect.right) }));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(toolbar);
    window.addEventListener("resize", measure);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [hasImage, isAnimated]);
  const bottomObstacle = useMemo(
    () => (hasImage && toolbarSpan ? { ...toolbarSpan, height: TOOLBAR_CLEARANCE } : null),
    [hasImage, toolbarSpan]
  );
  const [dockedSides, setDockedSides] = useState<Record<string, "left" | "right" | null>>({});
  const reportDock = useCallback((panel: string, side: "left" | "right" | null) => {
    setDockedSides((current) => (current[panel] === side ? current : { ...current, [panel]: side }));
  }, []);
  const panelsOpen = { info: preferences.showInfo && !slideshow, frames: framesOpen && Boolean(animation.state) && !slideshow };
  const leftDocked = (panelsOpen.info && dockedSides.info === "left") || (panelsOpen.frames && dockedSides.frames === "left");
  const rightDocked = (panelsOpen.info && dockedSides.info === "right") || (panelsOpen.frames && dockedSides.frames === "right");
  const activeMenuItems = menuSource === "more" ? moreItems : menuItems;

  return (
    <LiquidGlassLayer>
      <div
        className="iv-window"
        data-chrome-visible={chromeVisible ? "true" : "false"}
        data-toolbar={chrome.shown("bottom") ? "shown" : "hidden"}
        data-nav-left={chrome.shown("left") ? "shown" : "hidden"}
        data-nav-right={chrome.shown("right") ? "shown" : "hidden"}
        data-fullscreen={fullscreen ? "true" : "false"}
        data-maximized={maximized && !fullscreen ? "true" : "false"}
        data-immersive={immersive ? "true" : "false"}
        data-panel-left={leftDocked ? "true" : "false"}
        data-panel-right={rightDocked ? "true" : "false"}
        data-titlebar={preferences.titlebarPinned && !immersive ? "pinned" : chrome.shown("top") ? "revealed" : "hidden"}
        onPointerMove={noteActivity}
      >
        <LiquidGlassSurface
          as="header"
          className="iv-titlebar"
          onDoubleClick={(event: React.MouseEvent<HTMLElement>) => {
            if ((event.target as HTMLElement).closest("button, [data-no-drag]")) return;
            if (fullscreen) void toggleFullscreen();
            else void windowControls?.maximize();
          }}
          onPointerCancel={endWindowDrag}
          onPointerDown={beginWindowDrag}
          onPointerMove={moveWindowDrag}
          onPointerUp={endWindowDrag}
          tone="chrome"
        >
          <div className="iv-title">
            <icons.Image aria-hidden="true" className="iv-title-icon" size={15} />
            <span className="iv-title-name" title={current?.path}>{current?.name ?? "Mangoo 看图"}</span>
            {current ? (
              <span className="iv-title-meta" title={state.directory ?? undefined}>
                {[dims, fileSize].filter(Boolean).join("  ·  ")}
              </span>
            ) : null}
            {isRaw ? (
              <button
                className="iv-title-chip no-drag"
                data-no-drag="true"
                onClick={toggleRawSource}
                title={`${effectiveRawSource === "embedded" ? "正在显示相机内嵌的 JPEG 预览" : "正在显示 RAW 原图"}，点击切换（${keyOf("toggleRawSource") ?? "J"}）`}
                type="button"
              >
                <span data-button-content>{effectiveRawSource === "embedded" ? "内嵌 JPEG" : "RAW"}</span>
              </button>
            ) : null}
            {channel ? (
              <button
                className="iv-title-chip no-drag"
                data-channel={channel}
                data-no-drag="true"
                onClick={() => setChannel(null)}
                title="点击恢复全部通道"
                type="button"
              >
                <span data-button-content>{channel[0].toUpperCase()} 通道{preferences.channelPremultiply && channel !== "alpha" ? " · 预乘" : ""}</span>
              </button>
            ) : null}
          </div>
          <div className="iv-title-actions no-drag" data-no-drag="true">
            <IconButton active={preferences.showFilmstrip} disabled={state.entries.length < 2} label="缩略图条" onClick={() => setPreferences({ showFilmstrip: !preferences.showFilmstrip })} shortcut={keyOf("toggleFilmstrip")}>
              <icons.FilmStrip aria-hidden="true" size={15} />
            </IconButton>
            <IconButton active={preferences.showInfo} label="图片信息" onClick={() => setPreferences({ showInfo: !preferences.showInfo })} shortcut={keyOf("toggleInfo")}>
              <icons.Info aria-hidden="true" size={15} />
            </IconButton>
            <IconButton
              active={preferences.titlebarPinned}
              label={preferences.titlebarPinned ? "取消固定标题栏（自动隐藏）" : "固定标题栏"}
              onClick={() => {
                setPreferences({ titlebarPinned: !preferences.titlebarPinned });
                flashHud(preferences.titlebarPinned ? "标题栏将自动隐藏" : "标题栏已固定");
              }}
              shortcut={keyOf("toggleTitlebarPin")}
            >
              {preferences.titlebarPinned ? <icons.Pin aria-hidden="true" size={15} weight="fill" /> : <icons.Pin aria-hidden="true" size={15} />}
            </IconButton>
            <IconButton
              active={contextMenu !== null && menuSource === "more"}
              buttonRef={moreButtonRef}
              label="更多"
              onClick={() => {
                const rect = moreButtonRef.current?.getBoundingClientRect();
                setMenuSource("more");
                setContextMenu(rect ? { x: rect.right - 230, y: rect.bottom + 6 } : { x: 0, y: 0 });
              }}
            >
              <icons.MoreHorizontal aria-hidden="true" size={15} />
            </IconButton>
            <span className="iv-titlebar-divider" aria-hidden="true" />
            <WindowChrome />
          </div>
        </LiquidGlassSurface>

        <div className="iv-body">
          <div className="iv-main" data-comic={comic ? "true" : "false"}>
            {comic ? (
              <ComicView
                animatedExtensions={animatedExtensions}
                background={preferences.background === "black" ? "#000" : preferences.background === "light" ? "#e9ecef" : preferences.background === "dark" ? "#15181b" : "transparent"}
                cache={cache}
                entries={state.entries}
                gap={preferences.comicGap}
                index={state.index}
                jump={comicJump}
                rawSource={rawSource}
                onActivity={noteActivity}
                onCurrentChange={session.goTo}
                onZoom={(comicZoom) => {
                  setPreferences({ comicZoom });
                  flashHud(`宽度 ${Math.round(comicZoom * 100)}%`);
                }}
                zoom={preferences.comicZoom}
              />
            ) : null}
            <ViewerStage
              animate={animate}
              background={preferences.background}
              image={image}
              imageSize={imageSize}
              onActivity={noteActivity}
              onContextMenu={(x, y) => {
                setMenuSource("stage");
                setContextMenu({ x, y });
              }}
              onDropFiles={(files) => void dropFiles(files)}
              onNavigate={navigate}
              onPan={(dx, dy, bounded) => pan(dx, dy, bounded)}
              onPinch={(factor, anchor) => zoomBy(factor, anchor, "touch")}
              onToggleActualSize={toggleActualSize}
              onViewportChange={setViewport}
              onWheelNavigate={navigate}
              onWheelZoom={(factor, anchor, pinch) => zoomBy(factor, anchor, pinch ? "pinch" : "wheel")}
              overlay={stageOverlay}
              interpolation={interpolation}
              channel={channel}
              premultiply={preferences.channelPremultiply}
              renderLevel={canvasLevel}
              transform={transform}
              wheelNavigates={preferences.wheelAction === "navigate"}
              frameBitmap={animation.bitmap}
              zoomedIn={Boolean(imageSize) && zoomState === "custom" && transform.scale > fitScale(imageSize ?? { width: 1, height: 1 }, viewport, transform.rotation, "fit", preferences.upscaleSmall) + 1e-3}
            />

            {state.entries.length > 1 && !comic ? (
              <>
                <button aria-label="上一张" className="iv-nav-zone iv-nav-prev no-drag" data-pointer-glow="off" onClick={() => navigate(-1)} title="上一张（←）" type="button">
                  <icons.ChevronLeft aria-hidden="true" size={40} weight="light" />
                </button>
                <button aria-label="下一张" className="iv-nav-zone iv-nav-next no-drag" data-pointer-glow="off" onClick={() => navigate(1)} title="下一张（→）" type="button">
                  <icons.ChevronRight aria-hidden="true" size={40} weight="light" />
                </button>
              </>
            ) : null}

            {hasImage ? (
            <LiquidGlassSurface aria-label="看图工具栏" as="div" className="iv-toolbar" onPointerEnter={noteActivity} role="toolbar" tone="chrome">
              <IconButton disabled={state.entries.length < 2} label="上一张" onClick={() => navigate(-1)} shortcut={keyOf("previous")}>
                <icons.ArrowLeft aria-hidden="true" size={15} />
              </IconButton>
              <span className="iv-toolbar-counter" aria-live="polite">{counter}</span>
              <IconButton disabled={state.entries.length < 2} label="下一张" onClick={() => navigate(1)} shortcut={keyOf("next")}>
                <icons.ArrowRight aria-hidden="true" size={15} />
              </IconButton>
              <span className="iv-toolbar-divider" />
              <IconButton disabled={!image} label="缩小" onClick={() => zoomStep(-1)} shortcut={keyOf("zoomOut")}>
                <icons.ZoomOut aria-hidden="true" size={15} />
              </IconButton>
              <LiquidGlassButton
                aria-expanded={zoomMenuOpen}
                aria-haspopup="menu"
                aria-label={`缩放：${zoomLabel}`}
                className="iv-zoom-button no-drag"
                disabled={!image}
                onClick={() => setZoomMenuOpen((value) => !value)}
                ref={zoomButtonRef}
                title="选择缩放比例"
                tone="control"
                type="button"
              >
                <span>{zoomLabel}</span>
                <icons.ChevronUp aria-hidden="true" size={11} />
              </LiquidGlassButton>
              <IconButton disabled={!image} label="放大" onClick={() => zoomStep(1)} shortcut={keyOf("zoomIn")}>
                <icons.ZoomIn aria-hidden="true" size={15} />
              </IconButton>
              <IconButton
                disabled={!image}
                label={zoomState === "fit" ? "原始大小" : "适应窗口"}
                onClick={() => (zoomState === "fit" ? zoomTo(1) : setFit("fit"))}
                shortcut={zoomState === "fit" ? keyOf("actualSize") : keyOf("fit")}
              >
                {zoomState === "fit" ? <span className="iv-one-to-one" aria-hidden="true">1:1</span> : <icons.Minimize2 aria-hidden="true" size={14} />}
              </IconButton>
              <span className="iv-toolbar-divider" />
              <IconButton active={comic} disabled={state.entries.length < 1} label={comic ? "退出漫画模式" : "漫画（条漫）模式"} onClick={toggleComic} shortcut={keyOf("toggleComic")}>
                <icons.ScrollText aria-hidden="true" size={15} />
              </IconButton>
              <IconButton disabled={!image || comic} label="向右旋转" onClick={() => rotateBy(1)} shortcut={keyOf("rotateRight")}>
                <icons.RotateCw aria-hidden="true" size={15} />
              </IconButton>
              {isRaw ? (
                <LiquidGlassButton
                  aria-label={effectiveRawSource === "embedded" ? "正在显示内嵌 JPEG，切换为 RAW 原图" : "正在显示 RAW 原图，切换为内嵌 JPEG"}
                  className="iv-raw-switch no-drag"
                  data-pointer-glow="off"
                  onClick={toggleRawSource}
                  title={`RAW 原图 / 相机内嵌 JPEG（${keyOf("toggleRawSource") ?? "J"}）`}
                  tone="control"
                  type="button"
                >
                  {/* One compact chip naming what is shown; a click switches. */}
                  <span className="iv-raw-label" data-source={effectiveRawSource}>
                    {effectiveRawSource === "embedded" ? "JPG" : "RAW"}
                  </span>
                </LiquidGlassButton>
              ) : null}
              {isAnimated ? (
                <IconButton active={framesOpen} label="逐帧查看动图" onClick={() => toggleFrames()} shortcut={keyOf("nextFrame")}>
                  <icons.FilmReel aria-hidden="true" size={15} />
                </IconButton>
              ) : null}
              <IconButton active={slideshow} disabled={state.entries.length < 2} label={slideshow ? "停止幻灯片" : "幻灯片放映"} onClick={() => setSlideshow((value) => !value)} shortcut={keyOf("slideshow")}>
                {slideshow ? <icons.Pause aria-hidden="true" size={15} /> : <icons.Slideshow aria-hidden="true" size={15} />}
              </IconButton>
              <IconButton active={fullscreen} label={fullscreen ? "退出全屏" : "全屏"} onClick={() => void toggleFullscreen()} shortcut={keyOf("fullscreen")}>
                {fullscreen ? <icons.ExitFullscreen aria-hidden="true" size={15} /> : <icons.Fullscreen aria-hidden="true" size={15} />}
              </IconButton>
              <span className="iv-toolbar-divider" />
              <IconButton danger disabled={!hasImage} label="删除到回收站" onClick={requestDelete} shortcut={keyOf("delete")}>
                <icons.Trash2 aria-hidden="true" size={15} />
              </IconButton>
            </LiquidGlassSurface>
            ) : null}

            {toast ? (
              <div className="iv-toast" role="status">
                <span>{toast.message}</span>
                {toast.action ? (
                  <button className="iv-toast-action" onClick={() => { toast.action?.run(); setToast(null); }} type="button"><span data-button-content>{toast.action.label}</span></button>
                ) : null}
              </div>
            ) : null}
          </div>

        </div>

        {preferences.showFilmstrip && state.entries.length > 1 && !slideshow ? (
          <LiquidGlassSurface as="div" className="iv-filmstrip-pane" data-scrub={preferences.showScrubBar ? "true" : "false"} tone="chrome">
            {preferences.showScrubBar ? <ScrubBar entries={state.entries} index={state.index} onSelect={goToIndex} /> : null}
            <Filmstrip entries={state.entries} index={state.index} onSelect={goToIndex} />
          </LiquidGlassSurface>
        ) : null}
        <div className="iv-float-layer" style={{ bottom: filmstripShown ? filmstripHeight : 0 }}>
        {preferences.showInfo && !slideshow ? (
          <FloatingPanel
            defaultCorner="top-left"
            defaultSize={{ width: 344, height: 640 }}
            icon={<icons.Info aria-hidden="true" size={14} />}
            bottomObstacle={bottomObstacle}
            id="info-v4"
            onDockChange={(side) => reportDock("info", side)}
            insets={panelInsets}
            onClose={() => setPreferences({ showInfo: false })}
            title="图片信息"
          >
            <InfoPanel animation={animation.state} entry={current} image={image} />
          </FloatingPanel>
        ) : null}
        {framesOpen && animation.state && !slideshow ? (
          <FloatingPanel
            className="iv-frames-panel"
            defaultSize={{ width: 260, height: 132 }}
            icon={<icons.FilmReel aria-hidden="true" size={14} />}
            defaultCorner="bottom-left"
            bottomObstacle={bottomObstacle}
            id="frames-v2"
            onDockChange={(side) => reportDock("frames", side)}
            insets={panelInsets}
            onClose={() => toggleFrames(false)}
            resizable={false}
            title="动图帧"
          >
            <FrameInspector
              onLoop={animation.setLoop}
              onPlayPause={animation.playPause}
              onSeek={animation.seek}
              onSpeed={animation.setSpeed}
              onStep={animation.step}
              state={animation.state}
            />
          </FloatingPanel>
        ) : null}
        </div>
      </div>

      <AnchoredOverlay
        anchor={zoomButtonRef.current}
        ariaLabel="缩放比例"
        className="overlay-titlebar-popover iv-menu iv-menu-pointer"
        id="image-viewer-zoom"
        onDismiss={() => setZoomMenuOpen(false)}
        open={zoomMenuOpen}
        placement="top-start"
        preferredWidth={160}
        restoreFocusTarget={zoomButtonRef.current}
        role="menu"
        rootId="preview-render"
      >
        <OverlayMenuList ariaLabel="缩放比例" className="overlay-titlebar-menu">
          {(["fit", "fill"] as const).map((mode) => (
            <OverlayMenuItem checked={zoomState === mode} key={mode} onClick={() => { setFit(mode); setZoomMenuOpen(false); }} role="menuitemradio" selected={zoomState === mode} shortcut={mode === "fit" ? "0" : undefined}>
              {mode === "fit" ? "适应窗口" : "填满窗口"}
            </OverlayMenuItem>
          ))}
          <div className="overlay-menu-separator" role="separator" />
          {ZOOM_PRESETS.map((scale) => (
            <OverlayMenuItem
              checked={zoomState === "custom" && Math.abs(transform.scale - scale) < 0.005}
              key={scale}
              onClick={() => { zoomTo(scale); setZoomMenuOpen(false); }}
              role="menuitemradio"
              selected={zoomState === "custom" && Math.abs(transform.scale - scale) < 0.005}
              shortcut={scale === 1 ? "1" : undefined}
            >
              {formatZoom(scale)}
            </OverlayMenuItem>
          ))}
        </OverlayMenuList>
      </AnchoredOverlay>


      {contextMenu ? (
        <AnchoredOverlay
          anchor={pointOverlayAnchor(contextMenu.x, contextMenu.y)}
          ariaLabel={menuSource === "more" ? "更多" : "图片操作"}
          className={`context-menu iv-menu${contextMenu.byKeyboard ? "" : " iv-menu-pointer"}`}
          id="image-viewer-context-menu"
          onDismiss={() => setContextMenu(null)}
          open
          placement="bottom-start"
          preferredWidth={230}
          role="group"
          rootId="context-menu"
        >
          <OverlayMenuList ariaLabel={menuSource === "more" ? "更多" : "图片操作"}>
            {activeMenuItems.map((item) =>
              item.separator ? (
                <div className="overlay-menu-separator" key={item.id} role="separator" />
              ) : (
                <OverlayMenuItem
                  checked={item.checked}
                  danger={item.danger}
                  disabled={item.disabled}
                  icon={item.icon}
                  key={item.id}
                  onClick={(event) => {
                    if (item.submenu) {
                      setOpenSubmenu({ item, anchor: event.currentTarget });
                      return;
                    }
                    setContextMenu(null);
                    item.onSelect?.();
                  }}
                  onKeyDown={(event) => {
                    if (item.submenu && event.key === "ArrowRight") {
                      event.preventDefault();
                      event.stopPropagation();
                      setOpenSubmenu({ item, anchor: event.currentTarget });
                    }
                  }}
                  onPointerEnter={(event) => {
                    // Hovering opens the submenu; hovering a sibling closes it.
                    if (item.submenu) setOpenSubmenu({ item, anchor: event.currentTarget });
                    else setOpenSubmenu(null);
                  }}
                  role="menuitem"
                  shortcut={item.submenu ? undefined : item.checked ? <icons.Check aria-hidden="true" size={13} /> : item.shortcut}
                  submenu={Boolean(item.submenu)}
                >
                  {item.label}
                </OverlayMenuItem>
              )
            )}
          </OverlayMenuList>
        </AnchoredOverlay>
      ) : null}
      {contextMenu && openSubmenu?.item.submenu ? (
        <AnchoredOverlay
          anchor={openSubmenu.anchor}
          ariaLabel={openSubmenu.item.label ?? ""}
          className={`context-menu iv-menu${contextMenu.byKeyboard ? "" : " iv-menu-pointer"}`}
          id="image-viewer-context-submenu"
          onDismiss={() => setOpenSubmenu(null)}
          open
          parentId="image-viewer-context-menu"
          placement="right-start"
          preferredWidth={210}
          restoreFocusTarget={openSubmenu.anchor}
          role="group"
          rootId="context-menu"
          surfaceRole="nested"
        >
          <OverlayMenuList ariaLabel={openSubmenu.item.label ?? ""}>
            {openSubmenu.item.submenu.map((item) =>
              item.separator ? (
                <div className="overlay-menu-separator" key={item.id} role="separator" />
              ) : (
                <OverlayMenuItem
                  checked={item.checked}
                  disabled={item.disabled}
                  icon={item.icon}
                  key={item.id}
                  onClick={() => {
                    setContextMenu(null);
                    item.onSelect?.();
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "ArrowLeft") {
                      event.preventDefault();
                      event.stopPropagation();
                      const anchor = openSubmenu.anchor;
                      setOpenSubmenu(null);
                      anchor.focus();
                    }
                  }}
                  role="menuitem"
                  shortcut={item.checked ? <icons.Check aria-hidden="true" size={13} /> : item.shortcut}
                >
                  {item.label}
                </OverlayMenuItem>
              )
            )}
          </OverlayMenuList>
        </AnchoredOverlay>
      ) : null}

      <ModalSurface
        ariaDescribedBy="image-viewer-delete-description"
        ariaLabelledBy="image-viewer-delete-title"
        className="file-operation-dialog iv-dialog"
        id="image-viewer-delete-dialog"
        onDismiss={() => setDialog(null)}
        open={dialog?.kind === "delete"}
        role="alertdialog"
        rootId="file-operation"
      >
        <OverlayDialogHeader
          description={pairedPath ? "这张 RAW 和它的同名 JPEG 会一起移到回收站，需要时可以从回收站恢复。" : "图片会移到回收站，需要时可以从回收站恢复。"}
          descriptionId="image-viewer-delete-description"
          title="删除这张图片？"
          titleId="image-viewer-delete-title"
        />
        <OverlayDialogBody>
          <p className="overlay-dialog-text iv-dialog-filename">{current?.name}</p>
          <label className="iv-dialog-check">
            <input
              checked={dialog?.kind === "delete" && dialog.dontAskAgain}
              onChange={(event) => dialog?.kind === "delete" && setDialog({ ...dialog, dontAskAgain: event.target.checked })}
              type="checkbox"
            />
            <span>以后不再询问</span>
          </label>
          <OverlayDialogActions>
            <LiquidGlassButton className="overlay-dialog-button" onClick={() => setDialog(null)} type="button">取消</LiquidGlassButton>
            <LiquidGlassButton
              autoFocus
              className="overlay-dialog-button overlay-dialog-button-primary"
              onClick={() => void performDelete(dialog?.kind === "delete" && dialog.dontAskAgain)}
              tone="danger"
              type="button"
            >
              删除
            </LiquidGlassButton>
          </OverlayDialogActions>
        </OverlayDialogBody>
      </ModalSurface>

      <ModalSurface
        ariaLabelledBy="image-viewer-rename-title"
        busy={dialog?.kind === "rename" && dialog.busy}
        className="file-operation-dialog iv-dialog"
        id="image-viewer-rename-dialog"
        initialFocusRef={renameInputRef}
        onDismiss={() => setDialog(null)}
        open={dialog?.kind === "rename"}
        rootId="file-operation"
      >
        <OverlayDialogHeader
          description={current ? `只需修改名称部分，不输入扩展名时会自动保留 ${current.ext}。` : undefined}
          title="重命名图片"
          titleId="image-viewer-rename-title"
        />
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void performRename();
          }}
        >
          <OverlayDialogBody>
            <OverlayField
              autoComplete="off"
              disabled={dialog?.kind === "rename" && dialog.busy}
              error={dialog?.kind === "rename" ? dialog.error : null}
              inputRef={renameInputRef}
              label="新名称"
              onChange={(event) => dialog?.kind === "rename" && setDialog({ ...dialog, value: event.target.value, error: null })}
              spellCheck={false}
              type="text"
              value={dialog?.kind === "rename" ? dialog.value : ""}
            />
            <OverlayDialogActions>
              <LiquidGlassButton className="overlay-dialog-button" onClick={() => setDialog(null)} type="button">取消</LiquidGlassButton>
              <LiquidGlassButton
                className="overlay-dialog-button overlay-dialog-button-primary"
                disabled={dialog?.kind === "rename" && (dialog.busy || !dialog.value.trim())}
                tone="primary"
                type="submit"
              >
                {dialog?.kind === "rename" && dialog.busy ? "正在重命名…" : "重命名"}
              </LiquidGlassButton>
            </OverlayDialogActions>
          </OverlayDialogBody>
        </form>
      </ModalSurface>
    </LiquidGlassLayer>
  );
}
