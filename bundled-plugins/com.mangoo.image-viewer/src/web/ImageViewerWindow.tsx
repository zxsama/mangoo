import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
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
  icons,
  pointOverlayAnchor,
  useSecondaryWindowInterfaceStyle
} from "@mangoo/web/plugin-host";

import { extensionOf, splitPath } from "../shared/formats.ts";
import { formatBytes, prefetchOrder, validateFileName } from "../shared/navigation.ts";
import {
  clampPan,
  fitScale,
  formatZoom,
  identityTransform,
  nextZoomStop,
  renderLevel,
  rotate,
  zoomAt,
  type FitMode,
  type Size,
  type ViewTransform
} from "../shared/viewTransform.ts";
import { ShortcutList } from "./ImageViewerPluginPanel.tsx";
import { ViewerSettingsForm } from "./ViewerSettings.tsx";
import { Filmstrip } from "./Filmstrip.tsx";
import { InfoPanel } from "./InfoPanel.tsx";
import { viewerBridge } from "./bridge.ts";
import { ViewerImageCache, encodePng, viewerViewportLongPx, type LoadedImage } from "./imageSource.ts";
import { useViewerPreferences } from "./preferences.ts";
import { describeError, useViewerSession } from "./useViewerSession.ts";
import { ViewerStage } from "./ViewerStage.tsx";
import "./viewer.css";

type ZoomState = FitMode | "custom";
type Dialog =
  | { kind: "delete"; dontAskAgain: boolean }
  | { kind: "rename"; value: string; error: string | null; busy: boolean }
  | { kind: "settings" }
  | { kind: "shortcuts" }
  | null;

const ZOOM_PRESETS = [0.25, 0.5, 1, 2, 4];

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
      tone={danger ? "danger" : "control"}
      type="button"
    >
      {children}
    </LiquidGlassButton>
  );
}

export function ImageViewerWindow() {
  useSecondaryWindowInterfaceStyle();
  const bridge = viewerBridge();
  const [preferences, setPreferences] = useViewerPreferences();
  const session = useViewerSession(preferences.sortKey, preferences.sortDirection, preferences.loop);
  const { state, current } = session;
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
  const [animate, setAnimate] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const [slideshow, setSlideshow] = useState(false);
  const [chromeVisible, setChromeVisible] = useState(true);
  const [zoomMenuOpen, setZoomMenuOpen] = useState(false);
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [toast, setToast] = useState<{ message: string; action?: { label: string; run(): void } } | null>(null);
  /** Brief centred readout for zoom / rotation changes (like most viewers). */
  const [hud, setHud] = useState<string | null>(null);
  const hudTimer = useRef<number | null>(null);
  const [slideshowTick, setSlideshowTick] = useState(0);
  const [alwaysOnTop, setAlwaysOnTop] = useState(false);

  const zoomButtonRef = useRef<HTMLButtonElement | null>(null);
  const moreButtonRef = useRef<HTMLButtonElement | null>(null);
  const renameInputRef = useRef<HTMLInputElement | null>(null);
  const hideChromeTimer = useRef<number | null>(null);
  const animateTimer = useRef<number | null>(null);
  const toastTimer = useRef<number | null>(null);
  const lastRefresh = useRef(0);

  const imageSize = useMemo<Size | null>(
    () => (image ? { width: image.width / dpr, height: image.height / dpr } : null),
    [image, dpr]
  );

  const showToast = useCallback((message: string, action?: { label: string; run(): void }) => {
    setToast({ message, action });
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), action ? 6000 : 2400);
  }, []);

  const flashHud = useCallback((message: string) => {
    setHud(message);
    if (hudTimer.current) window.clearTimeout(hudTimer.current);
    hudTimer.current = window.setTimeout(() => setHud(null), 900);
  }, []);

  // ------------------------------------------------------------- bootstrap

  useEffect(() => {
    createCoreClient()
      .getImageViewerFormats()
      .then((formats) => {
        setAnimatedExtensions(formats.animatedExtensions);
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
    return () => window.removeEventListener("focus", onFocus);
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
    setImageError(null);
    const cached = cache.peek(ViewerImageCache.key(current, false)) ?? cache.peek(ViewerImageCache.key(current, true));
    if (cached) {
      setImage(cached);
      setImageLoading(false);
    } else {
      setImageLoading(true);
    }
    const viewportLongPx = viewerViewportLongPx();
    if (!cached) {
      cache
        .load(current, { viewportLongPx, full: false, animatedExtensions })
        .then((loaded) => {
          if (cancelled) return;
          setImage(loaded);
          setImageLoading(false);
        }, (error) => {
          if (cancelled) return;
          setImage(null);
          setImageLoading(false);
          setImageError(describeError(error));
        });
    }
    cache.pin([ViewerImageCache.key(current, false), ViewerImageCache.key(current, true)]);
    // Warm the neighbours once the visible picture is underway.
    const prefetch = window.setTimeout(() => {
      for (const index of prefetchOrder(state.entries.length, state.index, 2, preferences.loop)) {
        const entry = state.entries[index];
        if (entry) void cache.load(entry, { viewportLongPx, full: false, animatedExtensions, priority: "resource" }).catch(() => undefined);
      }
    }, 180);
    return () => {
      cancelled = true;
      window.clearTimeout(prefetch);
    };
    // Entries are read for prefetch only; navigation is keyed by `current`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current, cache, animatedExtensions]);

  // Reset the view for a new picture (unless the user asked to keep it).
  const imageKey = image?.key.split("\u0000")[0] ?? null;
  useEffect(() => {
    if (!imageKey) return;
    setAnimate(false);
    if (!preferences.keepTransform) {
      setZoomState(preferences.fitMode);
      setTransform((value) => ({ ...identityTransform(), scale: value.scale }));
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
    const needed = transform.scale * image.width;
    if (needed <= image.bitmap.width * 1.1) return;
    let cancelled = false;
    void cache.load(current, { viewportLongPx: viewerViewportLongPx(), full: true, animatedExtensions }).then((full) => {
      if (!cancelled) setImage(full);
    }, () => undefined);
    return () => {
      cancelled = true;
    };
  }, [current, image, transform.scale, cache, animatedExtensions]);

  // ------------------------------------------------------------- transforms

  const withAnimation = useCallback(() => {
    setAnimate(true);
    if (animateTimer.current) window.clearTimeout(animateTimer.current);
    animateTimer.current = window.setTimeout(() => setAnimate(false), 200);
  }, []);

  const center = useMemo(() => ({ x: viewport.width / 2, y: viewport.height / 2 }), [viewport]);

  const zoomTo = useCallback((scale: number, anchor = center, animated = true) => {
    if (!imageSize) return;
    if (animated) withAnimation();
    else setAnimate(false);
    setZoomState("custom");
    setTransform((value) => zoomAt(value, scale, anchor, imageSize, viewport));
    flashHud(formatZoom(Math.min(32, Math.max(0.02, scale))));
  }, [center, flashHud, imageSize, viewport, withAnimation]);

  /** Relative zoom (wheel, pinch): reads the live scale, never a stale one. */
  const zoomBy = useCallback((factor: number, anchor: { x: number; y: number }) => {
    if (!imageSize) return;
    setAnimate(false);
    setZoomState("custom");
    setTransform((value) => zoomAt(value, value.scale * factor, anchor, imageSize, viewport));
  }, [imageSize, viewport]);

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

  const pan = useCallback((deltaX: number, deltaY: number) => {
    if (!imageSize) return;
    setAnimate(false);
    setTransform((value) => clampPan({ ...value, panX: value.panX + deltaX, panY: value.panY + deltaY }, imageSize, viewport));
  }, [imageSize, viewport]);

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
    if (bridge) setFullscreen(await bridge.toggleFullscreen());
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
    if (!result.ok) showToast(result.error ?? "无法显示文件");
  }, [bridge, current, showToast]);

  const openExternally = useCallback(async () => {
    if (!bridge || !current) return;
    const result = await bridge.openExternally(current.path);
    if (!result.ok) showToast(result.error ?? "无法打开");
  }, [bridge, current, showToast]);

  const performDelete = useCallback(async (dontAskAgain = false) => {
    if (!bridge || !current) return;
    const target = current;
    const result = await bridge.trash(target.path);
    setDialog(null);
    if (dontAskAgain) setPreferences({ confirmDelete: false });
    if (!result.ok) {
      showToast(result.error ?? "删除失败");
      return;
    }
    cache.forget(target.path);
    session.removeCurrent();
    showToast(`“${target.name}”已移到回收站，可在回收站中恢复`);
  }, [bridge, cache, current, session, setPreferences, showToast]);

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
    session.replaceCurrent(result.path);
    setDialog(null);
    showToast("已重命名");
  }, [bridge, cache, current, dialog, session, showToast]);

  useEffect(() => {
    if (dialog?.kind !== "rename") return;
    const input = renameInputRef.current;
    if (!input || document.activeElement === input) return;
    input.focus();
    const dot = input.value.lastIndexOf(".");
    input.setSelectionRange(0, dot > 0 ? dot : input.value.length);
  }, [dialog]);

  const toggleAlwaysOnTop = useCallback(async () => {
    if (bridge) setAlwaysOnTop(await bridge.setAlwaysOnTop(!alwaysOnTop));
  }, [alwaysOnTop, bridge]);

  // -------------------------------------------------------- slideshow, chrome

  useEffect(() => {
    if (!slideshow) return;
    setSlideshowTick((value) => value + 1);
    const timer = window.setInterval(() => {
      setSlideshowTick((value) => value + 1);
      if (!preferences.loop && state.index >= state.entries.length - 1) {
        setSlideshow(false);
        return;
      }
      session.go(1);
    }, preferences.slideshowSeconds * 1000);
    return () => window.clearInterval(timer);
  }, [slideshow, preferences.slideshowSeconds, preferences.loop, session, state.index, state.entries.length]);

  const immersive = fullscreen || slideshow;
  const [controlsIdle, setControlsIdle] = useState(false);
  const noteActivity = useCallback(() => {
    setChromeVisible(true);
    setControlsIdle(false);
    if (hideChromeTimer.current) window.clearTimeout(hideChromeTimer.current);
    hideChromeTimer.current = window.setTimeout(() => {
      setControlsIdle(true);
      if (immersive) setChromeVisible(false);
    }, immersive ? 2200 : 2800);
  }, [immersive]);

  useEffect(() => {
    noteActivity();
  }, [immersive, noteActivity]);

  // ------------------------------------------------------------- keyboard

  const overlaysOpen = Boolean(dialog || zoomMenuOpen || contextMenu);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (overlaysOpen || isEditableTarget(event.target)) return;
      // A focused button activates on Space/Enter; do not also navigate.
      if ((event.key === " " || event.key === "Enter") && (event.target as HTMLElement | null)?.closest?.("button")) return;
      const ctrl = event.ctrlKey || event.metaKey;
      // Up/Down pan a zoomed-in picture and otherwise navigate, like most viewers.
      const zoomedIn = Boolean(imageSize) && transform.scale > fitScale(imageSize ?? { width: 1, height: 1 }, viewport, transform.rotation, "fit", preferences.upscaleSmall) + 1e-3;
      const key = event.key;
      let handled = true;
      if (ctrl && key.toLowerCase() === "o") void openFiles();
      else if (ctrl && event.shiftKey && key.toLowerCase() === "c") void copyPath();
      else if (ctrl && key.toLowerCase() === "c") void copyImage();
      else if (ctrl && key.toLowerCase() === "w") window.close();
      else if (ctrl && (key === "0")) setFit("fit");
      else if (ctrl && (key === "1")) zoomTo(1);
      else if (ctrl && (key === "=" || key === "+")) zoomStep(1);
      else if (ctrl && key === "-") zoomStep(-1);
      else if (ctrl) handled = false;
      else if (key === "ArrowRight" || key === "PageDown" || key === " " || (key === "ArrowDown" && !zoomedIn)) session.go(1);
      else if (key === "ArrowLeft" || key === "PageUp" || key === "Backspace" || (key === "ArrowUp" && !zoomedIn)) session.go(-1);
      else if (key === "ArrowDown") pan(0, -60);
      else if (key === "ArrowUp") pan(0, 60);
      else if (key === "Home") session.goTo(0);
      else if (key === "End") session.goTo(state.entries.length - 1);
      else if (key === "+" || key === "=") zoomStep(1);
      else if (key === "-" || key === "_") zoomStep(-1);
      else if (key === "0") setFit("fit");
      else if (key === "1") zoomTo(1);
      else if (key === "r") rotateBy(1);
      else if (key === "R" || (key === "l" || key === "L")) rotateBy(-1);
      else if (key === "h" || key === "H") flip("x");
      else if (key === "v" || key === "V") flip("y");
      else if (key === "F11" || key === "f" || key === "F") void toggleFullscreen();
      else if (key === "F5" || key === "s" || key === "S") setSlideshow((value) => !value);
      else if (key === "i" || key === "I") setPreferences({ showInfo: !preferences.showInfo });
      else if (key === "t" || key === "T") setPreferences({ showFilmstrip: !preferences.showFilmstrip });
      else if (key === "Delete") requestDelete();
      else if (key === "?" || (key === "/" && event.shiftKey)) setDialog({ kind: "shortcuts" });
      else if (key === "F2") requestRename();
      else if (key === "Escape") {
        if (slideshow) setSlideshow(false);
        else if (fullscreen) void toggleFullscreen();
        else if (preferences.escapeCloses) window.close();
        else handled = false;
      } else handled = false;
      if (handled) {
        event.preventDefault();
        noteActivity();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    copyImage, copyPath, flip, fullscreen, imageSize, noteActivity, openFiles, overlaysOpen, pan, preferences,
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
  const pixelated = preferences.pixelatedZoom && transform.scale >= 2;
  const canvasLevel = image?.kind === "bitmap" ? renderLevel((transform.scale * image.width) / Math.max(1, image.bitmap.width)) : 1;
  const zoomLabel = !image ? "—" : zoomState === "fit" ? `适应 · ${formatZoom(transform.scale)}` : zoomState === "fill" ? `填满 · ${formatZoom(transform.scale)}` : formatZoom(transform.scale);
  const fileSize = current && current.sizeBytes > 0 ? formatBytes(current.sizeBytes) : null;

  const stageOverlay = (
    <>
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
      {hud ? <div className="iv-hud" role="status">{hud}</div> : null}
      {slideshow ? (
        <div className="iv-slideshow-progress" aria-hidden="true">
          <span key={slideshowTick} style={{ animationDuration: `${preferences.slideshowSeconds}s` }} />
        </div>
      ) : null}
    </>
  );

  const menuItems: Array<{ id: string; label?: string; icon?: ReactNode; shortcut?: string; disabled?: boolean; danger?: boolean; checked?: boolean; onSelect?: () => void; separator?: boolean }> = [
    { id: "copy", label: "复制图片", icon: <icons.Copy size={14} />, shortcut: "Ctrl+C", disabled: !image, onSelect: () => void copyImage() },
    { id: "copy-path", label: "复制文件路径", icon: <icons.Clipboard size={14} />, shortcut: "Ctrl+Shift+C", disabled: !hasImage, onSelect: () => void copyPath() },
    { id: "s1", separator: true },
    { id: "rotate-right", label: "向右旋转", icon: <icons.RotateCw size={14} />, shortcut: "R", disabled: !hasImage, onSelect: () => rotateBy(1) },
    { id: "rotate-left", label: "向左旋转", icon: <icons.RotateCcw size={14} />, shortcut: "Shift+R", disabled: !hasImage, onSelect: () => rotateBy(-1) },
    { id: "flip-x", label: "水平翻转", icon: <icons.FlipHorizontal size={14} />, shortcut: "H", disabled: !hasImage, checked: transform.flipX, onSelect: () => flip("x") },
    { id: "flip-y", label: "垂直翻转", icon: <icons.FlipVertical size={14} />, shortcut: "V", disabled: !hasImage, checked: transform.flipY, onSelect: () => flip("y") },
    { id: "s2", separator: true },
    { id: "reveal", label: "在文件夹中显示", icon: <icons.FolderOpen size={14} />, disabled: !hasImage, onSelect: () => void reveal() },
    { id: "open-external", label: "用其他应用打开", icon: <icons.ExternalLink size={14} />, disabled: !hasImage, onSelect: () => void openExternally() },
    { id: "rename", label: "重命名…", icon: <icons.Pencil size={14} />, shortcut: "F2", disabled: !hasImage, onSelect: requestRename },
    { id: "delete", label: "删除到回收站", icon: <icons.Trash2 size={14} />, shortcut: "Delete", danger: true, disabled: !hasImage, onSelect: requestDelete },
    { id: "s3", separator: true },
    { id: "slideshow", label: slideshow ? "停止幻灯片" : "幻灯片放映", icon: slideshow ? <icons.Pause size={14} /> : <icons.Play size={14} />, shortcut: "S", disabled: state.entries.length < 2, onSelect: () => setSlideshow((value) => !value) },
    { id: "fullscreen", label: fullscreen ? "退出全屏" : "全屏", icon: fullscreen ? <icons.ExitFullscreen size={14} /> : <icons.Fullscreen size={14} />, shortcut: "F11", onSelect: () => void toggleFullscreen() }
  ];

  /** Titlebar "more" menu: window-level things that are not about the picture. */
  const moreItems: typeof menuItems = [
    { id: "open", label: "打开图片…", icon: <icons.FolderOpen size={14} />, shortcut: "Ctrl+O", onSelect: () => void openFiles() },
    { id: "open-folder", label: "打开文件夹…", icon: <icons.Folder size={14} />, onSelect: () => void openFolder() },
    { id: "s1", separator: true },
    { id: "on-top", label: "窗口置顶", icon: <icons.Pin size={14} />, checked: alwaysOnTop, onSelect: () => void toggleAlwaysOnTop() },
    { id: "main", label: "打开 Mangoo 资料库", icon: <icons.Library size={14} />, disabled: !bridge, onSelect: () => void bridge?.openMainWindow() },
    { id: "s2", separator: true },
    { id: "settings", label: "看图设置…", icon: <icons.Settings size={14} />, onSelect: () => openDialog({ kind: "settings" }) },
    { id: "shortcuts", label: "键盘快捷键", icon: <icons.Keyboard size={14} />, shortcut: "?", onSelect: () => openDialog({ kind: "shortcuts" }) }
  ];
  const [menuSource, setMenuSource] = useState<"stage" | "more">("stage");
  const activeMenuItems = menuSource === "more" ? moreItems : menuItems;

  return (
    <LiquidGlassLayer>
      <div
        className="iv-window"
        data-chrome-visible={chromeVisible ? "true" : "false"}
        data-controls-idle={controlsIdle && !overlaysOpen ? "true" : "false"}
        data-fullscreen={fullscreen ? "true" : "false"}
        data-immersive={immersive ? "true" : "false"}
        onPointerMove={noteActivity}
      >
        <LiquidGlassSurface as="header" className="iv-titlebar" tone="chrome">
          <div className="iv-title">
            <icons.Image aria-hidden="true" className="iv-title-icon" size={15} />
            <span className="iv-title-name" title={current?.path}>{current?.name ?? "Mangoo 看图"}</span>
            {current ? (
              <span className="iv-title-meta" title={state.directory ?? undefined}>
                {[counter, dims, fileSize].filter(Boolean).join("  ·  ")}
              </span>
            ) : null}
          </div>
          <div className="iv-title-actions no-drag" data-no-drag="true">
            <IconButton active={preferences.showFilmstrip} disabled={state.entries.length < 2} label="缩略图条" onClick={() => setPreferences({ showFilmstrip: !preferences.showFilmstrip })} shortcut="T">
              <icons.FilmStrip aria-hidden="true" size={15} />
            </IconButton>
            <IconButton active={preferences.showInfo} label="图片信息" onClick={() => setPreferences({ showInfo: !preferences.showInfo })} shortcut="I">
              <icons.Info aria-hidden="true" size={15} />
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
          <div className="iv-main">
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
              onNavigate={(direction) => session.go(direction)}
              onPan={pan}
              onPinch={zoomBy}
              onToggleActualSize={toggleActualSize}
              onViewportChange={setViewport}
              onWheelNavigate={(direction) => session.go(direction)}
              onWheelZoom={zoomBy}
              overlay={stageOverlay}
              pixelated={pixelated}
              renderLevel={canvasLevel}
              transform={transform}
              wheelNavigates={preferences.wheelAction === "navigate"}
            />

            {state.entries.length > 1 ? (
              <>
                <button aria-label="上一张" className="iv-nav-zone iv-nav-prev no-drag" data-pointer-glow="off" onClick={() => session.go(-1)} title="上一张（←）" type="button">
                  <icons.ChevronLeft aria-hidden="true" size={22} />
                </button>
                <button aria-label="下一张" className="iv-nav-zone iv-nav-next no-drag" data-pointer-glow="off" onClick={() => session.go(1)} title="下一张（→）" type="button">
                  <icons.ChevronRight aria-hidden="true" size={22} />
                </button>
              </>
            ) : null}

            {hasImage ? (
            <LiquidGlassSurface aria-label="看图工具栏" as="div" className="iv-toolbar" onPointerEnter={noteActivity} role="toolbar" tone="chrome">
              <IconButton disabled={state.entries.length < 2} label="上一张" onClick={() => session.go(-1)} shortcut="←">
                <icons.ArrowLeft aria-hidden="true" size={15} />
              </IconButton>
              <span className="iv-toolbar-counter" aria-live="polite">{counter}</span>
              <IconButton disabled={state.entries.length < 2} label="下一张" onClick={() => session.go(1)} shortcut="→">
                <icons.ArrowRight aria-hidden="true" size={15} />
              </IconButton>
              <span className="iv-toolbar-divider" />
              <IconButton disabled={!image} label="缩小" onClick={() => zoomStep(-1)} shortcut="−">
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
              <IconButton disabled={!image} label="放大" onClick={() => zoomStep(1)} shortcut="+">
                <icons.ZoomIn aria-hidden="true" size={15} />
              </IconButton>
              <IconButton
                disabled={!image}
                label={zoomState === "fit" ? "原始大小" : "适应窗口"}
                onClick={() => (zoomState === "fit" ? zoomTo(1) : setFit("fit"))}
                shortcut={zoomState === "fit" ? "1" : "0"}
              >
                {zoomState === "fit" ? <span className="iv-one-to-one" aria-hidden="true">1:1</span> : <icons.Minimize2 aria-hidden="true" size={14} />}
              </IconButton>
              <span className="iv-toolbar-divider" />
              <IconButton disabled={!image} label="向右旋转" onClick={() => rotateBy(1)} shortcut="R">
                <icons.RotateCw aria-hidden="true" size={15} />
              </IconButton>
              <IconButton active={slideshow} disabled={state.entries.length < 2} label={slideshow ? "停止幻灯片" : "幻灯片放映"} onClick={() => setSlideshow((value) => !value)} shortcut="S">
                {slideshow ? <icons.Pause aria-hidden="true" size={15} /> : <icons.Play aria-hidden="true" size={15} />}
              </IconButton>
              <IconButton active={fullscreen} label={fullscreen ? "退出全屏" : "全屏"} onClick={() => void toggleFullscreen()} shortcut="F11">
                {fullscreen ? <icons.ExitFullscreen aria-hidden="true" size={15} /> : <icons.Fullscreen aria-hidden="true" size={15} />}
              </IconButton>
              <span className="iv-toolbar-divider" />
              <IconButton danger disabled={!hasImage} label="删除到回收站" onClick={requestDelete} shortcut="Delete">
                <icons.Trash2 aria-hidden="true" size={15} />
              </IconButton>
            </LiquidGlassSurface>
            ) : null}

            {toast ? (
              <div className="iv-toast" role="status">
                <span>{toast.message}</span>
                {toast.action ? (
                  <button className="iv-toast-action" onClick={() => { toast.action?.run(); setToast(null); }} type="button">{toast.action.label}</button>
                ) : null}
              </div>
            ) : null}
          </div>

          {preferences.showInfo && !immersive ? (
            <LiquidGlassSurface as="div" className="iv-info-pane" scrollable tone="panel">
              <InfoPanel entry={current} image={image} />
            </LiquidGlassSurface>
          ) : null}
        </div>

        {preferences.showFilmstrip && state.entries.length > 1 && !slideshow ? (
          <LiquidGlassSurface as="div" className="iv-filmstrip-pane" tone="chrome">
            <Filmstrip entries={state.entries} index={state.index} onSelect={session.goTo} />
          </LiquidGlassSurface>
        ) : null}
      </div>

      <AnchoredOverlay
        anchor={zoomButtonRef.current}
        ariaLabel="缩放比例"
        className="overlay-titlebar-popover"
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

      <ModalSurface
        ariaLabelledBy="image-viewer-settings-title"
        className="file-operation-dialog overlay-dialog-wide iv-settings-dialog"
        id="image-viewer-settings-dialog"
        onDismiss={() => setDialog(null)}
        open={dialog?.kind === "settings"}
        rootId="file-operation"
      >
        <OverlayDialogHeader
          description="更改会立即生效，并在下次打开时保留。"
          title="看图设置"
          titleId="image-viewer-settings-title"
        />
        <OverlayDialogBody className="iv-settings-body">
          <ViewerSettingsForm onChange={setPreferences} preferences={preferences} />
        </OverlayDialogBody>
        <OverlayDialogActions>
          <LiquidGlassButton className="overlay-dialog-button overlay-dialog-button-primary" onClick={() => setDialog(null)} tone="primary" type="button">完成</LiquidGlassButton>
        </OverlayDialogActions>
      </ModalSurface>

      <ModalSurface
        ariaLabelledBy="image-viewer-shortcuts-title"
        className="file-operation-dialog overlay-dialog-wide iv-settings-dialog"
        id="image-viewer-shortcuts-dialog"
        onDismiss={() => setDialog(null)}
        open={dialog?.kind === "shortcuts"}
        rootId="file-operation"
      >
        <OverlayDialogHeader title="键盘快捷键" titleId="image-viewer-shortcuts-title" />
        <OverlayDialogBody className="iv-settings-body">
          <ShortcutList />
        </OverlayDialogBody>
        <OverlayDialogActions>
          <LiquidGlassButton className="overlay-dialog-button overlay-dialog-button-primary" onClick={() => setDialog(null)} tone="primary" type="button">知道了</LiquidGlassButton>
        </OverlayDialogActions>
      </ModalSurface>

      {contextMenu ? (
        <AnchoredOverlay
          anchor={pointOverlayAnchor(contextMenu.x, contextMenu.y)}
          ariaLabel={menuSource === "more" ? "更多" : "图片操作"}
          className="context-menu"
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
                  onClick={() => {
                    setContextMenu(null);
                    item.onSelect?.();
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
        className="file-operation-dialog"
        id="image-viewer-delete-dialog"
        onDismiss={() => setDialog(null)}
        open={dialog?.kind === "delete"}
        role="alertdialog"
        rootId="file-operation"
      >
        <OverlayDialogHeader
          description="图片会移到回收站，需要时可以从回收站恢复。"
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
        className="file-operation-dialog"
        id="image-viewer-rename-dialog"
        initialFocusRef={renameInputRef}
        onDismiss={() => setDialog(null)}
        open={dialog?.kind === "rename"}
        rootId="file-operation"
      >
        <OverlayDialogHeader
          description={current ? `扩展名 ${current.ext} 会自动保留。` : undefined}
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
