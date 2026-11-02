# Filesystem Scan Benchmark

Date: 2026-05-16

## Purpose

Validate the fast first-pass indexing path:

- traverse existing Windows/NTFS directories
- stat files
- filter media-like extensions
- batch insert folder/file rows into SQLite

This benchmark does not decode image/video content and does not generate thumbnails. It tests making an added root quickly browsable.

## Method

The benchmark generates synthetic directory trees with empty files and media-like extensions, then scans them using Python `os.scandir`.

Media extensions:

```text
.jpg .png .webp .heic .mp4 .mov .mkv
```

Non-media extension included for filtering:

```text
.txt
```

Each scan writes:

- `roots`
- `folders`
- `files`

SQLite settings:

- WAL
- synchronous NORMAL
- batched insert
- indexed folder/file browse keys

## 100k File Result

Dataset:

- Files: `100,000`
- Folders: `1,000`
- Scanned dirs: `1,011`
- Scanned files: `100,001`
- Media rows: `87,500`

Generation:

- Empty file tree creation: `25,058.685 ms`

Scan results across 3 runs:

| Metric | p50 |
| --- | ---: |
| Traverse | `280.155 ms` |
| SQLite insert | `128.219 ms` |
| Total scan + insert | `409.320 ms` |
| Traverse throughput | `356,948 files/sec` |
| DB size | `9.02 MB` |

## 500k File Result

Dataset:

- Files: `500,000`
- Folders: `5,000`
- Scanned dirs: `5,051`
- Scanned files: `500,001`
- Media rows: `437,500`

Generation:

- Empty file tree creation: `180,740.730 ms`

Scan results across 2 runs:

| Metric | p50 |
| --- | ---: |
| Traverse | `1,597.920 ms` |
| SQLite insert | `660.069 ms` |
| Total scan + insert | `2,257.989 ms` |
| Traverse throughput | `313,887 files/sec` |
| DB size | `45.45 MB` |

## Result

The fast first-pass indexing strategy is viable:

- Existing directory traversal and stat are fast on this NTFS test.
- SQLite insertion is not a bottleneck for file/folder rows.
- A root can become browsable before metadata extraction and thumbnails are complete.

Approximate extrapolation from the 500k run:

```text
1M files: 4-5s for first-pass scan + insert on this warmed local NTFS test
5M files: 22-25s for first-pass scan + insert on this warmed local NTFS test
```

This is only an estimate. Real user libraries may be slower due to:

- HDDs
- NAS/network drives
- antivirus
- deep paths
- permission errors
- symlinks/junctions
- large directory fanout
- cold filesystem cache

## Important Finding

Creating large numbers of files is much slower than scanning existing files.

In this benchmark:

- Creating/touching `500k` empty files took about `181s`.
- Scanning and inserting those files took about `2.26s`.

This reinforces that Mangoo should avoid bulk file creation in user media folders. Thumbnail/cache files must stay in Mangoo's own cache location.

## Implementation Rules

- First pass only collects path, folder, ext, size, mtime, ctime.
- Do not decode images/videos in the traversal loop.
- Do not hash file contents in the traversal loop.
- Do not generate thumbnails before making the root browsable.
- Batch SQLite writes.
- Handle permission errors and broken files as non-fatal.
- Treat file watcher events as hints; local rescan remains necessary.

## Artifacts

- Script: `tools/bench/fs-scan/fs_scan_bench.py`
- Archived raw results: `docs/performance-results/raw/2026-05-16/filesystem/`
- Generated filesystem trees and SQLite databases under `tools/bench/fs-scan/results/` were deleted after documentation.
