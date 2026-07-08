import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  FileOperationRecord,
  FolderRecord,
  MediaRecord,
  RootRecord,
  ScanSummary,
  SearchParams,
  TagRecord,
  TaskRecord,
  ThumbnailCacheRefreshMode,
  ThumbnailCacheStatsResponse,
  ThumbnailResponse,
  UserMetadataRecord,
  UserMetadataUpdate
} from "@megle/core-client";
import { CoreApiError } from "@megle/core-client";
import { createCoreClient } from "./client";
import { readDesktopDiagnostics, type DesktopDiagnostics } from "./desktop";
import {
  isFreshThumbnailForMediaRecord,
  isLiveThumbnailResponseForMediaRecord,
  mediaContentSignature,
  pickPreferredThumbnailResponse,
  readCachedThumbnailStates,
  requestThumbnailState,
  type ThumbnailRequestPriority,
  shouldRequestThumbnailState
} from "./mediaResources";
import type { ThumbnailGenerationMode } from "../features/preview/previewPreferences";

const INITIAL_MEDIA_WINDOW_LIMIT = 96;
const MEDIA_WINDOW_MAX_LIMIT = 256;
const FOLDER_PAGE_LIMIT = 500;
const SEARCH_DEBOUNCE_MS = 250;
const INTERACTIVE_FOLDER_SCAN_DEBOUNCE_MS = 300;
const SCAN_REFRESH_INTERVAL_MS = 800;
const SELECTED_THUMBNAIL_REPOLL_MS = 150;
// Coalescing window for visible/ahead priority-scope syncs during continuous
// scroll. A leading sync fires immediately on the first change, subsequent
// changes inside the window are collapsed, and a trailing sync flushes the
// latest viewport once the window elapses. `selected` stays off this path and
// syncs near-immediately because it is a direct user action.
const THUMBNAIL_SCOPE_SYNC_DEBOUNCE_MS = 120;
// Grace window (ms) a file stays "retained" after it last left the viewport
// scope. While retained, Core keeps its thumbnail task warm (demoted to
// background, never cancelled), so scrolling back within the window does not
// lose or regenerate the tile. Once elapsed, the file drops out of
// `retainedFileIds` and Core may cancel its pending task (renderedOnly/cacheOnly)
// or bulk-demote it (allPersist). This IS the "delay a few seconds before
// cancelling" the user asked for.
const THUMBNAIL_SCOPE_RETENTION_MS = 6000;
const SHOW_SUBFOLDER_CONTENTS_STORAGE_KEY = "megle.library.subfolder-content-open";
const SEEN_THUMBNAIL_PERSIST_BATCH_SIZE = 128;
const SEEN_THUMBNAIL_PERSIST_DEBOUNCE_MS = 500;
const SEEN_THUMBNAIL_PERSIST_RETRY_MS = 10_000;

export type LibrarySort =
  | "mtime_desc"
  | "mtime_asc"
  | "name_asc"
  | "name_desc"
  | "rating_desc"
  | "rating_asc";

export type MediaKindFilter = "image" | "video" | "other";
export type MinRatingFilter = 1 | 2 | 3 | 4 | 5;

export interface FileOpResult {
  ok: boolean;
  code?: string;
  message?: string;
  operations?: FileOperationRecord[];
}

export interface SearchState {
  q: string;
  kind?: MediaKindFilter;
  minRating?: MinRatingFilter;
  favorite?: boolean;
  tagIds: number[];
  sort: LibrarySort;
}

export interface LibraryState {
  roots: RootRecord[];
  folders: FolderRecord[];
  media: MediaRecord[];
  mediaSlots: Map<number, MediaRecord>;
  mediaTotalCount: number;
  loadedMediaRanges: Array<{ start: number; end: number }>;
  showChildFolderContents: boolean;
  canNavigateFolderBack: boolean;
  canNavigateFolderForward: boolean;
  selectedRootId: number | null;
  selectedFolderId: number | null;
  selectedFolderInfo: FolderRecord | null;
  selectedMediaId: number | null;
  selectedMedia: MediaRecord | null;
  selectedMetadata: UserMetadataRecord | null;
  folderChildrenByParent: Record<number, FolderRecord[]>;
  folderDescendantsByParent: Record<number, FolderRecord[]>;
  folderChildNextCursorByParent: Record<number, string | null>;
  expandedFolderIds: Set<number>;
  loadingFolderIds: Set<number>;
  loadingFolderDescendantIds: Set<number>;
  loadingMoreFolderIds: Set<number>;
  loading: boolean;
  loadingMoreMedia: boolean;
  mediaHasMore: boolean;
  thumbnailStatesByMediaId: Record<number, ThumbnailResponse>;
  addingRoot: boolean;
  rescanningRootIds: Set<number>;
  scanActive: boolean;
  tasks: TaskRecord[];
  busyTaskIds: Set<number>;
  tags: TagRecord[];
  tagsById: Map<number, TagRecord>;
  searchState: SearchState;
  searchActive: boolean;
  metadataSaving: boolean;
  error: string | null;
  lastScan: ScanSummary | null;
  recentOps: FileOperationRecord[];
  recentOpsLoading: boolean;
  diagnostics: DesktopDiagnostics | null;
  diagnosticsProbed: boolean;
  thumbnailCacheStats: ThumbnailCacheStatsResponse | null;
  thumbnailCacheStatsLoading: boolean;
  thumbnailCacheActionBusy: boolean;
  loadRecentOps: () => Promise<void>;
  refreshThumbnailCacheStats: () => Promise<ThumbnailCacheStatsResponse | null>;
  generateCurrentFolderThumbnailCache: () => Promise<void>;
  generateCurrentTreeThumbnailCache: () => Promise<void>;
  generateAllThumbnailCache: () => Promise<void>;
  retryThumbnailCacheFailures: () => Promise<void>;
  clearThumbnailCache: () => Promise<void>;
  renameFile: (fileId: number, newName: string) => Promise<FileOpResult>;
  renameFolder: (folderId: number, newName: string) => Promise<FileOpResult>;
  moveItems: (input: {
    fileIds?: number[];
    folderIds?: number[];
    targetFolderId: number;
  }) => Promise<FileOpResult>;
  deleteItems: (input: {
    fileIds?: number[];
    folderIds?: number[];
    permanent: boolean;
  }) => Promise<FileOpResult>;
  setSelectedRootId: (rootId: number) => void;
  setSelectedFolder: (folder: FolderRecord) => void;
  setSelectedFolderInfo: (folder: FolderRecord | null) => void;
  navigateFolderBack: () => void;
  navigateFolderForward: () => void;
  toggleShowChildFolderContents: () => void;
  setSelectedMediaId: (mediaId: number | null) => void;
  toggleFolderExpanded: (folderId: number) => void;
  requestFolderChildren: (folderId: number) => Promise<FolderRecord[]>;
  requestThumbnailStates: (mediaIds: number[], priority: ThumbnailRequestPriority) => void;
  requestMediaWindow: (startIndex: number, endIndex: number) => void;
  loadMoreFolderChildren: (folderId: number) => Promise<void>;
  loadMoreMedia: () => Promise<void>;
  rescanRoot: (rootId: number) => Promise<void>;
  cancelTask: (taskId: number) => Promise<void>;
  retryTask: (taskId: number) => Promise<void>;
  refreshTasks: () => Promise<void>;
  refresh: () => Promise<void>;
  addRoot: (path: string) => Promise<void>;
  setQ: (q: string) => void;
  setKind: (kind: MediaKindFilter | undefined) => void;
  setMinRating: (rating: MinRatingFilter | undefined) => void;
  toggleFavoriteFilter: () => void;
  toggleTagFilter: (tagId: number) => void;
  setSort: (sort: LibrarySort) => void;
  clearFilters: () => void;
  updateMetadata: (fileId: number, patch: UserMetadataUpdate) => Promise<void>;
  setFileTags: (fileId: number, tagIds: number[]) => Promise<void>;
  addFileTag: (fileId: number, tagId: number) => Promise<void>;
  removeFileTag: (fileId: number, tagId: number) => Promise<void>;
  createTag: (name: string, color?: string | null) => Promise<TagRecord | null>;
  deleteTag: (tagId: number) => Promise<void>;
}

type ScanRefreshSelectionToken = {
  rootId: number;
  folderId: number | null;
  version: number;
};

type LibrarySelectionScope = {
  rootId?: number | null;
  folderId?: number | null;
};

type FolderNavigationEntry = {
  rootId: number;
  folderId: number | null;
};

type FolderNavigationHistory = {
  entries: FolderNavigationEntry[];
  index: number;
};

type ThumbnailPriorityScope = {
  selected: number[];
  visible: number[];
  ahead: number[];
};

type PendingInteractiveScanRequest = {
  rootId: number;
  folderId: number;
  knownTaskIds: Set<number>;
};

type UseLibraryDataOptions = {
  persistentThumbnailCacheAutoRefresh: boolean;
  // How thumbnails are generated and whether they persist. Drives both the
  // whole-library auto-generation ("allPersist") and whether displayed
  // thumbnails are written to the DB at all ("cacheOnly" skips persistence).
  // Default "renderedOnly": only thumbnails actually rendered on screen persist.
  thumbnailGenerationMode?: ThumbnailGenerationMode;
};

export function useLibraryData(
  options: UseLibraryDataOptions = { persistentThumbnailCacheAutoRefresh: false }
): LibraryState {
  const client = useMemo(() => createCoreClient(), []);
  const mediaPageGeneration = useRef(0);
  const inFlightMediaPageKeys = useRef<Set<string>>(new Set());
  const loadedMediaPageKeys = useRef<Set<string>>(new Set());
  const inFlightFolderChildPageKeys = useRef<Set<string>>(new Set());
  const loadedFolderChildPageKeys = useRef<Set<string>>(new Set());
  const inFlightFolderDescendantIds = useRef<Set<number>>(new Set());
  const mediaPageControllersRef = useRef<Map<string, AbortController>>(new Map());
  const folderChildControllersRef = useRef<Map<string, AbortController>>(new Map());
  const folderDescendantControllersRef = useRef<Map<number, AbortController>>(new Map());
  const folderChildInitialRequestsRef = useRef<Map<number, Promise<FolderRecord[]>>>(
    new Map()
  );
  const thumbnailStateControllersRef = useRef<Map<ThumbnailRequestPriority, AbortController>>(
    new Map()
  );
  const seenThumbnailPersistIdsRef = useRef<Set<number>>(new Set());
  const seenThumbnailPersistFlushTimerRef = useRef<number | null>(null);
  const seenThumbnailPersistFlushInFlightRef = useRef(false);
  const seenThumbnailPersistControllerRef = useRef<AbortController | null>(null);
  const seenThumbnailPersistActiveRef = useRef(true);
  const loadTasksRequestRef = useRef<Promise<TaskRecord[]> | null>(null);
  const interactiveFolderScanControllerRef = useRef<AbortController | null>(null);
  const thumbnailPriorityScopeSyncControllerRef = useRef<AbortController | null>(null);
  const thumbnailCacheStatsControllerRef = useRef<AbortController | null>(null);
  const thumbnailCacheActionControllerRef = useRef<AbortController | null>(null);
  const thumbnailCacheAutoRefreshTimerRef = useRef<number | null>(null);
  const lastThumbnailCacheAutoRefreshScopeKeyRef = useRef<string | null>(null);
  const scanRefreshInFlightRef = useRef(false);
  const scanRefreshActiveRootIdRef = useRef<number | null>(null);
  const scanRefreshSelectionVersionRef = useRef(0);
  const [roots, setRoots] = useState<RootRecord[]>([]);
  const [folderChildrenByParent, setFolderChildrenByParent] = useState<Record<number, FolderRecord[]>>(
    {}
  );
  const folderChildrenByParentRef = useRef(folderChildrenByParent);
  folderChildrenByParentRef.current = folderChildrenByParent;
  const [folderDescendantsByParent, setFolderDescendantsByParent] = useState<
    Record<number, FolderRecord[]>
  >({});
  const folderDescendantsByParentRef = useRef(folderDescendantsByParent);
  folderDescendantsByParentRef.current = folderDescendantsByParent;
  const [folderChildNextCursorByParent, setFolderChildNextCursorByParent] = useState<
    Record<number, string | null>
  >({});
  const [expandedFolderIds, setExpandedFolderIds] = useState<Set<number>>(() => new Set());
  const [loadingFolderIds, setLoadingFolderIds] = useState<Set<number>>(() => new Set());
  const [loadingFolderDescendantIds, setLoadingFolderDescendantIds] = useState<Set<number>>(
    () => new Set()
  );
  const [loadingMoreFolderIds, setLoadingMoreFolderIds] = useState<Set<number>>(() => new Set());
  const [media, setMedia] = useState<MediaRecord[]>([]);
  const [mediaSlots, setMediaSlots] = useState<Map<number, MediaRecord>>(() => new Map());
  const [mediaTotalCount, setMediaTotalCount] = useState(0);
  const [loadedMediaRanges, setLoadedMediaRanges] = useState<Array<{ start: number; end: number }>>(
    []
  );
  const mediaTotalCountRef = useRef(mediaTotalCount);
  mediaTotalCountRef.current = mediaTotalCount;
  const loadedMediaRangesRef = useRef(loadedMediaRanges);
  loadedMediaRangesRef.current = loadedMediaRanges;
  // Keyset cursors for windowed pagination: maps a window START index to the
  // cursor that fetches the page beginning there (learned from the previous
  // page's nextCursor). Sequential deep scroll then pages with an O(1) keyset
  // cursor instead of an O(offset) OFFSET walk (~142ms at 650k rows -> ~16ms).
  // Random jumps (no cached cursor for that start) fall back to OFFSET. Only
  // valid for the non-search listMedia path; searchMedia stays OFFSET-based.
  const mediaCursorByStartIndexRef = useRef<Map<number, string>>(new Map());
  const [thumbnailStatesByMediaId, setThumbnailStatesByMediaId] = useState<
    Record<number, ThumbnailResponse>
  >({});
  const thumbnailStateSignaturesByMediaIdRef = useRef<Record<number, string>>({});
  const thumbnailStateRequestKeyByPriorityRef = useRef<Map<ThumbnailRequestPriority, string>>(
    new Map()
  );
  const [mediaNextCursor, setMediaNextCursor] = useState<string | null>(null);
  const mediaHasMore = media.length < mediaTotalCount || mediaNextCursor !== null;
  const [showChildFolderContents, setShowChildFolderContents] = useState<boolean>(() =>
    readStoredShowChildFolderContents()
  );
  const [selectedRootId, selectRoot] = useState<number | null>(null);
  const [selectedFolderId, selectFolder] = useState<number | null>(null);
  const [selectedFolderInfo, setSelectedFolderInfoState] = useState<FolderRecord | null>(null);
  const [selectedMediaId, selectMedia] = useState<number | null>(null);
  const [folderNavigationHistory, setFolderNavigationHistory] = useState<FolderNavigationHistory>({
    entries: [],
    index: -1
  });
  const selectedRootIdRef = useRef<number | null>(selectedRootId);
  const selectedFolderIdRef = useRef<number | null>(selectedFolderId);
  const selectedMediaIdRef = useRef<number | null>(selectedMediaId);
  const selectedMediaSnapshotRef = useRef<MediaRecord | null>(null);
  const folderNavigationHistoryRef = useRef<FolderNavigationHistory>(folderNavigationHistory);
  const thumbnailPriorityScopeRef = useRef<ThumbnailPriorityScope>(emptyThumbnailPriorityScope());
  // file id -> last time it was in the visible/ahead scope (ms). Drives the
  // retention grace window: a file still within THUMBNAIL_SCOPE_RETENTION_MS is
  // sent as `retainedFileIds` so Core keeps its task warm across scroll-back.
  const thumbnailScopeRecencyRef = useRef<Map<number, number>>(new Map());
  const thumbnailPriorityScopeSyncKeyRef = useRef<string | null>(null);
  const thumbnailPriorityScopeSyncTimerRef = useRef<number | null>(null);
  const thumbnailPriorityScopeSyncTrailingPendingRef = useRef(false);
  const pendingInteractiveScanRequestRef = useRef<PendingInteractiveScanRequest | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMoreMedia, setLoadingMoreMedia] = useState(false);
  const [addingRoot, setAddingRoot] = useState(false);
  const [rescanningRootIds, setRescanningRootIds] = useState<Set<number>>(() => new Set());
  const [tasks, setTasks] = useState<TaskRecord[]>([]);
  const [busyTaskIds, setBusyTaskIds] = useState<Set<number>>(() => new Set());
  const [taskPollFailures, setTaskPollFailures] = useState(0);
  const [scanRefreshFailures, setScanRefreshFailures] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [lastScan, setLastScan] = useState<ScanSummary | null>(null);
  const [tags, setTags] = useState<TagRecord[]>([]);
  const [searchState, setSearchState] = useState<SearchState>({
    q: "",
    kind: undefined,
    minRating: undefined,
    favorite: undefined,
    tagIds: [],
    sort: "name_asc"
  });
  const [debouncedQ, setDebouncedQ] = useState("");
  const [selectedMetadata, setSelectedMetadata] = useState<UserMetadataRecord | null>(null);
  const [metadataSaving, setMetadataSaving] = useState(false);
  const metadataEditGenerationRef = useRef(0);
  const [recentOps, setRecentOps] = useState<FileOperationRecord[]>([]);
  const [recentOpsLoading, setRecentOpsLoading] = useState(false);
  const [diagnostics, setDiagnostics] = useState<DesktopDiagnostics | null>(null);
  const [diagnosticsProbed, setDiagnosticsProbed] = useState(false);
  const [thumbnailCacheStats, setThumbnailCacheStats] =
    useState<ThumbnailCacheStatsResponse | null>(null);
  const [thumbnailCacheStatsLoading, setThumbnailCacheStatsLoading] = useState(false);
  const [thumbnailCacheActionBusy, setThumbnailCacheActionBusy] = useState(false);
  const loadedMediaById = useMemo(() => {
    const next = new Map<number, MediaRecord>();
    media.forEach((item) => next.set(item.id, item));
    mediaSlots.forEach((item) => {
      if (item) next.set(item.id, item);
    });
    return next;
  }, [media, mediaSlots]);
  const mediaById = useMemo(() => {
    const next = new Map(loadedMediaById);
    const selectedSnapshot = selectedMediaSnapshotRef.current;
    if (selectedSnapshot && !next.has(selectedSnapshot.id)) {
      next.set(selectedSnapshot.id, selectedSnapshot);
    }
    return next;
  }, [loadedMediaById]);
  const mediaByIdRef = useRef(mediaById);
  mediaByIdRef.current = mediaById;
  const tasksRef = useRef(tasks);
  tasksRef.current = tasks;
  const failedThumbnailRetrySignaturesRef = useRef<Set<string>>(new Set());
  const freshThumbnailStatesByMediaId = useMemo(
    () =>
      filterFreshThumbnailStates(
        thumbnailStatesByMediaId,
        mediaById,
        thumbnailStateSignaturesByMediaIdRef.current
      ),
    [mediaById, thumbnailStatesByMediaId]
  );
  const freshThumbnailStatesByMediaIdRef = useRef(freshThumbnailStatesByMediaId);
  freshThumbnailStatesByMediaIdRef.current = freshThumbnailStatesByMediaId;

  const liveSelectedMedia =
    selectedMediaId === null ? null : loadedMediaById.get(selectedMediaId) ?? null;
  if (liveSelectedMedia) {
    selectedMediaSnapshotRef.current = liveSelectedMedia;
  } else if (
    selectedMediaId === null ||
    selectedMediaSnapshotRef.current?.id !== selectedMediaId
  ) {
    selectedMediaSnapshotRef.current = null;
  }
  const selectedMedia =
    selectedMediaId === null
      ? null
      : liveSelectedMedia ??
        (selectedMediaSnapshotRef.current?.id === selectedMediaId
          ? selectedMediaSnapshotRef.current
          : null);
  const selectedRoot = roots.find((item) => item.id === selectedRootId) ?? null;
  const selectedThumbnail = selectedMedia
    ? freshThumbnailStatesByMediaId[selectedMedia.id]
    : undefined;
  const selectedMediaThumbnailRequestKey = selectedMedia
    ? mediaContentSignature(selectedMedia)
    : null;
  const folders = useMemo(
    () => Object.values(folderChildrenByParent).flat(),
    [folderChildrenByParent]
  );
  const scanActive = tasks.some((task) => task.status === "pending" || task.status === "running");
  const scanActiveRootTask =
    selectedRootId !== null &&
    tasks.some(
      (task) =>
        task.kind === "root_scan" &&
        task.rootId === selectedRootId &&
        isTaskActive(task)
    );
  const tagsById = useMemo(() => new Map(tags.map((tag) => [tag.id, tag])), [tags]);

  const searchActive = useMemo(
    () =>
      debouncedQ.trim() !== "" ||
      searchState.kind !== undefined ||
      searchState.minRating !== undefined ||
      searchState.favorite !== undefined ||
      searchState.tagIds.length > 0,
    [debouncedQ, searchState.kind, searchState.minRating, searchState.favorite, searchState.tagIds]
  );
  const searchActiveRef = useRef(searchActive);
  searchActiveRef.current = searchActive;
  const showChildFolderContentsRef = useRef(showChildFolderContents);
  showChildFolderContentsRef.current = showChildFolderContents;
  const searchStateRef = useRef(searchState);
  searchStateRef.current = searchState;
  const debouncedQRef = useRef(debouncedQ);
  debouncedQRef.current = debouncedQ;
  selectedRootIdRef.current = selectedRootId;
  selectedFolderIdRef.current = selectedFolderId;
  selectedMediaIdRef.current = selectedMediaId;
  folderNavigationHistoryRef.current = folderNavigationHistory;
  // The active generation mode, read by the thumbnail-persist scheduler (which
  // runs outside React render). "cacheOnly" never persists; "renderedOnly" and
  // "allPersist" persist thumbnails that reached the ready state.
  const generationModeRef = useRef<ThumbnailGenerationMode>(
    options.thumbnailGenerationMode ?? "renderedOnly"
  );
  generationModeRef.current = options.thumbnailGenerationMode ?? "renderedOnly";

  const currentThumbnailCacheScope = useCallback(
    (includeDescendants = showChildFolderContentsRef.current) => {
      const rootId = selectedRootIdRef.current;
      if (rootId === null) {
        return null;
      }
      return {
        rootId,
        folderId: selectedFolderIdRef.current ?? undefined,
        includeDescendants
      };
    },
    []
  );

  const refreshThumbnailCacheStats = useCallback(async () => {
    // The thumbnail-status tab shows LIBRARY-WIDE progress, so stats are always
    // computed over the whole library ({} scope) — never the current folder.
    // This is deliberate (issue #2): a folder-scoped stats query recomputes an
    // expensive whole-scope aggregate on every folder switch and pins the read
    // connection/WAL, which made switching folders slow while the tab was open.
    // A fixed whole-library scope keeps the server-side TTL cache warm across
    // folder switches, so stats never interfere with thumbnail refresh.
    thumbnailCacheStatsControllerRef.current?.abort();
    const controller = new AbortController();
    thumbnailCacheStatsControllerRef.current = controller;
    setThumbnailCacheStatsLoading(true);
    try {
      const stats = await client.getThumbnailCacheStats(
        {},
        {
          requestPriority: "metadata",
          signal: controller.signal
        }
      );
      if (thumbnailCacheStatsControllerRef.current === controller) {
        setThumbnailCacheStats(stats);
      }
      return stats;
    } catch (cause) {
      if (isAbortError(cause)) {
        return null;
      }
      setError(errorMessage(cause));
      return null;
    } finally {
      if (thumbnailCacheStatsControllerRef.current === controller) {
        thumbnailCacheStatsControllerRef.current = null;
      }
      setThumbnailCacheStatsLoading(false);
    }
  }, [client]);

  const enqueueThumbnailCacheLoop = useCallback(
    async (scope: { rootId?: number; folderId?: number; includeDescendants?: boolean }, refreshMode: ThumbnailCacheRefreshMode) => {
      thumbnailCacheActionControllerRef.current?.abort();
      const controller = new AbortController();
      thumbnailCacheActionControllerRef.current = controller;
      setThumbnailCacheActionBusy(true);
      try {
        while (!controller.signal.aborted) {
          const response = await client.enqueueThumbnailCache(
            {
              ...scope,
              refreshMode,
              limit: MEDIA_WINDOW_MAX_LIMIT
            },
            {
              requestPriority: "background",
              signal: controller.signal
            }
          );
          // Don't feed the enqueue response into the shared stats state: it no
          // longer computes whole-scope stats (that O(scope) query blocked the
          // writer). The settings tab polls real whole-library stats separately.
          // The loop terminates on acceptedCount / the derived pendingCandidateCount.
          const nothingLeft =
            response.pendingCandidateCount === 0 &&
            (refreshMode !== "retryFailedAndStale" || response.failedCount === 0);
          if (response.acceptedCount === 0 || nothingLeft) {
            break;
          }
          await yieldToBrowser();
        }
      } catch (cause) {
        if (!isAbortError(cause)) {
          setError(errorMessage(cause));
        }
      } finally {
        if (thumbnailCacheActionControllerRef.current === controller) {
          thumbnailCacheActionControllerRef.current = null;
        }
        setThumbnailCacheActionBusy(false);
        // Refresh the real whole-library stats once the manual loop settles.
        void refreshThumbnailCacheStats();
      }
    },
    [client, refreshThumbnailCacheStats]
  );

  const abortStaleMediaPageRequests = useCallback((keepRequestKey?: string) => {
    if (!keepRequestKey) {
      abortAllControllers(mediaPageControllersRef.current);
      inFlightMediaPageKeys.current.clear();
      return;
    }

    for (const [requestKey, controller] of mediaPageControllersRef.current) {
      if (requestKey === keepRequestKey) {
        continue;
      }
      controller.abort();
      mediaPageControllersRef.current.delete(requestKey);
      inFlightMediaPageKeys.current.delete(requestKey);
    }
  }, []);

  const abortStaleFolderRequests = useCallback(() => {
    abortAllControllers(folderChildControllersRef.current);
    abortAllControllers(folderDescendantControllersRef.current);
    folderChildInitialRequestsRef.current.clear();
    inFlightFolderChildPageKeys.current.clear();
    inFlightFolderDescendantIds.current.clear();
    setLoadingFolderIds(new Set());
    setLoadingFolderDescendantIds(new Set());
    setLoadingMoreFolderIds(new Set());
  }, []);

  const flushThumbnailPriorityScopeSync = useCallback(
    async (rootId = selectedRootIdRef.current) => {
      if (thumbnailPriorityScopeSyncTimerRef.current !== null) {
        window.clearTimeout(thumbnailPriorityScopeSyncTimerRef.current);
        thumbnailPriorityScopeSyncTimerRef.current = null;
      }
      // A synchronous flush pushes the latest scope, so no coalesced trailing
      // flush is owed once the window timer is torn down.
      thumbnailPriorityScopeSyncTrailingPendingRef.current = false;
      if (rootId === null) {
        return;
      }

      const scope = thumbnailPriorityScopeRef.current;
      // Compute the retention set: everything currently in scope plus every file
      // still within the recency grace window. Prune expired recency entries
      // here (the sync is the natural cadence for it).
      const nowMs = Date.now();
      const recency = thumbnailScopeRecencyRef.current;
      for (const [fileId, seenAt] of recency) {
        if (nowMs - seenAt > THUMBNAIL_SCOPE_RETENTION_MS) {
          recency.delete(fileId);
        }
      }
      const retained = new Set<number>([
        ...scope.selected,
        ...scope.visible,
        ...scope.ahead,
        ...recency.keys()
      ]);
      const input = {
        rootId,
        selectedFileIds: scope.selected,
        visibleFileIds: scope.visible,
        aheadFileIds: scope.ahead,
        retainedFileIds: Array.from(retained),
        generationMode: generationModeRef.current
      };
      if (
        input.selectedFileIds.length === 0 &&
        input.visibleFileIds.length === 0 &&
        input.aheadFileIds.length === 0
      ) {
        thumbnailPriorityScopeSyncKeyRef.current = null;
        return;
      }
      const requestKey = thumbnailPriorityScopeRequestKey(input);
      if (thumbnailPriorityScopeSyncKeyRef.current === requestKey) {
        return;
      }

      thumbnailPriorityScopeSyncControllerRef.current?.abort();
      const controller = new AbortController();
      thumbnailPriorityScopeSyncControllerRef.current = controller;
      thumbnailPriorityScopeSyncKeyRef.current = requestKey;
      try {
        await client.syncThumbnailPriorityScope(input, {
          requestPriority: "interactive",
          signal: controller.signal
        });
      } catch (cause) {
        if (isAbortError(cause)) {
          return;
        }
        if (thumbnailPriorityScopeSyncKeyRef.current === requestKey) {
          thumbnailPriorityScopeSyncKeyRef.current = null;
        }
        throw cause;
      } finally {
        if (thumbnailPriorityScopeSyncControllerRef.current === controller) {
          thumbnailPriorityScopeSyncControllerRef.current = null;
        }
      }
    },
    [client]
  );

  const scheduleThumbnailPriorityScopeSync = useCallback(
    (priority: ThumbnailRequestPriority, mediaIds: number[]) => {
      if (priority === "background") {
        return;
      }

      thumbnailPriorityScopeRef.current = nextThumbnailPriorityScope(
        thumbnailPriorityScopeRef.current,
        priority,
        mediaIds,
        selectedMediaIdRef.current
      );
      // Stamp recency for every file that just entered a foreground tier so it
      // stays "retained" (task kept warm) for the grace window after it later
      // leaves the scope. selected/visible/ahead all count as "recently seen".
      const nowMs = Date.now();
      for (const mediaId of mediaIds) {
        if (mediaId > 0) {
          thumbnailScopeRecencyRef.current.set(mediaId, nowMs);
        }
      }
      if (selectedRootIdRef.current === null) {
        if (thumbnailPriorityScopeSyncTimerRef.current !== null) {
          window.clearTimeout(thumbnailPriorityScopeSyncTimerRef.current);
          thumbnailPriorityScopeSyncTimerRef.current = null;
        }
        thumbnailPriorityScopeSyncTrailingPendingRef.current = false;
        return;
      }

      // Open (or continue) the coalescing window so a trailing sync always lands
      // shortly after the last visible/ahead change while scrolling settles. The
      // window is never reset per change; subsequent changes just flag that the
      // latest scope still needs a trailing flush.
      const openThumbnailScopeSyncWindow = () => {
        thumbnailPriorityScopeSyncTimerRef.current = window.setTimeout(() => {
          thumbnailPriorityScopeSyncTimerRef.current = null;
          if (!thumbnailPriorityScopeSyncTrailingPendingRef.current) {
            return;
          }
          thumbnailPriorityScopeSyncTrailingPendingRef.current = false;
          void flushThumbnailPriorityScopeSync().catch((cause) => {
            setError(errorMessage(cause));
          });
          openThumbnailScopeSyncWindow();
        }, THUMBNAIL_SCOPE_SYNC_DEBOUNCE_MS);
      };

      // A direct user selection must sync immediately. A visible change leads the
      // scroll burst: it flushes immediately on the leading edge, then coalesces
      // the rest of the burst into the trailing window above.
      if (priority === "selected" || priority === "visible") {
        if (priority === "visible" && thumbnailPriorityScopeSyncTimerRef.current !== null) {
          thumbnailPriorityScopeSyncTrailingPendingRef.current = true;
          return;
        }
        void flushThumbnailPriorityScopeSync().catch((cause) => {
          setError(errorMessage(cause));
        });
        if (priority === "visible") {
          openThumbnailScopeSyncWindow();
        }
        return;
      }

      // ahead: coalesce on the same cadence as visible so prefetch churn does not
      // burst the network during continuous scroll.
      if (thumbnailPriorityScopeSyncTimerRef.current !== null) {
        thumbnailPriorityScopeSyncTrailingPendingRef.current = true;
        return;
      }
      void flushThumbnailPriorityScopeSync().catch((cause) => {
        setError(errorMessage(cause));
      });
      openThumbnailScopeSyncWindow();
    },
    [flushThumbnailPriorityScopeSync]
  );

  const flushSeenThumbnailPersistQueue = useCallback(async () => {
    if (!seenThumbnailPersistActiveRef.current) {
      return;
    }
    if (seenThumbnailPersistFlushInFlightRef.current) {
      return;
    }
    if (seenThumbnailPersistFlushTimerRef.current !== null) {
      window.clearTimeout(seenThumbnailPersistFlushTimerRef.current);
      seenThumbnailPersistFlushTimerRef.current = null;
    }
    const batch = Array.from(seenThumbnailPersistIdsRef.current).slice(
      0,
      SEEN_THUMBNAIL_PERSIST_BATCH_SIZE
    );
    if (batch.length === 0) {
      return;
    }
    seenThumbnailPersistFlushInFlightRef.current = true;
    const controller = new AbortController();
    seenThumbnailPersistControllerRef.current = controller;
    let retryDelayMs = SEEN_THUMBNAIL_PERSIST_DEBOUNCE_MS;
    try {
      await client.enqueueThumbnailCache(
        {
          fileIds: batch,
          refreshMode: "staleOrMissing",
          limit: batch.length
        },
        {
          requestPriority: "background",
          signal: controller.signal
        }
      );
      for (const fileId of batch) {
        seenThumbnailPersistIdsRef.current.delete(fileId);
      }
    } catch (cause) {
      if (!isAbortError(cause)) {
        retryDelayMs = SEEN_THUMBNAIL_PERSIST_RETRY_MS;
        setError(errorMessage(cause));
      }
    } finally {
      if (seenThumbnailPersistControllerRef.current === controller) {
        seenThumbnailPersistControllerRef.current = null;
      }
      seenThumbnailPersistFlushInFlightRef.current = false;
      if (!seenThumbnailPersistActiveRef.current) {
        return;
      }
      if (seenThumbnailPersistIdsRef.current.size > 0) {
        if (seenThumbnailPersistFlushTimerRef.current !== null) {
          window.clearTimeout(seenThumbnailPersistFlushTimerRef.current);
        }
        seenThumbnailPersistFlushTimerRef.current = window.setTimeout(() => {
          seenThumbnailPersistFlushTimerRef.current = null;
          void flushSeenThumbnailPersistQueue();
        }, retryDelayMs);
      }
    }
  }, [client]);

  const scheduleSeenThumbnailPersist = useCallback(
    (mediaIds: number[]) => {
      if (!seenThumbnailPersistActiveRef.current) {
        return;
      }
      // "cacheOnly" mode never writes thumbnails to the DB — generated bytes
      // live only in the in-memory object-URL cache for this session.
      if (generationModeRef.current === "cacheOnly") {
        return;
      }
      if (mediaIds.length === 0) {
        return;
      }
      for (const mediaId of mediaIds) {
        if (mediaId > 0) {
          seenThumbnailPersistIdsRef.current.add(mediaId);
        }
      }
      if (seenThumbnailPersistFlushTimerRef.current !== null) {
        return;
      }
      seenThumbnailPersistFlushTimerRef.current = window.setTimeout(() => {
        seenThumbnailPersistFlushTimerRef.current = null;
        void flushSeenThumbnailPersistQueue();
      }, SEEN_THUMBNAIL_PERSIST_DEBOUNCE_MS);
    },
    [flushSeenThumbnailPersistQueue]
  );

  const abortThumbnailStateControllersForPriority = useCallback(
    (priority: ThumbnailRequestPriority) => {
      const priorities: ThumbnailRequestPriority[] =
        priority === "selected"
          ? ["selected"]
          : priority === "visible"
            ? ["visible", "ahead"]
            : ["ahead"];
      for (const targetPriority of priorities) {
        thumbnailStateControllersRef.current.get(targetPriority)?.abort();
        thumbnailStateControllersRef.current.delete(targetPriority);
        thumbnailStateRequestKeyByPriorityRef.current.delete(targetPriority);
      }
    },
    []
  );

  const requestThumbnailStates = useCallback((
    mediaIds: number[],
    priority: ThumbnailRequestPriority
  ) => {
    scheduleThumbnailPriorityScopeSync(priority, mediaIds);
    const normalizedMediaIds = normalizeThumbnailScopeMediaIds(
      mediaIds,
      priority,
      selectedMediaIdRef.current
    );
    const mediaRecords = normalizedMediaIds
      .map((mediaId) => mediaByIdRef.current.get(mediaId))
      .filter((mediaRecord): mediaRecord is MediaRecord => Boolean(mediaRecord));
    if (mediaRecords.length === 0) {
      return;
    }
    if (priority !== "background") {
      // Issue #3: persist only thumbnails that have ALREADY been generated
      // (state "ready"), not every tile that merely entered visible/ahead
      // scope. Freshly generated thumbnails get persisted when their state
      // resolves to "ready" in the response handler below. This stops the app
      // from generating+persisting for tiles that are scrolled past before they
      // ever render. "cacheOnly" is filtered inside scheduleSeenThumbnailPersist.
      const readyMediaIds = normalizedMediaIds.filter((mediaId) => {
        const mediaRecord = mediaByIdRef.current.get(mediaId);
        if (!mediaRecord) {
          return false;
        }
        const state =
          freshThumbnailStatesByMediaIdRef.current[mediaId]?.state ??
          explicitMediaThumbnailState(mediaRecord.thumbnailState);
        return state === "ready";
      });
      if (readyMediaIds.length > 0) {
        scheduleSeenThumbnailPersist(readyMediaIds);
      }
    }
    const requestKey = [
      priority,
      ...mediaRecords.map((mediaRecord) => mediaContentSignature(mediaRecord))
    ].join("|");
    const activeController = thumbnailStateControllersRef.current.get(priority);
    if (
      activeController &&
      !activeController.signal.aborted &&
      thumbnailStateRequestKeyByPriorityRef.current.get(priority) === requestKey
    ) {
      return;
    }

    abortThumbnailStateControllersForPriority(priority);
    const controller = new AbortController();
    thumbnailStateControllersRef.current.set(priority, controller);
    thumbnailStateRequestKeyByPriorityRef.current.set(priority, requestKey);

    const cachedStates = readCachedThumbnailStates(mediaRecords);
    if (Object.keys(cachedStates).length > 0) {
      recordThumbnailStateSignatures(
        cachedStates,
        mediaByIdRef.current,
        thumbnailStateSignaturesByMediaIdRef.current
      );
      setThumbnailStatesByMediaId((current) => mergeThumbnailStates(current, cachedStates));
    }

    const pendingRequests: Promise<unknown>[] = [];
    for (const mediaRecord of mediaRecords) {
      const currentThumbnail = freshThumbnailStatesByMediaIdRef.current[mediaRecord.id];
      const currentMediaSignature = mediaContentSignature(mediaRecord);
      const effectiveState =
        currentThumbnail?.state ?? explicitMediaThumbnailState(mediaRecord.thumbnailState);
      if (
        effectiveState === "failed" &&
        priority !== "background" &&
        !failedThumbnailRetrySignaturesRef.current.has(currentMediaSignature)
      ) {
        const failedTask = latestFailedThumbnailTaskForFile(tasksRef.current, mediaRecord.id);
        if (failedTask) {
          failedThumbnailRetrySignaturesRef.current.add(currentMediaSignature);
          thumbnailStateSignaturesByMediaIdRef.current[mediaRecord.id] = currentMediaSignature;
          setThumbnailStatesByMediaId((current) => ({
            ...current,
            [mediaRecord.id]: optimisticQueuedThumbnail(mediaRecord.id)
          }));
          const retryRequest = client
            .retryTask(failedTask.id)
            .then(() => client.listTasks({ signal: controller.signal }))
            .then((response) => {
              if (controller.signal.aborted) return;
              setTasks(response.items);
              setTaskPollFailures(0);
            })
            .then(() =>
              requestThumbnailState(mediaRecord, priority, { signal: controller.signal })
            )
            .then((thumbnail) => {
              if (controller.signal.aborted) return;
              setThumbnailStatesByMediaId((current) => ({
                ...current,
                [mediaRecord.id]: pickPreferredThumbnailResponse(
                  current[mediaRecord.id],
                  thumbnail
                )
              }));
            })
            .catch((cause) => {
              if (cause instanceof Error && cause.name === "AbortError") {
                return;
              }
              setError(errorMessage(cause));
            });
          pendingRequests.push(retryRequest);
          continue;
        }
      }
      const shouldFetchThumbnail =
        !currentThumbnail ||
        currentThumbnail?.state === "pending" ||
        currentThumbnail?.state === "queued";
      if (!shouldFetchThumbnail) {
        continue;
      }

      const requestedMediaSignature = currentMediaSignature;
      const thumbnailRequest = requestThumbnailState(mediaRecord, priority, { signal: controller.signal })
        .then((thumbnail) => {
          if (controller.signal.aborted) {
            return;
          }
          setThumbnailStatesByMediaId((current) => {
            const currentMediaRecord = mediaByIdRef.current.get(mediaRecord.id);
            const currentMediaSignature = currentMediaRecord
              ? mediaContentSignature(currentMediaRecord)
              : null;
            if (
              !currentMediaRecord ||
              currentMediaSignature !== requestedMediaSignature ||
              !isLiveThumbnailResponseForMediaRecord(currentMediaRecord, thumbnail)
            ) {
              delete thumbnailStateSignaturesByMediaIdRef.current[mediaRecord.id];
              return removeThumbnailState(current, mediaRecord.id);
            }

            thumbnailStateSignaturesByMediaIdRef.current[mediaRecord.id] =
              currentMediaSignature;
            const mergedThumbnail = pickPreferredThumbnailResponse(
              current[mediaRecord.id],
              thumbnail
            );
            // Issue #3: a thumbnail that just resolved to "ready" for a
            // non-background tile has genuinely been generated and shown — this
            // is the moment to persist it (scheduleSeenThumbnailPersist filters
            // cacheOnly). Persisting here, not at request time, ensures we only
            // write thumbnails that actually rendered.
            if (priority !== "background" && mergedThumbnail.state === "ready") {
              scheduleSeenThumbnailPersist([mediaRecord.id]);
            }
            if (current[mediaRecord.id] === mergedThumbnail) {
              return current;
            }
            return {
              ...current,
              [mediaRecord.id]: mergedThumbnail
            };
          });
        })
        .catch((cause) => {
          if (cause instanceof Error && cause.name === "AbortError") {
            return;
          }
          setThumbnailStatesByMediaId((current) => {
            const currentMediaRecord = mediaByIdRef.current.get(mediaRecord.id);
            const currentMediaSignature = currentMediaRecord
              ? mediaContentSignature(currentMediaRecord)
              : null;
            if (!currentMediaRecord || currentMediaSignature !== requestedMediaSignature) {
              delete thumbnailStateSignaturesByMediaIdRef.current[mediaRecord.id];
              return removeThumbnailState(current, mediaRecord.id);
            }
            delete thumbnailStateSignaturesByMediaIdRef.current[mediaRecord.id];
            return removeThumbnailState(current, mediaRecord.id);
          });
        });
      pendingRequests.push(thumbnailRequest);
    }
    if (pendingRequests.length === 0) {
      thumbnailStateControllersRef.current.delete(priority);
      thumbnailStateRequestKeyByPriorityRef.current.delete(priority);
      return;
    }
    void Promise.allSettled(pendingRequests).finally(() => {
      if (thumbnailStateRequestKeyByPriorityRef.current.get(priority) !== requestKey) {
        return;
      }
      thumbnailStateControllersRef.current.delete(priority);
      thumbnailStateRequestKeyByPriorityRef.current.delete(priority);
    });
  }, [
    abortThumbnailStateControllersForPriority,
    client,
    scheduleSeenThumbnailPersist,
    scheduleThumbnailPriorityScopeSync
  ]);

  const loadFolderChildren = useCallback(
    async (folderId: number, options: { force?: boolean } = {}) => {
      const force = options.force ?? false;
      const cachedChildren = folderChildrenByParentRef.current[folderId];
      if (!force && cachedChildren !== undefined) {
        return cachedChildren;
      }
      const existingRequest = folderChildInitialRequestsRef.current.get(folderId);
      if (!force && existingRequest) {
        return existingRequest;
      }
      const requestKey = folderChildPageRequestKey(folderId, "initial");
      if (force) {
        clearFolderChildPageKeys(loadedFolderChildPageKeys.current, folderId);
        folderChildControllersRef.current.get(requestKey)?.abort();
        folderChildInitialRequestsRef.current.delete(folderId);
      }
      const controller = new AbortController();
      folderChildControllersRef.current.set(requestKey, controller);
      setLoadingFolderIds((current) => new Set(current).add(folderId));

      const request = (async () => {
        const cursor = null;
        const page = await client.listFolderChildren(folderId, {
          cursor: cursor ?? undefined,
          limit: FOLDER_PAGE_LIMIT
        }, {
          signal: controller.signal
        });
        const nextChildrenByParent = {
          ...folderChildrenByParentRef.current,
          [folderId]: page.items
        };
        folderChildrenByParentRef.current = nextChildrenByParent;
        setFolderChildrenByParent(nextChildrenByParent);
        loadedFolderChildPageKeys.current.add(requestKey);
        setFolderChildNextCursorByParent((current) => ({
          ...current,
          [folderId]: page.nextCursor
        }));
        return page.items;
      })();

      folderChildInitialRequestsRef.current.set(folderId, request);
      try {
        return await request;
      } catch (cause) {
        if (isAbortError(cause)) {
          return [];
        }
        throw cause;
      } finally {
        if (folderChildControllersRef.current.get(requestKey) === controller) {
          folderChildControllersRef.current.delete(requestKey);
        }
        if (folderChildInitialRequestsRef.current.get(folderId) === request) {
          folderChildInitialRequestsRef.current.delete(folderId);
        }
        setLoadingFolderIds((current) => {
          const next = new Set(current);
          next.delete(folderId);
          return next;
        });
      }
    },
    [client]
  );

  const loadMoreFolderChildren = useCallback(
    async (folderId: number) => {
      const cursor = folderChildNextCursorByParent[folderId];
      if (!cursor || loadingMoreFolderIds.has(folderId)) {
        return;
      }
      const requestKey = folderChildPageRequestKey(folderId, cursor);
      if (
        inFlightFolderChildPageKeys.current.has(requestKey) ||
        loadedFolderChildPageKeys.current.has(requestKey)
      ) {
        return;
      }

      inFlightFolderChildPageKeys.current.add(requestKey);
      const controller = new AbortController();
      folderChildControllersRef.current.set(requestKey, controller);
      setLoadingMoreFolderIds((current) => new Set(current).add(folderId));
      setError(null);
      try {
        const page = await client.listFolderChildren(folderId, {
          cursor: cursor ?? undefined,
          limit: FOLDER_PAGE_LIMIT
        }, {
          signal: controller.signal
        });
        setFolderChildrenByParent((current) => ({
          ...current,
          [folderId]: [...(current[folderId] ?? []), ...page.items]
        }));
        loadedFolderChildPageKeys.current.add(requestKey);
        setFolderChildNextCursorByParent((current) => ({
          ...current,
          [folderId]: page.nextCursor
        }));
      } catch (cause) {
        if (isAbortError(cause)) {
          return;
        }
        setError(errorMessage(cause));
      } finally {
        if (folderChildControllersRef.current.get(requestKey) === controller) {
          folderChildControllersRef.current.delete(requestKey);
        }
        setLoadingMoreFolderIds((current) => {
          const next = new Set(current);
          next.delete(folderId);
          return next;
        });
        inFlightFolderChildPageKeys.current.delete(requestKey);
      }
    },
    [client, folderChildNextCursorByParent, loadingMoreFolderIds]
  );

  const loadFolderDescendants = useCallback(
    async (folderId: number) => {
      if (inFlightFolderDescendantIds.current.has(folderId)) {
        return folderDescendantsByParentRef.current[folderId] ?? [];
      }

      inFlightFolderDescendantIds.current.add(folderId);
      const controller = new AbortController();
      folderDescendantControllersRef.current.set(folderId, controller);
      setLoadingFolderDescendantIds((current) => new Set(current).add(folderId));
      try {
        const page = await client.listFolderChildren(folderId, {
          includeDescendants: true,
          limit: FOLDER_PAGE_LIMIT
        }, {
          signal: controller.signal
        });
        setFolderDescendantsByParent((current) => ({
          ...current,
          [folderId]: page.items
        }));
        await yieldToBrowser();
        return page.items;
      } catch (cause) {
        if (isAbortError(cause)) {
          return folderDescendantsByParentRef.current[folderId] ?? [];
        }
        throw cause;
      } finally {
        inFlightFolderDescendantIds.current.delete(folderId);
        if (folderDescendantControllersRef.current.get(folderId) === controller) {
          folderDescendantControllersRef.current.delete(folderId);
        }
        setLoadingFolderDescendantIds((current) => {
          const next = new Set(current);
          next.delete(folderId);
          return next;
        });
      }
    },
    [client]
  );

  const updateFolderNavigationHistory = useCallback(
    (updater: (current: FolderNavigationHistory) => FolderNavigationHistory) => {
      setFolderNavigationHistory((current) => {
        const next = updater(current);
        folderNavigationHistoryRef.current = next;
        return next;
      });
    },
    []
  );

  const initializeFolderNavigationHistory = useCallback(
    (entry: FolderNavigationEntry | null) => {
      if (!entry) {
        return;
      }
      updateFolderNavigationHistory((current) => {
        if (current.index >= 0 && current.entries.length > 0) {
          return current;
        }
        return { entries: [entry], index: 0 };
      });
    },
    [updateFolderNavigationHistory]
  );

  const pushFolderNavigationEntry = useCallback(
    (entry: FolderNavigationEntry) => {
      updateFolderNavigationHistory((current) => {
        const activeEntry = current.entries[current.index];
        if (sameFolderNavigationEntry(activeEntry, entry)) {
          return current;
        }
        const entries =
          current.index >= 0 ? current.entries.slice(0, current.index + 1) : [];
        entries.push(entry);
        return { entries, index: entries.length - 1 };
      });
    },
    [updateFolderNavigationHistory]
  );

  const setFolderNavigationIndex = useCallback(
    (index: number) => {
      updateFolderNavigationHistory((current) => {
        if (index < 0 || index >= current.entries.length || index === current.index) {
          return current;
        }
        return { ...current, index };
      });
    },
    [updateFolderNavigationHistory]
  );

  const loadRoots = useCallback(async (scope?: LibrarySelectionScope) => {
    const response = await client.listRoots();
    setRoots(response.items);
    const requestedRootId =
      scope && "rootId" in scope ? scope.rootId ?? null : selectedRootIdRef.current;
    const nextRootId = requestedRootId ?? response.items[0]?.id ?? null;
    selectRoot(nextRootId);
    const nextRoot = response.items.find((root) => root.id === nextRootId) ?? response.items[0];
    const nextFolderId =
      scope && "folderId" in scope
        ? scope.folderId ?? null
        : selectedFolderIdRef.current ?? nextRoot?.rootFolderId ?? null;
    selectedRootIdRef.current = nextRootId;
    selectedFolderIdRef.current = nextFolderId;
    selectFolder(nextFolderId);
    initializeFolderNavigationHistory(
      nextRootId === null ? null : { rootId: nextRootId, folderId: nextFolderId }
    );
    return { roots: response.items, selectedRoot: nextRoot ?? null, selectedFolderId: nextFolderId };
  }, [client, initializeFolderNavigationHistory]);

  // Refresh only the roots LIST (for status / lastScanAt display), WITHOUT
  // re-driving selection or navigation history. The 800ms scan refresh used the
  // full `loadRoots`, which re-applied a selection captured from a stale
  // refresh token — if the user clicked a different folder mid-refresh, the
  // in-flight refresh snapped the selection/expansion back to the old folder.
  const refreshRootsList = useCallback(async () => {
    const response = await client.listRoots();
    setRoots(response.items);
    return response.items;
  }, [client]);

  const loadTasks = useCallback(async () => {
    if (loadTasksRequestRef.current) {
      return loadTasksRequestRef.current;
    }

    const request = client
      .listTasks()
      .then((response) => {
        setTasks(response.items);
        setTaskPollFailures(0);
        return response.items;
      })
      .finally(() => {
        if (loadTasksRequestRef.current === request) {
          loadTasksRequestRef.current = null;
        }
      });
    loadTasksRequestRef.current = request;
    return request;
  }, [client]);

  const createScanRefreshSelectionToken = useCallback((): ScanRefreshSelectionToken | null => {
    if (selectedRootId === null) {
      return null;
    }

    return {
      rootId: selectedRootId,
      folderId: selectedFolderId,
      version: scanRefreshSelectionVersionRef.current
    };
  }, [selectedFolderId, selectedRootId]);

  const isCurrentScanRefreshSelection = useCallback(
    (token: ScanRefreshSelectionToken) =>
      token.version === scanRefreshSelectionVersionRef.current &&
      token.rootId === selectedRootId &&
      token.folderId === selectedFolderId,
    [selectedFolderId, selectedRootId]
  );

  const loadMediaPage = useCallback(
    async (scope: {
      rootId: number;
      folderId: number | undefined;
      includeDescendants: boolean;
      cursor?: string;
      offset?: number;
      limit: number;
      signal?: AbortSignal;
    }) => {
      const sort = searchStateRef.current.sort;
      return searchActiveRef.current
        ? client.searchMedia(buildSearchParams(searchStateRef.current, debouncedQRef.current, scope), {
            signal: scope.signal
          })
        : client.listMedia({
            cursor: scope.cursor,
            folderId: scope.folderId,
            includeDescendants: scope.includeDescendants,
            limit: scope.limit,
            offset: scope.offset,
            rootId: scope.rootId,
            sort: listMediaSort(sort)
          }, {
            signal: scope.signal
          });
    },
    [client]
  );

  const loadMediaWindow = useCallback(
    async (scope: {
      rootId: number;
      folderId: number | undefined;
      includeDescendants: boolean;
      offset: number;
      limit: number;
      requestGeneration: number;
      selectFirst?: boolean;
    }) => {
      if (scope.requestGeneration !== mediaPageGeneration.current) {
        return;
      }
      const offset = Math.max(0, scope.offset);
      const limit = Math.max(1, Math.min(MEDIA_WINDOW_MAX_LIMIT, scope.limit));
      // Prefer a keyset cursor when this window sequentially follows one we have
      // already loaded (the common deep-scroll case): O(1) instead of O(offset).
      // Search results are OFFSET-only, so never use a cursor there.
      const keysetCursor = searchActiveRef.current
        ? undefined
        : mediaCursorByStartIndexRef.current.get(offset);
      const requestKey = mediaPageRequestKey(
        scope.rootId,
        scope.folderId,
        keysetCursor ? `cursor:${keysetCursor}:${limit}` : `offset:${offset}:${limit}`,
        scope.includeDescendants,
        scope.requestGeneration
      );
      abortStaleMediaPageRequests(requestKey);
      if (
        inFlightMediaPageKeys.current.has(requestKey) ||
        loadedMediaPageKeys.current.has(requestKey)
      ) {
        return;
      }

      inFlightMediaPageKeys.current.add(requestKey);
      const controller = new AbortController();
      mediaPageControllersRef.current.set(requestKey, controller);
      setLoadingMoreMedia(true);
      try {
        const mediaPage = await loadMediaPage({
          folderId: scope.folderId,
          includeDescendants: scope.includeDescendants,
          limit,
          // Keyset when following a loaded page; OFFSET for the first page or a
          // random jump with no cached cursor. Passing only one keeps the Core
          // query on its intended path.
          cursor: keysetCursor,
          offset: keysetCursor ? undefined : offset,
          rootId: scope.rootId,
          signal: controller.signal
        });
        if (scope.requestGeneration !== mediaPageGeneration.current) {
          return;
        }
        const totalCount = Math.max(
          mediaPage.totalCount ?? mediaTotalCountRef.current,
          offset + mediaPage.items.length
        );
        setMediaTotalCount(totalCount);
        setMediaSlots((current) => {
          return applyMediaWindowToSlots(current, totalCount, offset, mediaPage.items);
        });
        setMedia((current) => appendUniqueMedia(current, mediaPage.items));
        setLoadedMediaRanges((current) =>
          mergeLoadedMediaRanges(current, offset, offset + mediaPage.items.length)
        );
        loadedMediaPageKeys.current.add(requestKey);
        setMediaNextCursor(mediaPage.nextCursor);
        // Remember the cursor that fetches the page right after this window, so
        // the next sequential scroll step can keyset past it. Only meaningful on
        // the non-search path (search cursors are not offset-addressable here).
        if (!searchActiveRef.current && mediaPage.nextCursor) {
          mediaCursorByStartIndexRef.current.set(
            offset + mediaPage.items.length,
            mediaPage.nextCursor
          );
        }
        if (scope.selectFirst) {
          selectMedia((current) => current ?? mediaPage.items[0]?.id ?? null);
        }
        await yieldToBrowser();
      } catch (cause) {
        if (isAbortError(cause)) {
          return;
        }
        if (scope.requestGeneration === mediaPageGeneration.current) {
          setError(errorMessage(cause));
        }
      } finally {
        if (mediaPageControllersRef.current.get(requestKey) === controller) {
          mediaPageControllersRef.current.delete(requestKey);
        }
        inFlightMediaPageKeys.current.delete(requestKey);
        if (scope.requestGeneration === mediaPageGeneration.current) {
          setLoadingMoreMedia(mediaPageControllersRef.current.size > 0);
        }
      }
    },
    [abortStaleMediaPageRequests, loadMediaPage]
  );

  const requestMediaWindow = useCallback(
    (startIndex: number, endIndex: number) => {
      const totalCount = mediaTotalCountRef.current;
      const clampedStart = Math.max(0, Math.floor(startIndex));
      const clampedEnd = Math.max(clampedStart, Math.ceil(endIndex));
      if (totalCount <= 0 || clampedStart >= totalCount) {
        return;
      }
      const pageOffset = clampedStart;
      const pageEnd = Math.min(
        totalCount,
        pageOffset + Math.min(MEDIA_WINDOW_MAX_LIMIT, Math.max(1, clampedEnd - pageOffset))
      );
      const missingRange = firstMissingMediaRange(
        loadedMediaRangesRef.current,
        pageOffset,
        pageEnd
      );
      if (!missingRange) {
        return;
      }
      const rootId = selectedRootIdRef.current;
      if (rootId === null) {
        return;
      }
      const folderId = selectedFolderIdRef.current;
      const folderFilter = folderId !== null && folderId !== undefined ? folderId : undefined;
      void loadMediaWindow({
        rootId,
        folderId: folderFilter,
        includeDescendants: showChildFolderContentsRef.current,
        offset: missingRange.start,
        limit: missingRange.end - missingRange.start,
        requestGeneration: mediaPageGeneration.current
      });
    },
    [loadMediaWindow]
  );

  const reloadCurrentMedia = useCallback(
    async (scope?: {
      rootId?: number;
      folderId?: number | null;
      includeDescendants?: boolean;
      scanRefreshSelectionToken?: ScanRefreshSelectionToken;
      setLoadingState?: boolean;
      // Background refresh (scan progress / task-status change): update the
      // media total so the scrollable area grows to include newly-scanned
      // files, but do NOT tear down loaded slots, abort in-flight windows, bump
      // the page generation, or reset scroll. A full teardown here (the old
      // behavior) blanked the grid and snapped scroll to top every 800ms during
      // a scan, and dropped a user's just-issued folder load by invalidating its
      // generation. The windowed pagination + thumbnail-state polling already
      // fill in new/changed tiles in place as the user scrolls.
      incremental?: boolean;
    }) => {
      const incremental = scope?.incremental ?? false;
      if (incremental) {
        const rootId = scope?.rootId ?? selectedRootIdRef.current;
        if (rootId === null || rootId === undefined) {
          return;
        }
        const scanRefreshSelectionToken = scope?.scanRefreshSelectionToken;
        if (
          scanRefreshSelectionToken &&
          !isCurrentScanRefreshSelection(scanRefreshSelectionToken)
        ) {
          return;
        }
        const folderId = scope && "folderId" in scope ? scope.folderId : selectedFolderIdRef.current;
        const includeDescendants =
          scope?.includeDescendants ?? showChildFolderContentsRef.current;
        const folderFilter = folderId !== null && folderId !== undefined ? folderId : undefined;
        // A limit=1 offset=0 read is the cheap first page: Core computes the
        // fresh COUNT there, so we learn the new total without disturbing any
        // loaded window. We never write its single item into slots (slot 0 is
        // already loaded); we only lift the total upward so newly-scanned files
        // become reachable by scrolling.
        const page = await loadMediaPage({
          folderId: folderFilter,
          includeDescendants,
          limit: 1,
          offset: 0,
          rootId,
          signal: undefined
        });
        if (
          scanRefreshSelectionToken &&
          !isCurrentScanRefreshSelection(scanRefreshSelectionToken)
        ) {
          return;
        }
        if (typeof page.totalCount === "number") {
          setMediaTotalCount((current) => Math.max(current, page.totalCount ?? current));
        }
        return;
      }

      const requestGeneration = ++mediaPageGeneration.current;
      abortStaleMediaPageRequests();
      const rootId = scope?.rootId ?? selectedRootId;
      const setLoadingState = scope?.setLoadingState ?? false;
      if (setLoadingState) {
        setLoading(true);
      }
      if (rootId === null) {
        if (setLoadingState) {
          setLoading(false);
        }
        return;
      }

      const scanRefreshSelectionToken = scope?.scanRefreshSelectionToken;
      if (
        scanRefreshSelectionToken &&
        !isCurrentScanRefreshSelection(scanRefreshSelectionToken)
      ) {
        return;
      }

      const root = roots.find((item) => item.id === rootId);
      if (!root) {
        if (setLoadingState) {
          setLoading(false);
        }
        return;
      }

      const folderId = scope && "folderId" in scope ? scope.folderId : selectedFolderId;
      const includeDescendants =
        scope?.includeDescendants ?? showChildFolderContentsRef.current;
      const folderFilter = folderId !== null && folderId !== undefined ? folderId : undefined;
      inFlightMediaPageKeys.current.clear();
      loadedMediaPageKeys.current.clear();
      setMedia([]);
      setMediaSlots(new Map());
      setMediaTotalCount(0);
      setLoadedMediaRanges([]);
      mediaCursorByStartIndexRef.current.clear();
      setMediaNextCursor(null);
      setError(null);

      try {
        await loadMediaWindow({
          rootId: root.id,
          folderId: folderFilter,
          includeDescendants,
          offset: 0,
          limit: INITIAL_MEDIA_WINDOW_LIMIT,
          requestGeneration,
          selectFirst: true
        });
        if (
          requestGeneration !== mediaPageGeneration.current ||
          (scanRefreshSelectionToken &&
            !isCurrentScanRefreshSelection(scanRefreshSelectionToken))
        ) {
          return;
        }
      } catch (cause) {
        setError(errorMessage(cause));
        throw cause;
      } finally {
        if (
          setLoadingState &&
          requestGeneration === mediaPageGeneration.current &&
          (!scanRefreshSelectionToken ||
            isCurrentScanRefreshSelection(scanRefreshSelectionToken))
        ) {
          setLoading(false);
        }
      }
    },
    [
      isCurrentScanRefreshSelection,
      loadMediaPage,
      loadMediaWindow,
      abortStaleMediaPageRequests,
      roots,
      selectedFolderId,
      selectedRootId
    ]
  );

  const refreshCurrentScanView = useCallback(async () => {
    const selectionToken = createScanRefreshSelectionToken();
    if (!selectionToken || !isCurrentScanRefreshSelection(selectionToken)) {
      return;
    }

    // Refresh only the roots LIST (status/lastScanAt) — do NOT re-select, or a
    // refresh that started before a folder click would snap the selection and
    // tree expansion back to the previously-selected folder.
    const refreshedRoots = await refreshRootsList();
    if (!isCurrentScanRefreshSelection(selectionToken)) {
      return;
    }
    const root = refreshedRoots.find((item) => item.id === selectionToken.rootId) ?? null;
    const resolvedFolderId = selectionToken.folderId ?? root?.rootFolderId ?? null;
    // Only ensure the currently-selected folder stays expanded; never re-drive
    // selection here.
    if (resolvedFolderId !== null && selectionToken.folderId !== null) {
      setExpandedFolderIds((current) =>
        current.has(resolvedFolderId) ? current : new Set(current).add(resolvedFolderId)
      );
    }

    if (selectionToken.folderId !== null || resolvedFolderId !== null) {
      const folderId = selectionToken.folderId ?? resolvedFolderId;
      if (folderId === null) {
        return;
      }
      // Refresh the folder tree FIRST, before tasks and media. The tree does not
      // depend on tasks; awaiting loadTasks() first (GET /api/tasks can be slow
      // with a large task table mid-scan) made the sidebar structure wait a full
      // refresh cycle to update. Prioritize directory structure.
      //
      // Force a re-fetch: loadFolderChildren short-circuits on its cache, so once
      // the sidebar cached the few sibling folders indexed early in the scan it
      // would never pick up the rest as they are scanned in (a deep folder tree
      // indexes its first child's contents fully before the next sibling appears).
      // React reconciles by key so already-shown folders do not flicker.
      await loadFolderChildren(folderId, { force: true });
      if (!isCurrentScanRefreshSelection(selectionToken)) {
        return;
      }
      if (root?.rootFolderId && root.rootFolderId !== folderId) {
        await loadFolderChildren(root.rootFolderId, { force: true });
      }
      // Tasks/scan progress are not needed for the tree; refresh them after the
      // structure is on screen (fire without blocking the media reload below).
      void loadTasks();
      await reloadCurrentMedia({
        rootId: selectionToken.rootId,
        folderId: selectionToken.folderId ?? resolvedFolderId,
        scanRefreshSelectionToken: selectionToken,
        incremental: true
      });
      return;
    }

    void loadTasks();
    await reloadCurrentMedia({
      rootId: selectionToken.rootId,
      folderId: null,
      scanRefreshSelectionToken: selectionToken,
      incremental: true
    });
  }, [
    createScanRefreshSelectionToken,
    isCurrentScanRefreshSelection,
    loadFolderChildren,
    refreshRootsList,
    loadTasks,
    reloadCurrentMedia
  ]);

  const loadLibrary = useCallback(async (scope?: LibrarySelectionScope) => {
    const requestGeneration = ++mediaPageGeneration.current;
    abortStaleMediaPageRequests();
    abortStaleFolderRequests();
    inFlightMediaPageKeys.current.clear();
    loadedMediaPageKeys.current.clear();
    setLoading(true);
    setError(null);
    try {
      const result = await loadRoots(scope);
      const root =
        result.roots.find((item) => item.id === selectedRootIdRef.current) ?? result.selectedRoot;
      const folderId =
        scope && "folderId" in scope
          ? scope.folderId ?? null
          : result.selectedFolderId ?? root?.rootFolderId ?? null;
      const includeDescendants = showChildFolderContentsRef.current;

      // Load the folder tree structure BEFORE tasks so the sidebar appears
      // immediately; tasks (scan progress) are not needed to render it and
      // GET /api/tasks can be slow mid-scan.
      if (folderId) {
        setExpandedFolderIds((current) => new Set(current).add(folderId));
        await loadFolderChildren(folderId);
      } else {
        setFolderChildrenByParent({});
        setFolderDescendantsByParent({});
        setFolderChildNextCursorByParent({});
      }
      void loadTasks();

      if (root) {
        const folderFilter = folderId !== null && folderId !== undefined ? folderId : undefined;
        setMedia([]);
      setMediaSlots(new Map());
        setMediaTotalCount(0);
        setLoadedMediaRanges([]);
        mediaCursorByStartIndexRef.current.clear();
        setMediaNextCursor(null);
        await loadMediaWindow({
          rootId: root.id,
          folderId: folderFilter,
          includeDescendants,
          offset: 0,
          limit: INITIAL_MEDIA_WINDOW_LIMIT,
          requestGeneration,
          selectFirst: true
        });
      } else {
        setMedia([]);
        setMediaSlots(new Map());
        setMediaTotalCount(0);
        setLoadedMediaRanges([]);
        mediaCursorByStartIndexRef.current.clear();
        setMediaNextCursor(null);
        selectMedia(null);
      }
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setLoading(false);
    }
  }, [
    loadMediaWindow,
    abortStaleFolderRequests,
    abortStaleMediaPageRequests,
    loadFolderChildren,
    loadRoots,
    loadTasks
  ]);

  useEffect(() => {
    void loadLibrary();
  }, [loadLibrary]);

  useEffect(() => {
    seenThumbnailPersistActiveRef.current = true;
    return () => {
      if (thumbnailPriorityScopeSyncTimerRef.current !== null) {
        window.clearTimeout(thumbnailPriorityScopeSyncTimerRef.current);
        thumbnailPriorityScopeSyncTimerRef.current = null;
      }
      thumbnailPriorityScopeSyncTrailingPendingRef.current = false;
      seenThumbnailPersistActiveRef.current = false;
      seenThumbnailPersistIdsRef.current.clear();
      if (seenThumbnailPersistFlushTimerRef.current !== null) {
        window.clearTimeout(seenThumbnailPersistFlushTimerRef.current);
        seenThumbnailPersistFlushTimerRef.current = null;
      }
      seenThumbnailPersistControllerRef.current?.abort();
      seenThumbnailPersistControllerRef.current = null;
      abortAllControllers(mediaPageControllersRef.current);
      abortAllControllers(folderChildControllersRef.current);
      abortAllControllers(folderDescendantControllersRef.current);
      abortAllControllers(thumbnailStateControllersRef.current);
      interactiveFolderScanControllerRef.current?.abort();
      thumbnailPriorityScopeSyncControllerRef.current?.abort();
      thumbnailCacheStatsControllerRef.current?.abort();
      thumbnailCacheActionControllerRef.current?.abort();
      if (thumbnailCacheAutoRefreshTimerRef.current !== null) {
        window.clearTimeout(thumbnailCacheAutoRefreshTimerRef.current);
        thumbnailCacheAutoRefreshTimerRef.current = null;
      }
    };
  }, []);

  // Debounce the search query input
  useEffect(() => {
    if (searchState.q === debouncedQ) {
      return;
    }
    const handle = window.setTimeout(() => {
      setDebouncedQ(searchState.q);
    }, SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(handle);
  }, [searchState.q, debouncedQ]);

  useEffect(() => {
    try {
      window.localStorage.setItem(
        SHOW_SUBFOLDER_CONTENTS_STORAGE_KEY,
        showChildFolderContents ? "1" : "0"
      );
    } catch {
      // Ignore storage failures.
    }
  }, [showChildFolderContents]);

  // Reload media when filters/sort/debounced-q change
  useEffect(() => {
    void loadLibrary();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    debouncedQ,
    searchState.kind,
    searchState.minRating,
    searchState.favorite,
    searchState.sort,
    searchState.tagIds.join(",")
  ]);

  // Load tag list once and refresh on demand via createTag/deleteTag.
  useEffect(() => {
    let cancelled = false;
    void client
      .listTags()
      .then((response) => {
        if (!cancelled) {
          setTags(response.items);
        }
      })
      .catch((cause) => {
        if (!cancelled) {
          setError(errorMessage(cause));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [client]);

  // Load user metadata for the currently selected media item.
  useEffect(() => {
    if (selectedMediaId === null) {
      setSelectedMetadata(null);
      return;
    }
    let cancelled = false;
    const requestGeneration = metadataEditGenerationRef.current;
    const requestMediaId = selectedMediaId;
    void client
      .getUserMetadata(selectedMediaId)
      .then((record) => {
        if (
          !cancelled &&
          selectedMediaIdRef.current === requestMediaId &&
          metadataEditGenerationRef.current === requestGeneration
        ) {
          setSelectedMetadata(record);
        }
      })
      .catch((cause) => {
        if (!cancelled) {
          setError(errorMessage(cause));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [client, selectedMediaId]);

  useEffect(() => {
    if (!selectedMedia) {
      abortThumbnailStateControllersForPriority("selected");
      scheduleThumbnailPriorityScopeSync("selected", []);
      return;
    }
    requestThumbnailStates([selectedMedia.id], "selected");
  }, [
    requestThumbnailStates,
    abortThumbnailStateControllersForPriority,
    scheduleThumbnailPriorityScopeSync,
    selectedMedia,
    selectedMediaThumbnailRequestKey
  ]);

  useEffect(() => {
    if (!selectedMedia || !shouldRepollThumbnailState(selectedMedia, selectedThumbnail)) {
      return;
    }

    const timer = window.setInterval(() => {
      requestThumbnailStates([selectedMedia.id], "selected");
    }, SELECTED_THUMBNAIL_REPOLL_MS);
    return () => window.clearInterval(timer);
  }, [
    requestThumbnailStates,
    selectedMedia,
    selectedMediaThumbnailRequestKey,
    selectedThumbnail?.state,
    selectedThumbnail?.updatedAt
  ]);

  useEffect(() => {
    if (!options.persistentThumbnailCacheAutoRefresh) {
      if (thumbnailCacheAutoRefreshTimerRef.current !== null) {
        window.clearTimeout(thumbnailCacheAutoRefreshTimerRef.current);
        thumbnailCacheAutoRefreshTimerRef.current = null;
      }
      lastThumbnailCacheAutoRefreshScopeKeyRef.current = null;
      return;
    }

    const scope = currentThumbnailCacheScope();
    if (!scope || scope.folderId === undefined) {
      return;
    }

    const scopeKey = `${scope.rootId}:${scope.folderId}:${scope.includeDescendants ? "tree" : "folder"}`;
    if (lastThumbnailCacheAutoRefreshScopeKeyRef.current === scopeKey) {
      return;
    }
    if (thumbnailCacheAutoRefreshTimerRef.current !== null) {
      window.clearTimeout(thumbnailCacheAutoRefreshTimerRef.current);
      thumbnailCacheAutoRefreshTimerRef.current = null;
    }

    let cancelled = false;
    thumbnailCacheAutoRefreshTimerRef.current = window.setTimeout(() => {
      if (cancelled) {
        return;
      }
      lastThumbnailCacheAutoRefreshScopeKeyRef.current = scopeKey;
      void client
        .enqueueThumbnailCache(
          {
            ...scope,
            refreshMode: "staleOrMissing",
            limit: MEDIA_WINDOW_MAX_LIMIT
          },
          { requestPriority: "background" }
        )
        .then(() => {
          // Refresh real whole-library stats (the enqueue response no longer
          // carries whole-scope stats — that query blocked the writer).
          void refreshThumbnailCacheStats();
        })
        .catch((cause) => {
          if (!isAbortError(cause)) {
            setError(errorMessage(cause));
          }
        });
    }, INTERACTIVE_FOLDER_SCAN_DEBOUNCE_MS);

    return () => {
      cancelled = true;
      if (thumbnailCacheAutoRefreshTimerRef.current !== null) {
        window.clearTimeout(thumbnailCacheAutoRefreshTimerRef.current);
        thumbnailCacheAutoRefreshTimerRef.current = null;
      }
    };
  }, [client, currentThumbnailCacheScope, options.persistentThumbnailCacheAutoRefresh, refreshThumbnailCacheStats, selectedFolderId, selectedRootId, showChildFolderContents]);

  useEffect(() => {
    interactiveFolderScanControllerRef.current?.abort();
    interactiveFolderScanControllerRef.current = null;
    if (selectedFolderId === null) {
      pendingInteractiveScanRequestRef.current = null;
      return;
    }

    // Never recursively "interactive-scan" the root folder itself for very
    // large libraries. On million-file roots this duplicates the background
    // root scan and blocks folder browsing without adding useful disclosure.
    if (selectedRoot?.rootFolderId === selectedFolderId) {
      pendingInteractiveScanRequestRef.current = null;
      return;
    }

    const rootId = selectedRootIdRef.current;
    if (rootId === null) {
      pendingInteractiveScanRequestRef.current = null;
      return;
    }

    pendingInteractiveScanRequestRef.current = {
      rootId,
      folderId: selectedFolderId,
      knownTaskIds: new Set(
        tasksRef.current
          .filter(
            (task) =>
              task.kind === "interactive_folder_scan" &&
              task.rootId === rootId &&
              task.folderId === selectedFolderId
          )
          .map((task) => task.id)
      )
    };

    let cancelled = false;
    const controller = new AbortController();
    interactiveFolderScanControllerRef.current = controller;
    const timer = window.setTimeout(() => {
      if (
        cancelled ||
        selectedRootIdRef.current !== rootId ||
        selectedFolderIdRef.current !== selectedFolderId
      ) {
        return;
      }
      void client
        .enqueueInteractiveFolderScan(selectedFolderId, { signal: controller.signal })
        .catch((cause) => {
          if (!cancelled && !isAbortError(cause)) {
            pendingInteractiveScanRequestRef.current = null;
            setError(errorMessage(cause));
          }
        });
    }, INTERACTIVE_FOLDER_SCAN_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      controller.abort();
      if (interactiveFolderScanControllerRef.current === controller) {
        interactiveFolderScanControllerRef.current = null;
      }
    };
  }, [client, selectedFolderId, selectedRoot?.rootFolderId]);

  const previousTaskStatusRef = useRef<Map<number, TaskRecord["status"]>>(new Map());
  useEffect(() => {
    const previous = previousTaskStatusRef.current;
    let shouldReloadLibrary = false;
    let interactiveRefreshScope: { rootId: number; folderId: number | null } | null = null;
    const pendingInteractiveScan = pendingInteractiveScanRequestRef.current;
    for (const task of tasks) {
      const prior = previous.get(task.id);
      const transitionedToSucceeded =
        Boolean(prior) && prior !== task.status && task.status === "succeeded";
      if (transitionedToSucceeded && task.kind === "root_scan" && task.rootId === selectedRootId) {
        shouldReloadLibrary = true;
        break;
      }
      if (
        pendingInteractiveScan &&
        task.kind === "interactive_folder_scan" &&
        task.rootId !== null &&
        task.status === "succeeded" &&
        task.rootId === pendingInteractiveScan.rootId &&
        task.folderId === pendingInteractiveScan.folderId
      ) {
        const firstObservedSucceeded =
          !prior && !pendingInteractiveScan.knownTaskIds.has(task.id);
        if (transitionedToSucceeded || firstObservedSucceeded) {
          interactiveRefreshScope = {
            rootId: task.rootId,
            folderId: task.folderId
          };
          pendingInteractiveScanRequestRef.current = null;
        }
      }
    }
    previousTaskStatusRef.current = new Map(tasks.map((task) => [task.id, task.status]));
    if (shouldReloadLibrary) {
      void loadLibrary();
      return;
    }
    if (interactiveRefreshScope) {
      void reloadCurrentMedia({
        rootId: interactiveRefreshScope.rootId,
        folderId: interactiveRefreshScope.folderId,
        incremental: true
      }).catch((cause) => {
        setError(errorMessage(cause));
      });
    }
  }, [loadLibrary, reloadCurrentMedia, selectedFolderId, selectedRootId, tasks]);

  useEffect(() => {
    if (!scanActive || taskPollFailures >= 3) {
      return;
    }

    const timer = window.setInterval(() => {
      void loadTasks().catch((cause) => {
        setTaskPollFailures((failures) => failures + 1);
        setError(errorMessage(cause));
      });
    }, 1000);
    return () => window.clearInterval(timer);
  }, [loadTasks, scanActive, taskPollFailures]);

  useEffect(() => {
    if (!scanActiveRootTask || scanRefreshFailures >= 3) {
      return;
    }

    const timer = window.setInterval(() => {
      if (scanRefreshInFlightRef.current) {
        return;
      }
      scanRefreshInFlightRef.current = true;
      void refreshCurrentScanView()
        .then(() => {
          setScanRefreshFailures(0);
        })
        .catch((cause) => {
          setScanRefreshFailures((failures) => failures + 1);
          setError(errorMessage(cause));
        })
        .finally(() => {
          scanRefreshInFlightRef.current = false;
        });
    }, SCAN_REFRESH_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [refreshCurrentScanView, scanActiveRootTask, scanRefreshFailures]);

  useEffect(() => {
    if (!scanActiveRootTask) {
      scanRefreshActiveRootIdRef.current = null;
      return;
    }

    if (scanRefreshActiveRootIdRef.current !== selectedRootId) {
      scanRefreshActiveRootIdRef.current = selectedRootId;
      setScanRefreshFailures(0);
    }
  }, [scanActiveRootTask, selectedRootId]);

  const refresh = useCallback(async () => {
    await loadLibrary();
  }, [loadLibrary]);

  const prepareNavigationMediaReload = useCallback(() => {
    mediaPageGeneration.current += 1;
    scanRefreshSelectionVersionRef.current += 1;
    abortStaleMediaPageRequests();
    abortStaleFolderRequests();
    abortAllControllers(thumbnailStateControllersRef.current);
    thumbnailPriorityScopeSyncControllerRef.current?.abort();
    thumbnailPriorityScopeSyncControllerRef.current = null;
    if (thumbnailPriorityScopeSyncTimerRef.current !== null) {
      window.clearTimeout(thumbnailPriorityScopeSyncTimerRef.current);
      thumbnailPriorityScopeSyncTimerRef.current = null;
    }
    thumbnailPriorityScopeSyncTrailingPendingRef.current = false;
    setScanRefreshFailures(0);
    setLoading(true);
    setLoadingMoreMedia(false);
    setError(null);
    setMedia([]);
    setMediaNextCursor(null);
    setSelectedFolderInfoState(null);
    selectMedia(null);
    thumbnailPriorityScopeRef.current = emptyThumbnailPriorityScope();
    thumbnailPriorityScopeSyncKeyRef.current = null;
  }, [abortStaleFolderRequests, abortStaleMediaPageRequests]);

  const selectLibraryFolder = useCallback(
    (entry: FolderNavigationEntry, options?: { recordHistory?: boolean }) => {
      const recordHistory = options?.recordHistory ?? true;
      prepareNavigationMediaReload();
      selectedRootIdRef.current = entry.rootId;
      selectedFolderIdRef.current = entry.folderId;
      selectRoot(entry.rootId);
      selectFolder(entry.folderId);
      if (recordHistory) {
        pushFolderNavigationEntry(entry);
      }
      if (entry.folderId !== null) {
        setExpandedFolderIds((current) => new Set(current).add(entry.folderId as number));
        void loadFolderChildren(entry.folderId).catch((cause) => {
          setError(errorMessage(cause));
        });
      }
      void reloadCurrentMedia({
        rootId: entry.rootId,
        folderId: entry.folderId,
        includeDescendants: showChildFolderContentsRef.current,
        setLoadingState: true
      }).catch((cause) => {
        setError(errorMessage(cause));
      });
    },
    [
      loadFolderChildren,
      prepareNavigationMediaReload,
      pushFolderNavigationEntry,
      reloadCurrentMedia
    ]
  );

  const navigateFolderHistory = useCallback(
    (direction: -1 | 1) => {
      const history = folderNavigationHistoryRef.current;
      const nextIndex = history.index + direction;
      const entry = history.entries[nextIndex];
      if (!entry) {
        return;
      }
      setFolderNavigationIndex(nextIndex);
      selectLibraryFolder(entry, { recordHistory: false });
    },
    [selectLibraryFolder, setFolderNavigationIndex]
  );

  const addRoot = useCallback(
    async (path: string) => {
      const trimmedPath = path.trim();
      if (!trimmedPath) return;

      setAddingRoot(true);
      setError(null);
      try {
        const response = await client.addRoot(trimmedPath);
        setLastScan(response.scan);
        if (response.rootId) {
          scanRefreshSelectionVersionRef.current += 1;
          setScanRefreshFailures(0);
          selectedRootIdRef.current = response.rootId;
          selectedFolderIdRef.current = null;
          selectRoot(response.rootId);
          selectFolder(null);
          setExpandedFolderIds(new Set());
          setFolderDescendantsByParent({});
          setFolderChildNextCursorByParent({});
        }
        await loadTasks();
        await loadLibrary(response.rootId ? { rootId: response.rootId, folderId: null } : undefined);
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        setAddingRoot(false);
      }
    },
    [client, loadLibrary, loadTasks]
  );

  const loadMoreMedia = useCallback(async () => {
    const loadedEnd = loadedMediaRangesRef.current.reduce(
      (maxEnd, range) => Math.max(maxEnd, range.end),
      0
    );
    requestMediaWindow(loadedEnd, loadedEnd + INITIAL_MEDIA_WINDOW_LIMIT);
  }, [requestMediaWindow]);

  const rescanRoot = useCallback(
    async (rootId: number) => {
      setRescanningRootIds((current) => new Set(current).add(rootId));
      setError(null);
      try {
        await client.enqueueScan(rootId);
        await loadTasks();
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        setRescanningRootIds((current) => {
          const next = new Set(current);
          next.delete(rootId);
          return next;
        });
      }
    },
    [client, loadTasks]
  );

  const markTaskBusy = useCallback((taskId: number, busy: boolean) => {
    setBusyTaskIds((current) => {
      const next = new Set(current);
      if (busy) {
        next.add(taskId);
      } else {
        next.delete(taskId);
      }
      return next;
    });
  }, []);

  const cancelTask = useCallback(
    async (taskId: number) => {
      markTaskBusy(taskId, true);
      setError(null);
      try {
        await client.cancelTask(taskId);
        await loadTasks();
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        markTaskBusy(taskId, false);
      }
    },
    [client, loadTasks, markTaskBusy]
  );

  const retryTask = useCallback(
    async (taskId: number) => {
      markTaskBusy(taskId, true);
      setError(null);
      try {
        await client.retryTask(taskId);
        await loadTasks();
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        markTaskBusy(taskId, false);
      }
    },
    [client, loadTasks, markTaskBusy]
  );

  const refreshTasks = useCallback(async () => {
    try {
      await loadTasks();
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }, [loadTasks]);

  const setQ = useCallback((q: string) => {
    setSearchState((current) => (current.q === q ? current : { ...current, q }));
  }, []);

  const setKind = useCallback((kind: MediaKindFilter | undefined) => {
    setSearchState((current) => ({ ...current, kind }));
  }, []);

  const setMinRating = useCallback((rating: MinRatingFilter | undefined) => {
    setSearchState((current) => ({ ...current, minRating: rating }));
  }, []);

  const toggleFavoriteFilter = useCallback(() => {
    setSearchState((current) => ({
      ...current,
      favorite: current.favorite === true ? undefined : true
    }));
  }, []);

  const toggleTagFilter = useCallback((tagId: number) => {
    setSearchState((current) => {
      const has = current.tagIds.includes(tagId);
      return {
        ...current,
        tagIds: has ? current.tagIds.filter((id) => id !== tagId) : [...current.tagIds, tagId]
      };
    });
  }, []);

  const setSort = useCallback((sort: LibrarySort) => {
    setSearchState((current) => (current.sort === sort ? current : { ...current, sort }));
  }, []);

  const clearFilters = useCallback(() => {
    setSearchState((current) => ({
      q: "",
      kind: undefined,
      minRating: undefined,
      favorite: undefined,
      tagIds: [],
      sort: current.sort
    }));
    setDebouncedQ("");
  }, []);

  const reflectMetadataLocally = useCallback(
    (fileId: number, patch: UserMetadataUpdate) => {
      setSelectedMetadata((current) => {
        if (current && current.fileId !== fileId) return current;
        if (!current && selectedMediaIdRef.current !== fileId) return current;
        const base = current ?? emptyUserMetadataRecord(fileId);
        const next: UserMetadataRecord = { ...base };
        if ("rating" in patch) next.rating = patch.rating ?? null;
        if ("favorite" in patch && patch.favorite !== undefined) next.favorite = patch.favorite;
        if ("note" in patch) next.note = patch.note ?? null;
        next.updatedAt = Date.now();
        return next;
      });
      if ("favorite" in patch && patch.favorite !== undefined) {
        const favorite = patch.favorite;
        setMedia((current) =>
          current.map((item) => (item.id === fileId ? { ...item, favorite } : item))
        );
      }
      if ("rating" in patch) {
        const rating = patch.rating ?? null;
        setMedia((current) =>
          current.map((item) => (item.id === fileId ? { ...item, rating } : item))
        );
      }
    },
    []
  );

  const updateMetadata = useCallback(
    async (fileId: number, patch: UserMetadataUpdate) => {
      const previousMetadata =
        selectedMetadata && selectedMetadata.fileId === fileId ? selectedMetadata : null;
      const previousMediaRow = media.find((item) => item.id === fileId) ?? null;
      metadataEditGenerationRef.current += 1;
      reflectMetadataLocally(fileId, patch);
      setMetadataSaving(true);
      try {
        const record = await client.updateUserMetadata(fileId, patch);
        reflectMetadataLocally(fileId, confirmedMetadataPatch(patch, record));
      } catch (cause) {
        if (previousMetadata) {
          setSelectedMetadata((current) =>
            current && current.fileId === fileId ? previousMetadata : current
          );
        }
        if (previousMediaRow) {
          setMedia((current) =>
            current.map((item) => (item.id === fileId ? previousMediaRow : item))
          );
        }
        setError(errorMessage(cause));
      } finally {
        setMetadataSaving(false);
      }
    },
    [client, media, reflectMetadataLocally, selectedMetadata]
  );

  const reflectFileTagsLocally = useCallback((fileId: number, tagIds: number[]) => {
    setSelectedMetadata((current) => {
      if (current && current.fileId !== fileId) return current;
      if (!current && selectedMediaIdRef.current !== fileId) return current;
      const base = current ?? emptyUserMetadataRecord(fileId);
      return { ...base, tagIds, updatedAt: Date.now() };
    });
    setMedia((current) =>
      current.map((item) => (item.id === fileId ? { ...item, tagIds } : item))
    );
  }, []);

  const capturePreviousTagIds = useCallback(
    (fileId: number): number[] | null => {
      if (selectedMetadata && selectedMetadata.fileId === fileId) {
        return selectedMetadata.tagIds;
      }
      const mediaRow = media.find((item) => item.id === fileId);
      if (mediaRow && mediaRow.tagIds) {
        return [...mediaRow.tagIds];
      }
      return null;
    },
    [media, selectedMetadata]
  );

  const setFileTags = useCallback(
    async (fileId: number, tagIds: number[]) => {
      const previous = capturePreviousTagIds(fileId);
      metadataEditGenerationRef.current += 1;
      reflectFileTagsLocally(fileId, tagIds);
      try {
        const response = await client.setFileTags(fileId, { tagIds });
        reflectFileTagsLocally(fileId, response.tagIds);
      } catch (cause) {
        if (previous) reflectFileTagsLocally(fileId, previous);
        setError(errorMessage(cause));
      }
    },
    [capturePreviousTagIds, client, reflectFileTagsLocally]
  );

  const addFileTag = useCallback(
    async (fileId: number, tagId: number) => {
      const previous = capturePreviousTagIds(fileId);
      const optimistic = previous && previous.includes(tagId) ? previous : [...(previous ?? []), tagId];
      metadataEditGenerationRef.current += 1;
      reflectFileTagsLocally(fileId, optimistic);
      try {
        const response = await client.addFileTag(fileId, { tagId });
        reflectFileTagsLocally(fileId, response.tagIds);
      } catch (cause) {
        if (previous) reflectFileTagsLocally(fileId, previous);
        setError(errorMessage(cause));
      }
    },
    [capturePreviousTagIds, client, reflectFileTagsLocally]
  );

  const removeFileTag = useCallback(
    async (fileId: number, tagId: number) => {
      const previous = capturePreviousTagIds(fileId);
      const optimistic = (previous ?? []).filter((id) => id !== tagId);
      metadataEditGenerationRef.current += 1;
      reflectFileTagsLocally(fileId, optimistic);
      try {
        const response = await client.removeFileTag(fileId, tagId);
        reflectFileTagsLocally(fileId, response.tagIds);
      } catch (cause) {
        if (previous) reflectFileTagsLocally(fileId, previous);
        setError(errorMessage(cause));
      }
    },
    [capturePreviousTagIds, client, reflectFileTagsLocally]
  );

  const createTag = useCallback(
    async (name: string, color?: string | null) => {
      try {
        const tag = await client.createTag({ name, color: color ?? null });
        setTags((current) => {
          if (current.some((existing) => existing.id === tag.id)) return current;
          return [...current, tag].sort((a, b) => a.name.localeCompare(b.name));
        });
        return tag;
      } catch (cause) {
        setError(errorMessage(cause));
        return null;
      }
    },
    [client]
  );

  const deleteTag = useCallback(
    async (tagId: number) => {
      try {
        await client.deleteTag(tagId);
        setTags((current) => current.filter((tag) => tag.id !== tagId));
        setSearchState((current) => ({
          ...current,
          tagIds: current.tagIds.filter((id) => id !== tagId)
        }));
      } catch (cause) {
        setError(errorMessage(cause));
      }
    },
    [client]
  );

  const loadRecentOps = useCallback(async () => {
    setRecentOpsLoading(true);
    try {
      const response = await client.listFileOperations({ limit: 50 });
      setRecentOps(response.items);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setRecentOpsLoading(false);
    }
  }, [client]);

  useEffect(() => {
    void loadRecentOps();
  }, [loadRecentOps]);

  useEffect(() => {
    let cancelled = false;
    void readDesktopDiagnostics().then((result) => {
      if (!cancelled) {
        setDiagnostics(result);
        setDiagnosticsProbed(true);
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const renameFile = useCallback(
    async (fileId: number, newName: string): Promise<FileOpResult> => {
      const previousMedia = media;
      const previousSelectedMediaId = selectedMediaId;
      // Optimistic: update name in grid
      setMedia((current) =>
        current.map((item) => (item.id === fileId ? { ...item, name: newName } : item))
      );
      try {
        await client.renameFileOp({ fileId, newName });
        await Promise.all([loadRecentOps(), loadLibrary()]);
        return { ok: true };
      } catch (cause) {
        setMedia(previousMedia);
        selectMedia(previousSelectedMediaId);
        const result = fileOpFailure(cause);
        setError(result.message ?? "Rename failed");
        return result;
      }
    },
    [client, loadLibrary, loadRecentOps, media, selectedMediaId]
  );

  const renameFolder = useCallback(
    async (folderId: number, newName: string): Promise<FileOpResult> => {
      const previousChildren = folderChildrenByParent;
      const previousDescendants = folderDescendantsByParent;
      // Optimistic: update folder name everywhere it appears
      setFolderChildrenByParent((current) => {
        let changed = false;
        const next: Record<number, FolderRecord[]> = {};
        for (const [parentId, children] of Object.entries(current)) {
          const updated = children.map((child) =>
            child.id === folderId ? { ...child, name: newName } : child
          );
          if (updated.some((child, idx) => child !== children[idx])) {
            changed = true;
          }
          next[Number(parentId)] = updated;
        }
        return changed ? next : current;
      });
      setFolderDescendantsByParent((current) => renameFolderInMap(current, folderId, newName));
      try {
        await client.renameFileOp({ folderId, newName });
        await Promise.all([loadRecentOps(), loadLibrary()]);
        return { ok: true };
      } catch (cause) {
        setFolderChildrenByParent(previousChildren);
        setFolderDescendantsByParent(previousDescendants);
        const result = fileOpFailure(cause);
        setError(result.message ?? "Rename failed");
        return result;
      }
    },
    [client, folderChildrenByParent, folderDescendantsByParent, loadLibrary, loadRecentOps]
  );

  const moveItems = useCallback(
    async (input: {
      fileIds?: number[];
      folderIds?: number[];
      targetFolderId: number;
    }): Promise<FileOpResult> => {
      const fileIds = input.fileIds ?? [];
      const folderIds = input.folderIds ?? [];
      if (fileIds.length === 0 && folderIds.length === 0) {
        return { ok: true };
      }
      const previousMedia = media;
      // Optimistic: drop moved files from current view if their target is different from current folder
      if (fileIds.length > 0 && input.targetFolderId !== selectedFolderId) {
        setMedia((current) => current.filter((item) => !fileIds.includes(item.id)));
      }
      try {
        const response = await client.moveFileOps({
          fileIds: fileIds.length > 0 ? fileIds : undefined,
          folderIds: folderIds.length > 0 ? folderIds : undefined,
          targetFolderId: input.targetFolderId
        });
        await Promise.all([loadRecentOps(), loadLibrary()]);
        return { ok: true, operations: response.operations };
      } catch (cause) {
        setMedia(previousMedia);
        const result = fileOpFailure(cause);
        setError(result.message ?? "Move failed");
        return result;
      }
    },
    [client, loadLibrary, loadRecentOps, media, selectedFolderId]
  );

  const deleteItems = useCallback(
    async (input: {
      fileIds?: number[];
      folderIds?: number[];
      permanent: boolean;
    }): Promise<FileOpResult> => {
      const fileIds = input.fileIds ?? [];
      const folderIds = input.folderIds ?? [];
      if (fileIds.length === 0 && folderIds.length === 0) {
        return { ok: true };
      }
      const previousMedia = media;
      const previousSelectedMediaId = selectedMediaId;
      const previousChildren = folderChildrenByParent;
      const previousDescendants = folderDescendantsByParent;

      // Optimistic: remove deleted items locally
      if (fileIds.length > 0) {
        setMedia((current) => current.filter((item) => !fileIds.includes(item.id)));
        if (selectedMediaId !== null && fileIds.includes(selectedMediaId)) {
          selectMedia(null);
        }
      }
      if (folderIds.length > 0) {
        setFolderChildrenByParent((current) => {
          const next: Record<number, FolderRecord[]> = {};
          for (const [parentId, children] of Object.entries(current)) {
            next[Number(parentId)] = children.filter((child) => !folderIds.includes(child.id));
          }
          return next;
        });
        setFolderDescendantsByParent((current) => removeFoldersFromMap(current, folderIds));
      }

      try {
        const response = await client.deleteFileOps({
          fileIds: fileIds.length > 0 ? fileIds : undefined,
          folderIds: folderIds.length > 0 ? folderIds : undefined,
          permanent: input.permanent
        });
        await Promise.all([loadRecentOps(), loadLibrary()]);
        return { ok: true, operations: response.operations };
      } catch (cause) {
        // Rollback
        setMedia(previousMedia);
        selectMedia(previousSelectedMediaId);
        setFolderChildrenByParent(previousChildren);
        setFolderDescendantsByParent(previousDescendants);
        const result = fileOpFailure(cause);
        setError(result.message ?? "Delete failed");
        return result;
      }
    },
    [
      client,
      folderChildrenByParent,
      folderDescendantsByParent,
      loadLibrary,
      loadRecentOps,
      media,
      selectedMediaId
    ]
  );

  const generateCurrentFolderThumbnailCache = useCallback(async () => {
    const scope = currentThumbnailCacheScope(false);
    if (!scope || scope.folderId === undefined) {
      return;
    }
    await enqueueThumbnailCacheLoop(scope, "staleOrMissing");
  }, [currentThumbnailCacheScope, enqueueThumbnailCacheLoop]);

  const generateCurrentTreeThumbnailCache = useCallback(async () => {
    const scope = currentThumbnailCacheScope(true);
    if (!scope || scope.folderId === undefined) {
      return;
    }
    await enqueueThumbnailCacheLoop(scope, "staleOrMissing");
  }, [currentThumbnailCacheScope, enqueueThumbnailCacheLoop]);

  const generateAllThumbnailCache = useCallback(async () => {
    // Whole-library generation walks the library with a keyset cursor: each
    // request scans one bounded window (Core returns `nextCursor`) and enqueues
    // the candidates in it. This keeps every writer job short so interactive
    // work interleaves, and makes the whole pass one scan total instead of the
    // O(files²) re-scan-from-the-start the folder-scoped loop would do over the
    // entire library. Progress comes from the separately-polled cache stats, so
    // we do not depend on per-batch stats here.
    thumbnailCacheActionControllerRef.current?.abort();
    const controller = new AbortController();
    thumbnailCacheActionControllerRef.current = controller;
    setThumbnailCacheActionBusy(true);
    try {
      let afterFileId = 0;
      while (!controller.signal.aborted) {
        const response = await client.enqueueThumbnailCache(
          {
            refreshMode: "staleOrMissing",
            limit: MEDIA_WINDOW_MAX_LIMIT,
            afterFileId
          },
          { requestPriority: "background", signal: controller.signal }
        );
        if (response.nextCursor === undefined || response.nextCursor === null) {
          break;
        }
        afterFileId = response.nextCursor;
        await yieldToBrowser();
      }
    } catch (cause) {
      if (!isAbortError(cause)) {
        setError(errorMessage(cause));
      }
    } finally {
      if (thumbnailCacheActionControllerRef.current === controller) {
        thumbnailCacheActionControllerRef.current = null;
      }
      setThumbnailCacheActionBusy(false);
      void refreshThumbnailCacheStats();
    }
  }, [client, refreshThumbnailCacheStats]);

  // Whole-library background generation, only in "allPersist" mode. Kicked off
  // once per root (the enqueue is idempotent server-side and bulk-priority, so
  // it yields to interactive/viewport work). "renderedOnly" and "cacheOnly"
  // never trigger it.
  const generationMode = options.thumbnailGenerationMode ?? "renderedOnly";
  const autoGenerateAllTriggeredRootRef = useRef<number | null>(null);
  useEffect(() => {
    if (generationMode !== "allPersist") {
      autoGenerateAllTriggeredRootRef.current = null;
      return;
    }
    if (selectedRootId === null) {
      return;
    }
    if (autoGenerateAllTriggeredRootRef.current === selectedRootId) {
      return;
    }
    autoGenerateAllTriggeredRootRef.current = selectedRootId;
    void generateAllThumbnailCache();
  }, [generationMode, selectedRootId, generateAllThumbnailCache]);

  // When NOT in whole-library mode, cancel any leftover bulk-priority queue from
  // a previous "allPersist"/"generate all" session so the worker does not keep
  // generating thumbnails for files the user isn't looking at. Runs once when
  // the mode becomes renderedOnly/cacheOnly. Viewport/ahead/selected tasks and
  // already-generated blobs are untouched — only the whole-library bulk pending
  // tasks are cancelled. (Fixes: "活动任务 30019" persisting in renderedOnly.)
  const cancelledBulkForModeRef = useRef<ThumbnailGenerationMode | null>(null);
  useEffect(() => {
    if (generationMode === "allPersist") {
      cancelledBulkForModeRef.current = null;
      return;
    }
    if (cancelledBulkForModeRef.current === generationMode) {
      return;
    }
    cancelledBulkForModeRef.current = generationMode;
    void client
      .cancelBulkThumbnailCache({ requestPriority: "background" })
      .then(() => {
        void refreshThumbnailCacheStats();
      })
      .catch((cause) => {
        if (!isAbortError(cause)) {
          // Non-fatal: leftover bulk tasks are only wasted work, not a failure.
          cancelledBulkForModeRef.current = null;
        }
      });
  }, [client, generationMode, refreshThumbnailCacheStats]);

  const retryThumbnailCacheFailures = useCallback(async () => {
    await enqueueThumbnailCacheLoop(currentThumbnailCacheScope() ?? {}, "retryFailedAndStale");
  }, [currentThumbnailCacheScope, enqueueThumbnailCacheLoop]);

  const clearPersistentThumbnailCache = useCallback(async () => {
    try {
      setThumbnailCacheActionBusy(true);
      await client.clearThumbnailCache({ requestPriority: "interactive" });
      await refreshThumbnailCacheStats();
    } catch (cause) {
      if (!isAbortError(cause)) {
        setError(errorMessage(cause));
      }
    } finally {
      setThumbnailCacheActionBusy(false);
    }
  }, [client, refreshThumbnailCacheStats]);

  return {
    roots,
    folders,
    media,
    mediaSlots,
    mediaTotalCount,
    loadedMediaRanges,
    showChildFolderContents,
    canNavigateFolderBack: folderNavigationHistory.index > 0,
    canNavigateFolderForward:
      folderNavigationHistory.index >= 0 &&
      folderNavigationHistory.index < folderNavigationHistory.entries.length - 1,
    selectedRootId,
    selectedFolderId,
    selectedFolderInfo,
    selectedMediaId,
    selectedMedia,
    selectedMetadata,
    folderChildrenByParent,
    folderDescendantsByParent,
    folderChildNextCursorByParent,
    expandedFolderIds,
    loadingFolderIds,
    loadingFolderDescendantIds,
    loadingMoreFolderIds,
    loading,
    loadingMoreMedia,
    mediaHasMore,
    thumbnailStatesByMediaId: freshThumbnailStatesByMediaId,
    addingRoot,
    rescanningRootIds,
    scanActive,
    tasks,
    busyTaskIds,
    tags,
    tagsById,
    searchState,
    searchActive,
    metadataSaving,
    error,
    lastScan,
    recentOps,
    recentOpsLoading,
    diagnostics,
    diagnosticsProbed,
    thumbnailCacheStats,
    thumbnailCacheStatsLoading,
    thumbnailCacheActionBusy,
    loadRecentOps,
    refreshThumbnailCacheStats,
    generateCurrentFolderThumbnailCache,
    generateCurrentTreeThumbnailCache,
    generateAllThumbnailCache,
    retryThumbnailCacheFailures,
    clearThumbnailCache: clearPersistentThumbnailCache,
    renameFile,
    renameFolder,
    moveItems,
    deleteItems,
    setSelectedRootId: (rootId: number) => {
      const root = roots.find((item) => item.id === rootId);
      const rootFolderId = root?.rootFolderId ?? null;
      selectLibraryFolder({ rootId, folderId: rootFolderId });
    },
    setSelectedFolder: (folder: FolderRecord) => {
      selectLibraryFolder({ rootId: folder.rootId, folderId: folder.id });
    },
    setSelectedFolderInfo: (folder: FolderRecord | null) => {
      setSelectedFolderInfoState(folder);
      selectMedia(null);
    },
    navigateFolderBack: () => navigateFolderHistory(-1),
    navigateFolderForward: () => navigateFolderHistory(1),
    toggleShowChildFolderContents: () => {
      const nextShowChildFolderContents = !showChildFolderContentsRef.current;
      showChildFolderContentsRef.current = nextShowChildFolderContents;
      setShowChildFolderContents(nextShowChildFolderContents);
      prepareNavigationMediaReload();
      void reloadCurrentMedia({
        rootId: selectedRootIdRef.current ?? undefined,
        folderId: selectedFolderIdRef.current,
        includeDescendants: nextShowChildFolderContents,
        setLoadingState: true
      }).catch((cause) => {
        setError(errorMessage(cause));
      });
    },
    setSelectedMediaId: (mediaId: number | null) => {
      if (mediaId !== null) {
        setSelectedFolderInfoState(null);
      }
      selectMedia(mediaId);
    },
    requestThumbnailStates,
    requestMediaWindow,
    requestFolderChildren: loadFolderChildren,
    toggleFolderExpanded: (folderId: number) => {
      setExpandedFolderIds((current) => {
        const next = new Set(current);
        if (next.has(folderId)) {
          next.delete(folderId);
        } else {
          next.add(folderId);
          void loadFolderChildren(folderId).catch((cause) => {
            setError(errorMessage(cause));
          });
        }
        return next;
      });
    },
    loadMoreFolderChildren,
    loadMoreMedia,
    rescanRoot,
    cancelTask,
    retryTask,
    refreshTasks,
    refresh,
    addRoot,
    setQ,
    setKind,
    setMinRating,
    toggleFavoriteFilter,
    toggleTagFilter,
    setSort,
    clearFilters,
    updateMetadata,
    setFileTags,
    addFileTag,
    removeFileTag,
    createTag,
    deleteTag
  };
}

function appendUniqueMedia(current: MediaRecord[], incoming: MediaRecord[]): MediaRecord[] {
  if (incoming.length === 0) {
    return current;
  }

  const next = [...current];
  const indexById = new Map(current.map((item, index) => [item.id, index]));
  let changed = false;
  for (const item of incoming) {
    const existingIndex = indexById.get(item.id);
    if (existingIndex === undefined) {
      indexById.set(item.id, next.length);
      next.push(item);
      changed = true;
    } else if (next[existingIndex] !== item) {
      next[existingIndex] = item;
      changed = true;
    }
  }
  return changed ? next : current;
}

function sameFolderNavigationEntry(
  left: FolderNavigationEntry | undefined,
  right: FolderNavigationEntry
): boolean {
  return Boolean(left && left.rootId === right.rootId && left.folderId === right.folderId);
}

function mergeLoadedMediaRanges(
  current: Array<{ start: number; end: number }>,
  start: number,
  end: number
): Array<{ start: number; end: number }> {
  if (end <= start) return current;
  const ranges = [...current, { start, end }].sort((a, b) => a.start - b.start);
  const merged: Array<{ start: number; end: number }> = [];
  for (const range of ranges) {
    const previous = merged.at(-1);
    if (!previous || range.start > previous.end) {
      merged.push({ ...range });
    } else {
      previous.end = Math.max(previous.end, range.end);
    }
  }
  return merged;
}

function firstMissingMediaRange(
  ranges: Array<{ start: number; end: number }>,
  start: number,
  end: number
): { start: number; end: number } | null {
  if (end <= start) return null;
  const sortedRanges = [...ranges].sort((left, right) => left.start - right.start);
  let cursor = start;
  for (const range of sortedRanges) {
    if (range.end <= cursor) {
      continue;
    }
    if (range.start > cursor) {
      break;
    }
    cursor = Math.max(cursor, range.end);
    if (cursor >= end) {
      return null;
    }
  }
  return { start: cursor, end };
}

function applyMediaWindowToSlots(
  current: Map<number, MediaRecord>,
  totalCount: number,
  offset: number,
  items: MediaRecord[]
): Map<number, MediaRecord> {
  const next = new Map(current);
  items.forEach((item, index) => {
    const targetIndex = offset + index;
    if (targetIndex >= 0 && targetIndex < totalCount) {
      next.set(targetIndex, item);
    }
  });
  return next;
}

function yieldToBrowser(): Promise<void> {
  return new Promise((resolve) => {
    window.setTimeout(resolve, 0);
  });
}

function abortAllControllers<K>(controllers: Map<K, AbortController>): void {
  for (const controller of controllers.values()) {
    controller.abort();
  }
  controllers.clear();
}

function isAbortError(cause: unknown): boolean {
  return cause instanceof Error && cause.name === "AbortError";
}

function mediaPageRequestKey(
  rootId: number,
  folderId: number | undefined,
  cursor: string,
  includeDescendants = false,
  generation?: number
): string {
  return [
    generation ?? "current",
    rootId,
    folderId ?? "root",
    includeDescendants ? "recursive" : "direct",
    cursor
  ].join(":");
}

function folderChildPageRequestKey(folderId: number, cursor: string): string {
  return [folderId, cursor].join(":");
}

function clearFolderChildPageKeys(keys: Set<string>, folderId: number): void {
  const prefix = `${folderId}:`;
  for (const key of keys) {
    if (key.startsWith(prefix)) {
      keys.delete(key);
    }
  }
}

function emptyUserMetadataRecord(fileId: number): UserMetadataRecord {
  return {
    favorite: false,
    fileId,
    note: null,
    rating: null,
    tagIds: [],
    updatedAt: Date.now()
  };
}

function confirmedMetadataPatch(
  requestedPatch: UserMetadataUpdate,
  record: UserMetadataRecord
): UserMetadataUpdate {
  const confirmed: UserMetadataUpdate = {};
  if ("rating" in requestedPatch) confirmed.rating = record.rating;
  if ("favorite" in requestedPatch) confirmed.favorite = record.favorite;
  if ("note" in requestedPatch) confirmed.note = record.note;
  return confirmed;
}

function emptyThumbnailPriorityScope(): ThumbnailPriorityScope {
  return {
    selected: [],
    visible: [],
    ahead: []
  };
}

function normalizeThumbnailScopeMediaIds(
  mediaIds: number[],
  priority: ThumbnailRequestPriority,
  selectedMediaId: number | null
): number[] {
  return [...new Set(mediaIds)]
    .filter((mediaId) => Number.isFinite(mediaId))
    .filter((mediaId) => priority === "selected" || mediaId !== selectedMediaId);
}

function nextThumbnailPriorityScope(
  current: ThumbnailPriorityScope,
  priority: ThumbnailRequestPriority,
  mediaIds: number[],
  selectedMediaId: number | null
): ThumbnailPriorityScope {
  if (priority === "background") {
    return current;
  }
  return {
    ...current,
    [priority]: normalizeThumbnailScopeMediaIds(mediaIds, priority, selectedMediaId)
  };
}

function thumbnailPriorityScopeRequestKey(input: {
  rootId: number;
  selectedFileIds: number[];
  visibleFileIds: number[];
  aheadFileIds: number[];
  retainedFileIds?: number[];
  generationMode?: string;
}): string {
  // Sort each id list so a set-order difference doesn't look like a scope
  // change (which would re-fire the sync needlessly). retained/mode are part of
  // the key so a retention change or mode switch DOES re-sync.
  const sorted = (ids: number[] | undefined) => [...(ids ?? [])].sort((a, b) => a - b);
  return JSON.stringify({
    rootId: input.rootId,
    selectedFileIds: sorted(input.selectedFileIds),
    visibleFileIds: sorted(input.visibleFileIds),
    aheadFileIds: sorted(input.aheadFileIds),
    retainedFileIds: sorted(input.retainedFileIds),
    generationMode: input.generationMode ?? null
  });
}

function mergeThumbnailStates(
  current: Record<number, ThumbnailResponse>,
  nextStates: Record<number, ThumbnailResponse>
): Record<number, ThumbnailResponse> {
  let changed = false;
  const next = { ...current };
  for (const [mediaId, thumbnail] of Object.entries(nextStates)) {
    const key = Number(mediaId);
    const mergedThumbnail = pickPreferredThumbnailResponse(current[key], thumbnail);
    if (current[key] !== mergedThumbnail) {
      next[key] = mergedThumbnail;
      changed = true;
    }
  }
  return changed ? next : current;
}

function recordThumbnailStateSignatures(
  states: Record<number, ThumbnailResponse>,
  mediaById: Map<number, MediaRecord>,
  signaturesByMediaId: Record<number, string>
): void {
  for (const mediaId of Object.keys(states)) {
    const mediaRecord = mediaById.get(Number(mediaId));
    if (mediaRecord) {
      signaturesByMediaId[Number(mediaId)] = mediaContentSignature(mediaRecord);
    }
  }
}

function filterFreshThumbnailStates(
  current: Record<number, ThumbnailResponse>,
  mediaById: Map<number, MediaRecord>,
  signaturesByMediaId: Record<number, string>
): Record<number, ThumbnailResponse> {
  let changed = false;
  const next: Record<number, ThumbnailResponse> = {};
  for (const [mediaId, thumbnail] of Object.entries(current)) {
    const key = Number(mediaId);
    const mediaRecord = mediaById.get(key);
    if (
      mediaRecord &&
      signaturesByMediaId[key] === mediaContentSignature(mediaRecord) &&
      isFreshThumbnailForMediaRecord(mediaRecord, thumbnail)
    ) {
      next[key] = thumbnail;
    } else {
      changed = true;
      delete signaturesByMediaId[key];
    }
  }
  return changed ? next : current;
}

function removeThumbnailState(
  current: Record<number, ThumbnailResponse>,
  mediaId: number
): Record<number, ThumbnailResponse> {
  if (!current[mediaId]) {
    return current;
  }
  const next = { ...current };
  delete next[mediaId];
  return next;
}

function renameFolderInMap(
  current: Record<number, FolderRecord[]>,
  folderId: number,
  newName: string
): Record<number, FolderRecord[]> {
  let changed = false;
  const next: Record<number, FolderRecord[]> = {};
  for (const [parentId, children] of Object.entries(current)) {
    const updated = children.map((child) =>
      child.id === folderId ? { ...child, name: newName } : child
    );
    if (updated.some((child, index) => child !== children[index])) {
      changed = true;
    }
    next[Number(parentId)] = updated;
  }
  return changed ? next : current;
}

function removeFoldersFromMap(
  current: Record<number, FolderRecord[]>,
  folderIds: number[]
): Record<number, FolderRecord[]> {
  const folderIdSet = new Set(folderIds);
  let changed = false;
  const next: Record<number, FolderRecord[]> = {};
  for (const [parentId, children] of Object.entries(current)) {
    const updated = children.filter((child) => !folderIdSet.has(child.id));
    if (updated.length !== children.length) {
      changed = true;
    }
    next[Number(parentId)] = updated;
  }
  return changed ? next : current;
}

function latestFailedThumbnailTaskForFile(tasks: TaskRecord[], fileId: number): TaskRecord | null {
  return (
    tasks
      .filter(
        (task) => task.kind === "thumbnail" && task.fileId === fileId && task.status === "failed"
      )
      .sort((left, right) => right.id - left.id)[0] ?? null
  );
}

function optimisticQueuedThumbnail(fileId: number): ThumbnailResponse {
  return {
    fileId,
    target: "grid_320",
    state: "queued",
    shortSidePx: 320,
    outputFormat: "image/webp",
    width: null,
    height: null,
    byteSize: null,
    servedBy: null,
    asset: null,
    error: null,
    updatedAt: null
  };
}

function explicitMediaThumbnailState(
  value: string | null | undefined
): ThumbnailResponse["state"] | null {
  if (
    value === "pending" ||
    value === "queued" ||
    value === "ready" ||
    value === "failed" ||
    value === "skipped_small"
  ) {
    return value;
  }
  return null;
}

function shouldRepollThumbnailState(
  mediaRecord: MediaRecord,
  thumbnail: ThumbnailResponse | undefined
): boolean {
  if (!shouldRequestThumbnailState(mediaRecord)) {
    return false;
  }
  return thumbnail?.state === undefined || thumbnail.state === "pending" || thumbnail.state === "queued";
}

function listMediaSort(sort: LibrarySort): "mtime_desc" | "mtime_asc" | "name_asc" | "name_desc" {
  switch (sort) {
    case "mtime_asc":
    case "name_asc":
    case "name_desc":
      return sort;
    default:
      return "mtime_desc";
  }
}

function isTaskActive(task: TaskRecord): boolean {
  return task.status === "pending" || task.status === "running";
}

function buildSearchParams(
  state: SearchState,
  debouncedQ: string,
  scope: {
    rootId: number;
    folderId: number | undefined;
    includeDescendants: boolean;
    cursor?: string;
    offset?: number;
    limit: number;
  }
): SearchParams {
  const params: SearchParams = {
    rootId: scope.rootId,
    limit: scope.limit,
    sort: state.sort
  };
  if (scope.cursor) params.cursor = scope.cursor;
  if (typeof scope.offset === "number") params.offset = scope.offset;
  if (scope.folderId !== undefined) {
    params.folderId = scope.folderId;
    params.includeDescendants = scope.includeDescendants;
  }
  const q = debouncedQ.trim();
  if (q) params.q = q;
  if (state.kind) params.kind = state.kind;
  if (state.minRating !== undefined) params.minRating = state.minRating;
  if (state.favorite !== undefined) params.favorite = state.favorite;
  if (state.tagIds.length > 0) params.tagIds = [...state.tagIds];
  return params;
}

function errorMessage(cause: unknown): string {
  if (cause instanceof Error) {
    return cause.message;
  }
  return "Core request failed";
}

function fileOpFailure(cause: unknown): FileOpResult {
  if (cause instanceof CoreApiError) {
    const body = cause.body;
    let code: string | undefined;
    let message: string | undefined;
    if (body && typeof body === "object") {
      const record = body as Record<string, unknown>;
      if (typeof record.code === "string") code = record.code;
      if (typeof record.error === "string") message = record.error;
      else if (typeof record.message === "string") message = record.message;
    }
    return {
      ok: false,
      code,
      message: message ?? `Request failed (${cause.status})`
    };
  }
  return {
    ok: false,
    message: errorMessage(cause)
  };
}

function readStoredShowChildFolderContents(): boolean {
  try {
    return window.localStorage.getItem(SHOW_SUBFOLDER_CONTENTS_STORAGE_KEY) !== "0";
  } catch {
    return true;
  }
}
