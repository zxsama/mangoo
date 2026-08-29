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

export type LibraryLifecycleState = "creating" | "ready" | "removing" | "error" | "recovery-required";
export type LibraryBindingState = "local-only" | "synchronizing" | "synchronized";
export type LibraryAvailability = "available" | "unavailable";
export type LibraryPreflightReason =
  | "ok"
  | "invalid-path"
  | "invalid-name"
  | "cloud-sync-root"
  | "reparse-point"
  | "nested-library-root"
  | "not-empty"
  | "marker-mismatch"
  | "not-writable"
  | "insufficient-space"
  | "path-changed";

export interface LibrarySummary {
  libraryUuid: string;
  rootUuid: string;
  name: string;
  icon?: string | null;
  color?: string | null;
  managedRoot: string;
  active: boolean;
  availability: LibraryAvailability;
  lifecycleState: LibraryLifecycleState;
  bindingState: LibraryBindingState;
  contextRevision: number;
}

export interface LibraryListResponse {
  items: LibrarySummary[];
  contextRevision: number;
}

export interface ActiveLibraryResponse {
  library: LibrarySummary | null;
  contextRevision: number;
  runtimeGeneration: number;
}

export interface LibraryPreflightRequest {
  name: string;
  managedRoot: string;
}

export interface LibraryPreflightResponse {
  ok: boolean;
  reason: LibraryPreflightReason;
  normalizedRoot: string;
  preflightToken: string | null;
  availableBytes?: number | null;
}

export interface CreateLibraryRequest {
  name: string;
  managedRoot: string;
  icon?: string | null;
  color?: string | null;
  initialMode: "local-only";
  preflightToken?: string | null;
  activate?: boolean;
}

export interface CreateLibraryResponse {
  library: LibrarySummary;
  activated: boolean;
  activationError?: string | null;
}

export interface UpdateLibraryRequest {
  name?: string;
  icon?: string | null;
  color?: string | null;
}

export interface EnableLibrarySyncRequest {
  providerId: string;
  accountIdentity: string;
}

export type LibrarySyncLifecycleState = "local-only" | "enabling" | "syncing" | "paused" | "unbinding" | "restoring" | "error";
export type LibrarySyncPhase = "preflight" | "capturing" | "uploading" | "publishing" | "active" | "reconcile" | "catalog-restore" | "catalog-ready" | "restoring-media" | "complete" | "retry-wait" | "compatibility-locked" | "cancelled";
export type CatalogReadiness = "not-created" | "restoring" | "catalog-ready" | "ready";
export type SyncAllowedAction = "enable" | "pause" | "resume" | "retry" | "sync-now" | "cancel-before-catalog" | "unbind" | "resolve-conflicts" | "reconnect";
export interface SyncProviderStatus { pluginId: string; accountUuid: string; accountDisplayName?: string | null; accountKind?: "personal" | "business" | null; runtimeState: string; authState: string; lastHealthyAt?: number | null; lastErrorCode?: string | null; }
export interface LibrarySyncStatus { libraryUuid: string; name: string; lifecycle: LibrarySyncLifecycleState; phase: LibrarySyncPhase; catalogReadiness: CatalogReadiness; providerId?: string | null; accountUuid?: string | null; remoteLibraryUuid: string | null; allowedActions: SyncAllowedAction[]; lastSuccessAt?: number | null; lastErrorCode?: string | null; retryAfter?: number | null; pollAfter?: number | null; pendingUploadItems: number; pendingDownloadItems: number; pendingUploadBytes: number; pendingDownloadBytes: number; conflictCount: number; localDivergenceCount: number; operationRevision: number; restoreUuid?: string | null; restoreItemsDone?: number | null; restoreItemsTotal?: number | null; restoreBytesDone?: number | null; restoreBytesTotal?: number | null; }
export interface SyncStatusSnapshot { revision: number; generatedAt: number; schedulerState: "starting" | "running" | "stopped" | "degraded"; providers: SyncProviderStatus[]; libraries: LibrarySyncStatus[]; }
export interface SyncOperation { operationUuid: string; libraryUuid?: string | null; kind: string; state: "pending" | "running" | "waiting" | "complete" | "failed" | "cancelled"; phase: string; errorCode?: string | null; updatedAt: number; }
export interface SyncCommandRequest { operationUuid: string; }
export interface SyncPreflightRequest { providerId: string; accountUuid: string; remoteLibraryUuid: string | null; }
export interface SyncPreflightResponse { ok: boolean; reason: string; preflightToken: string | null; remoteLibraryUuid: string | null; }
export interface SyncEnableRequest { operationUuid: string; providerId: string; accountUuid: string; preflightToken: string; }
export interface SyncUnbindPlan { planUuid: string; libraryUuid: string; pendingCount: number; publishedCount: number; unpublishedCount: number; cutoverActorSeq: number; remoteWatermark: number; originIdentity?: string | null; }
export interface SyncUnbindCommitRequest { operationUuid: string; planUuid: string; choice: "drain-before-unbind" | "keep-local-fork"; }
export interface SyncReconnectPreflightRequest { accountUuid: string; remoteLibraryUuid: string; }
export interface SyncReconnectPlan { planUuid: string; libraryUuid: string; remoteLibraryUuid: string; originIdentity?: string | null; localDivergenceCount: number; remoteDivergenceCount: number; remoteExists: boolean; remoteCompatible: boolean; sameRemoteIdentity: boolean; originRemoteWatermark: number; baseRemoteObjectCount: number; remoteObjectCount: number; remoteDescriptorSha256?: string | null; remoteBindingEpoch?: string | null; remoteAdvanced: boolean; mergeAllowed: boolean; publishNewAllowed: boolean; }
export interface SyncReconnectCommitRequest { operationUuid: string; planUuid: string; choice: "merge-local-fork" | "publish-new-remote"; }
export interface RemoteLibrarySummary { accountUuid: string; libraryUuid: string; name: string; icon: string | null; mediaCount: number | null; logicalBytes: number | null; lastSyncedAt: number | null; descriptorSize: number; updatedAt: number; compatible: boolean; boundLocally: boolean; errorCode: string | null; }
export interface RemoteLibraryListResponse { items: RemoteLibrarySummary[]; nextCursor: string | null; totalCount?: number; }
export interface SyncRestoreStartItem { accountUuid: string; libraryUuid: string; name: string; managedRoot: string; }
export interface SyncRestorePreflightRequest { items: SyncRestoreStartItem[]; }
export interface SyncRestorePreflight { preflightToken: string; expiresAt: number; totalItems: number; totalBytes: number; }
export interface SyncRestoreStartRequest { operationUuid: string; preflightToken: string; items: SyncRestoreStartItem[]; }
export interface SyncRestorePlan { restoreUuid: string; operationUuid: string; accountUuid: string; libraryUuid: string; name: string; managedRoot: string; state: string; phase: string; catalogReady: boolean; mediaItemsDone: number; mediaItemsTotal?: number | null; mediaBytesDone: number; mediaBytesTotal?: number | null; errorCode?: string | null; updatedAt: number; }
export interface SyncRestoreListResponse { items: SyncRestorePlan[]; }
export interface SyncConflictRecord { conflictUuid: string; entityUuid?: string | null; conflictKind: string; state: string; details?: unknown | null; createdAt: number; resolvedAt?: number | null; }
export interface SyncConflictListResponse { items: SyncConflictRecord[]; nextCursor: string | null; }
export interface SyncConflictResolutionRequest { operationUuid: string; resolution: "resolve" | "ignore"; }

export interface LibraryErrorResponse {
  code: string;
  message: string;
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

export interface ThumbnailFailureRecord {
  fileId: number;
  folderId: number;
  rootId: number;
  relativePath: string;
  failureKind: string;
  error: string;
  updatedAt: number;
}

export interface ThumbnailFailureListResponse {
  items: ThumbnailFailureRecord[];
  truncated: boolean;
}

export interface ThumbnailCacheSnapshotResponse extends ThumbnailCacheStatsResponse {
  failureDetails: ThumbnailFailureRecord[];
  failureDetailsTruncated: boolean;
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

export type TaskKind = "root_scan" | "interactive_folder_scan" | "thumbnail" | "root_purge";
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

export type NotificationOrigin = "user" | "background" | "recovery" | "system";
export type NotificationPresentation = "important" | "history_only" | "status_only";
export type NotificationOperationStatus =
  | "pending"
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled";

export interface NotificationOperationSnapshot {
  operationId: string;
  revision: number;
  kind: string;
  category: "task" | "file_operation" | "error" | "thumbnail_cache" | "sync";
  origin: NotificationOrigin;
  presentation: NotificationPresentation;
  status: NotificationOperationStatus;
  phase: string | null;
  title: string;
  summary: string | null;
  taskId: number | null;
  rootId: number | null;
  folderId: number | null;
  fileId: number | null;
  currentValue: number;
  totalValue: number | null;
  unit: string | null;
  errorCode: string | null;
  errorDetail: string | null;
  cancellable: boolean;
  retryable: boolean;
  attempt: number;
  occurrenceCount: number;
  startedAt: number;
  updatedAt: number;
  completedAt: number | null;
}

export interface NotificationOutboxEvent {
  /** Integer in the inclusive range 0..Number.MAX_SAFE_INTEGER. */
  sequence: number;
  operation: NotificationOperationSnapshot;
}

export interface NotificationOutboxResponse {
  items: NotificationOutboxEvent[];
  /** Integer in the inclusive range 0..Number.MAX_SAFE_INTEGER. */
  maxSequence: number;
}

export interface NotificationOutboxAckRequest {
  /** Integer in the inclusive range 0..Number.MAX_SAFE_INTEGER. */
  maxSequence: number;
}

export interface NotificationOutboxAckResponse {
  acknowledged: boolean;
}

export interface ListNotificationOutboxParams {
  /** Integer in the inclusive range 0..Number.MAX_SAFE_INTEGER. */
  afterSequence?: number;
  limit?: number;
  waitMs?: number;
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

export interface MediaPositionResponse {
  index: number;
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
  operationGroupId?: string | null;
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

export type PluginCapability = "sync-provider.v1";
export type PluginRuntimeState =
  | "disabled"
  | "permission-required"
  | "starting"
  | "ready"
  | "degraded"
  | "crashed-backoff"
  | "incompatible"
  | "configuration-required";
export type PluginAuthState =
  | "signed-out"
  | "authorization-required"
  | "authorizing"
  | "authenticated"
  | "expired"
  | "error";
export type PluginPermission =
  | { kind: "network"; schemes: "https"[]; hosts: string[] }
  | { kind: "oauth-loopback"; mode: "ephemeral-only" }
  | { kind: "secret"; namespace: string; operations: ("read" | "write" | "delete")[] }
  | { kind: "temp-storage"; maxBytes: number };

export interface HostPluginProjection {
  pluginId: string;
  name: string;
  selectedVersion: string;
  knownGoodVersion: string | null;
  enabledIntent: boolean;
  runtimeState: PluginRuntimeState;
  authState: "signed-out" | "authorization-required" | "authorizing" | "authenticated" | "expired" | "error" | null;
  capabilities: PluginCapability[];
  requestedPermissions: PluginPermission[];
  grantedPermissions: PluginPermission[];
  permissionRequired: boolean;
  accountUuid: string | null;
  accountDisplayName: string | null;
  accountKind: "personal" | "business" | null;
  compatibilityReason: string | null;
  backoffUntil: number | null;
  lastHealthyAt: number | null;
  lastErrorCode: string | null;
  lastErrorDetail: string | null;
}

export interface HostPluginListResponse {
  items: HostPluginProjection[];
}

export interface PluginDiscoveryError {
  manifestPath: string;
  message: string;
}

export interface PluginDiscoveryResponse {
  discovered: number;
  errors: PluginDiscoveryError[];
}

export interface PluginCommandResponse {
  accepted: boolean;
}
export interface PluginGrantRequest {
  permissions: PluginPermission[];
}
export interface PluginVersionRequest {
  version: string;
}
export interface PluginAuthBeginResponse {
  authorizationUrl: string;
  flowId: string;
  expiresAt: number;
  completionMode: "loopback" | "manual-response-uri";
  redirectUri: string | null;
}
export interface PluginAuthCompleteRequest {
  flowId: string;
  responseUri: string;
}
export interface PluginAuthCancelRequest {
  flowId: string;
}
export interface OneDriveConfiguration {
  source: "environment" | "host-override" | "missing";
  configured: boolean;
  readOnly: boolean;
  revision: number;
  errorCode: string | null;
}
export interface OneDriveConfigurationPlanRequest { clientId: string | null; expectedRevision: number; }
export interface OneDriveConfigurationPlan { planUuid: string; expectedRevision: number; effectiveChanged: boolean; affectedLibraries: number; requiresConfirmation: boolean; expiresAt: number; }
export interface OneDriveConfigurationCommitRequest { planUuid: string; expectedRevision: number; confirmed: boolean; }
export interface PluginHealthResponse {
  state: PluginRuntimeState;
  lastHealthyAt: number | null;
  lastErrorCode: string | null;
}
export interface PluginDiagnosticsResponse {
  generatedAt: number;
  entries: Record<string, unknown>[];
}
