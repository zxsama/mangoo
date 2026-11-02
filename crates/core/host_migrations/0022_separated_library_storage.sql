-- Old journals keep layout 1 and never grant authority for separated storage.
BEGIN IMMEDIATE;
ALTER TABLE library_creation_journal ADD COLUMN layout_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE library_creation_journal ADD COLUMN final_storage_path TEXT;
ALTER TABLE library_creation_journal ADD COLUMN content_roots_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE library_creation_journal ADD COLUMN owned_artifacts_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE library_removal_journal ADD COLUMN layout_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE sync_restore_plans ADD COLUMN database_path TEXT;
ALTER TABLE sync_restore_plans ADD COLUMN placement_base TEXT;
ALTER TABLE sync_restore_plans ADD COLUMN content_paths_json TEXT NOT NULL DEFAULT '[]';
UPDATE host_meta SET schema_version = 22 WHERE singleton = 1;
COMMIT;
