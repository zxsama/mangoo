-- A hidden registration remains the owner of its identity, media root and catalog.
-- This is deliberately independent of the destructive removal journal.
BEGIN IMMEDIATE;
CREATE TABLE library_list_visibility (
  library_uuid TEXT PRIMARY KEY REFERENCES libraries(library_uuid) ON DELETE CASCADE,
  hidden INTEGER NOT NULL CHECK (hidden IN (0, 1))
);

-- Opening never owns the selected root. Only the newly created local data
-- directory belongs to this intent; recovery must never clean up the root.
CREATE TABLE library_open_journal (
  library_uuid TEXT PRIMARY KEY,
  root_uuid TEXT NOT NULL,
  name TEXT NOT NULL,
  managed_root TEXT NOT NULL,
  local_data_dir TEXT NOT NULL,
  marker_identity TEXT NOT NULL,
  root_identity TEXT NOT NULL,
  local_identity TEXT,
  catalog_identity TEXT,
  phase TEXT NOT NULL CHECK (phase IN ('requested', 'building', 'committed')),
  created_at INTEGER NOT NULL
);

UPDATE host_meta SET schema_version = 21 WHERE singleton = 1;
COMMIT;
