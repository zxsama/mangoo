-- Cached remote root inventory discovered from Library descriptors.
BEGIN IMMEDIATE;

CREATE TABLE provider_remote_sync_roots (
  account_uuid TEXT NOT NULL,
  library_uuid TEXT NOT NULL,
  root_uuid TEXT NOT NULL,
  root_folder_uuid TEXT NOT NULL,
  requested_name TEXT NOT NULL,
  materialized_name TEXT NOT NULL,
  materialization_fingerprint TEXT NOT NULL CHECK(length(materialization_fingerprint) = 64),
  logical_revision INTEGER NOT NULL CHECK(logical_revision >= 0),
  logical_bytes INTEGER CHECK(logical_bytes IS NULL OR logical_bytes >= 0),
  compatible INTEGER NOT NULL DEFAULT 1 CHECK(compatible IN (0,1)),
  error_code TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(account_uuid, library_uuid, root_uuid),
  FOREIGN KEY(account_uuid, library_uuid)
    REFERENCES provider_remote_libraries(account_uuid, library_uuid) ON DELETE CASCADE
);

CREATE INDEX idx_provider_remote_sync_roots_page
  ON provider_remote_sync_roots(account_uuid, library_uuid, root_uuid);

UPDATE host_meta SET schema_version = 16 WHERE singleton = 1 AND schema_version < 16;

COMMIT;
