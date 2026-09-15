pub mod migrations;
pub mod notification_outbox;
mod sync_roots;
pub mod writer;

#[allow(unused_imports)]
pub use sync_roots::{SyncRootBindingToken, SyncRootDescriptorRecord, SyncRootLocalBindingRecord};
pub use writer::{DbWriter, WriteDurability};

use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;

use crate::debug_log;
use crate::thumbnails::{
    is_safe_cache_key, source_fingerprint_for, CacheIdentity, ThumbnailDecision, ThumbnailPolicy,
    GENERATED_FORMAT, GRID_320_PROFILE, GRID_320_SHORT_SIDE_PX,
};

#[allow(dead_code)]
pub const WAL_MODE: &str = "WAL";
// 0 disables SQLite's inline auto-checkpoint; a dedicated thread checkpoints
// off the hot path (see `run_periodic_wal_checkpoint`). Keeping some inline
// checkpointing as a safety net would reintroduce the mid-write freeze this
// avoids, so it is fully off and the thread is responsible for bounding the WAL.
const WAL_AUTOCHECKPOINT_PAGES: i64 = 0;
const WAL_JOURNAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024;
pub(crate) const WAL_TRUNCATE_THRESHOLD_BYTES: u64 = 256 * 1024 * 1024;
/// `synchronous=NORMAL` (1) is the recommended setting under WAL: it never
/// corrupts the database and only risks losing the last few *committed*
/// transactions on OS crash / power loss. Every Megle table (thumbnail
/// blobs, the file index, the task queue) is regenerable by rescanning, so
/// this trade is safe — and it removes one fsync per commit, the dominant
/// cost when the worker persists millions of thumbnails one row at a time.
const SQLITE_SYNCHRONOUS_NORMAL: i64 = 1;
const SQLITE_SYNCHRONOUS_FULL: i64 = 2;
/// Negative values are a size in KiB. -65536 → a 64 MiB page cache per
/// connection so hot browse/scan joins stay resident instead of re-reading
/// pages from disk on every grid page and thumbnail lookup.
const SQLITE_CACHE_SIZE_KIB: i64 = -65536;
/// 256 MiB memory-mapped I/O window so reads (notably serving thumbnail
/// blobs out of `thumb_blobs`) avoid per-page `read()` syscalls.
const SQLITE_MMAP_SIZE_BYTES: i64 = 256 * 1024 * 1024;
/// Keep temp B-trees (ORDER BY / GROUP BY / the cache-stats aggregation)
/// in memory rather than spilling to disk.
const SQLITE_TEMP_STORE_MEMORY: i64 = 2;

#[allow(dead_code)]
fn hex_string(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

pub struct Database {
    connection: Connection,
    path: Option<PathBuf>,
    notification_outbox_wake: Arc<tokio::sync::Notify>,
    notification_flush_signal: Arc<writer::NotificationFlushSignal>,
    notification_progress_interval_ms: i64,
}

pub struct NewRoot {
    pub path: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootRecord {
    pub id: i64,
    pub path: String,
    pub display_name: String,
    pub enabled: bool,
    pub created_at: i64,
    pub last_scan_at: Option<i64>,
    pub root_folder_id: Option<i64>,
    pub media_count: i64,
    /// Live reachability state. The DB layer always reports `true` (a root's
    /// storage is reachable until an API-level probe says otherwise); the API
    /// layer overlays the real-time value from `RootReachability` before the
    /// response leaves `list_roots`.
    pub reachable: bool,
}

#[derive(Debug, Clone)]
pub struct MediaPageQuery {
    pub root_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub include_descendants: bool,
    pub limit: i64,
    pub offset: Option<i64>,
    pub cursor: Option<String>,
    pub sort: String,
    pub kind: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub total_count: Option<i64>,
}

pub struct FolderUpsert {
    pub root_id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub path_hash: String,
    pub mtime: Option<i64>,
}

pub struct FileUpsert {
    pub root_id: i64,
    pub folder_id: i64,
    pub name: String,
    pub ext: String,
    pub size: i64,
    pub mtime: i64,
    pub ctime: Option<i64>,
    pub file_key: Option<String>,
}

pub struct ScanWriteBatch {
    pub folders: Vec<FolderUpsert>,
    pub files: Vec<ScanFileUpsert>,
    pub scan_generation: Option<i64>,
}

pub struct ScanFileUpsert {
    pub file: FileUpsert,
    pub media_kind: String,
    pub external_identity: Option<ExternalFileIdentity>,
}

#[derive(Debug, Clone)]
pub struct ExternalFileIdentity {
    pub volume_identity: Vec<u8>,
    pub file_identity: Vec<u8>,
    pub content_fingerprint: Option<Vec<u8>>,
    pub path: PathBuf,
}

#[allow(dead_code)]
pub struct ThumbnailStateUpsert {
    pub file_id: i64,
    pub profile: String,
    pub state: String,
    pub cache_key: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub byte_size: Option<i64>,
    pub error: Option<String>,
    pub source_fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ThumbnailTaskRequest {
    pub thumbnail: ThumbnailRecord,
    pub task_id: Option<i64>,
    #[allow(dead_code)]
    pub queued: bool,
    pub should_enqueue: bool,
}

#[derive(Debug, Clone)]
pub struct ThumbnailPriorityScopeSyncResult {
    #[allow(dead_code)]
    pub updated: usize,
    pub promoted_pending_task_ids: Vec<i64>,
}

#[derive(Debug, Clone, Copy)]
pub struct ThumbnailCacheScope {
    pub root_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub include_descendants: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailCacheRefreshMode {
    MissingOnly,
    StaleOrMissing,
    RetryFailedAndStale,
    /// Core-internal: the background transient-retry sweep's candidate set —
    /// ONLY `failed` rows classified `transient`. Unlike `RetryFailedAndStale`
    /// it deliberately excludes `missing` (and `stale`), so the 60s sweep never
    /// bulk-generates a large library's un-generated backlog behind the user's
    /// generation mode. Not client-reachable (no `from_wire_value` mapping);
    /// only `enqueue_transient_retry_for_root` uses it.
    RetryTransientFailedOnly,
}

impl ThumbnailCacheRefreshMode {
    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "missingOnly" => Some(Self::MissingOnly),
            "staleOrMissing" => Some(Self::StaleOrMissing),
            "retryFailedAndStale" => Some(Self::RetryFailedAndStale),
            // `RetryTransientFailedOnly` is intentionally unmapped: it is a
            // Core-internal sweep mode, not a client-selectable refresh mode.
            _ => None,
        }
    }

    /// Whether this mode requeues transient `failed` rows (resetting them so the
    /// worker regenerates them). Both retry modes do; the missing/stale
    /// generation modes do not.
    fn requeues_transient_failures(self) -> bool {
        matches!(
            self,
            Self::RetryFailedAndStale | Self::RetryTransientFailedOnly
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThumbnailCacheStats {
    pub cached_count: i64,
    pub missing_count: i64,
    pub stale_count: i64,
    pub failed_count: i64,
    pub pending_candidate_count: i64,
    pub total_blob_bytes: i64,
    pub active_bulk_task_count: i64,
}

#[derive(Debug, Clone)]
pub struct ThumbnailCacheSnapshot {
    pub stats: ThumbnailCacheStats,
    pub failure_details: Vec<ThumbnailFailureRecord>,
    pub failure_details_truncated: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailFailureRecord {
    pub file_id: i64,
    pub folder_id: i64,
    pub root_id: i64,
    pub relative_path: String,
    pub failure_kind: String,
    pub error: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct ThumbnailCacheEnqueueResult {
    pub accepted_count: usize,
    pub task_id_to_wake: Option<i64>,
    pub stats: ThumbnailCacheStats,
    /// In windowed (keyset) mode, the `file_id` to resume from on the next
    /// batch, or `None` once the scan reaches the end of the scope. Always
    /// `None` in the non-windowed (whole-scope) enqueue path, where the caller
    /// re-issues the same scope until nothing is accepted.
    pub next_cursor: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ThumbnailCacheClearResult {
    pub deleted_blob_count: usize,
    pub reset_thumbnail_count: usize,
}

pub const ROOT_SCAN_TASK_PRIORITY: i64 = 0;
pub const ROOT_SCAN_FOREGROUND_FAIRNESS_CONSUMED_PRIORITY: i64 = 1;
pub const THUMBNAIL_BULK_PRIORITY: i64 = -10;
pub const THUMBNAIL_BACKGROUND_PRIORITY: i64 = 0;
pub const THUMBNAIL_AHEAD_PRIORITY: i64 = 20;
pub const THUMBNAIL_VISIBLE_PRIORITY: i64 = 30;
pub const THUMBNAIL_SELECTED_PRIORITY: i64 = 40;
pub const INTERACTIVE_FOLDER_SCAN_TASK_PRIORITY: i64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailTaskPriority {
    Bulk,
    Background,
    Ahead,
    Visible,
    Selected,
}

/// What to do with a thumbnail task whose file has left the viewport scope AND
/// the client's retention grace window. Chosen by the caller from the current
/// generation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailScopeLeaverPolicy {
    /// renderedOnly / cacheOnly: cancel a pending leaver (running ones finish on
    /// their own). The user isn't looking at it and doesn't want whole-library
    /// generation, so it should stop being active work.
    Cancel,
    /// allPersist: demote a leaver to bulk priority so whole-library generation
    /// keeps making progress behind all viewport work.
    DemoteToBulk,
    /// Legacy / non-viewport callers: demote a leaver only to background
    /// priority (the pre-change behavior), never cancelling or bulk-demoting.
    DemoteToBackground,
}

impl ThumbnailTaskPriority {
    pub fn from_query_value(value: Option<&str>) -> Option<Self> {
        match value.unwrap_or("background") {
            "background" => Some(Self::Background),
            "ahead" => Some(Self::Ahead),
            "visible" => Some(Self::Visible),
            "selected" => Some(Self::Selected),
            _ => None,
        }
    }

    pub fn task_priority(self) -> i64 {
        match self {
            Self::Bulk => THUMBNAIL_BULK_PRIORITY,
            Self::Background => THUMBNAIL_BACKGROUND_PRIORITY,
            Self::Ahead => THUMBNAIL_AHEAD_PRIORITY,
            Self::Visible => THUMBNAIL_VISIBLE_PRIORITY,
            Self::Selected => THUMBNAIL_SELECTED_PRIORITY,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThumbnailSourceRecord {
    pub file_id: i64,
    pub root_id: i64,
    pub folder_id: i64,
    pub name: String,
    pub size: i64,
    pub mtime: i64,
    pub file_key: Option<String>,
    pub media_kind: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub metadata_status: Option<String>,
}

impl ThumbnailSourceRecord {
    pub fn cache_identity(&self) -> CacheIdentity<'_> {
        CacheIdentity {
            file_id: self.file_id,
            root_id: self.root_id,
            folder_id: self.folder_id,
            name: &self.name,
            size: self.size,
            mtime: self.mtime,
            file_key: self.file_key.as_deref(),
        }
    }

    pub fn source_fingerprint(&self, profile: &str) -> String {
        source_fingerprint_for(&self.cache_identity(), profile)
    }

    fn has_reliable_dimensions(&self) -> bool {
        self.metadata_status.as_deref() == Some("ready")
    }
}

#[derive(Debug)]
pub struct ScanWriteBatchResult {
    pub folder_ids: Vec<i64>,
    #[allow(dead_code)]
    pub file_ids: Vec<i64>,
}

#[derive(Debug)]
struct FileUpsertResult {
    id: i64,
    identity_changed: bool,
}

#[derive(Debug, Clone)]
struct ThumbnailCacheEntry {
    source: ThumbnailSourceRecord,
    thumbnail: Option<ThumbnailRecord>,
    has_blob: bool,
    blob_byte_size: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderRecord {
    pub id: i64,
    pub root_id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub status: String,
    /// Whether this folder has at least one active child folder. Lets the UI
    /// decide up front whether to render an expand affordance, instead of
    /// showing a chevron that vanishes once an empty child page loads. Computed
    /// with an EXISTS probe over `idx_folders_parent_status_id` (one index seek
    /// per row), so it stays cheap even for a full 500-folder page.
    pub has_children: bool,
    /// Active media in this folder and every active descendant folder. Folder
    /// pages compute all row counts in one shared recursive scope.
    pub media_count: i64,
}

const FOLDER_ANCESTOR_DEPTH_CAP: i64 = 4096;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRecord {
    pub id: i64,
    pub root_id: i64,
    pub folder_id: i64,
    pub name: String,
    pub ext: String,
    pub size: i64,
    pub mtime: i64,
    pub kind: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_ms: Option<i64>,
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_placeholder: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_placeholder_format: Option<String>,
    pub thumbnail_state: Option<String>,
    pub thumbnail_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<i64>,
    #[serde(default, skip_serializing_if = "is_default_favorite")]
    pub favorite: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tag_ids: Vec<i64>,
}

fn is_default_favorite(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailRecord {
    pub file_id: i64,
    pub profile: String,
    pub state: String,
    pub short_side_px: i64,
    pub output_format: String,
    pub cache_key: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub byte_size: Option<i64>,
    pub error: Option<String>,
    pub source_fingerprint: Option<String>,
    pub served_by: Option<String>,
    pub updated_at: Option<i64>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ThumbBlobRecord {
    pub file_id: i64,
    pub profile: String,
    pub data: Vec<u8>,
    pub width: i64,
    pub height: i64,
    pub byte_size: i64,
    pub output_format: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Total bytes of cached preview tiers this library keeps before evicting the
/// least recently used. Tiers are 1–3 MB each and are created for every large
/// image the user opens, so an unbounded cache would grow the library file
/// without limit. Regenerating an evicted tier costs one decode.
pub const PREVIEW_BLOB_CACHE_LIMIT_BYTES: i64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PreviewBlobRecord {
    pub file_id: i64,
    pub profile: String,
    pub data: Vec<u8>,
    pub width: i64,
    pub height: i64,
    pub byte_size: i64,
    pub output_format: String,
    /// Source file size at generation time; the tier is stale when it differs.
    pub source_size: i64,
    /// Source file mtime at generation time; the tier is stale when it differs.
    pub source_mtime: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct PreviewFailureRecord {
    pub file_id: i64,
    pub profile: String,
    pub failure_kind: String,
    pub error: String,
    pub source_size: i64,
    pub source_mtime: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    pub id: i64,
    pub kind: String,
    pub priority: i64,
    pub status: String,
    pub root_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub file_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub items_seen: i64,
    pub items_total: Option<i64>,
    pub folders_seen: i64,
    pub media_files_seen: i64,
    pub skipped_files: i64,
    #[serde(skip_serializing)]
    pub thumbnail_source_fingerprint: Option<String>,
    #[serde(skip_serializing)]
    pub attempt_generation: i64,
    #[allow(dead_code)]
    #[serde(skip_serializing)]
    pub root_uuid: Option<Vec<u8>>,
    #[allow(dead_code)]
    #[serde(skip_serializing)]
    pub local_binding_uuid: Option<Vec<u8>>,
    #[allow(dead_code)]
    #[serde(skip_serializing)]
    pub binding_generation: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationOrigin {
    User,
    Background,
    Recovery,
}

impl NotificationOrigin {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Background => "background",
            Self::Recovery => "recovery",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TaskNotificationDescriptor {
    kind: &'static str,
    title: &'static str,
    presentation: &'static str,
    cancellable: bool,
    retryable: bool,
}

fn task_notification_descriptor(kind: &str) -> anyhow::Result<Option<TaskNotificationDescriptor>> {
    Ok(match kind {
        "root_scan" => Some(TaskNotificationDescriptor {
            kind: "root_scan",
            title: "Scan library",
            presentation: "important",
            cancellable: true,
            retryable: true,
        }),
        "interactive_folder_scan" => Some(TaskNotificationDescriptor {
            kind: "interactive_folder_scan",
            title: "Scan folder",
            presentation: "important",
            cancellable: true,
            retryable: true,
        }),
        "root_purge" => Some(TaskNotificationDescriptor {
            kind: "root_purge",
            title: "Remove library",
            presentation: "important",
            cancellable: true,
            retryable: true,
        }),
        "thumbnail" => None,
        other => {
            return Err(anyhow::anyhow!(
                "task kind {other} is missing a notification descriptor"
            ))
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskScanProgress {
    pub items_seen: i64,
    pub items_total: Option<i64>,
    pub folders_seen: i64,
    pub media_files_seen: i64,
    pub skipped_files: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagRecord {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMetadataRecord {
    pub file_id: i64,
    pub rating: Option<i64>,
    pub favorite: bool,
    pub note: Option<String>,
    pub tag_ids: Vec<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct UserMetadataPatch {
    /// Outer `Option` indicates whether the caller provided the field; inner
    /// `Option` is the value (None clears the column).
    pub rating: Option<Option<i64>>,
    pub favorite: Option<bool>,
    pub note: Option<Option<String>>,
}

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub root_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub include_descendants: bool,
    pub kind: Option<String>,
    pub min_rating: Option<i64>,
    pub favorite: Option<bool>,
    pub tag_ids: Vec<i64>,
    pub sort: String,
    pub limit: i64,
    pub offset: Option<i64>,
    pub cursor: Option<String>,
}

#[derive(Debug)]
pub enum TagError {
    Duplicate,
    InvalidName,
    InvalidColor,
    UnknownTagId(i64),
    Other(anyhow::Error),
}

impl std::fmt::Display for TagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TagError::Duplicate => write!(f, "tag name already exists"),
            TagError::InvalidName => write!(f, "tag name is invalid"),
            TagError::InvalidColor => write!(f, "tag color is invalid"),
            TagError::UnknownTagId(id) => write!(f, "tag not found: {id}"),
            TagError::Other(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for TagError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TagError::Other(error) => error.source(),
            _ => None,
        }
    }
}

impl From<anyhow::Error> for TagError {
    fn from(error: anyhow::Error) -> Self {
        TagError::Other(error)
    }
}

impl From<rusqlite::Error> for TagError {
    fn from(error: rusqlite::Error) -> Self {
        TagError::Other(anyhow::Error::from(error))
    }
}

/// Decision returned by [`Database::request_thumbnail_task_in_transaction`] so
/// the same logic can run standalone or batched inside one transaction.
/// `terminal_thumbnail` is `Some` only on the no-op terminal path, letting the
/// non-batched wrapper reproduce its exact original return without a second
/// query.
struct ThumbnailTaskRequestOutcome {
    task_id: Option<i64>,
    queued: bool,
    accepted: bool,
    should_enqueue: bool,
    terminal_thumbnail: Option<ThumbnailRecord>,
}

/// Outcome of [`Database::mark_root_for_purge`].
#[derive(Debug)]
#[allow(dead_code)] // consumed by remove_root route in later tasks
pub enum MarkRootForPurge {
    NotFound,
    AlreadyPending,
    Marked {
        task_id: i64,
        attempt_generation: i64,
    },
}

/// Number of rows deleted per phase in [`Database::purge_root_next_batch`].
#[allow(dead_code)] // consumed by the root_purge worker branch in a later task
pub const PURGE_BATCH_SIZE: usize = 5000;

/// Outcome of one [`Database::purge_root_next_batch`] call.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // consumed by the root_purge worker branch in a later task
pub struct PurgeBatchOutcome {
    pub rows_deleted: usize,
    pub done: bool,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let connection = Connection::open(&path)?;
        Self::from_connection(connection, Some(path))
    }

    #[cfg(test)]
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let connection = Connection::open_in_memory()?;
        Self::from_connection(connection, None)
    }

    fn from_connection(connection: Connection, path: Option<PathBuf>) -> anyhow::Result<Self> {
        Self::from_connection_with_notification_state(
            connection,
            path,
            Arc::new(tokio::sync::Notify::new()),
            Arc::new(writer::NotificationFlushSignal::new()),
            notification_outbox::DEFAULT_PROGRESS_EMIT_INTERVAL_MS,
        )
    }

    fn from_connection_with_notification_state(
        connection: Connection,
        path: Option<PathBuf>,
        notification_outbox_wake: Arc<tokio::sync::Notify>,
        notification_flush_signal: Arc<writer::NotificationFlushSignal>,
        notification_progress_interval_ms: i64,
    ) -> anyhow::Result<Self> {
        // Set busy_timeout BEFORE any other operation so the busy handler
        // is registered for every subsequent SQL statement, including the
        // foreign_keys PRAGMA, migration writes, and transaction
        // escalation.
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        if path.is_some() {
            configure_file_database_wal(&connection)?;
        }
        connection.pragma_update(None, "foreign_keys", "ON")?;
        register_thumbnail_cache_sql_functions(&connection)?;
        Ok(Self {
            connection,
            path,
            notification_outbox_wake,
            notification_flush_signal,
            notification_progress_interval_ms,
        })
    }

    pub fn reopen(&self) -> anyhow::Result<Option<Self>> {
        let Some(path) = &self.path else {
            return Ok(None);
        };
        let connection = Connection::open(path)?;
        Self::from_connection_with_notification_state(
            connection,
            Some(path.clone()),
            Arc::clone(&self.notification_outbox_wake),
            Arc::clone(&self.notification_flush_signal),
            self.notification_progress_interval_ms,
        )
        .map(Some)
    }

    /// Shared wake signal for consumers waiting for committed Outbox rows.
    /// Reopened connections for the same Core instance intentionally share it.
    pub fn notification_outbox_wake(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.notification_outbox_wake)
    }

    fn notification_flush_signal(&self) -> Arc<writer::NotificationFlushSignal> {
        Arc::clone(&self.notification_flush_signal)
    }

    pub(crate) fn notification_operation_committed(&self, appended: bool) {
        if appended {
            self.notification_outbox_wake.notify_waiters();
        }
        self.notification_flush_signal.notify();
    }

    #[cfg(test)]
    pub(crate) fn set_notification_progress_interval_for_test(&mut self, interval_ms: i64) {
        assert!(interval_ms > 0);
        self.notification_progress_interval_ms = interval_ms;
    }

    fn publish_task_notification_in_transaction(
        &self,
        transaction: &Transaction<'_>,
        task_id: i64,
        origin_override: Option<NotificationOrigin>,
        mode: notification_outbox::PublishMode,
    ) -> anyhow::Result<bool> {
        self.publish_task_notification_in_transaction_at_millis(
            transaction,
            task_id,
            origin_override,
            mode,
            unix_timestamp_millis(),
        )
    }

    fn publish_task_notification_in_transaction_at_millis(
        &self,
        transaction: &Transaction<'_>,
        task_id: i64,
        origin_override: Option<NotificationOrigin>,
        mode: notification_outbox::PublishMode,
        updated_at_ms: i64,
    ) -> anyhow::Result<bool> {
        if updated_at_ms < 0 {
            return Err(anyhow::anyhow!(
                "notification timestamp must be non-negative"
            ));
        }
        let task = task_record_in_transaction(transaction, task_id)?;
        let Some(descriptor) = task_notification_descriptor(&task.kind)? else {
            return Ok(false);
        };
        let origin = origin_override
            .map(NotificationOrigin::as_str)
            .map(str::to_owned)
            .or_else(|| task_notification_origin_in_transaction(transaction, task_id))
            .unwrap_or_else(|| NotificationOrigin::Background.as_str().to_owned());
        let terminal = matches!(task.status.as_str(), "succeeded" | "failed" | "cancelled");
        let started_at = task.created_at.saturating_mul(1_000);
        let updated_at = updated_at_ms;
        let presentation =
            if task.status == "succeeded" && matches!(origin.as_str(), "background" | "recovery") {
                "history_only"
            } else {
                descriptor.presentation
            };
        let operation = notification_outbox::NotificationOperation {
            operation_id: format!("task:{}", task.id),
            revision: 0,
            kind: descriptor.kind.to_owned(),
            category: "task".to_owned(),
            origin,
            presentation: presentation.to_owned(),
            status: task.status.clone(),
            phase: Some(
                match task.status.as_str() {
                    "pending" => "queued",
                    "running" => "working",
                    "succeeded" => "completed",
                    "failed" => "failed",
                    "cancelled" => "cancelled",
                    _ => "unknown",
                }
                .to_owned(),
            ),
            title: descriptor.title.to_owned(),
            summary: task_notification_summary_in_transaction(transaction, &task)?,
            task_id: Some(task.id),
            root_id: task.root_id,
            library_uuid: None,
            root_uuid: None,
            managed_root_action: None,
            folder_id: task.folder_id,
            file_id: None,
            current_value: task.items_seen,
            total_value: task.items_total,
            unit: Some("items".to_owned()),
            error_code: task.error.as_ref().map(|_| {
                if task.status == "cancelled" {
                    "cancelled".to_owned()
                } else {
                    "failed".to_owned()
                }
            }),
            error_detail: task.error.clone(),
            cancellable: descriptor.cancellable && !terminal,
            retryable: task_retryable_in_transaction(transaction, &task)?,
            attempt: task.attempt_generation.saturating_add(1),
            occurrence_count: 1,
            started_at,
            updated_at,
            completed_at: terminal.then_some(updated_at),
        };
        let mut operation = operation;
        self.publish_notification_operation_in_transaction(transaction, &mut operation, mode)
    }

    fn publish_task_notification_after_commit(&self, appended: bool) {
        self.notification_operation_committed(appended);
    }

    #[cfg(test)]
    pub(crate) fn notification_heads_for_test(
        &self,
    ) -> anyhow::Result<Vec<notification_outbox::NotificationOperation>> {
        let mut statement = self.connection.prepare(
            "SELECT payload_json FROM notification_operation_heads ORDER BY operation_id ASC",
        )?;
        let payloads = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        payloads
            .into_iter()
            .map(|payload| serde_json::from_str(&payload).map_err(Into::into))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn execute_batch_for_test(&self, sql: &str) -> anyhow::Result<()> {
        self.connection.execute_batch(sql)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn file_row_exists_for_test(&self, file_id: i64) -> anyhow::Result<bool> {
        self.file_exists(file_id)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub(crate) fn set_write_durability(&self, durability: WriteDurability) -> anyhow::Result<()> {
        let synchronous = match durability {
            WriteDurability::Derived => SQLITE_SYNCHRONOUS_NORMAL,
            WriteDurability::Authoritative => SQLITE_SYNCHRONOUS_FULL,
        };
        // SQLite rejects changing synchronous from inside a transaction. The
        // single writer actor is therefore the only owner of this transition.
        self.connection
            .pragma_update(None, "synchronous", synchronous)?;
        Ok(())
    }

    /// Bind the managed Host identity to this Catalog. Safe on every open; a
    /// mismatch means the Catalog was copied under the wrong Library marker.
    pub fn initialize_sync_identity(
        &self,
        library_uuid: uuid::Uuid,
        root_uuid: uuid::Uuid,
        device_uuid: uuid::Uuid,
    ) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let root_count: i64 =
            transaction.query_row("SELECT COUNT(*) FROM roots", [], |row| row.get(0))?;
        let descriptor_count: i64 =
            transaction.query_row("SELECT COUNT(*) FROM sync_root_descriptors", [], |row| {
                row.get(0)
            })?;
        if root_count == 0 && descriptor_count == 0 {
            anyhow::bail!("managed Catalog must contain at least one root");
        }
        // Preserve legacy single-root identity while allowing already-enrolled
        // managed-root-v2 Catalogs to retain one stable UUID per selected root.
        if root_count == 1 {
            transaction.execute(
                "UPDATE roots SET root_uuid=?1 WHERE id=(SELECT id FROM roots LIMIT 1) AND root_folder_uuid IS NULL AND root_uuid != ?1",
                [root_uuid.as_bytes().as_slice()],
            )?;
        }
        let now = unix_timestamp_millis();
        transaction.execute(
            "INSERT OR IGNORE INTO sync_local_state(singleton,library_uuid,device_uuid,catalog_stream_uuid,next_actor_seq,next_local_mutation_generation,hlc_wall_ms,hlc_logical,capture_enabled,compatibility_state,updated_at) VALUES (1,?1,?2,?3,1,1,0,0,0,'compatible',?4)",
            rusqlite::params![library_uuid.as_bytes().as_slice(), device_uuid.as_bytes().as_slice(), uuid::Uuid::now_v7().as_bytes().as_slice(), now],
        )?;
        let stored: (Vec<u8>, Vec<u8>) = transaction.query_row(
            "SELECT library_uuid,device_uuid FROM sync_local_state WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if stored.0 != library_uuid.as_bytes() || stored.1 != device_uuid.as_bytes() {
            anyhow::bail!("Catalog sync identity does not match Host binding");
        }
        let roots = {
            let mut statement = transaction.prepare(
                "SELECT id,root_uuid,display_name,root_folder_uuid FROM roots ORDER BY id",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<Vec<u8>>>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (root_id, root_identity, display_name, linked_folder_uuid) in roots {
            let Some(sentinel_uuid) = linked_folder_uuid else {
                continue;
            };
            let requested_name = if display_name.is_empty() {
                "untitled".to_owned()
            } else {
                display_name
            };
            let materialized_name =
                crate::sync::reducer::canonicalize_materialized_component(&requested_name, None);
            let fingerprint = crate::sync::codec::sha256(&crate::sync::codec::canonical_bytes(&(
                crate::sync::reducer::MATERIALIZED_NAME_VERSION,
                &root_identity,
                &sentinel_uuid,
                &materialized_name,
            ))?);
            transaction.execute(
                "INSERT OR IGNORE INTO sync_root_descriptors(root_uuid,root_folder_uuid,library_root_uuid,requested_name,materialized_name,materialization_fingerprint,lifecycle,logical_revision,updated_at) VALUES (?1,?2,?3,?4,?5,?6,'active',0,?7)",
                rusqlite::params![root_identity, sentinel_uuid, root_uuid.as_bytes().as_slice(), requested_name, materialized_name, fingerprint.as_slice(), now],
            )?;
            transaction.execute(
                "UPDATE roots SET root_folder_uuid=?1 WHERE id=?2 AND root_folder_uuid IS NULL",
                rusqlite::params![sentinel_uuid, root_id],
            )?;
        }
        // Seed immutable location truth once. The empty-name sentinel keeps its
        // local shape; its portable location is the descriptor name under the
        // virtual Library root.
        transaction.execute(
            "INSERT INTO sync_location_revisions(revision_uuid,entity_uuid,parent_uuid,name,local_mutation_generation,origin,created_at) SELECT randomblob(16),d.root_folder_uuid,d.library_root_uuid,d.requested_name,1,'bootstrap',?1 FROM sync_root_descriptors d JOIN roots r ON r.root_uuid=d.root_uuid WHERE NOT EXISTS (SELECT 1 FROM sync_location_revisions x WHERE x.entity_uuid=d.root_folder_uuid)",
            [now],
        )?;
        transaction.execute(
            "INSERT INTO sync_location_revisions(revision_uuid,entity_uuid,parent_uuid,name,local_mutation_generation,origin,created_at) SELECT randomblob(16),f.folder_uuid,p.folder_uuid,f.name,1,'bootstrap',?1 FROM folders f JOIN roots r ON r.id=f.root_id JOIN folders p ON p.id=f.parent_id WHERE NOT EXISTS (SELECT 1 FROM sync_location_revisions x WHERE x.entity_uuid=f.folder_uuid)",
            [now],
        )?;
        transaction.execute(
            "INSERT INTO sync_location_revisions(revision_uuid,entity_uuid,parent_uuid,name,local_mutation_generation,origin,created_at) SELECT randomblob(16),f.folder_uuid,COALESCE(p.folder_uuid,r.root_uuid),f.name,1,'bootstrap',?1 FROM folders f JOIN roots r ON r.id=f.root_id LEFT JOIN folders p ON p.id=f.parent_id WHERE r.root_folder_uuid IS NULL AND NOT EXISTS (SELECT 1 FROM sync_location_revisions x WHERE x.entity_uuid=f.folder_uuid)",
            [now],
        )?;
        transaction.execute(
            "INSERT INTO sync_location_revisions(revision_uuid,entity_uuid,parent_uuid,name,local_mutation_generation,origin,created_at) SELECT randomblob(16),f.asset_uuid,p.folder_uuid,f.name,1,'bootstrap',?1 FROM files f JOIN folders p ON p.id=f.folder_id WHERE NOT EXISTS (SELECT 1 FROM sync_location_revisions x WHERE x.entity_uuid=f.asset_uuid)",
            [now],
        )?;
        crate::sync::inbox::seed_bootstrap_location_heads(&transaction)?;
        transaction.execute(
            "UPDATE sync_local_state SET next_local_mutation_generation=MAX(next_local_mutation_generation,2),updated_at=?1 WHERE singleton=1",
            [now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn begin_sync_capture(
        &self,
        binding_epoch: uuid::Uuid,
        actor_epoch: uuid::Uuid,
    ) -> anyhow::Result<i64> {
        self.begin_sync_capture_inner(binding_epoch, actor_epoch, true)
    }

    /// Starts the local actor after a checkpoint bootstrap. The restored
    /// Catalog already contains remote content revisions, so seeding initial
    /// publication intents here would incorrectly try to upload media that is
    /// not present on this device yet.
    pub(crate) fn begin_restored_sync_capture(
        &self,
        binding_epoch: uuid::Uuid,
        actor_epoch: uuid::Uuid,
    ) -> anyhow::Result<i64> {
        self.begin_sync_capture_inner(binding_epoch, actor_epoch, false)
    }

    fn begin_sync_capture_inner(
        &self,
        binding_epoch: uuid::Uuid,
        actor_epoch: uuid::Uuid,
        seed_publication_intents: bool,
    ) -> anyhow::Result<i64> {
        let transaction = self.connection.unchecked_transaction()?;
        let (device, stream, generation, active_actor, active_binding, capture_enabled): (
            Vec<u8>,
            Vec<u8>,
            i64,
            Option<Vec<u8>>,
            Option<Vec<u8>>,
            i64,
        ) = transaction.query_row(
            "SELECT device_uuid,catalog_stream_uuid,next_local_mutation_generation,active_actor_epoch,active_binding_epoch,capture_enabled FROM sync_local_state WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
        )?;
        if capture_enabled != 0 {
            if active_actor.as_deref() == Some(actor_epoch.as_bytes())
                && active_binding.as_deref() == Some(binding_epoch.as_bytes())
            {
                transaction.commit()?;
                return Ok(generation);
            }
            anyhow::bail!("different sync capture is already active");
        }
        let now = unix_timestamp_millis();
        transaction.execute(
            "INSERT INTO sync_actor_epochs(actor_epoch,device_uuid,stream_uuid,stream_kind,binding_epoch,state,next_actor_seq,published_through_seq,created_at) VALUES (?1,?2,?3,'catalog',?4,'active',1,0,?5)",
            rusqlite::params![actor_epoch.as_bytes().as_slice(),device,stream,binding_epoch.as_bytes().as_slice(),now],
        )?;
        transaction.execute(
            "UPDATE sync_local_state SET active_actor_epoch=?1,active_binding_epoch=?2,next_actor_seq=1,capture_enabled=1,updated_at=?3 WHERE singleton=1",
            rusqlite::params![actor_epoch.as_bytes().as_slice(),binding_epoch.as_bytes().as_slice(),now],
        )?;
        if seed_publication_intents {
            seed_initial_media_publication_intents(&transaction)?;
        }
        crate::sync::media::inventory::bind_pending_publication_jobs(
            &transaction,
            binding_epoch,
            1,
            now,
        )?;
        transaction.commit()?;
        Ok(generation)
    }

    #[allow(dead_code)]
    pub fn unbind_sync_capture(&self, published_through_seq: i64) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let actor: Option<Vec<u8>> = transaction.query_row(
            "SELECT active_actor_epoch FROM sync_local_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let Some(actor) = actor else {
            transaction.commit()?;
            return Ok(());
        };
        let now = unix_timestamp_millis();
        transaction.execute(
            "UPDATE sync_outbox SET state='archived' WHERE state IN ('pending','sealed') AND event_uuid IN (SELECT event_uuid FROM sync_events WHERE actor_epoch=?1 AND actor_seq>?2)",
            rusqlite::params![actor,published_through_seq],
        )?;
        transaction.execute(
            "UPDATE sync_actor_epochs SET state='closed',published_through_seq=?2,closed_at=?3 WHERE actor_epoch=?1",
            rusqlite::params![actor,published_through_seq,now],
        )?;
        transaction.execute(
            "UPDATE sync_local_state SET active_actor_epoch=NULL,active_binding_epoch=NULL,capture_enabled=0,updated_at=?1 WHERE singleton=1",
            [now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn enable_fake_sync_capture_for_test(&self) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let actor = uuid::Uuid::now_v7();
        let binding = uuid::Uuid::now_v7();
        let (device, stream): (Vec<u8>, Vec<u8>) = self.connection.query_row(
            "SELECT device_uuid,catalog_stream_uuid FROM sync_local_state WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        self.connection.execute(
            "INSERT INTO sync_actor_epochs(actor_epoch,device_uuid,stream_uuid,stream_kind,binding_epoch,state,next_actor_seq,published_through_seq,created_at) VALUES (?1,?2,?3,'catalog',?4,'active',1,0,?5)",
            rusqlite::params![actor.as_bytes().as_slice(),device,stream,binding.as_bytes().as_slice(),now],
        )?;
        self.connection.execute(
            "UPDATE sync_local_state SET active_actor_epoch=?1,active_binding_epoch=?2,capture_enabled=1,updated_at=?3 WHERE singleton=1",
            rusqlite::params![actor.as_bytes().as_slice(),binding.as_bytes().as_slice(),now],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn sync_capture_counts_for_test(&self) -> anyhow::Result<(i64, i64, i64)> {
        self.connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM sync_events),(SELECT COUNT(*) FROM sync_outbox),next_actor_seq FROM sync_local_state WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(Into::into)
    }

    #[allow(dead_code)]
    pub fn create_sync_publication_intent(
        &self,
        file_id: i64,
        source_path: &str,
        byte_size: i64,
    ) -> anyhow::Result<uuid::Uuid> {
        let transaction = self.connection.unchecked_transaction()?;
        let intent = create_sync_publication_intent_in_transaction(
            &transaction,
            file_id,
            source_path,
            byte_size,
            None,
        )?;
        crate::sync::fault::hit("publication.after-intent");
        transaction.commit()?;
        Ok(intent)
    }

    #[allow(dead_code)]
    pub fn bind_fake_sync_manifest(
        &self,
        intent_uuid: uuid::Uuid,
        manifest_hash: [u8; 32],
    ) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let (asset_uuid, revision_uuid): (Vec<u8>, Vec<u8>) = transaction.query_row(
            "SELECT asset_uuid,content_revision_uuid FROM sync_publication_intents WHERE intent_uuid=?1 AND state IN ('pending-hash','pending-manifest','ready')",
            [intent_uuid.as_bytes().as_slice()],
            |row| Ok((row.get(0)?,row.get(1)?)),
        )?;
        transaction.execute(
            "UPDATE asset_content_revisions SET manifest_hash=?1 WHERE content_revision_uuid=?2",
            rusqlite::params![manifest_hash.as_slice(), revision_uuid],
        )?;
        crate::sync::fault::hit("publication.after-manifest");
        let supersedes = content_supersedes_in_transaction(&transaction, &revision_uuid)?;
        let capture = crate::sync::capture::capture_catalog_mutation(
            &transaction,
            crate::sync::capture::MutationOrigin::LocalApi,
            crate::sync::capture::entity_uuid(&asset_uuid)?,
            crate::sync::types::Operation::SetContent {
                revision_uuid: crate::sync::types::RevisionUuid(uuid::Uuid::from_slice(
                    &revision_uuid,
                )?),
                manifest_id: hex_string(&manifest_hash),
                supersedes,
                manifest_pack_id: None,
                manifest_offset: None,
                manifest_length: None,
                byte_length: None,
                source_mtime_ms: transaction
                    .query_row(
                        "SELECT source_mtime_ms FROM asset_content_revisions WHERE content_revision_uuid=?1",
                        [&revision_uuid],
                        |row| row.get(0),
                    )?,
            },
        )?;
        transaction.execute(
            "UPDATE asset_content_revisions SET event_uuid=?1 WHERE content_revision_uuid=?2",
            rusqlite::params![capture.event_uuid, revision_uuid],
        )?;
        rebuild_content_heads_in_transaction(&transaction, &asset_uuid)?;
        transaction.execute(
            "UPDATE sync_publication_intents SET event_uuid=?1,state='canonicalized',updated_at=?2 WHERE intent_uuid=?3",
            rusqlite::params![capture.event_uuid,unix_timestamp_millis(),intent_uuid.as_bytes().as_slice()],
        )?;
        crate::sync::fault::hit("publication.after-canonicalization");
        transaction.commit()?;
        Ok(())
    }

    pub fn bind_sync_manifest(
        &self,
        intent_uuid: uuid::Uuid,
        manifest_hash: [u8; 32],
        transport_generation: u64,
    ) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let (asset_uuid, revision_uuid, binding_epoch): (Vec<u8>, Vec<u8>, Option<Vec<u8>>) = transaction.query_row(
            "SELECT i.asset_uuid,i.content_revision_uuid,i.binding_epoch FROM sync_publication_intents i JOIN media_publication_jobs j ON j.intent_uuid=i.intent_uuid WHERE i.intent_uuid=?1 AND i.state IN ('pending-hash','pending-manifest','ready','failed') AND j.phase='canonicalizing' AND j.transport_generation=?2 AND (j.root_uuid IS NULL OR EXISTS (SELECT 1 FROM sync_root_local_bindings b WHERE b.root_uuid=j.root_uuid AND b.local_binding_uuid=j.local_binding_uuid AND b.binding_generation=j.binding_generation AND b.state='active'))",
            rusqlite::params![intent_uuid.as_bytes().as_slice(),transport_generation],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        )?;
        let active_binding: Option<Vec<u8>> = transaction.query_row(
            "SELECT active_binding_epoch FROM sync_local_state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        if binding_epoch.is_some() && binding_epoch != active_binding {
            anyhow::bail!("media publication binding epoch is stale");
        }
        let dependencies_ready: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM media_publication_objects WHERE intent_uuid=?1) AND NOT EXISTS(SELECT 1 FROM media_publication_objects WHERE intent_uuid=?1 AND (state<>'durable' OR receipt_generation<>?2))",
            rusqlite::params![intent_uuid.as_bytes().as_slice(),transport_generation],
            |row| row.get(0),
        )?;
        if !dependencies_ready {
            anyhow::bail!("media publication dependencies are not durable");
        }
        let linked_hash: Vec<u8> = transaction.query_row(
            "SELECT manifest_hash FROM media_content_manifest_links WHERE content_revision_uuid=?1",
            [&revision_uuid],
            |row| row.get(0),
        )?;
        if linked_hash.as_slice() != manifest_hash {
            anyhow::bail!("media manifest link does not match sealed bytes");
        }
        let (manifest_pack_hash, manifest_offset, manifest_length, byte_length):
            (Vec<u8>, i64, i64, i64) = transaction.query_row(
                "SELECT manifest_pack_hash,pack_offset,record_length,byte_length FROM media_manifests WHERE manifest_hash=?1 AND manifest_pack_hash IS NOT NULL AND pack_offset IS NOT NULL AND record_length IS NOT NULL",
                [manifest_hash.as_slice()],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            )?;
        let supersedes = content_supersedes_in_transaction(&transaction, &revision_uuid)?;
        let capture = crate::sync::capture::capture_catalog_mutation(
            &transaction,
            crate::sync::capture::MutationOrigin::LocalApi,
            crate::sync::capture::entity_uuid(&asset_uuid)?,
            crate::sync::types::Operation::SetContent {
                revision_uuid: crate::sync::types::RevisionUuid(uuid::Uuid::from_slice(&revision_uuid)?),
                manifest_id: hex_string(&manifest_hash),
                supersedes,
                manifest_pack_id: Some(hex_string(&manifest_pack_hash)),
                manifest_offset: Some(u64::try_from(manifest_offset)?),
                manifest_length: Some(u64::try_from(manifest_length)?),
                byte_length: Some(u64::try_from(byte_length)?),
                source_mtime_ms: transaction.query_row(
                    "SELECT source_mtime_ms FROM asset_content_revisions WHERE content_revision_uuid=?1",
                    [&revision_uuid],
                    |row| row.get(0),
                )?,
            },
        )?;
        transaction.execute(
            "UPDATE asset_content_revisions SET event_uuid=?1,manifest_hash=?2 WHERE content_revision_uuid=?3",
            rusqlite::params![capture.event_uuid,manifest_hash.as_slice(),revision_uuid],
        )?;
        rebuild_content_heads_in_transaction(&transaction, &asset_uuid)?;
        transaction.execute(
            "UPDATE sync_publication_intents SET event_uuid=?1,state='canonicalized',updated_at=?2 WHERE intent_uuid=?3",
            rusqlite::params![capture.event_uuid,unix_timestamp_millis(),intent_uuid.as_bytes().as_slice()],
        )?;
        crate::sync::fault::hit("publication.after-canonicalization");
        transaction.commit()?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn apply_remote_sync_events(
        &self,
        events: &[crate::sync::types::EventEnvelope],
    ) -> anyhow::Result<usize> {
        let transaction = self.connection.unchecked_transaction()?;
        match crate::sync::inbox::apply_remote_batch(&transaction, events) {
            Ok(applied) => {
                transaction.commit()?;
                Ok(applied)
            }
            Err(error)
                if error.to_string().contains("unknown synchronization schema")
                    || error.to_string().contains("actor gap")
                    || error.to_string().contains("actor fork") =>
            {
                // Blocking diagnostics/frontiers are authoritative too. Keep
                // them while never advancing beyond the bad event.
                transaction.commit()?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(test)]
    pub(crate) fn write_durability_for_test(&self) -> anyhow::Result<i64> {
        self.connection
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .map_err(Into::into)
    }

    pub fn default_thumbnail_cache_dir(&self) -> PathBuf {
        if let Some(path) = &self.path {
            return path
                .parent()
                .map(|parent| parent.join("thumbnail-cache"))
                .unwrap_or_else(|| PathBuf::from("thumbnail-cache"));
        }
        std::env::temp_dir().join("megle-thumbnail-cache")
    }

    /// Access to the underlying connection. Sibling modules such as `fsops`
    /// use this for short lookups and caller-owned `BEGIN IMMEDIATE`
    /// transactions on their dedicated handle.
    pub(crate) fn connection_for_fsops(&self) -> &Connection {
        &self.connection
    }

    pub fn apply_migrations(&self) -> anyhow::Result<()> {
        self.connection
            .execute_batch(migrations::INITIAL_MIGRATION)?;
        if !self.migration_applied(2)? {
            self.connection
                .execute_batch(migrations::TASK_PROGRESS_MIGRATION)?;
        }
        if !self.migration_applied(3)? {
            self.connection
                .execute_batch(migrations::BROWSING_INDEXES_MIGRATION)?;
        }
        if !self.migration_applied(4)? {
            let result = self
                .connection
                .execute_batch(migrations::THUMBNAIL_STATE_MIGRATION);
            self.connection.pragma_update(None, "foreign_keys", "ON")?;
            result?;
        }
        if !self.migration_applied(5)? {
            let result = self
                .connection
                .execute_batch(migrations::THUMBNAIL_SOURCE_FINGERPRINT_MIGRATION);
            self.connection.pragma_update(None, "foreign_keys", "ON")?;
            result?;
        }
        if !self.migration_applied(6)? {
            self.connection
                .execute_batch(migrations::THUMBNAIL_TASK_ATTEMPT_FINGERPRINT_MIGRATION)?;
        }
        self.ensure_task_contract_prerequisite_columns()?;
        if !self.migration_applied(7)? {
            let result = self
                .connection
                .execute_batch(migrations::TASK_STATUS_CONTRACT_MIGRATION);
            self.connection.pragma_update(None, "foreign_keys", "ON")?;
            result?;
        }
        if !self.migration_applied(8)? {
            self.apply_task_attempt_generation_migration()?;
        }
        if !self.migration_applied(9)? {
            self.apply_scan_reconciliation_migration()?;
        }
        if !self.migration_applied(10)? {
            self.connection
                .execute_batch(migrations::MEDIA_FTS_CONTENTLESS_DELETE_MIGRATION)?;
        }
        if !self.migration_applied(11)? {
            self.apply_plugins_extended_migration()?;
        }
        if !self.migration_applied(12)? {
            self.apply_preview_pipeline_refactor_migration()?;
        }
        if !self.migration_applied(13)? {
            self.apply_preview_served_by_migration()?;
        }
        if !self.migration_applied(14)? {
            self.apply_interactive_folder_scan_task_migration()?;
        }
        if !self.migration_applied(15)? {
            self.connection
                .execute_batch(migrations::ROOT_STATUS_BROWSING_INDEXES_MIGRATION)?;
        }
        if !self.migration_applied(16)? {
            self.connection
                .execute_batch(migrations::FOLDER_STATUS_BROWSING_INDEXES_MIGRATION)?;
        }
        if !self.migration_applied(17)? {
            self.connection
                .execute_batch(migrations::THUMBNAIL_TASK_INDEXES_MIGRATION)?;
        }
        if !self.migration_applied(18)? {
            // Atomicity: the ALTER TABLE + schema_migrations registration (both
            // inside THUMB_FAILURE_KIND_MIGRATION's SQL, same shape as migration
            // 12/13's registration-in-SQL-file pattern) and the failure_kind
            // backfill must commit together. If the process crashes mid-backfill,
            // this transaction rolls back: the column add and the version-18
            // registration are undone together, so `migration_applied(18)` stays
            // false and the next startup reruns this whole gate from scratch
            // (ALTER TABLE ADD COLUMN is safe to retry here because the rollback
            // guarantees the column does not already exist).
            let transaction = self.connection.unchecked_transaction()?;
            transaction.execute_batch(migrations::THUMB_FAILURE_KIND_MIGRATION)?;
            backfill_thumbnail_failure_kind_in_transaction(&transaction)?;
            transaction.commit()?;
        }
        if !self.migration_applied(19)? {
            // Atomicity: the ALTER TABLE + partial index + schema_migrations
            // registration must commit together (same pattern as the
            // migration-18 gate above). Without a transaction, a crash between
            // the ALTER and the schema_migrations INSERT leaves the column
            // added but `migration_applied(19)` still false, so the next
            // startup re-runs `ALTER TABLE ADD COLUMN` and fails with
            // "duplicate column name" — the library would never open again.
            let transaction = self.connection.unchecked_transaction()?;
            transaction.execute_batch(migrations::ROOT_PENDING_DELETE_MIGRATION)?;
            transaction.commit()?;
        }
        if !self.migration_applied(20)? {
            let result = self
                .connection
                .execute_batch(migrations::ROOT_PURGE_TASK_KIND_MIGRATION);
            self.connection.pragma_update(None, "foreign_keys", "ON")?;
            result?;
        }
        if !self.migration_applied(21)? {
            self.connection
                .execute_batch(migrations::NOTIFICATION_OUTBOX_MIGRATION)?;
        }
        if !self.migration_applied(22)? {
            self.connection
                .execute_batch(migrations::FILE_OPERATION_JOURNAL_MIGRATION)?;
        }
        if !self.migration_applied(23)? {
            self.connection
                .execute_batch(migrations::FILE_OPERATION_RECOVERY_METADATA_MIGRATION)?;
        }
        if !self.migration_applied(24)? {
            self.connection
                .execute_batch(migrations::FILE_OPERATION_OBJECT_BINDING_MIGRATION)?;
        }
        if !self.migration_applied(25)? {
            self.connection
                .execute_batch(migrations::PREVIEW_BLOBS_MIGRATION)?;
        }
        if !self.migration_applied(26)? {
            self.connection
                .execute_batch(migrations::PREVIEW_FAILURES_MIGRATION)?;
        }
        if !self.migration_applied(27)? {
            self.connection
                .execute_batch(migrations::SYNC_PROTOCOL_FOUNDATION_MIGRATION)?;
        }
        if !self.migration_applied(28)? {
            self.connection
                .execute_batch(migrations::REMOVE_CATALOG_PLUGINS_MIGRATION)?;
        }
        if !self.migration_applied(29)? {
            self.connection
                .execute_batch(migrations::MEDIA_OBJECT_STORAGE_MIGRATION)?;
        }
        if !self.migration_applied(30)? {
            self.connection
                .execute_batch(migrations::SYNC_ROOT_IDENTITY_MIGRATION)?;
        }
        if !self.migration_applied(31)? {
            self.connection
                .execute_batch(migrations::SYNC_ROOT_GENERATION_FENCE_MIGRATION)?;
        }
        if !self.migration_applied(32)? {
            self.connection
                .execute_batch(migrations::SYNC_ROOT_RECONCILIATION_MIGRATION)?;
        }
        if !self.migration_applied(33)? {
            self.connection
                .execute_batch(migrations::MEDIA_MATERIALIZATION_DESTINATION_MIGRATION)?;
        }
        self.backfill_media_fts_if_empty()?;
        self.checkpoint_wal_if_larger_than(WAL_TRUNCATE_THRESHOLD_BYTES)?;
        Ok(())
    }

    /// Size in bytes of the `-wal` sidecar for this database's file, or 0 for
    /// in-memory databases or when the WAL file is absent. Used for checkpoint
    /// diagnostics and to assert the WAL stays bounded in tests.
    pub fn wal_size_bytes(&self) -> u64 {
        let Some(path) = &self.path else {
            return 0;
        };
        let wal_path = Self::wal_path_for_db_path(path);
        std::fs::metadata(&wal_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0)
    }

    pub(crate) fn checkpoint_wal_if_larger_than(
        &self,
        threshold_bytes: u64,
    ) -> anyhow::Result<bool> {
        if self.wal_size_bytes() <= threshold_bytes {
            return Ok(false);
        }
        let busy: i64 =
            self.connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        Ok(busy == 0)
    }

    /// Run one WAL checkpoint pass. Called by the writer actor between job
    /// batches — the actor owns the ONLY write connection, so a TRUNCATE here
    /// always gets its exclusive window (no other writer contends) and reliably
    /// resets the WAL. Escalate to TRUNCATE when the WAL has grown past the
    /// threshold (reclaims the file), otherwise run a cheap PASSIVE pass.
    pub(crate) fn run_wal_checkpoint_pass(&self) -> anyhow::Result<()> {
        if self.path.is_none() {
            return Ok(());
        }
        if self.checkpoint_wal_if_larger_than(WAL_TRUNCATE_THRESHOLD_BYTES)? {
            return Ok(());
        }
        self.connection
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                row.get::<_, i64>(0)
            })?;
        Ok(())
    }

    fn wal_path_for_db_path(path: &Path) -> PathBuf {
        let mut wal_path = path.as_os_str().to_os_string();
        wal_path.push("-wal");
        PathBuf::from(wal_path)
    }

    fn migration_applied(&self, version: i64) -> anyhow::Result<bool> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
            [version],
            |row| row.get(0),
        )?;
        Ok(count != 0)
    }

    fn ensure_task_contract_prerequisite_columns(&self) -> anyhow::Result<()> {
        for (column, definition) in [
            ("folder_id", "folder_id INTEGER REFERENCES folders(id)"),
            ("items_seen", "items_seen INTEGER NOT NULL DEFAULT 0"),
            ("items_total", "items_total INTEGER"),
            ("folders_seen", "folders_seen INTEGER NOT NULL DEFAULT 0"),
            (
                "media_files_seen",
                "media_files_seen INTEGER NOT NULL DEFAULT 0",
            ),
            ("skipped_files", "skipped_files INTEGER NOT NULL DEFAULT 0"),
            (
                "thumbnail_source_fingerprint",
                "thumbnail_source_fingerprint TEXT",
            ),
            (
                "attempt_generation",
                "attempt_generation INTEGER NOT NULL DEFAULT 0",
            ),
        ] {
            if !self.table_has_column("tasks", column)? {
                self.connection
                    .execute_batch(&format!("ALTER TABLE tasks ADD COLUMN {definition}"))?;
            }
        }
        Ok(())
    }

    fn apply_scan_reconciliation_migration(&self) -> anyhow::Result<()> {
        if !self.table_has_column("roots", "active_scan_generation")? {
            self.connection
                .execute_batch(migrations::SCAN_RECONCILIATION_MIGRATION)?;
        }
        for table in ["folders", "files"] {
            if !self.table_has_column(table, "scan_seen_at")? {
                self.connection.execute_batch(&format!(
                    "ALTER TABLE {table} ADD COLUMN scan_seen_at INTEGER"
                ))?;
            }
        }
        self.connection.execute(
            r#"
            INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
            VALUES (9, 'scan_reconciliation', unixepoch())
            "#,
            [],
        )?;
        Ok(())
    }

    fn table_has_column(&self, table: &str, column: &str) -> anyhow::Result<bool> {
        let mut statement = self
            .connection
            .prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
        for candidate in columns {
            if candidate? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[cfg(test)]
    fn table_exists(&self, table: &str) -> anyhow::Result<bool> {
        let exists: i64 = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type IN ('table', 'virtual') AND name = ?1)",
            [table],
            |row| row.get(0),
        )?;
        Ok(exists != 0)
    }

    fn apply_task_attempt_generation_migration(&self) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        if !table_has_column_in_transaction(&transaction, "tasks", "attempt_generation")? {
            transaction.execute_batch(
                "ALTER TABLE tasks ADD COLUMN attempt_generation INTEGER NOT NULL DEFAULT 0",
            )?;
        }
        transaction.execute(
            r#"
            INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
            VALUES (8, 'task_attempt_generation', unixepoch())
            "#,
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn apply_plugins_extended_migration(&self) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        for (column, definition) in [
            ("description", "description TEXT"),
            ("status", "status TEXT NOT NULL DEFAULT 'registered'"),
            (
                "capabilities_json",
                "capabilities_json TEXT NOT NULL DEFAULT '[]'",
            ),
            (
                "permissions_json",
                "permissions_json TEXT NOT NULL DEFAULT '[]'",
            ),
            ("last_error", "last_error TEXT"),
        ] {
            if !table_has_column_in_transaction(&transaction, "plugins", column)? {
                transaction
                    .execute_batch(&format!("ALTER TABLE plugins ADD COLUMN {definition}"))?;
            }
        }
        transaction.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_plugins_status ON plugins(status, updated_at)",
        )?;
        transaction.execute(
            r#"
            INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
            VALUES (11, 'plugins_extended', unixepoch())
            "#,
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn apply_preview_pipeline_refactor_migration(&self) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        for (column, definition) in [
            ("preview_placeholder", "preview_placeholder BLOB"),
            (
                "preview_placeholder_format",
                "preview_placeholder_format TEXT NOT NULL DEFAULT 'image/webp'",
            ),
        ] {
            if !table_has_column_in_transaction(&transaction, "media", column)? {
                transaction.execute_batch(&format!("ALTER TABLE media ADD COLUMN {definition}"))?;
            }
        }
        transaction.execute_batch(migrations::PREVIEW_PIPELINE_REFACTOR_MIGRATION)?;
        transaction.commit()?;
        Ok(())
    }

    fn apply_preview_served_by_migration(&self) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        if !table_has_column_in_transaction(&transaction, "thumbs", "served_by")? {
            transaction.execute_batch(migrations::PREVIEW_SERVED_BY_MIGRATION)?;
        } else {
            transaction.execute(
                r#"
                INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
                VALUES (13, 'preview_served_by', unixepoch())
                "#,
                [],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn apply_interactive_folder_scan_task_migration(&self) -> anyhow::Result<()> {
        let folder_select = if self.table_has_column("tasks", "folder_id")? {
            "folder_id"
        } else {
            "NULL"
        };
        let migration = format!(
            r#"
            PRAGMA foreign_keys = OFF;

            DROP TABLE IF EXISTS tasks_new;

            BEGIN IMMEDIATE;

            CREATE TABLE tasks_new (
              id INTEGER PRIMARY KEY,
              kind TEXT NOT NULL CHECK(kind IN ('root_scan', 'interactive_folder_scan', 'thumbnail')),
              priority INTEGER NOT NULL,
              status TEXT NOT NULL CHECK(status IN ('pending', 'running', 'succeeded', 'failed', 'cancelled')),
              root_id INTEGER REFERENCES roots(id) ON DELETE SET NULL,
              folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL,
              file_id INTEGER REFERENCES files(id) ON DELETE SET NULL,
              created_at INTEGER NOT NULL,
              updated_at INTEGER NOT NULL,
              items_seen INTEGER NOT NULL DEFAULT 0,
              items_total INTEGER,
              folders_seen INTEGER NOT NULL DEFAULT 0,
              media_files_seen INTEGER NOT NULL DEFAULT 0,
              skipped_files INTEGER NOT NULL DEFAULT 0,
              thumbnail_source_fingerprint TEXT,
              attempt_generation INTEGER NOT NULL DEFAULT 0,
              error TEXT
            );

            INSERT INTO tasks_new(
              id,
              kind,
              priority,
              status,
              root_id,
              folder_id,
              file_id,
              created_at,
              updated_at,
              items_seen,
              items_total,
              folders_seen,
              media_files_seen,
              skipped_files,
              thumbnail_source_fingerprint,
              attempt_generation,
              error
            )
            SELECT
              id,
              CASE
                WHEN kind IN ('root_scan', 'interactive_folder_scan', 'thumbnail') THEN kind
                ELSE 'root_scan'
              END,
              priority,
              CASE
                WHEN status IN ('pending', 'running', 'succeeded', 'failed', 'cancelled') THEN status
                ELSE 'failed'
              END,
              root_id,
              {folder_select},
              file_id,
              created_at,
              updated_at,
              items_seen,
              items_total,
              folders_seen,
              media_files_seen,
              skipped_files,
              thumbnail_source_fingerprint,
              attempt_generation,
              error
            FROM tasks;

            DROP TABLE tasks;
            ALTER TABLE tasks_new RENAME TO tasks;

            CREATE INDEX IF NOT EXISTS idx_tasks_status_priority
            ON tasks(status, priority, created_at);

            CREATE TEMP TABLE interactive_folder_scan_task_fk_check(count INTEGER CHECK(count = 0));
            INSERT INTO interactive_folder_scan_task_fk_check
            SELECT COUNT(*) FROM pragma_foreign_key_check;
            DROP TABLE interactive_folder_scan_task_fk_check;

            INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
            VALUES (14, 'interactive_folder_scan_task_contract', unixepoch());

            COMMIT;

            PRAGMA foreign_keys = ON;
            "#,
            folder_select = folder_select,
        );
        let result = self.connection.execute_batch(&migration);
        self.connection.pragma_update(None, "foreign_keys", "ON")?;
        result?;
        Ok(())
    }

    pub fn list_roots(&self) -> anyhow::Result<Vec<RootRecord>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT roots.id, roots.path, roots.display_name, roots.enabled, roots.created_at,
                   roots.last_scan_at,
                   root_folder.id AS root_folder_id,
                   (
                       SELECT COUNT(*)
                       FROM files
                       WHERE files.root_id = roots.id AND files.status = 'active'
                   ) AS media_count
            FROM roots
            LEFT JOIN folders root_folder
              ON root_folder.root_id = roots.id
             AND root_folder.parent_id IS NULL
             AND root_folder.name = ''
            WHERE roots.enabled = 1 AND roots.pending_delete_at IS NULL
            ORDER BY roots.id ASC
            "#,
        )?;
        let rows = statement.query_map([], root_from_row)?;

        let mut roots = Vec::new();
        for row in rows {
            roots.push(row?);
        }
        Ok(roots)
    }

    pub fn get_root(&self, root_id: i64) -> anyhow::Result<Option<RootRecord>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT roots.id, roots.path, roots.display_name, roots.enabled, roots.created_at,
                   roots.last_scan_at,
                   root_folder.id AS root_folder_id,
                   (
                       SELECT COUNT(*)
                       FROM files
                       WHERE files.root_id = roots.id AND files.status = 'active'
                   ) AS media_count
            FROM roots
            LEFT JOIN folders root_folder
              ON root_folder.root_id = roots.id
             AND root_folder.parent_id IS NULL
             AND root_folder.name = ''
            WHERE roots.id = ?1 AND roots.pending_delete_at IS NULL
            "#,
        )?;
        let mut rows = statement.query([root_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(root_from_row(row)?))
    }

    pub fn add_root(&self, root: NewRoot) -> anyhow::Result<i64> {
        let now = unix_timestamp();
        let transaction = self.connection.unchecked_transaction()?;
        let existing = {
            let mut statement =
                transaction.prepare("SELECT id, enabled FROM roots WHERE path = ?1")?;
            let mut rows = statement.query([&root.path])?;
            rows.next()?
                .map(|row| {
                    Ok::<(i64, bool), rusqlite::Error>((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)? != 0,
                    ))
                })
                .transpose()?
        };
        let id = if let Some((id, enabled)) = existing {
            // Re-enabling a previously-disabled root does NOT wipe its
            // folders/files here. `DELETE FROM files WHERE root_id` is O(files)
            // multiplied by the files table's many indexes; on a 600k-file root
            // that single statement ran inside the one DB writer for minutes,
            // freezing every other write (further add-root, viewport
            // priority-scope, thumbnail publish) past the client request
            // timeout. The root scan the caller enqueues right after reconciles
            // stale rows itself — `reconcile_root_scan_completion` upserts the
            // files it re-sees and marks any it does not as `missing` — so the
            // wipe is redundant. Dropping it keeps add_root O(1) and preserves
            // user_metadata/tags/thumbs across a disable -> re-enable cycle.
            transaction.execute(
                r#"
                UPDATE roots
                SET display_name = ?1,
                    enabled = 1,
                    last_scan_at = CASE WHEN ?2 = 0 THEN NULL ELSE last_scan_at END
                WHERE id = ?3
                "#,
                (&root.display_name, enabled as i64, id),
            )?;
            id
        } else {
            transaction.execute(
                r#"
                INSERT INTO roots(path, display_name, enabled, created_at)
                VALUES (?1, ?2, 1, ?3)
                "#,
                (&root.path, &root.display_name, now),
            )?;
            transaction.last_insert_rowid()
        };
        transaction.commit()?;
        Ok(id)
    }

    /// Whether the root at `path` exists AND is currently being purged
    /// (`pending_delete_at` set). Used to reject re-adding a path mid-removal.
    pub fn root_pending_delete_by_path(&self, path: &str) -> anyhow::Result<bool> {
        let pending: Option<Option<i64>> = self
            .connection
            .query_row(
                "SELECT pending_delete_at FROM roots WHERE path = ?1",
                [path],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?;
        Ok(matches!(pending, Some(Some(_))))
    }

    #[allow(dead_code)] // soft-delete path retired by remove_root (Task 7); still exercised by
                        // recovery tests as a test helper for the enabled=0 state that
                        // mark_root_for_purge still produces transiently (kept intentionally,
                        // see hd-task-10 scope note).
    pub fn disable_root(&self, root_id: i64) -> anyhow::Result<bool> {
        let transaction = self.connection.unchecked_transaction()?;
        let updated = transaction.execute(
            "UPDATE roots SET enabled = 0 WHERE id = ?1 AND enabled = 1",
            [root_id],
        )?;
        let mut appended = false;
        if updated != 0 {
            appended |= self.cancel_root_scan_tasks_in_transaction(&transaction, root_id)?;
            cleanup_inactive_thumbnail_artifacts_in_transaction(&transaction)?;
        }
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        Ok(updated != 0)
    }

    #[cfg(test)]
    pub fn create_root_scan_task(&self, root_id: i64) -> anyhow::Result<i64> {
        self.create_root_scan_task_with_origin(root_id, NotificationOrigin::Background)
    }

    pub fn create_root_scan_task_with_origin(
        &self,
        root_id: i64,
        origin: NotificationOrigin,
    ) -> anyhow::Result<i64> {
        self.create_root_scan_task_with_origin_for_root_token(root_id, None, origin)
    }

    pub(crate) fn create_root_scan_task_with_origin_for_root_token(
        &self,
        root_id: i64,
        token: Option<&SyncRootBindingToken>,
        origin: NotificationOrigin,
    ) -> anyhow::Result<i64> {
        let root = self
            .get_root(root_id)?
            .ok_or_else(|| anyhow::anyhow!("root not found: {root_id}"))?;
        if !root.enabled {
            return Err(anyhow::anyhow!("root is disabled: {root_id}"));
        }
        let now = unix_timestamp();
        let has_folder_column = self.table_has_column("tasks", "folder_id")?;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        if has_folder_column {
            transaction.execute(
                r#"
                INSERT INTO tasks(
                    kind, priority, status, root_id, folder_id, file_id, created_at, updated_at,
                    root_uuid, local_binding_uuid, binding_generation, error
                )
                VALUES ('root_scan', ?1, 'pending', ?2, NULL, NULL, ?3, ?3, ?4, ?5, ?6, NULL)
                "#,
                rusqlite::params![
                    ROOT_SCAN_TASK_PRIORITY,
                    root_id,
                    now,
                    token.map(|token| token.root_uuid.as_slice()),
                    token.map(|token| token.local_binding_uuid.as_slice()),
                    token
                        .map(|token| i64::try_from(token.binding_generation))
                        .transpose()?,
                ],
            )?;
        } else {
            transaction.execute(
                r#"
                INSERT INTO tasks(kind, priority, status, root_id, file_id, created_at, updated_at, error)
                VALUES ('root_scan', ?1, 'pending', ?2, NULL, ?3, ?3, NULL)
                "#,
                (ROOT_SCAN_TASK_PRIORITY, root_id, now),
            )?;
        }
        let task_id = transaction.last_insert_rowid();
        let appended = self.publish_task_notification_in_transaction(
            &transaction,
            task_id,
            Some(origin),
            notification_outbox::PublishMode::Phase,
        )?;
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        Ok(task_id)
    }

    /// Begin a hard delete of `root_id`: mark it `pending_delete_at`, cancel its
    /// scan tasks, and enqueue one `root_purge` task — all in one small
    /// transaction. Deletes NO file/folder rows (the background purge does).
    #[allow(dead_code)] // consumed by remove_root route in later tasks
    pub fn mark_root_for_purge(&self, root_id: i64) -> anyhow::Result<MarkRootForPurge> {
        self.mark_root_for_purge_with_origin(root_id, NotificationOrigin::Background)
    }

    pub fn mark_root_for_purge_with_origin(
        &self,
        root_id: i64,
        origin: NotificationOrigin,
    ) -> anyhow::Result<MarkRootForPurge> {
        let now = unix_timestamp();
        let transaction = self.connection.unchecked_transaction()?;
        let existing: Option<Option<i64>> = transaction
            .query_row(
                "SELECT pending_delete_at FROM roots WHERE id = ?1",
                [root_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?;
        let Some(pending_delete_at) = existing else {
            return Ok(MarkRootForPurge::NotFound);
        };
        if pending_delete_at.is_some() {
            return Ok(MarkRootForPurge::AlreadyPending);
        }
        transaction.execute(
            "UPDATE roots SET pending_delete_at = ?1, enabled = 0 WHERE id = ?2",
            (now, root_id),
        )?;
        let mut appended = false;
        appended |= self.cancel_root_scan_tasks_in_transaction(&transaction, root_id)?;
        transaction.execute(
            r#"
            INSERT INTO tasks(kind, priority, status, root_id, file_id, created_at, updated_at, error)
            VALUES ('root_purge', ?1, 'pending', ?2, NULL, ?3, ?3, NULL)
            "#,
            (ROOT_SCAN_TASK_PRIORITY, root_id, now),
        )?;
        let task_id = transaction.last_insert_rowid();
        let attempt_generation: i64 = transaction
            .query_row(
                "SELECT attempt_generation FROM tasks WHERE id = ?1",
                [task_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        appended |= self.publish_task_notification_in_transaction(
            &transaction,
            task_id,
            Some(origin),
            notification_outbox::PublishMode::Phase,
        )?;
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        Ok(MarkRootForPurge::Marked {
            task_id,
            attempt_generation,
        })
    }

    /// All roots currently mid-purge (used by startup recovery). Uses the
    /// partial index `idx_roots_pending_delete`.
    #[cfg(test)]
    pub fn roots_pending_delete_ids(&self) -> anyhow::Result<Vec<i64>> {
        let mut stmt = self
            .connection
            .prepare("SELECT id FROM roots WHERE pending_delete_at IS NOT NULL ORDER BY id ASC")?;
        let ids = stmt
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }

    /// Ensure every `pending_delete_at` root has a pending/running `root_purge`
    /// task. Called at startup recovery so an interrupted purge resumes.
    /// Returns the number of purge tasks created. Idempotent.
    pub fn ensure_pending_delete_purge_tasks(&self) -> anyhow::Result<usize> {
        let now = unix_timestamp();
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let mut created = 0;
        let mut appended = false;
        let mut root_ids = Vec::new();
        {
            let mut statement = transaction.prepare(
                "SELECT id FROM roots WHERE pending_delete_at IS NOT NULL ORDER BY id ASC",
            )?;
            for row in statement.query_map([], |row| row.get::<_, i64>(0))? {
                root_ids.push(row?);
            }
        }
        for root_id in root_ids {
            let existing_task: Option<(i64, String)> = transaction
                .query_row(
                    "SELECT id, status FROM tasks WHERE root_id=?1 AND kind='root_purge' ORDER BY id ASC LIMIT 1",
                    [root_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((task_id, status)) = existing_task {
                if status != "pending" && status != "running" {
                    transaction.execute(
                        r#"UPDATE tasks
                           SET status = 'pending', updated_at = ?1,
                               attempt_generation = attempt_generation + 1,
                               error = NULL
                           WHERE id = ?2"#,
                        (now, task_id),
                    )?;
                    appended |= self.publish_task_notification_in_transaction(
                        &transaction,
                        task_id,
                        task_notification_recovery_origin_override(&transaction, task_id),
                        notification_outbox::PublishMode::Phase,
                    )?;
                }
            } else if let Some(operation) =
                root_purge_notification_head_in_transaction(&transaction, root_id)?
            {
                let task_id = operation
                    .task_id
                    .ok_or_else(|| anyhow::anyhow!("root purge notification is missing task id"))?;
                transaction.execute(
                    r#"INSERT INTO tasks(
                         id, kind, priority, status, root_id, folder_id, file_id,
                         created_at, updated_at, items_seen, items_total, folders_seen,
                         media_files_seen, skipped_files, attempt_generation, error
                       ) VALUES (?1, 'root_purge', ?2, 'pending', ?3, NULL, NULL,
                                 ?4, ?5, ?6, ?7, 0, 0, 0, ?8, NULL)"#,
                    (
                        task_id,
                        ROOT_SCAN_TASK_PRIORITY,
                        root_id,
                        operation.started_at.div_euclid(1_000),
                        operation.updated_at.div_euclid(1_000),
                        operation.current_value,
                        operation.total_value,
                        operation.attempt.saturating_sub(1),
                    ),
                )?;
                appended |= self.publish_task_notification_in_transaction(
                    &transaction,
                    task_id,
                    None,
                    notification_outbox::PublishMode::Phase,
                )?;
                created += 1;
            } else {
                // A deleted terminal task may still own a historical operation head.
                // Never reuse that task ID, because operation IDs are task-derived.
                let mut task_id: i64 = transaction.query_row(
                    "SELECT COALESCE(MAX(id), 0) + 1 FROM tasks",
                    [],
                    |row| row.get(0),
                )?;
                while transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM notification_operation_heads WHERE operation_id = ?1)",
                    [format!("task:{task_id}")],
                    |row| row.get::<_, i64>(0),
                )? != 0
                {
                    task_id = task_id
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("task id overflow during recovery"))?;
                }
                transaction.execute(
                    r#"INSERT INTO tasks(id, kind, priority, status, root_id, file_id, created_at, updated_at, error)
                       VALUES (?1, 'root_purge', ?2, 'pending', ?3, NULL, ?4, ?4, NULL)"#,
                    (task_id, ROOT_SCAN_TASK_PRIORITY, root_id, now),
                )?;
                debug_assert_eq!(task_id, transaction.last_insert_rowid());
                appended |= self.publish_task_notification_in_transaction(
                    &transaction,
                    task_id,
                    Some(NotificationOrigin::Recovery),
                    notification_outbox::PublishMode::Phase,
                )?;
                created += 1;
            }
        }
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        Ok(created)
    }

    /// Delete ONE bounded batch of a root's rows, leaf-to-root, resumable.
    /// Phases (first non-empty runs this call): scan/interactive tasks ->
    /// thumbnail tasks (by file) -> files (cascade) -> folders -> the roots row.
    /// Idempotent: keyed by root_id, safe to re-run after an interruption.
    /// `file_operations` is left (its refs go NULL) as audit history.
    #[allow(dead_code)] // consumed by the root_purge worker branch in a later task
    pub fn purge_root_next_batch(
        &self,
        root_id: i64,
        batch_size: usize,
    ) -> anyhow::Result<PurgeBatchOutcome> {
        self.purge_root_next_batch_internal(root_id, None, None, batch_size)
    }

    pub fn purge_root_next_batch_for_task(
        &self,
        root_id: i64,
        task_id: i64,
        attempt_generation: i64,
        batch_size: usize,
    ) -> anyhow::Result<PurgeBatchOutcome> {
        self.purge_root_next_batch_internal(
            root_id,
            Some(task_id),
            Some(attempt_generation),
            batch_size,
        )
    }

    fn purge_root_next_batch_internal(
        &self,
        root_id: i64,
        task_id: Option<i64>,
        attempt_generation: Option<i64>,
        batch_size: usize,
    ) -> anyhow::Result<PurgeBatchOutcome> {
        let batch = batch_size.max(1) as i64;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let prior_items_seen = match (task_id, attempt_generation) {
            (Some(task_id), Some(attempt_generation)) => {
                let (status, current_attempt, items_seen): (String, i64, i64) = transaction
                    .query_row(
                        "SELECT status, attempt_generation, items_seen FROM tasks WHERE id = ?1",
                        [task_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )?;
                if status != "running" || current_attempt != attempt_generation {
                    return Err(anyhow::anyhow!(
                        "task {task_id} is not running for purge attempt {attempt_generation}"
                    ));
                }
                items_seen
            }
            _ => 0,
        };

        // Phase 1: this root's scan/interactive tasks. Deliberately EXCLUDES
        // 'root_purge' so the worker's own active task survives for progress
        // updates; it is swept in phase 5 with the roots row.
        let n = transaction.execute(
            r#"
            DELETE FROM tasks
            WHERE id IN (
                SELECT id FROM tasks
                WHERE root_id = ?1 AND kind IN ('root_scan','interactive_folder_scan')
                LIMIT ?2
            )
            "#,
            (root_id, batch),
        )?;
        if n > 0 {
            return self.commit_root_purge_batch(
                transaction,
                task_id,
                attempt_generation,
                prior_items_seen,
                n,
            );
        }

        // Phase 2: thumbnail tasks referencing this root's files.
        let n = transaction.execute(
            r#"
            DELETE FROM tasks
            WHERE id IN (
                SELECT t.id FROM tasks t
                JOIN files f ON f.id = t.file_id
                WHERE f.root_id = ?1 AND t.kind = 'thumbnail'
                LIMIT ?2
            )
            "#,
            (root_id, batch),
        )?;
        if n > 0 {
            return self.commit_root_purge_batch(
                transaction,
                task_id,
                attempt_generation,
                prior_items_seen,
                n,
            );
        }

        // Phase 3: files (cascade removes media/user_metadata/file_tags/thumbs/thumb_blobs).
        let n = transaction.execute(
            "DELETE FROM files WHERE id IN (SELECT id FROM files WHERE root_id = ?1 LIMIT ?2)",
            (root_id, batch),
        )?;
        if n > 0 {
            return self.commit_root_purge_batch(
                transaction,
                task_id,
                attempt_generation,
                prior_items_seen,
                n,
            );
        }

        // Phase 4: folders, LEAF-FIRST, bounded to `batch` total this call.
        // folders.parent_id is ON DELETE CASCADE self-referential, so deleting a
        // non-leaf folder cascades its entire subtree in one statement (unbounded;
        // >1000 deep hits SQLite's cascade recursion cap and errors). Delete only
        // childless folders, looping within this call: removing a leaf exposes its
        // parent as a new leaf, which the next iteration picks up. This drains
        // deep/linear chains within one call instead of needing one call per row,
        // while still never deleting more than `batch` folders per invocation and
        // never triggering a cascade (a leaf has no children).
        let mut folders_deleted: i64 = 0;
        loop {
            let remaining_budget = batch - folders_deleted;
            if remaining_budget <= 0 {
                break;
            }
            let deleted_this_pass = transaction.execute(
                r#"
                DELETE FROM folders
                WHERE id IN (
                    SELECT f.id FROM folders f
                    WHERE f.root_id = ?1
                      AND NOT EXISTS (SELECT 1 FROM folders c WHERE c.parent_id = f.id)
                    LIMIT ?2
                )
                "#,
                (root_id, remaining_budget),
            )? as i64;
            if deleted_this_pass == 0 {
                break;
            }
            folders_deleted += deleted_this_pass;
        }
        if folders_deleted > 0 {
            return self.commit_root_purge_batch(
                transaction,
                task_id,
                attempt_generation,
                prior_items_seen,
                folders_deleted as usize,
            );
        }

        // Phase 5: publish the terminal snapshot before deleting the active
        // root-purge task and the roots row. The final row count is known before
        // either delete, so determinate progress reaches its exact total.
        let task_rows: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM tasks WHERE root_id = ?1",
            [root_id],
            |row| row.get(0),
        )?;
        let root_rows: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM roots WHERE id = ?1",
            [root_id],
            |row| row.get(0),
        )?;
        let final_rows = (task_rows + root_rows) as usize;
        let mut appended = false;
        if let (Some(task_id), Some(_attempt_generation)) = (task_id, attempt_generation) {
            transaction.execute(
                r#"UPDATE tasks
                   SET status = 'succeeded', updated_at = ?1, items_seen = ?2,
                       items_total = COALESCE(items_total, ?3), error = NULL
                   WHERE id = ?4"#,
                (
                    unix_timestamp(),
                    prior_items_seen + final_rows as i64,
                    prior_items_seen + final_rows as i64,
                    task_id,
                ),
            )?;
            appended = self.publish_task_notification_in_transaction(
                &transaction,
                task_id,
                None,
                notification_outbox::PublishMode::Terminal,
            )?;
        }
        transaction.execute("DELETE FROM tasks WHERE root_id = ?1", [root_id])?;
        transaction.execute("DELETE FROM roots WHERE id = ?1", [root_id])?;
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        Ok(PurgeBatchOutcome {
            rows_deleted: final_rows,
            done: true,
        })
    }

    fn commit_root_purge_batch(
        &self,
        transaction: Transaction<'_>,
        task_id: Option<i64>,
        attempt_generation: Option<i64>,
        prior_items_seen: i64,
        rows_deleted: usize,
    ) -> anyhow::Result<PurgeBatchOutcome> {
        let mut appended = false;
        if let (Some(task_id), Some(attempt_generation)) = (task_id, attempt_generation) {
            let current_items_seen = prior_items_seen
                .checked_add(rows_deleted as i64)
                .ok_or_else(|| {
                    anyhow::anyhow!("root purge progress overflow for task {task_id}")
                })?;
            let updated = transaction.execute(
                r#"UPDATE tasks
                   SET updated_at = ?1, items_seen = ?2
                   WHERE id = ?3 AND status = 'running' AND attempt_generation = ?4"#,
                (
                    unix_timestamp(),
                    current_items_seen,
                    task_id,
                    attempt_generation,
                ),
            )?;
            if updated != 1 {
                return Err(anyhow::anyhow!(
                    "task {task_id} is not running for purge attempt {attempt_generation}"
                ));
            }
            appended = self.publish_task_notification_in_transaction(
                &transaction,
                task_id,
                None,
                notification_outbox::PublishMode::Progress,
            )?;
        }
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        Ok(PurgeBatchOutcome {
            rows_deleted,
            done: false,
        })
    }

    #[cfg(test)]
    pub fn create_interactive_folder_scan_task(&self, folder_id: i64) -> anyhow::Result<i64> {
        self.create_interactive_folder_scan_task_with_origin(
            folder_id,
            NotificationOrigin::Background,
        )
    }

    pub fn create_interactive_folder_scan_task_with_origin(
        &self,
        folder_id: i64,
        origin: NotificationOrigin,
    ) -> anyhow::Result<i64> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let root_id: i64 = transaction
            .query_row(
                r#"
                SELECT folders.root_id
                FROM folders
                JOIN roots ON roots.id = folders.root_id AND roots.enabled = 1
                WHERE folders.id = ?1 AND folders.status = 'active'
                "#,
                [folder_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("folder not found: {folder_id}"))?;
        let mut statement = transaction.prepare(
            r#"
            SELECT id
            FROM tasks
            WHERE kind = 'interactive_folder_scan'
              AND root_id = ?1
              AND status = 'pending'
            ORDER BY created_at DESC, id DESC
            "#,
        )?;
        let rows = statement.query_map([root_id], |row| row.get::<_, i64>(0))?;
        let mut pending_task_ids = Vec::new();
        for row in rows {
            pending_task_ids.push(row?);
        }
        drop(statement);

        let now = unix_timestamp();
        if let Some(&keeper_task_id) = pending_task_ids.first() {
            transaction.execute(
                r#"
                UPDATE tasks
                SET folder_id = ?1,
                    updated_at = ?2,
                    items_seen = 0,
                    items_total = NULL,
                    folders_seen = 0,
                    media_files_seen = 0,
                    skipped_files = 0,
                    thumbnail_source_fingerprint = NULL,
                    error = NULL
                WHERE id = ?3
                "#,
                (folder_id, now, keeper_task_id),
            )?;
            let mut appended = self.publish_task_notification_in_transaction(
                &transaction,
                keeper_task_id,
                Some(origin),
                notification_outbox::PublishMode::Phase,
            )?;
            if pending_task_ids.len() > 1 {
                for task_id in pending_task_ids.iter().skip(1) {
                    transaction.execute(
                        r#"
                        UPDATE tasks
                        SET status = 'cancelled',
                            updated_at = ?1,
                            error = 'superseded by newer interactive folder request'
                        WHERE id = ?2
                        "#,
                        (now, task_id),
                    )?;
                    appended |= self.publish_task_notification_in_transaction(
                        &transaction,
                        *task_id,
                        None,
                        notification_outbox::PublishMode::Terminal,
                    )?;
                }
            }
            transaction.commit()?;
            self.publish_task_notification_after_commit(appended);
            return Ok(keeper_task_id);
        }
        transaction.execute(
            r#"
            INSERT INTO tasks(
                kind, priority, status, root_id, folder_id, file_id, created_at, updated_at, error
            )
            VALUES ('interactive_folder_scan', ?1, 'pending', ?2, ?3, NULL, ?4, ?4, NULL)
            "#,
            (
                INTERACTIVE_FOLDER_SCAN_TASK_PRIORITY,
                root_id,
                folder_id,
                now,
            ),
        )?;
        let task_id = transaction.last_insert_rowid();
        let appended = self.publish_task_notification_in_transaction(
            &transaction,
            task_id,
            Some(origin),
            notification_outbox::PublishMode::Phase,
        )?;
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        Ok(task_id)
    }

    pub fn reset_running_root_scan_tasks_for_recovery(&self) -> anyhow::Result<usize> {
        let transaction = self.connection.unchecked_transaction()?;
        let task_ids = task_ids_by_status_in_transaction(
            &transaction,
            "kind IN ('root_scan', 'interactive_folder_scan') AND status = 'running'",
        )?;
        let now = unix_timestamp();
        let mut appended = false;
        for task_id in &task_ids {
            transaction.execute(
                "UPDATE tasks SET status = 'pending', updated_at = ?1, error = NULL WHERE id = ?2",
                (now, task_id),
            )?;
            appended |= self.publish_task_notification_in_transaction(
                &transaction,
                *task_id,
                task_notification_recovery_origin_override(&transaction, *task_id),
                notification_outbox::PublishMode::Phase,
            )?;
        }
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        let updated = task_ids.len();
        Ok(updated)
    }

    /// Reset a `root_purge` task stuck `running` from a crash mid-purge back to
    /// `pending`, so startup recovery's drain (see `pending_drain_task_ids`)
    /// picks it up. Must run BEFORE `ensure_pending_delete_purge_tasks`: once
    /// reset to `pending`, that call's `status IN ('pending','running')` guard
    /// sees an existing task and creates no duplicate.
    pub fn reset_running_root_purge_tasks_for_recovery(&self) -> anyhow::Result<usize> {
        let transaction = self.connection.unchecked_transaction()?;
        let task_ids = task_ids_by_status_in_transaction(
            &transaction,
            "kind = 'root_purge' AND status = 'running'",
        )?;
        let now = unix_timestamp();
        let mut appended = false;
        for task_id in &task_ids {
            transaction.execute(
                "UPDATE tasks SET status = 'pending', updated_at = ?1, error = NULL WHERE id = ?2",
                (now, task_id),
            )?;
            appended |= self.publish_task_notification_in_transaction(
                &transaction,
                *task_id,
                task_notification_recovery_origin_override(&transaction, *task_id),
                notification_outbox::PublishMode::Phase,
            )?;
        }
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        let updated = task_ids.len();
        Ok(updated)
    }

    pub fn reset_running_thumbnail_tasks_for_recovery(&self) -> anyhow::Result<usize> {
        let now = unix_timestamp();
        let transaction = self.connection.unchecked_transaction()?;
        let updated = transaction.execute(
            r#"
            UPDATE tasks
            SET status = 'pending',
                updated_at = ?1,
                thumbnail_source_fingerprint = NULL,
                error = NULL
            WHERE kind = 'thumbnail' AND status = 'running'
            "#,
            [now],
        )?;
        transaction.execute(
            r#"
            UPDATE thumbs
            SET state = 'queued',
                cache_key = NULL,
                width = NULL,
                height = NULL,
                byte_size = NULL,
                error = NULL,
                served_by = NULL,
                updated_at = ?1
            WHERE profile = 'grid_320'
              AND state = 'queued'
              AND file_id IN (
                SELECT file_id FROM tasks
                WHERE kind = 'thumbnail' AND status = 'pending'
              )
            "#,
            [now],
        )?;
        transaction.commit()?;
        Ok(updated)
    }

    pub fn list_pending_root_scan_task_ids(&self) -> anyhow::Result<Vec<i64>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT tasks.id
            FROM tasks
            JOIN roots ON roots.id = tasks.root_id AND roots.enabled = 1
            WHERE tasks.kind = 'root_scan' AND tasks.status = 'pending'
            ORDER BY tasks.priority DESC, tasks.created_at ASC, tasks.id ASC
            "#,
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;

        let mut task_ids = Vec::new();
        for row in rows {
            task_ids.push(row?);
        }
        Ok(task_ids)
    }

    /// Pending `root_purge` task ids, for the startup/idle drain. Deliberately
    /// does NOT join `roots ON roots.enabled = 1`: a root mid-purge is marked
    /// `enabled = 0` (see `mark_root_for_purge`), so requiring enabled=1 would
    /// exclude the very tasks this drain exists to resume.
    pub fn list_pending_root_purge_task_ids(&self) -> anyhow::Result<Vec<i64>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT id FROM tasks
            WHERE kind = 'root_purge' AND status = 'pending'
            ORDER BY priority DESC, created_at ASC, id ASC
            "#,
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        let mut task_ids = Vec::new();
        for row in rows {
            task_ids.push(row?);
        }
        Ok(task_ids)
    }

    pub fn list_pending_interactive_folder_scan_task_ids(&self) -> anyhow::Result<Vec<i64>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT tasks.id
            FROM tasks
            JOIN folders ON folders.id = tasks.folder_id AND folders.status = 'active'
            JOIN roots ON roots.id = folders.root_id AND roots.enabled = 1
            WHERE tasks.kind = 'interactive_folder_scan' AND tasks.status = 'pending'
            ORDER BY tasks.priority DESC, tasks.created_at ASC, tasks.id ASC
            "#,
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;

        let mut task_ids = Vec::new();
        for row in rows {
            task_ids.push(row?);
        }
        Ok(task_ids)
    }

    pub(crate) fn has_pending_interactive_folder_scan_task(
        &self,
        root_id: i64,
    ) -> anyhow::Result<bool> {
        let exists: i64 = self.connection.query_row(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM tasks
              JOIN folders ON folders.id = tasks.folder_id AND folders.status = 'active'
              JOIN roots ON roots.id = folders.root_id AND roots.enabled = 1
              WHERE tasks.kind = 'interactive_folder_scan'
                AND tasks.root_id = ?1
                AND tasks.status = 'pending'
            )
            "#,
            [root_id],
            |row| row.get(0),
        )?;
        Ok(exists != 0)
    }

    #[cfg(test)]
    pub fn list_pending_thumbnail_task_ids(&self) -> anyhow::Result<Vec<i64>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT tasks.id
            FROM tasks
            JOIN files ON files.id = tasks.file_id AND files.status = 'active'
            JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
            WHERE tasks.kind = 'thumbnail' AND tasks.status = 'pending'
            ORDER BY
              tasks.priority DESC,
              tasks.attempt_generation DESC,
              tasks.updated_at DESC,
              tasks.id DESC
            "#,
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;

        let mut task_ids = Vec::new();
        for row in rows {
            task_ids.push(row?);
        }
        Ok(task_ids)
    }

    pub fn list_pending_foreground_thumbnail_task_ids(&self) -> anyhow::Result<Vec<i64>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT tasks.id
            FROM tasks
            JOIN files ON files.id = tasks.file_id AND files.status = 'active'
            JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
            WHERE tasks.kind = 'thumbnail'
              AND tasks.status = 'pending'
              AND tasks.priority > ?1
            ORDER BY
              tasks.priority DESC,
              tasks.attempt_generation DESC,
              tasks.updated_at DESC,
              tasks.id DESC
            "#,
        )?;
        let rows = statement.query_map([THUMBNAIL_BACKGROUND_PRIORITY], |row| row.get(0))?;

        let mut task_ids = Vec::new();
        for row in rows {
            task_ids.push(row?);
        }
        Ok(task_ids)
    }

    pub fn list_pending_background_thumbnail_task_ids(&self) -> anyhow::Result<Vec<i64>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT tasks.id
            FROM tasks
            JOIN files ON files.id = tasks.file_id AND files.status = 'active'
            JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
            WHERE tasks.kind = 'thumbnail'
              AND tasks.status = 'pending'
              AND tasks.priority <= ?1
            ORDER BY tasks.priority DESC, tasks.created_at ASC, tasks.id ASC
            "#,
        )?;
        let rows = statement.query_map([THUMBNAIL_BACKGROUND_PRIORITY], |row| row.get(0))?;

        let mut task_ids = Vec::new();
        for row in rows {
            task_ids.push(row?);
        }
        Ok(task_ids)
    }

    pub(crate) fn has_pending_foreground_thumbnail_task(&self) -> anyhow::Result<bool> {
        let exists: i64 = self.connection.query_row(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM tasks
              JOIN files ON files.id = tasks.file_id AND files.status = 'active'
              JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
              WHERE tasks.kind = 'thumbnail'
                AND tasks.status = 'pending'
                AND tasks.priority > ?1
            )
            "#,
            [THUMBNAIL_BACKGROUND_PRIORITY],
            |row| row.get(0),
        )?;
        Ok(exists != 0)
    }

    pub(crate) fn has_pending_foreground_thumbnail_task_for_root(
        &self,
        root_id: i64,
    ) -> anyhow::Result<bool> {
        let exists: i64 = self.connection.query_row(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM tasks
              JOIN files ON files.id = tasks.file_id AND files.status = 'active'
              JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
              WHERE tasks.kind = 'thumbnail'
                AND files.root_id = ?1
                AND tasks.status = 'pending'
                AND tasks.priority > ?2
            )
            "#,
            (root_id, THUMBNAIL_BACKGROUND_PRIORITY),
            |row| row.get(0),
        )?;
        Ok(exists != 0)
    }

    pub(crate) fn has_pending_thumbnail_task_higher_priority(
        &self,
        priority: i64,
    ) -> anyhow::Result<bool> {
        let exists: i64 = self.connection.query_row(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM tasks
              JOIN files ON files.id = tasks.file_id AND files.status = 'active'
              JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
              WHERE tasks.kind = 'thumbnail'
                AND tasks.status = 'pending'
                AND tasks.priority > ?1
            )
            "#,
            [priority],
            |row| row.get(0),
        )?;
        Ok(exists != 0)
    }

    pub fn fail_pending_root_scan_tasks_for_disabled_roots(&self) -> anyhow::Result<usize> {
        let transaction = self.connection.unchecked_transaction()?;
        let task_ids = task_ids_by_status_in_transaction(
            &transaction,
            "kind IN ('root_scan', 'interactive_folder_scan') AND status = 'pending' AND root_id IN (SELECT id FROM roots WHERE enabled = 0)",
        )?;
        let now = unix_timestamp();
        let mut appended = false;
        for task_id in &task_ids {
            transaction.execute(
                "UPDATE tasks SET status = 'failed', updated_at = ?1, error = 'root is disabled' WHERE id = ?2",
                (now, task_id),
            )?;
            appended |= self.publish_task_notification_in_transaction(
                &transaction,
                *task_id,
                None,
                notification_outbox::PublishMode::Terminal,
            )?;
        }
        let updated = task_ids.len();
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        Ok(updated)
    }

    pub fn list_tasks(&self) -> anyhow::Result<Vec<TaskRecord>> {
        // Bounded so /api/tasks stays responsive on huge libraries: the most
        // recent tasks (covering active + recent thumbnail tasks the UI needs)
        // plus every scan task (root_scan / interactive_folder_scan are few and
        // the UI must track them across polls). Pending thumbnail tasks can
        // number in the millions after a "generate all", and returning them all
        // made this the slowest poll in the app. For small task tables this
        // returns the full set, identical to before.
        const RECENT_TASK_LIMIT: i64 = 2000;
        const TASK_COLUMNS: &str = "id, kind, priority, status, root_id, folder_id, file_id, \
             created_at, updated_at, items_seen, items_total, folders_seen, media_files_seen, \
             skipped_files, thumbnail_source_fingerprint, attempt_generation, \
             root_uuid, local_binding_uuid, binding_generation, error";

        let mut seen: HashSet<i64> = HashSet::new();
        let mut tasks: Vec<TaskRecord> = Vec::new();

        // Most recent tasks by id (primary-key ordered, fast even at millions).
        let mut recent = self.connection.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks ORDER BY id DESC LIMIT ?1"
        ))?;
        for row in recent.query_map([RECENT_TASK_LIMIT], task_from_row)? {
            let task = row?;
            if seen.insert(task.id) {
                tasks.push(task);
            }
        }
        drop(recent);

        // Always include scan tasks regardless of age (few; indexed by kind).
        let mut scans = self.connection.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE kind IN ('root_scan', 'interactive_folder_scan')"
        ))?;
        for row in scans.query_map([], task_from_row)? {
            let task = row?;
            if seen.insert(task.id) {
                tasks.push(task);
            }
        }
        drop(scans);

        tasks.sort_by_key(|task| task.id);
        Ok(tasks)
    }

    /// Counts pending/running *bulk* thumbnail tasks restricted to `file_ids`.
    /// The per-file cache-stats path runs on every browse "seen" batch; a
    /// bounded indexed COUNT (idx_tasks_kind_file_status) replaces loading the
    /// entire `tasks` table into memory to filter in Rust.
    pub fn count_active_bulk_thumbnail_tasks_for_file_ids(
        &self,
        file_ids: &[i64],
    ) -> anyhow::Result<i64> {
        let unique_file_ids: Vec<i64> = dedupe_scope_file_ids(file_ids).into_iter().collect();
        if unique_file_ids.is_empty() {
            return Ok(0);
        }
        let mut sql = String::from(
            "SELECT COUNT(*) FROM tasks \
             WHERE kind = 'thumbnail' \
               AND status IN ('pending', 'running') \
               AND priority = ?1 \
               AND file_id IN (",
        );
        let mut params: Vec<i64> = vec![THUMBNAIL_BULK_PRIORITY];
        for (index, file_id) in unique_file_ids.iter().enumerate() {
            if index > 0 {
                sql.push(',');
            }
            sql.push_str(&format!("?{}", index + 2));
            params.push(*file_id);
        }
        sql.push(')');
        let count: i64 =
            self.connection
                .query_row(&sql, params_from_iter(params.iter()), |row| row.get(0))?;
        Ok(count)
    }

    pub fn get_task(&self, task_id: i64) -> anyhow::Result<Option<TaskRecord>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT id, kind, priority, status, root_id, folder_id, file_id, created_at, updated_at,
                   items_seen, items_total, folders_seen, media_files_seen, skipped_files,
                   thumbnail_source_fingerprint, attempt_generation,
                   root_uuid, local_binding_uuid, binding_generation, error
            FROM tasks
            WHERE id = ?1
            "#,
        )?;
        let mut rows = statement.query([task_id])?;
        Ok(rows.next()?.map(task_from_row).transpose()?)
    }

    #[allow(dead_code)] // retained for potential foreground-promotion reuse
    pub fn active_thumbnail_task(&self, file_id: i64) -> anyhow::Result<Option<TaskRecord>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT id, kind, priority, status, root_id, folder_id, file_id, created_at, updated_at,
                   items_seen, items_total, folders_seen, media_files_seen, skipped_files,
                   thumbnail_source_fingerprint, attempt_generation,
                   root_uuid, local_binding_uuid, binding_generation, error
            FROM tasks
            WHERE kind = 'thumbnail'
              AND file_id = ?1
              AND status IN ('pending', 'running')
            ORDER BY
              priority DESC,
              created_at ASC,
              id ASC
            LIMIT 1
            "#,
        )?;
        let mut rows = statement.query([file_id])?;
        Ok(rows.next()?.map(task_from_row).transpose()?)
    }

    #[allow(dead_code)] // retained for potential foreground-promotion reuse
    pub fn touch_pending_foreground_thumbnail_task(
        &self,
        task_id: i64,
        requested_priority: i64,
    ) -> anyhow::Result<Option<TaskRecord>> {
        if requested_priority <= THUMBNAIL_BACKGROUND_PRIORITY {
            return self.get_task(task_id);
        }
        self.connection.execute(
            r#"
            UPDATE tasks
            SET updated_at = ?1,
                attempt_generation = attempt_generation + 1
            WHERE id = ?2
              AND kind = 'thumbnail'
              AND status = 'pending'
              AND priority >= ?3
            "#,
            (unix_timestamp(), task_id, requested_priority),
        )?;
        self.get_task(task_id)
    }

    #[cfg(test)]
    pub(crate) fn mark_task_running_current_attempt_for_test(
        &self,
        task_id: i64,
    ) -> anyhow::Result<()> {
        let attempt_generation = self.current_task_attempt_generation(task_id)?;
        self.mark_task_running_for_attempt(task_id, attempt_generation)
    }

    pub fn mark_task_running_for_attempt(
        &self,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let updated = transaction.execute(
            "UPDATE tasks SET status = 'running', updated_at = ?1, error = NULL WHERE id = ?2 AND status = 'pending' AND attempt_generation = ?3",
            (unix_timestamp(), task_id, attempt_generation),
        )?;
        let mut appended = false;
        if updated == 1 {
            let task = task_record_in_transaction(&transaction, task_id)?;
            if task.kind == "root_purge" && task.items_total.is_none() {
                let total = root_purge_work_total_in_transaction(
                    &transaction,
                    task.root_id.ok_or_else(|| {
                        anyhow::anyhow!("root_purge task missing root id: {task_id}")
                    })?,
                )?;
                transaction.execute(
                    "UPDATE tasks SET items_total = ?1 WHERE id = ?2",
                    (total, task_id),
                )?;
            }
            appended = self.publish_task_notification_in_transaction(
                &transaction,
                task_id,
                None,
                notification_outbox::PublishMode::Phase,
            )?;
        }
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        self.ensure_one_task_attempt_updated(task_id, updated, "pending", attempt_generation)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn mark_thumbnail_task_running_current_attempt_for_test(
        &self,
        task_id: i64,
        source_fingerprint: &str,
    ) -> anyhow::Result<()> {
        let attempt_generation = self.current_task_attempt_generation(task_id)?;
        self.mark_thumbnail_task_running_for_attempt(
            task_id,
            attempt_generation,
            source_fingerprint,
        )
    }

    pub fn mark_thumbnail_task_running_for_attempt(
        &self,
        task_id: i64,
        attempt_generation: i64,
        source_fingerprint: &str,
    ) -> anyhow::Result<()> {
        let updated = self.connection.execute(
            r#"
            UPDATE tasks
            SET status = 'running',
                updated_at = ?1,
                thumbnail_source_fingerprint = ?2,
                error = NULL
            WHERE id = ?3
              AND kind = 'thumbnail'
              AND status = 'pending'
              AND attempt_generation = ?4
            "#,
            (
                unix_timestamp(),
                source_fingerprint,
                task_id,
                attempt_generation,
            ),
        )?;
        if updated == 0 {
            let Some(task) = self.get_task(task_id)? else {
                return Err(anyhow::anyhow!("task not found: {task_id}"));
            };
            if task.attempt_generation != attempt_generation {
                return Err(anyhow::anyhow!(
                    "task {task_id} attempt mismatch; expected attempt {}, current attempt {}",
                    attempt_generation,
                    task.attempt_generation
                ));
            }
            if task.kind == "thumbnail"
                && task.status == "running"
                && task.thumbnail_source_fingerprint.as_deref() == Some(source_fingerprint)
            {
                return Ok(());
            }
            return Err(anyhow::anyhow!(
                "task {task_id} is not pending thumbnail; current status is {}",
                task.status
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn mark_task_succeeded_current_attempt_for_test(
        &self,
        task_id: i64,
    ) -> anyhow::Result<()> {
        let attempt_generation = self.current_task_attempt_generation(task_id)?;
        self.mark_task_succeeded_for_attempt(task_id, attempt_generation)
    }

    pub fn mark_task_succeeded_for_attempt(
        &self,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<()> {
        self.mark_task_succeeded_for_attempt_with_clock(
            task_id,
            attempt_generation,
            unix_timestamp_millis,
        )
    }

    #[cfg(test)]
    pub(crate) fn mark_task_succeeded_for_attempt_at_millis_for_test(
        &self,
        task_id: i64,
        attempt_generation: i64,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.mark_task_succeeded_for_attempt_with_clock(task_id, attempt_generation, || {
            updated_at_ms
        })
    }

    fn mark_task_succeeded_for_attempt_with_clock(
        &self,
        task_id: i64,
        attempt_generation: i64,
        clock: impl FnOnce() -> i64,
    ) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let updated_at_ms = clock();
        if updated_at_ms < 0 {
            return Err(anyhow::anyhow!("task timestamp must be non-negative"));
        }
        let updated = transaction.execute(
            "UPDATE tasks SET status = 'succeeded', updated_at = ?1, error = NULL WHERE id = ?2 AND status = 'running' AND attempt_generation = ?3",
            (updated_at_ms / 1_000, task_id, attempt_generation),
        )?;
        let appended = if updated == 1 {
            self.publish_task_notification_in_transaction_at_millis(
                &transaction,
                task_id,
                None,
                notification_outbox::PublishMode::Terminal,
                updated_at_ms,
            )?
        } else {
            false
        };
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        self.ensure_one_task_attempt_updated(task_id, updated, "running", attempt_generation)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn mark_task_failed_current_attempt_for_test(
        &self,
        task_id: i64,
        error: &str,
    ) -> anyhow::Result<()> {
        let attempt_generation = self.current_task_attempt_generation(task_id)?;
        self.mark_task_failed_for_attempt(task_id, attempt_generation, error)
    }

    pub fn mark_task_failed_for_attempt(
        &self,
        task_id: i64,
        attempt_generation: i64,
        error: &str,
    ) -> anyhow::Result<()> {
        let updated = self.try_mark_task_failed_for_attempt(task_id, attempt_generation, error)?;
        if !updated {
            return self.ensure_one_task_attempt_updated(
                task_id,
                0,
                "pending or running",
                attempt_generation,
            );
        }
        Ok(())
    }

    pub(crate) fn try_mark_task_failed_for_attempt(
        &self,
        task_id: i64,
        attempt_generation: i64,
        error: &str,
    ) -> anyhow::Result<bool> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let updated =
            mark_task_failed_in_transaction(&transaction, task_id, attempt_generation, error)?;
        let appended = if updated == 1 {
            self.publish_task_notification_in_transaction(
                &transaction,
                task_id,
                None,
                notification_outbox::PublishMode::Terminal,
            )?
        } else {
            false
        };
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        Ok(updated == 1)
    }

    pub(crate) fn yield_running_root_scan_task_to_pending(
        &self,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<()> {
        self.yield_running_root_scan_task_to_pending_with_priority(
            task_id,
            attempt_generation,
            ROOT_SCAN_TASK_PRIORITY,
        )
    }

    pub(crate) fn yield_running_root_scan_task_to_pending_after_foreground_thumbnail(
        &self,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<()> {
        self.yield_running_root_scan_task_to_pending_with_priority(
            task_id,
            attempt_generation,
            ROOT_SCAN_FOREGROUND_FAIRNESS_CONSUMED_PRIORITY,
        )
    }

    fn yield_running_root_scan_task_to_pending_with_priority(
        &self,
        task_id: i64,
        attempt_generation: i64,
        next_priority: i64,
    ) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let updated = transaction.execute(
            r#"
            UPDATE tasks
            SET status = 'pending',
                priority = ?1,
                updated_at = ?2,
                attempt_generation = attempt_generation + 1,
                items_seen = 0,
                items_total = NULL,
                folders_seen = 0,
                media_files_seen = 0,
                skipped_files = 0,
                thumbnail_source_fingerprint = NULL,
                error = NULL
            WHERE id = ?3
              AND kind = 'root_scan'
              AND status = 'running'
              AND attempt_generation = ?4
            "#,
            (next_priority, unix_timestamp(), task_id, attempt_generation),
        )?;
        let appended = if updated == 1 {
            self.publish_task_notification_in_transaction(
                &transaction,
                task_id,
                None,
                notification_outbox::PublishMode::Phase,
            )?
        } else {
            false
        };
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        self.ensure_one_task_attempt_updated(
            task_id,
            updated,
            "running root_scan",
            attempt_generation,
        )?;
        Ok(())
    }

    pub(crate) fn yield_running_interactive_folder_scan_task_to_pending(
        &self,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let updated = transaction.execute(
            r#"
            UPDATE tasks
            SET status = 'pending',
                priority = ?1,
                updated_at = ?2,
                attempt_generation = attempt_generation + 1,
                items_seen = 0,
                items_total = NULL,
                folders_seen = 0,
                media_files_seen = 0,
                skipped_files = 0,
                thumbnail_source_fingerprint = NULL,
                error = NULL
            WHERE id = ?3
              AND kind = 'interactive_folder_scan'
              AND status = 'running'
              AND attempt_generation = ?4
            "#,
            (
                INTERACTIVE_FOLDER_SCAN_TASK_PRIORITY,
                unix_timestamp(),
                task_id,
                attempt_generation,
            ),
        )?;
        let appended = if updated == 1 {
            self.publish_task_notification_in_transaction(
                &transaction,
                task_id,
                None,
                notification_outbox::PublishMode::Phase,
            )?
        } else {
            false
        };
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        self.ensure_one_task_attempt_updated(
            task_id,
            updated,
            "running interactive_folder_scan",
            attempt_generation,
        )?;
        Ok(())
    }

    pub(crate) fn yield_running_thumbnail_task_to_pending(
        &self,
        task_id: i64,
        attempt_generation: i64,
        next_priority: i64,
    ) -> anyhow::Result<()> {
        let updated = self.connection.execute(
            r#"
            UPDATE tasks
            SET status = 'pending',
                priority = ?1,
                updated_at = ?2,
                attempt_generation = attempt_generation + 1,
                items_seen = 0,
                items_total = NULL,
                folders_seen = 0,
                media_files_seen = 0,
                skipped_files = 0,
                thumbnail_source_fingerprint = NULL,
                error = NULL
            WHERE id = ?3
              AND kind = 'thumbnail'
              AND status = 'running'
              AND attempt_generation = ?4
            "#,
            (next_priority, unix_timestamp(), task_id, attempt_generation),
        )?;
        self.ensure_one_task_attempt_updated(
            task_id,
            updated,
            "running thumbnail",
            attempt_generation,
        )?;
        Ok(())
    }

    pub fn cancel_task(&self, task_id: i64) -> anyhow::Result<TaskRecord> {
        let transaction = self.connection.unchecked_transaction()?;
        let target: Option<(String, Option<i64>, String, i64)> = transaction
            .query_row(
                "SELECT kind, root_id, status, items_seen FROM tasks WHERE id = ?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let updated = transaction.execute(
            r#"
            UPDATE tasks
            SET status = 'cancelled',
                updated_at = ?1,
                error = 'cancelled'
            WHERE id = ?2 AND status IN ('pending', 'running')
            "#,
            (unix_timestamp(), task_id),
        )?;
        if updated == 1
            && target
                .as_ref()
                .is_some_and(|(kind, _, status, items_seen)| {
                    kind == "root_purge" && status == "pending" && *items_seen == 0
                })
        {
            if let Some(root_id) = target.as_ref().and_then(|(_, root_id, _, _)| *root_id) {
                transaction.execute(
                    "UPDATE roots SET pending_delete_at = NULL, enabled = 1 WHERE id = ?1",
                    [root_id],
                )?;
            }
        }
        let appended = if updated == 1 {
            self.publish_task_notification_in_transaction(
                &transaction,
                task_id,
                None,
                notification_outbox::PublishMode::Terminal,
            )?
        } else {
            false
        };
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        if updated == 0 {
            let Some(task) = self.get_task(task_id)? else {
                return Err(anyhow::anyhow!("task not found: {task_id}"));
            };
            return Err(anyhow::anyhow!(
                "task {task_id} is not cancellable; current status is {}",
                task.status
            ));
        }
        self.get_task(task_id)?
            .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))
    }

    pub fn retry_task(&self, task_id: i64) -> anyhow::Result<TaskRecord> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let task = task_record_in_transaction(&transaction, task_id)?;
        if task.status != "failed" && task.status != "cancelled" {
            return Err(anyhow::anyhow!(
                "task {task_id} is not retryable; current status is {}",
                task.status
            ));
        }
        if !matches!(
            task.kind.as_str(),
            "root_scan" | "interactive_folder_scan" | "root_purge" | "thumbnail"
        ) {
            return Err(anyhow::anyhow!(
                "task {task_id} is not retryable; unsupported kind is {}",
                task.kind
            ));
        }
        if !task_retryable_in_transaction(&transaction, &task)? {
            return Err(anyhow::anyhow!(
                "task {task_id} is not retryable; current status is {}",
                task.status
            ));
        }
        let appended = self.retry_task_in_transaction(&transaction, &task)?;
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        self.get_task(task_id)?
            .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))
    }

    fn retry_task_in_transaction(
        &self,
        transaction: &Transaction<'_>,
        task: &TaskRecord,
    ) -> anyhow::Result<bool> {
        let now = unix_timestamp();
        match task.kind.as_str() {
            "root_scan" => {
                let updated = transaction.execute(
                    r#"
                    UPDATE tasks
                    SET status = 'pending', updated_at = ?1,
                        attempt_generation = attempt_generation + 1,
                        items_seen = 0, items_total = NULL,
                        folders_seen = 0, media_files_seen = 0,
                        skipped_files = 0, thumbnail_source_fingerprint = NULL,
                        error = NULL
                    WHERE id = ?2 AND status IN ('failed', 'cancelled')
                    "#,
                    (now, task.id),
                )?;
                if updated != 1 {
                    return Err(anyhow::anyhow!(
                        "task {} is not retryable; current status is {}",
                        task.id,
                        task.status
                    ));
                }
            }
            "interactive_folder_scan" => {
                let updated = transaction.execute(
                    r#"
                    UPDATE tasks
                    SET status = 'pending', priority = ?1, updated_at = ?2,
                        attempt_generation = attempt_generation + 1,
                        items_seen = 0, items_total = NULL,
                        folders_seen = 0, media_files_seen = 0,
                        skipped_files = 0, thumbnail_source_fingerprint = NULL,
                        error = NULL
                    WHERE id = ?3 AND status IN ('failed', 'cancelled')
                    "#,
                    (ROOT_SCAN_TASK_PRIORITY, now, task.id),
                )?;
                if updated != 1 {
                    return Err(anyhow::anyhow!(
                        "task {} is not retryable; current status is {}",
                        task.id,
                        task.status
                    ));
                }
            }
            "root_purge" => {
                let updated = transaction.execute(
                    r#"UPDATE tasks
                       SET status = 'pending', updated_at = ?1,
                           attempt_generation = attempt_generation + 1,
                           error = NULL
                       WHERE id = ?2 AND status IN ('failed', 'cancelled')"#,
                    (now, task.id),
                )?;
                if updated != 1 {
                    return Err(anyhow::anyhow!(
                        "task {} is not retryable; current status is {}",
                        task.id,
                        task.status
                    ));
                }
            }
            "thumbnail" => {
                let file_id = task.file_id.ok_or_else(|| {
                    anyhow::anyhow!("thumbnail task missing file id: {}", task.id)
                })?;
                let source = thumbnail_source_in_transaction(transaction, file_id)?
                    .ok_or_else(|| anyhow::anyhow!("media item not found: {file_id}"))?;
                let updated = transaction.execute(
                    r#"
                    UPDATE tasks
                    SET status = 'pending', updated_at = ?1,
                        attempt_generation = attempt_generation + 1,
                        items_seen = 0, items_total = NULL,
                        folders_seen = 0, media_files_seen = 0,
                        skipped_files = 0, thumbnail_source_fingerprint = NULL,
                        error = NULL
                    WHERE id = ?2 AND status IN ('failed', 'cancelled')
                    "#,
                    (now, task.id),
                )?;
                if updated != 1 {
                    return Err(anyhow::anyhow!(
                        "task {} is not retryable; current status is {}",
                        task.id,
                        task.status
                    ));
                }
                upsert_thumbnail_state_in_transaction(
                    transaction,
                    ThumbnailStateUpsert {
                        file_id,
                        profile: GRID_320_PROFILE.to_string(),
                        state: "queued".to_string(),
                        cache_key: None,
                        width: None,
                        height: None,
                        byte_size: None,
                        error: None,
                        source_fingerprint: Some(source.source_fingerprint(GRID_320_PROFILE)),
                    },
                )?;
                return Ok(false);
            }
            _ => unreachable!("retry task kind validated before transaction"),
        }
        self.publish_task_notification_in_transaction(
            transaction,
            task.id,
            None,
            notification_outbox::PublishMode::Phase,
        )
    }

    pub fn task_is_cancelled(&self, task_id: i64) -> anyhow::Result<bool> {
        // Read only `status` (not a full `get_task` row): the scan hot path
        // calls this per-file, and a whole-row SELECT + `task_from_row`
        // deserialization (17 columns, 4 String allocations) is wasted work for
        // a single-column check.
        let status: Option<String> = self
            .connection
            .query_row("SELECT status FROM tasks WHERE id = ?1", [task_id], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(status.as_deref() == Some("cancelled"))
    }

    pub fn ensure_task_not_cancelled(&self, task_id: i64) -> anyhow::Result<()> {
        if self.task_is_cancelled(task_id)? {
            return Err(anyhow::anyhow!("task cancelled: {task_id}"));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn update_task_scan_progress_current_attempt_for_test(
        &self,
        task_id: i64,
        progress: TaskScanProgress,
    ) -> anyhow::Result<()> {
        let attempt_generation = self.current_task_attempt_generation(task_id)?;
        self.update_task_scan_progress_for_attempt(task_id, attempt_generation, progress)
    }

    /// Test-only: classify a seeded `failed` thumbnail row's `failure_kind`
    /// directly, without going through a task-attempt-guarded publish call.
    /// Fixtures across the crate seed failed rows via `upsert_thumbnail_state`
    /// (which never touches `failure_kind`), so the Task 5 automatic
    /// `RetryFailedAndStale` transient-only gate needs an explicit way to mark
    /// a seeded row `transient` or `permanent` in tests outside `db::tests`
    /// (private field access to `self.connection` doesn't cross module
    /// boundaries).
    #[cfg(test)]
    pub(crate) fn set_thumbnail_failure_kind_for_test(
        &self,
        file_id: i64,
        profile: &str,
        failure_kind: &str,
    ) -> anyhow::Result<()> {
        self.connection.execute(
            "UPDATE thumbs SET failure_kind = ?1 WHERE file_id = ?2 AND profile = ?3",
            (failure_kind, file_id, profile),
        )?;
        Ok(())
    }

    pub fn update_task_scan_progress_for_attempt(
        &self,
        task_id: i64,
        attempt_generation: i64,
        progress: TaskScanProgress,
    ) -> anyhow::Result<()> {
        self.update_task_scan_progress_for_attempt_with_clock(
            task_id,
            attempt_generation,
            progress,
            unix_timestamp_millis,
        )
    }

    #[cfg(test)]
    pub(crate) fn update_task_scan_progress_for_attempt_at_millis_for_test(
        &self,
        task_id: i64,
        attempt_generation: i64,
        progress: TaskScanProgress,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.update_task_scan_progress_for_attempt_with_clock(
            task_id,
            attempt_generation,
            progress,
            || updated_at_ms,
        )
    }

    fn update_task_scan_progress_for_attempt_with_clock(
        &self,
        task_id: i64,
        attempt_generation: i64,
        progress: TaskScanProgress,
        clock: impl FnOnce() -> i64,
    ) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let updated_at_ms = clock();
        if updated_at_ms < 0 {
            anyhow::bail!("task progress timestamp must be non-negative");
        }
        let updated = transaction.execute(
            r#"
            UPDATE tasks
            SET updated_at = ?1,
                items_seen = ?2,
                items_total = ?3,
                folders_seen = ?4,
                media_files_seen = ?5,
                skipped_files = ?6
            WHERE id = ?7 AND status = 'running' AND attempt_generation = ?8
            "#,
            (
                updated_at_ms / 1_000,
                progress.items_seen,
                progress.items_total,
                progress.folders_seen,
                progress.media_files_seen,
                progress.skipped_files,
                task_id,
                attempt_generation,
            ),
        )?;
        let appended = if updated == 1 {
            self.publish_task_notification_in_transaction_at_millis(
                &transaction,
                task_id,
                None,
                notification_outbox::PublishMode::Progress,
                updated_at_ms,
            )?
        } else {
            false
        };
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        self.ensure_one_task_attempt_updated(task_id, updated, "running", attempt_generation)?;
        Ok(())
    }

    pub fn list_media_page(&self, query: MediaPageQuery) -> anyhow::Result<Page<MediaRecord>> {
        let limit = query.limit.clamp(1, 500);
        let sort = normalize_media_sort(&query.sort);
        let offset = query.offset.unwrap_or(0).max(0);
        let use_offset = query.offset.is_some();
        let sort_clause = match sort {
            "mtime_asc" => "files.mtime ASC, files.id ASC",
            "name_asc" => "files.name ASC, files.id ASC",
            "name_desc" => "files.name DESC, files.id DESC",
            _ => "files.mtime DESC, files.id DESC",
        };
        let mut predicates = vec!["files.status = 'active'".to_string()];
        let mut parameters = Vec::new();
        let mut folder_scoped = false;
        if let Some(root_id) = query.root_id {
            predicates.push("files.root_id = ?".to_string());
            parameters.push(Value::Integer(root_id));
        }
        if let Some(folder_id) = query.folder_id {
            let root_folder_descendant_scope = if query.include_descendants {
                match query.root_id {
                    Some(root_id) => self
                        .connection
                        .query_row(
                            "SELECT 1 FROM folders WHERE id = ?1 AND root_id = ?2 AND parent_id IS NULL",
                            (folder_id, root_id),
                            |_| Ok(()),
                        )
                        .optional()?
                        .is_some(),
                    None => false,
                }
            } else {
                false
            };
            if query.include_descendants && !root_folder_descendant_scope {
                return self
                    .list_descendant_media_page(query, folder_id, sort, offset, use_offset, limit);
            } else {
                if !root_folder_descendant_scope {
                    predicates.push("files.folder_id = ?".to_string());
                    parameters.push(Value::Integer(folder_id));
                    folder_scoped = true;
                }
            }
        }
        if let Some(kind) = query.kind.as_deref() {
            predicates.push("media.kind = ?".to_string());
            parameters.push(Value::Text(kind.to_string()));
        }
        // Only the first page pays for COUNT(*); later pages of the same scroll
        // reuse the client's retained total. On million-row folders this count
        // is the dominant per-page cost.
        let is_first_media_page = if use_offset {
            offset == 0
        } else {
            query.cursor.is_none()
        };
        let total_count: Option<i64> = if is_first_media_page {
            let count_requires_media_join = query.kind.is_some();
            let count_sql = media_count_sql(&predicates.join(" AND "), count_requires_media_join);
            Some(self.connection.query_row(
                &count_sql,
                params_from_iter(parameters.clone()),
                |row| row.get(0),
            )?)
        } else {
            None
        };
        let cursor = if use_offset {
            None
        } else {
            query
                .cursor
                .as_deref()
                .map(|cursor| decode_media_cursor(cursor, sort))
                .transpose()?
        };
        if let Some(cursor) = &cursor {
            let predicate = match sort {
                "name_asc" => "files.name > ? OR (files.name = ? AND files.id > ?)",
                "name_desc" => "files.name < ? OR (files.name = ? AND files.id < ?)",
                "mtime_asc" => "files.mtime > ? OR (files.mtime = ? AND files.id > ?)",
                _ => "files.mtime < ? OR (files.mtime = ? AND files.id < ?)",
            };
            predicates.push(format!("({predicate})"));
            match &cursor.key {
                MediaCursorKey::Name(name) => {
                    parameters.push(Value::Text(name.clone()));
                    parameters.push(Value::Text(name.clone()));
                }
                MediaCursorKey::Mtime(mtime) => {
                    parameters.push(Value::Integer(*mtime));
                    parameters.push(Value::Integer(*mtime));
                }
            }
            parameters.push(Value::Integer(cursor.id));
        }
        parameters.push(Value::Integer(limit + 1));
        if use_offset {
            parameters.push(Value::Integer(offset));
        }

        // Pin the index that already matches the scope + sort so SQLite scans it
        // in order instead of materializing a TEMP B-TREE sort over the whole
        // scope. Without this, a root-scoped browse (the default whole-library
        // view) picks the mtime index and sorts ~1M rows by name per page.
        let files_index = if folder_scoped {
            match sort {
                "mtime_asc" => Some("idx_files_folder_status_mtime_id_asc"),
                "name_asc" => Some("idx_files_folder_status_name_id"),
                "name_desc" => Some("idx_files_folder_status_name_id_desc"),
                _ => Some("idx_files_folder_status_mtime_id"),
            }
        } else if query.root_id.is_some() {
            match sort {
                "mtime_asc" => Some("idx_files_root_status_mtime_id_asc"),
                "name_asc" => Some("idx_files_root_status_name_id"),
                "name_desc" => Some("idx_files_root_status_name_id_desc"),
                _ => Some("idx_files_root_status_mtime_id"),
            }
        } else {
            None
        };
        let files_from = match files_index {
            Some(index) => format!("files INDEXED BY {index}"),
            None => "files".to_string(),
        };

        let sql = format!(
            r#"
            SELECT files.id, files.root_id, files.folder_id, files.name, files.ext,
                   files.size, files.mtime, media.kind, media.width, media.height,
                   media.duration_ms, media.codec,
                   media.preview_placeholder, media.preview_placeholder_format,
                   media.metadata_status, files.file_key,
                   thumbs.profile, thumbs.state, thumbs.short_side_px,
                   thumbs.output_format, thumbs.cache_key, thumbs.width, thumbs.height,
                   thumbs.byte_size, thumbs.error, thumbs.source_fingerprint, thumbs.served_by,
                   thumbs.updated_at
            FROM {files_from}
            LEFT JOIN media ON media.file_id = files.id
            LEFT JOIN thumbs ON thumbs.file_id = files.id AND thumbs.profile = 'grid_320'
            CROSS JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
            WHERE {where_clause}
            ORDER BY {sort_clause}
            LIMIT ?
            {offset_clause}
            "#,
            files_from = files_from,
            where_clause = predicates.join(" AND "),
            sort_clause = sort_clause,
            offset_clause = if use_offset { "OFFSET ?" } else { "" },
        );

        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(parameters), media_from_row)?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        let next_cursor = if items.len() > limit as usize {
            items.pop();
            items.last().map(|item| encode_media_cursor(sort, item))
        } else {
            None
        };
        Ok(Page {
            items,
            next_cursor,
            total_count,
        })
    }

    pub fn media_position(
        &self,
        file_id: i64,
        query: &MediaPageQuery,
    ) -> anyhow::Result<Option<i64>> {
        let Some(target) = self.get_media(file_id)? else {
            return Ok(None);
        };
        if query
            .root_id
            .is_some_and(|root_id| root_id != target.root_id)
        {
            return Ok(None);
        }
        if let Some(folder_id) = query.folder_id {
            let in_scope = if query.include_descendants {
                self.list_folder_ancestors(target.folder_id)?
                    .is_some_and(|path| path.iter().any(|folder| folder.id == folder_id))
            } else {
                target.folder_id == folder_id
            };
            if !in_scope {
                return Ok(None);
            }
        }

        let sort = normalize_media_sort(&query.sort);
        let mut predicates = vec!["files.status = 'active'".to_string()];
        let mut parameters: Vec<Value> = Vec::new();
        if let Some(root_id) = query.root_id {
            predicates.push("files.root_id = ?".to_string());
            parameters.push(Value::Integer(root_id));
        }
        if let Some(folder_id) = query.folder_id {
            if query.include_descendants {
                predicates.push(
                    r#"files.folder_id IN (
                        WITH RECURSIVE descendant_folders(id) AS (
                            SELECT id FROM folders WHERE id = ?
                            UNION ALL
                            SELECT folders.id FROM folders
                            JOIN descendant_folders ON folders.parent_id = descendant_folders.id
                        )
                        SELECT id FROM descendant_folders
                    )"#
                    .to_string(),
                );
            } else {
                predicates.push("files.folder_id = ?".to_string());
            }
            parameters.push(Value::Integer(folder_id));
        }
        match sort {
            "name_asc" => {
                predicates
                    .push("(files.name < ? OR (files.name = ? AND files.id < ?))".to_string());
                parameters.push(Value::Text(target.name.clone()));
                parameters.push(Value::Text(target.name.clone()));
                parameters.push(Value::Integer(target.id));
            }
            "name_desc" => {
                predicates
                    .push("(files.name > ? OR (files.name = ? AND files.id > ?))".to_string());
                parameters.push(Value::Text(target.name.clone()));
                parameters.push(Value::Text(target.name.clone()));
                parameters.push(Value::Integer(target.id));
            }
            "mtime_asc" => {
                predicates
                    .push("(files.mtime < ? OR (files.mtime = ? AND files.id < ?))".to_string());
                parameters.push(Value::Integer(target.mtime));
                parameters.push(Value::Integer(target.mtime));
                parameters.push(Value::Integer(target.id));
            }
            _ => {
                predicates
                    .push("(files.mtime > ? OR (files.mtime = ? AND files.id > ?))".to_string());
                parameters.push(Value::Integer(target.mtime));
                parameters.push(Value::Integer(target.mtime));
                parameters.push(Value::Integer(target.id));
            }
        }
        let sql = format!(
            "SELECT COUNT(*) FROM files CROSS JOIN roots ON roots.id = files.root_id AND roots.enabled = 1 WHERE {}",
            predicates.join(" AND ")
        );
        let index = self
            .connection
            .query_row(&sql, params_from_iter(parameters), |row| row.get(0))?;
        Ok(Some(index))
    }

    fn list_descendant_media_page(
        &self,
        query: MediaPageQuery,
        folder_id: i64,
        sort: &str,
        offset: i64,
        use_offset: bool,
        limit: i64,
    ) -> anyhow::Result<Page<MediaRecord>> {
        let sort_clause = match sort {
            "mtime_asc" => "files.mtime ASC, files.id ASC",
            "name_asc" => "files.name ASC, files.id ASC",
            "name_desc" => "files.name DESC, files.id DESC",
            _ => "files.mtime DESC, files.id DESC",
        };
        let folder_index = match sort {
            "mtime_asc" => "idx_files_folder_status_mtime_id_asc",
            "name_asc" => "idx_files_folder_status_name_id",
            "name_desc" => "idx_files_folder_status_name_id_desc",
            _ => "idx_files_folder_status_mtime_id",
        };
        let descendant_cte = r#"
            WITH RECURSIVE descendant_folders(id) AS (
                SELECT id FROM folders WHERE id = ?
                UNION ALL
                SELECT folders.id
                FROM folders
                JOIN descendant_folders ON folders.parent_id = descendant_folders.id
                WHERE folders.status = 'active'
            )
        "#;
        let mut predicates = vec![
            "files.folder_id = descendant_folders.id".to_string(),
            "files.status = 'active'".to_string(),
        ];
        let mut base_parameters = vec![Value::Integer(folder_id)];
        if let Some(root_id) = query.root_id {
            predicates.push("files.root_id = ?".to_string());
            base_parameters.push(Value::Integer(root_id));
        }
        if let Some(kind) = query.kind.as_deref() {
            predicates.push("media.kind = ?".to_string());
            base_parameters.push(Value::Text(kind.to_string()));
        }

        // First page only: the recursive-CTE COUNT(*) over the whole subtree is
        // the dominant cost of paging a large folder tree; later pages reuse the
        // client's retained total.
        let is_first_media_page = if use_offset {
            offset == 0
        } else {
            query.cursor.is_none()
        };
        let total_count: Option<i64> = if is_first_media_page {
            let count_sql = format!(
                r#"
                {descendant_cte}
                SELECT COUNT(*)
                FROM descendant_folders
                CROSS JOIN files INDEXED BY idx_files_folder_status_name_id
                LEFT JOIN media ON media.file_id = files.id
                JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
                WHERE {where_clause}
                "#,
                where_clause = predicates.join(" AND "),
            );
            Some(self.connection.query_row(
                &count_sql,
                params_from_iter(base_parameters.clone()),
                |row| row.get(0),
            )?)
        } else {
            None
        };

        let cursor = if use_offset {
            None
        } else {
            query
                .cursor
                .as_deref()
                .map(|cursor| decode_media_cursor(cursor, sort))
                .transpose()?
        };
        let mut parameters = base_parameters;
        if let Some(cursor) = &cursor {
            let predicate = match sort {
                "name_asc" => "files.name > ? OR (files.name = ? AND files.id > ?)",
                "name_desc" => "files.name < ? OR (files.name = ? AND files.id < ?)",
                "mtime_asc" => "files.mtime > ? OR (files.mtime = ? AND files.id > ?)",
                _ => "files.mtime < ? OR (files.mtime = ? AND files.id < ?)",
            };
            predicates.push(format!("({predicate})"));
            match &cursor.key {
                MediaCursorKey::Name(name) => {
                    parameters.push(Value::Text(name.clone()));
                    parameters.push(Value::Text(name.clone()));
                }
                MediaCursorKey::Mtime(mtime) => {
                    parameters.push(Value::Integer(*mtime));
                    parameters.push(Value::Integer(*mtime));
                }
            }
            parameters.push(Value::Integer(cursor.id));
        }
        parameters.push(Value::Integer(limit + 1));
        if use_offset {
            parameters.push(Value::Integer(offset));
        }

        let sql = format!(
            r#"
            {descendant_cte}
            SELECT files.id, files.root_id, files.folder_id, files.name, files.ext,
                   files.size, files.mtime, media.kind, media.width, media.height,
                   media.duration_ms, media.codec,
                   media.preview_placeholder, media.preview_placeholder_format,
                   media.metadata_status, files.file_key,
                   thumbs.profile, thumbs.state, thumbs.short_side_px,
                   thumbs.output_format, thumbs.cache_key, thumbs.width, thumbs.height,
                   thumbs.byte_size, thumbs.error, thumbs.source_fingerprint, thumbs.served_by,
                   thumbs.updated_at
            FROM descendant_folders
            CROSS JOIN files INDEXED BY {folder_index}
            LEFT JOIN media ON media.file_id = files.id
            LEFT JOIN thumbs ON thumbs.file_id = files.id AND thumbs.profile = 'grid_320'
            CROSS JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
            WHERE {where_clause}
            ORDER BY {sort_clause}
            LIMIT ?
            {offset_clause}
            "#,
            where_clause = predicates.join(" AND "),
            offset_clause = if use_offset { "OFFSET ?" } else { "" },
        );

        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(parameters), media_from_row)?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        let next_cursor = if items.len() > limit as usize {
            items.pop();
            items.last().map(|item| encode_media_cursor(sort, item))
        } else {
            None
        };
        Ok(Page {
            items,
            next_cursor,
            total_count,
        })
    }

    pub fn get_media(&self, file_id: i64) -> anyhow::Result<Option<MediaRecord>> {
        let media = {
            let mut statement = self.connection.prepare(
                r#"
                SELECT files.id, files.root_id, files.folder_id, files.name, files.ext,
                       files.size, files.mtime, media.kind, media.width, media.height,
                       media.duration_ms, media.codec,
                       media.preview_placeholder, media.preview_placeholder_format,
                       media.metadata_status, files.file_key,
                       thumbs.profile, thumbs.state, thumbs.short_side_px,
                       thumbs.output_format, thumbs.cache_key, thumbs.width, thumbs.height,
                       thumbs.byte_size, thumbs.error, thumbs.source_fingerprint, thumbs.served_by,
                       thumbs.updated_at,
                       user_metadata.rating, user_metadata.favorite, user_metadata.note
                FROM files
                LEFT JOIN media ON media.file_id = files.id
                LEFT JOIN thumbs ON thumbs.file_id = files.id AND thumbs.profile = 'grid_320'
                LEFT JOIN user_metadata ON user_metadata.file_id = files.id
                JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
                WHERE files.id = ?1 AND files.status = 'active'
                "#,
            )?;
            let mut rows = statement.query([file_id])?;
            let Some(row) = rows.next()? else {
                return Ok(None);
            };
            let mut media = media_from_row(row)?;
            let rating: Option<i64> = row.get(28)?;
            let favorite: Option<i64> = row.get(29)?;
            let note: Option<String> = row.get(30)?;
            media.rating = rating;
            media.favorite = favorite.map(|value| value != 0).unwrap_or(false);
            media.note = note;
            media
        };
        let mut media = media;
        media.tag_ids = self.list_file_tag_ids(file_id)?;
        Ok(Some(media))
    }

    pub fn get_thumbnail(
        &self,
        file_id: i64,
        profile: &str,
    ) -> anyhow::Result<Option<ThumbnailRecord>> {
        let mut thumbnail = {
            let mut statement = self.connection.prepare(
                r#"
                SELECT files.id, thumbs.profile, thumbs.state, thumbs.short_side_px,
                       thumbs.output_format, thumbs.cache_key, thumbs.width, thumbs.height,
                       thumbs.byte_size, thumbs.error, thumbs.source_fingerprint, thumbs.served_by,
                       thumbs.updated_at
                FROM files
                JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
                LEFT JOIN thumbs ON thumbs.file_id = files.id AND thumbs.profile = ?2
                WHERE files.id = ?1 AND files.status = 'active'
                "#,
            )?;
            let mut rows = statement.query((file_id, profile))?;
            let Some(row) = rows.next()? else {
                return Ok(None);
            };
            thumbnail_from_row(row, profile)?
        };
        if let Some(source) = self.get_thumbnail_source(file_id)? {
            normalize_thumbnail_record_for_source(&mut thumbnail, &source);
        }
        Ok(Some(thumbnail))
    }

    #[cfg(test)]
    pub fn reset_thumbnail_after_stale_source_for_task_attempt(
        &self,
        file_id: i64,
        profile: &str,
        error: &str,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<bool> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        if !task_attempt_running_in_transaction(&transaction, task_id, attempt_generation)? {
            transaction.commit()?;
            return Ok(false);
        }
        reset_thumbnail_after_stale_source_in_transaction(&transaction, file_id, profile, error)?;
        transaction.commit()?;
        Ok(true)
    }

    #[cfg(test)]
    pub fn publish_thumbnail_failure_for_attempted_source_for_task_attempt(
        &self,
        file_id: i64,
        profile: &str,
        attempted_source_fingerprint: Option<&str>,
        error: &str,
        failure_kind: &str,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<bool> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        if !task_attempt_running_in_transaction(&transaction, task_id, attempt_generation)? {
            transaction.commit()?;
            return Ok(false);
        }
        let published = publish_thumbnail_failure_for_attempted_source_in_transaction(
            &transaction,
            file_id,
            profile,
            attempted_source_fingerprint,
            error,
            failure_kind,
        )?;
        transaction.commit()?;
        Ok(published)
    }

    /// Atomically claims the task's failed terminal transition and applies its
    /// thumbnail side effect. `None` means this attempt no longer owned the
    /// transition; `Some` reports whether a current-source failure was stored.
    pub(crate) fn try_mark_thumbnail_task_failed_for_attempt(
        &self,
        file_id: i64,
        profile: &str,
        attempted_source_fingerprint: Option<&str>,
        error: &str,
        failure_kind: &str,
        source_changed: bool,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<Option<bool>> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let updated =
            mark_task_failed_in_transaction(&transaction, task_id, attempt_generation, error)?;
        if updated == 0 {
            transaction.commit()?;
            return Ok(None);
        }

        let failure_persisted = if source_changed {
            reset_thumbnail_after_stale_source_in_transaction(
                &transaction,
                file_id,
                profile,
                error,
            )?;
            false
        } else {
            publish_thumbnail_failure_for_attempted_source_in_transaction(
                &transaction,
                file_id,
                profile,
                attempted_source_fingerprint,
                error,
                failure_kind,
            )?
        };
        let appended = self.publish_task_notification_in_transaction(
            &transaction,
            task_id,
            None,
            notification_outbox::PublishMode::Terminal,
        )?;
        transaction.commit()?;
        self.publish_task_notification_after_commit(appended);
        Ok(Some(failure_persisted))
    }

    pub fn get_thumbnail_source(
        &self,
        file_id: i64,
    ) -> anyhow::Result<Option<ThumbnailSourceRecord>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT files.id, files.root_id, files.folder_id, files.name, files.size,
                   files.mtime, files.file_key, media.kind, media.width, media.height,
                   media.metadata_status
            FROM files
            JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
            LEFT JOIN media ON media.file_id = files.id
            WHERE files.id = ?1 AND files.status = 'active'
            "#,
        )?;
        let mut rows = statement.query([file_id])?;
        Ok(rows.next()?.map(thumbnail_source_from_row).transpose()?)
    }

    fn list_thumbnail_cache_candidate_entries(
        &self,
        scope: ThumbnailCacheScope,
        refresh_mode: ThumbnailCacheRefreshMode,
        limit: usize,
    ) -> anyhow::Result<Vec<ThumbnailCacheEntry>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let (sql, params) = thumbnail_cache_candidate_query(scope, refresh_mode, limit);
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(params), thumbnail_cache_entry_from_row)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    fn first_scoped_pending_bulk_thumbnail_task_to_wake(
        &self,
        scope: ThumbnailCacheScope,
        refresh_mode: ThumbnailCacheRefreshMode,
    ) -> anyhow::Result<Option<(i64, i64, String)>> {
        let (sql, params) = thumbnail_cache_pending_bulk_task_query(scope, refresh_mode);
        self.connection
            .query_row(&sql, params_from_iter(params), |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()
            .map_err(Into::into)
    }

    fn list_thumbnail_cache_entries_for_file_ids(
        &self,
        file_ids: &[i64],
    ) -> anyhow::Result<Vec<ThumbnailCacheEntry>> {
        let file_ids = dedupe_scope_file_ids(file_ids);
        if file_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; file_ids.len()].join(", ");
        let sql = format!(
            r#"
            SELECT
                files.id,
                files.root_id,
                files.folder_id,
                files.name,
                files.size,
                files.mtime,
                files.file_key,
                media.kind,
                media.width,
                media.height,
                media.metadata_status,
                thumbs.profile,
                thumbs.state,
                thumbs.short_side_px,
                thumbs.output_format,
                thumbs.cache_key,
                thumbs.width,
                thumbs.height,
                thumbs.byte_size,
                thumbs.error,
                thumbs.source_fingerprint,
                thumbs.served_by,
                thumbs.updated_at,
                CASE WHEN thumb_blobs.file_id IS NULL THEN 0 ELSE 1 END,
                COALESCE(thumb_blobs.byte_size, 0)
            FROM files
            JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
            LEFT JOIN media ON media.file_id = files.id
            LEFT JOIN thumbs ON thumbs.file_id = files.id AND thumbs.profile = '{GRID_320_PROFILE}'
            LEFT JOIN thumb_blobs ON thumb_blobs.file_id = files.id AND thumb_blobs.profile = '{GRID_320_PROFILE}'
            WHERE files.status = 'active'
              AND COALESCE(media.kind, '') IN ('image', 'video')
              AND files.id IN ({placeholders})
            ORDER BY files.id ASC
            "#
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(
            params_from_iter(file_ids.into_iter().map(Value::Integer)),
            thumbnail_cache_entry_from_row,
        )?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// List up to `limit` scoped thumbnail-cache entries whose `file_id` is
    /// greater than `after_file_id`, ordered by `file_id`. Unlike
    /// [`Self::list_thumbnail_cache_candidate_entries`], this does NOT filter to
    /// refresh candidates — it returns every scoped entry in the keyset window
    /// so the caller can advance a cursor past cached files it visited.
    ///
    /// This is the driver of the whole-library "generate all" pass: a candidate
    /// query with `ORDER BY file_id LIMIT candidates` re-scans from the start on
    /// every batch (each pass skips already-enqueued files via a per-row task
    /// lookup), which is O(files) *per batch* and monopolizes the single writer
    /// for seconds at a time on a large library. A bounded keyset window scans
    /// exactly `limit` files per batch and the caller advances the cursor, so
    /// the whole library costs one scan total, split into short writer jobs that
    /// interleave with interactive writes.
    fn list_thumbnail_cache_window_entries(
        &self,
        scope: ThumbnailCacheScope,
        after_file_id: i64,
        limit: usize,
    ) -> anyhow::Result<Vec<ThumbnailCacheEntry>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut parts = thumbnail_cache_scope_query_parts(scope);
        parts.params.push(Value::Integer(after_file_id));
        let cursor_param = parts.params.len();
        parts.params.push(Value::Integer(limit as i64));
        let limit_param = parts.params.len();
        let select = format!(
            r#"
            SELECT
                files.id,
                files.root_id,
                files.folder_id,
                files.name,
                files.size,
                files.mtime,
                files.file_key,
                media.kind,
                media.width,
                media.height,
                media.metadata_status,
                thumbs.profile,
                thumbs.state,
                thumbs.short_side_px,
                thumbs.output_format,
                thumbs.cache_key,
                thumbs.width,
                thumbs.height,
                thumbs.byte_size,
                thumbs.error,
                thumbs.source_fingerprint,
                thumbs.served_by,
                thumbs.updated_at,
                CASE WHEN thumb_blobs.file_id IS NULL THEN 0 ELSE 1 END,
                COALESCE(thumb_blobs.byte_size, 0)
            FROM files
            JOIN roots ON roots.id = files.root_id
            LEFT JOIN media ON media.file_id = files.id
            LEFT JOIN thumbs ON thumbs.file_id = files.id AND thumbs.profile = '{GRID_320_PROFILE}'
            LEFT JOIN thumb_blobs ON thumb_blobs.file_id = files.id AND thumb_blobs.profile = '{GRID_320_PROFILE}'
            WHERE {where_clause}
              AND files.id > ?{cursor_param}
            ORDER BY files.id ASC
            LIMIT ?{limit_param}
            "#,
            where_clause = parts.where_clause
        );
        let sql = if parts.prefix.is_empty() {
            select
        } else {
            format!("{} {select}", parts.prefix)
        };
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(
            params_from_iter(parts.params),
            thumbnail_cache_entry_from_row,
        )?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    pub fn request_thumbnail_task(
        &self,
        file_id: i64,
        profile: &str,
        priority: ThumbnailTaskPriority,
    ) -> anyhow::Result<ThumbnailTaskRequest> {
        if profile != GRID_320_PROFILE {
            return Err(anyhow::anyhow!("unsupported thumbnail profile: {profile}"));
        }

        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let outcome =
            Self::request_thumbnail_task_in_transaction(&transaction, file_id, profile, priority)?;
        transaction.commit()?;

        // Reproduce the original return exactly: the terminal no-op path used the
        // in-transaction record; every other path re-reads the committed row.
        let thumbnail = match outcome.terminal_thumbnail {
            Some(thumbnail) => thumbnail,
            None => self
                .get_thumbnail(file_id, profile)?
                .ok_or_else(|| anyhow::anyhow!("media item not found: {file_id}"))?,
        };
        Ok(ThumbnailTaskRequest {
            thumbnail,
            task_id: outcome.task_id,
            queued: outcome.queued,
            should_enqueue: outcome.should_enqueue,
        })
    }

    /// Core of [`Database::request_thumbnail_task`] operating on a caller-owned
    /// transaction so a batch enqueue can run many requests under a single
    /// `BEGIN IMMEDIATE`/`COMMIT` instead of one transaction (plus one trailing
    /// `get_thumbnail`) per file.
    fn request_thumbnail_task_in_transaction(
        transaction: &Transaction<'_>,
        file_id: i64,
        profile: &str,
        priority: ThumbnailTaskPriority,
    ) -> anyhow::Result<ThumbnailTaskRequestOutcome> {
        if profile != GRID_320_PROFILE {
            return Err(anyhow::anyhow!("unsupported thumbnail profile: {profile}"));
        }
        let source = thumbnail_source_in_transaction(transaction, file_id)?
            .ok_or_else(|| anyhow::anyhow!("media item not found: {file_id}"))?;
        let source_fingerprint = source.source_fingerprint(profile);

        let requested_priority = priority.task_priority();
        let current = thumbnail_for_update_in_transaction(transaction, file_id, profile)?;
        if let Some(current) = current {
            let ready_has_required_blob = if requested_priority <= THUMBNAIL_BULK_PRIORITY
                && current.state == "ready"
                && current.source_fingerprint.as_deref() == Some(source_fingerprint.as_str())
            {
                thumb_blob_exists_in_transaction(transaction, file_id, profile)?
            } else {
                true
            };
            if thumbnail_state_is_terminal_for_current_source(
                &current,
                &source,
                source_fingerprint.as_str(),
                ready_has_required_blob,
            ) {
                let task_id = latest_thumbnail_task_id_in_transaction(transaction, file_id)?;
                return Ok(ThumbnailTaskRequestOutcome {
                    task_id,
                    queued: false,
                    accepted: false,
                    should_enqueue: false,
                    terminal_thumbnail: Some(current),
                });
            }
        }
        let existing_pending_task = pending_thumbnail_task(transaction, file_id)?;
        let existing_running_task = running_thumbnail_task(transaction, file_id)?;
        let mut queued = false;
        let mut accepted = false;
        let mut should_enqueue = false;
        let task_id = if let Some(existing_pending_task) = existing_pending_task {
            if existing_pending_task.priority < requested_priority {
                transaction.execute(
                    r#"
                    UPDATE tasks
                    SET priority = ?1,
                        updated_at = ?2
                    WHERE id = ?3
                      AND kind = 'thumbnail'
                      AND status = 'pending'
                      AND priority < ?1
                    "#,
                    (
                        requested_priority,
                        unix_timestamp(),
                        existing_pending_task.id,
                    ),
                )?;
                accepted = true;
                should_enqueue = true;
            } else if existing_pending_task.priority == requested_priority
                && requested_priority <= THUMBNAIL_BULK_PRIORITY
            {
                should_enqueue = true;
            } else if requested_priority > THUMBNAIL_BACKGROUND_PRIORITY {
                transaction.execute(
                    r#"
                    UPDATE tasks
                    SET updated_at = ?1,
                        attempt_generation = attempt_generation + 1
                    WHERE id = ?2
                      AND kind = 'thumbnail'
                      AND status = 'pending'
                    "#,
                    (unix_timestamp(), existing_pending_task.id),
                )?;
                should_enqueue = true;
            }
            existing_pending_task.id
        } else if let Some(existing_running_task) = existing_running_task {
            if existing_running_task.priority >= requested_priority {
                existing_running_task.id
            } else {
                let now = unix_timestamp();
                transaction.execute(
                    r#"
                    INSERT INTO tasks(
                        kind, priority, status, root_id, folder_id, file_id, created_at, updated_at,
                        thumbnail_source_fingerprint, error
                    )
                    VALUES ('thumbnail', ?1, 'pending', NULL, NULL, ?2, ?3, ?3, ?4, NULL)
                    "#,
                    (
                        requested_priority,
                        file_id,
                        now,
                        source_fingerprint.as_str(),
                    ),
                )?;
                queued = true;
                accepted = true;
                should_enqueue = true;
                transaction.last_insert_rowid()
            }
        } else {
            let now = unix_timestamp();
            transaction.execute(
                r#"
                INSERT INTO tasks(
                    kind, priority, status, root_id, folder_id, file_id, created_at, updated_at,
                    thumbnail_source_fingerprint, error
                )
                VALUES ('thumbnail', ?1, 'pending', NULL, NULL, ?2, ?3, ?3, ?4, NULL)
                "#,
                (
                    requested_priority,
                    file_id,
                    now,
                    source_fingerprint.as_str(),
                ),
            )?;
            queued = true;
            accepted = true;
            should_enqueue = true;
            transaction.last_insert_rowid()
        };
        if queued {
            upsert_thumbnail_state_in_transaction(
                transaction,
                ThumbnailStateUpsert {
                    file_id,
                    profile: profile.to_string(),
                    state: "queued".to_string(),
                    cache_key: None,
                    width: None,
                    height: None,
                    byte_size: None,
                    error: None,
                    source_fingerprint: Some(source_fingerprint),
                },
            )?;
        }
        Ok(ThumbnailTaskRequestOutcome {
            task_id: Some(task_id),
            queued,
            accepted,
            should_enqueue,
            terminal_thumbnail: None,
        })
    }

    /// Transaction-bound twin of [`Database::reset_failed_thumbnail_for_manual_retry`]
    /// so batch enqueue can reset failed rows inside its single transaction.
    /// Only resets rows classified `transient` — this is the automatic bulk
    /// `RetryFailedAndStale` path (called from `enqueue_thumbnail_cache_entries`
    /// / `enqueue_thumbnail_cache_tasks_for_file_ids`, never from the explicit
    /// per-task `retry_task` path, which resets its task/thumbnail rows directly
    /// and is unaffected by this gate).
    fn reset_failed_thumbnail_for_manual_retry_in_transaction(
        transaction: &Transaction<'_>,
        file_id: i64,
        profile: &str,
    ) -> anyhow::Result<()> {
        transaction.execute(
            r#"
            UPDATE thumbs
            SET state = 'pending',
                cache_key = NULL,
                width = NULL,
                height = NULL,
                byte_size = NULL,
                error = NULL,
                source_fingerprint = NULL,
                served_by = NULL,
                updated_at = ?1
            WHERE file_id = ?2
              AND profile = ?3
              AND state = 'failed'
              AND failure_kind = 'transient'
            "#,
            (unix_timestamp(), file_id, profile),
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn get_thumbnail_cache_stats(
        &self,
        scope: ThumbnailCacheScope,
    ) -> anyhow::Result<ThumbnailCacheStats> {
        get_thumbnail_cache_stats_for_scope(self, scope)
    }

    pub fn get_thumbnail_cache_snapshot(
        &self,
        scope: ThumbnailCacheScope,
        profile: &str,
        failure_limit: usize,
    ) -> anyhow::Result<ThumbnailCacheSnapshot> {
        self.get_thumbnail_cache_snapshot_with_hook(scope, profile, failure_limit, || {})
    }

    fn get_thumbnail_cache_snapshot_with_hook(
        &self,
        scope: ThumbnailCacheScope,
        profile: &str,
        failure_limit: usize,
        after_stats: impl FnOnce(),
    ) -> anyhow::Result<ThumbnailCacheSnapshot> {
        let transaction = self.connection.unchecked_transaction()?;
        let stats = get_thumbnail_cache_stats_for_scope_on_connection(&transaction, scope)?;
        after_stats();
        let mut failure_details = thumbnail_failures_on_connection(
            &transaction,
            profile,
            None,
            scope,
            failure_limit.saturating_add(1),
        )?;
        let failure_details_truncated = failure_details.len() > failure_limit;
        failure_details.truncate(failure_limit);
        transaction.commit()?;
        Ok(ThumbnailCacheSnapshot {
            stats,
            failure_details,
            failure_details_truncated,
        })
    }

    #[cfg(test)]
    fn get_thumbnail_cache_snapshot_with_hook_for_test(
        &self,
        scope: ThumbnailCacheScope,
        profile: &str,
        failure_limit: usize,
        after_stats: impl FnOnce(),
    ) -> anyhow::Result<ThumbnailCacheSnapshot> {
        self.get_thumbnail_cache_snapshot_with_hook(scope, profile, failure_limit, after_stats)
    }

    pub fn list_thumbnail_failures(
        &self,
        profile: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<ThumbnailFailureRecord>> {
        thumbnail_failures(
            self,
            profile,
            None,
            ThumbnailCacheScope {
                root_id: None,
                folder_id: None,
                include_descendants: false,
            },
            limit,
        )
    }

    pub fn get_thumbnail_failure(
        &self,
        file_id: i64,
        profile: &str,
    ) -> anyhow::Result<Option<ThumbnailFailureRecord>> {
        thumbnail_failure_by_file_on_connection(&self.connection, profile, file_id)
    }

    #[cfg(test)]
    pub fn get_thumbnail_cache_stats_for_file_ids(
        &self,
        file_ids: &[i64],
    ) -> anyhow::Result<ThumbnailCacheStats> {
        let entries = self.list_thumbnail_cache_entries_for_file_ids(file_ids)?;
        Ok(build_thumbnail_cache_stats_for_file_ids(
            self, file_ids, &entries,
        ))
    }

    pub fn enqueue_thumbnail_cache_tasks(
        &self,
        scope: ThumbnailCacheScope,
        refresh_mode: ThumbnailCacheRefreshMode,
        limit: usize,
        after_file_id: Option<i64>,
    ) -> anyhow::Result<ThumbnailCacheEnqueueResult> {
        if let Some(cursor) = after_file_id {
            return self.enqueue_thumbnail_cache_window(scope, refresh_mode, limit, cursor);
        }
        let entries = crate::debug::timed("enqueue_cache/candidate_query", || {
            self.list_thumbnail_cache_candidate_entries(scope, refresh_mode, limit)
        })?;
        let loop_start = std::time::Instant::now();
        let (accepted_count, mut task_id_to_wake) =
            self.enqueue_thumbnail_cache_entries(&entries, refresh_mode, limit)?;
        debug_log!(
            "enqueue_cache/loop: {}ms ({} candidates, {} accepted)",
            loop_start.elapsed().as_millis(),
            entries.len(),
            accepted_count
        );
        if task_id_to_wake.is_none() && limit > 0 {
            if let Some((task_id, file_id, status)) =
                self.first_scoped_pending_bulk_thumbnail_task_to_wake(scope, refresh_mode)?
            {
                if refresh_mode.requeues_transient_failures() && status == "failed" {
                    self.reset_failed_thumbnail_for_manual_retry(file_id, GRID_320_PROFILE)?;
                }
                task_id_to_wake = Some(task_id);
            }
        }
        // Do NOT run the O(scope) whole-stats aggregate here — on a large
        // descendant scope it was a multi-second query holding the single
        // writer (a 13s writer job = frozen grid). The only consumer of this
        // response's stats is the "generate current folder/tree" loop, which
        // terminates on `accepted_count == 0`; it also reads
        // `pending_candidate_count`, which we derive cheaply from the candidate
        // entries already in memory (candidates matching the refresh mode that
        // this batch did not accept). Whole-library stats are polled separately
        // by the settings tab (TTL-cached, off the writer).
        let pending_candidate_count = entries
            .iter()
            .filter(|entry| thumbnail_cache_entry_matches_refresh_mode(entry, refresh_mode))
            .count() as i64;
        let stats = ThumbnailCacheStats {
            pending_candidate_count,
            ..ThumbnailCacheStats::default()
        };
        Ok(ThumbnailCacheEnqueueResult {
            accepted_count,
            task_id_to_wake,
            stats,
            next_cursor: None,
        })
    }

    /// Windowed (keyset) variant used by the whole-library "generate all" pass.
    /// Scans exactly one bounded window of `limit` files past `after_file_id`,
    /// enqueues the candidates found in it, and reports the cursor to resume
    /// from. It deliberately skips the O(scope) work the whole-scope path does
    /// per call — the candidate pre-filter that re-scans from the start every
    /// batch, the pending-task wake fallback, and the full stats aggregate — so
    /// each writer job stays short (bounded by the window) and interleaves with
    /// interactive writes. Progress is surfaced by the separate cached stats
    /// route, so omitting stats here costs the caller nothing.
    fn enqueue_thumbnail_cache_window(
        &self,
        scope: ThumbnailCacheScope,
        refresh_mode: ThumbnailCacheRefreshMode,
        limit: usize,
        after_file_id: i64,
    ) -> anyhow::Result<ThumbnailCacheEnqueueResult> {
        let window_start = std::time::Instant::now();
        let entries = self.list_thumbnail_cache_window_entries(scope, after_file_id, limit)?;
        let scanned = entries.len();
        let last_file_id = entries.last().map(|entry| entry.source.file_id);
        // `usize::MAX` cap: the window itself bounds the batch, so enqueue every
        // candidate the window surfaced rather than stopping at an accept count.
        let (accepted_count, task_id_to_wake) =
            self.enqueue_thumbnail_cache_entries(&entries, refresh_mode, usize::MAX)?;
        // A short window means we reached the end of the scope; otherwise resume
        // after the last file we scanned (candidate or not).
        let next_cursor = if scanned == limit { last_file_id } else { None };
        debug_log!(
            "enqueue_cache/window: {}ms (cursor {} -> {:?}, {} scanned, {} accepted)",
            window_start.elapsed().as_millis(),
            after_file_id,
            next_cursor,
            scanned,
            accepted_count
        );
        Ok(ThumbnailCacheEnqueueResult {
            accepted_count,
            task_id_to_wake,
            stats: ThumbnailCacheStats::default(),
            next_cursor,
        })
    }

    /// Enqueue thumbnail tasks for the refresh-mode-matching entries in `entries`
    /// under one `BEGIN IMMEDIATE`/`COMMIT`, stopping once `accept_limit`
    /// candidates are accepted. Returns `(accepted_count, task_id_to_wake)`.
    /// Shared by the whole-scope and windowed enqueue paths. Batching under one
    /// transaction (rather than a commit + `get_thumbnail` per file) keeps the
    /// single writer's hold short.
    fn enqueue_thumbnail_cache_entries(
        &self,
        entries: &[ThumbnailCacheEntry],
        refresh_mode: ThumbnailCacheRefreshMode,
        accept_limit: usize,
    ) -> anyhow::Result<(usize, Option<i64>)> {
        let mut accepted_count = 0usize;
        let mut task_id_to_wake = None;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        for entry in entries {
            if accepted_count >= accept_limit {
                break;
            }
            if !thumbnail_cache_entry_matches_refresh_mode(entry, refresh_mode) {
                continue;
            }
            let file_id = entry.source.file_id;
            let status = thumbnail_cache_status(entry);
            if refresh_mode.requeues_transient_failures() && status == ThumbnailCacheStatus::Failed
            {
                Self::reset_failed_thumbnail_for_manual_retry_in_transaction(
                    &transaction,
                    file_id,
                    GRID_320_PROFILE,
                )?;
            }
            let outcome = Self::request_thumbnail_task_in_transaction(
                &transaction,
                file_id,
                GRID_320_PROFILE,
                ThumbnailTaskPriority::Bulk,
            )?;
            if outcome.accepted {
                accepted_count += 1;
            }
            if outcome.should_enqueue && task_id_to_wake.is_none() {
                task_id_to_wake = outcome.task_id;
            }
        }
        transaction.commit()?;
        Ok((accepted_count, task_id_to_wake))
    }

    pub fn enqueue_thumbnail_cache_tasks_for_file_ids(
        &self,
        file_ids: &[i64],
        refresh_mode: ThumbnailCacheRefreshMode,
        limit: usize,
    ) -> anyhow::Result<ThumbnailCacheEnqueueResult> {
        let entries = self.list_thumbnail_cache_entries_for_file_ids(file_ids)?;
        let mut accepted_count = 0usize;
        let mut task_id_to_wake = None;
        // One transaction for the whole batch: this path runs on every browse
        // "seen" persist (~every 500ms), so collapsing N per-file commits plus
        // trailing get_thumbnail reads into a single commit keeps the global
        // write lock held only briefly.
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        for entry in &entries {
            if accepted_count >= limit {
                break;
            }
            if !thumbnail_cache_entry_matches_refresh_mode(entry, refresh_mode) {
                continue;
            }
            let file_id = entry.source.file_id;
            let status = thumbnail_cache_status(entry);
            if refresh_mode.requeues_transient_failures() && status == ThumbnailCacheStatus::Failed
            {
                Self::reset_failed_thumbnail_for_manual_retry_in_transaction(
                    &transaction,
                    file_id,
                    GRID_320_PROFILE,
                )?;
            }
            let outcome = Self::request_thumbnail_task_in_transaction(
                &transaction,
                file_id,
                GRID_320_PROFILE,
                ThumbnailTaskPriority::Bulk,
            )?;
            if outcome.accepted {
                accepted_count += 1;
            }
            if outcome.should_enqueue && task_id_to_wake.is_none() {
                task_id_to_wake = outcome.task_id;
            }
            if accepted_count >= limit {
                break;
            }
        }
        transaction.commit()?;
        // Reuse the candidate entries already fetched above instead of
        // re-querying them; this path runs on every browse "seen" batch.
        let stats = build_thumbnail_cache_stats_for_file_ids(self, file_ids, &entries);
        Ok(ThumbnailCacheEnqueueResult {
            accepted_count,
            task_id_to_wake,
            stats,
            next_cursor: None,
        })
    }

    /// Only resets rows classified `transient` — see the `_in_transaction`
    /// twin's doc comment above for why this gate is safe for its (automatic
    /// bulk enqueue only) callers.
    fn reset_failed_thumbnail_for_manual_retry(
        &self,
        file_id: i64,
        profile: &str,
    ) -> anyhow::Result<()> {
        self.connection.execute(
            r#"
            UPDATE thumbs
            SET state = 'pending',
                cache_key = NULL,
                width = NULL,
                height = NULL,
                byte_size = NULL,
                error = NULL,
                source_fingerprint = NULL,
                served_by = NULL,
                updated_at = ?1
            WHERE file_id = ?2
              AND profile = ?3
              AND state = 'failed'
              AND failure_kind = 'transient'
            "#,
            (unix_timestamp(), file_id, profile),
        )?;
        Ok(())
    }

    pub fn clear_thumbnail_cache(&self) -> anyhow::Result<ThumbnailCacheClearResult> {
        let transaction = self.connection.unchecked_transaction()?;
        let deleted_blob_count = transaction.execute("DELETE FROM thumb_blobs", [])?;
        // Preview tiers are regenerable from the same originals; clearing the
        // thumbnail cache must not leave a second stale blob store behind.
        transaction.execute("DELETE FROM preview_blobs", [])?;
        transaction.execute("DELETE FROM preview_failures", [])?;
        let reset_thumbnail_count = transaction.execute(
            r#"
            UPDATE thumbs
            SET state = 'pending',
                cache_key = NULL,
                width = NULL,
                height = NULL,
                byte_size = NULL,
                error = NULL,
                source_fingerprint = NULL,
                served_by = NULL,
                updated_at = ?1
            WHERE state <> 'pending'
               OR cache_key IS NOT NULL
               OR width IS NOT NULL
               OR height IS NOT NULL
               OR byte_size IS NOT NULL
               OR error IS NOT NULL
               OR source_fingerprint IS NOT NULL
               OR served_by IS NOT NULL
            "#,
            [unix_timestamp()],
        )?;
        transaction.commit()?;
        Ok(ThumbnailCacheClearResult {
            deleted_blob_count,
            reset_thumbnail_count,
        })
    }

    /// Cancel every *pending* bulk-priority (whole-library) thumbnail task,
    /// leaving higher-priority (viewport/selected/ahead) work and already
    /// generated blobs untouched. Used when the user is in a generation mode
    /// that does NOT do whole-library background generation ("renderedOnly" /
    /// "cacheOnly"): any leftover bulk queue from a previous "generate all" /
    /// "allPersist" session should not keep the worker busy on files the user
    /// isn't looking at. Returns the number of tasks cancelled.
    ///
    /// Only `pending` bulk tasks are cancelled — a `running` one is already
    /// decoding and finishes on its own. The matching `thumbs` rows are left in
    /// `queued`; the next viewport request re-drives them, and the cache-status
    /// query treats queued-without-a-task as `missing`, so nothing is lost.
    pub fn cancel_pending_bulk_thumbnail_tasks(&self) -> anyhow::Result<usize> {
        let cancelled = self.connection.execute(
            r#"
            UPDATE tasks
            SET status = 'cancelled',
                updated_at = ?1
            WHERE kind = 'thumbnail'
              AND status = 'pending'
              AND priority = ?2
            "#,
            (unix_timestamp(), THUMBNAIL_BULK_PRIORITY),
        )?;
        Ok(cancelled)
    }

    #[allow(dead_code)]
    pub fn sync_thumbnail_priority_scope(
        &self,
        root_id: i64,
        selected_file_ids: &[i64],
        visible_file_ids: &[i64],
        ahead_file_ids: &[i64],
    ) -> anyhow::Result<usize> {
        // Backward-compatible wrapper: no retention set and DemoteToBackground
        // means leavers are only demoted to background, never cancelled or
        // bulk-demoted — the pre-change behavior.
        Ok(self
            .sync_thumbnail_priority_scope_with_promotions(
                root_id,
                selected_file_ids,
                visible_file_ids,
                ahead_file_ids,
                &[],
                ThumbnailScopeLeaverPolicy::DemoteToBackground,
            )?
            .updated)
    }

    pub fn sync_thumbnail_priority_scope_with_promotions(
        &self,
        root_id: i64,
        selected_file_ids: &[i64],
        visible_file_ids: &[i64],
        ahead_file_ids: &[i64],
        retained_file_ids: &[i64],
        leaver_policy: ThumbnailScopeLeaverPolicy,
    ) -> anyhow::Result<ThumbnailPriorityScopeSyncResult> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let root_exists: Option<i64> = transaction
            .query_row("SELECT id FROM roots WHERE id = ?1", [root_id], |row| {
                row.get(0)
            })
            .optional()?;
        if root_exists.is_none() {
            return Err(anyhow::anyhow!("root not found: {root_id}"));
        }

        let selected_file_ids = dedupe_scope_file_ids(selected_file_ids);
        let visible_file_ids = dedupe_scope_file_ids(visible_file_ids);
        let ahead_file_ids = dedupe_scope_file_ids(ahead_file_ids);
        // Files to keep warm even when out of scope (client recency grace
        // window). A leaver in this set is demoted to background, never
        // cancelled/bulk-demoted, so scrolling back within a few seconds keeps
        // its task alive.
        let retained_file_ids: HashSet<i64> = dedupe_scope_file_ids(retained_file_ids)
            .into_iter()
            .collect();
        let now = unix_timestamp();
        let mut updated = 0;
        // Bound the rescan to tasks that can actually change priority on this
        // sync: files entering the new selected/visible/ahead scope (promotion
        // candidates) plus any task currently elevated above the background
        // baseline (demotion candidates now that the viewport moved). Scanning
        // every pending task in the root made each scroll an
        // O(pending-tasks-in-root) write burst under the API lock —
        // pathological right after a "generate all", which queues a bulk task
        // per file. Out-of-scope bulk tasks stay at bulk priority (they are
        // already at/below background, so they need no demotion).
        let scope_file_ids: Vec<i64> = selected_file_ids
            .iter()
            .chain(visible_file_ids.iter())
            .chain(ahead_file_ids.iter())
            .copied()
            .collect::<HashSet<i64>>()
            .into_iter()
            .collect();
        // The two candidate sets above are read as two separate,
        // index-friendly queries instead of one query with an OR
        // disjunction. A single `(tasks.priority > ? OR tasks.file_id IN
        // (...))` predicate defeats the tasks indexes: SQLite ends up
        // driving the join from `files` (`SCAN files USING COVERING INDEX
        // idx_files_root`), which costs O(files-in-root) per sync — a
        // multi-hundred-ms stall on million-file roots, taken while holding
        // the single WAL writer. Splitting into a demotion-candidate query
        // driven from `tasks` (`idx_tasks_kind_status_priority`) and a
        // promotion-candidate query keyed by file id (`idx_tasks_kind_file_status`)
        // keeps both queries index-driven; the results are deduped by task
        // id before the update loop runs.
        let mut seen_task_ids: HashSet<i64> = HashSet::new();
        let mut pending_tasks: Vec<(i64, i64, i64, String)> = Vec::new();

        // Query 1: demotion candidates — tasks already elevated above the
        // background baseline, regardless of whether their file is still in
        // scope. Driven from `tasks` via idx_tasks_kind_status_priority.
        {
            let mut statement = transaction.prepare(
                "SELECT tasks.id, tasks.file_id, tasks.priority, tasks.status \
                 FROM tasks INDEXED BY idx_tasks_kind_status_priority \
                 JOIN files ON files.id = tasks.file_id \
                 WHERE tasks.kind = 'thumbnail' \
                   AND tasks.status IN ('pending', 'running') \
                   AND tasks.priority > ?1 \
                   AND files.root_id = ?2",
            )?;
            let rows = statement.query_map((THUMBNAIL_BACKGROUND_PRIORITY, root_id), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let task = row?;
                if seen_task_ids.insert(task.0) {
                    pending_tasks.push(task);
                }
            }
        }

        // Query 2: promotion candidates — tasks for files entering the new
        // selected/visible/ahead scope. Seeked per file id via
        // idx_tasks_kind_file_status.
        if !scope_file_ids.is_empty() {
            let mut select_sql = String::from(
                "SELECT tasks.id, tasks.file_id, tasks.priority, tasks.status \
                 FROM tasks \
                 JOIN files ON files.id = tasks.file_id \
                 WHERE tasks.kind = 'thumbnail' \
                   AND tasks.status IN ('pending', 'running') \
                   AND files.root_id = ?1 \
                   AND tasks.file_id IN (",
            );
            let mut select_params: Vec<i64> = vec![root_id];
            for (index, file_id) in scope_file_ids.iter().enumerate() {
                if index > 0 {
                    select_sql.push(',');
                }
                select_sql.push_str(&format!("?{}", index + 2));
                select_params.push(*file_id);
            }
            select_sql.push(')');
            let mut statement = transaction.prepare(&select_sql)?;
            let rows = statement.query_map(params_from_iter(select_params.iter()), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let task = row?;
                if seen_task_ids.insert(task.0) {
                    pending_tasks.push(task);
                }
            }
        }

        let mut promoted_pending_task_ids = Vec::new();
        let mut cancelled_leaver_count = 0usize;
        for (task_id, file_id, current_priority, status) in pending_tasks {
            let in_scope = selected_file_ids.contains(&file_id)
                || visible_file_ids.contains(&file_id)
                || ahead_file_ids.contains(&file_id);

            // A leaver (out of scope) outside the retention grace window is a
            // candidate for cancel/bulk-demote per the mode. In scope, or still
            // retained, or under the DemoteToBackground (legacy) policy → fall
            // through to the normal re-rank (out-of-scope resolves to
            // background, keeping it warm). A task already at/below bulk is left
            // alone. DemoteToBackground never cancels/bulk-demotes.
            let is_cancellable_leaver = !in_scope
                && !retained_file_ids.contains(&file_id)
                && current_priority > THUMBNAIL_BULK_PRIORITY
                && leaver_policy != ThumbnailScopeLeaverPolicy::DemoteToBackground;
            if is_cancellable_leaver {
                match leaver_policy {
                    ThumbnailScopeLeaverPolicy::Cancel if status == "pending" => {
                        updated += transaction.execute(
                            r#"
                            UPDATE tasks
                            SET status = 'cancelled',
                                updated_at = ?1
                            WHERE id = ?2
                              AND kind = 'thumbnail'
                              AND status = 'pending'
                            "#,
                            (now, task_id),
                        )?;
                        cancelled_leaver_count += 1;
                        continue;
                    }
                    ThumbnailScopeLeaverPolicy::Cancel => {
                        // Running: let it finish (bounded, self-completing);
                        // demote so it doesn't hold an elevated slot afterward.
                    }
                    ThumbnailScopeLeaverPolicy::DemoteToBulk => {}
                    ThumbnailScopeLeaverPolicy::DemoteToBackground => unreachable!(),
                }
                if current_priority != THUMBNAIL_BULK_PRIORITY {
                    updated += transaction.execute(
                        r#"
                        UPDATE tasks
                        SET priority = ?1,
                            updated_at = ?2
                        WHERE id = ?3
                          AND kind = 'thumbnail'
                          AND status IN ('pending', 'running')
                        "#,
                        (THUMBNAIL_BULK_PRIORITY, now, task_id),
                    )?;
                }
                continue;
            }

            let next_priority = thumbnail_scope_priority_for_file(
                file_id,
                &selected_file_ids,
                &visible_file_ids,
                &ahead_file_ids,
            );
            if current_priority == next_priority {
                continue;
            }
            updated += transaction.execute(
                r#"
                UPDATE tasks
                SET priority = ?1,
                    updated_at = ?2
                WHERE id = ?3
                  AND kind = 'thumbnail'
                  AND status IN ('pending', 'running')
                "#,
                (next_priority, now, task_id),
            )?;
            if status == "pending" && next_priority > current_priority {
                promoted_pending_task_ids.push(task_id);
            }
        }
        transaction.commit()?;
        if cancelled_leaver_count > 0 {
            debug_log!(
                "priority-scope: cancelled {} out-of-scope pending leaver task(s)",
                cancelled_leaver_count
            );
        }
        Ok(ThumbnailPriorityScopeSyncResult {
            updated,
            promoted_pending_task_ids,
        })
    }

    pub fn list_folder_children_page(
        &self,
        folder_id: i64,
        limit: i64,
        cursor: Option<String>,
        include_descendants: bool,
    ) -> anyhow::Result<Page<FolderRecord>> {
        let limit = limit.clamp(1, 500);
        let cursor = cursor.as_deref().map(decode_folder_cursor).transpose()?;
        let cursor_clause = if cursor.is_some() {
            "AND (folders.name > ?2 OR (folders.name = ?2 AND folders.id > ?3))"
        } else {
            ""
        };
        let sql = if include_descendants {
            format!(
                r#"
                WITH RECURSIVE descendant_folders(id) AS (
                    SELECT id
                    FROM folders
                    WHERE parent_id = ?1 AND status = 'active'
                    UNION ALL
                    SELECT folders.id
                    FROM folders
                    JOIN descendant_folders ON folders.parent_id = descendant_folders.id
                    WHERE folders.status = 'active'
                )
                SELECT folders.id, folders.root_id, folders.parent_id, folders.name, folders.status,
                    EXISTS(
                        SELECT 1 FROM folders child
                        WHERE child.parent_id = folders.id AND child.status = 'active'
                    ) AS has_children
                FROM folders
                JOIN descendant_folders ON descendant_folders.id = folders.id
                JOIN roots ON roots.id = folders.root_id AND roots.enabled = 1
                WHERE folders.status = 'active'
                  {cursor_clause}
                ORDER BY folders.name ASC, folders.id ASC
                LIMIT ?4
                "#,
                cursor_clause = cursor_clause,
            )
        } else {
            format!(
                r#"
                SELECT folders.id, folders.root_id, folders.parent_id, folders.name, folders.status,
                    EXISTS(
                        SELECT 1 FROM folders child
                        WHERE child.parent_id = folders.id AND child.status = 'active'
                    ) AS has_children
                FROM folders
                JOIN roots ON roots.id = folders.root_id AND roots.enabled = 1
                WHERE folders.parent_id = ?1 AND folders.status = 'active'
                  {cursor_clause}
                ORDER BY folders.name ASC, folders.id ASC
                LIMIT ?4
                "#,
                cursor_clause = cursor_clause,
            )
        };
        let mut parameters = vec![Value::Integer(folder_id)];
        if let Some(cursor) = &cursor {
            parameters.push(Value::Text(cursor.name.clone()));
            parameters.push(Value::Integer(cursor.id));
        } else {
            parameters.push(Value::Null);
            parameters.push(Value::Null);
        }
        parameters.push(Value::Integer(limit + 1));
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(parameters), |row| {
            Ok(FolderRecord {
                id: row.get(0)?,
                root_id: row.get(1)?,
                parent_id: row.get(2)?,
                name: row.get(3)?,
                status: row.get(4)?,
                has_children: row.get(5)?,
                media_count: 0,
            })
        })?;

        let mut folders = Vec::new();
        for row in rows {
            folders.push(row?);
        }
        drop(statement);
        let next_cursor = if folders.len() > limit as usize {
            folders.pop();
            folders.last().map(encode_folder_cursor)
        } else {
            None
        };
        let media_counts = self.folder_subtree_media_counts(folder_id)?;
        for folder in &mut folders {
            folder.media_count = media_counts.get(&folder.id).copied().unwrap_or(0);
        }
        Ok(Page {
            items: folders,
            next_cursor,
            total_count: None,
        })
    }

    /// Returns the active folder path in root-to-target order. The recursive
    /// walk is bounded and carries visited ids so a corrupt parent chain
    /// cannot run indefinitely; malformed chains are returned as errors for
    /// the API layer to expose as a server error rather than a false 404.
    pub fn list_folder_ancestors(
        &self,
        folder_id: i64,
    ) -> anyhow::Result<Option<Vec<FolderRecord>>> {
        let mut statement = self.connection.prepare(
            r#"
            WITH RECURSIVE ancestors(
                id, root_id, parent_id, name, status, depth, visited
            ) AS (
                SELECT folders.id, folders.root_id, folders.parent_id, folders.name,
                       folders.status, 0, ',' || folders.id || ','
                FROM folders
                JOIN roots ON roots.id = folders.root_id AND roots.enabled = 1
                WHERE folders.id = ?1 AND folders.status = 'active'
                UNION ALL
                SELECT parent.id, parent.root_id, parent.parent_id, parent.name,
                       parent.status, ancestors.depth + 1,
                       ancestors.visited || parent.id || ','
                FROM ancestors
                JOIN folders parent
                  ON parent.id = ancestors.parent_id
                 AND parent.root_id = ancestors.root_id
                 AND parent.status = 'active'
                WHERE ancestors.depth + 1 < ?2
                  AND instr(ancestors.visited, ',' || parent.id || ',') = 0
            )
            SELECT ancestors.id, ancestors.root_id, ancestors.parent_id,
                   ancestors.name, ancestors.status,
                   EXISTS(
                       SELECT 1 FROM folders child
                       WHERE child.parent_id = ancestors.id
                         AND child.status = 'active'
                   ) AS has_children
            FROM ancestors
            ORDER BY ancestors.depth DESC
            "#,
        )?;
        let rows = statement.query_map((folder_id, FOLDER_ANCESTOR_DEPTH_CAP), |row| {
            Ok(FolderRecord {
                id: row.get(0)?,
                root_id: row.get(1)?,
                parent_id: row.get(2)?,
                name: row.get(3)?,
                status: row.get(4)?,
                has_children: row.get(5)?,
                media_count: 0,
            })
        })?;

        let mut ancestors = Vec::new();
        for row in rows {
            ancestors.push(row?);
        }
        drop(statement);
        let Some(first) = ancestors.first() else {
            return Ok(None);
        };
        if first.parent_id.is_some() {
            anyhow::bail!(
                "invalid folder ancestor chain for {folder_id}: cycle, missing parent, or depth cap exceeded"
            );
        }

        for folder in &mut ancestors {
            folder.media_count = self
                .folder_subtree_media_counts(folder.id)?
                .get(&folder.id)
                .copied()
                .unwrap_or(0);
        }
        Ok(Some(ancestors))
    }

    #[cfg(test)]
    pub fn list_folder_children(&self, folder_id: i64) -> anyhow::Result<Vec<FolderRecord>> {
        Ok(self
            .list_folder_children_page(folder_id, 500, None, false)?
            .items)
    }

    fn folder_subtree_media_counts(&self, folder_id: i64) -> anyhow::Result<HashMap<i64, i64>> {
        let mut statement = self.connection.prepare(
            r#"
            WITH RECURSIVE folder_scope(id, parent_id, depth) AS (
                SELECT folders.id, folders.parent_id, 0
                FROM folders
                JOIN roots ON roots.id = folders.root_id AND roots.enabled = 1
                WHERE folders.id = ?1 AND folders.status = 'active'
                UNION ALL
                SELECT child.id, child.parent_id, folder_scope.depth + 1
                FROM folder_scope
                JOIN folders child
                  ON child.parent_id = folder_scope.id
                 AND child.status = 'active'
            )
            SELECT folder_scope.id, folder_scope.parent_id, folder_scope.depth,
                   COUNT(files.id) AS direct_media_count
            FROM folder_scope
            LEFT JOIN files
              ON files.folder_id = folder_scope.id
             AND files.status = 'active'
            GROUP BY folder_scope.id, folder_scope.parent_id, folder_scope.depth
            ORDER BY folder_scope.depth DESC
            "#,
        )?;
        let rows = statement.query_map([folder_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;

        let mut media_counts = HashMap::new();
        for row in rows {
            let (id, parent_id, depth, direct_media_count) = row?;
            let total = direct_media_count + media_counts.get(&id).copied().unwrap_or(0);
            media_counts.insert(id, total);
            if depth > 0 {
                if let Some(parent_id) = parent_id {
                    *media_counts.entry(parent_id).or_insert(0) += total;
                }
            }
        }
        Ok(media_counts)
    }

    pub(crate) fn get_folder_id_by_path(
        &self,
        root_id: i64,
        root_path: &Path,
        folder_path: &Path,
    ) -> anyhow::Result<Option<i64>> {
        let root = self
            .get_root(root_id)?
            .ok_or_else(|| anyhow::anyhow!("root not found: {root_id}"))?;
        let Some(mut folder_id) = root.root_folder_id else {
            return Ok(None);
        };
        let Ok(relative) = folder_path.strip_prefix(root_path) else {
            return Ok(None);
        };
        if relative.as_os_str().is_empty() {
            return Ok(Some(folder_id));
        }

        for component in relative.components() {
            let name = component.as_os_str().to_string_lossy();
            let Some(next_id) = self.find_folder_id(root_id, Some(folder_id), &name)? else {
                return Ok(None);
            };
            folder_id = next_id;
        }

        Ok(Some(folder_id))
    }

    pub(crate) fn ensure_folder_chain_for_path(
        &self,
        root_id: i64,
        root_path: &Path,
        folder_path: &Path,
    ) -> anyhow::Result<i64> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        ensure_root_enabled_in_transaction(&transaction, root_id)?;
        let folder_id = ensure_folder_chain_for_path_in_transaction(
            &transaction,
            root_id,
            root_path,
            folder_path,
        )?;
        transaction.commit()?;
        Ok(folder_id)
    }

    pub(crate) fn ensure_folder_chain_for_path_for_task_attempt(
        &self,
        root_id: i64,
        root_path: &Path,
        folder_path: &Path,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<i64> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        ensure_task_attempt_running_in_transaction(&transaction, task_id, attempt_generation)?;
        ensure_root_enabled_in_transaction(&transaction, root_id)?;
        let folder_id = ensure_folder_chain_for_path_in_transaction(
            &transaction,
            root_id,
            root_path,
            folder_path,
        )?;
        transaction.commit()?;
        Ok(folder_id)
    }

    pub(crate) fn begin_root_scan_reconciliation_for_task_attempt(
        &self,
        root_id: i64,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<i64> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        ensure_task_attempt_running_in_transaction(&transaction, task_id, attempt_generation)?;
        ensure_root_enabled_in_transaction(&transaction, root_id)?;
        let scan_generation = next_root_scan_generation_in_transaction(&transaction, root_id)?;
        transaction.execute(
            "UPDATE roots SET active_scan_generation = ?1 WHERE id = ?2",
            (scan_generation, root_id),
        )?;
        transaction.commit()?;
        Ok(scan_generation)
    }

    pub(crate) fn mark_file_missing_by_path_for_root_token(
        &self,
        token: &SyncRootBindingToken,
        root_id: i64,
        root_path: &Path,
        file_path: &Path,
    ) -> anyhow::Result<bool> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        Self::require_sync_root_binding_token_in_transaction(&transaction, token)?;
        let Some(parent_path) = file_path.parent() else {
            transaction.commit()?;
            return Ok(false);
        };
        let Some(folder_id) =
            get_folder_id_by_path_in_transaction(&transaction, root_id, root_path, parent_path)?
        else {
            transaction.commit()?;
            return Ok(false);
        };
        let file_name = file_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let updated = transaction.execute(
            "UPDATE files SET status = 'missing' WHERE root_id = ?1 AND folder_id = ?2 AND name = ?3 AND status = 'active'",
            (root_id, folder_id, file_name),
        )?;
        if updated != 0 {
            cleanup_inactive_thumbnail_artifacts_in_transaction(&transaction)?;
        }
        transaction.commit()?;
        Ok(updated != 0)
    }

    pub(crate) fn mark_folder_subtree_missing_by_path_for_root_token(
        &self,
        token: &SyncRootBindingToken,
        root_id: i64,
        root_path: &Path,
        folder_path: &Path,
    ) -> anyhow::Result<bool> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        Self::require_sync_root_binding_token_in_transaction(&transaction, token)?;
        let Some(folder_id) =
            get_folder_id_by_path_in_transaction(&transaction, root_id, root_path, folder_path)?
        else {
            transaction.commit()?;
            return Ok(false);
        };
        transaction.execute(
            r#"
            WITH RECURSIVE subtree(id) AS (
              SELECT id FROM folders WHERE id = ?1
              UNION ALL
              SELECT folders.id FROM folders JOIN subtree ON folders.parent_id = subtree.id
            )
            UPDATE folders SET status = 'missing' WHERE id IN (SELECT id FROM subtree)
            "#,
            [folder_id],
        )?;
        transaction.execute(
            r#"
            WITH RECURSIVE subtree(id) AS (
              SELECT id FROM folders WHERE id = ?1
              UNION ALL
              SELECT folders.id FROM folders JOIN subtree ON folders.parent_id = subtree.id
            )
            UPDATE files SET status = 'missing' WHERE folder_id IN (SELECT id FROM subtree)
            "#,
            [folder_id],
        )?;
        cleanup_inactive_thumbnail_artifacts_in_transaction(&transaction)?;
        transaction.commit()?;
        Ok(true)
    }

    pub(crate) fn upsert_external_media_file_for_root_token(
        &self,
        token: &SyncRootBindingToken,
        file: FileUpsert,
        media_kind: &str,
        volume_identity: &[u8],
        file_identity: &[u8],
        fingerprint: Option<&[u8]>,
        path: &Path,
    ) -> anyhow::Result<i64> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        Self::require_sync_root_binding_token_in_transaction(&transaction, token)?;
        let scan_generation = active_scan_generation_in_transaction(&transaction, file.root_id)?;
        let id = Self::upsert_external_media_file_in_transaction(
            &transaction,
            file,
            media_kind,
            volume_identity,
            file_identity,
            fingerprint,
            path,
            scan_generation,
            crate::sync::capture::MutationOrigin::ExternalFs,
        )?;
        transaction.commit()?;
        Ok(id)
    }

    pub(crate) fn ensure_folder_chain_for_path_for_root_token(
        &self,
        token: &SyncRootBindingToken,
        root_id: i64,
        root_path: &Path,
        folder_path: &Path,
    ) -> anyhow::Result<i64> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        Self::require_sync_root_binding_token_in_transaction(&transaction, token)?;
        ensure_root_enabled_in_transaction(&transaction, root_id)?;
        let folder_id = ensure_folder_chain_for_path_in_transaction(
            &transaction,
            root_id,
            root_path,
            folder_path,
        )?;
        transaction.commit()?;
        Ok(folder_id)
    }

    pub(crate) fn root_binding_token_for_task(
        &self,
        task_id: i64,
    ) -> anyhow::Result<Option<SyncRootBindingToken>> {
        self.connection
            .query_row(
                "SELECT root_uuid,local_binding_uuid,binding_generation FROM tasks WHERE id=?1",
                [task_id],
                |row| {
                    Ok((
                        row.get::<_, Option<Vec<u8>>>(0)?,
                        row.get::<_, Option<Vec<u8>>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .optional()?
            .map(|(root_uuid, local_binding_uuid, binding_generation)| {
                match (root_uuid, local_binding_uuid, binding_generation) {
                    (None, None, None) => Ok(None),
                    (Some(root_uuid), Some(local_binding_uuid), Some(binding_generation)) => {
                        Ok(Some(SyncRootBindingToken {
                            root_uuid: root_uuid.try_into().map_err(|_| {
                                anyhow::anyhow!("task root UUID must contain 16 bytes")
                            })?,
                            local_binding_uuid: local_binding_uuid.try_into().map_err(|_| {
                                anyhow::anyhow!("task binding UUID must contain 16 bytes")
                            })?,
                            binding_generation: u64::try_from(binding_generation)?,
                        }))
                    }
                    _ => anyhow::bail!("task root ownership is partial"),
                }
            })
            .transpose()
            .map(Option::flatten)
    }

    pub(crate) fn mark_file_missing_by_path(
        &self,
        root_id: i64,
        root_path: &Path,
        file_path: &Path,
    ) -> anyhow::Result<bool> {
        let Some(parent_path) = file_path.parent() else {
            return Ok(false);
        };
        let Some(folder_id) = self.get_folder_id_by_path(root_id, root_path, parent_path)? else {
            return Ok(false);
        };
        let file_name = file_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let transaction = self.connection.unchecked_transaction()?;
        let updated = transaction.execute(
            "UPDATE files SET status = 'missing' WHERE root_id = ?1 AND folder_id = ?2 AND name = ?3 AND status = 'active'",
            (root_id, folder_id, file_name),
        )?;
        if updated != 0 {
            cleanup_inactive_thumbnail_artifacts_in_transaction(&transaction)?;
        }
        transaction.commit()?;
        Ok(updated != 0)
    }

    /// Production watcher/scan reconciliation boundary. The stable platform
    /// file identity is primary; a content fingerprint is only used when one
    /// old missing candidate is unambiguous. This is local truth and never
    /// emits an outbox event by itself.
    pub(crate) fn mark_folder_subtree_missing_by_path(
        &self,
        root_id: i64,
        root_path: &Path,
        folder_path: &Path,
    ) -> anyhow::Result<bool> {
        let Some(folder_id) = self.get_folder_id_by_path(root_id, root_path, folder_path)? else {
            return Ok(false);
        };
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            r#"
            WITH RECURSIVE subtree(id) AS (
              SELECT id FROM folders WHERE id = ?1
              UNION ALL
              SELECT folders.id
              FROM folders
              JOIN subtree ON folders.parent_id = subtree.id
            )
            UPDATE folders
            SET status = 'missing'
            WHERE id IN (SELECT id FROM subtree)
            "#,
            [folder_id],
        )?;
        transaction.execute(
            r#"
            WITH RECURSIVE subtree(id) AS (
              SELECT id FROM folders WHERE id = ?1
              UNION ALL
              SELECT folders.id
              FROM folders
              JOIN subtree ON folders.parent_id = subtree.id
            )
            UPDATE files
            SET status = 'missing'
            WHERE folder_id IN (SELECT id FROM subtree)
            "#,
            [folder_id],
        )?;
        cleanup_inactive_thumbnail_artifacts_in_transaction(&transaction)?;
        transaction.commit()?;
        Ok(true)
    }

    #[allow(dead_code)]
    pub(crate) fn has_active_root_scan_task(&self, root_id: i64) -> anyhow::Result<bool> {
        let exists: i64 = self.connection.query_row(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM tasks
              WHERE kind = 'root_scan'
                AND root_id = ?1
                AND status IN ('pending', 'running')
            )
            "#,
            [root_id],
            |row| row.get(0),
        )?;
        Ok(exists != 0)
    }

    /// Returns true if there is already a `pending` root_scan task for the
    /// given root. Unlike `has_active_root_scan_task`, this ignores `running`
    /// tasks so the watcher can persist exactly one follow-up rescan to cover
    /// directory create / move-in events that happened after WalkDir already
    /// passed the affected path during a still-running scan.
    pub(crate) fn has_pending_root_scan_task(&self, root_id: i64) -> anyhow::Result<bool> {
        let exists: i64 = self.connection.query_row(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM tasks
              WHERE kind = 'root_scan'
                AND root_id = ?1
                AND status = 'pending'
            )
            "#,
            [root_id],
            |row| row.get(0),
        )?;
        Ok(exists != 0)
    }

    pub(crate) fn begin_root_scan_reconciliation(&self, root_id: i64) -> anyhow::Result<i64> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        ensure_root_enabled_in_transaction(&transaction, root_id)?;
        let scan_generation = next_root_scan_generation_in_transaction(&transaction, root_id)?;
        transaction.execute(
            "UPDATE roots SET active_scan_generation = ?1 WHERE id = ?2",
            (scan_generation, root_id),
        )?;
        transaction.commit()?;
        Ok(scan_generation)
    }

    pub(crate) fn reconcile_root_scan_completion(
        &self,
        root_id: i64,
        scan_generation: i64,
    ) -> anyhow::Result<()> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        ensure_root_enabled_in_transaction(&transaction, root_id)?;
        let active_scan_generation = active_scan_generation_in_transaction(&transaction, root_id)?;
        if active_scan_generation != Some(scan_generation) {
            return Err(anyhow::anyhow!(
                "root scan generation mismatch for root {root_id}; expected active generation {}, current active generation {:?}",
                scan_generation,
                active_scan_generation
            ));
        }
        transaction.execute(
            r#"
            UPDATE files
            SET status = 'missing'
            WHERE root_id = ?1
              AND status = 'active'
              AND COALESCE(scan_seen_at, 0) != ?2
            "#,
            (root_id, scan_generation),
        )?;
        transaction.execute(
            r#"
            UPDATE folders
            SET status = 'missing'
            WHERE root_id = ?1
              AND parent_id IS NOT NULL
              AND status = 'active'
              AND COALESCE(scan_seen_at, 0) != ?2
            "#,
            (root_id, scan_generation),
        )?;
        transaction.execute(
            "UPDATE roots SET active_scan_generation = NULL WHERE id = ?1 AND active_scan_generation = ?2",
            (root_id, scan_generation),
        )?;
        cleanup_inactive_thumbnail_artifacts_in_transaction(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    /// Task-attempt-guarded counterpart to `reconcile_root_scan_completion`.
    ///
    /// Reconciliation runs only if the task attempt is still current and
    /// running. A stale or superseded attempt returns `Ok(false)` without
    /// reconciling rows or clearing `active_scan_generation`, so a retried
    /// attempt's generation is preserved for the next run.
    pub(crate) fn reconcile_root_scan_completion_for_task_attempt(
        &self,
        root_id: i64,
        scan_generation: i64,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<bool> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        if !task_attempt_running_in_transaction(&transaction, task_id, attempt_generation)? {
            transaction.commit()?;
            return Ok(false);
        }
        ensure_root_enabled_in_transaction(&transaction, root_id)?;
        let active_scan_generation = active_scan_generation_in_transaction(&transaction, root_id)?;
        if active_scan_generation != Some(scan_generation) {
            return Err(anyhow::anyhow!(
                "root scan generation mismatch for root {root_id}; expected active generation {}, current active generation {:?}",
                scan_generation,
                active_scan_generation
            ));
        }
        transaction.execute(
            r#"
            UPDATE files
            SET status = 'missing'
            WHERE root_id = ?1
              AND status = 'active'
              AND COALESCE(scan_seen_at, 0) != ?2
            "#,
            (root_id, scan_generation),
        )?;
        transaction.execute(
            r#"
            UPDATE folders
            SET status = 'missing'
            WHERE root_id = ?1
              AND parent_id IS NOT NULL
              AND status = 'active'
              AND COALESCE(scan_seen_at, 0) != ?2
            "#,
            (root_id, scan_generation),
        )?;
        transaction.execute(
            "UPDATE roots SET active_scan_generation = NULL WHERE id = ?1 AND active_scan_generation = ?2",
            (root_id, scan_generation),
        )?;
        cleanup_inactive_thumbnail_artifacts_in_transaction(&transaction)?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn folder_exists(&self, folder_id: i64) -> anyhow::Result<bool> {
        let exists: i64 = self.connection.query_row(
            r#"
            SELECT EXISTS(
              SELECT 1
              FROM folders
              JOIN roots ON roots.id = folders.root_id AND roots.enabled = 1
              WHERE folders.id = ?1 AND folders.status = 'active'
            )
            "#,
            [folder_id],
            |row| row.get(0),
        )?;
        Ok(exists != 0)
    }

    #[allow(dead_code)]
    pub fn upsert_folder(&self, folder: FolderUpsert) -> anyhow::Result<i64> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let scan_generation = active_scan_generation_in_transaction(&transaction, folder.root_id)?;
        let folder_id = upsert_folder_in_transaction(&transaction, folder, scan_generation)?;
        transaction.commit()?;
        Ok(folder_id)
    }

    #[allow(dead_code)]
    pub fn upsert_file(&self, file: FileUpsert) -> anyhow::Result<i64> {
        let root_id = file.root_id;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let scan_generation = active_scan_generation_in_transaction(&transaction, root_id)?;
        let file_id = upsert_file_in_transaction(
            &transaction,
            file,
            scan_generation,
            crate::sync::capture::MutationOrigin::ExternalFs,
            None,
        )?
        .id;
        transaction.commit()?;
        Ok(file_id)
    }

    pub(crate) fn upsert_external_media_file(
        &self,
        file: FileUpsert,
        media_kind: &str,
        volume_identity: &[u8],
        file_identity: &[u8],
        fingerprint: Option<&[u8]>,
        path: &Path,
    ) -> anyhow::Result<i64> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let scan_generation = active_scan_generation_in_transaction(&transaction, file.root_id)?;
        let id = Self::upsert_external_media_file_in_transaction(
            &transaction,
            file,
            media_kind,
            volume_identity,
            file_identity,
            fingerprint,
            path,
            scan_generation,
            crate::sync::capture::MutationOrigin::ExternalFs,
        )?;
        transaction.commit()?;
        Ok(id)
    }

    fn upsert_external_media_file_in_transaction(
        transaction: &rusqlite::Transaction<'_>,
        file: FileUpsert,
        media_kind: &str,
        volume_identity: &[u8],
        file_identity: &[u8],
        fingerprint: Option<&[u8]>,
        path: &Path,
        scan_generation: Option<i64>,
        origin: crate::sync::capture::MutationOrigin,
    ) -> anyhow::Result<i64> {
        // Resolve only against pre-existing missing identities. The provisional
        // browse row does not exist yet, so it cannot self-match or overwrite
        // the old identity before the decision is made.
        // Failure to obtain an OS file ID must not turn the empty byte string
        // into a shared identity for every file on the volume. In that case a
        // path-scoped key keeps the fingerprint history durable without ever
        // producing a false exact-identity match after a rename.
        let has_stable_file_identity = !file_identity.is_empty();
        let persisted_file_identity = has_stable_file_identity
            .then(|| file_identity.to_vec())
            .unwrap_or_else(|| path.to_string_lossy().as_bytes().to_vec());
        let exact: Option<i64> = if has_stable_file_identity {
            transaction.query_row(
                "SELECT f.id FROM sync_local_file_identities i JOIN files f ON f.asset_uuid=i.asset_uuid WHERE i.volume_identity=?1 AND i.file_identity=?2 AND (f.status='missing' OR (?3 IS NOT NULL AND COALESCE(f.scan_seen_at,-1)<>?3)) LIMIT 1",
                rusqlite::params![volume_identity,file_identity,scan_generation], |row| row.get(0),
            ).optional()?
        } else {
            None
        };
        let fingerprint_candidates = if exact.is_none() {
            if let Some(fingerprint) = fingerprint {
                let mut statement = transaction.prepare("SELECT f.id FROM sync_local_file_identities i JOIN files f ON f.asset_uuid=i.asset_uuid WHERE i.content_fingerprint=?1 AND (f.status='missing' OR (?2 IS NOT NULL AND COALESCE(f.scan_seen_at,-1)<>?2)) ORDER BY f.asset_uuid")?;
                let candidates = statement
                    .query_map(rusqlite::params![fingerprint, scan_generation], |row| {
                        row.get::<_, i64>(0)
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                candidates
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let forced_reuse = exact
            .or_else(|| (fingerprint_candidates.len() == 1).then(|| fingerprint_candidates[0]));
        let previous_asset: Option<Vec<u8>> = if let Some(file_id) = forced_reuse {
            transaction
                .query_row(
                    "SELECT asset_uuid FROM files WHERE id=?1",
                    [file_id],
                    |row| row.get(0),
                )
                .optional()?
        } else {
            transaction
                .query_row(
                    "SELECT asset_uuid FROM files WHERE folder_id=?1 AND name=?2",
                    rusqlite::params![file.folder_id, file.name],
                    |row| row.get(0),
                )
                .optional()?
        };
        let previous_fingerprint = if let Some(asset_uuid) = previous_asset {
            transaction
                .query_row(
                    "SELECT content_fingerprint FROM sync_local_file_identities WHERE asset_uuid=?1",
                    [asset_uuid],
                    |row| row.get::<_, Option<Vec<u8>>>(0),
                )
                .optional()?
                .flatten()
        } else {
            None
        };
        let source_size = file.size;
        let source_mtime_ms = file.mtime.saturating_mul(1000);
        let result =
            upsert_file_in_transaction(transaction, file, scan_generation, origin, forced_reuse)?;
        upsert_media_kind_in_transaction(
            transaction,
            result.id,
            media_kind,
            result.identity_changed,
        )?;
        let asset: Vec<u8> = transaction.query_row(
            "SELECT asset_uuid FROM files WHERE id=?1",
            [result.id],
            |row| row.get(0),
        )?;
        let asset = uuid::Uuid::from_slice(&asset)?;
        let content_changed = fingerprint.map_or(result.identity_changed, |value| {
            previous_fingerprint.as_deref() != Some(value)
        });
        if fingerprint_candidates.len() > 1 {
            for old_id in fingerprint_candidates {
                let old: Vec<u8> = transaction.query_row(
                    "SELECT asset_uuid FROM files WHERE id=?1",
                    [old_id],
                    |row| row.get(0),
                )?;
                transaction.execute("INSERT INTO sync_identity_conflicts(conflict_uuid,old_asset_uuid,new_asset_uuid,reason,state,created_at) VALUES (?1,?2,?3,'ambiguous-fingerprint','open',?4)",rusqlite::params![uuid::Uuid::now_v7().as_bytes().as_slice(),old,asset.as_bytes().as_slice(),unix_timestamp_millis()])?;
            }
        }
        crate::sync::identity::record_identity(
            transaction,
            &crate::sync::capture::LocalFileIdentity {
                asset_uuid: asset,
                volume_identity: volume_identity.to_vec(),
                file_identity: persisted_file_identity,
                content_fingerprint: fingerprint.map(ToOwned::to_owned),
            },
            &path.to_string_lossy(),
        )?;
        let sync_initialized = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_local_state LIMIT 1)",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if content_changed
            && origin == crate::sync::capture::MutationOrigin::ExternalFs
            && sync_initialized
        {
            create_sync_publication_intent_in_transaction(
                transaction,
                result.id,
                &path.to_string_lossy(),
                source_size,
                Some(source_mtime_ms),
            )?;
        }
        Ok(result.id)
    }

    #[allow(dead_code)]
    pub fn upsert_media_kind(&self, file_id: i64, kind: &str) -> anyhow::Result<()> {
        self.connection.execute(
            r#"
            INSERT INTO media(file_id, kind, metadata_status)
            VALUES (?1, ?2, 'pending')
            ON CONFLICT(file_id) DO UPDATE SET
              kind = excluded.kind,
              width = NULL,
              height = NULL,
              duration_ms = NULL,
              codec = NULL,
              orientation = NULL,
              has_alpha = NULL,
              dominant_color = NULL,
              phash = NULL,
              preview_placeholder = NULL,
              preview_placeholder_format = 'image/webp',
              metadata_status = 'pending'
            "#,
            (file_id, kind),
        )?;
        Ok(())
    }

    /// Records image header dimensions and marks media metadata `ready`.
    /// Used by metadata/preview work outside the root scan ingest path. Leaves
    /// pixel-level metadata (duration_ms, codec) untouched so a later metadata
    /// task can still fill those in.
    #[allow(dead_code)]
    pub fn update_media_dimensions(
        &self,
        file_id: i64,
        width: i64,
        height: i64,
    ) -> anyhow::Result<bool> {
        let updated = self.connection.execute(
            r#"
            UPDATE media
            SET width = ?1,
                height = ?2,
                metadata_status = 'ready'
            WHERE file_id = ?3
            "#,
            (width, height, file_id),
        )?;
        Ok(updated != 0)
    }

    #[allow(dead_code)]
    pub fn update_media_preview_placeholder(
        &self,
        file_id: i64,
        placeholder: &[u8],
        format: &str,
    ) -> anyhow::Result<bool> {
        let updated = self.connection.execute(
            r#"
            UPDATE media
            SET preview_placeholder = ?1,
                preview_placeholder_format = ?2
            WHERE file_id = ?3
            "#,
            (placeholder, format, file_id),
        )?;
        Ok(updated != 0)
    }

    /// Reconstructs the absolute filesystem path for an active media file by
    /// walking `folders.parent_id` back to the root and prepending `roots.path`.
    /// Returns `None` if the file no longer exists, has been marked missing,
    /// or its root has been disabled. Used by the thumbnail worker to find the
    /// source bytes without holding a separate copy of the path on disk.
    pub fn resolve_file_source_path(&self, file_id: i64) -> anyhow::Result<Option<PathBuf>> {
        // Hard cap on the parent walk to defend against a corrupt DB (a
        // self-parent or a cycle). 4096 is well above any realistic folder
        // tree depth on Windows or POSIX while still bounding the worker
        // round-trip in pathological cases.
        const FOLDER_WALK_DEPTH_CAP: usize = 4096;

        let mut statement = self.connection.prepare(
            r#"
            SELECT roots.path, files.folder_id, files.name
            FROM files
            JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
            WHERE files.id = ?1 AND files.status = 'active'
            "#,
        )?;
        let mut rows = statement.query([file_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let root_path: String = row.get(0)?;
        let mut folder_id: i64 = row.get(1)?;
        let file_name: String = row.get(2)?;

        let mut segments: Vec<String> = Vec::new();
        let mut folder_statement = self
            .connection
            .prepare("SELECT name, parent_id FROM folders WHERE id = ?1")?;
        let mut depth = 0usize;
        loop {
            if depth >= FOLDER_WALK_DEPTH_CAP {
                tracing::warn!(
                    file_id,
                    depth_cap = FOLDER_WALK_DEPTH_CAP,
                    "resolve_file_source_path exceeded folder depth cap; treating file as unresolved"
                );
                return Ok(None);
            }
            depth += 1;
            let mut folder_rows = folder_statement.query([folder_id])?;
            let Some(folder_row) = folder_rows.next()? else {
                return Ok(None);
            };
            let name: String = folder_row.get(0)?;
            let parent_id: Option<i64> = folder_row.get(1)?;
            if !name.is_empty() {
                segments.push(name);
            }
            match parent_id {
                Some(id) => folder_id = id,
                None => break,
            }
        }
        segments.reverse();

        let mut path = PathBuf::from(root_path);
        for segment in segments {
            path.push(segment);
        }
        path.push(file_name);
        Ok(Some(path))
    }

    pub(crate) fn resolve_file_source_path_with_root_token(
        &self,
        file_id: i64,
    ) -> anyhow::Result<Option<(PathBuf, Option<SyncRootBindingToken>)>> {
        let Some(path) = self.resolve_file_source_path(file_id)? else {
            return Ok(None);
        };
        Ok(Some((
            path,
            self.sync_root_binding_token_for_file(file_id)?,
        )))
    }

    pub(crate) fn resolve_asset_source_path_with_root_token(
        &self,
        asset_uuid: uuid::Uuid,
    ) -> anyhow::Result<Option<(PathBuf, Option<SyncRootBindingToken>)>> {
        let file_id = self
            .connection
            .query_row(
                "SELECT id FROM files WHERE asset_uuid=?1 AND status='active'",
                [asset_uuid.as_bytes().as_slice()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        file_id
            .map(|file_id| self.resolve_file_source_path_with_root_token(file_id))
            .transpose()
            .map(Option::flatten)
    }

    pub fn resolve_folder_source_path(&self, folder_id: i64) -> anyhow::Result<Option<PathBuf>> {
        const FOLDER_WALK_DEPTH_CAP: usize = 4096;

        let mut statement = self.connection.prepare(
            r#"
            SELECT roots.path
            FROM folders
            JOIN roots ON roots.id = folders.root_id AND roots.enabled = 1
            WHERE folders.id = ?1 AND folders.status = 'active'
            "#,
        )?;
        let mut rows = statement.query([folder_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let root_path: String = row.get(0)?;

        let mut current_folder_id = folder_id;
        let mut segments: Vec<String> = Vec::new();
        let mut folder_statement = self
            .connection
            .prepare("SELECT name, parent_id FROM folders WHERE id = ?1")?;
        let mut depth = 0usize;
        loop {
            if depth >= FOLDER_WALK_DEPTH_CAP {
                tracing::warn!(
                    folder_id,
                    depth_cap = FOLDER_WALK_DEPTH_CAP,
                    "resolve_folder_source_path exceeded folder depth cap; treating folder as unresolved"
                );
                return Ok(None);
            }
            depth += 1;
            let mut folder_rows = folder_statement.query([current_folder_id])?;
            let Some(folder_row) = folder_rows.next()? else {
                return Ok(None);
            };
            let name: String = folder_row.get(0)?;
            let parent_id: Option<i64> = folder_row.get(1)?;
            if !name.is_empty() {
                segments.push(name);
            }
            match parent_id {
                Some(id) => current_folder_id = id,
                None => break,
            }
        }
        segments.reverse();

        let mut path = PathBuf::from(root_path);
        for segment in segments {
            path.push(segment);
        }
        Ok(Some(path))
    }

    pub fn commit_scan_batch(
        &mut self,
        batch: ScanWriteBatch,
    ) -> anyhow::Result<ScanWriteBatchResult> {
        // Use BEGIN IMMEDIATE rather than the default DEFERRED transaction
        // so the SQLite busy handler is engaged on transaction start. With
        // multiple writer connections (API, scan worker, watcher) on the
        // same WAL file, an in-flight DEFERRED transaction that later
        // escalates to a write can return SQLITE_BUSY without honoring
        // `busy_timeout`. Acquiring the write lock up front lets the busy
        // handler retry transparently and prevents transient "database is
        // locked" failures.
        //
        // A scan batch is a long background write (folders + files + media). It
        // runs on the single writer connection, so it never contends with other
        // writes; the writer actor checkpoints between batches.
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let root_ids = scan_batch_root_ids(&batch);
        for root_id in root_ids {
            ensure_root_enabled_in_transaction(&transaction, root_id)?;
        }
        let mut folder_ids = Vec::with_capacity(batch.folders.len());
        let mut file_ids = Vec::with_capacity(batch.files.len());

        let scan_generation = batch.scan_generation;

        for folder in batch.folders {
            folder_ids.push(upsert_folder_in_transaction(
                &transaction,
                folder,
                scan_generation,
            )?);
        }

        for file in batch.files {
            if let Some(identity) = file.external_identity {
                file_ids.push(Self::upsert_external_media_file_in_transaction(
                    &transaction,
                    file.file,
                    &file.media_kind,
                    &identity.volume_identity,
                    &identity.file_identity,
                    identity.content_fingerprint.as_deref(),
                    &identity.path,
                    scan_generation,
                    crate::sync::capture::MutationOrigin::ExternalFs,
                )?);
            } else {
                let result = upsert_file_in_transaction(
                    &transaction,
                    file.file,
                    scan_generation,
                    crate::sync::capture::MutationOrigin::ExternalFs,
                    None,
                )?;
                upsert_media_kind_in_transaction(
                    &transaction,
                    result.id,
                    &file.media_kind,
                    result.identity_changed,
                )?;
                file_ids.push(result.id);
            }
        }

        transaction.commit()?;
        Ok(ScanWriteBatchResult {
            folder_ids,
            file_ids,
        })
    }

    pub fn commit_scan_batch_for_task_attempt(
        &mut self,
        batch: ScanWriteBatch,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<ScanWriteBatchResult> {
        // Use BEGIN IMMEDIATE rather than the default DEFERRED transaction
        // so the SQLite busy handler is engaged on transaction start; see
        // `commit_scan_batch` for the full rationale.
        //
        // This is the batch-commit path taken by the real root-scan *task*
        // worker. It runs on the single writer connection, so its back-to-back
        // 256-row batches never contend with browsing writes; the writer actor
        // checkpoints between batches to keep the WAL bounded.
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        ensure_task_attempt_running_in_transaction(&transaction, task_id, attempt_generation)?;
        let root_ids = scan_batch_root_ids(&batch);
        for root_id in root_ids {
            ensure_root_enabled_in_transaction(&transaction, root_id)?;
        }
        let mut folder_ids = Vec::with_capacity(batch.folders.len());
        let mut file_ids = Vec::with_capacity(batch.files.len());

        let scan_generation = batch.scan_generation;

        for folder in batch.folders {
            folder_ids.push(upsert_folder_in_transaction(
                &transaction,
                folder,
                scan_generation,
            )?);
        }

        for file in batch.files {
            if let Some(identity) = file.external_identity {
                file_ids.push(Self::upsert_external_media_file_in_transaction(
                    &transaction,
                    file.file,
                    &file.media_kind,
                    &identity.volume_identity,
                    &identity.file_identity,
                    identity.content_fingerprint.as_deref(),
                    &identity.path,
                    scan_generation,
                    crate::sync::capture::MutationOrigin::ExternalFs,
                )?);
                continue;
            }
            let result = upsert_file_in_transaction(
                &transaction,
                file.file,
                scan_generation,
                crate::sync::capture::MutationOrigin::ExternalFs,
                None,
            )?;
            upsert_media_kind_in_transaction(
                &transaction,
                result.id,
                &file.media_kind,
                result.identity_changed,
            )?;
            file_ids.push(result.id);
        }

        transaction.commit()?;
        Ok(ScanWriteBatchResult {
            folder_ids,
            file_ids,
        })
    }

    #[allow(dead_code)]
    pub fn upsert_thumbnail_state(&self, state: ThumbnailStateUpsert) -> anyhow::Result<()> {
        upsert_thumbnail_state_in_connection(&self.connection, state)?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn upsert_thumbnail_state_if_source_fingerprint_current(
        &self,
        state: ThumbnailStateUpsert,
        expected_source_fingerprint: &str,
    ) -> anyhow::Result<bool> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let current_source = thumbnail_source_in_transaction(&transaction, state.file_id)?
            .ok_or_else(|| anyhow::anyhow!("media item not found: {}", state.file_id))?;
        if current_source.source_fingerprint(&state.profile) != expected_source_fingerprint {
            transaction.commit()?;
            return Ok(false);
        }
        upsert_thumbnail_state_in_transaction(&transaction, state)?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn upsert_thumbnail_state_if_source_fingerprint_and_task_attempt_current(
        &self,
        state: ThumbnailStateUpsert,
        expected_source_fingerprint: &str,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<bool> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        if !task_attempt_running_in_transaction(&transaction, task_id, attempt_generation)? {
            transaction.commit()?;
            return Ok(false);
        }
        let current_source = thumbnail_source_in_transaction(&transaction, state.file_id)?
            .ok_or_else(|| anyhow::anyhow!("media item not found: {}", state.file_id))?;
        if current_source.source_fingerprint(&state.profile) != expected_source_fingerprint {
            transaction.commit()?;
            return Ok(false);
        }
        upsert_thumbnail_state_in_transaction(&transaction, state)?;
        transaction.commit()?;
        Ok(true)
    }

    #[allow(dead_code)]
    pub fn upsert_thumb_blob(&self, blob: ThumbBlobRecord) -> anyhow::Result<()> {
        self.connection.execute(
            r#"
            INSERT INTO thumb_blobs(
                file_id, profile, data, width, height, byte_size, output_format, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(file_id, profile) DO UPDATE SET
                data = excluded.data,
                width = excluded.width,
                height = excluded.height,
                byte_size = excluded.byte_size,
                output_format = excluded.output_format,
                updated_at = excluded.updated_at
            "#,
            (
                blob.file_id,
                &blob.profile,
                &blob.data,
                blob.width,
                blob.height,
                blob.byte_size,
                &blob.output_format,
                blob.created_at,
                blob.updated_at,
            ),
        )?;
        Ok(())
    }

    pub fn publish_thumbnail_and_succeed_task_if_source_fingerprint_and_task_attempt_current(
        &self,
        state: ThumbnailStateUpsert,
        blob: ThumbBlobRecord,
        source_dimensions: Option<(i64, i64)>,
        expected_source_fingerprint: &str,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<bool> {
        if state.file_id != blob.file_id || state.profile != blob.profile {
            anyhow::bail!(
                "thumbnail blob does not match thumbnail state for file {} profile {}",
                state.file_id,
                state.profile
            );
        }
        let file_id = blob.file_id;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        if !task_attempt_running_in_transaction(&transaction, task_id, attempt_generation)? {
            transaction.commit()?;
            return Ok(false);
        }
        let current_source = thumbnail_source_in_transaction(&transaction, state.file_id)?
            .ok_or_else(|| anyhow::anyhow!("media item not found: {}", state.file_id))?;
        if current_source.source_fingerprint(&state.profile) != expected_source_fingerprint {
            transaction.commit()?;
            return Ok(false);
        }
        upsert_thumb_blob_in_transaction(&transaction, &blob)?;
        upsert_thumbnail_state_in_transaction(&transaction, state)?;
        // Persist the source dimensions captured during the decode the thumbnail
        // pipeline already performed, moving `media.width/height` from the
        // scan's `pending` placeholder to real values in the SAME transaction —
        // no second decode pass and no extra commit. The layout engine reads
        // these to give justified-rows / masonry their true aspect ratios.
        if let Some((source_width, source_height)) = source_dimensions {
            if source_width > 0 && source_height > 0 {
                transaction.execute(
                    "UPDATE media SET width = ?1, height = ?2, metadata_status = 'ready' WHERE file_id = ?3",
                    (source_width, source_height, file_id),
                )?;
            }
        }
        // Complete the task in the SAME transaction as the blob/state write.
        // The running-attempt guard above guarantees this row exists and is
        // running with this attempt, so the worker persists one thumbnail with a
        // single commit instead of a separate succeeded-UPDATE commit afterward.
        let succeeded = transaction.execute(
            "UPDATE tasks SET status = 'succeeded', updated_at = ?1, error = NULL WHERE id = ?2 AND status = 'running' AND attempt_generation = ?3",
            (unix_timestamp(), task_id, attempt_generation),
        )?;
        if succeeded != 1 {
            anyhow::bail!(
                "expected to complete exactly one running thumbnail task {task_id} (attempt {attempt_generation}), updated {succeeded}"
            );
        }
        transaction.commit()?;
        Ok(true)
    }

    pub fn get_thumb_blob(
        &self,
        file_id: i64,
        profile: &str,
    ) -> anyhow::Result<Option<ThumbBlobRecord>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT file_id, profile, data, width, height, byte_size, output_format, created_at, updated_at
            FROM thumb_blobs
            WHERE file_id = ?1 AND profile = ?2
            "#,
        )?;
        let mut rows = statement.query((file_id, profile))?;
        Ok(rows.next()?.map(thumb_blob_from_row).transpose()?)
    }

    pub fn get_preview_blob(
        &self,
        file_id: i64,
        profile: &str,
    ) -> anyhow::Result<Option<PreviewBlobRecord>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT file_id, profile, data, width, height, byte_size, output_format,
                   source_size, source_mtime, created_at, updated_at
            FROM preview_blobs
            WHERE file_id = ?1 AND profile = ?2
            "#,
        )?;
        let mut rows = statement.query((file_id, profile))?;
        Ok(rows.next()?.map(preview_blob_from_row).transpose()?)
    }

    pub fn get_preview_failure(
        &self,
        file_id: i64,
        profile: &str,
    ) -> anyhow::Result<Option<PreviewFailureRecord>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT file_id, profile, failure_kind, error, source_size, source_mtime,
                   created_at, updated_at
            FROM preview_failures
            WHERE file_id = ?1 AND profile = ?2
            "#,
        )?;
        let mut rows = statement.query((file_id, profile))?;
        Ok(rows.next()?.map(preview_failure_from_row).transpose()?)
    }

    #[allow(dead_code)]
    pub fn upsert_preview_failure(&self, failure: PreviewFailureRecord) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            r#"
            INSERT INTO preview_failures(
                file_id, profile, failure_kind, error, source_size, source_mtime,
                created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(file_id, profile) DO UPDATE SET
                failure_kind = excluded.failure_kind,
                error = excluded.error,
                source_size = excluded.source_size,
                source_mtime = excluded.source_mtime,
                updated_at = excluded.updated_at
            "#,
            (
                failure.file_id,
                &failure.profile,
                &failure.failure_kind,
                &failure.error,
                failure.source_size,
                failure.source_mtime,
                failure.created_at,
                failure.updated_at,
            ),
        )?;
        transaction.execute(
            "DELETE FROM preview_blobs WHERE file_id = ?1 AND profile = ?2",
            (failure.file_id, &failure.profile),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn upsert_preview_failure_for_root_token(
        &self,
        failure: PreviewFailureRecord,
        token: Option<&SyncRootBindingToken>,
    ) -> anyhow::Result<()> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        if let Some(token) = token {
            Self::require_sync_root_binding_token_in_transaction(&transaction, token)?;
        }
        transaction.execute(
            r#"
            INSERT INTO preview_failures(
                file_id, profile, failure_kind, error, source_size, source_mtime,
                created_at, updated_at
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
            ON CONFLICT(file_id, profile) DO UPDATE SET
                failure_kind=excluded.failure_kind,error=excluded.error,
                source_size=excluded.source_size,source_mtime=excluded.source_mtime,
                updated_at=excluded.updated_at
            "#,
            (
                failure.file_id,
                &failure.profile,
                &failure.failure_kind,
                &failure.error,
                failure.source_size,
                failure.source_mtime,
                failure.created_at,
                failure.updated_at,
            ),
        )?;
        transaction.execute(
            "DELETE FROM preview_blobs WHERE file_id=?1 AND profile=?2",
            (failure.file_id, &failure.profile),
        )?;
        transaction.commit()?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn upsert_preview_blob(&self, blob: PreviewBlobRecord) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            r#"
            INSERT INTO preview_blobs(
                file_id, profile, data, width, height, byte_size, output_format,
                source_size, source_mtime, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(file_id, profile) DO UPDATE SET
                data = excluded.data,
                width = excluded.width,
                height = excluded.height,
                byte_size = excluded.byte_size,
                output_format = excluded.output_format,
                source_size = excluded.source_size,
                source_mtime = excluded.source_mtime,
                updated_at = excluded.updated_at
            "#,
            (
                blob.file_id,
                &blob.profile,
                &blob.data,
                blob.width,
                blob.height,
                blob.byte_size,
                &blob.output_format,
                blob.source_size,
                blob.source_mtime,
                blob.created_at,
                blob.updated_at,
            ),
        )?;
        transaction.execute(
            "DELETE FROM preview_failures WHERE file_id = ?1 AND profile = ?2",
            (blob.file_id, &blob.profile),
        )?;
        // Evict least-recently-used tiers until the cache is inside budget:
        // walk newest-first and drop everything past the point where the
        // running total crosses the limit. The row just written is excluded
        // outright rather than relying on it sorting first — a caller-supplied
        // `updated_at` need not be the largest in the table, and a single tier
        // bigger than the whole budget would otherwise delete itself, making
        // the caller decode, store, and immediately miss.
        transaction.execute(
            r#"
            DELETE FROM preview_blobs
            WHERE rowid IN (
              SELECT rowid FROM (
                SELECT rowid,
                       SUM(byte_size) OVER (
                         ORDER BY updated_at DESC, rowid DESC
                         ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
                       ) AS running_bytes
                FROM preview_blobs
              )
              WHERE running_bytes > ?1
            )
              AND NOT (file_id = ?2 AND profile = ?3)
            "#,
            (PREVIEW_BLOB_CACHE_LIMIT_BYTES, blob.file_id, &blob.profile),
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn upsert_preview_blob_for_root_token(
        &self,
        blob: PreviewBlobRecord,
        token: Option<&SyncRootBindingToken>,
    ) -> anyhow::Result<()> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        if let Some(token) = token {
            Self::require_sync_root_binding_token_in_transaction(&transaction, token)?;
        }
        transaction.execute(
            r#"
            INSERT INTO preview_blobs(
                file_id, profile, data, width, height, byte_size, output_format,
                source_size, source_mtime, created_at, updated_at
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
            ON CONFLICT(file_id, profile) DO UPDATE SET
                data=excluded.data,width=excluded.width,height=excluded.height,
                byte_size=excluded.byte_size,output_format=excluded.output_format,
                source_size=excluded.source_size,source_mtime=excluded.source_mtime,
                updated_at=excluded.updated_at
            "#,
            (
                blob.file_id,
                &blob.profile,
                &blob.data,
                blob.width,
                blob.height,
                blob.byte_size,
                &blob.output_format,
                blob.source_size,
                blob.source_mtime,
                blob.created_at,
                blob.updated_at,
            ),
        )?;
        transaction.execute(
            "DELETE FROM preview_failures WHERE file_id=?1 AND profile=?2",
            (blob.file_id, &blob.profile),
        )?;
        transaction.execute(
            r#"DELETE FROM preview_blobs WHERE rowid IN (
                 SELECT rowid FROM (
                   SELECT rowid,SUM(byte_size) OVER (
                     ORDER BY updated_at DESC,rowid DESC
                     ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
                   ) AS running_bytes FROM preview_blobs
                 ) WHERE running_bytes>?1
               ) AND NOT (file_id=?2 AND profile=?3)"#,
            (PREVIEW_BLOB_CACHE_LIMIT_BYTES, blob.file_id, &blob.profile),
        )?;
        transaction.commit()?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn import_legacy_thumbnail_cache(&self) -> anyhow::Result<usize> {
        let cache_root = self.default_thumbnail_cache_dir();
        if !cache_root.is_dir() {
            return Ok(0);
        }
        let mut imported = 0usize;
        {
            let mut statement = self.connection.prepare(
                r#"
                SELECT file_id, profile, cache_key, width, height, byte_size, output_format
                FROM thumbs
                WHERE profile = 'grid_320'
                  AND state = 'ready'
                  AND cache_key IS NOT NULL
                  AND width IS NOT NULL
                  AND height IS NOT NULL
                  AND byte_size IS NOT NULL
                "#,
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?;
            for row in rows {
                let (file_id, profile, cache_key, width, height, _byte_size, output_format) = row?;
                if !is_safe_thumbnail_cache_key(&cache_key) {
                    invalidate_legacy_thumbnail_import_candidate(
                        &self.connection,
                        file_id,
                        &profile,
                        "unsafe legacy thumbnail cache key",
                    )?;
                    continue;
                }
                let cache_path = cache_root.join(&cache_key);
                let data = match std::fs::read(&cache_path) {
                    Ok(data) => data,
                    Err(error) => {
                        tracing::debug!(file_id, %error, "legacy thumbnail cache import skipped unreadable file");
                        invalidate_legacy_thumbnail_import_candidate(
                            &self.connection,
                            file_id,
                            &profile,
                            "legacy thumbnail cache file unreadable",
                        )?;
                        continue;
                    }
                };
                if !is_importable_webp(&data) {
                    tracing::debug!(
                        file_id,
                        "legacy thumbnail cache import skipped invalid WebP file"
                    );
                    invalidate_legacy_thumbnail_import_candidate(
                        &self.connection,
                        file_id,
                        &profile,
                        "legacy thumbnail cache file invalid",
                    )?;
                    continue;
                }
                let now = unix_timestamp();
                self.upsert_thumb_blob(ThumbBlobRecord {
                    file_id,
                    profile,
                    byte_size: data.len() as i64,
                    data,
                    width,
                    height,
                    output_format,
                    created_at: now,
                    updated_at: now,
                })?;
                imported += 1;
            }
        }
        cleanup_legacy_thumbnail_cache_tree(&cache_root);
        Ok(imported)
    }

    pub fn mark_root_scanned(&self, root_id: i64) -> anyhow::Result<()> {
        self.connection.execute(
            "UPDATE roots SET last_scan_at = ?1 WHERE id = ?2 AND enabled = 1",
            (unix_timestamp(), root_id),
        )?;
        Ok(())
    }

    pub fn mark_root_scanned_for_task_attempt(
        &self,
        root_id: i64,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        ensure_task_attempt_running_in_transaction(&transaction, task_id, attempt_generation)?;
        let updated = transaction.execute(
            "UPDATE roots SET last_scan_at = ?1 WHERE id = ?2 AND enabled = 1",
            (unix_timestamp(), root_id),
        )?;
        if updated != 1 {
            return Err(anyhow::anyhow!("root is disabled: {root_id}"));
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn root_enabled(&self, root_id: i64) -> anyhow::Result<bool> {
        let writable: bool = self
            .connection
            .query_row(
                "SELECT 1 FROM roots WHERE id = ?1 AND enabled = 1 AND pending_delete_at IS NULL",
                [root_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        Ok(writable)
    }

    #[allow(dead_code)]
    fn find_folder_id(
        &self,
        root_id: i64,
        parent_id: Option<i64>,
        name: &str,
    ) -> anyhow::Result<Option<i64>> {
        if let Some(parent_id) = parent_id {
            let mut statement = self.connection.prepare(
                r#"
                SELECT id FROM folders
                WHERE root_id = ?1 AND parent_id = ?2 AND name = ?3
                "#,
            )?;
            let mut rows = statement.query((root_id, parent_id, name))?;
            Ok(rows.next()?.map(|row| row.get(0)).transpose()?)
        } else {
            let mut statement = self.connection.prepare(
                r#"
                SELECT id FROM folders
                WHERE root_id = ?1 AND parent_id IS NULL AND name = ?2
                "#,
            )?;
            let mut rows = statement.query((root_id, name))?;
            Ok(rows.next()?.map(|row| row.get(0)).transpose()?)
        }
    }

    fn ensure_one_task_attempt_updated(
        &self,
        task_id: i64,
        updated: usize,
        expected_status: &str,
        expected_attempt_generation: i64,
    ) -> anyhow::Result<()> {
        if updated == 1 {
            return Ok(());
        }

        let Some(task) = self.get_task(task_id)? else {
            return Err(anyhow::anyhow!("task not found: {task_id}"));
        };
        if task.attempt_generation != expected_attempt_generation {
            return Err(anyhow::anyhow!(
                "task {task_id} attempt mismatch; expected attempt {}, current attempt {}",
                expected_attempt_generation,
                task.attempt_generation
            ));
        }
        Err(anyhow::anyhow!(
            "task {task_id} is not {expected_status}; current status is {}",
            task.status
        ))
    }

    pub(crate) fn current_task_attempt_generation(&self, task_id: i64) -> anyhow::Result<i64> {
        self.get_task(task_id)?
            .map(|task| task.attempt_generation)
            .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))
    }

    pub(crate) fn task_attempt_is_current(
        &self,
        task_id: i64,
        attempt_generation: i64,
    ) -> anyhow::Result<bool> {
        // Read only `attempt_generation` (not a full `get_task` row): the scan
        // hot path calls this per-file; a whole-row SELECT + deserialization is
        // wasted work for a single-column comparison.
        let current: Option<(i64, bool)> = self
            .connection
            .query_row(
                r#"
                SELECT attempt_generation,
                       root_uuid IS NULL OR EXISTS (
                         SELECT 1 FROM sync_root_local_bindings binding
                         WHERE binding.root_uuid = tasks.root_uuid
                           AND binding.local_binding_uuid = tasks.local_binding_uuid
                           AND binding.binding_generation = tasks.binding_generation
                           AND binding.state IN (
                             'materializing','active','local-only-fork','reconcile-required',
                             'relocating','local-rename-required'
                           )
                       )
                FROM tasks WHERE id = ?1
                "#,
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(current == Some((attempt_generation, true)))
    }

    pub fn list_tags(&self) -> anyhow::Result<Vec<TagRecord>> {
        let mut statement = self
            .connection
            .prepare("SELECT id, name, color FROM tags ORDER BY name ASC, id ASC")?;
        let rows = statement.query_map([], |row| {
            Ok(TagRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
            })
        })?;
        let mut tags = Vec::new();
        for row in rows {
            tags.push(row?);
        }
        Ok(tags)
    }

    pub fn create_tag(&self, name: &str, color: Option<&str>) -> Result<TagRecord, TagError> {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 64 {
            return Err(TagError::InvalidName);
        }
        if let Some(color_value) = color {
            if !is_valid_hex_color(color_value) {
                return Err(TagError::InvalidColor);
            }
        }
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let tag_uuid = uuid::Uuid::now_v7();
        let result = transaction.execute(
            "INSERT INTO tags(name, color, tag_uuid) VALUES (?1, ?2, ?3)",
            rusqlite::params![trimmed, color, tag_uuid.as_bytes().as_slice()],
        );
        match result {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                return Err(TagError::Duplicate);
            }
            Err(error) => return Err(TagError::from(error)),
        }
        let id = transaction.last_insert_rowid();
        crate::sync::capture::capture_catalog_mutation_if_initialized(
            &transaction,
            crate::sync::capture::MutationOrigin::LocalApi,
            crate::sync::types::EntityUuid(tag_uuid),
            crate::sync::types::Operation::SetField {
                field: "name".into(),
                value: serde_json::json!(trimmed),
            },
        )
        .map_err(TagError::Other)?;
        crate::sync::capture::capture_catalog_mutation_if_initialized(
            &transaction,
            crate::sync::capture::MutationOrigin::LocalApi,
            crate::sync::types::EntityUuid(tag_uuid),
            crate::sync::types::Operation::SetField {
                field: "color".into(),
                value: serde_json::json!(color),
            },
        )
        .map_err(TagError::Other)?;
        transaction.commit()?;
        Ok(TagRecord {
            id,
            name: trimmed.to_string(),
            color: color.map(str::to_string),
        })
    }

    pub fn delete_tag(&self, id: i64) -> anyhow::Result<bool> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let tag_uuid: Option<Vec<u8>> = transaction
            .query_row("SELECT tag_uuid FROM tags WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        let mut affected_files: Vec<i64> = {
            let mut statement = transaction
                .prepare("SELECT file_id FROM file_tags WHERE tag_id = ?1 ORDER BY file_id ASC")?;
            let rows = statement.query_map([id], |row| row.get::<_, i64>(0))?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row?);
            }
            ids
        };
        let updated = transaction.execute("DELETE FROM tags WHERE id = ?1", [id])?;
        if updated == 0 {
            // No transactional changes were made; commit the empty txn
            // so we release locks promptly.
            transaction.commit()?;
            return Ok(false);
        }
        // CASCADE has already removed file_tags rows. Resync each affected
        // file so media_fts.tags no longer carries the deleted tag's name.
        affected_files.sort_unstable();
        affected_files.dedup();
        for file_id in &affected_files {
            sync_media_fts_for_file_in_transaction(&transaction, *file_id)?;
        }
        if let Some(tag_uuid) = tag_uuid {
            crate::sync::capture::capture_catalog_mutation_if_initialized(
                &transaction,
                crate::sync::capture::MutationOrigin::LocalApi,
                crate::sync::capture::entity_uuid(&tag_uuid)?,
                crate::sync::types::Operation::Delete {
                    token: crate::sync::types::EventUuid::new(),
                },
            )?;
        }
        transaction.commit()?;
        Ok(true)
    }

    pub fn list_file_tag_ids(&self, file_id: i64) -> anyhow::Result<Vec<i64>> {
        let mut statement = self
            .connection
            .prepare("SELECT tag_id FROM file_tags WHERE file_id = ?1 ORDER BY tag_id ASC")?;
        let rows = statement.query_map([file_id], |row| row.get::<_, i64>(0))?;
        let mut tag_ids = Vec::new();
        for row in rows {
            tag_ids.push(row?);
        }
        Ok(tag_ids)
    }

    pub fn get_user_metadata(&self, file_id: i64) -> anyhow::Result<Option<UserMetadataRecord>> {
        if !self.file_exists(file_id)? {
            return Ok(None);
        }
        let row = self
            .connection
            .query_row(
                r#"
                SELECT rating, favorite, note, updated_at
                FROM user_metadata
                WHERE file_id = ?1
                "#,
                [file_id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, i64>(1)? != 0,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        let tag_ids = self.list_file_tag_ids(file_id)?;
        let (rating, favorite, note, updated_at) = match row {
            Some(values) => values,
            None => (None, false, None, 0_i64),
        };
        Ok(Some(UserMetadataRecord {
            file_id,
            rating,
            favorite,
            note,
            tag_ids,
            updated_at,
        }))
    }

    pub fn upsert_user_metadata_partial(
        &self,
        file_id: i64,
        patch: UserMetadataPatch,
    ) -> anyhow::Result<UserMetadataRecord> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let asset_uuid: Vec<u8> = transaction
            .query_row(
                "SELECT asset_uuid FROM files WHERE id=?1",
                [file_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("media item not found: {file_id}"))?;
        let now = unix_timestamp();
        let existing: Option<(Option<i64>, i64, Option<String>)> = transaction
            .query_row(
                "SELECT rating, favorite, note FROM user_metadata WHERE file_id = ?1",
                [file_id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;

        let (current_rating, current_favorite, current_note) =
            existing.clone().unwrap_or((None, 0, None));
        let next_rating = match patch.rating {
            Some(value) => value,
            None => current_rating,
        };
        let next_favorite = match patch.favorite {
            Some(value) => value as i64,
            None => current_favorite,
        };
        let next_note = match patch.note.clone() {
            Some(value) => value,
            None => current_note,
        };

        transaction.execute(
            r#"
            INSERT INTO user_metadata(file_id, rating, favorite, note, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(file_id) DO UPDATE SET
                rating = excluded.rating,
                favorite = excluded.favorite,
                note = excluded.note,
                updated_at = excluded.updated_at
            "#,
            (file_id, next_rating, next_favorite, &next_note, now),
        )?;
        // Only notes participate in media_fts. Rating/favorite-only edits are
        // authoritative metadata mutations, but rebuilding the FTS row here
        // needlessly deletes and reinserts a large hot index entry on every
        // capture. In a million-asset Library that index churn dominated the
        // local capture latency without changing any searchable value.
        if patch.note.is_some() {
            sync_media_fts_for_file_in_transaction(&transaction, file_id)?;
        }
        let entity = crate::sync::capture::entity_uuid(&asset_uuid)?;
        if patch.rating.is_some() {
            crate::sync::capture::capture_catalog_mutation_if_initialized(
                &transaction,
                crate::sync::capture::MutationOrigin::LocalApi,
                entity,
                crate::sync::types::Operation::SetField {
                    field: "rating".into(),
                    value: serde_json::json!(next_rating),
                },
            )?;
        }
        if patch.favorite.is_some() {
            crate::sync::capture::capture_catalog_mutation_if_initialized(
                &transaction,
                crate::sync::capture::MutationOrigin::LocalApi,
                entity,
                crate::sync::types::Operation::SetField {
                    field: "favorite".into(),
                    value: serde_json::json!(next_favorite != 0),
                },
            )?;
        }
        if patch.note.is_some() {
            crate::sync::capture::capture_catalog_mutation_if_initialized(
                &transaction,
                crate::sync::capture::MutationOrigin::LocalApi,
                entity,
                crate::sync::types::Operation::SetField {
                    field: "note".into(),
                    value: serde_json::json!(next_note),
                },
            )?;
        }
        transaction.commit()?;

        Ok(UserMetadataRecord {
            file_id,
            rating: next_rating,
            favorite: next_favorite != 0,
            note: next_note,
            tag_ids: self.list_file_tag_ids(file_id)?,
            updated_at: now,
        })
    }

    pub fn set_file_tags(&self, file_id: i64, tag_ids: &[i64]) -> Result<Vec<i64>, TagError> {
        if !self.file_exists(file_id).map_err(TagError::from)? {
            return Err(TagError::Other(anyhow::anyhow!(
                "media item not found: {file_id}"
            )));
        }
        let mut unique = std::collections::BTreeSet::new();
        for tag_id in tag_ids {
            unique.insert(*tag_id);
        }
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let asset_uuid: Vec<u8> = transaction.query_row(
            "SELECT asset_uuid FROM files WHERE id=?1",
            [file_id],
            |row| row.get(0),
        )?;
        let current: std::collections::BTreeSet<i64> = {
            let mut statement =
                transaction.prepare("SELECT tag_id FROM file_tags WHERE file_id=?1")?;
            let values = statement
                .query_map([file_id], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            values
        };
        for tag_id in &unique {
            let exists: i64 = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM tags WHERE id = ?1)",
                [tag_id],
                |row| row.get(0),
            )?;
            if exists == 0 {
                return Err(TagError::UnknownTagId(*tag_id));
            }
        }
        transaction.execute("DELETE FROM file_tags WHERE file_id = ?1", [file_id])?;
        for tag_id in &unique {
            transaction.execute(
                "INSERT INTO file_tags(file_id, tag_id) VALUES (?1, ?2)",
                (file_id, tag_id),
            )?;
        }
        sync_media_fts_for_file_in_transaction(&transaction, file_id)?;
        let owner = crate::sync::capture::entity_uuid(&asset_uuid).map_err(TagError::Other)?;
        for tag_id in unique.difference(&current) {
            let member = tag_entity_uuid(&transaction, *tag_id).map_err(TagError::Other)?;
            crate::sync::capture::capture_catalog_mutation_if_initialized(
                &transaction,
                crate::sync::capture::MutationOrigin::LocalApi,
                owner,
                crate::sync::types::Operation::SetAdd {
                    set: "tags".into(),
                    member_uuid: member,
                },
            )
            .map_err(TagError::Other)?;
        }
        for tag_id in current.difference(&unique) {
            let member = tag_entity_uuid(&transaction, *tag_id).map_err(TagError::Other)?;
            let observed_adds =
                crate::sync::capture::observed_tag_adds(&transaction, owner, member)
                    .map_err(TagError::Other)?;
            crate::sync::capture::capture_catalog_mutation_if_initialized(
                &transaction,
                crate::sync::capture::MutationOrigin::LocalApi,
                owner,
                crate::sync::types::Operation::SetRemove {
                    set: "tags".into(),
                    member_uuid: member,
                    observed_adds,
                },
            )
            .map_err(TagError::Other)?;
        }
        transaction.commit()?;
        Ok(unique.into_iter().collect())
    }

    pub fn add_file_tag(&self, file_id: i64, tag_id: i64) -> Result<Vec<i64>, TagError> {
        if !self.file_exists(file_id).map_err(TagError::from)? {
            return Err(TagError::Other(anyhow::anyhow!(
                "media item not found: {file_id}"
            )));
        }
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let asset_uuid: Vec<u8> = transaction.query_row(
            "SELECT asset_uuid FROM files WHERE id=?1",
            [file_id],
            |row| row.get(0),
        )?;
        let exists: i64 = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM tags WHERE id = ?1)",
            [tag_id],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Err(TagError::UnknownTagId(tag_id));
        }
        let inserted = transaction.execute(
            "INSERT OR IGNORE INTO file_tags(file_id, tag_id) VALUES (?1, ?2)",
            (file_id, tag_id),
        )?;
        sync_media_fts_for_file_in_transaction(&transaction, file_id)?;
        if inserted != 0 {
            let owner = crate::sync::capture::entity_uuid(&asset_uuid).map_err(TagError::Other)?;
            let member = tag_entity_uuid(&transaction, tag_id).map_err(TagError::Other)?;
            crate::sync::capture::capture_catalog_mutation_if_initialized(
                &transaction,
                crate::sync::capture::MutationOrigin::LocalApi,
                owner,
                crate::sync::types::Operation::SetAdd {
                    set: "tags".into(),
                    member_uuid: member,
                },
            )
            .map_err(TagError::Other)?;
        }
        transaction.commit()?;
        Ok(self.list_file_tag_ids(file_id)?)
    }

    pub fn remove_file_tag(&self, file_id: i64, tag_id: i64) -> Result<Vec<i64>, TagError> {
        if !self.file_exists(file_id).map_err(TagError::from)? {
            return Err(TagError::Other(anyhow::anyhow!(
                "media item not found: {file_id}"
            )));
        }
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let asset_uuid: Vec<u8> = transaction.query_row(
            "SELECT asset_uuid FROM files WHERE id=?1",
            [file_id],
            |row| row.get(0),
        )?;
        let removed = transaction.execute(
            "DELETE FROM file_tags WHERE file_id = ?1 AND tag_id = ?2",
            (file_id, tag_id),
        )?;
        if removed == 0 {
            return Err(TagError::UnknownTagId(tag_id));
        }
        sync_media_fts_for_file_in_transaction(&transaction, file_id)?;
        let owner = crate::sync::capture::entity_uuid(&asset_uuid).map_err(TagError::Other)?;
        let member = tag_entity_uuid(&transaction, tag_id).map_err(TagError::Other)?;
        let observed_adds = crate::sync::capture::observed_tag_adds(&transaction, owner, member)
            .map_err(TagError::Other)?;
        crate::sync::capture::capture_catalog_mutation_if_initialized(
            &transaction,
            crate::sync::capture::MutationOrigin::LocalApi,
            owner,
            crate::sync::types::Operation::SetRemove {
                set: "tags".into(),
                member_uuid: member,
                observed_adds,
            },
        )
        .map_err(TagError::Other)?;
        transaction.commit()?;
        Ok(self.list_file_tag_ids(file_id)?)
    }

    #[allow(dead_code)]
    pub fn sync_media_fts_for_file(&self, file_id: i64) -> anyhow::Result<()> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        sync_media_fts_for_file_in_transaction(&transaction, file_id)?;
        transaction.commit()?;
        Ok(())
    }

    /// Ensure `media_fts` has at least one row per `files` entry. Run once at
    /// startup; cheap when sizes already match.
    pub fn backfill_media_fts_if_empty(&self) -> anyhow::Result<()> {
        let files_count: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
        if files_count == 0 {
            return Ok(());
        }
        let fts_count: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM media_fts", [], |row| row.get(0))?;
        if fts_count >= files_count {
            return Ok(());
        }
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let mut statement = transaction.prepare("SELECT id FROM files")?;
        let ids: Vec<i64> = statement
            .query_map([], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for file_id in ids {
            sync_media_fts_for_file_in_transaction(&transaction, file_id)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn search_media_page(&self, query: SearchQuery) -> anyhow::Result<Page<MediaRecord>> {
        let limit = query.limit.clamp(1, 500);
        let sort = normalize_search_sort(&query.sort);
        let offset = query.offset.unwrap_or(0).max(0);
        let use_offset = query.offset.is_some();
        let trimmed_query = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        let mut predicates = vec!["files.status = 'active'".to_string()];
        let mut parameters: Vec<Value> = Vec::new();

        if trimmed_query.is_some() {
            predicates.push("media_fts.rowid = files.id".to_string());
            predicates.push("media_fts MATCH ?".to_string());
            parameters.push(Value::Text(build_fts_match_query(
                trimmed_query.as_deref().unwrap(),
            )));
        }
        if let Some(root_id) = query.root_id {
            predicates.push("files.root_id = ?".to_string());
            parameters.push(Value::Integer(root_id));
        }
        if let Some(folder_id) = query.folder_id {
            if query.include_descendants {
                predicates.push(
                    r#"files.folder_id IN (
                        WITH RECURSIVE descendant_folders(id) AS (
                            SELECT id FROM folders WHERE id = ?
                            UNION ALL
                            SELECT folders.id
                            FROM folders
                            JOIN descendant_folders ON folders.parent_id = descendant_folders.id
                        )
                        SELECT id FROM descendant_folders
                    )"#
                    .to_string(),
                );
            } else {
                predicates.push("files.folder_id = ?".to_string());
            }
            parameters.push(Value::Integer(folder_id));
        }
        if let Some(kind) = query.kind.as_deref() {
            predicates.push("media.kind = ?".to_string());
            parameters.push(Value::Text(kind.to_string()));
        }
        if let Some(min_rating) = query.min_rating {
            predicates
                .push("user_metadata.rating IS NOT NULL AND user_metadata.rating >= ?".to_string());
            parameters.push(Value::Integer(min_rating));
        }
        if let Some(favorite) = query.favorite {
            if favorite {
                predicates.push("user_metadata.favorite = 1".to_string());
            } else {
                predicates.push(
                    "(user_metadata.favorite IS NULL OR user_metadata.favorite = 0)".to_string(),
                );
            }
        }

        let mut tag_filter_clause = String::new();
        if !query.tag_ids.is_empty() {
            let mut unique_tags: Vec<i64> = query.tag_ids.clone();
            unique_tags.sort_unstable();
            unique_tags.dedup();
            let placeholders = unique_tags
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            tag_filter_clause = format!(
                " AND files.id IN (SELECT file_id FROM file_tags WHERE tag_id IN ({placeholders}) GROUP BY file_id HAVING COUNT(DISTINCT tag_id) = ?)"
            );
            for tag_id in &unique_tags {
                parameters.push(Value::Integer(*tag_id));
            }
            parameters.push(Value::Integer(unique_tags.len() as i64));
        }

        let sort_clause = match sort {
            "mtime_asc" => "files.mtime ASC, files.id ASC",
            "name_asc" => "files.name ASC, files.id ASC",
            "name_desc" => "files.name DESC, files.id DESC",
            "rating_desc" => {
                "user_metadata.rating IS NULL, user_metadata.rating DESC, files.id DESC"
            }
            "rating_asc" => "user_metadata.rating IS NULL, user_metadata.rating ASC, files.id ASC",
            _ => "files.mtime DESC, files.id DESC",
        };

        let fts_join = if trimmed_query.is_some() {
            ", media_fts"
        } else {
            ""
        };
        let count_predicates = predicates.clone();
        let count_parameters = parameters.clone();
        let total_count: i64 = {
            // Avoid materializing unused one-to-one LEFT JOINs for the count.
            // On million-asset libraries this turns the common no-filter
            // search count into an index-only files scan while preserving the
            // exact same predicates and enabled-root semantics.
            let count_media_join = query
                .kind
                .as_ref()
                .map(|_| " LEFT JOIN media ON media.file_id = files.id")
                .unwrap_or("");
            let count_metadata_join = if query.min_rating.is_some() || query.favorite.is_some() {
                " LEFT JOIN user_metadata ON user_metadata.file_id = files.id"
            } else {
                ""
            };
            let count_sql = format!(
                r#"
                SELECT COUNT(*)
                FROM files{fts_join}
                {count_media_join}
                {count_metadata_join}
                JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
                WHERE {where_clause}{tag_filter_clause}
                "#,
                fts_join = fts_join,
                count_media_join = count_media_join,
                count_metadata_join = count_metadata_join,
                where_clause = count_predicates.join(" AND "),
                tag_filter_clause = tag_filter_clause,
            );
            self.connection
                .query_row(&count_sql, params_from_iter(count_parameters), |row| {
                    row.get(0)
                })?
        };
        let cursor = if use_offset {
            None
        } else {
            query
                .cursor
                .as_deref()
                .map(|cursor| decode_search_cursor(cursor, sort))
                .transpose()?
        };
        if let Some(cursor) = &cursor {
            let predicate = match (sort, &cursor.key) {
                ("name_asc", SearchCursorKey::Name(name)) => {
                    parameters.push(Value::Text(name.clone()));
                    parameters.push(Value::Text(name.clone()));
                    parameters.push(Value::Integer(cursor.id));
                    "(files.name > ? OR (files.name = ? AND files.id > ?))".to_string()
                }
                ("name_desc", SearchCursorKey::Name(name)) => {
                    parameters.push(Value::Text(name.clone()));
                    parameters.push(Value::Text(name.clone()));
                    parameters.push(Value::Integer(cursor.id));
                    "(files.name < ? OR (files.name = ? AND files.id < ?))".to_string()
                }
                ("mtime_asc", SearchCursorKey::Mtime(value)) => {
                    parameters.push(Value::Integer(*value));
                    parameters.push(Value::Integer(*value));
                    parameters.push(Value::Integer(cursor.id));
                    "(files.mtime > ? OR (files.mtime = ? AND files.id > ?))".to_string()
                }
                ("rating_desc", SearchCursorKey::Rating(rating)) => {
                    let (clause, params_for_clause) =
                        build_rating_keyset_predicate(*rating, cursor.id, false);
                    for value in params_for_clause {
                        parameters.push(value);
                    }
                    clause
                }
                ("rating_asc", SearchCursorKey::Rating(rating)) => {
                    let (clause, params_for_clause) =
                        build_rating_keyset_predicate(*rating, cursor.id, true);
                    for value in params_for_clause {
                        parameters.push(value);
                    }
                    clause
                }
                (_, SearchCursorKey::Mtime(value)) => {
                    parameters.push(Value::Integer(*value));
                    parameters.push(Value::Integer(*value));
                    parameters.push(Value::Integer(cursor.id));
                    "(files.mtime < ? OR (files.mtime = ? AND files.id < ?))".to_string()
                }
                _ => {
                    return Err(anyhow::anyhow!("invalid search cursor"));
                }
            };
            predicates.push(predicate);
        }

        parameters.push(Value::Integer(limit + 1));
        if use_offset {
            parameters.push(Value::Integer(offset));
        }

        let sql = format!(
            r#"
            SELECT files.id, files.root_id, files.folder_id, files.name, files.ext,
                   files.size, files.mtime, media.kind, media.width, media.height,
                   media.duration_ms, media.codec,
                   media.preview_placeholder, media.preview_placeholder_format,
                   media.metadata_status, files.file_key,
                   thumbs.profile, thumbs.state, thumbs.short_side_px,
                   thumbs.output_format, thumbs.cache_key, thumbs.width, thumbs.height,
                   thumbs.byte_size, thumbs.error, thumbs.source_fingerprint, thumbs.served_by,
                   thumbs.updated_at,
                   user_metadata.rating, user_metadata.favorite, user_metadata.note
            FROM files{fts_join}
            LEFT JOIN media ON media.file_id = files.id
            LEFT JOIN thumbs ON thumbs.file_id = files.id AND thumbs.profile = 'grid_320'
            LEFT JOIN user_metadata ON user_metadata.file_id = files.id
            JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
            WHERE {where_clause}{tag_filter_clause}
            ORDER BY {sort_clause}
            LIMIT ?
            {offset_clause}
            "#,
            fts_join = fts_join,
            where_clause = predicates.join(" AND "),
            tag_filter_clause = tag_filter_clause,
            sort_clause = sort_clause,
            offset_clause = if use_offset { "OFFSET ?" } else { "" },
        );

        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(parameters), |row| {
            let mut media = media_from_row(row)?;
            let rating: Option<i64> = row.get(28)?;
            let favorite: Option<i64> = row.get(29)?;
            let note: Option<String> = row.get(30)?;
            media.rating = rating;
            media.favorite = favorite.map(|value| value != 0).unwrap_or(false);
            media.note = note;
            Ok(media)
        })?;

        let mut items: Vec<MediaRecord> = Vec::new();
        for row in rows {
            items.push(row?);
        }

        // Populate per-file tag ids in a single batched query so we avoid
        // N+1 round-trips when the result set is large.
        if !items.is_empty() {
            let id_set: Vec<i64> = items.iter().map(|item| item.id).collect();
            let placeholders = id_set.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            let sql = format!(
                "SELECT file_id, tag_id FROM file_tags \
                 WHERE file_id IN ({placeholders}) \
                 ORDER BY file_id ASC, tag_id ASC"
            );
            let mut tag_index: std::collections::HashMap<i64, Vec<i64>> =
                std::collections::HashMap::with_capacity(id_set.len());
            let mut tag_statement = self.connection.prepare(&sql)?;
            let parameters: Vec<Value> = id_set.iter().map(|id| Value::Integer(*id)).collect();
            let rows = tag_statement.query_map(params_from_iter(parameters), |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (file_id, tag_id) = row?;
                tag_index.entry(file_id).or_default().push(tag_id);
            }
            for item in items.iter_mut() {
                if let Some(tag_ids) = tag_index.remove(&item.id) {
                    item.tag_ids = tag_ids;
                }
            }
        }

        let next_cursor = if items.len() > limit as usize {
            items.pop();
            items.last().map(|item| encode_search_cursor(sort, item))
        } else {
            None
        };
        Ok(Page {
            items,
            next_cursor,
            total_count: Some(total_count),
        })
    }

    fn file_exists(&self, file_id: i64) -> anyhow::Result<bool> {
        let exists: i64 = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM files WHERE id = ?1)",
            [file_id],
            |row| row.get(0),
        )?;
        Ok(exists != 0)
    }
}

fn upsert_folder_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    folder: FolderUpsert,
    scan_generation: Option<i64>,
) -> anyhow::Result<i64> {
    if let Some(existing_id) =
        find_folder_id_in_transaction(transaction, folder.root_id, folder.parent_id, &folder.name)?
    {
        transaction.execute(
            r#"
            UPDATE folders
            SET path_hash = ?1, mtime = ?2, status = 'active', scan_seen_at = ?3
            WHERE id = ?4
            "#,
            (
                &folder.path_hash,
                folder.mtime,
                scan_generation,
                existing_id,
            ),
        )?;
        return Ok(existing_id);
    }

    transaction.execute(
        r#"
        INSERT INTO folders(root_id, parent_id, name, path_hash, mtime, status, scan_seen_at)
        VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6)
        "#,
        (
            folder.root_id,
            folder.parent_id,
            &folder.name,
            &folder.path_hash,
            folder.mtime,
            scan_generation,
        ),
    )?;
    Ok(transaction.last_insert_rowid())
}

fn scan_batch_root_ids(batch: &ScanWriteBatch) -> Vec<i64> {
    let mut root_ids = Vec::new();
    for root_id in batch
        .folders
        .iter()
        .map(|folder| folder.root_id)
        .chain(batch.files.iter().map(|file| file.file.root_id))
    {
        if !root_ids.contains(&root_id) {
            root_ids.push(root_id);
        }
    }
    root_ids
}

fn ensure_root_enabled_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    root_id: i64,
) -> anyhow::Result<()> {
    let writable: bool = transaction
        .query_row(
            "SELECT 1 FROM roots WHERE id = ?1 AND enabled = 1 AND pending_delete_at IS NULL",
            [root_id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if writable {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "root {root_id} is disabled or being removed"
    ))
}

fn active_scan_generation_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    root_id: i64,
) -> anyhow::Result<Option<i64>> {
    let generation = transaction.query_row(
        "SELECT active_scan_generation FROM roots WHERE id = ?1",
        [root_id],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    Ok(generation)
}

fn next_root_scan_generation_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    root_id: i64,
) -> anyhow::Result<i64> {
    let max_existing: Option<i64> = transaction.query_row(
        r#"
        SELECT MAX(value)
        FROM (
          SELECT active_scan_generation AS value
          FROM roots
          WHERE id = ?1 AND active_scan_generation IS NOT NULL
          UNION ALL
          SELECT scan_seen_at AS value
          FROM folders
          WHERE root_id = ?1 AND scan_seen_at IS NOT NULL
          UNION ALL
          SELECT scan_seen_at AS value
          FROM files
          WHERE root_id = ?1 AND scan_seen_at IS NOT NULL
        )
        "#,
        [root_id],
        |row| row.get(0),
    )?;
    let next_after_existing = max_existing.unwrap_or(0) + 1;
    Ok(unix_timestamp_millis().max(next_after_existing))
}

fn task_attempt_running_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    task_id: i64,
    attempt_generation: i64,
) -> anyhow::Result<bool> {
    let mut statement = transaction.prepare(
        r#"
        SELECT status, attempt_generation,
               root_uuid IS NULL OR EXISTS (
                 SELECT 1 FROM sync_root_local_bindings binding
                 WHERE binding.root_uuid = tasks.root_uuid
                   AND binding.local_binding_uuid = tasks.local_binding_uuid
                   AND binding.binding_generation = tasks.binding_generation
                   AND binding.state IN (
                     'materializing','active','local-only-fork','reconcile-required',
                     'relocating','local-rename-required'
                   )
               )
        FROM tasks
        WHERE id = ?1
        "#,
    )?;
    let mut rows = statement.query([task_id])?;
    let Some(row) = rows.next()? else {
        return Err(anyhow::anyhow!("task not found: {task_id}"));
    };
    let status: String = row.get(0)?;
    let current_attempt: i64 = row.get(1)?;
    let root_binding_current: bool = row.get(2)?;
    Ok(status == "running" && current_attempt == attempt_generation && root_binding_current)
}

fn get_folder_id_by_path_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    root_id: i64,
    root_path: &Path,
    folder_path: &Path,
) -> anyhow::Result<Option<i64>> {
    let mut folder_id = transaction
        .query_row(
            "SELECT id FROM folders WHERE root_id=?1 AND parent_id IS NULL AND name=''",
            [root_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let Ok(relative) = folder_path.strip_prefix(root_path) else {
        return Ok(None);
    };
    for component in relative.components() {
        let Some(parent_id) = folder_id else {
            return Ok(None);
        };
        let name = component.as_os_str().to_string_lossy();
        folder_id = transaction
            .query_row(
                "SELECT id FROM folders WHERE root_id=?1 AND parent_id=?2 AND name=?3",
                rusqlite::params![root_id, parent_id, name.as_ref()],
                |row| row.get(0),
            )
            .optional()?;
    }
    Ok(folder_id)
}

fn ensure_folder_chain_for_path_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    root_id: i64,
    root_path: &Path,
    folder_path: &Path,
) -> anyhow::Result<i64> {
    let scan_generation = active_scan_generation_in_transaction(transaction, root_id)?;
    let root_folder_id = upsert_folder_in_transaction(
        transaction,
        FolderUpsert {
            root_id,
            parent_id: None,
            name: String::new(),
            path_hash: hash_path(root_path),
            mtime: metadata_time(root_path.metadata().ok().as_ref()),
        },
        scan_generation,
    )?;
    let relative = folder_path.strip_prefix(root_path).map_err(|_| {
        anyhow::anyhow!(
            "folder path is outside root {}: {}",
            root_path.display(),
            folder_path.display()
        )
    })?;
    let mut parent_id = root_folder_id;
    let mut current_path = root_path.to_path_buf();
    for component in relative.components() {
        current_path.push(component.as_os_str());
        parent_id = upsert_folder_in_transaction(
            transaction,
            FolderUpsert {
                root_id,
                parent_id: Some(parent_id),
                name: component.as_os_str().to_string_lossy().into_owned(),
                path_hash: hash_path(&current_path),
                mtime: metadata_time(current_path.metadata().ok().as_ref()),
            },
            scan_generation,
        )?;
    }
    Ok(parent_id)
}

fn mark_task_failed_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    task_id: i64,
    attempt_generation: i64,
    error: &str,
) -> anyhow::Result<usize> {
    Ok(transaction.execute(
        "UPDATE tasks SET status = 'failed', updated_at = ?1, error = ?2 WHERE id = ?3 AND status IN ('pending', 'running') AND attempt_generation = ?4",
        (unix_timestamp(), error, task_id, attempt_generation),
    )?)
}

pub(super) fn hash_path(path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn metadata_time(metadata: Option<&std::fs::Metadata>) -> Option<i64> {
    let metadata = metadata?;
    let time = metadata.modified().ok()?;
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

fn ensure_task_attempt_running_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    task_id: i64,
    attempt_generation: i64,
) -> anyhow::Result<()> {
    if task_attempt_running_in_transaction(transaction, task_id, attempt_generation)? {
        return Ok(());
    }
    let (status, current_attempt): (String, i64) = transaction.query_row(
        "SELECT status, attempt_generation FROM tasks WHERE id = ?1",
        [task_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if current_attempt != attempt_generation {
        return Err(anyhow::anyhow!(
            "task {task_id} attempt mismatch; expected attempt {}, current attempt {}",
            attempt_generation,
            current_attempt
        ));
    }
    Err(anyhow::anyhow!(
        "task {task_id} is not running; current status is {status}"
    ))
}

impl Database {
    fn cancel_root_scan_tasks_in_transaction(
        &self,
        transaction: &Transaction<'_>,
        root_id: i64,
    ) -> anyhow::Result<bool> {
        let mut statement = transaction.prepare(
            r#"SELECT id FROM tasks
               WHERE kind IN ('root_scan', 'interactive_folder_scan')
                 AND root_id = ?1 AND status IN ('pending', 'running')
               ORDER BY id ASC"#,
        )?;
        let task_ids = statement
            .query_map([root_id], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);

        let now = unix_timestamp();
        let mut appended = false;
        for task_id in &task_ids {
            transaction.execute(
                "UPDATE tasks SET status = 'failed', updated_at = ?1, error = 'root is disabled' WHERE id = ?2",
                (now, task_id),
            )?;
            appended |= self.publish_task_notification_in_transaction(
                transaction,
                *task_id,
                None,
                notification_outbox::PublishMode::Terminal,
            )?;
        }
        Ok(appended)
    }
}

fn reusable_file_id_for_content_identity_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    file: &FileUpsert,
) -> anyhow::Result<Option<i64>> {
    let Some(file_key) = file.file_key.as_deref().filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    transaction
        .query_row(
            r#"
            SELECT id
            FROM files
            WHERE root_id = ?1
              AND file_key = ?2
              AND status IN ('active', 'missing')
              AND (folder_id <> ?3 OR name <> ?4)
            ORDER BY CASE status WHEN 'active' THEN 0 ELSE 1 END, id DESC
            LIMIT 1
            "#,
            (file.root_id, file_key, file.folder_id, &file.name),
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
}

fn rebuild_content_heads_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    asset_uuid: &[u8],
) -> anyhow::Result<()> {
    transaction.execute(
        "DELETE FROM asset_content_heads WHERE asset_uuid=?1",
        [asset_uuid],
    )?;
    transaction.execute(
        "INSERT INTO asset_content_heads(asset_uuid,content_revision_uuid,is_display_winner) SELECT r.asset_uuid,r.content_revision_uuid,CASE WHEN r.content_revision_uuid=(SELECT rr.content_revision_uuid FROM asset_content_revisions rr JOIN sync_events e ON e.event_uuid=rr.event_uuid WHERE rr.asset_uuid=?1 AND NOT EXISTS (SELECT 1 FROM asset_content_supersedes s WHERE s.observed_content_revision_uuid=rr.content_revision_uuid) ORDER BY e.hlc_wall_ms DESC,e.hlc_logical DESC,e.device_uuid DESC,e.actor_epoch DESC,e.actor_seq DESC,e.event_uuid DESC LIMIT 1) THEN 1 ELSE 0 END FROM asset_content_revisions r WHERE r.asset_uuid=?1 AND r.event_uuid IS NOT NULL AND NOT EXISTS (SELECT 1 FROM asset_content_supersedes s WHERE s.observed_content_revision_uuid=r.content_revision_uuid)",
        [asset_uuid],
    )?;
    Ok(())
}

fn content_supersedes_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    revision_uuid: &[u8],
) -> anyhow::Result<std::collections::BTreeSet<crate::sync::types::RevisionUuid>> {
    let mut statement = transaction.prepare(
        "SELECT observed_content_revision_uuid FROM asset_content_supersedes WHERE content_revision_uuid=?1 ORDER BY observed_content_revision_uuid",
    )?;
    let supersedes = statement
        .query_map([revision_uuid], |row| row.get::<_, Vec<u8>>(0))?
        .map(|value| {
            Ok(crate::sync::types::RevisionUuid(uuid::Uuid::from_slice(
                &value?,
            )?))
        })
        .collect();
    supersedes
}

fn create_sync_publication_intent_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
    source_path: &str,
    byte_size: i64,
    source_mtime_ms: Option<i64>,
) -> anyhow::Result<uuid::Uuid> {
    if byte_size < 0 {
        anyhow::bail!("publication source size cannot be negative");
    }
    let asset_uuid: Vec<u8> = transaction.query_row(
        "SELECT asset_uuid FROM files WHERE id=?1",
        [file_id],
        |row| row.get(0),
    )?;
    let generation: i64 = transaction.query_row(
        "SELECT next_local_mutation_generation FROM sync_local_state WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    transaction.execute(
        "UPDATE sync_local_state SET next_local_mutation_generation=?1 WHERE singleton=1",
        [generation + 1],
    )?;
    let revision = uuid::Uuid::now_v7();
    let intent = uuid::Uuid::now_v7();
    let now = unix_timestamp_millis();
    let superseded_revisions = {
        let mut statement = transaction.prepare(
            "SELECT h.content_revision_uuid FROM asset_content_heads h JOIN asset_content_revisions r ON r.content_revision_uuid=h.content_revision_uuid WHERE h.asset_uuid=?1 AND r.event_uuid IS NOT NULL UNION SELECT s.observed_content_revision_uuid FROM asset_content_heads h JOIN asset_content_revisions r ON r.content_revision_uuid=h.content_revision_uuid JOIN asset_content_supersedes s ON s.content_revision_uuid=h.content_revision_uuid WHERE h.asset_uuid=?1 AND r.event_uuid IS NULL ORDER BY 1",
        )?;
        let revisions = statement
            .query_map([&asset_uuid], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        revisions
    };
    let expected_content_hash: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT content_fingerprint FROM sync_local_file_identities WHERE asset_uuid=?1",
            [&asset_uuid],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if expected_content_hash
        .as_ref()
        .is_some_and(|value| value.len() != 32)
    {
        anyhow::bail!("captured content fingerprint must be SHA-256");
    }
    transaction.execute(
        "UPDATE sync_publication_intents SET state='cancelled',error_code='superseded_local_revision',updated_at=?1 WHERE asset_uuid=?2 AND event_uuid IS NULL AND state IN ('pending-hash','pending-manifest','ready','failed')",
        rusqlite::params![now,&asset_uuid],
    )?;
    transaction.execute(
        "UPDATE media_publication_jobs SET phase='cancelled',owner_token=NULL,lease_expires_at=NULL,error_code='superseded_local_revision',updated_at=?1 WHERE intent_uuid IN (SELECT intent_uuid FROM sync_publication_intents WHERE asset_uuid=?2 AND state='cancelled' AND event_uuid IS NULL) AND phase NOT IN ('event-published','archived')",
        rusqlite::params![now,&asset_uuid],
    )?;
    transaction.execute(
        "INSERT INTO asset_content_revisions(content_revision_uuid,asset_uuid,byte_size,local_mutation_generation,source_mtime_ms,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
        rusqlite::params![revision.as_bytes().as_slice(),&asset_uuid,byte_size,generation,source_mtime_ms,now],
    )?;
    for observed_revision in superseded_revisions {
        transaction.execute(
            "INSERT OR IGNORE INTO asset_content_supersedes(content_revision_uuid,observed_content_revision_uuid) VALUES (?1,?2)",
            rusqlite::params![revision.as_bytes().as_slice(),observed_revision],
        )?;
    }
    transaction.execute(
        "DELETE FROM asset_content_heads WHERE asset_uuid=?1",
        [&asset_uuid],
    )?;
    transaction.execute(
        "INSERT INTO asset_content_heads(asset_uuid,content_revision_uuid,is_display_winner) VALUES (?1,?2,1)",
        rusqlite::params![&asset_uuid,revision.as_bytes().as_slice()],
    )?;
    transaction.execute(
        "INSERT INTO sync_publication_intents(intent_uuid,asset_uuid,content_revision_uuid,binding_epoch,local_mutation_generation,source_path,state,attempt_count,created_at,updated_at) SELECT ?1,?2,?3,active_binding_epoch,?4,?5,'pending-hash',0,?6,?6 FROM sync_local_state WHERE singleton=1",
        rusqlite::params![intent.as_bytes().as_slice(),&asset_uuid,revision.as_bytes().as_slice(),generation,source_path,now],
    )?;
    transaction.execute(
        "INSERT INTO media_publication_jobs(intent_uuid,phase,source_identity_json,binding_epoch,created_at,updated_at) SELECT ?1,'pending-hash',CASE WHEN ?2 IS NULL THEN NULL ELSE json_object('expectedContentSha256',lower(hex(?2))) END,active_binding_epoch,?3,?3 FROM sync_local_state WHERE singleton=1",
        rusqlite::params![intent.as_bytes().as_slice(),expected_content_hash,now],
    )?;
    Ok(intent)
}

fn seed_initial_media_publication_intents(
    transaction: &rusqlite::Transaction<'_>,
) -> anyhow::Result<usize> {
    type Candidate = (i64, String, i64, i64);
    let candidates = {
        let mut statement = transaction.prepare(
            "WITH RECURSIVE folder_paths(id,root_id,relpath) AS (SELECT id,root_id,'' FROM folders WHERE parent_id IS NULL UNION ALL SELECT f.id,f.root_id,CASE WHEN p.relpath='' THEN f.name ELSE p.relpath||'/'||f.name END FROM folders f JOIN folder_paths p ON f.parent_id=p.id) SELECT f.id,CASE WHEN p.relpath='' THEN root.path||'/'||f.name ELSE root.path||'/'||p.relpath||'/'||f.name END,f.size,f.mtime FROM files f JOIN media ON media.file_id=f.id JOIN roots root ON root.id=f.root_id JOIN folder_paths p ON p.id=f.folder_id WHERE f.status='active' AND root.enabled=1 AND NOT EXISTS (SELECT 1 FROM asset_content_revisions r WHERE r.asset_uuid=f.asset_uuid) ORDER BY f.id",
        )?;
        let candidates = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<Vec<Candidate>, _>>()?;
        candidates
    };
    for (file_id, source_path, byte_size, source_mtime_seconds) in &candidates {
        create_sync_publication_intent_in_transaction(
            transaction,
            *file_id,
            source_path,
            *byte_size,
            Some(source_mtime_seconds.saturating_mul(1000)),
        )?;
    }
    Ok(candidates.len())
}

fn upsert_file_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    file: FileUpsert,
    scan_generation: Option<i64>,
    origin: crate::sync::capture::MutationOrigin,
    forced_reusable_file_id: Option<i64>,
) -> anyhow::Result<FileUpsertResult> {
    let root_id = file.root_id;
    let folder_id = file.folder_id;
    let name = file.name.clone();
    let size = file.size;
    let mtime = file.mtime;
    let file_key = file.file_key.clone();
    let previous_identity: Option<(i64, i64, Option<String>)> = transaction
        .query_row(
            "SELECT size, mtime, file_key FROM files WHERE folder_id = ?1 AND name = ?2",
            (folder_id, &name),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let identity_changed = previous_identity
        .as_ref()
        .map(|(previous_size, previous_mtime, previous_file_key)| {
            *previous_size != size
                || *previous_mtime != mtime
                || previous_file_key.as_deref() != file_key.as_deref()
        })
        .unwrap_or(true);

    let reusable_file_id = if previous_identity.is_none() {
        forced_reusable_file_id.or(reusable_file_id_for_content_identity_in_transaction(
            transaction,
            &file,
        )?)
    } else {
        None
    };
    let (id, identity_changed) = if let Some(reusable_file_id) = reusable_file_id {
        transaction.execute(
            r#"
            UPDATE files
            SET folder_id = ?1,
                name = ?2,
                ext = ?3,
                size = ?4,
                mtime = ?5,
                ctime = ?6,
                file_key = ?7,
                status = 'active',
                scan_seen_at = ?8
            WHERE id = ?9
            "#,
            (
                file.folder_id,
                &file.name,
                &file.ext,
                file.size,
                file.mtime,
                file.ctime,
                &file.file_key,
                scan_generation,
                reusable_file_id,
            ),
        )?;
        (reusable_file_id, false)
    } else {
        transaction.execute(
            r#"
            INSERT INTO files(root_id, folder_id, name, ext, size, mtime, ctime, file_key, status, scan_seen_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'active', ?9)
            ON CONFLICT(folder_id, name) DO UPDATE SET
              ext = excluded.ext,
              size = excluded.size,
              mtime = excluded.mtime,
              ctime = excluded.ctime,
              file_key = excluded.file_key,
              status = 'active',
              scan_seen_at = excluded.scan_seen_at
            "#,
            (
                file.root_id,
                file.folder_id,
                &file.name,
                &file.ext,
                file.size,
                file.mtime,
                file.ctime,
                &file.file_key,
                scan_generation,
            ),
        )?;
        let id = transaction.query_row(
            "SELECT id FROM files WHERE folder_id = ?1 AND name = ?2",
            (file.folder_id, &file.name),
            |row| row.get(0),
        )?;
        (id, identity_changed)
    };
    if let Some(file_key) = file_key.as_deref().filter(|value| !value.is_empty()) {
        let asset_uuid: Vec<u8> =
            transaction.query_row("SELECT asset_uuid FROM files WHERE id=?1", [id], |row| {
                row.get(0)
            })?;
        crate::sync::identity::record_identity(
            transaction,
            &crate::sync::capture::LocalFileIdentity {
                asset_uuid: uuid::Uuid::from_slice(&asset_uuid)?,
                volume_identity: root_id.to_be_bytes().to_vec(),
                file_identity: file_key.as_bytes().to_vec(),
                content_fingerprint: None,
            },
            &format!("{folder_id}/{name}"),
        )?;
        if reusable_file_id.is_some() && origin == crate::sync::capture::MutationOrigin::ExternalFs
        {
            let parent_uuid: Vec<u8> = transaction.query_row(
                "SELECT folder_uuid FROM folders WHERE id=?1",
                [folder_id],
                |row| row.get(0),
            )?;
            let mut supersedes = std::collections::BTreeSet::new();
            let mut statement = transaction
                .prepare("SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1")?;
            for row in statement.query_map([&asset_uuid], |row| row.get::<_, Vec<u8>>(0))? {
                supersedes.insert(crate::sync::types::RevisionUuid(uuid::Uuid::from_slice(
                    &row?,
                )?));
            }
            crate::sync::capture::capture_catalog_mutation_if_initialized(
                transaction,
                crate::sync::capture::MutationOrigin::ExternalFs,
                crate::sync::capture::entity_uuid(&asset_uuid)?,
                crate::sync::types::Operation::SetLocation {
                    revision_uuid: crate::sync::types::RevisionUuid::new(),
                    parent_uuid: Some(crate::sync::capture::entity_uuid(&parent_uuid)?),
                    name: name.clone(),
                    supersedes,
                },
            )?;
        }
    }
    invalidate_stale_thumbnail_states_in_transaction(
        transaction,
        CacheIdentity {
            file_id: id,
            root_id,
            folder_id,
            name: &name,
            size,
            mtime,
            file_key: file_key.as_deref(),
        },
    )?;
    sync_media_fts_for_file_in_transaction(transaction, id)?;
    Ok(FileUpsertResult {
        id,
        identity_changed,
    })
}

fn upsert_media_kind_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
    kind: &str,
    reset_metadata: bool,
) -> anyhow::Result<()> {
    let reset_metadata = i64::from(reset_metadata);
    transaction.execute(
        r#"
        INSERT INTO media(file_id, kind, metadata_status)
        VALUES (?1, ?2, 'pending')
        ON CONFLICT(file_id) DO UPDATE SET
          kind = excluded.kind,
          width = CASE WHEN ?3 != 0 OR kind IS NOT excluded.kind THEN NULL ELSE width END,
          height = CASE WHEN ?3 != 0 OR kind IS NOT excluded.kind THEN NULL ELSE height END,
          duration_ms = CASE WHEN ?3 != 0 OR kind IS NOT excluded.kind THEN NULL ELSE duration_ms END,
          codec = CASE WHEN ?3 != 0 OR kind IS NOT excluded.kind THEN NULL ELSE codec END,
          orientation = CASE WHEN ?3 != 0 OR kind IS NOT excluded.kind THEN NULL ELSE orientation END,
          has_alpha = CASE WHEN ?3 != 0 OR kind IS NOT excluded.kind THEN NULL ELSE has_alpha END,
          dominant_color = CASE WHEN ?3 != 0 OR kind IS NOT excluded.kind THEN NULL ELSE dominant_color END,
          phash = CASE WHEN ?3 != 0 OR kind IS NOT excluded.kind THEN NULL ELSE phash END,
          preview_placeholder = CASE WHEN ?3 != 0 OR kind IS NOT excluded.kind THEN NULL ELSE preview_placeholder END,
          preview_placeholder_format = CASE WHEN ?3 != 0 OR kind IS NOT excluded.kind THEN 'image/webp' ELSE preview_placeholder_format END,
          metadata_status = CASE WHEN ?3 != 0 OR kind IS NOT excluded.kind THEN 'pending' ELSE metadata_status END
        "#,
        (file_id, kind, reset_metadata),
    )?;
    Ok(())
}

fn invalidate_stale_thumbnail_states_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    identity: CacheIdentity<'_>,
) -> anyhow::Result<()> {
    invalidate_stale_thumbnail_states(
        transaction,
        identity.file_id,
        GRID_320_PROFILE,
        &source_fingerprint_for(&identity, GRID_320_PROFILE),
    )
}

fn invalidate_stale_thumbnail_states(
    connection: &rusqlite::Connection,
    file_id: i64,
    profile: &str,
    source_fingerprint: &str,
) -> anyhow::Result<()> {
    let updated = connection.execute(
        r#"
        UPDATE thumbs
        SET state = 'pending',
            cache_key = NULL,
            width = NULL,
            height = NULL,
            byte_size = NULL,
            error = NULL,
            source_fingerprint = NULL,
            served_by = NULL,
            updated_at = ?1
        WHERE file_id = ?2
          AND profile = ?3
          AND state NOT IN ('pending', 'queued')
          AND (source_fingerprint IS NULL OR source_fingerprint <> ?4)
        "#,
        (unix_timestamp(), file_id, profile, source_fingerprint),
    )?;
    if updated != 0 {
        connection.execute(
            "DELETE FROM thumb_blobs WHERE file_id = ?1 AND profile = ?2",
            (file_id, profile),
        )?;
    }
    Ok(())
}

fn upsert_thumbnail_state_in_connection(
    connection: &rusqlite::Connection,
    state: ThumbnailStateUpsert,
) -> anyhow::Result<()> {
    connection.execute(
        r#"
        INSERT INTO thumbs(
            file_id, profile, state, cache_key, width, height, byte_size,
            short_side_px, output_format, error, source_fingerprint, served_by, updated_at
        )
        VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, 320, 'image/webp', ?8, ?9,
            CASE WHEN ?3 = 'ready' AND ?4 IS NULL THEN 'db_blob' ELSE NULL END,
            ?10
        )
        ON CONFLICT(file_id, profile) DO UPDATE SET
            state = excluded.state,
            cache_key = excluded.cache_key,
            width = excluded.width,
            height = excluded.height,
            byte_size = excluded.byte_size,
            short_side_px = excluded.short_side_px,
            output_format = excluded.output_format,
            error = excluded.error,
            source_fingerprint = excluded.source_fingerprint,
            served_by = excluded.served_by,
            updated_at = excluded.updated_at
        "#,
        (
            state.file_id,
            &state.profile,
            &state.state,
            state.cache_key.as_deref(),
            state.width,
            state.height,
            state.byte_size,
            state.error.as_deref(),
            state.source_fingerprint.as_deref(),
            unix_timestamp(),
        ),
    )?;
    Ok(())
}

fn upsert_thumbnail_state_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    state: ThumbnailStateUpsert,
) -> anyhow::Result<()> {
    transaction.execute(
        r#"
        INSERT INTO thumbs(
            file_id, profile, state, cache_key, width, height, byte_size,
            short_side_px, output_format, error, source_fingerprint, served_by, updated_at
        )
        VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, 320, 'image/webp', ?8, ?9,
            CASE WHEN ?3 = 'ready' AND ?4 IS NULL THEN 'db_blob' ELSE NULL END,
            ?10
        )
        ON CONFLICT(file_id, profile) DO UPDATE SET
            state = excluded.state,
            cache_key = excluded.cache_key,
            width = excluded.width,
            height = excluded.height,
            byte_size = excluded.byte_size,
            short_side_px = excluded.short_side_px,
            output_format = excluded.output_format,
            error = excluded.error,
            source_fingerprint = excluded.source_fingerprint,
            served_by = excluded.served_by,
            updated_at = excluded.updated_at
        "#,
        (
            state.file_id,
            &state.profile,
            &state.state,
            state.cache_key.as_deref(),
            state.width,
            state.height,
            state.byte_size,
            state.error.as_deref(),
            state.source_fingerprint.as_deref(),
            unix_timestamp(),
        ),
    )?;
    Ok(())
}

struct PendingThumbnailTaskRecord {
    id: i64,
    priority: i64,
}

fn pending_thumbnail_task(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
) -> anyhow::Result<Option<PendingThumbnailTaskRecord>> {
    let mut statement = transaction.prepare(
        r#"
        SELECT id, priority
        FROM tasks
        WHERE kind = 'thumbnail'
          AND file_id = ?1
          AND status = 'pending'
        ORDER BY
          priority DESC,
          created_at ASC,
          id ASC
        LIMIT 1
        "#,
    )?;
    let mut rows = statement.query([file_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(PendingThumbnailTaskRecord {
        id: row.get(0)?,
        priority: row.get(1)?,
    }))
}

fn running_thumbnail_task(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
) -> anyhow::Result<Option<PendingThumbnailTaskRecord>> {
    let mut statement = transaction.prepare(
        r#"
        SELECT id, priority
        FROM tasks
        WHERE kind = 'thumbnail'
          AND file_id = ?1
          AND status = 'running'
        ORDER BY
          priority DESC,
          created_at ASC,
          id ASC
        LIMIT 1
        "#,
    )?;
    let mut rows = statement.query([file_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(PendingThumbnailTaskRecord {
        id: row.get(0)?,
        priority: row.get(1)?,
    }))
}

fn latest_thumbnail_task_id_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
) -> anyhow::Result<Option<i64>> {
    let mut statement = transaction.prepare(
        r#"
        SELECT id
        FROM tasks
        WHERE kind = 'thumbnail' AND file_id = ?1
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )?;
    let mut rows = statement.query([file_id])?;
    Ok(rows.next()?.map(|row| row.get(0)).transpose()?)
}

fn thumbnail_for_update_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
    profile: &str,
) -> anyhow::Result<Option<ThumbnailRecord>> {
    let mut statement = transaction.prepare(
        r#"
        SELECT files.id, thumbs.profile, thumbs.state, thumbs.short_side_px,
               thumbs.output_format, thumbs.cache_key, thumbs.width, thumbs.height,
               thumbs.byte_size, thumbs.error, thumbs.source_fingerprint, thumbs.served_by,
               thumbs.updated_at
        FROM files
        LEFT JOIN thumbs ON thumbs.file_id = files.id AND thumbs.profile = ?2
        WHERE files.id = ?1 AND files.status = 'active'
        "#,
    )?;
    let mut rows = statement.query((file_id, profile))?;
    Ok(rows
        .next()?
        .map(|row| thumbnail_from_row(row, profile))
        .transpose()?)
}

fn thumbnail_source_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
) -> anyhow::Result<Option<ThumbnailSourceRecord>> {
    let mut statement = transaction.prepare(
        r#"
        SELECT files.id, files.root_id, files.folder_id, files.name, files.size,
               files.mtime, files.file_key, media.kind, media.width, media.height,
               media.metadata_status
        FROM files
        JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
        LEFT JOIN media ON media.file_id = files.id
        WHERE files.id = ?1 AND files.status = 'active'
        "#,
    )?;
    let mut rows = statement.query([file_id])?;
    Ok(rows.next()?.map(thumbnail_source_from_row).transpose()?)
}

fn table_has_column_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
) -> anyhow::Result<bool> {
    let mut statement = transaction.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    for candidate in columns {
        if candidate? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Classify every existing failed thumbnail's `failure_kind` from its stored
/// `error` text and UPDATE it in place. Idempotent — only touches rows where
/// `failure_kind IS NULL`. Driven by the 0018 migration gate in
/// `apply_migrations` inside the same transaction as the ALTER TABLE +
/// schema_migrations registration (so the backfill and the "migration
/// applied" marker can never diverge), and directly by tests that need to
/// exercise the backfill logic against a hand-seeded transaction.
fn backfill_thumbnail_failure_kind_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
) -> anyhow::Result<()> {
    let mut stmt = transaction.prepare(
        "SELECT file_id, profile, coalesce(error, '') FROM thumbs \
         WHERE state = 'failed' AND failure_kind IS NULL",
    )?;
    let rows: Vec<(i64, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    for (file_id, profile, error) in rows {
        let kind = crate::thumbnails::classify_thumbnail_failure(&error).as_str();
        transaction.execute(
            "UPDATE thumbs SET failure_kind = ?1 WHERE file_id = ?2 AND profile = ?3",
            (kind, file_id, profile),
        )?;
    }
    Ok(())
}

fn reset_thumbnail_after_stale_source_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
    profile: &str,
    error: &str,
) -> anyhow::Result<()> {
    let now = unix_timestamp();
    let updated = transaction.execute(
        r#"
        UPDATE thumbs
        SET state = 'pending',
            cache_key = NULL,
            width = NULL,
            height = NULL,
            byte_size = NULL,
            error = ?1,
            source_fingerprint = NULL,
            served_by = NULL,
            updated_at = ?2
        WHERE file_id = ?3
          AND profile = ?4
          AND state IN ('pending', 'queued', 'failed')
        "#,
        (error, now, file_id, profile),
    )?;
    if updated == 0 {
        transaction.execute(
            r#"
            INSERT OR IGNORE INTO thumbs(
                file_id, profile, state, cache_key, width, height, byte_size,
                short_side_px, output_format, error, source_fingerprint, served_by, updated_at
            )
            VALUES (?1, ?2, 'pending', NULL, NULL, NULL, NULL, 320, 'image/webp', ?3, NULL, NULL, ?4)
            "#,
            (file_id, profile, error, now),
        )?;
    }
    transaction.execute(
        "DELETE FROM thumb_blobs WHERE file_id = ?1 AND profile = ?2",
        (file_id, profile),
    )?;
    Ok(())
}

fn publish_thumbnail_failure_for_attempted_source_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
    profile: &str,
    attempted_source_fingerprint: Option<&str>,
    error: &str,
    failure_kind: &str,
) -> anyhow::Result<bool> {
    let current_source = thumbnail_source_in_transaction(transaction, file_id)?
        .ok_or_else(|| anyhow::anyhow!("media item not found: {file_id}"))?;
    let current_source_fingerprint = current_source.source_fingerprint(profile);
    let is_current_attempt =
        attempted_source_fingerprint == Some(current_source_fingerprint.as_str());

    if is_current_attempt {
        upsert_thumbnail_state_in_transaction(
            transaction,
            ThumbnailStateUpsert {
                file_id,
                profile: profile.to_string(),
                state: "failed".to_string(),
                cache_key: None,
                width: None,
                height: None,
                byte_size: None,
                error: Some(error.to_string()),
                source_fingerprint: Some(current_source_fingerprint),
            },
        )?;
        transaction.execute(
            "UPDATE thumbs SET failure_kind = ?1 WHERE file_id = ?2 AND profile = ?3",
            (failure_kind, file_id, profile),
        )?;
        return Ok(true);
    }

    reset_thumbnail_after_stale_source_in_transaction(transaction, file_id, profile, error)?;
    Ok(false)
}

fn find_folder_id_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    root_id: i64,
    parent_id: Option<i64>,
    name: &str,
) -> anyhow::Result<Option<i64>> {
    if let Some(parent_id) = parent_id {
        let mut statement = transaction.prepare(
            r#"
            SELECT id FROM folders
            WHERE root_id = ?1 AND parent_id = ?2 AND name = ?3
            "#,
        )?;
        let mut rows = statement.query((root_id, parent_id, name))?;
        Ok(rows.next()?.map(|row| row.get(0)).transpose()?)
    } else {
        let mut statement = transaction.prepare(
            r#"
            SELECT id FROM folders
            WHERE root_id = ?1 AND parent_id IS NULL AND name = ?2
            "#,
        )?;
        let mut rows = statement.query((root_id, name))?;
        Ok(rows.next()?.map(|row| row.get(0)).transpose()?)
    }
}

fn root_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RootRecord> {
    Ok(RootRecord {
        id: row.get(0)?,
        path: row.get(1)?,
        display_name: row.get(2)?,
        enabled: row.get::<_, i64>(3)? != 0,
        created_at: row.get(4)?,
        last_scan_at: row.get(5)?,
        root_folder_id: row.get(6)?,
        media_count: row.get(7)?,
        reachable: true,
    })
}

fn media_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaRecord> {
    let id: i64 = row.get(0)?;
    let root_id: i64 = row.get(1)?;
    let folder_id: i64 = row.get(2)?;
    let name: String = row.get(3)?;
    let ext: String = row.get(4)?;
    let size: i64 = row.get(5)?;
    let mtime: i64 = row.get(6)?;
    let kind: Option<String> = row.get(7)?;
    let width: Option<i64> = row.get(8)?;
    let height: Option<i64> = row.get(9)?;
    let duration_ms: Option<i64> = row.get(10)?;
    let codec: Option<String> = row.get(11)?;
    let preview_placeholder: Option<Vec<u8>> = row.get(12)?;
    let preview_placeholder_format: Option<String> = row.get(13)?;
    let source = ThumbnailSourceRecord {
        file_id: id,
        root_id,
        folder_id,
        name: name.clone(),
        size,
        mtime,
        file_key: row.get(15)?,
        media_kind: kind.clone(),
        width,
        height,
        metadata_status: row.get(14)?,
    };
    let (thumbnail_state, thumbnail_cache_key) = media_thumbnail_summary_from_row(row, &source)?;

    Ok(MediaRecord {
        id,
        root_id,
        folder_id,
        name,
        ext,
        size,
        mtime,
        kind,
        width,
        height,
        duration_ms,
        codec,
        preview_placeholder,
        preview_placeholder_format,
        thumbnail_state,
        thumbnail_cache_key,
        rating: None,
        favorite: false,
        note: None,
        tag_ids: Vec::new(),
    })
}

fn media_thumbnail_summary_from_row(
    row: &rusqlite::Row<'_>,
    source: &ThumbnailSourceRecord,
) -> rusqlite::Result<(Option<String>, Option<String>)> {
    let Some(state) = row.get(17)? else {
        return Ok((None, None));
    };
    let mut thumbnail = ThumbnailRecord {
        file_id: source.file_id,
        profile: row
            .get::<_, Option<String>>(16)?
            .unwrap_or_else(|| GRID_320_PROFILE.to_string()),
        state,
        short_side_px: row
            .get::<_, Option<i64>>(18)?
            .unwrap_or(GRID_320_SHORT_SIDE_PX),
        output_format: row
            .get::<_, Option<String>>(19)?
            .unwrap_or_else(|| GENERATED_FORMAT.to_string()),
        cache_key: row.get(20)?,
        width: row.get(21)?,
        height: row.get(22)?,
        byte_size: row.get(23)?,
        error: row.get(24)?,
        source_fingerprint: row.get(25)?,
        served_by: row.get(26)?,
        updated_at: row.get(27)?,
    };
    normalize_thumbnail_record_for_source(&mut thumbnail, source);
    Ok((Some(thumbnail.state), None))
}

fn thumbnail_source_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThumbnailSourceRecord> {
    Ok(ThumbnailSourceRecord {
        file_id: row.get(0)?,
        root_id: row.get(1)?,
        folder_id: row.get(2)?,
        name: row.get(3)?,
        size: row.get(4)?,
        mtime: row.get(5)?,
        file_key: row.get(6)?,
        media_kind: row.get(7)?,
        width: row.get(8)?,
        height: row.get(9)?,
        metadata_status: row.get(10)?,
    })
}

fn normalize_thumbnail_record_for_source(
    thumbnail: &mut ThumbnailRecord,
    source: &ThumbnailSourceRecord,
) {
    if thumbnail.source_fingerprint.is_some()
        && !thumbnail_source_fingerprint_matches(thumbnail, source)
    {
        reset_thumbnail_record_to_pending(thumbnail);
        return;
    }
    if thumbnail.state == "ready" && !thumbnail_source_fingerprint_matches(thumbnail, source) {
        reset_thumbnail_record_to_pending(thumbnail);
    }
    if thumbnail.state == "failed" && !failed_is_current_for_source(thumbnail, source) {
        reset_thumbnail_record_to_pending(thumbnail);
    }
    if thumbnail.state == "skipped_small" && !skipped_small_is_current_for_source(thumbnail, source)
    {
        reset_thumbnail_record_to_pending(thumbnail);
    }
}

fn thumbnail_state_is_terminal_for_current_source(
    thumbnail: &ThumbnailRecord,
    source: &ThumbnailSourceRecord,
    source_fingerprint: &str,
    ready_has_required_blob: bool,
) -> bool {
    if thumbnail.source_fingerprint.as_deref() != Some(source_fingerprint) {
        return false;
    }
    match thumbnail.state.as_str() {
        "ready" => ready_has_required_blob,
        "failed" => true,
        "skipped_small" => skipped_small_is_current_for_source(thumbnail, source),
        _ => false,
    }
}

fn failed_is_current_for_source(
    thumbnail: &ThumbnailRecord,
    source: &ThumbnailSourceRecord,
) -> bool {
    thumbnail_source_fingerprint_matches(thumbnail, source)
}

fn skipped_small_is_current_for_source(
    thumbnail: &ThumbnailRecord,
    source: &ThumbnailSourceRecord,
) -> bool {
    thumbnail_source_fingerprint_matches(thumbnail, source)
        && source.has_reliable_dimensions()
        && ThumbnailPolicy::grid_320().initial_state(
            source.media_kind.as_deref(),
            source.width,
            source.height,
        ) == ThumbnailDecision::SkippedSmall
}

fn thumbnail_source_fingerprint_matches(
    thumbnail: &ThumbnailRecord,
    source: &ThumbnailSourceRecord,
) -> bool {
    thumbnail.source_fingerprint.as_deref()
        == Some(
            source
                .source_fingerprint(thumbnail.profile.as_str())
                .as_str(),
        )
}

fn reset_thumbnail_record_to_pending(thumbnail: &mut ThumbnailRecord) {
    thumbnail.state = "pending".to_string();
    thumbnail.cache_key = None;
    thumbnail.width = None;
    thumbnail.height = None;
    thumbnail.byte_size = None;
    thumbnail.error = None;
    thumbnail.source_fingerprint = None;
    thumbnail.served_by = None;
}

fn thumbnail_from_row(
    row: &rusqlite::Row<'_>,
    requested_profile: &str,
) -> rusqlite::Result<ThumbnailRecord> {
    let profile: Option<String> = row.get(1)?;
    let state: Option<String> = row.get(2)?;
    let short_side_px: Option<i64> = row.get(3)?;
    let output_format: Option<String> = row.get(4)?;
    let _cache_key: Option<String> = row.get(5)?;
    let source_fingerprint: Option<String> = row.get(10)?;
    let served_by: Option<String> = row.get(11)?;
    let state = match state {
        Some(state)
            if matches!(state.as_str(), "ready" | "skipped_small")
                && source_fingerprint.is_none() =>
        {
            "pending".to_string()
        }
        Some(state) => state,
        None => "pending".to_string(),
    };
    let metadata_is_usable = state == "ready" && source_fingerprint.is_some();
    Ok(ThumbnailRecord {
        file_id: row.get(0)?,
        profile: profile.unwrap_or_else(|| requested_profile.to_string()),
        state,
        short_side_px: short_side_px.unwrap_or(GRID_320_SHORT_SIDE_PX),
        output_format: output_format.unwrap_or_else(|| GENERATED_FORMAT.to_string()),
        cache_key: None,
        width: if metadata_is_usable {
            row.get(6)?
        } else {
            None
        },
        height: if metadata_is_usable {
            row.get(7)?
        } else {
            None
        },
        byte_size: if metadata_is_usable {
            row.get(8)?
        } else {
            None
        },
        error: row.get(9)?,
        source_fingerprint,
        served_by: if metadata_is_usable { served_by } else { None },
        updated_at: row.get(12)?,
    })
}

fn thumb_blob_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThumbBlobRecord> {
    Ok(ThumbBlobRecord {
        file_id: row.get(0)?,
        profile: row.get(1)?,
        data: row.get(2)?,
        width: row.get(3)?,
        height: row.get(4)?,
        byte_size: row.get(5)?,
        output_format: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn preview_blob_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PreviewBlobRecord> {
    Ok(PreviewBlobRecord {
        file_id: row.get(0)?,
        profile: row.get(1)?,
        data: row.get(2)?,
        width: row.get(3)?,
        height: row.get(4)?,
        byte_size: row.get(5)?,
        output_format: row.get(6)?,
        source_size: row.get(7)?,
        source_mtime: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn preview_failure_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PreviewFailureRecord> {
    Ok(PreviewFailureRecord {
        file_id: row.get(0)?,
        profile: row.get(1)?,
        failure_kind: row.get(2)?,
        error: row.get(3)?,
        source_size: row.get(4)?,
        source_mtime: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn thumb_blob_exists_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
    profile: &str,
) -> anyhow::Result<bool> {
    transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM thumb_blobs WHERE file_id = ?1 AND profile = ?2)",
            (file_id, profile),
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn upsert_thumb_blob_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    blob: &ThumbBlobRecord,
) -> anyhow::Result<()> {
    transaction.execute(
        r#"
        INSERT INTO thumb_blobs(
            file_id, profile, data, width, height, byte_size, output_format, created_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(file_id, profile) DO UPDATE SET
            data = excluded.data,
            width = excluded.width,
            height = excluded.height,
            byte_size = excluded.byte_size,
            output_format = excluded.output_format,
            updated_at = excluded.updated_at
        "#,
        (
            blob.file_id,
            &blob.profile,
            &blob.data,
            blob.width,
            blob.height,
            blob.byte_size,
            &blob.output_format,
            blob.created_at,
            blob.updated_at,
        ),
    )?;
    Ok(())
}

#[allow(dead_code)]
fn is_safe_thumbnail_cache_key(cache_key: &str) -> bool {
    is_safe_cache_key(cache_key)
}

#[allow(dead_code)]
fn is_importable_webp(data: &[u8]) -> bool {
    // Validate the RIFF/WebP container and that the header parses (dimensions
    // are readable) WITHOUT a full pixel decode. The legacy cache was written
    // by our own pipeline, so a header check guards against truncated/corrupt
    // files at one-time startup import while keeping the migration cheap even
    // for million-file libraries (a full decode per file dominated startup).
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return false;
    }
    image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        .is_some()
}

fn dedupe_scope_file_ids(file_ids: &[i64]) -> HashSet<i64> {
    file_ids
        .iter()
        .copied()
        .filter(|file_id| *file_id > 0)
        .collect()
}

fn thumbnail_scope_priority_for_file(
    file_id: i64,
    selected_file_ids: &HashSet<i64>,
    visible_file_ids: &HashSet<i64>,
    ahead_file_ids: &HashSet<i64>,
) -> i64 {
    if selected_file_ids.contains(&file_id) {
        THUMBNAIL_SELECTED_PRIORITY
    } else if visible_file_ids.contains(&file_id) {
        THUMBNAIL_VISIBLE_PRIORITY
    } else if ahead_file_ids.contains(&file_id) {
        THUMBNAIL_AHEAD_PRIORITY
    } else {
        THUMBNAIL_BACKGROUND_PRIORITY
    }
}

#[allow(dead_code)]
fn invalidate_legacy_thumbnail_import_candidate(
    connection: &rusqlite::Connection,
    file_id: i64,
    profile: &str,
    error: &str,
) -> anyhow::Result<()> {
    connection.execute(
        r#"
        UPDATE thumbs
        SET state = 'pending',
            cache_key = NULL,
            width = NULL,
            height = NULL,
            byte_size = NULL,
            error = ?3,
            source_fingerprint = NULL,
            served_by = NULL,
            updated_at = ?4
        WHERE file_id = ?1 AND profile = ?2
        "#,
        (file_id, profile, error, unix_timestamp()),
    )?;
    connection.execute(
        "DELETE FROM thumb_blobs WHERE file_id = ?1 AND profile = ?2",
        (file_id, profile),
    )?;
    Ok(())
}

pub(crate) fn cleanup_inactive_thumbnail_artifacts_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
) -> anyhow::Result<()> {
    transaction.execute(
        r#"
        DELETE FROM thumb_blobs
        WHERE EXISTS (
          SELECT 1
          FROM files
          JOIN roots ON roots.id = files.root_id
          WHERE files.id = thumb_blobs.file_id
            AND (files.status <> 'active' OR roots.enabled = 0)
        )
        "#,
        [],
    )?;
    transaction.execute(
        r#"
        DELETE FROM thumbs
        WHERE EXISTS (
          SELECT 1
          FROM files
          JOIN roots ON roots.id = files.root_id
          WHERE files.id = thumbs.file_id
            AND (files.status <> 'active' OR roots.enabled = 0)
        )
        "#,
        [],
    )?;
    // Preview tiers are reclaimed on the same terms as grid thumbs.
    //
    // Recycling files or disabling a root flips `files.status` WITHOUT deleting
    // the row, so the FK cascade never fires. Omitting this left 1-3 MB tiers
    // for unreachable files occupying the preview cache budget, where they
    // evicted tiers for images the user is actually looking at.
    transaction.execute(
        r#"
        DELETE FROM preview_blobs
        WHERE EXISTS (
          SELECT 1
          FROM files
          JOIN roots ON roots.id = files.root_id
          WHERE files.id = preview_blobs.file_id
            AND (files.status <> 'active' OR roots.enabled = 0)
        )
        "#,
        [],
    )?;
    transaction.execute(
        r#"
        DELETE FROM preview_failures
        WHERE EXISTS (
          SELECT 1
          FROM files
          JOIN roots ON roots.id = files.root_id
          WHERE files.id = preview_failures.file_id
            AND (files.status <> 'active' OR roots.enabled = 0)
        )
        "#,
        [],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThumbnailCacheStatus {
    Cached,
    Missing,
    Stale,
    Failed,
    Skipped,
}

struct ThumbnailCacheScopeQueryParts {
    prefix: String,
    where_clause: String,
    params: Vec<Value>,
}

fn register_thumbnail_cache_sql_functions(connection: &Connection) -> anyhow::Result<()> {
    connection.create_scalar_function(
        "megle_thumb_fingerprint",
        8,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let name: String = ctx.get(3)?;
            let file_key: Option<String> = ctx.get(6)?;
            let profile: String = ctx.get(7)?;
            let identity = CacheIdentity {
                file_id: ctx.get(0)?,
                root_id: ctx.get(1)?,
                folder_id: ctx.get(2)?,
                name: name.as_str(),
                size: ctx.get(4)?,
                mtime: ctx.get(5)?,
                file_key: file_key.as_deref(),
            };
            Ok(source_fingerprint_for(&identity, profile.as_str()))
        },
    )?;
    Ok(())
}

fn thumbnail_cache_scope_query_parts(scope: ThumbnailCacheScope) -> ThumbnailCacheScopeQueryParts {
    let mut params = Vec::new();
    let mut where_clauses = vec![
        "files.status = 'active'".to_string(),
        "roots.enabled = 1".to_string(),
        "COALESCE(media.kind, '') IN ('image', 'video')".to_string(),
    ];
    let mut descendant_folder_scope = false;
    let prefix = if let Some(folder_id) = scope.folder_id {
        if scope.include_descendants {
            descendant_folder_scope = true;
            params.push(Value::Integer(folder_id));
            where_clauses.push("files.folder_id IN (SELECT id FROM scoped_folders)".to_string());
            "WITH RECURSIVE scoped_folders(id) AS (
                SELECT id FROM folders WHERE id = ?1
                UNION ALL
                SELECT folders.id
                FROM folders
                JOIN scoped_folders ON folders.parent_id = scoped_folders.id
            )"
            .to_string()
        } else {
            params.push(Value::Integer(folder_id));
            where_clauses.push(format!("files.folder_id = ?{}", params.len()));
            String::new()
        }
    } else {
        String::new()
    };
    // A descendant folder scope already confines the query to one root's
    // subtree (folders never span roots), so `files.root_id = ?` is redundant
    // here. Worse, keeping it lets SQLite's planner pick the whole-root index
    // (`idx_files_root_status_*`) and scan every active file in the root
    // (millions), bloom-filtering down to the handful in scope — a
    // multi-second scan. Omitting it steers the planner onto the folder index
    // so it touches only the scoped files. See `push_root_id_scope`.
    if !descendant_folder_scope {
        push_root_id_scope(scope, &mut params, &mut where_clauses);
    }
    ThumbnailCacheScopeQueryParts {
        prefix,
        where_clause: where_clauses.join(" AND "),
        params,
    }
}

/// Append `files.root_id = ?` to a thumbnail-cache scope query unless the scope
/// has no root filter. Callers omit this for descendant folder scopes, where
/// the root predicate is redundant and harms the query plan (see
/// [`thumbnail_cache_scope_query_parts`]).
fn push_root_id_scope(
    scope: ThumbnailCacheScope,
    params: &mut Vec<Value>,
    where_clauses: &mut Vec<String>,
) {
    if let Some(root_id) = scope.root_id {
        params.push(Value::Integer(root_id));
        where_clauses.push(format!("files.root_id = ?{}", params.len()));
    }
}

fn thumbnail_cache_active_task_scope_query_parts(
    scope: ThumbnailCacheScope,
) -> ThumbnailCacheScopeQueryParts {
    let mut params = Vec::new();
    let mut where_clauses = vec![
        "files.status = 'active'".to_string(),
        "roots.enabled = 1".to_string(),
    ];
    let mut descendant_folder_scope = false;
    let prefix = if let Some(folder_id) = scope.folder_id {
        if scope.include_descendants {
            descendant_folder_scope = true;
            params.push(Value::Integer(folder_id));
            where_clauses.push("files.folder_id IN (SELECT id FROM scoped_folders)".to_string());
            "WITH RECURSIVE scoped_folders(id) AS (
                SELECT id FROM folders WHERE id = ?1
                UNION ALL
                SELECT folders.id
                FROM folders
                JOIN scoped_folders ON folders.parent_id = scoped_folders.id
            )"
            .to_string()
        } else {
            params.push(Value::Integer(folder_id));
            where_clauses.push(format!("files.folder_id = ?{}", params.len()));
            String::new()
        }
    } else {
        String::new()
    };
    // Redundant with a descendant folder scope and harmful to the plan — see
    // `thumbnail_cache_scope_query_parts`.
    if !descendant_folder_scope {
        push_root_id_scope(scope, &mut params, &mut where_clauses);
    }
    ThumbnailCacheScopeQueryParts {
        prefix,
        where_clause: where_clauses.join(" AND "),
        params,
    }
}

fn thumbnail_cache_current_fingerprint_sql() -> String {
    format!(
        "megle_thumb_fingerprint(files.id, files.root_id, files.folder_id, files.name, files.size, files.mtime, files.file_key, COALESCE(thumbs.profile, '{GRID_320_PROFILE}'))"
    )
}

fn thumbnail_cache_skipped_small_sql() -> String {
    format!(
        "media.metadata_status = 'ready' AND media.kind = 'image' AND media.width < {GRID_320_SHORT_SIDE_PX} AND media.height < {GRID_320_SHORT_SIDE_PX}"
    )
}

fn thumbnail_cache_status_sql_expression() -> String {
    thumbnail_cache_status_sql_expression_with_columns(&ThumbnailCacheStatusColumns::direct())
}

/// The column references the cache-status CASE expression reads. Callers that
/// precompute the current fingerprint once per row (the stats query) supply
/// aliased column names pointing at an inner subquery; the default uses the live
/// table columns and the per-row UDF.
struct ThumbnailCacheStatusColumns {
    thumb_state: String,
    thumb_source_fingerprint: String,
    thumb_blob_present: String,
    current_fingerprint: String,
    skipped_small: String,
}

impl ThumbnailCacheStatusColumns {
    /// Read directly from the joined tables, computing the fingerprint UDF inline
    /// (re-invoked per comparison). Used by the candidate/pending queries which
    /// are already bounded (LIMIT / single row) so the repeated UDF is cheap.
    fn direct() -> Self {
        Self {
            thumb_state: "thumbs.state".to_string(),
            thumb_source_fingerprint: "thumbs.source_fingerprint".to_string(),
            thumb_blob_present: "thumb_blobs.file_id IS NOT NULL".to_string(),
            current_fingerprint: thumbnail_cache_current_fingerprint_sql(),
            skipped_small: thumbnail_cache_skipped_small_sql(),
        }
    }
}

fn thumbnail_cache_status_sql_expression_with_columns(
    columns: &ThumbnailCacheStatusColumns,
) -> String {
    let ThumbnailCacheStatusColumns {
        thumb_state,
        thumb_source_fingerprint,
        thumb_blob_present,
        current_fingerprint,
        skipped_small,
    } = columns;
    format!(
        r#"
        CASE
          WHEN {skipped_small} THEN 'skipped'
          WHEN {thumb_state} IS NULL THEN 'missing'
          WHEN {thumb_state} = 'ready' THEN
            CASE
              WHEN {thumb_source_fingerprint} = {current_fingerprint} AND {thumb_blob_present} THEN 'cached'
              WHEN {thumb_source_fingerprint} = {current_fingerprint} THEN 'missing'
              ELSE 'stale'
            END
          WHEN {thumb_state} = 'failed' THEN
            CASE
              WHEN {thumb_source_fingerprint} = {current_fingerprint} THEN 'failed'
              ELSE 'stale'
            END
          WHEN {thumb_state} = 'skipped_small' THEN
            CASE
              WHEN {thumb_source_fingerprint} = {current_fingerprint} AND {skipped_small} THEN 'skipped'
              ELSE 'stale'
            END
          WHEN {thumb_state} IN ('pending', 'queued') THEN
            CASE
              WHEN {thumb_source_fingerprint} = {current_fingerprint} THEN 'missing'
              ELSE 'stale'
            END
          ELSE 'missing'
        END
        "#
    )
}

fn thumbnail_cache_refresh_mode_sql_condition(refresh_mode: ThumbnailCacheRefreshMode) -> String {
    match refresh_mode {
        ThumbnailCacheRefreshMode::MissingOnly => "cache_status = 'missing'".to_string(),
        ThumbnailCacheRefreshMode::StaleOrMissing => {
            "cache_status IN ('missing', 'stale')".to_string()
        }
        ThumbnailCacheRefreshMode::RetryFailedAndStale => {
            // Automatic bulk retry candidates: `failed` only qualifies when the
            // stored failure was classified `transient` (NAS blip, timeout,
            // etc). `permanent` failures (bad format, corrupt file, ...) never
            // enter the automatic candidate set — they still surface to the
            // user and remain retryable via the explicit single-task
            // `retry_task` path, which does not go through this query.
            "(cache_status IN ('missing', 'stale') OR (cache_status = 'failed' AND failure_kind = 'transient'))".to_string()
        }
        ThumbnailCacheRefreshMode::RetryTransientFailedOnly => {
            // Sweep-only: transient failures, nothing else. Excludes missing and
            // stale so the background sweep never bulk-generates the backlog.
            "(cache_status = 'failed' AND failure_kind = 'transient')".to_string()
        }
    }
}

fn thumbnail_cache_candidate_query(
    scope: ThumbnailCacheScope,
    refresh_mode: ThumbnailCacheRefreshMode,
    limit: usize,
) -> (String, Vec<Value>) {
    let mut parts = thumbnail_cache_scope_query_parts(scope);
    let status_expression = thumbnail_cache_status_sql_expression();
    let refresh_condition = thumbnail_cache_refresh_mode_sql_condition(refresh_mode);
    let terminal_current_ready = "COALESCE(thumb_state = 'ready' AND source_fingerprint = current_source_fingerprint AND has_blob = 1, 0)";
    // "Already has an active bulk-or-higher task" — this enqueue runs at
    // `THUMBNAIL_BULK_PRIORITY`, which is the *lowest* priority, so any active
    // (pending/running) thumbnail task for the file is by definition at
    // bulk-or-higher priority. We therefore DO NOT add a `priority >= ...`
    // predicate: a `priority >= THUMBNAIL_BULK_PRIORITY` filter is always true
    // yet forces SQLite off the `(kind, file_id, status)` covering index and
    // onto `(kind, status, priority)`, which lacks `file_id` — turning this
    // correlated subquery into a full scan of every pending thumbnail task per
    // scoped file (seconds on a large library). Omitting it keeps the O(1)
    // per-file index seek. If a priority tier below bulk is ever introduced,
    // reinstate an explicit filter here.
    debug_assert_eq!(
        THUMBNAIL_BULK_PRIORITY,
        [
            THUMBNAIL_BULK_PRIORITY,
            THUMBNAIL_BACKGROUND_PRIORITY,
            THUMBNAIL_AHEAD_PRIORITY,
            THUMBNAIL_VISIBLE_PRIORITY,
            THUMBNAIL_SELECTED_PRIORITY,
        ]
        .into_iter()
        .min()
        .unwrap(),
        "bulk must remain the lowest thumbnail priority for this query's index assumption",
    );
    let active_equal_or_higher_priority_task = r#"
        EXISTS (
          SELECT 1
          FROM tasks active_tasks
          WHERE active_tasks.kind = 'thumbnail'
            AND active_tasks.file_id = scoped_entries.file_id
            AND active_tasks.status IN ('pending', 'running')
        )
        "#
    .to_string();
    parts.params.push(Value::Integer(limit as i64));
    let limit_param = parts.params.len();
    let select = format!(
        r#"
        SELECT
            file_id,
            root_id,
            folder_id,
            name,
            size,
            mtime,
            file_key,
            media_kind,
            media_width,
            media_height,
            metadata_status,
            thumb_profile,
            thumb_state,
            short_side_px,
            output_format,
            cache_key,
            thumb_width,
            thumb_height,
            thumb_byte_size,
            error,
            source_fingerprint,
            served_by,
            updated_at,
            has_blob,
            blob_byte_size
        FROM (
            SELECT
                files.id AS file_id,
                files.root_id AS root_id,
                files.folder_id AS folder_id,
                files.name AS name,
                files.size AS size,
                files.mtime AS mtime,
                files.file_key AS file_key,
                media.kind AS media_kind,
                media.width AS media_width,
                media.height AS media_height,
                media.metadata_status AS metadata_status,
                thumbs.profile AS thumb_profile,
                thumbs.state AS thumb_state,
                thumbs.short_side_px AS short_side_px,
                thumbs.output_format AS output_format,
                thumbs.cache_key AS cache_key,
                thumbs.width AS thumb_width,
                thumbs.height AS thumb_height,
                thumbs.byte_size AS thumb_byte_size,
                thumbs.error AS error,
                thumbs.source_fingerprint AS source_fingerprint,
                thumbs.served_by AS served_by,
                thumbs.updated_at AS updated_at,
                thumbs.failure_kind AS failure_kind,
                CASE WHEN thumb_blobs.file_id IS NULL THEN 0 ELSE 1 END AS has_blob,
                COALESCE(thumb_blobs.byte_size, 0) AS blob_byte_size,
                {status_expression} AS cache_status,
                {current_fingerprint} AS current_source_fingerprint
            FROM files
            JOIN roots ON roots.id = files.root_id
            LEFT JOIN media ON media.file_id = files.id
            LEFT JOIN thumbs ON thumbs.file_id = files.id AND thumbs.profile = '{GRID_320_PROFILE}'
            LEFT JOIN thumb_blobs ON thumb_blobs.file_id = files.id AND thumb_blobs.profile = '{GRID_320_PROFILE}'
            WHERE {where_clause}
        ) scoped_entries
        WHERE {refresh_condition}
          AND NOT ({terminal_current_ready})
          AND NOT ({active_equal_or_higher_priority_task})
        ORDER BY file_id ASC
        LIMIT ?{limit_param}
        "#,
        current_fingerprint = thumbnail_cache_current_fingerprint_sql(),
        where_clause = parts.where_clause
    );
    let sql = if parts.prefix.is_empty() {
        select
    } else {
        format!("{} {select}", parts.prefix)
    };
    (sql, parts.params)
}

fn thumbnail_cache_pending_bulk_task_query(
    scope: ThumbnailCacheScope,
    refresh_mode: ThumbnailCacheRefreshMode,
) -> (String, Vec<Value>) {
    let parts = thumbnail_cache_scope_query_parts(scope);
    let status_expression = thumbnail_cache_status_sql_expression();
    let refresh_condition = thumbnail_cache_refresh_mode_sql_condition(refresh_mode);
    let select = format!(
        r#"
        SELECT active_tasks.id, scoped_entries.file_id, scoped_entries.cache_status
        FROM (
            SELECT
                files.id AS file_id,
                {status_expression} AS cache_status,
                thumbs.failure_kind AS failure_kind
            FROM files
            JOIN roots ON roots.id = files.root_id
            LEFT JOIN media ON media.file_id = files.id
            LEFT JOIN thumbs ON thumbs.file_id = files.id AND thumbs.profile = '{GRID_320_PROFILE}'
            LEFT JOIN thumb_blobs ON thumb_blobs.file_id = files.id AND thumb_blobs.profile = '{GRID_320_PROFILE}'
            WHERE {where_clause}
        ) scoped_entries
        JOIN tasks active_tasks ON active_tasks.file_id = scoped_entries.file_id
        WHERE {refresh_condition}
          AND active_tasks.kind = 'thumbnail'
          AND active_tasks.status = 'pending'
          AND active_tasks.priority = {THUMBNAIL_BULK_PRIORITY}
        ORDER BY active_tasks.created_at ASC, active_tasks.id ASC
        LIMIT 1
        "#,
        where_clause = parts.where_clause
    );
    let sql = if parts.prefix.is_empty() {
        select
    } else {
        format!("{} {select}", parts.prefix)
    };
    (sql, parts.params)
}

fn thumbnail_cache_stats_query(scope: ThumbnailCacheScope) -> (String, Vec<Value>) {
    let parts = thumbnail_cache_scope_query_parts(scope);
    // Compute the current source fingerprint exactly ONCE per row (only for rows
    // that actually have a thumb row — `missing` rows, the overwhelming majority
    // in a cold library, skip the UDF entirely via the CASE guard). The status
    // CASE then compares against this precomputed column instead of re-invoking
    // the SHA256 UDF ~3-6 times per row. On a 650k-file library this turned a
    // ~53s whole-library stats query (237k+ UDF calls) into a ~0.5s scan.
    let fingerprint_column = format!(
        "CASE WHEN thumbs.state IS NOT NULL THEN {} END",
        thumbnail_cache_current_fingerprint_sql()
    );
    let status_columns = ThumbnailCacheStatusColumns {
        thumb_state: "thumb_state".to_string(),
        thumb_source_fingerprint: "thumb_source_fingerprint".to_string(),
        thumb_blob_present: "thumb_blob_present".to_string(),
        current_fingerprint: "current_source_fingerprint".to_string(),
        skipped_small: "is_skipped_small".to_string(),
    };
    let status_expression = thumbnail_cache_status_sql_expression_with_columns(&status_columns);
    let skipped_small = thumbnail_cache_skipped_small_sql();
    let select = format!(
        r#"
        SELECT
            COALESCE(SUM(CASE WHEN cache_status = 'cached' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN cache_status = 'missing' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN cache_status = 'stale' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN cache_status = 'failed' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(blob_byte_size), 0)
        FROM (
            SELECT
                {status_expression} AS cache_status,
                blob_byte_size
            FROM (
                SELECT
                    thumbs.state AS thumb_state,
                    thumbs.source_fingerprint AS thumb_source_fingerprint,
                    (thumb_blobs.file_id IS NOT NULL) AS thumb_blob_present,
                    ({skipped_small}) AS is_skipped_small,
                    {fingerprint_column} AS current_source_fingerprint,
                    COALESCE(thumb_blobs.byte_size, 0) AS blob_byte_size
                FROM files
                JOIN roots ON roots.id = files.root_id
                LEFT JOIN media ON media.file_id = files.id
                LEFT JOIN thumbs ON thumbs.file_id = files.id AND thumbs.profile = '{GRID_320_PROFILE}'
                LEFT JOIN thumb_blobs ON thumb_blobs.file_id = files.id AND thumb_blobs.profile = '{GRID_320_PROFILE}'
                WHERE {where_clause}
            ) scoped_rows
        ) scoped_entries
        "#,
        where_clause = parts.where_clause
    );
    let sql = if parts.prefix.is_empty() {
        select
    } else {
        format!("{} {select}", parts.prefix)
    };
    (sql, parts.params)
}

fn thumbnail_cache_active_bulk_task_count_query(
    scope: ThumbnailCacheScope,
) -> (String, Vec<Value>) {
    let parts = thumbnail_cache_active_task_scope_query_parts(scope);
    let select = format!(
        r#"
        SELECT COUNT(*)
        FROM tasks
        JOIN files ON files.id = tasks.file_id
        JOIN roots ON roots.id = files.root_id
        WHERE tasks.kind = 'thumbnail'
          AND tasks.priority = {THUMBNAIL_BULK_PRIORITY}
          AND tasks.status IN ('pending', 'running')
          AND {where_clause}
        "#,
        where_clause = parts.where_clause
    );
    let sql = if parts.prefix.is_empty() {
        select
    } else {
        format!("{} {select}", parts.prefix)
    };
    (sql, parts.params)
}

fn thumbnail_cache_entry_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ThumbnailCacheEntry> {
    let source = ThumbnailSourceRecord {
        file_id: row.get(0)?,
        root_id: row.get(1)?,
        folder_id: row.get(2)?,
        name: row.get(3)?,
        size: row.get(4)?,
        mtime: row.get(5)?,
        file_key: row.get(6)?,
        media_kind: row.get(7)?,
        width: row.get(8)?,
        height: row.get(9)?,
        metadata_status: row.get(10)?,
    };
    let thumbnail = if let Some(state) = row.get::<_, Option<String>>(12)? {
        Some(ThumbnailRecord {
            file_id: source.file_id,
            profile: row
                .get::<_, Option<String>>(11)?
                .unwrap_or_else(|| GRID_320_PROFILE.to_string()),
            state,
            short_side_px: row
                .get::<_, Option<i64>>(13)?
                .unwrap_or(GRID_320_SHORT_SIDE_PX),
            output_format: row
                .get::<_, Option<String>>(14)?
                .unwrap_or_else(|| GENERATED_FORMAT.to_string()),
            cache_key: row.get(15)?,
            width: row.get(16)?,
            height: row.get(17)?,
            byte_size: row.get(18)?,
            error: row.get(19)?,
            source_fingerprint: row.get(20)?,
            served_by: row.get(21)?,
            updated_at: row.get(22)?,
        })
    } else {
        None
    };
    Ok(ThumbnailCacheEntry {
        source,
        thumbnail,
        has_blob: row.get::<_, i64>(23)? != 0,
        blob_byte_size: row.get(24)?,
    })
}

fn thumbnail_cache_status(entry: &ThumbnailCacheEntry) -> ThumbnailCacheStatus {
    if entry.source.has_reliable_dimensions()
        && ThumbnailPolicy::grid_320().initial_state(
            entry.source.media_kind.as_deref(),
            entry.source.width,
            entry.source.height,
        ) == ThumbnailDecision::SkippedSmall
    {
        if entry
            .thumbnail
            .as_ref()
            .is_some_and(|thumbnail| skipped_small_is_current_for_source(thumbnail, &entry.source))
        {
            return ThumbnailCacheStatus::Skipped;
        }
        return ThumbnailCacheStatus::Skipped;
    }

    let Some(thumbnail) = entry.thumbnail.as_ref() else {
        return ThumbnailCacheStatus::Missing;
    };

    match thumbnail.state.as_str() {
        "ready" => {
            if thumbnail_source_fingerprint_matches(thumbnail, &entry.source) && entry.has_blob {
                ThumbnailCacheStatus::Cached
            } else if thumbnail_source_fingerprint_matches(thumbnail, &entry.source) {
                ThumbnailCacheStatus::Missing
            } else {
                ThumbnailCacheStatus::Stale
            }
        }
        "failed" => {
            if failed_is_current_for_source(thumbnail, &entry.source) {
                ThumbnailCacheStatus::Failed
            } else {
                ThumbnailCacheStatus::Stale
            }
        }
        "skipped_small" => {
            if skipped_small_is_current_for_source(thumbnail, &entry.source) {
                ThumbnailCacheStatus::Skipped
            } else {
                ThumbnailCacheStatus::Stale
            }
        }
        "pending" | "queued" => {
            if thumbnail_source_fingerprint_matches(thumbnail, &entry.source) {
                ThumbnailCacheStatus::Missing
            } else {
                ThumbnailCacheStatus::Stale
            }
        }
        _ => ThumbnailCacheStatus::Missing,
    }
}

fn thumbnail_cache_entry_matches_refresh_mode(
    entry: &ThumbnailCacheEntry,
    refresh_mode: ThumbnailCacheRefreshMode,
) -> bool {
    match (refresh_mode, thumbnail_cache_status(entry)) {
        (_, ThumbnailCacheStatus::Cached | ThumbnailCacheStatus::Skipped) => false,
        (ThumbnailCacheRefreshMode::MissingOnly, ThumbnailCacheStatus::Missing) => true,
        (ThumbnailCacheRefreshMode::StaleOrMissing, ThumbnailCacheStatus::Missing)
        | (ThumbnailCacheRefreshMode::StaleOrMissing, ThumbnailCacheStatus::Stale) => true,
        (ThumbnailCacheRefreshMode::RetryFailedAndStale, ThumbnailCacheStatus::Missing)
        | (ThumbnailCacheRefreshMode::RetryFailedAndStale, ThumbnailCacheStatus::Stale)
        | (ThumbnailCacheRefreshMode::RetryFailedAndStale, ThumbnailCacheStatus::Failed) => true,
        // Sweep-only mode: transient failures exclusively (the `failure_kind`
        // gate is applied by the candidate SQL, so a `Failed` entry here is
        // already transient-classified).
        (ThumbnailCacheRefreshMode::RetryTransientFailedOnly, ThumbnailCacheStatus::Failed) => true,
        _ => false,
    }
}

#[cfg(test)]
fn get_thumbnail_cache_stats_for_scope(
    database: &Database,
    scope: ThumbnailCacheScope,
) -> anyhow::Result<ThumbnailCacheStats> {
    get_thumbnail_cache_stats_for_scope_on_connection(&database.connection, scope)
}

fn get_thumbnail_cache_stats_for_scope_on_connection(
    connection: &Connection,
    scope: ThumbnailCacheScope,
) -> anyhow::Result<ThumbnailCacheStats> {
    let (stats_sql, stats_params) = thumbnail_cache_stats_query(scope);
    let (cached_count, missing_count, stale_count, failed_count, total_blob_bytes) = connection
        .query_row(&stats_sql, params_from_iter(stats_params), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
    let (active_sql, active_params) = thumbnail_cache_active_bulk_task_count_query(scope);
    let active_bulk_task_count =
        connection.query_row(&active_sql, params_from_iter(active_params), |row| {
            row.get::<_, i64>(0)
        })?;
    Ok(ThumbnailCacheStats {
        cached_count,
        missing_count,
        stale_count,
        failed_count,
        pending_candidate_count: missing_count + stale_count,
        total_blob_bytes,
        active_bulk_task_count,
    })
}

fn thumbnail_failures(
    database: &Database,
    profile: &str,
    file_id: Option<i64>,
    scope: ThumbnailCacheScope,
    limit: usize,
) -> anyhow::Result<Vec<ThumbnailFailureRecord>> {
    thumbnail_failures_on_connection(&database.connection, profile, file_id, scope, limit)
}

fn thumbnail_failures_on_connection(
    connection: &Connection,
    profile: &str,
    file_id: Option<i64>,
    scope: ThumbnailCacheScope,
    limit: usize,
) -> anyhow::Result<Vec<ThumbnailFailureRecord>> {
    let mut parts = thumbnail_cache_scope_query_parts(scope);
    parts.params.push(Value::Text(profile.to_string()));
    let profile_parameter = parts.params.len();
    parts
        .params
        .push(file_id.map(Value::Integer).unwrap_or(Value::Null));
    let file_parameter = parts.params.len();
    parts.params.push(Value::Integer(limit as i64));
    let limit_parameter = parts.params.len();
    let cte_prefix = if parts.prefix.is_empty() {
        "WITH RECURSIVE".to_string()
    } else {
        format!("{},", parts.prefix)
    };
    let status_expression = thumbnail_cache_status_sql_expression();
    let sql = format!(
        r#"
        {cte_prefix} folder_paths(id, root_id, relative_path, depth) AS (
            SELECT folders.id, folders.root_id, '', 0
            FROM folders
            JOIN roots ON roots.id = folders.root_id AND roots.enabled = 1
            WHERE folders.parent_id IS NULL AND folders.status = 'active'
            UNION ALL
            SELECT child.id, child.root_id,
                   CASE
                     WHEN folder_paths.relative_path = '' THEN child.name
                     ELSE folder_paths.relative_path || '\' || child.name
                   END,
                   folder_paths.depth + 1
            FROM folder_paths
            JOIN folders child
             ON child.parent_id = folder_paths.id
             AND child.root_id = folder_paths.root_id
             AND child.status = 'active'
            WHERE folder_paths.depth + 1 < {FOLDER_ANCESTOR_DEPTH_CAP}
        )
        SELECT files.id, files.folder_id, files.root_id,
               roots.display_name ||
                 CASE WHEN folder_paths.relative_path = '' THEN ''
                      ELSE '\' || folder_paths.relative_path END ||
                 '\' || files.name,
               COALESCE(thumbs.failure_kind, 'unknown'),
               COALESCE(thumbs.error, 'thumbnail generation failed'),
               thumbs.updated_at
        FROM files
        JOIN roots ON roots.id = files.root_id
        JOIN media ON media.file_id = files.id
        JOIN thumbs ON thumbs.file_id = files.id
        LEFT JOIN thumb_blobs ON thumb_blobs.file_id = files.id AND thumb_blobs.profile = thumbs.profile
        JOIN folder_paths ON folder_paths.id = files.folder_id
        WHERE {where_clause}
          AND thumbs.profile = ?{profile_parameter}
          AND ({status_expression}) = 'failed'
          AND (?{file_parameter} IS NULL OR files.id = ?{file_parameter})
        ORDER BY thumbs.updated_at DESC, files.id ASC
        LIMIT ?{limit_parameter}
        "#,
        where_clause = parts.where_clause
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(parts.params), |row| {
        Ok(ThumbnailFailureRecord {
            file_id: row.get(0)?,
            folder_id: row.get(1)?,
            root_id: row.get(2)?,
            relative_path: row.get(3)?,
            failure_kind: row.get(4)?,
            error: row.get(5)?,
            updated_at: row.get::<_, i64>(6)?.saturating_mul(1_000),
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn thumbnail_failure_by_file_sql() -> String {
    let status_expression = thumbnail_cache_status_sql_expression();
    format!(
        r#"
        WITH RECURSIVE target_file(id, folder_id, root_id, name, failure_kind, error, updated_at) AS (
            SELECT files.id, files.folder_id, files.root_id, files.name,
                   COALESCE(thumbs.failure_kind, 'unknown'),
                   COALESCE(thumbs.error, 'thumbnail generation failed'),
                   thumbs.updated_at
            FROM files
            JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
            JOIN media ON media.file_id = files.id
            JOIN thumbs ON thumbs.file_id = files.id
            LEFT JOIN thumb_blobs
              ON thumb_blobs.file_id = files.id AND thumb_blobs.profile = thumbs.profile
            WHERE files.id = ?2
              AND thumbs.profile = ?1
              AND ({status_expression}) = 'failed'
        ),
        folder_ancestors(id, parent_id, root_id, relative_path, depth) AS (
            SELECT folders.id, folders.parent_id, folders.root_id,
                   CASE WHEN folders.parent_id IS NULL THEN '' ELSE folders.name END,
                   0
            FROM folders
            JOIN target_file ON target_file.folder_id = folders.id
            WHERE folders.status = 'active'
            UNION ALL
            SELECT parent.id, parent.parent_id, parent.root_id,
                   CASE
                     WHEN parent.parent_id IS NULL THEN folder_ancestors.relative_path
                     WHEN folder_ancestors.relative_path = '' THEN parent.name
                     ELSE parent.name || '\' || folder_ancestors.relative_path
                   END,
                   folder_ancestors.depth + 1
            FROM folder_ancestors
            JOIN folders parent
              ON parent.id = folder_ancestors.parent_id
             AND parent.root_id = folder_ancestors.root_id
             AND parent.status = 'active'
            WHERE folder_ancestors.depth + 1 < {FOLDER_ANCESTOR_DEPTH_CAP}
        )
        SELECT target_file.id, target_file.folder_id, target_file.root_id,
               roots.display_name ||
                 CASE WHEN folder_ancestors.relative_path = '' THEN ''
                      ELSE '\' || folder_ancestors.relative_path END ||
                 '\' || target_file.name,
               target_file.failure_kind,
               target_file.error,
               target_file.updated_at
        FROM target_file
        JOIN roots ON roots.id = target_file.root_id
        JOIN folder_ancestors
          ON folder_ancestors.root_id = target_file.root_id
         AND folder_ancestors.parent_id IS NULL
        "#
    )
}

fn thumbnail_failure_by_file_on_connection(
    connection: &Connection,
    profile: &str,
    file_id: i64,
) -> anyhow::Result<Option<ThumbnailFailureRecord>> {
    let sql = thumbnail_failure_by_file_sql();
    connection
        .query_row(&sql, (profile, file_id), |row| {
            Ok(ThumbnailFailureRecord {
                file_id: row.get(0)?,
                folder_id: row.get(1)?,
                root_id: row.get(2)?,
                relative_path: row.get(3)?,
                failure_kind: row.get(4)?,
                error: row.get(5)?,
                updated_at: row.get::<_, i64>(6)?.saturating_mul(1_000),
            })
        })
        .optional()
        .map_err(Into::into)
}

fn build_thumbnail_cache_stats_for_file_ids(
    database: &Database,
    file_ids: &[i64],
    entries: &[ThumbnailCacheEntry],
) -> ThumbnailCacheStats {
    let active_bulk_task_count = database
        .count_active_bulk_thumbnail_tasks_for_file_ids(file_ids)
        .unwrap_or(0);
    build_thumbnail_cache_stats_with_active_count(entries, active_bulk_task_count)
}

fn build_thumbnail_cache_stats_with_active_count(
    entries: &[ThumbnailCacheEntry],
    active_bulk_task_count: i64,
) -> ThumbnailCacheStats {
    let mut cached_count = 0;
    let mut missing_count = 0;
    let mut stale_count = 0;
    let mut failed_count = 0;
    let mut total_blob_bytes = 0;
    for entry in entries {
        total_blob_bytes += entry.blob_byte_size;
        match thumbnail_cache_status(entry) {
            ThumbnailCacheStatus::Cached => cached_count += 1,
            ThumbnailCacheStatus::Missing => missing_count += 1,
            ThumbnailCacheStatus::Stale => stale_count += 1,
            ThumbnailCacheStatus::Failed => failed_count += 1,
            ThumbnailCacheStatus::Skipped => {}
        }
    }
    ThumbnailCacheStats {
        cached_count,
        missing_count,
        stale_count,
        failed_count,
        pending_candidate_count: missing_count + stale_count,
        total_blob_bytes,
        active_bulk_task_count,
    }
}

#[allow(dead_code)]
fn cleanup_legacy_thumbnail_cache_tree(cache_root: &Path) {
    if let Err(error) = std::fs::remove_dir_all(cache_root) {
        tracing::debug!(
            path = %cache_root.display(),
            %error,
            "legacy thumbnail cache cleanup failed"
        );
    }
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecord> {
    Ok(TaskRecord {
        id: row.get(0)?,
        kind: row.get(1)?,
        priority: row.get(2)?,
        status: row.get(3)?,
        root_id: row.get(4)?,
        folder_id: row.get(5)?,
        file_id: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        items_seen: row.get(9)?,
        items_total: row.get(10)?,
        folders_seen: row.get(11)?,
        media_files_seen: row.get(12)?,
        skipped_files: row.get(13)?,
        thumbnail_source_fingerprint: row.get(14)?,
        attempt_generation: row.get(15)?,
        root_uuid: row.get(16)?,
        local_binding_uuid: row.get(17)?,
        binding_generation: row.get(18)?,
        error: row.get(19)?,
    })
}

fn task_record_in_transaction(
    transaction: &Transaction<'_>,
    task_id: i64,
) -> anyhow::Result<TaskRecord> {
    transaction
        .query_row(
            r#"
            SELECT id, kind, priority, status, root_id, folder_id, file_id, created_at, updated_at,
                   items_seen, items_total, folders_seen, media_files_seen, skipped_files,
                   thumbnail_source_fingerprint, attempt_generation,
                   root_uuid, local_binding_uuid, binding_generation, error
            FROM tasks
            WHERE id = ?1
            "#,
            [task_id],
            task_from_row,
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))
}

fn task_notification_summary_in_transaction(
    transaction: &Transaction<'_>,
    task: &TaskRecord,
) -> anyhow::Result<Option<String>> {
    let Some(root_id) = task.root_id else {
        return Ok(None);
    };
    let root_name = transaction
        .query_row(
            "SELECT display_name FROM roots WHERE id = ?1",
            [root_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(root_name) = root_name else {
        return Ok(None);
    };
    if task.kind != "interactive_folder_scan" {
        return Ok(Some(root_name));
    }
    let Some(mut folder_id) = task.folder_id else {
        return Ok(Some(root_name));
    };
    let mut names = Vec::new();
    for _ in 0..FOLDER_ANCESTOR_DEPTH_CAP {
        let folder = transaction
            .query_row(
                "SELECT parent_id, name FROM folders WHERE id = ?1 AND root_id = ?2",
                (folder_id, root_id),
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((parent_id, name)) = folder else {
            return Ok(Some(root_name));
        };
        let Some(parent_id) = parent_id else {
            names.reverse();
            return Ok(Some(
                std::iter::once(root_name)
                    .chain(names)
                    .collect::<Vec<_>>()
                    .join("\\"),
            ));
        };
        if !name.is_empty() {
            names.push(name);
        }
        folder_id = parent_id;
    }
    anyhow::bail!("folder notification path exceeded ancestor depth cap")
}

fn task_notification_origin_in_transaction(
    transaction: &Transaction<'_>,
    task_id: i64,
) -> Option<String> {
    let payload: Option<String> = transaction
        .query_row(
            "SELECT payload_json FROM notification_operation_heads WHERE operation_id = ?1",
            [format!("task:{task_id}")],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    payload
        .and_then(|payload| {
            serde_json::from_str::<notification_outbox::NotificationOperation>(&payload).ok()
        })
        .map(|operation| operation.origin)
}

fn task_notification_recovery_origin_override(
    transaction: &Transaction<'_>,
    task_id: i64,
) -> Option<NotificationOrigin> {
    task_notification_origin_in_transaction(transaction, task_id)
        .is_none()
        .then_some(NotificationOrigin::Recovery)
}

#[derive(Debug, Default, Clone, Copy)]
struct TaskRetryabilityResources {
    root_active: bool,
    folder_active: bool,
    root_pending_delete: bool,
}

fn task_retryable_for_state(
    task: &TaskRecord,
    resources: TaskRetryabilityResources,
) -> anyhow::Result<bool> {
    if !matches!(task.status.as_str(), "failed" | "cancelled") {
        return Ok(false);
    }
    match task.kind.as_str() {
        "root_scan" => Ok(resources.root_active),
        "interactive_folder_scan" => Ok(resources.folder_active),
        "root_purge" => Ok(resources.root_pending_delete),
        "thumbnail" => Ok(true),
        _ => Ok(task_notification_descriptor(&task.kind)?
            .is_some_and(|descriptor| descriptor.retryable)),
    }
}

fn task_retryable_in_transaction(
    transaction: &Transaction<'_>,
    task: &TaskRecord,
) -> anyhow::Result<bool> {
    let mut resources = TaskRetryabilityResources::default();
    match task.kind.as_str() {
        "root_scan" => {
            let Some(root_id) = task.root_id else {
                return Ok(false);
            };
            resources.root_active = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM roots WHERE id = ?1 AND enabled = 1 AND pending_delete_at IS NULL)",
                    [root_id],
                    |row| row.get::<_, i64>(0),
                )?
                != 0;
        }
        "interactive_folder_scan" => {
            let Some(folder_id) = task.folder_id else {
                return Ok(false);
            };
            resources.folder_active = transaction.query_row(
                "SELECT EXISTS(
                       SELECT 1 FROM folders
                       JOIN roots ON roots.id = folders.root_id
                       WHERE folders.id = ?1 AND folders.status = 'active'
                         AND roots.enabled = 1 AND roots.pending_delete_at IS NULL
                     )",
                [folder_id],
                |row| row.get::<_, i64>(0),
            )? != 0;
        }
        "root_purge" => {
            let Some(root_id) = task.root_id else {
                return Ok(false);
            };
            resources.root_pending_delete = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM roots WHERE id = ?1 AND pending_delete_at IS NOT NULL)",
                    [root_id],
                    |row| row.get::<_, i64>(0),
                )?
                != 0;
        }
        _ => {}
    };
    task_retryable_for_state(task, resources)
}

fn root_purge_work_total_in_transaction(
    transaction: &Transaction<'_>,
    root_id: i64,
) -> anyhow::Result<i64> {
    transaction
        .query_row(
            r#"
            SELECT
              (SELECT COUNT(*) FROM tasks WHERE root_id = ?1 AND kind <> 'thumbnail') +
              (SELECT COUNT(*) FROM tasks t JOIN files f ON f.id = t.file_id
                 WHERE t.kind = 'thumbnail' AND f.root_id = ?1) +
              (SELECT COUNT(*) FROM files WHERE root_id = ?1) +
              (SELECT COUNT(*) FROM folders WHERE root_id = ?1) +
              (SELECT COUNT(*) FROM roots WHERE id = ?1)
            "#,
            [root_id],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn task_ids_by_status_in_transaction(
    transaction: &Transaction<'_>,
    predicate: &str,
) -> anyhow::Result<Vec<i64>> {
    let mut statement = transaction.prepare(&format!("SELECT id FROM tasks WHERE {predicate}"))?;
    let ids = statement
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

fn root_purge_notification_head_in_transaction(
    transaction: &Transaction<'_>,
    root_id: i64,
) -> anyhow::Result<Option<notification_outbox::NotificationOperation>> {
    let mut statement = transaction.prepare(
        "SELECT payload_json FROM notification_operation_heads
         WHERE kind = 'root_purge' AND status IN ('pending', 'running')",
    )?;
    let payloads = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for payload in payloads {
        let operation: notification_outbox::NotificationOperation =
            serde_json::from_str(&payload).context("deserialize root purge notification head")?;
        if operation.root_id == Some(root_id) {
            return Ok(Some(operation));
        }
    }
    Ok(None)
}

#[derive(Debug)]
struct MediaCursor {
    key: MediaCursorKey,
    id: i64,
}

#[derive(Debug)]
enum MediaCursorKey {
    Name(String),
    Mtime(i64),
}

#[derive(Debug)]
struct FolderCursor {
    name: String,
    id: i64,
}

fn normalize_media_sort(sort: &str) -> &'static str {
    match sort {
        "mtime_asc" => "mtime_asc",
        "name_asc" => "name_asc",
        "name_desc" => "name_desc",
        _ => "mtime_desc",
    }
}

fn media_count_sql(where_clause: &str, include_media_join: bool) -> String {
    let media_join = if include_media_join {
        "LEFT JOIN media ON media.file_id = files.id"
    } else {
        ""
    };
    format!(
        r#"
        SELECT COUNT(*)
        FROM files
        {media_join}
        JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
        WHERE {where_clause}
        "#
    )
}

fn encode_media_cursor(sort: &str, item: &MediaRecord) -> String {
    match sort {
        "name_asc" | "name_desc" => {
            format!(
                "v1:media:{sort}:{}:{}",
                item.id,
                hex_encode(item.name.as_bytes())
            )
        }
        "mtime_asc" | "mtime_desc" => format!("v1:media:{sort}:{}:{}", item.id, item.mtime),
        _ => format!("v1:media:mtime_desc:{}:{}", item.id, item.mtime),
    }
}

fn decode_media_cursor(cursor: &str, expected_sort: &str) -> anyhow::Result<MediaCursor> {
    let mut parts = cursor.splitn(5, ':');
    let version = parts.next();
    let entity = parts.next();
    let sort = parts.next();
    let id = parts.next();
    let key = parts.next();
    if version != Some("v1") || entity != Some("media") || sort != Some(expected_sort) {
        return Err(anyhow::anyhow!("invalid media cursor"));
    }
    let id = id
        .ok_or_else(|| anyhow::anyhow!("invalid media cursor"))?
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("invalid media cursor"))?;
    let key = key.ok_or_else(|| anyhow::anyhow!("invalid media cursor"))?;
    let key = match expected_sort {
        "name_asc" | "name_desc" => MediaCursorKey::Name(
            hex_decode_to_string(key).map_err(|_| anyhow::anyhow!("invalid media cursor"))?,
        ),
        "mtime_asc" | "mtime_desc" => MediaCursorKey::Mtime(
            key.parse::<i64>()
                .map_err(|_| anyhow::anyhow!("invalid media cursor"))?,
        ),
        _ => return Err(anyhow::anyhow!("invalid media cursor")),
    };
    Ok(MediaCursor { key, id })
}

fn encode_folder_cursor(folder: &FolderRecord) -> String {
    format!(
        "v1:folder:{}:{}",
        folder.id,
        hex_encode(folder.name.as_bytes())
    )
}

fn decode_folder_cursor(cursor: &str) -> anyhow::Result<FolderCursor> {
    let mut parts = cursor.splitn(4, ':');
    if parts.next() != Some("v1") || parts.next() != Some("folder") {
        return Err(anyhow::anyhow!("invalid folder cursor"));
    }
    let id = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid folder cursor"))?
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("invalid folder cursor"))?;
    let name = hex_decode_to_string(
        parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("invalid folder cursor"))?,
    )
    .map_err(|_| anyhow::anyhow!("invalid folder cursor"))?;
    Ok(FolderCursor { name, id })
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn hex_decode_to_string(value: &str) -> anyhow::Result<String> {
    if value.len() % 2 != 0 {
        return Err(anyhow::anyhow!("invalid cursor hex"));
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    let chars = value.as_bytes();
    for index in (0..chars.len()).step_by(2) {
        let high = hex_value(chars[index])?;
        let low = hex_value(chars[index + 1])?;
        bytes.push((high << 4) | low);
    }
    Ok(String::from_utf8(bytes)?)
}

fn hex_value(value: u8) -> anyhow::Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(anyhow::anyhow!("invalid cursor hex")),
    }
}

fn configure_file_database_wal(connection: &Connection) -> anyhow::Result<()> {
    connection.pragma_update(None, "journal_mode", WAL_MODE)?;
    // Must follow journal_mode=WAL: NORMAL is only safe under WAL.
    connection.pragma_update(None, "synchronous", SQLITE_SYNCHRONOUS_NORMAL)?;
    // Disable inline auto-checkpoint: SQLite's auto-checkpoint runs on the
    // committing writer's own thread, so a checkpoint of a large WAL freezes
    // whichever interactive scroll write or blob commit happened to trip the
    // page threshold (measured ~1.8s stalls on a 650k-file library). A
    // dedicated background thread (see `run_periodic_wal_checkpoint` +
    // `start_wal_checkpoint_thread`) checkpoints off the hot path instead.
    connection.pragma_update(None, "wal_autocheckpoint", WAL_AUTOCHECKPOINT_PAGES)?;
    connection.pragma_update(None, "journal_size_limit", WAL_JOURNAL_SIZE_LIMIT_BYTES)?;
    connection.pragma_update(None, "cache_size", SQLITE_CACHE_SIZE_KIB)?;
    connection.pragma_update(None, "mmap_size", SQLITE_MMAP_SIZE_BYTES)?;
    connection.pragma_update(None, "temp_store", SQLITE_TEMP_STORE_MEMORY)?;
    Ok(())
}

pub(crate) fn unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn unix_timestamp_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn tag_entity_uuid(
    transaction: &rusqlite::Transaction<'_>,
    tag_id: i64,
) -> anyhow::Result<crate::sync::types::EntityUuid> {
    let bytes: Vec<u8> =
        transaction.query_row("SELECT tag_uuid FROM tags WHERE id=?1", [tag_id], |row| {
            row.get(0)
        })?;
    crate::sync::capture::entity_uuid(&bytes)
}

fn is_valid_hex_color(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' {
        return false;
    }
    bytes[1..].iter().all(|byte| byte.is_ascii_hexdigit())
}

/// Public re-export so other modules in the crate (e.g. `fsops`) can keep
/// FTS in sync inside their own transactions without owning the SQL.
pub(crate) fn sync_media_fts_for_file_in_transaction_pub(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
) -> anyhow::Result<()> {
    sync_media_fts_for_file_in_transaction(transaction, file_id)
}

fn sync_media_fts_for_file_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    file_id: i64,
) -> anyhow::Result<()> {
    let row = transaction
        .query_row(
            r#"
            SELECT files.name,
                   COALESCE(user_metadata.note, ''),
                   COALESCE((
                     SELECT GROUP_CONCAT(tags.name, ' ')
                     FROM file_tags
                     JOIN tags ON tags.id = file_tags.tag_id
                     WHERE file_tags.file_id = files.id
                   ), '')
            FROM files
            LEFT JOIN user_metadata ON user_metadata.file_id = files.id
            WHERE files.id = ?1
            "#,
            [file_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    transaction.execute("DELETE FROM media_fts WHERE rowid = ?1", [file_id])?;
    if let Some((name, note, tags)) = row {
        transaction.execute(
            "INSERT INTO media_fts(rowid, name, note, tags) VALUES (?1, ?2, ?3, ?4)",
            (file_id, name, note, tags),
        )?;
    }
    Ok(())
}

fn build_fts_match_query(value: &str) -> String {
    // Build a tolerant FTS5 match: split on whitespace, escape internal
    // double quotes, wrap each non-empty token in double quotes, and append
    // a `*` so partial matches succeed. We do not propagate operators;
    // callers can author advanced queries elsewhere if needed.
    let mut tokens = Vec::new();
    for raw in value.split_whitespace() {
        let escaped = raw.replace('"', "\"\"");
        if !escaped.is_empty() {
            tokens.push(format!("\"{}\"*", escaped));
        }
    }
    if tokens.is_empty() {
        // Fallback: match nothing-ish so no rows come back.
        return "\"\"".to_string();
    }
    tokens.join(" ")
}

#[derive(Debug)]
struct SearchCursor {
    key: SearchCursorKey,
    id: i64,
}

#[derive(Debug)]
enum SearchCursorKey {
    Mtime(i64),
    Name(String),
    Rating(Option<i64>),
}

fn normalize_search_sort(sort: &str) -> &'static str {
    match sort {
        "mtime_asc" => "mtime_asc",
        "name_asc" => "name_asc",
        "name_desc" => "name_desc",
        "rating_desc" => "rating_desc",
        "rating_asc" => "rating_asc",
        _ => "mtime_desc",
    }
}

fn encode_search_cursor(sort: &str, item: &MediaRecord) -> String {
    match sort {
        "name_asc" | "name_desc" => format!(
            "v1:search:{sort}:{}:{}",
            item.id,
            hex_encode(item.name.as_bytes())
        ),
        "mtime_asc" | "mtime_desc" => {
            format!("v1:search:{sort}:{}:{}", item.id, item.mtime)
        }
        "rating_desc" | "rating_asc" => {
            let rating = item
                .rating
                .map(|value| value.to_string())
                .unwrap_or_else(|| "null".to_string());
            format!("v1:search:{sort}:{}:{}", item.id, rating)
        }
        _ => format!("v1:search:mtime_desc:{}:{}", item.id, item.mtime),
    }
}

fn decode_search_cursor(cursor: &str, expected_sort: &str) -> anyhow::Result<SearchCursor> {
    let mut parts = cursor.splitn(5, ':');
    let version = parts.next();
    let entity = parts.next();
    let sort = parts.next();
    let id = parts.next();
    let key = parts.next();
    if version != Some("v1") || entity != Some("search") || sort != Some(expected_sort) {
        return Err(anyhow::anyhow!("invalid search cursor"));
    }
    let id = id
        .ok_or_else(|| anyhow::anyhow!("invalid search cursor"))?
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("invalid search cursor"))?;
    let key = key.ok_or_else(|| anyhow::anyhow!("invalid search cursor"))?;
    let key = match expected_sort {
        "name_asc" | "name_desc" => SearchCursorKey::Name(
            hex_decode_to_string(key).map_err(|_| anyhow::anyhow!("invalid search cursor"))?,
        ),
        "mtime_asc" | "mtime_desc" => SearchCursorKey::Mtime(
            key.parse::<i64>()
                .map_err(|_| anyhow::anyhow!("invalid search cursor"))?,
        ),
        "rating_desc" | "rating_asc" => {
            if key == "null" {
                SearchCursorKey::Rating(None)
            } else {
                SearchCursorKey::Rating(Some(
                    key.parse::<i64>()
                        .map_err(|_| anyhow::anyhow!("invalid search cursor"))?,
                ))
            }
        }
        _ => return Err(anyhow::anyhow!("invalid search cursor")),
    };
    Ok(SearchCursor { key, id })
}

/// Build the WHERE-clause keyset predicate for rating-sorted searches.
/// Returns the predicate text plus the parameters to bind, in order.
fn build_rating_keyset_predicate(
    cursor_rating: Option<i64>,
    cursor_id: i64,
    ascending: bool,
) -> (String, Vec<Value>) {
    // ORDER BY: `rating IS NULL ASC, rating <DIR>, id <DIR>`. Within the
    // null group (rating IS NULL = 1), all rows are equivalent except by id.
    // Outside the null group, compare by rating, breaking ties by id.
    let id_op = if ascending { ">" } else { "<" };
    let rating_op = if ascending { ">" } else { "<" };
    match cursor_rating {
        Some(rating) => {
            // Cursor is in the non-null group. Continue with non-null rows
            // strictly less/greater than the cursor rating, or the same
            // rating with greater/less id, or fall through to the null
            // group.
            let clause = format!(
                "((user_metadata.rating IS NOT NULL AND (user_metadata.rating {rating_op} ? OR (user_metadata.rating = ? AND files.id {id_op} ?))) OR user_metadata.rating IS NULL)"
            );
            (
                clause,
                vec![
                    Value::Integer(rating),
                    Value::Integer(rating),
                    Value::Integer(cursor_id),
                ],
            )
        }
        None => {
            // Cursor is already inside the null group. Continue with null
            // rows whose id is past the cursor in the requested direction.
            let clause = format!("(user_metadata.rating IS NULL AND files.id {id_op} ?)");
            (clause, vec![Value::Integer(cursor_id)])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated_database() -> Database {
        let database = Database::open_in_memory().expect("open in-memory database");
        database.apply_migrations().expect("apply migrations");
        database
    }

    #[test]
    fn external_scan_captures_initial_and_same_metadata_content_changes() {
        let mut database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        database
            .initialize_sync_identity(
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
                uuid::Uuid::now_v7(),
            )
            .expect("initialize sync identity");
        let root_folder_id = database
            .commit_scan_batch(ScanWriteBatch {
                folders: vec![FolderUpsert {
                    root_id,
                    parent_id: None,
                    name: String::new(),
                    path_hash: "root-hash".to_string(),
                    mtime: Some(10),
                }],
                files: Vec::new(),
                scan_generation: None,
            })
            .expect("commit root folder")
            .folder_ids[0];
        let scan = |fingerprint: &[u8]| ScanWriteBatch {
            folders: Vec::new(),
            files: vec![ScanFileUpsert {
                file: FileUpsert {
                    root_id,
                    folder_id: root_folder_id,
                    name: "same-size.jpg".to_string(),
                    ext: ".jpg".to_string(),
                    size: 4,
                    mtime: 10,
                    ctime: None,
                    file_key: Some("stable-file-id".to_string()),
                },
                media_kind: "image".to_string(),
                external_identity: Some(ExternalFileIdentity {
                    volume_identity: b"stable-volume".to_vec(),
                    file_identity: b"stable-file-id".to_vec(),
                    content_fingerprint: Some(fingerprint.to_vec()),
                    path: PathBuf::from("D:/Pictures/same-size.jpg"),
                }),
            }],
            scan_generation: None,
        };

        database
            .commit_scan_batch(scan(&[0xa1; 32]))
            .expect("first scan");
        let first_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM sync_publication_intents", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(first_count, 1, "initial scan must publish existing media");
        let binding: Option<Vec<u8>> = database
            .connection
            .query_row(
                "SELECT binding_epoch FROM sync_publication_intents",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(binding.is_none(), "local-only capture remains unbound");

        database
            .commit_scan_batch(scan(&[0xa1; 32]))
            .expect("unchanged rescan");
        let unchanged_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM sync_publication_intents", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            unchanged_count, 1,
            "unchanged rescan must not duplicate content"
        );

        database
            .commit_scan_batch(ScanWriteBatch {
                folders: Vec::new(),
                files: vec![ScanFileUpsert {
                    file: FileUpsert {
                        root_id,
                        folder_id: root_folder_id,
                        name: "same-size.jpg".to_string(),
                        ext: ".jpg".to_string(),
                        size: 4,
                        mtime: 10,
                        ctime: None,
                        file_key: Some("relocated-file-id".to_string()),
                    },
                    media_kind: "image".to_string(),
                    external_identity: Some(ExternalFileIdentity {
                        volume_identity: b"relocated-volume".to_vec(),
                        file_identity: b"relocated-file-id".to_vec(),
                        content_fingerprint: Some(vec![0xa1; 32]),
                        path: PathBuf::from("E:/Relocated/same-size.jpg"),
                    }),
                }],
                scan_generation: None,
            })
            .expect("same-content relocation scan");
        let relocated_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM sync_publication_intents", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            relocated_count, 1,
            "a local file-identity change with the same content fingerprint must not publish portable content"
        );

        let first_intent: Vec<u8> = database
            .connection
            .query_row(
                "SELECT intent_uuid FROM sync_publication_intents",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let binding = uuid::Uuid::now_v7();
        database
            .begin_sync_capture(binding, uuid::Uuid::now_v7())
            .unwrap();
        database
            .bind_fake_sync_manifest(uuid::Uuid::from_slice(&first_intent).unwrap(), [7; 32])
            .unwrap();

        database
            .commit_scan_batch(scan(&[0xb2; 32]))
            .expect("same-metadata overwrite scan");
        let changed_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM sync_publication_intents", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(changed_count, 2, "content fingerprint change must publish");
        let head_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM asset_content_heads", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            head_count, 1,
            "sequential local edits keep one content head"
        );
        let supersedes_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM asset_content_supersedes", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            supersedes_count, 1,
            "the new local revision must explicitly supersede the prior head"
        );
        database
            .commit_scan_batch(scan(&[0xc3; 32]))
            .expect("newer overwrite supersedes pending attempt");
        let states = {
            let mut statement = database
                .connection
                .prepare(
                    "SELECT state FROM sync_publication_intents ORDER BY local_mutation_generation",
                )
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            states,
            vec!["canonicalized", "cancelled", "pending-hash"],
            "a newer local edit must cancel the unpublished revision it replaces"
        );
        let intents = {
            let mut statement = database
                .connection
                .prepare(
                    "SELECT intent_uuid,content_revision_uuid FROM sync_publication_intents ORDER BY local_mutation_generation",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        database
            .bind_fake_sync_manifest(uuid::Uuid::from_slice(&intents[2].0).unwrap(), [8; 32])
            .unwrap();
        let payload: Vec<u8> = database
            .connection
            .query_row(
                "SELECT payload FROM sync_events ORDER BY actor_seq DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let event = crate::sync::codec::decode_event(&payload).unwrap();
        let crate::sync::types::Operation::SetContent { supersedes, .. } = event.operation else {
            panic!("latest publication must be a content event");
        };
        assert_eq!(
            supersedes,
            [crate::sync::types::RevisionUuid(
                uuid::Uuid::from_slice(&intents[0].1).unwrap()
            )]
            .into_iter()
            .collect(),
            "the canonical content event must carry the local causal supersession"
        );
    }

    #[test]
    fn file_database_configures_bounded_wal_settings() {
        let db_dir = unique_db_temp_dir("bounded-wal-settings");
        std::fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");

        let journal_mode: String = database
            .connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("query journal mode");
        let wal_autocheckpoint_pages: i64 = database
            .connection
            .query_row("PRAGMA wal_autocheckpoint", [], |row| row.get(0))
            .expect("query wal autocheckpoint");
        let journal_size_limit_bytes: i64 = database
            .connection
            .query_row("PRAGMA journal_size_limit", [], |row| row.get(0))
            .expect("query journal size limit");

        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        assert_eq!(wal_autocheckpoint_pages, WAL_AUTOCHECKPOINT_PAGES);
        assert_eq!(journal_size_limit_bytes, WAL_JOURNAL_SIZE_LIMIT_BYTES);

        let _ = std::fs::remove_dir_all(db_dir);
    }

    #[test]
    fn brand_new_empty_file_migrates_and_lists_no_roots() {
        let db_dir = unique_db_temp_dir("fresh-empty-migrate");
        std::fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("fresh.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database
            .apply_migrations()
            .expect("apply migrations on a fresh empty file");
        let roots = database.list_roots().expect("list roots on fresh db");
        assert!(roots.is_empty(), "a brand-new library must have no roots");
    }

    #[test]
    fn wal_size_bytes_reports_zero_for_in_memory_and_reads_file_wal() {
        // In-memory DB has no WAL sidecar.
        let mem = Database::open_in_memory().expect("open in-memory");
        assert_eq!(mem.wal_size_bytes(), 0);

        // File-backed DB in WAL mode: after a write (no checkpoint), the -wal
        // sidecar exists and is non-empty.
        let dir = unique_db_temp_dir("wal-size");
        std::fs::create_dir_all(&dir).expect("create db dir");
        let db_path = dir.join("wal.sqlite");
        let db = Database::open(&db_path).expect("open file db");
        db.apply_migrations().expect("apply migrations");
        db.create_tag("t", None).expect("write");
        assert!(
            db.wal_size_bytes() > 0,
            "expected a non-empty -wal after a write in WAL mode"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_database_can_truncate_oversized_wal_on_startup_maintenance() {
        let db_dir = unique_db_temp_dir("truncate-oversized-wal");
        std::fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        database
            .connection
            .pragma_update(None, "wal_autocheckpoint", 0)
            .expect("disable autocheckpoint for pressure setup");
        database
            .connection
            .pragma_update(None, "journal_size_limit", -1)
            .expect("disable journal size limit for pressure setup");

        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "wal-pressure.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 1024,
                mtime: 20,
                ctime: None,
                file_key: None,
            })
            .expect("insert file");
        database
            .upsert_thumb_blob(ThumbBlobRecord {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                data: vec![7; 1024 * 1024],
                width: 320,
                height: 180,
                byte_size: 1024 * 1024,
                output_format: crate::thumbnails::GENERATED_FORMAT.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .expect("insert large thumb blob");

        let wal_path = Database::wal_path_for_db_path(&db_path);
        let before_bytes = std::fs::metadata(&wal_path)
            .expect("wal file exists before checkpoint")
            .len();
        assert!(before_bytes > 1024 * 1024);

        let checkpointed = database
            .checkpoint_wal_if_larger_than(1)
            .expect("checkpoint oversized wal");
        let after_bytes = std::fs::metadata(&wal_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);

        assert!(checkpointed);
        assert!(
            after_bytes < before_bytes,
            "checkpoint should shrink WAL: before={before_bytes}, after={after_bytes}"
        );

        let _ = std::fs::remove_dir_all(db_dir);
    }

    #[test]
    fn preview_pipeline_migration_adds_placeholder_and_thumb_blobs() {
        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");

        assert!(database
            .table_has_column("media", "preview_placeholder")
            .unwrap());
        assert!(database
            .table_has_column("media", "preview_placeholder_format")
            .unwrap());
        assert!(database.table_exists("thumb_blobs").unwrap());

        let index_exists: i64 = database
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'index' AND name = 'idx_thumb_blobs_profile_updated_at')",
                [],
                |row| row.get(0),
            )
            .expect("query thumb_blobs index");
        assert_eq!(index_exists, 1);

        database
            .connection
            .execute(
                "INSERT INTO roots(path, display_name, enabled, created_at) VALUES ('D:/Pictures', 'Pictures', 1, 1)",
                [],
            )
            .expect("insert root");
        database
            .connection
            .execute(
                "INSERT INTO folders(root_id, parent_id, name, path_hash, mtime) VALUES (1, NULL, '', 'root-hash', 1)",
                [],
            )
            .expect("insert folder");
        database
            .connection
            .execute(
                "INSERT INTO files(root_id, folder_id, name, ext, size, mtime) VALUES (1, 1, 'image.jpg', '.jpg', 1000, 10)",
                [],
            )
            .expect("insert file");
        database
            .connection
            .execute(
                "INSERT INTO files(root_id, folder_id, name, ext, size, mtime) VALUES (1, 1, 'second.jpg', '.jpg', 1000, 11)",
                [],
            )
            .expect("insert second file");
        database
            .connection
            .execute(
                r#"
                INSERT INTO thumb_blobs(file_id, profile, data, width, height, byte_size, output_format, created_at, updated_at)
                VALUES (1, 'grid_320', X'524946460000000057454250', 40, 20, 12, 'image/webp', 1, 1)
                "#,
                [],
            )
            .expect("insert valid thumb blob");
        let invalid_profile = database
            .connection
            .execute(
                r#"
                INSERT OR IGNORE INTO thumb_blobs(file_id, profile, data, width, height, byte_size, output_format, created_at, updated_at)
                VALUES (1, 'preview', X'00', 1, 1, 1, 'image/webp', 1, 1)
                "#,
                [],
            )
            .expect("invalid profile should be ignored");
        assert_eq!(invalid_profile, 0);
        let invalid_format = database
            .connection
            .execute(
                r#"
                INSERT OR IGNORE INTO thumb_blobs(file_id, profile, data, width, height, byte_size, output_format, created_at, updated_at)
                VALUES (2, 'grid_320', X'00', 1, 1, 1, 'image/jpeg', 1, 2)
                "#,
                [],
            )
            .expect("invalid format should be ignored");
        assert_eq!(invalid_format, 0);
        let second_file_blob_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM thumb_blobs WHERE file_id = 2",
                [],
                |row| row.get(0),
            )
            .expect("count second file thumb blobs");
        assert_eq!(second_file_blob_count, 0);
    }

    #[test]
    fn preview_pipeline_migration_recovers_after_partial_apply() {
        let database = Database::open_in_memory().expect("open database");
        database
            .connection
            .execute_batch(migrations::INITIAL_MIGRATION)
            .expect("apply initial migration");
        database
            .connection
            .execute("ALTER TABLE media ADD COLUMN preview_placeholder BLOB", [])
            .expect("simulate partially added placeholder column");

        database
            .apply_migrations()
            .expect("apply migrations after partial preview pipeline migration");

        assert!(database
            .table_has_column("media", "preview_placeholder")
            .unwrap());
        assert!(database
            .table_has_column("media", "preview_placeholder_format")
            .unwrap());
        assert!(database.table_exists("thumb_blobs").unwrap());
        let version_12_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 12",
                [],
                |row| row.get(0),
            )
            .expect("query version 12");
        assert_eq!(version_12_count, 1);
    }

    #[test]
    fn legacy_thumbnail_cache_import_persists_blob_and_removes_file() {
        let db_dir = unique_db_temp_dir("legacy-cache-import");
        std::fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "legacy.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: None,
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");
        database
            .update_media_dimensions(file_id, 640, 320)
            .expect("set dimensions");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        let cache_key = "aa/bb/legacy.webp";
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "ready".to_string(),
                cache_key: Some(cache_key.to_string()),
                width: Some(640),
                height: Some(320),
                byte_size: Some(12),
                error: None,
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("seed ready thumbnail");
        let cache_path = database.default_thumbnail_cache_dir().join(cache_key);
        std::fs::create_dir_all(cache_path.parent().expect("cache parent"))
            .expect("create cache dirs");
        let valid_webp = test_webp_bytes();
        std::fs::write(&cache_path, &valid_webp).expect("write legacy cache");
        let orphan_path = database
            .default_thumbnail_cache_dir()
            .join("orphan")
            .join("unreferenced.webp");
        std::fs::create_dir_all(orphan_path.parent().expect("orphan parent"))
            .expect("create orphan dir");
        std::fs::write(&orphan_path, b"RIFF\x04\0\0\0WEBP").expect("write orphan cache");
        let corrupt_path = database
            .default_thumbnail_cache_dir()
            .join("corrupt")
            .join("bad.webp");
        std::fs::create_dir_all(corrupt_path.parent().expect("corrupt parent"))
            .expect("create corrupt dir");
        std::fs::write(&corrupt_path, b"not webp").expect("write corrupt cache");

        let imported = database
            .import_legacy_thumbnail_cache()
            .expect("import legacy cache");

        assert_eq!(imported, 1);
        let blob = database
            .get_thumb_blob(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("read thumb blob")
            .expect("thumb blob exists");
        assert_eq!(blob.data, valid_webp);
        assert!(!cache_path.exists());
        assert!(!orphan_path.exists());
        assert!(!corrupt_path.exists());
        assert!(!database.default_thumbnail_cache_dir().exists());

        let _ = std::fs::remove_dir_all(db_dir);
    }

    #[test]
    fn legacy_thumbnail_cache_import_invalidates_ready_row_when_referenced_webp_is_undecodable() {
        let db_dir = unique_db_temp_dir("legacy-cache-corrupt");
        std::fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "corrupt.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: None,
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");
        database
            .update_media_dimensions(file_id, 640, 320)
            .expect("set dimensions");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        let cache_key = "aa/bb/corrupt.webp";
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "ready".to_string(),
                cache_key: Some(cache_key.to_string()),
                width: Some(640),
                height: Some(320),
                byte_size: Some(12),
                error: None,
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("seed ready thumbnail");
        let cache_path = database.default_thumbnail_cache_dir().join(cache_key);
        std::fs::create_dir_all(cache_path.parent().expect("cache parent"))
            .expect("create cache dirs");
        std::fs::write(&cache_path, b"RIFF\x04\0\0\0WEBP").expect("write undecodable legacy cache");

        let imported = database
            .import_legacy_thumbnail_cache()
            .expect("import legacy cache");

        assert_eq!(imported, 0);
        assert!(database
            .get_thumb_blob(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("read thumb blob")
            .is_none());
        assert!(!database.default_thumbnail_cache_dir().exists());
        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_ne!(thumbnail.state, "ready");

        let _ = std::fs::remove_dir_all(db_dir);
    }

    fn seed_media_page_fixture(database: &Database) -> (i64, i64) {
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root-hash".to_string(),
                mtime: Some(10),
            })
            .expect("insert root folder");

        for (index, name) in ["alpha.jpg", "bravo.jpg", "charlie.jpg", "delta.jpg"]
            .iter()
            .enumerate()
        {
            let file_id = database
                .upsert_file(FileUpsert {
                    root_id,
                    folder_id,
                    name: (*name).to_string(),
                    ext: ".jpg".to_string(),
                    size: 100 + index as i64,
                    mtime: 100 + index as i64,
                    ctime: None,
                    file_key: None,
                })
                .expect("insert file");
            database
                .upsert_media_kind(file_id, "image")
                .expect("insert media kind");
        }

        (root_id, folder_id)
    }

    fn unique_db_temp_dir(label: &str) -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "megle-db-test-{label}-{}-{suffix}",
            std::process::id()
        ))
    }

    fn test_webp_bytes() -> Vec<u8> {
        let rgba = image::RgbaImage::from_pixel(2, 1, image::Rgba([20, 40, 60, 255]));
        let encoded =
            webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height()).encode(75.0);
        let bytes: &[u8] = &encoded;
        bytes.to_vec()
    }

    fn seed_thumbnail_cache_scope_fixture(database: &Database, label: &str) -> (i64, i64) {
        let root_id = database
            .add_root(NewRoot {
                path: format!("D:/Pictures/{label}"),
                display_name: format!("Pictures {label}"),
            })
            .expect("add cache scope root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: format!("{label}-root-hash"),
                mtime: Some(10),
            })
            .expect("insert cache scope folder");
        (root_id, folder_id)
    }

    fn seed_thumbnail_cache_candidate(
        database: &Database,
        root_id: i64,
        folder_id: i64,
        name: &str,
        file_key: Option<&str>,
        thumbnail_state: &str,
        with_blob: bool,
        blob_seed: Option<i64>,
        thumbnail_fingerprint_seed: Option<i64>,
    ) -> i64 {
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: name.to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: file_key.map(str::to_string),
            })
            .expect("insert candidate file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert candidate media");
        let current_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get candidate source")
            .expect("candidate source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        let source_fingerprint = thumbnail_fingerprint_seed
            .map(|seed| format!("stale-{seed}"))
            .unwrap_or_else(|| current_fingerprint.clone());
        if thumbnail_state != "absent" {
            database
                .upsert_thumbnail_state(ThumbnailStateUpsert {
                    file_id,
                    profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                    state: thumbnail_state.to_string(),
                    cache_key: None,
                    width: if thumbnail_state == "ready" {
                        Some(320)
                    } else {
                        None
                    },
                    height: if thumbnail_state == "ready" {
                        Some(160)
                    } else {
                        None
                    },
                    byte_size: if thumbnail_state == "ready" {
                        Some(42)
                    } else {
                        None
                    },
                    error: if thumbnail_state == "failed" {
                        Some("decode failed".to_string())
                    } else {
                        None
                    },
                    source_fingerprint: if thumbnail_state == "pending" {
                        None
                    } else {
                        Some(source_fingerprint)
                    },
                })
                .expect("seed thumbnail state");
        }
        if with_blob {
            database
                .upsert_thumb_blob(ThumbBlobRecord {
                    file_id,
                    profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                    data: test_webp_bytes(),
                    width: 320,
                    height: 160,
                    byte_size: blob_seed.unwrap_or(42),
                    output_format: crate::thumbnails::GENERATED_FORMAT.to_string(),
                    created_at: 1,
                    updated_at: 1,
                })
                .expect("seed candidate blob");
        }
        file_id
    }

    /// Seeds `count` real `files` rows (plus the root/folder they hang off) and
    /// returns their ids. Preview-tier tests only need valid `files.id` values,
    /// so this reuses the existing `seed_thumbnail_cache_scope_fixture` for the
    /// root/folder pair and the same `upsert_file` call shape as
    /// `seed_media_page_fixture` rather than introducing a new fixture path.
    fn seed_media_files(database: &Database, count: usize) -> Vec<i64> {
        let (root_id, folder_id) = seed_thumbnail_cache_scope_fixture(database, "preview-blobs");
        (0..count)
            .map(|index| {
                database
                    .upsert_file(FileUpsert {
                        root_id,
                        folder_id,
                        name: format!("photo-{index}.jpg"),
                        ext: ".jpg".to_string(),
                        size: 100 + index as i64,
                        mtime: 100 + index as i64,
                        ctime: None,
                        file_key: None,
                    })
                    .expect("insert preview fixture file")
            })
            .collect()
    }

    fn seed_single_media_file(database: &Database) -> i64 {
        seed_media_files(database, 1)[0]
    }

    #[test]
    fn inactive_files_do_not_strand_preview_tiers_in_the_cache_budget() {
        // Recycling a file or disabling a root flips `files.status` WITHOUT
        // deleting the row, so the FK cascade never fires. Grid thumbs were
        // already reclaimed here; preview tiers were not, so 1-3 MB blobs for
        // unreachable files sat in the 2 GiB budget and evicted tiers for
        // images the user is actually looking at.
        let database = Database::open_in_memory().expect("open");
        database.apply_migrations().expect("migrate");
        let file_ids = seed_media_files(&database, 2);
        let (kept, recycled) = (file_ids[0], file_ids[1]);

        for file_id in [kept, recycled] {
            database
                .upsert_preview_blob(PreviewBlobRecord {
                    file_id,
                    profile: PREVIEW_2048_PROFILE_FOR_TEST.to_string(),
                    data: vec![1, 2, 3, 4],
                    width: 2048,
                    height: 1152,
                    byte_size: 4,
                    output_format: "image/webp".to_string(),
                    source_size: 1,
                    source_mtime: 1,
                    created_at: 10,
                    updated_at: 10,
                })
                .expect("seed tier");
        }

        database
            .connection
            .execute(
                "UPDATE files SET status = 'deleted' WHERE id = ?1",
                [recycled],
            )
            .expect("mark inactive");

        let transaction = database
            .connection
            .unchecked_transaction()
            .expect("transaction");
        cleanup_inactive_thumbnail_artifacts_in_transaction(&transaction).expect("cleanup");
        transaction.commit().expect("commit");

        assert!(
            database
                .get_preview_blob(recycled, PREVIEW_2048_PROFILE_FOR_TEST)
                .expect("query")
                .is_none(),
            "an inactive file's preview tier must be reclaimed"
        );
        assert!(
            database
                .get_preview_blob(kept, PREVIEW_2048_PROFILE_FOR_TEST)
                .expect("query")
                .is_some(),
            "an active file's preview tier must survive"
        );
    }

    const PREVIEW_2048_PROFILE_FOR_TEST: &str = "preview_2048";

    fn media_names(items: &[MediaRecord]) -> Vec<&str> {
        items.iter().map(|item| item.name.as_str()).collect()
    }

    fn folder_names(items: &[FolderRecord]) -> Vec<&str> {
        items.iter().map(|item| item.name.as_str()).collect()
    }

    fn folder_named<'a>(items: &'a [FolderRecord], name: &str) -> &'a FolderRecord {
        items
            .iter()
            .find(|item| item.name == name)
            .unwrap_or_else(|| panic!("missing folder {name}"))
    }

    const OLD_INITIAL_MIGRATION_WITHOUT_TASK_PROGRESS: &str = r#"
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS schema_migrations (
          version INTEGER PRIMARY KEY,
          name TEXT NOT NULL,
          applied_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS roots (
          id INTEGER PRIMARY KEY,
          path TEXT NOT NULL UNIQUE,
          display_name TEXT NOT NULL,
          enabled INTEGER NOT NULL DEFAULT 1,
          created_at INTEGER NOT NULL,
          last_scan_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS folders (
          id INTEGER PRIMARY KEY,
          root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
          parent_id INTEGER REFERENCES folders(id) ON DELETE CASCADE,
          name TEXT NOT NULL,
          path_hash TEXT NOT NULL,
          mtime INTEGER,
          status TEXT NOT NULL DEFAULT 'active',
          UNIQUE(root_id, parent_id, name)
        );

        CREATE TABLE IF NOT EXISTS files (
          id INTEGER PRIMARY KEY,
          root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
          folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
          name TEXT NOT NULL,
          ext TEXT NOT NULL,
          size INTEGER NOT NULL,
          mtime INTEGER NOT NULL,
          ctime INTEGER,
          file_key TEXT,
          status TEXT NOT NULL DEFAULT 'active',
          UNIQUE(folder_id, name)
        );

        CREATE TABLE IF NOT EXISTS tasks (
          id INTEGER PRIMARY KEY,
          kind TEXT NOT NULL,
          priority INTEGER NOT NULL,
          status TEXT NOT NULL,
          root_id INTEGER REFERENCES roots(id) ON DELETE SET NULL,
          file_id INTEGER REFERENCES files(id) ON DELETE SET NULL,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          error TEXT
        );

        INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
        VALUES (1, 'initial', unixepoch());
    "#;

    const OLD_SCHEMA_WITH_LEGACY_THUMBS: &str = r#"
        PRAGMA foreign_keys = ON;

        CREATE TABLE schema_migrations (
          version INTEGER PRIMARY KEY,
          name TEXT NOT NULL,
          applied_at INTEGER NOT NULL
        );

        CREATE TABLE roots (
          id INTEGER PRIMARY KEY,
          path TEXT NOT NULL UNIQUE,
          display_name TEXT NOT NULL,
          enabled INTEGER NOT NULL DEFAULT 1,
          created_at INTEGER NOT NULL,
          last_scan_at INTEGER
        );

        CREATE TABLE folders (
          id INTEGER PRIMARY KEY,
          root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
          parent_id INTEGER REFERENCES folders(id) ON DELETE CASCADE,
          name TEXT NOT NULL,
          path_hash TEXT NOT NULL,
          mtime INTEGER,
          status TEXT NOT NULL DEFAULT 'active',
          UNIQUE(root_id, parent_id, name)
        );

        CREATE TABLE files (
          id INTEGER PRIMARY KEY,
          root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
          folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
          name TEXT NOT NULL,
          ext TEXT NOT NULL,
          size INTEGER NOT NULL,
          mtime INTEGER NOT NULL,
          ctime INTEGER,
          file_key TEXT,
          status TEXT NOT NULL DEFAULT 'active',
          UNIQUE(folder_id, name)
        );

        CREATE TABLE media (
          file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
          kind TEXT NOT NULL,
          width INTEGER,
          height INTEGER,
          duration_ms INTEGER,
          codec TEXT,
          orientation INTEGER,
          has_alpha INTEGER,
          dominant_color TEXT,
          phash TEXT,
          metadata_status TEXT NOT NULL DEFAULT 'pending'
        );

        CREATE TABLE thumbs (
          file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
          profile TEXT NOT NULL,
          cache_key TEXT,
          width INTEGER,
          height INTEGER,
          byte_size INTEGER,
          state TEXT NOT NULL,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(file_id, profile)
        );

        CREATE TABLE thumbs_new(stale TEXT);

        INSERT INTO schema_migrations(version, name, applied_at)
        VALUES
          (1, 'initial', unixepoch()),
          (2, 'task_progress', unixepoch()),
          (3, 'browsing_indexes', unixepoch());

        INSERT INTO roots(id, path, display_name, enabled, created_at)
        VALUES (1, 'D:/Pictures', 'Pictures', 1, 1);

        INSERT INTO folders(id, root_id, parent_id, name, path_hash, mtime)
        VALUES (1, 1, NULL, '', 'root-hash', 1);

        INSERT INTO files(id, root_id, folder_id, name, ext, size, mtime)
        VALUES
          (1, 1, 1, 'explicit.jpg', '.jpg', 100, 1),
          (2, 1, 1, 'queued.jpg', '.jpg', 100, 2),
          (3, 1, 1, 'tiny.jpg', '.jpg', 100, 3),
          (4, 1, 1, 'absolute.jpg', '.jpg', 100, 4),
          (5, 1, 1, 'ready.jpg', '.jpg', 100, 5);

        INSERT INTO media(file_id, kind, width, height, metadata_status)
        VALUES
          (1, 'image', 640, 480, 'ready'),
          (2, 'image', 640, 480, 'ready'),
          (3, 'image', 640, 480, 'ready'),
          (4, 'image', 640, 480, 'ready'),
          (5, 'image', 640, 480, 'ready');

        INSERT INTO thumbs(file_id, profile, cache_key, width, height, byte_size, state, updated_at)
        VALUES
          (1, 'grid', 'legacy/grid.webp', 320, 240, 111, 'ready', 10),
          (1, 'grid_320', 'explicit/stale.webp', 320, 240, 222, 'failed', 20),
          (2, 'grid', 'queued/stale.webp', 320, 240, 333, 'queued', 30),
          (3, 'tiny', 'tiny.webp', 80, 60, 444, 'ready', 40),
          (4, 'grid', 'C:\absolute\thumb.webp', 320, 240, 555, 'ready', 50),
          (5, 'grid', 'aa/bb/ready.webp', 320, 240, 666, 'ready', 60);
    "#;

    const VERSION_4_SCHEMA_WITH_READY_THUMBNAIL_WITHOUT_FINGERPRINT: &str = r#"
        PRAGMA foreign_keys = ON;

        CREATE TABLE schema_migrations (
          version INTEGER PRIMARY KEY,
          name TEXT NOT NULL,
          applied_at INTEGER NOT NULL
        );

        CREATE TABLE roots (
          id INTEGER PRIMARY KEY,
          path TEXT NOT NULL UNIQUE,
          display_name TEXT NOT NULL,
          enabled INTEGER NOT NULL DEFAULT 1,
          created_at INTEGER NOT NULL,
          last_scan_at INTEGER
        );

        CREATE TABLE folders (
          id INTEGER PRIMARY KEY,
          root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
          parent_id INTEGER REFERENCES folders(id) ON DELETE CASCADE,
          name TEXT NOT NULL,
          path_hash TEXT NOT NULL,
          mtime INTEGER,
          status TEXT NOT NULL DEFAULT 'active',
          UNIQUE(root_id, parent_id, name)
        );

        CREATE TABLE files (
          id INTEGER PRIMARY KEY,
          root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
          folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
          name TEXT NOT NULL,
          ext TEXT NOT NULL,
          size INTEGER NOT NULL,
          mtime INTEGER NOT NULL,
          ctime INTEGER,
          file_key TEXT,
          status TEXT NOT NULL DEFAULT 'active',
          UNIQUE(folder_id, name)
        );

        CREATE TABLE media (
          file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
          kind TEXT NOT NULL,
          width INTEGER,
          height INTEGER,
          duration_ms INTEGER,
          codec TEXT,
          orientation INTEGER,
          has_alpha INTEGER,
          dominant_color TEXT,
          phash TEXT,
          metadata_status TEXT NOT NULL DEFAULT 'pending'
        );

        CREATE TABLE thumbs (
          file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
          profile TEXT NOT NULL CHECK(profile IN ('grid_320')),
          state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending', 'queued', 'ready', 'failed', 'skipped_small')),
          cache_key TEXT,
          width INTEGER,
          height INTEGER,
          byte_size INTEGER,
          short_side_px INTEGER NOT NULL DEFAULT 320 CHECK(short_side_px = 320),
          output_format TEXT NOT NULL DEFAULT 'image/webp' CHECK(output_format = 'image/webp'),
          error TEXT,
          updated_at INTEGER NOT NULL,
          PRIMARY KEY(file_id, profile)
        );

        CREATE TABLE tasks (
          id INTEGER PRIMARY KEY,
          kind TEXT NOT NULL,
          priority INTEGER NOT NULL,
          status TEXT NOT NULL,
          root_id INTEGER REFERENCES roots(id) ON DELETE SET NULL,
          file_id INTEGER REFERENCES files(id) ON DELETE SET NULL,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          items_seen INTEGER NOT NULL DEFAULT 0,
          items_total INTEGER,
          folders_seen INTEGER NOT NULL DEFAULT 0,
          media_files_seen INTEGER NOT NULL DEFAULT 0,
          skipped_files INTEGER NOT NULL DEFAULT 0,
          error TEXT
        );

        INSERT INTO schema_migrations(version, name, applied_at)
        VALUES
          (1, 'initial', unixepoch()),
          (2, 'task_progress', unixepoch()),
          (3, 'browsing_indexes', unixepoch()),
          (4, 'thumbnail_state', unixepoch());

        INSERT INTO roots(id, path, display_name, enabled, created_at)
        VALUES (1, 'D:/Pictures', 'Pictures', 1, 1);

        INSERT INTO folders(id, root_id, parent_id, name, path_hash, mtime)
        VALUES (1, 1, NULL, '', 'root-hash', 1);

        INSERT INTO files(id, root_id, folder_id, name, ext, size, mtime)
        VALUES
          (1, 1, 1, 'legacy-ready.jpg', '.jpg', 100, 1),
          (2, 1, 1, 'legacy-skipped.jpg', '.jpg', 100, 2);

        INSERT INTO media(file_id, kind, width, height, metadata_status)
        VALUES
          (1, 'image', 640, 480, 'ready'),
          (2, 'image', 128, 128, 'ready');

        INSERT INTO thumbs(file_id, profile, state, cache_key, width, height, byte_size, updated_at)
        VALUES (1, 'grid_320', 'ready', 'aa/bb/legacy.webp', 320, 320, 16, 10);

        INSERT INTO thumbs(file_id, profile, state, updated_at)
        VALUES (2, 'grid_320', 'skipped_small', 11);
    "#;

    #[test]
    fn add_root_lists_and_reenables_existing_root() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");

        let same_root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Photos".to_string(),
            })
            .expect("upsert root");

        let roots = database.list_roots().expect("list roots");
        assert_eq!(root_id, same_root_id);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].path, "D:/Pictures");
        assert_eq!(roots[0].display_name, "Photos");
        assert!(roots[0].enabled);
    }

    #[test]
    fn reenabling_disabled_root_preserves_files_for_scan_reconcile() {
        // Re-adding a previously-disabled root must NOT wipe its folders/files
        // inline. `DELETE FROM files WHERE root_id` (multiplied by the files
        // table's many indexes) is an O(files) write; on a 600k-file root it ran
        // inside the single DB writer and froze every other write (add-root,
        // priority-scope, thumbnail publish) for minutes, past the client
        // request timeout. The subsequent root scan already reconciles stale
        // rows (upsert seen + mark unseen `missing` via
        // `reconcile_root_scan_completion`), so the wipe is redundant; dropping
        // it also preserves user_metadata/tags/thumbs across a disable ->
        // re-enable cycle.
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root-hash".to_string(),
                mtime: Some(10),
            })
            .expect("insert root folder");
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "keep.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 100,
                ctime: None,
                file_key: None,
            })
            .expect("insert file");

        assert!(database.disable_root(root_id).expect("disable root"));

        let same_root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("re-enable root");
        assert_eq!(same_root_id, root_id);

        let files_remaining: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id = ?1",
                [root_id],
                |row| row.get(0),
            )
            .expect("count files");
        let folders_remaining: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE root_id = ?1",
                [root_id],
                |row| row.get(0),
            )
            .expect("count folders");
        assert_eq!(files_remaining, 1, "re-enable must not delete files inline");
        assert_eq!(
            folders_remaining, 1,
            "re-enable must not delete folders inline"
        );
        // Row identity is preserved (same file_id), so cascaded
        // user_metadata/tags/thumbs survive the disable -> re-enable cycle.
        let preserved: Option<i64> = database
            .connection
            .query_row("SELECT id FROM files WHERE id = ?1", [file_id], |row| {
                row.get(0)
            })
            .optional()
            .expect("query file by id");
        assert_eq!(preserved, Some(file_id));
    }

    #[test]
    fn migrations_upgrade_old_initial_tasks_table_with_progress_columns() {
        let database = Database::open_in_memory().expect("open in-memory database");
        database
            .connection
            .execute_batch(OLD_INITIAL_MIGRATION_WITHOUT_TASK_PROGRESS)
            .expect("apply old initial migration");

        // Seed a root + root_scan task directly against the OLD (pre-migration)
        // schema via raw SQL. `add_root`/`create_root_scan_task` cannot be used
        // here: `create_root_scan_task` calls `get_root`, whose SELECT now
        // references `roots.pending_delete_at` (added by migration 0019), which
        // does not exist on this old schema snapshot. Inserting the rows by hand
        // against the OLD_INITIAL_MIGRATION_WITHOUT_TASK_PROGRESS column set is
        // what actually proves the migration backfills pre-existing rows.
        let now = unix_timestamp();
        database
            .connection
            .execute(
                "INSERT INTO roots(path, display_name, enabled, created_at) VALUES (?1, ?2, 1, ?3)",
                rusqlite::params!["D:/Pictures", "Pictures", now],
            )
            .expect("insert root row under old schema");
        let root_id = database.connection.last_insert_rowid();
        database
            .connection
            .execute(
                "INSERT INTO tasks(kind, priority, status, root_id, file_id, created_at, updated_at, error)
                 VALUES ('root_scan', ?1, 'pending', ?2, NULL, ?3, ?3, NULL)",
                rusqlite::params![ROOT_SCAN_TASK_PRIORITY, root_id, now],
            )
            .expect("insert root_scan task row under old schema");

        database
            .apply_migrations()
            .expect("apply current migrations");
        let columns: Vec<String> = {
            let mut statement = database
                .connection
                .prepare("PRAGMA table_info(tasks)")
                .expect("inspect tasks table");
            statement
                .query_map([], |row| row.get(1))
                .expect("query columns")
                .collect::<rusqlite::Result<Vec<String>>>()
                .expect("collect columns")
        };
        for column in [
            "folder_id",
            "items_seen",
            "items_total",
            "folders_seen",
            "media_files_seen",
            "skipped_files",
            "thumbnail_source_fingerprint",
        ] {
            assert!(columns.iter().any(|candidate| candidate == column));
        }

        let tasks = database.list_tasks().expect("list upgraded tasks");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].items_seen, 0);
        assert_eq!(tasks[0].items_total, None);
        assert_eq!(tasks[0].folder_id, None);
        assert_eq!(tasks[0].folders_seen, 0);
        assert_eq!(tasks[0].media_files_seen, 0);
        assert_eq!(tasks[0].skipped_files, 0);
        assert_eq!(tasks[0].thumbnail_source_fingerprint, None);
        let invalid_status = database
            .connection
            .execute(
                "UPDATE OR IGNORE tasks SET status = 'paused' WHERE id = ?1",
                [tasks[0].id],
            )
            .expect("invalid task status should be ignored");
        assert_eq!(invalid_status, 0);
    }

    #[test]
    fn thumbnails_state_migration_is_rerunnable_and_prefers_explicit_grid_320_rows() {
        let database = Database::open_in_memory().expect("open in-memory database");
        database
            .connection
            .execute_batch(OLD_SCHEMA_WITH_LEGACY_THUMBS)
            .expect("apply old thumbnail schema");

        database
            .apply_migrations()
            .expect("apply thumbnail state migration");
        database
            .apply_migrations()
            .expect("rerun migrations after thumbnail state migration");

        let stale_table_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'thumbs_new'",
                [],
                |row| row.get(0),
            )
            .expect("count stale temp tables");
        assert_eq!(stale_table_count, 0);

        let foreign_keys_enabled: i64 = database
            .connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign_keys pragma");
        assert_eq!(foreign_keys_enabled, 1);

        let foreign_key_violations: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .expect("foreign key check");
        assert_eq!(foreign_key_violations, 0);

        let explicit = database
            .get_thumbnail(1, "grid_320")
            .expect("get explicit thumbnail")
            .expect("explicit thumbnail exists");
        assert_eq!(explicit.state, "pending");
        assert_eq!(explicit.cache_key, None);
        assert_eq!(explicit.width, None);
        assert_eq!(explicit.height, None);
        assert_eq!(explicit.byte_size, None);
        assert_eq!(explicit.error, None);
        assert_eq!(explicit.updated_at, Some(20));

        let queued = database
            .get_thumbnail(2, "grid_320")
            .expect("get queued thumbnail")
            .expect("queued thumbnail exists");
        assert_eq!(queued.state, "queued");
        assert_eq!(queued.cache_key, None);
        assert_eq!(queued.width, None);
        assert_eq!(queued.height, None);
        assert_eq!(queued.byte_size, None);

        let unsupported_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM thumbs WHERE file_id = 3", [], |row| {
                row.get(0)
            })
            .expect("count unsupported profile rows");
        assert_eq!(unsupported_count, 0);

        let absolute = database
            .get_thumbnail(4, "grid_320")
            .expect("get absolute thumbnail")
            .expect("absolute thumbnail exists");
        assert_eq!(absolute.state, "pending");
        assert_eq!(absolute.cache_key, None);
        assert_eq!(absolute.width, None);
        assert_eq!(absolute.height, None);
        assert_eq!(absolute.byte_size, None);

        let ready = database
            .get_thumbnail(5, "grid_320")
            .expect("get ready thumbnail")
            .expect("ready thumbnail exists");
        assert_eq!(ready.state, "pending");
        assert_eq!(ready.cache_key, None);
        assert_eq!(ready.width, None);
        assert_eq!(ready.height, None);
        assert_eq!(ready.byte_size, None);
        assert_eq!(ready.source_fingerprint, None);
    }

    #[test]
    fn source_fingerprint_migration_upgrades_v4_database_and_invalidates_legacy_ready_rows() {
        let database = Database::open_in_memory().expect("open in-memory database");
        database
            .connection
            .execute_batch(VERSION_4_SCHEMA_WITH_READY_THUMBNAIL_WITHOUT_FINGERPRINT)
            .expect("apply version 4 schema");

        database
            .apply_migrations()
            .expect("apply source fingerprint migration");

        let thumb_columns: Vec<String> = {
            let mut statement = database
                .connection
                .prepare("PRAGMA table_info(thumbs)")
                .expect("inspect thumbs table");
            statement
                .query_map([], |row| row.get(1))
                .expect("query columns")
                .collect::<rusqlite::Result<Vec<String>>>()
                .expect("collect columns")
        };
        assert!(thumb_columns
            .iter()
            .any(|candidate| candidate == "source_fingerprint"));
        let version_5_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 5",
                [],
                |row| row.get(0),
            )
            .expect("query version 5");
        assert_eq!(version_5_count, 1);
        let version_6_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 6",
                [],
                |row| row.get(0),
            )
            .expect("query version 6");
        assert_eq!(version_6_count, 1);
        let version_7_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 7",
                [],
                |row| row.get(0),
            )
            .expect("query version 7");
        assert_eq!(version_7_count, 1);

        let thumbnail = database
            .get_thumbnail(1, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "pending");
        assert_eq!(thumbnail.cache_key, None);
        assert_eq!(thumbnail.width, None);
        assert_eq!(thumbnail.height, None);
        assert_eq!(thumbnail.byte_size, None);
        assert_eq!(thumbnail.source_fingerprint, None);

        let skipped = database
            .get_thumbnail(2, crate::thumbnails::GRID_320_PROFILE)
            .expect("get skipped thumbnail")
            .expect("skipped thumbnail exists");
        assert_eq!(skipped.state, "pending");
        assert_eq!(skipped.source_fingerprint, None);
    }

    #[test]
    fn task_attempt_generation_migration_recovers_partial_apply_and_defaults_existing_tasks() {
        let database = Database::open_in_memory().expect("open in-memory database");
        database
            .connection
            .execute_batch(migrations::INITIAL_MIGRATION)
            .expect("apply initial migration");
        database
            .connection
            .execute_batch(migrations::TASK_PROGRESS_MIGRATION)
            .expect("apply task progress migration");
        database
            .connection
            .execute_batch(migrations::BROWSING_INDEXES_MIGRATION)
            .expect("apply browsing indexes migration");
        database
            .connection
            .execute_batch(migrations::THUMBNAIL_STATE_MIGRATION)
            .expect("apply thumbnail state migration");
        database
            .connection
            .execute_batch(migrations::THUMBNAIL_SOURCE_FINGERPRINT_MIGRATION)
            .expect("apply thumbnail source fingerprint migration");
        database
            .connection
            .execute_batch(migrations::THUMBNAIL_TASK_ATTEMPT_FINGERPRINT_MIGRATION)
            .expect("apply thumbnail task fingerprint migration");
        database
            .connection
            .execute_batch(migrations::TASK_STATUS_CONTRACT_MIGRATION)
            .expect("apply task status contract migration");

        // Seed a root + root_scan task directly against the schema as it stands
        // right after 0007 (task_status_contract), i.e. BEFORE the v8 attempt
        // generation migration runs. `add_root`/`create_root_scan_task` cannot be
        // used here: `create_root_scan_task` calls `get_root`, whose SELECT now
        // references `roots.pending_delete_at` (added by migration 0019), which
        // does not exist yet on this snapshot. Raw SQL against the exact 0007
        // column set is what proves v8 backfills `attempt_generation` on
        // pre-existing rows rather than merely on freshly-inserted ones.
        let now = unix_timestamp();
        database
            .connection
            .execute(
                "INSERT INTO roots(path, display_name, enabled, created_at) VALUES (?1, ?2, 1, ?3)",
                rusqlite::params!["D:/Pictures", "Pictures", now],
            )
            .expect("insert root row before v8");
        let root_id = database.connection.last_insert_rowid();
        database
            .connection
            .execute(
                "INSERT INTO tasks(kind, priority, status, root_id, file_id, created_at, updated_at, error)
                 VALUES ('root_scan', ?1, 'pending', ?2, NULL, ?3, ?3, NULL)",
                rusqlite::params![ROOT_SCAN_TASK_PRIORITY, root_id, now],
            )
            .expect("insert root_scan task row before v8");
        let task_id = database.connection.last_insert_rowid();

        database
            .apply_migrations()
            .expect("apply v8 attempt generation migration");
        let task = database
            .get_task(task_id)
            .expect("get migrated task")
            .expect("task exists");
        assert_eq!(task.attempt_generation, 0);
        let version_8_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 8",
                [],
                |row| row.get(0),
            )
            .expect("query version 8");
        assert_eq!(version_8_count, 1);

        database
            .connection
            .execute("DELETE FROM schema_migrations WHERE version = 8", [])
            .expect("simulate missing v8 version after partial apply");
        database
            .apply_migrations()
            .expect("recover partial v8 apply without duplicate column failure");
        let version_8_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 8",
                [],
                |row| row.get(0),
            )
            .expect("query recovered version 8");
        assert_eq!(version_8_count, 1);
    }

    #[test]
    fn task_attempt_mutators_expose_only_attempt_guarded_public_surface() {
        let source = include_str!("mod.rs");
        for forbidden in [
            concat!("pub fn ", "mark_task_running", "("),
            concat!("pub fn ", "mark_thumbnail_task_running", "("),
            concat!("pub fn ", "mark_task_succeeded", "("),
            concat!("pub fn ", "mark_task_failed", "("),
            concat!("pub fn ", "update_task_scan_progress", "("),
        ] {
            assert!(
                !source.contains(forbidden),
                "unguarded task attempt mutator remains public: {forbidden}"
            );
        }
    }

    #[test]
    fn terminal_thumbnail_without_source_fingerprint_is_not_exposed_as_current() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_thumbnail_cache_scope_fixture(&database, "cache-enqueue");
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "legacy-null-fingerprint.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: None,
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        database
            .connection
            .execute(
                r#"
                INSERT INTO thumbs(file_id, profile, state, cache_key, width, height, byte_size, updated_at)
                VALUES (?1, 'grid_320', 'ready', 'aa/bb/legacy-null.webp', 320, 320, 16, 10)
                "#,
                [file_id],
            )
            .expect("insert legacy ready thumbnail");

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");

        assert_eq!(thumbnail.state, "pending");
        assert_eq!(thumbnail.cache_key, None);
        assert_eq!(thumbnail.width, None);
        assert_eq!(thumbnail.height, None);
        assert_eq!(thumbnail.byte_size, None);

        database
            .connection
            .execute(
                "UPDATE thumbs SET state = 'skipped_small', cache_key = NULL, width = NULL, height = NULL, byte_size = NULL WHERE file_id = ?1",
                [file_id],
            )
            .expect("mark legacy skipped thumbnail");
        let skipped = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get skipped thumbnail")
            .expect("thumbnail exists");
        assert_eq!(skipped.state, "pending");
    }

    #[test]
    fn list_media_page_returns_media_and_grid_thumbnail_state() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");

        database
            .connection
            .execute(
                r#"
                INSERT INTO folders(root_id, parent_id, name, path_hash, mtime)
                VALUES (?1, NULL, '', 'root-hash', 10)
                "#,
                [root_id],
            )
            .expect("insert folder");
        let folder_id = database.connection.last_insert_rowid();

        database
            .connection
            .execute(
                r#"
                INSERT INTO files(root_id, folder_id, name, ext, size, mtime)
                VALUES (?1, ?2, 'image.jpg', '.jpg', 1000, 20)
                "#,
                (root_id, folder_id),
            )
            .expect("insert file");
        let file_id = database.connection.last_insert_rowid();

        database
            .connection
            .execute(
                r#"
                INSERT INTO media(file_id, kind, width, height, metadata_status)
                VALUES (?1, 'image', 640, 480, 'ready')
                "#,
                [file_id],
            )
            .expect("insert media");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get thumbnail source")
            .expect("thumbnail source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);

        database
            .connection
            .execute(
                r#"
                INSERT INTO thumbs(
                    file_id, profile, cache_key, width, height, byte_size,
                    state, source_fingerprint, updated_at
                )
                VALUES (?1, 'grid_320', 'aa/bb/key.webp', 427, 320, 4096, 'ready', ?2, 30)
                "#,
                (file_id, source_fingerprint),
            )
            .expect("insert thumbnail");

        let page = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
                limit: 200,
                offset: None,
                cursor: None,
                sort: "mtime_desc".to_string(),
                kind: Some("image".to_string()),
            })
            .expect("list media page");

        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, file_id);
        assert_eq!(page.items[0].name, "image.jpg");
        assert_eq!(page.items[0].kind.as_deref(), Some("image"));
        assert_eq!(page.items[0].thumbnail_state.as_deref(), Some("ready"));
        assert_eq!(page.items[0].thumbnail_cache_key, None);

        let item = database
            .get_media(file_id)
            .expect("get media")
            .expect("media exists");
        assert_eq!(item.id, file_id);
        assert_eq!(item.name, "image.jpg");
        assert!(database
            .get_media(file_id + 10)
            .expect("get missing")
            .is_none());
    }

    #[test]
    fn media_records_keep_thumbnail_cache_key_null_for_db_blob_pipeline() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        for (index, name) in [
            "empty-cache.jpg",
            "dot-slash.jpg",
            "double-slash.jpg",
            "dot-segment.jpg",
        ]
        .iter()
        .enumerate()
        {
            let file_id = database
                .upsert_file(FileUpsert {
                    root_id,
                    folder_id,
                    name: (*name).to_string(),
                    ext: ".jpg".to_string(),
                    size: 200 + index as i64,
                    mtime: 200 + index as i64,
                    ctime: None,
                    file_key: None,
                })
                .expect("insert invalid cache-key file");
            database
                .upsert_media_kind(file_id, "image")
                .expect("insert media kind");
        }
        let file_ids: Vec<i64> = database
            .connection
            .prepare("SELECT id FROM files WHERE root_id = ?1 ORDER BY name ASC")
            .expect("prepare expanded file ids")
            .query_map([root_id], |row| row.get(0))
            .expect("query expanded file ids")
            .collect::<rusqlite::Result<Vec<i64>>>()
            .expect("collect expanded file ids");

        for (file_id, state, cache_key) in [
            (file_ids[0], "ready", "aa/bb/ready.webp"),
            (file_ids[1], "queued", "queued/stale.webp"),
            (file_ids[2], "ready", "C:\\absolute\\thumb.webp"),
            (file_ids[3], "ready", "../escape.webp"),
            (file_ids[4], "ready", ""),
            (file_ids[5], "ready", "./thumb.webp"),
            (file_ids[6], "ready", "aa//thumb.webp"),
            (file_ids[7], "ready", "aa/./thumb.webp"),
        ] {
            let source_fingerprint = if state == "ready" {
                Some(
                    database
                        .get_thumbnail_source(file_id)
                        .expect("get thumbnail source")
                        .expect("thumbnail source exists")
                        .source_fingerprint(crate::thumbnails::GRID_320_PROFILE),
                )
            } else {
                None
            };
            database
                .connection
                .execute(
                    r#"
                    INSERT INTO thumbs(
                        file_id, profile, state, cache_key, width, height, byte_size,
                        source_fingerprint, updated_at
                    )
                    VALUES (?1, 'grid_320', ?2, ?3, 320, 240, 1024, ?4, 100)
                    "#,
                    (file_id, state, cache_key, source_fingerprint),
                )
                .expect("insert thumbnail state");
        }

        let page = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: Some("image".to_string()),
            })
            .expect("list media page");
        assert_eq!(
            media_names(&page.items),
            vec![
                "alpha.jpg",
                "bravo.jpg",
                "charlie.jpg",
                "delta.jpg",
                "dot-segment.jpg",
                "dot-slash.jpg",
                "double-slash.jpg",
                "empty-cache.jpg"
            ]
        );
        assert_eq!(page.items[0].thumbnail_cache_key, None);
        assert_eq!(page.items[1].thumbnail_state.as_deref(), Some("queued"));
        assert_eq!(page.items[1].thumbnail_cache_key, None);
        assert_eq!(page.items[2].thumbnail_state.as_deref(), Some("ready"));
        assert_eq!(page.items[2].thumbnail_cache_key, None);
        assert_eq!(page.items[3].thumbnail_state.as_deref(), Some("ready"));
        assert_eq!(page.items[3].thumbnail_cache_key, None);
        for item in &page.items[4..] {
            assert_eq!(item.thumbnail_state.as_deref(), Some("ready"));
            assert_eq!(item.thumbnail_cache_key, None);
        }

        let absolute = database
            .get_media(file_ids[2])
            .expect("get media")
            .expect("media exists");
        assert_eq!(absolute.thumbnail_state.as_deref(), Some("ready"));
        assert_eq!(absolute.thumbnail_cache_key, None);
        let dot_segment = database
            .get_media(file_ids[7])
            .expect("get dot segment media")
            .expect("media exists");
        assert_eq!(dot_segment.thumbnail_state.as_deref(), Some("ready"));
        assert_eq!(dot_segment.thumbnail_cache_key, None);
    }

    #[test]
    fn media_records_normalize_terminal_thumbnail_state_by_source_fingerprint() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let cases = [
            ("current-ready.jpg", "ready", "current"),
            ("null-ready.jpg", "ready", "null"),
            ("stale-ready.jpg", "ready", "stale"),
            ("null-failed.jpg", "failed", "null"),
            ("stale-failed.jpg", "failed", "stale"),
            ("null-skipped.jpg", "skipped_small", "null"),
            ("stale-skipped.jpg", "skipped_small", "stale"),
        ];
        let mut expected = Vec::new();

        for (index, (name, state, fingerprint_mode)) in cases.iter().enumerate() {
            let file_id = database
                .upsert_file(FileUpsert {
                    root_id,
                    folder_id,
                    name: (*name).to_string(),
                    ext: ".jpg".to_string(),
                    size: 900 + index as i64,
                    mtime: 900 + index as i64,
                    ctime: None,
                    file_key: Some(format!("identity-{index}")),
                })
                .expect("insert thumbnail freshness file");
            database
                .upsert_media_kind(file_id, "image")
                .expect("insert media kind");
            database
                .connection
                .execute(
                    "UPDATE media SET width = 640, height = 480, metadata_status = 'ready' WHERE file_id = ?1",
                    [file_id],
                )
                .expect("set media metadata");
            let current_source_fingerprint = database
                .get_thumbnail_source(file_id)
                .expect("get thumbnail source")
                .expect("thumbnail source exists")
                .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
            let source_fingerprint = match *fingerprint_mode {
                "current" => Some(current_source_fingerprint),
                "stale" => Some("stale-source-fingerprint-that-must-not-match".to_string()),
                "null" => None,
                other => panic!("unsupported fingerprint mode: {other}"),
            };
            database
                .upsert_thumbnail_state(ThumbnailStateUpsert {
                    file_id,
                    profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                    state: (*state).to_string(),
                    cache_key: Some(format!("aa/bb/{name}.webp")),
                    width: Some(320),
                    height: Some(320),
                    byte_size: Some(64),
                    error: if *state == "failed" {
                        Some("decode failed".to_string())
                    } else {
                        None
                    },
                    source_fingerprint,
                })
                .expect("insert thumbnail state");
            expected.push((file_id, *name, *fingerprint_mode));
        }

        let page = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
                limit: 100,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: Some("image".to_string()),
            })
            .expect("list media page");

        for (file_id, name, fingerprint_mode) in expected {
            let page_item = page
                .items
                .iter()
                .find(|item| item.id == file_id)
                .expect("page item exists");
            let detail_item = database
                .get_media(file_id)
                .expect("get media detail")
                .expect("media detail exists");
            assert_eq!(page_item.name, name);
            assert_eq!(detail_item.name, name);
            if fingerprint_mode == "current" {
                assert_eq!(page_item.thumbnail_state.as_deref(), Some("ready"));
                assert_eq!(detail_item.thumbnail_state.as_deref(), Some("ready"));
                assert_eq!(page_item.thumbnail_cache_key, None);
                assert_eq!(detail_item.thumbnail_cache_key, None);
            } else {
                assert_eq!(page_item.thumbnail_state.as_deref(), Some("pending"));
                assert_eq!(page_item.thumbnail_cache_key, None);
                assert_eq!(detail_item.thumbnail_state.as_deref(), Some("pending"));
                assert_eq!(detail_item.thumbnail_cache_key, None);
            }
        }
    }

    #[test]
    fn thumbnails_persist_grid_320_webp_state_and_reject_unknown_values() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id: i64 = database
            .connection
            .query_row(
                "SELECT id FROM files WHERE root_id = ?1 AND folder_id = ?2 ORDER BY id LIMIT 1",
                (root_id, folder_id),
                |row| row.get(0),
            )
            .expect("seeded file id");

        database
            .connection
            .execute(
                r#"
                INSERT INTO thumbs(
                    file_id, profile, state, cache_key, width, height, byte_size,
                    short_side_px, output_format, updated_at
                )
                VALUES (?1, 'grid_320', 'pending', NULL, NULL, NULL, NULL, 320, 'image/webp', 100)
                "#,
                [file_id],
            )
            .expect("insert pending thumbnail state");

        for state in ["queued", "ready", "failed", "skipped_small"] {
            database
                .connection
                .execute(
                    r#"
                    UPDATE thumbs
                    SET state = ?1,
                        cache_key = CASE WHEN ?1 = 'ready' THEN 'aa/bb/key.webp' ELSE NULL END,
                        width = CASE WHEN ?1 = 'ready' THEN 427 ELSE NULL END,
                        height = CASE WHEN ?1 = 'ready' THEN 320 ELSE NULL END,
                        byte_size = CASE WHEN ?1 = 'ready' THEN 4096 ELSE NULL END,
                        error = CASE WHEN ?1 = 'failed' THEN 'decode failed' ELSE NULL END
                    WHERE file_id = ?2 AND profile = 'grid_320'
                    "#,
                    (state, file_id),
                )
                .expect("update thumbnail state");
        }

        let row: (String, i64, String, Option<String>) = database
            .connection
            .query_row(
                r#"
                SELECT state, short_side_px, output_format, cache_key
                FROM thumbs
                WHERE file_id = ?1 AND profile = 'grid_320'
                "#,
                [file_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("thumbnail state row");
        assert_eq!(row.0, "skipped_small");
        assert_eq!(row.1, 320);
        assert_eq!(row.2, "image/webp");
        assert_eq!(row.3, None);

        let invalid_status = database
            .connection
            .execute(
                r#"
                UPDATE OR IGNORE thumbs
                SET state = 'unknown'
                WHERE file_id = ?1 AND profile = 'grid_320'
                "#,
                [file_id],
            )
            .expect("invalid thumbnail status is ignored by check constraint");
        assert_eq!(invalid_status, 0);

        let invalid_profile = database
            .connection
            .execute(
                r#"
                INSERT OR IGNORE INTO thumbs(file_id, profile, state, short_side_px, output_format, updated_at)
                VALUES (?1, 'grid', 'pending', 320, 'image/webp', 101)
                "#,
                [file_id],
            )
            .expect("invalid thumbnail profile is ignored by check constraint");
        assert_eq!(invalid_profile, 0);

        let invalid_format = database
            .connection
            .execute(
                r#"
                UPDATE OR IGNORE thumbs
                SET output_format = 'image/jpeg'
                WHERE file_id = ?1 AND profile = 'grid_320'
                "#,
                [file_id],
            )
            .expect("invalid thumbnail format is ignored by check constraint");
        assert_eq!(invalid_format, 0);
    }

    #[test]
    fn list_media_page_keyset_pages_all_supported_sorts_without_offset() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);

        for (sort, expected_pages) in [
            (
                "name_asc",
                vec![
                    vec!["alpha.jpg", "bravo.jpg"],
                    vec!["charlie.jpg", "delta.jpg"],
                ],
            ),
            (
                "name_desc",
                vec![
                    vec!["delta.jpg", "charlie.jpg"],
                    vec!["bravo.jpg", "alpha.jpg"],
                ],
            ),
            (
                "mtime_desc",
                vec![
                    vec!["delta.jpg", "charlie.jpg"],
                    vec!["bravo.jpg", "alpha.jpg"],
                ],
            ),
            (
                "mtime_asc",
                vec![
                    vec!["alpha.jpg", "bravo.jpg"],
                    vec!["charlie.jpg", "delta.jpg"],
                ],
            ),
        ] {
            let first_page = database
                .list_media_page(MediaPageQuery {
                    root_id: Some(root_id),
                    folder_id: Some(folder_id),
                    include_descendants: false,
                    limit: 2,
                    offset: None,
                    cursor: None,
                    sort: sort.to_string(),
                    kind: Some("image".to_string()),
                })
                .expect("list first media page");
            assert_eq!(media_names(&first_page.items), expected_pages[0]);
            let cursor = first_page.next_cursor.expect("first page cursor");

            let second_page = database
                .list_media_page(MediaPageQuery {
                    root_id: Some(root_id),
                    folder_id: Some(folder_id),
                    include_descendants: false,
                    limit: 2,
                    offset: None,
                    cursor: Some(cursor),
                    sort: sort.to_string(),
                    kind: Some("image".to_string()),
                })
                .expect("list second media page");
            assert_eq!(media_names(&second_page.items), expected_pages[1]);
            assert!(second_page.next_cursor.is_none());
        }
    }

    #[test]
    fn list_media_page_reports_total_count_on_first_page_only() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);

        // The first page (offset 0) carries the total so the client can size
        // the virtual grid once.
        let first_page = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
                limit: 2,
                offset: Some(0),
                cursor: None,
                sort: "name_asc".to_string(),
                kind: Some("image".to_string()),
            })
            .expect("list first media page");
        assert_eq!(first_page.total_count, Some(4));
        assert_eq!(first_page.items.len(), 2);

        // Later offset windows omit COUNT(*) (the client retains the total),
        // while still returning the correct items and cursor.
        let page = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
                limit: 2,
                offset: Some(1),
                cursor: None,
                sort: "name_asc".to_string(),
                kind: Some("image".to_string()),
            })
            .expect("list offset media page");

        assert_eq!(page.total_count, None);
        assert_eq!(media_names(&page.items), vec!["bravo.jpg", "charlie.jpg"]);
        assert!(page.next_cursor.is_some());

        let root_descendant_page = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: true,
                limit: 2,
                offset: Some(1),
                cursor: None,
                sort: "name_asc".to_string(),
                kind: Some("image".to_string()),
            })
            .expect("list root descendant offset media page");

        assert_eq!(root_descendant_page.total_count, None);
        assert_eq!(
            media_names(&root_descendant_page.items),
            vec!["bravo.jpg", "charlie.jpg"]
        );
        assert!(root_descendant_page.next_cursor.is_some());
    }

    #[test]
    fn list_media_count_sql_joins_media_only_for_kind_filter() {
        let without_kind = media_count_sql("files.status = 'active'", false);
        assert!(!without_kind.contains("JOIN media"));
        assert!(!without_kind.contains("media.file_id"));

        let with_kind = media_count_sql("files.status = 'active' AND media.kind = ?", true);
        assert!(with_kind.contains("LEFT JOIN media ON media.file_id = files.id"));
    }

    #[test]
    fn list_folder_children_paginates_by_name_with_stable_cursor() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root-hash".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        for name in ["alpha", "bravo", "charlie"] {
            database
                .upsert_folder(FolderUpsert {
                    root_id,
                    parent_id: Some(root_folder_id),
                    name: name.to_string(),
                    path_hash: format!("{name}-hash"),
                    mtime: Some(2),
                })
                .expect("insert child folder");
        }

        let first_page = database
            .list_folder_children_page(root_folder_id, 2, None, false)
            .expect("list first child page");
        assert_eq!(folder_names(&first_page.items), vec!["alpha", "bravo"]);
        let cursor = first_page.next_cursor.expect("first child page cursor");

        let second_page = database
            .list_folder_children_page(root_folder_id, 2, Some(cursor), false)
            .expect("list second child page");
        assert_eq!(folder_names(&second_page.items), vec!["charlie"]);
        assert!(second_page.next_cursor.is_none());
    }

    #[test]
    fn list_folder_ancestors_returns_root_to_leaf_and_handles_missing_and_cycles() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root-hash".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let child_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "child".to_string(),
                path_hash: "child-hash".to_string(),
                mtime: Some(2),
            })
            .expect("insert child folder");
        let leaf_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(child_folder_id),
                name: "leaf".to_string(),
                path_hash: "leaf-hash".to_string(),
                mtime: Some(3),
            })
            .expect("insert leaf folder");

        let ancestors = database
            .list_folder_ancestors(leaf_folder_id)
            .expect("list folder ancestors")
            .expect("leaf folder exists");
        assert_eq!(folder_names(&ancestors), vec!["", "child", "leaf"]);
        assert_eq!(
            ancestors.iter().map(|folder| folder.id).collect::<Vec<_>>(),
            vec![root_folder_id, child_folder_id, leaf_folder_id]
        );
        assert_eq!(ancestors[0].parent_id, None);
        assert!(database
            .list_folder_ancestors(999_999)
            .expect("missing folder lookup")
            .is_none());

        database
            .connection
            .execute(
                "UPDATE folders SET parent_id = ?1 WHERE id = ?1",
                [child_folder_id],
            )
            .expect("inject folder cycle");
        assert!(
            database.list_folder_ancestors(child_folder_id).is_err(),
            "cyclic folder paths must fail instead of looping"
        );
    }

    #[test]
    fn list_folder_ancestors_enforces_depth_cap() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root-hash".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");

        let transaction = database
            .connection
            .unchecked_transaction()
            .expect("start deep folder transaction");
        let mut parent_id = root_folder_id;
        for depth in 0..FOLDER_ANCESTOR_DEPTH_CAP {
            let folder_id = root_folder_id + depth + 1;
            transaction
                .execute(
                    "INSERT INTO folders(id, root_id, parent_id, name, path_hash, mtime) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    (
                        folder_id,
                        root_id,
                        parent_id,
                        format!("folder-{depth}"),
                        format!("hash-{depth}"),
                        1,
                    ),
                )
                .expect("insert deep folder");
            parent_id = folder_id;
        }
        transaction.commit().expect("commit deep folder chain");

        assert!(
            database.list_folder_ancestors(parent_id).is_err(),
            "folder paths beyond the cap must fail promptly"
        );
    }

    #[test]
    fn list_folder_children_page_can_include_descendants_recursively() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root-hash".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let child_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "child".to_string(),
                path_hash: "child-hash".to_string(),
                mtime: Some(2),
            })
            .expect("insert child folder");
        let grandchild_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(child_folder_id),
                name: "grandchild".to_string(),
                path_hash: "grandchild-hash".to_string(),
                mtime: Some(3),
            })
            .expect("insert grandchild folder");
        let sibling_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "sibling".to_string(),
                path_hash: "sibling-hash".to_string(),
                mtime: Some(4),
            })
            .expect("insert sibling folder");

        for (folder_id, name, mtime) in [
            (root_folder_id, "root.jpg", 10),
            (child_folder_id, "child.jpg", 20),
            (grandchild_folder_id, "grandchild.jpg", 30),
            (sibling_folder_id, "sibling.jpg", 40),
        ] {
            let file_id = database
                .upsert_file(FileUpsert {
                    root_id,
                    folder_id,
                    name: name.to_string(),
                    ext: ".jpg".to_string(),
                    size: 1024,
                    mtime,
                    ctime: None,
                    file_key: None,
                })
                .expect("insert folder count test file");
            database
                .upsert_media_kind(file_id, "image")
                .expect("insert folder count test media");
        }

        let roots = database.list_roots().expect("list counted roots");
        assert_eq!(roots[0].media_count, 4);

        let direct_page = database
            .list_folder_children_page(root_folder_id, 10, None, false)
            .expect("list direct children");
        assert_eq!(folder_names(&direct_page.items), vec!["child", "sibling"]);
        // `child` has a grandchild -> hasChildren; `sibling` is a leaf. The UI
        // uses this to avoid flashing an expand chevron on childless folders.
        let child = folder_named(&direct_page.items, "child");
        let sibling = folder_named(&direct_page.items, "sibling");
        assert!(
            child.has_children,
            "child folder should report has_children"
        );
        assert!(
            !sibling.has_children,
            "leaf sibling folder should report no children"
        );
        assert_eq!(child.media_count, 2);
        assert_eq!(sibling.media_count, 1);

        let recursive_page = database
            .list_folder_children_page(root_folder_id, 10, None, true)
            .expect("list recursive children");
        assert_eq!(
            folder_names(&recursive_page.items),
            vec!["child", "grandchild", "sibling"]
        );
        assert!(
            folder_named(&recursive_page.items, "child").has_children,
            "child folder should report has_children in recursive listing"
        );
        assert!(
            !folder_named(&recursive_page.items, "grandchild").has_children,
            "leaf grandchild folder should report no children"
        );
        assert_eq!(folder_named(&recursive_page.items, "child").media_count, 2);
        assert_eq!(
            folder_named(&recursive_page.items, "grandchild").media_count,
            1
        );
        assert_eq!(
            folder_named(&recursive_page.items, "sibling").media_count,
            1
        );
    }

    #[test]
    fn list_media_page_optionally_includes_descendant_folder_media() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "root-hash".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let child_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "child".to_string(),
                path_hash: "child-hash".to_string(),
                mtime: Some(2),
            })
            .expect("insert child folder");
        let grandchild_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(child_folder_id),
                name: "grandchild".to_string(),
                path_hash: "grandchild-hash".to_string(),
                mtime: Some(3),
            })
            .expect("insert grandchild folder");

        for (folder_id, name, mtime) in [
            (root_folder_id, "root.jpg", 10),
            (child_folder_id, "child.jpg", 20),
            (grandchild_folder_id, "grandchild.jpg", 30),
        ] {
            let file_id = database
                .upsert_file(FileUpsert {
                    root_id,
                    folder_id,
                    name: name.to_string(),
                    ext: ".jpg".to_string(),
                    size: 1024,
                    mtime,
                    ctime: None,
                    file_key: None,
                })
                .expect("insert descendant test file");
            database
                .upsert_media_kind(file_id, "image")
                .expect("insert descendant test media");
        }

        let direct_page = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(child_folder_id),
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "mtime_desc".to_string(),
                kind: Some("image".to_string()),
            })
            .expect("list direct folder media");
        assert_eq!(media_names(&direct_page.items), vec!["child.jpg"]);

        let descendant_page = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: Some(child_folder_id),
                include_descendants: true,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "mtime_desc".to_string(),
                kind: Some("image".to_string()),
            })
            .expect("list descendant folder media");
        assert_eq!(
            media_names(&descendant_page.items),
            vec!["grandchild.jpg", "child.jpg"]
        );
    }

    #[test]
    fn disabled_root_is_hidden_from_roots_and_media_and_rejects_scan_tasks() {
        let database = migrated_database();
        let (root_id, _folder_id) = seed_media_page_fixture(&database);
        database.disable_root(root_id).expect("disable root");

        assert!(database.list_roots().expect("list roots").is_empty());
        let media = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert!(media.items.is_empty());

        let scan_task = database.create_root_scan_task(root_id);
        assert!(scan_task
            .expect_err("disabled root should reject scan task")
            .to_string()
            .contains("disabled"));
    }

    #[test]
    fn readding_disabled_root_reconciles_stale_rows_on_next_scan() {
        // Re-adding a disabled root keeps its cached rows immediately visible
        // (no O(files) inline wipe — that `DELETE FROM files WHERE root_id`, ×
        // the files table's many indexes, froze the single writer for minutes on
        // a 600k-file root). The next root scan reconciles stale rows the normal
        // way: rows it does not re-see are marked `missing` and stop showing.
        // This is the same mark-and-sweep a routine rescan uses, so a re-add is
        // no longer special-cased into a whole-root delete.
        let database = migrated_database();
        let (root_id, _folder_id) = seed_media_page_fixture(&database);
        database.disable_root(root_id).expect("disable root");

        let same_root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures Again".to_string(),
            })
            .expect("readd root");
        assert_eq!(same_root_id, root_id);

        let roots = database.list_roots().expect("list roots");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].display_name, "Pictures Again");

        // Cached rows are preserved and visible right after re-add (instant
        // content, and user_metadata/tags/thumbs survive the disable cycle).
        let before = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media before scan");
        assert!(
            !before.items.is_empty(),
            "re-add keeps cached rows visible instead of wiping them"
        );

        // The next scan reconciles: begin a fresh generation, re-see nothing,
        // then complete — every stale `active` row is marked `missing`.
        let scan_generation = database
            .begin_root_scan_reconciliation(root_id)
            .expect("begin scan reconciliation");
        database
            .reconcile_root_scan_completion(root_id, scan_generation)
            .expect("reconcile scan completion");

        let after = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media after scan");
        assert!(
            after.items.is_empty(),
            "reconcile marks rows the scan did not re-see as missing"
        );
    }

    #[test]
    fn commit_scan_batch_commits_folders_files_and_media_and_upserts_without_duplicates() {
        let mut database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");

        let first_result = database
            .commit_scan_batch(ScanWriteBatch {
                folders: vec![FolderUpsert {
                    root_id,
                    parent_id: None,
                    name: String::new(),
                    path_hash: "root-hash".to_string(),
                    mtime: Some(10),
                }],
                files: Vec::new(),
                scan_generation: None,
            })
            .expect("commit root folder");

        let root_folder_id = first_result.folder_ids[0];
        let photos_result = database
            .commit_scan_batch(ScanWriteBatch {
                folders: vec![FolderUpsert {
                    root_id,
                    parent_id: Some(root_folder_id),
                    name: "photos".to_string(),
                    path_hash: "photos-hash".to_string(),
                    mtime: Some(20),
                }],
                files: Vec::new(),
                scan_generation: None,
            })
            .expect("commit child folder");
        let photos_folder_id = photos_result.folder_ids[0];
        let second_result = database
            .commit_scan_batch(ScanWriteBatch {
                folders: Vec::new(),
                files: vec![
                    ScanFileUpsert {
                        file: FileUpsert {
                            root_id,
                            folder_id: root_folder_id,
                            name: "clip.mp4".to_string(),
                            ext: ".mp4".to_string(),
                            size: 100,
                            mtime: 30,
                            ctime: None,
                            file_key: None,
                        },
                        media_kind: "video".to_string(),
                        external_identity: None,
                    },
                    ScanFileUpsert {
                        file: FileUpsert {
                            root_id,
                            folder_id: photos_folder_id,
                            name: "image.jpg".to_string(),
                            ext: ".jpg".to_string(),
                            size: 200,
                            mtime: 40,
                            ctime: None,
                            file_key: None,
                        },
                        media_kind: "image".to_string(),
                        external_identity: None,
                    },
                ],
                scan_generation: None,
            })
            .expect("commit files");
        assert_eq!(second_result.file_ids.len(), 2);

        database
            .commit_scan_batch(ScanWriteBatch {
                folders: vec![FolderUpsert {
                    root_id,
                    parent_id: Some(root_folder_id),
                    name: "photos".to_string(),
                    path_hash: "photos-hash-updated".to_string(),
                    mtime: Some(50),
                }],
                files: vec![ScanFileUpsert {
                    file: FileUpsert {
                        root_id,
                        folder_id: photos_folder_id,
                        name: "image.jpg".to_string(),
                        ext: ".jpg".to_string(),
                        size: 300,
                        mtime: 60,
                        ctime: None,
                        file_key: None,
                    },
                    media_kind: "image".to_string(),
                    external_identity: None,
                }],
                scan_generation: None,
            })
            .expect("upsert same rows");

        let folder_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM folders", [], |row| row.get(0))
            .expect("count folders");
        let file_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .expect("count files");
        let media_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM media", [], |row| row.get(0))
            .expect("count media");
        let updated_size: i64 = database
            .connection
            .query_row(
                "SELECT size FROM files WHERE folder_id = ?1 AND name = 'image.jpg'",
                [photos_folder_id],
                |row| row.get(0),
            )
            .expect("updated file size");

        assert_eq!(folder_count, 2);
        assert_eq!(file_count, 2);
        assert_eq!(media_count, 2);
        assert_eq!(updated_size, 300);
    }

    #[test]
    fn incremental_upsert_during_active_scan_is_not_reconciled_missing() {
        let mut database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");

        let first_generation = database
            .begin_root_scan_reconciliation(root_id)
            .expect("begin first scan");
        let root_result = database
            .commit_scan_batch(ScanWriteBatch {
                folders: vec![FolderUpsert {
                    root_id,
                    parent_id: None,
                    name: String::new(),
                    path_hash: "root-hash".to_string(),
                    mtime: Some(10),
                }],
                files: Vec::new(),
                scan_generation: Some(first_generation),
            })
            .expect("commit first root folder");
        let root_folder_id = root_result.folder_ids[0];
        database
            .commit_scan_batch(ScanWriteBatch {
                folders: Vec::new(),
                files: vec![ScanFileUpsert {
                    file: FileUpsert {
                        root_id,
                        folder_id: root_folder_id,
                        name: "deleted.jpg".to_string(),
                        ext: ".jpg".to_string(),
                        size: 10,
                        mtime: 10,
                        ctime: None,
                        file_key: None,
                    },
                    media_kind: "image".to_string(),
                    external_identity: None,
                }],
                scan_generation: Some(first_generation),
            })
            .expect("commit old media");
        database
            .reconcile_root_scan_completion(root_id, first_generation)
            .expect("complete first scan");

        let second_generation = database
            .begin_root_scan_reconciliation(root_id)
            .expect("begin second scan");
        database
            .commit_scan_batch(ScanWriteBatch {
                folders: vec![FolderUpsert {
                    root_id,
                    parent_id: None,
                    name: String::new(),
                    path_hash: "root-hash".to_string(),
                    mtime: Some(20),
                }],
                files: Vec::new(),
                scan_generation: Some(second_generation),
            })
            .expect("commit second root folder");

        let concurrent_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id: root_folder_id,
                name: "concurrent.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 20,
                mtime: 20,
                ctime: None,
                file_key: None,
            })
            .expect("watcher-style upsert during active scan");
        database
            .upsert_media_kind(concurrent_file_id, "image")
            .expect("watcher-style media upsert");

        database
            .reconcile_root_scan_completion(root_id, second_generation)
            .expect("complete second scan");
        let page = database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert_eq!(media_names(&page.items), vec!["concurrent.jpg"]);
    }

    #[test]
    fn stale_task_attempt_cannot_reconcile_root_scan_completion() {
        // Regression for Blocker 2: when a guarded scan finishes after its
        // task attempt has been superseded (cancel + retry), reconciliation
        // must NOT run. It must not mark rows missing for the new generation
        // and must not clear the new attempt's `active_scan_generation`.
        let mut database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");

        // Seed an initial scan so we have rows to potentially clobber.
        let seed_generation = database
            .begin_root_scan_reconciliation(root_id)
            .expect("begin seed scan");
        let root_folder_id = database
            .commit_scan_batch(ScanWriteBatch {
                folders: vec![FolderUpsert {
                    root_id,
                    parent_id: None,
                    name: String::new(),
                    path_hash: "root-hash".to_string(),
                    mtime: Some(10),
                }],
                files: Vec::new(),
                scan_generation: Some(seed_generation),
            })
            .expect("commit root folder")
            .folder_ids[0];
        database
            .commit_scan_batch(ScanWriteBatch {
                folders: Vec::new(),
                files: vec![ScanFileUpsert {
                    file: FileUpsert {
                        root_id,
                        folder_id: root_folder_id,
                        name: "keep.jpg".to_string(),
                        ext: ".jpg".to_string(),
                        size: 10,
                        mtime: 10,
                        ctime: None,
                        file_key: None,
                    },
                    media_kind: "image".to_string(),
                    external_identity: None,
                }],
                scan_generation: Some(seed_generation),
            })
            .expect("commit seed file");
        database
            .reconcile_root_scan_completion(root_id, seed_generation)
            .expect("finish seed scan");

        // Begin a guarded scan tied to a running task.
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        let old_attempt = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;
        database
            .mark_task_running_for_attempt(task_id, old_attempt)
            .expect("mark old attempt running");
        let stale_generation = database
            .begin_root_scan_reconciliation(root_id)
            .expect("begin stale scan");

        // Supersede the task attempt mid-scan: cancel + retry bumps the
        // attempt generation, so the in-flight scan is now stale.
        database.cancel_task(task_id).expect("cancel task");
        database.retry_task(task_id).expect("retry task");

        // The guarded reconciliation must report not-current and must NOT
        // clear `active_scan_generation` or mark seed rows missing.
        let reconciled = database
            .reconcile_root_scan_completion_for_task_attempt(
                root_id,
                stale_generation,
                task_id,
                old_attempt,
            )
            .expect("guarded reconciliation should not error on stale attempt");
        assert!(
            !reconciled,
            "stale attempt must not reconcile after supersede"
        );

        // active_scan_generation must still be the stale generation; the
        // stale attempt did not clear it.
        let active_generation: Option<i64> = database
            .connection
            .query_row(
                "SELECT active_scan_generation FROM roots WHERE id = ?1",
                [root_id],
                |row| row.get(0),
            )
            .expect("read active scan generation");
        assert_eq!(active_generation, Some(stale_generation));

        // Seed file must remain active (not marked missing by stale
        // reconciliation against a generation that never wrote rows).
        let keep_status: String = database
            .connection
            .query_row(
                "SELECT status FROM files WHERE name = 'keep.jpg' AND root_id = ?1",
                [root_id],
                |row| row.get(0),
            )
            .expect("read keep.jpg status");
        assert_eq!(keep_status, "active");

        // The retried (current) attempt is still able to perform guarded
        // reconciliation under its own generation.
        let new_attempt = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;
        assert!(new_attempt > old_attempt);
        database
            .mark_task_running_for_attempt(task_id, new_attempt)
            .expect("mark new attempt running");
        let new_generation = database
            .begin_root_scan_reconciliation(root_id)
            .expect("begin new attempt reconciliation");
        assert!(new_generation >= stale_generation);
        let reconciled_now = database
            .reconcile_root_scan_completion_for_task_attempt(
                root_id,
                new_generation,
                task_id,
                new_attempt,
            )
            .expect("current attempt reconciliation");
        assert!(reconciled_now);
    }

    #[test]
    fn commit_scan_batch_rolls_back_when_file_folder_is_invalid() {
        let mut database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");

        let error = database
            .commit_scan_batch(ScanWriteBatch {
                folders: vec![FolderUpsert {
                    root_id,
                    parent_id: None,
                    name: String::new(),
                    path_hash: "root-hash".to_string(),
                    mtime: Some(10),
                }],
                files: vec![ScanFileUpsert {
                    file: FileUpsert {
                        root_id,
                        folder_id: 123456,
                        name: "orphan.jpg".to_string(),
                        ext: ".jpg".to_string(),
                        size: 100,
                        mtime: 20,
                        ctime: None,
                        file_key: None,
                    },
                    media_kind: "image".to_string(),
                    external_identity: None,
                }],
                scan_generation: None,
            })
            .expect_err("invalid folder id should rollback");

        assert!(error.to_string().contains("FOREIGN KEY"));
        let folder_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM folders", [], |row| row.get(0))
            .expect("count folders");
        let file_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .expect("count files");
        assert_eq!(folder_count, 0);
        assert_eq!(file_count, 0);
    }

    #[test]
    fn commit_scan_batch_refuses_pending_delete_root() {
        let mut database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/X".to_string(),
                display_name: "X".to_string(),
            })
            .expect("add root");
        database.mark_root_for_purge(root_id).expect("mark");
        let result = database.commit_scan_batch(ScanWriteBatch {
            folders: vec![FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "h".to_string(),
                mtime: Some(1),
            }],
            files: Vec::new(),
            scan_generation: None,
        });
        assert!(
            result.is_err(),
            "must refuse to write into a pending-delete root"
        );
    }

    #[test]
    fn commit_scan_batch_rejects_disabled_root_inside_write_transaction() {
        let mut database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        database.disable_root(root_id).expect("disable root");

        let error = database
            .commit_scan_batch(ScanWriteBatch {
                folders: vec![FolderUpsert {
                    root_id,
                    parent_id: None,
                    name: String::new(),
                    path_hash: "root-hash".to_string(),
                    mtime: Some(10),
                }],
                files: Vec::new(),
                scan_generation: None,
            })
            .expect_err("disabled root should reject scan batch");
        assert!(error.to_string().contains("disabled"));

        let folder_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE root_id = ?1",
                [root_id],
                |row| row.get(0),
            )
            .expect("count folders");
        assert_eq!(folder_count, 0);
    }

    #[test]
    fn task_lifecycle_records_pending_running_success_and_failure() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");

        let first_task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        let second_task_id = database
            .create_root_scan_task(root_id)
            .expect("create second root scan task");

        let tasks = database.list_tasks().expect("list tasks");
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, first_task_id);
        assert_eq!(tasks[0].kind, "root_scan");
        assert_eq!(tasks[0].priority, 0);
        assert_eq!(tasks[0].status, "pending");
        assert_eq!(tasks[0].root_id, Some(root_id));
        assert_eq!(tasks[0].file_id, None);
        assert_eq!(tasks[0].error, None);

        database
            .mark_task_running_current_attempt_for_test(first_task_id)
            .expect("mark task running");
        let task = database
            .get_task(first_task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "running");
        assert_eq!(task.error, None);

        database
            .mark_task_succeeded_current_attempt_for_test(first_task_id)
            .expect("mark task succeeded");
        let task = database
            .get_task(first_task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "succeeded");
        assert_eq!(task.error, None);

        database
            .mark_task_failed_current_attempt_for_test(second_task_id, "scan failed")
            .expect("mark task failed");
        let task = database
            .get_task(second_task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "failed");
        assert_eq!(task.error.as_deref(), Some("scan failed"));
    }

    #[test]
    fn root_scan_lifecycle_updates_one_notification_operation() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task_with_origin(root_id, NotificationOrigin::User)
            .expect("create root scan task");

        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark task running");
        database
            .update_task_scan_progress_current_attempt_for_test(
                task_id,
                TaskScanProgress {
                    items_seen: 50,
                    items_total: Some(100),
                    folders_seen: 4,
                    media_files_seen: 46,
                    skipped_files: 0,
                },
            )
            .expect("update task progress");
        database
            .mark_task_succeeded_current_attempt_for_test(task_id)
            .expect("mark task succeeded");

        let heads = database
            .notification_heads_for_test()
            .expect("read notification heads");
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].operation_id, format!("task:{task_id}"));
        assert_eq!(heads[0].status, "succeeded");
        assert_eq!(heads[0].origin, "user");
        assert_eq!(heads[0].current_value, 50);
        assert_eq!(heads[0].total_value, Some(100));
    }

    #[test]
    fn task_progress_clock_seam_emits_exactly_once_per_second() {
        const PROGRESS_UPDATES: i64 = 10_000;
        const EXPECTED_PROGRESS_EVENTS: usize = 10;
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/progress-clock-seam".to_string(),
                display_name: "Progress clock seam".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark task running");
        let attempt_generation = database
            .current_task_attempt_generation(task_id)
            .expect("read task attempt");
        let baseline = database
            .list_notification_outbox(0, 200)
            .expect("read baseline outbox")
            .last()
            .cloned()
            .expect("running phase event");

        for index in 0..PROGRESS_UPDATES {
            database
                .update_task_scan_progress_for_attempt_at_millis_for_test(
                    task_id,
                    attempt_generation,
                    TaskScanProgress {
                        items_seen: index + 1,
                        items_total: Some(PROGRESS_UPDATES),
                        folders_seen: index / 10,
                        media_files_seen: index + 1,
                        skipped_files: 0,
                    },
                    baseline.operation.updated_at + index + 1,
                )
                .expect("publish scheduled progress");
        }

        assert_eq!(database.flush_dirty_notification_heads().unwrap(), 0);
        let progress_events = database
            .list_notification_outbox(baseline.sequence, 200)
            .expect("read scheduled progress events");
        assert_eq!(progress_events.len(), EXPECTED_PROGRESS_EVENTS);
        assert_eq!(
            progress_events
                .iter()
                .map(|event| event.operation.updated_at - baseline.operation.updated_at)
                .collect::<Vec<_>>(),
            (1..=EXPECTED_PROGRESS_EVENTS)
                .map(|second| second as i64 * 1_000)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn task_terminal_in_same_millisecond_preserves_progress_timestamp() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/terminal-same-millisecond".to_string(),
                display_name: "Terminal same millisecond".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark task running");
        let attempt_generation = database
            .current_task_attempt_generation(task_id)
            .expect("read task attempt");
        let baseline = database
            .notification_heads_for_test()
            .expect("read running head")
            .pop()
            .expect("running head");
        let completed_at = baseline.updated_at + 1_000;

        database
            .update_task_scan_progress_for_attempt_at_millis_for_test(
                task_id,
                attempt_generation,
                TaskScanProgress {
                    items_seen: 1,
                    items_total: Some(1),
                    folders_seen: 0,
                    media_files_seen: 1,
                    skipped_files: 0,
                },
                completed_at,
            )
            .expect("publish progress");
        database
            .mark_task_succeeded_for_attempt_at_millis_for_test(
                task_id,
                attempt_generation,
                completed_at,
            )
            .expect("publish terminal");

        let terminal = database
            .notification_heads_for_test()
            .expect("read terminal head")
            .pop()
            .expect("terminal head");
        assert_eq!(terminal.status, "succeeded");
        assert_eq!(terminal.updated_at, completed_at);
        assert_eq!(terminal.completed_at, Some(completed_at));
    }

    #[test]
    fn task_terminal_after_ten_second_synthetic_progress_does_not_retreat() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/terminal-after-progress".to_string(),
                display_name: "Terminal after progress".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark task running");
        let attempt_generation = database
            .current_task_attempt_generation(task_id)
            .expect("read task attempt");
        let baseline_updated_at = database
            .notification_heads_for_test()
            .expect("read running head")
            .pop()
            .expect("running head")
            .updated_at;
        let final_progress_at = baseline_updated_at + 10_000;

        database
            .update_task_scan_progress_for_attempt_at_millis_for_test(
                task_id,
                attempt_generation,
                TaskScanProgress {
                    items_seen: 10_000,
                    items_total: Some(10_000),
                    folders_seen: 100,
                    media_files_seen: 10_000,
                    skipped_files: 0,
                },
                final_progress_at,
            )
            .expect("publish final synthetic progress");
        database
            .mark_task_succeeded_for_attempt_at_millis_for_test(
                task_id,
                attempt_generation,
                final_progress_at + 1,
            )
            .expect("publish terminal");

        let terminal = database
            .notification_heads_for_test()
            .expect("read terminal head")
            .pop()
            .expect("terminal head");
        assert_eq!(terminal.updated_at, final_progress_at + 1);
        assert_eq!(terminal.completed_at, Some(final_progress_at + 1));
    }

    #[test]
    fn task_terminal_clock_rollback_keeps_history_monotonic() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/terminal-clock-rollback".to_string(),
                display_name: "Terminal clock rollback".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark task running");
        let attempt_generation = database
            .current_task_attempt_generation(task_id)
            .expect("read task attempt");
        let baseline_sequence = database
            .list_notification_outbox(0, 200)
            .expect("read running event")
            .last()
            .cloned()
            .expect("running event");
        let final_progress_at = baseline_sequence.operation.updated_at + 10_000;

        database
            .update_task_scan_progress_for_attempt_at_millis_for_test(
                task_id,
                attempt_generation,
                TaskScanProgress {
                    items_seen: 10_000,
                    items_total: Some(10_000),
                    folders_seen: 100,
                    media_files_seen: 10_000,
                    skipped_files: 0,
                },
                final_progress_at,
            )
            .expect("publish final progress");
        database
            .mark_task_succeeded_for_attempt_at_millis_for_test(
                task_id,
                attempt_generation,
                baseline_sequence.operation.updated_at + 2_000,
            )
            .expect("publish terminal after clock rollback");

        let events = database
            .list_notification_outbox(baseline_sequence.sequence, 200)
            .expect("read progress and terminal history");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].operation.updated_at, final_progress_at);
        assert_eq!(events[1].operation.updated_at, final_progress_at);
        assert_eq!(events[1].operation.completed_at, Some(final_progress_at));
        assert!(events
            .windows(2)
            .all(|pair| pair[0].operation.updated_at <= pair[1].operation.updated_at));

        let terminal = database
            .notification_heads_for_test()
            .expect("read terminal head")
            .pop()
            .expect("terminal head");
        assert_eq!(terminal.updated_at, final_progress_at);
        assert_eq!(terminal.completed_at, Some(final_progress_at));
    }

    struct NotificationBenchmarkTempDir {
        path: PathBuf,
    }

    impl NotificationBenchmarkTempDir {
        fn new() -> Self {
            let suffix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "megle-notification-benchmark-{}-{suffix}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("create notification benchmark temp dir");
            Self { path }
        }

        fn database_path(&self, label: &str) -> PathBuf {
            self.path.join(format!("{label}.sqlite"))
        }
    }

    impl Drop for NotificationBenchmarkTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    #[ignore = "invoked by tools/bench/notification-center-bench.mjs"]
    fn notification_center_benchmark_probe() {
        const RAPID_PROGRESS_UPDATES: i64 = 10_000;
        const EXPECTED_PROGRESS_EVENTS: i64 = 10;
        const ACTIVE_OPERATION_COUNT: usize = 20;
        const LIBRARY_COUNT: usize = 4;
        const THUMBNAIL_COMPLETIONS_PER_LIBRARY: i64 = 2_500;
        const THUMBNAIL_BLOB_BYTES: i64 = 4;
        const PROBE_PREFIX: &str = "MEGLE_NOTIFICATION_BENCH_JSON:";
        let temp_dir = NotificationBenchmarkTempDir::new();
        let database =
            Database::open(&temp_dir.database_path("progress")).expect("open benchmark database");
        database
            .apply_migrations()
            .expect("apply benchmark migrations");
        let root_id = database
            .add_root(NewRoot {
                path: "D:/megle-notification-benchmark".to_string(),
                display_name: "Notification benchmark".to_string(),
            })
            .expect("add benchmark root");
        let rapid_task_id = database
            .create_root_scan_task(root_id)
            .expect("create rapid benchmark task");
        database
            .mark_task_running_current_attempt_for_test(rapid_task_id)
            .expect("mark rapid benchmark task running");

        let operation_id = format!("task:{rapid_task_id}");
        let baseline_sequence: i64 = database
            .connection
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM notification_outbox",
                [],
                |row| row.get(0),
            )
            .expect("read benchmark baseline sequence");
        let baseline_revision: i64 = database
            .connection
            .query_row(
                "SELECT revision FROM notification_operation_heads WHERE operation_id = ?1",
                [&operation_id],
                |row| row.get(0),
            )
            .expect("read benchmark baseline revision");
        let baseline_updated_at = database
            .list_notification_outbox(0, 200)
            .expect("read benchmark baseline event")
            .last()
            .expect("running phase event")
            .operation
            .updated_at;
        let attempt_generation = database
            .current_task_attempt_generation(rapid_task_id)
            .expect("read rapid task attempt");

        let input_started_at = std::time::Instant::now();
        for index in 0..RAPID_PROGRESS_UPDATES {
            database
                .update_task_scan_progress_for_attempt_at_millis_for_test(
                    rapid_task_id,
                    attempt_generation,
                    TaskScanProgress {
                        items_seen: index + 1,
                        items_total: Some(RAPID_PROGRESS_UPDATES),
                        folders_seen: index / 10,
                        media_files_seen: index + 1,
                        skipped_files: 0,
                    },
                    baseline_updated_at + index + 1,
                )
                .expect("publish rapid production progress update");
        }
        let input_elapsed_ms = input_started_at.elapsed().as_secs_f64() * 1_000.0;
        let active_operation_seconds = RAPID_PROGRESS_UPDATES / 1_000;
        let rapid_head_revision: i64 = database
            .connection
            .query_row(
                "SELECT revision FROM notification_operation_heads WHERE operation_id = ?1",
                [&operation_id],
                |row| row.get(0),
            )
            .expect("read rapid head revision");
        assert_eq!(
            rapid_head_revision - baseline_revision,
            RAPID_PROGRESS_UPDATES,
            "every production progress input must advance the logical head revision"
        );

        assert_eq!(
            database
                .flush_dirty_notification_heads()
                .expect("flush rapid dirty production head"),
            0,
            "the final scheduled update lands exactly on a one-second boundary"
        );
        let progress_last_sequence: i64 = database
            .connection
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM notification_outbox",
                [],
                |row| row.get(0),
            )
            .expect("read progress sequence boundary");
        let persisted_progress_events: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM notification_outbox WHERE sequence > ?1 AND sequence <= ?2",
                (baseline_sequence, progress_last_sequence),
                |row| row.get(0),
            )
            .expect("count persisted production progress events");
        let sampled_progress_offsets_ms = database
            .list_notification_outbox(baseline_sequence, 200)
            .expect("read persisted production progress events")
            .into_iter()
            .map(|event| event.operation.updated_at - baseline_updated_at)
            .collect::<Vec<_>>();
        assert_eq!(persisted_progress_events, EXPECTED_PROGRESS_EVENTS);
        assert_eq!(
            sampled_progress_offsets_ms,
            (1..=EXPECTED_PROGRESS_EVENTS)
                .map(|second| second * 1_000)
                .collect::<Vec<_>>()
        );

        let terminal_updated_at = baseline_updated_at + RAPID_PROGRESS_UPDATES + 1;
        database
            .mark_task_succeeded_for_attempt_at_millis_for_test(
                rapid_task_id,
                attempt_generation,
                terminal_updated_at,
            )
            .expect("publish rapid production terminal event");
        let terminal_last_sequence: i64 = database
            .connection
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM notification_outbox",
                [],
                |row| row.get(0),
            )
            .expect("read terminal sequence boundary");
        let terminal_event_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM notification_outbox WHERE sequence > ?1 AND sequence <= ?2",
                (progress_last_sequence, terminal_last_sequence),
                |row| row.get(0),
            )
            .expect("count persisted production terminal events");
        assert_eq!(terminal_event_count, 1);
        assert_eq!(persisted_progress_events, active_operation_seconds);
        let rapid_history = database
            .list_notification_outbox(baseline_sequence, 200)
            .expect("read rapid progress and terminal history");
        assert_eq!(
            rapid_history
                .last()
                .expect("rapid terminal event")
                .operation
                .updated_at,
            terminal_updated_at
        );
        assert_eq!(
            rapid_history
                .last()
                .expect("rapid terminal event")
                .operation
                .completed_at,
            Some(terminal_updated_at)
        );
        assert!(rapid_history
            .windows(2)
            .all(|pair| pair[0].operation.updated_at <= pair[1].operation.updated_at));

        let mut active_task_ids = Vec::with_capacity(ACTIVE_OPERATION_COUNT);
        for _ in 0..ACTIVE_OPERATION_COUNT {
            let task_id = database
                .create_root_scan_task(root_id)
                .expect("create active benchmark task");
            database
                .mark_task_running_current_attempt_for_test(task_id)
                .expect("mark active benchmark task running");
            active_task_ids.push(task_id);
        }

        let outbox_events = database
            .list_notification_outbox(0, 200)
            .expect("read production outbox events");
        let outbox_row_count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM notification_outbox", [], |row| {
                row.get(0)
            })
            .expect("count production outbox rows");
        let latest_sequence: i64 = database
            .connection
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0) FROM notification_outbox",
                [],
                |row| row.get(0),
            )
            .expect("read latest production outbox sequence");
        assert_eq!(outbox_row_count, outbox_events.len() as i64);
        assert_eq!(outbox_row_count, 53);

        let mut thumbnail_libraries = Vec::with_capacity(LIBRARY_COUNT);
        let thumbnail_started_at = std::time::Instant::now();
        for library_index in 0..LIBRARY_COUNT {
            let thumbnail_database = Database::open(
                &temp_dir.database_path(&format!("thumbnail-library-{}", library_index + 1)),
            )
            .expect("open thumbnail benchmark database");
            thumbnail_database
                .apply_migrations()
                .expect("apply thumbnail benchmark migrations");
            let root_id = thumbnail_database
                .add_root(NewRoot {
                    path: format!("D:/thumbnail-benchmark-library-{}", library_index + 1),
                    display_name: format!("Thumbnail benchmark library {}", library_index + 1),
                })
                .expect("add thumbnail benchmark root");
            let folder_id = thumbnail_database
                .upsert_folder(FolderUpsert {
                    root_id,
                    parent_id: None,
                    name: String::new(),
                    path_hash: format!("thumbnail-root-{library_index}"),
                    mtime: Some(1),
                })
                .expect("add thumbnail benchmark folder");

            let library_started_at = std::time::Instant::now();
            for completion_index in 0..THUMBNAIL_COMPLETIONS_PER_LIBRARY {
                let file_id = thumbnail_database
                    .upsert_file(FileUpsert {
                        root_id,
                        folder_id,
                        name: format!("thumbnail-{completion_index:05}.jpg"),
                        ext: ".jpg".to_string(),
                        size: 4_096 + completion_index,
                        mtime: 1_000 + completion_index,
                        ctime: None,
                        file_key: Some(format!("library-{library_index}-file-{completion_index}")),
                    })
                    .expect("insert thumbnail benchmark file");
                thumbnail_database
                    .upsert_media_kind(file_id, "image")
                    .expect("insert thumbnail benchmark media");
                let request = thumbnail_database
                    .request_thumbnail_task(
                        file_id,
                        crate::thumbnails::GRID_320_PROFILE,
                        ThumbnailTaskPriority::Background,
                    )
                    .expect("request production thumbnail task");
                let task_id = request.task_id.expect("thumbnail task id");
                let attempt_generation = thumbnail_database
                    .current_task_attempt_generation(task_id)
                    .expect("read thumbnail task attempt");
                let source_fingerprint = thumbnail_database
                    .get_thumbnail_source(file_id)
                    .expect("read thumbnail source")
                    .expect("thumbnail source exists")
                    .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
                thumbnail_database
                    .mark_thumbnail_task_running_for_attempt(
                        task_id,
                        attempt_generation,
                        &source_fingerprint,
                    )
                    .expect("mark production thumbnail task running");
                let published = thumbnail_database
                    .publish_thumbnail_and_succeed_task_if_source_fingerprint_and_task_attempt_current(
                        ThumbnailStateUpsert {
                            file_id,
                            profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                            state: "ready".to_string(),
                            cache_key: None,
                            width: Some(320),
                            height: Some(320),
                            byte_size: Some(THUMBNAIL_BLOB_BYTES),
                            error: None,
                            source_fingerprint: Some(source_fingerprint.clone()),
                        },
                        ThumbBlobRecord {
                            file_id,
                            profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                            data: b"RIFF".to_vec(),
                            width: 320,
                            height: 320,
                            byte_size: THUMBNAIL_BLOB_BYTES,
                            output_format: crate::thumbnails::GENERATED_FORMAT.to_string(),
                            created_at: 1,
                            updated_at: 1,
                        },
                        Some((320, 320)),
                        &source_fingerprint,
                        task_id,
                        attempt_generation,
                    )
                    .expect("publish production thumbnail completion");
                assert!(published);
            }

            let stats = thumbnail_database
                .get_thumbnail_cache_stats(ThumbnailCacheScope {
                    root_id: Some(root_id),
                    folder_id: None,
                    include_descendants: true,
                })
                .expect("read production thumbnail cache stats");
            let thumbnail_outbox_rows: i64 = thumbnail_database
                .connection
                .query_row("SELECT COUNT(*) FROM notification_outbox", [], |row| {
                    row.get(0)
                })
                .expect("count thumbnail outbox rows");
            let completed_task_rows: i64 = thumbnail_database
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM tasks WHERE kind = 'thumbnail' AND status = 'succeeded'",
                    [],
                    |row| row.get(0),
                )
                .expect("count completed thumbnail tasks");
            let ready_thumbnail_rows: i64 = thumbnail_database
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM thumbs WHERE state = 'ready'",
                    [],
                    |row| row.get(0),
                )
                .expect("count ready thumbnail rows");
            let blob_rows: i64 = thumbnail_database
                .connection
                .query_row("SELECT COUNT(*) FROM thumb_blobs", [], |row| row.get(0))
                .expect("count thumbnail blob rows");
            assert_eq!(thumbnail_outbox_rows, 0);
            assert_eq!(completed_task_rows, THUMBNAIL_COMPLETIONS_PER_LIBRARY);
            assert_eq!(ready_thumbnail_rows, THUMBNAIL_COMPLETIONS_PER_LIBRARY);
            assert_eq!(blob_rows, THUMBNAIL_COMPLETIONS_PER_LIBRARY);
            assert_eq!(stats.cached_count, THUMBNAIL_COMPLETIONS_PER_LIBRARY);
            assert_eq!(stats.missing_count, 0);
            assert_eq!(stats.stale_count, 0);
            assert_eq!(stats.failed_count, 0);
            assert_eq!(stats.pending_candidate_count, 0);
            assert_eq!(stats.active_bulk_task_count, 0);
            assert_eq!(
                stats.total_blob_bytes,
                THUMBNAIL_COMPLETIONS_PER_LIBRARY * THUMBNAIL_BLOB_BYTES
            );

            thumbnail_libraries.push(serde_json::json!({
                "libraryId": format!("bench-library-{}", library_index + 1),
                "libraryName": format!("Benchmark Library {}", library_index + 1),
                "completionCount": THUMBNAIL_COMPLETIONS_PER_LIBRARY,
                "completedTaskRows": completed_task_rows,
                "readyThumbnailRows": ready_thumbnail_rows,
                "blobRows": blob_rows,
                "perItemOutboxRows": thumbnail_outbox_rows,
                "completionElapsedMs": library_started_at.elapsed().as_secs_f64() * 1_000.0,
                "stats": {
                    "cachedCount": stats.cached_count,
                    "missingCount": stats.missing_count,
                    "staleCount": stats.stale_count,
                    "failedCount": stats.failed_count,
                    "pendingCandidateCount": stats.pending_candidate_count,
                    "totalBlobBytes": stats.total_blob_bytes,
                    "activeBulkTaskCount": stats.active_bulk_task_count,
                    "failureDetails": [],
                    "failureDetailsTruncated": false
                }
            }));
        }
        assert_eq!(thumbnail_libraries.len(), LIBRARY_COUNT);

        let probe = serde_json::json!({
            "events": outbox_events,
            "latestSequence": latest_sequence,
            "activeOperationIds": active_task_ids
                .iter()
                .map(|task_id| format!("task:{task_id}"))
                .collect::<Vec<_>>(),
            "rapidOperationId": operation_id,
            "metrics": {
                "rapidProgressUpdates": RAPID_PROGRESS_UPDATES,
                "baselineSequence": baseline_sequence,
                "baselineRevision": baseline_revision,
                "rapidHeadRevision": rapid_head_revision,
                "logicalProgressRevisionCount": rapid_head_revision - baseline_revision,
                "progressLastSequence": progress_last_sequence,
                "terminalLastSequence": terminal_last_sequence,
                "persistedProgressEvents": persisted_progress_events,
                "expectedProgressEvents": EXPECTED_PROGRESS_EVENTS,
                "sampledProgressOffsetsMs": sampled_progress_offsets_ms,
                "terminalEventCount": terminal_event_count,
                "terminalUpdatedOffsetMs": terminal_updated_at - baseline_updated_at,
                "rapidHistoryMonotonic": true,
                "activeOperationSeconds": active_operation_seconds,
                "inputElapsedMs": input_elapsed_ms,
                "outboxRowCount": outbox_row_count,
                "thumbnailCompletionCount": THUMBNAIL_COMPLETIONS_PER_LIBRARY * LIBRARY_COUNT as i64,
                "thumbnailElapsedMs": thumbnail_started_at.elapsed().as_secs_f64() * 1_000.0
            },
            "thumbnailLibraries": thumbnail_libraries
        });
        println!(
            "{PROBE_PREFIX}{}",
            serde_json::to_string(&probe).expect("serialize benchmark probe")
        );
    }

    #[test]
    #[ignore = "invoked by tools/bench/notification-center-bench.mjs"]
    fn notification_center_benchmark_fault_probe() {
        const PROBE_PREFIX: &str = "MEGLE_NOTIFICATION_BENCH_FAULT_JSON:";
        let temp_dir = NotificationBenchmarkTempDir::new();
        let database_path = temp_dir.database_path("fault");
        let database = Database::open(&database_path).expect("open fault-probe database");
        database
            .apply_migrations()
            .expect("apply fault-probe migrations");
        database
            .add_root(NewRoot {
                path: "D:/megle-notification-fault-probe".to_string(),
                display_name: "Notification fault probe".to_string(),
            })
            .expect("write fault-probe database");
        let (schema_migration_count, latest_schema_version): (i64, i64) = database
            .connection
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read initialized schema");
        let sidecars_observed = [
            database_path.clone(),
            PathBuf::from(format!("{}-wal", database_path.display())),
            PathBuf::from(format!("{}-shm", database_path.display())),
        ]
        .into_iter()
        .filter(|candidate| candidate.exists())
        .map(|candidate| {
            candidate
                .file_name()
                .expect("fault-probe sidecar basename")
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
        assert_eq!(
            sidecars_observed,
            vec!["fault.sqlite", "fault.sqlite-wal", "fault.sqlite-shm"]
        );
        let payload = serde_json::json!({
            "processId": std::process::id(),
            "tempDirectory": temp_dir.path,
            "databaseInitialized": true,
            "schemaMigrationCount": schema_migration_count,
            "latestSchemaVersion": latest_schema_version,
            "sidecarsObserved": sidecars_observed
        });
        println!(
            "{PROBE_PREFIX}{}",
            serde_json::to_string(&payload).expect("serialize fault-probe evidence")
        );
        use std::io::Write as _;
        std::io::stdout()
            .flush()
            .expect("flush fault-probe evidence");
        panic!("intentional notification benchmark Core failure after database initialization");
    }

    #[test]
    fn task_terminal_notifications_cover_failure_and_cancellation() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let failed_task = database
            .create_root_scan_task(root_id)
            .expect("create failed task");
        let cancelled_task = database
            .create_root_scan_task(root_id)
            .expect("create cancelled task");

        database
            .mark_task_failed_current_attempt_for_test(failed_task, "scan failed")
            .expect("mark task failed");
        database.cancel_task(cancelled_task).expect("cancel task");

        let heads = database
            .notification_heads_for_test()
            .expect("read notification heads");
        let failed = heads
            .iter()
            .find(|operation| operation.task_id == Some(failed_task))
            .expect("failed task head");
        let cancelled = heads
            .iter()
            .find(|operation| operation.task_id == Some(cancelled_task))
            .expect("cancelled task head");
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.error_code.as_deref(), Some("failed"));
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(cancelled.error_code.as_deref(), Some("cancelled"));
    }

    #[test]
    fn scan_notification_summaries_use_library_relative_paths() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/private/Artists".to_string(),
                display_name: "Artists".to_string(),
            })
            .expect("add root");
        let root_folder = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "summary-root".to_string(),
                mtime: Some(1),
            })
            .expect("root folder");
        let portraits = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder),
                name: "Portraits".to_string(),
                path_hash: "summary-portraits".to_string(),
                mtime: Some(2),
            })
            .expect("portraits folder");
        let concept = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(portraits),
                name: "Concept".to_string(),
                path_hash: "summary-concept".to_string(),
                mtime: Some(3),
            })
            .expect("concept folder");
        let root_task = database.create_root_scan_task(root_id).expect("root scan");
        let folder_task = database
            .create_interactive_folder_scan_task(concept)
            .expect("folder scan");
        let heads = database
            .notification_heads_for_test()
            .expect("notification heads");
        assert_eq!(
            heads
                .iter()
                .find(|item| item.task_id == Some(root_task))
                .and_then(|item| item.summary.as_deref()),
            Some("Artists")
        );
        assert_eq!(
            heads
                .iter()
                .find(|item| item.task_id == Some(folder_task))
                .and_then(|item| item.summary.as_deref()),
            Some("Artists\\Portraits\\Concept")
        );
        assert!(heads.iter().all(|item| !item
            .summary
            .as_deref()
            .unwrap_or_default()
            .contains("D:/")));
    }

    #[test]
    fn media_position_matches_list_sort_and_scope() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let mut file_ids = Vec::new();
        for (name, mtime) in [("c.jpg", 30), ("a.jpg", 10), ("b.jpg", 20)] {
            file_ids.push(
                database
                    .upsert_file(FileUpsert {
                        root_id,
                        folder_id,
                        name: name.to_string(),
                        ext: ".jpg".to_string(),
                        size: 10,
                        mtime,
                        ctime: None,
                        file_key: Some(format!("position-{name}")),
                    })
                    .expect("insert media"),
            );
        }
        let query = MediaPageQuery {
            root_id: Some(root_id),
            folder_id: Some(folder_id),
            include_descendants: false,
            limit: 10,
            offset: Some(0),
            cursor: None,
            sort: "name_asc".to_string(),
            kind: None,
        };
        let page = database
            .list_media_page(query.clone())
            .expect("list sorted media");
        for file_id in [file_ids[2], file_ids[0]] {
            let expected = page
                .items
                .iter()
                .position(|item| item.id == file_id)
                .map(|index| index as i64);
            assert_eq!(database.media_position(file_id, &query).unwrap(), expected);
        }
    }

    #[test]
    fn thumbnail_failure_details_are_bounded_and_library_relative() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_thumbnail_cache_scope_fixture(&database, "failure-details");
        let file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "broken.jpg",
            Some("broken"),
            "failed",
            false,
            None,
            None,
        );
        database
            .connection
            .execute(
                "UPDATE thumbs SET failure_kind = 'permanent' WHERE file_id = ?1 AND profile = ?2",
                (file_id, crate::thumbnails::GRID_320_PROFILE),
            )
            .expect("classify failure");
        let failures = database
            .list_thumbnail_failures(crate::thumbnails::GRID_320_PROFILE, 1)
            .expect("list failures");
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].file_id, file_id);
        assert_eq!(failures[0].failure_kind, "permanent");
        assert_eq!(
            failures[0].relative_path,
            "Pictures failure-details\\broken.jpg"
        );
        assert!(
            failures[0].updated_at >= 1_000_000_000_000,
            "failure diagnostics expose Unix milliseconds for renderer dates"
        );
        assert!(!failures[0].relative_path.contains("D:/"));
    }

    #[test]
    fn thumbnail_failure_by_file_walks_only_the_target_ancestor_chain() {
        let database = migrated_database();
        let (root_id, root_folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "target-failure-path");
        let parent_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "Artists".to_string(),
                path_hash: "target-failure-artists".to_string(),
                mtime: Some(11),
            })
            .expect("insert parent folder");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(parent_folder_id),
                name: "Portraits".to_string(),
                path_hash: "target-failure-portraits".to_string(),
                mtime: Some(12),
            })
            .expect("insert target folder");
        let file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "broken.jpg",
            Some("target-failure"),
            "failed",
            false,
            None,
            None,
        );

        let failure = database
            .get_thumbnail_failure(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("read target failure")
            .expect("target failure exists");
        assert_eq!(
            failure.relative_path,
            "Pictures target-failure-path\\Artists\\Portraits\\broken.jpg"
        );

        let explain_sql = format!("EXPLAIN QUERY PLAN {}", thumbnail_failure_by_file_sql());
        let mut statement = database
            .connection
            .prepare(&explain_sql)
            .expect("prepare target failure query plan");
        let plan = statement
            .query_map((crate::thumbnails::GRID_320_PROFILE, file_id), |row| {
                row.get::<_, String>(3)
            })
            .expect("explain target failure query")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect target failure plan")
            .join("\n");
        assert!(
            plan.contains("SEARCH files USING INTEGER PRIMARY KEY"),
            "target lookup must anchor from the requested file:\n{plan}"
        );
        assert!(
            !plan.contains("SCAN folders") && !plan.contains("MATERIALIZE folder_paths"),
            "target lookup must not materialize the full folder tree:\n{plan}"
        );
    }

    #[test]
    fn thumbnail_failure_snapshot_does_not_mix_count_and_details_across_a_writer_race() {
        let db_dir = unique_db_temp_dir("thumbnail-failure-snapshot-race");
        std::fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let reader = Database::open(&db_path).expect("open reader database");
        reader.apply_migrations().expect("migrate reader database");
        let (root_id, folder_id) = seed_thumbnail_cache_scope_fixture(&reader, "snapshot-race");
        let file_id = seed_thumbnail_cache_candidate(
            &reader,
            root_id,
            folder_id,
            "race.jpg",
            Some("race"),
            "failed",
            false,
            None,
            None,
        );
        let writer = Database::open(&db_path).expect("open writer database");
        writer.apply_migrations().expect("migrate writer database");

        let snapshot = reader
            .get_thumbnail_cache_snapshot_with_hook_for_test(
                ThumbnailCacheScope {
                    root_id: None,
                    folder_id: None,
                    include_descendants: false,
                },
                crate::thumbnails::GRID_320_PROFILE,
                100,
                || {
                    writer
                        .connection
                        .execute(
                            "UPDATE thumbs SET state = 'queued', error = NULL, failure_kind = NULL WHERE file_id = ?1 AND profile = ?2",
                            (file_id, crate::thumbnails::GRID_320_PROFILE),
                        )
                        .expect("commit concurrent thumbnail recovery");
                },
            )
            .expect("read atomic failure snapshot");

        assert_eq!(snapshot.stats.failed_count, 1);
        assert_eq!(snapshot.failure_details.len(), 1);
        assert_eq!(snapshot.failure_details[0].file_id, file_id);
        let fresh = reader
            .get_thumbnail_cache_snapshot(
                ThumbnailCacheScope {
                    root_id: None,
                    folder_id: None,
                    include_descendants: false,
                },
                crate::thumbnails::GRID_320_PROFILE,
                100,
            )
            .expect("read recovered snapshot");
        assert_eq!(fresh.stats.failed_count, 0);
        assert!(fresh.failure_details.is_empty());

        drop(writer);
        drop(reader);
        let _ = std::fs::remove_dir_all(db_dir);
    }

    #[test]
    fn thumbnail_failure_snapshot_does_not_mix_new_details_into_a_zero_count() {
        let db_dir = unique_db_temp_dir("thumbnail-failure-snapshot-new-failure-race");
        std::fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("megle.sqlite");
        let reader = Database::open(&db_path).expect("open reader database");
        reader.apply_migrations().expect("migrate reader database");
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&reader, "snapshot-new-failure-race");
        let file_id = seed_thumbnail_cache_candidate(
            &reader,
            root_id,
            folder_id,
            "race.jpg",
            Some("race"),
            "queued",
            false,
            None,
            None,
        );
        let writer = Database::open(&db_path).expect("open writer database");
        writer.apply_migrations().expect("migrate writer database");

        let snapshot = reader
            .get_thumbnail_cache_snapshot_with_hook_for_test(
                ThumbnailCacheScope {
                    root_id: None,
                    folder_id: None,
                    include_descendants: false,
                },
                crate::thumbnails::GRID_320_PROFILE,
                100,
                || {
                    writer
                        .connection
                        .execute(
                            "UPDATE thumbs SET state = 'failed', error = 'decode failed', failure_kind = 'permanent' WHERE file_id = ?1 AND profile = ?2",
                            (file_id, crate::thumbnails::GRID_320_PROFILE),
                        )
                        .expect("commit concurrent thumbnail failure");
                },
            )
            .expect("read atomic zero-failure snapshot");

        assert_eq!(snapshot.stats.failed_count, 0);
        assert!(snapshot.failure_details.is_empty());
        let fresh = reader
            .get_thumbnail_cache_snapshot(
                ThumbnailCacheScope {
                    root_id: None,
                    folder_id: None,
                    include_descendants: false,
                },
                crate::thumbnails::GRID_320_PROFILE,
                100,
            )
            .expect("read new failure snapshot");
        assert_eq!(fresh.stats.failed_count, 1);
        assert_eq!(fresh.failure_details.len(), 1);
        assert_eq!(fresh.failure_details[0].file_id, file_id);

        drop(writer);
        drop(reader);
        let _ = std::fs::remove_dir_all(db_dir);
    }

    #[test]
    fn thumbnail_tasks_never_create_notification_heads() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "thumbnail-only.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("thumbnail-only".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        let task_id = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Background,
            )
            .expect("request thumbnail")
            .task_id
            .expect("thumbnail task id");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark thumbnail running");
        database
            .mark_task_failed_current_attempt_for_test(task_id, "fixture failure")
            .expect("mark thumbnail failed");

        assert!(database
            .notification_heads_for_test()
            .expect("read notification heads")
            .is_empty());
    }

    #[test]
    fn task_notification_descriptors_exclude_only_thumbnail_work() {
        for kind in ["root_scan", "interactive_folder_scan", "root_purge"] {
            assert!(task_notification_descriptor(kind)
                .expect("known task kind")
                .is_some());
        }
        assert!(task_notification_descriptor("thumbnail")
            .expect("thumbnail exclusion")
            .is_none());
        assert!(task_notification_descriptor("future_non_thumbnail_kind").is_err());
    }

    #[test]
    fn root_scan_retryability_requires_active_non_pending_root() {
        for (label, pending_delete) in [("disabled", false), ("pending-delete", true)] {
            let database = migrated_database();
            let root_id = database
                .add_root(NewRoot {
                    path: format!("D:/retry-root-{label}"),
                    display_name: format!("retry-root-{label}"),
                })
                .expect("add root");
            let task_id = database
                .create_root_scan_task_with_origin(root_id, NotificationOrigin::User)
                .expect("create root scan task");
            database
                .mark_task_running_current_attempt_for_test(task_id)
                .expect("mark root scan running");
            if pending_delete {
                database
                    .connection
                    .execute(
                        "UPDATE roots SET enabled = 0, pending_delete_at = 1 WHERE id = ?1",
                        [root_id],
                    )
                    .expect("mark root pending delete");
            } else {
                database
                    .connection
                    .execute("UPDATE roots SET enabled = 0 WHERE id = ?1", [root_id])
                    .expect("disable root");
            }
            database
                .mark_task_failed_current_attempt_for_test(task_id, "root unavailable")
                .expect("fail root scan");

            let operation = database
                .notification_heads_for_test()
                .unwrap()
                .into_iter()
                .find(|operation| operation.task_id == Some(task_id))
                .expect("root scan operation");
            assert!(!operation.retryable, "invalid {label} root must not retry");
            assert!(database.retry_task(task_id).is_err());
        }
    }

    #[test]
    fn interactive_scan_retryability_requires_active_folder_and_root() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let task_id = database
            .create_interactive_folder_scan_task_with_origin(folder_id, NotificationOrigin::User)
            .expect("create interactive scan task");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark interactive scan running");
        database
            .connection
            .execute(
                "UPDATE folders SET status = 'missing' WHERE id = ?1",
                [folder_id],
            )
            .expect("invalidate folder");
        database
            .mark_task_failed_current_attempt_for_test(task_id, "folder unavailable")
            .expect("fail interactive scan");

        let operation = database
            .notification_heads_for_test()
            .unwrap()
            .into_iter()
            .find(|operation| operation.task_id == Some(task_id))
            .expect("interactive scan operation");
        assert!(!operation.retryable);
        assert!(database.retry_task(task_id).is_err());
        assert_eq!(
            database
                .get_task(task_id)
                .expect("read task")
                .expect("task remains")
                .root_id,
            Some(root_id)
        );
    }

    #[test]
    fn retry_task_rolls_back_transition_when_phase_notification_fails() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/retry-transaction".to_string(),
                display_name: "retry-transaction".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task_with_origin(root_id, NotificationOrigin::User)
            .expect("create root scan task");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark running");
        database
            .mark_task_failed_current_attempt_for_test(task_id, "retryable failure")
            .expect("mark failed");
        let outbox_before_retry = database
            .list_notification_outbox(0, 100)
            .expect("read initial outbox")
            .len();
        database
            .execute_batch_for_test(
                "CREATE TRIGGER fail_retry_phase_notification
                 BEFORE INSERT ON notification_outbox
                 WHEN json_extract(NEW.payload_json, '$.status') = 'pending'
                 BEGIN SELECT RAISE(ABORT, 'injected retry phase failure'); END;",
            )
            .expect("install retry phase failure trigger");

        assert!(database.retry_task(task_id).is_err());
        let task = database
            .get_task(task_id)
            .expect("read rolled-back task")
            .expect("failed task remains");
        assert_eq!(task.status, "failed");
        let operation = database
            .notification_heads_for_test()
            .unwrap()
            .into_iter()
            .find(|operation| operation.task_id == Some(task_id))
            .expect("failed operation remains");
        assert_eq!(operation.status, "failed");
        assert_eq!(
            database
                .list_notification_outbox(0, 100)
                .expect("read outbox")
                .len(),
            outbox_before_retry
        );

        database
            .execute_batch_for_test("DROP TRIGGER fail_retry_phase_notification;")
            .expect("remove retry phase failure trigger");
        let retried = database
            .retry_task(task_id)
            .expect("retry after failure removal");
        assert_eq!(retried.id, task_id);
        assert_eq!(retried.status, "pending");
        assert_eq!(
            database
                .notification_heads_for_test()
                .unwrap()
                .into_iter()
                .find(|operation| operation.task_id == Some(task_id))
                .expect("pending operation")
                .status,
            "pending"
        );
    }

    #[test]
    fn background_success_is_history_only_but_failure_and_user_success_are_important() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let background_success = database
            .create_root_scan_task(root_id)
            .expect("create background success task");
        let background_failure = database
            .create_root_scan_task(root_id)
            .expect("create background failure task");
        let background_cancel = database
            .create_root_scan_task(root_id)
            .expect("create background cancelled task");
        let user_success = database
            .create_root_scan_task_with_origin(root_id, NotificationOrigin::User)
            .expect("create user success task");

        database
            .mark_task_running_current_attempt_for_test(background_success)
            .expect("run background success task");
        database
            .mark_task_succeeded_current_attempt_for_test(background_success)
            .expect("complete background success task");
        database
            .mark_task_failed_current_attempt_for_test(background_failure, "scan failed")
            .expect("fail background task");
        database
            .cancel_task(background_cancel)
            .expect("cancel background task");
        database
            .mark_task_running_current_attempt_for_test(user_success)
            .expect("run user task");
        database
            .mark_task_succeeded_current_attempt_for_test(user_success)
            .expect("complete user task");

        let heads = database
            .notification_heads_for_test()
            .expect("read notification heads");
        let presentation_for = |task_id| {
            heads
                .iter()
                .find(|operation| operation.task_id == Some(task_id))
                .expect("task notification")
                .presentation
                .clone()
        };
        assert_eq!(presentation_for(background_success), "history_only");
        assert_eq!(presentation_for(background_failure), "important");
        assert_eq!(presentation_for(background_cancel), "important");
        assert_eq!(presentation_for(user_success), "important");
    }

    #[test]
    fn root_purge_progress_is_determinate_and_terminal_before_self_delete() {
        let database = migrated_database();
        let (root_id, _folder_id) = seed_media_page_fixture(&database);
        let file_id: i64 = database
            .connection
            .query_row(
                "SELECT id FROM files WHERE root_id = ?1 ORDER BY id LIMIT 1",
                [root_id],
                |row| row.get(0),
            )
            .expect("read purge file");
        database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Background,
            )
            .expect("queue thumbnail for purge");
        let mark = database
            .mark_root_for_purge_with_origin(root_id, NotificationOrigin::User)
            .expect("mark root for purge");
        let (task_id, attempt_generation) = match mark {
            MarkRootForPurge::Marked {
                task_id,
                attempt_generation,
            } => (task_id, attempt_generation),
            other => panic!("unexpected purge mark result: {other:?}"),
        };
        database
            .mark_task_running_for_attempt(task_id, attempt_generation)
            .expect("mark purge running");
        let total = database
            .get_task(task_id)
            .expect("read purge task")
            .expect("purge task exists")
            .items_total
            .expect("purge total");
        assert!(total > 0);

        let mut current = 0;
        loop {
            let outcome = database
                .purge_root_next_batch_for_task(root_id, task_id, attempt_generation, 1)
                .expect("purge batch");
            current += outcome.rows_deleted as i64;
            if outcome.done {
                break;
            }
            let task = database
                .get_task(task_id)
                .expect("read batch progress")
                .expect("purge task remains between batches");
            assert_eq!(task.items_seen, current);
            assert_eq!(task.items_total, Some(total));
        }

        assert_eq!(current, total);
        assert!(database
            .get_task(task_id)
            .expect("read deleted task")
            .is_none());
        assert!(database
            .get_root(root_id)
            .expect("read deleted root")
            .is_none());
        let events = database
            .list_notification_outbox(0, 200)
            .expect("read purge notifications");
        let purge_snapshots = events
            .iter()
            .filter(|event| event.operation.task_id == Some(task_id))
            .collect::<Vec<_>>();
        assert!(purge_snapshots
            .windows(2)
            .all(|pair| { pair[0].operation.current_value <= pair[1].operation.current_value }));
        let running = purge_snapshots
            .iter()
            .find(|event| event.operation.status == "running")
            .expect("running purge event");
        assert_eq!(running.operation.total_value, Some(total));
        let terminal = events
            .iter()
            .find(|event| {
                event.operation.task_id == Some(task_id) && event.operation.status == "succeeded"
            })
            .expect("terminal purge event");
        assert_eq!(terminal.operation.current_value, total);
        assert_eq!(terminal.operation.total_value, Some(total));
    }

    #[test]
    fn root_purge_recovery_preserves_total_current_and_operation_id() {
        let database = migrated_database();
        let (root_id, _folder_id) = seed_media_page_fixture(&database);
        let mark = database
            .mark_root_for_purge_with_origin(root_id, NotificationOrigin::User)
            .expect("mark root for purge");
        let (task_id, attempt_generation) = match mark {
            MarkRootForPurge::Marked {
                task_id,
                attempt_generation,
            } => (task_id, attempt_generation),
            other => panic!("unexpected purge mark result: {other:?}"),
        };
        database
            .mark_task_running_for_attempt(task_id, attempt_generation)
            .expect("mark purge running");
        let original = database
            .get_task(task_id)
            .expect("read initial purge task")
            .expect("initial purge task")
            .items_total
            .expect("initial purge total");

        let first = database
            .purge_root_next_batch_for_task(root_id, task_id, attempt_generation, 1)
            .expect("delete first purge batch");
        assert!(!first.done);
        let first_current = first.rows_deleted as i64;
        let first_task = database
            .get_task(task_id)
            .expect("read first purge progress")
            .expect("first purge task remains");
        assert_eq!(first_task.items_seen, first_current);
        assert_eq!(first_task.items_total, Some(original));
        database
            .reset_running_root_purge_tasks_for_recovery()
            .expect("reset interrupted purge");
        database
            .mark_task_running_for_attempt(task_id, attempt_generation)
            .expect("resume purge");

        let resumed = database
            .get_task(task_id)
            .expect("read resumed purge task")
            .expect("resumed purge task");
        assert_eq!(resumed.items_seen, first_current);
        assert_eq!(resumed.items_total, Some(original));
        let heads = database
            .notification_heads_for_test()
            .expect("read resumed head");
        assert_eq!(
            heads
                .iter()
                .filter(|operation| operation.operation_id == format!("task:{task_id}"))
                .count(),
            1
        );
        let resumed_operation = heads
            .iter()
            .find(|operation| operation.operation_id == format!("task:{task_id}"))
            .expect("resumed operation");
        assert_eq!(resumed_operation.current_value, first_current);
        assert_eq!(resumed_operation.total_value, Some(original));

        let mut current = resumed.items_seen;
        loop {
            let outcome = database
                .purge_root_next_batch_for_task(root_id, task_id, attempt_generation, 1)
                .expect("delete resumed purge batch");
            current += outcome.rows_deleted as i64;
            if outcome.done {
                break;
            }
            let task = database
                .get_task(task_id)
                .expect("read resumed purge progress")
                .expect("resumed purge task remains");
            assert_eq!(task.items_seen, current);
            assert_eq!(task.items_total, Some(original));
        }
        assert_eq!(current, original);
        let terminal = database
            .list_notification_outbox(0, 200)
            .expect("read resumed purge events")
            .into_iter()
            .find(|event| {
                event.operation.operation_id == format!("task:{task_id}")
                    && event.operation.status == "succeeded"
            })
            .expect("resumed terminal event");
        assert_eq!(terminal.operation.current_value, original);
        assert_eq!(terminal.operation.total_value, Some(original));
    }

    #[test]
    fn cancelled_root_purge_clears_pending_delete_without_new_operation() {
        let database = migrated_database();
        let (root_id, _folder_id) = seed_media_page_fixture(&database);
        let task_id = match database
            .mark_root_for_purge_with_origin(root_id, NotificationOrigin::User)
            .expect("mark root for purge")
        {
            MarkRootForPurge::Marked { task_id, .. } => task_id,
            other => panic!("unexpected purge mark result: {other:?}"),
        };
        database.cancel_task(task_id).expect("cancel purge");

        let root = database
            .connection
            .query_row(
                "SELECT pending_delete_at, enabled FROM roots WHERE id = ?1",
                [root_id],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?)),
            )
            .expect("read restored root");
        assert_eq!(root, (None, 1));
        assert_eq!(database.ensure_pending_delete_purge_tasks().unwrap(), 0);
        assert!(database.retry_task(task_id).is_err());
        let events = database.list_notification_outbox(0, 200).unwrap();
        let cancelled = events
            .iter()
            .find(|event| {
                event.operation.task_id == Some(task_id) && event.operation.status == "cancelled"
            })
            .expect("cancelled purge event");
        assert_eq!(cancelled.operation.operation_id, format!("task:{task_id}"));
        assert_eq!(cancelled.operation.status, "cancelled");
        assert!(!cancelled.operation.retryable);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.operation.category == "task")
                .map(|event| event.operation.operation_id.clone())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            1
        );
    }

    #[test]
    fn cancelling_started_root_purge_keeps_partial_root_hidden_and_retries_same_operation() {
        let database = migrated_database();
        let (root_id, _folder_id) = seed_media_page_fixture(&database);
        let (task_id, attempt_generation) = match database
            .mark_root_for_purge_with_origin(root_id, NotificationOrigin::User)
            .expect("mark root for purge")
        {
            MarkRootForPurge::Marked {
                task_id,
                attempt_generation,
            } => (task_id, attempt_generation),
            other => panic!("unexpected purge mark result: {other:?}"),
        };
        database
            .mark_task_running_for_attempt(task_id, attempt_generation)
            .expect("run purge");
        let first = database
            .purge_root_next_batch_for_task(root_id, task_id, attempt_generation, 1)
            .expect("delete first purge batch");
        assert!(!first.done);
        let partial_current = first.rows_deleted as i64;
        assert!(partial_current > 0);

        database.cancel_task(task_id).expect("cancel started purge");
        let root_state = database
            .connection
            .query_row(
                "SELECT pending_delete_at, enabled FROM roots WHERE id = ?1",
                [root_id],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?)),
            )
            .expect("read hidden root");
        assert!(root_state.0.is_some());
        assert_eq!(root_state.1, 0);
        assert!(database.list_roots().expect("list roots").is_empty());

        let cancelled = database
            .get_task(task_id)
            .expect("read cancelled task")
            .expect("cancelled task remains");
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(cancelled.items_seen, partial_current);
        let cancelled_operation = database
            .notification_heads_for_test()
            .unwrap()
            .into_iter()
            .find(|operation| operation.operation_id == format!("task:{task_id}"))
            .expect("cancelled purge operation");
        assert!(cancelled_operation.retryable);
        let retried = database.retry_task(task_id).expect("retry hidden purge");
        assert_eq!(retried.id, task_id);
        assert_eq!(retried.status, "pending");
        assert_eq!(retried.items_seen, partial_current);
        assert_eq!(
            database
                .notification_heads_for_test()
                .unwrap()
                .into_iter()
                .filter(|operation| operation.operation_id == format!("task:{task_id}"))
                .count(),
            1
        );

        database
            .cancel_task(task_id)
            .expect("cancel retried partial purge");
        assert_eq!(
            database
                .ensure_pending_delete_purge_tasks()
                .expect("recover cancelled partial purge"),
            0
        );
        let recovered = database
            .get_task(task_id)
            .expect("read recovered partial purge")
            .expect("recovered partial purge remains");
        assert_eq!(recovered.status, "pending");
        assert_eq!(recovered.items_seen, partial_current);
        assert_eq!(
            database
                .notification_heads_for_test()
                .unwrap()
                .into_iter()
                .filter(|operation| operation.operation_id == format!("task:{task_id}"))
                .count(),
            1
        );
    }

    #[test]
    fn failed_root_purge_retries_same_task_and_operation() {
        let database = migrated_database();
        let (root_id, _folder_id) = seed_media_page_fixture(&database);
        let task_id = match database
            .mark_root_for_purge_with_origin(root_id, NotificationOrigin::User)
            .expect("mark root for purge")
        {
            MarkRootForPurge::Marked { task_id, .. } => task_id,
            other => panic!("unexpected purge mark result: {other:?}"),
        };
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("run purge");
        let running = database
            .get_task(task_id)
            .expect("read running purge")
            .expect("running purge");
        let total = running.items_total.expect("purge total");
        let outcome = database
            .purge_root_next_batch_for_task(root_id, task_id, running.attempt_generation, 1)
            .expect("purge first batch");
        assert!(!outcome.done);
        let current = outcome.rows_deleted as i64;
        let partially_purged = database
            .get_task(task_id)
            .expect("read partial purge")
            .expect("partial purge task remains");
        assert_eq!(partially_purged.items_seen, current);
        assert_eq!(partially_purged.items_total, Some(total));
        database
            .mark_task_failed_current_attempt_for_test(task_id, "purge write failed")
            .expect("fail purge");
        assert!(
            database
                .notification_heads_for_test()
                .unwrap()
                .iter()
                .find(|operation| operation.task_id == Some(task_id))
                .expect("failed purge head")
                .retryable
        );
        let retried = database.retry_task(task_id).expect("retry purge");
        assert_eq!(retried.id, task_id);
        assert_eq!(retried.status, "pending");
        assert_eq!(retried.items_seen, current);
        assert_eq!(retried.items_total, Some(total));
        let heads = database.notification_heads_for_test().unwrap();
        assert_eq!(
            heads
                .iter()
                .filter(|operation| operation.operation_id == format!("task:{task_id}"))
                .count(),
            1
        );
        assert_eq!(
            heads
                .iter()
                .find(|operation| operation.task_id == Some(task_id))
                .unwrap()
                .status,
            "pending"
        );
    }

    #[test]
    fn failed_root_purge_startup_recovery_reuses_existing_task() {
        let database = migrated_database();
        let (root_id, _folder_id) = seed_media_page_fixture(&database);
        let task_id = match database.mark_root_for_purge(root_id).expect("mark") {
            MarkRootForPurge::Marked { task_id, .. } => task_id,
            other => panic!("unexpected mark result: {other:?}"),
        };
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("run purge");
        database
            .mark_task_failed_current_attempt_for_test(task_id, "purge failed")
            .expect("fail purge");

        assert_eq!(database.ensure_pending_delete_purge_tasks().unwrap(), 0);
        let task = database
            .get_task(task_id)
            .expect("read recovered task")
            .expect("recovered task");
        assert_eq!(task.status, "pending");
        assert_eq!(
            database
                .notification_heads_for_test()
                .unwrap()
                .iter()
                .filter(|operation| operation.operation_id == format!("task:{task_id}"))
                .count(),
            1
        );
    }

    #[test]
    fn task_recovery_republishes_pending_state_with_original_origin() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark task running");
        assert_eq!(
            database
                .reset_running_root_scan_tasks_for_recovery()
                .expect("reset running task"),
            1
        );

        let operation = database
            .notification_heads_for_test()
            .expect("read notification heads")
            .into_iter()
            .find(|operation| operation.task_id == Some(task_id))
            .expect("recovered task head");
        assert_eq!(operation.status, "pending");
        assert_eq!(operation.origin, "background");
    }

    #[test]
    fn task_recovery_without_existing_head_uses_recovery_origin() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Recovery".to_string(),
                display_name: "Recovery".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task_with_origin(root_id, NotificationOrigin::User)
            .expect("create root scan task");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark task running");
        database
            .connection
            .execute(
                "DELETE FROM notification_outbox WHERE operation_id = ?1",
                [format!("task:{task_id}")],
            )
            .expect("drop old outbox events");
        database
            .connection
            .execute(
                "DELETE FROM notification_operation_heads WHERE operation_id = ?1",
                [format!("task:{task_id}")],
            )
            .expect("drop old operation head");

        assert_eq!(
            database
                .reset_running_root_scan_tasks_for_recovery()
                .expect("reset running task"),
            1
        );
        let operation = database
            .notification_heads_for_test()
            .expect("read notification heads")
            .into_iter()
            .find(|operation| operation.task_id == Some(task_id))
            .expect("recovered task head");
        assert_eq!(operation.status, "pending");
        assert_eq!(operation.origin, "recovery");
    }

    #[test]
    fn task_is_cancelled_and_attempt_is_current_read_minimal_columns() {
        // Behavior guard for the lightweight `task_is_cancelled` /
        // `task_attempt_is_current` (they read only `status` /
        // `attempt_generation` instead of a full `get_task` row, because the
        // scan hot path calls them per-file). Semantics must be identical to a
        // full-row read.
        let database = Database::open_in_memory().expect("open in-memory db");
        database.apply_migrations().expect("apply migrations");
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        let attempt = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;

        // Pending, current attempt: not cancelled, attempt is current.
        assert!(!database.task_is_cancelled(task_id).expect("cancelled?"));
        assert!(database
            .task_attempt_is_current(task_id, attempt)
            .expect("attempt current?"));
        // A stale attempt is not current.
        assert!(!database
            .task_attempt_is_current(task_id, attempt - 1)
            .expect("stale attempt not current"));
        // A missing task is neither cancelled nor current.
        assert!(!database
            .task_is_cancelled(999_999)
            .expect("missing cancelled"));
        assert!(!database
            .task_attempt_is_current(999_999, 0)
            .expect("missing not current"));

        // After cancel: cancelled = true.
        database.cancel_task(task_id).expect("cancel task");
        assert!(database
            .task_is_cancelled(task_id)
            .expect("cancelled after cancel"));
    }

    #[test]
    fn mark_root_for_purge_is_o1_and_idempotent() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database); // 4 files
                                                                       // Also seed a pending root_scan task to prove it gets cancelled.
        let scan_task = database.create_root_scan_task(root_id).expect("scan task");

        let files_before: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id=?1",
                [root_id],
                |r| r.get(0),
            )
            .expect("count files");
        assert_eq!(files_before, 4);

        let first = database.mark_root_for_purge(root_id).expect("mark");
        let (task_id, _gen) = match first {
            MarkRootForPurge::Marked {
                task_id,
                attempt_generation,
            } => (task_id, attempt_generation),
            other => panic!("expected Marked, got {other:?}"),
        };

        // O(1): no files deleted by marking.
        let files_after: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM files WHERE root_id=?1",
                [root_id],
                |r| r.get(0),
            )
            .expect("count files");
        assert_eq!(files_after, 4, "marking must not delete files");

        // pending_delete_at is set; a root_purge task exists.
        let pending: Option<i64> = database
            .connection
            .query_row(
                "SELECT pending_delete_at FROM roots WHERE id=?1",
                [root_id],
                |r| r.get(0),
            )
            .expect("read pending");
        assert!(pending.is_some());
        let kind: String = database
            .connection
            .query_row("SELECT kind FROM tasks WHERE id=?1", [task_id], |r| {
                r.get(0)
            })
            .expect("task kind");
        assert_eq!(kind, "root_purge");
        // The pre-existing scan task was cancelled (marked failed).
        let scan_status: String = database
            .connection
            .query_row("SELECT status FROM tasks WHERE id=?1", [scan_task], |r| {
                r.get(0)
            })
            .expect("scan status");
        assert_eq!(scan_status, "failed");

        // Idempotent: a second mark makes no new task.
        assert!(matches!(
            database.mark_root_for_purge(root_id).expect("second mark"),
            MarkRootForPurge::AlreadyPending
        ));

        // Unknown root -> NotFound.
        assert!(matches!(
            database.mark_root_for_purge(999_999).expect("missing"),
            MarkRootForPurge::NotFound
        ));

        let ids = database.roots_pending_delete_ids().expect("pending ids");
        assert_eq!(ids, vec![root_id]);
        let _ = folder_id;
    }

    #[test]
    fn list_roots_and_get_root_hide_pending_delete_roots() {
        let database = migrated_database();
        let (root_id, _f) = seed_media_page_fixture(&database);
        database.mark_root_for_purge(root_id).expect("mark");
        assert!(
            database.list_roots().expect("list").is_empty(),
            "pending-delete root hidden from list_roots"
        );
        assert!(
            database.get_root(root_id).expect("get").is_none(),
            "pending-delete root hidden from get_root"
        );
    }

    #[test]
    fn root_pending_delete_by_path_reports_purging_paths() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Y".to_string(),
                display_name: "Y".to_string(),
            })
            .expect("add");
        // Not pending-delete yet.
        assert!(!database
            .root_pending_delete_by_path("D:/Y")
            .expect("check active"));
        database.mark_root_for_purge(root_id).expect("mark");
        // Now pending-delete.
        assert!(database
            .root_pending_delete_by_path("D:/Y")
            .expect("check pending"));
        // Unknown path -> not pending-delete.
        assert!(!database
            .root_pending_delete_by_path("D:/nope")
            .expect("check unknown"));
    }

    #[test]
    fn startup_recovery_reenqueues_purge_for_pending_delete_roots() {
        let database = migrated_database();
        let (root_id, _f) = seed_media_page_fixture(&database);
        let original_task_id = match database.mark_root_for_purge(root_id).expect("mark") {
            MarkRootForPurge::Marked { task_id, .. } => task_id,
            other => panic!("unexpected mark result: {other:?}"),
        };
        // Simulate a crash that lost the task row but kept pending_delete_at:
        database
            .connection
            .execute(
                "DELETE FROM tasks WHERE root_id=?1 AND kind='root_purge'",
                [root_id],
            )
            .expect("drop purge task");

        let created = database
            .ensure_pending_delete_purge_tasks()
            .expect("recover");
        assert_eq!(created, 1);
        let n: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE root_id=?1 AND kind='root_purge' AND status='pending'",
                [root_id],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(n, 1);
        let recovered_task_id: i64 = database
            .connection
            .query_row(
                "SELECT id FROM tasks WHERE root_id=?1 AND kind='root_purge'",
                [root_id],
                |r| r.get(0),
            )
            .expect("read recovered task id");
        assert_eq!(recovered_task_id, original_task_id);

        // Idempotent: a second call with the task already present creates nothing.
        let created_again = database
            .ensure_pending_delete_purge_tasks()
            .expect("recover again");
        assert_eq!(created_again, 0);
    }

    #[test]
    fn startup_recovery_does_not_reuse_terminal_root_purge_head_when_task_is_missing() {
        let database = migrated_database();
        for terminal_status in ["succeeded", "failed", "cancelled"] {
            let root_id = database
                .add_root(NewRoot {
                    path: format!("D:/terminal-head-{terminal_status}"),
                    display_name: format!("terminal-head-{terminal_status}"),
                })
                .expect("add root");
            let original_task_id = match database.mark_root_for_purge(root_id).expect("mark") {
                MarkRootForPurge::Marked { task_id, .. } => task_id,
                other => panic!("unexpected mark result: {other:?}"),
            };
            database
                .mark_task_running_current_attempt_for_test(original_task_id)
                .expect("run purge");
            match terminal_status {
                "succeeded" => database
                    .mark_task_succeeded_current_attempt_for_test(original_task_id)
                    .expect("finish historical purge operation"),
                "failed" => database
                    .mark_task_failed_current_attempt_for_test(original_task_id, "historical")
                    .expect("fail historical purge operation"),
                "cancelled" => {
                    database
                        .cancel_task(original_task_id)
                        .expect("cancel purge");
                }
                _ => unreachable!(),
            };
            database
                .connection
                .execute("DELETE FROM tasks WHERE id = ?1", [original_task_id])
                .expect("drop historical purge task");

            assert_eq!(
                database
                    .ensure_pending_delete_purge_tasks()
                    .expect("recover pending root"),
                1
            );
            let recovered_task_id: i64 = database
                .connection
                .query_row(
                    "SELECT id FROM tasks WHERE root_id = ?1 AND kind = 'root_purge'",
                    [root_id],
                    |row| row.get(0),
                )
                .expect("read new recovery task");
            assert_ne!(recovered_task_id, original_task_id);
            let heads = database
                .notification_heads_for_test()
                .expect("read operation heads");
            assert_eq!(
                heads
                    .iter()
                    .find(|operation| {
                        operation.operation_id == format!("task:{original_task_id}")
                    })
                    .expect("historical head")
                    .status,
                terminal_status
            );
            let recovered = heads
                .iter()
                .find(|operation| operation.task_id == Some(recovered_task_id))
                .expect("recovery head");
            assert_eq!(recovered.status, "pending");
            assert_eq!(recovered.origin, "recovery");
            database
                .connection
                .execute("DELETE FROM tasks WHERE root_id = ?1", [root_id])
                .expect("drop recovered task");
            database
                .connection
                .execute("DELETE FROM roots WHERE id = ?1", [root_id])
                .expect("drop test root");
        }
    }

    #[test]
    fn purge_root_next_batch_drains_leaf_to_root_in_bounded_batches() {
        let database = migrated_database();
        let (root_id, _folder_id) = seed_media_page_fixture(&database); // 4 files + media

        // Prove the media cascade: 4 media rows exist now.
        let media_before: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
            .expect("media");
        assert_eq!(media_before, 4);

        // A thumbnail task referencing a file must be purged (not orphaned to file_id NULL).
        let a_file: i64 = database
            .connection
            .query_row(
                "SELECT id FROM files WHERE root_id=?1 ORDER BY id LIMIT 1",
                [root_id],
                |r| r.get(0),
            )
            .expect("a file");
        database
            .connection
            .execute(
                "INSERT INTO tasks(kind, priority, status, file_id, created_at, updated_at) VALUES ('thumbnail', -10, 'pending', ?1, 1, 1)",
                [a_file],
            )
            .expect("seed thumbnail task");

        database.mark_root_for_purge(root_id).expect("mark");

        // Drain with a tiny batch to force multiple batches.
        let mut batches = 0;
        loop {
            let outcome = database.purge_root_next_batch(root_id, 2).expect("batch");
            assert!(
                outcome.rows_deleted <= 4,
                "each batch is bounded (<= batch_size for tasks/files/folders, + final root sweep)"
            );
            batches += 1;
            if outcome.done {
                break;
            }
            assert!(batches < 100, "must terminate");
        }
        assert!(batches >= 2, "small batch forces multiple batches");

        for (table, col) in [("files", "root_id"), ("folders", "root_id")] {
            let n: i64 = database
                .connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE {col}=?1"),
                    [root_id],
                    |r| r.get(0),
                )
                .expect("count");
            assert_eq!(n, 0, "{table} not drained");
        }
        let roots_n: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM roots WHERE id=?1", [root_id], |r| {
                r.get(0)
            })
            .expect("roots");
        assert_eq!(roots_n, 0, "roots row removed");
        // Media cascaded away with the files.
        let media_after: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM media", [], |r| r.get(0))
            .expect("media after");
        assert_eq!(media_after, 0, "media must cascade-delete with files");
        // No orphan thumbnail task (file_id was purged, not SET NULL).
        let orphan_tasks: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE kind='thumbnail'",
                [],
                |r| r.get(0),
            )
            .expect("count tasks");
        assert_eq!(orphan_tasks, 0);
    }

    #[test]
    fn purge_root_next_batch_is_resumable_after_interruption() {
        let database = migrated_database();
        let (root_id, _folder_id) = seed_media_page_fixture(&database);
        database.mark_root_for_purge(root_id).expect("mark");
        // One batch only (simulate crash), then resume to completion.
        let first = database.purge_root_next_batch(root_id, 2).expect("first");
        assert!(!first.done);
        loop {
            if database
                .purge_root_next_batch(root_id, 2)
                .expect("resume")
                .done
            {
                break;
            }
        }
        let n: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM roots WHERE id=?1", [root_id], |r| {
                r.get(0)
            })
            .expect("count");
        assert_eq!(n, 0);
    }

    #[test]
    fn purge_root_next_batch_deletes_folders_in_bounded_batches() {
        // A folders-only tree (no files) so the folder phase runs first. Root folder
        // (name '') + a chain of children. With the buggy unbounded delete, deleting
        // the root folder cascades the whole subtree in ONE batch; leaf-first keeps
        // each batch bounded.
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/tree".into(),
                display_name: "tree".into(),
            })
            .expect("add root");
        let mut parent: Option<i64> = None;
        let mut ids = Vec::new();
        for depth in 0..6 {
            let id = database
                .upsert_folder(FolderUpsert {
                    root_id,
                    parent_id: parent,
                    name: if depth == 0 {
                        String::new()
                    } else {
                        format!("d{depth}")
                    },
                    path_hash: format!("h{depth}"),
                    mtime: Some(1),
                })
                .expect("folder");
            ids.push(id);
            parent = Some(id);
        }
        let total_before: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE root_id=?1",
                [root_id],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(total_before, 6);

        database.mark_root_for_purge(root_id).expect("mark");

        // Drive batches until the folder phase begins (tasks first: the mark created
        // one root_purge task, excluded from phase 1, so phase 1/2/3 are empty here
        // and phase 4 runs immediately). One batch of 2 must delete at most 2 folders
        // (leaf-first), NOT cascade the whole tree.
        let outcome = database.purge_root_next_batch(root_id, 2).expect("batch");
        assert!(
            !outcome.done,
            "6 folders can't be drained in one batch of 2"
        );
        let remaining: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE root_id=?1",
                [root_id],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(
            remaining, 4,
            "leaf-first batch of 2 must leave 6-2=4 folders (buggy cascade leaves 0)"
        );

        // Finish draining; must complete with the roots row gone.
        let mut guard = 0;
        loop {
            if database
                .purge_root_next_batch(root_id, 2)
                .expect("drain")
                .done
            {
                break;
            }
            guard += 1;
            assert!(guard < 100, "must terminate");
        }
        let roots_n: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM roots WHERE id=?1", [root_id], |r| {
                r.get(0)
            })
            .expect("roots");
        assert_eq!(roots_n, 0);
        let folders_n: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE root_id=?1",
                [root_id],
                |r| r.get(0),
            )
            .expect("folders");
        assert_eq!(folders_n, 0);
        let _ = ids;
    }

    #[test]
    fn create_interactive_folder_scan_task_persists_folder_id_and_lists_pending_task() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "interactive-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "selected".to_string(),
                path_hash: "interactive-selected".to_string(),
                mtime: Some(2),
            })
            .expect("insert selected folder");

        let task_id = database
            .create_interactive_folder_scan_task(folder_id)
            .expect("create interactive folder scan task");

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.kind, "interactive_folder_scan");
        assert_eq!(task.status, "pending");
        assert_eq!(task.root_id, Some(root_id));
        assert_eq!(task.folder_id, Some(folder_id));
        assert_eq!(task.file_id, None);

        let tasks = database.list_tasks().expect("list tasks");
        assert!(tasks.iter().any(|candidate| {
            candidate.id == task_id
                && candidate.kind == "interactive_folder_scan"
                && candidate.root_id == Some(root_id)
                && candidate.folder_id == Some(folder_id)
                && candidate.file_id.is_none()
        }));
    }

    #[test]
    fn create_interactive_folder_scan_task_coalesces_duplicate_pending_folder_requests() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "interactive-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "selected".to_string(),
                path_hash: "interactive-selected".to_string(),
                mtime: Some(2),
            })
            .expect("insert selected folder");

        let first_task_id = database
            .create_interactive_folder_scan_task(folder_id)
            .expect("create first interactive task");
        let second_task_id = database
            .create_interactive_folder_scan_task(folder_id)
            .expect("create duplicate interactive task");

        assert_eq!(second_task_id, first_task_id);
        assert_eq!(
            database
                .list_pending_interactive_folder_scan_task_ids()
                .expect("list pending interactive tasks"),
            vec![first_task_id]
        );
    }

    #[test]
    fn create_interactive_folder_scan_task_retargets_pending_request_to_latest_folder_in_same_root()
    {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "interactive-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let folder_a = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "folder-a".to_string(),
                path_hash: "interactive-folder-a".to_string(),
                mtime: Some(2),
            })
            .expect("insert folder a");
        let folder_b = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "folder-b".to_string(),
                path_hash: "interactive-folder-b".to_string(),
                mtime: Some(3),
            })
            .expect("insert folder b");
        let folder_c = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "folder-c".to_string(),
                path_hash: "interactive-folder-c".to_string(),
                mtime: Some(4),
            })
            .expect("insert folder c");

        let first_task_id = database
            .create_interactive_folder_scan_task(folder_a)
            .expect("create folder a interactive task");
        database
            .create_interactive_folder_scan_task(folder_b)
            .expect("retarget pending task to folder b");
        let latest_task_id = database
            .create_interactive_folder_scan_task(folder_c)
            .expect("retarget pending task to folder c");

        assert_eq!(latest_task_id, first_task_id);
        let pending_ids = database
            .list_pending_interactive_folder_scan_task_ids()
            .expect("list pending interactive tasks");
        assert_eq!(pending_ids, vec![first_task_id]);
        let pending_task = database
            .get_task(first_task_id)
            .expect("get pending interactive task")
            .expect("pending interactive task exists");
        assert_eq!(pending_task.status, "pending");
        assert_eq!(pending_task.folder_id, Some(folder_c));
    }

    #[test]
    fn create_interactive_folder_scan_task_keeps_only_latest_pending_target_when_running_exists() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let root_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "interactive-root".to_string(),
                mtime: Some(1),
            })
            .expect("insert root folder");
        let folder_a = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "folder-a".to_string(),
                path_hash: "interactive-folder-a".to_string(),
                mtime: Some(2),
            })
            .expect("insert folder a");
        let folder_b = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "folder-b".to_string(),
                path_hash: "interactive-folder-b".to_string(),
                mtime: Some(3),
            })
            .expect("insert folder b");
        let folder_c = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "folder-c".to_string(),
                path_hash: "interactive-folder-c".to_string(),
                mtime: Some(4),
            })
            .expect("insert folder c");

        let running_task_id = database
            .create_interactive_folder_scan_task(folder_a)
            .expect("create running interactive task");
        database
            .mark_task_running_current_attempt_for_test(running_task_id)
            .expect("mark interactive task running");

        let first_pending_task_id = database
            .create_interactive_folder_scan_task(folder_b)
            .expect("create pending interactive task for folder b");
        let latest_pending_task_id = database
            .create_interactive_folder_scan_task(folder_c)
            .expect("retarget pending interactive task to folder c");

        assert_eq!(latest_pending_task_id, first_pending_task_id);
        let tasks = database.list_tasks().expect("list tasks");
        let pending_tasks: Vec<_> = tasks
            .iter()
            .filter(|task| task.kind == "interactive_folder_scan" && task.status == "pending")
            .collect();
        assert_eq!(pending_tasks.len(), 1);
        assert_eq!(pending_tasks[0].id, first_pending_task_id);
        assert_eq!(pending_tasks[0].folder_id, Some(folder_c));

        let running_tasks: Vec<_> = tasks
            .iter()
            .filter(|task| task.kind == "interactive_folder_scan" && task.status == "running")
            .collect();
        assert_eq!(running_tasks.len(), 1);
        assert_eq!(running_tasks[0].id, running_task_id);
        assert_eq!(running_tasks[0].folder_id, Some(folder_a));
    }

    #[test]
    fn thumbnail_request_coalesces_task_and_records_state_transition() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "coalesce.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: Some("identity-1".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");

        let first = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Background,
            )
            .expect("request thumbnail");
        let second = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail again");

        assert_eq!(first.thumbnail.state, "queued");
        assert_eq!(second.thumbnail.state, "queued");
        let task_id = first.task_id.expect("first request task id");
        assert_eq!(Some(task_id), second.task_id);
        assert!(first.queued);
        assert!(!second.queued);

        let tasks = database.list_tasks().expect("list tasks");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, task_id);
        assert_eq!(tasks[0].kind, "thumbnail");
        assert_eq!(tasks[0].status, "pending");
        assert_eq!(tasks[0].root_id, None);
        assert_eq!(tasks[0].file_id, Some(file_id));

        let pending = database
            .list_pending_thumbnail_task_ids()
            .expect("list thumbnail tasks");
        assert_eq!(pending, vec![task_id]);

        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark thumbnail running");
        let running_request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request while running");
        assert_eq!(running_request.task_id, Some(task_id));
        assert!(!running_request.queued);
        assert_eq!(database.list_tasks().expect("list tasks").len(), 1);

        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get thumbnail source")
            .expect("thumbnail source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "ready".to_string(),
                cache_key: Some("aa/bb/coalesce.webp".to_string()),
                width: Some(427),
                height: Some(320),
                byte_size: Some(64),
                error: None,
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("mark thumbnail ready");
        database
            .mark_task_succeeded_current_attempt_for_test(task_id)
            .expect("mark task succeeded");

        let ready = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request ready thumbnail");
        assert_eq!(ready.thumbnail.state, "ready");
        assert_eq!(ready.task_id, Some(task_id));
        assert!(!ready.queued);
        assert_eq!(database.list_tasks().expect("list tasks").len(), 1);
    }

    #[test]
    fn thumbnail_request_promotes_existing_pending_task_priority_when_lower_than_requested() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "priority-upgrade.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: Some("identity-priority".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");

        let first = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Background,
            )
            .expect("request thumbnail");
        let task_id = first.task_id.expect("thumbnail task id");
        database
            .connection
            .execute("UPDATE tasks SET priority = 0 WHERE id = ?1", [task_id])
            .expect("lower pending priority");

        let second = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail again");
        assert_eq!(second.task_id, Some(task_id));

        let task = database
            .get_task(task_id)
            .expect("get thumbnail task")
            .expect("thumbnail task exists");
        assert_eq!(task.status, "pending");
        assert_eq!(task.priority, THUMBNAIL_VISIBLE_PRIORITY);
    }

    #[test]
    fn thumbnail_bulk_priority_is_lower_than_background() {
        assert!(ThumbnailTaskPriority::Bulk.task_priority() < THUMBNAIL_BACKGROUND_PRIORITY);
    }

    #[test]
    fn thumbnail_request_promotes_existing_bulk_pending_task_priority_when_requested() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "bulk-priority-upgrade.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: Some("identity-bulk-priority".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");

        let first = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Bulk,
            )
            .expect("request bulk thumbnail");
        let task_id = first.task_id.expect("thumbnail task id");

        let second = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail again");
        assert_eq!(second.task_id, Some(task_id));

        let task = database
            .get_task(task_id)
            .expect("get thumbnail task")
            .expect("thumbnail task exists");
        assert_eq!(task.status, "pending");
        assert_eq!(task.priority, THUMBNAIL_VISIBLE_PRIORITY);
    }

    #[test]
    fn thumbnail_request_creates_new_selected_pending_task_when_lower_priority_task_is_running() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "selected-preempts-running.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: Some("identity-selected-running".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");

        let background = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Background,
            )
            .expect("request background thumbnail");
        let background_task_id = background.task_id.expect("background task id");
        database
            .mark_task_running_current_attempt_for_test(background_task_id)
            .expect("mark background thumbnail running");

        let selected = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Selected,
            )
            .expect("request selected thumbnail");
        let selected_task_id = selected.task_id.expect("selected task id");

        assert_ne!(selected_task_id, background_task_id);
        assert!(selected.queued);

        let duplicate_selected = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Selected,
            )
            .expect("repeat selected request");
        assert_eq!(duplicate_selected.task_id, Some(selected_task_id));
        assert!(!duplicate_selected.queued);

        let tasks = database.list_tasks().expect("list tasks");
        let running_background = tasks
            .iter()
            .find(|task| task.id == background_task_id)
            .expect("background task exists");
        assert_eq!(running_background.status, "running");
        assert_eq!(running_background.priority, THUMBNAIL_BACKGROUND_PRIORITY);

        let pending_selected: Vec<_> = tasks
            .iter()
            .filter(|task| {
                task.kind == "thumbnail"
                    && task.file_id == Some(file_id)
                    && task.status == "pending"
                    && task.priority == THUMBNAIL_SELECTED_PRIORITY
            })
            .collect();
        assert_eq!(pending_selected.len(), 1);
        assert_eq!(pending_selected[0].id, selected_task_id);
    }

    #[test]
    fn thumbnail_cache_enqueue_respects_refresh_mode_and_limit() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_thumbnail_cache_scope_fixture(&database, "cache-enqueue");

        let _cached_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "cached.jpg",
            Some("candidate-cached"),
            "ready",
            true,
            Some(10),
            None,
        );
        let missing_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "missing.jpg",
            Some("candidate-missing"),
            "absent",
            false,
            None,
            None,
        );
        let stale_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "stale.jpg",
            Some("candidate-stale"),
            "ready",
            true,
            Some(20),
            Some(19),
        );
        let failed_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "failed.jpg",
            Some("candidate-failed"),
            "failed",
            false,
            None,
            None,
        );
        // `RetryFailedAndStale` only treats `transient` failures as automatic
        // retry candidates (Task 5); this fixture's failed row must be
        // classified as such to keep exercising that path below.
        database
            .connection
            .execute(
                "UPDATE thumbs SET failure_kind = 'transient' WHERE file_id = ?1",
                [failed_file_id],
            )
            .expect("classify seeded failure as transient");

        let stats = database
            .get_thumbnail_cache_stats(ThumbnailCacheScope {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
            })
            .expect("get thumbnail cache stats");
        assert_eq!(stats.cached_count, 1);
        assert_eq!(stats.missing_count, 1);
        assert_eq!(stats.stale_count, 1);
        assert_eq!(stats.failed_count, 1);
        assert_eq!(stats.pending_candidate_count, 2);

        let enqueue_missing_or_stale = database
            .enqueue_thumbnail_cache_tasks(
                ThumbnailCacheScope {
                    root_id: Some(root_id),
                    folder_id: Some(folder_id),
                    include_descendants: false,
                },
                ThumbnailCacheRefreshMode::StaleOrMissing,
                2,
                None,
            )
            .expect("enqueue stale or missing candidates");
        assert_eq!(enqueue_missing_or_stale.accepted_count, 2);
        let bulk_file_ids: Vec<i64> = database
            .list_tasks()
            .expect("list tasks")
            .into_iter()
            .filter(|task| task.kind == "thumbnail" && task.priority == THUMBNAIL_BULK_PRIORITY)
            .filter_map(|task| task.file_id)
            .collect();
        assert_eq!(bulk_file_ids.len(), 2);
        assert!(bulk_file_ids.contains(&missing_file_id));
        assert!(bulk_file_ids.contains(&stale_file_id));
        assert!(!bulk_file_ids.contains(&failed_file_id));

        let retry_failed = database
            .enqueue_thumbnail_cache_tasks(
                ThumbnailCacheScope {
                    root_id: Some(root_id),
                    folder_id: Some(folder_id),
                    include_descendants: false,
                },
                ThumbnailCacheRefreshMode::RetryFailedAndStale,
                4,
                None,
            )
            .expect("enqueue stale and failed candidates");
        assert!(retry_failed.accepted_count >= 1);
        let failed_bulk_task = database
            .list_tasks()
            .expect("list tasks after retry")
            .into_iter()
            .find(|task| {
                task.kind == "thumbnail"
                    && task.priority == THUMBNAIL_BULK_PRIORITY
                    && task.file_id == Some(failed_file_id)
            });
        assert!(failed_bulk_task.is_some());
    }

    #[test]
    fn retry_failed_and_stale_candidate_gate_only_retries_transient_failures() {
        // Task 5: the automatic `RetryFailedAndStale` bulk enqueue must treat
        // `state = 'failed'` as a candidate only when `failure_kind =
        // 'transient'`. A `permanent` failure (bad format, corrupt file, ...)
        // must never be picked up by this batch path.
        let database = migrated_database();
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-retry-gate");

        let transient_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "transient-failed.jpg",
            Some("candidate-transient-failed"),
            "failed",
            false,
            None,
            None,
        );
        database
            .connection
            .execute(
                "UPDATE thumbs SET failure_kind = 'transient' WHERE file_id = ?1",
                [transient_file_id],
            )
            .expect("classify transient failure");

        let permanent_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "permanent-failed.jpg",
            Some("candidate-permanent-failed"),
            "failed",
            false,
            None,
            None,
        );
        database
            .connection
            .execute(
                "UPDATE thumbs SET failure_kind = 'permanent' WHERE file_id = ?1",
                [permanent_file_id],
            )
            .expect("classify permanent failure");

        let scope = ThumbnailCacheScope {
            root_id: Some(root_id),
            folder_id: Some(folder_id),
            include_descendants: false,
        };
        let result = database
            .enqueue_thumbnail_cache_tasks(
                scope,
                ThumbnailCacheRefreshMode::RetryFailedAndStale,
                4,
                None,
            )
            .expect("enqueue retry-failed-and-stale candidates");
        assert_eq!(
            result.accepted_count, 1,
            "only the transient failure should be accepted as an automatic retry candidate"
        );

        let transient_thumbnail = database
            .get_thumbnail(transient_file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get transient thumbnail")
            .expect("transient thumbnail exists");
        assert_eq!(
            transient_thumbnail.state, "queued",
            "transient failure must be requeued by the automatic retry gate"
        );

        let permanent_thumbnail = database
            .get_thumbnail(permanent_file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get permanent thumbnail")
            .expect("permanent thumbnail exists");
        assert_eq!(
            permanent_thumbnail.state, "failed",
            "permanent failure must never be touched by the automatic retry gate"
        );

        let tasks = database.list_tasks().expect("list tasks after retry gate");
        let transient_task = tasks.iter().find(|task| {
            task.kind == "thumbnail"
                && task.priority == THUMBNAIL_BULK_PRIORITY
                && task.file_id == Some(transient_file_id)
        });
        assert!(
            transient_task.is_some(),
            "transient failure must get a bulk retry task"
        );
        let permanent_task = tasks.iter().find(|task| {
            task.kind == "thumbnail"
                && task.priority == THUMBNAIL_BULK_PRIORITY
                && task.file_id == Some(permanent_file_id)
        });
        assert!(
            permanent_task.is_none(),
            "permanent failure must not get an automatic bulk retry task"
        );
    }

    #[test]
    fn manual_retry_task_still_retries_permanent_failure() {
        // Task 5: the explicit single-task `retry_task` path is the user clicking
        // "retry" on one item. It must remain able to
        // retry ANY failed thumbnail task, including one whose thumbnail row was
        // classified `permanent` — the transient-only gate applies solely to the
        // automatic `RetryFailedAndStale` bulk candidate query, which this path
        // never calls.
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "manual-retry-permanent.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("manual-retry-permanent-source".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);

        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        let task_id = request.task_id.expect("thumbnail task id");
        let attempt_generation = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;
        database
            .mark_thumbnail_task_running_for_attempt(
                task_id,
                attempt_generation,
                &source_fingerprint,
            )
            .expect("mark thumbnail running");
        let published = database
            .publish_thumbnail_failure_for_attempted_source_for_task_attempt(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                Some(source_fingerprint.as_str()),
                "thumbnail decode failed: unsupported format",
                "permanent",
                task_id,
                attempt_generation,
            )
            .expect("publish permanent failure");
        assert!(published);
        database
            .mark_task_failed_current_attempt_for_test(
                task_id,
                "thumbnail decode failed: unsupported format",
            )
            .expect("mark task failed");

        let (state, failure_kind): (String, Option<String>) = database
            .connection
            .query_row(
                "SELECT state, failure_kind FROM thumbs WHERE file_id = ?1 AND profile = ?2",
                (file_id, crate::thumbnails::GRID_320_PROFILE),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query thumbs row before retry");
        assert_eq!(state, "failed");
        assert_eq!(failure_kind.as_deref(), Some("permanent"));

        // The user explicitly retries this one failed task.
        let retried = database
            .retry_task(task_id)
            .expect("manual retry of a permanent failure must succeed");
        assert_eq!(retried.status, "pending");

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(
            thumbnail.state, "queued",
            "manual retry must requeue the thumbnail regardless of failure_kind"
        );
    }

    #[test]
    fn thumbnail_cache_scoped_candidate_query_is_limited_in_sql() {
        let database = migrated_database();
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-bounded-query");
        for index in 0..50 {
            seed_thumbnail_cache_candidate(
                &database,
                root_id,
                folder_id,
                &format!("candidate-{index:03}.jpg"),
                Some(&format!("candidate-bounded-query-{index:03}")),
                "absent",
                false,
                None,
                None,
            );
        }

        let (sql, params) = thumbnail_cache_candidate_query(
            ThumbnailCacheScope {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
            },
            ThumbnailCacheRefreshMode::MissingOnly,
            7,
        );
        assert!(
            sql.contains("LIMIT"),
            "candidate selection must be bounded at SQL time"
        );
        let mut statement = database
            .connection
            .prepare(&sql)
            .expect("prepare bounded candidate query");
        let rows = statement
            .query_map(params_from_iter(params), thumbnail_cache_entry_from_row)
            .expect("query bounded candidates");
        let entries: Vec<_> = rows
            .collect::<Result<Vec<_>, _>>()
            .expect("collect bounded candidates");
        assert_eq!(entries.len(), 7);
    }

    #[test]
    fn thumbnail_cache_windowed_enqueue_advances_cursor_and_completes() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_thumbnail_cache_scope_fixture(&database, "cache-window");
        // 10 missing candidates. A window of 4 must walk them in file_id order,
        // enqueue every candidate it scans (no accept cap), advance the cursor
        // past the last file scanned, and report scan-complete (next_cursor
        // None) once a window comes back short.
        let mut file_ids = Vec::new();
        for index in 0..10 {
            file_ids.push(seed_thumbnail_cache_candidate(
                &database,
                root_id,
                folder_id,
                &format!("window-{index:03}.jpg"),
                Some(&format!("window-key-{index:03}")),
                "absent",
                false,
                None,
                None,
            ));
        }
        file_ids.sort_unstable();

        let scope = ThumbnailCacheScope {
            root_id: Some(root_id),
            folder_id: Some(folder_id),
            include_descendants: false,
        };
        let mut cursor = Some(0);
        let mut total_accepted = 0usize;
        let mut batches = 0;
        while let Some(after) = cursor {
            let result = database
                .enqueue_thumbnail_cache_tasks(
                    scope,
                    ThumbnailCacheRefreshMode::StaleOrMissing,
                    4,
                    Some(after),
                )
                .expect("windowed enqueue batch");
            total_accepted += result.accepted_count;
            batches += 1;
            assert!(batches <= 6, "windowed enqueue must terminate");
            cursor = result.next_cursor;
        }

        // Full windows of 4 over 10 files: cursors after 4 and 8, then a short
        // window of 2 that ends the scan.
        assert_eq!(batches, 3);
        assert_eq!(total_accepted, 10);
        let bulk_file_ids: Vec<i64> = database
            .list_tasks()
            .expect("list tasks")
            .into_iter()
            .filter(|task| task.kind == "thumbnail" && task.priority == THUMBNAIL_BULK_PRIORITY)
            .filter_map(|task| task.file_id)
            .collect();
        assert_eq!(bulk_file_ids.len(), 10);
        for file_id in &file_ids {
            assert!(bulk_file_ids.contains(file_id));
        }
    }

    #[test]
    fn thumbnail_cache_stats_use_current_status_semantics() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_thumbnail_cache_scope_fixture(&database, "cache-stats");
        let child_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(folder_id),
                name: "child".to_string(),
                path_hash: "cache-stats-child-hash".to_string(),
                mtime: Some(11),
            })
            .expect("insert stats child folder");

        seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "cached.jpg",
            Some("stats-cached"),
            "ready",
            true,
            Some(10),
            None,
        );
        seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "ready-without-blob.jpg",
            Some("stats-ready-missing"),
            "ready",
            false,
            None,
            None,
        );
        seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "stale.jpg",
            Some("stats-stale"),
            "ready",
            true,
            Some(20),
            Some(19),
        );
        seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "failed.jpg",
            Some("stats-failed"),
            "failed",
            false,
            None,
            None,
        );
        let skipped_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "skipped-small.jpg",
            Some("stats-skipped"),
            "absent",
            false,
            None,
            None,
        );
        database
            .update_media_dimensions(skipped_file_id, 128, 128)
            .expect("mark skipped-small dimensions");
        seed_thumbnail_cache_candidate(
            &database,
            root_id,
            child_folder_id,
            "child-missing.jpg",
            Some("stats-child-missing"),
            "absent",
            false,
            None,
            None,
        );

        let direct_stats = database
            .get_thumbnail_cache_stats(ThumbnailCacheScope {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
            })
            .expect("get direct stats");
        assert_eq!(direct_stats.cached_count, 1);
        assert_eq!(direct_stats.missing_count, 1);
        assert_eq!(direct_stats.stale_count, 1);
        assert_eq!(direct_stats.failed_count, 1);
        assert_eq!(direct_stats.pending_candidate_count, 2);
        assert_eq!(direct_stats.total_blob_bytes, 30);

        let tree_stats = database
            .get_thumbnail_cache_stats(ThumbnailCacheScope {
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: true,
            })
            .expect("get tree stats");
        assert_eq!(tree_stats.cached_count, 1);
        assert_eq!(tree_stats.missing_count, 2);
        assert_eq!(tree_stats.stale_count, 1);
        assert_eq!(tree_stats.failed_count, 1);
        assert_eq!(tree_stats.pending_candidate_count, 3);
        assert_eq!(tree_stats.total_blob_bytes, 30);
    }

    #[test]
    fn cancel_pending_bulk_thumbnail_tasks_only_drops_bulk_pending() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_thumbnail_cache_scope_fixture(&database, "cancel-bulk");
        // Two missing candidates -> two bulk pending tasks.
        for index in 0..2 {
            seed_thumbnail_cache_candidate(
                &database,
                root_id,
                folder_id,
                &format!("bulk-{index}.jpg"),
                Some(&format!("cancel-bulk-{index}")),
                "absent",
                false,
                None,
                None,
            );
        }
        let bulk = database
            .enqueue_thumbnail_cache_tasks(
                ThumbnailCacheScope {
                    root_id: Some(root_id),
                    folder_id: Some(folder_id),
                    include_descendants: false,
                },
                ThumbnailCacheRefreshMode::StaleOrMissing,
                8,
                None,
            )
            .expect("enqueue bulk candidates");
        assert_eq!(bulk.accepted_count, 2);

        // A higher-priority (visible) task for a different file must survive.
        let visible_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "visible.jpg",
            Some("cancel-bulk-visible"),
            "absent",
            false,
            None,
            None,
        );
        database
            .request_thumbnail_task(
                visible_file_id,
                GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request visible thumbnail");

        let cancelled = database
            .cancel_pending_bulk_thumbnail_tasks()
            .expect("cancel pending bulk tasks");
        assert_eq!(cancelled, 2, "both bulk pending tasks cancelled");

        let remaining_bulk_pending = database
            .list_tasks()
            .expect("list tasks")
            .into_iter()
            .filter(|task| {
                task.kind == "thumbnail"
                    && task.priority == THUMBNAIL_BULK_PRIORITY
                    && task.status == "pending"
            })
            .count();
        assert_eq!(remaining_bulk_pending, 0, "no bulk pending tasks remain");

        // The visible task is untouched.
        let visible_pending = database
            .list_tasks()
            .expect("list tasks")
            .into_iter()
            .filter(|task| {
                task.kind == "thumbnail"
                    && task.priority == THUMBNAIL_VISIBLE_PRIORITY
                    && task.status == "pending"
                    && task.file_id == Some(visible_file_id)
            })
            .count();
        assert_eq!(visible_pending, 1, "visible task must survive cancel-bulk");
    }

    #[test]
    fn clear_thumbnail_cache_deletes_blobs_and_resets_states() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_thumbnail_cache_scope_fixture(&database, "cache-clear");
        let file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "clear.jpg",
            Some("candidate-clear"),
            "ready",
            true,
            Some(40),
            None,
        );

        let cleared = database
            .clear_thumbnail_cache()
            .expect("clear thumbnail cache");
        assert!(cleared.deleted_blob_count >= 1);
        assert!(cleared.reset_thumbnail_count >= 1);
        assert!(database
            .get_thumb_blob(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumb blob")
            .is_none());
        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "pending");
    }

    #[test]
    fn preview_blob_round_trips_and_reports_staleness_inputs() {
        let database = migrated_database();
        let file_id = seed_single_media_file(&database);

        assert!(database
            .get_preview_blob(file_id, "preview_2048")
            .expect("query empty")
            .is_none());

        database
            .upsert_preview_blob(PreviewBlobRecord {
                file_id,
                profile: "preview_2048".to_string(),
                data: vec![1, 2, 3, 4],
                width: 2048,
                height: 1152,
                byte_size: 4,
                output_format: "image/webp".to_string(),
                source_size: 9_000_000,
                source_mtime: 1_700_000_000,
                created_at: 10,
                updated_at: 10,
            })
            .expect("insert");

        let stored = database
            .get_preview_blob(file_id, "preview_2048")
            .expect("query")
            .expect("row present");
        assert_eq!(stored.data, vec![1, 2, 3, 4]);
        assert_eq!(stored.width, 2048);
        assert_eq!(stored.source_size, 9_000_000);
        assert_eq!(stored.source_mtime, 1_700_000_000);
    }

    #[test]
    fn preview_success_and_negative_cache_rows_replace_each_other() {
        let database = migrated_database();
        let file_id = seed_single_media_file(&database);
        let blob = PreviewBlobRecord {
            file_id,
            profile: "preview_native".to_string(),
            data: vec![1, 2, 3],
            width: 100,
            height: 50,
            byte_size: 3,
            output_format: "image/webp".to_string(),
            source_size: 99,
            source_mtime: 100,
            created_at: 10,
            updated_at: 10,
        };
        database
            .upsert_preview_blob(blob.clone())
            .expect("seed blob");

        database
            .upsert_preview_failure(PreviewFailureRecord {
                file_id,
                profile: "preview_native".to_string(),
                failure_kind: "permanent".to_string(),
                error: "unsupported camera".to_string(),
                source_size: 99,
                source_mtime: 100,
                created_at: 20,
                updated_at: 20,
            })
            .expect("store failure");
        assert!(database
            .get_preview_blob(file_id, "preview_native")
            .expect("query blob")
            .is_none());
        let failure = database
            .get_preview_failure(file_id, "preview_native")
            .expect("query failure")
            .expect("failure row");
        assert_eq!(failure.failure_kind, "permanent");
        assert_eq!(failure.error, "unsupported camera");
        assert_eq!(failure.source_size, 99);

        database
            .upsert_preview_blob(blob)
            .expect("replace with blob");
        assert!(database
            .get_preview_failure(file_id, "preview_native")
            .expect("query cleared failure")
            .is_none());
    }

    #[test]
    fn preview_blob_upsert_replaces_the_same_profile_in_place() {
        let database = migrated_database();
        let file_id = seed_single_media_file(&database);

        for (byte, updated_at) in [(1u8, 10i64), (2u8, 20i64)] {
            database
                .upsert_preview_blob(PreviewBlobRecord {
                    file_id,
                    profile: "preview_2048".to_string(),
                    data: vec![byte],
                    width: 2048,
                    height: 1152,
                    byte_size: 1,
                    output_format: "image/webp".to_string(),
                    source_size: 1,
                    source_mtime: 1,
                    created_at: 10,
                    updated_at,
                })
                .expect("upsert");
        }

        let count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM preview_blobs", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 1);
        let stored = database
            .get_preview_blob(file_id, "preview_2048")
            .expect("query")
            .expect("row");
        assert_eq!(stored.data, vec![2]);
        assert_eq!(stored.updated_at, 20);
    }

    #[test]
    fn preview_blob_cache_evicts_least_recently_used_rows_past_the_budget() {
        let database = migrated_database();
        let file_ids = seed_media_files(&database, 3);
        // Each blob is half the budget, so the third insert must evict the
        // oldest by updated_at.
        let half = PREVIEW_BLOB_CACHE_LIMIT_BYTES / 2;
        for (index, file_id) in file_ids.iter().enumerate() {
            database
                .upsert_preview_blob(PreviewBlobRecord {
                    file_id: *file_id,
                    profile: "preview_2048".to_string(),
                    data: vec![index as u8],
                    width: 2048,
                    height: 1152,
                    byte_size: half,
                    output_format: "image/webp".to_string(),
                    source_size: 1,
                    source_mtime: 1,
                    created_at: 10,
                    updated_at: 100 + index as i64,
                })
                .expect("upsert");
        }

        assert!(
            database
                .get_preview_blob(file_ids[0], "preview_2048")
                .expect("query")
                .is_none(),
            "oldest tier should be evicted"
        );
        // The middle tier still fits inside the budget: without this, a
        // regression that evicted everything except the row just written would
        // still satisfy the two assertions around it.
        assert!(database
            .get_preview_blob(file_ids[1], "preview_2048")
            .expect("query")
            .is_some());
        assert!(database
            .get_preview_blob(file_ids[2], "preview_2048")
            .expect("query")
            .is_some());
    }

    #[test]
    fn preview_blob_larger_than_the_whole_budget_is_still_stored() {
        // A tier that alone exceeds the budget must still be readable after the
        // write that produced it: the eviction pass runs in the same
        // transaction as the upsert and must never choose its own row as a
        // victim, or the caller would decode, store, and immediately miss.
        let database = migrated_database();
        let file_id = seed_single_media_file(&database);

        database
            .upsert_preview_blob(PreviewBlobRecord {
                file_id,
                profile: "preview_native".to_string(),
                data: vec![9],
                width: 8192,
                height: 8192,
                byte_size: PREVIEW_BLOB_CACHE_LIMIT_BYTES + 1,
                output_format: "image/webp".to_string(),
                source_size: 1,
                source_mtime: 1,
                created_at: 10,
                updated_at: 10,
            })
            .expect("upsert oversized tier");

        assert!(
            database
                .get_preview_blob(file_id, "preview_native")
                .expect("query")
                .is_some(),
            "a tier larger than the whole budget must survive its own write"
        );
    }

    #[test]
    fn clearing_the_thumbnail_cache_also_drops_preview_tiers() {
        let database = migrated_database();
        let file_id = seed_single_media_file(&database);
        database
            .upsert_preview_blob(PreviewBlobRecord {
                file_id,
                profile: "preview_2048".to_string(),
                data: vec![7],
                width: 2048,
                height: 1152,
                byte_size: 1,
                output_format: "image/webp".to_string(),
                source_size: 1,
                source_mtime: 1,
                created_at: 10,
                updated_at: 10,
            })
            .expect("upsert");
        assert!(database
            .get_preview_blob(file_id, "preview_2048")
            .expect("query seeded tier")
            .is_some());
        database
            .upsert_preview_failure(PreviewFailureRecord {
                file_id,
                profile: "preview_native".to_string(),
                failure_kind: "permanent".to_string(),
                error: "unsupported".to_string(),
                source_size: 1,
                source_mtime: 1,
                created_at: 10,
                updated_at: 10,
            })
            .expect("seed failure");

        database.clear_thumbnail_cache().expect("clear");

        assert!(database
            .get_preview_blob(file_id, "preview_2048")
            .expect("query")
            .is_none());
        assert!(database
            .get_preview_failure(file_id, "preview_native")
            .expect("query failure")
            .is_none());
    }

    #[test]
    fn deleting_a_file_cascades_to_its_preview_tiers() {
        let database = migrated_database();
        let file_id = seed_single_media_file(&database);
        database
            .upsert_preview_blob(PreviewBlobRecord {
                file_id,
                profile: "preview_2048".to_string(),
                data: vec![7],
                width: 2048,
                height: 1152,
                byte_size: 1,
                output_format: "image/webp".to_string(),
                source_size: 1,
                source_mtime: 1,
                created_at: 10,
                updated_at: 10,
            })
            .expect("upsert");
        assert!(database
            .get_preview_blob(file_id, "preview_2048")
            .expect("query seeded tier")
            .is_some());

        // SQLite only enforces foreign keys when the pragma is on, so assert it
        // instead of assuming — otherwise a cascade regression would pass here
        // simply because nothing was ever enforced.
        database
            .connection
            .pragma_update(None, "foreign_keys", "ON")
            .expect("enable foreign keys");
        let foreign_keys_enabled: i64 = database
            .connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("read foreign_keys pragma");
        assert_eq!(
            foreign_keys_enabled, 1,
            "foreign key enforcement must be on for this test to mean anything"
        );
        database
            .connection
            .execute("DELETE FROM files WHERE id = ?1", [file_id])
            .expect("delete file");

        let count: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM preview_blobs", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 0);
    }

    #[test]
    fn thumbnail_cache_enqueue_can_target_explicit_seen_file_ids() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_thumbnail_cache_scope_fixture(&database, "cache-explicit");
        let missing_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "missing.jpg",
            Some("candidate-missing-explicit"),
            "absent",
            false,
            None,
            None,
        );
        let skipped_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "skipped.jpg",
            Some("candidate-skipped-explicit"),
            "ready",
            true,
            None,
            None,
        );
        database
            .update_media_dimensions(skipped_file_id, 128, 128)
            .expect("mark explicit skipped media dimensions");

        let enqueue_result = database
            .enqueue_thumbnail_cache_tasks_for_file_ids(
                &[missing_file_id, skipped_file_id, 999_999],
                ThumbnailCacheRefreshMode::StaleOrMissing,
                16,
            )
            .expect("enqueue explicit file ids");

        assert_eq!(enqueue_result.accepted_count, 1);
        let queued_task = database
            .list_tasks()
            .expect("list explicit queued tasks")
            .into_iter()
            .find(|task| {
                task.kind == "thumbnail"
                    && task.file_id == Some(missing_file_id)
                    && task.priority == THUMBNAIL_BULK_PRIORITY
            });
        assert!(queued_task.is_some());
    }

    #[test]
    fn thumbnail_cache_scoped_enqueue_requeues_ready_current_row_without_blob() {
        let database = migrated_database();
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-scoped-ready-no-blob");
        let missing_blob_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "ready-without-blob.jpg",
            Some("candidate-scoped-ready-without-blob"),
            "ready",
            false,
            None,
            None,
        );
        let cached_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "ready-with-blob.jpg",
            Some("candidate-scoped-ready-with-blob"),
            "ready",
            true,
            Some(64),
            None,
        );
        let scope = ThumbnailCacheScope {
            root_id: Some(root_id),
            folder_id: Some(folder_id),
            include_descendants: false,
        };

        let stats = database
            .get_thumbnail_cache_stats(scope)
            .expect("get scoped stats");
        assert_eq!(stats.cached_count, 1);
        assert_eq!(stats.missing_count, 1);
        assert_eq!(stats.pending_candidate_count, 1);

        let enqueue_result = database
            .enqueue_thumbnail_cache_tasks(
                scope,
                ThumbnailCacheRefreshMode::StaleOrMissing,
                16,
                None,
            )
            .expect("enqueue scoped ready row without blob");

        assert_eq!(enqueue_result.accepted_count, 1);
        let queued_file_ids: Vec<i64> = database
            .list_tasks()
            .expect("list scoped queued tasks")
            .into_iter()
            .filter(|task| task.kind == "thumbnail" && task.priority == THUMBNAIL_BULK_PRIORITY)
            .filter_map(|task| task.file_id)
            .collect();
        assert_eq!(queued_file_ids, vec![missing_blob_file_id]);
        assert!(!queued_file_ids.contains(&cached_file_id));
    }

    #[test]
    fn thumbnail_cache_explicit_enqueue_requeues_ready_current_row_without_blob() {
        let database = migrated_database();
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-explicit-ready-no-blob");
        let missing_blob_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "ready-without-blob.jpg",
            Some("candidate-explicit-ready-without-blob"),
            "ready",
            false,
            None,
            None,
        );
        let cached_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "ready-with-blob.jpg",
            Some("candidate-explicit-ready-with-blob"),
            "ready",
            true,
            Some(64),
            None,
        );

        let stats = database
            .get_thumbnail_cache_stats_for_file_ids(&[missing_blob_file_id, cached_file_id])
            .expect("get explicit stats");
        assert_eq!(stats.cached_count, 1);
        assert_eq!(stats.missing_count, 1);
        assert_eq!(stats.pending_candidate_count, 1);

        let enqueue_result = database
            .enqueue_thumbnail_cache_tasks_for_file_ids(
                &[missing_blob_file_id, cached_file_id],
                ThumbnailCacheRefreshMode::StaleOrMissing,
                16,
            )
            .expect("enqueue explicit ready row without blob");

        assert_eq!(enqueue_result.accepted_count, 1);
        let queued_file_ids: Vec<i64> = database
            .list_tasks()
            .expect("list explicit queued tasks")
            .into_iter()
            .filter(|task| task.kind == "thumbnail" && task.priority == THUMBNAIL_BULK_PRIORITY)
            .filter_map(|task| task.file_id)
            .collect();
        assert_eq!(queued_file_ids, vec![missing_blob_file_id]);
        assert!(!queued_file_ids.contains(&cached_file_id));
    }

    #[test]
    fn thumbnail_cache_explicit_file_ids_skip_non_media_candidates() {
        let database = migrated_database();
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-explicit-non-media");
        let image_file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "image.jpg",
            Some("candidate-explicit-image"),
            "absent",
            false,
            None,
            None,
        );
        let no_media_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "not-classified.bin".to_string(),
                ext: ".bin".to_string(),
                size: 4096,
                mtime: 124,
                ctime: None,
                file_key: Some("candidate-explicit-unclassified".to_string()),
            })
            .expect("insert unclassified file");
        let document_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "document.pdf".to_string(),
                ext: ".pdf".to_string(),
                size: 4096,
                mtime: 125,
                ctime: None,
                file_key: Some("candidate-explicit-document".to_string()),
            })
            .expect("insert document file");
        database
            .upsert_media_kind(document_file_id, "document")
            .expect("insert unsupported media kind");

        let enqueue_result = database
            .enqueue_thumbnail_cache_tasks_for_file_ids(
                &[image_file_id, no_media_file_id, document_file_id],
                ThumbnailCacheRefreshMode::StaleOrMissing,
                16,
            )
            .expect("enqueue explicit file ids");

        assert_eq!(enqueue_result.accepted_count, 1);
        let queued_file_ids: Vec<i64> = database
            .list_tasks()
            .expect("list explicit queued tasks")
            .into_iter()
            .filter(|task| task.kind == "thumbnail" && task.priority == THUMBNAIL_BULK_PRIORITY)
            .filter_map(|task| task.file_id)
            .collect();
        assert_eq!(queued_file_ids, vec![image_file_id]);
    }

    #[test]
    fn thumbnail_cache_explicit_reenqueue_reports_pending_bulk_work() {
        let database = migrated_database();
        let (root_id, folder_id) =
            seed_thumbnail_cache_scope_fixture(&database, "cache-explicit-reenqueue");
        let file_id = seed_thumbnail_cache_candidate(
            &database,
            root_id,
            folder_id,
            "pending.jpg",
            Some("candidate-explicit-pending"),
            "absent",
            false,
            None,
            None,
        );

        let first = database
            .enqueue_thumbnail_cache_tasks_for_file_ids(
                &[file_id],
                ThumbnailCacheRefreshMode::StaleOrMissing,
                16,
            )
            .expect("enqueue explicit file id");
        assert_eq!(first.accepted_count, 1);
        let first_task_id = first.task_id_to_wake.expect("first bulk task id");

        let repeated = database
            .enqueue_thumbnail_cache_tasks_for_file_ids(
                &[file_id],
                ThumbnailCacheRefreshMode::StaleOrMissing,
                16,
            )
            .expect("reenqueue explicit file id");

        assert_eq!(repeated.accepted_count, 0);
        assert_eq!(repeated.task_id_to_wake, Some(first_task_id));
        assert_eq!(
            database
                .list_tasks()
                .expect("list tasks")
                .into_iter()
                .filter(|task| {
                    task.kind == "thumbnail"
                        && task.file_id == Some(file_id)
                        && task.priority == THUMBNAIL_BULK_PRIORITY
                })
                .count(),
            1
        );
    }

    #[test]
    fn thumbnail_request_waits_for_concurrent_writer_instead_of_returning_locked() {
        let db_dir = unique_db_temp_dir("thumbnail-request-lock");
        std::fs::create_dir_all(&db_dir).expect("create temp db dir");
        let db_path = db_dir.join("megle.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "locked-request.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: Some("locked-request".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");

        let blocker = Database::open(&db_path).expect("open blocker database");
        let (blocker_ready_sender, blocker_ready_receiver) = std::sync::mpsc::channel();
        let blocker_thread = std::thread::spawn(move || {
            let transaction =
                Transaction::new_unchecked(&blocker.connection, TransactionBehavior::Immediate)
                    .expect("begin immediate blocker transaction");
            blocker_ready_sender.send(()).expect("signal blocker ready");
            std::thread::sleep(std::time::Duration::from_millis(200));
            transaction.commit().expect("commit blocker transaction");
        });
        blocker_ready_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("blocker should acquire write lock");

        let (request_sender, request_receiver) = std::sync::mpsc::channel();
        let request_thread = std::thread::spawn(move || {
            let result = database.request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            );
            request_sender
                .send(result)
                .expect("send thumbnail request result");
        });

        match request_receiver.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(result) => {
                panic!("thumbnail request returned before writer released lock: {result:?}")
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("thumbnail request channel closed unexpectedly: {error}"),
        }

        blocker_thread.join().expect("join blocker thread");
        let request = request_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("thumbnail request should finish after writer releases lock")
            .expect("thumbnail request should not fail with database is locked");
        request_thread.join().expect("join request thread");

        assert_eq!(request.thumbnail.state, "queued");
        assert!(request.queued);

        let _ = std::fs::remove_dir_all(db_dir);
    }

    #[test]
    fn upsert_file_reuses_reliable_identity_across_path_changes_and_keeps_thumbnail() {
        let database = migrated_database();
        let (root_id, root_folder_id) = seed_media_page_fixture(&database);
        let source_folder = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "source".to_string(),
                path_hash: "source-hash".to_string(),
                mtime: Some(10),
            })
            .expect("insert source folder");
        let target_folder = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "target".to_string(),
                path_hash: "target-hash".to_string(),
                mtime: Some(11),
            })
            .expect("insert target folder");
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id: source_folder,
                name: "before.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: Some("stable-identity".to_string()),
            })
            .expect("insert source file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "ready".to_string(),
                cache_key: None,
                width: Some(320),
                height: Some(160),
                byte_size: Some(42),
                error: None,
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("seed ready thumbnail");
        database
            .upsert_thumb_blob(ThumbBlobRecord {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                data: test_webp_bytes(),
                width: 320,
                height: 160,
                byte_size: 42,
                output_format: crate::thumbnails::GENERATED_FORMAT.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .expect("seed thumb blob");

        let reused_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id: target_folder,
                name: "after.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: Some("stable-identity".to_string()),
            })
            .expect("upsert moved file");

        assert_eq!(reused_file_id, file_id);
        let media = database
            .get_media(file_id)
            .expect("get media")
            .expect("media exists");
        assert_eq!(media.folder_id, target_folder);
        assert_eq!(media.name, "after.jpg");
        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "ready");
        assert!(database
            .get_thumb_blob(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumb blob")
            .is_some());
        let old_path_rows: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM files WHERE folder_id = ?1 AND name = ?2",
                (source_folder, "before.jpg"),
                |row| row.get(0),
            )
            .expect("count old path rows");
        assert_eq!(old_path_rows, 0);
    }

    #[test]
    fn upsert_file_without_file_key_does_not_reuse_path_changes() {
        let database = migrated_database();
        let (root_id, root_folder_id) = seed_media_page_fixture(&database);
        let source_folder = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "source".to_string(),
                path_hash: "source-no-key".to_string(),
                mtime: Some(10),
            })
            .expect("insert source folder");
        let target_folder = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(root_folder_id),
                name: "target".to_string(),
                path_hash: "target-no-key".to_string(),
                mtime: Some(11),
            })
            .expect("insert target folder");
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id: source_folder,
                name: "before.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: None,
            })
            .expect("insert source file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");

        let moved_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id: target_folder,
                name: "after.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: None,
            })
            .expect("upsert moved file");

        assert_ne!(moved_file_id, file_id);
    }

    #[test]
    fn upsert_file_identity_change_resets_ready_thumbnail_and_clears_blob() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "changed.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: Some("before-change".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "ready".to_string(),
                cache_key: None,
                width: Some(320),
                height: Some(160),
                byte_size: Some(42),
                error: None,
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("seed ready thumbnail");
        database
            .upsert_thumb_blob(ThumbBlobRecord {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                data: test_webp_bytes(),
                width: 320,
                height: 160,
                byte_size: 42,
                output_format: crate::thumbnails::GENERATED_FORMAT.to_string(),
                created_at: 1,
                updated_at: 1,
            })
            .expect("seed thumb blob");

        let updated_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "changed.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 8192,
                mtime: 456,
                ctime: None,
                file_key: Some("after-change".to_string()),
            })
            .expect("update file identity");

        assert_eq!(updated_file_id, file_id);
        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "pending");
        assert_eq!(thumbnail.cache_key, None);
        assert!(database
            .get_thumb_blob(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumb blob")
            .is_none());
    }

    #[test]
    fn thumbnail_task_attempt_state_update_waits_for_concurrent_writer_instead_of_returning_locked()
    {
        let db_dir = unique_db_temp_dir("thumbnail-attempt-lock");
        std::fs::create_dir_all(&db_dir).expect("create temp db dir");
        let db_path = db_dir.join("megle.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "locked-attempt.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 4096,
                mtime: 123,
                ctime: None,
                file_key: Some("locked-attempt".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");

        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        let task_id = request.task_id.expect("thumbnail task id");
        let attempt_generation = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get thumbnail source")
            .expect("thumbnail source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .mark_thumbnail_task_running_for_attempt(
                task_id,
                attempt_generation,
                &source_fingerprint,
            )
            .expect("mark thumbnail running");

        let blocker = Database::open(&db_path).expect("open blocker database");
        let (blocker_ready_sender, blocker_ready_receiver) = std::sync::mpsc::channel();
        let (blocker_release_sender, blocker_release_receiver) = std::sync::mpsc::channel::<()>();
        let blocker_thread = std::thread::spawn(move || {
            let transaction =
                Transaction::new_unchecked(&blocker.connection, TransactionBehavior::Immediate)
                    .expect("begin immediate blocker transaction");
            blocker_ready_sender.send(()).expect("signal blocker ready");
            blocker_release_receiver
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("receive blocker release");
            transaction.commit().expect("commit blocker transaction");
        });
        blocker_ready_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("blocker should acquire write lock");

        let updater = Database::open(&db_path).expect("open updater database");
        let (update_sender, update_receiver) = std::sync::mpsc::channel();
        let update_thread = std::thread::spawn(move || {
            let result = updater
                .upsert_thumbnail_state_if_source_fingerprint_and_task_attempt_current(
                    ThumbnailStateUpsert {
                        file_id,
                        profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                        state: "failed".to_string(),
                        cache_key: None,
                        width: None,
                        height: None,
                        byte_size: None,
                        error: Some("locked failure".to_string()),
                        source_fingerprint: Some(source_fingerprint.clone()),
                    },
                    &source_fingerprint,
                    task_id,
                    attempt_generation,
                );
            update_sender.send(result).expect("send update result");
        });

        match update_receiver.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(result) => {
                panic!("thumbnail attempt update returned before writer released lock: {result:?}")
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("thumbnail attempt update channel closed unexpectedly: {error}"),
        }

        blocker_release_sender
            .send(())
            .expect("release blocker transaction");
        blocker_thread.join().expect("join blocker thread");
        let updated = update_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("thumbnail attempt update should finish after writer releases lock")
            .expect("thumbnail attempt update should not fail with database is locked");
        update_thread.join().expect("join update thread");

        assert!(updated);

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "failed");
        assert_eq!(thumbnail.error.as_deref(), Some("locked failure"));

        let _ = std::fs::remove_dir_all(db_dir);
    }

    #[test]
    fn thumbnail_request_invalidates_ready_state_when_source_identity_changes() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "identity.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("identity-a".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        let first = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        let first_task_id = first.task_id.expect("task id");
        database
            .mark_task_running_current_attempt_for_test(first_task_id)
            .expect("mark running");
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "ready".to_string(),
                cache_key: Some("aa/bb/stale.webp".to_string()),
                width: Some(320),
                height: Some(320),
                byte_size: Some(64),
                error: None,
                source_fingerprint: Some(
                    "stale-source-fingerprint-that-must-not-match".to_string(),
                ),
            })
            .expect("mark stale ready");
        database
            .mark_task_succeeded_current_attempt_for_test(first_task_id)
            .expect("mark succeeded");

        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request after source change");

        assert_eq!(request.thumbnail.state, "queued");
        assert!(request.queued);
        assert_ne!(request.task_id, Some(first_task_id));
        assert!(request.thumbnail.cache_key.is_none());
    }

    #[test]
    fn failed_thumbnail_without_source_fingerprint_is_regeneratable() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "failed-null.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("identity-a".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "failed".to_string(),
                cache_key: None,
                width: None,
                height: None,
                byte_size: None,
                error: Some("decode failed".to_string()),
                source_fingerprint: None,
            })
            .expect("insert failed thumbnail without fingerprint");

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "pending");
        assert_eq!(thumbnail.error, None);

        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        assert_eq!(request.thumbnail.state, "queued");
        assert!(request.queued);
    }

    #[test]
    fn failed_thumbnail_with_stale_source_fingerprint_is_regeneratable() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "failed-stale.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("identity-a".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "failed".to_string(),
                cache_key: None,
                width: None,
                height: None,
                byte_size: None,
                error: Some("decode failed".to_string()),
                source_fingerprint: Some(
                    "stale-source-fingerprint-that-must-not-match".to_string(),
                ),
            })
            .expect("insert stale failed thumbnail");

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "pending");
        assert_eq!(thumbnail.error, None);

        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        assert_eq!(request.thumbnail.state, "queued");
        assert!(request.queued);
    }

    #[test]
    fn failed_thumbnail_with_current_source_fingerprint_remains_terminal() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "failed-current.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("identity-a".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get thumbnail source")
            .expect("thumbnail source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "failed".to_string(),
                cache_key: None,
                width: None,
                height: None,
                byte_size: None,
                error: Some("decode failed".to_string()),
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("insert current failed thumbnail");

        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request failed thumbnail");
        assert_eq!(request.thumbnail.state, "failed");
        assert_eq!(request.thumbnail.error.as_deref(), Some("decode failed"));
        assert!(!request.queued);
    }

    #[test]
    fn file_upsert_marks_non_pending_thumbnail_stale_when_source_identity_changes() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "source-change.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("source-a".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "ready".to_string(),
                cache_key: Some("aa/bb/source-change.webp".to_string()),
                width: Some(320),
                height: Some(320),
                byte_size: Some(64),
                error: None,
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("mark ready");

        database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "source-change.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 200,
                mtime: 20,
                ctime: None,
                file_key: Some("source-b".to_string()),
            })
            .expect("update source identity");

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "pending");
        assert_eq!(thumbnail.cache_key, None);
        assert_eq!(thumbnail.source_fingerprint, None);
    }

    #[test]
    fn thumbnail_request_invalidates_skipped_small_state_when_source_identity_changes() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "small-then-large.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("small-a".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        database
            .connection
            .execute(
                "UPDATE media SET width = 128, height = 128, metadata_status = 'ready' WHERE file_id = ?1",
                [file_id],
            )
            .expect("set ready small metadata");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get thumbnail source")
            .expect("thumbnail source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "skipped_small".to_string(),
                cache_key: None,
                width: None,
                height: None,
                byte_size: None,
                error: None,
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("worker marks skipped small");

        database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "small-then-large.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 10_000,
                mtime: 20,
                ctime: None,
                file_key: Some("small-b".to_string()),
            })
            .expect("update source identity");
        database
            .connection
            .execute(
                "UPDATE media SET width = 640, height = 480, metadata_status = 'ready' WHERE file_id = ?1",
                [file_id],
            )
            .expect("set ready large metadata");
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request changed source thumbnail");

        assert_eq!(request.thumbnail.state, "queued");
        assert!(request.queued);
    }

    #[test]
    fn thumbnail_request_does_not_regress_ready_state_when_worker_wins_race() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "race.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("race-a".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");

        let first = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        let task_id = first.task_id.expect("task id");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark running");
        let fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "ready".to_string(),
                cache_key: Some("aa/bb/race.webp".to_string()),
                width: Some(320),
                height: Some(320),
                byte_size: Some(64),
                error: None,
                source_fingerprint: Some(fingerprint),
            })
            .expect("worker marks ready");

        let second = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request after ready");

        assert_eq!(second.thumbnail.state, "ready");
        assert!(!second.queued);
        assert_eq!(database.list_tasks().expect("list tasks").len(), 1);
    }

    #[test]
    fn thumbnail_request_reusing_pending_task_preserves_in_progress_thumbnail_state() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "pending-reuse.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("pending-reuse".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");

        let first = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Background,
            )
            .expect("queue thumbnail");
        let task_id = first.task_id.expect("task id");
        let fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "pending".to_string(),
                cache_key: None,
                width: None,
                height: None,
                byte_size: None,
                error: None,
                source_fingerprint: Some(fingerprint.clone()),
            })
            .expect("mark thumbnail pending");

        let second = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("reuse pending task");

        assert_eq!(second.task_id, Some(task_id));
        assert!(!second.queued);
        assert_eq!(second.thumbnail.state, "pending");
        assert_eq!(
            second.thumbnail.source_fingerprint.as_deref(),
            Some(fingerprint.as_str())
        );

        let stored = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(stored.state, "pending");
        assert_eq!(
            stored.source_fingerprint.as_deref(),
            Some(fingerprint.as_str())
        );

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "pending");
        assert_eq!(task.priority, THUMBNAIL_VISIBLE_PRIORITY);
    }

    #[test]
    fn thumbnail_request_reusing_running_task_preserves_in_progress_thumbnail_state() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "running-reuse.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("running-reuse".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");

        let first = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("queue thumbnail");
        let task_id = first.task_id.expect("task id");
        let fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "pending".to_string(),
                cache_key: None,
                width: None,
                height: None,
                byte_size: None,
                error: None,
                source_fingerprint: Some(fingerprint.clone()),
            })
            .expect("mark thumbnail pending");
        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark task running");

        let second = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("reuse running task");

        assert_eq!(second.task_id, Some(task_id));
        assert!(!second.queued);
        assert_eq!(second.thumbnail.state, "pending");
        assert_eq!(
            second.thumbnail.source_fingerprint.as_deref(),
            Some(fingerprint.as_str())
        );

        let stored = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(stored.state, "pending");
        assert_eq!(
            stored.source_fingerprint.as_deref(),
            Some(fingerprint.as_str())
        );

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "running");
        assert_eq!(task.priority, THUMBNAIL_VISIBLE_PRIORITY);
    }

    #[test]
    fn thumbnail_scope_sync_rebalances_pending_priorities_for_a_root() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let visible_folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(folder_id),
                name: "visible".to_string(),
                path_hash: "visible-folder".to_string(),
                mtime: Some(11),
            })
            .expect("insert visible folder");

        let selected_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "selected.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("selected-a".to_string()),
            })
            .expect("insert selected file");
        let visible_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "visible.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 11,
                ctime: None,
                file_key: Some("visible-a".to_string()),
            })
            .expect("insert visible file");
        let ahead_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id: visible_folder_id,
                name: "ahead.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 12,
                ctime: None,
                file_key: Some("ahead-a".to_string()),
            })
            .expect("insert ahead file");
        let stale_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id: visible_folder_id,
                name: "stale.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 13,
                ctime: None,
                file_key: Some("stale-a".to_string()),
            })
            .expect("insert stale file");
        let running_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id: visible_folder_id,
                name: "running.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 14,
                ctime: None,
                file_key: Some("running-a".to_string()),
            })
            .expect("insert running file");
        for file_id in [
            selected_file_id,
            visible_file_id,
            ahead_file_id,
            stale_file_id,
            running_file_id,
        ] {
            database
                .upsert_media_kind(file_id, "image")
                .expect("insert media kind");
        }

        let selected_task = database
            .request_thumbnail_task(
                selected_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Selected,
            )
            .expect("queue selected thumbnail");
        let visible_task = database
            .request_thumbnail_task(
                visible_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("queue visible thumbnail");
        let ahead_task = database
            .request_thumbnail_task(
                ahead_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Ahead,
            )
            .expect("queue ahead thumbnail");
        let stale_task = database
            .request_thumbnail_task(
                stale_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("queue stale thumbnail");
        let running_task = database
            .request_thumbnail_task(
                running_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("queue running thumbnail");
        let running_task_id = running_task.task_id.expect("running task id");
        database
            .mark_task_running_current_attempt_for_test(running_task_id)
            .expect("mark thumbnail running");

        let updated = database
            .sync_thumbnail_priority_scope(
                root_id,
                &[selected_file_id],
                &[visible_file_id],
                &[ahead_file_id],
            )
            .expect("sync thumbnail scope");
        assert_eq!(updated, 2);

        let selected_task = database
            .get_task(selected_task.task_id.expect("selected task id"))
            .expect("get selected task")
            .expect("selected task exists");
        let visible_task = database
            .get_task(visible_task.task_id.expect("visible task id"))
            .expect("get visible task")
            .expect("visible task exists");
        let ahead_task = database
            .get_task(ahead_task.task_id.expect("ahead task id"))
            .expect("get ahead task")
            .expect("ahead task exists");
        let stale_task = database
            .get_task(stale_task.task_id.expect("stale task id"))
            .expect("get stale task")
            .expect("stale task exists");
        let running_task = database
            .get_task(running_task_id)
            .expect("get running task")
            .expect("running task exists");

        assert_eq!(selected_task.priority, THUMBNAIL_SELECTED_PRIORITY);
        assert_eq!(visible_task.priority, THUMBNAIL_VISIBLE_PRIORITY);
        assert_eq!(ahead_task.priority, THUMBNAIL_AHEAD_PRIORITY);
        assert_eq!(stale_task.priority, THUMBNAIL_BACKGROUND_PRIORITY);
        assert_eq!(running_task.priority, THUMBNAIL_BACKGROUND_PRIORITY);
    }

    #[test]
    fn thumbnail_scope_sync_promotes_new_viewport_task_and_demotes_stale_task_in_one_call() {
        // Regression test for the two-query split in
        // sync_thumbnail_priority_scope_with_promotions: query 1 (elevated
        // tasks, demotion candidates) only sees tasks whose priority is
        // already above background, so a task that starts at background
        // priority and only enters scope on this call must be picked up by
        // query 2 (scoped file ids) or the promotion is silently dropped.
        // This test exercises both a promotion and a demotion in the same
        // sync call so the union (and its de-dup by task id) is proven.
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);

        let stale_elevated_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "stale-elevated.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 20,
                ctime: None,
                file_key: Some("stale-elevated-a".to_string()),
            })
            .expect("insert stale elevated file");
        let new_viewport_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "new-viewport.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 21,
                ctime: None,
                file_key: Some("new-viewport-a".to_string()),
            })
            .expect("insert new viewport file");
        for file_id in [stale_elevated_file_id, new_viewport_file_id] {
            database
                .upsert_media_kind(file_id, "image")
                .expect("insert media kind");
        }

        // Elevated above background from a previous scroll; this call's
        // scope no longer includes it, so it must be demoted.
        let stale_task = database
            .request_thumbnail_task(
                stale_elevated_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("queue stale elevated thumbnail");
        // At (or below) background priority going in; this call's scope
        // promotes it to selected. Query 1 alone would never see this task.
        let new_viewport_task = database
            .request_thumbnail_task(
                new_viewport_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Background,
            )
            .expect("queue new viewport thumbnail");

        let result = database
            .sync_thumbnail_priority_scope_with_promotions(
                root_id,
                &[new_viewport_file_id],
                &[],
                &[],
                // Retain the stale file so it demotes to background (kept warm),
                // exercising the re-rank path this test asserts.
                &[stale_elevated_file_id],
                ThumbnailScopeLeaverPolicy::DemoteToBulk,
            )
            .expect("sync thumbnail scope");

        assert_eq!(result.updated, 2);
        assert_eq!(
            result.promoted_pending_task_ids,
            vec![new_viewport_task.task_id.expect("new viewport task id")]
        );

        let stale_task = database
            .get_task(stale_task.task_id.expect("stale task id"))
            .expect("get stale task")
            .expect("stale task exists");
        let new_viewport_task = database
            .get_task(new_viewport_task.task_id.expect("new viewport task id"))
            .expect("get new viewport task")
            .expect("new viewport task exists");

        assert_eq!(stale_task.priority, THUMBNAIL_BACKGROUND_PRIORITY);
        assert_eq!(new_viewport_task.priority, THUMBNAIL_SELECTED_PRIORITY);
    }

    #[test]
    fn thumbnail_scope_leaver_is_cancelled_in_rendered_only_but_retained_or_bulk_otherwise() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);

        let make_visible_task = |name: &str, key: &str| {
            let file_id = database
                .upsert_file(FileUpsert {
                    root_id,
                    folder_id,
                    name: name.to_string(),
                    ext: ".jpg".to_string(),
                    size: 100,
                    mtime: 20,
                    ctime: None,
                    file_key: Some(key.to_string()),
                })
                .expect("insert file");
            database
                .upsert_media_kind(file_id, "image")
                .expect("insert media kind");
            let task = database
                .request_thumbnail_task(
                    file_id,
                    crate::thumbnails::GRID_320_PROFILE,
                    ThumbnailTaskPriority::Visible,
                )
                .expect("queue visible thumbnail");
            (file_id, task.task_id.expect("task id"))
        };

        // Two visible tasks for the renderedOnly check.
        let (leaver_file, leaver_task) = make_visible_task("leaver.jpg", "leaver-a");
        let (retained_file, retained_task) = make_visible_task("retained.jpg", "retained-a");

        // renderedOnly: an out-of-scope, non-retained leaver's pending task is
        // cancelled; a retained one is only demoted to background.
        database
            .sync_thumbnail_priority_scope_with_promotions(
                root_id,
                &[],
                &[],
                &[],
                &[retained_file],
                ThumbnailScopeLeaverPolicy::Cancel,
            )
            .expect("sync renderedOnly scope");
        assert_eq!(
            database.get_task(leaver_task).unwrap().unwrap().status,
            "cancelled",
            "non-retained leaver cancelled in renderedOnly"
        );
        assert_eq!(
            database.get_task(retained_task).unwrap().unwrap().priority,
            THUMBNAIL_BACKGROUND_PRIORITY,
            "retained leaver demoted to background, not cancelled"
        );
        assert_eq!(
            database.get_task(retained_task).unwrap().unwrap().status,
            "pending",
            "retained leaver stays pending"
        );

        // allPersist: a FRESH out-of-scope leaver is demoted to bulk, not
        // cancelled. (Created after the renderedOnly sync so that pass doesn't
        // touch it.)
        let (bulk_file, bulk_task) = make_visible_task("bulk.jpg", "bulk-a");
        database
            .sync_thumbnail_priority_scope_with_promotions(
                root_id,
                &[],
                &[],
                &[],
                &[],
                ThumbnailScopeLeaverPolicy::DemoteToBulk,
            )
            .expect("sync allPersist scope");
        let bulk = database.get_task(bulk_task).unwrap().unwrap();
        assert_eq!(bulk.status, "pending", "allPersist leaver not cancelled");
        assert_eq!(
            bulk.priority, THUMBNAIL_BULK_PRIORITY,
            "allPersist leaver demoted to bulk"
        );
        // The file ids kept for reference; no further assertion needed.
        let _ = bulk_file;
        let _ = leaver_file;
    }

    #[test]
    fn repeated_visible_thumbnail_request_moves_pending_task_to_front_of_same_priority_queue() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let first_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "first-visible.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("first-visible".to_string()),
            })
            .expect("insert first visible file");
        let second_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "second-visible.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 11,
                ctime: None,
                file_key: Some("second-visible".to_string()),
            })
            .expect("insert second visible file");
        for file_id in [first_file_id, second_file_id] {
            database
                .upsert_media_kind(file_id, "image")
                .expect("insert media kind");
        }

        let first_task_id = database
            .request_thumbnail_task(
                first_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("queue first visible thumbnail")
            .task_id
            .expect("first task id");
        let second_task = database
            .request_thumbnail_task(
                second_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("queue second visible thumbnail");
        let second_task_id = second_task.task_id.expect("second task id");

        let repeated = database
            .request_thumbnail_task(
                second_file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("repeat second visible thumbnail");

        assert_eq!(repeated.task_id, Some(second_task_id));
        assert!(
            repeated.should_enqueue,
            "same-priority foreground requests must wake the worker because the visible viewport changed"
        );
        assert_eq!(
            database
                .list_pending_foreground_thumbnail_task_ids()
                .expect("list foreground thumbnails")
                .into_iter()
                .take(2)
                .collect::<Vec<_>>(),
            vec![second_task_id, first_task_id]
        );
    }

    #[test]
    fn skipped_small_requires_ready_metadata_and_upsert_media_resets_dimensions() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "metadata.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: None,
            })
            .expect("insert file");
        database
            .connection
            .execute(
                r#"
                INSERT INTO media(file_id, kind, width, height, metadata_status)
                VALUES (?1, 'image', 128, 128, 'ready')
                "#,
                [file_id],
            )
            .expect("insert stale metadata");
        database
            .upsert_media_kind(file_id, "image")
            .expect("scan upsert media kind");

        let media = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists");
        assert_eq!(media.width, None);
        assert_eq!(media.height, None);
        assert_ne!(media.metadata_status.as_deref(), Some("ready"));

        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        assert_eq!(request.thumbnail.state, "queued");
        assert!(request.queued);
    }

    #[test]
    fn skipped_small_request_queues_worker_instead_of_publishing_terminal_inline() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "small-queued.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("small-queued".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        database
            .connection
            .execute(
                "UPDATE media SET width = 128, height = 128, metadata_status = 'ready' WHERE file_id = ?1",
                [file_id],
            )
            .expect("set small ready metadata");

        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request small thumbnail");

        assert_eq!(request.thumbnail.state, "queued");
        assert!(request.queued);
        let task_id = request.task_id.expect("queued task id");
        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.kind, "thumbnail");
        assert_eq!(task.status, "pending");
        assert_eq!(task.file_id, Some(file_id));
    }

    #[test]
    fn skipped_small_is_requeued_after_metadata_reset_even_when_file_identity_matches() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "same-source-metadata-reset.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("same-source".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        database
            .connection
            .execute(
                "UPDATE media SET width = 128, height = 128, metadata_status = 'ready' WHERE file_id = ?1",
                [file_id],
            )
            .expect("set ready small metadata");

        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get thumbnail source")
            .expect("thumbnail source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        database
            .upsert_thumbnail_state(ThumbnailStateUpsert {
                file_id,
                profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                state: "skipped_small".to_string(),
                cache_key: None,
                width: None,
                height: None,
                byte_size: None,
                error: None,
                source_fingerprint: Some(source_fingerprint),
            })
            .expect("worker marks skipped small");

        database
            .upsert_media_kind(file_id, "image")
            .expect("scan resets media metadata");
        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "pending");

        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail after metadata reset");
        assert_eq!(request.thumbnail.state, "queued");
        assert!(request.queued);
    }

    #[test]
    fn task_scan_progress_defaults_updates_and_rejects_finished_tasks() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create task");

        let pending_task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(pending_task.items_seen, 0);
        assert_eq!(pending_task.items_total, None);
        assert_eq!(pending_task.folders_seen, 0);
        assert_eq!(pending_task.media_files_seen, 0);
        assert_eq!(pending_task.skipped_files, 0);

        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark running");
        database
            .update_task_scan_progress_current_attempt_for_test(
                task_id,
                TaskScanProgress {
                    items_seen: 6,
                    items_total: None,
                    folders_seen: 2,
                    media_files_seen: 3,
                    skipped_files: 1,
                },
            )
            .expect("update progress");

        let running_task = database
            .get_task(task_id)
            .expect("get running task")
            .expect("running task exists");
        assert_eq!(running_task.items_seen, 6);
        assert_eq!(running_task.items_total, None);
        assert_eq!(running_task.folders_seen, 2);
        assert_eq!(running_task.media_files_seen, 3);
        assert_eq!(running_task.skipped_files, 1);

        database
            .mark_task_succeeded_current_attempt_for_test(task_id)
            .expect("mark succeeded");
        let late_update = database.update_task_scan_progress_current_attempt_for_test(
            task_id,
            TaskScanProgress {
                items_seen: 7,
                items_total: None,
                folders_seen: 2,
                media_files_seen: 4,
                skipped_files: 1,
            },
        );
        assert!(late_update
            .expect_err("finished task should reject progress updates")
            .to_string()
            .contains("running"));
    }

    #[test]
    fn task_status_transitions_reject_missing_and_finished_tasks() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");

        let missing_error = database
            .mark_task_running_current_attempt_for_test(task_id + 100)
            .expect_err("missing task should fail");
        assert!(missing_error.to_string().contains("task not found"));

        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark task running");
        database
            .mark_task_succeeded_current_attempt_for_test(task_id)
            .expect("mark task succeeded");
        let rerun_error = database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect_err("finished task should not be rerun");
        assert!(rerun_error.to_string().contains("not pending"));
    }

    #[test]
    fn mark_task_failed_rejects_finished_tasks() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let succeeded_task_id = database
            .create_root_scan_task(root_id)
            .expect("create succeeded task");
        let failed_task_id = database
            .create_root_scan_task(root_id)
            .expect("create failed task");

        database
            .mark_task_running_current_attempt_for_test(succeeded_task_id)
            .expect("mark succeeded task running");
        database
            .mark_task_succeeded_current_attempt_for_test(succeeded_task_id)
            .expect("mark task succeeded");
        let succeeded_error = database
            .mark_task_failed_current_attempt_for_test(succeeded_task_id, "late failure")
            .expect_err("succeeded task should not be marked failed");
        assert!(succeeded_error
            .to_string()
            .contains("not pending or running"));

        database
            .mark_task_failed_current_attempt_for_test(failed_task_id, "first failure")
            .expect("mark task failed");
        let failed_error = database
            .mark_task_failed_current_attempt_for_test(failed_task_id, "second failure")
            .expect_err("failed task should not be marked failed again");
        assert!(failed_error.to_string().contains("not pending or running"));
    }

    #[test]
    fn try_mark_task_failed_reports_transition_ownership_and_publishes_terminal_state() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/FailureOwnership".to_string(),
                display_name: "Failure Ownership".to_string(),
            })
            .expect("add root");
        let failed_task_id = database
            .create_root_scan_task_with_origin(root_id, NotificationOrigin::User)
            .expect("create failed task");
        let failed_attempt = database
            .get_task(failed_task_id)
            .expect("read failed task")
            .expect("failed task exists")
            .attempt_generation;

        assert!(database
            .try_mark_task_failed_for_attempt(
                failed_task_id,
                failed_attempt,
                "owned worker failure",
            )
            .expect("claim failed transition"));
        assert!(database
            .notification_heads_for_test()
            .expect("read failed notification")
            .into_iter()
            .any(|operation| operation.task_id == Some(failed_task_id)
                && operation.status == "failed"));

        let succeeded_task_id = database
            .create_root_scan_task_with_origin(root_id, NotificationOrigin::User)
            .expect("create succeeded task");
        let succeeded_attempt = database
            .get_task(succeeded_task_id)
            .expect("read succeeded task")
            .expect("succeeded task exists")
            .attempt_generation;
        database
            .mark_task_running_for_attempt(succeeded_task_id, succeeded_attempt)
            .expect("mark succeeded task running");
        database
            .mark_task_succeeded_for_attempt(succeeded_task_id, succeeded_attempt)
            .expect("mark task succeeded");

        assert!(!database
            .try_mark_task_failed_for_attempt(
                succeeded_task_id,
                succeeded_attempt,
                "late worker failure",
            )
            .expect("reject late failed transition"));
        let succeeded_task = database
            .get_task(succeeded_task_id)
            .expect("read succeeded task after late failure")
            .expect("succeeded task remains");
        assert_eq!(succeeded_task.status, "succeeded");
        assert_eq!(succeeded_task.error, None);
        assert!(database
            .notification_heads_for_test()
            .expect("read succeeded notification")
            .into_iter()
            .any(|operation| operation.task_id == Some(succeeded_task_id)
                && operation.status == "succeeded"));
    }

    #[test]
    fn cancel_pending_task_marks_cancelled_and_retry_resets_it_to_pending() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");

        let cancelled = database.cancel_task(task_id).expect("cancel task");
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(cancelled.error.as_deref(), Some("cancelled"));

        let retried = database.retry_task(task_id).expect("retry cancelled task");
        assert_eq!(retried.id, task_id);
        assert_eq!(retried.status, "pending");
        assert_eq!(retried.error, None);
        assert_eq!(retried.items_seen, 0);
    }

    #[test]
    fn retry_and_cancel_reject_missing_and_finished_tasks() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");

        let missing_cancel = database
            .cancel_task(task_id + 100)
            .expect_err("missing cancel should fail");
        assert!(missing_cancel.to_string().contains("task not found"));
        let missing_retry = database
            .retry_task(task_id + 100)
            .expect_err("missing retry should fail");
        assert!(missing_retry.to_string().contains("task not found"));

        database
            .mark_task_running_current_attempt_for_test(task_id)
            .expect("mark running");
        database
            .mark_task_succeeded_current_attempt_for_test(task_id)
            .expect("mark succeeded");

        let cancel_succeeded = database
            .cancel_task(task_id)
            .expect_err("succeeded task should reject cancel");
        assert!(cancel_succeeded.to_string().contains("not cancellable"));
        let retry_succeeded = database
            .retry_task(task_id)
            .expect_err("succeeded task should reject retry");
        assert!(retry_succeeded.to_string().contains("not retryable"));

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "succeeded");
    }

    #[test]
    fn retry_thumbnail_task_uses_current_source_without_stale_attempt_fingerprint() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "retry-source.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("retry-source-a".to_string()),
            })
            .expect("insert retry source");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        let task_id = request.task_id.expect("thumbnail task id");
        database
            .mark_thumbnail_task_running_current_attempt_for_test(task_id, "stale-attempt")
            .expect("mark thumbnail running");
        database
            .mark_task_failed_current_attempt_for_test(task_id, "decode failed")
            .expect("mark thumbnail failed");

        database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "retry-source.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 200,
                mtime: 20,
                ctime: None,
                file_key: Some("retry-source-b".to_string()),
            })
            .expect("change retry source identity");
        let current_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get current source")
            .expect("current source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);

        let retried = database.retry_task(task_id).expect("retry thumbnail task");
        assert_eq!(retried.status, "pending");
        assert_eq!(retried.thumbnail_source_fingerprint, None);

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "queued");
        assert_eq!(
            thumbnail.source_fingerprint.as_deref(),
            Some(current_fingerprint.as_str())
        );
    }

    #[test]
    fn old_root_scan_attempt_cannot_publish_after_cancelled_task_is_retried() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        let old_attempt = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;

        database
            .mark_task_running_for_attempt(task_id, old_attempt)
            .expect("mark old attempt running");
        database.cancel_task(task_id).expect("cancel task");
        let retried = database.retry_task(task_id).expect("retry task");
        assert_eq!(retried.status, "pending");
        assert!(retried.attempt_generation > old_attempt);

        let stale_publish = database
            .mark_task_succeeded_for_attempt(task_id, old_attempt)
            .expect_err("old attempt must not publish success");
        assert!(stale_publish.to_string().contains("attempt"));

        let task = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists");
        assert_eq!(task.status, "pending");
        assert_eq!(task.attempt_generation, retried.attempt_generation);
    }

    #[test]
    fn stale_root_scan_attempt_cannot_commit_batch_or_mark_root_scanned_after_retry() {
        let mut database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create root scan task");
        let old_attempt = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;
        database
            .mark_task_running_for_attempt(task_id, old_attempt)
            .expect("mark old attempt running");
        database.cancel_task(task_id).expect("cancel task");
        database.retry_task(task_id).expect("retry task");

        let batch_error = database
            .commit_scan_batch_for_task_attempt(
                ScanWriteBatch {
                    folders: vec![FolderUpsert {
                        root_id,
                        parent_id: None,
                        name: String::new(),
                        path_hash: "stale-root".to_string(),
                        mtime: Some(1),
                    }],
                    files: Vec::new(),
                    scan_generation: None,
                },
                task_id,
                old_attempt,
            )
            .expect_err("stale attempt must not commit batch");
        assert!(batch_error.to_string().contains("attempt"));

        let root = database.get_root(root_id).expect("get root").expect("root");
        assert_eq!(root.root_folder_id, None);

        let scanned_error = database
            .mark_root_scanned_for_task_attempt(root_id, task_id, old_attempt)
            .expect_err("stale attempt must not mark root scanned");
        assert!(scanned_error.to_string().contains("attempt"));
        let root = database.get_root(root_id).expect("get root").expect("root");
        assert_eq!(root.last_scan_at, None);
    }

    #[test]
    fn stale_thumbnail_attempt_cannot_publish_current_source_after_retry() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "stale-thumbnail-publish.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("same-source".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        let task_id = request.task_id.expect("task id");
        let old_attempt = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;
        database
            .mark_thumbnail_task_running_for_attempt(task_id, old_attempt, &source_fingerprint)
            .expect("mark old thumbnail running");
        database.cancel_task(task_id).expect("cancel task");
        database.retry_task(task_id).expect("retry task");

        let published = database
            .upsert_thumbnail_state_if_source_fingerprint_and_task_attempt_current(
                ThumbnailStateUpsert {
                    file_id,
                    profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                    state: "ready".to_string(),
                    cache_key: Some("aa/bb/stale.webp".to_string()),
                    width: Some(320),
                    height: Some(320),
                    byte_size: Some(64),
                    error: None,
                    source_fingerprint: Some(source_fingerprint.clone()),
                },
                &source_fingerprint,
                task_id,
                old_attempt,
            )
            .expect("guard stale publish");
        assert!(!published);

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "queued");
        assert_eq!(thumbnail.cache_key, None);
    }

    #[test]
    fn duplicate_thumbnail_claim_for_same_attempt_and_source_is_idempotent() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "duplicate-thumbnail-claim.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("duplicate-claim-source".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        let task_id = request.task_id.expect("task id");
        let attempt_generation = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;

        database
            .mark_thumbnail_task_running_for_attempt(
                task_id,
                attempt_generation,
                &source_fingerprint,
            )
            .expect("first claim marks thumbnail running");
        database
            .mark_thumbnail_task_running_for_attempt(
                task_id,
                attempt_generation,
                &source_fingerprint,
            )
            .expect("duplicate claim for same source and attempt is benign");

        let task = database
            .get_task(task_id)
            .expect("get task after duplicate claim")
            .expect("task exists after duplicate claim");
        assert_eq!(task.status, "running");
        assert_eq!(
            task.thumbnail_source_fingerprint.as_deref(),
            Some(source_fingerprint.as_str())
        );
        assert_eq!(task.error, None);
    }

    #[test]
    fn stale_thumbnail_attempt_cannot_publish_failure_after_retry() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "stale-failure-publish.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("failure-source".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        let task_id = request.task_id.expect("task id");
        let old_attempt = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;
        database
            .mark_thumbnail_task_running_for_attempt(task_id, old_attempt, &source_fingerprint)
            .expect("mark old thumbnail running");
        assert!(database
            .task_attempt_is_current(task_id, old_attempt)
            .expect("old attempt current before retry"));
        database.cancel_task(task_id).expect("cancel task");
        database.retry_task(task_id).expect("retry task");

        let published = database
            .publish_thumbnail_failure_for_attempted_source_for_task_attempt(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                Some(source_fingerprint.as_str()),
                "late decode failure",
                "permanent",
                task_id,
                old_attempt,
            )
            .expect("guard stale failure publish");
        assert!(!published);

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "queued");
        assert_eq!(thumbnail.error, None);
        assert_eq!(thumbnail.cache_key, None);
    }

    #[test]
    fn publish_thumbnail_failure_writes_failure_kind() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "transient-failure.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("transient-failure-source".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        let source_fingerprint = database
            .get_thumbnail_source(file_id)
            .expect("get source")
            .expect("source exists")
            .source_fingerprint(crate::thumbnails::GRID_320_PROFILE);
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        let task_id = request.task_id.expect("task id");
        let attempt_generation = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;
        database
            .mark_thumbnail_task_running_for_attempt(
                task_id,
                attempt_generation,
                &source_fingerprint,
            )
            .expect("mark thumbnail running");

        let published = database
            .publish_thumbnail_failure_for_attempted_source_for_task_attempt(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                Some(source_fingerprint.as_str()),
                "thumbnail decode failed: 找不到网络路径。 (os error 53)",
                "transient",
                task_id,
                attempt_generation,
            )
            .expect("publish failure");
        assert!(published);

        let (state, failure_kind): (String, Option<String>) = database
            .connection
            .query_row(
                "SELECT state, failure_kind FROM thumbs WHERE file_id = ?1 AND profile = ?2",
                (file_id, crate::thumbnails::GRID_320_PROFILE),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query thumbs row");
        assert_eq!(state, "failed");
        assert_eq!(failure_kind.as_deref(), Some("transient"));
    }

    #[test]
    fn stale_thumbnail_attempt_cannot_reset_after_retry() {
        let database = migrated_database();
        let (root_id, folder_id) = seed_media_page_fixture(&database);
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "stale-reset.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 10,
                ctime: None,
                file_key: Some("reset-source".to_string()),
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media");
        let request = database
            .request_thumbnail_task(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                ThumbnailTaskPriority::Visible,
            )
            .expect("request thumbnail");
        let task_id = request.task_id.expect("task id");
        let old_attempt = database
            .get_task(task_id)
            .expect("get task")
            .expect("task exists")
            .attempt_generation;
        database
            .mark_thumbnail_task_running_for_attempt(task_id, old_attempt, "old-source")
            .expect("mark old thumbnail running");
        assert!(database
            .task_attempt_is_current(task_id, old_attempt)
            .expect("old attempt current before retry"));
        database.cancel_task(task_id).expect("cancel task");
        database.retry_task(task_id).expect("retry task");

        let reset = database
            .reset_thumbnail_after_stale_source_for_task_attempt(
                file_id,
                crate::thumbnails::GRID_320_PROFILE,
                "late stale source",
                task_id,
                old_attempt,
            )
            .expect("guard stale reset");
        assert!(!reset);

        let thumbnail = database
            .get_thumbnail(file_id, crate::thumbnails::GRID_320_PROFILE)
            .expect("get thumbnail")
            .expect("thumbnail exists");
        assert_eq!(thumbnail.state, "queued");
        assert_eq!(thumbnail.error, None);
        assert_ne!(thumbnail.source_fingerprint, None);
    }

    #[test]
    fn recovery_resets_running_root_scans_and_lists_pending_root_scans_in_scheduler_order() {
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");

        let low_priority_pending = database
            .create_root_scan_task(root_id)
            .expect("create low priority pending");
        let stale_running = database
            .create_root_scan_task(root_id)
            .expect("create stale running");
        let failed = database
            .create_root_scan_task(root_id)
            .expect("create failed");
        let succeeded = database
            .create_root_scan_task(root_id)
            .expect("create succeeded");
        let high_priority_pending = database
            .create_root_scan_task(root_id)
            .expect("create high priority pending");

        database
            .connection
            .execute(
                r#"
                INSERT INTO tasks(kind, priority, status, root_id, file_id, created_at, updated_at, error)
                VALUES ('thumbnail', 100, 'pending', NULL, NULL, 1, 1, NULL)
                "#,
                [],
            )
            .expect("insert non-root pending task");

        database
            .mark_task_running_current_attempt_for_test(stale_running)
            .expect("mark stale running");
        database
            .connection
            .execute(
                "UPDATE tasks SET error = 'interrupted' WHERE id = ?1",
                [stale_running],
            )
            .expect("set stale running error");
        database
            .mark_task_failed_current_attempt_for_test(failed, "scan failed")
            .expect("mark failed");
        database
            .mark_task_running_current_attempt_for_test(succeeded)
            .expect("mark succeeded running");
        database
            .mark_task_succeeded_current_attempt_for_test(succeeded)
            .expect("mark succeeded");
        database
            .connection
            .execute(
                "UPDATE tasks SET priority = 10, created_at = 0 WHERE id = ?1",
                [high_priority_pending],
            )
            .expect("raise pending priority");

        let reset_count = database
            .reset_running_root_scan_tasks_for_recovery()
            .expect("reset running root scan tasks");
        assert_eq!(reset_count, 1);

        let stale_task = database
            .get_task(stale_running)
            .expect("get stale task")
            .expect("stale task exists");
        assert_eq!(stale_task.status, "pending");
        assert_eq!(stale_task.error, None);

        let failed_task = database
            .get_task(failed)
            .expect("get failed task")
            .expect("failed task exists");
        assert_eq!(failed_task.status, "failed");
        assert_eq!(failed_task.error.as_deref(), Some("scan failed"));

        let succeeded_task = database
            .get_task(succeeded)
            .expect("get succeeded task")
            .expect("succeeded task exists");
        assert_eq!(succeeded_task.status, "succeeded");

        let pending_ids = database
            .list_pending_root_scan_task_ids()
            .expect("list pending root scan task ids");
        assert_eq!(
            pending_ids,
            vec![high_priority_pending, low_priority_pending, stale_running]
        );
    }

    fn seed_search_fixture(database: &Database) -> (i64, i64, Vec<i64>) {
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Search".to_string(),
                display_name: "Search Root".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "search-root".to_string(),
                mtime: Some(1),
            })
            .expect("seed root folder");

        let entries = [
            ("alpha sunset.jpg", ".jpg", 100, "image"),
            ("beach photo.jpg", ".jpg", 200, "image"),
            ("clip.mp4", ".mp4", 300, "video"),
            ("readme.txt", ".txt", 400, "other"),
        ];
        let mut file_ids = Vec::new();
        for (index, (name, ext, mtime, kind)) in entries.iter().enumerate() {
            let file_id = database
                .upsert_file(FileUpsert {
                    root_id,
                    folder_id,
                    name: (*name).to_string(),
                    ext: (*ext).to_string(),
                    size: 1000 + index as i64,
                    mtime: *mtime,
                    ctime: None,
                    file_key: None,
                })
                .expect("insert file");
            database
                .upsert_media_kind(file_id, kind)
                .expect("insert media kind");
            file_ids.push(file_id);
        }
        (root_id, folder_id, file_ids)
    }

    #[test]
    fn create_tag_validates_name_and_rejects_duplicates() {
        let database = migrated_database();
        let tag = database
            .create_tag("Vacation", Some("#aabbcc"))
            .expect("create tag");
        assert_eq!(tag.name, "Vacation");
        assert_eq!(tag.color.as_deref(), Some("#aabbcc"));

        let tags = database.list_tags().expect("list tags");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].id, tag.id);

        let duplicate = database
            .create_tag("Vacation", None)
            .expect_err("duplicate tag should fail");
        assert!(matches!(duplicate, TagError::Duplicate));

        let invalid = database
            .create_tag("   ", None)
            .expect_err("empty tag should fail");
        assert!(matches!(invalid, TagError::InvalidName));

        let bad_color = database
            .create_tag("Other", Some("blue"))
            .expect_err("non-hex color should fail");
        assert!(matches!(bad_color, TagError::InvalidColor));

        assert!(database.delete_tag(tag.id).expect("delete tag"));
        assert!(database.list_tags().expect("list").is_empty());
    }

    #[test]
    fn upsert_user_metadata_partial_preserves_untouched_fields() {
        let database = migrated_database();
        let (_root, _folder, files) = seed_search_fixture(&database);
        let file_id = files[0];

        let initial = database
            .upsert_user_metadata_partial(
                file_id,
                UserMetadataPatch {
                    rating: Some(Some(4)),
                    favorite: Some(true),
                    note: Some(Some("first note".to_string())),
                },
            )
            .expect("initial upsert");
        assert_eq!(initial.rating, Some(4));
        assert!(initial.favorite);
        assert_eq!(initial.note.as_deref(), Some("first note"));
        assert!(initial.updated_at > 0);

        let only_favorite = database
            .upsert_user_metadata_partial(
                file_id,
                UserMetadataPatch {
                    rating: None,
                    favorite: Some(false),
                    note: None,
                },
            )
            .expect("partial favorite update");
        assert_eq!(only_favorite.rating, Some(4));
        assert!(!only_favorite.favorite);
        assert_eq!(only_favorite.note.as_deref(), Some("first note"));

        let cleared_rating = database
            .upsert_user_metadata_partial(
                file_id,
                UserMetadataPatch {
                    rating: Some(None),
                    favorite: None,
                    note: None,
                },
            )
            .expect("clear rating");
        assert!(cleared_rating.rating.is_none());
        assert!(!cleared_rating.favorite);
        assert_eq!(cleared_rating.note.as_deref(), Some("first note"));

        let cleared_note = database
            .upsert_user_metadata_partial(
                file_id,
                UserMetadataPatch {
                    rating: None,
                    favorite: None,
                    note: Some(None),
                },
            )
            .expect("clear note");
        assert!(cleared_note.note.is_none());
    }

    #[test]
    fn set_file_tags_replaces_dedupes_and_rejects_unknown_ids() {
        let database = migrated_database();
        let (_root, _folder, files) = seed_search_fixture(&database);
        let file_id = files[0];
        let tag_a = database.create_tag("alpha", None).expect("create tag a");
        let tag_b = database.create_tag("beta", None).expect("create tag b");
        let tag_c = database.create_tag("gamma", None).expect("create tag c");

        let unknown = database
            .set_file_tags(file_id, &[tag_a.id, 9999])
            .expect_err("unknown tag id should reject");
        assert!(matches!(unknown, TagError::UnknownTagId(9999)));

        let initial = database
            .set_file_tags(file_id, &[tag_a.id, tag_b.id, tag_a.id])
            .expect("set tags");
        assert_eq!(initial, vec![tag_a.id, tag_b.id]);

        let replaced = database
            .set_file_tags(file_id, &[tag_c.id])
            .expect("replace tags");
        assert_eq!(replaced, vec![tag_c.id]);

        let added = database.add_file_tag(file_id, tag_a.id).expect("add tag");
        assert_eq!(added, vec![tag_a.id, tag_c.id]);

        let removed = database
            .remove_file_tag(file_id, tag_c.id)
            .expect("remove tag");
        assert_eq!(removed, vec![tag_a.id]);

        let missing = database
            .remove_file_tag(file_id, tag_c.id)
            .expect_err("removing already-absent tag fails");
        assert!(matches!(missing, TagError::UnknownTagId(_)));
    }

    #[test]
    fn search_media_page_matches_name_note_and_tag_via_fts() {
        let database = migrated_database();
        let (root_id, _folder, files) = seed_search_fixture(&database);
        let alpha = files[0];
        let beach = files[1];
        let clip = files[2];

        database
            .upsert_user_metadata_partial(
                clip,
                UserMetadataPatch {
                    rating: None,
                    favorite: None,
                    note: Some(Some("contains a holiday note".to_string())),
                },
            )
            .expect("note for clip");

        let tag = database.create_tag("holiday", None).expect("create tag");
        database
            .add_file_tag(beach, tag.id)
            .expect("tag beach as holiday");

        let by_name = database
            .search_media_page(SearchQuery {
                q: Some("alpha".to_string()),
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                kind: None,
                min_rating: None,
                favorite: None,
                tag_ids: Vec::new(),
                sort: "mtime_desc".to_string(),
                limit: 50,
                offset: None,
                cursor: None,
            })
            .expect("search by name");
        assert_eq!(by_name.items.len(), 1);
        assert_eq!(by_name.items[0].id, alpha);

        let by_note = database
            .search_media_page(SearchQuery {
                q: Some("holiday".to_string()),
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                kind: None,
                min_rating: None,
                favorite: None,
                tag_ids: Vec::new(),
                sort: "mtime_desc".to_string(),
                limit: 50,
                offset: None,
                cursor: None,
            })
            .expect("search by tag/note token");
        let mut ids: Vec<i64> = by_note.items.iter().map(|m| m.id).collect();
        ids.sort();
        let mut expected = vec![beach, clip];
        expected.sort();
        assert_eq!(ids, expected);
        assert_eq!(by_note.total_count, Some(by_note.items.len() as i64));
    }

    #[test]
    fn search_media_page_combines_kind_min_rating_favorite_and_tag_ids_with_and() {
        let database = migrated_database();
        let (root_id, _folder, files) = seed_search_fixture(&database);
        let alpha = files[0];
        let beach = files[1];

        database
            .upsert_user_metadata_partial(
                alpha,
                UserMetadataPatch {
                    rating: Some(Some(5)),
                    favorite: Some(true),
                    note: None,
                },
            )
            .expect("alpha metadata");
        database
            .upsert_user_metadata_partial(
                beach,
                UserMetadataPatch {
                    rating: Some(Some(2)),
                    favorite: Some(false),
                    note: None,
                },
            )
            .expect("beach metadata");

        let beach_tag = database.create_tag("beach", None).expect("create tag");
        let summer_tag = database.create_tag("summer", None).expect("create summer");
        database
            .set_file_tags(alpha, &[beach_tag.id, summer_tag.id])
            .expect("alpha tags");
        database
            .set_file_tags(beach, &[beach_tag.id])
            .expect("beach tags");

        let result = database
            .search_media_page(SearchQuery {
                q: None,
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                kind: Some("image".to_string()),
                min_rating: Some(3),
                favorite: Some(true),
                tag_ids: vec![beach_tag.id, summer_tag.id],
                sort: "mtime_desc".to_string(),
                limit: 50,
                offset: None,
                cursor: None,
            })
            .expect("search combined filters");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.total_count, Some(result.items.len() as i64));
        assert_eq!(result.items[0].id, alpha);
        assert!(result.items[0].favorite);
        assert_eq!(result.items[0].rating, Some(5));
        let mut tag_ids = result.items[0].tag_ids.clone();
        tag_ids.sort();
        let mut expected_tags = vec![beach_tag.id, summer_tag.id];
        expected_tags.sort();
        assert_eq!(tag_ids, expected_tags);
    }

    #[test]
    fn search_media_page_sort_by_rating_desc_places_null_last() {
        let database = migrated_database();
        let (root_id, _folder, files) = seed_search_fixture(&database);
        database
            .upsert_user_metadata_partial(
                files[0],
                UserMetadataPatch {
                    rating: Some(Some(3)),
                    favorite: None,
                    note: None,
                },
            )
            .expect("alpha rating");
        database
            .upsert_user_metadata_partial(
                files[2],
                UserMetadataPatch {
                    rating: Some(Some(5)),
                    favorite: None,
                    note: None,
                },
            )
            .expect("clip rating");

        let result = database
            .search_media_page(SearchQuery {
                q: None,
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                kind: None,
                min_rating: None,
                favorite: None,
                tag_ids: Vec::new(),
                sort: "rating_desc".to_string(),
                limit: 50,
                offset: None,
                cursor: None,
            })
            .expect("rating desc");
        let ids: Vec<i64> = result.items.iter().map(|item| item.id).collect();
        // First two items are the rated ones (5 then 3), then NULL ratings
        // in id-desc order.
        assert_eq!(ids[0], files[2]);
        assert_eq!(ids[1], files[0]);
        assert!(ids.contains(&files[1]));
        assert!(ids.contains(&files[3]));
        let null_section = &ids[2..];
        assert!(null_section
            .iter()
            .all(|id| { *id == files[1] || *id == files[3] }));
    }

    #[test]
    fn search_media_page_without_query_applies_filters_via_non_fts_path() {
        let database = migrated_database();
        let (root_id, _folder, files) = seed_search_fixture(&database);
        let result = database
            .search_media_page(SearchQuery {
                q: None,
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                kind: Some("image".to_string()),
                min_rating: None,
                favorite: None,
                tag_ids: Vec::new(),
                sort: "name_asc".to_string(),
                limit: 50,
                offset: None,
                cursor: None,
            })
            .expect("image search without query");
        let names: Vec<&str> = result.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["alpha sunset.jpg", "beach photo.jpg"]);
        assert!(result
            .items
            .iter()
            .all(|item| item.kind.as_deref() == Some("image")));
        assert!(result.items.iter().all(|item| item.id != files[2]));
        assert_eq!(result.total_count, Some(result.items.len() as i64));
    }

    #[test]
    fn delete_tag_resyncs_fts_for_attached_files() {
        let database = migrated_database();
        let (root_id, _folder, files) = seed_search_fixture(&database);
        let beach = files[1];

        // Create a tag with a unique name and attach it to one file. Add a
        // distinct note so the FTS row gets rebuilt to a known shape, and so
        // we can assert the tag-only match disappears after deletion.
        let tag = database.create_tag("ZebraTag", None).expect("create tag");
        database
            .add_file_tag(beach, tag.id)
            .expect("attach tag to beach");

        // Sanity check: the tag name matches via FTS before deletion.
        let before = database
            .search_media_page(SearchQuery {
                q: Some("ZebraTag".to_string()),
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                kind: None,
                min_rating: None,
                favorite: None,
                tag_ids: Vec::new(),
                sort: "mtime_desc".to_string(),
                limit: 50,
                offset: None,
                cursor: None,
            })
            .expect("search before delete");
        let before_ids: Vec<i64> = before.items.iter().map(|item| item.id).collect();
        assert!(
            before_ids.contains(&beach),
            "expected ZebraTag to match {beach} via FTS before deletion, got {before_ids:?}"
        );

        assert!(database.delete_tag(tag.id).expect("delete tag"));

        // After deletion the FTS row for the previously tagged file must no
        // longer carry "ZebraTag", so the search returns no hits.
        let after = database
            .search_media_page(SearchQuery {
                q: Some("ZebraTag".to_string()),
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                kind: None,
                min_rating: None,
                favorite: None,
                tag_ids: Vec::new(),
                sort: "mtime_desc".to_string(),
                limit: 50,
                offset: None,
                cursor: None,
            })
            .expect("search after delete");
        let after_ids: Vec<i64> = after.items.iter().map(|item| item.id).collect();
        assert!(
            !after_ids.contains(&beach),
            "expected ZebraTag to no longer match {beach} via FTS after deletion, got {after_ids:?}"
        );
    }

    #[test]
    fn search_media_page_populates_tag_ids_in_one_batch() {
        let database = migrated_database();
        let (root_id, folder_id, _files) = seed_search_fixture(&database);

        // Five additional files with varying tag attachments so we can assert
        // each tag_ids slice is correct without resorting to a second
        // round-trip per row.
        let mut file_ids = Vec::new();
        for index in 0..5_i64 {
            let name = format!("batch_{index}.jpg");
            let id = database
                .upsert_file(FileUpsert {
                    root_id,
                    folder_id,
                    name: name.clone(),
                    ext: ".jpg".to_string(),
                    size: 10 + index,
                    mtime: 1000 + index,
                    ctime: None,
                    file_key: None,
                })
                .expect("insert batch file");
            database.upsert_media_kind(id, "image").expect("set kind");
            file_ids.push(id);
        }

        let tag_red = database.create_tag("red", None).expect("create red");
        let tag_blue = database.create_tag("blue", None).expect("create blue");
        let tag_green = database.create_tag("green", None).expect("create green");

        // Distinct attachments per file: empty, red, blue, red+blue, all three.
        database
            .set_file_tags(file_ids[0], &[])
            .expect("set [] for f0");
        database
            .set_file_tags(file_ids[1], &[tag_red.id])
            .expect("set red for f1");
        database
            .set_file_tags(file_ids[2], &[tag_blue.id])
            .expect("set blue for f2");
        database
            .set_file_tags(file_ids[3], &[tag_red.id, tag_blue.id])
            .expect("set red+blue for f3");
        database
            .set_file_tags(file_ids[4], &[tag_red.id, tag_blue.id, tag_green.id])
            .expect("set all for f4");

        let result = database
            .search_media_page(SearchQuery {
                q: None,
                root_id: Some(root_id),
                folder_id: Some(folder_id),
                include_descendants: false,
                kind: Some("image".to_string()),
                min_rating: None,
                favorite: None,
                tag_ids: Vec::new(),
                sort: "mtime_asc".to_string(),
                limit: 500,
                offset: None,
                cursor: None,
            })
            .expect("search batch");

        let by_id: std::collections::HashMap<i64, Vec<i64>> = result
            .items
            .iter()
            .map(|item| (item.id, item.tag_ids.clone()))
            .collect();

        let mut expected_f1 = vec![tag_red.id];
        expected_f1.sort();
        let mut actual_f1 = by_id.get(&file_ids[1]).cloned().unwrap_or_default();
        actual_f1.sort();
        assert_eq!(actual_f1, expected_f1);

        let mut expected_f3 = vec![tag_red.id, tag_blue.id];
        expected_f3.sort();
        let mut actual_f3 = by_id.get(&file_ids[3]).cloned().unwrap_or_default();
        actual_f3.sort();
        assert_eq!(actual_f3, expected_f3);

        let mut expected_f4 = vec![tag_red.id, tag_blue.id, tag_green.id];
        expected_f4.sort();
        let mut actual_f4 = by_id.get(&file_ids[4]).cloned().unwrap_or_default();
        actual_f4.sort();
        assert_eq!(actual_f4, expected_f4);

        let actual_f0 = by_id.get(&file_ids[0]).cloned().unwrap_or_default();
        assert!(
            actual_f0.is_empty(),
            "f0 must have no tags, got {actual_f0:?}"
        );
    }

    #[test]
    fn resolve_file_source_path_returns_none_on_self_parent_cycle() {
        // Simulate a corrupt DB where a folder points at itself. Without the
        // depth cap added in this hotfix, the parent walk would loop forever.
        // The cap keeps the worker responsive and surfaces a warn log instead.
        let database = migrated_database();
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "cycle-root".to_string(),
                mtime: Some(0),
            })
            .expect("insert root folder");
        let file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "looped.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 1,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .expect("insert file");
        database
            .upsert_media_kind(file_id, "image")
            .expect("insert media kind");

        // Force the cycle: folder.parent_id points at itself. The walk
        // must terminate via the depth cap and return Ok(None) rather than
        // hang.
        database
            .connection
            .execute(
                "UPDATE folders SET parent_id = id WHERE id = ?1",
                [folder_id],
            )
            .expect("inject self-parent cycle");

        // Bound the test wall-clock so a regression that re-introduces the
        // unbounded walk fails fast instead of hanging the suite.
        let (sender, receiver) = std::sync::mpsc::channel::<anyhow::Result<Option<PathBuf>>>();
        let database = std::sync::Arc::new(std::sync::Mutex::new(database));
        let resolver = std::sync::Arc::clone(&database);
        std::thread::spawn(move || {
            let guard = resolver.lock().expect("lock database");
            let result = guard.resolve_file_source_path(file_id);
            let _ = sender.send(result);
        });

        let outcome = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("resolve_file_source_path must terminate within 5s on a cycle");
        let resolved = outcome.expect("resolve returns Ok even on cycles");
        assert!(
            resolved.is_none(),
            "cyclic folder graph must resolve to None, got: {resolved:?}"
        );
    }

    /// Seeds a root+folder+2 files, each with a `failed` thumbnail row whose
    /// `error` text is set as given. When `null_out_failure_kind` is true, the
    /// row's `failure_kind` is explicitly nulled out afterward — simulating
    /// rows written after migration 0018 added the column but before this
    /// backfill classified them. When false, the seed skips that step
    /// entirely, for use against a pre-0018 schema where the `failure_kind`
    /// column does not exist yet. Returns (net_file_id, fmt_file_id).
    fn seed_two_failed_thumbs(
        database: &Database,
        net_error: &str,
        fmt_error: &str,
        null_out_failure_kind: bool,
    ) -> (i64, i64) {
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures/failkind-backfill".to_string(),
                display_name: "Failkind Backfill".to_string(),
            })
            .expect("add root");
        let folder_id = database
            .upsert_folder(FolderUpsert {
                root_id,
                parent_id: None,
                name: String::new(),
                path_hash: "failkind-backfill-root-hash".to_string(),
                mtime: Some(10),
            })
            .expect("insert root folder");

        let net_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "net.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 100,
                mtime: 100,
                ctime: None,
                file_key: None,
            })
            .expect("insert net file");
        database
            .upsert_media_kind(net_file_id, "image")
            .expect("insert net media kind");

        let fmt_file_id = database
            .upsert_file(FileUpsert {
                root_id,
                folder_id,
                name: "fmt.avif".to_string(),
                ext: ".avif".to_string(),
                size: 100,
                mtime: 100,
                ctime: None,
                file_key: None,
            })
            .expect("insert fmt file");
        database
            .upsert_media_kind(fmt_file_id, "image")
            .expect("insert fmt media kind");

        for (file_id, error) in [(net_file_id, net_error), (fmt_file_id, fmt_error)] {
            database
                .upsert_thumbnail_state(ThumbnailStateUpsert {
                    file_id,
                    profile: crate::thumbnails::GRID_320_PROFILE.to_string(),
                    state: "failed".to_string(),
                    cache_key: None,
                    width: None,
                    height: None,
                    byte_size: None,
                    error: Some(error.to_string()),
                    source_fingerprint: None,
                })
                .expect("seed failed thumbnail");
            if null_out_failure_kind {
                // Simulate rows written after 0018 added the column but
                // before this row was classified.
                database
                    .connection
                    .execute(
                        "UPDATE thumbs SET state = 'failed', error = ?1, failure_kind = NULL \
                         WHERE file_id = ?2 AND profile = ?3",
                        (error, file_id, crate::thumbnails::GRID_320_PROFILE),
                    )
                    .expect("null out failure_kind");
            }
        }

        (net_file_id, fmt_file_id)
    }

    fn read_failure_kind(database: &Database, file_id: i64) -> Option<String> {
        database
            .connection
            .query_row(
                "SELECT failure_kind FROM thumbs WHERE file_id = ?1 AND profile = ?2",
                (file_id, crate::thumbnails::GRID_320_PROFILE),
                |row| row.get(0),
            )
            .expect("read failure_kind")
    }

    #[test]
    fn migration_0018_backfills_failure_kind_from_error_text() {
        let dir = unique_db_temp_dir("failkind-backfill");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("bf.sqlite");
        // Open + migrate through 0017 by applying migrations, then manually insert
        // failed rows WITHOUT failure_kind (simulating pre-0018 data) is hard once
        // 0018 is auto-applied; instead: open, apply_migrations (adds column), then
        // NULL out failure_kind on inserted failed rows, then call the backfill fn.
        let db = Database::open(&db_path).unwrap();
        db.apply_migrations().unwrap();
        // seed a root+folder+2 files+2 failed thumbs with known errors, failure_kind NULL
        let (net_fid, fmt_fid) = seed_two_failed_thumbs(
            &db,
            "thumbnail decode failed: 找不到网络路径。 (os error 53)",
            "thumbnail decode failed: The image format Avif is not supported",
            true,
        );
        {
            let transaction = db.connection.unchecked_transaction().unwrap();
            backfill_thumbnail_failure_kind_in_transaction(&transaction).unwrap();
            transaction.commit().unwrap();
        }
        assert_eq!(
            read_failure_kind(&db, net_fid),
            Some("transient".to_string())
        );
        assert_eq!(
            read_failure_kind(&db, fmt_fid),
            Some("permanent".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Builds an in-memory database migrated through version 17 only — i.e.
    /// the schema state immediately BEFORE the 0018 gate has ever run: no
    /// `thumbs.failure_kind` column, no `schema_migrations` row for version
    /// 18. Mirrors the same "apply earlier migration constants directly"
    /// idiom as `task_attempt_generation_migration_recovers_partial_apply_and_defaults_existing_tasks`,
    /// rather than calling the public `apply_migrations()` (which would also
    /// run gate 18).
    fn database_migrated_through_17() -> Database {
        let database = Database::open_in_memory().expect("open in-memory database");
        database
            .connection
            .execute_batch(migrations::INITIAL_MIGRATION)
            .expect("apply initial migration");
        database
            .connection
            .execute_batch(migrations::TASK_PROGRESS_MIGRATION)
            .expect("apply task progress migration");
        database
            .connection
            .execute_batch(migrations::BROWSING_INDEXES_MIGRATION)
            .expect("apply browsing indexes migration");
        database
            .connection
            .execute_batch(migrations::THUMBNAIL_STATE_MIGRATION)
            .expect("apply thumbnail state migration");
        database
            .connection
            .pragma_update(None, "foreign_keys", "ON")
            .expect("re-enable foreign keys after thumbnail state migration");
        database
            .connection
            .execute_batch(migrations::THUMBNAIL_SOURCE_FINGERPRINT_MIGRATION)
            .expect("apply thumbnail source fingerprint migration");
        database
            .connection
            .pragma_update(None, "foreign_keys", "ON")
            .expect("re-enable foreign keys after fingerprint migration");
        database
            .connection
            .execute_batch(migrations::THUMBNAIL_TASK_ATTEMPT_FINGERPRINT_MIGRATION)
            .expect("apply thumbnail task attempt fingerprint migration");
        database
            .ensure_task_contract_prerequisite_columns()
            .expect("ensure task contract prerequisite columns");
        database
            .connection
            .execute_batch(migrations::TASK_STATUS_CONTRACT_MIGRATION)
            .expect("apply task status contract migration");
        database
            .connection
            .pragma_update(None, "foreign_keys", "ON")
            .expect("re-enable foreign keys after task status contract migration");
        database
            .apply_task_attempt_generation_migration()
            .expect("apply task attempt generation migration");
        database
            .apply_scan_reconciliation_migration()
            .expect("apply scan reconciliation migration");
        database
            .connection
            .execute_batch(migrations::MEDIA_FTS_CONTENTLESS_DELETE_MIGRATION)
            .expect("apply media fts contentless delete migration");
        database
            .apply_plugins_extended_migration()
            .expect("apply plugins extended migration");
        database
            .apply_preview_pipeline_refactor_migration()
            .expect("apply preview pipeline refactor migration");
        database
            .apply_preview_served_by_migration()
            .expect("apply preview served by migration");
        database
            .apply_interactive_folder_scan_task_migration()
            .expect("apply interactive folder scan task migration");
        database
            .connection
            .execute_batch(migrations::ROOT_STATUS_BROWSING_INDEXES_MIGRATION)
            .expect("apply root status browsing indexes migration");
        database
            .connection
            .execute_batch(migrations::FOLDER_STATUS_BROWSING_INDEXES_MIGRATION)
            .expect("apply folder status browsing indexes migration");
        database
            .connection
            .execute_batch(migrations::THUMBNAIL_TASK_INDEXES_MIGRATION)
            .expect("apply thumbnail task indexes migration");
        database
    }

    fn database_migrated_through_20() -> Database {
        let database = database_migrated_through_17();
        {
            let transaction = database.connection.unchecked_transaction().unwrap();
            transaction
                .execute_batch(migrations::THUMB_FAILURE_KIND_MIGRATION)
                .expect("apply thumbnail failure kind migration");
            backfill_thumbnail_failure_kind_in_transaction(&transaction)
                .expect("backfill thumbnail failure kinds");
            transaction.commit().expect("commit migration 18");
        }
        {
            let transaction = database.connection.unchecked_transaction().unwrap();
            transaction
                .execute_batch(migrations::ROOT_PENDING_DELETE_MIGRATION)
                .expect("apply root pending delete migration");
            transaction.commit().expect("commit migration 19");
        }
        database
            .connection
            .execute_batch(migrations::ROOT_PURGE_TASK_KIND_MIGRATION)
            .expect("apply root purge task kind migration");
        database
            .connection
            .pragma_update(None, "foreign_keys", "ON")
            .expect("re-enable foreign keys after migration 20");
        assert!(database.migration_applied(20).unwrap());
        assert!(!database.migration_applied(21).unwrap());
        database
    }

    fn database_migrated_through_22() -> Database {
        let database = database_migrated_through_20();
        database
            .connection
            .execute_batch(migrations::NOTIFICATION_OUTBOX_MIGRATION)
            .expect("apply migration 21");
        database
            .connection
            .execute_batch(migrations::FILE_OPERATION_JOURNAL_MIGRATION)
            .expect("apply migration 22");
        assert!(database.migration_applied(22).unwrap());
        assert!(!database.migration_applied(23).unwrap());
        database
    }

    fn database_migrated_through_23() -> Database {
        let database = database_migrated_through_22();
        database
            .connection
            .execute_batch(migrations::FILE_OPERATION_RECOVERY_METADATA_MIGRATION)
            .expect("apply migration 23");
        assert!(database.migration_applied(23).unwrap());
        assert!(!database.migration_applied(24).unwrap());
        database
    }

    fn index_key_columns(database: &Database, index_name: &str) -> Vec<(String, bool)> {
        let mut statement = database
            .connection
            .prepare(&format!("PRAGMA index_xinfo('{index_name}')"))
            .expect("inspect index columns");
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, i64>(5)? != 0,
                ))
            })
            .expect("query index columns")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect index columns")
            .into_iter()
            .filter_map(|(name, descending, is_key)| is_key.then_some((name?, descending)))
            .collect()
    }

    /// Proves the 0018 gate's atomicity: the ALTER TABLE (add
    /// `failure_kind`), the `schema_migrations` version-18 registration, and
    /// the failure_kind backfill all live inside ONE explicit transaction, so
    /// a crash/panic before that transaction's `commit()` rolls back all
    /// three together — never just the registration, and never a partial
    /// backfill with the registration already durable. This directly guards
    /// against the pre-fix bug where `execute_batch` (which auto-commits the
    /// registration row immediately) ran before a separate, unguarded
    /// backfill pass, so a mid-backfill crash left migration 18 marked
    /// applied with some `failed` rows permanently stuck at
    /// `failure_kind IS NULL`.
    #[test]
    fn migration_18_gate_rolls_back_registration_and_column_when_not_committed() {
        let database = database_migrated_through_17();
        assert!(!database.migration_applied(18).unwrap());
        assert!(!database.table_has_column("thumbs", "failure_kind").unwrap());

        // Seed 2 failed thumbnail rows against the pre-0018 schema (no
        // failure_kind column yet — upsert_thumbnail_state never references
        // it, matching real pre-migration data).
        let (net_fid, fmt_fid) = seed_two_failed_thumbs(
            &database,
            "thumbnail decode failed: 找不到网络路径。 (os error 53)",
            "thumbnail decode failed: The image format Avif is not supported",
            false,
        );

        // Simulate a crash: run exactly what the real 0018 gate runs (ALTER +
        // schema_migrations registration, both inside THUMB_FAILURE_KIND_MIGRATION,
        // plus the failure_kind backfill) inside one explicit transaction, then
        // drop it WITHOUT calling commit(). rusqlite's Transaction rolls back
        // on drop unless committed, exactly like the abrupt rollback SQLite
        // performs on an OS crash / process kill mid-transaction.
        {
            let transaction = database.connection.unchecked_transaction().unwrap();
            transaction
                .execute_batch(migrations::THUMB_FAILURE_KIND_MIGRATION)
                .expect("run ALTER + registration inside the uncommitted transaction");
            backfill_thumbnail_failure_kind_in_transaction(&transaction)
                .expect("run backfill inside the uncommitted transaction");
            // No `transaction.commit()` — dropped here, forcing a rollback.
        }

        // Nothing from the "crashed" attempt is visible: no column, no
        // registration row, and (implicitly) no partial backfill, because the
        // column it would have written to doesn't even exist anymore.
        assert!(
            !database.table_has_column("thumbs", "failure_kind").unwrap(),
            "ALTER TABLE must roll back along with the rest of the gate"
        );
        assert!(
            !database.migration_applied(18).unwrap(),
            "schema_migrations row must roll back too, so the gate reruns on next startup"
        );

        // Recovery: rerunning the real gate must succeed with no "duplicate
        // column" error (proving the ALTER really was rolled back, not just
        // the registration) and must classify the rows the crashed attempt
        // never durably touched.
        database
            .apply_migrations()
            .expect("rerun migration 18 gate cleanly after a simulated crash");
        assert!(database.migration_applied(18).unwrap());
        assert_eq!(
            read_failure_kind(&database, net_fid),
            Some("transient".to_string())
        );
        assert_eq!(
            read_failure_kind(&database, fmt_fid),
            Some("permanent".to_string())
        );
    }

    #[test]
    fn migration_0019_adds_root_pending_delete_column_and_index() {
        let database = migrated_database();
        let columns: Vec<String> = {
            let mut stmt = database
                .connection
                .prepare("PRAGMA table_info(roots)")
                .expect("inspect roots");
            stmt.query_map([], |row| row.get::<_, String>(1))
                .expect("query columns")
                .collect::<rusqlite::Result<Vec<String>>>()
                .expect("collect columns")
        };
        assert!(columns.iter().any(|c| c == "pending_delete_at"));
        let has_index: bool = database
            .connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_roots_pending_delete'",
                [],
                |_| Ok(true),
            )
            .optional()
            .expect("query index")
            .unwrap_or(false);
        assert!(has_index, "partial index must exist");
    }

    #[test]
    fn migration_0020_widens_task_kind_check_to_accept_root_purge() {
        let database = migrated_database();
        let (root_id, _folder_id) = seed_media_page_fixture(&database);

        // The new kind is accepted...
        database
            .connection
            .execute(
                r#"
                INSERT INTO tasks(kind, priority, status, root_id, file_id, created_at, updated_at, error)
                VALUES ('root_purge', 0, 'pending', ?1, NULL, 0, 0, NULL)
                "#,
                [root_id],
            )
            .expect("root_purge kind must be accepted by the CHECK constraint");

        // ...but the CHECK constraint still rejects unsupported kinds.
        let rejected = database.connection.execute(
            r#"
            INSERT INTO tasks(kind, priority, status, root_id, file_id, created_at, updated_at, error)
            VALUES ('bogus_kind', 0, 'pending', ?1, NULL, 0, 0, NULL)
            "#,
            [root_id],
        );
        assert!(
            rejected.is_err(),
            "unsupported task kinds must still be rejected"
        );

        // The rebuild must not have dropped any of the tasks indexes.
        for index_name in [
            "idx_tasks_status_priority",
            "idx_tasks_kind_file_status",
            "idx_tasks_kind_status_priority",
        ] {
            let exists: bool = database
                .connection
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1",
                    [index_name],
                    |_| Ok(true),
                )
                .optional()
                .expect("query index")
                .unwrap_or(false);
            assert!(exists, "{index_name} must survive the tasks rebuild");
        }
    }

    #[test]
    fn migration_0021_adds_notification_schema_and_constraints() {
        let database = migrated_database();

        let head_columns: Vec<String> = database
            .connection
            .prepare("PRAGMA table_info(notification_operation_heads)")
            .expect("inspect notification heads")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query notification head columns")
            .collect::<rusqlite::Result<Vec<String>>>()
            .expect("collect notification head columns");
        assert_eq!(
            head_columns,
            vec![
                "operation_id",
                "kind",
                "status",
                "presentation",
                "revision",
                "payload_json",
                "dirty",
                "last_outbox_at",
                "updated_at",
                "completed_at",
            ]
        );

        let outbox_columns: Vec<String> = database
            .connection
            .prepare("PRAGMA table_info(notification_outbox)")
            .expect("inspect notification outbox")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query notification outbox columns")
            .collect::<rusqlite::Result<Vec<String>>>()
            .expect("collect notification outbox columns");
        assert_eq!(
            outbox_columns,
            vec![
                "sequence",
                "operation_id",
                "revision",
                "payload_json",
                "created_at"
            ]
        );

        assert!(database
            .table_has_column("file_operations", "operation_group_id")
            .unwrap());
        assert!(database.migration_applied(21).unwrap());
        for index_name in [
            "idx_notification_heads_status_updated",
            "idx_file_operations_group",
        ] {
            let exists: bool = database
                .connection
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1",
                    [index_name],
                    |_| Ok(true),
                )
                .optional()
                .expect("query notification index")
                .unwrap_or(false);
            assert!(exists, "{index_name} must exist");
        }

        database
            .connection
            .execute(
                "INSERT INTO notification_operation_heads(operation_id, kind, status, presentation, revision, payload_json, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                ("operation-1", "scan", "pending", "important", 1_i64, "{}", 1_i64),
            )
            .expect("valid notification head must insert");
        database
            .connection
            .execute(
                "INSERT INTO notification_outbox(operation_id, revision, payload_json, created_at) VALUES (?1, ?2, ?3, ?4)",
                ("operation-1", 1_i64, "{}", 1_i64),
            )
            .expect("valid notification outbox row must insert");

        for (operation_id, status) in [("bad-status", "paused"), ("bad-status-2", "unknown")] {
            let result = database.connection.execute(
                "INSERT INTO notification_operation_heads(operation_id, kind, status, presentation, revision, payload_json, updated_at) VALUES (?1, 'scan', ?2, 'important', 1, '{}', 1)",
                (operation_id, status),
            );
            assert!(result.is_err(), "unsupported status must be rejected");
        }
        let result = database.connection.execute(
            "INSERT INTO notification_operation_heads(operation_id, kind, status, presentation, revision, payload_json, updated_at) VALUES ('bad-presentation', 'scan', 'pending', 'compact', 1, '{}', 1)",
            [],
        );
        assert!(result.is_err(), "unsupported presentation must be rejected");
        let result = database.connection.execute(
            "INSERT INTO notification_operation_heads(operation_id, kind, status, presentation, revision, payload_json, dirty, updated_at) VALUES ('bad-dirty', 'scan', 'pending', 'important', 1, '{}', 2, 1)",
            [],
        );
        assert!(
            result.is_err(),
            "dirty values other than 0 or 1 must be rejected"
        );

        database
            .connection
            .execute(
                "INSERT INTO file_operations(operation, source_path, target_path, status, created_at) VALUES ('rename', 'source', 'target', 'pending', 1)",
                [],
            )
            .expect("existing file_operations insert must remain valid");
        let operation_group_id: Option<String> = database
            .connection
            .query_row(
                "SELECT operation_group_id FROM file_operations WHERE id = last_insert_rowid()",
                [],
                |row| row.get(0),
            )
            .expect("read default operation group");
        assert_eq!(operation_group_id, None);
    }

    #[test]
    fn migration_0022_adds_base_file_operation_journal() {
        let database = database_migrated_through_22();
        let columns = database
            .connection
            .prepare("PRAGMA table_info(file_operations)")
            .expect("inspect file-operation journal")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query file-operation columns")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect file-operation columns");
        for column in [
            "operation_group_id",
            "entity_kind",
            "entity_id",
            "target_folder_id",
            "attempt_count",
            "error_code",
        ] {
            assert!(
                columns.iter().any(|actual| actual == column),
                "missing {column}"
            );
        }
        for column in ["root_id", "journal_phase", "source_identity"] {
            assert!(
                !columns.iter().any(|actual| actual == column),
                "migration 22 must not contain later recovery column {column}"
            );
        }
        assert!(database.migration_applied(22).unwrap());
        let pending_index: Option<String> = database
            .connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name='idx_file_operations_pending'",
                [],
                |row| row.get(0),
            )
            .optional()
            .expect("query pending journal index");
        assert!(
            pending_index
                .as_deref()
                .is_some_and(|sql| sql.contains("WHERE status = 'pending'")
                    && !sql.contains("operation_group_id IS NOT NULL")),
            "migration 22 must retain its published pending index"
        );

        database
            .connection
            .execute(
                r#"INSERT INTO file_operations(
                     operation, source_path, target_path, status, created_at,
                     operation_group_id, entity_kind, entity_id,
                     target_folder_id, attempt_count, error_code
                   ) VALUES ('move', 'old', 'new', 'pending', 1,
                             'group', 'file', 7, 9, 0, NULL)"#,
                [],
            )
            .expect("valid pending journal intent");
        let invalid_kind = database.connection.execute(
            r#"INSERT INTO file_operations(
                 operation, source_path, status, created_at, operation_group_id,
                 entity_kind, entity_id, attempt_count
               ) VALUES ('move', 'old', 'pending', 1, 'bad', 'root', 7, 0)"#,
            [],
        );
        assert!(invalid_kind.is_err(), "unsupported journal entity kind");
    }

    #[test]
    fn migration_0023_adds_recovery_metadata_on_fresh_database() {
        let database = database_migrated_through_22();
        database
            .apply_migrations()
            .expect("upgrade fresh version-22 database to version 23");
        for column in ["root_id", "journal_phase", "source_identity"] {
            assert!(
                database
                    .table_has_column("file_operations", column)
                    .unwrap(),
                "missing recovery metadata column {column}"
            );
        }
        assert!(database.migration_applied(22).unwrap());
        assert!(database.migration_applied(23).unwrap());
        let pending_index: String = database
            .connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name='idx_file_operations_pending'",
                [],
                |row| row.get(0),
            )
            .expect("query version-23 pending index");
        assert!(pending_index.contains("WHERE status = 'pending'"));
        assert!(pending_index.contains("operation_group_id IS NOT NULL"));
        let invalid_phase = database.connection.execute(
            r#"INSERT INTO file_operations(
                 operation, source_path, status, created_at, operation_group_id,
                 entity_kind, entity_id, root_id, journal_phase
               ) VALUES ('move', 'old', 'pending', 1, 'bad-phase',
                         'file', 7, 3, 'unknown')"#,
            [],
        );
        assert!(invalid_phase.is_err(), "unsupported journal phase");
    }

    #[test]
    fn migration_0023_upgrades_populated_v22_pending_journal_without_loss() {
        let database = database_migrated_through_22();
        database
            .connection
            .execute(
                r#"INSERT INTO file_operations(
                     id, operation, source_path, target_path, status, created_at,
                     operation_group_id, entity_kind, entity_id, target_folder_id,
                     attempt_count, error_code, error
                   ) VALUES (72, 'move', 'old-path', 'new-path', 'pending', 11,
                             'v22-group', 'file', 17, 23, 2, 'fs_error',
                             'persisted v22 error')"#,
                [],
            )
            .expect("seed populated version-22 pending journal");

        database
            .apply_migrations()
            .expect("upgrade v22 database to v23");

        let row: (
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<i64>,
            Option<i64>,
            i64,
            Option<String>,
            Option<String>,
            String,
            Option<i64>,
            Option<String>,
        ) = database
            .connection
            .query_row(
                r#"SELECT operation, source_path, target_path, status,
                          operation_group_id, entity_kind, entity_id,
                          target_folder_id, attempt_count, error_code, error,
                          journal_phase, root_id, source_identity
                   FROM file_operations WHERE id = 72"#,
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                        row.get(12)?,
                        row.get(13)?,
                    ))
                },
            )
            .expect("read upgraded v22 pending journal");
        assert_eq!(row.0, "move");
        assert_eq!(row.1, "old-path");
        assert_eq!(row.2, "new-path");
        assert_eq!(row.3, "pending");
        assert_eq!(row.4, "v22-group");
        assert_eq!(row.5.as_deref(), Some("file"));
        assert_eq!(row.6, Some(17));
        assert_eq!(row.7, Some(23));
        assert_eq!(row.8, 2);
        assert_eq!(row.9.as_deref(), Some("fs_error"));
        assert_eq!(row.10.as_deref(), Some("persisted v22 error"));
        assert_eq!(row.11, "legacy");
        assert_eq!(row.12, None);
        assert_eq!(row.13, None);
        assert!(database.migration_applied(23).unwrap());
    }

    #[test]
    fn migration_0022_preserves_legacy_file_operation_rows() {
        let database = database_migrated_through_20();
        database
            .connection
            .execute_batch(migrations::NOTIFICATION_OUTBOX_MIGRATION)
            .expect("apply migration 21");
        database
            .connection
            .execute(
                "INSERT INTO file_operations(id, operation, source_path, target_path, status, created_at, finished_at, error, operation_group_id) VALUES (71, 'rename', 'old', 'new', 'succeeded', 1, 2, NULL, 'legacy-group')",
                [],
            )
            .expect("seed version-21 operation");

        database
            .apply_migrations()
            .expect("upgrade through migration 22");

        let preserved: (
            String,
            Option<String>,
            Option<i64>,
            Option<i64>,
            i64,
            String,
            Option<String>,
            Option<String>,
        ) = database
            .connection
            .query_row(
                "SELECT status, entity_kind, entity_id, root_id, attempt_count, journal_phase, source_identity, error_code FROM file_operations WHERE id = 71",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .expect("read preserved operation");
        assert_eq!(
            preserved,
            (
                "succeeded".to_owned(),
                None,
                None,
                None,
                0,
                "legacy".to_owned(),
                None,
                None,
            )
        );
    }

    #[test]
    fn migration_0022_rolls_back_journal_columns_and_index_before_commit() {
        let database = database_migrated_through_20();
        database
            .connection
            .execute_batch(migrations::NOTIFICATION_OUTBOX_MIGRATION)
            .expect("apply migration 21");
        let (before_commit, _) = migrations::FILE_OPERATION_JOURNAL_MIGRATION
            .rsplit_once("COMMIT;")
            .expect("migration 22 must end with an explicit commit");
        let failing = format!("{before_commit}\nSELECT * FROM migration_0022_deliberate_failure;");

        assert!(database.connection.execute_batch(&failing).is_err());
        database
            .connection
            .execute_batch("ROLLBACK;")
            .expect("roll back failed migration 22");

        assert!(!database.migration_applied(22).unwrap());
        assert!(!database
            .table_has_column("file_operations", "entity_kind")
            .unwrap());
        let pending_index: Option<String> = database
            .connection
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_file_operations_pending'",
                [],
                |row| row.get(0),
            )
            .optional()
            .expect("query rolled-back pending index");
        assert!(pending_index.is_none());

        database.apply_migrations().expect("rerun migration 22");
        assert!(database.migration_applied(22).unwrap());
        assert!(database
            .table_has_column("file_operations", "entity_kind")
            .unwrap());
    }

    #[test]
    fn migration_0023_rolls_back_recovery_metadata_and_index_replacement() {
        let database = database_migrated_through_22();
        let original_index_sql: String = database
            .connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name='idx_file_operations_pending'",
                [],
                |row| row.get(0),
            )
            .expect("read version-22 pending index");
        let (before_commit, _) = migrations::FILE_OPERATION_RECOVERY_METADATA_MIGRATION
            .rsplit_once("COMMIT;")
            .expect("migration 23 must end with an explicit commit");
        let failing = format!("{before_commit}\nSELECT * FROM migration_0023_deliberate_failure;");

        assert!(database.connection.execute_batch(&failing).is_err());
        database
            .connection
            .execute_batch("ROLLBACK;")
            .expect("roll back failed migration 23");

        assert!(!database.migration_applied(23).unwrap());
        for column in ["root_id", "journal_phase", "source_identity"] {
            assert!(!database
                .table_has_column("file_operations", column)
                .unwrap());
        }
        let rolled_back_index_sql: String = database
            .connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name='idx_file_operations_pending'",
                [],
                |row| row.get(0),
            )
            .expect("read rolled-back pending index");
        assert_eq!(rolled_back_index_sql, original_index_sql);

        database.apply_migrations().expect("rerun migration 23");
        assert!(database.migration_applied(23).unwrap());
        assert!(database
            .table_has_column("file_operations", "journal_phase")
            .unwrap());
    }

    #[test]
    fn migration_0024_adds_object_binding_metadata_without_losing_pending_intents() {
        let database = database_migrated_through_23();
        database
            .connection
            .execute(
                r#"INSERT INTO file_operations(
                     id, operation, source_path, target_path, status, created_at,
                     operation_group_id, entity_kind, entity_id, root_id,
                     attempt_count, journal_phase, source_identity
                   ) VALUES (84, 'move', 'source', 'target', 'pending', 1,
                             'v23-binding', 'file', 9, 3, 1, 'executing',
                             'windows:1:2')"#,
                [],
            )
            .expect("seed version-23 pending intent");

        database.apply_migrations().expect("upgrade v23 to v24");

        let row: (String, Option<String>, Option<String>, String) = database
            .connection
            .query_row(
                r#"SELECT source_identity, target_parent_identity,
                          staging_identity, recycle_phase
                   FROM file_operations WHERE id = 84"#,
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read upgraded pending intent");
        assert_eq!(row.0, "windows:1:2");
        assert_eq!(row.1, None);
        assert_eq!(row.2, None);
        assert_eq!(row.3, "none");
        assert!(database.migration_applied(24).unwrap());

        let invalid_phase = database.connection.execute(
            r#"INSERT INTO file_operations(
                 operation, source_path, status, created_at, recycle_phase
               ) VALUES ('delete_recycle', 'source', 'pending', 1, 'unknown')"#,
            [],
        );
        assert!(invalid_phase.is_err(), "unsupported recycle phase");
    }

    #[test]
    fn migration_0024_rolls_back_all_binding_columns() {
        let database = database_migrated_through_23();
        let (before_commit, _) = migrations::FILE_OPERATION_OBJECT_BINDING_MIGRATION
            .rsplit_once("COMMIT;")
            .expect("migration 24 must end with an explicit commit");
        let failing = format!("{before_commit}\nSELECT * FROM migration_0024_deliberate_failure;");

        assert!(database.connection.execute_batch(&failing).is_err());
        database
            .connection
            .execute_batch("ROLLBACK;")
            .expect("roll back failed migration 24");

        assert!(!database.migration_applied(24).unwrap());
        for column in [
            "target_parent_identity",
            "staging_identity",
            "recycle_phase",
        ] {
            assert!(!database
                .table_has_column("file_operations", column)
                .unwrap());
        }

        database.apply_migrations().expect("rerun migration 24");
        assert!(database.migration_applied(24).unwrap());
    }

    #[test]
    fn migration_0021_upgrades_populated_v20_file_operations() {
        let database = database_migrated_through_20();
        database
            .connection
            .execute(
                "INSERT INTO file_operations(id, operation, source_path, target_path, status, created_at, finished_at, error) VALUES (41, 'move', 'before', 'after', 'succeeded', 10, 20, NULL)",
                [],
            )
            .expect("seed a version-20 file operation");

        database
            .apply_migrations()
            .expect("upgrade populated version-20 database to version 21");

        let operation: (String, String, Option<String>, String, i64, Option<i64>, Option<String>) =
            database
                .connection
                .query_row(
                    "SELECT operation, source_path, target_path, status, created_at, finished_at, operation_group_id FROM file_operations WHERE id = 41",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                        ))
                    },
                )
                .expect("read preserved file operation");
        assert_eq!(
            operation,
            (
                "move".to_string(),
                "before".to_string(),
                Some("after".to_string()),
                "succeeded".to_string(),
                10,
                Some(20),
                None,
            )
        );
    }

    #[test]
    fn migration_0021_rejects_duplicate_operation_revisions() {
        let database = migrated_database();
        database
            .connection
            .execute(
                "INSERT INTO notification_outbox(operation_id, revision, payload_json, created_at) VALUES ('operation-1', 7, '{}', 1)",
                [],
            )
            .expect("insert first outbox revision");
        let duplicate = database.connection.execute(
            "INSERT INTO notification_outbox(operation_id, revision, payload_json, created_at) VALUES ('operation-1', 7, '{}', 2)",
            [],
        );
        assert!(
            duplicate.is_err(),
            "duplicate operation revisions must fail"
        );
    }

    #[test]
    fn migration_0021_autoincrement_sequence_never_reuses_deleted_values() {
        let database = migrated_database();
        database
            .connection
            .execute(
                "INSERT INTO notification_outbox(operation_id, revision, payload_json, created_at) VALUES ('operation-1', 1, '{}', 1)",
                [],
            )
            .expect("insert first outbox row");
        let first_sequence = database.connection.last_insert_rowid();
        database
            .connection
            .execute("DELETE FROM notification_outbox", [])
            .expect("delete all outbox rows");
        database
            .connection
            .execute(
                "INSERT INTO notification_outbox(operation_id, revision, payload_json, created_at) VALUES ('operation-2', 1, '{}', 2)",
                [],
            )
            .expect("insert outbox row after emptying table");
        let second_sequence = database.connection.last_insert_rowid();
        assert!(
            second_sequence > first_sequence,
            "AUTOINCREMENT must not reuse an acknowledged sequence"
        );
    }

    #[test]
    fn migration_0021_rolls_back_every_change_before_commit() {
        let database = database_migrated_through_20();
        let (migration_before_commit, _) = migrations::NOTIFICATION_OUTBOX_MIGRATION
            .rsplit_once("COMMIT;")
            .expect("migration 21 must end in an explicit commit");
        let failing_migration =
            format!("{migration_before_commit}\nSELECT * FROM migration_0021_deliberate_failure;");

        let result = database.connection.execute_batch(&failing_migration);
        assert!(result.is_err(), "deliberate pre-commit failure must occur");
        database
            .connection
            .execute_batch("ROLLBACK;")
            .expect("roll back failed migration 21 transaction");

        assert!(!database.migration_applied(21).unwrap());
        assert!(!database
            .table_has_column("file_operations", "operation_group_id")
            .unwrap());
        for table_name in ["notification_operation_heads", "notification_outbox"] {
            let exists: bool = database
                .connection
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                    [table_name],
                    |_| Ok(true),
                )
                .optional()
                .expect("query rolled-back table")
                .unwrap_or(false);
            assert!(!exists, "{table_name} must roll back");
        }
        let remaining_indexes: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name IN ('idx_notification_heads_status_updated', 'idx_file_operations_group')",
                [],
                |row| row.get(0),
            )
            .expect("query rolled-back indexes");
        assert_eq!(remaining_indexes, 0, "migration indexes must roll back");
    }

    #[test]
    fn migration_0021_uses_exact_nonredundant_indexes() {
        let database = migrated_database();
        assert_eq!(
            index_key_columns(&database, "idx_notification_heads_status_updated"),
            vec![
                ("status".to_string(), false),
                ("updated_at".to_string(), true)
            ]
        );
        assert_eq!(
            index_key_columns(&database, "idx_file_operations_group"),
            vec![
                ("operation_group_id".to_string(), false),
                ("id".to_string(), false),
            ]
        );

        let indexes: Vec<(String, bool, String)> = database
            .connection
            .prepare("PRAGMA index_list(notification_outbox)")
            .expect("inspect notification outbox indexes")
            .query_map([], |row| {
                Ok((row.get(1)?, row.get::<_, i64>(2)? != 0, row.get(3)?))
            })
            .expect("query notification outbox indexes")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect notification outbox indexes");
        let unique_autoindexes: Vec<&str> = indexes
            .iter()
            .filter(|(_, unique, origin)| *unique && origin == "u")
            .map(|(name, _, _)| name.as_str())
            .collect();
        assert_eq!(unique_autoindexes.len(), 1);
        let expected_unique_columns = vec![
            ("operation_id".to_string(), false),
            ("revision".to_string(), false),
        ];
        assert_eq!(
            index_key_columns(&database, unique_autoindexes[0]),
            expected_unique_columns
        );
        let matching_btrees = indexes
            .iter()
            .filter(|(name, _, _)| index_key_columns(&database, name) == expected_unique_columns)
            .count();
        assert_eq!(
            matching_btrees, 1,
            "operation_id/revision must use one canonical B-tree"
        );
    }
}
