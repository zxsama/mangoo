PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS host_meta (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  schema_version INTEGER NOT NULL,
  active_library_uuid TEXT,
  context_revision INTEGER NOT NULL DEFAULT 0,
  activation_library_uuid TEXT,
  activation_revision INTEGER
);

INSERT OR IGNORE INTO host_meta(singleton, schema_version, context_revision)
VALUES (1, 1, 0);

CREATE TABLE IF NOT EXISTS libraries (
  library_uuid TEXT PRIMARY KEY,
  root_uuid TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  icon TEXT,
  color TEXT,
  local_data_dir TEXT NOT NULL UNIQUE,
  managed_root TEXT NOT NULL UNIQUE COLLATE NOCASE,
  lifecycle_state TEXT NOT NULL,
  binding_state TEXT NOT NULL CHECK (binding_state = 'local-only'),
  availability TEXT NOT NULL,
  marker_version INTEGER NOT NULL,
  marker_identity TEXT NOT NULL,
  origin_remote_identity TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  last_opened_at INTEGER
);

CREATE TABLE IF NOT EXISTS library_creation_journal (
  txn_uuid TEXT PRIMARY KEY,
  library_uuid TEXT NOT NULL,
  root_uuid TEXT NOT NULL,
  name TEXT NOT NULL,
  icon TEXT,
  color TEXT,
  requested_root TEXT NOT NULL,
  resolved_root TEXT NOT NULL,
  local_staging_dir TEXT NOT NULL,
  phase TEXT NOT NULL,
  created_root INTEGER NOT NULL DEFAULT 0,
  created_marker INTEGER NOT NULL DEFAULT 0,
  root_identity TEXT,
  staging_identity TEXT,
  marker_identity TEXT,
  error_code TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS library_removal_journal (
  txn_uuid TEXT PRIMARY KEY,
  library_uuid TEXT NOT NULL,
  managed_root TEXT NOT NULL,
  local_data_dir TEXT NOT NULL,
  root_quarantine TEXT,
  local_quarantine TEXT,
  root_quarantine_identity TEXT,
  local_quarantine_identity TEXT,
  marker_identity TEXT NOT NULL,
  phase TEXT NOT NULL,
  error_code TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS library_activation_journal (
  txn_uuid TEXT PRIMARY KEY,
  from_library_uuid TEXT,
  to_library_uuid TEXT,
  target_revision INTEGER NOT NULL,
  phase TEXT NOT NULL,
  error_code TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_libraries_lifecycle ON libraries(lifecycle_state, availability);
CREATE INDEX IF NOT EXISTS idx_creation_journal_phase ON library_creation_journal(phase);
CREATE INDEX IF NOT EXISTS idx_removal_journal_phase ON library_removal_journal(phase);
CREATE INDEX IF NOT EXISTS idx_activation_journal_phase ON library_activation_journal(phase);
