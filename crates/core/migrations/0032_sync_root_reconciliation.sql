-- Root-scoped capture identities and idempotent reconciliation replay.
BEGIN IMMEDIATE;

ALTER TABLE sync_root_local_bindings ADD COLUMN root_capture_epoch BLOB
  CHECK(root_capture_epoch IS NULL OR length(root_capture_epoch) = 16);
ALTER TABLE sync_root_local_bindings ADD COLUMN root_actor_epoch BLOB
  CHECK(root_actor_epoch IS NULL OR length(root_actor_epoch) = 16);

UPDATE sync_root_local_bindings
SET root_capture_epoch = COALESCE(root_capture_epoch, local_binding_uuid),
    root_actor_epoch = COALESCE(root_actor_epoch, local_binding_uuid);

CREATE TABLE sync_root_reconciliation_applied (
  plan_uuid BLOB NOT NULL CHECK(length(plan_uuid) = 16),
  item_ordinal INTEGER NOT NULL CHECK(item_ordinal >= 0),
  root_uuid BLOB NOT NULL REFERENCES sync_root_descriptors(root_uuid) ON DELETE RESTRICT CHECK(length(root_uuid) = 16),
  local_binding_uuid BLOB NOT NULL CHECK(length(local_binding_uuid) = 16),
  binding_generation INTEGER NOT NULL CHECK(binding_generation >= 1),
  applied_at INTEGER NOT NULL,
  PRIMARY KEY(plan_uuid, item_ordinal)
);

CREATE TABLE sync_root_publish_cutovers (
  operation_uuid BLOB PRIMARY KEY CHECK(length(operation_uuid) = 16),
  source_root_uuid BLOB NOT NULL CHECK(length(source_root_uuid) = 16),
  new_root_uuid BLOB NOT NULL UNIQUE CHECK(length(new_root_uuid) = 16),
  source_root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE RESTRICT,
  new_root_id INTEGER REFERENCES roots(id) ON DELETE RESTRICT,
  state TEXT NOT NULL CHECK(state IN ('planned','complete')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX idx_sync_root_reconciliation_binding
  ON sync_root_reconciliation_applied(root_uuid, local_binding_uuid, binding_generation);

DROP TRIGGER roots_keep_sync_identity;
CREATE TRIGGER roots_keep_sync_identity
BEFORE UPDATE OF root_uuid, root_folder_uuid ON roots
WHEN OLD.root_folder_uuid IS NOT NULL
  AND (NEW.root_uuid != OLD.root_uuid OR NEW.root_folder_uuid != OLD.root_folder_uuid)
  AND NOT EXISTS (
    SELECT 1 FROM sync_root_publish_cutovers cutover
    WHERE cutover.source_root_id = OLD.id
      AND cutover.source_root_uuid = OLD.root_uuid
      AND cutover.new_root_uuid = NEW.root_uuid
      AND cutover.state = 'planned'
  )
BEGIN
  SELECT RAISE(ABORT, 'selected sync root identity is immutable');
END;

DROP TRIGGER folders_keep_sync_root_sentinel_identity;
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
AND NOT EXISTS (
  SELECT 1 FROM sync_root_publish_cutovers cutover
  WHERE cutover.source_root_id = OLD.root_id
    AND cutover.source_root_uuid = (SELECT root_uuid FROM roots WHERE id = OLD.root_id)
    AND cutover.state = 'planned'
)
BEGIN
  SELECT RAISE(ABORT, 'sync root sentinel identity is immutable');
END;

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (32, 'sync_root_reconciliation', unixepoch());

COMMIT;
