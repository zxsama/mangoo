pub mod migrations;
pub mod writer;
pub use writer::DbWriter;

use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

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

pub struct Database {
    connection: Connection,
    path: Option<PathBuf>,
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
}

impl ThumbnailCacheRefreshMode {
    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "missingOnly" => Some(Self::MissingOnly),
            "staleOrMissing" => Some(Self::StaleOrMissing),
            "retryFailedAndStale" => Some(Self::RetryFailedAndStale),
            _ => None,
        }
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
    pub error: Option<String>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRecord {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub status: String,
    pub capabilities: Vec<String>,
    pub permissions: Vec<String>,
    pub manifest_path: String,
    pub installed_at: i64,
    pub updated_at: i64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PluginUpsert {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub status: String,
    pub capabilities: Vec<String>,
    pub permissions: Vec<String>,
    pub manifest_path: String,
    pub last_error: Option<String>,
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
        Ok(Self { connection, path })
    }

    pub fn reopen(&self) -> anyhow::Result<Option<Self>> {
        let Some(path) = &self.path else {
            return Ok(None);
        };
        Ok(Some(Self::open(path)?))
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
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

    /// Read-only access to the underlying connection. Used by sibling
    /// modules (like `fsops`) that need ad-hoc lookups but should not own
    /// transaction lifecycle on the shared handle.
    pub(crate) fn connection_for_fsops(&self) -> &Connection {
        &self.connection
    }

    /// Mutable access to the underlying connection so callers can begin
    /// `BEGIN IMMEDIATE` transactions. Mirrors how scan batching reaches
    /// into the connection for write-side flows.
    pub(crate) fn connection_for_fsops_mut(&mut self) -> &mut Connection {
        &mut self.connection
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
            WHERE roots.enabled = 1
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
            WHERE roots.id = ?1
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
            if !enabled {
                transaction.execute("DELETE FROM folders WHERE root_id = ?1", [id])?;
                transaction.execute("DELETE FROM files WHERE root_id = ?1", [id])?;
            }
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

    pub fn disable_root(&self, root_id: i64) -> anyhow::Result<bool> {
        let transaction = self.connection.unchecked_transaction()?;
        let updated = transaction.execute(
            "UPDATE roots SET enabled = 0 WHERE id = ?1 AND enabled = 1",
            [root_id],
        )?;
        if updated != 0 {
            cancel_root_scan_tasks_in_transaction(&transaction, root_id)?;
            cleanup_inactive_thumbnail_artifacts_in_transaction(&transaction)?;
        }
        transaction.commit()?;
        Ok(updated != 0)
    }

    pub fn create_root_scan_task(&self, root_id: i64) -> anyhow::Result<i64> {
        let root = self
            .get_root(root_id)?
            .ok_or_else(|| anyhow::anyhow!("root not found: {root_id}"))?;
        if !root.enabled {
            return Err(anyhow::anyhow!("root is disabled: {root_id}"));
        }
        let now = unix_timestamp();
        if self.table_has_column("tasks", "folder_id")? {
            self.connection.execute(
                r#"
                INSERT INTO tasks(
                    kind, priority, status, root_id, folder_id, file_id, created_at, updated_at, error
                )
                VALUES ('root_scan', ?1, 'pending', ?2, NULL, NULL, ?3, ?3, NULL)
                "#,
                (ROOT_SCAN_TASK_PRIORITY, root_id, now),
            )?;
        } else {
            self.connection.execute(
                r#"
                INSERT INTO tasks(kind, priority, status, root_id, file_id, created_at, updated_at, error)
                VALUES ('root_scan', ?1, 'pending', ?2, NULL, ?3, ?3, NULL)
                "#,
                (ROOT_SCAN_TASK_PRIORITY, root_id, now),
            )?;
        }
        Ok(self.connection.last_insert_rowid())
    }

    pub fn create_interactive_folder_scan_task(&self, folder_id: i64) -> anyhow::Result<i64> {
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
                }
            }
            transaction.commit()?;
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
        transaction.commit()?;
        Ok(task_id)
    }

    pub fn reset_running_root_scan_tasks_for_recovery(&self) -> anyhow::Result<usize> {
        let updated = self.connection.execute(
            r#"
            UPDATE tasks
            SET status = 'pending', updated_at = ?1, error = NULL
            WHERE kind IN ('root_scan', 'interactive_folder_scan') AND status = 'running'
            "#,
            [unix_timestamp()],
        )?;
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
        let updated = self.connection.execute(
            r#"
            UPDATE tasks
            SET status = 'failed',
                updated_at = ?1,
                error = 'root is disabled'
            WHERE kind IN ('root_scan', 'interactive_folder_scan')
              AND status = 'pending'
              AND root_id IN (SELECT id FROM roots WHERE enabled = 0)
            "#,
            [unix_timestamp()],
        )?;
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
             skipped_files, thumbnail_source_fingerprint, attempt_generation, error";

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
                   thumbnail_source_fingerprint, attempt_generation, error
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
                   thumbnail_source_fingerprint, attempt_generation, error
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
        let updated = self.connection.execute(
            "UPDATE tasks SET status = 'running', updated_at = ?1, error = NULL WHERE id = ?2 AND status = 'pending' AND attempt_generation = ?3",
            (unix_timestamp(), task_id, attempt_generation),
        )?;
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
        let updated = self.connection.execute(
            "UPDATE tasks SET status = 'succeeded', updated_at = ?1, error = NULL WHERE id = ?2 AND status = 'running' AND attempt_generation = ?3",
            (unix_timestamp(), task_id, attempt_generation),
        )?;
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
        let updated = self.connection.execute(
            "UPDATE tasks SET status = 'failed', updated_at = ?1, error = ?2 WHERE id = ?3 AND status IN ('pending', 'running') AND attempt_generation = ?4",
            (unix_timestamp(), error, task_id, attempt_generation),
        )?;
        if updated == 0 {
            return self.ensure_one_task_attempt_updated(
                task_id,
                updated,
                "pending or running",
                attempt_generation,
            );
        }
        Ok(())
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
              AND kind = 'root_scan'
              AND status = 'running'
              AND attempt_generation = ?4
            "#,
            (next_priority, unix_timestamp(), task_id, attempt_generation),
        )?;
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
        let updated = self.connection.execute(
            r#"
            UPDATE tasks
            SET status = 'cancelled',
                updated_at = ?1,
                error = 'cancelled'
            WHERE id = ?2 AND status IN ('pending', 'running')
            "#,
            (unix_timestamp(), task_id),
        )?;
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
        let task = self
            .get_task(task_id)?
            .ok_or_else(|| anyhow::anyhow!("task not found: {task_id}"))?;
        if task.status != "failed" && task.status != "cancelled" {
            return Err(anyhow::anyhow!(
                "task {task_id} is not retryable; current status is {}",
                task.status
            ));
        }

        match task.kind.as_str() {
            "root_scan" => self.retry_root_scan_task(&task),
            "interactive_folder_scan" => self.retry_interactive_folder_scan_task(&task),
            "thumbnail" => self.retry_thumbnail_task(&task),
            other => Err(anyhow::anyhow!(
                "task {task_id} is not retryable; unsupported kind is {other}"
            )),
        }
    }

    fn retry_root_scan_task(&self, task: &TaskRecord) -> anyhow::Result<TaskRecord> {
        let root_id = task
            .root_id
            .ok_or_else(|| anyhow::anyhow!("root_scan task missing root id: {}", task.id))?;
        let root = self
            .get_root(root_id)?
            .ok_or_else(|| anyhow::anyhow!("root not found: {root_id}"))?;
        if !root.enabled {
            return Err(anyhow::anyhow!("root is disabled: {root_id}"));
        }

        let updated = self.connection.execute(
            r#"
            UPDATE tasks
            SET status = 'pending',
                updated_at = ?1,
                attempt_generation = attempt_generation + 1,
                items_seen = 0,
                items_total = NULL,
                folders_seen = 0,
                media_files_seen = 0,
                skipped_files = 0,
                thumbnail_source_fingerprint = NULL,
                error = NULL
            WHERE id = ?2 AND status IN ('failed', 'cancelled')
            "#,
            (unix_timestamp(), task.id),
        )?;
        self.ensure_one_task_updated(task.id, updated, "failed or cancelled")?;
        self.get_task(task.id)?
            .ok_or_else(|| anyhow::anyhow!("task not found: {}", task.id))
    }

    fn retry_interactive_folder_scan_task(&self, task: &TaskRecord) -> anyhow::Result<TaskRecord> {
        let folder_id = task.folder_id.ok_or_else(|| {
            anyhow::anyhow!(
                "interactive_folder_scan task missing folder id: {}",
                task.id
            )
        })?;
        let exists: Option<i64> = self
            .connection
            .query_row(
                r#"
                SELECT folders.id
                FROM folders
                JOIN roots ON roots.id = folders.root_id AND roots.enabled = 1
                WHERE folders.id = ?1 AND folders.status = 'active'
                "#,
                [folder_id],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(anyhow::anyhow!("folder not found: {folder_id}"));
        }

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
            WHERE id = ?3 AND status IN ('failed', 'cancelled')
            "#,
            (ROOT_SCAN_TASK_PRIORITY, unix_timestamp(), task.id),
        )?;
        self.ensure_one_task_updated(task.id, updated, "failed or cancelled")?;
        self.get_task(task.id)?
            .ok_or_else(|| anyhow::anyhow!("task not found: {}", task.id))
    }

    fn retry_thumbnail_task(&self, task: &TaskRecord) -> anyhow::Result<TaskRecord> {
        let file_id = task
            .file_id
            .ok_or_else(|| anyhow::anyhow!("thumbnail task missing file id: {}", task.id))?;
        let source = self
            .get_thumbnail_source(file_id)?
            .ok_or_else(|| anyhow::anyhow!("media item not found: {file_id}"))?;
        let source_fingerprint = source.source_fingerprint(GRID_320_PROFILE);
        let now = unix_timestamp();
        let transaction = self.connection.unchecked_transaction()?;
        let updated = transaction.execute(
            r#"
            UPDATE tasks
            SET status = 'pending',
                updated_at = ?1,
                attempt_generation = attempt_generation + 1,
                items_seen = 0,
                items_total = NULL,
                folders_seen = 0,
                media_files_seen = 0,
                skipped_files = 0,
                thumbnail_source_fingerprint = NULL,
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
            &transaction,
            ThumbnailStateUpsert {
                file_id,
                profile: GRID_320_PROFILE.to_string(),
                state: "queued".to_string(),
                cache_key: None,
                width: None,
                height: None,
                byte_size: None,
                error: None,
                source_fingerprint: Some(source_fingerprint),
            },
        )?;
        transaction.commit()?;
        self.get_task(task.id)?
            .ok_or_else(|| anyhow::anyhow!("task not found: {}", task.id))
    }

    pub fn task_is_cancelled(&self, task_id: i64) -> anyhow::Result<bool> {
        Ok(self
            .get_task(task_id)?
            .map(|task| task.status == "cancelled")
            .unwrap_or(false))
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

    pub fn update_task_scan_progress_for_attempt(
        &self,
        task_id: i64,
        attempt_generation: i64,
        progress: TaskScanProgress,
    ) -> anyhow::Result<()> {
        let updated = self.connection.execute(
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
                unix_timestamp(),
                progress.items_seen,
                progress.items_total,
                progress.folders_seen,
                progress.media_files_seen,
                progress.skipped_files,
                task_id,
                attempt_generation,
            ),
        )?;
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

    pub fn publish_thumbnail_failure_for_attempted_source_for_task_attempt(
        &self,
        file_id: i64,
        profile: &str,
        attempted_source_fingerprint: Option<&str>,
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
        let current_source = thumbnail_source_in_transaction(&transaction, file_id)?
            .ok_or_else(|| anyhow::anyhow!("media item not found: {file_id}"))?;
        let current_source_fingerprint = current_source.source_fingerprint(profile);
        let is_current_attempt =
            attempted_source_fingerprint == Some(current_source_fingerprint.as_str());

        if is_current_attempt {
            upsert_thumbnail_state_in_transaction(
                &transaction,
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
            transaction.commit()?;
            return Ok(true);
        }

        reset_thumbnail_after_stale_source_in_transaction(&transaction, file_id, profile, error)?;
        transaction.commit()?;
        Ok(false)
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
            "#,
            (unix_timestamp(), file_id, profile),
        )?;
        Ok(())
    }

    pub fn get_thumbnail_cache_stats(
        &self,
        scope: ThumbnailCacheScope,
    ) -> anyhow::Result<ThumbnailCacheStats> {
        get_thumbnail_cache_stats_for_scope(self, scope)
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
                if refresh_mode == ThumbnailCacheRefreshMode::RetryFailedAndStale
                    && status == "failed"
                {
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
            if refresh_mode == ThumbnailCacheRefreshMode::RetryFailedAndStale
                && status == ThumbnailCacheStatus::Failed
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
            if refresh_mode == ThumbnailCacheRefreshMode::RetryFailedAndStale
                && status == ThumbnailCacheStatus::Failed
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
            "#,
            (unix_timestamp(), file_id, profile),
        )?;
        Ok(())
    }

    pub fn clear_thumbnail_cache(&self) -> anyhow::Result<ThumbnailCacheClearResult> {
        let transaction = self.connection.unchecked_transaction()?;
        let deleted_blob_count = transaction.execute("DELETE FROM thumb_blobs", [])?;
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
        let _root = self
            .get_root(root_id)?
            .ok_or_else(|| anyhow::anyhow!("root not found: {root_id}"))?;
        let root_folder_id = self.upsert_folder(FolderUpsert {
            root_id,
            parent_id: None,
            name: String::new(),
            path_hash: hash_path(root_path),
            mtime: metadata_time(root_path.metadata().ok().as_ref()),
        })?;
        let Ok(relative) = folder_path.strip_prefix(root_path) else {
            return Err(anyhow::anyhow!(
                "folder path is outside root {}: {}",
                root_path.display(),
                folder_path.display()
            ));
        };
        if relative.as_os_str().is_empty() {
            return Ok(root_folder_id);
        }

        let mut parent_id = root_folder_id;
        let mut current_path = root_path.to_path_buf();
        for component in relative.components() {
            current_path.push(component.as_os_str());
            let name = component.as_os_str().to_string_lossy().to_string();
            parent_id = self.upsert_folder(FolderUpsert {
                root_id,
                parent_id: Some(parent_id),
                name,
                path_hash: hash_path(&current_path),
                mtime: metadata_time(current_path.metadata().ok().as_ref()),
            })?;
        }

        Ok(parent_id)
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
        let file_id = upsert_file_in_transaction(&transaction, file, scan_generation)?.id;
        transaction.commit()?;
        Ok(file_id)
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
            let result = upsert_file_in_transaction(&transaction, file.file, scan_generation)?;
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
            let result = upsert_file_in_transaction(&transaction, file.file, scan_generation)?;
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
        let enabled: i64 = self.connection.query_row(
            "SELECT enabled FROM roots WHERE id = ?1",
            [root_id],
            |row| row.get(0),
        )?;
        Ok(enabled != 0)
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

    fn ensure_one_task_updated(
        &self,
        task_id: i64,
        updated: usize,
        expected_status: &str,
    ) -> anyhow::Result<()> {
        if updated == 1 {
            return Ok(());
        }

        let Some(task) = self.get_task(task_id)? else {
            return Err(anyhow::anyhow!("task not found: {task_id}"));
        };
        Err(anyhow::anyhow!(
            "task {task_id} is not {expected_status}; current status is {}",
            task.status
        ))
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
        Ok(self
            .get_task(task_id)?
            .map(|task| task.attempt_generation == attempt_generation)
            .unwrap_or(false))
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
        let result = self.connection.execute(
            "INSERT INTO tags(name, color) VALUES (?1, ?2)",
            (trimmed, color),
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
        let id = self.connection.last_insert_rowid();
        Ok(TagRecord {
            id,
            name: trimmed.to_string(),
            color: color.map(str::to_string),
        })
    }

    pub fn delete_tag(&self, id: i64) -> anyhow::Result<bool> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
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
        if !self.file_exists(file_id)? {
            return Err(anyhow::anyhow!("media item not found: {file_id}"));
        }
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
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
        sync_media_fts_for_file_in_transaction(&transaction, file_id)?;
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
        let exists: i64 = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM tags WHERE id = ?1)",
            [tag_id],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Err(TagError::UnknownTagId(tag_id));
        }
        transaction.execute(
            "INSERT OR IGNORE INTO file_tags(file_id, tag_id) VALUES (?1, ?2)",
            (file_id, tag_id),
        )?;
        sync_media_fts_for_file_in_transaction(&transaction, file_id)?;
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
        let removed = transaction.execute(
            "DELETE FROM file_tags WHERE file_id = ?1 AND tag_id = ?2",
            (file_id, tag_id),
        )?;
        if removed == 0 {
            return Err(TagError::UnknownTagId(tag_id));
        }
        sync_media_fts_for_file_in_transaction(&transaction, file_id)?;
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
            let count_sql = format!(
                r#"
                SELECT COUNT(*)
                FROM files{fts_join}
                LEFT JOIN media ON media.file_id = files.id
                LEFT JOIN user_metadata ON user_metadata.file_id = files.id
                JOIN roots ON roots.id = files.root_id AND roots.enabled = 1
                WHERE {where_clause}{tag_filter_clause}
                "#,
                fts_join = fts_join,
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

    pub fn list_plugins(&self) -> anyhow::Result<Vec<PluginRecord>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT id, name, version, description, enabled, status,
                   capabilities_json, permissions_json, manifest_path,
                   installed_at, updated_at, last_error
            FROM plugins
            ORDER BY name ASC, id ASC
            "#,
        )?;
        let rows = statement.query_map([], plugin_from_row)?;
        let mut plugins = Vec::new();
        for row in rows {
            plugins.push(row?);
        }
        Ok(plugins)
    }

    pub fn get_plugin(&self, id: &str) -> anyhow::Result<Option<PluginRecord>> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT id, name, version, description, enabled, status,
                   capabilities_json, permissions_json, manifest_path,
                   installed_at, updated_at, last_error
            FROM plugins
            WHERE id = ?1
            "#,
        )?;
        let mut rows = statement.query([id])?;
        match rows.next()? {
            Some(row) => Ok(Some(plugin_from_row(row)?)),
            None => Ok(None),
        }
    }

    /// Insert or update a plugin row, preserving the existing `enabled` flag
    /// when the row already exists.
    pub fn upsert_plugin(&self, upsert: PluginUpsert) -> anyhow::Result<PluginRecord> {
        let now = unix_timestamp();
        let capabilities_json = serde_json::to_string(&upsert.capabilities)?;
        let permissions_json = serde_json::to_string(&upsert.permissions)?;
        let transaction = self.connection.unchecked_transaction()?;

        let existing_enabled: Option<i64> = transaction
            .query_row(
                "SELECT enabled FROM plugins WHERE id = ?1",
                [&upsert.id],
                |row| row.get(0),
            )
            .optional()?;

        match existing_enabled {
            Some(enabled) => {
                transaction.execute(
                    r#"
                    UPDATE plugins
                    SET name = ?2,
                        version = ?3,
                        description = ?4,
                        status = ?5,
                        capabilities_json = ?6,
                        permissions_json = ?7,
                        manifest_path = ?8,
                        last_error = ?9,
                        enabled = ?10,
                        updated_at = ?11
                    WHERE id = ?1
                    "#,
                    rusqlite::params![
                        upsert.id,
                        upsert.name,
                        upsert.version,
                        upsert.description,
                        upsert.status,
                        capabilities_json,
                        permissions_json,
                        upsert.manifest_path,
                        upsert.last_error,
                        enabled,
                        now,
                    ],
                )?;
            }
            None => {
                transaction.execute(
                    r#"
                    INSERT INTO plugins(
                        id, name, version, description, enabled, status,
                        capabilities_json, permissions_json, manifest_path,
                        last_error, installed_at, updated_at
                    )
                    VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
                    "#,
                    rusqlite::params![
                        upsert.id,
                        upsert.name,
                        upsert.version,
                        upsert.description,
                        upsert.status,
                        capabilities_json,
                        permissions_json,
                        upsert.manifest_path,
                        upsert.last_error,
                        now,
                    ],
                )?;
            }
        }

        let record = transaction.query_row(
            r#"
            SELECT id, name, version, description, enabled, status,
                   capabilities_json, permissions_json, manifest_path,
                   installed_at, updated_at, last_error
            FROM plugins
            WHERE id = ?1
            "#,
            [&upsert.id],
            plugin_from_row,
        )?;
        transaction.commit()?;
        Ok(record)
    }

    /// Toggle the enabled flag for an existing plugin row. Returns `None`
    /// when no plugin with that id exists.
    pub fn set_plugin_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> anyhow::Result<Option<PluginRecord>> {
        let now = unix_timestamp();
        let updated = self.connection.execute(
            r#"
            UPDATE plugins
            SET enabled = ?2,
                updated_at = ?3
            WHERE id = ?1
            "#,
            rusqlite::params![id, enabled as i64, now],
        )?;
        if updated == 0 {
            return Ok(None);
        }
        self.get_plugin(id)
    }

    pub fn set_plugin_status(
        &self,
        id: &str,
        status: &str,
        last_error: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = unix_timestamp();
        self.connection.execute(
            r#"
            UPDATE plugins
            SET status = ?2,
                last_error = ?3,
                updated_at = ?4
            WHERE id = ?1
            "#,
            rusqlite::params![id, status, last_error, now],
        )?;
        Ok(())
    }

    pub fn delete_plugin(&self, id: &str) -> anyhow::Result<bool> {
        let deleted = self
            .connection
            .execute("DELETE FROM plugins WHERE id = ?1", [id])?;
        Ok(deleted > 0)
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
    let enabled: i64 = transaction.query_row(
        "SELECT enabled FROM roots WHERE id = ?1",
        [root_id],
        |row| row.get(0),
    )?;
    if enabled != 0 {
        return Ok(());
    }
    Err(anyhow::anyhow!("root is disabled: {root_id}"))
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
        SELECT status, attempt_generation
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
    Ok(status == "running" && current_attempt == attempt_generation)
}

fn hash_path(path: &Path) -> String {
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

fn cancel_root_scan_tasks_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    root_id: i64,
) -> anyhow::Result<usize> {
    let updated = transaction.execute(
        r#"
        UPDATE tasks
        SET status = 'failed',
            updated_at = ?1,
            error = 'root is disabled'
        WHERE kind IN ('root_scan', 'interactive_folder_scan')
          AND root_id = ?2
          AND status IN ('pending', 'running')
        "#,
        (unix_timestamp(), root_id),
    )?;
    Ok(updated)
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
              AND size = ?3
              AND mtime = ?4
              AND status IN ('active', 'missing')
              AND (folder_id <> ?5 OR name <> ?6)
            ORDER BY CASE status WHEN 'active' THEN 0 ELSE 1 END, id DESC
            LIMIT 1
            "#,
            (
                file.root_id,
                file_key,
                file.size,
                file.mtime,
                file.folder_id,
                &file.name,
            ),
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
}

fn upsert_file_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    file: FileUpsert,
    scan_generation: Option<i64>,
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
        reusable_file_id_for_content_identity_in_transaction(transaction, &file)?
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

fn is_safe_thumbnail_cache_key(cache_key: &str) -> bool {
    is_safe_cache_key(cache_key)
}

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
            "cache_status IN ('missing', 'stale', 'failed')".to_string()
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
                {status_expression} AS cache_status
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
        _ => false,
    }
}

fn get_thumbnail_cache_stats_for_scope(
    database: &Database,
    scope: ThumbnailCacheScope,
) -> anyhow::Result<ThumbnailCacheStats> {
    let (stats_sql, stats_params) = thumbnail_cache_stats_query(scope);
    let (cached_count, missing_count, stale_count, failed_count, total_blob_bytes) = database
        .connection
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
        database
            .connection
            .query_row(&active_sql, params_from_iter(active_params), |row| {
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
        error: row.get(16)?,
    })
}

fn plugin_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PluginRecord> {
    let capabilities_json: String = row.get(6)?;
    let permissions_json: String = row.get(7)?;
    let capabilities =
        serde_json::from_str::<Vec<String>>(&capabilities_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    let permissions = serde_json::from_str::<Vec<String>>(&permissions_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let enabled: i64 = row.get(4)?;
    Ok(PluginRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row.get(2)?,
        description: row.get(3)?,
        enabled: enabled != 0,
        status: row.get(5)?,
        capabilities,
        permissions,
        manifest_path: row.get(8)?,
        installed_at: row.get(9)?,
        updated_at: row.get(10)?,
        last_error: row.get(11)?,
    })
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

fn unix_timestamp() -> i64 {
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
    fn migrations_upgrade_old_initial_tasks_table_with_progress_columns() {
        let database = Database::open_in_memory().expect("open in-memory database");
        database
            .connection
            .execute_batch(OLD_INITIAL_MIGRATION_WITHOUT_TASK_PROGRESS)
            .expect("apply old initial migration");

        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root on old schema");
        database
            .create_root_scan_task(root_id)
            .expect("create task on old schema");

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
        let root_id = database
            .add_root(NewRoot {
                path: "D:/Pictures".to_string(),
                display_name: "Pictures".to_string(),
            })
            .expect("add root before v8");
        let task_id = database
            .create_root_scan_task(root_id)
            .expect("create task before v8");

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
    fn readding_disabled_root_does_not_resurrect_stale_index_rows_before_scan() {
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
        assert!(roots[0].root_folder_id.is_none());

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

    fn sample_plugin_upsert(id: &str) -> PluginUpsert {
        PluginUpsert {
            id: id.to_string(),
            name: format!("Plugin {id}"),
            version: "1.0.0".to_string(),
            description: Some("test plugin".to_string()),
            status: "registered".to_string(),
            capabilities: vec!["decoder".to_string(), "metadata".to_string()],
            permissions: vec!["read-media-file".to_string()],
            manifest_path: format!("D:/plugins/{id}/plugin.json"),
            last_error: None,
        }
    }

    #[test]
    fn plugin_crud_round_trip() {
        let database = migrated_database();

        assert!(
            database.list_plugins().expect("list plugins").is_empty(),
            "list_plugins must start empty"
        );

        let inserted = database
            .upsert_plugin(sample_plugin_upsert("com.example.alpha"))
            .expect("upsert alpha");
        assert_eq!(inserted.id, "com.example.alpha");
        assert!(!inserted.enabled, "newly inserted plugin starts disabled");
        assert_eq!(inserted.status, "registered");
        assert_eq!(
            inserted.capabilities,
            vec!["decoder".to_string(), "metadata".to_string()]
        );
        assert_eq!(inserted.permissions, vec!["read-media-file".to_string()]);

        let listed = database.list_plugins().expect("list plugins");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "com.example.alpha");

        let fetched = database
            .get_plugin("com.example.alpha")
            .expect("get plugin")
            .expect("plugin row exists");
        assert_eq!(fetched.id, "com.example.alpha");
        assert!(database
            .get_plugin("com.example.missing")
            .expect("get missing plugin")
            .is_none());

        let toggled = database
            .set_plugin_enabled("com.example.alpha", true)
            .expect("set enabled")
            .expect("plugin row exists");
        assert!(toggled.enabled);
        assert!(database
            .set_plugin_enabled("com.example.missing", true)
            .expect("set missing enabled")
            .is_none());

        database
            .set_plugin_status("com.example.alpha", "invalid", Some("manifest broke"))
            .expect("set status");
        let after_status = database
            .get_plugin("com.example.alpha")
            .expect("get plugin after status")
            .expect("plugin row exists");
        assert_eq!(after_status.status, "invalid");
        assert_eq!(after_status.last_error.as_deref(), Some("manifest broke"));

        assert!(database.delete_plugin("com.example.alpha").expect("delete"));
        assert!(!database
            .delete_plugin("com.example.alpha")
            .expect("delete missing"));
        assert!(database.list_plugins().expect("list plugins").is_empty());
    }

    #[test]
    fn plugin_upsert_preserves_enabled_when_row_exists() {
        let database = migrated_database();

        let mut upsert = sample_plugin_upsert("com.example.beta");
        upsert.status = "registered".to_string();
        let initial = database.upsert_plugin(upsert.clone()).expect("upsert beta");
        assert!(!initial.enabled);

        let toggled = database
            .set_plugin_enabled("com.example.beta", true)
            .expect("enable beta")
            .expect("plugin row exists");
        assert!(toggled.enabled);

        // Re-upsert (e.g., from a discovery pass) must not flip the enabled flag.
        let mut updated = upsert.clone();
        updated.name = "Plugin Beta v2".to_string();
        updated.status = "registered".to_string();
        let after = database.upsert_plugin(updated).expect("re-upsert beta");
        assert!(after.enabled, "enabled must persist across upserts");
        assert_eq!(after.name, "Plugin Beta v2");
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
}
