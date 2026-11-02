BEGIN IMMEDIATE;

-- Local policy survives pause, provider disable and local-only forks. A prior
-- completed media scan cannot satisfy an all-file policy upgrade.
ALTER TABLE roots ADD COLUMN file_collection_version INTEGER NOT NULL DEFAULT 0 CHECK(file_collection_version IN (0,1));
ALTER TABLE roots ADD COLUMN file_collection_scanned_version INTEGER NOT NULL DEFAULT 0 CHECK(file_collection_scanned_version IN (0,1));
UPDATE roots SET file_collection_version=1 WHERE root_folder_uuid IS NOT NULL OR EXISTS(SELECT 1 FROM sync_root_local_bindings b WHERE b.root_id=roots.id);
CREATE TRIGGER roots_all_file_policy_link AFTER UPDATE OF root_folder_uuid ON roots
WHEN NEW.root_folder_uuid IS NOT NULL
BEGIN UPDATE roots SET file_collection_version=1 WHERE id=NEW.id; END;
CREATE TRIGGER roots_all_file_policy_binding AFTER INSERT ON sync_root_local_bindings
BEGIN UPDATE roots SET file_collection_version=1 WHERE id=NEW.root_id; END;

-- One identity per directory entry; simultaneous hard links may share OS IDs.
ALTER TABLE sync_local_file_identities RENAME TO sync_local_file_identities_old;
CREATE TABLE sync_local_file_identities (
  volume_identity BLOB NOT NULL,
  file_identity BLOB NOT NULL,
  asset_uuid BLOB PRIMARY KEY NOT NULL CHECK(length(asset_uuid)=16),
  last_path TEXT NOT NULL,
  content_fingerprint BLOB,
  last_seen_at INTEGER NOT NULL
);
INSERT INTO sync_local_file_identities SELECT volume_identity,file_identity,asset_uuid,last_path,content_fingerprint,last_seen_at FROM sync_local_file_identities_old;
DROP TABLE sync_local_file_identities_old;
CREATE INDEX idx_sync_file_os_identity ON sync_local_file_identities(volume_identity,file_identity);
CREATE INDEX idx_sync_file_fingerprint ON sync_local_file_identities(volume_identity,content_fingerprint) WHERE content_fingerprint IS NOT NULL;
CREATE INDEX idx_sync_file_last_path ON sync_local_file_identities(last_path);
ALTER TABLE sync_local_file_identities ADD COLUMN source_stamp BLOB;

-- Selective local presentation indexes avoid scanning arbitrary numbers of
-- generic files to find a media page. Classification is still owned by media.
ALTER TABLE files ADD COLUMN browse_visible INTEGER NOT NULL DEFAULT 0 CHECK(browse_visible IN (0,1));
UPDATE files SET browse_visible=1 WHERE EXISTS(SELECT 1 FROM media WHERE media.file_id=files.id);
CREATE TRIGGER media_browse_insert AFTER INSERT ON media BEGIN UPDATE files SET browse_visible=1 WHERE id=NEW.file_id; END;
CREATE TRIGGER media_browse_delete AFTER DELETE ON media BEGIN UPDATE files SET browse_visible=0 WHERE id=OLD.file_id; DELETE FROM media_fts WHERE rowid=OLD.file_id; END;
CREATE INDEX idx_files_browse_root_name ON files(root_id,status,name,id) WHERE browse_visible=1;
CREATE INDEX idx_files_browse_root_mtime ON files(root_id,status,mtime,id) WHERE browse_visible=1;
CREATE INDEX idx_files_browse_folder_name ON files(folder_id,status,name,id) WHERE browse_visible=1;
CREATE INDEX idx_files_browse_folder_mtime ON files(folder_id,status,mtime,id) WHERE browse_visible=1;
CREATE INDEX idx_files_browse_name ON files(status,name,id) WHERE browse_visible=1;
CREATE INDEX idx_files_browse_mtime ON files(status,mtime,id) WHERE browse_visible=1;

CREATE TABLE sync_file_seed_progress (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 after_file_id INTEGER NOT NULL DEFAULT 0,
 through_file_id INTEGER NOT NULL
);

INSERT INTO schema_migrations(version,name,applied_at) VALUES(34,'all_file_collection',unixepoch());
COMMIT;
