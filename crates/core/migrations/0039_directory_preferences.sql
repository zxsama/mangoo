BEGIN IMMEDIATE;
ALTER TABLE roots ADD COLUMN icon TEXT;
ALTER TABLE roots ADD COLUMN color TEXT;
ALTER TABLE roots ADD COLUMN directory_sort_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE roots ADD COLUMN directory_sort_subtree_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE folders ADD COLUMN directory_sort_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE folders ADD COLUMN directory_sort_subtree_revision INTEGER NOT NULL DEFAULT 0;
CREATE TABLE directory_sort_rules (
    id INTEGER PRIMARY KEY,
    root_id INTEGER REFERENCES roots(id) ON DELETE CASCADE,
    folder_id INTEGER REFERENCES folders(id) ON DELETE CASCADE,
    sort_order TEXT NOT NULL CHECK(sort_order IN ('name_asc', 'name_desc')),
    recursive INTEGER NOT NULL CHECK(recursive IN (0, 1)),
    CHECK((root_id IS NOT NULL) != (folder_id IS NOT NULL)),
    UNIQUE(root_id),
    UNIQUE(folder_id)
);
CREATE INDEX idx_folders_parent_status_name_id ON folders(parent_id, status, name, id);
INSERT INTO schema_migrations(version, name, applied_at) VALUES(39, 'directory_preferences', unixepoch());
COMMIT;
