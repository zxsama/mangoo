//! File operations: rename, move, delete (recycle / permanent).
//!
//! Each public entry point performs:
//!   1. Source/target resolution from the database.
//!   2. Input validation (illegal name, path separator, traversal,
//!      Windows-reserved names, length).
//!   3. Conflict checks against the database and the filesystem.
//!   4. A `BEGIN IMMEDIATE` transaction that performs the FS mutation and
//!      DB updates as a single unit; FS errors roll back DB changes and
//!      record a `failed` row in `file_operations`.
//!   5. FTS sync for any affected files so search reflects renames.
//!
//! The contract surfaces operation kinds `rename`, `move`,
//! `delete_recycle`, `delete_permanent` — see
//! `contracts/core-api/openapi.yaml`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use uuid::Uuid;

use crate::db::{
    notification_outbox::{NotificationOperation, PublishMode},
    Database, Page,
};

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

#[derive(Debug)]
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
    let new_name = request.new_name.trim().to_string();
    validate_name(&new_name)?;

    let mut group = begin_operation_group(database, FileOperationKind::Rename, 1)?;

    let result = match (request.file_id, request.folder_id) {
        (Some(file_id), None) => rename_file(database, file_id, &new_name, &mut group),
        (None, Some(folder_id)) => rename_folder(database, folder_id, &new_name, &mut group),
        (Some(_), Some(_)) => Err(FsOpsError::new(
            FsOpsErrorCode::InvalidRequest,
            "exactly one of fileId or folderId must be provided",
        )),
        (None, None) => Err(FsOpsError::new(
            FsOpsErrorCode::InvalidRequest,
            "either fileId or folderId must be provided",
        )),
    };
    finish_operation_group(database, &mut group, result.as_ref().err())?;
    result
}

pub fn move_items(
    database: &Mutex<Database>,
    request: MoveRequest,
) -> FsOpsResult<Vec<FileOperationRecord>> {
    if request.file_ids.is_empty() && request.folder_ids.is_empty() {
        return Err(FsOpsError::new(
            FsOpsErrorCode::InvalidRequest,
            "at least one fileId or folderId must be provided",
        ));
    }

    let mut group = begin_operation_group(
        database,
        FileOperationKind::Move,
        request.file_ids.len() + request.folder_ids.len(),
    )?;

    let target = {
        let guard = database.lock().expect("database mutex poisoned");
        match load_folder_path(&guard.connection_for_fsops(), request.target_folder_id) {
            Ok(Some(target)) => target,
            Ok(None) => {
                let error = FsOpsError::new(
                    FsOpsErrorCode::NotFound,
                    format!("target folder not found: {}", request.target_folder_id),
                );
                drop(guard);
                finish_operation_group(database, &mut group, Some(&error))?;
                return Err(error);
            }
            Err(error) => {
                drop(guard);
                finish_operation_group(database, &mut group, Some(&error))?;
                return Err(error);
            }
        }
    };

    let mut records = Vec::new();

    for file_id in &request.file_ids {
        match move_file(database, *file_id, &target, &mut group) {
            Ok(record) => records.push(record),
            Err(error) => {
                finish_operation_group(database, &mut group, Some(&error))?;
                return Err(error);
            }
        }
    }
    for folder_id in &request.folder_ids {
        if *folder_id == request.target_folder_id {
            let error = FsOpsError::new(
                FsOpsErrorCode::InvalidRequest,
                format!("cannot move folder {folder_id} into itself"),
            );
            finish_operation_group(database, &mut group, Some(&error))?;
            return Err(error);
        }
        match move_folder(database, *folder_id, &target, &mut group) {
            Ok(record) => records.push(record),
            Err(error) => {
                finish_operation_group(database, &mut group, Some(&error))?;
                return Err(error);
            }
        }
    }

    finish_operation_group(database, &mut group, None)?;
    Ok(records)
}

pub fn delete(
    database: &Mutex<Database>,
    request: DeleteRequest,
) -> FsOpsResult<Vec<FileOperationRecord>> {
    if request.file_ids.is_empty() && request.folder_ids.is_empty() {
        return Err(FsOpsError::new(
            FsOpsErrorCode::InvalidRequest,
            "at least one fileId or folderId must be provided",
        ));
    }

    let mut group = begin_operation_group(
        database,
        if request.permanent {
            FileOperationKind::DeletePermanent
        } else {
            FileOperationKind::DeleteRecycle
        },
        request.file_ids.len() + request.folder_ids.len(),
    )?;
    let mut records = Vec::new();
    for file_id in &request.file_ids {
        match delete_file(database, *file_id, request.permanent, &mut group) {
            Ok(record) => records.push(record),
            Err(error) => {
                finish_operation_group(database, &mut group, Some(&error))?;
                return Err(error);
            }
        }
    }
    for folder_id in &request.folder_ids {
        match delete_folder(database, *folder_id, request.permanent, &mut group) {
            Ok(record) => records.push(record),
            Err(error) => {
                finish_operation_group(database, &mut group, Some(&error))?;
                return Err(error);
            }
        }
    }

    finish_operation_group(database, &mut group, None)?;
    Ok(records)
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
            "WHERE id < ?1"
        } else {
            ""
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

#[derive(Debug, Clone)]
struct FileOperationGroup {
    id: String,
    kind: FileOperationKind,
    total: i64,
    completed: i64,
    started_at: i64,
    terminal: bool,
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn begin_operation_group(
    database: &Mutex<Database>,
    kind: FileOperationKind,
    total: usize,
) -> FsOpsResult<FileOperationGroup> {
    let guard = database.lock().expect("database mutex poisoned");
    let transaction =
        Transaction::new_unchecked(guard.connection_for_fsops(), TransactionBehavior::Immediate)?;
    let group = FileOperationGroup {
        id: Uuid::now_v7().to_string(),
        kind,
        total: total as i64,
        completed: 0,
        started_at: now_millis(),
        terminal: false,
    };
    let mut operation = notification_for_group(&group, "pending", "queued", None, None);
    let appended = guard.publish_notification_operation_in_transaction(
        &transaction,
        &mut operation,
        PublishMode::Phase,
    )?;
    transaction.commit()?;
    if appended {
        guard.notification_outbox_wake().notify_one();
    }
    Ok(group)
}

fn notification_for_group(
    group: &FileOperationGroup,
    status: &str,
    phase: &str,
    error: Option<&FsOpsError>,
    completed_at: Option<i64>,
) -> NotificationOperation {
    let now = now_millis();
    NotificationOperation {
        operation_id: group.id.clone(),
        revision: 0,
        kind: group.kind.as_str().to_owned(),
        category: "file_operation".to_owned(),
        origin: "user".to_owned(),
        presentation: "important".to_owned(),
        status: status.to_owned(),
        phase: Some(phase.to_owned()),
        title: match group.kind {
            FileOperationKind::Rename => "Rename files",
            FileOperationKind::Move => "Move files",
            FileOperationKind::DeleteRecycle => "Move files to recycle bin",
            FileOperationKind::DeletePermanent => "Delete files",
        }
        .to_owned(),
        summary: None,
        task_id: None,
        root_id: None,
        folder_id: None,
        file_id: None,
        current_value: group.completed,
        total_value: Some(group.total),
        unit: Some("items".to_owned()),
        error_code: error.map(|error| error.code.as_str().to_owned()),
        error_detail: error.map(|error| error.message.clone()),
        cancellable: false,
        retryable: false,
        attempt: 1,
        occurrence_count: 1,
        started_at: group.started_at,
        updated_at: now,
        completed_at,
    }
}

fn publish_group_item_in_transaction(
    database: &Database,
    transaction: &Transaction<'_>,
    group: &FileOperationGroup,
    completed: i64,
    error: Option<&FsOpsError>,
) -> FsOpsResult<(bool, bool)> {
    let mut snapshot = group.clone();
    snapshot.completed = completed;
    let terminal = error.is_some() || completed >= group.total;
    let (status, phase, completed_at, mode) = match error {
        Some(_) => (
            "failed",
            "failed",
            Some(now_millis()),
            PublishMode::Terminal,
        ),
        None if terminal => (
            "succeeded",
            "completed",
            Some(now_millis()),
            PublishMode::Terminal,
        ),
        None => ("running", "working", None, PublishMode::Progress),
    };
    let mut operation = notification_for_group(&snapshot, status, phase, error, completed_at);
    let appended = database.publish_notification_operation_in_transaction(
        transaction,
        &mut operation,
        mode,
    )?;
    Ok((appended, terminal))
}

fn finish_operation_group(
    database: &Mutex<Database>,
    group: &mut FileOperationGroup,
    error: Option<&FsOpsError>,
) -> FsOpsResult<()> {
    if group.terminal {
        return Ok(());
    }
    let guard = database.lock().expect("database mutex poisoned");
    let transaction =
        Transaction::new_unchecked(guard.connection_for_fsops(), TransactionBehavior::Immediate)?;
    let (status, phase, completed_at) = match error {
        Some(_) => ("failed", "failed", Some(now_millis())),
        None => ("succeeded", "completed", Some(now_millis())),
    };
    let mut operation = notification_for_group(group, status, phase, error, completed_at);
    let appended = guard.publish_notification_operation_in_transaction(
        &transaction,
        &mut operation,
        PublishMode::Terminal,
    )?;
    transaction.commit()?;
    group.terminal = true;
    if appended {
        guard.notification_outbox_wake().notify_one();
    }
    Ok(())
}

// -------------- internal: rename ----------------------------------------

fn rename_file(
    database: &Mutex<Database>,
    file_id: i64,
    new_name: &str,
    group: &mut FileOperationGroup,
) -> FsOpsResult<FileOperationRecord> {
    // Step 1: brief lock to snapshot what we need for the FS rename and to
    // run the conflict check. We commit nothing here.
    let (info, source_path_string, target_path, target_path_string, conflict_in_db) = {
        let guard = database.lock().expect("database mutex poisoned");
        let conn = guard.connection_for_fsops();
        let info = match load_file_info(conn, file_id)? {
            Some(info) => info,
            None => {
                return Err(FsOpsError::new(
                    FsOpsErrorCode::NotFound,
                    format!("file not found: {file_id}"),
                ))
            }
        };
        let target_path = info.parent_dir.join(new_name);
        let source_path_string = info.path.to_string_lossy().to_string();
        let target_path_string = target_path.to_string_lossy().to_string();
        let conflict_in_db = file_exists_in_folder_conn(conn, info.folder_id, new_name)?;
        (
            info,
            source_path_string,
            target_path,
            target_path_string,
            conflict_in_db,
        )
    };

    if info.name == new_name {
        // Nothing to do; record an idempotent succeeded row.
        return write_succeeded_locked(
            database,
            FileOperationKind::Rename,
            &source_path_string,
            Some(&target_path_string),
            group,
        );
    }

    // The TOCTOU window between this check and `fs::rename` remains real on
    // Unix because `fs::rename` overwrites silently. `reserve_target_for_rename`
    // probes atomically on Unix (`O_CREATE_NEW`) before the actual rename.
    if conflict_in_db || target_path.exists() {
        return Err(FsOpsError::new(
            FsOpsErrorCode::NameConflict,
            format!("a file named '{new_name}' already exists in this folder"),
        ));
    }

    // Step 2: do the FS work with the lock released.
    let fs_result = atomic_rename(&info.path, &target_path);

    // Step 3: brief lock to commit DB changes and write the log row.
    let guard = database.lock().expect("database mutex poisoned");
    let conn = guard.connection_for_fsops();
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    if let Err(error) = fs_result {
        let operation_error =
            FsOpsError::new(FsOpsErrorCode::FsError, format!("rename failed: {error}"));
        let (appended, completed, terminal) = record_failure(
            &guard,
            &transaction,
            FileOperationKind::Rename,
            &source_path_string,
            Some(&target_path_string),
            &error.to_string(),
            &operation_error,
            group,
        )?;
        transaction.commit()?;
        group.completed = completed;
        group.terminal = terminal;
        if appended {
            guard.notification_outbox_wake().notify_one();
        }
        return Err(operation_error);
    }

    let new_ext = extension_from_name(new_name);
    transaction.execute(
        "UPDATE files SET name = ?1, ext = ?2 WHERE id = ?3",
        params![new_name, new_ext, file_id],
    )?;

    crate::db::sync_media_fts_for_file_in_transaction_pub(&transaction, file_id)?;

    let (record, appended, completed, terminal) = insert_operation(
        &guard,
        &transaction,
        FileOperationKind::Rename,
        &source_path_string,
        Some(&target_path_string),
        "succeeded",
        None,
        group,
    )?;
    transaction.commit()?;
    group.completed = completed;
    group.terminal = terminal;
    if appended {
        guard.notification_outbox_wake().notify_one();
    }
    Ok(record)
}

fn rename_folder(
    database: &Mutex<Database>,
    folder_id: i64,
    new_name: &str,
    group: &mut FileOperationGroup,
) -> FsOpsResult<FileOperationRecord> {
    // Step 1: snapshot under brief lock.
    let (info, parent_dir, target_path, source_path_string, target_path_string, conflict_in_db) = {
        let guard = database.lock().expect("database mutex poisoned");
        let conn = guard.connection_for_fsops();
        let info = match load_folder_info(conn, folder_id)? {
            Some(info) => info,
            None => {
                return Err(FsOpsError::new(
                    FsOpsErrorCode::NotFound,
                    format!("folder not found: {folder_id}"),
                ))
            }
        };
        if info.parent_id.is_none() {
            return Err(FsOpsError::new(
                FsOpsErrorCode::InvalidRequest,
                "cannot rename a root folder",
            ));
        }
        let parent_dir = info
            .path
            .parent()
            .ok_or_else(|| {
                FsOpsError::new(FsOpsErrorCode::InvalidRequest, "folder has no parent path")
            })?
            .to_path_buf();
        let target_path = parent_dir.join(new_name);
        let source_path_string = info.path.to_string_lossy().to_string();
        let target_path_string = target_path.to_string_lossy().to_string();
        let conflict_in_db =
            folder_name_exists_in_parent_conn(conn, info.parent_id, info.root_id, new_name)?;
        (
            info,
            parent_dir,
            target_path,
            source_path_string,
            target_path_string,
            conflict_in_db,
        )
    };
    let _ = parent_dir;

    if info.name == new_name {
        return write_succeeded_locked(
            database,
            FileOperationKind::Rename,
            &source_path_string,
            Some(&target_path_string),
            group,
        );
    }

    if conflict_in_db || target_path.exists() {
        return Err(FsOpsError::new(
            FsOpsErrorCode::NameConflict,
            format!("a folder named '{new_name}' already exists in this parent"),
        ));
    }

    // Step 2: FS work outside the lock.
    let fs_result = atomic_rename(&info.path, &target_path);

    // Step 3: brief lock for the commit.
    let guard = database.lock().expect("database mutex poisoned");
    let conn = guard.connection_for_fsops();
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    if let Err(error) = fs_result {
        let operation_error =
            FsOpsError::new(FsOpsErrorCode::FsError, format!("rename failed: {error}"));
        let (appended, completed, terminal) = record_failure(
            &guard,
            &transaction,
            FileOperationKind::Rename,
            &source_path_string,
            Some(&target_path_string),
            &error.to_string(),
            &operation_error,
            group,
        )?;
        transaction.commit()?;
        group.completed = completed;
        group.terminal = terminal;
        if appended {
            guard.notification_outbox_wake().notify_one();
        }
        return Err(operation_error);
    }

    transaction.execute(
        "UPDATE folders SET name = ?1 WHERE id = ?2",
        params![new_name, folder_id],
    )?;

    let descendant_files = collect_descendant_file_ids(&transaction, folder_id)?;
    for file_id in descendant_files {
        crate::db::sync_media_fts_for_file_in_transaction_pub(&transaction, file_id)?;
    }

    let (record, appended, completed, terminal) = insert_operation(
        &guard,
        &transaction,
        FileOperationKind::Rename,
        &source_path_string,
        Some(&target_path_string),
        "succeeded",
        None,
        group,
    )?;
    transaction.commit()?;
    group.completed = completed;
    group.terminal = terminal;
    if appended {
        guard.notification_outbox_wake().notify_one();
    }
    Ok(record)
}

// -------------- internal: move ------------------------------------------

fn move_file(
    database: &Mutex<Database>,
    file_id: i64,
    target: &FolderTarget,
    group: &mut FileOperationGroup,
) -> FsOpsResult<FileOperationRecord> {
    // Step 1: snapshot under brief lock. Fail-fast on cross-root and
    // collision; a same-folder move is treated as a no-op.
    let snapshot = {
        let guard = database.lock().expect("database mutex poisoned");
        let conn = guard.connection_for_fsops();
        let info = match load_file_info(conn, file_id)? {
            Some(info) => info,
            None => {
                return Err(FsOpsError::new(
                    FsOpsErrorCode::NotFound,
                    format!("file not found: {file_id}"),
                ))
            }
        };
        if info.root_id != target.root_id {
            return Err(FsOpsError::new(
                FsOpsErrorCode::CrossRoot,
                "cross_root: source and target are in different roots",
            ));
        }
        if info.folder_id == target.folder_id {
            let source = info.path.to_string_lossy().to_string();
            return write_succeeded_locked_with_guard(
                guard,
                FileOperationKind::Move,
                &source,
                Some(&source),
                group,
            );
        }
        let collision_in_db = file_exists_in_folder_conn(conn, target.folder_id, &info.name)?;
        (info, collision_in_db)
    };
    let (info, collision_in_db) = snapshot;

    let target_path = target.path.join(&info.name);
    if collision_in_db || target_path.exists() {
        return Err(FsOpsError::new(
            FsOpsErrorCode::NameConflict,
            format!(
                "a file named '{}' already exists in target folder",
                info.name
            ),
        ));
    }

    let source_path_string = info.path.to_string_lossy().to_string();
    let target_path_string = target_path.to_string_lossy().to_string();

    // Step 2: FS work outside the lock.
    let fs_result = atomic_rename(&info.path, &target_path);

    // Step 3: commit under brief lock.
    let guard = database.lock().expect("database mutex poisoned");
    let conn = guard.connection_for_fsops();
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    if let Err(error) = fs_result {
        let operation_error =
            FsOpsError::new(FsOpsErrorCode::FsError, format!("move failed: {error}"));
        let (appended, completed, terminal) = record_failure(
            &guard,
            &transaction,
            FileOperationKind::Move,
            &source_path_string,
            Some(&target_path_string),
            &error.to_string(),
            &operation_error,
            group,
        )?;
        transaction.commit()?;
        group.completed = completed;
        group.terminal = terminal;
        if appended {
            guard.notification_outbox_wake().notify_one();
        }
        return Err(operation_error);
    }

    transaction.execute(
        "UPDATE files SET folder_id = ?1 WHERE id = ?2",
        params![target.folder_id, file_id],
    )?;
    crate::db::sync_media_fts_for_file_in_transaction_pub(&transaction, file_id)?;

    let (record, appended, completed, terminal) = insert_operation(
        &guard,
        &transaction,
        FileOperationKind::Move,
        &source_path_string,
        Some(&target_path_string),
        "succeeded",
        None,
        group,
    )?;
    transaction.commit()?;
    group.completed = completed;
    group.terminal = terminal;
    if appended {
        guard.notification_outbox_wake().notify_one();
    }
    Ok(record)
}

fn move_folder(
    database: &Mutex<Database>,
    folder_id: i64,
    target: &FolderTarget,
    group: &mut FileOperationGroup,
) -> FsOpsResult<FileOperationRecord> {
    // Step 1: snapshot.
    let (info, target_path, source_path_string, target_path_string, conflict_in_db) = {
        let guard = database.lock().expect("database mutex poisoned");
        let conn = guard.connection_for_fsops();
        let info = match load_folder_info(conn, folder_id)? {
            Some(info) => info,
            None => {
                return Err(FsOpsError::new(
                    FsOpsErrorCode::NotFound,
                    format!("folder not found: {folder_id}"),
                ))
            }
        };
        if info.parent_id.is_none() {
            return Err(FsOpsError::new(
                FsOpsErrorCode::InvalidRequest,
                "cannot move a root folder",
            ));
        }
        if info.root_id != target.root_id {
            return Err(FsOpsError::new(
                FsOpsErrorCode::CrossRoot,
                "cross_root: source and target are in different roots",
            ));
        }
        if folder_is_ancestor_of(conn, folder_id, target.folder_id)? {
            return Err(FsOpsError::new(
                FsOpsErrorCode::InvalidRequest,
                "cannot move a folder into its own descendant",
            ));
        }
        let conflict_in_db = folder_name_exists_in_parent_conn(
            conn,
            Some(target.folder_id),
            info.root_id,
            &info.name,
        )?;
        let target_path = target.path.join(&info.name);
        let source_path_string = info.path.to_string_lossy().to_string();
        let target_path_string = target_path.to_string_lossy().to_string();
        (
            info,
            target_path,
            source_path_string,
            target_path_string,
            conflict_in_db,
        )
    };

    if conflict_in_db || target_path.exists() {
        return Err(FsOpsError::new(
            FsOpsErrorCode::NameConflict,
            format!(
                "a folder named '{}' already exists in target folder",
                info.name
            ),
        ));
    }

    // Step 2: FS work outside the lock.
    let fs_result = atomic_rename(&info.path, &target_path);

    // Step 3: commit under brief lock.
    let guard = database.lock().expect("database mutex poisoned");
    let conn = guard.connection_for_fsops();
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    if let Err(error) = fs_result {
        let operation_error =
            FsOpsError::new(FsOpsErrorCode::FsError, format!("move failed: {error}"));
        let (appended, completed, terminal) = record_failure(
            &guard,
            &transaction,
            FileOperationKind::Move,
            &source_path_string,
            Some(&target_path_string),
            &error.to_string(),
            &operation_error,
            group,
        )?;
        transaction.commit()?;
        group.completed = completed;
        group.terminal = terminal;
        if appended {
            guard.notification_outbox_wake().notify_one();
        }
        return Err(operation_error);
    }

    transaction.execute(
        "UPDATE folders SET parent_id = ?1 WHERE id = ?2",
        params![target.folder_id, folder_id],
    )?;
    let descendant_files = collect_descendant_file_ids(&transaction, folder_id)?;
    for file_id in descendant_files {
        crate::db::sync_media_fts_for_file_in_transaction_pub(&transaction, file_id)?;
    }

    let (record, appended, completed, terminal) = insert_operation(
        &guard,
        &transaction,
        FileOperationKind::Move,
        &source_path_string,
        Some(&target_path_string),
        "succeeded",
        None,
        group,
    )?;
    transaction.commit()?;
    group.completed = completed;
    group.terminal = terminal;
    if appended {
        guard.notification_outbox_wake().notify_one();
    }
    Ok(record)
}

// -------------- internal: delete ----------------------------------------

fn delete_file(
    database: &Mutex<Database>,
    file_id: i64,
    permanent: bool,
    group: &mut FileOperationGroup,
) -> FsOpsResult<FileOperationRecord> {
    // Step 1: snapshot path + symlink state under brief lock; the symlink
    // refusal must happen before the FS work even runs.
    let (info, source_path_string) = {
        let guard = database.lock().expect("database mutex poisoned");
        let conn = guard.connection_for_fsops();
        let info = match load_file_info(conn, file_id)? {
            Some(info) => info,
            None => {
                return Err(FsOpsError::new(
                    FsOpsErrorCode::NotFound,
                    format!("file not found: {file_id}"),
                ))
            }
        };
        if permanent {
            ensure_inside_enabled_root(conn, info.root_id, &info.path)?;
        }
        let source = info.path.to_string_lossy().to_string();
        (info, source)
    };

    let kind = if permanent {
        FileOperationKind::DeletePermanent
    } else {
        FileOperationKind::DeleteRecycle
    };

    if permanent {
        // Refuse to follow symlinks even though the path was inside an
        // enabled root: the link's *target* may not be. This check sits
        // outside the lock because it only reads the filesystem.
        refuse_if_symlink(&info.path)?;
    }

    // Step 2: FS work outside the lock.
    let fs_result = if permanent {
        fs::remove_file(&info.path).map_err(|error| error.to_string())
    } else {
        trash::delete(&info.path).map_err(|error| error.to_string())
    };

    // Step 3: commit under brief lock.
    let guard = database.lock().expect("database mutex poisoned");
    let conn = guard.connection_for_fsops();
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    if let Err(error) = fs_result {
        let operation_error =
            FsOpsError::new(FsOpsErrorCode::FsError, format!("delete failed: {error}"));
        let (appended, completed, terminal) = record_failure(
            &guard,
            &transaction,
            kind,
            &source_path_string,
            None,
            &error,
            &operation_error,
            group,
        )?;
        transaction.commit()?;
        group.completed = completed;
        group.terminal = terminal;
        if appended {
            guard.notification_outbox_wake().notify_one();
        }
        return Err(operation_error);
    }

    transaction.execute(
        "UPDATE files SET status = 'deleted' WHERE id = ?1",
        params![file_id],
    )?;
    transaction.execute("DELETE FROM media_fts WHERE rowid = ?1", params![file_id])?;
    crate::db::cleanup_inactive_thumbnail_artifacts_in_transaction(&transaction)?;

    let (record, appended, completed, terminal) = insert_operation(
        &guard,
        &transaction,
        kind,
        &source_path_string,
        None,
        "succeeded",
        None,
        group,
    )?;
    transaction.commit()?;
    group.completed = completed;
    group.terminal = terminal;
    if appended {
        guard.notification_outbox_wake().notify_one();
    }
    Ok(record)
}

fn delete_folder(
    database: &Mutex<Database>,
    folder_id: i64,
    permanent: bool,
    group: &mut FileOperationGroup,
) -> FsOpsResult<FileOperationRecord> {
    // Step 1: snapshot.
    let (info, source_path_string) = {
        let guard = database.lock().expect("database mutex poisoned");
        let conn = guard.connection_for_fsops();
        let info = match load_folder_info(conn, folder_id)? {
            Some(info) => info,
            None => {
                return Err(FsOpsError::new(
                    FsOpsErrorCode::NotFound,
                    format!("folder not found: {folder_id}"),
                ))
            }
        };
        if info.parent_id.is_none() {
            return Err(FsOpsError::new(
                FsOpsErrorCode::InvalidRequest,
                "cannot delete a root folder",
            ));
        }
        if permanent {
            ensure_inside_enabled_root(conn, info.root_id, &info.path)?;
        }
        let source = info.path.to_string_lossy().to_string();
        (info, source)
    };

    let kind = if permanent {
        FileOperationKind::DeletePermanent
    } else {
        FileOperationKind::DeleteRecycle
    };

    if permanent {
        refuse_if_symlink(&info.path)?;
    }

    // Step 2: FS work outside the lock.
    let fs_result = if permanent {
        fs::remove_dir_all(&info.path).map_err(|error| error.to_string())
    } else {
        trash::delete(&info.path).map_err(|error| error.to_string())
    };

    // Step 3: commit under brief lock.
    let guard = database.lock().expect("database mutex poisoned");
    let conn = guard.connection_for_fsops();
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    if let Err(error) = fs_result {
        let operation_error =
            FsOpsError::new(FsOpsErrorCode::FsError, format!("delete failed: {error}"));
        let (appended, completed, terminal) = record_failure(
            &guard,
            &transaction,
            kind,
            &source_path_string,
            None,
            &error,
            &operation_error,
            group,
        )?;
        transaction.commit()?;
        group.completed = completed;
        group.terminal = terminal;
        if appended {
            guard.notification_outbox_wake().notify_one();
        }
        return Err(operation_error);
    }

    let descendant_files = collect_descendant_file_ids(&transaction, folder_id)?;
    transaction.execute(
        r#"
        WITH RECURSIVE subtree(id) AS (
          SELECT id FROM folders WHERE id = ?1
          UNION ALL
          SELECT folders.id
          FROM folders
          JOIN subtree ON folders.parent_id = subtree.id
        )
        UPDATE folders SET status = 'deleted'
        WHERE id IN (SELECT id FROM subtree)
        "#,
        params![folder_id],
    )?;
    transaction.execute(
        r#"
        WITH RECURSIVE subtree(id) AS (
          SELECT id FROM folders WHERE id = ?1
          UNION ALL
          SELECT folders.id
          FROM folders
          JOIN subtree ON folders.parent_id = subtree.id
        )
        UPDATE files SET status = 'deleted'
        WHERE folder_id IN (SELECT id FROM subtree)
        "#,
        params![folder_id],
    )?;
    crate::db::cleanup_inactive_thumbnail_artifacts_in_transaction(&transaction)?;
    for file_id in &descendant_files {
        transaction.execute("DELETE FROM media_fts WHERE rowid = ?1", params![*file_id])?;
    }

    let (record, appended, completed, terminal) = insert_operation(
        &guard,
        &transaction,
        kind,
        &source_path_string,
        None,
        "succeeded",
        None,
        group,
    )?;
    transaction.commit()?;
    group.completed = completed;
    group.terminal = terminal;
    if appended {
        guard.notification_outbox_wake().notify_one();
    }
    Ok(record)
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

fn ensure_inside_enabled_root(
    conn: &rusqlite::Connection,
    root_id: i64,
    path: &Path,
) -> FsOpsResult<()> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT path, enabled FROM roots WHERE id = ?1",
            params![root_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let Some((root_path, enabled)) = row else {
        return Err(FsOpsError::new(
            FsOpsErrorCode::OutsideRoot,
            "root not found",
        ));
    };
    if enabled == 0 {
        return Err(FsOpsError::new(
            FsOpsErrorCode::OutsideRoot,
            "permanent delete is not allowed in disabled roots",
        ));
    }
    // Prefer canonicalized comparison so a symlink inside the root that
    // points outside cannot smuggle a permanent delete past the boundary.
    // If either side fails to canonicalize (e.g. the entry already
    // disappeared), fall back to the raw prefix check; the caller's
    // `refuse_if_symlink` guard catches the more dangerous symlink case.
    let prefix_ok = match (fs::canonicalize(path), fs::canonicalize(&root_path)) {
        (Ok(canonical_path), Ok(canonical_root)) => canonical_path.starts_with(&canonical_root),
        _ => path.starts_with(&root_path),
    };
    if !prefix_ok {
        return Err(FsOpsError::new(
            FsOpsErrorCode::OutsideRoot,
            format!(
                "permanent delete refused: path is outside root {}",
                root_path
            ),
        ));
    }
    Ok(())
}

/// Refuse to permanently delete an entry whose `symlink_metadata` reports a
/// symlink. We never want to follow a link out of the user's root; the
/// canonicalize check in `ensure_inside_enabled_root` is best-effort, but
/// this guard returns a clear error code so the UI can render it.
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

fn insert_operation(
    database: &Database,
    transaction: &Transaction<'_>,
    kind: FileOperationKind,
    source_path: &str,
    target_path: Option<&str>,
    status: &str,
    error: Option<&str>,
    group: &FileOperationGroup,
) -> FsOpsResult<(FileOperationRecord, bool, i64, bool)> {
    transaction.execute(
        r#"
        INSERT INTO file_operations(
          operation, source_path, target_path, status, created_at, finished_at, error,
          operation_group_id
        )
        VALUES (?1, ?2, ?3, ?4, unixepoch(), unixepoch(), ?5, ?6)
        "#,
        params![
            kind.as_str(),
            source_path,
            target_path,
            status,
            error,
            group.id
        ],
    )?;
    let id = transaction.last_insert_rowid();
    let record = transaction.query_row(
        r#"
        SELECT id, operation, source_path, target_path, status, created_at, finished_at, error,
               operation_group_id
        FROM file_operations
        WHERE id = ?1
        "#,
        params![id],
        record_from_row,
    )?;
    let completed = group.completed.saturating_add(1);
    let (appended, terminal) =
        publish_group_item_in_transaction(database, transaction, group, completed, None)?;
    Ok((record, appended, completed, terminal))
}

fn record_failure(
    database: &Database,
    transaction: &Transaction<'_>,
    kind: FileOperationKind,
    source_path: &str,
    target_path: Option<&str>,
    error: &str,
    operation_error: &FsOpsError,
    group: &FileOperationGroup,
) -> FsOpsResult<(bool, i64, bool)> {
    transaction.execute(
        r#"
        INSERT INTO file_operations(
          operation, source_path, target_path, status, created_at, finished_at, error,
          operation_group_id
        )
        VALUES (?1, ?2, ?3, 'failed', unixepoch(), unixepoch(), ?4, ?5)
        "#,
        params![kind.as_str(), source_path, target_path, error, group.id],
    )?;
    let completed = group.completed.saturating_add(1);
    let (appended, terminal) = publish_group_item_in_transaction(
        database,
        transaction,
        group,
        completed,
        Some(operation_error),
    )?;
    Ok((appended, completed, terminal))
}

/// Acquire the lock briefly, write a `succeeded` `file_operations` row,
/// and release. Used for idempotent no-op paths (e.g. rename to same name,
/// move into the same folder).
fn write_succeeded_locked(
    database: &Mutex<Database>,
    kind: FileOperationKind,
    source_path: &str,
    target_path: Option<&str>,
    group: &mut FileOperationGroup,
) -> FsOpsResult<FileOperationRecord> {
    let guard = database.lock().expect("database mutex poisoned");
    write_succeeded_locked_with_guard(guard, kind, source_path, target_path, group)
}

/// Variant that operates on an already-held guard so callers that already
/// snapshotted under a single lock can write the success row without
/// dropping and re-acquiring.
fn write_succeeded_locked_with_guard<G>(
    guard: G,
    kind: FileOperationKind,
    source_path: &str,
    target_path: Option<&str>,
    group: &mut FileOperationGroup,
) -> FsOpsResult<FileOperationRecord>
where
    G: std::ops::DerefMut<Target = Database>,
{
    let conn = guard.connection_for_fsops();
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let (record, appended, completed, terminal) = insert_operation(
        &guard,
        &transaction,
        kind,
        source_path,
        target_path,
        "succeeded",
        None,
        group,
    )?;
    transaction.commit()?;
    group.completed = completed;
    group.terminal = terminal;
    if appended {
        guard.notification_outbox_wake().notify_one();
    }
    Ok(record)
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
    }

    impl TestEnv {
        fn new(label: &str) -> Self {
            let root_path = unique_temp_dir(label);
            stdfs::create_dir_all(&root_path).expect("create root path");
            let database = Database::open_in_memory().expect("open database");
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
            }
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
        let _conflict = env.add_file(target, "second.jpg");

        let error = move_items(
            &env.database,
            MoveRequest {
                file_ids: vec![first, second],
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
        assert_eq!(operation.current_value, 1);
        assert_eq!(operation.total_value, Some(2));
        assert_eq!(operation.error_code.as_deref(), Some("name_conflict"));
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
    }

    #[test]
    fn filesystem_failure_persists_failed_audit_and_terminal_in_one_group() {
        let env = TestEnv::new("delete_filesystem_failure");
        let file_id = env.add_file(env.root_folder_id, "missing.jpg");
        stdfs::remove_file(env.root_path.join("missing.jpg")).expect("remove source from disk");

        let error = delete(
            &env.database,
            DeleteRequest {
                file_ids: vec![file_id],
                folder_ids: vec![],
                permanent: true,
            },
        )
        .expect_err("filesystem delete must fail");
        assert_eq!(error.code, FsOpsErrorCode::FsError);

        let page = list_recent(&env.database, 10, None).expect("recent file operations");
        let audit = page.items.first().expect("failed audit row");
        assert_eq!(audit.status, "failed");
        let group_id = audit
            .operation_group_id
            .as_deref()
            .expect("failed operation group id");
        let guard = env.database.lock().expect("db lock");
        let operation = guard
            .notification_heads_for_test()
            .expect("notification heads")
            .into_iter()
            .find(|operation| operation.operation_id == group_id)
            .expect("failed operation head");
        assert_eq!(operation.status, "failed");
        assert_eq!(operation.current_value, 1);
        assert_eq!(operation.total_value, Some(1));
        assert_eq!(operation.error_code.as_deref(), Some("fs_error"));
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
        let group = begin_operation_group(&env.database, FileOperationKind::Move, 2)
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
            .find(|operation| operation.operation_id == group.id)
            .expect("recovered group");
        assert_eq!(operation.status, "failed");
        assert_eq!(operation.origin, "recovery");
        assert_eq!(operation.error_code.as_deref(), Some("interrupted"));
        assert_eq!(operation.current_value, 0);
        assert_eq!(operation.total_value, Some(2));
        assert!(operation.completed_at.is_some());
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
