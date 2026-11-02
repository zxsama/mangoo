#!/usr/bin/env python3
"""
SQLite WAL concurrency benchmark for Mangoo Phase 0.

Simulates foreground browse queries while a background worker writes thumbnail
records. This validates that background indexing/thumbnail updates do not block
hot read paths when using WAL and short write transactions.
"""

from __future__ import annotations

import argparse
import json
import queue
import sqlite3
import statistics
import threading
import time
from pathlib import Path
from typing import Any


READ_QUERIES = [
    (
        "folder_first_page",
        """
        SELECT f.id, f.name, f.size, f.mtime, m.width, m.height
        FROM files f
        JOIN media m ON m.file_id = f.id
        WHERE f.folder_id = ?
        ORDER BY f.name, f.id
        LIMIT 200
        """,
    ),
    (
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
    (
        "fts_tag_search",
        """
        SELECT rowid, name
        FROM file_search
        WHERE file_search MATCH 'tag7'
        LIMIT 200
        """,
    ),
]


def ms(seconds: float) -> float:
    return round(seconds * 1000, 3)


def connect(db: Path, timeout: float = 30.0) -> sqlite3.Connection:
    conn = sqlite3.connect(str(db), timeout=timeout, check_same_thread=False)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA synchronous=NORMAL")
    conn.execute("PRAGMA temp_store=MEMORY")
    conn.execute("PRAGMA cache_size=-131072")
    conn.execute("PRAGMA mmap_size=268435456")
    return conn


def read_worker(
    db: Path,
    worker_id: int,
    folder_count: int,
    stop: threading.Event,
    out: queue.Queue[tuple[str, float]],
) -> None:
    conn = connect(db)
    idx = 0
    while not stop.is_set():
        name, sql = READ_QUERIES[idx % len(READ_QUERIES)]
        folder_id = ((worker_id * 997 + idx * 17) % folder_count) + 1
        params: tuple[Any, ...] = (folder_id,) if "?" in sql else ()
        start = time.perf_counter()
        conn.execute(sql, params).fetchall()
        out.put((name, ms(time.perf_counter() - start)))
        idx += 1
    conn.close()


def write_worker(
    db: Path,
    total_writes: int,
    batch_size: int,
    stop: threading.Event,
    out: queue.Queue[tuple[str, float]],
) -> None:
    conn = connect(db)
    inserted = 0
    file_id = 1
    while inserted < total_writes:
        batch = []
        for _ in range(min(batch_size, total_writes - inserted)):
            batch.append(
                (
                    file_id,
                    "grid",
                    f"concurrency/{file_id:09d}.webp",
                    256,
                    256,
                    8192 + (file_id % 8192),
                    "ready",
                    1_700_000_000 + file_id,
                )
            )
            file_id += 1
        start = time.perf_counter()
        with conn:
            conn.executemany(
                """
                INSERT OR REPLACE INTO thumbs(file_id, profile, cache_key, width, height, byte_size, state, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                """,
                batch,
            )
        out.put(("write_batch", ms(time.perf_counter() - start)))
        inserted += len(batch)
    stop.set()
    conn.close()


def summarize(samples: list[float]) -> dict[str, float]:
    if not samples:
        return {"count": 0}
    sorted_samples = sorted(samples)
    def percentile(p: float) -> float:
        idx = min(len(sorted_samples) - 1, int(round((len(sorted_samples) - 1) * p)))
        return sorted_samples[idx]
    return {
        "count": len(samples),
        "min_ms": round(min(samples), 3),
        "p50_ms": round(statistics.median(samples), 3),
        "p95_ms": round(percentile(0.95), 3),
        "p99_ms": round(percentile(0.99), 3),
        "max_ms": round(max(samples), 3),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--report", type=Path, default=Path("bench-results/sqlite_concurrency.json"))
    parser.add_argument("--readers", type=int, default=4)
    parser.add_argument("--folders", type=int, default=10_000)
    parser.add_argument("--writes", type=int, default=100_000)
    parser.add_argument("--batch-size", type=int, default=1_000)
    args = parser.parse_args()

    args.report.parent.mkdir(parents=True, exist_ok=True)
    stop = threading.Event()
    events: queue.Queue[tuple[str, float]] = queue.Queue()
    threads = [
        threading.Thread(
            target=read_worker,
            args=(args.db, idx, args.folders, stop, events),
            daemon=True,
        )
        for idx in range(args.readers)
    ]
    writer = threading.Thread(
        target=write_worker,
        args=(args.db, args.writes, args.batch_size, stop, events),
        daemon=True,
    )

    start = time.perf_counter()
    for thread in threads:
        thread.start()
    writer.start()
    writer.join()
    for thread in threads:
        thread.join(timeout=5)
    elapsed = time.perf_counter() - start

    grouped: dict[str, list[float]] = {}
    while not events.empty():
        name, value = events.get()
        grouped.setdefault(name, []).append(value)

    result = {
        "db": str(args.db),
        "readers": args.readers,
        "writes": args.writes,
        "write_batch_size": args.batch_size,
        "elapsed_ms": ms(elapsed),
        "write_rows_per_second": round(args.writes / elapsed, 2),
        "summaries": {name: summarize(values) for name, values in sorted(grouped.items())},
    }
    args.report.write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
