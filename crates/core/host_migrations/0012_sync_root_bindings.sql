-- Device-local synchronized-root bindings and durable managed-root v2 journals.
BEGIN IMMEDIATE;

CREATE UNIQUE INDEX ux_libraries_library_root_identity
  ON libraries(library_uuid, root_uuid);

CREATE TABLE library_local_settings (
  library_uuid TEXT PRIMARY KEY,
  library_root_uuid TEXT NOT NULL UNIQUE CHECK(length(library_root_uuid) = 36 AND library_root_uuid = lower(library_root_uuid) AND substr(library_root_uuid,9,1) = '-' AND substr(library_root_uuid,14,1) = '-' AND substr(library_root_uuid,19,1) = '-' AND substr(library_root_uuid,24,1) = '-' AND length(replace(library_root_uuid,'-','')) = 32 AND replace(library_root_uuid,'-','') NOT GLOB '*[^0-9a-f]*'),
  default_placement_base TEXT NOT NULL,
  canonical_placement_key TEXT NOT NULL COLLATE NOCASE,
  operation_revision INTEGER NOT NULL DEFAULT 0 CHECK(operation_revision >= 0),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  FOREIGN KEY(library_uuid, library_root_uuid) REFERENCES libraries(library_uuid, root_uuid) ON DELETE CASCADE
);

CREATE TABLE remote_sync_roots (
  library_uuid TEXT NOT NULL REFERENCES libraries(library_uuid) ON DELETE CASCADE,
  root_uuid TEXT NOT NULL CHECK(length(root_uuid) = 36 AND root_uuid = lower(root_uuid) AND substr(root_uuid,9,1) = '-' AND substr(root_uuid,14,1) = '-' AND substr(root_uuid,19,1) = '-' AND substr(root_uuid,24,1) = '-' AND length(replace(root_uuid,'-','')) = 32 AND replace(root_uuid,'-','') NOT GLOB '*[^0-9a-f]*'),
  root_folder_uuid TEXT NOT NULL CHECK(length(root_folder_uuid) = 36 AND root_folder_uuid = lower(root_folder_uuid) AND substr(root_folder_uuid,9,1) = '-' AND substr(root_folder_uuid,14,1) = '-' AND substr(root_folder_uuid,19,1) = '-' AND substr(root_folder_uuid,24,1) = '-' AND length(replace(root_folder_uuid,'-','')) = 32 AND replace(root_folder_uuid,'-','') NOT GLOB '*[^0-9a-f]*'),
  requested_name TEXT NOT NULL CHECK(length(requested_name) > 0),
  materialized_name TEXT NOT NULL CHECK(length(materialized_name) > 0),
  materialized_name_key TEXT NOT NULL COLLATE NOCASE,
  materialization_fingerprint TEXT NOT NULL CHECK(length(materialization_fingerprint) = 64),
  logical_revision INTEGER NOT NULL DEFAULT 0 CHECK(logical_revision >= 0),
  lifecycle TEXT NOT NULL CHECK(lifecycle IN ('active','tombstoned')),
  inventory_state TEXT NOT NULL CHECK(inventory_state IN ('unselected','placement-required','tombstoned','incompatible','inventory-error')),
  logical_bytes INTEGER CHECK(logical_bytes IS NULL OR logical_bytes >= 0),
  checkpoint_uuid TEXT CHECK(checkpoint_uuid IS NULL OR (length(checkpoint_uuid) = 36 AND checkpoint_uuid = lower(checkpoint_uuid) AND substr(checkpoint_uuid,9,1) = '-' AND substr(checkpoint_uuid,14,1) = '-' AND substr(checkpoint_uuid,19,1) = '-' AND substr(checkpoint_uuid,24,1) = '-' AND length(replace(checkpoint_uuid,'-','')) = 32 AND replace(checkpoint_uuid,'-','') NOT GLOB '*[^0-9a-f]*')),
  inventory_revision INTEGER NOT NULL DEFAULT 0 CHECK(inventory_revision >= 0),
  operation_revision INTEGER NOT NULL DEFAULT 0 CHECK(operation_revision >= 0),
  last_error_code TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(library_uuid, root_uuid),
  UNIQUE(library_uuid, root_folder_uuid)
);

CREATE UNIQUE INDEX ux_remote_sync_roots_active_materialized_name
  ON remote_sync_roots(library_uuid, materialized_name_key)
  WHERE lifecycle = 'active';

CREATE TRIGGER remote_sync_roots_keep_identity
BEFORE UPDATE OF library_uuid, root_uuid, root_folder_uuid ON remote_sync_roots
WHEN NEW.library_uuid != OLD.library_uuid
  OR NEW.root_uuid != OLD.root_uuid
  OR NEW.root_folder_uuid != OLD.root_folder_uuid
BEGIN
  SELECT RAISE(ABORT, 'remote sync root identity is immutable');
END;

CREATE TABLE local_root_bindings (
  library_uuid TEXT NOT NULL,
  root_uuid TEXT NOT NULL,
  local_binding_uuid TEXT NOT NULL UNIQUE CHECK(length(local_binding_uuid) = 36 AND local_binding_uuid = lower(local_binding_uuid) AND substr(local_binding_uuid,9,1) = '-' AND substr(local_binding_uuid,14,1) = '-' AND substr(local_binding_uuid,19,1) = '-' AND substr(local_binding_uuid,24,1) = '-' AND length(replace(local_binding_uuid,'-','')) = 32 AND replace(local_binding_uuid,'-','') NOT GLOB '*[^0-9a-f]*'),
  local_path TEXT NOT NULL,
  canonical_path_key TEXT NOT NULL UNIQUE COLLATE NOCASE,
  placement_parent TEXT NOT NULL,
  placement_parent_key TEXT NOT NULL COLLATE NOCASE,
  marker_identity TEXT,
  filesystem_identity TEXT,
  state TEXT NOT NULL CHECK(state IN ('materializing','active','paused','local-only-fork','reconcile-required','relocating','local-rename-required','unavailable','error')),
  binding_generation INTEGER NOT NULL DEFAULT 1 CHECK(binding_generation >= 1),
  desired_materialized_name TEXT NOT NULL CHECK(length(desired_materialized_name) > 0),
  desired_materialized_name_key TEXT NOT NULL COLLATE NOCASE,
  desired_materialization_fingerprint TEXT NOT NULL CHECK(length(desired_materialization_fingerprint) = 64),
  applied_materialized_name TEXT,
  applied_materialized_name_key TEXT COLLATE NOCASE,
  applied_materialization_fingerprint TEXT CHECK(applied_materialization_fingerprint IS NULL OR length(applied_materialization_fingerprint) = 64),
  operation_revision INTEGER NOT NULL DEFAULT 0 CHECK(operation_revision >= 0),
  cutover_checkpoint_uuid TEXT,
  cutover_frontier_json TEXT CHECK(cutover_frontier_json IS NULL OR json_valid(cutover_frontier_json)),
  verification_json TEXT CHECK(verification_json IS NULL OR json_valid(verification_json)),
  last_error_code TEXT,
  last_error_detail TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(library_uuid, root_uuid),
  FOREIGN KEY(library_uuid, root_uuid) REFERENCES remote_sync_roots(library_uuid, root_uuid) ON DELETE RESTRICT,
  CHECK((applied_materialized_name IS NULL) = (applied_materialized_name_key IS NULL)),
  CHECK((applied_materialized_name IS NULL) = (applied_materialization_fingerprint IS NULL))
);

CREATE UNIQUE INDEX ux_local_root_bindings_sibling_name
  ON local_root_bindings(placement_parent_key, applied_materialized_name_key)
  WHERE applied_materialized_name_key IS NOT NULL;
CREATE UNIQUE INDEX ux_local_root_bindings_desired_sibling_name
  ON local_root_bindings(placement_parent_key, desired_materialized_name_key);
CREATE INDEX idx_local_root_bindings_state
  ON local_root_bindings(state, updated_at, library_uuid, root_uuid);

CREATE TRIGGER local_root_bindings_keep_identity
BEFORE UPDATE OF library_uuid, root_uuid, local_binding_uuid ON local_root_bindings
WHEN NEW.library_uuid != OLD.library_uuid
  OR NEW.root_uuid != OLD.root_uuid
  OR NEW.local_binding_uuid != OLD.local_binding_uuid
BEGIN
  SELECT RAISE(ABORT, 'local root binding identity is immutable');
END;

CREATE TRIGGER local_root_bindings_reject_overlap_insert
BEFORE INSERT ON local_root_bindings
WHEN EXISTS (
  SELECT 1 FROM local_root_bindings existing
  WHERE NEW.canonical_path_key = existing.canonical_path_key
     OR (length(NEW.canonical_path_key) > length(existing.canonical_path_key)
         AND substr(NEW.canonical_path_key, 1, length(existing.canonical_path_key) + 1) = existing.canonical_path_key || '/')
     OR (length(existing.canonical_path_key) > length(NEW.canonical_path_key)
         AND substr(existing.canonical_path_key, 1, length(NEW.canonical_path_key) + 1) = NEW.canonical_path_key || '/')
)
BEGIN
  SELECT RAISE(ABORT, 'local root path overlaps an existing binding');
END;

CREATE TRIGGER local_root_bindings_reject_overlap_update
BEFORE UPDATE OF canonical_path_key ON local_root_bindings
WHEN EXISTS (
  SELECT 1 FROM local_root_bindings existing
  WHERE existing.local_binding_uuid != OLD.local_binding_uuid
    AND (
      NEW.canonical_path_key = existing.canonical_path_key
      OR (length(NEW.canonical_path_key) > length(existing.canonical_path_key)
          AND substr(NEW.canonical_path_key, 1, length(existing.canonical_path_key) + 1) = existing.canonical_path_key || '/')
      OR (length(existing.canonical_path_key) > length(NEW.canonical_path_key)
          AND substr(existing.canonical_path_key, 1, length(NEW.canonical_path_key) + 1) = NEW.canonical_path_key || '/')
    )
)
BEGIN
  SELECT RAISE(ABORT, 'local root path overlaps an existing binding');
END;

CREATE TRIGGER local_root_bindings_reject_placement_base_insert
BEFORE INSERT ON local_root_bindings
WHEN EXISTS (
  SELECT 1 FROM library_local_settings settings
  WHERE settings.canonical_placement_key = NEW.canonical_path_key
)
BEGIN
  SELECT RAISE(ABORT, 'placement base cannot be selected as a root');
END;

CREATE TRIGGER local_root_bindings_reject_placement_base_update
BEFORE UPDATE OF canonical_path_key ON local_root_bindings
WHEN EXISTS (
  SELECT 1 FROM library_local_settings settings
  WHERE settings.canonical_placement_key = NEW.canonical_path_key
)
BEGIN
  SELECT RAISE(ABORT, 'placement base cannot be selected as a root');
END;

CREATE TRIGGER library_local_settings_reject_bound_root_insert
BEFORE INSERT ON library_local_settings
WHEN EXISTS (
  SELECT 1 FROM local_root_bindings binding
  WHERE binding.canonical_path_key = NEW.canonical_placement_key
)
BEGIN
  SELECT RAISE(ABORT, 'placement base cannot be a selected root');
END;

CREATE TRIGGER library_local_settings_reject_bound_root_update
BEFORE UPDATE OF canonical_placement_key ON library_local_settings
WHEN EXISTS (
  SELECT 1 FROM local_root_bindings binding
  WHERE binding.canonical_path_key = NEW.canonical_placement_key
)
BEGIN
  SELECT RAISE(ABORT, 'placement base cannot be a selected root');
END;

CREATE TABLE root_operations (
  operation_uuid TEXT PRIMARY KEY CHECK(length(operation_uuid) = 36 AND operation_uuid = lower(operation_uuid) AND substr(operation_uuid,9,1) = '-' AND substr(operation_uuid,14,1) = '-' AND substr(operation_uuid,19,1) = '-' AND substr(operation_uuid,24,1) = '-' AND length(replace(operation_uuid,'-','')) = 32 AND replace(operation_uuid,'-','') NOT GLOB '*[^0-9a-f]*'),
  library_uuid TEXT NOT NULL,
  root_uuid TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('enroll','select','stop-sync','re-enable','logical-rename','relocate','automatic-rename','rebind')),
  state TEXT NOT NULL CHECK(state IN ('pending','running','waiting','completed','cancelled','failed')),
  phase TEXT NOT NULL,
  request_hash TEXT NOT NULL CHECK(length(request_hash) = 64),
  request_json TEXT NOT NULL CHECK(json_valid(request_json)),
  result_json TEXT CHECK(result_json IS NULL OR json_valid(result_json)),
  expected_operation_revision INTEGER NOT NULL CHECK(expected_operation_revision >= 0),
  expected_binding_generation INTEGER CHECK(expected_binding_generation IS NULL OR expected_binding_generation >= 1),
  operation_revision INTEGER NOT NULL DEFAULT 0 CHECK(operation_revision >= 0),
  plan_uuid TEXT,
  cutover_committed INTEGER NOT NULL DEFAULT 0 CHECK(cutover_committed IN (0,1)),
  error_code TEXT,
  error_detail TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  FOREIGN KEY(library_uuid, root_uuid) REFERENCES remote_sync_roots(library_uuid, root_uuid) ON DELETE RESTRICT,
  CHECK(
    (kind = 'enroll' AND phase IN ('planned','preflighting','reserving-identity','capturing-seed','publishing-descriptor','completed','cancelled','failed')) OR
    (kind = 'select' AND phase IN ('planned','preflighting','creating-binding','restoring-catalog','materializing','reconciling','completed','cancelled','failed')) OR
    (kind = 'stop-sync' AND phase IN ('planned','fencing','reconciling-visibility','sealing-cutover','local-only-fork','completed','cancelled','failed')) OR
    (kind = 're-enable' AND phase IN ('comparing','comparison-ready','committing','reconciling','materializing','completed','cancelled','failed')) OR
    (kind = 'rebind' AND phase IN ('planned','preflighting','verifying-existing','creating-binding','reconciling','completed','cancelled','failed')) OR
    (kind = 'logical-rename' AND phase IN ('planned','validating-heads','emitting-location','awaiting-projection','completed','cancelled','failed')) OR
    (kind IN ('relocate','automatic-rename') AND phase IN ('planned','preflighting','fenced','quiescing','transferring','verifying','catalog-cutover','host-cutover','reconciling','cleanup','preflight-failed','source-missing','target-conflict','transfer-failed','verification-failed','local-rename-required','completed','cancelled','failed'))
  ),
  CHECK(
    (state IN ('pending','running','waiting') AND phase NOT IN ('completed','cancelled','failed')) OR
    (state = 'completed' AND phase = 'completed') OR
    (state = 'cancelled' AND phase = 'cancelled') OR
    (state = 'failed' AND (
      phase = 'failed'
      OR (
        kind IN ('relocate','automatic-rename')
        AND phase IN ('preflight-failed','source-missing','target-conflict','transfer-failed','verification-failed','local-rename-required')
      )
    ))
  )
);

CREATE UNIQUE INDEX ux_root_operations_one_open
  ON root_operations(library_uuid, root_uuid)
  WHERE state IN ('pending','running','waiting');
CREATE INDEX idx_root_operations_recovery
  ON root_operations(state, updated_at, operation_uuid);

CREATE TRIGGER root_operations_keep_identity
BEFORE UPDATE OF operation_uuid, library_uuid, root_uuid, kind, request_hash, request_json,
  expected_operation_revision, expected_binding_generation, created_at
ON root_operations
WHEN NEW.operation_uuid != OLD.operation_uuid
  OR NEW.library_uuid != OLD.library_uuid
  OR NEW.root_uuid != OLD.root_uuid
  OR NEW.kind != OLD.kind
  OR NEW.request_hash != OLD.request_hash
  OR NEW.request_json != OLD.request_json
  OR NEW.expected_operation_revision != OLD.expected_operation_revision
  OR NOT (NEW.expected_binding_generation IS OLD.expected_binding_generation)
  OR NEW.created_at != OLD.created_at
BEGIN
  SELECT RAISE(ABORT, 'root operation identity and request are immutable');
END;

CREATE TABLE root_cutover_snapshots (
  snapshot_uuid TEXT PRIMARY KEY CHECK(length(snapshot_uuid) = 36 AND snapshot_uuid = lower(snapshot_uuid) AND substr(snapshot_uuid,9,1) = '-' AND substr(snapshot_uuid,14,1) = '-' AND substr(snapshot_uuid,19,1) = '-' AND substr(snapshot_uuid,24,1) = '-' AND length(replace(snapshot_uuid,'-','')) = 32 AND replace(snapshot_uuid,'-','') NOT GLOB '*[^0-9a-f]*'),
  operation_uuid TEXT NOT NULL UNIQUE REFERENCES root_operations(operation_uuid) ON DELETE RESTRICT,
  library_uuid TEXT NOT NULL,
  root_uuid TEXT NOT NULL,
  binding_generation INTEGER NOT NULL CHECK(binding_generation >= 1),
  checkpoint_uuid TEXT,
  event_frontier_json TEXT NOT NULL CHECK(json_valid(event_frontier_json)),
  catalog_snapshot_digest TEXT NOT NULL CHECK(length(catalog_snapshot_digest) = 64),
  provider_receipts_json TEXT NOT NULL CHECK(json_valid(provider_receipts_json)),
  remote_visibility TEXT NOT NULL CHECK(remote_visibility IN ('unknown','visible','hidden')),
  created_at INTEGER NOT NULL,
  FOREIGN KEY(library_uuid, root_uuid) REFERENCES remote_sync_roots(library_uuid, root_uuid) ON DELETE RESTRICT
);

CREATE TRIGGER root_cutover_snapshots_validate_operation
BEFORE INSERT ON root_cutover_snapshots
WHEN NOT EXISTS (
  SELECT 1 FROM root_operations operation
  WHERE operation.operation_uuid = NEW.operation_uuid
    AND operation.library_uuid = NEW.library_uuid
    AND operation.root_uuid = NEW.root_uuid
    AND operation.kind = 'stop-sync'
    AND operation.expected_binding_generation = NEW.binding_generation
)
BEGIN SELECT RAISE(ABORT, 'cutover snapshot operation mismatch'); END;

CREATE TRIGGER root_cutover_snapshots_immutable_update
BEFORE UPDATE ON root_cutover_snapshots
BEGIN SELECT RAISE(ABORT, 'root cutover snapshot is immutable'); END;
CREATE TRIGGER root_cutover_snapshots_immutable_delete
BEFORE DELETE ON root_cutover_snapshots
BEGIN SELECT RAISE(ABORT, 'root cutover snapshot is immutable'); END;

CREATE TABLE root_comparison_plans (
  plan_uuid TEXT PRIMARY KEY CHECK(length(plan_uuid) = 36 AND plan_uuid = lower(plan_uuid) AND substr(plan_uuid,9,1) = '-' AND substr(plan_uuid,14,1) = '-' AND substr(plan_uuid,19,1) = '-' AND substr(plan_uuid,24,1) = '-' AND length(replace(plan_uuid,'-','')) = 32 AND replace(plan_uuid,'-','') NOT GLOB '*[^0-9a-f]*'),
  operation_uuid TEXT NOT NULL UNIQUE REFERENCES root_operations(operation_uuid) ON DELETE CASCADE,
  cutover_snapshot_uuid TEXT NOT NULL REFERENCES root_cutover_snapshots(snapshot_uuid) ON DELETE RESTRICT,
  library_uuid TEXT NOT NULL,
  root_uuid TEXT NOT NULL,
  base_digest TEXT NOT NULL CHECK(length(base_digest) = 64),
  local_digest TEXT NOT NULL CHECK(length(local_digest) = 64),
  remote_digest TEXT NOT NULL CHECK(length(remote_digest) = 64),
  base_frontier_json TEXT NOT NULL CHECK(json_valid(base_frontier_json)),
  local_frontier_json TEXT NOT NULL CHECK(json_valid(local_frontier_json)),
  remote_frontier_json TEXT NOT NULL CHECK(json_valid(remote_frontier_json)),
  root_revision INTEGER NOT NULL CHECK(root_revision >= 0),
  account_generation INTEGER NOT NULL CHECK(account_generation >= 0),
  provider_generation INTEGER NOT NULL CHECK(provider_generation >= 0),
  item_count INTEGER NOT NULL DEFAULT 0 CHECK(item_count >= 0),
  conflict_count INTEGER NOT NULL DEFAULT 0 CHECK(conflict_count >= 0),
  chosen_mode TEXT CHECK(chosen_mode IS NULL OR chosen_mode IN ('merge-local-fork','publish-as-new')),
  status TEXT NOT NULL CHECK(status IN ('comparing','ready','committed','expired','cancelled','failed')),
  expires_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  FOREIGN KEY(library_uuid, root_uuid) REFERENCES remote_sync_roots(library_uuid, root_uuid) ON DELETE RESTRICT
);

CREATE TRIGGER root_comparison_plans_validate_lineage
BEFORE INSERT ON root_comparison_plans
WHEN NOT EXISTS (
  SELECT 1
  FROM root_operations operation
  JOIN root_cutover_snapshots snapshot
    ON snapshot.snapshot_uuid = NEW.cutover_snapshot_uuid
  WHERE operation.operation_uuid = NEW.operation_uuid
    AND operation.library_uuid = NEW.library_uuid
    AND operation.root_uuid = NEW.root_uuid
    AND operation.kind = 're-enable'
    AND snapshot.library_uuid = NEW.library_uuid
    AND snapshot.root_uuid = NEW.root_uuid
)
BEGIN SELECT RAISE(ABORT, 'comparison plan lineage mismatch'); END;

CREATE TRIGGER root_comparison_plans_keep_facts
BEFORE UPDATE OF operation_uuid, cutover_snapshot_uuid, library_uuid, root_uuid,
  base_digest, local_digest, remote_digest, base_frontier_json,
  local_frontier_json, remote_frontier_json, root_revision,
  account_generation, provider_generation, item_count, conflict_count,
  expires_at, created_at
ON root_comparison_plans
WHEN NEW.operation_uuid != OLD.operation_uuid
  OR NEW.cutover_snapshot_uuid != OLD.cutover_snapshot_uuid
  OR NEW.library_uuid != OLD.library_uuid
  OR NEW.root_uuid != OLD.root_uuid
  OR NEW.base_digest != OLD.base_digest
  OR NEW.local_digest != OLD.local_digest
  OR NEW.remote_digest != OLD.remote_digest
  OR NEW.base_frontier_json != OLD.base_frontier_json
  OR NEW.local_frontier_json != OLD.local_frontier_json
  OR NEW.remote_frontier_json != OLD.remote_frontier_json
  OR NEW.root_revision != OLD.root_revision
  OR NEW.account_generation != OLD.account_generation
  OR NEW.provider_generation != OLD.provider_generation
  OR NEW.item_count != OLD.item_count
  OR NEW.conflict_count != OLD.conflict_count
  OR NEW.expires_at != OLD.expires_at
  OR NEW.created_at != OLD.created_at
BEGIN SELECT RAISE(ABORT, 'comparison plan facts are immutable'); END;

CREATE TABLE root_comparison_items (
  plan_uuid TEXT NOT NULL REFERENCES root_comparison_plans(plan_uuid) ON DELETE CASCADE,
  item_ordinal INTEGER NOT NULL CHECK(item_ordinal >= 0),
  entity_uuid TEXT NOT NULL CHECK(length(entity_uuid) = 36 AND entity_uuid = lower(entity_uuid) AND substr(entity_uuid,9,1) = '-' AND substr(entity_uuid,14,1) = '-' AND substr(entity_uuid,19,1) = '-' AND substr(entity_uuid,24,1) = '-' AND length(replace(entity_uuid,'-','')) = 32 AND replace(entity_uuid,'-','') NOT GLOB '*[^0-9a-f]*'),
  field_name TEXT NOT NULL,
  classification TEXT NOT NULL CHECK(classification IN ('unchanged','local-only','remote-only','same-change','conflict','deleted-local','deleted-remote')),
  base_digest TEXT,
  local_digest TEXT,
  remote_digest TEXT,
  suggested_action TEXT CHECK(suggested_action IS NULL OR suggested_action IN ('keep-local','keep-remote','keep-both')),
  conflict_uuid TEXT CHECK(conflict_uuid IS NULL OR (length(conflict_uuid) = 36 AND conflict_uuid = lower(conflict_uuid) AND substr(conflict_uuid,9,1) = '-' AND substr(conflict_uuid,14,1) = '-' AND substr(conflict_uuid,19,1) = '-' AND substr(conflict_uuid,24,1) = '-' AND length(replace(conflict_uuid,'-','')) = 32 AND replace(conflict_uuid,'-','') NOT GLOB '*[^0-9a-f]*')),
  redacted_detail_json TEXT CHECK(redacted_detail_json IS NULL OR json_valid(redacted_detail_json)),
  PRIMARY KEY(plan_uuid, item_ordinal)
);

CREATE INDEX idx_root_comparison_items_page
  ON root_comparison_items(plan_uuid, item_ordinal);

CREATE TRIGGER root_comparison_items_immutable_update
BEFORE UPDATE ON root_comparison_items
BEGIN SELECT RAISE(ABORT, 'comparison item is immutable'); END;

CREATE TABLE root_comparison_resolutions (
  plan_uuid TEXT NOT NULL,
  item_ordinal INTEGER NOT NULL,
  resolution TEXT NOT NULL CHECK(resolution IN ('keep-local','keep-remote','keep-both')),
  resolved_at INTEGER NOT NULL,
  PRIMARY KEY(plan_uuid, item_ordinal),
  FOREIGN KEY(plan_uuid, item_ordinal) REFERENCES root_comparison_items(plan_uuid, item_ordinal) ON DELETE CASCADE
);

CREATE TABLE root_relocation_operations (
  operation_uuid TEXT PRIMARY KEY REFERENCES root_operations(operation_uuid) ON DELETE CASCADE,
  mode TEXT NOT NULL CHECK(mode IN ('explicit-move','automatic-rename')),
  desired_materialization_fingerprint TEXT NOT NULL CHECK(length(desired_materialization_fingerprint) = 64),
  source_path TEXT NOT NULL,
  source_canonical_key TEXT NOT NULL COLLATE NOCASE,
  source_filesystem_identity TEXT,
  target_path TEXT NOT NULL,
  target_canonical_key TEXT NOT NULL COLLATE NOCASE,
  target_filesystem_identity TEXT,
  staging_path TEXT,
  transferred_bytes INTEGER NOT NULL DEFAULT 0 CHECK(transferred_bytes >= 0),
  verified_bytes INTEGER NOT NULL DEFAULT 0 CHECK(verified_bytes >= 0),
  verification_digest TEXT,
  cleanup_state TEXT NOT NULL DEFAULT 'pending' CHECK(cleanup_state IN ('pending','not-required','ready','complete','failed')),
  updated_at INTEGER NOT NULL
);

CREATE TRIGGER root_relocation_operations_validate_kind
BEFORE INSERT ON root_relocation_operations
WHEN NOT EXISTS (
  SELECT 1 FROM root_operations operation
  WHERE operation.operation_uuid = NEW.operation_uuid
    AND (
      (NEW.mode = 'explicit-move' AND operation.kind = 'relocate') OR
      (NEW.mode = 'automatic-rename' AND operation.kind = 'automatic-rename')
    )
)
BEGIN SELECT RAISE(ABORT, 'relocation operation kind mismatch'); END;

CREATE TRIGGER root_relocation_operations_keep_plan
BEFORE UPDATE OF operation_uuid, mode, desired_materialization_fingerprint,
  source_path, source_canonical_key, source_filesystem_identity,
  target_path, target_canonical_key
ON root_relocation_operations
WHEN NEW.operation_uuid != OLD.operation_uuid
  OR NEW.mode != OLD.mode
  OR NEW.desired_materialization_fingerprint != OLD.desired_materialization_fingerprint
  OR NEW.source_path != OLD.source_path
  OR NEW.source_canonical_key != OLD.source_canonical_key
  OR NOT (NEW.source_filesystem_identity IS OLD.source_filesystem_identity)
  OR NEW.target_path != OLD.target_path
  OR NEW.target_canonical_key != OLD.target_canonical_key
BEGIN SELECT RAISE(ABORT, 'relocation plan is immutable'); END;

CREATE INDEX idx_remote_sync_roots_inventory
  ON remote_sync_roots(library_uuid, inventory_state, inventory_revision, root_uuid);
CREATE INDEX idx_root_comparison_plans_expiry
  ON root_comparison_plans(status, expires_at, plan_uuid);

UPDATE host_meta SET schema_version = 12 WHERE singleton = 1 AND schema_version < 12;

COMMIT;
