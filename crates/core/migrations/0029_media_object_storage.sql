-- Content-addressed original-media inventory and durable publication/restore jobs.
-- Immutable object bytes remain in managed files; Catalog stores only hashes,
-- bounded ranges, relative managed paths and state-machine cursors.
BEGIN IMMEDIATE;

ALTER TABLE asset_content_revisions ADD COLUMN source_mtime_ms INTEGER;

CREATE TABLE media_chunks (
  chunk_hash BLOB PRIMARY KEY CHECK(length(chunk_hash) = 32),
  byte_length INTEGER NOT NULL CHECK(byte_length > 0),
  pack_hash BLOB CHECK(pack_hash IS NULL OR length(pack_hash) = 32),
  pack_offset INTEGER CHECK(pack_offset IS NULL OR pack_offset >= 0),
  state TEXT NOT NULL CHECK(state IN ('pending','sealed','remote','corrupt')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK((pack_hash IS NULL) = (pack_offset IS NULL))
);

CREATE TABLE media_packs (
  pack_hash BLOB PRIMARY KEY CHECK(length(pack_hash) = 32),
  object_key TEXT NOT NULL UNIQUE,
  byte_length INTEGER NOT NULL CHECK(byte_length > 0),
  local_relpath TEXT,
  state TEXT NOT NULL CHECK(state IN ('building','sealed','uploading','durable','remote','corrupt','orphan')),
  receipt_generation INTEGER CHECK(receipt_generation IS NULL OR receipt_generation >= 0),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK(local_relpath IS NULL OR (length(local_relpath) BETWEEN 1 AND 1024 AND substr(local_relpath,1,1) NOT IN ('/','\\') AND instr(local_relpath,'..') = 0))
);

CREATE TABLE media_manifests (
  manifest_hash BLOB PRIMARY KEY CHECK(length(manifest_hash) = 32),
  content_hash BLOB NOT NULL CHECK(length(content_hash) = 32),
  byte_length INTEGER NOT NULL CHECK(byte_length >= 0),
  manifest_pack_hash BLOB CHECK(manifest_pack_hash IS NULL OR length(manifest_pack_hash) = 32),
  pack_offset INTEGER CHECK(pack_offset IS NULL OR pack_offset >= 0),
  record_length INTEGER CHECK(record_length IS NULL OR record_length > 0),
  state TEXT NOT NULL CHECK(state IN ('sealed','durable','remote','corrupt')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK((manifest_pack_hash IS NULL) = (pack_offset IS NULL)),
  CHECK((manifest_pack_hash IS NULL) = (record_length IS NULL))
);

CREATE TABLE media_content_manifest_links (
  content_revision_uuid BLOB PRIMARY KEY REFERENCES asset_content_revisions(content_revision_uuid) ON DELETE RESTRICT CHECK(length(content_revision_uuid) = 16),
  manifest_hash BLOB NOT NULL REFERENCES media_manifests(manifest_hash) ON DELETE RESTRICT CHECK(length(manifest_hash) = 32),
  linked_at INTEGER NOT NULL
);

CREATE TABLE media_publication_jobs (
  intent_uuid BLOB PRIMARY KEY REFERENCES sync_publication_intents(intent_uuid) ON DELETE RESTRICT CHECK(length(intent_uuid) = 16),
  phase TEXT NOT NULL CHECK(phase IN ('pending-hash','chunking','packs-sealed','uploading-data','data-durable','uploading-manifest','manifest-durable','canonicalizing','canonicalized','event-published','failed','cancelled','archived')),
  source_identity_json TEXT,
  binding_epoch BLOB CHECK(binding_epoch IS NULL OR length(binding_epoch) = 16),
  transport_generation INTEGER NOT NULL DEFAULT 0 CHECK(transport_generation >= 0),
  owner_token TEXT,
  lease_expires_at INTEGER,
  upload_object_key TEXT,
  upload_id TEXT,
  upload_offset INTEGER NOT NULL DEFAULT 0 CHECK(upload_offset >= 0),
  priority INTEGER NOT NULL DEFAULT 0 CHECK(priority BETWEEN 0 AND 3),
  attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
  next_attempt_at INTEGER,
  error_code TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE media_publication_objects (
  intent_uuid BLOB NOT NULL REFERENCES media_publication_jobs(intent_uuid) ON DELETE CASCADE CHECK(length(intent_uuid) = 16),
  ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
  object_kind TEXT NOT NULL CHECK(object_kind IN ('data-pack','manifest-pack','index-shard')),
  object_hash BLOB NOT NULL CHECK(length(object_hash) = 32),
  object_key TEXT NOT NULL,
  byte_length INTEGER NOT NULL CHECK(byte_length > 0),
  local_relpath TEXT NOT NULL CHECK(length(local_relpath) BETWEEN 1 AND 1024 AND substr(local_relpath,1,1) NOT IN ('/','\\') AND instr(local_relpath,'..') = 0),
  state TEXT NOT NULL CHECK(state IN ('sealed','uploading','durable','failed','orphan')),
  receipt_generation INTEGER,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(intent_uuid, object_kind, ordinal)
);

CREATE TABLE media_materialization_jobs (
  content_revision_uuid BLOB PRIMARY KEY REFERENCES asset_content_revisions(content_revision_uuid) ON DELETE RESTRICT CHECK(length(content_revision_uuid) = 16),
  asset_uuid BLOB NOT NULL CHECK(length(asset_uuid) = 16),
  destination_relpath TEXT NOT NULL CHECK(length(destination_relpath) BETWEEN 1 AND 1024 AND substr(destination_relpath,1,1) NOT IN ('/','\\') AND instr(destination_relpath,'..') = 0),
  phase TEXT NOT NULL CHECK(phase IN ('pending','fetching-manifest','downloading','assembling','publishing','complete','remote-content-missing','corrupt','path-conflict','disk-full','locked','failed','cancelled')),
  expected_length INTEGER NOT NULL CHECK(expected_length >= 0),
  downloaded_bytes INTEGER NOT NULL DEFAULT 0 CHECK(downloaded_bytes >= 0 AND downloaded_bytes <= expected_length),
  staging_relpath TEXT CHECK(staging_relpath IS NULL OR (length(staging_relpath) BETWEEN 1 AND 1024 AND substr(staging_relpath,1,1) NOT IN ('/','\\') AND instr(staging_relpath,'..') = 0)),
  priority INTEGER NOT NULL DEFAULT 0 CHECK(priority BETWEEN 0 AND 3),
  binding_epoch BLOB CHECK(binding_epoch IS NULL OR length(binding_epoch) = 16),
  transport_generation INTEGER NOT NULL DEFAULT 0 CHECK(transport_generation >= 0),
  owner_token TEXT,
  lease_expires_at INTEGER,
  attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
  next_attempt_at INTEGER,
  error_code TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE media_verified_ranges (
  pack_hash BLOB NOT NULL CHECK(length(pack_hash) = 32),
  range_start INTEGER NOT NULL CHECK(range_start >= 0),
  range_end INTEGER NOT NULL CHECK(range_end > range_start),
  range_hash BLOB NOT NULL CHECK(length(range_hash) = 32),
  local_relpath TEXT NOT NULL CHECK(length(local_relpath) BETWEEN 1 AND 1024 AND substr(local_relpath,1,1) NOT IN ('/','\\') AND instr(local_relpath,'..') = 0),
  verified_at INTEGER NOT NULL,
  PRIMARY KEY(pack_hash, range_start, range_end)
);

CREATE TABLE media_remote_objects (
  object_hash BLOB PRIMARY KEY CHECK(length(object_hash) = 32),
  object_kind TEXT NOT NULL CHECK(object_kind IN ('data-pack','manifest-pack','index-shard')),
  object_key TEXT NOT NULL UNIQUE,
  byte_length INTEGER NOT NULL CHECK(byte_length > 0),
  binding_epoch BLOB CHECK(binding_epoch IS NULL OR length(binding_epoch) = 16),
  receipt_generation INTEGER NOT NULL CHECK(receipt_generation >= 0),
  state TEXT NOT NULL CHECK(state IN ('durable','remote','missing','corrupt')),
  updated_at INTEGER NOT NULL
);

CREATE TABLE media_reachability (
  object_hash BLOB NOT NULL CHECK(length(object_hash) = 32),
  object_kind TEXT NOT NULL CHECK(object_kind IN ('data-pack','manifest-pack','index-shard')),
  content_revision_uuid BLOB NOT NULL REFERENCES asset_content_revisions(content_revision_uuid) ON DELETE RESTRICT CHECK(length(content_revision_uuid) = 16),
  state TEXT NOT NULL CHECK(state IN ('reachable','orphan','unknown','missing','corrupt')),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(object_hash, object_kind, content_revision_uuid)
);

CREATE INDEX idx_media_chunks_pack ON media_chunks(pack_hash, pack_offset) WHERE pack_hash IS NOT NULL;
CREATE INDEX idx_media_manifests_content ON media_manifests(content_hash, byte_length, manifest_hash);
CREATE INDEX idx_media_manifests_pack ON media_manifests(manifest_pack_hash, pack_offset) WHERE manifest_pack_hash IS NOT NULL;
CREATE INDEX idx_media_content_manifest_lookup ON media_content_manifest_links(manifest_hash, content_revision_uuid);
CREATE INDEX idx_media_publication_work ON media_publication_jobs(priority DESC, next_attempt_at, updated_at, intent_uuid) WHERE phase NOT IN ('canonicalized','event-published','cancelled','archived');
CREATE INDEX idx_media_publication_lease ON media_publication_jobs(lease_expires_at, intent_uuid) WHERE owner_token IS NOT NULL;
CREATE INDEX idx_media_publication_objects_state ON media_publication_objects(intent_uuid, state, ordinal);
CREATE INDEX idx_media_materialization_work ON media_materialization_jobs(priority DESC, next_attempt_at, updated_at, content_revision_uuid) WHERE phase NOT IN ('complete','cancelled');
CREATE INDEX idx_media_materialization_lease ON media_materialization_jobs(lease_expires_at, content_revision_uuid) WHERE owner_token IS NOT NULL;
CREATE INDEX idx_media_remote_objects_kind ON media_remote_objects(object_kind, state, object_hash);
CREATE INDEX idx_media_reachability_revision ON media_reachability(content_revision_uuid, state, object_kind);

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (29, 'media_object_storage', unixepoch());

COMMIT;
