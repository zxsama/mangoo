// Generated-client boundary placeholder.
// Keep this file mechanically aligned with contracts/core-api/openapi.yaml until generator output replaces it.

export interface Page<T> {
  items: T[];
  nextCursor: string | null;
  totalCount?: number;
}

export interface ScanSummary {
  foldersSeen: number;
  mediaFilesSeen: number;
  skippedFiles: number;
}

export interface AcceptedRootResponse {
  accepted: boolean;
  taskId: number | null;
  rootId: number | null;
  scan: ScanSummary | null;
}

export interface ScanTaskRequest {
  rootId: number;
}

export interface InteractiveFolderScanTaskRequest {
  folderId: number;
}

export interface ThumbnailPriorityScopeSyncRequest {
  rootId: number;
  selectedFileIds: number[];
  visibleFileIds: number[];
  aheadFileIds: number[];
  // Files to keep warm (never cancel) even when out of scope — the client's
  // recency grace window so scrolling back does not lose/regenerate a tile.
  retainedFileIds?: number[];
  // Current generation mode: renderedOnly/cacheOnly cancel a pending task that
  // has left the scope + retention window; allPersist demotes it to bulk.
  generationMode?: "renderedOnly" | "allPersist" | "cacheOnly";
}

export type ThumbnailGenerationMode = "renderedOnly" | "allPersist" | "cacheOnly";

export interface ThumbnailCacheScopeParams {
  rootId?: number;
  folderId?: number;
  includeDescendants?: boolean;
}

export type ThumbnailCacheRefreshMode =
  | "missingOnly"
  | "staleOrMissing"
  | "retryFailedAndStale";

export interface ThumbnailCacheTaskRequest extends ThumbnailCacheScopeParams {
  fileIds?: number[];
  refreshMode: ThumbnailCacheRefreshMode;
  limit?: number;
  afterFileId?: number;
}

export interface ThumbnailCacheStatsResponse {
  cachedCount: number;
  missingCount: number;
  staleCount: number;
  failedCount: number;
  pendingCandidateCount: number;
  totalBlobBytes: number;
  activeBulkTaskCount: number;
}

export interface ThumbnailCacheEnqueueResponse extends ThumbnailCacheStatsResponse {
  acceptedCount: number;
  nextCursor?: number;
}

export interface ThumbnailCacheClearResponse {
  cleared: boolean;
  deletedBlobCount: number;
  resetThumbnailCount: number;
}

export interface ThumbnailCacheCancelBulkResponse {
  cancelledCount: number;
}

export type ThumbnailPriority = "background" | "ahead" | "visible" | "selected";
export interface RootRecord {
  id: number;
  path: string;
  displayName: string;
  enabled: boolean;
  createdAt: number;
  lastScanAt: number | null;
  rootFolderId: number | null;
  mediaCount: number;
  reachable: boolean;
}

export type TaskKind = "root_scan" | "interactive_folder_scan" | "thumbnail";
export type TaskStatus = "pending" | "running" | "succeeded" | "failed" | "cancelled";

export interface TaskRecord {
  id: number;
  kind: TaskKind;
  priority: number;
  status: TaskStatus;
  rootId: number | null;
  folderId: number | null;
  fileId: number | null;
  createdAt: number;
  updatedAt: number;
  itemsSeen: number;
  itemsTotal: number | null;
  foldersSeen: number;
  mediaFilesSeen: number;
  skippedFiles: number;
  error: string | null;
}

export interface FolderRecord {
  id: number;
  rootId: number;
  parentId: number | null;
  name: string;
  status: string;
  hasChildren: boolean;
  mediaCount: number;
}

export interface MediaRecord {
  id: number;
  rootId: number;
  folderId: number;
  name: string;
  ext: string;
  size: number;
  mtime: number;
  kind?: string | null;
  width?: number | null;
  height?: number | null;
  durationMs?: number | null;
  codec?: string | null;
  previewPlaceholder?: number[] | null;
  previewPlaceholderFormat?: string | null;
  thumbnailState?: string | null;
  thumbnailCacheKey?: string | null;
  rating?: number | null;
  favorite?: boolean;
  note?: string | null;
  tagIds?: number[];
}

export interface TagRecord {
  id: number;
  name: string;
  color: string | null;
}

export interface TagListResponse {
  items: TagRecord[];
}

export interface CreateTagRequest {
  name: string;
  color?: string | null;
}

export interface DeleteTagResponse {
  deleted: boolean;
}

export interface UserMetadataRecord {
  fileId: number;
  rating: number | null;
  favorite: boolean;
  note: string | null;
  tagIds: number[];
  updatedAt: number;
}

export interface UserMetadataUpdate {
  rating?: number | null;
  favorite?: boolean;
  note?: string | null;
}

export interface FileTagsResponse {
  fileId: number;
  tagIds: number[];
}

export interface AddFileTagRequest {
  tagId: number;
}

export interface SetFileTagsRequest {
  tagIds: number[];
}

export interface ThumbnailAsset {
  width: number;
  height: number;
  byteSize: number;
}

export interface ThumbnailResponse {
  fileId: number;
  target: "grid_320";
  state: "pending" | "queued" | "ready" | "failed" | "skipped_small";
  shortSidePx: number;
  outputFormat: "image/webp";
  width: number | null;
  height: number | null;
  byteSize: number | null;
  servedBy: "db_blob" | null;
  asset: ThumbnailAsset | null;
  error: string | null;
  updatedAt: number | null;
}

export interface ListFolderChildrenParams {
  limit?: number;
  cursor?: string;
  includeDescendants?: boolean;
}

export interface ListMediaParams {
  rootId?: number;
  folderId?: number;
  includeDescendants?: boolean;
  limit?: number;
  offset?: number;
  cursor?: string;
  sort?: "mtime_desc" | "mtime_asc" | "name_asc" | "name_desc";
  kind?: "image" | "video" | "other";
}

export interface SearchParams {
  q?: string;
  rootId?: number;
  folderId?: number;
  includeDescendants?: boolean;
  kind?: "image" | "video" | "other";
  minRating?: number;
  favorite?: boolean;
  tagIds?: number[];
  sort?:
    | "mtime_desc"
    | "mtime_asc"
    | "name_asc"
    | "name_desc"
    | "rating_desc"
    | "rating_asc";
  limit?: number;
  offset?: number;
  cursor?: string;
}

export type FileOperationKind =
  | "rename"
  | "move"
  | "delete_recycle"
  | "delete_permanent";
export type FileOperationStatus = "succeeded" | "failed";

export interface FileOperationRecord {
  id: number;
  operation: FileOperationKind;
  sourcePath: string;
  targetPath: string | null;
  status: FileOperationStatus;
  createdAt: number;
  finishedAt: number | null;
  error: string | null;
}

export interface FileOperationListResponse {
  items: FileOperationRecord[];
  nextCursor: string | null;
}

export interface FileOperationsResponse {
  operations: FileOperationRecord[];
}

export interface RenameRequest {
  fileId?: number;
  folderId?: number;
  newName: string;
}

export interface MoveRequest {
  fileIds?: number[];
  folderIds?: number[];
  targetFolderId: number;
}

export interface DeleteRequest {
  fileIds?: number[];
  folderIds?: number[];
  permanent?: boolean;
}

export interface ListFileOperationsParams {
  limit?: number;
  cursor?: string;
}

export type PluginCapability = "decoder" | "metadata" | "action" | "import-provider";
export type PluginStatus = "registered" | "invalid" | "enabled" | "disabled";

export interface PluginRecord {
  id: string;
  name: string;
  version: string;
  description: string | null;
  enabled: boolean;
  status: PluginStatus;
  capabilities: PluginCapability[];
  permissions: string[];
  manifestPath: string;
  installedAt: number;
  updatedAt: number;
  lastError: string | null;
}

export interface PluginListResponse {
  items: PluginRecord[];
}

export interface PluginDiscoveryError {
  manifestPath: string;
  message: string;
}

export interface PluginDiscoveryResponse {
  discovered: number;
  errors: PluginDiscoveryError[];
}

export interface DeletePluginResponse {
  deleted: boolean;
}
