-- Validated immutable descriptor candidates retain causal heads and exact identity.
BEGIN IMMEDIATE;
ALTER TABLE provider_remote_libraries ADD COLUMN descriptor_key TEXT NOT NULL DEFAULT 'library-descriptor';
ALTER TABLE provider_remote_libraries ADD COLUMN checkpoint_key TEXT;
ALTER TABLE provider_remote_libraries ADD COLUMN checkpoint_sha256 TEXT;
CREATE TABLE provider_remote_descriptor_heads (
  account_uuid TEXT NOT NULL REFERENCES plugin_accounts(account_uuid) ON DELETE CASCADE,
  library_uuid TEXT NOT NULL,
  descriptor_key TEXT NOT NULL,
  account_generation INTEGER NOT NULL CHECK(account_generation >= 1),
  candidate_json TEXT NOT NULL CHECK(json_valid(candidate_json)),
  PRIMARY KEY(account_uuid,library_uuid,descriptor_key)
);
CREATE TABLE provider_verified_descriptors (
  account_uuid TEXT NOT NULL REFERENCES plugin_accounts(account_uuid) ON DELETE CASCADE,
  library_uuid TEXT NOT NULL,
  descriptor_key TEXT NOT NULL,
  account_generation INTEGER NOT NULL CHECK(account_generation >= 1),
  candidate_json TEXT NOT NULL CHECK(json_valid(candidate_json)),
  PRIMARY KEY(account_uuid,library_uuid,descriptor_key)
);
ALTER TABLE sync_restore_plans ADD COLUMN descriptor_key TEXT NOT NULL DEFAULT 'library-descriptor';
ALTER TABLE sync_restore_plans ADD COLUMN checkpoint_key TEXT;
ALTER TABLE sync_restore_plans ADD COLUMN checkpoint_sha256 TEXT;
CREATE TRIGGER sync_restore_pin_descriptor AFTER INSERT ON sync_restore_plans
BEGIN
  UPDATE sync_restore_plans SET
    descriptor_key=COALESCE((SELECT descriptor_key FROM provider_remote_libraries WHERE account_uuid=NEW.account_uuid AND library_uuid=NEW.library_uuid AND descriptor_sha256=NEW.descriptor_sha256),'library-descriptor'),
    checkpoint_key=(SELECT checkpoint_key FROM provider_remote_libraries WHERE account_uuid=NEW.account_uuid AND library_uuid=NEW.library_uuid AND descriptor_sha256=NEW.descriptor_sha256),
    checkpoint_sha256=(SELECT checkpoint_sha256 FROM provider_remote_libraries WHERE account_uuid=NEW.account_uuid AND library_uuid=NEW.library_uuid AND descriptor_sha256=NEW.descriptor_sha256)
  WHERE restore_uuid=NEW.restore_uuid;
END;
UPDATE host_meta SET schema_version=18 WHERE singleton=1 AND schema_version<18;
COMMIT;
