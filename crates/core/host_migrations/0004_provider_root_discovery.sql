-- Durable account-root discovery state. Root paging is separate from every
-- Library cursor, and discovered descriptors are transport inventory only.
BEGIN IMMEDIATE;

ALTER TABLE provider_account_root_cursors ADD COLUMN page_token TEXT;

CREATE TABLE provider_remote_libraries (
  account_uuid TEXT NOT NULL REFERENCES plugin_accounts(account_uuid) ON DELETE CASCADE,
  library_uuid TEXT NOT NULL,
  descriptor_size INTEGER NOT NULL CHECK(descriptor_size >= 0),
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  descriptor_etag TEXT,
  account_generation INTEGER NOT NULL CHECK(account_generation >= 1),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(account_uuid, library_uuid)
);

CREATE INDEX idx_provider_remote_libraries_updated
  ON provider_remote_libraries(account_uuid, updated_at, library_uuid);

UPDATE host_meta SET schema_version = 4 WHERE singleton = 1 AND schema_version < 4;

COMMIT;
