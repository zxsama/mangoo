-- Plugin lifecycle moved to the Host Registry in Host migration 0003.
-- There is deliberately no compatibility or data migration for development
-- Catalog plugin rows: retaining this table would create a writable second
-- truth that changes with the active Library.
BEGIN IMMEDIATE;

DROP INDEX IF EXISTS idx_plugins_status;
DROP TABLE IF EXISTS plugins;

INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
VALUES (28, 'remove_catalog_plugins', unixepoch());

COMMIT;
