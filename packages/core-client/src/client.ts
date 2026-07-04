import type {
  AcceptedRootResponse,
  AddFileTagRequest,
  CreateTagRequest,
  DeletePluginResponse,
  ThumbnailCacheClearResponse,
  ThumbnailCacheEnqueueResponse,
  ThumbnailCacheScopeParams,
  ThumbnailCacheTaskRequest,
  ThumbnailCacheStatsResponse,
  DeleteRequest,
  DeleteTagResponse,
  FileOperationListResponse,
  FileOperationRecord,
  FileOperationsResponse,
  FileTagsResponse,
  FolderRecord,
  InteractiveFolderScanTaskRequest,
  ListFileOperationsParams,
  ListFolderChildrenParams,
  ListMediaParams,
  MediaRecord,
  MoveRequest,
  Page,
  PluginDiscoveryResponse,
  PluginListResponse,
  PluginRecord,
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
  ThumbnailResponse,
  UserMetadataRecord,
  UserMetadataUpdate
} from "./generated-contract";

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
};

type QueryParams = Partial<ListMediaParams & ListFolderChildrenParams> & {
  priority?: ThumbnailPriority;
  target?: "grid_320";
  v?: number | string | null;
};

const MAX_CORE_REQUESTS = 12;
const MAX_CORE_INTERACTIVE_REQUESTS = 2;
let activeCoreRequests = 0;
let activeCoreInteractiveRequests = 0;
let coreRequestSequence = 0;
const coreRequestQueue: QueuedCoreRequest[] = [];

export function createCoreClient(config: CoreClientConfig) {
  async function request<T>(path: string, init: ScheduledRequestInit = {}): Promise<T> {
    const { requestPriority = "metadata", ...fetchInit } = init;
    const headers = new Headers(init.headers);
    if (init.body && !headers.has("content-type")) {
      headers.set("content-type", "application/json");
    }
    if (config.sessionToken) {
      headers.set("x-megle-session", config.sessionToken);
    }

    const response = await scheduleCoreRequest(requestPriority, init.signal, () =>
      fetch(resolveUrl(config.baseUrl, path), {
        ...fetchInit,
        headers
      })
    );
    if (!response.ok) {
      const body = await readResponseBody(response);
      throw new CoreApiError(response.status, body);
    }
    const body = await readJson(response);
    return body as T;
  }

  async function fetchBlob(path: string, options: BlobRequestOptions = {}): Promise<Blob> {
    const headers = new Headers();
    if (config.sessionToken) {
      headers.set("x-megle-session", config.sessionToken);
    }
    const response = await scheduleCoreRequest(options.requestPriority ?? "resource", options.signal, () =>
      fetch(resolveUrl(config.baseUrl, path), {
        headers,
        signal: options.signal
      })
    );
    if (!response.ok) {
      const body = await readResponseBody(response);
      throw new CoreApiError(response.status, body);
    }
    return response.blob();
  }

  return {
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
      request<ThumbnailCacheStatsResponse>(`/thumbnails/cache/stats${query(params)}`, {
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
    listMedia: (params: ListMediaParams = {}, options: CoreRequestOptions = {}) =>
      request<Page<MediaRecord>>(`/media${query(params)}`, {
        requestPriority: options.requestPriority ?? "navigation",
        signal: options.signal
      }),
    getMedia: (fileId: number) =>
      request<MediaRecord>(`/media/${fileId}`, { requestPriority: "interactive" }),
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
    listPlugins: () => request<PluginListResponse>("/plugins", { requestPriority: "metadata" }),
    getPlugin: (pluginId: string) =>
      request<PluginRecord>(`/plugins/${encodeURIComponent(pluginId)}`, {
        requestPriority: "metadata"
      }),
    discoverPlugins: () =>
      request<PluginDiscoveryResponse>("/plugins/discover", {
        method: "POST",
        requestPriority: "interactive"
      }),
    enablePlugin: (pluginId: string) =>
      request<PluginRecord>(`/plugins/${encodeURIComponent(pluginId)}/enable`, {
        method: "POST",
        requestPriority: "interactive"
      }),
    disablePlugin: (pluginId: string) =>
      request<PluginRecord>(`/plugins/${encodeURIComponent(pluginId)}/disable`, {
        method: "POST",
        requestPriority: "interactive"
      }),
    deletePlugin: (pluginId: string) =>
      request<DeletePluginResponse>(`/plugins/${encodeURIComponent(pluginId)}`, {
        method: "DELETE",
        requestPriority: "interactive"
      })
  };
}

function scheduleCoreRequest<T>(
  priority: CoreRequestPriority,
  signal: AbortSignal | null | undefined,
  run: () => Promise<T>
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
      abort: () => undefined
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
    // Release the slot exactly once, whichever comes first: the run settling, OR
    // the signal aborting. Relying on run() (the fetch) to always reject on abort
    // leaked a slot whenever that settlement was missed under heavy churn, and a
    // handful of leaked slots deadlocked the whole scheduler (active pinned at the
    // cap, nothing new can start). The abort watchdog guarantees the counter frees.
    let released = false;
    const releaseSlot = () => {
      if (released) {
        return;
      }
      released = true;
      entry.signal?.removeEventListener("abort", entry.abort);
      entry.signal?.removeEventListener("abort", onAbort);
      activeCoreRequests = Math.max(0, activeCoreRequests - 1);
      if (isInteractiveRequest) {
        activeCoreInteractiveRequests = Math.max(0, activeCoreInteractiveRequests - 1);
      }
      pumpCoreRequests();
    };
    const onAbort = () => {
      // Free the slot immediately and settle the caller; the underlying fetch may
      // continue but its result is ignored (release is idempotent).
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
