use anyhow::{bail, Context};
use rusqlite::{params, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use uuid::Uuid;

use super::Database;

fn root_now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRootDescriptorRecord {
    pub root_uuid: [u8; 16],
    pub root_folder_uuid: [u8; 16],
    pub library_root_uuid: [u8; 16],
    pub creation_event_uuid: Option<[u8; 16]>,
    pub winning_location_revision_uuid: Option<[u8; 16]>,
    pub requested_name: String,
    pub materialized_name: String,
    pub materialization_fingerprint: [u8; 32],
    pub lifecycle: String,
    pub logical_revision: u64,
    pub updated_at: i64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncRootBindingToken {
    pub root_uuid: [u8; 16],
    pub local_binding_uuid: [u8; 16],
    pub binding_generation: u64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRootLocalBindingRecord {
    pub root_uuid: [u8; 16],
    pub library_uuid: [u8; 16],
    pub local_binding_uuid: [u8; 16],
    pub binding_generation: u64,
    pub root_capture_epoch: [u8; 16],
    pub root_actor_epoch: [u8; 16],
    pub state: String,
    pub root_id: i64,
    pub operation_revision: u64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootRelocationCatalogCutover {
    pub root_id: i64,
    pub scan_task_id: i64,
}

#[allow(dead_code)]
fn fixed_bytes<const N: usize>(value: Vec<u8>, field: &str) -> anyhow::Result<[u8; N]> {
    value
        .try_into()
        .map_err(|_| anyhow::anyhow!("{field} must contain exactly {N} bytes"))
}

impl Database {
    pub fn sync_location_heads(&self, entity_uuid: Uuid) -> anyhow::Result<BTreeSet<Uuid>> {
        let transaction = self.connection.unchecked_transaction()?;
        let heads = location_heads(&transaction, entity_uuid)?;
        transaction.commit()?;
        Ok(heads)
    }

    pub fn fence_sync_root_relocation(
        &self,
        root_uuid: Uuid,
        local_binding_uuid: Uuid,
        old_generation: u64,
        new_generation: u64,
        operation_revision: u64,
    ) -> anyhow::Result<i64> {
        let transaction = self.connection.unchecked_transaction()?;
        let (root_id, current_generation, current_state): (i64, i64, String) = transaction
            .query_row(
                "SELECT root_id,binding_generation,state FROM sync_root_local_bindings WHERE root_uuid=?1 AND local_binding_uuid=?2",
                params![root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice()],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            )
            .context("local root binding not found")?;
        if current_generation == i64::try_from(new_generation)? {
            if current_state != "relocating" {
                transaction.execute(
                    "UPDATE sync_root_local_bindings SET state='relocating',operation_revision=?1,updated_at=?2 WHERE root_uuid=?3 AND local_binding_uuid=?4 AND binding_generation=?5 AND state IN ('active','unavailable','local-rename-required')",
                    params![i64::try_from(operation_revision)?,root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(new_generation)?],
                )?;
            }
            transaction.commit()?;
            return Ok(root_id);
        }
        if current_generation != i64::try_from(old_generation)? {
            bail!("stale root binding generation");
        }
        let changed = transaction.execute(
            "UPDATE sync_root_local_bindings SET binding_generation=?1,state='relocating',operation_revision=?2,updated_at=?3 WHERE root_uuid=?4 AND local_binding_uuid=?5 AND binding_generation=?6",
            params![i64::try_from(new_generation)?,i64::try_from(operation_revision)?,root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?],
        )?;
        if changed != 1 {
            bail!("stale root binding generation");
        }
        transaction.execute(
            "UPDATE tasks SET status='cancelled',error='stale-root-binding',updated_at=?1 WHERE root_uuid=?2 AND local_binding_uuid=?3 AND binding_generation=?4 AND status IN ('pending','running')",
            params![root_now_millis()/1000,root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?],
        )?;
        transaction.execute(
            "UPDATE file_operations SET status='cancelled',journal_phase='cancelled',error='stale-root-binding' WHERE root_uuid=?1 AND local_binding_uuid=?2 AND binding_generation=?3 AND status IN ('pending','running')",
            params![root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?],
        )?;
        transaction.execute(
            "UPDATE media_publication_jobs SET phase='cancelled',owner_token=NULL,lease_expires_at=NULL,error_code='stale-root-binding',updated_at=?1 WHERE root_uuid=?2 AND local_binding_uuid=?3 AND binding_generation=?4 AND phase NOT IN ('event-published','archived')",
            params![root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?],
        )?;
        transaction.execute(
            "UPDATE media_materialization_jobs SET phase='cancelled',owner_token=NULL,lease_expires_at=NULL,error_code='stale-root-binding',updated_at=?1 WHERE root_uuid=?2 AND local_binding_uuid=?3 AND binding_generation=?4 AND phase<>'complete'",
            params![root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?],
        )?;
        transaction.execute(
            "UPDATE sync_publication_intents SET state='cancelled',error_code='stale-root-binding',next_attempt_at=NULL,updated_at=?1 WHERE root_uuid=?2 AND local_binding_uuid=?3 AND binding_generation=?4 AND state IN ('pending-hash','pending-manifest','ready','failed')",
            params![root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?],
        )?;
        transaction.commit()?;
        Ok(root_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn cutover_sync_root_relocation(
        &self,
        root_uuid: Uuid,
        local_binding_uuid: Uuid,
        old_generation: u64,
        new_generation: u64,
        new_path: &Path,
        materialized_name: &str,
        operation_revision: u64,
    ) -> anyhow::Result<RootRelocationCatalogCutover> {
        let transaction = self.connection.unchecked_transaction()?;
        let (root_id, current_generation, current_state): (i64, i64, String) = transaction
            .query_row(
                "SELECT root_id,binding_generation,state FROM sync_root_local_bindings WHERE root_uuid=?1 AND local_binding_uuid=?2",
                params![root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice()],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            )
            .context("local root binding not found")?;
        if current_generation == i64::try_from(new_generation)?
            && matches!(current_state.as_str(), "reconcile-required" | "active")
        {
            let scan_task_id = transaction
                .query_row(
                    "SELECT id FROM tasks WHERE root_uuid=?1 AND local_binding_uuid=?2 AND binding_generation=?3 AND kind='root_scan' AND status IN ('pending','running','succeeded') ORDER BY id DESC LIMIT 1",
                    params![root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(new_generation)?],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or(0);
            transaction.commit()?;
            return Ok(RootRelocationCatalogCutover {
                root_id,
                scan_task_id,
            });
        }
        if current_generation != i64::try_from(new_generation)? || current_state != "relocating" {
            bail!("stale root binding generation");
        }
        let path = new_path.to_string_lossy().into_owned();
        transaction.execute(
            "UPDATE roots SET path=?1,display_name=?2,active_scan_generation=NULL,last_scan_at=NULL WHERE id=?3",
            params![path,materialized_name,root_id],
        )?;

        let mut folder_rows = transaction.prepare(
            "SELECT id,parent_id,name FROM folders WHERE root_id=?1 ORDER BY CASE WHEN parent_id IS NULL THEN 0 ELSE 1 END,id",
        )?;
        let rows = folder_rows
            .query_map([root_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(folder_rows);
        let mut resolved = BTreeMap::<i64, std::path::PathBuf>::new();
        let mut pending = rows;
        while !pending.is_empty() {
            let mut next = Vec::new();
            let mut progressed = false;
            for (id, parent_id, name) in pending {
                let full = match parent_id {
                    None => Some(new_path.to_path_buf()),
                    Some(parent_id) => resolved.get(&parent_id).map(|parent| parent.join(&name)),
                };
                if let Some(full) = full {
                    transaction.execute(
                        "UPDATE folders SET path_hash=?1 WHERE id=?2",
                        params![super::hash_path(&full), id],
                    )?;
                    resolved.insert(id, full);
                    progressed = true;
                } else {
                    next.push((id, parent_id, name));
                }
            }
            if !progressed {
                bail!("folder hierarchy is not resolvable");
            }
            pending = next;
        }
        transaction.execute(
            "UPDATE sync_publication_intents SET source_path=(WITH RECURSIVE paths(id,parent_id,relpath) AS (SELECT id,parent_id,'' FROM folders WHERE root_id=?1 AND parent_id IS NULL UNION ALL SELECT folder.id,folder.parent_id,CASE WHEN paths.relpath='' THEN folder.name ELSE paths.relpath||'/'||folder.name END FROM folders folder JOIN paths ON paths.id=folder.parent_id) SELECT ?2||CASE WHEN paths.relpath='' THEN '' ELSE '/'||paths.relpath END||'/'||files.name FROM files JOIN paths ON paths.id=files.folder_id WHERE files.asset_uuid=sync_publication_intents.asset_uuid),updated_at=?3 WHERE root_uuid=?4 AND local_binding_uuid=?5 AND binding_generation=?6",
            params![root_id,path.replace('\\', "/"),root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?],
        )?;
        let changed = transaction.execute(
            "UPDATE sync_root_local_bindings SET state='reconcile-required',operation_revision=?1,updated_at=?2 WHERE root_uuid=?3 AND local_binding_uuid=?4 AND binding_generation=?5 AND state='relocating'",
            params![i64::try_from(operation_revision)?,root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(new_generation)?],
        )?;
        if changed != 1 {
            bail!("stale root binding generation");
        }
        transaction.execute(
            "INSERT INTO tasks(kind,priority,status,root_id,folder_id,file_id,created_at,updated_at,root_uuid,local_binding_uuid,binding_generation,error) VALUES ('root_scan',100,'pending',?1,NULL,NULL,?2,?2,?3,?4,?5,NULL)",
            params![root_id,root_now_millis()/1000,root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(new_generation)?],
        )?;
        let scan_task_id = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(RootRelocationCatalogCutover {
            root_id,
            scan_task_id,
        })
    }

    pub fn sync_root_relocation_reconciliation_status(
        &self,
        root_uuid: Uuid,
        local_binding_uuid: Uuid,
        binding_generation: u64,
    ) -> anyhow::Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT status FROM tasks WHERE root_uuid=?1 AND local_binding_uuid=?2 AND binding_generation=?3 AND kind='root_scan' ORDER BY id DESC LIMIT 1",
                params![root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(binding_generation)?],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn sync_root_relocation_catalog_cutover_is_current(
        &self,
        root_uuid: Uuid,
        local_binding_uuid: Uuid,
        binding_generation: u64,
        target_path: &Path,
    ) -> anyhow::Result<bool> {
        let current = self
            .connection
            .query_row(
                "SELECT binding.state,roots.path FROM sync_root_local_bindings binding JOIN roots ON roots.id=binding.root_id WHERE binding.root_uuid=?1 AND binding.local_binding_uuid=?2 AND binding.binding_generation=?3",
                params![
                    root_uuid.as_bytes().as_slice(),
                    local_binding_uuid.as_bytes().as_slice(),
                    i64::try_from(binding_generation)?
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        Ok(current.is_some_and(|(state, path)| {
            matches!(state.as_str(), "reconcile-required" | "active" | "paused")
                && crate::sync_folders::canonical_path_key(Path::new(&path))
                    == crate::sync_folders::canonical_path_key(target_path)
        }))
    }

    pub fn complete_sync_root_relocation_projection(
        &self,
        root_uuid: Uuid,
        local_binding_uuid: Uuid,
        binding_generation: u64,
        operation_revision: u64,
        resume_state: &str,
    ) -> anyhow::Result<()> {
        if !matches!(resume_state, "active" | "paused") {
            bail!("invalid root resume state");
        }
        let changed = self.connection.execute(
            "UPDATE sync_root_local_bindings SET state=?1,operation_revision=MAX(operation_revision,?2),updated_at=?3 WHERE root_uuid=?4 AND local_binding_uuid=?5 AND binding_generation=?6 AND state IN ('reconcile-required','active','paused')",
            params![resume_state,i64::try_from(operation_revision)?,root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(binding_generation)?],
        )?;
        if changed != 1 {
            bail!("stale root binding generation");
        }
        Ok(())
    }

    pub fn abort_sync_root_relocation_projection(
        &self,
        root_uuid: Uuid,
        local_binding_uuid: Uuid,
        binding_generation: u64,
        operation_revision: u64,
        resume_state: &str,
    ) -> anyhow::Result<()> {
        if !matches!(resume_state, "active" | "paused" | "local-rename-required") {
            bail!("invalid root resume state");
        }
        let current_generation: i64 = self.connection.query_row(
            "SELECT binding_generation FROM sync_root_local_bindings WHERE root_uuid=?1 AND local_binding_uuid=?2",
            params![root_uuid.as_bytes().as_slice(), local_binding_uuid.as_bytes().as_slice()],
            |row| row.get(0),
        )?;
        if current_generation.saturating_add(1) == i64::try_from(binding_generation)? {
            self.fence_sync_root_relocation(
                root_uuid,
                local_binding_uuid,
                u64::try_from(current_generation)?,
                binding_generation,
                operation_revision,
            )?;
        }
        let changed = self.connection.execute(
            "UPDATE sync_root_local_bindings SET state=?1,operation_revision=MAX(operation_revision,?2),updated_at=?3 WHERE root_uuid=?4 AND local_binding_uuid=?5 AND binding_generation=?6 AND (state='relocating' OR state=?1)",
            params![resume_state,i64::try_from(operation_revision)?,root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(binding_generation)?],
        )?;
        if changed != 1 {
            bail!("stale root binding generation");
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ensure_selected_sync_root(
        &self,
        library_uuid: uuid::Uuid,
        root_uuid: uuid::Uuid,
        root_folder_uuid: uuid::Uuid,
        library_root_uuid: uuid::Uuid,
        path: &str,
        requested_name: &str,
        materialized_name: &str,
        materialization_fingerprint: [u8; 32],
        local_binding_uuid: uuid::Uuid,
        root_capture_epoch: uuid::Uuid,
        root_actor_epoch: uuid::Uuid,
        binding_generation: u64,
        state: &str,
        operation_revision: u64,
        enqueue_scan: bool,
    ) -> anyhow::Result<Option<i64>> {
        if let Some(root_id) = self.sync_root_id(root_uuid.as_bytes())? {
            self.upsert_sync_root_local_binding(&SyncRootLocalBindingRecord {
                root_uuid: *root_uuid.as_bytes(),
                library_uuid: *library_uuid.as_bytes(),
                local_binding_uuid: *local_binding_uuid.as_bytes(),
                binding_generation,
                root_capture_epoch: *root_capture_epoch.as_bytes(),
                root_actor_epoch: *root_actor_epoch.as_bytes(),
                state: state.to_owned(),
                root_id,
                operation_revision,
                updated_at: root_now_millis(),
            })?;
            return Ok(None);
        }
        let root_id = self.add_root(crate::db::NewRoot {
            path: path.to_owned(),
            display_name: materialized_name.to_owned(),
        })?;
        self.connection.execute(
            "UPDATE roots SET root_uuid=?1 WHERE id=?2",
            params![root_uuid.as_bytes().as_slice(), root_id],
        )?;
        self.connection.execute("INSERT OR IGNORE INTO folders(root_id,parent_id,name,path_hash,mtime,status,folder_uuid) VALUES (?1,NULL,'','',NULL,'active',?2)",params![root_id,root_folder_uuid.as_bytes().as_slice()])?;
        self.upsert_sync_root_descriptor(&SyncRootDescriptorRecord {
            root_uuid: *root_uuid.as_bytes(),
            root_folder_uuid: *root_folder_uuid.as_bytes(),
            library_root_uuid: *library_root_uuid.as_bytes(),
            creation_event_uuid: None,
            winning_location_revision_uuid: None,
            requested_name: requested_name.to_owned(),
            materialized_name: materialized_name.to_owned(),
            materialization_fingerprint,
            lifecycle: "active".to_owned(),
            logical_revision: 0,
            updated_at: root_now_millis(),
        })?;
        self.link_sync_root_descriptor(root_id, root_uuid.as_bytes(), root_folder_uuid.as_bytes())?;
        self.upsert_sync_root_local_binding(&SyncRootLocalBindingRecord {
            root_uuid: *root_uuid.as_bytes(),
            library_uuid: *library_uuid.as_bytes(),
            local_binding_uuid: *local_binding_uuid.as_bytes(),
            binding_generation,
            root_capture_epoch: *root_capture_epoch.as_bytes(),
            root_actor_epoch: *root_actor_epoch.as_bytes(),
            state: state.to_owned(),
            root_id,
            operation_revision,
            updated_at: root_now_millis(),
        })?;
        if enqueue_scan {
            let token = SyncRootBindingToken {
                root_uuid: *root_uuid.as_bytes(),
                local_binding_uuid: *local_binding_uuid.as_bytes(),
                binding_generation,
            };
            return Ok(Some(
                self.create_root_scan_task_with_origin_for_root_token(
                    root_id,
                    Some(&token),
                    crate::db::NotificationOrigin::User,
                )?,
            ));
        }
        Ok(None)
    }

    pub fn fence_root_as_local_only(
        &self,
        root_uuid: uuid::Uuid,
        local_binding_uuid: uuid::Uuid,
        old_generation: u64,
        new_generation: u64,
        root_capture_epoch: uuid::Uuid,
        root_actor_epoch: uuid::Uuid,
        operation_revision: u64,
    ) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let current:(i64,String,Vec<u8>,Vec<u8>,i64)=transaction.query_row("SELECT binding_generation,state,COALESCE(root_capture_epoch,local_binding_uuid),COALESCE(root_actor_epoch,local_binding_uuid),operation_revision FROM sync_root_local_bindings WHERE root_uuid=?1 AND local_binding_uuid=?2",params![root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?;
        if current.0 == i64::try_from(new_generation)?
            && current.1 == "local-only-fork"
            && current.2 == root_capture_epoch.as_bytes()
            && current.3 == root_actor_epoch.as_bytes()
            && current.4 >= i64::try_from(operation_revision)?
        {
            transaction.commit()?;
            return Ok(());
        }
        if current.0 != i64::try_from(old_generation)? {
            bail!("stale root binding generation");
        }
        let old_actor_epoch = current.3;
        let changed=transaction.execute("UPDATE sync_root_local_bindings SET state='local-only-fork',binding_generation=?1,root_capture_epoch=?2,root_actor_epoch=?3,operation_revision=?4,updated_at=?5 WHERE root_uuid=?6 AND local_binding_uuid=?7 AND binding_generation=?8",params![i64::try_from(new_generation)?,root_capture_epoch.as_bytes().as_slice(),root_actor_epoch.as_bytes().as_slice(),i64::try_from(operation_revision)?,root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?])?;
        if changed != 1 {
            let already_fenced: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_root_local_bindings WHERE root_uuid=?1 AND local_binding_uuid=?2 AND binding_generation=?3 AND operation_revision>=?4 AND state='local-only-fork')",
                params![root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(new_generation)?,i64::try_from(operation_revision)?],
                |row| row.get(0),
            )?;
            if !already_fenced {
                bail!("stale root binding generation");
            }
        }
        transaction.execute("UPDATE tasks SET status='cancelled',error='stale-root-binding' WHERE root_uuid=?1 AND local_binding_uuid=?2 AND binding_generation=?3 AND status IN ('pending','running')",params![root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?])?;
        transaction.execute("UPDATE file_operations SET status='cancelled',journal_phase='cancelled',error='stale-root-binding' WHERE root_uuid=?1 AND local_binding_uuid=?2 AND binding_generation=?3 AND status IN ('pending','running')",params![root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?])?;
        transaction.execute("UPDATE sync_publication_intents SET state='archived',error_code='root-local-only-fork',updated_at=?1 WHERE root_uuid=?2 AND local_binding_uuid=?3 AND binding_generation=?4 AND state NOT IN ('canonicalized','cancelled','archived')",params![root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?])?;
        transaction.execute("UPDATE media_publication_jobs SET phase='archived',error_code='root-local-only-fork',updated_at=?1 WHERE root_uuid=?2 AND local_binding_uuid=?3 AND binding_generation=?4 AND phase NOT IN ('event-published','cancelled','archived')",params![root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?])?;
        transaction.execute("UPDATE media_materialization_jobs SET phase='cancelled',error_code='root-local-only-fork',updated_at=?1 WHERE root_uuid=?2 AND local_binding_uuid=?3 AND binding_generation=?4 AND phase NOT IN ('complete','cancelled')",params![root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(old_generation)?])?;
        transaction.execute("UPDATE sync_outbox SET state='archived' WHERE event_uuid IN (SELECT event.event_uuid FROM sync_events event WHERE event.entity_uuid IN (SELECT asset_uuid FROM files WHERE root_id=(SELECT root_id FROM sync_root_local_bindings WHERE root_uuid=?1)) OR event.entity_uuid IN (SELECT folder_uuid FROM folders WHERE root_id=(SELECT root_id FROM sync_root_local_bindings WHERE root_uuid=?1))) AND state IN ('pending','sealed')",[root_uuid.as_bytes().as_slice()])?;
        let (device, stream): (Vec<u8>, Vec<u8>) = transaction.query_row(
            "SELECT device_uuid,catalog_stream_uuid FROM sync_local_state WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        transaction.execute(
            "UPDATE sync_actor_epochs SET state='closed' WHERE actor_epoch=?1",
            [old_actor_epoch],
        )?;
        transaction.execute("INSERT INTO sync_actor_epochs(actor_epoch,device_uuid,stream_uuid,stream_kind,binding_epoch,state,next_actor_seq,published_through_seq,created_at) VALUES (?1,?2,?3,'catalog',?4,'forked',1,0,?5) ON CONFLICT(actor_epoch) DO UPDATE SET state='forked',binding_epoch=excluded.binding_epoch",params![root_actor_epoch.as_bytes().as_slice(),device,stream,root_capture_epoch.as_bytes().as_slice(),root_now_millis()])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn canonical_root_snapshot(
        &self,
        root_uuid: uuid::Uuid,
    ) -> anyhow::Result<crate::sync_folders::RootSnapshot> {
        let (state, frontier) = crate::sync::persistence::capture_protocol_state(&self.connection)?;
        crate::sync_folders::snapshot_from_protocol_state(
            &state,
            root_uuid,
            serde_json::to_value(frontier)?,
        )
    }

    pub fn root_stop_work(
        &self,
        root_uuid: Uuid,
    ) -> anyhow::Result<crate::sync_folders::RootStopWork> {
        let actor:Vec<u8>=self.connection.query_row("SELECT COALESCE(root_actor_epoch,local_binding_uuid) FROM sync_root_local_bindings WHERE root_uuid=?1",[root_uuid.as_bytes().as_slice()],|row|row.get(0)).context("local root binding not found")?;
        let pending:i64=self.connection.query_row("SELECT COUNT(*) FROM sync_outbox outbox JOIN sync_events event ON event.event_uuid=outbox.event_uuid WHERE event.actor_epoch=?1 AND outbox.state IN ('pending','sealed')",[&actor],|row|row.get(0))?;
        let mut statement=self.connection.prepare("SELECT spool_path FROM sync_segments WHERE actor_epoch=?1 AND state IN ('published','acked') ORDER BY first_seq")?;
        let paths = statement
            .query_map([actor], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut receipt_keys = Vec::with_capacity(paths.len());
        for path in paths {
            let bytes = std::fs::read(path)?;
            receipt_keys.push(format!(
                "segments/{}.json",
                crate::sync::media::hex_hash(&crate::sync::codec::sha256(&bytes))
            ));
        }
        Ok(crate::sync_folders::RootStopWork {
            pending_work: pending.max(0) as u64,
            published_work: receipt_keys.len() as u64,
            receipt_keys,
        })
    }

    pub fn apply_root_reconciliation_item(
        &self,
        plan_uuid: Uuid,
        item_ordinal: u64,
        root_uuid: Uuid,
        local_binding_uuid: Uuid,
        binding_generation: u64,
        entity_uuid: Uuid,
        field_name: &str,
        resolution: &str,
        selected_value: Option<&str>,
        selected_entity: Option<&str>,
    ) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let already:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM sync_root_reconciliation_applied WHERE plan_uuid=?1 AND item_ordinal=?2)",params![plan_uuid.as_bytes().as_slice(),i64::try_from(item_ordinal)?],|row|row.get(0))?;
        if already {
            transaction.commit()?;
            return Ok(());
        }
        let current:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM sync_root_local_bindings WHERE root_uuid=?1 AND local_binding_uuid=?2 AND binding_generation=?3 AND state='reconcile-required')",params![root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(binding_generation)?],|row|row.get(0))?;
        if !current {
            bail!("stale root binding generation");
        }
        if matches!(resolution, "keep-local" | "keep-remote") {
            ensure_reconciliation_entity(&transaction, root_uuid, entity_uuid, selected_entity)?;
            capture_selected_snapshot_value(
                &transaction,
                plan_uuid,
                item_ordinal,
                b"selected",
                entity_uuid,
                field_name,
                selected_value,
                false,
            )?;
        } else if resolution == "keep-both" {
            let selected_values: serde_json::Value = selected_value
                .map(serde_json::from_str)
                .transpose()?
                .unwrap_or_default();
            let selected_entities: serde_json::Value = selected_entity
                .map(serde_json::from_str)
                .transpose()?
                .unwrap_or_default();
            for side in ["local", "remote"] {
                let entity = selected_entities
                    .get(side)
                    .filter(|value| !value.is_null())
                    .map(serde_json::to_string)
                    .transpose()?;
                ensure_reconciliation_entity(
                    &transaction,
                    root_uuid,
                    entity_uuid,
                    entity.as_deref(),
                )?;
            }
            if field_name == "set:tags" {
                let mut members = BTreeSet::new();
                for side in ["local", "remote"] {
                    if let Some(value) = selected_values
                        .get(side)
                        .filter(|value| !value.is_null())
                        .and_then(serde_json::Value::as_str)
                    {
                        members.extend(serde_json::from_str::<BTreeSet<String>>(value)?);
                    }
                }
                let merged = serde_json::to_string(&members)?;
                capture_selected_snapshot_value(
                    &transaction,
                    plan_uuid,
                    item_ordinal,
                    b"merged",
                    entity_uuid,
                    field_name,
                    Some(&merged),
                    false,
                )?;
            } else {
                for (side, variant, preserve_concurrent) in [
                    ("local", b"local".as_slice(), false),
                    ("remote", b"remote".as_slice(), true),
                ] {
                    let value = selected_values
                        .get(side)
                        .filter(|value| !value.is_null())
                        .and_then(serde_json::Value::as_str);
                    capture_selected_snapshot_value(
                        &transaction,
                        plan_uuid,
                        item_ordinal,
                        variant,
                        entity_uuid,
                        field_name,
                        value,
                        preserve_concurrent,
                    )?;
                }
            }
            preserve_both_reconciliation_versions(
                &transaction,
                entity_uuid,
                field_name,
                selected_value,
            )?;
        }
        transaction.execute("INSERT INTO sync_root_reconciliation_applied(plan_uuid,item_ordinal,root_uuid,local_binding_uuid,binding_generation,applied_at) VALUES (?1,?2,?3,?4,?5,?6)",params![plan_uuid.as_bytes().as_slice(),i64::try_from(item_ordinal)?,root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(binding_generation)?,root_now_millis()])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn complete_root_reconciliation_projection(
        &self,
        root_uuid: Uuid,
        local_binding_uuid: Uuid,
        binding_generation: u64,
        operation_revision: u64,
    ) -> anyhow::Result<()> {
        let changed=self.connection.execute("UPDATE sync_root_local_bindings SET state='active',operation_revision=?1,updated_at=?2 WHERE root_uuid=?3 AND local_binding_uuid=?4 AND binding_generation=?5 AND state IN ('reconcile-required','active')",params![i64::try_from(operation_revision)?,root_now_millis(),root_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),i64::try_from(binding_generation)?])?;
        if changed != 1 {
            bail!("stale root binding generation");
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn publish_root_as_new(
        &self,
        operation_uuid: Uuid,
        library_uuid: Uuid,
        source_root_uuid: Uuid,
        new_root_uuid: Uuid,
        new_root_folder_uuid: Uuid,
        requested_name: &str,
        materialized_name: &str,
        materialization_fingerprint: [u8; 32],
        local_binding_uuid: Uuid,
        root_capture_epoch: Uuid,
        root_actor_epoch: Uuid,
        operation_revision: u64,
    ) -> anyhow::Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        if let Some(state) = transaction
            .query_row(
                "SELECT state FROM sync_root_publish_cutovers WHERE operation_uuid=?1",
                [operation_uuid.as_bytes().as_slice()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            if state == "complete" {
                transaction.commit()?;
                return Ok(());
            }
        }
        let source_root_id: i64 = transaction
            .query_row(
                "SELECT id FROM roots WHERE root_uuid=?1 AND root_folder_uuid IS NOT NULL",
                [source_root_uuid.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .context("source sync root not found")?;
        if transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_root_publish_cutovers WHERE operation_uuid=?1)",
            [operation_uuid.as_bytes().as_slice()],
            |row| row.get::<_, bool>(0),
        )? == false
        {
            transaction.execute("INSERT INTO sync_root_publish_cutovers(operation_uuid,source_root_uuid,new_root_uuid,source_root_id,state,created_at,updated_at) VALUES (?1,?2,?3,?4,'planned',?5,?5)",params![operation_uuid.as_bytes().as_slice(),source_root_uuid.as_bytes().as_slice(),new_root_uuid.as_bytes().as_slice(),source_root_id,root_now_millis()])?;
        }
        let library_root_uuid: Vec<u8> = transaction.query_row(
            "SELECT library_root_uuid FROM sync_root_descriptors WHERE root_uuid=?1",
            [source_root_uuid.as_bytes().as_slice()],
            |row| row.get(0),
        )?;
        transaction.execute("INSERT OR IGNORE INTO sync_root_descriptors(root_uuid,root_folder_uuid,library_root_uuid,creation_event_uuid,winning_location_revision_uuid,requested_name,materialized_name,materialization_fingerprint,lifecycle,logical_revision,updated_at) VALUES (?1,?2,?3,NULL,?4,?5,?6,?7,'active',0,?8)",params![new_root_uuid.as_bytes().as_slice(),new_root_folder_uuid.as_bytes().as_slice(),library_root_uuid,derived_publish_uuid(operation_uuid,b"root-location").as_bytes().as_slice(),requested_name,materialized_name,materialization_fingerprint.as_slice(),root_now_millis()])?;
        transaction.execute("UPDATE tasks SET status='cancelled',error='root-published-as-new' WHERE root_uuid=?1 AND status IN ('pending','running')",[source_root_uuid.as_bytes().as_slice()])?;
        transaction.execute("UPDATE file_operations SET status='cancelled',journal_phase='cancelled',error='root-published-as-new' WHERE root_uuid=?1 AND status IN ('pending','running')",[source_root_uuid.as_bytes().as_slice()])?;
        transaction.execute("UPDATE sync_publication_intents SET state='archived',error_code='root-published-as-new',updated_at=?1 WHERE root_uuid=?2 AND state NOT IN ('canonicalized','cancelled','archived')",params![root_now_millis(),source_root_uuid.as_bytes().as_slice()])?;
        transaction.execute("UPDATE media_publication_jobs SET phase='archived',error_code='root-published-as-new',updated_at=?1 WHERE root_uuid=?2 AND phase NOT IN ('event-published','cancelled','archived')",params![root_now_millis(),source_root_uuid.as_bytes().as_slice()])?;
        transaction.execute("UPDATE media_materialization_jobs SET phase='cancelled',error_code='root-published-as-new',updated_at=?1 WHERE root_uuid=?2 AND phase NOT IN ('complete','cancelled')",params![root_now_millis(),source_root_uuid.as_bytes().as_slice()])?;
        transaction.execute("UPDATE sync_outbox SET state='archived' WHERE event_uuid IN (SELECT event_uuid FROM sync_events WHERE entity_uuid IN (SELECT asset_uuid FROM files WHERE root_id=?1) OR entity_uuid IN (SELECT folder_uuid FROM folders WHERE root_id=?1)) AND state IN ('pending','sealed')",[source_root_id])?;
        let old_actor_epoch:Vec<u8>=transaction.query_row("SELECT COALESCE(root_actor_epoch,local_binding_uuid) FROM sync_root_local_bindings WHERE root_uuid=?1",[source_root_uuid.as_bytes().as_slice()],|row|row.get(0))?;
        transaction.execute(
            "UPDATE sync_actor_epochs SET state='archived' WHERE actor_epoch=?1",
            [old_actor_epoch],
        )?;
        transaction.execute(
            "DELETE FROM sync_root_local_bindings WHERE root_uuid=?1",
            [source_root_uuid.as_bytes().as_slice()],
        )?;
        transaction.execute("UPDATE folders SET folder_uuid=?1 WHERE root_id=?2 AND parent_id IS NULL AND name='' AND folder_uuid=(SELECT root_folder_uuid FROM roots WHERE id=?2)",params![new_root_folder_uuid.as_bytes().as_slice(),source_root_id])?;
        transaction.execute(
            "UPDATE roots SET root_uuid=?1,root_folder_uuid=?2,display_name=?3 WHERE id=?4",
            params![
                new_root_uuid.as_bytes().as_slice(),
                new_root_folder_uuid.as_bytes().as_slice(),
                materialized_name,
                source_root_id
            ],
        )?;
        transaction.execute(
            "DELETE FROM sync_root_descriptors WHERE root_uuid=?1",
            [source_root_uuid.as_bytes().as_slice()],
        )?;
        let generation = 1i64;
        transaction.execute("INSERT INTO sync_root_local_bindings(root_uuid,library_uuid,local_binding_uuid,binding_generation,state,root_id,operation_revision,updated_at,root_capture_epoch,root_actor_epoch) VALUES (?1,?2,?3,?4,'active',?5,?6,?7,?8,?9)",params![new_root_uuid.as_bytes().as_slice(),library_uuid.as_bytes().as_slice(),local_binding_uuid.as_bytes().as_slice(),generation,source_root_id,i64::try_from(operation_revision)?,root_now_millis(),root_capture_epoch.as_bytes().as_slice(),root_actor_epoch.as_bytes().as_slice()])?;
        let (device, stream): (Vec<u8>, Vec<u8>) = transaction.query_row(
            "SELECT device_uuid,catalog_stream_uuid FROM sync_local_state WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        transaction.execute("INSERT INTO sync_actor_epochs(actor_epoch,device_uuid,stream_uuid,stream_kind,binding_epoch,state,next_actor_seq,published_through_seq,created_at) VALUES (?1,?2,?3,'catalog',?4,'active',1,0,?5) ON CONFLICT(actor_epoch) DO NOTHING",params![root_actor_epoch.as_bytes().as_slice(),device,stream,root_capture_epoch.as_bytes().as_slice(),root_now_millis()])?;
        crate::sync::capture::capture_catalog_mutation(
            &transaction,
            crate::sync::capture::MutationOrigin::LocalApi,
            crate::sync::types::EntityUuid(new_root_folder_uuid),
            crate::sync::types::Operation::SetField {
                field: "entity-kind".into(),
                value: serde_json::json!("folder"),
            },
        )?;
        commit_location_mutation_once(
            &transaction,
            derived_publish_uuid(operation_uuid, b"root-location"),
            new_root_folder_uuid,
            crate::sync::types::EntityUuid(Uuid::from_slice(&library_root_uuid)?),
            requested_name,
            &BTreeSet::new(),
        )?;
        let direct_children = {
            let sentinel_id: i64 = transaction.query_row(
                "SELECT id FROM folders WHERE root_id=?1 AND parent_id IS NULL AND name=''",
                [source_root_id],
                |row| row.get(0),
            )?;
            let mut children = Vec::new();
            let mut folders = transaction.prepare(
                "SELECT folder_uuid,name FROM folders WHERE parent_id=?1 ORDER BY folder_uuid",
            )?;
            for row in folders.query_map([sentinel_id], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
            })? {
                children.push(row?);
            }
            drop(folders);
            let mut files = transaction.prepare(
                "SELECT asset_uuid,name FROM files WHERE folder_id=?1 ORDER BY asset_uuid",
            )?;
            for row in files.query_map([sentinel_id], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
            })? {
                children.push(row?);
            }
            children
        };
        for (index, (entity, name)) in direct_children.into_iter().enumerate() {
            let entity = Uuid::from_slice(&entity)?;
            let heads = location_heads(&transaction, entity)?;
            let label = format!("child-location-{index}");
            commit_location_mutation_once(
                &transaction,
                derived_publish_uuid(operation_uuid, label.as_bytes()),
                entity,
                crate::sync::types::EntityUuid(new_root_folder_uuid),
                &name,
                &heads,
            )?;
        }
        // The prior fork actor is permanently sealed. Re-emit the complete
        // retained tree through the fresh root actor so publication of the
        // new identity never depends on reopening old queued work.
        let (state, _) = crate::sync::persistence::capture_protocol_state(&transaction)?;
        let snapshot = crate::sync_folders::snapshot_from_protocol_state(
            &state,
            new_root_uuid,
            serde_json::json!({}),
        )?;
        let mut publish_ordinal = 0u64;
        for (entity, fields) in &snapshot.entities {
            let entity_uuid = Uuid::parse_str(entity)?;
            for (field, value) in fields {
                if entity_uuid == new_root_folder_uuid
                    && matches!(field.as_str(), "field:entity-kind" | "location")
                {
                    continue;
                }
                if field.starts_with("root:") {
                    continue;
                }
                capture_selected_snapshot_value(
                    &transaction,
                    operation_uuid,
                    publish_ordinal,
                    b"publish-as-new",
                    entity_uuid,
                    field,
                    Some(value),
                    false,
                )?;
                publish_ordinal = publish_ordinal.saturating_add(1);
            }
        }
        transaction.execute("UPDATE sync_root_publish_cutovers SET new_root_id=?1,state='complete',updated_at=?2 WHERE operation_uuid=?3",params![source_root_id,root_now_millis(),operation_uuid.as_bytes().as_slice()])?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn sync_root_id(&self, root_uuid: &[u8; 16]) -> anyhow::Result<Option<i64>> {
        self.connection
            .query_row(
                "SELECT id FROM roots WHERE root_uuid=?1 AND root_folder_uuid IS NOT NULL",
                [root_uuid.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub(crate) fn sync_root_binding_token_for_root_id(
        &self,
        root_id: i64,
    ) -> anyhow::Result<Option<SyncRootBindingToken>> {
        self.connection
            .query_row(
                "SELECT root_uuid,local_binding_uuid,binding_generation FROM sync_root_local_bindings WHERE root_id=?1",
                [root_id],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?
            .map(|(root_uuid, local_binding_uuid, binding_generation)| {
                Ok(SyncRootBindingToken {
                    root_uuid: fixed_bytes(root_uuid, "root_uuid")?,
                    local_binding_uuid: fixed_bytes(local_binding_uuid, "local_binding_uuid")?,
                    binding_generation: u64::try_from(binding_generation)
                        .context("negative binding generation")?,
                })
            })
            .transpose()
    }

    pub(crate) fn sync_root_binding_token_for_file(
        &self,
        file_id: i64,
    ) -> anyhow::Result<Option<SyncRootBindingToken>> {
        let root_id = self
            .connection
            .query_row("SELECT root_id FROM files WHERE id=?1", [file_id], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?;
        root_id
            .map(|root_id| self.sync_root_binding_token_for_root_id(root_id))
            .transpose()
            .map(Option::flatten)
    }

    pub(crate) fn sync_root_binding_token_is_current(
        &self,
        token: &SyncRootBindingToken,
    ) -> anyhow::Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_root_local_bindings WHERE root_uuid=?1 AND local_binding_uuid=?2 AND binding_generation=?3 AND state IN ('materializing','active','local-only-fork','reconcile-required','relocating','local-rename-required'))",
                params![
                    token.root_uuid.as_slice(),
                    token.local_binding_uuid.as_slice(),
                    i64::try_from(token.binding_generation)
                        .context("binding generation exceeds SQLite integer range")?,
                ],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub(crate) fn sync_root_binding_token_is_watchable(
        &self,
        token: &SyncRootBindingToken,
    ) -> anyhow::Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_root_local_bindings WHERE root_uuid=?1 AND local_binding_uuid=?2 AND binding_generation=?3 AND state IN ('materializing','active','local-only-fork','reconcile-required'))",
                params![
                    token.root_uuid.as_slice(),
                    token.local_binding_uuid.as_slice(),
                    i64::try_from(token.binding_generation)
                        .context("binding generation exceeds SQLite integer range")?,
                ],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub(crate) fn require_sync_root_binding_token(
        &self,
        token: &SyncRootBindingToken,
    ) -> anyhow::Result<()> {
        if self.sync_root_binding_token_is_current(token)? {
            return Ok(());
        }
        bail!("stale root binding generation")
    }

    pub(crate) fn require_sync_root_binding_token_for_publication(
        &self,
        token: &SyncRootBindingToken,
    ) -> anyhow::Result<()> {
        let current = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_root_local_bindings WHERE root_uuid=?1 AND local_binding_uuid=?2 AND binding_generation=?3 AND state='active')",
            params![
                token.root_uuid.as_slice(),
                token.local_binding_uuid.as_slice(),
                i64::try_from(token.binding_generation)
                    .context("binding generation exceeds SQLite integer range")?,
            ],
            |row| row.get::<_, bool>(0),
        )?;
        if current {
            return Ok(());
        }
        bail!("stale root binding generation")
    }

    pub(crate) fn require_sync_root_binding_token_for_materialization(
        &self,
        token: &SyncRootBindingToken,
    ) -> anyhow::Result<()> {
        let current = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_root_local_bindings WHERE root_uuid=?1 AND local_binding_uuid=?2 AND binding_generation=?3 AND state IN ('materializing','active','reconcile-required'))",
            params![
                token.root_uuid.as_slice(),
                token.local_binding_uuid.as_slice(),
                i64::try_from(token.binding_generation)
                    .context("binding generation exceeds SQLite integer range")?,
            ],
            |row| row.get::<_, bool>(0),
        )?;
        if current {
            return Ok(());
        }
        bail!("stale root binding generation")
    }

    /// Applies the Host binding projection without allowing a stale revision
    /// or generation to roll the Catalog execution authority backward.
    #[allow(dead_code)]
    pub fn upsert_sync_root_local_binding(
        &self,
        binding: &SyncRootLocalBindingRecord,
    ) -> anyhow::Result<()> {
        if !is_local_binding_state(&binding.state) {
            bail!("invalid local root binding state");
        }
        if binding.binding_generation == 0 {
            bail!("binding generation must be positive");
        }
        let generation = i64::try_from(binding.binding_generation)
            .context("binding generation exceeds SQLite integer range")?;
        let operation_revision = i64::try_from(binding.operation_revision)
            .context("binding operation revision exceeds SQLite integer range")?;
        let changed = self.connection.execute(
            "INSERT INTO sync_root_local_bindings(root_uuid,library_uuid,local_binding_uuid,binding_generation,state,root_id,operation_revision,updated_at,root_capture_epoch,root_actor_epoch) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(root_uuid) DO UPDATE SET local_binding_uuid=excluded.local_binding_uuid,binding_generation=excluded.binding_generation,state=excluded.state,operation_revision=excluded.operation_revision,updated_at=excluded.updated_at,root_capture_epoch=excluded.root_capture_epoch,root_actor_epoch=excluded.root_actor_epoch WHERE sync_root_local_bindings.library_uuid=excluded.library_uuid AND sync_root_local_bindings.root_id=excluded.root_id AND (excluded.binding_generation>sync_root_local_bindings.binding_generation OR (excluded.binding_generation=sync_root_local_bindings.binding_generation AND excluded.operation_revision>=sync_root_local_bindings.operation_revision))",
            params![
                binding.root_uuid.as_slice(),
                binding.library_uuid.as_slice(),
                binding.local_binding_uuid.as_slice(),
                generation,
                binding.state,
                binding.root_id,
                operation_revision,
                binding.updated_at,
                binding.root_capture_epoch.as_slice(),
                binding.root_actor_epoch.as_slice(),
            ],
        )?;
        let actor_context: Option<(Vec<u8>, Vec<u8>)> = self
            .connection
            .query_row(
                "SELECT device_uuid,catalog_stream_uuid FROM sync_local_state WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((device, stream)) = actor_context {
            let actor_state = if binding.state == "local-only-fork" {
                "forked"
            } else {
                "active"
            };
            self.connection.execute(
                "INSERT INTO sync_actor_epochs(actor_epoch,device_uuid,stream_uuid,stream_kind,binding_epoch,state,next_actor_seq,published_through_seq,created_at) VALUES (?1,?2,?3,'catalog',?4,?5,1,0,?6) ON CONFLICT(actor_epoch) DO UPDATE SET state=excluded.state,binding_epoch=excluded.binding_epoch",
                params![binding.root_actor_epoch.as_slice(),device,stream,binding.root_capture_epoch.as_slice(),actor_state,root_now_millis()],
            )?;
        }
        if changed == 0 {
            let current = self.sync_root_local_binding(&binding.root_uuid)?;
            if current.as_ref() == Some(binding) {
                return Ok(());
            }
            bail!("stale local root binding projection");
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn sync_root_local_binding(
        &self,
        root_uuid: &[u8; 16],
    ) -> anyhow::Result<Option<SyncRootLocalBindingRecord>> {
        self.connection
            .query_row(
                "SELECT root_uuid,library_uuid,local_binding_uuid,binding_generation,state,root_id,operation_revision,updated_at,COALESCE(root_capture_epoch,local_binding_uuid),COALESCE(root_actor_epoch,local_binding_uuid) FROM sync_root_local_bindings WHERE root_uuid=?1",
                [root_uuid.as_slice()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, Vec<u8>>(8)?,
                        row.get::<_, Vec<u8>>(9)?,
                    ))
                },
            )
            .optional()?
            .map(
                |(
                    root_uuid,
                    library_uuid,
                    local_binding_uuid,
                    binding_generation,
                    state,
                    root_id,
                    operation_revision,
                    updated_at,
                    root_capture_epoch,
                    root_actor_epoch,
                )| {
                    Ok(SyncRootLocalBindingRecord {
                        root_uuid: fixed_bytes(root_uuid, "root_uuid")?,
                        library_uuid: fixed_bytes(library_uuid, "library_uuid")?,
                        local_binding_uuid: fixed_bytes(
                            local_binding_uuid,
                            "local_binding_uuid",
                        )?,
                        binding_generation: u64::try_from(binding_generation)
                            .context("negative binding generation")?,
                        state,
                        root_id,
                        operation_revision: u64::try_from(operation_revision)
                            .context("negative binding operation revision")?,
                        updated_at,
                        root_capture_epoch: fixed_bytes(root_capture_epoch, "root_capture_epoch")?,
                        root_actor_epoch: fixed_bytes(root_actor_epoch, "root_actor_epoch")?,
                    })
                },
            )
            .transpose()
    }

    /// Validates a captured execution token through caller-owned transaction
    /// state. Workers call this immediately before their claim/effect/complete
    /// write so a Host read cannot become the final authority.
    #[allow(dead_code)]
    pub fn sync_root_binding_token_matches_in_transaction(
        transaction: &rusqlite::Transaction<'_>,
        token: &SyncRootBindingToken,
    ) -> anyhow::Result<bool> {
        transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_root_local_bindings WHERE root_uuid=?1 AND local_binding_uuid=?2 AND binding_generation=?3 AND state IN ('materializing','active','local-only-fork','reconcile-required','relocating','local-rename-required'))",
                params![
                    token.root_uuid.as_slice(),
                    token.local_binding_uuid.as_slice(),
                    i64::try_from(token.binding_generation)
                        .context("binding generation exceeds SQLite integer range")?,
                ],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub(crate) fn require_sync_root_binding_token_in_transaction(
        transaction: &rusqlite::Transaction<'_>,
        token: &SyncRootBindingToken,
    ) -> anyhow::Result<()> {
        if Self::sync_root_binding_token_matches_in_transaction(transaction, token)? {
            return Ok(());
        }
        bail!("stale root binding generation")
    }

    /// Upserts reducer-derived descriptor fields while preserving the immutable
    /// portable identity tuple. Paths and device-local selection never enter
    /// this Catalog projection.
    #[allow(dead_code)]
    pub fn upsert_sync_root_descriptor(
        &self,
        descriptor: &SyncRootDescriptorRecord,
    ) -> anyhow::Result<()> {
        if descriptor.requested_name.is_empty() || descriptor.materialized_name.is_empty() {
            bail!("sync root names cannot be empty");
        }
        if !matches!(descriptor.lifecycle.as_str(), "active" | "tombstoned") {
            bail!("invalid sync root lifecycle");
        }
        let logical_revision = i64::try_from(descriptor.logical_revision)
            .context("logical revision exceeds SQLite integer range")?;
        let changed = self.connection.execute(
            "INSERT INTO sync_root_descriptors(root_uuid,root_folder_uuid,library_root_uuid,creation_event_uuid,winning_location_revision_uuid,requested_name,materialized_name,materialization_fingerprint,lifecycle,logical_revision,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(root_uuid) DO UPDATE SET creation_event_uuid=excluded.creation_event_uuid,winning_location_revision_uuid=excluded.winning_location_revision_uuid,requested_name=excluded.requested_name,materialized_name=excluded.materialized_name,materialization_fingerprint=excluded.materialization_fingerprint,lifecycle=excluded.lifecycle,logical_revision=excluded.logical_revision,updated_at=excluded.updated_at WHERE sync_root_descriptors.root_folder_uuid=excluded.root_folder_uuid AND sync_root_descriptors.library_root_uuid=excluded.library_root_uuid",
            params![
                descriptor.root_uuid.as_slice(),
                descriptor.root_folder_uuid.as_slice(),
                descriptor.library_root_uuid.as_slice(),
                descriptor.creation_event_uuid.as_ref().map(|value| value.as_slice()),
                descriptor
                    .winning_location_revision_uuid
                    .as_ref()
                    .map(|value| value.as_slice()),
                descriptor.requested_name,
                descriptor.materialized_name,
                descriptor.materialization_fingerprint.as_slice(),
                descriptor.lifecycle,
                logical_revision,
                descriptor.updated_at,
            ],
        )?;
        if changed != 1 {
            bail!("sync root identity does not match the existing descriptor");
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn sync_root_descriptor(
        &self,
        root_uuid: &[u8; 16],
    ) -> anyhow::Result<Option<SyncRootDescriptorRecord>> {
        self.connection
            .query_row(
                "SELECT root_uuid,root_folder_uuid,library_root_uuid,creation_event_uuid,winning_location_revision_uuid,requested_name,materialized_name,materialization_fingerprint,lifecycle,logical_revision,updated_at FROM sync_root_descriptors WHERE root_uuid=?1",
                [root_uuid.as_slice()],
                |row| {
                    let root_uuid: Vec<u8> = row.get(0)?;
                    let root_folder_uuid: Vec<u8> = row.get(1)?;
                    let library_root_uuid: Vec<u8> = row.get(2)?;
                    let creation_event_uuid: Option<Vec<u8>> = row.get(3)?;
                    let winning_location_revision_uuid: Option<Vec<u8>> = row.get(4)?;
                    let materialization_fingerprint: Vec<u8> = row.get(7)?;
                    let logical_revision: i64 = row.get(9)?;
                    Ok((
                        root_uuid,
                        root_folder_uuid,
                        library_root_uuid,
                        creation_event_uuid,
                        winning_location_revision_uuid,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        materialization_fingerprint,
                        row.get::<_, String>(8)?,
                        logical_revision,
                        row.get::<_, i64>(10)?,
                    ))
                },
            )
            .optional()?
            .map(
                |(
                    root_uuid,
                    root_folder_uuid,
                    library_root_uuid,
                    creation_event_uuid,
                    winning_location_revision_uuid,
                    requested_name,
                    materialized_name,
                    materialization_fingerprint,
                    lifecycle,
                    logical_revision,
                    updated_at,
                )| {
                    Ok(SyncRootDescriptorRecord {
                        root_uuid: fixed_bytes(root_uuid, "root_uuid")?,
                        root_folder_uuid: fixed_bytes(root_folder_uuid, "root_folder_uuid")?,
                        library_root_uuid: fixed_bytes(library_root_uuid, "library_root_uuid")?,
                        creation_event_uuid: creation_event_uuid
                            .map(|value| fixed_bytes(value, "creation_event_uuid"))
                            .transpose()?,
                        winning_location_revision_uuid: winning_location_revision_uuid
                            .map(|value| fixed_bytes(value, "winning_location_revision_uuid"))
                            .transpose()?,
                        requested_name,
                        materialized_name,
                        materialization_fingerprint: fixed_bytes(
                            materialization_fingerprint,
                            "materialization_fingerprint",
                        )?,
                        lifecycle,
                        logical_revision: u64::try_from(logical_revision)
                            .context("negative logical revision")?,
                        updated_at,
                    })
                },
            )
            .transpose()
    }

    pub fn sync_root_descriptors(&self) -> anyhow::Result<Vec<SyncRootDescriptorRecord>> {
        let mut statement = self
            .connection
            .prepare("SELECT root_uuid FROM sync_root_descriptors ORDER BY root_uuid")?;
        let roots = statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        roots
            .into_iter()
            .map(|root| {
                let root: [u8; 16] = fixed_bytes(root, "root_uuid")?;
                self.sync_root_descriptor(&root)?
                    .context("sync root descriptor disappeared")
            })
            .collect()
    }

    /// Links an already-created Catalog root to its existing empty-name
    /// sentinel. The migration trigger rejects any mismatched descriptor or
    /// sentinel UUID and makes the link immutable after this point.
    #[allow(dead_code)]
    pub fn link_sync_root_descriptor(
        &self,
        root_id: i64,
        root_uuid: &[u8; 16],
        root_folder_uuid: &[u8; 16],
    ) -> anyhow::Result<()> {
        let changed = self.connection.execute(
            "UPDATE roots SET root_uuid=?1,root_folder_uuid=?2 WHERE id=?3 AND root_folder_uuid IS NULL",
            params![root_uuid.as_slice(), root_folder_uuid.as_slice(), root_id],
        )?;
        if changed != 1 {
            bail!("sync root link missing or already bound");
        }
        Ok(())
    }

    /// Appends one root/folder logical rename using the operation UUID as its
    /// stable revision identity. Exact retries return the already-created
    /// event; stale head sets fail before any ledger or projection mutation.
    #[allow(dead_code)]
    pub fn commit_location_rename_once(
        &self,
        operation_uuid: Uuid,
        entity_uuid: Uuid,
        requested_name: &str,
        expected_heads: &BTreeSet<Uuid>,
        supersedes: &BTreeSet<Uuid>,
    ) -> anyhow::Result<Uuid> {
        if requested_name.is_empty() || expected_heads.is_empty() || supersedes != expected_heads {
            bail!("invalid-location-rename");
        }
        let transaction = self.connection.unchecked_transaction()?;
        let parent_uuid = current_location_parent(&transaction, entity_uuid)?;
        let event_uuid = commit_location_mutation_once(
            &transaction,
            operation_uuid,
            entity_uuid,
            parent_uuid,
            requested_name,
            expected_heads,
        )?;
        transaction.commit()?;
        Ok(event_uuid)
    }

    /// Resolves every currently-live location head with one explicit
    /// superseding revision. Omitting the requested name keeps the display
    /// winner's logical name; exact retries are idempotent.
    #[allow(dead_code)]
    pub fn resolve_location_conflict_once(
        &self,
        operation_uuid: Uuid,
        entity_uuid: Uuid,
        requested_name: Option<&str>,
        expected_heads: &BTreeSet<Uuid>,
        supersedes: &BTreeSet<Uuid>,
    ) -> anyhow::Result<Uuid> {
        if expected_heads.is_empty() || supersedes != expected_heads {
            bail!("stale-location-heads");
        }
        let transaction = self.connection.unchecked_transaction()?;
        let (parent_uuid, winning_name) = current_winning_location(&transaction, entity_uuid)?;
        let requested_name = requested_name.unwrap_or(&winning_name);
        if requested_name.is_empty() {
            bail!("invalid-location-rename");
        }
        let event_uuid = commit_location_mutation_once(
            &transaction,
            operation_uuid,
            entity_uuid,
            parent_uuid,
            requested_name,
            expected_heads,
        )?;
        transaction.commit()?;
        Ok(event_uuid)
    }
}

fn is_local_binding_state(state: &str) -> bool {
    matches!(
        state,
        "materializing"
            | "active"
            | "paused"
            | "local-only-fork"
            | "reconcile-required"
            | "relocating"
            | "local-rename-required"
            | "unavailable"
            | "error"
    )
}

fn ensure_reconciliation_entity(
    transaction: &rusqlite::Transaction<'_>,
    root_uuid: Uuid,
    entity_uuid: Uuid,
    selected_entity: Option<&str>,
) -> anyhow::Result<()> {
    let exists:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM files WHERE asset_uuid=?1) OR EXISTS(SELECT 1 FROM folders WHERE folder_uuid=?1) OR EXISTS(SELECT 1 FROM sync_root_descriptors WHERE root_folder_uuid=?1)",[entity_uuid.as_bytes().as_slice()],|row|row.get(0))?;
    if exists {
        return Ok(());
    }
    let Some(selected_entity) = selected_entity else {
        return Ok(());
    };
    let fields: BTreeMap<String, String> = serde_json::from_str(selected_entity)?;
    let kind = fields
        .get("field:entity-kind")
        .map(|value| serde_json::from_str::<String>(value))
        .transpose()?
        .unwrap_or_default();
    let location = fields
        .get("location")
        .map(|value| serde_json::from_str::<serde_json::Value>(value))
        .transpose()?
        .context("reconciliation location missing")?;
    let name = location
        .get("name")
        .and_then(serde_json::Value::as_str)
        .context("reconciliation location name missing")?;
    let parent = location
        .get("parentUuid")
        .and_then(serde_json::Value::as_str)
        .context("reconciliation location parent missing")?;
    let parent = Uuid::parse_str(parent)?;
    let root_id: i64 = transaction.query_row(
        "SELECT id FROM roots WHERE root_uuid=?1",
        [root_uuid.as_bytes().as_slice()],
        |row| row.get(0),
    )?;
    let parent_id: i64 = transaction
        .query_row(
            "SELECT id FROM folders WHERE folder_uuid=?1 AND root_id=?2",
            params![parent.as_bytes().as_slice(), root_id],
            |row| row.get(0),
        )
        .context("reconciliation parent missing")?;
    match kind.as_str() {
        "folder" => {
            transaction.execute("INSERT INTO folders(root_id,parent_id,name,path_hash,mtime,status,folder_uuid) VALUES (?1,?2,?3,'',NULL,'active',?4)",params![root_id,parent_id,name,entity_uuid.as_bytes().as_slice()])?;
        }
        "asset" => {
            let extension = fields
                .get("field:extension")
                .map(|value| serde_json::from_str::<String>(value))
                .transpose()?
                .unwrap_or_default();
            let content = fields
                .get("content")
                .map(|value| serde_json::from_str::<serde_json::Value>(value))
                .transpose()?;
            let size = content
                .as_ref()
                .and_then(|value| value.get("byteLength"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let mtime = content
                .as_ref()
                .and_then(|value| value.get("sourceMtimeMs"))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            transaction.execute("INSERT INTO files(root_id,folder_id,name,ext,size,mtime,status,asset_uuid) VALUES (?1,?2,?3,?4,?5,?6,'active',?7)",params![root_id,parent_id,name,extension,i64::try_from(size)?,mtime,entity_uuid.as_bytes().as_slice()])?;
            if let Some(content) = content {
                if let Some(revision) = content
                    .get("revisionUuid")
                    .and_then(serde_json::Value::as_str)
                {
                    let revision = Uuid::parse_str(revision)?;
                    let manifest = content
                        .get("manifestHash")
                        .and_then(serde_json::Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(crate::sync::media::parse_hash)
                        .transpose()?;
                    let generation:i64=transaction.query_row("SELECT next_local_mutation_generation FROM sync_local_state WHERE singleton=1",[],|row|row.get(0))?;
                    transaction.execute("INSERT OR IGNORE INTO asset_content_revisions(content_revision_uuid,asset_uuid,manifest_hash,byte_size,local_mutation_generation,created_at) VALUES (?1,?2,?3,?4,?5,?6)",params![revision.as_bytes().as_slice(),entity_uuid.as_bytes().as_slice(),manifest.as_ref().map(|value|value.as_slice()),i64::try_from(size)?,generation.max(1),root_now_millis()])?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn apply_selected_snapshot_value(
    transaction: &rusqlite::Transaction<'_>,
    entity_uuid: Uuid,
    field_name: &str,
    selected_value: Option<&str>,
) -> anyhow::Result<()> {
    let entity = entity_uuid.as_bytes().as_slice();
    match field_name {
        "existence" => {
            let status = if selected_value == Some("deleted") {
                "trashed"
            } else {
                "active"
            };
            transaction.execute(
                "UPDATE files SET status=?1 WHERE asset_uuid=?2",
                params![status, entity],
            )?;
            transaction.execute(
                "UPDATE folders SET status=?1 WHERE folder_uuid=?2 AND parent_id IS NOT NULL",
                params![status, entity],
            )?;
            transaction.execute("UPDATE sync_root_descriptors SET lifecycle=CASE WHEN ?1='trashed' THEN 'tombstoned' ELSE 'active' END,updated_at=?2 WHERE root_folder_uuid=?3",params![status,root_now_millis(),entity])?;
        }
        "location" => {
            if let Some(value) = selected_value {
                let location: serde_json::Value = serde_json::from_str(value)?;
                let name = location
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .context("reconciliation location name missing")?;
                let parent = location
                    .get("parentUuid")
                    .and_then(serde_json::Value::as_str)
                    .map(Uuid::parse_str)
                    .transpose()?;
                if transaction.execute("UPDATE files SET name=?1,folder_id=(SELECT id FROM folders WHERE folder_uuid=?2) WHERE asset_uuid=?3",params![name,parent.map(|v|v.as_bytes().to_vec()),entity])?==0 {
                transaction.execute("UPDATE folders SET name=?1,parent_id=(SELECT id FROM folders WHERE folder_uuid=?2) WHERE folder_uuid=?3 AND parent_id IS NOT NULL",params![name,parent.map(|v|v.as_bytes().to_vec()),entity])?;
            }
                transaction.execute("UPDATE sync_root_descriptors SET requested_name=?1,materialized_name=?1,materialization_fingerprint=?2,updated_at=?3 WHERE root_folder_uuid=?4",params![name,crate::sync::media::parse_hash(&crate::sync_folders::materialization_fingerprint(name))?.as_slice(),root_now_millis(),entity])?;
                transaction.execute(
                    "UPDATE roots SET display_name=?1 WHERE root_folder_uuid=?2",
                    params![name, entity],
                )?;
            }
        }
        "content" => {
            if let Some(value) = selected_value {
                let content: serde_json::Value = serde_json::from_str(value)?;
                let revision = Uuid::parse_str(
                    content
                        .get("revisionUuid")
                        .and_then(serde_json::Value::as_str)
                        .context("reconciliation content revision missing")?,
                )?;
                transaction.execute(
                    "UPDATE asset_content_heads SET is_display_winner=0 WHERE asset_uuid=?1",
                    [entity],
                )?;
                transaction.execute("INSERT INTO asset_content_heads(asset_uuid,content_revision_uuid,is_display_winner) SELECT ?1,?2,1 WHERE EXISTS(SELECT 1 FROM asset_content_revisions WHERE asset_uuid=?1 AND content_revision_uuid=?2) ON CONFLICT(asset_uuid,content_revision_uuid) DO UPDATE SET is_display_winner=1",params![entity,revision.as_bytes().as_slice()])?;
            }
        }
        field if field.starts_with("field:") => {
            if let Some(value) = selected_value {
                let field = &field[6..];
                let value: serde_json::Value = serde_json::from_str(value)?;
                let file_id: Option<i64> = transaction
                    .query_row(
                        "SELECT id FROM files WHERE asset_uuid=?1",
                        [entity],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(file_id) = file_id {
                    match field {
                        "rating" => {
                            transaction.execute("INSERT INTO user_metadata(file_id,rating,favorite,note,updated_at) VALUES (?1,?2,0,NULL,?3) ON CONFLICT(file_id) DO UPDATE SET rating=excluded.rating,updated_at=excluded.updated_at",params![file_id,value.as_i64(),root_now_millis()/1000])?;
                        }
                        "favorite" => {
                            transaction.execute("INSERT INTO user_metadata(file_id,rating,favorite,note,updated_at) VALUES (?1,NULL,?2,NULL,?3) ON CONFLICT(file_id) DO UPDATE SET favorite=excluded.favorite,updated_at=excluded.updated_at",params![file_id,value.as_bool().unwrap_or(false) as i64,root_now_millis()/1000])?;
                        }
                        "note" => {
                            transaction.execute("INSERT INTO user_metadata(file_id,rating,favorite,note,updated_at) VALUES (?1,NULL,0,?2,?3) ON CONFLICT(file_id) DO UPDATE SET note=excluded.note,updated_at=excluded.updated_at",params![file_id,value.as_str(),root_now_millis()/1000])?;
                        }
                        "extension" => {
                            transaction.execute(
                                "UPDATE files SET ext=?1 WHERE id=?2",
                                params![value.as_str(), file_id],
                            )?;
                        }
                        _ => {}
                    }
                } else if matches!(field, "name" | "color") {
                    transaction.execute(
                        &format!("UPDATE tags SET {field}=?1 WHERE tag_uuid=?2"),
                        params![value.as_str(), entity],
                    )?;
                }
            }
        }
        field if field.starts_with("set:") => {
            if field == "set:tags" {
                let members: Vec<String> = selected_value
                    .map(serde_json::from_str)
                    .transpose()?
                    .unwrap_or_default();
                if let Some(file_id) = transaction
                    .query_row(
                        "SELECT id FROM files WHERE asset_uuid=?1",
                        [entity],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                {
                    transaction.execute("DELETE FROM file_tags WHERE file_id=?1", [file_id])?;
                    for member in members {
                        let member = Uuid::parse_str(&member)?;
                        transaction.execute("INSERT OR IGNORE INTO file_tags(file_id,tag_id) SELECT ?1,id FROM tags WHERE tag_uuid=?2",params![file_id,member.as_bytes().as_slice()])?;
                    }
                }
            }
        }
        "root:requested-name" => {
            if let Some(value) = selected_value {
                transaction.execute("UPDATE sync_root_descriptors SET requested_name=?1,updated_at=?2 WHERE root_folder_uuid=?3",params![value,root_now_millis(),entity])?;
            }
        }
        "root:materialized-name" => {
            if let Some(value) = selected_value {
                transaction.execute("UPDATE sync_root_descriptors SET materialized_name=?1,updated_at=?2 WHERE root_folder_uuid=?3",params![value,root_now_millis(),entity])?;
                transaction.execute(
                    "UPDATE roots SET display_name=?1 WHERE root_folder_uuid=?2",
                    params![value, entity],
                )?;
            }
        }
        "root:materialization-fingerprint" => {
            if let Some(value) = selected_value {
                transaction.execute("UPDATE sync_root_descriptors SET materialization_fingerprint=?1,updated_at=?2 WHERE root_folder_uuid=?3",params![crate::sync::media::parse_hash(value)?.as_slice(),root_now_millis(),entity])?;
            }
        }
        "root:lifecycle" => {
            if let Some(value) = selected_value {
                transaction.execute("UPDATE sync_root_descriptors SET lifecycle=?1,updated_at=?2 WHERE root_folder_uuid=?3",params![value,root_now_millis(),entity])?;
            }
        }
        "root:logical-revision" | "root:winning-location-revision" => {}
        _ => {}
    }
    Ok(())
}

fn capture_selected_snapshot_value(
    transaction: &rusqlite::Transaction<'_>,
    plan_uuid: Uuid,
    item_ordinal: u64,
    variant: &[u8],
    entity_uuid: Uuid,
    field_name: &str,
    selected_value: Option<&str>,
    preserve_both: bool,
) -> anyhow::Result<()> {
    use crate::sync::capture::{capture_catalog_mutation, observed_tag_adds, MutationOrigin};
    use crate::sync::types::{EntityUuid, EventUuid, Operation, RevisionUuid};

    let entity = EntityUuid(entity_uuid);
    let capture = |operation| {
        capture_catalog_mutation(transaction, MutationOrigin::LocalApi, entity, operation)
            .map(|_| ())
    };
    match field_name {
        "existence" => match selected_value {
            Some("deleted") => capture(Operation::Delete {
                token: EventUuid(derived_reconciliation_uuid(
                    plan_uuid,
                    item_ordinal,
                    &[b"delete-token".as_slice(), variant].concat(),
                )),
            })?,
            Some("live") => {
                let mut statement = transaction.prepare(
                    "SELECT token_uuid FROM sync_tombstone_tokens token WHERE token.entity_uuid=?1 AND NOT EXISTS(SELECT 1 FROM sync_tombstone_removals removal WHERE removal.observed_token_uuid=token.token_uuid) ORDER BY token_uuid",
                )?;
                let observed_tokens = statement
                    .query_map([entity_uuid.as_bytes().as_slice()], |row| {
                        row.get::<_, Vec<u8>>(0)
                    })?
                    .map(|row| Ok(EventUuid(Uuid::from_slice(&row?)?)))
                    .collect::<anyhow::Result<BTreeSet<_>>>()?;
                drop(statement);
                if !observed_tokens.is_empty() {
                    capture(Operation::Restore { observed_tokens })?;
                }
            }
            None => {}
            Some(_) => bail!("invalid reconciliation existence value"),
        },
        "location" => {
            if let Some(value) = selected_value {
                let location: serde_json::Value = serde_json::from_str(value)?;
                let parent_uuid = location
                    .get("parentUuid")
                    .and_then(serde_json::Value::as_str)
                    .map(Uuid::parse_str)
                    .transpose()?
                    .map(EntityUuid);
                let name = location
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .context("reconciliation location name missing")?
                    .to_owned();
                let supersedes = if preserve_both {
                    BTreeSet::new()
                } else {
                    location_heads(transaction, entity_uuid)?
                        .into_iter()
                        .map(RevisionUuid)
                        .collect()
                };
                capture(Operation::SetLocation {
                    revision_uuid: RevisionUuid(derived_reconciliation_uuid(
                        plan_uuid,
                        item_ordinal,
                        &[b"location-revision".as_slice(), variant].concat(),
                    )),
                    parent_uuid,
                    name,
                    supersedes,
                })?;
            }
        }
        "content" => {
            if let Some(value) = selected_value {
                let content: serde_json::Value = serde_json::from_str(value)?;
                let manifest_id = content
                    .get("manifestId")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| {
                        content
                            .get("manifestHash")
                            .and_then(serde_json::Value::as_str)
                    })
                    .context("reconciliation content manifest missing")?
                    .to_owned();
                let supersedes = if preserve_both {
                    BTreeSet::new()
                } else {
                    content_heads(transaction, entity_uuid)?
                        .into_iter()
                        .map(RevisionUuid)
                        .collect()
                };
                capture(Operation::SetContent {
                    revision_uuid: RevisionUuid(derived_reconciliation_uuid(
                        plan_uuid,
                        item_ordinal,
                        &[b"content-revision".as_slice(), variant].concat(),
                    )),
                    manifest_id,
                    supersedes,
                    manifest_pack_id: content
                        .get("manifestPackId")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    manifest_offset: content
                        .get("manifestOffset")
                        .and_then(serde_json::Value::as_u64),
                    manifest_length: content
                        .get("manifestLength")
                        .and_then(serde_json::Value::as_u64),
                    byte_length: content
                        .get("byteLength")
                        .and_then(serde_json::Value::as_u64),
                    source_mtime_ms: content
                        .get("sourceMtimeMs")
                        .and_then(serde_json::Value::as_i64),
                })?;
            }
        }
        field if field.starts_with("field:") => {
            capture(Operation::SetField {
                field: field[6..].to_owned(),
                value: selected_value
                    .map(serde_json::from_str)
                    .transpose()?
                    .unwrap_or(serde_json::Value::Null),
            })?;
        }
        "set:tags" => {
            let selected = selected_value
                .map(serde_json::from_str::<BTreeSet<String>>)
                .transpose()?
                .unwrap_or_default()
                .into_iter()
                .map(|value| Ok(EntityUuid(Uuid::parse_str(&value)?)))
                .collect::<anyhow::Result<BTreeSet<_>>>()?;
            let current = current_tag_members(transaction, entity_uuid)?;
            for member in current.difference(&selected) {
                let observed_adds = observed_tag_adds(transaction, entity, *member)?;
                if !observed_adds.is_empty() {
                    capture(Operation::SetRemove {
                        set: "tags".to_owned(),
                        member_uuid: *member,
                        observed_adds,
                    })?;
                }
            }
            // Re-add every selected member under the fresh actor epoch. This
            // republishes local-fork-only membership without reopening the
            // sealed fork actor, while remaining an idempotent OR-set add.
            for member in selected {
                capture(Operation::SetAdd {
                    set: "tags".to_owned(),
                    member_uuid: member,
                })?;
            }
        }
        field if field.starts_with("root:") => {
            // Root descriptor fields are derived from portable root location,
            // existence and descriptor events. Keep the local projection
            // aligned for redundant comparison rows; the corresponding
            // portable field work above remains the synchronization truth.
            apply_selected_snapshot_value(transaction, entity_uuid, field, selected_value)?;
        }
        _ => apply_selected_snapshot_value(transaction, entity_uuid, field_name, selected_value)?,
    }
    Ok(())
}

fn derived_reconciliation_uuid(plan_uuid: Uuid, item_ordinal: u64, label: &[u8]) -> Uuid {
    let mut input = Vec::with_capacity(16 + 8 + label.len());
    input.extend_from_slice(plan_uuid.as_bytes());
    input.extend_from_slice(&item_ordinal.to_be_bytes());
    input.extend_from_slice(label);
    let mut bytes = crate::sync::codec::sha256(&input);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes[..16].try_into().expect("sha256 prefix"))
}

fn content_heads(
    transaction: &rusqlite::Transaction<'_>,
    entity_uuid: Uuid,
) -> anyhow::Result<BTreeSet<Uuid>> {
    let mut statement = transaction.prepare(
        "SELECT content_revision_uuid FROM asset_content_heads WHERE asset_uuid=?1 ORDER BY content_revision_uuid",
    )?;
    let heads = statement
        .query_map([entity_uuid.as_bytes().as_slice()], |row| {
            row.get::<_, Vec<u8>>(0)
        })?
        .map(|row| Ok(Uuid::from_slice(&row?)?))
        .collect();
    heads
}

fn current_tag_members(
    transaction: &rusqlite::Transaction<'_>,
    entity_uuid: Uuid,
) -> anyhow::Result<BTreeSet<crate::sync::types::EntityUuid>> {
    let mut statement = transaction.prepare(
        "SELECT DISTINCT addition.member_uuid FROM sync_set_adds addition WHERE addition.owner_uuid=?1 AND NOT EXISTS(SELECT 1 FROM sync_set_removals removal WHERE removal.observed_add_event_uuid=addition.add_event_uuid) ORDER BY addition.member_uuid",
    )?;
    let members = statement
        .query_map([entity_uuid.as_bytes().as_slice()], |row| {
            row.get::<_, Vec<u8>>(0)
        })?
        .map(|row| Ok(crate::sync::types::EntityUuid(Uuid::from_slice(&row?)?)))
        .collect();
    members
}

fn preserve_both_reconciliation_versions(
    transaction: &rusqlite::Transaction<'_>,
    entity_uuid: Uuid,
    field_name: &str,
    selected_value: Option<&str>,
) -> anyhow::Result<()> {
    transaction.execute("INSERT OR IGNORE INTO sync_identity_conflicts(conflict_uuid,old_asset_uuid,new_asset_uuid,reason,state,created_at) VALUES (?1,?2,?2,?3,'resolved',?4)",params![Uuid::now_v7().as_bytes().as_slice(),entity_uuid.as_bytes().as_slice(),format!("re-enable-keep-both:{field_name}:{}",selected_value.unwrap_or("none")),root_now_millis()])?;
    Ok(())
}

fn derived_publish_uuid(operation_uuid: Uuid, label: &[u8]) -> Uuid {
    let mut bytes = crate::sync::codec::sha256(&[operation_uuid.as_bytes(), label].concat());
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes[..16].try_into().expect("sha256 prefix"))
}

fn commit_location_mutation_once(
    transaction: &rusqlite::Transaction<'_>,
    operation_uuid: Uuid,
    entity_uuid: Uuid,
    parent_uuid: crate::sync::types::EntityUuid,
    requested_name: &str,
    expected_heads: &BTreeSet<Uuid>,
) -> anyhow::Result<Uuid> {
    let existing: Option<(Vec<u8>, Vec<u8>, Option<Vec<u8>>, String)> = transaction
            .query_row(
                "SELECT event_uuid,entity_uuid,parent_uuid,name FROM sync_location_revisions WHERE revision_uuid=?1",
                [operation_uuid.as_bytes().as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
    if let Some((event, stored_entity, _parent, stored_name)) = existing {
        let stored_supersedes = location_supersedes(&transaction, operation_uuid)?;
        if stored_entity != entity_uuid.as_bytes()
            || stored_name != requested_name
            || stored_supersedes != *expected_heads
        {
            bail!("idempotency-key-reused");
        }
        return Ok(Uuid::from_slice(&event)?);
    }
    let live_heads = location_heads(&transaction, entity_uuid)?;
    if live_heads != *expected_heads {
        bail!("stale-location-heads");
    }
    let capture = crate::sync::capture::capture_catalog_mutation(
        transaction,
        crate::sync::capture::MutationOrigin::LocalApi,
        crate::sync::types::EntityUuid(entity_uuid),
        crate::sync::types::Operation::SetLocation {
            revision_uuid: crate::sync::types::RevisionUuid(operation_uuid),
            parent_uuid: Some(parent_uuid),
            name: requested_name.to_owned(),
            supersedes: expected_heads
                .iter()
                .copied()
                .map(crate::sync::types::RevisionUuid)
                .collect(),
        },
    )?;
    let event_uuid = capture
        .event_uuid
        .context("logical rename did not emit an event")?;
    Ok(Uuid::from_slice(&event_uuid)?)
}

fn current_location_parent(
    transaction: &rusqlite::Transaction<'_>,
    entity_uuid: Uuid,
) -> anyhow::Result<crate::sync::types::EntityUuid> {
    Ok(current_winning_location(transaction, entity_uuid)?.0)
}

fn current_winning_location(
    transaction: &rusqlite::Transaction<'_>,
    entity_uuid: Uuid,
) -> anyhow::Result<(crate::sync::types::EntityUuid, String)> {
    let root_parent: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT library_root_uuid FROM sync_root_descriptors WHERE root_folder_uuid=?1",
            [entity_uuid.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(parent) = root_parent {
        let name: String = transaction.query_row(
            "SELECT r.name FROM sync_location_heads h JOIN sync_location_revisions r ON r.revision_uuid=h.revision_uuid WHERE h.entity_uuid=?1 AND h.is_display_winner=1",
            [entity_uuid.as_bytes().as_slice()],
            |row| row.get(0),
        )?;
        return Ok((
            crate::sync::types::EntityUuid(Uuid::from_slice(&parent)?),
            name,
        ));
    }
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM folders WHERE folder_uuid=?1 AND parent_id IS NOT NULL)",
        [entity_uuid.as_bytes().as_slice()],
        |row| row.get(0),
    )?;
    if !exists {
        bail!("folder-not-found");
    }
    let (parent, name): (Vec<u8>, String) = transaction.query_row(
        "SELECT r.parent_uuid,r.name FROM sync_location_heads h JOIN sync_location_revisions r ON r.revision_uuid=h.revision_uuid WHERE h.entity_uuid=?1 AND h.is_display_winner=1",
        [entity_uuid.as_bytes().as_slice()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok((
        crate::sync::types::EntityUuid(Uuid::from_slice(&parent)?),
        name,
    ))
}

fn location_heads(
    transaction: &rusqlite::Transaction<'_>,
    entity_uuid: Uuid,
) -> anyhow::Result<BTreeSet<Uuid>> {
    let mut statement = transaction.prepare(
        "SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1 ORDER BY revision_uuid",
    )?;
    let mut heads = BTreeSet::new();
    for row in statement.query_map([entity_uuid.as_bytes().as_slice()], |row| {
        row.get::<_, Vec<u8>>(0)
    })? {
        heads.insert(Uuid::from_slice(&row?)?);
    }
    Ok(heads)
}

fn location_supersedes(
    transaction: &rusqlite::Transaction<'_>,
    revision_uuid: Uuid,
) -> anyhow::Result<BTreeSet<Uuid>> {
    let mut statement = transaction.prepare(
        "SELECT observed_revision_uuid FROM sync_location_supersedes WHERE revision_uuid=?1 ORDER BY observed_revision_uuid",
    )?;
    let mut revisions = BTreeSet::new();
    for row in statement.query_map([revision_uuid.as_bytes().as_slice()], |row| {
        row.get::<_, Vec<u8>>(0)
    })? {
        revisions.insert(Uuid::from_slice(&row?)?);
    }
    Ok(revisions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> SyncRootDescriptorRecord {
        SyncRootDescriptorRecord {
            root_uuid: [1; 16],
            root_folder_uuid: [2; 16],
            library_root_uuid: [3; 16],
            creation_event_uuid: Some([4; 16]),
            winning_location_revision_uuid: Some([5; 16]),
            requested_name: "Photos".into(),
            materialized_name: "Photos".into(),
            materialization_fingerprint: [6; 32],
            lifecycle: "active".into(),
            logical_revision: 7,
            updated_at: 8,
        }
    }

    #[test]
    fn sync_root_descriptor_exists_without_browse_projection_and_identity_is_immutable() {
        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let descriptor = descriptor();
        database
            .upsert_sync_root_descriptor(&descriptor)
            .expect("insert descriptor");

        assert_eq!(
            database
                .sync_root_descriptor(&descriptor.root_uuid)
                .unwrap(),
            Some(descriptor.clone())
        );
        assert_eq!(
            database
                .connection
                .query_row("SELECT COUNT(*) FROM roots", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );

        let mut changed_identity = descriptor.clone();
        changed_identity.root_folder_uuid = [9; 16];
        assert!(database
            .upsert_sync_root_descriptor(&changed_identity)
            .is_err());
        let mut advanced = descriptor.clone();
        advanced.logical_revision = descriptor.logical_revision + 1;
        advanced.requested_name = "Renamed".to_string();
        advanced.materialized_name = "Renamed".to_string();
        database
            .upsert_sync_root_descriptor(&advanced)
            .expect("advance descriptor revision");
        let mut stale = descriptor.clone();
        stale.logical_revision = descriptor.logical_revision;
        assert!(database.upsert_sync_root_descriptor(&stale).is_err());
        let mut conflicting = advanced.clone();
        conflicting.requested_name = "Conflicting".to_string();
        assert!(database.upsert_sync_root_descriptor(&conflicting).is_err());
        assert_eq!(
            database
                .sync_root_descriptor(&descriptor.root_uuid)
                .unwrap(),
            Some(advanced)
        );
    }

    #[test]
    fn selected_sync_root_links_exactly_one_empty_sentinel() {
        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let descriptor = descriptor();
        database.upsert_sync_root_descriptor(&descriptor).unwrap();
        database
            .connection
            .execute(
                "INSERT INTO roots(path,display_name,enabled,created_at,root_uuid) VALUES ('D:/Photos','Photos',1,1,?1)",
                [descriptor.root_uuid.as_slice()],
            )
            .unwrap();
        let root_id = database.connection.last_insert_rowid();
        database
            .connection
            .execute(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,folder_uuid) VALUES (?1,NULL,'','root',?2)",
                params![root_id, descriptor.root_folder_uuid.as_slice()],
            )
            .unwrap();
        database
            .link_sync_root_descriptor(root_id, &descriptor.root_uuid, &descriptor.root_folder_uuid)
            .expect("link selected root");
        database
            .upsert_sync_root_local_binding(&SyncRootLocalBindingRecord {
                root_uuid: descriptor.root_uuid,
                library_uuid: [8; 16],
                local_binding_uuid: [9; 16],
                binding_generation: 1,
                root_capture_epoch: [10; 16],
                root_actor_epoch: [11; 16],
                state: "active".into(),
                root_id,
                operation_revision: 1,
                updated_at: 1,
            })
            .expect("project local binding");
        let file_id = database
            .upsert_file(super::super::FileUpsert {
                root_id,
                folder_id: database
                    .connection
                    .query_row(
                        "SELECT id FROM folders WHERE root_id=?1 AND parent_id IS NULL",
                        [root_id],
                        |row| row.get(0),
                    )
                    .unwrap(),
                name: "photo.jpg".into(),
                ext: "jpg".into(),
                size: 1,
                mtime: 1,
                ctime: None,
                file_key: Some("photo".into()),
            })
            .expect("insert file");

        let linked: Vec<u8> = database
            .connection
            .query_row(
                "SELECT root_folder_uuid FROM roots WHERE id=?1",
                [root_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(linked, descriptor.root_folder_uuid);
        assert!(database
            .connection
            .execute(
                "UPDATE folders SET name='Photos' WHERE root_id=?1 AND parent_id IS NULL",
                [root_id]
            )
            .is_err());
        assert!(database
            .connection
            .execute(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,folder_uuid) VALUES (?1,NULL,'','duplicate-root',?2)",
                params![root_id, [7_u8; 16].as_slice()],
            )
            .is_err());

        database
            .connection
            .execute(
                "UPDATE roots SET path='E:/Migrated/Photos' WHERE id=?1",
                [root_id],
            )
            .unwrap();
        let relocated: (i64, Vec<u8>, Vec<u8>) = database
            .connection
            .query_row(
                "SELECT id,root_uuid,root_folder_uuid FROM roots WHERE id=?1",
                [root_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(relocated.0, root_id);
        assert_eq!(relocated.1, descriptor.root_uuid);
        assert_eq!(relocated.2, descriptor.root_folder_uuid);
        let (source_path, token) = database
            .resolve_file_source_path_with_root_token(file_id)
            .unwrap()
            .expect("resolve relocated file");
        assert_eq!(
            source_path,
            std::path::PathBuf::from("E:/Migrated/Photos/photo.jpg")
        );
        assert_eq!(
            token,
            Some(SyncRootBindingToken {
                root_uuid: descriptor.root_uuid,
                local_binding_uuid: [9; 16],
                binding_generation: 1,
            })
        );
    }

    #[test]
    fn relocation_cutover_preserves_catalog_ids_and_fences_old_work() {
        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let descriptor = descriptor();
        let library_uuid = Uuid::from_bytes([9; 16]);
        let binding_uuid = Uuid::from_bytes([8; 16]);
        let _root_id = database
            .ensure_selected_sync_root(
                library_uuid,
                Uuid::from_bytes(descriptor.root_uuid),
                Uuid::from_bytes(descriptor.root_folder_uuid),
                Uuid::from_bytes(descriptor.library_root_uuid),
                "D:/Library/Photos",
                "Photos",
                "Photos",
                descriptor.materialization_fingerprint,
                binding_uuid,
                Uuid::from_bytes([7; 16]),
                Uuid::from_bytes([6; 16]),
                1,
                "active",
                1,
                false,
            )
            .unwrap();
        let root_id = database
            .sync_root_id(&descriptor.root_uuid)
            .unwrap()
            .unwrap();
        let folder_id: i64 = database
            .connection
            .query_row(
                "SELECT id FROM folders WHERE root_id=?1 AND parent_id IS NULL",
                [root_id],
                |row| row.get(0),
            )
            .unwrap();
        let asset_uuid = Uuid::from_bytes([5; 16]);
        database
            .connection
            .execute(
                "INSERT INTO files(root_id,folder_id,name,ext,size,mtime,file_key,status,asset_uuid) VALUES (?1,?2,'one.jpg','jpg',1,1,'file-key','active',?3)",
                params![root_id, folder_id, asset_uuid.as_bytes().as_slice()],
            )
            .unwrap();
        database
            .connection
            .execute(
                "INSERT INTO sync_local_file_identities(volume_identity,file_identity,asset_uuid,last_path,last_seen_at) VALUES ('volume','file',?1,'D:/Library/Photos/one.jpg',1)",
                [asset_uuid.as_bytes().as_slice()],
            )
            .unwrap();
        database
            .connection
            .execute(
                "INSERT INTO tasks(kind,priority,status,root_id,created_at,updated_at,root_uuid,local_binding_uuid,binding_generation) VALUES ('root_scan',1,'pending',?1,1,1,?2,?3,1)",
                params![
                    root_id,
                    descriptor.root_uuid.as_slice(),
                    binding_uuid.as_bytes().as_slice()
                ],
            )
            .unwrap();
        let old_task_id = database.connection.last_insert_rowid();
        database
            .fence_sync_root_relocation(
                Uuid::from_bytes(descriptor.root_uuid),
                binding_uuid,
                1,
                2,
                2,
            )
            .unwrap();
        let cutover = database
            .cutover_sync_root_relocation(
                Uuid::from_bytes(descriptor.root_uuid),
                binding_uuid,
                1,
                2,
                Path::new("E:/Moved/Photos"),
                "Photos",
                3,
            )
            .unwrap();
        assert_eq!(cutover.root_id, root_id);
        assert!(cutover.scan_task_id > 0);
        let (old_status, old_generation): (String, i64) = database
            .connection
            .query_row(
                "SELECT status,binding_generation FROM tasks WHERE id=?1",
                [old_task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(old_status, "cancelled");
        assert_eq!(old_generation, 1);
        let new_generation: i64 = database
            .connection
            .query_row(
                "SELECT binding_generation FROM tasks WHERE id=?1",
                [cutover.scan_task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(new_generation, 2);
        let current = database.get_root(root_id).unwrap().unwrap();
        assert_eq!(current.path, "E:/Moved/Photos");
        let identity_count: i64 = database
            .connection
            .query_row(
                "SELECT COUNT(*) FROM sync_local_file_identities WHERE asset_uuid=?1",
                [asset_uuid.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(identity_count, 1);
        assert!(database
            .sync_root_relocation_catalog_cutover_is_current(
                Uuid::from_bytes(descriptor.root_uuid),
                binding_uuid,
                2,
                Path::new("E:/Moved/Photos"),
            )
            .unwrap());
        assert!(!database
            .sync_root_relocation_catalog_cutover_is_current(
                Uuid::from_bytes(descriptor.root_uuid),
                binding_uuid,
                2,
                Path::new("D:/Library/Photos"),
            )
            .unwrap());
        let current_folder_id: i64 = database
            .connection
            .query_row(
                "SELECT id FROM folders WHERE root_id=?1 AND parent_id IS NULL",
                [root_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(current_folder_id, folder_id);
        let token = database
            .sync_root_binding_token_for_root_id(root_id)
            .unwrap()
            .unwrap();
        assert_eq!(token.binding_generation, 2);
        assert!(database
            .sync_root_binding_token_is_watchable(&token)
            .unwrap());
        assert!(database
            .require_sync_root_binding_token(&SyncRootBindingToken {
                binding_generation: 1,
                ..token.clone()
            })
            .is_err());
        database
            .complete_sync_root_relocation_projection(
                Uuid::from_bytes(descriptor.root_uuid),
                binding_uuid,
                2,
                4,
                "paused",
            )
            .unwrap();
        assert!(!database
            .sync_root_binding_token_is_watchable(&token)
            .unwrap());
        database
            .abort_sync_root_relocation_projection(
                Uuid::from_bytes(descriptor.root_uuid),
                binding_uuid,
                2,
                5,
                "paused",
            )
            .unwrap();
    }

    #[test]
    fn local_binding_projection_is_monotonic_and_fences_stale_work_after_restart() {
        let database_path = std::env::temp_dir().join(format!(
            "megle-sync-root-generation-fence-{}.sqlite",
            Uuid::now_v7()
        ));
        let root_uuid = [21; 16];
        let root_folder_uuid = [22; 16];
        let library_uuid = [23; 16];
        let binding_uuid = [24; 16];
        let root_id;
        let file_id;
        {
            let database = Database::open(&database_path).expect("open database");
            database.apply_migrations().expect("apply migrations");
            database
                .upsert_sync_root_descriptor(&SyncRootDescriptorRecord {
                    root_uuid,
                    root_folder_uuid,
                    library_root_uuid: [25; 16],
                    creation_event_uuid: None,
                    winning_location_revision_uuid: None,
                    requested_name: "Photos".into(),
                    materialized_name: "Photos".into(),
                    materialization_fingerprint: [26; 32],
                    lifecycle: "active".into(),
                    logical_revision: 1,
                    updated_at: 1,
                })
                .expect("insert descriptor");
            database
                .connection
                .execute(
                    "INSERT INTO roots(path,display_name,enabled,created_at,root_uuid) VALUES ('D:/Photos','Photos',1,1,?1)",
                    [root_uuid.as_slice()],
                )
                .expect("insert root");
            root_id = database.connection.last_insert_rowid();
            database
                .connection
                .execute(
                    "INSERT INTO folders(root_id,parent_id,name,path_hash,folder_uuid) VALUES (?1,NULL,'','root',?2)",
                    params![root_id, root_folder_uuid.as_slice()],
                )
                .expect("insert sentinel");
            database
                .link_sync_root_descriptor(root_id, &root_uuid, &root_folder_uuid)
                .expect("link descriptor");
            let root_folder_id = database
                .connection
                .query_row(
                    "SELECT id FROM folders WHERE root_id=?1 AND parent_id IS NULL",
                    [root_id],
                    |row| row.get::<_, i64>(0),
                )
                .expect("load root folder");
            file_id = database
                .upsert_file(crate::db::FileUpsert {
                    root_id,
                    folder_id: root_folder_id,
                    name: "preview.jpg".into(),
                    ext: ".jpg".into(),
                    size: 1,
                    mtime: 1,
                    ctime: None,
                    file_key: Some("preview".into()),
                })
                .expect("insert preview source");
            database
                .upsert_sync_root_local_binding(&SyncRootLocalBindingRecord {
                    root_uuid,
                    library_uuid,
                    local_binding_uuid: binding_uuid,
                    binding_generation: 1,
                    root_capture_epoch: [31; 16],
                    root_actor_epoch: [32; 16],
                    state: "active".into(),
                    root_id,
                    operation_revision: 1,
                    updated_at: 1,
                })
                .expect("insert binding projection");
            database
                .connection
                .execute(
                    "INSERT INTO tasks(kind,priority,status,root_id,created_at,updated_at) VALUES ('root_scan',0,'pending',?1,1,1)",
                    [root_id],
                )
                .expect("insert managed task");
            let task_token: (Vec<u8>, Vec<u8>, i64) = database
                .connection
                .query_row(
                    "SELECT root_uuid,local_binding_uuid,binding_generation FROM tasks WHERE root_id=?1",
                    [root_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("load managed task token");
            assert_eq!(task_token, (root_uuid.to_vec(), binding_uuid.to_vec(), 1));
        }

        let database = Database::open(&database_path).expect("reopen database");
        database.apply_migrations().expect("rerun migrations");
        let stored = database
            .sync_root_local_binding(&root_uuid)
            .expect("load binding projection")
            .expect("binding projection exists");
        assert_eq!(stored.binding_generation, 1);
        let transaction = database
            .connection
            .unchecked_transaction()
            .expect("begin token check");
        assert!(Database::sync_root_binding_token_matches_in_transaction(
            &transaction,
            &SyncRootBindingToken {
                root_uuid,
                local_binding_uuid: binding_uuid,
                binding_generation: 1,
            },
        )
        .expect("validate current token"));
        transaction.commit().expect("finish token check");

        database
            .upsert_sync_root_local_binding(&SyncRootLocalBindingRecord {
                binding_generation: 2,
                operation_revision: 2,
                updated_at: 2,
                ..stored.clone()
            })
            .expect("advance binding generation");
        assert!(database.upsert_sync_root_local_binding(&stored).is_err());
        assert!(database
            .upsert_preview_failure_for_root_token(
                crate::db::PreviewFailureRecord {
                    file_id,
                    profile: "preview_native".into(),
                    failure_kind: "permanent".into(),
                    error: "stale decode".into(),
                    source_size: 1,
                    source_mtime: 1,
                    created_at: 2,
                    updated_at: 2,
                },
                Some(&SyncRootBindingToken {
                    root_uuid,
                    local_binding_uuid: binding_uuid,
                    binding_generation: 1,
                }),
            )
            .is_err());
        assert!(database
            .get_preview_failure(file_id, "preview_native")
            .expect("query stale preview failure")
            .is_none());
        assert!(!database
            .task_attempt_is_current(
                database
                    .connection
                    .query_row("SELECT id FROM tasks WHERE root_id=?1", [root_id], |row| {
                        row.get(0)
                    },)
                    .unwrap(),
                0,
            )
            .expect("stale task attempt check"));
        assert!(database
            .connection
            .execute(
                "UPDATE tasks SET status='running' WHERE root_id=?1",
                [root_id]
            )
            .is_err());
        database
            .connection
            .execute(
                "INSERT INTO tasks(kind,priority,status,created_at,updated_at) VALUES ('thumbnail',0,'pending',3,3)",
                [],
            )
            .expect("insert legacy unowned task");
        let legacy_token: (Option<Vec<u8>>, Option<Vec<u8>>, Option<i64>) = database
            .connection
            .query_row(
                "SELECT root_uuid,local_binding_uuid,binding_generation FROM tasks WHERE root_id IS NULL ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("load legacy task token");
        assert_eq!(legacy_token, (None, None, None));

        drop(database);
        let _ = std::fs::remove_file(&database_path);
        let _ = std::fs::remove_file(database_path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(database_path.with_extension("sqlite-shm"));
    }

    #[test]
    fn local_only_fork_keeps_local_edits_out_of_the_provider_queue() {
        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let root_uuid = Uuid::from_bytes([31; 16]);
        let root_folder_uuid = Uuid::from_bytes([32; 16]);
        let library_uuid = Uuid::from_bytes([33; 16]);
        let binding_uuid = Uuid::from_bytes([34; 16]);
        database
            .upsert_sync_root_descriptor(&SyncRootDescriptorRecord {
                root_uuid: *root_uuid.as_bytes(),
                root_folder_uuid: *root_folder_uuid.as_bytes(),
                library_root_uuid: [35; 16],
                creation_event_uuid: None,
                winning_location_revision_uuid: None,
                requested_name: "Photos".into(),
                materialized_name: "Photos".into(),
                materialization_fingerprint: [36; 32],
                lifecycle: "active".into(),
                logical_revision: 1,
                updated_at: 1,
            })
            .unwrap();
        database
            .connection
            .execute(
                "INSERT INTO roots(path,display_name,enabled,created_at,root_uuid) VALUES ('D:/Photos','Photos',1,1,?1)",
                [root_uuid.as_bytes().as_slice()],
            )
            .unwrap();
        let root_id = database.connection.last_insert_rowid();
        database
            .connection
            .execute(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,folder_uuid) VALUES (?1,NULL,'','root',?2)",
                params![root_id, root_folder_uuid.as_bytes().as_slice()],
            )
            .unwrap();
        database
            .link_sync_root_descriptor(root_id, root_uuid.as_bytes(), root_folder_uuid.as_bytes())
            .unwrap();
        database
            .initialize_sync_identity(library_uuid, Uuid::from_bytes([37; 16]), Uuid::now_v7())
            .unwrap();
        database.enable_fake_sync_capture_for_test().unwrap();
        database
            .upsert_sync_root_local_binding(&SyncRootLocalBindingRecord {
                root_uuid: *root_uuid.as_bytes(),
                library_uuid: *library_uuid.as_bytes(),
                local_binding_uuid: *binding_uuid.as_bytes(),
                binding_generation: 1,
                root_capture_epoch: [41; 16],
                root_actor_epoch: [42; 16],
                state: "active".into(),
                root_id,
                operation_revision: 1,
                updated_at: 1,
            })
            .unwrap();
        let first_head = database
            .connection
            .query_row(
                "SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1",
                [root_folder_uuid.as_bytes().as_slice()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap();
        database
            .commit_location_rename_once(
                Uuid::now_v7(),
                root_folder_uuid,
                "Before stop",
                &BTreeSet::from([Uuid::from_slice(&first_head).unwrap()]),
                &BTreeSet::from([Uuid::from_slice(&first_head).unwrap()]),
            )
            .unwrap();
        assert_eq!(
            database
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM sync_outbox WHERE state='pending'",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            1
        );
        let stop_work = database.root_stop_work(root_uuid).unwrap();
        assert_eq!(stop_work.pending_work, 1);
        assert_eq!(stop_work.published_work, 0);
        database
            .fence_root_as_local_only(
                root_uuid,
                binding_uuid,
                1,
                2,
                Uuid::from_u128(301),
                Uuid::from_u128(302),
                2,
            )
            .unwrap();
        assert_eq!(
            database
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM sync_outbox WHERE state='archived'",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            1
        );
        let second_head = database
            .connection
            .query_row(
                "SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1",
                [root_folder_uuid.as_bytes().as_slice()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap();
        database
            .commit_location_rename_once(
                Uuid::now_v7(),
                root_folder_uuid,
                "After stop",
                &BTreeSet::from([Uuid::from_slice(&second_head).unwrap()]),
                &BTreeSet::from([Uuid::from_slice(&second_head).unwrap()]),
            )
            .unwrap();
        assert_eq!(
            database
                .connection
                .query_row("SELECT COUNT(*) FROM sync_outbox", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            database
                .connection
                .query_row("SELECT COUNT(*) FROM sync_events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );

        let fork_binding = database
            .sync_root_local_binding(root_uuid.as_bytes())
            .unwrap()
            .unwrap();
        let reconciled_binding_uuid = Uuid::from_u128(401);
        let reconciled_capture_epoch = Uuid::from_u128(402);
        let reconciled_actor_epoch = Uuid::from_u128(403);
        database
            .upsert_sync_root_local_binding(&SyncRootLocalBindingRecord {
                local_binding_uuid: *reconciled_binding_uuid.as_bytes(),
                binding_generation: 3,
                root_capture_epoch: *reconciled_capture_epoch.as_bytes(),
                root_actor_epoch: *reconciled_actor_epoch.as_bytes(),
                state: "reconcile-required".into(),
                operation_revision: 3,
                updated_at: 3,
                ..fork_binding
            })
            .unwrap();
        let plan_uuid = Uuid::from_u128(404);
        let selected_location = serde_json::to_string(&serde_json::json!({
            "revisionUuid": Uuid::from_u128(405),
            "parentUuid": Uuid::from_bytes([35; 16]),
            "name": "After stop"
        }))
        .unwrap();
        database
            .apply_root_reconciliation_item(
                plan_uuid,
                0,
                root_uuid,
                reconciled_binding_uuid,
                3,
                root_folder_uuid,
                "location",
                "keep-local",
                Some(&selected_location),
                None,
            )
            .unwrap();
        database
            .apply_root_reconciliation_item(
                plan_uuid,
                0,
                root_uuid,
                reconciled_binding_uuid,
                3,
                root_folder_uuid,
                "location",
                "keep-local",
                Some(&selected_location),
                None,
            )
            .unwrap();
        let (pending, actor): (i64, Vec<u8>) = database
            .connection
            .query_row(
                "SELECT COUNT(*),MAX(event.actor_epoch) FROM sync_outbox outbox JOIN sync_events event ON event.event_uuid=outbox.event_uuid WHERE outbox.state='pending'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(pending, 1);
        assert_eq!(actor, reconciled_actor_epoch.as_bytes());
        assert_eq!(
            database
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM sync_root_reconciliation_applied WHERE plan_uuid=?1",
                    [plan_uuid.as_bytes().as_slice()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn publish_as_new_reuses_catalog_tree_with_fresh_root_and_actor_identity() {
        let database = Database::open_in_memory().expect("open database");
        database.apply_migrations().expect("apply migrations");
        let library_uuid = Uuid::from_u128(410);
        let library_root_uuid = Uuid::from_u128(411);
        let source_root_uuid = Uuid::from_u128(412);
        let source_folder_uuid = Uuid::from_u128(413);
        let source_binding_uuid = Uuid::from_u128(414);
        database
            .upsert_sync_root_descriptor(&SyncRootDescriptorRecord {
                root_uuid: *source_root_uuid.as_bytes(),
                root_folder_uuid: *source_folder_uuid.as_bytes(),
                library_root_uuid: *library_root_uuid.as_bytes(),
                creation_event_uuid: None,
                winning_location_revision_uuid: None,
                requested_name: "Photos".into(),
                materialized_name: "Photos".into(),
                materialization_fingerprint: [7; 32],
                lifecycle: "active".into(),
                logical_revision: 2,
                updated_at: 1,
            })
            .unwrap();
        database.connection.execute("INSERT INTO roots(path,display_name,enabled,created_at,root_uuid) VALUES ('D:/Photos','Photos',1,1,?1)",[source_root_uuid.as_bytes().as_slice()]).unwrap();
        let root_id = database.connection.last_insert_rowid();
        database.connection.execute("INSERT INTO folders(root_id,parent_id,name,path_hash,folder_uuid) VALUES (?1,NULL,'','root',?2)",params![root_id,source_folder_uuid.as_bytes().as_slice()]).unwrap();
        database
            .link_sync_root_descriptor(
                root_id,
                source_root_uuid.as_bytes(),
                source_folder_uuid.as_bytes(),
            )
            .unwrap();
        database
            .initialize_sync_identity(library_uuid, library_root_uuid, Uuid::from_u128(415))
            .unwrap();
        database.enable_fake_sync_capture_for_test().unwrap();
        database
            .upsert_sync_root_local_binding(&SyncRootLocalBindingRecord {
                root_uuid: *source_root_uuid.as_bytes(),
                library_uuid: *library_uuid.as_bytes(),
                local_binding_uuid: *source_binding_uuid.as_bytes(),
                binding_generation: 2,
                root_capture_epoch: *Uuid::from_u128(416).as_bytes(),
                root_actor_epoch: *Uuid::from_u128(417).as_bytes(),
                state: "local-only-fork".into(),
                root_id,
                operation_revision: 3,
                updated_at: 1,
            })
            .unwrap();
        let sentinel_id: i64 = database
            .connection
            .query_row(
                "SELECT id FROM folders WHERE root_id=?1 AND parent_id IS NULL",
                [root_id],
                |row| row.get(0),
            )
            .unwrap();
        let asset_uuid = Uuid::from_u128(418);
        database
            .connection
            .execute(
                "INSERT INTO files(root_id,folder_id,name,ext,size,mtime,status,asset_uuid) VALUES (?1,?2,'local.jpg','jpg',1,1,'active',?3)",
                params![root_id, sentinel_id, asset_uuid.as_bytes().as_slice()],
            )
            .unwrap();
        let transaction = database.connection.unchecked_transaction().unwrap();
        crate::sync::capture::capture_catalog_mutation(
            &transaction,
            crate::sync::capture::MutationOrigin::LocalApi,
            crate::sync::types::EntityUuid(asset_uuid),
            crate::sync::types::Operation::SetField {
                field: "entity-kind".into(),
                value: serde_json::json!("asset"),
            },
        )
        .unwrap();
        crate::sync::capture::capture_catalog_mutation(
            &transaction,
            crate::sync::capture::MutationOrigin::LocalApi,
            crate::sync::types::EntityUuid(asset_uuid),
            crate::sync::types::Operation::SetLocation {
                revision_uuid: crate::sync::types::RevisionUuid(Uuid::from_u128(419)),
                parent_uuid: Some(crate::sync::types::EntityUuid(source_folder_uuid)),
                name: "local.jpg".into(),
                supersedes: BTreeSet::new(),
            },
        )
        .unwrap();
        transaction.commit().unwrap();
        let new_root_uuid = Uuid::from_u128(420);
        let new_folder_uuid = Uuid::from_u128(421);
        let new_binding_uuid = Uuid::from_u128(422);
        let capture_epoch = Uuid::from_u128(423);
        let actor_epoch = Uuid::from_u128(424);
        let operation_uuid = Uuid::from_u128(425);
        database
            .publish_root_as_new(
                operation_uuid,
                library_uuid,
                source_root_uuid,
                new_root_uuid,
                new_folder_uuid,
                "Photos",
                "Photos (00000000)",
                [8; 32],
                new_binding_uuid,
                capture_epoch,
                actor_epoch,
                1,
            )
            .unwrap();
        database
            .publish_root_as_new(
                operation_uuid,
                library_uuid,
                source_root_uuid,
                new_root_uuid,
                new_folder_uuid,
                "Photos",
                "Photos (00000000)",
                [8; 32],
                new_binding_uuid,
                capture_epoch,
                actor_epoch,
                1,
            )
            .unwrap();
        assert!(database
            .sync_root_descriptor(source_root_uuid.as_bytes())
            .unwrap()
            .is_none());
        assert!(database
            .sync_root_descriptor(new_root_uuid.as_bytes())
            .unwrap()
            .is_some());
        let binding = database
            .sync_root_local_binding(new_root_uuid.as_bytes())
            .unwrap()
            .unwrap();
        assert_eq!(binding.local_binding_uuid, *new_binding_uuid.as_bytes());
        assert_eq!(binding.root_capture_epoch, *capture_epoch.as_bytes());
        assert_eq!(binding.root_actor_epoch, *actor_epoch.as_bytes());
        assert_eq!(binding.state, "active");
        assert_eq!(
            database
                .connection
                .query_row("SELECT COUNT(*) FROM roots", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            database
                .connection
                .query_row(
                    "SELECT root_uuid FROM roots WHERE id=?1",
                    [root_id],
                    |row| row.get::<_, Vec<u8>>(0)
                )
                .unwrap(),
            new_root_uuid.as_bytes()
        );
        assert_eq!(database.connection.query_row("SELECT COUNT(*) FROM sync_outbox outbox JOIN sync_events event ON event.event_uuid=outbox.event_uuid WHERE event.actor_epoch=?1 AND outbox.state='pending'",[actor_epoch.as_bytes().as_slice()],|row|row.get::<_,i64>(0)).unwrap(),5);
        let state = crate::sync::persistence::capture_protocol_state(&database.connection)
            .unwrap()
            .0;
        assert!(state
            .entities
            .contains_key(&crate::sync::types::EntityUuid(asset_uuid)));
    }

    #[test]
    fn logical_rename_is_idempotent_and_rejects_stale_heads() {
        let database = Database::open_in_memory().unwrap();
        database.apply_migrations().unwrap();
        database
            .connection
            .execute(
                "INSERT INTO roots(path,display_name,enabled,created_at,root_uuid) VALUES ('D:/Photos','Photos',1,1,?1)",
                [[1_u8; 16].as_slice()],
            )
            .unwrap();
        let root_id = database.connection.last_insert_rowid();
        database
            .connection
            .execute(
                "INSERT INTO folders(root_id,parent_id,name,path_hash,folder_uuid) VALUES (?1,NULL,'','root',?2)",
                params![root_id, [2_u8; 16].as_slice()],
            )
            .unwrap();
        database.upsert_sync_root_descriptor(&descriptor()).unwrap();
        database
            .link_sync_root_descriptor(root_id, &[1; 16], &[2; 16])
            .unwrap();
        database
            .initialize_sync_identity(
                Uuid::from_bytes([9; 16]),
                Uuid::from_bytes([3; 16]),
                Uuid::now_v7(),
            )
            .unwrap();
        database.enable_fake_sync_capture_for_test().unwrap();
        let head = database
            .connection
            .query_row(
                "SELECT revision_uuid FROM sync_location_heads WHERE entity_uuid=?1",
                [[2_u8; 16].as_slice()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .unwrap();
        let heads = BTreeSet::from([Uuid::from_slice(&head).unwrap()]);
        let operation = Uuid::now_v7();
        let first = database
            .commit_location_rename_once(
                operation,
                Uuid::from_bytes([2; 16]),
                "Renamed",
                &heads,
                &heads,
            )
            .unwrap();
        let retry = database
            .commit_location_rename_once(
                operation,
                Uuid::from_bytes([2; 16]),
                "Renamed",
                &heads,
                &heads,
            )
            .unwrap();
        assert_eq!(first, retry);
        assert!(database
            .commit_location_rename_once(
                Uuid::now_v7(),
                Uuid::from_bytes([2; 16]),
                "Stale",
                &heads,
                &heads,
            )
            .is_err());
        assert_eq!(
            database
                .sync_root_descriptor(&[1; 16])
                .unwrap()
                .unwrap()
                .requested_name,
            "Renamed"
        );
        assert_eq!(
            database
                .connection
                .query_row(
                    "SELECT name FROM folders WHERE folder_uuid=?1",
                    [[2_u8; 16].as_slice()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            ""
        );

        let concurrent_revision = Uuid::now_v7();
        let remote = crate::sync::codec::seal_event(crate::sync::types::EventEnvelope {
            wire_version: crate::sync::codec::WIRE_VERSION,
            envelope_version: crate::sync::codec::ENVELOPE_VERSION,
            reducer_version: crate::sync::codec::REDUCER_VERSION,
            library_uuid: crate::sync::types::LibraryUuid(Uuid::from_bytes([9; 16])),
            event_uuid: crate::sync::types::EventUuid::new(),
            entity_uuid: crate::sync::types::EntityUuid(Uuid::from_bytes([2; 16])),
            stream_uuid: crate::sync::types::StreamUuid(Uuid::from_bytes([7; 16])),
            stream_kind: crate::sync::types::StreamKind::Catalog,
            actor: crate::sync::types::ActorId {
                device_uuid: crate::sync::types::DeviceUuid(Uuid::from_bytes([8; 16])),
                stream_uuid: crate::sync::types::StreamUuid(Uuid::from_bytes([7; 16])),
                actor_epoch: crate::sync::types::ActorEpochUuid(Uuid::from_bytes([6; 16])),
            },
            actor_seq: 1,
            binding_epoch: crate::sync::types::BindingEpochUuid(Uuid::from_bytes([5; 16])),
            hlc: crate::sync::types::Hlc {
                wall_ms: 10,
                logical: 0,
            },
            operation: crate::sync::types::Operation::SetLocation {
                revision_uuid: crate::sync::types::RevisionUuid(concurrent_revision),
                parent_uuid: Some(crate::sync::types::EntityUuid(Uuid::from_bytes([3; 16]))),
                name: "Remote".into(),
                supersedes: BTreeSet::new(),
            },
            base_events: BTreeSet::new(),
            key_version: 0,
            integrity_hash: [0; 32],
        })
        .unwrap();
        database.apply_remote_sync_events(&[remote]).unwrap();
        let conflicting_heads = location_heads(
            &database.connection.unchecked_transaction().unwrap(),
            Uuid::from_bytes([2; 16]),
        )
        .unwrap();
        assert_eq!(conflicting_heads.len(), 2);
        let resolution = Uuid::now_v7();
        let resolved_event = database
            .resolve_location_conflict_once(
                resolution,
                Uuid::from_bytes([2; 16]),
                Some("Resolved"),
                &conflicting_heads,
                &conflicting_heads,
            )
            .unwrap();
        assert_eq!(
            database
                .resolve_location_conflict_once(
                    resolution,
                    Uuid::from_bytes([2; 16]),
                    Some("Resolved"),
                    &conflicting_heads,
                    &conflicting_heads,
                )
                .unwrap(),
            resolved_event
        );
        let current_heads = location_heads(
            &database.connection.unchecked_transaction().unwrap(),
            Uuid::from_bytes([2; 16]),
        )
        .unwrap();
        assert_eq!(current_heads, BTreeSet::from([resolution]));
        let conflict_state: String = database
            .connection
            .query_row(
                "SELECT state FROM sync_conflicts WHERE entity_uuid=?1 AND conflict_kind='location-heads' ORDER BY created_at DESC LIMIT 1",
                [[2_u8; 16].as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(conflict_state, "resolved");
    }
}
