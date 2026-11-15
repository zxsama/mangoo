-- Plugin records shared across every Library of this Host (the "shared"
-- plugin-data scope). Each field value is stored wrapped as
-- {"v": value, "t": edit-ms, "d": device} and merged last-writer-wins by
-- (t, d), so the store can be reconciled with the mirror collection that
-- every activated Library carries (`mirror--<collection>` in plugin_records)
-- and therefore synchronizes through any Library's ordinary sync stream.
-- Deletion is the merged field "deleted", never a tombstone, so it converges
-- across every mirror.
BEGIN IMMEDIATE;

CREATE TABLE host_plugin_records (
  id INTEGER PRIMARY KEY,
  record_uuid BLOB NOT NULL UNIQUE CHECK(length(record_uuid) = 16),
  plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 255),
  collection TEXT NOT NULL CHECK(length(collection) BETWEEN 1 AND 24),
  fields_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(fields_json) AND json_type(fields_json) = 'object'),
  revision INTEGER NOT NULL CHECK(revision >= 1),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX idx_host_plugin_records_owner
  ON host_plugin_records(plugin_id, collection, record_uuid);

CREATE TABLE host_plugin_record_revisions (
  plugin_id TEXT PRIMARY KEY CHECK(length(plugin_id) BETWEEN 1 AND 255),
  revision INTEGER NOT NULL CHECK(revision >= 0)
);

-- Last reconciled (Library revision, Host revision) per plugin and Library:
-- an unchanged pair means the mirror and the shared store already agree.
CREATE TABLE host_plugin_mirror_state (
  plugin_id TEXT NOT NULL,
  library_uuid TEXT NOT NULL,
  library_revision INTEGER NOT NULL,
  host_revision INTEGER NOT NULL,
  PRIMARY KEY(plugin_id, library_uuid)
);

UPDATE host_meta SET schema_version = 23 WHERE singleton = 1;
COMMIT;
