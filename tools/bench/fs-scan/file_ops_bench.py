#!/usr/bin/env python3
"""
File operation consistency benchmark for Mangoo Phase 0.

Creates a small sandbox root, indexes it in SQLite, then performs rename, move,
and soft-delete operations while logging every operation. This validates the
ordering rule: filesystem operation first, database update second.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sqlite3
import time
from pathlib import Path


def now_ms() -> int:
    return int(time.time() * 1000)


def connect(db: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(str(db))
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA synchronous=NORMAL")
    return conn


def create_schema(conn: sqlite3.Connection) -> None:
    conn.executescript(
        """
        DROP TABLE IF EXISTS files;
        DROP TABLE IF EXISTS folders;
        DROP TABLE IF EXISTS file_operations;

        CREATE TABLE folders (
            id INTEGER PRIMARY KEY,
            rel_path TEXT NOT NULL UNIQUE
        );

        CREATE TABLE files (
            id INTEGER PRIMARY KEY,
            folder_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            status TEXT NOT NULL,
            UNIQUE(folder_id, name)
        );

        CREATE TABLE file_operations (
            id INTEGER PRIMARY KEY,
            operation TEXT NOT NULL,
            file_id INTEGER,
            source_path TEXT NOT NULL,
            target_path TEXT,
            status TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            error TEXT
        );
        """
    )


def setup_root(root: Path, count: int, overwrite: bool) -> None:
    if root.exists() and overwrite:
        shutil.rmtree(root)
    (root / "inbox").mkdir(parents=True, exist_ok=True)
    (root / "sorted").mkdir(parents=True, exist_ok=True)
    (root / ".trash").mkdir(parents=True, exist_ok=True)
    for idx in range(count):
        path = root / "inbox" / f"file_{idx:05d}.jpg"
        if not path.exists():
            path.write_bytes(f"sample {idx}\n".encode("utf-8"))


def index_root(conn: sqlite3.Connection, root: Path) -> None:
    folders = [(".", 1), ("inbox", 2), ("sorted", 3), (".trash", 4)]
    with conn:
        for rel, folder_id in folders:
            conn.execute("INSERT INTO folders(id, rel_path) VALUES (?, ?)", (folder_id, rel))
        file_id = 1
        for file in sorted((root / "inbox").glob("*.jpg")):
            conn.execute(
                "INSERT INTO files(id, folder_id, name, status) VALUES (?, 2, ?, 'ok')",
                (file_id, file.name),
            )
            file_id += 1


def log_op(conn: sqlite3.Connection, operation: str, file_id: int | None, source: Path, target: Path | None, status: str, error: str | None = None) -> None:
    conn.execute(
        """
        INSERT INTO file_operations(operation, file_id, source_path, target_path, status, created_at, error)
        VALUES (?, ?, ?, ?, ?, ?, ?)
        """,
        (operation, file_id, str(source), str(target) if target else None, status, now_ms(), error),
    )


def rename_file(conn: sqlite3.Connection, root: Path, file_id: int, new_name: str) -> bool:
    row = conn.execute(
        "SELECT f.name, d.rel_path FROM files f JOIN folders d ON d.id = f.folder_id WHERE f.id = ?",
        (file_id,),
    ).fetchone()
    source = root / row[1] / row[0]
    target = root / row[1] / new_name
    try:
        if target.exists():
            raise FileExistsError(f"target exists: {target}")
        os.replace(source, target)
        with conn:
            conn.execute("UPDATE files SET name = ? WHERE id = ?", (new_name, file_id))
            log_op(conn, "rename", file_id, source, target, "ok")
        return True
    except Exception as exc:
        with conn:
            log_op(conn, "rename", file_id, source, target, "failed", str(exc))
        return False


def move_file(conn: sqlite3.Connection, root: Path, file_id: int, target_folder_id: int) -> bool:
    row = conn.execute(
        "SELECT f.name, f.folder_id, d.rel_path FROM files f JOIN folders d ON d.id = f.folder_id WHERE f.id = ?",
        (file_id,),
    ).fetchone()
    target_folder = conn.execute("SELECT rel_path FROM folders WHERE id = ?", (target_folder_id,)).fetchone()[0]
    source = root / row[2] / row[0]
    target = root / target_folder / row[0]
    try:
        if target.exists():
            raise FileExistsError(f"target exists: {target}")
        os.replace(source, target)
        with conn:
            conn.execute("UPDATE files SET folder_id = ? WHERE id = ?", (target_folder_id, file_id))
            log_op(conn, "move", file_id, source, target, "ok")
        return True
    except Exception as exc:
        with conn:
            log_op(conn, "move", file_id, source, target, "failed", str(exc))
        return False


def soft_delete_file(conn: sqlite3.Connection, root: Path, file_id: int) -> bool:
    row = conn.execute(
        "SELECT f.name, d.rel_path FROM files f JOIN folders d ON d.id = f.folder_id WHERE f.id = ?",
        (file_id,),
    ).fetchone()
    source = root / row[1] / row[0]
    target = root / ".trash" / f"{file_id}_{row[0]}"
    try:
        if target.exists():
            raise FileExistsError(f"target exists: {target}")
        os.replace(source, target)
        with conn:
            conn.execute("UPDATE files SET folder_id = 4, name = ?, status = 'deleted' WHERE id = ?", (target.name, file_id))
            log_op(conn, "delete", file_id, source, target, "ok")
        return True
    except Exception as exc:
        with conn:
            log_op(conn, "delete", file_id, source, target, "failed", str(exc))
        return False


def verify(conn: sqlite3.Connection, root: Path) -> dict[str, object]:
    mismatches = []
    rows = conn.execute(
        "SELECT f.id, f.name, f.status, d.rel_path FROM files f JOIN folders d ON d.id = f.folder_id"
    ).fetchall()
    for file_id, name, status, rel_path in rows:
        path = root / rel_path / name
        if not path.exists():
            mismatches.append({"file_id": file_id, "path": str(path), "status": status})
    op_counts = dict(conn.execute("SELECT operation || ':' || status, count(*) FROM file_operations GROUP BY operation, status").fetchall())
    return {
        "files": len(rows),
        "mismatches": mismatches,
        "mismatch_count": len(mismatches),
        "operation_counts": op_counts,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path("tools/bench/fs-scan/results/file_ops_root"))
    parser.add_argument("--db", type=Path, default=Path("tools/bench/fs-scan/results/file_ops.sqlite"))
    parser.add_argument("--report", type=Path, default=Path("tools/bench/fs-scan/results/file_ops_report.json"))
    parser.add_argument("--files", type=int, default=1000)
    parser.add_argument("--overwrite", action="store_true")
    args = parser.parse_args()

    args.root = args.root.resolve()
    args.db = args.db.resolve()
    args.report = args.report.resolve()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    for suffix in ("", "-wal", "-shm"):
        candidate = Path(str(args.db) + suffix)
        if candidate.exists():
            candidate.unlink()

    setup_root(args.root, args.files, args.overwrite)
    conn = connect(args.db)
    create_schema(conn)
    index_root(conn, args.root)

    started = time.perf_counter()
    results = {"rename": 0, "move": 0, "delete": 0, "failed_conflict": 0}
    for file_id in range(1, 301):
        if rename_file(conn, args.root, file_id, f"renamed_{file_id:05d}.jpg"):
            results["rename"] += 1
    for file_id in range(301, 701):
        if move_file(conn, args.root, file_id, 3):
            results["move"] += 1
    for file_id in range(701, 901):
        if soft_delete_file(conn, args.root, file_id):
            results["delete"] += 1

    # Intentional failure: rename onto an existing target must not overwrite.
    if not rename_file(conn, args.root, 1, "renamed_00002.jpg"):
        results["failed_conflict"] += 1

    elapsed_ms = round((time.perf_counter() - started) * 1000, 3)
    verification = verify(conn, args.root)
    conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    conn.close()
    report = {
        "files": args.files,
        "elapsed_ms": elapsed_ms,
        "ops_per_second": round((sum(results.values()) / elapsed_ms) * 1000, 2),
        "results": results,
        "verification": verification,
    }
    args.report.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
