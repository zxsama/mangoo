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
const HOST_MIGRATION_0005: &str = include_str!("../host_migrations/0005_sync_orchestration.sql");
const HOST_MIGRATION_0006: &str =
    include_str!("../host_migrations/0006_sync_orchestration_recovery.sql");

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProviderStatus {
    pub plugin_id: String,
    pub account_uuid: String,
    pub account_display_name: Option<String>,
    pub runtime_state: String,
    pub auth_state: String,
    pub last_healthy_at: Option<i64>,
    pub last_error_code: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySyncStatus {
    pub library_uuid: String,
    pub name: String,
    pub lifecycle: String,
    pub phase: String,
    pub catalog_readiness: String,
    pub provider_id: Option<String>,
    pub account_uuid: Option<String>,
    pub remote_library_uuid: Option<String>,
    pub allowed_actions: Vec<String>,
    pub last_success_at: Option<i64>,
    pub last_error_code: Option<String>,
    pub retry_after: Option<i64>,
    pub poll_after: Option<i64>,
    pub pending_upload_items: u64,
    pub pending_download_items: u64,
    pub pending_upload_bytes: u64,
    pub pending_download_bytes: u64,
    pub conflict_count: u64,
    pub local_divergence_count: u64,
    pub operation_revision: u64,
    pub restore_uuid: Option<String>,
    pub restore_items_done: Option<u64>,
    pub restore_items_total: Option<u64>,
    pub restore_bytes_done: Option<u64>,
    pub restore_bytes_total: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusSnapshot {
    pub revision: u64,
    pub generated_at: i64,
    pub scheduler_state: String,
    pub providers: Vec<SyncProviderStatus>,
    pub libraries: Vec<LibrarySyncStatus>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteLibrarySummary {
    pub account_uuid: String,
    pub library_uuid: String,
    pub name: String,
    pub icon: Option<String>,
    pub media_count: Option<u64>,
    pub logical_bytes: Option<u64>,
    pub last_synced_at: Option<i64>,
    pub descriptor_size: u64,
    pub updated_at: i64,
    pub compatible: bool,
    pub bound_locally: bool,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredRemoteLibrary {
    pub metadata: super::sync::provider::ObjectMetadata,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub media_count: Option<u64>,
    pub logical_bytes: Option<u64>,
    pub last_synced_at: Option<i64>,
    pub compatible: bool,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOperation {
    pub operation_uuid: String,
    pub library_uuid: Option<String>,
    pub kind: String,
    pub state: String,
    pub phase: String,
    pub error_code: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncUnbindPlan {
    pub plan_uuid: String,
    pub library_uuid: String,
    pub pending_count: u64,
    pub published_count: u64,
    pub unpublished_count: u64,
    pub cutover_actor_seq: u64,
    pub remote_watermark: u64,
    pub origin_identity: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReconnectPlan {
    pub plan_uuid: String,
    pub library_uuid: String,
    pub remote_library_uuid: String,
    pub origin_identity: Option<String>,
    pub local_divergence_count: u64,
    pub remote_divergence_count: u64,
    pub remote_exists: bool,
    pub remote_compatible: bool,
    pub same_remote_identity: bool,
    pub origin_remote_watermark: u64,
    pub base_remote_object_count: u64,
    pub remote_object_count: u64,
    pub remote_descriptor_sha256: Option<String>,
    pub remote_binding_epoch: Option<String>,
    pub remote_advanced: bool,
    pub merge_allowed: bool,
    pub publish_new_allowed: bool,
}

#[derive(Debug, Clone)]
pub struct ReconnectPreviewFacts {
    pub remote_exists: bool,
    pub remote_compatible: bool,
    pub remote_object_count: u64,
    pub remote_descriptor_sha256: Option<[u8; 32]>,
    pub remote_binding_epoch: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UnbindReconciliation {
    pub plan_uuid: String,
    pub plugin_id: String,
    pub account_uuid: String,
    pub remote_namespace: String,
    pub binding_epoch: uuid::Uuid,
    pub transport_generation: u64,
    pub descriptor_sha256: Option<[u8; 32]>,
    pub choice: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRestorePreflight {
    pub preflight_token: String,
    pub expires_at: i64,
    pub total_items: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRestorePlan {
    pub restore_uuid: String,
    pub operation_uuid: String,
    pub account_uuid: String,
    pub library_uuid: String,
    pub name: String,
    pub managed_root: String,
    pub state: String,
    pub phase: String,
    pub catalog_ready: bool,
    pub media_items_done: u64,
    pub media_items_total: Option<u64>,
    pub media_bytes_done: u64,
    pub media_bytes_total: Option<u64>,
    pub error_code: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct StartRestoreRequest<'a> {
    pub operation_uuid: &'a str,
    pub account_uuid: &'a str,
    pub library_uuid: &'a str,
    pub managed_root: &'a Path,
}

#[derive(Debug, Clone)]
pub struct SyncBindingRecovery {
    pub library_uuid: String,
    pub state: String,
    pub plan_uuid: Option<String>,
    pub choice: Option<String>,
}

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

fn sync_operation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncOperation> {
    Ok(SyncOperation {
        operation_uuid: row.get(0)?,
        library_uuid: row.get(1)?,
        kind: row.get(2)?,
        state: row.get(3)?,
        phase: row.get(4)?,
        error_code: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn sync_public_state(
    state: Option<&str>,
) -> (&'static str, &'static str, &'static str, Vec<&'static str>) {
    match state {
        None => ("local-only", "complete", "ready", vec!["enable"]),
        Some("detached") => (
            "local-only",
            "complete",
            "ready",
            vec!["enable", "reconnect"],
        ),
        Some("preparing") => ("enabling", "preflight", "ready", vec!["retry", "unbind"]),
        Some("capturing") => (
            "enabling",
            "capturing",
            "ready",
            vec!["pause", "retry", "unbind"],
        ),
        Some("seed_ready") => (
            "enabling",
            "uploading",
            "ready",
            vec!["pause", "sync-now", "unbind"],
        ),
        Some("publishing") => (
            "enabling",
            "publishing",
            "ready",
            vec!["pause", "sync-now", "unbind"],
        ),
        Some("active") => (
            "syncing",
            "active",
            "ready",
            vec!["pause", "sync-now", "unbind", "resolve-conflicts"],
        ),
        Some("paused") => (
            "paused",
            "active",
            "ready",
            vec!["resume", "unbind", "reconnect"],
        ),
        Some("unbinding") => ("unbinding", "reconcile", "ready", vec!["retry"]),
        Some("restoring") => (
            "restoring",
            "catalog-restore",
            "restoring",
            vec!["pause", "cancel-before-catalog"],
        ),
        Some("compatibility_locked") => (
            "error",
            "compatibility-locked",
            "ready",
            vec!["retry", "reconnect", "resolve-conflicts"],
        ),
        Some("error") => (
            "error",
            "retry-wait",
            "ready",
            vec!["retry", "pause", "unbind"],
        ),
        Some(_) => ("error", "retry-wait", "ready", vec!["retry"]),
    }
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
    pub runtime_session_id: String,
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
    ) -> anyhow::Result<Option<Vec<BindingFence>>> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE plugin_accounts SET secret_ref=NULL,auth_state='signed-out',generation=generation+1,updated_at=?1 WHERE plugin_id=?2 AND generation=?3",
            params![unix_timestamp_millis(),plugin_id,expected_generation],
        )?;
        if changed != 1 {
            return Ok(None);
        }
        transaction.execute(
            "UPDATE library_sync_bindings SET state='paused',last_error_code='authorization-required',retry_after=NULL,poll_after=NULL,transport_generation=transport_generation+1,updated_at=?1 WHERE plugin_id=?2 AND state <> 'unbinding'",
            params![unix_timestamp_millis(), plugin_id],
        )?;
        let fences = {
            let mut statement = transaction.prepare(
                "SELECT account_uuid,binding_epoch,transport_generation FROM library_sync_bindings WHERE plugin_id=?1 AND state='paused' ORDER BY library_uuid",
            )?;
            let rows = statement.query_map([plugin_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
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
            fences
        };
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
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(Some(fences))
    }

    pub fn require_plugin_reauthorization_for_launch(
        &self,
        plugin_id: &str,
        expected_fingerprint: &str,
    ) -> anyhow::Result<bool> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if !plugin_runtime_launch_matches(&transaction, plugin_id, expected_fingerprint)? {
            return Ok(false);
        }
        let changed = transaction.execute(
            "UPDATE plugin_accounts SET secret_ref=NULL,auth_state='authorization-required',generation=generation+1,last_error='authorization-required',updated_at=?1 WHERE plugin_id=?2",
            params![unix_timestamp_millis(), plugin_id],
        )?;
        if changed != 1 {
            return Ok(false);
        }
        pause_plugin_bindings_in_transaction(&transaction, plugin_id)?;
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
            "UPDATE plugin_runtime_state SET state='degraded',pid=NULL,session_id=NULL,started_at=NULL,backoff_until=NULL,last_error_code='authorization-required',last_error_detail='Microsoft authorization is required',updated_at=?1 WHERE plugin_id=?2",
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
        if !fences.is_empty() {
            Self::bump_sync_status_revision(&transaction)?;
        }
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
            "SELECT a.plugin_id,a.account_uuid,a.generation,r.session_id,c.cursor,c.page_token FROM plugin_accounts a JOIN plugin_installations i ON i.plugin_id=a.plugin_id JOIN plugin_runtime_state r ON r.plugin_id=a.plugin_id LEFT JOIN provider_account_root_cursors c ON c.account_uuid=a.account_uuid AND c.scope='root' WHERE i.enabled_intent=1 AND r.state='ready' AND r.session_id IS NOT NULL AND a.auth_state='authenticated' AND EXISTS (SELECT 1 FROM json_each(r.capabilities_json) WHERE value='sync-provider.v1') ORDER BY a.plugin_id",
        )?;
        let transports = statement
            .query_map([], |row| {
                Ok(ProviderRootTransport {
                    plugin_id: row.get(0)?,
                    account_uuid: row.get(1)?,
                    account_generation: row.get(2)?,
                    runtime_session_id: row.get(3)?,
                    cursor: row.get(4)?,
                    page_token: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(transports)
    }

    pub fn commit_provider_root_page(
        &self,
        account_uuid: &str,
        account_generation: i64,
        runtime_session_id: &str,
        cursor: Option<&str>,
        page_token: &str,
        libraries: &[DiscoveredRemoteLibrary],
    ) -> anyhow::Result<()> {
        self.commit_provider_root_progress(
            account_uuid,
            account_generation,
            runtime_session_id,
            cursor,
            Some(page_token),
            libraries,
        )
    }

    pub fn commit_provider_root_cursor(
        &self,
        account_uuid: &str,
        account_generation: i64,
        runtime_session_id: &str,
        cursor: Option<&str>,
        libraries: &[DiscoveredRemoteLibrary],
    ) -> anyhow::Result<()> {
        self.commit_provider_root_progress(
            account_uuid,
            account_generation,
            runtime_session_id,
            cursor,
            None,
            libraries,
        )
    }

    fn commit_provider_root_progress(
        &self,
        account_uuid: &str,
        account_generation: i64,
        runtime_session_id: &str,
        cursor: Option<&str>,
        page_token: Option<&str>,
        libraries: &[DiscoveredRemoteLibrary],
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let current: Option<i64> = transaction
            .query_row(
                "SELECT a.generation FROM plugin_accounts a JOIN plugin_runtime_state r ON r.plugin_id=a.plugin_id WHERE a.account_uuid=?1 AND r.state='ready' AND r.session_id=?2",
                params![account_uuid, runtime_session_id],
                |row| row.get(0),
            )
            .optional()?;
        if current != Some(account_generation) {
            anyhow::bail!("stale-provider-account-generation");
        }
        for discovered in libraries {
            let metadata = &discovered.metadata;
            let library_uuid = metadata
                .key
                .strip_suffix("/library-descriptor")
                .ok_or_else(|| anyhow::anyhow!("invalid provider root descriptor key"))?;
            let canonical = uuid::Uuid::parse_str(library_uuid)?;
            if canonical.to_string() != library_uuid {
                anyhow::bail!("non-canonical provider root Library UUID");
            }
            transaction.execute(
                "INSERT INTO provider_remote_libraries(account_uuid,library_uuid,descriptor_size,descriptor_sha256,descriptor_etag,account_generation,updated_at,name,icon,media_count,logical_bytes,last_synced_at,compatible,descriptor_error_code) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) ON CONFLICT(account_uuid,library_uuid) DO UPDATE SET descriptor_size=excluded.descriptor_size,descriptor_sha256=excluded.descriptor_sha256,descriptor_etag=excluded.descriptor_etag,account_generation=excluded.account_generation,updated_at=excluded.updated_at,name=excluded.name,icon=excluded.icon,media_count=excluded.media_count,logical_bytes=excluded.logical_bytes,last_synced_at=excluded.last_synced_at,compatible=excluded.compatible,descriptor_error_code=excluded.descriptor_error_code",
                params![account_uuid,library_uuid,metadata.size as i64,metadata.sha256.as_slice(),metadata.etag,account_generation,now,discovered.name,discovered.icon,discovered.media_count.map(|v|v as i64),discovered.logical_bytes.map(|v|v as i64),discovered.last_synced_at,discovered.compatible as i64,discovered.error_code],
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
        let (generation, state): (i64, String) = transaction.query_row(
            "SELECT transport_generation,state FROM library_sync_bindings WHERE library_uuid=?1 AND binding_epoch=?2",
            params![library_uuid,binding_epoch.as_bytes().as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if state == "unbinding" {
            transaction.commit()?;
            return Ok(generation);
        }
        let next = generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("transport generation exhausted"))?;
        transaction.execute(
            "UPDATE library_sync_bindings SET state='unbinding',transport_generation=?1,updated_at=?2 WHERE library_uuid=?3 AND binding_epoch=?4",
            params![next,unix_timestamp_millis(),library_uuid,binding_epoch.as_bytes().as_slice()],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
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
        let now = unix_timestamp_millis();
        let mut statement=connection.prepare("SELECT l.library_uuid,l.root_uuid,l.name,l.icon,l.color,l.local_data_dir,l.managed_root,l.lifecycle_state,CASE WHEN b.state IN ('preparing','capturing','seed_ready','publishing','restoring') THEN 'synchronizing' ELSE 'synchronized' END,l.availability,l.marker_identity FROM libraries l JOIN library_sync_bindings b ON b.library_uuid=l.library_uuid WHERE b.state IN ('preparing','capturing','seed_ready','publishing','active','restoring','error') AND COALESCE(b.retry_after,b.poll_after,0)<=?1 ORDER BY l.library_uuid")?;
        let libraries = statement
            .query_map([now], descriptor_from_row)?
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
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 5 {
            connection.execute_batch(HOST_MIGRATION_0005)?;
        }
        let schema_version: i64 = connection.query_row(
            "SELECT schema_version FROM host_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version < 6 {
            connection.execute_batch(HOST_MIGRATION_0006)?;
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

    pub fn rollback_reconnect_plan(
        &self,
        plan_uuid: &str,
        library_uuid: &str,
        binding_epoch: uuid::Uuid,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM library_sync_bindings WHERE library_uuid=?1 AND binding_epoch=?2 AND state='preparing'",
            params![library_uuid, binding_epoch.as_bytes().as_slice()],
        )?;
        transaction.execute(
            "UPDATE sync_reconnect_plans SET state='planned',choice=NULL,updated_at=?2 WHERE plan_uuid=?1 AND library_uuid=?3 AND state='committed'",
            params![plan_uuid, unix_timestamp_millis(), library_uuid],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
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
        Self::bump_sync_status_revision(&transaction)?;
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

    fn bump_sync_status_revision(transaction: &rusqlite::Transaction<'_>) -> anyhow::Result<u64> {
        transaction.execute(
            "UPDATE host_meta SET sync_status_revision=sync_status_revision+1 WHERE singleton=1",
            [],
        )?;
        Ok(transaction.query_row(
            "SELECT sync_status_revision FROM host_meta WHERE singleton=1",
            [],
            |row| row.get::<_, i64>(0),
        )? as u64)
    }

    pub fn create_sync_preflight_token(
        &self,
        library_uuid: &str,
        plugin_id: &str,
        account_uuid: &str,
        remote_library_uuid: Option<&str>,
    ) -> anyhow::Result<String> {
        let token = uuid::Uuid::now_v7().to_string();
        let now = unix_timestamp_millis();
        self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "INSERT INTO sync_preflight_tokens(token,library_uuid,plugin_id,account_uuid,remote_library_uuid,expires_at,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![token,library_uuid,plugin_id,account_uuid,remote_library_uuid,now+300_000,now],
        )?;
        Ok(token)
    }

    pub fn consume_sync_preflight_token(
        &self,
        token: &str,
        library_uuid: &str,
        plugin_id: &str,
        account_uuid: &str,
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let changed = self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "UPDATE sync_preflight_tokens SET consumed_at=?1 WHERE token=?2 AND library_uuid=?3 AND plugin_id=?4 AND account_uuid=?5 AND consumed_at IS NULL AND expires_at>=?1",
            params![now,token,library_uuid,plugin_id,account_uuid],
        )?;
        if changed != 1 {
            anyhow::bail!("sync-preflight-expired");
        }
        Ok(())
    }

    pub fn sync_status_snapshot(&self) -> anyhow::Result<SyncStatusSnapshot> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let revision = connection.query_row(
            "SELECT sync_status_revision FROM host_meta WHERE singleton=1",
            [],
            |row| row.get::<_, i64>(0),
        )? as u64;
        let scheduler_state = connection.query_row(
            "SELECT sync_scheduler_state FROM host_meta WHERE singleton=1",
            [],
            |row| row.get::<_, String>(0),
        )?;
        let providers = {
            let mut statement = connection.prepare(
                "SELECT i.plugin_id,a.account_uuid,a.display_name,r.state,a.auth_state,r.last_healthy_at,r.last_error_code FROM plugin_installations i JOIN plugin_runtime_state r ON r.plugin_id=i.plugin_id JOIN plugin_accounts a ON a.plugin_id=i.plugin_id WHERE EXISTS (SELECT 1 FROM json_each(r.capabilities_json) WHERE value='sync-provider.v1') ORDER BY i.plugin_id",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok(SyncProviderStatus {
                        plugin_id: row.get(0)?,
                        account_uuid: row.get(1)?,
                        account_display_name: row.get(2)?,
                        runtime_state: row.get(3)?,
                        auth_state: row.get(4)?,
                        last_healthy_at: row.get(5)?,
                        last_error_code: row.get(6)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let mut statement = connection.prepare(
            "SELECT l.library_uuid,l.name,COALESCE(b.plugin_id,u.plugin_id),COALESCE(b.account_uuid,u.account_uuid),COALESCE(b.remote_namespace,u.remote_namespace),CASE WHEN u.state IN ('cutover','reconciling') THEN 'unbinding' WHEN b.state IS NULL AND u.state='complete' THEN 'detached' ELSE b.state END,b.last_success_at,COALESCE(u.last_error_code,b.last_error_code),COALESCE(u.next_retry_at,b.retry_after),b.poll_after,b.published_through_seq,COALESCE(b.pending_upload_items,0),MAX(COALESCE((SELECT MAX(updated_at) FROM sync_orchestration_operations op WHERE op.library_uuid=l.library_uuid),0),COALESCE((SELECT MAX(updated_at) FROM sync_restore_plans rp WHERE rp.library_uuid=l.library_uuid),0),COALESCE(u.updated_at,0)),COALESCE(b.pending_download_items,0),COALESCE(b.pending_download_bytes,0),COALESCE(b.conflict_count,0),COALESCE((SELECT catalog_ready FROM sync_restore_plans latest_restore WHERE latest_restore.library_uuid=l.library_uuid ORDER BY created_at DESC LIMIT 1),0),COALESCE(b.pending_upload_bytes,0),(SELECT restore_uuid FROM sync_restore_plans latest_restore WHERE latest_restore.library_uuid=l.library_uuid ORDER BY created_at DESC LIMIT 1),(SELECT media_items_done FROM sync_restore_plans latest_restore WHERE latest_restore.library_uuid=l.library_uuid ORDER BY created_at DESC LIMIT 1),(SELECT media_items_total FROM sync_restore_plans latest_restore WHERE latest_restore.library_uuid=l.library_uuid ORDER BY created_at DESC LIMIT 1),(SELECT media_bytes_done FROM sync_restore_plans latest_restore WHERE latest_restore.library_uuid=l.library_uuid ORDER BY created_at DESC LIMIT 1),(SELECT media_bytes_total FROM sync_restore_plans latest_restore WHERE latest_restore.library_uuid=l.library_uuid ORDER BY created_at DESC LIMIT 1) FROM libraries l LEFT JOIN library_sync_bindings b ON b.library_uuid=l.library_uuid LEFT JOIN sync_unbind_plans u ON u.plan_uuid=(SELECT latest_unbind.plan_uuid FROM sync_unbind_plans latest_unbind WHERE latest_unbind.library_uuid=l.library_uuid AND latest_unbind.state IN ('cutover','reconciling','complete') ORDER BY latest_unbind.updated_at DESC LIMIT 1) ORDER BY COALESCE(l.last_opened_at,l.created_at) DESC,l.library_uuid",
        )?;
        let libraries = statement
            .query_map([], |row| {
                let state: Option<String> = row.get(5)?;
                let (lifecycle, mut phase, mut readiness, mut actions) =
                    sync_public_state(state.as_deref());
                if state.as_deref() == Some("restoring") && row.get::<_, i64>(16)? != 0 {
                    phase = "restoring-media";
                    readiness = "catalog-ready";
                    actions = vec!["pause"];
                }
                let published: i64 = row.get::<_, Option<i64>>(10)?.unwrap_or(0);
                let pending: i64 = row.get(11)?;
                let pending_download: i64 = row.get(13)?;
                let pending_download_bytes: i64 = row.get(14)?;
                let conflicts: i64 = row.get(15)?;
                Ok(LibrarySyncStatus {
                    library_uuid: row.get(0)?,
                    name: row.get(1)?,
                    lifecycle: lifecycle.to_owned(),
                    phase: phase.to_owned(),
                    catalog_readiness: readiness.to_owned(),
                    provider_id: row.get(2)?,
                    account_uuid: row.get(3)?,
                    remote_library_uuid: row.get(4)?,
                    allowed_actions: actions.into_iter().map(str::to_owned).collect(),
                    last_success_at: row.get(6)?,
                    last_error_code: row.get(7)?,
                    retry_after: row.get(8)?,
                    poll_after: row.get(9)?,
                    pending_upload_items: pending.max(0) as u64,
                    pending_download_items: pending_download.max(0) as u64,
                    pending_upload_bytes: row.get::<_, i64>(17)?.max(0) as u64,
                    pending_download_bytes: pending_download_bytes.max(0) as u64,
                    conflict_count: conflicts.max(0) as u64,
                    local_divergence_count: (published == 0).then_some(pending).unwrap_or(0).max(0)
                        as u64,
                    operation_revision: row.get::<_, i64>(12)?.max(0) as u64,
                    restore_uuid: row.get(18)?,
                    restore_items_done: row
                        .get::<_, Option<i64>>(19)?
                        .map(|value| value.max(0) as u64),
                    restore_items_total: row
                        .get::<_, Option<i64>>(20)?
                        .map(|value| value.max(0) as u64),
                    restore_bytes_done: row
                        .get::<_, Option<i64>>(21)?
                        .map(|value| value.max(0) as u64),
                    restore_bytes_total: row
                        .get::<_, Option<i64>>(22)?
                        .map(|value| value.max(0) as u64),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SyncStatusSnapshot {
            revision,
            generated_at: unix_timestamp_millis(),
            scheduler_state,
            providers,
            libraries,
        })
    }

    pub fn set_sync_scheduler_state(&self, state: &str) -> anyhow::Result<()> {
        if !matches!(state, "starting" | "running" | "degraded" | "stopped") {
            anyhow::bail!("invalid-sync-scheduler-state");
        }
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let current: String = transaction.query_row(
            "SELECT sync_scheduler_state FROM host_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        if current != state {
            transaction.execute(
                "UPDATE host_meta SET sync_scheduler_state=?1 WHERE singleton=1",
                [state],
            )?;
            Self::bump_sync_status_revision(&transaction)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn update_sync_pending_metrics(
        &self,
        library_uuid: &str,
        upload_items: u64,
        upload_bytes: u64,
        download_items: u64,
        download_bytes: u64,
        conflict_count: u64,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE library_sync_bindings SET pending_upload_items=?2,pending_upload_bytes=?3,pending_download_items=?4,pending_download_bytes=?5,conflict_count=?6,updated_at=?7 WHERE library_uuid=?1",
            params![library_uuid,upload_items as i64,upload_bytes as i64,download_items as i64,download_bytes as i64,conflict_count as i64,unix_timestamp_millis()],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn list_remote_libraries(&self) -> anyhow::Result<Vec<RemoteLibrarySummary>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare(
            "SELECT r.account_uuid,r.library_uuid,r.name,r.icon,r.media_count,r.logical_bytes,r.last_synced_at,r.descriptor_size,r.updated_at,r.compatible,r.descriptor_error_code,EXISTS(SELECT 1 FROM library_sync_bindings b WHERE b.account_uuid=r.account_uuid AND COALESCE(b.remote_namespace,b.library_uuid)=r.library_uuid) FROM provider_remote_libraries r JOIN plugin_accounts a ON a.account_uuid=r.account_uuid WHERE r.account_generation=a.generation ORDER BY r.updated_at DESC,r.library_uuid",
        )?;
        let rows = statement
            .query_map([], |row| {
                let library_uuid: String = row.get(1)?;
                Ok(RemoteLibrarySummary {
                    account_uuid: row.get(0)?,
                    name: row.get::<_, Option<String>>(2)?.unwrap_or_else(|| {
                        format!("Remote {}", &library_uuid[..8.min(library_uuid.len())])
                    }),
                    library_uuid,
                    icon: row.get(3)?,
                    media_count: row.get::<_, Option<i64>>(4)?.map(|v| v.max(0) as u64),
                    logical_bytes: row.get::<_, Option<i64>>(5)?.map(|v| v.max(0) as u64),
                    last_synced_at: row.get(6)?,
                    descriptor_size: row.get::<_, i64>(7)?.max(0) as u64,
                    updated_at: row.get(8)?,
                    compatible: row.get::<_, i64>(9)? != 0,
                    error_code: row.get(10)?,
                    bound_locally: row.get::<_, i64>(11)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn begin_sync_operation_once(
        &self,
        operation_uuid: &str,
        library_uuid: Option<&str>,
        kind: &str,
        phase: &str,
        request: &serde_json::Value,
    ) -> anyhow::Result<(SyncOperation, bool)> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let request_json = serde_json::to_string(request)?;
        let inserted = transaction.execute("INSERT INTO sync_orchestration_operations(operation_uuid,library_uuid,kind,state,phase,request_json,created_at,updated_at) VALUES (?1,?2,?3,'running',?4,?5,?6,?6) ON CONFLICT(operation_uuid) DO NOTHING", params![operation_uuid,library_uuid,kind,phase,request_json,now])? != 0;
        if inserted {
            Self::bump_sync_status_revision(&transaction)?;
        } else {
            let (stored_library_uuid, stored_kind, stored_request): (Option<String>, String, String) = transaction.query_row(
                "SELECT library_uuid,kind,request_json FROM sync_orchestration_operations WHERE operation_uuid=?1",
                [operation_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            if stored_library_uuid.as_deref() != library_uuid
                || stored_kind != kind
                || stored_request != request_json
            {
                anyhow::bail!("operation-id-reused");
            }
        }
        let operation = transaction.query_row("SELECT operation_uuid,library_uuid,kind,state,phase,error_code,updated_at FROM sync_orchestration_operations WHERE operation_uuid=?1",[operation_uuid],sync_operation_from_row)?;
        transaction.commit()?;
        Ok((operation, inserted))
    }

    pub fn finish_sync_operation(
        &self,
        operation_uuid: &str,
        state: &str,
        phase: &str,
        error_code: Option<&str>,
    ) -> anyhow::Result<SyncOperation> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if transaction.execute("UPDATE sync_orchestration_operations SET state=?2,phase=?3,error_code=?4,updated_at=?5 WHERE operation_uuid=?1",params![operation_uuid,state,phase,error_code,now])? == 0 { anyhow::bail!("sync-operation-not-found"); }
        Self::bump_sync_status_revision(&transaction)?;
        let operation = transaction.query_row("SELECT operation_uuid,library_uuid,kind,state,phase,error_code,updated_at FROM sync_orchestration_operations WHERE operation_uuid=?1",[operation_uuid],sync_operation_from_row)?;
        transaction.commit()?;
        Ok(operation)
    }

    pub fn set_binding_state(&self, library_uuid: &str, state: &str) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let now = unix_timestamp_millis();
        if transaction.execute("UPDATE library_sync_bindings SET state=?2,last_error_code=NULL,retry_after=NULL,poll_after=?3,updated_at=?3 WHERE library_uuid=?1",params![library_uuid,state,now])? == 0 { anyhow::bail!("sync-binding-not-found"); }
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn pause_binding(&self, library_uuid: &str) -> anyhow::Result<i64> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let generation: i64 = transaction.query_row(
            "SELECT transport_generation FROM library_sync_bindings WHERE library_uuid=?1",
            [library_uuid],
            |row| row.get(0),
        )?;
        let next = generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("transport-generation-overflow"))?;
        transaction.execute("UPDATE library_sync_bindings SET state='paused',transport_generation=?2,retry_after=NULL,poll_after=NULL,updated_at=?3 WHERE library_uuid=?1",params![library_uuid,next,unix_timestamp_millis()])?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(next)
    }

    pub fn wake_binding(&self, library_uuid: &str) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if transaction.execute("UPDATE library_sync_bindings SET poll_after=0,retry_after=NULL,last_error_code=NULL,updated_at=?2 WHERE library_uuid=?1 AND state NOT IN ('paused','unbinding')",params![library_uuid,unix_timestamp_millis()])? == 0 { anyhow::bail!("sync-action-not-allowed"); }
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn wake_all_sync_bindings(&self) -> anyhow::Result<usize> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE library_sync_bindings SET poll_after=0,retry_after=NULL,last_error_code=NULL,updated_at=?1 WHERE state NOT IN ('paused','unbinding')",
            [unix_timestamp_millis()],
        )?;
        if changed > 0 {
            Self::bump_sync_status_revision(&transaction)?;
        }
        transaction.commit()?;
        Ok(changed)
    }

    pub fn record_sync_turn_success(&self, library_uuid: &str, active: bool) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let next_poll = now + if active { 30_000 } else { 180_000 };
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE library_sync_bindings SET last_success_at=?2,last_error_code=NULL,retry_after=NULL,poll_after=?3,updated_at=?2 WHERE library_uuid=?1 AND state NOT IN ('paused','unbinding')",
            params![library_uuid,now,next_poll],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn record_sync_turn_error(
        &self,
        library_uuid: &str,
        error_code: &str,
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE library_sync_bindings SET state='error',last_error_code=?2,retry_after=?3,poll_after=NULL,updated_at=?4 WHERE library_uuid=?1 AND state NOT IN ('paused','unbinding')",
            params![library_uuid,error_code,now+15_000,now],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn create_unbind_plan(&self, library_uuid: &str) -> anyhow::Result<SyncUnbindPlan> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let (binding, generation, published, origin, plugin_id, account_uuid, remote_namespace): (Vec<u8>, i64, i64, Option<String>, String, String, String) = transaction.query_row("SELECT b.binding_epoch,b.transport_generation,b.published_through_seq,l.origin_remote_identity,b.plugin_id,b.account_uuid,COALESCE(b.remote_namespace,b.library_uuid) FROM library_sync_bindings b JOIN libraries l ON l.library_uuid=b.library_uuid WHERE b.library_uuid=?1",[library_uuid],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)))?;
        let pending: i64 = transaction.query_row("SELECT COUNT(*) FROM host_sync_outbox WHERE library_uuid=?1 AND state IN ('pending','sealed')",[library_uuid],|row|row.get(0))?;
        let descriptor_sha256: Option<Vec<u8>> = transaction.query_row("SELECT sha256 FROM provider_object_receipts WHERE library_uuid=?1 AND binding_epoch=?2 AND object_key='library-descriptor' AND state IN ('created','already-exists-matching')",params![library_uuid,&binding],|row|row.get(0)).optional()?;
        let base_remote_object_count: i64 = transaction.query_row("SELECT COUNT(*) FROM provider_object_receipts WHERE library_uuid=?1 AND binding_epoch=?2 AND state IN ('created','already-exists-matching')",params![library_uuid,&binding],|row|row.get(0))?;
        let plan_uuid = uuid::Uuid::now_v7().to_string();
        transaction.execute("INSERT INTO sync_unbind_plans(plan_uuid,library_uuid,binding_epoch,transport_generation,cutover_actor_seq,remote_watermark,pending_count,published_count,unpublished_count,origin_identity,state,created_at,updated_at,plugin_id,account_uuid,remote_namespace,descriptor_sha256,base_remote_object_count) VALUES (?1,?2,?3,?4,?5,?5,?6,?5,?6,?7,'planned',?8,?8,?9,?10,?11,?12,?13)",params![plan_uuid,library_uuid,binding,generation,published,pending,origin,now,plugin_id,account_uuid,remote_namespace,descriptor_sha256,base_remote_object_count])?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(SyncUnbindPlan {
            plan_uuid,
            library_uuid: library_uuid.to_owned(),
            pending_count: pending as u64,
            published_count: published.max(0) as u64,
            unpublished_count: pending as u64,
            cutover_actor_seq: published.max(0) as u64,
            remote_watermark: published.max(0) as u64,
            origin_identity: origin,
        })
    }

    pub fn validate_unbind_plan(&self, library_uuid: &str, plan_uuid: &str) -> anyhow::Result<()> {
        let exists: i64 = self.connection.lock().unwrap_or_else(|p| p.into_inner()).query_row("SELECT COUNT(*) FROM sync_unbind_plans WHERE plan_uuid=?1 AND library_uuid=?2 AND state IN ('planned','cutover','reconciling')",params![plan_uuid,library_uuid],|row|row.get(0))?;
        if exists != 1 {
            anyhow::bail!("sync-unbind-plan-not-found");
        }
        Ok(())
    }

    pub fn advance_unbind_plan(
        &self,
        plan_uuid: &str,
        choice: &str,
        state: &str,
    ) -> anyhow::Result<()> {
        if !matches!(state, "cutover" | "reconciling") {
            anyhow::bail!("invalid-unbind-plan-state");
        }
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE sync_unbind_plans SET state=?2,choice=COALESCE(choice,?3),updated_at=?4 WHERE plan_uuid=?1 AND state IN ('planned','cutover','reconciling')",
            params![plan_uuid,state,choice,unix_timestamp_millis()],
        )?;
        if changed == 0 {
            anyhow::bail!("sync-unbind-plan-not-found");
        }
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn pending_sync_upload_count(&self, library_uuid: &str) -> anyhow::Result<u64> {
        let count: i64 = self.connection.lock().unwrap_or_else(|p| p.into_inner()).query_row(
            "SELECT COUNT(*) FROM host_sync_outbox WHERE library_uuid=?1 AND state IN ('pending','sealed')",
            [library_uuid],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u64)
    }

    pub fn complete_unbind_plan(&self, plan_uuid: &str, choice: &str) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute("UPDATE sync_unbind_plans SET state='complete',choice=?2,remote_visibility='visible',next_retry_at=NULL,last_error_code=NULL,updated_at=?3 WHERE plan_uuid=?1",params![plan_uuid,choice,unix_timestamp_millis()])?;
        transaction.execute("UPDATE sync_orchestration_operations SET state='complete',phase='complete',error_code=NULL,updated_at=?2 WHERE operation_uuid=(SELECT operation_uuid FROM sync_unbind_plans WHERE plan_uuid=?1)",params![plan_uuid,unix_timestamp_millis()])?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn attach_unbind_operation(
        &self,
        plan_uuid: &str,
        operation_uuid: &str,
    ) -> anyhow::Result<()> {
        let changed = self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "UPDATE sync_unbind_plans SET operation_uuid=?2,updated_at=?3 WHERE plan_uuid=?1 AND state IN ('planned','cutover','reconciling')",
            params![plan_uuid, operation_uuid, unix_timestamp_millis()],
        )?;
        if changed != 1 {
            anyhow::bail!("sync-unbind-plan-not-found");
        }
        Ok(())
    }

    pub fn pending_unbind_reconciliations(&self) -> anyhow::Result<Vec<UnbindReconciliation>> {
        let now = unix_timestamp_millis();
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare("SELECT plan_uuid,plugin_id,account_uuid,remote_namespace,binding_epoch,transport_generation,descriptor_sha256,choice FROM sync_unbind_plans WHERE state='reconciling' AND (next_retry_at IS NULL OR next_retry_at<=?1) ORDER BY updated_at,plan_uuid")?;
        let values = statement
            .query_map([now], |row| {
                let bytes: Vec<u8> = row.get(4)?;
                let binding_epoch = uuid::Uuid::from_slice(&bytes).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        bytes.len(),
                        rusqlite::types::Type::Blob,
                        Box::new(error),
                    )
                })?;
                let hash = row
                    .get::<_, Option<Vec<u8>>>(6)?
                    .and_then(|bytes| bytes.try_into().ok());
                Ok(UnbindReconciliation {
                    plan_uuid: row.get(0)?,
                    plugin_id: row.get(1)?,
                    account_uuid: row.get(2)?,
                    remote_namespace: row.get(3)?,
                    binding_epoch,
                    transport_generation: row.get::<_, i64>(5)?.saturating_add(1) as u64,
                    descriptor_sha256: hash,
                    choice: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(values)
    }

    pub fn record_unbind_reconcile_failure(
        &self,
        plan_uuid: &str,
        error_code: &str,
        retry_after_ms: u64,
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute("UPDATE sync_unbind_plans SET state='reconciling',remote_visibility='unknown',reconcile_attempts=reconcile_attempts+1,next_retry_at=?2,last_error_code=?3,updated_at=?1 WHERE plan_uuid=?4",params![now,now.saturating_add(retry_after_ms.min(i64::MAX as u64) as i64),error_code,plan_uuid])?;
        transaction.execute("UPDATE sync_orchestration_operations SET state='waiting',phase='retry-wait',error_code=?2,updated_at=?3 WHERE operation_uuid=(SELECT operation_uuid FROM sync_unbind_plans WHERE plan_uuid=?1)",params![plan_uuid,error_code,now])?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn unbind_plan_complete(&self, plan_uuid: &str) -> anyhow::Result<bool> {
        self.connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .query_row(
                "SELECT state='complete' FROM sync_unbind_plans WHERE plan_uuid=?1",
                [plan_uuid],
                |row| row.get::<_, i64>(0).map(|value| value != 0),
            )
            .map_err(Into::into)
    }

    pub fn reconnect_origin_transport(
        &self,
        library_uuid: &str,
        account_uuid: &str,
        remote_library_uuid: &str,
    ) -> anyhow::Result<Option<(uuid::Uuid, u64)>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let value: Option<(Vec<u8>, i64)> = connection
            .query_row(
                "SELECT binding_epoch,transport_generation FROM sync_unbind_plans WHERE library_uuid=?1 AND account_uuid=?2 AND remote_namespace=?3 AND state='complete' ORDER BY updated_at DESC LIMIT 1",
                params![library_uuid, account_uuid, remote_library_uuid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        value
            .map(|(binding, generation)| {
                Ok((
                    uuid::Uuid::from_slice(&binding)?,
                    generation.saturating_add(1) as u64,
                ))
            })
            .transpose()
    }

    pub fn create_reconnect_plan(
        &self,
        library_uuid: &str,
        account_uuid: &str,
        remote_library_uuid: &str,
        local_divergence_count: u64,
        facts: ReconnectPreviewFacts,
    ) -> anyhow::Result<SyncReconnectPlan> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let origin: Option<String> = transaction.query_row(
            "SELECT origin_remote_identity FROM libraries WHERE library_uuid=?1",
            [library_uuid],
            |row| row.get(0),
        )?;
        let account_ready = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM plugin_accounts a JOIN plugin_runtime_state r ON r.plugin_id=a.plugin_id WHERE a.account_uuid=?1 AND a.auth_state='authenticated' AND r.state='ready' AND EXISTS(SELECT 1 FROM json_each(r.capabilities_json) WHERE value='sync-provider.v1'))",
                [account_uuid],
                |row| row.get::<_, i64>(0),
            )? != 0;
        if !account_ready {
            anyhow::bail!("provider-not-ready");
        }
        let same_remote_identity = origin.as_deref() == Some(remote_library_uuid);
        let (origin_remote_watermark, base_remote_object_count, base_descriptor_sha256):
            (i64, i64, Option<Vec<u8>>) = transaction
            .query_row(
                "SELECT remote_watermark,base_remote_object_count,descriptor_sha256 FROM sync_unbind_plans WHERE library_uuid=?1 AND state='complete' ORDER BY updated_at DESC LIMIT 1",
                [library_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .unwrap_or((0, 0, None));
        let base_remote_object_count = base_remote_object_count.max(0) as u64;
        let count_divergence = facts.remote_object_count.abs_diff(base_remote_object_count);
        let descriptor_changed = base_descriptor_sha256
            .as_deref()
            .zip(
                facts
                    .remote_descriptor_sha256
                    .as_ref()
                    .map(|hash| hash.as_slice()),
            )
            .is_some_and(|(base, current)| base != current);
        let remote_divergence = count_divergence.max(u64::from(descriptor_changed));
        let origin_binding_epoch: Option<String> = transaction
            .query_row(
                "SELECT lower(hex(binding_epoch)) FROM sync_unbind_plans WHERE library_uuid=?1 AND state='complete' ORDER BY updated_at DESC LIMIT 1",
                [library_uuid],
                |row| row.get(0),
            )
            .optional()?;
        let same_binding_lineage = origin_binding_epoch
            .as_deref()
            .zip(facts.remote_binding_epoch.as_deref())
            .is_some_and(|(origin, remote)| {
                let normalized_remote = remote.replace('-', "").to_ascii_lowercase();
                origin == normalized_remote
            });
        let merge_allowed = facts.remote_exists
            && facts.remote_compatible
            && same_remote_identity
            && same_binding_lineage;
        let plan_uuid = uuid::Uuid::now_v7().to_string();
        transaction.execute("INSERT INTO sync_reconnect_plans(plan_uuid,library_uuid,account_uuid,remote_library_uuid,origin_identity,local_divergence_count,remote_divergence_count,remote_exists,remote_compatible,same_remote_identity,state,created_at,updated_at,origin_remote_watermark,base_remote_object_count,remote_object_count,remote_descriptor_sha256,remote_binding_epoch,merge_allowed,publish_new_allowed) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'planned',?11,?11,?12,?13,?14,?15,?16,?17,1)",params![plan_uuid,library_uuid,account_uuid,remote_library_uuid,origin,local_divergence_count as i64,remote_divergence as i64,i64::from(facts.remote_exists),i64::from(facts.remote_compatible),i64::from(same_remote_identity),now,origin_remote_watermark,base_remote_object_count,facts.remote_object_count as i64,facts.remote_descriptor_sha256.map(|v|v.to_vec()),facts.remote_binding_epoch,i64::from(merge_allowed)])?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(SyncReconnectPlan {
            plan_uuid,
            library_uuid: library_uuid.to_owned(),
            remote_library_uuid: remote_library_uuid.to_owned(),
            origin_identity: origin,
            local_divergence_count,
            remote_divergence_count: remote_divergence,
            remote_exists: facts.remote_exists,
            remote_compatible: facts.remote_compatible,
            same_remote_identity,
            origin_remote_watermark: origin_remote_watermark.max(0) as u64,
            base_remote_object_count,
            remote_object_count: facts.remote_object_count,
            remote_descriptor_sha256: facts
                .remote_descriptor_sha256
                .map(|bytes| bytes.iter().map(|byte| format!("{byte:02x}")).collect()),
            remote_binding_epoch: facts.remote_binding_epoch,
            remote_advanced: remote_divergence > 0,
            merge_allowed,
            publish_new_allowed: true,
        })
    }

    pub fn commit_reconnect_plan(&self, plan_uuid: &str, choice: &str) -> anyhow::Result<String> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        if !matches!(choice, "merge-local-fork" | "publish-new-remote") {
            anyhow::bail!("invalid-reconnect-choice");
        }
        let (library_uuid, account_uuid, remote_uuid, merge_allowed, publish_new_allowed): (String, String, String, i64, i64) = transaction.query_row("SELECT library_uuid,account_uuid,remote_library_uuid,merge_allowed,publish_new_allowed FROM sync_reconnect_plans WHERE plan_uuid=?1 AND state='planned'",[plan_uuid],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?;
        if choice == "merge-local-fork" && merge_allowed == 0 {
            anyhow::bail!("reconnect-merge-preflight-failed");
        }
        if choice == "publish-new-remote" && publish_new_allowed == 0 {
            anyhow::bail!("reconnect-publish-preflight-failed");
        }
        let plugin_id: String = transaction.query_row(
            "SELECT plugin_id FROM plugin_accounts WHERE account_uuid=?1",
            [&account_uuid],
            |row| row.get(0),
        )?;
        let binding = uuid::Uuid::now_v7();
        let actor = uuid::Uuid::now_v7();
        let namespace = if choice == "publish-new-remote" {
            library_uuid.clone()
        } else {
            remote_uuid.clone()
        };
        transaction.execute("INSERT INTO library_sync_bindings(library_uuid,plugin_id,account_uuid,remote_namespace,binding_epoch,actor_epoch,state,transport_generation,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,'preparing',1,?7,?7)",params![library_uuid,plugin_id,account_uuid,namespace,binding.as_bytes().as_slice(),actor.as_bytes().as_slice(),now])?;
        transaction.execute(
            "UPDATE libraries SET origin_remote_identity=?2,updated_at=?3 WHERE library_uuid=?1",
            params![library_uuid, namespace, now],
        )?;
        transaction.execute("UPDATE sync_reconnect_plans SET state='committed',choice=?2,updated_at=?3 WHERE plan_uuid=?1",params![plan_uuid,choice,now])?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(library_uuid)
    }

    pub fn start_restore_plan(
        &self,
        request: StartRestoreRequest<'_>,
    ) -> anyhow::Result<SyncRestorePlan> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let remote: Option<(Vec<u8>, String)> = transaction
            .query_row(
                "SELECT descriptor_sha256,name FROM provider_remote_libraries WHERE account_uuid=?1 AND library_uuid=?2 AND compatible=1",
                params![request.account_uuid, request.library_uuid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (hash, remote_name) =
            remote.ok_or_else(|| anyhow::anyhow!("remote-library-not-restorable"))?;
        let restore_uuid = uuid::Uuid::now_v7().to_string();
        transaction.execute("INSERT INTO sync_restore_plans(restore_uuid,operation_uuid,account_uuid,library_uuid,name,managed_root,descriptor_sha256,state,phase,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'restoring','catalog-restore',?8,?8)",params![restore_uuid,request.operation_uuid,request.account_uuid,request.library_uuid,remote_name,request.managed_root.to_string_lossy(),hash,now])?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(SyncRestorePlan {
            restore_uuid,
            operation_uuid: request.operation_uuid.to_owned(),
            account_uuid: request.account_uuid.to_owned(),
            library_uuid: request.library_uuid.to_owned(),
            name: remote_name,
            managed_root: request.managed_root.to_string_lossy().into_owned(),
            state: "restoring".into(),
            phase: "catalog-restore".into(),
            catalog_ready: false,
            media_items_done: 0,
            media_items_total: None,
            media_bytes_done: 0,
            media_bytes_total: None,
            error_code: None,
            updated_at: now,
        })
    }

    pub fn begin_restore_binding(
        &self,
        library_uuid: &str,
        account_uuid: &str,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let plugin_id: String = transaction.query_row(
            "SELECT plugin_id FROM plugin_accounts WHERE account_uuid=?1 AND auth_state='authenticated'",
            [account_uuid],
            |row| row.get(0),
        )?;
        let ready: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM plugin_runtime_state WHERE plugin_id=?1 AND state='ready' AND EXISTS (SELECT 1 FROM json_each(capabilities_json) WHERE value='sync-provider.v1')",
            [&plugin_id], |row| row.get(0),
        )?;
        if ready != 1 {
            anyhow::bail!("provider-not-ready");
        }
        let binding = uuid::Uuid::now_v7();
        let actor = uuid::Uuid::now_v7();
        let now = unix_timestamp_millis();
        transaction.execute(
            "INSERT INTO library_sync_bindings(library_uuid,plugin_id,account_uuid,remote_namespace,binding_epoch,actor_epoch,state,transport_generation,created_at,updated_at) VALUES (?1,?2,?3,?1,?4,?5,'restoring',1,?6,?6)",
            params![library_uuid,plugin_id,account_uuid,binding.as_bytes().as_slice(),actor.as_bytes().as_slice(),now],
        )?;
        transaction.execute(
            "UPDATE libraries SET origin_remote_identity=?1,availability='unavailable',updated_at=?2 WHERE library_uuid=?1",
            params![library_uuid,now],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn ensure_sync_provider_ready(&self, account_uuid: &str) -> anyhow::Result<()> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let plugin_id: String = connection.query_row(
            "SELECT plugin_id FROM plugin_accounts WHERE account_uuid=?1 AND auth_state='authenticated'",
            [account_uuid],
            |row| row.get(0),
        )?;
        let ready: i64 = connection.query_row(
            "SELECT COUNT(*) FROM plugin_runtime_state WHERE plugin_id=?1 AND state='ready' AND EXISTS (SELECT 1 FROM json_each(capabilities_json) WHERE value='sync-provider.v1')",
            [&plugin_id],
            |row| row.get(0),
        )?;
        if ready != 1 {
            anyhow::bail!("provider-not-ready");
        }
        Ok(())
    }

    pub fn plugin_id_for_account(&self, account_uuid: &str) -> anyhow::Result<Option<String>> {
        self.connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .query_row(
                "SELECT plugin_id FROM plugin_accounts WHERE account_uuid=?1",
                [account_uuid],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn create_restore_preflight(
        &self,
        request: &serde_json::Value,
        total_items: u64,
        total_bytes: u64,
    ) -> anyhow::Result<SyncRestorePreflight> {
        let now = unix_timestamp_millis();
        let expires_at = now + 10 * 60 * 1000;
        let token = uuid::Uuid::now_v7().to_string();
        self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "INSERT INTO sync_restore_preflights(token,request_json,total_items,total_bytes,expires_at,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![token,serde_json::to_string(request)?,total_items as i64,total_bytes as i64,expires_at,now],
        )?;
        Ok(SyncRestorePreflight {
            preflight_token: token,
            expires_at,
            total_items,
            total_bytes,
        })
    }

    pub fn consume_restore_preflight(
        &self,
        token: &str,
        request: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let request_json = serde_json::to_string(request)?;
        let changed = self.connection.lock().unwrap_or_else(|p| p.into_inner()).execute(
            "UPDATE sync_restore_preflights SET consumed_at=?1 WHERE token=?2 AND request_json=?3 AND consumed_at IS NULL AND expires_at>=?1",
            params![now,token,request_json],
        )?;
        if changed != 1 {
            anyhow::bail!("restore-preflight-expired-or-changed");
        }
        Ok(())
    }

    pub fn mark_restore_catalog_ready(&self, library_uuid: &str) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute("UPDATE library_sync_bindings SET last_success_at=?2,poll_after=?3,updated_at=?2 WHERE library_uuid=?1 AND state='restoring'",params![library_uuid,now,now+30_000])?;
        transaction.execute("UPDATE sync_restore_plans SET state='catalog-ready',phase='restoring-media',catalog_ready=1,updated_at=?2 WHERE library_uuid=?1 AND state='restoring'",params![library_uuid,now])?;
        transaction.execute(
            "UPDATE libraries SET availability='available',updated_at=?2 WHERE library_uuid=?1",
            params![library_uuid, now],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn update_restore_progress(
        &self,
        library_uuid: &str,
        done: u64,
        total: u64,
        bytes_done: u64,
        bytes_total: u64,
        conflict_count: u64,
    ) -> anyhow::Result<()> {
        let state = if done >= total {
            "complete"
        } else {
            "catalog-ready"
        };
        let phase = if done >= total {
            "complete"
        } else {
            "restoring-media"
        };
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE sync_restore_plans SET state=?2,phase=?3,media_items_done=?4,media_items_total=?5,media_bytes_done=?6,media_bytes_total=?7,updated_at=?8 WHERE library_uuid=?1 AND catalog_ready=1 AND state NOT IN ('paused','cancelled','failed')",
            params![library_uuid,state,phase,done as i64,total as i64,bytes_done as i64,bytes_total as i64,unix_timestamp_millis()],
        )?;
        transaction.execute(
            "UPDATE library_sync_bindings SET pending_download_items=?2,pending_download_bytes=?3,conflict_count=?4,updated_at=?5 WHERE library_uuid=?1",
            params![library_uuid, total.saturating_sub(done) as i64, bytes_total.saturating_sub(bytes_done) as i64, conflict_count as i64, unix_timestamp_millis()],
        )?;
        if state == "complete" {
            transaction.execute(
                "UPDATE library_sync_bindings SET state='active',poll_after=?2,updated_at=?3 WHERE library_uuid=?1 AND state='restoring'",
                params![library_uuid, unix_timestamp_millis() + 30_000, unix_timestamp_millis()],
            )?;
        }
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn update_library_conflict_count(
        &self,
        library_uuid: &str,
        conflict_count: u64,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE library_sync_bindings SET conflict_count=?2,updated_at=?3 WHERE library_uuid=?1",
            params![library_uuid, conflict_count as i64, unix_timestamp_millis()],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn fail_restore_plan(&self, restore_uuid: &str, error_code: &str) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE sync_restore_plans SET state='failed',phase='retry-wait',error_code=?2,updated_at=?3 WHERE restore_uuid=?1",
            params![restore_uuid, error_code, now],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn restore_plan(&self, restore_uuid: &str) -> anyhow::Result<Option<SyncRestorePlan>> {
        self.connection.lock().unwrap_or_else(|p|p.into_inner()).query_row("SELECT restore_uuid,operation_uuid,account_uuid,library_uuid,name,managed_root,state,phase,catalog_ready,media_items_done,media_items_total,media_bytes_done,media_bytes_total,error_code,updated_at FROM sync_restore_plans WHERE restore_uuid=?1",[restore_uuid],|row|Ok(SyncRestorePlan{restore_uuid:row.get(0)?,operation_uuid:row.get(1)?,account_uuid:row.get(2)?,library_uuid:row.get(3)?,name:row.get(4)?,managed_root:row.get(5)?,state:row.get(6)?,phase:row.get(7)?,catalog_ready:row.get::<_,i64>(8)?!=0,media_items_done:row.get::<_,i64>(9)?.max(0) as u64,media_items_total:row.get::<_,Option<i64>>(10)?.map(|v|v.max(0) as u64),media_bytes_done:row.get::<_,i64>(11)?.max(0) as u64,media_bytes_total:row.get::<_,Option<i64>>(12)?.map(|v|v.max(0) as u64),error_code:row.get(13)?,updated_at:row.get(14)?})).optional().map_err(Into::into)
    }

    pub fn restore_plan_for_library(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<Option<SyncRestorePlan>> {
        let restore_uuid = self
            .connection
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .query_row(
                "SELECT restore_uuid FROM sync_restore_plans WHERE library_uuid=?1 ORDER BY created_at DESC LIMIT 1",
                [library_uuid],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        restore_uuid
            .map(|restore_uuid| self.restore_plan(&restore_uuid))
            .transpose()
            .map(Option::flatten)
    }

    pub fn restore_plans_for_operation(
        &self,
        operation_uuid: &str,
    ) -> anyhow::Result<Vec<SyncRestorePlan>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare("SELECT restore_uuid,operation_uuid,account_uuid,library_uuid,name,managed_root,state,phase,catalog_ready,media_items_done,media_items_total,media_bytes_done,media_bytes_total,error_code,updated_at FROM sync_restore_plans WHERE operation_uuid=?1 ORDER BY created_at,restore_uuid")?;
        let plans = statement
            .query_map([operation_uuid], |row| {
                Ok(SyncRestorePlan {
                    restore_uuid: row.get(0)?,
                    operation_uuid: row.get(1)?,
                    account_uuid: row.get(2)?,
                    library_uuid: row.get(3)?,
                    name: row.get(4)?,
                    managed_root: row.get(5)?,
                    state: row.get(6)?,
                    phase: row.get(7)?,
                    catalog_ready: row.get::<_, i64>(8)? != 0,
                    media_items_done: row.get::<_, i64>(9)?.max(0) as u64,
                    media_items_total: row.get::<_, Option<i64>>(10)?.map(|v| v.max(0) as u64),
                    media_bytes_done: row.get::<_, i64>(11)?.max(0) as u64,
                    media_bytes_total: row.get::<_, Option<i64>>(12)?.map(|v| v.max(0) as u64),
                    error_code: row.get(13)?,
                    updated_at: row.get(14)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(anyhow::Error::from)?;
        Ok(plans)
    }

    pub fn pending_restore_recovery(&self) -> anyhow::Result<Vec<SyncRestorePlan>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare("SELECT restore_uuid,operation_uuid,account_uuid,library_uuid,name,managed_root,state,phase,catalog_ready,media_items_done,media_items_total,media_bytes_done,media_bytes_total,error_code,updated_at FROM sync_restore_plans WHERE state IN ('planned','restoring','catalog-ready','paused','failed') ORDER BY created_at,restore_uuid")?;
        let values = statement
            .query_map([], |row| {
                Ok(SyncRestorePlan {
                    restore_uuid: row.get(0)?,
                    operation_uuid: row.get(1)?,
                    account_uuid: row.get(2)?,
                    library_uuid: row.get(3)?,
                    name: row.get(4)?,
                    managed_root: row.get(5)?,
                    state: row.get(6)?,
                    phase: row.get(7)?,
                    catalog_ready: row.get::<_, i64>(8)? != 0,
                    media_items_done: row.get::<_, i64>(9)?.max(0) as u64,
                    media_items_total: row.get::<_, Option<i64>>(10)?.map(|v| v.max(0) as u64),
                    media_bytes_done: row.get::<_, i64>(11)?.max(0) as u64,
                    media_bytes_total: row.get::<_, Option<i64>>(12)?.map(|v| v.max(0) as u64),
                    error_code: row.get(13)?,
                    updated_at: row.get(14)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(values)
    }

    pub fn recover_sync_orchestration_journals(&self) -> anyhow::Result<()> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        // A committed reconnect that crashed before Catalog capture stays
        // committed. The manager resumes the preparing binding idempotently;
        // reverting the plan would allow a duplicate epoch to be created.
        // Unbind cutover/reconciliation is resumed through the existing plan;
        // planned rows are intentionally left untouched for the user.
        transaction.execute(
            "UPDATE library_sync_bindings SET poll_after=0,updated_at=?1 WHERE state='unbinding'",
            [now],
        )?;
        // Planned restore rows never exposed a usable Catalog. Mark them failed
        // so startup cleanup can remove any partially-created local copy.
        transaction.execute("UPDATE sync_restore_plans SET state='failed',phase='startup-recovery',error_code='interrupted-before-catalog',updated_at=?1 WHERE state='planned' AND catalog_ready=0", [now])?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn pending_sync_binding_recovery(&self) -> anyhow::Result<Vec<SyncBindingRecovery>> {
        let connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let mut statement = connection.prepare(
            "SELECT b.library_uuid,CASE WHEN p.plan_uuid IS NULL THEN b.state ELSE 'unbinding' END,p.plan_uuid,p.choice FROM library_sync_bindings b LEFT JOIN sync_unbind_plans p ON p.library_uuid=b.library_uuid AND p.state IN ('cutover','reconciling') WHERE b.state IN ('preparing','unbinding') OR p.plan_uuid IS NOT NULL UNION ALL SELECT p.library_uuid,'unbinding',p.plan_uuid,p.choice FROM sync_unbind_plans p LEFT JOIN library_sync_bindings b ON b.library_uuid=p.library_uuid WHERE p.state IN ('cutover','reconciling') AND b.library_uuid IS NULL ORDER BY 1",
        )?;
        let values = statement
            .query_map([], |row| {
                Ok(SyncBindingRecovery {
                    library_uuid: row.get(0)?,
                    state: row.get(1)?,
                    plan_uuid: row.get(2)?,
                    choice: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(values)
    }

    pub fn set_restore_state(
        &self,
        restore_uuid: &str,
        state: &str,
    ) -> anyhow::Result<SyncRestorePlan> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        let (library_uuid, catalog_ready, current_state): (String, i64, String) = transaction
            .query_row(
            "SELECT library_uuid,catalog_ready,state FROM sync_restore_plans WHERE restore_uuid=?1",
            [restore_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if state == "cancelled" && catalog_ready != 0 {
            anyhow::bail!("restore-action-not-allowed");
        }
        if current_state == "cancelled" || current_state == "complete" {
            anyhow::bail!("restore-action-not-allowed");
        }
        if state == "paused" && current_state == "paused" {
            transaction.commit()?;
            drop(connection);
            return self
                .restore_plan(restore_uuid)?
                .ok_or_else(|| anyhow::anyhow!("restore-not-found"));
        }
        if state == "restoring" && current_state != "paused" {
            transaction.commit()?;
            drop(connection);
            return self
                .restore_plan(restore_uuid)?
                .ok_or_else(|| anyhow::anyhow!("restore-not-found"));
        }
        let binding_state = match state {
            "paused" => "paused",
            "restoring" => "restoring",
            "cancelled" => "archived",
            _ => anyhow::bail!("restore-action-not-allowed"),
        };
        transaction.execute(
            "UPDATE sync_restore_plans SET state=?2,phase=CASE WHEN ?2='cancelled' THEN 'cancelled' ELSE phase END,updated_at=?3 WHERE restore_uuid=?1",
            params![restore_uuid,state,now],
        )?;
        transaction.execute(
            "UPDATE library_sync_bindings SET state=?2,poll_after=CASE WHEN ?2='restoring' THEN 0 ELSE poll_after END,updated_at=?3 WHERE library_uuid=?1",
            params![library_uuid,binding_state,now],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        drop(connection);
        self.restore_plan(restore_uuid)?
            .ok_or_else(|| anyhow::anyhow!("restore-not-found"))
    }

    pub fn commit_cancelled_restore_cleanup(
        &self,
        restore_uuid: &str,
        library_uuid: &str,
    ) -> anyhow::Result<SyncRestorePlan> {
        let now = unix_timestamp_millis();
        let mut connection = self.connection.lock().unwrap_or_else(|p| p.into_inner());
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE sync_restore_plans SET state='cancelled',phase='cancelled',updated_at=?2 WHERE restore_uuid=?1 AND library_uuid=?3 AND catalog_ready=0",
            params![restore_uuid, now, library_uuid],
        )?;
        transaction.execute(
            "DELETE FROM libraries WHERE library_uuid=?1",
            [library_uuid],
        )?;
        Self::bump_sync_status_revision(&transaction)?;
        transaction.commit()?;
        drop(connection);
        self.restore_plan(restore_uuid)?
            .ok_or_else(|| anyhow::anyhow!("restore-not-found"))
    }

    pub fn redacted_sync_diagnostics(
        &self,
        library_uuid: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let status = self
            .sync_status_snapshot()?
            .libraries
            .into_iter()
            .find(|v| v.library_uuid == library_uuid)
            .ok_or_else(|| anyhow::anyhow!("library-not-found"))?;
        Ok(
            serde_json::json!({"generatedAt":unix_timestamp_millis(),"libraryUuid":status.library_uuid,"lifecycle":status.lifecycle,"phase":status.phase,"catalogReadiness":status.catalog_readiness,"lastErrorCode":status.last_error_code,"pendingUploadItems":status.pending_upload_items,"pendingDownloadItems":status.pending_download_items,"conflictCount":status.conflict_count,"redacted":true}),
        )
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

    #[test]
    fn restore_library_becomes_available_only_after_catalog_ready() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "fake", "account");
        let now = unix_timestamp_millis();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES ('library','root','Library','local','root','ready','local-only','available',1,'marker',?1,?1)",
            [now],
        ).unwrap();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO sync_orchestration_operations(operation_uuid,kind,library_uuid,request_json,state,phase,created_at,updated_at) VALUES ('operation','restore',NULL,'{}','running','preparing',?1,?1)",
            [now],
        ).unwrap();
        registry.connection.lock().unwrap().execute(
            "INSERT INTO sync_restore_plans(restore_uuid,operation_uuid,account_uuid,library_uuid,name,managed_root,descriptor_sha256,state,phase,catalog_ready,created_at,updated_at) VALUES ('restore','operation','account','library','Library','root',zeroblob(32),'restoring','restoring-metadata',0,?1,?1)",
            [now],
        ).unwrap();

        registry
            .begin_restore_binding("library", "account")
            .unwrap();
        assert_eq!(
            registry.library("library").unwrap().unwrap().availability,
            "unavailable"
        );

        registry.mark_restore_catalog_ready("library").unwrap();
        assert_eq!(
            registry.library("library").unwrap().unwrap().availability,
            "available"
        );

        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    fn temp_registry_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "megle-host-registry-{}.sqlite",
            uuid::Uuid::now_v7()
        ))
    }

    fn register_test_library(registry: &HostRegistry, library_uuid: &str) {
        let descriptor = LibraryDescriptor {
            library_uuid: library_uuid.into(),
            root_uuid: uuid::Uuid::now_v7().to_string(),
            name: "Test Library".into(),
            icon: None,
            color: None,
            local_data_dir: std::env::temp_dir().join(format!("megle-data-{library_uuid}")),
            managed_root: std::env::temp_dir().join(format!("megle-root-{library_uuid}")),
            lifecycle_state: "ready".into(),
            binding_state: "local-only".into(),
            availability: "available".into(),
            marker_identity: "marker".into(),
        };
        registry
            .commit_creation("test-creation", &descriptor)
            .unwrap();
    }

    #[test]
    fn orchestration_status_and_preflight_tokens_are_durable_and_idempotent() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        register_test_plugin(&registry, "com.megle.sync", "account-sync");
        register_test_library(&registry, "library-sync");

        let before = registry.sync_status_snapshot().unwrap();
        assert_eq!(before.libraries[0].lifecycle, "local-only");
        assert_eq!(before.libraries[0].allowed_actions, vec!["enable"]);

        let token = registry
            .create_sync_preflight_token("library-sync", "com.megle.sync", "account-sync", None)
            .unwrap();
        registry
            .consume_sync_preflight_token(&token, "library-sync", "com.megle.sync", "account-sync")
            .unwrap();
        assert!(registry
            .consume_sync_preflight_token(&token, "library-sync", "com.megle.sync", "account-sync",)
            .is_err());

        let operation = registry
            .begin_sync_operation_once(
                "operation-sync",
                Some("library-sync"),
                "sync-now",
                "active",
                &serde_json::json!({}),
            )
            .unwrap();
        let repeated = registry
            .begin_sync_operation_once(
                "operation-sync",
                Some("library-sync"),
                "sync-now",
                "active",
                &serde_json::json!({}),
            )
            .unwrap();
        assert!(operation.1);
        assert!(!repeated.1);
        assert_eq!(operation.0.operation_uuid, repeated.0.operation_uuid);
        assert!(registry
            .begin_sync_operation_once(
                "operation-sync",
                Some("library-sync"),
                "pause",
                "active",
                &serde_json::json!({}),
            )
            .is_err());
        assert!(registry.sync_status_snapshot().unwrap().revision > before.revision);

        drop(registry);
        let reopened = HostRegistry::open(&path).unwrap();
        assert!(reopened.sync_status_snapshot().unwrap().revision > before.revision);
        drop(reopened);
        let _ = std::fs::remove_file(path);
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
    fn unbind_reconcile_failure_is_durable_and_restart_retryable() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let now = unix_timestamp_millis();
        register_test_plugin(&registry, "fake", "account");
        registry.connection.lock().unwrap().execute("INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES ('library','root','Library','local','root','ready','local-only','available',1,'marker',?1,?1)",[now]).unwrap();
        let (binding, _) = registry
            .begin_sync_binding("library", "fake", "account")
            .unwrap();
        registry
            .record_provider_receipt(
                "library",
                binding,
                1,
                "library-descriptor",
                None,
                &[9; 32],
                "created",
            )
            .unwrap();
        let plan = registry.create_unbind_plan("library").unwrap();
        registry
            .advance_unbind_plan(&plan.plan_uuid, "keep-local-fork", "cutover")
            .unwrap();
        registry.begin_unbind("library", binding).unwrap();
        registry.commit_local_unbind("library").unwrap();
        registry
            .advance_unbind_plan(&plan.plan_uuid, "keep-local-fork", "reconciling")
            .unwrap();
        registry
            .record_unbind_reconcile_failure(&plan.plan_uuid, "provider_unavailable", 0)
            .unwrap();
        drop(registry);
        let reopened = HostRegistry::open(&path).unwrap();
        let pending = reopened.pending_unbind_reconciliations().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].descriptor_sha256, Some([9; 32]));
        assert_eq!(
            reopened.sync_status_snapshot().unwrap().libraries[0].lifecycle,
            "unbinding"
        );
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn restore_preflight_token_is_exactly_once_and_payload_bound() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let request = serde_json::json!({"items":[{"libraryUuid":"a","managedRoot":"D:/A"}]});
        let preflight = registry.create_restore_preflight(&request, 1, 42).unwrap();
        assert!(registry
            .consume_restore_preflight(&preflight.preflight_token, &serde_json::json!({"items":[]}))
            .is_err());
        registry
            .consume_restore_preflight(&preflight.preflight_token, &request)
            .unwrap();
        assert!(registry
            .consume_restore_preflight(&preflight.preflight_token, &request)
            .is_err());
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reconnect_commit_fault_does_not_consume_preview_or_create_binding() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let now = unix_timestamp_millis();
        register_test_plugin(&registry, "fake", "account");
        registry.connection.lock().unwrap().execute("INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,origin_remote_identity,created_at,updated_at) VALUES ('library','root','Library','local','root','ready','local-only','available',1,'marker','library',?1,?1)",[now]).unwrap();
        let origin_binding = uuid::Uuid::now_v7();
        registry.connection.lock().unwrap().execute("INSERT INTO sync_unbind_plans(plan_uuid,library_uuid,binding_epoch,transport_generation,cutover_actor_seq,remote_watermark,pending_count,published_count,unpublished_count,origin_identity,state,choice,created_at,updated_at,plugin_id,account_uuid,remote_namespace,descriptor_sha256,base_remote_object_count,remote_visibility) VALUES ('origin-unbind','library',?1,2,8,8,0,8,0,'library','complete','keep-local-fork',?2,?2,'fake','account','library',?3,5,'visible')",params![origin_binding.as_bytes().as_slice(),now,&[4_u8;32]]).unwrap();
        let plan = registry
            .create_reconnect_plan(
                "library",
                "account",
                "library",
                3,
                ReconnectPreviewFacts {
                    remote_exists: true,
                    remote_compatible: true,
                    remote_object_count: 5,
                    remote_descriptor_sha256: Some([4; 32]),
                    remote_binding_epoch: Some(origin_binding.to_string()),
                },
            )
            .unwrap();
        assert!(plan.merge_allowed);
        registry.connection.lock().unwrap().execute_batch("CREATE TRIGGER fail_reconnect BEFORE INSERT ON library_sync_bindings BEGIN SELECT RAISE(ABORT,'fault-reconnect'); END;").unwrap();
        assert!(registry
            .commit_reconnect_plan(&plan.plan_uuid, "merge-local-fork")
            .is_err());
        let connection = registry.connection.lock().unwrap();
        let plan_state: String = connection
            .query_row(
                "SELECT state FROM sync_reconnect_plans WHERE plan_uuid=?1",
                [&plan.plan_uuid],
                |row| row.get(0),
            )
            .unwrap();
        let bindings: i64 = connection
            .query_row("SELECT COUNT(*) FROM library_sync_bindings", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(plan_state, "planned");
        assert_eq!(bindings, 0);
        drop(connection);
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn restore_plan_fault_leaves_no_partial_journal() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let now = unix_timestamp_millis();
        register_test_plugin(&registry, "fake", "account");
        registry.connection.lock().unwrap().execute("INSERT INTO provider_remote_libraries(account_uuid,library_uuid,descriptor_size,descriptor_sha256,account_generation,updated_at,name,compatible) VALUES ('account','remote',1,zeroblob(32),1,?1,'Remote',1)",[now]).unwrap();
        registry.connection.lock().unwrap().execute("INSERT INTO sync_orchestration_operations(operation_uuid,kind,state,phase,request_json,created_at,updated_at) VALUES ('operation','restore','running','catalog-restore','{}',?1,?1)",[now]).unwrap();
        registry.connection.lock().unwrap().execute_batch("CREATE TRIGGER fail_restore BEFORE INSERT ON sync_restore_plans BEGIN SELECT RAISE(ABORT,'fault-restore'); END;").unwrap();
        assert!(registry
            .start_restore_plan(StartRestoreRequest {
                operation_uuid: "operation",
                account_uuid: "account",
                library_uuid: "remote",
                managed_root: Path::new("D:/Remote")
            })
            .is_err());
        let count: i64 = registry
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sync_restore_plans", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn account_switch_fault_rolls_back_all_library_pauses() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let now = unix_timestamp_millis();
        register_test_plugin(&registry, "fake", "account");
        for suffix in ["a", "b"] {
            registry.connection.lock().unwrap().execute("INSERT INTO libraries(library_uuid,root_uuid,name,local_data_dir,managed_root,lifecycle_state,binding_state,availability,marker_version,marker_identity,created_at,updated_at) VALUES (?1,?2,?1,?1,?1,'ready','local-only','available',1,?1,?3,?3)",params![suffix,format!("root-{suffix}"),now]).unwrap();
            registry
                .begin_sync_binding(suffix, "fake", "account")
                .unwrap();
        }
        registry.connection.lock().unwrap().execute_batch("CREATE TRIGGER fail_account_switch BEFORE UPDATE ON library_sync_bindings WHEN NEW.state='paused' BEGIN SELECT RAISE(ABORT,'fault-account-switch'); END;").unwrap();
        assert!(registry.sign_out_plugin_account("fake", 1).is_err());
        let connection = registry.connection.lock().unwrap();
        let signed_out: i64 = connection
            .query_row(
                "SELECT auth_state='signed-out' FROM plugin_accounts WHERE plugin_id='fake'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let paused: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM library_sync_bindings WHERE state='paused'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((signed_out, paused), (0, 0));
        drop(connection);
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

        let fences = registry
            .sign_out_plugin_account("fake", 1)
            .unwrap()
            .unwrap();
        assert_eq!(fences.len(), 1);
        assert_eq!(fences[0].binding_epoch, binding);
        assert_eq!(fences[0].next_transport_generation, 2);
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
        let binding_state: (String, String, i64) = connection
            .query_row(
                "SELECT state,last_error_code,transport_generation FROM library_sync_bindings WHERE library_uuid='library'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            binding_state,
            ("paused".into(), "authorization-required".into(), 2)
        );
        drop(connection);
        drop(registry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_grant_requires_reauthorization_and_fences_remote_state() {
        let path = temp_registry_path();
        let registry = HostRegistry::open(&path).unwrap();
        let now = unix_timestamp_millis();
        register_test_plugin(&registry, "fake", "account");
        registry
            .set_plugin_account_secret("fake", Some("credential"), "authenticated", 1)
            .unwrap();
        let fingerprint = registry.plugin_runtime_launches().unwrap()[0].fingerprint();
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
            .require_plugin_reauthorization_for_launch("fake", &fingerprint)
            .unwrap());
        assert!(!registry
            .require_plugin_reauthorization_for_launch("fake", &fingerprint)
            .unwrap());
        let connection = registry.connection.lock().unwrap();
        let account: (Option<String>, String, i64) = connection
            .query_row(
                "SELECT secret_ref,auth_state,generation FROM plugin_accounts WHERE plugin_id='fake'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(account, (None, "authorization-required".into(), 2));
        let binding_state: (String, i64) = connection
            .query_row(
                "SELECT state,transport_generation FROM library_sync_bindings WHERE library_uuid='library'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(binding_state, ("paused".into(), 2));
        let runtime: (String, Option<String>) = connection
            .query_row(
                "SELECT state,last_error_code FROM plugin_runtime_state WHERE plugin_id='fake'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            runtime,
            ("degraded".into(), Some("authorization-required".into()))
        );
        for table in [
            "provider_binding_cursors",
            "provider_account_root_cursors",
            "provider_remote_libraries",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
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
        let discovered = DiscoveredRemoteLibrary {
            metadata,
            name: Some("Remote library".into()),
            icon: Some("folder".into()),
            media_count: Some(12),
            logical_bytes: Some(42_000),
            last_synced_at: Some(1234),
            compatible: true,
            error_code: None,
        };
        registry
            .commit_provider_root_page(
                "account",
                transport.account_generation,
                &transport.runtime_session_id,
                Some("cursor-zero"),
                "page-one",
                std::slice::from_ref(&discovered),
            )
            .unwrap();
        let transport = registry.provider_root_transports().unwrap().remove(0);
        assert_eq!(transport.cursor.as_deref(), Some("cursor-zero"));
        assert_eq!(transport.page_token.as_deref(), Some("page-one"));
        registry
            .commit_provider_root_cursor(
                "account",
                transport.account_generation,
                &transport.runtime_session_id,
                Some("cursor-one"),
                std::slice::from_ref(&discovered),
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
        let remote = registry.list_remote_libraries().unwrap().remove(0);
        assert_eq!(remote.name, "Remote library");
        assert_eq!(remote.icon.as_deref(), Some("folder"));
        assert_eq!(remote.media_count, Some(12));
        assert_eq!(remote.logical_bytes, Some(42_000));
        assert_eq!(remote.last_synced_at, Some(1234));
        assert!(remote.compatible);
        assert_eq!(remote.error_code, None);
        assert!(registry
            .commit_provider_root_cursor(
                "account",
                transport.account_generation + 1,
                &transport.runtime_session_id,
                Some("stale"),
                &[],
            )
            .is_err());
        assert!(registry
            .commit_provider_root_cursor(
                "account",
                transport.account_generation,
                "stale-session",
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
