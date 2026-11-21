-- Existing FTS rows remain searchable. New edits can index any number of
-- overlapping parts without putting a whole source into one SQLite value.
BEGIN IMMEDIATE;
CREATE TABLE notebook_search_parts (
  id INTEGER PRIMARY KEY,
  record_id INTEGER NOT NULL REFERENCES plugin_records(id) ON DELETE CASCADE
);
CREATE INDEX idx_notebook_search_parts_record ON notebook_search_parts(record_id);
INSERT INTO notebook_search_parts(id, record_id) SELECT rowid, rowid FROM notebook_fts;
INSERT INTO schema_migrations(version, name, applied_at) VALUES(42, 'notebook_search_parts', unixepoch());
COMMIT;
