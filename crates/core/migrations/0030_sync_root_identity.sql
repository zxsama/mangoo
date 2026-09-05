-- Portable synchronized-root descriptors and Catalog linkage for managed-root v2.
-- Device-local paths and operation journals deliberately remain in host.sqlite.
BEGIN IMMEDIATE;

CREATE TABLE sync_root_descriptors (
  root_uuid BLOB PRIMARY KEY CHECK(length(root_uuid) = 16),
  root_folder_uuid BLOB NOT NULL UNIQUE CHECK(length(root_folder_uuid) = 16),
  library_root_uuid BLOB NOT NULL CHECK(length(library_root_uuid) = 16),
  creation_event_uuid BLOB CHECK(creation_event_uuid IS NULL OR length(creation_event_uuid) = 16),
  winning_location_revision_uuid BLOB CHECK(winning_location_revision_uuid IS NULL OR length(winning_location_revision_uuid) = 16),
  requested_name TEXT NOT NULL CHECK(length(requested_name) > 0),
  materialized_name TEXT NOT NULL CHECK(length(materialized_name) > 0),
  materialization_fingerprint BLOB NOT NULL CHECK(length(materialization_fingerprint) = 32),
  lifecycle TEXT NOT NULL CHECK(lifecycle IN ('active','tombstoned')),
  logical_revision INTEGER NOT NULL DEFAULT 0 CHECK(logical_revision >= 0),
  updated_at INTEGER NOT NULL
);

CREATE INDEX idx_sync_root_descriptors_library_lifecycle
  ON sync_root_descriptors(library_root_uuid, lifecycle, root_uuid);
CREATE INDEX idx_sync_root_descriptors_updated
  ON sync_root_descriptors(updated_at, root_uuid);

CREATE TRIGGER sync_root_descriptors_keep_identity
BEFORE UPDATE OF root_uuid, root_folder_uuid, library_root_uuid ON sync_root_descriptors
WHEN NEW.root_uuid != OLD.root_uuid
  OR NEW.root_folder_uuid != OLD.root_folder_uuid
  OR NEW.library_root_uuid != OLD.library_root_uuid
BEGIN
  SELECT RAISE(ABORT, 'sync root identity is immutable');
END;

CREATE TRIGGER sync_root_descriptors_reject_revision_regression
BEFORE UPDATE ON sync_root_descriptors
WHEN NEW.logical_revision < OLD.logical_revision
  OR (
    NEW.logical_revision = OLD.logical_revision
    AND (
      NOT (NEW.creation_event_uuid IS OLD.creation_event_uuid)
      OR NOT (NEW.winning_location_revision_uuid IS OLD.winning_location_revision_uuid)
      OR NEW.requested_name != OLD.requested_name
      OR NEW.materialized_name != OLD.materialized_name
      OR NEW.materialization_fingerprint != OLD.materialization_fingerprint
      OR NEW.lifecycle != OLD.lifecycle
    )
  )
BEGIN
  SELECT RAISE(ABORT, 'sync root logical revision is stale or conflicting');
END;

ALTER TABLE roots ADD COLUMN root_folder_uuid BLOB
  CHECK(root_folder_uuid IS NULL OR length(root_folder_uuid) = 16);

CREATE UNIQUE INDEX ux_roots_root_folder_uuid
  ON roots(root_folder_uuid)
  WHERE root_folder_uuid IS NOT NULL;

CREATE UNIQUE INDEX ux_folders_root_empty_sentinel
  ON folders(root_id)
  WHERE parent_id IS NULL AND name = '';

CREATE TRIGGER roots_validate_sync_identity_insert
BEFORE INSERT ON roots
WHEN NEW.root_folder_uuid IS NOT NULL
BEGIN
  SELECT RAISE(ABORT, 'sync root must be linked after sentinel creation');
END;

CREATE TRIGGER roots_validate_sync_identity_update
BEFORE UPDATE OF root_uuid, root_folder_uuid ON roots
WHEN NEW.root_folder_uuid IS NOT NULL
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
    FROM sync_root_descriptors descriptor
    WHERE descriptor.root_uuid = NEW.root_uuid
      AND descriptor.root_folder_uuid = NEW.root_folder_uuid
  ) THEN RAISE(ABORT, 'root sync descriptor mismatch') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1
    FROM folders sentinel
    WHERE sentinel.root_id = NEW.id
      AND sentinel.parent_id IS NULL
      AND sentinel.name = ''
      AND sentinel.folder_uuid = NEW.root_folder_uuid
  ) THEN RAISE(ABORT, 'root sync sentinel mismatch') END;
END;

CREATE TRIGGER roots_keep_sync_identity
BEFORE UPDATE OF root_uuid, root_folder_uuid ON roots
WHEN OLD.root_folder_uuid IS NOT NULL
  AND (NEW.root_uuid != OLD.root_uuid OR NEW.root_folder_uuid != OLD.root_folder_uuid)
BEGIN
  SELECT RAISE(ABORT, 'selected sync root identity is immutable');
END;

CREATE TRIGGER folders_keep_sync_root_sentinel_identity
BEFORE UPDATE OF folder_uuid, parent_id, name, root_id ON folders
WHEN EXISTS (
  SELECT 1 FROM roots
  WHERE roots.id = OLD.root_id
    AND roots.root_folder_uuid = OLD.folder_uuid
)
AND (
  NEW.folder_uuid != OLD.folder_uuid
  OR NEW.root_id != OLD.root_id
  OR NEW.parent_id IS NOT OLD.parent_id
  OR NEW.name != OLD.name
)
BEGIN
  SELECT RAISE(ABORT, 'sync root sentinel identity is immutable');
END;

CREATE TRIGGER folders_validate_sync_root_sentinel_insert
BEFORE INSERT ON folders
WHEN NEW.parent_id IS NULL
  AND NEW.name = ''
  AND EXISTS (
    SELECT 1 FROM roots
    WHERE roots.id = NEW.root_id
      AND roots.root_folder_uuid IS NOT NULL
      AND roots.root_folder_uuid != NEW.folder_uuid
  )
BEGIN
  SELECT RAISE(ABORT, 'root sync sentinel mismatch');
END;

INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
VALUES (30, 'sync_root_identity', unixepoch());

COMMIT;
