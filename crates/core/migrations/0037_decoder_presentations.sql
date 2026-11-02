BEGIN IMMEDIATE;
-- Metadata stays separate from large BLOBs, whose data column must remain last.
CREATE TABLE decoder_presentations (
  file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  profile TEXT NOT NULL CHECK(profile IN ('grid_320','preview_2048','preview_3072','preview_native')),
  decoder_key TEXT NOT NULL,
  source_fingerprint TEXT NOT NULL,
  PRIMARY KEY(file_id,profile)
);
INSERT INTO schema_migrations(version,name,applied_at) VALUES(37,'decoder_presentations',unixepoch());
COMMIT;
