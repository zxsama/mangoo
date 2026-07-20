BEGIN IMMEDIATE;

ALTER TABLE file_operations ADD COLUMN root_id INTEGER;
ALTER TABLE file_operations ADD COLUMN journal_phase TEXT NOT NULL DEFAULT 'legacy'
  CHECK(journal_phase IN ('legacy','prepared','executing'));
ALTER TABLE file_operations ADD COLUMN source_identity TEXT;

DROP INDEX idx_file_operations_pending;
CREATE INDEX idx_file_operations_pending
  ON file_operations(id)
  WHERE status = 'pending' AND operation_group_id IS NOT NULL;

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (23, 'file_operation_recovery_metadata', unixepoch());

COMMIT;
