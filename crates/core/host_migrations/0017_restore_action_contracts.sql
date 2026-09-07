-- Root-aware restore inventory fences and durable per-root plans.
BEGIN IMMEDIATE;

ALTER TABLE sync_restore_preflights ADD COLUMN request_hash TEXT CHECK(request_hash IS NULL OR length(request_hash) = 64);
ALTER TABLE sync_restore_preflights ADD COLUMN result_json TEXT CHECK(result_json IS NULL OR json_valid(result_json));
ALTER TABLE sync_restore_preflights ADD COLUMN state TEXT NOT NULL DEFAULT 'ready' CHECK(state IN ('ready','claimed','completed','failed-recoverable'));
ALTER TABLE sync_restore_preflights ADD COLUMN operation_uuid TEXT;

ALTER TABLE sync_restore_plans ADD COLUMN restore_mode TEXT NOT NULL DEFAULT 'legacy-library' CHECK(restore_mode IN ('legacy-library','root-aware'));
ALTER TABLE sync_restore_plans ADD COLUMN inventory_fence TEXT;
ALTER TABLE sync_restore_plans ADD COLUMN request_hash TEXT CHECK(request_hash IS NULL OR length(request_hash) = 64);

CREATE TABLE sync_restore_root_plans (
  restore_uuid TEXT NOT NULL REFERENCES sync_restore_plans(restore_uuid) ON DELETE CASCADE,
  root_uuid TEXT NOT NULL,
  root_folder_uuid TEXT NOT NULL,
  requested_name TEXT NOT NULL,
  materialized_name TEXT NOT NULL,
  materialization_fingerprint TEXT NOT NULL CHECK(length(materialization_fingerprint) = 64),
  logical_revision INTEGER NOT NULL CHECK(logical_revision >= 0),
  selected INTEGER NOT NULL CHECK(selected IN (0,1)),
  target_path TEXT,
  canonical_target_key TEXT COLLATE NOCASE,
  local_binding_uuid TEXT,
  binding_generation INTEGER CHECK(binding_generation IS NULL OR binding_generation >= 1),
  logical_bytes INTEGER CHECK(logical_bytes IS NULL OR logical_bytes >= 0),
  phase TEXT NOT NULL CHECK(phase IN ('planned','inventory-imported','binding-staged','catalog-ready','completed','compensated')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(restore_uuid, root_uuid),
  CHECK((selected = 1 AND target_path IS NOT NULL AND canonical_target_key IS NOT NULL)
     OR (selected = 0 AND target_path IS NULL AND canonical_target_key IS NULL)),
  CHECK((selected = 1 AND local_binding_uuid IS NOT NULL AND binding_generation IS NOT NULL)
     OR (selected = 0 AND local_binding_uuid IS NULL AND binding_generation IS NULL))
);

CREATE INDEX idx_sync_restore_root_plans_phase
  ON sync_restore_root_plans(restore_uuid, phase, root_uuid);
CREATE INDEX idx_sync_restore_root_plans_target
  ON sync_restore_root_plans(canonical_target_key)
  WHERE selected = 1;

UPDATE host_meta SET schema_version = 17 WHERE singleton = 1 AND schema_version < 17;

COMMIT;
