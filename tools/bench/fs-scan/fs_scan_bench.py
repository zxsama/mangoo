#!/usr/bin/env python3
"""
Filesystem scan benchmark for Mangoo Phase 0.

Creates a synthetic Windows directory tree with many media-like files, then
measures directory traversal, file stat, media extension filtering, and batched
SQLite insert throughput.

This does not decode image/video content. It validates the "fast first pass"
that makes a newly added root browsable before thumbnail generation starts.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import statistics
import time
from pathlib import Path
from typing import Iterable


EXTS = [".jpg", ".png", ".webp", ".heic", ".mp4", ".mov", ".mkv", ".txt"]
MEDIA_EXTS = {".jpg", ".jpeg", ".png", ".webp", ".heic", ".mp4", ".mov", ".mkv"}


def now() -> float:
    return time.perf_counter()


def ms(seconds: float) -> float:
    return round(seconds * 1000, 3)


def connect(db_path: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(str(db_path))
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA synchronous=NORMAL")
    conn.execute("PRAGMA temp_store=MEMORY")
    conn.execute("PRAGMA cache_size=-131072")
    return conn


def create_schema(conn: sqlite3.Connection) -> None:
    conn.executescript(
        """
        DROP TABLE IF EXISTS roots;
        DROP TABLE IF EXISTS folders;
        DROP TABLE IF EXISTS files;

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
            rel_path TEXT NOT NULL,
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
            status TEXT NOT NULL
        );

        CREATE INDEX idx_folders_parent ON folders(root_id, parent_id, name);
        CREATE INDEX idx_files_folder_name ON files(folder_id, name, id);
        """
    )


def batched(items: list[tuple], size: int) -> Iterable[list[tuple]]:
    for idx in range(0, len(items), size):
        yield items[idx : idx + size]


def generate_tree(root: Path, files: int, folders: int, overwrite: bool) -> dict[str, object]:
    root.mkdir(parents=True, exist_ok=True)
    marker = root / ".mangoo_fs_scan_manifest.json"
    if marker.exists() and not overwrite:
        return json.loads(marker.read_text(encoding="utf-8"))

    start = now()
    folder_paths = []
    for folder_id in range(folders):
        group = folder_id // 100
        folder_path = root / f"group_{group:04d}" / f"folder_{folder_id:06d}"
        folder_path.mkdir(parents=True, exist_ok=True)
        folder_paths.append(folder_path)

    created = 0
    for file_id in range(files):
        folder_path = folder_paths[file_id % folders]
        ext = EXTS[file_id % len(EXTS)]
        file_path = folder_path / f"media_{file_id:09d}{ext}"
        if overwrite or not file_path.exists():
            file_path.touch()
        created += 1
        if created % 50_000 == 0:
            print(f"created/touched {created:,} / {files:,} files", flush=True)

    manifest = {
        "root": str(root),
        "files": files,
        "folders": folders,
        "elapsed_ms": ms(now() - start),
    }
    marker.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    return manifest


def scan_tree(root: Path, db_path: Path, batch_size: int) -> dict[str, object]:
    if db_path.exists():
        db_path.unlink()
    for suffix in ("-wal", "-shm"):
        candidate = Path(str(db_path) + suffix)
        if candidate.exists():
            candidate.unlink()

    conn = connect(db_path)
    create_schema(conn)
    base_time = int(time.time())
    with conn:
        conn.execute(
            "INSERT INTO roots(id, path, display_name, enabled, created_at, last_scan_at) VALUES (1, ?, ?, 1, ?, NULL)",
            (str(root), root.name, base_time),
        )

    folder_ids: dict[Path, int] = {root: 1}
    next_folder_id = 2
    next_file_id = 1
    folder_rows: list[tuple] = [(1, 1, None, root.name, ".", int(root.stat().st_mtime), "ok")]
    file_rows: list[tuple] = []
    scanned_dirs = 0
    scanned_files = 0
    media_files = 0

    traverse_start = now()
    stack = [root]
    while stack:
        current = stack.pop()
        scanned_dirs += 1
        parent_id = folder_ids[current]
        try:
            with os.scandir(current) as entries:
                for entry in entries:
                    try:
                        if entry.is_dir(follow_symlinks=False):
                            path = Path(entry.path)
                            folder_id = next_folder_id
                            next_folder_id += 1
                            folder_ids[path] = folder_id
                            stat = entry.stat(follow_symlinks=False)
                            rel_path = os.path.relpath(entry.path, root)
                            folder_rows.append(
                                (folder_id, 1, parent_id, entry.name, rel_path, int(stat.st_mtime), "ok")
                            )
                            stack.append(path)
                        elif entry.is_file(follow_symlinks=False):
                            scanned_files += 1
                            ext = Path(entry.name).suffix.lower()
                            if ext not in MEDIA_EXTS:
                                continue
                            stat = entry.stat(follow_symlinks=False)
                            file_rows.append(
                                (
                                    next_file_id,
                                    1,
                                    parent_id,
                                    entry.name,
                                    ext,
                                    stat.st_size,
                                    int(stat.st_mtime),
                                    int(stat.st_ctime),
                                    "ok",
                                )
                            )
                            next_file_id += 1
                            media_files += 1
                    except OSError:
                        continue
        except OSError:
            continue
    traverse_ms = ms(now() - traverse_start)

    insert_start = now()
    with conn:
        for rows in batched(folder_rows, batch_size):
            conn.executemany(
                "INSERT INTO folders(id, root_id, parent_id, name, rel_path, mtime, status) VALUES (?, ?, ?, ?, ?, ?, ?)",
                rows,
            )
        for rows in batched(file_rows, batch_size):
            conn.executemany(
                "INSERT INTO files(id, root_id, folder_id, name, ext, size, mtime, ctime, status) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rows,
            )
    conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    conn.close()
    insert_ms = ms(now() - insert_start)

    db_size = db_path.stat().st_size if db_path.exists() else 0
    return {
        "root": str(root),
        "db": str(db_path),
        "scanned_dirs": scanned_dirs,
        "scanned_files": scanned_files,
        "media_files": media_files,
        "folder_rows": len(folder_rows),
        "file_rows": len(file_rows),
        "traverse_ms": traverse_ms,
        "insert_ms": insert_ms,
        "total_ms": round(traverse_ms + insert_ms, 3),
        "files_per_second_traverse": round(scanned_files / (traverse_ms / 1000), 2) if traverse_ms else 0,
        "media_rows_per_second_insert": round(media_files / (insert_ms / 1000), 2) if insert_ms else 0,
        "db_size_mb": round(db_size / 1024 / 1024, 2),
    }


def scan_repeated(root: Path, db_path: Path, batch_size: int, runs: int) -> list[dict[str, object]]:
    results = []
    for idx in range(runs):
        result = scan_tree(root, db_path.with_name(f"{db_path.stem}_run{idx + 1}.sqlite"), batch_size)
        results.append(result)
    return results


def summarize(values: list[float]) -> dict[str, float]:
    if not values:
        return {"count": 0}
    ordered = sorted(values)
    return {
        "count": len(values),
        "min": min(values),
        "p50": statistics.median(values),
        "max": max(values),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path("tools/bench/fs-scan/results/tree"))
    parser.add_argument("--db", type=Path, default=Path("tools/bench/fs-scan/results/fs_scan.sqlite"))
    parser.add_argument("--report", type=Path, default=Path("tools/bench/fs-scan/results/fs_scan_report.json"))
    parser.add_argument("--files", type=int, default=100_000)
    parser.add_argument("--folders", type=int, default=1_000)
    parser.add_argument("--batch-size", type=int, default=10_000)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--overwrite", action="store_true")
    parser.add_argument("--generate-only", action="store_true")
    args = parser.parse_args()

    args.root = args.root.resolve()
    args.db = args.db.resolve()
    args.report = args.report.resolve()
    args.report.parent.mkdir(parents=True, exist_ok=True)

    generation = generate_tree(args.root, args.files, args.folders, args.overwrite)
    if args.generate_only:
        print(json.dumps({"generation": generation}, indent=2))
        return

    runs = scan_repeated(args.root, args.db, args.batch_size, args.runs)
    report = {
        "generation": generation,
        "runs": runs,
        "summary": {
            "traverse_ms": summarize([float(run["traverse_ms"]) for run in runs]),
            "insert_ms": summarize([float(run["insert_ms"]) for run in runs]),
            "total_ms": summarize([float(run["total_ms"]) for run in runs]),
            "files_per_second_traverse": summarize([float(run["files_per_second_traverse"]) for run in runs]),
        },
    }
    args.report.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()

