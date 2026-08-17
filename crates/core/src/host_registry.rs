use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

const HOST_MIGRATION_0001: &str = include_str!("../host_migrations/0001_host_registry.sql");
const HOST_MIGRATION_0002: &str =
    include_str!("../host_migrations/0002_sync_protocol_foundation.sql");

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
    /// Applies a verified Host-metadata segment after the currently durable
    /// frontier. The transaction owns ledger, projection and frontier so a
    /// crash can never expose only part of a Host update.
    pub fn apply_host_segment(
        &self,
        segment: &crate::sync::segment::SealedSegment,
    ) -> anyhow::Result<usize> {
        crate::sync::segment::verify_segment(segment, segment.header.predecessor_hash)?;
        if segment.header.stream_kind != crate::sync::types::StreamKind::HostMetadata {
            anyhow::bail!("catalog segment cannot be applied to Host Registry");
        }
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let actor = &segment.header.actor;
        let mut frontier: i64 = transaction.query_row(
            "SELECT applied_through_seq FROM host_sync_actor_frontiers WHERE device_uuid=?1 AND stream_uuid=?2 AND actor_epoch=?3",
            params![actor.device_uuid.0.as_bytes().as_slice(),actor.stream_uuid.0.as_bytes().as_slice(),actor.actor_epoch.0.as_bytes().as_slice()],
            |row| row.get(0),
        ).optional()?.unwrap_or(0);
        let mut applied = 0;
        for event in &segment.events {
            let existing: Option<(Vec<u8>,Vec<u8>)> = transaction.query_row(
                "SELECT event_uuid,integrity_hash FROM host_sync_events WHERE device_uuid=?1 AND stream_uuid=?2 AND actor_epoch=?3 AND actor_seq=?4",
                params![actor.device_uuid.0.as_bytes().as_slice(),actor.stream_uuid.0.as_bytes().as_slice(),actor.actor_epoch.0.as_bytes().as_slice(),event.actor_seq as i64],
                |row| Ok((row.get(0)?,row.get(1)?)),
            ).optional()?;
            if let Some((id, hash)) = existing {
                if id == event.event_uuid.0.as_bytes() && hash == event.integrity_hash.as_slice() {
                    continue;
                }
                transaction.execute("INSERT INTO host_sync_actor_frontiers(device_uuid,stream_uuid,actor_epoch,applied_through_seq,expected_next_seq,state,updated_at) VALUES (?1,?2,?3,?4,?5,'forked',?6) ON CONFLICT(device_uuid,stream_uuid,actor_epoch) DO UPDATE SET state='forked',updated_at=excluded.updated_at", params![actor.device_uuid.0.as_bytes().as_slice(),actor.stream_uuid.0.as_bytes().as_slice(),actor.actor_epoch.0.as_bytes().as_slice(),frontier,frontier+1,unix_timestamp_millis()])?;
                transaction.commit()?;
                anyhow::bail!("Host actor fork");
            }
            if event.actor_seq as i64 != frontier + 1 {
                transaction.execute("INSERT INTO host_sync_actor_frontiers(device_uuid,stream_uuid,actor_epoch,applied_through_seq,expected_next_seq,state,updated_at) VALUES (?1,?2,?3,?4,?5,'gap',?6) ON CONFLICT(device_uuid,stream_uuid,actor_epoch) DO UPDATE SET state='gap',updated_at=excluded.updated_at", params![actor.device_uuid.0.as_bytes().as_slice(),actor.stream_uuid.0.as_bytes().as_slice(),actor.actor_epoch.0.as_bytes().as_slice(),frontier,frontier+1,unix_timestamp_millis()])?;
                transaction.commit()?;
                anyhow::bail!("Host actor gap");
            }
            crate::sync::codec::verify_event(event)?;
            let library_uuid = event.library_uuid.to_string();
            let (field, value) = match &event.operation {
                crate::sync::types::Operation::SetField { field, value }
                    if field == "name" || field == "icon" =>
                {
                    (field.as_str(), value)
                }
                _ => anyhow::bail!("unsupported Host metadata operation"),
            };
            let operation = match field {
                "name" => "library-name-set",
                "icon" => "library-icon-set",
                _ => unreachable!(),
            };
            transaction.execute("INSERT INTO host_sync_events(event_uuid,library_uuid,stream_uuid,device_uuid,actor_epoch,binding_epoch,actor_seq,hlc_wall_ms,hlc_logical,wire_version,envelope_version,reducer_version,operation,apply_state,integrity_hash,created_at,applied_at,payload) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'applied',?14,?15,?15,?16)", params![event.event_uuid.0.as_bytes().as_slice(),library_uuid,actor.stream_uuid.0.as_bytes().as_slice(),actor.device_uuid.0.as_bytes().as_slice(),actor.actor_epoch.0.as_bytes().as_slice(),event.binding_epoch.0.as_bytes().as_slice(),event.actor_seq as i64,event.hlc.wall_ms as i64,event.hlc.logical as i64,event.wire_version,event.envelope_version,event.reducer_version,operation,event.integrity_hash.as_slice(),unix_timestamp_millis(),crate::sync::codec::encode_event(event)?])?;
            materialize_host_field_version(&transaction, event, field, value)?;
            merge_received_host_hlc(&transaction, event.hlc)?;
            frontier = event.actor_seq as i64;
            applied += 1;
        }
        transaction.execute("INSERT INTO host_sync_actor_frontiers(device_uuid,stream_uuid,actor_epoch,applied_through_seq,expected_next_seq,state,updated_at) VALUES (?1,?2,?3,?4,?5,'ready',?6) ON CONFLICT(device_uuid,stream_uuid,actor_epoch) DO UPDATE SET applied_through_seq=excluded.applied_through_seq,expected_next_seq=excluded.expected_next_seq,state='ready',updated_at=excluded.updated_at", params![actor.device_uuid.0.as_bytes().as_slice(),actor.stream_uuid.0.as_bytes().as_slice(),actor.actor_epoch.0.as_bytes().as_slice(),frontier,frontier+1,unix_timestamp_millis()])?;
        transaction.commit()?;
        Ok(applied)
    }

    pub fn host_checkpoint_snapshot(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<(
        crate::sync::checkpoint::StreamVector,
        crate::sync::checkpoint::HostMetadataSnapshot,
    )> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let descriptor = transaction.query_row(
            "SELECT l.library_uuid,l.root_uuid,l.name,l.icon,l.color,l.local_data_dir,l.managed_root,l.lifecycle_state,CASE WHEN b.state IN ('preparing','seeding') THEN 'synchronizing' WHEN b.state IN ('active','paused') THEN 'synchronized' ELSE l.binding_state END,l.availability,l.marker_identity FROM libraries l LEFT JOIN library_sync_bindings b ON b.library_uuid=l.library_uuid WHERE l.library_uuid=?1",
            [library_uuid],
            descriptor_from_row,
        ).optional()?.ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        let mut vector = BTreeMap::new();
        {
            let mut statement=transaction.prepare("SELECT e.device_uuid,e.stream_uuid,e.actor_epoch,MAX(e.actor_seq) FROM host_sync_events e WHERE e.library_uuid=?1 GROUP BY e.device_uuid,e.stream_uuid,e.actor_epoch ORDER BY e.device_uuid,e.stream_uuid,e.actor_epoch")?;
            for row in statement.query_map([library_uuid], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })? {
                let (device, stream, epoch, seq) = row?;
                vector.insert(
                    crate::sync::types::ActorId {
                        device_uuid: crate::sync::types::DeviceUuid(uuid::Uuid::from_slice(
                            &device,
                        )?),
                        stream_uuid: crate::sync::types::StreamUuid(uuid::Uuid::from_slice(
                            &stream,
                        )?),
                        actor_epoch: crate::sync::types::ActorEpochUuid(uuid::Uuid::from_slice(
                            &epoch,
                        )?),
                    },
                    seq as u64,
                );
            }
        }
        let events = {
            let mut statement = transaction.prepare("SELECT payload FROM host_sync_events WHERE library_uuid=?1 ORDER BY device_uuid,stream_uuid,actor_epoch,actor_seq")?;
            let events = statement
                .query_map([library_uuid], |r| r.get::<_, Vec<u8>>(0))?
                .map(|row| Ok(crate::sync::codec::decode_event(&row?)?))
                .collect::<anyhow::Result<Vec<_>>>()?;
            events
        };
        transaction.commit()?;
        Ok((
            vector,
            crate::sync::checkpoint::HostMetadataSnapshot {
                library_uuid: descriptor.library_uuid,
                name: descriptor.name,
                icon: descriptor.icon,
                binding_state: descriptor.binding_state,
                events,
            },
        ))
    }

    pub fn restore_host_checkpoint_metadata(
        &self,
        metadata: &crate::sync::checkpoint::HostMetadataSnapshot,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE libraries SET
                name=CASE WHEN EXISTS(SELECT 1 FROM host_sync_field_versions WHERE library_uuid=?1 AND field='name' AND is_winner=1) THEN name ELSE ?2 END,
                icon=CASE WHEN EXISTS(SELECT 1 FROM host_sync_field_versions WHERE library_uuid=?1 AND field='icon' AND is_winner=1) THEN icon ELSE ?3 END,
                updated_at=?4 WHERE library_uuid=?1",
            params![
                metadata.library_uuid,
                metadata.name,
                metadata.icon,
                unix_timestamp_millis()
            ],
        )?;
        if changed != 1 {
            anyhow::bail!("checkpoint Host metadata references an unknown Library");
        }
        for event in &metadata.events {
            crate::sync::codec::verify_event(event)?;
            let crate::sync::types::Operation::SetField { field, value } = &event.operation else {
                anyhow::bail!("checkpoint contains unsupported Host operation");
            };
            if !matches!(field.as_str(), "name" | "icon") {
                anyhow::bail!("checkpoint contains unsupported Host field");
            }
            let operation = match field.as_str() {
                "name" => "library-name-set",
                "icon" => "library-icon-set",
                _ => unreachable!(),
            };
            transaction.execute("INSERT OR IGNORE INTO host_sync_events(event_uuid,library_uuid,stream_uuid,device_uuid,actor_epoch,binding_epoch,actor_seq,hlc_wall_ms,hlc_logical,wire_version,envelope_version,reducer_version,operation,apply_state,integrity_hash,created_at,applied_at,payload) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'applied',?14,?15,?15,?16)", params![event.event_uuid.0.as_bytes().as_slice(),metadata.library_uuid,event.actor.stream_uuid.0.as_bytes().as_slice(),event.actor.device_uuid.0.as_bytes().as_slice(),event.actor.actor_epoch.0.as_bytes().as_slice(),event.binding_epoch.0.as_bytes().as_slice(),event.actor_seq as i64,event.hlc.wall_ms as i64,event.hlc.logical as i64,event.wire_version,event.envelope_version,event.reducer_version,operation,event.integrity_hash.as_slice(),unix_timestamp_millis(),crate::sync::codec::encode_event(event)?])?;
            materialize_host_field_version(&transaction, event, field, value)?;
            transaction.execute("INSERT INTO host_sync_actor_frontiers(device_uuid,stream_uuid,actor_epoch,applied_through_seq,expected_next_seq,state,updated_at) VALUES (?1,?2,?3,?4,?5,'ready',?6) ON CONFLICT(device_uuid,stream_uuid,actor_epoch) DO UPDATE SET applied_through_seq=MAX(applied_through_seq,excluded.applied_through_seq),expected_next_seq=MAX(expected_next_seq,excluded.expected_next_seq),updated_at=excluded.updated_at",params![event.actor.device_uuid.0.as_bytes().as_slice(),event.actor.stream_uuid.0.as_bytes().as_slice(),event.actor.actor_epoch.0.as_bytes().as_slice(),event.actor_seq as i64,event.actor_seq as i64+1,unix_timestamp_millis()])?;
            merge_received_host_hlc(&transaction, event.hlc)?;
        }
        transaction.commit()?;
        Ok(())
    }
    pub fn sync_bound_libraries(&self) -> anyhow::Result<Vec<LibraryDescriptor>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement=connection.prepare("SELECT l.library_uuid,l.root_uuid,l.name,l.icon,l.color,l.local_data_dir,l.managed_root,l.lifecycle_state,CASE WHEN b.state IN ('preparing','seeding') THEN 'synchronizing' ELSE 'synchronized' END,l.availability,l.marker_identity FROM libraries l JOIN library_sync_bindings b ON b.library_uuid=l.library_uuid WHERE b.state IN ('preparing','seeding','active','paused') ORDER BY l.library_uuid")?;
        let libraries = statement
            .query_map([], descriptor_from_row)?
            .collect::<Result<_, _>>()?;
        Ok(libraries)
    }
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.execute_batch(HOST_MIGRATION_0001)?;
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 2 {
            connection.execute_batch(HOST_MIGRATION_0002)?;
        }
        connection.execute(
            "INSERT OR IGNORE INTO host_sync_local_state(singleton,device_uuid,stream_uuid,next_actor_seq,hlc_wall_ms,hlc_logical,capture_enabled,updated_at) VALUES (1,?1,?2,1,0,0,0,?3)",
            params![uuid::Uuid::now_v7().as_bytes().as_slice(), uuid::Uuid::now_v7().as_bytes().as_slice(), unix_timestamp_millis()],
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn device_uuid(&self) -> anyhow::Result<uuid::Uuid> {
        let bytes: Vec<u8> = self
            .connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .query_row(
                "SELECT device_uuid FROM host_sync_local_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
        uuid::Uuid::from_slice(&bytes).map_err(Into::into)
    }

    /// Start provider-neutral first-enable capture. Transport-specific account
    /// and remote descriptor work is deliberately left to the provider child.
    pub fn begin_sync_binding(
        &self,
        library_uuid: &str,
        provider_id: &str,
        account_identity: &str,
    ) -> anyhow::Result<(uuid::Uuid, uuid::Uuid)> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let now = unix_timestamp_millis();
        let (device, stream, host_actor): (Vec<u8>, Vec<u8>, Option<Vec<u8>>) = transaction
            .query_row(
                "SELECT device_uuid,stream_uuid,active_actor_epoch FROM host_sync_local_state WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            )?;
        if host_actor.is_none() {
            let actor = uuid::Uuid::now_v7();
            transaction.execute(
                "INSERT INTO host_sync_actor_epochs(actor_epoch,device_uuid,stream_uuid,state,next_actor_seq,published_through_seq,created_at) VALUES (?1,?2,?3,'active',1,0,?4)",
                params![actor.as_bytes().as_slice(),device,stream,now],
            )?;
            transaction.execute(
                "UPDATE host_sync_local_state SET active_actor_epoch=?1,capture_enabled=1,updated_at=?2 WHERE singleton=1",
                params![actor.as_bytes().as_slice(),now],
            )?;
        }
        let binding = uuid::Uuid::now_v7();
        let catalog_actor = uuid::Uuid::now_v7();
        transaction.execute(
            "INSERT INTO library_sync_bindings(library_uuid,provider_id,account_identity,binding_epoch,actor_epoch,state,transport_generation,cutover_actor_seq,published_through_seq,capture_generation,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,'preparing',1,0,0,NULL,?6,?6)",
            params![library_uuid,provider_id,account_identity,binding.as_bytes().as_slice(),catalog_actor.as_bytes().as_slice(),now],
        )?;
        transaction.commit()?;
        Ok((binding, catalog_actor))
    }

    pub fn abort_sync_binding(
        &self,
        library_uuid: &str,
        binding_epoch: uuid::Uuid,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM library_sync_bindings WHERE library_uuid=?1 AND binding_epoch=?2 AND state='preparing'",
            params![library_uuid, binding_epoch.as_bytes().as_slice()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn set_sync_capture_generation(
        &self,
        library_uuid: &str,
        binding_epoch: uuid::Uuid,
        generation: i64,
    ) -> anyhow::Result<()> {
        let changed = self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "UPDATE library_sync_bindings SET capture_generation=?1,updated_at=?2 WHERE library_uuid=?3 AND binding_epoch=?4 AND state='preparing'",
            params![generation,unix_timestamp_millis(),library_uuid,binding_epoch.as_bytes().as_slice()],
        )?;
        if changed != 1 {
            anyhow::bail!("sync binding capture generation fence was lost");
        }
        Ok(())
    }

    pub fn pending_sync_capture_generation(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<Option<i64>> {
        Ok(self.connection.lock().unwrap_or_else(|p|p.into_inner()).query_row(
            "SELECT capture_generation FROM library_sync_bindings WHERE library_uuid=?1 AND state IN ('preparing','seeding')",
            [library_uuid],|row|row.get(0),
        ).optional()?)
    }

    /// Descriptor publication is fenced on a durable, validated Catalog
    /// checkpoint. Provider code may call this only after the Catalog gate has
    /// succeeded; Host then atomically exposes the seed identity.
    pub fn mark_sync_seed_ready(
        &self,
        library_uuid: &str,
        checkpoint_uuid: uuid::Uuid,
        capture_generation: i64,
        catalog_checkpoint_ready: bool,
    ) -> anyhow::Result<()> {
        if !catalog_checkpoint_ready {
            anyhow::bail!("catalog checkpoint is not ready for descriptor publication");
        }
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let changed = connection.execute(
            "UPDATE library_sync_bindings SET state='active',latest_checkpoint_uuid=?1,capture_generation=?2,updated_at=?3 WHERE library_uuid=?4 AND state IN ('preparing','seeding') AND capture_generation=?2",
            params![checkpoint_uuid.as_bytes().as_slice(),capture_generation,unix_timestamp_millis(),library_uuid],
        )?;
        if changed != 1 {
            anyhow::bail!("sync binding is not awaiting a seed checkpoint");
        }
        Ok(())
    }

    /// Local unbind cutover. The remote Library is not deleted; unpublished
    /// suffixes are archived by Catalog using the returned published frontier.
    pub fn commit_local_unbind(&self, library_uuid: &str) -> anyhow::Result<i64> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let published: i64 = transaction.query_row(
            "SELECT published_through_seq FROM library_sync_bindings WHERE library_uuid=?1",
            [library_uuid],
            |row| row.get(0),
        )?;
        transaction.execute(
            "UPDATE host_sync_outbox SET state='archived' WHERE library_uuid=?1 AND state IN ('pending','sealed')",
            [library_uuid],
        )?;
        transaction.execute(
            "DELETE FROM library_sync_bindings WHERE library_uuid=?1",
            [library_uuid],
        )?;
        transaction.commit()?;
        Ok(published)
    }

    pub fn sync_published_frontier(&self, library_uuid: &str) -> anyhow::Result<i64> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        connection
            .query_row(
                "SELECT published_through_seq FROM library_sync_bindings WHERE library_uuid=?1",
                [library_uuid],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn snapshot(&self) -> anyhow::Result<HostSnapshot> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let (active_library_uuid, context_revision) = connection.query_row(
            "SELECT active_library_uuid, context_revision FROM host_meta WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)),
        )?;
        let mut statement = connection.prepare(
            "SELECT l.library_uuid,l.root_uuid,l.name,l.icon,l.color,l.local_data_dir,l.managed_root,l.lifecycle_state,CASE WHEN b.state IN ('preparing','seeding') THEN 'synchronizing' WHEN b.state IN ('active','paused') THEN 'synchronized' ELSE l.binding_state END,l.availability,l.marker_identity FROM libraries l LEFT JOIN library_sync_bindings b ON b.library_uuid=l.library_uuid ORDER BY COALESCE(l.last_opened_at,l.created_at) DESC,l.created_at DESC",
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
                "SELECT l.library_uuid,l.root_uuid,l.name,l.icon,l.color,l.local_data_dir,l.managed_root,l.lifecycle_state,CASE WHEN b.state IN ('preparing','seeding') THEN 'synchronizing' WHEN b.state IN ('active','paused') THEN 'synchronized' ELSE l.binding_state END,l.availability,l.marker_identity FROM libraries l LEFT JOIN library_sync_bindings b ON b.library_uuid=l.library_uuid WHERE l.library_uuid=?1",
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
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE libraries SET name=COALESCE(?2,name), icon=CASE WHEN ?3 THEN ?4 ELSE icon END, color=CASE WHEN ?5 THEN ?6 ELSE color END, updated_at=?7 WHERE library_uuid=?1",
            params![library_uuid, name, icon.is_some(), icon.flatten(), color.is_some(), color.flatten(), unix_timestamp_millis()],
        )?;
        if changed != 0 {
            if let Some(name) = name {
                capture_host_metadata(
                    &transaction,
                    library_uuid,
                    "library-name-set",
                    serde_json::json!(name),
                )?;
            }
            if let Some(icon) = icon {
                capture_host_metadata(
                    &transaction,
                    library_uuid,
                    "library-icon-set",
                    serde_json::json!(icon),
                )?;
            }
        }
        transaction.commit()?;
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

fn capture_host_metadata(
    transaction: &rusqlite::Transaction<'_>,
    library_uuid: &str,
    operation: &str,
    value: serde_json::Value,
) -> anyhow::Result<()> {
    let enabled: i64 = transaction.query_row(
        "SELECT capture_enabled FROM host_sync_local_state WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    if enabled == 0 {
        return Ok(());
    }
    let binding: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT binding_epoch FROM library_sync_bindings WHERE library_uuid=?1 AND state IN ('preparing','seeding','active','paused')",
            [library_uuid],
            |row| row.get(0),
        )
        .optional()?;
    let Some(binding) = binding else {
        return Ok(());
    };
    let (device, stream, actor, seq, old_wall, old_logical): (
        Vec<u8>, Vec<u8>, Vec<u8>, i64, i64, i64,
    ) = transaction.query_row(
        "SELECT device_uuid,stream_uuid,active_actor_epoch,next_actor_seq,hlc_wall_ms,hlc_logical FROM host_sync_local_state WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
    )?;
    let now = unix_timestamp_millis();
    let hlc = crate::sync::types::Hlc {
        wall_ms: old_wall.max(0) as u64,
        logical: old_logical.max(0) as u32,
    }
    .tick(now.max(0) as u64);
    let library = uuid::Uuid::parse_str(library_uuid)?;
    let stream_uuid = crate::sync::types::StreamUuid(uuid::Uuid::from_slice(&stream)?);
    let field = match operation {
        "library-name-set" => "name",
        "library-icon-set" => "icon",
        _ => anyhow::bail!("unsupported Host metadata operation"),
    };
    let event = crate::sync::codec::seal_event(crate::sync::types::EventEnvelope {
        wire_version: crate::sync::codec::WIRE_VERSION,
        envelope_version: crate::sync::codec::ENVELOPE_VERSION,
        reducer_version: crate::sync::codec::REDUCER_VERSION,
        library_uuid: crate::sync::types::LibraryUuid(library),
        event_uuid: crate::sync::types::EventUuid::new(),
        entity_uuid: crate::sync::types::EntityUuid(library),
        stream_uuid,
        stream_kind: crate::sync::types::StreamKind::HostMetadata,
        actor: crate::sync::types::ActorId {
            device_uuid: crate::sync::types::DeviceUuid(uuid::Uuid::from_slice(&device)?),
            stream_uuid,
            actor_epoch: crate::sync::types::ActorEpochUuid(uuid::Uuid::from_slice(&actor)?),
        },
        actor_seq: seq as u64,
        binding_epoch: crate::sync::types::BindingEpochUuid(uuid::Uuid::from_slice(&binding)?),
        hlc,
        operation: crate::sync::types::Operation::SetField {
            field: field.into(),
            value,
        },
        base_events: std::collections::BTreeSet::new(),
        key_version: 0,
        integrity_hash: [0; 32],
    })?;
    let payload = crate::sync::codec::encode_event(&event)?;
    transaction.execute(
        "INSERT INTO host_sync_events(event_uuid,library_uuid,stream_uuid,device_uuid,actor_epoch,binding_epoch,actor_seq,hlc_wall_ms,hlc_logical,wire_version,envelope_version,reducer_version,operation,apply_state,integrity_hash,created_at,payload) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,1,1,?10,'local',?11,?12,?13)",
        params![event.event_uuid.0.as_bytes().as_slice(),library_uuid,stream,device,actor,binding,seq,hlc.wall_ms as i64,hlc.logical as i64,operation,event.integrity_hash.as_slice(),now,payload],
    )?;
    if let crate::sync::types::Operation::SetField { value, .. } = &event.operation {
        materialize_host_field_version(transaction, &event, field, value)?;
    }
    transaction.execute(
        "INSERT INTO host_sync_outbox(event_uuid,library_uuid,binding_epoch,state,attempt_count,created_at) VALUES (?1,?2,?3,'pending',0,?4)",
        params![event.event_uuid.0.as_bytes().as_slice(),library_uuid,binding,now],
    )?;
    transaction.execute(
        "UPDATE host_sync_local_state SET next_actor_seq=?1,hlc_wall_ms=?2,hlc_logical=?3,updated_at=?2 WHERE singleton=1",
        params![seq + 1,hlc.wall_ms as i64,hlc.logical as i64],
    )?;
    transaction.execute(
        "INSERT INTO host_sync_actor_frontiers(device_uuid,stream_uuid,actor_epoch,applied_through_seq,expected_next_seq,state,updated_at) VALUES (?1,?2,?3,?4,?5,'ready',?6) ON CONFLICT(device_uuid,stream_uuid,actor_epoch) DO UPDATE SET applied_through_seq=MAX(applied_through_seq,excluded.applied_through_seq),expected_next_seq=MAX(expected_next_seq,excluded.expected_next_seq),state='ready',updated_at=excluded.updated_at",
        params![device,stream,actor,seq,seq + 1,now],
    )?;
    Ok(())
}

fn merge_received_host_hlc(
    transaction: &rusqlite::Transaction<'_>,
    remote: crate::sync::types::Hlc,
) -> anyhow::Result<()> {
    let (wall, logical): (i64, i64) = transaction.query_row(
        "SELECT hlc_wall_ms,hlc_logical FROM host_sync_local_state WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let merged = crate::sync::types::Hlc {
        wall_ms: wall.max(0) as u64,
        logical: logical.max(0) as u32,
    }
    .merge(remote, unix_timestamp_millis().max(0) as u64);
    transaction.execute(
        "UPDATE host_sync_local_state SET hlc_wall_ms=?1,hlc_logical=?2,updated_at=?3 WHERE singleton=1",
        params![merged.wall_ms as i64, merged.logical as i64, unix_timestamp_millis()],
    )?;
    Ok(())
}

fn materialize_host_field_version(
    transaction: &rusqlite::Transaction<'_>,
    event: &crate::sync::types::EventEnvelope,
    field: &str,
    value: &serde_json::Value,
) -> anyhow::Result<()> {
    if field == "name" && !value.is_string() {
        anyhow::bail!("invalid Host name");
    }
    if field == "icon" && !(value.is_null() || value.is_string()) {
        anyhow::bail!("invalid Host optional scalar");
    }
    let current: Option<(i64, i64, Vec<u8>, Vec<u8>, i64, Vec<u8>)> = transaction
        .query_row(
            "SELECT hlc_wall_ms,hlc_logical,device_uuid,actor_epoch,actor_seq,event_uuid FROM host_sync_field_versions WHERE library_uuid=?1 AND field=?2 AND is_winner=1",
            params![event.library_uuid.to_string(), field],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
        )
        .optional()?;
    let incoming_key = (
        event.hlc.wall_ms,
        event.hlc.logical,
        event.actor.device_uuid.0.as_bytes().to_vec(),
        event.actor.actor_epoch.0.as_bytes().to_vec(),
        event.actor_seq,
        event.event_uuid.0.as_bytes().to_vec(),
    );
    let wins = current.as_ref().is_none_or(|current| {
        incoming_key
            > (
                current.0.max(0) as u64,
                current.1.max(0) as u32,
                current.2.clone(),
                current.3.clone(),
                current.4.max(0) as u64,
                current.5.clone(),
            )
    });
    if wins {
        transaction.execute(
            "UPDATE host_sync_field_versions SET is_winner=0 WHERE library_uuid=?1 AND field=?2 AND is_winner=1",
            params![event.library_uuid.to_string(), field],
        )?;
    }
    transaction.execute(
        "INSERT OR IGNORE INTO host_sync_field_versions(event_uuid,library_uuid,field,value_json,hlc_wall_ms,hlc_logical,device_uuid,actor_epoch,actor_seq,is_winner,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![event.event_uuid.0.as_bytes().as_slice(),event.library_uuid.to_string(),field,serde_json::to_string(value)?,event.hlc.wall_ms as i64,event.hlc.logical as i64,event.actor.device_uuid.0.as_bytes().as_slice(),event.actor.actor_epoch.0.as_bytes().as_slice(),event.actor_seq as i64,wins,unix_timestamp_millis()],
    )?;
    if wins {
        let column = match field {
            "name" => "name",
            "icon" => "icon",
            _ => anyhow::bail!("unsupported Host metadata field"),
        };
        let scalar: Option<&str> = value.as_str();
        transaction.execute(
            &format!("UPDATE libraries SET {column}=?1,updated_at=?2 WHERE library_uuid=?3"),
            params![
                scalar,
                unix_timestamp_millis(),
                event.library_uuid.to_string()
            ],
        )?;
    }
    Ok(())
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

    #[test]
    fn sync_binding_and_local_unbind_leave_remote_identity_out_of_library_lifecycle() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let now = unix_timestamp_millis();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES ('library','root','Library','local','root','ready','local-only','available',1,'marker',?1,?1)",
            [now],
        ).unwrap();
        let (binding, actor) = registry
            .begin_sync_binding("library", "fake", "account")
            .unwrap();
        assert_ne!(binding, actor);
        let row: (String, i64) = registry.connection.lock().unwrap().query_row(
            "SELECT state,published_through_seq FROM library_sync_bindings WHERE library_uuid='library'",
            [],
            |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!(row, ("preparing".into(), 0));
        assert_eq!(registry.commit_local_unbind("library").unwrap(), 0);
        let count: i64 = registry
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM library_sync_bindings", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        assert!(registry.library("library").unwrap().is_some());
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn host_incoming_segment_is_idempotent_and_materializes_metadata() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let library = uuid::Uuid::now_v7();
        registry.connection.lock().unwrap().execute("INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES (?1,?2,'Old','local','root','ready','local-only','available',1,'marker',1,1)",params![library.to_string(),uuid::Uuid::now_v7().to_string()]).unwrap();
        let event = crate::sync::codec::seal_event(crate::sync::types::EventEnvelope {
            wire_version: 1,
            envelope_version: 1,
            reducer_version: 1,
            library_uuid: crate::sync::types::LibraryUuid(library),
            event_uuid: crate::sync::types::EventUuid::new(),
            entity_uuid: crate::sync::types::EntityUuid(library),
            stream_uuid: crate::sync::types::StreamUuid(uuid::Uuid::now_v7()),
            stream_kind: crate::sync::types::StreamKind::HostMetadata,
            actor: crate::sync::types::ActorId {
                device_uuid: crate::sync::types::DeviceUuid(uuid::Uuid::now_v7()),
                stream_uuid: crate::sync::types::StreamUuid(uuid::Uuid::now_v7()),
                actor_epoch: crate::sync::types::ActorEpochUuid(uuid::Uuid::now_v7()),
            },
            actor_seq: 1,
            binding_epoch: crate::sync::types::BindingEpochUuid::new(),
            hlc: crate::sync::types::Hlc {
                wall_ms: 1,
                logical: 0,
            },
            operation: crate::sync::types::Operation::SetField {
                field: "name".into(),
                value: serde_json::json!("Remote"),
            },
            base_events: Default::default(),
            key_version: 0,
            integrity_hash: [0; 32],
        })
        .unwrap();
        let mut event = event;
        event.stream_uuid = event.actor.stream_uuid;
        event = crate::sync::codec::seal_event(event).unwrap();
        let segment = crate::sync::segment::seal_segment(vec![event], None).unwrap();
        assert_eq!(registry.apply_host_segment(&segment).unwrap(), 1);
        assert_eq!(registry.apply_host_segment(&segment).unwrap(), 0);
        assert_eq!(
            registry
                .library(&library.to_string())
                .unwrap()
                .unwrap()
                .name,
            "Remote"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn host_field_lww_is_independent_of_actor_arrival_order_and_merges_hlc() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let library = uuid::Uuid::now_v7();
        registry.connection.lock().unwrap().execute("INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES (?1,?2,'Old','local','root','ready','local-only','available',1,'marker',1,1)",params![library.to_string(),uuid::Uuid::now_v7().to_string()]).unwrap();
        let make = |wall, name: &str| {
            let stream = crate::sync::types::StreamUuid(uuid::Uuid::now_v7());
            let event = crate::sync::codec::seal_event(crate::sync::types::EventEnvelope {
                wire_version: 1,
                envelope_version: 1,
                reducer_version: 1,
                library_uuid: crate::sync::types::LibraryUuid(library),
                event_uuid: crate::sync::types::EventUuid::new(),
                entity_uuid: crate::sync::types::EntityUuid(library),
                stream_uuid: stream,
                stream_kind: crate::sync::types::StreamKind::HostMetadata,
                actor: crate::sync::types::ActorId {
                    device_uuid: crate::sync::types::DeviceUuid(uuid::Uuid::now_v7()),
                    stream_uuid: stream,
                    actor_epoch: crate::sync::types::ActorEpochUuid::new(),
                },
                actor_seq: 1,
                binding_epoch: crate::sync::types::BindingEpochUuid::new(),
                hlc: crate::sync::types::Hlc {
                    wall_ms: wall,
                    logical: 0,
                },
                operation: crate::sync::types::Operation::SetField {
                    field: "name".into(),
                    value: serde_json::json!(name),
                },
                base_events: Default::default(),
                key_version: 0,
                integrity_hash: [0; 32],
            })
            .unwrap();
            crate::sync::segment::seal_segment(vec![event], None).unwrap()
        };
        registry.apply_host_segment(&make(20, "winner")).unwrap();
        registry.apply_host_segment(&make(10, "loser")).unwrap();
        assert_eq!(
            registry
                .library(&library.to_string())
                .unwrap()
                .unwrap()
                .name,
            "winner"
        );
        let connection = registry.connection.lock().unwrap();
        let (history, winners, wall): (i64,i64,i64) = connection.query_row(
            "SELECT (SELECT COUNT(*) FROM host_sync_field_versions WHERE library_uuid=?1 AND field='name'),(SELECT COUNT(*) FROM host_sync_field_versions WHERE library_uuid=?1 AND field='name' AND is_winner=1),(SELECT hlc_wall_ms FROM host_sync_local_state WHERE singleton=1)",
            [library.to_string()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        ).unwrap();
        assert_eq!((history, winners), (2, 1));
        assert!(wall >= 20);
        drop(connection);
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn local_host_color_remains_local_only() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let library = uuid::Uuid::now_v7().to_string();
        registry.connection.lock().unwrap().execute("INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES (?1,?2,'Old','local','root','ready','local-only','available',1,'marker',1,1)",params![library,uuid::Uuid::now_v7().to_string()]).unwrap();
        registry
            .begin_sync_binding(&library, "fake", "account")
            .unwrap();
        registry
            .update_library_metadata(&library, None, None, Some(Some("#123456")))
            .unwrap();
        let events: i64 = registry
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM host_sync_events WHERE library_uuid=?1",
                [&library],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(events, 0);
        assert_eq!(
            registry
                .library(&library)
                .unwrap()
                .unwrap()
                .color
                .as_deref(),
            Some("#123456")
        );
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn host_incoming_gap_state_survives_rejected_segment() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let library = uuid::Uuid::now_v7();
        registry.connection.lock().unwrap().execute("INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES (?1,?2,'Old','local','root','ready','local-only','available',1,'marker',1,1)",params![library.to_string(),uuid::Uuid::now_v7().to_string()]).unwrap();
        let actor = crate::sync::types::ActorId {
            device_uuid: crate::sync::types::DeviceUuid(uuid::Uuid::now_v7()),
            stream_uuid: crate::sync::types::StreamUuid(uuid::Uuid::now_v7()),
            actor_epoch: crate::sync::types::ActorEpochUuid(uuid::Uuid::now_v7()),
        };
        let event = crate::sync::codec::seal_event(crate::sync::types::EventEnvelope {
            wire_version: 1,
            envelope_version: 1,
            reducer_version: 1,
            library_uuid: crate::sync::types::LibraryUuid(library),
            event_uuid: crate::sync::types::EventUuid::new(),
            entity_uuid: crate::sync::types::EntityUuid(library),
            stream_uuid: actor.stream_uuid,
            stream_kind: crate::sync::types::StreamKind::HostMetadata,
            actor: actor.clone(),
            actor_seq: 2,
            binding_epoch: crate::sync::types::BindingEpochUuid::new(),
            hlc: crate::sync::types::Hlc {
                wall_ms: 1,
                logical: 0,
            },
            operation: crate::sync::types::Operation::SetField {
                field: "name".into(),
                value: serde_json::json!("Remote"),
            },
            base_events: Default::default(),
            key_version: 0,
            integrity_hash: [0; 32],
        })
        .unwrap();
        let segment = crate::sync::segment::seal_segment(vec![event], None).unwrap();
        assert!(registry.apply_host_segment(&segment).is_err());
        let state: String = registry.connection.lock().unwrap().query_row(
            "SELECT state FROM host_sync_actor_frontiers WHERE device_uuid=?1 AND stream_uuid=?2 AND actor_epoch=?3",
            params![
                actor.device_uuid.0.as_bytes().as_slice(),
                actor.stream_uuid.0.as_bytes().as_slice(),
                actor.actor_epoch.0.as_bytes().as_slice()
            ],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(state, "gap");
        drop(registry);
        let _ = std::fs::remove_file(path);
    }
}
