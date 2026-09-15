-- Accept consecutive dots inside names while rejecting parent-directory components.
-- Copy before restoring ownership triggers: legacy all-NULL jobs must stay unchanged
-- until ordinary admission proves their current binding. No table references this queue.
BEGIN IMMEDIATE;

CREATE TABLE media_materialization_jobs_next (
  content_revision_uuid BLOB PRIMARY KEY REFERENCES asset_content_revisions(content_revision_uuid) ON DELETE RESTRICT CHECK(length(content_revision_uuid) = 16),
  asset_uuid BLOB NOT NULL CHECK(length(asset_uuid) = 16),
  destination_relpath TEXT NOT NULL CHECK(
    length(destination_relpath) BETWEEN 1 AND 1024
    AND substr(destination_relpath,1,1) NOT IN ('/',char(92))
    AND destination_relpath NOT GLOB '[A-Za-z]:*'
    AND instr('/' || replace(destination_relpath,char(92),'/') || '/', '/../') = 0
  ),
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
  updated_at INTEGER NOT NULL,
  root_uuid BLOB CHECK(root_uuid IS NULL OR length(root_uuid) = 16),
  local_binding_uuid BLOB CHECK(local_binding_uuid IS NULL OR length(local_binding_uuid) = 16),
  binding_generation INTEGER CHECK(binding_generation IS NULL OR binding_generation >= 1)
);

INSERT INTO media_materialization_jobs_next(content_revision_uuid,asset_uuid,destination_relpath,phase,expected_length,downloaded_bytes,staging_relpath,priority,binding_epoch,transport_generation,owner_token,lease_expires_at,attempt_count,next_attempt_at,error_code,created_at,updated_at,root_uuid,local_binding_uuid,binding_generation)
SELECT content_revision_uuid,asset_uuid,destination_relpath,phase,expected_length,downloaded_bytes,staging_relpath,priority,binding_epoch,transport_generation,owner_token,lease_expires_at,attempt_count,next_attempt_at,error_code,created_at,updated_at,root_uuid,local_binding_uuid,binding_generation FROM media_materialization_jobs;
DROP TABLE media_materialization_jobs;
ALTER TABLE media_materialization_jobs_next RENAME TO media_materialization_jobs;

CREATE INDEX idx_media_materialization_work ON media_materialization_jobs(priority DESC, next_attempt_at, updated_at, content_revision_uuid) WHERE phase NOT IN ('complete','cancelled');
CREATE INDEX idx_media_materialization_lease ON media_materialization_jobs(lease_expires_at, content_revision_uuid) WHERE owner_token IS NOT NULL;
CREATE INDEX idx_media_materialization_root_binding_work
  ON media_materialization_jobs(root_uuid, local_binding_uuid, binding_generation, phase, lease_expires_at, content_revision_uuid)
  WHERE root_uuid IS NOT NULL;

CREATE TRIGGER media_materialization_jobs_validate_root_ownership_insert
BEFORE INSERT ON media_materialization_jobs
BEGIN
  SELECT CASE WHEN NOT (
    (NEW.root_uuid IS NULL AND NEW.local_binding_uuid IS NULL AND NEW.binding_generation IS NULL)
    OR
    (NEW.root_uuid IS NOT NULL AND NEW.local_binding_uuid IS NOT NULL AND NEW.binding_generation IS NOT NULL)
  ) THEN RAISE(ABORT, 'materialization root ownership is partial') END;
  SELECT CASE WHEN NEW.root_uuid IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM files
    JOIN roots ON roots.id = files.root_id
    JOIN sync_root_local_bindings binding ON binding.root_id = roots.id
    WHERE files.asset_uuid = NEW.asset_uuid
      AND roots.root_uuid = NEW.root_uuid
      AND binding.local_binding_uuid = NEW.local_binding_uuid
      AND binding.binding_generation = NEW.binding_generation
  ) THEN RAISE(ABORT, 'managed materialization requires current root ownership') END;
END;

CREATE TRIGGER media_materialization_jobs_assign_root_ownership
AFTER INSERT ON media_materialization_jobs
WHEN NEW.root_uuid IS NULL AND EXISTS (
  SELECT 1 FROM files JOIN roots ON roots.id = files.root_id
  WHERE files.asset_uuid = NEW.asset_uuid AND roots.root_folder_uuid IS NOT NULL
)
BEGIN
  UPDATE media_materialization_jobs
  SET (root_uuid, local_binding_uuid, binding_generation) = (
    SELECT roots.root_uuid, binding.local_binding_uuid, binding.binding_generation
    FROM files
    JOIN roots ON roots.id = files.root_id
    JOIN sync_root_local_bindings binding ON binding.root_id = roots.id
    WHERE files.asset_uuid = NEW.asset_uuid
  )
  WHERE content_revision_uuid = NEW.content_revision_uuid;
  SELECT CASE WHEN (SELECT root_uuid FROM media_materialization_jobs WHERE content_revision_uuid = NEW.content_revision_uuid) IS NULL
    THEN RAISE(ABORT, 'managed materialization has no current root binding') END;
END;

CREATE TRIGGER media_materialization_jobs_validate_root_ownership_update
BEFORE UPDATE OF root_uuid, local_binding_uuid, binding_generation ON media_materialization_jobs
BEGIN
  SELECT CASE WHEN NOT (
    (NEW.root_uuid IS NULL AND NEW.local_binding_uuid IS NULL AND NEW.binding_generation IS NULL)
    OR
    (NEW.root_uuid IS NOT NULL AND NEW.local_binding_uuid IS NOT NULL AND NEW.binding_generation IS NOT NULL)
  ) THEN RAISE(ABORT, 'materialization root ownership is partial') END;
  SELECT CASE WHEN NEW.root_uuid IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM files
    JOIN roots ON roots.id = files.root_id
    JOIN sync_root_local_bindings binding ON binding.root_id = roots.id
    WHERE files.asset_uuid = NEW.asset_uuid
      AND roots.root_folder_uuid IS NOT NULL
      AND roots.root_uuid = NEW.root_uuid
      AND binding.local_binding_uuid = NEW.local_binding_uuid
      AND binding.binding_generation = NEW.binding_generation
  ) THEN RAISE(ABORT, 'managed materialization requires current root ownership') END;
END;

CREATE TRIGGER media_materialization_jobs_keep_root_ownership
BEFORE UPDATE OF root_uuid, local_binding_uuid, binding_generation ON media_materialization_jobs
WHEN OLD.root_uuid IS NOT NULL AND (
  NEW.root_uuid IS NOT OLD.root_uuid
  OR NEW.local_binding_uuid IS NOT OLD.local_binding_uuid
  OR NEW.binding_generation IS NOT OLD.binding_generation
)
BEGIN SELECT RAISE(ABORT, 'materialization root ownership is immutable'); END;

CREATE TRIGGER media_materialization_jobs_reject_unowned_managed_claim
BEFORE UPDATE OF owner_token, phase ON media_materialization_jobs
WHEN NEW.root_uuid IS NULL
  AND (NEW.owner_token IS NOT NULL OR NEW.phase IN ('publishing','complete'))
  AND EXISTS (
    SELECT 1 FROM files JOIN roots ON roots.id = files.root_id
    WHERE files.asset_uuid = NEW.asset_uuid AND roots.root_folder_uuid IS NOT NULL
  )
BEGIN SELECT RAISE(ABORT, 'managed materialization has no root ownership'); END;

CREATE TRIGGER media_materialization_jobs_fence_claim_and_success
BEFORE UPDATE OF owner_token, phase ON media_materialization_jobs
WHEN NEW.root_uuid IS NOT NULL
  AND (NEW.owner_token IS NOT NULL OR NEW.phase IN ('publishing','complete'))
  AND NOT EXISTS (
    SELECT 1 FROM sync_root_local_bindings binding
    WHERE binding.root_uuid = NEW.root_uuid
      AND binding.local_binding_uuid = NEW.local_binding_uuid
      AND binding.binding_generation = NEW.binding_generation
      AND binding.state IN ('materializing','active','reconcile-required')
  )
BEGIN SELECT RAISE(ABORT, 'stale root binding generation'); END;

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (33, 'media_materialization_destination', unixepoch());

COMMIT;
