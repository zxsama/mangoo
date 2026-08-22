use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

const HOST_MIGRATION_0001: &str = include_str!("../host_migrations/0001_host_registry.sql");
const HOST_MIGRATION_0002: &str =
    include_str!("../host_migrations/0002_sync_protocol_foundation.sql");
const HOST_MIGRATION_0003: &str = include_str!("../host_migrations/0003_plugin_runtime.sql");
const HOST_MIGRATION_0004: &str =
    include_str!("../host_migrations/0004_provider_root_discovery.sql");

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

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPluginProjection {
    pub plugin_id: String,
    pub selected_version: String,
    pub known_good_version: Option<String>,
    pub enabled_intent: bool,
    pub requested_permissions: serde_json::Value,
    pub granted_permissions: serde_json::Value,
    pub capabilities: serde_json::Value,
    pub permission_required: bool,
    pub runtime_state: String,
    pub auth_state: Option<String>,
    pub account_uuid: Option<String>,
    pub account_display_name: Option<String>,
    pub backoff_until: Option<i64>,
    pub last_healthy_at: Option<i64>,
    pub last_error_code: Option<String>,
    pub last_error_detail: Option<String>,
    pub compatibility_reason: Option<String>,
}

pub struct RegisterBundledPlugin<'a> {
    pub plugin_id: &'a str,
    pub version: &'a str,
    pub manifest_json: &'a str,
    pub entry_relative_path: &'a str,
    pub entry_sha256: &'a [u8; 32],
    pub install_root: &'a Path,
    pub trust_policy_id: &'a str,
    pub requested_permissions_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct PluginRuntimeLaunch {
    pub plugin_id: String,
    pub version: String,
    pub manifest_json: String,
    pub install_root: PathBuf,
    pub entry_sha256: [u8; 32],
    pub account_uuid: String,
    pub account_generation: i64,
    pub granted_permissions: Vec<crate::plugins::manifest::Permission>,
}

impl PluginRuntimeLaunch {
    pub fn fingerprint(&self) -> String {
        plugin_runtime_launch_fingerprint(
            &self.version,
            &self.account_uuid,
            self.account_generation,
            &self.granted_permissions,
        )
    }
}

fn plugin_runtime_launch_fingerprint(
    version: &str,
    account_uuid: &str,
    account_generation: i64,
    granted_permissions: &[crate::plugins::manifest::Permission],
) -> String {
    let grants =
        serde_json::to_string(granted_permissions).unwrap_or_else(|_| "invalid-grants".to_owned());
    format!("{version}:{account_uuid}:{account_generation}:{grants}")
}

fn plugin_runtime_launch_matches(
    connection: &Connection,
    plugin_id: &str,
    expected_fingerprint: &str,
) -> anyhow::Result<bool> {
    let current = connection
        .query_row(
            "SELECT i.selected_version,i.enabled_intent,i.requested_permissions_json,i.granted_permissions_json,a.account_uuid,a.generation,r.state FROM plugin_installations i JOIN plugin_accounts a ON a.plugin_id=i.plugin_id JOIN plugin_runtime_state r ON r.plugin_id=i.plugin_id WHERE i.plugin_id=?1",
            [plugin_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? != 0,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((
        version,
        enabled,
        requested_json,
        granted_json,
        account_uuid,
        account_generation,
        state,
    )) = current
    else {
        return Ok(false);
    };
    let requested =
        serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(&requested_json)?;
    let granted = serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(&granted_json)?;
    Ok(enabled
        && state != "configuration-required"
        && permissions_match(&requested, &granted)
        && plugin_runtime_launch_fingerprint(&version, &account_uuid, account_generation, &granted)
            == expected_fingerprint)
}

#[derive(Debug, Clone)]
pub struct PluginAccountSecret {
    pub account_uuid: String,
    pub plugin_id: String,
    pub secret_ref: Option<String>,
    pub generation: i64,
}

#[derive(Debug, Clone)]
pub struct BindingTransport {
    pub plugin_id: String,
    pub account_uuid: String,
    pub binding_epoch: uuid::Uuid,
    pub actor_epoch: uuid::Uuid,
    pub transport_generation: i64,
    pub state: String,
    pub remote_namespace: Option<String>,
    pub cursor: Option<String>,
    pub page_token: Option<String>,
    pub latest_checkpoint_uuid: Option<uuid::Uuid>,
}

#[derive(Debug, Clone)]
pub struct BindingFence {
    pub account_uuid: String,
    pub binding_epoch: uuid::Uuid,
    pub next_transport_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRootTransport {
    pub plugin_id: String,
    pub account_uuid: String,
    pub account_generation: i64,
    pub cursor: Option<String>,
    pub page_token: Option<String>,
}

pub struct HostRegistry {
    connection: Mutex<Connection>,
}

fn permissions_match(
    requested: &[crate::plugins::manifest::Permission],
    granted: &[crate::plugins::manifest::Permission],
) -> bool {
    requested.len() == granted.len()
        && requested
            .iter()
            .all(|permission| granted.contains(permission))
}

fn pause_plugin_bindings_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    plugin_id: &str,
) -> anyhow::Result<Vec<BindingFence>> {
    let now = unix_timestamp_millis();
    transaction.execute(
        "UPDATE library_sync_bindings SET state='paused',transport_generation=transport_generation+1,updated_at=?1 WHERE plugin_id=?2 AND state <> 'unbinding'",
        params![now, plugin_id],
    )?;
    let mut statement = transaction.prepare(
        "SELECT account_uuid,binding_epoch,transport_generation FROM library_sync_bindings WHERE plugin_id=?1 AND state='paused' ORDER BY library_uuid",
    )?;
    let rows = statement.query_map([plugin_id], |row| {
        let binding_epoch: Vec<u8> = row.get(1)?;
        Ok((
            row.get::<_, String>(0)?,
            binding_epoch,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut fences = Vec::new();
    for row in rows {
        let (account_uuid, binding_epoch, next_transport_generation) = row?;
        fences.push(BindingFence {
            account_uuid,
            binding_epoch: uuid::Uuid::from_slice(&binding_epoch)?,
            next_transport_generation: u64::try_from(next_transport_generation)
                .map_err(|_| anyhow::anyhow!("invalid transport generation"))?,
        });
    }
    Ok(fences)
}

fn commit_plugin_probation_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    plugin_id: &str,
) -> anyhow::Result<bool> {
    let candidate: Option<(String, String)> = transaction
        .query_row(
            "SELECT journal_uuid,to_version FROM plugin_version_journal WHERE plugin_id=?1 AND phase='probation' ORDER BY created_at DESC LIMIT 1",
            [plugin_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((journal_uuid, version)) = candidate else {
        return Ok(false);
    };
    let now = unix_timestamp_millis();
    transaction.execute("UPDATE plugin_versions SET state=CASE WHEN version=?1 THEN 'known-good' WHEN state='known-good' THEN 'retired' ELSE state END WHERE plugin_id=?2",params![version,plugin_id])?;
    transaction.execute("UPDATE plugin_installations SET selected_version=?1,known_good_version=?1,updated_at=?2 WHERE plugin_id=?3",params![version,now,plugin_id])?;
    transaction.execute(
        "UPDATE plugin_version_journal SET phase='committed',updated_at=?1 WHERE journal_uuid=?2",
        params![now, journal_uuid],
    )?;
    Ok(true)
}

fn rollback_plugin_probation_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    plugin_id: &str,
    error_code: &str,
) -> anyhow::Result<Vec<BindingFence>> {
    let pending: Option<(String, Option<String>, String)> = transaction.query_row(
        "SELECT journal_uuid,from_version,to_version FROM plugin_version_journal WHERE plugin_id=?1 AND phase IN ('verifying','draining','probation') ORDER BY created_at DESC LIMIT 1",
        [plugin_id],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).optional()?;
    let Some((journal_uuid, from_version, candidate)) = pending else {
        anyhow::bail!("plugin-probation-missing")
    };
    let fallback = from_version
        .or_else(|| {
            transaction
                .query_row(
                    "SELECT known_good_version FROM plugin_installations WHERE plugin_id=?1",
                    [plugin_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten()
        })
        .ok_or_else(|| anyhow::anyhow!("plugin-known-good-missing"))?;
    let now = unix_timestamp_millis();
    transaction.execute("UPDATE plugin_versions SET state=CASE WHEN version=?1 THEN 'rejected' WHEN version=?2 THEN 'known-good' ELSE state END WHERE plugin_id=?3",params![candidate,fallback,plugin_id])?;
    transaction.execute("UPDATE plugin_installations SET selected_version=?1,known_good_version=?1,updated_at=?2 WHERE plugin_id=?3",params![fallback,now,plugin_id])?;
    transaction.execute("UPDATE plugin_runtime_state SET state='starting',last_error_code=?1,updated_at=?2 WHERE plugin_id=?3",params![error_code,now,plugin_id])?;
    transaction.execute("UPDATE plugin_version_journal SET phase='rolled-back',error_code=?1,updated_at=?2 WHERE journal_uuid=?3",params![error_code,now,journal_uuid])?;
    pause_plugin_bindings_in_transaction(transaction, plugin_id)
}

impl HostRegistry {
    pub fn plugin_runtime_launch_is_current(
        &self,
        plugin_id: &str,
        expected_fingerprint: &str,
    ) -> anyhow::Result<bool> {
        plugin_runtime_launch_matches(
            &self.connection.lock().unwrap_or_else(|p| p.into_inner()),
            plugin_id,
            expected_fingerprint,
        )
    }

    pub fn require_ready_sync_provider(
        &self,
        plugin_id: &str,
        account_uuid: &str,
    ) -> anyhow::Result<()> {
        let auth_state = self.require_runtime_provider(plugin_id, account_uuid)?;
        if auth_state != "authenticated" {
            anyhow::bail!("provider-account-not-authenticated");
        }
        Ok(())
    }

    pub fn require_runtime_provider(
        &self,
        plugin_id: &str,
        account_uuid: &str,
    ) -> anyhow::Result<String> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let row = connection
            .query_row(
                "SELECT i.enabled_intent,r.state,a.auth_state,r.capabilities_json FROM plugin_installations i JOIN plugin_runtime_state r ON r.plugin_id=i.plugin_id JOIN plugin_accounts a ON a.plugin_id=i.plugin_id WHERE i.plugin_id=?1 AND a.account_uuid=?2",
                params![plugin_id, account_uuid],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)? != 0,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((enabled, runtime_state, auth_state, capabilities_json)) = row else {
            anyhow::bail!("provider-account-not-found");
        };
        if !enabled {
            anyhow::bail!("provider-disabled");
        }
        if runtime_state != "ready" {
            anyhow::bail!("provider-not-ready");
        }
        let capabilities = serde_json::from_str::<Vec<String>>(&capabilities_json)?;
        if !capabilities
            .iter()
            .any(|capability| capability == "sync-provider.v1")
        {
            anyhow::bail!("provider-incompatible");
        }
        Ok(auth_state)
    }

    pub fn plugin_account_secret(
        &self,
        plugin_id: &str,
    ) -> anyhow::Result<Option<PluginAccountSecret>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        connection.query_row("SELECT account_uuid,plugin_id,secret_ref,generation FROM plugin_accounts WHERE plugin_id=?1", [plugin_id], |row| Ok(PluginAccountSecret { account_uuid: row.get(0)?, plugin_id: row.get(1)?, secret_ref: row.get(2)?, generation: row.get(3)? })).optional().map_err(Into::into)
    }

    pub fn set_plugin_account_secret(
        &self,
        plugin_id: &str,
        secret_ref: Option<&str>,
        auth_state: &str,
        generation: i64,
    ) -> anyhow::Result<()> {
        let changed = self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute("UPDATE plugin_accounts SET secret_ref=?1,auth_state=?2,generation=?3,updated_at=?4 WHERE plugin_id=?5", params![secret_ref,auth_state,generation,unix_timestamp_millis(),plugin_id])?;
        if changed != 1 {
            anyhow::bail!("plugin-account-not-found");
        }
        Ok(())
    }

    pub fn set_plugin_account_secret_for_launch(
        &self,
        plugin_id: &str,
        expected_fingerprint: &str,
        secret_ref: Option<&str>,
        auth_state: &str,
    ) -> anyhow::Result<bool> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if !plugin_runtime_launch_matches(&transaction, plugin_id, expected_fingerprint)? {
            return Ok(false);
        }
        let changed = transaction.execute(
            "UPDATE plugin_accounts SET secret_ref=?1,auth_state=?2,updated_at=?3 WHERE plugin_id=?4",
            params![secret_ref,auth_state,unix_timestamp_millis(),plugin_id],
        )?;
        if changed != 1 {
            return Ok(false);
        }
        transaction.commit()?;
        Ok(true)
    }

    pub fn sign_out_plugin_account(
        &self,
        plugin_id: &str,
        expected_generation: i64,
    ) -> anyhow::Result<bool> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE plugin_accounts SET secret_ref=NULL,auth_state='signed-out',generation=generation+1,updated_at=?1 WHERE plugin_id=?2 AND generation=?3",
            params![unix_timestamp_millis(),plugin_id,expected_generation],
        )?;
        if changed != 1 {
            return Ok(false);
        }
        transaction.execute(
            "DELETE FROM provider_binding_cursors WHERE library_uuid IN (SELECT library_uuid FROM library_sync_bindings WHERE plugin_id=?1)",
            [plugin_id],
        )?;
        transaction.execute(
            "DELETE FROM provider_account_root_cursors WHERE account_uuid=(SELECT account_uuid FROM plugin_accounts WHERE plugin_id=?1)",
            [plugin_id],
        )?;
        transaction.execute(
            "DELETE FROM provider_remote_libraries WHERE account_uuid=(SELECT account_uuid FROM plugin_accounts WHERE plugin_id=?1)",
            [plugin_id],
        )?;
        transaction.execute(
            "UPDATE plugin_runtime_state SET state='starting',pid=NULL,session_id=NULL,started_at=NULL,backoff_until=NULL,last_error_code=NULL,last_error_detail=NULL,updated_at=?1 WHERE plugin_id=?2",
            params![unix_timestamp_millis(), plugin_id],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    pub fn plugin_runtime_session(&self, plugin_id: &str) -> anyhow::Result<Option<String>> {
        self.connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .query_row(
                "SELECT session_id FROM plugin_runtime_state WHERE plugin_id=?1 AND state='ready'",
                [plugin_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn reset_plugin_runtime_states_for_startup(
        &self,
        secret_store_available: bool,
    ) -> anyhow::Result<usize> {
        let now = unix_timestamp_millis();
        let onedrive_configured = std::env::var("MEGLE_ONEDRIVE_CLIENT_ID")
            .ok()
            .is_some_and(|value| !value.trim().is_empty());
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let rows = {
            let mut statement = transaction.prepare(
                "SELECT plugin_id,enabled_intent,requested_permissions_json,granted_permissions_json FROM plugin_installations",
            )?;
            let values = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)? != 0,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            values
        };
        let mut changed = 0;
        for (plugin_id, enabled, requested_json, granted_json) in rows {
            let requested: Vec<crate::plugins::manifest::Permission> =
                serde_json::from_str(&requested_json)?;
            let granted: Vec<crate::plugins::manifest::Permission> =
                serde_json::from_str(&granted_json)?;
            let requires_secret = requested.iter().any(|permission| {
                matches!(
                    permission,
                    crate::plugins::manifest::Permission::Secret { .. }
                )
            });
            let (state, error_code, error_detail) = if !enabled {
                ("disabled", None, None)
            } else if !permissions_match(&requested, &granted) {
                ("permission-required", None, None)
            } else if requires_secret && !secret_store_available {
                (
                    "configuration-required",
                    Some("secret-store-unavailable"),
                    Some("MEGLE_SECRET_KEY_FILE is missing or invalid"),
                )
            } else if plugin_id == "com.megle.onedrive" && !onedrive_configured {
                (
                    "configuration-required",
                    Some("onedrive-client-id-missing"),
                    Some("MEGLE_ONEDRIVE_CLIENT_ID is required"),
                )
            } else {
                ("starting", None, None)
            };
            changed += transaction.execute(
                "UPDATE plugin_runtime_state SET state=?1,pid=NULL,session_id=NULL,started_at=NULL,crash_count=0,backoff_until=NULL,last_error_code=?2,last_error_detail=?3,updated_at=?4 WHERE plugin_id=?5",
                params![state, error_code, error_detail, now, plugin_id],
            )?;
        }
        transaction.commit()?;
        Ok(changed)
    }

    pub fn pause_plugin_bindings(&self, plugin_id: &str) -> anyhow::Result<Vec<BindingFence>> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let fences = pause_plugin_bindings_in_transaction(&transaction, plugin_id)?;
        transaction.commit()?;
        Ok(fences)
    }
    pub fn plugin_runtime_launches(&self) -> anyhow::Result<Vec<PluginRuntimeLaunch>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare(
            "SELECT i.plugin_id,i.selected_version,v.manifest_json,i.install_root,v.entry_sha256,a.account_uuid,a.generation,i.requested_permissions_json,i.granted_permissions_json FROM plugin_installations i JOIN plugin_versions v ON v.plugin_id=i.plugin_id AND v.version=i.selected_version JOIN plugin_accounts a ON a.plugin_id=i.plugin_id JOIN plugin_runtime_state r ON r.plugin_id=i.plugin_id WHERE i.enabled_intent=1 AND r.state <> 'configuration-required' ORDER BY i.plugin_id",
        )?;
        let rows = statement.query_map([], |row| {
            let hash: Vec<u8> = row.get(4)?;
            Ok((
                PluginRuntimeLaunch {
                    plugin_id: row.get(0)?,
                    version: row.get(1)?,
                    manifest_json: row.get(2)?,
                    install_root: PathBuf::from(row.get::<_, String>(3)?),
                    entry_sha256: hash.try_into().map_err(|_| {
                        rusqlite::Error::InvalidColumnType(
                            4,
                            "entry_sha256".into(),
                            rusqlite::types::Type::Blob,
                        )
                    })?,
                    account_uuid: row.get(5)?,
                    account_generation: row.get(6)?,
                    granted_permissions: Vec::new(),
                },
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?;
        let mut launches = Vec::new();
        for row in rows {
            let (launch, requested, granted) = row?;
            let requested_permissions =
                serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(&requested)?;
            let granted_permissions =
                serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(&granted)?;
            if permissions_match(&requested_permissions, &granted_permissions) {
                let mut launch = launch;
                launch.granted_permissions = granted_permissions;
                launches.push(launch);
            }
        }
        Ok(launches)
    }

    pub fn register_bundled_plugin(&self, plugin: RegisterBundledPlugin<'_>) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let now = unix_timestamp_millis();
        let existing: Option<(String, String)> = transaction
            .query_row(
                "SELECT selected_version,install_root FROM plugin_installations WHERE plugin_id=?1",
                [plugin.plugin_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let is_new = existing.is_none();
        let is_selected = existing
            .as_ref()
            .is_none_or(|(selected_version, _)| selected_version == plugin.version);
        if let Some((selected_version, install_root)) = &existing {
            if Path::new(install_root) != plugin.install_root {
                anyhow::bail!("plugin-install-root-changed");
            }
            if selected_version != plugin.version {
                let selected_entry: String = transaction.query_row(
                    "SELECT entry_relative_path FROM plugin_versions WHERE plugin_id=?1 AND version=?2",
                    params![plugin.plugin_id, selected_version],
                    |row| row.get(0),
                )?;
                if selected_entry == plugin.entry_relative_path {
                    anyhow::bail!("plugin-version-entry-must-be-side-by-side");
                }
            }
        }
        if is_new {
            transaction.execute(
                "INSERT INTO plugin_installations(plugin_id,manifest_version,selected_version,enabled_intent,install_root,trust_policy_id,requested_permissions_json,granted_permissions_json,created_at,updated_at) VALUES (?1,2,?2,0,?3,?4,?5,'[]',?6,?6)",
                params![plugin.plugin_id,plugin.version,plugin.install_root.to_string_lossy(),plugin.trust_policy_id,plugin.requested_permissions_json,now],
            )?;
        } else if is_selected {
            transaction.execute(
                "UPDATE plugin_installations SET manifest_version=2,trust_policy_id=?1,requested_permissions_json=?2,updated_at=?3 WHERE plugin_id=?4",
                params![plugin.trust_policy_id,plugin.requested_permissions_json,now,plugin.plugin_id],
            )?;
        }
        transaction.execute(
            "INSERT INTO plugin_versions(plugin_id,version,manifest_json,entry_relative_path,entry_sha256,state,installed_at,last_verified_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?7) ON CONFLICT(plugin_id,version) DO UPDATE SET manifest_json=excluded.manifest_json,entry_relative_path=excluded.entry_relative_path,entry_sha256=excluded.entry_sha256,last_verified_at=excluded.last_verified_at",
            params![plugin.plugin_id,plugin.version,plugin.manifest_json,plugin.entry_relative_path,plugin.entry_sha256.as_slice(),if is_selected { "selected" } else { "candidate" },now],
        )?;
        transaction.execute(
            "INSERT INTO plugin_runtime_state(plugin_id,state,capabilities_json,crash_count,updated_at) VALUES (?1,'disabled','[]',0,?2) ON CONFLICT(plugin_id) DO NOTHING",
            params![plugin.plugin_id,now],
        )?;
        if is_selected {
            let (enabled, granted_json): (bool, String) = transaction.query_row(
                "SELECT enabled_intent,granted_permissions_json FROM plugin_installations WHERE plugin_id=?1",
                [plugin.plugin_id],
                |row| Ok((row.get::<_, i64>(0)? != 0, row.get(1)?)),
            )?;
            let requested = serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(
                plugin.requested_permissions_json,
            )?;
            let granted =
                serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(&granted_json)?;
            if enabled && !permissions_match(&requested, &granted) {
                transaction.execute(
                    "UPDATE plugin_runtime_state SET state='permission-required',pid=NULL,session_id=NULL,started_at=NULL,backoff_until=NULL,last_error_code=NULL,last_error_detail=NULL,updated_at=?1 WHERE plugin_id=?2",
                    params![now, plugin.plugin_id],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn set_plugin_grants(
        &self,
        plugin_id: &str,
        granted_permissions_json: &str,
        secret_store_available: bool,
    ) -> anyhow::Result<Vec<BindingFence>> {
        let granted = serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(
            granted_permissions_json,
        )?;
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let requested_json: String = transaction.query_row(
            "SELECT requested_permissions_json FROM plugin_installations WHERE plugin_id=?1",
            [plugin_id],
            |row| row.get(0),
        )?;
        let requested =
            serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(&requested_json)?;
        let requires_secret = requested.iter().any(|permission| {
            matches!(
                permission,
                crate::plugins::manifest::Permission::Secret { .. }
            )
        });
        if granted
            .iter()
            .any(|permission| !requested.contains(permission))
            || granted
                .iter()
                .enumerate()
                .any(|(index, permission)| granted[..index].contains(permission))
        {
            anyhow::bail!("permission-not-requested");
        }
        let granted_permissions_json = serde_json::to_string(&granted)?;
        let now = unix_timestamp_millis();
        let changed = transaction.execute(
            "UPDATE plugin_installations SET granted_permissions_json=?1,updated_at=?2 WHERE plugin_id=?3",
            params![granted_permissions_json, now, plugin_id],
        )?;
        if changed != 1 {
            anyhow::bail!("plugin-not-found");
        }
        let enabled = transaction.query_row(
            "SELECT enabled_intent FROM plugin_installations WHERE plugin_id=?1",
            [plugin_id],
            |row| row.get::<_, i64>(0),
        )? != 0;
        let state = if !enabled {
            "disabled"
        } else if !permissions_match(&requested, &granted) {
            "permission-required"
        } else if requires_secret && !secret_store_available {
            "configuration-required"
        } else {
            "starting"
        };
        transaction.execute(
            "UPDATE plugin_runtime_state SET state=?1,pid=NULL,session_id=NULL,started_at=NULL,backoff_until=NULL,last_error_code=CASE WHEN ?1='configuration-required' THEN 'secret-store-unavailable' ELSE NULL END,last_error_detail=CASE WHEN ?1='configuration-required' THEN 'MEGLE_SECRET_KEY_FILE is missing or invalid' ELSE NULL END,updated_at=?2 WHERE plugin_id=?3",
            params![state, now, plugin_id],
        )?;
        let fences = pause_plugin_bindings_in_transaction(&transaction, plugin_id)?;
        transaction.commit()?;
        Ok(fences)
    }

    pub fn upsert_plugin_account(
        &self,
        account_uuid: &str,
        plugin_id: &str,
        display_name: Option<&str>,
        auth_state: &str,
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "INSERT INTO plugin_accounts(account_uuid,plugin_id,display_name,auth_state,generation,created_at,updated_at) VALUES (?1,?2,?3,?4,1,?5,?5) ON CONFLICT(account_uuid) DO UPDATE SET display_name=excluded.display_name,auth_state=excluded.auth_state,updated_at=excluded.updated_at",
            params![account_uuid,plugin_id,display_name,auth_state,now],
        )?;
        Ok(())
    }

    pub fn list_host_plugins(&self) -> anyhow::Result<Vec<HostPluginProjection>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare(
            "SELECT i.plugin_id,i.selected_version,i.known_good_version,i.enabled_intent,i.requested_permissions_json,i.granted_permissions_json,r.state,a.auth_state,a.account_uuid,a.display_name,r.backoff_until,r.last_healthy_at,r.last_error_code,r.last_error_detail,r.capabilities_json FROM plugin_installations i JOIN plugin_runtime_state r ON r.plugin_id=i.plugin_id LEFT JOIN plugin_accounts a ON a.plugin_id=i.plugin_id ORDER BY i.plugin_id",
        )?;
        let rows = statement.query_map([], |row| {
            let requested: String = row.get(4)?;
            let granted: String = row.get(5)?;
            Ok(HostPluginProjection {
                plugin_id: row.get(0)?,
                selected_version: row.get(1)?,
                known_good_version: row.get(2)?,
                enabled_intent: row.get::<_, i64>(3)? != 0,
                requested_permissions: serde_json::from_str(&requested)
                    .unwrap_or(serde_json::Value::Null),
                granted_permissions: serde_json::from_str(&granted)
                    .unwrap_or(serde_json::Value::Null),
                capabilities: serde_json::from_str(&row.get::<_, String>(14)?)
                    .unwrap_or_else(|_| serde_json::json!([])),
                permission_required: row.get::<_, String>(6)? == "permission-required",
                runtime_state: row.get(6)?,
                auth_state: row.get(7)?,
                account_uuid: row.get(8)?,
                account_display_name: row.get(9)?,
                backoff_until: row.get(10)?,
                last_healthy_at: row.get(11)?,
                last_error_code: row.get(12)?,
                last_error_detail: row.get(13)?,
                compatibility_reason: (row.get::<_, String>(6)? == "incompatible")
                    .then(|| row.get::<_, Option<String>>(13).ok().flatten())
                    .flatten(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn host_plugin(&self, plugin_id: &str) -> anyhow::Result<Option<HostPluginProjection>> {
        Ok(self
            .list_host_plugins()?
            .into_iter()
            .find(|plugin| plugin.plugin_id == plugin_id))
    }

    pub fn set_plugin_intent(
        &self,
        plugin_id: &str,
        enabled: bool,
        secret_store_available: bool,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE plugin_installations SET enabled_intent=?1,updated_at=?2 WHERE plugin_id=?3",
            params![enabled, unix_timestamp_millis(), plugin_id],
        )?;
        if changed != 1 {
            anyhow::bail!("plugin-not-found");
        }
        if enabled {
            let now = unix_timestamp_millis();
            transaction.execute(
                "INSERT INTO plugin_accounts(account_uuid,plugin_id,auth_state,generation,created_at,updated_at) SELECT ?1,?2,'authorization-required',1,?3,?3 WHERE NOT EXISTS(SELECT 1 FROM plugin_accounts WHERE plugin_id=?2)",
                params![uuid::Uuid::now_v7().to_string(),plugin_id,now],
            )?;
        }
        let (requested_json, granted_json): (String, String) = transaction.query_row(
            "SELECT requested_permissions_json,granted_permissions_json FROM plugin_installations WHERE plugin_id=?1",
            [plugin_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let requested =
            serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(&requested_json)?;
        let granted =
            serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(&granted_json)?;
        let requires_secret =
            serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(&requested_json)?
                .iter()
                .any(|permission| {
                    matches!(
                        permission,
                        crate::plugins::manifest::Permission::Secret { .. }
                    )
                });
        let state = if !enabled {
            "disabled"
        } else if permissions_match(&requested, &granted) {
            if requires_secret && !secret_store_available {
                "configuration-required"
            } else {
                "starting"
            }
        } else {
            "permission-required"
        };
        transaction.execute(
            "UPDATE plugin_runtime_state SET state=?1,pid=NULL,session_id=NULL,started_at=NULL,backoff_until=NULL,last_error_code=CASE WHEN ?1='configuration-required' THEN 'secret-store-unavailable' ELSE NULL END,last_error_detail=CASE WHEN ?1='configuration-required' THEN 'MEGLE_SECRET_KEY_FILE is missing or invalid' ELSE NULL END,updated_at=?2 WHERE plugin_id=?3",
            params![state, unix_timestamp_millis(), plugin_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn update_plugin_account_auth(
        &self,
        plugin_id: &str,
        auth_state: &str,
        display_name: Option<&str>,
    ) -> anyhow::Result<()> {
        let changed = self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "UPDATE plugin_accounts SET auth_state=?1,display_name=COALESCE(?2,display_name),last_error=NULL,updated_at=?3 WHERE plugin_id=?4",
            params![auth_state,display_name,unix_timestamp_millis(),plugin_id],
        )?;
        if changed != 1 {
            anyhow::bail!("plugin-account-not-found");
        }
        Ok(())
    }

    pub fn update_plugin_account_auth_for_session(
        &self,
        plugin_id: &str,
        expected_session: &str,
        auth_state: &str,
        display_name: Option<&str>,
    ) -> anyhow::Result<bool> {
        let changed = self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "UPDATE plugin_accounts SET auth_state=?1,display_name=COALESCE(?2,display_name),last_error=NULL,updated_at=?3 WHERE plugin_id=?4 AND EXISTS(SELECT 1 FROM plugin_runtime_state r WHERE r.plugin_id=plugin_accounts.plugin_id AND r.state='ready' AND r.session_id=?5)",
            params![auth_state,display_name,unix_timestamp_millis(),plugin_id,expected_session],
        )?;
        Ok(changed == 1)
    }

    pub fn update_plugin_account_identity_for_session(
        &self,
        plugin_id: &str,
        expected_session: &str,
        stable_identity: Option<&str>,
    ) -> anyhow::Result<Option<Vec<BindingFence>>> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let current: Option<String> = transaction.query_row(
            "SELECT stable_account_identity FROM plugin_accounts WHERE plugin_id=?1 AND EXISTS(SELECT 1 FROM plugin_runtime_state r WHERE r.plugin_id=plugin_accounts.plugin_id AND r.state='ready' AND r.session_id=?2)",
            params![plugin_id, expected_session],
            |row| row.get(0),
        ).optional()?.flatten();
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_accounts WHERE plugin_id=?1 AND EXISTS(SELECT 1 FROM plugin_runtime_state r WHERE r.plugin_id=plugin_accounts.plugin_id AND r.state='ready' AND r.session_id=?2))",
            params![plugin_id, expected_session],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(None);
        }
        let fences = if current.as_deref() != stable_identity && current.is_some() {
            let fences = pause_plugin_bindings_in_transaction(&transaction, plugin_id)?;
            transaction.execute(
                "DELETE FROM provider_binding_cursors WHERE library_uuid IN (SELECT library_uuid FROM library_sync_bindings WHERE plugin_id=?1)",
                [plugin_id],
            )?;
            transaction.execute(
                "DELETE FROM provider_account_root_cursors WHERE account_uuid=(SELECT account_uuid FROM plugin_accounts WHERE plugin_id=?1)",
                [plugin_id],
            )?;
            transaction.execute(
                "DELETE FROM provider_remote_libraries WHERE account_uuid=(SELECT account_uuid FROM plugin_accounts WHERE plugin_id=?1)",
                [plugin_id],
            )?;
            fences
        } else {
            Vec::new()
        };
        transaction.execute(
            "UPDATE plugin_accounts SET stable_account_identity=?1,updated_at=?2 WHERE plugin_id=?3",
            params![stable_identity, now, plugin_id],
        )?;
        transaction.commit()?;
        Ok(Some(fences))
    }

    pub fn retry_plugin_runtime(
        &self,
        plugin_id: &str,
        secret_store_available: bool,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let (enabled, requested_json, granted_json): (bool, String, String) = transaction
            .query_row(
                "SELECT enabled_intent,requested_permissions_json,granted_permissions_json FROM plugin_installations WHERE plugin_id=?1",
                [plugin_id],
                |row| Ok((row.get::<_, i64>(0)? != 0, row.get(1)?, row.get(2)?)),
            )?;
        if !enabled {
            anyhow::bail!("plugin-disabled");
        }
        let requested =
            serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(&requested_json)?;
        let granted =
            serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(&granted_json)?;
        if !permissions_match(&requested, &granted) {
            anyhow::bail!("plugin-permission-required");
        }
        if !secret_store_available
            && requested.iter().any(|permission| {
                matches!(
                    permission,
                    crate::plugins::manifest::Permission::Secret { .. }
                )
            })
        {
            anyhow::bail!("plugin-configuration-required");
        }
        transaction.execute("UPDATE plugin_runtime_state SET state='starting',crash_count=0,backoff_until=NULL,last_error_code=NULL,last_error_detail=NULL,updated_at=?1 WHERE plugin_id=?2",params![unix_timestamp_millis(),plugin_id])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn plugin_runtime_retry_requested(&self, plugin_id: &str) -> anyhow::Result<bool> {
        Ok(self.connection.lock().unwrap_or_else(|p| p.into_inner()).query_row(
            "SELECT state='starting' AND crash_count=0 FROM plugin_runtime_state WHERE plugin_id=?1",
            [plugin_id],
            |row| row.get::<_, i64>(0),
        ).optional()?.unwrap_or(0) != 0)
    }

    #[cfg(test)]
    pub fn record_plugin_runtime_ready(
        &self,
        plugin_id: &str,
        session_id: &str,
        capabilities: &[String],
    ) -> anyhow::Result<()> {
        let capabilities = serde_json::to_string(capabilities)?;
        let now = unix_timestamp_millis();
        let changed = self.connection.lock().unwrap_or_else(|p|p.into_inner()).execute(
            "UPDATE plugin_runtime_state SET state='ready',session_id=?1,capabilities_json=?2,last_healthy_at=?3,crash_count=0,backoff_until=NULL,last_error_code=NULL,last_error_detail=NULL,updated_at=?3 WHERE plugin_id=?4",
            params![session_id,capabilities,now,plugin_id],
        )?;
        if changed != 1 {
            anyhow::bail!("plugin-not-found")
        }
        Ok(())
    }

    pub fn record_plugin_runtime_ready_for_launch(
        &self,
        plugin_id: &str,
        expected_fingerprint: &str,
        session_id: &str,
        capabilities: &[String],
    ) -> anyhow::Result<bool> {
        let capabilities = serde_json::to_string(capabilities)?;
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if !plugin_runtime_launch_matches(&transaction, plugin_id, expected_fingerprint)? {
            return Ok(false);
        }
        let changed = transaction.execute(
            "UPDATE plugin_runtime_state SET state='ready',session_id=?1,capabilities_json=?2,last_healthy_at=?3,crash_count=0,backoff_until=NULL,last_error_code=NULL,last_error_detail=NULL,updated_at=?3 WHERE plugin_id=?4",
            params![session_id, capabilities, now, plugin_id],
        )?;
        if changed != 1 {
            return Ok(false);
        }
        transaction.commit()?;
        Ok(true)
    }

    pub fn record_plugin_runtime_failure(
        &self,
        plugin_id: &str,
        expected_fingerprint: &str,
        state: &str,
        error_code: &str,
        detail: &str,
        backoff_until: Option<i64>,
    ) -> anyhow::Result<bool> {
        if !matches!(
            state,
            "degraded" | "crashed-backoff" | "incompatible" | "configuration-required"
        ) {
            anyhow::bail!("invalid-plugin-runtime-failure-state")
        }
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if !plugin_runtime_launch_matches(&transaction, plugin_id, expected_fingerprint)? {
            return Ok(false);
        }
        let changed = transaction.execute(
            "UPDATE plugin_runtime_state SET state=?1,crash_count=crash_count+1,backoff_until=?2,last_error_code=?3,last_error_detail=?4,updated_at=?5 WHERE plugin_id=?6",
            params![state,backoff_until,error_code,detail,unix_timestamp_millis(),plugin_id],
        )?;
        if changed != 1 {
            return Ok(false);
        }
        transaction.commit()?;
        Ok(true)
    }

    pub fn plugin_has_pending_probation(&self, plugin_id: &str) -> anyhow::Result<bool> {
        Ok(self.connection.lock().unwrap_or_else(|p|p.into_inner()).query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_version_journal WHERE plugin_id=?1 AND phase='probation')",
            [plugin_id],
            |row| row.get::<_, i64>(0),
        )? != 0)
    }

    pub fn select_plugin_version(
        &self,
        plugin_id: &str,
        version: &str,
        secret_store_available: bool,
    ) -> anyhow::Result<Vec<BindingFence>> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let tx = connection.transaction()?;
        let exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM plugin_versions WHERE plugin_id=?1 AND version=?2",
            params![plugin_id, version],
            |r| r.get(0),
        )?;
        if exists != 1 {
            anyhow::bail!("plugin-version-not-found")
        }
        let (from_version, enabled, granted_json): (String, bool, String) = tx.query_row(
            "SELECT selected_version,enabled_intent,granted_permissions_json FROM plugin_installations WHERE plugin_id=?1",
            [plugin_id],
            |row| Ok((row.get(0)?, row.get::<_, i64>(1)? != 0, row.get(2)?)),
        )?;
        if from_version == version {
            return Ok(Vec::new());
        }
        let switch_in_progress = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM plugin_version_journal WHERE plugin_id=?1 AND phase IN ('verifying','draining','probation'))",
            [plugin_id],
            |row| Ok(row.get::<_, i64>(0)? != 0),
        )?;
        if switch_in_progress {
            anyhow::bail!("plugin-version-switch-in-progress");
        }
        let journal_uuid = uuid::Uuid::now_v7().to_string();
        let now = unix_timestamp_millis();
        let manifest_json: String = tx.query_row(
            "SELECT manifest_json FROM plugin_versions WHERE plugin_id=?1 AND version=?2",
            params![plugin_id, version],
            |row| row.get(0),
        )?;
        let manifest: crate::plugins::manifest::ManifestRecord =
            serde_json::from_str(&manifest_json)?;
        let requested = manifest.requested_permissions;
        let requested_json = serde_json::to_string(&requested)?;
        let granted =
            serde_json::from_str::<Vec<crate::plugins::manifest::Permission>>(&granted_json)?;
        let requires_secret = requested.iter().any(|permission| {
            matches!(
                permission,
                crate::plugins::manifest::Permission::Secret { .. }
            )
        });
        let runtime_state = if !enabled {
            "disabled"
        } else if !permissions_match(&requested, &granted) {
            "permission-required"
        } else if requires_secret && !secret_store_available {
            "configuration-required"
        } else {
            "starting"
        };
        tx.execute(
            "INSERT INTO plugin_version_journal(journal_uuid,plugin_id,from_version,to_version,phase,created_at,updated_at) VALUES (?1,?2,?3,?4,'probation',?5,?5)",
            params![journal_uuid,plugin_id,from_version,version,now],
        )?;
        tx.execute("UPDATE plugin_versions SET state=CASE WHEN version=?1 THEN 'candidate' WHEN state='selected' THEN 'known-good' ELSE state END WHERE plugin_id=?2",params![version,plugin_id])?;
        tx.execute("UPDATE plugin_installations SET known_good_version=COALESCE(known_good_version,selected_version),selected_version=?1,requested_permissions_json=?2,updated_at=?3 WHERE plugin_id=?4",params![version,requested_json,now,plugin_id])?;
        tx.execute(
            "UPDATE plugin_runtime_state SET state=?1,pid=NULL,session_id=NULL,started_at=NULL,backoff_until=NULL,last_error_code=CASE WHEN ?1='configuration-required' THEN 'secret-store-unavailable' ELSE NULL END,last_error_detail=CASE WHEN ?1='configuration-required' THEN 'MEGLE_SECRET_KEY_FILE is missing or invalid' ELSE NULL END,updated_at=?2 WHERE plugin_id=?3",
            params![runtime_state, now, plugin_id],
        )?;
        let fences = pause_plugin_bindings_in_transaction(&tx, plugin_id)?;
        tx.commit()?;
        Ok(fences)
    }

    pub fn rollback_plugin_version(&self, plugin_id: &str) -> anyhow::Result<Vec<BindingFence>> {
        let known = self
            .connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .query_row(
                "SELECT known_good_version FROM plugin_installations WHERE plugin_id=?1",
                [plugin_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("plugin-not-found"))?
            .ok_or_else(|| anyhow::anyhow!("plugin-known-good-missing"))?;
        let fences = self.rollback_plugin_probation(plugin_id, "manual-rollback")?;
        let selected = self
            .host_plugin(plugin_id)?
            .ok_or_else(|| anyhow::anyhow!("plugin-not-found"))?
            .selected_version;
        if selected != known {
            anyhow::bail!("plugin-known-good-rollback-failed")
        }
        Ok(fences)
    }

    /// Marks a candidate healthy only after the supervisor probation window.
    /// The version switch and durable journal terminal state share one commit.
    pub fn commit_plugin_probation(&self, plugin_id: &str) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let tx = connection.transaction()?;
        commit_plugin_probation_in_transaction(&tx, plugin_id)?;
        tx.commit()?;
        Ok(())
    }

    pub fn commit_plugin_probation_for_launch(
        &self,
        plugin_id: &str,
        expected_fingerprint: &str,
    ) -> anyhow::Result<bool> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let tx = connection.transaction()?;
        if !plugin_runtime_launch_matches(&tx, plugin_id, expected_fingerprint)? {
            return Ok(false);
        }
        let committed = commit_plugin_probation_in_transaction(&tx, plugin_id)?;
        tx.commit()?;
        Ok(committed)
    }

    /// Fail closed on a candidate crash/health failure. This is idempotent and
    /// is also used during Host startup recovery for interrupted probation.
    pub fn rollback_plugin_probation(
        &self,
        plugin_id: &str,
        error_code: &str,
    ) -> anyhow::Result<Vec<BindingFence>> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let tx = connection.transaction()?;
        let fences = rollback_plugin_probation_in_transaction(&tx, plugin_id, error_code)?;
        tx.commit()?;
        Ok(fences)
    }

    pub fn rollback_plugin_probation_for_launch(
        &self,
        plugin_id: &str,
        expected_fingerprint: &str,
        error_code: &str,
    ) -> anyhow::Result<Option<Vec<BindingFence>>> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let tx = connection.transaction()?;
        if !plugin_runtime_launch_matches(&tx, plugin_id, expected_fingerprint)? {
            return Ok(None);
        }
        let fences = rollback_plugin_probation_in_transaction(&tx, plugin_id, error_code)?;
        tx.commit()?;
        Ok(Some(fences))
    }

    pub fn recover_plugin_version_journal(&self) -> anyhow::Result<usize> {
        let plugin_ids = {
            let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
            let mut statement = connection.prepare("SELECT DISTINCT plugin_id FROM plugin_version_journal WHERE phase IN ('verifying','draining','probation')")?;
            let values = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            values
        };
        for plugin_id in &plugin_ids {
            self.rollback_plugin_probation(plugin_id, "startup-recovery")?;
        }
        Ok(plugin_ids.len())
    }

    pub fn unregister_plugin(&self, plugin_id: &str) -> anyhow::Result<()> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let refs:i64=connection.query_row("SELECT (SELECT COUNT(*) FROM plugin_accounts WHERE plugin_id=?1)+(SELECT COUNT(*) FROM library_sync_bindings WHERE plugin_id=?1)",[plugin_id],|r|r.get(0))?;
        if refs > 0 {
            anyhow::bail!("plugin-in-use")
        }
        let changed = connection.execute(
            "DELETE FROM plugin_installations WHERE plugin_id=?1",
            [plugin_id],
        )?;
        if changed != 1 {
            anyhow::bail!("plugin-not-found")
        }
        Ok(())
    }

    pub fn binding_transport(
        &self,
        library_uuid: &str,
        scope: &str,
    ) -> anyhow::Result<Option<BindingTransport>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        connection.query_row(
            "SELECT b.plugin_id,b.account_uuid,b.binding_epoch,b.actor_epoch,b.transport_generation,b.state,b.remote_namespace,c.cursor,c.page_token,b.latest_checkpoint_uuid FROM library_sync_bindings b LEFT JOIN provider_binding_cursors c ON c.library_uuid=b.library_uuid AND c.binding_epoch=b.binding_epoch AND c.scope=?2 WHERE b.library_uuid=?1",
            params![library_uuid,scope],
            |row| {
                let epoch: Vec<u8> = row.get(2)?;
                let binding_epoch = uuid::Uuid::from_slice(&epoch).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Blob, Box::new(error))
                })?;
                let actor: Vec<u8> = row.get(3)?;
                let actor_epoch = uuid::Uuid::from_slice(&actor).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Blob, Box::new(error))
                })?;
                let checkpoint: Option<Vec<u8>> = row.get(9)?;
                Ok(BindingTransport {
                    plugin_id: row.get(0)?,
                    account_uuid: row.get(1)?,
                    binding_epoch,
                    actor_epoch,
                    transport_generation: row.get(4)?,
                    state: row.get(5)?,
                    remote_namespace: row.get(6)?,
                    cursor: row.get(7)?,
                    page_token: row.get(8)?,
                    latest_checkpoint_uuid: checkpoint
                        .map(|value| uuid::Uuid::from_slice(&value))
                        .transpose()
                        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Blob, Box::new(error)))?,
                })
            },
        ).optional().map_err(Into::into)
    }

    pub fn provider_root_transports(&self) -> anyhow::Result<Vec<ProviderRootTransport>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare(
            "SELECT a.plugin_id,a.account_uuid,a.generation,c.cursor,c.page_token FROM plugin_accounts a JOIN plugin_installations i ON i.plugin_id=a.plugin_id JOIN plugin_runtime_state r ON r.plugin_id=a.plugin_id LEFT JOIN provider_account_root_cursors c ON c.account_uuid=a.account_uuid AND c.scope='root' WHERE i.enabled_intent=1 AND r.state='ready' AND a.auth_state='authenticated' AND EXISTS (SELECT 1 FROM json_each(r.capabilities_json) WHERE value='sync-provider.v1') ORDER BY a.plugin_id",
        )?;
        let transports = statement
            .query_map([], |row| {
                Ok(ProviderRootTransport {
                    plugin_id: row.get(0)?,
                    account_uuid: row.get(1)?,
                    account_generation: row.get(2)?,
                    cursor: row.get(3)?,
                    page_token: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(transports)
    }

    pub fn commit_provider_root_page(
        &self,
        account_uuid: &str,
        account_generation: i64,
        cursor: Option<&str>,
        page_token: &str,
        libraries: &[super::sync::provider::ObjectMetadata],
    ) -> anyhow::Result<()> {
        self.commit_provider_root_progress(
            account_uuid,
            account_generation,
            cursor,
            Some(page_token),
            libraries,
        )
    }

    pub fn commit_provider_root_cursor(
        &self,
        account_uuid: &str,
        account_generation: i64,
        cursor: Option<&str>,
        libraries: &[super::sync::provider::ObjectMetadata],
    ) -> anyhow::Result<()> {
        self.commit_provider_root_progress(
            account_uuid,
            account_generation,
            cursor,
            None,
            libraries,
        )
    }

    fn commit_provider_root_progress(
        &self,
        account_uuid: &str,
        account_generation: i64,
        cursor: Option<&str>,
        page_token: Option<&str>,
        libraries: &[super::sync::provider::ObjectMetadata],
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let current: Option<i64> = transaction
            .query_row(
                "SELECT generation FROM plugin_accounts WHERE account_uuid=?1",
                [account_uuid],
                |row| row.get(0),
            )
            .optional()?;
        if current != Some(account_generation) {
            anyhow::bail!("stale-provider-account-generation");
        }
        for metadata in libraries {
            let library_uuid = metadata
                .key
                .strip_suffix("/library-descriptor")
                .ok_or_else(|| anyhow::anyhow!("invalid provider root descriptor key"))?;
            let canonical = uuid::Uuid::parse_str(library_uuid)?;
            if canonical.to_string() != library_uuid {
                anyhow::bail!("non-canonical provider root Library UUID");
            }
            transaction.execute(
                "INSERT INTO provider_remote_libraries(account_uuid,library_uuid,descriptor_size,descriptor_sha256,descriptor_etag,account_generation,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(account_uuid,library_uuid) DO UPDATE SET descriptor_size=excluded.descriptor_size,descriptor_sha256=excluded.descriptor_sha256,descriptor_etag=excluded.descriptor_etag,account_generation=excluded.account_generation,updated_at=excluded.updated_at",
                params![account_uuid,library_uuid,metadata.size as i64,metadata.sha256.as_slice(),metadata.etag,account_generation,now],
            )?;
        }
        transaction.execute(
            "INSERT INTO provider_account_root_cursors(account_uuid,scope,cursor,page_token,generation,updated_at) VALUES (?1,'root',?2,?3,?4,?5) ON CONFLICT(account_uuid,scope) DO UPDATE SET cursor=excluded.cursor,page_token=excluded.page_token,generation=excluded.generation,updated_at=excluded.updated_at",
            params![account_uuid,cursor,page_token,account_generation,now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn commit_binding_cursor(
        &self,
        library_uuid: &str,
        binding_epoch: uuid::Uuid,
        transport_generation: i64,
        remote_namespace: &str,
        scope: &str,
        cursor: Option<&str>,
    ) -> anyhow::Result<()> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let current: Option<i64> = connection.query_row(
            "SELECT transport_generation FROM library_sync_bindings WHERE library_uuid=?1 AND binding_epoch=?2",
            params![library_uuid,binding_epoch.as_bytes().as_slice()],
            |row| row.get(0),
        ).optional()?;
        if current != Some(transport_generation) {
            anyhow::bail!("stale-provider-transport-generation");
        }
        connection.execute(
            "INSERT INTO provider_binding_cursors(library_uuid,binding_epoch,remote_namespace,scope,cursor,page_token,transport_generation,updated_at) VALUES (?1,?2,?3,?4,?5,NULL,?6,?7) ON CONFLICT(library_uuid,binding_epoch,scope) DO UPDATE SET remote_namespace=excluded.remote_namespace,cursor=excluded.cursor,page_token=NULL,transport_generation=excluded.transport_generation,updated_at=excluded.updated_at",
            params![library_uuid,binding_epoch.as_bytes().as_slice(),remote_namespace,scope,cursor,transport_generation,unix_timestamp_millis()],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_binding_page(
        &self,
        library_uuid: &str,
        binding_epoch: uuid::Uuid,
        transport_generation: i64,
        remote_namespace: &str,
        scope: &str,
        cursor: Option<&str>,
        page_token: &str,
    ) -> anyhow::Result<()> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let current: Option<i64> = connection
            .query_row(
                "SELECT transport_generation FROM library_sync_bindings WHERE library_uuid=?1 AND binding_epoch=?2",
                params![library_uuid,binding_epoch.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()?;
        if current != Some(transport_generation) {
            anyhow::bail!("stale-provider-transport-generation");
        }
        connection.execute(
            "INSERT INTO provider_binding_cursors(library_uuid,binding_epoch,remote_namespace,scope,cursor,page_token,transport_generation,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(library_uuid,binding_epoch,scope) DO UPDATE SET remote_namespace=excluded.remote_namespace,cursor=excluded.cursor,page_token=excluded.page_token,transport_generation=excluded.transport_generation,updated_at=excluded.updated_at",
            params![library_uuid,binding_epoch.as_bytes().as_slice(),remote_namespace,scope,cursor,page_token,transport_generation,unix_timestamp_millis()],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_provider_receipt(
        &self,
        library_uuid: &str,
        binding_epoch: uuid::Uuid,
        transport_generation: i64,
        object_key: &str,
        etag: Option<&str>,
        sha256: &[u8; 32],
        state: &str,
    ) -> anyhow::Result<()> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let current: Option<i64> = connection.query_row(
            "SELECT transport_generation FROM library_sync_bindings WHERE library_uuid=?1 AND binding_epoch=?2",
            params![library_uuid,binding_epoch.as_bytes().as_slice()],
            |row| row.get(0),
        ).optional()?;
        if current != Some(transport_generation) {
            anyhow::bail!("stale-provider-transport-generation");
        }
        let now = unix_timestamp_millis();
        connection.execute(
            "INSERT INTO provider_object_receipts(library_uuid,binding_epoch,object_key,etag,sha256,state,generation,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8) ON CONFLICT(library_uuid,binding_epoch,object_key) DO UPDATE SET etag=excluded.etag,sha256=excluded.sha256,state=excluded.state,generation=excluded.generation,updated_at=excluded.updated_at",
            params![library_uuid,binding_epoch.as_bytes().as_slice(),object_key,etag,sha256.as_slice(),state,transport_generation,now],
        )?;
        Ok(())
    }

    pub fn advance_published_frontier(
        &self,
        library_uuid: &str,
        binding_epoch: uuid::Uuid,
        transport_generation: i64,
        published_through_seq: u64,
        object_key: &str,
    ) -> anyhow::Result<()> {
        let changed = self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "UPDATE library_sync_bindings SET published_through_seq=MAX(published_through_seq,?1),updated_at=?2 WHERE library_uuid=?3 AND binding_epoch=?4 AND transport_generation=?5 AND EXISTS (SELECT 1 FROM provider_object_receipts r WHERE r.library_uuid=library_sync_bindings.library_uuid AND r.binding_epoch=library_sync_bindings.binding_epoch AND r.generation=?5 AND r.object_key=?6 AND r.state IN ('created','already-exists-matching'))",
            params![published_through_seq as i64,unix_timestamp_millis(),library_uuid,binding_epoch.as_bytes().as_slice(),transport_generation,object_key],
        )?;
        if changed != 1 {
            anyhow::bail!("published frontier generation or receipt fence failed");
        }
        Ok(())
    }

    pub fn begin_unbind(
        &self,
        library_uuid: &str,
        binding_epoch: uuid::Uuid,
    ) -> anyhow::Result<i64> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let generation: i64 = transaction.query_row(
            "SELECT transport_generation FROM library_sync_bindings WHERE library_uuid=?1 AND binding_epoch=?2",
            params![library_uuid,binding_epoch.as_bytes().as_slice()],
            |row| row.get(0),
        )?;
        let next = generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("transport generation exhausted"))?;
        transaction.execute(
            "UPDATE library_sync_bindings SET state='unbinding',transport_generation=?1,updated_at=?2 WHERE library_uuid=?3 AND binding_epoch=?4",
            params![next,unix_timestamp_millis(),library_uuid,binding_epoch.as_bytes().as_slice()],
        )?;
        transaction.commit()?;
        Ok(next)
    }
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
            "SELECT l.library_uuid,l.root_uuid,l.name,l.icon,l.color,l.local_data_dir,l.managed_root,l.lifecycle_state,CASE WHEN b.state IN ('preparing','capturing','seed_ready','publishing') THEN 'synchronizing' WHEN b.state IN ('active','paused') THEN 'synchronized' ELSE l.binding_state END,l.availability,l.marker_identity FROM libraries l LEFT JOIN library_sync_bindings b ON b.library_uuid=l.library_uuid WHERE l.library_uuid=?1",
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
        let mut statement=connection.prepare("SELECT l.library_uuid,l.root_uuid,l.name,l.icon,l.color,l.local_data_dir,l.managed_root,l.lifecycle_state,CASE WHEN b.state IN ('preparing','capturing','seed_ready','publishing') THEN 'synchronizing' ELSE 'synchronized' END,l.availability,l.marker_identity FROM libraries l JOIN library_sync_bindings b ON b.library_uuid=l.library_uuid WHERE b.state IN ('preparing','capturing','seed_ready','publishing','active','paused') ORDER BY l.library_uuid")?;
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
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 3 {
            connection.execute_batch(HOST_MIGRATION_0003)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 4 {
            connection.execute_batch(HOST_MIGRATION_0004)?;
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
        plugin_id: &str,
        account_uuid: &str,
    ) -> anyhow::Result<(uuid::Uuid, uuid::Uuid)> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let now = unix_timestamp_millis();
        let provider_ready: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM plugin_installations i JOIN plugin_runtime_state r ON r.plugin_id=i.plugin_id JOIN plugin_accounts a ON a.plugin_id=i.plugin_id WHERE i.plugin_id=?1 AND a.account_uuid=?2 AND i.enabled_intent=1 AND r.state='ready' AND a.auth_state='authenticated' AND EXISTS (SELECT 1 FROM json_each(r.capabilities_json) WHERE value='sync-provider.v1')",
            params![plugin_id, account_uuid],
            |row| row.get(0),
        )?;
        if provider_ready != 1 {
            anyhow::bail!("provider-not-ready");
        }
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
            "INSERT INTO library_sync_bindings(library_uuid,plugin_id,account_uuid,binding_epoch,actor_epoch,state,transport_generation,cutover_actor_seq,published_through_seq,capture_generation,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,'preparing',1,0,0,NULL,?6,?6)",
            params![library_uuid,plugin_id,account_uuid,binding.as_bytes().as_slice(),catalog_actor.as_bytes().as_slice(),now],
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
            "SELECT capture_generation FROM library_sync_bindings WHERE library_uuid=?1 AND state IN ('preparing','capturing','seed_ready','publishing')",
            [library_uuid],|row|row.get(0),
        ).optional()?)
    }

    /// Descriptor publication is fenced on a durable, validated Catalog
    /// checkpoint. Provider code may call this only after the Catalog gate has
    /// succeeded; Host then atomically exposes the seed identity.
    pub fn mark_local_seed_ready(
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
            "UPDATE library_sync_bindings SET state='seed_ready',latest_checkpoint_uuid=?1,capture_generation=?2,updated_at=?3 WHERE library_uuid=?4 AND state IN ('preparing','capturing') AND capture_generation=?2",
            params![checkpoint_uuid.as_bytes().as_slice(),capture_generation,unix_timestamp_millis(),library_uuid],
        )?;
        if changed != 1 {
            anyhow::bail!("sync binding is not awaiting a seed checkpoint");
        }
        Ok(())
    }

    pub fn confirm_remote_descriptor(
        &self,
        library_uuid: &str,
        binding_epoch: uuid::Uuid,
        transport_generation: i64,
        checkpoint_key: &str,
    ) -> anyhow::Result<()> {
        let changed = self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "UPDATE library_sync_bindings SET state='active',updated_at=?1 WHERE library_uuid=?2 AND binding_epoch=?3 AND transport_generation=?4 AND state IN ('seed_ready','publishing') AND EXISTS (SELECT 1 FROM provider_object_receipts r WHERE r.library_uuid=library_sync_bindings.library_uuid AND r.binding_epoch=library_sync_bindings.binding_epoch AND r.object_key='library-descriptor' AND r.generation=?4 AND r.state IN ('created','already-exists-matching')) AND EXISTS (SELECT 1 FROM provider_object_receipts r WHERE r.library_uuid=library_sync_bindings.library_uuid AND r.binding_epoch=library_sync_bindings.binding_epoch AND r.object_key=?5 AND r.generation=?4 AND r.state IN ('created','already-exists-matching'))",
            params![unix_timestamp_millis(),library_uuid,binding_epoch.as_bytes().as_slice(),transport_generation,checkpoint_key],
        )?;
        if changed != 1 {
            anyhow::bail!("remote descriptor receipt is not durable");
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
            "SELECT l.library_uuid,l.root_uuid,l.name,l.icon,l.color,l.local_data_dir,l.managed_root,l.lifecycle_state,CASE WHEN b.state IN ('preparing','capturing','seed_ready','publishing') THEN 'synchronizing' WHEN b.state IN ('active','paused') THEN 'synchronized' ELSE l.binding_state END,l.availability,l.marker_identity FROM libraries l LEFT JOIN library_sync_bindings b ON b.library_uuid=l.library_uuid ORDER BY COALESCE(l.last_opened_at,l.created_at) DESC,l.created_at DESC",
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
                "SELECT l.library_uuid,l.root_uuid,l.name,l.icon,l.color,l.local_data_dir,l.managed_root,l.lifecycle_state,CASE WHEN b.state IN ('preparing','capturing','seed_ready','publishing') THEN 'synchronizing' WHEN b.state IN ('active','paused') THEN 'synchronized' ELSE l.binding_state END,l.availability,l.marker_identity FROM libraries l LEFT JOIN library_sync_bindings b ON b.library_uuid=l.library_uuid WHERE l.library_uuid=?1",
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
            "SELECT binding_epoch FROM library_sync_bindings WHERE library_uuid=?1 AND state IN ('preparing','capturing','seed_ready','publishing','active','paused')",
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

pub(crate) fn unix_timestamp_millis_for_runtime() -> i64 {
    unix_timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn register_test_plugin(registry: &HostRegistry, plugin_id: &str, account_uuid: &str) {
        registry
            .register_bundled_plugin(RegisterBundledPlugin {
                plugin_id,
                version: "1.0.0",
                manifest_json: r#"{"manifestVersion":2}"#,
                entry_relative_path: "bin/provider",
                entry_sha256: &[7; 32],
                install_root: Path::new("bundled"),
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();
        registry
            .upsert_plugin_account(account_uuid, plugin_id, Some("Test"), "authenticated")
            .unwrap();
        registry.set_plugin_intent(plugin_id, true, true).unwrap();
        registry
            .record_plugin_runtime_ready(plugin_id, "test-session", &["sync-provider.v1".into()])
            .unwrap();
    }

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
    fn sync_enable_requires_ready_authenticated_compatible_provider_account() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).expect("open host registry");
        register_test_plugin(&registry, "com.megle.ready", "account-ready");
        registry
            .set_plugin_intent("com.megle.ready", true, true)
            .unwrap();
        registry
            .record_plugin_runtime_ready(
                "com.megle.ready",
                "session-ready",
                &["sync-provider.v1".into()],
            )
            .unwrap();

        registry
            .require_ready_sync_provider("com.megle.ready", "account-ready")
            .unwrap();
        assert_eq!(
            registry
                .require_ready_sync_provider("com.megle.ready", "wrong-account")
                .unwrap_err()
                .to_string(),
            "provider-account-not-found"
        );

        registry
            .update_plugin_account_auth("com.megle.ready", "expired", None)
            .unwrap();
        assert_eq!(
            registry
                .require_ready_sync_provider("com.megle.ready", "account-ready")
                .unwrap_err()
                .to_string(),
            "provider-account-not-authenticated"
        );
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stale_runtime_launch_cannot_restore_ready_after_disable() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).expect("open host registry");
        register_test_plugin(&registry, "com.megle.stale", "account-stale");
        let fingerprint = registry.plugin_runtime_launches().unwrap()[0].fingerprint();
        assert!(registry
            .plugin_runtime_launch_is_current("com.megle.stale", &fingerprint)
            .unwrap());

        registry
            .set_plugin_intent("com.megle.stale", false, true)
            .unwrap();
        assert!(!registry
            .plugin_runtime_launch_is_current("com.megle.stale", &fingerprint)
            .unwrap());
        assert!(!registry
            .record_plugin_runtime_ready_for_launch(
                "com.megle.stale",
                &fingerprint,
                "stale-session",
                &["sync-provider.v1".into()],
            )
            .unwrap());
        let plugin = registry.host_plugin("com.megle.stale").unwrap().unwrap();
        assert_eq!(plugin.runtime_state, "disabled");
        assert!(!plugin.enabled_intent);
        assert!(!registry
            .record_plugin_runtime_failure(
                "com.megle.stale",
                &fingerprint,
                "crashed-backoff",
                "stale-failure",
                "an old supervisor failed after disable",
                None,
            )
            .unwrap());
        assert_eq!(
            registry
                .host_plugin("com.megle.stale")
                .unwrap()
                .unwrap()
                .runtime_state,
            "disabled"
        );

        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn signed_out_account_gets_a_new_runtime_generation_for_reauthentication() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).expect("open host registry");
        register_test_plugin(&registry, "com.megle.reauth", "account-reauth");
        let original = registry.plugin_runtime_launches().unwrap()[0].fingerprint();

        registry
            .set_plugin_account_secret("com.megle.reauth", None, "signed-out", 2)
            .unwrap();
        let launches = registry.plugin_runtime_launches().unwrap();
        assert_eq!(launches.len(), 1);
        let reauthentication = launches[0].fingerprint();
        assert_ne!(original, reauthentication);
        assert!(!registry
            .plugin_runtime_launch_is_current("com.megle.reauth", &original)
            .unwrap());
        assert!(registry
            .plugin_runtime_launch_is_current("com.megle.reauth", &reauthentication)
            .unwrap());

        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pausing_provider_bindings_returns_rpc_fences_after_generation_advance() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).expect("open host registry");
        register_test_plugin(&registry, "com.megle.fenced", "account-fenced");
        let now = unix_timestamp_millis();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES ('library-fenced','root-fenced','Library','local','root','ready','local-only','available',1,'marker',?1,?1)",
            [now],
        ).unwrap();
        let (binding_epoch, _) = registry
            .begin_sync_binding("library-fenced", "com.megle.fenced", "account-fenced")
            .unwrap();

        let fences = registry.pause_plugin_bindings("com.megle.fenced").unwrap();
        assert_eq!(fences.len(), 1);
        assert_eq!(fences[0].account_uuid, "account-fenced");
        assert_eq!(fences[0].binding_epoch, binding_epoch);
        assert_eq!(fences[0].next_transport_generation, 2);
        let binding = registry
            .binding_transport("library-fenced", "library")
            .unwrap()
            .unwrap();
        assert_eq!(binding.state, "paused");
        assert_eq!(binding.transport_generation, 2);
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn plugin_intent_and_grants_remain_independent() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let permissions = r#"[{"kind":"secret","namespace":"fake-sync","operations":["read"]}]"#;
        registry
            .register_bundled_plugin(RegisterBundledPlugin {
                plugin_id: "com.megle.permissions",
                version: "1.0.0",
                manifest_json: r#"{"manifestVersion":2}"#,
                entry_relative_path: "bin/provider",
                entry_sha256: &[7; 32],
                install_root: Path::new("bundled"),
                trust_policy_id: "test",
                requested_permissions_json: permissions,
            })
            .unwrap();
        registry
            .set_plugin_intent("com.megle.permissions", true, true)
            .unwrap();
        assert_eq!(
            registry
                .host_plugin("com.megle.permissions")
                .unwrap()
                .unwrap()
                .runtime_state,
            "permission-required"
        );
        registry
            .set_plugin_grants("com.megle.permissions", permissions, false)
            .unwrap();
        let plugin = registry
            .host_plugin("com.megle.permissions")
            .unwrap()
            .unwrap();
        assert!(plugin.enabled_intent);
        assert_eq!(plugin.runtime_state, "configuration-required");
        registry
            .reset_plugin_runtime_states_for_startup(true)
            .unwrap();
        assert_eq!(
            registry
                .host_plugin("com.megle.permissions")
                .unwrap()
                .unwrap()
                .runtime_state,
            "starting"
        );
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rediscovery_does_not_activate_a_candidate_version() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "com.megle.versioned-discovery", "account");
        registry
            .register_bundled_plugin(RegisterBundledPlugin {
                plugin_id: "com.megle.versioned-discovery",
                version: "2.0.0",
                manifest_json: r#"{"manifestVersion":2}"#,
                entry_relative_path: "bin/provider-v2",
                entry_sha256: &[8; 32],
                install_root: Path::new("bundled"),
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();
        let plugin = registry
            .host_plugin("com.megle.versioned-discovery")
            .unwrap()
            .unwrap();
        assert_eq!(plugin.selected_version, "1.0.0");
        let state: String = registry
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT state FROM plugin_versions WHERE plugin_id='com.megle.versioned-discovery' AND version='2.0.0'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "candidate");
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rediscovery_of_selected_version_requires_new_permissions() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "com.megle.permission-update", "account");
        registry
            .set_plugin_intent("com.megle.permission-update", true, true)
            .unwrap();
        registry
            .register_bundled_plugin(RegisterBundledPlugin {
                plugin_id: "com.megle.permission-update",
                version: "1.0.0",
                manifest_json: r#"{"manifestVersion":2}"#,
                entry_relative_path: "bin/provider",
                entry_sha256: &[7; 32],
                install_root: Path::new("bundled"),
                trust_policy_id: "test",
                requested_permissions_json:
                    r#"[{"kind":"secret","namespace":"fake-sync","operations":["read"]}]"#,
            })
            .unwrap();
        let plugin = registry
            .host_plugin("com.megle.permission-update")
            .unwrap()
            .unwrap();
        assert_eq!(plugin.runtime_state, "permission-required");
        assert!(plugin.enabled_intent);
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
        register_test_plugin(&registry, "fake", "account");
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
    fn remote_account_identity_change_pauses_bindings_and_clears_cursors() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let now = unix_timestamp_millis();
        register_test_plugin(&registry, "fake", "account");
        registry.connection.lock().unwrap().execute(
            "INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES ('library','root','Library','local','root','ready','local-only','available',1,'marker',?1,?1)",
            [now],
        ).unwrap();
        let (binding, _) = registry
            .begin_sync_binding("library", "fake", "account")
            .unwrap();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO provider_binding_cursors(library_uuid,binding_epoch,remote_namespace,scope,cursor,page_token,transport_generation,updated_at) VALUES ('library',?1,'library','library','old',NULL,1,?2)",
            params![binding.as_bytes().as_slice(), now],
        ).unwrap();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO provider_account_root_cursors(account_uuid,scope,cursor,generation,updated_at) VALUES ('account','root','old',1,?1)",
            [now],
        ).unwrap();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO provider_remote_libraries(account_uuid,library_uuid,descriptor_size,descriptor_sha256,account_generation,updated_at) VALUES ('account','018f0000-0000-7000-8000-000000000001',1,zeroblob(32),1,?1)",
            [now],
        ).unwrap();
        assert!(registry
            .update_plugin_account_identity_for_session("fake", "test-session", Some("drive-a"))
            .unwrap()
            .unwrap()
            .is_empty());
        let fences = registry
            .update_plugin_account_identity_for_session("fake", "test-session", Some("drive-b"))
            .unwrap()
            .unwrap();
        assert_eq!(fences.len(), 1);
        assert_eq!(fences[0].binding_epoch, binding);
        assert_eq!(fences[0].next_transport_generation, 2);
        let connection = registry.connection.lock().unwrap();
        let state: (String, i64) = connection.query_row(
            "SELECT state,transport_generation FROM library_sync_bindings WHERE library_uuid='library'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(state, ("paused".into(), 2));
        let binding_cursors: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM provider_binding_cursors WHERE library_uuid='library'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let root_cursors: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM provider_account_root_cursors WHERE account_uuid='account'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let remote_libraries: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM provider_remote_libraries WHERE account_uuid='account'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((binding_cursors, root_cursors, remote_libraries), (0, 0, 0));
        drop(connection);
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn signing_out_a_provider_account_clears_all_remote_cursors_and_inventory() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let now = unix_timestamp_millis();
        register_test_plugin(&registry, "fake", "account");
        registry.connection.lock().unwrap().execute(
            "INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES ('library','root','Library','local','root','ready','local-only','available',1,'marker',?1,?1)",
            [now],
        ).unwrap();
        let (binding, _) = registry
            .begin_sync_binding("library", "fake", "account")
            .unwrap();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO provider_binding_cursors(library_uuid,binding_epoch,remote_namespace,scope,cursor,page_token,transport_generation,updated_at) VALUES ('library',?1,'library','library','old',NULL,1,?2)",
            params![binding.as_bytes().as_slice(), now],
        ).unwrap();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO provider_account_root_cursors(account_uuid,scope,cursor,generation,updated_at) VALUES ('account','root','old',1,?1)",
            [now],
        ).unwrap();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO provider_remote_libraries(account_uuid,library_uuid,descriptor_size,descriptor_sha256,account_generation,updated_at) VALUES ('account','018f0000-0000-7000-8000-000000000001',1,zeroblob(32),1,?1)",
            [now],
        ).unwrap();

        assert!(registry.sign_out_plugin_account("fake", 1).unwrap());
        let connection = registry.connection.lock().unwrap();
        let counts = (
            connection
                .query_row("SELECT COUNT(*) FROM provider_binding_cursors", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            connection
                .query_row(
                    "SELECT COUNT(*) FROM provider_account_root_cursors",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            connection
                .query_row(
                    "SELECT COUNT(*) FROM provider_remote_libraries",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
        );
        assert_eq!(counts, (0, 0, 0));
        let account: (Option<String>, String, i64) = connection.query_row(
            "SELECT secret_ref,auth_state,generation FROM plugin_accounts WHERE plugin_id='fake'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(account, (None, "signed-out".into(), 2));
        drop(connection);
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn provider_root_inventory_is_generation_fenced_and_cursor_isolated() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "fake", "account");
        let transport = registry.provider_root_transports().unwrap().remove(0);
        assert_eq!(transport.account_uuid, "account");
        let library_uuid = uuid::Uuid::now_v7().to_string();
        let metadata = crate::sync::provider::ObjectMetadata {
            key: format!("{library_uuid}/library-descriptor"),
            size: 42,
            sha256: [9; 32],
            etag: Some("etag-one".into()),
        };
        registry
            .commit_provider_root_page(
                "account",
                transport.account_generation,
                Some("cursor-zero"),
                "page-one",
                std::slice::from_ref(&metadata),
            )
            .unwrap();
        let transport = registry.provider_root_transports().unwrap().remove(0);
        assert_eq!(transport.cursor.as_deref(), Some("cursor-zero"));
        assert_eq!(transport.page_token.as_deref(), Some("page-one"));
        registry
            .commit_provider_root_cursor(
                "account",
                transport.account_generation,
                Some("cursor-one"),
                std::slice::from_ref(&metadata),
            )
            .unwrap();
        let connection = registry.connection.lock().unwrap();
        let inventory: (i64, Vec<u8>, String) = connection
            .query_row(
                "SELECT descriptor_size,descriptor_sha256,descriptor_etag FROM provider_remote_libraries WHERE account_uuid='account' AND library_uuid=?1",
                [library_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(inventory, (42, vec![9; 32], "etag-one".into()));
        let root: (Option<String>, Option<String>) = connection
            .query_row(
                "SELECT cursor,page_token FROM provider_account_root_cursors WHERE account_uuid='account' AND scope='root'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(root, (Some("cursor-one".into()), None));
        drop(connection);
        assert!(registry
            .commit_provider_root_cursor(
                "account",
                transport.account_generation + 1,
                Some("stale"),
                &[],
            )
            .is_err());
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
        register_test_plugin(&registry, "fake", "account");
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
    fn provider_cursor_and_receipt_reject_old_transport_generation() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "fake", "account");
        registry.connection.lock().unwrap().execute("INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES ('library','root','Library','local','root','ready','local-only','available',1,'marker',1,1)", []).unwrap();
        let (binding, _) = registry
            .begin_sync_binding("library", "fake", "account")
            .unwrap();
        registry
            .commit_binding_page(
                "library",
                binding,
                1,
                "remote",
                "delta",
                Some("cursor-1"),
                "page-2",
            )
            .unwrap();
        let paged = registry
            .binding_transport("library", "delta")
            .unwrap()
            .unwrap();
        assert_eq!(paged.cursor.as_deref(), Some("cursor-1"));
        assert_eq!(paged.page_token.as_deref(), Some("page-2"));
        registry
            .commit_binding_cursor("library", binding, 1, "remote", "delta", Some("cursor-2"))
            .unwrap();
        registry
            .record_provider_receipt(
                "library",
                binding,
                1,
                "library-descriptor",
                Some("etag"),
                &[3; 32],
                "created",
            )
            .unwrap();
        assert_eq!(
            registry
                .binding_transport("library", "delta")
                .unwrap()
                .unwrap()
                .cursor
                .as_deref(),
            Some("cursor-2")
        );
        assert!(registry
            .binding_transport("library", "delta")
            .unwrap()
            .unwrap()
            .page_token
            .is_none());
        assert_eq!(registry.begin_unbind("library", binding).unwrap(), 2);
        assert!(registry
            .commit_binding_cursor("library", binding, 1, "remote", "delta", Some("late"))
            .is_err());
        assert!(registry
            .commit_binding_page(
                "library",
                binding,
                1,
                "remote",
                "delta",
                Some("cursor-2"),
                "late-page",
            )
            .is_err());
        assert!(registry
            .record_provider_receipt("library", binding, 1, "late", None, &[4; 32], "created")
            .is_err());
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

    #[test]
    fn candidate_probation_commits_or_rolls_back_and_recovers_after_restart() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "com.megle.versioned", "account-versioned");
        let now = unix_timestamp_millis();
        let manifest_json = format!(
            r#"{{"manifestVersion":2,"id":"com.megle.versioned","name":"Versioned","version":"2.0.0","engine":"process","entry":{{"windowsX64":"provider-v2.exe","linuxX64":"provider-v2"}},"rpc":{{"wireVersions":[1],"capabilities":["sync-provider.v1"],"methodVersions":{{"describe":1,"health":1,"diagnostics":1,"shutdown":1,"auth.begin":1,"auth.complete":1,"auth.status":1,"auth.sign_out":1,"delta.list":1,"object.head":1,"object.get":1,"object.get_range":1,"object.put_immutable":1,"upload.begin":1,"upload.write":1,"upload.commit":1,"upload.abort":1}}}},"activationEvents":["sync-enabled"],"requestedPermissions":[],"integrity":{{"sha256":"{}"}}}}"#,
            "0".repeat(64)
        );
        registry.connection.lock().unwrap().execute(
            "INSERT INTO plugin_versions(plugin_id,version,manifest_json,entry_relative_path,entry_sha256,state,installed_at) VALUES ('com.megle.versioned','2.0.0',?1,'provider-v2',?2,'candidate',?3)",
            params![manifest_json,[2u8;32].as_slice(),now],
        ).unwrap();
        let manifest_v3_json = manifest_json.replace("2.0.0", "3.0.0");
        registry.connection.lock().unwrap().execute(
            "INSERT INTO plugin_versions(plugin_id,version,manifest_json,entry_relative_path,entry_sha256,state,installed_at) VALUES ('com.megle.versioned','3.0.0',?1,'provider-v3',?2,'candidate',?3)",
            params![manifest_v3_json,[3u8;32].as_slice(),now],
        ).unwrap();
        registry
            .select_plugin_version("com.megle.versioned", "2.0.0", true)
            .unwrap();
        assert_eq!(
            registry
                .select_plugin_version("com.megle.versioned", "3.0.0", true)
                .unwrap_err()
                .to_string(),
            "plugin-version-switch-in-progress"
        );
        assert_eq!(
            registry
                .host_plugin("com.megle.versioned")
                .unwrap()
                .unwrap()
                .selected_version,
            "2.0.0"
        );
        registry
            .rollback_plugin_probation("com.megle.versioned", "health-failed")
            .unwrap();
        assert_eq!(
            registry
                .host_plugin("com.megle.versioned")
                .unwrap()
                .unwrap()
                .selected_version,
            "1.0.0"
        );

        registry
            .select_plugin_version("com.megle.versioned", "2.0.0", true)
            .unwrap();
        drop(registry);
        let registry = HostRegistry::open(&path).unwrap();
        assert_eq!(registry.recover_plugin_version_journal().unwrap(), 1);
        assert_eq!(
            registry
                .host_plugin("com.megle.versioned")
                .unwrap()
                .unwrap()
                .selected_version,
            "1.0.0"
        );

        registry
            .select_plugin_version("com.megle.versioned", "2.0.0", true)
            .unwrap();
        registry
            .commit_plugin_probation("com.megle.versioned")
            .unwrap();
        let projection = registry
            .host_plugin("com.megle.versioned")
            .unwrap()
            .unwrap();
        assert_eq!(projection.selected_version, "2.0.0");
        assert_eq!(projection.known_good_version.as_deref(), Some("2.0.0"));
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stale_runtime_launch_cannot_finish_candidate_probation_after_disable() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "com.megle.stale-probation", "account-stale");
        let now = unix_timestamp_millis();
        let manifest_json = format!(
            r#"{{"manifestVersion":2,"id":"com.megle.stale-probation","name":"Versioned","version":"2.0.0","engine":"process","entry":{{"windowsX64":"provider-v2.exe","linuxX64":"provider-v2"}},"rpc":{{"wireVersions":[1],"capabilities":["sync-provider.v1"],"methodVersions":{{"describe":1,"health":1,"diagnostics":1,"shutdown":1,"auth.begin":1,"auth.complete":1,"auth.status":1,"auth.sign_out":1,"delta.list":1,"object.head":1,"object.get":1,"object.get_range":1,"object.put_immutable":1,"upload.begin":1,"upload.write":1,"upload.commit":1,"upload.abort":1}}}},"activationEvents":["sync-enabled"],"requestedPermissions":[],"integrity":{{"sha256":"{}"}}}}"#,
            "0".repeat(64)
        );
        registry.connection.lock().unwrap().execute(
            "INSERT INTO plugin_versions(plugin_id,version,manifest_json,entry_relative_path,entry_sha256,state,installed_at) VALUES ('com.megle.stale-probation','2.0.0',?1,'provider-v2',?2,'candidate',?3)",
            params![manifest_json,[2u8;32].as_slice(),now],
        ).unwrap();
        registry
            .select_plugin_version("com.megle.stale-probation", "2.0.0", true)
            .unwrap();
        let fingerprint = registry.plugin_runtime_launches().unwrap()[0].fingerprint();
        registry
            .set_plugin_intent("com.megle.stale-probation", false, true)
            .unwrap();

        assert!(!registry
            .commit_plugin_probation_for_launch("com.megle.stale-probation", &fingerprint)
            .unwrap());
        assert!(registry
            .rollback_plugin_probation_for_launch(
                "com.megle.stale-probation",
                &fingerprint,
                "stale-health-failure",
            )
            .unwrap()
            .is_none());
        let plugin = registry
            .host_plugin("com.megle.stale-probation")
            .unwrap()
            .unwrap();
        assert_eq!(plugin.selected_version, "2.0.0");
        assert_eq!(plugin.known_good_version.as_deref(), Some("1.0.0"));
        assert_eq!(plugin.runtime_state, "disabled");

        drop(registry);
        let _ = std::fs::remove_file(path);
    }
}
