BEGIN IMMEDIATE;
ALTER TABLE folders ADD COLUMN icon TEXT;
ALTER TABLE folders ADD COLUMN color TEXT;
INSERT INTO schema_migrations(version, name, applied_at) VALUES(38, 'folder_appearance', unixepoch());
COMMIT;
