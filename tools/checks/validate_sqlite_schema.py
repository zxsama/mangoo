import sqlite3
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
HOST_MIGRATIONS = [
    ROOT / "crates" / "core" / "host_migrations" / "0001_host_registry.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0002_sync_protocol_foundation.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0003_plugin_runtime.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0004_provider_root_discovery.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0005_sync_orchestration.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0006_sync_orchestration_recovery.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0007_onedrive_auth_configuration.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0008_plugin_credential_retirement.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0009_plugin_auth_cancellation.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0010_plugin_secret_handles.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0011_plugin_secret_creations.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0012_sync_root_bindings.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0013_sync_root_reenable_generation.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0014_sync_root_preflights.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0015_sync_root_recovery.sql",
    ROOT / "crates" / "core" / "host_migrations" / "0016_remote_root_inventory.sql",
]
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
    ROOT / "crates" / "core" / "migrations" / "0018_thumb_failure_kind.sql",
    ROOT / "crates" / "core" / "migrations" / "0019_root_pending_delete.sql",
    ROOT / "crates" / "core" / "migrations" / "0020_root_purge_task_kind.sql",
    ROOT / "crates" / "core" / "migrations" / "0021_notification_outbox.sql",
    ROOT / "crates" / "core" / "migrations" / "0022_file_operation_journal.sql",
    ROOT / "crates" / "core" / "migrations" / "0023_file_operation_recovery_metadata.sql",
    ROOT / "crates" / "core" / "migrations" / "0024_file_operation_object_binding.sql",
    ROOT / "crates" / "core" / "migrations" / "0025_preview_blobs.sql",
    ROOT / "crates" / "core" / "migrations" / "0026_preview_failures.sql",
    ROOT / "crates" / "core" / "migrations" / "0027_sync_protocol_foundation.sql",
    ROOT / "crates" / "core" / "migrations" / "0028_remove_catalog_plugins.sql",
    ROOT / "crates" / "core" / "migrations" / "0029_media_object_storage.sql",
    ROOT / "crates" / "core" / "migrations" / "0030_sync_root_identity.sql",
    ROOT / "crates" / "core" / "migrations" / "0031_sync_root_generation_fence.sql",
    ROOT / "crates" / "core" / "migrations" / "0032_sync_root_reconciliation.sql",
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
    "media_fts",
    "thumb_blobs",
    "preview_blobs",
    "preview_failures",
    "notification_operation_heads",
    "notification_outbox",
    "sync_local_state",
    "sync_actor_epochs",
    "sync_actor_frontiers",
    "sync_events",
    "sync_outbox",
    "sync_segments",
    "sync_inbox_segments",
    "sync_field_versions",
    "sync_set_adds",
    "sync_set_removals",
    "sync_location_revisions",
    "sync_location_heads",
    "sync_location_supersedes",
    "sync_tombstone_tokens",
    "sync_tombstone_removals",
    "asset_content_revisions",
    "asset_content_heads",
    "asset_content_supersedes",
    "sync_conflicts",
    "sync_publication_intents",
    "sync_checkpoints",
    "sync_checkpoint_watermarks",
    "sync_bulk_operations",
    "sync_bulk_items",
    "sync_local_file_identities",
    "sync_identity_conflicts",
    "media_chunks",
    "media_packs",
    "media_manifests",
    "media_content_manifest_links",
    "media_publication_jobs",
    "media_publication_objects",
    "media_materialization_jobs",
    "media_verified_ranges",
    "media_remote_objects",
    "media_reachability",
    "sync_root_descriptors",
    "sync_root_local_bindings",
    "sync_root_reconciliation_applied",
    "sync_root_publish_cutovers",
}

REQUIRED_HOST_SYNC_ROOT_TABLES = {
    "library_local_settings",
    "remote_sync_roots",
    "local_root_bindings",
    "root_operations",
    "root_cutover_snapshots",
    "root_comparison_plans",
    "root_comparison_items",
    "root_comparison_resolutions",
    "root_relocation_operations",
    "root_stop_plans",
    "root_visibility_reconciliations",
    "root_comparison_snapshots",
    "root_reconciliation_work",
    "root_publish_as_new",
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
    "idx_thumb_blobs_profile_updated_at",
    "idx_preview_blobs_updated_at",
    "idx_roots_pending_delete",
    "idx_notification_heads_status_updated",
    "idx_file_operations_group",
    "idx_file_operations_pending",
    "ux_roots_root_uuid",
    "ux_folders_folder_uuid",
    "ux_files_asset_uuid",
    "ux_tags_tag_uuid",
    "idx_sync_outbox_pending",
    "idx_sync_inbox_ready",
    "idx_sync_publication_retry",
    "idx_sync_checkpoint_watermark_actor",
    "idx_sync_bulk_items_pending",
    "idx_media_chunks_pack",
    "idx_media_manifests_content",
    "idx_media_publication_work",
    "idx_media_materialization_work",
    "idx_media_reachability_revision",
    "ux_roots_root_folder_uuid",
    "ux_folders_root_empty_sentinel",
    "idx_sync_root_descriptors_library_lifecycle",
    "idx_sync_root_descriptors_updated",
    "idx_sync_root_local_bindings_library_state",
    "idx_sync_root_local_bindings_generation",
    "idx_tasks_root_binding_work",
    "idx_file_operations_root_binding_work",
    "idx_sync_publication_root_binding_work",
    "idx_media_publication_root_binding_work",
    "idx_media_materialization_root_binding_work",
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

EXPECTED_PREVIEW_BLOBS_COLUMNS = {
    "file_id",
    "profile",
    "data",
    "width",
    "height",
    "byte_size",
    "output_format",
    "source_size",
    "source_mtime",
    "created_at",
    "updated_at",
}

EXPECTED_PREVIEW_FAILURES_COLUMNS = {
    "file_id",
    "profile",
    "failure_kind",
    "error",
    "source_size",
    "source_mtime",
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


def index_key_columns(conn: sqlite3.Connection, index_name: str):
    return [
        (row[2], bool(row[3]))
        for row in conn.execute(f"PRAGMA index_xinfo('{index_name}')")
        if row[5] == 1
    ]


def apply_migration(conn: sqlite3.Connection, migration: Path) -> None:
    name = migration.name
    if name in {"0027_sync_protocol_foundation.sql", "0029_media_object_storage.sql"}:
        version = 27 if name.startswith("0027") else 29
        already_applied = conn.execute(
            "SELECT 1 FROM schema_migrations WHERE version = ?", (version,)
        ).fetchone()
        if already_applied is None:
            conn.executescript(migration.read_text(encoding="utf-8"))
        return
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
    if name == "0020_root_purge_task_kind.sql":
        # This migration's Rust counterpart always runs after the in-code
        # `interactive_folder_scan_task_contract` migration (version 14, which
        # adds `tasks.folder_id` and is never mirrored here as a standalone
        # file), so by the time it runs on a real database `folder_id` already
        # exists. This standalone schema check applies the numbered .sql files
        # only, so backfill that prerequisite column first, same defensive
        # shape as the 0009 special case above.
        if not table_has_column(conn, "tasks", "folder_id"):
            conn.execute(
                "ALTER TABLE tasks ADD COLUMN folder_id INTEGER REFERENCES folders(id) ON DELETE SET NULL"
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


def expect_integrity_error(conn: sqlite3.Connection, sql: str, params=()) -> None:
    try:
        conn.execute(sql, params)
    except sqlite3.IntegrityError:
        return
    fail(f"expected integrity error: {sql.strip()}")


def verify_sync_upgrade_from_populated_v26() -> None:
    conn = sqlite3.connect(":memory:")
    try:
        for migration in MIGRATIONS:
            if migration.name == "0027_sync_protocol_foundation.sql":
                break
            apply_migration(conn, migration)
        conn.execute("PRAGMA foreign_keys = ON")
        conn.execute(
            "INSERT INTO roots(path, display_name, enabled, created_at) VALUES ('D:/Seed', 'Seed', 1, 1)"
        )
        root_id = conn.execute("SELECT id FROM roots").fetchone()[0]
        conn.execute(
            "INSERT INTO folders(root_id,parent_id,name,path_hash,mtime) VALUES (?,NULL,'','seed-root',1)",
            (root_id,),
        )
        folder_id = conn.execute("SELECT id FROM folders").fetchone()[0]
        conn.execute(
            "INSERT INTO files(root_id,folder_id,name,ext,size,mtime) VALUES (?,?,'seed.jpg','.jpg',1,1)",
            (root_id, folder_id),
        )
        conn.execute("INSERT INTO tags(name,color) VALUES ('seed-tag','#fff')")
        apply_migration(
            conn,
            ROOT / "crates" / "core" / "migrations" / "0027_sync_protocol_foundation.sql",
        )
        for table, column in (
            ("roots", "root_uuid"),
            ("folders", "folder_uuid"),
            ("files", "asset_uuid"),
            ("tags", "tag_uuid"),
        ):
            value = conn.execute(f"SELECT {column} FROM {table}").fetchone()[0]
            if not isinstance(value, bytes) or len(value) != 16:
                fail(f"v26 upgrade did not backfill {table}.{column}")
        # A second application must be an idempotent no-op through the same
        # schema_migrations gate used by Database::apply_migrations.
        apply_migration(
            conn,
            ROOT / "crates" / "core" / "migrations" / "0027_sync_protocol_foundation.sql",
        )
        count = conn.execute(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 27"
        ).fetchone()[0]
        if count != 1:
            fail("migration 27 rerun did not preserve one registration row")
    finally:
        conn.close()


def verify_media_upgrade_from_populated_v28() -> None:
    conn = sqlite3.connect(":memory:")
    try:
        for migration in MIGRATIONS:
            if migration.name == "0029_media_object_storage.sql":
                break
            apply_migration(conn, migration)
        conn.execute("PRAGMA foreign_keys = ON")
        conn.execute(
            "INSERT INTO roots(path,display_name,enabled,created_at) VALUES ('D:/Media','Media',1,1)"
        )
        root_id = conn.execute("SELECT id FROM roots").fetchone()[0]
        conn.execute(
            "INSERT INTO folders(root_id,parent_id,name,path_hash,mtime,folder_uuid) VALUES (?,NULL,'','root',1,randomblob(16))",
            (root_id,),
        )
        folder_id = conn.execute("SELECT id FROM folders").fetchone()[0]
        conn.execute(
            "INSERT INTO files(root_id,folder_id,name,ext,size,mtime,asset_uuid) VALUES (?,?,'seed.jpg','jpg',3,1,randomblob(16))",
            (root_id, folder_id),
        )
        asset_uuid = conn.execute("SELECT asset_uuid FROM files").fetchone()[0]
        revision_uuid = bytes(range(16))
        conn.execute(
            "INSERT INTO asset_content_revisions(content_revision_uuid,asset_uuid,byte_size,local_mutation_generation,created_at) VALUES (?,?,3,1,1)",
            (revision_uuid, asset_uuid),
        )
        apply_migration(
            conn,
            ROOT / "crates" / "core" / "migrations" / "0029_media_object_storage.sql",
        )
        if not table_has_column(conn, "asset_content_revisions", "source_mtime_ms"):
            fail("0029 did not add asset_content_revisions.source_mtime_ms")
        if conn.execute(
            "SELECT byte_size FROM asset_content_revisions WHERE content_revision_uuid=?",
            (revision_uuid,),
        ).fetchone() != (3,):
            fail("0029 did not preserve populated content revisions")
        apply_migration(
            conn,
            ROOT / "crates" / "core" / "migrations" / "0029_media_object_storage.sql",
        )
        if conn.execute(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=29"
        ).fetchone() != (1,):
            fail("migration 29 rerun did not preserve one registration row")
    finally:
        conn.close()


def verify_sync_root_identity_contract() -> None:
    conn = sqlite3.connect(":memory:")
    conn.execute("PRAGMA foreign_keys = ON")
    try:
        for migration in MIGRATIONS:
            apply_migration(conn, migration)
        descriptor = bytes(range(16))
        root_folder = bytes(range(16, 32))
        library_root = bytes(range(32, 48))
        conn.execute(
            "INSERT INTO sync_root_descriptors(root_uuid,root_folder_uuid,library_root_uuid,requested_name,materialized_name,materialization_fingerprint,lifecycle,updated_at) VALUES (?,?,?,?,?,?,'active',1)",
            (descriptor, root_folder, library_root, "Photos", "Photos", bytes(32)),
        )
        if conn.execute("SELECT COUNT(*) FROM roots").fetchone()[0] != 0:
            fail("unselected sync root descriptor created browse roots")
        expect_integrity_error(
            conn,
            "UPDATE sync_root_descriptors SET root_folder_uuid=? WHERE root_uuid=?",
            (bytes(16), descriptor),
        )
        conn.execute(
            "UPDATE sync_root_descriptors SET requested_name='Renamed',materialized_name='Renamed',logical_revision=2,updated_at=2 WHERE root_uuid=?",
            (descriptor,),
        )
        expect_integrity_error(
            conn,
            "UPDATE sync_root_descriptors SET requested_name='Stale',logical_revision=1,updated_at=3 WHERE root_uuid=?",
            (descriptor,),
        )
        expect_integrity_error(
            conn,
            "UPDATE sync_root_descriptors SET requested_name='Conflicting',updated_at=3 WHERE root_uuid=?",
            (descriptor,),
        )
        if conn.execute("SELECT COUNT(*) FROM schema_migrations WHERE version=30").fetchone()[0] != 1:
            fail("migration 30 was not registered exactly once")
    finally:
        conn.close()


def verify_sync_root_generation_fence_contract() -> None:
    conn = sqlite3.connect(":memory:")
    conn.execute("PRAGMA foreign_keys = ON")
    try:
        for migration in MIGRATIONS:
            apply_migration(conn, migration)
        library_uuid = bytes(range(32, 48))
        root_uuid = bytes(range(16))
        root_folder_uuid = bytes(range(16, 32))
        binding_uuid = bytes(range(48, 64))
        conn.execute(
            "INSERT INTO sync_root_descriptors(root_uuid,root_folder_uuid,library_root_uuid,requested_name,materialized_name,materialization_fingerprint,lifecycle,updated_at) VALUES (?,?,?,?,?,?, 'active',1)",
            (root_uuid, root_folder_uuid, library_uuid, "Photos", "Photos", bytes(32)),
        )
        conn.execute(
            "INSERT INTO roots(path,display_name,enabled,created_at,root_uuid) VALUES ('D:/Photos','Photos',1,1,?)",
            (root_uuid,),
        )
        root_id = conn.execute("SELECT id FROM roots").fetchone()[0]
        conn.execute(
            "INSERT INTO folders(root_id,parent_id,name,path_hash,folder_uuid) VALUES (?,NULL,'','root',?)",
            (root_id, root_folder_uuid),
        )
        conn.execute(
            "UPDATE roots SET root_folder_uuid=? WHERE id=?",
            (root_folder_uuid, root_id),
        )
        conn.execute(
            "INSERT INTO sync_local_state(singleton,library_uuid,device_uuid,catalog_stream_uuid,updated_at) VALUES (1,?,randomblob(16),randomblob(16),1)",
            (library_uuid,),
        )
        conn.execute(
            "INSERT INTO sync_root_local_bindings(root_uuid,library_uuid,local_binding_uuid,binding_generation,state,root_id,operation_revision,updated_at) VALUES (?,?,?,1,'active',?,1,1)",
            (root_uuid, library_uuid, binding_uuid, root_id),
        )
        asset_uuid = bytes(range(64, 80))
        revision_uuid = bytes(range(80, 96))
        intent_uuid = bytes(range(96, 112))
        conn.execute(
            "INSERT INTO files(root_id,folder_id,name,ext,size,mtime,asset_uuid) VALUES (?,?,'photo.jpg','jpg',3,1,?)",
            (root_id, conn.execute("SELECT id FROM folders WHERE root_id=?", (root_id,)).fetchone()[0], asset_uuid),
        )
        conn.execute(
            "INSERT INTO asset_content_revisions(content_revision_uuid,asset_uuid,byte_size,local_mutation_generation,created_at) VALUES (?,?,3,1,1)",
            (revision_uuid, asset_uuid),
        )
        expect_integrity_error(
            conn,
            "UPDATE sync_root_local_bindings SET binding_generation=0,operation_revision=2 WHERE root_uuid=?",
            (root_uuid,),
        )
        conn.execute(
            "INSERT INTO tasks(kind,priority,status,root_id,created_at,updated_at) VALUES ('root_scan',0,'pending',?,1,1)",
            (root_id,),
        )
        task_id = conn.execute("SELECT id FROM tasks").fetchone()[0]
        if conn.execute(
            "SELECT root_uuid,local_binding_uuid,binding_generation FROM tasks WHERE id=?",
            (task_id,),
        ).fetchone() != (root_uuid, binding_uuid, 1):
            fail("managed task did not snapshot Catalog root ownership")
        conn.execute(
            "INSERT INTO file_operations(operation,source_path,status,created_at,root_id) VALUES ('rename','D:/Photos/photo.jpg','pending',1,?)",
            (root_id,),
        )
        file_operation_id = conn.execute(
            "SELECT id FROM file_operations ORDER BY id DESC LIMIT 1"
        ).fetchone()[0]
        conn.execute(
            "INSERT INTO sync_publication_intents(intent_uuid,asset_uuid,content_revision_uuid,local_mutation_generation,source_path,state,created_at,updated_at) VALUES (?,?,?,1,'D:/Photos/photo.jpg','pending-hash',1,1)",
            (intent_uuid, asset_uuid, revision_uuid),
        )
        conn.execute(
            "INSERT INTO media_publication_jobs(intent_uuid,phase,created_at,updated_at) VALUES (?,'pending-hash',1,1)",
            (intent_uuid,),
        )
        conn.execute(
            "INSERT INTO media_materialization_jobs(content_revision_uuid,asset_uuid,destination_relpath,phase,expected_length,created_at,updated_at) VALUES (?,?,'photo.jpg','pending',3,1,1)",
            (revision_uuid, asset_uuid),
        )
        for table, key_column, key_value in (
            ("file_operations", "id", file_operation_id),
            ("sync_publication_intents", "intent_uuid", intent_uuid),
            ("media_publication_jobs", "intent_uuid", intent_uuid),
            ("media_materialization_jobs", "content_revision_uuid", revision_uuid),
        ):
            if conn.execute(
                f"SELECT root_uuid,local_binding_uuid,binding_generation FROM {table} WHERE {key_column}=?",
                (key_value,),
            ).fetchone() != (root_uuid, binding_uuid, 1):
                fail(f"managed {table} row did not snapshot Catalog root ownership")
        expect_integrity_error(
            conn,
            "INSERT INTO tasks(kind,priority,status,created_at,updated_at,root_uuid,local_binding_uuid,binding_generation) VALUES ('thumbnail',0,'pending',1,1,?,?,1)",
            (root_uuid, binding_uuid),
        )
        expect_integrity_error(
            conn,
            "UPDATE file_operations SET local_binding_uuid=NULL WHERE id=?",
            (file_operation_id,),
        )
        expect_integrity_error(
            conn,
            "UPDATE sync_publication_intents SET binding_generation=NULL WHERE intent_uuid=?",
            (intent_uuid,),
        )
        expect_integrity_error(
            conn,
            "UPDATE media_publication_jobs SET local_binding_uuid=? WHERE intent_uuid=?",
            (bytes(reversed(binding_uuid)), intent_uuid),
        )
        expect_integrity_error(
            conn,
            "UPDATE media_materialization_jobs SET root_uuid=NULL WHERE content_revision_uuid=?",
            (revision_uuid,),
        )
        conn.execute(
            "UPDATE sync_root_local_bindings SET binding_generation=2,operation_revision=2,updated_at=2 WHERE root_uuid=?",
            (root_uuid,),
        )
        expect_integrity_error(
            conn,
            "UPDATE tasks SET status='running' WHERE id=?",
            (task_id,),
        )
        expect_integrity_error(
            conn,
            "UPDATE file_operations SET journal_phase='executing' WHERE id=?",
            (file_operation_id,),
        )
        expect_integrity_error(
            conn,
            "UPDATE sync_publication_intents SET state='pending-manifest' WHERE intent_uuid=?",
            (intent_uuid,),
        )
        expect_integrity_error(
            conn,
            "UPDATE media_publication_jobs SET owner_token='worker' WHERE intent_uuid=?",
            (intent_uuid,),
        )
        expect_integrity_error(
            conn,
            "UPDATE media_materialization_jobs SET owner_token='worker' WHERE content_revision_uuid=?",
            (revision_uuid,),
        )
        conn.execute(
            "INSERT INTO tasks(kind,priority,status,created_at,updated_at) VALUES ('thumbnail',0,'pending',1,1)"
        )
        legacy_task = conn.execute(
            "SELECT root_uuid,local_binding_uuid,binding_generation FROM tasks ORDER BY id DESC LIMIT 1"
        ).fetchone()
        if legacy_task != (None, None, None):
            fail("legacy task must remain unowned")
        expect_integrity_error(
            conn,
            "INSERT INTO tasks(kind,priority,status,created_at,updated_at,root_uuid) VALUES ('thumbnail',0,'pending',1,1,?)",
            (root_uuid,),
        )
        if conn.execute(
            "SELECT COUNT(*) FROM schema_migrations WHERE version=31"
        ).fetchone() != (1,):
            fail("migration 31 was not registered exactly once")
        for sql, params, expected_index in (
            ("SELECT id FROM tasks WHERE root_uuid=? AND local_binding_uuid=? AND binding_generation<? AND status IN ('pending','running') ORDER BY id LIMIT 100", (root_uuid, binding_uuid, 2), "idx_tasks_root_binding_work"),
            ("SELECT id FROM file_operations WHERE root_uuid=? AND local_binding_uuid=? AND binding_generation<? AND status='pending' ORDER BY id LIMIT 100", (root_uuid, binding_uuid, 2), "idx_file_operations_root_binding_work"),
            ("SELECT intent_uuid FROM sync_publication_intents WHERE root_uuid=? AND local_binding_uuid=? AND binding_generation<? AND state='pending-hash' ORDER BY intent_uuid LIMIT 100", (root_uuid, binding_uuid, 2), "idx_sync_publication_root_binding_work"),
            ("SELECT intent_uuid FROM media_publication_jobs WHERE root_uuid=? AND local_binding_uuid=? AND binding_generation<? AND phase='pending-hash' ORDER BY intent_uuid LIMIT 100", (root_uuid, binding_uuid, 2), "idx_media_publication_root_binding_work"),
            ("SELECT content_revision_uuid FROM media_materialization_jobs WHERE root_uuid=? AND local_binding_uuid=? AND binding_generation<? AND phase='pending' ORDER BY content_revision_uuid LIMIT 100", (root_uuid, binding_uuid, 2), "idx_media_materialization_root_binding_work"),
        ):
            assert_query_plan_uses(conn, sql, params, expected_index)
    finally:
        conn.close()


def verify_sync_root_generation_fence_upgrade_from_v30() -> None:
    conn = sqlite3.connect(":memory:")
    conn.execute("PRAGMA foreign_keys = ON")
    try:
        for migration in MIGRATIONS:
            if migration.name == "0031_sync_root_generation_fence.sql":
                break
            apply_migration(conn, migration)
        library_uuid = bytes(range(32, 48))
        root_uuid = bytes(range(16))
        root_folder_uuid = bytes(range(16, 32))
        conn.execute(
            "INSERT INTO sync_root_descriptors(root_uuid,root_folder_uuid,library_root_uuid,requested_name,materialized_name,materialization_fingerprint,lifecycle,updated_at) VALUES (?,?,?,?,?,?, 'active',1)",
            (root_uuid, root_folder_uuid, library_uuid, "Legacy", "Legacy", bytes(32)),
        )
        conn.execute(
            "INSERT INTO roots(path,display_name,enabled,created_at,root_uuid) VALUES ('D:/Legacy','Legacy',1,1,?)",
            (root_uuid,),
        )
        root_id = conn.execute("SELECT id FROM roots").fetchone()[0]
        conn.execute(
            "INSERT INTO folders(root_id,parent_id,name,path_hash,folder_uuid) VALUES (?,NULL,'','legacy-root',?)",
            (root_id, root_folder_uuid),
        )
        conn.execute(
            "UPDATE roots SET root_folder_uuid=? WHERE id=?",
            (root_folder_uuid, root_id),
        )
        folder_id = conn.execute("SELECT id FROM folders").fetchone()[0]
        conn.execute(
            "INSERT INTO files(root_id,folder_id,name,ext,size,mtime,asset_uuid) VALUES (?,?,'legacy.jpg','jpg',3,1,randomblob(16))",
            (root_id, folder_id),
        )
        asset_uuid = conn.execute("SELECT asset_uuid FROM files").fetchone()[0]
        revision_uuid = bytes(range(16))
        intent_uuid = bytes(range(16, 32))
        conn.execute(
            "INSERT INTO asset_content_revisions(content_revision_uuid,asset_uuid,byte_size,local_mutation_generation,created_at) VALUES (?,?,3,1,1)",
            (revision_uuid, asset_uuid),
        )
        conn.execute(
            "INSERT INTO tasks(kind,priority,status,root_id,created_at,updated_at) VALUES ('root_scan',0,'pending',?,1,1)",
            (root_id,),
        )
        conn.execute(
            "INSERT INTO file_operations(operation,source_path,status,created_at,root_id) VALUES ('rename','D:/Legacy/legacy.jpg','pending',1,?)",
            (root_id,),
        )
        conn.execute(
            "INSERT INTO sync_publication_intents(intent_uuid,asset_uuid,content_revision_uuid,local_mutation_generation,source_path,state,created_at,updated_at) VALUES (?,?,?,1,'D:/Legacy/legacy.jpg','pending-hash',1,1)",
            (intent_uuid, asset_uuid, revision_uuid),
        )
        conn.execute(
            "INSERT INTO media_publication_jobs(intent_uuid,phase,created_at,updated_at) VALUES (?,'pending-hash',1,1)",
            (intent_uuid,),
        )
        conn.execute(
            "INSERT INTO media_materialization_jobs(content_revision_uuid,asset_uuid,destination_relpath,phase,expected_length,created_at,updated_at) VALUES (?,?,'legacy.jpg','pending',3,1,1)",
            (revision_uuid, asset_uuid),
        )
        apply_migration(
            conn,
            ROOT / "crates" / "core" / "migrations" / "0031_sync_root_generation_fence.sql",
        )
        for table in (
            "tasks",
            "file_operations",
            "sync_publication_intents",
            "media_publication_jobs",
            "media_materialization_jobs",
        ):
            columns = {row[1] for row in conn.execute(f"PRAGMA table_info({table})")}
            if not {"root_uuid", "local_binding_uuid", "binding_generation"} <= columns:
                fail(f"migration 31 did not add ownership triplet to {table}")
            if conn.execute(
                f"SELECT COUNT(*) FROM {table} WHERE root_uuid IS NOT NULL OR local_binding_uuid IS NOT NULL OR binding_generation IS NOT NULL"
            ).fetchone() != (0,):
                fail(f"migration 31 guessed ownership for existing {table} rows")
        if conn.execute("SELECT COUNT(*) FROM sync_root_local_bindings").fetchone() != (0,):
            fail("migration 31 fabricated a Catalog binding from schema-30 path/name state")
        expect_integrity_error(conn, "UPDATE tasks SET status='running'")
        expect_integrity_error(
            conn, "UPDATE file_operations SET journal_phase='executing'"
        )
        expect_integrity_error(
            conn, "UPDATE sync_publication_intents SET state='pending-manifest'"
        )
        expect_integrity_error(
            conn, "UPDATE media_publication_jobs SET owner_token='worker'"
        )
        expect_integrity_error(
            conn, "UPDATE media_materialization_jobs SET owner_token='worker'"
        )
    finally:
        conn.close()


def verify_host_sync_root_binding_contract() -> None:
    conn = sqlite3.connect(":memory:")
    conn.execute("PRAGMA foreign_keys = ON")
    try:
        for migration in HOST_MIGRATIONS:
            conn.executescript(migration.read_text(encoding="utf-8"))
        library_uuid = "00000000-0000-7000-8000-000000000001"
        library_root_uuid = "00000000-0000-7000-8000-000000000099"
        conn.execute(
            "INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES (?,?,'Library','local','D:/Legacy','ready','local-only','available',1,'marker',1,1)",
            (library_uuid, library_root_uuid),
        )
        expect_integrity_error(
            conn,
            "INSERT INTO library_local_settings(library_uuid,library_root_uuid,default_placement_base,canonical_placement_key,created_at,updated_at) VALUES (?,?,?,?,1,1)",
            (
                library_uuid,
                "00000000-0000-7000-8000-000000000098",
                "D:/Wrong",
                "d:/wrong",
            ),
        )
        conn.execute(
            "INSERT INTO library_local_settings(library_uuid,library_root_uuid,default_placement_base,canonical_placement_key,created_at,updated_at) VALUES (?,?,?,?,1,1)",
            (
                library_uuid,
                library_root_uuid,
                "D:/Library",
                "d:/library",
            ),
        )
        conn.execute(
            "INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES (?,?,'Library 2','local2','D:/Legacy2','ready','local-only','available',1,'marker2',1,1)",
            (
                "00000000-0000-7000-8000-000000000090",
                "00000000-0000-7000-8000-000000000091",
            ),
        )
        conn.execute(
            "INSERT INTO library_local_settings(library_uuid,library_root_uuid,default_placement_base,canonical_placement_key,created_at,updated_at) VALUES (?,?,?,?,1,1)",
            (
                "00000000-0000-7000-8000-000000000090",
                "00000000-0000-7000-8000-000000000091",
                "D:/Library",
                "d:/library",
            ),
        )
        conn.execute(
            "INSERT INTO remote_sync_roots(library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name_key,materialization_fingerprint,lifecycle,inventory_state,updated_at) VALUES (?,?,?,?,?,?,?,'active','placement-required',1)",
            (
                library_uuid,
                "00000000-0000-7000-8000-000000000002",
                "00000000-0000-7000-8000-000000000003",
                "Photos",
                "Photos",
                "photos",
                "a" * 64,
            ),
        )
        if conn.execute("SELECT COUNT(*) FROM local_root_bindings").fetchone()[0] != 0:
            fail("placement-required remote root created a placeholder binding")
        expect_integrity_error(
            conn,
            "UPDATE remote_sync_roots SET root_folder_uuid=? WHERE library_uuid=? AND root_uuid=?",
            (
                "00000000-0000-7000-8000-000000000004",
                library_uuid,
                "00000000-0000-7000-8000-000000000002",
            ),
        )
        expect_integrity_error(
            conn,
            "INSERT INTO remote_sync_roots(library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name_key,materialization_fingerprint,lifecycle,inventory_state,updated_at) VALUES (?,?,?,?,?,?,?,'active','unselected',1)",
            (
                library_uuid,
                "00000000-0000-7000-8000-000000000004",
                "00000000-0000-7000-8000-000000000005",
                "PHOTOS",
                "PHOTOS",
                "photos",
                "b" * 64,
            ),
        )
        conn.execute(
            "INSERT INTO remote_sync_roots(library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name_key,materialization_fingerprint,lifecycle,inventory_state,updated_at) VALUES (?,?,?,?,?,?,?,'tombstoned','tombstoned',1)",
            (
                library_uuid,
                "00000000-0000-7000-8000-000000000025",
                "00000000-0000-7000-8000-000000000026",
                "Photos",
                "Photos",
                "photos",
                "b" * 64,
            ),
        )
        expect_integrity_error(
            conn,
            "INSERT INTO remote_sync_roots(library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name_key,materialization_fingerprint,lifecycle,inventory_state,updated_at) VALUES (?,?,?,?,?,?,?,'active','unselected',1)",
            (
                library_uuid,
                "not-a-canonical-uuid-value-00000000",
                "00000000-0000-7000-8000-000000000006",
                "Other",
                "Other",
                "other",
                "c" * 64,
            ),
        )
        expect_integrity_error(
            conn,
            "INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,state,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                library_uuid,
                "00000000-0000-7000-8000-000000000002",
                "00000000-0000-7000-8000-000000000007",
                "D:/Library",
                "d:/library",
                "D:/",
                "d:/",
                "materializing",
                "Photos",
                "photos",
                "d" * 64,
                1,
                1,
            ),
        )
        conn.execute(
            "INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,state,binding_generation,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,applied_materialized_name,applied_materialized_name_key,applied_materialization_fingerprint,created_at,updated_at) VALUES (?,?,?,?,?,?,?,'active',1,?,?,?,?,?,?,1,1)",
            (
                library_uuid,
                "00000000-0000-7000-8000-000000000002",
                "00000000-0000-7000-8000-000000000007",
                "D:/Library/Photos",
                "d:/library/photos",
                "D:/Library",
                "d:/library",
                "Photos",
                "photos",
                "d" * 64,
                "Photos",
                "photos",
                "d" * 64,
            ),
        )
        if conn.execute(
            "SELECT operation_revision FROM remote_sync_roots WHERE library_uuid=? AND root_uuid=?",
            (library_uuid, "00000000-0000-7000-8000-000000000002"),
        ).fetchone() != (0,):
            fail("remote sync root operation revision was not initialized")
        expect_integrity_error(
            conn,
            "INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,state,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                library_uuid,
                "00000000-0000-7000-8000-000000000002",
                "00000000-0000-7000-8000-000000000016",
                "D:/Elsewhere/Photos",
                "d:/elsewhere/photos",
                "D:/Elsewhere",
                "d:/elsewhere",
                "materializing",
                "Photos",
                "photos",
                "d" * 64,
                1,
                1,
            ),
        )
        conn.execute(
            "INSERT INTO remote_sync_roots(library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialized_name_key,materialization_fingerprint,lifecycle,inventory_state,updated_at) VALUES (?,?,?,?,?,?,?,'active','placement-required',1)",
            (
                library_uuid,
                "00000000-0000-7000-8000-000000000016",
                "00000000-0000-7000-8000-000000000017",
                "Videos",
                "Videos",
                "videos",
                "4" * 64,
            ),
        )
        expect_integrity_error(
            conn,
            "INSERT INTO local_root_bindings(library_uuid,root_uuid,local_binding_uuid,local_path,canonical_path_key,placement_parent,placement_parent_key,state,desired_materialized_name,desired_materialized_name_key,desired_materialization_fingerprint,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                library_uuid,
                "00000000-0000-7000-8000-000000000016",
                "00000000-0000-7000-8000-000000000018",
                "D:/Library/Photos/Nested",
                "d:/library/photos/nested",
                "D:/Library/Photos",
                "d:/library/photos",
                "materializing",
                "Videos",
                "videos",
                "4" * 64,
                1,
                1,
            ),
        )
        stop_operation_uuid = "00000000-0000-7000-8000-000000000010"
        snapshot_uuid = "00000000-0000-7000-8000-000000000011"
        conn.execute(
            "INSERT INTO root_operations(operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,cutover_committed,created_at,updated_at) VALUES (?,?,?,'stop-sync','completed','completed',?,'{}',0,1,1,1,1,1)",
            (
                stop_operation_uuid,
                library_uuid,
                "00000000-0000-7000-8000-000000000002",
                "e" * 64,
            ),
        )
        conn.execute(
            "INSERT INTO root_cutover_snapshots(snapshot_uuid,operation_uuid,library_uuid,root_uuid,binding_generation,event_frontier_json,catalog_snapshot_digest,provider_receipts_json,remote_visibility,created_at) VALUES (?,?,?,?,1,'{}',?,'[]','hidden',1)",
            (
                snapshot_uuid,
                stop_operation_uuid,
                library_uuid,
                "00000000-0000-7000-8000-000000000002",
                "f" * 64,
            ),
        )
        expect_integrity_error(
            conn,
            "UPDATE root_cutover_snapshots SET remote_visibility='visible' WHERE snapshot_uuid=?",
            (snapshot_uuid,),
        )
        expect_integrity_error(
            conn,
            "DELETE FROM root_cutover_snapshots WHERE snapshot_uuid=?",
            (snapshot_uuid,),
        )
        reenable_operation_uuid = "00000000-0000-7000-8000-000000000012"
        comparison_plan_uuid = "00000000-0000-7000-8000-000000000013"
        conn.execute(
            "INSERT INTO root_operations(operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,operation_revision,plan_uuid,created_at,updated_at) VALUES (?,?,?,'re-enable','running','comparison-ready',?,'{}',1,1,1,?,2,2)",
            (
                reenable_operation_uuid,
                library_uuid,
                "00000000-0000-7000-8000-000000000002",
                "0" * 64,
                comparison_plan_uuid,
            ),
        )
        expect_integrity_error(
            conn,
            "UPDATE root_operations SET phase='fencing' WHERE operation_uuid=?",
            (reenable_operation_uuid,),
        )
        expect_integrity_error(
            conn,
            "UPDATE root_operations SET state='completed' WHERE operation_uuid=?",
            (reenable_operation_uuid,),
        )
        expect_integrity_error(
            conn,
            "UPDATE root_operations SET state='failed',phase='planned' WHERE operation_uuid=?",
            (reenable_operation_uuid,),
        )
        expect_integrity_error(
            conn,
            "UPDATE root_operations SET expected_binding_generation=2 WHERE operation_uuid=?",
            (reenable_operation_uuid,),
        )
        expect_integrity_error(
            conn,
            "INSERT INTO root_cutover_snapshots(snapshot_uuid,operation_uuid,library_uuid,root_uuid,binding_generation,event_frontier_json,catalog_snapshot_digest,provider_receipts_json,remote_visibility,created_at) VALUES (?,?,?,?,1,'{}',?,'[]','hidden',1)",
            (
                "00000000-0000-7000-8000-000000000019",
                reenable_operation_uuid,
                library_uuid,
                "00000000-0000-7000-8000-000000000002",
                "9" * 64,
            ),
        )
        conn.execute(
            "INSERT INTO root_comparison_plans(plan_uuid,operation_uuid,cutover_snapshot_uuid,library_uuid,root_uuid,base_digest,local_digest,remote_digest,base_frontier_json,local_frontier_json,remote_frontier_json,root_revision,account_generation,provider_generation,item_count,conflict_count,status,expires_at,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,2,1,'ready',2,1,1)",
            (
                comparison_plan_uuid,
                reenable_operation_uuid,
                snapshot_uuid,
                library_uuid,
                "00000000-0000-7000-8000-000000000002",
                "1" * 64,
                "2" * 64,
                "3" * 64,
                "{}",
                "{}",
                "{}",
                3,
                4,
                5,
            ),
        )
        conn.executemany(
            "INSERT INTO root_comparison_items(plan_uuid,item_ordinal,entity_uuid,field_name,classification,suggested_action,redacted_detail_json) VALUES (?,?,?,?,?,?,?)",
            (
                (
                    comparison_plan_uuid,
                    0,
                    "00000000-0000-7000-8000-000000000014",
                    "name",
                    "same-change",
                    None,
                    "{}",
                ),
                (
                    comparison_plan_uuid,
                    1,
                    "00000000-0000-7000-8000-000000000015",
                    "parent",
                    "conflict",
                    "keep-local",
                    "{}",
                ),
            ),
        )
        comparison_page = conn.execute(
            "SELECT item_ordinal FROM root_comparison_items WHERE plan_uuid=? AND item_ordinal>? ORDER BY item_ordinal LIMIT 1",
            (comparison_plan_uuid, 0),
        ).fetchall()
        if comparison_page != [(1,)]:
            fail(f"comparison item cursor page was not stable: {comparison_page}")
        conn.execute(
            "INSERT INTO root_comparison_resolutions(plan_uuid,item_ordinal,resolution,resolved_at) VALUES (?,1,'keep-local',1)",
            (comparison_plan_uuid,),
        )
        expect_integrity_error(
            conn,
            "UPDATE root_comparison_items SET classification='unchanged' WHERE plan_uuid=? AND item_ordinal=1",
            (comparison_plan_uuid,),
        )
        expect_integrity_error(
            conn,
            "UPDATE root_comparison_plans SET base_digest=? WHERE plan_uuid=?",
            ("8" * 64, comparison_plan_uuid),
        )
        expect_integrity_error(
            conn,
            "INSERT INTO root_comparison_resolutions(plan_uuid,item_ordinal,resolution,resolved_at) VALUES (?,0,'discard-both',1)",
            (comparison_plan_uuid,),
        )
        conn.execute(
            "UPDATE root_comparison_plans SET status='expired',updated_at=2 WHERE status='ready' AND expires_at<=2"
        )
        if conn.execute(
            "SELECT status FROM root_comparison_plans WHERE plan_uuid=?",
            (comparison_plan_uuid,),
        ).fetchone() != ("expired",):
            fail("expired comparison plan did not transition through the indexed expiry query")
        conn.execute(
            "UPDATE root_operations SET state='completed',phase='completed' WHERE operation_uuid=?",
            (reenable_operation_uuid,),
        )
        relocation_operation_uuid = "00000000-0000-7000-8000-000000000027"
        conn.execute(
            "INSERT INTO root_operations(operation_uuid,library_uuid,root_uuid,kind,state,phase,request_hash,request_json,expected_operation_revision,expected_binding_generation,created_at,updated_at) VALUES (?,?,?,'relocate','running','planned',?,'{}',0,1,3,3)",
            (
                relocation_operation_uuid,
                library_uuid,
                "00000000-0000-7000-8000-000000000002",
                "7" * 64,
            ),
        )
        conn.execute(
            "INSERT INTO root_relocation_operations(operation_uuid,mode,desired_materialization_fingerprint,source_path,source_canonical_key,target_path,target_canonical_key,updated_at) VALUES (?,'explicit-move',?,'D:/Library/Photos','d:/library/photos','E:/Photos','e:/photos',3)",
            (relocation_operation_uuid, "6" * 64),
        )
        expect_integrity_error(
            conn,
            "UPDATE root_relocation_operations SET target_path='F:/Photos' WHERE operation_uuid=?",
            (relocation_operation_uuid,),
        )
        expect_integrity_error(
            conn,
            "DELETE FROM remote_sync_roots WHERE library_uuid=? AND root_uuid=?",
            (library_uuid, "00000000-0000-7000-8000-000000000002"),
        )
        if conn.execute("SELECT schema_version FROM host_meta WHERE singleton=1").fetchone()[0] != 16:
            fail("managed-root orchestration migrations did not update schema_version")
        expected_host_tables = {
            "library_local_settings",
            "remote_sync_roots",
            "local_root_bindings",
            "root_operations",
            "root_cutover_snapshots",
            "root_comparison_plans",
            "root_comparison_items",
            "root_comparison_resolutions",
            "root_relocation_operations",
            "root_path_preflights",
            "root_stop_plans",
            "root_visibility_reconciliations",
            "root_comparison_snapshots",
            "root_reconciliation_work",
            "root_publish_as_new",
        }
        actual = {row[0] for row in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        missing = expected_host_tables - actual
        if missing:
            fail(f"missing managed-root-v2 Host tables: {sorted(missing)}")
    finally:
        conn.close()


def verify_host_remote_root_inventory_upgrade_and_rollback() -> None:
    upgrade = sqlite3.connect(":memory:")
    upgrade.execute("PRAGMA foreign_keys = ON")
    try:
        for migration in HOST_MIGRATIONS[:-1]:
            upgrade.executescript(migration.read_text(encoding="utf-8"))
        if upgrade.execute("SELECT schema_version FROM host_meta WHERE singleton=1").fetchone() != (15,):
            fail("remote-root inventory upgrade fixture did not stop at Host schema 15")
        upgrade.execute(
            "INSERT INTO plugin_installations(plugin_id,manifest_version,selected_version,enabled_intent,install_root,trust_policy_id,requested_permissions_json,granted_permissions_json,created_at,updated_at) VALUES ('provider',2,'1',1,'provider','test','[]','[]',1,1)"
        )
        upgrade.execute(
            "INSERT INTO plugin_accounts(account_uuid,plugin_id,display_name,auth_state,generation,created_at,updated_at) VALUES ('account','provider','Account','authenticated',1,1,1)"
        )
        upgrade.execute(
            "INSERT INTO provider_remote_libraries(account_uuid,library_uuid,descriptor_size,descriptor_sha256,account_generation,updated_at) VALUES ('account','00000000-0000-7000-8000-000000000001',1,zeroblob(32),1,1)"
        )
        upgrade.commit()
        upgrade.executescript(HOST_MIGRATIONS[-1].read_text(encoding="utf-8"))
        if upgrade.execute("SELECT schema_version FROM host_meta WHERE singleton=1").fetchone() != (16,):
            fail("remote-root inventory migration did not upgrade Host schema 15 to 16")
        upgrade.execute(
            "INSERT INTO provider_remote_sync_roots(account_uuid,library_uuid,root_uuid,root_folder_uuid,requested_name,materialized_name,materialization_fingerprint,logical_revision,logical_bytes,updated_at) VALUES ('account','00000000-0000-7000-8000-000000000001','00000000-0000-7000-8000-000000000002','00000000-0000-7000-8000-000000000003','Photos','Photos',?,7,41,1)",
            ("a" * 64,),
        )
        upgrade.execute(
            "DELETE FROM provider_remote_libraries WHERE account_uuid='account' AND library_uuid='00000000-0000-7000-8000-000000000001'"
        )
        if upgrade.execute("SELECT COUNT(*) FROM provider_remote_sync_roots").fetchone() != (0,):
            fail("remote-root inventory rows did not cascade with their cached Library")
    finally:
        upgrade.close()

    rollback = sqlite3.connect(":memory:")
    try:
        for migration in HOST_MIGRATIONS[:-1]:
            rollback.executescript(migration.read_text(encoding="utf-8"))
        rollback.execute("CREATE TABLE provider_remote_sync_roots(sentinel TEXT NOT NULL)")
        rollback.commit()
        try:
            rollback.executescript(HOST_MIGRATIONS[-1].read_text(encoding="utf-8"))
        except sqlite3.OperationalError:
            rollback.rollback()
        else:
            fail("remote-root inventory migration unexpectedly succeeded over a conflicting table")
        if rollback.execute("SELECT schema_version FROM host_meta WHERE singleton=1").fetchone() != (15,):
            fail("failed remote-root inventory migration advanced the Host schema version")
        columns = [row[1] for row in rollback.execute("PRAGMA table_info(provider_remote_sync_roots)")]
        if columns != ["sentinel"]:
            fail("failed remote-root inventory migration left partial schema changes")
    finally:
        rollback.close()


def verify_host_plugin_upgrade_from_populated_v2() -> None:
    conn = sqlite3.connect(":memory:")
    try:
        conn.execute("PRAGMA foreign_keys = ON")
        for migration in HOST_MIGRATIONS[:2]:
            conn.executescript(migration.read_text(encoding="utf-8"))
        conn.execute(
            "INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES ('library','root','Library','local','managed','ready','local-only','available',1,'marker',1,1)"
        )
        conn.execute(
            "INSERT INTO library_sync_bindings(library_uuid,provider_id,account_identity,binding_epoch,actor_epoch,state,created_at,updated_at) VALUES ('library','legacy','account',randomblob(16),randomblob(16),'active',1,1)"
        )
        conn.executescript(HOST_MIGRATIONS[2].read_text(encoding="utf-8"))
        if conn.execute("SELECT COUNT(*) FROM libraries").fetchone()[0] != 1:
            fail("Host 0003 removed a Library registration")
        if conn.execute("SELECT COUNT(*) FROM library_sync_bindings").fetchone()[0] != 0:
            fail("Host 0003 retained a development-only legacy binding")
        if conn.execute("SELECT schema_version FROM host_meta").fetchone()[0] != 3:
            fail("Host 0003 did not advance schema version")
        # HostRegistry gates numbered migrations by host_meta. Reopening at v3
        # must not execute the destructive transition a second time.
        if conn.execute("SELECT schema_version FROM host_meta").fetchone()[0] < 3:
            conn.executescript(HOST_MIGRATIONS[2].read_text(encoding="utf-8"))
        if conn.execute("SELECT COUNT(*) FROM libraries").fetchone()[0] != 1:
            fail("Host 0003 reopen gate was not idempotent")
        conn.executescript(HOST_MIGRATIONS[3].read_text(encoding="utf-8"))
        if conn.execute("SELECT schema_version FROM host_meta").fetchone()[0] != 4:
            fail("Host 0004 did not advance schema version")
        if not table_has_column(conn, "provider_account_root_cursors", "page_token"):
            fail("Host 0004 did not add root page_token")
        if conn.execute("SELECT COUNT(*) FROM libraries").fetchone()[0] != 1:
            fail("Host 0004 removed a Library registration")
    finally:
        conn.close()


def assert_query_plan_uses(conn: sqlite3.Connection, sql: str, params, expected: str) -> None:
    detail = " | ".join(
        str(row[3]) for row in conn.execute(f"EXPLAIN QUERY PLAN {sql}", params)
    )
    if expected not in detail:
        fail(f"query plan did not use {expected}: {detail}")


def verify_sync_query_plans(conn: sqlite3.Connection) -> None:
    # These plans are independent of row count; validating the covering access
    # paths here is the same planner contract exercised by million-row fixtures
    # without making the fast schema gate allocate a million records.
    assert_query_plan_uses(
        conn,
        "SELECT event_uuid FROM sync_outbox WHERE state='pending' AND (next_attempt_at IS NULL OR next_attempt_at<=?) ORDER BY next_attempt_at,created_at,event_uuid LIMIT 100",
        (1,),
        "idx_sync_outbox_pending",
    )
    assert_query_plan_uses(
        conn,
        "SELECT segment_uuid FROM sync_inbox_segments WHERE state='ready' ORDER BY received_at,segment_uuid LIMIT 1",
        (),
        "idx_sync_inbox_ready",
    )
    assert_query_plan_uses(
        conn,
        "SELECT entity_uuid FROM sync_bulk_items WHERE bulk_operation_uuid=? AND state='pending' AND entity_uuid>? ORDER BY entity_uuid LIMIT 100",
        (b"0" * 16, b""),
        "idx_sync_bulk_items_pending",
    )
    assert_query_plan_uses(
        conn,
        "SELECT covered_through_seq FROM sync_checkpoint_watermarks WHERE stream_uuid=? AND device_uuid=? AND actor_epoch=?",
        (b"0" * 16, b"1" * 16, b"2" * 16),
        "idx_sync_checkpoint_watermark_actor",
    )
    assert_query_plan_uses(
        conn,
        "SELECT intent_uuid FROM media_publication_jobs WHERE phase NOT IN ('canonicalized','event-published','cancelled','archived') AND COALESCE(next_attempt_at,0)<=? ORDER BY priority DESC,next_attempt_at,updated_at,intent_uuid LIMIT 1",
        (1,),
        "idx_media_publication_work",
    )
    assert_query_plan_uses(
        conn,
        "SELECT chunk_hash FROM media_chunks WHERE pack_hash=? ORDER BY pack_offset",
        (b"0" * 32,),
        "idx_media_chunks_pack",
    )
    assert_query_plan_uses(
        conn,
        "SELECT content_revision_uuid FROM media_materialization_jobs WHERE phase NOT IN ('complete','cancelled') AND COALESCE(next_attempt_at,0)<=? ORDER BY priority DESC,next_attempt_at,updated_at,content_revision_uuid LIMIT 1",
        (1,),
        "idx_media_materialization_work",
    )


def main() -> None:
    for migration in MIGRATIONS:
        if not migration.exists():
            fail(f"missing migration: {migration}")

    verify_scan_reconciliation_repair_path()
    verify_sync_upgrade_from_populated_v26()
    verify_media_upgrade_from_populated_v28()
    verify_sync_root_identity_contract()
    verify_sync_root_generation_fence_contract()
    verify_sync_root_generation_fence_upgrade_from_v30()
    verify_host_sync_root_binding_contract()
    verify_host_remote_root_inventory_upgrade_and_rollback()
    verify_host_plugin_upgrade_from_populated_v2()

    for host_migration in HOST_MIGRATIONS:
        if not host_migration.exists():
            fail(f"missing Host Registry migration: {host_migration}")
    host_conn = sqlite3.connect(":memory:")
    try:
        for host_migration in HOST_MIGRATIONS:
            host_conn.executescript(host_migration.read_text(encoding="utf-8"))
        host_tables = {
            row[0]
            for row in host_conn.execute(
                "SELECT name FROM sqlite_schema WHERE type = 'table'"
            )
        }
        required_host_tables = {
            "host_meta",
            "libraries",
            "library_creation_journal",
            "library_removal_journal",
            "host_sync_local_state",
            "host_sync_actor_epochs",
            "host_sync_actor_frontiers",
            "host_sync_events",
            "host_sync_outbox",
            "host_sync_field_versions",
            "library_sync_bindings",
            "plugin_installations",
            "plugin_versions",
            "plugin_runtime_state",
            "plugin_accounts",
            "provider_account_root_cursors",
            "provider_remote_libraries",
            "provider_remote_sync_roots",
            "provider_binding_cursors",
            "provider_object_receipts",
            "plugin_activation_journal",
            "plugin_version_journal",
            "onedrive_configuration",
            "onedrive_configuration_plans",
            "plugin_credential_retirements",
            "plugin_auth_flow_ownership",
            "plugin_secret_handles",
            "plugin_secret_creations",
        "sync_orchestration_operations",
        "sync_preflight_tokens",
            "sync_unbind_plans",
            "sync_reconnect_plans",
            "sync_restore_plans",
            "sync_restore_preflights",
        }
        required_host_tables |= REQUIRED_HOST_SYNC_ROOT_TABLES
        missing_host_tables = required_host_tables - host_tables
        if missing_host_tables:
            fail(f"missing Host Registry tables: {sorted(missing_host_tables)}")
        meta = host_conn.execute(
            "SELECT schema_version, active_library_uuid, context_revision, sync_scheduler_state FROM host_meta WHERE singleton = 1"
        ).fetchone()
        if meta != (16, None, 0, "stopped"):
            fail(f"unexpected Host Registry metadata seed: {meta}")
        auth_flow_columns = {
            row[1]
            for row in host_conn.execute("PRAGMA table_info(plugin_auth_flow_ownership)")
        }
        if not {"state", "prior_secret_ref", "prior_stable_account_identity"}.issubset(auth_flow_columns):
            fail("plugin_auth_flow_ownership is missing cancellation-fence columns")
        secret_creation_columns = {
            row[1]
            for row in host_conn.execute("PRAGMA table_info(plugin_secret_creations)")
        }
        if not {
            "plugin_id",
            "account_uuid",
            "secret_ref",
            "namespace",
            "purpose",
            "launch_fingerprint",
        }.issubset(secret_creation_columns):
            fail("plugin_secret_creations is missing durable creation metadata")
        binding_cursor_columns = {
            row[1]
            for row in host_conn.execute(
                "PRAGMA table_info(provider_binding_cursors)"
            )
        }
        if "page_token" not in binding_cursor_columns:
            fail("provider_binding_cursors.page_token is required for durable pagination")
        root_cursor_columns = {
            row[1]
            for row in host_conn.execute(
                "PRAGMA table_info(provider_account_root_cursors)"
            )
        }
        if "page_token" not in root_cursor_columns:
            fail("provider_account_root_cursors.page_token is required for durable root pagination")
        remote_library_columns = {
            row[1]
            for row in host_conn.execute("PRAGMA table_info(provider_remote_libraries)")
        }
        required_remote_library_columns = {
            "name",
            "icon",
            "media_count",
            "logical_bytes",
            "last_synced_at",
            "compatible",
            "descriptor_error_code",
        }
        if not required_remote_library_columns.issubset(remote_library_columns):
            fail("provider_remote_libraries is missing descriptor summary columns")
        binding_columns = {
            row[1]
            for row in host_conn.execute("PRAGMA table_info(library_sync_bindings)")
        }
        required_binding_status = {
            "pending_upload_items",
            "pending_upload_bytes",
            "pending_download_items",
            "pending_download_bytes",
            "conflict_count",
        }
        if not required_binding_status.issubset(binding_columns):
            fail("library_sync_bindings is missing orchestration status counters")
        creation_columns = {
            row[1]
            for row in host_conn.execute(
                "PRAGMA table_info(library_creation_journal)"
            )
        }
        required_creation_columns = {"root_identity", "staging_identity", "marker_identity"}
        missing_creation_columns = required_creation_columns - creation_columns
        if missing_creation_columns:
            fail(
                "missing Host creation-journal identity columns: "
                f"{sorted(missing_creation_columns)}"
            )
        host_indexes = {
            row[0]
            for row in host_conn.execute(
                "SELECT name FROM sqlite_schema WHERE type = 'index' AND name IS NOT NULL"
            )
        }
        required_host_indexes = {
            "idx_host_sync_outbox_pending",
            "idx_host_sync_field_winner",
            "idx_host_sync_field_history",
            "idx_plugin_versions_state",
            "idx_plugin_runtime_schedule",
            "idx_provider_binding_schedule",
            "idx_provider_receipts_state",
            "idx_sync_operations_library",
            "idx_sync_operations_active",
            "idx_sync_unbind_plans_state",
            "idx_sync_reconnect_plans_state",
            "idx_sync_restore_plans_state",
            "idx_sync_restore_plans_operation",
            "idx_sync_preflight_expiry",
            "ux_libraries_library_root_identity",
            "ux_remote_sync_roots_active_materialized_name",
            "ux_local_root_bindings_sibling_name",
            "ux_local_root_bindings_desired_sibling_name",
            "idx_local_root_bindings_state",
            "ux_root_operations_one_open",
            "idx_root_operations_recovery",
            "idx_remote_sync_roots_inventory",
            "idx_root_comparison_items_page",
            "idx_root_comparison_plans_expiry",
        }
        missing_host_indexes = required_host_indexes - host_indexes
        if missing_host_indexes:
            fail(f"missing Host sync indexes: {sorted(missing_host_indexes)}")
        expect_integrity_error(
            host_conn,
            "INSERT INTO library_sync_bindings(library_uuid,plugin_id,account_uuid,binding_epoch,actor_epoch,state,created_at,updated_at) VALUES ('missing','onedrive','account',randomblob(16),randomblob(16),'unknown',1,1)",
        )
        expect_integrity_error(
            host_conn,
            "INSERT INTO plugin_installations(plugin_id,manifest_version,selected_version,enabled_intent,install_root,trust_policy_id,requested_permissions_json,granted_permissions_json,created_at,updated_at) VALUES ('invalid',1,'1.0.0',0,'root','bundled','[]','[]',1,1)",
        )
        expect_integrity_error(
            host_conn,
            "INSERT INTO plugin_runtime_state(plugin_id,state,updated_at) VALUES ('missing','ready',1)",
        )
        expect_integrity_error(
            host_conn,
            "INSERT INTO host_sync_actor_epochs(actor_epoch,device_uuid,stream_uuid,state,created_at) VALUES (randomblob(15),randomblob(16),randomblob(16),'active',1)",
        )
    finally:
        host_conn.close()

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
            verify_sync_query_plans(conn)

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
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 21"
            ).fetchone()
            if version is None:
                fail("migration version 21 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 22"
            ).fetchone()
            if version is None:
                fail("migration version 22 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 23"
            ).fetchone()
            if version is None:
                fail("migration version 23 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 24"
            ).fetchone()
            if version is None:
                fail("migration version 24 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 25"
            ).fetchone()
            if version is None:
                fail("migration version 25 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 26"
            ).fetchone()
            if version is None:
                fail("migration version 26 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 27"
            ).fetchone()
            if version is None:
                fail("migration version 27 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 29"
            ).fetchone()
            if version is None:
                fail("migration version 29 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 30"
            ).fetchone()
            if version is None:
                fail("migration version 30 was not recorded")
            version = conn.execute(
                "SELECT version FROM schema_migrations WHERE version = 31"
            ).fetchone()
            if version is None:
                fail("migration version 31 was not recorded")

            for table, column in (
                ("roots", "root_uuid"),
                ("folders", "folder_uuid"),
                ("files", "asset_uuid"),
                ("tags", "tag_uuid"),
            ):
                if not table_has_column(conn, table, column):
                    fail(f"missing synchronized identity column {table}.{column}")

            expect_integrity_error(
                conn,
                "INSERT INTO sync_actor_epochs(actor_epoch,device_uuid,stream_uuid,stream_kind,state,created_at) VALUES (randomblob(15),randomblob(16),randomblob(16),'catalog','active',1)",
            )
            expect_integrity_error(
                conn,
                "INSERT INTO sync_actor_epochs(actor_epoch,device_uuid,stream_uuid,stream_kind,state,created_at) VALUES (randomblob(16),randomblob(16),randomblob(16),'catalog','unknown',1)",
            )
            expect_integrity_error(
                conn,
                "INSERT INTO sync_segments(segment_uuid,stream_uuid,stream_kind,device_uuid,actor_epoch,first_seq,last_seq,event_count,wire_version,reducer_version,compression,decoded_bytes,payload_hash,spool_path,state,created_at) VALUES (randomblob(16),randomblob(16),'catalog',randomblob(16),randomblob(16),1,2,1,1,1,'none',1,randomblob(32),'x','sealed',1)",
            )
            expect_integrity_error(
                conn,
                "INSERT INTO sync_publication_intents(intent_uuid,asset_uuid,content_revision_uuid,local_mutation_generation,source_path,state,created_at,updated_at) VALUES (randomblob(16),randomblob(16),randomblob(16),1,'x','uploading',1,1)",
            )
            expect_integrity_error(
                conn,
                "INSERT INTO media_chunks(chunk_hash,byte_length,state,created_at,updated_at) VALUES (randomblob(31),1,'sealed',1,1)",
            )
            expect_integrity_error(
                conn,
                "INSERT INTO media_publication_jobs(intent_uuid,phase,created_at,updated_at) VALUES (randomblob(16),'unknown',1,1)",
            )
            expect_integrity_error(
                conn,
                "INSERT INTO media_verified_ranges(pack_hash,range_start,range_end,range_hash,local_relpath,verified_at) VALUES (randomblob(32),8,4,randomblob(32),'ranges/a',1)",
            )

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

            preview_blobs_columns = {
                row[1] for row in conn.execute("PRAGMA table_info(preview_blobs)").fetchall()
            }
            missing_preview_blobs_columns = (
                EXPECTED_PREVIEW_BLOBS_COLUMNS - preview_blobs_columns
            )
            if missing_preview_blobs_columns:
                fail(
                    f"missing preview blob columns: {sorted(missing_preview_blobs_columns)}"
                )
            # PRAGMA table_info returns columns in declaration order.
            preview_blobs_column_order = [
                row[1] for row in conn.execute("PRAGMA table_info(preview_blobs)").fetchall()
            ]
            if preview_blobs_column_order[-1] != "data":
                fail(
                    "preview_blobs.data must be the LAST declared column: a 1-3 MB "
                    "BLOB spills onto overflow pages, and SQLite reaches any column "
                    "declared after it by walking that row's whole overflow chain. "
                    "The LRU eviction pass reads byte_size for every cached row, so "
                    "moving data earlier costs ~1.3 ms per MB of cache (measured "
                    "1852 ms vs 2.6 ms at 800 rows / 1.2 GB) inside a write "
                    f"transaction. Declared order: {preview_blobs_column_order}"
                )

            preview_failures_columns = {
                row[1] for row in conn.execute("PRAGMA table_info(preview_failures)").fetchall()
            }
            missing_preview_failures_columns = (
                EXPECTED_PREVIEW_FAILURES_COLUMNS - preview_failures_columns
            )
            if missing_preview_failures_columns:
                fail(
                    "missing preview failure columns: "
                    f"{sorted(missing_preview_failures_columns)}"
                )

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
                "failure_kind",
                "updated_at",
            }
            missing_thumb_columns = required_thumb_columns - thumb_columns
            if missing_thumb_columns:
                fail(f"missing thumbnail columns: {sorted(missing_thumb_columns)}")

            if conn.execute(
                "SELECT 1 FROM sqlite_schema WHERE type='table' AND name='plugins'"
            ).fetchone():
                fail("legacy Catalog plugins table remains after migration 28")

            head_columns = {
                row[1]
                for row in conn.execute(
                    "PRAGMA table_info(notification_operation_heads)"
                ).fetchall()
            }
            required_head_columns = {
                "operation_id",
                "kind",
                "status",
                "presentation",
                "revision",
                "payload_json",
                "dirty",
                "last_outbox_at",
                "updated_at",
                "completed_at",
            }
            missing_head_columns = required_head_columns - head_columns
            if missing_head_columns:
                fail(
                    "missing notification head columns: "
                    f"{sorted(missing_head_columns)}"
                )

            outbox_columns = {
                row[1]
                for row in conn.execute("PRAGMA table_info(notification_outbox)").fetchall()
            }
            required_outbox_columns = {
                "sequence",
                "operation_id",
                "revision",
                "payload_json",
                "created_at",
            }
            missing_outbox_columns = required_outbox_columns - outbox_columns
            if missing_outbox_columns:
                fail(
                    "missing notification outbox columns: "
                    f"{sorted(missing_outbox_columns)}"
                )

            file_operation_columns = {
                row[1]
                for row in conn.execute("PRAGMA table_info(file_operations)").fetchall()
            }
            required_file_operation_columns = {
                "operation_group_id",
                "entity_kind",
                "entity_id",
                "root_id",
                "target_folder_id",
                "attempt_count",
                "journal_phase",
                "source_identity",
                "target_parent_identity",
                "staging_identity",
                "recycle_phase",
                "error_code",
            }
            missing_file_operation_columns = (
                required_file_operation_columns - file_operation_columns
            )
            if missing_file_operation_columns:
                fail(
                    "missing file-operation journal columns: "
                    f"{sorted(missing_file_operation_columns)}"
                )

            expected_head_index = [("status", False), ("updated_at", True)]
            if (
                index_key_columns(conn, "idx_notification_heads_status_updated")
                != expected_head_index
            ):
                fail(
                    "idx_notification_heads_status_updated must index "
                    "status ASC, updated_at DESC"
                )
            expected_group_index = [("operation_group_id", False), ("id", False)]
            if (
                index_key_columns(conn, "idx_file_operations_group")
                != expected_group_index
            ):
                fail(
                    "idx_file_operations_group must index operation_group_id, id"
                )
            if index_key_columns(conn, "idx_file_operations_pending") != [
                ("id", False)
            ]:
                fail("idx_file_operations_pending must index id")
            pending_index_sql = conn.execute(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name='idx_file_operations_pending'"
            ).fetchone()[0]
            if not (
                "WHERE status = 'pending'" in pending_index_sql
                and "operation_group_id IS NOT NULL" in pending_index_sql
            ):
                fail(
                    "idx_file_operations_pending must include only grouped pending intents"
                )

            outbox_indexes = conn.execute(
                "PRAGMA index_list(notification_outbox)"
            ).fetchall()
            unique_autoindexes = [
                row[1] for row in outbox_indexes if row[2] == 1 and row[3] == "u"
            ]
            if len(unique_autoindexes) != 1:
                fail("notification_outbox must have one UNIQUE autoindex")
            expected_outbox_unique = [
                ("operation_id", False),
                ("revision", False),
            ]
            if (
                index_key_columns(conn, unique_autoindexes[0])
                != expected_outbox_unique
            ):
                fail(
                    "notification_outbox UNIQUE autoindex must cover "
                    "operation_id, revision"
                )
            matching_outbox_indexes = [
                row[1]
                for row in outbox_indexes
                if index_key_columns(conn, row[1]) == expected_outbox_unique
            ]
            if len(matching_outbox_indexes) != 1:
                fail(
                    "notification_outbox operation/revision key must use one canonical B-tree"
                )

            conn.execute(
                """
                INSERT INTO notification_operation_heads(
                    operation_id, kind, status, presentation, revision,
                    payload_json, updated_at
                )
                VALUES ('schema-check-operation', 'scan', 'pending', 'important', 1, '{}', 1)
                """
            )
            conn.execute(
                """
                INSERT INTO notification_outbox(
                    operation_id, revision, payload_json, created_at
                )
                VALUES ('schema-check-operation', 1, '{}', 1)
                """
            )
            first_sequence = conn.execute(
                "SELECT sequence FROM notification_outbox WHERE operation_id = 'schema-check-operation'"
            ).fetchone()[0]
            duplicate_outbox_rows = conn.execute(
                """
                INSERT OR IGNORE INTO notification_outbox(
                    operation_id, revision, payload_json, created_at
                )
                VALUES ('schema-check-operation', 1, '{}', 2)
                """
            ).rowcount
            if duplicate_outbox_rows != 0:
                fail("notification outbox must reject duplicate operation revisions")
            conn.execute("DELETE FROM notification_outbox")
            conn.execute(
                """
                INSERT INTO notification_outbox(
                    operation_id, revision, payload_json, created_at
                )
                VALUES ('schema-check-operation', 2, '{}', 3)
                """
            )
            second_sequence = conn.execute(
                "SELECT sequence FROM notification_outbox WHERE operation_id = 'schema-check-operation'"
            ).fetchone()[0]
            if second_sequence <= first_sequence:
                fail("notification outbox AUTOINCREMENT sequence must not reuse values")
            for invalid_status in ("paused", "unknown"):
                invalid_status_rows = conn.execute(
                    """
                    INSERT OR IGNORE INTO notification_operation_heads(
                        operation_id, kind, status, presentation, revision,
                        payload_json, updated_at
                    )
                    VALUES (?, 'scan', ?, 'important', 1, '{}', 1)
                    """,
                    (f"invalid-status-{invalid_status}", invalid_status),
                ).rowcount
                if invalid_status_rows != 0:
                    fail("notification head status must reject unsupported values")
            invalid_presentation_rows = conn.execute(
                """
                INSERT OR IGNORE INTO notification_operation_heads(
                    operation_id, kind, status, presentation, revision,
                    payload_json, updated_at
                )
                VALUES ('invalid-presentation', 'scan', 'pending', 'compact', 1, '{}', 1)
                """
            ).rowcount
            if invalid_presentation_rows != 0:
                fail("notification head presentation must reject unsupported values")
            invalid_dirty_rows = conn.execute(
                """
                INSERT OR IGNORE INTO notification_operation_heads(
                    operation_id, kind, status, presentation, revision,
                    payload_json, dirty, updated_at
                )
                VALUES ('invalid-dirty', 'scan', 'pending', 'important', 1, '{}', 2, 1)
                """
            ).rowcount
            if invalid_dirty_rows != 0:
                fail("notification head dirty must reject values other than 0 or 1")

            conn.execute(
                """
                INSERT INTO file_operations(
                    operation, source_path, target_path, status, created_at
                )
                VALUES ('rename', 'source', 'target', 'pending', 1)
                """
            )
            legacy_defaults = conn.execute(
                """SELECT operation_group_id, entity_kind, entity_id, root_id,
                          target_folder_id, attempt_count, journal_phase,
                          source_identity, target_parent_identity,
                          staging_identity, recycle_phase, error_code
                   FROM file_operations WHERE id = last_insert_rowid()"""
            ).fetchone()
            if legacy_defaults != (
                None,
                None,
                None,
                None,
                None,
                0,
                "legacy",
                None,
                None,
                None,
                "none",
                None,
            ):
                fail(
                    "existing file_operations inserts must preserve legacy defaults"
                )
            try:
                conn.execute(
                    """INSERT INTO file_operations(
                         operation, source_path, status, created_at,
                         operation_group_id, journal_phase
                       ) VALUES ('move', 'source', 'pending', 1,
                                 'invalid-phase', 'unknown')"""
                )
            except sqlite3.IntegrityError:
                pass
            else:
                fail("file_operations.journal_phase must reject unknown values")
            try:
                conn.execute(
                    """INSERT INTO file_operations(
                         operation, source_path, status, created_at, recycle_phase
                       ) VALUES ('delete_recycle', 'source', 'pending', 1,
                                 'unknown')"""
                )
            except sqlite3.IntegrityError:
                pass
            else:
                fail("file_operations.recycle_phase must reject unknown values")

            conn.execute(
                """
                INSERT INTO roots(path, display_name, enabled, created_at)
                VALUES (?, ?, 1, 1)
                """,
                ("D:/Pictures", "Pictures"),
            )
            root_id = conn.execute("SELECT id FROM roots").fetchone()[0]
            root_uuid = conn.execute(
                "SELECT root_uuid FROM roots WHERE id = ?", (root_id,)
            ).fetchone()[0]
            if not isinstance(root_uuid, bytes) or len(root_uuid) != 16:
                fail("legacy root insert did not receive a stable UUID")
            expect_integrity_error(
                conn, "UPDATE roots SET root_uuid = NULL WHERE id = ?", (root_id,)
            )
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
