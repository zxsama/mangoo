BEGIN IMMEDIATE;

-- The path is planned before create_new; a NULL identity is pending evidence,
-- never authority to exclude or delete an existing filesystem object.
ALTER TABLE media_materialization_jobs ADD COLUMN staging_directory_identity TEXT;
ALTER TABLE media_materialization_jobs ADD COLUMN staging_file_identity TEXT;
CREATE INDEX idx_materialization_staging_path ON media_materialization_jobs(staging_relpath)
  WHERE staging_relpath IS NOT NULL;
CREATE INDEX idx_file_operations_owned_staging ON file_operations(root_id,target_path)
  WHERE operation='delete_recycle' AND staging_identity IS NOT NULL;

INSERT INTO schema_migrations(version,name,applied_at)
VALUES(35,'materialization_staging_ownership',unixepoch());
COMMIT;
