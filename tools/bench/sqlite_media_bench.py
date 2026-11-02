#!/usr/bin/env python3
"""
SQLite media index benchmark for Mangoo Phase 0.

This script intentionally uses Python stdlib sqlite3 so it can run before the
Rust workspace exists. It validates schema shape, SQLite pragmas, bulk insert,
indexes, keyset pagination, and representative read queries.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import statistics
import time
from pathlib import Path
from typing import Any, Iterable


IMAGE_EXTS = [".jpg", ".png", ".webp", ".heic", ".tif", ".raw"]
VIDEO_EXTS = [".mp4", ".mov", ".mkv"]
ALL_EXTS = IMAGE_EXTS + VIDEO_EXTS


def now() -> float:
    return time.perf_counter()


def ms(seconds: float) -> float:
    return round(seconds * 1000, 3)


def connect(db_path: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(str(db_path))
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA synchronous=NORMAL")
    conn.execute("PRAGMA temp_store=MEMORY")
    conn.execute("PRAGMA cache_size=-262144")
    conn.execute("PRAGMA foreign_keys=OFF")
    conn.execute("PRAGMA mmap_size=268435456")
    return conn


def create_schema(conn: sqlite3.Connection, enable_fts: bool) -> None:
    conn.executescript(
        """
        DROP TABLE IF EXISTS roots;
        DROP TABLE IF EXISTS folders;
        DROP TABLE IF EXISTS files;
        DROP TABLE IF EXISTS media;
        DROP TABLE IF EXISTS user_metadata;
        DROP TABLE IF EXISTS tags;
        DROP TABLE IF EXISTS file_tags;
        DROP TABLE IF EXISTS thumbs;
        DROP TABLE IF EXISTS file_operations;

        CREATE TABLE roots (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            display_name TEXT NOT NULL,
            enabled INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            last_scan_at INTEGER
        );

        CREATE TABLE folders (
            id INTEGER PRIMARY KEY,
            root_id INTEGER NOT NULL,
            parent_id INTEGER,
            name TEXT NOT NULL,
            path_hash TEXT NOT NULL,
            mtime INTEGER NOT NULL,
            status TEXT NOT NULL
        );

        CREATE TABLE files (
            id INTEGER PRIMARY KEY,
            root_id INTEGER NOT NULL,
            folder_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            ext TEXT NOT NULL,
            size INTEGER NOT NULL,
            mtime INTEGER NOT NULL,
            ctime INTEGER NOT NULL,
            file_key TEXT NOT NULL,
            status TEXT NOT NULL
        );

        CREATE TABLE media (
            file_id INTEGER PRIMARY KEY,
            kind TEXT NOT NULL,
            width INTEGER,
            height INTEGER,
            duration_ms INTEGER,
            codec TEXT,
            orientation INTEGER,
            has_alpha INTEGER NOT NULL,
            dominant_color INTEGER,
            phash TEXT,
            metadata_status TEXT NOT NULL
        );

        CREATE TABLE user_metadata (
            file_id INTEGER PRIMARY KEY,
            rating INTEGER NOT NULL,
            favorite INTEGER NOT NULL,
            note TEXT,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE tags (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            color TEXT
        );

        CREATE TABLE file_tags (
            file_id INTEGER NOT NULL,
            tag_id INTEGER NOT NULL,
            PRIMARY KEY (file_id, tag_id)
        );

        CREATE TABLE thumbs (
            file_id INTEGER NOT NULL,
            profile TEXT NOT NULL,
            cache_key TEXT NOT NULL,
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            byte_size INTEGER NOT NULL,
            state TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (file_id, profile)
        );

        CREATE TABLE file_operations (
            id INTEGER PRIMARY KEY,
            operation TEXT NOT NULL,
            source_path TEXT NOT NULL,
            target_path TEXT,
            status TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            error TEXT
        );
        """
    )
    if enable_fts:
        conn.execute("DROP TABLE IF EXISTS file_search")
        conn.execute(
            """
            CREATE VIRTUAL TABLE file_search USING fts5(
                name,
                relative_path,
                tags
            )
            """
        )


def create_indexes(conn: sqlite3.Connection) -> None:
    conn.executescript(
        """
        CREATE INDEX idx_folders_parent ON folders(root_id, parent_id, name);
        CREATE INDEX idx_files_folder_name ON files(folder_id, name, id);
        CREATE INDEX idx_files_ext_mtime ON files(ext, mtime DESC, id DESC);
        CREATE INDEX idx_files_mtime ON files(mtime DESC, id DESC);
        CREATE INDEX idx_files_status ON files(status, id);
        CREATE INDEX idx_media_kind_file ON media(kind, file_id);
        CREATE INDEX idx_user_rating_file ON user_metadata(rating, file_id);
        CREATE INDEX idx_user_favorite_file ON user_metadata(favorite, file_id);
        """
    )


def batched_range(total: int, batch_size: int) -> Iterable[tuple[int, int]]:
    start = 1
    while start <= total:
        end = min(total, start + batch_size - 1)
        yield start, end
        start = end + 1


def insert_roots_and_folders(
    conn: sqlite3.Connection,
    folder_count: int,
    root_count: int,
    base_time: int,
) -> None:
    roots = [
        (idx, f"D:/MediaRoot{idx}", f"MediaRoot{idx}", 1, base_time, None)
        for idx in range(1, root_count + 1)
    ]
    conn.executemany(
        "INSERT INTO roots(id, path, display_name, enabled, created_at, last_scan_at) VALUES (?, ?, ?, ?, ?, ?)",
        roots,
    )

    folders = []
    for folder_id in range(1, folder_count + 1):
        root_id = ((folder_id - 1) % root_count) + 1
        parent_id = folder_id // 10 if folder_id > 10 else None
        folders.append(
            (
                folder_id,
                root_id,
                parent_id,
                f"folder_{folder_id:06d}",
                f"{folder_id:016x}",
                base_time + (folder_id % 100_000),
                "ok",
            )
        )
    conn.executemany(
        """
        INSERT INTO folders(id, root_id, parent_id, name, path_hash, mtime, status)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        """,
        folders,
    )


def build_file_rows(
    start: int,
    end: int,
    folder_count: int,
    root_count: int,
    base_time: int,
) -> tuple[list[tuple[Any, ...]], list[tuple[Any, ...]], list[tuple[Any, ...]], list[tuple[Any, ...]]]:
    files = []
    media = []
    user_metadata = []
    fts_rows = []

    for file_id in range(start, end + 1):
        folder_id = ((file_id - 1) % folder_count) + 1
        root_id = ((folder_id - 1) % root_count) + 1
        ext = ALL_EXTS[file_id % len(ALL_EXTS)]
        kind = "video" if ext in VIDEO_EXTS else "image"
        name = f"media_{file_id:09d}{ext}"
        mtime = base_time + (file_id % 5_000_000)
        size = 32_768 + ((file_id * 7919) % 80_000_000)
        width = 320 + ((file_id * 37) % 5680)
        height = 240 + ((file_id * 53) % 3760)
        duration = 1_000 + ((file_id * 17) % 900_000) if kind == "video" else None
        codec = "h264" if kind == "video" else None
        rating = file_id % 6
        favorite = 1 if file_id % 97 == 0 else 0
        tags = f"tag{file_id % 32} color{file_id % 12} {'favorite' if favorite else 'normal'}"

        files.append(
            (
                file_id,
                root_id,
                folder_id,
                name,
                ext,
                size,
                mtime,
                mtime - 60,
                f"{root_id}:{folder_id}:{file_id}",
                "ok",
            )
        )
        media.append(
            (
                file_id,
                kind,
                width,
                height,
                duration,
                codec,
                1,
                1 if ext == ".png" else 0,
                (file_id * 2654435761) & 0xFFFFFF,
                f"{file_id:016x}",
                "ready",
            )
        )
        user_metadata.append(
            (
                file_id,
                rating,
                favorite,
                None,
                mtime,
            )
        )
        fts_rows.append(
            (
                file_id,
                name,
                f"MediaRoot{root_id}/folder_{folder_id:06d}/{name}",
                tags,
            )
        )

    return files, media, user_metadata, fts_rows


def bulk_insert(
    conn: sqlite3.Connection,
    rows: int,
    folder_count: int,
    root_count: int,
    batch_size: int,
    enable_fts: bool,
) -> dict[str, float]:
    base_time = 1_700_000_000
    timings: dict[str, float] = {}

    start = now()
    with conn:
        insert_roots_and_folders(conn, folder_count, root_count, base_time)
    timings["roots_folders_insert_ms"] = ms(now() - start)

    start = now()
    inserted = 0
    for batch_start, batch_end in batched_range(rows, batch_size):
        files, media, user_metadata, fts_rows = build_file_rows(
            batch_start,
            batch_end,
            folder_count,
            root_count,
            base_time,
        )
        with conn:
            conn.executemany(
                """
                INSERT INTO files(id, root_id, folder_id, name, ext, size, mtime, ctime, file_key, status)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                files,
            )
            conn.executemany(
                """
                INSERT INTO media(file_id, kind, width, height, duration_ms, codec, orientation, has_alpha, dominant_color, phash, metadata_status)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                media,
            )
            conn.executemany(
                """
                INSERT INTO user_metadata(file_id, rating, favorite, note, updated_at)
                VALUES (?, ?, ?, ?, ?)
                """,
                user_metadata,
            )
            if enable_fts:
                conn.executemany(
                    """
                    INSERT INTO file_search(rowid, name, relative_path, tags)
                    VALUES (?, ?, ?, ?)
                    """,
                    fts_rows,
                )
        inserted += len(files)
        if inserted % max(batch_size * 10, 100_000) == 0:
            print(f"inserted {inserted:,} / {rows:,} rows", flush=True)

    timings["files_media_metadata_insert_ms"] = ms(now() - start)
    return timings


def timed_query(
    conn: sqlite3.Connection,
    name: str,
    sql: str,
    params: tuple[Any, ...] = (),
    runs: int = 9,
) -> dict[str, Any]:
    samples = []
    row_count = 0
    for idx in range(runs):
        start = now()
        rows = conn.execute(sql, params).fetchall()
        elapsed = ms(now() - start)
        if idx > 0:
            samples.append(elapsed)
        row_count = len(rows)
    return {
        "name": name,
        "rows": row_count,
        "min_ms": min(samples),
        "p50_ms": round(statistics.median(samples), 3),
        "max_ms": max(samples),
    }


def run_queries(conn: sqlite3.Connection, folder_count: int, enable_fts: bool) -> list[dict[str, Any]]:
    target_folder = max(1, folder_count // 2)
    first_page = conn.execute(
        """
        SELECT f.name, f.id
        FROM files f
        WHERE f.folder_id = ?
        ORDER BY f.name, f.id
        LIMIT 200
        """,
        (target_folder,),
    ).fetchall()
    cursor_name, cursor_id = first_page[-1] if first_page else ("", 0)

    queries = [
        timed_query(
            conn,
            "folder_first_page",
            """
            SELECT f.id, f.name, f.size, f.mtime, m.width, m.height
            FROM files f
            JOIN media m ON m.file_id = f.id
            WHERE f.folder_id = ?
            ORDER BY f.name, f.id
            LIMIT 200
            """,
            (target_folder,),
        ),
        timed_query(
            conn,
            "folder_keyset_next_page",
            """
            SELECT f.id, f.name, f.size, f.mtime, m.width, m.height
            FROM files f
            JOIN media m ON m.file_id = f.id
            WHERE f.folder_id = ?
              AND (f.name > ? OR (f.name = ? AND f.id > ?))
            ORDER BY f.name, f.id
            LIMIT 200
            """,
            (target_folder, cursor_name, cursor_name, cursor_id),
        ),
        timed_query(
            conn,
            "ext_filter_recent_jpg",
            """
            SELECT f.id, f.name, f.mtime, m.width, m.height
            FROM files f
            JOIN media m ON m.file_id = f.id
            WHERE f.ext = '.jpg'
            ORDER BY f.mtime DESC, f.id DESC
            LIMIT 200
            """,
        ),
        timed_query(
            conn,
            "rating_filter",
            """
            SELECT f.id, f.name, u.rating
            FROM user_metadata u
            JOIN files f ON f.id = u.file_id
            WHERE u.rating = 5
            ORDER BY u.file_id
            LIMIT 200
            """,
        ),
        timed_query(
            conn,
            "recent_images",
            """
            SELECT f.id, f.name, f.mtime,
                   (SELECT m.width FROM media m WHERE m.file_id = f.id),
                   (SELECT m.height FROM media m WHERE m.file_id = f.id)
            FROM files f
            WHERE EXISTS (
                SELECT 1
                FROM media m
                WHERE m.file_id = f.id
                  AND m.kind = 'image'
            )
            ORDER BY f.mtime DESC, f.id DESC
            LIMIT 200
            """,
        ),
    ]

    if enable_fts:
        queries.append(
            timed_query(
                conn,
                "fts_tag_search",
                """
                SELECT rowid, name
                FROM file_search
                WHERE file_search MATCH 'tag7'
                LIMIT 200
                """,
            )
        )

    return queries


def compact_size(path: Path) -> float:
    total = 0
    for suffix in ("", "-wal", "-shm"):
        candidate = Path(str(path) + suffix)
        if candidate.exists():
            total += candidate.stat().st_size
    return round(total / (1024 * 1024), 2)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rows", type=int, default=1_000_000)
    parser.add_argument("--folders", type=int, default=0)
    parser.add_argument("--roots", type=int, default=4)
    parser.add_argument("--batch-size", type=int, default=10_000)
    parser.add_argument("--db", type=Path, default=Path("bench-results/sqlite_media_bench.sqlite"))
    parser.add_argument("--report", type=Path, default=Path("bench-results/sqlite_media_bench.json"))
    parser.add_argument("--no-fts", action="store_true")
    parser.add_argument("--reuse-db", action="store_true")
    args = parser.parse_args()

    args.db.parent.mkdir(parents=True, exist_ok=True)
    args.report.parent.mkdir(parents=True, exist_ok=True)

    enable_fts = not args.no_fts
    folder_count = args.folders or max(100, args.rows // 500)

    if not args.reuse_db:
        for suffix in ("", "-wal", "-shm"):
            candidate = Path(str(args.db) + suffix)
            if candidate.exists():
                candidate.unlink()

    total_start = now()
    conn = connect(args.db)

    timings: dict[str, float] = {}
    if not args.reuse_db:
        start = now()
        create_schema(conn, enable_fts)
        timings["schema_create_ms"] = ms(now() - start)

        timings.update(
            bulk_insert(
                conn,
                rows=args.rows,
                folder_count=folder_count,
                root_count=args.roots,
                batch_size=args.batch_size,
                enable_fts=enable_fts,
            )
        )

        start = now()
        create_indexes(conn)
        timings["index_create_ms"] = ms(now() - start)

        start = now()
        conn.execute("ANALYZE")
        conn.execute("PRAGMA optimize")
        timings["analyze_optimize_ms"] = ms(now() - start)

    start = now()
    query_results = run_queries(conn, folder_count, enable_fts)
    timings["query_suite_ms"] = ms(now() - start)

    conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    conn.close()

    result = {
        "rows": args.rows,
        "folders": folder_count,
        "roots": args.roots,
        "batch_size": args.batch_size,
        "fts": enable_fts,
        "db": str(args.db),
        "db_size_mb": compact_size(args.db),
        "timings": timings,
        "queries": query_results,
        "total_ms": ms(now() - total_start),
    }

    args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")

    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
