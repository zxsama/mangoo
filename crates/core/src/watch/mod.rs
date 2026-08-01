use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self as std_mpsc, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use notify::event::{ModifyKind, RenameMode};
use notify::{recommended_watcher, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::db::{Database, DbWriter, FileUpsert, NotificationOrigin, RootRecord};
use crate::decode::formats::{lowercase_extension as extension, media_kind_for_extension};
use crate::tasks::TaskSender;

const EVENT_COALESCE_WINDOW: Duration = Duration::from_millis(100);
const ROOT_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

pub struct WatcherHandle {
    shutdown_sender: Option<std_mpsc::Sender<()>>,
    join_handle: Option<thread::JoinHandle<()>>,
}

pub fn start_watcher(
    database: Database,
    task_queue: TaskSender,
    writer: DbWriter,
) -> WatcherHandle {
    let (shutdown_sender, shutdown_receiver) = std_mpsc::channel();
    let (ready_sender, ready_receiver) = std_mpsc::channel();
    let join_handle = thread::spawn(move || {
        if let Err(error) = run_watcher(
            database,
            task_queue,
            writer,
            shutdown_receiver,
            ready_sender,
        ) {
            tracing::warn!(%error, "watcher stopped unexpectedly");
        }
    });
    let _ = ready_receiver.recv_timeout(Duration::from_secs(2));
    WatcherHandle {
        shutdown_sender: Some(shutdown_sender),
        join_handle: Some(join_handle),
    }
}

impl WatcherHandle {
    #[allow(dead_code)]
    pub fn thread_id(&self) -> thread::ThreadId {
        self.join_handle
            .as_ref()
            .expect("watcher join handle")
            .thread()
            .id()
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        if let Some(sender) = self.shutdown_sender.take() {
            let _ = sender.send(());
        }
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

struct WatcherState {
    database: Database,
    task_queue: TaskSender,
    // File upserts route through this single-writer actor so the watcher
    // never contends the WAL write lock with the scan/thumbnail writers.
    writer: DbWriter,
    watcher: RecommendedWatcher,
    watched_roots: HashMap<i64, PathBuf>,
    pending_events: Vec<notify::Result<Event>>,
}

fn run_watcher(
    database: Database,
    task_queue: TaskSender,
    writer: DbWriter,
    shutdown_receiver: std_mpsc::Receiver<()>,
    ready_sender: std_mpsc::Sender<()>,
) -> anyhow::Result<()> {
    let (event_tx, event_rx) = std_mpsc::channel();
    let watcher = recommended_watcher(move |result| {
        let _ = event_tx.send(result);
    })?;
    let mut state = WatcherState {
        database,
        task_queue,
        writer,
        watcher,
        watched_roots: HashMap::new(),
        pending_events: Vec::new(),
    };

    sync_enabled_roots(&mut state)?;
    let _ = ready_sender.send(());
    let mut last_refresh = Instant::now();

    loop {
        if shutdown_receiver.try_recv().is_ok() {
            break;
        }
        match event_rx.recv_timeout(EVENT_COALESCE_WINDOW) {
            Ok(result) => {
                state.pending_events.push(result);
                while let Ok(next) = event_rx.try_recv() {
                    state.pending_events.push(next);
                }
                flush_pending_events(&mut state)?;
            }
            Err(RecvTimeoutError::Timeout) => {
                if last_refresh.elapsed() >= ROOT_REFRESH_INTERVAL {
                    sync_enabled_roots(&mut state)?;
                    last_refresh = Instant::now();
                }
                if !state.pending_events.is_empty() {
                    flush_pending_events(&mut state)?;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

fn flush_pending_events(state: &mut WatcherState) -> anyhow::Result<()> {
    let events = std::mem::take(&mut state.pending_events);
    for result in events {
        match result {
            Ok(event) => {
                if let Err(error) = handle_event(state, event) {
                    tracing::warn!(%error, "watcher event handling failed; queueing rescan");
                    queue_all_watched_roots_for_rescan(state)?;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "watcher received error; queueing rescan");
                queue_all_watched_roots_for_rescan(state)?;
            }
        }
    }
    Ok(())
}

fn handle_event(state: &mut WatcherState, event: Event) -> anyhow::Result<()> {
    if event.need_rescan() {
        queue_all_watched_roots_for_rescan(state)?;
        return Ok(());
    }

    if event.paths.is_empty() {
        return Ok(());
    }

    if let EventKind::Modify(ModifyKind::Name(rename_mode)) = event.kind {
        match rename_mode {
            RenameMode::Both if event.paths.len() >= 2 => {
                let from = event.paths.first().cloned().expect("rename from path");
                let to = event.paths.last().cloned().expect("rename to path");
                handle_remove_path(state, &from)?;
                handle_create_path(state, &to)?;
                return Ok(());
            }
            RenameMode::From => {
                for path in event.paths {
                    handle_remove_path(state, &path)?;
                }
                return Ok(());
            }
            RenameMode::To => {
                for path in event.paths {
                    handle_create_path(state, &path)?;
                }
                return Ok(());
            }
            _ => {}
        }
    }

    for path in event.paths {
        match event.kind {
            EventKind::Create(_) => handle_create_path(state, &path)?,
            EventKind::Modify(_) => handle_modify_path(state, &path)?,
            EventKind::Remove(_) => handle_remove_path(state, &path)?,
            _ => {}
        }
    }

    Ok(())
}

fn handle_create_path(state: &mut WatcherState, path: &Path) -> anyhow::Result<()> {
    let Some(root) = root_for_path(state, path) else {
        return Ok(());
    };
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(()),
    };

    if metadata.is_dir() {
        queue_root_rescan(&state.database, &state.task_queue, root.id)?;
        Ok(())
    } else if metadata.is_file() {
        upsert_media_file(state, &root, path, &metadata)
    } else {
        Ok(())
    }
}

fn handle_modify_path(state: &mut WatcherState, path: &Path) -> anyhow::Result<()> {
    let Some(root) = root_for_path(state, path) else {
        return Ok(());
    };
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(()),
    };

    if metadata.is_dir() {
        let root_id = root.id;
        let root_path = PathBuf::from(&root.path);
        let dir_path = path.to_path_buf();
        crate::debug_log!("watcher folder-chain -> writer: {}", dir_path.display());
        let _ = state.writer.write_blocking(move |db| {
            db.ensure_folder_chain_for_path(root_id, &root_path, &dir_path)
        })?;
        return Ok(());
    }

    if metadata.is_file() {
        upsert_media_file(state, &root, path, &metadata)?;
    }
    Ok(())
}

fn handle_remove_path(state: &mut WatcherState, path: &Path) -> anyhow::Result<()> {
    let Some(root) = root_for_path(state, path) else {
        return Ok(());
    };
    if state
        .database
        .mark_file_missing_by_path(root.id, Path::new(&root.path), path)?
    {
        return Ok(());
    }
    let _ =
        state
            .database
            .mark_folder_subtree_missing_by_path(root.id, Path::new(&root.path), path)?;
    Ok(())
}

fn upsert_media_file(
    state: &mut WatcherState,
    root: &RootRecord,
    path: &Path,
    metadata: &fs::Metadata,
) -> anyhow::Result<()> {
    // One `lowercase_extension` allocation per event, not two: the guard and the
    // stored `files.ext` are the same string.
    let ext = extension(path);
    let Some(kind) = media_kind_for_extension(ext.as_str()) else {
        return Ok(());
    };
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let root_id = root.id;
    let root_path = PathBuf::from(&root.path);
    let parent_path = parent.to_path_buf();
    crate::debug_log!("watcher folder-chain -> writer: {}", parent_path.display());
    let folder_id = state.writer.write_blocking(move |db| {
        db.ensure_folder_chain_for_path(root_id, &root_path, &parent_path)
    })?;
    crate::debug_log!("watcher upsert -> writer: {}", path.display());
    let file_upsert = FileUpsert {
        root_id: root.id,
        folder_id,
        name: file_name(path),
        ext,
        size: metadata.len() as i64,
        mtime: metadata_time(Some(metadata)),
        ctime: metadata_created_time(Some(metadata)),
        file_key: None,
    };
    let kind_owned = kind.to_owned();
    state.writer.write_blocking(move |db| {
        let file_id = db.upsert_file(file_upsert)?;
        db.upsert_media_kind(file_id, &kind_owned)?;
        Ok(file_id)
    })?;
    Ok(())
}

fn sync_enabled_roots(state: &mut WatcherState) -> anyhow::Result<()> {
    let current_roots = state.database.list_roots()?;
    let current_by_id: HashMap<i64, RootRecord> = current_roots
        .into_iter()
        .map(|root| (root.id, root))
        .collect();

    let previous_ids: Vec<i64> = state.watched_roots.keys().copied().collect();
    for root_id in previous_ids {
        if !current_by_id.contains_key(&root_id) {
            if let Some(path) = state.watched_roots.remove(&root_id) {
                let _ = state.watcher.unwatch(&path);
            }
        }
    }

    for (root_id, root) in current_by_id {
        if state.watched_roots.contains_key(&root_id) {
            continue;
        }
        let path = PathBuf::from(&root.path);
        state.watcher.watch(&path, RecursiveMode::Recursive)?;
        state.watched_roots.insert(root_id, path);
    }

    Ok(())
}

fn root_for_path<'a>(state: &'a WatcherState, path: &Path) -> Option<RootRecord> {
    let mut best: Option<RootRecord> = None;
    for (root_id, root_path) in &state.watched_roots {
        if !path.starts_with(root_path) {
            continue;
        }
        let Ok(Some(root)) = state.database.get_root(*root_id) else {
            continue;
        };
        if !root.enabled {
            continue;
        }
        let replace = best
            .as_ref()
            .map(|existing| {
                let existing_path = PathBuf::from(&existing.path);
                root_path.components().count() > existing_path.components().count()
            })
            .unwrap_or(true);
        if replace {
            best = Some(root);
        }
    }
    best
}

fn queue_all_watched_roots_for_rescan(state: &mut WatcherState) -> anyhow::Result<()> {
    let root_ids: Vec<i64> = state.watched_roots.keys().copied().collect();
    for root_id in root_ids {
        queue_root_rescan(&state.database, &state.task_queue, root_id)?;
    }
    Ok(())
}

fn queue_root_rescan(
    database: &Database,
    task_queue: &TaskSender,
    root_id: i64,
) -> anyhow::Result<bool> {
    if !database.root_enabled(root_id)? {
        return Ok(false);
    }
    // Coalesce duplicate *pending* rescans only. A `running` scan must not
    // suppress a follow-up: directory creates or move-ins that happen after
    // WalkDir has already passed the affected path otherwise stay invisible
    // until the next manual scan.
    if database.has_pending_root_scan_task(root_id)? {
        return Ok(false);
    }
    let task_id =
        database.create_root_scan_task_with_origin(root_id, NotificationOrigin::Background)?;
    match task_queue.try_send(task_id) {
        Ok(()) => {}
        Err(error) => {
            tracing::warn!(
                task_id,
                root_id,
                %error,
                "root scan task persisted but watcher could not notify worker immediately"
            );
        }
    }
    Ok(true)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string()
}

fn metadata_time(metadata: Option<&fs::Metadata>) -> i64 {
    metadata
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn metadata_created_time(metadata: Option<&fs::Metadata>) -> Option<i64> {
    metadata
        .and_then(|metadata| metadata.created().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::db::{MediaPageQuery, NewRoot};
    use crate::scan::scan_root;
    use notify::event::Flag;

    #[test]
    fn added_media_under_watched_root_becomes_visible_without_restart() {
        let temp_root = unique_temp_dir("root");
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("seed.jpg"), b"seed").expect("seed image");

        let db_dir = unique_temp_dir("db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("watch.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_root(&database, &temp_root, "watch-root");
        let root = test_root(&database, root_id);
        scan_root(&mut database, &root).expect("seed scan");

        let watcher_db = Database::open(&db_path).expect("reopen watcher db");
        let writer_db = Database::open(&db_path).expect("reopen writer db");
        let (task_sender, _task_receiver) = tokio::sync::mpsc::channel(8);
        let _watcher = start_watcher(watcher_db, task_sender, DbWriter::spawn(writer_db));

        fs::write(temp_root.join("new.jpg"), b"new image").expect("write new image");
        wait_for_media_count(&db_path, root_id, 2, Duration::from_secs(2));

        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn deleted_media_disappears_after_watcher_processes_remove_event() {
        let temp_root = unique_temp_dir("root");
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("seed.jpg"), b"seed").expect("seed image");

        let db_dir = unique_temp_dir("db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("watch.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_root(&database, &temp_root, "watch-root");
        let root = test_root(&database, root_id);
        scan_root(&mut database, &root).expect("seed scan");

        let watcher_db = Database::open(&db_path).expect("reopen watcher db");
        let writer_db = Database::open(&db_path).expect("reopen writer db");
        let (task_sender, _task_receiver) = tokio::sync::mpsc::channel(8);
        let _watcher = start_watcher(watcher_db, task_sender, DbWriter::spawn(writer_db));

        fs::remove_file(temp_root.join("seed.jpg")).expect("delete image");
        wait_for_media_count(&db_path, root_id, 0, Duration::from_secs(2));

        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn rename_or_move_within_root_keeps_one_index_row() {
        let temp_root = unique_temp_dir("root");
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::write(temp_root.join("old.jpg"), b"seed").expect("seed image");

        let db_dir = unique_temp_dir("db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("watch.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_root(&database, &temp_root, "watch-root");
        let root = test_root(&database, root_id);
        scan_root(&mut database, &root).expect("seed scan");

        let watcher_db = Database::open(&db_path).expect("reopen watcher db");
        let writer_db = Database::open(&db_path).expect("reopen writer db");
        let (task_sender, _task_receiver) = tokio::sync::mpsc::channel(8);
        let _watcher = start_watcher(watcher_db, task_sender, DbWriter::spawn(writer_db));

        fs::rename(temp_root.join("old.jpg"), temp_root.join("new.jpg")).expect("rename image");
        wait_for_media_names(&db_path, root_id, &["new.jpg"], Duration::from_secs(2));

        let inspector = Database::open(&db_path).expect("open inspector db");
        let page = inspector
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].name, "new.jpg");

        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn non_media_creates_are_ignored() {
        let temp_root = unique_temp_dir("root");
        fs::create_dir_all(&temp_root).expect("create root dir");

        let db_dir = unique_temp_dir("db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("watch.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_root(&database, &temp_root, "watch-root");
        let root = test_root(&database, root_id);
        scan_root(&mut database, &root).expect("seed scan");

        let watcher_db = Database::open(&db_path).expect("reopen watcher db");
        let writer_db = Database::open(&db_path).expect("reopen writer db");
        let (task_sender, _task_receiver) = tokio::sync::mpsc::channel(8);
        let _watcher = start_watcher(watcher_db, task_sender, DbWriter::spawn(writer_db));

        fs::write(temp_root.join("notes.txt"), b"not media").expect("write text file");
        wait_for_media_count(&db_path, root_id, 0, Duration::from_secs(2));

        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn disabled_roots_are_ignored_by_watcher_refresh_and_events() {
        let temp_root = unique_temp_dir("root");
        fs::create_dir_all(&temp_root).expect("create root dir");

        let db_dir = unique_temp_dir("db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("watch.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_root(&database, &temp_root, "watch-root");
        database.disable_root(root_id).expect("disable root");

        let watcher_db = Database::open(&db_path).expect("reopen watcher db");
        let writer_db = Database::open(&db_path).expect("reopen writer db");
        let (task_sender, mut task_receiver) = tokio::sync::mpsc::channel(8);
        let _watcher = start_watcher(watcher_db, task_sender, DbWriter::spawn(writer_db));

        fs::write(temp_root.join("disabled.jpg"), b"seed").expect("write image");
        std::thread::sleep(Duration::from_millis(200));
        assert!(task_receiver.try_recv().is_err());

        let inspector = Database::open(&db_path).expect("open inspector db");
        let page = inspector
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert!(page.items.is_empty());

        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn overflow_falls_back_to_a_bounded_rescan_or_equivalent_safe_recovery() {
        let temp_root = unique_temp_dir("root");
        fs::create_dir_all(&temp_root).expect("create root dir");

        let db_dir = unique_temp_dir("db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("watch.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_root(&database, &temp_root, "watch-root");

        let (task_sender, mut task_receiver) = tokio::sync::mpsc::channel(8);
        let queued = queue_root_rescan(&database, &task_sender, root_id)
            .expect("queue root rescan for overflow");
        assert!(queued);
        let task_id = task_receiver
            .blocking_recv()
            .expect("expected queued rescan task");
        assert!(task_id > 0);

        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn root_rescan_queue_does_not_block_when_task_channel_is_full() {
        let temp_root = unique_temp_dir("root");
        fs::create_dir_all(&temp_root).expect("create root dir");

        let db_dir = unique_temp_dir("db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("watch.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_root(&database, &temp_root, "watch-root");

        let (task_sender, mut task_receiver) = tokio::sync::mpsc::channel(1);
        task_sender.try_send(12345).expect("fill task channel");
        let queued = queue_root_rescan(&database, &task_sender, root_id)
            .expect("full queue should not fail rescan persistence");
        assert!(queued);
        assert!(database
            .has_active_root_scan_task(root_id)
            .expect("active root scan"));
        assert_eq!(
            database
                .list_pending_root_scan_task_ids()
                .expect("pending root scans")
                .len(),
            1
        );
        assert_eq!(task_receiver.try_recv().expect("dummy task id"), 12345);
        assert!(task_receiver.try_recv().is_err());

        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn overflow_recovery_ignores_root_disabled_before_refresh() {
        let temp_root = unique_temp_dir("root");
        fs::create_dir_all(&temp_root).expect("create root dir");

        let db_dir = unique_temp_dir("db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("watch.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_root(&database, &temp_root, "watch-root");
        let (event_tx, _event_rx) = std_mpsc::channel();
        let watcher = recommended_watcher(move |result| {
            let _ = event_tx.send(result);
        })
        .expect("create watcher");
        let (task_sender, mut task_receiver) = tokio::sync::mpsc::channel(8);
        let writer_db = Database::open(&db_path).expect("reopen writer db");
        let mut state = WatcherState {
            database,
            task_queue: task_sender,
            writer: DbWriter::spawn(writer_db),
            watcher,
            watched_roots: HashMap::from([(root_id, temp_root.clone())]),
            pending_events: Vec::new(),
        };

        state.database.disable_root(root_id).expect("disable root");
        queue_all_watched_roots_for_rescan(&mut state)
            .expect("disabled watched root should not fail overflow recovery");
        assert!(task_receiver.try_recv().is_err());

        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn notify_rescan_event_queues_coalesced_root_scan_tasks() {
        let temp_root = unique_temp_dir("root");
        fs::create_dir_all(&temp_root).expect("create root dir");

        let db_dir = unique_temp_dir("db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("watch.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_root(&database, &temp_root, "watch-root");
        let (task_sender, mut task_receiver) = tokio::sync::mpsc::channel(8);
        let mut state = test_watcher_state(database, task_sender, root_id, temp_root.clone());

        handle_event(
            &mut state,
            Event::new(EventKind::Other).set_flag(Flag::Rescan),
        )
        .expect("rescan event should queue scan");
        handle_event(
            &mut state,
            Event::new(EventKind::Other).set_flag(Flag::Rescan),
        )
        .expect("duplicate rescan event should coalesce");

        let first_task_id = task_receiver
            .blocking_recv()
            .expect("queued root scan task");
        assert!(first_task_id > 0);
        assert!(task_receiver.try_recv().is_err());
        let operation = state
            .database
            .notification_heads_for_test()
            .expect("read watcher notification")
            .into_iter()
            .find(|operation| operation.task_id == Some(first_task_id))
            .expect("watcher task notification");
        assert_eq!(operation.origin, "background");

        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn directory_create_during_running_scan_persists_followup_pending_rescan() {
        // Regression: a `running` root_scan must not suppress a follow-up
        // pending rescan when the watcher observes a new directory after
        // WalkDir already passed the affected path.
        let temp_root = unique_temp_dir("root");
        fs::create_dir_all(&temp_root).expect("create root dir");

        let db_dir = unique_temp_dir("db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("watch.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_root(&database, &temp_root, "watch-root");

        // Simulate an in-progress full scan: a root_scan task is currently
        // running for this root.
        let running_task_id = database
            .create_root_scan_task(root_id)
            .expect("create running root scan task");
        database
            .mark_task_running_current_attempt_for_test(running_task_id)
            .expect("mark task running");

        let (task_sender, mut task_receiver) = tokio::sync::mpsc::channel(8);
        let queued = queue_root_rescan(&database, &task_sender, root_id)
            .expect("queue root rescan while another scan is running");
        assert!(
            queued,
            "watcher should persist a follow-up rescan even when another scan is running"
        );

        let pending_ids = database
            .list_pending_root_scan_task_ids()
            .expect("list pending root scans");
        assert_eq!(
            pending_ids.len(),
            1,
            "exactly one follow-up pending rescan should exist"
        );
        let new_task_id = pending_ids[0];
        assert_ne!(new_task_id, running_task_id);

        let received_task_id = task_receiver
            .blocking_recv()
            .expect("queued follow-up rescan task");
        assert_eq!(received_task_id, new_task_id);

        // A second rescan request while the first is still pending should
        // coalesce, leaving exactly one pending follow-up.
        let coalesced = queue_root_rescan(&database, &task_sender, root_id)
            .expect("coalesce duplicate pending rescan");
        assert!(!coalesced, "duplicate pending rescan should coalesce");
        assert_eq!(
            database
                .list_pending_root_scan_task_ids()
                .expect("list pending root scans")
                .len(),
            1
        );

        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn single_path_rename_from_event_marks_old_media_missing() {
        let temp_root = unique_temp_dir("root");
        let outside_dir = unique_temp_dir("outside");
        fs::create_dir_all(&temp_root).expect("create root dir");
        fs::create_dir_all(&outside_dir).expect("create outside dir");
        let old_path = temp_root.join("old.jpg");
        fs::write(&old_path, b"old image").expect("write old image");

        let db_dir = unique_temp_dir("db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("watch.sqlite");
        let mut database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_root(&database, &temp_root, "watch-root");
        let root = test_root(&database, root_id);
        scan_root(&mut database, &root).expect("seed scan");
        fs::rename(&old_path, outside_dir.join("old.jpg")).expect("move image outside root");
        let (task_sender, _task_receiver) = tokio::sync::mpsc::channel(8);
        let mut state = test_watcher_state(database, task_sender, root_id, temp_root.clone());

        handle_event(
            &mut state,
            Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From))).add_path(old_path),
        )
        .expect("rename-from event should mark missing");

        let media = state
            .database
            .list_media_page(MediaPageQuery {
                root_id: Some(root_id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert!(media.items.is_empty());

        let _ = fs::remove_dir_all(temp_root);
        let _ = fs::remove_dir_all(outside_dir);
        let _ = fs::remove_dir_all(db_dir);
    }

    #[test]
    fn watcher_upserts_route_through_writer_and_land() {
        let (mut state, worker_db, temp_root, db_dir, root, path, metadata) =
            watcher_state_with_one_file();

        upsert_media_file(&mut state, &root, &path, &metadata).expect("upsert via writer");

        // Read back on an independent connection to the same file-backed DB —
        // the row must be committed and queryable, not merely buffered.
        let page = worker_db
            .list_media_page(MediaPageQuery {
                root_id: Some(root.id),
                folder_id: None,
                include_descendants: false,
                limit: 10,
                offset: None,
                cursor: None,
                sort: "name_asc".to_string(),
                kind: None,
            })
            .expect("list media");
        assert_eq!(page.items.len(), 1, "watched file must be upserted");
        assert_eq!(page.items[0].name, "new.jpg");

        let _ = fs::remove_dir_all(&temp_root);
        let _ = fs::remove_dir_all(&db_dir);
    }

    fn add_root(database: &Database, temp_root: &Path, display_name: &str) -> i64 {
        database
            .add_root(NewRoot {
                path: temp_root.to_string_lossy().to_string(),
                display_name: display_name.to_string(),
            })
            .expect("add root")
    }

    fn test_root(database: &Database, root_id: i64) -> RootRecord {
        database
            .get_root(root_id)
            .expect("get root")
            .expect("root exists")
    }

    fn test_watcher_state(
        database: Database,
        task_queue: TaskSender,
        root_id: i64,
        root_path: PathBuf,
    ) -> WatcherState {
        let (event_tx, _event_rx) = std_mpsc::channel();
        let watcher = recommended_watcher(move |result| {
            let _ = event_tx.send(result);
        })
        .expect("create watcher");
        let writer = DbWriter::spawn(
            database
                .reopen()
                .expect("reopen writer db")
                .expect("writer db"),
        );
        WatcherState {
            database,
            task_queue,
            writer,
            watcher,
            watched_roots: HashMap::from([(root_id, root_path)]),
            pending_events: Vec::new(),
        }
    }

    /// Builds a `WatcherState` with a real file-backed DB + a `DbWriter`, plus
    /// an independent connection (`worker_db`) to the same file for verifying
    /// writes landed. Mirrors `test_watcher_state`'s setup.
    fn watcher_state_with_one_file() -> (
        WatcherState,
        Database,
        PathBuf,
        PathBuf,
        RootRecord,
        PathBuf,
        fs::Metadata,
    ) {
        let temp_root = unique_temp_dir("root");
        fs::create_dir_all(&temp_root).expect("create root dir");
        let file_path = temp_root.join("new.jpg");
        fs::write(&file_path, b"new image").expect("write image");

        let db_dir = unique_temp_dir("db");
        fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("watch.sqlite");
        let database = Database::open(&db_path).expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_id = add_root(&database, &temp_root, "watch-root");
        let root = test_root(&database, root_id);

        let (task_sender, _task_receiver) = tokio::sync::mpsc::channel(8);
        let state = test_watcher_state(database, task_sender, root_id, temp_root.clone());

        let worker_db = Database::open(&db_path).expect("open worker db for verification");
        let metadata = fs::metadata(&file_path).expect("file metadata");
        (
            state, worker_db, temp_root, db_dir, root, file_path, metadata,
        )
    }

    fn wait_for_media_count(db_path: &Path, root_id: i64, expected: usize, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            let inspector = Database::open(db_path).expect("open inspector db");
            let page = inspector
                .list_media_page(MediaPageQuery {
                    root_id: Some(root_id),
                    folder_id: None,
                    include_descendants: false,
                    limit: 10,
                    offset: None,
                    cursor: None,
                    sort: "name_asc".to_string(),
                    kind: None,
                })
                .expect("list media");
            if page.items.len() == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "media count did not reach {expected}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_for_media_names(db_path: &Path, root_id: i64, expected: &[&str], timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            let inspector = Database::open(db_path).expect("open inspector db");
            let page = inspector
                .list_media_page(MediaPageQuery {
                    root_id: Some(root_id),
                    folder_id: None,
                    include_descendants: false,
                    limit: 10,
                    offset: None,
                    cursor: None,
                    sort: "name_asc".to_string(),
                    kind: None,
                })
                .expect("list media");
            let names: Vec<&str> = page.items.iter().map(|item| item.name.as_str()).collect();
            if names == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "media names did not reach {:?}; last names were {:?}",
                expected,
                names
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

        std::env::temp_dir().join(format!(
            "megle_watch_test_{}_{}_{}_{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }
}
