-- Host-owned plugin runtime, account and Provider transport state.
-- Development-only transition: legacy Catalog plugin rows and old Host
-- binding rows are deliberately not migrated.
BEGIN IMMEDIATE;

DROP TABLE IF EXISTS library_sync_bindings;

CREATE TABLE plugin_installations (
  plugin_id TEXT PRIMARY KEY,
  manifest_version INTEGER NOT NULL CHECK(manifest_version = 2),
  selected_version TEXT NOT NULL,
  known_good_version TEXT,
  enabled_intent INTEGER NOT NULL DEFAULT 0 CHECK(enabled_intent IN (0,1)),
  install_root TEXT NOT NULL,
  trust_policy_id TEXT NOT NULL,
  requested_permissions_json TEXT NOT NULL CHECK(json_valid(requested_permissions_json)),
  granted_permissions_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(granted_permissions_json)),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE plugin_versions (
  plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
  version TEXT NOT NULL,
  manifest_json TEXT NOT NULL CHECK(json_valid(manifest_json)),
  entry_relative_path TEXT NOT NULL,
  entry_sha256 BLOB NOT NULL CHECK(length(entry_sha256) = 32),
  state TEXT NOT NULL CHECK(state IN ('candidate','selected','known-good','rejected','retired')),
  installed_at INTEGER NOT NULL,
  last_verified_at INTEGER,
  PRIMARY KEY(plugin_id, version)
);

CREATE TABLE plugin_runtime_state (
  plugin_id TEXT PRIMARY KEY REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
  state TEXT NOT NULL CHECK(state IN ('disabled','permission-required','starting','ready','degraded','crashed-backoff','incompatible','configuration-required')),
  selected_wire_version INTEGER CHECK(selected_wire_version IS NULL OR selected_wire_version >= 1),
  capabilities_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(capabilities_json)),
  pid INTEGER,
  session_id TEXT,
  started_at INTEGER,
  last_healthy_at INTEGER,
  crash_count INTEGER NOT NULL DEFAULT 0 CHECK(crash_count >= 0),
  backoff_until INTEGER,
  last_error_code TEXT,
  last_error_detail TEXT,
  updated_at INTEGER NOT NULL
);

CREATE TABLE plugin_accounts (
  account_uuid TEXT PRIMARY KEY,
  plugin_id TEXT NOT NULL UNIQUE REFERENCES plugin_installations(plugin_id) ON DELETE RESTRICT,
  stable_account_identity TEXT,
  display_name TEXT,
  secret_ref TEXT,
  auth_state TEXT NOT NULL CHECK(auth_state IN ('signed-out','authorization-required','authorizing','authenticated','expired','error')),
  generation INTEGER NOT NULL DEFAULT 1 CHECK(generation >= 1),
  last_error TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE provider_account_root_cursors (
  account_uuid TEXT NOT NULL REFERENCES plugin_accounts(account_uuid) ON DELETE CASCADE,
  scope TEXT NOT NULL,
  cursor TEXT,
  generation INTEGER NOT NULL CHECK(generation >= 1),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(account_uuid, scope)
);

CREATE TABLE library_sync_bindings (
  library_uuid TEXT PRIMARY KEY REFERENCES libraries(library_uuid) ON DELETE CASCADE,
  plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE RESTRICT,
  account_uuid TEXT NOT NULL REFERENCES plugin_accounts(account_uuid) ON DELETE RESTRICT,
  remote_namespace TEXT,
  binding_epoch BLOB NOT NULL CHECK(length(binding_epoch) = 16),
  actor_epoch BLOB NOT NULL CHECK(length(actor_epoch) = 16),
  state TEXT NOT NULL CHECK(state IN ('preparing','capturing','seed_ready','publishing','active','paused','unbinding','restoring','compatibility_locked','error')),
  transport_generation INTEGER NOT NULL DEFAULT 1 CHECK(transport_generation >= 1),
  cutover_actor_seq INTEGER NOT NULL DEFAULT 0 CHECK(cutover_actor_seq >= 0),
  published_through_seq INTEGER NOT NULL DEFAULT 0 CHECK(published_through_seq >= 0),
  latest_checkpoint_uuid BLOB CHECK(latest_checkpoint_uuid IS NULL OR length(latest_checkpoint_uuid) = 16),
  capture_generation INTEGER CHECK(capture_generation IS NULL OR capture_generation >= 0),
  poll_after INTEGER,
  retry_after INTEGER,
  last_success_at INTEGER,
  last_error_code TEXT,
  incompatibility_reason TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(library_uuid, binding_epoch)
);

CREATE TABLE provider_binding_cursors (
  library_uuid TEXT NOT NULL,
  binding_epoch BLOB NOT NULL CHECK(length(binding_epoch) = 16),
  remote_namespace TEXT NOT NULL,
  scope TEXT NOT NULL,
  cursor TEXT,
  page_token TEXT,
  transport_generation INTEGER NOT NULL CHECK(transport_generation >= 1),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(library_uuid, binding_epoch, scope),
  FOREIGN KEY(library_uuid, binding_epoch) REFERENCES library_sync_bindings(library_uuid, binding_epoch) ON DELETE CASCADE
);

CREATE TABLE provider_object_receipts (
  library_uuid TEXT NOT NULL,
  binding_epoch BLOB NOT NULL CHECK(length(binding_epoch) = 16),
  object_key TEXT NOT NULL,
  etag TEXT,
  sha256 BLOB NOT NULL CHECK(length(sha256) = 32),
  state TEXT NOT NULL CHECK(state IN ('received','durable','created','already-exists-matching','conflict')),
  generation INTEGER NOT NULL CHECK(generation >= 1),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(library_uuid, binding_epoch, object_key),
  FOREIGN KEY(library_uuid, binding_epoch) REFERENCES library_sync_bindings(library_uuid, binding_epoch) ON DELETE CASCADE
);

CREATE TABLE plugin_activation_journal (
  journal_uuid TEXT PRIMARY KEY,
  plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
  account_uuid TEXT,
  action TEXT NOT NULL CHECK(action IN ('start','stop','retry','disable')),
  phase TEXT NOT NULL,
  session_id TEXT,
  error_code TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE plugin_version_journal (
  journal_uuid TEXT PRIMARY KEY,
  plugin_id TEXT NOT NULL REFERENCES plugin_installations(plugin_id) ON DELETE CASCADE,
  from_version TEXT,
  to_version TEXT NOT NULL,
  phase TEXT NOT NULL CHECK(phase IN ('verifying','draining','probation','committed','rolled-back','failed')),
  error_code TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX idx_plugin_versions_state ON plugin_versions(plugin_id, state, version);
CREATE INDEX idx_plugin_runtime_schedule ON plugin_runtime_state(state, backoff_until, plugin_id);
CREATE INDEX idx_plugin_accounts_auth ON plugin_accounts(auth_state, plugin_id);
CREATE INDEX idx_provider_binding_schedule ON library_sync_bindings(state, poll_after, retry_after, library_uuid);
CREATE INDEX idx_provider_receipts_state ON provider_object_receipts(state, updated_at, library_uuid);
CREATE INDEX idx_plugin_activation_journal_phase ON plugin_activation_journal(phase, updated_at);
CREATE INDEX idx_plugin_version_journal_phase ON plugin_version_journal(phase, updated_at);

UPDATE host_meta SET schema_version = 3 WHERE singleton = 1 AND schema_version < 3;

COMMIT;
