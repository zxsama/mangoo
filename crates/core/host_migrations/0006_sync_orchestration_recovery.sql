-- Forward-only orchestration recovery and confirmation state.
BEGIN IMMEDIATE;

ALTER TABLE sync_unbind_plans ADD COLUMN plugin_id TEXT;
ALTER TABLE sync_unbind_plans ADD COLUMN account_uuid TEXT;
ALTER TABLE sync_unbind_plans ADD COLUMN remote_namespace TEXT;
ALTER TABLE sync_unbind_plans ADD COLUMN descriptor_sha256 BLOB CHECK(descriptor_sha256 IS NULL OR length(descriptor_sha256) = 32);
ALTER TABLE sync_unbind_plans ADD COLUMN base_remote_object_count INTEGER NOT NULL DEFAULT 0 CHECK(base_remote_object_count >= 0);
ALTER TABLE sync_unbind_plans ADD COLUMN operation_uuid TEXT;
ALTER TABLE sync_unbind_plans ADD COLUMN reconcile_attempts INTEGER NOT NULL DEFAULT 0 CHECK(reconcile_attempts >= 0);
ALTER TABLE sync_unbind_plans ADD COLUMN next_retry_at INTEGER;
ALTER TABLE sync_unbind_plans ADD COLUMN last_error_code TEXT;
ALTER TABLE sync_unbind_plans ADD COLUMN remote_visibility TEXT NOT NULL DEFAULT 'unknown' CHECK(remote_visibility IN ('unknown','visible'));

ALTER TABLE sync_reconnect_plans ADD COLUMN origin_remote_watermark INTEGER NOT NULL DEFAULT 0 CHECK(origin_remote_watermark >= 0);
ALTER TABLE sync_reconnect_plans ADD COLUMN base_remote_object_count INTEGER NOT NULL DEFAULT 0 CHECK(base_remote_object_count >= 0);
ALTER TABLE sync_reconnect_plans ADD COLUMN remote_object_count INTEGER NOT NULL DEFAULT 0 CHECK(remote_object_count >= 0);
ALTER TABLE sync_reconnect_plans ADD COLUMN remote_descriptor_sha256 BLOB CHECK(remote_descriptor_sha256 IS NULL OR length(remote_descriptor_sha256) = 32);
ALTER TABLE sync_reconnect_plans ADD COLUMN remote_binding_epoch TEXT;
ALTER TABLE sync_reconnect_plans ADD COLUMN merge_allowed INTEGER NOT NULL DEFAULT 0 CHECK(merge_allowed IN (0,1));
ALTER TABLE sync_reconnect_plans ADD COLUMN publish_new_allowed INTEGER NOT NULL DEFAULT 1 CHECK(publish_new_allowed IN (0,1));

CREATE TABLE sync_restore_preflights (
  token TEXT PRIMARY KEY,
  request_json TEXT NOT NULL CHECK(json_valid(request_json)),
  total_items INTEGER NOT NULL CHECK(total_items > 0),
  total_bytes INTEGER NOT NULL CHECK(total_bytes >= 0),
  expires_at INTEGER NOT NULL,
  consumed_at INTEGER,
  created_at INTEGER NOT NULL
);

CREATE INDEX idx_sync_unbind_reconcile_retry
  ON sync_unbind_plans(state, next_retry_at, updated_at)
  WHERE state IN ('cutover','reconciling');
CREATE INDEX idx_sync_restore_preflight_expiry
  ON sync_restore_preflights(expires_at, consumed_at);

UPDATE host_meta SET schema_version = 6 WHERE singleton = 1 AND schema_version < 6;

COMMIT;
