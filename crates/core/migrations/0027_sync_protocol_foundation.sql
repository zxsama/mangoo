-- Provider-neutral synchronization truth and durable work queues.
-- UUIDs stay beside the existing INTEGER keys so browse queries retain their
-- current clustered/FK shape.  The nullable declaration is required by
-- SQLite ALTER TABLE; the migration backfills every row and the insert
-- triggers populate all future legacy inserts before they become visible.
BEGIN IMMEDIATE;

ALTER TABLE roots ADD COLUMN root_uuid BLOB CHECK(root_uuid IS NULL OR length(root_uuid) = 16);
ALTER TABLE folders ADD COLUMN folder_uuid BLOB CHECK(folder_uuid IS NULL OR length(folder_uuid) = 16);
ALTER TABLE files ADD COLUMN asset_uuid BLOB CHECK(asset_uuid IS NULL OR length(asset_uuid) = 16);
ALTER TABLE tags ADD COLUMN tag_uuid BLOB CHECK(tag_uuid IS NULL OR length(tag_uuid) = 16);

UPDATE roots SET root_uuid = randomblob(16) WHERE root_uuid IS NULL;
UPDATE folders SET folder_uuid = randomblob(16) WHERE folder_uuid IS NULL;
UPDATE files SET asset_uuid = randomblob(16) WHERE asset_uuid IS NULL;
UPDATE tags SET tag_uuid = randomblob(16) WHERE tag_uuid IS NULL;

CREATE UNIQUE INDEX ux_roots_root_uuid ON roots(root_uuid) WHERE root_uuid IS NOT NULL;
CREATE UNIQUE INDEX ux_folders_folder_uuid ON folders(folder_uuid) WHERE folder_uuid IS NOT NULL;
CREATE UNIQUE INDEX ux_files_asset_uuid ON files(asset_uuid) WHERE asset_uuid IS NOT NULL;
CREATE UNIQUE INDEX ux_tags_tag_uuid ON tags(tag_uuid) WHERE tag_uuid IS NOT NULL;

CREATE TRIGGER roots_assign_uuid AFTER INSERT ON roots
WHEN NEW.root_uuid IS NULL
BEGIN
  UPDATE roots SET root_uuid = randomblob(16) WHERE id = NEW.id;
END;
CREATE TRIGGER folders_assign_uuid AFTER INSERT ON folders
WHEN NEW.folder_uuid IS NULL
BEGIN
  UPDATE folders SET folder_uuid = randomblob(16) WHERE id = NEW.id;
END;
CREATE TRIGGER files_assign_uuid AFTER INSERT ON files
WHEN NEW.asset_uuid IS NULL
BEGIN
  UPDATE files SET asset_uuid = randomblob(16) WHERE id = NEW.id;
END;
CREATE TRIGGER tags_assign_uuid AFTER INSERT ON tags
WHEN NEW.tag_uuid IS NULL
BEGIN
  UPDATE tags SET tag_uuid = randomblob(16) WHERE id = NEW.id;
END;
CREATE TRIGGER roots_keep_uuid BEFORE UPDATE OF root_uuid ON roots
WHEN NEW.root_uuid IS NULL
BEGIN SELECT RAISE(ABORT, 'roots.root_uuid cannot be cleared'); END;
CREATE TRIGGER folders_keep_uuid BEFORE UPDATE OF folder_uuid ON folders
WHEN NEW.folder_uuid IS NULL
BEGIN SELECT RAISE(ABORT, 'folders.folder_uuid cannot be cleared'); END;
CREATE TRIGGER files_keep_uuid BEFORE UPDATE OF asset_uuid ON files
WHEN NEW.asset_uuid IS NULL
BEGIN SELECT RAISE(ABORT, 'files.asset_uuid cannot be cleared'); END;
CREATE TRIGGER tags_keep_uuid BEFORE UPDATE OF tag_uuid ON tags
WHEN NEW.tag_uuid IS NULL
BEGIN SELECT RAISE(ABORT, 'tags.tag_uuid cannot be cleared'); END;

CREATE TABLE sync_local_state (
  singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
  library_uuid BLOB NOT NULL CHECK(length(library_uuid) = 16),
  device_uuid BLOB NOT NULL CHECK(length(device_uuid) = 16),
  catalog_stream_uuid BLOB NOT NULL CHECK(length(catalog_stream_uuid) = 16),
  active_actor_epoch BLOB CHECK(active_actor_epoch IS NULL OR length(active_actor_epoch) = 16),
  active_binding_epoch BLOB CHECK(active_binding_epoch IS NULL OR length(active_binding_epoch) = 16),
  next_actor_seq INTEGER NOT NULL DEFAULT 1 CHECK(next_actor_seq >= 1),
  next_local_mutation_generation INTEGER NOT NULL DEFAULT 1 CHECK(next_local_mutation_generation >= 1),
  hlc_wall_ms INTEGER NOT NULL DEFAULT 0 CHECK(hlc_wall_ms >= 0),
  hlc_logical INTEGER NOT NULL DEFAULT 0 CHECK(hlc_logical >= 0),
  capture_enabled INTEGER NOT NULL DEFAULT 0 CHECK(capture_enabled IN (0,1)),
  compatibility_state TEXT NOT NULL DEFAULT 'compatible' CHECK(compatibility_state IN ('compatible','compatibility-lock')),
  updated_at INTEGER NOT NULL
);

CREATE TABLE sync_actor_epochs (
  actor_epoch BLOB PRIMARY KEY CHECK(length(actor_epoch) = 16),
  device_uuid BLOB NOT NULL CHECK(length(device_uuid) = 16),
  stream_uuid BLOB NOT NULL CHECK(length(stream_uuid) = 16),
  stream_kind TEXT NOT NULL CHECK(stream_kind IN ('catalog','host-metadata')),
  binding_epoch BLOB CHECK(binding_epoch IS NULL OR length(binding_epoch) = 16),
  state TEXT NOT NULL CHECK(state IN ('active','closed','archived','forked')),
  next_actor_seq INTEGER NOT NULL DEFAULT 1 CHECK(next_actor_seq >= 1),
  published_through_seq INTEGER NOT NULL DEFAULT 0 CHECK(published_through_seq >= 0),
  created_at INTEGER NOT NULL,
  closed_at INTEGER,
  UNIQUE(device_uuid, stream_uuid, actor_epoch)
);

CREATE TABLE sync_actor_frontiers (
  device_uuid BLOB NOT NULL CHECK(length(device_uuid) = 16),
  stream_uuid BLOB NOT NULL CHECK(length(stream_uuid) = 16),
  actor_epoch BLOB NOT NULL CHECK(length(actor_epoch) = 16),
  applied_through_seq INTEGER NOT NULL DEFAULT 0 CHECK(applied_through_seq >= 0),
  expected_next_seq INTEGER NOT NULL DEFAULT 1 CHECK(expected_next_seq >= 1),
  state TEXT NOT NULL DEFAULT 'ready' CHECK(state IN ('ready','gap','forked','corrupt','unknown-schema')),
  blocking_segment_uuid BLOB CHECK(blocking_segment_uuid IS NULL OR length(blocking_segment_uuid) = 16),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(device_uuid, stream_uuid, actor_epoch)
);

CREATE TABLE sync_events (
  event_uuid BLOB PRIMARY KEY CHECK(length(event_uuid) = 16),
  library_uuid BLOB NOT NULL CHECK(length(library_uuid) = 16),
  entity_uuid BLOB CHECK(entity_uuid IS NULL OR length(entity_uuid) = 16),
  stream_uuid BLOB NOT NULL CHECK(length(stream_uuid) = 16),
  stream_kind TEXT NOT NULL CHECK(stream_kind IN ('catalog','host-metadata')),
  device_uuid BLOB NOT NULL CHECK(length(device_uuid) = 16),
  actor_epoch BLOB NOT NULL CHECK(length(actor_epoch) = 16),
  binding_epoch BLOB CHECK(binding_epoch IS NULL OR length(binding_epoch) = 16),
  actor_seq INTEGER NOT NULL CHECK(actor_seq >= 1),
  local_mutation_generation INTEGER CHECK(local_mutation_generation IS NULL OR local_mutation_generation >= 1),
  hlc_wall_ms INTEGER NOT NULL CHECK(hlc_wall_ms >= 0),
  hlc_logical INTEGER NOT NULL CHECK(hlc_logical >= 0),
  wire_version INTEGER NOT NULL CHECK(wire_version >= 1),
  envelope_version INTEGER NOT NULL CHECK(envelope_version >= 1),
  reducer_version INTEGER NOT NULL CHECK(reducer_version >= 1),
  key_version INTEGER NOT NULL DEFAULT 0 CHECK(key_version >= 0),
  operation TEXT NOT NULL,
  apply_state TEXT NOT NULL CHECK(apply_state IN ('local','pending','applied','blocked','quarantined')),
  integrity_hash BLOB NOT NULL CHECK(length(integrity_hash) = 32),
  created_at INTEGER NOT NULL,
  applied_at INTEGER,
  payload BLOB NOT NULL,
  UNIQUE(device_uuid, stream_uuid, actor_epoch, actor_seq)
);

CREATE TABLE sync_outbox (
  event_uuid BLOB PRIMARY KEY REFERENCES sync_events(event_uuid) ON DELETE RESTRICT CHECK(length(event_uuid) = 16),
  binding_epoch BLOB NOT NULL CHECK(length(binding_epoch) = 16),
  state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','sealed','published','acked','archived')),
  segment_uuid BLOB CHECK(segment_uuid IS NULL OR length(segment_uuid) = 16),
  attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
  next_attempt_at INTEGER,
  created_at INTEGER NOT NULL,
  published_at INTEGER,
  acked_at INTEGER
);

CREATE TABLE sync_segments (
  segment_uuid BLOB PRIMARY KEY CHECK(length(segment_uuid) = 16),
  stream_uuid BLOB NOT NULL CHECK(length(stream_uuid) = 16),
  stream_kind TEXT NOT NULL CHECK(stream_kind IN ('catalog','host-metadata')),
  device_uuid BLOB NOT NULL CHECK(length(device_uuid) = 16),
  actor_epoch BLOB NOT NULL CHECK(length(actor_epoch) = 16),
  binding_epoch BLOB CHECK(binding_epoch IS NULL OR length(binding_epoch) = 16),
  first_seq INTEGER NOT NULL CHECK(first_seq >= 1),
  last_seq INTEGER NOT NULL CHECK(last_seq >= first_seq),
  event_count INTEGER NOT NULL CHECK(event_count = last_seq - first_seq + 1),
  wire_version INTEGER NOT NULL CHECK(wire_version >= 1),
  reducer_version INTEGER NOT NULL CHECK(reducer_version >= 1),
  compression TEXT NOT NULL CHECK(compression IN ('none','zstd')),
  decoded_bytes INTEGER NOT NULL CHECK(decoded_bytes >= 0),
  predecessor_hash BLOB CHECK(predecessor_hash IS NULL OR length(predecessor_hash) = 32),
  payload_hash BLOB NOT NULL CHECK(length(payload_hash) = 32),
  spool_path TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('building','sealed','published','acked','archived')),
  created_at INTEGER NOT NULL,
  UNIQUE(device_uuid, stream_uuid, actor_epoch, first_seq),
  UNIQUE(device_uuid, stream_uuid, actor_epoch, last_seq)
);

CREATE TABLE sync_inbox_segments (
  segment_uuid BLOB PRIMARY KEY CHECK(length(segment_uuid) = 16),
  stream_uuid BLOB NOT NULL CHECK(length(stream_uuid) = 16),
  device_uuid BLOB NOT NULL CHECK(length(device_uuid) = 16),
  actor_epoch BLOB NOT NULL CHECK(length(actor_epoch) = 16),
  first_seq INTEGER NOT NULL CHECK(first_seq >= 1),
  last_seq INTEGER NOT NULL CHECK(last_seq >= first_seq),
  event_count INTEGER NOT NULL CHECK(event_count = last_seq - first_seq + 1),
  payload_hash BLOB NOT NULL CHECK(length(payload_hash) = 32),
  spool_path TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('durable','ready','applying','applied','gap','quarantined','unknown-schema')),
  error_code TEXT,
  received_at INTEGER NOT NULL,
  applied_at INTEGER,
  UNIQUE(device_uuid, stream_uuid, actor_epoch, first_seq, last_seq)
);

CREATE TABLE sync_field_versions (
  entity_uuid BLOB NOT NULL CHECK(length(entity_uuid) = 16),
  field_name TEXT NOT NULL,
  event_uuid BLOB NOT NULL REFERENCES sync_events(event_uuid) ON DELETE RESTRICT CHECK(length(event_uuid) = 16),
  is_winner INTEGER NOT NULL CHECK(is_winner IN (0,1)),
  value BLOB,
  PRIMARY KEY(entity_uuid, field_name, event_uuid)
);

CREATE TABLE sync_set_adds (
  owner_uuid BLOB NOT NULL CHECK(length(owner_uuid) = 16),
  member_uuid BLOB NOT NULL CHECK(length(member_uuid) = 16),
  add_event_uuid BLOB NOT NULL REFERENCES sync_events(event_uuid) ON DELETE RESTRICT CHECK(length(add_event_uuid) = 16),
  PRIMARY KEY(owner_uuid, member_uuid, add_event_uuid)
);

CREATE TABLE sync_set_removals (
  remove_event_uuid BLOB NOT NULL REFERENCES sync_events(event_uuid) ON DELETE RESTRICT CHECK(length(remove_event_uuid) = 16),
  observed_add_event_uuid BLOB NOT NULL REFERENCES sync_events(event_uuid) ON DELETE RESTRICT CHECK(length(observed_add_event_uuid) = 16),
  PRIMARY KEY(remove_event_uuid, observed_add_event_uuid)
);

CREATE TABLE sync_location_revisions (
  revision_uuid BLOB PRIMARY KEY CHECK(length(revision_uuid) = 16),
  entity_uuid BLOB NOT NULL CHECK(length(entity_uuid) = 16),
  event_uuid BLOB REFERENCES sync_events(event_uuid) ON DELETE RESTRICT CHECK(event_uuid IS NULL OR length(event_uuid) = 16),
  parent_uuid BLOB CHECK(parent_uuid IS NULL OR length(parent_uuid) = 16),
  name TEXT NOT NULL,
  local_mutation_generation INTEGER NOT NULL CHECK(local_mutation_generation >= 1),
  origin TEXT NOT NULL CHECK(origin IN ('local_api','external_fs','bootstrap','remote_apply','recovery','materialize')),
  created_at INTEGER NOT NULL
);

CREATE TABLE sync_location_heads (
  entity_uuid BLOB NOT NULL CHECK(length(entity_uuid) = 16),
  revision_uuid BLOB NOT NULL REFERENCES sync_location_revisions(revision_uuid) ON DELETE RESTRICT CHECK(length(revision_uuid) = 16),
  is_display_winner INTEGER NOT NULL DEFAULT 0 CHECK(is_display_winner IN (0,1)),
  PRIMARY KEY(entity_uuid, revision_uuid)
);

CREATE TABLE sync_location_supersedes (
  revision_uuid BLOB NOT NULL REFERENCES sync_location_revisions(revision_uuid) ON DELETE CASCADE CHECK(length(revision_uuid) = 16),
  observed_revision_uuid BLOB NOT NULL REFERENCES sync_location_revisions(revision_uuid) ON DELETE RESTRICT CHECK(length(observed_revision_uuid) = 16),
  PRIMARY KEY(revision_uuid, observed_revision_uuid)
);

CREATE TABLE sync_tombstone_tokens (
  token_uuid BLOB PRIMARY KEY CHECK(length(token_uuid) = 16),
  entity_uuid BLOB NOT NULL CHECK(length(entity_uuid) = 16),
  event_uuid BLOB REFERENCES sync_events(event_uuid) ON DELETE RESTRICT CHECK(event_uuid IS NULL OR length(event_uuid) = 16),
  local_mutation_generation INTEGER NOT NULL CHECK(local_mutation_generation >= 1),
  created_at INTEGER NOT NULL
);

CREATE TABLE sync_tombstone_removals (
  remove_event_uuid BLOB NOT NULL REFERENCES sync_events(event_uuid) ON DELETE RESTRICT CHECK(length(remove_event_uuid) = 16),
  observed_token_uuid BLOB NOT NULL REFERENCES sync_tombstone_tokens(token_uuid) ON DELETE RESTRICT CHECK(length(observed_token_uuid) = 16),
  PRIMARY KEY(remove_event_uuid, observed_token_uuid)
);

CREATE TABLE asset_content_revisions (
  content_revision_uuid BLOB PRIMARY KEY CHECK(length(content_revision_uuid) = 16),
  asset_uuid BLOB NOT NULL CHECK(length(asset_uuid) = 16),
  event_uuid BLOB REFERENCES sync_events(event_uuid) ON DELETE RESTRICT CHECK(event_uuid IS NULL OR length(event_uuid) = 16),
  manifest_hash BLOB CHECK(manifest_hash IS NULL OR length(manifest_hash) = 32),
  byte_size INTEGER CHECK(byte_size IS NULL OR byte_size >= 0),
  local_mutation_generation INTEGER NOT NULL CHECK(local_mutation_generation >= 1),
  created_at INTEGER NOT NULL
);

CREATE TABLE asset_content_heads (
  asset_uuid BLOB NOT NULL CHECK(length(asset_uuid) = 16),
  content_revision_uuid BLOB NOT NULL REFERENCES asset_content_revisions(content_revision_uuid) ON DELETE RESTRICT CHECK(length(content_revision_uuid) = 16),
  is_display_winner INTEGER NOT NULL DEFAULT 0 CHECK(is_display_winner IN (0,1)),
  PRIMARY KEY(asset_uuid, content_revision_uuid)
);

CREATE TABLE asset_content_supersedes (
  content_revision_uuid BLOB NOT NULL REFERENCES asset_content_revisions(content_revision_uuid) ON DELETE CASCADE CHECK(length(content_revision_uuid) = 16),
  observed_content_revision_uuid BLOB NOT NULL REFERENCES asset_content_revisions(content_revision_uuid) ON DELETE RESTRICT CHECK(length(observed_content_revision_uuid) = 16),
  PRIMARY KEY(content_revision_uuid, observed_content_revision_uuid)
);

CREATE TABLE sync_conflicts (
  conflict_uuid BLOB PRIMARY KEY CHECK(length(conflict_uuid) = 16),
  entity_uuid BLOB CHECK(entity_uuid IS NULL OR length(entity_uuid) = 16),
  conflict_kind TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'open' CHECK(state IN ('open','resolved','ignored')),
  winner_event_uuid BLOB REFERENCES sync_events(event_uuid) ON DELETE RESTRICT CHECK(winner_event_uuid IS NULL OR length(winner_event_uuid) = 16),
  details BLOB,
  created_at INTEGER NOT NULL,
  resolved_at INTEGER
);

CREATE TABLE sync_publication_intents (
  intent_uuid BLOB PRIMARY KEY CHECK(length(intent_uuid) = 16),
  asset_uuid BLOB NOT NULL CHECK(length(asset_uuid) = 16),
  content_revision_uuid BLOB NOT NULL REFERENCES asset_content_revisions(content_revision_uuid) ON DELETE RESTRICT CHECK(length(content_revision_uuid) = 16),
  event_uuid BLOB UNIQUE REFERENCES sync_events(event_uuid) ON DELETE RESTRICT CHECK(event_uuid IS NULL OR length(event_uuid) = 16),
  binding_epoch BLOB CHECK(binding_epoch IS NULL OR length(binding_epoch) = 16),
  local_mutation_generation INTEGER NOT NULL CHECK(local_mutation_generation >= 1),
  source_path TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('pending-hash','pending-manifest','ready','canonicalized','cancelled','archived','failed')),
  attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
  next_attempt_at INTEGER,
  error_code TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE sync_checkpoints (
  checkpoint_uuid BLOB PRIMARY KEY CHECK(length(checkpoint_uuid) = 16),
  schema_version INTEGER NOT NULL CHECK(schema_version >= 1),
  reducer_version INTEGER NOT NULL CHECK(reducer_version >= 1),
  state TEXT NOT NULL CHECK(state IN ('building','ready','invalid','superseded')),
  spool_path TEXT,
  payload_hash BLOB CHECK(payload_hash IS NULL OR length(payload_hash) = 32),
  state_digest BLOB CHECK(state_digest IS NULL OR length(state_digest) = 32),
  capture_local_mutation_generation INTEGER NOT NULL CHECK(capture_local_mutation_generation >= 0),
  created_at INTEGER NOT NULL,
  ready_at INTEGER
);

CREATE TABLE sync_checkpoint_watermarks (
  checkpoint_uuid BLOB NOT NULL REFERENCES sync_checkpoints(checkpoint_uuid) ON DELETE CASCADE CHECK(length(checkpoint_uuid) = 16),
  stream_uuid BLOB NOT NULL CHECK(length(stream_uuid) = 16),
  device_uuid BLOB NOT NULL CHECK(length(device_uuid) = 16),
  actor_epoch BLOB NOT NULL CHECK(length(actor_epoch) = 16),
  covered_through_seq INTEGER NOT NULL CHECK(covered_through_seq >= 0),
  PRIMARY KEY(checkpoint_uuid, stream_uuid, device_uuid, actor_epoch)
);

CREATE TABLE sync_bulk_operations (
  bulk_operation_uuid BLOB PRIMARY KEY CHECK(length(bulk_operation_uuid) = 16),
  operation_kind TEXT NOT NULL CHECK(operation_kind IN ('delete-subtree')),
  root_entity_uuid BLOB NOT NULL CHECK(length(root_entity_uuid) = 16),
  observed_local_mutation_generation INTEGER NOT NULL CHECK(observed_local_mutation_generation >= 0),
  observed_frontier BLOB NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('pending','building','publishing','complete','failed','cancelled')),
  cursor_entity_uuid BLOB CHECK(cursor_entity_uuid IS NULL OR length(cursor_entity_uuid) = 16),
  item_count INTEGER NOT NULL DEFAULT 0 CHECK(item_count >= 0),
  rolling_hash BLOB CHECK(rolling_hash IS NULL OR length(rolling_hash) = 32),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE sync_bulk_items (
  bulk_operation_uuid BLOB NOT NULL REFERENCES sync_bulk_operations(bulk_operation_uuid) ON DELETE CASCADE CHECK(length(bulk_operation_uuid) = 16),
  entity_uuid BLOB NOT NULL CHECK(length(entity_uuid) = 16),
  action TEXT NOT NULL CHECK(action IN ('publish-delete','archive-local')),
  state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','complete','failed')),
  event_uuid BLOB REFERENCES sync_events(event_uuid) ON DELETE RESTRICT CHECK(event_uuid IS NULL OR length(event_uuid) = 16),
  PRIMARY KEY(bulk_operation_uuid, entity_uuid)
);

CREATE TABLE sync_local_file_identities (
  volume_identity BLOB NOT NULL,
  file_identity BLOB NOT NULL,
  asset_uuid BLOB NOT NULL CHECK(length(asset_uuid) = 16),
  last_path TEXT NOT NULL,
  content_fingerprint BLOB,
  last_seen_at INTEGER NOT NULL,
  PRIMARY KEY(volume_identity, file_identity),
  UNIQUE(asset_uuid)
);

CREATE TABLE sync_identity_conflicts (
  conflict_uuid BLOB PRIMARY KEY CHECK(length(conflict_uuid) = 16),
  old_asset_uuid BLOB CHECK(old_asset_uuid IS NULL OR length(old_asset_uuid) = 16),
  new_asset_uuid BLOB NOT NULL CHECK(length(new_asset_uuid) = 16),
  reason TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'open' CHECK(state IN ('open','resolved','ignored')),
  created_at INTEGER NOT NULL,
  resolved_at INTEGER
);

CREATE INDEX idx_sync_actor_epochs_active ON sync_actor_epochs(state, stream_uuid, device_uuid) WHERE state = 'active';
CREATE INDEX idx_sync_actor_frontiers_state ON sync_actor_frontiers(state, updated_at) WHERE state != 'ready';
CREATE INDEX idx_sync_events_entity ON sync_events(entity_uuid, hlc_wall_ms, hlc_logical) WHERE entity_uuid IS NOT NULL;
CREATE INDEX idx_sync_outbox_pending ON sync_outbox(next_attempt_at, created_at, event_uuid) WHERE state = 'pending';
CREATE INDEX idx_sync_segments_actor_range ON sync_segments(device_uuid, stream_uuid, actor_epoch, first_seq, last_seq);
CREATE INDEX idx_sync_inbox_ready ON sync_inbox_segments(state, received_at, segment_uuid);
CREATE UNIQUE INDEX idx_sync_field_winner ON sync_field_versions(entity_uuid, field_name) WHERE is_winner = 1;
CREATE UNIQUE INDEX idx_sync_location_winner ON sync_location_heads(entity_uuid) WHERE is_display_winner = 1;
CREATE INDEX idx_sync_location_entity ON sync_location_revisions(entity_uuid, local_mutation_generation, revision_uuid);
CREATE INDEX idx_sync_tombstone_entity ON sync_tombstone_tokens(entity_uuid, local_mutation_generation, token_uuid);
CREATE INDEX idx_asset_content_entity ON asset_content_revisions(asset_uuid, local_mutation_generation, content_revision_uuid);
CREATE UNIQUE INDEX idx_asset_content_winner ON asset_content_heads(asset_uuid) WHERE is_display_winner = 1;
CREATE INDEX idx_sync_conflicts_open ON sync_conflicts(created_at, conflict_uuid) WHERE state = 'open';
CREATE INDEX idx_sync_publication_retry ON sync_publication_intents(next_attempt_at, updated_at, intent_uuid) WHERE state IN ('pending-hash','pending-manifest','ready','failed');
CREATE INDEX idx_sync_checkpoints_ready ON sync_checkpoints(created_at DESC, checkpoint_uuid) WHERE state = 'ready';
CREATE INDEX idx_sync_checkpoint_watermark_actor ON sync_checkpoint_watermarks(stream_uuid, device_uuid, actor_epoch, covered_through_seq);
CREATE INDEX idx_sync_bulk_operations_work ON sync_bulk_operations(updated_at, bulk_operation_uuid) WHERE state IN ('pending','building','publishing','failed');
CREATE INDEX idx_sync_bulk_items_pending ON sync_bulk_items(bulk_operation_uuid, entity_uuid) WHERE state = 'pending';

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (27, 'sync_protocol_foundation', unixepoch());

COMMIT;
