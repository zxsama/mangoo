-- Adaptive downscaled preview tiers, cached as WebP BLOBs.
--
-- Deliberately NOT a new profile inside `thumb_blobs`: that table's
-- `CHECK(profile = 'grid_320')` cannot be altered in place, and on a real
-- library it holds ~663k rows / ~18 GB, so widening it would mean copying 18 GB
-- during app open. Preview tiers also want their own lifecycle — large,
-- regenerable, LRU-evicted — while grid thumbs are small, permanent, and owned
-- by the `thumbs` state machine.
--
-- `source_size` / `source_mtime` are the staleness key: the preview route
-- already stats the file, so validating a cached tier costs two integer
-- comparisons and no extra query.
BEGIN IMMEDIATE;

CREATE TABLE IF NOT EXISTS preview_blobs (
  file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  profile TEXT NOT NULL CHECK(profile IN ('preview_2048','preview_3072','preview_native')),
  data BLOB NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  byte_size INTEGER NOT NULL,
  output_format TEXT NOT NULL DEFAULT 'image/webp' CHECK(output_format = 'image/webp'),
  source_size INTEGER NOT NULL,
  source_mtime INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(file_id, profile)
);

-- Supports "evict least recently used until under budget" as one indexed scan.
CREATE INDEX IF NOT EXISTS idx_preview_blobs_updated_at
ON preview_blobs(updated_at);

INSERT INTO schema_migrations(version, name, applied_at)
VALUES (25, 'preview_blobs', unixepoch());

COMMIT;
