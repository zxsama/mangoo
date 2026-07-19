BEGIN IMMEDIATE;

CREATE TABLE notification_operation_heads (
  operation_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','running','succeeded','failed','cancelled')),
  presentation TEXT NOT NULL CHECK(presentation IN ('important','history_only','status_only')),
  revision INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  dirty INTEGER NOT NULL DEFAULT 1 CHECK(dirty IN (0,1)),
  last_outbox_at INTEGER,
  updated_at INTEGER NOT NULL,
  completed_at INTEGER
);

CREATE TABLE notification_outbox (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  operation_id TEXT NOT NULL,
  revision INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  UNIQUE(operation_id, revision)
);

ALTER TABLE file_operations ADD COLUMN operation_group_id TEXT;

CREATE INDEX idx_notification_heads_status_updated
  ON notification_operation_heads(status, updated_at DESC);
CREATE INDEX idx_notification_outbox_operation_revision
  ON notification_outbox(operation_id, revision);
CREATE INDEX idx_file_operations_group
  ON file_operations(operation_group_id, id);

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (21, 'notification_outbox', unixepoch());

COMMIT;
