-- Thumbnail task lookups and scheduling were effectively unindexed: the only
-- task index was idx_tasks_status_priority(status, priority, created_at), which
-- cannot serve the per-file lookups (kind + file_id + status) that
-- request_thumbnail_task runs 3-4x per file, nor the worker scheduler's
-- kind + status + priority scans. On million-file libraries those degrade to
-- large scans/sorts of the tasks table on the hot grid + worker paths.
CREATE INDEX IF NOT EXISTS idx_tasks_kind_file_status ON tasks(kind, file_id, status);
CREATE INDEX IF NOT EXISTS idx_tasks_kind_status_priority ON tasks(kind, status, priority);

INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
VALUES (17, 'thumbnail_task_indexes', unixepoch());
