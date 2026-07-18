-- Distinguish transient thumbnail failures (network/lock/memory — retryable
-- once the source is reachable) from permanent ones (unsupported/corrupt).
-- Nullable; only meaningful when thumbs.state = 'failed'.
ALTER TABLE thumbs ADD COLUMN failure_kind TEXT;

INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
VALUES (18, 'thumb_failure_kind', unixepoch());
