-- A derived per-Library search index. Source text is recoverable from the
-- synchronized document head and immutable chunks; the index is device-local.
BEGIN IMMEDIATE;
CREATE VIRTUAL TABLE notebook_fts USING fts5(
  search_text,
  tokenize = 'trigram'
);
CREATE TABLE notebook_search_pending (
  record_id INTEGER PRIMARY KEY REFERENCES plugin_records(id) ON DELETE CASCADE
);
INSERT INTO notebook_search_pending(record_id)
SELECT id FROM plugin_records WHERE plugin_id='com.mangoo.notebook' AND collection='documents' AND deleted=0;
INSERT INTO schema_migrations(version, name, applied_at) VALUES(41, 'notebook_search', unixepoch());
COMMIT;
