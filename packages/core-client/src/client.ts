import type {
  AcceptedRootResponse,
  AddFileTagRequest,
  ActiveLibraryResponse,
  CreateLibraryRequest,
  EnableLibrarySyncRequest,
  CreateLibraryResponse,
  CreateTagRequest,
  HostPluginProjection,
  HostPluginListResponse,
  ThumbnailCacheCancelBulkResponse,
  ThumbnailCacheClearResponse,
  ThumbnailCacheEnqueueResponse,
  ThumbnailCacheScopeParams,
  ThumbnailCacheTaskRequest,
  ThumbnailCacheSnapshotResponse,
  ThumbnailFailureListResponse,
  DeleteRequest,
  DeleteTagResponse,
  FileOperationListResponse,
  FileOperationRecord,
  FileOperationsResponse,
  FileTagsResponse,
  FolderRecord,
  InteractiveFolderScanTaskRequest,
  LibraryListResponse,
  LibraryPreflightRequest,
  LibraryPreflightResponse,
  LibrarySummary,
  ListNotificationOutboxParams,
  ListFileOperationsParams,
  ListFolderChildrenParams,
  ListMediaParams,
  MediaRecord,
  MediaPositionResponse,
  MoveRequest,
  NotificationOutboxAckRequest,
  NotificationOutboxAckResponse,
  NotificationOutboxResponse,
  Page,
  PluginDiscoveryResponse,
  PluginAuthBeginResponse,
  PluginAuthCancelRequest,
  OneDriveConfiguration,
  OneDriveConfigurationPlanRequest,
  OneDriveConfigurationPlan,
  OneDriveConfigurationCommitRequest,
  PluginAuthCompleteRequest,
  PluginCommandResponse,
  PluginDiagnosticsResponse,
  PluginGrantRequest,
  PluginHealthResponse,
  PluginVersionRequest,
  RenameRequest,
  RootRecord,
  ScanTaskRequest,
  SearchParams,
  SetFileTagsRequest,
  TagListResponse,
  TagRecord,
  TaskRecord,
  ThumbnailPriority,
  ThumbnailPriorityScopeSyncRequest,
  UpdateLibraryRequest,
  ThumbnailResponse,
  UserMetadataRecord,
  UserMetadataUpdate
} from "./generated-contract";
import type { RemoteLibraryListResponse, SyncCommandRequest, SyncConflictListResponse, SyncConflictResolutionRequest, SyncEnableRequest, SyncOperation, SyncPreflightRequest, SyncPreflightResponse, SyncReconnectCommitRequest, SyncReconnectPlan, SyncReconnectPreflightRequest, SyncRestoreListResponse, SyncRestorePlan, SyncRestorePreflight, SyncRestorePreflightRequest, SyncRestoreStartRequest, SyncStatusSnapshot, SyncUnbindCommitRequest, SyncUnbindPlan } from "./generated-contract";

export interface CoreClientConfig {
  baseUrl: string;
  sessionToken?: string;
}

export class CoreApiError extends Error {
  readonly status: number;
  readonly body: unknown;

  constructor(status: number, body: unknown) {
    super(`Core API request failed with status ${status}`);
    this.name = "CoreApiError";
    this.status = status;
    this.body = body;
  }
}

export interface BlobRequestOptions {
  signal?: AbortSignal;
  version?: number | string | null;
  requestPriority?: CoreRequestPriority;
}

export interface PreviewResourceOptions extends BlobRequestOptions {
  /** Longest side of the preview viewport in CSS pixels. */
  viewportLongPx?: number;
}

export interface PreviewResource {
  blob: Blob;
  /** original | preview_2048 | preview_3072 | preview_native */
  tier: string;
}

export interface CoreRequestOptions {
  signal?: AbortSignal;
  requestPriority?: CoreRequestPriority;
}

export type CoreRequestPriority =
  | "background"
  | "resource"
  | "metadata"
  | "interactive"
  | "navigation";

type ScheduledRequestInit = RequestInit & {
  requestPriority?: CoreRequestPriority;
};

type QueuedCoreRequest = {
  sequence: number;
  priority: CoreRequestPriority;
  signal?: AbortSignal;
  run: () => Promise<unknown>;
  resolve: (value: unknown) => void;
  reject: (reason: unknown) => void;
  abort: () => void;
  debugPath: string;
};

type QueryParams = Partial<ListMediaParams & ListFolderChildrenParams> & {
  priority?: ThumbnailPriority;
  target?: "grid_320";
  v?: number | string | null;
  viewportLongPx?: number;
};

const MAX_CORE_REQUESTS = 12;
const MAX_CORE_INTERACTIVE_REQUESTS = 2;
// Hard ceiling on how long a single Core request may stay in flight. A dropped
// connection (e.g. Core restart during dev, or a half-open socket) can leave a
// `fetch` pending forever; without a timeout its scheduler slot is never
// released and, once MAX_CORE_REQUESTS such requests accumulate, the whole
// scheduler wedges and NOTHING new loads (the "grid stuck on placeholders" bug).
// This bound guarantees every request settles — success, error, or timeout — so
// its slot is always freed. Generous so it never trips a legitimately slow query.
const CORE_REQUEST_TIMEOUT_MS = 60_000;
let activeCoreRequests = 0;
let activeCoreInteractiveRequests = 0;
let coreRequestSequence = 0;
const coreRequestQueue: QueuedCoreRequest[] = [];

// ── Debug facility ─────────────────────────────────────────────────────────
// One global switch gates all client-scheduler diagnostics so they can stay in
// the code (default off, zero overhead) and be flipped on live for debugging:
//   window.__megleDebug = true   // then read window.__megleCoreScheduler()
// When on, each in-flight request is tracked with metadata (path, priority,
// whether it has an abort signal, how long it has been active) so a stuck slot
// is immediately visible via the scheduler snapshot.
interface CoreDebugEntry {
  sequence: number;
  priority: CoreRequestPriority;
  path: string;
  hasSignal: boolean;
  startedAt: number;
}
const coreDebugActive = new Map<number, CoreDebugEntry>();
function coreDebugEnabled(): boolean {
  return (
    typeof window !== "undefined" &&
    (window as unknown as { __megleDebug?: boolean }).__megleDebug === true
  );
}

if (typeof window !== "undefined") {
  (window as unknown as { __megleCoreScheduler: () => unknown }).__megleCoreScheduler = () => {
    const now = typeof performance !== "undefined" ? performance.now() : Date.now();
    return {
      active: activeCoreRequests,
      interactive: activeCoreInteractiveRequests,
      queued: coreRequestQueue.length,
      max: MAX_CORE_REQUESTS,
      // Populated only while window.__megleDebug is on.
      inFlight: Array.from(coreDebugActive.values()).map((entry) => ({
        sequence: entry.sequence,
        priority: entry.priority,
        path: entry.path,
        hasSignal: entry.hasSignal,
        ageMs: Math.round(now - entry.startedAt)
      }))
    };
  };
}

export function createCoreClient(config: CoreClientConfig) {
  let observedContextRevision: string | null = null;
  async function request<T>(path: string, init: ScheduledRequestInit = {}): Promise<T> {
    const {
      requestPriority = "metadata",
      signal,
      ...fetchInit
    } = init;
    const headers = new Headers(init.headers);
    if (init.body && !headers.has("content-type")) {
      headers.set("content-type", "application/json");
    }
    if (config.sessionToken) {
      headers.set("x-megle-session", config.sessionToken);
    }
    const method = (fetchInit.method ?? "GET").toUpperCase();
    if (method !== "GET" && method !== "HEAD" && !headers.has("x-megle-context-revision") && observedContextRevision) {
      headers.set("x-megle-context-revision", observedContextRevision);
    }

    return scheduleCoreRequest(
      requestPriority,
      signal,
      () =>
        fetchWithTimeout<T>(
          resolveUrl(config.baseUrl, path),
          { ...fetchInit, headers },
          signal,
          async (response) => {
            observedContextRevision = response.headers.get("x-megle-context-revision") ?? observedContextRevision;
            if (!response.ok) {
              const body = await readResponseBody(response);
              throw new CoreApiError(response.status, body);
            }
            const body = await readJson(response);
            return body as T;
          }
        ),
      path
    );
  }

  async function requestNoContent(
    path: string,
    init: ScheduledRequestInit = {}
  ): Promise<void> {
    const { requestPriority = "metadata", signal, ...fetchInit } = init;
    const headers = new Headers(init.headers);
    if (config.sessionToken) {
      headers.set("x-megle-session", config.sessionToken);
    }
    const method = (fetchInit.method ?? "GET").toUpperCase();
    if (method !== "GET" && method !== "HEAD" && !headers.has("x-megle-context-revision") && observedContextRevision) {
      headers.set("x-megle-context-revision", observedContextRevision);
    }
    await scheduleCoreRequest(
      requestPriority,
      signal,
      () =>
        fetchWithTimeout<void>(
          resolveUrl(config.baseUrl, path),
          { ...fetchInit, headers },
          signal,
          async (response) => {
            observedContextRevision = response.headers.get("x-megle-context-revision") ?? observedContextRevision;
            if (!response.ok) {
              const body = await readResponseBody(response);
              throw new CoreApiError(response.status, body);
            }
          }
        ),
      path
    );
  }

  async function fetchBlob(path: string, options: BlobRequestOptions = {}): Promise<Blob> {
    const headers = new Headers();
    if (config.sessionToken) {
      headers.set("x-megle-session", config.sessionToken);
    }
    return scheduleCoreRequest(
      options.requestPriority ?? "resource",
      options.signal,
      () =>
        fetchWithTimeout(
          resolveUrl(config.baseUrl, path),
          { headers },
          options.signal,
          async (response) => {
            if (!response.ok) {
              const body = await readResponseBody(response);
              throw new CoreApiError(response.status, body);
            }
            return response.blob();
          }
        ),
      path
    );
  }

  async function fetchBlobWithTier(
    path: string,
    options: BlobRequestOptions = {}
  ): Promise<PreviewResource> {
    const headers = new Headers();
    if (config.sessionToken) {
      headers.set("x-megle-session", config.sessionToken);
    }
    return scheduleCoreRequest(
      options.requestPriority ?? "resource",
      options.signal,
      () =>
        fetchWithTimeout(
          resolveUrl(config.baseUrl, path),
          { headers },
          options.signal,
          async (response) => {
            if (!response.ok) {
              const body = await readResponseBody(response);
              throw new CoreApiError(response.status, body);
            }
            return {
              blob: await response.blob(),
              tier: response.headers.get("x-megle-preview-tier") ?? "original"
            };
          }
        ),
      path
    );
  }

  return {
    listLibraries: (options: CoreRequestOptions = {}) =>
      request<LibraryListResponse>("/libraries", {
        requestPriority: options.requestPriority ?? "navigation",
        signal: options.signal
      }),
    getActiveLibrary: (options: CoreRequestOptions = {}) =>
      request<ActiveLibraryResponse>("/libraries/active", {
        requestPriority: options.requestPriority ?? "navigation",
        signal: options.signal
      }),
    preflightLibrary: (body: LibraryPreflightRequest, options: CoreRequestOptions = {}) =>
      request<LibraryPreflightResponse>("/libraries/preflight", {
        method: "POST",
        requestPriority: options.requestPriority ?? "interactive",
        body: JSON.stringify(body),
        signal: options.signal
      }),
    createLibrary: (body: CreateLibraryRequest, options: CoreRequestOptions = {}) =>
      request<CreateLibraryResponse>("/libraries", {
        method: "POST",
        requestPriority: options.requestPriority ?? "interactive",
        body: JSON.stringify(body),
        signal: options.signal
      }),
    activateLibrary: (libraryUuid: string, contextRevision: number, options: CoreRequestOptions = {}) =>
      request<ActiveLibraryResponse>(`/libraries/${encodeURIComponent(libraryUuid)}/activate`, {
        method: "POST",
        headers: { "x-megle-context-revision": String(contextRevision) },
        requestPriority: options.requestPriority ?? "interactive",
        signal: options.signal
      }),
    updateLibrary: (libraryUuid: string, contextRevision: number, body: UpdateLibraryRequest, options: CoreRequestOptions = {}) =>
      request<LibrarySummary>(`/libraries/${encodeURIComponent(libraryUuid)}`, {
        method: "PATCH",
        headers: { "x-megle-context-revision": String(contextRevision) },
        requestPriority: options.requestPriority ?? "interactive",
        body: JSON.stringify(body),
        signal: options.signal
      }),
    removeLocalLibrary: (libraryUuid: string, contextRevision: number, options: CoreRequestOptions = {}) =>
      requestNoContent(`/libraries/${encodeURIComponent(libraryUuid)}/local-copy`, {
        method: "DELETE",
        headers: { "x-megle-context-revision": String(contextRevision) },
        requestPriority: options.requestPriority ?? "interactive",
        signal: options.signal
      }),
    enableLibrarySync: (libraryUuid: string, contextRevision: number, body: EnableLibrarySyncRequest, options: CoreRequestOptions = {}) =>
      request<LibrarySummary>(`/libraries/${encodeURIComponent(libraryUuid)}/sync-binding`, {
        method: "POST",
        headers: { "x-megle-context-revision": String(contextRevision) },
        requestPriority: options.requestPriority ?? "interactive",
        body: JSON.stringify(body),
        signal: options.signal
      }),
    unbindLibrarySync: (libraryUuid: string, contextRevision: number, options: CoreRequestOptions = {}) =>
      requestNoContent(`/libraries/${encodeURIComponent(libraryUuid)}/sync-binding`, {
        method: "DELETE",
        headers: { "x-megle-context-revision": String(contextRevision) },
        requestPriority: options.requestPriority ?? "interactive",
        signal: options.signal
      }),
    getSyncStatus: (options: CoreRequestOptions = {}) => request<SyncStatusSnapshot>("/sync/status", { requestPriority: options.requestPriority ?? "background", signal: options.signal }),
    wakeSync: (reason: "foreground" | "network" | "manual" = "manual", options: CoreRequestOptions = {}) => request<void>(`/sync/wake?reason=${encodeURIComponent(reason)}`, { method: "POST", requestPriority: options.requestPriority ?? "interactive", signal: options.signal }),
    listRemoteLibraries: (options: CoreRequestOptions = {}) => request<RemoteLibraryListResponse>("/sync/remote-libraries", { requestPriority: options.requestPriority ?? "background", signal: options.signal }),
    preflightLibrarySync: (libraryUuid: string, body: SyncPreflightRequest, options: CoreRequestOptions = {}) => request<SyncPreflightResponse>(`/libraries/${encodeURIComponent(libraryUuid)}/sync/preflight`, { method: "POST", requestPriority: options.requestPriority ?? "interactive", body: JSON.stringify(body), signal: options.signal }),
    startLibrarySyncEnable: (libraryUuid: string, contextRevision: number, body: SyncEnableRequest, options: CoreRequestOptions = {}) => request<SyncOperation>(`/libraries/${encodeURIComponent(libraryUuid)}/sync/enable`, { method: "POST", headers: { "x-megle-context-revision": String(contextRevision) }, requestPriority: options.requestPriority ?? "interactive", body: JSON.stringify(body), signal: options.signal }),
    pauseLibrarySync: (libraryUuid: string, contextRevision: number, body: SyncCommandRequest) => syncCommand(libraryUuid,"pause",contextRevision,body),
    resumeLibrarySync: (libraryUuid: string, contextRevision: number, body: SyncCommandRequest) => syncCommand(libraryUuid,"resume",contextRevision,body),
    syncLibraryNow: (libraryUuid: string, contextRevision: number, body: SyncCommandRequest) => syncCommand(libraryUuid,"sync-now",contextRevision,body),
    retryLibrarySync: (libraryUuid: string, contextRevision: number, body: SyncCommandRequest) => syncCommand(libraryUuid,"retry",contextRevision,body),
    planLibrarySyncUnbind: (libraryUuid: string) => request<SyncUnbindPlan>(`/libraries/${encodeURIComponent(libraryUuid)}/sync/unbind-plan`, { method: "POST", requestPriority: "interactive" }),
    commitLibrarySyncUnbind: (libraryUuid: string, contextRevision: number, body: SyncUnbindCommitRequest) => request<SyncOperation>(`/libraries/${encodeURIComponent(libraryUuid)}/sync/unbind-commit`, { method: "POST", headers: { "x-megle-context-revision": String(contextRevision) }, requestPriority: "interactive", body: JSON.stringify(body) }),
    preflightLibrarySyncReconnect: (libraryUuid: string, body: SyncReconnectPreflightRequest) => request<SyncReconnectPlan>(`/libraries/${encodeURIComponent(libraryUuid)}/sync/reconnect-preflight`, { method: "POST", requestPriority: "interactive", body: JSON.stringify(body) }),
    commitLibrarySyncReconnect: (libraryUuid: string, contextRevision: number, body: SyncReconnectCommitRequest) => request<SyncOperation>(`/libraries/${encodeURIComponent(libraryUuid)}/sync/reconnect-commit`, { method: "POST", headers: { "x-megle-context-revision": String(contextRevision) }, requestPriority: "interactive", body: JSON.stringify(body) }),
    preflightSyncRestore: (body: SyncRestorePreflightRequest) => request<SyncRestorePreflight>("/sync/restores/preflight", { method: "POST", requestPriority: "interactive", body: JSON.stringify(body) }),
    startSyncRestore: (body: SyncRestoreStartRequest) => request<SyncRestoreListResponse>("/sync/restores", { method: "POST", requestPriority: "interactive", body: JSON.stringify(body) }),
    getSyncRestore: (restoreUuid: string, options: CoreRequestOptions = {}) => request<SyncRestorePlan>(`/sync/restores/${encodeURIComponent(restoreUuid)}`, { requestPriority: options.requestPriority ?? "background", signal: options.signal }),
    pauseSyncRestore: (restoreUuid: string) => request<SyncRestorePlan>(`/sync/restores/${encodeURIComponent(restoreUuid)}/pause`, { method: "POST", requestPriority: "interactive" }),
    resumeSyncRestore: (restoreUuid: string) => request<SyncRestorePlan>(`/sync/restores/${encodeURIComponent(restoreUuid)}/resume`, { method: "POST", requestPriority: "interactive" }),
    cancelSyncRestore: (restoreUuid: string) => request<SyncRestorePlan>(`/sync/restores/${encodeURIComponent(restoreUuid)}/cancel`, { method: "POST", requestPriority: "interactive" }),
    listLibrarySyncConflicts: (libraryUuid: string, params: { cursor?: string; limit?: number } = {}, options: CoreRequestOptions = {}) => {
      const search = new URLSearchParams();
      if (params.cursor) search.set("cursor", params.cursor);
      if (params.limit) search.set("limit", String(params.limit));
      const suffix = search.size ? `?${search.toString()}` : "";
      return request<SyncConflictListResponse>(`/libraries/${encodeURIComponent(libraryUuid)}/sync/conflicts${suffix}`, { requestPriority: options.requestPriority ?? "background", signal: options.signal });
    },
    resolveLibrarySyncConflict: (libraryUuid: string, conflictUuid: string, contextRevision: number, body: SyncConflictResolutionRequest) => request<SyncOperation>(`/libraries/${encodeURIComponent(libraryUuid)}/sync/conflicts/${encodeURIComponent(conflictUuid)}/resolve`, { method: "POST", headers: { "x-megle-context-revision": String(contextRevision) }, requestPriority: "interactive", body: JSON.stringify(body) }),
    getLibrarySyncDiagnostics: (libraryUuid: string) => request<Record<string, unknown>>(`/libraries/${encodeURIComponent(libraryUuid)}/sync/diagnostics`, { requestPriority: "background" }),
    exportLibrarySyncDiagnostics: (libraryUuid: string) => request<Record<string, unknown>>(`/libraries/${encodeURIComponent(libraryUuid)}/sync/diagnostics/export`, { method: "POST", requestPriority: "interactive" }),
    listRoots: () => request<Page<RootRecord>>("/roots", { requestPriority: "navigation" }),
    listTasks: (options: CoreRequestOptions = {}) =>
      request<Page<TaskRecord>>("/tasks", {
        requestPriority: options.requestPriority ?? "navigation",
        signal: options.signal
      }),
    addRoot: (path: string, displayName?: string) =>
      request<AcceptedRootResponse>("/roots", {
        method: "POST",
        requestPriority: "interactive",
        body: JSON.stringify({ path, displayName })
      }),
    removeRoot: (rootId: number) =>
      request<AcceptedRootResponse>(`/roots/${rootId}`, {
        method: "DELETE",
        requestPriority: "interactive"
      }),
    enqueueScan: (rootId: number) =>
      request<AcceptedRootResponse>("/tasks/scan", {
        method: "POST",
        requestPriority: "interactive",
        body: JSON.stringify({ rootId } satisfies ScanTaskRequest)
      }),
    enqueueInteractiveFolderScan: (folderId: number, options: CoreRequestOptions = {}) =>
      request<AcceptedRootResponse>("/tasks/interactive-folder-scan", {
        method: "POST",
        requestPriority: options.requestPriority ?? "background",
        body: JSON.stringify({ folderId } satisfies InteractiveFolderScanTaskRequest),
        signal: options.signal
      }),
    syncThumbnailPriorityScope: (
      input: ThumbnailPriorityScopeSyncRequest,
      options: CoreRequestOptions = {}
    ) =>
      request<AcceptedRootResponse>("/tasks/thumbnail-priority-scope", {
        method: "POST",
        requestPriority: options.requestPriority ?? "interactive",
        body: JSON.stringify(input),
        signal: options.signal
      }),
    getThumbnailCacheStats: (
      params: ThumbnailCacheScopeParams = {},
      options: CoreRequestOptions = {}
    ) =>
      request<ThumbnailCacheSnapshotResponse>(`/thumbnails/cache/stats${query(params)}`, {
        requestPriority: options.requestPriority ?? "metadata",
        signal: options.signal
      }),
    listThumbnailCacheFailures: (
      limit = 100,
      options: CoreRequestOptions = {}
    ) =>
      request<ThumbnailFailureListResponse>(`/thumbnails/cache/failures${query({ limit })}`, {
        requestPriority: options.requestPriority ?? "metadata",
        signal: options.signal
      }),
    enqueueThumbnailCache: (
      input: ThumbnailCacheTaskRequest,
      options: CoreRequestOptions = {}
    ) =>
      request<ThumbnailCacheEnqueueResponse>("/tasks/thumbnail-cache", {
        method: "POST",
        requestPriority: options.requestPriority ?? "background",
        body: JSON.stringify(input),
        signal: options.signal
      }),
    clearThumbnailCache: (options: CoreRequestOptions = {}) =>
      request<ThumbnailCacheClearResponse>("/thumbnails/cache/clear", {
        method: "POST",
        requestPriority: options.requestPriority ?? "interactive",
        signal: options.signal
      }),
    cancelBulkThumbnailCache: (options: CoreRequestOptions = {}) =>
      request<ThumbnailCacheCancelBulkResponse>("/thumbnails/cache/cancel-bulk", {
        method: "POST",
        requestPriority: options.requestPriority ?? "interactive",
        signal: options.signal
      }),
    cancelTask: (taskId: number) =>
      request<AcceptedRootResponse>(`/tasks/${taskId}/cancel`, {
        method: "POST",
        requestPriority: "interactive"
      }),
    retryTask: (taskId: number) =>
      request<AcceptedRootResponse>(`/tasks/${taskId}/retry`, {
        method: "POST",
        requestPriority: "interactive"
      }),
    listFolderChildren: (
      folderId: number,
      params: ListFolderChildrenParams = {},
      options: CoreRequestOptions = {}
    ) =>
      request<Page<FolderRecord>>(`/folders/${folderId}/children${query(params)}`, {
        requestPriority: options.requestPriority ?? "navigation",
        signal: options.signal
      }),
    listFolderAncestors: (folderId: number, options: CoreRequestOptions = {}) =>
      request<Page<FolderRecord>>(`/folders/${folderId}/ancestors`, {
        requestPriority: options.requestPriority ?? "navigation",
        signal: options.signal
      }),
    listMedia: (params: ListMediaParams = {}, options: CoreRequestOptions = {}) =>
      request<Page<MediaRecord>>(`/media${query(params)}`, {
        requestPriority: options.requestPriority ?? "navigation",
        signal: options.signal
      }),
    getMedia: (fileId: number) =>
      request<MediaRecord>(`/media/${fileId}`, { requestPriority: "interactive" }),
    getMediaPosition: (
      fileId: number,
      params: Pick<ListMediaParams, "rootId" | "folderId" | "includeDescendants" | "sort">,
      options: CoreRequestOptions = {}
    ) => request<MediaPositionResponse>(`/media/${fileId}/position${query(params)}`, {
      requestPriority: options.requestPriority ?? "navigation",
      signal: options.signal
    }),
    getThumbnail: (
      fileId: number,
      target: "grid_320" = "grid_320",
      priority: ThumbnailPriority = "background",
      options: CoreRequestOptions = {}
    ) =>
      request<ThumbnailResponse>(`/media/${fileId}/thumbnail${query({ target, priority })}`, {
        requestPriority: options.requestPriority ?? thumbnailPriorityCoreRequestPriority(priority),
        signal: options.signal
      }),
    getThumbnailBlob: async (
      fileId: number,
      target: "grid_320" = "grid_320",
      options: BlobRequestOptions = {}
    ) => {
      return fetchBlob(
        `/media/${fileId}/thumbnail/blob${query({ target, v: options.version })}`,
        {
          ...options,
          requestPriority: options.requestPriority ?? "resource"
        }
      );
    },
    getPreviewBlob: (fileId: number, options: BlobRequestOptions = {}) =>
      fetchBlob(`/media/${fileId}/preview${query({ v: options.version })}`, {
        ...options,
        requestPriority: options.requestPriority ?? "interactive"
      }),
    getPreviewResource: (fileId: number, options: PreviewResourceOptions = {}) =>
      fetchBlobWithTier(
        `/media/${fileId}/preview${query({
          v: options.version,
          viewportLongPx: options.viewportLongPx
        })}`,
        {
          ...options,
          requestPriority: options.requestPriority ?? "interactive"
        }
      ),
    listTags: () => request<TagListResponse>("/tags", { requestPriority: "metadata" }),
    createTag: (body: CreateTagRequest) =>
      request<TagRecord>("/tags", {
        method: "POST",
        requestPriority: "interactive",
        body: JSON.stringify(body)
      }),
    deleteTag: (tagId: number) =>
      request<DeleteTagResponse>(`/tags/${tagId}`, {
        method: "DELETE",
        requestPriority: "interactive"
      }),
    getUserMetadata: (fileId: number) =>
      request<UserMetadataRecord>(`/media/${fileId}/metadata`, { requestPriority: "metadata" }),
    updateUserMetadata: (fileId: number, body: UserMetadataUpdate) =>
      request<UserMetadataRecord>(`/media/${fileId}/metadata`, {
        method: "PUT",
        requestPriority: "interactive",
        body: JSON.stringify(body)
      }),
    setFileTags: (fileId: number, body: SetFileTagsRequest) =>
      request<FileTagsResponse>(`/media/${fileId}/tags`, {
        method: "PUT",
        requestPriority: "interactive",
        body: JSON.stringify(body)
      }),
    addFileTag: (fileId: number, body: AddFileTagRequest) =>
      request<FileTagsResponse>(`/media/${fileId}/tags`, {
        method: "POST",
        requestPriority: "interactive",
        body: JSON.stringify(body)
      }),
    removeFileTag: (fileId: number, tagId: number) =>
      request<FileTagsResponse>(`/media/${fileId}/tags/${tagId}`, {
        method: "DELETE",
        requestPriority: "interactive"
      }),
    searchMedia: (params: SearchParams = {}, options: CoreRequestOptions = {}) =>
      request<Page<MediaRecord>>(`/search${searchQuery(params)}`, {
        requestPriority: options.requestPriority ?? "navigation",
        signal: options.signal
      }),
    renameFileOp: (body: RenameRequest) =>
      request<FileOperationRecord>("/file-ops/rename", {
        method: "POST",
        requestPriority: "interactive",
        body: JSON.stringify(body)
      }),
    moveFileOps: (body: MoveRequest) =>
      request<FileOperationsResponse>("/file-ops/move", {
        method: "POST",
        requestPriority: "interactive",
        body: JSON.stringify(body)
      }),
    deleteFileOps: (body: DeleteRequest) =>
      request<FileOperationsResponse>("/file-ops/delete", {
        method: "POST",
        requestPriority: "interactive",
        body: JSON.stringify(body)
      }),
    listFileOperations: (params: ListFileOperationsParams = {}) =>
      request<FileOperationListResponse>(`/file-ops${fileOpsQuery(params)}`, {
        requestPriority: "metadata"
      }),
    listNotificationOutbox: (
      params: ListNotificationOutboxParams,
      options: CoreRequestOptions = {}
    ) =>
      request<NotificationOutboxResponse>(
        `/notifications/outbox${notificationOutboxQuery(params)}`,
        {
          requestPriority: options.requestPriority ?? "metadata",
          signal: options.signal
        }
      ),
    ackNotificationOutbox: (body: NotificationOutboxAckRequest) =>
      request<NotificationOutboxAckResponse>("/notifications/outbox/ack", {
        method: "POST",
        requestPriority: "metadata",
        body: JSON.stringify(body)
      }),
    listPlugins: () => request<HostPluginListResponse>("/plugins", { requestPriority: "metadata" }),
    getPlugin: (pluginId: string) =>
      request<HostPluginProjection>(`/plugins/${encodeURIComponent(pluginId)}`, {
        requestPriority: "metadata"
      }),
    discoverPlugins: () =>
      request<PluginDiscoveryResponse>("/plugins/discover", {
        method: "POST",
        requestPriority: "interactive"
      }),
    enablePlugin: (pluginId: string) =>
      request<HostPluginProjection>(`/plugins/${encodeURIComponent(pluginId)}/enable`, {
        method: "POST",
        requestPriority: "interactive"
      }),
    disablePlugin: (pluginId: string) =>
      request<HostPluginProjection>(`/plugins/${encodeURIComponent(pluginId)}/disable`, {
        method: "POST",
        requestPriority: "interactive"
      }),
    unregisterPlugin: (pluginId: string) =>
      request<PluginCommandResponse>(`/plugins/${encodeURIComponent(pluginId)}`, {
        method: "DELETE",
        requestPriority: "interactive"
      }),
    updatePluginGrants: (pluginId: string, input: PluginGrantRequest) =>
      request<HostPluginProjection>(`/plugins/${encodeURIComponent(pluginId)}/grants`, { method: "PUT", requestPriority: "interactive", body: JSON.stringify(input) }),
    retryPlugin: (pluginId: string) =>
      request<PluginCommandResponse>(`/plugins/${encodeURIComponent(pluginId)}/retry`, { method: "POST", requestPriority: "interactive" }),
    selectPluginVersion: (pluginId: string, input: PluginVersionRequest) =>
      request<PluginCommandResponse>(`/plugins/${encodeURIComponent(pluginId)}/versions/select`, { method: "POST", requestPriority: "interactive", body: JSON.stringify(input) }),
    rollbackPluginVersion: (pluginId: string) =>
      request<PluginCommandResponse>(`/plugins/${encodeURIComponent(pluginId)}/versions/rollback`, { method: "POST", requestPriority: "interactive" }),
    beginPluginAuth: (pluginId: string) =>
      request<PluginAuthBeginResponse>(`/plugins/${encodeURIComponent(pluginId)}/auth/begin`, { method: "POST", requestPriority: "interactive" }),
    completePluginAuth: (pluginId: string, input: PluginAuthCompleteRequest) =>
      request<PluginCommandResponse>(`/plugins/${encodeURIComponent(pluginId)}/auth/complete`, { method: "POST", requestPriority: "interactive", body: JSON.stringify(input) }),
    cancelPluginAuth: (pluginId: string, input: PluginAuthCancelRequest) =>
      request<PluginCommandResponse>(`/plugins/${encodeURIComponent(pluginId)}/auth/cancel`, { method: "POST", requestPriority: "interactive", body: JSON.stringify(input) }),
    getOneDriveConfiguration: () => request<OneDriveConfiguration>("/plugins/com.megle.onedrive/configuration", { requestPriority: "metadata" }),
    planOneDriveConfiguration: (input: OneDriveConfigurationPlanRequest) => request<OneDriveConfigurationPlan>("/plugins/com.megle.onedrive/configuration/plan", { method: "POST", requestPriority: "interactive", body: JSON.stringify(input) }),
    commitOneDriveConfiguration: (input: OneDriveConfigurationCommitRequest) => request<OneDriveConfiguration>("/plugins/com.megle.onedrive/configuration", { method: "PUT", requestPriority: "interactive", body: JSON.stringify(input) }),
    signOutPlugin: (pluginId: string) =>
      request<PluginCommandResponse>(`/plugins/${encodeURIComponent(pluginId)}/auth/sign-out`, { method: "POST", requestPriority: "interactive" }),
    getPluginHealth: (pluginId: string) =>
      request<PluginHealthResponse>(`/plugins/${encodeURIComponent(pluginId)}/health`, { requestPriority: "metadata" }),
    getPluginDiagnostics: (pluginId: string) =>
      request<PluginDiagnosticsResponse>(`/plugins/${encodeURIComponent(pluginId)}/diagnostics`, { requestPriority: "metadata" })
  };

  function syncCommand(libraryUuid: string, action: string, contextRevision: number, body: SyncCommandRequest): Promise<SyncOperation> {
    return request<SyncOperation>(`/libraries/${encodeURIComponent(libraryUuid)}/sync/${action}`, { method: "POST", headers: { "x-megle-context-revision": String(contextRevision) }, requestPriority: "interactive", body: JSON.stringify(body) });
  }
}

function scheduleCoreRequest<T>(
  priority: CoreRequestPriority,
  signal: AbortSignal | null | undefined,
  run: () => Promise<T>,
  debugPath = ""
): Promise<T> {
  if (signal?.aborted) {
    return Promise.reject(createAbortError());
  }

  return new Promise<T>((resolve, reject) => {
    const entry: QueuedCoreRequest = {
      sequence: coreRequestSequence++,
      priority,
      signal: signal ?? undefined,
      run: run as () => Promise<unknown>,
      resolve: resolve as (value: unknown) => void,
      reject,
      abort: () => undefined,
      debugPath
    };
    entry.abort = () => {
      const queuedIndex = coreRequestQueue.indexOf(entry);
      if (queuedIndex >= 0) {
        coreRequestQueue.splice(queuedIndex, 1);
        entry.signal?.removeEventListener("abort", entry.abort);
        reject(createAbortError());
      }
    };

    entry.signal?.addEventListener("abort", entry.abort, { once: true });
    coreRequestQueue.push(entry);
    pumpCoreRequests();
  });
}

function pumpCoreRequests(): void {
  if (
    activeCoreResourceRequests() >= MAX_CORE_REQUESTS &&
    activeCoreInteractiveRequests >= MAX_CORE_INTERACTIVE_REQUESTS
  ) {
    return;
  }

  coreRequestQueue.sort((left, right) => {
    const priorityDelta = coreRequestPriorityRank(right.priority) - coreRequestPriorityRank(left.priority);
    return priorityDelta === 0 ? left.sequence - right.sequence : priorityDelta;
  });

  while (coreRequestQueue.length > 0) {
    const entryIndex = coreRequestQueue.findIndex((candidate) =>
      canStartCoreRequest(candidate.priority)
    );
    if (entryIndex < 0) {
      return;
    }
    const entry = coreRequestQueue.splice(entryIndex, 1)[0];
    if (!entry) {
      return;
    }
    if (entry.signal?.aborted) {
      entry.signal.removeEventListener("abort", entry.abort);
      entry.reject(createAbortError());
      continue;
    }

    activeCoreRequests += 1;
    const isInteractiveRequest = isInteractiveCoreRequest(entry.priority);
    if (isInteractiveRequest) {
      activeCoreInteractiveRequests += 1;
    }
    if (coreDebugEnabled()) {
      coreDebugActive.set(entry.sequence, {
        sequence: entry.sequence,
        priority: entry.priority,
        path: entry.debugPath,
        hasSignal: entry.signal !== undefined,
        startedAt: typeof performance !== "undefined" ? performance.now() : Date.now()
      });
    }
    // Release the slot exactly once, whichever comes first: the run settling OR
    // the signal aborting. Both paths funnel through `releaseSlot`, which is
    // idempotent, so a request that aborts mid-flight (its fetch also rejecting)
    // still decrements the counter exactly once.
    let released = false;
    const releaseSlot = () => {
      if (released) {
        return;
      }
      released = true;
      coreDebugActive.delete(entry.sequence);
      entry.signal?.removeEventListener("abort", onAbort);
      activeCoreRequests = Math.max(0, activeCoreRequests - 1);
      if (isInteractiveRequest) {
        activeCoreInteractiveRequests = Math.max(0, activeCoreInteractiveRequests - 1);
      }
      pumpCoreRequests();
    };
    const onAbort = () => {
      // Abort while active: free the slot and settle the caller. The underlying
      // fetch also rejects on the same signal, but `releaseSlot` is idempotent.
      releaseSlot();
      entry.reject(createAbortError());
    };
    entry.signal?.addEventListener("abort", onAbort, { once: true });
    entry.run().then(entry.resolve, entry.reject).finally(releaseSlot);
  }
}

function activeCoreResourceRequests(): number {
  return Math.max(0, activeCoreRequests - activeCoreInteractiveRequests);
}

function canStartCoreRequest(priority: CoreRequestPriority): boolean {
  if (isInteractiveCoreRequest(priority)) {
    return activeCoreInteractiveRequests < MAX_CORE_INTERACTIVE_REQUESTS;
  }
  return activeCoreResourceRequests() < MAX_CORE_REQUESTS;
}

function isInteractiveCoreRequest(priority: CoreRequestPriority): boolean {
  return coreRequestPriorityRank(priority) >= coreRequestPriorityRank("metadata");
}

function coreRequestPriorityRank(priority: CoreRequestPriority): number {
  switch (priority) {
    case "navigation":
      return 5;
    case "interactive":
      return 4;
    case "metadata":
      return 3;
    case "resource":
      return 2;
    case "background":
    default:
      return 1;
  }
}

function thumbnailPriorityCoreRequestPriority(priority: ThumbnailPriority): CoreRequestPriority {
  switch (priority) {
    case "selected":
      return "interactive";
    case "visible":
    case "ahead":
      return "resource";
    case "background":
    default:
      return "background";
  }
}

function createAbortError(): Error {
  const error = new Error("The operation was aborted.");
  error.name = "AbortError";
  return error;
}

/// Bound the complete HTTP operation: fetching headers, checking status, and
/// consuming the response body. Headers alone do not release the timer or the
/// scheduler slot, because a stalled JSON/blob stream is still live network
/// work. Every request owns an internal controller so either the caller or the
/// 60-second ceiling reaches the underlying fetch and settles the wrapper.
function fetchWithTimeout<T>(
  url: string,
  init: RequestInit,
  callerSignal: AbortSignal | null | undefined,
  consumeResponse: (response: Response) => Promise<T>
): Promise<T> {
  const requestController = new AbortController();
  const forwardCallerAbort = () => {
    requestController.abort(callerSignal?.reason ?? createAbortError());
  };
  if (callerSignal?.aborted) {
    forwardCallerAbort();
  } else {
    callerSignal?.addEventListener("abort", forwardCallerAbort, { once: true });
  }
  const timer = setTimeout(() => {
    requestController.abort(new Error("Core request timed out"));
  }, CORE_REQUEST_TIMEOUT_MS);
  let rejectOnAbort: (() => void) | null = null;
  const abortRequest = new Promise<never>((_resolve, reject) => {
    rejectOnAbort = () => {
      reject(requestController.signal.reason ?? createAbortError());
    };
    if (requestController.signal.aborted) {
      rejectOnAbort();
      return;
    }
    requestController.signal.addEventListener("abort", rejectOnAbort, { once: true });
  });
  const fetchRequest = (async () => {
    const response = await fetch(url, { ...init, signal: requestController.signal });
    return consumeResponse(response);
  })();
  return Promise.race([fetchRequest, abortRequest]).finally(() => {
    clearTimeout(timer);
    callerSignal?.removeEventListener("abort", forwardCallerAbort);
    if (rejectOnAbort) {
      requestController.signal.removeEventListener("abort", rejectOnAbort);
    }
  });
}

function resolveUrl(baseUrl: string, path: string): string {
  const normalizedBase = baseUrl.endsWith("/") ? baseUrl : `${baseUrl}/`;
  const normalizedPath = path.replace(/^\//, "");
  return new URL(normalizedPath, normalizedBase).toString();
}

function query(params: QueryParams): string {
  const search = new URLSearchParams();
  if ("rootId" in params && params.rootId) search.set("rootId", String(params.rootId));
  if ("folderId" in params && params.folderId) search.set("folderId", String(params.folderId));
  if ("includeDescendants" in params && typeof params.includeDescendants === "boolean") {
    search.set("includeDescendants", params.includeDescendants ? "true" : "false");
  }
  if (params.limit) search.set("limit", String(params.limit));
  if ("offset" in params && typeof params.offset === "number") {
    search.set("offset", String(params.offset));
  }
  if (params.cursor) search.set("cursor", params.cursor);
  if ("sort" in params && params.sort) search.set("sort", params.sort);
  if ("kind" in params && params.kind) search.set("kind", params.kind);
  if ("target" in params && params.target) search.set("target", params.target);
  if ("priority" in params && params.priority) search.set("priority", params.priority);
  if ("v" in params && params.v !== null && params.v !== undefined) {
    search.set("v", String(params.v));
  }
  // `typeof === "number"` is true for NaN and Infinity, and both stringify into
  // a value the server's typed extractor rejects with a 400 — killing the
  // preview outright. NaN is the ORDINARY failure mode here: the viewport is
  // measured off the DOM, and an unmounted or not-yet-laid-out element measures
  // as NaN. Drop the parameter instead, which degrades to "give me the
  // original" rather than to a broken request.
  if (
    "viewportLongPx" in params &&
    typeof params.viewportLongPx === "number" &&
    Number.isFinite(params.viewportLongPx)
  ) {
    search.set("viewportLongPx", String(Math.max(1, Math.round(params.viewportLongPx))));
  }
  const value = search.toString();
  return value ? `?${value}` : "";
}

function searchQuery(params: SearchParams): string {
  const search = new URLSearchParams();
  if (params.q) search.set("q", params.q);
  if (params.rootId) search.set("rootId", String(params.rootId));
  if (params.folderId) search.set("folderId", String(params.folderId));
  if (typeof params.includeDescendants === "boolean") {
    search.set("includeDescendants", params.includeDescendants ? "true" : "false");
  }
  if (params.kind) search.set("kind", params.kind);
  if (typeof params.minRating === "number") search.set("minRating", String(params.minRating));
  if (typeof params.favorite === "boolean") search.set("favorite", params.favorite ? "true" : "false");
  if (params.tagIds) {
    for (const tagId of params.tagIds) {
      search.append("tagId", String(tagId));
    }
  }
  if (params.sort) search.set("sort", params.sort);
  if (params.limit) search.set("limit", String(params.limit));
  if (typeof params.offset === "number") search.set("offset", String(params.offset));
  if (params.cursor) search.set("cursor", params.cursor);
  const value = search.toString();
  return value ? `?${value}` : "";
}

function fileOpsQuery(params: ListFileOperationsParams): string {
  const search = new URLSearchParams();
  if (params.limit) search.set("limit", String(params.limit));
  if (params.cursor) search.set("cursor", params.cursor);
  const value = search.toString();
  return value ? `?${value}` : "";
}

function notificationOutboxQuery(params: ListNotificationOutboxParams): string {
  const search = new URLSearchParams();
  if (typeof params.afterSequence !== "undefined") {
    search.set("afterSequence", String(params.afterSequence));
  }
  if (typeof params.limit !== "undefined") search.set("limit", String(params.limit));
  if (typeof params.waitMs !== "undefined") search.set("waitMs", String(params.waitMs));
  const value = search.toString();
  return value ? `?${value}` : "";
}

async function readJson(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) {
    return null;
  }
  return JSON.parse(text);
}

async function readResponseBody(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) {
    return null;
  }
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}
