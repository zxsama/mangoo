-- Negative cache for preview representations that cannot be decoded.
--
-- This is deliberately separate from preview_blobs. Its multi-megabyte data
-- column must remain last for SQLite overflow-page performance, and rebuilding
-- that table would copy up to 2 GB of regenerable cache during app startup.
BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS preview_failures (
  file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  profile TEXT NOT NULL CHECK(profile IN ('preview_2048','preview_3072','preview_native')),
  failure_kind TEXT NOT NULL CHECK(failure_kind IN ('permanent','transient')),
  error TEXT NOT NULL,
  source_size INTEGER NOT NULL,
  source_mtime INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(file_id, profile)
);

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (26, 'preview_failures', unixepoch());

COMMIT;
