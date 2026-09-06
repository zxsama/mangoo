-- Durable, expiring filesystem evidence used by enrollment, aggregate
-- selection and matching-marker rebind commits.
BEGIN IMMEDIATE;

CREATE TABLE root_path_preflights (
  preflight_token TEXT PRIMARY KEY,
  library_uuid TEXT NOT NULL REFERENCES libraries(library_uuid) ON DELETE CASCADE,
  root_uuid TEXT,
  mode TEXT NOT NULL CHECK(mode IN ('enroll-existing','materialize-remote','rebind-existing')),
  request_hash TEXT NOT NULL CHECK(length(request_hash)=64),
  request_json TEXT NOT NULL CHECK(json_valid(request_json)),
  result_json TEXT NOT NULL CHECK(json_valid(result_json)),
  expires_at INTEGER NOT NULL,
  consumed_at INTEGER,
  created_at INTEGER NOT NULL
);

CREATE INDEX idx_root_path_preflights_expiry
  ON root_path_preflights(library_uuid, expires_at, preflight_token);

UPDATE host_meta SET schema_version = 14 WHERE singleton = 1 AND schema_version < 14;

COMMIT;
