#!/usr/bin/env python3
"""
SQLite page API serialization benchmark for Mangoo Phase 0.

Measures query + row mapping + JSON serialization for paged media responses.
This approximates the data path from Core Service to React virtual grid.
"""

from __future__ import annotations

import argparse
import json
import random
import sqlite3
import statistics
import time
from pathlib import Path


def connect(db: Path) -> sqlite3.Connection:
    conn = sqlite3.connect(str(db))
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA temp_store=MEMORY")
    conn.execute("PRAGMA cache_size=-131072")
    conn.execute("PRAGMA mmap_size=268435456")
    return conn


def ms(seconds: float) -> float:
    return round(seconds * 1000, 3)


def summarize(samples: list[float]) -> dict[str, float]:
    ordered = sorted(samples)
    def pct(p: float) -> float:
        idx = min(len(ordered) - 1, int(round((len(ordered) - 1) * p)))
        return ordered[idx]
    return {
        "count": len(samples),
        "min_ms": round(min(samples), 3),
        "p50_ms": round(statistics.median(samples), 3),
        "p95_ms": round(pct(0.95), 3),
        "p99_ms": round(pct(0.99), 3),
        "max_ms": round(max(samples), 3),
    }


def fetch_page(conn: sqlite3.Connection, folder_id: int, limit: int) -> dict[str, object]:
    rows = conn.execute(
        """
        SELECT f.id, f.name, f.ext, f.size, f.mtime,
               m.kind, m.width, m.height, m.duration_ms,
               u.rating, u.favorite
        FROM files f
        JOIN media m ON m.file_id = f.id
        JOIN user_metadata u ON u.file_id = f.id
        WHERE f.folder_id = ?
        ORDER BY f.name, f.id
        LIMIT ?
        """,
        (folder_id, limit),
    ).fetchall()
    items = [
        {
            "id": row["id"],
            "name": row["name"],
            "ext": row["ext"],
            "size": row["size"],
            "mtime": row["mtime"],
            "kind": row["kind"],
            "width": row["width"],
            "height": row["height"],
            "durationMs": row["duration_ms"],
            "rating": row["rating"],
            "favorite": bool(row["favorite"]),
            "thumb": f"/thumbs/{row['id']}/grid",
        }
        for row in rows
    ]
    next_cursor = None
    if rows:
        last = rows[-1]
        next_cursor = {"name": last["name"], "id": last["id"]}
    return {
        "folderId": folder_id,
        "limit": limit,
        "nextCursor": next_cursor,
        "items": items,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--report", type=Path, default=Path("bench-results/sqlite_api_page.json"))
    parser.add_argument("--folders", type=int, default=10_000)
    parser.add_argument("--iterations", type=int, default=2_000)
    parser.add_argument("--limit", type=int, default=200)
    parser.add_argument("--seed", type=int, default=42)
    args = parser.parse_args()

    args.report.parent.mkdir(parents=True, exist_ok=True)
    rng = random.Random(args.seed)
    conn = connect(args.db)

    query_map_json_samples: list[float] = []
    payload_sizes: list[int] = []
    for idx in range(args.iterations):
        folder_id = rng.randint(1, args.folders)
        start = time.perf_counter()
        page = fetch_page(conn, folder_id, args.limit)
        payload = json.dumps(page, separators=(",", ":"), ensure_ascii=False)
        query_map_json_samples.append(ms(time.perf_counter() - start))
        payload_sizes.append(len(payload.encode("utf-8")))
        if idx % 500 == 0 and idx:
            print(f"completed {idx:,} / {args.iterations:,} page serializations", flush=True)

    conn.close()
    result = {
        "db": str(args.db),
        "folders": args.folders,
        "iterations": args.iterations,
        "limit": args.limit,
        "query_map_json": summarize(query_map_json_samples),
        "payload_bytes": {
            "min": min(payload_sizes),
            "p50": int(statistics.median(payload_sizes)),
            "max": max(payload_sizes),
        },
    }
    args.report.write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
