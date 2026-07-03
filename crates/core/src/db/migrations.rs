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
