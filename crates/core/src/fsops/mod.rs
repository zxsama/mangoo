//! File operations: rename, move, delete (recycle / permanent).
//!
//! Each public entry point uses a recoverable two-phase journal:
//!   1. Resolve and validate every requested item.
//!   2. Persist all `pending` intents and the logical operation head in one
//!      SQLite transaction.
//!   3. Perform filesystem work without holding the SQLite mutex.
//!   4. Finalize the intent, index/audit row, progress, and Outbox event in one
//!      SQLite transaction.
//!   5. On restart, reconcile pending intents against the actual old/new paths
//!      before generic interrupted-operation recovery runs.
//!
//! The contract surfaces operation kinds `rename`, `move`,
//! `delete_recycle`, `delete_permanent` — see
//! `contracts/core-api/openapi.yaml`.

mod journal;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;

use crate::db::{Database, Page};

/// Discriminants for `file_operations.operation`.
#[allow(dead_code)]
pub const FILE_OPERATION_VALUES: &[&str] =
    &["rename", "move", "delete_recycle", "delete_permanent"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOperationKind {
    Rename,
    Move,
    DeleteRecycle,
    DeletePermanent,
}

impl FileOperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FileOperationKind::Rename => "rename",
            FileOperationKind::Move => "move",
            FileOperationKind::DeleteRecycle => "delete_recycle",
            FileOperationKind::DeletePermanent => "delete_permanent",
        }
    }
}

/// One persisted entry in `file_operations`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOperationRecord {
    pub id: i64,
    pub operation: String,
    pub source_path: String,
    pub target_path: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
    #[serde(skip_serializing)]
    pub operation_group_id: Option<String>,
}

/// Stable error codes that the API surfaces in JSON bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsOpsErrorCode {
    InvalidName,
    InvalidRequest,
    NotFound,
    NameConflict,
    /// Source and target paths belong to different roots. The historical
    /// "cross_volume" name was misleading — two roots can sit on the same
    /// physical volume — so we surface this as "cross_root" on the wire.
    /// TODO: detect actual cross-volume moves (different drive letters /
    /// different mount points) and add a separate `cross_volume` code if we
    /// later add copy+verify+delete fallback.
    CrossRoot,
    FsError,
    OutsideRoot,
    /// Refused to follow a symlink during a permanent delete.
    SymlinkRefused,
}

impl FsOpsErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            FsOpsErrorCode::InvalidName => "invalid_name",
            FsOpsErrorCode::InvalidRequest => "invalid_request",
            FsOpsErrorCode::NotFound => "not_found",
            FsOpsErrorCode::NameConflict => "name_conflict",
            FsOpsErrorCode::CrossRoot => "cross_root",
            FsOpsErrorCode::FsError => "fs_error",
            FsOpsErrorCode::OutsideRoot => "outside_root",
            FsOpsErrorCode::SymlinkRefused => "symlink_refused",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FsOpsError {
    pub code: FsOpsErrorCode,
    pub message: String,
}

impl FsOpsError {
    fn new(code: FsOpsErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for FsOpsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for FsOpsError {}

impl From<rusqlite::Error> for FsOpsError {
    fn from(error: rusqlite::Error) -> Self {
        FsOpsError::new(FsOpsErrorCode::FsError, error.to_string())
    }
}

impl From<anyhow::Error> for FsOpsError {
    fn from(error: anyhow::Error) -> Self {
        FsOpsError::new(FsOpsErrorCode::FsError, error.to_string())
    }
}

pub type FsOpsResult<T> = Result<T, FsOpsError>;

#[derive(Debug, Clone)]
pub struct RenameRequest {
    pub file_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub new_name: String,
}

#[derive(Debug, Clone, Default)]
pub struct MoveRequest {
    pub file_ids: Vec<i64>,
    pub folder_ids: Vec<i64>,
    pub target_folder_id: i64,
}

#[derive(Debug, Clone, Default)]
pub struct DeleteRequest {
    pub file_ids: Vec<i64>,
    pub folder_ids: Vec<i64>,
    pub permanent: bool,
}

// -------------- public entry points -------------------------------------

pub fn rename(
    database: &Mutex<Database>,
    request: RenameRequest,
) -> FsOpsResult<FileOperationRecord> {
    journal::rename(database, request)
}

pub fn move_items(
    database: &Mutex<Database>,
    request: MoveRequest,
) -> FsOpsResult<Vec<FileOperationRecord>> {
    journal::move_items(database, request)
}

pub fn delete(
    database: &Mutex<Database>,
    request: DeleteRequest,
) -> FsOpsResult<Vec<FileOperationRecord>> {
    journal::delete(database, request)
}

pub(crate) fn recover_pending_file_operations(database: &Database) -> FsOpsResult<usize> {
    journal::recover_pending_file_operations(database)
}

pub fn list_recent(
    database: &Mutex<Database>,
    limit: i64,
    cursor: Option<&str>,
) -> FsOpsResult<Page<FileOperationRecord>> {
    let limit = limit.clamp(1, 200);
    let cursor_id = match cursor {
        Some(value) => Some(parse_cursor(value)?),
        None => None,
    };

    // Brief lock just for the SELECT — no FS work is performed here, so
    // the listing call is short, but we still scope the guard explicitly so
    // the lock is released the moment the Vec is materialized.
    let guard = database.lock().expect("database mutex poisoned");
    let conn = guard.connection_for_fsops();
    let sql = format!(
        "SELECT id, operation, source_path, target_path, status, created_at, finished_at, error, operation_group_id \
         FROM file_operations {} ORDER BY id DESC LIMIT {}",
        if cursor_id.is_some() {
            "WHERE status <> 'pending' AND id < ?1"
        } else {
            "WHERE status <> 'pending'"
        },
        if cursor_id.is_some() { "?2" } else { "?1" }
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = if let Some(cursor_id) = cursor_id {
        statement.query_map(params![cursor_id, limit + 1], record_from_row)?
    } else {
        statement.query_map(params![limit + 1], record_from_row)?
    };
    let mut records: Vec<FileOperationRecord> = Vec::new();
    for row in rows {
        records.push(row?);
    }
    drop(statement);
    drop(guard);
    let next_cursor = if records.len() > limit as usize {
        records.pop();
        records.last().map(|record| record.id.to_string())
    } else {
        None
    };
    Ok(Page {
        items: records,
        next_cursor,
        total_count: None,
    })
}

// -------------- helpers -------------------------------------------------

#[derive(Debug, Clone)]
struct FileInfo {
    folder_id: i64,
    root_id: i64,
    name: String,
    path: PathBuf,
    parent_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct FolderInfo {
    parent_id: Option<i64>,
    root_id: i64,
    name: String,
    path: PathBuf,
}

#[derive(Debug, Clone)]
struct FolderTarget {
    folder_id: i64,
    root_id: i64,
    path: PathBuf,
}

fn load_file_info(conn: &rusqlite::Connection, file_id: i64) -> FsOpsResult<Option<FileInfo>> {
    let row = conn
        .query_row(
            r#"
            SELECT files.folder_id, files.root_id, files.name
            FROM files
            WHERE files.id = ?1 AND files.status = 'active'
            "#,
            params![file_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((folder_id, root_id, name)) = row else {
        return Ok(None);
    };
    let parent_dir = match folder_path_for(conn, folder_id)? {
        Some(path) => path,
        None => return Ok(None),
    };
    let path = parent_dir.join(&name);
    Ok(Some(FileInfo {
        folder_id,
        root_id,
        name,
        path,
        parent_dir,
    }))
}

fn load_folder_info(
    conn: &rusqlite::Connection,
    folder_id: i64,
) -> FsOpsResult<Option<FolderInfo>> {
    let row = conn
        .query_row(
            r#"
            SELECT folders.parent_id, folders.root_id, folders.name
            FROM folders
            WHERE folders.id = ?1 AND folders.status = 'active'
            "#,
            params![folder_id],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((parent_id, root_id, name)) = row else {
        return Ok(None);
    };
    let path = match folder_path_for(conn, folder_id)? {
        Some(path) => path,
        None => return Ok(None),
    };
    Ok(Some(FolderInfo {
        parent_id,
        root_id,
        name,
        path,
    }))
}

fn load_folder_path(
    conn: &rusqlite::Connection,
    folder_id: i64,
) -> FsOpsResult<Option<FolderTarget>> {
    let Some(info) = load_folder_info(conn, folder_id)? else {
        return Ok(None);
    };
    Ok(Some(FolderTarget {
        folder_id,
        root_id: info.root_id,
        path: info.path,
    }))
}

/// Walk from a folder up to its root, joining names onto the root's path.
fn folder_path_for(conn: &rusqlite::Connection, folder_id: i64) -> FsOpsResult<Option<PathBuf>> {
    let mut chain: Vec<String> = Vec::new();
    let mut current_id = folder_id;
    let root_id;
    loop {
        let row: Option<(Option<i64>, i64, String)> = conn
            .query_row(
                "SELECT parent_id, root_id, name FROM folders WHERE id = ?1",
                params![current_id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((parent_id, current_root_id, name)) = row else {
            return Ok(None);
        };
        match parent_id {
            Some(parent) => {
                chain.push(name);
                current_id = parent;
            }
            None => {
                root_id = current_root_id;
                break;
            }
        }
    }
    let root_path: Option<String> = conn
        .query_row(
            "SELECT path FROM roots WHERE id = ?1",
            params![root_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(root_path) = root_path else {
        return Ok(None);
    };
    let mut path = PathBuf::from(root_path);
    for component in chain.iter().rev() {
        path.push(component);
    }
    Ok(Some(path))
}

fn folder_is_ancestor_of(
    conn: &rusqlite::Connection,
    ancestor_id: i64,
    descendant_id: i64,
) -> FsOpsResult<bool> {
    if ancestor_id == descendant_id {
        return Ok(true);
    }
    let mut current = descendant_id;
    loop {
        let parent: Option<Option<i64>> = conn
            .query_row(
                "SELECT parent_id FROM folders WHERE id = ?1",
                params![current],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?;
        let Some(parent) = parent else {
            return Ok(false);
        };
        match parent {
            Some(parent_id) if parent_id == ancestor_id => return Ok(true),
            Some(parent_id) => current = parent_id,
            None => return Ok(false),
        }
    }
}

fn file_exists_in_folder_conn(
    conn: &rusqlite::Connection,
    folder_id: i64,
    name: &str,
) -> FsOpsResult<bool> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM files WHERE folder_id = ?1 AND name = ?2 AND status = 'active')",
        params![folder_id, name],
        |row| row.get(0),
    )?;
    Ok(exists != 0)
}

fn folder_name_exists_in_parent_conn(
    conn: &rusqlite::Connection,
    parent_id: Option<i64>,
    root_id: i64,
    name: &str,
) -> FsOpsResult<bool> {
    let exists: i64 = if let Some(parent_id) = parent_id {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM folders WHERE parent_id = ?1 AND name = ?2 AND status = 'active')",
            params![parent_id, name],
            |row| row.get(0),
        )?
    } else {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM folders WHERE parent_id IS NULL AND root_id = ?1 AND name = ?2 AND status = 'active')",
            params![root_id, name],
            |row| row.get(0),
        )?
    };
    Ok(exists != 0)
}

fn collect_descendant_file_ids(
    transaction: &Transaction<'_>,
    folder_id: i64,
) -> FsOpsResult<Vec<i64>> {
    let mut statement = transaction.prepare(
        r#"
        WITH RECURSIVE subtree(id) AS (
          SELECT id FROM folders WHERE id = ?1
          UNION ALL
          SELECT folders.id
          FROM folders
          JOIN subtree ON folders.parent_id = subtree.id
        )
        SELECT files.id
        FROM files
        WHERE files.folder_id IN (SELECT id FROM subtree)
        "#,
    )?;
    let rows = statement.query_map(params![folder_id], |row| row.get::<_, i64>(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(ids)
}

/// Refuse to permanently delete an entry whose `symlink_metadata` reports a
/// symlink. This is kept separate from the journal's root-boundary check so
/// neither check holds a SQLite connection during filesystem I/O.
fn refuse_if_symlink(path: &Path) -> FsOpsResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                Err(FsOpsError::new(
                    FsOpsErrorCode::SymlinkRefused,
                    format!(
                        "permanent delete refused: '{}' is a symlink",
                        path.display()
                    ),
                ))
            } else {
                Ok(())
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The entry vanished between snapshot and FS work — let the
            // FS call surface the not-found error in its own time.
            Ok(())
        }
        Err(error) => Err(FsOpsError::new(
            FsOpsErrorCode::FsError,
            format!("symlink check failed: {error}"),
        )),
    }
}

/// Rename a file or directory with a small TOCTOU defense on Unix.
///
/// `std::fs::rename` calls `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`
/// on Windows, so an existing target raises an error. On Unix, however,
/// `rename(2)` overwrites silently. Even after the database collision check,
/// a concurrent writer can race a file into place. To narrow the window we
/// reserve the destination via `OpenOptions::create_new` first, delete the
/// placeholder, then perform the actual rename. If the placeholder cannot be
/// reserved we surface a clear `name_conflict`. The remaining race window is
/// only the gap between the placeholder removal and the rename, which is far
/// smaller than the original DB-check + rename window.
fn atomic_rename(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        match OpenOptions::new().write(true).create_new(true).open(target) {
            Ok(file) => {
                drop(file);
                if let Err(error) = fs::remove_file(target) {
                    // Removing our own placeholder failed — surface as IO.
                    return Err(error);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("target already exists: {}", target.display()),
                ));
            }
            Err(error) => return Err(error),
        }
    }
    fs::rename(source, target)
}

fn validate_name(name: &str) -> FsOpsResult<()> {
    if name.is_empty() {
        return Err(FsOpsError::new(
            FsOpsErrorCode::InvalidName,
            "name must not be empty",
        ));
    }
    if name.chars().count() > 255 {
        return Err(FsOpsError::new(
            FsOpsErrorCode::InvalidName,
            "name must be 255 characters or fewer",
        ));
    }
    if name == "." || name == ".." {
        return Err(FsOpsError::new(
            FsOpsErrorCode::InvalidName,
            "name must not be '.' or '..'",
        ));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(FsOpsError::new(
            FsOpsErrorCode::InvalidName,
            "name must not contain path separators",
        ));
    }
    if is_windows_reserved(name) {
        return Err(FsOpsError::new(
            FsOpsErrorCode::InvalidName,
            format!("'{name}' is a reserved Windows name"),
        ));
    }
    Ok(())
}

fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn extension_from_name(name: &str) -> String {
    // Match `scan::extension` so the on-disk name and the indexed `ext`
    // column agree on case. Without this, renaming "image.jpg" to
    // "image.MP4" would store ".MP4" while the rest of the indexer assumes
    // lowercase.
    Path::new(name)
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default()
}

fn parse_cursor(cursor: &str) -> FsOpsResult<i64> {
    cursor.parse::<i64>().map_err(|_| {
        FsOpsError::new(
            FsOpsErrorCode::InvalidRequest,
            format!("invalid file_operations cursor: {cursor}"),
        )
    })
}

fn record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileOperationRecord> {
    Ok(FileOperationRecord {
        id: row.get(0)?,
        operation: row.get(1)?,
        source_path: row.get(2)?,
        target_path: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        finished_at: row.get(6)?,
        error: row.get(7)?,
        operation_group_id: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, FileUpsert, FolderUpsert, NewRoot};
    use std::fs as stdfs;
    use std::path::PathBuf;

    struct TestEnv {
        database: Mutex<Database>,
        root_id: i64,
        root_folder_id: i64,
        root_path: PathBuf,
        database_path: Option<PathBuf>,
    }

    impl TestEnv {
        fn new(label: &str) -> Self {
            let root_path = unique_temp_dir(label);
            stdfs::create_dir_all(&root_path).expect("create root path");
            let database = Database::open_in_memory().expect("open database");
            Self::from_database(label, root_path, database, None)
        }

        fn new_file_backed(label: &str) -> Self {
            let root_path = unique_temp_dir(label);
            stdfs::create_dir_all(&root_path).expect("create root path");
            let database_path = root_path.join("library.sqlite");
            let database = Database::open(&database_path).expect("open file database");
            Self::from_database(label, root_path, database, Some(database_path))
        }

        fn from_database(
            label: &str,
            root_path: PathBuf,
            database: Database,
            database_path: Option<PathBuf>,
        ) -> Self {
            database.apply_migrations().expect("apply migrations");
            let root_id = database
                .add_root(NewRoot {
                    path: root_path.to_string_lossy().into_owned(),
                    display_name: format!("test {label}"),
                })
                .expect("add root");
            let root_folder_id = database
                .upsert_folder(FolderUpsert {
                    root_id,
                    parent_id: None,
                    name: String::new(),
                    path_hash: format!("root-{label}"),
                    mtime: Some(1),
                })
                .expect("insert root folder");
            Self {
                database: Mutex::new(database),
                root_id,
                root_folder_id,
                root_path,
                database_path,
            }
        }

        fn reopened_database(&self) -> Database {
            self.database
                .lock()
                .expect("db lock")
                .reopen()
                .expect("reopen database")
                .expect("file-backed database")
        }

        async fn run_startup_recovery(&self) {
            let worker_database = self.reopened_database();
            let writer = crate::db::DbWriter::spawn(self.reopened_database());
            let sender = crate::tasks::start_worker_with_ffmpeg(worker_database, writer, true);
            drop(sender);
            tokio::task::yield_now().await;
        }

        fn add_file(&self, folder_id: i64, name: &str) -> i64 {
            let path = self.folder_path(folder_id).join(name);
            stdfs::write(&path, b"x").expect("write test file");
            self.database
                .lock()
                .expect("db lock")
                .upsert_file(FileUpsert {
                    root_id: self.root_id,
                    folder_id,
                    name: name.to_string(),
                    ext: extension_from_name(name),
                    size: 1,
                    mtime: 2,
                    ctime: None,
                    file_key: None,
                })
                .expect("insert file")
        }

        fn add_folder(&self, parent_id: i64, name: &str) -> i64 {
            let path = self.folder_path(parent_id).join(name);
            stdfs::create_dir_all(&path).expect("create child folder");
            self.database
                .lock()
                .expect("db lock")
                .upsert_folder(FolderUpsert {
                    root_id: self.root_id,
                    parent_id: Some(parent_id),
                    name: name.to_string(),
                    path_hash: format!("hash-{}-{name}", parent_id),
                    mtime: Some(3),
                })
                .expect("insert folder")
        }

        fn folder_path(&self, folder_id: i64) -> PathBuf {
            let guard = self.database.lock().expect("db lock");
            folder_path_for(&guard.connection_for_fsops(), folder_id)
                .expect("folder path lookup")
                .expect("folder path resolved")
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            let _ = self.database_path.take();
            let _ = stdfs::remove_dir_all(&self.root_path);
        }
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "megle_fsops_{}_{}_{}_{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_nanos(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    #[test]
    fn rename_file_updates_disk_db_and_log() {
        let env = TestEnv::new("rename_ok");
        let file_id = env.add_file(env.root_folder_id, "old.jpg");
        let record = rename(
            &env.database,
            RenameRequest {
                file_id: Some(file_id),
                folder_id: None,
                new_name: "new.jpg".into(),
            },
        )
        .expect("rename");
        assert_eq!(record.operation, "rename");
        assert_eq!(record.status, "succeeded");
        assert!(env.root_path.join("new.jpg").exists());
        assert!(!env.root_path.join("old.jpg").exists());
        let media = env
            .database
            .lock()
            .expect("db lock")
            .get_media(file_id)
            .expect("get media")
            .unwrap();
        assert_eq!(media.name, "new.jpg");
    }

    #[test]
    fn rename_invalid_name_rejected() {
        let env = TestEnv::new("rename_invalid");
        let file_id = env.add_file(env.root_folder_id, "a.jpg");
        for bad in [
            "",
            "..",
            "with/slash.jpg",
            "with\\back.jpg",
            "CON",
            "Prn.txt",
        ] {
            let error = rename(
                &env.database,
                RenameRequest {
                    file_id: Some(file_id),
                    folder_id: None,
                    new_name: bad.into(),
                },
            )
            .expect_err(&format!("expected reject for {bad}"));
            assert_eq!(error.code, FsOpsErrorCode::InvalidName);
        }
    }

    #[test]
    fn rename_collision_returns_conflict() {
        let env = TestEnv::new("rename_conflict");
        let _ = env.add_file(env.root_folder_id, "a.jpg");
        let other_id = env.add_file(env.root_folder_id, "b.jpg");
        let error = rename(
            &env.database,
            RenameRequest {
                file_id: Some(other_id),
                folder_id: None,
                new_name: "a.jpg".into(),
            },
        )
        .expect_err("expected conflict");
        assert_eq!(error.code, FsOpsErrorCode::NameConflict);
    }

    #[test]
    fn rename_missing_file_returns_not_found() {
        let env = TestEnv::new("rename_404");
        let error = rename(
            &env.database,
            RenameRequest {
                file_id: Some(99_999),
                folder_id: None,
                new_name: "x.jpg".into(),
            },
        )
        .expect_err("expected not_found");
        assert_eq!(error.code, FsOpsErrorCode::NotFound);
    }

    #[test]
    fn move_file_within_root_succeeds() {
        let env = TestEnv::new("move_ok");
        let target_folder = env.add_folder(env.root_folder_id, "dst");
        let file_id = env.add_file(env.root_folder_id, "m.jpg");
        let records = move_items(
            &env.database,
            MoveRequest {
                file_ids: vec![file_id],
                folder_ids: vec![],
                target_folder_id: target_folder,
            },
        )
        .expect("move");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "succeeded");
        assert!(env.root_path.join("dst").join("m.jpg").exists());
        assert!(!env.root_path.join("m.jpg").exists());
        let media = env
            .database
            .lock()
            .expect("db lock")
            .get_media(file_id)
            .expect("get")
            .unwrap();
        assert_eq!(media.folder_id, target_folder);
    }

    #[test]
    fn move_cross_root_returns_cross_root() {
        let env_a = TestEnv::new("move_a");
        let env_b = TestEnv::new("move_b");
        let file_id = env_a.add_file(env_a.root_folder_id, "x.jpg");
        // Register the second root in env_a's database to simulate two roots.
        let (other_root, other_root_folder) = {
            let guard = env_a.database.lock().expect("db lock");
            let other_root = guard
                .add_root(NewRoot {
                    path: env_b.root_path.to_string_lossy().into_owned(),
                    display_name: "other".into(),
                })
                .expect("add second root");
            let other_root_folder = guard
                .upsert_folder(FolderUpsert {
                    root_id: other_root,
                    parent_id: None,
                    name: String::new(),
                    path_hash: "other-root".into(),
                    mtime: Some(1),
                })
                .expect("insert other root folder");
            (other_root, other_root_folder)
        };
        let _ = other_root;
        let error = move_items(
            &env_a.database,
            MoveRequest {
                file_ids: vec![file_id],
                folder_ids: vec![],
                target_folder_id: other_root_folder,
            },
        )
        .expect_err("expected cross_root");
        assert_eq!(error.code, FsOpsErrorCode::CrossRoot);
    }

    #[test]
    fn move_missing_target_returns_not_found() {
        let env = TestEnv::new("move_404");
        let file_id = env.add_file(env.root_folder_id, "m.jpg");
        let error = move_items(
            &env.database,
            MoveRequest {
                file_ids: vec![file_id],
                folder_ids: vec![],
                target_folder_id: 99_999,
            },
        )
        .expect_err("expected 404");
        assert_eq!(error.code, FsOpsErrorCode::NotFound);
    }

    #[test]
    fn delete_permanent_removes_disk_and_marks_db() {
        let env = TestEnv::new("delete_permanent");
        let file_id = env.add_file(env.root_folder_id, "z.jpg");
        let records = delete(
            &env.database,
            DeleteRequest {
                file_ids: vec![file_id],
                folder_ids: vec![],
                permanent: true,
            },
        )
        .expect("delete");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].operation, "delete_permanent");
        assert!(!env.root_path.join("z.jpg").exists());
        assert!(env
            .database
            .lock()
            .expect("db lock")
            .get_media(file_id)
            .expect("query media")
            .is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn delete_recycle_removes_from_disk() {
        let env = TestEnv::new("delete_recycle");
        let file_id = env.add_file(env.root_folder_id, "r.jpg");
        let records = delete(
            &env.database,
            DeleteRequest {
                file_ids: vec![file_id],
                folder_ids: vec![],
                permanent: false,
            },
        )
        .expect("recycle delete");
        assert_eq!(records[0].operation, "delete_recycle");
        assert!(!env.root_path.join("r.jpg").exists());
    }

    #[test]
    fn delete_with_files_and_folders_processes_both() {
        let env = TestEnv::new("delete_mixed");
        let file_id = env.add_file(env.root_folder_id, "mixed.jpg");
        let child = env.add_folder(env.root_folder_id, "subdir");
        let records = delete(
            &env.database,
            DeleteRequest {
                file_ids: vec![file_id],
                folder_ids: vec![child],
                permanent: true,
            },
        )
        .expect("delete mixed");
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|record| record.status == "succeeded"));
    }

    #[test]
    fn move_command_persists_one_uuid_v7_notification_group() {
        let env = TestEnv::new("move_notification_group");
        let target = env.add_folder(env.root_folder_id, "target");
        let file_ids = ["a.jpg", "b.jpg", "c.jpg"]
            .into_iter()
            .map(|name| env.add_file(env.root_folder_id, name))
            .collect::<Vec<_>>();

        let records = move_items(
            &env.database,
            MoveRequest {
                file_ids,
                folder_ids: vec![],
                target_folder_id: target,
            },
        )
        .expect("move command");

        assert_eq!(records.len(), 3);
        let group_id = records[0]
            .operation_group_id
            .as_deref()
            .expect("operation group id");
        let parsed = uuid::Uuid::parse_str(group_id).expect("UUID operation group id");
        assert_eq!(parsed.get_version_num(), 7);
        assert!(records
            .iter()
            .all(|record| { record.operation_group_id.as_deref() == Some(group_id) }));

        let guard = env.database.lock().expect("db lock");
        let heads = guard
            .notification_heads_for_test()
            .expect("notification heads");
        let operation = heads
            .iter()
            .find(|operation| operation.operation_id == group_id)
            .expect("file-operation head");
        assert_eq!(operation.category, "file_operation");
        assert_eq!(operation.kind, "move");
        assert_eq!(operation.status, "succeeded");
        assert_eq!(operation.current_value, 3);
        assert_eq!(operation.total_value, Some(3));
        assert_eq!(
            heads
                .iter()
                .filter(|operation| operation.operation_id == group_id)
                .count(),
            1
        );
    }

    #[test]
    fn rename_and_delete_persist_command_group_ids() {
        let env = TestEnv::new("rename_delete_notification_groups");
        let rename_id = env.add_file(env.root_folder_id, "rename.jpg");
        let renamed = rename(
            &env.database,
            RenameRequest {
                file_id: Some(rename_id),
                folder_id: None,
                new_name: "renamed.jpg".into(),
            },
        )
        .expect("rename command");
        let rename_group = renamed
            .operation_group_id
            .as_deref()
            .expect("rename group id");

        let delete_ids = ["delete-a.jpg", "delete-b.jpg"]
            .into_iter()
            .map(|name| env.add_file(env.root_folder_id, name))
            .collect::<Vec<_>>();
        let deleted = delete(
            &env.database,
            DeleteRequest {
                file_ids: delete_ids,
                folder_ids: vec![],
                permanent: true,
            },
        )
        .expect("delete command");
        let delete_group = deleted[0]
            .operation_group_id
            .as_deref()
            .expect("delete group id");

        assert_ne!(rename_group, delete_group);
        assert!(deleted
            .iter()
            .all(|record| { record.operation_group_id.as_deref() == Some(delete_group) }));
        let guard = env.database.lock().expect("db lock");
        let heads = guard
            .notification_heads_for_test()
            .expect("notification heads");
        assert_eq!(
            heads
                .iter()
                .find(|operation| operation.operation_id == rename_group)
                .expect("rename head")
                .status,
            "succeeded"
        );
        let delete_head = heads
            .iter()
            .find(|operation| operation.operation_id == delete_group)
            .expect("delete head");
        assert_eq!(delete_head.status, "succeeded");
        assert_eq!(delete_head.current_value, 2);
        assert_eq!(delete_head.total_value, Some(2));
    }

    #[test]
    fn partial_move_failure_keeps_monotonic_group_progress() {
        let env = TestEnv::new("move_partial_failure");
        let target = env.add_folder(env.root_folder_id, "target");
        let first = env.add_file(env.root_folder_id, "first.jpg");
        let second = env.add_file(env.root_folder_id, "second.jpg");
        let third = env.add_file(env.root_folder_id, "third.jpg");
        let _conflict = env.add_file(target, "second.jpg");

        let error = move_items(
            &env.database,
            MoveRequest {
                file_ids: vec![first, second, third],
                folder_ids: vec![],
                target_folder_id: target,
            },
        )
        .expect_err("second item must conflict");
        assert_eq!(error.code, FsOpsErrorCode::NameConflict);

        let guard = env.database.lock().expect("db lock");
        let operations = guard
            .notification_heads_for_test()
            .expect("notification heads")
            .into_iter()
            .filter(|operation| operation.category == "file_operation")
            .collect::<Vec<_>>();
        assert_eq!(operations.len(), 1);
        let operation = &operations[0];
        assert_eq!(operation.kind, "move");
        assert_eq!(operation.status, "failed");
        assert_eq!(operation.current_value, 3);
        assert_eq!(operation.total_value, Some(3));
        assert_eq!(operation.error_code.as_deref(), Some("name_conflict"));
        let audit_rows: Vec<(String, String)> = guard
            .connection_for_fsops()
            .prepare(
                "SELECT operation_group_id, status FROM file_operations WHERE operation_group_id = ?1 ORDER BY id",
            )
            .expect("prepare grouped audit query")
            .query_map([&operation.operation_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query grouped audit rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect grouped audit rows");
        assert_eq!(audit_rows.len(), 3);
        assert!(audit_rows
            .iter()
            .all(|(group_id, _)| group_id == &operation.operation_id));
        assert_eq!(
            audit_rows
                .iter()
                .filter(|(_, status)| status == "failed")
                .count(),
            1
        );
        let events = guard
            .list_notification_outbox(0, 200)
            .expect("notification outbox")
            .into_iter()
            .filter(|event| event.operation.operation_id == operation.operation_id)
            .collect::<Vec<_>>();
        assert!(events
            .windows(2)
            .all(|window| window[0].operation.current_value <= window[1].operation.current_value));
        assert_eq!(
            events.last().expect("terminal event").operation.status,
            "failed"
        );
        assert!(env.root_path.join("target").join("first.jpg").exists());
        assert!(env.root_path.join("second.jpg").exists());
        assert!(env.root_path.join("target").join("third.jpg").exists());
    }

    #[test]
    fn filesystem_failure_persists_failed_audit_and_terminal_in_one_group() {
        let env = TestEnv::new("delete_filesystem_failure");
        let file_id = env.add_file(env.root_folder_id, "missing.jpg");
        let second = env.add_file(env.root_folder_id, "second.jpg");
        let third = env.add_file(env.root_folder_id, "third.jpg");
        stdfs::remove_file(env.root_path.join("missing.jpg")).expect("remove source from disk");

        let error = delete(
            &env.database,
            DeleteRequest {
                file_ids: vec![file_id, second, third],
                folder_ids: vec![],
                permanent: true,
            },
        )
        .expect_err("filesystem delete must fail");
        assert_eq!(error.code, FsOpsErrorCode::FsError);

        let page = list_recent(&env.database, 10, None).expect("recent file operations");
        assert_eq!(page.items.len(), 3);
        let group_id = page.items[0]
            .operation_group_id
            .as_deref()
            .expect("failed operation group id");
        assert!(page
            .items
            .iter()
            .all(|audit| { audit.operation_group_id.as_deref() == Some(group_id) }));
        assert_eq!(
            page.items
                .iter()
                .filter(|audit| audit.status == "failed")
                .count(),
            1
        );
        let guard = env.database.lock().expect("db lock");
        let operation = guard
            .notification_heads_for_test()
            .expect("notification heads")
            .into_iter()
            .find(|operation| operation.operation_id == group_id)
            .expect("failed operation head");
        assert_eq!(operation.status, "failed");
        assert_eq!(operation.current_value, 3);
        assert_eq!(operation.total_value, Some(3));
        assert_eq!(operation.error_code.as_deref(), Some("fs_error"));
        assert!(!env.root_path.join("second.jpg").exists());
        assert!(!env.root_path.join("third.jpg").exists());
    }

    #[test]
    fn legacy_file_operation_json_omits_internal_group_id() {
        let env = TestEnv::new("legacy_operation_json");
        let file_id = env.add_file(env.root_folder_id, "legacy.jpg");
        let record = rename(
            &env.database,
            RenameRequest {
                file_id: Some(file_id),
                folder_id: None,
                new_name: "legacy-renamed.jpg".into(),
            },
        )
        .expect("rename command");
        assert!(record.operation_group_id.is_some());
        let json = serde_json::to_value(record).expect("serialize legacy response");
        assert!(json.get("operationGroupId").is_none());
    }

    #[test]
    fn interrupted_file_operation_group_recovers_to_terminal_failure() {
        let env = TestEnv::new("file_operation_recovery");
        let group_id = journal::begin_test_group(&env.database, FileOperationKind::Move, 2)
            .expect("begin file-operation group");

        let guard = env.database.lock().expect("db lock");
        assert_eq!(
            guard
                .recover_interrupted_notification_operations()
                .expect("recover interrupted group"),
            1
        );
        let operation = guard
            .notification_heads_for_test()
            .expect("notification heads")
            .into_iter()
            .find(|operation| operation.operation_id == group_id)
            .expect("recovered group");
        assert_eq!(operation.status, "failed");
        assert_eq!(operation.origin, "recovery");
        assert_eq!(operation.error_code.as_deref(), Some("interrupted"));
        assert_eq!(operation.current_value, 0);
        assert_eq!(operation.total_value, Some(2));
        assert!(operation.completed_at.is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_finalizes_pending_intent_when_fs_already_completed() {
        let env = TestEnv::new_file_backed("journal_fs_done");
        let file_id = env.add_file(env.root_folder_id, "before.jpg");
        env.database
            .lock()
            .expect("db lock")
            .connection_for_fsops()
            .execute_batch(
                r#"
                CREATE TRIGGER fail_file_operation_finalize
                BEFORE UPDATE OF status ON file_operations
                WHEN OLD.status = 'pending' AND NEW.status <> 'pending'
                BEGIN
                  SELECT RAISE(ABORT, 'forced finalize failure');
                END;
                "#,
            )
            .expect("install finalize failure trigger");

        let error = rename(
            &env.database,
            RenameRequest {
                file_id: Some(file_id),
                folder_id: None,
                new_name: "after.jpg".into(),
            },
        )
        .expect_err("finalize failure must surface without losing the intent");
        assert_eq!(error.code, FsOpsErrorCode::FsError);
        assert!(!env.root_path.join("before.jpg").exists());
        assert!(env.root_path.join("after.jpg").exists());

        let (journal_id, group_id, status): (i64, String, String) = env
            .database
            .lock()
            .expect("db lock")
            .connection_for_fsops()
            .query_row(
                "SELECT id, operation_group_id, status FROM file_operations ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("pending journal intent");
        assert_eq!(status, "pending");
        assert!(
            list_recent(&env.database, 10, None)
                .expect("legacy recent operations")
                .items
                .is_empty(),
            "legacy API must not expose journal-only pending status"
        );
        let before_recovery = env
            .database
            .lock()
            .expect("db lock")
            .notification_heads_for_test()
            .expect("notification heads")
            .into_iter()
            .find(|operation| operation.operation_id == group_id)
            .expect("operation head before recovery");
        assert_ne!(before_recovery.status, "failed");
        let indexed_name_before_recovery: String = env
            .database
            .lock()
            .expect("db lock")
            .connection_for_fsops()
            .query_row("SELECT name FROM files WHERE id = ?1", [file_id], |row| {
                row.get(0)
            })
            .expect("index update must have rolled back with finalize");
        assert_eq!(indexed_name_before_recovery, "before.jpg");

        env.database
            .lock()
            .expect("db lock")
            .connection_for_fsops()
            .execute_batch("DROP TRIGGER fail_file_operation_finalize;")
            .expect("remove finalize failure trigger");
        env.run_startup_recovery().await;

        let guard = env.database.lock().expect("db lock");
        let recovered_status: String = guard
            .connection_for_fsops()
            .query_row(
                "SELECT status FROM file_operations WHERE id = ?1",
                [journal_id],
                |row| row.get(0),
            )
            .expect("recovered journal status");
        let indexed_name: String = guard
            .connection_for_fsops()
            .query_row("SELECT name FROM files WHERE id = ?1", [file_id], |row| {
                row.get(0)
            })
            .expect("reconciled file row");
        let recovered = guard
            .notification_heads_for_test()
            .expect("notification heads")
            .into_iter()
            .find(|operation| operation.operation_id == group_id)
            .expect("recovered operation head");
        assert_eq!(recovered_status, "succeeded");
        assert_eq!(indexed_name, "after.jpg");
        assert_eq!(recovered.status, "succeeded");
        assert_eq!(recovered.current_value, 1);
        assert_eq!(recovered.total_value, Some(1));
        let terminal_event = guard
            .list_notification_outbox(0, 200)
            .expect("notification outbox")
            .into_iter()
            .filter(|event| event.operation.operation_id == group_id)
            .last()
            .expect("terminal recovery outbox event");
        assert_eq!(terminal_event.operation.status, "succeeded");
        assert_eq!(terminal_event.operation.current_value, 1);
        assert_eq!(terminal_event.operation.total_value, Some(1));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_retries_pending_intent_when_fs_not_started() {
        let env = TestEnv::new_file_backed("journal_fs_not_started");
        let file_id = env.add_file(env.root_folder_id, "before.jpg");
        let group_id = journal::begin_test_group(&env.database, FileOperationKind::Rename, 1)
            .expect("begin operation group");
        let source = env.root_path.join("before.jpg");
        let target = env.root_path.join("after.jpg");
        env.database
            .lock()
            .expect("db lock")
            .connection_for_fsops()
            .execute(
                r#"INSERT INTO file_operations(
                     operation, source_path, target_path, status, created_at,
                     operation_group_id, entity_kind, entity_id, attempt_count
                   ) VALUES ('rename', ?1, ?2, 'pending', unixepoch(), ?3, 'file', ?4, 0)"#,
                params![
                    source.to_string_lossy(),
                    target.to_string_lossy(),
                    group_id,
                    file_id
                ],
            )
            .expect("persist pending intent before filesystem work");

        env.run_startup_recovery().await;

        assert!(!source.exists());
        assert!(target.exists());
        let guard = env.database.lock().expect("db lock");
        let (status, attempt_count): (String, i64) = guard
            .connection_for_fsops()
            .query_row(
                "SELECT status, attempt_count FROM file_operations WHERE operation_group_id = ?1",
                [&group_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("recovered journal row");
        let indexed_name: String = guard
            .connection_for_fsops()
            .query_row("SELECT name FROM files WHERE id = ?1", [file_id], |row| {
                row.get(0)
            })
            .expect("reconciled file row");
        let recovered = guard
            .notification_heads_for_test()
            .expect("notification heads")
            .into_iter()
            .find(|operation| operation.operation_id == group_id)
            .expect("recovered operation head");
        assert_eq!(status, "succeeded");
        assert!(attempt_count >= 1);
        assert_eq!(indexed_name, "after.jpg");
        assert_eq!(recovered.status, "succeeded");
        assert_eq!(recovered.current_value, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_records_failed_retry_for_unexecutable_pending_intent() {
        let env = TestEnv::new_file_backed("journal_retry_failure");
        let file_id = env.add_file(env.root_folder_id, "before.jpg");
        let group_id = journal::begin_test_group(&env.database, FileOperationKind::Rename, 1)
            .expect("begin operation group");
        let source = env.root_path.join("before.jpg");
        let target = env.root_path.join("missing-parent").join("after.jpg");
        env.database
            .lock()
            .expect("db lock")
            .connection_for_fsops()
            .execute(
                r#"INSERT INTO file_operations(
                     operation, source_path, target_path, status, created_at,
                     operation_group_id, entity_kind, entity_id, attempt_count
                   ) VALUES ('rename', ?1, ?2, 'pending', unixepoch(), ?3, 'file', ?4, 0)"#,
                params![
                    source.to_string_lossy(),
                    target.to_string_lossy(),
                    group_id,
                    file_id
                ],
            )
            .expect("persist pending intent");

        env.run_startup_recovery().await;

        assert!(source.exists());
        assert!(!target.exists());
        let guard = env.database.lock().expect("db lock");
        let (status, attempt_count, error_code): (String, i64, Option<String>) = guard
            .connection_for_fsops()
            .query_row(
                "SELECT status, attempt_count, error_code FROM file_operations WHERE operation_group_id = ?1",
                [&group_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("failed recovered journal row");
        let recovered = guard
            .notification_heads_for_test()
            .expect("notification heads")
            .into_iter()
            .find(|operation| operation.operation_id == group_id)
            .expect("failed recovered operation head");
        assert_eq!(status, "failed");
        assert_eq!(attempt_count, 1);
        assert_eq!(error_code.as_deref(), Some("fs_error"));
        assert_eq!(recovered.status, "failed");
        assert_eq!(recovered.current_value, 1);
        assert_eq!(recovered.total_value, Some(1));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn startup_recovery_ignores_legacy_pending_audit_without_journal_metadata() {
        let env = TestEnv::new_file_backed("legacy_pending_audit");
        let audit_id = env
            .database
            .lock()
            .expect("db lock")
            .connection_for_fsops()
            .query_row(
                r#"INSERT INTO file_operations(
                     operation, source_path, target_path, status, created_at
                   ) VALUES ('rename', 'legacy-old', 'legacy-new', 'pending', 1)
                   RETURNING id"#,
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("seed pre-journal pending audit");

        env.run_startup_recovery().await;

        let guard = env.database.lock().expect("db lock");
        let row: (String, Option<String>, Option<String>) = guard
            .connection_for_fsops()
            .query_row(
                "SELECT status, operation_group_id, entity_kind FROM file_operations WHERE id = ?1",
                [audit_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("legacy audit survives startup recovery");
        assert_eq!(row, ("pending".to_owned(), None, None));
    }

    #[test]
    fn list_recent_returns_descending_ids() {
        let env = TestEnv::new("list_recent");
        for index in 0..3 {
            let file_id = env.add_file(env.root_folder_id, &format!("f{index}.jpg"));
            rename(
                &env.database,
                RenameRequest {
                    file_id: Some(file_id),
                    folder_id: None,
                    new_name: format!("g{index}.jpg"),
                },
            )
            .expect("rename");
        }
        let page = list_recent(&env.database, 50, None).expect("list");
        assert!(page.items.len() >= 3);
        for window in page.items.windows(2) {
            assert!(window[0].id > window[1].id, "ids must be DESC");
        }
    }

    #[test]
    fn rename_folder_updates_disk_and_db() {
        let env = TestEnv::new("rename_folder");
        let child = env.add_folder(env.root_folder_id, "old_dir");
        let _file_id = env.add_file(child, "inner.jpg");
        let record = rename(
            &env.database,
            RenameRequest {
                file_id: None,
                folder_id: Some(child),
                new_name: "new_dir".into(),
            },
        )
        .expect("rename folder");
        assert_eq!(record.operation, "rename");
        assert!(env.root_path.join("new_dir").join("inner.jpg").exists());
        assert!(!env.root_path.join("old_dir").exists());
    }

    #[test]
    fn extension_from_name_lowercases() {
        // Match the indexer convention: scan/mod.rs lowercases the extension
        // it stores in `files.ext`. Renames must do the same so the on-disk
        // name and the DB column don't drift apart on mixed-case extensions.
        assert_eq!(extension_from_name("image.MP4"), ".mp4");
        assert_eq!(extension_from_name("photo.JPG"), ".jpg");
        assert_eq!(extension_from_name("clip.MoV"), ".mov");
        assert_eq!(extension_from_name("noext"), "");
    }

    #[test]
    fn rename_to_mixed_case_extension_stores_lowercase_ext() {
        let env = TestEnv::new("rename_mixed_case_ext");
        let file_id = env.add_file(env.root_folder_id, "image.jpg");
        let record = rename(
            &env.database,
            RenameRequest {
                file_id: Some(file_id),
                folder_id: None,
                new_name: "image.MP4".into(),
            },
        )
        .expect("rename");
        assert_eq!(record.status, "succeeded");
        assert!(env.root_path.join("image.MP4").exists());

        // The on-disk name preserves user-typed case but `files.ext` must
        // be lowercased to agree with the rest of the indexer.
        let ext: String = env
            .database
            .lock()
            .expect("db lock")
            .connection_for_fsops()
            .query_row(
                "SELECT ext FROM files WHERE id = ?1",
                params![file_id],
                |row| row.get::<_, String>(0),
            )
            .expect("read ext");
        assert_eq!(ext, ".mp4");
    }

    #[test]
    fn rename_collision_does_not_clobber_existing_target_on_disk() {
        // Defense-in-depth regression for the TOCTOU window between the
        // collision check and `fs::rename`. Even on Windows where the
        // underlying `MoveFileExW` rejects existing targets, we want a
        // platform-agnostic test that asserts the existing target is not
        // overwritten when both files exist on disk.
        let env = TestEnv::new("rename_clobber_guard");
        let alpha_id = env.add_file(env.root_folder_id, "alpha.jpg");
        let _beta_id = env.add_file(env.root_folder_id, "beta.jpg");

        // Pre-populate the target on disk with distinguishable content so
        // we can prove it survived.
        let target_path = env.root_path.join("beta.jpg");
        stdfs::write(&target_path, b"beta-original").expect("seed beta");

        let error = rename(
            &env.database,
            RenameRequest {
                file_id: Some(alpha_id),
                folder_id: None,
                new_name: "beta.jpg".into(),
            },
        )
        .expect_err("rename onto existing target must fail");
        assert_eq!(error.code, FsOpsErrorCode::NameConflict);

        // The existing target retained its original bytes — no clobber.
        let bytes = stdfs::read(&target_path).expect("read beta");
        assert_eq!(bytes, b"beta-original".to_vec());
        // The source still exists too.
        assert!(env.root_path.join("alpha.jpg").exists());
    }

    #[cfg(unix)]
    #[test]
    fn permanent_delete_refuses_symlinked_entry() {
        use std::os::unix::fs::symlink;

        let env = TestEnv::new("symlink_refused");

        // An out-of-root file the symlink points at — we must keep it intact.
        let outside_dir = unique_temp_dir("symlink_outside");
        stdfs::create_dir_all(&outside_dir).expect("create outside dir");
        let outside_target = outside_dir.join("outside-target.bin");
        stdfs::write(&outside_target, b"keep-me").expect("write outside target");

        // Place a symlink inside the root that points at the outside target.
        let link_path = env.root_path.join("link.jpg");
        symlink(&outside_target, &link_path).expect("create symlink");

        // Insert a `files` row for the symlink so fsops can look it up.
        let file_id = env
            .database
            .lock()
            .expect("db lock")
            .upsert_file(FileUpsert {
                root_id: env.root_id,
                folder_id: env.root_folder_id,
                name: "link.jpg".to_string(),
                ext: ".jpg".to_string(),
                size: 1,
                mtime: 1,
                ctime: None,
                file_key: None,
            })
            .expect("insert symlink file row");

        let error = delete(
            &env.database,
            DeleteRequest {
                file_ids: vec![file_id],
                folder_ids: vec![],
                permanent: true,
            },
        )
        .expect_err("permanent delete of symlink must be refused");
        assert_eq!(error.code, FsOpsErrorCode::SymlinkRefused);

        // The link itself is untouched (we refused before any FS work).
        assert!(link_path.exists());
        // The outside target survived: we never followed the link.
        assert!(outside_target.exists());
        let bytes = stdfs::read(&outside_target).expect("read outside target");
        assert_eq!(bytes, b"keep-me".to_vec());

        let _ = stdfs::remove_dir_all(&outside_dir);
    }
}
