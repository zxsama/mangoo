-- Catalog-local execution authority for managed-root path work.
--
-- Host remains the orchestration authority. This projection exists so a
-- worker can compare the root/binding/generation token in the same Catalog
-- transaction that claims or completes durable work.
BEGIN IMMEDIATE;

CREATE TABLE sync_root_local_bindings (
  root_uuid BLOB PRIMARY KEY REFERENCES sync_root_descriptors(root_uuid) ON DELETE RESTRICT
    CHECK(length(root_uuid) = 16),
  library_uuid BLOB NOT NULL CHECK(length(library_uuid) = 16),
  local_binding_uuid BLOB NOT NULL CHECK(length(local_binding_uuid) = 16),
  binding_generation INTEGER NOT NULL CHECK(binding_generation >= 1),
  state TEXT NOT NULL CHECK(state IN (
    'materializing','active','paused','local-only-fork','reconcile-required',
    'relocating','local-rename-required','unavailable','error'
  )),
  root_id INTEGER NOT NULL UNIQUE REFERENCES roots(id) ON DELETE RESTRICT,
  operation_revision INTEGER NOT NULL DEFAULT 0 CHECK(operation_revision >= 0),
  updated_at INTEGER NOT NULL,
  UNIQUE(local_binding_uuid),
  UNIQUE(root_uuid, local_binding_uuid, binding_generation)
);

CREATE INDEX idx_sync_root_local_bindings_library_state
  ON sync_root_local_bindings(library_uuid, state, root_uuid);
CREATE INDEX idx_sync_root_local_bindings_generation
  ON sync_root_local_bindings(root_uuid, local_binding_uuid, binding_generation, state);

CREATE TRIGGER sync_root_local_bindings_validate_insert
BEFORE INSERT ON sync_root_local_bindings
BEGIN
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM roots
    WHERE roots.id = NEW.root_id
      AND roots.root_uuid = NEW.root_uuid
      AND roots.root_folder_uuid IS NOT NULL
  ) THEN RAISE(ABORT, 'local binding root identity mismatch') END;
  SELECT CASE WHEN EXISTS (SELECT 1 FROM sync_local_state WHERE singleton = 1)
    AND NOT EXISTS (
      SELECT 1 FROM sync_local_state
      WHERE singleton = 1 AND library_uuid = NEW.library_uuid
    ) THEN RAISE(ABORT, 'local binding library identity mismatch') END;
END;

CREATE TRIGGER sync_root_local_bindings_validate_update
BEFORE UPDATE ON sync_root_local_bindings
BEGIN
  SELECT CASE WHEN NEW.root_uuid != OLD.root_uuid
    OR NEW.library_uuid != OLD.library_uuid
    OR NEW.root_id != OLD.root_id
    THEN RAISE(ABORT, 'local binding root identity is immutable') END;
  SELECT CASE WHEN NEW.binding_generation < OLD.binding_generation
    OR NEW.operation_revision < OLD.operation_revision
    THEN RAISE(ABORT, 'local binding projection is stale') END;
  SELECT CASE WHEN NEW.binding_generation = OLD.binding_generation
    AND NEW.local_binding_uuid != OLD.local_binding_uuid
    THEN RAISE(ABORT, 'local binding identity requires generation advance') END;
  SELECT CASE WHEN NEW.binding_generation = OLD.binding_generation
    AND NEW.operation_revision = OLD.operation_revision
    AND (
      NEW.local_binding_uuid != OLD.local_binding_uuid
      OR NEW.state != OLD.state
    ) THEN RAISE(ABORT, 'local binding projection revision conflicts') END;
  SELECT CASE WHEN OLD.state = 'local-only-fork'
    AND NEW.state != 'local-only-fork'
    AND NEW.binding_generation <= OLD.binding_generation
    THEN RAISE(ABORT, 'sealed local fork requires generation advance') END;
END;

ALTER TABLE tasks ADD COLUMN root_uuid BLOB
  CHECK(root_uuid IS NULL OR length(root_uuid) = 16);
ALTER TABLE tasks ADD COLUMN local_binding_uuid BLOB
  CHECK(local_binding_uuid IS NULL OR length(local_binding_uuid) = 16);
ALTER TABLE tasks ADD COLUMN binding_generation INTEGER
  CHECK(binding_generation IS NULL OR binding_generation >= 1);

ALTER TABLE file_operations ADD COLUMN root_uuid BLOB
  CHECK(root_uuid IS NULL OR length(root_uuid) = 16);
ALTER TABLE file_operations ADD COLUMN local_binding_uuid BLOB
  CHECK(local_binding_uuid IS NULL OR length(local_binding_uuid) = 16);
ALTER TABLE file_operations ADD COLUMN binding_generation INTEGER
  CHECK(binding_generation IS NULL OR binding_generation >= 1);

ALTER TABLE sync_publication_intents ADD COLUMN root_uuid BLOB
  CHECK(root_uuid IS NULL OR length(root_uuid) = 16);
ALTER TABLE sync_publication_intents ADD COLUMN local_binding_uuid BLOB
  CHECK(local_binding_uuid IS NULL OR length(local_binding_uuid) = 16);
ALTER TABLE sync_publication_intents ADD COLUMN binding_generation INTEGER
  CHECK(binding_generation IS NULL OR binding_generation >= 1);

ALTER TABLE media_publication_jobs ADD COLUMN root_uuid BLOB
  CHECK(root_uuid IS NULL OR length(root_uuid) = 16);
ALTER TABLE media_publication_jobs ADD COLUMN local_binding_uuid BLOB
  CHECK(local_binding_uuid IS NULL OR length(local_binding_uuid) = 16);
ALTER TABLE media_publication_jobs ADD COLUMN binding_generation INTEGER
  CHECK(binding_generation IS NULL OR binding_generation >= 1);

ALTER TABLE media_materialization_jobs ADD COLUMN root_uuid BLOB
  CHECK(root_uuid IS NULL OR length(root_uuid) = 16);
ALTER TABLE media_materialization_jobs ADD COLUMN local_binding_uuid BLOB
  CHECK(local_binding_uuid IS NULL OR length(local_binding_uuid) = 16);
ALTER TABLE media_materialization_jobs ADD COLUMN binding_generation INTEGER
  CHECK(binding_generation IS NULL OR binding_generation >= 1);

CREATE INDEX idx_tasks_root_binding_work
  ON tasks(root_uuid, local_binding_uuid, binding_generation, status, kind, id)
  WHERE root_uuid IS NOT NULL;
CREATE INDEX idx_file_operations_root_binding_work
  ON file_operations(root_uuid, local_binding_uuid, binding_generation, status, journal_phase, id)
  WHERE root_uuid IS NOT NULL;
CREATE INDEX idx_sync_publication_root_binding_work
  ON sync_publication_intents(root_uuid, local_binding_uuid, binding_generation, state, updated_at, intent_uuid)
  WHERE root_uuid IS NOT NULL;
CREATE INDEX idx_media_publication_root_binding_work
  ON media_publication_jobs(root_uuid, local_binding_uuid, binding_generation, phase, lease_expires_at, intent_uuid)
  WHERE root_uuid IS NOT NULL;
CREATE INDEX idx_media_materialization_root_binding_work
  ON media_materialization_jobs(root_uuid, local_binding_uuid, binding_generation, phase, lease_expires_at, content_revision_uuid)
  WHERE root_uuid IS NOT NULL;

-- Every ownership token is all-null or all-present and cannot be silently
-- replaced once work has been assigned.
CREATE TRIGGER tasks_validate_root_ownership_insert
BEFORE INSERT ON tasks
BEGIN
  SELECT CASE WHEN NOT (
    (NEW.root_uuid IS NULL AND NEW.local_binding_uuid IS NULL AND NEW.binding_generation IS NULL)
    OR
    (NEW.root_uuid IS NOT NULL AND NEW.local_binding_uuid IS NOT NULL AND NEW.binding_generation IS NOT NULL)
  ) THEN RAISE(ABORT, 'task root ownership is partial') END;
  SELECT CASE WHEN NEW.root_uuid IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM roots
    JOIN sync_root_local_bindings binding ON binding.root_id = roots.id
    WHERE roots.id = COALESCE(
      NEW.root_id,
      (SELECT root_id FROM folders WHERE id = NEW.folder_id),
      (SELECT root_id FROM files WHERE id = NEW.file_id)
    )
      AND roots.root_uuid = NEW.root_uuid
      AND binding.local_binding_uuid = NEW.local_binding_uuid
      AND binding.binding_generation = NEW.binding_generation
  ) THEN RAISE(ABORT, 'managed task requires current root ownership') END;
END;

CREATE TRIGGER tasks_assign_root_ownership
AFTER INSERT ON tasks
WHEN NEW.root_uuid IS NULL AND EXISTS (
  SELECT 1 FROM roots
  WHERE roots.id = COALESCE(
    NEW.root_id,
    (SELECT root_id FROM folders WHERE id = NEW.folder_id),
    (SELECT root_id FROM files WHERE id = NEW.file_id)
  ) AND roots.root_folder_uuid IS NOT NULL
)
BEGIN
  UPDATE tasks
  SET (root_uuid, local_binding_uuid, binding_generation) = (
    SELECT roots.root_uuid, binding.local_binding_uuid, binding.binding_generation
    FROM roots JOIN sync_root_local_bindings binding ON binding.root_id = roots.id
    WHERE roots.id = COALESCE(
      NEW.root_id,
      (SELECT root_id FROM folders WHERE id = NEW.folder_id),
      (SELECT root_id FROM files WHERE id = NEW.file_id)
    )
  )
  WHERE id = NEW.id;
  SELECT CASE WHEN (SELECT root_uuid FROM tasks WHERE id = NEW.id) IS NULL
    THEN RAISE(ABORT, 'managed task has no current root binding') END;
END;

CREATE TRIGGER tasks_validate_root_ownership_update
BEFORE UPDATE OF root_uuid, local_binding_uuid, binding_generation ON tasks
BEGIN
  SELECT CASE WHEN NOT (
    (NEW.root_uuid IS NULL AND NEW.local_binding_uuid IS NULL AND NEW.binding_generation IS NULL)
    OR
    (NEW.root_uuid IS NOT NULL AND NEW.local_binding_uuid IS NOT NULL AND NEW.binding_generation IS NOT NULL)
  ) THEN RAISE(ABORT, 'task root ownership is partial') END;
  SELECT CASE WHEN NEW.root_uuid IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM roots
    JOIN sync_root_local_bindings binding ON binding.root_id = roots.id
    WHERE roots.id = COALESCE(
      NEW.root_id,
      (SELECT root_id FROM folders WHERE id = NEW.folder_id),
      (SELECT root_id FROM files WHERE id = NEW.file_id)
    )
      AND roots.root_folder_uuid IS NOT NULL
      AND roots.root_uuid = NEW.root_uuid
      AND binding.local_binding_uuid = NEW.local_binding_uuid
      AND binding.binding_generation = NEW.binding_generation
  ) THEN RAISE(ABORT, 'managed task requires current root ownership') END;
END;

CREATE TRIGGER tasks_keep_root_ownership
BEFORE UPDATE OF root_uuid, local_binding_uuid, binding_generation ON tasks
WHEN OLD.root_uuid IS NOT NULL AND (
  NEW.root_uuid IS NOT OLD.root_uuid
  OR NEW.local_binding_uuid IS NOT OLD.local_binding_uuid
  OR NEW.binding_generation IS NOT OLD.binding_generation
)
BEGIN SELECT RAISE(ABORT, 'task root ownership is immutable'); END;

CREATE TRIGGER tasks_reject_unowned_managed_claim
BEFORE UPDATE OF status ON tasks
WHEN NEW.root_uuid IS NULL
  AND NEW.status IN ('running','succeeded')
  AND EXISTS (
    SELECT 1 FROM roots
    WHERE roots.id = COALESCE(
      NEW.root_id,
      (SELECT root_id FROM folders WHERE id = NEW.folder_id),
      (SELECT root_id FROM files WHERE id = NEW.file_id)
    ) AND roots.root_folder_uuid IS NOT NULL
  )
BEGIN SELECT RAISE(ABORT, 'managed task has no root ownership'); END;

CREATE TRIGGER tasks_fence_claim_and_success
BEFORE UPDATE OF status ON tasks
WHEN NEW.root_uuid IS NOT NULL
  AND NEW.status IN ('running','succeeded')
  AND NOT EXISTS (
    SELECT 1 FROM sync_root_local_bindings binding
    WHERE binding.root_uuid = NEW.root_uuid
      AND binding.local_binding_uuid = NEW.local_binding_uuid
      AND binding.binding_generation = NEW.binding_generation
      AND binding.state IN (
        'materializing','active','local-only-fork','reconcile-required',
        'relocating','local-rename-required'
      )
  )
BEGIN SELECT RAISE(ABORT, 'stale root binding generation'); END;

CREATE TRIGGER file_operations_validate_root_ownership_insert
BEFORE INSERT ON file_operations
BEGIN
  SELECT CASE WHEN NOT (
    (NEW.root_uuid IS NULL AND NEW.local_binding_uuid IS NULL AND NEW.binding_generation IS NULL)
    OR
    (NEW.root_uuid IS NOT NULL AND NEW.local_binding_uuid IS NOT NULL AND NEW.binding_generation IS NOT NULL)
  ) THEN RAISE(ABORT, 'file operation root ownership is partial') END;
  SELECT CASE WHEN NEW.root_uuid IS NOT NULL AND NOT EXISTS (
      SELECT 1 FROM roots
      JOIN sync_root_local_bindings binding ON binding.root_id = roots.id
      WHERE roots.id = NEW.root_id
        AND roots.root_uuid = NEW.root_uuid
        AND binding.local_binding_uuid = NEW.local_binding_uuid
        AND binding.binding_generation = NEW.binding_generation
  ) THEN RAISE(ABORT, 'managed file operation requires current root ownership') END;
END;

CREATE TRIGGER file_operations_assign_root_ownership
AFTER INSERT ON file_operations
WHEN NEW.root_uuid IS NULL AND NEW.root_id IS NOT NULL
  AND EXISTS (SELECT 1 FROM roots WHERE id = NEW.root_id AND root_folder_uuid IS NOT NULL)
BEGIN
  UPDATE file_operations
  SET (root_uuid, local_binding_uuid, binding_generation) = (
    SELECT roots.root_uuid, binding.local_binding_uuid, binding.binding_generation
    FROM roots JOIN sync_root_local_bindings binding ON binding.root_id = roots.id
    WHERE roots.id = NEW.root_id
  )
  WHERE id = NEW.id;
  SELECT CASE WHEN (SELECT root_uuid FROM file_operations WHERE id = NEW.id) IS NULL
    THEN RAISE(ABORT, 'managed file operation has no current root binding') END;
END;

CREATE TRIGGER file_operations_validate_root_ownership_update
BEFORE UPDATE OF root_uuid, local_binding_uuid, binding_generation ON file_operations
BEGIN
  SELECT CASE WHEN NOT (
    (NEW.root_uuid IS NULL AND NEW.local_binding_uuid IS NULL AND NEW.binding_generation IS NULL)
    OR
    (NEW.root_uuid IS NOT NULL AND NEW.local_binding_uuid IS NOT NULL AND NEW.binding_generation IS NOT NULL)
  ) THEN RAISE(ABORT, 'file operation root ownership is partial') END;
  SELECT CASE WHEN NEW.root_uuid IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM roots
    JOIN sync_root_local_bindings binding ON binding.root_id = roots.id
    WHERE roots.id = NEW.root_id
      AND roots.root_folder_uuid IS NOT NULL
      AND roots.root_uuid = NEW.root_uuid
      AND binding.local_binding_uuid = NEW.local_binding_uuid
      AND binding.binding_generation = NEW.binding_generation
  ) THEN RAISE(ABORT, 'managed file operation requires current root ownership') END;
END;

CREATE TRIGGER file_operations_keep_root_ownership
BEFORE UPDATE OF root_uuid, local_binding_uuid, binding_generation ON file_operations
WHEN OLD.root_uuid IS NOT NULL AND (
  NEW.root_uuid IS NOT OLD.root_uuid
  OR NEW.local_binding_uuid IS NOT OLD.local_binding_uuid
  OR NEW.binding_generation IS NOT OLD.binding_generation
)
BEGIN SELECT RAISE(ABORT, 'file operation root ownership is immutable'); END;

CREATE TRIGGER file_operations_reject_unowned_managed_effect
BEFORE UPDATE OF status, journal_phase ON file_operations
WHEN NEW.root_uuid IS NULL
  AND (NEW.journal_phase = 'executing' OR NEW.status = 'succeeded')
  AND NEW.root_id IS NOT NULL
  AND EXISTS (
    SELECT 1 FROM roots
    WHERE roots.id = NEW.root_id AND roots.root_folder_uuid IS NOT NULL
  )
BEGIN SELECT RAISE(ABORT, 'managed file operation has no root ownership'); END;

CREATE TRIGGER file_operations_fence_effect_and_success
BEFORE UPDATE OF status, journal_phase ON file_operations
WHEN NEW.root_uuid IS NOT NULL
  AND (NEW.journal_phase = 'executing' OR NEW.status = 'succeeded')
  AND NOT EXISTS (
    SELECT 1 FROM sync_root_local_bindings binding
    WHERE binding.root_uuid = NEW.root_uuid
      AND binding.local_binding_uuid = NEW.local_binding_uuid
      AND binding.binding_generation = NEW.binding_generation
      AND binding.state IN (
        'materializing','active','local-only-fork','reconcile-required',
        'relocating','local-rename-required'
      )
  )
BEGIN SELECT RAISE(ABORT, 'stale root binding generation'); END;

CREATE TRIGGER sync_publication_intents_validate_root_ownership_insert
BEFORE INSERT ON sync_publication_intents
BEGIN
  SELECT CASE WHEN NOT (
    (NEW.root_uuid IS NULL AND NEW.local_binding_uuid IS NULL AND NEW.binding_generation IS NULL)
    OR
    (NEW.root_uuid IS NOT NULL AND NEW.local_binding_uuid IS NOT NULL AND NEW.binding_generation IS NOT NULL)
  ) THEN RAISE(ABORT, 'publication intent root ownership is partial') END;
  SELECT CASE WHEN NEW.root_uuid IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM files
    JOIN roots ON roots.id = files.root_id
    JOIN sync_root_local_bindings binding ON binding.root_id = roots.id
    WHERE files.asset_uuid = NEW.asset_uuid
      AND roots.root_uuid = NEW.root_uuid
      AND binding.local_binding_uuid = NEW.local_binding_uuid
      AND binding.binding_generation = NEW.binding_generation
  ) THEN RAISE(ABORT, 'managed publication intent requires current root ownership') END;
END;

CREATE TRIGGER sync_publication_intents_assign_root_ownership
AFTER INSERT ON sync_publication_intents
WHEN NEW.root_uuid IS NULL AND EXISTS (
  SELECT 1 FROM files JOIN roots ON roots.id = files.root_id
  WHERE files.asset_uuid = NEW.asset_uuid AND roots.root_folder_uuid IS NOT NULL
)
BEGIN
  UPDATE sync_publication_intents
  SET (root_uuid, local_binding_uuid, binding_generation) = (
    SELECT roots.root_uuid, binding.local_binding_uuid, binding.binding_generation
    FROM files
    JOIN roots ON roots.id = files.root_id
    JOIN sync_root_local_bindings binding ON binding.root_id = roots.id
    WHERE files.asset_uuid = NEW.asset_uuid
  )
  WHERE intent_uuid = NEW.intent_uuid;
  SELECT CASE WHEN (SELECT root_uuid FROM sync_publication_intents WHERE intent_uuid = NEW.intent_uuid) IS NULL
    THEN RAISE(ABORT, 'managed publication intent has no current root binding') END;
END;

CREATE TRIGGER sync_publication_intents_validate_root_ownership_update
BEFORE UPDATE OF root_uuid, local_binding_uuid, binding_generation ON sync_publication_intents
BEGIN
  SELECT CASE WHEN NOT (
    (NEW.root_uuid IS NULL AND NEW.local_binding_uuid IS NULL AND NEW.binding_generation IS NULL)
    OR
    (NEW.root_uuid IS NOT NULL AND NEW.local_binding_uuid IS NOT NULL AND NEW.binding_generation IS NOT NULL)
  ) THEN RAISE(ABORT, 'publication intent root ownership is partial') END;
  SELECT CASE WHEN NEW.root_uuid IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM files
    JOIN roots ON roots.id = files.root_id
    JOIN sync_root_local_bindings binding ON binding.root_id = roots.id
    WHERE files.asset_uuid = NEW.asset_uuid
      AND roots.root_folder_uuid IS NOT NULL
      AND roots.root_uuid = NEW.root_uuid
      AND binding.local_binding_uuid = NEW.local_binding_uuid
      AND binding.binding_generation = NEW.binding_generation
  ) THEN RAISE(ABORT, 'managed publication intent requires current root ownership') END;
END;

CREATE TRIGGER sync_publication_intents_keep_root_ownership
BEFORE UPDATE OF root_uuid, local_binding_uuid, binding_generation ON sync_publication_intents
WHEN OLD.root_uuid IS NOT NULL AND (
  NEW.root_uuid IS NOT OLD.root_uuid
  OR NEW.local_binding_uuid IS NOT OLD.local_binding_uuid
  OR NEW.binding_generation IS NOT OLD.binding_generation
)
BEGIN SELECT RAISE(ABORT, 'publication intent root ownership is immutable'); END;

CREATE TRIGGER sync_publication_intents_reject_unowned_managed_progress
BEFORE UPDATE OF state ON sync_publication_intents
WHEN NEW.root_uuid IS NULL
  AND NEW.state IN ('pending-hash','pending-manifest','ready','canonicalized')
  AND EXISTS (
    SELECT 1 FROM files JOIN roots ON roots.id = files.root_id
    WHERE files.asset_uuid = NEW.asset_uuid AND roots.root_folder_uuid IS NOT NULL
  )
BEGIN SELECT RAISE(ABORT, 'managed publication intent has no root ownership'); END;

CREATE TRIGGER sync_publication_intents_fence_progress
BEFORE UPDATE OF state ON sync_publication_intents
WHEN NEW.root_uuid IS NOT NULL
  AND NEW.state IN ('pending-hash','pending-manifest','ready','canonicalized')
  AND NOT EXISTS (
    SELECT 1 FROM sync_root_local_bindings binding
    WHERE binding.root_uuid = NEW.root_uuid
      AND binding.local_binding_uuid = NEW.local_binding_uuid
      AND binding.binding_generation = NEW.binding_generation
      AND binding.state = 'active'
  )
BEGIN SELECT RAISE(ABORT, 'stale root binding generation'); END;

CREATE TRIGGER media_publication_jobs_validate_root_ownership_insert
BEFORE INSERT ON media_publication_jobs
BEGIN
  SELECT CASE WHEN NOT (
    (NEW.root_uuid IS NULL AND NEW.local_binding_uuid IS NULL AND NEW.binding_generation IS NULL)
    OR
    (NEW.root_uuid IS NOT NULL AND NEW.local_binding_uuid IS NOT NULL AND NEW.binding_generation IS NOT NULL)
  ) THEN RAISE(ABORT, 'publication job root ownership is partial') END;
  SELECT CASE WHEN NEW.root_uuid IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM sync_publication_intents parent
    WHERE parent.intent_uuid = NEW.intent_uuid
      AND parent.root_uuid IS NEW.root_uuid
      AND parent.local_binding_uuid IS NEW.local_binding_uuid
      AND parent.binding_generation IS NEW.binding_generation
  ) THEN RAISE(ABORT, 'publication job ownership must match parent intent') END;
END;

CREATE TRIGGER media_publication_jobs_assign_root_ownership
AFTER INSERT ON media_publication_jobs
WHEN NEW.root_uuid IS NULL AND EXISTS (
  SELECT 1 FROM sync_publication_intents parent
  WHERE parent.intent_uuid = NEW.intent_uuid AND parent.root_uuid IS NOT NULL
)
BEGIN
  UPDATE media_publication_jobs
  SET (root_uuid, local_binding_uuid, binding_generation) = (
    SELECT parent.root_uuid, parent.local_binding_uuid, parent.binding_generation
    FROM sync_publication_intents parent
    WHERE parent.intent_uuid = NEW.intent_uuid
  )
  WHERE intent_uuid = NEW.intent_uuid;
END;

CREATE TRIGGER media_publication_jobs_validate_root_ownership_update
BEFORE UPDATE OF root_uuid, local_binding_uuid, binding_generation ON media_publication_jobs
BEGIN
  SELECT CASE WHEN NOT (
    (NEW.root_uuid IS NULL AND NEW.local_binding_uuid IS NULL AND NEW.binding_generation IS NULL)
    OR
    (NEW.root_uuid IS NOT NULL AND NEW.local_binding_uuid IS NOT NULL AND NEW.binding_generation IS NOT NULL)
  ) THEN RAISE(ABORT, 'publication job root ownership is partial') END;
  SELECT CASE WHEN NOT EXISTS (
    SELECT 1 FROM sync_publication_intents parent
    WHERE parent.intent_uuid = NEW.intent_uuid
      AND parent.root_uuid IS NEW.root_uuid
      AND parent.local_binding_uuid IS NEW.local_binding_uuid
      AND parent.binding_generation IS NEW.binding_generation
  ) THEN RAISE(ABORT, 'publication job ownership must match parent intent') END;
  SELECT CASE WHEN OLD.root_uuid IS NOT NULL AND (
    NEW.root_uuid IS NOT OLD.root_uuid
    OR NEW.local_binding_uuid IS NOT OLD.local_binding_uuid
    OR NEW.binding_generation IS NOT OLD.binding_generation
  ) THEN RAISE(ABORT, 'publication job root ownership is immutable') END;
END;

CREATE TRIGGER sync_publication_intents_keep_child_ownership
BEFORE UPDATE OF root_uuid, local_binding_uuid, binding_generation ON sync_publication_intents
WHEN EXISTS (
  SELECT 1 FROM media_publication_jobs child
  WHERE child.intent_uuid = NEW.intent_uuid
    AND NOT (
      child.root_uuid IS NEW.root_uuid
      AND child.local_binding_uuid IS NEW.local_binding_uuid
      AND child.binding_generation IS NEW.binding_generation
    )
)
BEGIN SELECT RAISE(ABORT, 'publication intent ownership conflicts with child job'); END;

CREATE TRIGGER media_publication_jobs_fence_claim_and_success
BEFORE UPDATE OF owner_token, phase ON media_publication_jobs
WHEN NEW.root_uuid IS NOT NULL
  AND (NEW.owner_token IS NOT NULL OR NEW.phase IN ('canonicalized','event-published'))
  AND NOT EXISTS (
    SELECT 1 FROM sync_root_local_bindings binding
    WHERE binding.root_uuid = NEW.root_uuid
      AND binding.local_binding_uuid = NEW.local_binding_uuid
      AND binding.binding_generation = NEW.binding_generation
      AND binding.state = 'active'
  )
BEGIN SELECT RAISE(ABORT, 'stale root binding generation'); END;

CREATE TRIGGER media_publication_jobs_reject_unowned_managed_claim
BEFORE UPDATE OF owner_token, phase ON media_publication_jobs
WHEN NEW.root_uuid IS NULL
  AND (NEW.owner_token IS NOT NULL OR NEW.phase IN ('canonicalized','event-published'))
  AND EXISTS (
    SELECT 1 FROM sync_publication_intents parent
    JOIN files ON files.asset_uuid = parent.asset_uuid
    JOIN roots ON roots.id = files.root_id
    WHERE parent.intent_uuid = NEW.intent_uuid
      AND roots.root_folder_uuid IS NOT NULL
  )
BEGIN SELECT RAISE(ABORT, 'managed publication job has no root ownership'); END;

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
VALUES (31, 'sync_root_generation_fence', unixepoch());

COMMIT;
