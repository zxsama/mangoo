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
    source_path: String,
    target_path: Option<String>,
    target_folder_id: Option<i64>,
    delete_root_path: Option<PathBuf>,
    preflight_error: Option<FsOpsError>,
}

#[derive(Debug, Clone)]
struct JournalIntent {
    id: i64,
    group_id: String,
    kind: FileOperationKind,
    entity_kind: Option<EntityKind>,
    entity_id: Option<i64>,
    source_path: PathBuf,
    target_path: Option<PathBuf>,
    target_folder_id: Option<i64>,
    attempt_count: i64,
    pending_error_code: Option<String>,
    pending_error_detail: Option<String>,
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

    match reconcile_filesystem_state(intent) {
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

fn reconcile_filesystem_state(intent: &JournalIntent) -> ReconcileAction {
    let source_exists = path_present(&intent.source_path);
    match intent.kind {
        FileOperationKind::DeleteRecycle | FileOperationKind::DeletePermanent => {
            if source_exists {
                ReconcileAction::Retry
            } else {
                ReconcileAction::FinalizeSuccess
            }
        }
        FileOperationKind::Rename | FileOperationKind::Move => {
            let Some(target_path) = intent.target_path.as_deref() else {
                return ReconcileAction::Fail(FsOpsError::new(
                    FsOpsErrorCode::FsError,
                    "pending rename/move operation is missing its target path",
                ));
            };
            if intent.source_path == target_path {
                return ReconcileAction::FinalizeSuccess;
            }
            match (source_exists, path_present(target_path)) {
                (false, true) => ReconcileAction::FinalizeSuccess,
                (true, false) => ReconcileAction::Retry,
                (true, true) => ReconcileAction::Fail(FsOpsError::new(
                    FsOpsErrorCode::NameConflict,
                    "source and target both exist during file-operation recovery",
                )),
                (false, false) => ReconcileAction::Fail(FsOpsError::new(
                    FsOpsErrorCode::NotFound,
                    "source and target are both missing during file-operation recovery",
                )),
            }
        }
    }
}

fn path_present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn apply_target_path_preflight(intent: &mut PreparedIntent, message: &str) {
    if intent.preflight_error.is_some() {
        return;
    }
    let Some(target) = intent.target_path.as_deref().map(Path::new) else {
        return;
    };
    if Path::new(&intent.source_path) != target && path_present(target) {
        intent.preflight_error = Some(FsOpsError::new(FsOpsErrorCode::NameConflict, message));
    }
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
                 operation_group_id, entity_kind, entity_id, target_folder_id,
                 attempt_count, error_code, error
               ) VALUES (?1, ?2, ?3, 'pending', unixepoch(), ?4, ?5, ?6, ?7, 0, ?8, ?9)"#,
            params![
                prepared.kind.as_str(),
                prepared.source_path,
                prepared.target_path,
                group_id,
                prepared.entity_kind.map(EntityKind::as_str),
                prepared.entity_id,
                prepared.target_folder_id,
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
            source_path: PathBuf::from(prepared.source_path),
            target_path: prepared.target_path.map(PathBuf::from),
            target_folder_id: prepared.target_folder_id,
            attempt_count: 0,
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
        "UPDATE file_operations SET attempt_count = attempt_count + 1 WHERE id = ?1 AND status = 'pending'",
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
    match (intent.kind, entity_kind) {
        (FileOperationKind::Rename, EntityKind::File) => {
            let name = target_name(intent)?;
            transaction.execute(
                "UPDATE files SET name = ?1, ext = ?2 WHERE id = ?3",
                params![name, extension_from_name(&name), entity_id],
            )?;
            crate::db::sync_media_fts_for_file_in_transaction_pub(transaction, entity_id)?;
        }
        (FileOperationKind::Rename, EntityKind::Folder) => {
            let name = target_name(intent)?;
            transaction.execute(
                "UPDATE folders SET name = ?1 WHERE id = ?2",
                params![name, entity_id],
            )?;
            sync_descendant_fts(transaction, entity_id)?;
        }
        (FileOperationKind::Move, EntityKind::File) => {
            let target_folder_id = required_target_folder(intent)?;
            transaction.execute(
                "UPDATE files SET folder_id = ?1 WHERE id = ?2",
                params![target_folder_id, entity_id],
            )?;
            crate::db::sync_media_fts_for_file_in_transaction_pub(transaction, entity_id)?;
        }
        (FileOperationKind::Move, EntityKind::Folder) => {
            let target_folder_id = required_target_folder(intent)?;
            transaction.execute(
                "UPDATE folders SET parent_id = ?1 WHERE id = ?2",
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
                  source_path, target_path, target_folder_id, attempt_count,
                  error_code, error
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
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
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
                source_path,
                target_path,
                target_folder_id,
                attempt_count,
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
                    source_path: PathBuf::from(source_path),
                    target_path: target_path.map(PathBuf::from),
                    target_folder_id,
                    attempt_count,
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
                        source_path: info.path.to_string_lossy().into_owned(),
                        target_path: Some(target.to_string_lossy().into_owned()),
                        target_folder_id: None,
                        delete_root_path: None,
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
                        source_path: info.path.to_string_lossy().into_owned(),
                        target_path: Some(target.to_string_lossy().into_owned()),
                        target_folder_id: None,
                        delete_root_path: None,
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
        source_path: info.path.to_string_lossy().into_owned(),
        target_path: target_path.map(|path| path.to_string_lossy().into_owned()),
        target_folder_id: target.map(|target| target.folder_id),
        delete_root_path: None,
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
        source_path: info.path.to_string_lossy().into_owned(),
        target_path: target_path.map(|path| path.to_string_lossy().into_owned()),
        target_folder_id: target.map(|target| target.folder_id),
        delete_root_path: None,
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
                        source_path: info.path.to_string_lossy().into_owned(),
                        target_path: None,
                        target_folder_id: None,
                        delete_root_path,
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
                        source_path: info.path.to_string_lossy().into_owned(),
                        target_path: None,
                        target_folder_id: None,
                        delete_root_path: root_path,
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
    if request.permanent {
        for intent in &mut prepared {
            apply_delete_filesystem_preflight(intent);
        }
    }
    Ok(prepared)
}

fn enabled_root_path(conn: &rusqlite::Connection, root_id: i64) -> FsOpsResult<PathBuf> {
    let root: Option<(String, i64)> = conn
        .query_row(
            "SELECT path, enabled FROM roots WHERE id = ?1",
            [root_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((path, enabled)) = root else {
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
    let inside_root = match (fs::canonicalize(source_path), fs::canonicalize(root_path)) {
        (Ok(source), Ok(root)) => source.starts_with(root),
        _ => source_path.starts_with(root_path),
    };
    if !inside_root {
        intent.preflight_error = Some(FsOpsError::new(
            FsOpsErrorCode::OutsideRoot,
            format!(
                "permanent delete refused: path is outside root {}",
                root_path.display()
            ),
        ));
        return;
    }
    intent.preflight_error = refuse_if_symlink(source_path).err();
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
        source_path: format!("{}-id:{entity_id}", entity_kind.as_str()),
        target_path: Some(target.to_owned()),
        target_folder_id: None,
        delete_root_path: None,
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
        source_path: "invalid-request".to_owned(),
        target_path: Some(target.to_owned()),
        target_folder_id: None,
        delete_root_path: None,
        preflight_error: Some(error),
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
