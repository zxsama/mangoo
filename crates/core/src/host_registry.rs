use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

const HOST_MIGRATION_0001: &str = include_str!("../host_migrations/0001_host_registry.sql");

#[derive(Debug, Clone)]
pub struct LibraryDescriptor {
    pub library_uuid: String,
    pub root_uuid: String,
    pub name: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub local_data_dir: PathBuf,
    pub managed_root: PathBuf,
    pub lifecycle_state: String,
    pub binding_state: String,
    pub availability: String,
    pub marker_identity: String,
}

#[derive(Debug, Clone)]
pub struct HostSnapshot {
    pub active_library_uuid: Option<String>,
    pub context_revision: u64,
    pub libraries: Vec<LibraryDescriptor>,
}

#[derive(Debug, Clone)]
pub struct CreationIntent {
    pub txn_uuid: String,
    pub descriptor: LibraryDescriptor,
    pub requested_root: PathBuf,
    pub staging_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CreationRecovery {
    pub txn_uuid: String,
    pub library_uuid: String,
    pub root_uuid: String,
    pub name: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub resolved_root: PathBuf,
    pub staging_dir: PathBuf,
    pub created_root: bool,
    pub root_identity: Option<String>,
    pub staging_identity: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RemovalRecovery {
    pub txn_uuid: String,
    pub library_uuid: String,
    pub marker_identity: String,
    pub managed_root: PathBuf,
    pub local_data_dir: PathBuf,
    pub root_quarantine: Option<PathBuf>,
    pub local_quarantine: Option<PathBuf>,
    pub root_quarantine_identity: Option<String>,
    pub local_quarantine_identity: Option<String>,
    pub phase: String,
}

pub struct HostRegistry {
    connection: Mutex<Connection>,
}

impl HostRegistry {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.execute_batch(HOST_MIGRATION_0001)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn snapshot(&self) -> anyhow::Result<HostSnapshot> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let (active_library_uuid, context_revision) = connection.query_row(
            "SELECT active_library_uuid, context_revision FROM host_meta WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)),
        )?;
        let mut statement = connection.prepare(
            "SELECT library_uuid, root_uuid, name, icon, color, local_data_dir, managed_root, lifecycle_state, binding_state, availability, marker_identity FROM libraries ORDER BY COALESCE(last_opened_at, created_at) DESC, created_at DESC",
        )?;
        let libraries = statement
            .query_map([], descriptor_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(HostSnapshot {
            active_library_uuid,
            context_revision,
            libraries,
        })
    }

    pub fn library(&self, library_uuid: &str) -> anyhow::Result<Option<LibraryDescriptor>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        connection
            .query_row(
                "SELECT library_uuid, root_uuid, name, icon, color, local_data_dir, managed_root, lifecycle_state, binding_state, availability, marker_identity FROM libraries WHERE library_uuid = ?1",
                [library_uuid],
                descriptor_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn update_library_metadata(
        &self,
        library_uuid: &str,
        name: Option<&str>,
        icon: Option<Option<&str>>,
        color: Option<Option<&str>>,
    ) -> anyhow::Result<Option<LibraryDescriptor>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let changed = connection.execute(
            "UPDATE libraries SET name=COALESCE(?2,name), icon=CASE WHEN ?3 THEN ?4 ELSE icon END, color=CASE WHEN ?5 THEN ?6 ELSE color END, updated_at=?7 WHERE library_uuid=?1",
            params![library_uuid, name, icon.is_some(), icon.flatten(), color.is_some(), color.flatten(), unix_timestamp_millis()],
        )?;
        drop(connection);
        if changed == 0 {
            Ok(None)
        } else {
            self.library(library_uuid)
        }
    }

    pub fn begin_creation(&self, intent: &CreationIntent) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let now = unix_timestamp_millis();
        transaction.execute(
            "INSERT INTO library_creation_journal(txn_uuid, library_uuid, root_uuid, name, icon, color, requested_root, resolved_root, local_staging_dir, phase, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'intent', ?10, ?10)",
            params![
                intent.txn_uuid,
                intent.descriptor.library_uuid,
                intent.descriptor.root_uuid,
                intent.descriptor.name,
                intent.descriptor.icon,
                intent.descriptor.color,
                intent.requested_root.to_string_lossy(),
                intent.descriptor.managed_root.to_string_lossy(),
                intent.staging_dir.to_string_lossy(),
                now,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn pending_creation_recovery(&self) -> anyhow::Result<Vec<CreationRecovery>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare("SELECT txn_uuid, library_uuid, root_uuid, name, icon, color, resolved_root, local_staging_dir, phase, created_root, created_marker, root_identity, staging_identity FROM library_creation_journal WHERE phase NOT IN ('created','rolled-back') ORDER BY created_at")?;
        let rows = statement
            .query_map([], |row| {
                Ok(CreationRecovery {
                    txn_uuid: row.get(0)?,
                    library_uuid: row.get(1)?,
                    root_uuid: row.get(2)?,
                    name: row.get(3)?,
                    icon: row.get(4)?,
                    color: row.get(5)?,
                    resolved_root: PathBuf::from(row.get::<_, String>(6)?),
                    staging_dir: PathBuf::from(row.get::<_, String>(7)?),
                    created_root: row.get::<_, i64>(9)? != 0,
                    root_identity: row.get(11)?,
                    staging_identity: row.get(12)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn pending_removal_recovery(&self) -> anyhow::Result<Vec<RemovalRecovery>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare("SELECT txn_uuid, library_uuid, marker_identity, managed_root, local_data_dir, root_quarantine, local_quarantine, root_quarantine_identity, local_quarantine_identity, phase FROM library_removal_journal WHERE phase NOT IN ('complete','rolled-back') ORDER BY created_at")?;
        let rows = statement
            .query_map([], |row| {
                Ok(RemovalRecovery {
                    txn_uuid: row.get(0)?,
                    library_uuid: row.get(1)?,
                    marker_identity: row.get(2)?,
                    managed_root: PathBuf::from(row.get::<_, String>(3)?),
                    local_data_dir: PathBuf::from(row.get::<_, String>(4)?),
                    root_quarantine: row.get::<_, Option<String>>(5)?.map(PathBuf::from),
                    local_quarantine: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
                    root_quarantine_identity: row.get(7)?,
                    local_quarantine_identity: row.get(8)?,
                    phase: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn mark_creation_rolled_back(&self, txn_uuid: &str) -> anyhow::Result<()> {
        self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute("UPDATE library_creation_journal SET phase='rolled-back', updated_at=?2 WHERE txn_uuid=?1", params![txn_uuid, unix_timestamp_millis()])?;
        Ok(())
    }

    pub fn mark_removal_complete(&self, txn_uuid: &str) -> anyhow::Result<()> {
        self.connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .execute(
            "UPDATE library_removal_journal SET phase='complete', updated_at=?2 WHERE txn_uuid=?1",
            params![txn_uuid, unix_timestamp_millis()],
        )?;
        Ok(())
    }

    pub fn rollback_removal(&self, txn_uuid: &str, library_uuid: &str) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE libraries SET lifecycle_state='ready', updated_at=?2 WHERE library_uuid=?1",
            params![library_uuid, unix_timestamp_millis()],
        )?;
        transaction.execute(
            "UPDATE library_removal_journal SET phase='rolled-back', updated_at=?2 WHERE txn_uuid=?1",
            params![txn_uuid, unix_timestamp_millis()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn update_creation_phase(
        &self,
        txn_uuid: &str,
        phase: &str,
        created_root: bool,
        created_marker: bool,
        marker_identity: Option<&str>,
        error_code: Option<&str>,
    ) -> anyhow::Result<()> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        connection.execute(
            "UPDATE library_creation_journal SET phase=?2, created_root=?3, created_marker=?4, marker_identity=COALESCE(?5, marker_identity), error_code=?6, updated_at=?7 WHERE txn_uuid=?1",
            params![txn_uuid, phase, created_root as i64, created_marker as i64, marker_identity, error_code, unix_timestamp_millis()],
        )?;
        Ok(())
    }

    pub fn update_creation_identities(
        &self,
        txn_uuid: &str,
        root_identity: Option<&str>,
        staging_identity: Option<&str>,
    ) -> anyhow::Result<()> {
        self.connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .execute(
                "UPDATE library_creation_journal SET root_identity=COALESCE(?2,root_identity), staging_identity=COALESCE(?3,staging_identity), updated_at=?4 WHERE txn_uuid=?1",
                params![txn_uuid, root_identity, staging_identity, unix_timestamp_millis()],
            )?;
        Ok(())
    }

    pub fn commit_creation(
        &self,
        txn_uuid: &str,
        descriptor: &LibraryDescriptor,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let now = unix_timestamp_millis();
        transaction.execute(
            "INSERT INTO libraries(library_uuid, root_uuid, name, icon, color, local_data_dir, managed_root, lifecycle_state, binding_state, availability, marker_version, marker_identity, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'ready', 'local-only', 'available', 1, ?8, ?9, ?9) ON CONFLICT(library_uuid) DO UPDATE SET name=excluded.name, icon=excluded.icon, color=excluded.color, local_data_dir=excluded.local_data_dir, managed_root=excluded.managed_root, lifecycle_state='ready', binding_state='local-only', availability='available', marker_identity=excluded.marker_identity, updated_at=excluded.updated_at",
            params![descriptor.library_uuid, descriptor.root_uuid, descriptor.name, descriptor.icon, descriptor.color, descriptor.local_data_dir.to_string_lossy(), descriptor.managed_root.to_string_lossy(), descriptor.marker_identity, now],
        )?;
        transaction.execute(
            "UPDATE library_creation_journal SET phase='created', updated_at=?2 WHERE txn_uuid=?1",
            params![txn_uuid, now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn begin_activation(
        &self,
        txn_uuid: &str,
        library_uuid: Option<&str>,
    ) -> anyhow::Result<u64> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let (from_library_uuid, current): (Option<String>, i64) = transaction.query_row(
            "SELECT active_library_uuid, context_revision FROM host_meta WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let next = current
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("context revision exhausted"))?;
        let now = unix_timestamp_millis();
        transaction.execute("INSERT INTO library_activation_journal(txn_uuid, from_library_uuid, to_library_uuid, target_revision, phase, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 'prepared', ?5, ?5)", params![txn_uuid, from_library_uuid, library_uuid, next, now])?;
        transaction.execute("UPDATE host_meta SET activation_library_uuid=?1, activation_revision=?2 WHERE singleton=1", params![library_uuid, next])?;
        transaction.commit()?;
        Ok(next as u64)
    }

    pub fn commit_activation(
        &self,
        txn_uuid: &str,
        library_uuid: Option<&str>,
        revision: u64,
    ) -> anyhow::Result<u64> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let prepared: Option<(Option<String>, i64)> = transaction.query_row(
            "SELECT to_library_uuid, target_revision FROM library_activation_journal WHERE txn_uuid=?1 AND phase='prepared'",
            [txn_uuid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let expected = prepared.ok_or_else(|| anyhow::anyhow!("activation-not-prepared"))?;
        if expected.0.as_deref() != library_uuid || expected.1 != revision as i64 {
            anyhow::bail!("activation-journal-mismatch");
        }
        transaction.execute("UPDATE host_meta SET active_library_uuid=?1, context_revision=?2, activation_library_uuid=NULL, activation_revision=NULL WHERE singleton=1", params![library_uuid, revision as i64])?;
        transaction.execute("UPDATE library_activation_journal SET phase='committed', updated_at=?2 WHERE txn_uuid=?1", params![txn_uuid, unix_timestamp_millis()])?;
        if let Some(library_uuid) = library_uuid {
            transaction.execute(
                "UPDATE libraries SET last_opened_at=?2, updated_at=?2 WHERE library_uuid=?1",
                params![library_uuid, unix_timestamp_millis()],
            )?;
        }
        transaction.commit()?;
        Ok(revision)
    }

    pub fn rollback_activation(&self, txn_uuid: &str, error_code: &str) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute("UPDATE host_meta SET activation_library_uuid=NULL, activation_revision=NULL WHERE singleton=1", [])?;
        transaction.execute("UPDATE library_activation_journal SET phase='rolled-back', error_code=?2, updated_at=?3 WHERE txn_uuid=?1", params![txn_uuid, error_code, unix_timestamp_millis()])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn recover_activation_journal(&self) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute("UPDATE library_activation_journal SET phase='rolled-back', error_code='startup-recovery', updated_at=?1 WHERE phase='prepared'", [unix_timestamp_millis()])?;
        transaction.execute("UPDATE host_meta SET activation_library_uuid=NULL, activation_revision=NULL WHERE singleton=1", [])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn begin_removal(
        &self,
        descriptor: &LibraryDescriptor,
        txn_uuid: &str,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let now = unix_timestamp_millis();
        transaction.execute(
            "UPDATE libraries SET lifecycle_state='removing', updated_at=?2 WHERE library_uuid=?1 AND lifecycle_state='ready'",
            params![descriptor.library_uuid, now],
        )?;
        transaction.execute(
            "INSERT INTO library_removal_journal(txn_uuid, library_uuid, managed_root, local_data_dir, marker_identity, phase, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 'requested', ?6, ?6)",
            params![txn_uuid, descriptor.library_uuid, descriptor.managed_root.to_string_lossy(), descriptor.local_data_dir.to_string_lossy(), descriptor.marker_identity, now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn update_removal_quarantine(
        &self,
        txn_uuid: &str,
        root: &Path,
        local: &Path,
        root_identity: &str,
        local_identity: &str,
    ) -> anyhow::Result<()> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        connection.execute(
            "UPDATE library_removal_journal SET root_quarantine=?2, local_quarantine=?3, root_quarantine_identity=?4, local_quarantine_identity=?5, phase='quarantined', updated_at=?6 WHERE txn_uuid=?1",
            params![txn_uuid, root.to_string_lossy(), local.to_string_lossy(), root_identity, local_identity, unix_timestamp_millis()],
        )?;
        Ok(())
    }

    pub fn update_removal_partial(
        &self,
        txn_uuid: &str,
        phase: &str,
        root: Option<&Path>,
        local: Option<&Path>,
        root_identity: Option<&str>,
        local_identity: Option<&str>,
    ) -> anyhow::Result<()> {
        self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "UPDATE library_removal_journal SET root_quarantine=COALESCE(?3,root_quarantine), local_quarantine=COALESCE(?4,local_quarantine), root_quarantine_identity=COALESCE(?5,root_quarantine_identity), local_quarantine_identity=COALESCE(?6,local_quarantine_identity), phase=?2, updated_at=?7 WHERE txn_uuid=?1",
            params![txn_uuid, phase, root.map(|p| p.to_string_lossy()), local.map(|p| p.to_string_lossy()), root_identity, local_identity, unix_timestamp_millis()],
        )?;
        Ok(())
    }

    pub fn commit_removal(&self, txn_uuid: &str, library_uuid: &str) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM libraries WHERE library_uuid=?1",
            [library_uuid],
        )?;
        transaction.execute(
            "UPDATE library_removal_journal SET phase='committed', updated_at=?2 WHERE txn_uuid=?1",
            params![txn_uuid, unix_timestamp_millis()],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn descriptor_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LibraryDescriptor> {
    Ok(LibraryDescriptor {
        library_uuid: row.get(0)?,
        root_uuid: row.get(1)?,
        name: row.get(2)?,
        icon: row.get(3)?,
        color: row.get(4)?,
        local_data_dir: PathBuf::from(row.get::<_, String>(5)?),
        managed_root: PathBuf::from(row.get::<_, String>(6)?),
        lifecycle_state: row.get(7)?,
        binding_state: row.get(8)?,
        availability: row.get(9)?,
        marker_identity: row.get(10)?,
    })
}

fn unix_timestamp_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_registry_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "megle-host-registry-{}.sqlite",
            uuid::Uuid::now_v7()
        ))
    }

    #[test]
    fn empty_registry_has_no_implicit_default_library() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).expect("open host registry");
        let snapshot = registry.snapshot().expect("read host snapshot");
        assert_eq!(snapshot.active_library_uuid, None);
        assert_eq!(snapshot.context_revision, 0);
        assert!(snapshot.libraries.is_empty());
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn activation_revision_is_durable_and_monotonic() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).expect("open host registry");
        let first = registry
            .begin_activation("first", None)
            .expect("begin first");
        assert_eq!(
            registry
                .commit_activation("first", None, first)
                .expect("first revision"),
            1
        );
        let second = registry
            .begin_activation("second", None)
            .expect("begin second");
        assert_eq!(
            registry
                .commit_activation("second", None, second)
                .expect("second revision"),
            2
        );
        drop(registry);
        let reopened = HostRegistry::open(&path).expect("reopen host registry");
        assert_eq!(
            reopened.snapshot().expect("read snapshot").context_revision,
            2
        );
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn startup_recovery_rolls_back_only_uncommitted_activation_intent() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        assert_eq!(
            registry
                .begin_activation("crashed", Some("target"))
                .unwrap(),
            1
        );
        registry.recover_activation_journal().unwrap();
        let snapshot = registry.snapshot().unwrap();
        assert_eq!(snapshot.active_library_uuid, None);
        assert_eq!(snapshot.context_revision, 0);

        let revision = registry.begin_activation("committed", None).unwrap();
        registry
            .commit_activation("committed", None, revision)
            .unwrap();
        registry.recover_activation_journal().unwrap();
        assert_eq!(registry.snapshot().unwrap().context_revision, 1);
        drop(registry);
        let _ = std::fs::remove_file(path);
    }
}
