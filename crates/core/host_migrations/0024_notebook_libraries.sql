BEGIN IMMEDIATE;

ALTER TABLE libraries ADD COLUMN kind TEXT NOT NULL DEFAULT 'media' CHECK(kind IN ('media','notes'));
ALTER TABLE library_creation_journal ADD COLUMN kind TEXT NOT NULL DEFAULT 'media' CHECK(kind IN ('media','notes'));
ALTER TABLE library_open_journal ADD COLUMN kind TEXT NOT NULL DEFAULT 'media' CHECK(kind IN ('media','notes'));
ALTER TABLE provider_remote_libraries ADD COLUMN kind TEXT NOT NULL DEFAULT 'media' CHECK(kind IN ('media','notes'));

ALTER TABLE sync_restore_plans ADD COLUMN kind TEXT NOT NULL DEFAULT 'media' CHECK(kind IN ('media','notes'));

UPDATE host_meta SET schema_version = 24 WHERE singleton = 1;
COMMIT;
