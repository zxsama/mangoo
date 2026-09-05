use anyhow::{bail, Context};
use rusqlite::{params, OptionalExtension};

use super::Database;

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
    pub state: String,
    pub root_id: i64,
    pub operation_revision: u64,
    pub updated_at: i64,
}

#[allow(dead_code)]
fn fixed_bytes<const N: usize>(value: Vec<u8>, field: &str) -> anyhow::Result<[u8; N]> {
    value
        .try_into()
        .map_err(|_| anyhow::anyhow!("{field} must contain exactly {N} bytes"))
}

impl Database {
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
            "INSERT INTO sync_root_local_bindings(root_uuid,library_uuid,local_binding_uuid,binding_generation,state,root_id,operation_revision,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(root_uuid) DO UPDATE SET local_binding_uuid=excluded.local_binding_uuid,binding_generation=excluded.binding_generation,state=excluded.state,operation_revision=excluded.operation_revision,updated_at=excluded.updated_at WHERE sync_root_local_bindings.library_uuid=excluded.library_uuid AND sync_root_local_bindings.root_id=excluded.root_id AND (excluded.binding_generation>sync_root_local_bindings.binding_generation OR (excluded.binding_generation=sync_root_local_bindings.binding_generation AND excluded.operation_revision>=sync_root_local_bindings.operation_revision))",
            params![
                binding.root_uuid.as_slice(),
                binding.library_uuid.as_slice(),
                binding.local_binding_uuid.as_slice(),
                generation,
                binding.state,
                binding.root_id,
                operation_revision,
                binding.updated_at,
            ],
        )?;
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
                "SELECT root_uuid,library_uuid,local_binding_uuid,binding_generation,state,root_id,operation_revision,updated_at FROM sync_root_local_bindings WHERE root_uuid=?1",
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
    }
}
