import { useActiveLibraryIdentity } from "../core/useActiveLibraryIdentity";
import {
  Copy,
  ExternalLink,
  Eye,
  FolderInput,
  FolderOpen,
  Pencil,
  RefreshCw,
  Trash2
} from "../design/icons";
import { Image as ImageIcon } from "../design/icons";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FolderRecord, MediaRecord, RootRecord } from "@mangoo/core-client";
import type { MediaFocusCommand, NotificationWindowIntent } from "@mangoo/notification-contract";
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
import { FolderAppearanceEditor } from "../features/library/FolderAppearanceEditor";
import { DirectorySortMenu } from "../features/library/DirectorySortMenu";
import { LibraryLoadingOverlay } from "../features/library/LibraryLoadingOverlay";
import {
  LibrarySidebar,
  type LibrarySidebarLocateRequest
} from "../features/library/LibrarySidebar";
import { LibrarySwitcher } from "../features/library/LibrarySwitcher";
import { usePluginContributionViews, usePluginViewLauncher } from "../features/plugins/PluginContributionsSlot";
import { useHostCapability } from "../features/plugins/hostPlugins";
import { openInImageViewer } from "../core/desktop";
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
import {
  beginFileOperationOverlay,
  useShortcuts
} from "../features/shortcuts/useShortcuts";
import { formatPreviewTitle, resolvePreviewTitlePosition } from "./previewTitle";
import { buildPreviewPrefetchWindow } from "./previewPrefetchWindow";
import {
  previewLandingWindow,
  resolvePendingPreviewSelection,
  type PendingPreviewSelection,
  resolvePreviewMediaWindow
} from "./previewMediaWindow";
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
const LIBRARY_LAYOUT_STORAGE_KEY = "mangoo.library.layout-mode";

export function App() {
  useOverlayScrollbars();
  const rootContributions = usePluginContributionViews("root-menu");
  // Host-engine image viewer: Library images can open in its standalone window.
  const imageViewerAvailable = useHostCapability("image-viewer.v1");
  const pluginView = usePluginViewLauncher();
  const overlays = useOverlayStack();
  useEffect(() => {
    emitRendererStartupLog();
  }, []);
  const interfaceStyle = useInterfaceStyle();
  const [menu, setMenu] = useState<ShellContextMenuState | null>(null);
  const contextMenuReturnFocusRef = useRef<HTMLElement | null>(null);
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
  // Latest preview index requested outside the loaded window; selected on arrival.
  const pendingPreviewSelectionRef = useRef<PendingPreviewSelection | null>(null);
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
  const { activeLibraryId, activeLibraryName, activeLibraryIcon, activeLibraryColor } = useActiveLibraryIdentity(libraryPhase);
  const [folderAppearance, setFolderAppearance] = useState<{
    folder: Pick<FolderRecord, "id" | "name" | "icon" | "color">; x: number; y: number; apply: LibraryState["updateFolderAppearance"];
  } | null>(null);
  const folderAppearanceFocusRef = useRef<HTMLElement | null>(null);
  useEffect(() => { setFolderAppearance(null); }, [activeLibraryId]);
  useEffect(() => {
    if (libraryPhase === "starting-core" || libraryPhase === "opening-library") setFolderAppearance(null);
  }, [libraryPhase]);
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
  const previewMediaWindow = useMemo(
    () => resolvePreviewMediaWindow(library.mediaSlots, library.selectedMediaId),
    [library.mediaSlots, library.selectedMediaId]
  );
  const orderedPreviewMedia = previewMediaWindow.loadedMedia;
  const selectedMediaIndex = previewMediaWindow.selectedAbsoluteIndex;
  const previewTitlePosition = resolvePreviewTitlePosition({
    media: library.media,
    mediaSlots: library.mediaSlots,
    mediaTotalCount: library.mediaTotalCount,
    selectedMediaId: library.selectedMediaId
  });
  // The number of images the FOLDER holds, not the number currently loaded.
  //
  // Bounding navigation on the loaded slot count was a dead end: in a 220-image
  // folder the preview refused to advance past 124 — the edge of what the grid
  // happened to have fetched — while the title and scrubber correctly showed
  // "124/220". MediaGrid already bounds this way (`Math.max(items.length,
  // totalCount)`); the preview did not, and nothing asked for the next window.
  const previewItemCount = library.mediaTotalCount;
  const canPreviewPrevious = canNavigatePreview({
    previewOpen,
    selectedMediaIndex,
    itemCount: previewItemCount,
    direction: "previous"
  });
  const canPreviewNext = canNavigatePreview({
    previewOpen,
    selectedMediaIndex,
    itemCount: previewItemCount,
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
      previewMediaWindow.selectedLoadedIndex,
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
          ext: neighbor.ext,
          presentation: neighbor.presentation
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
    previewMediaWindow.selectedLoadedIndex
  ]);

  const handleClosePreview = useCallback(() => {
    pendingPreviewSelectionRef.current = null;
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
      }),
    []
  );

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

  const dismissContextMenuState = useCallback(() => {
    setMenu(null);
    contextMenuReturnFocusRef.current = null;
  }, []);
  const closeContextMenu = useCallback(() => {
    setMenu(null);
    overlays.closeRoot("context-menu");
    contextMenuReturnFocusRef.current = null;
  }, [overlays]);
  const openContextMenu = useCallback(
    (nextMenu: ShellContextMenuState) => {
      const restoreFocusTarget =
        document.activeElement instanceof HTMLElement ? document.activeElement : null;
      overlays.openRoot("context-menu");
      contextMenuReturnFocusRef.current = restoreFocusTarget;
      setMenu(nextMenu);
    },
    [overlays],
  );
  const beginFileOperation = useCallback(
    (openOperation: () => void, restoreFocusTarget?: HTMLElement | null) =>
      beginFileOperationOverlay({
        fileOperationRestoreFocusRef: contextMenuReturnFocusRef,
        onDismissContextMenu: dismissContextMenuState,
        openOperation,
        overlays,
        restoreFocusTarget,
      }),
    [dismissContextMenuState, overlays],
  );
  const fileOperationOpen = Boolean(
    fileOps.rename.target || fileOps.move.target || fileOps.remove.target,
  );
  const previousFileOperationOpenRef = useRef(false);

  useEffect(() => {
    if (previousFileOperationOpenRef.current && !fileOperationOpen) {
      overlays.closeRoot("file-operation");
      const restoreFocusOwner = contextMenuReturnFocusRef.current;
      const clearFrame = window.requestAnimationFrame(() => {
        if (
          !fileOperationOpen &&
          contextMenuReturnFocusRef.current === restoreFocusOwner
        ) {
          contextMenuReturnFocusRef.current = null;
        }
      });
      previousFileOperationOpenRef.current = fileOperationOpen;
      return () => window.cancelAnimationFrame(clearFrame);
    }
    previousFileOperationOpenRef.current = fileOperationOpen;
  }, [fileOperationOpen, overlays]);

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
        itemCount: previewItemCount,
        direction,
        selectedMediaId: library.selectedMediaId,
        paintedMediaId: paintedMediaIdRef.current
      })
    ) {
      return;
    }
    library.setSelectedMediaId(target.id);
  }

  // The preview window is the single loader for every preview-side consumer.
  // Callers receive a loaded slot immediately or leave one absolute index to
  // be selected by the arrival effect below.
  const ensurePreviewMediaAtIndex = useCallback(
    (index: number): MediaRecord | undefined => {
      if (index < 0 || index >= library.mediaTotalCount) return undefined;
      const loaded = library.mediaSlots.get(index);
      if (loaded) {
        pendingPreviewSelectionRef.current = null;
        return loaded;
      }
      pendingPreviewSelectionRef.current = {
        index,
        mediaGeneration: library.mediaGeneration
      };
      const [start, end] = previewLandingWindow(index, library.mediaTotalCount);
      library.requestMediaWindow(start, end);
      return undefined;
    },
    [library]
  );

  // Steps to an adjacent index. When that index is inside the loaded window the
  // media is already in hand; when it is past the edge there is nothing to
  // select yet, so request that window and let the arriving slot be selected by
  // the effect below. Previously this returned early and navigation dead-ended.
  const stepPreview = useCallback(
    (direction: "previous" | "next") => {
      if (selectedMediaIndex < 0) return;
      const targetIndex = selectedMediaIndex + (direction === "next" ? 1 : -1);
      if (targetIndex < 0 || targetIndex > previewItemCount - 1) return;
      const target = ensurePreviewMediaAtIndex(targetIndex);
      if (target) {
        startPreviewNavigation(target, direction);
      }
    },
    [ensurePreviewMediaAtIndex, previewItemCount, selectedMediaIndex]
  );

  // Completes the latest step or scrub that crossed the loaded edge.
  useEffect(() => {
    const resolution = resolvePendingPreviewSelection({
      pending: pendingPreviewSelectionRef.current,
      previewOpen,
      mediaGeneration: library.mediaGeneration,
      mediaSlots: library.mediaSlots
    });
    if (resolution.status === "discard") {
      pendingPreviewSelectionRef.current = null;
      return;
    }
    if (resolution.status !== "apply") return;
    pendingPreviewSelectionRef.current = null;
    library.setSelectedMediaId(resolution.media.id);
  }, [library, previewOpen]);

  const handlePreviewPrevious = useCallback(() => {
    stepPreview("previous");
  }, [stepPreview]);

  const handlePreviewNext = useCallback(() => {
    stepPreview("next");
  }, [stepPreview]);

  const handleScrubToIndex = useCallback(
    (index: number) => {
      // Deliberately NOT startPreviewNavigation: the first-paint gate exists to
      // stop the keyboard and wheel outrunning the display, but a drag must
      // keep the thumb and the image in agreement, so it commits immediately.
      const slotMedia = ensurePreviewMediaAtIndex(index);
      if (slotMedia) {
        library.setSelectedMediaId(slotMedia.id);
      }
    },
    [ensurePreviewMediaAtIndex, library]
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
    fileOperationRestoreFocusRef: contextMenuReturnFocusRef,
    fileOps,
    library,
    onClosePreview: handleClosePreview,
    onPreviewNext: handlePreviewNext,
    onPreviewPrevious: handlePreviewPrevious,
    onDismissContextMenu: dismissContextMenuState,
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
      openContextMenu({
        x,
        y,
        items: buildFileItems({
          file: item,
          library,
          shiftKey,
          fileOps,
          onPreview: handleOpenPreview,
          beginFileOperation,
          closeMenu: closeContextMenu,
          imageViewerAvailable,
        })
      });
    },
    [beginFileOperation, closeContextMenu, fileOps, handleOpenPreview, imageViewerAvailable, library, openContextMenu]
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
      openContextMenu({
        x,
        y,
        items: buildFolderItems({
          folder,
          library,
          shiftKey,
          fileOps,
          onEditAppearance: (target) => {
            folderAppearanceFocusRef.current = contextMenuReturnFocusRef.current;
            closeContextMenu();
            setFolderAppearance({ folder: target, x, y, apply: library.updateFolderAppearance });
          },
          sortMenu: { id: "folder-sort", label: "排序", renderSubmenu: (anchor, onClose) =>
            <DirectorySortMenu anchor={anchor} onClose={onClose} library={library} target={{ kind: "folder", id: folder.id }} /> },
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
          beginFileOperation,
          closeMenu: closeContextMenu,
        })
      });
    },
    [beginFileOperation, closeContextMenu, fileOps, library, openContextMenu]
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
      openContextMenu({
        x,
        y,
        items: [...buildRootItems({
          root,
          onEditAppearance: (target) => {
            folderAppearanceFocusRef.current = contextMenuReturnFocusRef.current;
            closeContextMenu();
            setFolderAppearance({ folder: { id: target.id, name: target.displayName, icon: target.icon, color: target.color }, x, y, apply: library.updateRootAppearance });
          },
          sortMenu: { id: "root-sort", label: "排序", renderSubmenu: (anchor, onClose) =>
            <DirectorySortMenu anchor={anchor} onClose={onClose} library={library} target={{ kind: "root", id: root.id }} /> },
          onOpenRoot: (target) => {
            library.setSelectedRootId(target.id);
          },
          onRefresh: () => void library.refresh(),
          onRescanRoot: (rootId) => {
            void library.rescanRoot(rootId);
          },
          closeMenu: closeContextMenu,
        }), ...rootContributions.map(({ plugin, view }) => ({ id: `plugin:${plugin.pluginId}:${view.id}`, label: view.title, onSelect: () => { pluginView.open({ plugin, viewId: view.id, libraryUuid: activeLibraryId, rootId: root.id, rootUuid: null }); } }))]
      });
    },
    [closeContextMenu, library, openContextMenu, rootContributions, pluginView, activeLibraryId]
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
        {folderAppearance ? <FolderAppearanceEditor
          anchor={{ getBoundingClientRect: () => ({ left: folderAppearance.x, top: folderAppearance.y, right: folderAppearance.x, bottom: folderAppearance.y, width: 0, height: 0 }) }}
          folder={folderAppearance.folder} id="folder-appearance-editor" open rootId="context-menu" restoreFocusRef={folderAppearanceFocusRef}
          onApply={(icon, color) => folderAppearance.apply(folderAppearance.folder.id, { icon, color })}
          onClose={() => setFolderAppearance(null)}
        /> : null}
        <ShellOverlayHost
          contextMenu={menu}
          contextMenuReturnFocusRef={contextMenuReturnFocusRef}
          fileOps={fileOps}
          library={library}
          notificationAnchor={notificationAnchor}
          notificationAutoFocus={notificationAutoFocus}
          notificationModel={notificationModel}
          notificationsOpen={notificationsOpen}
          onCloseNotifications={closeNotifications}
          onDismissContextMenu={dismissContextMenuState}
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
        overlays={<>{renderOverlays()}{pluginView.view}</>}
      />
    </LiquidGlassLayer>
  );
}

// How many neighbours to pull in around a preview landing.
//
// A landing used to request exactly ONE item, which stranded the user on an
// island: after scrubbing far ahead you could step back into the previously
// loaded region but forward died immediately, because nothing after the landing
// slot existed. Requesting a span around the target means a landing always
// arrives with room to keep moving. MediaGrid does the equivalent with a row's
// worth of columns.
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
  beginFileOperation: (openOperation: () => void) => void;
  closeMenu: () => void;
  /** The image viewer plugin is enabled (Host-engine capability image-viewer.v1). */
  imageViewerAvailable?: boolean;
}

interface BuildFolderItemsArgs {
  folder: FolderRecord;
  library: LibraryState;
  shiftKey: boolean;
  fileOps: FileOpsController;
  onEditAppearance: (folder: FolderRecord) => void;
  sortMenu: ContextMenuItem;
  onOpenFolder: (folder: FolderRecord) => void;
  onRefreshFolder: (folder: FolderRecord) => void;
  onRescanRoot: (rootId: number) => void;
  beginFileOperation: (openOperation: () => void) => void;
  closeMenu: () => void;
}

interface BuildRootItemsArgs {
  root: RootRecord;
  onEditAppearance: (root: RootRecord) => void;
  sortMenu: ContextMenuItem;
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
  beginFileOperation,
  closeMenu,
  imageViewerAvailable = false
}: BuildFileItemsArgs): ContextMenuItem[] {
  const filePath = resolveMediaPath(library, file);
  const canReveal = Boolean(filePath && getDesktopShellActions());
  const viewerItems: ContextMenuItem[] = imageViewerAvailable && file.kind === "image" && filePath
    ? [{
        id: "open-image-viewer",
        label: "Open in Mangoo 看图",
        icon: <ImageIcon size={14} />,
        onSelect: () => {
          closeMenu();
          void openInImageViewer([filePath]);
        }
      }]
    : [];
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
    ...viewerItems,
    { id: "file-primary-separator", separator: true },
    {
      id: "rename",
      label: "Rename…",
      icon: <Pencil size={14} />,
      onSelect: () => {
        beginFileOperation(() => fileOps.openRename({ kind: "file", file }));
      }
    },
    {
      id: "move",
      label: "Move to…",
      icon: <FolderInput size={14} />,
      onSelect: () => {
        beginFileOperation(() => fileOps.openMove({ kind: "file", file }));
      }
    },
    {
      id: "delete-recycle",
      label: "Move to recycle bin",
      icon: <Trash2 size={14} />,
      onSelect: () => {
        beginFileOperation(() => fileOps.openDelete({ kind: "file", file }, false));
      }
    },
    {
      id: "delete-permanent",
      label: shiftKey ? "Delete permanently" : "Delete permanently…",
      danger: true,
      icon: <Trash2 size={14} />,
      onSelect: () => {
        beginFileOperation(() => fileOps.openDelete({ kind: "file", file }, true));
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
  onEditAppearance,
  sortMenu,
  onRefreshFolder,
  onRescanRoot,
  beginFileOperation,
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
      id: "folder-appearance",
      label: "图标与颜色…",
      icon: <Pencil size={14} />,
      disabled: folder.parentId === null,
      onSelect: () => onEditAppearance(folder)
    },
    {
      ...sortMenu
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
        beginFileOperation(() => fileOps.openRename({ kind: "folder", folder }));
      }
    },
    {
      id: "move",
      label: "Move to…",
      icon: <FolderInput size={14} />,
      onSelect: () => {
        beginFileOperation(() => fileOps.openMove({ kind: "folder", folder }));
      }
    },
    {
      id: "delete-recycle",
      label: "Move to recycle bin",
      icon: <Trash2 size={14} />,
      onSelect: () => {
        beginFileOperation(() => fileOps.openDelete({ kind: "folder", folder }, false));
      }
    },
    {
      id: "delete-permanent",
      label: shiftKey ? "Delete permanently" : "Delete permanently…",
      danger: true,
      icon: <Trash2 size={14} />,
      onSelect: () => {
        beginFileOperation(() => fileOps.openDelete({ kind: "folder", folder }, true));
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
  onEditAppearance,
  sortMenu,
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
      id: "root-appearance",
      label: "图标与颜色…",
      icon: <Pencil size={14} />,
      onSelect: () => onEditAppearance(root)
    },
    sortMenu,
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
