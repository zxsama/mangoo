-- Runtime identity is independent of account identity and Library transport.
BEGIN IMMEDIATE;
ALTER TABLE plugin_installations ADD COLUMN lifecycle_epoch INTEGER NOT NULL DEFAULT 1 CHECK(lifecycle_epoch >= 1);
ALTER TABLE plugin_runtime_state ADD COLUMN stop_pending INTEGER NOT NULL DEFAULT 0 CHECK(stop_pending IN (0,1));
ALTER TABLE library_sync_bindings ADD COLUMN plugin_suspended_state TEXT;
ALTER TABLE library_sync_bindings ADD COLUMN pause_resume_state TEXT;
-- Existing pauses have no proven origin. Never automatically resume them.
UPDATE host_meta SET schema_version=19 WHERE singleton=1 AND schema_version<19;
COMMIT;
