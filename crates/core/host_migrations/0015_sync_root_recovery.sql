-- Durable stop plans, visibility reconciliation, and root re-enable work.
BEGIN IMMEDIATE;

ALTER TABLE local_root_bindings ADD COLUMN root_capture_epoch TEXT
  CHECK(root_capture_epoch IS NULL OR length(root_capture_epoch) = 36);
ALTER TABLE local_root_bindings ADD COLUMN root_actor_epoch TEXT
  CHECK(root_actor_epoch IS NULL OR length(root_actor_epoch) = 36);

CREATE TABLE root_stop_plans (
  plan_uuid TEXT PRIMARY KEY,
  operation_uuid TEXT NOT NULL UNIQUE REFERENCES root_operations(operation_uuid) ON DELETE CASCADE,
  library_uuid TEXT NOT NULL,
  root_uuid TEXT NOT NULL,
  local_binding_uuid TEXT NOT NULL,
  binding_generation INTEGER NOT NULL CHECK(binding_generation >= 1),
  root_capture_epoch TEXT NOT NULL,
  root_actor_epoch TEXT NOT NULL,
  pending_work INTEGER NOT NULL CHECK(pending_work >= 0),
  published_work INTEGER NOT NULL CHECK(published_work >= 0),
  checkpoint_uuid TEXT,
  provider_id TEXT NOT NULL,
  account_uuid TEXT NOT NULL,
  remote_namespace TEXT NOT NULL,
  provider_binding_epoch TEXT NOT NULL,
  provider_transport_generation INTEGER NOT NULL CHECK(provider_transport_generation >= 1),
  event_frontier_json TEXT NOT NULL CHECK(json_valid(event_frontier_json)),
  catalog_snapshot_json TEXT NOT NULL CHECK(json_valid(catalog_snapshot_json)),
  catalog_snapshot_digest TEXT NOT NULL CHECK(length(catalog_snapshot_digest) = 64),
  provider_receipts_json TEXT NOT NULL CHECK(json_valid(provider_receipts_json)),
  expires_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  FOREIGN KEY(library_uuid, root_uuid) REFERENCES remote_sync_roots(library_uuid, root_uuid) ON DELETE RESTRICT
);

CREATE TABLE root_visibility_reconciliations (
  snapshot_uuid TEXT PRIMARY KEY REFERENCES root_cutover_snapshots(snapshot_uuid) ON DELETE RESTRICT,
  operation_uuid TEXT NOT NULL UNIQUE REFERENCES root_operations(operation_uuid) ON DELETE RESTRICT,
  state TEXT NOT NULL CHECK(state IN ('pending','visible','hidden','retryable-unknown')),
  attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
  next_retry_at INTEGER,
  last_error_code TEXT,
  checked_at INTEGER,
  updated_at INTEGER NOT NULL
);

CREATE TABLE root_comparison_snapshots (
  plan_uuid TEXT PRIMARY KEY REFERENCES root_comparison_plans(plan_uuid) ON DELETE CASCADE,
  base_snapshot_json TEXT NOT NULL CHECK(json_valid(base_snapshot_json)),
  local_snapshot_json TEXT NOT NULL CHECK(json_valid(local_snapshot_json)),
  remote_snapshot_json TEXT NOT NULL CHECK(json_valid(remote_snapshot_json))
);

CREATE TABLE root_reconciliation_work (
  plan_uuid TEXT NOT NULL REFERENCES root_comparison_plans(plan_uuid) ON DELETE RESTRICT,
  item_ordinal INTEGER NOT NULL,
  entity_uuid TEXT NOT NULL,
  field_name TEXT NOT NULL,
  resolution TEXT NOT NULL CHECK(resolution IN ('keep-local','keep-remote','keep-both')),
  selected_value_json TEXT,
  selected_entity_json TEXT CHECK(selected_entity_json IS NULL OR json_valid(selected_entity_json)),
  apply_order INTEGER NOT NULL CHECK(apply_order >= 0),
  state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','applying','complete','failed')),
  attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
  last_error_code TEXT,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(plan_uuid, item_ordinal),
  FOREIGN KEY(plan_uuid, item_ordinal) REFERENCES root_comparison_items(plan_uuid, item_ordinal) ON DELETE RESTRICT
);

CREATE TABLE root_publish_as_new (
  operation_uuid TEXT PRIMARY KEY REFERENCES root_operations(operation_uuid) ON DELETE RESTRICT,
  enrollment_operation_uuid TEXT NOT NULL UNIQUE,
  library_uuid TEXT NOT NULL,
  source_root_uuid TEXT NOT NULL,
  new_root_uuid TEXT NOT NULL,
  new_root_folder_uuid TEXT NOT NULL,
  new_local_binding_uuid TEXT NOT NULL,
  root_capture_epoch TEXT NOT NULL,
  root_actor_epoch TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('catalog-pending','complete','failed')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(library_uuid, new_root_uuid)
);

CREATE INDEX idx_root_visibility_retry
  ON root_visibility_reconciliations(state, next_retry_at, operation_uuid);
CREATE INDEX idx_root_reconciliation_pending
  ON root_reconciliation_work(state, updated_at, plan_uuid, apply_order, item_ordinal);
CREATE INDEX idx_root_publish_as_new_recovery
  ON root_publish_as_new(state, updated_at, operation_uuid);

UPDATE local_root_bindings
SET root_capture_epoch = COALESCE(root_capture_epoch, local_binding_uuid),
    root_actor_epoch = COALESCE(root_actor_epoch, local_binding_uuid);

UPDATE host_meta SET schema_version = 15 WHERE singleton = 1 AND schema_version < 15;

COMMIT;
