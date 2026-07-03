import sqlite3, sys, time

db = sys.argv[1]
N = int(sys.argv[2]) if len(sys.argv) > 2 else 1_000_000
base = 1_700_000_000
c = sqlite3.connect(db)
c.executescript("PRAGMA synchronous=OFF; PRAGMA journal_mode=MEMORY; PRAGMA temp_store=MEMORY; PRAGMA foreign_keys=OFF; PRAGMA cache_size=-524288;")

t0 = time.perf_counter()
c.execute("DELETE FROM tasks")
idx = c.execute("SELECT name,sql FROM sqlite_master WHERE type='index' AND sql IS NOT NULL AND tbl_name='tasks'").fetchall()
for name, _ in idx:
    c.execute(f'DROP INDEX IF EXISTS "{name}"')
BATCH = 100_000
done = 0
while done < N:
    end = min(N, done + BATCH)
    rows = [(i, 'thumbnail', -10, 'pending', None, None, i, base, base, 0, None, 0, 0, 0, 'fp', 1, None)
            for i in range(done + 1, end + 1)]
    with c:
        c.executemany(
            "INSERT INTO tasks(id,kind,priority,status,root_id,folder_id,file_id,created_at,updated_at,"
            "items_seen,items_total,folders_seen,media_files_seen,skipped_files,thumbnail_source_fingerprint,"
            "attempt_generation,error) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", rows)
    done = end
for name, sql in idx:
    c.execute(sql)
c.execute("ANALYZE")
c.execute("PRAGMA wal_checkpoint(TRUNCATE)")
c.commit()
print(f"seeded {c.execute('SELECT count(*) FROM tasks').fetchone()[0]:,} pending bulk thumbnail tasks in {time.perf_counter()-t0:.1f}s", flush=True)

# Re-open read-side with Core-matching pragmas for timing.
c.close()
c = sqlite3.connect(db)
c.executescript("PRAGMA synchronous=NORMAL; PRAGMA temp_store=MEMORY; PRAGMA cache_size=-65536; PRAGMA mmap_size=268435456;")

def timeit(label, sql, params=(), runs=5):
    c.execute(sql, params).fetchall()  # warm
    best = 1e9; n = 0
    for _ in range(runs):
        s = time.perf_counter(); r = c.execute(sql, params).fetchall(); best = min(best, time.perf_counter() - s); n = len(r)
    print(f"  {label}: {best*1000:.2f} ms (rows={n})", flush=True)

scope_ids = list(range(1, 301))  # ~300 visible/ahead file ids
inph = ",".join("?" * len(scope_ids))

print("=== priority-scope query (the scroll path) at 1M pending tasks ===")
timeit("OLD whole-root scan (pre-fix)",
       "SELECT tasks.id,tasks.file_id,tasks.priority,tasks.status FROM tasks JOIN files ON files.id=tasks.file_id "
       "WHERE tasks.kind='thumbnail' AND tasks.status IN ('pending','running') AND files.root_id=?", (1,))
timeit("NEW scoped (priority>0 OR file_id IN viewport)",
       "SELECT tasks.id,tasks.file_id,tasks.priority,tasks.status FROM tasks JOIN files ON files.id=tasks.file_id "
       "WHERE tasks.kind='thumbnail' AND tasks.status IN ('pending','running') AND files.root_id=? "
       f"AND (tasks.priority>0 OR tasks.file_id IN ({inph}))", tuple([1] + scope_ids))

print("=== worker scheduler scan per pass ===")
timeit("foreground pending (priority>0)",
       "SELECT tasks.id FROM tasks JOIN files ON files.id=tasks.file_id AND files.status='active' "
       "JOIN roots ON roots.id=files.root_id AND roots.enabled=1 "
       "WHERE tasks.kind='thumbnail' AND tasks.status='pending' AND tasks.priority>? "
       "ORDER BY tasks.priority DESC, tasks.attempt_generation DESC, tasks.updated_at DESC, tasks.id DESC", (0,))

print("=== per-file cache-stats count (seen-persist path) ===")
timeit("count_active_bulk for 128 file_ids",
       "SELECT COUNT(*) FROM tasks WHERE kind='thumbnail' AND status IN ('pending','running') AND priority=-10 "
       f"AND file_id IN ({','.join('?'*128)})", tuple(range(1, 129)))

print("=== /api/tasks unbounded list_tasks (loads ALL tasks) ===")
timeit("list_tasks SELECT * ORDER BY id (1M rows)",
       "SELECT id,kind,priority,status,root_id,folder_id,file_id,created_at,updated_at,items_seen,items_total,"
       "folders_seen,media_files_seen,skipped_files,thumbnail_source_fingerprint,attempt_generation,error "
       "FROM tasks ORDER BY id ASC")
c.close()
