-- Generic plugin-owned records stored in the Library Catalog.
--
-- Every row is the browse projection of one synchronized entity: the header
-- field `plugin-record.v1` (owner plugin + collection) plus one LWW field
-- `plugin-field.v1:<name>` per top-level field, with Delete/Restore tombstones.
-- Remote apply and checkpoint replay rebuild a row from that canonical truth
-- (`crate::db::plugin_records::rebuild_plugin_record`), so the table is a
-- derived projection exactly like `tags` or `user_metadata`.
--
-- `sync_captured` records whether the row's current content already exists as
-- canonical events. Rows written while capture is disabled are backfilled when
-- a sync binding enables capture.
BEGIN IMMEDIATE;

CREATE TABLE plugin_records (
  id INTEGER PRIMARY KEY,
  record_uuid BLOB NOT NULL UNIQUE CHECK(length(record_uuid) = 16),
  plugin_id TEXT NOT NULL CHECK(length(plugin_id) BETWEEN 1 AND 255),
  collection TEXT NOT NULL CHECK(length(collection) BETWEEN 1 AND 32),
  fields_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(fields_json) AND json_type(fields_json) = 'object'),
  deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0, 1)),
  revision INTEGER NOT NULL CHECK(revision >= 1),
  sync_captured INTEGER NOT NULL DEFAULT 0 CHECK(sync_captured IN (0, 1)),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

-- Keyset listing by owner + collection (record_uuid is UUIDv7: time ordered).
CREATE INDEX idx_plugin_records_owner_collection
  ON plugin_records(plugin_id, collection, deleted, record_uuid);
-- Change detection and incremental reads by revision.
CREATE INDEX idx_plugin_records_owner_revision
  ON plugin_records(plugin_id, revision);
-- Capture backfill scans only uncaptured rows.
CREATE INDEX idx_plugin_records_uncaptured
  ON plugin_records(sync_captured) WHERE sync_captured = 0;

-- Monotonic per-plugin change counter. Bumped by every local write and every
-- projection rebuild, so a reader can detect any change with one row read.
CREATE TABLE plugin_record_revisions (
  plugin_id TEXT PRIMARY KEY CHECK(length(plugin_id) BETWEEN 1 AND 255),
  revision INTEGER NOT NULL CHECK(revision >= 0)
);

INSERT INTO schema_migrations(version, name, applied_at) VALUES(40, 'plugin_records', unixepoch());
COMMIT;
