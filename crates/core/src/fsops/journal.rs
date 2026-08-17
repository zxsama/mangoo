use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::{collections::BTreeSet, collections::HashSet, sync::OnceLock, thread, time::Duration};

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
    target_parent_identity: Option<String>,
    staging_identity: Option<String>,
    preflight_error: Option<FsOpsError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalPhase {
    Legacy,
    Prepared,
    Executing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecyclePhase {
    None,
    Staged,
    ShellCompleted,
}

impl RecyclePhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Staged => "staged",
            Self::ShellCompleted => "shell_completed",
        }
    }

    fn parse(value: &str) -> FsOpsResult<Self> {
        match value {
            "none" => Ok(Self::None),
            "staged" => Ok(Self::Staged),
            "shell_completed" => Ok(Self::ShellCompleted),
            _ => Err(FsOpsError::new(
                FsOpsErrorCode::FsError,
                format!("unsupported recycle handoff phase: {value}"),
            )),
        }
    }
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
    target_parent_identity: Option<String>,
    staging_identity: Option<String>,
    recycle_phase: RecyclePhase,
    pending_error_code: Option<String>,
    pending_error_detail: Option<String>,
}

struct PersistedBatch {
    intents: Vec<JournalIntent>,
    first_error: Option<FsOpsError>,
}

#[derive(Debug)]
enum PathProbe {
    Exists(fs::Metadata),
    NotFound,
    Error(std::io::Error),
}

#[cfg(test)]
pub(super) fn rename(
    database: &Mutex<Database>,
    request: RenameRequest,
) -> FsOpsResult<FileOperationRecord> {
    let prepared = prepare_rename(database, request)?;
    let store = DirectJournalStore { database };
    let mut records = execute_prepared(&store, FileOperationKind::Rename, vec![prepared])?;
    records.pop().ok_or_else(|| {
        FsOpsError::new(
            FsOpsErrorCode::FsError,
            "rename completed without an audit row",
        )
    })
}

pub(super) fn rename_with_writer(
    database: &Arc<Mutex<Database>>,
    writer: &crate::db::DbWriter,
    request: RenameRequest,
) -> FsOpsResult<FileOperationRecord> {
    let prepared = prepare_rename(database, request)?;
    let store = WriterJournalStore::new(Arc::clone(database), writer.clone());
    let mut records = execute_prepared(&store, FileOperationKind::Rename, vec![prepared])?;
    records.pop().ok_or_else(|| {
        FsOpsError::new(
            FsOpsErrorCode::FsError,
            "rename completed without an audit row",
        )
    })
}

#[cfg(test)]
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
    let store = DirectJournalStore { database };
    execute_prepared(&store, FileOperationKind::Move, prepared)
}

pub(super) fn move_items_with_writer(
    database: &Arc<Mutex<Database>>,
    writer: &crate::db::DbWriter,
    request: MoveRequest,
) -> FsOpsResult<Vec<FileOperationRecord>> {
    if request.file_ids.is_empty() && request.folder_ids.is_empty() {
        return Err(FsOpsError::new(
            FsOpsErrorCode::InvalidRequest,
            "at least one fileId or folderId must be provided",
        ));
    }
    let prepared = prepare_move(database, &request)?;
    let store = WriterJournalStore::new(Arc::clone(database), writer.clone());
    execute_prepared(&store, FileOperationKind::Move, prepared)
}

#[cfg(test)]
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
    let store = DirectJournalStore { database };
    execute_prepared(&store, kind, prepared)
}

pub(super) fn delete_with_writer(
    database: &Arc<Mutex<Database>>,
    writer: &crate::db::DbWriter,
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
    let store = WriterJournalStore::new(Arc::clone(database), writer.clone());
    execute_prepared(&store, kind, prepared)
}

pub(crate) fn recover_pending_file_operations(database: &Database) -> FsOpsResult<usize> {
    let intents = load_pending_intents(database)?;
    let mut recovered = 0usize;
    let mut first_error = None;
    for mut intent in intents {
        match reconcile_intent(database, &mut intent) {
            Ok(Ok(_)) | Ok(Err(_)) => recovered += 1,
            Err(error) => {
                tracing::error!(
                    intent_id = intent.id,
                    operation_group_id = %intent.group_id,
                    %error,
                    "file-operation journal reconcile failed"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(recovered),
    }
}

fn recover_pending_file_operations_with_writer(
    database: Arc<Mutex<Database>>,
    writer: crate::db::DbWriter,
) -> FsOpsResult<usize> {
    let intents = {
        let guard = database
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        load_pending_intents(&guard)?
    };
    let store = WriterJournalStore::new(database, writer);
    let mut recovered = 0usize;
    let mut first_error = None;
    for mut intent in intents {
        match reconcile_intent_with_store(&store, &mut intent) {
            Ok(Ok(_)) | Ok(Err(_)) => recovered += 1,
            Err(error) => {
                tracing::error!(
                    intent_id = intent.id,
                    operation_group_id = %intent.group_id,
                    %error,
                    "single-writer file-operation repair failed"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(recovered),
    }
}

fn reconcile_intent_with_store(
    store: &WriterJournalStore,
    intent: &mut JournalIntent,
) -> FsOpsResult<FsOpsResult<FileOperationRecord>> {
    if let Some(code) = intent.pending_error_code.as_deref() {
        let error = persisted_error(code, intent.pending_error_detail.as_deref());
        store.finalize(intent, Some(&error))?;
        return Ok(Err(error));
    }
    if intent.entity_kind.is_none() || intent.entity_id.is_none() {
        let error = FsOpsError::new(
            FsOpsErrorCode::FsError,
            "pending file operation is missing replay metadata",
        );
        store.finalize(intent, Some(&error))?;
        return Ok(Err(error));
    }

    let action = {
        let guard = store
            .database
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        reconcile_filesystem_state(&guard, intent)
    };
    match action {
        ReconcileAction::FinalizeSuccess => store.finalize(intent, None).map(Ok),
        ReconcileAction::Retry => execute_intent_with_store(store, intent),
        ReconcileAction::RestoreRecycle => {
            restore_recycle_staging(intent)?;
            let journal_intent = intent.clone();
            let (phase, recycle_phase) = store
                .writer
                .write_blocking(move |database| {
                    let mut journal_intent = journal_intent;
                    reset_recycle_for_retry(database, &mut journal_intent)
                        .map_err(anyhow::Error::new)?;
                    Ok((journal_intent.phase, journal_intent.recycle_phase))
                })
                .map_err(FsOpsError::from)?;
            intent.phase = phase;
            intent.recycle_phase = recycle_phase;
            execute_intent_with_store(store, intent)
        }
        ReconcileAction::Fail(error) => {
            store.finalize(intent, Some(&error))?;
            Ok(Err(error))
        }
    }
}

pub(crate) fn schedule_pending_repair(database: Arc<Mutex<Database>>, writer: crate::db::DbWriter) {
    static ACTIVE_REPAIRS: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    let active = ACTIVE_REPAIRS.get_or_init(|| Mutex::new(HashSet::new()));
    let writer_id = writer.instance_id();
    if !active
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(writer_id)
    {
        return;
    }

    let spawn_result = thread::Builder::new()
        .name("megle-fsops-repair".to_owned())
        .spawn(move || {
            const RETRY_DELAYS: [Duration; 5] = [
                Duration::from_millis(50),
                Duration::from_millis(150),
                Duration::from_millis(400),
                Duration::from_millis(800),
                Duration::from_millis(1_600),
            ];
            for (attempt, delay) in RETRY_DELAYS.into_iter().enumerate() {
                thread::sleep(delay);
                match recover_pending_file_operations_with_writer(
                    Arc::clone(&database),
                    writer.clone(),
                ) {
                    Ok(recovered) => {
                        let pending = database
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .connection_for_fsops()
                            .query_row(
                                "SELECT COUNT(*) FROM file_operations WHERE status = 'pending' AND operation_group_id IS NOT NULL",
                                [],
                                |row| row.get::<_, i64>(0),
                            )
                            .unwrap_or(i64::MAX);
                        tracing::info!(
                            writer_id,
                            attempt = attempt + 1,
                            recovered,
                            pending,
                            "file-operation repair pass completed"
                        );
                        if pending == 0 {
                            break;
                        }
                    }
                    Err(error) => tracing::warn!(
                        writer_id,
                        attempt = attempt + 1,
                        %error,
                        "file-operation repair pass will retry"
                    ),
                }
            }
            active
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&writer_id);
        });
    if let Err(error) = spawn_result {
        active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&writer_id);
        tracing::error!(%error, "failed to spawn file-operation repair thread");
    }
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
    guard.notification_operation_committed(appended);
    Ok(group_id)
}

fn execute_prepared(
    store: &impl JournalStore,
    kind: FileOperationKind,
    prepared: Vec<PreparedIntent>,
) -> FsOpsResult<Vec<FileOperationRecord>> {
    let batch = match store.persist(kind, prepared) {
        Ok(batch) => batch,
        Err(error) => {
            store.schedule_repair();
            return Err(error);
        }
    };
    let mut intents = batch.intents;
    let mut records = Vec::with_capacity(intents.len());
    let mut first_error = batch.first_error;

    for intent in &mut intents {
        match execute_intent_with_store(store, intent) {
            Ok(Ok(record)) => records.push(record),
            Ok(Err(error)) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            Err(error) => {
                store.schedule_repair();
                return Err(error);
            }
        }
    }

    if let Some(error) = first_error {
        Err(error)
    } else {
        Ok(records)
    }
}

trait JournalStore {
    fn persist(
        &self,
        kind: FileOperationKind,
        prepared: Vec<PreparedIntent>,
    ) -> FsOpsResult<PersistedBatch>;
    fn validate_and_mark(
        &self,
        intent: &mut JournalIntent,
        source_identity: &str,
        target_parent_identity: Option<&str>,
    ) -> FsOpsResult<()>;
    fn record_recycle_phase(
        &self,
        intent: &mut JournalIntent,
        phase: RecyclePhase,
    ) -> FsOpsResult<()>;
    fn finalize(
        &self,
        intent: &JournalIntent,
        error: Option<&FsOpsError>,
    ) -> FsOpsResult<FileOperationRecord>;
    fn schedule_repair(&self);
}

#[cfg(test)]
struct DirectJournalStore<'a> {
    database: &'a Mutex<Database>,
}

#[cfg(test)]
impl JournalStore for DirectJournalStore<'_> {
    fn persist(
        &self,
        kind: FileOperationKind,
        prepared: Vec<PreparedIntent>,
    ) -> FsOpsResult<PersistedBatch> {
        let guard = self
            .database
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        persist_intents(&guard, kind, prepared)
    }

    fn validate_and_mark(
        &self,
        intent: &mut JournalIntent,
        source_identity: &str,
        target_parent_identity: Option<&str>,
    ) -> FsOpsResult<()> {
        let guard = self
            .database
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        validate_execution_source(&guard, intent, source_identity, target_parent_identity)?;
        mark_attempt(&guard, intent)
    }

    fn record_recycle_phase(
        &self,
        intent: &mut JournalIntent,
        phase: RecyclePhase,
    ) -> FsOpsResult<()> {
        let guard = self
            .database
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        update_recycle_phase(&guard, intent, phase)
    }

    fn finalize(
        &self,
        intent: &JournalIntent,
        error: Option<&FsOpsError>,
    ) -> FsOpsResult<FileOperationRecord> {
        let guard = self
            .database
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        finalize_intent(&guard, intent, error)
    }

    fn schedule_repair(&self) {
        tracing::warn!("direct test journal store cannot schedule production single-writer repair");
    }
}

struct WriterJournalStore {
    database: Arc<Mutex<Database>>,
    writer: crate::db::DbWriter,
}

impl WriterJournalStore {
    fn new(database: Arc<Mutex<Database>>, writer: crate::db::DbWriter) -> Self {
        Self { database, writer }
    }
}

impl JournalStore for WriterJournalStore {
    fn persist(
        &self,
        kind: FileOperationKind,
        prepared: Vec<PreparedIntent>,
    ) -> FsOpsResult<PersistedBatch> {
        self.writer
            .write_blocking(move |database| {
                persist_intents(database, kind, prepared).map_err(anyhow::Error::new)
            })
            .map_err(FsOpsError::from)
    }

    fn validate_and_mark(
        &self,
        intent: &mut JournalIntent,
        source_identity: &str,
        target_parent_identity: Option<&str>,
    ) -> FsOpsResult<()> {
        let mut journal_intent = intent.clone();
        let source_identity = source_identity.to_owned();
        let target_parent_identity = target_parent_identity.map(str::to_owned);
        let (attempt_count, phase) = self
            .writer
            .write_blocking(move |database| {
                validate_execution_source(
                    database,
                    &journal_intent,
                    &source_identity,
                    target_parent_identity.as_deref(),
                )
                .map_err(anyhow::Error::new)?;
                mark_attempt(database, &mut journal_intent).map_err(anyhow::Error::new)?;
                Ok((journal_intent.attempt_count, journal_intent.phase))
            })
            .map_err(FsOpsError::from)?;
        intent.attempt_count = attempt_count;
        intent.phase = phase;
        Ok(())
    }

    fn record_recycle_phase(
        &self,
        intent: &mut JournalIntent,
        phase: RecyclePhase,
    ) -> FsOpsResult<()> {
        let journal_intent = intent.clone();
        self.writer
            .write_blocking(move |database| {
                let mut journal_intent = journal_intent;
                update_recycle_phase(database, &mut journal_intent, phase)
                    .map_err(anyhow::Error::new)
            })
            .map_err(FsOpsError::from)?;
        intent.recycle_phase = phase;
        Ok(())
    }

    fn finalize(
        &self,
        intent: &JournalIntent,
        error: Option<&FsOpsError>,
    ) -> FsOpsResult<FileOperationRecord> {
        let intent = intent.clone();
        let error = error.cloned();
        self.writer
            .write_authoritative_blocking(move |database| {
                finalize_intent(database, &intent, error.as_ref()).map_err(anyhow::Error::new)
            })
            .map_err(FsOpsError::from)
    }

    fn schedule_repair(&self) {
        schedule_pending_repair(Arc::clone(&self.database), self.writer.clone());
    }
}

fn execute_intent_with_store(
    store: &impl JournalStore,
    intent: &mut JournalIntent,
) -> FsOpsResult<FsOpsResult<FileOperationRecord>> {
    if let Some(code) = intent.pending_error_code.as_deref() {
        let error = persisted_error(code, intent.pending_error_detail.as_deref());
        store.finalize(intent, Some(&error))?;
        return Ok(Err(error));
    }

    if intent.source_path == intent.target_path.as_deref().unwrap_or(Path::new("")) {
        return store.finalize(intent, None).map(Ok);
    }

    let source = open_verified_execution_source(intent)?;
    let target_parent = open_verified_execution_target_parent(intent)?;
    store.validate_and_mark(
        intent,
        source.identity(),
        target_parent.as_ref().map(|parent| parent.identity()),
    )?;
    let fs_result = if intent.kind == FileOperationKind::DeleteRecycle {
        match perform_recycle_action_with_store(
            store,
            intent,
            source,
            target_parent.as_ref().ok_or_else(|| {
                interrupted_error("recycle execution has no protected staging parent")
            })?,
        ) {
            Ok(result) => result,
            Err(error) => {
                store.schedule_repair();
                return Err(error);
            }
        }
    } else {
        perform_filesystem_action(intent, source, target_parent.as_ref())
    };
    match fs_result {
        Ok(()) => store.finalize(intent, None).map(Ok),
        Err(error) => {
            let operation_error = filesystem_operation_error(intent, error);
            if intent.kind == FileOperationKind::DeleteRecycle {
                reconcile_recycle_action_failure(intent)?;
            }
            store.finalize(intent, Some(&operation_error))?;
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

    let source = open_verified_execution_source(intent)?;
    let target_parent = open_verified_execution_target_parent(intent)?;
    validate_execution_source(
        database,
        intent,
        source.identity(),
        target_parent.as_ref().map(|parent| parent.identity()),
    )?;
    mark_attempt(database, intent)?;
    let fs_result = if intent.kind == FileOperationKind::DeleteRecycle {
        perform_recycle_action_direct(
            database,
            intent,
            source,
            target_parent.as_ref().ok_or_else(|| {
                interrupted_error("recycle execution has no protected staging parent")
            })?,
        )?
    } else {
        perform_filesystem_action(intent, source, target_parent.as_ref())
    };
    match fs_result {
        Ok(()) => finalize_intent(database, intent, None).map(Ok),
        Err(error) => {
            let operation_error = filesystem_operation_error(intent, error);
            if intent.kind == FileOperationKind::DeleteRecycle {
                reconcile_recycle_action_failure(intent)?;
            }
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
        ReconcileAction::RestoreRecycle => restore_staged_recycle_for_retry(database, intent),
        ReconcileAction::Fail(error) => {
            finalize_intent(database, intent, Some(&error))?;
            Ok(Err(error))
        }
    }
}

enum ReconcileAction {
    FinalizeSuccess,
    Retry,
    RestoreRecycle,
    Fail(FsOpsError),
}

fn reconcile_filesystem_state(database: &Database, intent: &JournalIntent) -> ReconcileAction {
    match intent.kind {
        FileOperationKind::DeletePermanent => {
            reconcile_delete_filesystem_state(database, intent, probe_path(&intent.source_path))
        }
        FileOperationKind::DeleteRecycle => reconcile_recycle_filesystem_state(
            database,
            intent,
            probe_path(&intent.source_path),
            intent
                .target_path
                .as_deref()
                .map(probe_path)
                .unwrap_or_else(|| {
                    PathProbe::Error(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "recycle intent has no staging path",
                    ))
                }),
        ),
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

fn reconcile_recycle_filesystem_state(
    database: &Database,
    intent: &JournalIntent,
    source: PathProbe,
    staging: PathProbe,
) -> ReconcileAction {
    if let PathProbe::Error(error) = source {
        return ReconcileAction::Fail(probe_interrupted_error(
            &intent.source_path,
            "recycle source",
            error,
        ));
    }
    let staging_path = intent.target_path.as_deref().unwrap_or(Path::new(""));
    if let PathProbe::Error(error) = staging {
        return ReconcileAction::Fail(probe_interrupted_error(
            staging_path,
            "recycle staging path",
            error,
        ));
    }
    match (source, staging, intent.phase) {
        (PathProbe::NotFound, PathProbe::NotFound, JournalPhase::Executing)
            if intent.recycle_phase == RecyclePhase::ShellCompleted =>
        {
            match verify_entity_snapshot(database, intent) {
                Ok(()) => ReconcileAction::FinalizeSuccess,
                Err(error) => ReconcileAction::Fail(error),
            }
        }
        (PathProbe::NotFound, PathProbe::NotFound, JournalPhase::Executing) => {
            ReconcileAction::Fail(interrupted_error(
                "recycle source and staging are missing without a durable Shell completion",
            ))
        }
        (PathProbe::NotFound, PathProbe::Exists(metadata), JournalPhase::Executing) => {
            match verify_staging_identity(intent, staging_path, &metadata) {
                Ok(()) => ReconcileAction::RestoreRecycle,
                Err(error) => ReconcileAction::Fail(error),
            }
        }
        (PathProbe::Exists(metadata), PathProbe::NotFound, JournalPhase::Prepared) => {
            match validate_prepared_source_replay(database, intent, &metadata) {
                Ok(()) => ReconcileAction::Retry,
                Err(error) => ReconcileAction::Fail(error),
            }
        }
        (PathProbe::Exists(_), PathProbe::NotFound, JournalPhase::Executing) => {
            ReconcileAction::Fail(interrupted_error(
                "recycle source still exists after staging may have started; refusing an unknown replay",
            ))
        }
        (PathProbe::NotFound, PathProbe::Exists(_), JournalPhase::Prepared) => {
            ReconcileAction::Fail(interrupted_error(
                "recycle staging exists before filesystem execution was durably started",
            ))
        }
        (PathProbe::Exists(_), PathProbe::Exists(_), _) => {
            ReconcileAction::Fail(interrupted_error(
                "recycle source and staging path both exist during recovery",
            ))
        }
        (PathProbe::NotFound, PathProbe::NotFound, JournalPhase::Prepared) => {
            ReconcileAction::Fail(interrupted_error(
                "recycle source disappeared before filesystem execution started",
            ))
        }
        (_, _, JournalPhase::Legacy) => ReconcileAction::Fail(interrupted_error(
            "legacy recycle intent has no durable execution phase",
        )),
        (PathProbe::Error(_), _, _) | (_, PathProbe::Error(_), _) => {
            unreachable!("probe errors returned above")
        }
    }
}

fn restore_staged_recycle_for_retry(
    database: &Database,
    intent: &mut JournalIntent,
) -> FsOpsResult<FsOpsResult<FileOperationRecord>> {
    restore_recycle_staging(intent)?;
    reset_recycle_for_retry(database, intent)?;
    execute_intent(database, intent)
}

fn reset_recycle_for_retry(database: &Database, intent: &mut JournalIntent) -> FsOpsResult<()> {
    let transaction = Transaction::new_unchecked(
        database.connection_for_fsops(),
        TransactionBehavior::Immediate,
    )?;
    let updated = transaction.execute(
        "UPDATE file_operations SET journal_phase = 'prepared', recycle_phase = 'none' WHERE id = ?1 AND status = 'pending' AND journal_phase = 'executing'",
        [intent.id],
    )?;
    if updated != 1 {
        return Err(FsOpsError::new(
            FsOpsErrorCode::FsError,
            format!("recycle intent {} could not return to prepared", intent.id),
        ));
    }
    transaction.commit()?;
    intent.phase = JournalPhase::Prepared;
    intent.recycle_phase = RecyclePhase::None;
    Ok(())
}

fn restore_recycle_staging(intent: &JournalIntent) -> FsOpsResult<()> {
    let staging_path = intent
        .target_path
        .as_deref()
        .ok_or_else(|| interrupted_error("recycle intent has no staging path to restore"))?;
    let staged = super::platform::VerifiedSource::open(staging_path).map_err(|error| {
        interrupted_error(format!(
            "cannot open recycle staging '{}': {error}",
            staging_path.display()
        ))
    })?;
    let expected = required_staging_identity(intent)?;
    if staged.identity() != expected {
        return Err(interrupted_error(
            "recycle staging identity changed before restoration",
        ));
    }
    let source_parent_path = intent
        .source_path
        .parent()
        .ok_or_else(|| interrupted_error("recycle source has no parent directory"))?;
    let source_parent =
        super::platform::VerifiedTargetParent::open(source_parent_path).map_err(|error| {
            interrupted_error(format!(
                "cannot open recycle restore parent '{}': {error}",
                source_parent_path.display()
            ))
        })?;
    if source_parent.identity() != intent.target_parent_identity.as_deref().unwrap_or_default() {
        return Err(interrupted_error(
            "recycle restore parent identity changed after staging",
        ));
    }
    staged
        .rename_no_replace(
            &source_parent,
            intent
                .source_path
                .file_name()
                .ok_or_else(|| interrupted_error("recycle source has no relative leaf"))?,
        )
        .map_err(|error| {
            interrupted_error(format!(
                "cannot restore recycle staging to '{}': {error}",
                intent.source_path.display()
            ))
        })
}

fn reconcile_recycle_action_failure(intent: &JournalIntent) -> FsOpsResult<()> {
    let staging_path = intent
        .target_path
        .as_deref()
        .ok_or_else(|| interrupted_error("recycle action failed without a staging path"))?;
    match (probe_path(&intent.source_path), probe_path(staging_path)) {
        (PathProbe::NotFound, PathProbe::NotFound) => Err(interrupted_error(
            "recycle source and staging disappeared before Shell completion was durably recorded",
        )),
        (PathProbe::NotFound, PathProbe::Exists(metadata)) => {
            verify_staging_identity(intent, staging_path, &metadata)?;
            restore_recycle_staging(intent)?;
            Ok(())
        }
        (PathProbe::Exists(metadata), PathProbe::NotFound) => {
            verify_source_identity(intent, &intent.source_path, &metadata)?;
            Ok(())
        }
        (PathProbe::Exists(source_metadata), PathProbe::Exists(staging_metadata)) => {
            verify_source_identity(intent, &intent.source_path, &source_metadata)?;
            verify_staging_identity(intent, staging_path, &staging_metadata)?;
            Err(interrupted_error(
                "recycle source and staging both exist after Shell failure",
            ))
        }
        (PathProbe::Error(error), _) => Err(probe_interrupted_error(
            &intent.source_path,
            "recycle source after failure",
            error,
        )),
        (_, PathProbe::Error(error)) => Err(probe_interrupted_error(
            staging_path,
            "recycle staging after failure",
            error,
        )),
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
    verify_source_identity(intent, &intent.source_path, metadata)?;
    validate_target_parent_snapshot(intent)
}

fn validate_target_parent_snapshot(intent: &JournalIntent) -> FsOpsResult<()> {
    if intent.kind == FileOperationKind::DeletePermanent {
        return Ok(());
    }
    open_verified_execution_target_parent(intent).map(|_| ())
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

fn verify_staging_identity(
    intent: &JournalIntent,
    staging_path: &Path,
    metadata: &fs::Metadata,
) -> FsOpsResult<()> {
    let expected = required_staging_identity(intent)?;
    let actual = filesystem_identity(staging_path, metadata)
        .ok_or_else(|| interrupted_error("filesystem did not provide a stable staging identity"))?;
    if actual != expected {
        return Err(interrupted_error(
            "recycle staging identity changed before recovery",
        ));
    }
    Ok(())
}

fn open_verified_execution_source(
    intent: &JournalIntent,
) -> FsOpsResult<super::platform::VerifiedSource> {
    let expected = intent.source_identity.as_deref().ok_or_else(|| {
        interrupted_error("file-operation execution requires a persisted stable source identity")
    })?;
    let source = super::platform::VerifiedSource::open(&intent.source_path).map_err(|error| {
        FsOpsError::new(
            FsOpsErrorCode::FsError,
            format!(
                "cannot open protected file-operation source '{}': {error}",
                intent.source_path.display()
            ),
        )
    })?;
    if source.identity() != expected {
        return Err(interrupted_error(
            "file-operation source identity changed before execution",
        ));
    }
    Ok(source)
}

fn open_verified_execution_target_parent(
    intent: &JournalIntent,
) -> FsOpsResult<Option<super::platform::VerifiedTargetParent>> {
    if intent.kind == FileOperationKind::DeletePermanent {
        return Ok(None);
    }
    let target_parent_path = intent
        .target_path
        .as_deref()
        .and_then(Path::parent)
        .ok_or_else(|| interrupted_error("file-operation target has no parent directory"))?;
    let expected = intent.target_parent_identity.as_deref().ok_or_else(|| {
        interrupted_error("file-operation execution requires a persisted target-parent identity")
    })?;
    let parent =
        super::platform::VerifiedTargetParent::open(target_parent_path).map_err(|error| {
            interrupted_error(format!(
                "cannot open protected target parent '{}': {error}",
                target_parent_path.display()
            ))
        })?;
    if parent.identity() != expected {
        return Err(interrupted_error(
            "file-operation target parent identity changed before execution",
        ));
    }
    Ok(Some(parent))
}

fn validate_execution_source(
    database: &Database,
    intent: &JournalIntent,
    source_identity: &str,
    target_parent_identity: Option<&str>,
) -> FsOpsResult<()> {
    if source_identity != intent.source_identity.as_deref().unwrap_or_default() {
        return Err(interrupted_error(
            "protected file-operation handle does not match the journal identity",
        ));
    }
    if intent.kind != FileOperationKind::DeletePermanent
        && target_parent_identity != intent.target_parent_identity.as_deref()
    {
        return Err(interrupted_error(
            "protected target-parent handle does not match the journal identity",
        ));
    }
    verify_entity_snapshot(database, intent)?;
    if intent.kind == FileOperationKind::DeletePermanent {
        let root_id = intent
            .root_id
            .ok_or_else(|| interrupted_error("permanent-delete intent has no root id"))?;
        let root_path = enabled_root_path(database.connection_for_fsops(), root_id)?;
        let metadata = match probe_path(&intent.source_path) {
            PathProbe::Exists(metadata) => metadata,
            PathProbe::NotFound => {
                return Err(interrupted_error(
                    "protected permanent-delete source disappeared before execution",
                ))
            }
            PathProbe::Error(error) => {
                return Err(probe_interrupted_error(
                    &intent.source_path,
                    "permanent-delete source",
                    error,
                ))
            }
        };
        validate_permanent_delete_path(
            &intent.source_path,
            &root_path,
            &metadata,
            Some(source_identity),
        )?;
    }
    Ok(())
}

fn validate_permanent_delete_replay(
    database: &Database,
    intent: &JournalIntent,
    metadata: &fs::Metadata,
) -> FsOpsResult<()> {
    let expected_identity = intent.source_identity.as_deref().ok_or_else(|| {
        interrupted_error("permanent-delete replay requires a persisted stable source identity")
    })?;
    verify_entity_snapshot(database, intent)?;
    let root_id = intent
        .root_id
        .ok_or_else(|| interrupted_error("permanent-delete intent has no root id"))?;
    let root_path = enabled_root_path(database.connection_for_fsops(), root_id)?;
    validate_permanent_delete_path(
        &intent.source_path,
        &root_path,
        metadata,
        Some(expected_identity),
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

fn perform_filesystem_action(
    intent: &JournalIntent,
    source: super::platform::VerifiedSource,
    target_parent: Option<&super::platform::VerifiedTargetParent>,
) -> std::io::Result<()> {
    match intent.kind {
        FileOperationKind::Rename | FileOperationKind::Move => {
            let target_path = intent.target_path.as_deref().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing target path")
            })?;
            source.rename_no_replace(
                target_parent.ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "missing protected target parent",
                    )
                })?,
                target_path.file_name().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "target path has no relative leaf",
                    )
                })?,
            )
        }
        FileOperationKind::DeletePermanent => source.delete_permanent(),
        FileOperationKind::DeleteRecycle => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "recycle requires the journaled handoff path",
        )),
    }
}

fn perform_recycle_action_with_store(
    store: &impl JournalStore,
    intent: &mut JournalIntent,
    source: super::platform::VerifiedSource,
    target_parent: &super::platform::VerifiedTargetParent,
) -> FsOpsResult<std::io::Result<()>> {
    let staging_path = required_recycle_staging_path(intent)?.to_path_buf();
    if source.identity() != required_staging_identity(intent)? {
        return Err(interrupted_error(
            "protected recycle source does not match the persisted staging identity",
        ));
    }
    if let Err(error) = source.rename_no_replace(
        target_parent,
        staging_path
            .file_name()
            .ok_or_else(|| interrupted_error("recycle staging path has no relative leaf"))?,
    ) {
        return Ok(Err(error));
    }
    store.record_recycle_phase(intent, RecyclePhase::Staged)?;

    let recycle_guard = match source.recycle_identity_guard() {
        Ok(guard) => guard,
        Err(error) => return Ok(Err(error)),
    };
    // trash 5.2.6 creates an IShellItem from a parsing name. Keep a second
    // same-object handle that shares DELETE while releasing the exclusive
    // source handle; after Shell returns, the guard proves that this file id
    // moved into $Recycle.Bin before shell_completed becomes durable.
    drop(source);
    if let Err(error) = trash::delete(&staging_path) {
        return Ok(Err(std::io::Error::other(error.to_string())));
    }
    if let Err(error) = recycle_guard.verify_shell_handoff(required_staging_identity(intent)?) {
        return Ok(Err(error));
    }
    store.record_recycle_phase(intent, RecyclePhase::ShellCompleted)?;
    Ok(Ok(()))
}

fn perform_recycle_action_direct(
    database: &Database,
    intent: &mut JournalIntent,
    source: super::platform::VerifiedSource,
    target_parent: &super::platform::VerifiedTargetParent,
) -> FsOpsResult<std::io::Result<()>> {
    let staging_path = required_recycle_staging_path(intent)?.to_path_buf();
    if source.identity() != required_staging_identity(intent)? {
        return Err(interrupted_error(
            "protected recycle source does not match the persisted staging identity",
        ));
    }
    if let Err(error) = source.rename_no_replace(
        target_parent,
        staging_path
            .file_name()
            .ok_or_else(|| interrupted_error("recycle staging path has no relative leaf"))?,
    ) {
        return Ok(Err(error));
    }
    update_recycle_phase(database, intent, RecyclePhase::Staged)?;
    let recycle_guard = match source.recycle_identity_guard() {
        Ok(guard) => guard,
        Err(error) => return Ok(Err(error)),
    };
    drop(source);
    if let Err(error) = trash::delete(&staging_path) {
        return Ok(Err(std::io::Error::other(error.to_string())));
    }
    if let Err(error) = recycle_guard.verify_shell_handoff(required_staging_identity(intent)?) {
        return Ok(Err(error));
    }
    update_recycle_phase(database, intent, RecyclePhase::ShellCompleted)?;
    Ok(Ok(()))
}

fn required_recycle_staging_path(intent: &JournalIntent) -> FsOpsResult<&Path> {
    intent
        .target_path
        .as_deref()
        .ok_or_else(|| interrupted_error("recycle intent has no protected staging path"))
}

fn required_staging_identity(intent: &JournalIntent) -> FsOpsResult<&str> {
    intent
        .staging_identity
        .as_deref()
        .ok_or_else(|| interrupted_error("recycle intent has no persisted staging identity"))
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
    database: &Database,
    kind: FileOperationKind,
    prepared: Vec<PreparedIntent>,
) -> FsOpsResult<PersistedBatch> {
    let transaction = Transaction::new_unchecked(
        database.connection_for_fsops(),
        TransactionBehavior::Immediate,
    )?;
    let group_id = Uuid::now_v7().to_string();
    let started_at = now_millis();
    let mut intents = Vec::with_capacity(prepared.len());
    let total = prepared.len() as i64;
    let mut first_error = None;
    let mut preflight_failures = 0i64;

    for prepared in prepared {
        let error_code = prepared
            .preflight_error
            .as_ref()
            .map(|error| error.code.as_str());
        let error_detail = prepared
            .preflight_error
            .as_ref()
            .map(|error| error.message.as_str());
        let status = if prepared.preflight_error.is_some() {
            "failed"
        } else {
            "pending"
        };
        transaction.execute(
            r#"INSERT INTO file_operations(
                 operation, source_path, target_path, status, created_at, finished_at,
                 operation_group_id, entity_kind, entity_id, root_id,
                 target_folder_id, attempt_count, journal_phase,
                 source_identity, target_parent_identity, staging_identity,
                 recycle_phase, error_code, error
               ) VALUES (?1, ?2, ?3, ?4, unixepoch(),
                         CASE WHEN ?4 = 'failed' THEN unixepoch() ELSE NULL END,
                         ?5, ?6, ?7, ?8, ?9, 0, 'prepared', ?10, ?11, ?12,
                         'none', ?13, ?14)"#,
            params![
                prepared.kind.as_str(),
                prepared.source_path,
                prepared.target_path,
                status,
                group_id,
                prepared.entity_kind.map(EntityKind::as_str),
                prepared.entity_id,
                prepared.root_id,
                prepared.target_folder_id,
                prepared.source_identity,
                prepared.target_parent_identity,
                prepared.staging_identity,
                error_code,
                error_detail,
            ],
        )?;
        if let Some(error) = prepared.preflight_error {
            preflight_failures += 1;
            if first_error.is_none() {
                first_error = Some(error);
            }
            continue;
        }
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
            target_parent_identity: prepared.target_parent_identity,
            staging_identity: prepared.staging_identity,
            recycle_phase: RecyclePhase::None,
            pending_error_code: error_code.map(str::to_owned),
            pending_error_detail: error_detail.map(str::to_owned),
        });
    }

    let mut operation = new_group_operation(&group_id, kind, total, started_at);
    let phase_appended = database.publish_notification_operation_in_transaction(
        &transaction,
        &mut operation,
        PublishMode::Phase,
    )?;
    let state_appended = if preflight_failures > 0 {
        publish_group_state_in_transaction(database, &transaction, &group_id)?
    } else {
        false
    };
    transaction.commit()?;
    database.notification_operation_committed(phase_appended || state_appended);
    Ok(PersistedBatch {
        intents,
        first_error,
    })
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

fn update_recycle_phase(
    database: &Database,
    intent: &mut JournalIntent,
    phase: RecyclePhase,
) -> FsOpsResult<()> {
    let transaction = Transaction::new_unchecked(
        database.connection_for_fsops(),
        TransactionBehavior::Immediate,
    )?;
    let updated = transaction.execute(
        r#"UPDATE file_operations SET recycle_phase = ?1
           WHERE id = ?2 AND status = 'pending' AND journal_phase = 'executing'"#,
        params![phase.as_str(), intent.id],
    )?;
    if updated != 1 {
        return Err(FsOpsError::new(
            FsOpsErrorCode::FsError,
            format!(
                "recycle intent {} could not record phase {}",
                intent.id,
                phase.as_str()
            ),
        ));
    }
    transaction.commit()?;
    intent.recycle_phase = phase;
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
        capture_successful_intent(&transaction, intent)?;
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
    database.notification_operation_committed(appended);
    Ok(record)
}

fn capture_successful_intent(
    transaction: &Transaction<'_>,
    intent: &JournalIntent,
) -> FsOpsResult<()> {
    let entity_id = intent.entity_id.ok_or_else(|| {
        FsOpsError::new(
            FsOpsErrorCode::FsError,
            "file operation is missing entity id",
        )
    })?;
    let entity_uuid: Vec<u8> = match intent.entity_kind {
        Some(EntityKind::File) => transaction.query_row(
            "SELECT asset_uuid FROM files WHERE id=?1",
            [entity_id],
            |row| row.get(0),
        )?,
        Some(EntityKind::Folder) => transaction.query_row(
            "SELECT folder_uuid FROM folders WHERE id=?1",
            [entity_id],
            |row| row.get(0),
        )?,
        None => return Ok(()),
    };
    let entity = crate::sync::capture::entity_uuid(&entity_uuid).map_err(FsOpsError::from)?;
    let operation = match intent.kind {
        FileOperationKind::Rename | FileOperationKind::Move => {
            let (parent_uuid, name): (Option<Vec<u8>>, String) = match intent.entity_kind {
                    Some(EntityKind::File) => transaction.query_row(
                        "SELECT p.folder_uuid,f.name FROM files f JOIN folders p ON p.id=f.folder_id WHERE f.id=?1",
                        [entity_id],
                        |row| Ok((row.get(0)?,row.get(1)?)),
                    )?,
                    Some(EntityKind::Folder) => transaction.query_row(
                        "SELECT COALESCE(p.folder_uuid,r.root_uuid),f.name FROM folders f JOIN roots r ON r.id=f.root_id LEFT JOIN folders p ON p.id=f.parent_id WHERE f.id=?1",
                        [entity_id],
                        |row| Ok((row.get(0)?,row.get(1)?)),
                    )?,
                    None => return Ok(()),
            };
            let mut supersedes = std::collections::BTreeSet::new();
            let mut statement = transaction
                .prepare("SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1")?;
            let rows = statement.query_map([&entity_uuid], |row| row.get::<_, Vec<u8>>(0))?;
            for row in rows {
                supersedes.insert(crate::sync::types::RevisionUuid(
                    Uuid::from_slice(&row?).map_err(anyhow::Error::from)?,
                ));
            }
            crate::sync::types::Operation::SetLocation {
                revision_uuid: crate::sync::types::RevisionUuid::new(),
                parent_uuid: parent_uuid
                    .as_deref()
                    .map(crate::sync::capture::entity_uuid)
                    .transpose()
                    .map_err(FsOpsError::from)?,
                name,
                supersedes,
            }
        }
        FileOperationKind::DeleteRecycle | FileOperationKind::DeletePermanent => {
            if intent.entity_kind == Some(EntityKind::Folder) {
                let initialized: i64 = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sync_local_state WHERE singleton=1)",
                    [],
                    |row| row.get(0),
                )?;
                if initialized != 0 {
                    crate::sync::bulk::begin_subtree_delete(transaction, &entity_uuid)
                        .map_err(FsOpsError::from)?;
                }
                return Ok(());
            }
            crate::sync::types::Operation::Delete {
                token: crate::sync::types::EventUuid::new(),
            }
        }
    };
    crate::sync::capture::capture_catalog_mutation_if_initialized(
        transaction,
        crate::sync::capture::MutationOrigin::LocalApi,
        entity,
        operation,
    )
    .map_err(FsOpsError::from)?;
    Ok(())
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
        r#"SELECT id, operation, source_path,
                  CASE WHEN operation LIKE 'delete_%' THEN NULL ELSE target_path END,
                  status,
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
                  attempt_count, journal_phase, source_identity,
                  target_parent_identity, staging_identity, recycle_phase,
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
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, String>(14)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, Option<String>>(16)?,
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
                target_parent_identity,
                staging_identity,
                recycle_phase,
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
                    target_parent_identity,
                    staging_identity,
                    recycle_phase: RecyclePhase::parse(&recycle_phase)?,
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
                        target_parent_identity: None,
                        staging_identity: None,
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
                        target_parent_identity: None,
                        staging_identity: None,
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
    apply_target_parent_identity_preflight(&mut prepared);
    Ok(prepared)
}

fn prepare_move(
    database: &Mutex<Database>,
    request: &MoveRequest,
) -> FsOpsResult<Vec<PreparedIntent>> {
    let mut prepared = {
        let guard = database.lock().expect("database mutex poisoned");
        let conn = guard.connection_for_fsops();
        let (file_ids, folder_ids) =
            normalize_batch_ids(conn, &request.file_ids, &request.folder_ids)?;
        let target = load_folder_path(conn, request.target_folder_id)?;
        let target_error = target.as_ref().is_none().then(|| {
            FsOpsError::new(
                FsOpsErrorCode::NotFound,
                format!("target folder not found: {}", request.target_folder_id),
            )
        });
        let mut prepared = Vec::with_capacity(file_ids.len() + folder_ids.len());
        for file_id in file_ids {
            prepared.push(prepare_move_file(
                conn,
                file_id,
                target.as_ref(),
                target_error.clone(),
            )?);
        }
        for folder_id in folder_ids {
            prepared.push(prepare_move_folder(
                conn,
                folder_id,
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
        apply_target_parent_identity_preflight(intent);
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
        target_parent_identity: None,
        staging_identity: None,
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
        target_parent_identity: None,
        staging_identity: None,
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
        let (file_ids, folder_ids) =
            normalize_batch_ids(conn, &request.file_ids, &request.folder_ids)?;
        let mut prepared = Vec::with_capacity(file_ids.len() + folder_ids.len());
        for file_id in file_ids {
            prepared.push(match load_file_info(conn, file_id)? {
                Some(info) => {
                    let root_path = request
                        .permanent
                        .then(|| enabled_root_path(conn, info.root_id));
                    let (delete_root_path, preflight_error) = split_root_result(root_path);
                    PreparedIntent {
                        kind,
                        entity_kind: Some(EntityKind::File),
                        entity_id: Some(file_id),
                        root_id: Some(info.root_id),
                        source_path: info.path.to_string_lossy().into_owned(),
                        target_path: (!request.permanent)
                            .then(|| recycle_staging_path(&info.path))
                            .map(|path| path.to_string_lossy().into_owned()),
                        target_folder_id: None,
                        delete_root_path,
                        source_identity: None,
                        target_parent_identity: None,
                        staging_identity: None,
                        preflight_error,
                    }
                }
                None => missing_intent(
                    kind,
                    EntityKind::File,
                    file_id,
                    "delete",
                    FsOpsError::new(
                        FsOpsErrorCode::NotFound,
                        format!("file not found: {file_id}"),
                    ),
                ),
            });
        }
        for folder_id in folder_ids {
            prepared.push(match load_folder_info(conn, folder_id)? {
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
                        entity_id: Some(folder_id),
                        root_id: Some(info.root_id),
                        source_path: info.path.to_string_lossy().into_owned(),
                        target_path: (!request.permanent)
                            .then(|| recycle_staging_path(&info.path))
                            .map(|path| path.to_string_lossy().into_owned()),
                        target_folder_id: None,
                        delete_root_path: root_path,
                        source_identity: None,
                        target_parent_identity: None,
                        staging_identity: None,
                        preflight_error: error,
                    }
                }
                None => missing_intent(
                    kind,
                    EntityKind::Folder,
                    folder_id,
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
            apply_target_parent_identity_preflight(intent);
            if intent.preflight_error.is_none() {
                intent.staging_identity = intent.source_identity.clone();
            }
        }
    }
    Ok(prepared)
}

fn normalize_batch_ids(
    conn: &rusqlite::Connection,
    file_ids: &[i64],
    folder_ids: &[i64],
) -> FsOpsResult<(Vec<i64>, Vec<i64>)> {
    let selected_folders: BTreeSet<i64> = folder_ids.iter().copied().collect();
    let mut logical_folders = Vec::with_capacity(selected_folders.len());
    for folder_id in selected_folders.iter().copied() {
        if !has_selected_folder_ancestor(conn, folder_id, &selected_folders)? {
            logical_folders.push(folder_id);
        }
    }

    let mut logical_files = Vec::new();
    for file_id in file_ids.iter().copied().collect::<BTreeSet<_>>() {
        let folder_id: Option<i64> = conn
            .query_row(
                "SELECT folder_id FROM files WHERE id = ?1 AND status = 'active'",
                [file_id],
                |row| row.get(0),
            )
            .optional()?;
        let covered = match folder_id {
            Some(folder_id) => folder_or_ancestor_is_selected(conn, folder_id, &selected_folders)?,
            None => false,
        };
        if !covered {
            logical_files.push(file_id);
        }
    }
    Ok((logical_files, logical_folders))
}

fn has_selected_folder_ancestor(
    conn: &rusqlite::Connection,
    folder_id: i64,
    selected: &BTreeSet<i64>,
) -> FsOpsResult<bool> {
    let parent_id: Option<Option<i64>> = conn
        .query_row(
            "SELECT parent_id FROM folders WHERE id = ?1 AND status = 'active'",
            [folder_id],
            |row| row.get(0),
        )
        .optional()?;
    match parent_id.flatten() {
        Some(parent_id) => folder_or_ancestor_is_selected(conn, parent_id, selected),
        None => Ok(false),
    }
}

fn folder_or_ancestor_is_selected(
    conn: &rusqlite::Connection,
    mut folder_id: i64,
    selected: &BTreeSet<i64>,
) -> FsOpsResult<bool> {
    let mut visited = HashSet::new();
    loop {
        if selected.contains(&folder_id) {
            return Ok(true);
        }
        if !visited.insert(folder_id) {
            return Ok(false);
        }
        let parent_id: Option<Option<i64>> = conn
            .query_row(
                "SELECT parent_id FROM folders WHERE id = ?1 AND status = 'active'",
                [folder_id],
                |row| row.get(0),
            )
            .optional()?;
        match parent_id.flatten() {
            Some(parent_id) => folder_id = parent_id,
            None => return Ok(false),
        }
    }
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
                Ok(Some(identity)) => intent.source_identity = Some(identity),
                Ok(None) => {
                    intent.preflight_error = Some(FsOpsError::new(
                        FsOpsErrorCode::Interrupted,
                        "filesystem did not provide a stable permanent-delete identity",
                    ));
                }
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
    match super::platform::VerifiedSource::open(source_path) {
        Ok(source) => intent.source_identity = Some(source.identity().to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            intent.preflight_error = Some(FsOpsError::new(
                FsOpsErrorCode::NotFound,
                format!(
                    "file-operation source '{}' does not exist",
                    source_path.display()
                ),
            ));
        }
        Err(error) => {
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

fn apply_target_parent_identity_preflight(intent: &mut PreparedIntent) {
    if intent.preflight_error.is_some() {
        return;
    }
    let Some(target_parent) = intent
        .target_path
        .as_deref()
        .map(Path::new)
        .and_then(Path::parent)
    else {
        intent.preflight_error = Some(FsOpsError::new(
            FsOpsErrorCode::InvalidRequest,
            "file-operation target has no parent directory",
        ));
        return;
    };
    match super::platform::VerifiedTargetParent::open(target_parent) {
        Ok(parent) => intent.target_parent_identity = Some(parent.identity().to_owned()),
        Err(error) => {
            intent.preflight_error = Some(FsOpsError::new(
                FsOpsErrorCode::FsError,
                format!(
                    "cannot open protected target parent '{}': {error}",
                    target_parent.display()
                ),
            ));
        }
    }
}

fn recycle_staging_path(source_path: &Path) -> PathBuf {
    let parent = source_path.parent().unwrap_or_else(|| Path::new(""));
    parent.join(format!(".megle-recycle-{}", Uuid::now_v7()))
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
        target_parent_identity: None,
        staging_identity: None,
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
        target_parent_identity: None,
        staging_identity: None,
        preflight_error: Some(error),
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
