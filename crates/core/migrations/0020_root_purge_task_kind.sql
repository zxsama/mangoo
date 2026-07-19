-- Hard-delete a folder (root): the background purge is a new `root_purge`
-- task kind (see docs/superpowers/specs/2026-07-19-library-folder-hard-delete-design.md
-- and Database::mark_root_for_purge). SQLite has no ALTER TABLE to broaden a
-- CHECK constraint, so the table is rebuilt in place, same shape as migration
-- 0007 / the interactive_folder_scan_task_contract migration (applied in code
-- as migration 14) that it follows.
PRAGMA foreign_keys = OFF;

DROP TABLE IF EXISTS tasks_new;

BEGIN IMMEDIATE;

CREATE TABLE tasks_new (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL CHECK(kind IN ('root_scan', 'interactive_folder_scan', 'thumbnail', 'root_purge')),
  priority INTEGER NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending', 'running', 'succeeded', 'failed', 'cancelled')),
  root_id INTEGER REFERENCES roots(id) ON DELETE SET NULL,
  folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL,
  file_id INTEGER REFERENCES files(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  items_seen INTEGER NOT NULL DEFAULT 0,
  items_total INTEGER,
  folders_seen INTEGER NOT NULL DEFAULT 0,
  media_files_seen INTEGER NOT NULL DEFAULT 0,
  skipped_files INTEGER NOT NULL DEFAULT 0,
  thumbnail_source_fingerprint TEXT,
  attempt_generation INTEGER NOT NULL DEFAULT 0,
  error TEXT
);

INSERT INTO tasks_new(
  id,
  kind,
  priority,
  status,
  root_id,
  folder_id,
  file_id,
  created_at,
  updated_at,
  items_seen,
  items_total,
  folders_seen,
  media_files_seen,
  skipped_files,
  thumbnail_source_fingerprint,
  attempt_generation,
  error
)
SELECT
  id,
  kind,
  priority,
  status,
  root_id,
  folder_id,
  file_id,
  created_at,
  updated_at,
  items_seen,
  items_total,
  folders_seen,
  media_files_seen,
  skipped_files,
  thumbnail_source_fingerprint,
  attempt_generation,
  error
FROM tasks;

DROP TABLE tasks;
ALTER TABLE tasks_new RENAME TO tasks;

-- Recreate every index the rebuilt table lost (indexes are dropped along with
-- their table): idx_tasks_status_priority (migration 0007) and the two
-- kind-scoped indexes (migration 0017).
CREATE INDEX IF NOT EXISTS idx_tasks_status_priority ON tasks(status, priority, created_at);
CREATE INDEX IF NOT EXISTS idx_tasks_kind_file_status ON tasks(kind, file_id, status);
CREATE INDEX IF NOT EXISTS idx_tasks_kind_status_priority ON tasks(kind, status, priority);

CREATE TEMP TABLE root_purge_task_kind_fk_check(count INTEGER CHECK(count = 0));
INSERT INTO root_purge_task_kind_fk_check
SELECT COUNT(*) FROM pragma_foreign_key_check;
DROP TABLE root_purge_task_kind_fk_check;

INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
VALUES (20, 'root_purge_task_kind', unixepoch());

COMMIT;

PRAGMA foreign_keys = ON;
