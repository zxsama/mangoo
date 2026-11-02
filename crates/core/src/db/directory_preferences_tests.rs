use super::directory_preferences::{
    folder_children_sql, page_folder_counts_sql, DirectorySortOrder,
};
use super::*;

mod write_contention {
    use super::*;
    use std::cell::RefCell;
    use std::sync::mpsc;
    use std::time::Duration;

    enum Event {
        WaitingForReservation,
        Finished(anyhow::Result<()>),
    }

    struct Gate {
        events: mpsc::Sender<Event>,
        resume: mpsc::Receiver<()>,
    }

    thread_local! {
        static GATE: RefCell<Option<Gate>> = const { RefCell::new(None) };
    }

    fn wait_for_competing_commit(_attempt: i32) -> bool {
        GATE.with(|slot| {
            let Some(gate) = slot.borrow_mut().take() else {
                return false;
            };
            if gate.events.send(Event::WaitingForReservation).is_err() {
                return false;
            }
            gate.resume.recv_timeout(Duration::from_secs(5)).is_ok()
        })
    }

    /// Release the competing writer only after SQLite asks to acquire the
    /// mutation's reservation. A deferred read->write upgrade instead returns
    /// SQLITE_BUSY immediately, which produces Finished(Err) before this gate.
    pub(super) fn run(
        path: &Path,
        operation: impl FnOnce(&Database) -> anyhow::Result<()> + Send + 'static,
    ) {
        let competing = Database::open(path).unwrap();
        let database = Database::open(path).unwrap();
        database
            .connection
            .busy_handler(Some(wait_for_competing_commit))
            .unwrap();
        let transaction =
            Transaction::new_unchecked(&competing.connection, TransactionBehavior::Immediate)
                .unwrap();
        transaction
            .execute("UPDATE roots SET display_name=display_name", [])
            .unwrap();
        let (events, received) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            GATE.with(|slot| {
                *slot.borrow_mut() = Some(Gate {
                    events: events.clone(),
                    resume: resumed,
                })
            });
            let result = operation(&database);
            GATE.with(|slot| slot.borrow_mut().take());
            events.send(Event::Finished(result)).unwrap();
        });
        match received.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::WaitingForReservation => {
                transaction.commit().unwrap();
                resume.send(()).unwrap();
                match received.recv_timeout(Duration::from_secs(5)).unwrap() {
                    Event::Finished(result) => result.unwrap(),
                    Event::WaitingForReservation => panic!("unexpected second writer gate"),
                }
                worker.join().unwrap();
            }
            Event::Finished(result) => {
                transaction.rollback().unwrap();
                worker.join().unwrap();
                panic!("mutation did not wait before reading its snapshot: {result:?}");
            }
        }
    }
}

#[test]
fn directory_preferences_reserve_writer_before_reading_competing_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("catalog.sqlite");
    let db = Database::open(&path).unwrap();
    db.apply_migrations().unwrap();
    let root = db
        .add_root(NewRoot {
            path: "contention-fixture".into(),
            display_name: "Fixture".into(),
        })
        .unwrap();
    let sentinel = folder(&db, root, None, "");
    let child = folder(&db, root, Some(sentinel), "child");
    drop(db);
    write_contention::run(&path, move |db| {
        db.update_directory_sort(
            DirectoryTarget::Root(root),
            rule(DirectorySortOrder::NameDesc, true),
        )?
        .context("root sort missing")?;
        Ok(())
    });
    write_contention::run(&path, move |db| {
        db.update_directory_sort(
            DirectoryTarget::Folder(child),
            rule(DirectorySortOrder::NameAsc, false),
        )?
        .context("folder sort missing")?;
        Ok(())
    });
    write_contention::run(&path, move |db| {
        db.update_directory_sort(DirectoryTarget::Folder(child), None)?
            .context("folder reset missing")?;
        Ok(())
    });
    write_contention::run(&path, move |db| {
        db.update_directory_sort(DirectoryTarget::Root(root), None)?
            .context("root reset missing")?;
        Ok(())
    });
    write_contention::run(&path, move |db| {
        db.update_root_appearance(root, "archive", "#abc")?
            .context("root appearance missing")?;
        Ok(())
    });
    let db = Database::open(&path).unwrap();
    assert!(db
        .read_directory_sort_state(DirectoryTarget::Root(root))
        .unwrap()
        .unwrap()
        .explicit_rule
        .is_none());
    assert!(db
        .read_directory_sort_state(DirectoryTarget::Folder(child))
        .unwrap()
        .unwrap()
        .explicit_rule
        .is_none());
    assert_eq!(
        db.get_root(root).unwrap().unwrap().icon.as_deref(),
        Some("archive")
    );
}

fn rule(order: DirectorySortOrder, recursive: bool) -> Option<DirectorySortRule> {
    Some(DirectorySortRule { order, recursive })
}

fn folder(db: &Database, root_id: i64, parent_id: Option<i64>, name: &str) -> i64 {
    db.upsert_folder(FolderUpsert {
        root_id,
        parent_id,
        name: name.into(),
        path_hash: format!("{root_id}/{parent_id:?}/{name}"),
        mtime: None,
    })
    .unwrap()
}

#[test]
fn directory_preferences_keyset_pages_bind_parent_order_revision_and_mode() {
    let db = wide_fixture(1205);
    let mut ascending = Vec::new();
    let mut cursor = None;
    loop {
        let page = db.list_folder_children_page(1, 500, cursor, false).unwrap();
        ascending.extend(page.items.iter().map(|f| f.id));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(ascending, (2..=1206).collect::<Vec<_>>());
    let first = db.list_folder_children_page(1, 500, None, false).unwrap();
    let old_cursor = first.next_cursor.unwrap();
    assert!(db
        .list_folder_children_page(2, 500, Some(old_cursor.clone()), false)
        .is_err());
    assert!(db
        .list_folder_children_page(1, 500, Some(old_cursor.clone()), true)
        .is_err());
    let result = db
        .update_directory_sort(
            DirectoryTarget::Root(1),
            rule(DirectorySortOrder::NameDesc, true),
        )
        .unwrap()
        .unwrap();
    assert!(result.invalidate_descendants);
    assert!(db
        .list_folder_children_page(1, 500, Some(old_cursor.clone()), false)
        .is_err());
    let mut descending = Vec::new();
    let mut cursor = None;
    loop {
        let page = db.list_folder_children_page(1, 500, cursor, false).unwrap();
        descending.extend(page.items.iter().map(|f| f.id));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    ascending.reverse();
    assert_eq!(descending, ascending);
    db.update_directory_sort(DirectoryTarget::Root(1), None)
        .unwrap();
    assert!(
        db.list_folder_children_page(1, 500, Some(old_cursor), false)
            .is_err(),
        "reset must not resurrect pre-change cursors"
    );

    // Duplicate names are legal in different parents; recursive page tie order is deterministic.
    for parent in [2, 3] {
        for name in ["same", "a", "A", "中", "\u{e000}", "\u{10000}"] {
            folder(&db, 1, Some(parent), name);
        }
    }
    let all = db.list_folder_children_page(1, 500, None, true).unwrap();
    let mut expected: Vec<_> = all.items.iter().map(|f| (f.name.clone(), f.id)).collect();
    let actual = expected.clone();
    expected.sort();
    assert_eq!(actual, expected);
    let mut pages = Vec::new();
    let mut cursor = None;
    loop {
        let page = db.list_folder_children_page(1, 73, cursor, true).unwrap();
        pages.extend(page.items.iter().map(|f| (f.name.clone(), f.id)));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(pages.len(), 1217);
    let mut sorted = pages.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(pages, sorted);
    db.update_directory_sort(
        DirectoryTarget::Root(1),
        rule(DirectorySortOrder::NameDesc, false),
    )
    .unwrap();
    let mut reverse_pages = Vec::new();
    let mut cursor = None;
    loop {
        let page = db.list_folder_children_page(1, 73, cursor, true).unwrap();
        reverse_pages.extend(page.items.iter().map(|f| (f.name.clone(), f.id)));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    pages.reverse();
    assert_eq!(reverse_pages, pages);
}

#[test]
fn directory_preferences_inherit_skip_nonrecursive_override_and_track_moves() {
    let db = wide_fixture(3);
    let child = folder(&db, 1, Some(2), "child");
    let deep = folder(&db, 1, Some(child), "deep");
    let state = |id| {
        db.directory_sort_state(DirectoryTarget::Folder(id))
            .unwrap()
            .unwrap()
    };
    db.update_directory_sort(
        DirectoryTarget::Root(1),
        rule(DirectorySortOrder::NameDesc, true),
    )
    .unwrap();
    assert_eq!(state(deep).effective_order, DirectorySortOrder::NameDesc);
    let before_deep = state(deep).revision;
    let unrelated = state(3).revision;
    let change = db
        .update_directory_sort(
            DirectoryTarget::Folder(2),
            rule(DirectorySortOrder::NameAsc, false),
        )
        .unwrap()
        .unwrap();
    assert!(!change.invalidate_descendants);
    assert_eq!(state(2).effective_order, DirectorySortOrder::NameAsc);
    assert_eq!(state(child).effective_order, DirectorySortOrder::NameDesc);
    assert_eq!(state(deep).revision, before_deep);
    assert_eq!(state(3).revision, unrelated);
    db.update_directory_sort(
        DirectoryTarget::Folder(child),
        rule(DirectorySortOrder::NameAsc, true),
    )
    .unwrap();
    assert_eq!(state(deep).effective_order, DirectorySortOrder::NameAsc);
    let new = folder(&db, 1, Some(deep), "new");
    assert_eq!(state(new).effective_order, DirectorySortOrder::NameAsc);
    db.update_directory_sort(DirectoryTarget::Folder(child), None)
        .unwrap();
    assert_eq!(state(new).effective_order, DirectorySortOrder::NameDesc);
    assert_ne!(state(deep).revision, before_deep);
    db.update_directory_sort(
        DirectoryTarget::Folder(3),
        rule(DirectorySortOrder::NameAsc, true),
    )
    .unwrap();
    let old = state(child).revision;
    db.connection
        .execute(
            "UPDATE folders SET parent_id=3,name='renamed' WHERE id=?1",
            [child],
        )
        .unwrap();
    assert_eq!(state(new).effective_order, DirectorySortOrder::NameAsc);
    assert_ne!(state(child).revision, old);
    // Explicit identity survives moving and renaming its row.
    db.update_directory_sort(
        DirectoryTarget::Folder(child),
        rule(DirectorySortOrder::NameDesc, false),
    )
    .unwrap();
    db.connection
        .execute(
            "UPDATE folders SET parent_id=4,name='renamed-again' WHERE id=?1",
            [child],
        )
        .unwrap();
    assert_eq!(
        state(child).explicit_rule.unwrap().order,
        DirectorySortOrder::NameDesc
    );
    assert_eq!(state(deep).effective_order, DirectorySortOrder::NameDesc);
}

#[test]
fn directory_preferences_reject_inactive_or_broken_ancestry_and_move_cursors() {
    let db = wide_fixture(3);
    let parent = folder(&db, 1, Some(2), "parent");
    folder(&db, 1, Some(parent), "a");
    folder(&db, 1, Some(parent), "b");
    let cursor = db
        .list_folder_children_page(parent, 1, None, false)
        .unwrap()
        .next_cursor
        .unwrap();
    db.connection
        .execute("UPDATE folders SET parent_id=3 WHERE id=?1", [parent])
        .unwrap();
    assert!(db
        .list_folder_children_page(parent, 1, Some(cursor), false)
        .is_err());
    db.connection
        .execute("UPDATE folders SET status='missing' WHERE id=3", [])
        .unwrap();
    assert!(db
        .read_directory_sort_state(DirectoryTarget::Folder(parent))
        .is_err());
    assert!(db
        .read_directory_sort_state(DirectoryTarget::Folder(3))
        .unwrap()
        .is_none());
    assert!(db
        .read_directory_sort_state(DirectoryTarget::Folder(i64::MAX))
        .unwrap()
        .is_none());
}

#[test]
fn directory_preferences_root_without_sentinel_reopens_and_guards_removal() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("catalog.sqlite");
    let db = Database::open(&path).unwrap();
    db.apply_migrations().unwrap();
    let root = db
        .add_root(NewRoot {
            path: temp.path().join("offline").to_string_lossy().into(),
            display_name: "offline".into(),
        })
        .unwrap();
    assert_eq!(db.get_root(root).unwrap().unwrap().root_folder_id, None);
    db.update_root_appearance(root, "archive", "#123AbC")
        .unwrap()
        .unwrap();
    db.update_directory_sort(
        DirectoryTarget::Root(root),
        rule(DirectorySortOrder::NameDesc, true),
    )
    .unwrap()
    .unwrap();
    drop(db);
    let db = Database::open(&path).unwrap();
    db.apply_migrations().unwrap();
    let saved = db.get_root(root).unwrap().unwrap();
    assert_eq!(saved.icon.as_deref(), Some("archive"));
    assert_eq!(saved.color.as_deref(), Some("#123AbC"));
    let sentinel = folder(&db, root, None, "");
    let child = folder(&db, root, Some(sentinel), "new-child");
    assert_eq!(
        db.directory_sort_state(DirectoryTarget::Folder(child))
            .unwrap()
            .unwrap()
            .effective_order,
        DirectorySortOrder::NameDesc
    );
    assert!(db
        .update_directory_sort(
            DirectoryTarget::Folder(sentinel),
            rule(DirectorySortOrder::NameAsc, false)
        )
        .unwrap()
        .is_none());
    db.connection
        .execute("UPDATE roots SET enabled=0 WHERE id=?1", [root])
        .unwrap();
    assert!(db
        .update_root_appearance(root, "folder", "#fff")
        .unwrap()
        .is_none());
    assert!(db
        .update_directory_sort(DirectoryTarget::Root(root), None)
        .unwrap()
        .is_none());
    db.connection
        .execute(
            "UPDATE roots SET enabled=1,pending_delete_at=1 WHERE id=?1",
            [root],
        )
        .unwrap();
    assert!(db
        .update_root_appearance(root, "folder", "#fff")
        .unwrap()
        .is_none());
    assert!(db
        .directory_sort_state(DirectoryTarget::Folder(child))
        .unwrap()
        .is_none());
}

#[test]
fn directory_preferences_page_counts_exact_and_queries_indexed() {
    let db = wide_fixture(10_000);
    let nested = folder(&db, 1, Some(2), "nested");
    folder(&db, 1, Some(3), "empty");
    db.connection.execute("INSERT INTO files(root_id,folder_id,name,ext,size,mtime,browse_visible) VALUES(1,?1,'hidden','',0,0,0)",[nested]).unwrap();
    db.connection.execute("INSERT INTO files(root_id,folder_id,name,ext,size,mtime,browse_visible,status) VALUES(1,?1,'gone','',0,0,1,'missing')",[nested]).unwrap();
    let page = db.list_folder_children_page(1, 2, None, false).unwrap();
    assert_eq!(
        (page.items[0].media_count, page.items[0].file_count),
        (1, 2)
    );
    assert_eq!(
        (page.items[1].media_count, page.items[1].file_count),
        (1, 1)
    );
    let descendant_page = db.list_folder_children_page(1, 500, None, true).unwrap();
    let empty = descendant_page
        .items
        .iter()
        .find(|f| f.name == "empty")
        .unwrap();
    assert_eq!((empty.media_count, empty.file_count), (0, 0));
    for order in [DirectorySortOrder::NameAsc, DirectorySortOrder::NameDesc] {
        let sql = format!(
            "EXPLAIN QUERY PLAN {}",
            folder_children_sql(order, true, false)
        );
        let plan = db
            .connection
            .prepare(&sql)
            .unwrap()
            .query_map(rusqlite::params![1, "folder005000", 5001, 201], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        println!("directory-keyset-plan {order:?}: {plan:?}");
        assert!(plan
            .iter()
            .any(|line| line.contains("idx_folders_parent_status_name_id")
                && (line.contains("name>") || line.contains("name<"))));
        assert!(!plan.iter().any(|line| line.contains("TEMP B-TREE")));
    }
    let sql = format!("EXPLAIN QUERY PLAN {}", page_folder_counts_sql(2));
    let plan = db
        .connection
        .prepare(&sql)
        .unwrap()
        .query_map([2, 3], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    println!("directory-count-plan: {plan:?}");
    assert!(plan
        .iter()
        .any(|line| line.contains("INDEX") && line.contains("folder_id=?")));
    let before = db.connection.total_changes();
    db.update_directory_sort(
        DirectoryTarget::Root(1),
        rule(DirectorySortOrder::NameDesc, true),
    )
    .unwrap();
    assert_eq!(
        db.connection.total_changes() - before,
        2,
        "one rule row plus one target version row regardless of width"
    );
    assert_eq!(
        db.connection
            .query_row("SELECT COUNT(*) FROM directory_sort_rules", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        1
    );
}

fn wide_fixture(count: i64) -> Database {
    let db = Database::open_in_memory().unwrap();
    db.apply_migrations().unwrap();
    db.connection
        .execute_batch(&format!(
            "BEGIN;
         INSERT INTO roots(id,path,display_name,created_at) VALUES(1,'benchmark','benchmark',0);
         INSERT INTO folders(id,root_id,parent_id,name,path_hash) VALUES(1,1,NULL,'','root');
         WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n < {count})
         INSERT INTO folders(id,root_id,parent_id,name,path_hash)
         SELECT n+1,1,1,printf('folder%06d',n),printf('hash%d',n) FROM seq;
         INSERT INTO files(root_id,folder_id,name,ext,size,mtime,browse_visible)
         SELECT 1,id,'item.dat','dat',1,0,1 FROM folders WHERE parent_id=1;
         COMMIT;"
        ))
        .unwrap();
    db
}

#[test]
#[ignore = "production path performance evidence; run explicitly with --ignored --nocapture"]
fn directory_preferences_production_benchmark() {
    use std::time::Instant;
    let label = std::env::var("MANGOO_DIRECTORY_BENCH_LABEL").unwrap_or_else(|_| "current".into());
    for width in [10_000, 50_000] {
        let db = wide_fixture(width);
        for iteration in 0..4 {
            let start = Instant::now();
            let page = db.list_folder_children_page(1, 200, None, false).unwrap();
            assert_eq!(page.items.len(), 200);
            assert!(page
                .items
                .iter()
                .all(|f| f.file_count == 1 && f.media_count == 1));
            println!(
                "directory-page label={label} width={width} iteration={iteration} first_us={}",
                start.elapsed().as_micros()
            );
            let start = Instant::now();
            let page = db
                .list_folder_children_page(1, 200, page.next_cursor, false)
                .unwrap();
            assert_eq!(page.items.len(), 200);
            println!(
                "directory-page label={label} width={width} iteration={iteration} next_us={}",
                start.elapsed().as_micros()
            );
        }
    }
    let db = wide_fixture(10_000);
    db.connection.execute_batch(
        "BEGIN;
         WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n < 256)
         INSERT INTO folders(id,root_id,parent_id,name,path_hash)
         SELECT n+20000,1,CASE n WHEN 1 THEN 2 ELSE n+19999 END,printf('deep%04d',n),printf('deep%d',n) FROM seq;
         INSERT INTO files(root_id,folder_id,name,ext,size,mtime,browse_visible)
         SELECT 1,id,'deep.dat','dat',1,0,1 FROM folders WHERE id>20000;
         COMMIT;"
    ).unwrap();
    for iteration in 0..4 {
        let start = Instant::now();
        let page = db.list_folder_children_page(1, 200, None, false).unwrap();
        assert_eq!(page.items[0].file_count, 257);
        println!(
            "directory-page label={label} width=10000 depth=256 iteration={iteration} first_us={}",
            start.elapsed().as_micros()
        );
    }
}
