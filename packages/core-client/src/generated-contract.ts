// Generated-client boundary placeholder.
// Keep this file mechanically aligned with contracts/core-api/openapi.yaml until generator output replaces it.

export interface PlaybackSession {
  id: string;
  src: string;
  expiresAt: number;
  renewAfterMs: number;
}

export interface Page<T> {
  items: T[];
  nextCursor: string | null;
  totalCount?: number;
}

export interface ScanSummary {
  foldersSeen: number;
  mediaFilesSeen: number;
  filesSeen: number;
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
  | "path-changed"
  | "content-roots-required"
  | "database-content-overlap"
  | "content-roots-overlap";

export type LibraryPreflightPathRole = "database" | "content-root";

export interface LibraryContentRootInput {
  path: string;
  createIfMissing: boolean;
}

export interface LibrarySummary {
  libraryUuid: string;
  rootUuid: string;
  name: string;
  icon?: string | null;
  color?: string | null;
  databasePath: string;
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
  databasePath: string;
  contentRoots: LibraryContentRootInput[];
}

export interface LibraryPreflightResponse {
  ok: boolean;
  reason: LibraryPreflightReason;
  pathRole: LibraryPreflightPathRole | null;
  rootIndex: number | null;
  normalizedDatabasePath: string | null;
  normalizedContentRoots: LibraryContentRootInput[] | null;
  preflightToken: string | null;
  availableBytes?: number | null;
}

export interface CreateLibraryRequest {
  name: string;
  databasePath: string;
  contentRoots: LibraryContentRootInput[];
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

export interface OpenLibraryRequest {
  databasePath: string;
}

export interface OpenLibraryResponse {
  library: LibrarySummary;
  activated: boolean;
  activationError?: string | null;
}

export interface EnableLibrarySyncRequest {
  providerId: string;
  accountIdentity: string;
}

export type SyncFolderInventoryState = "unselected" | "placement-required" | "tombstoned" | "incompatible" | "inventory-error";
export type LocalRootBindingState = "materializing" | "active" | "paused" | "local-only-fork" | "reconcile-required" | "relocating" | "local-rename-required" | "unavailable" | "error";
export type SyncFolderAllowedAction = "select" | "pause" | "resume" | "stop-sync" | "re-enable" | "rebind-existing" | "relocate" | "rename" | "resolve-conflicts" | "publish-as-new" | "retry";
export type RootPathPreflightMode = "enroll-existing" | "materialize-remote" | "relocate" | "rebind-existing";
export type RootPathPreflightReason = "ok" | "invalid-path" | "path-unavailable" | "cloud-sync-root" | "reparse-point" | "path-changed" | "overlaps-active-root" | "placement-base-selected" | "marker-mismatch" | "directory-not-empty" | "insufficient-space" | "destination-exists" | "source-missing" | "root-not-selected" | "stale-binding-generation" | "stale-operation-revision" | "operation-in-progress" | "migration-required";
export type RootOperationKind = "enroll" | "select" | "stop-sync" | "re-enable" | "logical-rename" | "relocate" | "automatic-rename" | "rebind";
export type RootOperationState = "pending" | "running" | "waiting" | "completed" | "cancelled" | "failed";
export type RootOperationPhase = "planned" | "preflighting" | "reserving-identity" | "capturing-seed" | "publishing-descriptor" | "creating-binding" | "restoring-catalog" | "materializing" | "reconciling" | "fencing" | "reconciling-visibility" | "sealing-cutover" | "local-only-fork" | "comparing" | "comparison-ready" | "committing" | "verifying-existing" | "validating-heads" | "emitting-location" | "awaiting-projection" | "fenced" | "quiescing" | "transferring" | "verifying" | "catalog-cutover" | "host-cutover" | "cleanup" | "preflight-failed" | "source-missing" | "target-conflict" | "transfer-failed" | "verification-failed" | "local-rename-required" | "completed" | "cancelled" | "failed";
export type RootRelocationMode = "explicit-move" | "automatic-rename";
export type SyncRootLifecycle = "active" | "tombstoned";
export type RootComparisonStatus = "comparing" | "ready" | "committed" | "expired" | "cancelled" | "failed";
export type RootComparisonClassification = "unchanged" | "local-only" | "remote-only" | "same-change" | "conflict" | "deleted-local" | "deleted-remote";
export type RootComparisonResolution = "keep-local" | "keep-remote" | "keep-both";

export interface SyncFolderDescriptor { libraryUuid: string; libraryRootUuid: string; rootUuid: string; rootFolderUuid: string; requestedName: string; materializedName: string; materializationFingerprint: string; logicalRevision: number; lifecycle: SyncRootLifecycle; inventoryState: SyncFolderInventoryState; inventoryRevision: number; operationRevision: number; logicalBytes?: number | null; checkpointUuid?: string | null; }
export interface LocalRootBinding { localBindingUuid: string; localPath: string; placementParent: string; state: LocalRootBindingState; bindingGeneration: number; desiredMaterializedName: string; desiredMaterializationFingerprint: string; appliedMaterializedName?: string | null; appliedMaterializationFingerprint?: string | null; operationRevision: number; lastErrorCode?: string | null; }
export interface RootRebindEvidence { markerIdentity: string; priorCutoverSnapshotUuid: string; }
export interface SyncFolderRecord { descriptor: SyncFolderDescriptor; binding: LocalRootBinding | null; allowedActions: SyncFolderAllowedAction[]; selectedByDefault?: boolean; proposedLocalPath?: string | null; locationHeads?: string[]; rebindEvidence?: RootRebindEvidence | null; lastSyncedAt?: number | null; activeOperation?: RootOperation | null; latestOperation?: RootOperation | null; }
export interface SyncFolderListResponse { revision: number; items: SyncFolderRecord[]; }
export interface SyncFolderSettings { libraryUuid: string; libraryRootUuid: string; defaultPlacementBase: string; operationRevision: number; }
export interface UpdateSyncFolderSettingsRequest { operationUuid: string; expectedOperationRevision: number; defaultPlacementBase: string; }
export interface RootMutationRequest { operationUuid: string; expectedOperationRevision: number; expectedBindingGeneration?: number | null; }
export interface RootOperationFenceRequest extends RootMutationRequest { expectedBindingGeneration: number; }
export interface RootPathPreflightRequest extends RootMutationRequest { rootUuid?: string | null; mode: RootPathPreflightMode; path: string; }
export interface RootPathPreflightResult { rootUuid: string | null; ok: boolean; reason: RootPathPreflightReason; normalizedPath: string; proposedMaterializedName?: string | null; preflightToken: string | null; expiresAt: number | null; availableBytes?: number | null; }
export interface RootEnrollmentCommitRequest { operationUuid: string; expectedOperationRevision: number; rootUuid: string; rootFolderUuid: string; preflightToken: string; path: string; requestedName: string; }
export interface RootSelectionItem { rootUuid: string; operationUuid: string; expectedOperationRevision: number; expectedBindingGeneration?: number | null; selected: boolean; path: string; }
export interface RootSelectionPreflightRequest { items: RootSelectionItem[]; }
export interface RootSelectionPreflightResult { preflightToken: string; expiresAt: number; items: RootPathPreflightResult[]; }
export interface RootSelectionCommitRequest { preflightToken: string; items: RootSelectionItem[]; }
export interface RootRebindPreflightRequest { operationUuid: string; expectedOperationRevision: number; expectedBindingGeneration?: number | null; path: string; markerIdentity: string; priorCutoverSnapshotUuid: string; }
export interface RootRebindPlan { planUuid: string; rootUuid: string; path: string; markerMatches: boolean; contentEvidenceDigest: string; expiresAt: number; }
export interface RootRebindCommitRequest { operationUuid: string; expectedOperationRevision: number; expectedBindingGeneration?: number | null; planUuid: string; }
export interface RootCommandRequest extends RootMutationRequest { expectedBindingGeneration: number; }
export interface RootStopCommitRequest extends RootOperationFenceRequest { planUuid: string; }
export interface RootStopPlan { planUuid: string; operationUuid: string; rootUuid: string; localBindingUuid: string; bindingGeneration: number; rootCaptureEpoch: string; rootActorEpoch: string; pendingWork: number; publishedWork: number; checkpointUuid?: string | null; eventFrontier: unknown; catalogSnapshotDigest: string; providerReceipts: unknown[]; expiresAt: number; }
export interface RootComparisonPlan { planUuid: string; operationUuid: string; rootUuid: string; baseDigest: string; localDigest: string; remoteDigest: string; baseFrontier: unknown; localFrontier: unknown; remoteFrontier: unknown; rootRevision: number; accountGeneration: number; providerGeneration: number; status: RootComparisonStatus; itemCount: number; conflictCount: number; chosenMode: "merge-local-fork" | "publish-as-new" | null; expiresAt: number; }
export interface RootComparisonItem { itemOrdinal: number; entityUuid: string; fieldName: string; classification: RootComparisonClassification; suggestedAction?: RootComparisonResolution | null; resolution: RootComparisonResolution | null; conflictUuid?: string | null; redactedDetail?: unknown | null; }
export interface RootComparisonItemListResponse { items: RootComparisonItem[]; nextCursor: number | null; }
export interface RootComparisonItemResolutionRequest extends RootMutationRequest { expectedBindingGeneration: number; resolution: RootComparisonResolution; }
export interface RootReenableCommitRequest { operationUuid: string; expectedOperationRevision: number; expectedBindingGeneration: number; planUuid: string; mode: "merge-local-fork" | "publish-as-new"; }
export interface RootLogicalRenameRequest { operationUuid: string; expectedOperationRevision: number; expectedBindingGeneration?: number | null; requestedName: string; expectedLocationHeads: string[]; supersedes: string[]; }
export interface RootLogicalRenamePlan { planUuid: string; requestedName: string; materializedName: string; materializationFingerprint: string; expectedLocationHeads: string[]; supersedes: string[]; expiresAt: number; }
export interface RootLogicalRenameCommitRequest extends RootLogicalRenameRequest { planUuid: string; }
export interface RootRelocationPreflightRequest { operationUuid: string; expectedOperationRevision: number; expectedBindingGeneration: number; mode: RootRelocationMode; targetPath: string; }
export interface RootRelocationCommitRequest extends RootRelocationPreflightRequest { preflightToken: string; desiredMaterializationFingerprint: string; }
export interface RootOperation { operationUuid: string; libraryUuid: string; rootUuid: string; kind: RootOperationKind; state: RootOperationState; phase: RootOperationPhase; operationRevision: number; cutoverCommitted: boolean; errorCode?: string | null; transferMode?: RootRelocationMode | null; bytesDone?: number | null; bytesTotal?: number | null; itemsDone?: number | null; itemsTotal?: number | null; availableBytes?: number | null; canCancel?: boolean; cancellationBoundary?: "before-cutover" | "after-cutover" | null; recoveryState?: string | null; createdAt: number; updatedAt: number; }
export interface RootOperationListResponse { items: RootOperation[]; }
export interface RootOperationPageResponse { items: RootOperation[]; nextCursor: string | null; }
export interface RemoteLibraryRoot { accountUuid: string; libraryUuid: string; rootUuid: string; rootFolderUuid: string; requestedName: string; materializedName: string; materializationFingerprint: string; logicalRevision: number; logicalBytes: number | null; proposedLocalPath: string | null; selectedByDefault: boolean; compatible: boolean; errorCode: string | null; }
export interface RemoteLibraryRootListResponse { items: RemoteLibraryRoot[]; nextCursor: string | null; inventoryFence: string; }
export interface RootLocationConflict { conflictUuid: string; rootUuid: string; locationHeads: string[]; state: "open" | "resolved"; createdAt: number; resolvedAt?: number | null; }
export interface RootConflictListResponse { items: RootLocationConflict[]; nextCursor: string | null; }
export interface RootLocationConflictResolutionRequest { operationUuid: string; expectedOperationRevision: number; expectedBindingGeneration?: number | null; expectedLocationHeads: string[]; supersedes: string[]; requestedName?: string | null; }
export interface SyncFolderErrorResponse { code: "invalid-input" | "invalid-path" | "library-not-found" | "root-not-found" | "operation-not-found" | "conflict-not-found" | "stale_library_context" | "stale-operation-revision" | "stale-binding-generation" | "stale-location-heads" | "idempotency-key-reused" | "path-occupied" | "path-overlap" | "invalid-lifecycle-transition" | "operation-in-progress" | "cutover-committed" | "comparison-required" | "comparison-expired" | "migration-required" | "internal-error"; message: string; operationUuid?: string | null; }

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
export interface SyncRestoreLegacyItem { restoreMode?: "legacy-library"; accountUuid: string; libraryUuid: string; name: string; placementBase: string; }
export interface SyncRestoreRootChoice { rootUuid: string; expectedLogicalRevision: number; expectedMaterializationFingerprint: string; selected: boolean; targetPath?: string; }
export interface SyncRestoreRootAwareItem { restoreMode: "root-aware"; accountUuid: string; libraryUuid: string; placementBase: string; inventoryFence: string; roots: SyncRestoreRootChoice[]; }
export type SyncRestoreStartItem = SyncRestoreLegacyItem | SyncRestoreRootAwareItem;
export interface SyncRestorePreflightRequest { items: SyncRestoreStartItem[]; }
export interface SyncRestoreRootResult { libraryUuid: string; rootUuid: string; selected: boolean; ok: boolean; reason: string | null; normalizedTargetPath: string | null; requiredBytes: number; availableBytes: number | null; volumeKey: string | null; }
export interface SyncRestoreVolumeResult { volumeKey: string; requiredBytes: number; availableBytes: number | null; ok: boolean; }
export interface SyncRestorePreflight { preflightToken: string | null; expiresAt: number; totalItems: number; totalBytes: number; ok: boolean; rootResults: SyncRestoreRootResult[]; volumeResults: SyncRestoreVolumeResult[]; }
export interface SyncRestoreStartRequest { operationUuid: string; preflightToken: string; items: SyncRestoreStartItem[]; }
export interface SyncRestorePlan { restoreUuid: string; operationUuid: string; accountUuid: string; libraryUuid: string; name: string; databasePath: string; placementBase: string; contentPaths: string[]; state: string; phase: string; catalogReady: boolean; mediaItemsDone: number; mediaItemsTotal?: number | null; mediaBytesDone: number; mediaBytesTotal?: number | null; errorCode?: string | null; updatedAt: number; }
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
  icon: string | null;
  color: string | null;
  id: number;
  path: string;
  displayName: string;
  enabled: boolean;
  createdAt: number;
  lastScanAt: number | null;
  rootFolderId: number | null;
  mediaCount: number;
  fileCount: number;
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
  filesSeen: number;
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
  libraryUuid?: string | null;
  rootUuid?: string | null;
  managedRootAction?: ManagedRootNotificationAction | null;
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

export interface ManagedRootNotificationAction {
  kind: "review-placement";
  libraryUuid: string;
  rootUuid: string;
  inventoryRevision: number;
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

export interface UpdateFolderAppearanceRequest {
  icon: string;
  color: string;
}

/** Root and ordinary folder appearance share the same paired validation. */
export type UpdateRootAppearanceRequest = UpdateFolderAppearanceRequest;

export type DirectorySortOrder = "name_asc" | "name_desc";
export interface DirectorySortRule { order: DirectorySortOrder; recursive: boolean; }
export interface DirectorySortSource { rootId: number; folderId: number | null; }
export interface DirectorySortState {
  rootId: number;
  folderId: number | null;
  explicitRule: DirectorySortRule | null;
  effectiveOrder: DirectorySortOrder;
  source: DirectorySortSource | null;
  revision: string;
}
export interface DirectorySortMutationResult {
  state: DirectorySortState;
  invalidateDescendants: boolean;
}
export interface FolderChildrenPage extends Page<FolderRecord> {
  sortState: DirectorySortState;
}

export interface FolderRecord {
  icon: string | null;
  color: string | null;
  id: number;
  rootId: number;
  parentId: number | null;
  name: string;
  status: string;
  hasChildren: boolean;
  mediaCount: number;
  fileCount: number;
}

export interface FilePresentation {
  previewKind: "original-image" | "original-video" | "static-thumbnail";
  thumbnail: "available" | "unavailable";
  generation: number;
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
  presentation?: FilePresentation;
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
  state: "pending" | "queued" | "ready" | "failed" | "skipped_small" | "unavailable";
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

export type PluginCapability = "sync-provider.v1" | "file-decoder.v1";
export type PluginRuntimeState =
  | "disabled"
  | "stopping"
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
  | { kind: "temp-storage"; maxBytes: number }
  | { kind: "host-ui"; operations: ("core-api" | "pick-folder" | "reveal-path" | "clipboard-write" | "external-link")[] };

export interface HostPluginViewDocument {
  pluginId: string;
  lifecycleEpoch: number;
  version: string;
  viewId: string;
  title: string;
  document: string;
}

export type PluginUiSlot = "library-menu" | "library-settings" | "plugin-detail" | "root-menu" | "notification-center";
export interface PluginUiView {
  id: string;
  title: string;
  slot: PluginUiSlot;
}
export interface PluginUiContribution {
  apiVersion: 1;
  entry: string;
  sha256: string;
  views: PluginUiView[];
}

export interface HostPluginProjection {
  contributions: PluginContributions | null;
  lifecycleEpoch: number;
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
  completionMode: "loopback" | "manual-response-uri" | "host-callback";
  redirectUri: string | null;
}
export interface PluginAuthCompleteRequest {
  flowId: string;
  responseUri: string;
}
export interface PluginAuthCancelRequest {
  flowId: string;
}
export interface PluginConfigurationField {
  id: string;
  label: string;
  description: string;
  format: "uuid" | "text" | "choice" | "https-url" | "loopback-url";
  required: boolean;
  environment: string | null;
  defaultValue: string | null;
  choices: string[];
  hidden: boolean;
  debugOnly: boolean;
}
export interface PluginAuthenticationContribution {
  modes: ("loopback" | "manual-response-uri" | "host-callback")[];
  authorizationHosts: string[];
  manualRedirectUri: string | null;
  loopbackRedirectPath: string | null;
  ignoredCallbackFields: string[];
}
export interface PluginPresentationContribution {
  setupLabel: string;
  accountLabel: string;
  icon: "cloud" | "storage";
  helpLinks: { id: string; label: string; url: string }[];
  setupSteps: { title: string; body: string; helpLinkId: string | null }[];
}
export interface PluginContributions {
  ui?: PluginUiContribution;
  configuration: PluginConfigurationField[];
  authentication: PluginAuthenticationContribution | null;
  presentation: PluginPresentationContribution | null;
  primaryCredential: { namespace: string; purpose: string } | null;
}
export interface PluginConfiguration {
  /** Aggregate precedence across visible fields: missing, environment, host-override, default. */
  source: "environment" | "host-override" | "missing" | "default";
  configured: boolean;
  /** True only when every visible field is environment-locked; use lockedFields per field. */
  readOnly: boolean;
  revision: number;
  errorCode: string | null;
  values: Record<string, string>;
  fields: PluginConfigurationField[];
  lockedFields: string[];
}
export interface PluginConfigurationPlanRequest { values: Record<string, string | null>; expectedRevision: number; }
export interface PluginConfigurationPlan { planUuid: string; expectedRevision: number; effectiveChanged: boolean; affectedLibraries: number; requiresConfirmation: boolean; expiresAt: number; }
export interface PluginConfigurationCommitRequest { planUuid: string; expectedRevision: number; confirmed: boolean; }
export interface PluginExternalLinkRequest { kind: "authorization" | "help"; id: string; }
export interface PluginExternalLink { kind: "authorization" | "help"; id: string; url: string; expiresAt: number | null; }
export interface PluginHealthResponse {
  state: PluginRuntimeState;
  lastHealthyAt: number | null;
  lastErrorCode: string | null;
}
export interface PluginDiagnosticsResponse {
  generatedAt: number;
  entries: Record<string, unknown>[];
}
