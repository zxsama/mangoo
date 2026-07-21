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
import { AppShell } from "../app-shell/AppShell";
import { useOverlayScrollbars } from "../app-shell/useOverlayScrollbars";
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
  subscribeLibraryPhase
} from "../core/desktop";
import { type LibraryLoadPhase } from "../core/libraryTypes";
import {
  configureOriginalPreviewBuffer,
  configureThumbnailCache,
  prefetchOriginalPreview
} from "../core/mediaResources";
import { useLibraryData, type LibraryState } from "../core/useLibraryData";
import { LiquidGlassLayer, useInterfaceStyle } from "../design/liquid-glass";
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
import {
  shouldRequestActiveLibraryThumbnailRefresh
} from "../features/notifications/notificationModel";
import { useNotifications } from "../features/notifications/useNotifications";
import {
  readStoredPreviewPreferences,
  storePreviewPreferences,
  type PreviewPreferences
} from "../features/preview/previewPreferences";
import { subscribePreference } from "../core/preferenceSync";
import { useShortcuts } from "../features/shortcuts/useShortcuts";
import { formatPreviewTitle, resolvePreviewTitlePosition } from "./previewTitle";

type CompactPopover = "notifications" | "filter" | "sort" | null;
type PreviewViewState = { mode: "fit-long-edge" | "actual"; scale: number };
type PreviewViewCommands = { reset: () => void; toggleActualSize: () => void };

const DEFAULT_PREVIEW_VIEW_STATE: PreviewViewState = {
  mode: "fit-long-edge",
  scale: 1
};
const PREVIEW_PREFETCH_UNKNOWN_MEDIA_BYTES = 32 * 1024 * 1024;
const MAX_PREVIEW_PREFETCH_CANDIDATES = 512;
const LIBRARY_LAYOUT_STORAGE_KEY = "megle.library.layout-mode";

export function App() {
  useOverlayScrollbars();
  const interfaceStyle = useInterfaceStyle();
  const [menu, setMenu] = useState<ShellContextMenuState | null>(null);
  const [activeCompactPopover, setActiveCompactPopover] =
    useState<CompactPopover>(null);
  const notifications = useNotifications();
  const notificationModel = notifications.compact;
  const [notificationAnchor, setNotificationAnchor] = useState<HTMLButtonElement | null>(null);
  const notificationThumbnailRefreshRequestedRef = useRef<string | null>(null);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [previewDisplayedMediaId, setPreviewDisplayedMediaId] = useState<number | null>(null);
  const previewNavigationLockedRef = useRef(false);
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
  const [switcherOpen, setSwitcherOpen] = useState(false);
  const [activeLibraryId, setActiveLibraryId] = useState<string | null>(null);
  const [activeLibraryName, setActiveLibraryName] = useState<string>("Library");
  const [activeLibraryIcon, setActiveLibraryIcon] = useState<string | undefined>(undefined);
  const [activeLibraryColor, setActiveLibraryColor] = useState<string | undefined>(undefined);
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
  const previewNavigationReady =
    !previewOpen ||
    (library.selectedMediaId !== null && previewDisplayedMediaId === library.selectedMediaId);
  const canPreviewPrevious = previewNavigationReady && selectedMediaIndex > 0;
  const canPreviewNext =
    previewNavigationReady &&
    selectedMediaIndex >= 0 &&
    selectedMediaIndex < orderedPreviewMedia.length - 1;

  useEffect(() => {
    if (!previewOpen || selectedMediaIndex < 0) return;
    const controller = new AbortController();
    const bufferBytes = previewBufferLimitMbToBytes(previewPreferences.previewBufferLimitMb);
    const prefetchWindow = buildPreviewPrefetchWindow(
      orderedPreviewMedia,
      selectedMediaIndex,
      bufferBytes
    );
    for (const neighbor of prefetchWindow) {
      prefetchOriginalPreview(neighbor, { signal: controller.signal });
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
    previewNavigationLockedRef.current = false;
    setPreviewViewCommands(null);
  }, []);

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
      // clear. Advance to "ready" from ANY non-error, non-ready phase: on a library
      // switch this comes from "loading-media", but on COLD START no phase event is
      // ever pushed (the initial Core boot uses the normal startup path, not the
      // switch path), so the phase would otherwise stay stuck at "starting-core".
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

  useEffect(() => {
    storePreviewPreferences(previewPreferences);
    configureOriginalPreviewBuffer(
      previewBufferLimitMbToBytes(previewPreferences.previewBufferLimitMb)
    );
    configureThumbnailCache(
      thumbnailCacheLimitMbToBytes(previewPreferences.thumbnailCacheLimitMb)
    );
  }, [previewPreferences]);

  const closeMenu = useCallback(() => setMenu(null), []);

  useEffect(() => {
    if (!activeCompactPopover) return;

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      event.preventDefault();
      setActiveCompactPopover(null);
    }

    function handlePointerDown(event: PointerEvent) {
      const target = event.target;
      if (!(target instanceof Element)) return;
      if (
        target.closest(
          "[data-compact-popover-root], [data-compact-popover-trigger]"
        )
      ) {
        return;
      }
      setActiveCompactPopover(null);
    }

    function handleWindowInactive() {
      setActiveCompactPopover(null);
    }

    function handleVisibilityChange() {
      if (document.visibilityState !== "visible") handleWindowInactive();
    }

    document.addEventListener("keydown", handleKeyDown);
    document.addEventListener("pointerdown", handlePointerDown, true);
    document.addEventListener("visibilitychange", handleVisibilityChange);
    window.addEventListener("blur", handleWindowInactive);
    window.addEventListener("pagehide", handleWindowInactive);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      document.removeEventListener("pointerdown", handlePointerDown, true);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
      window.removeEventListener("blur", handleWindowInactive);
      window.removeEventListener("pagehide", handleWindowInactive);
    };
  }, [activeCompactPopover]);

  const setCompactPopover = useCallback((next: CompactPopover) => {
    setActiveCompactPopover(next);
  }, []);

  const toggleCompactPopover = useCallback(
    (next: Exclude<CompactPopover, null>) => {
      setActiveCompactPopover((current) => (current === next ? null : next));
    },
    []
  );

  const setFilterOpen = useCallback(
    (open: boolean) => {
      setCompactPopover(open ? "filter" : null);
    },
    [setCompactPopover]
  );

  const setSortOpen = useCallback(
    (open: boolean) => {
      setCompactPopover(open ? "sort" : null);
    },
    [setCompactPopover]
  );

  const notificationsOpen = activeCompactPopover === "notifications";
  const filterMenuOpen = activeCompactPopover === "filter";
  const sortMenuOpen = activeCompactPopover === "sort";

  useEffect(() => {
    if (!notificationsOpen) {
      notificationThumbnailRefreshRequestedRef.current = null;
      return;
    }
    if (!shouldRequestActiveLibraryThumbnailRefresh({
      activeLibraryId,
      summaries: notificationModel.thumbnail.summaries,
      statsLibraryId: library.thumbnailCacheStatsLibraryId,
      requestedLibraryId: notificationThumbnailRefreshRequestedRef.current
    })) {
      return;
    }
    const requestedLibraryId = activeLibraryId;
    if (!requestedLibraryId) return;
    notificationThumbnailRefreshRequestedRef.current = requestedLibraryId;
    queueMicrotask(() => {
      if (notificationThumbnailRefreshRequestedRef.current !== requestedLibraryId) return;
      void library.refreshThumbnailCacheStats();
    });
  }, [
    activeLibraryId,
    library.refreshThumbnailCacheStats,
    library.thumbnailCacheStatsLibraryId,
    notificationModel.thumbnail.summaries,
    notificationsOpen
  ]);

  const onToggleNotifications = useCallback(() => {
    toggleCompactPopover("notifications");
  }, [toggleCompactPopover]);

  const handleOpenNotificationCenter = useCallback(() => {
    setActiveCompactPopover(null);
    void openNotificationsWindow();
  }, []);

  const shellRightActions = (
    <ShellRightActions
      notificationButtonRef={setNotificationAnchor}
      notificationIndicator={notificationModel.indicator}
      notificationsOpen={notificationsOpen}
      onToggleNotifications={onToggleNotifications}
    />
  );

  const handleOpenPreview = useCallback(
    (mediaId: number) => {
      previewNavigationLockedRef.current = true;
      setPreviewDisplayedMediaId(null);
      library.setSelectedMediaId(mediaId);
      setPreviewViewCommands(null);
      setPreviewOpen(true);
    },
    [library]
  );

  function startPreviewNavigation(target: MediaRecord | undefined) {
    if (!target) return;
    if (previewOpen) {
      if (previewNavigationLockedRef.current || !previewNavigationReady) return;
      previewNavigationLockedRef.current = true;
      setPreviewDisplayedMediaId(null);
    }
    library.setSelectedMediaId(target.id);
  }

  const handlePreviewPrevious = useCallback(() => {
    if (selectedMediaIndex <= 0) return;
    const previous = orderedPreviewMedia[selectedMediaIndex - 1];
    startPreviewNavigation(previous);
  }, [library, orderedPreviewMedia, previewNavigationReady, previewOpen, selectedMediaIndex]);

  const handlePreviewNext = useCallback(() => {
    if (selectedMediaIndex < 0 || selectedMediaIndex >= orderedPreviewMedia.length - 1) return;
    const next = orderedPreviewMedia[selectedMediaIndex + 1];
    startPreviewNavigation(next);
  }, [library, orderedPreviewMedia, previewNavigationReady, previewOpen, selectedMediaIndex]);

  const handlePreviewMediaSettled = useCallback(
    (mediaId: number) => {
      if (mediaId !== library.selectedMediaId) return;
      setPreviewDisplayedMediaId(mediaId);
      previewNavigationLockedRef.current = false;
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
        mediaCount={Math.max(library.mediaTotalCount, library.media.length)}
        minRating={library.searchState.minRating}
        onClearFilters={library.clearFilters}
        onFilterOpenChange={setFilterOpen}
        onLayoutModeChange={setLayoutMode}
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
        onOpenLibrarySwitcher={() => setSwitcherOpen(true)}
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
        onPreviewCommandChange={setPreviewViewCommands}
        onPreviewMediaSettled={handlePreviewMediaSettled}
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
          notificationModel={notificationModel}
          notificationsOpen={notificationsOpen}
          onCloseContextMenu={closeMenu}
          onOpenNotificationCenter={handleOpenNotificationCenter}
        />
        <LibrarySwitcher
          open={switcherOpen}
          onClose={() => setSwitcherOpen(false)}
          library={library}
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
              onRetry={() => window.location.reload()}
              onSwitchLibrary={() => setSwitcherOpen(true)}
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

function buildPreviewPrefetchWindow(
  media: MediaRecord[],
  selectedIndex: number,
  bufferBytes: number
): MediaRecord[] {
  if (bufferBytes <= 0) {
    return [];
  }

  const windowItems: MediaRecord[] = [];
  let estimatedBytes = 0;
  for (
    let offset = 1;
    selectedIndex + offset < media.length && windowItems.length < MAX_PREVIEW_PREFETCH_CANDIDATES;
    offset += 1
  ) {
    const neighbor = media[selectedIndex + offset];
    if (!neighbor || neighbor.kind !== "image") {
      continue;
    }
    const neighborBytes =
      neighbor.size > 0 ? neighbor.size : PREVIEW_PREFETCH_UNKNOWN_MEDIA_BYTES;
    if (windowItems.length > 0 && estimatedBytes + neighborBytes > bufferBytes) {
      break;
    }
    windowItems.push(neighbor);
    estimatedBytes += neighborBytes;
    if (estimatedBytes >= bufferBytes) {
      break;
    }
  }
  return windowItems;
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
