pub const INITIAL_MIGRATION: &str = include_str!("../../migrations/0001_initial.sql");
pub const TASK_PROGRESS_MIGRATION: &str = include_str!("../../migrations/0002_task_progress.sql");
pub const BROWSING_INDEXES_MIGRATION: &str =
    include_str!("../../migrations/0003_browsing_indexes.sql");
pub const THUMBNAIL_STATE_MIGRATION: &str =
    include_str!("../../migrations/0004_thumbnail_state.sql");
pub const THUMBNAIL_SOURCE_FINGERPRINT_MIGRATION: &str =
    include_str!("../../migrations/0005_thumbnail_source_fingerprint.sql");
pub const THUMBNAIL_TASK_ATTEMPT_FINGERPRINT_MIGRATION: &str =
    include_str!("../../migrations/0006_thumbnail_task_attempt_fingerprint.sql");
pub const TASK_STATUS_CONTRACT_MIGRATION: &str =
    include_str!("../../migrations/0007_task_status_contract.sql");
#[allow(dead_code)]
pub const TASK_ATTEMPT_GENERATION_MIGRATION: &str =
    include_str!("../../migrations/0008_task_attempt_generation.sql");
pub const SCAN_RECONCILIATION_MIGRATION: &str =
    include_str!("../../migrations/0009_scan_reconciliation.sql");
pub const MEDIA_FTS_CONTENTLESS_DELETE_MIGRATION: &str =
    include_str!("../../migrations/0010_media_fts_contentless_delete.sql");
#[allow(dead_code)]
pub const PLUGINS_EXTENDED_MIGRATION: &str =
    include_str!("../../migrations/0011_plugins_extended.sql");
pub const PREVIEW_PIPELINE_REFACTOR_MIGRATION: &str =
    include_str!("../../migrations/0012_preview_pipeline_refactor.sql");
pub const PREVIEW_SERVED_BY_MIGRATION: &str =
    include_str!("../../migrations/0013_preview_served_by.sql");
pub const ROOT_STATUS_BROWSING_INDEXES_MIGRATION: &str =
    include_str!("../../migrations/0015_root_status_browsing_indexes.sql");
pub const FOLDER_STATUS_BROWSING_INDEXES_MIGRATION: &str =
    include_str!("../../migrations/0016_folder_status_browsing_indexes.sql");
pub const THUMBNAIL_TASK_INDEXES_MIGRATION: &str =
    include_str!("../../migrations/0017_thumbnail_task_indexes.sql");
pub const THUMB_FAILURE_KIND_MIGRATION: &str =
    include_str!("../../migrations/0018_thumb_failure_kind.sql");
pub const ROOT_PENDING_DELETE_MIGRATION: &str =
    include_str!("../../migrations/0019_root_pending_delete.sql");
pub const ROOT_PURGE_TASK_KIND_MIGRATION: &str =
    include_str!("../../migrations/0020_root_purge_task_kind.sql");
pub const NOTIFICATION_OUTBOX_MIGRATION: &str =
    include_str!("../../migrations/0021_notification_outbox.sql");
pub const FILE_OPERATION_JOURNAL_MIGRATION: &str =
    include_str!("../../migrations/0022_file_operation_journal.sql");
pub const FILE_OPERATION_RECOVERY_METADATA_MIGRATION: &str =
    include_str!("../../migrations/0023_file_operation_recovery_metadata.sql");
pub const FILE_OPERATION_OBJECT_BINDING_MIGRATION: &str =
    include_str!("../../migrations/0024_file_operation_object_binding.sql");
pub const PREVIEW_BLOBS_MIGRATION: &str = include_str!("../../migrations/0025_preview_blobs.sql");
pub const PREVIEW_FAILURES_MIGRATION: &str =
    include_str!("../../migrations/0026_preview_failures.sql");
pub const SYNC_PROTOCOL_FOUNDATION_MIGRATION: &str =
    include_str!("../../migrations/0027_sync_protocol_foundation.sql");
pub const REMOVE_CATALOG_PLUGINS_MIGRATION: &str =
    include_str!("../../migrations/0028_remove_catalog_plugins.sql");
pub const MEDIA_OBJECT_STORAGE_MIGRATION: &str =
    include_str!("../../migrations/0029_media_object_storage.sql");
pub const SYNC_ROOT_IDENTITY_MIGRATION: &str =
    include_str!("../../migrations/0030_sync_root_identity.sql");
pub const SYNC_ROOT_GENERATION_FENCE_MIGRATION: &str =
    include_str!("../../migrations/0031_sync_root_generation_fence.sql");
