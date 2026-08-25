BEGIN IMMEDIATE;

ALTER TABLE plugin_accounts ADD COLUMN account_kind TEXT
  CHECK (account_kind IS NULL OR account_kind IN ('personal', 'business'));

CREATE TABLE onedrive_configuration (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  host_override_client_id TEXT,
  revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
  updated_at INTEGER NOT NULL
);

CREATE TABLE onedrive_configuration_plans (
  plan_uuid TEXT PRIMARY KEY,
  expected_revision INTEGER NOT NULL,
  requested_override TEXT,
  effective_changed INTEGER NOT NULL CHECK (effective_changed IN (0, 1)),
  affected_libraries INTEGER NOT NULL,
  requires_confirmation INTEGER NOT NULL CHECK (requires_confirmation IN (0, 1)),
  expires_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);

INSERT INTO onedrive_configuration(singleton, host_override_client_id, revision, updated_at)
VALUES (1, NULL, 1, 0);

UPDATE host_meta SET schema_version = 7 WHERE singleton = 1;
COMMIT;
