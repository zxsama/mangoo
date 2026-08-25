BEGIN IMMEDIATE;

ALTER TABLE plugin_auth_flow_ownership
  ADD COLUMN state TEXT NOT NULL DEFAULT 'active'
  CHECK(state IN ('active', 'completing', 'cancelling'));

ALTER TABLE plugin_auth_flow_ownership
  ADD COLUMN prior_secret_ref TEXT;

ALTER TABLE plugin_auth_flow_ownership
  ADD COLUMN prior_stable_account_identity TEXT;

UPDATE host_meta SET schema_version = 9 WHERE singleton = 1;
COMMIT;
