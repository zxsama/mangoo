-- Hard-delete a folder (root): a non-NULL `pending_delete_at` marks a root that
-- is being purged in the background. It is the authoritative "being removed"
-- state (replaces the enabled=0 soft delete) and drives startup recovery.
ALTER TABLE roots ADD COLUMN pending_delete_at INTEGER;

-- Partial index so startup recovery scans only the (usually zero) roots being
-- purged, not the whole roots table.
CREATE INDEX IF NOT EXISTS idx_roots_pending_delete
  ON roots(id) WHERE pending_delete_at IS NOT NULL;

INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
VALUES (19, 'root_pending_delete', unixepoch());
