import sqlite3, sys, time

db = sys.argv[1]
N = int(sys.argv[2]) if len(sys.argv) > 2 else 1_000_000
BIG_FOLDER_FILES = 100_000           # folder 2 holds a large single-folder page test
base = 1_700_000_000
t0 = time.perf_counter()
c = sqlite3.connect(db)
c.executescript("PRAGMA synchronous=OFF; PRAGMA journal_mode=MEMORY; PRAGMA temp_store=MEMORY; PRAGMA foreign_keys=OFF; PRAGMA cache_size=-524288;")

# Capture + drop user indexes on the hot tables (rebuild after bulk load).
idx = c.execute(
    "SELECT name, sql FROM sqlite_master WHERE type='index' AND sql IS NOT NULL "
    "AND tbl_name IN ('files','media','thumbs','tasks')"
).fetchall()
print("dropping", len(idx), "indexes for bulk load", flush=True)
for name, _ in idx:
    c.execute(f"DROP INDEX IF EXISTS \"{name}\"")

# Root + folders: folder 1 = root folder; 2 = big folder; 3..1002 = normal.
n_small_folders = 1000
total_folders = 2 + n_small_folders
c.execute("DELETE FROM roots"); c.execute("DELETE FROM folders")
c.execute("INSERT INTO roots(id,path,display_name,enabled,created_at,last_scan_at,active_scan_generation) VALUES (1,'D:/MillionLib','MillionLib',1,?,?,NULL)", (base, base))
folders = [(1, 1, None, '', 'roothash', base, 'active', base)]
for fid in range(2, total_folders + 1):
    folders.append((fid, 1, 1, f'folder_{fid:05d}', f'{fid:016x}', base, 'active', base))
c.executemany("INSERT INTO folders(id,root_id,parent_id,name,path_hash,mtime,status,scan_seen_at) VALUES (?,?,?,?,?,?,?,?)", folders)

def folder_for(i):
    # first BIG_FOLDER_FILES -> folder 2; rest spread across folders 3..1002
    if i <= BIG_FOLDER_FILES:
        return 2
    return 3 + ((i - BIG_FOLDER_FILES - 1) % n_small_folders)

BATCH = 50_000
inserted = 0
while inserted < N:
    files, media, thumbs = [], [], []
    end = min(N, inserted + BATCH)
    for i in range(inserted + 1, end + 1):
        fol = folder_for(i)
        mt = base + i
        files.append((i, 1, fol, f'media_{i:09d}.jpg', '.jpg', 40000 + (i % 5_000_000), mt, mt - 60, str(i), 'active', base))
        media.append((i, 'image', 1920, 1080, None, None, None, 0, None, None, 'ready', None, 'image/webp'))
        thumbs.append((i, 'grid_320', 'skipped_small', None, None, None, None, 320, 'image/webp', None, None, mt, None))
    with c:
        c.executemany("INSERT INTO files(id,root_id,folder_id,name,ext,size,mtime,ctime,file_key,status,scan_seen_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)", files)
        c.executemany("INSERT INTO media(file_id,kind,width,height,duration_ms,codec,orientation,has_alpha,dominant_color,phash,metadata_status,preview_placeholder,preview_placeholder_format) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)", media)
        c.executemany("INSERT INTO thumbs(file_id,profile,state,cache_key,width,height,byte_size,short_side_px,output_format,source_fingerprint,error,updated_at,served_by) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)", thumbs)
    inserted = end
    print(f"inserted {inserted:,}/{N:,} files ({time.perf_counter()-t0:.1f}s)", flush=True)

print("recreating indexes...", flush=True)
ti = time.perf_counter()
for name, sql in idx:
    c.execute(sql)
print(f"indexes rebuilt in {time.perf_counter()-ti:.1f}s", flush=True)
c.execute("ANALYZE")
c.execute("PRAGMA wal_checkpoint(TRUNCATE)")
c.commit()
print("counts:", {t: c.execute(f"SELECT count(*) FROM {t}").fetchone()[0] for t in ('roots','folders','files','media','thumbs')})
print(f"DONE in {time.perf_counter()-t0:.1f}s", flush=True)
c.close()
