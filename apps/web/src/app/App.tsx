import {
  Copy,
  ExternalLink,
  Eye,
  FolderInput,
  FolderOpen,
  Pencil,
  RefreshCw,
  Trash2
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FolderRecord, MediaRecord, RootRecord } from "@megle/core-client";
import type { MediaFocusCommand, NotificationWindowIntent } from "@megle/notification-contract";
import { AppShell } from "../app-shell/AppShell";
import { useOverlayScrollbars } from "../app-shell/useOverlayScrollbars";
import { useShellNotificationOverlay } from "../app-shell/useShellNotificationOverlay";
import {
  ShellOverlayHost,
  type ShellContextMenuState
} from "../app-shell/ShellOverlayHost";
import {
  LibraryTitlebarToolbar,
  PreviewTitlebarToolbar,
  ShellSidebarToggle,
  ShellTitlebarPrimaryActions,
  ShellRightActions,
} from "../app-shell/ShellTopBar";
import {
  copyText,
  getDesktopBridge,
  getDesktopShellActions,
  listLibraries,
  notifyDesktopShellReady,
  openNotificationsWindow,
  openPluginsWindow,
  openPath,
  revealPath,
  retryLibraryLoad,
  switchLibrary,
  subscribeLibraryPhase
} from "../core/desktop";
import { type LibraryLoadPhase } from "../core/libraryTypes";
import {
  configureThumbnailCache,
  prefetchOriginalPreview
} from "../core/mediaResources";
import { useLibraryData, type LibraryState } from "../core/useLibraryData";
import { LiquidGlassLayer, useInterfaceStyle } from "../design/liquid-glass";
import { useOverlayStack, type OverlayRootId } from "../design/overlays";
import { type ContextMenuItem } from "../features/file-ops/ContextMenu";
import {
  useFileOpsController,
  type FileOpsController
} from "../features/file-ops/useFileOps";
import { LibraryLoadingOverlay } from "../features/library/LibraryLoadingOverlay";
import {
  LibrarySidebar,
  type LibrarySidebarLocateRequest
} from "../features/library/LibrarySidebar";
import { LibrarySwitcher } from "../features/library/LibrarySwitcher";
import {
  LibraryCenterPane,
  LibraryInspectorPane
} from "../features/library/LibraryView";
import {
  DEFAULT_LIBRARY_LAYOUT_MODE,
  isLibraryLayoutMode,
  type LibraryLayoutMode
} from "../features/media-grid/layoutMode";
import {
  readStoredLibraryGridPreferences,
  storeLibraryGridPreferences,
  type LibraryGridPreferences
} from "../features/media-grid/gridPreferences";
import { OnboardingHero } from "../features/onboarding/OnboardingHero";
import { useNotifications } from "../features/notifications/useNotifications";
import {
  readStoredPreviewPreferences,
  storePreviewPreferences,
  type PreviewPreferences
} from "../features/preview/previewPreferences";
import { subscribePreference } from "../core/preferenceSync";
import { emitRendererStartupLog } from "../core/notificationReporting";
import { runCoordinatedMediaFocusSwitch } from "../core/mediaFocusLocateWorkflow";
import { useShortcuts } from "../features/shortcuts/useShortcuts";
import { formatPreviewTitle, resolvePreviewTitlePosition } from "./previewTitle";
import { buildPreviewPrefetchWindow } from "./previewPrefetchWindow";
import { canNavigatePreview, canStartPreviewNavigation } from "./previewNavigationGate";
import { PreviewRenderMenu } from "../features/preview/PreviewRenderMenu";
import {
  channelFilterId,
  imageRenderingForFilter
} from "../features/preview/previewRenderSettings";
import { previewViewportLongPx } from "../features/preview/MediaPreview";
import {
  configurePreviewBuffers,
  prefetchPreviewBitmap,
  previewPrefetchBudgetBytes
} from "../features/preview/previewBitmapSource";
import { selectPreviewRenderLane } from "../features/preview/previewRenderLane";
import {
  prepareMediaFocusNavigation,
  type MediaGridLocateRequest
} from "./mediaFocusWorkflow";

type PreviewViewState = { mode: "fit-long-edge" | "actual"; scale: number };
type PreviewViewCommands = { reset: () => void; toggleActualSize: () => void };

const DEFAULT_PREVIEW_VIEW_STATE: PreviewViewState = {
  mode: "fit-long-edge",
  scale: 1
};
const LIBRARY_LAYOUT_STORAGE_KEY = "megle.library.layout-mode";

export function App() {
  useOverlayScrollbars();
  const overlays = useOverlayStack();
  useEffect(() => {
    emitRendererStartupLog();
  }, []);
  const interfaceStyle = useInterfaceStyle();
  const [menu, setMenu] = useState<ShellContextMenuState | null>(null);
  const notifications = useNotifications();
  const notificationModel = notifications.compact;
  const {
    closeNotifications,
    notificationAnchor,
    notificationAutoFocus,
    notificationButtonRef,
    notificationsOpen,
    onToggleNotifications
  } = useShellNotificationOverlay();
  const [previewOpen, setPreviewOpen] = useState(false);
  const [previewDisplayedMediaId, setPreviewDisplayedMediaId] = useState<number | null>(null);
  // The media whose pixels are actually on screen. Written on EVERY paint —
  // including PreviewCanvas's error path, so a media that fails to decode
  // releases the navigation gate instead of trapping it forever.
  const paintedMediaIdRef = useRef<number | null>(null);
  const [sidebarsHidden, setSidebarsHidden] = useState(false);
  const [locateRequest, setLocateRequest] =
    useState<LibrarySidebarLocateRequest | undefined>(undefined);
  const locateRequestIdRef = useRef(0);
  const [previewViewState, setPreviewViewState] =
    useState<PreviewViewState>(DEFAULT_PREVIEW_VIEW_STATE);
  const [previewViewCommands, setPreviewViewCommands] =
    useState<PreviewViewCommands | null>(null);
  const [layoutMode, setLayoutMode] = useState<LibraryLayoutMode>(() =>
    readStoredLibraryLayoutMode()
  );
  const [gridPreferences, setGridPreferences] = useState<LibraryGridPreferences>(() =>
    readStoredLibraryGridPreferences()
  );
  const [previewPreferences, setPreviewPreferences] = useState<PreviewPreferences>(() =>
    readStoredPreviewPreferences()
  );
  const [libraryPhase, setLibraryPhase] = useState<LibraryLoadPhase>("starting-core");
  const [librarySwitcherAnchor, setLibrarySwitcherAnchor] =
    useState<HTMLButtonElement | null>(null);
  const [librarySwitcherSource, setLibrarySwitcherSource] =
    useState<HTMLButtonElement | null>(null);
  const [librarySwitcherAutoFocus, setLibrarySwitcherAutoFocus] = useState(false);
  const [activeLibraryId, setActiveLibraryId] = useState<string | null>(null);
  const [activeLibraryName, setActiveLibraryName] = useState<string>("Library");
  const [activeLibraryIcon, setActiveLibraryIcon] = useState<string | undefined>(undefined);
  const [activeLibraryColor, setActiveLibraryColor] = useState<string | undefined>(undefined);
  const [pendingMediaFocus, setPendingMediaFocus] = useState<MediaFocusCommand | null>(null);
  const pendingMediaFocusRef = useRef<MediaFocusCommand | null>(null);
  const mediaFocusAbortRef = useRef<{
    requestId: string;
    controller: AbortController;
  } | null>(null);
  const [mediaGridLocateRequest, setMediaGridLocateRequest] =
    useState<MediaGridLocateRequest | null>(null);
  const mediaGridLocateRequestRef = useRef<MediaGridLocateRequest | null>(null);
  const mediaFocusSwitchKeyRef = useRef<string | null>(null);
  const mediaFocusLocateKeyRef = useRef<string | null>(null);
  const hasLoadedActiveLibraryNameRef = useRef(false);
  const library = useLibraryData({
    activeLibraryId,
    activeLibraryName,
    persistentThumbnailCacheAutoRefresh:
      previewPreferences.persistentThumbnailCacheAutoRefresh,
    thumbnailGenerationMode: previewPreferences.thumbnailGenerationMode
  });

  // Apply preference changes broadcast from the separate settings OS window so the
  // main window updates live while that window is open. We only mirror into local
  // state here (the settings window owns persistence) — no re-broadcast, so there
  // is no echo loop.
  useEffect(() => {
    const unsubscribers = [
      subscribePreference("grid", (value) =>
        setGridPreferences(value as LibraryGridPreferences)
      ),
      subscribePreference("preview", (value) =>
        setPreviewPreferences(value as PreviewPreferences)
      ),
      subscribePreference("interfaceStyle", (value) =>
        interfaceStyle.setInterfaceStyle(value as Parameters<typeof interfaceStyle.setInterfaceStyle>[0])
      )
    ];
    return () => unsubscribers.forEach((off) => off());
  }, [interfaceStyle]);
  const fileOps = useFileOpsController(library);
  const orderedPreviewMedia = useMemo(() => {
    const orderedWindowMedia = orderedMediaSlots(library.mediaSlots);
    return orderedWindowMedia.length > 0 ? orderedWindowMedia : library.media;
  }, [library.media, library.mediaSlots]);
  const selectedMediaIndex = orderedPreviewMedia.findIndex(
    (item) => item.id === library.selectedMediaId
  );
  const previewTitlePosition = resolvePreviewTitlePosition({
    media: library.media,
    mediaSlots: library.mediaSlots,
    mediaTotalCount: library.mediaTotalCount,
    selectedMediaId: library.selectedMediaId
  });
  const canPreviewPrevious = canNavigatePreview({
    previewOpen,
    selectedMediaIndex,
    itemCount: orderedPreviewMedia.length,
    direction: "previous"
  });
  const canPreviewNext = canNavigatePreview({
    previewOpen,
    selectedMediaIndex,
    itemCount: orderedPreviewMedia.length,
    direction: "next"
  });

  useEffect(() => {
    if (!previewOpen || selectedMediaIndex < 0) return;
    const controller = new AbortController();
    const bufferBytes = previewBufferLimitMbToBytes(previewPreferences.previewBufferLimitMb);
    const viewportLongPx = previewViewportLongPx();
    // Size the window against the DECODED-frame budget, not the raw setting.
    //
    // `buildPreviewPrefetchWindow` measures candidates in compressed file
    // bytes, but the pool that actually holds them is bounded in decoded bytes
    // and only gets PREVIEW_BITMAP_POOL_BUDGET_RATIO of the setting. A decoded
    // frame is 10-50x its file size, so sizing by the full budget picked a
    // window an order of magnitude larger than the pool could hold: most of it
    // fetched, decoded, and was evicted before it could ever be painted.
    const prefetchWindow = buildPreviewPrefetchWindow(
      orderedPreviewMedia,
      selectedMediaIndex,
      previewPrefetchBudgetBytes(bufferBytes, viewportLongPx)
    );
    for (const neighbor of prefetchWindow) {
      // Still images are warmed as DECODED bitmaps — that is what makes the
      // next step paint in one tick. Animated ones keep the <img> lane, so warm
      // their compressed bytes instead; warming both would double-fetch.
      if (
        selectPreviewRenderLane({
          source: "original",
          kind: neighbor.kind,
          ext: neighbor.ext
        }) === "canvas"
      ) {
        prefetchPreviewBitmap(neighbor, { signal: controller.signal, viewportLongPx });
      } else {
        prefetchOriginalPreview(neighbor, { signal: controller.signal });
      }
    }
    return () => controller.abort();
  }, [
    orderedPreviewMedia,
    previewOpen,
    previewPreferences.previewBufferLimitMb,
    selectedMediaIndex
  ]);

  const handleClosePreview = useCallback(() => {
    setPreviewOpen(false);
    setPreviewDisplayedMediaId(null);
    setPreviewViewCommands(null);
  }, []);

  const clearMediaFocusState = useCallback((requestId: string) => {
    if (mediaFocusAbortRef.current?.requestId === requestId) {
      mediaFocusAbortRef.current.controller.abort();
      mediaFocusAbortRef.current = null;
    }
    if (pendingMediaFocusRef.current?.requestId === requestId) {
      pendingMediaFocusRef.current = null;
      setPendingMediaFocus(null);
    }
    setMediaGridLocateRequest((current) =>
      current?.requestId === requestId ? null : current
    );
    if (mediaGridLocateRequestRef.current?.requestId === requestId) {
      mediaGridLocateRequestRef.current = null;
    }
    if (mediaFocusSwitchKeyRef.current === requestId) mediaFocusSwitchKeyRef.current = null;
    if (mediaFocusLocateKeyRef.current === requestId) mediaFocusLocateKeyRef.current = null;
  }, []);

  const completeMediaFocus = useCallback((requestId: string, ok: boolean) => {
    const bridge = getDesktopBridge();
    void bridge?.completeMediaFocus?.({ requestId, ok });
    clearMediaFocusState(requestId);
  }, [clearMediaFocusState]);

  useEffect(() => {
    const bridge = getDesktopBridge();
    return bridge?.onFocusMedia?.((command) => {
      if (
        mediaFocusAbortRef.current &&
        mediaFocusAbortRef.current.requestId !== command.requestId
      ) {
        mediaFocusAbortRef.current.controller.abort();
      }
      const previous = pendingMediaFocusRef.current;
      if (previous && previous.requestId !== command.requestId) {
        void bridge.completeMediaFocus?.({ requestId: previous.requestId, ok: false });
      }
      const previousLocate = mediaGridLocateRequestRef.current;
      if (previousLocate && previousLocate.requestId !== command.requestId) {
        void bridge.completeMediaFocus?.({ requestId: previousLocate.requestId, ok: false });
      }
      pendingMediaFocusRef.current = command;
      mediaFocusAbortRef.current = {
        requestId: command.requestId,
        controller: new AbortController()
      };
      mediaGridLocateRequestRef.current = null;
      mediaFocusSwitchKeyRef.current = null;
      mediaFocusLocateKeyRef.current = null;
      setMediaGridLocateRequest(null);
      setPendingMediaFocus(command);
    });
  }, []);

  useEffect(() => {
    const bridge = getDesktopBridge();
    return bridge?.onFocusMediaCancelled?.(({ requestId }) => {
      if (mediaFocusAbortRef.current?.requestId !== requestId) return;
      clearMediaFocusState(requestId);
    });
  }, [clearMediaFocusState]);

  useEffect(() => {
    if (!pendingMediaFocus) return;
    const requestKey = pendingMediaFocus.requestId;
    const signal = mediaFocusAbortRef.current?.requestId === requestKey
      ? mediaFocusAbortRef.current.controller.signal
      : null;
    if (!signal || signal.aborted) return;
    if (activeLibraryId !== pendingMediaFocus.libraryId) {
      if (mediaFocusSwitchKeyRef.current === requestKey) return;
      mediaFocusSwitchKeyRef.current = requestKey;
      const bridge = getDesktopBridge();
      void runCoordinatedMediaFocusSwitch({
        libraryId: pendingMediaFocus.libraryId,
        requestId: requestKey,
        signal,
        switchLibrary,
        updateStage: async (update) =>
          (await bridge?.updateMediaFocusStage?.(update)) === true
      })
        .then((ok) => {
          if (signal.aborted || pendingMediaFocusRef.current?.requestId !== requestKey) return;
          if (!ok) {
            completeMediaFocus(requestKey, false);
          }
        })
        .catch(() => completeMediaFocus(requestKey, false));
      return;
    }
    if (libraryPhase !== "ready" || library.loading) return;
    mediaFocusSwitchKeyRef.current = null;
    if (mediaFocusLocateKeyRef.current === requestKey) return;
    mediaFocusLocateKeyRef.current = requestKey;
    const bridge = getDesktopBridge();
    void Promise.resolve(
      bridge?.updateMediaFocusStage?.({ requestId: requestKey, stage: "locating" }) ?? false
    )
      .then((accepted) => {
        if (signal.aborted || pendingMediaFocusRef.current?.requestId !== requestKey) return;
        if (!accepted) {
          completeMediaFocus(requestKey, false);
          return;
        }
        return prepareMediaFocusNavigation(pendingMediaFocus, {
          closePreview: handleClosePreview,
          locateMedia: (command, locateSignal) =>
            library.locateMedia(command, { signal: locateSignal })
        }, signal).then((locateRequest) => {
          if (signal.aborted || pendingMediaFocusRef.current?.requestId !== requestKey) return;
          if (!locateRequest) {
            completeMediaFocus(requestKey, false);
            return;
          }
          pendingMediaFocusRef.current = null;
          setPendingMediaFocus(null);
          mediaGridLocateRequestRef.current = locateRequest;
          setMediaGridLocateRequest(locateRequest);
        });
      })
      .catch(() => completeMediaFocus(requestKey, false));
  }, [
    activeLibraryId,
    completeMediaFocus,
    handleClosePreview,
    library.loading,
    library.locateMedia,
    libraryPhase,
    pendingMediaFocus
  ]);

  const handleMediaLocateSettled = useCallback((requestId: string, ok: boolean) => {
    completeMediaFocus(requestId, ok);
  }, [completeMediaFocus]);

  const toggleSidebars = useCallback(() => {
    setSidebarsHidden((current) => !current);
  }, []);

  const locateFolder = useCallback((folderId: number | null | undefined) => {
    if (folderId == null) {
      return;
    }
    locateRequestIdRef.current += 1;
    setSidebarsHidden(false);
    setLocateRequest({
      folderId,
      requestId: locateRequestIdRef.current
    });
  }, []);

  const locateCurrentFolder = useCallback(() => {
    const selectedRoot =
      library.roots.find((root) => root.id === library.selectedRootId) ?? null;
    const folderId =
      previewOpen && library.selectedMedia
        ? library.selectedMedia.folderId
        : library.selectedFolderId ?? selectedRoot?.rootFolderId ?? null;
    locateFolder(folderId);
  }, [
    library.roots,
    library.selectedFolderId,
    library.selectedMedia,
    library.selectedRootId,
    locateFolder,
    previewOpen
  ]);

  useEffect(() => {
    void notifyDesktopShellReady();
  }, []);

  useEffect(
    () =>
      subscribeLibraryPhase((phase) => {
        setLibraryPhase(phase);
        if (phase === "loading-media") {
          setActiveLibraryId(null);
        }
      }),
    []
  );

  // Derive the sidebar's displayed library name from the registry: fetch it on
  // mount (the initial render may predate any "ready" transition) and again
  // whenever the library phase settles to "ready" (create/switch/open all land
  // there), so the name follows the active library across switches.
  useEffect(() => {
    if (hasLoadedActiveLibraryNameRef.current && libraryPhase !== "ready") {
      return;
    }
    hasLoadedActiveLibraryNameRef.current = true;
    let cancelled = false;
    void listLibraries().then((libraries) => {
      if (cancelled) return;
      const active = libraries.find((entry) => entry.active);
      setActiveLibraryId(active?.id ?? null);
      setActiveLibraryName(active?.name ?? "Library");
      setActiveLibraryIcon(active?.icon);
      setActiveLibraryColor(active?.color);
    });
    return () => {
      cancelled = true;
    };
  }, [libraryPhase]);

  useEffect(() => {
    if (!library.loading && library.roots.length >= 0) {
      // Data settled (roots resolved, even if empty) → the loading overlay can
      // clear. Desktop cold start and library switches replay "loading-media";
      // browser-only use has no Desktop phase producer and settles from its initial
      // phase here as well.
      setLibraryPhase((p) =>
        p === "ready" || (typeof p === "object" && p.kind === "error") ? p : "ready"
      );
    }
  }, [library.loading, library.roots.length]);

  useEffect(() => {
    try {
      window.localStorage.setItem(LIBRARY_LAYOUT_STORAGE_KEY, layoutMode);
    } catch {
      // Ignore storage failures in hardened/browser-restricted environments.
    }
  }, [layoutMode]);

  useEffect(() => {
    storeLibraryGridPreferences(gridPreferences);
  }, [gridPreferences]);

  // Publish the render settings as CSS custom properties on <body>. styles.css
  // consumes them on the shared `.preview-image` class, so the canvas, <img>
  // and <video> lanes are all covered by one rule with no repaint and no
  // re-decode.
  //
  // The VALUES come from previewRenderSettings, not from CSS selectors, so the
  // mapping lives in exactly one tested place instead of being duplicated as a
  // keyword list in the stylesheet. Both defaults (`auto`, `none`) are the CSS
  // initial values, so the untouched common case costs nothing.
  useEffect(() => {
    const channel = channelFilterId(previewPreferences.previewChannel);
    document.body.style.setProperty(
      "--preview-image-rendering",
      imageRenderingForFilter(previewPreferences.previewRenderFilter)
    );
    document.body.style.setProperty(
      "--preview-channel-filter",
      channel === null ? "none" : `url(#${channel})`
    );
  }, [previewPreferences.previewChannel, previewPreferences.previewRenderFilter]);

  useEffect(() => {
    storePreviewPreferences(previewPreferences);
    configurePreviewBuffers(
      previewBufferLimitMbToBytes(previewPreferences.previewBufferLimitMb)
    );
    configureThumbnailCache(
      thumbnailCacheLimitMbToBytes(previewPreferences.thumbnailCacheLimitMb)
    );
  }, [previewPreferences]);

  const closeMenu = useCallback(() => setMenu(null), []);

  const filterMenuOpen = overlays.isRootOpen("filter");
  const sortMenuOpen = overlays.isRootOpen("sort");
  const layoutMenuOpen = overlays.isRootOpen("layout");
  const previewRenderMenuOpen = overlays.isRootOpen("preview-render");
  const librarySwitcherOpen = overlays.isRootOpen("library");

  const setRootOpen = useCallback(
    (rootId: OverlayRootId, open: boolean) => {
      if (open) overlays.openRoot(rootId);
      else overlays.closeRoot(rootId);
    },
    [overlays]
  );

  const openLibrarySwitcher = useCallback((source: HTMLButtonElement, autoFocus: boolean) => {
    setLibrarySwitcherSource(source);
    setLibrarySwitcherAutoFocus(autoFocus);
    setRootOpen("library", true);
  }, [setRootOpen]);
  const closeLibrarySwitcher = useCallback(() => {
    setRootOpen("library", false);
  }, [setRootOpen]);

  const setFilterOpen = useCallback(
    (open: boolean) => {
      setRootOpen("filter", open);
    },
    [setRootOpen]
  );

  const setSortOpen = useCallback(
    (open: boolean) => {
      setRootOpen("sort", open);
    },
    [setRootOpen]
  );

  const setLayoutOpen = useCallback(
    (open: boolean) => {
      setRootOpen("layout", open);
    },
    [setRootOpen]
  );

  const handleOpenNotificationCenter = useCallback((intent?: NotificationWindowIntent) => {
    closeNotifications();
    void openNotificationsWindow(intent);
  }, [closeNotifications]);

  const shellRightActions = (
    <ShellRightActions
      notificationButtonRef={notificationButtonRef}
      notificationIndicator={notificationModel.indicator}
      notificationsOpen={notificationsOpen}
      onToggleNotifications={onToggleNotifications}
    />
  );

  const handleOpenPreview = useCallback(
    (mediaId: number) => {
      paintedMediaIdRef.current = null;
      setPreviewDisplayedMediaId(null);
      library.setSelectedMediaId(mediaId);
      setPreviewViewCommands(null);
      setPreviewOpen(true);
    },
    [library]
  );

  function startPreviewNavigation(
    target: MediaRecord | undefined,
    direction: "previous" | "next"
  ) {
    if (!target) return;
    // Drop, do not queue. A wheel flick emits many events and a held arrow key
    // repeats far faster than a cold image can start painting; queueing would
    // bank those steps and skip images the user never saw.
    if (
      !canStartPreviewNavigation({
        previewOpen,
        selectedMediaIndex,
        itemCount: orderedPreviewMedia.length,
        direction,
        selectedMediaId: library.selectedMediaId,
        paintedMediaId: paintedMediaIdRef.current
      })
    ) {
      return;
    }
    library.setSelectedMediaId(target.id);
  }

  const handlePreviewPrevious = useCallback(() => {
    if (selectedMediaIndex <= 0) return;
    startPreviewNavigation(orderedPreviewMedia[selectedMediaIndex - 1], "previous");
  }, [library, orderedPreviewMedia, selectedMediaIndex]);

  const handlePreviewNext = useCallback(() => {
    if (selectedMediaIndex < 0 || selectedMediaIndex >= orderedPreviewMedia.length - 1) return;
    startPreviewNavigation(orderedPreviewMedia[selectedMediaIndex + 1], "next");
  }, [library, orderedPreviewMedia, selectedMediaIndex]);

  const handleScrubToIndex = useCallback(
    (index: number) => {
      // Deliberately NOT startPreviewNavigation: the first-paint gate exists to
      // stop the keyboard and wheel outrunning the display, but a drag must
      // keep the thumb and the image in agreement, so it commits immediately.
      const slotMedia = library.mediaSlots.get(index);
      if (slotMedia) {
        library.setSelectedMediaId(slotMedia.id);
        return;
      }
      // Outside the loaded window: ask for that region. The thumb and tooltip
      // have already moved; the image follows when the slot resolves. This is
      // the one place position and picture can briefly disagree, and it is
      // bounded by a single round-trip.
      library.requestMediaWindow(index, index + 1);
    },
    [library]
  );

  const handleToggleScrubberPinned = useCallback(() => {
    setPreviewPreferences((current) => ({
      ...current,
      previewScrubberPinned: !current.previewScrubberPinned
    }));
  }, []);

  const handlePreviewMediaSettled = useCallback(
    (mediaId: number) => {
      // Recorded BEFORE the early return: a paint that lands after the user
      // already moved on still proves this media reached the screen.
      paintedMediaIdRef.current = mediaId;
      if (mediaId !== library.selectedMediaId) return;
      setPreviewDisplayedMediaId(mediaId);
    },
    [library.selectedMediaId]
  );

  useShortcuts({
    fileOps,
    library,
    onClosePreview: handleClosePreview,
    onPreviewNext: handlePreviewNext,
    onPreviewPrevious: handlePreviewPrevious,
    onToggleSidebars: toggleSidebars,
    previewOpen
  });

  const handleMediaContextMenu = useCallback(
    ({
      item,
      x,
      y,
      shiftKey
    }: {
      item: MediaRecord;
      x: number;
      y: number;
      shiftKey: boolean;
    }) => {
      setMenu({
        x,
        y,
        items: buildFileItems({
          file: item,
          library,
          shiftKey,
          fileOps,
          onPreview: handleOpenPreview,
          closeMenu: () => setMenu(null)
        })
      });
    },
    [fileOps, handleOpenPreview, library]
  );

  const handleFolderContextMenu = useCallback(
    ({
      folder,
      x,
      y,
      shiftKey
    }: {
      folder: FolderRecord;
      x: number;
      y: number;
      shiftKey: boolean;
    }) => {
      setMenu({
        x,
        y,
        items: buildFolderItems({
          folder,
          library,
          shiftKey,
          fileOps,
          onOpenFolder: (target) => {
            library.setSelectedFolder(target);
          },
          onRefreshFolder: (target) => {
            library.setSelectedFolder(target);
            void library.refresh();
          },
          onRescanRoot: (rootId) => {
            void library.rescanRoot(rootId);
          },
          closeMenu: () => setMenu(null)
        })
      });
    },
    [fileOps, library]
  );

  const handleRootContextMenu = useCallback(
    ({
      root,
      x,
      y
    }: {
      root: RootRecord;
      x: number;
      y: number;
      shiftKey: boolean;
    }) => {
      setMenu({
        x,
        y,
        items: buildRootItems({
          root,
          onOpenRoot: (target) => {
            library.setSelectedRootId(target.id);
          },
          onRefresh: () => void library.refresh(),
          onRescanRoot: (rootId) => {
            void library.rescanRoot(rootId);
          },
          closeMenu: () => setMenu(null)
        })
      });
    },
    [library]
  );

  function renderCenterTitlebar() {
    const primaryActions = sidebarsHidden ? (
      <ShellSidebarToggle
        sidebarsHidden
        onToggleSidebars={toggleSidebars}
      />
    ) : undefined;
    if (previewOpen && library.selectedMedia) {
      return (
        <PreviewTitlebarToolbar
          canGoNext={canPreviewNext}
          canGoPrevious={canPreviewPrevious}
          mode={previewViewState.mode}
          scale={previewViewState.scale}
          selectedName={formatPreviewTitle(library.selectedMedia.name, previewTitlePosition)}
          shellActions={sidebarsHidden ? shellRightActions : undefined}
          sidebarsHidden={sidebarsHidden}
          onBack={handleClosePreview}
          onGoNext={handlePreviewNext}
          onGoPrevious={handlePreviewPrevious}
          primaryActions={primaryActions}
          onToggleActualSize={() => previewViewCommands?.toggleActualSize()}
          renderActions={
            <PreviewRenderMenu
              channel={previewPreferences.previewChannel}
              filter={previewPreferences.previewRenderFilter}
              onChangeChannel={(previewChannel) =>
                setPreviewPreferences((current) => ({ ...current, previewChannel }))
              }
              onChangeFilter={(previewRenderFilter) =>
                setPreviewPreferences((current) => ({ ...current, previewRenderFilter }))
              }
              onOpenChange={(open) => setRootOpen("preview-render", open)}
              open={previewRenderMenuOpen}
            />
          }
        />
      );
    }

    const selectedRoot =
      library.roots.find((root) => root.id === library.selectedRootId) ?? null;
    const selectedFolder = library.folders.find(
      (folder) => folder.id === library.selectedFolderId
    );
    return (
      <LibraryTitlebarToolbar
        canNavigateFolderBack={library.canNavigateFolderBack}
        canNavigateFolderForward={library.canNavigateFolderForward}
        favorite={library.searchState.favorite}
        filterOpen={filterMenuOpen}
        kind={library.searchState.kind}
        layoutMode={layoutMode}
        layoutOpen={layoutMenuOpen}
        mediaCount={Math.max(library.mediaTotalCount, library.media.length)}
        minRating={library.searchState.minRating}
        onClearFilters={library.clearFilters}
        onFilterOpenChange={setFilterOpen}
        onLayoutModeChange={setLayoutMode}
        onLayoutOpenChange={setLayoutOpen}
        onNavigateFolderBack={library.navigateFolderBack}
        onNavigateFolderForward={library.navigateFolderForward}
        onSetKind={library.setKind}
        onSetMinRating={library.setMinRating}
        onSetQ={library.setQ}
        onSortOpenChange={setSortOpen}
        onSetSort={library.setSort}
        onToggleFavorite={library.toggleFavoriteFilter}
        onToggleTag={library.toggleTagFilter}
        primaryActions={primaryActions}
        q={library.searchState.q}
        scanActive={library.scanActive}
        searchActive={library.searchActive}
        shellActions={sidebarsHidden ? shellRightActions : undefined}
        sidebarsHidden={sidebarsHidden}
        sort={library.searchState.sort}
        sortOpen={sortMenuOpen}
        tagIds={library.searchState.tagIds}
        tags={library.tags}
        title={selectedFolder?.name ?? selectedRoot?.displayName ?? "Library"}
      />
    );
  }

  function renderPrimaryTitlebarActions() {
    return (
      <ShellTitlebarPrimaryActions
        activeLibraryColor={activeLibraryColor}
        activeLibraryIcon={activeLibraryIcon}
        activeLibraryName={activeLibraryName}
        librarySwitcherButtonRef={setLibrarySwitcherAnchor}
        librarySwitcherOpen={librarySwitcherOpen}
        onOpenLibrarySwitcher={openLibrarySwitcher}
        onOpenPlugins={handleOpenPlugins}
        onOpenSettings={handleOpenSettings}
        onLocateFolder={locateCurrentFolder}
        onToggleSidebars={toggleSidebars}
        sidebarsHidden={sidebarsHidden}
      />
    );
  }

  function renderSidebar() {
    return (
      <LibrarySidebar
        library={library}
        locateRequest={locateRequest}
        onFolderContextMenu={handleFolderContextMenu}
        onRootContextMenu={handleRootContextMenu}
      />
    );
  }

  function renderCenterPane() {
    // During a library switch, render nothing in the center: the
    // `.workbench-column-center` already IS a frosted-glass surface, so leaving
    // it empty shows that existing glass (no stale grid, no OnboardingHero
    // flash) — do NOT stack another blurred overlay on top of it.
    if (library.switching) {
      return null;
    }
    return library.roots.length === 0 && !library.loading ? (
      <OnboardingHero
        rootCount={library.roots.length}
        loading={library.loading}
        onAddRoot={(path) => library.addRoot(path)}
      />
    ) : (
      <LibraryCenterPane
        gridPreferences={gridPreferences}
        library={library}
        layoutMode={layoutMode}
        onClosePreview={handleClosePreview}
        onFolderContextMenu={handleFolderContextMenu}
        onMediaContextMenu={handleMediaContextMenu}
        onOpenPreview={handleOpenPreview}
        onLocateFolder={locateFolder}
        mediaLocateRequest={mediaGridLocateRequest}
        onMediaLocateSettled={handleMediaLocateSettled}
        onPreviewCommandChange={setPreviewViewCommands}
        onPreviewMediaSettled={handlePreviewMediaSettled}
        onScrubTo={handleScrubToIndex}
        onToggleScrubberPinned={handleToggleScrubberPinned}
        scrubberCurrent={previewTitlePosition?.current ?? 0}
        scrubberPinned={previewPreferences.previewScrubberPinned}
        scrubberTotal={previewTitlePosition?.total ?? 0}
        onPreviewNext={handlePreviewNext}
        onPreviewPrevious={handlePreviewPrevious}
        onPreviewViewStateChange={setPreviewViewState}
        previewViewState={previewViewState}
        previewOpen={previewOpen}
        sidebarLayoutTransitionKey={sidebarsHidden ? "hidden" : "visible"}
      />
    );
  }

  function renderRightPane() {
    return <LibraryInspectorPane library={library} previewOpen={previewOpen} />;
  }

  function renderOverlays() {
    return (
      <>
        <ShellOverlayHost
          contextMenu={menu}
          fileOps={fileOps}
          library={library}
          notificationAnchor={notificationAnchor}
          notificationAutoFocus={notificationAutoFocus}
          notificationModel={notificationModel}
          notificationsOpen={notificationsOpen}
          onCloseContextMenu={closeMenu}
          onOpenNotificationCenter={handleOpenNotificationCenter}
        />
        <LibrarySwitcher
          anchor={librarySwitcherAnchor}
          autoFocus={librarySwitcherAutoFocus}
          library={library}
          notificationContext={
            activeLibraryId
              ? { libraryId: activeLibraryId, libraryName: activeLibraryName }
              : null
          }
          onClose={closeLibrarySwitcher}
          open={librarySwitcherOpen}
          restoreFocusTarget={librarySwitcherSource}
        />
      </>
    );
  }

  function handleOpenSettings() {
    void getDesktopBridge()?.openSettingsWindow?.();
  }

  function handleOpenPlugins() {
    void openPluginsWindow();
  }

  return (
    <LiquidGlassLayer>
      <AppShell
        layout="library"
        sidebarsHidden={sidebarsHidden}
        titlebarLeft={sidebarsHidden ? null : renderPrimaryTitlebarActions()}
        titlebarCenter={renderCenterTitlebar()}
        titlebarRight={sidebarsHidden ? null : shellRightActions}
        sidebar={renderSidebar()}
        centerPane={
          <>
            {renderCenterPane()}
            <LibraryLoadingOverlay
              phase={libraryPhase}
              onRetry={() => void retryLibraryLoad()}
              onSwitchLibrary={openLibrarySwitcher}
            />
          </>
        }
        rightPane={renderRightPane()}
        overlays={renderOverlays()}
      />
    </LiquidGlassLayer>
  );
}

function orderedMediaSlots(mediaSlots: Map<number, MediaRecord>): MediaRecord[] {
  return Array.from(mediaSlots.entries())
    .sort(([leftIndex], [rightIndex]) => leftIndex - rightIndex)
    .map(([, item]) => item);
}

function previewBufferLimitMbToBytes(limitMb: number): number {
  return Math.max(0, Math.round(limitMb * 1024 * 1024));
}

function thumbnailCacheLimitMbToBytes(limitMb: number): number {
  return Math.max(0, Math.round(limitMb * 1024 * 1024));
}

function readStoredLibraryLayoutMode(): LibraryLayoutMode {
  try {
    const value = window.localStorage.getItem(LIBRARY_LAYOUT_STORAGE_KEY);
    return isLibraryLayoutMode(value) ? value : DEFAULT_LIBRARY_LAYOUT_MODE;
  } catch {
    return DEFAULT_LIBRARY_LAYOUT_MODE;
  }
}

interface BuildFileItemsArgs {
  file: MediaRecord;
  library: LibraryState;
  shiftKey: boolean;
  fileOps: FileOpsController;
  onPreview: (mediaId: number) => void;
  closeMenu: () => void;
}

interface BuildFolderItemsArgs {
  folder: FolderRecord;
  library: LibraryState;
  shiftKey: boolean;
  fileOps: FileOpsController;
  onOpenFolder: (folder: FolderRecord) => void;
  onRefreshFolder: (folder: FolderRecord) => void;
  onRescanRoot: (rootId: number) => void;
  closeMenu: () => void;
}

interface BuildRootItemsArgs {
  root: RootRecord;
  onOpenRoot: (root: RootRecord) => void;
  onRefresh: () => void;
  onRescanRoot: (rootId: number) => void;
  closeMenu: () => void;
}

function buildFileItems({
  file,
  library,
  shiftKey,
  fileOps,
  onPreview,
  closeMenu
}: BuildFileItemsArgs): ContextMenuItem[] {
  const filePath = resolveMediaPath(library, file);
  const canReveal = Boolean(filePath && getDesktopShellActions());
  return [
    {
      id: "preview",
      label: "Preview",
      icon: <Eye size={14} />,
      onSelect: () => {
        closeMenu();
        onPreview(file.id);
      }
    },
    {
      id: "open",
      label: "Open",
      icon: <ExternalLink size={14} />,
      disabled: !filePath || !getDesktopShellActions(),
      onSelect: () => {
        closeMenu();
        if (filePath) void openPath(filePath);
      }
    },
    { id: "file-primary-separator", separator: true },
    {
      id: "rename",
      label: "Rename…",
      icon: <Pencil size={14} />,
      onSelect: () => {
        closeMenu();
        fileOps.openRename({ kind: "file", file });
      }
    },
    {
      id: "move",
      label: "Move to…",
      icon: <FolderInput size={14} />,
      onSelect: () => {
        closeMenu();
        fileOps.openMove({ kind: "file", file });
      }
    },
    {
      id: "delete-recycle",
      label: "Move to recycle bin",
      icon: <Trash2 size={14} />,
      onSelect: () => {
        closeMenu();
        fileOps.openDelete({ kind: "file", file }, false);
      }
    },
    {
      id: "delete-permanent",
      label: shiftKey ? "Delete permanently" : "Delete permanently…",
      danger: true,
      icon: <Trash2 size={14} />,
      onSelect: () => {
        closeMenu();
        fileOps.openDelete({ kind: "file", file }, true);
      }
    },
    { id: "file-shell-separator", separator: true },
    {
      id: "copy-path",
      label: "Copy path",
      icon: <Copy size={14} />,
      disabled: !filePath,
      onSelect: () => {
        closeMenu();
        if (filePath) void copyText(filePath);
      }
    },
    {
      id: "reveal",
      label: "Reveal in Explorer",
      icon: <FolderOpen size={14} />,
      disabled: !canReveal,
      onSelect: () => {
        closeMenu();
        if (filePath) void revealPath(filePath);
      }
    }
  ];
}

function buildFolderItems({
  folder,
  library,
  shiftKey,
  fileOps,
  onOpenFolder,
  onRefreshFolder,
  onRescanRoot,
  closeMenu
}: BuildFolderItemsArgs): ContextMenuItem[] {
  const folderPath = resolveFolderPath(library, folder);
  const canReveal = Boolean(folderPath && getDesktopShellActions());
  return [
    {
      id: "open-folder",
      label: "Open folder",
      icon: <FolderOpen size={14} />,
      onSelect: () => {
        closeMenu();
        onOpenFolder(folder);
      }
    },
    {
      id: "refresh-folder",
      label: "Refresh folder",
      icon: <RefreshCw size={14} />,
      onSelect: () => {
        closeMenu();
        onRefreshFolder(folder);
      }
    },
    {
      id: "rescan-root",
      label: "Rescan root",
      icon: <RefreshCw size={14} />,
      onSelect: () => {
        closeMenu();
        onRescanRoot(folder.rootId);
      }
    },
    { id: "folder-primary-separator", separator: true },
    {
      id: "rename",
      label: "Rename…",
      icon: <Pencil size={14} />,
      onSelect: () => {
        closeMenu();
        fileOps.openRename({ kind: "folder", folder });
      }
    },
    {
      id: "move",
      label: "Move to…",
      icon: <FolderInput size={14} />,
      onSelect: () => {
        closeMenu();
        fileOps.openMove({ kind: "folder", folder });
      }
    },
    {
      id: "delete-recycle",
      label: "Move to recycle bin",
      icon: <Trash2 size={14} />,
      onSelect: () => {
        closeMenu();
        fileOps.openDelete({ kind: "folder", folder }, false);
      }
    },
    {
      id: "delete-permanent",
      label: shiftKey ? "Delete permanently" : "Delete permanently…",
      danger: true,
      icon: <Trash2 size={14} />,
      onSelect: () => {
        closeMenu();
        fileOps.openDelete({ kind: "folder", folder }, true);
      }
    },
    { id: "folder-shell-separator", separator: true },
    {
      id: "copy-path",
      label: "Copy path",
      icon: <Copy size={14} />,
      disabled: !folderPath,
      onSelect: () => {
        closeMenu();
        if (folderPath) void copyText(folderPath);
      }
    },
    {
      id: "reveal",
      label: "Reveal in Explorer",
      icon: <FolderOpen size={14} />,
      disabled: !canReveal,
      onSelect: () => {
        closeMenu();
        if (folderPath) void revealPath(folderPath);
      }
    }
  ];
}

function buildRootItems({
  root,
  onOpenRoot,
  onRefresh,
  onRescanRoot,
  closeMenu
}: BuildRootItemsArgs): ContextMenuItem[] {
  const canReveal = Boolean(root.path && getDesktopShellActions());
  return [
    {
      id: "open-root",
      label: "Open root",
      icon: <FolderOpen size={14} />,
      onSelect: () => {
        closeMenu();
        onOpenRoot(root);
      }
    },
    {
      id: "refresh-root",
      label: "Refresh folder",
      icon: <RefreshCw size={14} />,
      onSelect: () => {
        closeMenu();
        onRefresh();
      }
    },
    {
      id: "rescan-root",
      label: "Rescan root",
      icon: <RefreshCw size={14} />,
      onSelect: () => {
        closeMenu();
        onRescanRoot(root.id);
      }
    },
    { id: "root-management-separator", separator: true },
    {
      id: "rename-root",
      label: "Rename root",
      icon: <Pencil size={14} />,
      disabled: true,
      onSelect: () => {}
    },
    {
      id: "move-root",
      label: "Move root",
      icon: <FolderInput size={14} />,
      disabled: true,
      onSelect: () => {}
    },
    {
      id: "delete-root",
      label: "Remove root",
      icon: <Trash2 size={14} />,
      disabled: true,
      onSelect: () => {}
    },
    { id: "root-shell-separator", separator: true },
    {
      id: "copy-path",
      label: "Copy path",
      icon: <Copy size={14} />,
      onSelect: () => {
        closeMenu();
        void copyText(root.path);
      }
    },
    {
      id: "reveal",
      label: "Reveal in Explorer",
      icon: <FolderOpen size={14} />,
      disabled: !canReveal,
      onSelect: () => {
        closeMenu();
        void revealPath(root.path);
      }
    }
  ];
}

function resolveMediaPath(library: LibraryState, file: MediaRecord): string | null {
  const folderPath = resolveFolderPathById(library, file.rootId, file.folderId);
  return folderPath ? joinPath(folderPath, file.name) : null;
}

function resolveFolderPath(library: LibraryState, folder: FolderRecord): string | null {
  return resolveFolderPathById(library, folder.rootId, folder.id);
}

function resolveFolderPathById(
  library: LibraryState,
  rootId: number,
  folderId: number
): string | null {
  const root = library.roots.find((item) => item.id === rootId);
  if (!root) return null;
  if (root.rootFolderId === folderId) return root.path;

  const foldersById = new Map(library.folders.map((folder) => [folder.id, folder]));
  const segments: string[] = [];
  let current = foldersById.get(folderId) ?? null;
  let depth = 0;

  while (current && current.id !== root.rootFolderId && depth < 128) {
    segments.unshift(current.name);
    if (current.parentId === null || current.parentId === root.rootFolderId) break;
    current = foldersById.get(current.parentId) ?? null;
    depth += 1;
  }

  if (segments.length === 0 && folderId !== root.rootFolderId) {
    return null;
  }

  return segments.reduce((path, segment) => joinPath(path, segment), root.path);
}

function joinPath(base: string, segment: string): string {
  const separator = base.includes("\\") ? "\\" : "/";
  return `${base.replace(/[\\/]+$/, "")}${separator}${segment.replace(/^[\\/]+/, "")}`;
}

export function PlaceholderView({ title, detail }: { title: string; detail: string }) {
  return (
    <section className="workspace simple-workspace" aria-label={`${title} workbench`}>
      <header className="toolbar">
        <div>
          <div className="toolbar-title">{title}</div>
          <div className="toolbar-meta">{detail}</div>
        </div>
      </header>
    </section>
  );
}
