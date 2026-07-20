BEGIN IMMEDIATE;

ALTER TABLE file_operations ADD COLUMN entity_kind TEXT
  CHECK(entity_kind IN ('file','folder'));
ALTER TABLE file_operations ADD COLUMN entity_id INTEGER;
ALTER TABLE file_operations ADD COLUMN target_folder_id INTEGER;
ALTER TABLE file_operations ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 0
  CHECK(attempt_count >= 0);
ALTER TABLE file_operations ADD COLUMN error_code TEXT;

CREATE INDEX idx_file_operations_pending
  ON file_operations(id)
  WHERE status = 'pending';

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (22, 'file_operation_journal', unixepoch());

COMMIT;
