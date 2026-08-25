BEGIN IMMEDIATE;
CREATE TABLE plugin_secret_handles (
  plugin_id TEXT NOT NULL,
  account_uuid TEXT NOT NULL,
  secret_ref TEXT NOT NULL,
  namespace TEXT NOT NULL,
  purpose TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY(plugin_id, account_uuid, secret_ref)
);
CREATE INDEX idx_plugin_secret_handles_account ON plugin_secret_handles(plugin_id, account_uuid, created_at);
UPDATE host_meta SET schema_version = 10 WHERE singleton = 1;
COMMIT;
