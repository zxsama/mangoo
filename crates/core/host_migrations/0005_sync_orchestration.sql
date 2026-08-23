-- Durable Host-wide synchronization orchestration state.
BEGIN IMMEDIATE;

ALTER TABLE host_meta ADD COLUMN sync_status_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE host_meta ADD COLUMN sync_scheduler_state TEXT NOT NULL DEFAULT 'stopped' CHECK(sync_scheduler_state IN ('starting','running','degraded','stopped'));
ALTER TABLE library_sync_bindings ADD COLUMN pending_upload_items INTEGER NOT NULL DEFAULT 0 CHECK(pending_upload_items >= 0);
ALTER TABLE library_sync_bindings ADD COLUMN pending_upload_bytes INTEGER NOT NULL DEFAULT 0 CHECK(pending_upload_bytes >= 0);
ALTER TABLE library_sync_bindings ADD COLUMN pending_download_items INTEGER NOT NULL DEFAULT 0 CHECK(pending_download_items >= 0);
ALTER TABLE library_sync_bindings ADD COLUMN pending_download_bytes INTEGER NOT NULL DEFAULT 0 CHECK(pending_download_bytes >= 0);
ALTER TABLE library_sync_bindings ADD COLUMN conflict_count INTEGER NOT NULL DEFAULT 0 CHECK(conflict_count >= 0);
ALTER TABLE provider_remote_libraries ADD COLUMN name TEXT;
ALTER TABLE provider_remote_libraries ADD COLUMN icon TEXT;
ALTER TABLE provider_remote_libraries ADD COLUMN media_count INTEGER CHECK(media_count IS NULL OR media_count >= 0);
ALTER TABLE provider_remote_libraries ADD COLUMN logical_bytes INTEGER CHECK(logical_bytes IS NULL OR logical_bytes >= 0);
ALTER TABLE provider_remote_libraries ADD COLUMN last_synced_at INTEGER;
ALTER TABLE provider_remote_libraries ADD COLUMN compatible INTEGER NOT NULL DEFAULT 0 CHECK(compatible IN (0,1));
ALTER TABLE provider_remote_libraries ADD COLUMN descriptor_error_code TEXT;

CREATE TABLE sync_orchestration_operations (
  operation_uuid TEXT PRIMARY KEY,
  library_uuid TEXT REFERENCES libraries(library_uuid) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN ('enable','pause','resume','sync-now','retry','unbind','reconnect','restore')),
  state TEXT NOT NULL CHECK(state IN ('pending','running','waiting','complete','failed','cancelled')),
  phase TEXT NOT NULL,
  request_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(request_json)),
  result_json TEXT CHECK(result_json IS NULL OR json_valid(result_json)),
  error_code TEXT,
  progress_items_done INTEGER NOT NULL DEFAULT 0 CHECK(progress_items_done >= 0),
  progress_items_total INTEGER CHECK(progress_items_total IS NULL OR progress_items_total >= 0),
  progress_bytes_done INTEGER NOT NULL DEFAULT 0 CHECK(progress_bytes_done >= 0),
  progress_bytes_total INTEGER CHECK(progress_bytes_total IS NULL OR progress_bytes_total >= 0),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE sync_preflight_tokens (
  token TEXT PRIMARY KEY,
  library_uuid TEXT NOT NULL REFERENCES libraries(library_uuid) ON DELETE CASCADE,
  plugin_id TEXT NOT NULL,
  account_uuid TEXT NOT NULL REFERENCES plugin_accounts(account_uuid) ON DELETE CASCADE,
  remote_library_uuid TEXT,
  expires_at INTEGER NOT NULL,
  consumed_at INTEGER,
  created_at INTEGER NOT NULL
);

CREATE TABLE sync_unbind_plans (
  plan_uuid TEXT PRIMARY KEY,
  library_uuid TEXT NOT NULL REFERENCES libraries(library_uuid) ON DELETE CASCADE,
  binding_epoch BLOB NOT NULL CHECK(length(binding_epoch) = 16),
  transport_generation INTEGER NOT NULL CHECK(transport_generation >= 1),
  cutover_actor_seq INTEGER NOT NULL CHECK(cutover_actor_seq >= 0),
  remote_watermark INTEGER NOT NULL CHECK(remote_watermark >= 0),
  pending_count INTEGER NOT NULL CHECK(pending_count >= 0),
  published_count INTEGER NOT NULL CHECK(published_count >= 0),
  unpublished_count INTEGER NOT NULL CHECK(unpublished_count >= 0),
  origin_identity TEXT,
  state TEXT NOT NULL CHECK(state IN ('planned','cutover','reconciling','complete','failed')),
  choice TEXT CHECK(choice IS NULL OR choice IN ('drain-before-unbind','keep-local-fork')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE sync_reconnect_plans (
  plan_uuid TEXT PRIMARY KEY,
  library_uuid TEXT NOT NULL REFERENCES libraries(library_uuid) ON DELETE CASCADE,
  account_uuid TEXT NOT NULL REFERENCES plugin_accounts(account_uuid) ON DELETE RESTRICT,
  remote_library_uuid TEXT NOT NULL,
  origin_identity TEXT,
  local_divergence_count INTEGER NOT NULL DEFAULT 0 CHECK(local_divergence_count >= 0),
  remote_divergence_count INTEGER NOT NULL DEFAULT 0 CHECK(remote_divergence_count >= 0),
  remote_exists INTEGER NOT NULL DEFAULT 0 CHECK(remote_exists IN (0,1)),
  remote_compatible INTEGER NOT NULL DEFAULT 0 CHECK(remote_compatible IN (0,1)),
  same_remote_identity INTEGER NOT NULL DEFAULT 0 CHECK(same_remote_identity IN (0,1)),
  state TEXT NOT NULL CHECK(state IN ('planned','committed','failed')),
  choice TEXT CHECK(choice IS NULL OR choice IN ('merge-local-fork','publish-new-remote')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE sync_restore_plans (
  restore_uuid TEXT PRIMARY KEY,
  operation_uuid TEXT NOT NULL REFERENCES sync_orchestration_operations(operation_uuid) ON DELETE CASCADE,
  account_uuid TEXT NOT NULL REFERENCES plugin_accounts(account_uuid) ON DELETE RESTRICT,
  library_uuid TEXT NOT NULL,
  name TEXT NOT NULL,
  managed_root TEXT NOT NULL COLLATE NOCASE,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  state TEXT NOT NULL CHECK(state IN ('planned','restoring','catalog-ready','complete','paused','cancelled','failed')),
  phase TEXT NOT NULL,
  catalog_ready INTEGER NOT NULL DEFAULT 0 CHECK(catalog_ready IN (0,1)),
  media_items_done INTEGER NOT NULL DEFAULT 0 CHECK(media_items_done >= 0),
  media_items_total INTEGER CHECK(media_items_total IS NULL OR media_items_total >= 0),
  media_bytes_done INTEGER NOT NULL DEFAULT 0 CHECK(media_bytes_done >= 0),
  media_bytes_total INTEGER CHECK(media_bytes_total IS NULL OR media_bytes_total >= 0),
  error_code TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(account_uuid, library_uuid),
  UNIQUE(managed_root)
);

CREATE INDEX idx_sync_operations_library ON sync_orchestration_operations(library_uuid, updated_at DESC, operation_uuid);
CREATE INDEX idx_sync_operations_active ON sync_orchestration_operations(state, updated_at, operation_uuid) WHERE state IN ('pending','running','waiting','failed');
CREATE INDEX idx_sync_unbind_plans_state ON sync_unbind_plans(library_uuid, state, updated_at);
CREATE INDEX idx_sync_reconnect_plans_state ON sync_reconnect_plans(library_uuid, state, updated_at);
CREATE INDEX idx_sync_restore_plans_state ON sync_restore_plans(state, updated_at, restore_uuid);
CREATE INDEX idx_sync_restore_plans_operation ON sync_restore_plans(operation_uuid, created_at, restore_uuid);
CREATE INDEX idx_sync_preflight_expiry ON sync_preflight_tokens(expires_at, consumed_at);

UPDATE host_meta SET schema_version = 5 WHERE singleton = 1 AND schema_version < 5;

COMMIT;
