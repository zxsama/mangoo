import sqlite3
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MIGRATIONS = [
    ROOT / "crates" / "core" / "migrations" / "0001_initial.sql",
    ROOT / "crates" / "core" / "migrations" / "0002_task_progress.sql",
    ROOT / "crates" / "core" / "migrations" / "0003_browsing_indexes.sql",
    ROOT / "crates" / "core" / "migrations" / "0004_thumbnail_state.sql",
    ROOT / "crates" / "core" / "migrations" / "0005_thumbnail_source_fingerprint.sql",
    ROOT / "crates" / "core" / "migrations" / "0006_thumbnail_task_attempt_fingerprint.sql",
    ROOT / "crates" / "core" / "migrations" / "0007_task_status_contract.sql",
    ROOT / "crates" / "core" / "migrations" / "0008_task_attempt_generation.sql",
    ROOT / "crates" / "core" / "migrations" / "0009_scan_reconciliation.sql",
    ROOT / "crates" / "core" / "migrations" / "0010_media_fts_contentless_delete.sql",
    ROOT / "crates" / "core" / "migrations" / "0011_plugins_extended.sql",
    ROOT / "crates" / "core" / "migrations" / "0012_preview_pipeline_refactor.sql",
    ROOT / "crates" / "core" / "migrations" / "0013_preview_served_by.sql",
    ROOT / "crates" / "core" / "migrations" / "0015_root_status_browsing_indexes.sql",
    ROOT / "crates" / "core" / "migrations" / "0016_folder_status_browsing_indexes.sql",
    ROOT / "crates" / "core" / "migrations" / "0017_thumbnail_task_indexes.sql",
]

TASK_PROGRESS_COLUMNS = {
    "items_seen",
    "items_total",
    "folders_seen",
    "media_files_seen",
    "skipped_files",
    "thumbnail_source_fingerprint",
    "attempt_generation",
}

REQUIRED_TABLES = {
    "schema_migrations",
    "roots",
    "folders",
    "files",
    "media",
    "user_metadata",
    "tags",
    "file_tags",
    "thumbs",
    "tasks",
    "file_operations",
    "plugins",
    "media_fts",
    "thumb_blobs",
}

REQUIRED_INDEXES = {
    "idx_folders_root_parent_name",
    "idx_files_folder_name",
    "idx_files_folder_mtime_id",
    "idx_files_folder_name_id_desc",
    "idx_files_folder_mtime_id_asc",
    "idx_files_folder_status_name_id",
    "idx_files_folder_status_name_id_desc",
    "idx_files_folder_status_mtime_id",
    "idx_files_folder_status_mtime_id_asc",
    "idx_folders_parent_status_id",
    "idx_files_root_mtime_id",
    "idx_files_root_name_id",
    "idx_files_root_name_id_desc",
    "idx_files_root_mtime_id_asc",
    "idx_files_root_status_name_id",
    "idx_files_root_status_name_id_desc",
    "idx_files_root_status_mtime_id",
    "idx_files_root_status_mtime_id_asc",
    "idx_files_global_mtime_id",
    "idx_files_global_mtime_id_asc",
    "idx_files_global_name_id",
    "idx_files_global_name_id_desc",
    "idx_files_ext",
    "idx_media_kind_file",
    "idx_user_metadata_rating",
    "idx_user_metadata_favorite",
    "idx_file_tags_tag_file",
    "idx_thumbs_profile_state",
    "idx_thumbs_state_updated",
    "idx_tasks_status_priority",
    "idx_tasks_kind_file_status",
    "idx_tasks_kind_status_priority",
    "idx_file_operations_status_created",
    "idx_plugins_status",
    "idx_thumb_blobs_profile_updated_at",
}

EXPECTED_MEDIA_COLUMNS = {
    "file_id",
    "kind",
    "width",
    "height",
    "duration_ms",
    "codec",
    "orientation",
    "has_alpha",
    "dominant_color",
    "phash",
    "metadata_status",
}
EXPECTED_MEDIA_COLUMNS |= {"preview_placeholder", "preview_placeholder_format"}

EXPECTED_THUMB_BLOBS_COLUMNS = {
    "file_id",
    "profile",
    "data",
    "width",
    "height",
    "byte_size",
    "output_format",
    "created_at",
    "updated_at",
}

THUMBNAIL_STATUSES = {"pending", "queued", "ready", "failed", "skipped_small"}
TASK_STATUSES = {"pending", "running", "succeeded", "failed", "cancelled"}
TASK_KINDS = {"root_scan", "thumbnail"}
THUMBNAIL_PROFILE = "grid_320"
THUMBNAIL_FORMAT = "image/webp"


def fail(message: str) -> None:
    print(f"FAIL: {message}", file=sys.stderr)
    raise SystemExit(1)


def table_has_column(conn: sqlite3.Connection, table: str, column: str) -> bool:
    return any(row[1] == column for row in conn.execute(f"PRAGMA table_info({table})"))


def apply_migration(conn: sqlite3.Connection, migration: Path) -> None:
    name = migration.name
    if name == "0009_scan_reconciliation.sql":
        if not table_has_column(conn, "roots", "active_scan_generation"):
            conn.executescript(migration.read_text(encoding="utf-8"))
        for table in ("folders", "files"):
            if not table_has_column(conn, table, "scan_seen_at"):
                conn.execute(f"ALTER TABLE {table} ADD COLUMN scan_seen_at INTEGER")
        conn.execute(
            """
            INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
            VALUES (9, 'scan_reconciliation', unixepoch())
            """
        )
        return
    if name == "0012_preview_pipeline_refactor.sql":
        if not table_has_column(conn, "media", "preview_placeholder"):
            conn.execute("ALTER TABLE media ADD COLUMN preview_placeholder BLOB")
        if not table_has_column(conn, "media", "preview_placeholder_format"):
            conn.execute(
                "ALTER TABLE media ADD COLUMN preview_placeholder_format TEXT NOT NULL DEFAULT 'image/webp'"
            )
        conn.executescript(migration.read_text(encoding="utf-8"))
        return
    if name == "0013_preview_served_by.sql":
        if not table_has_column(conn, "thumbs", "served_by"):
            conn.executescript(migration.read_text(encoding="utf-8"))
        else:
            conn.execute(
                """
                INSERT OR IGNORE INTO schema_migrations(version, name, applied_at)
                VALUES (13, 'preview_served_by', unixepoch())
                """
            )
        return
    conn.executescript(migration.read_text(encoding="utf-8"))


def verify_scan_reconciliation_repair_path() -> None:
    conn = sqlite3.connect(":memory:")
    try:
        conn.executescript(
            """
            CREATE TABLE schema_migrations (
              version INTEGER PRIMARY KEY,
              name TEXT NOT NULL,
              applied_at INTEGER NOT NULL
            );
            CREATE TABLE roots (
              id INTEGER PRIMARY KEY,
              active_scan_generation INTEGER
            );
            CREATE TABLE folders (
              id INTEGER PRIMARY KEY
            );
            CREATE TABLE files (
              id INTEGER PRIMARY KEY
            );
            """
        )
        apply_migration(
            conn,
            ROOT / "crates" / "core" / "migrations" / "0009_scan_reconciliation.sql",
        )
        for table in ("folders", "files"):
            if not table_has_column(conn, table, "scan_seen_at"):
                fail(f"0009 repair path did not add {table}.scan_seen_at")
        version = conn.execute(
            "SELECT version FROM schema_migrations WHERE version = 9"
        ).fetchone()
        if version is None:
            fail("0009 repair path did not record migration version 9")
    finally:
        conn.close()


def main() -> None:
    for migration in MIGRATIONS:
        if not migration.exists():
            fail(f"missing migration: {migration}")

    verify_scan_reconciliation_repair_path()

    with tempfile.TemporaryDirectory(prefix="megle_schema_") as temp_dir:
        db_path = Path(temp_dir) / "schema.sqlite"
        conn = sqlite3.connect(db_path)
        try:
            for migration in MIGRATIONS:
                apply_migration(conn, migration)
            conn.execute("PRAGMA foreign_keys = ON")

            tables = {
                row[0]
                for row in conn.execute(
                    "SELECT name FROM sqlite_schema WHERE type IN ('table', 'virtual')"
                )
            }
            missing_tables = REQUIRED_TABLES - tables
            if missing_tables:
                fail(f"missing tables: {sorted(missing_tables)}")

            indexes = {
                row[0]
                for row in conn.execute("SELECT name FROM sqlite_schema WHERE type = 'index'")
                if row[0] is not None
            }
            missing_indexes = REQUIRED_INDEXES - indexes
            if missing_indexes:
                fail(f"missing indexes: {sorted(missing_indexes)}")

            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 1"
            ).fetchone()
            if version is None:
                fail("migration version 1 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 2"
            ).fetchone()
            if version is None:
                fail("migration version 2 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 3"
            ).fetchone()
            if version is None:
                fail("migration version 3 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 4"
            ).fetchone()
            if version is None:
                fail("migration version 4 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 5"
            ).fetchone()
            if version is None:
                fail("migration version 5 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 6"
            ).fetchone()
            if version is None:
                fail("migration version 6 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 7"
            ).fetchone()
            if version is None:
                fail("migration version 7 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 8"
            ).fetchone()
            if version is None:
                fail("migration version 8 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 9"
            ).fetchone()
            if version is None:
                fail("migration version 9 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 10"
            ).fetchone()
            if version is None:
                fail("migration version 10 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 11"
            ).fetchone()
            if version is None:
                fail("migration version 11 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 12"
            ).fetchone()
            if version is None:
                fail("migration version 12 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 13"
            ).fetchone()
            if version is None:
                fail("migration version 13 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 15"
            ).fetchone()
            if version is None:
                fail("migration version 15 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 16"
            ).fetchone()
            if version is None:
                fail("migration version 16 was not recorded")

            media_columns = {
                row[1] for row in conn.execute("PRAGMA table_info(media)").fetchall()
            }
            missing_media_columns = EXPECTED_MEDIA_COLUMNS - media_columns
            if missing_media_columns:
                fail(f"missing media columns: {sorted(missing_media_columns)}")

            thumb_blobs_columns = {
                row[1] for row in conn.execute("PRAGMA table_info(thumb_blobs)").fetchall()
            }
            missing_thumb_blobs_columns = EXPECTED_THUMB_BLOBS_COLUMNS - thumb_blobs_columns
            if missing_thumb_blobs_columns:
                fail(f"missing thumb blob columns: {sorted(missing_thumb_blobs_columns)}")

            task_columns = {
                row[1] for row in conn.execute("PRAGMA table_info(tasks)").fetchall()
            }
            missing_task_columns = TASK_PROGRESS_COLUMNS - task_columns
            if missing_task_columns:
                fail(f"missing task progress columns: {sorted(missing_task_columns)}")

            thumb_columns = {
                row[1] for row in conn.execute("PRAGMA table_info(thumbs)").fetchall()
            }
            required_thumb_columns = {
                "file_id",
                "profile",
                "state",
                "cache_key",
                "width",
                "height",
                "byte_size",
                "short_side_px",
                "output_format",
                "source_fingerprint",
                "served_by",
                "error",
                "updated_at",
            }
            missing_thumb_columns = required_thumb_columns - thumb_columns
            if missing_thumb_columns:
                fail(f"missing thumbnail columns: {sorted(missing_thumb_columns)}")

            plugin_columns = {
                row[1] for row in conn.execute("PRAGMA table_info(plugins)").fetchall()
            }
            required_plugin_columns = {
                "id",
                "name",
                "version",
                "description",
                "enabled",
                "status",
                "capabilities_json",
                "permissions_json",
                "manifest_path",
                "installed_at",
                "updated_at",
                "last_error",
            }
            missing_plugin_columns = required_plugin_columns - plugin_columns
            if missing_plugin_columns:
                fail(f"missing plugin columns: {sorted(missing_plugin_columns)}")

            conn.execute(
                """
                INSERT INTO roots(path, display_name, enabled, created_at)
                VALUES (?, ?, 1, 1)
                """,
                ("D:/Pictures", "Pictures"),
            )
            root_id = conn.execute("SELECT id FROM roots").fetchone()[0]
            conn.execute(
                """
                INSERT INTO folders(root_id, parent_id, name, path_hash, mtime)
                VALUES (?, NULL, ?, ?, 1)
                """,
                (root_id, "", "root-hash"),
            )
            folder_id = conn.execute("SELECT id FROM folders").fetchone()[0]
            conn.execute(
                """
                INSERT INTO files(root_id, folder_id, name, ext, size, mtime)
                VALUES (?, ?, ?, ?, ?, ?)
                """,
                (root_id, folder_id, "image.jpg", ".jpg", 1000, 10),
            )
            file_id = conn.execute("SELECT id FROM files").fetchone()[0]
            conn.execute(
                """
                INSERT INTO media(file_id, kind, width, height, metadata_status)
                VALUES (?, 'image', 640, 480, 'ready')
                """,
                (file_id,),
            )
            conn.execute(
                """
                INSERT INTO thumbs(
                    file_id, profile, state, cache_key, width, height, byte_size,
                    short_side_px, output_format, served_by, updated_at
                )
                VALUES (?, ?, 'ready', NULL, 427, 320, 4096, 320, ?, 'db_blob', 11)
                """,
                (file_id, THUMBNAIL_PROFILE, THUMBNAIL_FORMAT),
            )
            invalid_served_by = conn.execute(
                """
                UPDATE OR IGNORE thumbs
                SET served_by = 'thumbnail-cache'
                WHERE file_id = ? AND profile = ?
                """,
                (file_id, THUMBNAIL_PROFILE),
            ).rowcount
            if invalid_served_by != 0:
                fail("thumbnail served_by must reject disk-cache serving sources")
            for status in THUMBNAIL_STATUSES - {"ready"}:
                conn.execute(
                    """
                    UPDATE thumbs
                    SET state = ?, cache_key = NULL, width = NULL, height = NULL,
                        byte_size = NULL, served_by = NULL,
                        error = CASE WHEN ? = 'failed' THEN 'decode failed' ELSE NULL END
                    WHERE file_id = ? AND profile = ?
                    """,
                    (status, status, file_id, THUMBNAIL_PROFILE),
                )
            invalid_status = conn.execute(
                """
                UPDATE OR IGNORE thumbs
                SET state = 'unknown'
                WHERE file_id = ? AND profile = ?
                """,
                (file_id, THUMBNAIL_PROFILE),
            ).rowcount
            if invalid_status != 0:
                fail("thumbnail state must reject unsupported status values")
            invalid_profile = conn.execute(
                """
                INSERT OR IGNORE INTO thumbs(file_id, profile, state, short_side_px, output_format, updated_at)
                VALUES (?, 'grid', 'pending', 320, ?, 12)
                """,
                (file_id, THUMBNAIL_FORMAT),
            ).rowcount
            if invalid_profile != 0:
                fail("thumbnail profile must reject unsupported profile values")
            invalid_format = conn.execute(
                """
                UPDATE OR IGNORE thumbs
                SET output_format = 'image/jpeg'
                WHERE file_id = ? AND profile = ?
                """,
                (file_id, THUMBNAIL_PROFILE),
            ).rowcount
            if invalid_format != 0:
                fail("thumbnail output format must reject unsupported values")
            conn.execute(
                """
                INSERT INTO tasks(kind, priority, status, root_id, created_at, updated_at)
                VALUES ('root_scan', 0, 'pending', ?, 1, 1)
                """,
                (root_id,),
            )
            task_id = conn.execute("SELECT id FROM tasks").fetchone()[0]
            for status in TASK_STATUSES - {"pending"}:
                conn.execute(
                    "UPDATE tasks SET status = ? WHERE id = ?",
                    (status, task_id),
                )
            invalid_task_status = conn.execute(
                "UPDATE OR IGNORE tasks SET status = 'paused' WHERE id = ?",
                (task_id,),
            ).rowcount
            if invalid_task_status != 0:
                fail("task status must reject unsupported status values")
            invalid_task_kind = conn.execute(
                """
                INSERT OR IGNORE INTO tasks(kind, priority, status, created_at, updated_at)
                VALUES ('watcher_scan', 0, 'pending', 1, 1)
                """
            ).rowcount
            if invalid_task_kind != 0:
                fail("task kind must reject unsupported kind values")
            page = conn.execute(
                """
                SELECT files.id, files.name
                FROM files
                JOIN media ON media.file_id = files.id
                WHERE files.folder_id = ? AND files.status = 'active'
                ORDER BY files.mtime DESC, files.id DESC
                LIMIT 200
                """,
                (folder_id,),
            ).fetchall()
            if page != [(file_id, "image.jpg")]:
                fail("paged media smoke query returned unexpected rows")
        finally:
            conn.close()

    print("PASS: sqlite schema")


if __name__ == "__main__":
    main()
