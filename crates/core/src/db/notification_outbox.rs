use anyhow::{anyhow, Context};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::Database;

const MAX_OUTBOX_PAGE: i64 = 200;
const PROGRESS_EMIT_INTERVAL_MS: i64 = 1_000;

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
    /// Publish one logical operation using its own immediate transaction.
    /// The wake is sent only after the transaction has committed successfully.
    pub fn publish_notification_operation(
        &self,
        operation: &mut NotificationOperation,
        mode: PublishMode,
    ) -> anyhow::Result<bool> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let appended =
            self.publish_notification_operation_in_transaction(&transaction, operation, mode)?;
        transaction.commit()?;
        if appended {
            self.notification_outbox_wake.notify_one();
        }
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

        let previous: Option<(i64, Option<i64>)> = transaction
            .query_row(
                "SELECT revision, last_outbox_at FROM notification_operation_heads WHERE operation_id = ?1",
                [&operation.operation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let next_revision = previous
            .as_ref()
            .map(|(revision, _)| revision.saturating_add(1))
            .unwrap_or(1);
        let previous_last_outbox_at = previous.and_then(|(_, last)| last);
        let append = should_emit(mode, previous_last_outbox_at, operation.updated_at);
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
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let dirty_heads = collect_dirty_heads(&transaction)?;
        let mut appended = 0usize;
        for (operation_id, revision, payload_json, updated_at) in dirty_heads {
            let operation = deserialize_operation(&payload_json, Some(&operation_id))?;
            if operation.revision != revision {
                return Err(anyhow!(
                    "notification head revision mismatch for {}",
                    operation_id
                ));
            }
            if append_event_if_missing_in_transaction(&transaction, &operation, updated_at)? {
                appended += 1;
            }
            transaction.execute(
                "UPDATE notification_operation_heads SET dirty = 0, last_outbox_at = ?2 WHERE operation_id = ?1 AND revision = ?3",
                params![operation_id, updated_at, revision],
            )?;
        }
        transaction.commit()?;
        if appended > 0 {
            self.notification_outbox_wake.notify_one();
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
            self.notification_outbox_wake.notify_one();
        }
        Ok(recovered)
    }
}

fn should_emit(mode: PublishMode, previous_last_outbox_at: Option<i64>, now_ms: i64) -> bool {
    match mode {
        PublishMode::Phase | PublishMode::Terminal => true,
        PublishMode::Progress => previous_last_outbox_at
            .map(|last| now_ms.saturating_sub(last) >= PROGRESS_EMIT_INTERVAL_MS)
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
) -> anyhow::Result<Vec<(String, i64, String, i64)>> {
    let mut statement = transaction.prepare(
        "SELECT operation_id, revision, payload_json, updated_at FROM notification_operation_heads WHERE dirty = 1 ORDER BY updated_at ASC, operation_id ASC",
    )?;
    let rows = statement
        .query_map([], |row| {
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
        "SELECT operation_id, payload_json FROM notification_operation_heads WHERE status IN ('pending', 'running') ORDER BY updated_at ASC, operation_id ASC",
    )?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn migrated_database() -> Database {
        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        database
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
        operation.updated_at += PROGRESS_EMIT_INTERVAL_MS;
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

    #[tokio::test(flavor = "current_thread")]
    async fn wake_is_sent_after_commit_and_shared_by_reopened_database() {
        let path = std::env::temp_dir().join(format!(
            "megle-notification-wake-{}-{}.sqlite",
            std::process::id(),
            super::super::unix_timestamp_millis()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).expect("open file database");
        db.apply_migrations().expect("apply migrations");
        let reopened = db.reopen().expect("reopen").expect("file database");
        let wake = db.notification_outbox_wake();
        let waiter = tokio::spawn(async move {
            wake.notified().await;
        });
        let mut operation = fixture_operation("task:wake", "root_scan", "running", 10);
        assert!(reopened
            .publish_notification_operation(&mut operation, PublishMode::Phase)
            .unwrap());
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("wake after commit")
            .expect("waiter task");
        drop(reopened);
        drop(db);
        let _ = std::fs::remove_file(path);
    }
}
