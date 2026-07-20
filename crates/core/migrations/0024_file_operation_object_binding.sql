BEGIN IMMEDIATE;

ALTER TABLE file_operations ADD COLUMN target_parent_identity TEXT;
ALTER TABLE file_operations ADD COLUMN staging_identity TEXT;
ALTER TABLE file_operations ADD COLUMN recycle_phase TEXT NOT NULL DEFAULT 'none'
  CHECK(recycle_phase IN ('none','staged','shell_completed'));

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (24, 'file_operation_object_binding', unixepoch());

COMMIT;
