BEGIN IMMEDIATE;

CREATE TABLE plugin_credential_retirements (
  retirement_uuid TEXT PRIMARY KEY,
  plugin_id TEXT NOT NULL,
  account_uuid TEXT NOT NULL,
  secret_ref TEXT NOT NULL,
  namespace TEXT NOT NULL,
  purpose TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
  next_retry_at INTEGER NOT NULL,
  last_error_code TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(plugin_id, account_uuid, secret_ref)
);

CREATE INDEX idx_plugin_credential_retirements_due
  ON plugin_credential_retirements(next_retry_at, created_at);

CREATE TABLE plugin_auth_flow_ownership (
  plugin_id TEXT PRIMARY KEY,
  flow_id TEXT NOT NULL,
  supervisor_session TEXT NOT NULL,
  prior_auth_state TEXT NOT NULL,
  prior_display_name TEXT,
  prior_account_kind TEXT,
  created_at INTEGER NOT NULL
);

UPDATE host_meta SET schema_version = 8 WHERE singleton = 1;
COMMIT;
