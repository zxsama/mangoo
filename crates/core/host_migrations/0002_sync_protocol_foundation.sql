-- Host-owned stream for Library name/icon plus per-Library transport binding.
-- Host and Catalog sequences are intentionally independent.
BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS host_sync_local_state (
  singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
  device_uuid BLOB NOT NULL CHECK(length(device_uuid) = 16),
  stream_uuid BLOB NOT NULL CHECK(length(stream_uuid) = 16),
  active_actor_epoch BLOB CHECK(active_actor_epoch IS NULL OR length(active_actor_epoch) = 16),
  next_actor_seq INTEGER NOT NULL DEFAULT 1 CHECK(next_actor_seq >= 1),
  hlc_wall_ms INTEGER NOT NULL DEFAULT 0 CHECK(hlc_wall_ms >= 0),
  hlc_logical INTEGER NOT NULL DEFAULT 0 CHECK(hlc_logical >= 0),
  capture_enabled INTEGER NOT NULL DEFAULT 0 CHECK(capture_enabled IN (0,1)),
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS host_sync_actor_epochs (
  actor_epoch BLOB PRIMARY KEY CHECK(length(actor_epoch) = 16),
  device_uuid BLOB NOT NULL CHECK(length(device_uuid) = 16),
  stream_uuid BLOB NOT NULL CHECK(length(stream_uuid) = 16),
  state TEXT NOT NULL CHECK(state IN ('active','closed','archived','forked')),
  next_actor_seq INTEGER NOT NULL DEFAULT 1 CHECK(next_actor_seq >= 1),
  published_through_seq INTEGER NOT NULL DEFAULT 0 CHECK(published_through_seq >= 0),
  created_at INTEGER NOT NULL,
  closed_at INTEGER,
  UNIQUE(device_uuid, stream_uuid, actor_epoch)
);

CREATE TABLE IF NOT EXISTS host_sync_actor_frontiers (
  device_uuid BLOB NOT NULL CHECK(length(device_uuid) = 16),
  stream_uuid BLOB NOT NULL CHECK(length(stream_uuid) = 16),
  actor_epoch BLOB NOT NULL CHECK(length(actor_epoch) = 16),
  applied_through_seq INTEGER NOT NULL DEFAULT 0 CHECK(applied_through_seq >= 0),
  expected_next_seq INTEGER NOT NULL DEFAULT 1 CHECK(expected_next_seq >= 1),
  state TEXT NOT NULL DEFAULT 'ready' CHECK(state IN ('ready','gap','forked','corrupt','unknown-schema')),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(device_uuid, stream_uuid, actor_epoch)
);

CREATE TABLE IF NOT EXISTS host_sync_events (
  event_uuid BLOB PRIMARY KEY CHECK(length(event_uuid) = 16),
  -- Ledger remains valid after a local Library copy is removed.
  library_uuid TEXT NOT NULL,
  stream_uuid BLOB NOT NULL CHECK(length(stream_uuid) = 16),
  device_uuid BLOB NOT NULL CHECK(length(device_uuid) = 16),
  actor_epoch BLOB NOT NULL CHECK(length(actor_epoch) = 16),
  binding_epoch BLOB CHECK(binding_epoch IS NULL OR length(binding_epoch) = 16),
  actor_seq INTEGER NOT NULL CHECK(actor_seq >= 1),
  hlc_wall_ms INTEGER NOT NULL CHECK(hlc_wall_ms >= 0),
  hlc_logical INTEGER NOT NULL CHECK(hlc_logical >= 0),
  wire_version INTEGER NOT NULL CHECK(wire_version >= 1),
  envelope_version INTEGER NOT NULL CHECK(envelope_version >= 1),
  reducer_version INTEGER NOT NULL CHECK(reducer_version >= 1),
  operation TEXT NOT NULL CHECK(operation IN ('library-name-set','library-icon-set')),
  apply_state TEXT NOT NULL CHECK(apply_state IN ('local','pending','applied','blocked','quarantined')),
  integrity_hash BLOB NOT NULL CHECK(length(integrity_hash) = 32),
  created_at INTEGER NOT NULL,
  applied_at INTEGER,
  payload BLOB NOT NULL,
  UNIQUE(device_uuid, stream_uuid, actor_epoch, actor_seq)
);

CREATE TABLE IF NOT EXISTS host_sync_outbox (
  event_uuid BLOB PRIMARY KEY REFERENCES host_sync_events(event_uuid) ON DELETE RESTRICT CHECK(length(event_uuid) = 16),
  library_uuid TEXT NOT NULL,
  binding_epoch BLOB NOT NULL CHECK(length(binding_epoch) = 16),
  state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','sealed','published','acked','archived')),
  segment_uuid BLOB CHECK(segment_uuid IS NULL OR length(segment_uuid) = 16),
  attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
  next_attempt_at INTEGER,
  created_at INTEGER NOT NULL,
  published_at INTEGER,
  acked_at INTEGER
);

-- Every Host scalar version is retained; exactly one row per field is the
-- materialized LWW winner. The event ledger remains the canonical history.
CREATE TABLE IF NOT EXISTS host_sync_field_versions (
  event_uuid BLOB PRIMARY KEY REFERENCES host_sync_events(event_uuid) ON DELETE RESTRICT CHECK(length(event_uuid) = 16),
  library_uuid TEXT NOT NULL,
  field TEXT NOT NULL CHECK(field IN ('name','icon')),
  value_json TEXT NOT NULL,
  hlc_wall_ms INTEGER NOT NULL CHECK(hlc_wall_ms >= 0),
  hlc_logical INTEGER NOT NULL CHECK(hlc_logical >= 0),
  device_uuid BLOB NOT NULL CHECK(length(device_uuid) = 16),
  actor_epoch BLOB NOT NULL CHECK(length(actor_epoch) = 16),
  actor_seq INTEGER NOT NULL CHECK(actor_seq >= 1),
  is_winner INTEGER NOT NULL CHECK(is_winner IN (0,1)),
  created_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_host_sync_field_winner ON host_sync_field_versions(library_uuid,field) WHERE is_winner = 1;
CREATE INDEX IF NOT EXISTS idx_host_sync_field_history ON host_sync_field_versions(library_uuid,field,hlc_wall_ms,hlc_logical,device_uuid,actor_epoch,actor_seq,event_uuid);

CREATE TABLE IF NOT EXISTS library_sync_bindings (
  library_uuid TEXT PRIMARY KEY REFERENCES libraries(library_uuid) ON DELETE CASCADE,
  provider_id TEXT NOT NULL,
  account_identity TEXT NOT NULL,
  remote_library_identity TEXT,
  binding_epoch BLOB NOT NULL CHECK(length(binding_epoch) = 16),
  actor_epoch BLOB NOT NULL CHECK(length(actor_epoch) = 16),
  state TEXT NOT NULL CHECK(state IN ('preparing','seeding','active','paused','unbinding','error','compatibility-lock')),
  transport_generation INTEGER NOT NULL DEFAULT 1 CHECK(transport_generation >= 1),
  cutover_actor_seq INTEGER NOT NULL DEFAULT 0 CHECK(cutover_actor_seq >= 0),
  published_through_seq INTEGER NOT NULL DEFAULT 0 CHECK(published_through_seq >= 0),
  latest_checkpoint_uuid BLOB CHECK(latest_checkpoint_uuid IS NULL OR length(latest_checkpoint_uuid) = 16),
  capture_generation INTEGER CHECK(capture_generation IS NULL OR capture_generation >= 0),
  poll_after INTEGER,
  retry_after INTEGER,
  last_success_at INTEGER,
  last_error_code TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_host_sync_actor_epochs_active ON host_sync_actor_epochs(state, stream_uuid, device_uuid) WHERE state = 'active';
CREATE INDEX IF NOT EXISTS idx_host_sync_frontiers_state ON host_sync_actor_frontiers(state, updated_at) WHERE state != 'ready';
CREATE INDEX IF NOT EXISTS idx_host_sync_outbox_pending ON host_sync_outbox(next_attempt_at, created_at, event_uuid) WHERE state = 'pending';
CREATE INDEX IF NOT EXISTS idx_library_sync_bindings_schedule ON library_sync_bindings(state, poll_after, retry_after, library_uuid) WHERE state IN ('preparing','seeding','active','error');

UPDATE host_meta SET schema_version = 2 WHERE singleton = 1 AND schema_version < 2;

COMMIT;
