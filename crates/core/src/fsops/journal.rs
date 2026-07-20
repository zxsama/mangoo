use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use uuid::Uuid;

use super::*;
use crate::db::notification_outbox::{NotificationOperation, PublishMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntityKind {
    File,
    Folder,
}

impl EntityKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Folder => "folder",
        }
    }

    fn parse(value: &str) -> FsOpsResult<Self> {
        match value {
            "file" => Ok(Self::File),
            "folder" => Ok(Self::Folder),
            _ => Err(FsOpsError::new(
                FsOpsErrorCode::FsError,
                format!("unsupported file-operation entity kind: {value}"),
            )),
        }
    }
}

#[derive(Debug)]
struct PreparedIntent {
    kind: FileOperationKind,
    entity_kind: Option<EntityKind>,
    entity_id: Option<i64>,
    root_id: Option<i64>,
    source_path: String,
    target_path: Option<String>,
    target_folder_id: Option<i64>,
    delete_root_path: Option<PathBuf>,
    source_identity: Option<String>,
    preflight_error: Option<FsOpsError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalPhase {
    Legacy,
    Prepared,
    Executing,
}

impl JournalPhase {
    fn parse(value: &str) -> FsOpsResult<Self> {
        match value {
            "legacy" => Ok(Self::Legacy),
            "prepared" => Ok(Self::Prepared),
            "executing" => Ok(Self::Executing),
            _ => Err(FsOpsError::new(
                FsOpsErrorCode::FsError,
                format!("unsupported file-operation journal phase: {value}"),
            )),
        }
    }
}

#[derive(Debug, Clone)]
struct JournalIntent {
    id: i64,
    group_id: String,
    kind: FileOperationKind,
    entity_kind: Option<EntityKind>,
    entity_id: Option<i64>,
    root_id: Option<i64>,
    source_path: PathBuf,
    target_path: Option<PathBuf>,
    target_folder_id: Option<i64>,
    attempt_count: i64,
    phase: JournalPhase,
    source_identity: Option<String>,
    pending_error_code: Option<String>,
    pending_error_detail: Option<String>,
}

#[derive(Debug)]
enum PathProbe {
    Exists(fs::Metadata),
    NotFound,
    Error(std::io::Error),
}

pub(super) fn rename(
    database: &Mutex<Database>,
    request: RenameRequest,
) -> FsOpsResult<FileOperationRecord> {
    let prepared = prepare_rename(database, request)?;
    let mut records = execute_prepared(database, FileOperationKind::Rename, vec![prepared])?;
    records.pop().ok_or_else(|| {
        FsOpsError::new(
            FsOpsErrorCode::FsError,
            "rename completed without an audit row",
        )
    })
}

pub(super) fn move_items(
    database: &Mutex<Database>,
    request: MoveRequest,
) -> FsOpsResult<Vec<FileOperationRecord>> {
    // An empty item set is an invalid API command, not a logical operation;
    // reject it before allocating a group id or writing an audit row.
    if request.file_ids.is_empty() && request.folder_ids.is_empty() {
        return Err(FsOpsError::new(
            FsOpsErrorCode::InvalidRequest,
            "at least one fileId or folderId must be provided",
        ));
    }
    let prepared = prepare_move(database, &request)?;
    execute_prepared(database, FileOperationKind::Move, prepared)
}

pub(super) fn delete(
    database: &Mutex<Database>,
    request: DeleteRequest,
) -> FsOpsResult<Vec<FileOperationRecord>> {
    // As with move, an empty item set has no operation to journal.
    if request.file_ids.is_empty() && request.folder_ids.is_empty() {
        return Err(FsOpsError::new(
            FsOpsErrorCode::InvalidRequest,
            "at least one fileId or folderId must be provided",
        ));
    }
    let kind = if request.permanent {
        FileOperationKind::DeletePermanent
    } else {
        FileOperationKind::DeleteRecycle
    };
    let prepared = prepare_delete(database, &request, kind)?;
    execute_prepared(database, kind, prepared)
}

pub(crate) fn recover_pending_file_operations(database: &Database) -> FsOpsResult<usize> {
    let intents = load_pending_intents(database)?;
    let mut recovered = 0usize;
    for mut intent in intents {
        match reconcile_intent(database, &mut intent)? {
            Ok(_) | Err(_) => recovered += 1,
        }
    }
    Ok(recovered)
}

#[cfg(test)]
pub(super) fn begin_test_group(
    database: &Mutex<Database>,
    kind: FileOperationKind,
    total: i64,
) -> FsOpsResult<String> {
    let guard = database.lock().expect("database mutex poisoned");
    let transaction =
        Transaction::new_unchecked(guard.connection_for_fsops(), TransactionBehavior::Immediate)?;
    let group_id = Uuid::now_v7().to_string();
    let mut operation = new_group_operation(&group_id, kind, total, now_millis());
    let appended = guard.publish_notification_operation_in_transaction(
        &transaction,
        &mut operation,
        PublishMode::Phase,
    )?;
    transaction.commit()?;
    if appended {
        guard.notification_outbox_wake().notify_one();
    }
    Ok(group_id)
}

fn execute_prepared(
    database: &Mutex<Database>,
    kind: FileOperationKind,
    prepared: Vec<PreparedIntent>,
) -> FsOpsResult<Vec<FileOperationRecord>> {
    let mut intents = persist_intents(database, kind, prepared)?;
    let mut records = Vec::with_capacity(intents.len());
    let mut first_error = None;

    for intent in &mut intents {
        match execute_intent_locked(database, intent)? {
            Ok(record) => records.push(record),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }

    if let Some(error) = first_error {
        Err(error)
    } else {
        Ok(records)
    }
}

fn execute_intent_locked(
    database: &Mutex<Database>,
    intent: &mut JournalIntent,
) -> FsOpsResult<FsOpsResult<FileOperationRecord>> {
    if let Some(code) = intent.pending_error_code.as_deref() {
        let error = persisted_error(code, intent.pending_error_detail.as_deref());
        let guard = database.lock().expect("database mutex poisoned");
        finalize_intent(&guard, intent, Some(&error))?;
        return Ok(Err(error));
    }

    if intent.source_path == intent.target_path.as_deref().unwrap_or(Path::new("")) {
        let guard = database.lock().expect("database mutex poisoned");
        return finalize_intent(&guard, intent, None).map(Ok);
    }

    {
        let guard = database.lock().expect("database mutex poisoned");
        mark_attempt(&guard, intent)?;
    }
    let fs_result = perform_filesystem_action(intent);
    let guard = database.lock().expect("database mutex poisoned");
    match fs_result {
        Ok(()) => finalize_intent(&guard, intent, None).map(Ok),
        Err(error) => {
            let operation_error = filesystem_operation_error(intent, error);
            finalize_intent(&guard, intent, Some(&operation_error))?;
            Ok(Err(operation_error))
        }
    }
}

fn execute_intent(
    database: &Database,
    intent: &mut JournalIntent,
) -> FsOpsResult<FsOpsResult<FileOperationRecord>> {
    if let Some(code) = intent.pending_error_code.as_deref() {
        let error = persisted_error(code, intent.pending_error_detail.as_deref());
        finalize_intent(database, intent, Some(&error))?;
        return Ok(Err(error));
    }

    if intent.source_path == intent.target_path.as_deref().unwrap_or(Path::new("")) {
        return finalize_intent(database, intent, None).map(Ok);
    }

    mark_attempt(database, intent)?;
    match perform_filesystem_action(intent) {
        Ok(()) => finalize_intent(database, intent, None).map(Ok),
        Err(error) => {
            let operation_error = filesystem_operation_error(intent, error);
            finalize_intent(database, intent, Some(&operation_error))?;
            Ok(Err(operation_error))
        }
    }
}

fn reconcile_intent(
    database: &Database,
    intent: &mut JournalIntent,
) -> FsOpsResult<FsOpsResult<FileOperationRecord>> {
    if let Some(code) = intent.pending_error_code.as_deref() {
        let error = persisted_error(code, intent.pending_error_detail.as_deref());
        finalize_intent(database, intent, Some(&error))?;
        return Ok(Err(error));
    }
    if intent.entity_kind.is_none() || intent.entity_id.is_none() {
        let error = FsOpsError::new(
            FsOpsErrorCode::FsError,
            "pending file operation is missing replay metadata",
        );
        finalize_intent(database, intent, Some(&error))?;
        return Ok(Err(error));
    }

    match reconcile_filesystem_state(database, intent) {
        ReconcileAction::FinalizeSuccess => finalize_intent(database, intent, None).map(Ok),
        ReconcileAction::Retry => execute_intent(database, intent),
        ReconcileAction::Fail(error) => {
            finalize_intent(database, intent, Some(&error))?;
            Ok(Err(error))
        }
    }
}

enum ReconcileAction {
    FinalizeSuccess,
    Retry,
    Fail(FsOpsError),
}

fn reconcile_filesystem_state(database: &Database, intent: &JournalIntent) -> ReconcileAction {
    match intent.kind {
        FileOperationKind::DeleteRecycle | FileOperationKind::DeletePermanent => {
            reconcile_delete_filesystem_state(database, intent, probe_path(&intent.source_path))
        }
        FileOperationKind::Rename | FileOperationKind::Move => {
            let Some(target_path) = intent.target_path.as_deref() else {
                return ReconcileAction::Fail(FsOpsError::new(
                    FsOpsErrorCode::FsError,
                    "pending rename/move operation is missing its target path",
                ));
            };
            reconcile_move_filesystem_state(
                database,
                intent,
                target_path,
                probe_path(&intent.source_path),
                probe_path(target_path),
            )
        }
    }
}

fn reconcile_delete_filesystem_state(
    database: &Database,
    intent: &JournalIntent,
    source: PathProbe,
) -> ReconcileAction {
    match source {
        PathProbe::Error(error) => ReconcileAction::Fail(probe_interrupted_error(
            &intent.source_path,
            "delete source",
            error,
        )),
        PathProbe::NotFound if intent.phase == JournalPhase::Executing => {
            match verify_entity_snapshot(database, intent) {
                Ok(()) => ReconcileAction::FinalizeSuccess,
                Err(error) => ReconcileAction::Fail(error),
            }
        }
        PathProbe::NotFound => ReconcileAction::Fail(interrupted_error(format!(
            "delete source '{}' disappeared before filesystem execution was durably started",
            intent.source_path.display()
        ))),
        PathProbe::Exists(_) if intent.phase != JournalPhase::Prepared => {
            ReconcileAction::Fail(interrupted_error(format!(
                "delete source '{}' still exists after filesystem execution may have started; refusing destructive replay",
                intent.source_path.display()
            )))
        }
        PathProbe::Exists(metadata) => {
            let validation = if intent.kind == FileOperationKind::DeletePermanent {
                validate_permanent_delete_replay(database, intent, &metadata)
            } else {
                validate_prepared_source_replay(database, intent, &metadata)
            };
            match validation {
                Ok(()) => ReconcileAction::Retry,
                Err(error) => ReconcileAction::Fail(error),
            }
        }
    }
}

fn reconcile_move_filesystem_state(
    database: &Database,
    intent: &JournalIntent,
    target_path: &Path,
    source: PathProbe,
    target: PathProbe,
) -> ReconcileAction {
    if let PathProbe::Error(error) = source {
        return ReconcileAction::Fail(probe_interrupted_error(
            &intent.source_path,
            "source",
            error,
        ));
    }
    if let PathProbe::Error(error) = target {
        return ReconcileAction::Fail(probe_interrupted_error(target_path, "target", error));
    }
    if intent.phase == JournalPhase::Legacy {
        return ReconcileAction::Fail(interrupted_error(
            "legacy pending file operation has no durable execution phase",
        ));
    }
    if intent.source_path == target_path {
        return match source {
            PathProbe::Exists(metadata) => {
                match validate_prepared_source_replay(database, intent, &metadata) {
                    Ok(()) => ReconcileAction::FinalizeSuccess,
                    Err(error) => ReconcileAction::Fail(error),
                }
            }
            PathProbe::NotFound => ReconcileAction::Fail(FsOpsError::new(
                FsOpsErrorCode::NotFound,
                "idempotent rename/move source is missing during recovery",
            )),
            PathProbe::Error(_) => unreachable!("probe errors returned above"),
        };
    }
    match (source, target, intent.phase) {
        (PathProbe::NotFound, PathProbe::Exists(target_metadata), JournalPhase::Executing) => {
            let validation = verify_entity_snapshot(database, intent)
                .and_then(|()| verify_source_identity(intent, target_path, &target_metadata));
            match validation {
                Ok(()) => ReconcileAction::FinalizeSuccess,
                Err(error) => ReconcileAction::Fail(error),
            }
        }
        (PathProbe::Exists(source_metadata), PathProbe::NotFound, JournalPhase::Prepared) => {
            match validate_prepared_source_replay(database, intent, &source_metadata) {
                Ok(()) => ReconcileAction::Retry,
                Err(error) => ReconcileAction::Fail(error),
            }
        }
        (PathProbe::Exists(_), PathProbe::NotFound, JournalPhase::Executing) => {
            ReconcileAction::Fail(interrupted_error(
                "source still exists after rename/move execution may have started; refusing replay",
            ))
        }
        (PathProbe::NotFound, PathProbe::Exists(_), JournalPhase::Prepared) => {
            ReconcileAction::Fail(interrupted_error(
                "prepared rename/move source disappeared before execution started",
            ))
        }
        (PathProbe::Exists(_), PathProbe::Exists(_), _) => ReconcileAction::Fail(FsOpsError::new(
            FsOpsErrorCode::NameConflict,
            "source and target both exist during file-operation recovery",
        )),
        (PathProbe::NotFound, PathProbe::NotFound, _) => ReconcileAction::Fail(FsOpsError::new(
            FsOpsErrorCode::NotFound,
            "source and target are both missing during file-operation recovery",
        )),
        (PathProbe::Error(_), _, _) | (_, PathProbe::Error(_), _) => {
            unreachable!("probe errors returned above")
        }
        (_, _, JournalPhase::Legacy) => unreachable!("legacy phase returned above"),
    }
}

fn probe_path(path: &Path) -> PathProbe {
    match fs::symlink_metadata(path) {
        Ok(metadata) => PathProbe::Exists(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => PathProbe::NotFound,
        Err(error) => PathProbe::Error(error),
    }
}

fn apply_target_path_preflight(intent: &mut PreparedIntent, message: &str) {
    if intent.preflight_error.is_some() {
        return;
    }
    let Some(target) = intent.target_path.as_deref().map(Path::new) else {
        return;
    };
    if Path::new(&intent.source_path) == target {
        return;
    }
    intent.preflight_error = match probe_path(target) {
        PathProbe::Exists(_) => Some(FsOpsError::new(FsOpsErrorCode::NameConflict, message)),
        PathProbe::NotFound => None,
        PathProbe::Error(error) => Some(FsOpsError::new(
            FsOpsErrorCode::FsError,
            format!(
                "cannot inspect file-operation target '{}': {error}",
                target.display()
            ),
        )),
    };
}

fn probe_interrupted_error(path: &Path, role: &str, error: std::io::Error) -> FsOpsError {
    interrupted_error(format!(
        "cannot determine {role} state for '{}': {error}",
        path.display()
    ))
}

fn interrupted_error(message: impl Into<String>) -> FsOpsError {
    FsOpsError::new(FsOpsErrorCode::Interrupted, message)
}

fn verify_entity_snapshot(database: &Database, intent: &JournalIntent) -> FsOpsResult<()> {
    let entity_kind = intent
        .entity_kind
        .ok_or_else(|| interrupted_error("pending file operation is missing its entity kind"))?;
    let entity_id = intent
        .entity_id
        .ok_or_else(|| interrupted_error("pending file operation is missing its entity id"))?;
    let (root_id, path) =
        load_journal_entity_snapshot(database.connection_for_fsops(), entity_kind, entity_id)?
            .ok_or_else(|| {
                interrupted_error("journal entity is deleted or absent from the index")
            })?;
    if intent.root_id != Some(root_id) || intent.source_path != path {
        return Err(interrupted_error(format!(
            "journal entity no longer resolves to its recorded source '{}'",
            intent.source_path.display()
        )));
    }
    if intent.kind == FileOperationKind::Move {
        let target_folder_id = required_target_folder(intent)?;
        let target = load_folder_path(database.connection_for_fsops(), target_folder_id)?
            .ok_or_else(|| interrupted_error("move target folder is no longer active"))?;
        let expected_parent = intent
            .target_path
            .as_deref()
            .and_then(Path::parent)
            .ok_or_else(|| interrupted_error("move target path has no parent"))?;
        if target.root_id != root_id || target.path != expected_parent {
            return Err(interrupted_error(
                "move target no longer matches the recorded journal destination",
            ));
        }
    }
    Ok(())
}

fn load_journal_entity_snapshot(
    conn: &rusqlite::Connection,
    entity_kind: EntityKind,
    entity_id: i64,
) -> FsOpsResult<Option<(i64, PathBuf)>> {
    match entity_kind {
        EntityKind::File => {
            let row: Option<(i64, i64, String)> = conn
                .query_row(
                    r#"SELECT folder_id, root_id, name
                       FROM files WHERE id = ?1 AND status <> 'deleted'"#,
                    [entity_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((folder_id, root_id, name)) = row else {
                return Ok(None);
            };
            Ok(folder_path_for(conn, folder_id)?.map(|parent| (root_id, parent.join(name))))
        }
        EntityKind::Folder => {
            let root_id: Option<i64> = conn
                .query_row(
                    "SELECT root_id FROM folders WHERE id = ?1 AND status <> 'deleted'",
                    [entity_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(root_id) = root_id else {
                return Ok(None);
            };
            Ok(folder_path_for(conn, entity_id)?.map(|path| (root_id, path)))
        }
    }
}

fn validate_prepared_source_replay(
    database: &Database,
    intent: &JournalIntent,
    metadata: &fs::Metadata,
) -> FsOpsResult<()> {
    verify_entity_snapshot(database, intent)?;
    verify_source_identity(intent, &intent.source_path, metadata)
}

fn verify_source_identity(
    intent: &JournalIntent,
    probed_path: &Path,
    metadata: &fs::Metadata,
) -> FsOpsResult<()> {
    let expected = intent.source_identity.as_deref().ok_or_else(|| {
        interrupted_error("pending file operation has no durable source identity")
    })?;
    let actual = filesystem_identity(probed_path, metadata)
        .ok_or_else(|| interrupted_error("filesystem did not provide a stable source identity"))?;
    if actual != expected {
        return Err(interrupted_error(
            "file-operation source identity changed before recovery",
        ));
    }
    Ok(())
}

fn validate_permanent_delete_replay(
    database: &Database,
    intent: &JournalIntent,
    metadata: &fs::Metadata,
) -> FsOpsResult<()> {
    verify_entity_snapshot(database, intent)?;
    let root_id = intent
        .root_id
        .ok_or_else(|| interrupted_error("permanent-delete intent has no root id"))?;
    let root_path = enabled_root_path(database.connection_for_fsops(), root_id)?;
    validate_permanent_delete_path(
        &intent.source_path,
        &root_path,
        metadata,
        intent.source_identity.as_deref(),
    )?;
    Ok(())
}

fn validate_permanent_delete_path(
    source_path: &Path,
    root_path: &Path,
    metadata: &fs::Metadata,
    expected_identity: Option<&str>,
) -> FsOpsResult<Option<String>> {
    refuse_if_link_or_reparse_point(source_path, metadata)?;
    let canonical_source = fs::canonicalize(source_path).map_err(|error| {
        FsOpsError::new(
            FsOpsErrorCode::FsError,
            format!(
                "cannot canonicalize delete source '{}': {error}",
                source_path.display()
            ),
        )
    })?;
    let canonical_root = fs::canonicalize(root_path).map_err(|error| {
        FsOpsError::new(
            FsOpsErrorCode::FsError,
            format!(
                "cannot canonicalize delete root '{}': {error}",
                root_path.display()
            ),
        )
    })?;
    if !canonical_source.starts_with(&canonical_root) {
        return Err(FsOpsError::new(
            FsOpsErrorCode::OutsideRoot,
            format!(
                "permanent delete refused: path is outside root {}",
                root_path.display()
            ),
        ));
    }
    let identity = filesystem_identity(source_path, metadata);
    if let Some(expected) = expected_identity {
        if identity.as_deref() != Some(expected) {
            return Err(interrupted_error(
                "permanent-delete source identity changed before recovery",
            ));
        }
    }
    Ok(identity)
}

#[cfg(windows)]
fn filesystem_identity(path: &Path, _metadata: &fs::Metadata) -> Option<String> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information as *mut _) }
            != 0;
    if !succeeded {
        return None;
    }
    let file_index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Some(format!(
        "windows:{}:{file_index}",
        information.dwVolumeSerialNumber
    ))
}

#[cfg(unix)]
fn filesystem_identity(_path: &Path, metadata: &fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(any(windows, unix)))]
fn filesystem_identity(_path: &Path, _metadata: &fs::Metadata) -> Option<String> {
    None
}

fn perform_filesystem_action(intent: &JournalIntent) -> std::io::Result<()> {
    match intent.kind {
        FileOperationKind::Rename | FileOperationKind::Move => atomic_rename(
            &intent.source_path,
            intent.target_path.as_deref().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing target path")
            })?,
        ),
        FileOperationKind::DeletePermanent => match intent.entity_kind {
            Some(EntityKind::File) => fs::remove_file(&intent.source_path),
            Some(EntityKind::Folder) => fs::remove_dir_all(&intent.source_path),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "missing delete entity kind",
            )),
        },
        FileOperationKind::DeleteRecycle => trash::delete(&intent.source_path)
            .map_err(|error| std::io::Error::other(error.to_string())),
    }
}

fn filesystem_operation_error(intent: &JournalIntent, error: std::io::Error) -> FsOpsError {
    let code = if matches!(
        intent.kind,
        FileOperationKind::Rename | FileOperationKind::Move
    ) && error.kind() == std::io::ErrorKind::AlreadyExists
    {
        FsOpsErrorCode::NameConflict
    } else {
        FsOpsErrorCode::FsError
    };
    FsOpsError::new(code, format!("{} failed: {error}", intent.kind.as_str()))
}

fn persist_intents(
    database: &Mutex<Database>,
    kind: FileOperationKind,
    prepared: Vec<PreparedIntent>,
) -> FsOpsResult<Vec<JournalIntent>> {
    let guard = database.lock().expect("database mutex poisoned");
    let transaction =
        Transaction::new_unchecked(guard.connection_for_fsops(), TransactionBehavior::Immediate)?;
    let group_id = Uuid::now_v7().to_string();
    let started_at = now_millis();
    let mut intents = Vec::with_capacity(prepared.len());

    for prepared in prepared {
        let error_code = prepared
            .preflight_error
            .as_ref()
            .map(|error| error.code.as_str());
        let error_detail = prepared
            .preflight_error
            .as_ref()
            .map(|error| error.message.as_str());
        transaction.execute(
            r#"INSERT INTO file_operations(
                 operation, source_path, target_path, status, created_at,
                 operation_group_id, entity_kind, entity_id, root_id,
                 target_folder_id, attempt_count, journal_phase,
                 source_identity, error_code, error
               ) VALUES (?1, ?2, ?3, 'pending', unixepoch(), ?4, ?5, ?6, ?7,
                         ?8, 0, 'prepared', ?9, ?10, ?11)"#,
            params![
                prepared.kind.as_str(),
                prepared.source_path,
                prepared.target_path,
                group_id,
                prepared.entity_kind.map(EntityKind::as_str),
                prepared.entity_id,
                prepared.root_id,
                prepared.target_folder_id,
                prepared.source_identity,
                error_code,
                error_detail,
            ],
        )?;
        intents.push(JournalIntent {
            id: transaction.last_insert_rowid(),
            group_id: group_id.clone(),
            kind: prepared.kind,
            entity_kind: prepared.entity_kind,
            entity_id: prepared.entity_id,
            root_id: prepared.root_id,
            source_path: PathBuf::from(prepared.source_path),
            target_path: prepared.target_path.map(PathBuf::from),
            target_folder_id: prepared.target_folder_id,
            attempt_count: 0,
            phase: JournalPhase::Prepared,
            source_identity: prepared.source_identity,
            pending_error_code: error_code.map(str::to_owned),
            pending_error_detail: error_detail.map(str::to_owned),
        });
    }

    let mut operation = new_group_operation(&group_id, kind, intents.len() as i64, started_at);
    let appended = guard.publish_notification_operation_in_transaction(
        &transaction,
        &mut operation,
        PublishMode::Phase,
    )?;
    transaction.commit()?;
    if appended {
        guard.notification_outbox_wake().notify_one();
    }
    Ok(intents)
}

fn mark_attempt(database: &Database, intent: &mut JournalIntent) -> FsOpsResult<()> {
    let transaction = Transaction::new_unchecked(
        database.connection_for_fsops(),
        TransactionBehavior::Immediate,
    )?;
    let updated = transaction.execute(
        "UPDATE file_operations SET attempt_count = attempt_count + 1, journal_phase = 'executing' WHERE id = ?1 AND status = 'pending' AND journal_phase = 'prepared'",
        [intent.id],
    )?;
    if updated != 1 {
        return Err(FsOpsError::new(
            FsOpsErrorCode::FsError,
            format!("file-operation intent {} is no longer pending", intent.id),
        ));
    }
    transaction.commit()?;
    intent.attempt_count = intent.attempt_count.saturating_add(1);
    intent.phase = JournalPhase::Executing;
    Ok(())
}

fn finalize_intent(
    database: &Database,
    intent: &JournalIntent,
    error: Option<&FsOpsError>,
) -> FsOpsResult<FileOperationRecord> {
    let transaction = Transaction::new_unchecked(
        database.connection_for_fsops(),
        TransactionBehavior::Immediate,
    )?;
    if error.is_none() {
        apply_success_to_index(&transaction, intent)?;
    }
    let status = if error.is_some() {
        "failed"
    } else {
        "succeeded"
    };
    let updated = transaction.execute(
        r#"UPDATE file_operations
           SET status = ?1, finished_at = unixepoch(), error_code = ?2, error = ?3
           WHERE id = ?4 AND status = 'pending'"#,
        params![
            status,
            error.map(|error| error.code.as_str()),
            error.map(|error| error.message.as_str()),
            intent.id
        ],
    )?;
    if updated != 1 {
        return Err(FsOpsError::new(
            FsOpsErrorCode::FsError,
            format!("file-operation intent {} was already finalized", intent.id),
        ));
    }
    let appended = publish_group_state_in_transaction(database, &transaction, &intent.group_id)?;
    let record = operation_record_in_transaction(&transaction, intent.id)?;
    transaction.commit()?;
    if appended {
        database.notification_outbox_wake().notify_one();
    }
    Ok(record)
}

fn apply_success_to_index(
    transaction: &Transaction<'_>,
    intent: &JournalIntent,
) -> FsOpsResult<()> {
    let entity_id = intent.entity_id.ok_or_else(|| {
        FsOpsError::new(
            FsOpsErrorCode::FsError,
            "file-operation intent is missing its entity id",
        )
    })?;
    let entity_kind = intent.entity_kind.ok_or_else(|| {
        FsOpsError::new(
            FsOpsErrorCode::FsError,
            "file-operation intent is missing its entity kind",
        )
    })?;
    if matches!(
        intent.kind,
        FileOperationKind::Rename | FileOperationKind::Move
    ) {
        remove_target_index_conflicts(transaction, intent, entity_kind, entity_id)?;
    }
    match (intent.kind, entity_kind) {
        (FileOperationKind::Rename, EntityKind::File) => {
            let name = target_name(intent)?;
            transaction.execute(
                "UPDATE files SET name = ?1, ext = ?2, status = 'active' WHERE id = ?3",
                params![name, extension_from_name(&name), entity_id],
            )?;
            crate::db::sync_media_fts_for_file_in_transaction_pub(transaction, entity_id)?;
        }
        (FileOperationKind::Rename, EntityKind::Folder) => {
            let name = target_name(intent)?;
            transaction.execute(
                "UPDATE folders SET name = ?1, status = 'active' WHERE id = ?2",
                params![name, entity_id],
            )?;
            sync_descendant_fts(transaction, entity_id)?;
        }
        (FileOperationKind::Move, EntityKind::File) => {
            let target_folder_id = required_target_folder(intent)?;
            transaction.execute(
                "UPDATE files SET folder_id = ?1, status = 'active' WHERE id = ?2",
                params![target_folder_id, entity_id],
            )?;
            crate::db::sync_media_fts_for_file_in_transaction_pub(transaction, entity_id)?;
        }
        (FileOperationKind::Move, EntityKind::Folder) => {
            let target_folder_id = required_target_folder(intent)?;
            transaction.execute(
                "UPDATE folders SET parent_id = ?1, status = 'active' WHERE id = ?2",
                params![target_folder_id, entity_id],
            )?;
            sync_descendant_fts(transaction, entity_id)?;
        }
        (
            FileOperationKind::DeleteRecycle | FileOperationKind::DeletePermanent,
            EntityKind::File,
        ) => {
            transaction.execute(
                "UPDATE files SET status = 'deleted' WHERE id = ?1",
                [entity_id],
            )?;
            transaction.execute("DELETE FROM media_fts WHERE rowid = ?1", [entity_id])?;
            crate::db::cleanup_inactive_thumbnail_artifacts_in_transaction(transaction)?;
        }
        (
            FileOperationKind::DeleteRecycle | FileOperationKind::DeletePermanent,
            EntityKind::Folder,
        ) => {
            let descendant_files = collect_descendant_file_ids(transaction, entity_id)?;
            transaction.execute(
                r#"WITH RECURSIVE subtree(id) AS (
                     SELECT id FROM folders WHERE id = ?1
                     UNION ALL
                     SELECT folders.id FROM folders JOIN subtree ON folders.parent_id = subtree.id
                   )
                   UPDATE folders SET status = 'deleted' WHERE id IN (SELECT id FROM subtree)"#,
                [entity_id],
            )?;
            transaction.execute(
                r#"WITH RECURSIVE subtree(id) AS (
                     SELECT id FROM folders WHERE id = ?1
                     UNION ALL
                     SELECT folders.id FROM folders JOIN subtree ON folders.parent_id = subtree.id
                   )
                   UPDATE files SET status = 'deleted' WHERE folder_id IN (SELECT id FROM subtree)"#,
                [entity_id],
            )?;
            crate::db::cleanup_inactive_thumbnail_artifacts_in_transaction(transaction)?;
            for file_id in descendant_files {
                transaction.execute("DELETE FROM media_fts WHERE rowid = ?1", [file_id])?;
            }
        }
    }
    Ok(())
}

fn remove_target_index_conflicts(
    transaction: &Transaction<'_>,
    intent: &JournalIntent,
    entity_kind: EntityKind,
    entity_id: i64,
) -> FsOpsResult<()> {
    let target_name = target_name(intent)?;
    let mut removed = false;
    match entity_kind {
        EntityKind::File => {
            let target_folder_id = if intent.kind == FileOperationKind::Move {
                required_target_folder(intent)?
            } else {
                transaction.query_row(
                    "SELECT folder_id FROM files WHERE id = ?1",
                    [entity_id],
                    |row| row.get(0),
                )?
            };
            let duplicate_ids = transaction
                .prepare("SELECT id FROM files WHERE folder_id = ?1 AND name = ?2 AND id <> ?3")?
                .query_map(params![target_folder_id, target_name, entity_id], |row| {
                    row.get::<_, i64>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for duplicate_id in duplicate_ids {
                transaction.execute("DELETE FROM media_fts WHERE rowid = ?1", [duplicate_id])?;
                transaction.execute("DELETE FROM files WHERE id = ?1", [duplicate_id])?;
                removed = true;
            }
        }
        EntityKind::Folder => {
            let root_id = intent
                .root_id
                .ok_or_else(|| interrupted_error("folder operation has no root id"))?;
            let target_parent_id = if intent.kind == FileOperationKind::Move {
                Some(required_target_folder(intent)?)
            } else {
                transaction.query_row(
                    "SELECT parent_id FROM folders WHERE id = ?1",
                    [entity_id],
                    |row| row.get::<_, Option<i64>>(0),
                )?
            };
            let duplicate_ids = transaction
                .prepare(
                    r#"SELECT id FROM folders
                       WHERE root_id = ?1 AND parent_id IS ?2 AND name = ?3 AND id <> ?4"#,
                )?
                .query_map(
                    params![root_id, target_parent_id, target_name, entity_id],
                    |row| row.get::<_, i64>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for duplicate_id in duplicate_ids {
                for file_id in collect_descendant_file_ids(transaction, duplicate_id)? {
                    transaction.execute("DELETE FROM media_fts WHERE rowid = ?1", [file_id])?;
                }
                transaction.execute("DELETE FROM folders WHERE id = ?1", [duplicate_id])?;
                removed = true;
            }
        }
    }
    if removed {
        crate::db::cleanup_inactive_thumbnail_artifacts_in_transaction(transaction)?;
    }
    Ok(())
}

fn sync_descendant_fts(transaction: &Transaction<'_>, folder_id: i64) -> FsOpsResult<()> {
    for file_id in collect_descendant_file_ids(transaction, folder_id)? {
        crate::db::sync_media_fts_for_file_in_transaction_pub(transaction, file_id)?;
    }
    Ok(())
}

fn target_name(intent: &JournalIntent) -> FsOpsResult<String> {
    intent
        .target_path
        .as_deref()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| {
            FsOpsError::new(
                FsOpsErrorCode::FsError,
                "file-operation target path has no name",
            )
        })
}

fn required_target_folder(intent: &JournalIntent) -> FsOpsResult<i64> {
    intent.target_folder_id.ok_or_else(|| {
        FsOpsError::new(
            FsOpsErrorCode::FsError,
            "move intent is missing its target folder id",
        )
    })
}

fn publish_group_state_in_transaction(
    database: &Database,
    transaction: &Transaction<'_>,
    group_id: &str,
) -> FsOpsResult<bool> {
    let (total, current, failed, max_attempt): (i64, i64, i64, i64) = transaction.query_row(
        r#"SELECT COUNT(*),
                  COALESCE(SUM(CASE WHEN status <> 'pending' THEN 1 ELSE 0 END), 0),
                  COALESCE(SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END), 0),
                  COALESCE(MAX(attempt_count), 0)
           FROM file_operations WHERE operation_group_id = ?1"#,
        [group_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let payload: String = transaction.query_row(
        "SELECT payload_json FROM notification_operation_heads WHERE operation_id = ?1",
        [group_id],
        |row| row.get(0),
    )?;
    let mut operation: NotificationOperation = serde_json::from_str(&payload).map_err(|error| {
        FsOpsError::new(
            FsOpsErrorCode::FsError,
            format!("invalid notification head for file-operation group {group_id}: {error}"),
        )
    })?;
    operation.current_value = current;
    operation.total_value = Some(total);
    operation.attempt = max_attempt.max(1);
    operation.updated_at = now_millis();
    operation.error_code = None;
    operation.error_detail = None;
    operation.completed_at = None;
    let terminal = current == total;
    let mode = if terminal {
        operation.completed_at = Some(operation.updated_at);
        if failed > 0 {
            operation.status = "failed".to_owned();
            operation.phase = Some("failed".to_owned());
            let first_error: Option<(Option<String>, Option<String>)> = transaction
                .query_row(
                    "SELECT error_code, error FROM file_operations WHERE operation_group_id = ?1 AND status = 'failed' ORDER BY id LIMIT 1",
                    [group_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((code, detail)) = first_error {
                operation.error_code = code;
                operation.error_detail = detail;
            }
        } else {
            operation.status = "succeeded".to_owned();
            operation.phase = Some("completed".to_owned());
        }
        PublishMode::Terminal
    } else {
        operation.status = "running".to_owned();
        operation.phase = Some("working".to_owned());
        PublishMode::Progress
    };
    Ok(
        database.publish_notification_operation_in_transaction(
            transaction,
            &mut operation,
            mode,
        )?,
    )
}

fn operation_record_in_transaction(
    transaction: &Transaction<'_>,
    operation_id: i64,
) -> FsOpsResult<FileOperationRecord> {
    Ok(transaction.query_row(
        r#"SELECT id, operation, source_path, target_path, status,
                  created_at, finished_at, error, operation_group_id
           FROM file_operations WHERE id = ?1"#,
        [operation_id],
        record_from_row,
    )?)
}

fn load_pending_intents(database: &Database) -> FsOpsResult<Vec<JournalIntent>> {
    let mut statement = database.connection_for_fsops().prepare(
        r#"SELECT id, operation_group_id, operation, entity_kind, entity_id,
                  root_id, source_path, target_path, target_folder_id,
                  attempt_count, journal_phase, source_identity, error_code, error
           FROM file_operations
           WHERE status = 'pending' AND operation_group_id IS NOT NULL
           ORDER BY id"#,
    )?;
    let rows = statement
        .query_map([], |row| {
            let operation: String = row.get(2)?;
            let entity_kind: Option<String> = row.get(3)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                operation,
                entity_kind,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    rows.into_iter()
        .map(
            |(
                id,
                group_id,
                operation,
                entity_kind,
                entity_id,
                root_id,
                source_path,
                target_path,
                target_folder_id,
                attempt_count,
                journal_phase,
                source_identity,
                error_code,
                error_detail,
            )| {
                Ok(JournalIntent {
                    id,
                    group_id: group_id.ok_or_else(|| {
                        FsOpsError::new(
                            FsOpsErrorCode::FsError,
                            format!("pending file-operation intent {id} has no group id"),
                        )
                    })?,
                    kind: parse_operation_kind(&operation)?,
                    entity_kind: entity_kind.as_deref().map(EntityKind::parse).transpose()?,
                    entity_id,
                    root_id,
                    source_path: PathBuf::from(source_path),
                    target_path: target_path.map(PathBuf::from),
                    target_folder_id,
                    attempt_count,
                    phase: JournalPhase::parse(&journal_phase)?,
                    source_identity,
                    pending_error_code: error_code,
                    pending_error_detail: error_detail,
                })
            },
        )
        .collect()
}

fn parse_operation_kind(value: &str) -> FsOpsResult<FileOperationKind> {
    match value {
        "rename" => Ok(FileOperationKind::Rename),
        "move" => Ok(FileOperationKind::Move),
        "delete_recycle" => Ok(FileOperationKind::DeleteRecycle),
        "delete_permanent" => Ok(FileOperationKind::DeletePermanent),
        _ => Err(FsOpsError::new(
            FsOpsErrorCode::FsError,
            format!("unsupported pending file operation: {value}"),
        )),
    }
}

fn persisted_error(code: &str, detail: Option<&str>) -> FsOpsError {
    let code = match code {
        "invalid_name" => FsOpsErrorCode::InvalidName,
        "invalid_request" => FsOpsErrorCode::InvalidRequest,
        "not_found" => FsOpsErrorCode::NotFound,
        "name_conflict" => FsOpsErrorCode::NameConflict,
        "cross_root" => FsOpsErrorCode::CrossRoot,
        "outside_root" => FsOpsErrorCode::OutsideRoot,
        "symlink_refused" => FsOpsErrorCode::SymlinkRefused,
        "interrupted" => FsOpsErrorCode::Interrupted,
        _ => FsOpsErrorCode::FsError,
    };
    FsOpsError::new(
        code,
        detail.unwrap_or("file operation failed during preflight"),
    )
}

fn new_group_operation(
    group_id: &str,
    kind: FileOperationKind,
    total: i64,
    started_at: i64,
) -> NotificationOperation {
    NotificationOperation {
        operation_id: group_id.to_owned(),
        revision: 0,
        kind: kind.as_str().to_owned(),
        category: "file_operation".to_owned(),
        origin: "user".to_owned(),
        presentation: "important".to_owned(),
        status: "pending".to_owned(),
        phase: Some("queued".to_owned()),
        title: match kind {
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
        current_value: 0,
        total_value: Some(total),
        unit: Some("items".to_owned()),
        error_code: None,
        error_detail: None,
        cancellable: false,
        retryable: false,
        attempt: 1,
        occurrence_count: 1,
        started_at,
        updated_at: started_at,
        completed_at: None,
    }
}

fn prepare_rename(
    database: &Mutex<Database>,
    request: RenameRequest,
) -> FsOpsResult<PreparedIntent> {
    let validation_error = validate_name(request.new_name.trim()).err();
    let new_name = request.new_name.trim().to_owned();
    let mut prepared = (|| -> FsOpsResult<PreparedIntent> {
        let guard = database.lock().expect("database mutex poisoned");
        let conn = guard.connection_for_fsops();
        match (request.file_id, request.folder_id) {
            (Some(file_id), None) => match load_file_info(conn, file_id)? {
                Some(info) => {
                    let target = info.parent_dir.join(&new_name);
                    let mut error = validation_error;
                    if error.is_none()
                        && info.name != new_name
                        && file_exists_in_folder_conn(conn, info.folder_id, &new_name)?
                    {
                        error = Some(FsOpsError::new(
                            FsOpsErrorCode::NameConflict,
                            format!("a file named '{new_name}' already exists in this folder"),
                        ));
                    }
                    Ok(PreparedIntent {
                        kind: FileOperationKind::Rename,
                        entity_kind: Some(EntityKind::File),
                        entity_id: Some(file_id),
                        root_id: Some(info.root_id),
                        source_path: info.path.to_string_lossy().into_owned(),
                        target_path: Some(target.to_string_lossy().into_owned()),
                        target_folder_id: None,
                        delete_root_path: None,
                        source_identity: None,
                        preflight_error: error,
                    })
                }
                None => Ok(missing_intent(
                    FileOperationKind::Rename,
                    EntityKind::File,
                    file_id,
                    &new_name,
                    validation_error.unwrap_or_else(|| {
                        FsOpsError::new(
                            FsOpsErrorCode::NotFound,
                            format!("file not found: {file_id}"),
                        )
                    }),
                )),
            },
            (None, Some(folder_id)) => match load_folder_info(conn, folder_id)? {
                Some(info) => {
                    let target = info.path.parent().unwrap_or(Path::new("")).join(&new_name);
                    let mut error = validation_error;
                    if error.is_none() && info.parent_id.is_none() {
                        error = Some(FsOpsError::new(
                            FsOpsErrorCode::InvalidRequest,
                            "cannot rename a root folder",
                        ));
                    }
                    if error.is_none()
                        && info.name != new_name
                        && (folder_name_exists_in_parent_conn(
                            conn,
                            info.parent_id,
                            info.root_id,
                            &new_name,
                        )?)
                    {
                        error = Some(FsOpsError::new(
                            FsOpsErrorCode::NameConflict,
                            format!("a folder named '{new_name}' already exists in this parent"),
                        ));
                    }
                    Ok(PreparedIntent {
                        kind: FileOperationKind::Rename,
                        entity_kind: Some(EntityKind::Folder),
                        entity_id: Some(folder_id),
                        root_id: Some(info.root_id),
                        source_path: info.path.to_string_lossy().into_owned(),
                        target_path: Some(target.to_string_lossy().into_owned()),
                        target_folder_id: None,
                        delete_root_path: None,
                        source_identity: None,
                        preflight_error: error,
                    })
                }
                None => Ok(missing_intent(
                    FileOperationKind::Rename,
                    EntityKind::Folder,
                    folder_id,
                    &new_name,
                    validation_error.unwrap_or_else(|| {
                        FsOpsError::new(
                            FsOpsErrorCode::NotFound,
                            format!("folder not found: {folder_id}"),
                        )
                    }),
                )),
            },
            (Some(_), Some(_)) => Ok(request_error_intent(
                FileOperationKind::Rename,
                &new_name,
                validation_error.unwrap_or_else(|| {
                    FsOpsError::new(
                        FsOpsErrorCode::InvalidRequest,
                        "exactly one of fileId or folderId must be provided",
                    )
                }),
            )),
            (None, None) => Ok(request_error_intent(
                FileOperationKind::Rename,
                &new_name,
                validation_error.unwrap_or_else(|| {
                    FsOpsError::new(
                        FsOpsErrorCode::InvalidRequest,
                        "either fileId or folderId must be provided",
                    )
                }),
            )),
        }
    })()?;
    apply_source_identity_preflight(&mut prepared);
    apply_target_path_preflight(&mut prepared, "rename target already exists");
    Ok(prepared)
}

fn prepare_move(
    database: &Mutex<Database>,
    request: &MoveRequest,
) -> FsOpsResult<Vec<PreparedIntent>> {
    let mut prepared = {
        let guard = database.lock().expect("database mutex poisoned");
        let conn = guard.connection_for_fsops();
        let target = load_folder_path(conn, request.target_folder_id)?;
        let target_error = target.as_ref().is_none().then(|| {
            FsOpsError::new(
                FsOpsErrorCode::NotFound,
                format!("target folder not found: {}", request.target_folder_id),
            )
        });
        let mut prepared = Vec::with_capacity(request.file_ids.len() + request.folder_ids.len());
        for file_id in &request.file_ids {
            prepared.push(prepare_move_file(
                conn,
                *file_id,
                target.as_ref(),
                target_error.clone(),
            )?);
        }
        for folder_id in &request.folder_ids {
            prepared.push(prepare_move_folder(
                conn,
                *folder_id,
                target.as_ref(),
                request.target_folder_id,
                target_error.clone(),
            )?);
        }
        prepared
    };
    for intent in &mut prepared {
        apply_source_identity_preflight(intent);
        apply_target_path_preflight(intent, "move target already exists");
    }
    Ok(prepared)
}

fn prepare_move_file(
    conn: &rusqlite::Connection,
    file_id: i64,
    target: Option<&FolderTarget>,
    target_error: Option<FsOpsError>,
) -> FsOpsResult<PreparedIntent> {
    let Some(info) = load_file_info(conn, file_id)? else {
        return Ok(missing_intent(
            FileOperationKind::Move,
            EntityKind::File,
            file_id,
            "move-target",
            target_error.unwrap_or_else(|| {
                FsOpsError::new(
                    FsOpsErrorCode::NotFound,
                    format!("file not found: {file_id}"),
                )
            }),
        ));
    };
    let mut error = target_error;
    let target_path = if let Some(target) = target {
        if error.is_none() && info.root_id != target.root_id {
            error = Some(FsOpsError::new(
                FsOpsErrorCode::CrossRoot,
                "cross_root: source and target are in different roots",
            ));
        }
        Some(if info.folder_id == target.folder_id {
            info.path.clone()
        } else {
            let path = target.path.join(&info.name);
            if error.is_none() && file_exists_in_folder_conn(conn, target.folder_id, &info.name)? {
                error = Some(FsOpsError::new(
                    FsOpsErrorCode::NameConflict,
                    format!(
                        "a file named '{}' already exists in target folder",
                        info.name
                    ),
                ));
            }
            path
        })
    } else {
        None
    };
    Ok(PreparedIntent {
        kind: FileOperationKind::Move,
        entity_kind: Some(EntityKind::File),
        entity_id: Some(file_id),
        root_id: Some(info.root_id),
        source_path: info.path.to_string_lossy().into_owned(),
        target_path: target_path.map(|path| path.to_string_lossy().into_owned()),
        target_folder_id: target.map(|target| target.folder_id),
        delete_root_path: None,
        source_identity: None,
        preflight_error: error,
    })
}

fn prepare_move_folder(
    conn: &rusqlite::Connection,
    folder_id: i64,
    target: Option<&FolderTarget>,
    requested_target_folder_id: i64,
    target_error: Option<FsOpsError>,
) -> FsOpsResult<PreparedIntent> {
    let Some(info) = load_folder_info(conn, folder_id)? else {
        return Ok(missing_intent(
            FileOperationKind::Move,
            EntityKind::Folder,
            folder_id,
            "move-target",
            target_error.unwrap_or_else(|| {
                FsOpsError::new(
                    FsOpsErrorCode::NotFound,
                    format!("folder not found: {folder_id}"),
                )
            }),
        ));
    };
    let mut error = target_error;
    if error.is_none() && folder_id == requested_target_folder_id {
        error = Some(FsOpsError::new(
            FsOpsErrorCode::InvalidRequest,
            format!("cannot move folder {folder_id} into itself"),
        ));
    }
    if error.is_none() && info.parent_id.is_none() {
        error = Some(FsOpsError::new(
            FsOpsErrorCode::InvalidRequest,
            "cannot move a root folder",
        ));
    }
    let target_path = if let Some(target) = target {
        if error.is_none() && info.root_id != target.root_id {
            error = Some(FsOpsError::new(
                FsOpsErrorCode::CrossRoot,
                "cross_root: source and target are in different roots",
            ));
        }
        if error.is_none() && folder_is_ancestor_of(conn, folder_id, target.folder_id)? {
            error = Some(FsOpsError::new(
                FsOpsErrorCode::InvalidRequest,
                "cannot move a folder into its own descendant",
            ));
        }
        let path = target.path.join(&info.name);
        if error.is_none()
            && folder_name_exists_in_parent_conn(
                conn,
                Some(target.folder_id),
                info.root_id,
                &info.name,
            )?
        {
            error = Some(FsOpsError::new(
                FsOpsErrorCode::NameConflict,
                format!(
                    "a folder named '{}' already exists in target folder",
                    info.name
                ),
            ));
        }
        Some(path)
    } else {
        None
    };
    Ok(PreparedIntent {
        kind: FileOperationKind::Move,
        entity_kind: Some(EntityKind::Folder),
        entity_id: Some(folder_id),
        root_id: Some(info.root_id),
        source_path: info.path.to_string_lossy().into_owned(),
        target_path: target_path.map(|path| path.to_string_lossy().into_owned()),
        target_folder_id: target.map(|target| target.folder_id),
        delete_root_path: None,
        source_identity: None,
        preflight_error: error,
    })
}

fn prepare_delete(
    database: &Mutex<Database>,
    request: &DeleteRequest,
    kind: FileOperationKind,
) -> FsOpsResult<Vec<PreparedIntent>> {
    let mut prepared = {
        let guard = database.lock().expect("database mutex poisoned");
        let conn = guard.connection_for_fsops();
        let mut prepared = Vec::with_capacity(request.file_ids.len() + request.folder_ids.len());
        for file_id in &request.file_ids {
            prepared.push(match load_file_info(conn, *file_id)? {
                Some(info) => {
                    let root_path = request
                        .permanent
                        .then(|| enabled_root_path(conn, info.root_id));
                    let (delete_root_path, preflight_error) = split_root_result(root_path);
                    PreparedIntent {
                        kind,
                        entity_kind: Some(EntityKind::File),
                        entity_id: Some(*file_id),
                        root_id: Some(info.root_id),
                        source_path: info.path.to_string_lossy().into_owned(),
                        target_path: None,
                        target_folder_id: None,
                        delete_root_path,
                        source_identity: None,
                        preflight_error,
                    }
                }
                None => missing_intent(
                    kind,
                    EntityKind::File,
                    *file_id,
                    "delete",
                    FsOpsError::new(
                        FsOpsErrorCode::NotFound,
                        format!("file not found: {file_id}"),
                    ),
                ),
            });
        }
        for folder_id in &request.folder_ids {
            prepared.push(match load_folder_info(conn, *folder_id)? {
                Some(info) => {
                    let mut error = info.parent_id.is_none().then(|| {
                        FsOpsError::new(
                            FsOpsErrorCode::InvalidRequest,
                            "cannot delete a root folder",
                        )
                    });
                    let root_path = if request.permanent && error.is_none() {
                        match enabled_root_path(conn, info.root_id) {
                            Ok(path) => Some(path),
                            Err(root_error) => {
                                error = Some(root_error);
                                None
                            }
                        }
                    } else {
                        None
                    };
                    PreparedIntent {
                        kind,
                        entity_kind: Some(EntityKind::Folder),
                        entity_id: Some(*folder_id),
                        root_id: Some(info.root_id),
                        source_path: info.path.to_string_lossy().into_owned(),
                        target_path: None,
                        target_folder_id: None,
                        delete_root_path: root_path,
                        source_identity: None,
                        preflight_error: error,
                    }
                }
                None => missing_intent(
                    kind,
                    EntityKind::Folder,
                    *folder_id,
                    "delete",
                    FsOpsError::new(
                        FsOpsErrorCode::NotFound,
                        format!("folder not found: {folder_id}"),
                    ),
                ),
            });
        }
        prepared
    };
    for intent in &mut prepared {
        if request.permanent {
            apply_delete_filesystem_preflight(intent);
        } else {
            apply_source_identity_preflight(intent);
        }
    }
    Ok(prepared)
}

fn enabled_root_path(conn: &rusqlite::Connection, root_id: i64) -> FsOpsResult<PathBuf> {
    let root: Option<(String, i64, Option<i64>)> = conn
        .query_row(
            "SELECT path, enabled, pending_delete_at FROM roots WHERE id = ?1",
            [root_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((path, enabled, pending_delete_at)) = root else {
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
    if pending_delete_at.is_some() {
        return Err(FsOpsError::new(
            FsOpsErrorCode::OutsideRoot,
            "permanent delete is not allowed while the root is pending deletion",
        ));
    }
    Ok(PathBuf::from(path))
}

fn split_root_result(
    result: Option<FsOpsResult<PathBuf>>,
) -> (Option<PathBuf>, Option<FsOpsError>) {
    match result {
        Some(Ok(path)) => (Some(path), None),
        Some(Err(error)) => (None, Some(error)),
        None => (None, None),
    }
}

fn apply_delete_filesystem_preflight(intent: &mut PreparedIntent) {
    if intent.preflight_error.is_some() {
        return;
    }
    let Some(root_path) = intent.delete_root_path.as_deref() else {
        return;
    };
    let source_path = Path::new(&intent.source_path);
    match probe_path(source_path) {
        PathProbe::Exists(metadata) => {
            match validate_permanent_delete_path(source_path, root_path, &metadata, None) {
                Ok(identity) => intent.source_identity = identity,
                Err(error) => intent.preflight_error = Some(error),
            }
        }
        PathProbe::NotFound => {
            intent.preflight_error = Some(FsOpsError::new(
                FsOpsErrorCode::FsError,
                format!("delete source '{}' does not exist", source_path.display()),
            ));
        }
        PathProbe::Error(error) => {
            intent.preflight_error = Some(FsOpsError::new(
                FsOpsErrorCode::FsError,
                format!(
                    "cannot inspect delete source '{}': {error}",
                    source_path.display()
                ),
            ));
        }
    }
}

fn apply_source_identity_preflight(intent: &mut PreparedIntent) {
    if intent.preflight_error.is_some() {
        return;
    }
    let source_path = Path::new(&intent.source_path);
    match probe_path(source_path) {
        PathProbe::Exists(metadata) => {
            intent.source_identity = filesystem_identity(source_path, &metadata)
        }
        PathProbe::NotFound => {
            intent.preflight_error = Some(FsOpsError::new(
                FsOpsErrorCode::NotFound,
                format!(
                    "file-operation source '{}' does not exist",
                    source_path.display()
                ),
            ));
        }
        PathProbe::Error(error) => {
            intent.preflight_error = Some(FsOpsError::new(
                FsOpsErrorCode::FsError,
                format!(
                    "cannot inspect file-operation source '{}': {error}",
                    source_path.display()
                ),
            ));
        }
    }
}

fn missing_intent(
    kind: FileOperationKind,
    entity_kind: EntityKind,
    entity_id: i64,
    target: &str,
    error: FsOpsError,
) -> PreparedIntent {
    PreparedIntent {
        kind,
        entity_kind: Some(entity_kind),
        entity_id: Some(entity_id),
        root_id: None,
        source_path: format!("{}-id:{entity_id}", entity_kind.as_str()),
        target_path: Some(target.to_owned()),
        target_folder_id: None,
        delete_root_path: None,
        source_identity: None,
        preflight_error: Some(error),
    }
}

fn request_error_intent(
    kind: FileOperationKind,
    target: &str,
    error: FsOpsError,
) -> PreparedIntent {
    PreparedIntent {
        kind,
        entity_kind: None,
        entity_id: None,
        root_id: None,
        source_path: "invalid-request".to_owned(),
        target_path: Some(target.to_owned()),
        target_folder_id: None,
        delete_root_path: None,
        source_identity: None,
        preflight_error: Some(error),
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
