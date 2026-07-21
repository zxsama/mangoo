use anyhow::{anyhow, Context};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::Database;

const MAX_OUTBOX_PAGE: i64 = 200;
pub(super) const DEFAULT_PROGRESS_EMIT_INTERVAL_MS: i64 = 1_000;

/// The complete logical operation snapshot persisted in a head and delivered
/// through the Outbox. Strings intentionally mirror the Core API contract so
/// future task/file-operation producers can share this storage boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationOperation {
    pub operation_id: String,
    pub revision: i64,
    pub kind: String,
    pub category: String,
    pub origin: String,
    pub presentation: String,
    pub status: String,
    pub phase: Option<String>,
    pub title: String,
    pub summary: Option<String>,
    pub task_id: Option<i64>,
    pub root_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub file_id: Option<i64>,
    pub current_value: i64,
    pub total_value: Option<i64>,
    pub unit: Option<String>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub cancellable: bool,
    pub retryable: bool,
    pub attempt: i64,
    pub occurrence_count: i64,
    pub started_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationOutboxEvent {
    pub sequence: i64,
    pub operation: NotificationOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishMode {
    Phase,
    Progress,
    Terminal,
}

impl NotificationOperation {
    /// Thumbnail work has a separate aggregate operation. Keeping the task
    /// rejection here prevents an accidental per-file/per-thumbnail stream at
    /// the persistence boundary while still allowing `thumbnail_cache` heads.
    fn is_thumbnail_task(&self) -> bool {
        self.category == "task" && self.kind == "thumbnail"
    }
}

impl Database {
    /// Test-only convenience for publishing through an owned transaction.
    /// Production producers must publish inside their existing writer-owned
    /// transaction and notify only after that transaction commits.
    #[cfg(test)]
    pub(crate) fn publish_notification_operation(
        &self,
        operation: &mut NotificationOperation,
        mode: PublishMode,
    ) -> anyhow::Result<bool> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let appended =
            self.publish_notification_operation_in_transaction(&transaction, operation, mode)?;
        transaction.commit()?;
        self.notification_operation_committed(appended);
        Ok(appended)
    }

    /// Publish inside a caller-owned transaction. This method deliberately
    /// does not notify: callers must commit first, then notify the shared wake.
    pub(crate) fn publish_notification_operation_in_transaction(
        &self,
        transaction: &Transaction<'_>,
        operation: &mut NotificationOperation,
        mode: PublishMode,
    ) -> anyhow::Result<bool> {
        if operation.is_thumbnail_task() {
            return Err(anyhow!(
                "thumbnail tasks must use the thumbnail_cache aggregate"
            ));
        }

        let previous: Option<(i64, Option<i64>, i64)> = transaction
            .query_row(
                "SELECT revision, last_outbox_at, updated_at FROM notification_operation_heads WHERE operation_id = ?1",
                [&operation.operation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (next_revision, previous_last_outbox_at) = previous
            .map(|(revision, last_outbox_at, previous_updated_at)| {
                operation.updated_at = operation.updated_at.max(previous_updated_at);
                if let Some(completed_at) = operation.completed_at.as_mut() {
                    *completed_at = (*completed_at).max(operation.updated_at);
                }
                (revision.saturating_add(1), last_outbox_at)
            })
            .unwrap_or((1, None));
        let append = should_emit(
            mode,
            previous_last_outbox_at,
            operation.updated_at,
            self.notification_progress_interval_ms,
        );
        let dirty = !append;
        operation.revision = next_revision;
        let payload_json =
            serde_json::to_string(operation).context("serialize notification operation")?;

        transaction.execute(
            r#"
            INSERT INTO notification_operation_heads(
              operation_id, kind, status, presentation, revision, payload_json,
              dirty, last_outbox_at, updated_at, completed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(operation_id) DO UPDATE SET
              kind = excluded.kind,
              status = excluded.status,
              presentation = excluded.presentation,
              revision = excluded.revision,
              payload_json = excluded.payload_json,
              dirty = excluded.dirty,
              last_outbox_at = excluded.last_outbox_at,
              updated_at = excluded.updated_at,
              completed_at = excluded.completed_at
            "#,
            params![
                operation.operation_id,
                operation.kind,
                operation.status,
                operation.presentation,
                operation.revision,
                payload_json,
                dirty as i64,
                if append {
                    Some(operation.updated_at)
                } else {
                    previous_last_outbox_at
                },
                operation.updated_at,
                operation.completed_at,
            ],
        )?;

        if append {
            append_event_in_transaction(transaction, operation, operation.updated_at)?;
        }
        Ok(append)
    }

    pub fn list_notification_outbox(
        &self,
        after: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<NotificationOutboxEvent>> {
        let after = after.max(0);
        let limit = limit.clamp(1, MAX_OUTBOX_PAGE);
        let mut statement = self.connection.prepare(
            "SELECT sequence, payload_json FROM notification_outbox WHERE sequence > ?1 ORDER BY sequence ASC LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![after, limit], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        rows.into_iter()
            .map(|(sequence, payload_json)| {
                let operation = deserialize_operation(&payload_json, None)?;
                Ok(NotificationOutboxEvent {
                    sequence,
                    operation,
                })
            })
            .collect()
    }

    pub fn acknowledge_notification_outbox(&self, max_sequence: i64) -> anyhow::Result<usize> {
        if max_sequence < 0 {
            return Ok(0);
        }
        Ok(self.connection.execute(
            "DELETE FROM notification_outbox WHERE sequence <= ?1",
            [max_sequence],
        )?)
    }

    /// Emit the latest state of every throttled head without creating another
    /// logical revision. This is used by recovery/periodic flush paths.
    pub fn flush_dirty_notification_heads(&self) -> anyhow::Result<usize> {
        self.flush_notification_heads(None)
    }

    pub(crate) fn flush_due_notification_heads(&self, now_ms: i64) -> anyhow::Result<usize> {
        self.flush_notification_heads(Some(now_ms))
    }

    pub(crate) fn next_dirty_notification_flush_at(&self) -> anyhow::Result<Option<i64>> {
        Ok(self.connection.query_row(
            "SELECT MIN(COALESCE(last_outbox_at, updated_at) + ?1) FROM notification_operation_heads WHERE dirty = 1",
            [self.notification_progress_interval_ms],
            |row| row.get(0),
        )?)
    }

    fn flush_notification_heads(&self, due_at: Option<i64>) -> anyhow::Result<usize> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let dirty_heads =
            collect_dirty_heads(&transaction, due_at, self.notification_progress_interval_ms)?;
        let mut appended = 0usize;
        for (operation_id, revision, payload_json, updated_at) in dirty_heads {
            let operation = deserialize_operation(&payload_json, Some(&operation_id))?;
            if operation.revision != revision {
                return Err(anyhow!(
                    "notification head revision mismatch for {}",
                    operation_id
                ));
            }
            let emitted_at = due_at.unwrap_or(updated_at);
            if append_event_if_missing_in_transaction(&transaction, &operation, emitted_at)? {
                appended += 1;
            }
            transaction.execute(
                "UPDATE notification_operation_heads SET dirty = 0, last_outbox_at = ?2 WHERE operation_id = ?1 AND revision = ?3",
                params![operation_id, emitted_at, revision],
            )?;
        }
        transaction.commit()?;
        if appended > 0 {
            self.notification_outbox_wake.notify_waiters();
        }
        Ok(appended)
    }

    /// Mark non-task operations interrupted after a Core restart. Task heads
    /// remain for task startup recovery, which owns their task-row semantics.
    pub fn recover_interrupted_notification_operations(&self) -> anyhow::Result<usize> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let candidates = collect_recovery_candidates(&transaction)?;
        let now = super::unix_timestamp_millis();
        let mut recovered = 0usize;
        for (operation_id, payload_json) in candidates {
            let mut operation = deserialize_operation(&payload_json, Some(&operation_id))?;
            if operation.category == "task" {
                continue;
            }
            operation.status = "failed".to_string();
            operation.origin = "recovery".to_string();
            operation.error_code = Some("interrupted".to_string());
            operation.error_detail =
                Some("Core stopped before the operation completed".to_string());
            operation.completed_at = Some(now);
            operation.updated_at = now;
            self.publish_notification_operation_in_transaction(
                &transaction,
                &mut operation,
                PublishMode::Terminal,
            )?;
            recovered += 1;
        }
        transaction.commit()?;
        if recovered > 0 {
            self.notification_outbox_wake.notify_waiters();
        }
        Ok(recovered)
    }
}

fn should_emit(
    mode: PublishMode,
    previous_last_outbox_at: Option<i64>,
    now_ms: i64,
    progress_emit_interval_ms: i64,
) -> bool {
    match mode {
        PublishMode::Phase | PublishMode::Terminal => true,
        PublishMode::Progress => previous_last_outbox_at
            .map(|last| now_ms.saturating_sub(last) >= progress_emit_interval_ms)
            .unwrap_or(true),
    }
}

fn append_event_in_transaction(
    transaction: &Transaction<'_>,
    operation: &NotificationOperation,
    created_at: i64,
) -> anyhow::Result<()> {
    transaction.execute(
        "INSERT INTO notification_outbox(operation_id, revision, payload_json, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            operation.operation_id,
            operation.revision,
            serde_json::to_string(operation).context("serialize outbox event")?,
            created_at,
        ],
    )?;
    Ok(())
}

fn append_event_if_missing_in_transaction(
    transaction: &Transaction<'_>,
    operation: &NotificationOperation,
    created_at: i64,
) -> anyhow::Result<bool> {
    let inserted = transaction.execute(
        "INSERT INTO notification_outbox(operation_id, revision, payload_json, created_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(operation_id, revision) DO NOTHING",
        params![
            operation.operation_id,
            operation.revision,
            serde_json::to_string(operation).context("serialize outbox recovery event")?,
            created_at,
        ],
    )?;
    Ok(inserted > 0)
}

fn deserialize_operation(
    payload_json: &str,
    expected_operation_id: Option<&str>,
) -> anyhow::Result<NotificationOperation> {
    let operation: NotificationOperation =
        serde_json::from_str(payload_json).context("deserialize notification operation")?;
    if expected_operation_id.is_some_and(|expected| expected != operation.operation_id) {
        return Err(anyhow!(
            "notification payload operation id does not match its head"
        ));
    }
    Ok(operation)
}

fn collect_dirty_heads(
    transaction: &Transaction<'_>,
    due_at: Option<i64>,
    progress_emit_interval_ms: i64,
) -> anyhow::Result<Vec<(String, i64, String, i64)>> {
    let mut statement = transaction.prepare(
        "SELECT operation_id, revision, payload_json, updated_at FROM notification_operation_heads WHERE dirty = 1 AND (?1 IS NULL OR COALESCE(last_outbox_at, updated_at) + ?2 <= ?1) ORDER BY updated_at ASC, operation_id ASC",
    )?;
    let rows = statement
        .query_map(params![due_at, progress_emit_interval_ms], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn collect_recovery_candidates(
    transaction: &Transaction<'_>,
) -> anyhow::Result<Vec<(String, String)>> {
    let mut statement = transaction.prepare(
        r#"SELECT heads.operation_id, heads.payload_json
           FROM notification_operation_heads AS heads
           WHERE heads.status IN ('pending', 'running')
             AND NOT EXISTS (
               SELECT 1 FROM file_operations
               WHERE file_operations.operation_group_id = heads.operation_id
                 AND file_operations.status = 'pending'
             )
           ORDER BY heads.updated_at ASC, heads.operation_id ASC"#,
    )?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    };
    use std::thread;
    use std::time::Duration;

    use super::*;

    static NEXT_TEMP_DATABASE: AtomicUsize = AtomicUsize::new(0);

    fn migrated_database() -> Database {
        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        database
    }

    fn temporary_database_path(label: &str) -> PathBuf {
        let serial = NEXT_TEMP_DATABASE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "megle-notification-{label}-{}-{}-{serial}.sqlite",
            std::process::id(),
            super::super::unix_timestamp_millis()
        ))
    }

    fn head_revision_and_dirty(database: &Database, operation_id: &str) -> (i64, i64) {
        database
            .connection
            .query_row(
                "SELECT revision, dirty FROM notification_operation_heads WHERE operation_id = ?1",
                [operation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("notification head")
    }

    fn fixture_operation(
        operation_id: &str,
        kind: &str,
        status: &str,
        updated_at: i64,
    ) -> NotificationOperation {
        NotificationOperation {
            operation_id: operation_id.to_string(),
            revision: 0,
            kind: kind.to_string(),
            category: "task".to_string(),
            origin: "background".to_string(),
            presentation: "important".to_string(),
            status: status.to_string(),
            phase: Some("working".to_string()),
            title: "Fixture operation".to_string(),
            summary: None,
            task_id: Some(7),
            root_id: Some(3),
            folder_id: None,
            file_id: None,
            current_value: 10,
            total_value: Some(100),
            unit: Some("items".to_string()),
            error_code: None,
            error_detail: None,
            cancellable: true,
            retryable: false,
            attempt: 1,
            occurrence_count: 1,
            started_at: updated_at,
            updated_at,
            completed_at: None,
        }
    }

    #[test]
    fn progress_is_coalesced_but_terminal_event_is_immediate() {
        let db = migrated_database();
        let mut operation = fixture_operation("task:7", "root_scan", "running", 10);
        assert!(db
            .publish_notification_operation(&mut operation, PublishMode::Phase)
            .unwrap());
        operation.current_value = 20;
        operation.updated_at += 100;
        assert!(!db
            .publish_notification_operation(&mut operation, PublishMode::Progress)
            .unwrap());
        operation.status = "succeeded".to_string();
        operation.completed_at = Some(operation.updated_at + 1);
        assert!(db
            .publish_notification_operation(&mut operation, PublishMode::Terminal)
            .unwrap());
        let page = db.list_notification_outbox(0, 100).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page.last().unwrap().operation.status, "succeeded");
        assert_eq!(page.last().unwrap().operation.current_value, 20);
    }

    #[test]
    fn progress_emits_at_the_one_second_boundary() {
        let db = migrated_database();
        let mut operation = fixture_operation("task:interval", "root_scan", "running", 1_000);
        assert!(db
            .publish_notification_operation(&mut operation, PublishMode::Phase)
            .unwrap());
        operation.updated_at += DEFAULT_PROGRESS_EMIT_INTERVAL_MS;
        assert!(db
            .publish_notification_operation(&mut operation, PublishMode::Progress)
            .unwrap());
    }

    #[test]
    fn acknowledged_events_replay_safely_and_dirty_heads_recover() {
        let db = migrated_database();
        let mut operation = fixture_operation("task:9", "root_scan", "running", 1);
        assert!(db
            .publish_notification_operation(&mut operation, PublishMode::Phase)
            .unwrap());
        let first = db.list_notification_outbox(0, 100).unwrap();
        let replay = db.list_notification_outbox(0, 100).unwrap();
        assert_eq!(first[0].sequence, replay[0].sequence);
        assert_eq!(
            db.acknowledge_notification_outbox(first[0].sequence)
                .unwrap(),
            1
        );
        assert!(db.list_notification_outbox(0, 100).unwrap().is_empty());

        operation.current_value = 2;
        operation.updated_at += 100;
        assert!(!db
            .publish_notification_operation(&mut operation, PublishMode::Progress)
            .unwrap());
        assert_eq!(db.flush_dirty_notification_heads().unwrap(), 1);
        let recovered = db.list_notification_outbox(first[0].sequence, 100).unwrap();
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].sequence > first[0].sequence);
        assert_eq!(recovered[0].operation.current_value, 2);
    }

    #[test]
    fn dirty_flush_is_idempotent_and_clears_the_head() {
        let db = migrated_database();
        let mut operation = fixture_operation("task:flush", "root_scan", "running", 1_000);
        assert!(db
            .publish_notification_operation(&mut operation, PublishMode::Phase)
            .unwrap());
        operation.current_value = 20;
        operation.updated_at += 100;
        assert!(!db
            .publish_notification_operation(&mut operation, PublishMode::Progress)
            .unwrap());
        assert_eq!(head_revision_and_dirty(&db, "task:flush"), (2, 1));
        assert_eq!(db.list_notification_outbox(0, 100).unwrap().len(), 1);

        assert_eq!(db.flush_dirty_notification_heads().unwrap(), 1);
        assert_eq!(head_revision_and_dirty(&db, "task:flush"), (2, 0));
        let flushed = db.list_notification_outbox(0, 100).unwrap();
        assert_eq!(flushed.len(), 2);
        assert_eq!(flushed[1].operation.current_value, 20);

        assert_eq!(db.flush_dirty_notification_heads().unwrap(), 0);
        assert_eq!(db.list_notification_outbox(0, 100).unwrap(), flushed);
        assert_eq!(head_revision_and_dirty(&db, "task:flush"), (2, 0));
    }

    #[test]
    fn failed_dirty_flush_rolls_back_all_outbox_and_dirty_changes() {
        let db = migrated_database();
        for operation_id in ["task:flush-a", "task:flush-b"] {
            let mut operation = fixture_operation(operation_id, "root_scan", "running", 1_000);
            assert!(db
                .publish_notification_operation(&mut operation, PublishMode::Phase)
                .unwrap());
            operation.current_value = 20;
            operation.updated_at += 100;
            assert!(!db
                .publish_notification_operation(&mut operation, PublishMode::Progress)
                .unwrap());
        }
        assert_eq!(head_revision_and_dirty(&db, "task:flush-a"), (2, 1));
        assert_eq!(head_revision_and_dirty(&db, "task:flush-b"), (2, 1));
        let before = db.list_notification_outbox(0, 100).unwrap();

        db.connection
            .execute(
                "UPDATE notification_operation_heads SET payload_json = ?2 WHERE operation_id = ?1",
                params!["task:flush-b", "not-json"],
            )
            .expect("corrupt second dirty payload for rollback test");
        assert!(db.flush_dirty_notification_heads().is_err());

        assert_eq!(db.list_notification_outbox(0, 100).unwrap(), before);
        assert_eq!(head_revision_and_dirty(&db, "task:flush-a"), (2, 1));
        assert_eq!(head_revision_and_dirty(&db, "task:flush-b"), (2, 1));
    }

    #[test]
    fn failed_publish_rolls_back_head_and_outbox() {
        let db = migrated_database();
        let mut operation = fixture_operation("task:publish", "root_scan", "running", 1_000);
        assert!(db
            .publish_notification_operation(&mut operation, PublishMode::Phase)
            .unwrap());
        let before = db.list_notification_outbox(0, 100).unwrap();

        operation.status = "invalid-status".to_string();
        operation.updated_at += 100;
        assert!(db
            .publish_notification_operation(&mut operation, PublishMode::Progress)
            .is_err());

        assert_eq!(db.list_notification_outbox(0, 100).unwrap(), before);
        assert_eq!(head_revision_and_dirty(&db, "task:publish"), (1, 0));
    }

    #[test]
    fn thumbnail_task_is_rejected_but_aggregate_is_allowed() {
        let db = migrated_database();
        let mut thumbnail_task = fixture_operation("task:thumb", "thumbnail", "running", 10);
        assert!(db
            .publish_notification_operation(&mut thumbnail_task, PublishMode::Phase)
            .is_err());
        assert!(db.list_notification_outbox(0, 100).unwrap().is_empty());

        thumbnail_task.category = "thumbnail_cache".to_string();
        assert!(db
            .publish_notification_operation(&mut thumbnail_task, PublishMode::Phase)
            .unwrap());
    }

    #[test]
    fn recovery_fails_non_task_heads_and_leaves_tasks_for_task_recovery() {
        let db = migrated_database();
        let mut file_operation = fixture_operation("file:1", "delete", "running", 10);
        file_operation.category = "file_operation".to_string();
        assert!(db
            .publish_notification_operation(&mut file_operation, PublishMode::Phase)
            .unwrap());
        let mut task = fixture_operation("task:1", "root_scan", "running", 10);
        assert!(db
            .publish_notification_operation(&mut task, PublishMode::Phase)
            .unwrap());

        assert_eq!(db.recover_interrupted_notification_operations().unwrap(), 1);
        let events = db.list_notification_outbox(0, 100).unwrap();
        let recovered = events
            .iter()
            .find(|event| event.operation.operation_id == "file:1" && event.operation.revision == 2)
            .expect("recovery terminal event");
        assert_eq!(recovered.operation.status, "failed");
        assert_eq!(recovered.operation.origin, "recovery");
        assert_eq!(
            recovered.operation.error_code.as_deref(),
            Some("interrupted")
        );
        assert!(
            events
                .iter()
                .all(|event| event.operation.operation_id != "task:1"
                    || event.operation.revision == 1)
        );
    }

    #[test]
    fn generic_recovery_leaves_heads_with_pending_file_operation_journals_running() {
        let db = migrated_database();
        let mut operation = fixture_operation("file:pending", "move", "running", 1_000);
        operation.category = "file_operation".to_owned();
        db.publish_notification_operation(&mut operation, PublishMode::Phase)
            .expect("publish file-operation head");
        db.connection
            .execute(
                r#"INSERT INTO file_operations(
                     operation, source_path, target_path, status, created_at,
                     operation_group_id, entity_kind, entity_id, root_id,
                     target_folder_id, attempt_count, journal_phase, source_identity
                   ) VALUES ('move', 'old', 'new', 'pending', 1,
                             'file:pending', 'file', 7, 3, 9, 1,
                             'executing', 'windows:1:7')"#,
                [],
            )
            .expect("insert pending journal");

        assert_eq!(db.recover_interrupted_notification_operations().unwrap(), 0);
        let payload: String = db
            .connection
            .query_row(
                "SELECT payload_json FROM notification_operation_heads WHERE operation_id = 'file:pending'",
                [],
                |row| row.get(0),
            )
            .expect("read pending head");
        let operation: NotificationOperation =
            serde_json::from_str(&payload).expect("deserialize pending head");
        assert_eq!(operation.status, "running");
        assert_eq!(operation.completed_at, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wake_is_sent_after_commit_and_shared_by_reopened_database() {
        let path = temporary_database_path("wake");
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).expect("open file database");
        db.apply_migrations().expect("apply migrations");
        let reopened = db.reopen().expect("reopen").expect("file database");
        let wake = db.notification_outbox_wake();
        let waiter = wake.notified();
        tokio::pin!(waiter);
        waiter.as_mut().enable();
        let mut operation = fixture_operation("task:wake", "root_scan", "running", 10);
        assert!(reopened
            .publish_notification_operation(&mut operation, PublishMode::Phase)
            .unwrap());
        tokio::time::timeout(Duration::from_secs(1), waiter.as_mut())
            .await
            .expect("wake after commit");
        drop(reopened);
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rollback_does_not_wake_but_successful_commit_does() {
        let db = migrated_database();
        let wake = db.notification_outbox_wake();

        let failed_notification = wake.notified();
        tokio::pin!(failed_notification);
        failed_notification.as_mut().enable();
        let mut invalid = fixture_operation("task:rollback", "root_scan", "running", 1_000);
        invalid.status = "invalid-status".to_string();
        assert!(db
            .publish_notification_operation(&mut invalid, PublishMode::Phase)
            .is_err());
        assert!(db.list_notification_outbox(0, 100).unwrap().is_empty());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), failed_notification.as_mut())
                .await
                .is_err()
        );

        let committed_notification = wake.notified();
        tokio::pin!(committed_notification);
        committed_notification.as_mut().enable();
        let mut valid = fixture_operation("task:commit", "root_scan", "running", 1_000);
        assert!(db
            .publish_notification_operation(&mut valid, PublishMode::Phase)
            .unwrap());
        tokio::time::timeout(Duration::from_secs(1), committed_notification.as_mut())
            .await
            .expect("successful commit should wake");
    }

    #[test]
    fn concurrent_phase_publishes_have_unique_contiguous_revisions() {
        const PUBLISHERS: usize = 8;
        let path = temporary_database_path("concurrent");
        let database = Database::open(&path).expect("open file database");
        database.apply_migrations().expect("apply migrations");
        drop(database);

        let barrier = Arc::new(Barrier::new(PUBLISHERS));
        let mut handles = Vec::with_capacity(PUBLISHERS);
        for index in 0..PUBLISHERS {
            let barrier = Arc::clone(&barrier);
            let path = path.clone();
            handles.push(thread::spawn(move || -> Result<(), String> {
                let database = Database::open(&path).map_err(|error| error.to_string())?;
                barrier.wait();
                let mut operation = fixture_operation(
                    "task:concurrent",
                    "root_scan",
                    "running",
                    1_000 + index as i64,
                );
                database
                    .publish_notification_operation(&mut operation, PublishMode::Phase)
                    .map_err(|error| error.to_string())?;
                Ok(())
            }));
        }
        for handle in handles {
            handle
                .join()
                .expect("publisher thread should not panic")
                .expect("publish should succeed");
        }

        let database = Database::open(&path).expect("reopen file database");
        let events = database.list_notification_outbox(0, 100).unwrap();
        assert_eq!(events.len(), PUBLISHERS);
        let mut revisions: Vec<_> = events
            .iter()
            .map(|event| event.operation.revision)
            .collect();
        revisions.sort_unstable();
        assert_eq!(revisions, (1..=PUBLISHERS as i64).collect::<Vec<_>>());
        let mut sequences: Vec<_> = events.iter().map(|event| event.sequence).collect();
        sequences.sort_unstable();
        sequences.dedup();
        assert_eq!(sequences.len(), PUBLISHERS);
        drop(database);
        let _ = std::fs::remove_file(path);
    }
}
