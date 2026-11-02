BEGIN IMMEDIATE;

-- Ordinary file collection is independent of sync enrollment.
UPDATE roots SET file_collection_version=1;
CREATE TRIGGER roots_all_file_policy_insert AFTER INSERT ON roots
BEGIN UPDATE roots SET file_collection_version=1 WHERE id=NEW.id; END;

-- Keep the optional media projection selective without removing file search.
DROP TRIGGER media_browse_delete;
CREATE TRIGGER media_browse_delete AFTER DELETE ON media
BEGIN UPDATE files SET browse_visible=0 WHERE id=OLD.file_id; END;

-- A durable keyset cursor, rebuilt in bounded writer turns by the runtime.
CREATE TABLE file_browse_fts_progress (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
  after_file_id INTEGER NOT NULL DEFAULT 0,
  through_file_id INTEGER NOT NULL,
  completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1))
);
INSERT INTO file_browse_fts_progress(singleton,through_file_id)
SELECT 1,COALESCE(MAX(id),0) FROM files;

ALTER TABLE tasks ADD COLUMN files_seen INTEGER NOT NULL DEFAULT 0;
UPDATE tasks SET files_seen=media_files_seen;

INSERT INTO schema_migrations(version,name,applied_at) VALUES(36,'all_file_browsing',unixepoch());
COMMIT;
